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
use super::{Stack, StackItem};
use ton_block::{BuilderData, SliceData};

#[test]
fn test_push_increases_depth() {
    let mut stack = Stack::new();
    stack.push(StackItem::int(1));
    assert_eq!(stack.depth(), 1)
}

#[test]
fn test_take_returns_elements_from_topmost_to_bottom() {
    let mut stack = Stack::new();
    for i in 0..5 {
        stack.push(StackItem::int(i));
    }
    assert_eq!(stack.get(0).unwrap(), &StackItem::int(4));
    assert_eq!(stack.get(1).unwrap(), &StackItem::int(3));
    assert_eq!(stack.get(2).unwrap(), &StackItem::int(2));
}

#[test]
fn test_fift_output() {
    assert_eq!(StackItem::default().dump_as_fift(), "(null)");
    assert_eq!(StackItem::int(1200000000).dump_as_fift(), "1200000000");
    assert_eq!(StackItem::nan().dump_as_fift(), "NaN");
    let builder = BuilderData::with_bitstring(vec![0x57, 0x74]).unwrap();
    let cell = builder.clone().into_cell().unwrap();
    assert_eq!(
        StackItem::cell(cell.clone()).dump_as_fift(),
        "C{A657BCF14616E598023A10E66EA9B79E3E9CD9F93F338EB6DACE17F475A300F8}"
    );
    assert_eq!(StackItem::builder(builder).dump_as_fift(), "BC{00035774}");
    let builder = BuilderData::with_bitstring(vec![0x57, 0x74, 0x80]).unwrap();
    assert_eq!(StackItem::builder(builder).dump_as_fift(), "BC{00045774}");
    let builder = BuilderData::with_bitstring(vec![0x57, 0x60]).unwrap();
    assert_eq!(StackItem::builder(builder).dump_as_fift(), "BC{00035760}");
    assert_eq!(
        StackItem::slice(SliceData::load_cell(cell).unwrap()).dump_as_fift(),
        "CS{Cell{00035774} bits: 0..13; refs: 0..0}"
    );
    assert_eq!(StackItem::tuple(vec![]).dump_as_fift(), "[]");
    assert_eq!(
        StackItem::tuple(vec![StackItem::nan(), StackItem::int(1234567890)]).dump_as_fift(),
        "[ NaN 1234567890 ]"
    );
}
