/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use ton_api::ton::tvm::{
    cell::Cell as TvmCell, numberdecimal::NumberDecimal, slice::Slice, stackentry,
    Number as TvmNumber, StackEntry,
};
use ton_block::base64_decode;

const ADMIN_ADDR: &str = "Ef9_s2qAzJOOb6HOyYIRx7RtAqgvM9pNoYUfWQW-mUImE2J8";
const JETTON_ADDR: &str = "EQAOtrk3eOGwebP9KMNVSmIGo3mA0bN1e12SiBf0fgkWDEj8";
const OWNER_ADDR: &str = "EQBvvC5hmeO1wDG4DlqWEGFp8bgCHckcvGXgRXTVSty9_71b";

fn mk_num(hex: &str) -> StackEntry {
    StackEntry::Tvm_StackEntryNumber(stackentry::StackEntryNumber {
        number: TvmNumber::Tvm_NumberDecimal(NumberDecimal {
            number: hex.to_string(), // "0x0", "0x-1", "-0x1"
        }),
    })
}

fn stack_cell_from_b64(b64: &str) -> StackEntry {
    let bytes = base64_decode(b64).expect("invalid base64 in test data");
    let cell = TvmCell { bytes };
    StackEntry::Tvm_StackEntryCell(stackentry::StackEntryCell { cell })
}

fn stack_slice_from_b64(b64: &str) -> StackEntry {
    let bytes = base64_decode(b64).expect("invalid base64 in test data");
    let slice = Slice { bytes };
    StackEntry::Tvm_StackEntrySlice(stackentry::StackEntrySlice { slice })
}

fn assert_jetton_master(json: &serde_json::Value) {
    assert_eq!(json["mintable"], true);
    assert_eq!(json["contract_type"], "jetton_master");
    assert_eq!(json["admin_address"], ADMIN_ADDR);
}

fn assert_jetton_wallet(json: &serde_json::Value) {
    assert_eq!(json["owner"], OWNER_ADDR);
    assert_eq!(json["jetton"], JETTON_ADDR);
    assert_eq!(json["contract_type"], "jetton_wallet");
}

#[test]
fn parse_jetton_master_data_onchain_with_cell() {
    // total_supply = 0x0
    let e0 = mk_num("0x0");

    // mintable = -0x1
    let e1 = mk_num("-0x1");

    // admin cell
    let admin_slice_b64 = "te6cckEBAQEAJAAAQ5/v9m1QGZJxzfQ52TBCOPaNoFUF5ntJtDCj6yC30yhEwnCgiu9J";
    let e2 = stack_cell_from_b64(admin_slice_b64);

    // jetton_content cell
    let jetton_content_b64 =
        "te6cckECCQEAAZIAAQMAwAECASACAwFDv/hy69tRTZyXwoO38K5ReQKeK2EZw5RicZ5PRu\
        2PdBPmQAQBQ7/3QH6XjwGkBxFBGxrLdzqWvdk/qDu1yoQ1ATyMSzrJH0AIAQIABQH+ZGF0Y\
        TphcHBsaWNhdGlvbi9qc29uLCU3QiUyMm5hbWUlMjIlM0ElMjJUZXN0JTIwQVBJJTIwVG9r\
        ZW4lMjAxNzY0NjYyMTg1NTgwJTIyJTJDJTIyc3ltYm9sJTIyJTNBJTIyVEFUJTIyJTJDJTI\
        yZGVzY3JpcHRpb24lMjIlMwYB/kElMjJBJTIwdGVzdCUyMHRva2VuJTIwZm9yJTIwQVBJJT\
        IwdGVzdGluZyUyMHB1cnBvc2VzJTIyJTJDJTIyaW1hZ2UlMjIlM0ElMjJodHRwcyUzQSUyR\
        iUyRmNkbi1pY29ucy1wbmcuZnJlZXBpay5jb20lMkY1MTIlMkY1NjEHAGA5JTJGNTYxOTc0\
        MC5wbmclMjIlMkMlMjJkZWNpbWFscyUyMiUzQSUyMjklMjIlN0QABAA55w5fXw==";
    let e3 = stack_cell_from_b64(jetton_content_b64);

    // jetton_wallet_code cell
    let wallet_code_b64 = "te6cckEBAQEAIwAAQgK6KRjIlH6bJa+awbiDNXdUFz5YEvgHo9bmQqFHCVlTlcq9O8g=";
    let e4 = stack_cell_from_b64(wallet_code_b64);

    let stack = vec![e0, e1, e2, e3, e4];
    let json = parse_jetton_master_data(&stack, false).expect("parse_jetton_master_data failed");
    assert_jetton_master(&json);
}

#[test]
fn parse_jetton_master_data_onchain_with_slice() {
    let e0 = mk_num("0x0");
    let e1 = mk_num("-0x1");

    // admin slice
    let slice_b64 = "n+/2bVAZknHN9DnZMEI49o2gVQXme0m0MKPrILfTKETCYA==";
    let e2 = stack_slice_from_b64(slice_b64);

    // jetton_content cell
    let jetton_content_b64 =
        "te6ccgECCQEAAZIAAQMAwAECASACAwFDv/hy69tRTZyXwoO38K5ReQKeK2EZw5RicZ5PRu\
        2PdBPmQAQBQ7/3QH6XjwGkBxFBGxrLdzqWvdk/qDu1yoQ1ATyMSzrJH0AIAQIABQH+ZGF0Y\
        TphcHBsaWNhdGlvbi9qc29uLCU3QiUyMm5hbWUlMjIlM0ElMjJUZXN0JTIwQVBJJTIwVG9r\
        ZW4lMjAxNzY0NjYyMTg1NTgwJTIyJTJDJTIyc3ltYm9sJTIyJTNBJTIyVEFUJTIyJTJDJTI\
        yZGVzY3JpcHRpb24lMjIlMwYB/kElMjJBJTIwdGVzdCUyMHRva2VuJTIwZm9yJTIwQVBJJT\
        IwdGVzdGluZyUyMHB1cnBvc2VzJTIyJTJDJTIyaW1hZ2UlMjIlM0ElMjJodHRwcyUzQSUyR\
        iUyRmNkbi1pY29ucy1wbmcuZnJlZXBpay5jb20lMkY1MTIlMkY1NjEHAGA5JTJGNTYxOTc0\
        MC5wbmclMjIlMkMlMjJkZWNpbWFscyUyMiUzQSUyMjklMjIlN0QABAA5";
    let e3 = stack_cell_from_b64(jetton_content_b64);

    // jetton_wallet_code cell
    let wallet_code_b64 = "te6ccgEBAQEAIwAAQgK6KRjIlH6bJa+awbiDNXdUFz5YEvgHo9bmQqFHCVlTlQ==";
    let e4 = stack_cell_from_b64(wallet_code_b64);

    let stack = vec![e0, e1, e2, e3, e4];
    let json = parse_jetton_master_data(&stack, false).expect("parse_jetton_master_data failed");
    assert_jetton_master(&json);
}

#[test]
fn parse_jetton_wallet_data_test() {
    let e0 = mk_num("0x38866cac3c000");
    let cell1 = "te6cckEBAQEAJAAAQ4AN94XMMzx2uAY3ActSwgwtPjcAQ7kjl4y8CK6aqVuXv/DS1hZm";
    let e1 = stack_cell_from_b64(cell1);
    let cell2 = "te6cckEBAQEAJAAAQ4AB1tcm7xw2DzZ/pRhqqUxA1G8wGjZur2uyUQL+j8EiwZC3temd";
    let e2 = stack_cell_from_b64(cell2);
    let cell3 = "te6cckEBAQEAIwAIQgK6KRjIlH6bJa+awbiDNXdUFz5YEvgHo9bmQqFHCVlTlSN648M=";
    let e3 = stack_cell_from_b64(cell3);

    let stack = vec![e0, e1, e2, e3];
    let json = parse_jetton_wallet_data(&stack, false).expect("parse_jetton_master_data failed");
    assert_jetton_wallet(&json);
}

#[test]
fn parse_jetton_wallet_data_test_slice() {
    let e0 = mk_num("0x38866cac3c000");
    let slice1 = "gA33hcwzPHa4BjcBy1LCDC0+NwBDuSOXjLwIrpqpW5e/4A==";
    let e1 = stack_slice_from_b64(slice1);
    let slice2 = "gAHW1ybvHDYPNn+lGGqpTEDUbzAaNm6va7JRAv6PwSLBgA==";
    let e2 = stack_slice_from_b64(slice2);
    let e3 = stack_cell_from_b64("te6ccgEBAQEAAgAAAA==");

    let stack = vec![e0, e1, e2, e3];
    let json = parse_jetton_wallet_data(&stack, false).expect("parse_jetton_master_data failed");
    assert_jetton_wallet(&json);
}

#[test]
fn parse_nft_item_data_with_slice() {
    let e0 = mk_num("-0x1");
    let e1 = mk_num("0x0");
    let slice1 = "gAam1EA9WSAx7CcdwYgH9CdTpmCnfTDxPdDoapQ6tLYdAA==";
    let e2 = stack_slice_from_b64(slice1);
    let slice2 = "n+/2bVAZknHN9DnZMEI49o2gVQXme0m0MKPrILfTKETCYA==";
    let e3 = stack_slice_from_b64(slice2);
    let cell = "te6ccgEBAQEAJgAASAFodHRwczovL2V4YW1wbGUuY29tL25mdC1pdGVtLTAuanNvbg==";
    let e4 = stack_cell_from_b64(cell);

    let stack = vec![e0, e1, e2, e3, e4];
    let json = parse_nft_item_data(&stack, false).expect("parse_jetton_master_data failed");
    assert_eq!(json["owner_address"], ADMIN_ADDR);
    assert_eq!(json["collection_address"], "EQA1NqIB6skBj2E47gxAP6E6nTMFO-mHie6HQ1Sh1aWw6MfH");
}

#[test]
fn test_parse_nft_collection_data() {
    let e0 = mk_num("0x1");
    let e1 = stack_cell_from_b64(
        "te6cckEBAQEAZQAAxgFodHRwczovL2F2YXRhcnMubWRzLnlhbmRleC5uZXQvaT9pZD03NT\
        I4YTNmMjJjMGU3Yjg5YmM0ZGRiMTY0ZTMyNDc1Nl9sLTEyMTY1NzQ2LWltYWdlcy10aHVtY\
        nMmbj0xM0vduX4=",
    );
    let e2 =
        stack_cell_from_b64("te6cckEBAQEAJAAAQ5/v9m1QGZJxzfQ52TBCOPaNoFUF5ntJtDCj6yC30yhEwnCgiu9J");

    let stack = vec![e0, e1, e2];
    let json = parse_nft_collection(&stack, false).expect("parse_jetton_master_data failed");
    assert_eq!(json["owner_address"], ADMIN_ADDR,);
    assert_eq!(json["contract_type"], "nft_collection",);
}
