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
use std::fmt::Display;
use ton_block::{BlockIdExt, UInt256};

mod block_id;
mod block_meta;
mod db_slice;
mod shard_ident_key;
mod status_key;
mod storage_cell;

use crate::db::DbKey;
pub use block_meta::*;
pub use db_slice::*;
pub use shard_ident_key::*;
pub use status_key::*;
pub use storage_cell::*;

/*
/// Usually >= 1; 0 used to indicate the initial state, i.e. "zerostate"
pub type BlockSeqNo = i32;
pub type BlockVertSeqNo = u32;
pub type WorkchainId = i32;
pub type ShardId = i64;
*/

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PersistentStatePartId {
    WholeState(BlockIdExt),
    Head(BlockIdExt),
    Part(BlockIdExt, u64),
}

impl PersistentStatePartId {
    pub fn block_id(&self) -> &BlockIdExt {
        match self {
            PersistentStatePartId::WholeState(block_id) => block_id,
            PersistentStatePartId::Head(block_id) => block_id,
            PersistentStatePartId::Part(block_id, _) => block_id,
        }
    }
    pub fn part_prefix(&self) -> u64 {
        match self {
            PersistentStatePartId::WholeState(_) => 0,
            PersistentStatePartId::Head(id) => id.shard().shard_prefix_with_tag(),
            PersistentStatePartId::Part(_, prefix) => *prefix,
        }
    }
    pub fn is_head(&self) -> bool {
        matches!(self, PersistentStatePartId::Head(_))
    }
    pub fn is_whole_state(&self) -> bool {
        matches!(self, PersistentStatePartId::WholeState(_))
    }
}

impl Display for PersistentStatePartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistentStatePartId::WholeState(block_id) => write!(f, "WholeState {}", block_id),
            PersistentStatePartId::Head(block_id) => write!(f, "Head {}", block_id),
            PersistentStatePartId::Part(block_id, prefix) => {
                write!(f, "Part {:016x} {}", prefix, block_id)
            }
        }
    }
}

pub struct PersistentStatePartKey(Vec<u8>);
impl From<&PersistentStatePartId> for PersistentStatePartKey {
    fn from(id: &PersistentStatePartId) -> Self {
        match id {
            PersistentStatePartId::WholeState(block_id) => {
                Self(block_id.root_hash().as_slice().to_vec())
            }
            PersistentStatePartId::Head(block_id) => {
                let mut key = vec![0u8; 32 + 8];
                key[..32].copy_from_slice(block_id.root_hash().as_slice());
                key[32..].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
                Self(key)
            }
            PersistentStatePartId::Part(block_id, prefix) => {
                let mut key = vec![0u8; 32 + 8];
                key[..32].copy_from_slice(block_id.root_hash().as_slice());
                key[32..].copy_from_slice(&prefix.to_be_bytes());
                Self(key)
            }
        }
    }
}

impl From<PersistentStatePartId> for PersistentStatePartKey {
    fn from(id: PersistentStatePartId) -> Self {
        (&id).into()
    }
}

impl DbKey for PersistentStatePartKey {
    fn key_name(&self) -> &'static str {
        if self.0.len() == 32 {
            "PersistentStatePartKey(WholeState)"
        } else if self.0[32..] == [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff] {
            "PersistentStatePartKey(Head)"
        } else {
            "PersistentStatePartKey(Part)"
        }
    }

    fn as_string(&self) -> String {
        if self.0.len() == 32 {
            format!("PersistentStatePartKey(WholeState {})", hex::encode(&self.0))
        } else if self.0[32..] == [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff] {
            format!("PersistentStatePartKey(Head {:x})", UInt256::from_slice(&self.0[..32]))
        } else {
            format!(
                "PersistentStatePartKey(Part {:016x} {:x})",
                u64::from_be_bytes(self.0[32..].try_into().unwrap()),
                UInt256::from_slice(&self.0[..32])
            )
        }
    }

    fn key(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
