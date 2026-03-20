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

mod common;

use common::*;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    AccStatusChange, AccountStatus, AnycastInfo, CurrencyCollection, Grams, InternalMessageHeader,
    Message, MsgAddressInt, MsgAddressIntOrNone, Serializable, SliceData, StorageUsed,
    TrBouncePhase, TrBouncePhase::Nofunds, TrBouncePhaseNofunds, TrBouncePhaseOk,
    SENDMSG_BOUNCE_IF_FAIL, SENDMSG_ORDINARY,
};

#[test]
fn test_action_phase_failed() {
    let mut test_case = TransactionTestCase {
        gas_used: 1307,
        msg_income: 1_000_000,
        lt_delta: 2,
        start_balance: 100_000_000,
        storage_fee: 6618,
        ..Default::default()
    };
    test_case.expect_compute_vm_success(10);
    test_case.expect_two_out_messages(SENDMSG_ORDINARY);
    test_case.expect_action_fail_with_one_message();
    test_case.expect_total_fees(test_case.storage_fee + test_case.gas_fees());
    test_case.expect_end_balance(
        test_case.start_balance + test_case.msg_income
            - test_case.storage_fee
            - test_case.gas_fees(),
    );

    let code = create_send_two_messages_code();
    let data = create_two_messages_data();

    let mut test_ctx = TransactionTestContext::with_params(code, data, None, &test_case);
    let trans = test_ctx.execute(0).unwrap();
    pretty_assertions::assert_eq!(test_ctx.acc, test_ctx.new_acc);

    let good_trans = test_ctx.create_sample_transaction(test_case);
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_action_phase_failed_with_flag_16_but_unsuccess_bounce() {
    let mut test_case = TransactionTestCase {
        bounce: Some(Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: Grams::from(MSG_FWD_FEE),
        })),
        gas_used: 1323,
        msg_income: 1_000_000,
        lt_delta: 2,
        start_balance: 100_000_000,
        storage_fee: 6643,
        ..Default::default()
    };
    test_case.expect_compute_vm_success(10);
    test_case.expect_two_out_messages(SENDMSG_BOUNCE_IF_FAIL);
    test_case.expect_action_fail_with_one_message();
    test_case.expect_total_fees(test_case.storage_fee + test_case.gas_fees());
    test_case.expect_end_balance(
        test_case.start_balance + test_case.msg_income
            - test_case.storage_fee
            - test_case.gas_fees(),
    );

    let code = compile_code_to_cell(
        "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 16
        SENDRAWMSG
        PUSHINT 16
        SENDRAWMSG
    ",
    )
    .unwrap();
    let data = create_two_messages_data();

    let mut test_ctx = TransactionTestContext::with_params(code, data, None, &test_case);
    let acc_before = test_ctx.acc.clone();
    let trans = execute(&test_ctx.msg, &mut test_ctx.acc, test_ctx.tr_lt).unwrap();
    check_account_and_transaction(
        &acc_before,
        &test_ctx.acc,
        &test_ctx.msg,
        Some(&trans),
        test_ctx.new_acc.balance().unwrap().grams,
        0,
    );
    pretty_assertions::assert_eq!(test_ctx.acc, test_ctx.new_acc);

    let good_trans = test_ctx.create_sample_transaction(test_case);
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_action_phase_failed_with_flag_16() {
    let mut test_case = TransactionTestCase {
        bounce: Some(TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsed::default(),
            msg_fees: Grams::from(MSG_MINE_FEE),
            fwd_fees: Grams::from(MSG_FWD_FEE - MSG_MINE_FEE),
        })),
        gas_limit: Some(15000),
        gas_used: 1323,
        lt_delta: 3,
        msg_income: 150_000_000,
        start_balance: 10_000_000,
        storage_fee: 6630,
        ..Default::default()
    };
    test_case.expect_compute_vm_success(10);
    test_case.expect_two_out_messages(SENDMSG_BOUNCE_IF_FAIL);
    test_case.expect_action_fail_with_one_message();
    test_case.expect_total_fees(test_case.storage_fee + test_case.gas_fees() + MSG_MINE_FEE);
    test_case.expect_end_balance(test_case.start_balance - test_case.storage_fee);

    let code = compile_code_to_cell(
        "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 16
        SENDRAWMSG
        PUSHINT 16
        SENDRAWMSG
    ",
    )
    .unwrap();
    let data = create_two_messages_data();

    let mut test_ctx = TransactionTestContext::with_params(code, data, None, &test_case);
    let acc_before = test_ctx.acc.clone();
    let trans = execute(&test_ctx.msg, &mut test_ctx.acc, test_ctx.tr_lt).unwrap();
    check_account_and_transaction(
        &acc_before,
        &test_ctx.acc,
        &test_ctx.msg,
        Some(&trans),
        test_ctx.new_acc.balance().unwrap().grams,
        1,
    );
    pretty_assertions::assert_eq!(test_ctx.acc, test_ctx.new_acc);

    let mut good_trans = test_ctx.create_sample_transaction(test_case);
    let mut message = Message::with_int_header(InternalMessageHeader {
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(
            MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        ),
        dst: MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap(),
        value: CurrencyCollection::with_grams(126_770_000),
        extra_flags: Default::default(),
        fwd_fee: Grams::from(6_666_718),
        created_lt: 2000000002,
        created_at: BLOCK_UT.into(),
    });
    let builder = (-1i32).write_to_new_cell().unwrap();
    message.set_body(SliceData::load_builder(builder).unwrap());
    good_trans.add_out_message(&message).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_message_with_anycast_output_address_bounced_action() {
    // message with0 anycast in address will fail action phase and get bounce
    let mut test_case = TransactionTestCase {
        bounce: Some(TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsed::default(),
            msg_fees: Grams::from(333_328),
            fwd_fees: Grams::from(666_672),
        })),
        gas_fees: Some(583_000),
        gas_limit: Some(1_000_000),
        gas_used: 583,
        lt_delta: 3,
        msg_income: 100_000_000_000,
        start_balance: 300_000_000_000,
        storage_fee: 4,
        workchain: Some(0),
        ..Default::default()
    };
    test_case.expect_compute_vm_success(4);
    test_case.expect_total_fees(916332);
    test_case.expect_end_balance(test_case.start_balance - test_case.storage_fee);

    test_case.phase_action.success = false;
    test_case.phase_action.valid = true;
    test_case.phase_action.status_change = AccStatusChange::Unchanged;
    test_case.phase_action.tot_actions = 1;
    test_case.phase_action.msgs_created = 0;
    test_case.phase_action.action_list_hash =
        "0xd56418efb988e935bb3f36e07879046f6aa802d396d1b06f879af6db89000ac5".parse().unwrap();
    test_case.phase_action.result_code = 36;
    test_case.phase_action.result_arg = None;
    test_case.phase_action.no_funds = false;
    test_case.phase_action.tot_msg_size = StorageUsed::default();
    test_case.transaction_credit_first = false;
    test_case.transaction_aborted = true;

    let code = compile_code_to_cell(
        "
        PUSHROOT
        PUSHINT 16 ; bounce if action failed
        SENDRAWMSG
    ",
    )
    .unwrap();

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()),
        0,
        SENDER_ACCOUNT.clone(),
    )
    .unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        dst,
        CurrencyCollection::with_grams(test_case.msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = 0;
    let data = Message::with_int_header(hdr).serialize().unwrap();

    let mut test_ctx = TransactionTestContext::with_params(code, data, None, &test_case);
    let trans = execute(&test_ctx.msg, &mut test_ctx.acc, test_ctx.tr_lt).unwrap();
    pretty_assertions::assert_eq!(test_ctx.acc, test_ctx.new_acc);

    let mut good_trans = test_ctx.create_sample_transaction(test_case);
    let mut message = Message::with_int_header(InternalMessageHeader {
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(
            MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        ),
        dst: MsgAddressInt::with_standart(None, 0, THIRD_ACCOUNT.clone()).unwrap(),
        value: CurrencyCollection::with_grams(99_998_417_000),
        extra_flags: Default::default(),
        fwd_fee: Grams::from(666_672),
        created_lt: 2000000002,
        created_at: BLOCK_UT.into(),
    });
    let builder = (-1i32).write_to_new_cell().unwrap();
    message.set_body(SliceData::load_builder(builder).unwrap());
    good_trans.add_out_message(&message).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage() {
    // account balance is not enough to pay for storage fee

    // i. Bad code without accept
    let code = "
        PUSHCTR C4
        PUSHINT 0
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
    let start_balance = 154258689 - due;
    // message value is enough to compute, but not enough to send value, no bounce because no flag 16
    execute_transaction_case(start_balance, code, data.clone(), 5_750_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(5_750_100)
        .expect_compute_result(0)
        .expect_no_bounce()
        .expect_status(AccountStatus::AccStateActive);

    // vi. good code with accept and with sendmsg
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
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
    let start_balance = 155322542 - due;
    // message value is enough to compute, but not enough to send value, no bounce because no flag 16
    execute_transaction_case(start_balance, code, data.clone(), 6_010_100, true)
        .expect_balance(100)
        .expect_count_out_msgs(0)
        .expect_storage_fees_due(due)
        .expect_credit(6_010_100)
        .expect_compute_result(0)
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
        data,
        20_000_000,
        true,
        result_balance,
        count_out_msgs,
    );
}

#[test]
fn test_sendrawmsg_message() {
    // can send value
    execute_sendrawmsg_message(0, 10_000_000, 48_667_458, 1);
    // cannot send small value, but can bounce
    execute_sendrawmsg_message(16, 0, 45_741_311, 1);
}
