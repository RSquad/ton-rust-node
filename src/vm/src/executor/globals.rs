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
        types::{Instruction, InstructionOptions},
    },
    stack::StackItem,
};
use ton_block::{Mask, Status};

const STACK: u8 = 0x02;
const CMD: u8 = 0x04;
const SET: u8 = 0x10;

fn execute_setget_globalvar(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut inst = Instruction::new(name);
    let mut params = 0;
    if how.bit(CMD) {
        inst = inst.set_opts(InstructionOptions::Length(1..32))
    }
    if how.bit(STACK) {
        params += 1;
    }
    if how.bit(SET) {
        params += 1;
    }
    engine.load_instruction(inst)?;
    fetch_stack(engine, params)?;
    let k = if how.bit(STACK) {
        engine.cmd.var(0).as_integer_value(0..=254)?
    } else {
        engine.cmd.length()
    };
    if how.bit(SET) {
        let mut c7 = engine.ctrl_mut(7)?.as_tuple_mut()?;
        let x = engine.cmd.var_mut(params - 1).withdraw();
        let len = if k < c7.len() {
            c7[k] = x;
            c7.len()
        } else if !x.is_null() {
            c7.resize(k, StackItem::None);
            c7.push(x);
            c7.len()
        } else {
            0
        };
        engine.use_gas(Gas::tuple_gas_price(len));
        engine.ctrls.put(7, StackItem::tuple(c7))?;
    } else {
        let x = engine.ctrl(7)?.tuple_item(k, true)?;
        engine.cc.stack.push(x);
    }
    Ok(())
}

// GETGLOBVAR (k–x), returns the k-th global variable for 0 ≤ k < 255.
// Equivalent to PUSH c7; SWAP; INDEXVARQ
pub(super) fn execute_getglobvar(engine: &mut Engine) -> Status {
    execute_setget_globalvar(engine, "GETGLOBVAR", STACK)
}

// GETGLOB k( –x), returns the k-th global variable for 1 ≤ k ≤ 31
// Equivalent to PUSH c7; INDEXQ k.
pub(super) fn execute_getglob(engine: &mut Engine) -> Status {
    execute_setget_globalvar(engine, "GETGLOB", CMD)
}

// SETGLOBVAR (x k– ), assigns x to the k-th global variable for 0 ≤ k <255.
// Equivalent to PUSH c7; ROTREV; SETINDEXVARQ; POP c7.
pub(super) fn execute_setglobvar(engine: &mut Engine) -> Status {
    execute_setget_globalvar(engine, "SETGLOBVAR", SET | STACK)
}

// SETGLOB k (x– ), assigns x to the k-th global variable for 1 ≤ k ≤ 31.
// Equivalent to PUSH c7; SWAP; SETINDEXQ k; POP c7
pub(super) fn execute_setglob(engine: &mut Engine) -> Status {
    execute_setget_globalvar(engine, "SETGLOB", SET | CMD)
}
