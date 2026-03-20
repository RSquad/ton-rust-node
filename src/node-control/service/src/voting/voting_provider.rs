/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::client::AdnlClientConfig;
use control_client::{
    client_adnl::ControlClientAdnl,
    client_api::{ControlClient, SignRq},
};
use std::{collections::HashMap, str::FromStr};
use ton_block::{SigPubKey, UInt256, ValidatorDescr, ValidatorSet};

#[derive(Clone)]
pub struct ValidatorEntry {
    pub key_id: Vec<u8>,
    pub public_key: Vec<u8>,
    pub adnl_addrs: Vec<(Vec<u8>, u64)>,
    pub expired_at: u64,
}

pub struct ValidatorConfig {
    pub keys: HashMap<u64, ValidatorEntry>,
}

impl ValidatorConfig {
    pub fn from_map(keys: HashMap<u64, ValidatorEntry>) -> Self {
        Self { keys }
    }

    pub fn find_key(&self, public_key: &[u8]) -> Option<ValidatorEntry> {
        self.keys
            .iter()
            .find(|(_, entry)| entry.public_key == public_key)
            .map(|(_, entry)| entry.clone())
    }
}

#[async_trait::async_trait]
pub trait VotingProvider: Send + Sync {
    async fn setup(&mut self) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;
    async fn send_boc(&mut self, boc: &[u8]) -> anyhow::Result<()>;
    async fn sign(&mut self, message: &[u8], key_id: Vec<u8>) -> anyhow::Result<Vec<u8>>;
    async fn validator_config(&mut self) -> anyhow::Result<ValidatorConfig>;
    async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet>;
    async fn export_public_key(&mut self, key_id: &[u8]) -> anyhow::Result<Vec<u8>>;
}

pub struct VotingProviderImpl {
    client: Box<dyn ControlClient>,
}

impl VotingProviderImpl {
    pub fn new(config: AdnlClientConfig) -> Self {
        let client = ControlClientAdnl::new(config, 4);
        Self { client: Box::new(client) }
    }

    #[cfg(test)]
    pub fn with_client(client: Box<dyn ControlClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl VotingProvider for VotingProviderImpl {
    async fn setup(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.client.shutdown().await
    }

    async fn send_boc(&mut self, boc: &[u8]) -> anyhow::Result<()> {
        self.client.send_boc(boc).await
    }

    async fn sign(&mut self, message: &[u8], key_id: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        self.client.sign(&SignRq { key_hash: key_id, data: message.to_vec() }).await
    }

    async fn validator_config(&mut self) -> anyhow::Result<ValidatorConfig> {
        let remote = self.client.get_validator_config().await?;
        let keys = remote
            .validators
            .into_iter()
            .map(|validator| {
                let adnl_addrs: Vec<(Vec<u8>, u64)> = validator
                    .adnl_addrs
                    .iter()
                    .map(|entry| (entry.id.clone(), entry.expire_at as u64))
                    .collect();
                (
                    validator.election_date as u64,
                    ValidatorEntry {
                        key_id: validator.id,
                        public_key: vec![],
                        adnl_addrs,
                        expired_at: validator.expire_at as u64,
                    },
                )
            })
            .collect::<HashMap<u64, ValidatorEntry>>();
        Ok(ValidatorConfig::from_map(keys))
    }

    async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet> {
        let bytes = self.client.get_config_param(34).await?;
        let param: serde_json::Value = serde_json::from_str(&String::from_utf8(bytes)?)?;
        let map = param
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("invalid config param"))?
            .get("p34")
            .and_then(|v| v.as_object())
            .ok_or_else(|| anyhow::anyhow!("p34 entry not found"))?;
        let utime_since = map
            .get("utime_since")
            .and_then(|value| value.as_u64())
            .map(|v| v as u32)
            .ok_or_else(|| anyhow::anyhow!("utime_since"))?;
        let utime_until = map
            .get("utime_until")
            .and_then(|value| value.as_u64())
            .map(|v| v as u32)
            .ok_or_else(|| anyhow::anyhow!("utime_until"))?;
        let _ = map
            .get("total")
            .and_then(|value| value.as_u64())
            .map(|v| v as u16)
            .ok_or_else(|| anyhow::anyhow!("total"))?;
        let main = map
            .get("main")
            .and_then(|value| value.as_u64())
            .map(|v| v as u16)
            .ok_or_else(|| anyhow::anyhow!("main"))?;
        let json_list = map
            .get("list")
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow::anyhow!("list"))?;
        let mut list = vec![];
        for entry in json_list {
            let map = entry.as_object().ok_or_else(|| anyhow::anyhow!("invalid list entry"))?;
            let pubkey = map
                .get("public_key")
                .and_then(|v| v.as_str())
                .map(hex::decode)
                .transpose()?
                .ok_or(anyhow::anyhow!("public_key"))?;
            let weight = map
                .get("weight_dec")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or(anyhow::anyhow!("weight"))?;
            let adnl_addr =
                map.get("adnl_addr").and_then(|v| v.as_str()).map(UInt256::from_str).transpose()?;
            let descr = ValidatorDescr {
                public_key: SigPubKey::from_bytes(&pubkey)
                    .map_err(|_| anyhow::anyhow!("public key is invalid"))?,
                weight,
                adnl_addr,
                mc_seq_no_since: 0,
                prev_weight_sum: 0,
            };
            list.push(descr);
        }
        ValidatorSet::new(utime_since, utime_until, main, list)
    }

    async fn export_public_key(&mut self, key_id: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.client.export_key_pub(key_id).await
    }
}
