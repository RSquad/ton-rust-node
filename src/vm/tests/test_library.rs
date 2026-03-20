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
use num::BigInt;
use std::{collections::HashSet, str::FromStr};
use ton_block::{
    BuilderData, Cell, CellType, ExceptionCode, HashmapE, IBitstring, MerkleProof, Serializable,
    SimpleLib, SliceData, StateInitLib, UInt256,
};
use ton_vm::{
    int,
    stack::{
        integer::{
            math::{utils::divmod, Round},
            IntegerData,
        },
        Stack, StackItem,
    },
};

#[test]
fn test_use_library_normal_load_cell_from_ref() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash = lib_code.repr_hash();

    let mut code_use_lib = BuilderData::with_raw(vec![2], 8).unwrap();
    code_use_lib.append_raw(hash.as_slice(), 256).unwrap();
    code_use_lib.set_type(CellType::LibraryReference);

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        ONE
        PUSHREF
        CTOS
        BLESS
        POP C0
    ",
        code_use_lib.into_cell().unwrap(),
    )
    .with_library(lib)
    .expect_int_stack(&[1, 2]);
}

#[test]
fn test_use_library_normal_compose_cell() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash = lib_code.repr_hash();
    assert_eq!(
        hash,
        "d816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf8f"
            .parse::<UInt256>()
            .unwrap()
    );

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case(
        "
        ONE
        NEWC
        PUSHINT 2
        STUR 8
        PUSHSLICE xd816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf8f
        STSLICER
        TRUE
        ENDXC
        CTOS
        BLESS
        POP C0
    ",
    )
    .with_library(lib)
    .expect_int_stack(&[1, 2]);
}

#[test]
fn test_use_library_normal_jmpref() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash = lib_code.repr_hash();

    let mut code_use_lib = BuilderData::with_raw(vec![2], 8).unwrap();
    code_use_lib.append_raw(hash.as_slice(), 256).unwrap();
    code_use_lib.set_type(CellType::LibraryReference);

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        ONE
    ",
        code_use_lib.into_cell().unwrap(),
    )
    .with_library(lib)
    .expect_int_stack(&[1, 2]);
}

#[test]
fn test_use_library_with_wrong_cell_hash() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash = lib_code.repr_hash();

    let mut code_use_lib = BuilderData::with_raw(vec![2], 8).unwrap();
    code_use_lib.append_raw(&[0; 32], 256).unwrap();
    code_use_lib.set_type(CellType::LibraryReference);

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        ONE
        PUSHREF
        CTOS
        BLESS
        POP C0
    ",
        code_use_lib.into_cell().unwrap(),
    )
    .with_library(lib)
    .expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_use_library_with_cell_type_error() {
    let lib_code1 = BuilderData::with_raw(vec![0x71], 8).unwrap().into_cell().unwrap();
    let lib_code2 = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash1 = lib_code1.repr_hash();
    let hash2 = lib_code2.repr_hash();

    let cell = Cell::default();
    let hash = cell.repr_hash();

    let mut code_use_lib = BuilderData::with_raw(vec![3], 8).unwrap();
    code_use_lib.append_raw(hash.as_slice(), 256).unwrap();
    code_use_lib.append_u16(0).unwrap();
    code_use_lib.checked_append_reference(Cell::default()).unwrap();
    code_use_lib.set_type(CellType::MerkleProof);

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash1.into(), &lib_code1).unwrap();
    lib.setref(hash2.into(), &lib_code2).unwrap();

    test_case_with_ref(
        "
        ONE
        PUSHREF
        CTOS
        BLESS
        POP C0
    ",
        code_use_lib.into_cell().unwrap(),
    )
    .with_library(lib)
    .expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_compose_exotic_cell_normal() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();

    let hash = lib_code.repr_hash();

    let mut code_use_lib = BuilderData::with_raw(vec![2], 8).unwrap();
    code_use_lib.append_raw(hash.as_slice(), 256).unwrap();
    code_use_lib.set_type(CellType::LibraryReference);

    test_case_with_ref(
        "
        PUSHREF
        HASHCU
        NEWC
        PUSHINT 2   ; library reference exotic cell type
        STUR 8
        STU 256
        TRUE
        ENDXC
    ",
        lib_code,
    )
    .expect_item(StackItem::Cell(code_use_lib.into_cell().unwrap()));
}

#[test]
fn test_compose_exotic_cell_and_load_normal() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();

    let hash = lib_code.repr_hash();

    let mut code_use_lib = BuilderData::default();
    code_use_lib.append_raw(hash.as_slice(), 256).unwrap();

    test_case_with_ref(
        "
        PUSHREF
        HASHCU
        NEWC
        PUSHINT 2   ; library reference exotic cell type
        STUR 8
        STU 256
        TRUE
        ENDXC
        XCTOS
        THROWIFNOT 111
        LDU 8
        SWAP
        TWO
        EQUAL
        THROWIFNOT 112
    ",
        lib_code,
    )
    .expect_item(StackItem::Slice(SliceData::load_builder(code_use_lib).unwrap()))
    .expect_gas(1000000000, 1000000000, 0, 999999037);
}

#[test]
fn test_incorrect_library() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();

    let hash =
        UInt256::from_str("0xd816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf80")
            .unwrap();
    assert_ne!(hash, lib_code.repr_hash());

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        PUSHREF
        PUSHSLICE xd816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf80
        NEWC
        PUSHINT 2   ; library reference exotic cell type
        STUR 8
        STSLICE
        TRUE
        ENDXC
        CTOS
    ",
        lib_code,
    )
    .with_library(lib)
    .expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_merkle_proof_cell() {
    // let merkle_proof = BuilderData::with_raw(vec![0x01, 0x02, 0x03], 24).unwrap().into_cell().unwrap();
    let merkle_proof = Cell::default();

    let hash = merkle_proof.repr_hash();
    let data = hash.as_slice().to_vec();
    let merkle_data = BuilderData::with_raw(data, 256).unwrap().into_cell().unwrap();

    let mut data = vec![0x03];
    data.extend_from_slice(hash.as_slice());
    data.extend_from_slice(&0u16.to_be_bytes());
    let merkle_data_full = BuilderData::with_raw_and_refs(data, 8 + 256 + 16, vec![merkle_proof])
        .unwrap()
        .into_cell()
        .unwrap();

    test_case_with_ref(
        "
        PUSHREF
        CTOS

        NEWC
        ENDC

        NEWC
        STREF
        PUSHINT 3   ; MerkleProof exotic cell type
        STUR 8
        STSLICE
        ZERO
        STUR 16
        TRUE
        ENDXC

        XCTOS
        THROWIFNOT 111
    ",
        merkle_data,
    )
    .expect_item(StackItem::Slice(SliceData::load_cell(merkle_data_full).unwrap()))
    .expect_gas(1000000000, 1000000000, 0, 999998461);

    // try to resolve merkle proof cell - CellUnderflow
    let code = "
        NEWC
        PUSHINT 1
        STUR 8
        ENDC
        DUP
        HASHCU

        NEWC
        PUSHINT 3   ; MerkleProof exotic cell type
        STUR 8
        STU 256
        ZERO
        STUR 16
        STREF
        TRUE
        ENDXC

        CTOS
        LDU 8
        ENDS
    ";

    expect_exception(code, ExceptionCode::CellUnderflow);

    // try to create merkle proof cell with zero hash (wrong) - CellOverflow
    let code = "
        NEWC
        PUSHINT 1
        STUR 8
        ENDC

        NEWC
        PUSHINT 3   ; MerkleProof exotic cell type
        STUR 8
        ZERO
        STUR 256
        ZERO
        STUR 16
        STREF
        TRUE
        ENDXC

        CTOS
        LDU 8
        ENDS
    ";

    test_case(code)
        .expect_failure(ExceptionCode::CellOverflow)
        .expect_gas(1000000000, 1000000000, 0, 999998626);
}

#[test]
fn test_cdepth_merkle_proof() {
    let c1 = BuilderData::with_raw(vec![0x01], 8).unwrap().into_cell().unwrap();
    let c2 = BuilderData::with_raw(vec![0x02], 8).unwrap().into_cell().unwrap();
    let c3 = BuilderData::with_raw(vec![0x03], 8).unwrap().into_cell().unwrap();
    let c4 = BuilderData::with_raw_and_refs(vec![0x04], 8, [c1, c2]).unwrap().into_cell().unwrap();
    let hash3 = c3.repr_hash();
    // let hash4 = c4.repr_hash();
    let root =
        BuilderData::with_raw_and_refs(vec![0x05], 8, [c4, c3]).unwrap().into_cell().unwrap();
    let hash5 = root.repr_hash();

    let merkle =
        MerkleProof::create_with_subtrees(&root, |hash| hash == &hash3, |hash| hash == &hash5)
            .unwrap();
    let merkle = merkle.serialize().unwrap();

    println!("{:#.100}", root);
    println!("{:#.100}", merkle);

    test_case_with_ref(
        "
        PUSHREF
        CDEPTH
        ",
        merkle.clone(),
    )
    .expect_int_stack(&[2])
    .expect_success();

    test_case_with_ref(
        "
        PUSHREF
        XCTOS
        DROP
        PLDREF
        DUP
        ZERO
        CDEPTHIX
        OVER
        CDEPTHI 1
        PUSH s2
        CDEPTHI 2
        BLKSWAP 1,3
        CDEPTHI 3
        ",
        merkle.clone(),
    )
    .expect_int_stack(&[2, 1, 1, 1])
    .expect_success();

    test_case_with_ref(
        "
        PUSHREF
        DUP
        CLEVEL
        OVER
        CLEVELMASK
        ROT
        XCTOS
        DROP
        PLDREF
        DUP
        CLEVEL
        OVER
        CLEVELMASK
        ROT
        CTOS
        PLDREFIDX 1
        DUP
        CLEVEL
        SWAP
        CLEVELMASK
        ",
        merkle.clone(),
    )
    .expect_int_stack(&[0, 0, 1, 1, 0, 0])
    .expect_success();

    test_case_with_ref(
        "
        PUSHREF
        XCTOS
        DROP
        PLDREF
        DUP
        CHASHI 0
        SWAP
        PUSHINT 1
        CHASHIX
        ",
        merkle.clone(),
    )
    .expect_stack(
        Stack::new()
            .push(
                int!(parse_hex "a69d6f441223c6475fd95057b05971c75a1613ae0c5737aa456aaa49d392b50a"),
            )
            .push(
                int!(parse_hex "d1d919f91c250ddac1c0aaf746d4b9cb0afe271a880c97e3a9da5ac6193baff5"),
            ),
    )
    .expect_success();

    let merkle = MerkleProof::create(&merkle, |h| h != &hash3).unwrap().serialize().unwrap();
    println!("{:#.100}", merkle);

    test_case_with_ref(
        "
        PUSHREF
        XCTOS
        DROP
        PLDREF
        XCTOS
        DROP
        PLDREF
        DUP
        CHASHI 0
        OVER
        PUSHINT 1
        CHASHIX
        ROT
        CHASHI 2
        ",
        merkle.clone(),
    )
    .expect_stack(
        Stack::new()
            .push(
                int!(parse_hex "a69d6f441223c6475fd95057b05971c75a1613ae0c5737aa456aaa49d392b50a"),
            )
            .push(
                int!(parse_hex "d1d919f91c250ddac1c0aaf746d4b9cb0afe271a880c97e3a9da5ac6193baff5"),
            )
            .push(
                int!(parse_hex "0e4f91022ff2356e8dbd59f4c41cc89a648ddf496b92252dde795bec68140a98"),
            ),
    )
    .expect_success();
}

#[test]
fn test_complex_merkle_cells() {
    let mut include = HashSet::<UInt256>::default();
    let cell1 = BuilderData::with_raw(vec![11], 8).unwrap().into_cell().unwrap();
    let cell2 = BuilderData::with_raw(vec![12], 8).unwrap().into_cell().unwrap();
    let cell3 = BuilderData::with_raw(vec![13], 8).unwrap().into_cell().unwrap();
    let cell4 =
        BuilderData::with_raw_and_refs(vec![43], 8, vec![cell3]).unwrap().into_cell().unwrap();
    include.insert(cell1.repr_hash());
    include.insert(cell2.repr_hash());
    let cell12 = BuilderData::with_raw_and_refs(vec![14], 8, vec![cell1, cell2])
        .unwrap()
        .into_cell()
        .unwrap();
    include.insert(cell12.repr_hash());
    let cell = BuilderData::with_raw_and_refs(vec![111], 8, vec![cell4, cell12])
        .unwrap()
        .into_cell()
        .unwrap();
    include.insert(cell.repr_hash());

    let merkle_proof = MerkleProof::create_with_subtrees(
        &cell,
        |hash| include.get(hash).is_some(),
        |hash| include.get(hash).is_some(),
    )
    .unwrap();

    let merkle_cell = merkle_proof.serialize().unwrap();

    println!("{:#.100}", merkle_cell);

    // merkle cells couldn't be resolved
    test_case_with_ref("PUSHREFSLICE", merkle_cell.clone())
        .expect_failure(ExceptionCode::CellUnderflow);

    // check if it is a merkle cell
    test_case_with_ref(
        "
        PUSHREF
        XCTOS
        SWAP
        LDU 8
        DROP
    ",
        merkle_cell.clone(),
    )
    .expect_int_stack(&[-1, 3]);
}

#[test]
fn test_merkle_update_cell() {
    let hash = Cell::default().repr_hash();
    let mut data = vec![0x04];
    data.extend_from_slice(hash.as_slice());
    data.extend_from_slice(hash.as_slice());
    data.extend_from_slice(&0u16.to_be_bytes());
    data.extend_from_slice(&0u16.to_be_bytes());
    let length_in_bits = data.len() * 8;
    let merkle_data =
        BuilderData::with_raw(data.clone(), length_in_bits).unwrap().into_cell().unwrap();
    let merkle_data_full =
        BuilderData::with_raw_and_refs(data, length_in_bits, vec![Cell::default(); 2])
            .unwrap()
            .into_cell()
            .unwrap();

    test_case_with_ref(
        "
        PUSHREF
        CTOS

        NEWC
        ENDC
        NEWC
        ENDC

        NEWC
        STREF
        STREF
        STSLICE
        TRUE
        ENDXC

        XCTOS
        THROWIFNOT 111
    ",
        merkle_data,
    )
    .expect_item(StackItem::Slice(SliceData::load_cell(merkle_data_full).unwrap()))
    .expect_gas(1000000000, 1000000000, 0, 999998011);

    let code = "
        NEWC
        PUSHINT 2
        STUR 8
        ENDC
        DUP
        HASHCU

        NEWC
        PUSHINT 1
        STUR 8
        ENDC
        DUP
        HASHCU

        NEWC
        PUSHINT 4   ; MerkleUpdate exotic cell type
        STUR 8
        STU 256
        STREF
        STU 256
        STREF
        ZERO
        STUR 16
        ZERO
        STUR 16
        TRUE
        ENDXC

        CTOS
        LDU 8
        ENDS
    ";

    expect_exception(code, ExceptionCode::CellUnderflow);

    let code = "
        NEWC
        PUSHINT 2
        STUR 8
        ENDC

        NEWC
        PUSHINT 1
        STUR 8
        ENDC

        NEWC
        PUSHINT 4   ; MerkleUpdate exotic cell type
        STUR 8
        ZERO
        STUR 256
        ZERO
        STUR 16
        STREF
        ZERO
        STUR 256
        ZERO
        STUR 16
        STREF
        TRUE
        ENDXC

        CTOS
        LDU 8
        ENDS
    ";

    expect_exception(code, ExceptionCode::CellOverflow);
}

#[test]
fn test_compose_exotic_cell_wrong_type() {
    expect_exception("ONE TRUE ENDXC", ExceptionCode::TypeCheckError);
    expect_exception("NIL TRUE ENDXC", ExceptionCode::TypeCheckError);
    expect_exception("NULL TRUE ENDXC", ExceptionCode::TypeCheckError);
    expect_exception("PUSHCONT {} TRUE ENDXC", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x12 TRUE ENDXC", ExceptionCode::TypeCheckError);
    expect_exception("NEWC ENDC TRUE ENDXC", ExceptionCode::TypeCheckError);
}

#[test]
fn test_compose_exotic_cell_wrong_cell_format() {
    expect_exception(
        "
        NEWC
        TRUE
        ENDXC
    ",
        ExceptionCode::CellOverflow,
    );

    expect_exception(
        "
        NEWC
        TWO
        STUR 8
        TRUE
        ENDXC
    ",
        ExceptionCode::CellOverflow,
    );
}

#[test]
fn test_compose_exotic_cell_and_load_as_cell() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();

    let hash = lib_code.repr_hash();

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        PUSHREF
        HASHCU
        NEWC
        PUSHINT 2   ; library reference exotic cell type
        STUR 8
        STU 256
        TRUE
        ENDXC
        DUP
        XLOAD
        DROP
        XLOAD
    ",
        lib_code.clone(),
    )
    .with_library(lib)
    .expect_item(StackItem::Cell(lib_code));
}

#[test]
fn test_code_as_exotic_cell() {
    let mut lib = HashmapE::with_bit_len(256);

    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let hash = lib_code.repr_hash();
    lib.setref(hash.into(), &lib_code).unwrap();

    let code = lib_code.as_library_cell();
    // normal case with code as library cell
    test_case_with_bytecode(code.clone()).with_library(lib.clone()).expect_item(StackItem::int(2));

    // code as library cell without libraries to check gas consumption
    // wrong cell passed to TVM as empty cell with reference
    test_case_with_bytecode(code.clone()).expect_failure(ExceptionCode::CellUnderflow);

    let lib_code = code;
    let hash = lib_code.repr_hash();
    lib.setref(hash.into(), &lib_code).unwrap();

    let code = lib_code.as_library_cell();
    // code as library cell with recursive library cell
    test_case_with_bytecode(code)
        .with_library(lib.clone())
        .expect_failure(ExceptionCode::CellUnderflow);

    // put pruned branch exotic cell to library and try to load it as code
    let mut builder = BuilderData::new();
    builder.set_type(CellType::PrunedBranch);
    builder
        .append_u8(u8::from(CellType::PrunedBranch))
        .unwrap()
        .append_u8(1)
        .unwrap()
        .append_raw(UInt256::rand().as_slice(), 256)
        .unwrap()
        .append_u16(1)
        .unwrap();
    let pruned_cell = builder.into_cell().unwrap();

    let lib_code = pruned_cell;

    let hash = lib_code.repr_hash();

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    let code = lib_code.as_library_cell();

    test_case_with_bytecode(code).expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_compose_exotic_cell_and_load_quite_as_cell() {
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();

    let hash = lib_code.repr_hash();

    let mut lib = HashmapE::with_bit_len(256);
    lib.setref(hash.into(), &lib_code).unwrap();

    test_case_with_ref(
        "
        PUSHREF
        HASHCU
        NEWC
        PUSHINT 2   ; library reference exotic cell type
        STUR 8
        STU 256
        TRUE
        ENDXC
        XLOADQ
        THROWIFNOT 100
    ",
        lib_code.clone(),
    )
    .with_library(lib)
    .expect_item(StackItem::Cell(lib_code));
}

#[test]
fn test_load_exotic_cell_as_cell_wrong_type() {
    expect_exception("ONE XLOAD", ExceptionCode::TypeCheckError);
    expect_exception("NIL XLOAD", ExceptionCode::TypeCheckError);
    expect_exception("NULL XLOAD", ExceptionCode::TypeCheckError);
    expect_exception("NEWC XLOAD", ExceptionCode::TypeCheckError);
    expect_exception("PUSHCONT {} XLOAD", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x12 XLOAD", ExceptionCode::TypeCheckError);

    expect_exception("ONE XLOADQ", ExceptionCode::TypeCheckError);
    expect_exception("NIL XLOADQ", ExceptionCode::TypeCheckError);
    expect_exception("NULL XLOADQ", ExceptionCode::TypeCheckError);
    expect_exception("NEWC XLOADQ", ExceptionCode::TypeCheckError);
    expect_exception("PUSHCONT {} XLOADQ", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x12 XLOADQ", ExceptionCode::TypeCheckError);
}

#[test]
fn test_compose_exotic_cell_wrong_cell_type() {
    test_case(
        "
        NEWC
        PUSHINT 0
        STUR 8
        TRUE
        ENDXC
    ",
    )
    .expect_failure(ExceptionCode::CellOverflow);

    test_case(
        "
        NEWC
        PUSHINT 5
        STUR 8
        TRUE
        ENDXC
    ",
    )
    .expect_failure(ExceptionCode::CellOverflow);
}

fn test_bigint_div(
    dividend: &str,
    divisor: &str,
    round_mode: Round,
    quot_ans: &str,
    remainder_ans: &str,
) {
    let dividend = BigInt::from_str(dividend).unwrap();
    let divisor = BigInt::from_str(divisor).unwrap();

    let (quot, remainder) = divmod(&dividend, &divisor, round_mode);

    let quot_ans = BigInt::from_str(quot_ans).unwrap();
    let remainder_ans = BigInt::from_str(remainder_ans).unwrap();

    assert_eq!(quot, quot_ans);
    assert_eq!(remainder, remainder_ans);
}

#[test]
fn tests_bigint_div() {
    test_bigint_div("1000", "9", Round::Ceil, "112", "-8");
    test_bigint_div(
        "1000000000000000000",
        "9000000000000000",
        Round::Ceil,
        "112",
        "-8000000000000000",
    );
    test_bigint_div("-1000", "9", Round::Ceil, "-111", "-1");
    test_bigint_div(
        "-1000000000000000000",
        "9000000000000000",
        Round::Ceil,
        "-111",
        "-1000000000000000",
    );
    test_bigint_div("1000", "-9", Round::Ceil, "-111", "1");
    test_bigint_div(
        "1000000000000000000",
        "-9000000000000000",
        Round::Ceil,
        "-111",
        "1000000000000000",
    );
    test_bigint_div("-1000", "-9", Round::Ceil, "112", "8");
    test_bigint_div(
        "-1000000000000000000",
        "-9000000000000000",
        Round::Ceil,
        "112",
        "8000000000000000",
    );

    test_bigint_div("1000", "9", Round::Nearest, "111", "1");
    test_bigint_div(
        "1000000000000000000",
        "9000000000000000",
        Round::Nearest,
        "111",
        "1000000000000000",
    );
    test_bigint_div("-1000", "9", Round::Nearest, "-111", "-1");
    test_bigint_div(
        "-1000000000000000000",
        "9000000000000000",
        Round::Nearest,
        "-111",
        "-1000000000000000",
    );
    test_bigint_div("1000", "-9", Round::Nearest, "-111", "1");
    test_bigint_div(
        "1000000000000000000",
        "-9000000000000000",
        Round::Nearest,
        "-111",
        "1000000000000000",
    );
    test_bigint_div("-1000", "-9", Round::Nearest, "111", "-1");
    test_bigint_div(
        "-1000000000000000000",
        "-9000000000000000",
        Round::Nearest,
        "111",
        "-1000000000000000",
    );

    test_bigint_div(
        "5000000000000000000",
        "2000000000000000000",
        Round::Nearest,
        "3",
        "-1000000000000000000",
    );
    test_bigint_div(
        "-5000000000000000000",
        "2000000000000000000",
        Round::Nearest,
        "-2",
        "-1000000000000000000",
    );
    test_bigint_div(
        "5000000000000000000",
        "-2000000000000000000",
        Round::Nearest,
        "-2",
        "1000000000000000000",
    );
    test_bigint_div(
        "-5000000000000000000",
        "-2000000000000000000",
        Round::Nearest,
        "3",
        "1000000000000000000",
    );

    test_bigint_div("1000", "9", Round::FloorToNegativeInfinity, "111", "1");
    test_bigint_div(
        "1000000000000000000",
        "9000000000000000",
        Round::FloorToNegativeInfinity,
        "111",
        "1000000000000000",
    );
    test_bigint_div("-1000", "9", Round::FloorToNegativeInfinity, "-112", "8");
    test_bigint_div(
        "-1000000000000000000",
        "9000000000000000",
        Round::FloorToNegativeInfinity,
        "-112",
        "8000000000000000",
    );
    test_bigint_div("1000", "-9", Round::FloorToNegativeInfinity, "-112", "-8");
    test_bigint_div(
        "1000000000000000000",
        "-9000000000000000",
        Round::FloorToNegativeInfinity,
        "-112",
        "-8000000000000000",
    );
    test_bigint_div("-1000", "-9", Round::FloorToNegativeInfinity, "111", "-1");
    test_bigint_div(
        "-1000000000000000000",
        "-9000000000000000",
        Round::FloorToNegativeInfinity,
        "111",
        "-1000000000000000",
    );

    test_bigint_div(
        "303424019600764000",
        "67374462762615477834925",
        Round::Nearest,
        "0",
        "303424019600764000",
    );
    test_bigint_div(
        "3034724019600764000",
        "67374462762615477834925",
        Round::Nearest,
        "0",
        "3034724019600764000",
    );
    test_bigint_div(
        "30934724019600764000",
        "67374462762615477834925",
        Round::Nearest,
        "0",
        "30934724019600764000",
    );
    test_bigint_div(
        "30934724401965080764000",
        "67374462762615477834925",
        Round::Nearest,
        "0",
        "30934724401965080764000",
    );
    test_bigint_div(
        "309347247401965080764000",
        "67374462762615477834925",
        Round::Nearest,
        "5",
        "-27525066411112308410625",
    );
    test_bigint_div(
        "2309347247401965080764000",
        "67374462762615477834925",
        Round::Nearest,
        "34",
        "18615513473038834376550",
    );
    test_bigint_div(
        "23093472474019650810764000",
        "67374462762615477834925",
        Round::Nearest,
        "343",
        "-15968253557458086615275",
    );
    test_bigint_div(
        "23093472474019650810764000",
        "68374492762615427834911",
        Round::Nearest,
        "338",
        "-17106079744363797435918",
    );
    test_bigint_div(
        "23093472474019650810764000",
        "18374592262615427834517",
        Round::Nearest,
        "1257",
        "-3390000087941977223869",
    );
    test_bigint_div(
        "2573300903069472664743019426508341031766400380",
        "1858237458809226201617542047938488403458417",
        Round::Nearest,
        "1385",
        "-357977381305624497276309886465407023507165",
    );
    test_bigint_div(
        "25733009030694726647744301942650558341009131766400380",
        "1858237458809226201617534420474793848840393458417",
        Round::Nearest,
        "13848",
        "136701104562207744685287915613122267363154241764",
    );
    test_bigint_div(
        "2530090306947266477443019426505584100931766400380",
        "1858237458809226201617534420474793848840393458417",
        Round::Nearest,
        "1",
        "671852848138040275825485006030790252091372941963",
    );
    test_bigint_div(
        "253009030694726647743019426505584100931766400380",
        "1858237458809226201617534420474793848840393458417",
        Round::Nearest,
        "0",
        "253009030694726647743019426505584100931766400380",
    );
    test_bigint_div(
        "25300903069472664774301942650558410093176640038",
        "1858237458809226201617534420474793848840393458417",
        Round::Nearest,
        "0",
        "25300903069472664774301942650558410093176640038",
    );
    test_bigint_div(
        "253009030698",
        "1858237458809226201617534420474793848840393458417",
        Round::Nearest,
        "0",
        "253009030698",
    );
    test_bigint_div(
        "19251874299181659011971842759180",
        "35863987523541299551572269966",
        Round::Nearest,
        "537",
        "-7087000960018847222466212562",
    );
    test_bigint_div(
        "15313291547689795475496512878911",
        "20557862036838502455082",
        Round::Nearest,
        "744887358",
        "8940664709053968225555",
    );
    test_bigint_div(
        "12381583265651812264166510594403",
        "98847588943634081944421359",
        Round::Nearest,
        "125259",
        "33122161150793890235587422",
    );
    //test_bigint_div("12381583265651812264166510594403", "0", Round::Nearest, "NaN", "NaN"); panic
    test_bigint_div(
        "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        "115792089237316195423570985008687907853269984665640564039457584007913129639934",
        Round::Nearest,
        "1",
        "1",
    );
    test_bigint_div(
        "156",
        "115792089237316195423570985008687907853269984665640564039457584007913129639934",
        Round::Nearest,
        "0",
        "156",
    );
    test_bigint_div(
        "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        "156",
        Round::Nearest,
        "742256982290488432202378109030050691367115286318208743842676820563545702820",
        "15",
    );
}

#[test]
fn recursive_load_cell_failed() {
    let mut hash = UInt256::new();
    let mut cell = Cell::default();
    let mut library = StateInitLib::default();
    for _ in 0..10 {
        let mut b = BuilderData::default();
        b.set_type(CellType::LibraryReference);
        b.append_i8(2).unwrap();
        b.append_raw(hash.as_slice(), 256).unwrap();
        cell = b.clone().into_cell().unwrap();
        hash = cell.repr_hash();

        library.set(&hash, &SimpleLib::new(cell.clone(), false)).unwrap();
    }
    let library = library.inner();

    test_case_with_ref("NOP", cell.clone())
        .with_library(library.clone())
        .with_gas_limit(10000)
        .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref("NOP", cell.clone())
        .with_library(library)
        .with_gas_limit(100)
        .expect_failure(ExceptionCode::OutOfGas);
}
