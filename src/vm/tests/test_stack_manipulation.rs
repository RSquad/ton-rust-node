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
use ton_assembler::{compile_code, CompileError};
use ton_block::ExceptionCode;
use ton_vm::{
    int,
    stack::{continuation::ContinuationData, integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn test_pop_operation_fails_on_empty_stack() {
    test_case("POP s0").expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_pop_operation_drops_top_of_stack_on_s0() {
    test_case(
        "PUSHINT  1
         POP     s0",
    )
    .expect_stack(&Stack::new());
}

#[test]
fn test_pop_operation_fails_on_stack_underflow() {
    test_case(
        "PUSHINT  1
         POP     s1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_pop_operation_moves_top_to_i() {
    test_case(
        "PUSHINT  2
         PUSHINT  2
         PUSHINT -30000
         POP     s2",
    )
    .expect_int_stack(&[-30000, 2]);
}

#[test]
fn test_nop() {
    test_case("NOP").expect_stack(&Stack::new());
}

#[test]
fn test_nop_bytecode() {
    let exp_vec: Vec<u8> = vec![0x00, 0x80];
    test_case("NOP").expect_bytecode(exp_vec);
}

#[allow(overflowing_literals)]
#[test]
fn test_nop_command_too_many_params() {
    test_case("NOP s0").expect_compilation_failure(CompileError::too_many_params(1, 5, "NOP"));
}

#[test]
fn test_xchg0_command() {
    test_case("XCHG S0")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "XCHG", "arg 0"));
}

#[test]
fn test_xchg_command() {
    test_case("XCHG S2, S1").expect_compilation_failure(CompileError::logic_error(
        1,
        1,
        "XCHG",
        "arg 1 should be greater than arg 0",
    ));
}

#[test]
fn test_xchg_s0_compilation_failure() {
    test_case("XCHG S0")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "XCHG", "arg 0"));
}

#[test]
fn test_xchg_s16_compilation_failure() {
    test_case("XCHG S16")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "XCHG", "arg 0"));
}

#[test]
fn test_xchg_si_only() {
    for arg in 1..16 {
        let s_size = arg + 1; // stack size for successfull execution
        for arg_type in 1..5 {
            let mut code: String = "".to_string(); // test code for execution
            let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
            let mut stack = Stack::new(); // expected stack state
            for push_value in 0..s_size {
                let stack_position = s_size - push_value - 1;
                // build code
                if stack_position == arg {
                    match arg_type {
                        0 => {
                            // int
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if stack_position <= 10 {
                                bytecode.push(0x70 | arg);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(arg);
                            }
                        }
                        1 => {
                            // continuation
                            code += "PUSHCONT  { NOP }\n";
                            bytecode.append(
                                &mut compile_code("PUSHCONT  { NOP }").unwrap().get_bytestring(0),
                            );
                        }
                        2 => {
                            // slice
                            code += "PUSHSLICE  x4_\n";
                            bytecode.append(
                                &mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0),
                            );
                        }
                        3 => {
                            // cell
                            code += "NEWC ENDC\n";
                            bytecode
                                .append(&mut compile_code("NEWC ENDC").unwrap().get_bytestring(0));
                        }
                        4 => {
                            // builder
                            code += "NEWC\n";
                            bytecode.append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                        }
                        _ => {}
                    }
                    stack.push(int!(0));
                } else if stack_position == 0 {
                    match arg_type {
                        0 => {
                            stack.push(int!(arg));
                        }
                        1 => {
                            stack.push_cont(ContinuationData::with_code(
                                compile_code("NOP").unwrap(),
                            ));
                        }
                        2 => {
                            stack.push(create::slice([0x40]));
                        }
                        3 => {
                            stack.push(create::cell([0x80]));
                        }
                        4 => {
                            stack.push(create::builder([0x80]));
                        }
                        _ => {}
                    }
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                } else {
                    stack.push(int!(stack_position));
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                }
            }
            code = code + "XCHG S" + &arg.to_string();
            bytecode.push(arg);
            bytecode.push(0x80);
            let context = "Code:\n".to_string() + &code + "\n";
            // check
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&context))
                .expect_stack_extended(&stack, Some(&context));
        }
    }
}

#[test]
fn test_xchg_s1_alias() {
    compare_code("SWAP", "XCHG S1")
}

#[allow(overflowing_literals)]
#[test]
fn test_xchg_s0_si() {
    for arg in 0..256 {
        let s_size = arg + 1; // stack size for successfull execution
        for arg_type in 1..5 {
            let mut code: String = "".to_string(); // test code for execution
            let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
            let mut stack = Stack::new(); // expected stack state
            for push_value in 0..s_size {
                let stack_position = s_size - push_value - 1;
                // build code
                if stack_position == arg {
                    match arg_type {
                        0 => {
                            // int
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if stack_position <= 10 {
                                bytecode.push(0x70 | arg);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(arg);
                            }
                        }
                        1 => {
                            // continuation
                            code += "PUSHCONT  { NOP }\n";
                            bytecode.append(
                                &mut compile_code("PUSHCONT  { NOP }").unwrap().get_bytestring(0),
                            );
                        }
                        2 => {
                            // slice
                            code += "PUSHSLICE  x4_\n";
                            bytecode.append(
                                &mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0),
                            );
                        }
                        3 => {
                            // cell
                            code += "NEWC ENDC\n";
                            bytecode
                                .append(&mut compile_code("NEWC ENDC").unwrap().get_bytestring(0));
                        }
                        4 => {
                            // builder
                            code += "NEWC\n";
                            bytecode.append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                        }
                        _ => {}
                    }
                    stack.push(int!(0));
                } else if stack_position == 0 {
                    match arg_type {
                        0 => {
                            stack.push(int!(arg));
                        }
                        1 => {
                            stack.push_cont(ContinuationData::with_code(
                                compile_code("NOP").unwrap(),
                            ));
                        }
                        2 => {
                            stack.push(create::slice([0x40]));
                        }
                        3 => {
                            stack.push(create::cell([0x80]));
                        }
                        4 => {
                            stack.push(create::builder([0x80]));
                        }
                        _ => {}
                    }
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                } else {
                    stack.push(int!(stack_position));
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                }
            }
            code = code + "XCHG S0, S" + &arg.to_string();
            bytecode.push(0x11);
            bytecode.push(arg);
            bytecode.push(0x80);
            let context = "Code:\n".to_string() + &code + "\n";
            // check
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&context))
                .expect_stack_extended(&stack, Some(&context));
        }
    }
}

#[test]
fn test_xchg_s1_si() {
    for arg in 2..16 {
        let s_size = arg + 1; // stack size for successfull execution

        for arg_type in 1..5 {
            let mut code: String = "".to_string(); // test code for execution
            let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
            let mut stack = Stack::new(); // expected stack state
            for push_value in 0..s_size {
                let stack_position = s_size - push_value - 1;
                // build code
                if stack_position == arg {
                    match arg_type {
                        0 => {
                            // int
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if stack_position <= 10 {
                                bytecode.push(0x70 | arg);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(arg);
                            }
                        }
                        1 => {
                            // continuation
                            code += "PUSHCONT  { NOP }\n";
                            bytecode.append(
                                &mut compile_code("PUSHCONT  { NOP }").unwrap().get_bytestring(0),
                            );
                        }
                        2 => {
                            // slice
                            code += "PUSHSLICE  x4_\n";
                            bytecode.append(
                                &mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0),
                            );
                        }
                        3 => {
                            // cell
                            code += "NEWC ENDC\n";
                            bytecode
                                .append(&mut compile_code("NEWC ENDC").unwrap().get_bytestring(0));
                        }
                        4 => {
                            // builder
                            code += "NEWC\n";
                            bytecode.append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                        }
                        _ => {}
                    }
                    stack.push(int!(1));
                } else if stack_position == 1 {
                    match arg_type {
                        0 => {
                            stack.push(int!(arg));
                        }
                        1 => {
                            stack.push_cont(ContinuationData::with_code(
                                compile_code("NOP").unwrap(),
                            ));
                        }
                        2 => {
                            stack.push(create::slice([0x40]));
                        }
                        3 => {
                            stack.push(create::cell([0x80]));
                        }
                        4 => {
                            stack.push(create::builder([0x80]));
                        }
                        _ => {}
                    }
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                } else {
                    stack.push(int!(stack_position));
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                }
            }
            code = code + "XCHG S1, S" + &arg.to_string();
            bytecode.push(0x10 | arg);
            bytecode.push(0x80);
            let context = "Code:\n".to_string() + &code + "\n";
            // check
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&context))
                .expect_stack_extended(&stack, Some(&context));
        }
    }
}

#[cfg(feature = "ci_run")]
#[test]
fn test_xchg_si_sj() {
    for left in 1..17 {
        for right in 0..17 {
            for arg_type in 1..5 {
                let s_size = (if left >= right { left } else { right }) + 1; // stack size for successfull execution
                let mut code: String = "".to_string(); // test code for execution
                let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
                let mut stack = Stack::new(); // expected stack state
                                              // build code
                for push_value in 0..s_size {
                    let stack_position = s_size - push_value - 1;
                    if stack_position == right {
                        match arg_type {
                            0 => {
                                // int
                                code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                                if stack_position <= 10 {
                                    bytecode.push(0x70 | left);
                                } else {
                                    bytecode.push(0x80);
                                    bytecode.push(left);
                                }
                            }
                            1 => {
                                // continuation
                                code += "PUSHCONT  { NOP }\n";
                                bytecode.append(
                                    &mut compile_code("PUSHCONT  { NOP }")
                                        .unwrap()
                                        .get_bytestring(0),
                                );
                            }
                            2 => {
                                // slice
                                code += "PUSHSLICE  x4_\n";
                                bytecode.append(
                                    &mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0),
                                );
                            }
                            3 => {
                                // cell
                                code += "NEWC ENDC\n";
                                bytecode.append(
                                    &mut compile_code("NEWC ENDC").unwrap().get_bytestring(0),
                                );
                            }
                            4 => {
                                // builder
                                code += "NEWC\n";
                                bytecode
                                    .append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                            }
                            _ => {}
                        }
                        stack.push(int!(left));
                    } else {
                        if stack_position == left {
                            match arg_type {
                                0 => {
                                    stack.push(int!(right));
                                }
                                1 => {
                                    stack.push_cont(ContinuationData::with_code(
                                        compile_code("NOP").unwrap(),
                                    ));
                                }
                                2 => {
                                    stack.push(create::slice([0x40]));
                                }
                                3 => {
                                    stack.push(create::cell([0x80]));
                                }
                                4 => {
                                    stack.push(create::builder([0x80]));
                                }
                                _ => {}
                            }
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if stack_position <= 10 {
                                bytecode.push(0x70 | stack_position);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(stack_position);
                            }
                        } else {
                            stack.push(int!(stack_position));
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if stack_position <= 10 {
                                bytecode.push(0x70 | stack_position);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(stack_position);
                            }
                        }
                    }
                }
                code = code + "XCHG S" + &left.to_string() + ", S" + &right.to_string();
                if left == 1 {
                    bytecode.push(0x10 | right);
                    bytecode.push(0x80);
                } else {
                    bytecode.push(0x10);
                    bytecode.push(0xFF & ((left << 4) | right));
                    bytecode.push(0x80);
                }
                let context = "Code:\n".to_string() + &code + "\n";
                // check
                if left >= 16 || right >= 16 {
                    test_case(&code).expect_compilation_failure_extended(
                        CompileError::out_of_range(
                            (s_size + 1) as usize,
                            1,
                            "XCHG",
                            if left >= 16 { "arg 0" } else { "Register 2" },
                        ),
                        Some(&context),
                    );
                } else {
                    if left < right {
                        test_case(&code)
                            .expect_bytecode_extended(bytecode, Some(&context))
                            .expect_stack_extended(&stack, Some(&context));
                    } else {
                        if left == right {
                            test_case(&code).expect_compilation_failure_extended(
                                CompileError::logic_error(
                                    (s_size + 1) as usize,
                                    1,
                                    "XCHG",
                                    "arg 1 should be greater than arg 0",
                                ),
                                Some(&context),
                            );
                        } else {
                            test_case(&code).expect_compilation_failure_extended(
                                CompileError::logic_error(
                                    (s_size + 1) as usize,
                                    1,
                                    "XCHG",
                                    "arg 1 should be greater than arg 0",
                                ),
                                Some(&context),
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn test_xchg_si_underflow_exception() {
    for arg in 1..16 {
        let mut code: String = "".to_string(); // test code for execution
        let s_size = arg + 1; // stack size for successfull execution
        for push_value in 0..(s_size - 1) {
            code = code + "PUSHINT " + &push_value.to_string() + "\n";
        }
        code = code + "XCHG S" + &arg.to_string();
        let context = "Code:\n".to_string() + &code + "\n";
        test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
    }
}

#[test]
fn test_xchg_si_sj_underflow_exception() {
    for left in 0..15 {
        for right in (left + 1)..16 {
            let mut code: String = "".to_string(); // test code for execution
            let s_size = right + 1; // stack size for successfull execution
            for push_value in 0..(s_size - 1) {
                code = code + "PUSHINT " + &push_value.to_string() + "\n";
            }
            code = code + "XCHG S" + &left.to_string() + ", S" + &right.to_string();
            let context = "Code:\n".to_string() + &code + "\n";
            test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
        }
    }
}

#[test]
fn test_pop_short_operation_drops_top_of_stack_on_s0() {
    test_case(
        "PUSHINT  1
         POP     s0",
    )
    .expect_stack(&Stack::new());
}

#[test]
fn test_pop_short_stack_underflow() {
    test_case(
        "PUSHINT  1
         POP     s1",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_xchg_s0_si_underflow_exception() {
    for arg in 1..256 {
        let mut code: String = "".to_string(); // test code for execution
        let s_size = arg + 1; // stack size for successfull execution
        for push_value in 0..(s_size - 1) {
            code = code + "PUSHINT " + &push_value.to_string() + "\n";
        }
        code = code + "XCHG S0, S" + &arg.to_string();
        let context = "Code:\n".to_string() + &code + "\n";
        test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
    }
}

#[test]
fn test_xchg_s1_si_underflow_exception() {
    for arg in 2..16 {
        let mut code: String = "".to_string(); // test code for execution
        let s_size = arg + 1; // stack size for successfull execution
        for push_value in 0..(s_size - 1) {
            code = code + "PUSHINT " + &push_value.to_string() + "\n";
        }
        code = code + "XCHG S1, S" + &arg.to_string();
        let context = "Code:\n".to_string() + &code + "\n";
        test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
    }
}

#[test]
fn test_push_si() {
    for arg in 0..16 {
        let s_size = arg + 1; // stack size for successfull execution
        for arg_type in 1..5 {
            let mut code: String = "".to_string(); // test code for execution
            let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
            let mut stack = Stack::new(); // expected stack state
            for push_value in 0..s_size {
                let stack_position = s_size - push_value - 1;
                // build code
                if stack_position == arg {
                    match arg_type {
                        0 => {
                            // int
                            stack.push(int!(arg));
                            code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                            if arg <= 10 {
                                bytecode.push(0x70 | arg);
                            } else {
                                bytecode.push(0x80);
                                bytecode.push(arg);
                            }
                        }
                        1 => {
                            // continuation
                            stack.push_cont(ContinuationData::with_code(
                                compile_code("NOP").unwrap(),
                            ));
                            code += "PUSHCONT  { NOP }\n";
                            bytecode.append(
                                &mut compile_code("PUSHCONT  { NOP }").unwrap().get_bytestring(0),
                            );
                        }
                        2 => {
                            // slice
                            stack.push(create::slice([0x40]));
                            code += "PUSHSLICE  x4_\n";
                            bytecode.append(
                                &mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0),
                            );
                        }
                        3 => {
                            // cell
                            stack.push(create::cell([0x80]));
                            code += "NEWC ENDC\n";
                            bytecode
                                .append(&mut compile_code("NEWC ENDC").unwrap().get_bytestring(0));
                        }
                        4 => {
                            // builder
                            stack.push(create::builder([0x80]));
                            code += "NEWC\n";
                            bytecode.append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                        }
                        _ => {}
                    }
                } else {
                    stack.push(int!(stack_position));
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                }
            }
            match arg_type {
                0 => {
                    stack.push(int!(arg));
                }
                1 => {
                    stack.push_cont(ContinuationData::with_code(compile_code("NOP").unwrap()));
                }
                2 => {
                    stack.push(create::slice([0x40]));
                }
                3 => {
                    stack.push(create::cell([0x80]));
                }
                4 => {
                    stack.push(create::builder([0x80]));
                }
                _ => {}
            }
            code = code + "PUSH S" + &arg.to_string();
            bytecode.push(0x20 | arg);
            bytecode.push(0x80);
            let context = "Code:\n".to_string() + &code + "\n";
            // check
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&context))
                .expect_stack_extended(&stack, Some(&context));
        }
    }
}

fn compare_code(code1: &str, code2: &str) {
    assert_eq!(compile_code(code1), compile_code(code2));
}

#[test]
fn test_push_s0_alias() {
    // alias
    compare_code("DUP", "PUSH S0")
}

#[test]
fn test_push_s1_alias() {
    // alias
    compare_code("OVER", "PUSH S1")
}

#[test]
fn test_pop_s2000() {
    test_case("POP s2000")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "POP", "arg 0"));
}

#[test]
fn test_push_s2000() {
    test_case("PUSH s2000")
        .expect_compilation_failure(CompileError::out_of_range(1, 1, "PUSH", "arg 0"));
}

#[test]
fn test_push_si_underflow_exception() {
    for arg in 1..16 {
        let mut code: String = "".to_string(); // test code for execution
        let s_size = arg + 1; // stack size for successfull execution
        for push_value in 0..(s_size - 1) {
            code = code + "PUSHINT " + &push_value.to_string() + "\n";
        }
        code = code + "PUSH S" + &arg.to_string();
        let context = "Code:\n".to_string() + &code + "\n";
        test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
    }
}

#[test]
fn test_pop_si() {
    for arg in 0..16 {
        let s_size = arg + 1; // stack size for successfull execution
        for arg_type in 1..5 {
            let mut code: String = "".to_string(); // test code for execution
            let mut bytecode = Vec::<u8>::new(); // bytecode for iteration
            let mut stack = Stack::new(); // expected stack state
            if arg != 0 {
                match arg_type {
                    0 => {
                        stack.push(int!(arg));
                    }
                    1 => {
                        stack.push_cont(ContinuationData::with_code(compile_code("NOP").unwrap()));
                    }
                    2 => {
                        stack.push(create::slice([0x40]));
                    }
                    3 => {
                        stack.push(create::cell([0x80]));
                    }
                    4 => {
                        stack.push(create::builder([0x80]));
                    }
                    _ => {}
                }
            }
            for push_value in 0..s_size - 1 {
                let stack_position = s_size - push_value - 1;
                // build code
                if stack_position == arg {
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                } else {
                    stack.push(int!(stack_position));
                    code = code + "PUSHINT " + &stack_position.to_string() + "\n";
                    if stack_position <= 10 {
                        bytecode.push(0x70 | stack_position);
                    } else {
                        bytecode.push(0x80);
                        bytecode.push(stack_position);
                    }
                }
            }
            match arg_type {
                0 => {
                    // int
                    code += "PUSHINT 0\n";
                    bytecode.push(0x70 | arg);
                }
                1 => {
                    // continuation
                    code += "PUSHCONT  { NOP }\n";
                    bytecode
                        .append(&mut compile_code("PUSHCONT  { NOP }").unwrap().get_bytestring(0));
                }
                2 => {
                    // slice
                    code += "PUSHSLICE  x4_\n";
                    bytecode.append(&mut compile_code("PUSHSLICE  x4_").unwrap().get_bytestring(0));
                }
                3 => {
                    // cell
                    code += "NEWC ENDC\n";
                    bytecode.append(&mut compile_code("NEWC ENDC").unwrap().get_bytestring(0));
                }
                4 => {
                    // builder
                    code += "NEWC\n";
                    bytecode.append(&mut compile_code("NEWC").unwrap().get_bytestring(0));
                }
                _ => {}
            }
            code = code + "POP S" + &arg.to_string();
            bytecode.push(0x30 | arg);
            bytecode.push(0x80);
            let context = "Code:\n".to_string() + &code + "\n";
            // check
            test_case(&code)
                .expect_bytecode_extended(bytecode, Some(&context))
                .expect_stack_extended(&stack, Some(&context));
        }
    }
}

#[test]
fn test_pop_si_underflow_exception() {
    for arg in 1..16 {
        let mut code: String = "".to_string(); // test code for execution
        let s_size = arg + 1; // stack size for successfull execution
        for push_value in 0..(s_size - 1) {
            code = code + "PUSHINT " + &push_value.to_string() + "\n";
        }
        code = code + "POP S" + &arg.to_string();
        let context = "Code:\n".to_string() + &code + "\n";
        test_case(&code).expect_failure_extended(ExceptionCode::StackUnderflow, Some(&context));
    }
}

#[test]
fn test_pop_s0_alias() {
    compare_code("DROP", "POP S0")
}

#[test]
fn test_pop_s1_alias() {
    compare_code("NIP", "POP S1")
}
