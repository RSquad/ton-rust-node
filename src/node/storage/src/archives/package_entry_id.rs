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
use std::{
    borrow::Borrow,
    collections::hash_map::DefaultHasher,
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
    str::FromStr,
};
use ton_block::{base64_encode, error, fail, BlockIdExt, Result, ShardIdent, UInt256};

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum PackageEntryId<B: Borrow<BlockIdExt>> {
    Empty,
    Block(B),
    ZeroState(B),
    PersistentState((B, B)),
    Proof(B),
    ProofLink(B),
    Signatures(B),
    Candidate((B, UInt256, UInt256)),
    BlockInfo(B),
}

impl PackageEntryId<BlockIdExt> {
    pub fn from_filename(filename: &str) -> Result<Self> {
        if filename == Self::Empty.filename_prefix() {
            return Ok(PackageEntryId::Empty);
        }
        let mut template = PackageEntryId::Block(BlockIdExt::default());
        loop {
            let prefix = template.filename_prefix();
            if !filename.starts_with((prefix.to_string() + "_").as_str()) {
                template = match template {
                    PackageEntryId::Block(b) => PackageEntryId::ZeroState(b),
                    PackageEntryId::ZeroState(b) => PackageEntryId::Proof(b),
                    PackageEntryId::Proof(b) => PackageEntryId::ProofLink(b),
                    PackageEntryId::ProofLink(b) => PackageEntryId::Signatures(b),
                    PackageEntryId::Signatures(b) => PackageEntryId::BlockInfo(b),
                    PackageEntryId::BlockInfo(b) => PackageEntryId::PersistentState((b.clone(), b)),
                    _ => fail!("Cannot accept filename {filename}"),
                };
                continue;
            }
            let mut pos = prefix.len() + 1;
            let (block_id, len) = Self::parse_block_id(&filename[pos..filename.len()])?;
            match &mut template {
                PackageEntryId::PersistentState((id1, id2)) => {
                    pos += len + 1;
                    let (block_id2, _) = Self::parse_block_id(&filename[pos..filename.len()])?;
                    *id1 = block_id;
                    *id2 = block_id2;
                }
                PackageEntryId::Block(id)
                | PackageEntryId::BlockInfo(id)
                | PackageEntryId::Proof(id)
                | PackageEntryId::ProofLink(id)
                | PackageEntryId::Signatures(id)
                | PackageEntryId::ZeroState(id) => *id = block_id,
                _ => (),
            };
            break Ok(template);
        }
    }
}

impl<B: Borrow<BlockIdExt>> PackageEntryId<B> {
    fn filename_prefix(&self) -> &'static str {
        match self {
            PackageEntryId::Empty => "empty",
            PackageEntryId::Block(_) => "block",
            PackageEntryId::ZeroState(_) => "zerostate",
            PackageEntryId::PersistentState(_) => "state",
            PackageEntryId::Proof(_) => "proof",
            PackageEntryId::ProofLink(_) => "prooflink",
            PackageEntryId::Signatures(_) => "signatures",
            PackageEntryId::Candidate(_) => "candidate",
            PackageEntryId::BlockInfo(_) => "info",
        }
    }

    // parse block ID: (wc,shard,seqno):rh:fh,
    // for example (-1,F800000000000000,10):19..68:5C..DC
    fn parse_block_id(filename: &str) -> Result<(BlockIdExt, usize)> {
        fn find_id(ids: &str, upto: char) -> Result<usize> {
            let ret = ids.find(upto).ok_or_else(|| error!("separator {upto} not found"))?;
            if ids.len() == ret + 1 {
                fail!("too short")
            }
            Ok(ret)
        }
        fn parse_ids(ids: &str) -> Result<(BlockIdExt, usize)> {
            if find_id(ids, '(')? != 0 {
                fail!("bad format")
            }
            let pos_a = find_id(&ids[1..], ',')? + 1;
            let pos_b = find_id(&ids[pos_a + 1..], ',')? + pos_a + 1;
            let pos_c = find_id(&ids[pos_b + 1..], ')')? + pos_b + 1;
            if find_id(&ids[pos_c + 1..], ':')? != 0 {
                fail!("bad format")
            }
            let pos_d = find_id(&ids[pos_c + 2..], ':')? + pos_c + 2;
            if ids.len() <= pos_d + 64 {
                fail!("bad format")
            }
            let id = BlockIdExt {
                shard_id: ShardIdent::with_tagged_prefix(
                    i32::from_str(&ids[1..pos_a])?,
                    u64::from_str_radix(&ids[pos_a + 1..pos_b], 16)?,
                )?,
                seq_no: u32::from_str(&ids[pos_b + 1..pos_c])?,
                root_hash: UInt256::from_str(&ids[pos_c + 2..pos_d])?,
                file_hash: UInt256::from_str(&ids[pos_d + 1..pos_d + 65])?,
            };
            Ok((id, pos_d + 65))
        }
        parse_ids(filename).map_err(|e| error!("Invalid block id {filename}: {e}"))
    }
}

impl<B: Borrow<BlockIdExt>> Display for PackageEntryId<B> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.filename().as_str())
    }
}

/// parse file name
/// for example block_555_F800000000000000_100_19685CDC8B64BBB5
pub fn parse_short_filename(filename: &str) -> Result<(i32, u64, u32)> {
    enum Id {
        Wc = 1,
        Shard,
        SeqNo,
        Count = 5,
    }
    fn parse_ids(ids: Vec<&str>) -> Result<(i32, u64, u32)> {
        if ids.len() != Id::Count as usize {
            fail!("too short")
        } else {
            let ret = (
                i32::from_str(ids[Id::Wc as usize])?,
                u64::from_str_radix(ids[Id::Shard as usize], 16)?,
                u32::from_str(ids[Id::SeqNo as usize])?,
            );
            Ok(ret)
        }
    }
    parse_ids(filename.split('_').collect())
        .map_err(|e| error!("Invalid block file name {}: {}", filename, e))
}

pub trait GetFileName {
    fn filename(&self) -> String {
        self.filename_any(false)
    }
    fn filename_short(&self) -> String {
        self.filename_any(true)
    }
    fn filename_any(&self, short: bool) -> String;
}

impl GetFileName for BlockIdExt {
    fn filename_any(&self, short: bool) -> String {
        if short {
            let mut hasher = DefaultHasher::new();
            self.hash(&mut hasher);
            format!(
                "{}_{:016X}_{}_{:016X}",
                self.shard().workchain_id(),
                self.shard().shard_prefix_with_tag(),
                self.seq_no(),
                hasher.finish()
            )
        } else {
            format!(
                "({},{:016x},{}):{:064X}:{:064X}",
                self.shard().workchain_id(),
                self.shard().shard_prefix_with_tag(),
                self.seq_no(),
                self.root_hash(),
                self.file_hash(),
            )
        }
    }
}

impl<B: Borrow<BlockIdExt>> GetFileName for PackageEntryId<B> {
    fn filename_any(&self, short: bool) -> String {
        match self {
            PackageEntryId::Empty => self.filename_prefix().to_string(),
            PackageEntryId::Block(id)
            | PackageEntryId::BlockInfo(id)
            | PackageEntryId::Proof(id)
            | PackageEntryId::ProofLink(id)
            | PackageEntryId::Signatures(id)
            | PackageEntryId::ZeroState(id) => {
                format!("{}_{}", self.filename_prefix(), id.borrow().filename_any(short))
            }
            PackageEntryId::PersistentState((mc_id, id)) => format!(
                "{}_{}_{}",
                self.filename_prefix(),
                mc_id.borrow().filename_any(short),
                id.borrow().filename_any(short)
            ),
            PackageEntryId::Candidate((id, collated_data_hash, source)) => format!(
                "{}_{}_{:X}_{}",
                self.filename_prefix(),
                id.borrow().filename_any(short),
                collated_data_hash,
                base64_encode(source)
            ),
        }
    }
}

#[cfg(test)]
#[path = "../tests/test_package_entry_id.rs"]
mod tests;
