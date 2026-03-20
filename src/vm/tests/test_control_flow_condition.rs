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
use ton_assembler::compile_code_to_cell;
use ton_block::{ExceptionCode, SliceData};
use ton_vm::{
    int,
    stack::{continuation::ContinuationData, integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn execute_continuation_if_non_zero() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        PUSHCONT {
            PUSHINT 1
        }
        IF",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn execute_continuation_if_non_zero_cant() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
        }
        IF",
    )
    .expect_stack(Stack::new().push(int!(2)));
}

#[test]
fn execute_continuation_if_non_zero_err_type_check() {
    test_case(
        "PUSHINT 2
        PUSHSLICE x8_
        PUSHCONT {
            PUSHINT 1
        }
        IF",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn execute_continuation_if_non_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 2
        PUSHSLICE x8_
        IF",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn execute_continuation_if_zero() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
        }
        IFNOT",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn execute_continuation_if_zero_cant() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        PUSHCONT {
            PUSHINT 1
        }
        IFNOT",
    )
    .expect_stack(Stack::new().push(int!(2)));
}

#[test]
fn execute_continuation_if_zero_err_type_check() {
    test_case(
        "PUSHINT 0
        PUSHSLICE x8_
        PUSHCONT {
            PUSHINT 1
        }
        IFNOT",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn execute_continuation_if_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 0
        PUSHSLICE x8_
        IFNOT",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

mod ifelse {
    use super::*;

    #[test]
    fn execute_continuation_if_else_non_zero() {
        test_case(
            "
            PUSHINT 2
            PUSHINT 1
            PUSHCONT {
                PUSHINT 1
            }
            PUSHCONT {
                PUSHINT 3
            }
            IFELSE",
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    }

    #[test]
    fn execute_continuation2_if_else_non_zero() {
        test_case(
            "
            PUSHINT 2
            PUSHINT 0
            PUSHCONT {
                PUSHINT 1
            }
            PUSHCONT {
                PUSHINT 3
            }
            IFELSE",
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            PUSHCONT {
                PUSHINT 3
            }
            IFELSE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check_cont() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 3
            }
            IFELSE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation2_if_else_non_zero_err_type_check_cont() {
        test_case(
            "
            PUSHINT 0
            PUSHCONT {
                PUSHINT 3
            }
            PUSHSLICE x8_
            IFELSE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation2_if_else_non_zero_err_type_check() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            PUSHCONT {
                PUSHINT 3
            }
            IFELSE
        ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod ifelseref {
    use super::*;

    #[test]
    fn execute_continuation_if_else_non_zero() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHINT 1
            PUSHCONT {
                PUSHINT 1
            }
            IFELSEREF",
            cont,
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    }

    #[test]
    fn execute_continuation2_if_else_non_zero() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHINT 0
            PUSHCONT {
                PUSHINT 1
            }
            IFELSEREF",
            cont,
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_invalid_opcode() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFELSEREF",
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            IFELSEREF",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check_cont() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFELSEREF",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation2_if_else_non_zero_err_type_check() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            PUSHCONT {
                PUSHINT 3
            }
            IFELSEREF
        ",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod ifrefelse {
    use super::*;

    #[test]
    fn execute_continuation_if_else_non_zero() {
        let cont = compile_code_to_cell("PUSHINT 1").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHINT 1
            PUSHCONT {
                PUSHINT 3
            }
            IFREFELSE",
            cont,
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    }

    #[test]
    fn execute_continuation2_if_else_non_zero() {
        let cont = compile_code_to_cell("PUSHINT 1").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHINT 0
            PUSHCONT {
                PUSHINT 3
            }
            IFREFELSE",
            cont,
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_invalid_opcode() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFREFELSE",
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check() {
        let cont = compile_code_to_cell("PUSHINT 1").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            IFREFELSE",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check_cont() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFREFELSE",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn execute_continuation2_if_else_non_zero_err_type_check() {
        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            PUSHCONT {
                PUSHINT 1
            }
            IFREFELSE",
            cont,
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod ifrefelseref {
    use super::*;

    #[test]
    fn execute_continuation_if_else_non_zero() {
        let cont1 = compile_code_to_cell("PUSHINT 1").unwrap();
        let cont2 = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_refs(
            "
            PUSHINT 2
            PUSHINT 1
            IFREFELSEREF",
            vec![cont1, cont2],
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
    }

    #[test]
    fn execute_continuation2_if_else_non_zero() {
        let cont1 = compile_code_to_cell("PUSHINT 1").unwrap();
        let cont2 = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_refs(
            "
            PUSHINT 2
            PUSHINT 0
            IFREFELSEREF",
            vec![cont1, cont2],
        )
        .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_invalid_opcode() {
        test_case(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFREFELSEREF",
        )
        .expect_failure(ExceptionCode::InvalidOpcode);

        let cont = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_ref(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFREFELSEREF",
            cont,
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn execute_continuation_if_else_non_zero_err_type_check() {
        let cont1 = compile_code_to_cell("PUSHINT 1").unwrap();
        let cont2 = compile_code_to_cell("PUSHINT 3").unwrap();
        test_case_with_refs(
            "
            PUSHINT 2
            PUSHSLICE x8_
            IFREFELSEREF",
            vec![cont1, cont2],
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

#[test]
fn switch_to_continuation_from_register_if_non_zero() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHINT 1
        IFRET",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_to_continuation_from_register_if_non_zero_cant() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHINT 0
        IFRET
        PUSHCONT {
            PUSHINT 2
        }
        POPCTR c0",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(2)));
}

#[test]
fn switch_to_continuation_from_register_if_non_zero_err_type_check() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHSLICE x8_
        IFRET",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[ignore] // c0, c1 may be inited
#[test]
fn switch_to_continuation_from_register_if_non_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 1
        IFRET",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_to_continuation_from_register_if_zero() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHINT 0
        IFNOTRET",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_to_continuation_from_register_if_zero_cant() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHINT 1
        IFNOTRET
        PUSHCONT {
            PUSHINT 2
        }
        POPCTR c0",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(2)));
}

#[test]
fn switch_to_continuation_from_register_if_zero_err_type_check() {
    test_case(
        "PUSHINT 2
        PUSHCONT {
            PUSHINT 1
        }
        POPCTR c0
        PUSHSLICE x8_
        IFNOTRET",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[ignore] // c0, c1 may be inited
#[test]
fn switch_to_continuation_from_register_if_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 0
        IFNOTRET",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_to_continuation_if_non_zero_yes() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        PUSHCONT {
            PUSHINT 3
        }
        IFJMP",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
}

#[test]
fn switch_to_continuation_if_zero() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
        }
        IFNOTJMP",
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_to_continuation_if_non_zero_no() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        PUSHCONT {
            PUSHINT 1
        }
        IFJMP",
    )
    .expect_stack(Stack::new().push(int!(2)));
}

#[test]
fn switch_to_continuation_if_non_zero_err_type_check() {
    test_case(
        "PUSHINT 2
        PUSHSLICE x8_
        PUSHCONT {
            PUSHINT 1
        }
        IFJMP",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_to_continuation_if_zero_cant() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        PUSHCONT {
            PUSHINT 1
        }
        IFNOTJMP",
    )
    .expect_stack(Stack::new().push(int!(2)));
}

#[test]
fn switch_to_continuation_if_non_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 2
        PUSHSLICE x8_
        IFJMP",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_to_continuation_if_zero_err_type_check() {
    test_case(
        "PUSHINT 0
        PUSHSLICE x8_
        PUSHCONT {
            PUSHINT 1
        }
        IFNOTJMP",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_to_continuation_if_zero_err_type_check_cont() {
    test_case(
        "PUSHINT 0
        PUSHSLICE x8_
        IFNOTJMP",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn push_cont_from_first_ref_cc_if_non_zero_simple() {
    let compile_data = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_ref(
        "PUSHINT 2
        PUSHINT 1
        IFREF",
        compile_data,
    )
    .expect_int_stack(&[2, 1]);
}

#[test]
fn push_cont_from_first_ref_cc_if_non_zero_cant() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_ref(
        "PUSHINT 2
        PUSHINT 0
        IFREF",
        slice,
    )
    .expect_item(int!(2));
}

#[test]
fn push_cont_from_first_ref_cc_if_non_zero_err_type_check() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_ref(
        "PUSHINT 2
        PUSHSLICE x8_
        IFREF",
        slice,
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn push_cont_from_first_ref_cc_if_non_zero_err_cell_underflow() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        IFREF",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);

    test_case(
        "PUSHINT 2
        PUSHINT 0
        IFREF",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn push_cont_from_first_ref_cc_if_zero() {
    let compile_data = compile_code_to_cell("PUSHINT 1").unwrap();
    let slice = compile_data;
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 0
        IFNOTREF",
        vec![slice],
    )
    .expect_int_stack(&[2, 1]);
}

#[test]
fn push_cont_from_first_ref_cc_if_zero_cant() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 1
        IFNOTREF",
        vec![slice],
    )
    .expect_item(int!(2));
}

#[test]
fn push_cont_from_first_ref_cc_if_zero_err_type_check() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHSLICE x8_
        IFNOTREF",
        vec![slice],
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn push_cont_from_first_ref_cc_if_zero_err_cell_underflow() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        IFNOTREF",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);

    test_case(
        "PUSHINT 2
        PUSHINT 1
        IFNOTREF",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn return_value_if_condition_non_zero() {
    test_case(
        "PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        CONDSEL",
    )
    .expect_item(int!(2));

    test_case(
        "PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_item(int!(2));
}

#[test]
fn return_value2_if_condition_zero() {
    test_case(
        "PUSHINT 0
        PUSHINT 2
        PUSHINT 3
        CONDSEL",
    )
    .expect_item(int!(3));

    test_case(
        "PUSHINT 0
        PUSHINT 2
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_item(int!(3));
}

#[test]
fn return_value_builder_if_condition_non_zero() {
    test_case(
        "PUSHINT 1
        NEWC
        PUSHINT 3
        CONDSEL",
    )
    .expect_item(create::builder([0x80]));

    test_case(
        "PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_item(int!(2));
}

#[test]
fn return_value2_cont_if_condition_zero() {
    test_case(
        "PUSHINT 0
        PUSHINT 2
        PUSHCONT {}
        CONDSEL",
    )
    .expect_item(StackItem::continuation(ContinuationData::new_empty()));

    test_case(
        "PUSHINT 0
        PUSHCONT {}
        PUSHCONT {}
        CONDSELCHK",
    )
    .expect_item(StackItem::continuation(ContinuationData::new_empty()));
}

#[test]
fn return_value_cell_if_condition_non_zero() {
    test_case(
        "PUSHINT 1
        NEWC
        ENDC
        PUSHINT 3
        CONDSEL",
    )
    .expect_item(create::cell([0x80]));

    test_case(
        "PUSHINT 1
        NEWC
        ENDC
        NEWC
        ENDC
        CONDSELCHK",
    )
    .expect_item(create::cell([0x80]));
}

#[test]
fn return_value2_slice_if_condition_zero() {
    test_case(
        "PUSHINT 0
        PUSHINT 2
        NEWC
        ENDC
        CTOS
        CONDSEL",
    )
    .expect_item(StackItem::Slice(SliceData::new_empty()));

    test_case(
        "PUSHINT 0
        NEWC
        ENDC
        CTOS
        NEWC
        ENDC
        CTOS
        CONDSELCHK",
    )
    .expect_item(StackItem::Slice(SliceData::new_empty()));
}

#[test]
fn return_value_condition_err_type_check() {
    test_case(
        "PUSHSLICE x8_
        PUSHINT 2
        PUSHINT 3
        CONDSEL",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "PUSHSLICE x8_
        PUSHINT 2
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn return_value_arg_err_type_check() {
    test_case(
        "PUSHINT 0
        PUSHSLICE x8_
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "PUSHINT 0
        PUSHINT 3
        PUSHSLICE x8_
        CONDSELCHK",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn condsel_stack_underflow() {
    test_case(
        "
        PUSHINT 2
        PUSHINT 3
        CONDSEL",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHINT 2
        PUSHINT 3
        CONDSELCHK",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn switch_cont_from_first_ref_cc_if_non_zero() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 1
        IFJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_cont_from_first_ref_cc_if_non_zero_cant() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 0
        IFJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
}

#[test]
fn switch_cont_from_first_ref_cc_if_non_zero_err_type_check() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHSLICE x8_
        IFJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_cont_from_first_ref_cc_if_non_zero_err_cell_underflow() {
    test_case(
        "PUSHINT 2
        PUSHINT 1
        IFJMPREF
        PUSHINT 3",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);

    test_case(
        "PUSHINT 2
        PUSHINT 0
		IFJMPREF
        PUSHINT 3",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn switch_cont_from_first_ref_cc_if_zero() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 0
		IFNOTJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(1)));
}

#[test]
fn switch_cont_from_first_ref_cc_if_zero_cant() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHINT 1
		IFNOTJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_stack(Stack::new().push(int!(2)).push(int!(3)));
}

#[test]
fn switch_cont_from_first_ref_cc_if_zero_err_type_check() {
    let slice = compile_code_to_cell("PUSHINT 1").unwrap();
    test_case_with_refs(
        "PUSHINT 2
        PUSHSLICE x8_
		IFNOTJMPREF
        PUSHINT 3",
        vec![slice],
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn switch_cont_from_first_ref_cc_if_zero_err_cell_underflow() {
    test_case(
        "PUSHINT 2
        PUSHINT 0
        IFNOTJMPREF
        PUSHINT 3",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);

    test_case(
        "PUSHINT 2
        PUSHINT 1
		IFNOTJMPREF
        PUSHINT 3",
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}
