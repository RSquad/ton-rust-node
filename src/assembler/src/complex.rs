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
use super::{
    convert::to_big_endian_octet_string,
    errors::{OperationError, ParameterError, ToOperationParameterError},
    parse::*,
    CompileResult, Engine, EnsureParametersCountInRange, Unit, Units,
};
use crate::{
    debug::{DbgNode, DbgPos},
    DbgInfo,
};
use num::{BigInt, Integer, Num};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    ops::Range,
};
use ton_block::{error, BuilderData, Cell, HashmapE, HashmapType, SliceData, Status};

trait CommandBehaviourModifier {
    fn modify(code: Vec<u8>) -> Vec<u8>;
}

struct Signaling {}
struct Quiet {}

impl CommandBehaviourModifier for Signaling {
    fn modify(code: Vec<u8>) -> Vec<u8> {
        code
    }
}

impl CommandBehaviourModifier for Quiet {
    fn modify(code: Vec<u8>) -> Vec<u8> {
        let mut code = code;
        code.insert(0, 0xB7);
        code
    }
}

fn compile_with_register(
    register: &str,
    symbol: char,
    range: Range<isize>,
    code: &[u8],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    let reg = parse_register(register, symbol, range).parameter("arg 0")? as u8;
    let mut ret = code.to_vec();
    ret[code.len() - 1] |= reg;
    destination.write_command(ret.as_slice(), DbgNode::from(pos))
}

fn compile_with_any_register(
    register: &str,
    code_stack_short: &[u8],
    code_stack_long: &[u8],
    code_ctrls: &[u8],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_with_register(register, 'S', 0..16, code_stack_short, destination, pos.clone()).or_else(
        |e| {
            if let OperationError::Parameter(_, ParameterError::UnexpectedType) = e {
                compile_with_register(register, 'C', 0..16, code_ctrls, destination, pos.clone())
            } else if let OperationError::Parameter(_, ParameterError::OutOfRange) = e {
                compile_with_register(
                    register,
                    'S',
                    16..256,
                    code_stack_long,
                    destination,
                    pos.clone(),
                )
            } else {
                Err(e)
            }
        },
    )
}

fn compile_call(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let number = parse_const_u14(par[0]).parameter("Number")?;
    if number < 256 {
        destination.write_command(&[0xF0, number as u8], DbgNode::from(pos))
    } else if number < 16384 {
        let hi = 0x3F & ((number / 256) as u8);
        let lo = (number % 256) as u8;
        destination.write_command(&[0xF1, hi, lo], DbgNode::from(pos))
    } else {
        Err(ParameterError::OutOfRange.parameter("Number"))
    }
}

fn compile_ref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    command: &[u8],
    pos: DbgPos,
) -> CompileResult {
    if engine.line_no == 0 && engine.char_no == 0 {
        // the case of instruction form without an argument
        return destination.write_command(command, DbgNode::from(pos));
    }
    par.assert_len(1)?;
    let (cont, dbg) =
        engine.compile(par[0]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    let dbg2 = DbgNode::from_ext(pos, vec![dbg]);
    destination.write_composite_command(command, vec![cont], dbg2)
}

fn compile_callref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xDB, 0x3C], pos)
}

fn compile_jmpref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xDB, 0x3D], pos)
}

fn compile_ifref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x00], pos)
}

fn compile_ifnotref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x01], pos)
}

fn compile_ifjmpref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x02], pos)
}

fn compile_ifnotjmpref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x03], pos)
}

fn compile_ifrefelse(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x0D], pos)
}

fn compile_ifelseref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0xE3, 0x0E], pos)
}

fn compile_ifrefelseref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    if engine.line_no == 0 && engine.char_no == 0 {
        // the case of instruction form without an argument
        return destination.write_command(&[0xE3, 0x0F], DbgNode::from(pos));
    }
    par.assert_len(2)?;
    let (cont1, dbg1) =
        engine.compile(par[0]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    let (cont2, dbg2) =
        engine.compile(par[1]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    let dbg = DbgNode::from_ext(pos, vec![dbg1, dbg2]);
    destination.write_composite_command(&[0xE3, 0x0F], vec![cont1, cont2], dbg)
}

fn build_code_dict(
    engine: &mut Engine,
    dict_key_bitlen: usize,
    tokens: &str,
) -> Result<(Vec<BuilderData>, DbgNode), OperationError> {
    let tokens = tokens
        .split(&[' ', '\t', '\n', '\r', ',', '='])
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>();
    if tokens.len().is_odd() {
        return Err(OperationError::CodeDictConstruction("Odd number of tokens".to_string()));
    }

    let mut map = HashMap::new();
    let mut dict = HashmapE::with_bit_len(dict_key_bitlen);
    let mut info = DbgInfo::default();
    for pair in tokens.chunks(2) {
        // parse the key
        let key = pair[0];
        if !key.to_ascii_lowercase().starts_with('x') {
            return Err(OperationError::CodeDictConstruction(format!(
                "key {key} should start with 'x'"
            )));
        }
        let key_slice = SliceData::from_string(&key[1..])
            .map_err(|_| ParameterError::UnexpectedType.parameter("key"))?;
        if key_slice.remaining_bits() != dict_key_bitlen {
            return Err(OperationError::CodeDictConstruction(format!(
                "key {key} should have {dict_key_bitlen} bits"
            )));
        }

        // get an assembled fragment by the name
        let name = pair[1];
        let (value_cell, mut value_dbg) = engine
            .named_units
            .get(name)
            .ok_or(OperationError::CodeDictConstruction(format!("Fragment {name} isn't defined")))?
            .clone()
            .finalize();
        let value_slice = SliceData::load_cell(value_cell).unwrap();

        // try setting value slice as is, otherwise set as a cell
        if dict.set(key_slice.clone(), &value_slice.clone()).is_ok() {
            map.insert(key_slice.clone(), (value_dbg, value_slice.clone()));
        } else {
            let value_cell = value_slice.clone().into_cell()?;
            info.append(&mut value_dbg);
            dict.setref(key_slice.clone(), value_cell)
                .map_err(|e| OperationError::CodeDictConstruction(e.to_string()))?;
        }
    }

    // update debug info
    for (key, (mut value_dbg, value_slice)) in map {
        let value_slice_after = dict
            .get(key.clone())
            .map_err(|e| OperationError::CodeDictConstruction(e.to_string()))?
            .ok_or(OperationError::CodeDictConstruction(format!(
                "Value for key {key} is not found"
            )))?;
        adjust_debug_map(&mut value_dbg, value_slice, value_slice_after)
            .map_err(|e| OperationError::CodeDictConstruction(e.to_string()))?;
        info.append(&mut value_dbg);
    }

    let dict_cell = dict.data().cloned().unwrap_or_default();
    let b = BuilderData::from_cell(&dict_cell)
        .map_err(|_| ParameterError::UnexpectedType.parameter("parameter"))?;
    Ok((vec![b], make_dbgnode(dict_cell, info)))
}

fn compile_dictpushconst(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    let len = par.len();
    if len < 1 {
        par.assert_len(1)?;
    }
    let dict_key_bitlen = parse_const_u10(par[0])
        .map_err(|e| OperationError::CodeDictConstruction(e.to_string()))?
        as usize;
    let prefix = [0xF4, 0xA4 | (dict_key_bitlen >> 8) as u8, dict_key_bitlen as u8];
    if len == 1 {
        destination.write_command(&prefix, DbgNode::from(pos))
    } else if len == 2 {
        let (data, dbg_node) = build_code_dict(engine, dict_key_bitlen, par[1])?;
        let mut dbg = DbgNode::from(pos);
        dbg.append_node(dbg_node);
        destination.write_composite_command(&prefix, data, dbg)
    } else {
        par.assert_len(2)
    }
}

fn compile_pushref(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0x88], pos)
}

fn compile_pushrefslice(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0x89], pos)
}

fn compile_pushrefcont(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_ref(engine, par, destination, &[0x8A], pos)
}

fn compile_pop(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    compile_with_any_register(par[0], &[0x30], &[0x57, 0x00], &[0xED, 0x50], destination, pos)
}

fn compile_push(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    compile_with_any_register(par[0], &[0x20], &[0x56, 0x00], &[0xED, 0x40], destination, pos)
}

fn write_pushcont(
    cont: BuilderData,
    dbg: DbgNode,
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    let r = cont.references_used() as u8;
    if r > 3 {
        return Err(OperationError::NotFitInSlice);
    }
    let x = cont.data().len() as u8;
    if x > 127 - 2 {
        return Err(OperationError::NotFitInSlice);
    }
    // 1000111r rxxxxxxx ccc...
    let mut code = vec![0x8e | ((r & 2) >> 1), ((r & 1) << 7) | x];
    let mut dbg2 = DbgNode::from(pos);
    dbg2.inline_node(code.len() * 8, dbg);
    code.extend_from_slice(cont.data());
    let mut refs = Vec::with_capacity(cont.references().len());
    for r in cont.references() {
        refs.push(BuilderData::from_cell(r).map_err(|_| OperationError::NotFitInSlice)?);
    }

    destination.write_composite_command(&code, refs, dbg2)
}

fn compile_pushcont(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    if engine.line_no == 0 && engine.char_no == 0 {
        return Err(OperationError::MissingBlock);
    }
    par.assert_len(1)?;
    let (cont, dbg) =
        engine.compile(par[0]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    if cont.references_used() > 0 {
        write_pushcont(cont.clone(), dbg.clone(), destination, pos.clone()).or_else(|_| {
            let dbg2 = DbgNode::from_ext(pos, vec![dbg]);
            destination.write_composite_command(&[0x8E, 0x80], vec![cont], dbg2)
        })
    } else {
        let n = cont.data().len();
        if n <= 15 {
            let mut command = vec![0x90 | n as u8];
            let mut dbg2 = DbgNode::from(pos);
            dbg2.inline_node(command.len() * 8, dbg);
            command.extend_from_slice(cont.data());
            destination.write_command(command.as_slice(), dbg2)
        } else if n <= 125 {
            let mut command = vec![0x8E, n as u8];
            let mut dbg2 = DbgNode::from(pos);
            dbg2.inline_node(command.len() * 8, dbg);
            command.extend_from_slice(cont.data());
            destination.write_command(command.as_slice(), dbg2)
        } else if n <= 127 {
            //We cannot put command and code in one cell, because it will
            //be more than 1023 bits: 127 bytes (pushcont data) + 2 bytes(opcode).
            //Write as r = 1 and xx = 0x00.
            let dbg2 = DbgNode::from_ext(pos, vec![dbg]);
            destination.write_composite_command(&[0x8E, 0x80], vec![cont], dbg2)
        } else {
            log::error!(target: "compile", "Maybe cell longer than 1024 bit?");
            Err(OperationError::NotFitInSlice)
        }
    }
}

fn compile_callxargs(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(2)?;
    let pargs = parse_const_u4(par[0]).parameter("pargs")?;
    if par[1] == "-1" {
        destination.write_command(&[0xDB, pargs & 0x0F], DbgNode::from(pos))
    } else {
        let rargs = parse_const_i4(par[1]).parameter("rargs")?;
        destination
            .write_command(&[0xDA, ((pargs & 0x0F) << 4) | (rargs & 0x0F)], DbgNode::from(pos))
    }
}

struct Div<M: CommandBehaviourModifier>(PhantomData<M>);

macro_rules! div_variant {
    (@resolve $command:ident => $code: expr) => {
        impl<M: CommandBehaviourModifier> Div<M> {
            pub fn $command(
                _engine: &mut Engine,
                par: &[&str],
                destination: &mut Units,
                pos: DbgPos,
            ) -> CompileResult {
                par.assert_len_in(0..=1)?;
                destination.write_command(
                    &M::modify({
                        if par.len() == 1 {
                            let v = $code | 0b00010000;
                            vec![0xA9, v, parse_const_u8_plus_one(par[0]).parameter("arg 0")?]
                        } else {
                            let v = $code & (!0b00010000);
                            vec![0xA9, v]
                        }
                    }),
                    DbgNode::from(pos)
                )
            }
        }
    };

    ($($command: ident => $code:expr)*) => {
        $(
            div_variant!(@resolve $command => $code);
        )*
    };
}

div_variant!(
    lshiftdiv => 0b11010100
    lshiftdivc => 0b11010110
    lshiftdivr => 0b11000101
    lshiftdivmod => 0b11011100
    lshiftdivmodc => 0b11011110
    lshiftdivmodr => 0b11011101
    lshiftmod => 0b11011000
    lshiftmodc => 0b11011010
    lshiftmodr => 0b11011001
    modpow2 => 0b00111000
    modpow2c => 0b00111010
    modpow2r => 0b00111001
    mulmodpow2 => 0b10111000
    mulmodpow2c => 0b10111010
    mulmodpow2r => 0b10111001
    mulrshift => 0b10110100
    mulrshiftc => 0b10110110
    mulrshiftr => 0b10110101
    mulrshiftmod => 0b10111100
    mulrshiftmodc => 0b10111110
    mulrshiftmodr => 0b10111101
    rshiftc => 0b00110110
    rshiftr => 0b00110101
    rshiftmod => 0b00111100
    rshiftmodr => 0b00111101
    rshiftmodc => 0b00111110
);

impl<M: CommandBehaviourModifier> Div<M> {
    pub fn lshift(
        _engine: &mut Engine,
        par: &[&str],
        destination: &mut Units,
        pos: DbgPos,
    ) -> CompileResult {
        par.assert_len_in(0..=1)?;
        destination.write_command(
            &M::modify({
                if par.len() == 1 {
                    vec![0xAA, parse_const_u8_plus_one(par[0]).parameter("arg 0")?]
                } else {
                    vec![0xAC]
                }
            }),
            DbgNode::from(pos),
        )
    }

    fn rshift(
        _engine: &mut Engine,
        par: &[&str],
        destination: &mut Units,
        pos: DbgPos,
    ) -> CompileResult {
        par.assert_len_in(0..=1)?;
        let command = if par.len() == 1 {
            vec![0xAB, parse_const_u8_plus_one(par[0]).parameter("value")?]
        } else {
            vec![0xAD]
        };
        destination.write_command(&M::modify(command), DbgNode::from(pos))
    }
}

fn compile_setcontargs(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len_in(1..=2)?;
    let rargs = parse_const_u4(par[0]).parameter("register")?;
    let nargs = if par.len() == 2 { parse_const_i4(par[1]).parameter("arg 1")? } else { 0x0F };
    destination.write_command(&[0xEC, ((rargs & 0x0F) << 4) | (nargs & 0x0F)], DbgNode::from(pos))
}

fn compile_pushint(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let (sub_str, radix) = if par[0].len() > 2 && (par[0][0..2].eq("0x") || par[0][0..2].eq("0X")) {
        (par[0][2..].to_string(), 16)
    } else if par[0].len() > 3 && (par[0][0..3].eq("-0x") || par[0][0..3].eq("-0X")) {
        let mut sub_str = par[0].to_string();
        sub_str.replace_range(1..3, "");
        (sub_str, 16)
    } else {
        (par[0].to_string(), 10)
    };
    destination.write_command(
        match i32::from_str_radix(sub_str.as_str(), radix) {
            Ok(number @ -5..=10) => Ok(vec![0x70 | ((number & 0x0F) as u8)]),
            Ok(number @ -128..=127) => Ok(vec![0x80, (number & 0xFF) as u8]),
            Ok(number @ -32768..=32767) => {
                Ok(vec![0x81, ((number >> 8) & 0xFF) as u8, (number & 0xFF) as u8])
            }
            _ => {
                if let Ok(int) = BigInt::from_str_radix(sub_str.as_str(), radix) {
                    if let Some(mut int_bytes) = to_big_endian_octet_string(&int) {
                        let mut bytecode = vec![0x82];
                        bytecode.append(&mut int_bytes);
                        Ok(bytecode)
                    } else {
                        Err(ParameterError::OutOfRange.parameter("arg 0"))
                    }
                } else {
                    Err(ParameterError::OutOfRange.parameter("arg 0"))
                }
            }
        }?
        .as_slice(),
        DbgNode::from(pos),
    )
}

fn compile_bchkbits(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    destination.write_command(
        {
            if par.len() == 1 {
                Ok(vec![0xCF, 0x38, parse_const_u8_plus_one(par[0]).parameter("value")?])
            } else {
                Ok::<Vec<u8>, OperationError>(vec![0xCF, 0x39])
            }
        }?
        .as_slice(),
        DbgNode::from(pos),
    )
}

fn compile_bchkbitsq(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    if par.len() == 1 {
        destination.write_command(
            vec![0xCF, 0x3C, parse_const_u8_plus_one(par[0]).parameter("value")?].as_slice(),
            DbgNode::from(pos),
        )
    } else {
        destination.write_command(&[0xCF, 0x3D], DbgNode::from(pos))
    }
}

fn compile_dumpstr(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    mut buffer: Vec<u8>,
    max_len: usize,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let string = parse_string(par[0]);
    let string = string.as_slice();
    let len = string.len();
    if len > max_len {
        return Err(ParameterError::OutOfRange.parameter(par[0]));
    }
    buffer[1] |= (len - 1 + 16 - max_len) as u8;
    buffer.extend_from_slice(string);
    destination.write_command(buffer.as_slice(), DbgNode::from(pos))
}

fn compile_dumptosfmt(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_dumpstr(engine, par, destination, vec![0xFE, 0xF0], 16, pos)
}

fn compile_logstr(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_dumpstr(engine, par, destination, vec![0xFE, 0xF0, 0x00], 15, pos)
}

fn compile_printstr(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_dumpstr(engine, par, destination, vec![0xFE, 0xF0, 0x01], 15, pos)
}

fn compile_stsliceconst(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    if par[0] == "0" {
        destination.write_command(&[0xCF, 0x81], DbgNode::from(pos))
    } else if par[0] == "1" {
        destination.write_command(&[0xCF, 0x83], DbgNode::from(pos))
    } else {
        let buffer = compile_slice(par[0], vec![0xCF, 0x80], 9, 2, 3).parameter("arg 0")?;
        destination.write_command(buffer.as_slice(), DbgNode::from(pos))
    }
}

fn compile_pushslice(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let buffer = match compile_slice(par[0], vec![0x8B, 0], 8, 0, 4) {
        Ok(buffer) => buffer,
        Err(_) => compile_slice(par[0], vec![0x8D, 0], 8, 3, 7).parameter("arg 0")?,
    };
    destination.write_command(buffer.as_slice(), DbgNode::from(pos))
}

fn compile_xchg(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len_in(0..=2)?;
    if par.is_empty() {
        destination.write_command(&[0x01], DbgNode::from(pos))
    } else if par.len() == 1 {
        compile_with_register(par[0], 'S', 1..16, &[0x00], destination, pos)
    } else {
        // 2 parameters
        let reg1 = parse_register(par[0], 'S', 0..16).parameter("arg 0")? as u8;
        let reg2 = parse_register(par[1], 'S', 0..256).parameter("arg 1")? as u8;
        if reg1 >= reg2 {
            Err(OperationError::LogicErrorInParameters("arg 1 should be greater than arg 0"))
        } else if reg1 == 0 {
            if reg2 <= 15 {
                // XCHG s0, si == XCHG si
                destination.write_command(&[reg2], DbgNode::from(pos))
            } else {
                destination.write_command(&[0x11, reg2], DbgNode::from(pos))
            }
        } else if reg1 == 1 {
            if (2..=15).contains(&reg2) {
                destination.write_command(&[0x10 | reg2], DbgNode::from(pos))
            } else {
                Err(ParameterError::OutOfRange.parameter("Register 2"))
            }
        } else if reg2 > 15 {
            Err(ParameterError::OutOfRange.parameter("Register 2"))
        } else {
            destination
                .write_command(&[0x10, ((reg1 << 4) & 0xF0) | (reg2 & 0x0F)], DbgNode::from(pos))
        }
    }
}

fn compile_throw_helper(
    par: &[&str],
    short_opcode: u8,
    long_opcode: u8,
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let number = parse_const_u11(par[0]).parameter("Number")?;
    destination.write_command(
        {
            if number < 64 {
                let number = number as u8;
                Ok(vec![0xF2, short_opcode | number])
            } else if number < 2048 {
                let hi = long_opcode | ((number / 256) as u8);
                let lo = (number % 256) as u8;
                Ok(vec![0xF2, hi, lo])
            } else {
                Err(ParameterError::OutOfRange.parameter("Number"))
            }
        }?
        .as_slice(),
        DbgNode::from(pos),
    )
}

pub(super) fn compile_slice(
    par: &str,
    mut prefix: Vec<u8>,
    offset: usize,
    r: usize,
    x: usize,
) -> std::result::Result<Vec<u8>, ParameterError> {
    // prefix - offset..r..x - data
    let shift = (offset + r + x) % 8;
    let mut buffer = parse_slice(par, shift)?;
    let len = buffer.len() as u8 - 1;
    if len >= (1 << x) {
        return Err(ParameterError::OutOfRange);
    }
    if (offset % 8) + r + x < 8 {
        // a tail of the prefix and a start of the data are in a same byte
        buffer[0] |= prefix.pop().unwrap();
    }
    prefix.append(&mut buffer);
    // skip r writing - no references writing
    if shift < x {
        prefix[(offset + r) / 8] |= len >> shift
    }
    if shift != 0 {
        prefix[(offset + r + x) / 8] |= len << (8 - shift)
    }
    Ok(prefix)
}

fn compile_sdbegins(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    // Regular version have special two aliaces: SDBEGINS '0', SDBEGINS '1'
    if par[0] == "0" {
        destination.write_command(&[0xD7, 0x28, 0x02], DbgNode::from(pos))
    } else if par[0] == "1" {
        destination.write_command(&[0xD7, 0x28, 0x06], DbgNode::from(pos))
    } else {
        let buffer = compile_slice(par[0], vec![0xD7, 0x28], 14, 0, 7).parameter("arg 0")?;
        destination.write_command(buffer.as_slice(), DbgNode::from(pos))
    }
}

fn compile_sdbeginsq(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let buffer = compile_slice(par[0], vec![0xD7, 0x2C], 14, 0, 7).parameter("arg 0")?;
    destination.write_command(buffer.as_slice(), DbgNode::from(pos))
}

fn compile_throw(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_throw_helper(par, 0x00, 0xC0, destination, pos)
}

fn compile_throwif(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_throw_helper(par, 0x40, 0xD0, destination, pos)
}

fn compile_throwifnot(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    compile_throw_helper(par, 0x80, 0xE0, destination, pos)
}

fn compile_blob(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let data = par[0];
    if !data.to_ascii_lowercase().starts_with('x') {
        return Err(ParameterError::UnexpectedType.parameter("parameter"));
    }
    let slice = SliceData::from_string(&data[1..])
        .map_err(|_| ParameterError::UnexpectedType.parameter("parameter"))?;
    destination.write_command_bitstring(slice.storage(), slice.remaining_bits(), DbgNode::from(pos))
}

fn compile_cell(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    if engine.line_no == 0 && engine.char_no == 0 {
        return Err(OperationError::MissingBlock);
    }
    par.assert_len(1)?;
    let (cont, dbg) =
        engine.compile(par[0]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    let mut dbg2 = DbgNode::default();
    dbg2.append_node(dbg);
    destination.write_composite_command(&[], vec![cont], dbg2)
}

fn compile_inline(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;
    let name = par[0];
    if let Some(unit) = engine.named_units.get(name) {
        destination.write_unit(unit.clone())
    } else {
        Err(OperationError::FragmentIsNotDefined(name.to_string()))
    }
}

fn compile_code_dict_cell(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(2)?;
    let dict_key_bitlen =
        par[0].parse::<usize>().map_err(|e| OperationError::CodeDictConstruction(e.to_string()))?;
    let (data, dbg_node) = build_code_dict(engine, dict_key_bitlen, par[1])?;
    let mut dbg = DbgNode::default();
    dbg.append_node(dbg_node);
    destination.write_composite_command(&[], data, dbg)
}

fn adjust_debug_map(map: &mut DbgInfo, before: SliceData, after: SliceData) -> Status {
    let hash_before = before.cell()?.repr_hash();
    let hash_after = after.cell()?.repr_hash();
    let entry_before =
        map.remove(&hash_before).ok_or_else(|| error!("Failed to remove old value."))?;
    let adjustment = after.pos();
    let mut entry_after = BTreeMap::new();
    for (k, v) in entry_before {
        entry_after.insert(k + adjustment, v);
    }
    map.insert(hash_after, entry_after);
    Ok(())
}

struct DbgNodeMaker {
    info: DbgInfo,
}

impl DbgNodeMaker {
    fn new(info: DbgInfo) -> Self {
        Self { info }
    }
    fn make(&self, cell: Cell) -> DbgNode {
        let mut node = DbgNode::default();
        if let Some(map) = self.info.get(&cell.repr_hash()) {
            for (offset, pos) in map {
                node.offsets.push((*offset, pos.clone()))
            }
        }
        for r in 0..cell.references_count() {
            let child = cell.reference(r).unwrap();
            let child_node = self.make(child);
            node.append_node(child_node);
        }
        node
    }
}

fn make_dbgnode(cell: Cell, dbginfo: DbgInfo) -> DbgNode {
    DbgNodeMaker::new(dbginfo).make(cell)
}

fn compile_inline_computed_cell(
    engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(2)?;

    let name = par[0];
    let (code, mut _value_dbg) = engine
        .named_units
        .get(name)
        .ok_or(OperationError::FragmentIsNotDefined(name.to_string()))?
        .clone()
        .finalize();

    let param2 = par[1];
    let capabilities = if param2.to_ascii_lowercase().starts_with("0x") {
        u64::from_str_radix(&param2[2..], 16)
            .map_err(|_| ParameterError::NotSupported.parameter("capabilities"))?
    } else {
        par[1].parse::<u64>().map_err(|_| ParameterError::NotSupported.parameter("capabilities"))?
    };

    // initialize and run vm
    let mut vm = ton_vm::executor::Engine::with_capabilities(capabilities)
        .setup(code, None, None, None, vec![])
        .expect("Failed to load cell data");
    match vm.execute() {
        Err(_e) => return Err(OperationError::CellComputeError),
        Ok(code) => {
            if code != 0 {
                return Err(OperationError::CellComputeError);
            }
        }
    };

    // get a cell from the top of the stack
    let cell = vm.stack().get(0)?.as_cell().map_err(|_| OperationError::CellComputeNotACell)?;

    // pull refs and data from the cell separately
    let mut refs = Vec::new();
    for r in 0..cell.references_count() {
        let c = cell.reference(r).map_err(|_| OperationError::CellComputeInternal)?;
        let b = BuilderData::from_cell(&c).map_err(|_| OperationError::CellComputeInternal)?;
        refs.push(b);
    }
    let slice = SliceData::load_cell_ref(cell).map_err(|_| OperationError::CellComputeInternal)?;
    let dbg_node = make_dbgnode(cell.clone(), DbgInfo::default());

    // write the cell's data and refs
    destination.write_command_bitstring(
        slice.storage(),
        slice.remaining_bits(),
        DbgNode::default(),
    )?;
    destination.write_composite_command(&[], refs, dbg_node)
}

fn compile_fragment(
    engine: &mut Engine,
    par: &[&str],
    _destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(2)?;
    let name = par[0];
    let (builder, dbg) =
        engine.compile(par[1]).map_err(|e| OperationError::Nested(Box::new(e)))?.finalize();
    let unit = Unit::new(builder, dbg);
    if engine.named_units.insert(name.to_string(), unit).is_some() {
        return Err(OperationError::FragmentIsAlreadyDefined(name.to_string()));
    }
    engine.dbgpos = None;
    Ok(())
}

fn compile_loc(
    engine: &mut Engine,
    par: &[&str],
    _destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(2)?;
    let filename = par[0];
    let line = par[1]
        .parse::<usize>()
        .map_err(|_| ParameterError::NotSupported.parameter("line number"))?;
    if line == 0 {
        engine.dbgpos = None;
    } else {
        engine.dbgpos = Some(DbgPos { filename: filename.to_string(), line });
    }
    Ok(())
}

fn compile_library_cell(
    _engine: &mut Engine,
    par: &[&str],
    destination: &mut Units,
    _pos: DbgPos,
) -> CompileResult {
    par.assert_len(1)?;

    let hash = hex::decode(par[0]).map_err(|e| OperationError::Internal(e.to_string()))?;

    let mut b = BuilderData::with_raw(vec![0x02], 8)?;
    b.append_raw(hash.as_slice(), 256)?;
    b.set_type(ton_block::CellType::LibraryReference);

    let mut dbg = DbgNode::default();
    dbg.append_node(DbgNode::default());
    destination.write_composite_command(&[], vec![b], dbg)
}

// Compilation engine *********************************************************

impl Engine {
    #[rustfmt::skip]
    pub fn add_complex_commands(&mut self) {
        // Alphabetically sorted
        self.handlers.insert("-ROLL",          Engine::ROLLREV);
        self.handlers.insert("-ROLLX",         Engine::ROLLREVX);
        self.handlers.insert("-ROT",           Engine::ROTREV);
        self.handlers.insert("2DROP",          Engine::DROP2);
        self.handlers.insert("2DUP",           Engine::DUP2);
        self.handlers.insert("2OVER",          Engine::OVER2);
        self.handlers.insert("2ROT",           Engine::ROT2);
        self.handlers.insert("2SWAP",          Engine::SWAP2);
        self.handlers.insert("CALL",           compile_call);
        self.handlers.insert("CALLDICT",       compile_call);
        self.handlers.insert("CALLREF",        compile_callref);
        self.handlers.insert("CALLXARGS",      compile_callxargs);
        self.handlers.insert("BCHKBITS",       compile_bchkbits);
        self.handlers.insert("BCHKBITSQ",      compile_bchkbitsq);
        self.handlers.insert("DEBUGSTR",       compile_dumptosfmt);
        self.handlers.insert("DUMPTOSFMT",     compile_dumptosfmt);
        self.handlers.insert("IFREF",          compile_ifref);
        self.handlers.insert("IFNOTREF",       compile_ifnotref);
        self.handlers.insert("IFJMPREF",       compile_ifjmpref);
        self.handlers.insert("IFNOTJMPREF",    compile_ifnotjmpref);
        self.handlers.insert("IFREFELSE",      compile_ifrefelse);
        self.handlers.insert("IFELSEREF",      compile_ifelseref);
        self.handlers.insert("IFREFELSEREF",   compile_ifrefelseref);
        self.handlers.insert("DICTPUSHCONST",  compile_dictpushconst);
        self.handlers.insert("JMPDICT",        Engine::JMP);
        self.handlers.insert("JMPREF",         compile_jmpref);
        self.handlers.insert("LOGSTR",         compile_logstr);
        self.handlers.insert("LSHIFT",         Div::<Signaling>::lshift);
        self.handlers.insert("LSHIFTDIV",      Div::<Signaling>::lshiftdiv);
        self.handlers.insert("LSHIFTDIVC",     Div::<Signaling>::lshiftdivc);
        self.handlers.insert("LSHIFTDIVMOD",   Div::<Signaling>::lshiftdivmod);
        self.handlers.insert("LSHIFTDIVMODC",  Div::<Signaling>::lshiftdivmodc);
        self.handlers.insert("LSHIFTDIVMODR",  Div::<Signaling>::lshiftdivmodr);
        self.handlers.insert("LSHIFTDIVR",     Div::<Signaling>::lshiftdivr);
        self.handlers.insert("LSHIFTMOD",      Div::<Signaling>::lshiftmod);
        self.handlers.insert("LSHIFTMODC",     Div::<Signaling>::lshiftmodc);
        self.handlers.insert("LSHIFTMODR",     Div::<Signaling>::lshiftmodr);
        self.handlers.insert("MODPOW2",        Div::<Signaling>::modpow2);
        self.handlers.insert("MODPOW2C",       Div::<Signaling>::modpow2c);
        self.handlers.insert("MODPOW2R",       Div::<Signaling>::modpow2r);
        self.handlers.insert("MULMODPOW2",     Div::<Signaling>::mulmodpow2);
        self.handlers.insert("MULMODPOW2C",    Div::<Signaling>::mulmodpow2c);
        self.handlers.insert("MULMODPOW2R",    Div::<Signaling>::mulmodpow2r);
        self.handlers.insert("MULRSHIFT",      Div::<Signaling>::mulrshift);
        self.handlers.insert("MULRSHIFTC",     Div::<Signaling>::mulrshiftc);
        self.handlers.insert("MULRSHIFTMOD",   Div::<Signaling>::mulrshiftmod);
        self.handlers.insert("MULRSHIFTMODC",  Div::<Signaling>::mulrshiftmodc);
        self.handlers.insert("MULRSHIFTMODR",  Div::<Signaling>::mulrshiftmodr);
        self.handlers.insert("MULRSHIFTR",     Div::<Signaling>::mulrshiftr);
        self.handlers.insert("POP",            compile_pop);
        self.handlers.insert("PRINTSTR",       compile_printstr);
        self.handlers.insert("PUSH",           compile_push);
        self.handlers.insert("PUSHCONT",       compile_pushcont);
        self.handlers.insert("PUSHINT",        compile_pushint);
        self.handlers.insert("PUSHREF",        compile_pushref);
        self.handlers.insert("PUSHREFCONT",    compile_pushrefcont);
        self.handlers.insert("PUSHSLICE",      compile_pushslice);
        self.handlers.insert("PUSHREFSLICE",   compile_pushrefslice);
        self.handlers.insert("SETCONTARGS",    compile_setcontargs);
        self.handlers.insert("SWAP",           compile_xchg);
        self.handlers.insert("QLSHIFT",        Div::<Quiet>::lshift);
        self.handlers.insert("QLSHIFTDIV",     Div::<Quiet>::lshiftdiv);
        self.handlers.insert("QLSHIFTDIVC",    Div::<Quiet>::lshiftdivc);
        self.handlers.insert("QLSHIFTDIVMOD",  Div::<Quiet>::lshiftdivmod);
        self.handlers.insert("QLSHIFTDIVMODC", Div::<Quiet>::lshiftdivmodc);
        self.handlers.insert("QLSHIFTDIVMODR", Div::<Quiet>::lshiftdivmodr);
        self.handlers.insert("QLSHIFTDIVR",    Div::<Quiet>::lshiftdivr);
        self.handlers.insert("QLSHIFTMOD",     Div::<Quiet>::lshiftmod);
        self.handlers.insert("QLSHIFTMODC",    Div::<Quiet>::lshiftmodc);
        self.handlers.insert("QLSHIFTMODR",    Div::<Quiet>::lshiftmodr);
        self.handlers.insert("QMODPOW2",       Div::<Quiet>::modpow2);
        self.handlers.insert("QMODPOW2C",      Div::<Quiet>::modpow2c);
        self.handlers.insert("QMODPOW2R",      Div::<Quiet>::modpow2r);
        self.handlers.insert("QMULMODPOW2",    Div::<Quiet>::mulmodpow2);
        self.handlers.insert("QMULMODPOW2C",   Div::<Quiet>::mulmodpow2c);
        self.handlers.insert("QMULMODPOW2R",   Div::<Quiet>::mulmodpow2r);
        self.handlers.insert("QMULRSHIFT",     Div::<Quiet>::mulrshift);
        self.handlers.insert("QMULRSHIFTC",    Div::<Quiet>::mulrshiftc);
        self.handlers.insert("QMULRSHIFTMOD",  Div::<Quiet>::mulrshiftmod);
        self.handlers.insert("QMULRSHIFTMODC", Div::<Quiet>::mulrshiftmodc);
        self.handlers.insert("QMULRSHIFTMODR", Div::<Quiet>::mulrshiftmodr);
        self.handlers.insert("QMULRSHIFTR",    Div::<Quiet>::mulrshiftr);
        self.handlers.insert("QRSHIFT",        Div::<Quiet>::rshift);
        self.handlers.insert("QRSHIFTC",       Div::<Quiet>::rshiftc);
        self.handlers.insert("QRSHIFTMOD",     Div::<Quiet>::rshiftmod);
        self.handlers.insert("QRSHIFTMODC",    Div::<Quiet>::rshiftmodc);
        self.handlers.insert("QRSHIFTMODR",    Div::<Quiet>::rshiftmodr);
        self.handlers.insert("QRSHIFTR",       Div::<Quiet>::rshiftr);
        self.handlers.insert("RSHIFT",         Div::<Signaling>::rshift);
        self.handlers.insert("RSHIFTMOD",      Div::<Signaling>::rshiftmod);
        self.handlers.insert("RSHIFTMODC",     Div::<Signaling>::rshiftmodc);
        self.handlers.insert("RSHIFTMODR",     Div::<Signaling>::rshiftmodr);
        self.handlers.insert("RSHIFTR",        Div::<Signaling>::rshiftr);
        self.handlers.insert("RSHIFTC",        Div::<Signaling>::rshiftc);
        self.handlers.insert("SDBEGINS",       compile_sdbegins);
        self.handlers.insert("SDBEGINSQ",      compile_sdbeginsq);
        self.handlers.insert("SETCONTARGS",    compile_setcontargs);
        self.handlers.insert("STSLICECONST",   compile_stsliceconst);
        self.handlers.insert("THROW",          compile_throw);
        self.handlers.insert("THROWIF",        compile_throwif);
        self.handlers.insert("THROWIFNOT",     compile_throwifnot);
        self.handlers.insert("XCHG",           compile_xchg);
        // Pseudo instructions
        self.handlers.insert(".BLOB",          compile_blob);
        self.handlers.insert(".CELL",          compile_cell);
        self.handlers.insert(".INLINE",        compile_inline);
        self.handlers.insert(".LIBRARY-CELL",  compile_library_cell);

        self.handlers.insert(".CODE-DICT-CELL",       compile_code_dict_cell);
        self.handlers.insert(".INLINE-COMPUTED-CELL", compile_inline_computed_cell);
        self.handlers.insert(".FRAGMENT",             compile_fragment);
        self.handlers.insert(".LOC",                  compile_loc);
    }
}
