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
use std::any::type_name;
use ton_block::{fail, BlockIdExt, Result, ShardIdent, UInt256};

pub trait Serializable {
    const SIZE: usize;
    type Bytes: AsRef<[u8]>;
    fn deserialize(data: &[u8]) -> Result<Self>
    where
        Self: Sized,
    {
        if data.len() < Self::SIZE {
            fail!(
                "Not enough data to deserialize {}, need {}, have {}",
                type_name::<Self>(),
                Self::SIZE,
                data.len()
            )
        }
        Self::deserialize_checked(data)
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self>
    where
        Self: Sized;
    fn serialize(&self) -> Self::Bytes;
}

impl Serializable for BlockIdExt {
    const SIZE: usize = ShardIdent::SIZE + 68;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        let mut ret = [0u8; Self::SIZE];
        ret[..ShardIdent::SIZE].copy_from_slice(&self.shard_id.serialize());
        ret[ShardIdent::SIZE..ShardIdent::SIZE + 4].copy_from_slice(&self.seq_no.serialize());
        ret[ShardIdent::SIZE + 4..ShardIdent::SIZE + 36].copy_from_slice(self.root_hash.as_ref());
        ret[ShardIdent::SIZE + 36..].copy_from_slice(self.file_hash.as_ref());
        ret
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let shard_id = ShardIdent::deserialize_checked(data)?;
        let seq_no = u32::deserialize_checked(&data[ShardIdent::SIZE..])?;
        let root_hash = UInt256::from(&data[ShardIdent::SIZE + 4..ShardIdent::SIZE + 36]);
        let file_hash = UInt256::from(&data[ShardIdent::SIZE + 36..]);
        Ok(Self::with_params(shard_id, seq_no, root_hash, file_hash))
    }
}

impl Serializable for bool {
    const SIZE: usize = 1;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        [*self as u8]
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        Ok(data[0] != 0)
    }
}

impl Serializable for ShardIdent {
    const SIZE: usize = 12;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        let mut ret = [0u8; Self::SIZE];
        ret[..4].copy_from_slice(&self.workchain_id().to_le_bytes());
        ret[4..].copy_from_slice(&self.shard_prefix_with_tag().serialize());
        ret
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let wc = u32::deserialize_checked(&data[..4])? as i32;
        let sh = u64::deserialize_checked(&data[4..])?;
        Self::with_tagged_prefix(wc, sh)
    }
}

impl Serializable for u8 {
    const SIZE: usize = 1;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        [*self]
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        Ok(data[0])
    }
}

impl Serializable for u16 {
    const SIZE: usize = 2;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        self.to_le_bytes()
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        Ok(Self::from_le_bytes(data[..2].try_into()?))
    }
}

impl Serializable for u32 {
    const SIZE: usize = 4;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        self.to_le_bytes()
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        Ok(Self::from_le_bytes(data[..4].try_into()?))
    }
}

impl Serializable for u64 {
    const SIZE: usize = 8;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        self.to_le_bytes()
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        Ok(Self::from_le_bytes(data[..8].try_into()?))
    }
}
