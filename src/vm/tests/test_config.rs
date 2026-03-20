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
use ton_block::{ExceptionCode, SliceData};
use ton_vm::stack::{Stack, StackItem};

mod common;
use common::*;

fn test_case_with_c7(code: &str) -> TestCaseInputs {
    let prefix = "
        PUSHINT 0
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        PUSHINT 4
        PUSHINT 5
        PUSHINT 6
        ; balance 1000 grams and no others
        PUSHINT 1000
        NULL
        PAIR
        ; prepare my address addr_var $11_0_000001000_0:32_0101_0101 => $1100_0000_1000_0:32_0101_0101
        PUSHSLICE xC080000000055
        ; prepare config_param dictionary [-1]=x12345, [2000]=x67890
        NEWC
        STSLICECONST x12345
        ENDC
        PUSHINT -1
        NULL
        PUSHINT 32
        DICTISETREF
        NEWC
        STSLICECONST x67890
        ENDC
        PUSHINT 2000
        ROT
        PUSHINT 32
        DICTISETREF
        NEWC
        STSLICECONST xABCDEF
        ENDC
        PUSHINT 1856 ; incoming value
        NULL
        PAIR
        PUSHINT 9112 ; storage fees
        PUSHINT 1
        PUSHINT 2
        PUSHINT 3
        TRIPLE ; prev blocks
        PUSHINT 1
        ; PUSHSLICE x78030000 ; 888 global id
        PUSHSLICE x00000378 ; 888 global id
        ; NEWC
        ; PUSHINT 888 ; global id
        ; STU 32
        ; ENDC
        ; CTOS
        PUSHINT 3
        PUSHINT 4
        PUSHINT 5
        PUSHINT 6
        PUSHINT 7
        TUPLE 7 ; unpacked config params
        PUSHINT 777 ; due payment
        PUSHINT 12 ; precompiled gas
        TRUE
        FALSE
        PUSHSLICE x2_
        PUSHINT 127
        PUSHINT 10001001
        PUSHINT 1717171717
        PUSHINT 1857 ; original value
        PUSHINT 1858 ; incoming value
        NULL
        NULL ; state init
        TUPLE 10
        PUSHINT 18
        TUPLEVAR
        SINGLE
        POP c7";
    test_case(format!("{} {}", prefix, code))
}

mod getparam {
    use super::*;
    use ton_block::Serializable;

    #[test]
    fn normal_flow() {
        test_case_with_c7("GLOBALID").expect_item(StackItem::int(888));
        test_case_with_c7("GETPRECOMPILEDGAS").expect_item(StackItem::int(12));
        test_case_with_c7("GETPARAM 1").expect_item(StackItem::int(1));
        test_case_with_c7("GETPARAM 2").expect_item(StackItem::int(2));
        test_case_with_c7("GETPARAM 6").expect_item(StackItem::int(6));
        test_case_with_c7("GETPARAM 7")
            .expect_item(create::tuple(&[StackItem::int(1000), StackItem::None]));
        test_case_with_c7("INCOMINGVALUE UNTUPLE 2 DROP").expect_int_stack(&[1856]);
        test_case_with_c7("DUEPAYMENT").expect_int_stack(&[777]);
        test_case_with_c7("PREVBLOCKSINFOTUPLE UNTUPLE 3").expect_int_stack(&[1, 2, 3]);
        test_case_with_c7("UNPACKEDCONFIGTUPLE UNTUPLE 7").expect_stack(
            Stack::new()
                .push(StackItem::int(1))
                .push(StackItem::to_slice(888u32).unwrap())
                .push(StackItem::int(3))
                .push(StackItem::int(4))
                .push(StackItem::int(5))
                .push(StackItem::int(6))
                .push(StackItem::int(7)),
        );
        test_case_with_c7("INMSGPARAMS UNTUPLE 10").expect_stack(
            Stack::new()
                .push(StackItem::boolean(true))
                .push(StackItem::boolean(false))
                .push(StackItem::to_slice(ton_block::MsgAddressExt::AddrNone).unwrap())
                .push(StackItem::int(127))
                .push(StackItem::int(10001001))
                .push(StackItem::int(1717171717))
                .push(StackItem::int(1857))
                .push(StackItem::int(1858))
                .push(StackItem::None)
                .push(StackItem::None),
        );
        test_case_with_c7(
            "
            INMSG_BOUNCE
            INMSG_BOUNCED
            INMSG_SRC
            INMSG_FWDFEE
            INMSG_LT
            INMSG_UTIME
            INMSG_ORIGVALUE
            INMSG_VALUE
            INMSG_VALUEEXTRA
            INMSG_STATEINIT
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::boolean(true))
                .push(StackItem::boolean(false))
                .push(StackItem::to_slice(ton_block::MsgAddressExt::AddrNone).unwrap())
                .push(StackItem::int(127))
                .push(StackItem::int(10001001))
                .push(StackItem::int(1717171717))
                .push(StackItem::int(1857))
                .push(StackItem::int(1858))
                .push(StackItem::None)
                .push(StackItem::None),
        );
    }

    #[test]
    fn range_check_error() {
        for index in 1..=15 {
            let code = format!(
                "
                NIL
                SINGLE
                POP c7
                GETPARAM {index}
            "
            );
            expect_exception(&code, ExceptionCode::RangeCheckError);
        }
        let configs: [(&str, &[i32]); 13] = [
            ("NOW", &[3]),
            ("BLOCKLT", &[4]),
            ("LTIME", &[5]),
            ("RANDSEED", &[6]),
            ("BALANCE UNTUPLE 2 DROP", &[1000]),
            ("MYADDR", &[]),
            ("CONFIGROOT", &[]),
            ("MYCODE", &[]),
            ("INCOMINGVALUE UNTUPLE 2 DROP", &[1856]),
            ("STORAGEFEES", &[9112]),
            ("PREVBLOCKSINFOTUPLE UNTUPLE 3", &[1, 2, 3]),
            ("DUEPAYMENT", &[777]),
            ("GETPRECOMPILEDGAS", &[12]),
        ];
        for (code, expected) in configs.iter() {
            if !expected.is_empty() {
                test_case_with_c7(code).expect_int_stack(expected);
            }
            // test_case_with_c7().expect_int_stack(expected);
            let code = format!(
                "
                NIL
                SINGLE
                POP c7
                {code}
            "
            );
            expect_exception(&code, ExceptionCode::RangeCheckError);
        }
    }

    #[test]
    fn my_code_gas() {
        let code = ton_assembler::compile_code_to_cell(
            "
            MYCODE
            CTOS ; 100 gas
            DROP
            MYCODE
            CTOS ; 25 gas
            SBITS
        ",
        )
        .unwrap();
        let library_cell_code = code.as_library_cell();
        let mut library = ton_block::HashmapE::with_bit_len(256);
        let key = code.repr_hash().write_to_bitstring().unwrap();
        library.setref(key, &code).unwrap();
        // simple case
        test_case_with_bytecode(code)
            .with_account(SHARD_ACCOUNT.clone())
            .with_mc_state_proof(MC_STATE_PROOF.clone())
            .expect_gas(1_000_000_000, 1_000_000_000, 0, 999999738)
            .expect_int_stack(&[72]);

        // library cell without library
        test_case_with_bytecode(library_cell_code)
            .with_account(SHARD_ACCOUNT.clone())
            .with_mc_state_proof(MC_STATE_PROOF.clone())
            .expect_gas(1_000_000_000, 1_000_000_000, 0, 999999840)
            .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod root {

    use super::*;
    use ton_block::{HashmapE, HashmapType};

    #[test]
    fn normal_flow() {
        let mut params = HashmapE::with_bit_len(32);
        params
            .setref(
                SliceData::from_raw(2000i32.to_be_bytes().to_vec(), 32),
                &SliceData::new(vec![0x67, 0x89, 0x08]).into_cell().unwrap(),
            )
            .unwrap();
        params
            .setref(
                SliceData::from_raw((-1i32).to_be_bytes().to_vec(), 32),
                &SliceData::new(vec![0x12, 0x34, 0x58]).into_cell().unwrap(),
            )
            .unwrap();
        test_case_with_c7("CONFIGROOT").expect_item(StackItem::dict(params.data()));

        test_case(
            "
            ZERO
            DUP
            DUP
            DUP2
            DUP2
            DUP2
            NULL
            TUPLE 10
            SINGLE
            POP c7
            CONFIGROOT
        ",
        )
        .expect_item(StackItem::None);
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            CONFIGROOT
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod dict {

    use super::*;
    use ton_block::{HashmapE, HashmapType};

    #[test]
    fn normal_flow() {
        let mut params = HashmapE::with_bit_len(32);
        params
            .setref(
                SliceData::from_raw(2000i32.to_be_bytes().to_vec(), 32),
                &SliceData::new(vec![0x67, 0x89, 0x08]).into_cell().unwrap(),
            )
            .unwrap();
        params
            .setref(
                SliceData::from_raw((-1i32).to_be_bytes().to_vec(), 32),
                &SliceData::new(vec![0x12, 0x34, 0x58]).into_cell().unwrap(),
            )
            .unwrap();
        test_case_with_c7("CONFIGDICT").expect_stack(
            Stack::new()
                .push(StackItem::Cell(params.data().unwrap().clone()))
                .push(StackItem::int(32)),
        );

        test_case(
            "
            ZERO
            DUP
            DUP
            DUP2
            DUP2
            DUP2
            NULL
            TUPLE 10
            SINGLE
            POP c7
            CONFIGDICT
        ",
        )
        .expect_stack(Stack::new().push(StackItem::None).push(StackItem::int(32)));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            CONFIGDICT
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod param_ref {

    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("PUSHINT -1 CONFIGPARAM").expect_stack(
            Stack::new().push(create::cell([0x12, 0x34, 0x58])).push(StackItem::int(-1)),
        );

        test_case_with_c7("PUSHINT 2000 CONFIGPARAM").expect_stack(
            Stack::new().push(create::cell([0x67, 0x89, 0x08])).push(StackItem::int(-1)),
        );

        test_case_with_c7("PUSHINT 0 CONFIGPARAM")
            .expect_stack(Stack::new().push(StackItem::int(0)));

        test_case(
            "
            ZERO
            DUP
            DUP
            DUP2
            DUP2
            DUP2
            NULL
            TUPLE 10
            SINGLE
            POP c7
            ZERO
            CONFIGPARAM
        ",
        )
        .expect_item(StackItem::int(0));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            ZERO
            CONFIGPARAM
        ",
            ExceptionCode::RangeCheckError,
        );
    }

    #[test]
    fn type_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            NULL
            CONFIGPARAM
        ",
            ExceptionCode::TypeCheckError,
        );
    }
}

mod param_opt {

    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("PUSHINT -1 CONFIGOPTPARAM")
            .expect_item(create::cell([0x12, 0x34, 0x58]));
        test_case_with_c7("PUSHINT 2000 CONFIGOPTPARAM")
            .expect_item(create::cell([0x67, 0x89, 0x08]));
        test_case_with_c7("PUSHINT 0 CONFIGOPTPARAM").expect_item(StackItem::None);

        test_case(
            "
            ZERO
            DUP
            DUP
            DUP2
            DUP2
            DUP2
            NULL
            TUPLE 10
            SINGLE
            POP c7
            ZERO
            CONFIGOPTPARAM
        ",
        )
        .expect_item(StackItem::None);
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            ZERO
            CONFIGOPTPARAM
        ",
            ExceptionCode::RangeCheckError,
        );
    }

    #[test]
    fn type_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            NULL
            CONFIGOPTPARAM
        ",
            ExceptionCode::TypeCheckError,
        );
    }
}

mod balance {
    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("BALANCE UNPAIR")
            .expect_stack(Stack::new().push(StackItem::int(1000)).push(StackItem::None));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            RANDSEED
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod myaddr {
    use super::*;

    #[test]
    fn normal_flow() {
        let slice = SliceData::new(vec![0xc0, 0x80, 0x00, 0x00, 0x00, 0x05, 0x58]);
        test_case_with_c7("MYADDR").expect_item(StackItem::Slice(slice));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            MYADDR
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod randseed {
    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("RANDSEED").expect_item(StackItem::int(6));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            SINGLE
            POP c7
            RANDSEED
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod now {
    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("NOW").expect_item(StackItem::int(3));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            PUSHINT 1
            SINGLE
            SINGLE
            POP c7
            NOW
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod blocklt {
    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("BLOCKLT").expect_item(StackItem::int(4));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            PUSHINT 1
            SINGLE
            SINGLE
            POP c7
            BLOCKLT
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

mod ltime {
    use super::*;

    #[test]
    fn normal_flow() {
        test_case_with_c7("LTIME").expect_item(StackItem::int(5));
    }

    #[test]
    fn range_check_error() {
        expect_exception(
            "
            NIL
            POP c7
            LTIME
        ",
            ExceptionCode::RangeCheckError,
        );
    }
}

#[test]
fn test_setgetglobvar_normal() {
    test_case(
        "
        ONE
        TWO
        TEN
        TRIPLE
        POP c7
        PUSHINT 9
        PUSHINT 3
        SETGLOBVAR
        PUSHINT 2
        GETGLOBVAR
        PUSHINT 3
        GETGLOBVAR
    ",
    )
    .expect_int_stack(&[10, 9]);

    test_case(
        "
        ONE
        TWO
        TEN
        TRIPLE
        POP c7
        PUSHINT 5
        GETGLOBVAR
    ",
    )
    .expect_item(StackItem::None);
}

#[test]
fn test_setgetglob_normal() {
    test_case(
        "
        ONE
        TWO
        TEN
        TRIPLE
        POP c7
        PUSHINT 9
        SETGLOB 31
        GETGLOB 2
        GETGLOB 31
    ",
    )
    .expect_int_stack(&[10, 9]);

    test_case(
        "
        ONE
        TWO
        TEN
        TRIPLE
        POP c7
        GETGLOB 5
    ",
    )
    .expect_item(StackItem::None);
}

#[test]
fn test_setgetglobvar_range_error() {
    expect_exception(
        "
        PUSHINT 255
        GETGLOBVAR
    ",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "
        PUSHINT -1
        GETGLOBVAR
    ",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "
        ZERO
        PUSHINT 255
        SETGLOBVAR
    ",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "
        ZERO
        PUSHINT -1
        SETGLOBVAR
    ",
        ExceptionCode::RangeCheckError,
    );
}

#[test]
fn test_setgetglobvar_stack_underflow() {
    expect_exception("GETGLOBVAR", ExceptionCode::StackUnderflow);
    expect_exception("SETGLOBVAR", ExceptionCode::StackUnderflow);
    expect_exception("ZERO SETGLOBVAR", ExceptionCode::StackUnderflow);
}

mod calc_fees {
    use super::*;
    use ton_block::{
        CurrencyCollection, InternalMessageHeader, Message, MsgAddressInt, Serializable,
    };

    #[test]
    fn test_calc_gas_fee_bad() {
        expect_exception("GETGASFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETGASFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO NULL GETGASFEE", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO GETGASFEE", ExceptionCode::TypeCheckError);
        expect_exception("TRUE TRUE GETGASFEE", ExceptionCode::RangeCheckError);
        expect_exception("PUSHPOW2 63 TRUE GETGASFEE", ExceptionCode::RangeCheckError);

        expect_exception("GETGASFEESIMPLE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETGASFEESIMPLE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO NULL GETGASFEESIMPLE", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO GETGASFEESIMPLE", ExceptionCode::TypeCheckError);
        expect_exception("TRUE TRUE GETGASFEESIMPLE", ExceptionCode::RangeCheckError);
        expect_exception("PUSHPOW2 63 TRUE GETGASFEESIMPLE", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_calc_gas_fee_good() {
        test_case("GLOBALID").with_mc_state(MC_STATE_ROOT.clone()).expect_int_stack(&[795]);

        test_case(
            "
            PREVMCBLOCKS
            PREVKEYBLOCK
            PREVMCBLOCKS_100
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .execute();

        test_case(
            "
            PUSHINT 100
            FALSE
            GETGASFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(1000000));

        test_case(
            "
            PUSHINT 2000
            TRUE
            GETGASFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(20000000));

        test_case(
            "
            PUSHPOW2DEC 63
            TRUE
            GETGASFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("92233720368547758070000").unwrap());

        test_case(
            "
            PUSHINT 100
            FALSE
            GETGASFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(100_000));

        test_case(
            "
            PUSHINT 2000
            TRUE
            GETGASFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(20_000_000));

        test_case(
            "
            PUSHPOW2DEC 63
            TRUE
            GETGASFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("92233720368547758070000").unwrap());
    }

    #[test]
    fn test_calc_fwd_fee_bad() {
        expect_exception("GETFORWARDFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETFORWARDFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ZERO GETFORWARDFEE", ExceptionCode::StackUnderflow);
        expect_exception("NULL ZERO ZERO GETFORWARDFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL ZERO GETFORWARDFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO ZERO NULL GETFORWARDFEE", ExceptionCode::TypeCheckError);
        expect_exception("PUSHPOW2 63 ZERO TRUE GETFORWARDFEE", ExceptionCode::RangeCheckError);
        expect_exception("ZERO PUSHPOW2 63 TRUE GETFORWARDFEE", ExceptionCode::RangeCheckError);

        expect_exception("GETFORWARDFEESIMPLE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETFORWARDFEESIMPLE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ZERO GETFORWARDFEESIMPLE", ExceptionCode::StackUnderflow);
        expect_exception("NULL ZERO ZERO GETFORWARDFEESIMPLE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL ZERO GETFORWARDFEESIMPLE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO ZERO NULL GETFORWARDFEESIMPLE", ExceptionCode::TypeCheckError);
        expect_exception(
            "PUSHPOW2 63 ZERO TRUE GETFORWARDFEESIMPLE",
            ExceptionCode::RangeCheckError,
        );
        expect_exception(
            "ZERO PUSHPOW2 63 TRUE GETFORWARDFEESIMPLE",
            ExceptionCode::RangeCheckError,
        );

        expect_exception("GETORIGINALFWDFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETORIGINALFWDFEE", ExceptionCode::StackUnderflow);
        expect_exception("NULL ZERO GETORIGINALFWDFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL GETORIGINALFWDFEE", ExceptionCode::TypeCheckError);
        expect_exception("TRUE TRUE GETORIGINALFWDFEE", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_calc_fwd_fee_good() {
        test_case(
            "
            PUSHINT 5
            PUSHINT 20000
            TRUE
            GETFORWARDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(215000000));

        test_case(
            "
            PUSHINT 5
            PUSHPOW2DEC 63
            TRUE
            GETFORWARDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("92233720368547773070000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHINT 5
            TRUE
            GETFORWARDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("9223372036854775817050000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            TRUE
            GETFORWARDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("9315605757223323575070000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            FALSE
            GETFORWARDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("931560575722332357507000").unwrap());

        test_case(
            "
            PUSHINT 5
            PUSHINT 20000
            TRUE
            GETFORWARDFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::int(205000000));

        test_case(
            "
            PUSHINT 5
            PUSHPOW2DEC 63
            TRUE
            GETFORWARDFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("92233720368547763070000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHINT 5
            TRUE
            GETFORWARDFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("9223372036854775807050000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            TRUE
            GETFORWARDFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("9315605757223323565070000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            FALSE
            GETFORWARDFEESIMPLE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("931560575722332356507000").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            TRUE
            GETORIGINALFWDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("13834952502971197438").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            FALSE
            GETORIGINALFWDFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("13834952502971197438").unwrap());
    }

    #[test]
    fn test_calc_storage_fee_bad() {
        expect_exception("GETSTORAGEFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO GETSTORAGEFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ZERO GETSTORAGEFEE", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ZERO ZERO GETSTORAGEFEE", ExceptionCode::StackUnderflow);
        expect_exception("NULL ZERO ZERO ZERO GETSTORAGEFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL ZERO ZERO GETSTORAGEFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO ZERO NULL ZERO GETSTORAGEFEE", ExceptionCode::TypeCheckError);
        expect_exception("ZERO ZERO ZERO NULL GETSTORAGEFEE", ExceptionCode::TypeCheckError);
        expect_exception(
            "PUSHPOW2 63 ZERO ZERO TRUE GETSTORAGEFEE",
            ExceptionCode::RangeCheckError,
        );
        expect_exception(
            "ZERO PUSHPOW2 63 ZERO TRUE GETSTORAGEFEE",
            ExceptionCode::RangeCheckError,
        );
        expect_exception(
            "ZERO ZERO PUSHPOW2 63 TRUE GETSTORAGEFEE",
            ExceptionCode::RangeCheckError,
        );
    }

    #[test]
    fn test_calc_storage_fee_good() {
        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            TRUE
            GETSTORAGEFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("650335181531487160332425701902778368008").unwrap());

        test_case(
            "
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            PUSHPOW2DEC 63
            FALSE
            GETSTORAGEFEE
        ",
        )
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::big_int("650335181531487160332425701902778369").unwrap());
    }

    #[test]
    fn test_get_accont_and_messag_params() {
        let src = MsgAddressInt::standard(-1, [0x00; 32]);
        let addr = MsgAddressInt::standard(0, [0x55; 32]);
        let value = CurrencyCollection::with_grams(5618);
        let h = InternalMessageHeader::with_addresses_and_bounce(src, addr, value, true);
        let msg = Message::with_int_header(h);
        let message_root = msg.serialize().unwrap();

        test_case(
            "
            BLKDROP 5
            INCOMINGVALUE
        ",
        )
        .with_message_cell(message_root.clone())
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_item(StackItem::tuple(vec![StackItem::int(5618), StackItem::None]));

        test_case("DUEPAYMENT")
            .with_account(SHARD_ACCOUNT.clone())
            .with_mc_state(MC_STATE_ROOT.clone())
            .expect_item(StackItem::int(738));

        test_case(
            "
            BLKDROP 5
            INMSG_BOUNCE
            INMSG_BOUNCED
            ",
        )
        .with_message_cell(message_root.clone())
        .with_mc_state(MC_STATE_ROOT.clone())
        .expect_int_stack(&[-1, 0]);
    }
}
