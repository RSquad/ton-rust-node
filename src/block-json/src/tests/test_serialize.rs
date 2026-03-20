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
    assert_json_eq, generate_test_account, generate_test_message, generate_test_stateinit,
    AccountTestOptions, StateInitTestOptions,
};
use std::{fs::read, path::Path};
use ton_block::{
    read_single_root_boc, write_boc, AccountId, IBitstring, ShardStateUnsplit, Transaction,
    TransactionProcessingStatus,
};

fn assert_json_eq_file(json: &str, name: &str) {
    let expected =
        std::fs::read_to_string(format!("src/tests/data/{}-ethalon.json", name)).unwrap();
    assert_json_eq(json, &expected, name);
}

fn generate_sample_account() -> AccountSerializationSet {
    let account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let boc = account.write_to_bytes().unwrap();
    AccountSerializationSet { account, prev_code_hash: None, boc, proof: None }
}

fn generate_sample_frozen_account() -> AccountSerializationSet {
    let mut account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let cloned_account = account.clone();
    account.try_freeze().unwrap();
    account.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap();
    let boc = account.write_to_bytes().unwrap();
    AccountSerializationSet {
        account,
        prev_code_hash: cloned_account.get_code_hash(),
        boc,
        proof: None,
    }
}

fn generate_sample_message() -> MessageSerializationSet {
    let mut msg = Message::with_ext_in_header(ExternalInboundMessageHeader {
        src: MsgAddressExt::with_extern(SliceData::new(vec![1, 2, 3, 4, 5, 0x80])).unwrap(),
        dst: MsgAddressInt::default(),
        import_fee: 15.into(),
    });
    let mut options = StateInitTestOptions::with_default_setup(false);
    options.data = None;
    msg.set_state_init(generate_test_stateinit(options));
    msg.set_body(SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]));
    let cell = msg.serialize().unwrap();
    let boc = write_boc(&cell).unwrap();
    let id = msg.hash().unwrap();
    MessageSerializationSet {
        message: msg,
        id,
        block_id: None,
        transaction_id: None,
        transaction_now: Some(123),
        status: MessageProcessingStatus::Processing,
        boc,
        proof: None,
    }
}

fn generate_sample_transaction(
    set_src: bool,
) -> (MessageSerializationSet, TransactionSerializationSet) {
    let transaction = generate_transaction([55; 32].into());
    let mut message = transaction.get_out_msg(0).unwrap().unwrap();
    if set_src {
        if let CommonMsgInfo::IntMsgInfo(header) = message.header_mut() {
            header.set_src(MsgAddressInt::default())
        }
    }
    let cell = message.serialize().unwrap();
    let boc = write_boc(&cell).unwrap();
    let id = message.hash().unwrap();
    let msg = MessageSerializationSet {
        message,
        id,
        block_id: None,
        transaction_id: transaction.hash().ok(),
        transaction_now: Some(transaction.now()),
        status: MessageProcessingStatus::Processing,
        boc,
        proof: None,
    };

    let cell = transaction.serialize().unwrap();
    let boc = write_boc(&cell).unwrap();
    let id = transaction.hash().unwrap();
    let tr = TransactionSerializationSet {
        transaction,
        id,
        status: TransactionProcessingStatus::Preliminary,
        block_id: None,
        workchain_id: -1,
        boc,
        proof: None,
    };

    (msg, tr)
}

pub fn generate_transaction(address: AccountId) -> Transaction {
    let s_in_msg = generate_test_message(true, StateInitTestOptions::with_default_setup(true));
    let s_out_msg1 = generate_test_message(true, StateInitTestOptions::with_default_setup(true));
    let (at, lt) = s_out_msg1.at_and_lt().unwrap();
    let mut s_out_msg2 = Message::default();
    s_out_msg2.set_at_and_lt(at, lt + 1);
    let mut s_out_msg3 = Message::default();
    s_out_msg3.set_at_and_lt(at, lt + 2);
    let s_status_update = HashUpdate::default();
    let s_tr_desc = TransactionDescr::default();
    let mut tr = Transaction::with_address_and_status(address, AccountStatus::AccStateActive);
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

#[test]
fn test_account_into_json_0() {
    let account = generate_sample_account();
    let json = db_serialize_account("id", &account).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "workchain_id": 0,
  "boc": "te6ccgEBFgEA8gAEdcAAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fISgMxAOt5orFvAAAAAAAAAABRdIdugA974AQIDBAIDzsAFBgEPP/////////QRAA8//x//////9AFDoA9Bcq9CvSeZR50tmWldnk60bjFEx5FdlFVin83DzELlmBUCASAHCAIBIAsMAAVQskACASAJCgADA5EABQQCWQIBIA0OAgEgDxAABQQDIQAFBAPpAAUEBLEABwcxLckBDz/////////0EgEPPz////////QTAQ8P////////9BQADz//P//////0AA8/////////9A==",
  "last_paid": 123456789,
  "bits_dec": "817",
  "bits": "2331",
  "cells_dec": "9",
  "cells": "09",
  "due_payment_dec": "111",
  "due_payment": "016f",
  "last_trans_lt_dec": "0",
  "last_trans_lt": "00",
  "balance_dec": "100000000000",
  "balance": "09174876e800",
  "balance_other": [
    {
      "currency": 1,
      "value_dec": "100",
      "value": "0164"
    },
    {
      "currency": 2,
      "value_dec": "200",
      "value": "01c8"
    },
    {
      "currency": 3,
      "value_dec": "300",
      "value": "0212c"
    },
    {
      "currency": 4,
      "value_dec": "400",
      "value": "02190"
    },
    {
      "currency": 5,
      "value_dec": "500",
      "value": "021f4"
    },
    {
      "currency": 6,
      "value_dec": "600",
      "value": "02258"
    },
    {
      "currency": 7,
      "value_dec": "10000100",
      "value": "059896e4"
    }
  ],
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBBQEANgABDz/////////0AQEPP/////////QCAQ8/P///////9AMBDw/////////0BAAPP/8///////Q=",
  "code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a",
  "data": "te6ccgEBAQEACgAADz//H//////0",
  "data_hash": "47cc6bba530c25a982969baf59254598715aecb5b9d14531d96d24d8a623dd93",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgBAA8/////////9A==",
  "library_hash": "4359e3721d98903035218ff07d3df30d0ce59d224abd2d7b0bfe65423fb0f67f",
  "acc_type": 1
}"#
    );
}

#[test]
fn test_account_into_json_q() {
    let account = generate_sample_account();
    let json = db_serialize_account_ex("id", &account, SerializationMode::QServer).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "workchain_id": 0,
  "boc": "te6ccgEBFgEA8gAEdcAAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fISgMxAOt5orFvAAAAAAAAAABRdIdugA974AQIDBAIDzsAFBgEPP/////////QRAA8//x//////9AFDoA9Bcq9CvSeZR50tmWldnk60bjFEx5FdlFVin83DzELlmBUCASAHCAIBIAsMAAVQskACASAJCgADA5EABQQCWQIBIA0OAgEgDxAABQQDIQAFBAPpAAUEBLEABwcxLckBDz/////////0EgEPPz////////QTAQ8P////////9BQADz//P//////0AA8/////////9A==",
  "last_paid": 123456789,
  "bits": "0x331",
  "cells": "0x9",
  "due_payment": "0x6f",
  "last_trans_lt": "0x0",
  "balance": "0x174876e800",
  "balance_other": [
    {
      "currency": 1,
      "value": "0x64"
    },
    {
      "currency": 2,
      "value": "0xc8"
    },
    {
      "currency": 3,
      "value": "0x12c"
    },
    {
      "currency": 4,
      "value": "0x190"
    },
    {
      "currency": 5,
      "value": "0x1f4"
    },
    {
      "currency": 6,
      "value": "0x258"
    },
    {
      "currency": 7,
      "value": "0x9896e4"
    }
  ],
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBBQEANgABDz/////////0AQEPP/////////QCAQ8/P///////9AMBDw/////////0BAAPP/8///////Q=",
  "code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a",
  "data": "te6ccgEBAQEACgAADz//H//////0",
  "data_hash": "47cc6bba530c25a982969baf59254598715aecb5b9d14531d96d24d8a623dd93",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgBAA8/////////9A==",
  "library_hash": "4359e3721d98903035218ff07d3df30d0ce59d224abd2d7b0bfe65423fb0f67f",
  "acc_type": 1,
  "acc_type_name": "Active"
}"#
    );
}

#[test]
fn test_frozen_account_into_json_0() {
    let account = generate_sample_frozen_account();
    let json = db_serialize_account("id", &account).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "workchain_id": 0,
  "boc": "te6ccgEBDgEAnwABs8AAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fICgFvAOt5orFvAAAAAAAAAABRdIdugAunN4KZIi5byQcakWhKcrtTk46OydPaN44Gl2Jbfw6CKQAECA87AAgMCASAEBQIBIAgJAAVQskACASAGBwADA5EABQQCWQIBIAoLAgEgDA0ABQQDIQAFBAPpAAUEBLEABwcxLck=",
  "last_paid": 123456789,
  "bits_dec": "367",
  "bits": "216f",
  "cells_dec": "1",
  "cells": "01",
  "due_payment_dec": "111",
  "due_payment": "016f",
  "last_trans_lt_dec": "0",
  "last_trans_lt": "00",
  "balance_dec": "100000000000",
  "balance": "09174876e800",
  "balance_other": [
    {
      "currency": 1,
      "value_dec": "100",
      "value": "0164"
    },
    {
      "currency": 2,
      "value_dec": "200",
      "value": "01c8"
    },
    {
      "currency": 3,
      "value_dec": "300",
      "value": "0212c"
    },
    {
      "currency": 4,
      "value_dec": "400",
      "value": "02190"
    },
    {
      "currency": 5,
      "value_dec": "500",
      "value": "021f4"
    },
    {
      "currency": 6,
      "value_dec": "600",
      "value": "02258"
    },
    {
      "currency": 7,
      "value_dec": "10000100",
      "value": "059896e4"
    }
  ],
  "state_hash": "d39bc14c91172de4838d48b425395da9c9c74764e9ed1bc7034bb12dbf874114",
  "acc_type": 2,
  "prev_code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a"
}"#
    );
}

#[test]
fn test_frozen_account_into_json_q() {
    let account = generate_sample_frozen_account();
    let json = db_serialize_account_ex("id", &account, SerializationMode::QServer).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "workchain_id": 0,
  "boc": "te6ccgEBDgEAnwABs8AAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fICgFvAOt5orFvAAAAAAAAAABRdIdugAunN4KZIi5byQcakWhKcrtTk46OydPaN44Gl2Jbfw6CKQAECA87AAgMCASAEBQIBIAgJAAVQskACASAGBwADA5EABQQCWQIBIAoLAgEgDA0ABQQDIQAFBAPpAAUEBLEABwcxLck=",
  "last_paid": 123456789,
  "bits": "0x16f",
  "cells": "0x1",
  "due_payment": "0x6f",
  "last_trans_lt": "0x0",
  "balance": "0x174876e800",
  "balance_other": [
    {
      "currency": 1,
      "value": "0x64"
    },
    {
      "currency": 2,
      "value": "0xc8"
    },
    {
      "currency": 3,
      "value": "0x12c"
    },
    {
      "currency": 4,
      "value": "0x190"
    },
    {
      "currency": 5,
      "value": "0x1f4"
    },
    {
      "currency": 6,
      "value": "0x258"
    },
    {
      "currency": 7,
      "value": "0x9896e4"
    }
  ],
  "state_hash": "d39bc14c91172de4838d48b425395da9c9c74764e9ed1bc7034bb12dbf874114",
  "acc_type": 2,
  "acc_type_name": "Frozen",
  "prev_code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a"
}"#
    );
}

#[test]
fn test_pruned_account_into_json_0() {
    let account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let code = account.get_code().map(|cell| cell.repr_hash());
    let libs = account.libraries().root().map(|cell| cell.repr_hash());
    let cell = account.serialize().unwrap();
    let proof = MerkleProof::create(&cell, |hash| {
        Some(hash) != code.as_ref() && Some(hash) != libs.as_ref()
    })
    .unwrap();
    let account = proof.virtualize().unwrap();
    let boc = proof.write_to_bytes().unwrap();
    let sender = AccountSerializationSet { account, prev_code_hash: None, boc, proof: None };
    let json = db_serialize_account("id", &sender).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "workchain_id": 0,
  "boc": "te6ccgEBEgEA/wAJRgMD3nR55pLP1heO3c90S64tvvvZVo3e6lH/kuIzsABZjwAFASR1wAAAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8hKAzEA63misW8AAAAAAAAAAFF0h26AD3vgCAwQFAgPOwAYHKEgBATwoFk8ht2pTz+c1EBl7mcc11Nl7ZS5pUPMXvL/pVYSKAAQADz//H//////0KEgBAUNZ43IdmJAwNSGP8H098w0M5Z0iSr0tewv+ZUI/sPZ/AAECASAICQIBIAwNAAVQskACASAKCwADA5EABQQCWQIBIA4PAgEgEBEABQQDIQAFBAPpAAUEBLEABwcxLck=",
  "last_paid": 123456789,
  "bits_dec": "817",
  "bits": "2331",
  "cells_dec": "9",
  "cells": "09",
  "due_payment_dec": "111",
  "due_payment": "016f",
  "last_trans_lt_dec": "0",
  "last_trans_lt": "00",
  "balance_dec": "100000000000",
  "balance": "09174876e800",
  "balance_other": [
    {
      "currency": 1,
      "value_dec": "100",
      "value": "0164"
    },
    {
      "currency": 2,
      "value_dec": "200",
      "value": "01c8"
    },
    {
      "currency": 3,
      "value_dec": "300",
      "value": "0212c"
    },
    {
      "currency": 4,
      "value_dec": "400",
      "value": "02190"
    },
    {
      "currency": 5,
      "value_dec": "500",
      "value": "021f4"
    },
    {
      "currency": 6,
      "value_dec": "600",
      "value": "02258"
    },
    {
      "currency": 7,
      "value_dec": "10000100",
      "value": "059896e4"
    }
  ],
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a",
  "data": "te6ccgEBAQEACgAADz//H//////0",
  "data_hash": "47cc6bba530c25a982969baf59254598715aecb5b9d14531d96d24d8a623dd93",
  "library_hash": "4359e3721d98903035218ff07d3df30d0ce59d224abd2d7b0bfe65423fb0f67f",
  "acc_type": 1
}"#
    );
}

#[test]
fn test_message_into_json_0() {
    let msg = generate_sample_message();
    let json = db_serialize_message("id", &msg).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "59bf855c9fbee1152e1e151368f5af5850f22f606c819c43adb2fb319e07a4c8",
  "boc": "te6ccgEBAwEAZgACZpFACBAYICwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQ+3tH/////////gIBAUOgD0Fyr0K9J5lHnS2ZaV2eTrRuMUTHkV2UVWKfzcPMQuWIAgAPP/////////Q=",
  "status": 2,
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBAQEACgAADz/////////0",
  "code_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5YgBAA8/////////9A==",
  "library_hash": "c39760fbba54774b6c7fa76bfd46d6fb89d1fe0b19570bef3c4d08decc8b4566",
  "body": "te6ccgEBAQEACgAADz/////////0",
  "body_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "msg_type": 1,
  "src": ":0102030405",
  "dst": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "dst_workchain_id": 0,
  "import_fee_dec": "15",
  "import_fee": "00f",
  "created_at": 123
}"#
    );
}

#[test]
fn test_message_into_json_q() {
    let msg = generate_sample_message();
    let json = db_serialize_message_ex("id", &msg, SerializationMode::QServer).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "59bf855c9fbee1152e1e151368f5af5850f22f606c819c43adb2fb319e07a4c8",
  "boc": "te6ccgEBAwEAZgACZpFACBAYICwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQ+3tH/////////gIBAUOgD0Fyr0K9J5lHnS2ZaV2eTrRuMUTHkV2UVWKfzcPMQuWIAgAPP/////////Q=",
  "status": 2,
  "status_name": "processing",
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBAQEACgAADz/////////0",
  "code_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5YgBAA8/////////9A==",
  "library_hash": "c39760fbba54774b6c7fa76bfd46d6fb89d1fe0b19570bef3c4d08decc8b4566",
  "body": "te6ccgEBAQEACgAADz/////////0",
  "body_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "msg_type": 1,
  "msg_type_name": "extIn",
  "src": ":0102030405",
  "dst": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "dst_workchain_id": 0,
  "import_fee": "0xf",
  "created_at": 123
}"#
    );
}

#[test]
fn test_transaction_wo_out_msgs_into_json() {
    let mut transaction = generate_transaction([55; 32].into());
    transaction.out_msgs = OutMessages::default();
    let cell = transaction.serialize().unwrap();
    let boc = write_boc(&cell).unwrap();
    let id = transaction.hash().unwrap();
    let tr = TransactionSerializationSetEx {
        transaction: &transaction,
        id: &id,
        status: TransactionProcessingStatus::Preliminary,
        block_id: None,
        workchain_id: None,
        boc: &boc,
        proof: None,
    };
    let json = db_serialize_transaction("id", tr).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "e3b749754b59906d07223d3ede2a901a03f75f2371f04f504cb2f863eabb8a48",
  "boc": "te6ccgECDgEAAuIAA7Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3AAAAAAAB4h8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHJAUagBAgMBAaAEAIJyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADAAIEY0IAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAt78BQsGBwEPP/////////QIAUOgD0Fyr0K9J5lHnS2ZaV2eTrRuMUTHkV2UVWKfzcPMQuWYCwHe////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////DAFFrcm6/FaUEVZY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1EJAUWtybr8VpQRVlj6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQoARa3JuvxWlBFXWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tRAA8/////////9AHe/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+DQDepqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqam",
  "status": 1,
  "compute": {
    "skipped_reason": 0,
    "compute_type": 0
  },
  "credit_first": false,
  "aborted": false,
  "destroyed": false,
  "tr_type": 0,
  "lt_dec": "123423",
  "lt": "41e21f",
  "prev_trans_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "prev_trans_lt_dec": "0",
  "prev_trans_lt": "00",
  "now": 0,
  "outmsg_cnt": 3,
  "orig_status": 1,
  "end_status": 2,
  "in_msg": "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
  "out_msgs": [],
  "account_addr": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "workchain_id": 0,
  "total_fees_dec": "653",
  "total_fees": "0228d",
  "balance_delta_dec": "-653",
  "balance_delta": "-fdd72",
  "old_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "new_hash": "0000000000000000000000000000000000000000000000000000000000000000"
}"#
    );
}

#[test]
fn test_transaction_into_json_0() {
    let (msg, tr) = generate_sample_transaction(false);
    let json = db_serialize_message("id", &msg).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
  "transaction_id": "c5e2abf8035f793ade1573078c2d772d11d0e7d1364004677a5ab9367359737d",
  "boc": "te6ccgECCgEAAjgABGNCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAALe/AEHAgMBDz/////////0BAFDoA9Bcq9CvSeZR50tmWldnk60bjFEx5FdlFVin83DzELlmAcB3v///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////wgBRa3JuvxWlBFWWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tRBQFFrcm6/FaUEVZY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1EGAEWtybr8VpQRV1j6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQAPP/////////QB3v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/gkA3qampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampg==",
  "status": 2,
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBBAEAfAABDz/////////0AQFFrcm6/FaUEVZY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1ECAUWtybr8VpQRVlj6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQMARa3JuvxWlBFXWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tR",
  "code_hash": "360f4a95e55ffb03c422b80f244e624bce83701769594d1f28fee6675365c649",
  "data": "te6ccgEBAQEACgAADz/////////0",
  "data_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgBAA8/////////9A==",
  "library_hash": "4359e3721d98903035218ff07d3df30d0ce59d224abd2d7b0bfe65423fb0f67f",
  "body": "te6ccgECAwEAAVUAAd7///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////8BAd7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v4CAN6mpqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqY=",
  "body_hash": "7cedc673b096999b859bad6d552c4574bec3c5aafca70fe586794cc6aff7326b",
  "msg_type": 0,
  "src": "",
  "dst": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "dst_workchain_id": 0,
  "ihr_disabled": true,
  "extra_flags_dec": "0",
  "extra_flags": "000",
  "fwd_fee_dec": "0",
  "fwd_fee": "000",
  "bounce": false,
  "bounced": false,
  "value_dec": "0",
  "value": "000",
  "created_lt_dec": "0",
  "created_lt": "00",
  "created_at": 0
}"#
    );

    let json = db_serialize_transaction("id", &tr).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "c5e2abf8035f793ade1573078c2d772d11d0e7d1364004677a5ab9367359737d",
  "boc": "te6ccgECFQEAA10AA7Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3AAAAAAAB4h8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHJAUagBAgMCAeAJBACCcgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAwACAgHbBQYCASAHCAEBSBQBASAJAQEgEwRjQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAC3vwKEAsMAQ8/////////9A0BQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgQAd7///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////8RAUWtybr8VpQRVlj6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQ4BRa3JuvxWlBFWWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tRDwBFrcm6/FaUEVdY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1EADz/////////0Ad7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v4SAN6mpqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqYAYEIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAAAAABgQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAAA",
  "status": 1,
  "compute": {
    "skipped_reason": 0,
    "compute_type": 0
  },
  "credit_first": false,
  "aborted": false,
  "destroyed": false,
  "tr_type": 0,
  "lt_dec": "123423",
  "lt": "41e21f",
  "prev_trans_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "prev_trans_lt_dec": "0",
  "prev_trans_lt": "00",
  "now": 0,
  "outmsg_cnt": 3,
  "orig_status": 1,
  "end_status": 2,
  "in_msg": "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
  "out_msgs": [
    "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
    "274af286c6cdcfcd78e291a5143e7a09ab04fab51bf03e0b210bde025ada77b0",
    "382593470147adb03e6bbae5dc26ad33fa07136434556181001b3e9f9d7ca303"
  ],
  "account_addr": "-1:3737373737373737373737373737373737373737373737373737373737373737",
  "workchain_id": -1,
  "total_fees_dec": "653",
  "total_fees": "0228d",
  "balance_delta_dec": "-653",
  "balance_delta": "-fdd72",
  "old_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "new_hash": "0000000000000000000000000000000000000000000000000000000000000000"
}"#
    );
}

#[test]
fn test_transaction_into_json_q() {
    let (msg, tr) = generate_sample_transaction(true);
    let json = db_serialize_message_ex("id", &msg, SerializationMode::QServer).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "2b5933e5a6cee439f79fdc6fff7ba6e8ca2b09006999b2db34507613acbab44c",
  "transaction_id": "c5e2abf8035f793ade1573078c2d772d11d0e7d1364004677a5ab9367359737d",
  "boc": "te6ccgECCgEAAlkABKVIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFvfgEHAgMBDz/////////0BAFDoA9Bcq9CvSeZR50tmWldnk60bjFEx5FdlFVin83DzELlmAcB3v///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////wgBRa3JuvxWlBFWWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tRBQFFrcm6/FaUEVZY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1EGAEWtybr8VpQRV1j6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQAPP/////////QB3v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/gkA3qampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampg==",
  "status": 2,
  "status_name": "processing",
  "fixed_prefix_length": 23,
  "tick": false,
  "tock": true,
  "code": "te6ccgEBBAEAfAABDz/////////0AQFFrcm6/FaUEVZY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1ECAUWtybr8VpQRVlj6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQMARa3JuvxWlBFXWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tR",
  "code_hash": "360f4a95e55ffb03c422b80f244e624bce83701769594d1f28fee6675365c649",
  "data": "te6ccgEBAQEACgAADz/////////0",
  "data_hash": "7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c",
  "library": "te6ccgEBAgEALwABQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgBAA8/////////9A==",
  "library_hash": "4359e3721d98903035218ff07d3df30d0ce59d224abd2d7b0bfe65423fb0f67f",
  "body": "te6ccgECAwEAAVUAAd7///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////8BAd7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v4CAN6mpqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqY=",
  "body_hash": "7cedc673b096999b859bad6d552c4574bec3c5aafca70fe586794cc6aff7326b",
  "msg_type": 0,
  "msg_type_name": "internal",
  "src": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "src_workchain_id": 0,
  "dst": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "dst_workchain_id": 0,
  "ihr_disabled": true,
  "extra_flags": "0x0",
  "fwd_fee": "0x0",
  "bounce": false,
  "bounced": false,
  "value": "0x0",
  "created_lt": "0x0",
  "created_at": 0
}"#
    );

    let json = db_serialize_transaction_ex("id", &tr, SerializationMode::QServer).unwrap();
    println!("\n\n{:#}", serde_json::json!(json));
    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(json)),
        r#"{
  "json_version": 8,
  "id": "c5e2abf8035f793ade1573078c2d772d11d0e7d1364004677a5ab9367359737d",
  "boc": "te6ccgECFQEAA10AA7Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3Nzc3AAAAAAAB4h8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHJAUagBAgMCAeAJBACCcgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAwACAgHbBQYCASAHCAEBSBQBASAJAQEgEwRjQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAC3vwKEAsMAQ8/////////9A0BQ6APQXKvQr0nmUedLZlpXZ5OtG4xRMeRXZRVYp/Nw8xC5ZgQAd7///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////8RAUWtybr8VpQRVlj6K9/kZRUaMgNpSv/NAI82i9LMjMgQ+2tbUQ4BRa3JuvxWlBFWWPor3+RlFRoyA2lK/80AjzaL0syMyBD7a1tRDwBFrcm6/FaUEVdY+ivf5GUVGjIDaUr/zQCPNovSzIzIEPtrW1EADz/////////0Ad7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v7+/v4SAN6mpqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqampqYAYEIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAAAAABgQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAAA",
  "status": 1,
  "status_name": "preliminary",
  "compute": {
    "skipped_reason": 0,
    "skipped_reason_name": "noState",
    "compute_type": 0,
    "compute_type_name": "skipped"
  },
  "credit_first": false,
  "aborted": false,
  "destroyed": false,
  "tr_type": 0,
  "tr_type_name": "ordinary",
  "lt": "0x1e21f",
  "prev_trans_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "prev_trans_lt": "0x0",
  "now": 0,
  "outmsg_cnt": 3,
  "orig_status": 1,
  "orig_status_name": "Active",
  "end_status": 2,
  "end_status_name": "Frozen",
  "in_msg": "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
  "out_msgs": [
    "d56066036810aed3ac77c6291bc4e8ece4d3b8d7d51e4cc8c127db17ba676bee",
    "274af286c6cdcfcd78e291a5143e7a09ab04fab51bf03e0b210bde025ada77b0",
    "382593470147adb03e6bbae5dc26ad33fa07136434556181001b3e9f9d7ca303"
  ],
  "account_addr": "-1:3737373737373737373737373737373737373737373737373737373737373737",
  "workchain_id": -1,
  "total_fees": "0x28d",
  "balance_delta": "-0x28d",
  "old_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "new_hash": "0000000000000000000000000000000000000000000000000000000000000000"
}"#
    );
}

fn test_json_block(blockhash: &str, mode: SerializationMode) {
    let filename = format!("{}.boc", blockhash);
    let in_path = Path::new("src/tests/data").join(&filename);
    let boc = read(in_path.clone()).unwrap_or_else(|_| panic!("Error reading file {:?}", in_path));
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let block = Block::construct_from_cell(cell).unwrap();
    let id = block.hash().unwrap();
    let block = BlockSerializationSet { block, id, status: BlockProcessingStatus::Proposed, boc };

    let json =
        format!("{:#}", serde_json::json!(db_serialize_block_ex("id", &block, mode).unwrap()));
    let filename =
        format!("{}{}", blockhash, if let SerializationMode::QServer = mode { "-Q" } else { "" });
    assert_json_eq_file(&json, &filename);
}

#[test]
fn test_get_config() {
    let filename =
        "src/tests/data/9C9906A80D020952E0192DC60C0B2BF1F55FE9A9E065606E8FE25C08BD1AA6B2.boc";
    let in_path = Path::new(filename);
    let boc = read(in_path).unwrap_or_else(|_| panic!("Error reading file {:?}", filename));
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let block = Block::construct_from_cell(cell).unwrap();
    let extra = block.read_extra().unwrap();
    let master = extra.read_custom().unwrap().unwrap();
    let config = master.config().unwrap();
    let json = serialize_config_param(config, 12).unwrap();
    let etalon = r#"{
  "p12": [
    {
      "workchain_id": 0,
      "enabled_since": 1573821854,
      "monitor_min_split": 0,
      "min_split": 2,
      "max_split": 32,
      "active": true,
      "accept_msgs": true,
      "flags": 0,
      "zerostate_root_hash": "55b13f6d0e1d0c34c9c2160f6f918e92d82bf9ddcf8de2e4c94a3fdf39d15446",
      "zerostate_file_hash": "ee0bedfe4b32761fb35e9e1d8818ea720cad1a0e7b4d2ed673c488e72e910342",
      "version": 0,
      "basic": true,
      "vm_version": -1,
      "vm_mode": 0
    }
  ]
}"#;
    pretty_assertions::assert_eq!(etalon, json);
    /*if json != etalon {
      std::fs::write("real_data/p12-config-param.json", &json).unwrap();
      panic!("json != etalon")
    }*/
}

#[test]
fn test_block_into_json_1() {
    test_json_block(
        "89ED400A43E76664437EFC9C79B84AC387493A9EE5E789338FF71C25F54218BE",
        SerializationMode::Standart,
    )
}

#[test]
fn test_block_into_json_2() {
    test_json_block(
        "18AFCDD25BE0989CE516504263EB351818A0FF8F6AB3689501C8E3B767EF413C",
        SerializationMode::Standart,
    )
}

#[test]
fn test_block_into_json_3() {
    test_json_block(
        "046784ea72574ace66375629229700afa4c7e032a360fc94df4c20231fddea45",
        SerializationMode::Standart,
    )
}

#[test]
fn test_block_into_json_q() {
    test_json_block(
        "89ED400A43E76664437EFC9C79B84AC387493A9EE5E789338FF71C25F54218BE",
        SerializationMode::QServer,
    )
}

#[test]
fn test_key_block_into_json() {
    test_json_block(
        "9C9906A80D020952E0192DC60C0B2BF1F55FE9A9E065606E8FE25C08BD1AA6B2",
        SerializationMode::Standart,
    )
}

fn get_validator_set() -> ValidatorSet {
    let keydat = base64_decode("7w3fX5jiuo8PyQoFaEL+K9pE/XvbKjH63i0JcraLlBM=").unwrap();
    let key = SigPubKey::from_bytes(&keydat).unwrap();
    let vd1 = ValidatorDescr::with_params(key, 1, None);
    let key = SigPubKey::from_bytes(&keydat).unwrap();
    let vd2 = ValidatorDescr::with_params(key, 2, None);
    ValidatorSet::new(1234567, 39237233, 1, vec![vd1, vd2]).unwrap()
}

fn get_config_param11() -> ConfigParam11 {
    let normal_params = ConfigProposalSetup {
        min_tot_rounds: 1,
        max_tot_rounds: 2,
        min_wins: 3,
        max_losses: 4,
        min_store_sec: 5,
        max_store_sec: 6,
        bit_price: 7,
        cell_price: 8,
    };
    let critical_params = ConfigProposalSetup {
        min_tot_rounds: 10,
        max_tot_rounds: 20,
        min_wins: 30,
        max_losses: 40,
        min_store_sec: 50000,
        max_store_sec: 60000,
        bit_price: 70000,
        cell_price: 80000,
    };
    ConfigVotingSetup::new(&normal_params, &critical_params).unwrap()
}

#[test]
fn test_crafted_key_block_into_json() {
    let filename =
        "src/tests/data/48377CD82FF8091D6A45908727C8D4E5FC521603E5633AF3AC8C9E45F9579D5B.boc";
    let in_path = Path::new(filename);
    let boc = read(in_path).unwrap_or_else(|_| panic!("Error reading file {:?}", filename));
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    // println!("slice = {}", root_cell);
    let key = base64_decode("7w3fX5jiuo8PyQoFaEL+K9pE/XvbKjH63i0JcraLlBM=").unwrap();
    // ef0ddf5f98e2ba8f0fc90a056842fe2bda44fd7bdb2a31fade2d0972b68b9413
    let mut block = Block::construct_from_cell(cell).unwrap();

    // Need to add next config params: 3 4 6 9 33 35 36 37 39

    let cp3 = ConfigParamEnum::ConfigParam3(ConfigParam3 { fee_collector_addr: [133; 32].into() });
    let cp4 = ConfigParamEnum::ConfigParam4(ConfigParam4 { dns_root_addr: [144; 32].into() });
    let cp6 = ConfigParamEnum::ConfigParam6(ConfigParam6 {
        mint_new_price: 123.into(),
        mint_add_price: 1458347523.into(),
    });
    let cp9 = ConfigParamEnum::ConfigParam9({
        let mut mp = MandatoryParams::default();
        for i in 1..10u32 {
            mp.add_key(&i).unwrap();
        }
        ConfigParam9 { mandatory_params: mp }
    });
    let cp11 = ConfigParamEnum::ConfigParam11(get_config_param11());

    let mut cp33 = ConfigParam33::new();
    cp33.prev_temp_validators = get_validator_set();
    let cp33 = ConfigParamEnum::ConfigParam33(cp33);

    let mut cp35 = ConfigParam35::new();
    cp35.cur_temp_validators = get_validator_set();
    let cp35 = ConfigParamEnum::ConfigParam35(cp35);

    let mut cp36 = ConfigParam36::new();
    cp36.next_validators = get_validator_set();
    let cp36 = ConfigParamEnum::ConfigParam36(cp36);

    let mut cp37 = ConfigParam37::new();
    cp37.next_temp_validators = get_validator_set();
    let cp37 = ConfigParamEnum::ConfigParam37(cp37);

    let cp39 = ConfigParamEnum::ConfigParam39({
        let mut cp = ConfigParam39::new();

        let spk = SigPubKey::from_bytes(&key).unwrap();
        let cs = CryptoSignature::with_r_s(&[1; 32], &[2; 32]);
        let vtk = ValidatorTempKey::with_params(UInt256::from([3; 32]), spk, 100500, 1562663724);
        let vstk = ValidatorSignedTempKey::with_key_and_signature(vtk, cs);
        cp.insert(&UInt256::from([1; 32]), &vstk).unwrap();

        let spk = SigPubKey::from_bytes(&key).unwrap();
        let cs = CryptoSignature::with_r_s(&[6; 32], &[7; 32]);
        let vtk = ValidatorTempKey::with_params(UInt256::from([8; 32]), spk, 500100, 1562664724);
        let vstk = ValidatorSignedTempKey::with_key_and_signature(vtk, cs);
        cp.insert(&UInt256::from([2; 32]), &vstk).unwrap();

        cp
    });

    let mut suspended = SuspendedAddressList::default();
    suspended.set_suspended_until(1742976363);
    suspended.add_suspended_address(0, [0xFF; 32].into()).unwrap();
    suspended.add_suspended_address(-1, [0; 32].into()).unwrap();
    let cp44 = ConfigParamEnum::ConfigParam44(suspended);

    let mut precompiled = PrecompiledContractsList::default();
    precompiled.add(&[0; 32].into(), 157).unwrap();
    precompiled.add(&[0xFF; 32].into(), 300).unwrap();
    let cp45 = ConfigParamEnum::ConfigParam45(precompiled);

    let mut oracles = Oracles::new();
    oracles
        .set_raw([0x53; 32].write_to_bitstring().unwrap(), &[0x37; 32].write_to_new_cell().unwrap())
        .unwrap();
    oracles
        .set_raw([0x73; 32].write_to_bitstring().unwrap(), &[0x17; 32].write_to_new_cell().unwrap())
        .unwrap();
    let oracle_bridge_params = OracleBridgeParams {
        oracles,
        bridge_address: [0x11; 32].into(),
        oracle_mutlisig_address: [0x22; 32].into(),
        external_chain_address: [0x33; 32].into(),
    };

    let mut oracles = Oracles::new();
    oracles
        .set_raw([0x17; 32].write_to_bitstring().unwrap(), &[0x28; 32].write_to_new_cell().unwrap())
        .unwrap();
    oracles
        .set_raw([0xCD; 32].write_to_bitstring().unwrap(), &[0xF0; 32].write_to_new_cell().unwrap())
        .unwrap();
    let jetton_bridge_params = JettonBridgeParams {
        oracles,
        prices: JettonBridgePrices {
            bridge_burn_fee: 1000.into(),
            bridge_mint_fee: 2000.into(),
            wallet_min_tons_for_storage: 3000.into(),
            wallet_gas_consumption: 4000.into(),
            minter_min_tons_for_storage: 5000.into(),
            discover_gas_consumption: 6000.into(),
        },
        state_flags: 17,
        bridge_address: [0x34; 32].into(),
        oracles_address: [0x45; 32].into(),
        external_chain_address: [0x56; 32].into(),
    };

    let mut extra = block.read_extra().unwrap();
    let mut custom = extra.read_custom().unwrap().unwrap();

    // Need to add prev_block_signatures
    let cs = CryptoSignature::with_r_s(&[1; 32], &[2; 32]);
    let csp = CryptoSignaturePair::with_params(UInt256::from([12; 32]), cs.clone());
    custom.prev_blk_signatures_mut().set(&123_u16, &csp).unwrap();
    custom.prev_blk_signatures_mut().set(&345_u16, &csp).unwrap();

    // Need to add shard with FutureSplitMerge
    let sd = ShardDescr::with_params(
        42,
        17,
        25,
        UInt256::from_le_bytes(&[70]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let mut wc0 = custom.hashes().get(&0_u32).unwrap().unwrap();
    let mut key = ton_block::BuilderData::new();
    key.append_bit_one().unwrap();
    key.append_bit_one().unwrap();
    let key = SliceData::load_builder(key).unwrap();
    wc0.0.split(key, |old| Ok((old, sd))).unwrap();
    custom.hashes_mut().set(&0_u32, &wc0).unwrap();

    pretty_assertions::assert_eq!(custom.prev_blk_signatures().len().unwrap(), 2);

    let config_params = custom.config_mut().as_mut().unwrap();

    config_params.set_config(cp3).unwrap();
    config_params.set_config(cp4).unwrap();
    config_params.set_config(cp6).unwrap();
    config_params.set_config(cp9).unwrap();
    config_params.set_config(cp11).unwrap();
    config_params.set_config(cp33).unwrap();
    config_params.set_config(cp35).unwrap();
    config_params.set_config(cp36).unwrap();
    config_params.set_config(cp37).unwrap();
    config_params.set_config(cp39).unwrap();
    config_params.set_config(cp44).unwrap();
    config_params.set_config(cp45).unwrap();
    config_params.set_config(ConfigParamEnum::ConfigParam71(oracle_bridge_params)).unwrap();
    config_params.set_config(ConfigParamEnum::ConfigParam79(jetton_bridge_params)).unwrap();

    extra.write_custom(&custom).unwrap();
    block.write_extra(&extra).unwrap();

    let id = block.hash().unwrap();
    let block = BlockSerializationSet { block, id, status: BlockProcessingStatus::Proposed, boc };

    let json = db_serialize_block("id", &block).unwrap();
    let json = format!("{:#}", serde_json::json!(json));

    assert_json_eq_file(&json, "crafted-key-block");
}

#[test]
fn test_db_serialize_block_signatures() {
    let doc = serde_json::to_string_pretty(&serde_json::json!(db_serialize_block_signatures(
        "_id",
        &UInt256::from([1; 32]),
        &[
            CryptoSignaturePair::with_params(
                UInt256::from([2; 32]),
                CryptoSignature::with_r_s(&[3; 32], &[4; 32])
            ),
            CryptoSignaturePair::with_params(
                UInt256::from([5; 32]),
                CryptoSignature::with_r_s(&[6; 32], &[7; 32])
            )
        ]
    )
    .unwrap()))
    .unwrap();
    println!("{}", doc);

    pretty_assertions::assert_eq!(
        doc,
        r#"{
  "json_version": 8,
  "_id": "0101010101010101010101010101010101010101010101010101010101010101",
  "signatures": [
    {
      "node_id": "0202020202020202020202020202020202020202020202020202020202020202",
      "r": "0303030303030303030303030303030303030303030303030303030303030303",
      "s": "0404040404040404040404040404040404040404040404040404040404040404"
    },
    {
      "node_id": "0505050505050505050505050505050505050505050505050505050505050505",
      "r": "0606060606060606060606060606060606060606060606060606060606060606",
      "s": "0707070707070707070707070707070707070707070707070707070707070707"
    }
  ]
}"#
    )
}

#[test]
fn test_serialize_shard_descr() {
    let sd = ShardDescr::default();
    let doc = serialize_shard_descr(&sd, SerializationMode::Standart).unwrap();
    print!("{}", serde_json::to_string_pretty(&doc).unwrap());
    pretty_assertions::assert_eq!(
        doc,
        serde_json::from_str::<serde_json::Value>(
            r#"
    {
      "seq_no": 0,
      "reg_mc_seqno": 0,
      "start_lt_dec": "0",
      "start_lt": "00",
      "end_lt_dec": "0",
      "end_lt": "00",
      "root_hash": "0000000000000000000000000000000000000000000000000000000000000000",
      "file_hash": "0000000000000000000000000000000000000000000000000000000000000000",
      "before_split": false,
      "before_merge": false,
      "want_split": false,
      "want_merge": false,
      "nx_cc_updated": false,
      "gen_utime": 0,
      "next_catchain_seqno": 0,
      "next_validator_shard": "0000000000000000",
      "min_ref_mc_seqno": 0,
      "fees_collected_dec": "0",
      "fees_collected": "000",
      "funds_created_dec": "0",
      "funds_created": "000",
      "flags": 0
    }
    "#
        )
        .unwrap()
    );
}

#[test]
fn test_db_serialize_block_proof() {
    let boc = read("src/tests/data/block_proof").expect("Error reading proof file");
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let proof = BlockProof::construct_from_cell(cell).unwrap();
    let json = serde_json::to_string_pretty(&serde_json::json!(db_serialize_block_proof(
        "_id", &proof
    )
    .unwrap()))
    .unwrap();
    assert_json_eq_file(&json, "proof");
}

/// Test that serialization of ordinary signatures includes signature_type field
/// This ensures new serialization is compatible with new deserialization
#[test]
fn test_db_serialize_block_proof_includes_signature_type() {
    let boc = read("src/tests/data/block_proof").expect("Error reading proof file");
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let proof = BlockProof::construct_from_cell(cell).unwrap();

    let map = db_serialize_block_proof("_id", &proof).unwrap();

    // Verify signature_type is present and set to "ordinary"
    assert!(map.contains_key("signature_type"), "signature_type field should be present");
    assert_eq!(map.get("signature_type").unwrap(), "ordinary");
}

/// Test backward compatibility: verify that serialized JSON can be parsed back
/// This tests the full roundtrip through serialize -> deserialize
#[test]
fn test_block_proof_serialize_deserialize_roundtrip_ordinary() {
    use ton_block::BlockSignaturesVariant;

    let boc = read("src/tests/data/block_proof").expect("Error reading proof file");
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let original_proof = BlockProof::construct_from_cell(cell).unwrap();

    // Serialize to JSON
    let json_map = db_serialize_block_proof("_id", &original_proof).unwrap();

    // Deserialize back
    let parsed_proof =
        crate::parse_block_proof(&json_map, original_proof.proof_for.file_hash.clone()).unwrap();

    // Verify the proof structure matches
    assert_eq!(parsed_proof.proof_for, original_proof.proof_for);

    // Verify signatures are Ordinary and match
    match (original_proof.signatures.as_ref(), parsed_proof.signatures.as_ref()) {
        (
            Some(BlockSignaturesVariant::Ordinary(orig)),
            Some(BlockSignaturesVariant::Ordinary(parsed)),
        ) => {
            assert_eq!(orig.validator_info, parsed.validator_info);
            assert_eq!(orig.pure_signatures.weight(), parsed.pure_signatures.weight());
        }
        _ => panic!("Both should be Ordinary variants"),
    }

    // Verify binary representation matches
    assert_eq!(original_proof.write_to_bytes().unwrap(), parsed_proof.write_to_bytes().unwrap());
}

#[test]
fn test_db_serialize_block_proof_simplex() {
    use ton_block::{
        BlockProof, BlockSignaturesPure, BlockSignaturesSimplex, BlockSignaturesVariant,
        CryptoSignature, CryptoSignaturePair, UInt256, ValidatorBaseInfo,
    };

    // Load a real block proof from test data
    let boc = read("src/tests/data/block_proof").expect("Error reading proof file");
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let original_proof = BlockProof::construct_from_cell(cell).unwrap();

    // Create pure signatures (copy weight from original)
    let original_weight =
        original_proof.signatures.as_ref().map(|s| s.pure_signatures().weight()).unwrap_or(0);
    let mut pure_signatures = BlockSignaturesPure::new();
    pure_signatures.set_weight(original_weight);
    pure_signatures.add_sigpair(CryptoSignaturePair {
        node_id_short: UInt256::from([0x11; 32]),
        sign: CryptoSignature::with_r_s(&[0x22; 32], &[0x33; 32]),
    });

    // Create simplex-specific data
    let validator_info = ValidatorBaseInfo::with_params(12345, 6789);
    let session_id = UInt256::from([0xAA; 32]);
    let slot = 42u32;
    let candidate_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    let simplex_sigs = BlockSignaturesSimplex::new_finalize(
        validator_info,
        pure_signatures,
        session_id,
        slot,
        data,
    );

    // Create a proof with simplex signatures using the same proof structure
    let proof = BlockProof::with_params(
        original_proof.proof_for.clone(),
        original_proof.root.clone(),
        Some(BlockSignaturesVariant::Simplex(simplex_sigs)),
    );

    // Serialize to JSON
    let result = db_serialize_block_proof("_id", &proof);
    assert!(result.is_ok(), "Serialization should succeed: {:?}", result.err());

    let map = result.unwrap();

    // Verify simplex-specific fields are present
    assert_eq!(map.get("signature_type").unwrap(), "simplex");
    assert!(map.contains_key("session_id"), "session_id should be present");
    assert!(map.contains_key("slot"), "slot should be present");
    assert!(map.contains_key("candidate_data"), "candidate_data should be present");

    // Verify slot value
    assert_eq!(map.get("slot").unwrap(), 42);

    // Verify common fields
    assert!(map.contains_key("signatures"), "signatures should be present");
    assert!(map.contains_key("validator_list_hash_short"));
    assert!(map.contains_key("catchain_seqno"));
    assert_eq!(map.get("validator_list_hash_short").unwrap(), 12345);
    assert_eq!(map.get("catchain_seqno").unwrap(), 6789);
}

fn prepare_shard_state_json(name: &str, workchain_id: i32, mode: SerializationMode) -> String {
    let boc = read(format!("src/tests/data/states/{}", name))
        .unwrap_or_else(|_| panic!("Error reading file {:?}", name));
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let id = format!("state:{:x}", cell.repr_hash());
    let state = ShardStateUnsplit::construct_from_cell(cell).unwrap();
    let set = ShardStateSerializationSet { state, boc, id, block_id: None, workchain_id };
    format!("{:#}", serde_json::json!(db_serialize_shard_state_ex("id", &set, mode).unwrap()))
}

fn check_shard_state(name: &str, workchain_id: i32, mode: SerializationMode) {
    let json = prepare_shard_state_json(name, workchain_id, mode);
    let postfix = match mode {
        SerializationMode::QServer => "-Q",
        SerializationMode::Standart => "",
        _ => panic!(),
    };
    //std::fs::write(file_name.clone() + postfix + "-ethalon.json", &json).unwrap();
    let name = format!("states/{}{}", name, postfix);
    assert_json_eq_file(&json, &name);
}

#[test]
fn test_serialize_mc_zerostate_s() {
    check_shard_state(
        "zerostate_-1_D270B87B2952B5BA7DAA70AAF0A8C361BEFCF4D8D2DB92F9640D5443070838E4",
        -1,
        SerializationMode::Standart,
    );
}

#[test]
fn test_serialize_mc_zerostate_q() {
    check_shard_state(
        "zerostate_-1_D270B87B2952B5BA7DAA70AAF0A8C361BEFCF4D8D2DB92F9640D5443070838E4",
        -1,
        SerializationMode::QServer,
    );
}

#[test]
fn test_serialize_wc_zerostate_s() {
    check_shard_state(
        "zerostate_0_97AF4602A57FC884F68BB4659BAB8875DC1F5E45A9FD4FBAFD0C9BC10AA5067C",
        0,
        SerializationMode::Standart,
    );
}

#[test]
fn test_serialize_wc_zerostate_q() {
    check_shard_state(
        "zerostate_0_97AF4602A57FC884F68BB4659BAB8875DC1F5E45A9FD4FBAFD0C9BC10AA5067C",
        0,
        SerializationMode::QServer,
    );
}

#[test]
fn test_serialize_wc_state_s() {
    check_shard_state(
        "state_4723_0_c800000000000000_81832210A895E93967B7D2A0638159FC5FD88C1DB402545AAAABA509BE93017F",
        0,
        SerializationMode::Standart
    );
}

#[test]
fn test_serialize_wc_state_q() {
    check_shard_state(
        "state_4723_0_c800000000000000_81832210A895E93967B7D2A0638159FC5FD88C1DB402545AAAABA509BE93017F",
        0,
        SerializationMode::QServer
    );
}

fn check_transaction_field(
    file: &str,
    field_name: &str,
    std_value: impl Into<Value>,
    q_value: impl Into<Value>,
) {
    let boc = std::fs::read(Path::new("src/tests/data/transactions").join(file)).unwrap();
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");
    let id = cell.repr_hash();
    let tr = Transaction::construct_from_cell(cell).unwrap();
    let set = TransactionSerializationSet {
        block_id: None,
        boc,
        id,
        proof: None,
        status: TransactionProcessingStatus::Finalized,
        workchain_id: 0,
        transaction: tr,
    };
    let serialized = db_serialize_transaction_ex("id", &set, SerializationMode::Standart).unwrap();
    pretty_assertions::assert_eq!(serde_json::json!(serialized)[field_name], std_value.into());
    let serialized = db_serialize_transaction_ex("id", &set, SerializationMode::QServer).unwrap();
    pretty_assertions::assert_eq!(serde_json::json!(serialized)[field_name], q_value.into());
}

#[test]
fn test_balance_delta() {
    check_transaction_field("aborted_bounced.boc", "balance_delta", "000", "0x0");
    check_transaction_field("ext_in&int_out.boc", "balance_delta", "-f8e1369309", "-0x1ec96cf6");
    check_transaction_field(
        "ext_in&int_out_special.boc",
        "balance_delta",
        "-f2d92301e7d945ff",
        "-0x26dcfe1826ba00",
    );
    check_transaction_field("int_in.boc", "balance_delta", "0c71b149203e800", "0x71b149203e800");
}

#[test]
fn test_ext_in_msg_fee() {
    check_transaction_field("aborted_bounced.boc", "ext_in_msg_fee", Value::Null, Value::Null);
    check_transaction_field("ext_in&int_out.boc", "ext_in_msg_fee", "051c80e0", "0x1c80e0");
    check_transaction_field("ext_in&int_out_special.boc", "ext_in_msg_fee", "000", "0x0");
    check_transaction_field("int_in.boc", "ext_in_msg_fee", Value::Null, Value::Null);
}

#[test]
fn test_serialize_deleted_account_s() {
    let account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let set = DeletedAccountSerializationSet {
        account_id: MsgAddressInt::default().address().clone(),
        workchain_id: MsgAddressInt::default().workchain_id(),
        prev_code_hash: account.get_code_hash(),
    };
    let doc = db_serialize_deleted_account("id", &set).unwrap();

    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(doc)),
        r#"{
  "json_version": 8,
  "id": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "workchain_id": 0,
  "acc_type": 3,
  "prev_code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a"
}"#
    )
}

#[test]
fn test_serialize_deleted_account_q() {
    let account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let set = DeletedAccountSerializationSet {
        account_id: MsgAddressInt::default().address().clone(),
        workchain_id: MsgAddressInt::default().workchain_id(),
        prev_code_hash: account.get_code_hash(),
    };
    let doc = db_serialize_deleted_account_ex("id", &set, SerializationMode::QServer).unwrap();

    pretty_assertions::assert_eq!(
        format!("{:#}", serde_json::json!(doc)),
        r#"{
  "json_version": 8,
  "id": "0:0000000000000000000000000000000000000000000000000000000000000000",
  "workchain_id": 0,
  "acc_type": 3,
  "acc_type_name": "NonExist",
  "prev_code_hash": "3c28164f21b76a53cfe73510197b99c735d4d97b652e6950f317bcbfe955848a"
}"#
    )
}

#[test]
fn test_block_order() {
    let block = std::fs::read(
        "src/tests/data/89ED400A43E76664437EFC9C79B84AC387493A9EE5E789338FF71C25F54218BE.boc",
    )
    .unwrap();
    let block = Block::construct_from_bytes(&block).unwrap();
    pretty_assertions::assert_eq!("4c6dd7m", block_order(&block, 814551).unwrap());
    let block = std::fs::read(
        "src/tests/data/18AFCDD25BE0989CE516504263EB351818A0FF8F6AB3689501C8E3B767EF413C.boc",
    )
    .unwrap();
    let block = Block::construct_from_bytes(&block).unwrap();
    pretty_assertions::assert_eq!("17b00540960604", block_order(&block, 123).unwrap());
}
