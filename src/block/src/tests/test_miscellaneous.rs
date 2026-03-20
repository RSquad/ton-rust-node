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
    write_read_and_assert, AccountId, Block, CurrencyCollection, IntermediateAddress,
    InternalMessageHeader, Message, MsgAddressInt, MsgEnvelope, ShardIdent,
};
use std::str::FromStr;

#[test]
fn test_process_info_key() {
    let pik1 = ProcessedInfoKey::with_params(1, 34534);
    let pik2 = ProcessedInfoKey::with_params(2, 34534);
    let pik3 = ProcessedInfoKey::with_params(2, 34535);

    assert_ne!(pik1, pik2);
    assert_ne!(pik3, pik2);
    write_read_and_assert(pik1);
    write_read_and_assert(pik2);
    write_read_and_assert(pik3);
}

#[test]
fn test_processed_upto() {
    let put1 = ProcessedUpto::with_params(123475628374, UInt256::from([22; 32]), None);
    let put2 = ProcessedUpto::with_params(23847651928, UInt256::from([23; 32]), None);
    let put3 = ProcessedUpto::with_params(2, UInt256::default(), None);

    assert_ne!(put1, put2);
    assert_ne!(put3, put2);
    write_read_and_assert(put1);
    write_read_and_assert(put2);
    write_read_and_assert(put3);
}

#[test]
fn test_process_info() {
    let mut pi2 = ProcessedInfo::default();

    let pik1 = ProcessedInfoKey::with_params(1, 1111);
    let pik2 = ProcessedInfoKey::with_params(2, 2222);
    let pik3 = ProcessedInfoKey::with_params(3, 3333);

    let put1 = ProcessedUpto::with_params(111111, UInt256::from([1; 32]), None);
    let put2 = ProcessedUpto::with_params(222222, UInt256::from([2; 32]), None);
    let put3 = ProcessedUpto::with_params(333333, UInt256::from([3; 32]), None);

    pi2.set(&pik1, &put1).unwrap();
    pi2.set(&pik2, &put2).unwrap();
    pi2.set(&pik3, &put3).unwrap();

    write_read_and_assert(pi2.clone());

    pi2.iterate(|put| {
        println!("{:?}", put);
        Ok(true)
    })
    .unwrap();

    let a = pi2.get(&pik1).unwrap();
    assert_eq!(a, Some(put1));
    let a = pi2.get(&pik2).unwrap();
    assert_eq!(a, Some(put2));
    let a = pi2.get(&pik3).unwrap();
    assert_eq!(a, Some(put3));

    write_read_and_assert(pi2.clone());

    pi2.iterate(|put| {
        println!("{:?}", put);
        Ok(true)
    })
    .unwrap();
}

#[test]
fn test_find_shards_by_routing_custom() {
    // message from 0xd8... to 0x9C...
    // src address: 1101_0111_
    // dst address: 1011_1101_
    // envelope.cur_addr: 32 - total src
    // envelope.next_addr: 37 - 5 high bits form dst
    let src =
        AccountId::from_str("d78b3fd904191a09d111af6bd6aee2c891ee19edd419e40520e3312f68cbcec1")
            .unwrap();
    let dst =
        AccountId::from_str("9dd300cee029b9c799ef1c8317554a937c80aa475fb1324f0e22e80ac7a55ca3")
            .unwrap();
    let hdr = InternalMessageHeader::with_addresses_and_bounce(
        MsgAddressInt::with_standart(None, 0, src).unwrap(),
        MsgAddressInt::with_standart(None, 0, dst).unwrap(),
        CurrencyCollection::with_grams(3000000000),
        true,
    );
    let msg = Message::with_int_header(hdr);
    let mut env = MsgEnvelope::with_message_and_fee(&msg, 100.into()).unwrap();
    env.set_next_addr(IntermediateAddress::use_dest_bits(37).unwrap())
        .set_cur_addr(IntermediateAddress::use_dest_bits(32).unwrap());
    let (cur_prefix, next_prefix) = env.calc_cur_next_prefix().unwrap();
    println!("cur: {}, next: {}", cur_prefix, next_prefix);

    assert_eq!(cur_prefix.prefix, 0xd78b3fd904191a09);
    assert_eq!(next_prefix.prefix, 0x9f8b3fd904191a09);

    let block = Block::construct_from_file("src/tests/data/key_block_not_all_shardes.boc").unwrap();
    let extra = block.read_extra().unwrap().read_custom().unwrap().expect("need key block");
    let shards = extra.shards();

    let shard_src = ShardIdent::with_tagged_prefix(0, 0xd800000000000000).unwrap();
    let shard_dst = ShardIdent::with_tagged_prefix(0, 0x9C00000000000000).unwrap();

    let found_shard = shards.get_shard(&shard_src).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(found_shard.unwrap().shard(), &shard_src);

    let found_shard = shards.get_shard(&shard_dst).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(found_shard.unwrap().shard(), &shard_dst);

    let found_shard = shards.find_shard_by_prefix(&cur_prefix).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(found_shard.unwrap().shard(), &shard_src);

    let found_shard = shards.find_shard_by_prefix(&next_prefix).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(found_shard.unwrap().shard(), &shard_dst);
}
