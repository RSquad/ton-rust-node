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
mod common;
use common::*;
use ton_vm::{
    int,
    stack::{integer::IntegerData, StackItem},
};

#[test]
fn super_long_flat_main_function() {
    let mut source = "PUSHINT 0".to_string();
    (0..3000).for_each(|_| source.push_str(" INC"));
    test_case(&source).expect_item(int!(3000));
}

#[test]
fn super_long_continuation_function() {
    let mut source = "PUSHINT 0 PUSHCONT {".to_string();
    (0..3000).for_each(|_| source.push_str(" INC"));
    source.push_str("} JMPX");
    test_case(&source).expect_item(int!(3000));
}

#[test]
fn test_continuation_from_1_to_1000() {
    for i in 1..=1000 {
        let mut source = String::new();
        source += "PUSHINT 0 ";
        source += "PUSHCONT { ";
        (0..i).for_each(|_| source += "INC ");
        source += "} CALLX";
        test_case(&source).expect_item(int!(i));
    }
}

#[test]
fn test_4_sibling_continuations() {
    let n = 127;
    let mut source = String::new();
    source += "PUSHINT 0 ";
    source += "PUSHCONT { ";
    (0..n).for_each(|_| source += "INC ");
    source += "} CALLX ";
    source += "PUSHCONT { ";
    (0..n).for_each(|_| source += "DEC ");
    source += "} CALLX ";
    source += "PUSHCONT { ";
    (0..n).for_each(|_| source += "INC ");
    source += "} CALLX ";
    source += "PUSHCONT { ";
    (0..n).for_each(|_| source += "DEC ");
    source += "} CALLX";
    test_case(&source).expect_item(int!(0));
}
