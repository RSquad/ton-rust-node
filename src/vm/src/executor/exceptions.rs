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
    error::TvmError,
    executor::{
        continuation::callx,
        engine::{
            storage::{copy_to_var, fetch_stack, swap},
            Engine,
        },
        microcode::{CC, CTRL, SAVELIST, VAR},
        types::{Instruction, InstructionOptions},
    },
    stack::{continuation::ContinuationType, StackItem},
};
use std::ops::Range;
use ton_block::{fail, Exception, ExceptionCode, Status};

//Utilities **********************************************************************************
//(c c' -)
//c'.nargs = c'.stack.depth + 2
//c'.savelist[2] = c2, cc.savelist[2] = c2
//c'.savelist[0] = cc, c.savelist[0] = cc
//callx c
fn init_try_catch(engine: &mut Engine) -> Status {
    fetch_stack(engine, 2)?;
    if engine.cc.stack.depth() < engine.cmd.pargs() {
        fail!(ExceptionCode::StackUnderflow)
    }
    engine.cmd.var(1).as_continuation()?;
    engine.cmd.var_mut(0).as_continuation_mut().map(|catch_cont| {
        catch_cont.type_of = ContinuationType::TryCatch;
    })?;
    engine.cmd.var_mut(1).as_continuation_mut().map(|try_cont| try_cont.remove_from_savelist(0))?;
    if engine.ctrl(2).is_ok() {
        copy_to_var(engine, ctrl!(2))?;
        swap(engine, savelist!(var!(0), 2), var!(2))?;
        copy_to_var(engine, ctrl!(2))?;
        swap(engine, savelist!(CC, 2), var!(3))?;
    }
    // special swapping for callx: it calls a cont from var0, but at this point var0 holds catch cont
    swap(engine, var!(0), var!(1))?;
    swap(engine, ctrl!(2), var!(1))?;
    callx(engine, 0, false)?;
    copy_to_var(engine, ctrl!(0))?;
    let length = engine.cmd.var_count();
    swap(engine, savelist!(ctrl!(2), 0), var!(length - 1))?;
    Ok(())
}

fn do_throw(
    engine: &mut Engine,
    number_index: Option<usize>,
    value_index: Option<usize>,
) -> Status {
    let number = if let Some(number_index) = number_index {
        engine.cmd.var(number_index).as_integer_value(0..=0xFFFFi32)?
    } else {
        engine.cmd.integer() as i32
    };
    let value = if let Some(value_index) = value_index {
        engine.cmd.var(value_index).clone()
    } else {
        StackItem::int(0)
    };
    let exception = Exception::from_number(number, String::new(), file!(), line!());
    fail!(TvmError::new(exception, value))
}

//Handlers ***********************************************************************************

fn execute_throw(engine: &mut Engine, range: Range<isize>) -> Status {
    engine
        .load_instruction(Instruction::new("THROW").set_opts(InstructionOptions::Integer(range)))?;
    do_throw(engine, None, None)
}

// (=> throw 0 n)
pub(super) fn execute_throw_short(engine: &mut Engine) -> Status {
    execute_throw(engine, 0..64)
}

// (=> throw 0 n)
pub(super) fn execute_throw_long(engine: &mut Engine) -> Status {
    execute_throw(engine, 0..2048)
}

// helper for THROWIF/THROWIFNOT instructions
fn execute_throwif_throwifnot(
    engine: &mut Engine,
    reverse_condition: bool,
    range: Range<isize>,
) -> Status {
    engine.load_instruction(
        Instruction::new(if reverse_condition { "THROWIFNOT" } else { "THROWIF" })
            .set_opts(InstructionOptions::Integer(range)),
    )?;
    fetch_stack(engine, 1)?;
    if reverse_condition ^ engine.cmd.var(0).as_bool()? {
        do_throw(engine, None, None)
    } else {
        Ok(())
    }
}

pub(super) fn execute_throwif_short(engine: &mut Engine) -> Status {
    execute_throwif_throwifnot(engine, false, 0..64)
}

pub(super) fn execute_throwif_long(engine: &mut Engine) -> Status {
    execute_throwif_throwifnot(engine, false, 0..2048)
}

pub(super) fn execute_throwifnot_short(engine: &mut Engine) -> Status {
    execute_throwif_throwifnot(engine, true, 0..64)
}

pub(super) fn execute_throwifnot_long(engine: &mut Engine) -> Status {
    execute_throwif_throwifnot(engine, true, 0..2048)
}

// helper for THROWANYIF/THROWANYIFNOT instructions
fn execute_throwanyif_throwanyifnot(engine: &mut Engine, reverse_condition: bool) -> Status {
    engine.load_instruction(Instruction::new(if reverse_condition {
        "THROWANYIFNOT"
    } else {
        "THROWANYIF"
    }))?;
    fetch_stack(engine, 2)?;
    if reverse_condition ^ engine.cmd.var(0).as_bool()? {
        do_throw(engine, Some(1), None)
    } else {
        Ok(())
    }
}

// (n f, f!=0 => throw 0 n)
pub(super) fn execute_throwanyif(engine: &mut Engine) -> Status {
    execute_throwanyif_throwanyifnot(engine, false)
}

// (n f, f==0 => throw 0 n)
pub(super) fn execute_throwanyifnot(engine: &mut Engine) -> Status {
    execute_throwanyif_throwanyifnot(engine, true)
}

// (n => throw 0 n)
pub(super) fn execute_throwany(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("THROWANY"))?;
    fetch_stack(engine, 1)?;
    do_throw(engine, Some(0), None)
}

// (x => throw x n)
pub(super) fn execute_throwarg(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("THROWARG").set_opts(InstructionOptions::Integer(0..2048)),
    )?;
    fetch_stack(engine, 1)?;
    do_throw(engine, None, Some(0))
}

// (x n => throw x n)
pub(super) fn execute_throwargany(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("THROWARGANY"))?;
    fetch_stack(engine, 2)?;
    do_throw(engine, Some(0), Some(1))
}

// helper for THROWARGANYIF[NOT] instructions
fn execute_throwarganyif_throwarganyifnot(engine: &mut Engine, reverse_condition: bool) -> Status {
    engine.load_instruction(Instruction::new(if reverse_condition {
        "THROWARGANYIFNOT"
    } else {
        "THROWARGANYIF"
    }))?;
    fetch_stack(engine, 3)?;
    if reverse_condition ^ engine.cmd.var(0).as_bool()? {
        do_throw(engine, Some(1), Some(2))
    } else {
        Ok(())
    }
}

// (x n f, f!=0 => throw x n)
pub(super) fn execute_throwarganyif(engine: &mut Engine) -> Status {
    execute_throwarganyif_throwarganyifnot(engine, false)
}

// (x n f, f==0 => throw x n)
pub(super) fn execute_throwarganyifnot(engine: &mut Engine) -> Status {
    execute_throwarganyif_throwarganyifnot(engine, true)
}

// helper for THROWARGIF[NOT] instructions
fn execute_throwargif_throwargifnot(engine: &mut Engine, reverse_condition: bool) -> Status {
    engine.load_instruction(
        Instruction::new(if reverse_condition { "THROWARGIFNOT" } else { "THROWARGIF" })
            .set_opts(InstructionOptions::Integer(0..2048)),
    )?;
    fetch_stack(engine, 2)?;
    if reverse_condition ^ engine.cmd.var(0).as_bool()? {
        do_throw(engine, None, Some(1))
    } else {
        Ok(())
    }
}

// (x f, f!=0 => throw x n)
pub(super) fn execute_throwargif(engine: &mut Engine) -> Status {
    execute_throwargif_throwargifnot(engine, false)
}

// (x f, f==0 => throw x n)
pub(super) fn execute_throwargifnot(engine: &mut Engine) -> Status {
    execute_throwargif_throwargifnot(engine, true)
}

// (c c' - )
pub(super) fn execute_try(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("TRY"))?;
    init_try_catch(engine)
}

// (c c' - )
//move 0<=p<=15 stack elements from cc to c, return 0<=r<=15 stack values of resulting stack of c or c'.
pub(super) fn execute_tryargs(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("TRYARGS").set_opts(InstructionOptions::ArgumentAndReturnConstraints),
    )?;
    init_try_catch(engine)
}
