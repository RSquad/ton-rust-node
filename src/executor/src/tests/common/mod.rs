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
#![allow(dead_code)]
#![allow(clippy::duplicate_mod)]
#![allow(clippy::field_reassign_with_default)]

pub(crate) mod cross_check;

use crate::{
    BlockchainConfig, ExecuteParams, ExecutorError, OrdinaryTransactionExecutor,
    TickTockTransactionExecutor, TransactionExecutor,
};
#[cfg(feature = "cross_check")]
use std::sync::Arc;
use std::sync::LazyLock;
use ton_assembler::compile_code_to_cell;
use ton_block::{
    base64_decode, read_single_root_boc, read_single_root_boc_file, AccStatusChange, Account,
    AccountId, AccountStatus, AccountStorage, AddSub, BuilderData, Cell, Coins, ComputeSkipReason,
    ConfigParam8, ConfigParamEnum, ConfigParams, CurrencyCollection, Deserializable,
    ExternalInboundMessageHeader, GetRepresentationHash, HashmapAugType, HashmapType, InRefValue,
    InternalMessageHeader, KeyExtBlkRef, McStateExtra, MerkleProof, Message, MsgAddressInt,
    OutAction, OutActions, Result, Serializable, ShardAccount, ShardIdent, ShardStateUnsplit,
    SimpleLib, SliceData, StateInit, StorageInfo, StorageUsageCalc, StorageUsed, TickTock,
    TrActionPhase, TrBouncePhase, TrComputePhase, TrComputePhaseSkipped, TrComputePhaseVm,
    TrCreditPhase, TrStoragePhase, Transaction, TransactionDescr, TransactionDescrOrdinary,
    TransactionTickTock, UInt15, UInt256, VarUInteger32, VarUInteger7, DICT_HASH_MIN_CELLS,
};
use ton_vm::{smart_contract_info::PrevBlocksInfo, stack::read_stack_item};

include!("../../../../common/src/log.rs");
include!("../../../../block/src/tests/test_utils.rs");

pub const BLOCK_LT: u64 = 2_000_000_000;
pub const PREV_BLOCK_LT: u64 = 1_998_000_000;
pub const ACCOUNT_UT: u32 = 1572169011;
pub const BLOCK_UT: u32 = 1576526553;
pub const MSG1_BALANCE: u64 = 50_000_000;
pub const MSG2_BALANCE: u64 = 100_000_000;
pub const MSG_FWD_FEE: u64 = 10_000_000;
pub const MSG_MINE_FEE: u64 = 3_333_282;

pub static SENDER_ACCOUNT: AccountId = AccountId::with_uint256([0x11; 128]);
pub static RECEIVER_ACCOUNT: AccountId = AccountId::with_uint256([0x22; 128]);
pub static THIRD_ACCOUNT: AccountId = AccountId::with_uint256([0x33; 128]);
pub static BLOCKCHAIN_CONFIG: LazyLock<BlockchainConfig> = LazyLock::new(default_config);
pub static SIMPLE_MC_STATE: LazyLock<Cell> =
    LazyLock::new(|| mc_state_proof_cell_with_config(BLOCKCHAIN_CONFIG.raw_config().clone(), None));

pub fn mc_state_cell_with_config(config: ConfigParams) -> ShardStateUnsplit {
    let mc_seqno = 1234567;
    let mut mc_state = ShardStateUnsplit::with_ident(ShardIdent::masterchain());
    mc_state.set_seq_no(mc_seqno);
    mc_state.set_global_id(42);
    let mut extra = McStateExtra { config, ..Default::default() };
    for i in 0..16 {
        extra.prev_blocks.add_fake_id(mc_seqno - i, false).unwrap();
        extra.prev_blocks.add_fake_id((mc_seqno / 100) * 100 - i * 100, false).unwrap();
    }
    extra.prev_blocks.add_fake_id(0, true).unwrap();
    extra.prev_blocks.add_fake_id(mc_seqno - 30, true).unwrap();
    extra.last_key_block = extra.prev_blocks.get(&(mc_seqno - 30)).unwrap().map(|r| r.blk_ref);
    mc_state.write_custom(Some(&extra)).unwrap();
    mc_state
}

pub fn make_proof_cell(p: &impl Serializable) -> Cell {
    let proof = p.serialize().unwrap();
    let proof = MerkleProof { hash: proof.hash(0).clone(), depth: proof.depth(0), proof };
    proof.serialize().unwrap()
}

pub fn mc_state_proof_cell_with_config(config: ConfigParams, libs: Option<Cell>) -> Cell {
    let mut mc_state = mc_state_cell_with_config(config);
    if let Some(libs) = libs {
        *mc_state.libraries_mut() = ton_block::Libraries::with_hashmap(Some(libs));
    }
    make_proof_cell(&mc_state)
}

pub fn create_config(cfg_name: &str) -> Result<ConfigParams> {
    ConfigParams::construct_from_file(cfg_name)
}

// pub fn create_config(cfg_name: &str) -> Result<ConfigParams> {
//     use ton_block::SUPPORTED_VERSION;
//     let mut config = ConfigParams::construct_from_file(cfg_name)?;
//     // assert_eq!(config.global_version(), SUPPORTED_VERSION, "global version must be {}", SUPPORTED_VERSION);
//     // config.update_param(8, |param_opt| {
//     //     if let Some(ton_block::ConfigParamEnum::ConfigParam8(param)) = param_opt.as_mut() {
//     //         param.global_version.version = SUPPORTED_VERSION;
//     //     }
//     // }).unwrap();
//     // config.update_param(19, |param_opt| {
//     //     *param_opt = Some(ton_block::ConfigParamEnum::ConfigParam19(795));
//     // }).unwrap();
//     config.update_param(43, |param_opt| {
//         *param_opt = None;
//     }).unwrap();
//     config.write_to_file(cfg_name).unwrap();
//     Ok(config)
// }

#[allow(dead_code)]
pub fn custom_config(version: Option<u32>, capabilities: Option<u64>) -> BlockchainConfig {
    let mut config = create_config("real_boc/config.boc").unwrap();
    let mut param8 = ConfigParam8 { global_version: config.get_global_version().unwrap() };
    if let Some(version) = version {
        param8.global_version.version = version
    }
    if let Some(capabilities) = capabilities {
        param8.global_version.capabilities |= capabilities
    }
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    BlockchainConfig::with_config(config).unwrap()
}

pub fn default_config() -> BlockchainConfig {
    BlockchainConfig::with_config(create_config("real_boc/config.boc").unwrap()).unwrap()
}

#[cfg(not(feature = "cross_check"))]
pub fn execute_params(last_tr_lt: u64) -> ExecuteParams {
    ExecuteParams {
        block_unixtime: BLOCK_UT,
        block_lt: last_tr_lt - last_tr_lt % 1_000_000,
        last_tr_lt,
        ..ExecuteParams::default()
    }
}

#[cfg(feature = "cross_check")]
pub fn execute_params(last_tr_lt: u64) -> ExecuteParams {
    enum DebugType {
        None,
        Simple,
        Emulator,
    }
    let debug = DebugType::None;
    // let _ = cross_check::DisableCrossCheck::new();
    let (verbosity, pattern, trace_callback) = match debug {
        DebugType::None => (4, None, None),
        DebugType::Simple => (2048 + 4, Some("{m}"), None),
        DebugType::Emulator => {
            let emulator_trace_callback: Option<Arc<ton_vm::executor::TraceCallback>> =
                Some(Arc::new(ton_vm::executor::Engine::emulator_trace_callback));
            (2048 + 4, Some("{m}"), emulator_trace_callback)
        }
    };
    init_log_without_config(pattern, log::LevelFilter::Debug, None);
    cross_check::set_cross_check_verbosity(verbosity);
    ExecuteParams {
        block_unixtime: BLOCK_UT,
        block_lt: last_tr_lt - last_tr_lt % 1_000_000,
        last_tr_lt,
        trace_callback,
        debug: !matches!(debug, DebugType::None),
        ..ExecuteParams::default()
    }
}

pub fn execute_params_none() -> ExecuteParams {
    execute_params(BLOCK_LT + 1)
}

pub fn execute_params_simple(last_tr_lt: u64, block_unixtime: u32) -> ExecuteParams {
    let mut params = execute_params(last_tr_lt);
    params.block_unixtime = block_unixtime;
    params
}

pub fn create_two_internal_messages() -> (Message, Message) {
    let msg1 = create_int_msg(
        SENDER_ACCOUNT.clone(),
        THIRD_ACCOUNT.clone(),
        MSG1_BALANCE,
        false,
        BLOCK_LT + 2,
    );
    let msg2 = create_int_msg(
        SENDER_ACCOUNT.clone(),
        THIRD_ACCOUNT.clone(),
        MSG2_BALANCE,
        true,
        BLOCK_LT + 3,
    );
    (msg1, msg2)
}

pub fn create_two_messages_data() -> Cell {
    let (msg1, msg2) = create_two_internal_messages();

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    b.into_cell().unwrap()
}

pub fn create_two_messages_data_2(src: AccountId, w_id: i8) -> Cell {
    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, w_id, src.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, w_id, src).unwrap());

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    b.into_cell().unwrap()
}

pub fn create_int_msg_workchain(
    w_id: i8,
    src: AccountId,
    dest: AccountId,
    value: impl Into<Coins>,
    bounce: bool,
    lt: u64,
) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, w_id, src).unwrap(),
        MsgAddressInt::with_standart(None, w_id, dest).unwrap(),
        CurrencyCollection::from_coins(value.into()),
    );
    hdr.bounce = bounce;
    hdr.created_lt = lt;
    Message::with_int_header(hdr)
}

pub fn create_int_msg(
    src: AccountId,
    dest: AccountId,
    value: impl Into<Coins>,
    bounce: bool,
    lt: u64,
) -> Message {
    create_int_msg_workchain(-1, src, dest, value, bounce, lt)
}

pub fn create_send_two_messages_code() -> Cell {
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
    ",
    )
    .unwrap()
}

pub fn create_test_account_workchain(
    amount: impl Into<Coins>,
    w_id: i8,
    address: AccountId,
    code: Cell,
    data: Cell,
) -> Account {
    let mut account = Account::with_storage(
        &MsgAddressInt::with_standart(None, w_id, address).unwrap(),
        &StorageInfo::with_values(ACCOUNT_UT, None),
        &AccountStorage::active(
            0,
            CurrencyCollection::from_coins(amount.into()),
            StateInit::default(),
        ),
    );
    account.set_code(code);
    account.set_data(data);
    account.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    account
}

pub fn create_test_account(
    amount: impl Into<Coins>,
    address: AccountId,
    code: Cell,
    data: Cell,
) -> Account {
    create_test_account_workchain(amount, -1, address, code, data)
}

pub fn create_test_external_msg() -> Message {
    create_test_external_msg_with_address(SENDER_ACCOUNT.clone())
}

pub fn create_test_external_msg_with_address(acc_id: AccountId) -> Message {
    let hdr = ExternalInboundMessageHeader::new(
        Default::default(),
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap(),
    );
    Message::with_ext_in_header_and_body(hdr, SliceData::default())
}

pub fn check_account_and_transaction_balances(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
) {
    if trans.is_none() {
        // no checks needed
        return;
    }

    let trans = trans.unwrap();

    let mut left = acc_before.balance().cloned().unwrap_or_default();
    if let Some(value) = msg.get_value() {
        left.add(value).unwrap();
    }

    let mut right = acc_after.balance().cloned().unwrap_or_default();
    right.add(trans.total_fees()).unwrap();
    right.coins.add(trans.blackhole_burned()).unwrap();
    trans
        .iterate_out_msgs(|out_msg| {
            if let Some(header) = out_msg.int_header() {
                right.add(header.value())?;
                right.coins.add(header.fwd_fee())?;
            }
            Ok(true)
        })
        .unwrap();
    pretty_assertions::assert_eq!(left, right);

    // check fees
    let descr = trans.read_description().unwrap();
    if let TransactionDescr::Ordinary(descr) = descr {
        let total_fee = trans.total_fees().clone();

        let mut fees = CurrencyCollection::default();
        fees.coins += descr.storage_ph.as_ref().map_or(0, |st| st.storage_fees_collected.as_u128());
        if let Some(storage) = descr.storage_ph.as_ref() {
            if storage.status_change == AccStatusChange::Deleted {
                if descr.credit_first {
                    fees.add(&msg.get_value().cloned().unwrap_or_default()).unwrap();
                    fees.coins -= msg.get_value().cloned().unwrap_or_default().coins;
                }
                fees.add(&acc_before.balance().cloned().unwrap_or_default()).unwrap();
                fees.coins -= acc_before.balance().map_or(0, |cc| cc.coins.as_u128());
            }
        }
        if let Some(cr) = descr.credit_ph.as_ref() {
            if let Some(g) = cr.due_fees_collected.as_ref() {
                fees.coins += *g
            }
        }
        if let TrComputePhase::Vm(cp) = &descr.compute_ph {
            fees.coins += cp.gas_fees
        }
        if let Some(ap) = descr.action.as_ref() {
            fees.coins += ap.total_action_fees()
        }
        if let Some(TrBouncePhase::Ok(bp)) = descr.bounce.as_ref() {
            fees.coins += bp.msg_fees
        }
        let addr = msg.dst().unwrap();
        let is_special =
            BLOCKCHAIN_CONFIG.is_special_account(addr.is_masterchain(), addr.address()).unwrap();
        let is_ext_msg = msg.is_inbound_external();
        if is_ext_msg && !is_special {
            let config = BLOCKCHAIN_CONFIG.raw_config();
            let in_msg_cell = msg.serialize().unwrap();
            let mut calc = StorageUsageCalc::with_limits(0, 0);
            calc.append_cell(&in_msg_cell, false, &mut 0).unwrap();
            let fwd_prices = config.fwd_prices(msg.is_masterchain()).unwrap();
            let in_fwd_fee = fwd_prices.calc_fwd_fee(calc.bits(), calc.cells());
            fees.coins += in_fwd_fee;
        }
        pretty_assertions::assert_eq!(fees, total_fee);
    }

    // check messages fees
    let mut fwd_fees = Coins::zero();
    trans
        .iterate_out_msgs(|out_msg| {
            if let Some(header) = out_msg.int_header() {
                fwd_fees.add(&header.fwd_fee)?;
            }
            Ok(true)
        })
        .unwrap();

    let descr = trans.read_description().unwrap();
    if let TransactionDescr::Ordinary(descr) = descr {
        let mut trans_fwd_fee = Coins::zero();
        if let Some(ap) = descr.action.as_ref() {
            if ap.success {
                trans_fwd_fee += ap.total_fwd_fees() - ap.total_action_fees()
            }
        }
        if descr.bounce.is_some() {
            if let TrBouncePhase::Ok(bp) = &descr.bounce.as_ref().unwrap() {
                trans_fwd_fee += bp.fwd_fees;
            }
        }
        pretty_assertions::assert_eq!(fwd_fees, trans_fwd_fee);
    }

    // check logical time
    if !acc_after.is_none() {
        pretty_assertions::assert_eq!(
            trans.logical_time() + trans.msg_count() as u64 + 1,
            acc_after.last_tr_time().unwrap()
        );
    }

    // other checks
    pretty_assertions::assert_eq!(trans.orig_status, acc_before.status());
    // if frozen account hash equals to addr, it will be uninitialized
    // assert_eq!(trans.end_status, acc_after.status());
}

pub fn check_account_and_transaction(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
    result_account_balance: impl Into<Coins>,
    count_out_msgs: usize,
) {
    if let Some(trans) = trans {
        pretty_assertions::assert_eq!(
            (trans.out_msgs.len().unwrap(), acc_after.balance().cloned().unwrap_or_default()),
            (count_out_msgs, CurrencyCollection::from_coins(result_account_balance.into()))
        );
    }
    check_account_and_transaction_balances(acc_before, acc_after, msg, trans);
}

pub fn execute_with_params(
    mc_state_proof: Cell,
    in_msg_cell: Option<Cell>,
    acc: &mut Account,
    params: &ExecuteParams,
) -> Result<Transaction> {
    let proof = MerkleProof::construct_from_cell(mc_state_proof.clone())?;
    let mc_state = ShardStateUnsplit::construct_from_cell(proof.proof.clone())?;
    let config = if mc_state_proof == *SIMPLE_MC_STATE {
        BLOCKCHAIN_CONFIG.to_owned()
    } else {
        BlockchainConfig::with_config(mc_state.read_custom()?.unwrap().config)?
    };
    #[cfg(feature = "cross_check")]
    let mc_state_proof = if let Some(data) = ton_block::HashmapType::data(&params.state_libs) {
        let mut mc_state = mc_state;
        *mc_state.libraries_mut() = ton_block::Libraries::with_hashmap(Some(data.clone()));
        make_proof_cell(&mc_state)
    } else {
        mc_state_proof
    };
    let block_version = config.global_version();
    let dict_hash_min_cells = config.size_limits_config().acc_state_cells_for_storage_dict;
    let executor: Box<dyn TransactionExecutor> = if in_msg_cell.is_none() {
        let tt = acc.get_tick_tock().unwrap();
        let tt = match (tt.tick, tt.tock) {
            (true, false) => TransactionTickTock::Tick,
            (false, true) => TransactionTickTock::Tock,
            (_tick, _tock) => panic!("must be tick or tock"),
        };
        Box::new(TickTockTransactionExecutor::new(config, tt))
    } else {
        Box::new(OrdinaryTransactionExecutor::new(config))
    };
    #[cfg(feature = "cross_check")]
    let acc_before = acc.clone();
    let trans = executor.execute_with_params(in_msg_cell.clone(), acc, params.clone());
    if trans.is_ok() {
        if block_version < 11 {
            acc.del_storage_stat();
        } else {
            acc.calc_storage_stat_dict(dict_hash_min_cells).unwrap();
        }
    }
    #[cfg(feature = "cross_check")]
    cross_check::cross_check(
        mc_state_proof,
        &acc_before,
        acc,
        in_msg_cell.as_ref(),
        trans.as_ref().ok(),
        params,
        0,
    );
    trans
}

pub fn execute(msg: &Message, acc: &mut Account, tr_lt: u64) -> Result<Transaction> {
    let msg_cell = msg.serialize()?;
    let acc_before = acc.clone();
    let params = execute_params(tr_lt);
    let trans = execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), acc, &params);
    check_account_and_transaction_balances(&acc_before, acc, msg, trans.as_ref().ok());
    trans
}

pub fn execute_c(
    msg: &Message,
    acc: &mut Account,
    tr_lt: u64,
    result_account_balance: impl Into<Coins>,
    count_out_msgs: usize,
) -> Result<Transaction> {
    let msg_cell = msg.serialize()?;
    let acc_before = acc.clone();
    let params = execute_params(tr_lt);
    let trans = execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), acc, &params);
    check_account_and_transaction(
        &acc_before,
        acc,
        &msg,
        trans.as_ref().ok(),
        result_account_balance,
        count_out_msgs,
    );
    trans
}

pub struct ExecutionCaseResult {
    pub acc: Account,
    pub tr_res: Result<Transaction>,
}

impl ExecutionCaseResult {
    pub fn expect_balance(&self, expect_balance: u64) -> &Self {
        pretty_assertions::assert_eq!(
            self.acc.balance().map_or(0, |cc| cc.coins.as_u128()),
            expect_balance as u128,
            "expected balance {expect_balance}"
        );
        self
    }
    pub fn expect_status(&self, status: AccountStatus) -> &Self {
        pretty_assertions::assert_eq!(self.acc.status(), status, "expected status");
        self
    }
    pub fn expect_count_out_msgs(&self, count: usize) -> &Self {
        let tr = self.tr();
        pretty_assertions::assert_eq!(
            tr.out_msgs.len().unwrap(),
            count,
            "expected count out messages"
        );
        self
    }
    pub fn expect_not_aborted(&self) -> &Self {
        assert!(!self.read_ordinary_description().aborted, "expected transaction not aborted");
        self
    }
    pub fn read_ordinary_description(&self) -> TransactionDescrOrdinary {
        let TransactionDescr::Ordinary(descr) = self.tr().read_description().unwrap() else {
            panic!("Not found ordinary description")
        };
        descr
    }
    pub fn read_compute_phase(&self) -> TrComputePhaseVm {
        let TransactionDescr::Ordinary(descr) = self.tr().read_description().unwrap() else {
            panic!("Not found description")
        };
        let TrComputePhase::Vm(vm) = descr.compute_ph else { panic!("Not found compute phase") };
        vm
    }
    pub fn expect_compute_skipped(&self, reason: ComputeSkipReason) -> &Self {
        let descr = self.read_ordinary_description();
        pretty_assertions::assert_eq!(
            descr.compute_ph,
            TrComputePhase::Skipped(TrComputePhaseSkipped { reason })
        );
        self
    }
    pub fn expect_compute_result(&self, exit_code: i32) -> &Self {
        let vm = self.read_compute_phase();
        pretty_assertions::assert_eq!(vm.exit_code, exit_code, "{:?}", vm);
        self
    }
    pub fn expect_gas_used(&self, gas: u64) -> &Self {
        let vm = self.read_compute_phase();
        assert_eq!(vm.gas_used.as_u64(), gas, "expected gas used {}", vm.gas_used);
        self
    }
    pub fn expect_action_failed(&self, result_code: i32) -> &Self {
        let descr = self.read_ordinary_description();
        let Some(action) = descr.action else { panic!("no action phase") };
        assert!(!action.success, "expected action failed with code {result_code}");
        pretty_assertions::assert_eq!(
            action.result_code,
            result_code,
            "expected action failed with code {result_code}"
        );
        self
    }
    pub fn expect_action_success(&self, tot_actions: i16) -> &Self {
        let descr = self.read_ordinary_description();
        let Some(action) = descr.action else { panic!("no action phase") };
        assert!(
            action.success,
            "expected action success but it failed with code {}",
            action.result_code
        );
        pretty_assertions::assert_eq!(
            action.tot_actions,
            tot_actions,
            "expected action success with {tot_actions} actions"
        );
        self
    }
    pub fn expect_no_bounce(&self) -> &Self {
        let descr = self.read_ordinary_description();
        assert!(descr.bounce.is_none(), "expected no bounce phase");
        self
    }
    pub fn expect_bounce_no_funds(&self) -> &Self {
        let descr = self.read_ordinary_description();
        assert!(
            matches!(descr.bounce, Some(TrBouncePhase::Nofunds(_))),
            "expected bounce phase with no funds"
        );
        self
    }
    pub fn expect_bounce_success(&self) -> &Self {
        let descr = self.read_ordinary_description();
        assert!(matches!(descr.bounce, Some(TrBouncePhase::Ok(_))));
        self
    }
    pub fn expect_storage_fees_due(&self, due: u64) -> &Self {
        let descr = self.read_ordinary_description();
        if due == 0 {
            pretty_assertions::assert_eq!(
                descr.storage_ph.unwrap().storage_fees_due,
                None,
                "No storage fees due expected"
            )
        } else {
            pretty_assertions::assert_eq!(
                descr.storage_ph.unwrap().storage_fees_due,
                Some(due.into()),
                "wrong storage fees due expected {due}"
            )
        }
        self
    }
    pub fn expect_credit(&self, credit: u64) -> &Self {
        let descr = self.read_ordinary_description();
        pretty_assertions::assert_eq!(
            descr.credit_ph.unwrap().credit,
            CurrencyCollection::with_coins(credit),
            "credit mismatch"
        );
        self
    }
    pub(crate) fn expect_err(&self, err: ExecutorError) -> &Self {
        pretty_assertions::assert_eq!(self.tr_res.as_ref().unwrap_err().downcast_ref(), Some(&err),);
        self
    }
    pub(crate) fn expect_no_accept(&self) {
        assert!(matches!(
            self.tr_res.as_ref().unwrap_err().downcast_ref(),
            Some(ExecutorError::NoAcceptError(_, _))
        ))
    }
    fn tr(&self) -> &Transaction {
        self.tr_res.as_ref().expect("expected transaction")
    }
}

pub fn execute_transaction_case(
    start_balance: u64,
    code: &str,
    data: Cell,
    msg_value: u64,
    bounce: bool,
) -> ExecutionCaseResult {
    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, msg_value, bounce, BLOCK_LT - 2);

    let tr_res = execute(&msg, &mut acc, BLOCK_LT + 1);
    ExecutionCaseResult { acc, tr_res }
}

pub fn execute_acc_with_message(mut acc: Account, msg: &Message) -> ExecutionCaseResult {
    let tr_res = execute(msg, &mut acc, BLOCK_LT + 1);
    ExecutionCaseResult { acc, tr_res }
}

pub fn execute_acc_with_message_and_time(
    mut acc: Account,
    msg: &Message,
    now: u32,
) -> ExecutionCaseResult {
    let msg_cell = msg.serialize().unwrap();
    let acc_before = acc.clone();
    let params = execute_params_simple(BLOCK_LT + 1, now);
    let tr_res = execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), &mut acc, &params);
    check_account_and_transaction_balances(&acc_before, &acc, msg, tr_res.as_ref().ok());
    ExecutionCaseResult { acc, tr_res }
}

pub fn execute_custom_transaction(
    start_balance: u64,
    code: &str,
    data: Cell,
    msg_balance: u64,
    bounce: bool,
    result_account_balance: impl Into<Coins>,
    count_out_msgs: usize,
) -> (Account, Transaction) {
    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, msg_balance, bounce, BLOCK_LT - 2);
    let trans =
        execute_c(&msg, &mut acc, BLOCK_LT + 1, result_account_balance, count_out_msgs).unwrap();
    (acc, trans)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_custom_transaction_with_extra_balance(
    start_balance: u64,
    start_extra_balance: u64,
    code: &str,
    data: Cell,
    msg_balance: u64,
    result_account_balance: u64,
    result_account_extra_balance: u64,
    count_out_msgs: usize,
) {
    let acc_id = SENDER_ACCOUNT.clone();
    let code = compile_code_to_cell(code).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let mut acc_balance = acc.balance().unwrap().clone();
    acc_balance
        .other
        .set(&11111111u32, &VarUInteger32::from_two_u128(0, start_extra_balance.into()).unwrap())
        .unwrap();
    acc.set_balance(acc_balance);
    let msg = create_int_msg(THIRD_ACCOUNT.clone(), acc_id, msg_balance, false, BLOCK_LT - 2);

    let tr_lt = BLOCK_LT + 1;
    let trans = execute(&msg, &mut acc, tr_lt).unwrap();

    pretty_assertions::assert_eq!(trans.out_msgs.len().unwrap(), count_out_msgs);
    let mut answer = CurrencyCollection::with_coins(result_account_balance);
    answer
        .other
        .set(
            &11111111u32,
            &VarUInteger32::from_two_u128(0, result_account_extra_balance.into()).unwrap(),
        )
        .unwrap();
    pretty_assertions::assert_eq!(acc.balance().unwrap_or(&CurrencyCollection::default()), &answer)
}

pub fn gen_test_account() -> Account {
    generate_test_account(true, AccountTestOptions::with_default_setup(true))
}

pub fn get_tr_descr(tr: &Transaction) -> TransactionDescrOrdinary {
    if let TransactionDescr::Ordinary(descr) = tr.read_description().unwrap() {
        descr
    } else {
        panic!("Not found description")
    }
}

pub fn compare_transaction(trans: &Transaction, good_trans: &Transaction) {
    pretty_assertions::assert_eq!(
        trans.read_description().unwrap(),
        good_trans.read_description().unwrap(),
        "description mismatch"
    );
    trans
        .out_msgs
        .scan_diff(&good_trans.out_msgs, |key: UInt15, msg1, msg2| {
            if let (Some(InRefValue(msg1)), Some(InRefValue(msg2))) = (&msg1, &msg2) {
                let value1 = msg1.int_header().map(|h| h.value().clone()).unwrap_or_default();
                let value2 = msg2.int_header().map(|h| h.value().clone()).unwrap_or_default();
                value1
                    .other
                    .scan_diff(&value2.other, |key: u32, val1, val2| {
                        pretty_assertions::assert_eq!(val1, val2, "for key {}", key);
                        Ok(true)
                    })
                    .unwrap();
            }
            pretty_assertions::assert_eq!(msg1, msg2, "for key {}", key.0);
            Ok(true)
        })
        .unwrap();
    pretty_assertions::assert_eq!(trans, good_trans, "transaction mismatch")
}

pub fn replay_transaction(
    c: Option<(&mut criterion::Criterion, &str)>,
    acc: &str,
    acc_after: &str,
    tr: &str,
    prev: &str,
    mc_state_proof: Cell,
) {
    println!("prepare to read account");
    let mut account = if let Ok(data) = base64_decode(acc) {
        if let Ok(shard_acc) = ShardAccount::construct_from_bytes(&data) {
            shard_acc.read_account().unwrap()
        } else {
            Account::construct_from_bytes(&data).unwrap()
        }
    } else {
        let cell = read_single_root_boc(std::fs::read(acc).unwrap()).unwrap();
        if let Ok(shard_acc) = ShardAccount::construct_from_full_cell(cell.clone()) {
            shard_acc.read_account().unwrap()
        } else {
            Account::construct_from_cell(cell).unwrap()
        }
    };
    println!("prepare to read transaction");
    let transaction = if let Ok(data) = base64_decode(tr) {
        Transaction::construct_from_bytes(&data).unwrap()
    } else {
        Transaction::construct_from_file(tr).unwrap()
    };
    // account.write_to_file("real_boc/storage_limit_old.boc").unwrap();
    // transaction.write_to_file("real_boc/storage_limit_transaction.boc").unwrap();
    let old_root = account.serialize().unwrap();
    let hash_update = transaction.read_state_update().unwrap();
    println!("transaction hash: {:X}", transaction.serialize().unwrap().repr_hash());
    println!("account hash: {:X}", old_root.repr_hash());
    pretty_assertions::assert_eq!(&hash_update.old_hash, old_root.repr_hash());
    // dbg!(&account, &transaction, transaction.read_in_msg().unwrap());
    let message = transaction.read_in_msg().unwrap();
    let msg_cell = transaction.in_msg_cell();
    let account_after = Account::construct_from_file(acc_after).unwrap_or_else(|_| account.clone());

    let mut left = account.balance().cloned().unwrap_or_default().coins;
    if let Some(msg) = message.as_ref() {
        if let Some(value) = msg.get_value() {
            left.add(&value.coins).unwrap();
        }
    }

    let mc = MerkleProof::construct_from_cell(mc_state_proof.clone()).unwrap();
    let mc = ShardStateUnsplit::construct_from_cell(mc.proof.clone()).unwrap();
    let extra = mc.read_custom().unwrap().unwrap();
    let state_libs = mc.libraries().clone().inner();

    // let config = ton_block_json::debug_possible_config_params(&extra.config_params).unwrap();
    // std::fs::write("d:\\ton_config.json", config).unwrap();

    let tx_lt = transaction.logical_time();
    let mut params = execute_params(tx_lt);
    params.seed_block = UInt256::rand();
    params.state_libs = state_libs;
    params.block_unixtime = transaction.now();
    let data = base64_decode(prev).unwrap();
    params.prev_blocks_info = if let Ok(cell) = read_single_root_boc(data) {
        let mut slice = SliceData::load_cell(cell).unwrap();
        PrevBlocksInfo::Tuple(read_stack_item(&mut slice).unwrap())
    } else {
        let last_mc_id = KeyExtBlkRef::fake(extra.after_key_block, mc.seq_no(), mc.gen_lt());
        PrevBlocksInfo::Raw(last_mc_id, extra.prev_blocks)
    };
    if let Some((c, name)) = c {
        c.bench_function(name, |b| {
            b.iter(|| {
                execute_with_params(
                    mc_state_proof.clone(),
                    msg_cell.clone(),
                    &mut account.clone(),
                    &params,
                )
            })
        });
    }

    let mut our_transaction =
        execute_with_params(mc_state_proof.clone(), msg_cell, &mut account, &params).unwrap();

    let mut right = account.balance().cloned().unwrap_or_default().coins;
    right.add(&our_transaction.total_fees().coins).unwrap();
    right.add(&our_transaction.blackhole_burned()).unwrap();
    our_transaction
        .iterate_out_msgs(|out_msg| {
            if let Some(header) = out_msg.int_header() {
                right.add(&header.value().coins)?;
                right.add(header.fwd_fee())?;
            }
            Ok(true)
        })
        .unwrap();
    pretty_assertions::assert_eq!(left, right);

    our_transaction.set_prev_trans_hash(transaction.prev_trans_hash().clone());
    our_transaction.set_prev_trans_lt(transaction.prev_trans_lt());
    our_transaction.write_state_update(&hash_update).unwrap();
    // our_transaction.write_to_file(tr).unwrap();

    let max = transaction.msg_count().max(our_transaction.msg_count());
    for i in 0..max {
        pretty_assertions::assert_eq!(
            our_transaction.get_out_msg(i).ok(),
            transaction.get_out_msg(i).ok(),
            "output message {i} mismatch",
        )
    }

    // if let (Some(our_gas_used), Some(gas_used)) = (our_transaction.gas_used(), transaction.gas_used()) {
    //     if our_gas_used.abs_diff(gas_used) == 1 {
    //         let mut transaction = transaction.clone();
    //         transaction.set_gas_used(our_gas_used as u32).unwrap();
    //         assert_eq!(our_transaction, transaction);
    //         transaction.write_to_file(tr).unwrap();
    //     }
    // }
    // pretty_assertions::assert_eq!(our_transaction, transaction);
    pretty_assertions::assert_eq!(
        our_transaction.read_description().unwrap(),
        transaction.read_description().unwrap()
    );

    // account.write_to_file(acc_after).unwrap();
    let new_root = account.serialize().unwrap();
    // let hash_update = ton_block::HashUpdate::with_hashes(old_hash.clone(), new_hash.clone());
    // our_transaction.write_state_update(&hash_update).unwrap();
    // our_transaction.write_to_file(tr).unwrap();
    // account.write_to_file(acc_after).unwrap();
    if hash_update.new_hash == *new_root.repr_hash() {
        pretty_assertions::assert_eq!(our_transaction, transaction);
        pretty_assertions::assert_eq!(account, account_after);
        return;
    }
    // the new account can be unrechable in the blockchain - try to save new one
    // it will be correct because of hash cheking in state update of transaction
    // account.write_to_file("real_boc/account_new.boc").unwrap();
    pretty_assertions::assert_eq!(account, account_after);
}

pub fn read_config(cfg: &str) -> Result<ConfigParams> {
    println!("prepare to read config");
    let config = if let Ok(data) = base64_decode(cfg) {
        let data = read_single_root_boc(data).unwrap();
        if let Ok(config) = ConfigParams::construct_from_cell(data.clone()) {
            println!("config params read as base64");
            config
        } else {
            println!("config hashmap read as base64");
            ConfigParams::with_root(data)?
        }
    } else if let Ok(config) = create_config(cfg) {
        println!("config read from file boc {cfg}");
        config
        // let config = ton_block_json::debug_possible_config_params(&config).unwrap();
        // std::fs::write("d:\\config.json", config).unwrap();
    } else if let Ok(data) = read_single_root_boc(std::fs::read(cfg).unwrap()) {
        println!("config hashmap read from boc");
        ConfigParams::with_root(data)?
    } else {
        println!("config read from file as json");
        let json: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(cfg).unwrap()).unwrap();
        ton_block_json::parse_config(json.get("config").unwrap().as_object().unwrap())?
        // let cfg = cfg.replace("json", "boc");
        // config.write_to_file(&cfg).unwrap();
    };
    // let mut config = config;
    // config.config_addr = [0x55; 32].into();
    // config.write_to_file(cfg).unwrap();
    Ok(config)
}

pub fn replay_transaction_by_files(acc: &str, acc_after: &str, tr: &str, cfg: &str) {
    replay_transaction_full(acc, acc_after, tr, cfg, "", "")
}

pub fn replay_transaction_full(
    acc: &str,
    acc_after: &str,
    tr: &str,
    cfg: &str,
    prev: &str,
    libs: &str,
) {
    let config = read_config(cfg).unwrap();
    assert!(config.valid_config_data(false, None).unwrap());
    let libs = if libs.is_empty() {
        None
    } else if let Ok(data) = std::fs::read(libs) {
        Some(read_single_root_boc(data).unwrap())
    } else {
        None
    };
    let mc_state_proof = mc_state_proof_cell_with_config(config, libs);
    replay_transaction(None, acc, acc_after, tr, prev, mc_state_proof)
}

pub fn replay_with_mc_state(mc: &str, cfg: &str, acc: &str, acc_after: &str, tr: &str) {
    let mut mc_state = ShardStateUnsplit::construct_from_file(mc).unwrap();
    let config = read_config(cfg).unwrap();
    let mut extra = mc_state.read_custom().unwrap().unwrap();
    println!("prev blocks len: {}", extra.prev_blocks.len().unwrap());
    extra.config = config;
    mc_state.write_custom(Some(&extra)).unwrap();
    let mc_state_proof = make_proof_cell(&mc_state);
    replay_transaction(None, acc, acc_after, tr, "", mc_state_proof)
}

pub fn replay_with_mc_state_proof(mc: &str, acc: &str, acc_after: &str, tr: &str) {
    let mc_state_proof = read_single_root_boc_file(mc).unwrap();
    replay_transaction(None, acc, acc_after, tr, "", mc_state_proof)
}

pub fn replay_with_key_block_proof(mc: &str, acc: &str, acc_after: &str, tr: &str) {
    let mc_state_proof = read_single_root_boc_file(mc).unwrap();
    replay_transaction(None, acc, acc_after, tr, "", mc_state_proof)
}

pub fn try_replay_transaction(
    account: &mut Account,
    message: Option<&Message>,
    config: BlockchainConfig,
    params: &ExecuteParams,
) -> Result<Transaction> {
    let mc_state_proof = mc_state_proof_cell_with_config(config.raw_config().clone(), None);
    let msg_cell = message.map(|msg| msg.serialize().unwrap());
    execute_with_params(mc_state_proof, msg_cell, account, params)
}

#[derive(Default)]
pub(crate) struct TransactionTestCase {
    pub(crate) bounce: Option<TrBouncePhase>,
    pub(crate) end_balance: u64,
    pub(crate) gas_credit: Option<u16>,
    pub(crate) gas_fees: Option<u64>,
    pub(crate) gas_limit: Option<u32>,
    pub(crate) gas_used: u32,
    pub(crate) lt_delta: u64,
    pub(crate) msg_income: u64,
    pub(crate) no_last_paid: bool,
    pub(crate) out_actions: OutActions,
    pub(crate) phase_action: TrActionPhase,
    pub(crate) phase_compute_vm: TrComputePhaseVm,
    pub(crate) start_balance: u64,
    pub(crate) storage_fee: u64,
    pub(crate) total_fees: u64,
    pub(crate) transaction_aborted: bool,
    pub(crate) transaction_credit_first: bool,
    pub(crate) workchain: Option<i8>,
}

impl TransactionTestCase {
    pub(crate) fn expect_action_fail_with_one_message(&mut self) {
        self.phase_action.success = false;
        self.phase_action.valid = true;
        self.phase_action.status_change = AccStatusChange::Unchanged;
        self.phase_action.tot_actions = self.out_actions.len() as i16;
        self.phase_action.msgs_created = 1;
        self.phase_action.add_fwd_fees(&(MSG_FWD_FEE).into());
        self.phase_action.action_list_hash = self.out_actions.hash().unwrap();
        self.phase_action.result_code = 37;
        self.phase_action.result_arg = Some(1);
        self.phase_action.no_funds = true;
        self.phase_action.tot_msg_size = StorageUsed::with_values_checked(1, 705).unwrap();
        self.transaction_credit_first = false;
        self.transaction_aborted = true;
    }

    pub(crate) fn expect_action_success_with_two_messages(
        &mut self,
        msg1: &Message,
        msg2: &Message,
    ) {
        self.phase_action.success = true;
        self.phase_action.valid = true;
        self.phase_action.status_change = AccStatusChange::Unchanged;
        self.phase_action.tot_actions = self.out_actions.len() as i16;
        self.phase_action.msgs_created = 2;
        self.phase_action.add_fwd_fees(&(2 * MSG_FWD_FEE).into());
        self.phase_action.add_action_fees(&(2 * MSG_MINE_FEE).into());
        self.phase_action.action_list_hash = self.out_actions.hash().unwrap();
        append_message(&mut self.phase_action.tot_msg_size, &msg1).unwrap();
        append_message(&mut self.phase_action.tot_msg_size, &msg2).unwrap();
        self.transaction_credit_first = true;
        self.transaction_aborted = false;
    }

    pub(crate) fn expect_compute_vm_success(&mut self, steps: u32) {
        self.phase_compute_vm.success = true;
        self.phase_compute_vm.gas_credit = self.gas_credit.map(Into::into);
        self.phase_compute_vm.gas_fees = self.gas_fees().into();
        self.phase_compute_vm.gas_limit = self.gas_limit.map_or(VarUInteger7::new(100), Into::into);
        self.phase_compute_vm.gas_used = self.gas_used.into();
        self.phase_compute_vm.vm_steps = steps;
    }

    pub(crate) fn expect_end_balance(&mut self, balance: u64) {
        self.end_balance = balance
    }

    pub(crate) fn expect_total_fees(&mut self, fees: u64) {
        self.total_fees = fees
    }

    pub(crate) fn expect_two_out_messages(&mut self, mode: u8) -> (Message, Message) {
        let (mut msg1, mut msg2) = create_two_internal_messages();
        self.out_actions.push_back(OutAction::new_send(mode, msg1.clone()));
        self.out_actions.push_back(OutAction::new_send(mode, msg2.clone()));
        let msg_remain_fee = MSG_FWD_FEE - MSG_MINE_FEE;
        let int_header = msg1.int_header_mut().unwrap();
        int_header.value.coins = (MSG1_BALANCE - MSG_FWD_FEE).into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
        let int_header = msg2.int_header_mut().unwrap();
        int_header.value.coins = (MSG2_BALANCE - MSG_FWD_FEE).into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
        (msg1, msg2)
    }

    pub(crate) fn gas_fees(&self) -> u64 {
        self.gas_fees.unwrap_or_else(|| self.gas_used as u64 * 10000)
    }

    pub(crate) fn create_int_msg(&self, bounce: bool) -> Message {
        create_int_msg_workchain(
            self.workchain.unwrap_or(-1),
            THIRD_ACCOUNT.clone(),
            SENDER_ACCOUNT.clone(),
            self.msg_income,
            bounce,
            PREV_BLOCK_LT,
        )
    }
}

pub(crate) struct TransactionTestContext {
    pub(crate) acc: Account,
    pub(crate) msg: Message,
    pub(crate) new_acc: Account,
    pub(crate) tr_lt: u64,
}

impl TransactionTestContext {
    pub(crate) fn with_params(
        code: Cell,
        data: Cell,
        msg: Option<Message>,
        test_case: &TransactionTestCase,
    ) -> Self {
        let wc = test_case.workchain.unwrap_or(-1);
        let mut acc = create_test_account_workchain(
            test_case.start_balance,
            wc,
            SENDER_ACCOUNT.clone(),
            code.clone(),
            data.clone(),
        );
        if !test_case.no_last_paid {
            acc.set_last_paid(BLOCK_UT - 100);
        }
        let msg = msg.unwrap_or_else(|| {
            create_int_msg_workchain(
                wc,
                THIRD_ACCOUNT.clone(),
                SENDER_ACCOUNT.clone(),
                test_case.msg_income,
                true,
                PREV_BLOCK_LT,
            )
        });
        let mut new_acc = create_test_account_workchain(
            test_case.end_balance,
            wc,
            SENDER_ACCOUNT.clone(),
            code,
            data,
        );
        if !test_case.no_last_paid {
            new_acc.set_last_paid(BLOCK_UT);
        }
        new_acc.set_last_tr_time(BLOCK_LT + test_case.lt_delta);
        Self { acc, msg, new_acc, tr_lt: BLOCK_LT + 1 }
    }

    pub(crate) fn execute(&mut self, count_out_msgs: usize) -> Result<Transaction> {
        let msg_cell = self.msg.serialize()?;
        let acc_before = self.acc.clone();
        let params = execute_params_simple(self.tr_lt, BLOCK_UT);
        let trans =
            execute_with_params(SIMPLE_MC_STATE.to_owned(), Some(msg_cell), &mut self.acc, &params);
        check_account_and_transaction(
            &acc_before,
            &self.acc,
            &self.msg,
            trans.as_ref().ok(),
            self.new_acc.balance().unwrap().coins,
            count_out_msgs,
        );
        self.new_acc.set_last_paid(BLOCK_UT);
        assert_eq!(self.acc, self.new_acc);
        trans
    }
    pub(crate) fn create_sample_transaction(self, test_case: TransactionTestCase) -> Transaction {
        let mut trans =
            Transaction::with_account_and_message(&self.new_acc, &self.msg, self.tr_lt).unwrap();
        trans.set_total_fees(CurrencyCollection::with_coins(test_case.total_fees));
        trans.set_now(BLOCK_UT);
        let mut description = TransactionDescrOrdinary::default();
        description.storage_ph = Some(TrStoragePhase {
            storage_fees_collected: test_case.storage_fee.into(),
            storage_fees_due: None,
            status_change: AccStatusChange::Unchanged,
        });
        description.credit_ph = if test_case.gas_credit.is_some() {
            None
        } else {
            Some(TrCreditPhase {
                due_fees_collected: None,
                credit: CurrencyCollection::with_coins(test_case.msg_income),
            })
        };
        description.compute_ph = TrComputePhase::Vm(test_case.phase_compute_vm);
        description.action = Some(test_case.phase_action);
        description.credit_first = test_case.transaction_credit_first;
        description.bounce = test_case.bounce;
        description.aborted = test_case.transaction_aborted;
        description.destroyed = false;
        let description = TransactionDescr::Ordinary(description);
        trans.write_description(&description).unwrap();
        trans
    }

    pub(crate) fn set_library(&mut self, code: Cell, public: bool) {
        let key = code.repr_hash().write_to_bitstring().unwrap();
        let value = SimpleLib::new(code, public).write_to_new_cell().unwrap();
        self.acc.library_mut().unwrap().set_raw(key.clone(), &value).unwrap();
        self.acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
        self.new_acc.library_mut().unwrap().set_raw(key, &value).unwrap();
        self.new_acc.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap();
    }
}

/// append cells and bits count without limits
/// except other currencies
pub fn append_message(storage: &mut StorageUsed, msg: &Message) -> Result<()> {
    let mut calc = StorageUsageCalc::with_limits(0, 0);
    // don't calc storage for Extra Currencies
    let root = if let Some(copy) = msg.copy_without_extra_currencies() {
        copy.serialize()?
    } else {
        msg.serialize()?
    };
    calc.append_cell(&root, true, &mut 0)?;
    let other = calc.storage_used()?;
    storage.add_bits_and_cells(other.bits(), other.cells());
    Ok(())
}
