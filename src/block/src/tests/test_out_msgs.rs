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
    generate_test_stateinit, types::Coins, write_read_and_assert, AccountId, AccountStatus,
    HashUpdate, InMsgExternal, InternalMessageHeader, MsgAddressInt, StateInitTestOptions,
    TransactionDescr,
};
use std::str::FromStr;

fn get_message_with_addrs(src: AccountId, dst: AccountId) -> Message {
    let mut msg = Message::with_int_header(InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, src).unwrap(),
        MsgAddressInt::with_standart(None, 0, dst).unwrap(),
        CurrencyCollection::default(),
    ));
    msg.set_state_init(generate_test_stateinit(StateInitTestOptions::with_default_setup(false)));
    msg
}

fn get_message() -> Message {
    get_message_with_addrs(AccountId::from([0; 32]), AccountId::from([1; 32]))
}

fn transaction() -> Transaction {
    let mut tr = Transaction::with_address_and_status(
        AccountId::from([1; 32]),
        AccountStatus::AccStateActive,
    );

    let s_in_msg = get_message();
    let s_out_msg1 = get_message();
    let s_out_msg2 = get_message();
    let s_out_msg3 = get_message();

    let s_status_update = HashUpdate::default();
    let s_tr_desc = TransactionDescr::default();

    tr.set_logical_time(123423);
    tr.set_end_status(AccountStatus::AccStateFrozen);
    tr.set_total_fees(CurrencyCollection::with_coins(653));
    tr.write_in_msg(Some(&s_in_msg)).unwrap();
    tr.add_out_message(&s_out_msg1).unwrap();
    tr.add_out_message(&s_out_msg2).unwrap();
    tr.add_out_message(&s_out_msg3).unwrap();
    tr.write_state_update(&s_status_update).unwrap();
    tr.write_description(&s_tr_desc).unwrap();
    tr
}

fn get_out_ext_msg() -> OutMsg {
    let tr_cell = ChildCell::with_struct(&transaction()).unwrap();
    let msg_cell = ChildCell::with_struct(&get_message()).unwrap();
    OutMsg::external(msg_cell, tr_cell)
}

#[test]
fn test_out_msg_external_serialization() {
    let msg = get_out_ext_msg();
    write_read_and_assert(msg);
}

#[test]
fn test_out_msg_immediately_serialization() {
    let msg = OutMsg::immediate(
        ChildCell::with_struct(&MsgEnvelope::default()).unwrap(),
        ChildCell::with_struct(&transaction()).unwrap(),
        ChildCell::with_struct(&InMsg::External(InMsgExternal::default())).unwrap(),
    );
    write_read_and_assert(msg);
}

#[test]
fn test_out_msg_new_serialization() {
    let msg = OutMsg::new(
        ChildCell::with_struct(&MsgEnvelope::default()).unwrap(),
        ChildCell::with_struct(&transaction()).unwrap(),
    );
    write_read_and_assert(msg);
}

#[test]
fn test_out_msg_transit_serialization() {
    let msg = OutMsg::transit(
        ChildCell::with_struct(&MsgEnvelope::default()).unwrap(),
        ChildCell::with_struct(&InMsg::External(InMsgExternal::default())).unwrap(),
        false,
    );
    write_read_and_assert(msg);
}

#[test]
fn test_out_msg_dequeue_serialization() {
    let msg = OutMsg::Dequeue(OutMsgDequeue::with_cells(
        ChildCell::with_struct(&MsgEnvelope::default()).unwrap(),
        243563457456709,
    ));
    write_read_and_assert(msg);
}

#[test]
fn test_out_msg_dequeue_short_serialization() {
    let msg = OutMsg::DequeueShort(OutMsgDequeueShort {
        msg_env_hash: UInt256::from_str(
            "b44798875f5c390ea9d405b653abb213fb25c108ddd316ccfbb10df2558d6e6c",
        )
        .unwrap(),
        next_workchain: -1,
        next_addr_pfx: 238798479,
        import_block_lt: 1000234234,
    });
    write_read_and_assert(msg);
}

#[test]
fn test_serialization_out_msg_descr() {
    let mut desc = OutMsgDescr::default();
    for _ in 0..10 {
        desc.insert(&get_out_ext_msg()).unwrap();
    }
    write_read_and_assert(desc);
}

#[test]
fn test_serialization_out_msg_queue() {
    let mut queue = OutMsgQueue::default();
    for n in 0..100 {
        let msg = get_message();
        let out_msg_env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();
        queue.insert(0, n, &out_msg_env, 11).unwrap();
    }
    println!("{:?}", queue);
    write_read_and_assert(queue);
}

fn create_account_id(n: u8) -> AccountId {
    AccountId::from([
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, n,
    ])
}

#[test]
fn test_work_with_out_msg_desc() {
    let tr = transaction();
    let tr_cell = ChildCell::with_struct(&tr).unwrap();
    let mut msg_desc = OutMsgDescr::default();

    // test OutMsg::External
    let msg = get_message_with_addrs(create_account_id(1), create_account_id(2));
    let out_msg_ext = OutMsg::external(ChildCell::with_struct(&msg).unwrap(), tr_cell.clone());

    msg_desc.insert(&out_msg_ext).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 1);

    let msg = get_message_with_addrs(create_account_id(2), create_account_id(1));
    let out_msg_ext = OutMsg::external(ChildCell::with_struct(&msg).unwrap(), tr_cell.clone());

    msg_desc.insert(&out_msg_ext).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 2);

    // msg_desc.remove(out_msg_ext);
    // assert_eq!(msg_desc.len().unwrap(), 1);

    // test OutMsg::Immediate
    let msg = get_message_with_addrs(create_account_id(3), create_account_id(4));
    let msg_in = InMsg::external(ChildCell::with_struct(&msg).unwrap(), tr_cell.clone());

    let env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();
    let out_msg = OutMsgImmediate::with_cells(
        ChildCell::with_struct(&env).unwrap(),
        tr_cell.clone(),
        ChildCell::with_struct(&msg_in).unwrap(),
    );
    let out_msg_imm = OutMsg::Immediate(out_msg);

    msg_desc.insert(&out_msg_imm).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 3);

    // test OutMsg::OutMsgNew
    let msg = get_message_with_addrs(create_account_id(4), create_account_id(5));
    let env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();

    let out_msg_new = OutMsg::new(ChildCell::with_struct(&env).unwrap(), tr_cell.clone());

    msg_desc.insert(&out_msg_new).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 4);

    // test OutMsg::OutMsgTransit
    let msg = get_message_with_addrs(create_account_id(5), create_account_id(6));
    let msg_in = InMsg::external(ChildCell::with_struct(&msg).unwrap(), tr_cell.clone());

    let out_msg_transit = OutMsg::Transit(OutMsgTransit::with_cells(
        ChildCell::with_struct(&MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap())
            .unwrap(),
        ChildCell::with_struct(&msg_in).unwrap(),
    ));

    msg_desc.insert(&out_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 5);

    // test OutMsg::OutMsgDequeue
    let msg = get_message_with_addrs(create_account_id(6), create_account_id(7));
    let env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();
    let out_msg = OutMsgDequeue::with_cells(ChildCell::with_struct(&env).unwrap(), 32523);
    let out_msg_dequeue = OutMsg::Dequeue(out_msg);

    msg_desc.insert(&out_msg_dequeue).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 6);

    // test OutMsg::OutMsgDequeueShort
    let out_msg_dequeue_short = OutMsg::DequeueShort(OutMsgDequeueShort {
        msg_env_hash: UInt256::from_str(
            "b44798875f5c390ea9d405b653abb213fb25c108ddd316ccfbb10df2558d6e6c",
        )
        .unwrap(),
        next_workchain: -100,
        next_addr_pfx: 6,
        import_block_lt: 1234567890,
    });

    let msg = get_message_with_addrs(create_account_id(7), create_account_id(8));
    let hash = msg.serialize().unwrap().repr_hash();
    msg_desc.insert_with_key(&hash, &out_msg_dequeue_short).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 7);

    // test OutMsg::OutMsgTransitRequeued
    let msg = get_message_with_addrs(create_account_id(8), create_account_id(9));
    let msg_in = InMsg::external(ChildCell::with_cell(msg.serialize().unwrap()), tr_cell.clone());
    let out_msg_transit = OutMsg::TransitRequeued(OutMsgTransitRequeued::with_cells(
        ChildCell::with_struct(&MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap())
            .unwrap(),
        ChildCell::with_struct(&msg_in).unwrap(),
    ));

    msg_desc.insert(&out_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 8);
    // test OutMsg::NewDefer
    let msg = get_message_with_addrs(create_account_id(10), create_account_id(11));
    let env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();

    let out_msg_new = OutMsg::new_defer(ChildCell::with_struct(&env).unwrap(), tr_cell.clone());

    msg_desc.insert(&out_msg_new).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 9);

    // test OutMsg::DeferredTransit
    let msg = get_message_with_addrs(create_account_id(12), create_account_id(13));
    let msg_in = InMsg::external(ChildCell::with_struct(&msg).unwrap(), tr_cell.clone());

    let env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();
    let out_msg_transit = OutMsg::DeferredTransit(OutMsgDefferedTransit::with_cells(
        ChildCell::with_struct(&env).unwrap(),
        ChildCell::with_struct(&msg_in).unwrap(),
    ));

    msg_desc.insert(&out_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 10);
}

#[test]
fn test_out_msg_queue_and_info() {
    let mut queue = OutMsgQueue::default();

    // test OutMsg::External
    let msg = get_message_with_addrs(create_account_id(1), create_account_id(2));
    let out_msg_env = MsgEnvelope::with_message_and_fee(&msg, Coins::one()).unwrap();

    queue.insert(0, 1, &out_msg_env, 11).unwrap();
    assert_eq!(queue.len().unwrap(), 1);
    write_read_and_assert(queue.clone());

    let omq_info =
        OutMsgQueueInfo::with_params(queue, ProcessedInfo::default(), Default::default());
    write_read_and_assert(omq_info);
}

#[test]
fn test_enqueued_msg() {
    let em1 = EnqueuedMsg::new();
    let em2 = EnqueuedMsg::default();
    assert_eq!(em1, em2);
    write_read_and_assert(em1);

    let em1 = EnqueuedMsg::with_param(
        234523452345,
        &MsgEnvelope::with_message_and_fee(&Message::default(), 27348376.into()).unwrap(),
    )
    .unwrap();
    let em2 = EnqueuedMsg::with_param(
        234523452346,
        &MsgEnvelope::with_message_and_fee(&Message::default(), 27348377.into()).unwrap(),
    )
    .unwrap();
    assert_ne!(em1, em2);

    write_read_and_assert(em1);
    write_read_and_assert(em2);
}
