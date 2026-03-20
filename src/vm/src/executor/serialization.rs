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
            storage::{fetch_reference, fetch_stack},
            Engine,
        },
        gas::gas_state::Gas,
        microcode::{BUILDER, CC, CELL, VAR},
        types::{Instruction, InstructionOptions},
    },
    stack::{
        integer::{behavior::OperationBehavior, IntegerData},
        StackItem,
    },
};
use ton_block::{
    fail, BuilderData, CellType, Deserializable, ExceptionCode, GasConsumer, IBitstring, Mask,
    MsgAddrStd, MsgAddress, Result, Serializable, Status, MAX_LEVEL,
};

const QUIET: u8 = 0x01; // quiet variant
const STACK: u8 = 0x02; // length of int in stack
const CMD: u8 = 0x04; // length of int in cmd parameter
const BITS: u8 = 0x08; // check bits
const REFS: u8 = 0x10; // check refs
const INV: u8 = 0x20; // Remain free in builder
const SIGNED: u8 = 0x40; // signed integer
const BE: u8 = 0x80; // big-endian integer

// Cell serialization related instructions ************************************

// used of free bits or/and refs in builder
fn size_b(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    match engine.cmd.var(0).as_builder()? {
        b if how.bit(INV) => {
            if how.bit(BITS) {
                engine.cc.stack.push_int(b.bits_free());
            }
            if how.bit(REFS) {
                engine.cc.stack.push_int(b.references_free());
            }
        }
        b => {
            if how.bit(BITS) {
                engine.cc.stack.push_int(b.bits_used());
            }
            if how.bit(REFS) {
                engine.cc.stack.push_int(b.references_used());
            }
        }
    }
    Ok(())
}

/// BBITS (b - x), returns the number of data bits already stored in Builder b.
pub fn execute_bbits(engine: &mut Engine) -> Status {
    size_b(engine, "BBITS", BITS)
}

/// BREFS (b - y), returns the number of cell references already stored in b.
pub fn execute_brefs(engine: &mut Engine) -> Status {
    size_b(engine, "BREFS", REFS)
}

/// BBITREFS (b - x y), returns the numbers of both data bits and cell references in b.
pub fn execute_bbitrefs(engine: &mut Engine) -> Status {
    size_b(engine, "BBITREFS", BITS | REFS)
}

/// BREMBITS (b - x`), returns the number of data bits that can still be stored in b.
pub fn execute_brembits(engine: &mut Engine) -> Status {
    size_b(engine, "BREMBITS", INV | BITS)
}

/// BREMREFS (b - y`), returns the number of references that can still be stored in b.
pub fn execute_bremrefs(engine: &mut Engine) -> Status {
    size_b(engine, "BREMREFS", INV | REFS)
}

/// BREMBITREFS (b - x` y`).
pub fn execute_brembitrefs(engine: &mut Engine) -> Status {
    size_b(engine, "BREMBITREFS", INV | BITS | REFS)
}

// (builder - cell)
pub fn execute_endc(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ENDC"))?;
    fetch_stack(engine, 1)?;
    convert(engine, var!(0), CELL, BUILDER)?;
    engine.cc.stack.push(engine.cmd.vars.remove(0));
    Ok(())
}

// (builder x - cell)
pub fn execute_endxc(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ENDXC"))?;
    fetch_stack(engine, 2)?;
    let special = engine.cmd.var(0).as_bool()?;
    let mut b = engine.cmd.var_mut(1).as_builder_mut()?;
    if special {
        if b.length_in_bits() < 8 {
            engine.use_gas(Gas::finalize_price());
            fail!(ExceptionCode::CellOverflow, "Not enough data for a special cell")
        }
        let cell_type = match CellType::try_from(b.data()[0]) {
            Ok(cell_type) => cell_type,
            Err(err) => {
                engine.use_gas(Gas::finalize_price());
                fail!(ExceptionCode::CellOverflow, "{}", err)
            }
        };
        match cell_type {
            // allow the following known types
            CellType::PrunedBranch
            | CellType::LibraryReference
            | CellType::MerkleProof
            | CellType::MerkleUpdate => (),
            // deny all other types
            _ => {
                engine.use_gas(Gas::finalize_price());
                fail!(ExceptionCode::CellOverflow, "Incorrect type of exotic cell: {}", cell_type)
            }
        }
        b.set_type(cell_type)
    }
    let cell = engine.finalize_cell(b)?;
    engine.cc.stack.push(StackItem::Cell(cell));
    Ok(())
}

// ( - builder)
pub fn execute_newc(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("NEWC"))?;
    engine.cc.stack.push_builder(BuilderData::new());
    Ok(())
}

// store data from one builder to another
fn store_data(
    engine: &mut Engine,
    var: usize,
    x: Result<BuilderData>,
    quiet: bool,
    finalize: bool,
) -> Status {
    let result = match x {
        Ok(x) => {
            let b = engine.cmd.var(var).as_builder()?;
            if b.can_append(&x) {
                let mut b = engine.cmd.var_mut(var).as_builder_mut()?;
                b.append_builder(&x)?;
                if finalize {
                    engine.try_use_gas(Gas::finalize_price())?;
                }
                engine.cc.stack.push_builder(b);
                0
            } else if quiet {
                -1
            } else {
                fail!(ExceptionCode::CellOverflow)
            }
        }
        Err(_) if quiet => 1,
        Err(err) => return Err(err),
    };
    if result != 0 {
        let len = engine.cmd.var_count();
        engine.cc.stack.push(engine.cmd.var(len - 1).clone());
        engine.cc.stack.push(engine.cmd.var(len - 2).clone());
        engine.cc.stack.push_int(result);
    } else if quiet {
        engine.cc.stack.push_bool(false);
    }
    Ok(())
}

// stores data from one builder ot another
fn store_b(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var(0).as_builder()?;
        engine.cmd.var(1).as_builder()?;
        1
    } else {
        engine.cmd.var(0).as_builder()?;
        x = engine.cmd.var(1).as_builder()?;
        0
    };
    let x = Ok(x.clone());
    store_data(engine, b, x, how.bit(QUIET), false)
}

/// STB (b` b - b``), appends all data from Builder b` to Builder b.
pub fn execute_stb(engine: &mut Engine) -> Status {
    store_b(engine, "STB", 0)
}

/// STBR (b b` - b``), concatenates two Builders, equivalent to SWAP; STB.
pub fn execute_stbr(engine: &mut Engine) -> Status {
    store_b(engine, "STBR", INV)
}

/// STBQ (builder builder - (builder builder -1) | (builder 0)).
pub fn execute_stbq(engine: &mut Engine) -> Status {
    store_b(engine, "STBQ", QUIET)
}

/// STBRQ (builder builder - (builder builder -1) | (builder 0)).
pub fn execute_stbrq(engine: &mut Engine) -> Status {
    store_b(engine, "STBRQ", INV | QUIET)
}

// appends the cell as a reference to the builder
fn store_r(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var(0).as_cell()?;
        engine.cmd.var(1).as_builder()?;
        1
    } else {
        engine.cmd.var(0).as_builder()?;
        x = engine.cmd.var(1).as_cell()?;
        0
    };
    let x = BuilderData::with_raw_and_refs(vec![], 0, vec![x.clone()]);
    store_data(engine, b, x, how.bit(QUIET), false)
}

// (cell builder - builder)
pub fn execute_stref(engine: &mut Engine) -> Status {
    store_r(engine, "STREF", 0)
}

/// STREFR (b c - b`).
pub fn execute_strefr(engine: &mut Engine) -> Status {
    store_r(engine, "STREFR", INV)
}

// (cell builder - (cell builder -1) | (builder 0))
pub fn execute_strefq(engine: &mut Engine) -> Status {
    store_r(engine, "STREFQ", QUIET)
}

// (builder cell - (builder cell -1) | (builder 0))
pub fn execute_strefrq(engine: &mut Engine) -> Status {
    store_r(engine, "STREFRQ", INV | QUIET)
}

// store one builder to another as reference
fn store_br(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var_mut(0).as_builder_mut()?;
        engine.cmd.var(1).as_builder()?;
        1
    } else {
        engine.cmd.var(0).as_builder()?;
        x = engine.cmd.var_mut(1).as_builder_mut()?;
        0
    };
    let x = BuilderData::with_raw_and_refs(vec![], 0, vec![x.into_cell()?]);
    store_data(engine, b, x, how.bit(QUIET), true)
}

/// STBREF (b` b - b``), equivalent to SWAP; STBREFREV
pub fn execute_stbref(engine: &mut Engine) -> Status {
    store_br(engine, "STBREF", 0)
}

// (builder_outer builder_inner - builder)
pub fn execute_endcst(engine: &mut Engine) -> Status {
    store_br(engine, "ENDCST", INV)
}

/// STBREFQ
pub fn execute_stbrefq(engine: &mut Engine) -> Status {
    store_br(engine, "STBREFQ", QUIET)
}

/// STBREFQ
pub fn execute_stbrefrq(engine: &mut Engine) -> Status {
    store_br(engine, "STBREFRQ", INV | QUIET)
}

fn store_s(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var(0).as_slice()?;
        engine.cmd.var(1).as_builder()?;
        1
    } else {
        engine.cmd.var(0).as_builder()?;
        x = engine.cmd.var(1).as_slice()?;
        0
    };
    let x = Ok(x.as_builder()?);
    store_data(engine, b, x, how.bit(QUIET), false)
}

// (D b - b')
pub(crate) fn execute_stdict(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("STDICT"))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_builder()?;
    let x = match engine.cmd.var(1).as_dict()? {
        Some(x) => BuilderData::with_raw_and_refs(vec![0xC0], 1, vec![x.clone()]),
        None => BuilderData::with_raw(vec![0x40], 1),
    };
    store_data(engine, 0, x, false, false)
}

// (s b - b)
pub fn execute_stslice(engine: &mut Engine) -> Status {
    store_s(engine, "STSLICE", 0)
}

/// STSLICER (b s - b`)
pub fn execute_stslicer(engine: &mut Engine) -> Status {
    store_s(engine, "STSLICER", INV)
}

// (slice builder - (slice builder -1) | (builder 0))
pub fn execute_stsliceq(engine: &mut Engine) -> Status {
    store_s(engine, "STSLICEQ", QUIET)
}

// (builder slice - (builder slice -1 ) | (builder 0))
pub fn execute_stslicerq(engine: &mut Engine) -> Status {
    store_s(engine, "STSLICERQ", INV | QUIET)
}

fn store_addr(
    builder: &mut BuilderData,
    slice_or_null: &StackItem,
    none_allowed: bool,
) -> Result<bool> {
    if none_allowed && slice_or_null.is_null() {
        MsgAddress::AddrNone.write_to(builder)?;
    } else if let Ok(slice) = slice_or_null.as_slice() {
        let result = MsgAddress::construct_from(&mut slice.clone());
        let Ok(MsgAddress::AddrStd(MsgAddrStd { anycast, .. })) = result else {
            fail!(ExceptionCode::CellOverflow, "expected MsgAddress::AddrStd (standard address)");
        };
        if anycast.is_some() {
            fail!(ExceptionCode::CellOverflow, "cannot store anycast address");
        }
        builder.checked_append_references_and_data(slice)?;
    } else {
        return Ok(false);
    }
    Ok(true)
}

fn store_opt_std_addr<T: OperationBehavior>(
    engine: &mut Engine,
    name: &'static str,
    none_allowed: bool,
) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let mut builder = engine.cmd.var_mut(0).as_builder_mut()?;
    let var = engine.cmd.var_mut(1).withdraw();
    match store_addr(&mut builder, &var, none_allowed) {
        Ok(true) => {
            engine.cc.stack.push_builder(builder);
            if T::quiet() {
                engine.cc.stack.push_bool(false);
            }
        }
        Ok(false) => {
            if T::quiet() && none_allowed {
                // inconsistent here for quiet version
                engine.cc.stack.push_slice(Default::default());
                engine.cc.stack.push_builder(builder);
                engine.cc.stack.push_bool(true);
            } else {
                fail!(ExceptionCode::TypeCheckError)
            }
        }
        Err(err) => {
            if T::quiet() {
                engine.cc.stack.push(var);
                engine.cc.stack.push_builder(builder);
                engine.cc.stack.push_bool(true);
            } else {
                return Err(err);
            }
        }
    }
    Ok(())
}

// (slice builder - (builder slice -1 ) | (builder2 0))
pub fn execute_ststdaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "STSTDADDRQ" } else { "STSTDADDR" };
    store_opt_std_addr::<T>(engine, name, false)
}

// (slice_or_none builder - (builder -1 ) | (builder2 0))
pub fn execute_stoptstdaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "STOPTSTDADDRQ" } else { "STOPTSTDADDR" };
    store_opt_std_addr::<T>(engine, name, true)
}

fn check_b(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    let mut instruction = Instruction::new(name);
    let mut params = 1;
    if how.bit(BITS) {
        params += 1
    }
    if how.bit(REFS) {
        params += 1
    }
    if how.bit(CMD) {
        params -= 1;
        instruction = instruction.set_opts(InstructionOptions::LengthMinusOne(0..256))
    }
    engine.load_instruction(instruction)?;
    fetch_stack(engine, params)?;
    // TODO: right order of type check
    let l = if how.bit(CMD) {
        engine.cmd.length()
    } else if how.bit(BITS) {
        engine.cmd.var(params - 2).as_integer_value(0..=1023)?
    } else {
        0
    };
    let r = if how.bit(REFS) { engine.cmd.var(0).as_integer_value(0..=4)? } else { 0 };
    let b = engine.cmd.var(params - 1).as_builder()?;
    let mut status = true;
    if how.bit(BITS) {
        status &= b.check_enough_space(l)
    }
    if how.bit(REFS) {
        status &= b.check_enough_refs(r)
    }
    if how.bit(QUIET) {
        engine.cc.stack.push(boolean!(status));
    } else if !status {
        fail!(ExceptionCode::CellOverflow)
    }
    Ok(())
}

pub fn execute_bchkrefs(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKREFS", REFS | STACK)
}

pub fn execute_bchkrefsq(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKREFSQ", REFS | STACK | QUIET)
}

pub fn execute_bchkbitrefs(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITREFS", BITS | REFS | STACK)
}

pub fn execute_bchkbitrefsq(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITREFSQ", BITS | REFS | STACK | QUIET)
}

pub fn execute_bchkbits_short(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITS", BITS | CMD)
}

pub fn execute_bchkbits_long(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITS", BITS | STACK)
}

pub fn execute_bchkbitsq_short(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITS", BITS | CMD | QUIET)
}

pub fn execute_bchkbitsq_long(engine: &mut Engine) -> Status {
    check_b(engine, "BCHKBITS", BITS | STACK | QUIET)
}

fn store(engine: &mut Engine, name: &'static str, how: u8) -> Status {
    engine.load_instruction(
        Instruction::new(name).set_opts(InstructionOptions::LengthMinusOne(0..256)),
    )?;
    fetch_stack(engine, 2)?;
    let len = engine.cmd.length();
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var(0).as_integer()?.as_builder(len, how.bit(SIGNED), how.bit(BE));
        engine.cmd.var(1).as_builder()?;
        1
    } else {
        engine.cmd.var(0).as_builder()?;
        x = engine.cmd.var(1).as_integer()?.as_builder(len, how.bit(SIGNED), how.bit(BE));
        0
    };
    store_data(engine, b, x, how.bit(QUIET), false)
}

// (x builder - builder)
pub fn execute_sti(engine: &mut Engine) -> Status {
    store(engine, "STI", SIGNED | BE)
}

// (x builder - builder)
pub fn execute_stu(engine: &mut Engine) -> Status {
    store(engine, "STU", BE)
}

// (x builder - builder)
pub fn execute_stir(engine: &mut Engine) -> Status {
    store(engine, "STIR", INV | SIGNED | BE)
}

// (x builder - builder)
pub fn execute_stur(engine: &mut Engine) -> Status {
    store(engine, "STUR", INV | BE)
}

// (x builder - builder)
pub fn execute_stiq(engine: &mut Engine) -> Status {
    store(engine, "STIQ", QUIET | SIGNED | BE)
}

// (x builder - builder)
pub fn execute_stuq(engine: &mut Engine) -> Status {
    store(engine, "STUQ", QUIET | BE)
}

// (x builder - builder)
pub fn execute_stirq(engine: &mut Engine) -> Status {
    store(engine, "STIRQ", QUIET | INV | SIGNED | BE)
}

// (x builder - builder)
pub fn execute_sturq(engine: &mut Engine) -> Status {
    store(engine, "STURQ", QUIET | INV | BE)
}

fn store_x(engine: &mut Engine, name: &'static str, how: u8, limit: usize) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 3)?;
    let len = engine.cmd.var(0).as_integer()?;
    let x;
    let b = if how.bit(INV) {
        x = engine.cmd.var(1).as_integer()?;
        engine.cmd.var(2).as_builder()?;
        2
    } else {
        engine.cmd.var(1).as_builder()?;
        x = engine.cmd.var(2).as_integer()?;
        1
    };
    let len = len.as_integer_value(0..=limit)?;
    let x = x.as_builder(len, how.bit(SIGNED), how.bit(BE));
    store_data(engine, b, x, how.bit(QUIET), false)
}

// (integer builder nbits - builder)
pub fn execute_stix(engine: &mut Engine) -> Status {
    store_x(engine, "STIX", SIGNED | BE, 257)
}

// (integer builder nbits - builder)
pub fn execute_stux(engine: &mut Engine) -> Status {
    store_x(engine, "STUX", BE, 256)
}

// (builder integer nbits - builder)
pub fn execute_stixr(engine: &mut Engine) -> Status {
    store_x(engine, "STIXR", INV | SIGNED | BE, 257)
}

// (builder integer nbits - builder)
pub fn execute_stuxr(engine: &mut Engine) -> Status {
    store_x(engine, "STUXR", INV | BE, 256)
}

// (integer builder nbits - (integer builder integer) | (builder integer))
pub fn execute_stixq(engine: &mut Engine) -> Status {
    store_x(engine, "STIXQ", QUIET | SIGNED | BE, 257)
}

// (integer builder nbits - (integer builder integer) | (builder integer))
pub fn execute_stuxq(engine: &mut Engine) -> Status {
    store_x(engine, "STUXQ", QUIET | BE, 256)
}

// (builder integer nbits - (builder integer integer) | (builder integer))
pub fn execute_stixrq(engine: &mut Engine) -> Status {
    store_x(engine, "STIXRQ", QUIET | INV | SIGNED | BE, 257)
}

// (builder integer nbits - (builder integer integer) | (builder integer))
pub fn execute_stuxrq(engine: &mut Engine) -> Status {
    store_x(engine, "STUXRQ", QUIET | INV | BE, 256)
}

// stores the integer to the builder in little-endian order
fn store_l(engine: &mut Engine, name: &'static str, bits: usize, how: u8) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_builder()?;
    let x = engine.cmd.var(1).as_integer()?.as_builder(bits, how.bit(SIGNED), how.bit(BE));
    store_data(engine, 0, x, false, false)
}

/// STILE4 (x b - b`), stores a little-endian signed 32-bit integer.
pub fn execute_stile4(engine: &mut Engine) -> Status {
    store_l(engine, "STILE4", 32, SIGNED)
}

/// STULE4 (x b - b`), stores a little-endian unsigned 32-bit integer.
pub fn execute_stule4(engine: &mut Engine) -> Status {
    store_l(engine, "STULE4", 32, 0)
}

/// STILE8 (x b - b`), stores a little-endian signed 64-bit integer.
pub fn execute_stile8(engine: &mut Engine) -> Status {
    store_l(engine, "STILE8", 64, SIGNED)
}

/// STULE8 (x b - b`), stores a little-endian unsigned 64-bit integer.
pub fn execute_stule8(engine: &mut Engine) -> Status {
    store_l(engine, "STULE8", 64, 0)
}

fn store_bits(mut builder: BuilderData, n: usize, bit: bool) -> Result<BuilderData> {
    if n != 0 {
        builder.append_raw(vec![if bit { 0xFF } else { 0 }; n / 8 + 1].as_slice(), n)?;
    }
    Ok(builder)
}

fn stbits(engine: &mut Engine, name: &'static str, bit: bool) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let n = engine.cmd.var(0).as_integer()?;
    engine.cmd.var(1).as_builder()?;
    let n = n.as_integer_value(0..=1023)?;
    let b = engine.cmd.var_mut(1).as_builder_mut()?;
    engine.cc.stack.push_builder(store_bits(b, n, bit)?);
    Ok(())
}

/// STZEROES (b n – b`), stores n binary zeroes into Builder b.
pub fn execute_stzeroes(engine: &mut Engine) -> Status {
    stbits(engine, "STZEROES", false)
}

/// stores n binary ones into Builder b.
pub fn execute_stones(engine: &mut Engine) -> Status {
    stbits(engine, "STONES", true)
}

pub fn execute_stsame(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("STSAME"))?;
    fetch_stack(engine, 3)?;
    let x = engine.cmd.var(0).as_integer()?;
    let n = engine.cmd.var(1).as_integer()?;
    engine.cmd.var(2).as_builder()?;
    let x = x.as_integer_value(0..=1)?;
    let n = n.as_integer_value(0..=1023)?;
    let b = engine.cmd.var_mut(2).as_builder_mut()?;
    engine.cc.stack.push_builder(store_bits(b, n, x != 0)?);
    Ok(())
}

pub fn execute_stsliceconst(engine: &mut Engine) -> Status {
    engine.load_instruction(
        Instruction::new("STSLICECONST").set_opts(InstructionOptions::Bitstring(9, 2, 3, 0)),
    )?;
    fetch_stack(engine, 1)?;
    let mut builder = engine.cmd.var_mut(0).as_builder_mut()?;
    let slice = engine.cmd.slice();
    builder.checked_append_references_and_data(slice)?;
    engine.cc.stack.push_builder(builder);
    Ok(())
}

pub fn execute_strefconst(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("STREFCONST"))?;
    fetch_reference(engine, CC)?;
    fetch_stack(engine, 1)?;
    let mut b = {
        engine.cmd.var(0).as_cell()?;
        engine.cmd.var_mut(1).as_builder_mut()?
    };
    b.checked_append_reference(engine.cmd.var(0).as_cell()?.clone())?;
    engine.cc.stack.push_builder(b);
    Ok(())
}

pub fn execute_stref2const(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("STREF2CONST"))?;
    fetch_reference(engine, CC)?;
    fetch_reference(engine, CC)?;
    fetch_stack(engine, 1)?;
    let mut b = {
        engine.cmd.var(0).as_cell()?;
        engine.cmd.var(1).as_cell()?;
        engine.cmd.var_mut(2).as_builder_mut()?
    };
    b.checked_append_reference(engine.cmd.var(0).as_cell()?.clone())?;
    b.checked_append_reference(engine.cmd.var(1).as_cell()?.clone())?;
    engine.cc.stack.push_builder(b);
    Ok(())
}

/// BDEPTH (b - x), returns the depth of Builder b.
pub fn execute_bdepth(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BDEPTH"))?;
    fetch_stack(engine, 1)?;
    let mut depth = 0;
    let b = engine.cmd.var(0).as_builder()?;
    for cell in b.references() {
        depth = std::cmp::max(depth, 1 + cell.depth(MAX_LEVEL));
    }
    engine.cc.stack.push_int(depth);
    Ok(())
}

/// CDEPTH (c - x), returns the depth of Cell c.
pub fn execute_cdepth(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CDEPTH"))?;
    fetch_stack(engine, 1)?;
    let depth = if engine.cmd.var(0).is_null() {
        0
    } else {
        let c = engine.cmd.var(0).as_cell()?;
        if c.references_count() == 0 {
            0
        } else {
            c.depth(MAX_LEVEL)
        }
    };
    engine.cc.stack.push_int(depth);
    Ok(())
}

/// CDEPTHI (c - x), returns the depth i of Cell c.
pub fn execute_cdepthi(engine: &mut Engine) -> Status {
    engine
        .load_instruction(Instruction::new("CDEPTHI").set_opts(InstructionOptions::Length(0..4)))?;
    fetch_stack(engine, 1)?;
    let i = engine.cmd.length();
    let c = engine.cmd.var(0).as_cell()?;
    let depth = c.depth(i);
    engine.cc.stack.push_int(depth);
    Ok(())
}

/// CDEPTHIX (c i - x), returns the depth i of Cell c.
pub fn execute_cdepthix(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CDEPTHIX"))?;
    fetch_stack(engine, 2)?;
    let i = engine.cmd.var(0).as_integer_value(0..=3)?;
    let c = engine.cmd.var(1).as_cell()?;
    let depth = c.depth(i);
    engine.cc.stack.push_int(depth);
    Ok(())
}

/// CHASHI (c - x), returns the hash i of Cell c.
pub fn execute_chashi(engine: &mut Engine) -> Status {
    engine
        .load_instruction(Instruction::new("CHASHI").set_opts(InstructionOptions::Length(0..4)))?;
    fetch_stack(engine, 1)?;
    let i = engine.cmd.length();
    let c = engine.cmd.var(0).as_cell()?;
    let hash = c.hash(i);
    let hash_int = IntegerData::from_unsigned_bytes_be(hash.as_slice());
    engine.cc.stack.push_int(hash_int);
    Ok(())
}

/// CHASHIX (c i - x), returns the hash i of Cell c.
pub fn execute_chashix(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CHASHIX"))?;
    fetch_stack(engine, 2)?;
    let i = engine.cmd.var(0).as_integer_value(0..=3)?;
    let c = engine.cmd.var(1).as_cell()?;
    let hash = c.hash(i);
    let hash_int = IntegerData::from_unsigned_bytes_be(hash.as_slice());
    engine.cc.stack.push_int(hash_int);
    Ok(())
}

/// CLEVEL (c - x), returns the level of Cell c.
pub fn execute_clevel(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CLEVEL"))?;
    fetch_stack(engine, 1)?;
    let c = engine.cmd.var(0).as_cell()?;
    engine.cc.stack.push_int(c.level() as u32);
    Ok(())
}

/// CLEVELMASK (c - x), returns the level mask of Cell c.
pub fn execute_clevel_mask(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CLEVELMASK"))?;
    fetch_stack(engine, 1)?;
    let c = engine.cmd.var(0).as_cell()?;
    engine.cc.stack.push_int(c.level_mask().mask() as u32);
    Ok(())
}

/// SDEPTH (s - x), returns the depth of Slice s.
pub fn execute_sdepth(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SDEPTH"))?;
    fetch_stack(engine, 1)?;
    let mut depth = 0;
    let s = engine.cmd.var(0).as_slice()?;
    let n = s.remaining_references();
    for i in 0..n {
        depth = std::cmp::max(depth, 1 + s.reference(i)?.depth(MAX_LEVEL));
    }
    engine.cc.stack.push_int(depth);
    Ok(())
}
