/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::Serializable;

#[test]
fn test_slice_from_string() {
    assert_eq!(SliceData::from_string("0102_").unwrap(), SliceData::new(vec![1, 2]));
    assert_eq!(SliceData::from_string("0102").unwrap(), SliceData::new(vec![1, 2, 0x80]));
    assert_eq!(SliceData::from_string("012").unwrap(), SliceData::new(vec![1, 0x28]));
    assert_eq!(SliceData::from_string("012_").unwrap(), SliceData::new(vec![1, 0x20]));
    assert_eq!(SliceData::from_string("").unwrap(), SliceData::default());
}

fn check_git_bits(data: u16) {
    let vec = vec![(data >> 8) as u8, data as u8 | 1];
    let slice = SliceData::new(vec);
    for len in 1..=8 {
        for offset in 0..16 - len {
            let result = slice.get_bits(offset, len).unwrap();
            let check = ((data >> (16 - len - offset)) & ((1 << len) - 1)) as u8;
            println!(
                "offset: {}, len: {}, result: 0x{:X}, check: 0x{:X}",
                offset, len, result, check
            );
            assert_eq!(result, check);
        }
    }
}

#[test]
fn test_git_bits_simple() {
    check_git_bits(0xA853);
    check_git_bits(0x3720);
    check_git_bits(0x7342);
}

#[test]
fn compare_slices() {
    let mut slice1 = SliceData::new(vec![0xAA, 0x37, 0xA5, 0x55, 0x80]);
    let mut slice2 = SliceData::new(vec![0x37, 0xA5]);
    slice1.append_reference(SliceData::new(vec![0xFF])).unwrap();
    slice1.append_reference(SliceData::new_empty()).unwrap();
    slice2.append_reference(SliceData::new_empty()).unwrap();
    slice1.shrink_data(9..23);
    slice1.shrink_references(1..2);
    slice2.shrink_data(1..);
    assert_eq!(slice1, slice2);

    let mut slice1 = SliceData::new(vec![0xAA, 0x37, 0xA5, 0x55, 0x80]);
    let mut slice2 = slice1.clone();
    slice1.append_reference(SliceData::new(vec![0xFF])).unwrap();
    slice1.append_reference(SliceData::new_empty()).unwrap();
    slice2.append_reference(SliceData::new_empty()).unwrap();
    slice1.shrink_references(1..=1);
    assert_eq!(slice1, slice2);
}

#[test]
fn test_append_slice() {
    let mut slice1 = SliceData::new(vec![0x11, 0x22, 0x33, 0x80]);
    let mut slice2 = SliceData::new(vec![0x77, 0x88, 0x99, 0x80]);
    slice1.shrink_data(12..20);
    slice2.shrink_data(8..12);
    let mut builder = slice1.into_builder().unwrap();
    builder.checked_append_references_and_data(&slice2).unwrap();
    assert_eq!(builder.data(), &[0x23, 0x80]);
}

#[test]
fn test_common_prefix_equal_case() {
    let source = SliceData::new(vec![0x11, 0x22, 0x33, 0x80]);
    let (c, rem_1, rem_2) = SliceData::common_prefix(&source, &source);
    assert_eq!(c.unwrap(), source);
    assert!(rem_1.is_none() && rem_2.is_none());
}

#[test]
fn test_common_prefix_first_includes_second_case() {
    let mut slice1 = SliceData::new(vec![0x11, 0x22, 0x38]);
    let mut slice2 = SliceData::new(vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x80]);
    let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);

    assert_eq!(c.unwrap(), SliceData::new(vec![0x11, 0x22, 0x38]));
    assert_eq!(rem_1, None);
    assert_eq!(rem_2.unwrap(), SliceData::new(vec![0x34, 0x45, 0x58]));

    std::mem::swap(&mut slice1, &mut slice2);
    let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);
    assert_eq!(c.unwrap(), SliceData::new(vec![0x11, 0x22, 0x38]));
    assert_eq!(rem_1.unwrap(), SliceData::new(vec![0x34, 0x45, 0x58]));
    assert_eq!(rem_2, None);
}

#[test]
fn test_common_prefix_equal_case_but_len_not_divided_by_8() {
    let table = vec![
        vec![0x11, 0x22, 0xC0],
        vec![0x11, 0x22, 0xE0],
        vec![0x11, 0x22, 0xF0],
        vec![0x11, 0x22, 0xF8],
        vec![0x11, 0x22, 0xFC],
        vec![0x11, 0x22, 0xFE],
        vec![0x11, 0x22, 0xFF],
    ];
    for s in table {
        let source = SliceData::new(s.to_vec());
        let (c, rem_1, rem_2) = SliceData::common_prefix(&source.clone(), &source.clone());

        assert_eq!(c.unwrap(), source);
        assert!(rem_1.is_none() && rem_2.is_none());
    }
}

#[test]
fn test_common_prefix_general_case() {
    let table = vec![
        (
            vec![0x11, 0xFF, 0xC0],
            vec![0x11, 0x80, 0xFF, 0xC0],
            vec![0x11, 0xC0],
            vec![0xFF, 0x80],
            vec![0x01, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xE0],
            vec![0x11, 0xC0, 0xFF, 0xE0],
            vec![0x11, 0xE0],
            vec![0xFF, 0x80],
            vec![0x03, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xF0],
            vec![0x11, 0xE0, 0xFF, 0xF0],
            vec![0x11, 0xF0],
            vec![0xFF, 0x80],
            vec![0x07, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xF8],
            vec![0x11, 0xF0, 0xFF, 0xF8],
            vec![0x11, 0xF8],
            vec![0xFF, 0x80],
            vec![0x0F, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xFC],
            vec![0x11, 0xF8, 0xFF, 0xFC],
            vec![0x11, 0xFC],
            vec![0xFF, 0x80],
            vec![0x1F, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xFE],
            vec![0x11, 0xFC, 0xFF, 0xFE],
            vec![0x11, 0xFE],
            vec![0xFF, 0x80],
            vec![0x3F, 0xFF, 0x80],
        ),
        (
            vec![0x11, 0xFF, 0xFF],
            vec![0x11, 0xFE, 0xFF, 0xFF],
            vec![0x11, 0xFF],
            vec![0xFF, 0x80],
            vec![0x7F, 0xFF, 0x80],
        ),
    ];
    for v in table {
        let (a, b, prefix, expected_a, expected_b) = v.clone();
        let mut slice1 = SliceData::new(a);
        let mut slice2 = SliceData::new(b);
        let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);

        assert_eq!(c.unwrap(), SliceData::new(prefix.clone()));
        assert_eq!(rem_1.unwrap(), SliceData::new(expected_a.clone()));
        assert_eq!(rem_2.unwrap(), SliceData::new(expected_b.clone()));

        std::mem::swap(&mut slice1, &mut slice2);
        let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);
        assert_eq!(c.unwrap(), SliceData::new(prefix));
        assert_eq!(rem_1.unwrap(), SliceData::new(expected_b));
        assert_eq!(rem_2.unwrap(), SliceData::new(expected_a));
    }
}

#[test]
fn test_common_prefix_no_matches_case() {
    let mut slice1 = SliceData::new(vec![0x11, 0x22, 0x33, 0x80]);
    let mut slice2 = SliceData::new(vec![0x80, 0x22, 0x33, 0x44, 0x55, 0x80]);
    let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);

    assert_eq!(c, None);
    assert_eq!(rem_1.unwrap(), slice1);
    assert_eq!(rem_2.unwrap(), slice2);

    std::mem::swap(&mut slice1, &mut slice2);
    let (c, rem_1, rem_2) = SliceData::common_prefix(&slice1, &slice2);
    assert_eq!(c, None);
    assert_eq!(rem_1.unwrap(), slice1);
    assert_eq!(rem_2.unwrap(), slice2);
}

#[test]
fn test_common_prefix_empty_input_case() {
    let (c, rem_1, rem_2) =
        SliceData::common_prefix(&SliceData::new_empty(), &SliceData::new_empty());
    assert!(c.is_none() && rem_1.is_none() && rem_2.is_none());

    let (c, rem_1, rem_2) =
        SliceData::common_prefix(&SliceData::new(vec![0xFF]), &SliceData::new_empty());
    assert_eq!(rem_1.unwrap(), SliceData::new(vec![0xFF]));
    assert!(c.is_none() && rem_2.is_none());

    let (c, rem_1, rem_2) =
        SliceData::common_prefix(&SliceData::new_empty(), &SliceData::new(vec![0xFF]));
    assert_eq!(rem_2.unwrap(), SliceData::new(vec![0xFF]));
    assert!(c.is_none() && rem_1.is_none());
}

#[test]
fn test_is_full_cell_slice() {
    assert!(SliceData::default().is_full_cell_slice());
    let mut slice = SliceData::new(vec![0x11, 0x22, 0x33, 0x80]);
    assert!(slice.is_full_cell_slice());
    slice.append_reference(SliceData::default()).unwrap();
    assert!(slice.is_full_cell_slice());
    slice.get_next_byte().unwrap();
    assert!(!slice.is_full_cell_slice());
}

#[test]
fn test_overwrite_prefix() {
    let mut slice = SliceData::new(vec![0x11, 0x22, 0x33, 0x80]);
    let prefix = SliceData::from_raw(vec![0x12, 0x30], 8 + 4);
    let expected = SliceData::new(vec![0x12, 0x32, 0x33, 0x80]);
    slice.overwrite_prefix(&prefix).unwrap();
    assert_eq!(slice, expected);
}

#[test]
fn test_convert_slice_to_cell() {
    let builder = BuilderData::with_raw_and_refs(vec![0x57], 8, vec![Cell::default()]).unwrap();
    let cell = builder.into_cell().unwrap();
    let slice = SliceData::load_cell(cell.clone()).unwrap();
    assert_eq!(cell, slice.clone().into_cell().unwrap());
    let mut s = slice.clone();
    assert!(!s.get_next_bit().unwrap());
    assert_ne!(cell, s.into_cell().unwrap());

    let mut s = slice;
    assert_eq!(s.checked_drain_reference().unwrap(), Cell::default());
    assert_ne!(cell, s.into_cell().unwrap());
}

#[test]
fn test_empty_slice() {
    let slice = SliceData::new_empty();
    assert!(slice.is_empty_bitstring());
    assert!(slice.is_empty_cell());
    assert!(slice.is_none());
    assert!(slice.is_full_cell_slice());
}

#[test]
fn test_get_int_negative() {
    let mut slice = (-239i32).write_to_bitstring().unwrap();
    assert_eq!(slice.get_int(32).unwrap() as i32, -239);
    assert_eq!(slice.get_next_i32().unwrap(), -239);
}
