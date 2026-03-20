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
use ton_assembler::compile_code_to_cell;
use ton_block::{ExceptionCode, SliceData};
use ton_vm::{
    executor::gas::gas_state::Gas,
    int,
    stack::{integer::IntegerData, StackItem},
};

#[test]
fn gas_spending_success() {
    test_case("ONE").with_gas(Gas::test_with_limit(1000)).expect_gas(1000000000, 1000, 0, 977);
}

#[test]
fn gas_spending_with_ref_success() {
    let code = compile_code_to_cell("TWO").unwrap();
    test_case_with_ref("ONE", code)
        .with_gas(Gas::test_with_limit(1000))
        .expect_gas(1000000000, 1000, 0, 849);
}

#[test]
fn gas_spending_failure_out_of_gas() {
    test_case("PUSHINT 1")
        .with_gas(Gas::test_with_limit(0))
        .skip_fift_check() // disable fift because specific gas limit
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn max_gas_limit() {
    let gas = Gas::test_with_limit(1000);
    assert_eq!(gas.get_gas_limit(), 1000);
    assert_eq!(gas.get_gas_max(), 1000000000);
}

#[test]
fn new_gas_limit() {
    let mut gas = Gas::test();
    for i in 0..2 {
        gas.new_gas_limit(1000);
        let using_result = gas.use_gas(100);
        assert_eq!(using_result, 900 - i * 100);
        assert_eq!(gas.get_gas_limit(), 1000);
        assert_eq!(gas.get_gas_credit(), 0);
        assert_eq!(gas.get_gas_remaining(), 900 - i * 100);
        assert_eq!(gas.get_gas_max(), 1000000000);
    }
}

#[test]
fn gas_using_negate() {
    let mut gas = Gas::test_with_limit(1000);
    let using_result = gas.use_gas(2000);
    assert_eq!(using_result, -1000);
    assert_eq!(gas.get_gas_limit(), 1000);
    assert_eq!(gas.get_gas_credit(), 0);
    assert_eq!(gas.get_gas_remaining(), -1000);
    assert_eq!(gas.get_gas_max(), 1000000000);
}

#[test]
fn accept() {
    test_case(
        "
        NEWC
        PUSHINT 100
        STUR 8
        ENDC
        ACCEPT
        ACCEPT
    ",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_success()
    .expect_gas(1000000000, 1000000000, 0, 999999347);
}

#[test]
fn set_gas_limit_more() {
    test_case(
        "PUSHINT 10000
         SETGASLIMIT",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_success()
    .expect_gas(1000000000, 10000, 0, 9935); // PUSHINT 1000 (34) + SETGASLIMIT (26)
}

#[test]
fn set_gas_limit_spec_limit() {
    test_case(format!(
        "PUSHINT {}
         SETGASLIMIT",
        i64::MAX
    ))
    .with_gas(Gas::test_with_limit(1000))
    .expect_success()
    .expect_gas(1000000000, 1000000000, 0, 999999946); // PUSHINT 1000 (34) + SETGASLIMIT (26)
}

#[test]
fn set_gas_limit_fail_stack_underflow() {
    test_case("SETGASLIMIT")
        .with_gas(Gas::test_with_limit(1000))
        .expect_gas(1000000000, 1000, 0, 924)
        .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn set_gas_limit_negative() {
    test_case(
        "PUSHINT -7
        SETGASLIMIT",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_gas(1000000000, 1000, 0, 948)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn buygas_normal() {
    test_case(
        "PUSHINT 1000
         BUYGAS",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_success()
    .expect_gas(1000000000, 100, 0, 35);
}

#[test]
fn buygas_out_of_range() {
    test_case(
        "PUSHINT 115792089237316195423570985008687907853269984665640564039457584007913129639935
        BUYGAS",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_success()
    .expect_gas(1000000000, 1000000000, 0, 999999946);
}

#[test]
fn buygas_fail_out_of_gas() {
    let mut gas = Gas::test_with_limit(100);
    gas.use_gas(100);

    test_case(
        "PUSHINT 10000
         BUYGAS",
    )
    .with_gas(gas)
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 100, 0, -34)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn buygas_with_zero_gas_limit() {
    test_case(
        "PUSHINT 10000
         BUYGAS",
    )
    .with_gas(Gas::test_with_limit(0))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 0, 0, -34)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn buygas_fail_stack_underflow() {
    test_case("BUYGAS")
        .with_gas(Gas::test_with_limit(1000))
        .skip_fift_check() // temporarily disable fift while no implementation
        .expect_gas(1000000000, 1000, 0, 924)
        .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn buygas_negative() {
    test_case(
        "PUSHINT -70
         BUYGAS",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 948)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn gramtogas() {
    test_case(
        "PUSHINT 10
         GRAMTOGAS",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 951)
    .expect_item(int!(1));
}

#[test]
fn gramtogas_neg() {
    test_case(
        "PUSHINT -10
         GRAMTOGAS",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 943)
    .expect_item(int!(0));
}

#[test]
fn gramtogas_spec_limit() {
    let bigint = (num::BigInt::from(i64::MAX) + 7) * 10;
    test_case(format!(
        "PUSHINT {}
         GRAMTOGAS",
        bigint
    ))
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 946)
    .expect_item(int!(i64::MAX));
}

#[test]
fn gramtogas_fail_stack_underflow() {
    test_case("GRAMTOGAS")
        .with_gas(Gas::test_with_limit(1000))
        .skip_fift_check() // temporarily disable fift while no implementation
        .expect_gas(1000000000, 1000, 0, 924)
        .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn gasconsumed() {
    test_case("GASCONSUMED").expect_int_stack(&[26]);
}

#[test]
fn gastogram() {
    test_case(
        "PUSHINT 200
         GASTOGRAM",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 935)
    .expect_item(int!(2000));
}

#[test]
fn gastogram_neg() {
    test_case(
        "PUSHINT -10
         GASTOGRAM",
    )
    .with_gas(Gas::test_with_limit(1000))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_gas(1000000000, 1000, 0, 943)
    .expect_item(int!(-100));
}

#[test]
fn gastogram_max() {
    test_case(format!(
        "
        PUSHINT {}
        GASTOGRAM
    ",
        i64::MAX
    ))
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_item(int!(i64::MAX as i128 * 10));
}

#[test]
fn gastogram_fail_stack_underflow() {
    test_case("GASTOGRAM")
        .with_gas(Gas::test_with_limit(1000))
        .skip_fift_check() // temporarily disable fift while no implementation
        .expect_gas(1000000000, 1000, 0, 924)
        .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn gastogram_o_gramtogas_eq_identity() {
    test_case(
        "
        PUSHINT 10000
        GASTOGRAM
        GRAMTOGAS",
    )
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_item(int!(10000));
}

#[test]
fn gramtogas_o_gastogram_eq_identity() {
    test_case(
        "
        PUSHINT 1000000000
        GRAMTOGAS
        GASTOGRAM",
    )
    .skip_fift_check() // temporarily disable fift while no implementation
    .expect_item(int!(1000000000));
}

#[test]
fn commit_with_out_of_gas() {
    test_case(
        "
        COMMIT
        ",
    )
    .with_gas_limit(9)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn commit_with_throw() {
    test_case(
        "
        PUSHINT 100

        NEWC
        STSLICECONST x1234_
        ENDC
        POPROOT

        COMMIT

        NEWC
        STSLICECONST xF_
        ENDC
        POPROOT

        THROW 101
        ",
    )
    .expect_custom_failure(101);
}

#[test]
fn commit_with_default_commit() {
    test_case(
        "
        PUSHINT 100
        COMMIT

        NEWC
        STSLICECONST xF_
        ENDC
        POPROOT
        ",
    )
    .expect_item(int!(100));
}

#[test]
fn commit_with_sendrawmsg() {
    test_case(
        "
        NEWC
        STSLICECONST xF_
        ENDC
        PUSHINT 10
        SENDRAWMSG
        COMMIT
        THROW 101
        ",
    )
    .expect_custom_failure(101);
}

#[test]
fn commit_with_double_sendrawmsg() {
    test_case(
        "
        NEWC
        STSLICECONST xF_
        ENDC
        PUSHINT 10
        SENDRAWMSG
        COMMIT

        NEWC
        STSLICECONST x1234_
        ENDC
        PUSHINT 10
        SENDRAWMSG
        THROW 101
        ",
    )
    .expect_custom_failure(101);
}

#[test]
fn commit_with_double_poproot() {
    test_case(
        "
        NEWC
        STSLICECONST xF_
        ENDC
        POPROOT
        COMMIT

        NEWC
        STSLICECONST x1234_
        ENDC
        POPROOT
        THROW 9
        ",
    )
    .expect_custom_failure(9);
}

#[test]
fn test_out_of_gas_inside_command() {
    let code = compile_code_to_cell("TWO").unwrap();
    test_case_with_ref("ONE", code)
        .with_gas(Gas::test_with_limit(10))
        .expect_gas(1000000000, 10, 0, -8)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_bad_command() {
    let code = compile_code_to_cell("TWO").unwrap();
    test_case_with_ref("CTOS", code)
        .with_gas(Gas::test_with_limit(10))
        .expect_gas(1000000000, 10, 0, -8)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_implicit_jmpref_cmd() {
    let code = compile_code_to_cell("TWO").unwrap();
    test_case_with_ref("ONE", code)
        .with_gas(Gas::test_with_limit(18))
        .expect_gas(1000000000, 18, 0, -10)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_implicit_jmpref_load_cell() {
    let code = compile_code_to_cell("TWO").unwrap();
    test_case_with_ref("ONE", code)
        .with_gas(Gas::test_with_limit(28))
        .expect_gas(1000000000, 28, 0, -100)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_bad_implicit_jmpref() {
    test_case_with_ref("ONE", SliceData::new(vec![0x40]).into_cell().unwrap())
        .with_gas(Gas::test_with_limit(28))
        .expect_gas(1000000000, 28, 0, -100)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_after_bad_implicit_jmpref_exact() {
    test_case_with_ref("ONE", SliceData::new(vec![0x40]).into_cell().unwrap())
        .with_gas(Gas::test_with_limit(128))
        .expect_gas(1000000000, 128, 0, -18)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_after_bad_implicit_jmpref_and_command() {
    test_case_with_ref("ONE", SliceData::new(vec![0x70, 40]).into_cell().unwrap())
        .with_gas(Gas::test_with_limit(146))
        .expect_gas(1000000000, 146, 0, -18)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_implicit_ret() {
    test_case("NOP")
        .with_gas(Gas::test_with_limit(18))
        .expect_gas(1000000000, 18, 0, -5)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_try_command() {
    test_case(
        "
        PUSHCONT {
            CTOS
        }
        PUSHCONT {
            PUSHINT 7
        }
        TRY
    ",
    )
    .with_gas(Gas::test_with_limit(36))
    .expect_gas(1000000000, 36, 0, -26)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_try_block() {
    test_case(
        "
        PUSHCONT {
            CTOS
        }
        PUSHCONT {
            PUSHINT 7
        }
        TRY
    ",
    )
    .with_gas(Gas::test_with_limit(62))
    .expect_gas(1000000000, 62, 0, -18)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_catch_block() {
    test_case(
        "
        PUSHCONT {
            CTOS
        }
        PUSHCONT {
            PUSHINT 7
        }
        TRY
    ",
    )
    .with_gas(Gas::test_with_limit(130))
    .expect_gas(1000000000, 130, 0, -18)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_out_of_gas_inside_ret_of_catch_block() {
    test_case(
        "
        PUSHCONT {
            CTOS
        }
        PUSHCONT {
            PUSHINT 7
        }
        TRY
    ",
    )
    .with_gas(Gas::test_with_limit(148))
    .expect_gas(1000000000, 148, 0, -5)
    .expect_failure(ExceptionCode::OutOfGas);
}

// #[test]
// fn test_out_of_gas_with_credit() {
//     test_case("
//         PUSHINT 7
//     ")
//     .with_gas(Gas::test_with_credit(100))
//     .expect_gas(1000000000, 0, 100, -8)
//     .expect_failure(ExceptionCode::OutOfGas);
// }

#[ignore]
#[test]
fn test_my_gas() {
    let code = "ZERO ADDCONST 2";
    test_case(code)
        .with_gas(Gas::test_with_limit(105))
        .expect_gas(1000000000, 105, 0, 82)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_system_exception_steps() {
    test_case("CTOS").expect_steps(2).expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_direct_exception_steps_oog() {
    test_case("THROW 111")
        .with_gas(Gas::test_with_limit(30))
        .expect_gas(1000000000, 30, 0, -4)
        .expect_steps(2)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_direct_exception_steps() {
    test_case("THROW 111").expect_steps(1).expect_custom_failure(111);
}

#[test]
fn test_system_exception_steps_oog() {
    test_case("CTOS")
        .with_gas(Gas::test_with_limit(10))
        .expect_gas(1000000000, 10, 0, -8)
        .expect_steps(2)
        .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_system_exception_steps_oog_implicit_ret() {
    test_case(
        "
        PUSHCONT {
        }
        CALLX
    ",
    )
    .with_gas(Gas::test_with_limit(36))
    .expect_gas(1000000000, 36, 0, -5)
    .expect_steps(4)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_system_exception_steps_oog_repeat() {
    test_case(
        "
        PUSHINT 2000
        PUSHCONT {
            PUSHINT 100
            PUSHINT 100
            ADD
        }
        REPEAT
    ",
    )
    .with_gas(Gas::test_with_limit(99999))
    .expect_gas(1000000000, 99999, 0, -23)
    .expect_steps(5334)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_system_exception_steps_oog_sendrawmsg() {
    test_case(
        "
        PUSHINT 1
        PUSHINT 2
        SENDRAWMSG
    ",
    )
    .with_gas(Gas::test_with_limit(50))
    .expect_gas(1000000000, 50, 0, -12)
    .expect_steps(4)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_tuple_gas() {
    test_case(
        "
        GETGLOB 4
        PUSHINT -1
        SETINDEXQ 1
        PUSHINT 10000000
        SETINDEXQ 2
        NULL
        SETINDEXQ 3
        PUSHINT 0
        SETINDEXQ 4
        SETGLOB 4
    ",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_gas(1000000000, 1000, 0, 747)
    .expect_success();
}

#[test]
fn test_tuple_non_quiet_gas() {
    test_case(
        "GETGLOB 4
        PUSHINT 0
        SETINDEXQ 0
        NULL
        SETINDEX 0
        SETGLOB 4",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_gas(1000000000, 1000, 0, 848)
    .expect_success();
}

#[test]
fn test_tuple_null_outbound_gas() {
    test_case(
        "PUSHINT 7
        TUPLE 1
        NULL
        SETINDEXQ 1",
    )
    .with_gas(Gas::test_with_limit(1000))
    .expect_gas(1000000000, 1000, 0, 906)
    .expect_item(create::tuple(&[int!(7)]));
}

#[test]
fn test_deep_stack_switch() {
    test_case(
        "NULL
        PUSHINT 10000
        PUSHCONT {
            BLKPUSH 15, 0
        }
        REPEAT
        ZERO
        ONLYX
    ",
    )
    .expect_steps(20007)
    .expect_success();
}

#[test]
fn test_long_tuple_chain() {
    test_case(
        "
        NULL
        AGAINEND
        TUPLE 1
    ",
    )
    .with_gas_limit(10_000_000)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_call_stack_overflow() {
    test_case(
        "
        CALL 0
    ",
    )
    .with_gas_limit(10_000_000)
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_stack_overflow_bugreport() {
    test_case(
        "
        PUSHCONT {
            DUP
            EXECUTE
        }
        DUP
        EXECUTE
    ",
    )
    .with_gas(Gas::test_with_limit(1000000))
    .expect_failure(ExceptionCode::OutOfGas);
}

#[test]
fn test_chksignu_revised() {
    const ITERS: usize = 11;
    let code = format!("
        PUSHINT 64173879152840467909425465404518979291640888877140559404928490924164878686861
        PUSHSLICE xedf0554ee6f844bb7b08c91771d44c30dd69cc5b192ca2d8beff2e38b34f3d8f3c6e76b8c37c2a2fa3ea0bf082a128e2ae4c5befd941160ffcf4aed9e0d8f905
        PUSHINT 111233756821887609796309114891759447673260034901478403836056446283110540365989
        PUSHINT {ITERS}
        PUSHCONT {{
            BLKPUSH 3, 2
            CHKSIGNU
            THROWIFNOT 111
        }}
        REPEAT
    ");
    test_case(&code)
        .with_gas(Gas::test_with_limit(10_000))
        .expect_gas(1000000000, 10_000, 0, 4748)
        .expect_success();
}

#[test]
fn test_stack_switch_gas_bug() {
    test_case(
        "
        PUSHCONT {}
        DUP
        TRYARGS 0, 0
        CALLDICT 0
    ",
    )
    .with_gas_limit(100_000)
    .expect_steps(1950)
    .expect_failure(ExceptionCode::OutOfGas);
}
