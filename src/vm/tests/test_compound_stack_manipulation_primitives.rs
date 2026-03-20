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
#[cfg(feature = "ci_run")]
use common::create;
use common::{test_case, test_case_with_bytecode, test_framework::Expects};
#[cfg(feature = "ci_run")]
use ton_assembler::compile_code;
use ton_assembler::CompileError;
#[cfg(feature = "ci_run")]
use ton_block::{BuilderData, Cell};
use ton_block::{ExceptionCode, SliceData};
#[cfg(feature = "ci_run")]
use ton_vm::stack::continuation::ContinuationData;
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

#[cfg(feature = "ci_run")]
fn push_code_by_type(push_type: u8, value: u8) -> String {
    match push_type {
        // int
        0 => format!("PUSHINT {} \n", value),
        // continuation
        1 => format!("PUSHCONT  {{ NOP }}\n"),
        // slice
        2 => format!("PUSHSLICE  x4_\n"),
        // cell
        3 => format!("NEWC ENDC\n"),
        // builder
        4 => format!("NEWC\n"),
        _ => unreachable!(),
    }
}

#[cfg(feature = "ci_run")]
fn stack_item_by_type(push_type: u8, value: u8) -> StackItem {
    match push_type {
        0 => int!(value),
        1 => StackItem::continuation(ContinuationData::with_code(compile_code("NOP").unwrap())),
        2 => create::slice([0x40]),
        3 => StackItem::cell(Cell::default()),
        4 => StackItem::builder(BuilderData::default()),
        _ => unreachable!(),
    }
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xchg2_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            let s_size = std::cmp::max(i, j) + 1;
            for type_1 in 0..5 {
                for type_2 in 0..5 {
                    let mut code: String = "".to_string(); // test code for execution
                    let mut stack = Stack::new(); // expected stack state
                                                  // fill stack
                    for stack_values in 0..s_size {
                        let stack_position = s_size - stack_values - 1;
                        if stack_position == i {
                            code += &push_code_by_type(type_1, stack_position);
                            stack.push(stack_item_by_type(type_1, stack_position));
                        } else {
                            if stack_position == j {
                                code += &push_code_by_type(type_2, stack_position);
                                stack.push(stack_item_by_type(type_2, stack_position));
                            } else {
                                code += &push_code_by_type(0, stack_position);
                                stack.push(stack_item_by_type(0, stack_position));
                            }
                        }
                    }
                    let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                    bytecode.push(0x50);
                    bytecode.push((i * 16) | j);
                    bytecode.push(0x80);
                    code += &("XCHG2 S".to_string() + &i.to_string() + ", S" + &j.to_string());
                    let msg = "Code:\n".to_string() + &code + "\n";
                    if s_size >= 2 {
                        // build expected stack
                        stack.swap(1, i as usize).unwrap();
                        stack.swap(0, j as usize).unwrap();
                        // test
                        test_case(&code)
                            .expect_bytecode_extended(bytecode, Some(&msg))
                            .expect_stack_extended(&stack, Some(&msg));
                    } else {
                        test_case(&code)
                            .expect_failure_extended(ExceptionCode::StackUnderflow, Some(&msg));
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xchg2_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         XCHG2 s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x50, 0x23, 0x80])
    .expect_stack(Stack::new().push(int!(4)).push(int!(3)).push(int!(2)).push(int!(1)));
}

#[test]
fn test_xchg2_command_stackunderflow() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 s2, s3",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xchg2_command_wrong_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 s21, s3",
    )
    .expect_compilation_failure(CompileError::out_of_range(4, 10, "XCHG2", "arg 0"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 s1, s33",
    )
    .expect_compilation_failure(CompileError::out_of_range(4, 10, "XCHG2", "arg 1"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 s21, s33",
    )
    .expect_compilation_failure(CompileError::out_of_range(4, 10, "XCHG2", "arg 0"));
}

#[test]
fn test_xchg2_command_no_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 ",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCHG2"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 S3",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCHG2"));
}

#[test]
fn test_xchg2_command_too_many_params() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG2 s0, s1, s2",
    )
    .expect_compilation_failure(CompileError::too_many_params(4, 24, "XCHG2"));
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xchg3_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            for k in vec![0, 1, 2, 3, 4, 5, 15] {
                let s_size = std::cmp::max(i, std::cmp::max(j, k)) + 1;
                for type_1 in 0..5 {
                    for type_2 in 0..5 {
                        for type_3 in 0..5 {
                            let mut code: String = "".to_string(); // test code for execution
                            let mut stack = Stack::new(); // expected stack state
                                                          // fill stack
                            for stack_values in 0..s_size {
                                let stack_position = s_size - stack_values - 1;
                                if stack_position == i {
                                    code += &push_code_by_type(type_1, stack_position);
                                    stack.push(stack_item_by_type(type_1, stack_position));
                                } else {
                                    if stack_position == j {
                                        code += &push_code_by_type(type_2, stack_position);
                                        stack.push(stack_item_by_type(type_2, stack_position));
                                    } else {
                                        if stack_position == k {
                                            code += &push_code_by_type(type_3, stack_position);
                                            stack.push(stack_item_by_type(type_3, stack_position));
                                        } else {
                                            code += &push_code_by_type(0, stack_position);
                                            stack.push(stack_item_by_type(0, stack_position));
                                        }
                                    }
                                }
                            }
                            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                            bytecode.push(0x40 | i);
                            bytecode.push((j * 16) | k);
                            bytecode.push(0x80);
                            code += &("XCHG3 S".to_string()
                                + &i.to_string()
                                + ", S"
                                + &j.to_string()
                                + ", S"
                                + &k.to_string())
                                .to_string();
                            let msg = "Code:\n".to_string() + &code + "\n";
                            // test
                            if s_size >= 3 {
                                // build expected stack
                                stack.swap(2, i as usize).unwrap();
                                stack.swap(1, j as usize).unwrap();
                                stack.swap(0, k as usize).unwrap();
                                test_case(&code)
                                    .expect_bytecode_extended(bytecode, Some(&msg))
                                    .expect_stack_extended(&stack, Some(&msg));
                            } else {
                                test_case(&code).expect_failure_extended(
                                    ExceptionCode::StackUnderflow,
                                    Some(&msg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xchg3_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s3, s4, s5",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x43, 0x45, 0x80])
    .expect_stack(
        Stack::new()
            .push(int!(6))
            .push(int!(5))
            .push(int!(4))
            .push(int!(3))
            .push(int!(2))
            .push(int!(1)),
    );
}

#[test]
fn test_xchg3_command_stackunderflow() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         XCHG3 s3, s4, s5",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xchg3_command_wrong_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s31, s4, s5",
    )
    .expect_compilation_failure(CompileError::out_of_range(7, 10, "XCHG3", "arg 0"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s3, s43, s5",
    )
    .expect_compilation_failure(CompileError::out_of_range(7, 10, "XCHG3", "arg 1"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s3, s4, s54",
    )
    .expect_compilation_failure(CompileError::out_of_range(7, 10, "XCHG3", "arg 2"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s31, s41, s5",
    )
    .expect_compilation_failure(CompileError::out_of_range(7, 10, "XCHG3", "arg 0"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s3, s42, s52",
    )
    .expect_compilation_failure(CompileError::out_of_range(7, 10, "XCHG3", "arg 1"));
}

#[test]
fn test_xchg3_command_too_many_params() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSHINT  5
         PUSHINT  6
         XCHG3 s0, s1, s2, s3",
    )
    .expect_compilation_failure(CompileError::too_many_params(7, 28, "XCHG3"));
}

#[test]
fn test_xchg3_command_no_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG3 ",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCHG3"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG3 S3",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCHG3"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCHG3 S0, S1",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCHG3"));
}

//Test for long form 540ijk — XCHG3 s(i),s(j),s(k)
//XCHG3 at this moment compiled to short form, but if XCHG3 in bytecode will be presented in
//long form - it must be executed successfully.
#[test]
fn test_xchg3_longform_from_bytecode() {
    //PUSHINT 1 PUSHINT 2 PUSHINT 3 PUSHINT 4 PUSHINT 5 PUSHINT 6 XCHG3 s3, s4, s5"
    let code = SliceData::new(vec![0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x54, 0x3, 0x45, 0x80])
        .into_cell()
        .unwrap();
    test_case_with_bytecode(code).expect_stack(
        Stack::new()
            .push(int!(6))
            .push(int!(5))
            .push(int!(4))
            .push(int!(3))
            .push(int!(2))
            .push(int!(1)),
    );
}

#[test]
fn test_xchg3_longform_stackunderflow() {
    //PUSHINT 1 PUSHINT 2 PUSHINT 3 XCHG3 s3, s4, s5"
    let code =
        SliceData::new(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x3, 0x45, 0x80]).into_cell().unwrap();
    test_case_with_bytecode(code).expect_failure(ExceptionCode::StackUnderflow);
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xcpu_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            let s_size = std::cmp::max(i, j) + 1;
            let mut code = String::new(); // test code for execution
            let mut stack = Stack::new(); // expected stack state
                                          // fill stack
            for stack_values in 0..s_size {
                let stack_position = s_size - stack_values - 1;
                code += &push_code_by_type(0, stack_position);
                stack.push(stack_item_by_type(0, stack_position));
            }
            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
            bytecode.push(0x51);
            bytecode.push((i * 16) | j);
            bytecode.push(0x80);
            code += &format!("XCPU S{}, S{}", i, j);
            let msg = format!("Code:\n{}\n", code);
            // build expected stack
            stack.swap(0, i as usize).unwrap();
            stack.push_copy(j as usize).unwrap();
            // test
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&msg))
                .expect_stack_extended(&stack, Some(&msg));
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xcpu_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         XCPU s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x51, 0x23, 0x80])
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(4)).push(int!(3)).push(int!(2)).push(int!(1)),
    );
}

#[test]
fn test_xcpu_command_stackunderflow() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU s2, s3",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xcpu_command_wrong_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU s5, s33",
    )
    .expect_compilation_failure(CompileError::out_of_range(4, 10, "XCPU", "arg 1"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU s51, s3",
    )
    .expect_compilation_failure(CompileError::out_of_range(4, 10, "XCPU", "arg 0"));
}

#[test]
fn test_xcpu_command_no_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU ",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCPU"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU S0",
    )
    .expect_compilation_failure(CompileError::missing_params(4, 10, "XCPU"));
}

#[test]
fn test_xcpu_command_too_many_params() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         XCPU s0, s1, s2",
    )
    .expect_compilation_failure(CompileError::too_many_params(4, 23, "XCPU"));
}

#[cfg(feature = "ci_run")]
#[test]
// 52ij - PUXC  s(i),s(j − 1), equivalent to PUSH s(i); SWAP; XCHG s(j).
fn test_puxc_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            let s_size = std::cmp::max(i, j) + 1;
            let mut code: String = "".to_string(); // test code for execution
            let mut stack = Stack::new(); // expected stack state
                                          // fill stack
            for stack_values in 0..s_size {
                let stack_position = s_size - stack_values - 1;
                code += &push_code_by_type(0, stack_position);
                stack.push(stack_item_by_type(0, stack_position));
            }
            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
            bytecode.push(0x52);
            bytecode.push((i * 16) | j);
            bytecode.push(0x80);
            code += &format!("PUXC S{}, S{}", i, j as i8 - 1);
            let msg = format!("Code:\n{}\n", code);
            // build expected stack
            stack.push_copy(i as usize).unwrap();
            stack.swap(0, 1).unwrap();
            stack.swap(0, j as usize).unwrap();
            // test
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&msg))
                .expect_stack_extended(&stack, Some(&msg));
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_puxc_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUXC s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x52, 0x24, 0x80])
    .expect_stack(
        Stack::new().push(int!(4)).push(int!(2)).push(int!(3)).push(int!(2)).push(int!(1)),
    );

    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUXC s2, s-1",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x52, 0x20, 0x80])
    .expect_int_stack(&[1, 2, 3, 2, 4]);
}

#[test]
fn test_puxc_operation_fails_on_first_operand_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUXC s2, s0",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_puxc_operation_fails_on_second_operand_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUXC s0, s2",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_push2_fails_on_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSH2 s2, s0",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_push2_fails_on_empty_stack() {
    test_case("PUSH2 s0, s0").expect_failure(ExceptionCode::StackUnderflow);
}

#[cfg(feature = "ci_run")]
#[test]
fn test_push2_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            let s_size = std::cmp::max(i, j) + 1;
            for type_1 in 0..5 {
                for type_2 in 0..5 {
                    let mut code: String = "".to_string(); // test code for execution
                    let mut stack = Stack::new(); // expected stack state
                                                  // fill stack
                    for stack_values in 0..s_size {
                        let stack_position = s_size - stack_values - 1;
                        if stack_position == i {
                            code += &push_code_by_type(type_1, stack_position);
                            stack.push(stack_item_by_type(type_1, stack_position));
                        } else {
                            if stack_position == j {
                                code += &push_code_by_type(type_2, stack_position);
                                stack.push(stack_item_by_type(type_2, stack_position));
                            } else {
                                code += &push_code_by_type(0, stack_position);
                                stack.push(stack_item_by_type(0, stack_position));
                            }
                        }
                    }
                    let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                    bytecode.push(0x53);
                    bytecode.push((i * 16) | j);
                    bytecode.push(0x80);
                    code += &("PUSH2 S".to_string() + &i.to_string() + ", S" + &j.to_string());
                    let msg = "Code:\n".to_string() + &code + "\n";
                    if s_size >= 1 && i < s_size && j <= s_size {
                        // build expected stack
                        stack.push_copy(i as usize).unwrap();
                        stack.push_copy((j + 1) as usize).unwrap();
                        // test
                        test_case(&code)
                            .expect_bytecode_extended(bytecode, Some(&msg))
                            .expect_stack_extended(&stack, Some(&msg));
                    } else {
                        test_case(&code).expect_failure(ExceptionCode::StackUnderflow);
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_push2_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSH2 s0, s1",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x53, 0x01, 0x80])
    .expect_stack(Stack::new().push(int!(1)).push(int!(2)).push(int!(2)).push(int!(1)));
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xc2pu_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            for k in vec![0, 1, 2, 3, 4, 5, 15] {
                let s_size = std::cmp::max(i, std::cmp::max(j, k)) + 1;
                for type_1 in 0..5 {
                    for type_2 in 0..5 {
                        for type_3 in 0..5 {
                            let mut code: String = "".to_string(); // test code for execution
                            let mut stack = Stack::new(); // expected stack state
                                                          // fill stack
                            for stack_values in 0..s_size {
                                let stack_position = s_size - stack_values - 1;
                                if stack_position == i {
                                    code += &push_code_by_type(type_1, stack_position);
                                    stack.push(stack_item_by_type(type_1, stack_position));
                                } else {
                                    if stack_position == j {
                                        code += &push_code_by_type(type_2, stack_position);
                                        stack.push(stack_item_by_type(type_2, stack_position));
                                    } else {
                                        if stack_position == k {
                                            code += &push_code_by_type(type_3, stack_position);
                                            stack.push(stack_item_by_type(type_3, stack_position));
                                        } else {
                                            code += &push_code_by_type(0, stack_position);
                                            stack.push(stack_item_by_type(0, stack_position));
                                        }
                                    }
                                }
                            }
                            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                            bytecode.push(0x54);
                            bytecode.push(0x10 | i);
                            bytecode.push((j * 16) | k);
                            bytecode.push(0x80);
                            code += &("XC2PU S".to_string()
                                + &i.to_string()
                                + ", S"
                                + &j.to_string()
                                + ", S"
                                + &k.to_string())
                                .to_string();
                            let msg = "Code:\n".to_string() + &code + "\n";
                            if s_size >= 2 {
                                // build expected stack
                                stack.swap(1, i as usize).unwrap();
                                stack.swap(0, j as usize).unwrap();
                                stack.push_copy(k as usize).unwrap();
                                // test
                                test_case(&code)
                                    .expect_bytecode_extended(bytecode, Some(&msg))
                                    .expect_stack_extended(&stack, Some(&msg));
                            } else {
                                test_case(&code).expect_failure_extended(
                                    ExceptionCode::StackUnderflow,
                                    Some(&msg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xc2pu_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         XC2PU s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x11, 0x23, 0x80])
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(4)).push(int!(3)).push(int!(2)).push(int!(1)),
    );
}

#[cfg(feature = "ci_run")]
#[test]
// 542ijk - XCPUXC s(i),s(j),s(k−1), equivalent to XCHG s1,s(i); PUSH s(j); SWAP; XCHG s(k-1).
fn test_xcpuxc_bulk() {
    for i in vec![0, 1, 2, 4, 5, 15] {
        for j in vec![0, 1, 2, 4, 5, 15] {
            for k in vec![0, 1, 2, 4, 5, 15] {
                let size = std::cmp::max(i, std::cmp::max(j, k)) + 2;
                let mut code = String::new(); // test code for execution
                let mut stack = Stack::new(); // expected stack state
                                              // fill stack
                for i in 0..size {
                    let stack_position = size - i - 1;
                    code += &push_code_by_type(0, stack_position);
                    stack.push(stack_item_by_type(0, stack_position));
                }
                let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                bytecode.push(0x54);
                bytecode.push(0x20 | i);
                bytecode.push((j * 16) | k);
                bytecode.push(0x80);
                // build expected stack
                stack.swap(1, i as usize).unwrap();
                stack.push_copy(j as usize).unwrap();
                stack.swap(0, 1).unwrap();
                stack.swap(0, k as usize).unwrap();
                code += &format!("XCPUXC S{}, S{}, S{}", i, j, k as i8 - 1);
                let msg = format!("i:{}, j:{}, k:{}\n Code:\n{}\n", i, j, k, code);
                // test
                test_case(&code)
                    .expect_bytecode_extended(bytecode, Some(&msg))
                    .expect_stack_extended(&stack, Some(&msg));
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xcpuxc_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         XC2PU s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x11, 0x23, 0x80])
    .expect_stack(
        Stack::new().push(int!(1)).push(int!(4)).push(int!(3)).push(int!(2)).push(int!(1)),
    );
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xcpu2_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            for k in vec![0, 1, 2, 3, 4, 5, 15] {
                let s_size = std::cmp::max(i, std::cmp::max(j, k)) + 1;
                for type_1 in 0..5 {
                    for type_2 in 0..5 {
                        for type_3 in 0..5 {
                            let mut code: String = "".to_string(); // test code for execution
                            let mut stack = Stack::new(); // expected stack state
                                                          // fill stack
                            for stack_values in 0..s_size {
                                let stack_position = s_size - stack_values - 1;
                                if stack_position == i {
                                    code += &push_code_by_type(type_1, stack_position);
                                    stack.push(stack_item_by_type(type_1, stack_position));
                                } else {
                                    if stack_position == j {
                                        code += &push_code_by_type(type_2, stack_position);
                                        stack.push(stack_item_by_type(type_2, stack_position));
                                    } else {
                                        if stack_position == k {
                                            code += &push_code_by_type(type_3, stack_position);
                                            stack.push(stack_item_by_type(type_3, stack_position));
                                        } else {
                                            code += &push_code_by_type(0, stack_position);
                                            stack.push(stack_item_by_type(0, stack_position));
                                        }
                                    }
                                }
                            }
                            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                            bytecode.push(0x54);
                            bytecode.push(0x30 | i);
                            bytecode.push((j * 16) | k);
                            bytecode.push(0x80);
                            code += &format!("XCPU2 S{}, S{}, S{}", i, j, k);
                            let msg = format!("Code:\n{}\n", code);
                            // build expected stack
                            stack.swap(0, i as usize).unwrap();
                            stack.push_copy(j as usize).unwrap();
                            stack.push_copy((k + 1) as usize).unwrap();
                            // test
                            if s_size > 1 {
                                test_case(&code)
                                    .expect_bytecode_extended(bytecode, Some(&msg))
                                    .expect_stack_extended(&stack, Some(&msg));
                            } else {
                                test_case(&code).expect_failure_extended(
                                    ExceptionCode::StackUnderflow,
                                    Some(&msg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_xcpu2_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         XCPU2 s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x31, 0x23, 0x80])
    .expect_stack(
        Stack::new()
            .push(int!(1))
            .push(int!(2))
            .push(int!(4))
            .push(int!(3))
            .push(int!(2))
            .push(int!(1)),
    );
}

#[test]
fn test_xcpu2_command_stackunderflow() {
    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s2, s1, s1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, s2, s1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, s1, s2",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xcpu2_command_wrong_parameters() {
    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 c1, s1, s1",
    )
    .expect_compilation_failure(CompileError::unexpected_type(3, 10, "XCPU2", "arg 0"));

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s16, s1, s1",
    )
    .expect_compilation_failure(CompileError::out_of_range(3, 10, "XCPU2", "arg 0"));

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, c1, s1",
    )
    .expect_compilation_failure(CompileError::unexpected_type(3, 10, "XCPU2", "arg 1"));

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, s16, s1",
    )
    .expect_compilation_failure(CompileError::out_of_range(3, 10, "XCPU2", "arg 1"));

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, s1, c1",
    )
    .expect_compilation_failure(CompileError::unexpected_type(3, 10, "XCPU2", "arg 2"));

    test_case(
        "PUSHINT  0
         PUSHINT  1
         XCPU2 s1, s1, s16",
    )
    .expect_compilation_failure(CompileError::out_of_range(3, 10, "XCPU2", "arg 2"));
}

#[test]
fn test_xcpu2_command_no_parameters() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         XCPU2 s1",
    )
    .expect_compilation_failure(CompileError::missing_params(3, 10, "XCPU2"));

    test_case(
        "PUSHINT  1
         PUSHINT  2
         XCPU2 s1, s2",
    )
    .expect_compilation_failure(CompileError::missing_params(3, 10, "XCPU2"));
}

#[test]
fn test_xcpu2_command_too_many_params() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         XCPU2 s0, s1, s1, s3",
    )
    .expect_compilation_failure(CompileError::too_many_params(3, 28, "XCPU2"));
}

#[test]
fn test_puxc2_s15() {
    test_case("PUXC2 s15, s0, s15")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "PUXC2", "arg 2"));
}

#[test]
fn test_puxc2_operation_fails_on_first_operand_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUXC2 s2, s0, s0",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_puxc2_operation_fails_on_second_operand_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUXC2 s0, s2, s0",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_puxc2_operation_fails_on_third_operand_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUXC2 s0, s0, s2",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[cfg(feature = "ci_run")]
#[test]
// 544ijk — PUXC2 s(i),s(j − 1),s(k − 1), equivalent to PUSH s(i); XCHG s2; XCHG2 s(j),s(k).
fn test_puxc2_bulk() {
    for i in vec![0, 1, 2, 4, 5, 15] {
        for j in vec![0, 1, 2, 4, 5, 15] {
            for k in vec![0, 1, 2, 4, 5, 15] {
                let size = std::cmp::max(i, std::cmp::max(j, k)) + 2;
                let mut code = String::new(); // test code for execution
                let mut stack = Stack::new(); // expected stack state
                                              // fill stack
                for i in 0..size {
                    let stack_position = size - i - 1;
                    code += &push_code_by_type(0, stack_position);
                    stack.push(stack_item_by_type(0, stack_position));
                }
                let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                bytecode.push(0x54);
                bytecode.push(0x40 | i);
                bytecode.push((j * 16) | k);
                bytecode.push(0x80);
                // build expected stack
                stack.push_copy(i as usize).unwrap();
                stack.swap(0, 2).unwrap();
                stack.swap(1, j as usize).unwrap();
                stack.swap(0, k as usize).unwrap();
                code += &format!("PUXC2 S{}, S{}, S{}", i, j as i8 - 1, k as i8 - 1);
                let msg = format!("i:{}, j:{}, k: {}\n Code:\n{}\n", i, j, k, code);
                // test
                test_case(&code)
                    .expect_bytecode_extended(bytecode, Some(&msg))
                    .expect_stack_extended(&stack, Some(&msg));
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_puxc2_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUXC2 s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x41, 0x34, 0x80])
    .expect_stack(
        Stack::new().push(int!(3)).push(int!(4)).push(int!(3)).push(int!(2)).push(int!(1)),
    );
}

#[cfg(feature = "ci_run")]
#[test]
// 545ijk - PUXCPU s(i),s(j−1),s(k−1), equivalent to PUSH s(i); SWAP; XCHG s(j - 1); PUSH s(k).
fn test_puxcpu_bulk() {
    for i in vec![0, 1, 2, 4, 5, 15] {
        for j in vec![0, 1, 2, 4, 5, 15] {
            for k in vec![0, 1, 2, 4, 5, 15] {
                let size = std::cmp::max(i, std::cmp::max(j, k)) + 2;
                let mut code = String::new(); // test code for execution
                let mut stack = Stack::new(); // expected stack state
                                              // fill stack
                for i in 0..size {
                    let stack_position = size - i - 1;
                    code += &push_code_by_type(0, stack_position);
                    stack.push(stack_item_by_type(0, stack_position));
                }
                let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                bytecode.push(0x54);
                bytecode.push(0x50 | i);
                bytecode.push((j * 16) | k);
                bytecode.push(0x80);
                // build expected stack
                stack.push_copy(i as usize).unwrap();
                stack.swap(0, 1).unwrap();
                stack.swap(0, j as usize).unwrap();
                stack.push_copy(k as usize).unwrap();
                // test
                code += &format!("PUXCPU S{}, S{}, S{}", i, j as i8 - 1, k as i8 - 1);
                let msg = format!("i:{}, j:{}, k: {}\n Code:\n{}\n", i, j, k, code);
                test_case(&code)
                    .expect_bytecode_extended(bytecode, Some(&msg))
                    .expect_stack_extended(&stack, Some(&msg));
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_puxcpu_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUXCPU s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x51, 0x34, 0x80])
    .expect_stack(
        Stack::new()
            .push(int!(1))
            .push(int!(4))
            .push(int!(3))
            .push(int!(3))
            .push(int!(2))
            .push(int!(1)),
    );
}

#[cfg(feature = "ci_run")]
#[test]
// 546ijk - PU2XC s(i),s(j−1),s(k−2), equivalent to PUSH s(i); SWAP; PUXC s(j), s(k − 1).
fn test_pu2xc_bulk() {
    for i in vec![0, 1, 2, 4, 5, 15] {
        for j in vec![0, 1, 2, 4, 5, 15] {
            for k in vec![0, 1, 2, 4, 5, 15] {
                let size = std::cmp::max(i, std::cmp::max(j, k)) + 2;
                let mut code = String::new(); // test code for execution
                let mut stack = Stack::new(); // expected stack state
                                              // fill stack
                for i in 0..size {
                    let stack_position = size - i - 1;
                    code += &push_code_by_type(0, stack_position);
                    stack.push(stack_item_by_type(0, stack_position));
                }
                let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                bytecode.push(0x54);
                bytecode.push(0x60 | i);
                bytecode.push((j * 16) | k);
                bytecode.push(0x80);
                // build expected stack
                stack.push_copy(i as usize).unwrap();
                stack.swap(0, 1).unwrap();
                stack.push_copy(j as usize).unwrap();
                stack.swap(0, 1).unwrap();
                stack.swap(0, k as usize).unwrap();
                // test
                code += &format!("PU2XC S{}, S{}, S{}", i, j as i8 - 1, k as i8 - 2);
                let msg = format!("i:{}, j:{}, k: {}\n Code:\n{}\n", i, j, k, code);
                test_case(&code)
                    .expect_bytecode_extended(bytecode, Some(&msg))
                    .expect_stack_extended(&stack, Some(&msg));
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_pu2xc_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PU2XC s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x61, 0x35, 0x80])
    .expect_int_stack(&[4, 2, 3, 3, 2, 1]);
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PU2XC s3, s3, s3",
    )
    .expect_int_stack(&[4, 2, 3, 1, 1, 1]);
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PU2XC s1, s2, s-1",
    )
    .expect_int_stack(&[1, 2, 3, 3, 4, 2]);
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PU2XC s0, s-1, s-2",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x60, 0x00, 0x80])
    .expect_int_stack(&[1, 2, 3, 4, 4, 4]);
}

#[cfg(feature = "ci_run")]
#[test]
fn test_push3_bulk() {
    for i in vec![0, 1, 2, 3, 4, 5, 15] {
        for j in vec![0, 1, 2, 3, 4, 5, 15] {
            for k in vec![0, 1, 2, 3, 4, 5, 15] {
                let s_size = std::cmp::max(i, std::cmp::max(j, k)) + 1;
                for type_1 in 0..2 {
                    for type_2 in 2..4 {
                        for type_3 in 3..5 {
                            let mut code: String = "".to_string(); // test code for execution
                            let mut stack = Stack::new(); // expected stack state
                                                          // fill stack
                            for stack_values in 0..s_size {
                                let stack_position = s_size - stack_values - 1;
                                if stack_position == i {
                                    code += &push_code_by_type(type_1, stack_position);
                                    stack.push(stack_item_by_type(type_1, stack_position));
                                } else {
                                    if stack_position == j {
                                        code += &push_code_by_type(type_2, stack_position);
                                        stack.push(stack_item_by_type(type_2, stack_position));
                                    } else {
                                        if stack_position == k {
                                            code += &push_code_by_type(type_3, stack_position);
                                            stack.push(stack_item_by_type(type_3, stack_position));
                                        } else {
                                            code += &push_code_by_type(0, stack_position);
                                            stack.push(stack_item_by_type(0, stack_position));
                                        }
                                    }
                                }
                            }
                            let mut bytecode = compile_code(&code).unwrap().get_bytestring(0);
                            bytecode.push(0x54);
                            bytecode.push(0x70 | i);
                            bytecode.push((j * 16) | k);
                            bytecode.push(0x80);
                            code += &format!("PUSH3 S{}, S{}, S{}", i, j, k);
                            let msg = format!("Code:\n{}\n", code);
                            // build expected stack
                            stack.push_copy(i as usize).unwrap();
                            stack.push_copy((j + 1) as usize).unwrap();
                            stack.push_copy((k + 2) as usize).unwrap();
                            // test
                            if s_size >= 1 {
                                test_case(&code)
                                    .expect_bytecode_extended(bytecode, Some(&msg))
                                    .expect_stack_extended(&stack, Some(&msg));
                            } else {
                                test_case(&code).expect_failure_extended(
                                    ExceptionCode::StackUnderflow,
                                    Some(&msg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "ci_run"))]
#[test]
fn test_push3_command() {
    test_case(
        "PUSHINT  1
         PUSHINT  2
         PUSHINT  3
         PUSHINT  4
         PUSH3 s1, s2, s3",
    )
    .expect_bytecode(vec![0x71, 0x72, 0x73, 0x74, 0x54, 0x71, 0x23, 0x80])
    .expect_stack(
        Stack::new()
            .push(int!(1))
            .push(int!(2))
            .push(int!(3))
            .push(int!(4))
            .push(int!(3))
            .push(int!(2))
            .push(int!(1)),
    );
}

#[test]
fn test_push3_fails_on_stack_underflow() {
    test_case(
        "PUSHINT 1
         PUSHINT 2
         PUSH3 s2, s0, s0",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_push3_on_empty_stack() {
    test_case("PUSH3 s0, s0, s0").expect_failure(ExceptionCode::StackUnderflow);
}
