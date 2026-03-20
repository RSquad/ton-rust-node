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
use ton_block::{ExceptionCode, SliceData};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
};

#[test]
fn test_throw_short() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        THROW 20
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(20)));
}

#[test]
fn test_throw_normal_termination() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 5
        }
        POPCTR c2
        PUSHINT 7
        THROWARG 0
        ",
    )
    .expect_int_stack(&[7, 0, 5]);
}

#[test]
fn test_throw_alternative_termination() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 5
        }
        POPCTR c2
        PUSHINT 7
        THROWARG 1
        ",
    )
    .expect_int_stack(&[7, 1, 5]);
}

#[test]
fn test_throw_long() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        THROW 2047
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2047)));
}

#[test]
fn test_throw_666() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        THROW 666
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(666)));
}

#[test]
fn test_throw_long_handler_gets_new_stack() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 10
            THROW 2047
        }
        PUSHCONT {
        }
        TRY
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2047)));
}

#[test]
fn test_throw_without_parameter() {
    test_case("THROW").expect_compilation_failure(CompileError::missing_params(1, 1, "THROW"));
}

#[test]
fn test_throw_out_of_range() {
    test_case("THROW 2048")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "THROW", "Number"));
}

#[test]
fn test_throw_non_integer() {
    test_case("THROW s1")
        .expect_compilation_failure(CompileError::unexpected_type(1, 1, "THROW", "Number"));
}

#[test]
fn test_throwif() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 0
        THROWIF 2
        PUSHINT 1
        THROWIF 2047
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2047)));
}

#[test]
fn test_throwif_0_short_catch() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 6
        }
        POPCTR c2
        PUSHINT 0
        THROWIF 2047
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHINT 4
        THROWARGIF 0
        PUSHINT 5
        ",
    )
    .expect_int_stack(&[3, 0, 6]);
}

#[test]
fn test_throwif_1_short_catch() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 6
        }
        POPCTR c2
        PUSHINT 0
        THROWIF 2047
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHINT 4
        THROWARGIF 1
        PUSHINT 5
        ",
    )
    .expect_int_stack(&[3, 1, 6]);
}

#[test]
fn test_throwif_short() {
    test_case(
        "
        PUSHINT 0
        THROWIF 2047
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHINT 4
        THROWARGIF 1
        PUSHINT 5
        ",
    )
    .expect_stack(Stack::new().push(int!(3)));
}

#[test]
fn test_throw_big_exception() {
    test_case("PUSHINT 1 PUSHINT 2 ADD ACCEPT PUSHINT 3 THROW 100").expect_custom_failure(100);
}

#[test]
fn test_throwif_stack_underflow() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        THROWIF 33
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(ExceptionCode::StackUnderflow as u64)));
}

#[test]
fn test_throwif_not_an_integer() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHSLICE xC_
        THROWIF 33
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(ExceptionCode::TypeCheckError as u64)));
}

#[test]
fn test_throwifnot() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 1
        THROWIFNOT 2
        PUSHINT 0
        THROWIFNOT 2047
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2047)));
}

#[test]
fn test_throwifnot_short() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 1
        THROWIFNOT 2047
        PUSHINT 0
        THROWIFNOT 1
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn test_throwifnot_stack_underflow() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        THROWIFNOT 33
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(ExceptionCode::StackUnderflow as u64)));
}

#[test]
fn test_throwifnot_not_an_integer() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHSLICE xC_
        THROWIFNOT 33
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(ExceptionCode::TypeCheckError as u64)));
}

#[test]
fn test_throwarg() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 135
        THROWARG 1000
        ",
    )
    .expect_stack(Stack::new().push(int!(135)).push(int!(1000)));
}

#[test]
fn test_throwargany() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 135
        PUSHINT 60000
        THROWARGANY
        ",
    )
    .expect_stack(Stack::new().push(int!(135)).push(int!(60000)));
}

#[test]
fn test_throwargany_number_not_integer() {
    test_case(
        "
        PUSHINT 135
        PUSHSLICE xC_
        THROWARGANY
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwargany_value_not_integer() {
    let slice_data = vec![0xC0];
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHSLICE xC_
        PUSHINT 135
        THROWARGANY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(slice_data))).push(int!(135)));
}

#[test]
fn test_throwarganyif() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 1 ; value
        PUSHINT 2 ; number
        PUSHINT 0 ; condition
        THROWARGANYIF
        PUSHINT 10 ; value
        PUSHINT 20 ; number
        PUSHINT 1  ; condition
        THROWARGANYIF
        ",
    )
    .expect_stack(Stack::new().push(int!(10)).push(int!(20)));
}

#[test]
fn test_throwarganyif_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        PUSHINT 2
        THROWARGANYIF
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwarganyif_value_not_integer() {
    let slice_data = vec![0xC0];
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHSLICE xC_ ; value
        PUSHINT 135   ; number
        PUSHINT 20    ; condition
        THROWARGANYIF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(slice_data))).push(int!(135)));
}

#[test]
fn test_throwarganyif_number_not_integer() {
    test_case(
        "
        PUSHINT 1     ; value
        PUSHSLICE xC_ ; number
        PUSHINT 2     ; condition
        THROWARGANYIF
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwarganyif_condition_not_integer() {
    test_case(
        "
        PUSHINT 1     ; value
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWARGANYIF
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwarganyifnot() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 1 ; value
        PUSHINT 2 ; number
        PUSHINT 1 ; condition
        THROWARGANYIFNOT
        PUSHINT 10 ; value
        PUSHINT 20 ; number
        PUSHINT 0  ; condition
        THROWARGANYIFNOT
        ",
    )
    .expect_stack(Stack::new().push(int!(10)).push(int!(20)));
}

#[test]
fn test_throwarganyifnot_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        PUSHINT 2
        THROWARGANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwarganyifnot_value_not_integer() {
    let slice_data = vec![0xC0];
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHSLICE xC_ ; value
        PUSHINT 135   ; number
        PUSHINT 0     ; condition
        THROWARGANYIFNOT
        ",
    )
    .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(slice_data))).push(int!(135)));
}

#[test]
fn test_throwarganyifnot_number_not_integer() {
    test_case(
        "
        PUSHINT 1     ; value
        PUSHSLICE xC_ ; number
        PUSHINT 0     ; condition
        THROWARGANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwarganyifnot_condition_not_integer() {
    test_case(
        "
        PUSHINT 1     ; value
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWARGANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwany() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 500 ; number
        THROWANY
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(500)));
}

#[test]
fn test_throwany_stack_underflow() {
    test_case("THROWANY").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwanyif() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 2 ; number
        PUSHINT 0 ; condition
        THROWANYIF
        PUSHINT 20 ; number
        PUSHINT 1  ; condition
        THROWANYIF
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(20)));
}

#[test]
fn test_throwanyif_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        THROWANYIF
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwanyif_number_not_integer() {
    test_case(
        "
        PUSHSLICE xC_ ; number
        PUSHINT 2     ; condition
        THROWANYIF
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwanyif_condition_not_integer() {
    test_case(
        "
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWANYIF
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwanyifnot() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 2 ; number
        PUSHINT 1 ; condition
        THROWANYIFNOT
        PUSHINT 20 ; number
        PUSHINT 0  ; condition
        THROWANYIFNOT
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(20)));
}

#[test]
fn test_throwanyifnot_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        THROWANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwanyifnot_number_not_integer() {
    test_case(
        "
        PUSHSLICE xC_ ; number
        PUSHINT 0     ; condition
        THROWANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwanyifnot_condition_not_integer() {
    test_case(
        "
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWANYIFNOT
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwargif() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 2 ; value
        PUSHINT 0 ; condition
        THROWARGIF 55
        PUSHINT 20 ; value
        PUSHINT 1  ; condition
        THROWARGIF 2047
        ",
    )
    .expect_stack(Stack::new().push(int!(20)).push(int!(2047)));
}

#[test]
fn test_throwargif_nan_condition() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 777
        }
        POPCTR c2
        PUSHINT 20 ; value
        PUSHNAN ; condition
        THROWARGIF 2047 ; number
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(4)).push(int!(777)));
}

#[test]
fn test_throwargif_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        THROWARGIF 50
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwargif_condition_not_integer() {
    test_case(
        "
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWARGIF 55
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_throwargifnot() {
    test_case(
        "
        PUSHCONT {}
        POPCTR c2
        PUSHINT 2 ; value
        PUSHINT 1 ; condition
        THROWARGIFNOT 55
        PUSHINT 20 ; value
        PUSHINT 0  ; condition
        THROWARGIFNOT 2047
        ",
    )
    .expect_stack(Stack::new().push(int!(20)).push(int!(2047)));
}

#[test]
fn test_throwargifnot_stack_underflow() {
    test_case(
        "
        PUSHINT 1
        THROWARGIFNOT 50
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_throwargifnot_condition_not_integer() {
    test_case(
        "
        PUSHINT 2     ; number
        PUSHSLICE xC_ ; condition
        THROWARGIFNOT 55
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_switch_to_c2_when_exception() {
    test_case(
        "
        PUSHCONT {
            DUP
            PUSH s2
        }
        POPCTR c2
        PUSHINT 1
        PUSH s1
        ",
    )
    .expect_stack(
        Stack::new()
            .push(int!(0)) // PUSH s2
            .push(int!(ExceptionCode::StackUnderflow as u64)) // DUP
            .push(int!(ExceptionCode::StackUnderflow as u64)) // number
            .push(int!(0)), // value
    );
}

#[test]
fn test_default_exception_handler() {
    test_case(
        "
        PUSHINT 1
        PUSH s1
        ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_try_command_no_exception() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
            PUSHINT 2
            ADD
        }
        PUSHCONT {
            PUSH s1
            INC
        }
        TRY
        PUSHINT 4
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(3)).push(int!(4)));
}

#[test]
fn test_try_command_with_exception() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
            PUSH s2
        }
        PUSHCONT {
            DUP
        }
        TRY
        PUSHINT 4
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)).push(int!(2)).push(int!(4)));
}

#[test]
fn test_try_command_rethrow_exception() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 10
            EQUAL
        }
        POPCTR c2
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
            PUSH s2
        }
        PUSHCONT {
            THROW 10
        }
        TRY
        PUSHINT 4
    ",
    )
    .with_gas_limit(1000)
    .expect_stack(
        Stack::new()
            .push(int!(0)) // THROW 10: value
            .push(boolean!(true)) // THROW 10: number == 10 (original c2)
            .push(int!(4)),
    )
    .expect_steps(15)
    .expect_gas(1000000000, 1000, 0, 650)
    .expect_success();
}

#[test]
fn test_try_command_nested_try() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 1
            PUSHCONT {
                PUSHINT 2
                PUSH s2
            }
            PUSHCONT {
                PUSHINT 5
                THROW 10
            }
            TRY
        }
        PUSHCONT {
            PUSHINT 10
            EQUAL
        }
        TRY
        PUSHINT 4
        ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(-1)).push(int!(4)));
}

#[test]
fn test_try_command_wrong_parameters() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 1
        }
        PUSHINT 1
        TRY
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tryargs_command_call_with_3_1() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHCONT {
            ADD
            ADD
            DUP
            DEC
        }
        PUSHCONT {
            PUSHINT -1
        }
        TRYARGS 3, 1",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(5)));
}

#[test]
fn test_tryargs_command_call_with_0_1() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHCONT {
            PUSHINT 5
        }
        PUSHCONT {
            PUSHINT -1
        }
        TRYARGS 0, 1",
    )
    .expect_stack(
        Stack::new().push(int!(0)).push(int!(1)).push(int!(2)).push(int!(3)).push(int!(5)),
    );
}

#[test]
fn test_tryargs_command_call_with_1_0() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHCONT {
            PUSHINT 5
        }
        PUSHCONT {
            PUSHINT -1
        }
        TRYARGS 1, 0",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));
}

#[test]
fn test_tryargs_command_stackunderflow() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 1
        PUSHCONT {
            PUSHINT 5
        }
        PUSHCONT {
            PUSHINT -1
        }
        TRYARGS 10, 1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tryargs_command_with_exception() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 1
        PUSHCONT {
            PUSH s1
        }
        PUSHCONT {
            PUSHINT -1
            PUSHINT -2
        }
        TRYARGS 1, 2",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(-1)).push(int!(-2)));
}

#[test]
fn test_throw_0() {
    test_case(
        "
        TEN
        THROW 0
    ",
    )
    .expect_item(int!(0));
    test_case(
        "
        TEN
        THROWARG 0
    ",
    )
    .expect_item(int!(10));
}

#[test]
fn test_throw_1() {
    test_case(
        "
        TEN
        THROW 1
    ",
    )
    .expect_item(int!(0));
    test_case(
        "
        TEN
        THROWARG 1
    ",
    )
    .expect_item(int!(10));
}

#[test]
fn test_invalid_opcode_in_catch() {
    test_case(
        "
        PUSHCONT {
            INC
        }
        PUSHCONT {
            ;; PUSHCONT 7 bytes but only 6 are provided
            .blob x97000000000000
        }
        TRY
    ",
    )
    .with_gas_limit(1000)
    .expect_steps(7)
    .expect_gas(1000000000, 1000, 0, 802)
    // TODO improve test_framework.rs
    // .expect_stack(Stack::new()
    //     .push(int!(0))
    //     .push(int!(6))
    // )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn test_catch_should_not_return() {
    test_case(
        "
        PUSHCONT { }
        AGAINEND
        POP c2
        PUSHCONT { }
        WHILEEND
    ",
    )
    .with_gas_limit(1000)
    .expect_steps(8)
    .expect_gas(1000000000, 1000, 0, 842)
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)))
    .expect_success();
}

#[test]
fn test_pop_c2_from_c0() {
    test_case(
        "
        AGAINEND
        PUSHCTR c0
        POPSAVE c2
    ",
    )
    .with_gas_limit(1000)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_default_exception_handler_is_nonephemeral() {
    test_case(
        "
        PUSHINT 12345
        PUSHCTR c2
        JMPX
    ",
    )
    .skip_fift_check() // TODO: because of different stack
    .with_gas_limit(1000)
    .expect_steps(3)
    .expect_gas(1000000000, 1000, 0, 922)
    .expect_custom_failure(12345);
}

#[test]
fn test_bug_popsave() {
    test_case(
        "
        ;; c0 = QuitCont(0)
        ;; c2 = ExcQuitCont
        PUSHCONT { } ;; OrdCont
        AGAINEND
        ;; c0 = AgainCont
        POPSAVE c2 ;; Durov envelopes c0 with ArgContExt here
        ;; c0 = AgainCont { savelist.c2 = ExcQuitCont }
        ;; c2 = OrdCont
        PUSHCTR c0
        POPSAVE c2
        ;; c0.savelist.c2 = OrdCont -- this doesn't happen since c0.savelist.c2 is already defined
        ;; c2 = AgainCont { savelist.c2 = ExcQuitCont }
    ",
    )
    .with_gas_limit(1000)
    .expect_steps(8)
    .expect_gas(1000000000, 1000, 0, 805)
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_again_nargs() {
    test_case(
        "
        AGAINEND
        PUSHCONT { }
        PUSHCTR c0
        POPSAVE c2
        SWAP
    ",
    )
    .with_gas_limit(1000)
    .expect_steps(53)
    .expect_gas(1000000000, 1000, 0, -11)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_try_nargs_bug() {
    test_case(
        "
        PUSHINT 111
        PUSHCONT {
            PUSHINT 222
            PUSHINT 333
            PUSHCTR c2
            ; w/o the fix, insn below fails because
            ; catch cont's nargs has been wrongly set
            ; to 2 by TRY
            SETCONTARGS 3, -1
            POPCTR c2
            THROW 444
        }
        PUSHCONT {
            PUSHINT 555
        }
        TRY
    ",
    )
    .expect_stack(
        Stack::new()
            .push(int!(111))
            .push(int!(222))
            .push(int!(333))
            .push(int!(0))
            .push(int!(444))
            .push(int!(555)),
    )
    .expect_success();
}
