/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::stack::{integer::IntegerData, SliceData};

#[test]
fn test_signed_big_endian_ser_deser() {
    test_ser_deser(true, true);
}

#[test]
fn test_unsigned_big_endian_ser_deser() {
    test_ser_deser(false, true);
}

#[test]
fn test_signed_little_endian_ser_deser() {
    test_ser_deser(true, false);
}

#[test]
fn test_unsigned_little_endian_ser_deser() {
    test_ser_deser(false, false);
}

fn test_ser_deser(signed: bool, big_endian: bool) {
    let initial = IntegerData::from_str_radix("18AB_C0435ACE", 16).unwrap();

    let data = initial.try_serialize(46, signed, big_endian).unwrap();
    let mut data = SliceData::load_builder(data).unwrap();
    let resulted =
        IntegerData::from_bytes(data.get_next_bits(46).unwrap(), 46, signed, big_endian).unwrap();

    assert_eq!(initial, resulted);
}
