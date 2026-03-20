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
use crate::{db::U32Key, db_impl_cbor, traits::Serializable};
use ton_block::{Result, ShardIdent};

#[derive(Clone, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PackageEntryInfo {
    pub seqno: u32,
    pub shard: ShardIdent,
}

impl Serializable for PackageEntryInfo {
    const SIZE: usize = ShardIdent::SIZE + 4;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        let mut ret = [0u8; Self::SIZE];
        ret[..4].copy_from_slice(&self.seqno.serialize());
        ret[4..].copy_from_slice(&self.shard.serialize());
        ret
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let seqno = u32::deserialize_checked(data)?;
        let shard = ShardIdent::deserialize_checked(&data[4..])?;
        let ret = Self { seqno, shard };
        Ok(ret)
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PackageEntryMeta {
    entry_size: u64,
    info: Option<[u8; PackageEntryInfo::SIZE]>,
    version: u32,
}

impl PackageEntryMeta {
    pub fn with_data(entry_size: u64, version: u32, info: Option<&PackageEntryInfo>) -> Self {
        Self { entry_size, info: info.map(|info| info.serialize()), version }
    }

    pub const fn entry_size(&self) -> u64 {
        self.entry_size
    }

    pub fn get_info(&self) -> Result<Option<PackageEntryInfo>> {
        let Some(info) = self.info.as_ref() else { return Ok(None) };
        Ok(Some(PackageEntryInfo::deserialize(info)?))
    }

    pub const fn version(&self) -> u32 {
        self.version
    }
}

db_impl_cbor!(PackageEntryMetaDb, U32Key, PackageEntryMeta);
