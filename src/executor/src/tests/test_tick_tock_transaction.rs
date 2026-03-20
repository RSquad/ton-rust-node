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
#![allow(clippy::field_reassign_with_default)]

use super::*;

mod common;
use common::*;
use pretty_assertions::assert_eq;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    accounts::{Account, AccountStorage, StorageInfo},
    messages::{CommonMsgInfo, InternalMessageHeader, Message, MsgAddressInt},
    out_actions::{OutAction, OutActions, SENDMSG_ORDINARY},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrStoragePhase,
        Transaction, TransactionDescr, TransactionDescrTickTock, TransactionTickTock,
    },
    types::Coins,
    AccountId, BuilderData, Cell, CurrencyCollection, GetRepresentationHash, Serializable,
    StateInit, StorageUsed, TickTock, DICT_HASH_MIN_CELLS,
};
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

fn create_test_data() -> Cell {
    let (msg1, msg2) = create_two_internal_messages();

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    b.into_cell().unwrap()
}

fn create_two_internal_messages() -> (Message, Message) {
    let msg1 = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        MSG1_BALANCE,
        false,
        BLOCK_LT + 2,
    );
    let msg2 = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        MSG2_BALANCE,
        true,
        BLOCK_LT + 3,
    );
    (msg1, msg2)
}

fn create_int_msg(src: AccountId, dest: AccountId, value: u64, bounce: bool, lt: u64) -> Message {
    let cell = dest.serialize().unwrap();
    let builder = BuilderData::with_raw_and_refs(vec![1, 2, 3, 4], 15, [cell]).unwrap();
    let body = SliceData::load_builder(builder).unwrap();
    let balance = CurrencyCollection::with_coins(value);
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, src).unwrap(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
        balance,
    );
    hdr.bounce = bounce;
    hdr.created_lt = lt;
    hdr.created_at = 0;
    Message::with_int_header_and_body(hdr, body)
}

fn create_test_code() -> Cell {
    let code = "
    ACCEPT
    PUSHROOT
    CTOS
    LDREF
    PLDREF
    PUSHINT 0
    SENDRAWMSG
    PUSHINT 0
    SENDRAWMSG
    ";

    compile_code_to_cell(code).unwrap()
}

fn create_test_account(amount: u64, address: AccountId, code: Cell, data: Cell) -> Account {
    let mut state = StateInit::default();
    state.set_special(TickTock::with_values(true, false));
    let mut account = Account::with_storage(
        &MsgAddressInt::with_standart(None, -1, address).unwrap(),
        &StorageInfo::with_values(ACCOUNT_UT, None),
        &AccountStorage::active(0, CurrencyCollection::with_coins(amount), state),
    );
    account.set_code(code);
    account.set_data(data);
    account.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    account
}

#[test]
fn test_tick_tock_executor_active_acc_with_code1() {
    let used = 1307; //gas units
    let storage_fees = 0;
    let msg_mine_fee = 0;
    let msg_fwd_fee = 0;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = 0;

    let acc_id = THIRD_ACCOUNT.clone();
    let start_balance = 2000000000;
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), create_test_code(), create_test_data());
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let mut new_acc =
        create_test_account(start_balance, acc_id.clone(), create_test_code(), create_test_data());
    new_acc.set_last_tr_time(BLOCK_LT + 1);

    let params = execute_params_none();
    let trans = execute_with_params(SIMPLE_MC_STATE.to_owned(), None, &mut acc, &params).unwrap();

    let mut good_trans = Transaction::with_address_and_status(acc_id, acc.status());
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    let hash = actions.hash().unwrap();
    msg1.value_mut().unwrap().coins = Coins::from(MSG1_BALANCE - msg_fwd_fee);
    msg2.value_mut().unwrap().coins = Coins::from(MSG2_BALANCE - msg_fwd_fee);
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }

    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees((storage_fees + gas_fees + msg_mine_fee * 2).into());

    let mut description = TransactionDescrTickTock::default();
    description.storage = TrStoragePhase::with_params(0.into(), None, AccStatusChange::Unchanged);

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 10000000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees(&(2 * msg_fwd_fee).into());
    action_ph.add_action_fees(&(2 * msg_mine_fee).into());
    action_ph.action_list_hash = hash;
    action_ph.tot_msg_size = StorageUsed::with_values_checked(4, 1904).unwrap();

    description.action = Some(action_ph);
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::TickTock(description)).unwrap();
    compare_transaction(&trans, &good_trans);
}
/*
fn create_wallet_data() -> Cell {
    //test public key
    BuilderData::with_raw(vec![0x00; 32], 256).unwrap().into()
}

fn create_wallet_code() -> Cell {
    let code = "
    ; s1 - body slice
    IFNOTRET
    ACCEPT
    BLOCKLT
    LTIME
    INC         ; increase logical time by 1
    PUSH s2     ; body to top
    PUSHINT 96  ; internal header in body, cut unixtime and lt
    SDSKIPLAST

    NEWC
    STSLICE
    STU 64         ; store tr lt
    STU 32         ; store unixtime
    STSLICECONST 0 ; no init
    STSLICECONST 0 ; body (Either X)
    ENDC
    PUSHINT 0
    SENDRAWMSG
    ";
    compile_code_to_cell(code).unwrap()
}

#[test]
fn test_light_wallet_contract() {
    let contract_code = create_wallet_code();
    let contract_data = create_wallet_data();
    let acc1_id = SENDER_ACCOUNT.clone();
    let acc2_id = RECEIVER_ACCOUNT.clone();

    let gas_used1 = 1387;
    let gas_fee1 = gas_used1 * 10000;
    let gas_fee2 = 1000000; // flat_gas_price
    let start_balance1 = 1000000000;
    let start_balance2 = 500000000;
    let fwd_fee = 10000000;
    let storage_fee1 = 138234403;
    let storage_fee2 = 138234403; // TODO: check here!!!

    let acc1 = create_test_account(start_balance1.clone(), acc1_id.clone(), contract_code.clone(), contract_data.clone());
    let mut shard_acc1 = Some(ShardAccount::with_params(acc1.clone(), UInt256::default(), 0).unwrap());
    let acc2 = create_test_account(start_balance2, acc2_id.clone(), contract_code.clone(), contract_data.clone());
    let mut shard_acc2 = Some(ShardAccount::with_params(acc2, UInt256::default(), 0).unwrap());

    let config = BLOCKCHAIN_CONFIG.to_owned();

    let transfer = 100000000;
    let lt = BLOCK_LT + 1;

    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let trans = executor.execute(
        &mut shard_acc1, BLOCK_UT, BLOCK_LT, lt, true
    ).unwrap();
    let msg = trans.get_out_msg(0).unwrap();
    println!("{:?}", msg);
    //new acc.balance = acc.balance - in_fwd_fee - transfer_value - storage_fees - gas_fee
    //transfer_value is reduced by fwd_fees:
    //new transfer_value = transfer_value - msg.fwd.fee
    let newbalance1 = start_balance1 - fwd_fee - transfer - storage_fee1 - gas_fee1;
    assert_eq!(shard_acc1.clone().unwrap().read_account().unwrap().balance().unwrap().clone(), CurrencyCollection::with_coins(newbalance1));
    assert_ne!(shard_acc1.clone().unwrap().last_trans_lt(), 0);
    assert_ne!(shard_acc1.unwrap().last_trans_hash(), &UInt256::default());

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let _trans = executor.execute(&mut shard_acc2, BLOCK_UT, BLOCK_LT, lt, true).unwrap();

    //new acc.balance = acc.balance + transfer_value - fwd_fee - storage_fee - gas_fee
    let newbalance2 = start_balance2 + transfer - fwd_fee - storage_fee2 - gas_fee2;
    assert_eq!(shard_acc2.clone().unwrap().read_account().unwrap().balance().unwrap().clone(), CurrencyCollection::with_coins(newbalance2));
    assert_ne!(shard_acc2.clone().unwrap().last_trans_lt(), 0);
    assert_ne!(shard_acc2.unwrap().last_trans_hash(), &UInt256::default());

}

fn test_transfer_code(mode: u8, ending: &str) -> Cell {
    let code = format!("
        PUSHCONT {{
            ACCEPT
            NEWC        ; create builder
            STSLICE     ; store internal msg slice into builder (next in stack - internal message body like a slice)
            ENDC        ; finish cell creating
            PUSHINT {x}
            SENDRAWMSG  ; send message with created cell as a root
            {e}
        }}
        IF ; top-of-stack value is function selector, it is non zero - message is external
    ",
    x = mode,
    e = ending
    );

    compile_code_to_cell(&code).unwrap()
}

fn create_test_transfer_account(amount: u64, mode: u8) -> Account {
    create_test_transfer_account_with_ending(amount, mode, "")
}

fn create_test_transfer_account_with_ending(amount: u64, mode: u8, ending: &str) -> Account {
    let acc_id = SENDER_ACCOUNT.clone();

    let mut state = StateInit::default();
    state.set_code(test_transfer_code(mode, ending));
    Account::with_storage(
        &MsgAddressInt::with_standart(
            None,
            -1,
            acc_id
        ).unwrap(),
        &StorageInfo::with_values(
            ACCOUNT_UT,
            None,
        ),
        &AccountStorage {
            last_trans_lt: 0,
            balance: CurrencyCollection::with_coins(amount),
            state: AccountState::with_state(state),
        }
    )
}

fn create_test_external_msg_with_int(transfer_value: u64) -> Message {
    let acc_id = SENDER_ACCOUNT.clone();
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap();
    hdr.import_fee = Coins::zero();
    let mut msg = Message::with_ext_in_header(hdr);

    let int_msg = create_int_msg(
        acc_id.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer_value,
        false,
        BLOCK_LT + 2
    );
    msg.set_body(int_msg.serialize().unwrap().into());

    msg
}

#[test]
fn test_trexecutor_active_acc_with_code2() {
    let start_balance = 2000000000;
    let gas_used = 1170;
    let gas_fees = gas_used * 10000;
    let transfer = 50000000;
    let storage_fee = 78924597;
    let msg_mine_fee = 3333282;
    let msg_fwd_fee = 10000000;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;

    let acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);
    let old_acc = ShardAccount::with_params(acc.clone(), UInt256::default(), 0).unwrap();
    let config = BLOCKCHAIN_CONFIG.to_owned();
    let mut new_acc = create_test_transfer_account(
        start_balance - (msg_fwd_fee + transfer + storage_fee + gas_fees), 0);
    let msg = create_test_external_msg_with_int(transfer);
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt);

    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let mut shard_acc = Some(old_acc.clone());
    let trans = executor.execute(
        &mut shard_acc, BLOCK_UT, BLOCK_LT, tr_lt, true
    ).unwrap();
    //println!("{:#?}", trans.read_description().unwrap());

    let mut good_trans = Transaction::with_account_and_message(&old_acc.read_account().unwrap(), &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let msg1 = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer,
        false,
        BLOCK_LT + 2
    );
    let mut msg1_new_value = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer - msg_fwd_fee,
        false,
        BLOCK_LT + 2
    );
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1_new_value.header_mut() {
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
    }

    good_trans.add_out_message(&msg1_new_value.clone()).unwrap();
    good_trans.set_total_fees((msg_fwd_fee + storage_fee + gas_fees + msg_mine_fee).into());

    let old = old_acc.read_account().unwrap().serialize().unwrap());
    let new = new_acc.serialize().unwrap());

    let mut description = TransactionDescrTickTock::default();
    description.storage_ph = TrStoragePhase::with_params(storage_fee.into(), None, AccStatusChange::Unchanged);

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_fees.into();
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&msg_fwd_fee.into());
    action_ph.add_action_fees(&msg_mine_fee.into());
    append_message(&mut action_ph.tot_msg_size, &msg1_new_value).unwrap();

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1));
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::TickTock(description)).unwrap();
    compare_transaction(&trans, &good_trans);

    // TODO: New fields in StorageInfo were added, so now worck incorrect
    //assert_eq!(shard_acc.unwrap().read_account().unwrap(), new_acc);
}

#[test]
fn test_trexecutor_active_acc_credit_first_false() {
    let start_balance = 1000000000;
    let acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::default(), 0).unwrap());
    let lt = BLOCK_LT + 1;

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let trans = executor.execute(&mut shard_acc, BLOCK_UT, BLOCK_LT, lt, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_credit_first().unwrap(), false);
}

#[test]
fn test_trexecutor_active_acc_with_zero_balance() {
    let start_balance = 0;
    let acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);
    let transfer = 1000000000;
    let storage_fee = 76796891;
    let gas_fee = 1000000; // flat_gas_price

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::default(), 0).unwrap());
    let lt = BLOCK_LT + 1;

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let trans = executor.execute(&mut shard_acc, BLOCK_UT, BLOCK_LT, lt, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
    assert_eq!(
        shard_acc.unwrap().read_account().unwrap().balance().unwrap(),
        &CurrencyCollection::with_coins(transfer - storage_fee - gas_fee));
}

//contract send all its balance to another account using special mode in SENDRAWMSG.
//contract balance must equal to zero after transaction.
fn active_acc_send_all_balance(ending: &str) {
    let start_balance = 10_000_000_000; //10 coins
    let acc = create_test_transfer_account_with_ending(start_balance, SENDMSG_ALL_BALANCE, ending);

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::from(SENDER_ACCOUNT.clone().get_bytestring(0)), 0).unwrap());
    let lt = BLOCK_LT + 1;

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = TickTockTransactionExecutor::new(config, TransactionTickTock::Tick);
    let trans = executor.execute(&mut shard_acc, BLOCK_UT, BLOCK_LT, lt, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
    assert_eq!(shard_acc.unwrap().read_account().unwrap().balance().unwrap(), &CurrencyCollection::with_coins(0));
    assert!(trans.get_out_msg(0).unwrap().is_some());
    assert!(trans.get_out_msg(1).unwrap().is_none());
}

#[test]
fn test_trexecutor_active_acc_send_all_balance() {
    active_acc_send_all_balance("");
}

#[test]
fn test_trexecutor_active_acc_send_all_balance_with_commit_and_throw() {
    active_acc_send_all_balance("COMMIT THROW 11");
}

#[test]
fn test_trexecutor_active_acc_send_all_balance_with_commit_and_secondmsg_with_throw() {
    active_acc_send_all_balance(
        "COMMIT
         NEWC
         STSLICECONST x1234_
         ENDC
         PUSHINT 10
         SENDRAWMSG
         THROW 11"
    );
}
*/
#[test]
fn test_build_ticktock_stack() {
    let acc_balance = 10_000_000;
    let acc_id = RECEIVER_ACCOUNT.clone();
    let account = create_test_account(acc_balance, acc_id, create_test_code(), create_test_data());

    let executor =
        TickTockTransactionExecutor::new(BLOCKCHAIN_CONFIG.clone(), TransactionTickTock::Tock);
    let test_stack1 = executor.build_stack(None, &account).unwrap();

    //stack for internal msg
    let mut etalon_stack1 = Stack::new();
    etalon_stack1
        .push(int!(10_000_000))
        .push(StackItem::integer(IntegerData::from_unsigned_bytes_be([0x22; 32])))
        .push(int!(-1))
        .push(int!(-2));

    assert_eq!(test_stack1, etalon_stack1);

    let executor =
        TickTockTransactionExecutor::new(BLOCKCHAIN_CONFIG.clone(), TransactionTickTock::Tick);
    let test_stack2 = executor.build_stack(None, &account).unwrap();

    //stack for external msg
    let mut etalon_stack2 = Stack::new();
    etalon_stack2
        .push(int!(10_000_000))
        .push(StackItem::integer(IntegerData::from_unsigned_bytes_be([0x22; 32])))
        .push(int!(0))
        .push(int!(-2));

    assert_eq!(test_stack2, etalon_stack2);
}
