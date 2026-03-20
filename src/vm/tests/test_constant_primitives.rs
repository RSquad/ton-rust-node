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
use ton_block::{BuilderData, Cell, ExceptionCode, IBitstring, SliceData};
use ton_vm::{
    int,
    stack::{continuation::ContinuationData, integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

const PUSHINT1: [u8; 2] = [0x71, 0x80];
const PUSHINT2: [u8; 2] = [0x72, 0x80];
const PUSHINT3: [u8; 2] = [0x73, 0x80];
const PUSHINT4: [u8; 2] = [0x74, 0x80];

/*
• 8B08 — PUSHSLICE x8_, pushes an empty slice (bitstring ‘’).
• 8B04 — PUSHSLICE x4_, pushes bitstring ‘0’.
• 8B0C — PUSHSLICE xC_, pushes bitstring ‘1’.
*/
#[test]
fn push_empty_slice() {
    test_case(
        "
        PUSHSLICE x8_
    ",
    )
    .expect_bytecode(vec![0x8B, 0x08, 0x80]);
}

#[test]
fn push_slice_zero() {
    test_case(
        "
        PUSHSLICE x4_
    ",
    )
    .expect_bytecode(vec![0x8B, 0x04, 0x80]);
}

#[test]
fn push_slice_one() {
    test_case(
        "
        PUSHSLICE xC_
    ",
    )
    .expect_bytecode(vec![0x8B, 0x0C, 0x80]);
}

mod small_slice {
    use super::*;

    #[test]
    fn push_xf() {
        test_case(
            "
            PUSHSLICE xF
        ",
        )
        .expect_bytecode(vec![0x8B, 0x1F, 0x80, 0x80]);
    }

    #[test]
    fn push_xf8_() {
        test_case(
            "
            PUSHSLICE xF8_
        ",
        )
        .expect_bytecode(vec![0x8B, 0x1F, 0x80, 0x80]);
    }

    #[test]
    fn push_xff() {
        test_case(
            "
            PUSHSLICE xFF
        ",
        )
        .expect_bytecode(vec![0x8B, 0x1F, 0xF8, 0x80]);
    }

    #[test]
    fn push_xff8_() {
        test_case(
            "
            PUSHSLICE xFF8_
        ",
        )
        .expect_bytecode(vec![0x8B, 0x1F, 0xF8, 0x80]);
    }

    #[test]
    fn push_xfff() {
        test_case(
            "
            PUSHSLICE xFFF
        ",
        )
        .expect_bytecode(vec![0x8B, 0x2F, 0xFF, 0x80, 0x80]);
    }

    #[test]
    fn push_xfff_() {
        test_case(
            "
            PUSHSLICE xFFF_
        ",
        )
        .expect_bytecode(vec![0x8B, 0x1F, 0xFF, 0x80]);
    }

    #[test]
    fn push_max() {
        // 124 data bits
        test_case(
            "
            PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF8_
        ",
        )
        .expect_bytecode(vec![
            0x8B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xF8, 0x80,
        ]);
    }
}

#[test]
fn push_cont_from_first_ref_cc() {
    let slice = SliceData::new(PUSHINT1.to_vec());
    test_case_with_refs("PUSHREFCONT", vec![slice.clone().into_cell().unwrap()])
        .expect_item(StackItem::continuation(ContinuationData::with_code(slice)));
}

#[test]
fn push_cont_from_first_ref_cc_err_no_cell() {
    test_case("PUSHREFCONT").expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushref_success() {
    let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs("PUSHREF", vec![slice]).expect_item(create::cell(PUSHINT1));
}

#[test]
fn pushref_failure_on_cosecutive_calls_with_one_reference() {
    let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "PUSHREF
        PUSHREF",
        vec![slice],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushref_failure_after_pushrefcont() {
    let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "PUSHREFCONT
        PUSHREF",
        vec![slice],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushref_failure_without_reference() {
    test_case("PUSHREF").expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushrefslice_success() {
    let slice = SliceData::new(PUSHINT1.to_vec());
    test_case_with_refs("PUSHREFSLICE", vec![slice.clone().into_cell().unwrap()])
        .expect_item(StackItem::Slice(slice));
}

#[test]
fn pushrefslice_failure_on_cosecutive_calls() {
    let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "PUSHREFSLICE
        PUSHREFSLICE",
        vec![slice],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushrefslice_failure_after_pushrefcont() {
    let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "PUSHREFCONT
        PUSHREFSLICE",
        vec![slice],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushrefslice_failure_without_reference() {
    test_case("PUSHREFSLICE").expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn pushref_multiple_references() {
    let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
    let slice3 = SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap();
    let slice4 = SliceData::new(PUSHINT4.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "PUSHREF
        PUSHREFSLICE
        PUSHREFCONT
        PUSHREF",
        vec![slice1, slice2.clone(), slice3.clone(), slice4],
    )
    .expect_stack(
        Stack::new()
            .push(create::cell(PUSHINT1))
            .push(StackItem::Slice(SliceData::load_cell(slice2).unwrap()))
            .push_cont(ContinuationData::with_code(SliceData::load_cell(slice3).unwrap()))
            .push(create::cell(PUSHINT4)),
    );
}

fn composite_slice(prefix: Vec<u8>, body: SliceData, refs: &[SliceData]) -> Cell {
    let mut builder = BuilderData::with_bitstring(prefix).unwrap();
    builder.append_builder(&body.into_builder().unwrap()).unwrap();
    let remainder = builder.length_in_bits() % 8;
    if remainder != 0 {
        builder.append_bits(0, 8 - remainder).unwrap();
    }
    refs.iter().for_each(|r| {
        builder.checked_append_reference(r.clone().into_cell().unwrap()).unwrap();
    });
    builder.into_cell().unwrap()
}

#[test]
fn pushslice_with_refs() {
    let mut data = SliceData::new(vec![0xA5, 0xB6, 0xFE]); // simple cell with 2 bytes and 6 bits
    let pushint1 = SliceData::new(PUSHINT1.to_vec());
    let code = composite_slice(
        vec![0x8D, 0b0010_0000, 0b1010_0000], // PUSHSLICE 8D 1 - ref, 2 - bytes
        data.clone(),
        &[pushint1.clone()],
    );
    data.trim_right();
    data.append_reference(pushint1).unwrap();
    test_case_with_bytecode(code).expect_stack(Stack::new().push(StackItem::Slice(data)));
}

#[test]
fn pushslice_with_ref0() {
    let mut data = SliceData::new(vec![0xA5, 0xB6, 0xC0]); // simple cell with 2 bytes and 1 bit
    let pushint1 = SliceData::new(PUSHINT1.to_vec());
    let code = composite_slice(
        vec![0x8C, 0b0000_0101], // PUSHSLICE 8D 1 - ref, 2 - bytes
        data.clone(),
        &[pushint1.clone()],
    );
    data.trim_right();
    data.append_reference(pushint1).unwrap();
    test_case_with_bytecode(code).expect_stack(Stack::new().push(StackItem::Slice(data)));
}

#[test]
fn pushcont_with_refs() {
    let code = composite_slice(
        vec![0x8E, 0b1000_0001, 0x80], // PUSHSCONT 8F_ 1 - ref, 1 - byte
        SliceData::new(vec![0x72, 0x80]),
        &[
            SliceData::new(PUSHINT3.to_vec()),
            SliceData::new(vec![0x71, 0x01, 0xDE, 0x74, 0x80]), // PUSHINT 1 SWAP IF PUSHINT 4
        ],
    );
    test_case_with_bytecode(code)
        .expect_stack(Stack::new().push(int!(2)).push(int!(3)).push(int!(4)));
}

#[test]
fn test_strefconst() {
    let slice_internal = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    let builder = BuilderData::with_raw_and_refs(vec![], 0, vec![slice_internal.clone()]).unwrap();
    test_case_with_refs(
        "
        NEWC
        STREFCONST",
        vec![slice_internal],
    )
    .expect_item(StackItem::builder(builder));
}

fn bytecode_with_refs(bytecode: SliceData, references: &[Cell]) -> Cell {
    let mut builder = bytecode.into_builder().unwrap();
    for reference in references {
        builder.checked_append_reference(reference.clone()).unwrap();
    }
    builder.into_cell().unwrap()
}

#[test]
fn test_strefconst_variations() {
    let references = vec![
        SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap(),
        SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap(),
        SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap(),
    ];

    let strefconst = test_case_with_refs(
        "
        NEWC
        STREFCONST",
        references.clone(),
    )
    .expect_success();

    let strefconst_by_sliceconst = test_case_with_bytecode(bytecode_with_refs(
        SliceData::new(vec![
            0xC8, // NEWC
            0xCF, 0xA2, // STREFCONST (using STSLICECONST)
            0x80,
        ]),
        &references,
    ))
    .expect_success();

    strefconst.expect_same_results(strefconst_by_sliceconst);

    let strefconst_x2 = test_case_with_refs(
        "
        NEWC
        STREFCONST
        STREFCONST",
        references.clone(),
    );

    let stref2const = test_case_with_refs(
        "
        NEWC
        STREF2CONST",
        references.clone(),
    );

    strefconst_x2.expect_same_results(stref2const);

    let strefconst_x3 = test_case_with_refs(
        "
        NEWC
        STREFCONST
        STREFCONST
        STREFCONST",
        references.clone(),
    );

    let stref3const = test_case_with_refs(
        "
        NEWC
        STREF3CONST",
        references.clone(),
    );

    strefconst_x3.expect_same_results(stref3const);

    let stref2const = test_case_with_refs(
        "
        NEWC
        STREF2CONST",
        references.clone(),
    )
    .expect_success();

    let stref2const_by_sliceconst = test_case_with_bytecode(bytecode_with_refs(
        SliceData::new(vec![
            0xC8, // NEWC
            0xCF, 0xC2, // STREF2CONST (using STSLICECONST)
            0x80,
        ]),
        &references,
    ))
    .expect_success();

    stref2const.expect_same_results(stref2const_by_sliceconst);

    let stref3const_by_sliceconst = test_case_with_bytecode(bytecode_with_refs(
        SliceData::new(vec![
            0xC8, // NEWC
            0xCF, 0xE2, // STREF3CONST (using STSLICECONST)
            0x80,
        ]),
        &references,
    ));

    let stref3const = test_case_with_refs(
        "
        NEWC
        STREF3CONST",
        references,
    );

    stref3const.expect_same_results(stref3const_by_sliceconst);
}

#[test]
fn neg_missed_part_of_args_2() {
    let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "
        NEWC
        STREF3CONST
    ",
        vec![slice1, slice2],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}

#[test]
fn neg_missed_part_of_arg_1() {
    let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
    test_case_with_refs(
        "
        NEWC
        STREF2CONST
    ",
        vec![slice1],
    )
    .expect_failure(ExceptionCode::InvalidOpcode);
}
