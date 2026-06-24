use std::env;

use alloy_primitives::{hex, keccak256, Address, B256, U256};
use alloy_sol_types::SolValue;
use anyhow::{Context, Result};
use kzg_rs::Bytes32;
use op_succinct_client_utils::witness::EtheraSidecarMailboxStore;
use reqwest::Url;
use serde_json::{json, Value};

const MAILBOX_ADDRESS_ENV: &str = "ETHERA_SIDECAR_MAILBOX_ADDRESS";
const LEGACY_MAILBOX_ADDRESS_ENV: &str = "MAILBOX_ADDRESS";
const L2_RPC_ENV: &str = "L2_RPC";

const INBOX_CHAIN_IDS_SLOT: u64 = 0;
const OUTBOX_CHAIN_IDS_SLOT: u64 = 1;
const INBOX_ROOTS_SLOT: u64 = 2;
const OUTBOX_ROOTS_SLOT: u64 = 3;

pub struct EtheraSidecarMailboxSource {
    mailbox_address: Address,
    l2_rpc: Url,
    client: reqwest::Client,
}

impl EtheraSidecarMailboxSource {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(address) = env::var(MAILBOX_ADDRESS_ENV)
            .ok()
            .or_else(|| env::var(LEGACY_MAILBOX_ADDRESS_ENV).ok())
        else {
            return Ok(None);
        };

        let mailbox_address = address
            .parse::<Address>()
            .with_context(|| format!("failed to parse Ethera sidecar mailbox address {address}"))?;
        let l2_rpc = env::var(L2_RPC_ENV)
            .context("L2_RPC must be set when Ethera sidecar mailbox extraction is enabled")?
            .parse::<Url>()
            .context("failed to parse L2_RPC")?;

        Ok(Some(Self { mailbox_address, l2_rpc, client: reqwest::Client::new() }))
    }

    pub async fn load(&self, block_number: u64) -> Result<EtheraSidecarMailboxStore> {
        let inbox_chains = self.load_chain_ids(INBOX_CHAIN_IDS_SLOT, block_number).await?;
        let outbox_chains = self.load_chain_ids(OUTBOX_CHAIN_IDS_SLOT, block_number).await?;
        let inbox_roots = self.load_roots(INBOX_ROOTS_SLOT, &inbox_chains, block_number).await?;
        let outbox_roots = self.load_roots(OUTBOX_ROOTS_SLOT, &outbox_chains, block_number).await?;

        Ok(EtheraSidecarMailboxStore::new(
            inbox_chains.into_iter().map(u256_to_bytes32).collect(),
            outbox_chains.into_iter().map(u256_to_bytes32).collect(),
            inbox_roots.into_iter().map(b256_to_bytes32).collect(),
            outbox_roots.into_iter().map(b256_to_bytes32).collect(),
        ))
    }

    async fn load_chain_ids(&self, slot: u64, block_number: u64) -> Result<Vec<U256>> {
        let length_proof = self
            .eth_get_proof(vec![word_hex(U256::from(slot).to_be_bytes::<32>())], block_number)
            .await?;
        let length = storage_proof_value(&length_proof, 0)
            .transpose()?
            .map(u256_to_usize)
            .transpose()?
            .unwrap_or(0);

        if length == 0 {
            return Ok(Vec::new());
        }

        let base = U256::from_be_bytes(keccak256(U256::from(slot).to_be_bytes::<32>()).0);
        let storage_keys = (0..length)
            .map(|index| word_hex((base + U256::from(index as u64)).to_be_bytes::<32>()))
            .collect::<Vec<_>>();
        let proof = self.eth_get_proof(storage_keys, block_number).await?;

        (0..length)
            .map(|index| {
                storage_proof_value(&proof, index)
                    .transpose()
                    .map(|value| value.unwrap_or(U256::ZERO))
            })
            .collect()
    }

    async fn load_roots(
        &self,
        slot: u64,
        chain_ids: &[U256],
        block_number: u64,
    ) -> Result<Vec<B256>> {
        if chain_ids.is_empty() {
            return Ok(Vec::new());
        }

        let storage_keys = chain_ids
            .iter()
            .map(|chain_id| {
                let encoded = (*chain_id, U256::from(slot)).abi_encode();
                word_hex(keccak256(encoded).0)
            })
            .collect::<Vec<_>>();
        let proof = self.eth_get_proof(storage_keys, block_number).await?;

        (0..chain_ids.len())
            .map(|index| {
                storage_proof_value(&proof, index).transpose().map(|value| {
                    value.map(|word| B256::from(word.to_be_bytes::<32>())).unwrap_or(B256::ZERO)
                })
            })
            .collect()
    }

    async fn eth_get_proof(&self, storage_keys: Vec<String>, block_number: u64) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_getProof",
            "params": [
                format!("0x{}", hex::encode(self.mailbox_address.as_slice())),
                storage_keys,
                format!("0x{block_number:x}")
            ],
            "id": 1
        });

        let response = self
            .client
            .post(self.l2_rpc.clone())
            .json(&payload)
            .send()
            .await
            .context("eth_getProof request failed")?;
        let body =
            response.json::<Value>().await.context("failed to decode eth_getProof response")?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("eth_getProof returned error: {error}");
        }

        body.get("result").cloned().context("eth_getProof response missing result")
    }
}

fn storage_proof_value(proof: &Value, index: usize) -> Option<Result<U256>> {
    proof.get("storageProof")?.as_array()?.get(index)?.get("value")?.as_str().map(decode_u256)
}

fn decode_u256(value: &str) -> Result<U256> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.is_empty() {
        return Ok(U256::ZERO);
    }

    U256::from_str_radix(value, 16).context("failed to decode storage word")
}

fn u256_to_usize(value: U256) -> Result<usize> {
    if value > U256::from(usize::MAX) {
        anyhow::bail!("storage array length does not fit usize");
    }

    Ok(value.to::<usize>())
}

fn u256_to_bytes32(value: U256) -> Bytes32 {
    Bytes32(value.to_be_bytes::<32>())
}

fn b256_to_bytes32(value: B256) -> Bytes32 {
    Bytes32(value.0)
}

fn word_hex(word: [u8; 32]) -> String {
    format!("0x{}", hex::encode(word))
}
