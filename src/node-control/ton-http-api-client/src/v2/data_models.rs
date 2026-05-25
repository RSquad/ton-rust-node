/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::v2::RPCStackEntry;
use common::serde_utils;
use std::fmt::{Display, Formatter};
use ton_block::{AccountId, MsgAddrStd, MsgAddress};

pub fn make_addr(account_id: &AccountId) -> anyhow::Result<MsgAddress> {
    let addr = MsgAddress::AddrStd(MsgAddrStd {
        anycast: None,
        workchain_id: -1,
        address: account_id.clone(),
    });

    Ok(addr)
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct RunGetMethodParams {
    pub address: String,
    #[serde(rename = "method")]
    pub method_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<Vec<RPCStackEntry>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seqno: Option<u32>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct RunGetMethodRes {
    pub gas_used: i64,
    pub stack: Vec<RPCStackEntry>,
    pub exit_code: i32,
    pub last_transaction_id: Option<TransactionId>,
    pub block_id: Option<BlockIdExt>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountState {
    Active,
    /// TON HTTP APIs typically return `"uninit"`; some stacks use `"uninitialized"`.
    #[serde(rename = "uninit", alias = "uninitialized")]
    #[default]
    Uninitialized,
    Frozen,
}

impl std::fmt::Display for AccountState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountState::Active => write!(f, "active"),
            AccountState::Uninitialized => write!(f, "uninit"),
            AccountState::Frozen => write!(f, "frozen"),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct GetAddressInformationRes {
    #[serde(rename = "@type")]
    pub r#type: String,
    #[serde(with = "serde_utils::u64_as_str_or_num")]
    pub balance: u64,
    #[serde(default, with = "serde_utils::option_b64")]
    pub code: Option<Vec<u8>>,
    #[serde(default, with = "serde_utils::option_b64")]
    pub data: Option<Vec<u8>>,
    pub last_transaction_id: TransactionId,
    pub block_id: BlockIdExt,
    pub sync_utime: u64,
    #[serde(default)]
    pub extra_currencies: Vec<serde_json::Value>,
    pub state: AccountState,
    #[serde(default)]
    pub frozen_hash: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct GetExtendedAddressInformationRes {
    #[serde(rename = "@type")]
    pub r#type: String,

    pub address: AccountAddress,
    pub balance: u64,
    pub extra_currencies: Vec<serde_json::Value>,

    pub last_transaction_id: TransactionId,
    pub block_id: BlockIdExt,

    pub sync_utime: u64,
    pub account_state: RawAccountState,
    pub revision: i64,
    //#[serde(rename = "@extra")]
    //pub extra: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AccountAddress {
    #[serde(rename = "@type")]
    pub r#type: String,
    pub account_address: String,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct TransactionId {
    #[serde(rename = "@type")]
    pub r#type: String,

    #[serde(with = "serde_utils::u64_as_str")]
    pub lt: u64,

    #[serde(with = "serde_utils::b64")]
    pub hash: Vec<u8>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct BlockIdExt {
    #[serde(rename = "@type", skip_serializing)]
    pub r#type: String,
    pub workchain: i32,

    #[serde(with = "serde_utils::i64_as_str")]
    pub shard: i64,
    pub seqno: u32,

    #[serde(with = "serde_utils::b64")]
    pub root_hash: Vec<u8>,

    #[serde(with = "serde_utils::b64")]
    pub file_hash: Vec<u8>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct RawAccountState {
    #[serde(rename = "@type")]
    pub r#type: String,

    #[serde(with = "serde_utils::b64")]
    pub code: Vec<u8>,

    #[serde(with = "serde_utils::b64")]
    pub data: Vec<u8>,

    #[serde(with = "serde_utils::b64")]
    pub frozen_hash: Vec<u8>,
}

#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum WalletType {
    #[serde(rename = "wallet simple r1")]
    SimpleR1,
    #[serde(rename = "wallet simple r2")]
    SimpleR2,
    #[serde(rename = "wallet simple r3")]
    SimpleR3,
    #[serde(rename = "wallet v2 r1")]
    V2R1,
    #[serde(rename = "wallet v2 r2")]
    V2R2,
    #[serde(rename = "wallet v3 r1")]
    V3R1,
    #[serde(rename = "wallet v3 r2")]
    V3R2,
    #[serde(rename = "wallet v4 r1")]
    V4R1,
    #[serde(rename = "wallet v4 r2")]
    V4R2,
    #[serde(rename = "wallet v5 r1")]
    V5R1,
    #[serde(untagged)]
    Other(String),
}

impl Display for WalletType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletType::SimpleR1 => write!(f, "SimpleR1"),
            WalletType::SimpleR2 => write!(f, "SimpleR2"),
            WalletType::SimpleR3 => write!(f, "SimpleR3"),
            WalletType::V2R1 => write!(f, "V2R1"),
            WalletType::V2R2 => write!(f, "V2R2"),
            WalletType::V3R1 => write!(f, "V3R1"),
            WalletType::V3R2 => write!(f, "V3R2"),
            WalletType::V4R1 => write!(f, "V4R1"),
            WalletType::V4R2 => write!(f, "V4R2"),
            WalletType::V5R1 => write!(f, "V5R1"),
            WalletType::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Subset of the `getMasterchainInfo` response — only the `last` block id is captured.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct GetMasterchainInfoRes {
    pub last: BlockIdExt,
}

/// Subset of the `getBlockHeader` response — only `gen_utime` is captured.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct GetBlockHeaderRes {
    pub gen_utime: u32,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct GetWalletInformationRes {
    #[serde(rename = "@type")]
    pub r#type: Option<String>,
    pub wallet: bool,
    #[serde(with = "serde_utils::u64_as_str_or_num")]
    pub balance: u64,
    pub account_state: AccountState,
    pub last_transaction_id: TransactionId,
    pub wallet_type: Option<WalletType>,
    pub seqno: Option<u32>,
    pub wallet_id: Option<u64>,
}

#[cfg(test)]
mod account_state_tests {
    use super::AccountState;

    #[test]
    fn deserializes_uninit_from_ton_http_api() {
        let v: AccountState = serde_json::from_value(serde_json::json!("uninit")).unwrap();
        assert_eq!(v, AccountState::Uninitialized);
    }

    #[test]
    fn deserializes_uninitialized_alias() {
        let v: AccountState = serde_json::from_value(serde_json::json!("uninitialized")).unwrap();
        assert_eq!(v, AccountState::Uninitialized);
    }

    #[test]
    fn serializes_uninitialized_as_uninit() {
        let s = serde_json::to_string(&AccountState::Uninitialized).unwrap();
        assert_eq!(s, "\"uninit\"");
    }
}
