/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_assembler::{compile_code, compile_code_to_cell, CompileError};
use ton_block::{ExceptionCode, SliceData};
use ton_vm::{
    int,
    stack::{continuation::ContinuationData, integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[ignore] // we should have c0, c1, c2 and other instantiated
#[test]
fn test_pushctr_uninitialized_register() {
    test_case("PUSHCTR c1").expect_failure(ExceptionCode::TypeCheckError);
}

mod cont_bytecode {
    use super::*;

    #[test]
    fn common_use_case() {
        test_case(
            "PUSHCONT {
                PUSHINT 1
                PUSHINT 2
            }",
        )
        .expect_bytecode(vec![0x92, 0x71, 0x72, 0x80]);

        test_case(
            "PUSHCONT {
                PUSHCONT {
                    PUSHINT 100
                }
                PUSHCONT {
                    PUSHINT 100
                }
                PUSHCONT {
                    PUSHINT 100
                }
            }",
        )
        .expect_bytecode(vec![0x99, 0x92, 0x80, 0x64, 0x92, 0x80, 0x64, 0x92, 0x80, 0x64, 0x80]);
    }

    #[test]
    fn more_than_15bytes() {
        test_case(
            "PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                PUSHINT 4
                PUSHINT 5
                PUSHINT 6
                PUSHINT 7
                PUSHINT 8
                PUSHINT 9
                PUSHINT 10
                PUSHINT 11
                PUSHINT 12
            }",
        )
        .expect_bytecode(vec![
            0x9E, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x80, 0x0B, 0x80,
            0x0C, 0x80,
        ]);
    }

    #[test]
    fn recursion() {
        test_case(
            "PUSHCONT {
                PUSHCONT {
                    PUSHCONT {
                        PUSHCONT {
                            PUSHCONT {
                                NOP
                            }
                        }
                    }
                }
            }",
        )
        .expect_bytecode(vec![0x95, 0x94, 0x93, 0x92, 0x91, 0x0, 0x80]);
    }

    #[test]
    fn empty_recursion() {
        test_case(
            "PUSHCONT {
                PUSHCONT {
                    PUSHCONT {
                        PUSHCONT {
                            PUSHCONT {
                            }
                        }
                    }
                }
            }",
        )
        .expect_bytecode(vec![0x94, 0x93, 0x92, 0x91, 0x90, 0x80]);
    }
}

#[test]
fn test_pushctr_different_type() {
    test_case(
        "PUSHCONT {
             PUSHINT 4
         }
         POPCTR c4
         PUSHCTR c4",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "NEWDICT
         POPCTR c4
         PUSHCTR c4",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "NEWC
         ENDC
         POPCTR c1
         PUSHCTR c1",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "NEWC
         ENDC
         POPCTR c2
         PUSHCTR c2",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "NEWC
         ENDC
         POPCTR c3
         PUSHCTR c3",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_pushctr() {
    let cont = ContinuationData::with_code(compile_code("PUSHINT 4").unwrap());
    test_case(
        "PUSHCONT {
             PUSHINT 4
         }
         POPCTR c1
         PUSHCTR c1
         POPCTR c2
         PUSHCTR c2
         POPCTR c3
         PUSHCTR c3",
    )
    .expect_stack(Stack::new().push_cont(cont));

    test_case(
        "PUSHINT 4
         NEWC
         STU 16
         ENDC
         POPCTR c4
         PUSHCTR c4
         CTOS
         LDU 16
         POP s0",
    )
    .expect_item(int!(4));
}

#[test]
fn test_pushslice() {
    test_case(
        "PUSHSLICE x788_
         LDU 4
         DROP",
    )
    .expect_item(int!(7));
}

#[test]
fn pushcont_simple() {
    let cont = ContinuationData::with_code(SliceData::new(vec![0x00, 0x00, 0x00, 0x80]));
    test_case(
        "PUSHCONT {
            NOP
            NOP
            NOP
        }",
    )
    .expect_stack(Stack::new().push_cont(cont));
}

#[test]
fn bless() {
    let cont = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    test_case(
        "PUSHSLICE x2_
         PUSHSLICE x1_
         PUSHSLICE x3_
         NEWC
         STSLICE
         STSLICE
         STSLICE
         ENDC
         CTOS
         BLESS",
    )
    .expect_stack(Stack::new().push_cont(cont));
}

#[test]
fn blessargs() {
    let mut cont = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    cont.nargs = 2;
    cont.stack.push(int!(1));
    cont.stack.push(int!(2));
    test_case(
        "PUSHSLICE x2_
         PUSHSLICE x1_
         PUSHSLICE x3_
         NEWC
         STSLICE
         STSLICE
         STSLICE
         ENDC
         CTOS
         PUSHINT 1
         SWAP
         PUSHINT 2
         SWAP
         BLESSARGS 2, 2",
    )
    .expect_stack(Stack::new().push_cont(cont));
}

#[test]
fn blessvarargs() {
    let mut cont = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    cont.nargs = 2;
    cont.stack.push(int!(1));
    cont.stack.push(int!(2));
    test_case(
        "PUSHSLICE x2_
         PUSHSLICE x1_
         PUSHSLICE x3_
         NEWC
         STSLICE
         STSLICE
         STSLICE
         ENDC
         CTOS
         PUSHINT 1
         SWAP
         PUSHINT 2
         SWAP
         PUSHINT 2
         PUSHINT 2
         BLESSVARARGS",
    )
    .expect_stack(Stack::new().push_cont(cont));
}

#[test]
fn callx() {
    test_case(
        "PUSHINT 1
         PUSHCONT {
             PUSHINT 2
             MUL
         }
         SETCONTARGS 0, 1
         CALLX",
    )
    .expect_success();
}

#[test]
fn callxargs_normal() {
    test_case(
        "
        PUSHINT 10
        PUSHINT 20
        PUSHINT 30
        PUSHCONT {
            DEPTH
            PUSHINT 40
            SWAP
        }
        SETNUMARGS 1
        CALLXARGS 1, 1
    ",
    )
    .expect_int_stack(&[10, 20, 1]);
}

#[test]
fn callxargs_with_callx() {
    test_case(
        "
        PUSHINT 5
        PUSHCONT {
            PUSHINT 10
            PUSHINT 20
            PUSHINT 30
            PUSHCONT {
                DEPTH
                PUSHINT 40
                SWAP
            }
            CALLXARGS 1, 1
            PUSHCONT {
                ZERO
            }
            CALLX
        }
        CALLX
    ",
    )
    .expect_int_stack(&[5, 10, 20, 1, 0]);

    test_case(
        "
        PUSHINT 5
        PUSHCONT {
            PUSHINT 10
            PUSHINT 20
            PUSHCONT {
                INC
            }
            CALLXARGS 1, 0
            PUSHCONT {
                ZERO
            }
            CALLX
        }
        CALLX
    ",
    )
    .expect_int_stack(&[5, 10, 0]);
}

#[test]
fn callxargs_underflow_params() {
    test_case(
        "
        PUSHINT 10
        PUSHINT 20
        PUSHINT 30
        PUSHCONT {
            DEPTH
            PUSHINT 40
            SWAP
        }
        SETNUMARGS 2
        CALLXARGS 1, 1
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn push_pop_continuation() {
    let cont0 = ContinuationData::with_code(compile_code("PUSHINT 0 NOP NOP").unwrap());
    let cont1 = ContinuationData::with_code(compile_code("PUSHINT 1 NOP NOP").unwrap());
    test_case(
        "PUSHCONT {
             PUSHINT 0
             NOP
             NOP
         }
         PUSHCONT {
             PUSHINT 1
             NOP
             NOP
         }
         POP c1
         POP c2",
    )
    .expect_stack(&Stack::new())
    .expect_ctrl(2, &StackItem::continuation(cont0))
    .expect_ctrl(1, &StackItem::continuation(cont1));
}

#[test]
fn test_bug_pop_c2_null() {
    test_case(
        "
        PUSH c2
        ISNULL
    ",
    )
    .expect_int_stack(&[0]);

    test_case(
        "
        PUSHCONT {}
        AGAINEND
        POP c2
        NULL
    ",
    )
    .with_gas_limit(1000)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn returnargs() {
    test_case(
        "
        PUSHCONT {
            DEPTH
        }
        SETNUMARGS 3
        POPCTR c0
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHINT 4
        PUSHINT 5
        PUSHINT 6
        RETURNARGS 4
    ",
    )
    .expect_int_stack(&[1, 2, 6, 3]);
}

#[test]
fn setnumvarargs_pos() {
    test_case(
        "
        PUSHINT 10
        PUSHINT 20
        PUSHINT 30
        PUSHINT 40
        PUSHCONT {
            DEPTH
        }
        PUSHINT 2
        SETNUMVARARGS
        CALLX
    ",
    )
    .expect_int_stack(&[10, 20, 30, 40, 2]);
}

#[test]
fn setnumvarargs_minusone() {
    test_case(
        "
        PUSHINT 0
        PUSHINT 10
        PUSHINT 20
        PUSHINT 30
        PUSHCONT {
            DEPTH
        }
        PUSHINT -1
        SETNUMVARARGS
        CALLX
    ",
    )
    .expect_int_stack(&[0, 10, 20, 30, 4]);
}

#[test]
fn setnumvarargs_bad_stack_arg() {
    test_case(
        "PUSHINT 0
         PUSHCONT {
             PUSHINT 4
         }
         PUSHINT 256
         SETNUMVARARGS
         CALLX",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

mod returnvarargs {
    use super::*;

    #[test]
    fn test_simple_ok() {
        test_case(
            "
            PUSHCONT {
                DEPTH
            }
            SETNUMARGS 3
            POPCTR c0
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHINT 4
            PUSHINT 5
            PUSHINT 6
            PUSHINT 4
            RETURNVARARGS
        ",
        )
        .expect_int_stack(&[1, 2, 6, 3]);
    }

    #[test]
    fn test_simple_all() {
        test_case(
            "
            PUSHCONT {
                DEPTH
            }
            SETNUMARGS 3
            POPCTR c0
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHINT 0
            RETURNVARARGS
        ",
        )
        .expect_int_stack(&[1, 2, 3, 3]);
    }

    #[test]
    fn test_simple_underflow() {
        test_case(
            "
            PUSHCONT {
                DEPTH
            }
            SETNUMARGS 3
            POPCTR c0
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHINT 4
            PUSHINT 5
            RETURNVARARGS
        ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

#[test]
fn save() {
    let cont = ContinuationData::with_code(compile_code("NOP").unwrap());
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         POPCTR c0
         PUSHCONT {
             NOP
         }
         POPCTR C1
         SAVE C1
         PUSHINT 1",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(1)))
    .expect_ctrl(1, &StackItem::continuation(cont));
}

#[test]
fn savealt() {
    let cont0 = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    let mut cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    assert!(cont1.put_to_savelist(0, StackItem::continuation(cont0)).is_ok());
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         POPCTR c0
         PUSHCONT {
             PUSHINT 2
         }
         POPCTR C1
         SAVEALT C0
         PUSHINT 1
         RETALT",
    )
    .expect_int_stack(&[1, 2, 2]);
}

#[ignore]
#[test]
fn saveboth() {
    let mut cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    let cont2 = ContinuationData::with_code(compile_code("NOP NOP").unwrap());
    assert!(cont1.put_to_savelist(2, StackItem::continuation(cont2.clone())).is_ok());
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         POPCTR c0
         PUSHCONT {
             PUSHINT 2
         }
         POPCTR C1
         PUSHCONT {
             NOP
             NOP
         }
         POPCTR C2
         SAVEALT C0
         SAVEBOTH C2
         PUSHINT 1
         RETALT",
    )
    .expect_int_stack(&[1, 2, 2])
    .expect_ctrl(2, &StackItem::continuation(cont2));

    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            NOP
        }
        POP c0
        SAVEBOTH c0
        ",
    )
    .expect_item(int!(1));

    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            NOP
        }
        POP c1
        SAVEBOTH c1
        ",
    )
    .expect_item(int!(1));

    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            NOP
        }
        POP c2
        SAVEBOTH c2
        ",
    )
    .expect_item(int!(1));

    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            NOP
        }
        POP c3
        SAVEBOTH c3
        ",
    )
    .expect_item(int!(1));

    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            NOP
        }
        POP c9
        SAVEBOTH c9
        ",
    )
    .expect_item(int!(1));
}

#[ignore] // it will be fixed in other PR
#[test]
fn test_save_both_failure() {
    test_case(
        "
        PUSHCONT {
            SAVEBOTH c7
        }
        CALLX
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn setaltctr() {
    let mut cont0 = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    let cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    cont0.put_to_savelist(0, StackItem::continuation(cont1)).unwrap();
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         POPCTR c1
         PUSHCONT {
             NOP
         }
         SETALTCTR c0",
    )
    .expect_ctrl(1, &StackItem::continuation(cont0));
}

#[test]
fn setcontargs() {
    let mut cont = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    cont.nargs = 2;
    cont.stack.push(int!(1));
    cont.stack.push(int!(2));
    cont.stack.push(int!(2));
    cont.stack.push(int!(3));
    test_case(
        "PUSHSLICE x2_
         PUSHSLICE x1_
         PUSHSLICE x3_
         NEWC
         STSLICE
         STSLICE
         STSLICE
         ENDC
         CTOS
         PUSHINT 1
         SWAP
         PUSHINT 2
         SWAP
         BLESSARGS 2, 2
         PUSHINT 1
         SWAP
         PUSHINT 2
         SWAP
         PUSHINT 3
         SWAP
         SETCONTARGS 2, 2",
    )
    .expect_stack(Stack::new().push(int!(1)).push_cont(cont));
}

#[test]
fn setcontctr() {
    let mut cont0 = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    let cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    cont0.put_to_savelist(0, StackItem::continuation(cont1)).unwrap();
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         PUSHCONT {
             NOP
         }
         SWAP
         SETCONTCTR c0",
    )
    .expect_stack(Stack::new().push_cont(cont0));
}

#[test]
fn setcontctrx() {
    let mut cont0 = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    let cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    cont0.put_to_savelist(0, StackItem::continuation(cont1)).unwrap();
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         PUSHCONT {
             NOP
         }
         SWAP
         PUSHINT 0
         SETCONTCTRX",
    )
    .expect_stack(Stack::new().push_cont(cont0));
}

#[test]
fn setcontctrmany_normal() {
    let mut cont0 = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    test_case("PUSHCONT { DUP } SETCONTCTRMANY 0")
        .expect_stack(Stack::new().push_cont(cont0.clone()));

    let cont1 = ContinuationData::with_code(compile_code("ONE").unwrap());
    let cont2 = ContinuationData::with_code(compile_code("TWO").unwrap());
    cont0.put_to_savelist(0, StackItem::continuation(cont1)).unwrap();
    cont0.put_to_savelist(1, StackItem::continuation(cont2)).unwrap();
    test_case(
        "PUSHCONT {
            TWO
        }
        POPCTR c1
        PUSHCONT {
            ONE
        }
        POPCTR c0
        PUSHCONT {
            PUSH s0
        }
        SETCONTCTRMANY 3",
    )
    .expect_stack(Stack::new().push_cont(cont0.clone()).push_int(1));

    test_case(
        "PUSHCONT {
            TWO
        }
        POPCTR c1
        PUSHCONT {
            ONE
        }
        POPCTR c0
        PUSHCONT {
            PUSH s0
        }
        PUSHINT 3
        SETCONTCTRMANYX",
    )
    .expect_stack(Stack::new().push_cont(cont0.clone()).push_int(1));
}

#[test]
fn setcontctrmany_error() {
    expect_exception("SETCONTCTRMANY 0", ExceptionCode::StackUnderflow);
    expect_exception("PUSHCONT {} SETCONTCTRMANY 64", ExceptionCode::RangeCheckError);
    expect_exception("ZERO SETCONTCTRMANY 1", ExceptionCode::TypeCheckError);

    expect_exception("SETCONTCTRMANYX", ExceptionCode::StackUnderflow);
    expect_exception("PUSHINT 0 SETCONTCTRMANYX", ExceptionCode::StackUnderflow);
    expect_exception("PUSHCONT {} SETCONTCTRMANYX", ExceptionCode::StackUnderflow);
    expect_exception("PUSHCONT {} NULL SETCONTCTRMANYX", ExceptionCode::TypeCheckError);
    expect_exception("PUSHCONT {} PUSHINT 64 SETCONTCTRMANYX", ExceptionCode::RangeCheckError);
    expect_exception("ZERO PUSHINT 1 SETCONTCTRMANYX", ExceptionCode::TypeCheckError);
}

#[test]
fn test_setcontctrx_range() {
    test_case(
        "
        PUSHCONT {
        }
        PUSHCONT {
        }
        PUSHINT 10
        SETCONTCTRX
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn setcontvarargs() {
    let mut cont = ContinuationData::with_code(compile_code("PUSH s0").unwrap());
    cont.nargs = 2;
    cont.stack.push(int!(1));
    cont.stack.push(int!(2));
    cont.stack.push(int!(1));
    cont.stack.push(int!(2));
    test_case(
        "
        PUSHSLICE x20   ; PUSH s0
        PUSHINT 1
        PUSHINT 2
        ROT
        BLESSARGS 2, 4
        PUSHINT 1
        PUSHINT 2
        ROT
        PUSHINT 2
        PUSHINT 2
        SETCONTVARARGS
    ",
    )
    .expect_stack(Stack::new().push_cont(cont));
}

#[test]
fn setretctr() {
    let cont1 = ContinuationData::with_code(compile_code("NOP").unwrap());
    test_case(
        "PUSHCONT {
             PUSH s0
         }
         POPCTR c0
         PUSHCONT {
             NOP
         }
         SETRETCTR c1
         PUSHINT 1",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(1)))
    .expect_ctrl(1, &StackItem::continuation(cont1));
}

#[test]
fn test_setretctr_range() {
    test_case(
        "
        PUSHINT 10
        SETRETCTR c2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHINT 10
        SETRETCTR c10
    ",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn test_popctr() {
    let cont = ContinuationData::with_code(compile_code("PUSHINT 7").unwrap());
    test_case(
        "PUSHCONT {
             PUSHINT 7
         }
         POPCTR c1
         PUSHCTR c1",
    )
    .expect_stack(Stack::new().push_cont(cont));

    test_case(
        "NEWC
         ENDC
         POPCTR c1",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case("POPCTR c1").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_command_callcc() {
    test_case(
        "
        PUSHINT 3
        PUSHCONT {
            SWAP
            INC
            SWAP
            POPCTR C0
        }
        CALLCC
        PUSHINT 2
        MUL
    ",
    )
    .expect_int_stack(&[8]);
}

#[test]
fn test_command_callxvarargs() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHCONT {
             PUSHINT 2
             MUL
             DUP
             DUP
             ADD
         }
         PUSHINT 1
         PUSHINT 1
         CALLXVARARGS",
    )
    .expect_int_stack(&[2, 4]);
}

#[test]
fn test_command_callccvarargs() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHCONT {
             SWAP
             PUSHINT 2
             MUL
             DUP
             DUP
             ADD
             XCHG S2
             POPCTR C0
         }
         PUSHINT 1
         PUSHINT 1
         CALLCCVARARGS
         INC",
    )
    .expect_int_stack(&[2, 3]);
}

#[test]
fn test_command_callccargs() {
    test_case(
        "PUSHINT 2
         PUSHINT 1
         PUSHCONT {
             SWAP
             PUSHINT 2
             MUL
             DUP
             DUP
             ADD
             XCHG S2
             POPCTR C0
         }
         CALLCCARGS 1,1
         INC",
    )
    .expect_int_stack(&[2, 3]);
}

#[test]
fn execute_command_retbool() {
    test_case(
        "
        PUSHCONT {
            PUSHINT 4
        }
        POPCTR C0
        RETTRUE
    ",
    )
    .expect_int_stack(&[4]);

    test_case(
        "
        PUSHCONT {
            PUSHINT 5
        }
        POPCTR C1
        RETFALSE
    ",
    )
    .expect_int_stack(&[5]);

    test_case(
        "
        PUSHCONT {
            PUSHINT 6
        }
        POPCTR C0
        PUSHCONT {
            PUSHINT 7
        }
        POPCTR C1
        PUSHINT 1
        BRANCH
    ",
    )
    .expect_int_stack(&[6]);

    test_case(
        "
        PUSHCONT {
            PUSHINT 8
        }
        POPCTR C0
        PUSHCONT {
            PUSHINT 9
        }
        POPCTR C1
        PUSHINT 0
        BRANCH
    ",
    )
    .expect_int_stack(&[9, 8]);
}

#[test]
fn test_command_jpmxdata() {
    let slice = compile_code("ZERO").unwrap();
    test_case(
        "
        PUSHCONT {
            NOP
        }
        JMPXDATA
        ZERO
    ",
    )
    .expect_stack(Stack::new().push(StackItem::Slice(slice)));
}

#[test]
fn test_command_retdata() {
    let slice = compile_code("ZERO").unwrap();
    test_case(
        "
        PUSHCONT {
            NOP
        }
        POPCTR C0
        RETDATA
        ZERO
    ",
    )
    .expect_stack(Stack::new().push(StackItem::Slice(slice)));
}

#[test]
fn test_command_jmpxargs() {
    test_case(
        "
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHCONT {
            MUL
        }
        JMPXARGS 2
        NOP
    ",
    )
    .expect_int_stack(&[6]);
}

mod jmpxvarargs {
    use super::*;

    #[test]
    fn test_simple() {
        test_case(
            "
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHCONT {
                MUL
            }
            PUSHINT 2
            JMPXVARARGS
            NOP
        ",
        )
        .expect_int_stack(&[6]);
    }

    #[test]
    fn test_nargs_ok() {
        test_case(
            "
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHCONT {
                MUL
            }
            SETNUMARGS 2
            PUSHINT 2
            JMPXVARARGS
            NOP
        ",
        )
        .expect_int_stack(&[6]);
    }

    #[test]
    fn test_nargs_underflow() {
        test_case(
            "
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHCONT {
                MUL
            }
            SETNUMARGS 3
            PUSHINT 2
            JMPXVARARGS
            NOP
        ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_nargs_pargs_ok() {
        test_case(
            "
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHCONT {
                MUL
            }
            SETCONTARGS 1, 1
            SETNUMARGS 1
            PUSHINT 1
            JMPXVARARGS
            NOP
        ",
        )
        .expect_int_stack(&[6]);
    }
}

#[test]
fn checkatexitcode() {
    let exp_vec: Vec<u8> = vec![0x91, 0x00, 0xED, 0xF3, 0x80];
    test_case(
        "PUSHCONT {
             NOP
         }
         ATEXIT",
    )
    .expect_bytecode(exp_vec);
}

#[test]
fn checkatexitcode_error() {
    test_case(
        "
        PUSHINT 1
        ATEXIT
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn checkatexitstackchange() {
    let cont = ContinuationData::with_code(compile_code("NOP").unwrap());
    test_case(
        "PUSHCONT {
             NOP
         }
         PUSHCONT {
             PUSHINT 1
         }
         ATEXIT",
    )
    .expect_stack(Stack::new().push_cont(cont).push(int!(1)));
}

#[test]
fn checkatexitcallorder() {
    test_case(
        "PUSHINT 1
         PUSHCONT {
             PUSHCONT {
                 SWAP
                 PUSHINT 5
             }
             ATEXIT
             PUSHCONT {
                 LSHIFT 1
             }
         }
         CALLX
         ADD
         SWAP
         ATEXIT",
    )
    .expect_item(int!(12));
}

#[test]
fn test_booleval_c0_exit() {
    test_case(
        "PUSHINT 1
         PUSHCONT {
             DUP
             PUSHINT 2
         }
         BOOLEVAL
         PUSHINT 3",
    )
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(1)).push(int!(2)).push(int!(-1)).push(int!(3)),
    );
}

#[test]
fn test_booleval_c1_exit() {
    test_case(
        "PUSHINT 1
         PUSHCONT {
             DUP
             PUSHINT 2
             RETFALSE
         }
         BOOLEVAL
         PUSHINT 3",
    )
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(1)).push(int!(2)).push(int!(0)).push(int!(3)),
    );
}

#[test]
fn test_booleval_c0exit_and_cc_stack_empty() {
    test_case(
        "PUSHCONT {
            PUSHINT 2
         }
         BOOLEVAL
         PUSHINT 3",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(-1)).push(int!(3)));
}

#[test]
fn test_booleval_c0exit_minimal_stack() {
    test_case(
        "PUSHCONT {
         }
         BOOLEVAL",
    )
    .expect_item(int!(-1));
}

#[test]
fn test_booleval_c1exit_minimal_stack() {
    test_case(
        "PUSHCONT {
            RETALT
         }
         BOOLEVAL",
    )
    .expect_item(int!(0));
}

#[test]
fn test_booleval_wrong_type() {
    test_case(
        "
         PUSHCONT {
             PUSHINT 2
         }
         PUSHINT 1
         BOOLEVAL",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_booleval_empty_stack() {
    test_case("BOOLEVAL").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_booleval_additional_args() {
    test_case("BOOLEVAL 1")
        .expect_compilation_failure(CompileError::too_many_params(1, 10, "BOOLEVAL"));
}

#[test]
fn test_booleval_c0_exit_from_sub() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            PUSHINT 2
            PUSHCONT {
                PUSHINT 3
                RET
            }
            BOOLEVAL
        }
        CALLX
        PUSHINT 4",
    )
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(2)).push(int!(3)).push(int!(-1)).push(int!(4)),
    );
}

mod call {
    use super::*;

    #[test]
    fn test_normal_flow_with_min_n() {
        test_case(
            "
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 100
                }
                IFNOT
            }
            POPCTR c3
            CALL 0",
        )
        .expect_item(int!(100));
    }

    #[test]
    fn test_normal_flow_with_long_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 512
                SUB
            }
            POPCTR c3
            CALL 512",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_calldict_long_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 512
                SUB
            }
            POPCTR c3
            CALLDICT 512",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_normal_flow_with_max_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 16383
                SUB
            }
            POPCTR c3
            CALL 16383",
        )
        .expect_item(int!(0));
    }
}

mod jmp {
    use super::*;

    #[test]
    fn test_normal_flow_with_min_n() {
        test_case(
            "
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 100
                }
                IFNOT
            }
            POPCTR c3
            JMP 0",
        )
        .expect_item(int!(100));
    }

    #[test]
    fn test_normal_flow_with_long_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 512
                SUB
            }
            POPCTR c3
            JMP 512",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_jmpdict_long_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 512
                SUB
            }
            POPCTR c3
            JMPDICT 512",
        )
        .expect_item(int!(0));
    }
    #[test]
    fn test_normal_flow_with_max_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 16383
                SUB
            }
            POPCTR c3
            JMP 16383",
        )
        .expect_item(int!(0));
    }
}

mod prepare {
    use super::*;

    #[test]
    fn test_normal_flow_with_min_n() {
        test_case(
            "
            PUSHCONT {
                PUSHCONT {
                    PUSHINT 100
                }
                IFNOT
            }
            POPCTR c3
            PREPARE 0
            EXECUTE",
        )
        .expect_item(int!(100));
    }

    #[test]
    fn test_normal_flow_with_long_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 512
                SUB
            }
            POPCTR c3
            JMP 512",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_normal_flow_with_max_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 16383
                SUB
            }
            POPCTR c3
            PREPARE 16383
            EXECUTE",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_preparedict_alias_normal_flow_with_max_n() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 16383
                SUB
            }
            POPCTR c3
            PREPAREDICT 16383
            EXECUTE",
        )
        .expect_item(int!(0));
    }
}

#[test]
fn test_callref_success() {
    let dup = compile_code_to_cell("DUP").unwrap();

    test_case_with_refs(
        "PUSHINT 0
        CALLREF
        INC",
        vec![dup],
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn test_callref_failure_no_reference() {
    test_case(
        "PUSHINT 0
        CALLREF
        INC",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn test_jmpref_success() {
    let ref_code = compile_code_to_cell("DUP INC INC").unwrap();

    test_case_with_refs(
        "PUSHINT 0
        JMPREF
        INC",
        vec![ref_code],
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(2)));
}

#[test]
fn test_jmpref_failure_no_reference() {
    test_case(
        "PUSHINT 0
        JMPREF
        INC",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn test_jmprefdata_success() {
    let ref_slice = compile_code_to_cell("SWAP INC INC").unwrap();
    let suffix = "INC INC INC DUP";
    let suffix_slice = compile_code_to_cell(suffix).unwrap();

    let mut test_case_body = "
    PUSHINT 0
    JMPREFDATA
    "
    .to_owned();

    test_case_body.push_str(suffix);

    test_case_with_refs(&test_case_body, vec![ref_slice]).expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::load_cell(suffix_slice).unwrap()))
            .push(int!(2)),
    );
}

#[test]
fn test_jmprefdata_failure_no_reference() {
    test_case(
        "PUSHINT 0
        JMPREFDATA
        INC",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn ifretalt_zero() {
    test_case(
        "PUSHCONT {
             PUSHINT 10
         }
         POPCTR c1
         PUSHINT 20
         PUSHINT 0
         IFRETALT",
    )
    .expect_item(int!(20));
}

#[test]
fn ifretalt_one() {
    test_case(
        "PUSHCONT {
             PUSHINT 10
         }
         POPCTR c1
         PUSHINT 1
         IFRETALT",
    )
    .expect_item(int!(10));
}

#[test]
fn ifnotretalt_zero() {
    test_case(
        "PUSHCONT {
             PUSHINT 10
         }
         POPCTR c1
         PUSHINT 0
         IFNOTRETALT",
    )
    .expect_item(int!(10));
}

#[test]
fn ifnotretalt_one() {
    test_case(
        "PUSHCONT {
             PUSHINT 10
         }
         POPCTR c1
         PUSHINT 20
         PUSHINT 1
         IFNOTRETALT",
    )
    .expect_item(int!(20));
}

#[test]
fn ifbitjmp_zero() {
    test_case(
        "
        PUSHINT 0
        PUSHCONT {
            PUSHINT 10
        }
        IFBITJMP 0",
    )
    .expect_item(int!(0));
}

#[test]
fn ifbitjmp_0s_bit_is_set() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            PUSHINT 10
        }
        IFBITJMP 0",
    )
    .expect_stack(Stack::new().push(int!(1)).push(int!(10)));
}

#[test]
fn ifbitjmp_0s_bit_is_not_set() {
    test_case(
        "
        PUSHINT 2
        PUSHCONT {
            PUSHINT 10
        }
        IFBITJMP 0",
    )
    .expect_item(int!(2));
}

#[test]
fn ifbitjmp_31st_bit_is_set() {
    test_case(
        "
        PUSHINT 2147483648
        PUSHCONT {
            PUSHINT 10
        }
        IFBITJMP 31",
    )
    .expect_stack(Stack::new().push(int!(2147483648u32)).push(int!(10)));
}

#[test]
fn ifbitjmp_31st_bit_is_not_set() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            PUSHINT 10
        }

        IFBITJMP 31",
    )
    .expect_item(int!(1));
}

#[test]
fn ifbitjmpref_zero() {
    let slice = SliceData::new(vec![0x72, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 0
        IFBITJMPREF 0
        INC
    ",
        slice,
    )
    .expect_item(int!(1));
}

#[test]
fn ifbitjmpref_0s_bit_is_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 3
        IFBITJMPREF 0
    ",
        slice,
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(1)));
}

#[test]
fn ifbitjmpref_0s_bit_is_not_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 2
        IFBITJMPREF 0
        INC
    ",
        slice,
    )
    .expect_item(int!(3));
}

#[test]
fn ifbitjmpref_31st_bit_is_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 2147483648
        IFBITJMPREF 31
    ",
        slice,
    )
    .expect_stack(Stack::new().push(int!(2147483648u32)).push(int!(1)));
}

#[test]
fn ifbitjmpref_31st_bit_is_not_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 123
        IFBITJMPREF 31
        INC
    ",
        slice,
    )
    .expect_item(int!(124));
}

#[test]
fn ifnbitjmp_0s_bit_is_set() {
    test_case(
        "
        PUSHINT 14
        PUSHCONT {
            PUSHINT 10
        }
        IFNBITJMP 0
    ",
    )
    .expect_stack(Stack::new().push(int!(14)).push(int!(10)));
}

#[test]
fn ifnbitjmpref_zero() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_refs(
        "PUSHINT 0
         IFNBITJMPREF 0",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn ifnbitjmpref_0s_bit_is_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 3
        IFNBITJMPREF 0
        INC
    ",
        slice,
    )
    .expect_item(int!(4));
}

#[test]
fn ifnbitjmpref_0s_bit_is_not_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_refs(
        "PUSHINT 2
         IFNBITJMPREF 0",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn ifnbitjmpref_31st_bit_is_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "
        PUSHINT 2147483648
        IFNBITJMPREF 31
        DEC
    ",
        slice,
    )
    .expect_stack(Stack::new().push(int!(2147483647u32)));
}

#[test]
fn ifnbitjmpref_31st_bit_is_not_set() {
    let slice = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    test_case_with_ref(
        "PUSHINT 123
        IFNBITJMPREF 31",
        slice,
    )
    .expect_stack(Stack::new().push(int!(123)).push(int!(1)));
}

mod retargs {
    use super::*;

    #[test]
    fn test_simple() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                RETARGS 2
            }
            CALLX
            ADD
        ",
        )
        .expect_int_stack(&[5]);
    }

    #[test]
    fn test_fee_for_split_and_combine_stack() {
        // destination stack is empty - pay nothing
        test_case(
            "PUSHINT 0\n".repeat(33)
                + "
            PUSHCONT {}
            SETCONTARGS 0, 0
            JMPX
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 15339)
        .expect_success();

        // Note that the following tests pass with and without CapTvmBugfixes2023
        // which includes a fix for a bug in pop_range().

        // move small amount of stack twice - pay nothing
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            SETCONTARGS 2, 7
            JMPX
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13411)
        .expect_int_stack(&[0, 1, 2, 3, 9 + 3]);

        // move small amount of stack twice - pay only for concatenation of two stacks
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            SETCONTARGS 2, 7
            CALLX
            DEPTH
            PUSHINT 7
            ONLYTOPX
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13260)
        .expect_int_stack(&[0, 0, 1, 2, 3, 9 + 3, 124]);

        // move small amount of stack - pay nothing
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            PUSHINT 17
            JMPXVARARGS
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13403)
        .expect_int_stack(&[0, 1, 2, 3, 17 + 3]);

        // move big amount of stack and left big - pay for moving
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            PUSHINT 33
            JMPXVARARGS
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13402)
        .expect_int_stack(&[0, 1, 2, 3, 33 + 3]);

        // move big amount of stack and left small - pay for moving
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            PUSHINT 120
            JMPXVARARGS
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13315)
        .expect_int_stack(&[0, 1, 2, 3, 120 + 3]);

        // move all the stack - pay nothing
        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {
                    PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            PUSHINT 128
            JMPXVARARGS
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13395)
        .expect_int_stack(&[0, 1, 2, 3, 128 + 3]);

        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {"
                + "PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            PUSHINT 40
            RETURNVARARGS
            JMPX
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13266);

        test_case(
            "PUSHINT 0\n".repeat(128)
                + "PUSHCONT {"
                + "PUSHINT 1
                    PUSHINT 2
                    PUSHINT 3
                    DEPTH
                    PUSHINT 5
                    ONLYTOPX
                }
            RETURNARGS 14
            JMPX
        ",
        )
        .with_gas_limit(16000)
        .expect_gas(1000000000, 16000, 0, 13240);
    }

    #[test]
    fn test_nargs_pargs_ok() {
        test_case(
            "
            PUSHCONT {
                DEPTH
            }
            SETNUMARGS 2
            POPCTR c0
            PUSHINT 1
            PUSHINT 2
            PUSHINT 3
            PUSHINT 4
            RETARGS 2
        ",
        )
        .expect_int_stack(&[3, 4, 2]);
    }
}

mod retvarargs {
    use super::*;

    #[test]
    fn test_simple() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                PUSHINT 2
                RETVARARGS
            }
            CALLX
            ADD
        ",
        )
        .expect_int_stack(&[5]);
    }

    #[test]
    fn test_all() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                PUSHINT -1
                RETVARARGS
            }
            CALLX
            ADD
        ",
        )
        .expect_int_stack(&[1, 5]);
    }

    #[test]
    fn test_underflow() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                PUSHINT 4
                RETVARARGS
            }
            CALLX
            ADD
        ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_overflow() {
        test_case(
            "
            PUSHCONT {
                PUSHINT 1
                PUSHINT 2
                PUSHINT 3
                PUSHINT 4
                RETVARARGS
            }
            CALLX
            ADD
        ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_nargs_pargs_all_ok() {
        test_case(
            "
            PUSHCONT {
                DEPTH
            }
            SETNUMARGS 2
            POPCTR c0
            PUSHINT 1
            PUSHINT 2
            PUSHINT -1
            RETVARARGS
        ",
        )
        .expect_int_stack(&[1, 2, 2]);
    }
}

mod root {
    use super::*;

    #[test]
    fn type_check_error_put_null() {
        test_case(
            "
            NEWDICT
            POPROOT
        ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn type_check_error_put_slice() {
        test_case(
            "
            PUSHSLICE x_
            POPROOT
        ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn check_root_simple_get() {
        test_case(
            "
            PUSHROOT
        ",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn normal_setget_cell_with_dictionary() {
        test_case(
            "
            PUSHSLICE x57
            PUSHINT 0
            PUSHROOT
            CTOS
            PLDDICTQ
            NULLSWAPIFNOT
            DROP
            PUSHINT 256
            DICTUSET
            NEWC
            STDICT
            ENDC
            POPROOT

            PUSHROOT
            CTOS
            PLDDICT
            PUSHINT 0
            SWAP
            PUSHINT 256
            DICTUGET
            THROWIFNOT 100
            PLDU 8
        ",
        )
        .expect_item(int!(0x57));
    }
}

#[test]
fn test_samealt_simple() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            PUSHINT 2
            PUSHCONT {
                PUSHINT 3
                SAMEALT
                RETALT
                PUSHINT 4
            }
            CALLX
            PUSHINT 5
            RETALT
        }
        CALLX
        PUSHINT 6
    ",
    )
    .expect_int_stack(&[1, 2, 3, 5]);
}

#[test]
fn test_samealt_save_simple() {
    test_case(
        "
        PUSHINT 1
        PUSHCONT {
            PUSHINT 2
            PUSHCONT {
                PUSHINT 3
                PUSHCONT {
                    PUSHINT 4
                }
                POPCTR c1
                SAMEALTSAVE
                RETALT
                PUSHINT 5
            }
            CALLX
            PUSHINT 6
            RETALT
        }
        CALLX
        PUSHINT 7
    ",
    )
    .expect_int_stack(&[1, 2, 3, 6, 4, 7]);
}

#[test]
fn test_pushctrx_range() {
    test_case(
        "
        PUSHINT 10
        PUSHCTRX
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_popctrx_range() {
    test_case(
        "
        PUSHCONT {
        }
        PUSHINT 10
        POPCTRX
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

mod runvm {
    use super::*;

    #[test]
    fn test_runvm_error() {
        expect_exception("RUNVM 0", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RUNVM 0", ExceptionCode::TypeCheckError);
        expect_exception(
            "PUSHSLICE x73
            RUNVM 512",
            ExceptionCode::RangeCheckError,
        );

        expect_exception("RUNVMX", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RUNVMX", ExceptionCode::StackUnderflow);
        expect_exception("PUSHSLICE x73 RUNVMX", ExceptionCode::TypeCheckError);
        expect_exception("NULL RUNVMX", ExceptionCode::TypeCheckError);
        expect_exception("ZERO PUSHSLICE x73 RUNVMX", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_runvm_success() {
        // simplest case without any additional arguments on stack
        test_case(
            "
            ZERO ; no arguments
            PUSHSLICE x73 ; PUSHINT 3
            RUNVM 0",
        )
        .expect_int_stack(&[3, 0])
        .expect_gas_used(142);

        // pass and one argument on stack
        test_case(
            "TEN
            ONE ; pass one stack value
            PUSHSLICE x73 ; PUSHINT 3
            RUNVM 0",
        )
        .expect_int_stack(&[10, 3, 0])
        .expect_gas_used(160);

        // pass gas max one argument on stack
        test_case(
            "TEN
            ONE ; pass one stack value
            PUSHSLICE x73 ; PUSHINT 3
            PUSHINT 1000; gas max
            RUNVM 64",
        )
        .expect_int_stack(&[10, 3, 0])
        .expect_gas_used(194);

        // pass gas max, limit and one argument on stack
        // get gas used
        test_case(
            "TEN
            ONE ; pass one stack value
            PUSHSLICE x73 ; PUSHINT 3
            PUSHINT 500; gas limit
            PUSHINT 1000; gas max
            RUNVM 72",
        )
        .expect_int_stack(&[10, 3, 0, 23])
        .expect_gas_used(228);

        // pass gas max, limit and one argument on stack
        // pass c7
        // get one stack item back
        // get gas used
        test_case(
            "TEN
            ONE ; pass one stack value
            PUSHSLICE x7374 ; PUSHINT 3 PUSHINT 4
            PUSHINT 1 ; one value back
            NIL ; empty c7
            PUSHINT 500; gas limit
            PUSHINT 1000; gas max
            RUNVM 344",
        )
        .expect_int_stack(&[4, 0, 41])
        .expect_gas_used(290);
    }

    #[test]
    fn test_runvm_exception() {
        // test with a real exception
        let code = compile_code_to_cell("DROP2").unwrap();
        test_case_with_ref(
            "TEN
            ONE ; pass one stack value
            PUSHREFSLICE
            RUNVM 0",
            code,
        )
        .expect_int_stack(&[0, 2]);

        // TODO: test with a custom exception
        let code = compile_code_to_cell("THROW 777").unwrap();
        test_case_with_ref(
            "TEN
            ONE ; pass one stack value
            PUSHREFSLICE
            RUNVM 0",
            code,
        )
        .expect_int_stack(&[0, 777]);
    }
}
