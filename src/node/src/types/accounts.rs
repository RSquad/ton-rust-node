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
use crate::engine_traits::EngineOperations;
use std::sync::Arc;
use ton_block::{
    fail, Account, AccountBlock, AccountId, AccountStorageStat, Augmentation, Cell, HashUpdate,
    HashmapAugType, HashmapRemover, LibDescr, Libraries, Result, Serializable, ShardAccount,
    ShardAccounts, StateInitLib, Transaction, Transactions, UInt256, UsageTree,
};

pub struct ShardAccountStuff {
    account: Account,
    account_id: AccountId,
    shard_acc: ShardAccount,
    lt: u64,
    transactions: Transactions,
    state_update: HashUpdate,
    orig_libs: StateInitLib,
    storage_dict: Option<Cell>,
    storage_dict_usage: Option<UsageTree>,
    account_updates: Vec<Cell>,
    original_root: Cell,
    lt_compatible: bool,
    dict_hash_min_cells: u32,
}

impl ShardAccountStuff {
    pub fn init(
        engine: &Arc<dyn EngineOperations>,
        account_id: AccountId,
        shard_acc: ShardAccount,
        lt: u64,
        full_collated_data: bool,
        lt_compatible: bool,
        dict_hash_min_cells: u32,
    ) -> Result<Self> {
        let mut account = shard_acc.read_account()?;
        let account_hash = shard_acc.account_hash();
        let mut storage_dict = if let Some(dict_hash) = account.dict_hash() {
            if let Some(dict) = engine.get_account_storage_dict(dict_hash) {
                Some(dict)
            } else {
                let now = std::time::Instant::now();
                let result = account.init_storage_stat(dict_hash_min_cells)?;
                log::debug!("TIME init_storage_stat {:?}", now.elapsed());
                result
            }
        } else {
            None
        };
        let mut storage_dict_usage = None;
        if let Some(dict) = &mut storage_dict {
            if full_collated_data {
                let usage_tree = UsageTree::with_params(dict.clone(), true);
                *dict = usage_tree.root_cell();
                storage_dict_usage = Some(usage_tree);
                account.import_storage_stat_dict(dict.clone())?;
            }
        }
        let orig_libs = account.libraries();
        Ok(Self {
            account_id,
            account,
            original_root: shard_acc.account_cell(),
            shard_acc,
            lt,
            transactions: Transactions::default(),
            state_update: HashUpdate::with_hashes(account_hash.clone(), account_hash),
            orig_libs,
            storage_dict,
            storage_dict_usage,
            account_updates: Vec::new(),
            lt_compatible,
            dict_hash_min_cells,
        })
    }

    pub fn update_shard_state(&mut self, new_accounts: &mut ShardAccounts) -> Result<AccountBlock> {
        if self.account.is_none() {
            new_accounts.remove(self.account_id.clone())?;
        } else {
            let value = self.shard_acc.write_to_new_cell()?;
            new_accounts.set_builder_serialized(
                self.account_id.clone(),
                &value,
                &self.account.aug()?,
            )?;
        }
        AccountBlock::with_params(&self.account_id, &self.transactions, &self.state_update)
    }

    pub fn lt(&self) -> u64 {
        self.lt
    }
    pub fn fetch_max_lt(&mut self, lt: u64) {
        self.lt = self.lt.max(lt).max(self.shard_acc.last_trans_lt() + 1);
    }
    pub fn account(&self) -> &Account {
        &self.account
    }
    // pub fn account_root(&self) -> Cell {
    //     self.account_root.clone()
    // }
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }
    pub fn storage_dict(&self) -> Option<Cell> {
        self.storage_dict.clone()
    }
    pub fn storage_dict_usage(&self) -> Option<&UsageTree> {
        self.storage_dict_usage.as_ref()
    }
    pub fn account_updates(&self) -> &[Cell] {
        &self.account_updates
    }

    pub fn original_root(&self) -> &Cell {
        &self.original_root
    }

    pub fn add_transaction(
        &mut self,
        transaction: &mut Transaction,
        account: Account,
    ) -> Result<()> {
        transaction.set_prev_trans_hash(self.shard_acc.last_trans_hash().clone());
        transaction.set_prev_trans_lt(self.shard_acc.last_trans_lt());
        // log::trace!("{} {}", self.collated_block_descr, debug_transaction(transaction.clone())?);
        self.account = account;
        self.storage_dict = self.account.update_storage_stat(self.dict_hash_min_cells)?;
        self.account_updates.extend(AccountStorageStat::get_roots(self.account.state_init()));
        self.shard_acc.write_account(&self.account)?;
        let new_hash = self.shard_acc.account_hash();
        let old_hash = std::mem::replace(&mut self.state_update.new_hash, new_hash.clone());
        let state_update = HashUpdate::with_hashes(old_hash, new_hash);
        transaction.write_state_update(&state_update)?;
        let tr_root = transaction.serialize()?;
        *self.shard_acc.last_trans_hash_mut() = tr_root.repr_hash();
        *self.shard_acc.last_trans_lt_mut() = transaction.logical_time();
        self.lt = transaction.logical_time() + transaction.out_msgs.len()? as u64 + 1;
        if self.lt_compatible {
            self.lt -= 1;
        }
        self.transactions.setref(&transaction.logical_time(), tr_root, transaction.total_fees())?;
        Ok(())
    }

    pub fn update_public_libraries(&self, libraries: &mut Libraries) -> Result<()> {
        let new_libs = self.account.libraries();
        if new_libs.root() != self.orig_libs.root() {
            self.orig_libs.scan_diff(&new_libs, |key: UInt256, old, new| {
                let old = old.unwrap_or_default();
                let new = new.unwrap_or_default();
                if old.is_public_library() && !new.is_public_library() {
                    self.remove_public_library(key, libraries)?;
                } else if !old.is_public_library() && new.is_public_library() {
                    self.add_public_library(key, new.root, libraries)?;
                }
                Ok(true)
            })?;
        }
        Ok(())
    }

    pub fn remove_public_library(&self, key: UInt256, libraries: &mut Libraries) -> Result<()> {
        log::trace!("Removing public library {key:x} of account {:x}", self.account_id);
        let mut lib_descr = match libraries.get(&key)? {
            Some(ld) => ld,
            None => fail!(
                "Cannot remove public library {key:x} of account {:x} because this public \
                library did not exist",
                self.account_id
            ),
        };
        if lib_descr.lib().repr_hash() != key {
            fail!(
                "Cannot remove public library {key:x} of account {:x} because this public \
                library LibDescr record does not contain a library root cell with required hash",
                self.account_id
            )
        }
        if !lib_descr.publishers_mut().remove(&self.account_id)? {
            fail!(
                "Cannot remove public library {key:x} of account {:x} because this public \
                library LibDescr record does not list this account as one of publishers",
                self.account_id
            )
        }
        if lib_descr.publishers().is_empty() {
            log::debug!("Library {key:x} has no publishers left, removing altogether");
            libraries.remove(&key)?;
        } else {
            libraries.set(&key, &lib_descr)?;
        }
        Ok(())
    }

    pub fn add_public_library(
        &self,
        key: UInt256,
        library: Cell,
        libraries: &mut Libraries,
    ) -> Result<()> {
        log::trace!("Adding public library {key:x} of account {:x}", self.account_id);
        if key != library.repr_hash() {
            fail!(
                "Can't add library {:x} because it mismatch given key {key:x}",
                library.repr_hash()
            )
        }
        let lib_descr = if let Some(mut old_lib_descr) = libraries.get(&key)? {
            if old_lib_descr.lib().repr_hash() != library.repr_hash() {
                fail!(
                    "Cannot add public library {key:x} of account {:x} because existing \
                    LibDescr record for this library does not contain a library root cell \
                    with required hash",
                    self.account_id
                )
            }
            if old_lib_descr.publishers().check_key(&self.account_id)? {
                fail!(
                    "Cannot add public library {key:x} of account {:x} because this \
                    public library's LibDescr record already listed this account as \
                    a publisher",
                    self.account_id
                )
            }
            old_lib_descr.publishers_mut().set(&self.account_id, &())?;
            old_lib_descr
        } else {
            LibDescr::from_lib_data_by_publisher(library, self.account_id.clone())
        };
        libraries.set(&key, &lib_descr)?;
        Ok(())
    }
}
