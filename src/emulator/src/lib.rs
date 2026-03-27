/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde_json::json;
use std::ffi::{c_char, c_void, CStr, CString};
use ton_block::{
    base64_decode, base64_encode, fail, read_single_root_boc, write_boc, Cell, ConfigParams,
    Deserializable, HashUpdate, HashmapE, Result, Serializable, ShardAccount, SliceData,
    TransactionTickTock, UInt256,
};
use ton_executor::{
    BlockchainConfig, ExecuteParams, ExecutorError, OrdinaryTransactionExecutor,
    TickTockTransactionExecutor, TransactionExecutor,
};
use ton_vm::{
    executor::BehaviorModifiers,
    smart_contract_info::PrevBlocksInfo,
    stack::{read_stack_item, StackItem},
};

include!("../../common/src/log.rs");

fn deserialize_boc(boc_ptr: *const c_char) -> Result<Cell> {
    if boc_ptr.is_null() {
        fail!("Received null pointer")
    }
    let boc_cstr = unsafe { CStr::from_ptr(boc_ptr) };
    let data = base64_decode(boc_cstr.to_string_lossy().as_bytes())?;
    read_single_root_boc(data)
}

fn deserialize_object<T: Deserializable>(boc_ptr: *const c_char) -> Result<T> {
    let cell = deserialize_boc(boc_ptr)?;
    T::construct_from_cell(cell)
}

fn log_level_from_verbosity(verbosity_level: u32) -> log::LevelFilter {
    match verbosity_level {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    }
}

/**
 * @brief Creates TransactionEmulator object
 * @param config_params_boc Base64 encoded BoC serialized Config dictionary (Hashmap 32 ^Cell)
 * @param vm_log_verbosity Verbosity level of VM log. 0 - log truncated to last 256 characters. 1 - unlimited length log.
 * 2 - for each command prints its cell hash and offset. 3 - for each command log prints all stack values.
 * @return Pointer to TransactionEmulator or nullptr in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_create(
    config_params_boc: *const c_char,
    vm_log_verbosity: u32,
) -> *mut c_void {
    init_log_without_config(None, log_level_from_verbosity(vm_log_verbosity), None);
    let err = match deserialize_boc(config_params_boc) {
        Ok(config_params_root) => match ConfigParams::with_root(config_params_root) {
            Ok(config_params) => {
                let emulator = Box::new(Emulator::new(config_params));
                return Box::into_raw(emulator) as *mut c_void;
            }
            Err(err) => err,
        },
        Err(err) => err,
    };
    log::error!("Failed to deserialize config params: {err}");
    std::ptr::null_mut()
}

/**
 * @brief Set unixtime for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param unixtime Unix timestamp
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_unixtime(
    transaction_emulator: *mut c_void,
    unixtime: u32,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
    transaction_emulator.unixtime = unixtime;
}

/**
 * @brief Set lt for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param lt Logical time
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_lt(transaction_emulator: *mut c_void, lt: u64) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
    transaction_emulator.lt = lt;
}

/**
 * @brief Set rand seed for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param rand_seed_hex Hex string of length 64
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_rand_seed(
    transaction_emulator: *mut c_void,
    rand_seed_hex: *const c_char,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    if rand_seed_hex.is_null() {
        log::error!("Received null pointer for rand_seed_hex");
        return;
    }
    let rand_seed_hex = unsafe { CStr::from_ptr(rand_seed_hex) };
    match rand_seed_hex.to_string_lossy().parse() {
        Ok(rand_seed) => {
            let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
            transaction_emulator.rand_seed = rand_seed;
        }
        Err(err) => {
            log::error!("Failed to parse rand_seed_hex: {err}");
        }
    }
}

/**
 * @brief Set ignore_chksig flag for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param ignore_chksig Whether emulation should always succeed on CHKSIG operation
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_ignore_chksig(
    transaction_emulator: *mut c_void,
    ignore_chksig: bool,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
    transaction_emulator.ignore_chksig = ignore_chksig;
}

/**
 * @brief Set config for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param config_boc Base64 encoded BoC serialized Config dictionary (Hashmap 32 ^Cell)
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_config(
    transaction_emulator: *mut c_void,
    config_params_boc: *const c_char,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    let err = match deserialize_boc(config_params_boc) {
        Ok(config_params_root) => match ConfigParams::with_root(config_params_root) {
            Ok(config_params) => {
                let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
                transaction_emulator.config_params = config_params;
                return;
            }
            Err(err) => err,
        },
        Err(err) => err,
    };
    log::error!("Failed to parse config_params: {err}");
}

/**
 * @brief Set libraries for emulation
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param libs_boc Base64 encoded BoC serialized shared libraries dictionary (HashmapE 256 ^Cell).
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_libs(
    transaction_emulator: *mut c_void,
    libs_boc: *const c_char,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    match deserialize_boc(libs_boc) {
        Ok(libs_cell) => {
            let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
            transaction_emulator.libs = Some(libs_cell);
        }
        Err(err) => {
            log::error!("Failed to parse libs_boc: {err}");
        }
    }
}

/**
 * @brief Set tuple of previous blocks (13th element of c7)
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param info_boc Base64 encoded BoC serialized TVM tuple (VmStackValue).
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_set_prev_blocks_info(
    transaction_emulator: *mut c_void,
    info_boc: *const c_char,
) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    match deserialize_boc(info_boc) {
        Ok(info_cell) => {
            let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
            match SliceData::load_cell(info_cell).and_then(|mut slice| read_stack_item(&mut slice))
            {
                Ok(info) => {
                    if info.is_tuple() {
                        transaction_emulator.prev_blocks_info = PrevBlocksInfo::Tuple(info);
                    } else {
                        transaction_emulator.prev_blocks_info =
                            PrevBlocksInfo::Tuple(StackItem::tuple(Vec::new()));
                    }
                }
                Err(err) => {
                    log::error!("Failed to parse info_cell: {err}");
                }
            }
        }
        Err(err) => {
            log::error!("Failed to parse info_boc: {err}");
        }
    }
}

// Helper function to create error response JSON and convert it to C string.
fn error_response(err: impl ToString) -> *const c_char {
    let result = json!({
        "success": false,
        "error": err.to_string(),
        "external_not_accepted": false,
    });
    let c_string = CString::new(format!("{result:#}")).unwrap();
    c_string.into_raw()
}

/**
 * @brief Emulate transaction
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param shard_account_boc Base64 encoded BoC serialized ShardAccount
 * @param message_boc Base64 encoded BoC serialized inbound Message (internal or external)
 * @return Json object with error:
 * {
 *   "success": false,
 *   "error": "Error description",
 *   "external_not_accepted": false,
 *   // and optional fields "vm_exit_code", "vm_log", "elapsed_time" in case external message was not accepted.
 * }
 * Or success:
 * {
 *   "success": true,
 *   "transaction": "Base64 encoded Transaction boc",
 *   "shard_account": "Base64 encoded new ShardAccount boc",
 *   "vm_log": "execute DUP...",
 *   "actions": "Base64 encoded compute phase actions boc (OutList n)",
 *   "elapsed_time": 0.02
 * }
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_emulate_transaction(
    transaction_emulator: *mut c_void,
    shard_account_boc: *const c_char,
    message_boc: *const c_char,
) -> *const c_char {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return std::ptr::null_mut();
    }
    let shard_acc = match deserialize_object(shard_account_boc) {
        Ok(shard_acc) => shard_acc,
        Err(err) => {
            log::error!("Failed to parse shard_account_boc: {err}");
            return error_response(err);
        }
    };
    let in_msg_cell = match deserialize_boc(message_boc) {
        Ok(cell) => cell,
        Err(err) => {
            log::error!("Failed to parse message_boc: {err}");
            return error_response(err);
        }
    };
    let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
    match transaction_emulator.emulate_transaction(shard_acc, Some(in_msg_cell), false) {
        Ok(result) => {
            let c_string = CString::new(result).unwrap();
            c_string.into_raw()
        }
        Err(err) => {
            log::error!("Failed to emulate transaction: {err}");
            error_response(err)
        }
    }
}

/**
 * @brief Emulate tick tock transaction
 * @param transaction_emulator Pointer to TransactionEmulator object
 * @param shard_account_boc Base64 encoded BoC serialized ShardAccount of special account
 * @param is_tock True for tock transactions, false for tick
 * @return Json object with error:
 * {
 *   "success": false,
 *   "error": "Error description",
 *   "external_not_accepted": false
 * }
 * Or success:
 * {
 *   "success": true,
 *   "transaction": "Base64 encoded Transaction boc",
 *   "shard_account": "Base64 encoded new ShardAccount boc",
 *   "vm_log": "execute DUP...",
 *   "actions": "Base64 encoded compute phase actions boc (OutList n)",
 *   "elapsed_time": 0.02
 * }
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_emulate_tick_tock_transaction(
    transaction_emulator: *mut c_void,
    shard_account_boc: *const c_char,
    is_tock: bool,
) -> *const c_char {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return std::ptr::null_mut();
    }
    let shard_acc = match deserialize_object(shard_account_boc) {
        Ok(shard_acc) => shard_acc,
        Err(err) => {
            log::error!("Failed to parse shard_account_boc: {err}");
            return error_response(err);
        }
    };
    let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
    match transaction_emulator.emulate_transaction(shard_acc, None, is_tock) {
        Ok(result) => {
            let c_string = CString::new(result).unwrap();
            c_string.into_raw()
        }
        Err(err) => {
            log::error!("Failed to emulate transaction: {err}");
            error_response(err)
        }
    }
}

/**
 * @brief Destroy TransactionEmulator object
 * @param transaction_emulator Pointer to TransactionEmulator object
 */
#[unsafe(no_mangle)]
pub extern "C" fn transaction_emulator_destroy(transaction_emulator: *mut c_void) {
    if transaction_emulator.is_null() {
        log::error!("Received null pointer for transaction_emulator");
        return;
    }
    unsafe {
        let _ = Box::from_raw(transaction_emulator as *mut Emulator);
    }
}

/**
 * @brief Set global verbosity level of the library
 * @param verbosity_level New verbosity level (0 - never, 1 - error, 2 - warning, 3 - info, 4 - debug)
 */
#[unsafe(no_mangle)]
pub extern "C" fn emulator_set_verbosity_level(verbosity_level: u32) {
    let log_level = log_level_from_verbosity(verbosity_level);
    log::set_max_level(log_level);
}

/**
 * @brief Destroy string created by emulator library
 * @param string Pointer to string to destroy
 *
 * This function should be used to free strings returned by emulator library functions.
 * It is not safe to use caller's free() on them, as they may have been allocated using a different allocator.
 */
#[unsafe(no_mangle)]
pub extern "C" fn string_destroy(string: *mut c_void) {
    if string.is_null() {
        log::error!("Received null pointer for destroy string");
        return;
    }
    unsafe {
        let _ = CString::from_raw(string as *mut c_char);
    }
}

/**
 * @brief Get git commit hash and date of the library
 */
#[unsafe(no_mangle)]
pub extern "C" fn emulator_version() -> *const c_char {
    let result = json!({
        "emulatorLibCommitHash": std::option_env!("BUILD_GIT_COMMIT").unwrap_or("Not set"),
        "emulatorLibCommitDate": std::option_env!("BUILD_GIT_DATE").unwrap_or("Not set"),
    });
    let c_string = CString::new(result.to_string()).unwrap();
    c_string.into_raw()
}

#[derive(Default)]
struct Emulator {
    config_params: ConfigParams,
    unixtime: u32,
    lt: u64,
    rand_seed: UInt256,
    ignore_chksig: bool,
    prev_blocks_info: PrevBlocksInfo,
    libs: Option<Cell>,
}

impl Emulator {
    fn new(config_params: ConfigParams) -> Self {
        Emulator { config_params, ..Default::default() }
    }

    fn emulate_transaction(
        &self,
        mut shard_acc: ShardAccount,
        in_msg_cell: Option<Cell>,
        is_tock: bool,
    ) -> Result<String> {
        let config = BlockchainConfig::with_config(self.config_params.clone())
            .inspect_err(|err| log::error!("Failed to create BlockchainConfig: {err}"))?;

        let executor: Box<dyn TransactionExecutor> = if in_msg_cell.is_some() {
            Box::new(OrdinaryTransactionExecutor::new(config))
        } else {
            Box::new(TickTockTransactionExecutor::new(config, TransactionTickTock::new(is_tock)))
        };
        let last_tr_lt = self.lt;
        let block_lt = last_tr_lt - last_tr_lt % 1_000_000;
        let behavior_modifiers =
            Some(BehaviorModifiers { chksig_always_succeed: self.ignore_chksig });
        let params = ExecuteParams {
            block_lt,
            last_tr_lt,
            block_unixtime: self.unixtime,
            seed_block: self.rand_seed.clone(),
            state_libs: HashmapE::with_hashmap(256, self.libs.clone()),
            behavior_modifiers,
            prev_blocks_info: self.prev_blocks_info.clone(),
            ..Default::default()
        };
        let mut account = shard_acc
            .read_account()
            .inspect_err(|err| log::error!("Failed to read account: {err}"))?;
        let now = std::time::Instant::now();
        let result = executor.execute_with_params(in_msg_cell, &mut account, params);
        let elapsed_time = now.elapsed().as_micros() as i64;
        let result = match result {
            Ok(mut transaction) => {
                transaction.set_prev_trans_lt(shard_acc.last_trans_lt());
                transaction.set_prev_trans_hash(shard_acc.last_trans_hash().clone());
                let old_hash = shard_acc.account_hash();
                shard_acc.write_account(&account)?;
                let new_hash = shard_acc.account_hash();
                let hash_update = HashUpdate::with_hashes(old_hash, new_hash);
                transaction.write_state_update(&hash_update)?;
                let tr_cell = transaction.serialize()?;
                shard_acc.set_last_trans_hash(tr_cell.repr_hash());
                shard_acc.set_last_trans_lt(transaction.logical_time());
                let actions = json!(null);
                json!({
                    "success": true,
                    "transaction": base64_encode(write_boc(&tr_cell)?),
                    "shard_account": shard_acc.write_to_base64()?,
                    "vm_log": "",
                    "actions": actions,
                    "elapsed_time": elapsed_time,
                })
            }
            Err(err) => {
                if let Some(ExecutorError::NoAcceptError(vm_exit_code, _)) = err.downcast_ref() {
                    json!({
                        "success": false,
                        "error": "External message not accepted by smart contract",
                        "external_not_accepted": true,
                        "vm_log": "",
                        "vm_exit_code": vm_exit_code,
                        "elapsed_time": elapsed_time,
                    })
                } else {
                    json!({
                        "success": false,
                        "error": err.to_string(),
                        "external_not_accepted": false,
                    })
                }
            }
        };
        Ok(format!("{result:#}"))
    }
}

#[cfg(test)]
#[path = "tests/test_emulator.rs"]
mod tests;
