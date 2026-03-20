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
use pretty_assertions::assert_eq;
use std::sync::LazyLock;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    AccountId, AccountStatus, BuilderData, Cell, Coins, CurrencyCollection, GetRepresentationHash,
    HashmapE, MsgAddressInt, Serializable, SliceData, StateInit, Status, TrComputePhase,
    Transaction, DICT_HASH_MIN_CELLS, SENDMSG_ORDINARY, SET_LIB_CODE_ADD_PRIVATE,
};

mod common;
use common::*;

static LIBRARY_CELL: LazyLock<Cell> = LazyLock::new(|| {
    let cell = compile_code_to_cell("TWO").unwrap();
    assert_eq!(
        cell.repr_hash().as_hex_string(),
        "d816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf8f"
    );
    cell
});

fn create_test_code_with_using_libs() -> Cell {
    compile_code_to_cell(
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
        NEWC
        PUSHINT 2
        STUR 8
        PUSHSLICE xd816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf8f
        STSLICER
        TRUE
        ENDXC
        DUP
        CTOS
        DROP
        CTOS
        BLESS
        POP C0
    ",
    )
    .unwrap()
}

#[test]
fn test_trexecutor_active_acc_with_code_with_libs_in_msg() {
    let mut test_case = TransactionTestCase {
        gas_limit: Some(12300),
        gas_used: 2247,
        lt_delta: 4,
        msg_income: 123_000_000,
        no_last_paid: true,
        start_balance: 2_000_000_000,
        storage_fee: 316_562_767,
        ..Default::default()
    };
    let (msg1, msg2) = test_case.expect_two_out_messages(SENDMSG_ORDINARY);
    test_case.expect_compute_vm_success(25);
    test_case.expect_action_success_with_two_messages(&msg1, &msg2);
    test_case.expect_total_fees(test_case.storage_fee + test_case.gas_fees() + MSG_MINE_FEE * 2);
    test_case.expect_end_balance(
        test_case.start_balance + test_case.msg_income
            - (MSG1_BALANCE + MSG2_BALANCE + test_case.storage_fee + test_case.gas_fees()),
    );

    let code = create_test_code_with_using_libs();
    let data = create_two_messages_data();
    let mut msg = create_int_msg(
        RECEIVER_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        test_case.msg_income,
        false,
        BLOCK_LT - 1_000_000 + 1,
    );
    let mut state_init = StateInit::default();
    state_init.set_library_code(LIBRARY_CELL.to_owned(), true).unwrap();
    msg.set_state_init(state_init);

    let mut test_ctx = TransactionTestContext::with_params(code, data, Some(msg), &test_case);
    let trans = test_ctx.execute(2).unwrap();

    let mut good_trans = test_ctx.create_sample_transaction(test_case);
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn test_trexecutor_active_acc_with_code_with_libs_in_state() {
    let mut test_case = TransactionTestCase {
        gas_credit: Some(10000),
        gas_limit: Some(0),
        gas_used: 2247,
        lt_delta: 4,
        no_last_paid: true,
        start_balance: 2_000_000_000,
        storage_fee: 401_405_046,
        ..Default::default()
    };
    let (msg1, msg2) = test_case.expect_two_out_messages(SENDMSG_ORDINARY);
    test_case.expect_compute_vm_success(25);
    test_case.expect_action_success_with_two_messages(&msg1, &msg2);
    test_case.expect_total_fees(
        MSG_FWD_FEE + test_case.storage_fee + test_case.gas_fees() + MSG_MINE_FEE * 2,
    );
    test_case.expect_end_balance(
        test_case.start_balance
            - (MSG1_BALANCE
                + MSG2_BALANCE
                + MSG_FWD_FEE
                + test_case.storage_fee
                + test_case.gas_fees()),
    );

    let code = create_test_code_with_using_libs();
    let data = create_two_messages_data();
    let msg = create_test_external_msg();

    let mut test_ctx = TransactionTestContext::with_params(code, data, Some(msg), &test_case);
    test_ctx.set_library(LIBRARY_CELL.clone(), true);
    let trans = test_ctx.execute(2).unwrap();

    let mut good_trans = test_ctx.create_sample_transaction(test_case);
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    compare_transaction(&trans, &good_trans);
}

#[test]
fn set_library_test() {
    // library code and data
    let new_code = "ACCEPT";
    let mut state = StateInit::default();
    state.set_code(compile_code_to_cell(new_code).unwrap());

    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 1
        SETLIBCODE
    ";

    let code = compile_code_to_cell(code).unwrap();
    let start_balance = 1_000_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc =
        create_test_account(start_balance, acc_id.clone(), code, state.serialize().unwrap());
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 20_000_000, false, BLOCK_LT - 2);

    let tr_lt = BLOCK_LT + 1;
    assert_eq!(acc.libraries().len().unwrap(), 0);
    let trans = execute(&msg, &mut acc, tr_lt).unwrap();
    assert_eq!(trans.out_msgs.len().unwrap(), 0);
    assert_eq!(acc.libraries().len().unwrap(), 1);

    /*
    let code = format!("
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SETLIBCODE
    ");
    acc.set_code(compile_code_to_cell(code).unwrap());
    let trans = executor.execute_for_account(Some(&msg), &mut acc, HashmapE::default(), BLOCK_UT, BLOCK_LT, lt, true).unwrap();
    assert_eq!(acc.libraries().len().unwrap(), 0);
    */
}

#[ignore]
#[test]
fn set_ext_library_test() {
    // library code and data
    let mut state_lib = HashmapE::with_bit_len(256);
    state_lib.setref(LIBRARY_CELL.repr_hash().into(), &LIBRARY_CELL).unwrap();

    let code = format!(
        "
        ACCEPT
        PUSHCTR C4
        HASHCU
        PUSHINT {}
        CHANGELIB
    ",
        SET_LIB_CODE_ADD_PRIVATE
    );

    let code = compile_code_to_cell(&code).unwrap();
    let start_balance = 1_000_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, LIBRARY_CELL.to_owned());
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 20_000_000, false, BLOCK_LT - 2);
    let msg_cell = msg.serialize().unwrap();

    let tr_lt = BLOCK_LT + 1;
    assert_eq!(acc.libraries().len().unwrap(), 0);
    let mut params = execute_params(tr_lt);
    params.state_libs = state_lib;
    let trans =
        execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), &mut acc, &params).unwrap();
    assert_eq!(trans.out_msgs.len().unwrap(), 0);
    assert_eq!(acc.libraries().len().unwrap(), 1);
}

#[test]
fn set_code_test() {
    let code = "
        ACCEPT
        PUSHCTR C4
        SETCODE
    ";
    let new_code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let code = compile_code_to_cell(code).unwrap();
    let new_code = compile_code_to_cell(new_code).unwrap();
    let start_balance = 500_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, new_code);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 20_000_000, false, BLOCK_LT - 2);

    // set new code send tx
    let tr_lt = BLOCK_LT + 1;
    execute_c(&msg, &mut acc, tr_lt, 400404216, 0).unwrap();

    // set new data for code send tx
    let out_msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        THIRD_ACCOUNT.clone(),
        11_000_000,
        false,
        BLOCK_LT + 1,
    );
    let data = out_msg.serialize().unwrap();
    acc.set_data(data);
    acc.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();

    // run send tx code
    execute_c(&msg, &mut acc, tr_lt, 403394216, 1).unwrap();
}

#[test]
fn set_code_unsuccess_test() {
    let code = "
        ACCEPT
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHINT 1000000000
        PUSHINT 0
        RAWRESERVE
    ";
    let new_code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let code = compile_code_to_cell(code).unwrap();
    let new_code = compile_code_to_cell(new_code).unwrap();
    let start_balance = 500_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), new_code);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 20_000_000, false, BLOCK_LT - 2);

    // set new code send tx
    let tr_lt = BLOCK_LT + 1;
    execute_c(&msg, &mut acc, tr_lt, 344060641, 0).unwrap();
    assert_eq!(acc.get_code().unwrap(), code);
}

#[test]
fn test_my_code() {
    let code = "
        ACCEPT
        MYCODE
        DUP
        CTOS
        DROP
        CTOS
        SBITS
        THROWIF 62
    ";
    let start_balance = 200_000_000;

    let acc_id = SENDER_ACCOUNT.clone();
    let in_msg = create_int_msg(acc_id.clone(), acc_id.clone(), 10_000_000, false, BLOCK_LT - 2);
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(SENDER_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);

    let acc_before = acc.clone();
    let trans = execute(&msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction_balances(&acc_before, &acc, &msg, None);
    let trans = trans.unwrap();

    let descr = trans.read_description().unwrap();
    if let TrComputePhase::Vm(vm) = descr.compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 62);
    } else {
        unreachable!()
    }
}

#[test]
fn test_library_cell_code() {
    cross_check::disable_cross_check(); // need to support mc_state_proof update
    let my_code = ton_assembler::compile_code_to_cell(
        "
        MYCODE
        CTOS ; 100 gas
        DROP
        MYCODE
        CTOS ; 25 gas
    ",
    )
    .unwrap();
    let mut library = ton_block::HashmapE::with_bit_len(256);
    let key = my_code.repr_hash().write_to_bitstring().unwrap();
    library.setref(key.clone(), &my_code).unwrap();

    let code = my_code.as_library_cell();
    let data = Cell::default();

    let mut acc = create_test_account(1_000_000_000, SENDER_ACCOUNT.clone(), code, data);
    let msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        1_000_000_000,
        false,
        BLOCK_LT - 2,
    );
    let msg_cell = msg.serialize().unwrap();

    let mut params = execute_params(BLOCK_LT + 1);
    params.state_libs = library;
    let trans =
        execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), &mut acc, &params).unwrap();
    let descr = trans.read_description().unwrap();
    let comp = descr.compute_phase_ref().unwrap();
    dbg!(&comp);
    assert_eq!(trans.gas_used().unwrap(), 236);
}

fn storage_fee_instruction(
    start_balance: u64,
    begin_due: u64,
    bounce: bool,
    storage_phase_due: bool,
) {
    let storage_fees_collected = 158514102;
    let code = format!(
        "
        ACCEPT
        STORAGEFEES
        PUSHINT {}
        CMP
        THROWIFNOT 62
    ",
        storage_fees_collected
    );

    let acc_id = SENDER_ACCOUNT.clone();
    let in_msg = create_int_msg(acc_id.clone(), acc_id.clone(), 10_000_000, false, BLOCK_LT - 2);
    let code = compile_code_to_cell(&code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    acc.set_due_payment(Some(Coins::from(begin_due)));

    let msg = create_int_msg(SENDER_ACCOUNT.clone(), acc_id, 200_000_000, bounce, BLOCK_LT - 2);

    let acc_before = acc.clone();
    let trans = execute(&msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction_balances(&acc_before, &acc, &msg, None);
    let trans = trans.unwrap();

    let descr = trans.read_description().unwrap();
    if let TrComputePhase::Vm(vm) = descr.compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 62, "{:#?}", descr);
    } else {
        panic!("wrong transaction desciption")
    }

    let storage_ph = get_tr_descr(&trans).storage_ph.unwrap();
    assert_eq!(storage_ph.storage_fees_due.is_some(), storage_phase_due);
    assert_eq!(
        Coins::from(storage_fees_collected),
        storage_ph.storage_fees_collected + storage_ph.storage_fees_due.unwrap_or_default()
    );
}

#[ignore = "need to investigate later"]
#[test]
fn test_storage_fee_instruction() {
    storage_fee_instruction(200_000_000, 0, false, false);
    storage_fee_instruction(200_000_000, 0, true, false);

    storage_fee_instruction(141782075, 0, false, false);
    storage_fee_instruction(141782075, 0, true, true);

    storage_fee_instruction(60000000, 1000, false, false);
    storage_fee_instruction(60000000, 1000, true, true);
}

#[test]
fn account_uninit_with_libs_disabled() {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(LIBRARY_CELL.to_owned(), true).unwrap();

    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_coins(10000000000000000000),
        10,
        10,
    );

    let mut msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999984738712408, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
}

#[test]
fn account_nonexist_with_libs_disabled() {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(LIBRARY_CELL.to_owned(), true).unwrap();

    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::default();

    let mut msg =
        create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 14200000000, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
}

#[test]
fn account_frozen_with_libs_disabled() {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(LIBRARY_CELL.to_owned(), true).unwrap();

    let frozen_hash = state_init.serialize().unwrap().repr_hash();
    let acc_id = SliceData::from(state_init.hash().unwrap());
    let mut acc = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::default(),
        BLOCK_LT + 2,
        BLOCK_UT,
        Some(136774881.into()),
        frozen_hash,
    );

    let mut msg =
        create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 14_200_000_000, false, BLOCK_LT - 2);
    msg.set_state_init(state_init);

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 14062225119, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
}

#[test]
fn test_simple_account_with_libs() {
    let mut test_case = TransactionTestCase {
        start_balance: 2_000_000_000,
        msg_income: 123_000_000,
        storage_fee: 373_212_941,
        gas_limit: Some(12300),
        gas_used: 1307,
        lt_delta: 4,
        no_last_paid: true,
        ..Default::default()
    };
    let (msg1, msg2) = test_case.expect_two_out_messages(SENDMSG_ORDINARY);
    test_case.expect_compute_vm_success(10);
    test_case.expect_action_success_with_two_messages(&msg1, &msg2);
    test_case.expect_total_fees(test_case.storage_fee + test_case.gas_fees() + MSG_MINE_FEE * 2);
    test_case.expect_end_balance(
        test_case.start_balance + test_case.msg_income
            - (MSG1_BALANCE + MSG2_BALANCE + test_case.storage_fee + test_case.gas_fees()),
    );

    let code = create_send_two_messages_code();
    let data = create_two_messages_data();
    let msg = test_case.create_int_msg(false);
    let mut test_ctx = TransactionTestContext::with_params(code, data, Some(msg), &test_case);
    test_ctx.set_library(LIBRARY_CELL.to_owned(), true);

    let trans = test_ctx.execute(2).unwrap();

    let mut good_trans = test_ctx.create_sample_transaction(test_case);
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    compare_transaction(&trans, &good_trans);
}

fn create_non_zero_level_cell() -> Result<Cell> {
    use ton_block::{CellType, IBitstring};

    let mut b = BuilderData::new();
    b.append_u8(1)?;
    b.append_u8(1)?;
    b.append_raw(&[0; 32], 256)?;
    b.append_u16(0)?;
    b.set_type(CellType::PrunedBranch);
    //b.set_level_mask(LevelMask::with_level(1));
    let c = b.into_cell()?;

    let mut b = BuilderData::new();
    b.checked_append_reference(c)?;
    b.into_cell()
}

fn check_aborted_and_zero_level(tr: &Transaction, acc: &Account) -> Status {
    assert!(tr.read_description()?.is_aborted(), "not aborted");
    assert!(acc.serialize()?.level() == 0, "non-zero level");
    Ok(())
}

fn run_account_non_zero_level(code: Cell, body: SliceData, external: bool, bounce: bool) -> Status {
    let acc_id = SENDER_ACCOUNT.clone();
    let mut acc = create_test_account(1_000_000_000, acc_id.clone(), code.clone(), Cell::default());
    let orig_acc = acc.clone();
    let tr_lt = BLOCK_LT + 1;

    let mut msg = if external {
        create_test_external_msg()
    } else {
        create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 1_000_000_000, bounce, BLOCK_LT - 2)
    };
    msg.set_body(body);
    let trans = execute(&msg, &mut acc, tr_lt)?;
    assert_eq!(acc.status(), orig_acc.status());
    assert_eq!(acc.state_init().cloned(), orig_acc.state_init().cloned());
    check_aborted_and_zero_level(&trans, &acc)
}

#[derive(Debug, PartialEq)]
enum Mode {
    Uninit,
    Frozen,
    Empty,
}

fn run_account_non_zero_level_stateinit(
    state_init: StateInit,
    mode: &Mode,
    external: bool,
    bounce: bool,
) -> Status {
    let acc_id = SliceData::from_raw(state_init.hash()?.as_slice().to_vec(), 256);
    let acc_addr = MsgAddressInt::with_standart(None, -1, acc_id.clone())?;
    let acc = match mode {
        Mode::Frozen => Account::frozen(
            acc_addr,
            CurrencyCollection::default(),
            BLOCK_LT + 2,
            BLOCK_UT,
            Some(100_000_000.into()),
            state_init.hash()?,
        ),
        Mode::Uninit => Account::uninit(
            acc_addr,
            CurrencyCollection::with_coins(1_000_000_000_000),
            BLOCK_LT + 3,
            BLOCK_UT - 300000000,
        ),
        Mode::Empty => Account::default(),
    };
    let mut msg = if external {
        create_test_external_msg_with_address(acc_id)
    } else {
        create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 1_000_000_000, bounce, BLOCK_LT - 2)
    };
    msg.set_state_init(state_init);

    let res = execute_acc_with_message(acc.clone(), &msg);
    if external && mode == &Mode::Uninit {
        res.expect_no_accept();
    } else {
        assert!(acc.serialize()?.level() == 0, "non-zero level");
        res.expect_status(AccountStatus::AccStateActive);
    };
    Ok(())
}

#[test]
fn test_account_non_zero_level() -> Status {
    let poison = create_non_zero_level_cell()?;

    let save_data = compile_code_to_cell(
        "
        DROP LDREF DROP
        POPCTR c4
        ACCEPT
    ",
    )
    .unwrap();
    let setcode = compile_code_to_cell(
        "
        DROP LDREF DROP
        SETCODE
        ACCEPT
    ",
    )
    .unwrap();
    let setlibcode = compile_code_to_cell(
        "
        DROP LDREF DROP
        PUSHINT 1
        SETLIBCODE
        ACCEPT
    ",
    )
    .unwrap();

    let body = SliceData::load_cell_ref(&poison)?;
    for code in [save_data, setcode, setlibcode] {
        run_account_non_zero_level(code.clone(), body.clone(), false, false)?;
        run_account_non_zero_level(code.clone(), body.clone(), false, true)?;
        run_account_non_zero_level(code.clone(), body.clone(), true, false)?;
    }

    let check_with_statinit = |mode: &Mode, external: bool, bounce: bool| -> Status {
        let mut state_init = StateInit::default();
        let mut b = BuilderData::from_cell(&compile_code_to_cell("PUSHREF DROP").unwrap())?;
        b.checked_append_reference(poison.clone())?;
        state_init.set_code(b.into_cell()?);
        run_account_non_zero_level_stateinit(state_init, mode, external, bounce)?;

        let state_init = StateInit::with_code_and_data(Cell::default(), poison.clone());
        run_account_non_zero_level_stateinit(state_init, mode, external, bounce)?;

        // message with libraries is not allowed
        // let mut libs = HashmapE::with_bit_len(256);
        // let key = poison.repr_hash().into();
        // libs.setref(key, &poison)?;

        // let mut state_init = StateInit::default();
        // state_init.set_code(Cell::default());
        // state_init.set_library(libs.data().unwrap().clone());
        // run_account_non_zero_level_stateinit(state_init, &mode, external, bounce)?;
        Ok(())
    };

    for mode in [Mode::Empty, Mode::Uninit, Mode::Frozen] {
        for bounce in [false, true] {
            check_with_statinit(&mode, false, bounce)?;
        }
    }

    check_with_statinit(&Mode::Uninit, true, false)?;

    Ok(())
}

#[test]
fn test_change_lib_implicity() {
    // manually create an output action list with one ChangeLib action
    let code = compile_code_to_cell(
        "
        PUSHREF
        .cell {
            .blob x26fa1dd403
            .cell {}
            .cell {
                .blob x00
            }
        }
        POPCTR c5
    ",
    )
    .unwrap();

    let start_balance = 1_000_000_000;
    let acc = create_test_account(start_balance, SENDER_ACCOUNT.clone(), code, Cell::default());
    assert_eq!(acc.libraries().len().unwrap(), 0);

    let msg = create_int_msg(
        THIRD_ACCOUNT.clone(),
        SENDER_ACCOUNT.clone(),
        14_200_000_000,
        false,
        BLOCK_LT - 2,
    );
    let res = execute_acc_with_message(acc, &msg);
    assert_eq!(res.acc.libraries().len().unwrap(), 1);
}

#[test]
fn test_change_library_with_big_tree_and_flag16() {
    // data of account has a big tree with more than 1000 cells
    // code gets own data and sets lib code with flag 16 - bounce if fails
    // cell size exceeds 1000 cells limit for SetLibCode
    let mut big_tree = BuilderData::new();
    for cell_num in 0..1001u32 {
        let mut cell = cell_num.write_to_new_cell().unwrap();
        if big_tree.references_free() != 0 {
            big_tree.checked_append_reference(cell.into_cell().unwrap()).unwrap();
        } else {
            cell.checked_append_reference(big_tree.into_cell().unwrap()).unwrap();
            big_tree = cell;
        }
    }
    let data = big_tree.into_cell().unwrap();
    let code = "
        ACCEPT
        PUSHROOT
        PUSHINT 17 ; set lib code public, bounce if fail
        SETLIBCODE
    ";
    let code = compile_code_to_cell(code).unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let acc = create_test_account(100_000_000_000, acc_id.clone(), code, data.clone());
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 100_000_000_000, true, BLOCK_LT - 2);
    execute_acc_with_message(acc, &msg)
        .expect_balance(64_479_875_650)
        .expect_count_out_msgs(1)
        .expect_compute_result(0)
        .expect_action_failed(43) // RESULT_CODE_LIB_EXCEEDED_LIMITS
        .expect_bounce_success();

    let code = "
        ACCEPT
        PUSHROOT
        PUSHINT 1 ; set lib code public
        SETLIBCODE
    ";
    let code = compile_code_to_cell(code).unwrap();
    let acc_id = SENDER_ACCOUNT.clone();
    let acc = create_test_account(100_000_000_000, acc_id.clone(), code, data);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, 100_000_000_000, true, BLOCK_LT - 2);
    execute_acc_with_message(acc, &msg)
        .expect_balance(164474397577)
        .expect_count_out_msgs(0)
        .expect_compute_result(0)
        .expect_action_failed(43) // RESULT_CODE_LIB_EXCEEDED_LIMITS
        .expect_no_bounce();
}
