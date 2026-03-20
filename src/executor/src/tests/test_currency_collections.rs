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
use super::*;
use common::*;
use pretty_assertions::assert_eq;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    AccStatusChange, Account, AccountId, AccountStatus, ComputeSkipReason, CurrencyCollection,
    GetRepresentationHash, Coins, InternalMessageHeader, Message, MsgAddressInt,
    MsgAddressIntOrNone, OutAction, OutActions, Serializable, SliceData, StateInit, StorageUsed,
    TrActionPhase, TrBouncePhaseNofunds, TrBouncePhaseOk, TrComputePhase, TrComputePhaseVm,
    TrCreditPhase, TrStoragePhase, Transaction, TransactionDescr, VarUInteger32,
    DICT_HASH_MIN_CELLS, SENDMSG_PAY_FEE_SEPARATELY, SENDMSG_REMAINING_MSG_BALANCE,
};

fn create_msg_currency_workchain(
    w_id: i8,
    dest: AccountId,
    currencies: &CurrencyCollection,
    bounce: bool,
) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, w_id, THIRD_ACCOUNT.clone()).unwrap(),
        MsgAddressInt::with_standart(None, w_id, dest).unwrap(),
        currencies.clone(),
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.created_lt = BLOCK_LT - 2;
    hdr.created_at = 0;
    Message::with_int_header(hdr)
}

fn create_msg_currency(dest: AccountId, currencies: &CurrencyCollection, bounce: bool) -> Message {
    create_msg_currency_workchain(0, dest, currencies, bounce)
}

pub fn check_account_and_transaction_with_currencies(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
    result_account_balance: &CurrencyCollection,
    count_out_msgs: usize,
) {
    if let Some(trans) = trans {
        assert_eq!(
            (
                trans.out_msgs.len().unwrap(),
                &acc_after.balance().unwrap().coins,
                acc_after.balance().unwrap().other.get(&11111111u32).unwrap()
            ),
            (
                count_out_msgs,
                &result_account_balance.coins,
                result_account_balance.other.get(&11111111u32).unwrap()
            ),
            "balance after {:?}",
            acc_after.balance().unwrap()
        );
    }
    check_account_and_transaction_balances(acc_before, acc_after, msg, trans);
}

pub fn execute_currencies(
    msg: &Message,
    acc: &mut Account,
    tr_lt: u64,
    result_account_balance: &CurrencyCollection,
    count_out_msgs: usize,
) -> Result<Transaction> {
    let acc_before = acc.clone();
    let trans = execute(msg, acc, tr_lt);
    check_account_and_transaction_with_currencies(
        &acc_before,
        acc,
        msg,
        trans.as_ref().ok(),
        result_account_balance,
        count_out_msgs,
    );
    trans
}

#[test]
fn test_currency_collection_uninit_account() {
    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::with_coins(1_400_200_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from(123000)).unwrap();

    let msg = create_msg_currency(SENDER_ACCOUNT.clone(), &currencies, false);

    let tr_lt = BLOCK_LT + 1;

    let result_currencies = currencies.clone();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_currency_collection_message_with_zero_coins() {
    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::default();
    currencies.other.set(&11111111u32, &VarUInteger32::from(123000)).unwrap();

    let msg = create_msg_currency(SENDER_ACCOUNT.clone(), &currencies, false);

    let tr_lt = BLOCK_LT + 1;

    let result_currencies = currencies.clone();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_currency_collection_activate_account() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::with_coins(1_400_200_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from(123000)).unwrap();

    let mut msg =
        create_msg_currency(AccountId::from(state_init.hash().unwrap()), &currencies, false);
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_coins(1_400_100_000);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from(123000)).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_balance_instruction_with_currency() {
    let begin_extra = 123000;
    let message_extra = 100000;

    let code = &*format!(
        "
        PUSHINT  11111111
        BALANCE SECOND
        PUSHINT 32
        DICTUGET

        PUSHCONT {{
            LDVARUINT32
            DROP
            PUSHINT {}
            CMP
            THROWIFNOT 13
        }}
        PUSHCONT {{
            THROW 12
        }}
        IFELSE
    ",
        begin_extra + message_extra
    );

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_coins(1_400_200_000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        begin_currencies.clone(),
        0,
        10,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let mut currencies = CurrencyCollection::with_coins(1_000_000_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from(message_extra)).unwrap();

    let mut msg =
        create_msg_currency(AccountId::from(state_init.hash().unwrap()), &currencies, false);
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_coins(2367739615);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from(223000)).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
    let vm_phase = get_tr_descr(&trans).compute_ph;
    if let TrComputePhase::Vm(vm) = vm_phase {
        assert_eq!(vm.exit_code, 13);
    } else {
        unreachable!();
    }
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_add_currency_collection_and_activate_account() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_coins(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        begin_currencies.clone(),
        10,
        10,
    );

    let mut currencies = CurrencyCollection::with_coins(1_000_000_000);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from(1230000)).unwrap();

    let mut msg =
        create_msg_currency(AccountId::from(state_init.hash().unwrap()), &currencies, false);
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_coins(2385594300);
    result_currencies
        .other
        .set(&11111111u32, &VarUInteger32::from(begin_extra + new_extra))
        .unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

fn add_currency_collection_to_active_account_with_bounce_flag(bounce: bool) {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_coins(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        begin_currencies.clone(),
        0,
        10,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let mut currencies = CurrencyCollection::with_coins(1_000_000_000);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from(1230000)).unwrap();

    let mut msg =
        create_msg_currency(AccountId::from(state_init.hash().unwrap()), &currencies, bounce);
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_coins(2373277687);
    result_currencies
        .other
        .set(&11111111u32, &VarUInteger32::from(begin_extra + new_extra))
        .unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_add_currency_collection_to_active_account() {
    add_currency_collection_to_active_account_with_bounce_flag(false);
    add_currency_collection_to_active_account_with_bounce_flag(true);
}

#[test]
fn test_cannot_delete_uninit_account_with_currency_collections_without_bounce() {
    let mut begin_currencies = CurrencyCollection::with_coins(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        begin_currencies.clone(),
        BLOCK_LT + 3,
        BLOCK_UT - 300000000,
    );

    let acc_addr = acc.get_id().unwrap().clone();
    let mut currencies = CurrencyCollection::with_coins(15);
    let add_extra = 456000;
    currencies.other.set(&11111111u32, &VarUInteger32::from(add_extra)).unwrap();
    let msg = create_msg_currency_workchain(-1, acc_addr, &currencies, false);

    let acc_balance = acc.balance().unwrap().coins;
    let mut new_currencies = CurrencyCollection::default();
    new_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra + add_extra)).unwrap();
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &new_currencies, 0).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        currencies.coins + acc_balance,
        Some(Coins::from(2650451629)),
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let sum_fees = CurrencyCollection::from_coins(currencies.coins + acc_balance);
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_cannot_delete_uninit_account_with_currency_collections_with_bounce() {
    let mut begin_currencies = CurrencyCollection::with_coins(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        begin_currencies.clone(),
        BLOCK_LT + 3,
        BLOCK_UT - 300000000,
    );

    let acc_addr = acc.get_id().unwrap().clone();
    let mut currencies = CurrencyCollection::with_coins(15);
    let add_extra = 456000;
    currencies.other.set(&11111111u32, &VarUInteger32::from(add_extra)).unwrap();
    let msg = create_msg_currency_workchain(-1, acc_addr, &currencies, true);

    let acc_balance = acc.balance().unwrap().coins;
    let mut new_currencies = CurrencyCollection::with_coins(15);
    new_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra + add_extra)).unwrap();
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &new_currencies, 0).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance,
        Some(Coins::from(2650451644)),
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = false;
    description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
        msg_size: StorageUsed::with_values_checked(1, 69).unwrap(),
        req_fwd_fees: Coins::from(11690000),
    }));
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let sum_fees = CurrencyCollection::from_coins(begin_currencies.coins);
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_freeze_uninit_account_with_currency_collections() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_coins(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        begin_currencies.clone(),
        0,
        BLOCK_UT - 30000000,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let acc_addr = acc.get_id().unwrap().clone();
    let mut currencies = CurrencyCollection::with_coins(15);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from(new_extra)).unwrap();
    let msg = create_msg_currency_workchain(-1, acc_addr, &currencies, false);

    let acc_balance = acc.balance().unwrap().coins;
    let tr_lt = BLOCK_LT + 2;
    let mut new_currency = begin_currencies.clone();
    new_currency.add(&currencies).unwrap();
    new_currency.coins = Coins::default();
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &new_currency, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        currencies.coins + acc_balance,
        Some(Coins::from(499420135)),
        AccStatusChange::Frozen,
    ));
    description.credit_ph = Some(TrCreditPhase::new(currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let sum_fees = CurrencyCollection::from_coins(begin_currencies.coins + currencies.coins);
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_bounce_with_currency_collection() {
    let mut begin_currencies = CurrencyCollection::with_coins(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        begin_currencies.clone(),
        10,
        10,
    );

    let mut currencies = CurrencyCollection::with_coins(1_000_000_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from(1230000)).unwrap();

    let msg = create_msg_currency(SENDER_ACCOUNT.clone(), &currencies, true);

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_coins(1385694300);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from(begin_extra)).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 1).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph =
        Some(TrStoragePhase::with_params(Coins::from(14505700), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::new(currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = false;

    let msg_fee = 389660;
    let fwd_fees = 779340;
    description.bounce = Some(TrBouncePhase::Ok(TrBouncePhaseOk {
        msg_size: StorageUsed::with_values_checked(1, 69).unwrap(),
        msg_fees: Coins::from(msg_fee),
        fwd_fees: Coins::from(fwd_fees),
    }));

    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let mut outmsg_currency = currencies.clone();
    outmsg_currency.coins -= Coins::from(msg_fee + fwd_fees);
    let mut message = Message::with_int_header(InternalMessageHeader {
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(
            MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
        ),
        dst: MsgAddressInt::with_standart(None, 0, THIRD_ACCOUNT.clone()).unwrap(),
        value: outmsg_currency,
        extra_flags: Default::default(),
        fwd_fee: fwd_fees.into(),
        created_lt: BLOCK_LT + 2,
        created_at: BLOCK_UT.into(),
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
    good_trans.add_out_message(&message).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(14895360));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);

    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_currencies_with_sendmsg() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 1
        SENDRAWMSG
    ";

    let mut out_msg_currencies = CurrencyCollection::with_coins(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(120)).unwrap();

    let mut out_msg = create_msg_currency(THIRD_ACCOUNT.clone(), &out_msg_currencies, false);
    out_msg.set_src(MsgAddressIntOrNone::Some(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
    ));
    let data = out_msg.serialize().unwrap();

    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_coins(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(100)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, 0, acc_id).unwrap(),
        begin_currencies.clone(),
        0,
        10,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let mut in_msg_currencies = CurrencyCollection::with_coins(1000000000);
    in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(100)).unwrap();

    let msg = create_msg_currency(SENDER_ACCOUNT.clone(), &in_msg_currencies, false);

    let mut currencies_result = CurrencyCollection::with_coins(1828709098);
    currencies_result.other.set(&11111111u32, &VarUInteger32::from(80)).unwrap();

    let trans = execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, 1).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 69689902;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(in_msg_currencies.clone()));

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 601.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 601000.into();
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.fwd_fee = 666672.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 1;
    action_ph.total_fwd_fees = Some(1000000.into());
    action_ph.total_action_fees = Some(333328.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&out_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(70624230));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

/// account balance is 2000000000ng and 11111111:100 extra
/// input message with variable value
/// sends message with 1100000000ng and 11111111:120 extra
/// expects to receive value and out message count
fn execute_sendrawmsg_message(
    mode: u8,
    send_value_coins: u64,
    send_value_tokens: u128,
    result_balance_coins: u64,
    result_balance_tokens: u128,
    count_out_msgs: usize,
) -> Account {
    let code = format!(
        "
        ACCEPT
        PUSHCTR C4
        PUSHINT {}
        SENDRAWMSG
    ",
        mode
    );

    let mut out_msg_currencies = CurrencyCollection::with_coins(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(120)).unwrap();

    let mut out_msg = create_msg_currency(THIRD_ACCOUNT.clone(), &out_msg_currencies, false);
    out_msg.set_src(MsgAddressIntOrNone::None);
    let data = out_msg.serialize().unwrap();

    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(&code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_coins(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(100)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap(),
        begin_currencies.clone(),
        0,
        10,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let mut in_msg_currencies = CurrencyCollection::from_coins(send_value_coins.into());
    if send_value_tokens != 0 {
        in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(send_value_tokens)).unwrap();
    }

    let msg = create_msg_currency(acc_id, &in_msg_currencies, false);

    let mut currencies_result = CurrencyCollection::with_coins(result_balance_coins);
    currencies_result.other.set(&11111111u32, &VarUInteger32::from(result_balance_tokens)).unwrap();

    execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, count_out_msgs).unwrap();
    acc
}

#[test]
fn test_sendrawmsg_currency_messages() {
    execute_sendrawmsg_message(0, 1_400_000_000, 140, 2_236_083_908, 120, 1);
    execute_sendrawmsg_message(1, 1_400_000_000, 140, 2_235_083_908, 120, 1);
    execute_sendrawmsg_message(0, 0, 140, 1_936_684_908, 240, 0);
    execute_sendrawmsg_message(0, 1_400_000_000, 0, 3_336_083_908, 100, 0);

    execute_sendrawmsg_message(128, 1_400_000_000, 140, 0, 120, 1);
    let acc = execute_sendrawmsg_message(128 + 32, 1_400_000_000, 140, 0, 120, 1);
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);

    execute_sendrawmsg_message(64, 1_400_000_000, 100, 836492461, 80, 1);
}

#[test]
fn test_currencies_with_sendmsg_64_flag() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 65
        SENDRAWMSG
    ";
    // account with 2000000000ng and 11111111:150 extra
    // input message with 1000000000ng and 11111111:100 extra
    // sends message with 1100000000ng and 11111111:240 extra (2101`000000ng)

    let mut out_msg_currencies = CurrencyCollection::with_coins(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(240)).unwrap();

    let mut out_msg = create_msg_currency(THIRD_ACCOUNT.clone(), &out_msg_currencies, false);
    out_msg.set_src(MsgAddressIntOrNone::Some(
        MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap(),
    ));
    let data = out_msg.serialize().unwrap();

    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_coins(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from(150)).unwrap();

    let mut acc = Account::active(
        MsgAddressInt::with_standart(None, 0, acc_id).unwrap(),
        begin_currencies.clone(),
        0,
        10,
        state_init.clone(),
        DICT_HASH_MIN_CELLS,
    )
    .unwrap();

    let mut in_msg_currencies = CurrencyCollection::with_coins(1000000000);
    in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from(100)).unwrap();

    let msg = create_msg_currency(SENDER_ACCOUNT.clone(), &in_msg_currencies, false);

    let mut currencies_result = CurrencyCollection::with_coins(828508651);
    currencies_result.other.set(&11111111u32, &VarUInteger32::from(10)).unwrap();

    let trans = execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, 1).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 69882349;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(in_msg_currencies.clone()));

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 609.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 609000.into();
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(
        SENDMSG_PAY_FEE_SEPARATELY + SENDMSG_REMAINING_MSG_BALANCE,
        out_msg.clone(),
    ));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value.coins.add(&in_msg_currencies.coins).unwrap();
        int_header.fwd_fee = 666672.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 1;
    action_ph.total_fwd_fees = Some(1000000.into());
    action_ph.total_action_fees = Some(333328.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &out_msg).unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&out_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(70824677));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_send_rawreserve_with_extra_messages_fail() {
    let start_balance = 1_602_586_890;
    let start_extra_balance = 1_542_586_890;
    let reserve = 1_080_012_743;
    let extra_reserve = 1_080_012_743;
    let result_account_balance = 1_316_306_139;
    let result_account_extra_balance = 1_542_586_890;
    let count_out_msgs = 0;
    let code = format!(
        "
        PUSHINT {}

        NEWC
        PUSHINT {}
        STVARUINT32
        PUSHINT 11111111
        NEWDICT
        PUSHINT 32
        DICTUSETB

        PUSHINT 0
        RAWRESERVEX

        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ",
        reserve, extra_reserve
    );
    let data = create_two_messages_data();
    execute_custom_transaction_with_extra_balance(
        start_balance,
        start_extra_balance,
        &code,
        data,
        41_000_000,
        result_account_balance,
        result_account_extra_balance,
        count_out_msgs,
    );
}
