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
    blockchain_config::BlockchainConfig, error::ExecutorError, ActionPhaseResult, ExecuteParams,
    TransactionExecutor,
};
use ton_block::{
    error, fail, Account, Cell, Coins, CurrencyCollection, Message, Result, Serializable,
    SliceData, TrComputePhase, Transaction, TransactionDescr, TransactionDescrTickTock,
    TransactionTickTock,
};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
    SmartContractInfo,
};

pub struct TickTockTransactionExecutor {
    config: BlockchainConfig,
    tt: TransactionTickTock,
}

impl TickTockTransactionExecutor {
    pub fn new(config: BlockchainConfig, tt: TransactionTickTock) -> Self {
        Self { config, tt }
    }
}

impl TransactionExecutor for TickTockTransactionExecutor {
    ///
    /// Create end execute tick or tock transaction for special account
    fn execute_with_params(
        &self,
        in_msg_cell: Option<Cell>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        if in_msg_cell.is_some() {
            fail!("Tick Tock transaction must not have input message")
        }
        let account_id = match account.get_id() {
            Some(addr) => addr.clone(),
            None => fail!("Tick Tock contract should have Standard address"),
        };
        match account.get_tick_tock() {
            Some(tt) => {
                if tt.tock != self.tt.is_tock() && tt.tick != self.tt.is_tick() {
                    fail!("wrong type of account's tick tock flag")
                }
            }
            None => fail!("Account {:x} is not special account for tick tock", account_id),
        }
        let account_address = account.get_addr().cloned().unwrap_or_default();
        log::debug!(target: "executor", "tick tock transation account {:x}", account_id);
        let mut acc_balance = account.balance().cloned().unwrap_or_default();

        let is_masterchain = true;
        let is_special = true;
        let lt = std::cmp::max(account.last_tr_time().unwrap_or_default(), params.last_tr_lt);
        let mut tr = self.create_transaction(account_id.clone());
        tr.orig_status = account.status();
        tr.set_logical_time(lt);
        tr.set_now(params.block_unixtime);
        account.set_last_paid(0);
        let storage = self
            .storage_phase(account, &mut acc_balance, &mut tr, is_masterchain, is_special)
            .map_err(|e| {
                error!(ExecutorError::TrExecutorError(format!(
                    "cannot create storage phase of a new transaction for \
                smart contract for reason {}",
                    e
                )))
            })?;
        let storage_fees_collected = storage.storage_fees_collected.as_u128();
        let mut description = TransactionDescrTickTock {
            tt: self.tt.clone(),
            storage,
            ..TransactionDescrTickTock::default()
        };

        let old_account = account.clone();
        let original_acc_balance = acc_balance.clone();

        let config_params = self.config().raw_config().clone();
        let mut smc_info = SmartContractInfo {
            myself: SliceData::load_builder(
                account_address.write_to_new_cell().unwrap_or_default(),
            )?,
            block_lt: params.block_lt,
            trans_lt: lt,
            unix_time: params.block_unixtime,
            balance: acc_balance.clone(),
            config_params,
            storage_fees_collected,
            due_payment: account.due_payment().map_or(0, Coins::as_u128),
            prev_blocks_info: params.prev_blocks_info.clone(),
            ..Default::default()
        };
        smc_info.calc_rand_seed(
            params.seed_block.clone(),
            &account_address.address().get_bytestring(0),
        );
        let mut stack = Stack::new();
        stack
            .push(int!(account.balance().map_or(0, |value| value.coins.as_u128())))
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(
                account_id.get_bytestring(0),
            )))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        log::debug!(target: "executor", "compute_phase {}", lt);
        let (compute_ph, actions, new_data) = match self.compute_phase(
            None,
            account,
            &mut acc_balance,
            &CurrencyCollection::default(),
            smc_info,
            stack,
            is_masterchain,
            is_special,
            false,
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
        };
        let mut out_msgs = vec![];
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm success");
                    log::debug!(target: "executor", "action_phase {}", lt);
                    match self.action_phase(
                        &mut tr,
                        account,
                        &original_acc_balance,
                        &mut acc_balance,
                        &mut CurrencyCollection::default(),
                        &Coins::zero(),
                        actions.unwrap_or_default(),
                        new_data,
                        &account_address,
                        is_special,
                    ) {
                        Ok(ActionPhaseResult { phase, messages, .. }) => {
                            out_msgs = messages;
                            Some(phase)
                        }
                        Err(e) => fail!(ExecutorError::TrExecutorError(format!(
                            "cannot create action phase of a new transaction \
                                     for smart contract for reason {}",
                            e
                        ))),
                    }
                } else {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm failed");
                    None
                }
            }
            TrComputePhase::Skipped(ref skipped) => {
                log::debug!(target: "executor", 
                    "compute_phase: skipped: reason {:?}", skipped.reason);
                None
            }
        };

        description.aborted = match &description.action {
            Some(phase) => {
                log::debug!(target: "executor", 
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code);
                !phase.success
            }
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            }
        };

        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        tr.set_end_status(account.status());
        account.set_balance(acc_balance);
        if description.aborted {
            *account = old_account;
        }
        let lt = self.add_messages(&mut tr, out_msgs, params.last_tr_lt)?;
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::TickTock(description))?;
        Ok(tr)
    }
    fn ordinary_transaction(&self) -> bool {
        false
    }
    fn config(&self) -> &BlockchainConfig {
        &self.config
    }
    fn build_stack(&self, _in_msg: Option<&Message>, account: &Account) -> Result<Stack> {
        let account_balance =
            account.balance().ok_or_else(|| error!("Can't get account balance."))?.coins.as_u128();
        let account_id = account.get_id().ok_or_else(|| error!("Can't get account id."))?;
        let mut stack = Stack::new();
        stack
            .push(int!(account_balance))
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(
                account_id.get_bytestring(0),
            )))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        Ok(stack)
    }
}

#[cfg(test)]
#[path = "tests/test_tick_tock_transaction.rs"]
mod tests;
