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
use std::str::FromStr;
use ton_block::{CurrencyCollection, InternalMessageHeader, Message, MsgAddressInt, Serializable};

#[test]
fn test_msg_envelope() {
    let src = MsgAddressInt::from_str(
        "0:a4cd3dfa89c5aa75c5542b3414d1c0c7974aaa5009fbe9c4ab6a5566c50cc607",
    )
    .unwrap();
    let dst = MsgAddressInt::from_str(
        "0:7c270a15afd9b685d6cabe4334172fa0d2e8ceb61ac033e7ce8876f2cf7122be",
    )
    .unwrap();
    let src_shard = ShardIdent::with_tagged_prefix(0, 0xa400000000000000).unwrap();
    let dst_shard = ShardIdent::with_tagged_prefix(0, 0x7c00000000000000).unwrap();
    let hdr = InternalMessageHeader::with_addresses(src, dst, CurrencyCollection::default());
    let msg = Message::with_int_header(hdr);
    let msg_cell = msg.serialize().unwrap();
    let env =
        MsgEnvelopeStuff::new(msg.clone(), msg_cell.clone(), &src_shard, Grams::default(), false)
            .unwrap();
    assert!(src_shard.contains_full_prefix(env.cur_prefix()));
    assert!(dst_shard.contains_full_prefix(env.next_prefix()));

    let env = MsgEnvelopeStuff::new(msg, msg_cell, &src_shard, Grams::default(), true).unwrap();
    assert!(src_shard.contains_full_prefix(env.cur_prefix()));
    assert!(!dst_shard.contains_full_prefix(env.next_prefix()));
    let shard = ShardIdent::with_tagged_prefix(0, 0x7400000000000000).unwrap();
    assert!(shard.contains_full_prefix(env.next_prefix()));
}

#[test]
fn test_msg_envelope_before_split() {
    let src = MsgAddressInt::from_str(
        "0:84cd3dfa89c5aa75c5542b3414d1c0c7974aaa5009fbe9c4ab6a5566c50cc607",
    )
    .unwrap();
    let dst = MsgAddressInt::from_str(
        "0:fc270a15afd9b685d6cabe4334172fa0d2e8ceb61ac033e7ce8876f2cf7122be",
    )
    .unwrap();
    let src_prefix = AccountIdPrefixFull::checked_prefix(&src).unwrap();
    let dst_prefix = AccountIdPrefixFull::checked_prefix(&dst).unwrap();
    let src_shard = ShardIdent::with_tagged_prefix(0, 0xA000000000000000).unwrap();
    let dst_shard = ShardIdent::with_tagged_prefix(0, 0xE000000000000000).unwrap();
    let hdr = InternalMessageHeader::with_addresses(src, dst, CurrencyCollection::default());
    let msg = Message::with_int_header(hdr);
    let msg_cell = msg.serialize().unwrap();
    // create envelope before split (so src and dst are in the same shard)
    let env = MsgEnvelopeStuff::new(
        msg.clone(),
        msg_cell.clone(),
        &src_shard.merge().unwrap(),
        Grams::default(),
        true,
    )
    .unwrap();
    assert_eq!(env.src_prefix(), &src_prefix);
    assert_eq!(env.dst_prefix(), &dst_prefix);
    assert_eq!(env.cur_prefix(), &dst_prefix);
    assert_eq!(env.next_prefix(), &dst_prefix);
    assert!(!src_shard.contains_full_prefix(env.cur_prefix()));
    assert!(dst_shard.contains_full_prefix(env.cur_prefix()));
    assert!(dst_shard.contains_full_prefix(env.next_prefix()));
}
