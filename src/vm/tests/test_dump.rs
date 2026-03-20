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
use ton_assembler::CompileError;
use ton_vm::{
    int,
    stack::{integer::IntegerData, StackItem},
};

mod common;
use common::*;

#[test]
fn test_dump_compilation_failed() {
    test_single_argument_fail("DUMPSTKTOP", -1);
    test_single_argument_fail("DUMPSTKTOP", 0);
    test_single_argument_fail("DUMPSTKTOP", 15);

    test_single_argument_fail("DUMP", -1);
    test_single_argument_fail("DUMP", 15);

    test_single_argument_fail("PRINT", -1);
    test_single_argument_fail("PRINT", 15);
}

#[test]
fn test_dump_compilation_success() {
    test_case("TWO HEXDUMP ").expect_bytecode(vec![0x72, 0xFE, 0x10, 0x80]).expect_success();
    test_case("TWO HEXPRINT").expect_bytecode(vec![0x72, 0xFE, 0x11, 0x80]).expect_success();
    test_case("TWO BINDUMP ").expect_bytecode(vec![0x72, 0xFE, 0x12, 0x80]).expect_success();
    test_case("TWO BINPRINT").expect_bytecode(vec![0x72, 0xFE, 0x13, 0x80]).expect_success();
    test_case("TWO STRDUMP ").expect_bytecode(vec![0x72, 0xFE, 0x14, 0x80]).expect_success();
    test_case("TWO STRPRINT").expect_bytecode(vec![0x72, 0xFE, 0x15, 0x80]).expect_success();
    test_case("TWO DUMP   0").expect_bytecode(vec![0x72, 0xFE, 0x20, 0x80]).expect_success();
    test_case("TWO PRINT  0").expect_bytecode(vec![0x72, 0xFE, 0x30, 0x80]).expect_success();
    test_case("DUMPSTK ").expect_bytecode(vec![0xFE, 0x00, 0x80]).expect_success();
    test_case("DEBUGON ").expect_bytecode(vec![0xFE, 0x1F, 0x80]).expect_success();
    test_case("DEBUGOFF").expect_bytecode(vec![0xFE, 0x1E, 0x80]).expect_success();
    test_case("DUMPSTKTOP 1").expect_bytecode(vec![0xFE, 0x01, 0x80]).expect_success();
    test_case("LOGSTR 1").expect_bytecode(vec![0xFE, 0xF1, 0x00, 0x31, 0x80]).expect_success();
    test_case("LOGFLUSH").expect_bytecode(vec![0xFE, 0xF0, 0x00, 0x80]).expect_success();

    let code = "DUMPTOSFMT 1234567890123456";
    let bytecode = vec![
        0xFE, 0xFF, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33,
        0x34, 0x35, 0x36, 0x80,
    ];
    test_case(code).expect_bytecode(bytecode).expect_success();

    let code = "LOGSTR 123456789012345";
    let bytecode = vec![
        0xFE, 0xFF, 0, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32,
        0x33, 0x34, 0x35, 0x80,
    ];
    test_case(code).expect_bytecode(bytecode).expect_success();

    let code = "PRINTSTR 123456789012345";
    let bytecode = vec![
        0xFE, 0xFF, 1, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32,
        0x33, 0x34, 0x35, 0x80,
    ];
    test_case(code).expect_bytecode(bytecode).expect_success();

    let bytecode = vec![0xFE, 0xF3, 0x01, 0x49, 0x4E, 0x43, 0x80];
    test_case("PRINTSTR INC").expect_bytecode(bytecode).expect_success();

    let bytecode = vec![0xFE, 0xF3, 0x01, 0x46, 0x4f, 0x4F, 0x70, 0x80];
    test_case("PRINTSTR FOO ZERO").expect_bytecode(bytecode).expect_item(int!(0));
}

#[test]
fn test_dump_compilation_error() {
    test_case(
        "\
        PRINTSTR
        INC
    ",
    )
    .expect_compilation_failure(CompileError::missing_params(1, 1, "PRINTSTR"));
    test_case(
        "\n\
        PRINTSTR
        INC
    ",
    )
    .expect_compilation_failure(CompileError::missing_params(2, 1, "PRINTSTR"));
    test_case(
        "\n\
        PRINTSTR
        INC
    ",
    )
    .expect_compilation_failure(CompileError::missing_params(2, 1, "PRINTSTR"));

    test_case(
        "\
        PRINTSTR
        1234567890123456
    ",
    )
    .expect_compilation_failure(CompileError::out_of_range(
        1,
        1,
        "PRINTSTR",
        "1234567890123456",
    ));
    test_case("PRINTSTR 1234567890123456").expect_compilation_failure(CompileError::out_of_range(
        1,
        1,
        "PRINTSTR",
        "1234567890123456",
    ));
    test_case("PRINTSTR FOO BAR").expect_compilation_failure(CompileError::unknown(1, 14, "BAR"));
}

#[test]
fn test_dump_error_stack() {
    test_case("STRDUMP TWO").expect_item(int!(2));
    test_case("DUMP 5 TWO").expect_item(int!(2));
}
