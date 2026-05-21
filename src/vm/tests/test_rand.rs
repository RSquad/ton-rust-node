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
use ton_block::ExceptionCode;
use ton_vm::{
    int,
    stack::{integer::IntegerData, StackItem},
};

mod common;
use common::*;

#[test]
fn test_rand_normal_case() {
    test_case(
        "
        PUSHINT 124711402 ; magic 0x076ef1ea
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        TUPLE 10
        SINGLE
        POP C7
        PUSHINT 1234567890
        SETRAND
        PUSHINT 1234567890
        ADDRAND
        PUSHINT 789000
        RAND
        PUSHINT 191575
        EQUAL
    ",
    )
    .expect_item(int!(-1));
}

#[test]
fn test_randu_normal_case() {
    test_case(
        "
        PUSHINT 124711402 ; magic 0x076ef1ea
        ZERO
        ZERO
        ZERO
        ZERO
        ZERO
        PUSHINT 1234567890
        ZERO
        ZERO
        ZERO
        TUPLE 10
        SINGLE
        POP C7
        PUSHPOW2 255
        SETRAND
        PUSHPOW2DEC 256
        SETRAND
        PUSHINT 1234567890
        SETRAND
        RANDU256
        PUSHINT 55155587004147699562571990193761432594891582513305377283752159430470838410715
        EQUAL
    ",
    )
    .expect_item(int!(-1));
}

#[test]
fn test_rand_error_flow() {
    expect_exception("ADDRAND", ExceptionCode::StackUnderflow);
    expect_exception("SETRAND", ExceptionCode::StackUnderflow);
    expect_exception("RAND", ExceptionCode::StackUnderflow);
    expect_exception("NULL ADDRAND", ExceptionCode::TypeCheckError);
    expect_exception("NULL SETRAND", ExceptionCode::TypeCheckError);
    expect_exception("NULL RAND", ExceptionCode::TypeCheckError);
    expect_exception("PUSHINT -1 SETRAND", ExceptionCode::RangeCheckError);
    expect_exception("PUSHNEGPOW2 256 SETRAND", ExceptionCode::RangeCheckError);
    expect_exception("PUSHNAN SETRAND", ExceptionCode::IntegerOverflow);
}
