use std::collections::BTreeSet;

use alloy_primitives::{keccak256, B256};
use kzg_rs::Bytes32;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Debug, Default, Serialize, Deserialize, rkyv::Serialize, rkyv::Archive, rkyv::Deserialize,
)]
pub struct EtheraSidecarMailboxStore {
    pub inbox_chains: Vec<Bytes32>,
    pub outbox_chains: Vec<Bytes32>,
    pub inbox_roots: Vec<Bytes32>,
    pub outbox_roots: Vec<Bytes32>,
}

impl EtheraSidecarMailboxStore {
    pub fn new(
        inbox_chains: Vec<Bytes32>,
        outbox_chains: Vec<Bytes32>,
        inbox_roots: Vec<Bytes32>,
        outbox_roots: Vec<Bytes32>,
    ) -> Self {
        Self { inbox_chains, outbox_chains, inbox_roots, outbox_roots }
    }

    fn inbox_chain_ids(&self) -> Vec<u64> {
        decode_chain_ids(&self.inbox_chains)
    }

    fn outbox_chain_ids(&self) -> Vec<u64> {
        decode_chain_ids(&self.outbox_chains)
    }

    pub fn db_columns(
        &self,
    ) -> (Option<Vec<Vec<u8>>>, Option<Vec<Vec<u8>>>, Option<Vec<Vec<u8>>>, Option<Vec<Vec<u8>>>)
    {
        (
            bytes32_column(&self.inbox_chains),
            bytes32_column(&self.outbox_chains),
            bytes32_column(&self.inbox_roots),
            bytes32_column(&self.outbox_roots),
        )
    }
}

pub fn compute_ethera_sidecar_mailbox_root(mailbox_store: &EtheraSidecarMailboxStore) -> B256 {
    let inbox_chain_ids = mailbox_store.inbox_chain_ids();
    let outbox_chain_ids = mailbox_store.outbox_chain_ids();
    let mut chain_ids = BTreeSet::new();
    chain_ids.extend(inbox_chain_ids.iter().copied());
    chain_ids.extend(outbox_chain_ids.iter().copied());

    let mut bytes = Vec::with_capacity(15 + chain_ids.len() * 72);
    bytes.extend_from_slice(b"MAILBOX");
    bytes.extend_from_slice(&(chain_ids.len() as u64).to_be_bytes());

    for chain_id in chain_ids {
        bytes.extend_from_slice(&chain_id.to_be_bytes());
        bytes.extend_from_slice(&root_for_chain(
            chain_id,
            &inbox_chain_ids,
            &mailbox_store.inbox_roots,
        ));
        bytes.extend_from_slice(&root_for_chain(
            chain_id,
            &outbox_chain_ids,
            &mailbox_store.outbox_roots,
        ));
    }

    B256::from(keccak256(&bytes))
}

fn decode_chain_ids(values: &[Bytes32]) -> Vec<u64> {
    values
        .iter()
        .map(|value| {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&value.0[24..]);
            u64::from_be_bytes(bytes)
        })
        .collect()
}

fn bytes32_column(values: &[Bytes32]) -> Option<Vec<Vec<u8>>> {
    (!values.is_empty()).then(|| values.iter().map(|value| value.0.to_vec()).collect())
}

fn root_for_chain(chain_id: u64, chain_ids: &[u64], roots: &[Bytes32]) -> [u8; 32] {
    chain_ids
        .iter()
        .position(|candidate| *candidate == chain_id)
        .and_then(|index| roots.get(index))
        .map(|root| root.0)
        .unwrap_or([0u8; 32])
}
