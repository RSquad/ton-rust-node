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
    AccStatusChange, Account, AccountStatus, Coins, ComputeSkipReason, CurrencyCollection,
    GetRepresentationHash, InternalMessageHeader, MsgAddressInt, MsgAddressIntOrNone, OutAction,
    OutActions, Serializable, SliceData, StateInit, StorageUsed, TrActionPhase,
    TrBouncePhaseNofunds, TrBouncePhaseOk, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
    TrStoragePhase, Transaction, TransactionDescr, UInt256, DICT_HASH_MIN_CELLS, SENDMSG_ORDINARY,
};

fn send_message_to_freeze_and_immediately_unfreeze_account(
    acc: Account,
    state_init: &StateInit,
    bounce: bool,
) -> Account {
    let msg_income = 150000000 + 1000000000; // and not enough to not be frozen
    let due = 286774881;
    // send message to freeze and unfreeze account
    let mut msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);
    msg.set_state_init(state_init.clone());

    let result = execute_acc_with_message(acc, &msg);

    if bounce {
        result
            .expect_balance(0)
            .expect_count_out_msgs(1)
            .expect_status(AccountStatus::AccStateFrozen)
            .expect_storage_fees_due(due)
            .expect_credit(msg_income)
            .expect_compute_skipped(ComputeSkipReason::BadState)
            .expect_bounce_success();
    } else {
        result
            .expect_balance(700155119)
            .expect_count_out_msgs(2)
            .expect_status(AccountStatus::AccStateActive)
            .expect_credit(msg_income)
            .expect_storage_fees_due(0)
            .expect_compute_result(0)
            .expect_action_success(2)
            .expect_no_bounce();
    }
    result.acc
}

fn send_money_to_frozen_account(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 150000000;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let due = acc.due_payment().cloned().unwrap_or_default();
    let new_acc_balance = if bounce { 0.into() } else { Coins::from(msg_income) - due };
    let trans =
        execute_c(&msg, &mut acc, tr_lt, new_acc_balance, if bounce { 1 } else { 0 }).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::from_coins(if bounce { 0.into() } else { new_acc_balance }),
        BLOCK_LT + if bounce { 4 } else { 3 } + trans.msg_count() as u64,
        BLOCK_UT,
        if bounce { Some(due) } else { None },
        acc.frozen_hash().unwrap().clone(),
    );

    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        if bounce { 0.into() } else { due },
        if bounce { Some(due) } else { None },
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    let msg_fee = 3333282;
    if bounce {
        description.bounce = Some(TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsed::default(),
            msg_fees: msg_fee.into(),
            fwd_fees: 6666718.into(),
        }));

        let mut message = Message::with_int_header(InternalMessageHeader {
            ihr_disabled: true,
            bounce: false,
            bounced: true,
            src: MsgAddressIntOrNone::Some(
                MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
            ),
            dst: MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap(),
            value: CurrencyCollection::with_coins(140000000),
            extra_flags: Default::default(),
            fwd_fee: 6666718.into(),
            created_lt: 2000000004,
            created_at: 1576526553,
        });
        message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
        good_trans.add_out_message(&message).unwrap();
    } else {
        description.bounce = None;
    }

    good_trans.set_total_fees(CurrencyCollection::from_coins(if bounce {
        msg_fee.into()
    } else {
        due
    }));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + if bounce { 3 } else { 2 });
    good_trans.set_now(BLOCK_UT);

    let description = TransactionDescr::Ordinary(description);
    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn lead_account_into_even_more_debt(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 15;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let old_due = acc.due_payment().cloned().unwrap_or_default();
    acc.set_last_paid(acc.last_paid() - 100);
    let new_balance = if bounce { msg_income } else { 0 };
    let trans = execute_c(&msg, &mut acc, tr_lt, new_balance, 0).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    assert!(old_due < acc.due_payment().cloned().unwrap_or_default());

    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_coins(0),
        BLOCK_LT + if bounce { 4 } else { 3 },
        BLOCK_UT,
        Some(acc.due_payment().cloned().unwrap_or_default()),
        acc.frozen_hash().unwrap().clone(),
    );
    new_acc.set_balance(new_balance.into());
    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::new(if bounce { 0 } else { msg_income }),
        acc.due_payment().cloned(),
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);
    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(if bounce { 0 } else { msg_income }));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + if bounce { 3 } else { 2 });
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn send_message_to_freeze_account(mut acc: Account, start_balance: u64, bounce: bool) -> Account {
    let msg_income = 150000000;
    let storage_fee = 286774882;
    let total_balance = start_balance + if bounce { 0 } else { msg_income };
    let due_payment = storage_fee - start_balance - if bounce { 0 } else { msg_income };

    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);

    let state_init = acc.state_init().unwrap().clone();
    let frozen_hash = state_init.serialize().unwrap().repr_hash().clone();

    // send message to freeze account
    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::default(),
        BLOCK_LT + if bounce { 3 } else { 2 },
        BLOCK_UT,
        Some(due_payment.into()),
        frozen_hash,
    );
    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, if bounce { 1 } else { 0 }).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    assert_eq!(acc, new_acc);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 1).unwrap();
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: (total_balance).into(), // collect full balance as fee
        storage_fees_due: Some(Coins::from(due_payment)), // also due_payment credit for next transaction
        status_change: AccStatusChange::Frozen,           // freeze account
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_coins(msg_income),
    });

    if bounce {
        description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);
        description.bounce = Some(TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsed::default(),
            msg_fees: MSG_MINE_FEE.into(),
            fwd_fees: (MSG_FWD_FEE - MSG_MINE_FEE).into(),
        }));
        let h = InternalMessageHeader {
            ihr_disabled: true,
            bounce: false,
            bounced: true,
            src: MsgAddressIntOrNone::Some(
                MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
            ),
            dst: MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap(),
            value: CurrencyCollection::with_coins(msg_income - MSG_FWD_FEE),
            fwd_fee: (MSG_FWD_FEE - MSG_MINE_FEE).into(),
            created_lt: BLOCK_LT + 2,
            created_at: BLOCK_UT.into(),
            ..Default::default()
        };
        let message = Message::with_int_header_and_body(h, SliceData::from_raw(vec![0xff; 4], 32));
        good_trans.add_out_message(&message).unwrap();
        good_trans.set_total_fees(CurrencyCollection::with_coins(total_balance + MSG_MINE_FEE));
    } else {
        description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);
        description.bounce = None;
        good_trans.set_total_fees(CurrencyCollection::with_coins(total_balance));
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn unfreeze_account(mut acc: Account, state_init: &StateInit, bounce: bool) -> Account {
    let msg_income = 1000000000 + 150000000;
    let mut msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);

    let gas_used = 1307u32;
    let gas_fees = gas_used as u64 * 10000;

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;

    let acc_balance = acc.get_balance().unwrap().coins;

    // due collected
    let due = if bounce { 0.into() } else { acc.due_payment().cloned().unwrap_or_default() };
    let new_acc_balance =
        acc_balance + (msg_income - gas_fees - MSG1_BALANCE - MSG2_BALANCE) as u128 - due;
    let tr_lt = BLOCK_LT + 3;
    msg.set_state_init(state_init.clone());
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc_balance, 2).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(acc.last_tr_time().unwrap());
    new_acc.set_balance(CurrencyCollection::from_coins(new_acc_balance));
    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let (Some(int_header), Some(int_header2)) = (msg1.int_header_mut(), msg2.int_header_mut()) {
        int_header.value.coins = Coins::from(MSG1_BALANCE - msg_fwd_fee);
        int_header2.value.coins = Coins::from(MSG2_BALANCE - msg_fwd_fee);
        int_header.fwd_fee = msg_remain_fee.into();
        int_header2.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
        int_header2.created_at = BLOCK_UT.into();
        int_header.created_lt = acc.last_tr_time().unwrap() - 2;
        int_header2.created_lt = acc.last_tr_time().unwrap() - 1;
    }

    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: due,
        storage_fees_due: if bounce { acc.due_payment().cloned() } else { None },
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::from_coins(msg_income.into()),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = ((msg_income as u32 - due.as_u128() as u32) / 10000).into();
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
    action_ph.action_list_hash = actions.hash().unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg1).unwrap();
    append_message(&mut action_ph.tot_msg_size, &msg2).unwrap();

    description.action = Some(action_ph);
    description.credit_first = !bounce;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    good_trans.write_in_msg(Some(&msg)).unwrap();
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees(CurrencyCollection::from_coins(
        due + (gas_fees + msg_mine_fee * 2) as u128,
    ));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(acc.last_tr_time().unwrap() - 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn try_unfreeze_account_with_small_value(
    mut acc: Account,
    state_init: &StateInit,
    bounce: bool,
) -> Account {
    let due = acc.due_payment().cloned().unwrap_or_default();
    let msg_income = due + 1;
    let mut msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, bounce, BLOCK_LT);

    let new_acc_balance = 1;
    let tr_lt = BLOCK_LT + 3;
    msg.set_state_init(state_init.clone());
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc_balance, 0).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(tr_lt + 1);
    new_acc.set_balance(CurrencyCollection::with_coins(new_acc_balance));
    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: due,
        storage_fees_due: if bounce { Some(due) } else { None },
        status_change: AccStatusChange::Unchanged,
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::from_coins(msg_income),
    });
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);
    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    good_trans.write_in_msg(Some(&msg)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::from_coins(due));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn test_freeze_and_unfreeze_account(bounce: bool) {
    let start_balance = 1; // not enough to pay storage
    let acc_id = SENDER_ACCOUNT.clone();
    let acc = create_test_account(
        start_balance,
        acc_id,
        create_send_two_messages_code(),
        create_two_messages_data(),
    );
    let state_init = acc.state_init().unwrap().clone();

    send_message_to_freeze_and_immediately_unfreeze_account(acc.clone(), &state_init, bounce);

    let acc_frozen = send_message_to_freeze_account(acc, start_balance, bounce);
    assert!(acc_frozen.is_frozen());
    if !bounce {
        // TODO: account will be unfrozen for bounce message
        try_unfreeze_account_with_small_value(acc_frozen.clone(), &state_init, bounce);
        // account will remain frozen
    }

    unfreeze_account(acc_frozen.clone(), &state_init, bounce);

    lead_account_into_even_more_debt(acc_frozen.clone(), bounce);

    let acc_frozen = send_money_to_frozen_account(acc_frozen, bounce);
    unfreeze_account(acc_frozen, &state_init, bounce);
}

#[test]
fn test_freeze_and_unfreeze_account_without_bounce() {
    test_freeze_and_unfreeze_account(false);
}

#[test]
fn test_freeze_and_unfreeze_account_with_bounce() {
    test_freeze_and_unfreeze_account(true);
}

fn delete_frozen_account(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_id().unwrap().clone();
    let msg_income = 15;
    let due = 1262901841;
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_addr.clone(), msg_income, bounce, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce { 15 } else { 0 }, 0).unwrap();

    let new_acc = if bounce {
        let mut new_acc = Account::with_address_and_ballance(
            &MsgAddressInt::with_standart(None, -1, acc_addr).unwrap(),
            &msg_income.into(),
        );
        new_acc.set_last_paid(acc.last_paid());
        new_acc.set_last_tr_time(BLOCK_LT + 4);
        new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
        new_acc
    } else {
        Account::default()
    };
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::new(if bounce { 0 } else { msg_income }),
        Some(Coins::new(due + if bounce { msg_income } else { 0 })),
        AccStatusChange::Deleted,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_coins(if bounce { 0 } else { msg_income }));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(if bounce {
        AccountStatus::AccStateUninit
    } else {
        AccountStatus::AccStateNonexist
    });
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn delete_account_and_get_bounced_message(mut acc: Account) -> Account {
    let acc_address = acc.get_id().unwrap().clone();
    let msg_income = 150000000000000000;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), acc_address.clone(), msg_income, true, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 1).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        Coins::zero(),
        Some(1262901856u64.into()),
        AccStatusChange::Deleted,
    ));
    description.credit_ph =
        Some(TrCreditPhase::new(CurrencyCollection::with_coins(150000000000000000)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = false;
    description.bounce = Some(TrBouncePhase::Ok(TrBouncePhaseOk {
        msg_size: StorageUsed::default(),
        msg_fees: 3333282u64.into(),
        fwd_fees: 6666718u64.into(),
    }));
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut message = Message::with_int_header(InternalMessageHeader {
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(
            MsgAddressInt::with_standart(None, -1, acc_address).unwrap(),
        ),
        dst: MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap(),
        value: CurrencyCollection::with_coins(149999999990000000),
        extra_flags: Default::default(),
        fwd_fee: 6666718u64.into(),
        created_lt: 2000000004,
        created_at: 1576526553,
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&message).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_coins(3333282));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn delete_and_immediately_restore_account(mut acc: Account, state_init: &StateInit) -> Account {
    let msg_income = 150000000000000000;
    let mut msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SliceData::from(state_init.hash().unwrap()),
        msg_income,
        true,
        BLOCK_LT,
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 1).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    assert_eq!(get_tr_descr(&trans).storage_ph.unwrap().status_change, AccStatusChange::Deleted);
    assert_eq!(trans.end_status, AccountStatus::AccStateNonexist);

    acc
}

fn try_delete_active_account(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 15;
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        acc.get_id().unwrap().clone(),
        msg_income,
        bounce,
        BLOCK_LT,
    );

    let acc_balance_before = acc.balance().unwrap().coins;

    let due = 1664733855;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce { msg_income } else { 0 }, 0).unwrap();

    let mut new_acc = Account::uninit(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_coins(if bounce { msg_income } else { 0 }),
        BLOCK_LT + 3,
        BLOCK_UT,
    );
    new_acc.set_due_payment(Some(Coins::from(due + if bounce { msg_income } else { 0 })));
    new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance_before + if bounce { 0 } else { msg_income as u128 },
        Some(Coins::from(due + if bounce { msg_income } else { 0 })),
        AccStatusChange::Frozen,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(15)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_coins(
        acc_balance_before + if bounce { 0 } else { msg_income as u128 },
    ));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn delete_uninit_account(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_id().unwrap().clone();
    let msg_income = 15;
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_addr.clone(), msg_income, bounce, BLOCK_LT);

    let acc_balance = acc.balance().unwrap().coins;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce { msg_income } else { 0 }, 0).unwrap();

    let new_acc = if bounce {
        let mut new_acc = Account::with_address_and_ballance(
            &MsgAddressInt::with_standart(None, -1, acc_addr).unwrap(),
            &msg_income.into(),
        );
        new_acc.set_last_paid(acc.last_paid());
        new_acc.set_last_tr_time(BLOCK_LT + 4);
        new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
        new_acc
    } else {
        Account::default()
    };
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce { 0 } else { msg_income as u128 },
        Some((2650451629 + if bounce { msg_income } else { 0 }).into()),
        AccStatusChange::Deleted,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_coins(
        acc_balance + if bounce { 0 } else { msg_income as u128 },
    ));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(if bounce {
        AccountStatus::AccStateUninit
    } else {
        AccountStatus::AccStateNonexist
    });
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

fn delete_uninit_account_with_small_due(mut acc: Account, bounce: bool) {
    let msg_income = 15;
    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        acc.get_id().unwrap().clone(),
        msg_income,
        bounce,
        BLOCK_LT,
    );

    let acc_balance = acc.balance().unwrap().coins;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce { msg_income } else { 0 }, 0).unwrap();

    let due = 8803;
    let new_acc = Account::default();
    acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce { 0 } else { msg_income as u128 },
        Some((due + if bounce { msg_income } else { 0 }).into()),
        AccStatusChange::Unchanged,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsed::default(),
            req_fwd_fees: 10000000.into(),
        }));
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_coins(acc_balance + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
}

fn test_delete_account(bounce: bool) {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());

    let acc_id = SliceData::from(state_init.hash().unwrap());
    let acc_frozen = Account::frozen_standard(
        acc_id.clone(),
        0,
        BLOCK_LT + 3,
        BLOCK_UT - 100000000,
        1000000,
        UInt256::default(),
    );

    if !bounce {
        // it doesn't work in cpp node with bounce
        delete_frozen_account(acc_frozen.clone(), bounce);
    }
    delete_account_and_get_bounced_message(acc_frozen.clone());
    if !bounce {
        // account cannot be deleted in cpp node with bounce
        delete_and_immediately_restore_account(acc_frozen, &state_init);
    }

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    let acc_active =
        Account::active_standard(acc_id.clone(), 17, 0, BLOCK_UT - 100000000, state_init);

    try_delete_active_account(acc_active, bounce); // account will be frozen instead delete

    let acc_uninit =
        Account::uninit_standard(acc_id.clone(), 17, BLOCK_LT + 3, BLOCK_UT - 300000000);

    if !bounce {
        // cannot delete account with bounce and small value
        delete_uninit_account(acc_uninit, bounce);
    }

    let acc_uninit = Account::uninit_standard(acc_id, 17, BLOCK_LT + 3, BLOCK_UT - 1000);

    if !bounce {
        // cannot delete account with bounce and small value
        delete_uninit_account_with_small_due(acc_uninit, bounce); // account will be deleted
    }
}

#[test]
fn test_delete_account_without_bounce() {
    test_delete_account(false);
}

#[test]
fn test_delete_account_with_bounce() {
    test_delete_account(true);
}

fn delete_frozen_account_and_bounce(mut acc: Account) -> Account {
    let msg_income = 1500000000000000;
    let msg_balance_before = acc.balance().unwrap().coins;
    let msg =
        create_int_msg(THIRD_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), msg_income, true, BLOCK_LT);

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 1).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        msg_balance_before,
        Some(1275108872u64.into()),
        AccStatusChange::Deleted,
    ));
    description.credit_ph = Some(TrCreditPhase::new(CurrencyCollection::with_coins(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    let msg_fee = 3333282u64;
    description.bounce = Some(TrBouncePhase::Ok(TrBouncePhaseOk {
        msg_size: StorageUsed::default(),
        msg_fees: msg_fee.into(),
        fwd_fees: 6666718u64.into(),
    }));

    let mut good_trans =
        Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    let mut message = Message::with_int_header(InternalMessageHeader {
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(
            MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        ),
        dst: MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap(),
        value: CurrencyCollection::with_coins(1499999990000000),
        extra_flags: Default::default(),
        fwd_fee: 6666718u64.into(),
        created_lt: 2000000004,
        created_at: 1576526553,
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
    good_trans.add_out_message(&message).unwrap();

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_coins(3333297));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();
    compare_transaction(&trans, &good_trans);
    acc
}

#[test]
fn delete_account_and_bounce_message() {
    let acc_id = SENDER_ACCOUNT.clone();
    let acc_frozen = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap(),
        CurrencyCollection::with_coins(15),
        BLOCK_LT + 3,
        BLOCK_UT - 100000000,
        Some(1000000.into()),
        UInt256::default(),
    );

    delete_frozen_account_and_bounce(acc_frozen);
}

#[test]
fn test_send_state_init_in_ext_msg_to_active_acc() {
    let code = compile_code_to_cell("ACCEPT").unwrap();

    let mut msg = create_test_external_msg();
    let state_init = StateInit::with_code_and_data(code.clone(), Default::default());
    // let state_hash = state_init.serialize().unwrap().repr_hash();
    msg.set_state_init(state_init.clone());

    let addr = SENDER_ACCOUNT.clone();
    let balance = 1_000_000_000;

    let mut acc = Account::uninit_standard(addr.clone(), balance, 0, BLOCK_UT);
    execute(&msg, &mut acc, BLOCK_LT + 1)
        .expect_err("Must fail with no compute phase with bad state");

    let mut acc = Account::active_standard(addr.clone(), balance, 0, BLOCK_UT, state_init);
    execute(&msg, &mut acc, BLOCK_LT + 1)
        .expect_err("Must fail with no compute phase with bad state");

    let mut acc =
        Account::frozen_standard(addr.clone(), balance, 0, BLOCK_UT, 0, UInt256::default());
    execute(&msg, &mut acc, BLOCK_LT + 1)
        .expect_err("Must fail with no compute phase with bad state");
}
