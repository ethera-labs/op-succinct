//! This module contains the prologue phase of the client program, pulling in the boot
//! information, which is passed to the zkVM a public inputs to be verified on chain.

use alloy_primitives::B256;
use alloy_sol_types::sol;
use kona_genesis::RollupConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ABI encoding of AggregationOutputs is 8 * 32-byte slots.
pub const AGGREGATION_OUTPUTS_SIZE: usize = 8 * 32;

/// Hash the serialized rollup config using SHA256. Note: The rollup config is never unrolled
/// on-chain, so switching to a different hash function is not a concern, as long as the config hash
/// is consistent with the one on the contract.
pub fn hash_rollup_config(config: &RollupConfig) -> B256 {
    let serialized_config = serde_json::to_string_pretty(config).unwrap();

    // Create a SHA256 hasher
    let mut hasher = Sha256::new();

    // Hash the serialized config
    hasher.update(serialized_config.as_bytes());

    // Finalize and convert to B256
    let hash = hasher.finalize();
    B256::from_slice(&hash)
}

sol! {
    #[derive(Debug, Serialize, Deserialize)]
    struct MailboxInfoStruct {
        bytes32[] inbox_chains;
        bytes32[] outbox_chains;
        bytes32[] inbox_roots;
        bytes32[] outbox_roots;
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct BootInfoStruct {
        bytes32 l1Head;
        bytes32 l2PreRoot;
        bytes32 l2PostRoot;
        uint64 l2BlockNumber;
        bytes32 rollupConfigHash;
        bytes32 mailboxRoot;
    }
}
