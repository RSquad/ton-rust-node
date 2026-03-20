/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use common::serde_utils;
use ton_block::AccountStatus;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ShardAccountState {
    #[serde(with = "serde_utils::account_status_as_str")]
    pub status: AccountStatus,
    pub balance: u128,
    pub last_paid: u32,
    pub last_trans: u64,
    #[serde(with = "serde_utils::hex_string")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Account {
    Nonexist,
    ShardAccountState(ShardAccountState),
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BlockchainConfigInfo {
    #[serde(with = "serde_utils::hex_string")]
    pub state_proof: Vec<u8>,
    #[serde(with = "serde_utils::hex_string")]
    pub config_proof: Vec<u8>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SignRq {
    #[serde(with = "serde_utils::hex_string")]
    pub key_hash: Vec<u8>,
    #[serde(with = "serde_utils::hex_string")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AddValidatorPermKeyRq {
    #[serde(with = "serde_utils::hex_string")]
    pub key_hash: Vec<u8>,
    pub election_date: i32,
    pub expire_at: i32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AddValidatorTempKeyRq {
    #[serde(with = "serde_utils::hex_string")]
    pub perm_key_hash: Vec<u8>,
    #[serde(with = "serde_utils::hex_string")]
    pub key_hash: Vec<u8>,
    pub expire_at: i32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AddAdnlAddressRq {
    #[serde(with = "serde_utils::hex_string")]
    pub key_hash: Vec<u8>,
    pub category: i32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AddValidatorAdnlAddrRq {
    #[serde(with = "serde_utils::hex_string")]
    pub perm_key_hash: Vec<u8>,
    #[serde(with = "serde_utils::hex_string")]
    pub key_hash: Vec<u8>,
    pub expire_at: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineValidatorConfig {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.validator.config"
    pub adnl: Vec<EngineAdnl>,
    pub dht: Vec<EngineDht>,
    pub validators: Vec<EngineValidator>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineAdnl {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.adnl"

    #[serde(with = "serde_utils::b64")]
    pub id: Vec<u8>,
    pub category: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineDht {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.dht"

    #[serde(with = "serde_utils::b64")]
    pub id: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineValidator {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.validator"

    #[serde(with = "serde_utils::b64")]
    pub id: Vec<u8>,
    pub temp_keys: Vec<EngineValidatorTempKey>,
    pub adnl_addrs: Vec<EngineValidatorAdnlAddress>,
    pub election_date: i64,
    pub expire_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineValidatorTempKey {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.validatorTempKey"

    #[serde(with = "serde_utils::b64")]
    pub key: Vec<u8>,
    pub expire_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineValidatorAdnlAddress {
    #[serde(rename = "@type")]
    pub type_name: String, // "engine.validatorAdnlAddress"
    #[serde(with = "serde_utils::b64")]
    pub id: Vec<u8>,
    pub expire_at: i64,
}

#[async_trait::async_trait]
pub trait ClientAPI: Send + Sync {
    async fn get_account_state(&mut self, address: &str) -> anyhow::Result<Account>;
    async fn get_blockchain_config(&mut self) -> anyhow::Result<BlockchainConfigInfo>;
    async fn get_validator_config(&mut self) -> anyhow::Result<EngineValidatorConfig>;
    async fn get_config_param(&mut self, id: u32) -> anyhow::Result<Vec<u8>>;

    async fn sign(&mut self, rq: &SignRq) -> anyhow::Result<Vec<u8>>;
    async fn generate_key_pair(&mut self) -> anyhow::Result<Vec<u8>>;
    async fn export_key_pub(&mut self, key_hash: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn add_validator_perm_key(&mut self, rq: &AddValidatorPermKeyRq) -> anyhow::Result<()>;
    async fn add_validator_temp_key(&mut self, rq: &AddValidatorTempKeyRq) -> anyhow::Result<()>;
    async fn add_adnl_address(&mut self, rq: &AddAdnlAddressRq) -> anyhow::Result<()>;
    async fn add_validator_adnl_addr(&mut self, rq: &AddValidatorAdnlAddrRq) -> anyhow::Result<()>;

    async fn send_boc(&mut self, boc: &[u8]) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait Shutdown: Send + Sync {
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait ControlClient: ClientAPI + Shutdown {}

impl<T: ClientAPI + Shutdown> ControlClient for T {}
