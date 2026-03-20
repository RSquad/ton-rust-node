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
use crate::{
    define_HashmapE, fail, BuilderData, Cell, Deserializable, HashmapSubtree, Result, Serializable,
    SliceData, UInt256,
};

#[cfg(test)]
#[path = "tests/test_miscellaneous.rs"]
mod tests;

/*
// key is [ shard:uint64 mc_seqno:uint32 ]
_ (HashmapE 96 ProcessedUpto) = ProcessedInfo;
*/
define_HashmapE!(ProcessedInfo, 96, ProcessedUpto);

/// Struct ProcessedInfoKey describe key for ProcessedInfo
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ProcessedInfoKey {
    pub shard: u64,
    pub mc_seqno: u32,
}

impl ProcessedInfoKey {
    // New instance ProcessedInfoKey structure
    pub fn with_params(shard: u64, mc_seqno: u32) -> Self {
        ProcessedInfoKey { shard, mc_seqno }
    }
    pub fn seq_no(&self) -> u32 {
        self.mc_seqno
    }
}

impl Serializable for ProcessedInfoKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.shard.write_to(cell)?;
        self.mc_seqno.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for ProcessedInfoKey {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.shard.read_from(cell)?;
        self.mc_seqno.read_from(cell)?;
        Ok(())
    }
}

///
/// Struct ProcessedUpto
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ProcessedUpto {
    pub last_msg_lt: u64,
    pub last_msg_hash: UInt256,
    pub original_shard: Option<u64>,
}

impl ProcessedUpto {
    // New instance ProcessedUpto structure
    pub fn with_params(
        last_msg_lt: u64,
        last_msg_hash: UInt256,
        original_shard: Option<u64>,
    ) -> Self {
        Self { last_msg_lt, last_msg_hash, original_shard }
    }
}

impl Serializable for ProcessedUpto {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.last_msg_lt.write_to(cell)?;
        self.last_msg_hash.write_to(cell)?;
        if let Some(s) = &self.original_shard {
            s.write_to(cell)?;
        }
        Ok(())
    }
}

impl Deserializable for ProcessedUpto {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.last_msg_lt.read_from(cell)?;
        self.last_msg_hash.read_from(cell)?;
        self.original_shard = match cell.remaining_bits() {
            0 => None,
            64 => Some(cell.get_next_u64()?),
            1 => None,
            65 => Deserializable::construct_maybe_from(cell)?,
            _ => fail!("Invalid cell size"),
        };
        Ok(())
    }
}

// IhrPendingInfo structure
define_HashmapE!(IhrPendingInfo, 320, IhrPendingSince);

impl IhrPendingInfo {
    pub fn split_inplace(&mut self, split_key: &SliceData) -> Result<()> {
        self.0 = self.0.subtree_with_prefix(split_key, &mut 0)?;
        Ok(())
    }
}

///
/// IhrPendingSince structure
///
/// ihr_pending$_
///     import_lt:uint64
/// = IhrPendingSince;
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct IhrPendingSince {
    import_lt: u64,
}

impl IhrPendingSince {
    /// New default instance IhrPendingSince structure
    pub fn new() -> Self {
        Self::default()
    }

    // New instance IhrPendingSince structure
    pub fn with_import_lt(import_lt: u64) -> Self {
        IhrPendingSince { import_lt }
    }

    pub fn import_lt(&self) -> u64 {
        self.import_lt
    }
}

impl Serializable for IhrPendingSince {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.import_lt.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for IhrPendingSince {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.import_lt.read_from(cell)?;
        Ok(())
    }
}
