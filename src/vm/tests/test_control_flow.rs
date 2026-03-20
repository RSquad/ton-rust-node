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
fn callargs() {
    test_case(
        "PUSHINT 1
        PUSHINT 2
        PUSHCONT {
           INC
        }
        CALLXARGS 1, -1",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(3)));
}

#[test]
fn callargs_err() {
    test_case(
        "PUSHINT 1
        PUSHINT 2
        PUSHCONT {
            DROP
           INC
        }
        CALLXARGS 1, -1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn switch_to_continuation_from_register() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        RET",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_to_continuation_from_register_normal() {
    test_case(
        "
        PUSHINT 10
        RET
    ",
    )
    .expect_item(int!(10));
}

#[test]
fn switch_to_continuation() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        JMPX",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn test_unknown_command_after_pushcont() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            INC
        }
        UNKNOWN_COMMAND
        DROP",
    )
    .expect_compilation_failure(CompileError::unknown(6, 9, "UNKNOWN_COMMAND"));
}

#[test]
fn test_while_factorial() {
    test_case(
        "
        PUSHINT 5
        DUP
        ; Loop condition
        PUSHCONT {
            DEC
            DUP
        }
        ; Loop body
        PUSHCONT {
            DUP
            PUSH S2
            MUL
            POP S2
        }
        ; Start loop
        WHILE
        DROP",
    )
    .expect_item(int!(120));
}

#[test]
fn test_whilebrk_factorial() {
    test_case(
        "
        PUSHINT 5
        DUP
        ; Loop condition
        PUSHCONT {
            DEC
            DUP
        }
        ; Loop body
        PUSHCONT {
            DUP
            PUSH S2
            MUL
            POP S2
            DUP
            DEC
            IFNOTRETALT
        }
        ; Start loop
        WHILEBRK
        DROP
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 50]);

    // simple while
    test_case(
        "
        PUSHINT 5
        DUP
        ; Loop condition
        PUSHCONT {
            DEC
            DUP
        }
        ; Loop body
        PUSHCONT {
            DUP
            PUSH S2
            MUL
            POP S2
            DUP
            DEC
            IFNOTRETALT
        }
        ; Start loop
        WHILE
        DROP
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 1]);
}

#[test]
fn test_while_pushcont() {
    test_case(
        "
        PUSHINT 5
        DUP
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            DUP
            PUSH s2
            PUSHCONT {
                MUL
                POP s2
            }
            CALLX
        }
        WHILE
        DROP",
    )
    .expect_item(int!(120));
}

#[test]
fn test_whilebrk() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
        }
        WHILEBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_whilebrk_if() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            DUP
            PUSHINT 7
            EQUAL
            PUSHCONT {
                RETALT
            }
            IF
        }
        WHILEBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_whilebrk_call() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            DUP
            PUSHINT 7
            EQUAL
            PUSHCONT {
                IFRETALT
            }
            CALLX
        }
        WHILEBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_whilebrk_nested() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
            PUSHINT 0
            PUSHCONT {
                INC
                DUP
            }
            PUSHCONT {
                DUP
                PUSHINT 7
                EQUAL
                IFRETALT
            }
            WHILEBRK
            DROP
        }
        WHILEBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_whilebrk_nested_call() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            PUSHINT 0
            PUSHCONT {
                PUSHCONT {
                    INC
                    DUP
                }
                PUSHCONT {
                    DUP
                    PUSHINT 7
                    EQUAL
                    IFRETALT
                }
                WHILEBRK
                DROP
            }
            CALLX
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
        }
        WHILEBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_repeatbrk_nested() {
    test_case(
        "
        PUSHINT 13
        PUSHINT 25
        PUSHCONT {
            DEC
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
            PUSHINT 0
            PUSHINT 25
            PUSHCONT {
                INC
                DUP
                PUSHINT 7
                EQUAL
                IFRETALT
            }
            REPEATBRK
            DROP
        }
        REPEATBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_untilbrk_nested() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
            PUSHINT 0
            PUSHCONT {
                INC
                DUP
                PUSHINT 7
                EQUAL
                IFRETALT
                PUSHINT 0
            }
            UNTILBRK
            DROP

            PUSHINT 0
        }
        UNTILBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_againbrk_nested() {
    test_case(
        "
        PUSHINT 13
        PUSHCONT {
            DEC
            DUP
            PUSHINT 7
            EQUAL
            IFRETALT
            PUSHINT 0
            PUSHCONT {
                INC
                DUP
                PUSHINT 7
                EQUAL
                IFRETALT
            }
            AGAINBRK
            DROP
        }
        AGAINBRK
        ADDCONST 3",
    )
    .expect_item(int!(10));
}

#[test]
fn test_call() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            INC
            PUSHCONT {
                INC
                PUSHCONT {
                    INC
                }
                CALLX
            }
            CALLX
        }
        CALLX",
    )
    .expect_item(int!(3));
}

#[test]
fn test_whileend_factorial() {
    test_case(
        "
        PUSHINT 5
        DUP
        ; Loop condition
        PUSHCONT {
            SWAP
            DEC
            DUP
        }
        WHILEEND
        ; Loop body
        TUCK
        MUL",
    )
    .expect_stack(Stack::new().push(int!(120)).push(int!(0)));
}

#[test]
fn test_whileendbrk_factorial() {
    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            ; Loop condition
            PUSHCONT {
                DEC
                DUP
            }
            WHILEENDBRK
            ; Loop body
            TUCK
            MUL
            SWAP

            DUP
            DEC
            IFNOTRETALT
        }
        CALLX
        DROP
        PUSHINT 50",
    )
    .expect_stack(Stack::new().push(int!(120)).push(int!(50)));

    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            ; Loop condition
            PUSHCONT {
                DEC
                DUP
            }
            WHILEENDBRK
            ; Loop body
            TUCK
            MUL
            SWAP
        }
        CALLX
        DROP
        PUSHINT 50",
    )
    .expect_stack(Stack::new().push(int!(120)).push(int!(50)));

    // whileend
    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            ; Loop condition
            PUSHCONT {
                DEC
                DUP
            }
            WHILEEND
            ; Loop body
            TUCK
            MUL
            SWAP

            DUP
            DEC
            IFNOTRETALT
        }
        CALLX
        DROP
        PUSHINT 50",
    )
    .expect_int_stack(&[120, 1]);

    // whileend
    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            ; Loop condition
            PUSHCONT {
                DEC
                DUP
            }
            WHILEEND
            ; Loop body
            TUCK
            MUL
            SWAP
        }
        CALLX
        DROP
        PUSHINT 50",
    )
    .skip_fift_check()
    .expect_stack(Stack::new().push(int!(120)).push(int!(50)));
}

#[test]
fn test_repeat_empty_loop() {
    test_case(
        "
        PUSHINT 3
        PUSHINT 0
        PUSHCONT {
            INC
        }
        REPEAT",
    )
    .expect_item(int!(3));
}

#[test]
fn test_repeat_factorial() {
    test_case(
        "
        PUSHINT 5 ; factorial argument
        DUP
        DUP
        DEC
        DEC
        PUSHCONT {
            DEC
            TUCK
            MUL
            SWAP
        }
        REPEAT
        DROP",
    )
    .expect_item(int!(120));
}

#[test]
fn test_repeat_nested_loops() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            PUSHINT 10
            PUSHCONT {
                INC
            }
            REPEAT
        }
        REPEAT",
    )
    .expect_item(int!(50));
}

#[test]
fn test_missing_comma_in_parameters() {
    test_case(
        "PUSH2 s1 s2
       DROP",
    )
    .expect_compilation_failure(CompileError::syntax(1, 10, "Missing comma"));
}

#[test]
fn switch_to_continuation_err_type_check_cont() {
    test_case("JMPX").expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHINT 0
        JMPX
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn execute_continuation() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        CALLX",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn execute_continuation_err_type_stack_underflow() {
    test_case("CALLX").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_too_many_parameters() {
    test_case(
        "PUSHINT 1, 2
       SWAP S1, S2
       DROP",
    )
    .expect_compilation_failure(CompileError::too_many_params(1, 12, "PUSHINT"));

    test_case(
        "DROP
       DROP S1",
    )
    .expect_compilation_failure(CompileError::too_many_params(2, 13, "DROP"));

    test_case(
        "PUSHINT 1 2
       SWAP S1, S2
       DROP",
    )
    .expect_compilation_failure(CompileError::unknown(1, 11, "2"));

    test_case(
        "PUSHCONT PUSHINT 2
       DROP",
    )
    .expect_compilation_failure(CompileError::missing_block(1, 1, "PUSHCONT"));
}

#[test]
fn test_repeat_toobig_number() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2147483648 ; test range check error
        PUSHCONT {
            INC
        }
        REPEAT",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_repeat_toomanyparameters() {
    test_case("REPEAT S0")
        .expect_compilation_failure(CompileError::too_many_params(1, 8, "REPEAT"));
}

#[test]
fn test_repeat_command_stackunderflow() {
    test_case("REPEAT").expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHINT 1
        REPEAT",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHCONT {
            PUSHINT 1
        }
        REPEAT",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_repeat_command_wrongtype() {
    test_case(
        "
        PUSHINT 1
        PUSHSLICE x7_
        REPEAT",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHINT 1
        PUSHSLICE x7_
        PUSHCONT {
            INC
        }
        REPEAT",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_repeatend_empty_loop() {
    test_case(
        "
        PUSHINT 3
        PUSHINT 0
        REPEATEND
        INC",
    )
    .expect_item(int!(3));
}

#[test]
fn test_repeatend_command_simple() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2
        REPEATEND
        INC",
    )
    .expect_item(int!(2));
}

#[test]
fn test_repeatend_nested_command() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        REPEATEND
        PUSHINT 7
        REPEATEND
        INC",
    )
    .expect_item(int!(35));
}

#[test]
fn test_repeatend_factorial() {
    test_case(
        "
        PUSHINT 5 ; parameter
        DUP
        DUP
        DEC
        DEC
        REPEATEND
        SWAP
        DEC
        TUCK
        MUL
        ",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(120)));
}

#[test]
fn test_repeatend_command_stackunderflow() {
    test_case("REPEATEND").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_repeatend_command_wrongtype() {
    test_case(
        "
        PUSHSLICE x7_
        REPEATEND",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            INC
        }
        REPEATEND",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_repeatend_toomanyparameters() {
    test_case("REPEATEND S0").expect_compilation_failure(CompileError::too_many_params(
        1,
        11,
        "REPEATEND",
    ));
}

#[test]
fn test_repeat_simple() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2
        PUSHCONT {
            INC
        }
        REPEAT",
    )
    .expect_item(int!(2));
}

#[test]
fn test_repeat_command_with_ret() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            INC
            DUP
            PUSHINT 3
            SUB
            IFNOTRET
            INC
        }
        REPEAT",
    )
    .expect_item(int!(9));
}

#[test]
fn test_repeatbrk_command_with_retalt() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            INC
            DUP
            PUSHINT 3
            EQUAL
            IFRETALT
        }
        REPEATBRK
        PUSHINT 6",
    )
    .expect_int_stack(&[3, 6]);

    // simple repeat
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            INC
            DUP
            PUSHINT 3
            EQUAL
            IFRETALT
        }
        REPEAT
        PUSHINT 6",
    )
    .expect_int_stack(&[3]);
}

#[test]
fn test_repeatendbrk_command_with_retalt() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 0
            PUSHINT 5
            REPEATENDBRK
            INC
            DUP
            PUSHINT 3
            EQUAL
            IFRETALT
        }
        CALLX
        PUSHINT 50
    ",
    )
    .expect_int_stack(&[3, 50]);

    // repeatend
    test_case(
        "
        PUSHCONT {
            PUSHINT 0
            PUSHINT 5
            REPEATEND
            INC
            DUP
            PUSHINT 3
            EQUAL
            IFRETALT
        }
        CALLX
        PUSHINT 50
    ",
    )
    .expect_item(int!(3));
}

#[test]
fn test_repeatend_command_with_ret() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        REPEATEND
        INC
        DUP
        PUSHINT 3
        SUB
        IFNOTRET
        INC
        ",
    )
    .expect_item(int!(9));
}

#[test]
fn test_whileend_command_with_ret() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            DEC
            DUP
        }
        WHILEEND
        SWAP
        INC
        SWAP
        DUP
        PUSHINT 3
        SUB
        IFNOTRET
        SWAP
        INC
        SWAP
        ",
    )
    .expect_stack(Stack::new().push(int!(7)).push(int!(0)));
}

#[test]
fn test_repeat_inside_while() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        INC
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            PUSHINT 7
            PUSHCONT {
                SWAP
                INC
                SWAP
            }
            REPEAT
        }
        WHILE
        DROP
        ",
    )
    .expect_item(int!(35));
}

#[test]
fn test_repeatend_inside_while() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        INC
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            PUSHINT 7
            REPEATEND
            SWAP
            INC
            SWAP
        }
        WHILE
        DROP
        ",
    )
    .expect_item(int!(35));
}

#[test]
fn test_repeatend_inside_repeat() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            PUSHINT 7
            REPEATEND
            INC
        }
        REPEAT",
    )
    .expect_item(int!(35));
}

#[test]
fn test_repeat_inside_repeatend() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        REPEATEND
        PUSHINT 7
        PUSHCONT {
            INC
        }
        REPEAT",
    )
    .expect_item(int!(35));
}

#[test]
fn test_while_inside_repeatend() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        REPEATEND
        PUSHINT 7
        INC
        PUSHCONT {
            DEC
            DUP
        }
        PUSHCONT {
            SWAP
            INC
            SWAP
        }
        WHILE
        DROP",
    )
    .expect_item(int!(35));
}

#[test]
fn test_while_inside_repeat() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 5
        PUSHCONT {
            PUSHINT 7
            INC
            PUSHCONT {
                DEC
                DUP
            }
            PUSHCONT {
                SWAP
                INC
                SWAP
            }
            WHILE
            DROP
        }
        REPEAT",
    )
    .expect_item(int!(35));
}

#[test]
fn test_whileend_inside_repeatend() {
    test_case(
        "
        PUSHINT 0
        DUP
        PUSHINT 5
        REPEATEND
        DROP
        PUSHINT 7
        INC
        PUSHCONT {
            DEC
            DUP
        }
        WHILEEND
        SWAP
        INC
        SWAP",
    )
    .expect_stack(Stack::new().push(int!(35)).push(int!(0)));
}

#[test]
fn test_whileend_inside_repeat() {
    test_case(
        "
        PUSHINT 0
        DUP
        PUSHINT 5
        PUSHCONT {
            DROP
            PUSHINT 7
            INC
            PUSHCONT {
                DEC
                DUP
            }
            WHILEEND
            SWAP
            INC
            SWAP
        }
        REPEAT",
    )
    .expect_stack(Stack::new().push(int!(35)).push(int!(0)));
}

#[test]
fn test_whileend_inside_while() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2
        INC
        PUSHCONT {
            PUSHINT 2
            ONLYX
            DEC
            DUP
        }
        PUSHCONT {
            PUSHINT 3
            INC
            PUSHCONT {
                DEC
                DUP
            }
            WHILEEND
            XCHG S2
            INC
            XCHG S2
        }
        WHILE",
    )
    .expect_stack(Stack::new().push(int!(6)).push(int!(0)));
}

#[test]
fn test_whileend_inside_whileend() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2
        INC
        PUSHCONT {
            PUSHINT 2
            ONLYX
            DEC
            DUP
        }
        WHILEEND
        PUSHINT 3
        INC
        PUSHCONT {
            DEC
            DUP
        }
        WHILEEND
        XCHG S2
        INC
        XCHG S2",
    )
    .expect_stack(Stack::new().push(int!(6)).push(int!(0)));
}

#[test]
fn test_while_inside_while() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 2
        INC
        PUSHCONT {
            PUSHINT 2
            ONLYX
            DEC
            DUP
        }
        PUSHCONT {
            PUSHINT 3
            INC
            PUSHCONT {
                DEC
                DUP
            }
            PUSHCONT {
                XCHG S2
                INC
                XCHG S2
            }
            WHILE
        }
        WHILE
        DROP",
    )
    .expect_item(int!(6));
}

#[test]
fn test_repeatend_toobig_number() {
    test_case(
        "
        PUSHINT 2147483648 ; test range check error
        REPEATEND
        DEC
        DUP",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_whileend_toomanyparameters() {
    test_case("WHILEEND S0")
        .expect_compilation_failure(CompileError::too_many_params(1, 10, "WHILEEND"));
}
// TODO: need RET in both

#[test]
fn test_whileend_command_stackunderflow() {
    test_case("WHILEEND").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_whileend_command_wrongtype() {
    test_case(
        "
        PUSHINT 1
        WHILEEND",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

// TODO: need RET in both

#[test]
fn test_until_factorial() {
    test_case(
        "
        PUSHINT 5
        DUP
        PUSHCONT {
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            PUSHINT 1
            EQUAL
        }
        UNTIL
        DROP
        ",
    )
    .expect_item(int!(120));
}

#[test]
fn test_untilbrk_factorial() {
    test_case(
        "PUSHINT 5
        DUP
        PUSHCONT {
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            DEC
            IFNOTRETALT
            PUSHINT 0
        }
        UNTILBRK
        DROP
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 50]);

    // simple until
    test_case(
        "PUSHINT 5
        DUP
        PUSHCONT {
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            DEC
            IFNOTRETALT
            PUSHINT 0
        }
        UNTIL
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 1]);
}

#[test]
fn test_untilendbrk_factorial() {
    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            UNTILENDBRK
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            DEC
            IFNOTRETALT
            PUSHINT 0
        }
        CALLX
        DROP
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 50]);

    // untilend
    test_case(
        "PUSHCONT {
            PUSHINT 5
            DUP
            UNTILEND
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            DEC
            IFNOTRETALT
            PUSHINT 0
        }
        CALLX
        DROP
        PUSHINT 50
        ",
    )
    .expect_int_stack(&[120, 1]);
}

#[test]
fn test_until_nested_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            PUSHCONT {
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            UNTIL
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_until_nested_3_times() {
    test_case(
        "
        PUSHINT 16
        PUSHINT 0 ; dummy
        PUSHCONT {
            PUSHCONT {
                PUSHCONT {
                    DROP
                    DEC
                    DUP
                    ISZERO
                    DUP
                }
                UNTIL
                DUP
            }
            UNTIL
            DUP
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(0));
}

#[test]
fn test_until_with_ret() {
    test_case(
        "
        PUSHINT 4
        PUSHCONT {
            INC
            INC
            DUP
            PUSHINT 6
            GREATER
            IFRET
            DUP
            PUSHINT 20
            EQUAL
        }
        UNTIL",
    )
    .expect_empty_stack();
}

#[test]
fn test_until_with_atexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 2
        }
        ATEXITALT
        PUSHCONT {
            PUSHINT 1
            RETALT
            PUSHINT -1
        }
        UNTIL
        PUSHINT 3
    ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(3)));
}

#[test]
fn test_until_with_jmpx() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 2
        }
        PUSHCONT {
            PUSHINT 1
            SWAP
            JMPX
            PUSHINT -1
        }
        UNTIL
        PUSHINT 3
    ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(3)));
}

#[test]
fn test_while_with_jmpx() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 0
        }
        PUSHCONT {
            PUSHINT 1
            SWAP
            JMPX
            PUSHINT -1
        }
        PUSHCONT {
            PUSHINT -1
        }
        WHILE
        PUSHINT 3
    ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(3)));
}

#[test]
fn test_whileend_with_jmpx() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 0
        }
        PUSHCONT {
            PUSHINT 1
            SWAP
            JMPX
            PUSHINT -1
        }
        WHILEEND
        PUSHINT 3
    ",
    )
    .expect_stack(Stack::new().push(int!(1)));
}

#[test]
fn test_untilend_with_jmpx() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 1
            PUSHINT 2
        }
        UNTILEND
        JMPX
        PUSHINT 3
    ",
    )
    .expect_stack(Stack::new().push(int!(1)));
}

#[test]
fn test_until_nested_3_times_with_atexitalt() {
    test_case(
        "
    PUSHCONT {
        PUSHCONT {
            PUSHCONT {
                PUSHINT 2
            }
            ATEXITALT
            PUSHCONT {
                PUSHINT 1
                RETALT
                PUSHINT -1
            }
            UNTIL
            PUSHINT 3
        }
        UNTIL
        PUSHINT 4
    }
    UNTIL
    PUSHINT 5
    ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(5)));
}

#[test]
fn test_untilend_factorial() {
    test_case(
        "
        PUSHINT 5
        DUP
        PUSHCONT {
            UNTILEND
            DUP
            DEC
            XCHG s1, s2
            MUL
            SWAP
            DEC
            DUP
            ONE
            EQUAL
        }
        CALLX
        DROP
        ",
    )
    .expect_item(int!(120));
}

#[test]
fn test_while_inside_until_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            PUSHCONT {
                PUSH s1
                PUSH s1
                GREATER
            }
            PUSHCONT {
                SWAP
            }
            WHILE
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_until_loop_with_exit_on_throw() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 10
            PUSHCONT {
                DEC
                DUP
                PUSHINT 0
                GREATER

                PUSHCONT {
                    THROW 100
                }
                IFNOT
                PUSHINT 0
            }
            UNTIL
            PUSHINT -1
        }
        PUSHCONT {
            PUSHINT 4
        }
        TRY
    ",
    )
    .expect_int_stack(&[0, 100, 4]);
}

#[test]
fn test_while_loop_with_exit_on_throw() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 10
            PUSHCONT {
                DEC
                PUSHINT -1
            }
            PUSHCONT {
                DUP
                PUSHINT 0
                GREATER

                PUSHCONT {
                    THROW 100
                }
                IFNOT
            }
            WHILE
            PUSHINT -1
        }
        PUSHCONT {
            PUSHINT 4
        }
        TRY
    ",
    )
    .expect_int_stack(&[0, 100, 4]);
}

#[test]
fn test_untilend_nested_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            UNTILEND
            PUSHCONT {
                UNTILEND
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            CALLX
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        CALLX
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_untilend_inside_until_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            PUSHCONT {
                UNTILEND
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            CALLX
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_until_inside_untilend_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            UNTILEND
            PUSHCONT {
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            UNTIL
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        CALLX
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_until_inside_until_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            PUSHCONT {
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            UNTIL
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_until_inside_while_gcd() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            DUP
            ISPOS
        }
        PUSHCONT {
            PUSHCONT {
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            UNTIL
            PUSH s1
            SUB
        }
        WHILE
        DROP",
    )
    .expect_item(int!(13));
}

#[test]
fn test_untilend_with_ret() {
    test_case(
        "
        PUSHINT 4
        PUSHCONT {
            UNTILEND
            INC
            INC
            DUP
            PUSHINT 6
            GREATER
            IFRET
            DUP
            PUSHINT 20
            EQUAL
        }
        CALLX",
    )
    .expect_empty_stack();
}

#[test]
fn test_untilend_with_ifret() {
    test_case(
        "
        PUSHINT 1
        PUSHINT -10
        PUSHCONT {
            UNTILEND
            INC
            INC
            DUP
            PUSHINT 6
            GREATER
            IFRET
            DUP
            PUSHINT 20
            EQUAL
        }
        CALLX
        PUSHINT 8",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(8)));
}

#[test]
fn test_untilend_with_until() {
    test_case(
        "
        PUSHINT 377
        PUSHINT 169
        PUSHCONT {
            PUSHCONT {
                UNTILEND
                SWAP
                PUSH s1
                PUSH s1
                LEQ
            }
            CALLX
            PUSH s1
            SUB
            DUP
            ISZERO
        }
        UNTIL
        DROP",
    )
    .expect_item(int!(13));

    test_case(
        "
        PUSHINT -10
        PUSHINT -10
        PUSHINT 10
        PUSHCONT {
            UNTILEND
            DEC
            DUP
            PUSHINT 0
            LEQ
        }
        UNTIL
        ",
    )
    .expect_item(int!(-10));
}

#[test]
fn test_untilend_nested_3_times() {
    test_case(
        "
        PUSHCONT {
            UNTILEND
            PUSHINT 377
            PUSHINT 169
            PUSHCONT {
                UNTILEND
                PUSHCONT {
                    UNTILEND
                    SWAP
                    PUSH s1
                    PUSH s1
                    LEQ
                }
                CALLX
                PUSH s1
                SUB
                DUP
                ISZERO
            }
            CALLX
            INC
        }
        CALLX
        PUSHINT 100
        ",
    )
    .expect_stack(Stack::new().push(int!(13)).push(int!(100)));
}

#[test]
fn test_callx_consecutive() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            INC
        }
        CALLX
        PUSHCONT {
            INC
        }
        CALLX
        ",
    )
    .expect_item(int!(2));
}

#[test]
fn test_callx_nested() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            PUSHCONT {
                INC
            }
            CALLX
        }
        CALLX
        ",
    )
    .expect_item(int!(1));
}

#[test]
fn test_callx_nested_consecutive() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            PUSHCONT {
                INC
            }
            CALLX
            PUSHCONT {
                INC
            }
            CALLX
        }
        CALLX
        ",
    )
    .expect_item(int!(2));
}

#[test]
fn test_again_loop_with_exit_on_throw() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 3
            PUSHCONT {
                DEC
                DUP
                IFRET
                DROP
                THROW 100
            }
            AGAIN
            PUSHINT -1
        }
        PUSHCONT {
            PUSHINT 4
        }
        TRY
    ",
    )
    .expect_int_stack(&[0, 100, 4]);
}

#[test]
fn test_again_break_loop_with_retalt() {
    test_case(
        "
        PUSHINT 3
        DUP
        PUSHCONT {
            DEC
            DUP
            DUP
            IFNOTRETALT
        }
        AGAINBRK
        PUSHINT 4
    ",
    )
    .expect_int_stack(&[3, 2, 1, 0, 0, 4]);

    // simple again
    test_case(
        "
        PUSHINT 3
        DUP
        PUSHCONT {
            DEC
            DUP
            DUP
            IFNOTRETALT
        }
        AGAIN
        PUSHINT 4
    ",
    )
    .expect_int_stack(&[3, 2, 1, 0, 0]);
}

#[test]
fn test_againend_break_loop_with_retalt() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 3
            DUP
            AGAINENDBRK
            DEC
            DUP
            DUP
            IFNOTRETALT
        }
        CALLX
        PUSHINT 4
    ",
    )
    .with_root_data(Default::default())
    .expect_int_stack(&[3, 2, 1, 0, 0, 4]);

    // againend
    test_case(
        "
        PUSHCONT {
            PUSHINT 3
            DUP
            AGAINEND
            DEC
            DUP
            DUP
            IFNOTRETALT
        }
        CALLX
        PUSHINT 4
    ",
    )
    .expect_int_stack(&[3, 2, 1, 0, 0]);
}

#[test]
fn test_again_loop_with_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT 2
            }
            SETEXITALT
            PUSHCONT {
                PUSHINT 1
                RETALT
                PUSHINT -1
            }
            AGAIN
        }
        CALLX
        PUSHINT 3
    ",
    )
    .expect_int_stack(&[1, 2, 3]);
}

#[test]
fn test_againend_loop_with_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT 2
            }
            SETEXITALT
            AGAINEND
            PUSHINT 1
            RETALT
            PUSHINT -1
        }
        CALLX
        PUSHINT 3
    ",
    )
    .expect_int_stack(&[1, 2, 3]);
}

#[test]
fn test_again_loop_with_exception() {
    test_case(
        "
        PUSHINT 3
        PUSHCONT {
            DEC
            DUP
            IFRET
            INC
            PUSH s1
            RETALT
        }
        AGAIN
        PUSHINT -1
        PUSHINT -2",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_again_loop_too_many_args() {
    test_case(
        "
        PUSHINT 3
        PUSHCONT {
            DEC
            DUP
            IFRET
            INC
            PUSHCONT {
                PUSHINT 5
            }
            POPCTR c1
            RETALT
        }
        AGAIN 1",
    )
    .expect_compilation_failure(CompileError::too_many_params(14, 15, "AGAIN"));
}

#[test]
fn test_again_loop_wrongtype() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT 5
            }
            POPCTR c1
            RETALT
        }
        PUSHINT 3
        AGAIN",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_againend_loop_with_exception() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 4
        }
        PUSHINT 3
        AGAINEND
        DEC
        DUP
        IFRET
        DROP
        PUSH s1
        RETALT",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_ifret_on_nan() {
    test_case(
        "
        PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHNAN
        IFRET
    ",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn test_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT 2
            }
            SETEXITALT
        }
        CALLX
        PUSHINT 3
    ",
    )
    .expect_int_stack(&[3]);
}

#[test]
fn test_retalt_in_nested_calls() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 5
                }
                ATEXITALT
                PUSHINT 1
                PUSHCONT {
                    PUSHINT 4
                    RETALT
                }
                IFJMP
                PUSHINT 6
            }
            CALLX
            PUSHINT 7
        }
        CALLX
    ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(int!(4)).push(int!(5)).push(int!(7)));
}

#[test]
fn test_ifretalt_in_nested_calls() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 5
                }
                ATEXITALT
                PUSHINT 1
                PUSHCONT {
                    PUSHINT 4
                    PUSHINT 1
                    IFRETALT
                }
                IFJMP
                PUSHINT 6
            }
            CALLX
            PUSHINT 7
        }
        CALLX
    ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(int!(4)).push(int!(5)).push(int!(7)));
}

#[test]
fn test_ifnotretalt_in_nested_calls() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 5
                }
                ATEXITALT
                PUSHINT 1
                PUSHCONT {
                    PUSHINT 4
                    PUSHINT 0
                    IFNOTRETALT
                }
                IFJMP
                PUSHINT 6
            }
            CALLX
            PUSHINT 7
        }
        CALLX
    ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(int!(4)).push(int!(5)).push(int!(7)));
}

#[test]
fn test_again_loop_with_retalt_and_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 0
        }
        POPCTR c0
        PUSHCONT {
            PUSHINT 1   ;should be unreachable
        }
        POPCTR c1
        PUSHCONT {
            PUSHINT 2
        }
        SETEXITALT
        PUSHINT 3
        PUSHCONT {
            DEC
            DUP
            IFRET
            DROP
            RETALT
        }
        AGAIN
        PUSHINT 3       ;should be unreachable",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(0)));
}

#[test]
fn test_nested_again_loop_with_retalt_and_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT -1  ;should be unreachable
            }
            POPCTR c1
            PUSHCONT {
                PUSHINT 1
            }
            SETEXITALT
            PUSHINT 10
            PUSHCONT {
                DEC
                DUP
                IFRET
                DROP
                PUSHINT 10
                PUSHCONT {
                    DEC
                    DUP
                    IFRET
                    DROP
                    RETALT
                }
                AGAIN
                PUSHINT -1  ;should be unreachable
            }
            AGAIN
            PUSHINT -1      ;should be unreachable
        }
        CALLX
        PUSHINT 0
        ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(0)));
}

#[test]
fn test_nested_again_loop_with_retbool_and_setexitalt() {
    test_case(
        "
        PUSHCONT {
            PUSHCONT {
                PUSHINT -1  ;should be unreachable
            }
            POPCTR c1
            PUSHCONT {
                PUSHINT 1
            }
            SETEXITALT
            PUSHINT 10
            PUSHCONT {
                DEC
                DUP
                IFRET
                DROP
                PUSHINT 10
                PUSHCONT {
                    DEC
                    DUP
                    IFRET
                    RETBOOL ; equivalent of RETALT
                }
                AGAIN
                PUSHINT -1  ;should be unreachable
            }
            AGAIN
            PUSHINT -1      ;should be unreachable
        }
        CALLX
        PUSHINT 0
        ",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(0)));
}

#[test]
fn test_again_loop_with_retalt_and_ret() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 4
        }
        PUSHINT 3
        PUSHCONT {
            DEC
            DUP
            IFRET
            DROP
            POPCTR c1
            RETALT
        }
        AGAIN
        PUSHINT 1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_thenret() {
    test_case(
        "
        PUSHCONT {
            ADDCONST 10
        }
        POP c0
        PUSHCONT {
            PUSHINT 1
        }
        THENRET
        CALLX
        ",
    )
    .expect_item(int!(11));
}

#[test]
fn test_thenret_exception() {
    test_case(
        "
        PUSHCONT {
            ADDCONST 10
        }
        POP c0
        PUSHINT 0
        THENRET
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            ADDCONST 10
        }
        POP c0
        PUSHSLICE x4_
        THENRET
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            ADDCONST 10
        }
        POP c0
        NEWC
        ENDC
        THENRET
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            ADDCONST 10
        }
        POP c0
        NEWC
        THENRET
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_thenretalt() {
    test_case(
        "
        PUSHCONT {
            MULCONST 100
        }
        POP c1
        PUSHCONT {
            PUSHINT 2
        }
        THENRETALT
        CALLX
        ",
    )
    .expect_item(int!(200));
}

#[test]
fn test_thenretalt_exception() {
    test_case(
        "
        PUSHCONT {
            MULCONST 100
        }
        POP c1
        PUSHINT 0
        THENRETALT
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            MULCONST 100
        }
        POP c1
        PUSHSLICE x4_
        THENRETALT
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            MULCONST 100
        }
        POP c1
        NEWC
        ENDC
        THENRETALT
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHCONT {
            MULCONST 100
        }
        POP c1
        NEWC
        THENRETALT
        CALLX
        ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_again_loop_infinite() {
    test_case(
        "
        PUSHCONT {
        }
        AGAIN
    ",
    )
    .with_gas(ton_vm::executor::gas::gas_state::Gas::test_with_limit(1000))
    .expect_steps(196)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_repeat_loop_infinite() {
    test_case(
        "
        PUSHINT 10000
        PUSHCONT {
        }
        REPEAT
    ",
    )
    .with_gas(ton_vm::executor::gas::gas_state::Gas::test_with_limit(1000))
    .expect_steps(191)
    .expect_failure(ExceptionCode::OutOfGas);
}
