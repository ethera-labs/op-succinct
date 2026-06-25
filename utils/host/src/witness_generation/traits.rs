use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use kona_preimage::{HintWriter, NativeChannel, OracleReader};
use kona_proof::{
    l1::{OracleBlobProvider, OracleL1ChainProvider},
    l2::OracleL2ChainProvider,
    CachingOracle,
};
use op_succinct_client_utils::witness::{
    executor::{get_inputs_for_pipeline, WitnessExecutor},
    preimage_store::PreimageStore,
    BlobData, EtheraSidecarMailboxStore, WitnessData,
};
use sp1_sdk::SP1Stdin;
use tracing::{info, warn};

use crate::witness_generation::{
    ethera_sidecar::EtheraSidecarMailboxSource, OnlineBlobStore, PreimageWitnessCollector,
};

pub type DefaultOracleBase = CachingOracle<OracleReader<NativeChannel>, HintWriter<NativeChannel>>;

#[async_trait]
pub trait WitnessGenerator {
    type WitnessData: WitnessData;
    type WitnessExecutor: WitnessExecutor<
            O = PreimageWitnessCollector<DefaultOracleBase>,
            B = OnlineBlobStore<OracleBlobProvider<DefaultOracleBase>>,
            L1 = OracleL1ChainProvider<PreimageWitnessCollector<DefaultOracleBase>>,
            L2 = OracleL2ChainProvider<PreimageWitnessCollector<DefaultOracleBase>>,
        > + Sync
        + Send;

    fn get_executor(&self) -> &Self::WitnessExecutor;

    async fn run(
        &self,
        preimage_chan: NativeChannel,
        hint_chan: NativeChannel,
    ) -> Result<Self::WitnessData> {
        let (witness, _) = self.run_with_ethera_sidecar_mailbox(preimage_chan, hint_chan).await?;
        Ok(witness)
    }

    async fn run_with_ethera_sidecar_mailbox(
        &self,
        preimage_chan: NativeChannel,
        hint_chan: NativeChannel,
    ) -> Result<(Self::WitnessData, EtheraSidecarMailboxStore)> {
        let preimage_witness_store = Arc::new(Mutex::new(PreimageStore::default()));
        let blob_data = Arc::new(Mutex::new(BlobData::default()));

        let preimage_oracle = Arc::new(CachingOracle::new(
            2048,
            OracleReader::new(preimage_chan),
            HintWriter::new(hint_chan),
        ));
        let blob_provider = OracleBlobProvider::new(preimage_oracle.clone());

        let oracle = Arc::new(PreimageWitnessCollector {
            preimage_oracle: preimage_oracle.clone(),
            preimage_witness_store: preimage_witness_store.clone(),
        });
        let beacon = OnlineBlobStore { provider: blob_provider.clone(), store: blob_data.clone() };

        let (boot_info, input) = get_inputs_for_pipeline(oracle.clone()).await?;
        if let Some((cursor, l1_provider, l2_provider)) = input {
            let rollup_config = Arc::new(boot_info.rollup_config.clone());
            let l1_config = Arc::new(boot_info.l1_config.clone());
            let pipeline = self
                .get_executor()
                .create_pipeline(
                    rollup_config,
                    l1_config,
                    cursor.clone(),
                    oracle.clone(),
                    beacon,
                    l1_provider.clone(),
                    l2_provider.clone(),
                )
                .await
                .unwrap();
            self.get_executor().run(boot_info.clone(), pipeline, cursor, l2_provider).await?;
        }

        let mailbox_store = match EtheraSidecarMailboxSource::from_env()? {
            Some(source) => match source.load(boot_info.claimed_l2_block_number).await {
                Ok(store) => {
                    info!(
                        block_number = boot_info.claimed_l2_block_number,
                        inbox_chain_count = store.inbox_chains.len(),
                        outbox_chain_count = store.outbox_chains.len(),
                        inbox_root_count = store.inbox_roots.len(),
                        outbox_root_count = store.outbox_roots.len(),
                        "Loaded Ethera sidecar mailbox store"
                    );
                    store
                }
                Err(error) => {
                    warn!(
                        block_number = boot_info.claimed_l2_block_number,
                        %error,
                        "Failed to load Ethera sidecar mailbox store"
                    );
                    EtheraSidecarMailboxStore::default()
                }
            },
            None => EtheraSidecarMailboxStore::default(),
        };

        let witness = Self::WitnessData::from_parts(
            preimage_witness_store.lock().unwrap().clone(),
            blob_data.lock().unwrap().clone(),
            mailbox_store.clone(),
        );

        Ok((witness, mailbox_store))
    }

    fn get_sp1_stdin(&self, witness: Self::WitnessData) -> Result<SP1Stdin>;
}
