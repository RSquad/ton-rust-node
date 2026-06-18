/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{block_handle_db::BlockHandle, db::rocksdb::RocksDb};
use std::{fmt::Display, ops::Sub, sync::Arc};
use ton_block::{error, fail, AccountIdPrefixFull, ByteOrderRead, Result, ShardIdent};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlocksIndexKey {
    Seqno { shard: ShardIdent, seqno: u32 },
    Lt { shard: ShardIdent, lt: u64 },
    // In some configuration it is possible more than one block with the same utime,
    // so store seqno to make the key unique.
    Utime { shard: ShardIdent, utime: u32, seqno: u32 },
}
impl BlocksIndexKey {
    const SEQNO_PFX: &'static str = "sn";
    const LT_PFX: &'static str = "lt";
    const UTIME_PFX: &'static str = "ut";

    pub fn key_with_seqno(shard: &ShardIdent, seqno: u32) -> String {
        format!(
            "{}:{}:{:010}:{:016X}",
            Self::SEQNO_PFX,
            shard.workchain_id(),
            seqno,
            shard.shard_prefix_with_tag()
        )
    }
    pub fn prefix_with_seqno(wc: i32, seqno: u32) -> String {
        format!("{}:{}:{:010}:", Self::SEQNO_PFX, wc, seqno,)
    }
    pub fn key_with_lt(shard: &ShardIdent, lt: u64) -> String {
        format!(
            "{}:{}:{:016}:{:016X}",
            Self::LT_PFX,
            shard.workchain_id(),
            lt,
            shard.shard_prefix_with_tag()
        )
    }
    pub fn prefix_with_lt(wc: i32, lt: u64) -> String {
        format!("{}:{}:{:016}:", Self::LT_PFX, wc, lt,)
    }
    pub fn key_with_utime(shard: &ShardIdent, utime: u32, seqno: u32) -> String {
        format!(
            "{}:{}:{:010}:{:016X}:{:010}",
            Self::UTIME_PFX,
            shard.workchain_id(),
            utime,
            shard.shard_prefix_with_tag(),
            seqno
        )
    }
    pub fn prefix_with_utime(wc: i32, utime: u32) -> String {
        format!("{}:{}:{:010}:", Self::UTIME_PFX, wc, utime,)
    }
    #[cfg(test)]
    pub fn to_string(&self) -> String {
        match self {
            BlocksIndexKey::Seqno { shard, seqno } => Self::key_with_seqno(shard, *seqno),
            BlocksIndexKey::Lt { shard, lt } => Self::key_with_lt(shard, *lt),
            BlocksIndexKey::Utime { shard, utime, seqno } => {
                Self::key_with_utime(shard, *utime, *seqno)
            }
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() < 4 {
            fail!("Invalid key format");
        }
        let shard =
            ShardIdent::with_tagged_prefix(parts[1].parse()?, u64::from_str_radix(parts[3], 16)?)?;
        match parts[0] {
            Self::SEQNO_PFX => {
                if parts.len() != 4 {
                    fail!("Invalid key format");
                }
                Ok(BlocksIndexKey::Seqno { shard, seqno: parts[2].parse()? })
            }
            Self::LT_PFX => {
                if parts.len() != 4 {
                    fail!("Invalid key format");
                }
                Ok(BlocksIndexKey::Lt { shard, lt: parts[2].parse()? })
            }
            Self::UTIME_PFX => {
                if parts.len() != 5 {
                    fail!("Invalid key format");
                }
                Ok(BlocksIndexKey::Utime {
                    shard,
                    utime: parts[2].parse()?,
                    seqno: parts[4].parse()?,
                })
            }
            _ => fail!("Invalid key prefix"),
        }
    }
    pub fn parse_bytes(s: &[u8]) -> Result<Self> {
        Self::parse(std::str::from_utf8(s)?)
    }
}

pub struct LookupResult {
    pub shard: ShardIdent,
    pub mc_ref: u32,
    pub offset: u32,
}

pub struct BlockIndexDb {
    db: Arc<RocksDb>,
    cf_name: String,
}

impl BlockIndexDb {
    pub fn with_db(db: Arc<RocksDb>, cf_name: String, create_if_not_exist: bool) -> Result<Self> {
        if db.cf_handle(&cf_name).is_none() {
            if create_if_not_exist {
                let (options, cache) = Self::build_cf_options();
                db.create_cf(&cf_name, &options)?;
                db.register_cache(cache);
            } else {
                fail!("Column family `{}` does not exist", cf_name);
            }
        }
        Ok(Self { db, cf_name })
    }

    fn build_cf_options() -> (rocksdb::Options, rocksdb::Cache) {
        let mut options = rocksdb::Options::default();
        let mut block_opts = rocksdb::BlockBasedOptions::default();

        // specified cache for blocks.
        let cache = rocksdb::Cache::new_lru_cache(1024 * 1024);
        block_opts.set_block_cache(&cache);

        // save in LRU block cache also indexes and bloom filters
        block_opts.set_cache_index_and_filter_blocks(true);

        // keep indexes and filters in block cache until tablereader freed
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        // Setup bloom filter with length of 10 bits per key.
        // This length provides less than 1% false positive rate.
        block_opts.set_bloom_filter(10.0, false);

        options.set_block_based_table_factory(&block_opts);

        // Enable whole key bloom filter in memtable.
        options.set_memtable_whole_key_filtering(true);

        (options, cache)
    }

    pub fn put(&self, block: &BlockHandle, offset: u32) -> Result<()> {
        log::trace!(
            "Indexing block {}, offset: {}, mc ref: {}",
            block.id(),
            offset,
            block.masterchain_ref_seq_no()
        );
        self.put_raw(
            block.id().shard(),
            block.id().seq_no(),
            block.end_lt(),
            block.gen_utime(),
            block.masterchain_ref_seq_no(),
            offset,
        )
    }

    /// Write block index entries from raw values (for archive import).
    pub fn put_raw(
        &self,
        shard: &ShardIdent,
        seq_no: u32,
        end_lt: u64,
        gen_utime: u32,
        mc_ref_seq_no: u32,
        offset: u32,
    ) -> Result<()> {
        let cf = self.cf()?;

        let value = Self::serialize_value(mc_ref_seq_no, offset);
        let mut transaction = rocksdb::WriteBatch::default();

        let key = BlocksIndexKey::key_with_lt(shard, end_lt);
        log::trace!("Putting key: {}", key);
        transaction.put_cf(&cf, &key, value);

        let key = BlocksIndexKey::key_with_seqno(shard, seq_no);
        log::trace!("Putting key: {}", key);
        transaction.put_cf(&cf, &key, value);

        let key = BlocksIndexKey::key_with_utime(shard, gen_utime, seq_no);
        log::trace!("Putting key: {}", key);
        transaction.put_cf(&cf, &key, value);

        self.db.write(transaction)?;

        Ok(())
    }

    pub fn lookup_by_lt(
        &self,
        acc_prefix: &AccountIdPrefixFull,
        lt: u64,
    ) -> Result<Option<LookupResult>> {
        log::trace!("Searching block by lt: {}, prefix: {}", lt, acc_prefix);
        let mut result = None;
        self.lookup_by(
            acc_prefix,
            &BlocksIndexKey::prefix_with_lt(acc_prefix.workchain_id(), lt),
            false,
            &|k| {
                let parsed_key = BlocksIndexKey::parse_bytes(k).ok()?;
                let BlocksIndexKey::Lt { shard, lt: found_lt } = parsed_key else {
                    return None;
                };
                Some((shard, found_lt))
            },
            lt,
            &mut |r| {
                // Stop at the first found block because shard&LT is unique key
                result = Some(r);
                Ok(false)
            },
        )?;
        Ok(result)
    }

    pub fn lookup_by_utime(
        &self,
        acc_prefix: &AccountIdPrefixFull,
        utime: u32,
        process_result: &mut dyn FnMut(LookupResult) -> Result<bool>,
    ) -> Result<()> {
        log::trace!("Searching block by utime: {}, prefix: {}", utime, acc_prefix);
        self.lookup_by(
            acc_prefix,
            &BlocksIndexKey::prefix_with_utime(acc_prefix.workchain_id(), utime),
            false,
            &|k| {
                let parsed_key = BlocksIndexKey::parse_bytes(k).ok()?;
                let BlocksIndexKey::Utime { shard, utime: found_utime, seqno: _ } = parsed_key
                else {
                    return None;
                };
                Some((shard, found_utime))
            },
            utime,
            process_result,
        )
    }

    pub fn lookup_by_seqno(
        &self,
        acc_prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<LookupResult>> {
        log::trace!("Searching block by seqno: {}, prefix: {}", seqno, acc_prefix);
        let mut result = None;
        self.lookup_by(
            acc_prefix,
            &BlocksIndexKey::prefix_with_seqno(acc_prefix.workchain_id(), seqno),
            true,
            &|k| {
                let parsed_key = BlocksIndexKey::parse_bytes(k).ok()?;
                let BlocksIndexKey::Seqno { shard, seqno: found_seqno } = parsed_key else {
                    return None;
                };
                Some((shard, found_seqno))
            },
            seqno,
            &mut |r| {
                // Stop at the first found block because shard&LT is unique key
                result = Some(r);
                Ok(false)
            },
        )?;
        Ok(result)
    }

    const MAX_READS: usize = 1000;

    fn lookup_by<T: Ord + Sub<Output = T> + Copy + Display>(
        &self,
        acc_prefix: &AccountIdPrefixFull,
        key_prefix: &str,
        exact: bool,
        parse_key: &dyn Fn(&[u8]) -> Option<(ShardIdent, T)>,
        target: T,
        process_result: &mut dyn FnMut(LookupResult) -> Result<bool>,
    ) -> Result<()> {
        let cf = self.cf()?;

        let mut iter = self.db.raw_iterator_cf(&cf);
        // Seeks to the specified key or the first key that **lexicographically follows** it.
        log::trace!("Seek to: {}", key_prefix);
        iter.seek(key_prefix.as_bytes());

        let mut reads = Self::MAX_READS;
        let mut found_exact = false;
        let mut check_reads_limit = || {
            reads -= 1;
            if reads == 0 {
                fail!("Too many reads, seems db is corrupted");
            }
            Ok(())
        };

        // Found key is >= requested prefix
        if iter.key().is_some() {
            while let Some(k) = iter.key() {
                check_reads_limit()?;
                let Some((shard, found_value)) = parse_key(k) else {
                    // No more target keys
                    break;
                };
                log::trace!("get_by: forward: found key: {shard} {found_value}");
                if shard.workchain_id() != acc_prefix.workchain_id() {
                    // No more target keys
                    break;
                }
                if shard.contains_full_prefix(acc_prefix) {
                    // If found key is equal to target it is possible to have more keys
                    // with the same value, otherwise no more then one key will be found
                    if found_value == target {
                        let (mc_ref, offset) = Self::deserialize_value(iter.value())?;
                        found_exact = true;
                        if !process_result(LookupResult { shard, mc_ref, offset })? {
                            return Ok(());
                        }
                    } else if found_value > target {
                        if !exact && !found_exact {
                            let (mc_ref, offset) = Self::deserialize_value(iter.value())?;
                            process_result(LookupResult { shard, mc_ref, offset })?;
                        }
                        break;
                    } else {
                        // found_value < target
                        fail!(
                            "Logic error: found_value {} < target {}, seems db is corrupted",
                            found_value,
                            target
                        );
                    }
                }
                iter.next();
            }
        }

        log::trace!("get_by: made {} reads", Self::MAX_READS - reads);
        Ok(())
    }

    pub fn destroy(&mut self) -> Result<()> {
        self.db.drop_cf(&self.cf_name)?;
        Ok(())
    }

    fn deserialize_value(value: Option<&[u8]>) -> Result<(u32, u32)> {
        let value = value.ok_or_else(|| error!("Value is None"))?;
        let mut cursor = std::io::Cursor::new(value);
        let offset = cursor.read_le_u32()?;
        let mc_ref = cursor.read_le_u32()?;
        Ok((mc_ref, offset))
    }

    fn serialize_value(mc_ref: u32, offset: u32) -> [u8; 8] {
        ((mc_ref as u64) << 32 | offset as u64).to_le_bytes()
    }

    fn cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(&self.cf_name)
            .ok_or_else(|| error!("Can't get `{}` cf handle", self.cf_name))
    }
}

#[cfg(test)]
#[path = "../tests/test_block_index_db.rs"]
mod tests;
