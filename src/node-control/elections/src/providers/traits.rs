/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use control_client::client_api::Account as ControlClientAccount;
use std::collections::HashMap;
use ton_block::{
    ValidatorSet,
    config_params::{ConfigParam15, ConfigParam16, ConfigParam17},
};

fn serialize_hex<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let hex_string = hex::encode(bytes);
    serializer.serialize_str(&hex_string)
}

fn serialize_adnl_addrs<S>(addrs: &Vec<(Vec<u8>, u64)>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut seq = serde::Serializer::serialize_seq(serializer, Some(addrs.len()))?;
    for (addr, timestamp) in addrs {
        let hex_addr = hex::encode(addr);
        serde::ser::SerializeSeq::serialize_element(&mut seq, &(hex_addr, *timestamp))?;
    }
    serde::ser::SerializeSeq::end(seq)
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ValidatorEntry {
    #[serde(serialize_with = "serialize_hex")]
    pub key_id: Vec<u8>,
    #[serde(serialize_with = "serialize_hex")]
    pub public_key: Vec<u8>,
    #[serde(serialize_with = "serialize_adnl_addrs")]
    pub adnl_addrs: Vec<(Vec<u8>, u64)>,
    pub expired_at: u64,
}

impl ValidatorEntry {
    pub fn adnl_addr(&self) -> Option<Vec<u8>> {
        self.adnl_addrs.first().map(|(addr, _)| addr.clone())
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ValidatorConfig {
    pub keys: HashMap<u64, ValidatorEntry>,
}

impl ValidatorConfig {
    pub fn new() -> Self {
        Self { keys: HashMap::new() }
    }
    pub fn find(&self, election_id: u64) -> Option<ValidatorEntry> {
        self.keys.get(&election_id).cloned()
    }
}

pub struct Account {
    account: ControlClientAccount,
}

impl Account {
    pub fn new(account: ControlClientAccount) -> Self {
        Self { account }
    }
    pub fn balance(&self) -> u64 {
        match &self.account {
            ControlClientAccount::ShardAccountState(state) => state.balance as u64,
            _ => 0,
        }
    }
}

#[async_trait::async_trait]
pub trait ElectionsProvider: Send + Sync {
    async fn setup(&self) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;
    async fn new_validator_key(
        &mut self,
        since: u64,
        until: u64,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;
    async fn new_adnl_addr(&mut self, perm_key_id: Vec<u8>, until: u64) -> anyhow::Result<Vec<u8>>;
    async fn validator_config(&mut self) -> anyhow::Result<ValidatorConfig>;
    async fn election_parameters(&mut self) -> anyhow::Result<ConfigParam15>;
    async fn send_boc(&mut self, msg_boc: &[u8]) -> anyhow::Result<()>;
    async fn sign(&mut self, key_hash: Vec<u8>, data: Vec<u8>) -> anyhow::Result<Vec<u8>>;
    async fn account(&mut self, address: &str) -> anyhow::Result<Account>;
    async fn export_public_key(&mut self, key_id: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet>;
    async fn config_param_16(&mut self) -> anyhow::Result<ConfigParam16> {
        anyhow::bail!("config_param_16 not implemented")
    }
    async fn config_param_17(&mut self) -> anyhow::Result<ConfigParam17> {
        anyhow::bail!("config_param_17 not implemented")
    }
    async fn get_next_vset(&mut self) -> anyhow::Result<Option<ValidatorSet>>;
}
