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
    write_read_and_assert, AccountId, AccountStatus, ExternalInboundMessageHeader, HashUpdate,
    HashmapType, InternalMessageHeader, MsgAddressExt, MsgAddressInt, StateInit, TickTock,
    TransactionDescr,
};

macro_rules! chcell {
    ($data:expr) => {
        ChildCell::with_struct(&$data).unwrap()
    };
}

fn create_external_message() -> Message {
    let src =
        MsgAddressExt::with_extern(SliceData::new(vec![0x23, 0x52, 0x73, 0x00, 0x80])).unwrap();
    let dst = MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap();
    let mut hdr = ExternalInboundMessageHeader::new(src, dst);
    hdr.import_fee = 10.into();
    Message::with_ext_in_header(hdr)
}

fn create_internal_message() -> Message {
    let hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap(),
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x22; 32])).unwrap(),
        CurrencyCollection::default(),
    );
    Message::with_int_header(hdr)
}

fn create_transation() -> Transaction {
    let mut t = Transaction::with_address_and_status(
        AccountId::from([1; 32]),
        AccountStatus::AccStateActive,
    );
    t.set_logical_time(1111);
    t.set_total_fees(CurrencyCollection::with_grams(2222));
    t
}

#[test]
fn test_serde_inmsg_ext_withdata() {
    let msg_descriptor =
        InMsgExternal::with_cells(chcell!(create_external_message()), chcell!(create_transation()));
    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_ext() {
    let msg_descriptor = InMsg::External(InMsgExternal::default());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_ihr_withdata() {
    let msg_descriptor = InMsgIHR::with_cells(
        chcell!(create_internal_message()),
        chcell!(create_transation()),
        10.into(),
        Cell::default(),
    );

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_ihr() {
    let msg_descriptor = InMsg::IHR(InMsgIHR::default());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_imm_withdata() {
    let msg_descriptor = InMsgFinal::with_cells(
        chcell!(MsgEnvelope::default()),
        chcell!(create_transation()),
        10.into(),
    );

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_imm() {
    let msg_descriptor = InMsg::Immediate(InMsgFinal::default());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_tr_withdata() {
    let msg_descriptor = InMsgTransit::with_cells(
        chcell!(MsgEnvelope::default()),
        chcell!(MsgEnvelope::default()),
        123.into(),
    );

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_transit() {
    let msg_descriptor = InMsg::Transit(InMsgTransit::default());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_discarded_fin_withdata() {
    let msg_descriptor =
        InMsgDiscardedFinal::with_cells(chcell!(MsgEnvelope::default()), 1234567, 123.into());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_discarded_fin() {
    let msg_descriptor = InMsg::DiscardedFinal(InMsgDiscardedFinal::default());

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_discarded_tr_withdata() {
    let mut b = BuilderData::new();
    b.append_raw(&[1, 2, 3], 3 * 8).unwrap();
    let msg_descriptor = InMsgDiscardedTransit::with_cells(
        chcell!(MsgEnvelope::default()),
        1234567,
        123.into(),
        b.into_cell().unwrap(),
    );

    write_read_and_assert(msg_descriptor);
}

#[test]
fn test_serde_inmsg_discarded_tr() {
    let msg_descriptor = InMsg::DiscardedTransit(InMsgDiscardedTransit::default());

    write_read_and_assert(msg_descriptor);
}

fn create_account_id(n: u8) -> AccountId {
    AccountId::from([
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, n,
    ])
}

fn get_message_with_addrs(src: AccountId, dst: AccountId) -> Message {
    let mut msg = Message::with_int_header(InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, src).unwrap(),
        MsgAddressInt::with_standart(None, 0, dst).unwrap(),
        CurrencyCollection::default(),
    ));

    let mut stinit = StateInit::default();
    stinit.set_fixed_prefix_length(23.try_into().unwrap());
    stinit.set_special(TickTock::with_values(false, true));
    let code = SliceData::new(vec![
        0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    stinit.set_code(code.into_cell().unwrap());
    let data = SliceData::new(vec![
        0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    stinit.set_data(data.into_cell().unwrap());
    let library = SliceData::new(vec![
        0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    stinit.set_library_code(library.into_cell().unwrap(), true).unwrap();

    msg.set_state_init(stinit);

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
    tr.set_total_fees(CurrencyCollection::with_grams(653));
    tr.write_in_msg(Some(&s_in_msg)).unwrap();
    tr.add_out_message(&s_out_msg1).unwrap();
    tr.add_out_message(&s_out_msg2).unwrap();
    tr.add_out_message(&s_out_msg3).unwrap();
    tr.write_state_update(&s_status_update).unwrap();
    tr.write_description(&s_tr_desc).unwrap();
    tr
}

#[test]
fn test_work_with_in_msg_desc() {
    let mut msg_desc = InMsgDescr::default();

    // test InMsg::External
    let msg = get_message_with_addrs(create_account_id(1), create_account_id(2));
    let tr = transaction();
    let tr_cell = chcell!(tr);
    let in_msg_ext = InMsg::external(chcell!(msg), tr_cell.clone());

    msg_desc.insert(&in_msg_ext).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 1);

    let msg = get_message_with_addrs(create_account_id(2), create_account_id(1));
    let in_msg_ext = InMsg::external(chcell!(msg), tr_cell.clone());

    msg_desc.insert(&in_msg_ext).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 2);

    // msg_desc.remove(in_msg_ext);

    // assert_eq!(msg_desc.len().unwrap(), 1);

    // test InMsg::IHR
    let msg = get_message_with_addrs(create_account_id(3), create_account_id(4));

    let in_msg_ihr = InMsg::ihr(chcell!(msg), tr_cell.clone(), Grams::one(), Cell::default());

    msg_desc.insert(&in_msg_ihr).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 3);

    // test InMsg::Final
    let msg = get_message_with_addrs(create_account_id(4), create_account_id(5));
    let msg = MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap();

    let in_msg_final = InMsg::final_msg(chcell!(msg), tr_cell.clone(), Grams::one());

    msg_desc.insert(&in_msg_final).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 4);

    // test InMsg::InMsgTransit
    let msg = get_message_with_addrs(create_account_id(5), create_account_id(6));
    let msg1 = get_message_with_addrs(create_account_id(6), create_account_id(4));

    let in_msg_transit = InMsg::transit(
        chcell!(MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap()),
        chcell!(MsgEnvelope::with_message_and_fee(&msg1, Grams::one()).unwrap()),
        Grams::one(),
    );

    msg_desc.insert(&in_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 5);

    // test InMsg::DiscardedFinal
    let msg = get_message_with_addrs(create_account_id(6), create_account_id(7));
    let msg = MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap();

    let in_msg_final = InMsg::discarded_final(chcell!(msg), 453453, Grams::one());

    msg_desc.insert(&in_msg_final).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 6);

    // test InMsg::DiscardedTransit
    let msg = get_message_with_addrs(create_account_id(7), create_account_id(8));

    let in_msg_transit = InMsg::DiscardedTransit(InMsgDiscardedTransit::with_cells(
        chcell!(MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap()),
        453453,
        Grams::one(),
        SliceData::new_empty().into_cell().unwrap(),
    ));

    msg_desc.insert(&in_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 7);

    // test InMsg::InMsgDeferredFinal
    let msg = get_message_with_addrs(create_account_id(9), create_account_id(10));
    let msg = MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap();

    let in_msg_final = InMsg::deferred_final(chcell!(msg), tr_cell.clone(), Grams::one());

    msg_desc.insert(&in_msg_final).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 8);

    // test InMsg::DeferredTransit
    let msg = get_message_with_addrs(create_account_id(11), create_account_id(12));
    let msg1 = get_message_with_addrs(create_account_id(13), create_account_id(14));

    let in_msg_transit = InMsg::deferred_transit(
        chcell!(MsgEnvelope::with_message_and_fee(&msg, Grams::one()).unwrap()),
        chcell!(MsgEnvelope::with_message_and_fee(&msg1, Grams::one()).unwrap()),
    );

    msg_desc.insert(&in_msg_transit).unwrap();
    assert_eq!(msg_desc.len().unwrap(), 9);
}

#[test]
fn test_inmsg_serde_with_cmnmsg_success() {
    let msg = Message::default();
    let orig_status = AccountStatus::AccStateActive;
    let acc_id = AccountId::from([1; 32]);
    let tr = Transaction::with_address_and_status(acc_id.clone(), orig_status);
    let enveloped = MsgEnvelope::with_message_and_fee(&msg, 10.into()).unwrap();
    let msg_cell = ChildCell::with_struct(&msg).unwrap();
    let tr_cell = ChildCell::with_struct(&tr).unwrap();
    assert_eq!(tr_cell.read_struct().unwrap(), tr);
    let env_cell = ChildCell::with_struct(&enveloped).unwrap();
    let inmsg_variants = [
        InMsg::external(msg_cell.clone(), tr_cell.clone()),
        InMsg::ihr(msg_cell.clone(), tr_cell.clone(), 1.into(), Cell::default()),
        InMsg::immediate(env_cell.clone(), tr_cell.clone(), 2.into()),
        InMsg::final_msg(env_cell.clone(), tr_cell.clone(), 3.into()),
        InMsg::transit(env_cell.clone(), env_cell.clone(), 4.into()),
        InMsg::discarded_final(env_cell.clone(), 10, 5.into()),
        InMsg::discarded_transit(env_cell.clone(), 20, 6.into(), Cell::default()),
        InMsg::deferred_final(env_cell.clone(), tr_cell.clone(), 7.into()),
        InMsg::deferred_transit(env_cell.clone(), env_cell.clone()),
    ];
    for ref inmsg in inmsg_variants {
        if let Some(tr2) = inmsg.read_transaction().unwrap() {
            assert_eq!(tr2, tr);
        }
        let cell = inmsg.serialize().unwrap();
        let inmsg2 = InMsg::construct_from_cell(cell).unwrap();
        assert_eq!(inmsg, &inmsg2);
        let msg2 = inmsg2.read_message().unwrap();
        assert_eq!(msg2, msg);
        if let Some(tr2) = inmsg2.read_transaction().unwrap() {
            assert_eq!(tr2, tr);
        }
    }
}
