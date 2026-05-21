/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::rpc_server::ApiError;
use ton_block::error;

#[derive(Debug, serde::Deserialize)]
struct StackWrapper {
    stack: Vec<RPCStackEntry>,
}

const EMPTY_LIST_JSON: &str = r#"{
  "stack": [
    [
      "list",
      {
        "@type": "tvm.list",
        "elements": []
      }
    ]
  ]
}"#;

const LIST_WITH_NUMBERS_JSON: &str = r#"{
  "stack": [
    [
      "list",
      {
        "@type": "tvm.list",
        "elements": [
          {
            "@type": "tvm.stackEntryNumber",
            "number": {
              "@type": "tvm.numberDecimal",
              "number": "1762610952"
            }
          },
          {
            "@type": "tvm.stackEntryNumber",
            "number": {
              "@type": "tvm.numberDecimal",
              "number": "1762676488"
            }
          }
        ]
      }
    ]
  ]
}"#;

const CELL_INPUT_JSON: &str = r#"{
  "stack": [
    [
      "cell",
      "te6cckEBAQEABgAACAAB4kAwYI+3"
    ]
  ]
}"#;

const CELL_OUTPUT_JSON: &str = r#"{
  "stack": [
    [
      "cell",
      {
        "bytes": "te6cckEBAQEABgAACAAB4kAwYI+3",
        "object": {
          "data": {
            "b64": "AAHiQA==",
            "len": 32
          },
          "refs": [],
          "special": false
        }
      }
    ]
  ]
}"#;

const SIMPLE_STACK_JSON: &str = r#"[["num", 123], ["tvm.Cell", "te6cckEBAQEABgAACAAB4kAwYI+3"]]"#;
const SAMPLE_TRANSACTION_BOC: &str = "te6cckECCgEAAjwAA698rzi1TiTv7uzX0F5ib+p1I8OSOBKJlSl/pz3EwVmgHMAAAADoSlysOqnsRVo8P/O+kGDrsbjrJa+rp2Mxq/Pu5PjkTWpbp2bwAAAA6EaMHDaRInLAADQIAQIDAgHgBAUAgnIRSaK4B/MGTv/y2QsBn6QkVAb4YKDW5V4FvbigQHTHkFgv1aAdtvLJI1N5KcSAvY7iOfoC1nqrtt6tw/dCykBMAgcMBgRACAkB34n/lecWqcSd/d2a+gvMTf1OpHhyRwJRMqUv9Oe4mCs0A5gFJ1lTlcS1/fpRrKAsTPXicV/PS9yiPoySCXe39jJuKzRSp00XytovwnvQaMbQCYisXQSAHOI/kEl6FR0sJ2IAEAAAAVNIkTtAAAAAKAwGAQHfBwBqQgBsnT5LwW+251RM0XADLzmTXSH/c5OgnJDB0EoDVagwFag34R1gAAAAAAAAAAAAAAAAAAAArUn/lecWqcSd/d2a+gvMTf1OpHhyRwJRMqUv9Oe4mCs0A5kANk6fJeC323OqJmi4AZecya6Q/7nJ0E5IYOglAarUGArUG/COsAAAAAAAHQlLlYjSJE5YQAClQXZQELB2AxOIAAAAAAAAAAAQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAYcAAAAAAAAIAAAAAAANCgxyf9BPqeGgQHcSUpljidRCNY9Z5iPYOqJQbD/jA8EBQFYwzYJQq";

const RETURN_TUPLE_JSON: &str = r#"{
  "stack": [
    ["num", "0x1"],
    ["num", "0x2"],
    ["num", "0x3"],
    ["num", "0x4"],
    ["num", "0x5"],
    ["num", "0x6"],
    ["num", "0x7"],
    ["num", "0x8"],
    ["num", "0x9"],
    ["num", "0xa"]
  ]
}"#;

const RETURN_LIST_JSON: &str = r#"{
  "stack": [
    [
      "list",
      {
        "@type": "tvm.list",
        "elements": [
          {
            "@type": "tvm.stackEntryNumber",
            "number": {
              "@type": "tvm.numberDecimal",
              "number": "3"
            }
          },
          {
            "@type": "tvm.stackEntryNumber",
            "number": {
              "@type": "tvm.numberDecimal",
              "number": "2"
            }
          },
          {
            "@type": "tvm.stackEntryNumber",
            "number": {
              "@type": "tvm.numberDecimal",
              "number": "1"
            }
          }
        ]
      }
    ]
  ]
}"#;

const MIXED_STACK_JSON: &str = include_str!("serde_test_mixed.json");

fn assert_round_trip(json: &str) {
    let expected: serde_json::Value = serde_json::from_str(json).expect("valid json");
    let wrapper: StackWrapper =
        serde_json::from_value(expected.clone()).expect("deserializes into stack");
    let actual = serde_json::json!({
        "stack": serialize_stack(wrapper.stack).expect("serializes back")
    });
    assert_eq!(actual, expected);
}

#[test]
fn empty_list_matches_format() {
    assert_round_trip(EMPTY_LIST_JSON);
}

#[test]
fn list_with_numbers_matches_format() {
    assert_round_trip(LIST_WITH_NUMBERS_JSON);
}

#[test]
fn cell_input_is_accepted_and_serializes_to_detailed_form() {
    let wrapper: StackWrapper =
        serde_json::from_str(CELL_INPUT_JSON).expect("input cell deserializes");
    let actual = serde_json::json!({
        "stack": serialize_stack(wrapper.stack).expect("serializes back")
    });
    let expected: serde_json::Value = serde_json::from_str(CELL_OUTPUT_JSON).expect("valid output");
    assert_eq!(actual, expected);
}

#[test]
fn cell_output_form_round_trips() {
    assert_round_trip(CELL_OUTPUT_JSON);
}

#[test]
fn simple_stack_accepts_numeric_and_tvm_cell_tags() {
    let stack: Vec<RPCStackEntry> =
        serde_json::from_str(SIMPLE_STACK_JSON).expect("simple stack parses");
    assert_eq!(stack.len(), 2);
    if let RPCStackEntry::Tvm_StackEntryNumber(number) = &stack[0] {
        assert_eq!(number.number.number(), "123");
    } else {
        panic!("expected number entry");
    }
    let stack =
        stack.into_iter().map(|e| serialize_stack_entry(&e.into()).unwrap()).collect::<Vec<_>>();
    let reserialized = serde_json::json!(stack);
    let expected = serde_json::json!([
        ["num", "123"],
        serde_json::from_str::<serde_json::Value>(
            r#"["cell", {
                "bytes": "te6cckEBAQEABgAACAAB4kAwYI+3",
                "object": {
                    "data": {"b64": "AAHiQA==", "len": 32},
                    "refs": [],
                    "special": false
                }
            }]"#
        )
        .unwrap()
    ]);
    assert_eq!(reserialized, expected);
}

#[test]
fn return_tuple_matches_format() {
    assert_round_trip(RETURN_TUPLE_JSON);
}

#[test]
fn return_list_matches_format() {
    assert_round_trip(RETURN_LIST_JSON);
}

#[test]
fn mixed_stack_snapshot_matches() {
    assert_round_trip(MIXED_STACK_JSON);
}

#[test]
fn normalize_number_decimal_converts_hex_and_keeps_decimal() {
    assert_eq!(normalize_number_decimal("0x0"), "0");
    assert_eq!(normalize_number_decimal("0x7b"), "123");
    assert_eq!(normalize_number_decimal("-0x7b"), "-123");
    assert_eq!(normalize_number_decimal("0XFF"), "255");
    assert_eq!(normalize_number_decimal("-0Xf"), "-15");
    assert_eq!(normalize_number_decimal("123"), "123");
    assert_eq!(normalize_number_decimal("-456"), "-456");
}

#[test]
fn normalize_number_decimal_invalid_hex_returns_original() {
    assert_eq!(normalize_number_decimal("0x"), "0x");
    assert_eq!(normalize_number_decimal("-0x"), "-0x");
    assert_eq!(normalize_number_decimal("0xZZ"), "0xZZ");
    assert_eq!(normalize_number_decimal("-0x1G"), "-0x1G");
}

#[test]
fn test_json_serde_block_id_ext() {
    let mc_block_id = BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        199473,
        "e+vumjCp4eC225NtPhUXGzN0JbNkJ81AKrEW7ozIHgA=".parse().unwrap(),
        "mX/lgP4LV3ufM0Mrb7cAUFMp9wVyd6uCxrC440FnBDk=".parse().unwrap(),
    );
    let json = serde_json::json!({"block_id": serialize_block_id(&mc_block_id)});
    pretty_assertions::assert_eq!(
        format!("{json:#}"),
        r#"{
  "block_id": {
    "@type": "ton.blockIdExt",
    "workchain": -1,
    "shard": "-9223372036854775808",
    "seqno": 199473,
    "root_hash": "e+vumjCp4eC225NtPhUXGzN0JbNkJ81AKrEW7ozIHgA=",
    "file_hash": "mX/lgP4LV3ufM0Mrb7cAUFMp9wVyd6uCxrC440FnBDk="
  }
}"#
    );
}

#[test]
fn test_json_serde_shard_account() {
    let shard_account = ShardAccount::with_account_root(
        Default::default(),
        "e+vumjCp4eC225NtPhUXGzN0JbNkJ81AKrEW7ozIHgA=".parse().unwrap(),
        28485000001,
    );

    let json = serde_json::json!({"last_transaction_id": serialize_shard_account(&shard_account)});

    pretty_assertions::assert_eq!(
        format!("{json:#}"),
        r#"{
  "last_transaction_id": {
    "@type": "internal.transactionId",
    "lt": "28485000001",
    "hash": "e+vumjCp4eC225NtPhUXGzN0JbNkJ81AKrEW7ozIHgA="
  }
}"#
    );
}

#[test]
fn serialize_ext_transaction_includes_account_field() {
    let tr_cell = read_single_root_boc(base64_decode(SAMPLE_TRANSACTION_BOC).unwrap()).unwrap();
    let tr = Transaction::construct_from_cell(tr_cell.clone()).unwrap();
    let address = "Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk";
    let account = "0:f2bce2d53893bfbbb35f417989bfa9d48f0e48e04a2654a5fe9cf71305668073";

    let json =
        serialize_transaction(&tr, tr_cell, address, "ext.transaction", Some(account), false)
            .expect("ext.transaction should serialize");

    pretty_assertions::assert_eq!(json["@type"], serde_json::json!("ext.transaction"));
    pretty_assertions::assert_eq!(json["account"], serde_json::json!(account));
    pretty_assertions::assert_eq!(json["address"]["account_address"], serde_json::json!(address));
    pretty_assertions::assert_eq!(json["in_msg"]["@type"], serde_json::json!("ext.message"));
    pretty_assertions::assert_eq!(json["in_msg"]["source"], serde_json::json!(""));
    pretty_assertions::assert_eq!(json["in_msg"]["destination"], serde_json::json!(address));
    pretty_assertions::assert_eq!(json["out_msgs"][0]["@type"], serde_json::json!("ext.message"));
    pretty_assertions::assert_eq!(json["out_msgs"][0]["source"], serde_json::json!(address));
}

#[test]
fn serialize_raw_transaction_omits_account_field() {
    let tr_cell = read_single_root_boc(base64_decode(SAMPLE_TRANSACTION_BOC).unwrap()).unwrap();
    let tr = Transaction::construct_from_cell(tr_cell.clone()).unwrap();

    let json = serialize_transaction(
        &tr,
        tr_cell,
        "Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk",
        "raw.transaction",
        None,
        false,
    )
    .expect("raw.transaction should serialize");

    assert!(json.get("account").is_none(), "raw.transaction must not contain account");
    pretty_assertions::assert_eq!(json["in_msg"]["@type"], serde_json::json!("raw.message"));
    pretty_assertions::assert_eq!(
        json["in_msg"]["source"],
        serde_json::json!({
            "@type": "accountAddress",
            "account_address": "",
        })
    );
    pretty_assertions::assert_eq!(
        json["in_msg"]["destination"],
        serde_json::json!({
            "@type": "accountAddress",
            "account_address": "Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk",
        })
    );
}

#[test]
fn test_api_error_display() {
    let err = ApiError::bad_request("Invalid input");
    let display = format!("{}", err);
    assert_eq!(display, "Bad Request: Invalid input (code -32400)");

    let err = error!("Some internal error");
    let display = format!("{}", err);
    assert_eq!(display, "Some internal error");
}
