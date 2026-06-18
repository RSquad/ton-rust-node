/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    accounts::{Account, ShardAccount},
    define_HashmapAugE,
    dictionary::hashmapaug::{Augmentable, HashmapAugType},
    error, fail,
    types::{CurrencyCollection, Number5},
    AccountId, Augmentation, BuilderData, Cell, Deserializable, HashmapSubtree, IBitstring, Result,
    Serializable, SliceData, UInt256,
};
use std::fmt;

#[cfg(test)]
#[path = "tests/test_shard_accounts.rs"]
mod tests;

/////////////////////////////////////////////////////////////////////////////////////////
// 4.1.9. The combined state of all accounts in a shard. The split part
// of the shardchain state (cf. 1.2.1 and 1.2.2) is given by (upd from Lite Client v11):
// _ (HashmapAugE 256 ShardAccount DepthBalanceInfo) = ShardAccounts;
define_HashmapAugE!(ShardAccounts, 256, AccountId, ShardAccount, DepthBalanceInfo);
impl HashmapSubtree for ShardAccounts {}

impl ShardAccounts {
    pub fn insert(
        &mut self,
        split_depth: u8,
        account: &Account,
        last_trans_hash: UInt256,
        last_trans_lt: u64,
    ) -> Result<Option<AccountId>> {
        match account.get_id() {
            Some(acc_id) => {
                let depth_balance_info =
                    DepthBalanceInfo::new(split_depth as u32, account.get_balance().unwrap())?;
                let sh_account =
                    ShardAccount::with_params(account, last_trans_hash, last_trans_lt)?;
                self.set_builder_serialized(
                    acc_id.clone(),
                    &sh_account.write_to_new_cell()?,
                    &depth_balance_info,
                )?;
                Ok(Some(acc_id.clone()))
            }
            _ => Ok(None),
        }
    }

    pub fn account(&self, account_id: &AccountId) -> Result<Option<ShardAccount>> {
        self.get_serialized(account_id.clone())
    }

    pub fn balance(&self, account_id: &AccountId) -> Result<Option<DepthBalanceInfo>> {
        match self.get_serialized_raw(account_id.clone())? {
            Some(mut slice) => Ok(Some(DepthBalanceInfo::construct_from(&mut slice)?)),
            None => Ok(None),
        }
    }

    pub fn full_balance(&self) -> &CurrencyCollection {
        &self.root_extra().balance
    }

    pub fn split_for(&mut self, split_key: &SliceData) -> Result<&DepthBalanceInfo> {
        *self = self.subtree_with_prefix(split_key, &mut 0)?;
        self.update_root_extra()
    }
}

impl Augmentation<DepthBalanceInfo> for ShardAccount {
    fn aug(&self) -> Result<DepthBalanceInfo> {
        self.read_account()?.aug()
    }
}

/// depth_balance$_ split_depth:(#<= 30) balance:CurrencyCollection = DepthBalanceInfo;
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct DepthBalanceInfo {
    split_depth: Number5,
    balance: CurrencyCollection,
}

impl DepthBalanceInfo {
    pub fn new(split_depth: u32, balance: &CurrencyCollection) -> Result<Self> {
        Ok(Self { split_depth: Number5::new_checked(split_depth, 30)?, balance: balance.clone() })
    }

    pub fn set_split_depth(&mut self, split_depth: u32) -> Result<()> {
        self.split_depth = Number5::new_checked(split_depth, 30)?;
        Ok(())
    }

    pub fn set_balance(&mut self, balance: CurrencyCollection) {
        self.balance = balance
    }

    pub fn balance(&self) -> &CurrencyCollection {
        &self.balance
    }
}

impl Augmentable for DepthBalanceInfo {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        self.split_depth = self.split_depth.max(other.split_depth);
        self.balance.calc(&other.balance)
    }
}

impl Deserializable for DepthBalanceInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.split_depth.read_from(cell)?;
        self.balance.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for DepthBalanceInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.split_depth.write_to(cell)?;
        self.balance.write_to(cell)?;
        Ok(())
    }
}
