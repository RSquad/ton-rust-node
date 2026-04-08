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
use crate::archives::{ARCHIVE_SIZE, KEY_ARCHIVE_SIZE};
use std::path::{Path, PathBuf};
use ton_block::{fail, Result, ShardIdent};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum PackageType {
    Blocks,
    KeyBlocks, //Temp
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PackageId {
    id: u32,
    package_type: PackageType,
}

impl PackageId {
    pub const fn with_values(id: u32, package_type: PackageType) -> Self {
        Self { id, package_type }
    }

    pub const fn for_block(mc_seq_no: u32) -> Self {
        Self::with_values(mc_seq_no, PackageType::Blocks)
    }

    pub const fn for_key_block(mc_seq_no: u32) -> Self {
        Self::with_values(mc_seq_no, PackageType::KeyBlocks)
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn package_type(&self) -> PackageType {
        self.package_type
    }

    pub fn path(&self) -> String {
        match self.package_type {
            //PackageType::Temp =>
            //    "files/packages/".into(),
            PackageType::Blocks => format!("archive/packages/arch{:04}/", self.id / ARCHIVE_SIZE),
            PackageType::KeyBlocks => {
                format!("archive/packages/key{:03}/", self.id / KEY_ARCHIVE_SIZE)
            }
        }
    }

    pub fn full_path(&self, db_root: impl AsRef<Path>, shard: &ShardIdent) -> Result<PathBuf> {
        let name = match self.package_type {
            //PackageType::Temp => format!("temp.archive.{}.pack", self.id),
            PackageType::KeyBlocks => {
                if shard.is_masterchain() {
                    format!("key.archive.{:06}.pack", self.id)
                } else {
                    fail!("Cannot get key archive path for shard {shard}")
                }
            }
            PackageType::Blocks => {
                if shard.is_masterchain() {
                    format!("archive.{:05}.pack", self.id)
                } else {
                    format!("archive.{:05}.{shard}.pack", self.id)
                }
            }
        };
        Ok(db_root.as_ref().join(self.path()).join(&name))
    }
}

#[cfg(test)]
#[path = "../tests/test_package_id.rs"]
mod tests;
