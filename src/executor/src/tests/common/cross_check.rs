/*
 * Copyright 2018-2020 EVERX DEV SOLUTIONS LTD.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![cfg(test)]
#![allow(dead_code)]

use super::*;
use crate::ExecuteParams;
use pretty_assertions::assert_eq;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicI32, Ordering},
        LazyLock, Mutex,
    },
    thread,
    thread::ThreadId,
};
use ton_block::{
    read_single_root_boc, write_boc, Account, Deserializable, Serializable, Transaction,
};

static DISABLED_TESTS: LazyLock<Mutex<HashSet<ThreadId>>> = LazyLock::new(Default::default);
static DISABLED_TESTS_SCOPE: LazyLock<Mutex<HashSet<ThreadId>>> = LazyLock::new(Default::default);
static VERBOSITY: AtomicI32 = AtomicI32::new(0);

pub(crate) fn disable_cross_check() {
    let thread_id = thread::current().id();
    DISABLED_TESTS.lock().unwrap().insert(thread_id);
}

pub(crate) fn set_cross_check_verbosity(verbosity: i32) {
    assert!(cfg!(feature = "cross_check"), "cross_check feature is not enabled");
    VERBOSITY.store(verbosity, Ordering::Relaxed);
}

pub struct DisableCrossCheck {
    thread_id: ThreadId,
}

impl DisableCrossCheck {
    pub fn new() -> DisableCrossCheck {
        let thread_id = thread::current().id();
        DISABLED_TESTS_SCOPE.lock().unwrap().insert(thread_id);
        DisableCrossCheck { thread_id }
    }
}

impl Drop for DisableCrossCheck {
    fn drop(&mut self) {
        DISABLED_TESTS.lock().unwrap().remove(&self.thread_id);
    }
}

fn load_cells(data: &[u8]) -> Cell {
    read_single_root_boc(data).unwrap()
}

pub(crate) fn cross_check(
    mc_state_proof: Cell,
    acc_before: &Account,
    acc_after: &Account,
    in_msg_cell: Option<&Cell>,
    transaction: Option<&Transaction>,
    params: &ExecuteParams,
    _it: u32,
) {
    let thread_id = thread::current().id();

    if DISABLED_TESTS.lock().unwrap().remove(&thread_id)
        || DISABLED_TESTS_SCOPE.lock().unwrap().contains(&thread_id)
    {
        return;
    }

    let proof = MerkleProof::construct_from_cell(mc_state_proof.clone()).unwrap();
    let mc_state: ShardStateUnsplit = proof.virtualize().unwrap();
    let extra = mc_state.read_custom().unwrap().unwrap();
    let block_version = extra.config.global_version();
    // assert!(extra.config.global_version() >= ton_block::SUPPORTED_VERSION, "global_version {} must be >= {}",
    //     config.global_version(), ton_block::SUPPORTED_VERSION);
    #[cfg(windows)]
    let lib_name = "../../ton/build/crypto/Release/vm_run_shared.dll";
    #[cfg(target_os = "linux")]
    let lib_name = "../../ton-node-cpp/build/crypto/libvm_run_shared.so";
    #[cfg(target_os = "macos")]
    let lib_name = "../../ton-node-cpp/build/crypto/libvm_run_shared.dylib";

    let lib = libloading::Library::new(lib_name).unwrap();

    // set_cross_check_verbosity(2048 + 4);

    // println!("auto acc = \"{}\"", acc_before.write_to_base64().unwrap());
    // println!("auto cfg = \"{}\"", config.raw_config().write_to_base64().unwrap());
    // println!("auto msg = \"{}\"", msg.unwrap().write_to_base64().unwrap());

    let acc_data = acc_before.write_to_bytes().unwrap();
    let cfg_data = write_boc(&mc_state_proof).unwrap();

    let mut res_acc_size: i32 = 10000000;
    let mut res_tx_size: i32 = 10000000;
    let mut res_acc = vec![0u8; res_acc_size as usize];
    let mut res_tx = vec![0u8; res_tx_size as usize];

    let verbosity = VERBOSITY.load(Ordering::Relaxed);
    if let Some(in_msg_cell) = in_msg_cell {
        let msg_data = write_boc(in_msg_cell).unwrap();

        unsafe {
            type RunBoc<'a> = libloading::Symbol<
                'a,
                unsafe extern "C" fn(
                    *const u8,
                    i32,
                    *const u8,
                    i32,
                    *const u8,
                    i32,
                    u64,
                    i32,
                    u64,
                    *mut u8,
                    &mut i32,
                    *mut u8,
                    &mut i32,
                    i32,
                ) -> bool,
            >;
            let run_boc: RunBoc = lib.get(b"replay_ordinary_transaction_ext").unwrap();

            let res: bool = run_boc(
                acc_data.as_ptr(),
                acc_data.len() as i32,
                msg_data.as_ptr(),
                msg_data.len() as i32,
                cfg_data.as_ptr(),
                cfg_data.len() as i32,
                params.last_tr_lt,
                params.block_unixtime as i32,
                params.block_lt,
                res_acc.as_mut_ptr(),
                &mut res_acc_size,
                res_tx.as_mut_ptr(),
                &mut res_tx_size,
                verbosity,
            );
            assert!(res, "check preallocated size for output data {}", res_tx_size);
        }
    } else {
        let tick = if let TransactionDescr::TickTock(descr) =
            transaction.unwrap().read_description().unwrap()
        {
            descr.tt.is_tick()
        } else {
            unreachable!();
        };
        unsafe {
            type RunBoc<'a> = libloading::Symbol<
                'a,
                unsafe extern "C" fn(
                    *const u8,
                    i32,
                    *const u8,
                    i32,
                    u64,
                    i32,
                    u64,
                    bool,
                    *mut u8,
                    &mut i32,
                    *mut u8,
                    &mut i32,
                    i32,
                ) -> bool,
            >;
            let run_boc: RunBoc = lib.get(b"replay_ticktock_transaction_ext").unwrap();

            let res: bool = run_boc(
                acc_data.as_ptr(),
                acc_data.len() as i32,
                cfg_data.as_ptr(),
                cfg_data.len() as i32,
                params.last_tr_lt,
                params.block_unixtime as i32,
                params.block_lt,
                tick,
                res_acc.as_mut_ptr(),
                &mut res_acc_size,
                res_tx.as_mut_ptr(),
                &mut res_tx_size,
                verbosity,
            );
            assert!(res, "check preallocated size for output data");
        }
    }

    if res_tx_size != 0 {
        let acc_file_cells = load_cells(&res_acc[0..res_acc_size as usize]);
        let tx_file_cells = load_cells(&res_tx[0..res_tx_size as usize]);

        let acc_file = Account::construct_from_cell(acc_file_cells.clone()).unwrap();
        let tx_file = Transaction::construct_from_cell(tx_file_cells.clone()).unwrap();

        let mut transaction = transaction.cloned().unwrap_or_else(|| {
            panic!("transaction is None but real is {:#?}", tx_file.read_description().unwrap())
        });
        transaction.write_state_update(&tx_file.read_state_update().unwrap()).unwrap();

        /*if acc_after != acc_file {
            println!("Iteration {}", _it);
        }*/
        let mut acc_after = acc_after.clone();
        if block_version < 11 {
            acc_after.del_storage_stat();
        }
        let balance_after = acc_after.balance().cloned().unwrap_or_default();
        let balance_file = acc_file.balance().cloned().unwrap_or_default();
        balance_after
            .other
            .scan_diff(&balance_file.other, |key: u32, value1, value2| {
                assert_eq!(value1, value2, "for key {}", key);
                Ok(true)
            })
            .unwrap();
        acc_after
            .libraries()
            .scan_diff(&acc_file.libraries(), |key: UInt256, lib1, lib2| {
                assert_eq!(lib1, lib2, "for key {:x}", key);
                Ok(true)
            })
            .unwrap();
        assert_eq!(acc_after, acc_file);

        assert_eq!(transaction.read_description().unwrap(), tx_file.read_description().unwrap());
        transaction
            .out_msgs
            .scan_diff(&tx_file.out_msgs, |key: ton_block::UInt15, msg1, msg2| {
                assert_eq!(msg1, msg2, "for key {}", key.0);
                Ok(true)
            })
            .unwrap();
        assert_eq!(transaction, tx_file);

        assert_eq!(acc_after.serialize().unwrap(), acc_file_cells);
        assert_eq!(transaction.serialize().unwrap(), tx_file_cells);
    } else {
        assert!(transaction.is_none());
    }
}
