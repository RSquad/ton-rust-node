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
use ton_block::ExceptionCode;
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn test_pick_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 1
         PICK",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(2)).push(int!(1)));

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 1
         PUSHX",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(2)).push(int!(1)));
}

#[test]
fn test_pick_command_stackunderflow() {
    test_case("PICK").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_pick_command_stackunderflow_param() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PICK",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_pick_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         PICK",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_pick_command_too_many_params() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PICK s0",
    )
    .expect_compilation_failure(CompileError::too_many_params(4, 15, "PICK"));
}

#[test]
fn test_blkswap_command_works_and_equivalent_to_reverse_combo() {
    let mut expected_stack = Stack::new();
    expected_stack
        .push(int!(8))
        .push(int!(7))
        .push(int!(6))
        .push(int!(3))
        .push(int!(2))
        .push(int!(1))
        .push(int!(0))
        .push(int!(5))
        .push(int!(4));
    test_case(
        "PUSHINT 8
         PUSHINT 7
         PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKSWAP 2, 4",
    )
    .expect_bytecode(
        // 55ij for BLKSWAP i+1, j+1
        // e.g. 0x55 0x13 for BLKSWAP 2, 4
        vec![0x78, 0x77, 0x76, 0x75, 0x74, 0x73, 0x72, 0x71, 0x70, 0x55, 0x13, 0x80],
    )
    .expect_stack(&expected_stack);

    test_case(
        "PUSHINT 8
         PUSHINT 7
         PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         REVERSE 2, 4
         REVERSE 4, 0
         REVERSE 6, 0",
    )
    .expect_stack(&expected_stack);
}

#[test]
fn test_blkswap_zero_first_argument() {
    test_case("BLKSWAP 0, 2")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "BLKSWAP", "arg 0"));
}

#[test]
fn test_blkswap_zero_second_argument() {
    test_case("BLKSWAP 3, 0")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "BLKSWAP", "arg 1"));
}

#[test]
fn test_blkswap_i_plus_j_equals_depth() {
    let mut expected_stack = Stack::new();
    expected_stack
        .push(int!(4))
        .push(int!(3))
        .push(int!(2))
        .push(int!(1))
        .push(int!(0))
        .push(int!(8))
        .push(int!(7))
        .push(int!(6))
        .push(int!(5));
    test_case(
        "PUSHINT 8
         PUSHINT 7
         PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKSWAP 4, 5",
    )
    .expect_stack(&expected_stack);
}

#[test]
fn test_blkswap_stack_underflow() {
    test_case(
        "PUSHINT 8
         PUSHINT 7
         PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKSWAP 3, 7",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_blkdrop_command() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKDROP 2",
    )
    .expect_bytecode(
        // 5f0i for BLKDROP i
        vec![0x72, 0x71, 0x70, 0x5f, 0x02, 0x80],
    )
    .expect_item(int!(2));
}

#[test]
fn test_blkdrop_can_clean_whole_stack() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKDROP 3",
    )
    .expect_empty_stack();
}

#[test]
fn test_blkdrop_stack_underflow() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         BLKDROP 4",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_blkdrop_argument_out_of_range() {
    test_case("BLKDROP 16")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "BLKDROP", "arg 0"));
}

#[test]
fn test_blkdrop2_command_normal() {
    test_case(
        "
        PUSHINT 2
        PUSHINT 1
        PUSHINT 0
        BLKDROP2 2, 1",
    )
    .expect_item(int!(0));

    test_case(
        "
        PUSHINT 4
        PUSHINT 3
        PUSHINT 2
        PUSHINT 1
        PUSHINT 0
        BLKDROP2 3, 1",
    )
    .expect_int_stack(&[4, 0]);
}

#[test]
fn test_blkdrop2_command_error() {
    expect_exception("BLKDROP2 15, 15", ExceptionCode::StackUnderflow);
    expect_exception("BLKDROP2 1, 2", ExceptionCode::StackUnderflow);
    expect_exception("ZERO BLKDROP2 1, 2", ExceptionCode::StackUnderflow);
    test_case(
        "
        PUSHINT 1
        PUSHINT 0
        BLKDROP2 2, 1
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_blkpush_simple() {
    test_case(
        "
        ONE
        TWO
        BLKPUSH 2, 1
    ",
    )
    .expect_int_stack(&[1, 2, 1, 2]);
}

#[test]
fn test_reverse_command_even_length() {
    test_case(
        "PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         REVERSE 4, 2",
    )
    .expect_bytecode(
        // 5Eij for REVERSE (i+2), j
        // e.g. 5E22 for REVERSE 4, 2
        vec![0x76, 0x75, 0x74, 0x73, 0x72, 0x71, 0x70, 0x5e, 0x22, 0x80],
    )
    .expect_int_stack(&[6, 2, 3, 4, 5, 1, 0]);
}

#[test]
fn test_reverse_command_odd_length() {
    test_case(
        "PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         REVERSE 5, 2",
    )
    .expect_int_stack(&[2, 3, 4, 5, 6, 1, 0]);
}

#[test]
fn test_reverse_command_whole_stack() {
    test_case(
        "PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         REVERSE 7, 0",
    )
    .expect_int_stack(&[0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn test_reverse_first_argument_too_small() {
    test_case("REVERSE 1, 3")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "REVERSE", "arg 0"));
}

#[test]
fn test_reverse_first_argument_equals_2() {
    test_case(
        "PUSHINT 1
         PUSHINT 0
         REVERSE 2, 0",
    )
    .expect_int_stack(&[0, 1]);
}

#[test]
fn test_reverse_second_argument_too_large() {
    test_case("REVERSE 2, 16")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "REVERSE", "arg 1"));
}

#[test]
fn test_reverse_stack_underflow() {
    test_case(
        "PUSHINT 6
         PUSHINT 5
         PUSHINT 4
         PUSHINT 3
         PUSHINT 2
         PUSHINT 1
         PUSHINT 0
         REVERSE 5, 3",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollx_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 2
         ROLLX",
    )
    .expect_int_stack(&[1, 3, 4, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 0
         ROLLX",
    )
    .expect_int_stack(&[1, 2, 3, 4]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 260
            ROLLX
            DEPTH
            PUSHINT 1
            ONLYTOPX",
    )
    .expect_gas_used(11356)
    .expect_int_stack(&[600]);
}

#[test]
fn test_rollx_stack_underflow() {
    test_case("ROLLX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollx_command_stackunderflow_param() {
    test_case(
        "
        PUSHINT 1
        ROLLX
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         ROLLX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_rollx_command_too_many_params() {
    test_case("ROLLX 2").expect_compilation_failure(CompileError::too_many_params(1, 7, "ROLLX"));
}

#[test]
fn test_rollrevx_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 2
         ROLLREVX",
    )
    .expect_int_stack(&[1, 4, 2, 3]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 0
         ROLLREVX",
    )
    .expect_int_stack(&[1, 2, 3, 4]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 2
         -ROLLX",
    )
    .expect_int_stack(&[1, 4, 2, 3]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 260
            ROLLREVX
            DEPTH
            PUSHINT 1
            ONLYTOPX",
    )
    .expect_gas_used(11356)
    .expect_int_stack(&[600]);
}

#[test]
fn test_rollrevx_stack_underflow() {
    test_case("ROLLREVX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollrevx_command_stackunderflow_param() {
    test_case(
        "
        PUSHINT 1
        ROLLREVX
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "
        PUSHINT 0
        ROLLREVX
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollrevx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         ROLLREVX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_rollrevx_command_too_many_params() {
    test_case("ROLLREVX 2")
        .expect_compilation_failure(CompileError::too_many_params(1, 10, "ROLLREVX"));
}

#[test]
fn test_blkswx_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 5
         PUSHINT 3
         PUSHINT 2
         BLKSWX",
    )
    .expect_int_stack(&[4, 5, 1, 2, 3]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 280
            PUSHINT 260
            BLKSWX
            DEPTH
            PUSHINT 1
            ONLYTOPX",
    )
    .expect_gas_used(11670)
    .expect_int_stack(&[600]);
}

#[test]
fn test_blkswx_stack_underflow() {
    test_case("BLKSWX").expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "PUSHINT 1
         BLKSWX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_blkswx_command_stackunderflow_param() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         BLKSWX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_blkswx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         PUSHINT 1
         BLKSWX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
    test_case(
        "PUSHINT 1
         PUSHSLICE x7_
         BLKSWX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_blkswx_command_too_many_params() {
    test_case("BLKSWX 2").expect_compilation_failure(CompileError::too_many_params(1, 8, "BLKSWX"));
}

#[test]
fn test_revx_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 5
         PUSHINT 2
         PUSHINT 1
         REVX",
    )
    .expect_int_stack(&[1, 2, 4, 3, 5]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 5
         PUSHINT 0
         PUSHINT 1
         REVX",
    )
    .expect_int_stack(&[1, 2, 3, 4, 5]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 20
            PUSHINT 260
            REVX
            DEPTH
            PUSHINT 1
            ONLYTOPX",
    )
    .expect_gas_used(11377)
    .expect_int_stack(&[600]);
}

#[test]
fn test_revx_stack_underflow() {
    test_case("REVX").expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "PUSHINT 1
         REVX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_revx_command_stackunderflow_param() {
    test_case(
        "
        PUSHINT 2
        PUSHINT 1
        REVX
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_revx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         PUSHINT 2
         REVX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
    test_case(
        "PUSHINT 1
         PUSHSLICE x7_
         REVX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_revx_command_too_many_params() {
    test_case("REVX 2").expect_compilation_failure(CompileError::too_many_params(1, 6, "REVX"));
}

#[test]
fn test_dropx_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 1
         DROPX",
    )
    .expect_empty_stack();
}

#[test]
fn test_dropx_command_stackunderflow() {
    test_case("DROPX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_dropx_command_stackunderflow_param() {
    test_case(
        "PUSHINT 1
         DROPX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_dropx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         DROPX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_dropx_command_too_many_params() {
    test_case("DROPX s0").expect_compilation_failure(CompileError::too_many_params(1, 7, "DROPX"));
}

#[test]
fn test_tuck_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         TUCK",
    )
    .expect_int_stack(&[2, 1, 2]);
}

#[test]
fn test_tuck_command_stackunderflow() {
    test_case("TUCK").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuck_command_too_many_params() {
    test_case("TUCK s0").expect_compilation_failure(CompileError::too_many_params(1, 6, "TUCK"));
}

#[test]
fn test_depth_command() {
    test_case(
        "PUSHINT 1
         DEPTH",
    )
    .expect_int_stack(&[1, 1]);
}

#[test]
fn test_depth_on_empty_stack() {
    test_case("DEPTH").expect_int_stack(&[0]);
}

#[test]
fn test_depth_command_too_many_params() {
    test_case("DEPTH s0").expect_compilation_failure(CompileError::too_many_params(1, 7, "DEPTH"));
}

#[test]
fn test_chkdepth_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 1
         CHKDEPTH",
    )
    .expect_int_stack(&[1]);
    test_case(
        "PUSHINT 1
         CHKDEPTH",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "PUSHINT 0\n".repeat(256)
            + "PUSHINT 255
         CHKDEPTH",
    )
    .expect_int_stack(&[0].repeat(256));
}

#[test]
fn test_chkdepth_on_empty_stack() {
    test_case("CHKDEPTH").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_chkdepth_command_too_many_params() {
    test_case("CHKDEPTH s0")
        .expect_compilation_failure(CompileError::too_many_params(1, 10, "CHKDEPTH"));
}

#[test]
fn test_xchgx_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  1
         XCHGX",
    )
    .expect_int_stack(&[2, 1]);
}

#[test]
fn test_xchgx_command_stackunderflow() {
    test_case("XCHGX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xchgx_command_stackunderflow_param() {
    test_case(
        "PUSHINT 2
         XCHGX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         XCHGX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xchgx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         XCHGX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_xchgx_command_too_many_params() {
    test_case("XCHGX s0").expect_compilation_failure(CompileError::too_many_params(1, 7, "XCHGX"));
}

#[test]
fn test_onlytopx_command() {
    test_case(
        "PUSHINT 0
         ONLYTOPX",
    )
    .expect_empty_stack();
    test_case(
        "PUSHSLICE x7_
         PUSHINT 0
         ONLYTOPX",
    )
    .expect_empty_stack();
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 1
         ONLYTOPX",
    )
    .expect_int_stack(&[2]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 280
        ONLYTOPX
        PUSHINT 280
        ONLYTOPX
        PUSHINT 1
        ONLYTOPX",
    )
    .expect_gas_used(11410)
    .expect_int_stack(&[0]);

    test_case(
        "PUSHINT 0\n".repeat(600)
            + "PUSHINT 280
        ONLYTOPX",
    )
    .with_gas_limit(11300)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_onlytopx_command_stackunderflow() {
    test_case("ONLYTOPX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_onlytopx_command_stackunderflow_param() {
    test_case(
        "PUSHINT 1
         ONLYTOPX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
    test_case(
        "PUSHINT 1
         PUSHINT 2
         ONLYTOPX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_onlytopx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         ONLYTOPX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_onlytopx_command_too_many_params() {
    test_case("ONLYTOPX s0")
        .expect_compilation_failure(CompileError::too_many_params(1, 10, "ONLYTOPX"));
}

#[test]
fn test_onlyx_command() {
    test_case(
        "PUSHINT 0
         ONLYX",
    )
    .expect_empty_stack();
    test_case(
        "PUSHSLICE x7_
         PUSHINT 0
         ONLYX",
    )
    .expect_empty_stack();
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 1
         ONLYX",
    )
    .expect_int_stack(&[1]);
}

#[test]
fn test_onlyx_command_stackunderflow() {
    test_case("ONLYX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_onlyx_command_stackunderflow_param() {
    test_case(
        "PUSHINT 1
         ONLYX",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_onlyx_command_wrongtype() {
    test_case(
        "PUSHSLICE x7_
         ONLYX",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_onlyx_command_too_many_params() {
    test_case("ONLYX s0").expect_compilation_failure(CompileError::too_many_params(1, 7, "ONLYX"));
}

#[test]
fn test_rot_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         ROT",
    )
    .expect_int_stack(&[2, 3, 1]);
}

#[test]
fn test_rot_stack_underflow() {
    test_case("ROT").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rotrev_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         ROTREV",
    )
    .expect_int_stack(&[3, 1, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         -ROT",
    )
    .expect_int_stack(&[3, 1, 2]);
}

#[test]
fn test_rotrev_stack_underflow() {
    test_case("ROTREV").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_drop2_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 1
         DROP2",
    )
    .expect_empty_stack();

    test_case(
        "PUSHINT 1
         PUSHINT 1
         2DROP",
    )
    .expect_empty_stack();
}

#[test]
fn test_drop2_command_stackunderflow() {
    test_case("DROP2").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_dup2_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         DUP2",
    )
    .expect_int_stack(&[1, 2, 1, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         2DUP",
    )
    .expect_int_stack(&[1, 2, 1, 2]);
}

#[test]
fn test_dup2_command_stackunderflow() {
    test_case("DUP2").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_roll_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         ROLL 2",
    )
    .expect_int_stack(&[1, 3, 4, 2]);
}

#[test]
fn test_roll_stack_underflow() {
    test_case("ROLL 1").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_roll_command_too_many_params() {
    test_case("ROLL 2,2").expect_compilation_failure(CompileError::too_many_params(1, 8, "ROLL"));
}

#[test]
fn test_rollrev_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         ROLLREV 2",
    )
    .expect_int_stack(&[1, 4, 2, 3]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         -ROLL 2",
    )
    .expect_int_stack(&[1, 4, 2, 3]);
}

#[test]
fn test_rollrev_stack_underflow() {
    test_case("ROLLREV 1").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollrev_command_stackunderflow_param() {
    test_case("ROLLREV 1").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_rollrev_command_too_many_params() {
    test_case("ROLLREV 2,2")
        .expect_compilation_failure(CompileError::too_many_params(1, 11, "ROLLREV"));
}

#[test]
fn test_rot2_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         PUSHINT 5
         PUSHINT 6
         ROT2",
    )
    .expect_int_stack(&[3, 4, 5, 6, 1, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         2SWAP",
    )
    .expect_int_stack(&[3, 4, 1, 2]);
}

#[test]
fn test_rot2_command_stackunderflow() {
    test_case("ROT2").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_swap2_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         SWAP2",
    )
    .expect_int_stack(&[3, 4, 1, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         2SWAP",
    )
    .expect_int_stack(&[3, 4, 1, 2]);
}

#[test]
fn test_swap2_command_stackunderflow() {
    test_case("SWAP2").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_over2_command() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         OVER2",
    )
    .expect_int_stack(&[1, 2, 3, 4, 1, 2]);

    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSHINT 3
         PUSHINT 4
         2OVER",
    )
    .expect_int_stack(&[1, 2, 3, 4, 1, 2]);
}

#[test]
fn test_over2_command_stackunderflow() {
    test_case("OVER2").expect_failure(ExceptionCode::StackUnderflow);
}
