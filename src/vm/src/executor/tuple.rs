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
    executor::{
        engine::{storage::fetch_stack, Engine},
        gas::gas_state::Gas,
        types::{Instruction, InstructionOptions, WhereToGetParams},
    },
    stack::{integer::IntegerData, StackItem},
};
use ton_block::{fail, ExceptionCode, Mask, Status};

fn tuple(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut inst = Instruction::new(name);
    if how.bit(CMD) {
        inst = inst.set_opts(InstructionOptions::Length(0..16));
    }
    engine.load_instruction(inst)?;
    let params = if how.bit(CMD) {
        engine.cmd.length()
    } else {
        fetch_stack(engine, 1)?;
        engine.cmd.var(0).as_integer_value(0..=255)?
    };
    fetch_stack(engine, params)?;
    let vars = engine.cmd.var_count();
    let mut tuple = engine.cmd.vars.split_off(vars - params);
    tuple.reverse();
    engine.use_gas(Gas::tuple_gas_price(tuple.len()));
    engine.cc.stack.push_tuple(tuple);
    Ok(())
}

// TUPLE n (x1 . . . xn – t)
pub(super) fn execute_tuple_create(engine: &mut Engine) -> Status {
    tuple(engine, "TUPLE", CMD)
}

// TUPLEVAR (x1 . . . xn n – t)
pub(super) fn execute_tuple_createvar(engine: &mut Engine) -> Status {
    tuple(engine, "TUPLEVAR", STACK)
}

fn tuple_index(engine: &mut Engine, how: u8) -> Status {
    let index = how.mask(INDEX);
    let params = if index == 0 { 2 } else { 1 };
    engine.load_instruction(match index & 3 {
        0 => Instruction::new("INDEXVAR"),
        1 => Instruction::new("INDEX").set_opts(InstructionOptions::Length(0..16)),
        2 => Instruction::new("INDEX2").set_opts(InstructionOptions::StackRegisterPair(
            WhereToGetParams::GetFromLastByte2Bits,
        )),
        3 => Instruction::new("INDEX3").set_opts(InstructionOptions::StackRegisterTrio(
            WhereToGetParams::GetFromLastByte2Bits,
        )),
        _ => fail!("unreachabe tuple_index"),
    })?;
    fetch_stack(engine, params)?;
    let n = if index == 0 { engine.cmd.var(0).as_integer_value(0..=254)? } else { 0 };
    if engine.cmd.var(params - 1).is_null() && how.bit(QUIET) {
        engine.cc.stack.push(StackItem::None);
        return Ok(());
    }
    let value = match index & 3 {
        0 => engine.cmd.var(1).tuple_item(n, how.bit(QUIET))?,
        1 => {
            let n = engine.cmd.length();
            engine.cmd.var_mut(0).tuple_item(n, how.bit(QUIET))?
        }
        2 => {
            let n = engine.cmd.sregs().ra;
            let value = engine.cmd.var(0).tuple_item(n, false)?;
            let n = engine.cmd.sregs().rb;
            value.tuple_item(n, false)?
        }
        3 => {
            let n = engine.cmd.sregs3().ra;
            let value = engine.cmd.var(0).tuple_item(n, false)?;
            let n = engine.cmd.sregs3().rb;
            let value = value.tuple_item(n, false)?;
            let n = engine.cmd.sregs3().rc;
            value.tuple_item(n, false)?
        }
        _ => fail!("unreachabe tuple_index"),
    };
    engine.cc.stack.push(value);
    Ok(())
}

// INDEX k (t – x)
pub(super) fn execute_tuple_index(engine: &mut Engine) -> Status {
    tuple_index(engine, 1 | CMD)
}

// INDEXQ k (t – x)
pub(super) fn execute_tuple_index_quiet(engine: &mut Engine) -> Status {
    tuple_index(engine, 1 | CMD | QUIET)
}

// INDEX2 i,j (t – x)
pub(super) fn execute_tuple_index2(engine: &mut Engine) -> Status {
    tuple_index(engine, 2 | CMD)
}

// INDEX3 i,j,k (t – x)
pub(super) fn execute_tuple_index3(engine: &mut Engine) -> Status {
    tuple_index(engine, 3 | CMD)
}

// INDEXVAR (t n – x)
pub(super) fn execute_tuple_indexvar(engine: &mut Engine) -> Status {
    tuple_index(engine, STACK)
}

// INDEXVARQ (t n – x)
pub(super) fn execute_tuple_indexvar_quiet(engine: &mut Engine) -> Status {
    tuple_index(engine, STACK | QUIET)
}

const INDEX: u8 = 0x03; // mask for INDEX index
const COUNT: u8 = 0x01;
const CMD: u8 = 0x04;
const QUIET: u8 = 0x10;
const STACK: u8 = 0x08;

const CMP: u8 = 0xC0; // mask for comparsion
const EXACT: u8 = 0x40;
const LESS: u8 = 0x80;
const MORE: u8 = 0xC0;

fn untuple(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut params = 1;
    let mut inst = Instruction::new(name);

    if how.bit(STACK) {
        params += 1;
    }
    if how.bit(CMD) {
        inst = inst.set_opts(InstructionOptions::Length(0..16));
    }
    engine.load_instruction(inst)?;
    fetch_stack(engine, params)?;
    let mut n = if how.bit(CMD) {
        engine.cmd.length()
    } else if how.bit(STACK) {
        engine.cmd.var(0).as_integer_value(0..=255)?
    } else {
        0
    };
    let tuple = engine.cmd.var(params - 1).as_tuple()?;
    let len = tuple.len();
    let mask = how.mask(CMP);
    if ((mask == EXACT) && (len != n))
        || ((mask == LESS) && (len < n))
        || ((mask == MORE) && (len > n))
    {
        fail!(ExceptionCode::TypeCheckError)
    }
    if how.mask(CMP) == MORE {
        n = len;
    }
    engine.use_gas(Gas::tuple_gas_price(n));
    let mut vars = engine.cmd.var_mut(params - 1).withdraw_tuple_part(n)?;
    vars.drain(..).for_each(|v| {
        engine.cc.stack.push(v);
    });
    if how.bit(COUNT) {
        engine.cc.stack.push(int!(len));
    }
    Ok(())
}

// ISTUPLE (t – ?)
pub(super) fn execute_istuple(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ISTUPLE"))?;
    fetch_stack(engine, 1)?;
    let tuple = engine.cmd.var(0).as_tuple().is_ok();
    engine.cc.stack.push(boolean!(tuple));
    Ok(())
}

// UNPACKFIRST k (t – x1 . . . xk)
pub(super) fn execute_tuple_unpackfirst(engine: &mut Engine) -> Status {
    untuple(engine, "UNPACKFIRST", CMD | LESS)
}

// UNPACKFIRSTVAR (t n – x1 . . . xn)
pub(super) fn execute_tuple_unpackfirstvar(engine: &mut Engine) -> Status {
    untuple(engine, "UNPACKFIRSTVAR", STACK | LESS)
}

// UNTUPLE n (t – x1 . . . xn)
pub(super) fn execute_tuple_un(engine: &mut Engine) -> Status {
    untuple(engine, "UNTUPLE", CMD | EXACT)
}

// UNTUPLEVAR (t n – x1 . . . xn)
pub(super) fn execute_tuple_untuplevar(engine: &mut Engine) -> Status {
    untuple(engine, "UNTUPLEVAR", STACK | EXACT)
}

// EXPLODE n (t – x1 . . . xm m)
pub(super) fn execute_tuple_explode(engine: &mut Engine) -> Status {
    untuple(engine, "EXPLODE", CMD | MORE | COUNT)
}

// EXPLODEVAR (t n – x1 . . . xm m)
pub(super) fn execute_tuple_explodevar(engine: &mut Engine) -> Status {
    untuple(engine, "EXPLODEVAR", STACK | MORE | COUNT)
}

fn set_index(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut params = 2;
    let mut inst = Instruction::new(name);

    if how.bit(STACK) {
        params += 1;
    }
    if how.bit(CMD) {
        inst = inst.set_opts(InstructionOptions::Length(0..16));
    }
    engine.load_instruction(inst)?;
    fetch_stack(engine, params)?;
    let n = if how.bit(CMD) {
        engine.cmd.length()
    } else if how.bit(STACK) {
        engine.cmd.var(0).as_integer_value(0..=254)?
    } else {
        unreachable!("internal error in set_index, how = {}", how)
    };
    let mut tuple = if how.bit(QUIET) && engine.cmd.var(params - 1).is_null() {
        vec![]
    } else {
        engine.cmd.var_mut(params - 1).as_tuple_mut()?
    };
    let var = engine.cmd.var_mut(params - 2).withdraw();
    let len = tuple.len();
    if n < len {
        tuple[n] = var;
        engine.use_gas(Gas::tuple_gas_price(len));
    } else if how.bit(QUIET) {
        if !var.is_null() {
            tuple.append(&mut vec![StackItem::None; n - len]);
            tuple.push(var);
            engine.use_gas(Gas::tuple_gas_price(n + 1));
        }
    } else {
        fail!(ExceptionCode::RangeCheckError, "set_index failed {} >= {}", n, len)
    }
    engine.cc.stack.push_tuple(tuple);
    Ok(())
}

// SETINDEX k (t x – t0)
pub(super) fn execute_tuple_setindex(engine: &mut Engine) -> Status {
    set_index(engine, "SETINDEX", CMD)
}

// SETINDEXQ k (t x – t0)
pub(super) fn execute_tuple_setindex_quiet(engine: &mut Engine) -> Status {
    set_index(engine, "SETINDEXQ", CMD | QUIET)
}

// SETINDEXVAR (t x k – t0)
pub(super) fn execute_tuple_setindexvar(engine: &mut Engine) -> Status {
    set_index(engine, "SETINDEXVAR", STACK)
}

// SETINDEXVARQ (t x k – t0)
pub(super) fn execute_tuple_setindexvar_quiet(engine: &mut Engine) -> Status {
    set_index(engine, "SETINDEXVARQ", STACK | QUIET)
}

fn tuple_length(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    let _ = match engine.cmd.var(0).as_tuple() {
        Ok(tuple) => engine.cc.stack.push(int!(tuple.len())),
        Err(_) if how.bit(QUIET) => engine.cc.stack.push(int!(-1)),
        Err(err) => return Err(err),
    };
    Ok(())
}

// TLEN (t – n)
pub(super) fn execute_tuple_len(engine: &mut Engine) -> Status {
    tuple_length(engine, "TLEN", 0)
}

// QTLEN (t – n or −1)
pub(super) fn execute_tuple_len_quiet(engine: &mut Engine) -> Status {
    tuple_length(engine, "QTLEN", QUIET)
}

// LAST (t – x)
pub(super) fn execute_tuple_last(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("LAST"))?;
    fetch_stack(engine, 1)?;
    match engine.cmd.var(0).as_tuple()?.last() {
        Some(var) => {
            engine.cc.stack.push(var.clone());
            Ok(())
        }
        None => fail!(ExceptionCode::TypeCheckError, "tuple is empty"),
    }
}

// TPUSH (t x – t0)
pub(super) fn execute_tuple_push(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("TPUSH"))?;
    fetch_stack(engine, 2)?;
    let len = engine.cmd.var(1).as_tuple()?.len();
    if len >= 255 {
        fail!(ExceptionCode::TypeCheckError);
    }
    let mut tuple = engine.cmd.var_mut(1).as_tuple_mut()?;
    let value = engine.cmd.var(0).clone();
    tuple.push(value);
    engine.use_gas(Gas::tuple_gas_price(tuple.len()));
    engine.cc.stack.push_tuple(tuple);
    Ok(())
}

// TPOP (t – t0 x)
pub(super) fn execute_tuple_pop(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("TPOP"))?;
    fetch_stack(engine, 1)?;
    let mut tuple = engine.cmd.var_mut(0).as_tuple_mut()?;
    let value = tuple.pop().ok_or(ExceptionCode::TypeCheckError)?;
    engine.use_gas(Gas::tuple_gas_price(tuple.len()));
    engine.cc.stack.push_tuple(tuple);
    engine.cc.stack.push(value);
    Ok(())
}
