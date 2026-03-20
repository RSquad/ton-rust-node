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
    accounts::{Account, AccountStatus, StorageUsed},
    blocks::Block,
    define_HashmapAugE, define_HashmapE,
    dictionary::hashmapaug::{Augmentation, HashmapAugType},
    error,
    error::BlockError,
    fail,
    merkle_proof::MerkleProof,
    messages::Message,
    shard::ShardStateUnsplit,
    types::{ChildCell, CurrencyCollection, Coins, InRefValue, VarUInteger3, VarUInteger7},
    AccountId, BuilderData, Cell, Deserializable, HashmapType, IBitstring, Result, Serializable,
    SliceData, UInt256, UsageTree,
};
use std::{fmt, sync::Arc};

#[cfg(test)]
#[path = "tests/test_transactions.rs"]
pub(crate) mod tests;

/*
acst_unchanged$0 = AccStatusChange;  // x -> x
acst_frozen$10 = AccStatusChange;    // init -> frozen
acst_deleted$11 = AccStatusChange;   // frozen -> deleted
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AccStatusChange {
    #[default]
    Unchanged,
    Frozen,
    Deleted,
}

impl Serializable for AccStatusChange {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let (tag, bits_count) = match self {
            AccStatusChange::Unchanged => (0b0, 1),
            AccStatusChange::Frozen => (0b10, 2),
            AccStatusChange::Deleted => (0b11, 2),
        };
        cell.append_bits(tag, bits_count)?;
        Ok(())
    }
}

impl Deserializable for AccStatusChange {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = match cell.get_next_bit()? {
            false => AccStatusChange::Unchanged,
            true => match cell.get_next_bit()? {
                false => AccStatusChange::Frozen,
                true => AccStatusChange::Deleted,
            },
        };
        Ok(())
    }
}

/*
cskip_no_state$00 = ComputeSkipReason;
cskip_bad_state$01 = ComputeSkipReason;
cskip_no_gas$10 = ComputeSkipReason;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq, Copy)]
pub enum ComputeSkipReason {
    #[default]
    NoState = 1,
    BadState = 2,
    NoGas = 3,
    Suspended = 4,
}

impl Serializable for ComputeSkipReason {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let (tag, bits) = match self {
            ComputeSkipReason::NoState => (0b00, 2),
            ComputeSkipReason::BadState => (0b01, 2),
            ComputeSkipReason::NoGas => (0b10, 2),
            ComputeSkipReason::Suspended => (0b110, 3),
        };
        cell.append_bits(tag, bits)?;
        Ok(())
    }
}

impl Deserializable for ComputeSkipReason {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = match cell.get_next_bits(2)?[0] {
            0b00000000 => ComputeSkipReason::NoState,
            0b01000000 => ComputeSkipReason::BadState,
            0b10000000 => ComputeSkipReason::NoGas,
            tag => {
                // 0b11000000
                match cell.get_next_bit()? {
                    false => ComputeSkipReason::Suspended,
                    true => fail!(BlockError::InvalidConstructorTag {
                        t: tag as u32,
                        s: "ComputeSkipReason".to_string()
                    }),
                }
            }
        };
        Ok(())
    }
}

/*
tr_phase_compute_skipped$0
    reason:ComputeSkipReason
= TrComputePhase;

tr_phase_compute_vm$1
    success:Bool
    msg_state_used:Bool
    account_activated:Bool
    gas_fees:Coins
    _:^[
        gas_used:(VarUInteger 7)
        gas_limit:(VarUInteger 7)
        gas_credit:(Maybe (VarUInteger 3))
        mode:int8
        exit_code:int32
        exit_arg:(Maybe int32)
        vm_steps:uint32
        vm_init_state_hash:uint256
        vm_final_state_hash:uint256
    ]
  = TrComputePhase;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrComputePhaseSkipped {
    pub reason: ComputeSkipReason,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrComputePhaseVm {
    pub success: bool,
    pub msg_state_used: bool,
    pub account_activated: bool,
    pub gas_fees: Coins,
    pub gas_used: VarUInteger7,
    pub gas_limit: VarUInteger7,
    pub gas_credit: Option<VarUInteger3>,
    pub mode: i8,
    pub exit_code: i32,
    pub exit_arg: Option<i32>,
    pub vm_steps: u32,
    pub vm_init_state_hash: UInt256,
    pub vm_final_state_hash: UInt256,
}

impl TrComputePhaseVm {
    /// Set flag, that account is activated. Use 'msg_used' parameter
    /// to indicate that inbound message is used for this activation.
    /// Due to Durov's code does not do enything
    pub fn activated(_msg_used: bool) -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrComputePhase {
    Skipped(TrComputePhaseSkipped),
    Vm(TrComputePhaseVm),
}

impl TrComputePhase {
    pub fn skipped(reason: ComputeSkipReason) -> Self {
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason })
    }

    pub fn get_vmphase_mut(&mut self) -> Option<&mut TrComputePhaseVm> {
        match self {
            TrComputePhase::Vm(ref mut vm_ref) => Some(vm_ref),
            _ => None,
        }
    }

    /// Set flag, that account is activated. Use 'msg_used' parameter
    /// to indicate that inbound message is used for this activation.
    pub fn activated(&mut self, _msg_used: bool) {
        match self {
            TrComputePhase::Vm(_phase_ref) => {
                // removed due Durov's code
                // phase_ref.account_activated = true;
                // phase_ref.msg_state_used = msg_used;
            }
            _ => {
                let vm_phase = TrComputePhaseVm::default();
                // removed due Durov's code
                // vm_phase.account_activated = true;
                // vm_phase.msg_state_used = msg_used;
                *self = TrComputePhase::Vm(vm_phase);
            }
        }
    }

    pub fn is_accepted(&self) -> bool {
        match self {
            TrComputePhase::Vm(ref vm_phase) => vm_phase.gas_credit.is_none(),
            _ => false,
        }
    }

    pub fn is_skipped(&self) -> Option<&ComputeSkipReason> {
        match self {
            TrComputePhase::Skipped(skipped_phase) => Some(&skipped_phase.reason),
            TrComputePhase::Vm(_) => None,
        }
    }

    pub fn is_success(&self) -> Option<&TrComputePhaseVm> {
        match self {
            TrComputePhase::Vm(vm_phase) if vm_phase.success => Some(vm_phase),
            _ => None,
        }
    }

    pub fn gas_fees(&self) -> Coins {
        match self {
            TrComputePhase::Vm(vm_phase) => vm_phase.gas_fees,
            _ => Coins::zero(),
        }
    }
}

impl Default for TrComputePhase {
    fn default() -> Self {
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason: ComputeSkipReason::NoState })
    }
}

impl Serializable for TrComputePhase {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if let TrComputePhase::Skipped(s) = self {
            cell.append_bit_zero()?; // tr_phase_compute_skipped$0
            s.reason.write_to(cell)?; // reason:ComputeSkipReason
        } else if let TrComputePhase::Vm(v) = self {
            cell.append_bit_one()? // tr_phase_compute_vm$1
                .append_bit_bool(v.success)? // success:Bool
                .append_bit_bool(v.msg_state_used)? // msg_state_used:Bool
                .append_bit_bool(v.account_activated)?; // account_activated:Bool

            v.gas_fees.write_to(cell)?; // gas_fees:Coin

            // fields below are serialized into separate cell
            let mut sep_cell = BuilderData::new();
            v.gas_used.write_to(&mut sep_cell)?; // gas_used:(VarUInteger 7)
            v.gas_limit.write_to(&mut sep_cell)?; // gas_limit:(VarUInteger 7)
            v.gas_credit.write_to(&mut sep_cell)?; // gas_credit:(Maybe (VarUInteger 3))
            v.mode.write_to(&mut sep_cell)?; // mode:int8
            v.exit_code.write_to(&mut sep_cell)?; // exit_code:int32
            v.exit_arg.write_to(&mut sep_cell)?; // exit_arg:(Maybe int32)
            v.vm_steps.write_to(&mut sep_cell)?; // vm_steps:uint32
            v.vm_init_state_hash.write_to(&mut sep_cell)?; // vm_init_state_hash:uint256
            v.vm_final_state_hash.write_to(&mut sep_cell)?; // vm_final_state_hash:uint256

            cell.checked_append_reference(sep_cell.into_cell()?)?;
        }

        Ok(())
    }
}

impl Deserializable for TrComputePhase {
    fn construct_from(cell: &mut SliceData) -> Result<Self> {
        if !cell.get_next_bit()? {
            let reason = Deserializable::construct_from(cell)?;
            Ok(TrComputePhase::Skipped(TrComputePhaseSkipped { reason }))
        } else {
            // tr_phase_compute_vm$1
            let success = cell.get_next_bit()?; // success:Bool
            let msg_state_used = cell.get_next_bit()?; // msg_state_used:Bool
            let account_activated = cell.get_next_bit()?; // account_activated:Bool
            let gas_fees = Deserializable::construct_from(cell)?; // gas_fees:Coin

            // fields below are serialized into separate cell
            let sep_cell = &mut SliceData::load_cell(cell.checked_drain_reference()?)?;

            let gas_used = Deserializable::construct_from(sep_cell)?; // gas_used:(VarUInteger 7)
            let gas_limit = Deserializable::construct_from(sep_cell)?; // gas_limit:(VarUInteger 7)
            let gas_credit = Deserializable::construct_maybe_from(sep_cell)?; // gas_credit:(Maybe (VarUInteger 3))
            let mode = Deserializable::construct_from(sep_cell)?; // mode:int8
            let exit_code = Deserializable::construct_from(sep_cell)?; // exit_code:int32
            let exit_arg = Deserializable::construct_maybe_from(sep_cell)?; // exit_arg:(Maybe int32)
            let vm_steps = Deserializable::construct_from(sep_cell)?; // vm_steps:uint32
            let vm_init_state_hash = Deserializable::construct_from(sep_cell)?; // vm_init_state_hash:uint256
            let vm_final_state_hash = Deserializable::construct_from(sep_cell)?; // vm_final_state_hash:uint256
            let v = TrComputePhaseVm {
                success,
                msg_state_used,
                account_activated,
                gas_fees,
                gas_used,
                gas_limit,
                gas_credit,
                mode,
                exit_code,
                exit_arg,
                vm_steps,
                vm_init_state_hash,
                vm_final_state_hash,
            };
            Ok(TrComputePhase::Vm(v))
        }
    }
}

/*
tr_phase_storage$_
  storage_fees_collected:Coins
  storage_fees_due:(Maybe Coins)
  status_change:AccStatusChange
= TrStoragePhase;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrStoragePhase {
    pub storage_fees_collected: Coins,
    pub storage_fees_due: Option<Coins>,
    pub status_change: AccStatusChange,
}

impl TrStoragePhase {
    pub const fn with_params(
        collected: Coins,
        due: Option<Coins>,
        status: AccStatusChange,
    ) -> Self {
        TrStoragePhase {
            storage_fees_collected: collected,
            storage_fees_due: due,
            status_change: status,
        }
    }
}

impl Serializable for TrStoragePhase {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.storage_fees_collected.write_to(cell)?;
        self.storage_fees_due.write_to(cell)?;
        self.status_change.write_to(cell)?;

        Ok(())
    }
}

impl Deserializable for TrStoragePhase {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.storage_fees_collected.read_from(cell)?;
        self.storage_fees_due.read_from(cell)?;
        self.status_change.read_from(cell)?;

        Ok(())
    }
}

/*
tr_phase_bounce_negfunds$00 = TrBouncePhase;

tr_phase_bounce_nofunds$01
  msg_size:StorageUsed
  req_fwd_fees:Coins
= TrBouncePhase;

tr_phase_bounce_ok$1
  msg_size:StorageUsed
  msg_fees:Coins
  fwd_fees:Coins
= TrBouncePhase;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrBouncePhaseNofunds {
    pub msg_size: StorageUsed,
    pub req_fwd_fees: Coins,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrBouncePhaseOk {
    pub msg_size: StorageUsed,
    pub msg_fees: Coins,
    pub fwd_fees: Coins,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum TrBouncePhase {
    #[default]
    Negfunds,
    Nofunds(TrBouncePhaseNofunds),
    Ok(TrBouncePhaseOk),
}

impl TrBouncePhase {
    pub const fn ok(msg_size: StorageUsed, msg_fees: Coins, fwd_fees: Coins) -> Self {
        TrBouncePhase::Ok(TrBouncePhaseOk::with_params(msg_size, msg_fees, fwd_fees))
    }
    pub const fn no_funds(msg_size: StorageUsed, req_fwd_fees: Coins) -> Self {
        TrBouncePhase::Nofunds(TrBouncePhaseNofunds::with_params(msg_size, req_fwd_fees))
    }
}

impl Serializable for TrBouncePhase {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            TrBouncePhase::Negfunds => {
                //tr_phase_bounce_negfunds$00
                cell.append_bits(0b00, 2)?;
            }
            TrBouncePhase::Nofunds(bp) => {
                // tr_phase_bounce_nofunds$01
                cell.append_bits(0b01, 2)?;
                bp.msg_size.write_to(cell)?; // msg_size:StorageUsed
                bp.req_fwd_fees.write_to(cell)?; // req_fwd_fees:Coins
            }
            TrBouncePhase::Ok(bp) => {
                // tr_phase_bounce_ok$1
                cell.append_bit_one()?;
                bp.msg_size.write_to(cell)?; // msg_size:StorageUsed
                bp.msg_fees.write_to(cell)?; // msg_fees:Coins
                bp.fwd_fees.write_to(cell)?; // fwd_fees:Coins
            }
        };
        Ok(())
    }
}

impl Deserializable for TrBouncePhase {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        if cell.get_next_bit()? {
            // tr_phase_bounce_ok$1
            let mut bp = TrBouncePhaseOk::default();
            bp.msg_size.read_from(cell)?; // msg_size:StorageUsed
            bp.msg_fees.read_from(cell)?; // msg_fees:Coins
            bp.fwd_fees.read_from(cell)?; // fwd_fees:Coins
            *self = TrBouncePhase::Ok(bp);
        } else if cell.get_next_bit()? {
            // tr_phase_bounce_nofunds$01
            let mut bp = TrBouncePhaseNofunds::default();
            bp.msg_size.read_from(cell)?; // msg_size:StorageUsed
            bp.req_fwd_fees.read_from(cell)?; // req_fwd_fees:Coins
            *self = TrBouncePhase::Nofunds(bp);
        } else {
            //tr_phase_bounce_negfunds$00
            *self = TrBouncePhase::Negfunds;
        }
        Ok(())
    }
}

impl TrBouncePhaseOk {
    pub const fn with_params(msg_size: StorageUsed, msg_fees: Coins, fwd_fees: Coins) -> Self {
        TrBouncePhaseOk { msg_size, msg_fees, fwd_fees }
    }
}

impl TrBouncePhaseNofunds {
    pub const fn with_params(msg_size: StorageUsed, req_fwd_fees: Coins) -> Self {
        TrBouncePhaseNofunds { msg_size, req_fwd_fees }
    }
}

/*
tr_phase_credit$_
    due_fees_collected:(Maybe Coins)
    credit:CurrencyCollection
= TrCreditPhase;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrCreditPhase {
    pub due_fees_collected: Option<Coins>,
    pub credit: CurrencyCollection,
}

impl TrCreditPhase {
    pub const fn new(credit: CurrencyCollection) -> Self {
        TrCreditPhase { due_fees_collected: None, credit }
    }
}

impl Serializable for TrCreditPhase {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.due_fees_collected.write_to(cell)?;
        self.credit.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for TrCreditPhase {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.due_fees_collected.read_from(cell)?;
        self.credit.read_from(cell)?;
        Ok(())
    }
}

/*
tick$0 = TickTock;
tock$1 = TickTock;
There are two kinds of TickTock: in transaction and in messages.
*/
#[derive(Default, PartialEq, Eq, Clone, Debug)]
pub enum TransactionTickTock {
    #[default]
    Tick,
    Tock,
}

impl TransactionTickTock {
    pub fn new(is_tock: bool) -> Self {
        if is_tock {
            TransactionTickTock::Tock
        } else {
            TransactionTickTock::Tick
        }
    }
    pub fn is_tick(&self) -> bool {
        self == &TransactionTickTock::Tick
    }
    pub fn is_tock(&self) -> bool {
        self == &TransactionTickTock::Tock
    }
}

impl Serializable for TransactionTickTock {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            TransactionTickTock::Tick => cell.append_bit_zero()?,
            TransactionTickTock::Tock => cell.append_bit_one()?,
        };
        Ok(())
    }
}

impl Deserializable for TransactionTickTock {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        if cell.get_next_bit()? {
            *self = TransactionTickTock::Tock
        } else {
            *self = TransactionTickTock::Tick
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrActionPhase {
    pub success: bool,
    pub valid: bool,
    pub no_funds: bool,
    pub status_change: AccStatusChange,
    // TODO: remove two next Options and store if it is not zero
    pub total_fwd_fees: Option<Coins>,
    pub total_action_fees: Option<Coins>,
    pub result_code: i32,
    pub result_arg: Option<i32>,
    pub tot_actions: i16,
    pub spec_actions: i16,
    pub skipped_actions: i16,
    pub msgs_created: i16,
    pub action_list_hash: UInt256,
    pub tot_msg_size: StorageUsed,
    pub action_fine: Coins,
}

impl TrActionPhase {
    pub fn add_fwd_fees(&mut self, fees: &Coins) {
        if !fees.is_zero() {
            self.total_fwd_fees.get_or_insert_default().add_checked(fees);
        }
    }
    pub fn total_fwd_fees(&self) -> Coins {
        self.total_fwd_fees.unwrap_or_default()
    }
    pub fn del_action_fees(&mut self) {
        self.total_action_fees = None;
    }
    pub fn add_action_fees(&mut self, fees: &Coins) {
        if !fees.is_zero() {
            self.total_action_fees.get_or_insert_default().add_checked(fees);
        }
    }
    pub fn add_action_fine(&mut self) {
        if !self.action_fine.is_zero() {
            self.total_action_fees.get_or_insert_default().add_checked(&self.action_fine);
        }
    }
    pub fn total_action_fees(&self) -> Coins {
        self.total_action_fees.unwrap_or_default()
    }
}

impl Serializable for TrActionPhase {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_bool(self.success)? // success:Bool
            .append_bit_bool(self.valid)? // valid:Bool
            .append_bit_bool(self.no_funds)?; // no_funds:Bool
        self.status_change.write_to(cell)?; // status_change:AccStatusChange
        self.total_fwd_fees.write_to(cell)?; // total_fwd_fees:(Maybe Coins)
        self.total_action_fees.write_to(cell)?; // total_action_fees:(Maybe Coins)
        self.result_code.write_to(cell)?; // result_code:int32
        self.result_arg.write_to(cell)?; // result_arg:(Maybe int32)
        self.tot_actions.write_to(cell)?; // tot_actions:uint16
        self.spec_actions.write_to(cell)?; // spec_actions:uint16
        self.skipped_actions.write_to(cell)?; // skipped_actions: uint16
        self.msgs_created.write_to(cell)?; // msgs_created:uint16
        self.action_list_hash.write_to(cell)?; // action_list_hash:uint256
        self.tot_msg_size.write_to(cell)?; // tot_msg_size:StorageUsed
        Ok(())
    }
}

impl Deserializable for TrActionPhase {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.success = cell.get_next_bit()?; // success:Bool
        self.valid = cell.get_next_bit()?; // valid:Bool
        self.no_funds = cell.get_next_bit()?; // no_funds:Bool
        self.status_change.read_from(cell)?; // status_change:AccStatusChange
        self.total_fwd_fees.read_from(cell)?; // total_fwd_fees:(Maybe Coins)
        self.total_action_fees.read_from(cell)?; // total_action_fees:(Maybe Coins)
        self.result_code.read_from(cell)?; // result_code:int32
        self.result_arg.read_from(cell)?; // result_arg:(Maybe int32)
        self.tot_actions.read_from(cell)?; // tot_actions:uint16
        self.spec_actions.read_from(cell)?; // spec_actions:uint16
        self.skipped_actions.read_from(cell)?; // skipped_actions: uint16
        self.msgs_created.read_from(cell)?; // msgs_created:uint16
        self.action_list_hash.read_from(cell)?; // action_list_hash:uint256
        self.tot_msg_size.read_from(cell)?; // tot_msg_size:StorageUsed
        Ok(())
    }
}

/*
split_merge_info$_
  cur_shard_pfx_len:(## 6)
  acc_split_depth:(##6)
  this_addr:uint256
  sibling_addr:uint256
= SplitMergeInfo;
*/

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitMergeInfo {
    pub cur_shard_pfx_len: u8,
    pub acc_split_depth: u8,
    pub this_addr: AccountId,
    pub sibling_addr: AccountId,
}

impl Serializable for SplitMergeInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if 0 != self.cur_shard_pfx_len & 0b11000000 {
            fail!(BlockError::InvalidData("self.cur_shard_pfx_len is too long".to_string()))
        } else {
            cell.append_bits(self.cur_shard_pfx_len as usize, 6)?;
        }
        if 0 != self.acc_split_depth & 0b11000000 {
            fail!(BlockError::InvalidData("self.acc_split_depth is too long".to_string()))
        } else {
            cell.append_bits(self.acc_split_depth as usize, 6)?;
        }
        self.this_addr.write_to(cell)?;
        self.sibling_addr.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for SplitMergeInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.cur_shard_pfx_len = cell.get_next_bits(6)?[0] >> 2;
        self.acc_split_depth = cell.get_next_bits(6)?[0] >> 2;
        self.this_addr.read_from(cell)?;
        self.sibling_addr.read_from(cell)?;
        Ok(())
    }
}

/*
trans_ord$0000
    storage_ph:(Maybe TrStoragePhase)
    credit_ph:(Maybe TrCreditPhase)
    compute_ph:TrComputePhase
    action:(Maybe ^TrActionPhase)
    aborted:Boolean
    bounce:(Maybe TrBouncePhase)
    destroyed:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrOrdinary {
    pub credit_first: bool,
    pub storage_ph: Option<TrStoragePhase>,
    pub credit_ph: Option<TrCreditPhase>,
    pub compute_ph: TrComputePhase,
    pub action: Option<TrActionPhase>,
    pub aborted: bool,
    pub bounce: Option<TrBouncePhase>,
    pub destroyed: bool,
}

impl Serializable for TransactionDescrOrdinary {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        // constructor tag is written in TransactionDescr::write_to
        cell.append_bit_bool(self.credit_first)?;
        self.storage_ph.write_to(cell)?;
        self.credit_ph.write_to(cell)?;
        self.compute_ph.write_to(cell)?;
        cell.append_bit_bool(self.action.is_some())?;
        cell.append_bit_bool(self.aborted)?;
        self.bounce.write_to(cell)?;
        cell.append_bit_bool(self.destroyed)?;

        if let Some(a) = &self.action {
            a.serialize()?.write_to(cell)?;
        }

        Ok(())
    }
}

impl Deserializable for TransactionDescrOrdinary {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // constructor tag is read in TransactionDescr::write_to
        self.credit_first = cell.get_next_bit()?;
        self.storage_ph.read_from(cell)?;
        self.credit_ph.read_from(cell)?;
        self.compute_ph.read_from(cell)?;
        self.action = if cell.get_next_bit()? {
            Some(TrActionPhase::construct_from_cell(cell.checked_drain_reference()?)?)
        } else {
            None
        };
        self.aborted = cell.get_next_bit()?;
        self.bounce.read_from(cell)?;
        self.destroyed = cell.get_next_bit()?;
        Ok(())
    }
}

/*
trans_storage$0001
    storage_ph:TrStoragePhase
constructor tag is written and read in TransactionDescr::write_to
*/
type TransactionDescrStorage = TrStoragePhase;

/*
trans_tick_tock$001
    tt:TickTock
    storage:TrStoragePhase
    compute_ph:TrComputePhase
    action:(Maybe ^TrActionPhase)
    aborted:Boolean
    destroyed:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrTickTock {
    pub tt: TransactionTickTock,
    pub storage: TrStoragePhase,
    pub compute_ph: TrComputePhase,
    pub action: Option<TrActionPhase>,
    pub aborted: bool,
    pub destroyed: bool,
}

impl TransactionDescrTickTock {
    pub fn tick() -> Self {
        Self { tt: TransactionTickTock::Tick, ..Self::default() }
    }
    pub fn tock() -> Self {
        Self { tt: TransactionTickTock::Tock, ..Self::default() }
    }
}

impl Serializable for TransactionDescrTickTock {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.tt.write_to(cell)?;
        self.storage.write_to(cell)?;
        self.compute_ph.write_to(cell)?;
        cell.append_bit_bool(self.action.is_some())?;
        cell.append_bit_bool(self.aborted)?;
        cell.append_bit_bool(self.destroyed)?;

        if let Some(a) = &self.action {
            a.serialize()?.write_to(cell)?;
        }

        Ok(())
    }
}

impl Deserializable for TransactionDescrTickTock {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // self.tt.read_from(cell)?;
        self.storage.read_from(cell)?;
        self.compute_ph.read_from(cell)?;
        self.action = if cell.get_next_bit()? {
            Some(TrActionPhase::construct_from_cell(cell.checked_drain_reference()?)?)
        } else {
            None
        };
        self.aborted = cell.get_next_bit()?;
        self.destroyed = cell.get_next_bit()?;
        Ok(())
    }
}

/*
trans_split_prepare$0100
    split_info:SplitMergeInfo
    compute_ph:TrComputePhase
    action:(Maybe ^TrActionPhase)
    aborted:Boolean
    destroyed:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrSplitPrepare {
    pub split_info: SplitMergeInfo,
    pub compute_ph: TrComputePhase,
    pub action: Option<TrActionPhase>,
    pub aborted: bool,
    pub destroyed: bool,
}

impl Serializable for TransactionDescrSplitPrepare {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.split_info.write_to(cell)?;
        self.compute_ph.write_to(cell)?;
        self.action.is_some().write_to(cell)?;
        self.aborted.write_to(cell)?;
        self.destroyed.write_to(cell)?;

        if let Some(a) = &self.action {
            a.serialize()?.write_to(cell)?;
        }

        Ok(())
    }
}

impl Deserializable for TransactionDescrSplitPrepare {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.split_info.read_from(cell)?;
        self.compute_ph.read_from(cell)?;
        self.action = if cell.get_next_bit()? {
            Some(TrActionPhase::construct_from_cell(cell.checked_drain_reference()?)?)
        } else {
            None
        };
        self.aborted = cell.get_next_bit()?;
        self.destroyed = cell.get_next_bit()?;
        Ok(())
    }
}

/*
trans_split_install$0101
    split_info:SplitMergeInfo
    prepare_transaction:^Transaction
    installed:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrSplitInstall {
    pub split_info: SplitMergeInfo,
    pub prepare_transaction: Arc<Transaction>,
    pub installed: bool,
}

impl Serializable for TransactionDescrSplitInstall {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.split_info.write_to(cell)?;
        cell.append_bit_bool(self.installed)?;
        cell.checked_append_reference(self.prepare_transaction.serialize()?)?;
        Ok(())
    }
}

impl Deserializable for TransactionDescrSplitInstall {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.split_info.read_from(cell)?;
        self.installed = cell.get_next_bit()?;

        let tr = Transaction::construct_from_reference(cell)?;
        self.prepare_transaction = Arc::new(tr);

        Ok(())
    }
}

/*
trans_merge_prepare$0110
    split_info:SplitMergeInfo
    storage_ph:TrStoragePhase
    aborted:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrMergePrepare {
    pub split_info: SplitMergeInfo,
    pub storage_ph: TrStoragePhase,
    pub aborted: bool,
}

impl Serializable for TransactionDescrMergePrepare {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.split_info.write_to(cell)?;
        self.storage_ph.write_to(cell)?;
        cell.append_bit_bool(self.aborted)?;
        Ok(())
    }
}

impl Deserializable for TransactionDescrMergePrepare {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.split_info.read_from(cell)?;
        self.storage_ph.read_from(cell)?;
        self.aborted = cell.get_next_bit()?;
        Ok(())
    }
}

/*
trans_merge_install$0111
    split_info:SplitMergeInfo
    prepare_transaction:^Transaction
    credit_ph:(Maybe TrCreditPhase)
    compute_ph:TrComputePhase
    action:(Maybe ^TrActionPhase)
    aborted:Boolean
    destroyed:Boolean
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionDescrMergeInstall {
    pub split_info: SplitMergeInfo,
    pub prepare_transaction: Arc<Transaction>,
    pub credit_ph: Option<TrCreditPhase>,
    pub compute_ph: TrComputePhase,
    pub action: Option<TrActionPhase>,
    pub aborted: bool,
    pub destroyed: bool,
}

impl Serializable for TransactionDescrMergeInstall {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.split_info.write_to(cell)?;
        cell.checked_append_reference(self.prepare_transaction.serialize()?)?;
        self.credit_ph.write_to(cell)?;
        self.compute_ph.write_to(cell)?;
        self.action.is_some().write_to(cell)?;
        self.aborted.write_to(cell)?;
        self.destroyed.write_to(cell)?;

        if let Some(a) = &self.action {
            a.serialize()?.write_to(cell)?;
        }

        Ok(())
    }
}

impl Deserializable for TransactionDescrMergeInstall {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.split_info.read_from(cell)?;

        let tr = Transaction::construct_from_reference(cell)?;
        self.prepare_transaction = Arc::new(tr);

        self.credit_ph.read_from(cell)?;
        self.compute_ph.read_from(cell)?;
        self.action = if cell.get_next_bit()? {
            Some(TrActionPhase::construct_from_cell(cell.checked_drain_reference()?)?)
        } else {
            None
        };
        self.aborted = cell.get_next_bit()?;
        self.destroyed = cell.get_next_bit()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionDescr {
    Ordinary(TransactionDescrOrdinary),
    Storage(TransactionDescrStorage),
    TickTock(TransactionDescrTickTock),
    SplitPrepare(TransactionDescrSplitPrepare),
    SplitInstall(TransactionDescrSplitInstall),
    MergePrepare(TransactionDescrMergePrepare),
    MergeInstall(TransactionDescrMergeInstall),
}

impl Default for TransactionDescr {
    fn default() -> Self {
        TransactionDescr::Ordinary(TransactionDescrOrdinary::default())
    }
}

impl TransactionDescr {
    pub fn is_aborted(&self) -> bool {
        match self {
            TransactionDescr::Ordinary(ref desc) => desc.aborted,
            TransactionDescr::TickTock(ref desc) => desc.aborted,
            TransactionDescr::SplitPrepare(ref desc) => desc.aborted,
            TransactionDescr::MergePrepare(ref desc) => desc.aborted,
            TransactionDescr::MergeInstall(ref desc) => desc.aborted,
            _ => false,
        }
    }

    pub fn compute_phase_ref(&self) -> Option<&TrComputePhase> {
        match self {
            TransactionDescr::Ordinary(ref desc) => Some(&desc.compute_ph),
            TransactionDescr::TickTock(ref desc) => Some(&desc.compute_ph),
            TransactionDescr::SplitPrepare(ref desc) => Some(&desc.compute_ph),
            TransactionDescr::MergeInstall(ref desc) => Some(&desc.compute_ph),
            _ => None,
        }
    }

    pub fn action_phase_ref(&self) -> Option<&TrActionPhase> {
        match self {
            TransactionDescr::Ordinary(ref desc) => desc.action.as_ref(),
            TransactionDescr::TickTock(ref desc) => desc.action.as_ref(),
            TransactionDescr::SplitPrepare(ref desc) => desc.action.as_ref(),
            TransactionDescr::MergeInstall(ref desc) => desc.action.as_ref(),
            _ => None,
        }
    }

    pub fn bounce_phase_ref(&self) -> Option<&TrBouncePhase> {
        match &self {
            TransactionDescr::Ordinary(desc) => desc.bounce.as_ref(),
            _ => None,
        }
    }

    pub fn is_credit_first(&self) -> Option<bool> {
        match self {
            TransactionDescr::Ordinary(ref tr) => Some(tr.credit_first),
            _ => None,
        }
    }

    pub fn is_split(&self) -> bool {
        matches!(self, TransactionDescr::SplitPrepare(_) | TransactionDescr::SplitInstall(_))
    }

    pub fn is_merge(&self) -> bool {
        matches!(self, TransactionDescr::MergePrepare(_) | TransactionDescr::MergeInstall(_))
    }

    ///
    /// mark the transaction as aborted
    ///
    pub fn mark_as_aborted(&mut self) {
        match self {
            TransactionDescr::Ordinary(ref mut desc) => {
                desc.aborted = true;
            }
            TransactionDescr::TickTock(ref mut desc) => {
                desc.aborted = true;
            }
            TransactionDescr::SplitPrepare(ref mut desc) => {
                desc.aborted = true;
            }
            TransactionDescr::MergePrepare(ref mut desc) => {
                desc.aborted = true;
            }
            TransactionDescr::MergeInstall(ref mut desc) => {
                desc.aborted = true;
            }
            _ => (),
        };
    }
}

impl Serializable for TransactionDescr {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            TransactionDescr::Ordinary(o) => {
                cell.append_bits(0b0000, 4)?;
                o.write_to(cell)?;
            }
            TransactionDescr::Storage(s) => {
                cell.append_bits(0b0001, 4)?;
                s.write_to(cell)?;
            }
            TransactionDescr::TickTock(tt) => {
                cell.append_bits(0b001, 3)?;
                tt.write_to(cell)?;
            }
            TransactionDescr::SplitPrepare(sp) => {
                cell.append_bits(0b0100, 4)?;
                sp.write_to(cell)?;
            }
            TransactionDescr::SplitInstall(si) => {
                cell.append_bits(0b0101, 4)?;
                si.write_to(cell)?;
            }
            TransactionDescr::MergePrepare(mp) => {
                cell.append_bits(0b0110, 4)?;
                mp.write_to(cell)?;
            }
            TransactionDescr::MergeInstall(mi) => {
                cell.append_bits(0b0111, 4)?;
                mi.write_to(cell)?;
            }
        }
        Ok(())
    }
}

impl Deserializable for TransactionDescr {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        match cell.get_next_bits(4)?[0] {
            0b0000_0000 => {
                let mut o = TransactionDescrOrdinary::default();
                o.read_from(cell)?;
                *self = TransactionDescr::Ordinary(o);
            }
            0b0001_0000 => {
                let mut s = TransactionDescrStorage::default();
                s.read_from(cell)?;
                *self = TransactionDescr::Storage(s);
            }
            0b0010_0000 => {
                let mut tt = TransactionDescrTickTock::tick();
                tt.read_from(cell)?;
                *self = TransactionDescr::TickTock(tt);
            }
            0b0011_0000 => {
                let mut tt = TransactionDescrTickTock::tock();
                tt.read_from(cell)?;
                *self = TransactionDescr::TickTock(tt);
            }
            0b0100_0000 => {
                let mut sp = TransactionDescrSplitPrepare::default();
                sp.read_from(cell)?;
                *self = TransactionDescr::SplitPrepare(sp);
            }
            0b0101_0000 => {
                let mut si = TransactionDescrSplitInstall::default();
                si.read_from(cell)?;
                *self = TransactionDescr::SplitInstall(si);
            }
            0b0110_0000 => {
                let mut mp = TransactionDescrMergePrepare::default();
                mp.read_from(cell)?;
                *self = TransactionDescr::MergePrepare(mp);
            }
            0b0111_0000 => {
                let mut mi = TransactionDescrMergeInstall::default();
                mi.read_from(cell)?;
                *self = TransactionDescr::MergeInstall(mi);
            }
            tag => fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: "TransactionDescr".to_string()
            }),
        }
        Ok(())
    }
}

/*
update_hashes#72 {X:Type} old_hash:bits256 new_hash:bits256
  = HASH_UPDATE X;
*/
const HASH_UPDATE_TAG: u8 = 0x72;
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct HashUpdate {
    pub old_hash: UInt256,
    pub new_hash: UInt256,
}

impl HashUpdate {
    // Creates new instance of HashUpdate with given hashes
    pub fn with_hashes(old_hash: UInt256, new_hash: UInt256) -> Self {
        HashUpdate { old_hash, new_hash }
    }
}

impl Serializable for HashUpdate {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(HASH_UPDATE_TAG)?;
        self.old_hash.write_to(cell)?;
        self.new_hash.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for HashUpdate {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != HASH_UPDATE_TAG {
            fail!(BlockError::InvalidConstructorTag { t: tag as u32, s: "HashUpdate".to_string() })
        }
        self.old_hash.read_from(cell)?;
        self.new_hash.read_from(cell)?;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct UInt15(pub i16);

impl UInt15 {
    pub fn from_lt(lt: u64) -> Self {
        Self(lt as i16)
    }
}

impl Serializable for UInt15 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(self.0 as usize, 15)?;
        Ok(())
    }
}

impl Deserializable for UInt15 {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.0 = slice.get_next_int(15)? as i16;
        Ok(())
    }
}

define_HashmapE! {OutMessages, 15, InRefValue<Message>}

pub type TransactionId = UInt256;

/*
transaction$0111
    account_addr:bits256
    lt:uint64
    prev_trans_hash:bits256
    prev_trans_lt:uint64
    now:uint32
    outmsg_cnt:uint15
    orig_status:AccountStatus
    end_status:AccountStatus
    ^[  in_msg:(Maybe ^(Message Any))
        out_msgs:(HashmapE 15 ^(Message Any)) ]
    total_fees:CurrencyCollection
    state_update:^(HASH_UPDATE Account)
    description:^TransactionDescr
= Transaction;
*/
#[derive(Debug, Clone)]
pub struct Transaction {
    account_addr: AccountId,
    lt: u64,
    prev_trans_hash: UInt256,
    prev_trans_lt: u64,
    now: u32,
    outmsg_cnt: i16,
    pub orig_status: AccountStatus,
    pub end_status: AccountStatus,
    pub in_msg: ChildCell<Message>,
    pub out_msgs: OutMessages,
    total_fees: CurrencyCollection,
    state_update: ChildCell<HashUpdate>,
    description: ChildCell<TransactionDescr>,
}

impl Transaction {
    /// create new transaction
    /// TODO Refactor constructors - use one for old msg format and one for new
    pub fn with_address_and_status(account_addr: AccountId, orig_status: AccountStatus) -> Self {
        Transaction {
            account_addr,
            lt: 0,
            prev_trans_hash: UInt256::default(),
            prev_trans_lt: 0,
            now: 0,
            outmsg_cnt: 0,
            orig_status,
            end_status: AccountStatus::AccStateActive,
            in_msg: ChildCell::default(),
            out_msgs: OutMessages::default(),
            total_fees: CurrencyCollection::default(),
            state_update: ChildCell::default(),
            description: ChildCell::default(),
        }
    }

    pub fn with_account_and_message(account: &Account, msg: &Message, lt: u64) -> Result<Self> {
        let account_addr = account
            .get_id()
            .or_else(|| msg.int_dst_account_id())
            .ok_or_else(|| error!("cannot resolve destination address of message"))?
            .clone();
        Ok(Transaction {
            account_addr,
            lt,
            prev_trans_hash: UInt256::default(),
            prev_trans_lt: 0,
            now: 0,
            outmsg_cnt: 0,
            orig_status: account.status(),
            end_status: account.status(),
            in_msg: ChildCell::with_struct(msg)?,
            out_msgs: OutMessages::default(),
            total_fees: CurrencyCollection::default(),
            state_update: ChildCell::default(),
            description: ChildCell::default(),
        })
    }

    /// Get account address of transaction
    pub fn account_id(&self) -> &AccountId {
        &self.account_addr
    }

    /// set transaction time
    pub fn set_logical_time(&mut self, lt: u64) {
        self.lt = lt;
    }

    /// get transaction logical time
    pub fn logical_time(&self) -> u64 {
        self.lt
    }

    /// get hash of previous transaction
    pub fn prev_trans_hash(&self) -> &UInt256 {
        &self.prev_trans_hash
    }

    pub fn set_prev_trans_hash(&mut self, hash: UInt256) {
        self.prev_trans_hash = hash
    }

    /// get logical time of previous transaction
    pub fn prev_trans_lt(&self) -> u64 {
        self.prev_trans_lt
    }

    pub fn set_prev_trans_lt(&mut self, lt: u64) {
        self.prev_trans_lt = lt
    }

    /// set end status accaunt
    pub fn set_end_status(&mut self, end_status: AccountStatus) {
        self.end_status = end_status;
    }

    /// add fee
    pub fn add_fee_coins(&mut self, fee: &Coins) -> Result<bool> {
        crate::types::AddSub::add(&mut self.total_fees.coins, fee)
    }

    /// set total fees
    pub fn set_total_fees(&mut self, fees: CurrencyCollection) {
        self.total_fees = fees;
    }

    /// get total fees
    pub fn total_fees(&self) -> &CurrencyCollection {
        &self.total_fees
    }

    /// get total fees mutable
    pub fn total_fees_mut(&mut self) -> &mut CurrencyCollection {
        &mut self.total_fees
    }

    pub fn read_in_msg(&self) -> Result<Option<Message>> {
        match self.in_msg.is_empty() {
            true => Ok(None),
            false => self.in_msg.read_struct().map(Some),
        }
    }

    pub fn write_in_msg(&mut self, value: Option<&Message>) -> Result<()> {
        match value {
            Some(m) => self.in_msg.write_struct(m),
            None => {
                self.in_msg = ChildCell::default();
                Ok(())
            }
        }
    }

    pub fn set_in_msg_cell(&mut self, msg_cell: Cell) {
        self.in_msg.set_cell(msg_cell);
    }

    pub fn in_msg_cell(&self) -> Option<Cell> {
        match self.in_msg.is_empty() {
            true => None,
            false => Some(self.in_msg.cell()),
        }
    }

    /// get output message by index
    pub fn get_out_msg(&self, index: i16) -> Result<Option<Message>> {
        Ok(self.out_msgs.get(&UInt15(index))?.map(|InRefValue(msg)| msg))
    }

    /// iterate output messages
    pub fn iterate_out_msgs<F>(&self, mut f: F) -> Result<bool>
    where
        F: FnMut(Message) -> Result<bool>,
    {
        self.out_msgs.iterate(|InRefValue(msg)| f(msg))
    }

    /// add output message to Hashmap
    pub fn add_out_message(&mut self, msg: &Message) -> Result<()> {
        self.out_msgs.setref(&UInt15(self.outmsg_cnt), &msg.serialize()?)?;
        self.outmsg_cnt += 1;
        Ok(())
    }

    pub fn read_state_update(&self) -> Result<HashUpdate> {
        self.state_update.read_struct()
    }

    pub fn write_state_update(&mut self, value: &HashUpdate) -> Result<()> {
        self.state_update.write_struct(value)
    }

    pub fn state_update_cell(&self) -> Cell {
        self.state_update.cell()
    }

    pub fn read_description(&self) -> Result<TransactionDescr> {
        self.description.read_struct()
    }

    pub fn write_description(&mut self, value: &TransactionDescr) -> Result<()> {
        self.description.write_struct(value)
    }

    pub fn description_cell(&self) -> Cell {
        self.description.cell()
    }

    pub fn msg_count(&self) -> i16 {
        self.outmsg_cnt
    }

    /// return now time
    pub fn now(&self) -> u32 {
        self.now
    }

    /// set now time
    pub fn set_now(&mut self, now: u32) {
        self.now = now;
    }

    pub fn prepare_proof(&self, block_root: &Cell) -> Result<Cell> {
        // proof for transaction and block info in block

        let usage_tree = UsageTree::with_root(block_root.clone());
        let block = Block::construct_from_cell(usage_tree.root_cell())?;

        block.read_info()?;

        block
            .read_extra()?
            .read_account_blocks()?
            .get_serialized(self.account_id().clone())?
            .ok_or_else(|| {
                BlockError::InvalidArg(
                    "Transaction doesn't belong to given block \
                     (can't find account block)"
                        .to_string(),
                )
            })?
            .transactions()
            .get(&self.logical_time())?
            .ok_or_else(|| {
                BlockError::InvalidArg("Transaction doesn't belong to given block".to_string())
            })?;

        MerkleProof::create_by_usage_tree(block_root, &usage_tree)?.serialize()
    }

    pub fn contains_out_msg(&self, created_lt: u64, hash: &UInt256) -> bool {
        if (created_lt > self.lt) && (created_lt <= self.lt + self.outmsg_cnt as u64) {
            let key = UInt15::from_lt(created_lt - self.lt - 1);
            if let Ok(Some(cell)) = self.out_msgs.get_ref(&key) {
                return &cell.repr_hash() == hash;
            }
        }
        false
    }

    pub fn gas_used(&self) -> Option<u64> {
        if let Ok(description) = self.read_description() {
            if let Some(TrComputePhase::Vm(compute_ph)) = description.compute_phase_ref() {
                return Some(compute_ph.gas_used.as_u64());
            }
        }
        None
    }

    // TODO remove after VM alignment
    pub fn set_gas_used(&mut self, new_gas: u32) -> Result<()> {
        let mut description = self.read_description()?;
        let compute_ph = match &mut description {
            TransactionDescr::Ordinary(desc) => &mut desc.compute_ph,
            TransactionDescr::TickTock(desc) => &mut desc.compute_ph,
            TransactionDescr::SplitPrepare(desc) => &mut desc.compute_ph,
            TransactionDescr::MergeInstall(desc) => &mut desc.compute_ph,
            _ => fail!("Transaction has compute phase skipped"),
        };
        if let Some(vm) = compute_ph.get_vmphase_mut() {
            vm.gas_used = new_gas.into();
            self.write_description(&description)
        } else {
            fail!("Transaction has compute phase skipped")
        }
    }
}

impl PartialEq for Transaction {
    fn eq(&self, other: &Transaction) -> bool {
        self.account_addr == other.account_addr
            && self.lt == other.lt
            && self.prev_trans_hash == other.prev_trans_hash
            && self.prev_trans_lt == other.prev_trans_lt
            && self.now == other.now
            && self.outmsg_cnt == other.outmsg_cnt
            && self.orig_status == other.orig_status
            && self.end_status == other.end_status
            && self.in_msg == other.in_msg
            && self.out_msgs == other.out_msgs
            && self.total_fees == other.total_fees
            && self.state_update == other.state_update
            && self.description == other.description
    }
}

impl Eq for Transaction {}

impl Default for Transaction {
    fn default() -> Self {
        Transaction {
            account_addr: AccountId::from([0; 32]),
            lt: 0,
            prev_trans_hash: UInt256::default(),
            prev_trans_lt: 0,
            now: 0,
            outmsg_cnt: 0,
            orig_status: AccountStatus::AccStateUninit,
            end_status: AccountStatus::AccStateUninit,
            in_msg: ChildCell::default(),
            out_msgs: OutMessages::default(),
            total_fees: CurrencyCollection::default(),
            state_update: ChildCell::default(),
            description: ChildCell::default(),
        }
    }
}
const TRANSACTION_TAG: usize = 0x7;

impl Serializable for Transaction {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        let tag = TRANSACTION_TAG;
        builder.append_bits(tag, 4)?;
        self.account_addr.write_to(builder)?; // account_addr: AccountId,
        builder.append_u64(self.lt)?; // lt: u64,
        self.prev_trans_hash.write_to(builder)?;
        self.prev_trans_lt.write_to(builder)?;
        self.now.write_to(builder)?;
        builder.append_bits(self.outmsg_cnt as usize, 15)?; // outmsg_cnt: u15
        self.orig_status.write_to(builder)?; // orig_status: AccountStatus,
        self.end_status.write_to(builder)?; // end_status: AccountStatus
        let mut builder1 = BuilderData::new();
        if self.in_msg.is_empty() {
            builder1.append_bit_zero()?;
        } else {
            builder1.append_bit_one()?;
            builder1.checked_append_reference(self.in_msg.cell())?;
        };
        self.out_msgs.write_to(&mut builder1)?;
        builder.checked_append_reference(builder1.into_cell()?)?;
        self.total_fees.write_to(builder)?; // total_fees
        self.state_update.write_to(builder)?; // ^(HASH_UPDATE Account)
        self.description.write_to(builder)?; // ^TransactionDescr
        Ok(())
    }
}

impl Deserializable for Transaction {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_int(4)? as usize;
        if tag != TRANSACTION_TAG {
            fail!(BlockError::InvalidConstructorTag { t: tag as u32, s: "Transaction".to_string() })
        }
        self.account_addr.read_from(cell)?; // account_addr
        self.lt = cell.get_next_u64()?; // lt
        self.prev_trans_hash.read_from(cell)?;
        self.prev_trans_lt.read_from(cell)?;
        self.now.read_from(cell)?;
        self.outmsg_cnt = cell.get_next_int(15)? as i16; // outmsg_cnt
        self.orig_status.read_from(cell)?; // orig_status
        self.end_status.read_from(cell)?; // end_status
        let cell1 = &mut SliceData::load_cell(cell.checked_drain_reference()?)?;
        if cell1.get_next_bit()? {
            self.in_msg.read_from(cell1)?;
        } else {
            self.in_msg = ChildCell::default();
        }
        self.out_msgs = OutMessages::construct_from(cell1)?;
        self.total_fees.read_from(cell)?; // total_fees
        self.state_update.read_from(cell)?; // ^(HASH_UPDATE Account)
        self.description.read_from(cell)?; // ^TransactionDescr

        Ok(())
    }
}

define_HashmapAugE!(Transactions, 64, u64, InRefValue<Transaction>, CurrencyCollection);

impl Transactions {
    pub fn insert(&mut self, tr: &Transaction) -> Result<()> {
        let cell = tr.serialize()?;
        let lt = tr.logical_time();
        let total_fees = tr.total_fees();
        self.setref(&lt, cell, total_fees)
    }
    pub fn total_fees(&self) -> &CurrencyCollection {
        self.root_extra()
    }
}

impl Augmentation<CurrencyCollection> for Transaction {
    fn aug(&self) -> Result<CurrencyCollection> {
        Ok(self.total_fees.clone())
    }
}

impl Augmentation<CurrencyCollection> for InRefValue<Transaction> {
    fn aug(&self) -> Result<CurrencyCollection> {
        Ok(self.total_fees.clone())
    }
}

/// 4.2.15. Collection of all transactions of an account.
/// From Lite Client v11:
/// acc_trans#5 account_addr:bits256
///      transactions:(HashmapAug 64 ^Transaction CurrencyCollection)
///      state_update:^(HASH_UPDATE Account)
/// = AccountBlock;
#[derive(Clone, Debug, Default, Eq)]
pub struct AccountBlock {
    account_addr: AccountId,
    transactions: Transactions, // HashmapAug 64 ^Transaction CurrencyCollection
    state_update: ChildCell<HashUpdate>, // ^(HASH_UPDATE Account)
}

impl PartialEq for AccountBlock {
    fn eq(&self, other: &AccountBlock) -> bool {
        self.account_addr.eq(&other.account_addr)
            && self.transactions.eq(&other.transactions)
            && self.state_update.eq(&other.state_update)
    }
}

impl AccountBlock {
    pub fn with_address(account_addr: AccountId) -> AccountBlock {
        AccountBlock {
            account_addr,
            transactions: Transactions::default(),
            state_update: ChildCell::default(),
        }
    }

    pub fn with_transaction(
        account_addr: AccountId,
        transaction: &Transaction,
    ) -> Result<AccountBlock> {
        let mut transactions = Transactions::default();
        transactions.setref(
            &transaction.logical_time(),
            transaction.serialize()?,
            transaction.total_fees(),
        )?;
        Ok(AccountBlock {
            account_addr,
            transactions,
            state_update: transaction.state_update.clone(),
        })
    }

    pub fn with_params(
        account_addr: &AccountId,
        transactions: &Transactions,
        state_update: &HashUpdate,
    ) -> Result<Self> {
        Ok(Self {
            account_addr: account_addr.clone(),
            transactions: transactions.clone(),
            state_update: ChildCell::with_struct(state_update)?,
        })
    }

    /// add transaction to block
    pub fn add_transaction(&mut self, transaction: &Transaction) -> Result<()> {
        self.add_serialized_transaction(transaction, transaction.serialize()?)
    }

    /// append serialized transaction to block (use to increase speed)
    pub fn add_serialized_transaction(
        &mut self,
        transaction: &Transaction,
        transaction_cell: Cell,
    ) -> Result<()> {
        self.transactions.setref(
            &transaction.logical_time(),
            transaction_cell,
            transaction.total_fees(),
        )?;
        Ok(())
    }

    /// get hash update for Account
    pub fn read_state_update(&self) -> Result<HashUpdate> {
        self.state_update.read_struct()
    }

    /// set hash update for Account
    pub fn write_state_update(&mut self, state_update: &HashUpdate) -> Result<()> {
        self.state_update.write_struct(state_update)
    }

    // get Block AccountId
    pub fn account_id(&self) -> &AccountId {
        &self.account_addr
    }

    // get Block AccountId as SliceData
    pub fn account_addr(&self) -> SliceData {
        self.account_addr.clone()
    }

    /// get sum of all acoount's transactions
    pub fn total_fee(&self) -> &CurrencyCollection {
        self.transactions.root_extra()
    }

    /// update
    pub fn calculate_and_write_state(
        &mut self,
        old_state: &ShardStateUnsplit,
        new_state: &ShardStateUnsplit,
    ) -> Result<()> {
        if self.transactions.is_empty() {
            fail!(BlockError::InvalidData("No transactions in account block".to_string()))
        } else if let Some(transaction) = self.transactions.single_value()? {
            // if block has only one transaction for account just copy state update from transaction
            self.write_state_update(&transaction.0.read_state_update()?)?;
        } else {
            // otherwice it is need to calculate Hash update
            let old_hash = old_state
                .read_accounts()?
                .get_serialized(self.account_addr.clone())?
                .ok_or_else(|| {
                    BlockError::Other(format!(
                        "Account should be in old shard state {:x}",
                        self.account_addr
                    ))
                })?
                .account_cell()
                .repr_hash();
            let new_hash = new_state
                .read_accounts()?
                .get_serialized(self.account_addr.clone())?
                .ok_or_else(|| {
                    BlockError::Other(format!(
                        "Account should be in new shard state {:x}",
                        self.account_addr
                    ))
                })?
                .account_cell()
                .repr_hash();
            self.write_state_update(&HashUpdate::with_hashes(old_hash, new_hash))?;
        }
        Ok(())
    }

    pub fn transaction_iterate<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(Transaction) -> Result<bool>,
    {
        self.transactions.iterate_objects(|InRefValue(transaction)| p(transaction))
    }

    pub fn transaction_iterate_full<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(u64, Cell, CurrencyCollection) -> Result<bool>,
    {
        self.transactions.iterate_slices_with_keys_and_aug(|key, transaction, aug| {
            p(key, transaction.reference(0)?, aug)
        })
    }

    pub fn transactions(&self) -> &Transactions {
        &self.transactions
    }

    pub fn transaction(&self, lt: u64) -> Result<Option<Transaction>> {
        Ok(self.transactions.get(&lt)?.map(|InRefValue(tr)| tr))
    }
}

const ACCOUNT_BLOCK_TAG: usize = 0x5;

impl Serializable for AccountBlock {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let tag = ACCOUNT_BLOCK_TAG;

        cell.append_bits(tag, 4)?;
        self.account_addr.write_to(cell)?;
        self.transactions.write_hashmap_root(cell)?;
        self.state_update.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for AccountBlock {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(4)? as usize;
        if tag != ACCOUNT_BLOCK_TAG {
            fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: std::any::type_name::<Self>().to_string()
            })
        }
        self.account_addr.read_from(slice)?;

        let mut trs = Transactions::default();
        trs.read_hashmap_root(slice)?;
        self.transactions = trs;

        self.state_update.read_from(slice)?;

        Ok(())
    }
}

/////////////////////////////////////////////////////////////////////////////////////////
// 4.2.17. Collection of all transactions in a block.
// _ (HashmapAugE 256 AccountBlock CurrencyCollection) = ShardAccountBlocks;
define_HashmapAugE!(ShardAccountBlocks, 256, AccountId, AccountBlock, CurrencyCollection);

impl Augmentation<CurrencyCollection> for AccountBlock {
    fn aug(&self) -> Result<CurrencyCollection> {
        Ok(self.transactions.total_fees().clone())
    }
}

/// external interface for ShardAccountBlock
impl ShardAccountBlocks {
    /// insert new AccountBlock or replace existing
    // TODO: will be removed when acc_id as slice and set as type
    pub fn insert(&mut self, account_block: &AccountBlock) -> Result<()> {
        self.set_builder_serialized(
            account_block.account_addr.clone(),
            &account_block.write_to_new_cell()?,
            account_block.total_fee(),
        )?;
        Ok(())
    }

    /// adds transaction to account by id from transaction
    pub fn add_transaction(&mut self, transaction: &Transaction) -> Result<()> {
        self.add_serialized_transaction(transaction, transaction.serialize()?)
    }

    pub fn add_serialized_transaction(
        &mut self,
        transaction: &Transaction,
        transaction_cell: Cell,
    ) -> Result<()> {
        let account_id = transaction.account_id();
        // get AccountBlock for accountId, if not exist, create it
        let mut account_block;
        match self.get_serialized(account_id.clone())? {
            Some(acc) => {
                account_block = acc;
                let mut account_state_update = account_block.read_state_update()?;
                let state_update = transaction.read_state_update()?;
                if account_state_update.new_hash != state_update.old_hash {
                    fail!(
                        "hash {:x} is not {:x} of next transaction {}",
                        state_update.old_hash,
                        account_state_update.new_hash,
                        transaction.logical_time()
                    )
                }
                account_state_update.new_hash = state_update.new_hash;
                account_block.write_state_update(&account_state_update)?;
                account_block.add_serialized_transaction(transaction, transaction_cell)?;
            }
            None => {
                account_block = AccountBlock::with_transaction(account_id.clone(), transaction)?
            }
        };
        self.set_builder_serialized(
            account_id.clone(),
            &account_block.write_to_new_cell()?,
            &account_block.aug()?,
        )?;
        Ok(())
    }

    pub fn full_transaction_fees(&self) -> &CurrencyCollection {
        self.root_extra()
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Copy)]
pub enum TransactionProcessingStatus {
    #[default]
    Unknown = 0,
    Preliminary,
    Proposed,
    Finalized,
    Refused,
}
