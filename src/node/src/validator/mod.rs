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
pub mod accept_block;
pub mod candidate_db;
pub mod collator;
pub mod consensus;
pub mod consensus_overlay;
mod fabric;
mod log_parser;
mod mutex_wrapper;
pub mod out_msg_queue;
mod out_msg_queue_cleaner;
pub mod out_msg_queue_manager;
#[cfg(feature = "telemetry")]
pub mod telemetry;
pub mod validate_query;
pub mod validator_group;
pub mod validator_manager;
pub mod validator_session_listener;
pub mod validator_utils;

use crate::shard_state::ShardStateStuff;
use std::sync::Arc;
use ton_block::{
    error, BlkMasterInfo, BlockIdExt, ConfigParams, CurrencyCollection, ExtBlkRef, KeyExtBlkRef,
    Libraries, McStateExtra, Result, UInt256,
};

/// Minimum global version that allows equal `gen_utime` between consecutive blocks.
///
/// C++ parity: `allow_same_timestamp_ = global_version_ >= 13`.
/// Applies to all consensus types (not simplex-specific despite the name).
/// Under `xp25` feature this constant is unused — `allow_same_timestamp` is always true.
#[cfg(not(feature = "xp25"))]
pub(super) const SIMPLEX_ALLOW_SAME_TIMESTAMP_FROM_GLOBAL_VERSION: u32 = 13;

#[derive(Clone, Default, Debug)]
pub struct BlockCandidate {
    pub block_id: BlockIdExt,
    pub data: Vec<u8>,
    pub collated_data: Vec<u8>,
    pub collated_file_hash: UInt256,
    pub created_by: UInt256,
}

#[derive(Clone, Default, serde::Deserialize)]
pub struct CollatorSettings {
    pub want_split: Option<bool>,
    pub want_merge: Option<bool>,
    pub is_fake: bool,
    #[cfg(test)]
    pub is_bundle: bool,
    // produce blocks identical to cpp-node - mostly for tests
    pub lt_compatible: bool,
    // true when running under simplex consensus (passed from ValidatorGroup)
    pub is_simplex: bool,
    // when set, collator must not choose gen_utime_ms earlier than this value
    pub min_gen_utime_ms: Option<u64>,
}

impl CollatorSettings {
    #[allow(dead_code)]
    pub fn fake() -> Self {
        Self { is_fake: true, ..Self::default() }
    }
}

pub struct McData {
    mc_state_extra: McStateExtra,
    prev_key_block_seqno: u32,
    prev_key_block: Option<BlockIdExt>,
    state: Arc<ShardStateStuff>, // TODO put here what you need from masterchain state and block and init in `unpack_last_mc_state`
}

impl McData {
    pub fn new(mc_state: Arc<ShardStateStuff>) -> Result<Self> {
        let mc_state_extra = mc_state
            .state()?
            .read_custom()?
            .ok_or_else(|| error!("Can't read custom field from mc state"))?;

        // prev key block
        let (prev_key_block_seqno, prev_key_block) = if mc_state_extra.after_key_block {
            (mc_state.block_id().seq_no(), Some(mc_state.block_id().clone()))
        } else if let Some(block_ref) = mc_state_extra.last_key_block.clone() {
            (block_ref.seq_no, Some(block_ref.master_block_id().1))
        } else {
            (0, None)
        };
        Ok(Self { mc_state_extra, prev_key_block, prev_key_block_seqno, state: mc_state })
    }

    pub fn config(&self) -> &ConfigParams {
        self.mc_state_extra.config()
    }
    pub fn mc_state_extra(&self) -> &McStateExtra {
        &self.mc_state_extra
    }
    pub fn prev_key_block_seqno(&self) -> u32 {
        self.prev_key_block_seqno
    }
    pub fn prev_key_block(&self) -> Option<&BlockIdExt> {
        self.prev_key_block.as_ref()
    }
    pub fn last_mc_block_id(&self) -> KeyExtBlkRef {
        KeyExtBlkRef {
            key: self.mc_state_extra.after_key_block,
            blk_ref: ExtBlkRef {
                end_lt: self.state.state().map_or(0, |state| state.gen_lt()),
                seq_no: self.state.block_id().seq_no,
                root_hash: self.state.block_id().root_hash.clone(),
                file_hash: self.state.block_id().file_hash.clone(),
            },
        }
    }
    pub fn state(&self) -> &Arc<ShardStateStuff> {
        &self.state
    }
    pub fn vert_seq_no(&self) -> Result<u32> {
        Ok(self.state().state()?.vert_seq_no())
    }
    pub fn get_lt_align(&self) -> u64 {
        1000000
    }
    pub fn global_balance(&self) -> &CurrencyCollection {
        &self.mc_state_extra.global_balance
    }
    pub fn libraries(&self) -> Result<&Libraries> {
        Ok(self.state.state()?.libraries())
    }
    pub fn master_ref(&self) -> Result<BlkMasterInfo> {
        let end_lt = self.state.state()?.gen_lt();
        let master = ExtBlkRef {
            end_lt,
            seq_no: self.state.state()?.seq_no(),
            root_hash: self.state.block_id().root_hash().clone(),
            file_hash: self.state.block_id().file_hash().clone(),
        };
        Ok(BlkMasterInfo { master })
    }
}

/* UNUSED
impl CollatorSettings {
    pub fn want_merge() -> Self {
        let mut settings = Self::default();
        settings.want_merge = Some(true);
        settings
    }
    pub fn want_split() -> Self {
        let mut settings = Self::default();
        settings.want_split = Some(true);
        settings
    }
}
*/
