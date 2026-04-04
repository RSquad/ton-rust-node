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
use crate::{
    account_from_message, blockchain_config::BlockchainConfig, check_account_size_limits,
    error::ExecutorError, ActionPhaseResult, ExecuteParams, TransactionExecutor,
};
#[cfg(feature = "timings")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "timings")]
use std::time::Instant;
use ton_block::{
    error, fail, AccStatusChange, Account, AddSub, Cell, Coins, CommonMsgInfo, ComputeSkipReason,
    Deserializable, Message, MsgAddressInt, Result, Serializable, StorageUsageCalc, TrBouncePhase,
    TrComputePhase, Transaction, TransactionDescr, TransactionDescrOrdinary, MASTERCHAIN_ID,
    MAX_MSG_MERKLE_DEPTH,
};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
    SmartContractInfo,
};

#[cfg(test)]
#[path = "tests/test_ordinary_transaction.rs"]
mod tests1;

#[cfg(test)]
#[path = "tests/test_ordinary_libs_and_code.rs"]
mod tests2;

#[cfg(test)]
#[path = "tests/test_ordinary_freeze.rs"]
mod tests3;

#[cfg(test)]
#[path = "tests/test_ordinary_rawreserve.rs"]
mod tests4;

#[cfg(test)]
#[path = "tests/test_random_gen.rs"]
mod tests5;

#[cfg(test)]
#[path = "tests/test_currency_collections.rs"]
mod tests6;

#[cfg(test)]
#[path = "tests/test_bounced_action_phase.rs"]
mod tests7;

pub struct OrdinaryTransactionExecutor {
    config: BlockchainConfig,

    #[cfg(feature = "timings")]
    timings: [AtomicU64; 3], // 0 - preparation, 1 - compute, 2 - after compute
}

impl OrdinaryTransactionExecutor {
    pub fn new(config: BlockchainConfig) -> Self {
        Self {
            config,

            #[cfg(feature = "timings")]
            timings: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
        }
    }

    #[cfg(feature = "timings")]
    pub fn timing(&self, kind: usize) -> u64 {
        self.timings[kind].load(Ordering::Relaxed)
    }
}

impl TransactionExecutor for OrdinaryTransactionExecutor {
    ///
    /// Create end execute transaction from message for account
    fn execute_with_params(
        &self,
        in_msg_cell: Option<Cell>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        #[cfg(feature = "timings")]
        let mut now = Instant::now();

        let in_msg_cell =
            in_msg_cell.ok_or_else(|| error!("Ordinary transaction must have input message"))?;
        let in_msg_hash = in_msg_cell.repr_hash();
        let in_msg = Message::construct_from_cell(in_msg_cell.clone())?;
        log::debug!(
            target: "executor",
            "Ordinary transaction executing, in message id: {in_msg_hash:x}"
        );
        let mut msg_balance = Default::default();
        let (bounce, is_ext_msg, account_address) = match in_msg.header() {
            CommonMsgInfo::ExtOutMsgInfo(_) => fail!(ExecutorError::InvalidExtMessage),
            CommonMsgInfo::IntMsgInfo(hdr) => {
                msg_balance = hdr.value.clone();
                (hdr.bounce, false, &hdr.dst)
            }
            CommonMsgInfo::ExtInMsgInfo(hdr) => (false, true, &hdr.dst),
        };

        let (wc_id, account_id) = account_address.extract_std_address(true)?;
        let is_masterchain = wc_id == MASTERCHAIN_ID;
        log::debug!(target: "executor", "Account = {}:{:x}", wc_id,account_id);
        if let Some(hash) = account.frozen_hash() {
            log::debug!(target: "executor", "Account is frozen, hash = {:x}", hash);
        }
        let mut acc_balance = account.balance().cloned().unwrap_or_default();
        let is_special = self.config.is_special_account(is_masterchain, &account_id)?;
        let account_address = MsgAddressInt::with_params(wc_id, account_id.clone())?;

        log::debug!(
            target: "executor",
            "acc_balance: {}, msg_balance: {}, credit_first: {}, is_special: {}",
            acc_balance.coins, msg_balance.coins, !bounce, is_special);

        log::debug!(target: "executor", "account_last_tr_lt: {}, last_tr_lt: {}, msg_lt: {}, now: {}",
            account.last_tr_time().unwrap_or_default(),
            params.last_tr_lt,
            in_msg.created_lt().unwrap_or_default() + 1,
            params.block_unixtime,
        );

        let lt = account
            .last_tr_time()
            .unwrap_or(0)
            .max(params.last_tr_lt)
            .max(in_msg.created_lt().unwrap_or(0) + 1);
        let mut tr = self.create_transaction(account_id.clone());
        tr.orig_status = account.status();
        tr.set_logical_time(lt);
        tr.set_now(params.block_unixtime);
        tr.set_in_msg_cell(in_msg_cell.clone());

        let mut description = TransactionDescrOrdinary {
            credit_first: !bounce,
            ..TransactionDescrOrdinary::default()
        };

        // first check if contract can pay for importing external message
        if is_ext_msg && !is_special {
            // extranal message comes serialized
            let limits = self.config.size_limits_config();
            let mut calc = StorageUsageCalc::with_limits(
                limits.max_msg_cells as u64,
                limits.max_msg_bits as u64,
            );
            let max_merkle_depth = calc.append_cell(&in_msg_cell, false, &mut 0)?;
            log::debug!(target: "executor", "inbound external message storage bits: {}, cells: {}", calc.bits(), calc.cells());
            if calc.bits() > limits.max_msg_bits as u64
                || calc.cells() > limits.max_msg_cells as u64
            {
                log::debug!(target: "executor", "inbound external message too large, invalid");
                fail!(ExecutorError::InvalidExtMessage)
            }
            if max_merkle_depth > MAX_MSG_MERKLE_DEPTH {
                log::debug!(target: "executor", "inbound external message has too big merkle depth, invalid");
                fail!(ExecutorError::InvalidExtMessage)
            }
            let fwd_prices = self.config.get_fwd_prices(in_msg.is_masterchain());
            let in_fwd_fee = fwd_prices.calc_fwd_fee(calc.bits(), calc.cells());
            log::debug!(target: "executor", "import message fee: {}, acc_balance: {}", in_fwd_fee, acc_balance.coins);

            let in_fwd_fee = Coins::try_from(in_fwd_fee)?;
            if !acc_balance.coins.sub(&in_fwd_fee)? {
                fail!(ExecutorError::NoFundsToImportMsg)
            }
            tr.add_fee_coins(&in_fwd_fee)?;
        }

        if let Some(burning_cfg) = self.config.burning_config() {
            if is_masterchain
                && !msg_balance.coins.is_zero()
                && burning_cfg.blackhole_addr.as_ref() == Some(&account_id)
            {
                let burned = std::mem::take(&mut msg_balance.coins);
                log::debug!(
                    target: "executor",
                    "Burning {burned} nanocoins for blackhole account {account_id:x}",
                );
                tr.set_blackhole_burned(burned);
            }
        }

        if description.credit_first && !is_ext_msg {
            description.credit_ph = match self.credit_phase(&msg_balance, &mut acc_balance) {
                Ok(credit_ph) => Some(credit_ph),
                Err(e) => fail!(
                    ExecutorError::TrExecutorError(
                        format!("cannot create credit phase of a new transaction for smart contract for reason {}", e)
                    )
                )
            };
        }
        let storage_fees_collected;
        let was_deleted_or_frozen;
        description.storage_ph = match self.storage_phase(
            account,
            &mut acc_balance,
            &mut tr,
            is_masterchain,
            is_special,
        ) {
            Ok(storage_ph) => {
                storage_fees_collected = storage_ph.storage_fees_collected.as_u128();
                was_deleted_or_frozen = storage_ph.status_change != AccStatusChange::Unchanged;
                Some(storage_ph)
            }
            Err(e) => fail!(ExecutorError::TrExecutorError(format!(
                "cannot create storage phase of a new transaction for smart contract for reason {}",
                e
            ))),
        };

        if description.credit_first && msg_balance.coins > acc_balance.coins {
            msg_balance.coins = acc_balance.coins;
        }

        log::debug!(target: "executor",
            "storage_phase: {}", if description.storage_ph.is_some() {"present"} else {"none"});
        let mut original_acc_balance = account.balance().cloned().unwrap_or_default();
        original_acc_balance.sub(tr.total_fees())?;

        if !description.credit_first && !is_ext_msg {
            description.credit_ph = match self.credit_phase(&msg_balance, &mut acc_balance) {
                Ok(credit_ph) => Some(credit_ph),
                Err(e) => fail!(
                    ExecutorError::TrExecutorError(
                        format!("cannot create credit phase of a new transaction for smart contract for reason {}", e)
                    )
                )
            };
        }
        log::debug!(target: "executor",
            "credit_phase: {}", if description.credit_ph.is_some() {"present"} else {"none"});

        let last_paid = if !is_special { params.block_unixtime } else { 0 };
        account.set_last_paid(last_paid);
        #[cfg(feature = "timings")]
        {
            self.timings[0].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        let config_params = self.config.raw_config().clone();
        let mut smc_info = SmartContractInfo {
            myself: account_address.write_to_bitstring()?,
            block_lt: params.block_lt,
            trans_lt: lt,
            unix_time: params.block_unixtime,
            balance: acc_balance.clone(),
            in_msg: Some(in_msg.clone()),
            incoming_value: msg_balance.clone(),
            storage_fees_collected,
            config_params,
            prev_blocks_info: params.prev_blocks_info.clone(),
            ..Default::default()
        };
        smc_info.calc_rand_seed(params.seed_block.clone(), &account_id.get_bytestring(0));
        let mut stack = Stack::new();
        stack
            .push(int!(acc_balance.coins.as_u128()))
            .push(int!(msg_balance.coins.as_u128()))
            .push(StackItem::Cell(in_msg_cell))
            .push(StackItem::Slice(in_msg.body().cloned().unwrap_or_default()))
            .push(boolean!(is_ext_msg));
        log::debug!(target: "executor", "compute_phase");
        let mut bad_state = false;
        if account.is_none() && !is_ext_msg && !was_deleted_or_frozen {
            if let Some(mut new_acc) = account_from_message(
                self.config(),
                &in_msg,
                &account_address,
                &msg_balance,
                if !is_special { smc_info.unix_time() } else { 0 },
                true,
            ) {
                if check_account_size_limits(self.config().size_limits_config(), &mut new_acc)? {
                    *account = new_acc;
                } else {
                    bad_state = true;
                }
            }
        }

        let (compute_ph, actions, new_data) = if bad_state {
            (TrComputePhase::skipped(ComputeSkipReason::BadState), None, None)
        } else {
            match self.compute_phase(
                Some(&in_msg),
                account,
                &mut acc_balance,
                &msg_balance,
                smc_info,
                stack,
                is_masterchain,
                is_special,
                was_deleted_or_frozen,
                &params,
            ) {
                Ok((compute_ph, actions, new_data)) => (compute_ph, actions, new_data),
                Err(e) => {
                    log::debug!(target: "executor", "compute_phase error: {}", e);
                    match e.downcast_ref::<ExecutorError>() {
                        Some(ExecutorError::NoAcceptError(_, _)) => return Err(e),
                        _ => fail!(ExecutorError::TrExecutorError(e.to_string())),
                    }
                }
            }
        };
        let mut out_msgs = vec![];
        let need_bounce;
        description.compute_ph = compute_ph;
        description.action = match &description.compute_ph {
            TrComputePhase::Vm(phase) => {
                tr.add_fee_coins(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: success");
                    log::debug!(target: "executor", "action_phase: lt={}", lt);
                    match self.action_phase(
                        &mut tr,
                        account,
                        &original_acc_balance,
                        &mut acc_balance,
                        &mut msg_balance,
                        &phase.gas_fees,
                        actions.unwrap_or_default(),
                        new_data,
                        &account_address,
                        is_special
                    ) {
                        Ok(ActionPhaseResult{phase, messages, bounce}) => {
                            need_bounce = bounce;
                            out_msgs = messages;
                            Some(phase)
                        }
                        Err(e) => fail!(
                            ExecutorError::TrExecutorError(
                                format!("cannot create action phase of a new transaction for smart contract for reason {}", e)
                            )
                        )
                    }
                } else {
                    log::debug!(target: "executor", "compute_phase: failed");
                    need_bounce = true;
                    None
                }
            }
            TrComputePhase::Skipped(skipped) => {
                log::debug!(target: "executor", "compute_phase: skipped reason {:?}", skipped.reason);
                if is_ext_msg {
                    fail!(ExecutorError::ExtMsgComputeSkipped(skipped.reason))
                }
                need_bounce = true;
                None
            }
        };

        #[cfg(feature = "timings")]
        {
            self.timings[1].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        description.aborted = match description.action.as_ref() {
            Some(phase) => {
                log::debug!(
                    target: "executor",
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code
                );
                if AccStatusChange::Deleted == phase.status_change {
                    *account = Account::default();
                    description.destroyed = true;
                }
                !phase.success
            }
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            }
        };

        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        if description.aborted && !is_ext_msg && bounce && need_bounce {
            log::debug!(target: "executor", "bounce_phase");
            description.bounce = match self.bounce_phase(
                msg_balance.clone(),
                &mut acc_balance,
                &description.compute_ph,
                description.action.as_ref(),
                &in_msg,
                &mut tr,
                &account_address
            ) {
                Ok((bounce_ph, Some(bounce_msg))) => {
                    out_msgs.push(bounce_msg);
                    Some(bounce_ph)
                }
                Ok((bounce_ph, None)) => Some(bounce_ph),
                Err(e) => fail!(
                    ExecutorError::TrExecutorError(
                        format!("cannot create bounce phase of a new transaction for smart contract for reason {}", e)
                    )
                )
            };
            // TODO: check here
            // if money can be returned to sender
            // restore account balance - storage fee
            if let Some(TrBouncePhase::Ok(_)) = description.bounce {
                log::debug!(target: "executor", "restore balance {} => {}", acc_balance.coins, original_acc_balance.coins);
                acc_balance = original_acc_balance;
            }
        }
        if account.is_uninit() && acc_balance.is_zero()? {
            log::debug!(target: "executor", "delete uninitialized account with zero balance");
            *account = Account::default();
        } else if account.is_none() && !acc_balance.is_zero()? {
            // if tr.orig_status != ton_block::AccountStatus::AccStateNonexist {
            //     fail!("cannot delete account with non-zero balance")
            // } else {
            log::debug!(target: "executor", "balance is not zero, so make uninit account");
            *account = Account::uninit(account_address, acc_balance.clone(), 0, last_paid);
            // }
        }
        tr.set_end_status(account.status());
        if let Some(hash) = account.frozen_hash() {
            if account_id.contains_bytes(hash.as_slice()) {
                account.uninit_account();
            }
        }
        log::debug!(target: "executor", "set balance {}", acc_balance.coins);
        account.set_balance(acc_balance);
        log::debug!(target: "executor", "add messages {}, start_lt {}", out_msgs.len(), lt);
        let lt = self.add_messages(&mut tr, out_msgs, lt)?;
        log::debug!(target: "executor", "set end_lt {}", lt);
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::Ordinary(description))?;
        #[cfg(feature = "timings")]
        self.timings[2].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
        Ok(tr)
    }
    fn ordinary_transaction(&self) -> bool {
        true
    }
    fn config(&self) -> &BlockchainConfig {
        &self.config
    }
    fn build_stack(&self, in_msg: Option<&Message>, account: &Account) -> Result<Stack> {
        let mut stack = Stack::new();
        let in_msg = match in_msg {
            Some(in_msg) => in_msg,
            None => return Ok(stack),
        };
        let acc_balance = int!(account.balance().map_or(0, |value| value.coins.as_u128()));
        let msg_balance = int!(in_msg.get_value().map_or(0, |value| value.coins.as_u128()));
        let function_selector = boolean!(in_msg.is_inbound_external());
        let body_slice = in_msg.body().cloned().unwrap_or_default();
        let in_msg_cell = in_msg.serialize().unwrap_or_default();
        stack
            .push(acc_balance)
            .push(msg_balance)
            .push(StackItem::Cell(in_msg_cell))
            .push(StackItem::Slice(body_slice))
            .push(function_selector);
        Ok(stack)
    }
}
