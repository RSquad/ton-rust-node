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
use crate::{
    error::{tvm_exception_code, tvm_exception_full, tvm_exception_or_custom_code, TvmError},
    executor::{
        continuation::{switch, switch_to_c0},
        engine::handlers::Handlers,
        gas::gas_state::Gas,
        math::DivMode,
        microcode::{CTRL, VAR},
        storage::fetch_stack,
        types::{
            Instruction, InstructionExt, InstructionOptions, InstructionParameter, LengthAndIndex,
            RegisterPair, RegisterTrio, WhereToGetParams,
        },
    },
    smart_contract_info::SmartContractInfo,
    stack::{
        continuation::{ContinuationData, ContinuationType},
        integer::IntegerData,
        savelist::SaveList,
        Stack, StackItem,
    },
};
use std::{
    collections::HashSet,
    ops::Range,
    sync::{Arc, LazyLock, Mutex},
};
use ton_block::{
    error, fail, BuilderData, Cell, CellType, Deserializable, Error, Exception, ExceptionCode,
    GasConsumer, GlobalCapabilities, HashmapE, IBitstring, Result, SliceData, Status, UInt256,
};

pub(super) type ExecuteHandler = fn(&mut Engine) -> Status;

pub(super) struct SliceProto {
    data_window: Range<usize>,
    references_window: Range<usize>,
}

impl Default for SliceProto {
    fn default() -> Self {
        Self { data_window: 0..0, references_window: 0..0 }
    }
}

impl SliceProto {
    fn pos(&self) -> usize {
        self.data_window.start
    }
}

impl From<&SliceData> for SliceProto {
    fn from(slice: &SliceData) -> Self {
        Self {
            data_window: slice.pos()..slice.pos() + slice.remaining_bits(),
            references_window: slice.get_references(),
        }
    }
}

pub type TraceCallback = dyn Fn(&Engine, &EngineTraceInfo) + Send + Sync + 'static;

#[derive(Debug)]
pub struct RunChildVm {
    pub code: SliceData,
    pub data: StackItem,
    pub c7: StackItem,
    pub stack: Stack,
    pub gas_max: i64,
    pub gas_limit: i64,
    pub same_c3: bool,
    pub return_data: bool,
    pub return_actions: bool,
    pub return_gas: bool,
    pub isolate_gas: bool,
    pub ret_vals: i32,
}

pub struct Engine {
    pub(in crate::executor) cc: ContinuationData,
    pub(in crate::executor) cmd: InstructionExt,
    pub(in crate::executor) ctrls: SaveList,
    pub(in crate::executor) libraries: Vec<HashmapE>, // 256 bit dictionaries
    pub(in crate::executor) modifiers: BehaviorModifiers,
    pub(in crate::executor) checked_signatures_count: usize,
    // get_extra_balance_counter: usize, // TODO: what is it?
    // SliceData::load_cell() is faster than trying to cache SliceData for each
    // visited cell with HashMap<UInt256, SliceData>
    visited_cells: HashSet<UInt256>,
    cstate: Option<(Cell, Cell)>,
    time: u64,
    gas: Gas,
    free_gas_consumed: i64, // gas consumed by free operations
    code_page: isize,
    debug_on: isize, // status of debug can be recursively incremented
    step: u32,       // number of executable command
    debug_buffer: String,
    cmd_code: SliceProto, // start of current cmd
    last_cmd: u8,
    trace: u8,
    trace_callback: Option<Arc<TraceCallback>>,
    log_string: Option<&'static str>,
    max_data_depth: u16,
    capabilities: u64,
    block_version: u32,
}

#[derive(Debug, Clone, Default)]
pub struct BehaviorModifiers {
    pub chksig_always_succeed: bool,
}

#[derive(Eq, Debug, PartialEq)]
pub enum EngineTraceInfoType {
    Start,
    Normal,
    Finish,
    Implicit,
    Exception,
    Dump,
}

pub struct EngineTraceInfo<'a> {
    pub info_type: EngineTraceInfoType,
    pub step: u32, // number of executable command
    pub cmd_str: String,
    pub cmd_code: SliceData, // start of current cmd
    pub stack: &'a Stack,
    pub gas_used: i64,
    pub gas_cmd: i64,
}

impl EngineTraceInfo<'_> {
    pub fn has_cmd(&self) -> bool {
        matches!(self.info_type, EngineTraceInfoType::Normal | EngineTraceInfoType::Implicit)
    }
}

impl GasConsumer for Engine {
    fn finalize_cell(&mut self, builder: BuilderData) -> Result<Cell> {
        self.use_gas(Gas::finalize_price());
        builder
            .finalize(1024)
            .map_err(|err| error!(ExceptionCode::CellOverflow, "finalize cell error: {:?}", err))
    }
    fn load_cell(&mut self, cell: Cell) -> Result<SliceData> {
        self.load_hashed_cell(cell, true)
    }
    fn finalize_cell_and_load(&mut self, builder: BuilderData) -> Result<SliceData> {
        let cell = self.finalize_cell(builder)?;
        self.load_hashed_cell(cell, true)
    }
}

static HANDLERS_CP0: LazyLock<Handlers> = LazyLock::new(Handlers::new_code_page_0);

impl Engine {
    pub const TRACE_NONE: u8 = 0x00;
    pub const TRACE_CODE: u8 = 0x01;
    pub const TRACE_GAS: u8 = 0x02;
    pub const TRACE_STACK: u8 = 0x04;
    pub const TRACE_CTRLS: u8 = 0x08;
    pub const TRACE_ALL: u8 = 0xFF;
    pub const TRACE_ALL_BUT_CTRLS: u8 = 0x07;

    // External API ***********************************************************

    pub fn with_capabilities(capabilities: u64) -> Engine {
        let trace = if cfg!(feature = "verbose") {
            Engine::TRACE_ALL
        } else if cfg!(feature = "fift_check") {
            Engine::TRACE_ALL_BUT_CTRLS
        } else {
            Engine::TRACE_NONE
        };
        let log_enabled = log::log_enabled!(target: "tvm", log::Level::Debug)
            || log::log_enabled!(target: "tvm", log::Level::Trace)
            || log::log_enabled!(target: "tvm", log::Level::Info)
            || log::log_enabled!(target: "tvm", log::Level::Error)
            || log::log_enabled!(target: "tvm", log::Level::Warn);
        let trace_callback: Option<Arc<TraceCallback>> = if !log_enabled {
            None
        } else if cfg!(feature = "fift_check") {
            Some(Arc::new(Self::fift_trace_callback))
        } else if cfg!(feature = "verbose") {
            Some(Arc::new(Self::default_trace_callback))
        } else {
            Some(Arc::new(Self::simple_trace_callback))
        };
        Engine {
            cc: ContinuationData::new_empty(),
            cmd: InstructionExt::default(),
            ctrls: SaveList::new(),
            libraries: Vec::new(),
            modifiers: BehaviorModifiers::default(),
            checked_signatures_count: 0,
            visited_cells: HashSet::new(),
            cstate: None,
            time: 0,
            gas: Gas::empty(),
            free_gas_consumed: 0,
            code_page: 0,
            debug_on: 1,
            step: 0,
            debug_buffer: String::new(),
            cmd_code: SliceProto::default(),
            last_cmd: 0,
            trace,
            trace_callback,
            log_string: None,
            max_data_depth: 512,
            capabilities,
            block_version: 0,
        }
    }

    pub fn set_block_version(&mut self, block_version: u32) {
        self.block_version = block_version
    }

    pub fn set_max_data_depth(&mut self, max_data_depth: u16) {
        self.max_data_depth = max_data_depth
    }

    pub fn assert_ctrl(&self, ctrl: usize, item: &StackItem) -> &Engine {
        match self.ctrls.get(ctrl) {
            Some(x) => assert!(Stack::eq_item(x, item)),
            None => unreachable!("ctrl[{}] is empty", ctrl),
        }
        self
    }

    pub fn assert_stack(&self, stack: &Stack) -> &Engine {
        assert!(self.cc.stack.eq(stack));
        self
    }

    pub fn check_capabilities(&self, capabilities: u64) -> bool {
        (self.capabilities & capabilities) == capabilities
    }

    pub fn check_capability(&self, capability: GlobalCapabilities) -> Status {
        if (self.capabilities & capability as u64) == 0 {
            fail!(ExceptionCode::InvalidOpcode, "{:?} is absent", capability)
        } else {
            Ok(())
        }
    }

    pub fn block_version(&self) -> u32 {
        self.block_version
    }

    pub fn eq_stack(&self, stack: &Stack) -> bool {
        self.cc.stack.eq(stack)
    }

    pub fn stack(&self) -> &Stack {
        &self.cc.stack
    }

    pub fn try_use_gas(&mut self, gas: i64) -> Result<()> {
        self.gas.try_use_gas(gas)?;
        Ok(())
    }

    pub fn use_gas(&mut self, gas: i64) -> i64 {
        self.gas.use_gas(gas)
    }

    pub fn gas_used(&self) -> i64 {
        self.gas.get_gas_used_full()
    }

    pub fn gas_remaining(&self) -> i64 {
        self.gas.get_gas_remaining()
    }

    pub fn withdraw_stack(&mut self) -> Stack {
        std::mem::take(&mut self.cc.stack)
    }

    pub fn get_stack_result_fift(&self) -> String {
        self.cc.stack.iter().map(|item| item.dump_as_fift()).collect::<Vec<_>>().join(" ")
    }

    pub fn get_empty_committed_state_fift() -> String {
        "(null) (null)".to_string()
    }

    pub fn get_committed_state_fift(&self) -> String {
        if let Some((c4, c5)) = &self.cstate {
            format!("C{{{:X}}} C{{{:X}}}", c4.repr_hash(), c5.repr_hash())
        } else {
            Self::get_empty_committed_state_fift()
        }
    }

    pub fn try_commit(&mut self) -> Status {
        let c4 = self.get_root().as_cell()?.clone();
        let c5 = self.get_actions().as_cell()?.clone();
        if c4.repr_depth() > self.max_data_depth {
            fail!(
                ExceptionCode::CellOverflow,
                "Root cell repr depth is too big {} > {}",
                c4.repr_depth(),
                self.max_data_depth
            )
        }
        if c4.level() != 0 {
            fail!(ExceptionCode::CellOverflow, "Root cell level is not 0")
        }
        if c5.repr_depth() > self.max_data_depth {
            fail!(
                ExceptionCode::CellOverflow,
                "Actions cell repr depth is too big {} > {}",
                c5.repr_depth(),
                self.max_data_depth
            )
        }
        if c5.level() != 0 {
            fail!(ExceptionCode::CellOverflow, "Actions cell level is not 0")
        }
        self.cstate = Some((c4, c5));
        Ok(())
    }

    pub fn steps(&self) -> u32 {
        self.step
    }

    fn is_trace_enabled(&self) -> bool {
        self.trace_callback.is_some()
    }

    fn trace_info(&self, info_type: EngineTraceInfoType, gas: i64, log_string: Option<String>) {
        if let Some(trace_callback) = self.trace_callback.as_ref() {
            // bigint param has been withdrawn during execution, so take it from the stack
            let cmd_str = if let Some(mut big) = self.cmd.biginteger_raw() {
                if !self.cc.stack.is_empty() {
                    if let Ok(_big) = self.cc.stack.get(0).and_then(StackItem::as_integer) {
                        big = _big;
                    }
                }
                format!(
                    "{}{} {big}",
                    self.cmd.proto.name_prefix.unwrap_or_default(),
                    self.cmd.proto.name,
                )
            } else {
                log_string.or_else(|| self.cmd.dump_with_params()).unwrap_or_default()
            };
            let info = EngineTraceInfo {
                info_type,
                step: self.step,
                cmd_str,
                cmd_code: self.cmd_code(0).unwrap_or_default(),
                stack: &self.cc.stack,
                gas_used: self.gas_used(),
                gas_cmd: self.gas_used() - gas,
            };
            trace_callback(self, &info);
        }
    }

    fn default_trace_callback(&self, info: &EngineTraceInfo) {
        if self.trace_bit(Engine::TRACE_CODE) && info.has_cmd() {
            log::trace!(
                target: "tvm",
                "{}: {}\n{}\n",
                info.step,
                info.cmd_str,
                self.cmd_code_string()
            );
        }
        if self.trace_bit(Engine::TRACE_GAS) {
            log::trace!(
                target: "tvm",
                "Gas: {} ({})\n",
                info.gas_used,
                info.gas_cmd
            );
        }
        if self.trace_bit(Engine::TRACE_STACK) {
            log::trace!(target: "tvm", "{}", self.dump_stack("Stack trace", false));
        }
        if self.trace_bit(Engine::TRACE_CTRLS) {
            log::trace!(target: "tvm", "{}", self.dump_ctrls(true));
        }
        if info.info_type == EngineTraceInfoType::Dump {
            log::info!(target: "tvm", "{}", info.cmd_str);
        }
    }

    #[allow(dead_code)]
    fn fift_trace_callback(&self, info: &EngineTraceInfo) {
        if info.info_type == EngineTraceInfoType::Dump {
            log::info!(target: "tvm", "{}", info.cmd_str);
        } else if info.info_type == EngineTraceInfoType::Exception {
            if self.trace_bit(Engine::TRACE_CODE) {
                log::info!(target: "tvm", "BAD_CODE: {} {:?}\n", info.cmd_str, self.cmd_code_string());
            }
            if self.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "tvm", " [ {} ] \n", self.get_stack_result_fift());
            }
            if self.trace_bit(Engine::TRACE_CTRLS) {
                log::trace!(target: "tvm", "{}", self.dump_ctrls(true));
            }
            if self.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "tvm", "gas - {}\n", info.gas_used);
            }
        } else if info.has_cmd() {
            if self.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "tvm", "gas remaining: {}\n", self.gas_remaining() + info.gas_cmd);
            }
            if self.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "tvm", "stack: [ {} ]\n", self.get_stack_result_fift());
            }
            if self.trace_bit(Engine::TRACE_CODE) {
                log::info!(target: "tvm", "code cell hash: {:X} offset: {}\n", info.cmd_code.cell().unwrap().repr_hash(), info.cmd_code.pos());
                let cmd_str = match info.cmd_str.as_str() {
                    "POP s0" => "DROP",
                    "POP s1" => "NIP",
                    "PUSH s0" => "DUP",
                    "PUSH s1" => "OVER",
                    "SWAP s0,s1" => "SWAP",
                    "XCHG s0,s1" => "XCHG s1",
                    "XCHG s0,s2" => "XCHG s2",
                    "XCHG s0,s3" => "XCHG s3",
                    cmd_str => cmd_str,
                };
                log::info!(target: "tvm", "execute {}\n", cmd_str);
            }
            if self.trace_bit(Engine::TRACE_CTRLS) {
                log::trace!(target: "tvm", "{}", self.dump_ctrls(true));
            }
        }
    }

    pub fn emulator_trace_callback(&self, info: &EngineTraceInfo) {
        if info.has_cmd() {
            if self.trace_bit(Engine::TRACE_CODE) {
                log::info!(target: "executor", "code cell hash: {:X} offset: {}\n",
                    info.cmd_code.cell().unwrap().repr_hash(), info.cmd_code.pos());
                log::info!(target: "executor", "{}\n", info.cmd_str);
            }
            if self.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "executor", " [ {} ] \n", self.get_stack_result_fift());
            }
            if self.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "executor", "gas - {}\n", info.gas_used);
            }
            // log::info!(target: "executor", "code cell hash: {:X} offset: {}\n",
            //     info.cmd_code.cell().unwrap().repr_hash(), info.cmd_code.pos());
            // log::info!(target: "executor", "{}\n", info.cmd_str);
            // log::info!(target: "executor", " [ {} ] \n", self.get_stack_result_fift());
            // log::info!(target: "executor", "gas - {}\n", info.gas_used);
        }
    }

    #[allow(dead_code)]
    fn dump_stack_result(stack: &Stack) -> String {
        static PREV_STACK: LazyLock<Mutex<Stack>> = LazyLock::new(|| Mutex::new(Stack::new()));
        let mut prev_stack = PREV_STACK.lock().unwrap();
        let mut result = String::new();
        let mut iter = prev_stack.iter();
        let mut same = false;
        for item in stack.iter() {
            if let Some(prev) = iter.next() {
                if prev == item {
                    same = true;
                    continue;
                }
                while iter.next().is_some() {}
            }
            if same {
                same = false;
                result = "--\"-- ".to_string();
            }
            let string = match item {
                StackItem::None => "N".to_string(),
                StackItem::Integer(data) => match data.bitsize() {
                    Ok(0..=230) => data.to_string(),
                    Ok(bitsize) => format!("I{}", bitsize),
                    Err(err) => err.to_string(),
                },
                StackItem::Cell(data) => {
                    format!("C{}-{}", data.bit_length(), data.references_count())
                }
                StackItem::Continuation(data) => format!("T{}", data.code().remaining_bits() / 8),
                StackItem::Builder(data) => {
                    format!("B{}-{}", data.length_in_bits(), data.references().len())
                }
                StackItem::Slice(data) => {
                    format!("S{}-{}", data.remaining_bits(), data.remaining_references())
                }
                StackItem::Tuple(data) => match data.len() {
                    0 => "[]".to_string(),
                    len => format!("[@{}]", len),
                },
            };
            result += &string;
            result += " ";
        }
        *prev_stack = stack.clone();
        result
    }

    #[allow(dead_code)]
    pub fn simple_trace_callback(enine: &Engine, info: &EngineTraceInfo) {
        if info.info_type == EngineTraceInfoType::Dump {
            log::info!(target: "tvm", "{}", info.cmd_str);
        } else if info.info_type == EngineTraceInfoType::Start {
            if enine.trace_bit(Engine::TRACE_CTRLS) {
                log::trace!(target: "tvm", "{}", enine.dump_ctrls(true));
            }
            if enine.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "tvm", " [ {} ] \n", Self::dump_stack_result(info.stack));
            }
            if enine.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "tvm", "gas - {}\n", info.gas_used);
            }
        } else if info.info_type == EngineTraceInfoType::Exception {
            if enine.trace_bit(Engine::TRACE_CODE) {
                log::info!(target: "tvm", "{} ({}) BAD_CODE: {}\n", info.step, info.gas_cmd, info.cmd_str);
            }
            if enine.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "tvm", " [ {} ] \n", Self::dump_stack_result(info.stack));
            }
            if enine.trace_bit(Engine::TRACE_CTRLS) {
                log::trace!(target: "tvm", "{}", enine.dump_ctrls(true));
            }
            if enine.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "tvm", "gas - {}\n", info.gas_used);
            }
        } else if info.has_cmd() {
            if enine.trace_bit(Engine::TRACE_CODE) {
                log::info!(target: "tvm", "{}\n", info.cmd_str);
            }
            if enine.trace_bit(Engine::TRACE_STACK) {
                log::info!(target: "tvm", " [ {} ] \n", Self::dump_stack_result(info.stack));
            }
            if enine.trace_bit(Engine::TRACE_CTRLS) {
                log::trace!(target: "tvm", "{}", enine.dump_ctrls(true));
            }
            if enine.trace_bit(Engine::TRACE_GAS) {
                log::info!(target: "tvm", "gas - {}\n", info.gas_used);
            }
        }
    }

    pub fn execute(&mut self) -> Result<i32> {
        self.trace_info(EngineTraceInfoType::Start, 0, None);
        let result = loop {
            if let Some(result) = self.seek_next_cmd()? {
                break result;
            }
            self.cmd_code = SliceProto::from(self.cc.code());
            let execution_result = match HANDLERS_CP0.get_handler(self) {
                Err(err) => match self.basic_use_gas(8) {
                    Err(err) => Some(err),
                    Ok(_) => Some(err),
                },
                Ok(Some(handler)) => {
                    match handler(self) {
                        Err(e) => {
                            // Some(update_error_description(e, |e|
                            //     format!("CMD: {}{} err: {}", self.cmd.proto.name_prefix.unwrap_or_default(), self.cmd.proto.name, e)
                            // ))
                            Some(e)
                        }
                        Ok(_) => self.gas.check_gas_remaining().err(),
                    }
                }
                Ok(None) => {
                    let code = self.last_cmd();
                    log::trace!(target: "tvm", "Invalid code: {} ({:#X})\n", code, code);
                    if let Err(err) = self.try_use_gas(Gas::basic_gas_price(0, 0)) {
                        Some(err)
                    } else {
                        Some(error!(ExceptionCode::InvalidOpcode))
                    }
                }
            };
            self.cmd.clear();
            if let Some(err) = execution_result {
                self.raise_exception(err)?;
            }
        };
        self.trace_info(
            EngineTraceInfoType::Finish,
            self.gas_used(),
            Some("NORMAL TERMINATION".to_string()),
        );
        self.try_commit()?;
        Ok(result)
    }

    fn step_next_ref(&mut self, reference: Cell) -> Result<Option<i32>> {
        self.step += 1;
        self.log_string = Some("implicit JMPREF");
        self.try_use_gas(Gas::implicit_jmp_price())?;
        let code = self.load_hashed_cell(reference, true)?;
        *self.cc.code_mut() = code;
        Ok(None)
    }
    fn step_ordinary(&mut self) -> Result<Option<i32>> {
        self.step += 1;
        self.log_string = Some("implicit RET");
        self.try_use_gas(Gas::implicit_ret_price())?;
        if self.ctrls.get(0).is_none() {
            return Ok(Some(0));
        }
        switch_to_c0(self)?;
        Ok(None)
    }
    fn step_pushint(&mut self, code: i32) -> Result<Option<i32>> {
        self.step += 1;
        self.log_string = Some("implicit PUSHINT");
        self.cc.stack.push_int(code);
        switch(self, ctrl!(0))?;
        Ok(None)
    }
    fn step_try_catch(&mut self) -> Result<Option<i32>> {
        self.step += 1;
        self.log_string = Some("IMPLICIT RET FROM TRY-CATCH");
        self.try_use_gas(Gas::implicit_ret_price())?;
        self.ctrls.remove(2);
        switch(self, ctrl!(0))?;
        Ok(None)
    }
    fn step_while_loop(&mut self, body: SliceData, cond: SliceData) -> Result<Option<i32>> {
        match self.check_while_loop_condition() {
            Ok(true) => {
                self.log_string = Some("NEXT WHILE ITERATION");
                self.discharge_nargs();
                let mut cond = ContinuationData::with_code(cond);
                let mut while_ = ContinuationData::move_without_stack(&mut self.cc, body);
                while_.savelist.put_opt(0, self.ctrl_mut(0)?.withdraw());
                cond.savelist.put_opt(0, StackItem::continuation(while_));
                self.ctrls.put_opt(0, StackItem::continuation(cond));
            }
            Ok(false) => {
                self.log_string = Some("RET FROM WHILE");
                switch(self, ctrl!(0))?;
            }
            Err(err) => {
                let quit = ContinuationType::Quit(ExceptionCode::NormalTermination as i32);
                self.ctrls.put(0, StackItem::continuation(ContinuationData::with_type(quit)))?;
                return Err(err);
            }
        }
        Ok(None)
    }
    fn step_repeat_loop(&mut self, body: SliceData) -> Result<Option<i32>> {
        if let ContinuationType::RepeatLoopBody(_, ref mut counter) = self.cc.type_of {
            if *counter > 1 {
                *counter -= 1;
                self.log_string = Some("NEXT REPEAT ITERATION");
                self.discharge_nargs();
                let mut repeat = ContinuationData::move_without_stack(&mut self.cc, body);
                repeat.savelist.put_opt(0, self.ctrl_mut(0)?.withdraw());
                self.ctrls.put_opt(0, StackItem::continuation(repeat));
            } else {
                self.log_string = Some("RET FROM REPEAT");
                switch(self, ctrl!(0))?;
            }
        }
        Ok(None)
    }
    fn step_until_loop(&mut self, body: SliceData) -> Result<Option<i32>> {
        match self.check_until_loop_condition() {
            Ok(true) => {
                self.log_string = Some("NEXT UNTIL ITERATION");
                self.discharge_nargs();
                let mut until = ContinuationData::move_without_stack(&mut self.cc, body);
                until.savelist.put_opt(0, self.ctrl_mut(0)?.withdraw());
                self.ctrls.put_opt(0, StackItem::continuation(until));
            }
            Ok(false) => {
                self.log_string = Some("RET FROM UNTIL");
                switch(self, ctrl!(0))?;
            }
            Err(err) => return Err(err),
        }
        Ok(None)
    }
    fn step_again_loop(&mut self, body: SliceData) -> Result<Option<i32>> {
        self.log_string = Some("NEXT AGAIN ITERATION");
        self.discharge_nargs();
        let again = ContinuationData::move_without_stack(&mut self.cc, body);
        self.ctrls.put_opt(0, StackItem::continuation(again));
        Ok(None)
    }

    fn discharge_nargs(&mut self) {
        if self.cc.nargs != -1 {
            let depth = self.cc.stack.depth();
            let _ = self.cc.stack.drop_range_straight((depth - self.cc.nargs as usize)..depth);
            self.cc.nargs = -1;
        }
    }

    fn make_external_error(&mut self) -> Result<Option<i32>> {
        let number = self.cc.stack.drop(0)?.as_integer_value(0..=0xffffi32)?;
        if number == ExceptionCode::NormalTermination as i32
            || number == ExceptionCode::AlternativeTermination as i32
        {
            return Ok(Some(number));
        }
        let value = match self.cc.stack.drop(0) {
            Ok(item) => item.as_integer().cloned().unwrap_or_default(),
            Err(_) => IntegerData::zero(),
        };
        let exception = match ExceptionCode::from_i32(number) {
            Some(code) => Exception::from_code(code, String::new(), file!(), line!()),
            None => Exception::from_number(number, String::new(), file!(), line!()),
        };
        fail!(TvmError::new(exception, StackItem::int(value)))
    }

    // return Ok(Some(exit_code)) - if you want to stop execution
    pub(in crate::executor) fn seek_next_cmd(&mut self) -> Result<Option<i32>> {
        while self.cc.code().remaining_bits() == 0 {
            let gas = self.gas_used();
            self.log_string = None;
            let result = if let Some(reference) = self.cc.code().reference_opt(0) {
                self.step_next_ref(reference)
            } else {
                match self.cc.type_of.clone() {
                    ContinuationType::Ordinary => self.step_ordinary(),
                    ContinuationType::PushInt(code) => self.step_pushint(code),
                    ContinuationType::Quit(exit_code) => Ok(Some(exit_code)),
                    ContinuationType::TryCatch => self.step_try_catch(),
                    ContinuationType::WhileLoopCondition(body, cond) => {
                        self.step_while_loop(body, cond)
                    }
                    ContinuationType::RepeatLoopBody(code, _counter) => self.step_repeat_loop(code),
                    ContinuationType::UntilLoopCondition(body) => self.step_until_loop(body),
                    ContinuationType::AgainLoopBody(slice) => self.step_again_loop(slice),
                    ContinuationType::ExcQuit => Ok(self.make_external_error()?),
                }
            };
            if self.is_trace_enabled() {
                if let Some(log_string) = self.log_string {
                    self.trace_info(
                        EngineTraceInfoType::Implicit,
                        gas,
                        Some(log_string.to_string()),
                    );
                }
            }
            match self.gas.check_gas_remaining().and(result) {
                Ok(None) => (),
                Ok(Some(exit_code)) => return Ok(Some(exit_code)),
                Err(err) => match self.raise_exception(err) {
                    Ok(Some(exit_code)) => return Ok(Some(exit_code)),
                    Ok(None) => (),
                    Err(err) => return Err(err),
                },
            }
        }
        Ok(None)
    }

    fn resolve_init_code_cell(&self, code: Cell) -> Result<SliceData> {
        let mut slice = SliceData::load_cell(code.clone())?;
        if code.cell_type() == CellType::Ordinary {
            return Ok(slice);
        } else if code.cell_type() == CellType::LibraryReference {
            slice.move_by(8)?;
            if let Ok(cell) = self.load_library_cell(slice) {
                if cell.cell_type() == CellType::Ordinary {
                    return SliceData::load_cell(cell);
                } else {
                    log::warn!(
                        target: "tvm",
                        "Library cell must be ordinary, but is {}",
                        cell.cell_type()
                    );
                }
            }
        } else {
            fail!(
                ExceptionCode::CellUnderflow,
                "Code cell must be ordinary or library cell, but is {}",
                code.cell_type()
            );
        }
        let mut builder = BuilderData::new();
        builder.checked_append_reference(code)?;
        SliceData::load_builder(builder)
    }

    fn load_library_cell(&self, hash: SliceData) -> Result<Cell> {
        for library in &self.libraries {
            if let Some(lib_bucket) = library.get(hash.clone())? {
                let lib = lib_bucket.reference(0)?;
                if !hash.contains_bytes(lib.repr_hash().as_slice()) {
                    fail!(
                        ExceptionCode::DictionaryError,
                        "Librariy hash does not correspond to map key {:x}",
                        hash
                    )
                }
                return Ok(lib);
            }
        }
        fail!(ExceptionCode::CellUnderflow, "Libraries do not contain code with hash {:x}", hash)
    }

    /// Loads cell to slice checking in precashed map
    pub fn load_hashed_cell(&mut self, mut cell: Cell, resolve_special: bool) -> Result<SliceData> {
        let mut library_loaded = false;
        loop {
            if !library_loaded {
                let hash = cell.repr_hash();
                let first = self.visited_cells.insert(hash.clone());
                self.try_use_gas(Gas::load_cell_price(first))?;
            }
            let mut slice = SliceData::load_cell(cell)?;
            if !resolve_special || slice.cell_type() == CellType::Ordinary {
                return Ok(slice);
            }
            match slice.cell_type() {
                CellType::LibraryReference => {
                    if library_loaded {
                        fail!(ExceptionCode::CellUnderflow, "Library cell already loaded");
                    }
                    slice.move_by(8)?;
                    cell = self.load_library_cell(slice).map_err(|err| {
                        error!(
                            ExceptionCode::CellUnderflow,
                            "Failed to load library cell: {:?}", err
                        )
                    })?;
                    library_loaded = true;
                    continue;
                }
                CellType::PrunedBranch => {
                    let virtualization = slice.virtualization();
                    if virtualization != 0 {
                        fail!(
                            ExceptionCode::CellUnderflow,
                            "Pruned branch cell virtualization must be 0, but is {}",
                            virtualization
                        );
                    }
                    fail!(ExceptionCode::CellUnderflow, "pruned branch cell cannot be loaded")
                }
                CellType::MerkleProof => {
                    fail!(ExceptionCode::CellUnderflow, "merkle proof cell cannot be loaded")
                }
                CellType::MerkleUpdate => {
                    fail!(ExceptionCode::CellUnderflow, "merkle update cell cannot be loaded")
                }
                cell_type => {
                    fail!(ExceptionCode::CellUnderflow, "Wrong resolving cell type {}", cell_type)
                }
            }
        }
    }

    pub fn is_committed_state(&self) -> bool {
        self.cstate.is_some()
    }

    pub fn get_committed_state(self) -> Option<(Cell, Cell)> {
        self.cstate
    }

    pub fn get_actions(&self) -> StackItem {
        match self.ctrls.get(5) {
            Some(x) => x.clone(),
            None => StackItem::None,
        }
    }

    fn get_root(&self) -> StackItem {
        match self.ctrls.get(4) {
            Some(x) => x.clone(),
            None => StackItem::None,
        }
    }

    pub fn ctrl(&self, index: usize) -> Result<&StackItem> {
        self.ctrls
            .get(index)
            .ok_or_else(|| error!(ExceptionCode::RangeCheckError, "get ctrl {} failed", index))
    }

    pub fn ctrl_mut(&mut self, index: usize) -> Result<&mut StackItem> {
        self.ctrls
            .get_mut(index)
            .ok_or_else(|| error!(ExceptionCode::RangeCheckError, "get ctrl {} failed", index))
    }

    pub fn ctrls(&self) -> &SaveList {
        &self.ctrls
    }

    pub fn cc(&self) -> &ContinuationData {
        &self.cc
    }

    fn dump_msg(message: &'static str, data: String) -> String {
        format!("--- {} {:-<4$}\n{}\n{:-<40}\n", message, "", data, "", 35 - message.len())
    }

    pub fn dump_ctrls(&self, short: bool) -> String {
        Self::dump_msg(
            "Control registers",
            SaveList::REGS
                .iter()
                .filter_map(|i| {
                    self.ctrls.get(*i).map(|item| {
                        if !short {
                            format!("{}: {}", i, item)
                        } else if *i == 3 {
                            "3: copy of CC".to_string()
                        } else if *i == 7 {
                            "7: SmartContractInfo".to_string()
                        } else if let StackItem::Continuation(x) = item {
                            format!("{}: {:?}", i, x.type_of)
                        } else {
                            format!("{}: {}", i, item.dump_as_fift())
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    pub fn dump_stack(&self, message: &'static str, short: bool) -> String {
        Self::dump_msg(
            message,
            self.cc
                .stack
                .iter()
                .map(|item| if !short { item.to_string() } else { item.dump_as_fift() })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    pub fn set_trace(&mut self, trace_mask: u8) {
        self.trace = trace_mask
    }

    pub fn set_trace_callback(
        &mut self,
        callback: impl Fn(&Engine, &EngineTraceInfo) + Send + Sync + 'static,
    ) {
        self.trace_callback = Some(Arc::new(callback));
    }

    pub fn set_arc_trace_callback(&mut self, callback: Arc<TraceCallback>) {
        self.trace_callback = Some(callback);
    }

    pub fn trace_bit(&self, trace_mask: u8) -> bool {
        (self.trace & trace_mask) == trace_mask
    }

    pub fn behavior_modifiers(&self) -> &BehaviorModifiers {
        &self.modifiers
    }

    pub fn modify_behavior(&mut self, modifiers: BehaviorModifiers) {
        self.modifiers = modifiers;
    }

    pub fn setup(
        self,
        code: Cell,
        ctrls: Option<SaveList>,
        stack: Option<Stack>,
        gas: Option<Gas>,
        libraries: Vec<HashmapE>,
    ) -> Result<Self> {
        self.setup_checked(
            code,
            ctrls.unwrap_or_default(),
            stack.unwrap_or_default(),
            gas.unwrap_or_else(Gas::test),
            libraries,
        )
    }

    pub fn setup_checked(
        mut self,
        code: Cell,
        mut ctrls: SaveList,
        stack: Stack,
        gas: Gas,
        libraries: Vec<HashmapE>,
    ) -> Result<Self> {
        self.libraries = libraries;
        let slice = self.resolve_init_code_cell(code)?;
        *self.cc.code_mut() = slice.clone();
        self.cmd_code = SliceProto::from(&slice);
        self.cc.stack = stack;
        self.gas = gas;
        let cont = ContinuationType::Quit(ExceptionCode::NormalTermination as i32);
        self.ctrls.put(0, StackItem::continuation(ContinuationData::with_type(cont)))?;
        let cont = ContinuationType::Quit(ExceptionCode::AlternativeTermination as i32);
        self.ctrls.put(1, StackItem::continuation(ContinuationData::with_type(cont)))?;
        let cont = ContinuationData::with_type(ContinuationType::ExcQuit);
        self.ctrls.put(2, StackItem::continuation(cont))?;
        let cont = ContinuationData::with_code(slice);
        self.ctrls.put(3, StackItem::continuation(cont))?;
        self.ctrls.put(4, StackItem::cell(Cell::default()))?;
        self.ctrls.put(5, StackItem::cell(Cell::default()))?;
        self.ctrls.put(7, SmartContractInfo::default().as_temp_data_item())?;
        self.ctrls.apply(&mut ctrls);
        Ok(self)
    }

    pub(crate) fn run_child_vm(&mut self, params: RunChildVm) -> Status {
        let (visited_cells, checked_signatures_count, free_gas_consumed) = if params.isolate_gas {
            (HashSet::new(), 0, 0)
        } else {
            (
                std::mem::take(&mut self.visited_cells),
                self.checked_signatures_count,
                self.free_gas_consumed,
            )
        };
        let mut ctrls = SaveList::new();
        let cont = ContinuationType::Quit(ExceptionCode::NormalTermination as i32);
        ctrls.put(0, StackItem::continuation(ContinuationData::with_type(cont)))?;
        let cont = ContinuationType::Quit(ExceptionCode::AlternativeTermination as i32);
        ctrls.put(1, StackItem::continuation(ContinuationData::with_type(cont)))?;
        let cont = ContinuationData::with_type(ContinuationType::ExcQuit);
        ctrls.put(2, StackItem::continuation(cont))?;
        let cont = if params.same_c3 {
            ContinuationData::with_code(params.code.clone())
        } else {
            let cont = ContinuationType::Quit(11);
            ContinuationData::with_type(cont)
        };
        ctrls.put(3, StackItem::continuation(cont))?;
        ctrls.put(4, params.data)?;
        ctrls.put(5, StackItem::cell(Cell::default()))?;
        ctrls.put(7, params.c7)?;
        let gas_max = params.gas_max.min(self.gas.get_gas_remaining());
        let gas_limit = params.gas_limit.min(gas_max);
        let gas_price = self.gas.get_gas_price();
        let gas = Gas::new(gas_limit, 0, gas_max, gas_price);
        let cmd_code = SliceProto::from(&params.code);
        let mut child = Engine {
            cc: ContinuationData::with_code_and_stack(params.code, params.stack),
            cmd: InstructionExt::default(),
            ctrls,
            libraries: self.libraries.clone(),
            modifiers: self.modifiers.clone(),
            checked_signatures_count,
            visited_cells,
            cstate: None,
            time: self.time,
            gas,
            free_gas_consumed,
            code_page: 0,
            debug_on: self.debug_on,
            step: 0,
            debug_buffer: String::new(),
            cmd_code,
            last_cmd: 0,
            trace: self.trace,
            trace_callback: self.trace_callback.clone(),
            log_string: None,
            max_data_depth: self.max_data_depth,
            capabilities: self.capabilities,
            block_version: self.block_version,
        };
        let mut result = match child.execute() {
            Ok(result) => result,
            Err(err) => {
                // in case of error we have copy of code on stack
                match child.cc.stack.pop().and_then(|x| x.as_integer_value(0..=0xFFF)) {
                    Ok(code) => {
                        debug_assert_eq!(code, tvm_exception_or_custom_code(&err));
                    }
                    Err(err) => {
                        log::error!(
                            target: "tvm",
                            "Failed to pop error code from stack {err:?}",
                        );
                    }
                }
                tvm_exception_or_custom_code(&err)
            }
        };
        // let mut result = child.execute().unwrap_or_else(|err| tvm_exception_or_custom_code(&err));
        log::debug!(
            target: "tvm",
            "Child VM finished. res: {result}, steps: {}, gas: {}, stack depth: {}\n",
            child.step, child.gas_used(), child.cc.stack.depth()
        );

        self.step += child.step;
        self.use_gas(child.gas_used().min(child.gas.get_gas_limit() + 1)); // +1?
        let ret_vals = if result == 0 || result == 1 {
            if params.ret_vals >= 0 {
                if params.ret_vals as usize <= child.cc.stack.depth() {
                    params.ret_vals as usize
                } else {
                    result = !(ExceptionCode::StackUnderflow as i32);
                    self.cc.stack.push_int(0);
                    0
                }
            } else {
                child.cc.stack.depth()
            }
        } else {
            child.cc.stack.depth().min(1)
        };
        self.use_gas(Gas::stack_price(ret_vals));

        child.cmd.withdraw_vars();
        if fetch_stack(&mut child, ret_vals).is_err() {
            log::error!(
                target: "tvm",
                "Failed to fetch stack {ret_vals} from child VM"
            );
        }
        let stack = child.cmd.withdraw_vars();
        for var in stack.into_iter().rev() {
            self.cc.stack.push(var);
        }
        self.cc.stack.push_int(result);

        if !params.isolate_gas {
            self.visited_cells = child.visited_cells;
        }
        debug_assert!(self.block_version >= 11 || (!params.return_data && !params.return_actions));
        let (data, actions) = child.cstate.unwrap_or_default();
        if params.return_data {
            self.cc.stack.push_cell(data);
        }
        if params.return_actions {
            self.cc.stack.push_cell(actions);
        }
        if params.return_gas {
            self.cc.stack.push_int(child.gas.get_gas_used());
        }
        Ok(())
    }

    // Internal API ***********************************************************

    #[allow(dead_code)]
    pub(in crate::executor) fn local_time(&mut self) -> u64 {
        self.time += 1;
        self.time
    }

    // Implementation *********************************************************

    pub(in crate::executor) fn load_instruction(&mut self, proto: Instruction) -> Status {
        self.cmd.proto = proto;
        self.cmd.params.clear();
        self.cmd.vars.clear();
        self.step += 1;
        self.extract_instruction()
    }

    pub(in crate::executor) fn switch_debug(&mut self, on_off: bool) {
        self.debug_on += if on_off { 1 } else { -1 }
    }

    pub(in crate::executor) fn debug(&self) -> bool {
        self.debug_on > 0 && self.is_trace_enabled()
    }

    pub(in crate::executor) fn dump(&mut self, dump: &str) {
        self.debug_buffer += dump;
    }

    pub(in crate::executor) fn flush(&mut self) {
        if self.debug_on > 0 {
            let buffer = std::mem::take(&mut self.debug_buffer);
            if self.trace_callback.is_none() {
                log::info!(target: "tvm", "{}", buffer);
            } else {
                self.trace_info(EngineTraceInfoType::Dump, 0, Some(buffer));
            }
        } else {
            self.debug_buffer = String::new()
        }
    }

    ///Get gas state
    pub fn get_gas(&self) -> &Gas {
        &self.gas
    }
    ///Set gas state
    pub fn set_gas(&mut self, gas: Gas) {
        self.gas = gas
    }
    ///Interface to gas state set_gas_limit method
    pub fn new_gas_limit(&mut self, gas: i64) {
        self.gas.new_gas_limit(gas)
    }

    fn check_while_loop_condition(&mut self) -> Result<bool> {
        let x = self.cc.stack.drop(0)?;
        let y = x.as_integer()?;
        Ok(!y.is_zero())
    }

    fn check_until_loop_condition(&mut self) -> Result<bool> {
        Ok(!self.check_while_loop_condition()?)
    }

    fn extract_slice(
        &mut self,
        offset: usize,
        r: usize,
        x: usize,
        mut refs: usize,
        mut bytes: usize,
    ) -> Result<SliceData> {
        let mut code = self.cmd_code(offset)?;
        let mut slice = code.clone();
        if r != 0 {
            refs += slice.get_next_int(r)? as usize;
        }
        if x != 0 {
            bytes += slice.get_next_int(x)? as usize;
        }
        let shift = (8 * bytes + offset + r + x + 7) & !7; // round to 8 bits
        let bits = shift - r - x - offset;
        if slice.remaining_bits() < bits || slice.remaining_references() < refs {
            fail!(ExceptionCode::InvalidOpcode)
        }
        code.shrink_data(shift - offset..);
        code.shrink_references(refs..)?;
        *self.cc.code_mut() = code;

        slice.shrink_data(..bits);
        slice.shrink_references(..refs)?;

        Ok(slice)
    }

    fn basic_use_gas(&mut self, mut bits: usize) -> Result<()> {
        bits += self.cc.code().pos().saturating_sub(self.cmd_code.pos());
        self.try_use_gas(Gas::basic_gas_price(bits, 0))
    }

    fn extract_instruction(&mut self) -> Status {
        let gas = self.gas_used();
        match self.cmd.proto.opts {
            Some(InstructionOptions::ArgumentConstraints) => {
                let param = self.next_cmd()?;
                self.basic_use_gas(0)?;
                self.cmd.params.push(InstructionParameter::Pargs(((param >> 4) & 0x0F) as usize));
                self.cmd.params.push(InstructionParameter::Nargs(if (param & 0x0F) == 15 {
                    -1
                } else {
                    (param & 0x0F) as isize
                }))
            }
            Some(InstructionOptions::ArgumentAndReturnConstraints) => {
                let param = self.next_cmd()?;
                self.basic_use_gas(0)?;
                self.cmd.params.push(InstructionParameter::Pargs(((param >> 4) & 0x0F) as usize));
                self.cmd.params.push(InstructionParameter::Rargs((param & 0x0F) as usize))
            }
            Some(InstructionOptions::BigInteger) => {
                self.basic_use_gas(5)?;

                let bigint = IntegerData::from_big_endian_octet_stream(|| self.next_cmd())?;
                self.cmd.params.push(InstructionParameter::BigInteger(bigint))
            }
            Some(InstructionOptions::ControlRegister) => {
                self.basic_use_gas(0)?;
                let creg = (self.last_cmd() & 0x0F) as usize;
                if !SaveList::REGS.contains(&creg) {
                    fail!(ExceptionCode::RangeCheckError)
                }
                self.cmd.params.push(InstructionParameter::ControlRegister(creg))
            }
            Some(InstructionOptions::DivisionMode) => {
                let mode = DivMode::with_flags(self.next_cmd()?);
                if mode.shift_parameter() {
                    let len = self.next_cmd()? as usize + 1;
                    self.cmd.params.push(InstructionParameter::Length(len))
                }
                self.basic_use_gas(0)?;
                self.cmd.proto.name = mode.command_name()?;
                self.cmd.params.push(InstructionParameter::DivisionMode(mode));
            }
            Some(InstructionOptions::Integer(ref range)) => {
                let number = if *range == (-32768..32768) {
                    self.basic_use_gas(16)?;
                    (((self.next_cmd()? as i16) << 8) | (self.next_cmd()? as i16)) as isize
                } else if *range == (-128..128) {
                    self.basic_use_gas(8)?;
                    (self.next_cmd()? as i8) as isize
                } else if *range == (-5..11) {
                    self.basic_use_gas(0)?;
                    match self.last_cmd() & 0x0F {
                        value @ 0..=10 => value as isize,
                        value => value as isize - 16,
                    }
                } else if *range == (0..32) {
                    self.basic_use_gas(0)?;
                    (self.last_cmd() & 0x1F) as isize
                } else if *range == (0..64) {
                    self.basic_use_gas(0)?;
                    (self.last_cmd() % 64) as isize
                } else if *range == (0..2048) {
                    self.basic_use_gas(8)?;
                    let hi = (self.last_cmd() as i16) & 0x07;
                    let lo = self.next_cmd()? as i16;
                    (hi * 256 + lo) as isize
                } else if *range == (0..16384) {
                    self.basic_use_gas(8)?;
                    let hi = (self.last_cmd() as i16) & 0x3F;
                    let lo = self.next_cmd()? as i16;
                    (hi * 256 + lo) as isize
                } else if *range == (0..256) {
                    self.basic_use_gas(8)?;
                    self.next_cmd()? as isize
                } else if *range == (0..15) {
                    self.basic_use_gas(0)?;
                    match self.last_cmd() & 0x0F {
                        15 => fail!(ExceptionCode::RangeCheckError),
                        value => value as isize,
                    }
                } else if *range == (1..15) {
                    self.basic_use_gas(0)?;
                    match self.last_cmd() & 0x0F {
                        0 | 15 => fail!(ExceptionCode::RangeCheckError),
                        value => value as isize,
                    }
                } else if *range == (-15..240) {
                    self.basic_use_gas(0)?;
                    match self.last_cmd() {
                        value @ 0..=240 => value as isize,
                        value @ 0xF1..=0xFF => value as isize - 256,
                    }
                } else if *range == (0..0x1000) {
                    self.basic_use_gas(8)?;
                    ((self.last_cmd() & 0xF) as isize) << 8 | self.next_cmd()? as isize
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                };
                self.cmd.params.push(InstructionParameter::Integer(number))
            }
            Some(InstructionOptions::Length(ref range)) => {
                if *range == (0..16) {
                    self.cmd
                        .params
                        .push(InstructionParameter::Length((self.last_cmd() & 0x0F) as usize))
                } else if *range == (0..4) {
                    let length = self.last_cmd() & 3;
                    self.cmd.params.push(InstructionParameter::Length(length as usize))
                } else if *range == (1..32) {
                    let length = self.last_cmd() & 0x1F;
                    self.cmd.params.push(InstructionParameter::Length(length as usize))
                } else if *range == (0..256) {
                    let length = self.next_cmd()?;
                    self.cmd.params.push(InstructionParameter::Length(length as usize))
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                }
                self.basic_use_gas(0)?;
            }
            Some(InstructionOptions::LengthAndIndex) => {
                self.basic_use_gas(0)?;
                // This is currently needed only for special-case BLKPUSH command and works the same way
                // as InstructionOptions::StackRegisterPair(WhereToGetParams::GetFromLastByte)
                let params = self.last_cmd();
                let (length, index) = (params >> 4, params & 0x0F);
                self.cmd.params.push(InstructionParameter::LengthAndIndex(LengthAndIndex {
                    length: length as usize,
                    index: index as usize,
                }))
            }
            Some(InstructionOptions::LengthMinusOne(ref range)) => {
                let len = if *range == (0..8) {
                    self.last_cmd() & 0x07
                } else if *range == (0..256) {
                    self.next_cmd()?
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                } as usize
                    + 1;
                self.cmd.params.push(InstructionParameter::Length(len));
                self.basic_use_gas(0)?;
            }
            Some(InstructionOptions::LengthMinusOneAndIndexMinusOne) => {
                let params = self.next_cmd()?;
                self.basic_use_gas(0)?;
                let (l_minus_1, i_minus_1) = (params >> 4, params & 0x0F);
                self.cmd.params.push(InstructionParameter::LengthAndIndex(LengthAndIndex {
                    length: (l_minus_1 + 1) as usize,
                    index: (i_minus_1 + 1) as usize,
                }))
            }
            Some(InstructionOptions::LengthMinusTwoAndIndex) => {
                let params = self.next_cmd()?;
                self.basic_use_gas(0)?;
                let (l_minus_2, i) = (params >> 4, params & 0x0F);
                self.cmd.params.push(InstructionParameter::LengthAndIndex(LengthAndIndex {
                    length: (l_minus_2 + 2) as usize,
                    index: i as usize,
                }))
            }
            Some(InstructionOptions::Pargs(ref range)) => {
                if *range == (0..16) {
                    self.cmd
                        .params
                        .push(InstructionParameter::Pargs((self.last_cmd() & 0x0F) as usize))
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                }
                self.basic_use_gas(0)?;
            }
            Some(InstructionOptions::Rargs(ref range)) => {
                if *range == (0..16) {
                    self.cmd
                        .params
                        .push(InstructionParameter::Rargs((self.last_cmd() & 0x0F) as usize))
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                }
                self.basic_use_gas(0)?;
            }
            Some(InstructionOptions::StackRegister(ref range)) => {
                if *range == (0..16) {
                    self.cmd.params.push(InstructionParameter::StackRegister(
                        (self.last_cmd() & 0x0F) as usize,
                    ))
                } else if *range == (0..256) {
                    let reg = self.next_cmd()? as usize;
                    self.cmd.params.push(InstructionParameter::StackRegister(reg))
                } else {
                    fail!(ExceptionCode::RangeCheckError)
                }
                self.basic_use_gas(0)?;
            }
            Some(InstructionOptions::StackRegisterPair(ref place)) => {
                let (ra, rb) = match place {
                    WhereToGetParams::GetFromLastByte2Bits => {
                        let opcode_ra_rb = self.last_cmd();
                        ((opcode_ra_rb >> 2) & 0x03, opcode_ra_rb & 0x03)
                    }
                    WhereToGetParams::GetFromLastByte => {
                        let opcode_ra_rb = self.last_cmd();
                        ((opcode_ra_rb & 0xF0) >> 4, opcode_ra_rb & 0x0F)
                    }
                    WhereToGetParams::GetFromNextByte => {
                        let ra_rb = self.next_cmd()?;
                        ((ra_rb & 0xF0) >> 4, ra_rb & 0x0F)
                    }
                    WhereToGetParams::GetFromNextByteLong => {
                        let rb = self.next_cmd()?;
                        (0, rb)
                    }
                    _ => (0, 0),
                };
                self.basic_use_gas(0)?;
                self.cmd.params.push(InstructionParameter::StackRegisterPair(RegisterPair {
                    ra: ra as usize,
                    rb: rb as usize,
                }))
            }
            Some(InstructionOptions::StackRegisterTrio(ref place)) => {
                let last = self.last_cmd();
                let (ra, rb, rc) = match place {
                    WhereToGetParams::GetFromLastByte2Bits => {
                        // INDEX3 2 bits per index
                        ((last >> 4) & 0x03, (last >> 2) & 0x03, last & 0x03)
                    }
                    _ => {
                        // Three-arguments functions are 2-byte 4ijk XCHG3 instructions
                        // And 54[0-7]ijk long-form XCHG3 - PUSH3
                        // We assume that in the second case 0x54 byte is already consumed,
                        // and we have to deal with *ijk layout for arguments
                        let rb_rc = self.next_cmd()?;
                        (last & 0x0F, rb_rc >> 4, rb_rc & 0x0F)
                    }
                };
                self.basic_use_gas(0)?;
                self.cmd.params.push(InstructionParameter::StackRegisterTrio(RegisterTrio {
                    ra: ra as usize,
                    rb: rb as usize,
                    rc: rc as usize,
                }))
            }
            Some(InstructionOptions::Dictionary(offset, bits)) => {
                self.use_gas(Gas::basic_gas_price(offset + 1 + bits, 0));
                let mut code = self.cmd_code(offset)?;
                let cell = code
                    .get_next_dictionary()
                    .map_err(|_| ExceptionCode::InvalidOpcode)?
                    .ok_or(ExceptionCode::InvalidOpcode)?;
                self.cmd.params.push(InstructionParameter::Cell(cell));
                let length = code.get_next_int(bits)? as usize;
                *self.cc.code_mut() = code;
                self.cmd.params.push(InstructionParameter::Length(length))
            }
            Some(InstructionOptions::Bytestring(offset, r, x, bytes)) => {
                self.use_gas(Gas::basic_gas_price(offset + r + x, 0));
                let slice = self.extract_slice(offset, r, x, 0, bytes)?;
                if slice.remaining_bits() % 8 != 0 {
                    fail!(ExceptionCode::InvalidOpcode)
                }
                self.cmd.params.push(InstructionParameter::Slice(slice))
            }
            Some(InstructionOptions::Bitstring(offset, r, x, refs)) => {
                self.use_gas(Gas::basic_gas_price(offset + r + x, 0));
                let mut slice = self.extract_slice(offset, r, x, refs, 0)?;
                slice.trim_right();
                self.cmd.params.push(InstructionParameter::Slice(slice));
            }
            None => {
                self.basic_use_gas(0)?;
            }
        }
        self.trace_info(EngineTraceInfoType::Normal, gas, None);
        Ok(())
    }

    // raises the exception and tries to dispatch it via c(2).
    // If c(2) is not set, returns that exception, otherwise, returns None
    fn raise_exception(&mut self, err: Error) -> Result<Option<i32>> {
        if let Some(code) = tvm_exception_code(&err) {
            self.step += 1;
            if code == ExceptionCode::OutOfGas {
                log::trace!(target: "tvm", "OUT OF GAS CODE: {}\n", self.cmd_code_string());
                return Err(err);
            }
        }
        if let Err(err) = self.gas.try_use_gas(Gas::exception_price()) {
            self.step += 1;
            return Err(err);
        }
        let n = self.cmd.vars.len();
        self.trace_info(
            EngineTraceInfoType::Exception,
            self.gas_used(),
            Some(format!("EXCEPTION: {:?}", err)),
        );
        let Some(c2) = self.ctrls.remove(2) else {
            return Err(err);
        };
        let (exception, value) = tvm_exception_full(err).inspect_err(
            |err| log::trace!(target: "tvm", "BAD CODE: {}: {}\n", self.cmd_code_string(), err),
        )?;
        if let Ok(c2) = c2.as_continuation() {
            if c2.type_of == ContinuationType::ExcQuit {
                if let Some(exit_code) = exception.is_normal_termination() {
                    let cont = ContinuationData::with_type(ContinuationType::Quit(exit_code));
                    self.cmd.push_var(StackItem::Continuation(Arc::new(cont)));
                    self.cc.stack.push(value);
                    self.cmd.vars[n].as_continuation_mut()?.nargs = 1;
                    switch(self, var!(n))?;
                    return Ok(None);
                } else {
                    self.cc.stack = Stack::new();
                    self.cc.stack.push(value);
                    self.cc.stack.push(int!(exception.exception_or_custom_code()));
                    fail!(exception)
                }
            }
        }
        self.cmd.push_var(c2);
        self.cc.stack.push(value);
        self.cc.stack.push(int!(exception.exception_or_custom_code()));
        self.cmd.vars[n].as_continuation_mut()?.nargs = 2;
        switch(self, var!(n))?;
        Ok(None)
    }

    pub(in crate::executor) fn last_cmd(&self) -> u8 {
        self.last_cmd
    }

    pub(in crate::executor) fn next_cmd(&mut self) -> Result<u8> {
        match self.cc.code_mut().get_next_byte() {
            Ok(cmd) => {
                self.last_cmd = cmd;
                Ok(cmd)
            }
            Err(_) => fail!(
                ExceptionCode::InvalidOpcode,
                "remaining bits expected >= 8, but actual value is: {}",
                self.cc.code().remaining_bits()
            ),
        }
    }

    fn cmd_code_string(&self) -> String {
        match self.cmd_code(0) {
            Ok(code) => code.to_string(),
            Err(err) => err.to_string(),
        }
    }
    fn cmd_code(&self, offset: usize) -> Result<SliceData> {
        let mut code = match self.cc.code().cell_opt() {
            Some(cell) => SliceData::load_cell_ref(cell)?,
            None => SliceData::load_cell(self.cc.code().clone().into_cell()?)?, // or error
        };
        let data = &self.cmd_code.data_window;
        let refs = self.cmd_code.references_window.clone();
        code.shrink(data.start + offset..data.end, refs);
        Ok(code)
    }

    /// Set code page for interpret bytecode. now only code page 0 is supported
    pub(in crate::executor) fn code_page_mut(&mut self) -> &mut isize {
        &mut self.code_page
    }

    /// get smartcontract info param from ctrl(7) tuple index 0
    pub(in crate::executor) fn smci_param(&self, index: usize) -> Result<&StackItem> {
        let smci = self.ctrl(7)?.tuple_item_ref(0)?;
        smci.tuple_item_ref(index)
    }

    /// get smartcontract info extra param from ctrl(7) tuple index 0 then tuple index 14
    pub(in crate::executor) fn smci_extra_param(
        &self,
        index: usize,
        extra: usize,
    ) -> Result<&StackItem> {
        self.smci_param(index)?.tuple_item_ref(extra)
    }

    pub(in crate::executor) fn rand(&self) -> Result<&IntegerData> {
        self.smci_param(6)?.as_integer()
    }

    pub(in crate::executor) fn set_rand(&mut self, rand: IntegerData) -> Status {
        let mut tuple = self.ctrl_mut(7)?.as_tuple_mut()?;
        let t1 = match tuple.first_mut() {
            Some(t1) => t1,
            None => fail!(
                ExceptionCode::RangeCheckError,
                "set tuple index is {} but length is {}",
                0,
                tuple.len()
            ),
        };
        let mut t1_items = t1.as_tuple_mut()?;
        match t1_items.get_mut(6) {
            Some(v) => *v = StackItem::int(rand),
            None => fail!(
                ExceptionCode::RangeCheckError,
                "set tuple index is {} but length is {}",
                6,
                t1_items.len()
            ),
        }
        self.use_gas(Gas::tuple_gas_price(t1_items.len()));
        *t1 = StackItem::tuple(t1_items);
        self.use_gas(Gas::tuple_gas_price(tuple.len()));
        *self.ctrl_mut(7)? = StackItem::tuple(tuple);
        Ok(())
    }

    pub(crate) fn get_config_param(&mut self, index: i32) -> Result<Option<Cell>> {
        if let StackItem::Cell(data) = self.smci_param(9)? {
            let params = HashmapE::with_hashmap(32, Some(data.clone()));
            let mut key = BuilderData::new();
            key.append_i32(index)?;
            if let Some(value) = params.get_with_gas(SliceData::load_builder(key)?, self)? {
                return Ok(value.reference_opt(0));
            }
        }
        Ok(None)
    }

    pub(crate) fn _read_config_param<T: Deserializable>(&mut self, index: i32) -> Result<T> {
        match self.get_config_param(index)? {
            Some(cell) => T::construct_from_cell(cell),
            None => fail!("Cannot get config param {}", index),
        }
    }
}
