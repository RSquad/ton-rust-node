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
        types::Instruction,
    },
    stack::{integer::IntegerData, StackItem},
};
use ton_block::{Mask, Status};

pub(super) fn execute_null(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("NULL"))?;
    engine.cc.stack.push(StackItem::None);
    Ok(())
}

pub(super) fn execute_isnull(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ISNULL"))?;
    fetch_stack(engine, 1)?;
    let result = engine.cmd.var(0).is_null();
    engine.cc.stack.push(boolean!(result));
    Ok(())
}

const ARG: u8 = 0x03; // args number
const DBL: u8 = 0x04; // DouBLe NULL in result
const INV: u8 = 0x08; // INVert rule to get output value: get it upon unsuccessful call
const ZERO: u8 = 0x10; // zeroswapif instead nullswapif

fn nullzeroswapif(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let args = how.mask(ARG);
    debug_assert!(args == 1 || args == 2);
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, args as usize)?;
    let new_element = if how.bit(ZERO) { int!(0) } else { StackItem::None };
    if engine.cmd.var(0).as_bool()? ^ how.bit(INV) {
        if how.bit(DBL) {
            engine.cc.stack.push(new_element.clone());
        }
        engine.cc.stack.push(new_element);
    }
    if args > 1 {
        engine.cc.stack.push(engine.cmd.vars.remove(1));
    }
    engine.cc.stack.push(engine.cmd.vars.remove(0));
    Ok(())
}

// integer - (integer) | (null integer)
pub(super) fn execute_nullswapif(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLSWAPIF", 1)
}

// integer - (integer) | (null integer)
pub(super) fn execute_nullswapif2(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLSWAPIF2", 1 | DBL)
}

// integer - (integer) | (null integer)
pub(super) fn execute_nullswapifnot(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLSWAPIFNOT", 1 | INV)
}

// integer - (integer) | (null integer)
pub(super) fn execute_nullswapifnot2(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLSWAPIFNOT2", 1 | INV | DBL)
}

// x integer - (x integer) | (null x integer)
pub(super) fn execute_nullrotrif(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLROTRIF", 2)
}

// x integer - (x integer) | (null x integer)
pub(super) fn execute_nullrotrif2(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLROTRIF2", 2 | DBL)
}

// x integer - (x integer) | (null x integer)
pub(super) fn execute_nullrotrifnot(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLROTRIFNOT", 2 | INV)
}

// x integer - (x integer) | (null x integer)
pub(super) fn execute_nullrotrifnot2(engine: &mut Engine) -> Status {
    nullzeroswapif(engine, "NULLROTRIFNOT2", 2 | INV | DBL)
}
