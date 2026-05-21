/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![allow(clippy::too_many_arguments)]

use crate::{blockchain_config::BlockchainConfig, error::ExecutorError};
use std::{
    collections::HashMap,
    convert::TryInto,
    sync::{Arc, LazyLock},
};
use ton_block::{
    error, fail, time_checker, unpack_out_action_slices, AccStatusChange, Account, AccountId,
    AccountStatus, AddSub, BlockError, BouncedByPhase, Cell, ChildCell, Coins, ComputeSkipReason,
    CurrencyCollection, Deserializable, ExceptionCode, GasLimitsPrices, GetRepresentationHash,
    GlobalCapabilities, HashmapE, HashmapFilterResult, IBitstring, Mask, Message, MsgAddressInt,
    NewBounceBody, NewBounceComputePhaseInfo, NewBounceOriginalInfo, OutAction, Result,
    Serializable, SimpleLib, SizeLimitsConfig, SliceData, StateInit, StorageUsageCalc,
    TrActionPhase, TrBouncePhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase, TrStoragePhase,
    Transaction, UInt256, VarUInteger16, VarUInteger3, VarUInteger32, WorkchainFormat,
    BASE_WORKCHAIN_ID, CHANGE_SET_LIB_BOUNCE_IF_FAIL, MASTERCHAIN_ID, MAX_MERKLE_DEPTH,
    MAX_MSG_MERKLE_DEPTH, RESERVE_ALL_BUT, RESERVE_BOUNCE_IF_FAIL, RESERVE_IGNORE_ERROR,
    RESERVE_PLUS_ORIG, RESERVE_REVERSE, RESERVE_VALID_MODES, SENDMSG_ALL_BALANCE,
    SENDMSG_BOUNCE_IF_FAIL, SENDMSG_DELETE_IF_EMPTY, SENDMSG_IGNORE_ERROR,
    SENDMSG_PAY_FEE_SEPARATELY, SENDMSG_REMAINING_MSG_BALANCE, SENDMSG_VALID_FLAGS,
    SET_LIB_CODE_ADD_PUBLIC,
};
use ton_vm::{
    error::tvm_exception_full,
    executor::{gas::gas_state::Gas, BehaviorModifiers, Engine, TraceCallback},
    smart_contract_info::{PrevBlocksInfo, SmartContractInfo},
    stack::{savelist::SaveList, Stack, StackItem},
};

pub const RESULT_CODE_SKIPPED: i32 = 1;
pub const RESULT_CODE_ACTIONLIST_INVALID: i32 = 32;
pub const RESULT_CODE_TOO_MANY_ACTIONS: i32 = 33;
pub const RESULT_CODE_UNKNOWN_OR_INVALID_ACTION: i32 = 34;
pub const RESULT_CODE_INCORRECT_SRC_ADDRESS: i32 = 35;
pub const RESULT_CODE_INCORRECT_DST_ADDRESS: i32 = 36;
pub const RESULT_CODE_NOT_ENOUGH_COINS: i32 = 37;
pub const RESULT_CODE_NOT_ENOUGH_EXTRA: i32 = 38;
// pub const RESULT_CODE_CANNOT_PACK: i32 = 39;
pub const RESULT_CODE_INVALID_BALANCE: i32 = 40;
pub const RESULT_CODE_BAD_ACCOUNT_STATE: i32 = 41;
pub const RESULT_CODE_NON_ZERO_CELL_LEVEL: i32 = 42;
pub const RESULT_CODE_TOO_MANY_EXTRA: i32 = 44;
/// Result code indicating that invalid or unsupported extra flags were provided in a message or transaction.
pub const RESULT_CODE_INVALID_EXTRA_FLAGS: i32 = 45;
pub const RESULT_CODE_EXCEEDED_LIMITS: i32 = 50;
pub const RESULT_CODE_UNSUPPORTED: i32 = -1;

/// Extra flag indicating that the new bounce message format should be used.
pub const EXTRA_FLAG_NEW_BOUNCE_FORMAT: u8 = 0x1;
/// Extra flag indicating that the full message body should be included in the bounce.
pub const EXTRA_FLAG_FULL_BODY_BOUNCE: u8 = 0x2;
/// Bitmask of all supported extra flags for message processing (combines all valid extra flags).
pub const SUPPORTED_EXTRA_FLAGS: u8 = EXTRA_FLAG_NEW_BOUNCE_FORMAT | EXTRA_FLAG_FULL_BODY_BOUNCE;

const RESULT_CODE_LIB_BAD_CELL: i32 = 41;
const RESULT_CODE_LIB_BAD_ACCOUNT_STATE: i32 = 42;
const RESULT_CODE_LIB_EXCEEDED_LIMITS: i32 = 43;

pub(crate) const MAX_ACTIONS: usize = 255;

static SPECIAL_LIMIT_ACCOUNTS: LazyLock<HashMap<MsgAddressInt, (u32, u64)>> = LazyLock::new(|| {
    let limits = [
        ("UQBeSl-dumOHieZ3DJkNKVkjeso7wZ0VpzR4LCbLGTQ8xr57", 1740787200, 70_000_000),
        ("EQC3VcQ-43klww9UfimR58TBjBzk7GPupXQ3CNuthoNp-uTR", 1740787200, 70_000_000),
        ("EQBhwBb8jvokGvfreHRRoeVxI237PrOJgyrsAhLA-4rBC_H5", 1740787200, 70_000_000),
        ("EQCkoRp4OE-SFUoMEnYfL3vF43T3AzNfW8jyTC4yzk8cJqMS", 1740787200, 70_000_000),
        ("UQBN5ICras79U8FYEm71ws34n-ZNIQ0LRNpckOUsIV3OebnC", 1740787200, 70_000_000),
        ("EQBDanbCeUqI4_v-xrnAN0_I2wRvEIaLg1Qg2ZN5c6Zl1KOh", 1740787200, 225_000_000),
    ];
    limits
        .into_iter()
        .map(|(addr, until, new_limit)| (addr.parse().unwrap(), (until, new_limit)))
        .collect()
});

#[derive(Eq, PartialEq, Debug)]
pub enum IncorrectCheckRewrite {
    Anycast,
    WrongWorkchain,
    HandWriteCheck,
    Other,
}

#[cfg(test)]
#[path = "tests/test_tr_phases.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/test_transaction_executor_with_real_data.rs"]
mod tests_with_real_data;

#[derive(Clone, Default)]
pub struct ExecuteParams {
    pub state_libs: HashmapE,
    pub block_unixtime: u32,
    pub block_lt: u64,
    pub last_tr_lt: u64,
    pub seed_block: UInt256,
    pub debug: bool,
    pub trace_callback: Option<Arc<TraceCallback>>,
    pub behavior_modifiers: Option<BehaviorModifiers>,
    pub prev_blocks_info: PrevBlocksInfo,
}

pub struct ActionPhaseResult {
    pub phase: TrActionPhase,
    pub messages: Vec<Message>,
    pub bounce: bool,
}

impl ActionPhaseResult {
    fn new(phase: TrActionPhase, messages: Vec<Message>, bounce: bool) -> ActionPhaseResult {
        ActionPhaseResult { phase, messages, bounce }
    }

    fn from_phase(phase: TrActionPhase, bounce: bool) -> ActionPhaseResult {
        ActionPhaseResult { phase, messages: vec![], bounce }
    }
}

pub trait TransactionExecutor {
    fn execute_with_params(
        &self,
        in_msg_cell: Option<Cell>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction>;

    fn ordinary_transaction(&self) -> bool;
    fn config(&self) -> &BlockchainConfig;

    fn build_stack(&self, in_msg: Option<&Message>, account: &Account) -> Result<Stack>;

    /// Implementation of transaction's storage phase.
    /// If account does not exist - phase skipped.
    /// Calculates storage fees and substracts them from account balance.
    /// If account balance becomes negative after that, then account is frozen.
    /// is_special - flag indicating that account is in list of special smart contracts, for which storage fees are not applied
    fn storage_phase(
        &self,
        acc: &mut Account,
        acc_balance: &mut CurrencyCollection,
        tr: &mut Transaction,
        is_masterchain: bool,
        is_special: bool,
    ) -> Result<TrStoragePhase> {
        log::debug!(target: "executor", "storage_phase");
        if tr.now() < acc.last_paid() {
            fail!("transaction timestamp must be greater then account timestamp")
        }
        let original_due_payment = acc.due_payment().cloned();
        let mut fee = match acc.storage_info() {
            Some(storage_info) if !is_special => {
                self.config().calc_storage_fees(storage_info, is_masterchain, tr.now())?
            }
            _ => Default::default(),
        };
        if let Some(due_payment) = acc.due_payment() {
            fee.add(due_payment)?;
            acc.set_due_payment(None);
        }

        if acc_balance.coins >= fee {
            log::debug!(target: "executor", "acc_balance: {}, storage fee: {}", acc_balance.coins, fee);
            acc_balance.coins.sub(&fee)?;
            tr.add_fee_coins(&fee)?;
            Ok(TrStoragePhase::with_params(fee, None, AccStatusChange::Unchanged))
        } else {
            log::debug!(target: "executor", "acc_balance: {} is storage fee from total: {}", acc_balance.coins, fee);
            let storage_fees_collected = std::mem::take(&mut acc_balance.coins);
            tr.add_fee_coins(&storage_fees_collected)?;
            fee.sub(&storage_fees_collected)?;
            if is_special {
                log::debug!(target: "executor", "special account, due payment {fee} still active");
                acc.set_due_payment(original_due_payment);
                return Ok(TrStoragePhase::with_params(
                    storage_fees_collected,
                    Some(fee),
                    AccStatusChange::Unchanged,
                ));
            }
            let need_freeze = acc.is_active()
                && fee > self.config().get_gas_config(is_masterchain).freeze_due_limit;
            let need_delete = (acc.is_uninit() || acc.is_frozen())
                && acc_balance.other.is_empty()
                && fee > self.config().get_gas_config(is_masterchain).delete_due_limit;
            if need_delete {
                log::debug!(target: "executor", "due payment {} have to delete", fee);
                tr.total_fees_mut().add(acc_balance)?;
                *acc = Account::default();
                acc_balance.other = Default::default();
                Ok(TrStoragePhase::with_params(
                    storage_fees_collected,
                    Some(fee),
                    AccStatusChange::Deleted,
                ))
            } else if need_freeze {
                log::debug!(target: "executor", "due payment {} have to freeze", fee);
                acc.set_due_payment(Some(fee));
                if acc.status() == AccountStatus::AccStateActive {
                    acc.try_freeze()?;
                    Ok(TrStoragePhase::with_params(
                        storage_fees_collected,
                        Some(fee),
                        AccStatusChange::Frozen,
                    ))
                } else {
                    Ok(TrStoragePhase::with_params(
                        storage_fees_collected,
                        Some(fee),
                        AccStatusChange::Unchanged,
                    ))
                }
            } else {
                log::debug!(target: "executor", "due payment {} still active", fee);
                acc.set_due_payment(Some(fee));
                Ok(TrStoragePhase::with_params(
                    storage_fees_collected,
                    Some(fee),
                    AccStatusChange::Unchanged,
                ))
            }
        }
    }

    /// Implementation of transaction's credit phase.
    /// Increases account balance by the amount that appears in the internal message header.
    /// If account does not exist - phase skipped.
    /// If message is not internal - phase skipped.
    fn credit_phase(
        &self,
        msg_balance: &CurrencyCollection,
        acc_balance: &mut CurrencyCollection,
    ) -> Result<TrCreditPhase> {
        log::debug!(
            target: "executor",
            "credit_phase: add funds {} to {}",
            msg_balance.coins, acc_balance.coins
        );
        acc_balance.add(msg_balance)?;
        Ok(TrCreditPhase::new(msg_balance.clone()))
    }

    /// Implementation of transaction's computing phase.
    /// Evaluates new accout state and invokes TVM if account has contract code.
    fn compute_phase(
        &self,
        msg: Option<&Message>,
        acc: &mut Account,
        acc_balance: &mut CurrencyCollection,
        msg_balance: &CurrencyCollection,
        mut smc_info: SmartContractInfo,
        stack: Stack,
        is_masterchain: bool,
        is_special: bool,
        was_deleted_or_frozen: bool,
        params: &ExecuteParams,
    ) -> Result<(TrComputePhase, Option<Cell>, Option<Cell>)> {
        log::debug!(target: "executor", "acc balance: {}", acc_balance.coins);
        log::debug!(target: "executor", "msg balance: {}", msg_balance.coins);
        let is_ordinary = self.ordinary_transaction();
        if acc_balance.coins.is_zero() {
            log::debug!(target: "executor", "skip computing phase no gas");
            return Ok((TrComputePhase::skipped(ComputeSkipReason::NoGas), None, None));
        }
        let mut vm_phase = TrComputePhaseVm::default();
        let is_external = if let Some(msg) = msg {
            if let Some(int_header) = msg.int_header() {
                log::debug!(target: "executor", "msg internal, bounce: {}", int_header.bounce);
                if acc.is_none() && was_deleted_or_frozen {
                    log::debug!(target: "executor", "account was deleted, skip computing phase");
                    let reason = if msg.state_init().is_none() {
                        ComputeSkipReason::NoState
                    } else {
                        ComputeSkipReason::BadState
                    };
                    return Ok((TrComputePhase::skipped(reason), None, None));
                }
                false
            } else {
                log::debug!(target: "executor", "msg external");
                if let Some(state_init) = msg.state_init() {
                    if let Some(addr) = acc.get_addr() {
                        if msg.is_inbound_external()
                            && !addr
                                .address()
                                .contains_bytes(state_init.serialize()?.repr_hash().as_slice())
                        {
                            log::error!(target: "executor", "in_msg_state hash mismatch in external message");
                            return Ok((
                                TrComputePhase::skipped(ComputeSkipReason::BadState),
                                None,
                                None,
                            ));
                        }
                    }
                }
                true
            }
        } else {
            debug_assert!(!acc.is_none());
            false
        };
        let gas_config = self.config().get_gas_config(is_masterchain);
        let mut gas = init_gas(
            acc,
            params.block_unixtime,
            acc_balance.coins.as_u128(),
            msg_balance.coins.as_u128(),
            is_external,
            is_special,
            is_ordinary,
            gas_config,
        );
        if gas.get_gas_limit() == 0 && gas.get_gas_credit() == 0 {
            log::debug!(target: "executor", "skip computing phase no gas");
            return Ok((TrComputePhase::skipped(ComputeSkipReason::NoGas), None, None));
        }

        let mut libraries = vec![];
        let mut result_acc = acc.clone();
        if let Some(msg) = msg {
            if let Some(state_init) = msg.state_init() {
                libraries.push(state_init.libraries().clone().inner());
            }
            if let Some(reason) = compute_new_state(
                &mut result_acc,
                msg,
                self.config(),
                params.block_unixtime,
                was_deleted_or_frozen,
            )? {
                *acc = result_acc;
                return Ok((TrComputePhase::skipped(reason), None, None));
            }
        };

        vm_phase.gas_credit = match gas.get_gas_credit() as u32 {
            0 => None,
            value => Some(value.try_into()?),
        };
        vm_phase.gas_limit = (gas.get_gas_limit() as u64).try_into()?;

        let code = if let Some(code) = result_acc.get_code() {
            code
        } else {
            vm_phase.exit_code = -13;
            if is_external {
                fail!(ExecutorError::NoAcceptError(vm_phase.exit_code, None))
            } else {
                vm_phase.exit_arg = None;
                vm_phase.success = false;
                vm_phase.gas_fees =
                    Coins::try_from(if is_special { 0 } else { gas_config.calc_gas_fee(0) })?;
                if !acc_balance.coins.sub(&vm_phase.gas_fees)? {
                    log::debug!(
                        target: "executor",
                        "can't sub funds: {} from acc_balance: {}",
                        vm_phase.gas_fees, acc_balance.coins
                    );
                    fail!("can't sub funds: from acc_balance")
                }
                *acc = result_acc;
                return Ok((TrComputePhase::Vm(vm_phase), None, None));
            }
        };
        let data = result_acc.get_data().unwrap_or_default();
        libraries.push(result_acc.libraries().inner());
        libraries.push(params.state_libs.clone());

        let code_hash = code.repr_hash();
        let precompiled = self.config().raw_config().precompiled_contracts_list()?;
        let precompiled = if let Some(list) = precompiled { list.get(&code_hash)? } else { None };
        if let Some(precompiled) = precompiled.as_ref() {
            if gas.get_gas_limit() < precompiled.gas_usage as i64 {
                log::debug!(target: "executor", "skip computing phase no gas for precompiled contract");
                return Ok((TrComputePhase::skipped(ComputeSkipReason::NoGas), None, None));
            }
            smc_info.set_precompiled_gas_usage(precompiled.gas_usage);
            // Contract is marked as precompiled in global config, but implementation is not available
            // In this case we run TVM and override gas_used
            log::info!(
                target: "executor",
                "Unknown precompiled contract (code_hash={code_hash:x}, gas_usage={}), running VM",
                precompiled.gas_usage
            );
            let gas_limit =
                if is_special { gas_config.special_gas_limit } else { gas_config.gas_limit };
            let gas_credit = if gas.get_gas_credit() != 0 { gas_limit } else { 0 };
            gas = Gas::new(
                gas_limit as i64,
                gas_credit as i64,
                gas_limit as i64,
                gas_config.get_real_gas_price() as i64,
            );
        }
        smc_info.set_mycode(code.clone());
        let mut ctrls = SaveList::new();
        ctrls.put(7, smc_info.as_temp_data_item()).unwrap();
        ctrls.put(4, StackItem::Cell(data)).unwrap();
        #[cfg(debug_assertions)]
        check_vm_init_params(&ctrls, &stack);

        let mut vm = Engine::with_capabilities(self.config().capabilites())
            .setup_checked(code.clone(), ctrls, stack, gas, libraries)
            .map_err(|err| error!("Cannot init TVM: {}", err))?;
        if params.debug {
            #[cfg(not(feature = "cross_check"))]
            vm.set_trace(Engine::TRACE_ALL);
        } else {
            vm.set_trace(0);
        }
        if let Some(trace_callback) = params.trace_callback.clone() {
            vm.set_arc_trace_callback(trace_callback);
        }
        vm.set_block_version(self.config().block_version());
        if let Some(modifiers) = params.behavior_modifiers.clone() {
            vm.modify_behavior(modifiers);
        }

        //TODO: set vm_init_state_hash

        let result = vm.execute();
        log::trace!(target: "executor", "execute result: {:?}", result);
        vm_phase.success = vm.is_committed_state();
        let mut raw_exit_arg = None;
        match result {
            Ok(exit_code) => vm_phase.exit_code = exit_code,
            Err(err) => {
                log::debug!(target: "executor", "VM terminated with exception: {}", err);
                let (exception, value) = tvm_exception_full(err)?;
                vm_phase.exit_code = if let Some(code) = exception.custom_code() {
                    code
                } else {
                    match exception.exception_code() {
                        Some(ExceptionCode::OutOfGas) => !(ExceptionCode::OutOfGas as i32), // correct error code according cpp code
                        Some(error_code) => error_code as i32,
                        None => ExceptionCode::UnknownError as i32,
                    }
                };
                // set exit_arg only if phase is !success
                if let Ok(v) = value.as_integer_value(i32::MIN..=i32::MAX) {
                    if v != 0 && !vm_phase.success {
                        vm_phase.exit_arg = Some(v);
                    }
                }
                raw_exit_arg = Some(value);
            }
        };
        log::debug!(target: "executor", "VM terminated with exit code {}", vm_phase.exit_code);

        // calc gas fees
        let gas = vm.get_gas();
        let credit = gas.get_gas_credit() as u32;
        //for external messages gas will not be exacted if VM throws the exception and gas_credit != 0
        if let Some(precompiled) = precompiled.as_ref() {
            vm_phase.gas_used = precompiled.gas_usage.try_into()?;
            vm_phase.vm_steps = 0;
        } else {
            let used = gas.get_gas_used() as u64;
            vm_phase.gas_used = used.try_into()?;
            vm_phase.vm_steps = vm.steps();
        }
        if credit != 0 {
            if is_external {
                fail!(ExecutorError::NoAcceptError(vm_phase.exit_code, raw_exit_arg))
            }
            vm_phase.gas_fees = Coins::zero();
        } else {
            // credit == 0 means contract accepted
            let gas_fees =
                if is_special { 0 } else { gas_config.calc_gas_fee(vm_phase.gas_used.as_u64()) };
            vm_phase.gas_fees = gas_fees.try_into()?;
        };

        log::debug!(
            target: "executor",
            "gas after: gl: {}, gc: {}, gu: {}, fees: {}",
            gas.get_gas_limit() as u64, credit, vm_phase.gas_used.as_u64(), vm_phase.gas_fees
        );

        //set mode
        vm_phase.mode = 0;
        //TODO: vm_final_state_hash
        log::debug!(target: "executor", "acc_balance: {}, gas fees: {}", acc_balance.coins, vm_phase.gas_fees);
        if !acc_balance.coins.sub(&vm_phase.gas_fees)? {
            log::error!(
                target: "executor",
                "This situation is unreachable: can't sub funds: {} from acc_balance: {}",
                vm_phase.gas_fees, acc_balance.coins
            );
            fail!("can't sub funds: from acc_balance")
        }

        let (new_data, out_actions) = vm.get_committed_state().unzip();
        *acc = result_acc;
        Ok((TrComputePhase::Vm(vm_phase), out_actions, new_data))
    }

    /// Implementation of transaction's action phase.
    /// If computing phase is successful then action phase is started.
    /// If TVM invoked in computing phase returned some output actions,
    /// then they will be added to transaction's output message list.
    /// Total value from all outbound internal messages will be collected and
    /// substracted from account balance. If account has enough funds this
    /// will be succeded, otherwise action phase is failed, transaction will be
    /// marked as aborted, account changes will be rollbacked.
    fn action_phase(
        &self,
        tr: &mut Transaction,
        acc: &mut Account,
        original_acc_balance: &CurrencyCollection,
        acc_balance: &mut CurrencyCollection,
        msg_remaining_balance: &mut CurrencyCollection, // it could be zero for tick tock transactions
        compute_phase_fees: &Coins,
        actions_cell: Cell,
        new_data: Option<Cell>,
        my_addr: &MsgAddressInt,
        is_special: bool,
    ) -> Result<ActionPhaseResult> {
        let mut out_msgs = vec![];
        let mut acc_copy = acc.clone();
        let mut acc_remaining_balance = acc_balance.clone();
        let mut phase = TrActionPhase::default();
        let mut total_reserved_value = Coins::default();
        let mut bounce = false;
        phase.action_list_hash = actions_cell.repr_hash().clone();
        let action_slices = match unpack_out_action_slices(actions_cell) {
            Ok(actions) => actions,
            Err((position, err)) => {
                log::debug!(
                    target: "executor",
                    "action list invalid at position {position}: {err}"
                );
                phase.result_code = RESULT_CODE_ACTIONLIST_INVALID;
                phase.result_arg = Some(position as i32);
                return Ok(ActionPhaseResult::from_phase(phase, bounce));
            }
        };
        if action_slices.len() > MAX_ACTIONS {
            log::debug!(target: "executor", "too many actions: {}", action_slices.len());
            phase.result_code = RESULT_CODE_TOO_MANY_ACTIONS;
            phase.result_arg = Some(MAX_ACTIONS as i32 + 1);
            return Ok(ActionPhaseResult::from_phase(phase, bounce));
        }
        phase.tot_actions = action_slices.len() as i16;

        let mut account_deleted = false;

        if phase.result_code != 0 {
            return Ok(ActionPhaseResult::from_phase(phase, bounce));
        }
        let limits = self.config().size_limits_config();

        let mut parsed_actions = Vec::with_capacity(action_slices.len());
        for (i, slice) in action_slices.into_iter().enumerate() {
            if let Err(err) = OutAction::skip(&mut slice.clone()) {
                log::debug!(
                    target: "executor",
                    "invalid action {i} found while preprocessing action list: {err:?}"
                );
                if let Some(BlockError::OutActionError(_, mode)) = err.downcast_ref() {
                    if mode.bit(SENDMSG_IGNORE_ERROR) {
                        phase.skipped_actions += 1;
                        parsed_actions.push(None);
                        continue;
                    } else if mode.bit(SENDMSG_BOUNCE_IF_FAIL) {
                        bounce = true;
                    }
                };
                phase.result_code = RESULT_CODE_UNKNOWN_OR_INVALID_ACTION;
                if i != 0 {
                    phase.result_arg = Some(i as i32);
                }
                return finish_action_phase_with_fine(
                    tr,
                    phase,
                    Some(msg_remaining_balance),
                    acc_balance,
                    bounce,
                );
            }
            parsed_actions.push(Some(slice));
        }

        phase.valid = true;
        for (i, slice) in parsed_actions.into_iter().enumerate() {
            let Some(mut slice) = slice else {
                continue;
            };
            // set result_arg to index of failed action in case of error
            phase.result_arg = Some(i as i32);
            let mut init_balance = acc_remaining_balance.clone();
            let mut err_code = match OutAction::construct_from(&mut slice) {
                Ok(OutAction::SendMsg { mode, mut out_msg }) => {
                    log::debug!(
                        target: "executor",
                        "\nAction #{i}\nType: SendMsg flag: {mode}, value: {}\nInitial balance: {}",
                        balance_to_string(out_msg.value()),
                        balance_to_string(Some(&acc_remaining_balance))
                    );
                    let result = outmsg_action_handler(
                        &mut phase,
                        mode,
                        &mut out_msg,
                        &mut acc_remaining_balance,
                        msg_remaining_balance,
                        compute_phase_fees,
                        self.config(),
                        is_special,
                        my_addr,
                        &total_reserved_value,
                        &mut account_deleted,
                    );
                    match result {
                        Ok(_) => {
                            phase.msgs_created += 1;
                            out_msgs.push(out_msg);
                            0
                        }
                        Err(code) => {
                            log::debug!(
                                target: "executor",
                                "outmsg_action_handler failed with code {code}"
                            );
                            if code != RESULT_CODE_SKIPPED && (SENDMSG_BOUNCE_IF_FAIL & mode) != 0 {
                                bounce = true;
                            }
                            code
                        }
                    }
                }
                Ok(OutAction::ReserveCurrency { mode, value }) => {
                    log::debug!(
                        target: "executor",
                        "\nAction #{i}\nType: ReserveCurrency flag: \
                        {mode} value: {}\nInitial balance: {}",
                        balance_to_string(Some(&value)),
                        balance_to_string(Some(&acc_remaining_balance))
                    );
                    match reserve_action_handler(
                        mode,
                        &value,
                        original_acc_balance,
                        &mut acc_remaining_balance,
                    ) {
                        Ok(reserved_value) => {
                            phase.spec_actions += 1;
                            match total_reserved_value.add(&reserved_value) {
                                Ok(_) => 0,
                                Err(_) => RESULT_CODE_INVALID_BALANCE,
                            }
                        }
                        Err(code) => {
                            if code != RESULT_CODE_SKIPPED && (RESERVE_BOUNCE_IF_FAIL & mode) != 0 {
                                bounce = true;
                            }
                            code
                        }
                    }
                }
                Ok(OutAction::SetCode { new_code }) => {
                    log::debug!(
                        target: "executor",
                        "\nAction #{i}\nType: SetCode {:x}\nInitial balance: {}",
                        new_code.repr_hash(),
                        balance_to_string(Some(&acc_remaining_balance))
                    );
                    match setcode_action_handler(&mut acc_copy, new_code) {
                        None => {
                            phase.spec_actions += 1;
                            0
                        }
                        Some(code) => code,
                    }
                }
                Ok(OutAction::ChangeLibrary { mode, code, hash }) => {
                    log::debug!(
                        target: "executor",
                        "\nAction #{i}\nType: ChangeLibrary flag: {mode}\nInitial balance: {}",
                        balance_to_string(Some(&acc_remaining_balance))
                    );
                    let mode = mode >> 1;
                    let code =
                        change_library_action_handler(limits, &mut acc_copy, mode, code, hash)
                            .unwrap_or_else(|err| {
                                log::debug!(
                                    target: "executor",
                                    "change_library_action_handler failed: {err}"
                                );
                                RESULT_CODE_LIB_BAD_ACCOUNT_STATE
                            });
                    if code == 0 {
                        phase.spec_actions += 1;
                    } else if mode.bit(CHANGE_SET_LIB_BOUNCE_IF_FAIL) {
                        bounce = true;
                    }
                    code
                }
                Ok(OutAction::None) => RESULT_CODE_UNKNOWN_OR_INVALID_ACTION,
                Err(err) => {
                    let msg = match err.downcast_ref() {
                        Some(BlockError::OutActionError(err, _)) => err.to_string(),
                        _ => err.to_string(),
                    };
                    log::debug!(
                        target: "executor",
                        "cannot parse outbound message in action {i}: format is invalid, err: {msg}"
                    );
                    RESULT_CODE_UNSUPPORTED
                }
            };
            init_balance.sub(&acc_remaining_balance)?;
            log::debug!(target: "executor", "Final balance:   {}\nDelta:           {}",
                balance_to_string(Some(&acc_remaining_balance)),
                balance_to_string(Some(&init_balance))
            );
            // This is required here because changes to libraries are applied even if action phase fails
            if err_code != 0 && !is_special && !check_account_size_limits(limits, &mut acc_copy)? {
                fail!("Account size limits exceeded");
            }
            if err_code == RESULT_CODE_UNSUPPORTED {
                err_code = RESULT_CODE_UNKNOWN_OR_INVALID_ACTION;
            }
            if err_code == RESULT_CODE_SKIPPED {
                phase.skipped_actions += 1;
            } else if err_code != 0 {
                log::debug!(target: "executor", "action failed: error_code={}", err_code);
                phase.valid = true;
                phase.result_code = err_code;
                if err_code == RESULT_CODE_NOT_ENOUGH_COINS
                    || err_code == RESULT_CODE_NOT_ENOUGH_EXTRA
                {
                    phase.no_funds = true;
                }
                return finish_action_phase_with_fine(
                    tr,
                    phase,
                    Some(msg_remaining_balance),
                    acc_balance,
                    bounce,
                );
            }
        }

        //calc new account balance
        if !total_reserved_value.is_zero() {
            log::debug!(
                target: "executor",
                "\nReturn reserved balance:\nInitial:  {}\nReserved: {}",
                balance_to_string(Some(&acc_remaining_balance)),
                total_reserved_value
            );
            if let Err(err) = acc_remaining_balance.coins.add(&total_reserved_value) {
                log::debug!(
                    target: "executor",
                    "failed to add account balance with reserved value {err}"
                );
                fail!("failed to add account balance with reserved value {}", err)
            }
        }

        log::debug!(
            target: "executor",
            "Final:    {}", balance_to_string(Some(&acc_remaining_balance))
        );

        msg_remaining_balance.coins.sub_checked(&phase.action_fine);

        if account_deleted {
            log::debug!(target: "executor", "\nAccount deleted");
            phase.status_change = AccStatusChange::Deleted;
        }

        phase.valid = true;

        if let Some(new_data) = new_data {
            acc_copy.set_data(new_data);
        }
        if !is_special && !check_account_size_limits(limits, &mut acc_copy)? {
            log::debug!(
                target: "executor",
                "Account size limits exceeded. Taking fine and rolling back state"
            );
            phase.result_code = RESULT_CODE_EXCEEDED_LIMITS;
            return finish_action_phase_with_fine(tr, phase, None, acc_balance, true);
        }

        phase.add_action_fine();
        let fee = phase.total_action_fees();
        log::debug!(target: "executor", "Total action fees: {}", fee);
        tr.add_fee_coins(&fee)?;

        // Reset result_arg on success
        phase.result_arg = None;
        phase.success = true;
        *acc_balance = acc_remaining_balance;
        *acc = acc_copy;
        Ok(ActionPhaseResult::new(phase, out_msgs, bounce))
    }

    /// Implementation of transaction's bounce phase.
    /// Bounce phase occurs only if transaction 'aborted' flag is set and
    /// if inbound message is internal message with field 'bounce=true'.
    /// Generates outbound internal message for original message sender, with value equal
    /// to value of original message minus gas payments and forwarding fees
    /// and empty body. Generated message is added to transaction's output message list.
    fn bounce_phase(
        &self,
        mut remaining_msg_balance: CurrencyCollection,
        acc_balance: &mut CurrencyCollection,
        compute_phase: &TrComputePhase,
        action_phase: Option<&TrActionPhase>,
        msg: &Message,
        tr: &mut Transaction,
        my_addr: &MsgAddressInt,
    ) -> Result<(TrBouncePhase, Option<Message>)> {
        let Some(mut int_header) = msg.int_header().cloned() else {
            fail!("Not found msg internal header")
        };
        if !int_header.bounce {
            fail!("Bounce flag not set")
        }
        // create bounced message and swap src and dst addresses
        int_header.dst = int_header.src()?.clone();
        if int_header.dst.rewrite_pfx().is_some() {
            fail!("Bounced message with rewritten source address is not allowed");
        }
        int_header.set_src(my_addr.clone());
        // create header for new bounced message and swap src and dst addresses
        int_header.ihr_disabled = true;
        int_header.bounce = false;
        int_header.bounced = true;
        let extra_flags = int_header.extra_flags.inner() as u8 & SUPPORTED_EXTRA_FLAGS;
        int_header.extra_flags = VarUInteger16::from(extra_flags as u64);

        let mut calc = StorageUsageCalc::with_limits(0, 0);
        if let Some(root) = int_header.value.other.root() {
            calc.append_cell(root, true, &mut 0)?;
        }
        let body = if extra_flags.bit(EXTRA_FLAG_NEW_BOUNCE_FORMAT) {
            let original_body = if let Some(body) = msg.body() {
                if extra_flags.bit(EXTRA_FLAG_FULL_BODY_BOUNCE) {
                    body.clone().into_cell()?
                } else {
                    body.get_slice(0, body.remaining_bits())?.into_cell()?
                }
            } else {
                Cell::default()
            };
            let (bounced_by_phase, exit_code, compute) = match &compute_phase {
                TrComputePhase::Vm(phase) => {
                    let compute = NewBounceComputePhaseInfo {
                        gas_used: phase.gas_used.inner() as u32,
                        vm_steps: phase.vm_steps,
                    };
                    if let Some(action) = action_phase {
                        (BouncedByPhase::Action, action.result_code, Some(compute))
                    } else {
                        (BouncedByPhase::Compute, phase.exit_code, Some(compute))
                    }
                }
                TrComputePhase::Skipped(skipped) => {
                    (BouncedByPhase::ComputeSkip, -(skipped.reason as i32), None)
                }
            };
            let bounce_body = NewBounceBody {
                original_body,
                original_info: ChildCell::with_struct(&NewBounceOriginalInfo {
                    value: int_header.value.clone(),
                    created_at: int_header.created_at,
                    created_lt: int_header.created_lt,
                })?,
                bounced_by_phase,
                exit_code,
                compute_phase: compute,
            };
            Some(bounce_body.write_to_new_cell()?)
        } else if self.config().has_capability(GlobalCapabilities::CapBounceMsgBody) {
            let mut builder = (-1i32).write_to_new_cell()?;
            if let Some(body) = msg.body() {
                let mut body_copy = body.clone();
                body_copy.shrink_data(0..256);
                builder.append_bytestring(&body_copy)?;
            }
            Some(builder)
        } else {
            None
        };

        let mut bounce_msg = Message::with_int_header(int_header);
        if let Some(body) = body {
            for cell in body.references() {
                calc.append_cell(cell, true, &mut 0)?;
            }
            bounce_msg.set_body(SliceData::load_builder(body)?);
        }
        let msg_size = calc.storage_used()?;

        let is_masterchain = msg.is_masterchain();
        let fwd_prices = self.config().get_fwd_prices(is_masterchain);
        let fwd_full_fees =
            fwd_prices.calc_fwd_fee(msg_size.bits(), msg_size.cells()).try_into()?;
        let fwd_mine_fees = fwd_prices.mine_fee_checked(&fwd_full_fees)?;
        let fwd_fees = fwd_full_fees - fwd_mine_fees;

        let compute_phase_fees = compute_phase.gas_fees();
        if remaining_msg_balance.coins < fwd_full_fees + compute_phase_fees {
            log::debug!(
                target: "executor",
                "bounce phase - not enough value {} to get compute fee \
                {compute_phase_fees} and fwd fee {fwd_full_fees}",
                remaining_msg_balance.coins
            );
            return Ok((TrBouncePhase::no_funds(msg_size, fwd_full_fees), None));
        }

        log::debug!(
            target: "executor",
            "get compute fee {compute_phase_fees} and forward fee {fwd_full_fees} \
            from bounce msg {remaining_msg_balance}"
        );

        acc_balance.sub(&remaining_msg_balance)?;
        remaining_msg_balance.coins.sub(&fwd_full_fees)?;
        remaining_msg_balance.coins.sub(&compute_phase_fees)?;
        let Some(int_header) = bounce_msg.int_header_mut() else {
            fail!("Error during getting message header")
        };
        int_header.value = remaining_msg_balance.clone();
        int_header.fwd_fee = fwd_fees;

        log::debug!(
            target: "executor",
            "bounce fees: {} bounce value: {}",
            fwd_full_fees, bounce_msg.value().unwrap().coins
        );
        tr.add_fee_coins(&fwd_mine_fees)?;
        Ok((TrBouncePhase::ok(msg_size, fwd_mine_fees, fwd_fees), Some(bounce_msg)))
    }

    fn add_messages(
        &self,
        tr: &mut Transaction,
        out_msgs: Vec<Message>,
        mut lt: u64,
    ) -> Result<u64> {
        lt += 1;
        for mut msg in out_msgs {
            msg.set_at_and_lt(tr.now(), lt);
            tr.add_out_message(&msg)?;
            lt += 1;
        }
        Ok(lt)
    }

    fn create_transaction(&self, account_id: AccountId) -> Transaction {
        Transaction::with_address_and_status(account_id, AccountStatus::AccStateNonexist)
    }
}

/// Calculate new account state according to inbound message and current account state.
/// If account does not exist - it can be created with uninitialized state.
/// If account is uninitialized - it can be created with active state.
/// If account exists - it can be frozen.
/// Returns computed initial phase.
fn compute_new_state(
    acc: &mut Account,
    in_msg: &Message,
    config: &BlockchainConfig,
    now: u32,
    was_deleted_or_frozen: bool,
) -> Result<Option<ComputeSkipReason>> {
    log::debug!(target: "executor", "compute_account_state");
    let original_state = acc.clone();
    match acc.status() {
        AccountStatus::AccStateNonexist => {
            log::error!(target: "executor", "account must exist");
            Ok(Some(if in_msg.state_init().is_none() {
                ComputeSkipReason::NoState
            } else {
                ComputeSkipReason::BadState
            }))
        }
        //Account exists, but can be in different states.
        AccountStatus::AccStateActive => {
            //account is active, just return it
            log::debug!(target: "executor", "account state: AccountActive");
            Ok(None)
        }
        AccountStatus::AccStateUninit => {
            log::debug!(target: "executor", "AccountUninit");
            if let Some(state_init) = in_msg.state_init() {
                let addr = acc.get_addr().ok_or_else(|| error!("account must have address"))?;
                if let Some(suspended_address_list) =
                    config.raw_config().suspended_address_list()?
                {
                    if suspended_address_list.is_address_suspended(addr, now)? {
                        log::debug!(target: "executor", "account is suspended");
                        return Ok(Some(ComputeSkipReason::Suspended));
                    }
                }
                // if msg is a constructor message then
                // borrow code and data from it and switch account state to 'active'.
                log::debug!(target: "executor", "message for uninitialized: activated");
                let text = "Cannot construct account from message with hash";
                if !check_state_init(state_init, addr.is_masterchain(), text, &in_msg.hash()?) {
                    return Ok(Some(ComputeSkipReason::BadState));
                }
                match acc.try_activate(state_init) {
                    Err(err) => {
                        log::debug!(target: "executor", "reason: {}", err);
                        Ok(Some(ComputeSkipReason::BadState))
                    }
                    Ok(_) => {
                        if check_account_size_limits(config.size_limits_config(), acc)? {
                            Ok(None)
                        } else {
                            *acc = original_state;
                            Ok(Some(ComputeSkipReason::BadState))
                        }
                    }
                }
            } else {
                log::debug!(target: "executor", "message for uninitialized: skip computing phase");
                Ok(Some(ComputeSkipReason::NoState))
            }
        }
        AccountStatus::AccStateFrozen => {
            log::debug!(target: "executor", "AccountFrozen");
            //account balance was credited and if it positive after that
            //and inbound message bear code and data then make some check and unfreeze account
            if let Some(state_init) = in_msg.state_init() {
                if was_deleted_or_frozen {
                    log::debug!(target: "executor", "account was frozen, skip computing phase");
                    return Ok(Some(ComputeSkipReason::BadState));
                }
                if let Some(due) = acc.due_payment() {
                    log::warn!(target: "executor", "unfreeze account with due payment {}", due);
                    // return Ok(Some(ComputeSkipReason::BadState))
                }
                let text = "Cannot unfreeze account from message with hash";
                if !check_state_init(state_init, false, text, &in_msg.hash()?) {
                    return Ok(Some(ComputeSkipReason::BadState));
                }
                if let Err(err) = acc.try_activate(state_init) {
                    log::debug!(target: "executor", "reason: {}", err);
                    Ok(Some(ComputeSkipReason::BadState))
                } else {
                    log::debug!(target: "executor", "message for frozen: activated");
                    if check_account_size_limits(config.size_limits_config(), acc)? {
                        Ok(None)
                    } else {
                        *acc = original_state;
                        Ok(Some(ComputeSkipReason::BadState))
                    }
                }
            } else {
                Ok(Some(ComputeSkipReason::NoState))
            }
        }
    }
}

fn check_replace_src_addr<'a>(
    src: &'a Option<MsgAddressInt>,
    acc_addr: &'a MsgAddressInt,
) -> Option<&'a MsgAddressInt> {
    match src {
        None => Some(acc_addr),
        Some(src) => match src {
            MsgAddressInt::AddrStd(_) => {
                if src != acc_addr {
                    None
                } else {
                    Some(src)
                }
            }
            MsgAddressInt::AddrVar(_) => None,
        },
    }
}

fn is_valid_addr_len(
    addr_len: u16,
    min_addr_len: u16,
    max_addr_len: u16,
    addr_len_step: u16,
) -> bool {
    (addr_len >= min_addr_len)
        && (addr_len <= max_addr_len)
        && ((addr_len == min_addr_len)
            || (addr_len == max_addr_len)
            || ((addr_len_step != 0) && (addr_len - min_addr_len).is_multiple_of(addr_len_step)))
}

fn check_rewrite_dest_addr(
    dst: &MsgAddressInt,
    config: &BlockchainConfig,
    my_addr: &MsgAddressInt,
) -> std::result::Result<MsgAddressInt, IncorrectCheckRewrite> {
    let repack;
    let (anycast_opt, addr_len, workchain_id, address) = match dst {
        MsgAddressInt::AddrVar(dst) => {
            repack = (dst.addr_len.as_u32() == 256)
                && (dst.workchain_id >= -128)
                && (dst.workchain_id < 128);
            (&dst.anycast, dst.addr_len.as_u16(), dst.workchain_id, &dst.address)
        }
        MsgAddressInt::AddrStd(dst) => {
            repack = false;
            (&dst.anycast, 256, dst.workchain_id as i32, &dst.address)
        }
    };

    let is_masterchain = workchain_id == MASTERCHAIN_ID;
    if !is_masterchain {
        if my_addr.workchain_id() != workchain_id && my_addr.workchain_id() != MASTERCHAIN_ID {
            log::debug!(
                target: "executor",
                "cannot send message from {} to {} it doesn't allow yet",
                my_addr, dst
            );
            return Err(IncorrectCheckRewrite::Other);
        }
        let workchains = config.raw_config().workchains().unwrap_or_default();
        if let Ok(Some(wc)) = workchains.get(&workchain_id) {
            if !wc.accept_msgs {
                log::debug!(
                    target: "executor",
                    "destination address belongs to workchain {} not accepting new messages",
                    workchain_id
                );
                return Err(IncorrectCheckRewrite::Other);
            }
            let (min_addr_len, max_addr_len, addr_len_step) = match wc.format {
                WorkchainFormat::Extended(wf) => {
                    (wf.min_addr_len(), wf.max_addr_len(), wf.addr_len_step())
                }
                WorkchainFormat::Basic(_) => (256, 256, 0),
            };
            if !is_valid_addr_len(addr_len, min_addr_len, max_addr_len, addr_len_step) {
                log::debug!(
                    target: "executor",
                    "destination address has length {} invalid for destination workchain {}",
                    addr_len, workchain_id
                );
                return Err(IncorrectCheckRewrite::HandWriteCheck);
            }
        } else {
            log::debug!(
                target: "executor",
                "destination address contains unknown workchain_id {}",
                workchain_id
            );
            return Err(IncorrectCheckRewrite::WrongWorkchain);
        }
    } else {
        if my_addr.workchain_id() != MASTERCHAIN_ID && my_addr.workchain_id() != BASE_WORKCHAIN_ID {
            log::debug!(
                target: "executor",
                "masterchain cannot accept from {} workchain",
                my_addr.workchain_id()
            );
            return Err(IncorrectCheckRewrite::Other);
        }
        if addr_len != 256 {
            log::debug!(
                target: "executor",
                "destination address has length {} invalid for destination workchain {}",
                addr_len, workchain_id
            );
            return Err(IncorrectCheckRewrite::Other);
        }
    }

    if anycast_opt.is_some() {
        log::debug!(target: "executor", "address cannot be anycast");
        return Err(IncorrectCheckRewrite::Anycast);
    }

    if !repack {
        Ok(dst.clone())
    } else if addr_len == 256 && (-128..128).contains(&workchain_id) {
        // repack as an addr_std
        MsgAddressInt::with_standart(anycast_opt.clone(), workchain_id as i8, address.clone())
            .map_err(|_| IncorrectCheckRewrite::Other)
    } else {
        // repack as an addr_var
        MsgAddressInt::with_variant(anycast_opt.clone(), workchain_id, address.clone())
            .map_err(|_| IncorrectCheckRewrite::Other)
    }
}

fn outmsg_action_handler(
    phase: &mut TrActionPhase,
    mut mode: u8,
    msg: &mut Message,
    acc_balance: &mut CurrencyCollection,
    msg_balance: &mut CurrencyCollection,
    compute_phase_fees: &Coins,
    config: &BlockchainConfig,
    is_special: bool,
    my_addr: &MsgAddressInt,
    reserved_value: &Coins,
    account_deleted: &mut bool,
) -> std::result::Result<CurrencyCollection, i32> {
    if msg.is_inbound_external() {
        log::debug!(target: "executor", "outbound msg cannot be inbound external");
        return Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION);
    }
    // outbound message cannot contain libraries
    if let Some(init) = msg.state_init() {
        if !init.libraries().is_empty() {
            log::debug!(target: "executor", "outbound internal msg with not empty libraries");
            return Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION);
        }
    }
    // we cannot send all balance from account and from message simultaneously ?
    if (mode & !SENDMSG_VALID_FLAGS) != 0
        || mode.bit(SENDMSG_REMAINING_MSG_BALANCE | SENDMSG_ALL_BALANCE)
        || (mode.bit(SENDMSG_DELETE_IF_EMPTY) && mode.non(SENDMSG_ALL_BALANCE))
    {
        log::error!(target: "executor", "outmsg mode has unsupported flags");
        return Err(RESULT_CODE_UNSUPPORTED);
    }
    let check_skip_invalid = move |code: i32| {
        if mode.bit(SENDMSG_IGNORE_ERROR) {
            Err(RESULT_CODE_SKIPPED)
        } else {
            Err(code)
        }
    };
    if let Some(new_src) = check_replace_src_addr(&msg.src(), my_addr) {
        msg.set_src_address(new_src.clone());
    } else {
        log::warn!(target: "executor", "Incorrect source address {:?}", msg.src());
        return Err(RESULT_CODE_INCORRECT_SRC_ADDRESS);
    }

    if let Some(int_header) = msg.int_header_mut() {
        match check_rewrite_dest_addr(&int_header.dst, config, my_addr) {
            Ok(new_dst) => int_header.dst = new_dst,
            Err(IncorrectCheckRewrite::Anycast) => {
                log::warn!(target: "executor", "Incorrect destination anycast address {}", int_header.dst);
                return check_skip_invalid(RESULT_CODE_INCORRECT_DST_ADDRESS);
            }
            Err(IncorrectCheckRewrite::WrongWorkchain) => {
                log::warn!(target: "executor", "Incorrect destination workchain id {}", int_header.dst);
                return check_skip_invalid(RESULT_CODE_INCORRECT_DST_ADDRESS);
            }
            Err(IncorrectCheckRewrite::HandWriteCheck) => {
                log::warn!(target: "executor", "Incorrect destination address according to handwrite check {}", int_header.dst);
                return check_skip_invalid(RESULT_CODE_INCORRECT_DST_ADDRESS);
            }
            Err(IncorrectCheckRewrite::Other) => {
                log::warn!(target: "executor", "Incorrect destination address {}", int_header.dst);
                return check_skip_invalid(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION);
            }
        }

        if int_header.extra_flags.inner() > SUPPORTED_EXTRA_FLAGS as u128 {
            log::warn!(target: "executor", "unsupported extra flags in outbound message: {}", int_header.extra_flags.inner());
            return check_skip_invalid(RESULT_CODE_INVALID_EXTRA_FLAGS);
        }

        if config.block_version() >= 8 {
            int_header.fwd_fee = Coins::zero();
        }
        if config.block_version() >= 11 {
            int_header.ihr_disabled = true;
        }
        int_header.bounced = false;

        let max_extras = config.size_limits_config().max_msg_extra_currencies as usize;
        let extras_count = int_header.value.other.count(max_extras + 1).map_err(|err| {
            log::warn!(target: "executor", "cannot count extra currencies: {err}");
            RESULT_CODE_TOO_MANY_EXTRA // replicate CPP behavior
        })?;
        if extras_count > max_extras {
            log::debug!(target: "executor",
                "too many extra currencies in outbound message");
            return check_skip_invalid(RESULT_CODE_TOO_MANY_EXTRA);
        }
        int_header.value.remove_zero_currencies().map_err(|err| {
            log::warn!(target: "executor", "cannot remove zero extra currencies: {err}");
            RESULT_CODE_TOO_MANY_EXTRA // replicate CPP behavior
        })?;
    }

    let fwd_prices = config.get_fwd_prices(msg.is_masterchain());
    let limits = config.size_limits_config();
    // calc max cells according to funds
    let mut max_cells = limits.max_msg_cells as u64;
    let mut fine_per_cell = 0;
    if !is_special {
        fine_per_cell = (fwd_prices.cell_price >> 16) / 4;
        let mut funds = acc_balance.coins.as_u128();
        if let Some(int_header) = msg.int_header() {
            if mode.non(SENDMSG_ALL_BALANCE) && mode.non(SENDMSG_PAY_FEE_SEPARATELY) {
                let mut new_funds = int_header.value.coins;
                if mode.bit(SENDMSG_REMAINING_MSG_BALANCE) {
                    new_funds += msg_balance.coins;
                    if !new_funds.sub_checked(compute_phase_fees) {
                        log::warn!(
                            target: "executor",
                            "not enough value {} to transfer with the message: \
                            all of the inbound message value has been consumed {}",
                            new_funds, compute_phase_fees
                        );
                        return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
                    }
                    if !new_funds.sub_checked(&phase.action_fine) {
                        log::warn!(
                            target: "executor",
                            "not enough value to transfer with the message: \
                            all of the inbound message value has been consumed"
                        );
                        return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
                    }
                }
                funds = funds.min(new_funds.as_u128());
            }
        }
        if funds < max_cells as u128 * fine_per_cell as u128 {
            max_cells = (funds / fine_per_cell as u128) as u64;
        }
        log::trace!(target: "executor", "max_cells {}, fine_per_cell {}, max_bits {}",
            max_cells, fine_per_cell, limits.max_msg_bits);
    }

    // copy for repack
    let int_header_copy = msg.int_header().cloned();
    let mut msg_copy = msg.copy_without_extra_currencies().unwrap_or_else(|| msg.clone());
    let mut fine = Coins::zero();

    // force body and init to be serialized in separate cells
    // according to cpp node
    let (force_body_to_ref, body) = match msg.body() {
        None => (false, None),
        Some(body) => {
            let b = body.clone().into_cell().unwrap_or_default();
            (b.references_count() >= 2, Some(b))
        }
    };
    let (force_init_to_ref, init) = match msg.state_init() {
        None => (false, None),
        Some(init) => {
            let b = init.serialize().unwrap_or_default();
            (b.references_count() >= 2, Some(b))
        }
    };
    // we will try to serialize message with different parameters to find the best one
    // first variant will None - will try to use custom layout
    // then we will try to serialize with body_to_ref and init_to_ref different combinations
    let serialize_params =
        [None, Some((false, false)), Some((true, false)), Some((false, true)), Some((true, true))];
    for params in serialize_params {
        let (body_to_ref, init_to_ref) = match params {
            None => {
                let (body_to_ref, init_to_ref) = msg_copy.serialization_params();
                (body_to_ref.unwrap_or_default(), init_to_ref.unwrap_or_default())
            }
            Some((body_to_ref, init_to_ref)) => (
                if !force_body_to_ref || body_to_ref {
                    body_to_ref
                } else {
                    continue;
                },
                if !force_init_to_ref || init_to_ref {
                    init_to_ref
                } else {
                    continue;
                },
            ),
        };
        let mut sstat = StorageUsageCalc::with_limits(max_cells, limits.max_msg_bits as u64);
        if let Some(body) = &body {
            sstat.append_cell(body, body_to_ref, &mut 0).map_err(|err| {
                log::error!(target: "executor", "cannot calc msg storage used for body: {err}");
                RESULT_CODE_UNKNOWN_OR_INVALID_ACTION
            })?;
        }
        if let Some(init) = &init {
            sstat.append_cell(init, init_to_ref, &mut 0).map_err(|err| {
                log::error!(target: "executor", "cannot calc msg storage used for state_init: {err}");
                RESULT_CODE_UNKNOWN_OR_INVALID_ACTION
            })?;
        }

        log::debug!(target: "executor", "msg_storage cells: {}, bits: {}", sstat.cells(), sstat.bits());
        if !is_special {
            fine = Coins::from(fine_per_cell * max_cells.min(sstat.cells()));
        }
        let mut acc_balance_copy = acc_balance.clone();
        let mut collect_fine = || {
            if !is_special {
                if acc_balance.coins.sub_checked(&fine) {
                    log::debug!(target: "executor", "collect fine {}", fine);
                } else {
                    log::debug!(target: "executor", "not enough funds to pay fine for cells, \
                        using all remaining balance {}", acc_balance.coins);
                    fine = acc_balance.coins;
                    acc_balance.coins.clear();
                }
                phase.action_fine += fine;
            }
        };
        let compute_fwd_fee = if !is_special {
            if sstat.cells() > max_cells && max_cells < limits.max_msg_cells as u64 {
                log::debug!(target: "executor", "not enough funds to process a message (max_cells={})", max_cells);
                collect_fine();
                return check_skip_invalid(RESULT_CODE_INVALID_BALANCE);
            }
            if sstat.bits() > limits.max_msg_bits as u64 || sstat.cells() > max_cells {
                log::debug!(target: "executor", "message too large, invalid");
                collect_fine();
                return check_skip_invalid(RESULT_CODE_INVALID_BALANCE);
            }
            if sstat.max_merkle_depth() > MAX_MSG_MERKLE_DEPTH {
                log::debug!(target: "executor", "message has too big merkle depth, invalid");
                collect_fine();
                return check_skip_invalid(RESULT_CODE_INVALID_BALANCE);
            }
            Coins::try_from(fwd_prices.calc_fwd_fee(sstat.bits(), sstat.cells()))
                .map_err(|err| {
                    log::error!(target: "executor", "cannot calc fwd fee message in action phase : {}", err);
                    RESULT_CODE_INVALID_BALANCE
                })?
        } else {
            Coins::zero()
        };
        log::debug!(target: "executor", "msg fwd fee {}", compute_fwd_fee);

        let (fwd_mine_fee, total_fwd_fees);
        let mut result_value; // to sub from acc_balance
        if let Some(int_header) = msg_copy.int_header_mut() {
            if let Some(h) = &int_header_copy {
                *int_header = h.clone()
            }
            result_value = int_header.value.clone();
            let fwd_fee = if !is_special {
                int_header.fwd_fee.max(compute_fwd_fee)
            } else {
                Default::default()
            };
            fwd_mine_fee =
                fwd_prices.mine_fee_checked(&fwd_fee).map_err(|_| RESULT_CODE_UNSUPPORTED)?;
            total_fwd_fees = if !is_special { fwd_fee } else { Default::default() };

            let fwd_remain_fee = fwd_fee - fwd_mine_fee;
            if mode.bit(SENDMSG_ALL_BALANCE) {
                //send all remaining account balance
                result_value.coins = acc_balance_copy.coins;
                int_header.value = result_value.clone();

                mode &= !SENDMSG_PAY_FEE_SEPARATELY;
            } else if mode.bit(SENDMSG_REMAINING_MSG_BALANCE) {
                //send all remainig balance of inbound message
                result_value.coins.add(&msg_balance.coins).ok();
                if mode.non(SENDMSG_PAY_FEE_SEPARATELY) {
                    if &result_value.coins < compute_phase_fees {
                        // TODO: collect_fine();
                        return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
                    }
                    result_value.coins.sub(compute_phase_fees).map_err(|err| {
                        log::error!(target: "executor", "cannot subtract msg balance : {}", err);
                        RESULT_CODE_ACTIONLIST_INVALID
                    })?;
                }
                int_header.value = result_value.clone();
            }
            if mode.bit(SENDMSG_PAY_FEE_SEPARATELY) {
                //we must pay the fees, sum them with msg value
                result_value.coins += total_fwd_fees;
            } else if int_header.value.coins < total_fwd_fees {
                //msg value is too small, reciever cannot pay the fees
                log::warn!(
                    target: "executor",
                    "msg balance {} is too small, cannot pay fwd+ihr fees: {}",
                    int_header.value.coins, total_fwd_fees
                );
                collect_fine();
                return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
            } else {
                //reciever will pay the fees
                int_header.value.coins -= total_fwd_fees;
            }

            //set evaluated fees and value back to msg
            int_header.fwd_fee = fwd_remain_fee;
            if let Some(h) = msg.int_header_mut() {
                *h = int_header.clone();
            }
        } else if msg.ext_out_header().is_some() {
            fwd_mine_fee = compute_fwd_fee;
            total_fwd_fees = compute_fwd_fee;
            result_value = CurrencyCollection::from_coins(compute_fwd_fee);
        } else {
            return Err(-1);
        }

        if acc_balance_copy.coins < result_value.coins {
            log::warn!(
                target: "executor",
                "account balance {} is too small, cannot send {}", acc_balance_copy.coins, result_value.coins
            );
            collect_fine();
            return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
        }
        // check if we need to repack message
        let result = msg.serialize_with_params(Some(body_to_ref), Some(init_to_ref));
        let root = match result {
            Ok((root, _, _)) => root,
            Err(err) => {
                log::debug!(target: "executor", "outbound message does not fit into a cell after rewriting {body_to_ref:?}, {init_to_ref:?}: {err}");
                continue;
            }
        };
        let add_bits = root.length_in_bits() as u64;
        if params.is_some() {
            match root.into_cell().and_then(Deserializable::construct_from_cell) {
                Ok(msg_repacked) => {
                    *msg = msg_repacked;
                }
                Err(err) => {
                    // it is impossible case
                    log::debug!(target: "executor", "cannot construct repacked message: {err}");
                    collect_fine();
                    return Err(-2);
                }
            }
        }
        if matches!(acc_balance_copy.sub(&result_value), Ok(false) | Err(_)) {
            log::warn!(
                target: "executor",
                "account balance {} is too small, cannot send {}", acc_balance_copy.coins, result_value.coins
            );
            collect_fine();
            return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_EXTRA);
        }
        let limit = limits.max_msg_extra_currencies as usize;
        match acc_balance_copy.other.filter_all_values(VarUInteger32::is_zero, Some(limit)) {
            Ok(HashmapFilterResult::Cancel) => {
                log::debug!(target: "executor", "invalid value:ExtraCurrencies in a proposed outbound message: too many currencies (max {limit})");
                return check_skip_invalid(RESULT_CODE_TOO_MANY_EXTRA);
            }
            Err(err) => {
                log::warn!(target: "executor", "invalid value:ExtraCurrencies in a proposed outbound message : {err}");
                return check_skip_invalid(RESULT_CODE_NOT_ENOUGH_COINS);
            }
            _ => (),
        }
        *acc_balance = acc_balance_copy;

        if mode.bit(SENDMSG_DELETE_IF_EMPTY)
            && mode.bit(SENDMSG_ALL_BALANCE)
            && acc_balance.coins.is_zero()
            && reserved_value.is_zero()
        {
            *account_deleted = true;
        }

        // total fwd fees is sum of messages full fwd and ihr fees
        log::debug!(target: "executor", "action add forward fee {}", total_fwd_fees);
        phase.add_fwd_fees(&total_fwd_fees);

        // total action fees is sum of messages fwd mine fees
        log::debug!(target: "executor", "action add action fee {}", fwd_mine_fee);
        phase.add_action_fees(&fwd_mine_fee);

        phase.tot_msg_size.add_bits_and_cells(sstat.bits() + add_bits, sstat.cells() + 1);

        if mode.any(SENDMSG_ALL_BALANCE | SENDMSG_REMAINING_MSG_BALANCE) {
            msg_balance.coins.clear();
        }

        log::debug!(
            target: "executor",
            "Message details:\n\tFlag: {mode}\n\tValue: {}\n\tSource: {}\n\tDestination: {}\n\tBody: {}\n\tStateInit: {}",
            balance_to_string(Some(&result_value)),
            msg.src().map_or("None".to_string(), |addr| addr.to_string()),
            msg.dst().map_or("None".to_string(), |addr| addr.to_string()),
            msg.body().map_or("None".to_string(), |data| data.to_string()),
            msg.state_init().map_or("None".to_string(), |_| "Present".to_string())
        );
        return Ok(result_value);
    }
    if !is_special {
        if acc_balance.coins.sub_checked(&fine) {
            log::debug!(target: "executor", "collect fine {}", fine);
        } else {
            log::debug!(target: "executor", "not enough funds to pay fine for cells, \
                using all remaining balance {}", acc_balance.coins);
            fine = acc_balance.coins;
            acc_balance.coins.clear();
        }
        phase.action_fine += fine;
    }
    Err(-2) // cannot repack message - impossible case
}

/// Reserves some coins from accout balance.
/// Returns calculated reserved value. its calculation depends on mode.
/// Reduces balance by the amount of the reserved value.
fn reserve_action_handler(
    mode: u8,
    val: &CurrencyCollection,
    original_acc_balance: &CurrencyCollection,
    acc_remaining_balance: &mut CurrencyCollection,
) -> std::result::Result<Coins, i32> {
    if mode & !RESERVE_VALID_MODES != 0 {
        return Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION);
    }
    log::debug!(target: "executor", "Reserve with mode = {} value = {}", mode, balance_to_string(Some(val)));

    if !val.other.is_empty() {
        return Err(RESULT_CODE_UNSUPPORTED);
    }
    let mut reserved;
    if mode & RESERVE_PLUS_ORIG != 0 {
        // Append all currencies
        if mode & RESERVE_REVERSE != 0 {
            reserved = original_acc_balance.coins;
            if !reserved.sub_checked(&val.coins) {
                return Err(RESULT_CODE_UNSUPPORTED);
            }
        } else {
            reserved = val.coins;
            reserved.add(&original_acc_balance.coins).or(Err(RESULT_CODE_INVALID_BALANCE))?;
        }
    } else {
        if mode & RESERVE_REVERSE != 0 {
            // flag 8 without flag 4 unacceptable
            return Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION);
        }
        reserved = val.coins;
    }
    if let Ok(true) = acc_remaining_balance.coins.sub(&reserved) {
    } else if mode & RESERVE_IGNORE_ERROR != 0 {
        reserved = acc_remaining_balance.coins;
        acc_remaining_balance.coins.clear();
    } else {
        return Err(RESULT_CODE_NOT_ENOUGH_COINS);
    }

    if mode & RESERVE_ALL_BUT != 0 {
        // swap all currencies
        std::mem::swap(&mut reserved, &mut acc_remaining_balance.coins);
    }

    Ok(reserved)
}

fn setcode_action_handler(acc: &mut Account, code: Cell) -> Option<i32> {
    log::debug!(target: "executor", "OutAction::SetCode\nPrevious code hash: {:x}\nNew code hash:      {:x}",
        acc.get_code().unwrap_or_default().repr_hash(),
        code.repr_hash(),
    );
    if code.level() > 0 {
        return Some(RESULT_CODE_NON_ZERO_CELL_LEVEL);
    }
    match acc.set_code(code) {
        true => None,
        false => Some(RESULT_CODE_BAD_ACCOUNT_STATE),
    }
}

fn change_library_action_handler(
    limits: &SizeLimitsConfig,
    acc: &mut Account,
    mode: u8, // raw value left shifted
    code: Option<Cell>,
    hash: Option<UInt256>,
) -> Result<i32> {
    let Some(library) = acc.library_mut() else { return Ok(RESULT_CODE_LIB_BAD_ACCOUNT_STATE) };
    if let Some(code) = code {
        log::debug!(target: "executor", "OutAction::ChangeLibrary mode: {}, code: {}", mode, code);
        if code.level() > 0 {
            return Ok(RESULT_CODE_LIB_BAD_CELL);
        }
        let hash = code.repr_hash();
        let is_public = mode.bit(SET_LIB_CODE_ADD_PUBLIC);
        if let Some(exist) = library.get(hash)? {
            if exist.root.repr_hash() == hash && exist.is_public_library() == is_public {
                return Ok(0);
            }
        }
        let mut sstat = StorageUsageCalc::with_limits(0, 0);
        let max_merkle_depth = sstat.append_cell(&code, true, &mut 0)? as u8;
        if sstat.cells() > limits.max_library_cells as u64 || max_merkle_depth > MAX_MERKLE_DEPTH {
            log::debug!(target: "executor", "library exceeds size limits: cells {}, bits {}, merkle depth {}",
                sstat.cells(), sstat.bits(), max_merkle_depth);
            return Ok(RESULT_CODE_LIB_EXCEEDED_LIMITS);
        }
        let key = hash.write_to_bitstring()?;
        let lib = SimpleLib::new(code, is_public);
        library.set_raw(key, &lib.write_to_new_cell()?)?;
    } else if let Some(hash) = hash {
        log::debug!(target: "executor", "OutAction::ChangeLibrary mode: {}, hash: {:x}", mode, hash);
        library.remove(&hash)?;
    } else {
        log::warn!(target: "executor", "OutAction::ChangeLibrary mode: {}", mode);
        return Ok(RESULT_CODE_LIB_BAD_CELL);
    }
    Ok(0)
}

fn override_gas(acc: &Account, now: u32) -> Option<u64> {
    let addr = acc.get_addr()?;
    let (until, gas_limit) = *SPECIAL_LIMIT_ACCOUNTS.get(addr)?;
    if now < until {
        log::debug!(target: "executor", "overriding gas limit for account {addr} to {gas_limit} until {until}");
        Some(gas_limit)
    } else {
        None
    }
}

fn init_gas(
    acc: &Account,
    now: u32,
    acc_balance: u128,
    msg_balance: u128,
    is_external: bool,
    is_special: bool,
    is_ordinary: bool,
    gas_info: &GasLimitsPrices,
) -> Gas {
    let (gas_limit, max_gas_threshold) = if let Some(gas_limit) = override_gas(acc, now) {
        (gas_limit, gas_info.calc_max_gas_threshold(gas_limit))
    } else {
        (gas_info.gas_limit, gas_info.max_gas_threshold)
    };
    let gas_max = if is_special {
        gas_info.special_gas_limit
    } else {
        gas_info.calc_gas(acc_balance, gas_limit, max_gas_threshold).min(gas_limit)
    };
    let gas_limit = if !is_ordinary || is_special {
        gas_max
    } else {
        gas_max.min(gas_info.calc_gas(msg_balance, gas_limit, max_gas_threshold))
    };
    let gas_credit = if is_external {
        gas_info.gas_credit.min(gas_max).min(VarUInteger3::MAX.as_u64())
    } else {
        0
    };
    log::debug!(
        target: "executor",
        "gas before: gm: {}, gl: {}, gc: {}, price: {}",
        gas_max, gas_limit, gas_credit, gas_info.get_real_gas_price()
    );
    Gas::new(
        gas_limit as i64,
        gas_credit as i64,
        gas_max as i64,
        gas_info.get_real_gas_price() as i64,
    )
}

fn check_state_init(
    init: &StateInit,
    disable_set_lib: bool,
    text: &str,
    msg_hash: &UInt256,
) -> bool {
    if let Some(cell) = init.libraries().root() {
        if cell.level() > 0 {
            log::debug!(target: "executor", "non-zero level in stateinit libs");
            return false;
        }
    }
    match init.libraries().len() {
        Ok(len) => {
            if !disable_set_lib || len == 0 {
                true
            } else {
                log::debug!(
                    target: "executor",
                    "{} {:x} because libraries are disabled",
                        text, msg_hash
                );
                false
            }
        }
        Err(err) => {
            log::debug!(
                target: "executor",
                "{} {:x} because libraries are broken {}",
                    text, msg_hash, err
            );
            false
        }
    }
}

/// Calculate new account according to inbound message.
/// If message has no value, account will not created.
/// If hash of state_init is equal to account address (or flag check address is false), account will be active.
/// Otherwise, account will be nonexist or uninit according bounce flag: if bounce, account will be uninit that save money.
pub(super) fn account_from_message(
    config: &BlockchainConfig,
    msg: &Message,
    account_address: &MsgAddressInt, // without anycast
    msg_remaining_balance: &CurrencyCollection,
    last_paid: u32,
    check_address: bool, // used only in tests
) -> Option<Account> {
    let hdr = msg.int_header()?;
    let msg_hash = msg.hash().ok()?;
    if let Some(init) = msg.state_init() {
        if init.code().is_some() {
            let init_hash = init.hash().ok()?;
            let equal = if let Some(fixed_prefix_length) = init.fixed_prefix_length() {
                if let Err(err) = config.check_fixed_prefix_length(fixed_prefix_length.as_u32()) {
                    log::trace!(
                        target: "executor",
                        "Cannot construct account from message with hash {msg_hash:x} because {err}"
                    );
                    return None;
                }
                let mut init_hash = SliceData::from(&init_hash);
                let mut address = account_address.address().clone();
                init_hash.move_by(fixed_prefix_length.as_usize()).ok()?;
                address.move_by(fixed_prefix_length.as_usize()).ok()?;
                init_hash == address
            } else {
                account_address.address().contains_bytes(init_hash.as_slice())
            };
            if !check_address || equal {
                let text = "Cannot construct account from message with hash";
                let disable_set_lib = account_address.is_masterchain();
                if check_state_init(init, disable_set_lib, text, &msg_hash) {
                    return Account::active(
                        account_address.clone(),
                        msg_remaining_balance.clone(),
                        0,
                        last_paid,
                        init.clone(),
                        config.size_limits_config().acc_state_cells_for_storage_dict,
                    )
                    .ok();
                }
            } else if check_address {
                log::trace!(
                    target: "executor",
                    "Cannot construct account from message with hash {msg_hash:x} \
                        because the destination address {:x} does not match with hash message code {init_hash:x}",
                        account_address.address()
                );
            }
        }
    }
    if !hdr.bounce {
        Some(Account::uninit(account_address.clone(), msg_remaining_balance.clone(), 0, last_paid))
    } else {
        log::trace!(
            target: "executor",
            "Account will not be created. Value of {:x} message will be returned",
            msg_hash
        );
        None
    }
}
fn balance_to_string(balance: Option<&CurrencyCollection>) -> String {
    let value = balance.map_or(0, |cc| cc.coins.as_u128());
    if value == 0 {
        return "0".to_string();
    }
    format!(
        "{}.{:03} {:03} {:03}      ({})",
        value / 1e9 as u128,
        (value % 1e9 as u128) / 1e6 as u128,
        (value % 1e6 as u128) / 1e3 as u128,
        value % 1e3 as u128,
        value,
    )
}

fn finish_action_phase_with_fine(
    tr: &mut Transaction,
    mut phase: TrActionPhase,
    msg_remaining_balance: Option<&mut CurrencyCollection>,
    acc_balance: &mut CurrencyCollection,
    bounce: bool,
) -> Result<ActionPhaseResult> {
    if let Some(msg_remaining_balance) = msg_remaining_balance {
        msg_remaining_balance.coins.sub_checked(&phase.action_fine);
    }
    acc_balance.coins.sub_checked(&phase.action_fine);
    phase.del_action_fees();
    phase.add_action_fine();
    let fee = phase.total_action_fees();
    log::debug!(target: "executor", "Total action fine: {}", fee);
    tr.add_fee_coins(&fee)?;
    Ok(ActionPhaseResult::new(phase, vec![], bounce))
}

#[cfg(debug_assertions)]
fn check_vm_init_params(ctrls: &SaveList, stack: &Stack) {
    // account balance is duplicated in stack and in c7 - so check
    let balance_in_smc = ctrls
        .get(7)
        .unwrap()
        .tuple_item_ref(0)
        .unwrap()
        .tuple_item_ref(7)
        .unwrap()
        .tuple_item_ref(0)
        .unwrap()
        .as_integer()
        .unwrap();
    let stack_depth = stack.depth();
    debug_assert_ne!(stack_depth, 0, "stack is empty");
    let balance_in_stack = stack.get(stack_depth - 1).unwrap().as_integer().unwrap();
    debug_assert_eq!(balance_in_smc, balance_in_stack);
}

pub(super) fn check_account_size_limits(cfg: &SizeLimitsConfig, acc: &mut Account) -> Result<bool> {
    let max_acc_state_cells = if acc.get_addr().is_some_and(|addr| addr.is_masterchain()) {
        cfg.max_mc_acc_state_cells as u64
    } else {
        cfg.max_acc_state_cells as u64
    };
    let acc_id = acc.get_id().cloned().unwrap_or_default();
    let _tc = time_checker!(|| format!("account {:x} precalc_storage_stat", acc_id), 100);
    let Some(stat) = acc.precalc_storage_stat()? else {
        return Ok(true);
    };
    if stat.total_cells() > max_acc_state_cells {
        log::debug!(target: "executor", "account has too many cells {} (max cells = {})", stat.total_cells(), max_acc_state_cells);
        return Ok(false);
    }
    let max_merkle_depth = stat.max_merkle_depth()?;
    if max_merkle_depth > MAX_MERKLE_DEPTH {
        log::debug!(target: "executor", "account has too big merkle depth {max_merkle_depth}");
        return Ok(false);
    }
    Ok(true)
}
