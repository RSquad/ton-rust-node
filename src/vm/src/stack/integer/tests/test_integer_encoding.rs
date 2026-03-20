/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::stack::{integer::IntegerData, BuilderData, SliceData};

#[test]
fn encoding_one_positive_byte() {
    let src = IntegerData::from_u32(99);

    let a = src.try_serialize(8, true, true).unwrap();
    let b = BuilderData::with_raw(vec![0b01100011], 8).unwrap();
    assert_eq!(a, b);

    let mut a = SliceData::load_builder(a).unwrap();
    let value = IntegerData::from_bytes(a.get_next_bits(8).unwrap(), 8, true, true).unwrap();
    assert_eq!(src, value);
}

#[test]
fn encoding_one_negative_byte() {
    let src = IntegerData::from_i32(-99);

    let a = src.try_serialize(8, true, true).unwrap();
    let b = BuilderData::with_raw(vec![0b10011101], 8).unwrap();
    assert_eq!(a, b);

    let mut a = SliceData::load_builder(a).unwrap();
    let value = IntegerData::from_bytes(a.get_next_bits(8).unwrap(), 8, true, true).unwrap();
    assert_eq!(src, value);
}

#[test]
fn encoding_two_positive_bytes() {
    let src = IntegerData::from_u32(99);

    let a = src.try_serialize(16, true, true).unwrap();
    let b = BuilderData::with_raw(vec![0b00000000, 0b01100011], 16).unwrap();
    assert_eq!(a, b);

    let mut a = SliceData::load_builder(a).unwrap();
    let value = IntegerData::from_bytes(a.get_next_bits(16).unwrap(), 16, true, true).unwrap();
    assert_eq!(src, value);
}

#[test]
fn encoding_two_negative_bytes() {
    let src = IntegerData::from_i32(-99);

    let a = src.try_serialize(16, true, true).unwrap();
    let b = BuilderData::with_raw(vec![0b11111111, 0b10011101], 16).unwrap();
    assert_eq!(a, b);

    let mut a = SliceData::load_builder(a).unwrap();
    let value = IntegerData::from_bytes(a.get_next_bits(16).unwrap(), 16, true, true).unwrap();
    assert_eq!(src, value);
}
