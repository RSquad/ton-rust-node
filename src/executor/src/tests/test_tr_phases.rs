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
use crate::{
    blockchain_config::BlockchainConfig, error::ExecutorError, OrdinaryTransactionExecutor,
};

mod common;
use common::*;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    AccStatusChange, Account, AccountId, AccountStatus, BuilderData, Coins, ComputeSkipReason,
    ConfigParamEnum, CurrencyCollection, ExtOutMessageHeader, ExternalInboundMessageHeader,
    GetRepresentationHash, InternalMessageHeader, Message, MsgAddressExt, MsgAddressInt, OutAction,
    OutActions, Serializable, SliceData, StateInit, StorageUsed, SuspendedAddressList,
    TrActionPhase, TrComputePhase, TrComputePhaseVm, Transaction, VarUInteger32,
    DICT_HASH_MIN_CELLS, RESERVE_ALL_BUT, RESERVE_EXACTLY, RESERVE_IGNORE_ERROR,
    SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_ORDINARY, SENDMSG_PAY_FEE_SEPARATELY,
};
use ton_vm::{
    executor::gas::gas_state::Gas,
    int,
    stack::{integer::IntegerData, StackItem},
};

fn create_ext_msg(dest: AccountId) -> Message {
    let body_slice = SliceData::default();
    let hdr = ExternalInboundMessageHeader::new(
        Default::default(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
    );
    Message::with_ext_in_header_and_body(hdr, body_slice)
}

fn create_int_msg(
    src: AccountId,
    dest: AccountId,
    value: u64,
    bounce: bool,
    lt: u64,
    fwd_fee: impl Into<Coins>,
) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, src).unwrap(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
        CurrencyCollection::with_coins(value),
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.fwd_fee = fwd_fee.into();
    hdr.created_lt = lt;
    Message::with_int_header(hdr)
}

fn create_ext_out_msg(src_addr: AccountId) -> Message {
    let mut hdr = ExtOutMessageHeader::default();
    hdr.set_src(MsgAddressInt::with_standart(None, -1, src_addr).unwrap());
    hdr.created_lt = 1;
    hdr.created_at = 0x12345678;
    let mut msg = Message::with_ext_out_header(hdr);
    msg.set_body(SliceData::default());
    msg
}

fn create_state_init() -> StateInit {
    let mut init = StateInit::default();
    let code = compile_code_to_cell("PUSHINT 1 PUSHINT 1 ACCEPT").unwrap();
    let data = SliceData::new(vec![0x22; 32]).into_cell().unwrap();
    init.code = Some(code);
    init.data = Some(data);
    init
}

fn ordinary_compute_phase(msg: &Message, acc: &mut Account) -> Result<TrComputePhase> {
    ordinary_compute_phase_with_config(msg, acc, BLOCKCHAIN_CONFIG.to_owned(), 0)
}

fn ordinary_compute_phase_with_config(
    msg: &Message,
    acc: &mut Account,
    config: BlockchainConfig,
    block_unixtime: u32,
) -> Result<TrComputePhase> {
    let msg_balance = msg.get_value().cloned().unwrap_or_default();
    let mut acc_balance = acc.get_balance().cloned().unwrap_or_else(|| msg_balance.clone());
    let acc_address = msg.dst_ref().unwrap();

    let config_params = config.raw_config().clone();
    let info = SmartContractInfo {
        myself: SliceData::load_builder(acc_address.write_to_new_cell().unwrap_or_default())
            .unwrap(),
        in_msg: Some(msg.clone()),
        incoming_value: msg_balance.clone(),
        balance: acc_balance.clone(),
        config_params,
        ..Default::default()
    };
    let executor = OrdinaryTransactionExecutor::new(config.clone());
    let stack = executor.build_stack(Some(msg), acc).unwrap();
    if acc.is_none() && msg.is_internal() {
        let last_paid = info.unix_time();
        *acc = account_from_message(
            &config,
            msg,
            msg.dst_ref().unwrap(),
            msg.value().unwrap(),
            last_paid,
            true,
        )
        .unwrap();
    }

    let (phase, _actions, _new_data) = executor.compute_phase(
        Some(msg),
        acc,
        &mut acc_balance,
        &msg_balance,
        info,
        stack,
        msg.is_masterchain(),
        false,
        false,
        &ExecuteParams { block_unixtime, ..Default::default() },
    )?;
    acc.set_balance(acc_balance);
    Ok(phase)
}

/// Calculate new account according to inbound message.
/// If message has no value, account will not created.
/// If hash of state_init is equal to account address
/// (or flag check address is false), account will be active.
/// Otherwise, account will be nonexist or uninit according bounce flag:
/// if bounce, account will be uninit that save money.
fn test_account_from_message(msg: &Message) -> Option<Account> {
    account_from_message(
        &BLOCKCHAIN_CONFIG,
        msg,
        msg.dst_ref()?,
        msg.value().unwrap_or(&CurrencyCollection::default()),
        0,
        false,
    )
}

#[test]
fn test_computing_phase_extmsg_to_acc_notexist_nogas() {
    let mut msg = create_ext_msg(SENDER_ACCOUNT.clone());
    let mut acc = Account::default();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));

    msg.set_state_init(create_state_init());
    let mut acc = Account::default();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_notexist_intmsg_state() {
    let msg =
        create_int_msg(SENDER_ACCOUNT.clone(), RECEIVER_ACCOUNT.clone(), 5_000_000, false, 5, 0);

    let mut acc = Account::default();
    pretty_assertions::assert_eq!(acc.status(), AccountStatus::AccStateNonexist);
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoState));
    pretty_assertions::assert_eq!(acc.status(), AccountStatus::AccStateUninit);
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_nostate() {
    let msg = create_ext_msg(SENDER_ACCOUNT.clone());

    //create uninitialized account
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        &CurrencyCollection::with_coins(1000),
    );
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_with_state() {
    let state_init = create_state_init();
    let hash = state_init.hash().unwrap();
    let addr = AccountId::from(*hash.as_slice());
    let mut msg = create_ext_msg(addr.clone());
    msg.set_state_init(state_init);

    //create uninitialized account
    let balance = 5000000;
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, addr).unwrap(),
        &CurrencyCollection::with_coins(balance),
    );
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();

    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    // vm_phase.msg_state_used = true;
    // vm_phase.account_activated = true;
    vm_phase.exit_code = 0;
    let used = 67u32;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(500.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 4;
    pretty_assertions::assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

#[test]
fn test_computing_phase_acc_uninit_intmsg_with_nostate() {
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap(),
        &CurrencyCollection::with_coins(5_000_000),
    );
    let msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        1_000_000, // it is enough coins by config to buy gas for uninit account
        // but message has no state
        false,
        5,
        0,
    );
    let phase = ordinary_compute_phase(&msg, &mut acc.clone()).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoState));

    let msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        500, // it is not enough by config to buy gas for uninit account
        false,
        5,
        0,
    );

    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_uninit_intmsg_no_state_has_priority_over_suspended() {
    let mut raw_cfg = BLOCKCHAIN_CONFIG.raw_config().clone();
    let mut suspended = SuspendedAddressList::default();
    let addr = MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap();
    suspended.add_suspended_address(addr.workchain_id(), addr.address().clone()).unwrap();
    suspended.set_suspended_until(1);
    raw_cfg.set_config(ConfigParamEnum::ConfigParam44(suspended)).unwrap();
    let cfg = BlockchainConfig::with_config(raw_cfg).unwrap();

    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap(),
        &CurrencyCollection::with_coins(5_000_000),
    );
    let msg =
        create_int_msg(SENDER_ACCOUNT.clone(), RECEIVER_ACCOUNT.clone(), 1_000_000, false, 5, 0);

    let phase = ordinary_compute_phase_with_config(&msg, &mut acc, cfg, 0).unwrap();
    pretty_assertions::assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoState));
}

#[test]
fn test_computing_phase_acc_active_extmsg() {
    //external inbound msg for account
    let msg = create_ext_msg(SENDER_ACCOUNT.clone());

    //msg just for creating account with active state
    let balance = 5000000;
    let mut ctor_msg =
        create_int_msg(SENDER_ACCOUNT.clone(), SENDER_ACCOUNT.clone(), balance, false, 5, 0);
    let mut init = StateInit::default();
    let code = compile_code_to_cell("PUSHINT 1 PUSHINT 2 ADD ACCEPT").unwrap();
    let data = SliceData::new(vec![0x22; 32]).into_cell().unwrap();
    init.code = Some(code);
    init.data = Some(data);
    ctor_msg.set_state_init(init);

    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut acc = test_account_from_message(&ctor_msg).unwrap();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 0;
    let used = 85u32;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(500u16.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 5;
    pretty_assertions::assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

fn create_account(balance: u64, address: &[u8; 32], code: Cell, data: Cell) -> Account {
    //msg just for creating account with active state
    let mut ctor_msg =
        create_int_msg(SENDER_ACCOUNT.clone(), AccountId::from(*address), balance, false, 5, 0);

    let mut init = StateInit::default();
    init.code = Some(code);
    init.data = Some(data);
    ctor_msg.set_state_init(init);

    test_account_from_message(&ctor_msg).unwrap()
}

#[test]
fn test_computing_phase_activeacc_gas_not_accepted() {
    let balance = 100000000;
    let address = [0x33; 32];
    let mut acc = create_account(
        balance,
        &address,
        compile_code_to_cell("PUSHINT 1 PUSHINT 2 THROW 100 ADD ACCEPT").unwrap(),
        SliceData::new(vec![0x22; 32]).into_cell().unwrap(),
    );

    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from(address));
    let result = ordinary_compute_phase(&msg, &mut acc);
    println!("{:?}", result);
    let e = result.expect_err("Must generate ExecutorError::NoAcceptError()");
    pretty_assertions::assert_eq!(
        e.downcast_ref(),
        Some(&ExecutorError::NoAcceptError(100, Some(int!(0))))
    );
}

#[test]
fn test_computing_phase_activeacc_gas_consumed_after_accept() {
    let balance = 10000000;
    let address = [0x33; 32];
    let mut acc = create_account(
        balance,
        &address,
        compile_code_to_cell("PUSHINT 1 PUSHINT 2 ADD ACCEPT PUSHINT 3 THROW 100").unwrap(),
        SliceData::new(vec![0x22; 32]).into_cell().unwrap(),
    );

    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from(address));
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 100;
    vm_phase.gas_used = 182u32.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(1000u16.into());
    let gas_fees = 182u64 * gas_config.get_real_gas_price();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 6;
    pretty_assertions::assert_eq!(phase, TrComputePhase::Vm(vm_phase));
    pretty_assertions::assert_eq!(acc.balance().unwrap().coins, Coins::from(balance - gas_fees));
}

fn call_action_phase(
    start_acc_balance: u64,
    out_msg_value: u64,
    must_succeded: bool,
    no_funds: bool,
    fwd_fees: u64,
    action_fees: impl Into<Coins>,
    res: i32,
    res_arg: Option<i32>,
) {
    let msg = create_ext_msg(SENDER_ACCOUNT.clone());

    //msg just for creating account with active state
    let mut ctor_msg = create_int_msg(
        AccountId::from([0x12; 32]),
        SENDER_ACCOUNT.clone(),
        start_acc_balance,
        false,
        5,
        0,
    );
    let mut init = StateInit::default();
    let code = SliceData::new_empty().into_cell().unwrap();
    let data = SliceData::new(vec![0x22; 32]).into_cell().unwrap();
    init.code = Some(code);
    init.data = Some(data);
    ctor_msg.set_state_init(init);

    let mut acc = test_account_from_message(&ctor_msg).unwrap();
    let mut tr = Transaction::with_account_and_message(&acc, &msg, 1).unwrap();

    let mut actions = OutActions::default();
    let msg = create_ext_out_msg(SENDER_ACCOUNT.clone());
    let mut storage = StorageUsed::default();
    append_message(&mut storage, &msg).unwrap();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = OrdinaryTransactionExecutor::new(config.clone());
    let fwd_prices = config.get_fwd_prices(ctor_msg.is_masterchain());
    let msg_fwd_fees = Coins::from(fwd_prices.lump_price)
        - fwd_prices.mine_fee_checked(&fwd_prices.lump_price.into()).unwrap();
    let mut msg = create_int_msg(
        SENDER_ACCOUNT.clone(),
        RECEIVER_ACCOUNT.clone(),
        out_msg_value,
        true,
        5,
        msg_fwd_fees,
    );
    let mut msg_remaining_balance = msg.get_value().cloned().unwrap_or_default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg.clone()));
    if must_succeded {
        msg.value_mut().unwrap().coins = (out_msg_value - fwd_prices.lump_price).into();
        append_message(&mut storage, &msg).unwrap();
    }

    // this message costs 10000000
    let msg = create_ext_out_msg(SENDER_ACCOUNT.clone());
    if must_succeded {
        append_message(&mut storage, &msg).unwrap();
    }
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));
    let actions_hash = actions.hash().unwrap();
    let mut acc_balance = CurrencyCollection::with_coins(start_acc_balance);
    let original_acc_balance = acc_balance.clone();

    let my_addr = acc.get_addr().unwrap().clone();

    let result = executor
        .action_phase(
            &mut tr,
            &mut acc,
            &original_acc_balance,
            &mut acc_balance,
            &mut msg_remaining_balance,
            &Coins::zero(),
            actions.serialize().unwrap(),
            None,
            &my_addr,
            false,
        )
        .unwrap();
    let phase = result.phase;
    let mut phase2 = TrActionPhase::default();
    phase2.success = must_succeded;
    phase2.valid = true;
    phase2.no_funds = no_funds;
    phase2.msgs_created = if must_succeded { 3 } else { 1 };
    phase2.tot_actions = 3;
    phase2.status_change = AccStatusChange::Unchanged;
    phase2.action_list_hash = actions_hash;
    phase2.add_fwd_fees(&fwd_fees.into());
    if must_succeded {
        phase2.add_action_fees(&action_fees.into());
    }
    phase2.result_code = res;
    phase2.result_arg = res_arg;
    phase2.tot_msg_size = storage;
    pretty_assertions::assert_eq!(phase, phase2);
    if !no_funds {
        let balance = start_acc_balance - out_msg_value - 2 * fwd_prices.lump_price;
        pretty_assertions::assert_eq!(acc_balance.coins.as_u128(), balance.into());
    }
}

#[test]
fn test_action_phase_active_acc_with_actions_nofunds() {
    let fwd_config = BLOCKCHAIN_CONFIG.get_fwd_prices(true);
    let fwd_fee = fwd_config.lump_price;
    // will send one external message then fails
    call_action_phase(
        100000200,
        300,
        false,
        true,
        fwd_fee,
        fwd_fee,
        RESULT_CODE_NOT_ENOUGH_COINS,
        Some(1),
    );
}

#[test]
fn test_action_phase_active_acc_with_actions_success() {
    let fwd_config = BLOCKCHAIN_CONFIG.get_fwd_prices(true);
    let fwd_fee = fwd_config.lump_price * 3;
    let mine_fee = Coins::from(fwd_config.lump_price * 2)
        + fwd_config.mine_fee_checked(&fwd_config.lump_price.into()).unwrap();
    call_action_phase(5000000000, 100000000, true, false, fwd_fee, mine_fee, 0, None);
}

fn init_test_gas(acc_balance: u128, msg_balance: u128) -> Gas {
    init_gas(
        &Account::default(),
        0,
        acc_balance,
        msg_balance,
        msg_balance == 0,
        false,
        true,
        BLOCKCHAIN_CONFIG.get_gas_config(false),
    )
}

#[test]
fn test_gas_init1() {
    let gas_test = init_test_gas(15000000, 0);
    let gas_etalon = Gas::new(0, 10000, 37500, 400);
    pretty_assertions::assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init2() {
    let gas_test = init_test_gas(4000000, 0);
    let gas_etalon = Gas::new(0, 10000, 10000, 400);
    pretty_assertions::assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init3() {
    let gas_test = init_test_gas(10000000, 100000);
    let gas_etalon = Gas::new(250, 0, 25000, 400);
    pretty_assertions::assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init4() {
    let gas_test = init_test_gas(1_000_000_000_000_000_000, 1_000_000_000_000);
    let gas_etalon = Gas::new(1000000, 0, 1000000, 400);
    pretty_assertions::assert_eq!(gas_test, gas_etalon);
}

mod actions {

    use super::*;
    use ton_block::{
        AddSub, AnycastInfo, ConfigParam12, ConfigParam18, ConfigParam31, ConfigParamEnum,
        StoragePrices, WorkchainDescr, WorkchainFormat0, Workchains, RESERVE_PLUS_ORIG,
        RESERVE_REVERSE,
    };

    #[derive(Default)]
    struct TestCase {
        check_balance_enough: bool,
        expected_remains: Option<CurrencyCollection>,
        expected_reserve: Option<std::result::Result<Coins, i32>>,
        fee_value: Option<u64>,
        msg_coins_balance: Option<u64>,
        msg_other_balance: Option<u128>,
        src_coins_balance: u64,
        src_other_balance: Option<u128>,
        sub_coins_balance: u64,
        sub_other_balance: Option<u128>,
    }

    fn check(mode: u8, test_case: &TestCase) {
        let mut sub = CurrencyCollection::with_coins(test_case.sub_coins_balance);
        if let Some(other_balance) = test_case.sub_other_balance {
            sub.other
                .set(&11111111u32, &VarUInteger32::from_two_u128(0, other_balance).unwrap())
                .unwrap()
        }
        let mut src = CurrencyCollection::with_coins(test_case.src_coins_balance);
        if let Some(other_balance) = test_case.src_other_balance {
            src.other
                .set(&11111111u32, &VarUInteger32::from_two_u128(0, other_balance).unwrap())
                .unwrap()
        }
        let mut dst = src.clone();
        let msg = if let Some(msg_balance) = test_case.msg_coins_balance {
            let mut msg = CurrencyCollection::with_coins(msg_balance);
            if let Some(msg_balance) = test_case.msg_other_balance {
                msg.other
                    .set(&11111111u32, &VarUInteger32::from_two_u128(0, msg_balance).unwrap())
                    .unwrap()
            }
            dst.add(&msg).unwrap();
            Some(msg)
        } else {
            None
        };
        if let Some(fee) = test_case.fee_value {
            let fee = CurrencyCollection::with_coins(fee);
            dst.sub(&fee).unwrap();
        }
        let result = reserve_action_handler(mode, &sub, &src, &mut dst);
        if test_case.check_balance_enough {
            src.clone().sub(&sub).unwrap();
        }
        if let Some(msg) = msg {
            src.add(&msg).unwrap();
        }
        pretty_assertions::assert_eq!(
            result,
            test_case.expected_reserve.unwrap_or(Err(RESULT_CODE_UNSUPPORTED))
        );
        pretty_assertions::assert_eq!(&dst, test_case.expected_remains.as_ref().unwrap_or(&src))
    }

    #[test]
    fn test_reserve_exactly() {
        let mode = RESERVE_EXACTLY;
        let mut test_case = TestCase {
            check_balance_enough: true,
            src_coins_balance: 500,
            src_other_balance: Some(500),
            sub_coins_balance: 123,
            sub_other_balance: Some(123),
            ..Default::default()
        };
        // enough balance and extra
        check(mode, &test_case);
        // not enough balance
        test_case.check_balance_enough = false;
        test_case.src_coins_balance = 100;
        test_case.src_other_balance = Some(100);
        check(mode, &test_case);
        // not enough extra
        test_case.src_coins_balance = 123;
        test_case.sub_coins_balance = 100;
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_all_but() {
        // reserve = remaining_balance - value
        let mode = RESERVE_ALL_BUT;
        let mut test_case = TestCase {
            check_balance_enough: true,
            src_coins_balance: 500,
            src_other_balance: Some(500),
            sub_coins_balance: 123,
            sub_other_balance: Some(123),
            ..Default::default()
        };
        // enough balance and extra
        check(mode, &test_case);
        // not enough balance and extra
        test_case.check_balance_enough = false;
        test_case.src_coins_balance = 100;
        test_case.src_other_balance = Some(100);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_exactly_skip_error() {
        // reserve = min(value, remaining_balance)
        let mode = RESERVE_IGNORE_ERROR;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(0)),
            expected_reserve: Some(Ok(Coins::new(100))),
            src_coins_balance: 100,
            sub_coins_balance: 123,
            ..Default::default()
        };
        // reserved less than needed
        check(mode, &test_case);
        // extra not enough
        test_case.expected_remains = None;
        test_case.expected_reserve = None;
        test_case.src_other_balance = Some(100);
        test_case.sub_other_balance = Some(123);
        check(mode, &test_case);
        // balance and extra enough
        test_case.src_coins_balance = 123;
        test_case.src_other_balance = Some(123);
        test_case.sub_coins_balance = 100;
        test_case.sub_other_balance = Some(100);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_all_but_skip_error() {
        // reserve = remaining_balance - min(value, remaining_balance)
        let mode = RESERVE_ALL_BUT | RESERVE_IGNORE_ERROR;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(100)),
            expected_reserve: Some(Ok(Coins::new(0))),
            src_coins_balance: 100,
            sub_coins_balance: 123,
            ..Default::default()
        };
        // reserved become less than needed
        check(mode, &test_case);
        // extra not enough
        test_case.expected_remains = None;
        test_case.expected_reserve = None;
        test_case.src_other_balance = Some(100);
        test_case.sub_other_balance = Some(123);
        check(mode, &test_case);
        // balance and extra enough
        test_case.src_coins_balance = 123;
        test_case.src_other_balance = Some(123);
        test_case.sub_coins_balance = 100;
        test_case.sub_other_balance = Some(100);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_sum_mode() {
        // reserve = original_balance + value
        let mode = RESERVE_PLUS_ORIG;
        let mut test_case = TestCase {
            src_coins_balance: 100,
            src_other_balance: Some(100),
            sub_coins_balance: 123,
            sub_other_balance: Some(123),
            ..Default::default()
        };
        // reserve exceed balance
        check(mode, &test_case);
        // message added money, so reserve don't exceed balance
        test_case.msg_coins_balance = Some(300);
        test_case.msg_other_balance = Some(300);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_minus_sum_mode() {
        // reserve = remaining_balance - (original_balance + value)
        let mode = RESERVE_PLUS_ORIG | RESERVE_ALL_BUT;
        let mut test_case = TestCase {
            src_coins_balance: 100,
            src_other_balance: Some(100),
            sub_coins_balance: 123,
            sub_other_balance: Some(123),
            ..Default::default()
        };
        // not enough balance and extra
        check(mode, &test_case);
        // message added money, so balance is enough
        test_case.msg_coins_balance = Some(300);
        test_case.msg_other_balance = Some(300);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_remaining_mode() {
        // reserve = min(original_balance + value, remaining_balance)
        let mode = RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(0)),
            expected_reserve: Some(Ok(Coins::new(100))),
            src_coins_balance: 100,
            sub_coins_balance: 123,
            ..Default::default()
        };
        // reserved become less than needed
        check(mode, &test_case);
        // extra not enough
        test_case.expected_remains = None;
        test_case.expected_reserve = None;
        test_case.src_other_balance = Some(100);
        test_case.sub_other_balance = Some(123);
        check(mode, &test_case);
        // message added money, so balance is enough
        test_case.msg_coins_balance = Some(300);
        test_case.msg_other_balance = Some(300);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_7_mode() {
        // reserve = remaining_balance - min(original_balance + value, remaining_balance)
        let mode = RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(100)),
            expected_reserve: Some(Ok(Coins::new(0))),
            src_coins_balance: 100,
            sub_coins_balance: 123,
            ..Default::default()
        };
        // reserved without message balance
        check(mode, &test_case);
        // reserved with message balance
        test_case.expected_remains = Some(CurrencyCollection::with_coins(223));
        test_case.expected_reserve = Some(Ok(Coins::new(177)));
        test_case.msg_coins_balance = Some(300);
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_unsupported_mode() {
        let mut test_case =
            TestCase { src_coins_balance: 100, sub_coins_balance: 123, ..Default::default() };
        test_case.expected_reserve = Some(Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION));
        for mode in 8..=11 {
            check(mode, &test_case);
        }
        check(32, &test_case);
    }

    #[test]
    fn test_reserve_all_except_mode() {
        // reserve = original_balance - value
        let mode = RESERVE_REVERSE | RESERVE_PLUS_ORIG;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(10)),
            expected_reserve: Some(Ok(Coins::new(90))),
            src_coins_balance: 100,
            sub_coins_balance: 10,
            ..Default::default()
        };
        // balance enough
        check(mode, &test_case);
        // balance not enough
        test_case.expected_remains = None;
        test_case.expected_reserve = None;
        test_case.src_coins_balance = 10;
        test_case.sub_coins_balance = 100;
        check(mode, &test_case);
        // balance not enough despite message balance
        test_case.expected_remains = Some(CurrencyCollection::with_coins(310));
        test_case.msg_coins_balance = Some(300);
        check(mode, &test_case);
        // balance not enough because of fee
        test_case.expected_remains = Some(CurrencyCollection::with_coins(1));
        test_case.expected_reserve = Some(Err(RESULT_CODE_NOT_ENOUGH_COINS));
        test_case.fee_value = Some(99);
        test_case.msg_coins_balance = None;
        test_case.src_coins_balance = 100;
        test_case.sub_coins_balance = 10;
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_13_mode() {
        // reserve = remaining_balance - (original_balance - value)
        let mode = RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_ALL_BUT;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(23)),
            expected_reserve: Some(Ok(Coins::new(400))),
            msg_coins_balance: Some(300),
            src_coins_balance: 123,
            sub_coins_balance: 100,
            ..Default::default()
        };
        // Balance enough
        check(mode, &test_case);
        // Balance not enough
        test_case.expected_remains = Some(CurrencyCollection::with_coins(400));
        test_case.expected_reserve = None;
        test_case.src_coins_balance = 100;
        test_case.sub_coins_balance = 123;
        check(mode, &test_case);
        // With fee
        test_case.expected_remains = Some(CurrencyCollection::with_coins(100));
        test_case.expected_reserve = Some(Err(RESULT_CODE_NOT_ENOUGH_COINS));
        test_case.fee_value = Some(23);
        test_case.msg_coins_balance = None;
        test_case.src_coins_balance = 123;
        test_case.sub_coins_balance = 10;
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_14_mode() {
        // reserve = min(original_balance - value, remaining_balance)
        let mode = RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(400)),
            expected_reserve: Some(Ok(Coins::new(23))),
            msg_coins_balance: Some(300),
            src_coins_balance: 123,
            sub_coins_balance: 100,
            ..Default::default()
        };
        // Balance enough
        check(mode, &test_case);
        // With message
        test_case.expected_remains = Some(CurrencyCollection::with_coins(400));
        test_case.expected_reserve = None;
        test_case.src_coins_balance = 100;
        test_case.sub_coins_balance = 123;
        check(mode, &test_case);
        // With fee
        test_case.expected_remains = Some(CurrencyCollection::with_coins(0));
        test_case.expected_reserve = Some(Ok(Coins::new(100)));
        test_case.fee_value = Some(23);
        test_case.msg_coins_balance = None;
        test_case.src_coins_balance = 123;
        test_case.sub_coins_balance = 10;
        check(mode, &test_case);
    }

    #[test]
    fn test_reserve_15_mode() {
        // reserve = remaining_balance - min(original_balance - value, remaining_balance)
        let mode = RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT;
        let mut test_case = TestCase {
            expected_remains: Some(CurrencyCollection::with_coins(23)),
            expected_reserve: Some(Ok(Coins::new(400))),
            msg_coins_balance: Some(300),
            src_coins_balance: 123,
            sub_coins_balance: 100,
            ..Default::default()
        };
        // Balance enough
        check(mode, &test_case);
        // With message
        test_case.expected_remains = Some(CurrencyCollection::with_coins(400));
        test_case.expected_reserve = None;
        test_case.src_coins_balance = 100;
        test_case.sub_coins_balance = 123;
        check(mode, &test_case);
        // With fee
        test_case.expected_remains = Some(CurrencyCollection::with_coins(100));
        test_case.expected_reserve = Some(Ok(Coins::new(0)));
        test_case.fee_value = Some(23);
        test_case.msg_coins_balance = None;
        test_case.src_coins_balance = 123;
        test_case.sub_coins_balance = 10;
        check(mode, &test_case);
    }

    fn test_sendmsg_action(
        mode: u8,
        val: u64,
        bal: u64,
        msg_lt: u64,
        fwd_fee: u64,
        mine_fee: u64,
        error: Option<i32>,
    ) {
        let mut balance = CurrencyCollection::with_coins(bal);
        let mut acc_remaining_balance = balance.clone();
        let mut msg_remaining_balance = CurrencyCollection::with_coins(val);
        let mut phase = TrActionPhase::default();
        phase.add_fwd_fees(&3.into());
        phase.add_action_fees(&5.into());
        let address = MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap();
        let mut msg = create_int_msg(
            SENDER_ACCOUNT.clone(),
            RECEIVER_ACCOUNT.clone(),
            val,
            false,
            0,
            fwd_fee,
        );

        msg.set_at_and_lt(0, msg_lt);
        let res = outmsg_action_handler(
            &mut phase,
            mode,
            &mut msg,
            &mut acc_remaining_balance,
            &mut msg_remaining_balance,
            &Coins::zero(),
            &BLOCKCHAIN_CONFIG,
            false,
            &address,
            &Default::default(),
            &mut false,
        );

        let mut res_val = CurrencyCollection::with_coins(val);
        if (mode & SENDMSG_ALL_BALANCE) != 0 {
            res_val = CurrencyCollection::with_coins(bal);
        } else if (mode & SENDMSG_PAY_FEE_SEPARATELY) != 0 {
            res_val.add(&CurrencyCollection::with_coins(fwd_fee)).unwrap();
        }

        if error.is_some() {
            pretty_assertions::assert_eq!(res, Err(error.unwrap()));
            return;
        }
        pretty_assertions::assert_eq!(res, Ok(res_val.clone()));

        balance.sub(&res_val).unwrap();
        pretty_assertions::assert_eq!(acc_remaining_balance, balance);

        pretty_assertions::assert_eq!(msg.src_ref().expect("must be internal msg"), &address);
        pretty_assertions::assert_eq!(msg.at_and_lt().unwrap(), (0, msg_lt));
        pretty_assertions::assert_eq!(msg.get_fee().unwrap(), Some((fwd_fee - mine_fee).into()));

        res_val.sub(&CurrencyCollection::with_coins(fwd_fee)).unwrap();
        pretty_assertions::assert_eq!(msg.get_value().unwrap().clone(), res_val);

        let mut total_fwd_fees = Coins::zero();
        total_fwd_fees.add(&3u64.into()).unwrap();
        total_fwd_fees.add(&fwd_fee.into()).unwrap();
        pretty_assertions::assert_eq!(phase.total_fwd_fees(), total_fwd_fees);

        let mut total_action_fees = Coins::zero();
        total_action_fees.add(&5u64.into()).unwrap();
        total_action_fees.add(&mine_fee.into()).unwrap();
        pretty_assertions::assert_eq!(phase.total_action_fees(), total_action_fees);
    }

    #[test]
    fn test_sendmsg_internal_fees_separately() {
        test_sendmsg_action(
            SENDMSG_PAY_FEE_SEPARATELY,
            10000000,
            50000000,
            12,
            10000000,
            3333282,
            None,
        )
    }

    #[test]
    fn test_sendmsg_internal_ordinary_skip_error() {
        test_sendmsg_action(
            SENDMSG_IGNORE_ERROR,
            15000000,
            9000000,
            12,
            10000000,
            3333282,
            Some(RESULT_CODE_SKIPPED),
        )
    }

    #[test]
    fn test_sendmsg_internal_fees_separately_skip_error() {
        test_sendmsg_action(
            SENDMSG_IGNORE_ERROR | SENDMSG_PAY_FEE_SEPARATELY,
            15000000,
            9000000,
            12,
            10000000,
            3333282,
            Some(RESULT_CODE_SKIPPED),
        )
    }

    #[test]
    fn test_sendmsg_internal_ordinary() {
        test_sendmsg_action(SENDMSG_ORDINARY, 10000000, 50000000, 12, 10000000, 3333282, None)
    }

    #[test]
    fn test_sendmsg_internal_ordinary_no_funds() {
        test_sendmsg_action(
            SENDMSG_ORDINARY,
            15000000,
            8000000,
            12,
            10000000,
            3333282,
            Some(RESULT_CODE_NOT_ENOUGH_COINS),
        )
    }

    #[test]
    fn test_sendmsg_internal_all_balance() {
        //test case:
        //balance was 120, contract sent msg with value = 120
        //then vm executed, gas exacted and balance became 100
        //then in action phase need to transfer all remaining balance (100)
        test_sendmsg_action(SENDMSG_ALL_BALANCE, 12000000, 10000000, 3, 10000000, 3333282, None)
    }

    #[test]
    fn test_sendmsg_internal_wrong_mode() {
        test_sendmsg_action(
            4,
            15000000,
            8000000,
            12,
            10000000,
            3333282,
            Some(RESULT_CODE_UNSUPPORTED),
        )
    }

    #[test]
    fn test_check_rewrite_dst() {
        let mut wc0 = WorkchainDescr::default();
        wc0.accept_msgs = true;
        let wc1 = wc0.clone();
        let mut wcs = Workchains::default();
        wcs.set(&0, &wc0).unwrap();
        wcs.set(&-1, &wc1).unwrap();

        let mut wc3 = wc0.clone();
        wc3.format =
            WorkchainFormat::Extended(WorkchainFormat0::with_params(128, 512, 128, 3).unwrap());
        wcs.set(&3, &wc3).unwrap();
        let mut wc4 = wc0;
        wc4.accept_msgs = false;
        wcs.set(&4, &wc4).unwrap();
        let mut raw_cfg = BLOCKCHAIN_CONFIG.raw_config().clone();
        raw_cfg
            .set_config(ConfigParamEnum::ConfigParam12(ConfigParam12 { workchains: wcs }))
            .unwrap();

        let mut cf18 = ConfigParam18::default();
        cf18.insert(&StoragePrices::default()).unwrap();
        raw_cfg.set_config(ConfigParamEnum::ConfigParam18(cf18)).unwrap();
        raw_cfg.set_config(ConfigParamEnum::ConfigParam31(ConfigParam31::default())).unwrap();
        let cfg = BlockchainConfig::with_config(raw_cfg).unwrap();

        // simple masterchain
        let dst = MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // from masterchain to workchain
        let dst = MsgAddressInt::with_standart(None, 0, THIRD_ACCOUNT.clone()).unwrap();
        let src = MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src).unwrap(), dst);

        // from workchain to masterchain
        let dst = MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap();
        let src = MsgAddressInt::with_standart(None, 0, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src).unwrap(), dst);

        // from workchain to masterchain
        let dst = MsgAddressInt::with_standart(None, -1, THIRD_ACCOUNT.clone()).unwrap();
        let src = MsgAddressInt::with_standart(None, 2, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &src),
            Err(IncorrectCheckRewrite::Other)
        );

        // from workchain to workchain
        let dst = MsgAddressInt::with_standart(None, 0, THIRD_ACCOUNT.clone()).unwrap();
        let src = MsgAddressInt::with_standart(None, 2, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &src),
            Err(IncorrectCheckRewrite::Other)
        );

        // anycast masterchain
        let dst = MsgAddressInt::with_standart(
            Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()),
            -1,
            RECEIVER_ACCOUNT.clone(),
        )
        .unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::Anycast)
        );

        // simple workchain
        let dst = MsgAddressInt::with_standart(None, 0, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // unknown workchain
        let dst = MsgAddressInt::with_standart(None, 10, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::WrongWorkchain)
        );

        // anycast
        let dst = MsgAddressInt::with_standart(
            Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()),
            0,
            RECEIVER_ACCOUNT.clone(),
        )
        .unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::Anycast)
        );

        // addr_var
        let dst =
            MsgAddressInt::with_variant(None, 0, SliceData::from_raw(vec![0x22; 32], 256)).unwrap();
        let answer = MsgAddressInt::with_standart(None, 0, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), answer);

        // incorrect len address
        let dst =
            MsgAddressInt::with_variant(None, 0, SliceData::from_raw(vec![0x22; 32], 255)).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::HandWriteCheck)
        );

        // custom workchain
        let dst = MsgAddressInt::with_standart(None, 3, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // custom workchain not accept msgs
        let dst = MsgAddressInt::with_standart(None, 4, RECEIVER_ACCOUNT.clone()).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::Other)
        );

        // custom workchain incorrect len address
        let dst =
            MsgAddressInt::with_variant(None, 3, SliceData::from_raw(vec![0x22; 20], 160)).unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::HandWriteCheck)
        );

        // custom workchain addr_var
        let dst =
            MsgAddressInt::with_variant(None, 3, SliceData::from_raw(vec![0x22; 16], 128)).unwrap();
        pretty_assertions::assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // masterchain addr_var
        let dst = MsgAddressInt::with_variant(None, -1, SliceData::from_raw(vec![0x22; 16], 128))
            .unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::Other)
        );

        // custom workchain anycast
        let dst = MsgAddressInt::with_variant(
            Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()),
            3,
            SliceData::from_raw(vec![0x22; 16], 128),
        )
        .unwrap();
        pretty_assertions::assert_eq!(
            check_rewrite_dest_addr(&dst, &cfg, &dst),
            Err(IncorrectCheckRewrite::Anycast)
        )
    }
}

#[test]
fn test_account_from_message_any() {
    let src = MsgAddressInt::with_standart(None, 0, SENDER_ACCOUNT.clone()).unwrap();
    let dst = MsgAddressInt::with_standart(None, 0, RECEIVER_ACCOUNT.clone()).unwrap();
    let ext = MsgAddressExt::with_extern([0x99; 32].into()).unwrap();

    // external inbound message
    let hdr = ExternalInboundMessageHeader::new(ext.clone(), dst.clone());
    let msg = Message::with_ext_in_header(hdr);
    assert!(
        test_account_from_message(&msg).is_none(),
        "account mustn't be constructed using external message"
    );

    // external outbound message
    let hdr = ExtOutMessageHeader::with_addresses(src.clone(), ext);
    let msg = Message::with_ext_out_header(hdr);
    assert!(
        test_account_from_message(&msg).is_none(),
        "account mustn't be constructed using external message"
    );

    // message without StateInit and with bounce
    let value = CurrencyCollection::with_coins(0);
    let hdr =
        InternalMessageHeader::with_addresses_and_bounce(src.clone(), dst.clone(), value, true);
    let msg = Message::with_int_header(hdr);
    assert!(
        test_account_from_message(&msg).is_none(),
        "account must not be constructed without StateInit and with bounce and zero msg balance"
    );

    // message without code
    let value = CurrencyCollection::with_coins(0);
    let hdr =
        InternalMessageHeader::with_addresses_and_bounce(src.clone(), dst.clone(), value, true);
    let mut msg = Message::with_int_header(hdr);
    let init = StateInit::default();
    msg.set_state_init(init);
    assert!(
        test_account_from_message(&msg).is_none(),
        "account mustn't be constructed without code"
    );

    // message without balance
    let hdr = InternalMessageHeader::with_addresses_and_bounce(
        src.clone(),
        dst.clone(),
        Default::default(),
        false,
    );
    let mut msg = Message::with_int_header(hdr);
    let mut init = StateInit::default();
    init.set_code(SliceData::new(vec![0x71, 0x80]).into_cell().unwrap());
    msg.set_state_init(init);
    pretty_assertions::assert_eq!(
        test_account_from_message(&msg).unwrap().status(),
        AccountStatus::AccStateActive,
        "account must be constructed without balance"
    );

    // message without StateInit and without bounce
    let value = CurrencyCollection::with_coins(100);
    let hdr = InternalMessageHeader::with_addresses_and_bounce(src, dst, value, false);
    let mut msg = Message::with_int_header(hdr);
    pretty_assertions::assert_eq!(
        test_account_from_message(&msg).unwrap().status(),
        AccountStatus::AccStateUninit,
        "account must be constructed without StateInit and without bounce"
    );

    // message with code and without bounce
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    msg.set_state_init(init);
    pretty_assertions::assert_eq!(
        test_account_from_message(&msg).unwrap().status(),
        AccountStatus::AccStateActive,
        "account must be constructed with code and without bounce"
    );

    // message with code and with bounce
    msg.int_header_mut().unwrap().bounce = true;
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    msg.set_state_init(init);
    pretty_assertions::assert_eq!(
        test_account_from_message(&msg).unwrap().status(),
        AccountStatus::AccStateActive,
        "account must be constructed with code and with bounce"
    );

    // message with libraries and bounce
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true)
        .unwrap();
    msg.set_state_init(init);
    pretty_assertions::assert_eq!(
        test_account_from_message(&msg).unwrap().status(),
        AccountStatus::AccStateActive,
        "account must be constructed if libraries enabled"
    );

    execute_acc_with_message(Account::default(), &msg).expect_status(AccountStatus::AccStateUninit);
}

#[test]
fn test_generate_account_and_update() {
    let mut account = gen_test_account();
    account.set_code(Cell::default()); // set code does not update storage stat
    let cell = account.serialize().unwrap(); // serialization doesn't update storage stat
    let account2 = Account::construct_from_cell(cell).unwrap();
    pretty_assertions::assert_eq!(account, account2);
    account.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    assert_ne!(account, account2);
}

#[test]
fn test_special_limit_accounts() {
    assert_ne!(SPECIAL_LIMIT_ACCOUNTS.len(), 0);
}
