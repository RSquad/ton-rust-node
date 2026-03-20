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
use crate::{archives::package_entry_id::PackageEntryId, db::DbKey, db_impl_cbor};
use std::{
    borrow::Borrow,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};
use ton_block::BlockIdExt;

#[derive(Debug)]
pub struct PackageOffsetKey {
    entry_id_hash: [u8; 8],
}

impl PackageOffsetKey {
    pub fn from_entry_type<B: Borrow<BlockIdExt> + Hash>(entry_id: &PackageEntryId<B>) -> Self {
        let mut hasher = DefaultHasher::new();
        entry_id.hash(&mut hasher);
        Self { entry_id_hash: hasher.finish().to_le_bytes() }
    }
}

impl<B: Borrow<BlockIdExt> + Hash> From<&PackageEntryId<B>> for PackageOffsetKey {
    fn from(entry_id: &PackageEntryId<B>) -> Self {
        Self::from_entry_type(entry_id)
    }
}

impl DbKey for PackageOffsetKey {
    fn key_name(&self) -> &'static str {
        "PackageOffsetKey"
    }
    fn key(&self) -> &[u8] {
        &self.entry_id_hash
    }
}

db_impl_cbor!(PackageOffsetsDb, PackageOffsetKey, u64);
