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
        microcode::{BUILDER, CELL, CONTINUATION, SLICE, VAR},
    },
    stack::{continuation::ContinuationData, StackItem},
};
use ton_block::{fail, GasConsumer, SliceData, Status};

// Utilities ******************************************************************

fn convert_any(engine: &mut Engine, x: u16, to: u16, from: u16) -> Status {
    if engine.cmd.vars.len() <= storage_index!(x) {
        fail!("convert_any no var {} in cmd", storage_index!(x));
    }
    let data = match address_tag!(x) {
        VAR => match from {
            BUILDER => {
                let var = engine.cmd.var_mut(storage_index!(x));
                let builder = var.as_builder_mut()?;
                match to {
                    CELL => {
                        let cell = engine.finalize_cell(builder)?;
                        StackItem::Cell(cell)
                    }
                    SLICE => {
                        let slice = if builder.references_used() == 0 {
                            SliceData::load_bitstring(builder)?
                        } else {
                            SliceData::load_builder(builder)?
                        };
                        StackItem::Slice(slice)
                    }
                    _ => fail!("can convert builder only to cell or to slice"),
                }
            }
            CELL => {
                let var = engine.cmd.var(storage_index!(x));
                let cell = var.as_cell()?.clone();
                let slice = engine.load_cell(cell)?;
                match to {
                    CONTINUATION => StackItem::continuation(ContinuationData::with_code(slice)),
                    SLICE => StackItem::Slice(slice),
                    _ => fail!("can convert cell only to slice or to continuation"),
                }
            }
            SLICE => {
                let var = engine.cmd.var(storage_index!(x));
                let slice = var.as_slice()?.clone();
                match to {
                    CONTINUATION => StackItem::continuation(ContinuationData::with_code(slice)),
                    _ => fail!("can convert slice only to continuation"),
                }
            }
            _ => fail!("cannot convert"),
        },
        _ => StackItem::None,
    };
    if data.is_null() {
        fail!("cannot convert_any x: {:X}, to: {:X}, from: {:X}", x, to, from)
    } else {
        *engine.cmd.var_mut(storage_index!(x)) = data;
    }
    Ok(())
}

// Microfunctions *************************************************************

// Convert type of x; x addressing is described in executor/microcode.rs
// to, from are one of { BUILDER, CELL, CONTINUATION, SLICE }
pub(in crate::executor) fn convert(engine: &mut Engine, x: u16, to: u16, from: u16) -> Status {
    convert_any(engine, x, to, from)
}
