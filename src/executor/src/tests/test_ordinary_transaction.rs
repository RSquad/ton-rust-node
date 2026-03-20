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
#![allow(clippy::field_reassign_with_default)]
include!("../../../common/src/log.rs");

use super::*;
use crate::{
    EXTRA_FLAG_FULL_BODY_BOUNCE, EXTRA_FLAG_NEW_BOUNCE_FORMAT, RESULT_CODE_INCORRECT_SRC_ADDRESS,
};
use pretty_assertions::assert_eq;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    accounts::{Account, AccountStorage, StorageInfo},
    messages::{CommonMsgInfo, ExternalInboundMessageHeader, Message, MsgAddressInt},
    out_actions::{
        OutAction, OutActions, SENDMSG_ALL_BALANCE, SENDMSG_ORDINARY, SENDMSG_REMAINING_MSG_BALANCE,
    },
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    AccountId, AccountStatus, AnycastInfo, BouncedByPhase, BuilderData, Cell, ComputeSkipReason,
    ConfigParam8, ConfigParamEnum, ConfigParams, CurrencyCollection, Deserializable, ExceptionCode,
    GetRepresentationHash, GlobalVersion, Coins, IBitstring, InternalMessageHeader, MerkleProof,
    NewBounceBody, NewBounceComputePhaseInfo, NewBounceOriginalInfo, Serializable, SliceData,
    StateInit, StorageUsed, TrBouncePhaseNofunds, UInt256, DICT_HASH_MIN_CELLS,
    SENDMSG_PAY_FEE_SEPARATELY,
};
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn test_simple_transaction() {
    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 64
        SENDRAWMSG
        SWAP
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 100_000_000_000;
    let compute_phase_gas_fees = 1317000;
    let msg_income = 1_400_200_000;

    let acc_id = RECEIVER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    let mut b = BuilderData::default();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);

    let msg =
        create_int_msg_workchain(0, THIRD_ACCOUNT.clone(), acc_id, msg_income, false, BLOCK_LT - 2);

    let new_acc = Account::active(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_coins(99849728119),
        BLOCK_LT + 4,
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 2).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 271881;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));

    let gas_used = (compute_phase_gas_fees / 1000) as u32;
    let gas_fees = compute_phase_gas_fees;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 11;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.coins =
                Coins::from(MSG1_BALANCE + msg_income - compute_phase_gas_fees - MSG_FWD_FEE);
            int_header2.value.coins = Coins::from(MSG2_BALANCE - MSG_FWD_FEE);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 2;
    action_ph.total_fwd_fees = Some((2 * MSG_FWD_FEE).into());
    action_ph.total_action_fees = Some((2 * MSG_MINE_FEE).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg1).unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg2).unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(
        gas_fees + storage_phase_fees + MSG_MINE_FEE * 2,
    ));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    assert_eq!(acc, new_acc);
}

#[test]
fn test_trexecutor_with_bad_code() {
    let used = 748; //gas units
    let gas_fees = used as u64 * 10000;
    let storage_fees = 284115250;
    let total_fees = storage_fees + gas_fees;

    let acc_id = SENDER_ACCOUNT.clone();
    let start_balance = 2000000000;
    let bad_code = compile_code_to_cell(
        "
        ACCEPT
        NEWC
        ENDC
        CTOS
        LDREF
    ",
    )
    .unwrap();
    let mut acc = create_test_account(
        start_balance,
        acc_id.clone(),
        bad_code.clone(),
        create_two_messages_data(),
    );
    let msg = create_int_msg(acc_id.clone(), acc_id.clone(), 1000000, false, BLOCK_LT);
    let mut new_acc = create_test_account(
        Coins::from(start_balance) + msg.value().unwrap().coins - Coins::from(total_fees),
        acc_id,
        bad_code,
        create_two_messages_data(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let tr_lt = BLOCK_LT + 1;

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let mut trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 0).unwrap();
    assert_eq!(acc, new_acc);

    good_trans.set_total_fees(total_fees.into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));
    description.credit_ph =
        Some(TrCreditPhase { due_fees_collected: None, credit: msg.value().unwrap().clone() });

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.exit_code = 9;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 100.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 6;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    trans.set_now(0);
    let description = TransactionDescr::Ordinary(description);
    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_trexecutor_with_code_without_accept() {
    let acc_id = SENDER_ACCOUNT.clone();
    let start_balance = 2000000000;
    let msg = create_test_external_msg();
    // balance - (balance of 2 output messages + storage_fee + gas_fee)

    let tr_lt = BLOCK_LT + 1;
    // enough gas for smartcontract - normal termination, but NoAccept error
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(0, None));

    // enough gas for smartcontract - but exception during run and NoAccept error
    let code = compile_code_to_cell("CTOS").unwrap();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(
        err.downcast::<ExecutorError>().unwrap(),
        ExecutorError::NoAcceptError(7, Some(ton_vm::int!(0)))
    );

    // not enough gas for smartcontract - OutOfGas Exception and NoAccept error
    let code = compile_code_to_cell("AGAINEND NEWC ENDC DROP").unwrap();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(
        err.downcast::<ExecutorError>().unwrap(),
        ExecutorError::NoAcceptError(-14, Some(ton_vm::int!(10057)))
    );

    // Due to ACCEPT, the transaction will be completed
    let code = compile_code_to_cell("ACCEPT AGAINEND NEWC ENDC DROP").unwrap();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    execute(&msg, &mut acc, tr_lt).unwrap();

    // not exist code - transaction will be aborted
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id, code, create_two_messages_data());
    acc.state_init_mut().unwrap().code = None;
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(-13, None));

    // not exist code but special account - transaction will be aborted
    let acc_id = AccountId::from([0x66; 32]);
    assert!(BLOCKCHAIN_CONFIG.is_special_account(true, &acc_id).unwrap());
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());

    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id).unwrap();
    hdr.import_fee = Coins::zero();
    let mut msg_copy = Message::with_ext_in_header(hdr);
    msg_copy.set_body(SliceData::default());

    acc.state_init_mut().unwrap().code = None;
    let err = execute(&msg_copy, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(-13, None));
}

#[test]
fn test_trexecutor_with_no_funds_for_storage() {
    let import_fee = 10000000;
    let balance = 1;
    let start_balance = import_fee + balance; // to pay for import external message and not enough to pay storage
    let last_paid = BLOCK_UT - 100;
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(
        start_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    acc.set_last_paid(last_paid);
    let msg = create_test_external_msg();
    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_due_payment(Some(6605.into()));

    execute(&msg, &mut acc, BLOCK_LT + 1).expect_err("no funds for external message");
    assert_eq!(acc, new_acc);
}

#[test]
fn test_trexecutor_active_acc_with_code1_not_enough_balance() {
    let acc_id = SENDER_ACCOUNT.clone();
    let start_balance = 9000000;
    let mut acc = create_test_account(
        start_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let msg = create_test_external_msg();
    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, start_balance, 0);
    assert!(trans.is_err());
}

#[test]
fn test_trexecutor_active_acc_with_code1() {
    let used = 1307; //gas units
    let storage_fees = 288370662;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let gas_fees = used as u64 * 10000;
    let gas_credit = 10000;

    let acc_id = SENDER_ACCOUNT.clone();
    let start_balance = 2000000000;
    let mut acc = create_test_account(
        start_balance,
        acc_id.clone(),
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance - (150000000 + MSG_FWD_FEE + storage_fees + gas_fees);
    let mut new_acc = create_test_account(
        end_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let msg = create_test_external_msg();
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 3);

    let trans = execute_c(&msg, &mut acc, tr_lt, end_balance, 2).unwrap();
    new_acc.set_last_paid(acc.last_paid());

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.value.coins = Coins::from(MSG1_BALANCE - MSG_FWD_FEE);
            int_header2.value.coins = Coins::from(MSG2_BALANCE - MSG_FWD_FEE);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    let msg1 = msg1;
    let msg2 = msg2;
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees((MSG_FWD_FEE + storage_fees + gas_fees + MSG_MINE_FEE * 2).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(gas_credit.into());
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees(&(2 * MSG_FWD_FEE).into());
    action_ph.add_action_fees(&(2 * MSG_MINE_FEE).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg1).unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg2).unwrap();

    description.action = Some(action_ph);
    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();
    compare_transaction(&trans, &good_trans);
    assert_eq!(acc, new_acc);
}

fn create_transfer_ext_msg(src: AccountId, dest: AccountId, value: u64) -> Message {
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, src.clone()).unwrap();
    hdr.import_fee = Coins::zero();

    let int_msg = create_int_msg(src, dest, value, true, 0);
    let int_header = match int_msg.withdraw_header() {
        CommonMsgInfo::IntMsgInfo(int_hdr) => int_hdr,
        _ => panic!("must be internal message header"),
    };

    let mut msg = Message::with_ext_in_header(hdr);
    msg.set_body(SliceData::load_builder(int_header.write_to_new_cell().unwrap()).unwrap());
    msg
}

#[test]
fn test_light_wallet_contract() {
    let contract_data = BuilderData::with_raw(vec![0x00; 32], 256).unwrap().into_cell().unwrap();
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
    let contract_code = compile_code_to_cell(code).unwrap();
    let acc1_id = SENDER_ACCOUNT.clone();
    let acc2_id = RECEIVER_ACCOUNT.clone();

    let gas_used1 = 1387;
    let gas_fee1 = gas_used1 * 10000;
    let gas_fee2 = 1000000; // flat_gas_price
    let start_balance1 = u64::MAX / 4;
    let start_balance2 = u64::MAX / 4;
    let fwd_fee = MSG_FWD_FEE;
    let storage_fee1 = 140362109;
    let storage_fee2 = 140362109;

    let mut acc1 = create_test_account(
        start_balance1,
        acc1_id.clone(),
        contract_code.clone(),
        contract_data.clone(),
    );
    let mut acc2 =
        create_test_account(start_balance2, acc2_id.clone(), contract_code, contract_data);

    let transfer = u64::MAX / 8;
    //new acc.balance = acc.balance - in_fwd_fee - transfer_value - storage_fees - gas_fee
    let newbalance1 = start_balance1 - fwd_fee - transfer - storage_fee1 - gas_fee1;
    let in_msg = create_transfer_ext_msg(acc1_id, acc2_id, transfer);
    let trans = execute_c(&in_msg, &mut acc1, BLOCK_LT + 1, newbalance1, 1).unwrap();
    let msg = trans.get_out_msg(0).unwrap();

    //new acc.balance = acc.balance + transfer_value - fwd_fee - storage_fee - gas_fee
    let newbalance2 = start_balance2 + transfer - fwd_fee - storage_fee2 - gas_fee2;
    let _trans = execute_c(&msg.unwrap(), &mut acc2, BLOCK_LT + 1, newbalance2, 0).unwrap();
}

fn test_transfer_code(mode: u8, ending: &str) -> Cell {
    let code = format!("
        PUSHCONT {{
            ACCEPT
            DUP
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
    let mut acc = Account::with_storage(
        &MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        &StorageInfo::with_values(ACCOUNT_UT, None),
        &AccountStorage::active(0, CurrencyCollection::with_coins(amount), StateInit::default()),
    );
    acc.set_code(test_transfer_code(mode, ending));
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    acc
}

fn create_test_external_msg_with_int_ex(
    workchain_id: i8,
    src: AccountId,
    dest: AccountId,
    transfer_value: u64,
) -> Message {
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, workchain_id, src.clone()).unwrap();
    hdr.import_fee = Coins::zero();
    let mut msg = Message::with_ext_in_header(hdr);

    let int_msg =
        create_int_msg_workchain(workchain_id, src, dest, transfer_value, false, BLOCK_LT + 2);
    msg.set_body(SliceData::load_builder(int_msg.write_to_new_cell().unwrap()).unwrap());

    msg
}

fn create_test_external_msg_with_int(transfer_value: u64) -> Message {
    create_test_external_msg_with_int_ex(
        -1,
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer_value,
    )
}

#[test]
fn test_trexecutor_active_acc_with_code2() {
    let start_balance = 2000000000;
    let gas_used = 1188;
    let gas_fees = gas_used as u64 * 10000;
    let transfer = 50000000;
    let storage_fee = 79456523;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;

    let mut acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);
    let mut new_acc = create_test_transfer_account(
        start_balance - (MSG_FWD_FEE + transfer + storage_fee + gas_fees),
        0,
    );
    let msg = create_test_external_msg_with_int(transfer);
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 2);

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 1).unwrap();
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    new_acc.set_data(acc.get_data().unwrap());
    new_acc.set_last_paid(acc.last_paid());
    new_acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let msg1 = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer,
        false,
        BLOCK_LT + 2,
    );
    let mut msg1_new_value = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer - MSG_FWD_FEE,
        false,
        BLOCK_LT + 2,
    );
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1_new_value.header_mut() {
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
    }

    good_trans.add_out_message(&msg1_new_value).unwrap();
    good_trans.set_total_fees((MSG_FWD_FEE + storage_fee + gas_fees + MSG_MINE_FEE).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(storage_fee.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 11;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.add_action_fees(&MSG_MINE_FEE.into());
    append_message(&mut action_ph.tot_msg_size, &msg1_new_value).unwrap();

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1));
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);
    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();
    compare_transaction(&trans, &good_trans);
    assert_eq!(acc, new_acc);
}

#[test]
fn test_trexecutor_active_acc_credit_first_false() {
    let start_balance = 1000000000;
    let transfer = 50000000;
    let storage_fee = 79456523;
    let gas_fee = 1000000; // flat_gas_price
    let result_account_balance = start_balance + transfer - storage_fee - gas_fee;

    let acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), transfer, true, 0);

    execute_acc_with_message(acc, &msg)
        .expect_balance(result_account_balance)
        .expect_count_out_msgs(0)
        .expect_not_aborted()
        .expect_compute_result(0);
}

#[test]
fn test_trexecutor_active_acc_with_zero_balance() {
    let start_balance = 0;
    let transfer = 1000000000;
    let storage_fee = 77328817;
    let gas_fee = 1000000; // flat_gas_price
    let result_account_balance = start_balance + transfer - storage_fee - gas_fee;

    let acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), transfer, false, 0);

    execute_acc_with_message(acc, &msg)
        .expect_balance(result_account_balance)
        .expect_count_out_msgs(0)
        .expect_not_aborted()
        .expect_compute_result(0);
}

//contract send all its balance to another account using special mode in SENDRAWMSG.
//contract balance must equal to zero after transaction.
fn active_acc_send_all_balance(ending: &str) {
    let start_balance = 10_000_000_000; //10 coins
    let mut acc =
        create_test_transfer_account_with_ending(start_balance, SENDMSG_ALL_BALANCE, ending);

    let msg = create_test_external_msg_with_int(start_balance);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 0, 1).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans
        .read_description()
        .unwrap()
        .compute_phase_ref()
        .unwrap()
        .clone()
        .get_vmphase_mut()
        .unwrap()
        .success;
    assert_eq!(vm_phase_success, true);
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
         THROW 11",
    );
}

#[test]
fn test_trexecutor_active_acc_send_all_balance_then_extra_sendmsg() {
    // second message is sent after SENDRAWMSG with SENDMSG_ALL_BALANCE mode
    // and it will fail because not enough balance
    active_acc_send_all_balance(
        "
        NEWC
        STSLICE
        ENDC
        PUSHINT 2 ; ignore error
        SENDRAWMSG
    ",
    );
}

#[test]
fn test_skip_compute_phase_on_ext_msg() {
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        &1_000_000_000.into(),
    );

    let msg = create_test_external_msg_with_int(1_000_000_000);

    let err = execute(&msg, &mut acc, BLOCK_LT + 1).unwrap_err();
    assert_eq!(
        err.downcast::<ExecutorError>().unwrap(),
        ExecutorError::ExtMsgComputeSkipped(ComputeSkipReason::NoState)
    );
}

#[test]
fn test_build_ordinary_empty_stack() {
    let acc_balance = 10_000_000;
    let acc_id = RECEIVER_ACCOUNT.clone();
    let acc = create_test_account(
        acc_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());

    let test_stack1 = executor.build_stack(None, &acc).unwrap();
    assert_eq!(test_stack1, Stack::new());
}

#[test]
fn test_build_ordinary_stack() {
    let acc_balance = 10_000_000;
    let msg_balance = 15_000;
    let acc_id = RECEIVER_ACCOUNT.clone();
    let msg_int = create_int_msg(SENDER_ACCOUNT.clone(), acc_id.clone(), msg_balance, false, 0);
    let acc = create_test_account(
        acc_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());

    let test_stack1 = executor.build_stack(Some(&msg_int), &acc).unwrap();

    let body_slice1 = msg_int.body().cloned().unwrap_or_default();

    //stack for internal msg
    let mut ethalon_stack1 = Stack::new();
    ethalon_stack1
        .push(int!(acc_balance))
        .push(int!(msg_balance))
        .push(StackItem::Cell(msg_int.serialize().unwrap()))
        .push(StackItem::Slice(body_slice1))
        .push(int!(0));

    assert_eq!(test_stack1, ethalon_stack1);

    let msg_ext = create_test_external_msg();
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let test_stack2 = executor.build_stack(Some(&msg_ext), &acc).unwrap();

    let body_slice2 = msg_ext.body().cloned().unwrap_or_default();

    //stack for external msg
    let mut ethalon_stack2 = Stack::new();
    ethalon_stack2
        .push(int!(acc_balance))
        .push(int!(0))
        .push(StackItem::Cell(msg_ext.serialize().unwrap()))
        .push(StackItem::Slice(body_slice2))
        .push(int!(-1));

    assert_eq!(test_stack2, ethalon_stack2);
}

#[test]
fn test_drain_account() {
    let storage_fee = 286774882;
    let start_balance = 1; // not enough to pay storage
    let msg_income = 200000000; //  but enough not to freeze
    let total_balance = start_balance + msg_income;
    let due_payment = storage_fee - total_balance;
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(
        start_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    // send message to zero account but not freeze
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        false,
        BLOCK_LT + 2,
    );
    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 4);
    new_acc.set_balance(CurrencyCollection::default());
    new_acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    new_acc.set_due_payment(Some(86774881.into()));

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 0).unwrap();
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();

    assert_eq!(acc, new_acc);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: total_balance.into(), // collect full balance as fee
        storage_fees_due: Some(Coins::from(due_payment)), // also due_payment credit for next transaction
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_coins(total_balance));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

// send money to account without money and send all remaining balance to another account
// account must not be freezen
#[test]
fn test_send_value_to_account_without_money_without_bounce() {
    let start_balance = 0; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000; // send enough money to pay storage fee and to run contract
    let storage_fee = 3492;
    let total_balance = 0;
    let gas_used = 591;
    let gas_fees = 5910000;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let total_fees = gas_fees + MSG_MINE_FEE + storage_fee;
    let remain_balance = msg_income - MSG_FWD_FEE - gas_fees - storage_fee;
    let mut out_msg = create_int_msg(SENDER_ACCOUNT.clone(), [0; 32].into(), 100, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 128
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        false,
        PREV_BLOCK_LT,
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 1).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 99999.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = remain_balance.into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let mut acc_balance = CurrencyCollection::with_coins(start_balance + msg_income - storage_fee);
    account.set_balance(acc_balance.clone());
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor
        .compute_phase(
            Some(&msg),
            &mut account,
            &mut acc_balance,
            &CurrencyCollection::with_coins(msg_income),
            smci,
            stack,
            msg.is_masterchain(),
            false,
            false,
            &ExecuteParams::default(),
        )
        .unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.add_action_fees(&MSG_MINE_FEE.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = out_msg;
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(total_fees));
    // good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_outmsg_ihr_fee() {
    let start_balance = 100000000000;
    let msg_income = 1_000_000_000;
    let storage_fee = 3528;
    let total_balance = 100984246372;
    let gas_used = 575;
    let gas_fees = 5750000;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let total_fees = gas_fees + MSG_MINE_FEE + storage_fee;
    let mut out_msg = create_int_msg(SENDER_ACCOUNT.clone(), [0; 32].into(), 100, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 1
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        false,
        PREV_BLOCK_LT,
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 1).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = CurrencyCollection::with_coins(100);
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.add_action_fees(&MSG_MINE_FEE.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = out_msg;
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_value_to_account_without_money_with_bounce() {
    let start_balance = 0; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000; // send enough money to pay storage fee and to run contract
    let storage_fee = 3480;
    let total_balance = 0;
    let gas_used = 591;
    let gas_fees = gas_used as u64 * 10000;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let total_fees = gas_fees + MSG_MINE_FEE;
    let remain_balance = msg_income - MSG_FWD_FEE - gas_fees; // by specs
    let mut out_msg = create_int_msg(SENDER_ACCOUNT.clone(), [0; 32].into(), 0, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 128
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_due_payment(Some(storage_fee.into()));
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 1).unwrap();

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Coins::default(),
        storage_fees_due: Some(storage_fee.into()),
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = remain_balance.into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let acc_balance =
        CurrencyCollection::with_coins(acc.balance().unwrap().coins.as_u128() as u64 + msg_income);
    account.set_balance(acc_balance);
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor
        .compute_phase(
            Some(&msg),
            &mut account.clone(),
            &mut account.balance().cloned().unwrap_or_default(),
            &msg.get_value().cloned().unwrap_or_default(),
            smci,
            stack,
            msg.is_masterchain(),
            false,
            false,
            &ExecuteParams::default(),
        )
        .unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.add_action_fees(&MSG_MINE_FEE.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = out_msg;
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_msg_value_to_account_without_money_with_bounce() {
    let start_balance = 3492; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000; // send enough money to run contract
    let total_balance = 0;
    let gas_used = 583;
    let gas_fees = gas_used as u64 * 10000;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
    let total_fees = start_balance + gas_fees + MSG_MINE_FEE;
    let remain_balance = msg_income - MSG_FWD_FEE - gas_fees; // by specs
    let mut out_msg = create_int_msg(SENDER_ACCOUNT.clone(), [0; 32].into(), 0, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 64
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 1).unwrap();
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Coins::from(start_balance),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = remain_balance.into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let acc_balance =
        CurrencyCollection::from_coins(acc.balance().unwrap().coins + msg_income as u128);
    account.set_balance(acc_balance);
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor
        .compute_phase(
            Some(&msg),
            &mut account.clone(),
            &mut account.balance().cloned().unwrap_or_default(),
            &msg.get_value().cloned().unwrap_or_default(),
            smci,
            stack,
            msg.is_masterchain(),
            false,
            false,
            &ExecuteParams::default(),
        )
        .unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.add_action_fees(&MSG_MINE_FEE.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = out_msg;
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage_and_freeze() {
    // 1. Bad code without accept
    // bouncable message value is not used to pay storage fee
    let code = "CTOS";
    let data = Cell::default();
    let start_balance = 100;
    let due = 105786786; // due payment too much to freeze account

    // account will be frozen
    // account balance will be used to pay storage fee
    // message value will be moved to account balance because it is not enough for bounce message
    let (acc, tr) =
        execute_custom_transaction(start_balance, code, data.clone(), 300, true, 300, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);

    // account will be frozen
    // account balance will be used to pay storage fee
    // bounced message will be sent
    let (acc, tr) =
        execute_custom_transaction(start_balance, code, data.clone(), due + 40_000_000, true, 0, 1);
    assert_eq!(get_tr_descr(&tr).compute_ph, TrComputePhase::skipped(ComputeSkipReason::NoState));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage() {
    // init_log_without_config(None, log::LevelFilter::Debug, None);
    // account balance is not enough to pay for storage fee
    // bouncable message value is not used to pay storage fee

    // i. Bad code without accept
    let code = "CTOS";
    let data = Cell::default();
    let due = 1000;
    let start_balance = 107382665 - due;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute, but not enough to pay for bounce value
    execute_transaction_case(start_balance, code, data.clone(), 1_000_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(1_000_100)
        .expect_compute_result(7)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and bounce value
    // all the message value will be bounced
    execute_transaction_case(start_balance, code, data.clone(), 12_000_000, true)
        .expect_balance(0)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(12_000_000)
        .expect_compute_result(7)
        .expect_bounce_success()
        .expect_status(AccountStatus::AccStateActive);

    // ii. Bad code with accept
    let code = "ACCEPT CTOS";
    let due = 1064853;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    // same as without accept
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute, but not enough to pay for bounce value
    // same as without accept
    execute_transaction_case(start_balance, code, data.clone(), 1_000_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(1_000_100)
        .expect_compute_result(7)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and to pay for bounce value
    // same as without accept
    execute_transaction_case(start_balance, code, data.clone(), 12_000_000, true)
        .expect_balance(0)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(12_000_000)
        .expect_compute_result(7)
        .expect_bounce_success()
        .expect_status(AccountStatus::AccStateActive);

    // iii. good code without accept
    let code = "NOP";
    let due = 1000;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    // same as with bad code
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and no need to bounce
    execute_transaction_case(start_balance, code, data.clone(), 1_000_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(1_000_100)
        .expect_compute_result(0)
        .expect_no_bounce()
        .expect_status(AccountStatus::AccStateActive);

    // // iv. good code with accept
    let code = "ACCEPT";
    let due = 532927;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    // same as without accept
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and no need to bounce
    // same as without accept
    execute_transaction_case(start_balance, code, data.clone(), 1_000_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(1_000_100)
        .expect_compute_result(0)
        .expect_no_bounce()
        .expect_status(AccountStatus::AccStateActive);

    // v. good code without accept and with sendmsg
    let code = "
        PUSHCTR C4
        PUSHINT 16
        SENDRAWMSG
    ";
    let out_msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        THIRD_ACCOUNT.clone(),
        11_000_000,
        false,
        BLOCK_LT + 1,
    );
    let data = out_msg.serialize().unwrap();
    let due = 1000;
    let start_balance = 154790616 - due;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    // the same
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute but not enough to send message and bounce
    execute_transaction_case(start_balance, code, data.clone(), 5_830_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(5_830_100)
        .expect_compute_result(0)
        .expect_action_failed(37)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute but not enough to send message, but enough to bounce
    execute_transaction_case(start_balance, code, data.clone(), 15_830_100, true)
        .expect_balance(0)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(15_830_100)
        .expect_compute_result(0)
        .expect_action_failed(37)
        .expect_bounce_success()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and to send message and no bounce
    execute_transaction_case(start_balance, code, data.clone(), 16_830_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(16_830_100)
        .expect_compute_result(0)
        .expect_action_success(1)
        .expect_no_bounce()
        .expect_status(AccountStatus::AccStateActive);

    // vi. good code with accept and with sendmsg
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 16
        SENDRAWMSG
    ";
    let out_msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        THIRD_ACCOUNT.clone(),
        11_000_000,
        false,
        BLOCK_LT + 1,
    );
    let data = out_msg.serialize().unwrap();
    let due = 1000;
    let start_balance = 155854469 - due;

    // message value is not enough to compute and to bounce so value will be moved to account balance
    // the same
    execute_transaction_case(start_balance, code, data.clone(), 100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(100)
        .expect_compute_skipped(ComputeSkipReason::NoGas)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute but not enough to send message and bounce
    execute_transaction_case(start_balance, code, data.clone(), 6_090_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(6_090_100)
        .expect_compute_result(0)
        .expect_action_failed(37)
        .expect_bounce_no_funds()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute but not enough to send message, but enough to bounce
    execute_transaction_case(start_balance, code, data.clone(), 16_090_100, true)
        .expect_balance(0)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(16_090_100)
        .expect_compute_result(0)
        .expect_action_failed(37)
        .expect_bounce_success()
        .expect_status(AccountStatus::AccStateActive);

    // message value is enough to compute and to send message and no bounce
    execute_transaction_case(start_balance, code, data.clone(), 17_090_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(1)
        .expect_storage_fees_due(due)
        .expect_credit(17_090_100)
        .expect_compute_result(0)
        .expect_action_success(1)
        .expect_no_bounce()
        .expect_status(AccountStatus::AccStateActive);
}

fn execute_sendrawmsg_message(
    mode: u8,
    send_value: u64,
    result_balance: u64,
    count_out_msgs: usize,
) {
    let code = format!(
        "
        ACCEPT
        PUSHCTR C4
        PUSHINT {}
        SENDRAWMSG
    ",
        mode
    );
    let out_msg =
        create_int_msg(SENDER_ACCOUNT.clone(), THIRD_ACCOUNT.clone(), send_value, false, BLOCK_LT);
    let data = out_msg.serialize().unwrap();
    let start_balance = 200_000_000;
    execute_custom_transaction(
        start_balance,
        &code,
        data.clone(),
        20_000_000,
        false,
        result_balance,
        count_out_msgs,
    );
    execute_custom_transaction(
        start_balance,
        &code,
        data,
        20_000_000,
        true,
        result_balance,
        count_out_msgs,
    );
}

#[test]
fn test_sendrawmsg_message() {
    execute_sendrawmsg_message(0, 14_000_000, 44_667_458, 1);
    execute_sendrawmsg_message(1, 14_000_000, 34_667_458, 1);
    execute_sendrawmsg_message(0, 0, 60_263_237, 0);
    execute_sendrawmsg_message(1, 0, 50_263_237, 1);

    execute_sendrawmsg_message(128, 14_000_000, 0, 1);
    execute_sendrawmsg_message(128 + 1, 14_000_000, 0, 1);
    execute_sendrawmsg_message(128 + 32, 14_000_000, 0, 1); // deleted account
    execute_sendrawmsg_message(128, 0, 0, 1);
    execute_sendrawmsg_message(128 + 1, 0, 0, 1);
    execute_sendrawmsg_message(128 + 32, 0, 0, 1); // deleted account

    execute_sendrawmsg_message(64, 14_000_000, 30_145_531, 1);
    execute_sendrawmsg_message(64 + 1, 14_000_000, 14_055_531, 1);
    execute_sendrawmsg_message(64, 0, 45_741_311, 1);
    execute_sendrawmsg_message(64 + 1, 0, 29_651_311, 1);

    // tests for flag +2 are considered in test_send_rawreserve_messages
}

#[test]
fn test_rand_seed() {
    let run_test = |code: Cell, seed: UInt256| {
        let out_msg =
            create_int_msg(SENDER_ACCOUNT.clone(), [0; 32].into(), 1_000_000_000, false, BLOCK_LT);
        let data = out_msg.serialize().unwrap();
        let msg = create_int_msg(
            THIRD_ACCOUNT.clone(),
            SENDER_ACCOUNT.clone(),
            1_000_000_000,
            false,
            PREV_BLOCK_LT,
        );
        let msg_cell = msg.serialize().unwrap();
        let tr_lt = BLOCK_LT + 1;
        let acc_id = SENDER_ACCOUNT.clone();
        let mut acc = create_test_account(2_000_000_000, acc_id, code, data);
        let mut params = execute_params(tr_lt);
        params.seed_block = seed;
        let trans =
            execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), &mut acc, &params)
                .unwrap();
        assert_eq!(trans.out_msgs.len().unwrap(), 1);
    };

    let code = compile_code_to_cell(
        "
        PUSHROOT
        RANDSEED
        THROWIFNOT 5
        PUSHINT 0
        SENDRAWMSG
    ",
    )
    .unwrap();
    run_test(code, UInt256::with_array([15; 32]));

    // if the user forgot to set the rand_seed_block value, then this 0 will be clearly visible on tests
    cross_check::disable_cross_check(); // TODO: fift always inits rand_seed
    let code = compile_code_to_cell(
        "
        PUSHROOT
        RANDSEED
        THROWIF 5
        PUSHINT 0
        SENDRAWMSG
    ",
    )
    .unwrap();
    run_test(code, UInt256::ZERO);
}

#[ignore = "test is working only for old VM"]
#[test]
fn test_change_128_flag() {
    // message with 128 flag is processed last
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        0,
        2,
    );
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());
    // check ordering messages
    assert_eq!(
        trans.out_msgs.export_vector().unwrap()[1].0.get_value().unwrap().coins,
        CurrencyCollection::with_coins(MSG2_BALANCE).coins,
    );
    for (i, msg) in trans.out_msgs.export_vector().unwrap().iter().enumerate() {
        assert_eq!(msg.0.int_header().unwrap().created_lt, 2000000002 + i as u64);
    }

    // if money is not enough, transaction fail
    let code = "
        PUSHINT 10
        PUSHINT 0
        RAWRESERVE

        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 44404882 + 68259633 - 1;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        112252293,
        0,
    );
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // if money is not enough, with mode 2 message will be skipped
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 130
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 44404882 + 68259633 - 1;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        9999999,
        1,
    );
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // two messages with 128 flag is disabled
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 128
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        1236111632,
        0,
    );
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // two messages with 128 flag is disabled, but flag 2 ignores error
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 130
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        0,
        1,
    );
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 128 flag is processed after rawreserve
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1000
        PUSHINT 0
        RAWRESERVE
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        1000,
        1,
    );
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 128+32 flag is processed last
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        0,
        2,
    );
    assert!(acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 32 flag is not valid without 128 flag
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 32
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ";

    let start_balance = 310000000;
    let msg_income = 1230000000;
    let (acc, trans) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        1237947412,
        0,
    );
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // if there is reserved value, then do not remove account
    let code = "
        PUSHINT 10
        PUSHINT 0
        RAWRESERVE

        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 3100000000;
    let msg_income = 444048082;
    let (acc, _) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        10,
        2,
    );
    assert!(!acc.is_none());

    // if action phase aborted, account will not destroyed
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 160
        SENDRAWMSG
    ";

    let start_balance = 3100000000;
    let msg_income = 444048082;
    let (acc, _) = execute_custom_transaction(
        start_balance,
        code,
        create_two_messages_data(),
        msg_income,
        false,
        3240159714,
        0,
    );
    assert!(!acc.is_none());
}

#[allow(clippy::too_many_arguments)]
fn test_uninit_account(
    code: Option<&Cell>,
    msg_balance: u64,
    bounce: bool,
    begin_status: AccountStatus,
    end_status: AccountStatus,
    addr_eq_state_hash: bool,
    result_account_balance: u64,
    count_out_msgs: usize,
    config: &ConfigParams,
) {
    let mut state_init = StateInit::default();
    if let Some(code) = code {
        state_init.set_code(code.clone());
    }

    let mut acc;
    let mut acc_id = SENDER_ACCOUNT.clone();
    if begin_status == AccountStatus::AccStateNonexist {
        if addr_eq_state_hash {
            acc_id = AccountId::from(state_init.hash().unwrap())
        }
        acc = Account::default();
    } else if begin_status == AccountStatus::AccStateUninit {
        if addr_eq_state_hash {
            acc_id = AccountId::from(state_init.hash().unwrap())
        }
        acc =
            Account::with_address(MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap());
    } else {
        panic!("Incorrect begin Account Status");
    }

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, msg_balance, bounce, PREV_BLOCK_LT);
    if code.is_some() {
        msg.set_state_init(state_init);
    }
    acc.update_storage_stat(config.size_limits_config().unwrap().acc_state_cells_for_storage_dict)
        .unwrap();
    let acc_copy = acc.clone();
    let config = BlockchainConfig::with_config(config.clone()).unwrap();
    let trans =
        try_replay_transaction(&mut acc, Some(&msg), config, &execute_params_none()).unwrap();
    assert_eq!(acc.status(), end_status);

    check_account_and_transaction(
        &acc_copy,
        &acc,
        &msg,
        Some(&trans),
        result_account_balance,
        count_out_msgs,
    );
}

fn test_uninit_account_initstate_default(
    msg_balance: u64,
    bounce: bool,
    begin_status: AccountStatus,
    end_status: AccountStatus,
    result_account_balance: u64,
    count_out_msgs: usize,
    config: &ConfigParams,
) {
    let mut acc;
    let acc_id = SENDER_ACCOUNT.clone();
    if begin_status == AccountStatus::AccStateNonexist {
        acc = Account::default();
    } else if begin_status == AccountStatus::AccStateUninit {
        acc =
            Account::with_address(MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap());
    } else {
        panic!("Incorrect begin Account Status");
    }

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, msg_balance, bounce, PREV_BLOCK_LT);
    msg.set_state_init(StateInit::default());
    let acc_copy = acc.clone();
    let config = BlockchainConfig::with_config(config.clone()).unwrap();
    let trans =
        try_replay_transaction(&mut acc, Some(&msg), config, &execute_params_none()).unwrap();
    assert_eq!(acc.status(), end_status);

    check_account_and_transaction(
        &acc_copy,
        &acc,
        &msg,
        Some(&trans),
        result_account_balance,
        count_out_msgs,
    );
}

#[test]
fn test_uninit_accounts() {
    let code = compile_code_to_cell(
        "
        PUSHROOT
        SENDRAWMSG
    ",
    )
    .unwrap();

    let mut config = ConfigParams::construct_from_file("real_boc/config.boc").unwrap();
    let capabilities = 0x2e;
    let global_version = GlobalVersion { version: 0, capabilities };

    config.set_config(ConfigParamEnum::ConfigParam8(ConfigParam8 { global_version })).unwrap();

    // code hash matches with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateActive,
        true,
        990_000_000,
        0,
        &config,
    );
    // code hash does not match with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    // code hash does not match with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    // not enougt money to execute
    test_uninit_account(
        Some(&code),
        1000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1000,
        0,
        &config,
    );
    // code hash matches with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateActive,
        true,
        990_000_000,
        0,
        &config,
    );
    // not enougt money to execute
    test_uninit_account(
        Some(&code),
        1000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        true,
        1000,
        0,
        &config,
    );
    // absence of code for init account
    test_uninit_account(
        None,
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );

    // if message has money, change account to AccStateUninit state
    test_uninit_account(
        None,
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    test_uninit_account(
        None,
        1_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000,
        0,
        &config,
    );
    // if bounce, account no need to create
    test_uninit_account(
        None,
        1_000_000_000,
        true,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateNonexist,
        false,
        0,
        1,
        &config,
    );
    // message has not money
    test_uninit_account(
        None,
        0,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateNonexist,
        false,
        0,
        0,
        &config,
    );

    // code hash matches with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateActive,
        true,
        990_000_000,
        0,
        &config,
    );
    // code hash does not match with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    // code hash does not match with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    // not enougt money to execute
    test_uninit_account(
        Some(&code),
        1000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1000,
        0,
        &config,
    );
    // code hash matches with account address
    test_uninit_account(
        Some(&code),
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateActive,
        true,
        990_000_000,
        0,
        &config,
    );
    // not enougt money to execute
    test_uninit_account(
        Some(&code),
        1000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        true,
        1000,
        0,
        &config,
    );
    // absence of code for init account
    test_uninit_account(
        None,
        1_000_000_000,
        false,
        AccountStatus::AccStateUninit,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );

    // if message has money, change account to AccStateUninit state
    test_uninit_account(
        None,
        1_000_000_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000_000_000,
        0,
        &config,
    );
    test_uninit_account(
        None,
        1_000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        false,
        1_000,
        0,
        &config,
    );
    // if bounce, account no need to create
    test_uninit_account(
        None,
        1_000_000_000,
        true,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateNonexist,
        false,
        0,
        1,
        &config,
    );
    // message has not money
    test_uninit_account(
        None,
        0,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateNonexist,
        false,
        0,
        0,
        &config,
    );

    // if init state is default, change account to AccStateUninit that save moneys
    test_uninit_account_initstate_default(
        1000,
        false,
        AccountStatus::AccStateNonexist,
        AccountStatus::AccStateUninit,
        1000,
        0,
        &config,
    );
}

#[test]
fn adjust_msg_value() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 64
        SENDRAWMSG
    ";
    let start_balance = 0;

    let acc_id = RECEIVER_ACCOUNT.clone();
    let out_msg = create_int_msg(acc_id.clone(), acc_id.clone(), 0, false, BLOCK_LT - 2);
    let code = compile_code_to_cell(code).unwrap();
    let data = out_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 1_400_200_000, false, BLOCK_LT - 2);

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 0, 1).unwrap();
}

#[test]
fn test_check_replace_src_addr() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 0;

    let acc_id = RECEIVER_ACCOUNT.clone();
    let out_msg =
        create_int_msg(SENDER_ACCOUNT.clone(), acc_id.clone(), 1_000_000_000, false, BLOCK_LT - 2);
    let code = compile_code_to_cell(code).unwrap();
    let data = out_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 1_400_200_000, false, BLOCK_LT - 2);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 1_240_463_237, 0).unwrap();
    let descr = trans.read_description().unwrap();
    assert_eq!(descr.action_phase_ref().unwrap().result_code, 35);
}

#[test]
fn special_account() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 200_000_000;

    let acc_id = THIRD_ACCOUNT.clone();
    let in_msg = create_int_msg(acc_id.clone(), acc_id.clone(), 10_000_000, false, BLOCK_LT - 2);
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let addr = acc.get_addr().unwrap();
    assert!(BLOCKCHAIN_CONFIG.is_special_account(addr.is_masterchain(), addr.address()).unwrap());

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 204_200_000, 1).unwrap();
    assert_eq!(acc.last_paid(), 0);
}

#[test]
fn test_fail_bound_message_with_nonexist_account() {
    let mut acc = Account::default();

    let msg_income = 46372;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, true, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 46372, 0).unwrap();

    let mut new_acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        &msg_income.into(),
    );
    new_acc.set_last_paid(1576526553);
    new_acc.set_last_tr_time(BLOCK_LT + 3);
    new_acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(Coins::zero(), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
        msg_size: StorageUsed::default(),
        req_fwd_fees: 10000000.into(),
    }));

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(0));
    good_trans.orig_status = AccountStatus::AccStateNonexist;
    good_trans.set_end_status(AccountStatus::AccStateUninit);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_fail_bound_message_with_nonexist_account_2() {
    let mut acc = Account::default();

    let msg_income = 0;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, true, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(Coins::zero(), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
        msg_size: StorageUsed::default(),
        req_fwd_fees: 10000000.into(),
    }));

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(msg_income));
    good_trans.orig_status = AccountStatus::AccStateNonexist;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn account_without_code() {
    let state_init = StateInit::default();
    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_coins(10000000000000000000),
        10,
        10,
    );

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999984737712408, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, -13);
    } else {
        unreachable!()
    }
}

#[test]
fn account_without_code2() {
    let state_init = StateInit::default();
    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = create_test_account(
        10000000000000000000,
        acc_id.clone(),
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    *acc.state_init_mut().unwrap() = state_init.clone();
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999999970712369, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, -13);
    } else {
        unreachable!()
    }
}

#[test]
fn account_without_data() {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_coins(10000000000000000000),
        10,
        10,
    );

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999984737712408, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 0);
    } else {
        unreachable!()
    }
}

#[test]
fn incorrect_acc_timestamp() {
    let acc_id = SENDER_ACCOUNT.clone();
    let start_balance = 20000000000000;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), acc_id.clone(), 14_200_000, false, BLOCK_LT - 2);

    let tr_lt = BLOCK_LT + 1;
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id, code, create_two_messages_data());
    acc.set_last_paid(acc.last_paid() + 1000000000);
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert!(matches!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::TrExecutorError(_)));
}

#[test]
fn sendmsg_64_fail() {
    let code = "
        PUSHROOT
        CTOS
        LDREF
        PLDREF

        SWAP

        PUSHINT 67
        SENDRAWMSG
        PUSHINT 64
        SENDRAWMSG
    ";
    let data = create_two_messages_data();
    execute_custom_transaction(410_000_000 - 21097413, code, data, 41_000_000, false, 49999999, 1);
}

#[test]
fn test_account_with_enought_balance_to_run_compute_phase() {
    let acc_id = AccountId::from([0x66; 32]);
    let mut acc = create_test_account(
        0,
        acc_id.clone(),
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let mut msg = create_test_external_msg();
    if let CommonMsgInfo::ExtInMsgInfo(header) = msg.header_mut() {
        header.dst = MsgAddressInt::with_standart(None, -1, acc_id).unwrap();
    } else {
        unreachable!()
    }

    let err = execute(&msg, &mut acc, BLOCK_LT + 1).expect_err("no funds for external message");
    assert_eq!(
        err.downcast::<ExecutorError>().unwrap(),
        ExecutorError::ExtMsgComputeSkipped(ComputeSkipReason::NoGas)
    );
}

fn make_transaction_from_workchain_to_masterchain(
    from_worckchain_to_masterchain: bool,
    result_acc_balance: u64,
    bounce: bool,
) {
    let (w_id_src, w_id_dst) = if from_worckchain_to_masterchain { (0, -1) } else { (-1, 0) };
    let code = if bounce {
        "CTOS"
    } else {
        "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    "
    };
    let start_balance = 200_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let in_msg = create_int_msg_workchain(
        w_id_dst,
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2,
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc =
        create_test_account_workchain(start_balance, w_id_dst, acc_id.clone(), code, data);

    let mut msg = create_int_msg_workchain(
        w_id_dst,
        RECEIVER_ACCOUNT.clone(),
        acc_id,
        14_200_000,
        bounce,
        BLOCK_LT - 2,
    );
    msg.set_src_address(
        MsgAddressInt::with_standart(None, w_id_src, RECEIVER_ACCOUNT.clone()).unwrap(),
    );

    let tr = execute_c(&msg, &mut acc, BLOCK_LT + 1, result_acc_balance, 1).unwrap();
    if bounce {
        assert!(matches!(get_tr_descr(&tr).bounce.unwrap(), TrBouncePhase::Ok(_)));
    }
}

#[test]
fn send_message_from_workchain_to_masterchain() {
    make_transaction_from_workchain_to_masterchain(true, 42867458, false);
    make_transaction_from_workchain_to_masterchain(false, 203443677, false);

    make_transaction_from_workchain_to_masterchain(true, 47869017, true);
    make_transaction_from_workchain_to_masterchain(false, 199847869, true);
}

fn make_transaction_from_workchain_to_masterchain2(
    from_worckchain_to_masterchain: bool,
    result_acc_balance: u64,
) {
    let (w_id_src, w_id_dst) = if from_worckchain_to_masterchain { (0, -1) } else { (-1, 0) };
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 200_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let mut in_msg = create_int_msg_workchain(
        w_id_dst,
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2,
    );
    in_msg.set_src_address(
        MsgAddressInt::with_standart(None, w_id_src, SENDER_ACCOUNT.clone()).unwrap(),
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc =
        create_test_account_workchain(start_balance, w_id_src, acc_id.clone(), code, data);

    let msg = create_int_msg_workchain(
        w_id_src,
        RECEIVER_ACCOUNT.clone(),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2,
    );

    execute_c(&msg, &mut acc, BLOCK_LT + 1, result_acc_balance, 1).unwrap();
}

#[test]
fn send_message_from_workchain_to_masterchain2() {
    make_transaction_from_workchain_to_masterchain2(true, 203443677);
    make_transaction_from_workchain_to_masterchain2(false, 42867458);
}

#[test]
fn test_message_with_anycast_output_address() {
    let start_balance = 300_000_000_000;
    let msg_income = 100_000_000_000;

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()),
        0,
        SENDER_ACCOUNT.clone(),
    )
    .unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399999424996,
        0,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 575000.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    description.action = Some(TrActionPhase {
        success: true,
        valid: true,
        tot_actions: 1,
        skipped_actions: 1,
        msgs_created: 0,
        action_list_hash: "0x0695bc3c098f64c83a812ddfb987e1aa1e463fe90f663e182ba02ef5aab9de24"
            .parse()
            .unwrap(),
        ..TrActionPhase::default()
    });

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_message_with_anycast_output_address_2() {
    let start_balance = 300_000_000_000;
    let msg_income = 100_000_000_000;

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()),
        0,
        SENDER_ACCOUNT.clone(),
    )
    .unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 0
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399999424996,
        0,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 575000.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash =
        "0x221ff324345e3bb2b1f6d6277d3613c9a17048d4d3b6b34f6103c93cf4b427d1".parse().unwrap();
    action_ph.result_code = 36;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsed::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_message_with_anycast_address() {
    let msg_income = 100_000_000_000; // send enough money to run contract
    let mut acc = Account::default();

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()),
        0,
        SENDER_ACCOUNT.clone(),
    )
    .unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, THIRD_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let msg = Message::with_int_header(hdr);

    let tr_lt = BLOCK_LT + 1;
    let trans = execute(&msg, &mut acc, tr_lt).unwrap();

    let address =
        AccountId::from_string("2222211111111111111111111111111111111111111111111111111111111111")
            .unwrap();
    let new_acc = Account::uninit(
        MsgAddressInt::with_standart(None, 0, address).unwrap(),
        CurrencyCollection::with_coins(msg_income),
        BLOCK_LT + 2,
        BLOCK_UT,
    );
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();

    description.credit_first = true;
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.storage_ph = Some(Default::default());
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();
    good_trans.orig_status = AccountStatus::AccStateNonexist;

    good_trans.set_total_fees(CurrencyCollection::with_coins(0));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_action_phase_fields_with_unsuccess_action() {
    let start_balance = 100000000;
    let msg_income = 1_000_000;
    let storage_fee = 6618;
    let total_balance = 87923382;
    let gas_used = 1307;
    let gas_fees = 13070000;
    let total_fees = gas_fees + storage_fee;

    let code = compile_code_to_cell(
        "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(
        start_balance,
        acc_id.clone(),
        code.clone(),
        create_two_messages_data(),
    );
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        false,
        PREV_BLOCK_LT,
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, create_two_messages_data());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().coins, 0).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    let (msg1, msg2) = create_two_internal_messages();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2));

    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(&MSG_FWD_FEE.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.result_code = 37;
    action_ph.result_arg = Some(1);
    action_ph.no_funds = true;
    action_ph.tot_msg_size = StorageUsed::with_values_checked(1, 705).unwrap();

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_coins(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_message_with_zero_value() {
    let mut acc = Account::default();

    let msg = create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), 0, false, BLOCK_LT + 1);

    let tr_lt = BLOCK_LT + 1;

    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateNonexist);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, CurrencyCollection::default());
}

fn build_contract_info(acc: &Account, msg: &Message, config: &ConfigParams) -> SmartContractInfo {
    let config_params = config.clone();
    SmartContractInfo {
        myself: SliceData::load_builder(msg.dst_ref().unwrap().write_to_new_cell().unwrap())
            .unwrap(),
        block_lt: BLOCK_LT,
        trans_lt: BLOCK_LT + 2,
        unix_time: BLOCK_UT,
        balance: acc.balance().unwrap().clone(),
        in_msg: Some(msg.clone()),
        incoming_value: msg.value().cloned().unwrap_or_default(),
        config_params,
        ..Default::default()
    }
}

#[test]
fn test_message_with_var_address() {
    let start_balance = 300_000_000_000;
    let msg_income = 100_000_000_000;

    let dst = MsgAddressInt::with_variant(None, 0, SliceData::new(vec![6; 31])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399999424996,
        0,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 575000.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.skipped_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash =
        "0xd95f4f546c3d9df56a578939486abbfd78b8bd53a820350aab3cf896a8924669".parse().unwrap();
    action_ph.result_code = 0;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsed::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_message_with_var_address_in_masterchain() {
    let start_balance = 300_000_000_000;
    let msg_income = 100_000_000_000;

    let dst = MsgAddressInt::with_variant(None, -1, SliceData::new(vec![6; 31])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account_workchain(start_balance, -1, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        -1,
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399994246388, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399994246388,
        -1,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 3612.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 5750000.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.skipped_actions = 0;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash =
        "0x6e4d01ecc1c74a741dacd6d4250bc9f7b72b519abbaa57c255f781fd305288c7".parse().unwrap();
    action_ph.result_code = 34;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsed::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(5753612));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_message_to_nonexisting_workchain() {
    let start_balance = 300_000_000_000;
    let msg_income = 100_000_000_000;

    let dst = MsgAddressInt::with_standart(None, 1, AccountId::from([0x44; 32])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_coins(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ",
    )
    .unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account_workchain(start_balance, -1, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        -1,
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        true,
        PREV_BLOCK_LT,
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399994246423, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399994246423,
        -1,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap(),
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 3577.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 5750000.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.skipped_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash =
        "0xa8d20bc0b9058e2ea89c21595fecae517f3a18c49a5492df8675b85ca8aed63f".parse().unwrap();
    action_ph.result_code = 0;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsed::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(5753577));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn bounced_message_with_special_account() {
    let acc_id = AccountId::from([0x66; 32]);
    assert!(BLOCKCHAIN_CONFIG.is_special_account(true, &acc_id).unwrap());

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_coins(1_400_200_000),
        10,
        10,
    );

    let msg = create_int_msg_workchain(
        -1,
        THIRD_ACCOUNT.clone(),
        acc_id,
        494915282787965,
        true,
        BLOCK_LT,
    );

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 1400200000, 1).unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let descr = get_tr_descr(&trans);
    if let TrBouncePhase::Ok(bounce) = descr.bounce.unwrap() {
        assert_eq!(bounce.fwd_fees, Coins::from(6666718));
    } else {
        unreachable!()
    }
}

fn create_account(balance: u64, code: &str) -> Account {
    let code = compile_code_to_cell(code).unwrap();
    create_test_account_workchain(balance, 0, SENDER_ACCOUNT.clone(), code, Cell::default())
}

fn create_bouncable_message(value: u64) -> Message {
    create_int_msg_workchain(0, [1; 32].into(), SENDER_ACCOUNT.clone(), value, true, BLOCK_LT)
}

#[test]
fn test_bouncable() -> Result<()> {
    let throw_code = "THROW 123";
    let bounce_gas_leftover_code = "
        PUSH s2
        CTOS
        PUSHINT 4
        SDSKIPFIRST
        PUSHINT 267 ; 2 + 1 + 8 + 256 = 267 source address as destination
        SDCUTFIRST
        NEWC
        STSLICECONST x62_
        STSLICE
        PUSHINT 111
        STZEROES
        ENDC
        PUSHINT 64
        SENDRAWMSG
    ";
    // account with tiny balance and big dues
    let tiny_balance = 1;
    let msg_value = 2_800_000;
    let due_payment = 2_000_000;

    // account has enough gas and can send message, but due payment is growing
    let mut account = create_account(tiny_balance, bounce_gas_leftover_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute(&msg, &mut account, BLOCK_LT)?;
    let msg = tr.get_out_msg(0)?.unwrap();
    let hdr = msg.int_header().unwrap();
    assert!(!hdr.bounced);
    assert_eq!(hdr.value().coins.as_u128(), 373_000);
    assert_eq!(account.balance().unwrap().coins.as_u128(), 0);
    assert_eq!(account.due_payment().unwrap().as_u128(), 2_118_021);

    // account has enough value to send bounced message, but due payment is growing
    // we get not enough value to send bounced message
    let mut account = create_account(tiny_balance, throw_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute(&msg, &mut account, BLOCK_LT)?;
    let msg = tr.get_out_msg(0)?.unwrap();
    let hdr = msg.int_header().unwrap();
    assert!(hdr.bounced);
    assert_eq!(hdr.value().coins.as_u128(), 1_700_000);
    assert_eq!(account.balance().unwrap().coins.as_u128(), 0);
    assert_eq!(account.due_payment().unwrap().as_u128(), 2_106_850);

    Ok(())
}

fn prepare_recursive_merkle_cell(mut depth: u32) -> Result<Cell> {
    let ref1 = BuilderData::with_bytes(b"first reference")?.into_cell()?;
    let ref2 = BuilderData::with_bytes(b"second reference")?.into_cell()?;
    let mut builder = BuilderData::with_bytes(b"root cell")?;
    builder.checked_append_reference(ref1.clone())?;
    builder.checked_append_reference(ref2.clone())?;
    let root = builder.into_cell()?;

    let merkle =
        MerkleProof::create(&root, |hash| hash == &ref1.repr_hash() || hash == &root.repr_hash())?;
    let mut proof = merkle.serialize()?;

    depth -= 1;
    while depth > 0 {
        let merkle = MerkleProof { hash: proof.repr_hash(), depth: proof.repr_depth(), proof };
        proof = merkle.serialize()?;
        depth -= 1;
    }
    Ok(proof)
}

#[test]
fn test_action_fine() {
    // account is sending message with extra merkle
    let start_balance = 1_000_000_000;
    let msg_income = 30_000_000;
    let msg_value = msg_income;
    let flag = 0;
    let bounce = false;
    let code = format!(
        "
        ACCEPT
        DROP ; drop internal flag
        LDREF ; load proof cell from body which is new msg
        SWAP
        ; CTOS ; merkle proof unable to convert to slice 
        DUP
        HASHCU
        NEWC ; new merkle proof cell
        PUSHINT 3 ; merkle proof flag
        STUR 8
        STU 256 ; hash
        PUSHINT 3 ; merkle proof depth
        STUR 16
        STREF ; merkle proof cell
        TRUE
        ENDXC ; new merkle proof cell
        NEWC
        STREF
        STSLICE ; new message
        ENDC
        PUSHINT {flag}
        SENDRAWMSG
    "
    );
    let mut out_msg = create_int_msg_workchain(
        0,
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        msg_value,
        bounce,
        0,
    );
    assert!(!out_msg.is_masterchain());
    let proof = prepare_recursive_merkle_cell(2).unwrap();
    out_msg.set_body(SliceData::with_reference(proof).unwrap());

    let mut msg = create_int_msg_workchain(
        0,
        SENDER_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        msg_income,
        bounce,
        BLOCK_LT - 2,
    );
    assert!(!msg.is_masterchain());
    let body = SliceData::load_builder(out_msg.write_to_new_cell().unwrap()).unwrap();
    msg.set_body(body);
    let account = create_account(start_balance, &code);
    assert!(!account.get_addr().unwrap().is_masterchain());
    execute_acc_with_message(account, &msg).expect_action_failed(40);
}

fn do_test_acc_size_limits(workchain_id: i8, acc_id: &AccountId, mut config: ConfigParams) {
    let depth = 999;
    let mut size_cfg = config.size_limits_config().unwrap();
    if workchain_id == -1 {
        size_cfg.max_mc_acc_state_cells = depth;
    } else {
        size_cfg.max_acc_state_cells = depth;
    }
    config.set_config(ConfigParamEnum::ConfigParam43(size_cfg.clone())).unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();

    let mut big_tree = BuilderData::new();
    for cell_num in 0..depth {
        let mut cell = BuilderData::with_bytes(cell_num.to_be_bytes()).unwrap();
        if big_tree.references_free() != 0 {
            big_tree.checked_append_reference(cell.into_cell().unwrap()).unwrap();
        } else {
            cell.checked_append_reference(big_tree.into_cell().unwrap()).unwrap();
            big_tree = cell;
        }
    }
    let big_tree = big_tree.into_cell().unwrap();

    let code = compile_code_to_cell(
        "
        ACCEPT
        NEWC
        PUSH c4
        STREFR
        ENDC
        POP c4
        PUSH s1
        PUSHINT 10
        SDATASIZE
        POP s0
        IFNOTRET
        PUSH s2
        NEWC
        STSLICE
        ENDC
        PUSHINT 0
        SENDRAWMSG
    ",
    )
    .unwrap();

    let big_state_init = StateInit::with_code_and_data(code.clone(), big_tree.clone());
    let big_acc_id = AccountId::from(big_state_init.hash().unwrap());
    let mut big_msg = create_int_msg_workchain(
        workchain_id,
        acc_id.clone(),
        big_acc_id.clone(),
        1_000_000_000,
        true,
        PREV_BLOCK_LT,
    );
    big_msg.set_state_init(big_state_init.clone());

    let small_state_init = StateInit::with_code_and_data(code, big_tree.reference(0).unwrap());
    let small_acc_id = AccountId::from(small_state_init.hash().unwrap());
    let mut small_msg = create_int_msg_workchain(
        workchain_id,
        acc_id.clone(),
        small_acc_id.clone(),
        1_000_000_000,
        true,
        PREV_BLOCK_LT,
    );
    small_msg.set_state_init(small_state_init.clone());

    // nonexist account
    let mut acc = Account::default();

    let trans =
        try_replay_transaction(&mut acc, Some(&big_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(trans.read_description().unwrap().is_aborted());
    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().is_skipped().unwrap(),
        &ComputeSkipReason::BadState
    );
    assert_eq!(trans.end_status, AccountStatus::AccStateNonexist);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 1);

    let mut acc = Account::default();

    let trans =
        try_replay_transaction(&mut acc, Some(&small_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(!trans.read_description().unwrap().is_aborted());
    assert_eq!(trans.end_status, AccountStatus::AccStateActive);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 0);

    // uninit account
    let mut acc = Account::uninit(
        MsgAddressInt::standard(workchain_id, big_acc_id.clone()),
        CurrencyCollection::with_coins(1000),
        BLOCK_LT,
        BLOCK_UT,
    );
    // let mut acc = Account::uninit_standard(big_acc_id.clone(), 1000, BLOCK_LT, BLOCK_UT);

    let trans =
        try_replay_transaction(&mut acc, Some(&big_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(trans.read_description().unwrap().is_aborted());
    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().is_skipped().unwrap(),
        &ComputeSkipReason::BadState
    );
    assert_eq!(trans.end_status, AccountStatus::AccStateUninit);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 1);

    let mut acc = Account::uninit(
        MsgAddressInt::standard(workchain_id, small_acc_id.clone()),
        CurrencyCollection::with_coins(1000),
        BLOCK_LT,
        BLOCK_UT,
    );

    let trans =
        try_replay_transaction(&mut acc, Some(&small_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(!trans.read_description().unwrap().is_aborted());
    assert_eq!(trans.end_status, AccountStatus::AccStateActive);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 0);

    // frozen account
    let mut acc = Account::frozen(
        MsgAddressInt::standard(workchain_id, big_acc_id.clone()),
        CurrencyCollection::with_coins(1000),
        BLOCK_LT,
        BLOCK_UT,
        None,
        big_state_init.hash().unwrap(),
    );

    let trans =
        try_replay_transaction(&mut acc, Some(&big_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(trans.read_description().unwrap().is_aborted());
    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().is_skipped().unwrap(),
        &ComputeSkipReason::BadState
    );
    assert_eq!(trans.end_status, AccountStatus::AccStateFrozen);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 1);

    let mut acc = Account::frozen(
        MsgAddressInt::standard(workchain_id, small_acc_id.clone()),
        CurrencyCollection::with_coins(1000),
        BLOCK_LT,
        BLOCK_UT,
        None,
        small_state_init.hash().unwrap(),
    );

    // active account
    let trans =
        try_replay_transaction(&mut acc, Some(&small_msg), config.clone(), &execute_params_none())
            .unwrap();
    assert!(!trans.read_description().unwrap().is_aborted());
    assert_eq!(trans.end_status, AccountStatus::AccStateActive);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 0);

    let msg = create_test_external_msg_with_int_ex(
        workchain_id,
        small_acc_id.clone(),
        acc_id.clone(),
        20_000_000,
    );

    let trans =
        try_replay_transaction(&mut acc, Some(&msg), config.clone(), &execute_params_none())
            .unwrap();
    assert_eq!(trans.end_status, AccountStatus::AccStateActive);
    assert_eq!(trans.out_msgs.count(2).unwrap(), 0);
    assert_eq!(trans.read_description().unwrap().action_phase_ref().unwrap().result_code, 50);
}

#[test]
fn test_acc_size_limits() {
    let config = ConfigParams::construct_from_file("real_boc/config12.boc").unwrap();
    do_test_acc_size_limits(0, &SENDER_ACCOUNT, config.clone());
    do_test_acc_size_limits(-1, &SENDER_ACCOUNT, config.clone());
}

#[test]
fn test_override_gas_limit() {
    // infinite loop contract with different limits
    let code = compile_code_to_cell(
        "ACCEPT
        AGAINEND
        NEWC
        ENDC
        HASHCU",
    )
    .unwrap();

    // use simple account with standard limits
    let acc_id = RECEIVER_ACCOUNT.clone();
    let msg = create_int_msg_workchain(
        0,
        SENDER_ACCOUNT.clone(),
        acc_id.clone(),
        10000000,
        false,
        BLOCK_LT,
    );
    let acc =
        create_test_account_workchain(100_000_000_000, 0, acc_id, code.clone(), Default::default());
    execute_acc_with_message(acc, &msg).expect_compute_result(-14).expect_gas_used(1_000_000);

    // use account with overridden gas limit
    let addr: MsgAddressInt = "UQBeSl-dumOHieZ3DJkNKVkjeso7wZ0VpzR4LCbLGTQ8xr57".parse().unwrap();
    let acc_id = addr.address().clone();
    let msg = create_int_msg_workchain(
        0,
        SENDER_ACCOUNT.clone(),
        acc_id.clone(),
        10000000,
        false,
        BLOCK_LT,
    );
    let mut acc =
        create_test_account_workchain(100_000_000_000, 0, acc_id, code.clone(), Default::default());
    execute_acc_with_message(acc.clone(), &msg)
        .expect_compute_result(-14)
        .expect_gas_used(70_000_000);

    acc.set_last_paid(1740787200);
    execute_acc_with_message_and_time(acc, &msg, 1740787200)
        .expect_compute_result(-14)
        .expect_gas_used(1_000_000);
}

#[test]
fn test_empty_dict_hash() {
    let msg = create_int_msg_workchain(
        0,
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        1_000_000,
        false,
        PREV_BLOCK_LT,
    );
    let mut acc = Account::active(
        msg.dst().unwrap(),
        1_000_000_000.into(),
        0,
        ACCOUNT_UT,
        StateInit::default(),
        2,
    )
    .unwrap();
    assert_eq!(acc.storage_info().unwrap().dict_hash(), None);
    assert_eq!(acc.update_storage_stat(1).unwrap(), None);

    let mut config = ConfigParams::construct_from_file("real_boc/config.boc").unwrap();

    let mut limits = config.size_limits_config().unwrap();
    limits.acc_state_cells_for_storage_dict = 1;
    config.set_config(ConfigParamEnum::ConfigParam43(limits)).unwrap();
    let config = BlockchainConfig::with_config(config).unwrap();

    try_replay_transaction(&mut acc, Some(&msg), config.clone(), &execute_params_none()).unwrap();
    assert_eq!(acc.storage_info().unwrap().dict_hash(), Some(&UInt256::default()));
    assert_eq!(acc.update_storage_stat(1).unwrap(), None);
}

#[test]
fn test_new_bounce() {
    let mut acc = Account::default();
    let balance = 1_000_000_000;
    let mut msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        balance,
        true,
        PREV_BLOCK_LT,
    );
    let header = msg.int_header_mut().unwrap();
    header.extra_flags =
        (EXTRA_FLAG_NEW_BOUNCE_FORMAT as u64 | EXTRA_FLAG_FULL_BODY_BOUNCE as u64).into();

    let original_body_ref =
        BuilderData::with_bytes(b"Original body ref").unwrap().into_cell().unwrap();
    let mut original_body = BuilderData::with_ref(original_body_ref);
    original_body.append_bitstring(b"Original body root").unwrap();
    msg.set_body(SliceData::load_builder(original_body.clone()).unwrap());

    let tr = execute(&msg, &mut acc, BLOCK_LT).unwrap();
    let out_msg = tr.get_out_msg(0).unwrap().unwrap();
    let hdr = out_msg.int_header().unwrap();
    assert!(hdr.bounced);
    assert_eq!(
        hdr.extra_flags.inner(),
        (EXTRA_FLAG_NEW_BOUNCE_FORMAT | EXTRA_FLAG_FULL_BODY_BOUNCE) as u128
    );
    let mut body = out_msg.body().unwrap().clone();
    let bounce_body = NewBounceBody::construct_from(&mut body).unwrap();
    assert_eq!(bounce_body.original_body, original_body.clone().into_cell().unwrap());
    assert_eq!(bounce_body.bounced_by_phase, BouncedByPhase::ComputeSkip);
    assert_eq!(bounce_body.exit_code, -(ComputeSkipReason::NoState as i32));
    assert_eq!(bounce_body.compute_phase, None);
    assert_eq!(
        bounce_body.original_info.read_struct().unwrap(),
        NewBounceOriginalInfo {
            value: CurrencyCollection::with_coins(balance),
            created_at: msg.at_and_lt().unwrap().0,
            created_lt: msg.at_and_lt().unwrap().1,
        }
    );

    let code = compile_code_to_cell("CTOS").unwrap();
    let mut acc = create_test_account(balance, RECEIVER_ACCOUNT.clone(), code, Cell::default());
    let tr = execute(&msg, &mut acc, BLOCK_LT).unwrap();
    let out_msg = tr.get_out_msg(0).unwrap().unwrap();
    assert!(out_msg.int_header().unwrap().bounced);
    let bounce_body = NewBounceBody::construct_from(&mut out_msg.body().unwrap().clone()).unwrap();
    assert_eq!(bounce_body.bounced_by_phase, BouncedByPhase::Compute);
    assert_eq!(bounce_body.exit_code, ExceptionCode::TypeCheckError as i32);
    assert_eq!(
        bounce_body.compute_phase.unwrap(),
        NewBounceComputePhaseInfo { gas_used: 68, vm_steps: 2 }
    );

    msg.int_header_mut().unwrap().extra_flags = (EXTRA_FLAG_NEW_BOUNCE_FORMAT as u64).into();
    let code = compile_code_to_cell(
        "
        PUSHROOT
        CTOS
        PLDREF
        PUSHINT 16
        SENDRAWMSG
    ",
    )
    .unwrap();
    let data_msg = create_int_msg_workchain(
        0,
        RECEIVER_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        1000,
        false,
        BLOCK_LT,
    );
    let data = BuilderData::with_raw_and_refs(vec![0x55; 32], 256, [data_msg.serialize().unwrap()])
        .unwrap();

    let mut acc =
        create_test_account(balance, RECEIVER_ACCOUNT.clone(), code, data.into_cell().unwrap());
    let tr = execute(&msg, &mut acc, BLOCK_LT).unwrap();
    let out_msg = tr.get_out_msg(0).unwrap().unwrap();
    assert!(out_msg.int_header().unwrap().bounced);
    let bounce_body = NewBounceBody::construct_from(&mut out_msg.body().unwrap().clone()).unwrap();
    assert_eq!(bounce_body.bounced_by_phase, BouncedByPhase::Action);
    assert_eq!(bounce_body.exit_code, RESULT_CODE_INCORRECT_SRC_ADDRESS);
    assert_eq!(
        &bounce_body.original_body,
        &BuilderData::with_raw(original_body.data(), original_body.bits_used())
            .unwrap()
            .into_cell()
            .unwrap()
    );
}
