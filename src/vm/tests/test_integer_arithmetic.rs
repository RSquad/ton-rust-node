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
    int,
    stack::{integer::IntegerData, StackItem},
};

#[test]
fn test_pushint() {
    test_case("PUSHINT 0").expect_item(int!(0));
    test_case("PUSHINT 1").expect_item(int!(1));
    test_case("PUSHINT -1").expect_item(int!(-1));
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935",
    )
    .expect_success();
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639936",
    )
    .expect_compilation_failure(CompileError::out_of_range(1, 1, "PUSHINT", "arg 0"));
    test_case(
        "PUSHINT -115792089237316195423570985008687907853269984665640564039457584007913129639936",
    )
    .expect_success();
    test_case(
        "PUSHINT -115792089237316195423570985008687907853269984665640564039457584007913129639937",
    )
    .expect_compilation_failure(CompileError::out_of_range(1, 1, "PUSHINT", "arg 0"));
}

#[test]
fn test_add() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         ADD",
    )
    .expect_item(int!(7));
    test_case(
        "PUSHINT -10
         PUSHINT 2
         ADD",
    )
    .expect_item(int!(-8));
    test_case(
        "PUSHINT -6
         PUSHINT -6
         ADD",
    )
    .expect_item(int!(-12));
    test_case(
        "PUSHINT 0
         PUSHINT 0
         ADD",
    )
    .expect_item(int!(0));

    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT -115792089237316195423570985008687907853269984665640564039457584007913129639935
         ADD",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT -115792089237316195423570985008687907853269984665640564039457584007913129639936
         ADD",
    )
    .expect_item(int!(-1));
}

#[test]
fn test_sub() {
    test_case(
        "PUSHINT 56
         PUSHINT 20
         SUB",
    )
    .expect_item(int!(36));
    test_case(
        "PUSHINT 10
         PUSHINT -35
         SUB",
    )
    .expect_item(int!(45));
    test_case(
        "PUSHINT -9
         PUSHINT 3
         SUB",
    )
    .expect_item(int!(-12));
    test_case(
        "PUSHINT 1
         PUSHINT 17
         SUB",
    )
    .expect_item(int!(-16));
    test_case(
        "PUSHINT 0
         PUSHINT 0
         SUB",
    )
    .expect_item(int!(0));
}

#[test]
fn test_subr() {
    test_case(
        "PUSHINT 10
         PUSHINT 12
         SUBR",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 20
         PUSHINT -7
         SUBR",
    )
    .expect_item(int!(-27));
    test_case(
        "PUSHINT -2
         PUSHINT 10
         SUBR",
    )
    .expect_item(int!(12));
    test_case(
        "PUSHINT 124
         PUSHINT 15
         SUBR",
    )
    .expect_item(int!(-109));
    test_case(
        "PUSHINT 0
         PUSHINT 0
         SUBR",
    )
    .expect_item(int!(0));
}

mod mul {
    use super::*;

    #[test]
    fn test_mul_positive_by_zero() {
        test_case(
            "PUSHINT 2
             PUSHINT 0
             MUL",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_mul_negative_by_zero() {
        test_case(
            "PUSHINT -2
             PUSHINT 0
             MUL",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_mul_positive_numbers() {
        test_case(
            "PUSHINT 2
             PUSHINT 5
             MUL",
        )
        .expect_item(int!(10));
    }

    #[test]
    fn test_mul_opposite_signs() {
        test_case(
            "PUSHINT 1
             PUSHINT -3
             MUL",
        )
        .expect_item(int!(-3));
        test_case(
            "PUSHINT -4
             PUSHINT 7
             MUL",
        )
        .expect_item(int!(-28));
    }

    #[test]
    fn test_mul_negative_numbers() {
        test_case(
            "PUSHINT -4
             PUSHINT -14
             MUL",
        )
        .expect_item(int!(56));
    }
}

#[test]
fn test_addconst() {
    test_case(
        "PUSHINT 2
         ADDCONST 5",
    )
    .expect_item(int!(7));
    test_case(
        "PUSHINT -10
         ADDCONST 2",
    )
    .expect_item(int!(-8));
    test_case(
        "PUSHINT -6
         ADDCONST -6",
    )
    .expect_item(int!(-12));
    test_case(
        "PUSHINT 0
         ADDCONST 0",
    )
    .expect_item(int!(0));
}

#[test]
fn test_addconst_failed_more_127() {
    test_case(
        "PUSHINT 2
         ADDCONST 128",
    )
    .expect_compilation_failure(CompileError::out_of_range(2, 10, "ADDCONST", "arg 0"));
}

#[test]
fn test_addconst_failed_less_minus_128() {
    test_case(
        "PUSHINT 2
         ADDCONST -129",
    )
    .expect_compilation_failure(CompileError::out_of_range(2, 10, "ADDCONST", "arg 0"));
}

#[test]
fn test_mulconst() {
    test_case(
        "PUSHINT 2
         MULCONST 0",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         MULCONST 0 ",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 2
         MULCONST 5",
    )
    .expect_item(int!(10));
    test_case(
        "PUSHINT 1
         MULCONST -3",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT -4
         MULCONST 7",
    )
    .expect_item(int!(-28));
    test_case(
        "PUSHINT -4
         MULCONST -14",
    )
    .expect_item(int!(56));
}

#[test]
fn test_mulconst_failed_more_127() {
    test_case(
        "PUSHINT 2
         MULCONST 128",
    )
    .expect_compilation_failure(CompileError::out_of_range(2, 10, "MULCONST", "arg 0"));
}

#[test]
fn test_mulconst_failed_less_minus_128() {
    test_case(
        "PUSHINT 2
         MULCONST -129",
    )
    .expect_compilation_failure(CompileError::out_of_range(2, 10, "MULCONST", "arg 0"));
}

#[test]
fn test_inc() {
    test_case(
        "PUSHINT 2
         INC",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 0
         INC",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -5
         INC",
    )
    .expect_item(int!(-4));
}

#[test]
fn test_dec() {
    test_case(
        "PUSHINT 5
         DEC",
    )
    .expect_item(int!(4));
    test_case(
        "PUSHINT 0
         DEC",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -8
         DEC",
    )
    .expect_item(int!(-9));
}

mod negate {
    use super::*;
    #[test]
    fn test_negate_positive_number() {
        test_case(
            "PUSHINT 5
             NEGATE",
        )
        .expect_item(int!(-5));
    }

    #[test]
    fn test_zero() {
        test_case(
            "PUSHINT 0
             NEGATE",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_negate_negative_number() {
        test_case(
            "PUSHINT -1
             NEGATE",
        )
        .expect_item(int!(1));
    }
}
