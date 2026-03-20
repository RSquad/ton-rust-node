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
use crate::{
    generate_test_message, write_read_and_assert, CurrencyCollection, InternalMessageHeader,
    MsgAddressInt, ShardIdent, StateInitTestOptions,
};
use std::fmt::Debug;

fn check_serialize<T: Debug + Default + Deserializable + PartialEq + Serializable>(src: &T) {
    let mut b = BuilderData::new();
    src.write_to(&mut b).unwrap();
    let mut s = SliceData::load_builder(b).unwrap();
    let mut cmp = T::default();
    cmp.read_from(&mut s).unwrap();
    assert_eq!(src, &cmp);
}

#[test]
fn test_serialize_intermediate_addr_regular() {
    check_serialize(&IntermediateAddressRegular::with_use_src_bits(0).unwrap());
    check_serialize(&IntermediateAddressRegular::with_use_src_bits(1).unwrap());
    check_serialize(&IntermediateAddressRegular::with_use_src_bits(54).unwrap());
    check_serialize(&IntermediateAddressRegular::with_use_src_bits(96).unwrap());
}

#[test]
fn test_intermediate_addr_regular_cons() {
    IntermediateAddressRegular::with_use_src_bits(97).expect_err("must not allow more than 96");
    IntermediateAddressRegular::with_use_dest_bits(97).expect_err("must not allow more than 96");
}

#[test]
fn test_intermediate_addr_regular_set() {
    let mut a = IntermediateAddressRegular::with_use_src_bits(0).unwrap();
    a.set_use_src_bits(97).expect_err("must not allow more than 96");
}

#[test]
fn test_serialize_intermediate_addr_simple() {
    check_serialize(&IntermediateAddressSimple::with_addr(-1, 0x0102030405060708));
    check_serialize(&IntermediateAddressSimple::with_addr(0, 0xFF_FF_FF_FF_FF_FF_FF_FF));
    check_serialize(&IntermediateAddressSimple::with_addr(1, 0));
    check_serialize(&IntermediateAddressSimple::with_addr(127, 0xCD_CD_CD_CD_CD_CD_CD_CD));
}

#[test]
fn test_serialize_intermediate_addr_ext() {
    check_serialize(&IntermediateAddressExt::with_addr(-1, 0x0102030405060708));
    check_serialize(&IntermediateAddressExt::with_addr(0, 0xFF_FF_FF_FF_FF_FF_FF_FF));
    check_serialize(&IntermediateAddressExt::with_addr(1, 0));
    check_serialize(&IntermediateAddressExt::with_addr(3462346, 0xCD_CD_CD_CD_CD_CD_CD_CD));
}

#[test]
fn test_serialize_intermediate_address() {
    fn check_ext(addr: IntermediateAddressExt) {
        check_serialize(&IntermediateAddress::Ext(addr))
    }

    fn check_regular(addr: IntermediateAddressRegular) {
        check_serialize(&IntermediateAddress::Regular(addr))
    }

    fn check_simple(addr: IntermediateAddressSimple) {
        check_serialize(&IntermediateAddress::Simple(addr))
    }

    check_regular(IntermediateAddressRegular::with_use_src_bits(0).unwrap());
    check_regular(IntermediateAddressRegular::with_use_src_bits(96).unwrap());
    check_simple(IntermediateAddressSimple::with_addr(-1, 0x0102030405060708));
    check_simple(IntermediateAddressSimple::with_addr(1, 0xFE_FE_FE_FE_FE_FE_FE_FE));
    check_ext(IntermediateAddressExt::with_addr(-1, 0x0102030405060708));
    check_ext(IntermediateAddressExt::with_addr(1, 0xCD_CD_CD_CD_CD_CD_CD_CD));
}

#[test]
fn test_serialization_msg_envelope() {
    write_read_and_assert(MsgEnvelope::default());

    let mut msg = MsgEnvelope::with_message_and_fee(
        &generate_test_message(true, StateInitTestOptions::with_default_setup(false)),
        12312.into(),
    )
    .unwrap();
    msg.set_metadata(1, Some(MsgMetadata::new(MsgAddressInt::standard(-1, [0x22; 32]), 0)));
    write_read_and_assert(msg.clone());

    msg.set_cur_addr(IntermediateAddress::Simple(IntermediateAddressSimple::with_addr(
        -1,
        0x0102030405060708,
    )))
    .set_next_addr(IntermediateAddress::Simple(IntermediateAddressSimple::with_addr(
        -1,
        0x0102030405060708,
    )));
    write_read_and_assert(msg.clone());

    assert!(msg.collect_fee(123.into()));
    write_read_and_assert(msg.clone());
}

// prepare for testing purposes
fn prepare_test_env_message(
    src_prefix: u64,
    dst_prefix: u64,
    bits: u8,
    at: u32,
    lt: u64,
) -> Result<(Message, MsgEnvelope)> {
    let shard = ShardIdent::with_prefix_len(bits, 0, src_prefix)?;
    let src = UInt256::from_le_bytes(&src_prefix.to_be_bytes());
    let dst = UInt256::from_le_bytes(&dst_prefix.to_be_bytes());
    let src = MsgAddressInt::with_standart(None, 0, src.into())?;
    let dst = MsgAddressInt::with_standart(None, 0, dst.into())?;

    // let src_prefix = AccountIdPrefixFull::prefix(&src).unwrap();
    // let dst_prefix = AccountIdPrefixFull::prefix(&dst).unwrap();
    // let ia = IntermediateAddress::full_src();
    // let route_info = src_prefix.perform_hypercube_routing(&dst_prefix, &shard, ia)?.unwrap();
    // let cur_prefix  = src_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.0)?;
    // let next_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.1)?;

    let hdr = InternalMessageHeader::with_addresses(
        src,
        dst,
        CurrencyCollection::with_coins(1_000_000_000),
    );
    let mut msg = Message::with_int_header(hdr);
    msg.set_at_and_lt(at, lt);

    let env = MsgEnvelope::hypercube_routing(&msg, &shard, 1_000_000.into())?;
    Ok((msg, env))
}

#[test]
fn test_prepare_msg_envelope() {
    let (msg, env) =
        prepare_test_env_message(0xd78b3fd904191a09, 0x9dd300cee029b9c7, 4, 0, 0).unwrap();
    let src = msg
        .src_ref()
        .ok_or_else(|| error!("source address of message {:x} is invalid", env.message_hash()))
        .unwrap();
    let src_prefix = AccountIdPrefixFull::checked_prefix(src).unwrap();
    let dst = msg
        .dst_ref()
        .ok_or_else(|| error!("destination address of message {:x} is invalid", env.message_hash()))
        .unwrap();
    let dst_prefix = AccountIdPrefixFull::checked_prefix(dst).unwrap();
    assert_eq!(src_prefix, AccountIdPrefixFull::workchain(0, 0xd78b3fd904191a09));
    assert_eq!(dst_prefix, AccountIdPrefixFull::workchain(0, 0x9dd300cee029b9c7));

    let (cur_prefix, next_prefix) = env.calc_cur_next_prefix().unwrap();
    assert_eq!(cur_prefix, AccountIdPrefixFull::workchain(0, 0xd78b3fd904191a09));
    assert_eq!(next_prefix, AccountIdPrefixFull::workchain(0, 0x978b3fd904191a09));

    let src_shard = ShardIdent::with_tagged_prefix(0, 0xD800000000000000).unwrap();
    src_prefix
        .perform_hypercube_routing(&dst_prefix, &src_shard, IntermediateAddress::default())
        .unwrap();
}

#[test]
fn test_routing_with_hop() {
    let pfx_len = 12;
    let src = 0xd78b3fd904191a09;
    let dst = 0xd4d300cee029b9c7;
    let hop = 0xd48b3fd904191a09;
    let (msg, env) = prepare_test_env_message(src, dst, pfx_len, 0, 0).unwrap();
    let src_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, src).unwrap();
    let dst_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, dst).unwrap();
    let hop_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, hop).unwrap();
    let src_addr = msg
        .src_ref()
        .ok_or_else(|| error!("source address of message {:x} is invalid", env.message_hash()))
        .unwrap();
    let src_prefix = AccountIdPrefixFull::checked_prefix(src_addr).unwrap();
    let dst_addr = msg
        .dst_ref()
        .ok_or_else(|| error!("destination address of message {:x} is invalid", env.message_hash()))
        .unwrap();
    let dst_prefix = AccountIdPrefixFull::checked_prefix(dst_addr).unwrap();
    assert!(src_shard_id.contains_full_prefix(&src_prefix));
    assert!(dst_shard_id.contains_full_prefix(&dst_prefix));

    assert_eq!(src_prefix, AccountIdPrefixFull::workchain(0, src));
    assert_eq!(dst_prefix, AccountIdPrefixFull::workchain(0, dst));

    let (cur_prefix, next_prefix) = env.calc_cur_next_prefix().unwrap();

    assert_eq!(src_prefix, cur_prefix);
    assert_ne!(dst_prefix, next_prefix);
    assert!(src_shard_id.contains_full_prefix(&cur_prefix));
    println!("shard: {}, prefix: {:x}", hop_shard_id, next_prefix.prefix);
    assert!(hop_shard_id.contains_full_prefix(&next_prefix));
    assert!(!dst_shard_id.contains_full_prefix(&next_prefix));

    assert_eq!(cur_prefix, AccountIdPrefixFull::workchain(0, src));
    assert_eq!(next_prefix, AccountIdPrefixFull::workchain(0, hop));

    src_prefix
        .perform_hypercube_routing(&dst_prefix, &src_shard_id, IntermediateAddress::default())
        .unwrap();
    let route_info = next_prefix
        .perform_hypercube_routing(&dst_prefix, &hop_shard_id, IntermediateAddress::default())
        .unwrap();
    let prefix = next_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.0).unwrap();
    assert_eq!(prefix, next_prefix);
    let prefix = next_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.1).unwrap();
    println!("shard: {}, prefix: {:x}", dst_shard_id, prefix.prefix);
    assert!(dst_shard_id.contains_full_prefix(&prefix));
    println!("dst_prefix: {:x}, prefix: {:x}", dst_prefix.prefix, prefix.prefix);
    assert_ne!(prefix, dst_prefix);
}

#[test]
fn test_intermediate_addr_default() {
    assert_eq!(IntermediateAddress::default(), IntermediateAddress::full_src());
}
