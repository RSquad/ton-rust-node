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
        engine::Engine,
        gas::gas_state::Gas,
        microcode::{CC, CC_SAVELIST, CTRL, CTRL_SAVELIST, STACK, VAR, VAR_SAVELIST},
    },
    stack::{continuation::ContinuationData, savelist::SaveList, StackItem},
};
use std::{mem, ops::Range};
use ton_block::{error, fail, ExceptionCode, Result, Status};

// Utilities ******************************************************************

fn continuation_by_address(engine: &mut Engine, address: u16) -> Result<&ContinuationData> {
    match address_tag!(address) {
        VAR => engine.cmd.var(storage_index!(address)).as_continuation(),
        CTRL => match engine.ctrls.get(storage_index!(address)) {
            Some(ctrl) => ctrl.as_continuation(),
            None => fail!(ExceptionCode::TypeCheckError),
        },
        _ => fail!("continuation_by_address: {:X}", address_tag!(address)),
    }
}

#[macro_export]
macro_rules! continuation_mut_by_address {
    ($engine:ident, $address:expr) => {
        match address_tag!($address) {
            VAR => $engine.cmd.var_mut(storage_index!($address)).as_continuation_mut(),
            CTRL => match $engine.ctrls.get_mut(storage_index!($address)) {
                Some(ctrl) => ctrl.as_continuation_mut(),
                None => ton_block::fail!(ExceptionCode::TypeCheckError),
            },
            _ => fail!("continuation_mut_by_address: {:X}", address_tag!($address)),
        }
    };
}

fn move_stack_from_cc(engine: &mut Engine, dst: u16, drop: Range<usize>) -> Status {
    let save = drop.len();
    let peer = continuation_mut_by_address!(engine, dst)?;
    if peer.nargs >= 0 {
        if save > peer.nargs as usize {
            fail!(ExceptionCode::StackOverflow)
        } else {
            peer.nargs -= save as isize
        }
    }
    if drop.start == 0 {
        let src_len = engine.cc.stack.depth();
        if src_len < drop.end {
            fail!(
                ExceptionCode::StackUnderflow,
                "drop_range: {}..{}, depth: {}",
                drop.start,
                drop.end,
                src_len
            )
        }
        if peer.stack.is_empty() && drop.end == src_len {
            mem::swap(&mut peer.stack, &mut engine.cc.stack);
        } else {
            let drain = engine.cc.stack.storage.drain(src_len - drop.end..);
            peer.stack.storage.extend(drain);
        }
    } else {
        let mut popped = engine.cc.stack.drop_range_straight(drop)?;
        peer.stack.append(&mut popped);
    }
    Ok(())
}

// Swapping *******************************************************************

struct Info {
    flags: u16,
    index: usize,
}

impl std::fmt::UpperHex for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "flags: {:X}, index: {:X}", self.flags, self.index)
    }
}

impl Info {
    fn item<'a>(&self, engine: &'a mut Engine) -> Result<&'a mut StackItem> {
        match address_tag!(self.flags) {
            VAR => Ok(engine.cmd.var_mut(self.index)),
            _ => fail!("Info.item {:x}\n", self.flags),
        }
    }
    #[rustfmt::skip]
    fn list<'a>(&mut self, engine: &'a mut Engine) -> Result<&'a mut SaveList> {
        match address_tag!(self.flags) {
            CC_SAVELIST => {
                self.index = savelist_index!(self.flags);
                Ok(&mut engine.cc.savelist)
            },
            CTRL => Ok(&mut engine.ctrls),
            CTRL_SAVELIST => {
                let continuation = engine.ctrls.get_mut(storage_index!(self.flags))
                    .ok_or_else(|| error!("Info.list: {:X} - {}", self, storage_index!(self.flags)))?
                    .as_continuation_mut()?;
                self.index = savelist_index!(self.flags);
                Ok(&mut continuation.savelist)
            },
            VAR_SAVELIST => {
                let continuation = engine.cmd.var_mut(storage_index!(self.flags))
                    .as_continuation_mut()?;
                self.index = savelist_index!(self.flags);
                Ok(&mut continuation.savelist)
            }
            _ => fail!("Info.list {:X}\n", self)
        }
    }
}

fn put_to_list(engine: &mut Engine, x: &mut Info, y: StackItem) -> Result<Option<StackItem>> {
    x.list(engine)?.put(x.index, y)
}

fn put_to_list_from_item(engine: &mut Engine, x: &mut Info, y: &Info) -> Result<Option<StackItem>> {
    if !SaveList::can_put(x.index, y.item(engine)?) {
        if log::log_enabled!(log::Level::Error) {
            let value = x.list(engine)?.get(x.index).cloned().unwrap_or_else(StackItem::default);
            log::error!(
                target: "tvm",
                "Cannot set: {} to list with index: {} and value: {}",
                y.item(engine)?.clone(), x.index, value
            );
        }
        fail!(ExceptionCode::TypeCheckError)
    } else {
        let y = y.item(engine)?.withdraw();
        x.list(engine)?.put(x.index, y)
    }
}

fn put_to_list_from_list(
    engine: &mut Engine,
    x: &mut Info,
    y: &mut Info,
) -> Result<Option<StackItem>> {
    x.list(engine)?;
    if let Some(new) = y.list(engine)?.get(y.index) {
        if SaveList::can_put(x.index, new) {
            if let Some(y) = y.list(engine)?.remove(y.index) {
                return x.list(engine)?.put(x.index, y);
            }
        }
    }
    if log::log_enabled!(log::Level::Error) {
        let old = x.list(engine)?.get(x.index).cloned().unwrap_or_else(StackItem::default);
        let new = y.list(engine)?.get(y.index).cloned().unwrap_or_else(StackItem::default);
        log::error!(
            target: "tvm",
            "Cannot set: {} to list with index: {} and value: {}",
            new, x.index, old
        );
    }
    fail!(ExceptionCode::TypeCheckError)
}

fn swap_with_list(engine: &mut Engine, mut x: Info, y: Info) -> Status {
    if x.list(engine)?.get(x.index).is_some() || !y.item(engine)?.is_null() {
        *y.item(engine)? = match put_to_list_from_item(engine, &mut x, &y)? {
            Some(x) => x,
            None => StackItem::None,
        };
    }
    Ok(())
}

fn swap_between_lists(engine: &mut Engine, mut x: Info, mut y: Info) -> Status {
    if y.list(engine)?.get(y.index).is_some() {
        if let Some(x) = put_to_list_from_list(engine, &mut x, &mut y)? {
            put_to_list(engine, &mut y, x)?;
        }
    } else if x.list(engine)?.get(x.index).is_some() {
        put_to_list_from_list(engine, &mut y, &mut x)?;
    }
    Ok(())
}

// x <-> y
// x and y addressing is described in executor/microcode.rs
pub(in crate::executor) fn swap(engine: &mut Engine, mut x: u16, mut y: u16) -> Status {
    if address_tag!(x) > address_tag!(y) {
        mem::swap(&mut x, &mut y);
    }
    let x = Info { flags: x, index: storage_index!(x) };
    let y = Info { flags: y, index: storage_index!(y) };
    match address_tag!(x.flags) {
        CC_SAVELIST | CTRL | CTRL_SAVELIST | VAR_SAVELIST => match address_tag!(y.flags) {
            CC_SAVELIST | CTRL | CTRL_SAVELIST | VAR_SAVELIST => swap_between_lists(engine, x, y),
            VAR => swap_with_list(engine, x, y),
            _ => fail!("swap_any: {:X}, {:X}", x, y),
        },
        CC => match address_tag!(y.flags) {
            CTRL => match engine.ctrls.get_mut(y.index) {
                Some(c) => {
                    mem::swap(c.as_continuation_mut()?, &mut engine.cc);
                    Ok(())
                }
                None => fail!(ExceptionCode::TypeCheckError),
            },
            VAR => {
                mem::swap(engine.cmd.var_mut(y.index).as_continuation_mut()?, &mut engine.cc);
                Ok(())
            }
            _ => fail!("swap CC-{:X}", y),
        },
        VAR => match address_tag!(y.flags) {
            CC_SAVELIST | CTRL_SAVELIST | VAR_SAVELIST => swap_with_list(engine, y, x),
            VAR => {
                engine.cmd.vars.swap(x.index, y.index);
                Ok(())
            }
            _ => fail!("swap_any: {:X}, {:X}", x, y),
        },
        _ => {
            fail!("swap_any {:X}-{:X}", x, y)
        }
    }
}

// Microfunctions *************************************************************

// c[*] = CC.savelist[*], excluding given indexes
pub(in crate::executor) fn apply_savelist_excluding_c0_c1(engine: &mut Engine) -> Status {
    engine.cc.savelist.remove(0);
    engine.cc.savelist.remove(1);
    engine.ctrls.apply(&mut engine.cc.savelist);
    Ok(())
}

pub(in crate::executor) fn apply_savelist(engine: &mut Engine) -> Status {
    engine.ctrls.apply(&mut engine.cc.savelist);
    Ok(())
}

// ctx.cmd.push_var(copy-of-src)
// src addressing is described in executor/microcode.rs
pub(in crate::executor) fn copy_to_var(engine: &mut Engine, src: u16) -> Status {
    let copy = match address_tag!(src) {
        CC => {
            let copy = engine.cc.copy_without_stack();
            StackItem::continuation(copy)
        }
        CTRL => engine.ctrls.get(storage_index!(src)).cloned().unwrap_or_default(),
        // CTRL => match engine.ctrls.get(storage_index!(src)) {
        //     Some(ctrl) => ctrl.clone(),
        //     None => fail!(ExceptionCode::TypeCheckError, "read empty control register {}", storage_index!(src))
        // }
        STACK => engine.cc.stack.get(stack_index!(src))?.clone(),
        VAR => engine.cmd.var(storage_index!(src)).clone(),
        _ => fail!("copy_to_var: {}", src),
    };
    engine.cmd.push_var(copy);
    Ok(())
}

// ctx.cmd.push_var(src.references[0])
pub(in crate::executor) fn fetch_reference(engine: &mut Engine, src: u16) -> Status {
    let cell = match address_tag!(src) {
        CC => engine.cc.drain_reference()?,
        _ => fail!("fetch_reference: {:X}", src),
    };
    engine.cmd.push_var(StackItem::Cell(cell));
    Ok(())
}

// ctx.cmd.push_var(CC.stack[0..depth])
pub(in crate::executor) fn fetch_stack(engine: &mut Engine, depth: usize) -> Status {
    if engine.cc.stack.depth() < depth {
        fail!(ExceptionCode::StackUnderflow)
    } else {
        engine.cmd.vars.append(&mut engine.cc.stack.drop_range(0..depth)?);
        Ok(())
    }
}

// dst.stack.push(CC.stack)
// dst addressing is described in executor/microcode.rs
pub(in crate::executor) fn pop_all(engine: &mut Engine, dst: u16) -> Status {
    let nargs = continuation_by_address(engine, dst)?.nargs;
    let depth = engine.cc.stack.depth();
    let pargs = engine.cmd.pargs_raw();
    let drop = if nargs < 0 {
        pargs.unwrap_or(depth)
    } else if let Some(pargs) = pargs {
        if pargs < nargs as usize {
            fail!(
                ExceptionCode::StackUnderflow,
                "depth: {}, pargs: {}, nargs: {}",
                depth,
                pargs,
                nargs
            )
        }
        pargs
    } else {
        nargs as usize
    };
    pop_range(engine, 0..drop, dst)
}

// dst.stack.push(CC.stack[range])
// dst addressing is described in executor/microcode.rs
pub(in crate::executor) fn pop_range(engine: &mut Engine, drop: Range<usize>, dst: u16) -> Status {
    let save = drop.len();
    let src_depth = engine.cc.stack.depth();
    let dst_depth = continuation_by_address(engine, dst)?.stack.depth();
    // pay for stack splitting
    if src_depth > save {
        engine.try_use_gas(Gas::stack_price(save))?;
    }
    // pay for stack concatenation
    if dst_depth != 0 {
        engine.try_use_gas(Gas::stack_price(save + dst_depth))?;
    }
    move_stack_from_cc(engine, dst, drop)
}
