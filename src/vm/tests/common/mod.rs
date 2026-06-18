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
#![allow(dead_code)]

pub mod test_framework;
use std::sync::LazyLock;
pub use test_framework::*;
use ton_assembler::CompileError;
use ton_block::{
    Account, BuilderData, Cell, ConfigParamEnum, CurrencyCollection, Deserializable, ExceptionCode,
    MerkleProof, MsgAddressInt, Serializable, ShardAccount, ShardStateUnsplit, SliceData,
    StateInit, DICT_HASH_MIN_CELLS, SUPPORTED_VERSION,
};
use ton_vm::stack::StackItem;

pub static MC_STATE_ROOT: LazyLock<Cell> = LazyLock::new(|| {
    let mc_state_name = "../block/src/tests/data/free-ton-mc-state-61884";
    // this code to modify config params real state
    let mut mc_state = ShardStateUnsplit::construct_from_file(mc_state_name).unwrap();
    let mut extra = mc_state.read_custom().unwrap().unwrap();
    extra.after_key_block = true;
    mc_state.write_custom(Some(&extra)).unwrap();
    mc_state
        .update_config_param(19, |param_opt| {
            *param_opt = Some(ConfigParamEnum::ConfigParam19(795));
        })
        .unwrap();
    mc_state
        .update_config_param(8, |param_opt| {
            if let Some(ConfigParamEnum::ConfigParam8(param)) = param_opt.as_mut() {
                param.global_version.version = SUPPORTED_VERSION;
            }
        })
        .unwrap();
    mc_state
        .update_config_param(43, |param_opt| {
            *param_opt = Some(ConfigParamEnum::ConfigParam43(Default::default()));
        })
        .unwrap();
    mc_state.update_config_smc().unwrap();
    // mc_state.write_to_file(mc_state_name).unwrap();
    // let mc_state = ton_block_json::debug_state(mc_state).unwrap();
    // std::fs::write("../target/mc_state.json", mc_state).unwrap();

    mc_state.serialize().unwrap()
});

pub static MC_STATE_PROOF: LazyLock<Cell> = LazyLock::new(|| {
    let mc_state_proof = MerkleProof {
        hash: MC_STATE_ROOT.repr_hash().clone(),
        depth: MC_STATE_ROOT.repr_depth(),
        proof: MC_STATE_ROOT.clone(),
    };
    mc_state_proof.serialize().unwrap()
});

pub static ACCOUNT_ROOT: LazyLock<Cell> = LazyLock::new(|| SHARD_ACCOUNT.serialize().unwrap());
pub static SHARD_ACCOUNT: LazyLock<ShardAccount> = LazyLock::new(|| {
    let addr = MsgAddressInt::standard(0, [0x55; 32]);
    let balance = CurrencyCollection::with_coins(1234567);
    let state_init = StateInit::default();
    let mut account =
        Account::active(addr.clone(), balance, 0, 0, state_init, DICT_HASH_MIN_CELLS).unwrap();
    account.set_due_payment(Some(738.into()));
    ShardAccount::with_params(&account, Default::default(), 1357).unwrap()
});

pub mod create {
    use super::*;

    pub fn cell<T: AsRef<[u8]>>(data: T) -> StackItem {
        let data = data.as_ref().to_vec();
        StackItem::Cell(BuilderData::with_bitstring(data).unwrap().into_cell().unwrap())
    }

    pub fn builder<T: AsRef<[u8]>>(data: T) -> StackItem {
        let builder = BuilderData::with_bitstring(data.as_ref().to_vec()).unwrap();
        StackItem::builder(builder)
    }

    pub fn slice<T: AsRef<[u8]>>(data: T) -> StackItem {
        let data = data.as_ref().to_vec();
        let slice = SliceData::new(data);
        StackItem::Slice(slice)
    }

    pub fn tuple<T: AsRef<[StackItem]>>(data: &T) -> StackItem {
        let data = data.as_ref().to_vec();
        StackItem::tuple(data)
    }
}

pub fn test_single_argument_fail(cmd: &str, argument: isize) {
    let code = format!("{} {}", cmd, argument);
    test_case(code).expect_compilation_failure(CompileError::out_of_range(1, 1, cmd, "arg 0"));
}

pub fn expect_exception(code: &str, exc_code: ExceptionCode) {
    test_case(code).expect_failure_extended(exc_code, Some(code));
}

// TODO: move to common
pub fn init_env_logger() {
    let _ = env_logger::builder()
        .format_file(true)
        .format_line_number(true)
        .format_target(false)
        .write_style(env_logger::WriteStyle::Auto)
        .try_init();
}
