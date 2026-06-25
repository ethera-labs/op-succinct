//! ETHERA: submit aggregation proofs to the shared publisher service (off-chain settlement path).

use alloy_primitives::{Address, B256};
use anyhow::Result;
use op_succinct_client_utils::types::AggregationOutputs;
use reqwest::Url;
use serde::Serialize;
use tracing::{debug, error};

#[derive(Serialize)]
struct SubmitReq {
    superblock_number: u64,
    superblock_hash: B256,
    chain_id: u32,
    prover_address: Address,
    l1_head: B256,
    aggregation_outputs: AggregationOutputs,
    l2_start_block: u64,
    /// SHA256 hash of the SP1 aggregation verifying key, as eight u32 words packed into B256.
    agg_vkey_hash: B256,
    /// Publisher currently validates this field is present before forwarding to superblock-prover.
    agg_vk: B256,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<Vec<u8>>,
}

pub fn build_aggregation_outputs(
    l1_head: B256,
    l2_pre_root: B256,
    l2_post_root: B256,
    l2_block_number: u64,
    rollup_config_hash: B256,
    mailbox_root: B256,
    multi_block_vkey: B256,
    prover_address: Address,
) -> AggregationOutputs {
    AggregationOutputs {
        l1Head: l1_head,
        l2PreRoot: l2_pre_root,
        l2PostRoot: l2_post_root,
        l2BlockNumber: l2_block_number,
        rollupConfigHash: rollup_config_hash,
        mailboxRoot: mailbox_root,
        multiBlockVKey: multi_block_vkey,
        proverAddress: prover_address,
    }
}

/// Post an aggregation submission to the shared publisher.
pub async fn submit_to_publisher(
    endpoint: &Url,
    superblock_number: u64,
    superblock_hash: B256,
    chain_id_l2: u32,
    prover_address: Address,
    l1_head: B256,
    aggregation_outputs: AggregationOutputs,
    l2_start_block: u64,
    agg_vkey_hash: B256,
    proof_bytes: Option<&[u8]>,
) -> Result<()> {
    debug!(endpoint = %endpoint, "Creating HTTP client for publisher request");
    let client = reqwest::Client::new();

    let body = SubmitReq {
        superblock_number,
        superblock_hash,
        chain_id: chain_id_l2,
        prover_address,
        l1_head,
        aggregation_outputs,
        l2_start_block,
        agg_vkey_hash,
        agg_vk: agg_vkey_hash,
        proof: proof_bytes.map(|p| p.to_vec()),
    };

    let resp = client.post(endpoint.clone()).json(&body).send().await.map_err(|e| {
        error!(
            endpoint = %endpoint,
            error = %e,
            is_timeout = e.is_timeout(),
            is_connect = e.is_connect(),
            is_dns = e.to_string().contains("dns"),
            "HTTP client request failed"
        );

        if e.is_timeout() {
            anyhow::anyhow!("request timed out to publisher at {}", endpoint)
        } else if e.is_connect() {
            anyhow::anyhow!(
                "connection failed to publisher at {} (service may be down or unreachable)",
                endpoint
            )
        } else if e.to_string().contains("dns") {
            anyhow::anyhow!(
                "DNS resolution failed for publisher at {} (check hostname/service)",
                endpoint
            )
        } else {
            anyhow::anyhow!("failed to send request to publisher at {}: {}", endpoint, e)
        }
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("publisher responded with {}: {}", status, text);
    }

    Ok(())
}
