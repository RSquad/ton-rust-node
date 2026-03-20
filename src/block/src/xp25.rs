/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    define_HashmapE, fail, BinTree, BinTreeType, BlockError, BlockIdExt, BuilderData, Cell,
    Deserializable, IBitstring, Result, Serializable, ShardIdent, SliceData, UInt256,
};
use std::{any::type_name, collections::HashMap};

const SHARD_BLOCK_REF_PFX: u8 = 0x01;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShardBlockRef {
    pub seq_no: u32,
    pub root_hash: UInt256,
    pub file_hash: UInt256,
    pub end_lt: u64,
}

impl Deserializable for ShardBlockRef {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != SHARD_BLOCK_REF_PFX {
            fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: type_name::<Self>().to_string()
            })
        }
        Ok(Self {
            seq_no: slice.get_next_u32()?,
            root_hash: UInt256::construct_from(slice)?,
            file_hash: UInt256::construct_from(slice)?,
            end_lt: slice.get_next_u64()?,
        })
    }
}

impl Serializable for ShardBlockRef {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        builder.append_u8(SHARD_BLOCK_REF_PFX)?;
        self.seq_no.write_to(builder)?;
        self.root_hash.write_to(builder)?;
        self.file_hash.write_to(builder)?;
        self.end_lt.write_to(builder)?;
        Ok(())
    }
}

impl ShardBlockRef {
    pub fn with_params(block_id: &BlockIdExt, end_lt: u64) -> Self {
        Self {
            seq_no: block_id.seq_no,
            root_hash: block_id.root_hash.clone(),
            file_hash: block_id.file_hash.clone(),
            end_lt,
        }
    }

    pub fn into_block_id(self, shard_id: ShardIdent) -> Result<BlockIdExt> {
        Ok(BlockIdExt {
            shard_id,
            seq_no: self.seq_no,
            root_hash: self.root_hash,
            file_hash: self.file_hash,
        })
    }
}

define_HashmapE! {RefShardBlocks, 32, BinTree<ShardBlockRef>}

impl RefShardBlocks {
    pub fn with_ids<'a>(ids: impl IntoIterator<Item = &'a (BlockIdExt, u64)>) -> Result<Self> {
        // Naive implementation.
        //TODO optimise me!

        let mut ref_shard_blocks = HashMap::new(); // wc -> shard -> id
        for (id, end_lt) in ids {
            let shards = loop {
                if let Some(wc) = ref_shard_blocks.get_mut(&id.shard().workchain_id()) {
                    break wc;
                }
                ref_shard_blocks.insert(id.shard().workchain_id(), HashMap::new());
            };
            shards.insert(
                id.shard().shard_prefix_with_tag(),
                ShardBlockRef::with_params(id, *end_lt),
            );
        }
        Self::with_ids_map(ref_shard_blocks)
    }

    pub fn with_ids_map<'a>(ids: HashMap<i32, HashMap<u64, ShardBlockRef>>) -> Result<Self> {
        // Naive implementation.
        //TODO optimise me!

        let mut result = Self::default();
        for (wc, mut shards) in ids {
            let key = ShardIdent::full(wc);
            let mut bintree;
            if let Some(val) = shards.get(&key.shard_prefix_with_tag()) {
                bintree = BinTree::with_item(val)?;
            } else {
                bintree = BinTree::with_item(&ShardBlockRef::default())?;
                let mut unfinished_keys = vec![key];
                while let Some(key) = unfinished_keys.pop() {
                    bintree.split(key.shard_key(false), |_| {
                        let (left, right) = key.split()?;
                        let left_val =
                            if let Some(val) = shards.remove(&left.shard_prefix_with_tag()) {
                                val
                            } else {
                                unfinished_keys.push(left);
                                ShardBlockRef::default()
                            };
                        let right_val =
                            if let Some(val) = shards.remove(&right.shard_prefix_with_tag()) {
                                val
                            } else {
                                unfinished_keys.push(right);
                                ShardBlockRef::default()
                            };
                        Ok((left_val, right_val))
                    })?;
                }
                if !shards.is_empty() {
                    fail!("wrong ids (shards is not empty after bintree filling)")
                }
            }
            result.set(&wc, &bintree)?;
        }

        Ok(result)
    }

    pub fn iterate_shard_block_refs<F>(&self, mut func: F) -> Result<bool>
    where
        F: FnMut(BlockIdExt, u64) -> Result<bool>,
    {
        self.iterate_with_keys(|wc_id: i32, shards| {
            shards.iterate(|prefix, info| {
                let shard_ident = ShardIdent::with_prefix_slice(wc_id, prefix)?;
                let end_lt = info.end_lt;
                let block_id = info.into_block_id(shard_ident)?;
                func(block_id, end_lt)
            })
        })
    }

    pub fn ref_shard_block(&self, shard_ident: &ShardIdent) -> Result<Option<ShardBlockRef>> {
        if let Some(shards) = self.get(&shard_ident.workchain_id())? {
            if let Some(sbr) = shards.get(shard_ident.shard_key(false))? {
                return Ok(Some(sbr));
            }
        }
        Ok(None)
    }
}

pub const WC_EXTRA_PFX: u16 = 0x11aa;

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct WcExtra {
    pub ref_shard_blocks: RefShardBlocks,
}

impl Serializable for WcExtra {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        builder.append_u16(WC_EXTRA_PFX)?;
        self.ref_shard_blocks.write_to(builder)?;
        Ok(())
    }
}

impl Deserializable for WcExtra {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_u16()?;
        if tag != WC_EXTRA_PFX {
            fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: type_name::<Self>().to_string()
            })
        }
        self.ref_shard_blocks.read_from(slice)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/test_xp25.rs"]
mod tests;
