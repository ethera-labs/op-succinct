//! ETHERA: sidecar shared-publisher relay + mailbox helpers on [`Proposer`].

use super::*;

impl<P, H: OPSuccinctHost> Proposer<P, H>
where
    P: Provider + 'static + Clone,
{
    pub(super) fn uses_shared_publisher(&self) -> bool {
        self.driver_config.publisher_url.is_some()
    }

    pub(super) fn expected_range_mode_for_aggregation(&self) -> RequestMode {
        if self.requester_config.mock {
            RequestMode::Mock
        } else {
            RequestMode::Real
        }
    }

    pub(super) async fn latest_known_proposed_block_number(&self) -> Result<u64> {
        if self.uses_shared_publisher() {
            let latest_relayed = self
                .driver_config
                .driver_db_client
                .get_latest_relayed_block_number(
                    self.requester_config.l1_chain_id,
                    self.requester_config.l2_chain_id,
                )
                .await
                .context("failed to load latest relayed aggregation block from database")?;

            let latest_relayed = latest_relayed
                .map(u64::try_from)
                .transpose()
                .context("latest relayed block number is negative")?;

            return Ok(latest_relayed
                .unwrap_or(self.requester_config.min_l2_block)
                .max(self.requester_config.min_l2_block));
        }

        let onchain = get_latest_proposed_block_number(
            self.contract_config.l2oo_address,
            self.driver_config.fetcher.as_ref(),
        )
        .await
        .context("failed to query latest proposed block from L2 output oracle")?;

        Ok(onchain.max(self.requester_config.min_l2_block))
    }

    pub(super) async fn cancel_invalid_unrequested_aggregations(&self) -> Result<()> {
        let mut unrequested = self
            .driver_config
            .driver_db_client
            .fetch_requests_by_status(
                RequestStatus::Unrequested,
                &self.program_config.commitments,
                self.requester_config.l1_chain_id,
                self.requester_config.l2_chain_id,
            )
            .await?;
        unrequested.retain(|request| request.req_type == RequestType::Aggregation);

        if unrequested.is_empty() {
            return Ok(());
        }

        let mut completed = self
            .driver_config
            .driver_db_client
            .fetch_requests_by_status(
                RequestStatus::Complete,
                &self.program_config.commitments,
                self.requester_config.l1_chain_id,
                self.requester_config.l2_chain_id,
            )
            .await?;
        let expected_mode = self.expected_range_mode_for_aggregation();
        completed.retain(|request| {
            request.req_type == RequestType::Range && request.mode == expected_mode
        });
        completed.sort_by_key(|request| request.start_block);

        for aggregation in unrequested {
            let mut range_proofs = completed
                .iter()
                .filter(|request| {
                    request.start_block >= aggregation.start_block &&
                        request.end_block <= aggregation.end_block
                })
                .cloned()
                .collect::<Vec<_>>();
            range_proofs.sort_by_key(|request| request.start_block);

            if !self.validate_aggregation_request(&range_proofs, &aggregation).await {
                self.driver_config
                    .driver_db_client
                    .update_request_status(aggregation.id, RequestStatus::Cancelled)
                    .await?;
                info!(
                    request_id = aggregation.id,
                    start_block = aggregation.start_block,
                    end_block = aggregation.end_block,
                    "Cancelled invalid aggregation request"
                );
            }
        }

        Ok(())
    }

    pub(super) async fn checkpoint_l1_block_hash(
        &self,
        existing_request: Option<(Vec<u8>, i64)>,
        latest_proposed_block_number: i64,
        highest_proven_contiguous_block_number: i64,
    ) -> Result<(B256, i64)> {
        // Largest range-proof `l1Head` in the batch. The checkpoint must cover it (see
        // `select_checkpoint_block_number`), so it gates both reuse and fresh selection below.
        // `None` when no completed range proof has a recorded l1 head, in which case `safe` is a
        // sufficient floor.
        let batch_max_l1_head = self
            .driver_config
            .driver_db_client
            .get_max_l1_head_block_number_for_range(
                latest_proposed_block_number,
                highest_proven_contiguous_block_number,
                &self.program_config.commitments,
                self.requester_config.l1_chain_id,
                self.requester_config.l2_chain_id,
            )
            .await?
            .map(u64::try_from)
            .transpose()
            .context("Range proof l1_head_block_number is negative")?;

        // Reuse an existing checkpoint only while it still matches the on-chain mapping and still
        // covers the batch's max l1Head.
        let reuse_checkpoint = if let Some((hash, block_number)) = existing_request {
            let existing_hash = B256::from_slice(&hash);
            let existing_number = u64::try_from(block_number)
                .context("Existing checkpointed L1 block number is negative")?;
            let onchain_hash = self
                .contract_config
                .l2oo_contract
                .historicBlockHashes(U256::from(existing_number))
                .call()
                .await?
                .0;

            if onchain_hash == B256::ZERO || onchain_hash != existing_hash {
                warn!(block_number, "Checkpoint mismatch or missing on-chain; re-checkpointing");
                None
            } else if batch_max_l1_head.is_some_and(|max| existing_number < max) {
                warn!(
                    block_number,
                    ?batch_max_l1_head,
                    "Cached checkpoint is below the batch's max l1Head; re-checkpointing"
                );
                None
            } else {
                Some((existing_hash, block_number))
            }
        } else {
            None
        };

        if let Some(checkpoint) = reuse_checkpoint {
            return Ok(checkpoint);
        }

        // Checkpoint a reorg-stable `safe` head, floored at the batch's max l1Head so the
        // aggregation guest's header walk covers every range proof.
        let safe_header = self.driver_config.fetcher.get_l1_header(BlockId::safe()).await?;
        let checkpoint_number =
            select_checkpoint_block_number(safe_header.number, batch_max_l1_head);
        let checkpoint_header = if checkpoint_number == safe_header.number {
            safe_header
        } else {
            self.driver_config.fetcher.get_l1_header(checkpoint_number.into()).await?
        };

        let transaction_request = self
            .contract_config
            .l2oo_contract
            .checkpointBlockHash(U256::from(checkpoint_header.number))
            .into_transaction_request();

        let receipt = self
            .driver_config
            .signer
            .send_transaction_request_with_timeout(
                self.driver_config.fetcher.as_ref().rpc_config.l1_rpc.clone(),
                transaction_request,
                self.requester_config.tx_confirmation_timeout,
            )
            .await?;

        if !receipt.status() {
            return Err(anyhow!("Checkpoint block transaction reverted: {:?}", receipt));
        }

        info!(block_number = checkpoint_header.number, "Checkpointed L1 block hash");
        Ok((checkpoint_header.hash_slow(), checkpoint_header.number as i64))
    }

    /// Relay a completed aggregation proof to the shared publisher service.
    pub(super) async fn relay_aggregation_proof_to_shared_publisher(
        &self,
        completed_agg_proof: &OPSuccinctRequest,
    ) -> Result<()> {
        let publisher_url = self
            .driver_config
            .publisher_url
            .as_ref()
            .context("shared publisher URL is not configured")?;

        let start_block = completed_agg_proof.start_block as u64;
        let end_block = completed_agg_proof.end_block as u64;
        let pre_output = self.driver_config.fetcher.get_l2_output_at_block(start_block).await?;
        let output = self.driver_config.fetcher.get_l2_output_at_block(end_block).await?;
        let post_root = B256::from(output.output_root.0);

        let l1_head = B256::from_slice(
            completed_agg_proof
                .checkpointed_l1_block_hash
                .as_ref()
                .context("aggregation proof must have checkpointed L1 block hash")?,
        );

        let (mailbox_root, _mailbox_info) = self
            .fetch_mailbox_data(completed_agg_proof.end_block, completed_agg_proof.l2_chain_id)
            .await?;

        let prover_address = Address::from_slice(
            completed_agg_proof
                .prover_address
                .as_ref()
                .context("prover address must be set for aggregation proofs")?,
        );

        let aggregation_outputs = build_aggregation_outputs(
            l1_head,
            B256::from(pre_output.output_root.0),
            post_root,
            end_block,
            self.program_config.commitments.rollup_config_hash,
            mailbox_root,
            self.program_config.commitments.range_vkey_commitment,
            prover_address,
        );

        submit_to_publisher(
            publisher_url,
            end_block,
            post_root,
            self.requester_config.l2_chain_id as u32,
            prover_address,
            l1_head,
            aggregation_outputs,
            start_block,
            self.program_config.commitments.agg_vkey_hash,
            completed_agg_proof.proof.as_deref(),
        )
        .await?;

        info!(
            start_block,
            end_block,
            publisher = %publisher_url,
            "Published aggregation proof to shared publisher"
        );

        Ok(())
    }

    pub(super) async fn fetch_mailbox_data(
        &self,
        end_block: i64,
        l2_chain_id: i64,
    ) -> Result<(B256, MailboxInfoStruct)> {
        let mailbox_data = self
            .driver_config
            .driver_db_client
            .fetch_mailbox_store(
                end_block,
                self.requester_config.l1_chain_id,
                l2_chain_id,
                RequestType::Range,
                &self.program_config.commitments,
            )
            .await?;

        let Some((inbox_chains, outbox_chains, inbox_roots, outbox_roots, mailbox_root)) =
            mailbox_data
        else {
            return Ok((
                B256::ZERO,
                MailboxInfoStruct {
                    inbox_chains: Vec::new(),
                    outbox_chains: Vec::new(),
                    inbox_roots: Vec::new(),
                    outbox_roots: Vec::new(),
                },
            ));
        };

        let root = mailbox_root
            .filter(|bytes| bytes.len() == 32)
            .map(|bytes| B256::from_slice(&bytes))
            .unwrap_or(B256::ZERO);

        let to_b256_vec = |values: Option<Vec<Vec<u8>>>| {
            values
                .unwrap_or_default()
                .into_iter()
                .filter(|bytes| bytes.len() == 32)
                .map(|bytes| B256::from_slice(&bytes))
                .collect::<Vec<_>>()
        };

        Ok((
            root,
            MailboxInfoStruct {
                inbox_chains: to_b256_vec(inbox_chains),
                outbox_chains: to_b256_vec(outbox_chains),
                inbox_roots: to_b256_vec(inbox_roots),
                outbox_roots: to_b256_vec(outbox_roots),
            },
        ))
    }
}
