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
    base64_decode, base64_encode, read_single_root_boc, read_single_root_boc_file, write_boc,
    Serializable, Transaction,
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
    let account_root = read_single_root_boc_file(account).unwrap();
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

    // 5. Create emulator
    let p = transaction_emulator_create(config_params_boc.as_ptr(), 5);

    // 6. Set block parameters
    let unixtime = json["now"].as_u64().unwrap();
    let lt = json["lt"].as_u64().unwrap();
    transaction_emulator_set_unixtime(p, unixtime as u32);
    transaction_emulator_set_lt(p, lt);

    // rand_seed
    let rand_seed_str = json["rand_seed"].as_str().unwrap();
    let rand_seed_hex = CString::new(rand_seed_str).unwrap();
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
