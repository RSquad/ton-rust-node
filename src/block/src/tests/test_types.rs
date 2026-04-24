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
use crate::{
    base64_decode, base64_encode,
    cell::{Cell, CellType},
    ed25519_generate_private_key, ed25519_sign_with_secret, write_read_and_assert,
    Ed25519KeyOption,
};
use num::{CheckedAdd, CheckedSub};

#[test]
fn test_uint256_formatting() {
    let value =
        UInt256::from_str("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")
            .unwrap();
    assert_eq!(
        value.to_string(),
        "UInt256[[\
            12, 34, 56, 78, 90, AB, CD, EF, 12, 34, 56, 78, 90, AB, CD, EF, \
            12, 34, 56, 78, 90, AB, CD, EF, 12, 34, 56, 78, 90, AB, CD, EF\
        ]]"
    );
    assert_eq!(
        format!("{:?}", value),
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
    );
    assert_eq!(
        format!("{:x}", value),
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
    );
    assert_eq!(
        format!("{:#x}", value),
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
    );
    assert_eq!(
        format!("{:#X}", value),
        "0x1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF"
    );
}

#[test]
fn test_uint256_construct() {
    assert_eq!(
        UInt256::from_le_bytes(&0x0123456789ABCDEFu64.to_be_bytes()),
        UInt256::from_str("0123456789ABCDEF000000000000000000000000000000000000000000000000")
            .unwrap()
    );
    assert_eq!(
        UInt256::from_be_bytes(&0x0123456789ABCDEFu64.to_be_bytes()),
        UInt256::from_str("0000000000000000000000000000000000000000000000000123456789ABCDEF")
            .unwrap()
    );
    assert_eq!(
        UInt256::from_le_bytes(&0x0123456789ABCDEFu64.to_le_bytes()),
        UInt256::from_str("EFCDAB8967452301000000000000000000000000000000000000000000000000")
            .unwrap()
    );
    assert_eq!(
        UInt256::from_be_bytes(&0x0123456789ABCDEFu64.to_le_bytes()),
        UInt256::from_str("000000000000000000000000000000000000000000000000EFCDAB8967452301")
            .unwrap()
    );
    assert_eq!(
        UInt256::from_le_bytes(&[1, 2, 3]),
        UInt256::from_str("0102030000000000000000000000000000000000000000000000000000000000")
            .unwrap()
    );
    assert_eq!(
        UInt256::from_be_bytes(&[1, 2, 3]),
        UInt256::from_str("0000000000000000000000000000000000000000000000000000000000010203")
            .unwrap()
    );
}

#[test]
fn test_uint256_ordering() {
    assert!(
        UInt256::from_str("b5fb2792ecc96042d5f2f739c0a2586896c60719d1d8ad34f9d5f7ff578ffd89")
            .unwrap()
            < UInt256::from_str("de48d8a9c6823c908cbf72c42f60d993424e4ac5298a16c6b811c9876b366827")
                .unwrap()
    );
    assert!(
        UInt256::from_str("de48d8a9c6823c908cbf72c42f60d993424e4ac5298a16c6b811c9876b366827")
            .unwrap()
            > UInt256::from_str("15de0c10aaed5c7b9cdef181fd1b00abb8890ea5a1b86c961d7125e00c114691")
                .unwrap()
    );
}

#[test]
fn test_check_cell_types() {
    fn construct(cells: Vec<Cell>, cell_type: CellType, len: usize) -> Result<Cell> {
        assert!(len > 1);
        let mut data = vec![0x80; len];
        data[0] = cell_type.into();
        let raw = Cell::build_data(&data, cell_type, 0, cells.len(), None)?;
        Cell::with_data_and_refs(&raw, false, &cells, None, None)
    }

    construct(vec![], CellType::LibraryReference, 2)
        .expect_err("LibraryReference cell should be checked for 264 bits length");
    construct(vec![], CellType::LibraryReference, 35)
        .expect_err("LibraryReference cell should be checked for 264 bits length");
    construct(vec![Cell::default()], CellType::LibraryReference, 34)
        .expect_err("LibraryReference cell should be checked for no references");
    construct(vec![], CellType::LibraryReference, 34).unwrap();

    construct(vec![], CellType::MerkleProof, 2)
        .expect_err("MerkleProof cell should be checked for 280 bits length");
    construct(vec![], CellType::MerkleProof, 37)
        .expect_err("MerkleProof cell should be checked for 280 bits length");
    construct(vec![], CellType::MerkleProof, 36)
        .expect_err("MerkleProof cell should be checked for single reference");
    construct(vec![Cell::default(); 2], CellType::MerkleProof, 36)
        .expect_err("MerkleProof cell should be checked for single reference");
    construct(vec![Cell::default()], CellType::MerkleProof, 36).unwrap();

    construct(vec![], CellType::MerkleUpdate, 2)
        .expect_err("MerkleUpdate cell should be checked for 552 bits length");
    construct(vec![], CellType::MerkleUpdate, 71)
        .expect_err("MerkleUpdate cell should be checked for 552 bits length");
    construct(vec![], CellType::MerkleUpdate, 70)
        .expect_err("MerkleUpdate cell should be checked for two references");
    construct(vec![Cell::default()], CellType::MerkleUpdate, 70)
        .expect_err("MerkleUpdate cell should be checked for two references");
    construct(vec![Cell::default(); 2], CellType::MerkleUpdate, 70).unwrap();
}

#[test]
fn test_parse_int256() {
    let b64_without_pad = "GfgI79Xf3q7r4q1SPz7wAqBt0W6CjavuADODoz/DQE8";
    let b64 = "GfgI79Xf3q7r4q1SPz7wAqBt0W6CjavuADODoz/DQE8=";
    let hex = "19F808EFD5DFDEAEEBE2AD523F3EF002A06DD16E828DABEE003383A33FC3404F";
    assert_eq!(43, b64_without_pad.len());
    assert_eq!(44, b64.len());

    let ethalon = hex::decode(hex).unwrap();
    assert_eq!(32, ethalon.len());
    assert_eq!(b64, &base64_encode(&ethalon));
    assert_eq!(base64_decode(b64_without_pad).unwrap(), ethalon);
    assert_eq!(base64_decode(b64).unwrap(), ethalon);

    let hex_hash = hex.parse::<UInt256>().unwrap();
    assert_eq!(hex_hash, b64.parse::<UInt256>().unwrap());
    b64_without_pad.parse::<UInt256>().expect_err("we use only canonical padding base64");
}

#[test]
fn test_shared_secret() {
    let alice = Ed25519KeyOption::generate().unwrap();
    let bob = Ed25519KeyOption::generate().unwrap();
    let shared_secret = alice.shared_secret(bob.pub_key().unwrap()).unwrap();
    assert_eq!(shared_secret, bob.shared_secret(alice.pub_key().unwrap()).unwrap());
}

#[rustfmt::skip]
#[test]
fn test_get_bytestring() {

    let mut slice = SliceData::from_raw( vec![0b10110111, 0b01111011, 0b11101111, 0b10111111], 32);
    assert_eq!(slice.get_bytestring( 0), vec![0b10110111, 0b01111011, 0b11101111, 0b10111111]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01101110, 0b11110111, 0b11011111, 0b01111110]);
    assert_eq!(slice.get_bytestring( 2), vec![0b11011101, 0b11101111, 0b10111110, 0b11111100]);
    assert_eq!(slice.get_bytestring( 3), vec![0b10111011, 0b11011111, 0b01111101, 0b11111000]);
    assert_eq!(slice.get_bytestring( 7), vec![0b10111101, 0b11110111, 0b11011111, 0b10000000]);
    assert_eq!(slice.get_bytestring( 8), vec![0b01111011, 0b11101111, 0b10111111]);
    assert_eq!(slice.get_bytestring( 9), vec![0b11110111, 0b11011111, 0b01111110]);
    assert_eq!(slice.get_bytestring(10), vec![0b11101111, 0b10111110, 0b11111100]);
    assert_eq!(slice.get_bytestring(24), vec![0b10111111]);
    assert_eq!(slice.get_bytestring(25), vec![0b01111110]);
    assert_eq!(slice.get_bytestring(26), vec![0b11111100]);
    assert_eq!(slice.get_bytestring(31), vec![0b10000000]);
    assert!(slice.get_bytestring(32).is_empty());
    assert!(slice.get_bytestring(33).is_empty());

    slice.move_by(1).unwrap();
    assert_eq!(slice.get_bytestring( 0), vec![0b01101110, 0b11110111, 0b11011111, 0b01111110]);
    assert_eq!(slice.get_bytestring( 1), vec![0b11011101, 0b11101111, 0b10111110, 0b11111100]);
    assert_eq!(slice.get_bytestring(25), vec![0b11111100]);
    assert_eq!(slice.get_bytestring(30), vec![0b10000000]);
    assert!(slice.get_bytestring(31).is_empty());

    let mut slice = SliceData::from_raw( vec![0b10110111, 0b01111011, 0b11101111, 0b10111111], 32);
    slice.shrink_data(0..=30);
    assert_eq!(slice.get_bytestring( 0), vec![0b10110111, 0b01111011, 0b11101111, 0b10111110]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01101110, 0b11110111, 0b11011111, 0b01111100]);
    assert_eq!(slice.get_bytestring(25), vec![0b01111100]);
    assert_eq!(slice.get_bytestring(30), vec![0b10000000]);
    assert!(slice.get_bytestring(31).is_empty());

    let mut slice = SliceData::from_raw( vec![0b10110111, 0b01111011, 0b11101111, 0b10111111], 32);
    slice.shrink_data(0..=29);
    assert_eq!(slice.get_bytestring( 0), vec![0b10110111, 0b01111011, 0b11101111, 0b10111100]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01101110, 0b11110111, 0b11011111, 0b01111000]);
    assert_eq!(slice.get_bytestring(25), vec![0b01111000]);
    assert_eq!(slice.get_bytestring(29), vec![0b10000000]);
    assert!(slice.get_bytestring(30).is_empty());

    let mut slice = SliceData::from_raw( vec![0b10110111, 0b01111011, 0b11101111, 0b10111111], 32);
    slice.shrink_data(0..=23);
    assert_eq!(slice.get_bytestring( 0), vec![0b10110111, 0b01111011, 0b11101111]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01101110, 0b11110111, 0b11011110]);
    assert_eq!(slice.get_bytestring(23), vec![0b10000000]);
    assert!(slice.get_bytestring(24).is_empty());

    let mut slice = SliceData::from_raw( vec![0b10110111, 0b01111011, 0b11101111, 0b10111111], 32);
    slice.shrink_data(0..=21);
    assert_eq!(slice.get_bytestring( 0), vec![0b10110111, 0b01111011, 0b11101100]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01101110, 0b11110111, 0b11011000]);
    assert_eq!(slice.get_bytestring(21), vec![0b10000000]);
    assert!(slice.get_bytestring(22).is_empty());

    slice.move_by(6).unwrap();
    assert_eq!(slice.get_bytestring( 0), vec![0b11011110, 0b11111011]);
    assert_eq!(slice.get_bytestring( 1), vec![0b10111101, 0b11110110]);
    assert_eq!(slice.get_bytestring(14), vec![0b11000000]);
    assert_eq!(slice.get_bytestring(15), vec![0b10000000]);

    slice.move_by(1).unwrap();
    assert_eq!(slice.get_bytestring( 0), vec![0b10111101, 0b11110110]);
    assert_eq!(slice.get_bytestring( 1), vec![0b01111011, 0b11101100]);
    assert_eq!(slice.get_bytestring(14), vec![0b10000000]);
    assert!(slice.get_bytestring(15).is_empty());

}

#[test]
fn test_ed25519_signing() {
    let data = [1, 2, 3];
    let secret_key = ed25519_generate_private_key().unwrap();
    let signature1 = secret_key.sign(&data);
    let key = Ed25519KeyOption::from_private_key(secret_key.as_bytes()).unwrap();
    let signature2 = key.sign(&data).unwrap();
    assert_eq!(&signature1, signature2.as_slice());
    let signature3 = ed25519_sign_with_secret(secret_key.as_bytes(), &data).unwrap();
    assert_eq!(signature1, signature3);
}

#[test]
fn test_get_len() {
    let size = VarUInteger32::get_len(&0u32.into());
    assert_eq!(size, 0);
    let value = BigInt::from_slice(Sign::Plus, &[1, 2, 3, 4, 5, 6, 7, 0xFFFFFFFF]);
    let size = VarUInteger32::get_len(&value);
    assert_eq!(size, 32);
    let size = VarUInteger32::get_len(&BigInt::from_slice(Sign::Plus, &[1, 2, 3, 4, 5, 6, 7]));
    assert_eq!(size, 25);
    let size = VarUInteger32::get_len(&1u32.into());
    assert_eq!(size, 1);
    let size = VarUInteger32::get_len(&BigInt::from_slice(Sign::Plus, &[u32::MAX; 8]));
    assert_eq!(size, 32);
}

#[test]
fn test_varuinteger_with_zero() {
    let vui32: VarUInteger32 = VarUInteger32::default();
    let b = vui32.serialize().unwrap();
    let mut s = SliceData::new(vec![0b00000100]);
    assert_eq!(&s.cell().unwrap(), &b);
    let mut v2: VarUInteger32 = VarUInteger32::default();
    v2.read_from(&mut s).unwrap();
    assert_eq!(vui32, v2);
}

#[test]
fn test_varuinteger7_from_into() {
    let mut b1: SliceData = SliceData::new(vec![0b00100000, 0b01010000]);
    println!("b1 = {}", b1);
    let mut vui7: VarUInteger7 = VarUInteger7::default();
    vui7.read_from(&mut b1).unwrap();
    println!("vui7 = {}", vui7);
    assert_eq!(VarUInteger7::from(2), vui7);

    let mut b2 = SliceData::new(vec![0b00100010, 0b00000100, 0b01000100, 0b00000001]);
    let mut v2 = VarUInteger7::default();
    v2.read_from(&mut b2).unwrap();
    let mut v3 = VarUInteger7::default();
    v3.read_from(&mut b2).unwrap();
    v2 += 1;
    assert_eq!(v2, v3);

    let mut s1: BuilderData = BuilderData::new();
    v2.write_to(&mut s1).unwrap();
    println!("s1 = {}", s1);
    println!("v2 = {}", v2);
}

#[test]
fn test_varuinteger7_serialization() {
    VarUInteger7::try_from(u64::MAX).expect_err("should not be contructable");
    VarUInteger7::try_from(0x01FF_FFFF_FFFF_FFFFu64).expect_err("should not be contructable");
    let v = VarUInteger7::try_from(0x00FF_FFFF_FFFF_FFFFu64).unwrap();
    v.serialize().unwrap();

    let mut v = VarUInteger7::default();
    v.read_from(&mut SliceData::new(vec![0b00100000, 0b01000001])).unwrap();
    assert_eq!(VarUInteger7::from(2), v);
    v.read_from(&mut SliceData::new(vec![0b00111111, 0b11100001])).unwrap();
    assert_eq!(VarUInteger7::from(255), v);
    v.read_from(&mut SliceData::new(vec![0b00010000])).unwrap();
    assert_eq!(VarUInteger7::from(0), v);
    v.read_from(&mut SliceData::new(vec![0b01011111, 0b11111111, 0b11100001])).unwrap();
    assert_eq!(VarUInteger7::from(65535), v);
    v.read_from(&mut SliceData::new(vec![
        0b11011111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11100001,
    ]))
    .unwrap();
    assert_eq!(VarUInteger7::try_from((1u64 << 48) - 1).unwrap(), v);
}

#[test]
fn test_varuinteger32_serialization() {
    let mut g = VarUInteger32::default();
    g.read_from(&mut SliceData::new(vec![
        0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11111001,
    ]))
    .unwrap();
    assert_eq!(VarUInteger32::from_two_u128(0, (1 << 56) - 1).unwrap(), g);

    let g1 = VarUInteger32::from_two_u128(
        0x00800000000000000000000000000000,
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF,
    )
    .unwrap();
    write_read_and_assert(g1.clone());

    g.read_from(&mut SliceData::new(vec![
        0xFC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x07, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFC,
    ]))
    .unwrap();
    assert_eq!(g1, g);
}

#[test]
fn test_var_uinteger_32_addition() {
    let mut a = VarUInteger32::from_two_u128(0, 0).unwrap();
    let b = VarUInteger32::from_two_u128(0, 1).unwrap();
    a.add(&b).unwrap();
    assert_eq!(a, b);

    let mut a = VarUInteger32::from_two_u128(123, 567).unwrap();
    let b = VarUInteger32::from_two_u128(876, 432).unwrap();
    a.add(&b).unwrap();
    assert_eq!(a, VarUInteger32::from_two_u128(999, 999).unwrap());

    let mut a = VarUInteger32::from_two_u128(0, 1).unwrap();
    let b = VarUInteger32::from_two_u128(0, 1).unwrap();
    a.sub(&b).unwrap();
    assert_eq!(a, VarUInteger32::from_two_u128(0, 0).unwrap());
}

#[test]
fn test_number5_serialization() {
    let mut v = Number5::default();

    v.read_from(&mut SliceData::new(vec![0b00000100])).unwrap();
    assert_eq!(Number5::new(0).unwrap(), v);
    v.read_from(&mut SliceData::new(vec![0b00001100])).unwrap();
    assert_eq!(Number5::new(1).unwrap(), v);
    v.read_from(&mut SliceData::new(vec![0b10000100])).unwrap();
    assert_eq!(Number5::new(16).unwrap(), v);
    v.read_from(&mut SliceData::new(vec![0b11111100])).unwrap();
    assert_eq!(Number5::new(31).unwrap(), v);
    v.read_from(&mut SliceData::new(vec![0b10000100])).unwrap();
    assert_eq!(Number5::new(16).unwrap(), v);
    write_read_and_assert(v);
}

#[test]
fn test_number32_serialization() {
    let mut v = Number32::default();

    v.read_from(&mut SliceData::new(vec![
        0b00000000, 0b00000000, 0b00000000, 0b00000000, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(0u32), v);
    v.read_from(&mut SliceData::new(vec![
        0b00000000, 0b00000000, 0b00000000, 0b00000001, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(1u32), v);
    v.read_from(&mut SliceData::new(vec![
        0b00000000, 0b00000000, 0b00000000, 0b00010000, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(16u32), v);
    v.read_from(&mut SliceData::new(vec![
        0b11111111, 0b00000000, 0b00000000, 0b00000000, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(0xFF000000u32), v);
    v.read_from(&mut SliceData::new(vec![
        0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(0xFFFFFFFFu32), v);
    v.read_from(&mut SliceData::new(vec![
        0b00000000, 0b00000000, 0b00000000, 0b00011111, 0b10000000,
    ]))
    .unwrap();
    assert_eq!(Number32::from(31u32), v);
    write_read_and_assert(v);
}

#[test]
fn test_coins_serialization() {
    write_read_and_assert(Coins::try_from(u64::MAX as u128).unwrap());

    let g = Coins::try_from(956_956_956_956_000_000_000u128).unwrap();
    let s = g.write_to_new_cell().unwrap();
    assert_eq!(s.data(), hex::decode("933e072122d1d2818000").unwrap());
    assert_eq!(g, Coins::construct_from_cell(s.into_cell().unwrap()).unwrap());

    let mut g = Coins::zero();
    g.read_from(&mut SliceData::new(vec![0b00010000, 0b000101000])).unwrap();
    assert_eq!(Coins::from(2), g);
    g.read_from(&mut SliceData::new(vec![0b00011111, 0b11110001])).unwrap();
    assert_eq!(Coins::from(255), g);
    g.read_from(&mut SliceData::new(vec![0b00001000])).unwrap();
    assert_eq!(Coins::zero(), g);
    g.read_from(&mut SliceData::new(vec![0b00101111, 0b11111111, 0b11110001])).unwrap();
    assert_eq!(Coins::from(65535), g);
    g.read_from(&mut SliceData::new(vec![
        0b01111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110001,
    ]))
    .unwrap();
    assert_eq!(Coins::try_from((1u128 << 56) - 1).unwrap(), g);

    let s = Coins::from(2).write_to_new_cell().unwrap();
    assert_eq!(SliceData::load_builder(s).unwrap(), SliceData::new(vec![0b00010000, 0b00101000]));

    let s = Coins::from(252).write_to_new_cell().unwrap();
    assert_eq!(SliceData::load_builder(s).unwrap(), SliceData::new(vec![0b00011111, 0b11001000]));

    let s = Coins::zero().write_to_new_cell().unwrap();
    assert_eq!(SliceData::load_builder(s).unwrap(), SliceData::new(vec![0b00001000]));

    let s = Coins::from(65534).write_to_new_cell().unwrap();
    assert_eq!(
        SliceData::load_builder(s).unwrap(),
        SliceData::new(vec![0b00101111, 0b11111111, 0b11101000])
    );

    let s = Coins::from(0xFFFFFFFFFFFFFE).write_to_new_cell().unwrap();
    assert_eq!(
        SliceData::load_builder(s).unwrap(),
        SliceData::new(vec![
            0b01111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
            0b11101000
        ])
    );

    for n in 0..1000 {
        write_read_and_assert(Coins::from(n));
    }
    for n in 1000000000..1000001000 {
        write_read_and_assert(Coins::from(n));
    }
    for n in 1000000000000000..1000000000001000 {
        write_read_and_assert(Coins::from(n));
    }
}

define_HashmapE! {SimpleMap, 8, u8}

#[test]
fn test_filter() {
    let mut tree_1 = SimpleMap::default();
    tree_1.set(&0b11001100u8, &0).unwrap();
    tree_1.set(&0b11010000u8, &1).unwrap();
    tree_1.set(&0b11010100u8, &0).unwrap();
    tree_1.set(&0b11011000u8, &3).unwrap();
    tree_1.set(&0b11011100u8, &0).unwrap();
    tree_1.retire(|value| value != 0).unwrap();

    let mut tree_2 = SimpleMap::default();
    tree_2.set(&0b11010000u8, &1).unwrap();
    tree_2.set(&0b11011000u8, &3).unwrap();

    let correct_dif: Vec<(SliceData, Option<u8>, Option<u8>)> = Vec::new();
    let mut diff_vec: Vec<(SliceData, Option<u8>, Option<u8>)> = Vec::new();
    tree_1
        .scan_diff(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    assert!(correct_dif == diff_vec);
}

#[test]
fn test_coins_parsing() {
    let g = Coins::from_str("0xffffffffffffffffffffffffffffffff").unwrap();
    assert_eq!(g.0, 340282366920938463463374607431768211455u128);
    Coins::from_str("0x100000000000000000000000000000000").unwrap_err();
    Coins::from_str("340282366920938463463374607431768211455").unwrap();
    assert_eq!(g.0, 340282366920938463463374607431768211455u128);
    Coins::from_str("340282366920938463463374607431768211456").unwrap_err();
}

#[test]
fn test_checked_operations() {
    let mut v = VarUInteger7::try_from(0x00FF_FFFF_FFFF_FFFFu64).unwrap();
    let one = VarUInteger7::one();
    assert!(!v.add_checked(&one));
    assert!(v.sub_checked(&one));
    assert!(v.add_checked(&one));

    let mut v = VarUInteger3::try_from(0x00FF_FFFFu32).unwrap();
    let one = VarUInteger3::one();
    assert!(!v.add_checked(&one));
    assert!(v.sub_checked(&one));
    assert!(v.add_checked(&one));

    let mut v = Coins::try_from(0x00FF_FFFF_FFFF_FFFF_FFFF_FFFF_FFFF_FFFFu128).unwrap();
    let one = Coins::one();
    assert!(!v.add_checked(&one));
    assert!(v.sub_checked(&one));
    assert!(v.add_checked(&one));
}

#[test]
fn test_math_traits() {
    let mut a = Coins::from(10);
    a *= 10;
    a *= Coins::from(10);
    a <<= 3;
    let mut b = ((a >> 2) << 1) + 5;
    b += 1;
    b -= 3;
    assert_eq!(b.as_u128(), (1000 << 3 >> 2 << 1) + 5 + 1 - 3);

    let mut a = Coins::try_from((1u128 << 120) - 1).unwrap();
    assert!(!a.add_checked(&Coins::one()), "should not fit in Coins");
    assert!(a.checked_add(&Coins::one()).is_none(), "should not fit in Coins");

    let mut a = Coins::zero();
    assert!(!a.sub_checked(&Coins::one()), "should not sub with negative");
    assert!(a.checked_sub(&Coins::one()).is_none(), "should not sub with negative");
}
