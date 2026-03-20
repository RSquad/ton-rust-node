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
        dump::{
            dump_var, execute_dump_bin, execute_dump_hex, execute_dump_stack,
            execute_dump_stack_top, execute_dump_str, execute_dump_string, execute_print_bin,
            execute_print_hex, execute_print_str, BIN, HEX, STR,
        },
        engine::Engine,
    },
    stack::{integer::IntegerData, Stack, StackItem},
};
use ton_block::{BuilderData, SliceData};

#[test]
fn test_dump_var() {
    [0, 15, 23466454, 347387434, 4383434].iter().for_each(|value| {
        assert_eq!(format!("{}", *value), dump_var(&int!(*value), 0));
        assert_eq!(format!("{:X}", *value), dump_var(&int!(*value), HEX));
        assert_eq!(format!("{:b}", *value), dump_var(&int!(*value), BIN));
    });
    [-15, -23466454, -476343874].iter().for_each(|value| {
        assert_eq!(format!("{}", *value), dump_var(&int!(*value), 0));
        assert_eq!(format!("-{:X}", -*value), dump_var(&int!(*value), HEX));
        assert_eq!(format!("-{:b}", -*value), dump_var(&int!(*value), BIN));
    });

    let slice = StackItem::Slice(SliceData::new(vec![0x41, 0x42, 0x43, 0x80]));
    assert_eq!("ABC".to_string(), dump_var(&slice, STR));
    assert_eq!("CS<414243>(0..24)", dump_var(&slice, HEX));
    assert_eq!("CS<010000010100001001000011>(0..24)", dump_var(&slice, BIN));
}

#[test]
fn test_dump_commands() {
    let int = -15;
    let builder = BuilderData::with_raw(vec![0x41, 0x42, 0x43], 24).unwrap(); // ABC
    let mut stack = Stack::new();
    stack.push_builder(builder.clone());
    stack.push(StackItem::Cell(builder.clone().into_cell().unwrap()));
    stack.push(StackItem::Slice(SliceData::load_builder(builder).unwrap()));
    stack.push(int!(int));
    let code = SliceData::new(vec![1, 0, 0x0A, 0x80]).into_cell().unwrap();
    let engine =
        &mut Engine::with_capabilities(0).setup(code, None, Some(stack), None, vec![]).unwrap();
    log::trace!("--- {} as str\n", int);
    execute_dump_str(engine).unwrap();
    log::trace!("--- {} as hex\n", int);
    execute_dump_hex(engine).unwrap();
    log::trace!("--- {} as bin\n", int);
    execute_dump_bin(engine).unwrap();
    log::trace!("--- stack\n");
    execute_dump_stack(engine).unwrap();
    log::trace!("--- top 2 of stack\n");
    assert_eq!(engine.next_cmd().unwrap(), 1);
    execute_dump_stack_top(engine).unwrap();
    log::trace!("--- str, hex, bin\n");
    assert_eq!(engine.next_cmd().unwrap(), 0);
    execute_print_hex(engine).unwrap();
    execute_print_bin(engine).unwrap();
    execute_print_str(engine).unwrap();
    execute_dump_string(engine).unwrap(); // flush with LF
}
