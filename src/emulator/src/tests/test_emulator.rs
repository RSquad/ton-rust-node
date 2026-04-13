/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use ton_block::{
    Account, Deserializable, Serializable, Transaction, base64_decode, base64_encode,
    read_single_root_boc, write_boc,
};

fn cell_to_base64(cell: &Cell) -> String {
    base64_encode(write_boc(cell).unwrap())
}

#[test]
fn test_emulator() {
    let config = "../executor/real_boc/config.boc";
    let config_params = ConfigParams::construct_from_file(config).unwrap();
    let config_params_boc = cell_to_base64(config_params.root().unwrap());
    let account = "../executor/real_boc/two_messages_account_old.boc";
    let account_root = Cell::read_from_file(account);
    let shard_acc = ShardAccount::with_account_root(account_root, Default::default(), 0);
    let account = shard_acc.read_account().unwrap();
    let shard_account_boc = shard_acc.write_to_base64().unwrap();
    let transaction = "../executor/real_boc/two_messages_transaction.boc";
    let transaction = Transaction::construct_from_file(transaction).unwrap();
    let message_boc = cell_to_base64(&transaction.in_msg_cell().unwrap());

    let p = transaction_emulator_create(config_params_boc.as_ptr() as *const c_char, 0);
    transaction_emulator_set_unixtime(p, account.last_paid());
    let result = transaction_emulator_emulate_transaction(
        p,
        shard_account_boc.as_ptr() as *const c_char,
        message_boc.as_ptr() as *const c_char,
    );
    let result = unsafe { CString::from_raw(result as *mut c_char) };
    println!("Emulation result: {}", result.to_string_lossy());
    transaction_emulator_destroy(p);
    let result = emulator_version();
    let result = unsafe { CString::from_raw(result as *mut c_char) };
    println!("Emulator version: {}", result.to_string_lossy());
}

#[test]
fn test_transaction() {
    let json_path = "src/tests/4C90C139A5736F34EA3EEF62F0B06431719913835EA5A1B9173F20B2EF711583_66815326000001.json";
    let json_str = std::fs::read_to_string(json_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // 1. Config params
    let config_params_boc = CString::new(json["config_params_boc"].as_str().unwrap()).unwrap();

    // 2. Shard account state (before transaction)
    let shard_account_boc = CString::new(json["shard_account_boc"].as_str().unwrap()).unwrap();

    // 3. Input message
    let message_boc = CString::new(json["message_boc"].as_str().unwrap()).unwrap();

    // 4. Prev blocks info
    let prev_blocks_info_boc =
        CString::new(json["prev_blocks_info_boc"].as_str().unwrap()).unwrap();
    let expected_tx_hash = json["tx_hash"].as_str().unwrap();

    // 5. Block parameters from JSON
    let now = json["now"].as_u64().unwrap() as u32;
    let lt = json["lt"].as_u64().unwrap();
    let rand_seed_hex = CString::new(json["rand_seed"].as_str().unwrap()).unwrap();

    // 6. Create emulator
    let p = transaction_emulator_create(config_params_boc.as_ptr(), 5);

    // 7. Set block parameters
    transaction_emulator_set_unixtime(p, now);
    transaction_emulator_set_lt(p, lt);
    transaction_emulator_set_rand_seed(p, rand_seed_hex.as_ptr());

    // 7. Set prev blocks info
    transaction_emulator_set_prev_blocks_info(p, prev_blocks_info_boc.as_ptr());

    // 8. Emulate transaction
    let result = transaction_emulator_emulate_transaction(
        p,
        shard_account_boc.as_ptr(),
        message_boc.as_ptr(),
    );

    let result_str = unsafe { CString::from_raw(result as *mut c_char) };
    let result_json: serde_json::Value = serde_json::from_slice(result_str.as_bytes()).unwrap();

    assert!(result_json["success"].as_bool().unwrap());
    let tx_boc = result_json["transaction"].as_str().unwrap();
    let tx_cell = read_single_root_boc(base64_decode(tx_boc).unwrap()).unwrap();
    let tx_hash = tx_cell.repr_hash().to_hex_string().to_uppercase();
    assert_eq!(tx_hash, expected_tx_hash, "Transaction hash mismatch!");

    transaction_emulator_destroy(p);
}

// ===== TVM Emulator tests =====

/// Helper: extract code and data cells from an account BoC file
fn load_account_code_data(path: &str) -> (Cell, Cell) {
    let account_root = Cell::read_from_file(path);
    let account = Account::construct_from_cell(account_root).unwrap();
    let code = account.get_code().expect("Account has no code");
    let data = account.get_data().unwrap_or_default();
    (code, data)
}

#[test]
fn test_tvm_emulator_create_destroy() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 0);
    assert!(!p.is_null(), "tvm_emulator_create returned null");
    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_set_gas_limit() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 0);
    assert!(!p.is_null());

    let ok = tvm_emulator_set_gas_limit(p, 500_000);
    assert!(ok, "tvm_emulator_set_gas_limit failed");

    let ok = tvm_emulator_set_debug_enabled(p, true);
    assert!(ok, "tvm_emulator_set_debug_enabled failed");

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_set_c7() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 0);
    assert!(!p.is_null());

    let address =
        CString::new("0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap();
    let rand_seed =
        CString::new("628d5512834b482d4982d69d445da5f63e79de5f45f7ac52b25a9efc9a0db11c").unwrap();

    let ok = tvm_emulator_set_c7(
        p,
        address.as_ptr(),
        1700000000,
        1_000_000_000,
        rand_seed.as_ptr(),
        std::ptr::null(), // no config
    );
    assert!(ok, "tvm_emulator_set_c7 failed");

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_set_c7_with_config() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let config = ConfigParams::construct_from_file("../executor/real_boc/config.boc").unwrap();
    let config_boc = CString::new(cell_to_base64(config.root().unwrap())).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 0);
    assert!(!p.is_null());

    let address =
        CString::new("0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap();
    let rand_seed =
        CString::new("628d5512834b482d4982d69d445da5f63e79de5f45f7ac52b25a9efc9a0db11c").unwrap();

    let ok = tvm_emulator_set_c7(
        p,
        address.as_ptr(),
        1700000000,
        1_000_000_000,
        rand_seed.as_ptr(),
        config_boc.as_ptr(),
    );
    assert!(ok, "tvm_emulator_set_c7 with config failed");

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_set_libraries() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 0);
    assert!(!p.is_null());

    // Passing null should return false
    let ok = tvm_emulator_set_libraries(p, std::ptr::null());
    assert!(!ok, "tvm_emulator_set_libraries should fail with null");

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_run_get_method_simple() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 4);
    assert!(!p.is_null());

    // Set c7 so the emulator has basic context
    let address =
        CString::new("0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap();
    let rand_seed =
        CString::new("628d5512834b482d4982d69d445da5f63e79de5f45f7ac52b25a9efc9a0db11c").unwrap();
    tvm_emulator_set_c7(
        p,
        address.as_ptr(),
        1700000000,
        1_000_000_000,
        rand_seed.as_ptr(),
        std::ptr::null(),
    );

    // Run get method with empty stack (null stack_boc)
    // seqno get method id = 85143
    let result = tvm_emulator_run_get_method(p, 85143, std::ptr::null());
    assert!(!result.is_null(), "tvm_emulator_run_get_method returned null");

    let result_str = unsafe { CString::from_raw(result as *mut c_char) };
    let result_json: serde_json::Value = serde_json::from_slice(result_str.as_bytes()).unwrap();
    println!("run_get_method result: {}", result_json);

    // The result should have the expected fields
    assert_eq!(result_json["vm_exit_code"].as_i64().unwrap(), 0);
    assert_eq!(result_json["gas_used"].as_i64().unwrap(), 571);

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_run_get_method_with_json_data() {
    // Use the same JSON test data as transaction test to get a real account
    let json_path = "src/tests/4C90C139A5736F34EA3EEF62F0B06431719913835EA5A1B9173F20B2EF711583_66815326000001.json";
    let json_str = std::fs::read_to_string(json_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Extract account from shard_account_boc
    let shard_account_boc_str = json["shard_account_boc"].as_str().unwrap();
    let shard_account_cell =
        read_single_root_boc(base64_decode(shard_account_boc_str).unwrap()).unwrap();
    let shard_acc = ShardAccount::construct_from_cell(shard_account_cell).unwrap();
    let account = shard_acc.read_account().unwrap();

    let code = account.get_code().expect("Account has no code");
    let data = account.get_data().unwrap_or_default();

    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();
    let config_params_boc = CString::new(json["config_params_boc"].as_str().unwrap()).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 4);
    assert!(!p.is_null());

    // Set c7 with config
    let address = account.get_addr().unwrap();
    let address_str =
        CString::new(format!("{}:{:x}", address.workchain_id(), address.address())).unwrap();
    let rand_seed = CString::new(json["rand_seed"].as_str().unwrap()).unwrap();

    tvm_emulator_set_c7(
        p,
        address_str.as_ptr(),
        json["now"].as_u64().unwrap_or(1700000000) as u32,
        1_000_000_000,
        rand_seed.as_ptr(),
        config_params_boc.as_ptr(),
    );

    tvm_emulator_set_gas_limit(p, 1_000_000);

    // Try running seqno (method_id = 85143)
    let result = tvm_emulator_run_get_method(p, 85143, std::ptr::null());
    assert!(!result.is_null());

    let result_str = unsafe { CString::from_raw(result as *mut c_char) };
    let result_json: serde_json::Value = serde_json::from_slice(result_str.as_bytes()).unwrap();
    println!("run_get_method (json data) result: {}", result_json);

    assert_eq!(result_json["vm_exit_code"].as_i64().unwrap(), 0);
    assert_eq!(result_json["gas_used"].as_i64().unwrap(), 571);

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_send_internal_message() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 4);
    assert!(!p.is_null());

    let address =
        CString::new("0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap();
    let rand_seed =
        CString::new("0000000000000000000000000000000000000000000000000000000000000000").unwrap();
    tvm_emulator_set_c7(
        p,
        address.as_ptr(),
        1700000000,
        1_000_000_000,
        rand_seed.as_ptr(),
        std::ptr::null(),
    );

    // Create an empty message body cell
    let empty_body = Cell::default();
    let body_boc = CString::new(cell_to_base64(&empty_body)).unwrap();

    let result = tvm_emulator_send_internal_message(p, body_boc.as_ptr(), 1_000_000);
    assert!(!result.is_null(), "tvm_emulator_send_internal_message returned null");

    let result_str = unsafe { CString::from_raw(result as *mut c_char) };
    let result_json: serde_json::Value = serde_json::from_slice(result_str.as_bytes()).unwrap();
    println!("send_internal_message result: {}", result_json);

    assert!(result_json.get("vm_exit_code").is_some());
    assert!(result_json.get("gas_used").is_some());

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_send_external_message() {
    let (code, data) = load_account_code_data("../executor/real_boc/simple_account_old.boc");
    let code_boc = CString::new(cell_to_base64(&code)).unwrap();
    let data_boc = CString::new(cell_to_base64(&data)).unwrap();

    let p = tvm_emulator_create(code_boc.as_ptr(), data_boc.as_ptr(), 4);
    assert!(!p.is_null());

    let address =
        CString::new("0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap();
    let rand_seed =
        CString::new("0000000000000000000000000000000000000000000000000000000000000000").unwrap();
    tvm_emulator_set_c7(
        p,
        address.as_ptr(),
        1700000000,
        1_000_000_000,
        rand_seed.as_ptr(),
        std::ptr::null(),
    );

    let empty_body = Cell::default();
    let body_boc = CString::new(cell_to_base64(&empty_body)).unwrap();

    let result = tvm_emulator_send_external_message(p, body_boc.as_ptr());
    assert!(!result.is_null(), "tvm_emulator_send_external_message returned null");

    let result_str = unsafe { CString::from_raw(result as *mut c_char) };
    let result_json: serde_json::Value = serde_json::from_slice(result_str.as_bytes()).unwrap();
    println!("send_external_message result: {}", result_json);

    assert!(result_json.get("vm_exit_code").is_some());
    assert!(result_json.get("gas_used").is_some());

    tvm_emulator_destroy(p);
}

#[test]
fn test_tvm_emulator_null_safety() {
    // All functions should handle null pointers gracefully
    assert!(tvm_emulator_create(std::ptr::null(), std::ptr::null(), 0).is_null());
    assert!(!tvm_emulator_set_gas_limit(std::ptr::null_mut(), 100));
    assert!(!tvm_emulator_set_debug_enabled(std::ptr::null_mut(), true));
    assert!(!tvm_emulator_set_libraries(std::ptr::null_mut(), std::ptr::null()));
    assert!(!tvm_emulator_set_c7(
        std::ptr::null_mut(),
        std::ptr::null(),
        0,
        0,
        std::ptr::null(),
        std::ptr::null(),
    ));
    assert!(tvm_emulator_run_get_method(std::ptr::null_mut(), 0, std::ptr::null()).is_null());
    assert!(tvm_emulator_send_external_message(std::ptr::null_mut(), std::ptr::null()).is_null());
    assert!(
        tvm_emulator_send_internal_message(std::ptr::null_mut(), std::ptr::null(), 0).is_null()
    );
    // destroy with null should not crash
    tvm_emulator_destroy(std::ptr::null_mut());
}
