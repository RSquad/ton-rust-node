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
use crate::blockchain_config::BlockchainConfig;
use pretty_assertions::assert_eq;
use std::io::{BufRead, BufReader};
use ton_block::{
    base64_decode, read_single_root_boc, Account, AccountStorage, Block, BocWriter, Cell,
    ConfigParams, CurrencyCollection, Deserializable, Message, MsgAddressInt, Result, Serializable,
    ShardAccount, StateInit, StorageInfo, TrComputePhase, Transaction, UnixTime,
};

mod common;

use common::*;

#[ignore]
#[test]
fn test_sample_replay_transaction() {
    let code = "code_boc_file_name_here";
    let data = "data_boc_file_name_here";
    let message = "msg_boc_file_name_here";
    let key_block = "key_block_boc_file_name_here";
    replay_contract_by_files(code, data, message, key_block).unwrap()
}

// account_code and account_data - filenames with bocs of code and data for account
// in_message - filename with boc of message
// key_block - filename with masterchain keyblock
fn replay_contract_by_files(
    account_code: &str,
    account_data: &str,
    in_message: &str,
    key_block: &str,
) -> Result<()> {
    let code = read_single_root_boc(std::fs::read(account_code)?)?;
    let data = read_single_root_boc(std::fs::read(account_data)?)?;
    let block = Block::construct_from_file(key_block)?;
    let mc_block_extra = block.read_extra()?.read_custom()?.expect("must be key block");
    let config = mc_block_extra.config().cloned().expect("must be in key block");
    let message = Message::construct_from_file(in_message)?;
    try_replay_contract_as_transaction(code, data, message, config)?;
    Ok(())
}

#[ignore]
#[test]
fn test_sample_replay_many_transactions() {
    let code = "code_boc_file_name_here";
    let data = "data_boc_file_name_here";
    let message = "msg_text_file_name_here";
    let key_block = "key_block_boc_file_name_here";
    many_replay_contract_by_files(code, data, message, key_block).unwrap()
}

// account_code and account_data - filenames with bocs of code and data for account
// in_message - filename with serialized messages as base64
// key_block - filename with masterchain keyblock
fn many_replay_contract_by_files(
    account_code: &str,
    account_data: &str,
    in_message: &str,
    key_block: &str,
) -> Result<()> {
    let code = read_single_root_boc(std::fs::read(account_code)?)?;
    let data = read_single_root_boc(std::fs::read(account_data)?)?;
    let cell = read_single_root_boc(std::fs::read(key_block)?)?;
    let block = Block::construct_from_cell(cell).unwrap();
    let mc_block_extra = block.read_extra()?.read_custom()?.expect("must be key block");
    let config = mc_block_extra.config().cloned().expect("must be in key block");
    let file = std::fs::File::open(in_message)?;
    let mut result = vec![];
    let mut idx = 1;
    for ln in BufReader::new(file).lines() {
        let contents = ln.unwrap();
        println!("Message no. #{} len: {}", idx, contents.len());
        if idx > 0 {
            let message = Message::construct_from_base64(&contents)?;
            let transaction = try_replay_contract_as_transaction(
                code.clone(),
                data.clone(),
                message,
                config.clone(),
            )?;
            let descr = transaction.read_description()?;
            let compute_ph = descr.compute_phase_ref().expect("no compute phase");
            match compute_ph {
                TrComputePhase::Vm(vm) => {
                    let steps = vm.vm_steps;
                    let gas = vm.gas_used;
                    result.push((idx, contents.len(), steps, gas));
                }
                _ => panic!("compute skipped"),
            }
        }
        idx += 1;
    }
    panic!("{:?}", result)
}

// creates account with code and data and address from message
// then executes with config params
fn try_replay_contract_as_transaction(
    code: Cell,
    data: Cell,
    message: Message,
    config: ConfigParams,
) -> Result<Transaction> {
    let at = UnixTime::now() as u32;
    let lt = 1_000_005;

    let mut state = StateInit::default();
    state.set_code(code);
    state.set_data(data);

    let account_id = message.int_dst_account_id().expect("must be external inbound message");
    let addr = MsgAddressInt::with_standart(None, 0, account_id.clone())?;
    let balance = CurrencyCollection::with_coins(100_000_000_000);

    let mut account = Account::with_storage(
        &addr,
        &StorageInfo::with_values(at - 100, None),
        &AccountStorage::active(0, balance, state),
    );
    account
        .calc_storage_stat_dict(
            config.size_limits_config().unwrap().acc_state_cells_for_storage_dict,
        )
        .unwrap();
    let params = common::execute_params_simple(lt, at);
    let config = BlockchainConfig::with_config(config).unwrap();
    try_replay_transaction(&mut account, Some(&message), config, &params)
}

#[ignore]
#[test]
fn run_validator_transaction() {
    let prefix = "../target/cmp/validator/0,2000000000000000,53544985";
    replay_with_mc_state_proof(
        &format!("{prefix}/mc_state_proof.boc"),
        &format!("{prefix}/account_old.boc"),
        &format!("{prefix}/account_new.boc"),
        &format!("{prefix}/transaction.boc"),
    )
}

#[test]
fn test_simple_transaction() {
    replay_transaction_by_files(
        "real_boc/simple_account_old.boc",
        "real_boc/simple_account_new.boc",
        "real_boc/simple_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_runvm_transaction() {
    replay_transaction_by_files(
        "real_boc/runvm_account_old.boc",
        "real_boc/runvm_account_new.boc",
        "real_boc/runvm_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_account_exceeds_size_after_action() {
    replay_transaction_by_files(
        "real_boc/size_exceeds_account_old.boc",
        "real_boc/size_exceeds_account_new.boc",
        "real_boc/size_exceeds_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_repack_msg_transaction() {
    replay_transaction_by_files(
        "real_boc/repack_msg_account_old.boc",
        "real_boc/repack_msg_account_new.boc",
        "real_boc/repack_msg_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_two_messages_transaction() {
    replay_transaction_by_files(
        "real_boc/two_messages_account_old.boc",
        "real_boc/two_messages_account_new.boc",
        "real_boc/two_messages_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_real_deploy_transaction() {
    replay_transaction_by_files(
        "real_boc/deploy_account_old.boc",
        "real_boc/deploy_account_new.boc",
        "real_boc/deploy_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_init_account_transaction() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/init_account_new.boc",
        "real_boc/init_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_execute_bounced_message() {
    replay_transaction_by_files(
        "real_boc/bounce_msg_account_old.boc",
        "real_boc/bounce_msg_account_new.boc",
        "real_boc/bounce_msg_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_execute_out_message_with_body_in_ref() {
    replay_transaction_by_files(
        "real_boc/msg_body_ref_account_old.boc",
        "real_boc/msg_body_ref_account_new.boc",
        "real_boc/msg_body_ref_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_execute_uninit_account() {
    replay_transaction_by_files(
        "real_boc/uninit_account_old.boc",
        "real_boc/uninit_account_new.boc",
        "real_boc/uninit_account_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_send_remainig_msg_balance() {
    replay_transaction_by_files(
        "real_boc/send_remainig_msg_balance_account_old.boc",
        "real_boc/send_remainig_msg_balance_account_new.boc",
        "real_boc/send_remainig_msg_balance_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_out_of_gas_transaction() {
    replay_transaction_by_files(
        "real_boc/out_of_gas_account_old.boc",
        "real_boc/out_of_gas_account_new.boc",
        "real_boc/out_of_gas_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_wrong_skip_reason() {
    replay_transaction_by_files(
        "real_boc/wrong_skip_reason_account_old.boc",
        "real_boc/wrong_skip_reason_account_new.boc",
        "real_boc/wrong_skip_reason_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_wrong_compute_phase() {
    replay_transaction_by_files(
        "real_boc/wrong_compute_phase_account_old.boc",
        "real_boc/wrong_compute_phase_account_new.boc",
        "real_boc/wrong_compute_phase_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_check_nofunds_to_send_message_without_error() {
    replay_transaction_by_files(
        "real_boc/nofunds_without_error_account_old.boc",
        "real_boc/nofunds_without_error_account_new.boc",
        "real_boc/nofunds_without_error_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_bounce_message_to_new_account() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/bounce_message_to_new_account_account_new.boc",
        "real_boc/bounce_message_to_new_account_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_out_of_gas_in_cmd() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/bounce_message_to_new_account_account_new.boc",
        "real_boc/bounce_message_to_new_account_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_freeze_account() {
    replay_transaction_by_files(
        "real_boc/freeze_account_old.boc",
        "real_boc/freeze_account_new.boc",
        "real_boc/freeze_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_send_to_frozen_account() {
    replay_transaction_by_files(
        "real_boc/send_to_frozen_account_old.boc",
        "real_boc/send_to_frozen_account_new.boc",
        "real_boc/send_to_frozen_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_unfreeze_account() {
    replay_transaction_by_files(
        "real_boc/unfreeze_account_old.boc",
        "real_boc/unfreeze_account_new.boc",
        "real_boc/unfreeze_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_bounce_to_empty_account() {
    replay_transaction_by_files(
        "real_boc/bounce_to_empty_account_old.boc",
        "real_boc/bounce_to_empty_account_new.boc",
        "real_boc/bounce_to_empty_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_bounce_to_low_balance_account() {
    replay_transaction_by_files(
        "real_boc/bounce_to_low_balance_account_old.boc",
        "real_boc/bounce_to_low_balance_account_new.boc",
        "real_boc/bounce_to_low_balance_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_depool_balance_check() {
    replay_transaction_by_files(
        "real_boc/depool_balance_check_account_old.boc",
        "real_boc/depool_balance_check_account_new.boc",
        "real_boc/depool_balance_check_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_no_balance_to_send_transaction() {
    replay_transaction_by_files(
        "real_boc/no_balance_to_send_account_old.boc",
        "real_boc/no_balance_to_send_account_new.boc",
        "real_boc/no_balance_to_send_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_int_message_to_elector_transaction() {
    replay_transaction_by_files(
        "real_boc/int_message_to_elector_account_old.boc",
        "real_boc/int_message_to_elector_account_new.boc",
        "real_boc/int_message_to_elector_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_int_message_to_elector2_transaction() {
    replay_transaction_by_files(
        "real_boc/int_message_to_elector2_account_old.boc",
        "real_boc/int_message_to_elector2_account_new.boc",
        "real_boc/int_message_to_elector2_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_tick_tock_message() {
    replay_transaction_by_files(
        "real_boc/tick_tock_acc_old.boc",
        "real_boc/tick_tock_acc_new.boc",
        "real_boc/tick_tock_tx.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_count_steps_vm() {
    replay_transaction_by_files(
        "real_boc/count_steps_acc_old.boc",
        "real_boc/count_steps_acc_new.boc",
        "real_boc/count_steps_tx.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_not_aborted_accepted_transaction() {
    replay_transaction_by_files(
        "real_boc/not_abort_accept_account_account_old.boc",
        "real_boc/not_abort_accept_account_account_new.boc",
        "real_boc/not_abort_accept_account_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_rawreserve_flag16_transaction() {
    replay_transaction_by_files(
        "real_boc/rawreserve_flag16_account_old.boc",
        "real_boc/rawreserve_flag16_account_new.boc",
        "real_boc/rawreserve_flag16_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_deploy_account_with_fixed_prefix() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/deploy_fixed_prefix_account_new.boc",
        "real_boc/deploy_fixed_prefix_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_storage_limit() {
    replay_transaction_by_files(
        "real_boc/storage_limit_old.boc",
        "real_boc/storage_limit_new.boc",
        "real_boc/storage_limit_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_state_init_cell_same_in_body() {
    replay_transaction_by_files(
        "real_boc/state_init_cell_same_in_body_account_old.boc",
        "real_boc/state_init_cell_same_in_body_account_new.boc",
        "real_boc/state_init_cell_same_in_body_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_body_with_pruned_cell() {
    replay_transaction_by_files(
        "real_boc/body_with_pruned_cell_account_old.boc",
        "real_boc/body_with_pruned_cell_account_new.boc",
        "real_boc/body_with_pruned_cell_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_out_of_gas_on_commit() {
    replay_transaction_by_files(
        "real_boc/out_of_gas_on_commit_account_old.boc",
        "real_boc/out_of_gas_on_commit_account_new.boc",
        "real_boc/out_of_gas_on_commit_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_init_wo_state() {
    replay_transaction_by_files(
        "real_boc/init_wo_state_account_old.boc",
        "real_boc/init_wo_state_account_new.boc",
        "real_boc/init_wo_state_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_bad_action_error() {
    replay_transaction_by_files(
        "real_boc/bad_action_account_old.boc",
        "real_boc/bad_action_account_new.boc",
        "real_boc/bad_action_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_bad_action_with_ignore_flag() {
    replay_transaction_by_files(
        "real_boc/bad_action_with_ignore_flag_account_old.boc",
        "real_boc/bad_action_with_ignore_flag_account_new.boc",
        "real_boc/bad_action_with_ignore_flag_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_size_limits_v12() {
    replay_transaction_by_files(
        "real_boc/size_limits_v12_account_old.boc",
        "real_boc/size_limits_v12_account_new.boc",
        "real_boc/size_limits_v12_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_due_payment_in_smc() {
    replay_transaction_by_files(
        "real_boc/due_payment_in_smc_account_old.boc",
        "real_boc/due_payment_in_smc_account_new.boc",
        "real_boc/due_payment_in_smc_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_fwd_fee_payment_in_smc() {
    replay_transaction_by_files(
        "real_boc/fwd_fee_payment_in_smc_account_old.boc",
        "real_boc/fwd_fee_payment_in_smc_account_new.boc",
        "real_boc/fwd_fee_payment_in_smc_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_raw_reserve_with_flag4() {
    replay_transaction_full(
        "real_boc/raw_reserve_with_flag4_account_old.boc",
        "real_boc/raw_reserve_with_flag4_account_new.boc",
        "real_boc/raw_reserve_with_flag4_transaction.boc",
        "real_boc/config.boc",
        "",
        "real_boc/libs.boc",
    )
}

#[test]
fn test_wrong_coins_serial_in_actions() {
    replay_transaction_by_files(
        "real_boc/wrong_coins_serial_in_actions_account_old.boc",
        "real_boc/wrong_coins_serial_in_actions_account_new.boc",
        "real_boc/wrong_coins_serial_in_actions_transaction.boc",
        "real_boc/config.boc",
    )
}

#[test]
fn test_msg_cell_fine_calc() {
    replay_transaction_by_files(
        "real_boc/msg_cell_fine_calc_account_old.boc",
        "real_boc/msg_cell_fine_calc_account_new.boc",
        "real_boc/msg_cell_fine_calc_transaction.boc",
        "real_boc/config.boc",
    )
}

// bounced message with state init and value was sent to new account
// account must be uninitialized if not enough value to compute
#[test]
fn test_state_init_nogas() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/state_init_nogas_abc17907_account_new.boc",
        "real_boc/state_init_nogas_abc17907_transaction.boc",
        "real_boc/config.boc",
    );
}

// not bounced message with state init and value was sent to new account
// account must be uninitialized if not enough value to compute
#[test]
fn test_non_bounce_with_state_init_nogas() {
    replay_transaction_by_files(
        "real_boc/empty_account.boc",
        "real_boc/non_bounce_with_state_init_nogas_account_new.boc",
        "real_boc/non_bounce_with_state_init_nogas_transaction.boc",
        "real_boc/config.boc",
    );
}

#[test]
fn test_pruned_cell_load_tx() {
    replay_transaction_by_files(
        "real_boc/pruned_cell_load_account_old.boc",
        "real_boc/pruned_cell_load_account_new.boc",
        "real_boc/pruned_cell_load_transaction.boc",
        "real_boc/config.boc",
    )
}

// account has anycast in address
#[test]
fn test_with_anycast() {
    replay_transaction_by_files(
        "real_boc/with_anycast_account_old.boc",
        "real_boc/with_anycast_account_new.boc",
        "real_boc/with_anycast_transaction.boc",
        "real_boc/config.boc",
    );
}

#[test]
fn test_with_bad_anycast() {
    replay_transaction_by_files(
        "real_boc/with_bad_anycast_account_old.boc",
        "real_boc/with_bad_anycast_account_new.boc",
        "real_boc/with_bad_anycast_transaction.boc",
        "real_boc/config.boc",
    );
}

#[test]
fn test_new_storage_prices() {
    replay_transaction_full(
        "real_boc/new_storage_prices_account_old.boc",
        "real_boc/new_storage_prices_account_new.boc",
        "real_boc/new_storage_prices_transaction.boc",
        "real_boc/config13.boc",
        "",
        "real_boc/libs.boc",
    )
}

#[test]
fn test_storage_fee_round() {
    replay_transaction_full(
        "real_boc/storage_fee_round_account_old.boc",
        "real_boc/storage_fee_round_account_new.boc",
        "real_boc/storage_fee_round_transaction.boc",
        "real_boc/config13.boc",
        "",
        "real_boc/libs.boc",
    )
}

#[test]
fn test_sendmsg_no_root_gas() {
    replay_transaction_by_files(
        "real_boc/sendmsg_no_root_gas_account_old.boc",
        "real_boc/sendmsg_no_root_gas_account_new.boc",
        "real_boc/sendmsg_no_root_gas_transaction.boc",
        "real_boc/config13.boc",
    )
}

#[test]
fn test_action_result_arg() {
    replay_transaction_by_files(
        "real_boc/action_result_arg_account_old.boc",
        "real_boc/action_result_arg_account_new.boc",
        "real_boc/action_result_arg_transaction.boc",
        "real_boc/config13.boc",
    )
}

#[ignore = "test for replay transaction by message from transaction"]
#[test]
fn test_replay_transaction_by_message_from_transaction() {
    let prefix = "real_boc/bad_";
    let transaction =
        Transaction::construct_from_file(prefix.to_owned() + "transaction.boc").unwrap();
    let mut hash_update = transaction.read_state_update().unwrap();
    let mut message = transaction.read_in_msg().unwrap().unwrap();
    message.int_header_mut().unwrap().bounce = true;
    let mut account = Account::construct_from_file(prefix.to_owned() + "account_old.boc").unwrap();
    let addr = account.get_addr().unwrap().clone();
    let rewrite_pfx = ton_block::SliceData::new(vec![0x01, 0x20, 0x80]);
    let anycast = ton_block::AnycastInfo::with_rewrite_pfx(rewrite_pfx).ok();
    let addr = MsgAddressInt::with_standart(anycast, 0, addr.address().clone()).unwrap();
    account.set_addr(addr);
    account.set_due_payment(Some(100.into()));
    account.write_to_file(prefix.to_owned() + "account_old.boc").unwrap();
    hash_update.old_hash = account.serialize().unwrap().repr_hash().clone();

    let at = transaction.now();
    let lt = transaction.logical_time();
    let params = common::execute_params_simple(lt, at);
    let config = ConfigParams::construct_from_file("real_boc/config.boc").unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();
    let mut transaction =
        try_replay_transaction(&mut account, Some(&message), config, &params).unwrap();
    account.calc_storage_stat_dict(1 << 31).unwrap();
    hash_update.new_hash = account.serialize().unwrap().repr_hash().clone();
    transaction.write_state_update(&hash_update).unwrap();
    transaction.write_to_file(prefix.to_owned() + "transaction.boc").unwrap();
    account.write_to_file(prefix.to_owned() + "account_new.boc").unwrap();
}

#[ignore = "test for replay transaction by message from file"]
#[test]
fn test_replay_transaction_by_message_from_file() {
    let mut account = Account::construct_from_file("real_boc/account_old.boc").unwrap();
    let message = Message::construct_from_file("real_boc/message.boc").unwrap();
    let config = read_config("real_boc/config.boc").unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();
    let at = match message.int_header() {
        Some(hdr) => hdr.created_at,
        None => UnixTime::now() as u32,
    };
    let lt = account.last_tr_time().unwrap() + 100;

    let params = common::execute_params_simple(lt, at);
    let tr = try_replay_transaction(&mut account, Some(&message), config, &params).unwrap();
    tr.write_to_file("real_boc/transaction_my.boc").unwrap();
    account.write_to_file("real_boc/account_new_my.boc").unwrap();
}

#[test]
fn test_reserve_value_message() {
    let mut account =
        Account::construct_from_file("real_boc/reserve_value_from_account.boc").unwrap();
    let message = Message::construct_from_file("real_boc/reserve_value_message.boc").unwrap();
    let config = ConfigParams::construct_from_file("real_boc/config.boc").unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();
    let (at, lt) = message.at_and_lt().unwrap();

    let params = common::execute_params_simple(lt, at);
    let our_transaction =
        try_replay_transaction(&mut account, Some(&message), config, &params).unwrap();
    assert_eq!(our_transaction.out_msgs.len().unwrap(), 1);
}

#[test]
fn test_revert_action_phase() {
    let mut account = Account::construct_from_file("real_boc/revert_action_account.boc").unwrap();
    let transaction =
        Transaction::construct_from_file("real_boc/revert_action_transaction.boc").unwrap();
    let message = transaction.read_in_msg().unwrap().unwrap();
    let config = ConfigParams::construct_from_file("real_boc/config.boc").unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();

    let lt: u64 = 241181000001;
    let at: u32 = 1626694478;
    let params = common::execute_params_simple(lt, at);

    let mut answer = account.clone();
    try_replay_transaction(&mut account, Some(&message), config, &params).unwrap();
    answer.set_data(account.get_data().unwrap());
    answer.set_balance(account.get_balance().unwrap().clone());
    answer.set_last_tr_time(account.last_tr_time().unwrap_or(0));
    assert_eq!(answer, account);
}

#[ignore]
#[test]
fn test_bad_single() {
    replay_transaction_by_files(
        "real_boc/bad_account_old.boc",
        "real_boc/bad_account_new.boc",
        "real_boc/bad_transaction.boc",
        "real_boc/config.boc",
    )
}

#[ignore]
#[test]
fn test_bad_trans() {
    let json = "../../emulator/emulator_test.json";
    let prefix = "real_boc/bad_".to_string();
    // let libs: std::path::PathBuf = std::path::PathBuf::from(json).parent().unwrap().join("libs.boc");
    // let libs = libs.to_string_lossy();
    let json = std::fs::read_to_string(json).unwrap();
    let json: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&json).unwrap();
    let acc = json["shard_account_boc"].as_str().unwrap();
    let tr = json["tx_boc"].as_str().unwrap();
    let prev = json["prev_blocks_info_boc"].as_str().unwrap();
    let libs = json["libs_boc"].as_str().unwrap();
    let cfg = json["config_params_boc"].as_str().unwrap();
    // let cfg = "real_boc/config.boc";
    let shard_acc = ShardAccount::construct_from_base64(acc).unwrap();
    BocWriter::with_root(&shard_acc.account_cell())
        .unwrap()
        .write_to_file(prefix.clone() + "account_old.boc")
        .unwrap();
    BocWriter::with_root(&shard_acc.account_cell())
        .unwrap()
        .write_to_file(prefix.clone() + "account_new.boc")
        .unwrap();
    std::fs::write(prefix.clone() + "transaction.boc", base64_decode(tr).unwrap()).unwrap();
    std::fs::write("real_boc/libs.boc", base64_decode(libs).unwrap()).unwrap();
    std::fs::write("real_boc/config13.boc", base64_decode(cfg).unwrap()).unwrap();

    replay_transaction_full(acc, &(prefix + "account_new.boc"), tr, cfg, prev, &libs);
}
