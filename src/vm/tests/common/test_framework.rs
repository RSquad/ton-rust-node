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
#![allow(dead_code)]
include!("../../../common/src/log.rs");

use std::{os::raw::c_char, sync::LazyLock};
use ton_assembler::{compile_code, compile_code_to_builder, CompileError};
use ton_block::{
    BocWriter, Cell, Deserializable, Error, Exception, ExceptionCode, HashmapE, LibDescr,
    Libraries, MerkleProof, Message, Result, Serializable, ShardAccount, ShardStateUnsplit,
    SliceData, UInt256, UnixTime, SUPPORTED_VERSION,
};
use ton_vm::{
    error::{tvm_exception, tvm_exception_code, tvm_exception_or_custom_code},
    executor::{gas::gas_state::Gas, BehaviorModifiers, Engine},
    smart_contract_info::SmartContractInfo,
    stack::{savelist::SaveList, Stack, StackItem},
};

fn logger_init() {
    // do not init twice
    if log::log_enabled!(log::Level::Info) {
        return;
    }
    let log_level =
        if cfg!(feature = "verbose") { log::LevelFilter::Trace } else { log::LevelFilter::Info };
    let output_file = if cfg!(feature = "log_file") { Some("log/log.txt") } else { None };
    init_log_without_config(Some("{m}"), log_level, output_file);
}

pub struct TestCaseInputs {
    code: Option<String>,
    bytecode: Option<Cell>,
    ctrls: SaveList,
    stack: Stack,
    refs: Vec<Cell>,
    time: Option<i32>,
    gas: Option<Gas>,
    library: Option<Cell>,
    mc_state_proof: Option<Cell>,
    shard_account: Option<ShardAccount>,
    message: Option<Cell>,
    behavior_modifiers: Option<BehaviorModifiers>,
    capabilities: u64,
    block_version: u32,
    skip_fift_check: bool,
}

impl TestCaseInputs {
    pub fn new(code: String, stack: Stack, refs: Vec<Cell>, capabilities: u64) -> TestCaseInputs {
        logger_init();
        TestCaseInputs {
            code: Some(code),
            bytecode: None,
            ctrls: SaveList::new(),
            stack,
            refs,
            time: None,
            gas: None,
            library: None,
            mc_state_proof: None,
            shard_account: None,
            message: None,
            behavior_modifiers: None,
            capabilities,
            block_version: SUPPORTED_VERSION,
            skip_fift_check: false,
        }
    }

    pub fn with_bytecode(code: Cell) -> TestCaseInputs {
        logger_init();
        TestCaseInputs {
            code: None,
            bytecode: Some(code),
            ctrls: SaveList::new(),
            stack: Stack::new(),
            refs: Vec::new(),
            time: None,
            gas: None,
            library: None,
            mc_state_proof: None,
            shard_account: None,
            message: None,
            behavior_modifiers: None,
            capabilities: 0,
            block_version: SUPPORTED_VERSION,
            skip_fift_check: false,
        }
    }

    pub fn with_mc_state(mut self, mc_state_root: Cell) -> TestCaseInputs {
        assert!(mc_state_root.level() == 0, "state should not contain pruned cells");
        let mc_state_proof = MerkleProof {
            hash: mc_state_root.repr_hash(),
            depth: mc_state_root.repr_depth(),
            proof: mc_state_root,
        };
        self.mc_state_proof = Some(mc_state_proof.serialize().unwrap());
        self
    }

    pub fn with_mc_state_proof(mut self, mc_state_proof: Cell) -> TestCaseInputs {
        self.mc_state_proof = Some(mc_state_proof);
        self
    }

    // it should be serialized ShardAccount
    pub fn with_account(mut self, shard_account: ShardAccount) -> TestCaseInputs {
        self.shard_account = Some(shard_account);
        self
    }

    pub fn with_message_cell(mut self, message: Cell) -> TestCaseInputs {
        self.message = Some(message);
        self
    }

    pub fn with_ref(mut self, cell: Cell) -> TestCaseInputs {
        assert!(self.refs.len() < 4);
        self.refs.push(cell);
        self
    }

    pub fn with_refs(mut self, refs: Vec<Cell>) -> TestCaseInputs {
        self.refs = refs;
        self
    }

    pub fn with_root_data(self, root_data: Cell) -> TestCaseInputs {
        self.with_ctrl(4, StackItem::Cell(root_data))
    }

    pub fn with_temp_data(self, temp_data: SmartContractInfo) -> TestCaseInputs {
        self.with_ctrl(7, temp_data.as_temp_data_item())
    }

    // do not run with stack - use refs, then do PUSHREF*
    pub fn with_stack(mut self, stack: Stack) -> TestCaseInputs {
        self.stack = stack;
        self
    }

    pub fn _with_capability(mut self, capability: ton_block::GlobalCapabilities) -> TestCaseInputs {
        self.skip_fift_check = true;
        self.capabilities |= capability as u64;
        self
    }

    pub fn with_block_version(mut self, block_version: u32) -> TestCaseInputs {
        // self.skip_fift_check = block_version < 10;
        self.block_version = block_version;
        self
    }

    pub fn skip_fift_check(mut self) -> TestCaseInputs {
        self.skip_fift_check = true;
        self
    }

    pub fn with_ctrl(mut self, ctrl: usize, item: StackItem) -> TestCaseInputs {
        self.ctrls.put(ctrl, item).expect("test arguments must be valid");
        self
    }

    pub fn with_time(mut self, time: u32) -> TestCaseInputs {
        self.time = Some(time as i32);
        self
    }

    pub fn with_gas(mut self, gas: Gas) -> TestCaseInputs {
        self.gas = Some(gas);
        self
    }

    pub fn with_gas_limit(self, gas_limit: i64) -> TestCaseInputs {
        self.with_gas(Gas::test_with_limit(gas_limit))
    }

    pub fn with_library(mut self, library: HashmapE) -> TestCaseInputs {
        self.library = ton_block::HashmapType::inner(library);
        self
    }

    pub fn with_library_root(mut self, library: Cell) -> TestCaseInputs {
        self.library = Some(library);
        self
    }

    pub fn with_behavior_modifiers(
        mut self,
        behavior_modifiers: BehaviorModifiers,
    ) -> TestCaseInputs {
        self.skip_fift_check = true;
        self.behavior_modifiers = Some(behavior_modifiers);
        self
    }

    pub fn expect_bytecode(self, bytecode: Vec<u8>) -> TestCaseInputs {
        self.expect_bytecode_extended(bytecode, None)
    }

    pub fn expect_bytecode_extended(
        self,
        bytecode: Vec<u8>,
        message: Option<&str>,
    ) -> TestCaseInputs {
        let inputcode = SliceData::new(bytecode);
        let compilation_result = compile_code(self.code.as_ref().unwrap());
        match compilation_result {
            Ok(ref selfcode) => {
                let mut selfcode = selfcode.clone();
                let mut bytevec = vec![];
                while selfcode.remaining_bits() != 0 {
                    bytevec.append(&mut selfcode.get_bytestring(0));
                    if selfcode.remaining_references() > 0 {
                        selfcode = SliceData::load_cell(selfcode.reference(0).unwrap()).unwrap();
                    } else {
                        break;
                    }
                }
                bytevec.push(0x80);
                let selfcode = SliceData::new(bytevec);
                if !selfcode.eq(&inputcode) {
                    match message {
                        Some(msg) => panic!(
                            "{}Bytecode did not match:\n Expected: <{:x?}>\n But was: <{:x?}>",
                            msg, inputcode, selfcode
                        ),
                        None => panic!(
                            "Bytecode did not match:\n Expected: <{:x?}>\n But was: <{:x?}>",
                            inputcode, selfcode
                        ),
                    }
                };
            }
            Err(e) => match message {
                Some(msg) => panic!("{}{}", msg, e),
                None => panic!("{}", e),
            },
        }
        self
    }

    pub fn expect_compilation_failure(self, error: CompileError) -> TestCaseInputs {
        self.expect_compilation_failure_extended(error, None)
    }

    pub fn expect_compilation_failure_extended(
        self,
        error: CompileError,
        message: Option<&str>,
    ) -> TestCaseInputs {
        let compilation_result = compile_code(self.code.as_ref().unwrap());
        match message {
            None => {
                let actual = compilation_result.expect_err(&format!("Error expected {}", error));
                assert_eq!(
                    error, actual,
                    "Expected (left): <{}>, but was (right): <{}>.",
                    error, actual
                )
            }
            Some(msg) => {
                let actual =
                    compilation_result.expect_err(&format!("{}. Error expected {}", msg, error));
                assert_eq!(
                    error, actual,
                    "{}\nExpected (left): <{}>, but was (right): <{}>.",
                    msg, error, actual
                )
            }
        }
        self
    }
}

impl From<TestCaseInputs> for TestCase {
    fn from(inputs: TestCaseInputs) -> Self {
        TestCase::new(inputs)
    }
}

pub struct TestCase {
    executor: Option<Engine>,
    compilation_result: std::result::Result<Cell, CompileError>,
    execution_result: Result<i32>,
}

impl TestCase {
    fn executor(&self, message: Option<&str>) -> &Engine {
        match self.executor {
            Some(ref exectuor) => exectuor,
            None => {
                let err = self.compilation_result.as_ref().unwrap_err();
                match message {
                    Some(msg) => panic!(
                        "{}No executor was created, because of bytecode compilation error {:?}",
                        msg, err
                    ),
                    None => panic!(
                        "No executor was created, because of bytecode compilation error {:?}",
                        err
                    ),
                }
            }
        }
    }
}

static EMPTY_LIBRARY: LazyLock<Cell> = LazyLock::new(|| {
    let lib_cell = Cell::default();
    let mut lib = LibDescr::new(lib_cell.clone());
    lib.publishers_mut().add_key(&UInt256::ZERO).unwrap();
    let mut library = Libraries::default();
    library.set(&lib_cell.repr_hash(), &lib).unwrap();
    library.root().unwrap().clone()
});

fn compare_with_fift(
    bytecode: Cell,
    data_cell: Option<Cell>,
    library: Option<Cell>,
    mc_state_proof: Option<Cell>,
    shard_account: Option<ShardAccount>,
    message: Option<Cell>,
    code: String,
    executor: Option<&Engine>,
    execution_result: &Result<i32>,
    time: i32,
    gas_remaining: i32,
    block_version: i32,
) {
    #[cfg(windows)]
    let lib_name = "../../ton/build/crypto/Release/vm_run_shared.dll";
    #[cfg(not(windows))]
    let lib_name = "../../ton-node-cpp/build/crypto/libvm_run_shared.so";
    assert!(std::fs::exists(lib_name).unwrap());
    let lib = libloading::Library::new(lib_name).expect("no shared dll found");
    let code_cell = if bytecode != Cell::default() {
        bytecode.clone()
    } else if let Some(shard_account) = shard_account.as_ref() {
        shard_account.read_account().unwrap().code().unwrap().clone()
    } else {
        panic!("Bytecode is empty and no account root provided");
    };
    let mut data = vec![];
    if data_cell.is_none()
        && library.is_none()
        && mc_state_proof.is_none()
        && shard_account.is_none()
        && message.is_none()
    {
        data = ton_block::write_boc(&code_cell).unwrap();
        // code is written to BOC and can be checked with FIFT
        // "fift.boc" file>B B>boc <s 1000000 0x48 runvmx .s
        // std::fs::write("../target/check/fift.boc", data.as_slice()).ok();
    } else if mc_state_proof.is_none() && shard_account.is_none() && message.is_none() {
        let roots = vec![
            code_cell,
            data_cell.unwrap_or_default(),
            library.unwrap_or_else(|| EMPTY_LIBRARY.clone()),
        ];
        let bag = BocWriter::with_roots(roots).unwrap();
        bag.write(&mut data).unwrap();
    } else {
        let mut roots = vec![
            code_cell,
            data_cell.unwrap_or_default(),
            library.unwrap_or_else(|| EMPTY_LIBRARY.clone()),
            shard_account.unwrap_or_default().serialize().unwrap(),
            mc_state_proof.unwrap_or_else(|| crate::common::MC_STATE_PROOF.clone()),
        ];
        if let Some(message) = message {
            roots.push(message);
        }
        let bag = BocWriter::with_roots(roots).unwrap();
        bag.write(&mut data).unwrap();
    };
    let size = data.len() * 8;
    let fift_result;
    unsafe {
        let run_boc: libloading::Symbol<
            unsafe extern "C" fn(*const u8, i32, i32, i32, i32, i32) -> *mut c_char,
        > = lib.get(b"run_vm_boc").unwrap();
        let free_mem: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_char> =
            lib.get(b"free_mem").unwrap();
        let log_mask = 0;
        // let log_mask = 10; // Stack + Gas
        let res = run_boc(data.as_ptr(), size as i32, time, gas_remaining, block_version, log_mask);
        assert!(!res.is_null(), "Fift execution failed, check fift logs");
        fift_result = std::ffi::CStr::from_ptr(res).to_string_lossy().trim().to_string();
        free_mem(res);
    }
    let tvm_result = if let Some(executor) = executor {
        let gas = executor.gas_used();
        let committed_state_fift = executor.get_committed_state_fift();
        match &execution_result {
            Ok(result) => {
                let stack = executor.get_stack_result_fift();
                match stack.is_empty() {
                    true => format!("{} {} {}", result, gas, committed_state_fift),
                    false => format!("{} {} {} {}", stack, result, gas, committed_state_fift),
                }
            }
            Err(err) => {
                if let Some(ExceptionCode::OutOfGas) = tvm_exception_code(err) {
                    format!(
                        "{} {} {} {}",
                        gas,
                        !(ExceptionCode::OutOfGas as i32),
                        gas,
                        committed_state_fift
                    )
                } else {
                    let err = tvm_exception_or_custom_code(err);
                    format!("0 {} {} {}", err, gas, committed_state_fift)
                }
            }
        }
    } else {
        let err = execution_result.as_ref().unwrap_err();
        let err = if let Some(ExceptionCode::OutOfGas) = tvm_exception_code(err) {
            !(ExceptionCode::OutOfGas as i32)
        } else {
            tvm_exception_or_custom_code(err)
        };
        format!("0 {} 0 {}", err, Engine::get_empty_committed_state_fift())
    };
    if tvm_result != fift_result {
        log::info!("bytecode: {}\n", hex::encode(bytecode.data()));
        log::info!("code:\n{}\n", code);
        assert_eq!(tvm_result, fift_result, "fift check: {:?}", execution_result);
    }
}

impl TestCase {
    pub(super) fn new(mut args: TestCaseInputs) -> TestCase {
        let code = args.code.unwrap_or_default();
        let mut bytecode = if let Some(bytecode) = args.bytecode {
            bytecode
        } else {
            match compile_code_to_builder(&code) {
                Ok(mut bytecode) => {
                    assert!(
                        bytecode.references_free() >= args.refs.len(),
                        "Cannot use 4 refs with long code"
                    );
                    for reference in args.refs.drain(..).rev() {
                        bytecode.checked_prepend_reference(reference).unwrap();
                    }
                    bytecode.into_cell().unwrap()
                }
                Err(e) => {
                    return TestCase {
                        executor: None,
                        compilation_result: Err(e),
                        execution_result: Ok(-1),
                    };
                }
            }
        };
        if !args.stack.is_empty() {
            args.skip_fift_check = true;
        }
        let mut time = args.time.unwrap_or(UnixTime::now() as i32);
        let mut libraries = vec![HashmapE::with_hashmap(256, args.library.clone())];
        if args.mc_state_proof.is_some() || args.shard_account.is_some() || args.message.is_some() {
            let mc_state_root = if let Some(mc_state_proof) = &args.mc_state_proof {
                let mc_state_proof =
                    MerkleProof::construct_from_cell(mc_state_proof.clone()).unwrap();
                let mc_state_root = mc_state_proof.proof.clone().virtualize(1);
                let mc_state =
                    ShardStateUnsplit::construct_from_cell(mc_state_root.clone()).unwrap();
                if !mc_state.libraries().is_empty() {
                    libraries.push(mc_state.libraries().clone().inner());
                }
                time = mc_state.gen_time() as i32 + 1;
                Some(mc_state_root)
            } else {
                None
            };
            let balance = if let Some(shard_account) = &args.shard_account {
                let account = shard_account.read_account().unwrap();

                if bytecode == Cell::default() {
                    if let Some(code) = account.code() {
                        bytecode = code.clone();
                    }
                }
                if let Some(data) = account.data() {
                    args.ctrls.put(4, StackItem::Cell(data.clone())).unwrap();
                }
                account.balance().map_or(0, |value| value.coins.as_u128())
            } else {
                0
            };
            if let Some(message_root) = &args.message {
                let message = Message::construct_from_cell(message_root.clone()).unwrap();
                if let Some(state_init) = message.state_init() {
                    if !state_init.libraries().is_empty() {
                        libraries.push(state_init.libraries().clone().inner());
                    }
                }
                let value = message.value().map_or(0, |value| value.coins.as_u128());
                assert!(args.stack.is_empty(), "Stack must be empty when using real data");
                args.stack
                    .push(StackItem::int(balance))
                    .push(StackItem::int(value))
                    .push(StackItem::Cell(message_root.clone()))
                    .push(StackItem::Slice(message.body().cloned().unwrap_or_default()))
                    .push(StackItem::boolean(!message.is_internal()));
            }
            let mut smci = SmartContractInfo::with_params(
                args.shard_account.as_ref(),
                args.message.clone(),
                mc_state_root,
            )
            .unwrap();
            smci.set_mycode(bytecode.clone());
            args.ctrls.put(7, smci.as_temp_data_item()).unwrap();
        }
        let data = args.ctrls.get(4).map(|data| data.as_cell().cloned().unwrap_or_default());
        let gas = args.gas.unwrap_or_else(Gas::test);
        // debug_assert_eq!(args.capabilities, 0);
        let setup_result = Engine::with_capabilities(args.capabilities).setup_checked(
            bytecode.clone(),
            args.ctrls,
            args.stack,
            gas.clone(),
            libraries,
        );
        let (execution_result, executor) = match setup_result {
            Ok(mut executor) => {
                executor.set_block_version(args.block_version);
                if let Some(modifiers) = args.behavior_modifiers {
                    executor.modify_behavior(modifiers);
                }
                (executor.execute(), Some(executor))
            }
            Err(err) => (Err(err), None),
        };
        if cfg!(feature = "fift_check") && !args.skip_fift_check {
            compare_with_fift(
                bytecode.clone(),
                data,
                args.library,
                args.mc_state_proof,
                args.shard_account,
                args.message,
                code,
                executor.as_ref(),
                &execution_result,
                time,
                gas.get_gas_remaining() as i32,
                args.block_version as i32,
            )
        }
        TestCase { executor, compilation_result: Ok(bytecode), execution_result }
    }

    pub fn get_root(self) -> Option<Cell> {
        if let Some(eng) = self.executor {
            if let Some((c, _)) = eng.get_committed_state() {
                return Some(c.clone());
            }
        }
        None
    }

    pub fn get_actions(self) -> Option<Cell> {
        if let Some(eng) = self.executor {
            if let Some((_, c)) = eng.get_committed_state() {
                return Some(c.clone());
            }
        }
        None
    }
}

pub trait Expects {
    fn expect_stack(self, stack: &Stack) -> TestCase;
    fn expect_stack_extended(self, stack: &Stack, message: Option<&str>) -> TestCase;
    fn expect_empty_stack(self) -> TestCase;
    fn expect_int_stack(self, stack_contents: &[i32]) -> TestCase;
    fn expect_item(self, stack_item: StackItem) -> TestCase;
    fn expect_item_extended(self, stack_item: StackItem, message: Option<&str>) -> TestCase;
    fn expect_success(self) -> TestCase;
    fn expect_success_extended(self, message: Option<&str>) -> TestCase;
    fn expect_ctrl(self, ctrl: usize, item: &StackItem) -> TestCase;
    fn expect_ctrl_extended(self, ctrl: usize, item: &StackItem, message: Option<&str>)
        -> TestCase;
    fn expect_failure(self, exception_code: ExceptionCode) -> TestCase;
    fn expect_custom_failure(self, custom_code: i32) -> TestCase;
    fn expect_custom_failure_extended<F: Fn(&Exception) -> bool>(
        self,
        op: F,
        exc_name: &str,
        message: Option<&str>,
    ) -> TestCase;
    fn expect_failure_extended(
        self,
        exception_code: ExceptionCode,
        message: Option<&str>,
    ) -> TestCase;
    fn expect_root_data(self, cell: Cell) -> TestCase;
    fn expect_same_results(self, other: Self);
    fn expect_gas(
        self,
        max_gas_limit: i64,
        gas_limit: i64,
        gas_credit: i64,
        gas_remaining: i64,
    ) -> TestCase;
    fn expect_gas_used(self, gas_used: i64) -> TestCase;
    fn expect_steps(self, steps: u32) -> TestCase;
    fn execute(self) -> TestCase;
    fn stack(self) -> Stack;
}

impl<T: Into<TestCase>> Expects for T {
    fn expect_stack(self, stack: &Stack) -> TestCase {
        self.expect_stack_extended(stack, None)
    }

    fn expect_stack_extended(self, stack: &Stack, message: Option<&str>) -> TestCase {
        let test_case: TestCase = self.into();
        let executor = test_case.executor(message);
        match test_case.execution_result {
            Ok(_) => {
                if !executor.eq_stack(stack) {
                    if let Some(msg) = message {
                        log::info!(target: "tvm", "{}", msg)
                    }
                    log::info!(target: "tvm", "\nExpected stack: \n{}", stack);
                    log::info!(
                        target: "tvm",
                        "\n{}\n",
                        executor.dump_stack("Actual Stack:", false)
                    );
                    panic!("Stack is not expected")
                }
            }
            // TODO this is not quite right: execution may fail but still produce a stack
            Err(ref e) => {
                log::info!(target: "tvm", "\nExpected stack: \n{}", stack);
                print_failed_detail_extended(&test_case, e, message);
                panic!("Execution error: {:?}", e)
            }
        }
        test_case
    }

    fn expect_empty_stack(self) -> TestCase {
        self.expect_stack(&Stack::new())
    }

    // Order of items in array like in spec docs right item is top item
    fn expect_int_stack(self, stack_contents: &[i32]) -> TestCase {
        let mut stack = Stack::new();
        for element in stack_contents {
            stack.push(StackItem::int(*element));
        }
        self.expect_stack(&stack)
    }

    fn expect_item(self, stack_item: StackItem) -> TestCase {
        self.expect_item_extended(stack_item, None)
    }

    fn expect_item_extended(self, stack_item: StackItem, message: Option<&str>) -> TestCase {
        self.expect_stack_extended(Stack::new().push(stack_item), message)
    }

    fn expect_success(self) -> TestCase {
        self.expect_success_extended(None)
    }

    fn expect_success_extended(self, message: Option<&str>) -> TestCase {
        let test_case: TestCase = self.into();
        let executor = test_case.executor(message);
        print_stack(&test_case, executor);
        if let Err(ref e) = test_case.execution_result {
            match message {
                None => {
                    print_failed_detail_extended(&test_case, e, message);
                    panic!("Execution error: {:?}", e);
                }
                Some(msg) => {
                    print_failed_detail_extended(&test_case, e, message);
                    panic!("{}\nExecution error: {:?}", msg, e);
                }
            }
        }
        test_case
    }

    fn expect_ctrl(self, ctrl: usize, item: &StackItem) -> TestCase {
        self.expect_ctrl_extended(ctrl, item, None)
    }

    fn expect_ctrl_extended(
        self,
        ctrl: usize,
        item: &StackItem,
        message: Option<&str>,
    ) -> TestCase {
        let test_case: TestCase = self.into();
        let executor = test_case.executor(message);
        match test_case.execution_result {
            Ok(_) => executor.assert_ctrl(ctrl, item),
            Err(ref e) => {
                print_failed_detail_extended(&test_case, e, message);
                panic!("Execution error: {}", e);
            }
        };
        test_case
    }

    fn expect_failure(self, exception_code: ExceptionCode) -> TestCase {
        self.expect_failure_extended(exception_code, None)
    }

    fn expect_custom_failure_extended<F: Fn(&Exception) -> bool>(
        self,
        op: F,
        exc_name: &str,
        message: Option<&str>,
    ) -> TestCase {
        let test_case: TestCase = self.into();
        let executor = test_case.executor(message);
        match test_case.execution_result {
            Ok(_) => {
                log::info!(
                    target: "tvm",
                    "Expected failure: {}, however execution succeeded.",
                    exc_name
                );
                print_stack(&test_case, executor);
                match message {
                    None => panic!("Expected failure: {}, however execution succeeded.", exc_name),
                    Some(msg) => panic!(
                        "{}.\nExpected failure: {}, however execution succeeded.",
                        msg, exc_name
                    ),
                }
            }
            Err(ref e) => {
                if let Some(exception) = tvm_exception(e) {
                    if op(exception) {
                        let msg2 = &exception.comment;
                        match message {
                            Some(msg) => panic!(
                                "{} - {}\nNon expected exception: {}, expected: {}",
                                msg2, msg, e, exc_name
                            ),
                            None => panic!(
                                "{}\nNon expected exception: {}, expected: {}",
                                msg2, e, exc_name
                            ),
                        }
                    }
                } else {
                    let code = e.downcast_ref::<ExceptionCode>();
                    match code {
                        Some(code) => {
                            let e = Exception::from(*code);
                            if op(&e) {
                                panic!("Non expected exception: {}, expected: {}", e, exc_name)
                            }
                        }
                        None => {
                            if op(&Exception::from(ExceptionCode::FatalError)) {
                                panic!("Non expected exception: {}, expected: {}", e, exc_name)
                            }
                        }
                    }
                }
            }
        }
        test_case
    }

    fn expect_custom_failure(self, custom_code: i32) -> TestCase {
        self.expect_custom_failure_extended(
            |e| e.custom_code() != Some(custom_code),
            "custom exception",
            None,
        )
    }

    fn expect_failure_extended(
        self,
        exception_code: ExceptionCode,
        message: Option<&str>,
    ) -> TestCase {
        self.expect_custom_failure_extended(
            |e| e.exception_code() != Some(exception_code),
            &format!("{}", exception_code),
            message,
        )
    }

    fn expect_root_data(self, cell: Cell) -> TestCase {
        self.expect_ctrl(4, &StackItem::Cell(cell))
    }

    fn expect_same_results(self, other: Self) {
        let case1 = self.expect_success();
        let case2 = other.expect_success();
        let stack = case2.executor.unwrap().withdraw_stack();
        case1.expect_stack_extended(&stack, Some("results are not the same!"));
    }

    fn expect_gas(
        self,
        max_gas_limit: i64,
        gas_limit: i64,
        gas_credit: i64,
        gas_remaining: i64,
    ) -> TestCase {
        let test_case: TestCase = self.into();
        let gas = test_case.executor(None).get_gas();
        assert_eq!(gas.get_gas_max(), max_gas_limit, "{:?}", gas);
        assert_eq!(gas.get_gas_limit(), gas_limit, "{:?}", gas);
        assert_eq!(gas.get_gas_credit(), gas_credit, "{}", gas.get_gas_remaining());
        assert_eq!(gas.get_gas_remaining(), gas_remaining);
        test_case
    }

    fn expect_gas_used(self, gas_used: i64) -> TestCase {
        let test_case: TestCase = self.into();
        let gas = test_case.executor(None).get_gas();
        assert_eq!(gas.get_gas_used(), gas_used);
        test_case
    }

    fn expect_steps(self, steps: u32) -> TestCase {
        let test_case: TestCase = self.into();
        assert_eq!(test_case.executor(None).steps(), steps);
        test_case
    }

    fn execute(self) -> TestCase {
        let test_case: TestCase = self.into();
        test_case.executor(None);
        test_case
    }

    fn stack(self) -> Stack {
        let test_case: TestCase = self.into();
        test_case.executor(None).stack().clone()
    }
}

fn print_stack(test_case: &TestCase, executor: &Engine) {
    if test_case.execution_result.is_ok() {
        log::info!(target: "tvm", "Post-execution:\n");
        log::info!(target: "tvm", "{}", executor.dump_stack("Post-execution stack state", false));
        log::info!(target: "tvm", "{}", executor.dump_ctrls(false));
    }
}

#[allow(dead_code)]
fn print_failed_detail(case: &TestCase, exception: &Error) {
    print_failed_detail_extended(case, exception, None)
}

fn print_failed_detail_extended(case: &TestCase, error: &Error, message: Option<&str>) {
    log::info!(target: "tvm", "exception: {:?}\n", error);
    let msg2 = tvm_exception(error).map_or(String::new(), |e| e.comment.clone());
    match message {
        Some(ref msg) => log::info!(
            target: "tvm",
            "{} failed with {} {}.\nBytecode: {:x?}\n",
            msg, error, msg2, case.compilation_result
        ),
        None => log::info!(
            target: "tvm",
            "failed with {} {}.\nBytecode: {:x?}\n",
            error, msg2, case.compilation_result
        ),
    }
}

pub fn test_case_with_refs(code: impl ToString, references: Vec<Cell>) -> TestCaseInputs {
    TestCaseInputs::new(code.to_string(), Stack::new(), references, 0)
}

pub fn test_case_with_ref(code: impl ToString, reference: Cell) -> TestCaseInputs {
    TestCaseInputs::new(code.to_string(), Stack::new(), vec![reference], 0)
}

pub fn test_case(code: impl ToString) -> TestCaseInputs {
    TestCaseInputs::new(code.to_string(), Stack::new(), vec![], 0)
}

pub fn test_case_with_bytecode(code: Cell) -> TestCaseInputs {
    TestCaseInputs::with_bytecode(code)
}

pub fn test_case_with_real_data(
    mc_state_proof: &str,
    account: &str,
    message: &str,
) -> TestCaseInputs {
    let mc_state_proof = Cell::read_from_file(mc_state_proof);
    let account = Cell::read_from_file(account);
    let shard_account = ShardAccount::with_account_root(account, Default::default(), 0);
    let message = Cell::read_from_file(message);
    TestCaseInputs::with_bytecode(Cell::default())
        .with_mc_state_proof(mc_state_proof)
        .with_account(shard_account)
        .with_message_cell(message)
}
