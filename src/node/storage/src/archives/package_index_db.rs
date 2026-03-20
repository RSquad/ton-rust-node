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
use crate::{archives::file_maps::BlockRanges, db::U32Key, db_impl_cbor};
use std::{collections::HashMap, convert::TryInto};
use ton_block::{Result, ShardIdent};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PackageIndexEntry {
    deleted: bool,
    finalized: bool,
    blocks_ranges: HashMap<(i32, u64), BlockRanges>,
}

impl PackageIndexEntry {
    pub fn new() -> Self {
        Self::with_data(false, false, &lockfree::map::Map::new())
    }

    pub fn with_data(
        deleted: bool,
        finalized: bool,
        blocks_ranges: &lockfree::map::Map<ShardIdent, BlockRanges>,
    ) -> Self {
        let mut fb_map = HashMap::new();
        for guard in blocks_ranges {
            fb_map.insert(
                (guard.key().workchain_id(), guard.key().shard_prefix_with_tag()),
                guard.val().clone(),
            );
        }
        Self { deleted, finalized, blocks_ranges: fb_map }
    }

    pub const fn deleted(&self) -> bool {
        self.deleted
    }

    pub const fn finalized(&self) -> bool {
        self.finalized
    }

    pub fn blocks_ranges(self) -> Result<lockfree::map::Map<ShardIdent, BlockRanges>> {
        let map = lockfree::map::Map::new();
        for ((workchain_id, shard_prefix), info) in self.blocks_ranges {
            let shard_ident = ShardIdent::with_tagged_prefix(workchain_id, shard_prefix)?;
            map.insert(shard_ident, info);
        }
        Ok(map)
    }
}

db_impl_cbor!(PackageIndexDb, U32Key, PackageIndexEntry);

impl PackageIndexDb {
    pub fn for_each_deserialized(
        &self,
        mut predicate: impl FnMut(u32, PackageIndexEntry) -> Result<bool>,
    ) -> Result<bool> {
        self.for_each(&mut |key_data, data| {
            let key = u32::from_le_bytes(key_data.try_into()?);
            let value = serde_cbor::from_slice(data)?;
            predicate(key, value)
        })
    }
}
