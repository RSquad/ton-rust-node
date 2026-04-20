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
pub mod archive_import;
pub mod block;
pub mod block_proof;
pub mod boot;
pub mod collator_test_bundle;
pub mod config;
pub mod engine;
pub mod engine_operations;
pub mod engine_traits;
pub mod error;
pub mod ext_messages;
pub mod full_node;
pub mod internal_db;
pub mod macros;
pub mod network;
pub mod rng;
pub mod rpc_server;
pub mod shard_state;
pub mod shard_states_keeper;
pub mod sync;
pub mod types;
pub mod validating_utils;
pub mod validator;

#[cfg(not(feature = "xp25"))]
mod shard_blocks;
#[cfg(feature = "xp25")]
mod shard_blocks_intershard;
#[cfg(feature = "xp25")]
mod shard_blocks {
    pub use crate::shard_blocks_intershard::*;
}

include!("../../common/src/info.rs");

#[cfg(test)]
#[path = "tests/test_helper.rs"]
pub mod test_helper;
