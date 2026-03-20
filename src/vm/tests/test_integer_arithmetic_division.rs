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
use ton_block::ExceptionCode;
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

#[test]
fn test_div() {
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIV",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIV",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIV",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 8
         PUSHINT -2
         DIV",
    )
    .expect_item(int!(-4));
    test_case(
        "PUSHINT -16
         PUSHINT 2
         DIV",
    )
    .expect_item(int!(-8));
    test_case(
        "PUSHINT -9
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 2
         PUSHINT 4
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT 1
         DIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -16
         PUSHINT 32
         DIV",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIV",
    )
    .expect_item(int!(-3));
}

#[test]
fn test_div_failed_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 0
         DIV",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_divr() {
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIVR",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(-3));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIVR",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIVR",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIVR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIVR",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 4
         PUSHINT 2
         DIVR",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVR",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 8
         PUSHINT 7
         DIVR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 0
         PUSHINT 7
         DIVR",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         DIVR",
    )
    .expect_item(int!(0));
}

#[test]
fn test_divr_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         DIVR",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_divc() {
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVC",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIVC",
    )
    .expect_item(int!(-2));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIVC",
    )
    .expect_item(int!(-2));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIVC",
    )
    .expect_item(int!(3));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIVC",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIVC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIVC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIVC",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIVC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIVC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 4
         PUSHINT 2
         DIVC",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT 7
         DIVC",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT -7
         DIVC",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT 0
         PUSHINT 7
         DIVC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         DIVC",
    )
    .expect_item(int!(0));
}

#[test]
fn test_divc_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         DIVC",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_mod() {
    test_case(
        "PUSHINT 8
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(-2));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(-2));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 4
         PUSHINT 2
         MOD",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 8
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         MOD",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         MOD",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 8
         PUSHINT 7
         MOD",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 0
         PUSHINT 7
         MOD",
    )
    .expect_item(int!(0));
}

#[test]
fn test_mod_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         MOD",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_adddivmod_success() {
    test_case(
        "PUSHINT 3
         PUSHINT 6
         PUSHINT 4
         ADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         PUSHINT -8
         ADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(-7)));

    test_case(
        "PUSHINT -5
         PUSHINT 2
         PUSHINT 5
         ADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(2)));

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT -5
         ADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(-2)));
    // let mut rng = rand::thread_rng();
    // rand test
    for _ in 0..100 {
        let z: u8 = rand::random();
        if z == 0 {
            continue;
        }
        let w: u8 = rand::random();
        let x: u8 = rand::random();
        let code = format!(
            "
            PUSHINT {x}
            PUSHINT {w}
            PUSHINT {z}
            ADDDIVMOD
        "
        );
        let v = x as i64 + w as i64;
        let q = v / z as i64;
        let r = v % z as i64;
        println!("ADDDIVMOD: ({x} + {w}) / {z} => ({q}, {r})");
        test_case(&code).expect_stack(Stack::new().push(int!(q)).push(int!(r)));
    }
}

#[test]
fn test_adddivmod_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 1
         PUSHINT 0
         ADDDIVMOD",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_divmod() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(0)));
    test_case(
        "PUSHINT 2
         PUSHINT 4
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)));
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(2)));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-3)).push(int!(-1)));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-3)).push(int!(1)));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(-2)));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(-1)));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(1)));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(-2)));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-0)).push(int!(0)));
}

#[test]
fn test_divmod_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         DIVMOD",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_divmodr() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(0)));
    test_case(
        "PUSHINT 2
         PUSHINT 4
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(-2)));
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(-1)));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(-3)).push(int!(-1)));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(-3)).push(int!(1)));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(1)));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(-1)));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(-1)));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(-1)).push(int!(1)));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(1)));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIVMODR",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
}

#[test]
fn test_divmodr_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         DIVMODR",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_divmodc() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(0)));
    test_case(
        "PUSHINT 2
         PUSHINT 4
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(-2)));
    test_case(
        "PUSHINT 8
         PUSHINT 3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(-1)));
    test_case(
        "PUSHINT 8
         PUSHINT -3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(-2)).push(int!(2)));
    test_case(
        "PUSHINT -8
         PUSHINT 3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(-2)).push(int!(-2)));
    test_case(
        "PUSHINT -8
         PUSHINT -3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(1)));
    test_case(
        "PUSHINT 2
         PUSHINT 3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(-1)));
    test_case(
        "PUSHINT 2
         PUSHINT -3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(-2)));
    test_case(
        "PUSHINT -2
         PUSHINT -3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(1)));
    test_case(
        "PUSHINT 0
         PUSHINT 3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
    test_case(
        "PUSHINT 0
         PUSHINT -3
         DIVMODC",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
}

#[test]
fn test_divmodc_failed_div_on_zero() {
    test_case(
        "PUSHINT 8
         PUSHINT 0
         DIVMODC",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_addrshiftmod_success() {
    test_case(
        "PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[1, 3]);
    test_case(
        "PUSHINT 4
         PUSHINT 3
         PUSHINT 0
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[7, 0]);
    test_case(
        "PUSHINT 0
         PUSHINT 3
         PUSHINT 2
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[0, 3]);
    test_case(
        "PUSHINT 3
         PUSHINT 3
         PUSHINT 8
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[0, 6]);
    test_case(
        "PUSHINT 7
         PUSHINT 3
         PUSHINT 2
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[2, 2]);
    test_case(
        "PUSHINT -7
         PUSHINT 2
         PUSHINT 3
         ADDRSHIFTMOD",
    )
    .expect_int_stack(&[-1, 3]);
}

#[test]
fn test_rshift() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         RSHIFT",
    )
    .expect_bytecode(vec![0x74, 0x72, 0xAD, 0x80])
    .expect_item(int!(1));
    test_case(
        "PUSHINT 4
         PUSHINT 0
         RSHIFT",
    )
    .expect_item(int!(4));
    test_case(
        "PUSHINT 0
         PUSHINT 2
         RSHIFT",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 3
         PUSHINT 8
         RSHIFT",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 7
         PUSHINT 2
         RSHIFT",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -2
         PUSHINT 2
         RSHIFT",
    )
    .expect_item(int!(-1));
}

#[test]
fn test_rshiftc() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         RSHIFTC",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 4
         PUSHINT 0
         RSHIFTC",
    )
    .expect_item(int!(4));
    test_case(
        "PUSHINT 0
         PUSHINT 2
         RSHIFTC",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 3
         PUSHINT 8
         RSHIFTC",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 7
         PUSHINT 2
         RSHIFTC",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT -2
         PUSHINT 2
         RSHIFTC",
    )
    .expect_item(int!(0));
}

#[test]
fn test_rshiftr() {
    test_case(
        "PUSHINT 4
         PUSHINT 2
         RSHIFTR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 4
         PUSHINT 0
         RSHIFTR",
    )
    .expect_item(int!(4));
    test_case(
        "PUSHINT 0
         PUSHINT 2
         RSHIFTR",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 3
         PUSHINT 8
         RSHIFTR",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 7
         PUSHINT 2
         RSHIFTR",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT -3
         PUSHINT 2
         RSHIFTR",
    )
    .expect_item(int!(-1));
    test_case(
        "PUSHINT -2
         PUSHINT 2
         RSHIFTR",
    )
    .expect_item(int!(0));
}

#[test]
fn test_modpow2() {
    test_case(
        "PUSHINT 5
         MODPOW2 1",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 6
         MODPOW2 3",
    )
    .expect_item(int!(6));
    test_case(
        "PUSHINT 4
         MODPOW2 1",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT 5
         MODPOW2 2",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT 5
         MODPOW2 3",
    )
    .expect_item(int!(5));
    test_case(
        "PUSHINT 7
         MODPOW2 2",
    )
    .expect_item(int!(3));
}

#[test]
fn test_modpow2_with_zero() {
    test_case(
        "PUSHINT 0
         MODPOW2 5",
    )
    .expect_item(int!(0));
}

#[test]
fn test_muldiv_success() {
    test_case(
        "PUSHINT 4
         PUSHINT 8
         PUSHINT 16
         MULDIV",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 5
         MULDIV",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         PUSHINT 1
         MULDIV",
    )
    .expect_item(int!(-8));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 10
         MULDIV",
    )
    .expect_item(int!(0));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         PUSHINT -1
         MULDIV",
    )
    .expect_item(int!(6));
}

#[test]
fn test_muldiv_failed_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 0
         MULDIV",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_quiet_muldiv_does_not_fail_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 0
         QMULDIV",
    )
    .expect_stack(Stack::new().push(int!(nan)));
}

#[test]
fn test_quiet_muldivmod_does_not_fail_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 0
         QMULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(nan)).push(int!(nan)));
}

#[test]
fn test_muldivr() {
    test_case(
        "PUSHINT 4
         PUSHINT 8
         PUSHINT 16
         MULDIVR",
    )
    .expect_item(int!(2));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 5
         MULDIVR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         PUSHINT 1
         MULDIVR",
    )
    .expect_item(int!(-8));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 10
         MULDIVR",
    )
    .expect_item(int!(1));
    test_case(
        "PUSHINT -2
         PUSHINT 3
         PUSHINT -1
         MULDIVR",
    )
    .expect_item(int!(6));
}

#[test]
fn test_muldivr_failed_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 0
         MULDIVR",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_muladddivmod_success() {
    test_case(
        "PUSHINT 4
         PUSHINT 8
         PUSHINT 1
         PUSHINT 16
         MULADDDIVMOD", // x y w z
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    test_case(
        "PUSHINT 7
         PUSHINT 3
         PUSHINT 1
         PUSHINT 5
         MULADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(4)).push(int!(2)));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         PUSHINT 1
         PUSHINT 1
         MULADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-7)).push(int!(0)));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 1
         PUSHINT 11
         MULADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(6)));
    test_case(
        "PUSHINT -23
         PUSHINT 3
         PUSHINT 2
         PUSHINT -2
         MULADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(33)).push(int!(-1)));
}

#[test]
fn test_muladddivmod_failed_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 3
         PUSHINT 0
         MULADDDIVMOD",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_muladddivmod_514_bit() {
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT 1
         PUSHINT 100
         PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         MULADDDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(100)));
}

#[test]
fn test_muladdrshiftmod_success() {
    test_case(
        "PUSHINT 4
         PUSHINT 8
         PUSHINT 1
         PUSHINT 4
         MULADDRSHIFTMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    test_case(
        "PUSHINT 7
         PUSHINT 3
         PUSHINT 1
         PUSHINT 2
         MULADDRSHIFTMOD",
    )
    .expect_stack(Stack::new().push(int!(5)).push(int!(2)));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         PUSHINT 1
         PUSHINT 0
         MULADDRSHIFTMOD",
    )
    .expect_stack(Stack::new().push(int!(-7)).push(int!(0)));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 1
         PUSHINT 4
         MULADDRSHIFTMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(6)));
}

#[test]
fn test_muladdrshiftmod_514_bit() {
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT 1
         PUSHINT 100
         PUSHINT 255
         MULADDRSHIFTMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(99)));
}

#[test]
fn test_muldivmod() {
    test_case(
        "PUSHINT 4
         PUSHINT 8
         PUSHINT 16
         MULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(0)));
    test_case(
        "PUSHINT 7
         PUSHINT 3
         PUSHINT 5
         MULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(4)).push(int!(1)));
    test_case(
        "PUSHINT -2
         PUSHINT 4
         PUSHINT 1
         MULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(-8)).push(int!(0)));
    test_case(
        "PUSHINT 1
         PUSHINT 5
         PUSHINT 11
         MULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(5)));
    test_case(
        "PUSHINT -23
         PUSHINT 3
         PUSHINT -2
         MULDIVMOD",
    )
    .expect_stack(Stack::new().push(int!(34)).push(int!(-1)));
}

#[test]
fn test_muldivmod_failed_div_on_zero() {
    test_case(
        "PUSHINT 2
         PUSHINT 5
         PUSHINT 0
         MULDIVMOD",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_muldivmod_514_bit() {
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
         MULDIVMOD",
    ).expect_stack(Stack::new()
        .push(int!(parse "115792089237316195423570985008687907853269984665640564039457584007913129639935"))
        .push(int!(0)));
}

mod mulrshift {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_bytecode(vec![0x70, 0x71, 0x72, 0xa9, 0xa4, 0x80])
        .expect_item(int!(0));

        test_case(
            "PUSHINT 1
             PUSHINT 2
             PUSHINT 0
             MULRSHIFT",
        )
        .expect_bytecode(vec![0x71, 0x72, 0x70, 0xa9, 0xa4, 0x80])
        .expect_item(int!(2));

        test_case(
            "PUSHINT 2
             PUSHINT 0
             PUSHINT 1
             MULRSHIFT",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             PUSHINT 256
             MULRSHIFT",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             PUSHINT 255
             MULRSHIFT",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             PUSHINT 256
             MULRSHIFT",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT 2
            PUSHPOW2 254
            PUSHINT 255
             MULRSHIFT",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             PUSHINT 1
             MULRSHIFT",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT 1
             PUSHINT -2
             PUSHINT 1
             MULRSHIFT",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT 1
             MULRSHIFT",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 0..256

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT -1
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT 257
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_underflow_error() {
        test_case("MULRSHIFT").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -2
             PUSHINT 257
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHSLICE  x8_
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             ENDC
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             PUSHINT 2
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHCONT { NOP }
             MULRSHIFT",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod mulrshift_tt {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 0
             PUSHINT 1
             MULRSHIFT 1",
        )
        .expect_bytecode(vec![0x70, 0x71, 0xa9, 0xb4, 0x00, 0x80])
        .expect_item(int!(0));

        test_case(
            "PUSHINT 1
             PUSHINT 2
             MULRSHIFT 2",
        )
        .expect_bytecode(vec![0x71, 0x72, 0xa9, 0xb4, 0x01, 0x80])
        .expect_item(int!(0));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             MULRSHIFT 256",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             MULRSHIFT 255",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             MULRSHIFT 256",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             MULRSHIFT 255",
        )
        .expect_bytecode(vec![0x72, 0x83, 0xfd, 0xa9, 0xb4, 0xfe, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             MULRSHIFT 2",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT 1
             PUSHINT -2
             MULRSHIFT 2",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFT 2",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT 3
            PUSHINT 3
            MULRSHIFT 1",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHINT 3
            PUSHINT -3
            MULRSHIFT 1",
        )
        .expect_item(int!(-5));

        test_case(
            "PUSHINT -1
            PUSHINT 2
            MULRSHIFT 2",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT 2147483647
             PUSHINT 127
             MULRSHIFT 6",
        )
        .expect_item(int!(parse "4261412862"));

        test_case(
            "PUSHINT -2147483648
            PUSHINT 127
            MULRSHIFT 10",
        )
        .expect_item(int!(-266338304));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             MULRSHIFT 256",
        ).expect_item(int!(parse "28948022309329048855892746252171976963317496166410141009864396001978282409984"));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             MULCONST -1
             MULRSHIFT 256",
        ).expect_item(int!(parse "-28948022309329048855892746252171976963317496166410141009864396001978282409984"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 1..256
        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFT -1",
        )
        .expect_compilation_failure(CompileError::unexpected_type(
            3,
            14,
            "MULRSHIFT",
            "arg 0",
        ));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFT 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "MULRSHIFT",
            "arg 0",
        ));
    }

    #[test]
    fn test_underflow_error() {
        test_case("MULRSHIFT 1").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             MULRSHIFT 1",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             MULRSHIFT 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod mulrshiftr {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_bytecode(vec![0x70, 0x71, 0x72, 0xa9, 0xa5, 0x80])
        .expect_item(int!(0));

        test_case(
            "PUSHINT 1
             PUSHINT 2
             PUSHINT 0
             MULRSHIFTR",
        )
        .expect_bytecode(vec![0x71, 0x72, 0x70, 0xa9, 0xa5, 0x80])
        .expect_item(int!(2));

        test_case(
            "PUSHINT 2
             PUSHINT 0
             PUSHINT 1
             MULRSHIFTR",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             PUSHINT 256
             MULRSHIFTR",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             PUSHINT 255
             MULRSHIFTR",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             PUSHINT 256
             MULRSHIFTR",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
            PUSHPOW2 254
            PUSHINT 255
             MULRSHIFTR",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             PUSHINT 1
             MULRSHIFTR",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT 1
             PUSHINT -2
             PUSHINT 1
             MULRSHIFTR",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT 1
             MULRSHIFTR",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             PUSHINT 1
             MULRSHIFTR",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 0..256

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT -1
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             PUSHINT -2
             PUSHINT 257
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_underflow_error() {
        test_case("MULRSHIFTR").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -2
             PUSHINT 257
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHSLICE  x8_
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             ENDC
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             PUSHINT 2
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHCONT { NOP }
             MULRSHIFTR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod mulrshiftr_tt {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 0
             PUSHINT 1
             MULRSHIFTR 1",
        )
        .expect_bytecode(vec![0x70, 0x71, 0xa9, 0xb5, 0x00, 0x80])
        .expect_item(int!(0));

        test_case(
            "PUSHINT 1
             PUSHINT 2
             MULRSHIFTR 2",
        )
        .expect_bytecode(vec![0x71, 0x72, 0xa9, 0xb5, 0x01, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             MULRSHIFTR 256",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHPOW2 254
             PUSHINT 2
             MULRSHIFTR 255",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             MULRSHIFTR 256",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHPOW2 254
             MULRSHIFTR 255",
        )
        .expect_bytecode(vec![0x72, 0x83, 0xfd, 0xa9, 0xb5, 0xfe, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT -1
             PUSHINT 2
             MULRSHIFTR 2",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT 1
             PUSHINT -2
             MULRSHIFTR 2",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFTR 2",
        )
        .expect_item(int!(1));

        test_case(
            "PUSHINT 3
            PUSHINT 3
            MULRSHIFTR 1",
        )
        .expect_item(int!(5));

        test_case(
            "PUSHINT 3
            PUSHINT -3
            MULRSHIFTR 1",
        )
        .expect_item(int!(-4));

        test_case(
            "PUSHINT -1
            PUSHINT 2
            MULRSHIFTR 1",
        )
        .expect_item(int!(-1));

        test_case(
            "PUSHINT -1
            PUSHINT 2
            MULRSHIFTR 2",
        )
        .expect_item(int!(0));

        test_case(
            "PUSHINT 2147483647
             PUSHINT 127
             MULRSHIFTR 6",
        )
        .expect_item(int!(parse "4261412862"));

        test_case(
            "PUSHINT -2147483648
            PUSHINT 127
            MULRSHIFTR 10",
        )
        .expect_item(int!(-266338304));
        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             MULRSHIFTR 256",
        ).expect_item(int!(parse "28948022309329048855892746252171976963317496166410141009864396001978282409984"));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             MULCONST -1
             MULRSHIFTR 256",
        ).expect_item(int!(parse "-28948022309329048855892746252171976963317496166410141009864396001978282409984"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 1..256
        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFTR -1",
        )
        .expect_compilation_failure(CompileError::unexpected_type(
            3,
            14,
            "MULRSHIFTR",
            "arg 0",
        ));

        test_case(
            "PUSHINT -1
             PUSHINT -2
             MULRSHIFTR 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "MULRSHIFTR",
            "arg 0",
        ));
    }

    #[test]
    fn test_underflow_error() {
        test_case("MULRSHIFTR 1").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             MULRSHIFTR 1",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             MULRSHIFTR 2",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod lshiftadddivmod {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 11 ; x
             PUSHINT 2 ; w add 2 - 90
             PUSHINT 4 ; z div by 4 - 22, 2
             PUSHINT 3 ; y shift by 3 - 88
             LSHIFTADDDIVMOD",
        )
        .expect_bytecode(vec![0x80, 0x0b, 0x72, 0x74, 0x73, 0xa9, 0xc0, 0x80])
        .expect_int_stack(&[22, 2]);

        test_case(
            "PUSHINT 1
             PUSHINT 2 ; add 2 - 18
             PUSHINT 3 ; div by 3 - 6, 0
             PUSHINT 4 ; shift by 4 - 16
             LSHIFTADDDIVMOD",
        )
        .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0xa9, 0xc0, 0x80])
        .expect_int_stack(&[6, 0]);

        test_case(
            "PUSHINT 3
             PUSHINT 2
             PUSHINT 1
             PUSHINT 0
             LSHIFTADDDIVMOD",
        )
        .expect_int_stack(&[5, 0]);

        test_case(
            "PUSHPOW2 255
             PUSHINT 3
             PUSHPOW2 255
             PUSHINT 2
             LSHIFTADDDIVMOD",
        )
        .expect_int_stack(&[4, 3]);

        test_case(
            "PUSHPOW2 255
             PUSHINT 1
             PUSHPOW2 255
             PUSHINT 255
             LSHIFTADDDIVMOD",
        )
        .expect_stack(Stack::new().push(int!(parse "57896044618658097711785492504343953926634992332820282019728792003956564819968")).push(int!(1)));

        test_case(
            "PUSHPOW2 200
             PUSHINT 2
             PUSHPOW2 198
             PUSHINT 0
             LSHIFTADDDIVMOD",
        )
        .expect_int_stack(&[4, 2]);
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 0..256

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT 1
             PUSHINT -1
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT 1
             PUSHINT 257
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_underflow_error() {
        test_case("LSHIFTADDDIVMOD").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -2
             PUSHINT 257
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHSLICE  x8_
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             PUSHSLICE  x8_
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             NEWC
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             ENDC
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             NEWC
             ENDC
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             PUSHINT 2
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHCONT { NOP }
             PUSHINT 3
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHINT 2
             PUSHCONT { NOP }
             LSHIFTADDDIVMOD",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod lshiftdiv {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 11
             PUSHINT 3
             PUSHINT 4
             LSHIFTDIV",
        )
        .expect_bytecode(vec![0x80, 0x0b, 0x73, 0x74, 0xa9, 0xc4, 0x80])
        .expect_item(int!(58));

        test_case(
            "PUSHINT 1
             PUSHINT 16
             PUSHINT 4
             LSHIFTDIV",
        )
        .expect_bytecode(vec![0x71, 0x80, 0x10, 0x74, 0xa9, 0xc4, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHINT 1
             PUSHINT 0
             LSHIFTDIV",
        )
        .expect_item(int!(2));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHINT 255
             LSHIFTDIV",
        ).expect_item(int!(parse "57896044618658097711785492504343953926634992332820282019728792003956564819968"));

        test_case(
            "PUSHPOW2 200
             PUSHINT 2
             PUSHINT 0
             LSHIFTDIV",
        )
        .expect_item(int!(parse "803469022129495137770981046170581301261101496891396417650688"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 0..256

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT -1
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT 257
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_underflow_error() {
        test_case("LSHIFTDIV").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -2
             PUSHINT 257
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHSLICE  x8_
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             ENDC
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             PUSHINT 2
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHCONT { NOP }
             LSHIFTDIV",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod lshiftdiv_tt {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 11
             PUSHINT 3
             LSHIFTDIV 4",
        )
        .expect_bytecode(vec![0x80, 0x0b, 0x73, 0xa9, 0xd4, 0x03, 0x80])
        .expect_item(int!(58));

        test_case(
            "PUSHINT 1
             PUSHINT 16
             LSHIFTDIV 4",
        )
        .expect_bytecode(vec![0x71, 0x80, 0x10, 0xa9, 0xd4, 0x03, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHINT 1
             LSHIFTDIV 1",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             LSHIFTDIV 2",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             LSHIFTDIV 255",
        ).expect_item(int!(parse "57896044618658097711785492504343953926634992332820282019728792003956564819968"));

        test_case(
            "PUSHPOW2 200
             PUSHINT 2
             LSHIFTDIV 1",
        )
        .expect_item(int!(parse "1606938044258990275541962092341162602522202993782792835301376"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 1..256

        test_case(
            "PUSHINT 1
             PUSHINT 1
             LSHIFTDIV 0",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "LSHIFTDIV",
            "arg 0",
        ));

        test_case(
            "PUSHINT 1
             PUSHINT 1
             LSHIFTDIV 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "LSHIFTDIV",
            "arg 0",
        ));
    }

    #[test]
    fn test_underflow_error() {
        test_case("LSHIFTDIV 1").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 2
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             LSHIFTDIV 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod lshiftdivr {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 11
             PUSHINT 3
             PUSHINT 4
             LSHIFTDIVR",
        )
        .expect_bytecode(vec![0x80, 0x0b, 0x73, 0x74, 0xa9, 0xc5, 0x80])
        .expect_item(int!(59));

        test_case(
            "PUSHINT 1
             PUSHINT 16
             PUSHINT 4
             LSHIFTDIVR",
        )
        .expect_bytecode(vec![0x71, 0x80, 0x10, 0x74, 0xa9, 0xc5, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHINT 1
             PUSHINT 0
             LSHIFTDIVR",
        )
        .expect_item(int!(2));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHINT 255
             LSHIFTDIVR",
        ).expect_item(int!(parse "57896044618658097711785492504343953926634992332820282019728792003956564819968"));

        test_case(
            "PUSHPOW2 200
             PUSHINT 2
             PUSHINT 0
             LSHIFTDIVR",
        )
        .expect_item(int!(parse "803469022129495137770981046170581301261101496891396417650688"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 0..256

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT -1
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT 257
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_underflow_error() {
        test_case("LSHIFTDIVR").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -2
             PUSHINT 257
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHSLICE  x8_
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             NEWC
             ENDC
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             PUSHINT 2
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHINT 1
             PUSHCONT { NOP }
             LSHIFTDIVR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod lshiftdivr_tt {
    use super::*;

    #[test]
    fn test_success() {
        test_case(
            "PUSHINT 11
             PUSHINT 3
             LSHIFTDIVR 4",
        )
        .expect_bytecode(vec![0x80, 0x0b, 0x73, 0xa9, 0xd5, 0x03, 0x80])
        .expect_item(int!(59));

        test_case(
            "PUSHINT 1
             PUSHINT 16
             LSHIFTDIVR 4",
        )
        .expect_bytecode(vec![0x71, 0x80, 0x10, 0xa9, 0xd5, 0x03, 0x80])
        .expect_item(int!(1));

        test_case(
            "PUSHINT 2
             PUSHINT 1
             LSHIFTDIVR 1",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             LSHIFTDIVR 2",
        )
        .expect_item(int!(4));

        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             LSHIFTDIVR 255",
        ).expect_item(int!(parse "57896044618658097711785492504343953926634992332820282019728792003956564819968"));

        test_case(
            "PUSHPOW2 200
             PUSHINT 2
             LSHIFTDIVR 1",
        )
        .expect_item(int!(parse "1606938044258990275541962092341162602522202993782792835301376"));
    }

    #[test]
    fn test_range_error() {
        // last argument should be in range 1..256

        test_case(
            "PUSHINT 1
             PUSHINT 1
             LSHIFTDIVR 0",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "LSHIFTDIVR",
            "arg 0",
        ));

        test_case(
            "PUSHINT 1
             PUSHINT 1
             LSHIFTDIVR 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(
            3,
            14,
            "LSHIFTDIVR",
            "arg 0",
        ));
    }

    #[test]
    fn test_underflow_error() {
        test_case("LSHIFTDIVR 1").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 257
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_type_check_error() {
        test_case(
            "PUSHSLICE  x8_
             PUSHINT 2
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHSLICE  x8_
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT 1
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             NEWC
             ENDC
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHCONT { NOP }
             PUSHINT 1
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 0
             PUSHCONT { NOP }
             LSHIFTDIVR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}
