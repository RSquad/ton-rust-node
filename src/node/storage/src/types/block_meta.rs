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
use crate::{block_handle_db, traits::Serializable};
#[cfg(test)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicU64, Ordering};
use ton_block::{Block, Result};

#[derive(Debug, Default)]
pub struct BlockMeta {
    flags: AtomicU64,
    pub gen_utime: u32,
    pub end_lt: u64,
    pub params: u32,
    #[cfg(test)]
    pub test_counter: AtomicU32,
}

impl BlockMeta {
    pub fn from_block(block: &Block) -> Result<Self> {
        let info = block.read_info()?;
        let flags = if info.key_block() { block_handle_db::FLAG_KEY_BLOCK } else { 0 };
        Ok(Self::with_data(flags, info.gen_utime(), info.end_lt(), 0, 0))
    }

    pub fn with_data(
        flags: u32,
        gen_utime: u32,
        end_lt: u64,
        masterchain_ref_seq_no: u32,
        params: u32,
    ) -> Self {
        Self {
            flags: AtomicU64::new(((flags as u64) << 32) | masterchain_ref_seq_no as u64),
            gen_utime,
            end_lt,
            params,
            #[cfg(test)]
            test_counter: AtomicU32::new(0),
        }
    }

    pub fn flags(&self) -> u32 {
        (self.flags.load(Ordering::Relaxed) >> 32) as u32
    }

    pub fn set_flags(&self, flags: u32) -> u32 {
        (self.flags.fetch_or((flags as u64) << 32, Ordering::Relaxed) >> 32) as u32
    }

    pub fn reset(&self, flags: u32, reset_mc_ref_seq_no: bool) {
        if reset_mc_ref_seq_no {
            self.flags.fetch_and((!flags as u64) << 32, Ordering::Relaxed);
        } else {
            self.flags.fetch_and(!((flags as u64) << 32), Ordering::Relaxed);
        }
    }

    pub fn masterchain_ref_seq_no(&self) -> u32 {
        self.flags.load(Ordering::Relaxed) as u32
    }

    pub fn set_masterchain_ref_seq_no(&self, masterchain_ref_seq_no: u32) -> u32 {
        self.flags.fetch_or(masterchain_ref_seq_no as u64, Ordering::Relaxed) as u32
    }
}

impl Serializable for BlockMeta {
    #[cfg(not(test))]
    const SIZE: usize = 20;
    #[cfg(test)]
    const SIZE: usize = 24;
    type Bytes = [u8; Self::SIZE];

    fn serialize(&self) -> Self::Bytes {
        const FLAG_MASK: u64 = 0x0FFF_FFFF_FFFF_FFFF;
        let mut ret = [0u8; Self::SIZE];
        let flags = self.flags.load(Ordering::Relaxed) & FLAG_MASK;
        ret[..8].copy_from_slice(&flags.serialize());
        ret[8..12].copy_from_slice(&self.gen_utime.serialize());
        ret[12..20].copy_from_slice(&self.end_lt.serialize());
        #[cfg(test)]
        ret[20..].copy_from_slice(&self.test_counter.load(Ordering::SeqCst).serialize());
        ret
    }

    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let flags = u64::deserialize(data)?;
        let gen_utime = u32::deserialize(&data[8..])?;
        let gen_lt = u64::deserialize(&data[12..])?;
        let masterchain_ref_seq_no = flags as u32;
        let flags = (flags >> 32) as u32;
        let bm = Self::with_data(flags, gen_utime, gen_lt, masterchain_ref_seq_no, 0);
        #[cfg(test)]
        {
            let test_counter = u32::deserialize(&data[20..]).unwrap_or_default();
            bm.test_counter.store(test_counter, Ordering::Relaxed);
        }
        Ok(bm)
    }
}
