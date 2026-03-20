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
    dictionary::hashmapaug::{Augmentation, HashmapAugType},
    error,
    error::{BlockError, Result},
    fail,
    merkle_proof::MerkleProof,
    messages::{Message, MsgAddressInt, StateInit, StateInitLib, TickTock},
    shard::{ShardIdent, ShardStateUnsplit},
    shard_accounts::DepthBalanceInfo,
    types::{AddSub, ChildCell, CurrencyCollection, Grams, Number5, VarUInteger7},
    AccountId, AccountStorageStat, BuilderData, Cell, ConfigParams, Deserializable, GasConsumer,
    GetRepresentationHash, HashmapType, IBitstring, Serializable, SliceData, UInt256, UsageTree,
    DICT_HASH_MIN_CELLS,
};
use std::{collections::HashSet, fmt};

#[cfg(test)]
#[path = "tests/test_accounts.rs"]
mod tests;

// ///////////////////////////////////////////////////////////////////////////////
// ///
// /// 4.1.5. Storage profile of an account.
// ///
// /// storage_used$_ cells:(VarUInteger 7) bits:(VarUInteger 7)
// ///
// /// storage_extra_none$000 = StorageExtraInfo;
// /// storage_extra_info$001 dict_hash:uint256 = StorageExtraInfo;
// ///
// /// storage_info$_ used:StorageUsed storage_extra:StorageExtraInfo last_paid:uint32
// /// due_payment:(Maybe Grams) = StorageInfo;
// ///
// /// 4.1.6. Account description.
// ///
// /// original format
// /// account_none$0 = Account;
// /// account$1 addr:MsgAddressInt storage_stat:StorageInfo
// /// storage:AccountStorage = Account;
// ///
// /// account_storage$_ last_trans_lt:uint64
// /// balance:CurrencyCollection state:AccountState
// /// = AccountStorage;
// ///
// /// account_uninit$00 = AccountState;
// /// account_active$1 _:StateInit = AccountState;
// /// account_frozen$01 state_hash:uint256 = AccountState;
// ///
// /// acc_state_uninit$00 = AccountStatus;
// /// acc_state_frozen$01 = AccountStatus;
// /// acc_state_active$10 = AccountStatus;
// /// acc_state_nonexist$11 = AccountStatus;
// ///
// /// tick_tock$_ tick:Boolean tock:Boolean = TickTock;
// /// _ fixed_prefix_length:(Maybe (## 5)) special:(Maybe TickTock)
// /// code:(Maybe ^Cell) data:(Maybe ^Cell)
// /// library:(Maybe ^Cell) = StateInit;

///////////////////////////////////////////////////////////////////////////////
///
/// 4.1.5. Storage profile of an account.
///
/// storage_used$_ cells:(VarUInteger 7) bits:(VarUInteger 7)
///
#[derive(Eq, Clone, Debug, Default, PartialEq)]
pub struct StorageUsed {
    cells: VarUInteger7,
    bits: VarUInteger7,
}

impl StorageUsed {
    pub const fn new() -> Self {
        StorageUsed { cells: VarUInteger7::zero(), bits: VarUInteger7::zero() }
    }
    pub const fn bits(&self) -> u64 {
        self.bits.as_u64()
    }
    pub const fn cells(&self) -> u64 {
        self.cells.as_u64()
    }

    pub fn with_values_checked(cells: u64, bits: u64) -> Result<Self> {
        Ok(Self { cells: VarUInteger7::try_from(cells)?, bits: VarUInteger7::try_from(bits)? })
    }

    /// append cells and bits count
    pub fn add_bits_and_cells(&mut self, bits: u64, cells: u64) {
        self.bits += bits;
        self.cells += cells;
    }
}

impl Serializable for StorageUsed {
    fn write_to(&self, output: &mut BuilderData) -> Result<()> {
        self.cells.write_to(output)?; //cells:(VarUInteger 7)
        self.bits.write_to(output)?; //bits:(VarUInteger 7)
        Ok(())
    }
}

impl Deserializable for StorageUsed {
    fn read_from(&mut self, data: &mut SliceData) -> Result<()> {
        self.cells.read_from(data)?; //cells:(VarUInteger 7)
        self.bits.read_from(data)?; //bits:(VarUInteger 7)
        Ok(())
    }
}

impl fmt::Display for StorageUsed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "StorageUsed[cells = {}, bits = {}]", self.cells, self.bits)
    }
}

pub struct StorageUsageCalc {
    bits: u64,
    cells: u64,
    max_merkle_depth: u32,
    limit_bits: u64,
    limit_cells: u64,
    hashes: HashSet<UInt256>,
}

impl StorageUsageCalc {
    pub const fn bits(&self) -> u64 {
        self.bits
    }
    pub const fn cells(&self) -> u64 {
        self.cells
    }
    pub const fn max_merkle_depth(&self) -> u32 {
        self.max_merkle_depth
    }

    pub fn with_limits(limit_cells: u64, limit_bits: u64) -> Self {
        Self {
            bits: 0,
            cells: 0,
            max_merkle_depth: 0,
            limit_bits,
            limit_cells,
            hashes: HashSet::new(),
        }
    }

    fn add_checked(&mut self, cells: u64, bits: u64) -> bool {
        if self.limit_cells != 0 && self.cells + cells > self.limit_cells
            || self.limit_bits != 0 && self.bits + bits > self.limit_bits
        {
            return false;
        }
        self.cells += cells;
        self.bits += bits;
        true
    }

    ///
    /// append cell and bits count into
    ///
    /// Returns:
    /// - Ok(max_merkle_depth)
    ///
    pub fn append_cell(
        &mut self,
        cell: &Cell,
        add_root: bool,
        gas_consumer: &mut impl GasConsumer,
    ) -> Result<u32> {
        if add_root
            && (!self.hashes.insert(cell.repr_hash())
                || !self.add_checked(1, cell.bit_length() as u64))
        {
            return Ok(0);
        }
        if cell.is_pruned() {
            return Ok(0);
        }
        let mut max_merkle_depth = 0;
        let slice = gas_consumer.load_cell(cell.clone())?;
        for i in 0..slice.remaining_references() {
            let merkle_depth = self.append_cell(&slice.reference(i)?, true, gas_consumer)?;
            max_merkle_depth = max_merkle_depth.max(merkle_depth);
        }
        if cell.is_merkle() {
            max_merkle_depth += 1;
        }
        self.max_merkle_depth = self.max_merkle_depth.max(max_merkle_depth);
        Ok(max_merkle_depth)
    }

    pub fn append_builder(
        &mut self,
        root: &BuilderData,
        add_root: bool,
        gas_consumer: &mut impl GasConsumer,
    ) -> Result<()> {
        if add_root && !self.add_checked(1, root.bits_used() as u64) {
            return Ok(());
        }
        for cell in root.references() {
            self.append_cell(cell, true, gas_consumer)?;
        }
        Ok(())
    }

    pub fn storage_used(&self) -> Result<StorageUsed> {
        StorageUsed::with_values_checked(self.cells, self.bits)
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// storage_extra_none$000 = StorageExtraInfo;
/// storage_extra_info$001 dict_hash:uint256 = StorageExtraInfo;

#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub struct StorageExtraInfo {
    dict_hash: Option<UInt256>,
}

impl StorageExtraInfo {
    pub const fn new() -> Self {
        StorageExtraInfo { dict_hash: None }
    }
}

impl Serializable for StorageExtraInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if let Some(hash) = &self.dict_hash {
            cell.append_bits(0b001, 3)?; // prefix storage_extra_info
            hash.write_to(cell)?;
        } else {
            cell.append_bits(0b000, 3)?; // prefix storage_extra_none
        }
        Ok(())
    }
}

impl Deserializable for StorageExtraInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let flags = cell.get_next_int(3)?;
        if flags == 0b001 {
            self.dict_hash = Some(cell.get_next_hash()?);
        } else {
            self.dict_hash = None;
        }
        Ok(())
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// 4.1.5. Storage profile of an account.
/// storage_info$_ used:StorageUsed storage_extra:StorageExtraInfo last_paid:uint32
/// due_payment:(Maybe Grams) = StorageInfo;

#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub struct StorageInfo {
    used: StorageUsed,
    storage_extra: StorageExtraInfo,
    last_paid: u32,
    due_payment: Option<Grams>,
}

impl StorageInfo {
    pub const fn new() -> Self {
        StorageInfo {
            used: StorageUsed::new(),
            storage_extra: StorageExtraInfo::new(),
            last_paid: 0,
            due_payment: None,
        }
    }
    pub fn with_values(last_paid: u32, due_payment: Option<Grams>) -> Self {
        StorageInfo {
            used: StorageUsed::default(),
            storage_extra: StorageExtraInfo::default(),
            last_paid,
            due_payment,
        }
    }
    pub const fn used(&self) -> &StorageUsed {
        &self.used
    }
    pub const fn dict_hash(&self) -> Option<&UInt256> {
        self.storage_extra.dict_hash.as_ref()
    }
    pub const fn last_paid(&self) -> u32 {
        self.last_paid
    }
    pub const fn due_payment(&self) -> Option<&Grams> {
        self.due_payment.as_ref()
    }
}

impl Serializable for StorageInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.used.write_to(cell)?;
        self.storage_extra.write_to(cell)?;
        cell.append_u32(self.last_paid)?;
        self.due_payment.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for StorageInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.used.read_from(cell)?;
        self.storage_extra.read_from(cell)?;
        self.last_paid = cell.get_next_u32()?;
        self.due_payment.read_from(cell)?;
        Ok(())
    }
}

impl fmt::Display for StorageInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "StorageInfo[\r\nlast_paid = {}, \r\ndue_payment = {:?}]",
            self.last_paid, self.due_payment
        )
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// 4.1.6. Account description.
///
/// acc_state_uninit$00 = AccountStatus;
/// acc_state_frozen$01 = AccountStatus;
/// acc_state_active$10 = AccountStatus;
/// acc_state_nonexist$11 = AccountStatus;
///

#[derive(Default, PartialEq, Eq, Clone, Debug, PartialOrd, Ord)]
pub enum AccountStatus {
    #[default]
    AccStateUninit,
    AccStateFrozen,
    AccStateActive,
    AccStateNonexist,
}

/// serialize AccountStatus
impl Serializable for AccountStatus {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        // write to cell only prefix
        match self {
            AccountStatus::AccStateUninit => cell.append_bits(0b00, 2)?,
            AccountStatus::AccStateFrozen => cell.append_bits(0b01, 2)?,
            AccountStatus::AccStateActive => cell.append_bits(0b10, 2)?,
            AccountStatus::AccStateNonexist => cell.append_bits(0b11, 2)?,
        };
        Ok(())
    }
}

// deserialize AccountStatus
impl Deserializable for AccountStatus {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // read value of AccountStatus from cell
        let flags = cell.get_next_bits(2)?;
        *self = match flags[0] & 0xC0 {
            0x00 => AccountStatus::AccStateUninit,
            0x80 => AccountStatus::AccStateActive,
            0x40 => AccountStatus::AccStateFrozen,
            0xC0 => AccountStatus::AccStateNonexist,
            _ => fail!(BlockError::Other("unreachable".to_string())),
        };
        Ok(())
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// 4.1.6. Account description.
///
/// account_storage$_ last_trans_lt:uint64
/// balance:CurrencyCollection state:AccountState
/// = AccountStorage;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountStorage {
    last_trans_lt: u64,
    balance: CurrencyCollection,
    state: AccountState,
}

impl AccountStorage {
    pub const fn new() -> Self {
        Self {
            last_trans_lt: 0,
            balance: CurrencyCollection::new(),
            state: AccountState::AccountUninit,
        }
    }

    /// Construct storage for uninit account
    pub fn unint(balance: CurrencyCollection) -> Self {
        Self { balance, ..Self::default() }
    }

    /// Construct storage for active account
    pub fn active(last_trans_lt: u64, balance: CurrencyCollection, state_init: StateInit) -> Self {
        Self { last_trans_lt, balance, state: AccountState::AccountActive { state_init } }
    }

    /// Construct storage for frozen account
    pub fn frozen(
        last_trans_lt: u64,
        balance: CurrencyCollection,
        state_init_hash: UInt256,
    ) -> Self {
        Self { last_trans_lt, balance, state: AccountState::AccountFrozen { state_init_hash } }
    }

    /// Construct storage for uninit account with balance
    pub fn with_balance(balance: CurrencyCollection) -> Self {
        Self::unint(balance)
    }

    const fn state(&self) -> &AccountState {
        &self.state
    }

    pub const fn state_init(&self) -> Option<&StateInit> {
        match &self.state {
            AccountState::AccountActive { state_init } => Some(state_init),
            _ => None,
        }
    }

    fn calc_storage_used(&self) -> Result<StorageUsed> {
        let root_cell = self.serialize()?;
        let mut calc = StorageUsageCalc::with_limits(0, 0);
        calc.append_cell(&root_cell, true, &mut 0)?;
        calc.storage_used()
    }
}

impl Serializable for AccountStorage {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.last_trans_lt.write_to(cell)?; //last_trans_lt:uint64
        self.balance.write_to(cell)?; //balance:CurrencyCollection
        self.state.write_to(cell)?; //state:AccountState
        Ok(())
    }
}

impl fmt::Display for AccountStorage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "AccountStorage[last_trans_lt {}, balance {}, account state {:?}]",
            self.last_trans_lt, self.balance, self.state
        )
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// 4.1.6. Account description.
///
/// account_uninit$00 = AccountState;
/// account_active$1 _:StateInit = AccountState;
/// account_frozen$01 state_hash:uint256 = AccountState;
///
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum AccountState {
    #[default]
    AccountUninit,
    AccountActive {
        state_init: StateInit,
    },
    AccountFrozen {
        state_init_hash: UInt256,
    },
}

impl Serializable for AccountState {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            AccountState::AccountUninit => {
                cell.append_bits(0b00, 2)?; // prefix AccountUninit
            }
            AccountState::AccountFrozen { state_init_hash } => {
                cell.append_bits(0b01, 2)?; // prefix AccountFrozen
                state_init_hash.write_to(cell)?;
            }
            AccountState::AccountActive { state_init } => {
                cell.append_bits(0b1, 1)?; // prefix AccountActive
                state_init.write_to(cell)?; // StateInit
            }
        }
        Ok(())
    }
}

impl Deserializable for AccountState {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let ret = if slice.get_next_bit()? {
            let state_init = StateInit::construct_from(slice)?;
            AccountState::AccountActive { state_init }
        } else if slice.get_next_bit()? {
            let state_init_hash = slice.get_next_hash()?;
            AccountState::AccountFrozen { state_init_hash }
        } else {
            AccountState::AccountUninit
        };
        Ok(ret)
    }
}

impl fmt::Display for AccountState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AccountStorage[{:?}]", self)
    }
}

#[derive(Clone, Default)]
struct AccountStuff {
    addr: MsgAddressInt,
    storage_info: StorageInfo,
    storage: AccountStorage,

    storage_stat: AccountStorageStat,
}

impl fmt::Debug for AccountStuff {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AccountStuff")
            .field("addr", &self.addr)
            .field("storage_info", &self.storage_info)
            .field("storage", &self.storage)
            .finish()
    }
}

impl AccountStuff {
    pub fn storage_info(&self) -> &StorageInfo {
        &self.storage_info
    }

    pub fn state_init_mut(&mut self) -> Option<&mut StateInit> {
        match self.storage.state {
            AccountState::AccountActive { ref mut state_init } => Some(state_init),
            _ => None,
        }
    }
    fn update_storage_stat(&mut self, dict_hash_min_cells: u32) -> Result<Option<Cell>> {
        self.storage_info.used = self.storage_stat.update(&self.storage)?;
        if self.storage_info.used.cells.as_u64() >= dict_hash_min_cells as u64
            && !self.addr.is_masterchain()
        {
            let dict_root = self.storage_stat.dict_root()?;
            self.storage_info.storage_extra.dict_hash =
                Some(dict_root.map_or_else(Default::default, Cell::repr_hash));
            Ok(dict_root.cloned())
        } else {
            self.storage_info.storage_extra.dict_hash = None;
            Ok(None)
        }
    }

    fn init_storage_stat(&mut self, dict_hash_min_cells: u32) -> Result<Option<Cell>> {
        let dict_hash = self.storage_info.dict_hash().cloned();
        let used = self.storage_info.used.clone();
        let result = self.update_storage_stat(dict_hash_min_cells)?;
        if dict_hash.as_ref() != self.storage_info.dict_hash() {
            fail!(
                "Storage stat dict hash mismatch, expected {:?}, got {:?}",
                dict_hash,
                self.storage_info.dict_hash()
            );
        }
        if used != self.storage_info.used {
            fail!(
                "Storage used mismatch after storage stat init, expected {}, got {}",
                used,
                self.storage_info.used
            );
        }
        Ok(result)
    }

    fn import_storage_stat_dict(&mut self, dict: Cell) -> Result<()> {
        let dict_hash = self
            .storage_info
            .dict_hash()
            .ok_or_else(|| error!("Cannot import storage stat dict: dict_hash is None"))?;
        if &dict.repr_hash() != dict_hash {
            fail!(
                "Cannot import storage stat dict: hash mismatch, expected {:x}, got {:x}",
                dict_hash,
                dict.repr_hash()
            )
        }
        self.storage_stat =
            AccountStorageStat::try_from_dict(dict, &self.storage, &self.storage_info.used)?;
        Ok(())
    }
}

impl Serializable for AccountStuff {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        self.addr.write_to(builder)?;
        self.storage_info.write_to(builder)?;
        self.storage.write_to(builder)?;
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct Account {
    stuff: Option<AccountStuff>,
}

impl PartialEq for Account {
    fn eq(&self, other: &Account) -> bool {
        match (self.stuff(), other.stuff()) {
            (Some(stuff1), Some(stuff2)) => {
                stuff1.addr == stuff2.addr
                    && stuff1.storage_info == stuff2.storage_info
                    && stuff1.storage == stuff2.storage
            }
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for Account {}

impl Account {
    pub const fn new() -> Self {
        Account { stuff: None }
    }
    const fn with_stuff(stuff: AccountStuff) -> Self {
        debug_assert!(stuff.addr.rewrite_pfx().is_none());
        Self { stuff: Some(stuff) }
    }

    pub fn active(
        addr: MsgAddressInt,
        balance: CurrencyCollection,
        last_trans_lt: u64,
        last_paid: u32,
        state_init: StateInit,
        dict_hash_min_cells: u32,
    ) -> Result<Self> {
        let mut account = Account::with_stuff(AccountStuff {
            addr,
            storage_info: StorageInfo::with_values(last_paid, None),
            storage: AccountStorage::active(last_trans_lt, balance, state_init),
            storage_stat: AccountStorageStat::new(),
        });
        account.update_storage_stat(dict_hash_min_cells)?;
        Ok(account)
    }

    pub fn active_standard(
        addr: impl Into<AccountId>,
        balance: u64,
        last_trans_lt: u64,
        last_paid: u32,
        state_init: StateInit,
    ) -> Self {
        Account::active(
            MsgAddressInt::standard(-1, addr),
            CurrencyCollection::with_grams(balance),
            last_trans_lt,
            last_paid,
            state_init,
            DICT_HASH_MIN_CELLS,
        )
        .unwrap()
    }

    ///
    /// create unintialized account, only with address and balance
    ///
    pub fn with_address_and_ballance(addr: &MsgAddressInt, balance: &CurrencyCollection) -> Self {
        Account::with_stuff(AccountStuff {
            addr: addr.clone(),
            storage_info: StorageInfo::default(),
            storage: AccountStorage::with_balance(balance.clone()),
            storage_stat: AccountStorageStat::new(),
        })
    }

    ///
    /// Create unintialize account with zero balance
    ///
    pub fn with_address(addr: MsgAddressInt) -> Self {
        Account::with_stuff(AccountStuff {
            addr,
            storage_info: StorageInfo::new(),
            storage: AccountStorage::new(),
            storage_stat: AccountStorageStat::new(),
        })
    }

    ///
    /// Create initialized account from "constructor internal message"
    ///
    pub fn from_message(msg: &Message) -> Option<Self> {
        let hdr = msg.int_header()?;
        if hdr.value().grams.is_zero() {
            return None;
        }
        let mut storage = AccountStorage { balance: hdr.value().clone(), ..Default::default() };
        if let Some(init) = msg.state_init() {
            init.code()?;
            storage.state = AccountState::AccountActive { state_init: init.clone() };
        } else if hdr.bounce {
            return None;
        }
        let account = Account::with_stuff(AccountStuff {
            addr: hdr.dst.clone(),
            storage_info: StorageInfo::new(),
            storage,
            storage_stat: AccountStorageStat::new(),
        });
        Some(account)
    }

    // freeze active account
    pub fn try_freeze(&mut self) -> Result<()> {
        if let Some(stuff) = self.stuff_mut() {
            if let AccountState::AccountActive { state_init } = &stuff.storage.state {
                let state_init_hash = state_init.hash()?;
                stuff.storage.state = AccountState::AccountFrozen { state_init_hash };
            }
        }
        Ok(())
    }

    // make account uninitialized
    pub fn uninit_account(&mut self) {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage.state = AccountState::AccountUninit;
        } else {
            debug_assert!(false, "Account is None and cannot be made uninitialized")
        }
    }

    /// create frozen account - for test purposes
    pub fn frozen(
        addr: MsgAddressInt,
        balance: CurrencyCollection,
        last_trans_lt: u64,
        last_paid: u32,
        due_payment: Option<Grams>,
        state_hash: UInt256,
    ) -> Self {
        let storage = AccountStorage::frozen(last_trans_lt, balance, state_hash);
        let used = storage.calc_storage_used().unwrap();
        let storage_info = StorageInfo {
            used,
            storage_extra: StorageExtraInfo::default(),
            last_paid,
            due_payment,
        };
        let storage_stat = AccountStorageStat::new();
        let stuff = AccountStuff { addr, storage_info, storage, storage_stat };
        Account::with_stuff(stuff)
    }

    pub fn frozen_standard(
        addr: impl Into<AccountId>,
        balance: u64,
        last_trans_lt: u64,
        last_paid: u32,
        due_payment: u64,
        state_hash: UInt256,
    ) -> Self {
        Account::frozen(
            MsgAddressInt::standard(-1, addr),
            CurrencyCollection::with_grams(balance),
            last_trans_lt,
            last_paid,
            if due_payment == 0 { None } else { Some(due_payment.into()) },
            state_hash,
        )
    }

    /// create uninit account - for test purposes
    pub fn uninit(
        addr: MsgAddressInt,
        balance: CurrencyCollection,
        last_trans_lt: u64,
        last_paid: u32,
    ) -> Self {
        let storage = AccountStorage { last_trans_lt, balance, state: AccountState::AccountUninit };
        let bits = storage.write_to_new_cell().unwrap().length_in_bits();
        let storage_info = StorageInfo {
            used: StorageUsed::with_values_checked(1, bits as u64).unwrap(),
            storage_extra: StorageExtraInfo::default(),
            last_paid,
            due_payment: None,
        };
        let storage_stat = AccountStorageStat::new();
        let stuff = AccountStuff { addr, storage_info, storage, storage_stat };
        Account::with_stuff(stuff)
    }

    pub fn uninit_standard(
        addr: impl Into<AccountId>,
        balance: u64,
        last_trans_lt: u64,
        last_paid: u32,
    ) -> Self {
        Account::uninit(
            MsgAddressInt::standard(-1, addr),
            CurrencyCollection::with_grams(balance),
            last_trans_lt,
            last_paid,
        )
    }

    // constructor only for tests
    pub fn with_storage(
        addr: &MsgAddressInt,
        storage_info: &StorageInfo,
        storage: &AccountStorage,
    ) -> Self {
        Account::with_stuff(AccountStuff {
            addr: addr.clone(),
            storage_info: storage_info.clone(),
            storage: storage.clone(),
            storage_stat: AccountStorageStat::new(),
        })
    }

    pub fn is_none(&self) -> bool {
        self.stuff().is_none()
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state(), Some(AccountState::AccountActive { .. }))
    }

    pub fn is_uninit(&self) -> bool {
        matches!(self.state(), Some(AccountState::AccountUninit))
    }

    pub fn is_frozen(&self) -> bool {
        matches!(self.state(), Some(AccountState::AccountFrozen { .. }))
    }

    pub fn frozen_hash(&self) -> Option<&UInt256> {
        match self.state() {
            Some(AccountState::AccountFrozen { state_init_hash }) => Some(state_init_hash),
            _ => None,
        }
    }

    pub fn belongs_to_shard(&self, shard: &ShardIdent) -> Result<bool> {
        match self.get_addr() {
            Some(addr) => Ok(addr.workchain_id() == shard.workchain_id()
                && shard.contains_account(addr.address())?),
            None => fail!("Account is None"),
        }
    }

    fn stuff(&self) -> Option<&AccountStuff> {
        self.stuff.as_ref()
    }

    fn stuff_mut(&mut self) -> Option<&mut AccountStuff> {
        self.stuff.as_mut()
    }

    pub fn dict_hash(&self) -> Option<&UInt256> {
        self.stuff().and_then(|s| s.storage_info.dict_hash())
    }

    pub fn update_storage_stat(&mut self, dict_hash_min_cells: u32) -> Result<Option<Cell>> {
        match self.stuff_mut() {
            Some(stuff) => stuff.update_storage_stat(dict_hash_min_cells),
            None => Ok(None),
        }
    }

    pub fn init_storage_stat(&mut self, dict_hash_min_cells: u32) -> Result<Option<Cell>> {
        match self.stuff_mut() {
            Some(stuff) => stuff.init_storage_stat(dict_hash_min_cells),
            None => Ok(None),
        }
    }

    pub fn import_storage_stat_dict(&mut self, dict: Cell) -> Result<()> {
        if let Some(stuff) = self.stuff_mut() {
            stuff.import_storage_stat_dict(dict)
        } else {
            fail!("Cannot import storage stat dict: account is None")
        }
    }

    pub fn storage_stat(&self) -> Option<&AccountStorageStat> {
        self.stuff().map(|stuff| &stuff.storage_stat)
    }

    pub fn del_storage_stat(&mut self) {
        if let Some(stuff) = self.stuff.as_mut() {
            stuff.storage_info.storage_extra.dict_hash = None;
        }
    }

    #[cfg(test)]
    /// getting statistic using storage for calculate storage/transfer fee
    fn storage_used(&self) -> Result<StorageUsed> {
        if let Some(stuff) = self.stuff() {
            stuff.storage.calc_storage_used()
        } else {
            Ok(StorageUsed::default())
        }
    }

    /// Getting account ID
    pub fn get_id(&self) -> Option<&AccountId> {
        Some(self.get_addr()?.address())
    }

    pub fn get_addr(&self) -> Option<&MsgAddressInt> {
        self.stuff().map(|s| &s.addr)
    }

    /// Get ref to account's AccountState.
    /// Return None if account is empty (AccountNone)
    fn state(&self) -> Option<&AccountState> {
        self.stuff().map(|s| &s.storage.state)
    }

    pub fn state_init(&self) -> Option<&StateInit> {
        match self.state() {
            Some(AccountState::AccountActive { state_init }) => Some(state_init),
            _ => None,
        }
    }

    pub fn state_init_mut(&mut self) -> Option<&mut StateInit> {
        self.stuff_mut().and_then(|stuff| stuff.state_init_mut())
    }

    pub fn get_tick_tock(&self) -> Option<&TickTock> {
        self.state_init().and_then(|s| s.special.as_ref())
    }

    /// Get ref to account's storage information.
    /// Return None if account is empty (AccountNone)
    pub fn storage_info(&self) -> Option<&StorageInfo> {
        self.stuff().map(|s| s.storage_info())
    }

    pub fn storage_info_cells(&self) -> u64 {
        self.stuff().map(|info| info.storage_info().used().cells()).unwrap_or(0)
    }

    /// getting the root of the cell with Code of Smart Contract
    pub fn code(&self) -> Option<&Cell> {
        self.state_init()?.code.as_ref()
    }

    /// getting the root of the cell with Code of Smart Contract
    pub fn get_code(&self) -> Option<Cell> {
        self.code().cloned()
    }

    /// getting the hash of the root of the cell with Code of Smart Contract
    pub fn get_code_hash(&self) -> Option<UInt256> {
        Some(self.state_init()?.code.as_ref()?.repr_hash())
    }

    /// getting the root of the cell with persistent Data of Smart Contract
    pub fn data(&self) -> Option<&Cell> {
        self.state_init()?.data.as_ref()
    }

    /// getting the root of the cell with persistent Data of Smart Contract
    pub fn get_data(&self) -> Option<Cell> {
        self.data().cloned()
    }

    /// getting hash of the root of the cell with persistent Data of Smart Contract
    pub fn get_data_hash(&self) -> Option<UInt256> {
        Some(self.state_init()?.data.as_ref()?.repr_hash())
    }

    /// save persistent data of smart contract
    /// (for example, after execute code of smart contract into transaction)
    pub fn set_data(&mut self, new_data: Cell) -> bool {
        if let Some(state_init) = self.state_init_mut() {
            state_init.set_data(new_data);
            return true;
        }
        false
    }

    /// set new code of smart contract
    pub fn set_code(&mut self, new_code: Cell) -> bool {
        if let Some(state_init) = self.state_init_mut() {
            state_init.set_code(new_code);
            return true;
        }
        false
    }

    pub fn library_mut(&mut self) -> Option<&mut StateInitLib> {
        Some(&mut self.state_init_mut()?.library)
    }

    /// Try to activate account with new StateInit
    pub fn try_activate(&mut self, state_init: &StateInit) -> Result<()> {
        if let Some(stuff) = self.stuff_mut() {
            let new_state = match &stuff.storage.state {
                AccountState::AccountUninit => {
                    if stuff.addr.address().contains_bytes(state_init.hash()?.as_slice()) {
                        AccountState::AccountActive { state_init: state_init.clone() }
                    } else {
                        fail!("StateInit doesn't correspond to uninit account address")
                    }
                }
                AccountState::AccountFrozen { state_init_hash } => {
                    if state_init_hash == &state_init.hash()? {
                        AccountState::AccountActive { state_init: state_init.clone() }
                    } else {
                        fail!("StateInit doesn't correspond to frozen hash")
                    }
                }
                _ => stuff.storage.state.clone(),
            };
            stuff.storage.state = new_state;
            Ok(())
        } else {
            fail!("Cannot activate not existing account")
        }
    }

    /// getting to the root of the cell with library
    pub fn libraries(&self) -> StateInitLib {
        match self.state_init() {
            Some(state_init) => state_init.libraries().clone(),
            None => StateInitLib::default(),
        }
    }

    /// Get enum variant indicating current state of account
    pub fn status(&self) -> AccountStatus {
        if let Some(stuff) = self.stuff() {
            match stuff.storage.state() {
                AccountState::AccountUninit => AccountStatus::AccStateUninit,
                AccountState::AccountFrozen { state_init_hash: _ } => AccountStatus::AccStateFrozen,
                AccountState::AccountActive { state_init: _ } => AccountStatus::AccStateActive,
            }
        } else {
            AccountStatus::AccStateNonexist
        }
    }

    pub fn last_paid(&self) -> u32 {
        match self.stuff() {
            Some(stuff) => stuff.storage_info.last_paid,
            None => 0,
        }
    }

    /// calculate storage fee and sub funds, freeze if not enough
    pub fn set_last_paid(&mut self, last_paid: u32) {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage_info.last_paid = last_paid;
        }
    }

    /// getting due payment
    pub fn due_payment(&self) -> Option<&Grams> {
        self.stuff().and_then(|s| s.storage_info.due_payment.as_ref())
    }

    /// setting due payment
    pub fn set_due_payment(&mut self, due_payment: Option<Grams>) {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage_info.due_payment = due_payment
        } else {
            debug_assert!(due_payment.is_none(), "Account is None, but due payment is not None");
        }
    }

    /// getting balance of the account
    pub fn balance(&self) -> Option<&CurrencyCollection> {
        self.stuff().map(|s| &s.storage.balance)
    }

    /// deprecated: getting balance of the account
    pub fn get_balance(&self) -> Option<&CurrencyCollection> {
        self.balance()
    }

    /// getting balance of the account or empty balance
    pub fn balance_checked(&self) -> CurrencyCollection {
        match self.stuff() {
            Some(s) => s.storage.balance.clone(),
            None => CurrencyCollection::default(),
        }
    }

    /// setting balance of the account
    pub fn set_balance(&mut self, balance: CurrencyCollection) {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage.balance = balance
        } else {
            debug_assert!(balance.is_zero().unwrap(), "Account is None, but balance is not zero");
        }
    }

    /// adding funds to account (for example, for credit phase transaction)
    pub fn add_funds(&mut self, funds_to_add: &CurrencyCollection) -> Result<()> {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage.balance.add(funds_to_add)?;
        } else {
            debug_assert!(
                funds_to_add.grams.is_zero(),
                "Account is None, but funds to add is not zero"
            );
        }
        Ok(())
    }

    /// subtraction funds from account (for example, rollback transaction)
    pub fn sub_funds(&mut self, funds_to_sub: &CurrencyCollection) -> Result<bool> {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage.balance.sub(funds_to_sub)
        } else {
            debug_assert!(
                funds_to_sub.grams.is_zero(),
                "Account is None, but funds to subtract is not zero"
            );
            Ok(false)
        }
    }

    pub fn fixed_prefix_length(&self) -> Option<Number5> {
        self.state_init().and_then(|s| s.fixed_prefix_length)
    }

    pub fn last_tr_time(&self) -> Option<u64> {
        self.stuff().map(|stuff| stuff.storage.last_trans_lt)
    }

    pub fn set_last_tr_time(&mut self, tr_lt: u64) {
        if let Some(stuff) = self.stuff_mut() {
            stuff.storage.last_trans_lt = tr_lt;
        }
    }

    pub fn prepare_proof(&self, state_root: &Cell) -> Result<Cell> {
        match self.get_id() {
            Some(addr) => {
                // proof for account in shard state

                let usage_tree = UsageTree::with_root(state_root.clone());
                let ss = ShardStateUnsplit::construct_from_cell(usage_tree.root_cell())?;

                ss.read_accounts()?
                    .get(addr)?
                    .ok_or_else(|| {
                        error!(BlockError::InvalidArg(
                            "Account doesn't belong to given shard state".to_string()
                        ))
                    })?
                    .read_account()?;

                MerkleProof::create_by_usage_tree(state_root, &usage_tree)
                    .and_then(|proof| proof.serialize())
            }
            None => fail!(BlockError::InvalidData("Account cannot be None".to_string())),
        }
    }

    fn read_original_format(slice: &mut SliceData) -> Result<Self> {
        let addr = Deserializable::construct_from(slice)?;
        let storage_info = Deserializable::construct_from(slice)?;
        let last_trans_lt = Deserializable::construct_from(slice)?; //last_trans_lt:uint64
        let balance = Deserializable::construct_from(slice)?; //balance:CurrencyCollection
        let state = Deserializable::construct_from(slice)?; //state:AccountState
        let storage = AccountStorage { last_trans_lt, balance, state };
        let storage_stat = AccountStorageStat::new();
        Ok(Account::with_stuff(AccountStuff { addr, storage_info, storage, storage_stat }))
    }

    fn read_version(slice: &mut SliceData, _version: u32) -> Result<Self> {
        let addr = Deserializable::construct_from(slice)?;
        let storage_info = Deserializable::construct_from(slice)?;
        let last_trans_lt = Deserializable::construct_from(slice)?; //last_trans_lt:uint64
        let balance = CurrencyCollection::construct_from(slice)?; //balance:CurrencyCollection
        let state = Deserializable::construct_from(slice)?; //state:AccountState
        let storage = AccountStorage { last_trans_lt, balance, state };
        let storage_stat = AccountStorageStat::new();
        let stuff = AccountStuff { addr, storage_info, storage, storage_stat };
        Ok(Account::with_stuff(stuff))
    }
}

// functions for testing purposes
impl Account {
    pub fn set_addr(&mut self, addr: MsgAddressInt) {
        if let Some(s) = self.stuff_mut() {
            s.addr = addr;
        }
    }

    pub fn update_config_smc(&mut self, config: &ConfigParams) -> Result<()> {
        let data = self.get_data().ok_or_else(|| error!("config SMC doesn't contain data"))?;
        let mut data = SliceData::load_cell(data)?;
        data.checked_drain_reference()
            .map_err(|_| error!("config SMC data doesn't contain reference with old config"))?;
        let mut builder = data.into_builder()?;
        let cell = config.config_params.data().ok_or_else(|| error!("configs musn't be empty"))?;
        builder.checked_prepend_reference(cell.clone())?;
        self.set_data(builder.into_cell()?);
        Ok(())
    }

    pub fn get_config(&self) -> Result<ConfigParams> {
        let data = self.get_data().ok_or_else(|| error!("config SMC doesn't contain data"))?;
        let mut data = SliceData::load_cell(data)?;
        let config_cell = data
            .checked_drain_reference()
            .map_err(|_| error!("config SMC data doesn't contain reference with config"))?;
        Ok(ConfigParams::with_root(config_cell))
    }
}

impl Augmentation<DepthBalanceInfo> for Account {
    fn aug(&self) -> Result<DepthBalanceInfo> {
        let mut info = DepthBalanceInfo::default();
        if let Some(balance) = self.balance() {
            info.set_balance(balance.clone());
        }
        if let Some(state_init) = self.state_init() {
            if let Some(fixed_prefix_length) = state_init.fixed_prefix_length {
                info.set_split_depth(fixed_prefix_length);
            }
        }
        Ok(info)
    }
}

impl Serializable for Account {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        if let Some(stuff) = self.stuff() {
            builder.append_bit_one()?;
            stuff.addr.write_to(builder)?;
            stuff.storage_info.write_to(builder)?;
            stuff.storage.last_trans_lt.write_to(builder)?; //last_trans_lt:uint64
            stuff.storage.balance.write_to(builder)?; //balance:CurrencyCollection
            stuff.storage.state.write_to(builder)?; //state:AccountState
        } else {
            builder.append_bit_zero()?;
        }
        Ok(())
    }
}

impl Deserializable for Account {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        if slice.get_next_bit()? {
            Self::read_original_format(slice)
        } else if slice.remaining_bits() == 0 {
            Ok(Account::default())
        } else {
            let tag = slice.get_next_int(3)? as u32;
            match tag {
                0 => Ok(Account::default()),
                1 => match Account::read_version(slice, tag) {
                    Ok(account) => Ok(account),
                    Err(err) => fail!("cannot deserialize account with tag {}, err {}", tag, err),
                },
                t => {
                    let s = format!("wrong tag {} deserializing account", tag);
                    fail!(BlockError::InvalidConstructorTag { t, s })
                }
            }
        }
    }
}

impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Account[{:?}]", self)
    }
}

/*
account_descr$_ account:^Account last_trans_hash:bits256
  last_trans_lt:uint64 = ShardAccount;
*/

/// struct ShardAccount
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ShardAccount {
    account: ChildCell<Account>,
    last_trans_hash: UInt256,
    last_trans_lt: u64,
}

impl ShardAccount {
    pub fn with_account_root(
        account_root: Cell,
        last_trans_hash: UInt256,
        last_trans_lt: u64,
    ) -> Self {
        ShardAccount { account: ChildCell::with_cell(account_root), last_trans_hash, last_trans_lt }
    }

    pub fn with_params(
        account: &Account,
        last_trans_hash: UInt256,
        last_trans_lt: u64,
    ) -> Result<Self> {
        Ok(ShardAccount {
            account: ChildCell::with_struct(account)?,
            last_trans_hash,
            last_trans_lt,
        })
    }

    pub fn read_account(&self) -> Result<Account> {
        self.account.read_struct()
    }

    pub fn write_account(&mut self, value: &Account) -> Result<()> {
        self.account.write_struct(value)
    }

    pub fn last_trans_hash(&self) -> &UInt256 {
        &self.last_trans_hash
    }

    pub fn set_last_trans_hash(&mut self, hash: UInt256) {
        self.last_trans_hash = hash
    }

    pub fn last_trans_lt(&self) -> u64 {
        self.last_trans_lt
    }

    pub fn set_last_trans_lt(&mut self, lt: u64) {
        self.last_trans_lt = lt
    }

    pub fn last_trans_hash_mut(&mut self) -> &mut UInt256 {
        &mut self.last_trans_hash
    }

    pub fn last_trans_lt_mut(&mut self) -> &mut u64 {
        &mut self.last_trans_lt
    }

    pub fn account_cell(&self) -> Cell {
        self.account.cell()
    }

    pub fn account_hash(&self) -> UInt256 {
        self.account.hash()
    }

    pub fn set_account_cell(&mut self, cell: Cell) {
        self.account.set_cell(cell);
    }
}

impl Serializable for ShardAccount {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.account.write_to(cell)?;
        self.last_trans_hash.write_to(cell)?;
        self.last_trans_lt.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for ShardAccount {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.account.read_from(cell)?;
        self.last_trans_hash.read_from(cell)?;
        self.last_trans_lt.read_from(cell)?;
        Ok(())
    }
}
