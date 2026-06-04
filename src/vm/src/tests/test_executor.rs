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
        engine::Engine,
        math::DivMode,
        serialize_currency_collection,
        types::{Instruction, InstructionOptions},
    },
    stack::{
        integer::{
            behavior::{OperationBehavior, Quiet, Signaling},
            IntegerData,
        },
        Stack, StackItem,
    },
};
use std::collections::HashSet;
use ton_block::{
    BuilderData, Cell, CurrencyCollection, Deserializable, ExceptionCode, IBitstring, SliceData,
    Status,
};

#[test]
fn test_assert_stack() {
    let mut engine = Engine::with_capabilities(0);
    engine.cc.stack.push(int!(0));
    engine.cc.stack.push(int!(-1));
    engine.cc.stack.push(int!(1));
    let mut stack = Stack::new();
    stack.push(int!(0));
    stack.push(int!(-1));
    stack.push(int!(1));
    engine.assert_stack(&stack);
}

#[test]
fn test_next_cmd_failed() {
    let mut engine = Engine::with_capabilities(0);
    engine.next_cmd().expect_err("Should be generated exception for empty code");
}

#[test]
fn test_div_mode_names_not_intersect() {
    let mut set = HashSet::new();
    for flags in 0..=0b11111111 {
        let mode = DivMode::with_flags(flags);
        if mode.shift_parameter() {
            continue;
        }
        if let Ok(name) = mode.command_name() {
            assert!(set.insert(name.to_string()));
        }
    }
}

#[test]
fn test_division_primitives_names() {
    for mode in 0..64 {
        let mode = DivMode::with_flags(mode << 2);
        if let Ok(name) = mode.command_name() {
            // println!("(0b{:08b}, [\"{name}C\", \"{name}\", \"{name}R\"]),", mode.flags);
            assert_eq!(name, mode.command_name_static().unwrap())
        } else {
            assert!(!mode.is_valid(false));
        }
    }
    for mode in 0..64 {
        let mode = DivMode::with_flags(mode << 2);
        if let Ok(name) = mode.command_name() {
            // println!("(0b{:08b}, [\"Q{name}C\", \"Q{name}\", \"Q{name}R\"]),", mode.flags);
            assert_eq!(name, mode.command_name_static().unwrap())
        } else {
            assert!(!mode.is_valid(true));
        }
    }
}

#[test]
fn test_division_primitives_execution() {
    let mut count = 0;
    for flags in 0..=0b11111111 {
        let mode = DivMode::with_flags(flags);
        if !mode.is_valid(false) {
            // println!("Flags: {:#010b}, <NOT IMPLEMENTED>", mode.flags);
            continue;
        }
        test_div_primitive_execution::<Signaling>(&mode);
        if !mode.shift_parameter() {
            count += 1;
        }
    }
    assert_eq!(60, count);
    let mut count = 0;
    for flags in 0..=0b11111111 {
        let mode = DivMode::with_flags(flags);
        if !mode.is_valid(true) {
            // println!("Flags: {:#010b}, <NOT IMPLEMENTED>", mode.flags);
            continue;
        }
        test_div_primitive_execution::<Quiet>(&mode);
        if !mode.shift_parameter() {
            count += 1;
        }
    }
    assert_eq!(60, count);
}

fn get_command_name<T>(name: &str) -> String
where
    T: OperationBehavior,
{
    let mut result = name.to_owned();
    if let Some(str) = T::name_prefix() {
        result.insert_str(0, str)
    };
    result
}

fn command_name_from_mode<T>(mode: &DivMode) -> String
where
    T: OperationBehavior,
{
    match mode.command_name() {
        Ok(name) => get_command_name::<T>(name),
        Err(_) => {
            panic!("Flags: {:#010b}, Cmd: <NOT IMPLEMENTED>", mode.flags)
        }
    }
}

fn test_div_primitive_execution<T>(mode: &DivMode)
where
    T: OperationBehavior,
{
    let _command_name = command_name_from_mode::<T>(mode);
    // println!("Flags: {:#010b}, Cmd: {}", mode.flags, _command_name);

    let mut value = 15;
    let add = 6;
    let mul_shift = 3;
    let div_shift = 1;

    let multiplier: i32 = 1 << mul_shift;
    let divisor: i32 = 1 << div_shift;
    let mut stack = Stack::new();

    stack.push(int!(value));

    if mode.premultiply() && !mode.mul_by_shift() {
        stack.push(int!(multiplier));
    }

    if mode.preadd() {
        stack.push_int(add);
    }

    if !(mode.div_by_shift() && mode.shift_parameter()) {
        stack.push(int!(if mode.div_by_shift() { div_shift } else { divisor }));
    }
    if mode.mul_by_shift() && !mode.shift_parameter() {
        stack.push(int!(mul_shift));
    }

    let code = div_generate_bytecode::<T>(mode, mul_shift as u8, div_shift as u8);
    let mut engine =
        Engine::with_capabilities(0).setup(code, None, Some(stack), None, vec![]).unwrap();

    match engine.execute() {
        Err(e) => panic!("Execute error: {}", e),
        Ok(_) => {
            if mode.premultiply() {
                value *= multiplier
            }
            if mode.preadd() {
                value += add;
            }

            let (expected_quotient, expected_remainder) = IntegerData::from_i32(value)
                .div::<T>(&IntegerData::from_i32(divisor), mode.rounding_strategy().unwrap())
                .unwrap();

            if mode.need_remainder() {
                let actual_remainder_si = engine.cc.stack.drop(0).unwrap();
                let actual_remainder = actual_remainder_si.as_integer().unwrap();
                assert_eq!(expected_remainder, *actual_remainder, "Remainder");
            }

            if mode.need_quotient() {
                let actual_quotient_si = engine.cc.stack.drop(0).unwrap();
                let actual_quotient = actual_quotient_si.as_integer().unwrap();
                assert_eq!(expected_quotient, *actual_quotient, "Quotient");
            }
        }
    }
}

fn div_generate_bytecode<T>(mode: &DivMode, mul_shift: u8, div_shift: u8) -> Cell
where
    T: OperationBehavior,
{
    let mut res = Vec::<u8>::with_capacity(5);
    if T::quiet() {
        res.push(0xB7);
    }

    res.push(0xA9);
    res.push(mode.flags);
    if mode.shift_parameter() && (mode.mul_by_shift() || mode.div_by_shift()) {
        if mode.mul_by_shift() {
            res.push(mul_shift - 1);
        } else {
            res.push(div_shift - 1);
        }
    }

    res.push(0x80);
    BuilderData::with_bitstring(res).unwrap().into_cell().unwrap()
}

fn test_slice(offset: usize, r: usize, x: usize) -> Status {
    let mut builder = BuilderData::default();
    builder.append_bits(0x7A53, offset)?; // prefix of command
    builder.append_bits(0, r)?; // references
    builder.append_bits(2, x)?; // bytes
    builder.append_bits(0, (8 - (offset + r + x) % 8) % 8)?; // remainder of data
    builder.append_bits(0xF837, 16)?; // data 2 bytes
    builder.append_bits(0x34, 8)?; // remainder in code slice

    let mut code = SliceData::load_builder(builder)?;
    // println!("offset: {}, r: {}, x: {}, code: {}", offset, r, x, code);
    let mut engine =
        Engine::with_capabilities(0).setup(code.clone().into_cell()?, None, None, None, vec![])?;
    engine.load_instruction(
        Instruction::new("PUSHCTR").set_opts(InstructionOptions::Bitstring(offset, r, x, 0)),
    )?;

    let slice = engine.cmd.slice().clone();
    assert_eq!(engine.seek_next_cmd().unwrap(), None);

    let mut remainder = code.clone();
    remainder.shrink_data(32..);
    assert_eq!(&remainder, engine.cc.code());

    code.shrink_data(offset + r + x..31);
    assert_eq!(code, slice);
    Ok(())
}

#[test]
fn test_extract_slice() {
    test_slice(9, 2, 3).unwrap(); // STSLICECONST a command, x, r and data in the same byte
    test_slice(6, 0, 7).unwrap();
    test_slice(7, 2, 7).unwrap();
    test_slice(12, 0, 4).unwrap();
    test_slice(8, 0, 4).unwrap();
    test_slice(8, 2, 5).unwrap();
    test_slice(0, 3, 7).unwrap();

    for r in 0..4 {
        for x in 2..8 {
            let min_offset = std::cmp::min(8, 16 - r - x);
            for offset in min_offset..16 - r - x {
                test_slice(offset, r, x).unwrap();
            }
        }
    }
}

#[test]
fn test_currency_collection_ser() {
    let b1 = serialize_currency_collection(12345678u128, None).unwrap();
    let b2 = BuilderData::with_raw(vec![0x3b, 0xc6, 0x14, 0xe0], 29).unwrap();
    assert_eq!(b1, b2);
}

#[test]
fn test_tvm_serialize_currency_collection() {
    let coins = 1u64 << 63;
    let coins1 = int!(coins).as_coins().unwrap();
    let builder = serialize_currency_collection(coins1, None).unwrap();
    let mut slice = SliceData::load_builder(builder).unwrap();
    let coins1 = CurrencyCollection::construct_from(&mut slice).unwrap();
    let coins2 = CurrencyCollection::with_coins(coins);
    assert_eq!(coins1, coins2);

    assert_eq!(
        tvm_exception_code(&int!(1u128 << 120).as_coins().expect_err("Expect range check error")),
        Some(ExceptionCode::RangeCheckError)
    );
}
