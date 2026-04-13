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
use super::*;

#[test]
fn test_out_action_create() {
    let out_msg = Message::default();
    let action_send = OutAction::new_send(0, out_msg.clone());
    assert_eq!(action_send, OutAction::SendMsg { mode: 0, out_msg });
    let new_code = Cell::default();
    let action_set = OutAction::new_set(new_code.clone());
    assert_eq!(action_set, OutAction::SetCode { new_code });
}

fn test_action_serde_equality(action: OutAction) {
    let action_cell = action.serialize().unwrap();
    let deser_action = OutAction::construct_from_cell(action_cell).unwrap();
    assert_eq!(action, deser_action);
}

#[test]
fn test_sendmsg_action_serde() {
    test_action_serde_equality(OutAction::new_send(SENDMSG_ORDINARY, Message::default()));
    test_action_serde_equality(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, Message::default()));
    test_action_serde_equality(OutAction::new_send(SENDMSG_ALL_BALANCE, Message::default()));
}

#[test]
fn test_setcode_action_serde() {
    let code = Cell::default();
    test_action_serde_equality(OutAction::new_set(code));
}

#[test]
fn test_reserve_action_serde() {
    test_action_serde_equality(OutAction::new_reserve(
        RESERVE_EXACTLY,
        CurrencyCollection::with_coins(12345),
    ));
    test_action_serde_equality(OutAction::new_reserve(
        RESERVE_EXACTLY | RESERVE_IGNORE_ERROR,
        CurrencyCollection::with_coins(54321),
    ));
}

fn get_out_actions() -> OutActions {
    let code = SliceData::new(vec![0x71, 0x80]).into_cell().unwrap();
    let msg = Message::default();
    let mut oa = OutActions::new();
    oa.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg.clone()));
    oa.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, msg.clone()));
    oa.push_back(OutAction::new_send(SENDMSG_IGNORE_ERROR, msg));
    oa.push_back(OutAction::new_set(Cell::default()));
    oa.push_back(OutAction::new_set(Cell::default()));
    oa.push_back(OutAction::new_set(Cell::default()));
    oa.push_back(OutAction::new_reserve(RESERVE_EXACTLY, CurrencyCollection::with_coins(12345678)));
    oa.push_back(OutAction::new_reserve(RESERVE_ALL_BUT, CurrencyCollection::with_coins(87654321)));
    oa.push_back(OutAction::new_change_library(CHANGE_LIB_MODE, None, Some(code.repr_hash())));
    oa.push_back(OutAction::new_change_library(SET_LIB_CODE_MODE, Some(code), None));
    oa
}

#[test]
fn test_outactions() {
    let oa = get_out_actions();
    assert_eq!(oa.len(), 10);
    for a in oa.iter() {
        println!("action {:?}", a);
    }
}

#[test]
fn test_outactions_serialization() {
    let oa = get_out_actions();
    let b = oa.serialize().unwrap();
    let mut s = SliceData::load_cell(b).unwrap();

    println!("action send slice: {}", s);

    let mut oa_restored = OutActions::new();
    oa_restored.read_from(&mut s).unwrap();

    for a in oa_restored.iter() {
        println!("action {:?}", a);
    }
    assert_eq!(oa, oa_restored);
}

#[test]
fn test_unpack_out_action_slices_valid_list() {
    let mut actions = OutActions::new();
    actions.push_back(OutAction::new_set(Cell::default()));
    actions.push_back(OutAction::new_reserve(RESERVE_EXACTLY, CurrencyCollection::with_coins(1)));

    let actions_cell = actions.serialize().unwrap();
    let slices = unpack_out_action_slices(SliceData::load_cell(actions_cell).unwrap()).unwrap();
    assert_eq!(slices.len(), 2);

    let mut s0 = slices[0].clone();
    let mut s1 = slices[1].clone();
    let a0 = OutAction::construct_from(&mut s0).unwrap();
    let a1 = OutAction::construct_from(&mut s1).unwrap();

    assert!(matches!(a0, OutAction::SetCode { .. }));
    assert!(matches!(a1, OutAction::ReserveCurrency { .. }));
}

#[test]
fn test_unpack_out_action_slices_rejects_non_empty_tail() {
    let mut tail_builder = BuilderData::new();
    tail_builder.append_bit_one().unwrap();
    let tail = tail_builder.into_cell().unwrap();
    let mut root = BuilderData::new();
    root.checked_append_reference(tail).unwrap();
    OutAction::new_set(Cell::default()).write_to(&mut root).unwrap();
    let actions_cell = root.into_cell().unwrap();

    assert!(unpack_out_action_slices(SliceData::load_cell(actions_cell).unwrap()).is_err());
}

#[test]
fn test_deserialize_out_action_slices_valid_list() {
    let actions = get_out_actions();
    let slice = SliceData::load_cell(actions.serialize().unwrap()).unwrap();
    let slices = unpack_out_action_slices(slice).unwrap();
    assert_eq!(slices.len(), actions.len());
    for (expected, mut slice) in actions.into_iter().zip(slices.into_iter()) {
        let actual = OutAction::construct_from(&mut slice).unwrap();
        assert_eq!(expected, actual);
    }
}

/// Non-canonical Grams in a SendMsg action must trigger OutActionError
#[test]
fn test_sendmsg_non_canonical_coins_gives_out_action_error() {
    let append_addr_std = |b: &mut BuilderData| {
        b.append_bits(0b10, 2).unwrap();
        b.append_bit_zero().unwrap();
        b.append_bits(0, 8).unwrap();
        for _ in 0..4 {
            b.append_u64(0).unwrap();
        }
    };

    let build_msg_cell = |coins_len: usize, coins_bytes: &[u8]| -> Cell {
        let mut b = BuilderData::new();
        b.append_bit_zero().unwrap();
        b.append_bit_one().unwrap();
        b.append_bit_zero().unwrap();
        b.append_bit_zero().unwrap();
        append_addr_std(&mut b);
        append_addr_std(&mut b);
        b.append_bits(coins_len, 4).unwrap();
        b.append_raw(coins_bytes, coins_len * 8).unwrap();
        b.append_bit_zero().unwrap();
        b.append_bits(0, 4).unwrap();
        b.append_bits(0, 4).unwrap();
        b.append_u64(0).unwrap();
        b.append_u32(0).unwrap();
        b.append_bit_zero().unwrap();
        b.append_bit_zero().unwrap();
        b.into_cell().unwrap()
    };

    let build_action_slice = |msg_cell: Cell, mode: u8| -> SliceData {
        let mut b = BuilderData::new();
        ACTION_SEND_MSG.write_to(&mut b).unwrap();
        mode.write_to(&mut b).unwrap();
        b.checked_append_reference(msg_cell).unwrap();
        SliceData::load_cell(b.into_cell().unwrap()).unwrap()
    };

    // Canonical: 1 TON = 0x3B9ACA00 in 4 bytes -> parses fine
    let msg_cell = build_msg_cell(4, &[0x3B, 0x9A, 0xCA, 0x00]);
    let mut slice = build_action_slice(msg_cell, 0);
    OutAction::skip(&mut slice.clone()).unwrap();
    let action = OutAction::construct_from(&mut slice).unwrap();
    assert!(matches!(action, OutAction::SendMsg { mode: 0, .. }));

    // Non-canonical: same value but len=5 with leading 0x00 -> OutActionError
    let msg_cell = build_msg_cell(5, &[0x00, 0x3B, 0x9A, 0xCA, 0x00]);
    let mut slice = build_action_slice(msg_cell, 3);
    OutAction::skip(&mut slice.clone()).unwrap();
    let err = OutAction::construct_from(&mut slice).unwrap_err();
    match err.downcast_ref::<BlockError>() {
        Some(BlockError::OutActionError(_, mode)) => assert_eq!(*mode, 3),
        other => panic!("expected OutActionError with mode=3, got: {other:?}"),
    }
}

#[test]
fn test_deserialize_bad_out_action() {
    let valid_cell = OutAction::new_set(Cell::default()).serialize().unwrap();
    let mut valid_slice = SliceData::load_cell(valid_cell).unwrap();
    OutAction::construct_from(&mut valid_slice).unwrap(); // sanity check that the valid slice is indeed valid

    let mut invalid_builder = BuilderData::new();
    0xffff_ffffu32.write_to(&mut invalid_builder).unwrap();
    let mut invalid_slice = SliceData::load_cell(invalid_builder.into_cell().unwrap()).unwrap();

    OutAction::construct_from(&mut invalid_slice).unwrap_err(); // sanity check that the invalid slice is indeed invalid
}
