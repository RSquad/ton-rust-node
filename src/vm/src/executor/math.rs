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
    error::tvm_exception_code,
    executor::{
        engine::{storage::fetch_stack, Engine},
        types::{Instruction, InstructionOptions},
    },
    stack::{
        integer::{
            behavior::OperationBehavior,
            math::{
                utils::{div_by_shift, divmod},
                Round,
            },
            utils::{binary_op, construct_double_nan, process_double_result, unary_op},
            IntegerData,
        },
        StackItem,
    },
};
use std::{borrow::Cow, cmp::Ordering, mem};
use ton_block::{fail, Bitmask, ExceptionCode, Result, Status};

// Common definitions *********************************************************

type Binary = fn(&IntegerData, &IntegerData) -> Result<IntegerData>;
type BinaryAssign = fn(&IntegerData, &mut IntegerData) -> Status;
type BinaryConst = fn(isize, &IntegerData) -> Result<IntegerData>;
type Unary = fn(&IntegerData) -> Result<IntegerData>;
type UnaryWithLen = fn(&IntegerData, usize) -> Result<IntegerData>;
type FnFits = fn(&IntegerData, usize) -> Result<bool>;

// Implementation of binary operation which takes both arguments from stack
fn binary<T>(engine: &mut Engine, name: &'static str, handler: Binary) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new(name).set_name_prefix(T::name_prefix()))?;
    if engine.cc.stack.depth() < 2 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).withdraw();
    let v1 = engine.cc.stack.get_mut(1).as_integer_mut()?;
    *v1 = handler(v0.as_integer()?, v1)?;
    engine.cc.stack.drop(0)?;
    Ok(())
}

fn binary_assign<T>(engine: &mut Engine, name: &'static str, handler: BinaryAssign) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new(name).set_name_prefix(T::name_prefix()))?;
    if engine.cc.stack.depth() < 2 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).withdraw();
    let v1 = engine.cc.stack.get_mut(1).as_integer_mut()?;
    handler(v0.as_integer()?, v1)?;
    engine.cc.stack.drop(0)?;
    Ok(())
}

// Implementation of binary operation which takes one argument from stack
// and another from instruction
fn binary_with_const<T>(engine: &mut Engine, name: &'static str, handler: BinaryConst) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(
        Instruction::new(name)
            .set_name_prefix(T::name_prefix())
            .set_opts(InstructionOptions::Integer(-128..128)),
    )?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cmd.integer();
    let v1 = engine.cc.stack.get_mut(0).as_integer_mut()?;
    *v1 = handler(v0, v1)?;
    Ok(())
}

// Implementation of binary comparsion two arguments in stack
const MIN: Bitmask = 0x01;
const MAX: Bitmask = 0x02;
fn minmax<T>(engine: &mut Engine, name: &'static str, compare_type: Bitmask) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new(name).set_name_prefix(T::name_prefix()))?;
    fetch_stack(engine, 2)?;
    let mut x = engine.cmd.var(0).clone();
    let mut y = engine.cmd.var(1).clone();
    match x.as_integer()?.compare::<T>(y.as_integer()?)? {
        None => {
            on_nan_parameter!(T)?;
            x = int!(nan);
            y = int!(nan);
        }
        Some(Ordering::Less) => {
            if compare_type == MAX {
                mem::swap(&mut x, &mut y);
            }
        }
        _ => {
            if compare_type != MAX {
                mem::swap(&mut x, &mut y);
            }
        }
    };
    engine.cc.stack.push(x);
    if compare_type == MIN | MAX {
        engine.cc.stack.push(y);
    }
    Ok(())
}

// Implementation of common function for different fits_in
fn fits_in<T>(engine: &mut Engine, length: usize, op_fit: FnFits) -> Status
where
    T: OperationBehavior,
{
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let x = engine.cc.stack.get(0)?.as_integer()?;
    if x.is_nan() {
        on_nan_parameter!(T)?;
        *engine.cc.stack.get_mut(0) = int!(nan);
    } else if !op_fit(x, length)? {
        on_integer_overflow!(T)?;
        *engine.cc.stack.get_mut(0) = int!(nan);
    }
    Ok(())
}

// Implementation of unary operation which takes its argument from stack
fn unary<T>(engine: &mut Engine, name: &'static str, handler: Unary) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new(name).set_name_prefix(T::name_prefix()))?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).as_integer_mut()?;
    *v0 = handler(v0)?;
    Ok(())
}

// Implementation of unary operation which takes its argument from stack
// and makes use of the parameter from instruction
fn unary_with_len<T>(engine: &mut Engine, name: &'static str, handler: UnaryWithLen) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(
        Instruction::new(name)
            .set_name_prefix(T::name_prefix())
            .set_opts(InstructionOptions::LengthMinusOne(0..256)),
    )?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).as_integer_mut()?;
    *v0 = handler(v0, engine.cmd.length())?;
    Ok(())
}

macro_rules! boolint {
    ($val:expr) => {
        if $val {
            IntegerData::minus_one()
        } else {
            IntegerData::zero()
        }
    };
}

macro_rules! cmp {
    ($x:expr, $y:expr, $t:ty, $rule:expr) => {
        compare::<$t>($x, $y, $rule, file!(), line!())
    };
}

// Comparison rules
const EQUAL: Bitmask = 0x01;
const GREATER: Bitmask = 0x02;
const LESS: Bitmask = 0x04;

fn compare<T>(
    x: &IntegerData,
    y: &IntegerData,
    comparison_rule: Bitmask,
    file: &'static str,
    line: u32,
) -> Result<IntegerData>
where
    T: OperationBehavior,
{
    let result = x.compare::<T>(y)?;
    if comparison_rule == 0 {
        match result {
            Some(Ordering::Equal) => Ok(IntegerData::zero()),
            Some(Ordering::Greater) => Ok(IntegerData::one()),
            Some(Ordering::Less) => Ok(IntegerData::minus_one()),
            None => {
                T::on_nan_parameter(file, line)?;
                Ok(IntegerData::nan())
            }
        }
    } else {
        match result {
            Some(Ordering::Equal) => Ok(boolint!((comparison_rule & EQUAL) != 0)),
            Some(Ordering::Greater) => Ok(boolint!((comparison_rule & GREATER) != 0)),
            Some(Ordering::Less) => Ok(boolint!((comparison_rule & LESS) != 0)),
            None => {
                T::on_nan_parameter(file, line)?;
                Ok(IntegerData::nan())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DivMode {
    pub(super) flags: Bitmask,
}

#[rustfmt::skip]
impl DivMode {

    const PRE_MULTIPLICATION: Bitmask                    = 0b10000000;
    const MULTIPLICATION_REPLACED_BY_LEFT_SHIFT: Bitmask = 0b01000000;
    const DIVISION_REPLACED_BY_RIGHT_SHIFT: Bitmask      = 0b00100000;
    const SHIFT_OPERATION_PARAMETER_PASSED: Bitmask      = 0b00010000;
    const REMAINDER_RESULT_REQUIRED: Bitmask             = 0b00001000;
    const QUOTIENT_RESULT_REQUIRED: Bitmask              = 0b00000100;
    const ROUNDING_MODE_CEILING: Bitmask                 = 0b00000010;
    const ROUNDING_MODE_NEAREST_INTEGER: Bitmask         = 0b00000001;

    pub(super) fn with_flags(flags: Bitmask) -> DivMode {
        DivMode {
            flags,
        }
    }

    const NAMES: [([&'static str; 3], u8); 32] = [
        (["ADDDIVMODC",           "ADDDIVMOD",           "ADDDIVMODR"          ], 0b0000_0000),  // preadd div
        (["DIVC",                 "DIV",                 "DIVR"                ], 0b0000_0100),  // !R
        (["MODC",                 "MOD",                 "MODR"                ], 0b0000_1000),  // !Q
        (["DIVMODC",              "DIVMOD",              "DIVMODR"             ], 0b0000_1100),  // div    

        (["ADDRSHIFTMODC_VAR",    "ADDRSHIFTMOD_VAR",    "ADDRSHIFTMODR_VAR"   ], 0b0010_0000),  // preadd + div-by-shift
        (["MODPOW2C_VAR",         "MODPOW2_VAR",         "MODPOW2R_VAR"        ], 0b0010_0100),  // !R + div-by-shift
        (["RSHIFTC_VAR",          "RSHIFT_VAR",          "RSHIFTR_VAR"         ], 0b0010_1000),  // !Q + div-by-shift
        (["RSHIFTMODC_VAR",       "RSHIFTMOD_VAR",       "RSHIFTMODR_VAR"      ], 0b0010_1100),  // div-by-shift

        (["ADDRSHIFTMODC",        "ADDRSHIFTMOD",        "ADDRSHIFTMODR"       ], 0b0011_0000),  // preadd + div-by-shift
        (["MODPOW2C",             "MODPOW2",             "MODPOW2R"            ], 0b0011_0100),  // !R + div-by-shift
        (["RSHIFTC",              "RSHIFT",              "RSHIFTR"             ], 0b0011_1000),  // !Q + div-by-shift
        (["RSHIFTMODC",           "RSHIFTMOD",           "RSHIFTMODR"          ], 0b0011_1100),  // div-by-shift

        (["MULADDDIVMODC",        "MULADDDIVMOD",        "MULADDDIVMODR"       ], 0b1000_0000),  // premultiply + preadd
        (["MULDIVC",              "MULDIV",              "MULDIVR"             ], 0b1000_0100),  // !R + div-by-shift
        (["MULMODC",              "MULMOD",              "MULMODR"             ], 0b1000_1000),  // !Q + div-by-shift
        (["MULDIVMODC",           "MULDIVMOD",           "MULDIVMODR"          ], 0b1000_1100),  // premultiply

        (["MULADDRSHIFTCMOD_VAR", "MULADDRSHIFTMOD_VAR", "MULADDRSHIFTRMOD_VAR"], 0b1010_0000),  // premultiply + preadd + div-by-shift
        (["MULRSHIFTC_VAR",       "MULRSHIFT_VAR",       "MULRSHIFTR_VAR"      ], 0b1010_0100),  // !R + premultiply + div-by-shift
        (["MULMODPOW2C_VAR",      "MULMODPOW2_VAR",      "MULMODPOW2R_VAR"     ], 0b1010_1000),  // !Q + premultiply + div-by-shift
        (["MULRSHIFTMODC_VAR",    "MULRSHIFTMOD_VAR",    "MULRSHIFTMODR_VAR"   ], 0b1010_1100),  // premultiply + div-by-shift

        (["MULADDRSHIFTCMOD",     "MULADDRSHIFTMOD",     "MULADDRSHIFTRMOD"    ], 0b1011_0000),  // premultiply + preadd + div-by-shift
        (["MULRSHIFTC",           "MULRSHIFT",           "MULRSHIFTR"          ], 0b1011_0100),  // !R + premultiply + div-by-shift
        (["MULMODPOW2C",          "MULMODPOW2",          "MULMODPOW2R"         ], 0b1011_1000),  // !Q + premultiply + div-by-shift
        (["MULMODPOW2C",          "MULRSHIFTMOD",        "MULMODPOW2R"         ], 0b1011_1100),  // premultiply + div-by-shift

        (["LSHIFTADDDIVMODC_VAR", "LSHIFTADDDIVMOD_VAR", "LSHIFTADDDIVMODR_VAR"], 0b1100_0000),  // premultiply + preadd mul-by-shift
        (["LSHIFTDIVC_VAR",       "LSHIFTDIV_VAR",       "LSHIFTDIVR_VAR"      ], 0b1100_0100),  // !R + premultiply + mul-by-shift
        (["LSHIFTMODC_VAR",       "LSHIFTMOD_VAR",       "LSHIFTMODR_VAR"      ], 0b1100_1000),  // !Q + premultiply + mul-by-shift
        (["LSHIFTDIVMODC_VAR",    "LSHIFTDIVMOD_VAR",    "LSHIFTDIVMODR_VAR"   ], 0b1100_1100),  // premultiply + mul-by-shift

        (["LSHIFTADDDIVMODC",     "LSHIFTADDDIVMOD",     "LSHIFTADDDIVMODR"    ], 0b1101_0000),  // premultiply + preadd mul-by-shift
        (["LSHIFTDIVC",           "LSHIFTDIV",           "LSHIFTDIVR"          ], 0b1101_0100),  // !R + premultiply + mul-by-shift
        (["LSHIFTMODC",           "LSHIFTMOD",           "LSHIFTMODR"          ], 0b1101_1000),  // !Q + premultiply + mul-by-shift
        (["LSHIFTDIVMODC",        "LSHIFTDIVMOD",        "LSHIFTDIVMODR"       ], 0b1101_1100),  // premultiply + mul-by-shift
    ];

    pub(super) fn command_name(&self) -> Result<&'static str> {
        if !self.is_valid() {
            fail!(ExceptionCode::InvalidOpcode)
        }
        let mut index = 0;
        if self.premultiply() { // 0b10000000
            index += 12;
        }
        if self.mul_by_shift() { // 0b11000000
            index += 12
        }
        if self.div_by_shift() { // 0b00100000
            index += 4
        }
        if self.shift_parameter() { // 0b00010000
            index += 4
        }
        match (self.contains(DivMode::REMAINDER_RESULT_REQUIRED), self.contains(DivMode::QUOTIENT_RESULT_REQUIRED)) {
            (false, false) => index += 0,
            (false, true) => index += 1, // 0b00000100
            (true, false) => index += 2, // 0b00001000
            (true, true) => index += 3, // 0b00001100
        }
        match self.rounding_strategy() {
            Ok(rounding_strategy) => Ok(Self::NAMES[index].0[rounding_strategy as usize]),
            Err(e) => Err(e),
        }
    }

    pub(super) fn is_valid(&self) -> bool {
        !self.contains(DivMode::DIVISION_REPLACED_BY_RIGHT_SHIFT | DivMode::MULTIPLICATION_REPLACED_BY_LEFT_SHIFT)
            && !self.contains(DivMode::ROUNDING_MODE_NEAREST_INTEGER | DivMode::ROUNDING_MODE_CEILING)
            && (!self.contains(DivMode::MULTIPLICATION_REPLACED_BY_LEFT_SHIFT) || self.premultiply())
            && (!self.shift_parameter() || self.mul_by_shift() || self.div_by_shift())
    }

    #[cfg(test)]
    pub(super) fn command_name_static(&self) -> Result<&'static str> {
        let Some((names, _)) = Self::NAMES.iter().find(|(_, mode)| *mode == (self.flags & 0xFC)) else {
            fail!(ExceptionCode::InvalidOpcode)
        };
        match self.rounding_strategy() {
            Ok(rounding_strategy) => Ok(names[rounding_strategy as usize]),
            Err(e) => Err(e),
        }
    }

    fn contains(&self, bits: Bitmask) -> bool {
        (self.flags & bits) == bits
    }

    pub(super) fn div_by_shift(&self) -> bool {
        self.contains(DivMode::DIVISION_REPLACED_BY_RIGHT_SHIFT)
    }

    pub(super) fn mul_by_shift(&self) -> bool {
        self.contains(
            DivMode::PRE_MULTIPLICATION | DivMode::MULTIPLICATION_REPLACED_BY_LEFT_SHIFT
        )
    }

    pub(super) fn need_quotient(&self) -> bool {
        self.contains(DivMode::QUOTIENT_RESULT_REQUIRED) || !self.contains(DivMode::REMAINDER_RESULT_REQUIRED)
    }

    pub(super) fn need_remainder(&self) -> bool {
        self.contains(DivMode::REMAINDER_RESULT_REQUIRED) || !self.contains(DivMode::QUOTIENT_RESULT_REQUIRED)
    }

    pub(super) fn premultiply(&self) -> bool {
        self.contains(DivMode::PRE_MULTIPLICATION)
    }

    pub(super) fn preadd(&self) -> bool {
        !self.contains(DivMode::QUOTIENT_RESULT_REQUIRED) && !self.contains(DivMode::REMAINDER_RESULT_REQUIRED)
    }

    pub(super) fn rounding_strategy(&self) -> Result<Round> {
        if self.contains(DivMode::ROUNDING_MODE_NEAREST_INTEGER | DivMode::ROUNDING_MODE_CEILING) {
            fail!(ExceptionCode::InvalidOpcode)
        } else if self.contains(DivMode::ROUNDING_MODE_NEAREST_INTEGER) {
            Ok(Round::Nearest)
        } else if self.contains(DivMode::ROUNDING_MODE_CEILING) {
            Ok(Round::Ceil)
        } else {
            Ok(Round::FloorToNegativeInfinity)
        }
    }

    pub(super) fn shift_parameter(&self) -> bool {
        self.contains(DivMode::SHIFT_OPERATION_PARAMETER_PASSED)
    }

}

// Implementation *************************************************************

// (x – |x|)
pub(super) fn execute_abs<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new("ABS").set_name_prefix(T::name_prefix()))?;
    fetch_stack(engine, 1)?;
    let var = engine.cmd.var(0).clone();
    if var.as_integer()?.is_nan() {
        on_nan_parameter!(T)?;
        engine.cc.stack.push(var);
    } else if var.as_integer()?.is_neg() {
        engine.cc.stack.push(StackItem::int(var.as_integer()?.neg::<T>()?));
    } else {
        engine.cc.stack.push(var);
    }
    Ok(())
}

// (x y - x+y)
pub(super) fn execute_add<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_assign::<T>(engine, "ADD", |y, x| x.add_assign::<T>(y))
}

// (x - x+y)
pub(super) fn execute_addconst<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "ADDCONST", |y, x| x.add_i8::<T>(&(y as i8)))
}

// (x y - x&y)
pub(super) fn execute_and<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "AND", |y, x| x.and::<T>(y))
}

// (x - c)
pub(super) fn execute_bitsize<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "BITSIZE", |x| {
        if x.is_nan() {
            on_nan_parameter!(T)?;
            Ok(IntegerData::nan())
        } else if x.is_zero() {
            Ok(IntegerData::zero())
        } else {
            Ok(IntegerData::from_u32(x.bitsize()? as u32))
        }
    })
}

// (x - x), throws exception if x == NaN
pub(super) fn execute_chknan(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CHKNAN"))?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    if engine.cc.stack.get(0)?.as_integer()?.is_nan() {
        fail!(ExceptionCode::IntegerOverflow)
    }
    Ok(())
}

// (x y - x?y)
pub(super) fn execute_cmp<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "CMP", |y, x| cmp!(x, y, T, 0))
}

// (x - x-1)
pub(super) fn execute_dec<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new("DEC").set_name_prefix(T::name_prefix()))?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).as_integer_mut()?;
    *v0 = v0.sub_i8::<T>(&1)?;
    Ok(())
}

fn get_var<'a>(engine: &'a Engine, index: &mut isize) -> Result<&'a IntegerData> {
    if *index < 0 {
        fail!(ExceptionCode::StackUnderflow);
    }
    let result = engine.cmd.var(*index as usize).as_integer();
    *index -= 1;
    result
}

fn get_shift(engine: &Engine, index: &mut isize) -> Result<usize> {
    if engine.cmd.has_length() {
        Ok(engine.cmd.length())
    } else {
        Ok(get_var(engine, index)?.as_integer_value(0..=256)?)
    }
}

// Multiple division modes
pub(super) fn execute_divmod<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(
        Instruction::new("DIV")
            .set_name_prefix(T::name_prefix())
            .set_opts(InstructionOptions::DivisionMode),
    )?;
    let mode = engine.cmd.division_mode().clone();
    if !mode.is_valid() {
        fail!(ExceptionCode::InvalidOpcode);
    }

    let mut n = 1;
    if mode.premultiply() && !(mode.mul_by_shift() && engine.cmd.has_length()) {
        n += 1
    }
    if !mode.div_by_shift() || !engine.cmd.has_length() {
        n += 1
    }
    if mode.preadd() {
        n += 1;
    }

    fetch_stack(engine, n)?;
    for i in 0..n {
        engine.cmd.var(i).as_integer()?;
    }

    let mut index = n as isize - 1;
    let rounding = mode.rounding_strategy()?;
    let x = get_var(engine, &mut index)?;
    let y = if mode.premultiply() && !mode.mul_by_shift() {
        get_var(engine, &mut index)?
    } else {
        &IntegerData::nan()
    };
    let w = if mode.preadd() { get_var(engine, &mut index)? } else { &IntegerData::nan() };
    let z = if !mode.div_by_shift() { get_var(engine, &mut index)? } else { &IntegerData::nan() };
    // dbg!(x, y, w, z);
    let v = if mode.premultiply() {
        let v = if mode.mul_by_shift() {
            let shift = get_shift(engine, &mut index)?;
            // dbg!(x, shift);
            unary_op::<T, _, _, _, _, _>(x, |x| x << shift, || None, |result, _| Ok(Some(result)))?
        } else {
            // dbg!(x, y);
            debug_assert!(y.as_int().is_some());
            binary_op::<T, _, _, _, _, _>(
                x,
                y,
                |x, y| x * y,
                || None,
                |result, _| Ok(Some(result)),
            )?
        };
        Cow::Owned(v)
    } else {
        Cow::Borrowed(x.as_int())
    };
    let v = if mode.preadd() {
        debug_assert!(w.as_int().is_some());
        let v = if let Some(v) = v.as_ref() {
            // dbg!(v, w);
            unary_op::<T, _, _, _, _, _>(w, |w| v + w, || None, |result, _| Ok(Some(result)))?
        } else {
            None
        };
        Cow::Owned(v)
    } else {
        v
    };
    let (q, r) = if let Some(v) = v.as_ref() {
        if mode.div_by_shift() {
            let shift = get_shift(engine, &mut index)?;
            // dbg!(v, shift);
            process_double_result::<T, _>(div_by_shift(v, shift, rounding), construct_double_nan)?
        } else {
            // dbg!(v, z);
            debug_assert!(z.as_int().is_some());
            if z.is_zero() {
                on_integer_overflow!(T)?;
                construct_double_nan()
            } else {
                unary_op::<T, _, _, _, _, _>(
                    z,
                    |z| divmod(v, z, rounding),
                    construct_double_nan,
                    process_double_result::<T, _>,
                )?
            }
        }
    } else {
        construct_double_nan()
    };

    if mode.need_quotient() {
        engine.cc.stack.push(StackItem::int(q));
    }
    if mode.need_remainder() {
        engine.cc.stack.push(StackItem::int(r));
    }
    Ok(())
}

// (x y - x==y)
pub(super) fn execute_equal<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "EQUAL", |y, x| cmp!(x, y, T, EQUAL))
}

// (x - x==y)
pub(super) fn execute_eqint<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "EQINT", |y, x| {
        cmp!(x, &IntegerData::from_i32(y as i32), T, EQUAL)
    })
}

// (x - x), throws exception if does not fit
pub(super) fn execute_fits<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(
        Instruction::new("FITS")
            .set_name_prefix(T::name_prefix())
            .set_opts(InstructionOptions::LengthMinusOne(0..256)),
    )?;
    let length = engine.cmd.length();
    fits_in::<T>(engine, length, IntegerData::fits_in)
}

// (x c - x), throws exception if does not fit
pub(super) fn execute_fitsx<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new("FITSX").set_name_prefix(T::name_prefix()))?;
    fetch_stack(engine, 1)?;
    let length = engine.cmd.var(0).as_integer_value(0..=1023)?;
    fits_in::<T>(engine, length, IntegerData::fits_in)
}

// (x y - x>=y)
pub(super) fn execute_geq<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "GEQ", |y, x| cmp!(x, y, T, EQUAL | GREATER))
}

// (x y - x>y)
pub(super) fn execute_greater<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "GREATER", |y, x| cmp!(x, y, T, GREATER))
}

// (x - x>y)
pub(super) fn execute_gtint<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "GTINT", |y, x| {
        cmp!(x, &IntegerData::from_i32(y as i32), T, GREATER)
    })
}

// (x - x+1)
pub(super) fn execute_inc<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new("INC").set_name_prefix(T::name_prefix()))?;
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow)
    }
    let v0 = engine.cc.stack.get_mut(0).as_integer_mut()?;
    *v0 = v0.add_i8::<T>(&1)?;
    Ok(())
}

// (x - x==NaN)
pub(super) fn execute_isnan(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ISNAN"))?;
    fetch_stack(engine, 1)?;
    let is_nan = engine.cmd.var(0).as_integer()?.is_nan();
    engine.cc.stack.push(boolean!(is_nan));
    Ok(())
}

// (x y - x<=y)
pub(super) fn execute_leq<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "LEQ", |y, x| cmp!(x, y, T, EQUAL | LESS))
}

// (x y - x<y)
pub(super) fn execute_less<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "LESS", |y, x| cmp!(x, y, T, LESS))
}

// (x - x<y)
pub(super) fn execute_lessint<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "LESSINT", |y, x| {
        cmp!(x, &IntegerData::from_i32(y as i32), T, LESS)
    })
}

// (x y - x<<y)
pub(super) fn execute_lshift<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    if engine.last_cmd() == 0xAC {
        binary::<T>(engine, "LSHIFT", |y, x| x.shl::<T>(y.as_integer_value(0..=1023)?))
    } else {
        unary_with_len::<T>(engine, "LSHIFT", |x, y| x.shl::<T>(y))
    }
}

// (x y - max(x, y))
pub(super) fn execute_max<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    minmax::<T>(engine, "MAX", MAX)
}

// (x y - min(x, y))
pub(super) fn execute_min<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    minmax::<T>(engine, "MIN", MIN)
}

// (x y - min(x, y) max(y,x))
pub(super) fn execute_minmax<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    minmax::<T>(engine, "MINMAX", MIN | MAX)
}

// (x y - x*y)
pub(super) fn execute_mul<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "MUL", |y, x| x.mul::<T>(y))
}

// (x - x*y)
pub(super) fn execute_mulconst<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "MULCONST", |y, x| x.mul_i8::<T>(&(y as i8)))
}

// (x - -x)
pub(super) fn execute_negate<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "NEGATE", |x| x.neg::<T>())
}

// (x y - x!=y)
pub(super) fn execute_neq<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "NEQ", |y, x| cmp!(x, y, T, GREATER | LESS))
}

// (x - x!=y)
pub(super) fn execute_neqint<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary_with_const::<T>(engine, "NEQINT", |y, x| {
        cmp!(x, &IntegerData::from_i32(y as i32), T, GREATER | LESS)
    })
}

// (x y - ~x)
pub(super) fn execute_not<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "NOT", |x| x.not::<T>())
}

// (x y - x|y)
pub(super) fn execute_or<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "OR", |y, x| x.or::<T>(y))
}

// (x - 2^x)
pub(super) fn execute_pow2<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "POW2", |x| match x.as_integer_value(0..=1023) {
        Ok(shift) => IntegerData::one().shl::<T>(shift),
        Err(exception) => match tvm_exception_code(&exception) {
            Some(ExceptionCode::IntegerOverflow) => {
                on_integer_overflow!(T)?;
                Ok(IntegerData::nan())
            }
            _ => Err(exception),
        },
    })
}

// (x - x>>y)
pub(super) fn execute_rshift<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    if engine.last_cmd() == 0xAD {
        binary::<T>(engine, "RSHIFT", |y, x| x.shr::<T>(y.as_integer_value(0..=1023)?))
    } else {
        unary_with_len::<T>(engine, "RSHIFT", |x, y| x.shr::<T>(y))
    }
}

// (x - sign(x))
pub(super) fn execute_sgn<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "SGN", |x| {
        if x.is_nan() {
            on_nan_parameter!(T)?;
            return Ok(IntegerData::nan());
        }
        Ok(if x.is_neg() {
            IntegerData::minus_one()
        } else if x.is_zero() {
            IntegerData::zero()
        } else {
            IntegerData::one()
        })
    })
}

// (x y - x-y)
pub(super) fn execute_sub<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "SUB", |y, x| x.sub::<T>(y))
}

// (x y - y-x)
pub(super) fn execute_subr<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "SUB", |y, x| y.sub::<T>(x))
}

// (x - c)
pub(super) fn execute_ubitsize<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    unary::<T>(engine, "UBITSIZE", |x| {
        if x.is_nan() || x.is_neg() {
            T::on_range_check_error(file!(), line!())?;
            Ok(IntegerData::nan())
        } else {
            Ok(IntegerData::from_u32(x.ubitsize()? as u32))
        }
    })
}

// (x - x), throws exception if does not fit
pub(super) fn execute_ufits<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(
        Instruction::new("UFITS")
            .set_name_prefix(T::name_prefix())
            .set_opts(InstructionOptions::LengthMinusOne(0..256)),
    )?;
    let length = engine.cmd.length();
    fits_in::<T>(engine, length, IntegerData::ufits_in)
}

// (x c - x), throws exception if does not fit
pub(super) fn execute_ufitsx<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new("UFITSX").set_name_prefix(T::name_prefix()))?;
    fetch_stack(engine, 1)?;
    let length = engine.cmd.var(0).as_integer_value(0..=1023)?;
    fits_in::<T>(engine, length, IntegerData::ufits_in)
}

// (x y - x^y)
pub(super) fn execute_xor<T>(engine: &mut Engine) -> Status
where
    T: OperationBehavior,
{
    binary::<T>(engine, "XOR", |y, x| x.xor::<T>(y))
}
