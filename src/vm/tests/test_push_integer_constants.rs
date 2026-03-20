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
use ton_assembler::CompileError;
use ton_vm::{
    boolean, int,
    stack::{
        integer::{behavior::Signaling, IntegerData},
        StackItem,
    },
};

#[test]
fn test_check_bytecode_pushnan() {
    let bytecode = vec![0x83, 0xFF, 0x80];
    test_case("PUSHNAN").expect_bytecode(bytecode.clone());
    test_case("PUSHPOW2 256").expect_bytecode(bytecode);
}

#[test]
fn test_check_bytecode_push_big_positive_integer() {
    let bytecode = vec![
        0x82, 0x68, 0x17, 0xc6, 0xe3, 0xc0, 0x32, 0xf8, 0x90, 0x45, 0xad, 0x74, 0x66, 0x84, 0x04,
        0x5f, 0x15, 0x80,
    ];
    test_case("PUSHINT 123456789123456789123456789123456789").expect_bytecode(bytecode);
}

#[test]
fn test_check_bytecode_push_big_negative_integer() {
    let bytecode = vec![
        0x82, 0x6f, 0xe8, 0x39, 0x1c, 0x3f, 0xcd, 0x07, 0x6f, 0xba, 0x52, 0x8b, 0x99, 0x7b, 0xfb,
        0xa0, 0xeb, 0x80,
    ];
    test_case("PUSHINT -123456789123456789123456789123456789").expect_bytecode(bytecode);
}

//test from spec A.3.1
#[test]
fn test_check_bytecode_push_big_integer_10pow8() {
    let bytecode = vec![0x82, 0x10, 0x05, 0xF5, 0xe1, 0x00, 0x80];
    test_case("PUSHINT 100000000").expect_bytecode(bytecode);
}

#[test]
fn test_execute_push_big_integer() {
    test_case("PUSHINT -12345678").expect_item(int!(-12345678));
    test_case("PUSHINT 12345678").expect_item(int!(12345678));
    test_case("PUSHINT -1234567").expect_item(int!(-1234567));

    test_case("PUSHINT -9124676477647797897998").expect_item(int!(-9124676477647797897998i128));

    test_case("PUSHINT 9124676477647797897998").expect_item(int!(9124676477647797897998i128));
}

#[test]
fn test_pushint_signed() {
    test_case(
        "PUSHINT -5
         PUSHINT 2
         ADD",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT 5
         PUSHINT -2
         ADD",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT -48
         PUSHINT 42
         ADD",
    )
    .expect_item(int!(-6));
    test_case(
        "PUSHINT 48
         PUSHINT -42
         ADD",
    )
    .expect_item(int!(6));
    test_case(
        "PUSHINT -32760
         PUSHINT 32744
         ADD",
    )
    .expect_item(int!(-16));
    test_case(
        "PUSHINT 32760
         PUSHINT -32744
         ADD",
    )
    .expect_item(int!(16));
    test_case(
        "PUSHINT -123456789123456789123456789123456789
         PUSHINT 123456789123456789123456789123456754
         ADD",
    )
    .expect_item(int!(-35));
    test_case(
        "PUSHINT 123456789123456789123456789123456789
         PUSHINT -123456789123456789123456789123456754
         ADD",
    )
    .expect_item(int!(35));
}

#[test]
fn test_pushpow2() {
    test_case("PUSHPOW2 4").expect_item(int!(16));
    test_case("PUSHPOW2 100")
        .expect_item(StackItem::int(IntegerData::one().shl::<Signaling>(100).unwrap()));
}

#[test]
fn test_pushnan() {
    test_case("PUSHPOW2 256").expect_item(int!(nan));
    test_case("PUSHNAN").expect_item(int!(nan));
}

#[test]
fn test_pushpow2dec() {
    test_case("PUSHPOW2DEC 4").expect_item(int!(15));
    test_case("PUSHPOW2DEC 100").expect_item(int!(1267650600228229401496703205375u128));
    test_case("PUSHPOW2DEC 256").expect_item(
        int!(parse "115792089237316195423570985008687907853269984665640564039457584007913129639935")
    );
}

#[test]
fn test_pushnegpow2() {
    test_case("PUSHNEGPOW2 4").expect_item(int!(-16));
    test_case("PUSHNEGPOW2 100").expect_item(int!(-1267650600228229401496703205376i128));
    test_case("PUSHNEGPOW2 256").expect_item(int!(parse "-115792089237316195423570985008687907853269984665640564039457584007913129639936"));
}

#[test]
fn test_pushpow2_outofrange() {
    let oper_error = CompileError::out_of_range(1, 1, "PUSHPOW2", "arg 0");
    test_case("PUSHPOW2 257").expect_compilation_failure(oper_error.clone());
    test_case("PUSHPOW2 0").expect_compilation_failure(oper_error);
}

#[test]
fn test_pushpow2dec_outofrange() {
    let oper_error = CompileError::out_of_range(1, 1, "PUSHPOW2DEC", "arg 0");

    test_case("PUSHPOW2DEC 257").expect_compilation_failure(oper_error.clone());
    test_case("PUSHPOW2DEC 0").expect_compilation_failure(oper_error);
}

#[test]
fn test_pushnegpow2_outofrange() {
    let oper_error = CompileError::out_of_range(1, 1, "PUSHNEGPOW2", "arg 0");

    test_case("PUSHNEGPOW2 257").expect_compilation_failure(oper_error.clone());
    test_case("PUSHNEGPOW2 0").expect_compilation_failure(oper_error);
}

#[test]
fn test_pushpow2_unexpectedtype() {
    test_case("PUSHPOW2 qw")
        .expect_compilation_failure(CompileError::unexpected_type(1, 1, "PUSHPOW2", "arg 0"));
}

#[test]
fn test_pushpow2dec_unexpectedtype() {
    test_case("PUSHPOW2DEC qw").expect_compilation_failure(CompileError::unexpected_type(
        1,
        1,
        "PUSHPOW2DEC",
        "arg 0",
    ));
}

#[test]
fn test_pushnegpow2_unexpectedtype() {
    test_case("PUSHNEGPOW2 qw").expect_compilation_failure(CompileError::unexpected_type(
        1,
        1,
        "PUSHNEGPOW2",
        "arg 0",
    ));
}

#[test]
fn test_pushint_numbers() {
    test_case("ZERO").expect_item(int!(0));
    test_case("ONE").expect_item(int!(1));
    test_case("TWO").expect_item(int!(2));
    test_case("TEN").expect_item(int!(10));
    test_case("FALSE").expect_item(boolean!(false));
    test_case("TRUE").expect_item(boolean!(true));
}

#[test]
fn test_hexadecimal_string() {
    test_case("PUSHINT 0xFF").expect_item(int!(255));
    test_case("PUSHINT 0x12345678").expect_item(int!(305419896));
    test_case("PUSHINT -0x64").expect_item(int!(-100));
    test_case("PUSHINT 0XFF").expect_item(int!(255));
    test_case("PUSHINT 0X12345678").expect_item(int!(305419896));
    test_case("PUSHINT -0x14AC1").expect_item(int!(-84673));
}
