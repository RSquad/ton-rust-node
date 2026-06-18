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
use crate::{db::DbKey, error::StorageError, traits::Serializable, types::DbSlice, TARGET};
use adnl::common::add_unbound_object_to_map;
use rocksdb::{
    BoundColumnFamily, Cache, DBWithThreadMode, IteratorMode, MultiThreaded, Options,
    SnapshotWithThreadMode, WriteBatch,
};
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc, Mutex,
    },
};
use ton_block::{error, fail, BlockIdExt, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessType {
    ReadWrite,
    ReadOnly,
    Secondary(PathBuf),
}

pub const LAST_UNNEEDED_KEY_BLOCK: &str = "LastUnneededKeyBlockId"; // Latest key block we can delete in archives GC
pub const NODE_STATE_DB_NAME: &str = "node_state_db";
pub const NODE_DB_NAME: &str = "db";
pub const CATCHAINS_DB_NAME: &str = "catchains";

pub type DbPredicateMut<'a> = &'a mut dyn FnMut(&[u8], &[u8]) -> Result<bool>;

pub struct RocksDb {
    db: Option<DBWithThreadMode<MultiThreaded>>,
    locks: lockfree::map::Map<String, AtomicI32>,
    // Block caches attached to this DB's column families.
    caches: Mutex<Vec<Cache>>,
}

impl RocksDb {
    /// Creates new instance with given path and ability to additionally configure options
    pub fn new(
        path: impl AsRef<Path>,
        name: &str,
        spec_cf_opts: impl Into<Option<HashMap<String, Options>>>,
        access_type: AccessType,
    ) -> Result<Arc<Self>> {
        let path = path.as_ref().join(name);
        let mut spec_cf_opts = spec_cf_opts.into().unwrap_or_default();
        let options = Self::build_db_options();

        let cfs = DBWithThreadMode::<MultiThreaded>::list_cf(&options, &path).unwrap_or_default();

        log::info!(
            target: TARGET,
            "Opening DB {} (mode: {:?}) with {} cfs",
            name, access_type, cfs.len()
        );

        let cfs_opt = cfs.clone().into_iter().map(|cf| {
            let opt = spec_cf_opts.remove(&cf).unwrap_or_default();
            rocksdb::ColumnFamilyDescriptor::new(cf, opt)
        });
        let db = match &access_type {
            AccessType::ReadWrite => {
                DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&options, &path, cfs_opt)?
            }
            AccessType::ReadOnly => {
                DBWithThreadMode::<MultiThreaded>::open_cf_descriptors_read_only(
                    &options, &path, cfs_opt, false,
                )?
            }
            AccessType::Secondary(secondary_path) => {
                DBWithThreadMode::<MultiThreaded>::open_cf_descriptors_as_secondary(
                    &options,
                    &path,
                    secondary_path,
                    cfs_opt,
                )?
            }
        };

        // Clean up CFs from old archives in-place.
        if !matches!(access_type, AccessType::ReadOnly | AccessType::Secondary(_))
            && cfs.len() > 100
        {
            Self::clean_up_old_cf(&db, &cfs)
                .map_err(|e| error!("Error while clean_up_old_cf: {}", e))?;
        }

        Ok(Arc::new(Self {
            db: Some(db),
            locks: lockfree::map::Map::new(),
            caches: Mutex::new(Vec::new()),
        }))
    }

    fn build_db_options() -> Options {
        let mut options = Options::default();

        // If true, the database will be created if it is missing.
        options.create_if_missing(true);

        // By default, RocksDB uses only one background thread for flush and
        // compaction. Calling this function will set it up such that total of
        // `total_threads` is used. Good value for `total_threads` is the number of
        // cores.
        let num_cpus = std::thread::available_parallelism().unwrap().get();
        options.set_max_subcompactions(std::cmp::max(num_cpus as u32 / 2, 1));
        options.set_max_background_jobs(std::cmp::max(num_cpus as i32 / 2, 2));
        options.increase_parallelism(num_cpus as i32);

        // If true, missing column families will be automatically created.
        options.create_missing_column_families(true);

        options.set_max_total_wal_size(1024 * 1024 * 1024);

        // Bound the number of open SST TableReaders. Default is -1 (unlimited),
        // which on an archival node with tens of thousands of CFs causes
        // table-reader memory to grow without bound (each open SST keeps its
        // block index, bloom filter and pinned index/filter blocks in RAM —
        // tens to hundreds of KB per file). Observed: 170k+ open .sst fds.
        // 8192 lets RocksDB LRU-evict cold TableReaders.
        options.set_max_open_files(8192);
        // Shard the table cache to reduce lock contention with many CFs.
        options.set_table_cache_num_shard_bits(6);

        // Let compaction and flush go through the OS page cache (the default)
        // so that newly created SST files are already warm in cache and
        // subsequent reads don't cause cold disk I/O. Previously direct I/O
        // was enabled here to avoid compaction traffic evicting useful data,
        // but with partitioned filters the metadata working set is much
        // smaller, so the eviction pressure from compaction is acceptable.

        options.enable_statistics();
        options.set_dump_malloc_stats(true);

        // Specify the maximal size of the info log file. If the log file
        // is larger than `max_log_file_size`, a new info log file will
        // be created.
        // If max_log_file_size == 0, all logs will be written to one log file.
        options.set_max_log_file_size(1024 * 1024 * 100);

        // Maximal info log files to be kept.
        // Default: 1000
        options.set_keep_log_file_num(3);

        options
    }

    fn clean_up_old_cf(db: &DBWithThreadMode<MultiThreaded>, cfs: &[String]) -> Result<bool> {
        if let Some(cf) = db.cf_handle(NODE_STATE_DB_NAME) {
            if let Ok(Some(db_slice)) = db.get_pinned_cf(&cf, LAST_UNNEEDED_KEY_BLOCK) {
                let id = BlockIdExt::deserialize(db_slice.as_ref())?;
                let prefixes = ["entry_meta_db_", "offsets_db_", "status_db_"];
                log::info!(target: TARGET, "Read last unneeded key block: {}", id.seq_no());
                for cf_name in cfs {
                    for pfx in &prefixes {
                        if cf_name.contains(pfx) {
                            let num = cf_name.replace(pfx, "").parse::<u32>().unwrap_or(u32::MAX);
                            if num < id.seq_no() {
                                log::warn!(target: TARGET, "Dropping old CF {cf_name}");
                                let result = db.drop_cf(cf_name);
                                log::warn!(target: TARGET, "Dropped old CF {cf_name}: {result:?}");
                            }
                            break;
                        }
                    }
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn table<K: DbKey + Send + Sync>(
        self: Arc<Self>,
        family: impl ToString,
        create_if_not_exist: bool,
    ) -> Result<RocksDbTable<K>> {
        RocksDbTable::with_db(self, family, create_if_not_exist)
    }

    pub fn db(&self) -> &DBWithThreadMode<MultiThreaded> {
        self.db.as_ref().expect("rocksdb was occasionaly destroyed")
    }

    /// Registers a block cache associated with one of this DB's column families.
    pub fn register_cache(&self, cache: Cache) {
        self.caches.lock().expect("caches mutex poisoned").push(cache);
    }

    /// Returns approximate memory usage of this RocksDB instance (in bytes).
    pub fn memory_usage(&self) -> crate::RocksDbMemoryUsage {
        let Ok(mut builder) = rocksdb::perf::MemoryUsageBuilder::new() else {
            return Default::default();
        };
        builder.add_db(self.db());
        // approximate_cache_total() reports only caches registered via add_cache().
        if let Ok(caches) = self.caches.lock() {
            for cache in caches.iter() {
                builder.add_cache(cache);
            }
        }
        let Ok(mu) = builder.build() else {
            return Default::default();
        };
        crate::RocksDbMemoryUsage {
            mem_tables: mu.approximate_mem_table_total(),
            block_cache: mu.approximate_cache_total(),
            table_readers: mu.approximate_mem_table_readers_total(),
        }
    }

    pub fn cfs(&self) -> Option<Vec<String>> {
        let Some(db) = &self.db else { return None };
        DBWithThreadMode::<MultiThreaded>::list_cf(&Options::default(), db.path()).ok()
    }

    // Error is occured if column family is already created
    fn create_cf(&self, name: &str) -> Result<()> {
        self.db().create_cf(name, &Options::default())?;
        Ok(())
    }

    pub fn drop_table(&self, name: &str) -> Result<bool> {
        if let Some(lock) = self.locks.get(name) {
            let lock = lock.val();
            if lock.compare_exchange(0, -1000000, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                self.db().drop_cf(name)?;
                self.locks.remove(name);
                return Ok(true);
            } else {
                return Ok(false);
            }
        }
        fail!("Attempt to drop already dropped table {}", name)
    }

    pub fn drop_table_force(&self, name: &str) -> Result<()> {
        if self.drop_table(name).is_err() {
            self.db().drop_cf(name)?;
            self.locks.remove(name);
        }
        Ok(())
    }

    fn cf(&self, name: &str) -> Result<Arc<BoundColumnFamily<'_>>> {
        self.db()
            .cf_handle(name)
            .ok_or_else(|| error!("no handle for column family {} in rocksdb", name))
    }

    pub fn destroy_db(path: impl AsRef<Path>) -> Result<bool> {
        let opts = Options::default();
        log::info!(target: TARGET, "Destroying DB {}...", path.as_ref().display());
        match DBWithThreadMode::<MultiThreaded>::destroy(&opts, path.as_ref()) {
            Ok(_) => Ok(true),
            Err(err) => fail!("cannot destroy database {:?} : {}", path.as_ref(), err),
        }
    }

    pub fn destroy(&mut self) -> Result<bool> {
        match self.db.take() {
            Some(db) => {
                let path = db.path().to_path_buf();
                drop(db);
                Self::destroy_db(path)
            }
            None => fail!("rocksdb already destroyed"),
        }
    }

    fn get_meta(&self) -> &str {
        self.db().path().to_str().unwrap()
    }

    fn try_get_raw(&self, key: &[u8]) -> Result<Option<DbSlice<'_>>> {
        Ok(self.db().get_pinned(key)?.map(|value| value.into()))
    }

    pub fn try_get<K: DbKey + Send + Sync>(&self, key: &K) -> Result<Option<DbSlice<'_>>> {
        self.try_get_raw(key.key())
    }

    /// Gets value from collection by the key
    pub fn get<K: DbKey + Send + Sync>(&self, key: &K) -> Result<DbSlice<'_>> {
        self.try_get(key)?.ok_or_else(|| {
            let meta = self.get_meta();
            let what = if meta.is_empty() {
                key.as_string()
            } else {
                format!("{} ({})", key.as_string(), meta)
            };
            StorageError::KeyNotFound(key.key_name(), what).into()
        })
    }

    fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<()> {
        Ok(self.db().put(key, value)?)
    }

    /// Puts value into collection by the key
    pub fn put<K: DbKey + Send + Sync>(&self, key: &K, value: &[u8]) -> Result<()> {
        self.put_raw(key.key(), value)
    }

    pub fn delete_raw(&self, key: &[u8]) -> Result<()> {
        Ok(self.db().delete(key)?)
    }

    /// Deletes value from collection by the key
    pub fn delete<K: DbKey + Send + Sync>(&self, key: &K) -> Result<()> {
        self.delete_raw(key.key())
    }

    pub fn contains<K: DbKey + Send + Sync>(&self, key: &K) -> Result<bool> {
        Ok(self.try_get(key)?.is_some())
    }

    pub fn create_checkpoint(&self, path: impl AsRef<Path>) -> Result<()> {
        let Some(db) = &self.db else {
            fail!("Database already destroyed");
        };
        let checkpoint = rocksdb::checkpoint::Checkpoint::new(db)?;
        checkpoint.create_checkpoint(path)?;
        Ok(())
    }
}

pub struct RocksDbTable<K> {
    db: Arc<RocksDb>,
    family: String,
    phantom: std::marker::PhantomData<K>,
}

impl<K: DbKey + Send + Sync> RocksDbTable<K> {
    pub(crate) fn with_db(
        db: Arc<RocksDb>,
        family: impl ToString,
        create_if_not_exist: bool,
    ) -> Result<Self> {
        let family = family.to_string();
        loop {
            if db.locks.get(&family).is_some() {
                break;
            }
            if let Err(e) = db.cf(&family) {
                if create_if_not_exist {
                    db.create_cf(&family)?;
                } else {
                    log::warn!(
                        target:TARGET,
                        "Column family {family} cannot be opened and is not allowed to create: {e}"
                    );
                    break;
                }
            }
            add_unbound_object_to_map(&db.locks, family.clone(), || Ok(AtomicI32::new(0)))?;
        }
        let ret = Self { db, family, phantom: std::marker::PhantomData };
        Ok(ret)
    }

    fn cf(&self) -> Result<Arc<BoundColumnFamily<'_>>> {
        self.db.cf(&self.family)
    }

    pub fn len(&self) -> Result<usize> {
        // be careful in usual code
        Ok(self.db.iterator_cf(&self.cf()?, IteratorMode::Start).count())
    }

    /// Returns true, if collection is empty; false otherwise
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.db.iterator_cf(&self.cf()?, IteratorMode::Start).next().is_none())
    }

    pub fn destroy(&mut self) -> Result<bool> {
        self.db.drop_table(&self.family)
    }

    fn get_meta(&self) -> &str {
        self.family.as_str()
    }

    pub fn try_get_raw(&self, key: &[u8]) -> Result<Option<DbSlice<'_>>> {
        if let Some(lock) = self.db.locks.get(&self.family) {
            let lock = lock.val();
            if lock.fetch_add(1, Ordering::Relaxed) >= 0 {
                let ret = self.db.get_pinned_cf(&self.cf()?, key);
                lock.fetch_sub(1, Ordering::Relaxed);
                return Ok(ret?.map(|value| value.into()));
            }
        }
        fail!("Attempt to read from dropped table {}", self.family)
    }

    pub fn for_each(&self, predicate: DbPredicateMut) -> Result<bool> {
        if let Some(lock) = self.db.locks.get(&self.family) {
            let lock = lock.val();
            if lock.fetch_add(1, Ordering::Relaxed) >= 0 {
                for iter in self.db.iterator_cf(&self.cf()?, IteratorMode::Start) {
                    let (key, value) = iter?;
                    match predicate(key.as_ref(), value.as_ref()) {
                        Ok(false) => {
                            lock.fetch_sub(1, Ordering::Relaxed);
                            return Ok(false);
                        }
                        Ok(true) => (),
                        Err(e) => {
                            lock.fetch_sub(1, Ordering::Relaxed);
                            return Err(e);
                        }
                    }
                }
                lock.fetch_sub(1, Ordering::Relaxed);
                return Ok(true);
            }
        }
        fail!("Attempt to iterate over dropped table {}", self.family)
    }

    pub fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<()> {
        if let Some(lock) = self.db.locks.get(&self.family) {
            let lock = lock.val();
            if lock.fetch_add(1, Ordering::Relaxed) >= 0 {
                let ret = self.db.put_cf(&self.cf()?, key, value);
                lock.fetch_sub(1, Ordering::Relaxed);
                return Ok(ret?);
            }
        }
        fail!("Attempt to write into dropped table {}", self.family)
    }

    /// Puts value into collection by the key
    pub fn put(&self, key: &K, value: &[u8]) -> Result<()> {
        self.put_raw(key.key(), value)
    }

    pub fn delete_raw(&self, key: &[u8]) -> Result<()> {
        if let Some(lock) = self.db.locks.get(&self.family) {
            let lock = lock.val();
            if lock.fetch_add(1, Ordering::Relaxed) >= 0 {
                let ret = self.db.delete_cf(&self.cf()?, key);
                lock.fetch_sub(1, Ordering::Relaxed);
                return Ok(ret?);
            }
        }
        fail!("Attempt to delete from dropped table {}", self.family)
    }

    /// Deletes value from collection by the key
    pub fn delete(&self, key: &K) -> Result<()> {
        self.delete_raw(key.key())
    }

    /// Append a delete operation for `key` into `batch`. Caller commits.
    pub fn add_delete_to_batch(&self, batch: &mut WriteBatch, key: &K) -> Result<()> {
        batch.delete_cf(&self.cf()?, key.key());
        Ok(())
    }

    pub fn try_get(&self, key: &K) -> Result<Option<DbSlice<'_>>> {
        self.try_get_raw(key.key())
    }

    /// Gets value from collection by the key
    pub fn get(&self, key: &K) -> Result<DbSlice<'_>> {
        self.try_get(key)?.ok_or_else(|| {
            let meta = self.get_meta();
            let what = if meta.is_empty() {
                key.as_string()
            } else {
                format!("{} ({})", key.as_string(), meta)
            };
            StorageError::KeyNotFound(key.key_name(), what).into()
        })
    }

    /// Gets slice with given size starting from given offset from collection by the key
    pub fn get_slice(&self, key: &K, offset: u64, size: u64) -> Result<DbSlice<'_>> {
        self.get_vec(key, offset, size).map(DbSlice::Vector)
    }

    fn get_vec(&self, key: &K, offset: u64, size: u64) -> Result<Vec<u8>> {
        self.get(key).and_then(|value| {
            if offset >= value.len() as u64 || offset + size > value.as_ref().len() as u64 {
                return Err(StorageError::OutOfRange.into());
            }

            let mut result = Vec::new();
            result.extend_from_slice(&value[offset as usize..(offset + size) as usize]);
            Ok(result)
        })
    }

    /// Determines, is key exists in key-value collection
    pub fn contains(&self, key: &K) -> Result<bool> {
        Ok(self.try_get(key)?.is_some())
    }

    pub fn snapshot(&self) -> Result<Arc<RocksDbSnapshot<'_>>> {
        Ok(Arc::new(RocksDbSnapshot::new(self.db.clone(), self.db.snapshot(), self.family.clone())))
    }

    pub fn begin_transaction(&self) -> Result<Box<RocksDbTransaction>> {
        Ok(Box::new(RocksDbTransaction::new(self.db.clone(), self.family.clone())))
    }
}

impl Deref for RocksDb {
    type Target = rocksdb::DBWithThreadMode<MultiThreaded>;

    fn deref(&self) -> &Self::Target {
        self.db.as_ref().unwrap()
    }
}

// TODO: snapshot without family by RocksDb
pub struct RocksDbSnapshot<'db> {
    db: Arc<RocksDb>,
    snapshot: SnapshotWithThreadMode<'db, DBWithThreadMode<MultiThreaded>>,
    family: String,
}

impl<'db> RocksDbSnapshot<'db> {
    pub(crate) fn new(
        db: Arc<RocksDb>,
        snapshot: SnapshotWithThreadMode<'db, DBWithThreadMode<MultiThreaded>>,
        family: String,
    ) -> Self {
        Self { db, snapshot, family }
    }

    /// Get meta information (like DB name or something)
    fn get_meta(&self) -> &str {
        ""
    }

    fn cf(&self) -> Result<Arc<BoundColumnFamily<'_>>> {
        self.db.cf(&self.family)
    }

    fn try_get_raw(&self, key: &[u8]) -> Result<Option<DbSlice<'_>>> {
        Ok(self.snapshot.get_cf(&self.cf()?, key)?.map(|value| value.into()))
    }

    fn try_get<K: DbKey + Send + Sync>(&self, key: &K) -> Result<Option<DbSlice<'_>>> {
        self.try_get_raw(key.key())
    }

    /// Gets value from collection by the key
    pub fn get<K: DbKey + Send + Sync>(&self, key: &K) -> Result<DbSlice<'_>> {
        self.try_get(key)?.ok_or_else(|| {
            let meta = self.get_meta();
            let what = if meta.is_empty() {
                key.as_string()
            } else {
                format!("{} ({})", key.as_string(), meta)
            };
            StorageError::KeyNotFound(key.key_name(), what).into()
        })
    }

    pub fn for_each(&self, predicate: DbPredicateMut) -> Result<bool> {
        for iter in self.snapshot.iterator_cf(&self.cf()?, IteratorMode::Start) {
            let (key, value) = iter?;
            if !predicate(key.as_ref(), value.as_ref())? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl Debug for RocksDbSnapshot<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[snapshot] for {}", self.family)
    }
}

// TODO: Batch for RockDb

pub struct RocksDbTransaction {
    db: Arc<RocksDb>,
    batch: Option<WriteBatch>,
    family: String,
}

/// Implementation of transaction for key-value collection for RocksDB.
impl RocksDbTransaction {
    fn new(db: Arc<RocksDb>, family: String) -> Self {
        Self { db, batch: Some(WriteBatch::default()), family }
    }
    fn cf(&self) -> Result<Arc<BoundColumnFamily<'_>>> {
        self.db.cf(&self.family)
    }

    pub fn put_raw(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut batch = self.batch.take().unwrap();
        batch.put_cf(&self.cf()?, key, value);
        self.batch = Some(batch);
        Ok(())
    }

    /// Puts value into collection by the key
    pub fn put<K: DbKey + Send + Sync>(&mut self, key: &K, value: &[u8]) -> Result<()> {
        self.put_raw(key.key(), value)
    }

    pub fn delete_raw(&mut self, key: &[u8]) -> Result<()> {
        let mut batch = self.batch.take().unwrap();
        batch.delete_cf(&self.cf()?, key);
        self.batch = Some(batch);
        Ok(())
    }

    /// Deletes value from collection by the key
    pub fn delete<K: DbKey + Send + Sync>(&mut self, key: &K) -> Result<()> {
        self.delete_raw(key.key())
    }

    pub fn commit(self) -> Result<()> {
        Ok(self.db.write(self.batch.unwrap())?)
    }
}

pub async fn destroy_rocks_db(path: &str, name: &str) -> ton_block::Result<()> {
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    let mut path = std::path::Path::new(path);
    let db_path = path.join(name);
    // Clean up DB
    if db_path.exists() {
        let opts = rocksdb::Options::default();
        while let Err(e) =
            rocksdb::DBWithThreadMode::<rocksdb::MultiThreaded>::destroy(&opts, db_path.as_path())
        {
            println!("Can't destroy DB: {}", e);
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        }
    }
    // Clean up DB folder
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path)
            .map_err(|e| ton_block::error!("Can't clean DB folder {}: {}", db_path.display(), e))?
    }
    // Clean up upper folder if empty
    while path.exists() {
        if std::fs::read_dir(path)?.count() > 0 {
            break;
        }
        if let Err(e) = std::fs::remove_dir_all(path) {
            println!("Ignored: can't clean DB enclosing folder {}: {}", path.display(), e);
            break;
        }
        path = if let Some(path) = path.parent() { path } else { break }
    }
    Ok(())
}
