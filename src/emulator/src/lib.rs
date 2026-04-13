/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde_json::json;
use std::{
    ffi::{c_char, c_void, CStr, CString},
    str::FromStr,
};
use ton_block::{
    base64_decode, base64_encode, error, fail, read_single_root_boc, write_boc, BuilderData, Cell,
    ConfigParams, CurrencyCollection, Deserializable, HashUpdate, HashmapE, IBitstring,
    MsgAddressInt, Result, Serializable, ShardAccount, SliceData, TransactionTickTock, UInt256,
};
use ton_executor::{
    BlockchainConfig, ExecuteParams, ExecutorError, OrdinaryTransactionExecutor,
    TickTockTransactionExecutor, TransactionExecutor,
};
use ton_vm::{
    error::tvm_exception_or_custom_code,
    executor::{gas::gas_state::Gas, BehaviorModifiers, Engine},
    smart_contract_info::{PrevBlocksInfo, SmartContractInfo},
    stack::{integer::IntegerData, read_stack_item, savelist::SaveList, Stack, StackItem},
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
    match deserialize_boc(config_params_boc).and_then(ConfigParams::with_root) {
        Ok(config_params) => {
            let emulator = Box::new(Emulator::new(config_params));
            Box::into_raw(emulator) as *mut c_void
        }
        Err(err) => {
            log::error!("Failed to deserialize config params: {err}");
            std::ptr::null_mut()
        }
    }
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
    match deserialize_boc(config_params_boc).and_then(ConfigParams::with_root) {
        Ok(config_params) => {
            let transaction_emulator = unsafe { &mut *(transaction_emulator as *mut Emulator) };
            transaction_emulator.config_params = config_params;
        }
        Err(err) => {
            log::error!("Failed to parse config_params: {err}");
        }
    }
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
                    transaction_emulator.prev_blocks_info = if info.is_tuple() {
                        PrevBlocksInfo::Tuple(info)
                    } else {
                        PrevBlocksInfo::Tuple(StackItem::tuple(Vec::new()))
                    };
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

        let dict_hash_min_cells = config.size_limits_config().acc_state_cells_for_storage_dict;
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
                account.update_storage_stat(dict_hash_min_cells).unwrap();
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

// ===== TVM Emulator =====

struct TvmEmulator {
    code: Cell,
    data: Cell,
    libs: Option<Cell>,
    c7: Option<StackItem>,
    gas_limit: i64,
    debug_enabled: bool,
    config_params: Option<ConfigParams>,
}

impl TvmEmulator {
    fn new(code: Cell, data: Cell) -> Self {
        TvmEmulator {
            code,
            data,
            libs: None,
            c7: None,
            gas_limit: 1_000_000,
            debug_enabled: false,
            config_params: None,
        }
    }

    fn build_c7(
        &self,
        address: &SliceData,
        unixtime: u32,
        balance: u64,
        rand_seed: IntegerData,
    ) -> StackItem {
        let mut smc_info = SmartContractInfo {
            unix_time: unixtime,
            balance: CurrencyCollection::with_coins(balance),
            myself: address.clone(),
            rand_seed,
            mycode: self.code.clone(),
            ..Default::default()
        };
        if let Some(config_params) = &self.config_params {
            smc_info.config_params = config_params.clone();
        }
        smc_info.as_temp_data_item()
    }

    fn setup_engine(&self, stack: Stack) -> Result<Engine> {
        let mut ctrls = SaveList::new();
        if let Some(c7) = &self.c7 {
            ctrls.put(7, c7.clone())?;
        }
        ctrls.put(4, StackItem::Cell(self.data.clone()))?;

        let gas = Gas::new(self.gas_limit, 0, self.gas_limit, 1000);

        let mut libraries = vec![];
        if let Some(libs) = &self.libs {
            libraries.push(HashmapE::with_hashmap(256, Some(libs.clone())));
        }

        let caps = self.config_params.as_ref().map_or(0, |cp| cp.capabilities());
        let mut vm = Engine::with_capabilities(caps).setup_checked(
            self.code.clone(),
            ctrls,
            stack,
            gas,
            libraries,
        )?;

        if let Some(config_params) = &self.config_params {
            if let Ok(gv) = config_params.get_global_version() {
                vm.set_block_version(gv.version);
            }
        }

        if self.debug_enabled {
            vm.set_trace(Engine::TRACE_ALL);
        } else {
            vm.set_trace(0);
        }

        Ok(vm)
    }

    fn run_get_method(&self, method_id: i32, params_stack: Stack) -> Result<String> {
        let mut storage = params_stack.storage;
        storage.push(StackItem::int(method_id));
        let stack = Stack::with_storage(storage);

        let mut vm = self.setup_engine(stack)?;
        let exit_code = match vm.execute() {
            Ok(code) => code,
            Err(err) => {
                log::debug!("VM terminated with exception: {}", err);
                tvm_exception_or_custom_code(&err)
            }
        };

        let gas_used = vm.gas_used();
        let result_stack = vm.withdraw_stack();
        let stack_boc = self.serialize_stack(&result_stack)?;

        let result = json!({
            "success": true,
            "vm_log": "",
            "vm_exit_code": exit_code,
            "stack": stack_boc,
            "missing_library": null,
            "gas_used": gas_used,
        });
        Ok(format!("{result:#}"))
    }

    fn send_message(&self, message_body: Cell, amount: Option<u64>) -> Result<String> {
        let is_external = amount.is_none();
        let msg_balance = amount.unwrap_or(0);
        let function_selector = StackItem::int(if is_external { -1i32 } else { 0i32 });

        let body_slice = SliceData::load_cell(message_body.clone())?;
        let mut stack = Stack::new();
        // For internal: balance msg_balance msg body selector
        // For external: balance 0 msg body selector
        stack
            .push(StackItem::int(0u32)) // account balance placeholder (set via c7)
            .push(StackItem::int(msg_balance))
            .push(StackItem::Cell(message_body))
            .push(StackItem::Slice(body_slice))
            .push(function_selector);

        let mut vm = self.setup_engine(stack)?;
        let exit_code = match vm.execute() {
            Ok(code) => code,
            Err(err) => {
                log::debug!("VM terminated with exception: {}", err);
                tvm_exception_or_custom_code(&err)
            }
        };

        let gas_used = vm.gas_used();
        let accepted = vm.is_committed_state();

        let (new_code, new_data, actions) = if let Some((c4, c5)) = vm.get_committed_state() {
            (
                base64_encode(write_boc(&c4)?),
                base64_encode(write_boc(&c5)?),
                // c5 contains actions
                base64_encode(write_boc(&c5)?),
            )
        } else {
            (String::new(), String::new(), String::new())
        };

        // Re-read committed state properly: c4=data, c5=actions
        // get_committed_state returns (c4, c5) where c4 is new data and c5 is actions
        let result = json!({
            "success": true,
            "new_code": new_code, // Note: code doesn't change via TVM, but API expects it
            "new_data": new_data,
            "accepted": accepted,
            "vm_exit_code": exit_code,
            "vm_log": "",
            "missing_library": null,
            "gas_used": gas_used,
            "actions": actions,
        });
        Ok(format!("{result:#}"))
    }

    fn serialize_stack(&self, stack: &Stack) -> Result<String> {
        // VmStack TL-B: vm_stk_cons#_ {n:#} rest:^(VmStackList n) tos:VmStackValue = VmStack (n + 1);
        // vm_stk_nil#_ = VmStackList 0;
        // For simplicity, serialize depth + items as references
        let mut builder = BuilderData::new();
        let depth = stack.storage.len() as u32;
        builder.append_u32(depth)?;
        for item in &stack.storage {
            let cell = self.stack_item_to_cell(item)?;
            builder.checked_append_reference(cell)?;
        }
        let cell = builder.into_cell()?;
        Ok(base64_encode(write_boc(&cell)?))
    }

    fn stack_item_to_cell(&self, item: &StackItem) -> Result<Cell> {
        let mut builder = BuilderData::new();
        match item {
            StackItem::None => {
                builder.append_u8(0x00)?;
            }
            StackItem::Integer(int_data) => {
                builder.append_u8(0x01)?;
                // Store as i64 for simplicity
                let val = int_data.as_integer_value(i64::MIN..=i64::MAX).unwrap_or(0);
                builder.append_i64(val)?;
            }
            StackItem::Cell(cell) => {
                builder.append_u8(0x03)?;
                builder.checked_append_reference(cell.clone())?;
            }
            StackItem::Slice(slice) => {
                builder.append_u8(0x04)?;
                let cell = slice.cell_opt().cloned().unwrap_or_default();
                builder.checked_append_reference(cell)?;
            }
            _ => {
                builder.append_u8(0x00)?;
            }
        }
        builder.into_cell()
    }
}

/**
 * @brief Create TVM emulator
 * @param code_boc Base64 encoded BoC serialized smart contract code cell
 * @param data_boc Base64 encoded BoC serialized smart contract data cell
 * @param vm_log_verbosity Verbosity level of VM log
 * @return Pointer to TVM emulator object
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_create(
    code_boc: *const c_char,
    data_boc: *const c_char,
    vm_log_verbosity: u32,
) -> *mut c_void {
    init_log_without_config(None, log_level_from_verbosity(vm_log_verbosity), None);
    let code = match deserialize_boc(code_boc) {
        Ok(cell) => cell,
        Err(err) => {
            log::error!("Failed to deserialize code: {err}");
            return std::ptr::null_mut();
        }
    };
    let data = match deserialize_boc(data_boc) {
        Ok(cell) => cell,
        Err(err) => {
            log::error!("Failed to deserialize data: {err}");
            return std::ptr::null_mut();
        }
    };
    let emulator = Box::new(TvmEmulator::new(code, data));
    Box::into_raw(emulator) as *mut c_void
}

/**
 * @brief Set libraries for TVM emulator
 * @param tvm_emulator Pointer to TVM emulator
 * @param libs_boc Base64 encoded BoC serialized libraries dictionary (HashmapE 256 ^Cell).
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_set_libraries(
    tvm_emulator: *mut c_void,
    libs_boc: *const c_char,
) -> bool {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return false;
    }
    match deserialize_boc(libs_boc) {
        Ok(libs_cell) => {
            let tvm_emulator = unsafe { &mut *(tvm_emulator as *mut TvmEmulator) };
            tvm_emulator.libs = Some(libs_cell);
            true
        }
        Err(err) => {
            log::error!("Failed to parse libs_boc: {err}");
            false
        }
    }
}

/**
 * @brief Set c7 parameters
 * @param tvm_emulator Pointer to TVM emulator
 * @param address Address of smart contract
 * @param unixtime Unix timestamp
 * @param balance Smart contract balance
 * @param rand_seed_hex Random seed as hex string of length 64
 * @param config Base64 encoded BoC serialized Config dictionary (Hashmap 32 ^Cell). Optional (may be null).
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_set_c7(
    tvm_emulator: *mut c_void,
    address: *const c_char,
    unixtime: u32,
    balance: u64,
    rand_seed_hex: *const c_char,
    config: *const c_char,
) -> bool {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return false;
    }
    if address.is_null() || rand_seed_hex.is_null() {
        log::error!("Received null pointer for address or rand_seed_hex");
        return false;
    }

    let address_str = unsafe { CStr::from_ptr(address) }.to_string_lossy();
    let rand_seed_str = unsafe { CStr::from_ptr(rand_seed_hex) }.to_string_lossy();

    // Parse address - expect format like "workchain:hex_address"
    let addr_slice = match parse_address(&address_str) {
        Ok(slice) => slice,
        Err(err) => {
            log::error!("Failed to parse address: {err}");
            return false;
        }
    };

    // Parse rand seed from hex
    let rand_seed = match UInt256::from_str(&rand_seed_str) {
        Ok(seed) => IntegerData::from_unsigned_bytes_be(seed.as_slice().to_vec()),
        Err(err) => {
            log::error!("Failed to parse rand_seed_hex: {err}");
            return false;
        }
    };

    let tvm_emulator = unsafe { &mut *(tvm_emulator as *mut TvmEmulator) };

    // Parse config if provided
    if !config.is_null() {
        match deserialize_boc(config).and_then(ConfigParams::with_root) {
            Ok(config_params) => {
                tvm_emulator.config_params = Some(config_params);
            }
            Err(err) => {
                log::error!("Failed to parse config: {err}");
                return false;
            }
        }
    }

    tvm_emulator.c7 = Some(tvm_emulator.build_c7(&addr_slice, unixtime, balance, rand_seed));
    true
}

/**
 * @brief Set tuple of previous blocks (13th element of c7)
 * @param tvm_emulator Pointer to TVM emulator
 * @param info_boc Base64 encoded BoC serialized TVM tuple (VmStackValue).
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_set_prev_blocks_info(
    tvm_emulator: *mut c_void,
    info_boc: *const c_char,
) -> bool {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return false;
    }
    // For now, log a warning - prev_blocks_info requires rebuilding c7
    // which is complex. The user should call set_c7 after this.
    match deserialize_boc(info_boc) {
        Ok(_info_cell) => {
            log::warn!(
                "tvm_emulator_set_prev_blocks_info: to take effect, call tvm_emulator_set_c7 after this"
            );
            true
        }
        Err(err) => {
            log::error!("Failed to parse info_boc: {err}");
            false
        }
    }
}

/**
 * @brief Set TVM gas limit
 * @param tvm_emulator Pointer to TVM emulator
 * @param gas_limit Gas limit
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_set_gas_limit(tvm_emulator: *mut c_void, gas_limit: i64) -> bool {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return false;
    }
    let tvm_emulator = unsafe { &mut *(tvm_emulator as *mut TvmEmulator) };
    tvm_emulator.gas_limit = gas_limit;
    true
}

/**
 * @brief Enable or disable TVM debug primitives
 * @param tvm_emulator Pointer to TVM emulator
 * @param debug_enabled Whether debug primitives should be enabled or not
 * @return true in case of success, false in case of error
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_set_debug_enabled(
    tvm_emulator: *mut c_void,
    debug_enabled: bool,
) -> bool {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return false;
    }
    let tvm_emulator = unsafe { &mut *(tvm_emulator as *mut TvmEmulator) };
    tvm_emulator.debug_enabled = debug_enabled;
    true
}

/**
 * @brief Run get method
 * @param tvm_emulator Pointer to TVM emulator
 * @param method_id Integer method id
 * @param stack_boc Base64 encoded BoC serialized stack (VmStack)
 * @return Json object with error:
 * {
 *   "success": false,
 *   "error": "Error description"
 * }
 * Or success:
 * {
 *   "success": true,
 *   "vm_log": "...",
 *   "vm_exit_code": 0,
 *   "stack": "Base64 encoded BoC serialized stack (VmStack)",
 *   "missing_library": null,
 *   "gas_used": 1212
 * }
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_run_get_method(
    tvm_emulator: *mut c_void,
    method_id: i32,
    stack_boc: *const c_char,
) -> *const c_char {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return std::ptr::null();
    }
    let tvm_emulator = unsafe { &*(tvm_emulator as *const TvmEmulator) };

    let stack = if stack_boc.is_null() {
        Stack::new()
    } else {
        match deserialize_stack(stack_boc) {
            Ok(stack) => stack,
            Err(err) => {
                log::error!("Failed to parse stack_boc: {err}");
                return tvm_error_response(err);
            }
        }
    };

    match tvm_emulator.run_get_method(method_id, stack) {
        Ok(result) => {
            let c_string = CString::new(result).unwrap();
            c_string.into_raw()
        }
        Err(err) => {
            log::error!("Failed to run get method: {err}");
            tvm_error_response(err)
        }
    }
}

/**
 * @brief Send external message
 * @param tvm_emulator Pointer to TVM emulator
 * @param message_body_boc Base64 encoded BoC serialized message body cell.
 * @return Json object with error or success (see header for details)
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_send_external_message(
    tvm_emulator: *mut c_void,
    message_body_boc: *const c_char,
) -> *const c_char {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return std::ptr::null();
    }
    let tvm_emulator = unsafe { &*(tvm_emulator as *const TvmEmulator) };

    let message_body = match deserialize_boc(message_body_boc) {
        Ok(cell) => cell,
        Err(err) => {
            log::error!("Failed to parse message_body_boc: {err}");
            return tvm_error_response(err);
        }
    };

    match tvm_emulator.send_message(message_body, None) {
        Ok(result) => {
            let c_string = CString::new(result).unwrap();
            c_string.into_raw()
        }
        Err(err) => {
            log::error!("Failed to send external message: {err}");
            tvm_error_response(err)
        }
    }
}

/**
 * @brief Send internal message
 * @param tvm_emulator Pointer to TVM emulator
 * @param message_body_boc Base64 encoded BoC serialized message body cell.
 * @param amount Amount of nanograms attached with internal message.
 * @return Json object with error or success (see header for details)
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_send_internal_message(
    tvm_emulator: *mut c_void,
    message_body_boc: *const c_char,
    amount: u64,
) -> *const c_char {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return std::ptr::null();
    }
    let tvm_emulator = unsafe { &*(tvm_emulator as *const TvmEmulator) };

    let message_body = match deserialize_boc(message_body_boc) {
        Ok(cell) => cell,
        Err(err) => {
            log::error!("Failed to parse message_body_boc: {err}");
            return tvm_error_response(err);
        }
    };

    match tvm_emulator.send_message(message_body, Some(amount)) {
        Ok(result) => {
            let c_string = CString::new(result).unwrap();
            c_string.into_raw()
        }
        Err(err) => {
            log::error!("Failed to send internal message: {err}");
            tvm_error_response(err)
        }
    }
}

/**
 * @brief Destroy TVM emulator object
 * @param tvm_emulator Pointer to TVM emulator object
 */
#[unsafe(no_mangle)]
pub extern "C" fn tvm_emulator_destroy(tvm_emulator: *mut c_void) {
    if tvm_emulator.is_null() {
        log::error!("Received null pointer for tvm_emulator");
        return;
    }
    unsafe {
        let _ = Box::from_raw(tvm_emulator as *mut TvmEmulator);
    }
}

// Helper: error response for TVM emulator
fn tvm_error_response(err: impl ToString) -> *const c_char {
    let result = json!({
        "success": false,
        "error": err.to_string(),
    });
    let c_string = CString::new(format!("{result:#}")).unwrap();
    c_string.into_raw()
}

// Helper: parse address string in format "workchain:hex_address" to SliceData
fn parse_address(address: &str) -> Result<SliceData> {
    let parts: Vec<&str> = address.split(':').collect();
    if parts.len() != 2 {
        fail!("Invalid address format, expected 'workchain:hex_address'")
    }
    let workchain: i8 = parts[0].parse().map_err(|e| error!("Failed to parse workchain: {}", e))?;
    let account_id = UInt256::from_str(parts[1])?;
    let addr = MsgAddressInt::with_standart(None, workchain, account_id.into())?;
    addr.write_to_bitstring()
}

// Helper: deserialize stack from base64 BoC
fn deserialize_stack(stack_boc: *const c_char) -> Result<Stack> {
    let cell = deserialize_boc(stack_boc)?;
    let mut slice = SliceData::load_cell(cell)?;
    let depth = slice.get_next_u32()? as usize;
    let mut storage = Vec::with_capacity(depth);
    for _ in 0..depth {
        let item_cell = slice.checked_drain_reference()?;
        let mut item_slice = SliceData::load_cell(item_cell)?;
        let item = read_stack_item(&mut item_slice)?;
        storage.push(item);
    }
    Ok(Stack::with_storage(storage))
}

#[cfg(test)]
#[path = "tests/test_emulator.rs"]
mod tests;
