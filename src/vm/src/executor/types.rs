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
    executor::math::DivMode,
    stack::{integer::IntegerData, StackItem},
};
use std::{fmt, ops::Range};
use ton_block::{error, Cell, Result, SliceData};

macro_rules! param {
    ($self:ident, $id:ident) => {{
        debug_assert!($self.params.len() < 3);
        for p in &$self.params {
            if let InstructionParameter::$id(x) = p {
                return Some(*x);
            }
        }
        None
    }};
}

macro_rules! param_ref {
    ($self:ident, $id:ident) => {{
        debug_assert!($self.params.len() < 3);
        for p in &$self.params {
            if let InstructionParameter::$id(ref x) = p {
                return Some(x);
            }
        }
        None
    }};
}

macro_rules! param_ref_mut {
    ($self:ident, $id:ident) => {{
        debug_assert!($self.params.len() < 3);
        for p in &mut $self.params {
            if let InstructionParameter::$id(ref mut x) = p {
                return Some(x);
            }
        }
        None
    }};
}

pub(super) enum InstructionOptions {
    // What will be set:
    ArgumentConstraints,                    // Nargs, Pargs
    ArgumentAndReturnConstraints,           // Pargs, Rargs
    BigInteger,                             // BigInteger
    Bytestring(usize, usize, usize, usize), // byte aligned SliceData from code
    ControlRegister,                        // ControlRegister
    Dictionary(usize, usize),               // SliceData with dictionary and Integer for index
    DivisionMode,                           // DivisionMode
    Integer(Range<isize>),                  // Integer
    Length(Range<usize>),                   // Length
    LengthAndIndex,                         // LengthAndIndex
    LengthMinusOne(Range<usize>),           // Length
    LengthMinusOneAndIndexMinusOne,         // LengthAndIndex
    LengthMinusTwoAndIndex,                 // LengthAndIndex
    Pargs(Range<usize>),                    // Pargs
    Rargs(Range<usize>),                    // Rargs
    Bitstring(usize, usize, usize, usize),  // SliceData from code
    StackRegister(Range<usize>),            // StackRegister
    StackRegisterPair(WhereToGetParams),    // StackRegisterPair
    StackRegisterTrio(WhereToGetParams),    // StackRegisterTrio
}

#[derive(Debug, PartialEq)]
pub(super) enum WhereToGetParams {
    GetFromLastByte2Bits,
    GetFromLastByte,
    GetFromNextByte,
    GetFromNextByteLong,
    GetFromNextByteMinusOne,
    GetFromNextByteMinusOneMinusOne,
    GetFromNextByteMinusOneMinusTwo,
}

#[derive(Debug)]
pub(super) enum InstructionParameter {
    BigInteger(IntegerData),
    ControlRegister(usize),
    DivisionMode(DivMode),
    Integer(isize),
    Length(usize),
    LengthAndIndex(LengthAndIndex),
    Nargs(isize),
    Pargs(usize),
    Rargs(usize),
    Slice(SliceData),
    Cell(Cell),
    StackRegister(usize),
    StackRegisterPair(RegisterPair),
    StackRegisterTrio(RegisterTrio),
}

#[derive(Debug)]
pub(super) struct RegisterPair {
    pub(super) ra: usize,
    pub(super) rb: usize,
}

#[derive(Debug)]
pub(super) struct RegisterTrio {
    pub(super) ra: usize,
    pub(super) rb: usize,
    pub(super) rc: usize,
}

#[derive(Debug)]
pub(super) struct LengthAndIndex {
    pub(super) length: usize,
    pub(super) index: usize,
}

pub(super) struct Instruction {
    /// Instruction mnemonic
    pub(super) name: &'static str,
    pub(super) name_prefix: Option<&'static str>,
    /// Options
    pub(super) opts: Option<InstructionOptions>,
}

impl Instruction {
    pub(super) fn new(name: &'static str) -> Self {
        Self { name, name_prefix: None, opts: None }
    }
    pub(super) fn set_name_prefix(mut self, prefix: Option<&'static str>) -> Self {
        self.name_prefix = prefix;
        self
    }
    pub(super) fn set_opts(mut self, opts: InstructionOptions) -> Self {
        self.opts = Some(opts);
        self
    }
}

pub(super) struct InstructionExt {
    pub(super) proto: Instruction,
    /// Instruction parameters extracted from code slice using proto.opts
    pub(super) params: Vec<InstructionParameter>,
    /// Variables
    pub(super) vars: Vec<StackItem>,
}

impl Default for InstructionExt {
    fn default() -> Self {
        Self { proto: Instruction::new("NOP"), params: Vec::new(), vars: Vec::new() }
    }
}

impl InstructionExt {
    fn name(&self) -> String {
        match self.proto.name_prefix {
            Some(prefix) => prefix.to_string() + self.proto.name,
            None => self.proto.name.to_string(),
        }
    }
    pub(super) fn clear(&mut self) {
        self.params.clear()
    }
    pub(super) fn cell_raw(&self) -> Option<&Cell> {
        param_ref!(self, Cell)
    }
    pub(super) fn creg_raw(&self) -> Option<usize> {
        param!(self, ControlRegister)
    }
    pub(super) fn division_mode_raw(&self) -> Option<&DivMode> {
        param_ref!(self, DivisionMode)
    }
    pub(super) fn integer_raw(&self) -> Option<isize> {
        param!(self, Integer)
    }
    pub(super) fn biginteger_raw(&self) -> Option<&IntegerData> {
        param_ref!(self, BigInteger)
    }
    pub(super) fn biginteger_mut_raw(&mut self) -> Option<&mut IntegerData> {
        param_ref_mut!(self, BigInteger)
    }
    pub(super) fn length_raw(&self) -> Option<usize> {
        param!(self, Length)
    }
    pub(super) fn nargs_raw(&self) -> Option<isize> {
        param!(self, Nargs)
    }
    pub(super) fn pargs_raw(&self) -> Option<usize> {
        param!(self, Pargs)
    }
    pub(super) fn rargs_raw(&self) -> Option<usize> {
        param!(self, Rargs)
    }
    pub(super) fn slice_raw(&self) -> Option<&SliceData> {
        param_ref!(self, Slice)
    }
    pub(super) fn sreg_raw(&self) -> Option<usize> {
        param!(self, StackRegister)
    }
    pub(super) fn sregs_raw(&self) -> Option<&RegisterPair> {
        param_ref!(self, StackRegisterPair)
    }
    pub(super) fn sregs3_raw(&self) -> Option<&RegisterTrio> {
        param_ref!(self, StackRegisterTrio)
    }
    pub(super) fn length_and_index_raw(&self) -> Option<&LengthAndIndex> {
        param_ref!(self, LengthAndIndex)
    }
    pub(super) fn cell(&self) -> &Cell {
        self.cell_raw().unwrap()
    }
    pub(super) fn creg(&self) -> usize {
        self.creg_raw().unwrap()
    }
    pub(super) fn biginteger_mut(&mut self) -> &mut IntegerData {
        self.biginteger_mut_raw().unwrap()
    }
    pub(super) fn division_mode(&self) -> &DivMode {
        self.division_mode_raw().unwrap()
    }
    pub(super) fn has_length(&self) -> bool {
        self.length_raw().is_some()
    }
    pub(super) fn integer(&self) -> isize {
        self.integer_raw().unwrap()
    }
    pub(super) fn length(&self) -> usize {
        self.length_raw().unwrap()
    }
    pub(super) fn nargs(&self) -> isize {
        self.nargs_raw().unwrap_or(-1)
    }
    pub(super) fn pargs(&self) -> usize {
        self.pargs_raw().unwrap_or(0)
    }
    pub(super) fn push_var(&mut self, var: StackItem) {
        self.vars.push(var)
    }
    pub(super) fn rargs(&self) -> usize {
        self.rargs_raw().unwrap_or(0)
    }
    pub(super) fn slice(&self) -> &SliceData {
        self.slice_raw().unwrap()
    }
    pub(super) fn sreg(&self) -> usize {
        self.sreg_raw().unwrap()
    }
    pub(super) fn sregs(&self) -> &RegisterPair {
        self.sregs_raw().unwrap()
    }
    pub(super) fn sregs3(&self) -> &RegisterTrio {
        self.sregs3_raw().unwrap()
    }
    pub(super) fn length_and_index(&self) -> &LengthAndIndex {
        self.length_and_index_raw().unwrap()
    }
    pub(super) fn var(&self, index: usize) -> &StackItem {
        self.vars.get(index).unwrap()
    }
    pub(super) fn var_count(&self) -> usize {
        self.vars.len()
    }
    pub(super) fn var_mut(&mut self, index: usize) -> &mut StackItem {
        self.vars.get_mut(index).unwrap()
    }
    pub(super) fn last_var(&self) -> Result<&StackItem> {
        self.vars.last().ok_or_else(|| error!("no vars for {}", self.name()))
    }
    pub(super) fn last_var_mut(&mut self) -> Result<&mut StackItem> {
        let name = self.name();
        self.vars.last_mut().ok_or_else(|| error!("no vars for {}", name))
    }
    pub(super) fn pop_var(&mut self) -> Result<StackItem> {
        self.vars.pop().ok_or_else(|| error!("no vars for {}", self.name()))
    }
    pub(super) fn withdraw_vars(&mut self) -> Vec<StackItem> {
        std::mem::take(&mut self.vars)
    }
    pub(super) fn dump_with_params(&self) -> Option<String> {
        let mut trace = String::new();
        if let Some(prefix) = self.proto.name_prefix {
            trace += prefix;
        }
        trace += self.proto.name;
        trace += &match self.proto.opts {
            Some(InstructionOptions::ArgumentAndReturnConstraints) => {
                format!(" {}, {}", self.pargs_raw()?, self.rargs_raw()?)
            }
            Some(InstructionOptions::ArgumentConstraints) => {
                format!(" {}, {}", self.pargs_raw()?, self.nargs_raw()?)
            }
            Some(InstructionOptions::BigInteger) => format!(" {}", self.biginteger_raw()?), // TODO: it is zero because execution withdraws it
            Some(InstructionOptions::ControlRegister) => format!(" c{}", self.creg_raw()?),
            Some(InstructionOptions::DivisionMode) => {
                let mode = self.division_mode();
                if mode.shift_parameter() {
                    format!(" {}", self.length())
                } else {
                    String::new()
                }
            }
            Some(InstructionOptions::Integer(_)) => format!(" {}", self.integer_raw()?),
            Some(InstructionOptions::Length(_)) | Some(InstructionOptions::LengthMinusOne(_)) => {
                format!(" {}", self.length_raw()?)
            }
            Some(InstructionOptions::LengthAndIndex)
            | Some(InstructionOptions::LengthMinusOneAndIndexMinusOne)
            | Some(InstructionOptions::LengthMinusTwoAndIndex) => {
                let length_and_index = self.length_and_index_raw()?;
                format!(" {}, {}", length_and_index.length, length_and_index.index)
            }
            Some(InstructionOptions::Pargs(_)) => format!(" {}", self.pargs_raw()?),
            Some(InstructionOptions::Rargs(_)) => format!(" {}", self.rargs_raw()?),
            Some(InstructionOptions::StackRegister(_)) => format!(" s{}", self.sreg_raw()?),
            Some(InstructionOptions::StackRegisterPair(
                WhereToGetParams::GetFromNextByteMinusOne,
            )) => format!(" s{},s{}", self.sregs_raw()?.ra, self.sregs_raw()?.rb as isize - 1),
            Some(InstructionOptions::StackRegisterPair(_)) => {
                format!(" s{},s{}", self.sregs_raw()?.ra, self.sregs_raw()?.rb)
            }
            Some(InstructionOptions::StackRegisterTrio(
                WhereToGetParams::GetFromNextByteMinusOne,
            )) => format!(
                " s{},s{},s{}",
                self.sregs3_raw()?.ra,
                self.sregs3_raw()?.rb,
                self.sregs3_raw()?.rc as isize - 1,
            ),
            Some(InstructionOptions::StackRegisterTrio(
                WhereToGetParams::GetFromNextByteMinusOneMinusOne,
            )) => format!(
                " s{},s{},s{}",
                self.sregs3_raw()?.ra,
                self.sregs3_raw()?.rb as isize - 1,
                self.sregs3_raw()?.rc as isize - 1,
            ),
            Some(InstructionOptions::StackRegisterTrio(
                WhereToGetParams::GetFromNextByteMinusOneMinusTwo,
            )) => format!(
                " s{},s{},s{}",
                self.sregs3_raw()?.ra,
                self.sregs3_raw()?.rb as isize - 1,
                self.sregs3_raw()?.rc as isize - 2,
            ),
            Some(InstructionOptions::StackRegisterTrio(_)) => format!(
                " s{},s{},s{}",
                self.sregs3_raw()?.ra,
                self.sregs3_raw()?.rb,
                self.sregs3_raw()?.rc,
            ),
            Some(InstructionOptions::Bitstring(_, _, _, _)) => format!(" x{:X}", self.slice_raw()?),
            Some(InstructionOptions::Bytestring(_, _, _, _)) => {
                format!(" x{:X}", self.slice_raw()?)
            }
            Some(InstructionOptions::Dictionary(_, _)) => format!(" {}", self.length_raw()?),
            None => String::new(),
        };
        Some(trace)
    }
}

impl From<Instruction> for InstructionExt {
    fn from(proto: Instruction) -> Self {
        Self {
            proto: Instruction {
                name: proto.name,
                name_prefix: proto.name_prefix,
                opts: proto.opts,
            },
            params: Vec::new(),
            vars: Vec::new(),
        }
    }
}

impl fmt::Debug for InstructionExt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.proto.name)
    }
}
