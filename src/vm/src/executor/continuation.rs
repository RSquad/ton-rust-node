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
        engine::{
            data::convert,
            storage::{
                apply_savelist, apply_savelist_excluding_c0_c1, copy_to_var, fetch_reference,
                fetch_stack, pop_all, pop_range, swap,
            },
            Engine,
        },
        gas::gas_state::Gas,
        microcode::{CC, CELL, CONTINUATION, CTRL, SAVELIST, SLICE, VAR},
        types::{Instruction, InstructionOptions, InstructionParameter},
        RunChildVm,
    },
    stack::{
        continuation::{ContinuationData, ContinuationType},
        integer::{behavior::Signaling, IntegerData},
        savelist::SaveList,
        Stack, StackItem,
    },
};
use std::{
    mem,
    ops::{Range, RangeInclusive},
};
use ton_block::{fail, ExceptionCode, Mask, Status};

const CALLX: u8 = 0x40; // CALLX to found value
const SWITCH: u8 = 0x80; // SWITCH to found value
const PREPARE: u8 = 0xC0; // pass found value to stack

// Utilities ******************************************************************

// (continuation - ),
// pargs = cmd.pargs if any else cc.stack.depth
// if pargs > cc.stack.depth {
//     StackOverflow
// }
// move_out = cc.stack[0..pargs]
// move_in  = if continuation.nargs < 0 {
//     move_out
// } else if continuation.nargs > pargs {
//     StackOverflow
// } else {
//     move_out[0..pargs + continuation.nargs - continuation.stack.depth]
// }
// cc.stack -= move_out, continuation.stack += move_in,
// if cmd.nargs exists {
//     if cmd.nargs < 0 {
//         cc.nargs = -1
//     } else {
//         cc.nargs = cc.stack.depth + cmd.nargs
//     }
// }
// continuation.stack.push(cc), cc = continuation, c[*] = cc.savelist[*]
fn callcc(engine: &mut Engine, callee: usize) -> Status {
    let vars = engine.cmd.var_count();
    match vars.cmp(&callee) {
        std::cmp::Ordering::Less => fail!("callcc: {:X}", callee),
        std::cmp::Ordering::Equal => fetch_stack(engine, 1)?,
        _ => (),
    }
    pop_all(engine, var!(callee))?;
    swap(engine, var!(callee), CC)?;
    apply_savelist(engine)?;
    let mut old_cc = engine.cmd.var_mut(callee).as_continuation_mut()?.withdraw();
    if let Some(nargs) = engine.cmd.nargs_raw() {
        old_cc.nargs = nargs
    }
    engine.cc.stack.push_cont(old_cc);
    Ok(())
}

// (continuation - ),
// pargs = cmd.pargs if any else cc.stack.depth
// if pargs > cc.stack.depth {
//     StackOverflow
// }
// move_out = cc.stack[0..pargs]
// move_in  = if continuation.nargs < 0 {
//     move_out
// } else if continuation.nargs > pargs {
//     StackOverflow
// } else {
//     move_out[0..pargs + continuation.nargs - continuation.stack.depth]
// }
// cc.stack -= move_out, continuation.stack += move_in,
// if cmd.nargs exists {
//     if cmd.nargs < 0 {
//         cc.nargs = -1
//     } else {
//         cc.nargs = cc.stack.depth + cmd.nargs
//     }
// } else if cmd.rargs exists {
//     cc.nargs = cc.stack.depth + cmd.rargs
// }
// if continuation.savelist[0].is_none {
//     cc.savelist[0] = c[0]
//     continuation.savelist[0] = cc
// }
// cc = continuation, c[*] = cc.savelist[*]
pub(super) fn callx(engine: &mut Engine, callee: usize, need_convert: bool) -> Status {
    let vars = engine.cmd.var_count();
    if vars < callee {
        fail!("callx {:X}", callee)
    } else if vars == callee {
        fetch_stack(engine, 1)?;
    } else if need_convert && engine.cmd.var(callee).as_cell().is_ok() {
        convert(engine, var!(callee), CONTINUATION, CELL)?;
    }
    pop_all(engine, var!(callee))?;
    let has_c0 = engine.cmd.var(callee).as_continuation()?.savelist.get(0).is_some();
    if has_c0 {
        swap(engine, var!(callee), CC)?;
    } else {
        swap(engine, ctrl!(0), savelist!(CC, 0))?;
        swap(engine, var!(callee), CC)?;
        swap(engine, var!(callee), ctrl!(0))?;
    }
    apply_savelist(engine)?;
    if let Some(nargs) = engine.cmd.nargs_raw() {
        continuation_mut_by_address!(engine, ctrl!(0))?.nargs = nargs
    } else if let Some(rargs) = engine.cmd.rargs_raw() {
        continuation_mut_by_address!(engine, ctrl!(0))?.nargs = rargs as isize
    } else {
        continuation_mut_by_address!(engine, ctrl!(0))?.nargs = -1;
    }
    Ok(())
}

type NRange = RangeInclusive<isize>;
type PRange = RangeInclusive<isize>;

fn fetch_nargs(engine: &mut Engine, idx: usize, nrange: NRange) -> Status {
    let nargs = engine.cmd.var(idx).as_integer_value(nrange)?;
    engine.cmd.params.push(InstructionParameter::Nargs(nargs));
    Ok(())
}

fn fetch_pargs(engine: &mut Engine, idx: usize, prange: PRange) -> Status {
    let pargs = engine.cmd.var(idx).as_integer_value(prange)?;
    if pargs >= 0 {
        engine.cmd.params.push(InstructionParameter::Pargs(pargs as usize));
    }
    Ok(())
}

fn fetch_nargs_pargs(engine: &mut Engine, nrange: NRange, prange: PRange) -> Status {
    fetch_nargs(engine, 0, nrange)?;
    fetch_pargs(engine, 1, prange)
}

// (continuation - ),
// move_out = cc.stack
// move_in  = if continuation.nargs < 0 {
//     move_out
// } else if continuation.nargs > cc.stack.depth {
//     StackOverflow
// } else {
//     move_out[0..cc.stack.depth + continuation.nargs - continuation.stack.depth]
// }
// cc.stack -= move_out, continuation.stack += move_in,
// tmp = cc, cc = continuation, c[*] = cc.savelist[*], cc.stack.push(slice(tmp))
fn jmpxdata(engine: &mut Engine) -> Status {
    pop_all(engine, var!(0))?;
    swap(engine, var!(0), CC)?;
    apply_savelist(engine)?;
    let slice = engine.cmd.var(0).as_continuation()?.code().clone();
    engine.cc.stack.push(StackItem::Slice(slice));
    Ok(())
}

// checks special case for REPEAT*, UNTIL*, WHILE*
// works as a continue, not as a break
pub(super) fn ret(engine: &mut Engine) -> Status {
    switch(engine, ctrl!(0))
}

fn retalt(engine: &mut Engine) -> Status {
    switch(engine, ctrl!(1))
}

// ( - ), if c[?].savelist[i].is_none() { c[?].savelist[i] = c[i] }
fn save(engine: &mut Engine, index: usize) -> Status {
    let creg = engine.cmd.creg();
    let skip = match engine.ctrls.get(index) {
        Some(c) => c.as_continuation()?.savelist.get(creg).is_some(),
        None => fail!(ExceptionCode::TypeCheckError),
    };
    if !skip {
        let v = engine.cmd.var_count() as u16;
        copy_to_var(engine, ctrl!(creg))?;
        swap(engine, var!(v), savelist!(ctrl!(index), creg))
    } else {
        Ok(())
    }
}

// (x1 ... xR y {R N} - continuation), y->continuation, continuation.stack.push(x1 ... xR)
fn setcont(engine: &mut Engine, v: usize, need_to_convert: bool) -> Status {
    fetch_stack(engine, v + 1)?; // fetch slice or continuation from stack and nargs/parags
    match v {
        0 => {}
        1 => fetch_nargs(engine, 0, -1..=255)?,
        2 => fetch_nargs_pargs(engine, -1..=255, 0..=255)?,
        _ => fail!(ExceptionCode::FatalError),
    }
    if need_to_convert {
        engine.cmd.var(v).as_slice()?;
    } else {
        engine.cmd.var(v).as_continuation()?;
    }
    if engine.cc.stack.depth() < engine.cmd.pargs() {
        fail!(ExceptionCode::StackUnderflow)
    } else if need_to_convert {
        convert(engine, var!(v as u16), CONTINUATION, SLICE)?
    }
    let pargs = engine.cmd.pargs();
    if pargs != 0 {
        pop_range(engine, 0..pargs, var!(v as u16))?
    }
    // update nargs
    let nargs = engine.cmd.nargs();
    if nargs >= 0 {
        engine.cmd.var_mut(v).as_continuation_mut()?.nargs = nargs;
    }
    // return continuation to stack
    engine.cc.stack.push(engine.cmd.pop_var()?);
    Ok(())
}

// switch to continuation from var!(0)
fn jmpx(engine: &mut Engine, need_convert: bool) -> Status {
    if need_convert && engine.cmd.var(0).as_cell().is_ok() {
        convert(engine, var!(0), CONTINUATION, CELL)?;
    }
    pop_all(engine, var!(0))?;
    swap(engine, var!(0), CC)?;
    apply_savelist_excluding_c0_c1(engine)
}

// (continuation - ),
// pargs = cmd.pargs if any else cc.stack.depth
// if pargs > cc.stack.depth {
//     StackOverflow
// }
// move_out = cc.stack[0..pargs]
// move_in  = if continuation.nargs < 0 {
//     move_out
// } else if continuation.nargs > pargs {
//     StackOverflow
// } else {
//     move_out[0..pargs + continuation.nargs - continuation.stack.depth]
// }
// cc.stack -= move_out, continuation.stack += move_in,
// cc = continuation, c[*] = cc.savelist[*]
pub(super) fn switch(engine: &mut Engine, continuation: u16) -> Status {
    pop_all(engine, continuation)?;
    swap(engine, continuation, CC)?;
    let drop_c0 = (continuation == ctrl!(0)) && engine.cc.savelist.get(0).is_none();
    let drop_c1 = (continuation == ctrl!(1)) && engine.cc.savelist.get(1).is_none();
    apply_savelist(engine)?;
    if drop_c0 {
        engine.ctrls.remove(0);
    }
    if drop_c1 {
        let cont = ContinuationData::with_type(ContinuationType::Quit(1));
        engine.ctrls.put(1, StackItem::continuation(cont))?;
    }
    Ok(())
}

pub(super) fn switch_to_c0(engine: &mut Engine) -> Status {
    pop_all(engine, ctrl!(0))?;
    let c0 = engine.ctrls.get_mut(0).ok_or(ExceptionCode::FatalError)?.as_continuation_mut()?;
    mem::swap(&mut engine.cc, c0);
    let drop_c0 = engine.cc.savelist.get(0).is_none();
    engine.ctrls.apply(&mut engine.cc.savelist);
    if drop_c0 {
        engine.ctrls.remove(0);
    }
    Ok(())
}

// Continuation related instructions ******************************************
// (c - ), execute C infinitely
pub(super) fn execute_again(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("AGAIN"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::AgainLoopBody(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    copy_to_var(engine, CC)?;
    swap(engine, savelist!(var!(1), 0), var!(2))?; // again.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // continuation.savelist[0] = again
    switch(engine, var!(0))
}

// Continuation related instructions ******************************************
// (c - ), execute C infinitely with break
pub(super) fn execute_again_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("AGAINBRK"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::AgainLoopBody(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    swap(engine, savelist!(CC, 1), ctrl!(1))?; // cc.savelist[1] = c[1]
    copy_to_var(engine, CC)?; // var[2] = cc
    copy_to_var(engine, var!(2))?; // var[3] = cc
    swap(engine, savelist!(var!(1), 0), var!(2))?; // again.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = again
    swap(engine, savelist!(var!(0), 1), var!(3))?; // body.savelist[1] = cc
    switch(engine, var!(0)) // jump to body
}

// ( - ), execute CC infinitely
pub(super) fn execute_againend(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("AGAINEND"))?;
    let body = engine.cc.code_mut().withdraw();
    let cont = ContinuationData::with_code(body.clone());
    engine.cmd.vars.push(StackItem::continuation(cont));
    let cont = ContinuationData::with_type(ContinuationType::AgainLoopBody(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    copy_to_var(engine, CC)?;
    swap(engine, savelist!(var!(1), 0), var!(2))?; // again.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // continuation.savelist[0] = again
    switch(engine, var!(0))
}

// ( - ), execute CC infinitely with break
pub(super) fn execute_againend_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("AGAINENDBRK"))?;
    let body = engine.cc.code_mut().withdraw();
    let cont = ContinuationData::with_code(body.clone());
    engine.cmd.vars.push(StackItem::continuation(cont));
    let cont = ContinuationData::with_type(ContinuationType::AgainLoopBody(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    copy_to_var(engine, CC)?; // var[2] = cc
    copy_to_var(engine, var!(2))?; // var[3] = cc
    swap(engine, savelist!(var!(1), 0), var!(2))?; // again.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = again
    swap(engine, savelist!(var!(0), 1), var!(3))?; // body.savelist[1] = cc
    switch(engine, var!(0)) // jump to body
}

// (continuation - ), continuation.savelist[0] = c[0], c[0] = continuation
pub(super) fn execute_atexit(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ATEXIT"))?;
    fetch_stack(engine, 1)?;
    swap(engine, var!(0), ctrl!(0))?;
    swap(engine, var!(0), savelist!(ctrl!(0), 0))
}

// (continuation - ), continuation.savelist[1] = c[1], c[1] = continuation
pub(super) fn execute_atexitalt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ATEXITALT"))?;
    fetch_stack(engine, 1)?;
    swap(engine, var!(0), ctrl!(1))?;
    swap(engine, var!(0), savelist!(ctrl!(1), 1))
}

// (slice - continuation)
pub(super) fn execute_bless(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLESS"))?;
    setcont(engine, 0, true)
}

// (x1 ... xR slice - continuation), continuation.stack.push(x1 ... xR)
pub(super) fn execute_blessargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("BLESSARGS").set_opts(InstructionOptions::ArgumentConstraints),
    )?;
    setcont(engine, 0, true)
}

// (x1 ... xR slice R N - continuation), continuation.stack.push(x1 ... xR)
pub(super) fn execute_blessva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLESSVARARGS"))?;
    setcont(engine, 2, true)
}

//(c - )
// c'= continuation {PUSHINT -1}, c'[0] = cc
// c''= continuation {PUSHINT 0}, c''[0] = cc
//c[0] = c', c[1] = c''
//execute c
pub(super) fn execute_booleval(engine: &mut Engine) -> Status {
    let mut old_cc_idx = ctrl!(0);
    engine.load_instruction(Instruction::new("BOOLEVAL"))?;
    fetch_stack(engine, 1)?;
    engine.cmd.var(0).as_continuation()?;
    let cont = ContinuationData::with_type(ContinuationType::PushInt(-1));
    engine.cmd.push_var(StackItem::continuation(cont));
    let cont = ContinuationData::with_type(ContinuationType::PushInt(0));
    engine.cmd.push_var(StackItem::continuation(cont));
    callx(engine, 0, false)?;
    let has_save_c0 = !engine.cc.can_put_to_savelist_once(0);
    if has_save_c0 {
        old_cc_idx = var!(0)
    };
    copy_to_var(engine, old_cc_idx)?;
    swap(engine, savelist!(var!(1), 0), old_cc_idx)?;
    swap(engine, savelist!(var!(2), 0), var!(3))?;
    swap(engine, ctrl!(0), var!(1))?;
    swap(engine, ctrl!(1), var!(2))
}

// n ( - n), calls the continuation in c3
// approximately equivalent to PUSHINT n; PUSH c3; EXECUTE
fn execute_call(engine: &mut Engine, name: &'static str, range: Range<isize>, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name).set_opts(InstructionOptions::Integer(range)))?;
    let n = engine.cmd.integer();
    if how == PREPARE {
        copy_to_var(engine, ctrl!(3))?;
        engine.cc.stack.push(int!(n));
        engine.cc.stack.push(engine.cmd.pop_var()?);
        Ok(())
    } else {
        engine.cc.stack.push(int!(n));
        copy_to_var(engine, ctrl!(3))?;
        match how {
            SWITCH => switch(engine, var!(0)),
            CALLX => callx(engine, 0, false),
            _ => fail!("how: 0x{:X}", how),
        }
    }
}

// 0 =< n =< 255
pub(super) fn execute_call_short(engine: &mut Engine) -> Status {
    execute_call(engine, "CALLDICT", 0..256, CALLX)
}
// 0 =< n < (2 ^ 14)
pub(super) fn execute_call_long(engine: &mut Engine) -> Status {
    execute_call(engine, "CALLDICT", 0..16384, CALLX)
}
// 0 =< n < (2 ^ 14)
pub(super) fn execute_jmp(engine: &mut Engine) -> Status {
    execute_call(engine, "JMP", 0..16384, SWITCH)
}
// 0 =< n < (2 ^ 14)
pub(super) fn execute_prepare(engine: &mut Engine) -> Status {
    execute_call(engine, "PREPARE", 0..16384, PREPARE)
}

// (continuation - ), callcc pattern
pub(super) fn execute_callcc(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CALLCC"))?;
    callcc(engine, 0)
}

// (continuation - ), callcc pattern
pub(super) fn execute_callccargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("CALLCCARGS").set_opts(InstructionOptions::ArgumentConstraints),
    )?;
    callcc(engine, 0)
}

// (continuation pargs rargs - ), callcc pattern
pub(super) fn execute_callccva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CALLCCVARARGS"))?;
    fetch_stack(engine, 3)?;
    fetch_nargs_pargs(engine, -1..=255, -1..=255)?;
    callcc(engine, 2)
}

// equivalent to PUSHREFCONT; CALLX
// e.g. remove first reference from CC and then call it
pub(super) fn execute_callref(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CALLREF"))?;
    fetch_reference(engine, CC)?;
    convert(engine, var!(0), CONTINUATION, CELL)?;
    callx(engine, 0, false)
}

// (continuation - ), callx pattern
pub(super) fn execute_callx(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CALLX"))?;
    callx(engine, 0, false)
}

// (continuation - ), callx pattern
pub(super) fn execute_callxargs(engine: &mut Engine) -> Status {
    let cmd = engine.last_cmd();
    engine.load_instruction(Instruction::new("CALLXARGS").set_opts(if cmd == 0xDA {
        InstructionOptions::ArgumentAndReturnConstraints
    } else {
        InstructionOptions::Pargs(0..16)
    }))?;
    callx(engine, 0, false)
}

// (continuation pargs rargs - ), callx pattern
pub(super) fn execute_callxva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CALLXVARARGS"))?;
    fetch_stack(engine, 3)?;
    fetch_nargs_pargs(engine, -1..=254, -1..=254)?;
    callx(engine, 2, false)
}

// (continuation1 continuation2 - continuation1), continuation1.savelist[0] = continuation2
pub(super) fn execute_compos(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("COMPOS"))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_continuation()?;
    engine.cmd.var(1).as_continuation()?;
    swap(engine, var!(0), savelist!(var!(1), 0))?;
    engine.cc.stack.push(engine.cmd.pop_var()?);
    Ok(())
}

// (continuation1 continuation2 - continuation1), continuation1.savelist[1] = continuation2
pub(super) fn execute_composalt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("COMPOSALT"))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_continuation()?;
    engine.cmd.var(1).as_continuation()?;
    swap(engine, var!(0), savelist!(var!(1), 1))?;
    engine.cc.stack.push(engine.cmd.pop_var()?);
    Ok(())
}

// (continuation1 continuation2 - continuation1),
// continuation1.savelist[0] = continuation2, continuation1.savelist[1] = continuation2
pub(super) fn execute_composboth(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("COMPOSBOTH"))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_continuation()?;
    engine.cmd.var(1).as_continuation()?;
    copy_to_var(engine, var!(0))?;
    swap(engine, var!(0), savelist!(var!(1), 0))?;
    swap(engine, var!(2), savelist!(var!(1), 1))?;
    engine.cc.stack.push(engine.cmd.vars.remove(1));
    Ok(())
}

// (f x y - ), x f != 0 else y
pub(super) fn execute_condsel(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CONDSEL"))?;
    fetch_stack(engine, 3)?;
    if !engine.cmd.var(2).as_bool()? {
        engine.cc.stack.push(engine.cmd.vars.remove(0));
    } else {
        engine.cc.stack.push(engine.cmd.vars.remove(1));
    }
    Ok(())
}

// (f x y - ), x f != 0 else y, throws exception, if types mismatch
pub(super) fn execute_condselchk(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CONDSELCHK"))?;
    fetch_stack(engine, 3)?;
    if mem::discriminant(engine.cmd.var(0)) != mem::discriminant(engine.cmd.var(1)) {
        fail!(ExceptionCode::TypeCheckError)
    }
    if !engine.cmd.var(2).as_bool()? {
        engine.cc.stack.push(engine.cmd.vars.remove(0));
    } else {
        engine.cc.stack.push(engine.cmd.vars.remove(1));
    }
    Ok(())
}

const CALL: u8 = 0x00; // call cont
const JMP: u8 = 0x01; // jump to cont
const RET: u8 = 0x04; // ret to c0
const ALT: u8 = 0x08; // ret to c1
const REF: u8 = 0x10; // use refslice as cont
const REF2: u8 = 0x02; // use refslice as second cont
const INV: u8 = 0x20; // condition not
const ELSE: u8 = 0x40; // IFELSE
const THROW: u8 = 0x80; // checks if condition is NaN then throw IntegerOverflow

fn execute_if_mask(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut params = 2;
    if how.bit(ELSE) {
        params += 1;
    }
    if how.bit(REF) {
        params -= 1;
    }
    if how.bit(REF2) {
        params -= 1;
    }
    if how.bit(RET) {
        params -= 1;
    }

    engine.load_instruction(Instruction::new(name))?;
    if how.bit(REF) {
        fetch_reference(engine, CC)?
    }
    if how.bit(REF2) {
        fetch_reference(engine, CC)?
    }
    fetch_stack(engine, params)?;
    if how.bit(THROW) && engine.cmd.last_var()?.as_integer()?.is_nan() {
        fail!(ExceptionCode::IntegerOverflow)
    }
    match engine.cmd.last_var()?.as_bool()? ^ how.bit(INV) {
        false if how.bit(ELSE) => {
            if !how.bit(REF) {
                engine.cmd.var(0).as_continuation()?;
            }
            callx(engine, 1, how.bit(REF2))
        }
        false => Ok(()),
        true if how.bit(ELSE) => {
            if !how.bit(REF2) {
                engine.cmd.var(1).as_continuation()?;
            }
            callx(engine, 0, how.bit(REF))
        }
        true if how.bit(JMP) => jmpx(engine, how.bit(REF)),
        true if how.bit(ALT) => retalt(engine),
        true if how.bit(RET) => ret(engine),
        true => callx(engine, 0, how.bit(REF)),
    }
}

// (condition continuation - ): callx if condition != 0
pub(super) fn execute_if(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IF", CALL)
}

// (condition continuation1 continuation2 - ): if condition != 0 callx continuation1 else callx continuation2
pub(super) fn execute_ifelse(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFELSE", CALL | ELSE | INV)
}

// (condition continuation - ): equivalent to PUSHREFCONT; IFELSE
pub(super) fn execute_ifelseref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFELSEREF", CALL | ELSE | INV | REF)
}

// (condition continuation - ): switch if condition != 0
pub(super) fn execute_ifjmp(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFJMP", JMP)
}

// (condition continuation - ): callx if condition == 0
pub(super) fn execute_ifnot(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOT", CALL | INV)
}

// (condition continuation - ): switch if condition == 0
pub(super) fn execute_ifnotjmp(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOTJMP", JMP | INV)
}

// (condition - Continuation): pushrefcont if condition == 0
pub(super) fn execute_ifnotref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOTREF", CALL | INV | REF)
}

// (condition - ): switch to continuation from references[0] if condition != 0
pub(super) fn execute_ifjmpref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFJMPREF", JMP | REF)
}

// (condition - ): switch to continuation from references[0] if condition == 0
pub(super) fn execute_ifnotjmpref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOTJMPREF", JMP | INV | REF)
}

// (condition - ): switch if condition == 0
pub(super) fn execute_ifnotret(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOTRET", RET | INV)
}

// (condition - Continuation): pushrefcont if condition != 0
pub(super) fn execute_ifref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFREF", CALL | REF)
}

// (condition continuation - ): equivalent to PUSHREFCONT; SWAP; IFELSE
pub(super) fn execute_ifrefelse(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFREFELSE", CALL | ELSE | REF)
}

// (condition - ): equivalent to PUSHREFCONT; PUSHREFCONT; IFELSE
pub(super) fn execute_ifrefelseref(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFREFELSEREF", CALL | ELSE | REF | REF2)
}

// (condition - ): switch if condition != 0
pub(super) fn execute_ifret(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFRET", RET | THROW)
}

// (f - ), RETALT f != 0
pub(super) fn execute_ifretalt(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFRETALT", RET | ALT)
}

// (f - ), RETALT f == 0
pub(super) fn execute_ifnotretalt(engine: &mut Engine) -> Status {
    execute_if_mask(engine, "IFNOTRETALT", RET | ALT | INV)
}

// c[0] <-> c[1]
pub(super) fn execute_invert(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("INVERT"))?;
    swap(engine, ctrl!(0), ctrl!(1))
}

pub(super) fn execute_jmpref(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("JMPREF"))?;
    fetch_reference(engine, CC)?;
    jmpx(engine, true)
}

pub(super) fn execute_jmprefdata(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("JMPREFDATA"))?;
    fetch_reference(engine, CC)?;
    convert(engine, var!(0), CONTINUATION, CELL)?;
    jmpxdata(engine)
}

// (continuation - ), switch pattern
pub(super) fn execute_jmpx(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("JMPX"))?;
    fetch_stack(engine, 1)?;
    jmpx(engine, false)
}

fn execute_ifbit_mask(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name).set_opts(InstructionOptions::Integer(0..32)))?;
    if how.bit(REF) {
        fetch_reference(engine, CC)?;
    } else {
        fetch_stack(engine, 1)?;
        engine.cmd.var(0).as_continuation()?;
    }
    if engine.cc.stack.depth() < 1 {
        fail!(ExceptionCode::StackUnderflow);
    }
    let is_zero = {
        let x = engine.cc.stack.get(0)?.as_integer()?;
        let nbit = engine.cmd.integer() as u32;
        let test_bit_mask = IntegerData::from_u32(1 << nbit);
        x.and::<Signaling>(&test_bit_mask)?.is_zero()
    };
    if is_zero ^ how.bit(INV) {
        Ok(())
    } else {
        jmpx(engine, how.bit(REF))
    }
}

// (x continuation - x), switch if n's bit of x is set
pub(super) fn execute_ifbitjmp(engine: &mut Engine) -> Status {
    execute_ifbit_mask(engine, "IFBITJMP", 0)
}

// (x continuation - x), switch if n's bit of x is not set
pub(super) fn execute_ifnbitjmp(engine: &mut Engine) -> Status {
    execute_ifbit_mask(engine, "IFNBITJMP", INV)
}

// (x - x), switch pattern if n'th bit is set
pub(super) fn execute_ifbitjmpref(engine: &mut Engine) -> Status {
    execute_ifbit_mask(engine, "IFBITJMPREF", REF)
}

// (x - x), switch pattern if n'th bit is not set
pub(super) fn execute_ifnbitjmpref(engine: &mut Engine) -> Status {
    execute_ifbit_mask(engine, "IFNBITJMPREF", REF | INV)
}

// (continuation - ), continuation.nargs = cmd.pargs, then switch pattern
pub(super) fn execute_jmpxargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("JMPXARGS").set_opts(InstructionOptions::Pargs(0..16)),
    )?;
    fetch_stack(engine, 1)?;
    switch(engine, var!(0))
}

// (continuation p - ), continuation.nargs = cmd.pargs, then switch pattern
pub(super) fn execute_jmpxva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("JMPXVARARGS"))?;
    fetch_stack(engine, 2)?;
    fetch_pargs(engine, 0, -1..=254)?;
    switch(engine, var!(1))
}

// (integer_repeat_count body_continuation - )
// body.savelist[0] = cc
// cc.savelist[0] = c[0]
pub(super) fn execute_repeat(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("REPEAT"))?;
    fetch_stack(engine, 2)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let counter = engine.cmd.var(1).as_integer_value(-0x80000000..=0x7FFFFFFF)?;
    if counter <= 0 {
        Ok(())
    } else {
        let cont = ContinuationData::with_type(ContinuationType::RepeatLoopBody(body, counter));
        engine.cmd.vars.push(StackItem::continuation(cont));
        swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
        copy_to_var(engine, CC)?;
        swap(engine, savelist!(var!(2), 0), var!(3))?; // ec_repeat.savelist[0] = cc
        swap(engine, savelist!(var!(0), 0), var!(2))?; // body.savelist[0] = ec_repeat
        switch(engine, var!(0))
    }
}

// (integer_repeat_count body_continuation - )
// cc.savelist[0] = c[0]
// ec_repeat.savelist[0] = cc
// body.savelist[0] = ec_repeat
// body.savelist[1] = cc
pub(super) fn execute_repeat_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("REPEATBRK"))?;
    fetch_stack(engine, 2)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let counter = engine.cmd.var(1).as_integer_value(-0x80000000..=0x7FFFFFFF)?;
    if counter <= 0 {
        Ok(())
    } else {
        let cont = ContinuationData::with_type(ContinuationType::RepeatLoopBody(body, counter));
        engine.cmd.vars.push(StackItem::continuation(cont));
        swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
        swap(engine, savelist!(CC, 1), ctrl!(1))?; // cc.savelist[1] = c[1]
        copy_to_var(engine, CC)?;
        copy_to_var(engine, var!(3))?;
        swap(engine, savelist!(var!(2), 0), var!(3))?; // ec_repeat.savelist[0] = cc
        swap(engine, savelist!(var!(0), 0), var!(2))?; // body.savelist[0] = ec_repeat
        swap(engine, savelist!(var!(0), 1), var!(4))?; // body.savelist[1] = cc
        switch(engine, var!(0)) // jump to body
    }
}

// (integer_repeat_count - )
// body.savelist[0] = cc
// cc.savelist[0] = c[0]
pub(super) fn execute_repeatend(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("REPEATEND"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cc.code().clone();
    let counter = engine.cmd.var(0).as_integer_value(-0x80000000..=0x7FFFFFFF)?;
    if counter <= 0 {
        ret(engine)
    } else {
        let cont = ContinuationData::with_code(body.clone());
        engine.cmd.vars.push(StackItem::continuation(cont));
        let cont = ContinuationData::with_type(ContinuationType::RepeatLoopBody(body, counter));
        engine.cmd.vars.push(StackItem::continuation(cont));
        swap(engine, savelist!(var!(2), 0), ctrl!(0))?; // ec_repeat.savelist[0] = c[0]
        swap(engine, savelist!(var!(1), 0), var!(2))?; // body.savelist[0] = ec_repeat
        switch(engine, var!(1))
    }
}

// (integer_repeat_count - )
// ec_repeat.savelist[0] = c[0]
// body.savelist[0] = ec_repeat
// body.savelist[1] = c[0]
pub(super) fn execute_repeatend_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("REPEATENDBRK"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cc.code().clone();
    let counter = engine.cmd.var(0).as_integer_value(-0x80000000..=0x7FFFFFFF)?;
    if counter <= 0 {
        ret(engine)
    } else {
        let cont = ContinuationData::with_code(body.clone());
        engine.cmd.vars.push(StackItem::continuation(cont));
        let cont = ContinuationData::with_type(ContinuationType::RepeatLoopBody(body, counter));
        engine.cmd.vars.push(StackItem::continuation(cont));
        copy_to_var(engine, ctrl!(0))?;
        swap(engine, savelist!(var!(2), 0), ctrl!(0))?; // ec_repeat.savelist[0] = c[0]
        swap(engine, savelist!(var!(1), 0), var!(2))?; // body.savelist[0] = ec_repeat
        swap(engine, savelist!(var!(1), 1), var!(3))?; // body.savelist[1] = c[0]
        switch(engine, var!(1))
    }
}

// c[0].stack = cc.stack, cc.stack = ()
// cc = continuation, c[2..] = cc.savelist[2..]
// (continuation - ), var[0] = cc.stack.pop(), then jmpxdata pattern
pub(super) fn execute_jmpxdata(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("JMPXDATA"))?;
    fetch_stack(engine, 1)?;
    jmpxdata(engine)
}

// switch to c[0]
pub(super) fn execute_ret(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RET"))?;
    ret(engine)
}

// switch to c[1]
pub(super) fn execute_retalt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RETALT"))?;
    retalt(engine)
}

// switch to c[0] with pargs
pub(super) fn execute_retargs(engine: &mut Engine) -> Status {
    engine
        .load_instruction(Instruction::new("RETARGS").set_opts(InstructionOptions::Pargs(0..16)))?;
    switch(engine, ctrl!(0))
}

// (p - ) switch to c[0] with p params
pub(super) fn execute_retva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RETVARARGS"))?;
    fetch_stack(engine, 1)?;
    fetch_pargs(engine, 0, -1..=254)?;
    switch(engine, ctrl!(0))
}

// (condition - ), if condition != 0 then RET else RETALT
pub(super) fn execute_retbool(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RETBOOL"))?;
    fetch_stack(engine, 1)?;
    match engine.cmd.var(0).as_bool()? {
        false => retalt(engine),
        _ => ret(engine),
    }
}

// var[0] = c[0], then jmpxdata pattern
pub(super) fn execute_retdata(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RETDATA"))?;
    let cont = ContinuationData::with_type(ContinuationType::Quit(
        ExceptionCode::NormalTermination as i32,
    ));
    engine.cmd.push_var(StackItem::continuation(cont));
    swap(engine, ctrl!(0), var!(0))?;
    jmpxdata(engine)
}

// (xN ... xN-p xN-p-1 ... x0 - xN-p-1 ... x0), c0.stack.push(xN ... xN-p)
pub(super) fn execute_returnargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("RETURNARGS").set_opts(InstructionOptions::Rargs(0..16)),
    )?;
    if engine.cc.stack.depth() < engine.cmd.rargs() {
        fail!(ExceptionCode::StackUnderflow)
    } else {
        let drop = engine.cmd.rargs()..engine.cc.stack.depth();
        pop_range(engine, drop, ctrl!(0))
    }
}

// (xN ... xN-p xN-p-1 ... x0 p - xN-p-1 ... x0), c0.stack.push(xN ... xN-p)
pub(super) fn execute_returnva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RETURNVARARGS"))?;
    fetch_stack(engine, 1)?;
    let rargs = engine.cmd.var(0).as_integer_value(0..=255)?;
    if engine.cc.stack.depth() < rargs {
        fail!(ExceptionCode::StackUnderflow)
    } else {
        let drop = rargs..engine.cc.stack.depth();
        pop_range(engine, drop, ctrl!(0))
    }
}

// ( - ), c[1] = c[0]
pub(super) fn execute_samealt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SAMEALT"))?;
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, ctrl!(1), var!(0))
}

// ( - ), c[0].savelist[1] = c[1], c[1] = c[0]
pub(super) fn execute_samealt_save(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SAMEALTSAVE"))?;
    swap(engine, savelist!(ctrl!(0), 1), ctrl!(1))?;
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, ctrl!(1), var!(0))
}

// ( - ), if c[0].savelist[i].is_none() { c[0].savelist[i] = c[i] }
pub(super) fn execute_save(engine: &mut Engine) -> Status {
    engine
        .load_instruction(Instruction::new("SAVE").set_opts(InstructionOptions::ControlRegister))?;
    save(engine, 0)
}

// ( - ), if c[1].savelist[i].is_none() { c[1].savelist[i] = c[i] }
pub(super) fn execute_savealt(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SAVEALT").set_opts(InstructionOptions::ControlRegister),
    )?;
    save(engine, 1)
}

// ( - ), if c[0].savelist[i].is_none() { c[0].savelist[i] = c[i] }
// if c[1].savelist[i].is_none() { c[1].savelist[i] = c[i] }
pub(super) fn execute_saveboth(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SAVEBOTH").set_opts(InstructionOptions::ControlRegister),
    )?;
    if engine.ctrl(0).is_ok() || engine.ctrl(1).is_ok() {
        fail!(ExceptionCode::TypeCheckError)
    } else {
        save(engine, 0)?;
    }
    save(engine, 1)
}

// (x - ), c1.savelist[i] = x
pub(super) fn execute_setaltctr(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SETALTCTR").set_opts(InstructionOptions::ControlRegister),
    )?;
    fetch_stack(engine, 1)?;
    let creg = engine.cmd.creg();
    swap(engine, var!(0), savelist!(ctrl!(1), creg))
}

// (x1 ... xR continuation - continuation), continuation.stack.push(x1 ... xR)
pub(super) fn execute_setcontargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SETCONTARGS").set_opts(InstructionOptions::ArgumentConstraints),
    )?;
    setcont(engine, 0, false)
}

// (x1 ... xR continuation R N - continuation), continuation.stack.push(x1 ... xR)
pub(super) fn execute_setcontva(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETCONTVARARGS"))?;
    setcont(engine, 2, false)
}

// (continuation n - continuation)
pub(super) fn execute_setnumvarargs(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETNUMVARARGS"))?;
    setcont(engine, 1, false)
}

// (x continuation - continuation), continuation.savelist[i] = x
pub(super) fn execute_setcontctr(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SETCONTCTR").set_opts(InstructionOptions::ControlRegister),
    )?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_continuation()?;
    let creg = engine.cmd.creg();
    swap(engine, var!(1), savelist!(var!(0), creg))?;
    engine.cc.stack.push(engine.cmd.vars.remove(0));
    Ok(())
}

// (x continuation i - continuation), continuation.savelist[i] = x
pub(super) fn execute_setcontctrx(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETCONTCTRX"))?;
    fetch_stack(engine, 3)?;
    let creg = engine.cmd.var(0).as_integer_value(0..=255)?;
    if !SaveList::REGS.contains(&(creg as usize)) {
        fail!(ExceptionCode::RangeCheckError)
    }
    engine.cmd.var(1).as_continuation()?;
    swap(engine, var!(2), savelist!(var!(1), creg))?;
    engine.cc.stack.push(engine.cmd.vars.remove(1));
    Ok(())
}

fn setcontctrmany_helper(engine: &mut Engine, x: usize) -> Status {
    if x & (1 << 6) != 0 {
        fail!(ExceptionCode::RangeCheckError, "no control register for c6")
    }
    let mut cont = engine.cmd.last_var_mut()?.as_continuation_mut()?.withdraw();
    for i in 0..8usize {
        let mask = 1 << i;
        if x & mask != 0 {
            if let Some(var) = engine.ctrls.get(i).cloned() {
                cont.put_to_savelist(i, var)?;
            }
        }
    }
    engine.cc.stack.push_cont(cont);
    Ok(())
}

// (continuation - continuation), continuation.savelist[i] = c[i]
pub(super) fn execute_setcontctrmany(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SETCONTCTRMANY").set_opts(InstructionOptions::Length(0..256)),
    )?;
    fetch_stack(engine, 1)?;
    setcontctrmany_helper(engine, engine.cmd.length())
}

// (continuation x - continuation), continuation.savelist[i] = c[i]
pub(super) fn execute_setcontctrmanyx(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETCONTCTRMANYX"))?;
    fetch_stack(engine, 2)?;
    let x = engine.cmd.var(0).as_integer_value(0..=255)?;
    setcontctrmany_helper(engine, x)
}

// (continuation - ), continuation.savelist[0] = c[0], continuation.savelist[1] = c[1],
// c[1] = continuation
pub(super) fn execute_setexitalt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETEXITALT"))?;
    fetch_stack(engine, 1)?;
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, var!(1), savelist!(var!(0), 0))?;
    if engine.cc.savelist.get(1).is_some() {
        copy_to_var(engine, ctrl!(1))?;
        swap(engine, var!(2), savelist!(var!(0), 1))?;
    }
    swap(engine, var!(0), ctrl!(1))
}

// (x - ), c0.savelist[i] = x
pub(super) fn execute_setretctr(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("SETRETCTR").set_opts(InstructionOptions::ControlRegister),
    )?;
    fetch_stack(engine, 1)?;
    let creg = engine.cmd.creg();
    swap(engine, var!(0), savelist!(ctrl!(0), creg))
}

// (continuation - continuation), continuation.savelist[0] = c[0]
pub(super) fn execute_thenret(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("THENRET"))?;
    fetch_stack(engine, 1)?;
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, savelist!(var!(0), 0), var!(1))?;
    engine.cc.stack.push(engine.cmd.vars.remove(0));
    Ok(())
}

// (continuation - continuation), continuation.savelist[0] = c[1]
pub(super) fn execute_thenretalt(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("THENRETALT"))?;
    fetch_stack(engine, 1)?;
    copy_to_var(engine, ctrl!(1))?;
    swap(engine, savelist!(var!(0), 0), var!(1))?;
    engine.cc.stack.push(engine.cmd.vars.remove(0));
    Ok(())
}

// (body - )
// cc.savelist[0] = c[0]
// condition.savelist[0] = cc
// body.savelist[0] = condition
// switch to body
pub(super) fn execute_until(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("UNTIL"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::UntilLoopCondition(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    copy_to_var(engine, CC)?;
    swap(engine, savelist!(var!(1), 0), var!(2))?; // ec_until.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = ec_until
    switch(engine, var!(0))
}

// (body - )
// cc.savelist[0] = c[0]
// ec_until.savelist[0] = cc
// body.savelist[0] = ec_until
// body.savelist[1] = cc
// switch to body
pub(super) fn execute_until_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("UNTILBRK"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::UntilLoopCondition(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    swap(engine, savelist!(CC, 1), ctrl!(1))?; // cc.savelist[1] = c[1]
    copy_to_var(engine, CC)?;
    copy_to_var(engine, var!(2))?;
    swap(engine, savelist!(var!(1), 0), var!(2))?; // ec_until.savelist[0] = cc
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = ec_until
    swap(engine, savelist!(var!(0), 1), var!(3))?; // body.savelist[1] = cc
    switch(engine, var!(0))
}

// cc is body
// condition.savelist[0] = c[0]
// body.savelist[0] = condition
// switch to body
pub(super) fn execute_untilend(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("UNTILEND"))?;
    let body = engine.cc.code_mut().withdraw();
    let cont = ContinuationData::with_code(body.clone());
    engine.cmd.vars.push(StackItem::continuation(cont));
    let cont = ContinuationData::with_type(ContinuationType::UntilLoopCondition(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(var!(1), 0), ctrl!(0))?; // ec_until.savelist[0] = c[0]
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = ec_until
    switch(engine, var!(0))
}

// cc is body
// ec_until.savelist[0] = c[0]
// body.savelist[0] = ec_until
// body.savelist[1] = c[0]
// switch to body
pub(super) fn execute_untilend_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("UNTILENDBRK"))?;
    let body = engine.cc.code_mut().withdraw();
    let cont = ContinuationData::with_code(body.clone());
    engine.cmd.vars.push(StackItem::continuation(cont));
    let cont = ContinuationData::with_type(ContinuationType::UntilLoopCondition(body));
    engine.cmd.vars.push(StackItem::continuation(cont));
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, savelist!(var!(1), 0), ctrl!(0))?; // ec_until.savelist[0] = c[0]
    swap(engine, savelist!(var!(0), 0), var!(1))?; // body.savelist[0] = ec_until
    swap(engine, savelist!(var!(0), 1), var!(2))?; // body.savelist[1] = c[0]
    switch(engine, var!(0))
}
// .set(0x18, execute_while_break)
// .set(0x19, execute_whileend_break)

// (condition body - )
// cc.savelist[0] = c[0]
// ec_while.savelist[0] = cc
// condition.savelist[0] = ec_while
// switch to condition
pub(super) fn execute_while(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("WHILE"))?;
    fetch_stack(engine, 2)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cond = engine.cmd.var(1).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::WhileLoopCondition(body, cond));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    copy_to_var(engine, CC)?;
    swap(engine, savelist!(var!(2), 0), var!(3))?; // ec_while.savelist[0] = cc
    swap(engine, savelist!(var!(1), 0), var!(2))?; // condition.savelist[0] = ec_while
    switch(engine, var!(1))
}

// (condition body - )
// cc.savelist[0] = c[0]
// ec_while.savelist[0] = cc
// condition.savelist[0] = ec_while
// condition.savelist[1] = cc
// switch to condition
pub(super) fn execute_while_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("WHILEBRK"))?;
    fetch_stack(engine, 2)?;
    let body = engine.cmd.var(0).as_continuation()?.code().clone();
    let cond = engine.cmd.var(1).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::WhileLoopCondition(body, cond));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(CC, 0), ctrl!(0))?; // cc.savelist[0] = c[0]
    swap(engine, savelist!(CC, 1), ctrl!(1))?; // cc.savelist[1] = c[1]
    copy_to_var(engine, CC)?;
    copy_to_var(engine, var!(3))?;
    copy_to_var(engine, var!(3))?;
    swap(engine, savelist!(var!(2), 0), var!(3))?; // ec_while.savelist[0] = cc
    swap(engine, savelist!(var!(1), 0), var!(2))?; // condition.savelist[0] = ec_while
    swap(engine, savelist!(var!(1), 1), var!(4))?; // condition.savelist[1] = cc
    swap(engine, savelist!(var!(0), 1), var!(5))?; // body.savelist[1] = cc
    switch(engine, var!(1))
}

// cc is body
// (condition - )
// condition.savelist[0] = c[0]
// ec_while.savelist[0] = condition
// switch to condition
pub(super) fn execute_whileend(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("WHILEEND"))?;
    fetch_stack(engine, 1)?;
    let body = engine.cc.code_mut().withdraw();
    let cond = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::WhileLoopCondition(body, cond));
    engine.cmd.vars.push(StackItem::continuation(cont));
    swap(engine, savelist!(var!(1), 0), ctrl!(0))?; // ec_while.savelist[0] = c[0]
    swap(engine, savelist!(var!(0), 0), var!(1))?; // condition.savelist[0] = ec_while
    switch(engine, var!(0))
}

// cc is body
// (condition - )
// condition.savelist[0] = c[0]
// ec_while.savelist[0] = condition
// switch to condition
pub(super) fn execute_whileend_break(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("WHILEENDBRK"))?;
    fetch_stack(engine, 1)?;
    pop_all(engine, var!(0))?; // move stack to cond check
    let body = engine.cc.code_mut().withdraw();
    let cond = engine.cmd.var(0).as_continuation()?.code().clone();
    let cont = ContinuationData::with_type(ContinuationType::WhileLoopCondition(body, cond));
    engine.cmd.vars.push(StackItem::continuation(cont));
    copy_to_var(engine, ctrl!(0))?;
    swap(engine, savelist!(var!(1), 0), ctrl!(0))?; // ec_while.savelist[0] = c[0]
    swap(engine, savelist!(var!(0), 0), var!(1))?; // condition.savelist[0] = ec_while
    swap(engine, savelist!(var!(0), 1), var!(2))?; // condition.savelist[1] = ec_while
    switch(engine, var!(0))
}

fn run_vm(engine: &mut Engine, mode: isize) -> Status {
    debug_assert!(engine.block_version() >= 10, "But block version is {}", engine.block_version());
    if mode >= 512 {
        fail!(ExceptionCode::RangeCheckError, "invalid flags {mode:#x}");
    }
    engine.try_use_gas(Gas::runvm_gas_price())?;
    let mut gas_max = if mode.bit(64) {
        engine.cc.stack.pop()?.as_integer_value(0..=Gas::infinity())?
    } else {
        Gas::infinity()
    };
    let gas_limit = if mode.bit(8) {
        engine.cc.stack.pop()?.as_integer_value(0..=Gas::infinity())?
    } else {
        Gas::infinity()
    };
    gas_max = if mode.bit(64) { gas_max.max(gas_limit) } else { gas_limit };
    let c7 = if mode.bit(16) { engine.cc.stack.pop()? } else { StackItem::tuple(Vec::new()) };
    c7.as_tuple()?;
    let data =
        if mode.bit(4) { engine.cc.stack.pop()? } else { StackItem::cell(Default::default()) };
    data.as_cell()?;
    let ret_vals =
        if mode.bit(256) { engine.cc.stack.pop()?.as_integer_value(0..=1i32 << 30)? } else { -1 };
    let code = engine.cc.stack.pop()?.as_slice()?.clone();
    let stack_size = engine.cc.stack.pop()?.as_integer_value(0..=engine.cc.stack.depth())?;
    engine.try_use_gas(Gas::stack_price(stack_size))?;
    fetch_stack(engine, stack_size)?;

    let mut stack = engine.cmd.withdraw_vars();
    stack.reverse();
    let stack = Stack::with_storage(stack);

    let params = RunChildVm {
        code,
        c7,
        data,
        stack,
        gas_max,
        gas_limit,
        push_0: mode.mask(3) == 3,
        same_c3: mode.bit(1),
        return_data: mode.bit(4),
        return_actions: mode.bit(32),
        return_gas: mode.bit(8),
        isolate_gas: mode.bit(128),
        ret_vals,
    };
    log::debug!(
        target: "tvm",
        "Child VM run with mode: {mode:#b}, params: {params:?}\n"
    );
    engine.run_child_vm(params)?;
    Ok(())
}

pub(super) fn execute_runvm(engine: &mut Engine) -> Status {
    let opts = InstructionOptions::Integer(0..0x1000);
    engine.load_instruction(Instruction::new("RUNVM").set_opts(opts))?;
    let mode = engine.cmd.integer();
    run_vm(engine, mode)
}

pub(super) fn execute_runvmx(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RUNVMX"))?;
    let mode = engine.cc.stack.pop()?.as_integer_value(0..=0xFFF)?;
    run_vm(engine, mode)
}
