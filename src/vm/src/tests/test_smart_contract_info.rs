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
use ton_block::{base64_decode, read_single_root_boc, ton_method_id};

#[test]
fn test_smart_contract_info_serialization_default() {
    let sci = SmartContractInfo::default();
    sci.as_temp_data_item();
}

#[test]
fn test_smart_contract_info() {
    let sci = SmartContractInfo::default();
    let item = sci.as_temp_data_item();
    let result = item
        .as_tuple()
        .expect("result must be a tuple")
        .first()
        .expect("tuple must have at least one item")
        .as_tuple()
        .expect("SMCI list must be a tuple")
        .len();
    assert_eq!(result, 17);
}

#[test]
fn test_run_get_method_seqno_with_config() {
    let mc_state_name = "../block/src/tests/data/free-ton-mc-state-61884";
    let mc_state_cell = Cell::read_from_file(mc_state_name);
    let method_id = ton_method_id("seqno");
    assert_eq!(method_id, 0x14C97);

    let mc_state = ShardStateUnsplit::construct_from_cell(mc_state_cell.clone()).unwrap();
    let shard_account = mc_state.read_accounts().unwrap().get(&[0x55; 32].into()).unwrap().unwrap();
    let result =
        run_smc_method(&shard_account, mc_state_cell.clone(), method_id, Vec::new()).unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.gas_used, 869);
    assert_eq!(result.stack.len(), 1);
    assert_eq!(result.stack[0].number().unwrap().number(), "0x0");
}

#[test]
fn test_run_get_method_seqno_with_elector() {
    let mc_state_name = "../block/src/tests/data/free-ton-mc-state-61884";
    let mc_state_cell = Cell::read_from_file(mc_state_name);
    let method_id = ton_method_id("seqno");
    assert_eq!(method_id, 0x14C97);

    let mc_state = ShardStateUnsplit::construct_from_cell(mc_state_cell.clone()).unwrap();
    let shard_account = mc_state.read_accounts().unwrap().get(&[0x33; 32].into()).unwrap().unwrap();
    // let account = "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";
    // let account = base64_decode(account).unwrap();
    // let account = ton_block::Account::construct_from_bytes(&account).unwrap();
    // let shard_account = ShardAccount::with_params(&account, Default::default(), 0).unwrap();
    let result =
        run_smc_method(&shard_account, mc_state_cell.clone(), method_id, Vec::new()).unwrap();
    assert_eq!(result.exit_code, 11);
    assert_eq!(result.gas_used, 770);
    assert_eq!(result.stack.len(), 1);
    assert_eq!(result.stack[0].number().unwrap().number(), "0x14c97");
}

fn extract_account_boc<'a>(json: &'a str, account_id: &str) -> Option<&'a str> {
    let id_key = format!("\"id\": \"{}\"", account_id);
    let id_pos = json.find(&id_key)?;
    let boc_key = "\"boc\": \"";
    let boc_start = json[id_pos..].find(boc_key)? + id_pos + boc_key.len();
    let rest = &json[boc_start..];
    let boc_end = rest.find('"')?;
    Some(&rest[..boc_end])
}

fn extract_json_string_field(json: &str, field: &str) -> anyhow::Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| anyhow::anyhow!("invalid json: {err}"))?;
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("missing string field `{field}`"))
}

fn should_dump_external_boc_response() -> bool {
    std::env::var("TON_VM_DUMP_ELECTOR_RESPONSE")
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn stack_entry_to_json(entry: &ton_api::ton::tvm::StackEntry) -> serde_json::Value {
    match entry {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryNumber(number) => serde_json::json!({
            "type": "num",
            "value": number.number.number(),
        }),
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryCell(cell) => serde_json::json!({
            "type": "cell",
            "value": cell.cell.bytes.clone(),
        }),
        ton_api::ton::tvm::StackEntry::Tvm_StackEntrySlice(slice) => serde_json::json!({
            "type": "slice",
            "value": slice.slice.bytes.clone(),
        }),
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(tuple) => serde_json::json!({
            "type": "tuple",
            "elements": tuple.tuple.elements().iter().map(stack_entry_to_json).collect::<Vec<_>>(),
        }),
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryList(list) => serde_json::json!({
            "type": "list",
            "elements": list.list.elements().iter().map(stack_entry_to_json).collect::<Vec<_>>(),
        }),
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryUnsupported => serde_json::json!({
            "type": "unsupported",
        }),
    }
}

fn stack_to_json(stack: &[ton_api::ton::tvm::StackEntry]) -> serde_json::Value {
    serde_json::Value::Array(stack.iter().map(stack_entry_to_json).collect())
}

fn maybe_dump_external_boc_response(result: &ton_api::ton::smc::runresult::RunResult) {
    if !should_dump_external_boc_response() {
        return;
    }

    let output_path = format!("{}/src/tests/elector_response.json", env!("CARGO_MANIFEST_DIR"));
    let payload = serde_json::json!({
        "exit_code": result.exit_code,
        "gas_used": result.gas_used,
        "method": "participant_list_extended",
        "method_id": ton_method_id("participant_list_extended"),
        "stack": stack_to_json(&result.stack),
    });
    let pretty = serde_json::to_string_pretty(&payload).expect("serialize response to json");
    std::fs::write(&output_path, pretty).expect("write elector response json");
    println!("saved run_get_method response to {output_path}");
}

fn load_elector_shard_account() -> ShardAccount {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let json_path =
        format!("{}/../node/tests/test_run_net_py/zerostate_blank_elections.json", manifest_dir);
    let json = std::fs::read_to_string(json_path).unwrap();
    let boc_b64 = extract_account_boc(
        &json,
        "-1:3333333333333333333333333333333333333333333333333333333333333333",
    )
    .expect("elector account boc not found");
    let boc = base64_decode(boc_b64).unwrap();
    let account_cell = read_single_root_boc(boc).unwrap();
    ShardAccount::with_account_root(account_cell, Default::default(), 0)
}

fn load_mc_state_cell() -> Cell {
    let mc_state_name = "../block/src/tests/data/free-ton-mc-state-61884";
    Cell::read_from_file(mc_state_name)
}

fn run_elector_method(
    method: &str,
    stack: Vec<ton_api::ton::tvm::StackEntry>,
) -> ton_api::ton::smc::runresult::RunResult {
    let shard_account = load_elector_shard_account();
    let mc_state_cell = load_mc_state_cell();
    let method_id = ton_method_id(method);
    run_smc_method(&shard_account, mc_state_cell, method_id, stack).unwrap()
}

fn stack_number(value: i64) -> ton_api::ton::tvm::StackEntry {
    let items = convert_stack(&[StackItem::int(value)]).unwrap();
    items.into_iter().next().unwrap()
}

fn stack_number_from_str(value: &str) -> ton_api::ton::tvm::StackEntry {
    let number = parse_stack_number(value).unwrap();
    let items = convert_stack(&[StackItem::int(number)]).unwrap();
    items.into_iter().next().unwrap()
}

fn load_external_elector_shard_account() -> ShardAccount {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let json_path = format!("{manifest_dir}/src/tests/elector.json");
    let json = std::fs::read_to_string(&json_path).unwrap();
    let boc_b64 = extract_json_string_field(&json, "boc").expect("missing boc field");

    let account_bytes = base64_decode(&boc_b64).unwrap();
    let account = ton_block::Account::construct_from_bytes(&account_bytes).unwrap();
    ShardAccount::with_params(&account, Default::default(), 0).unwrap()
}

fn run_external_elector_method(
    method: &str,
    stack: Vec<ton_api::ton::tvm::StackEntry>,
) -> ton_api::ton::smc::runresult::RunResult {
    let shard_account = load_external_elector_shard_account();
    let mc_state_cell = load_mc_state_cell();
    let method_id = ton_method_id(method);
    run_smc_method(&shard_account, mc_state_cell, method_id, stack).unwrap()
}

fn first_external_participant_pubkey_and_wallet() -> (String, String) {
    let result = run_external_elector_method("participant_list_extended", Vec::new());
    assert_eq!(result.exit_code, 0);
    assert!(result.stack.len() >= 5);

    let participants = match &result.stack[4] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryList(list) => list.list.elements(),
        other => panic!("expected participants list at stack[4], got {:?}", other),
    };
    assert!(!participants.is_empty(), "participants list is empty");

    let participant_fields = match &participants[0] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(tuple) => tuple.tuple.elements(),
        other => panic!("expected participant tuple, got {:?}", other),
    };
    assert_eq!(participant_fields.len(), 2, "participant must be tuple(pubkey, args)");

    let pubkey = participant_fields[0]
        .number()
        .expect("participant pubkey must be number")
        .number()
        .to_owned();

    let args = match &participant_fields[1] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(tuple) => tuple.tuple.elements(),
        other => panic!("expected participant args tuple, got {:?}", other),
    };
    assert_eq!(args.len(), 4, "participant args must contain 4 fields");

    let wallet_addr =
        args[2].number().expect("participant wallet addr must be number").number().to_owned();

    (pubkey, wallet_addr)
}

#[test]
fn test_run_get_method_participant_list_extended_empty_list() {
    let result = run_elector_method("participant_list_extended", Vec::new());
    assert_eq!(result.exit_code, 0);
    assert!(result.stack.len() >= 5);
    match &result.stack[4] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryList(list) => {
            assert!(list.list.elements().is_empty());
        }
        other => panic!("expected list, got {:?}", other),
    }
}

#[test]
fn test_run_get_method_elector_active_election_id() {
    let result = run_elector_method("active_election_id", Vec::new());
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stack.len(), 1);
    assert_eq!(result.stack[0].number().unwrap().number(), "0x0");
}

#[test]
fn test_run_get_method_elector_compute_returned_stake_zero_addr() {
    let result = run_elector_method("compute_returned_stake", vec![stack_number(0)]);
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stack.len(), 1);
    assert_eq!(result.stack[0].number().unwrap().number(), "0x0");
}

#[test]
fn test_run_get_method_elector_participates_in_zero_pubkey() {
    let result = run_elector_method("participates_in", vec![stack_number(0)]);
    assert_eq!(result.exit_code, 0);
    assert!(result.stack.len() >= 1);
    assert!(result.stack[0].number().is_some());
}

#[test]
fn test_run_get_method_participant_list_extended_from_external_boc_keeps_pubkey() {
    let result = run_external_elector_method("participant_list_extended", Vec::new());
    maybe_dump_external_boc_response(&result);

    assert_eq!(result.exit_code, 0);
    assert!(result.stack.len() >= 5);

    let participants = match &result.stack[4] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryList(list) => list.list.elements(),
        other => panic!("expected participants list at stack[4], got {:?}", other),
    };
    assert!(!participants.is_empty(), "participants list is empty");

    let first = &participants[0];
    let participant_fields = match first {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(tuple) => tuple.tuple.elements(),
        other => panic!("expected participant tuple, got {:?}", other),
    };
    assert_eq!(participant_fields.len(), 2, "participant must be tuple(pubkey, args)");
    assert!(
        matches!(participant_fields[0], ton_api::ton::tvm::StackEntry::Tvm_StackEntryNumber(_)),
        "participant pubkey must be number"
    );

    let args = match &participant_fields[1] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(tuple) => tuple.tuple.elements(),
        other => panic!("expected participant args tuple, got {:?}", other),
    };
    assert_eq!(args.len(), 4, "participant args must contain 4 fields");
    assert!(
        args.iter().all(|entry| {
            matches!(entry, ton_api::ton::tvm::StackEntry::Tvm_StackEntryNumber(_))
        }),
        "all participant args fields must be numbers"
    );
}

#[test]
fn test_run_get_method_participates_in_from_external_boc_existing_pubkey() {
    let (pubkey, _) = first_external_participant_pubkey_and_wallet();
    assert_eq!(
        pubkey, "0x6a258fe61308a122a4e781aa9cf6e18af017fb5536ffbceead903616b40fb0",
        "unexpected first participant pubkey in elector snapshot"
    );

    let result =
        run_external_elector_method("participates_in", vec![stack_number_from_str(&pubkey)]);

    assert_eq!(result.exit_code, 0);
    assert!(!result.stack.is_empty(), "participates_in must return non-empty stack");
    let value = result.stack[0]
        .number()
        .expect("participates_in first result entry must be number")
        .number()
        .to_owned();
    assert_eq!(
        value, "0x363841cca8800",
        "unexpected participates_in result for first participant pubkey"
    );
}

#[test]
fn test_run_get_method_compute_returned_stake_from_external_boc_existing_wallet() {
    let (_, wallet_addr) = first_external_participant_pubkey_and_wallet();
    assert_eq!(
        wallet_addr, "0x121871ca1f8e4c8ab479a12a8dcacc43767fdfb4f0efda0837a9f6ec92076a66",
        "unexpected first participant wallet in elector snapshot"
    );

    let result = run_external_elector_method(
        "compute_returned_stake",
        vec![stack_number_from_str(&wallet_addr)],
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stack.len(), 1, "compute_returned_stake must return one value");
    let value = result.stack[0]
        .number()
        .expect("compute_returned_stake result must be number")
        .number()
        .to_owned();
    assert_eq!(
        value, "0x0",
        "unexpected compute_returned_stake result for first participant wallet"
    );
}

#[test]
fn test_convert_stack() {
    let mut tuple = StackItem::None;
    for n in 0..3 {
        tuple = StackItem::tuple(vec![StackItem::int(n), tuple]);
    }
    let items = convert_stack(&[tuple.clone()]).unwrap();
    let result = convert_ton_stack(&items).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], tuple);
}

#[test]
fn test_convert_stack_none_is_empty_list() {
    let items = convert_stack(&[StackItem::None]).unwrap();
    assert_eq!(items.len(), 1);
    match &items[0] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryList(list) => {
            assert!(list.list.elements().is_empty());
        }
        other => panic!("expected list, got {:?}", other),
    }
}

#[test]
fn test_convert_stack_tuple_pair_with_inner_tuple() {
    let tuple =
        StackItem::tuple(vec![StackItem::int(1), StackItem::tuple(vec![StackItem::int(2)])]);
    let items = convert_stack(&[tuple]).unwrap();
    assert_eq!(items.len(), 1);
    match &items[0] {
        ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(outer) => {
            let outer = outer.tuple.elements();
            assert_eq!(outer.len(), 2);
            assert_eq!(outer[0].number().unwrap().number(), "0x1");
            match &outer[1] {
                ton_api::ton::tvm::StackEntry::Tvm_StackEntryTuple(inner) => {
                    let inner = inner.tuple.elements();
                    assert_eq!(inner.len(), 1);
                    assert_eq!(inner[0].number().unwrap().number(), "0x2");
                }
                other => panic!("expected inner tuple, got {:?}", other),
            }
        }
        other => panic!("expected tuple, got {:?}", other),
    }
}
