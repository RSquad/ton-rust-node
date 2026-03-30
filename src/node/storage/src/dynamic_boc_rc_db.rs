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
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    db::rocksdb::RocksDb,
    shardstate_db_async::CellsDbConfig,
    types::{StoredCell, StoringCell},
    StorageAlloc, TARGET,
};
use std::{
    fs::write,
    io::{Cursor, Write},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use ton_block::{
    error, fail, merkle_update::CellsFactory, BuilderData, ByteOrderRead, Cell, CellData,
    CellsStorage, CellsTempStorage, Result, UInt256, MAX_LEVEL, MAX_REFERENCES_COUNT,
};

pub const BROKEN_CELL_BEACON_FILE: &str = "ton_node.broken_cell";

// FnvHashMap is a standard HashMap with FNV hasher. This hasher is bit faster than default one.
pub type CellsCounters = fnv::FnvHashMap<UInt256, u32>;

#[derive(Debug, PartialEq, Eq)]
enum VisitedCell {
    New { cell: Cell, parents_count: u32 },
    Updated { parents_count: u32 },
}

impl VisitedCell {
    fn with_raw_counter(parents_count: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(parents_count);
        Ok(Self::Updated { parents_count: reader.read_le_u32()? })
    }

    fn with_counter(parents_count: u32) -> Self {
        Self::Updated { parents_count }
    }

    fn with_new_cell(cell: Cell) -> Self {
        Self::New { cell, parents_count: 1 }
    }

    fn inc_parents_count(&mut self) -> Result<u32> {
        let parents_count = match self {
            VisitedCell::New { parents_count, .. } => parents_count,
            VisitedCell::Updated { parents_count, .. } => parents_count,
        };
        if *parents_count == u32::MAX {
            fail!("Parents count has reached the maximum value");
        }
        *parents_count += 1;
        Ok(*parents_count)
    }

    fn dec_parents_count(&mut self) -> Result<u32> {
        let parents_count = match self {
            VisitedCell::New { parents_count, .. } => parents_count,
            VisitedCell::Updated { parents_count, .. } => parents_count,
        };
        if *parents_count == 0 {
            fail!("Can't decrement - parents count is already zero");
        }
        *parents_count -= 1;
        Ok(*parents_count)
    }

    fn parents_count(&self) -> u32 {
        match self {
            VisitedCell::New { parents_count, .. } => *parents_count,
            VisitedCell::Updated { parents_count, .. } => *parents_count,
        }
    }

    fn serialize_counter(&self) -> [u8; 4] {
        self.parents_count().to_le_bytes()
    }

    fn serialize_cell(&self) -> Result<Option<Vec<u8>>> {
        match self {
            VisitedCell::Updated { .. } => Ok(None),
            VisitedCell::New { cell, .. } => {
                let data = StoredCell::serialize(cell.deref())?;
                Ok(Some(data))
            }
        }
    }

    fn cell(&self) -> Option<&Cell> {
        match self {
            VisitedCell::New { cell, .. } => Some(cell),
            VisitedCell::Updated { .. } => None,
        }
    }
}

pub struct DynamicBocDb {
    db: Arc<RocksDb>,
    cells_cf_name: String,
    counters_cf_name: String,
    db_root_path: PathBuf,
    storing_cells: Arc<lockfree::map::Map<UInt256, Cell>>,
    storing_cells_count: AtomicU64,
    cells_counters: Option<Arc<parking_lot::Mutex<CellsCounters>>>,
    cell_cache: quick_cache::sync::Cache<UInt256, Cell>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<StorageTelemetry>,
    allocated: Arc<StorageAlloc>,
}

impl DynamicBocDb {
    pub(crate) fn with_db(
        db: Arc<RocksDb>,
        cell_db_cf: &str,
        counters_cf_name: &str,
        db_root_path: impl AsRef<Path>,
        config: &CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Self> {
        if db.cf_handle(cell_db_cf).is_none() {
            db.create_cf(cell_db_cf, &Self::build_cells_cf_options(config))?;
        }
        if db.cf_handle(counters_cf_name).is_none() {
            db.create_cf(counters_cf_name, &Self::build_cells_cf_options(config))?;
        }
        let cells_counters = if config.prefill_cells_counters {
            let counters = CellsCounters::default();
            Some(Arc::new(parking_lot::Mutex::new(counters)))
        } else {
            None
        };
        Ok(Self {
            db,
            cells_cf_name: cell_db_cf.to_string(),
            counters_cf_name: counters_cf_name.to_string(),
            db_root_path: db_root_path.as_ref().to_path_buf(),
            storing_cells: Arc::new(lockfree::map::Map::new()),
            storing_cells_count: AtomicU64::new(0),
            cells_counters,
            cell_cache: quick_cache::sync::Cache::new(config.cells_lru_cache_capacity),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        })
    }

    pub fn build_cells_cf_options(config: &CellsDbConfig) -> rocksdb::Options {
        Self::build_cf_options(config.cells_cache_size_bytes)
    }

    pub fn build_counters_cf_options(config: &CellsDbConfig) -> rocksdb::Options {
        Self::build_cf_options(config.counters_cache_size_bytes)
    }

    fn build_cf_options(cache_size: u64) -> rocksdb::Options {
        let mut options = rocksdb::Options::default();
        let mut block_opts = rocksdb::BlockBasedOptions::default();

        // specified cache for blocks.
        let cache = rocksdb::Cache::new_lru_cache(cache_size as usize);
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

        // Amount of data to build up in memory (backed by an unsorted log
        // on disk) before converting to a sorted on-disk file.
        //
        // Larger values increase performance, especially during bulk loads.
        // Up to max_write_buffer_number write buffers may be held in memory
        // at the same time,
        // so you may wish to adjust this parameter to control memory usage.
        // Also, a larger write buffer will result in a longer recovery time
        // the next time the database is opened.
        options.set_write_buffer_size(1024 * 1024 * 1024);

        // The maximum number of write buffers that are built up in memory.
        // The default and the minimum number is 2, so that when 1 write buffer
        // is being flushed to storage, new writes can continue to the other
        // write buffer.
        // If max_write_buffer_number > 3, writing will be slowed down to
        // options.delayed_write_rate if we are writing to the last write buffer
        // allowed.
        options.set_max_write_buffer_number(4);

        // if prefix_extractor is set and memtable_prefix_bloom_size_ratio is not 0,
        // create prefix bloom for memtable with the size of
        // write_buffer_size * memtable_prefix_bloom_size_ratio.
        // If it is larger than 0.25, it is sanitized to 0.25.
        let transform = rocksdb::SliceTransform::create_fixed_prefix(32);
        options.set_prefix_extractor(transform);
        options.set_memtable_prefix_bloom_ratio(0.1);

        options
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        if let Ok(cf) = self.counters_cf() {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start).count()
        } else {
            0
        }
    }

    // Is not thread-safe!
    pub fn save_boc(
        self: &Arc<Self>,
        root_cell: Cell,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Cell> {
        let root_id = root_cell.hash(MAX_LEVEL);
        log::debug!(target: TARGET, "DynamicBocDb::save_boc  {:x}", root_id);

        let cells_cf = self.cells_cf()?;

        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        if let Some(val) = self.db.get_pinned_cf(&cells_cf, root_id.as_slice())? {
            log::info!(target: TARGET, "DynamicBocDb::save_boc  ALREADY EXISTS  {:x}", root_id);
            let cell = StoredCell::deserialize(self, &root_id, &val)?;
            #[cfg(feature = "telemetry")]
            {
                self.telemetry
                    .stored_cells
                    .update(self.allocated.storage_cells.load(Ordering::Relaxed));
                self.telemetry.loaded_cells_from_db.update(1);
                self.telemetry.load_cell_from_db_time_nanos.update(now.elapsed().as_nanos() as u64);
            }
            return Ok(Cell::with_cell_impl(cell));
        }

        let mut guard = self.cells_counters.as_ref().map(|m| m.lock());
        let mut cells_counters: Option<&mut CellsCounters> = guard.as_deref_mut();
        #[cfg(feature = "telemetry")]
        self.telemetry
            .cached_cells_counters
            .update(cells_counters.as_ref().map(|c| c.len()).unwrap_or_default() as u64);

        let now = std::time::Instant::now();
        let counters_cf = self.counters_cf()?;
        let mut visited = fnv::FnvHashMap::default();
        let _ = self.save_cells_recursive(
            &root_cell,
            &mut visited,
            &root_id,
            check_stop,
            &mut cells_counters,
            &counters_cf,
        )?;
        let cells_traverse_time = now.elapsed().as_micros();

        let now2 = std::time::Instant::now();
        let mut wrote_cells = 0;
        #[cfg(feature = "telemetry")]
        let wrote_counters = visited.len();
        let mut transaction = rocksdb::WriteBatch::default();
        for (id, vc) in visited.iter() {
            // cell
            if let Some(data) = vc.serialize_cell()? {
                transaction.put_cf(&cells_cf, id.as_slice(), &data);
                wrote_cells += 1;
            }

            // counter
            transaction.put_cf(&counters_cf, id.as_slice(), vc.serialize_counter());
        }
        let tr_build_time = now2.elapsed().as_micros();

        let now3 = Instant::now();
        self.db.write(transaction)?;
        #[cfg(feature = "telemetry")]
        if !visited.is_empty() {
            self.telemetry.boc_db_element_write_nanos.update(
                now3.elapsed().as_nanos() as u64 / (wrote_cells as u64 + wrote_counters as u64),
            );
        }
        let tr_commit_time = now3.elapsed().as_micros();

        let now4 = Instant::now();
        for (id, _) in visited.iter() {
            let mut stack = vec![id.clone()];
            while let Some(id) = stack.pop() {
                if let Some(removed) = self.storing_cells.remove(&id) {
                    log::trace!(
                        target: TARGET,
                        "DynamicBocDb::save_boc  {:x}  cell removed from storing_cells", id
                    );
                    let _storing_cells_count =
                        self.storing_cells_count.fetch_sub(1, Ordering::Relaxed);
                    #[cfg(feature = "telemetry")]
                    self.telemetry.storing_cells.update(_storing_cells_count - 1);

                    for i in 0..removed.val().references_count() {
                        stack.push(removed.val().reference_repr_hash(i)?);
                    }
                }
            }
        }
        let storing_cells_cleanup_time = now4.elapsed().as_micros();

        let saved_root = if let Some(c) = visited.get(&root_id).and_then(|vc| vc.cell()) {
            c.clone()
        } else {
            // only if the root cell was already saved (just updated counter) - we need to load it here
            self.load_cell(&root_id, true)?
        };

        let updated = visited.len() - wrote_cells;
        let total_time = now.elapsed().as_micros() as u64;
        #[cfg(feature = "telemetry")]
        {
            self.telemetry.stored_new_cells.update(wrote_cells as u64);
            self.telemetry.updated_counters.update((wrote_counters - wrote_cells) as u64);
            self.telemetry.save_boc_total_micros.update(total_time);
            self.telemetry.save_boc_traverse_micros.update(cells_traverse_time as u64);
            self.telemetry.save_boc_tr_build_micros.update(tr_build_time as u64);
            self.telemetry.save_boc_commit_micros.update(tr_commit_time as u64);
            self.telemetry.save_boc_cleanup_micros.update(storing_cells_cleanup_time as u64);
        }

        log::debug!(
            target: TARGET, "DynamicBocDb::save_boc  {:x}  created {}  updated {}  TIME: {} (tr:{}|blt:{}|cmt:{}|scc:{})",
            root_id, wrote_cells, updated, total_time, cells_traverse_time, tr_build_time,
            tr_commit_time, storing_cells_cleanup_time
        );

        Ok(saved_root)
    }

    pub fn fill_counters(&self, check_stop: &dyn Fn() -> bool) -> Result<()> {
        let mutex = self
            .cells_counters
            .as_ref()
            .ok_or_else(|| error!("INTERNAL ERROR: fill_counters called without counters cache"))?;
        let now = Instant::now();
        let mut cells_counters = mutex.lock();
        if !cells_counters.is_empty() {
            fail!("INTERNAL ERROR: fill_counters called with already filled counters cache");
        }
        let counters_cf = self.counters_cf()?;
        for kv in self.db.iterator_cf(&counters_cf, rocksdb::IteratorMode::Start) {
            let (key, value) = kv?;
            let cell_id = UInt256::from_slice(key.as_ref());
            let counter = Cursor::new(value).read_le_u32()?;
            cells_counters.insert(cell_id, counter);
            let len = cells_counters.len();
            if len % 1_000_000 == 0 {
                let time = now.elapsed().as_millis() as usize;
                if time > 0 {
                    log::info!(
                        target: TARGET,
                        "DynamicBocDb::fill_counters  processed {} ({} items/sec)",
                        len, len * 1000 / time
                    );
                }
            }
            if check_stop() {
                log::warn!(target: TARGET, "DynamicBocDb::fill_counters  STOPPED");
                return Ok(());
            }
        }
        let time = now.elapsed().as_secs();
        log::info!(
            target: TARGET,
            "DynamicBocDb::fill_counters  processed {} in {} sec, speed: {} items/sec",
            cells_counters.len(), time, cells_counters.len() / time as usize
        );
        Ok(())
    }

    // Is not thread-safe!
    pub fn delete_boc(
        self: &Arc<Self>,
        root_cell_id: &UInt256,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        log::debug!(target: TARGET, "DynamicBocDb::delete_boc  {:x}", root_cell_id);

        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let mut visited = fnv::FnvHashMap::default();
        let mut guard = self.cells_counters.as_ref().map(|m| m.lock());
        let cells_counters: Option<&mut CellsCounters> = guard.as_deref_mut();
        #[cfg(feature = "telemetry")]
        self.telemetry
            .cached_cells_counters
            .update(cells_counters.as_ref().map(|c| c.len()).unwrap_or_default() as u64);
        self.delete_cells_recursive(
            root_cell_id,
            &mut visited,
            root_cell_id,
            check_stop,
            cells_counters,
        )?;
        #[cfg(feature = "telemetry")]
        let traverse_time = now.elapsed().as_micros();

        #[cfg(feature = "telemetry")]
        let now2 = std::time::Instant::now();
        let cells_cf = self.cells_cf()?;
        let counters_cf = self.counters_cf()?;
        let mut deleted = 0;
        let mut transaction = rocksdb::WriteBatch::default();
        for (id, cell) in visited.iter() {
            let counter = cell.parents_count();
            if counter == 0 {
                transaction.delete_cf(&cells_cf, id.as_slice());
                // if there is no counter with the key, then it will be just ignored
                transaction.delete_cf(&counters_cf, id.as_slice());
                deleted += 1;
            } else {
                transaction.put_cf(&counters_cf, id.as_slice(), counter.to_le_bytes());

                // update old format cell
                if let Some(cell) = cell.serialize_cell()? {
                    transaction.put(id, &cell);
                }
            }
        }
        #[cfg(feature = "telemetry")]
        let tr_build_time = now2.elapsed().as_micros();

        #[cfg(feature = "telemetry")]
        let now3 = Instant::now();
        self.db.write(transaction)?;

        #[cfg(feature = "telemetry")]
        let tr_commit_time = now3.elapsed().as_micros();
        #[cfg(feature = "telemetry")]
        let total_time = now.elapsed().as_micros() as u64;

        let updated = visited.len() - deleted;
        #[cfg(feature = "telemetry")]
        if !visited.is_empty() {
            self.telemetry
                .boc_db_element_write_nanos
                .update(now3.elapsed().as_nanos() as u64 / (visited.len() as u64 + deleted as u64));
            self.telemetry.deleted_cells.update(deleted as u64);
            self.telemetry.updated_counters.update(updated as u64);
            self.telemetry.delete_boc_total_micros.update(total_time);
            self.telemetry.delete_boc_traverse_micros.update(traverse_time as u64);
            self.telemetry.delete_boc_tr_build_micros.update(tr_build_time as u64);
            self.telemetry.delete_boc_commit_micros.update(tr_commit_time as u64);
        }

        #[cfg(feature = "telemetry")]
        log::debug!(
            target: TARGET,
            "DynamicBocDb::delete_boc  {:x}  deleted {}  updated {}  TIME: {} (tr:{}|blt:{}|cmt:{})",
            root_cell_id, deleted, updated, total_time, traverse_time, tr_build_time, tr_commit_time
        );
        #[cfg(not(feature = "telemetry"))]
        log::debug!(
            target: TARGET,
            "DynamicBocDb::delete_boc  {:x}  deleted {}  updated {}",
            root_cell_id, deleted, updated
        );
        Ok(())
    }

    pub(crate) fn load_cell(self: &Arc<Self>, cell_id: &UInt256, panic: bool) -> Result<Cell> {
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        if let Some(cell) = self.cell_cache.get(cell_id) {
            #[cfg(feature = "telemetry")]
            {
                self.telemetry.cell_cache_hits.update(1);
                self.telemetry
                    .load_cell_from_cache_time_nanos
                    .update(now.elapsed().as_nanos() as u64);
            }
            return Ok(cell);
        }
        #[cfg(feature = "telemetry")]
        self.telemetry.cell_cache_misses.update(1);
        let cell = self.load_cell_uncached(cell_id, panic)?;
        #[cfg(feature = "telemetry")]
        let now_insert = Instant::now();
        self.cell_cache.insert(cell_id.clone(), cell.clone());
        #[cfg(feature = "telemetry")]
        {
            self.telemetry
                .store_cell_to_cache_time_nanos
                .update(now_insert.elapsed().as_nanos() as u64);
            self.telemetry.cell_cache_len.update(self.cell_cache.len() as u64);
        }
        Ok(cell)
    }

    pub(crate) fn load_cell_uncached(
        self: &Arc<Self>,
        cell_id: &UInt256,
        panic: bool,
    ) -> Result<Cell> {
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let storage_cell_data = match self.db.get_pinned_cf(&self.cells_cf()?, cell_id.as_slice()) {
            Ok(Some(data)) => data,
            _ => {
                if let Some(guard) = self.storing_cells.get(cell_id) {
                    log::trace!(
                        target: TARGET,
                        "DynamicBocDb::load_cell from storing_cells by id {cell_id:x}",
                    );
                    return Ok(guard.val().clone());
                }

                if !panic {
                    fail!("Can't load cell {:x} from db", cell_id);
                }

                log::error!("FATAL!");
                log::error!("FATAL! Can't load cell {:x} from db", cell_id);
                log::error!("FATAL!");

                let path = Path::new(&self.db_root_path).join(BROKEN_CELL_BEACON_FILE);
                write(path, "")?;

                std::thread::sleep(Duration::from_millis(100));
                std::process::exit(0xFF);
            }
        };

        #[cfg(feature = "telemetry")]
        let load_cell_from_db_time_nanos = now.elapsed().as_nanos() as u64;

        let storage_cell = match StoredCell::deserialize(self, cell_id, &storage_cell_data) {
            Ok(cell) => Arc::new(cell),
            Err(e) => {
                if !panic {
                    fail!("Can't deserialize cell {:x} from db, error: {:?}", cell_id, e);
                }

                log::error!("FATAL!");
                log::error!(
                    "FATAL! Can't deserialize cell {:x} from db, data: {}, error: {:?}",
                    cell_id,
                    hex::encode(&storage_cell_data),
                    e
                );
                log::error!("FATAL!");

                let path = Path::new(&self.db_root_path).join(BROKEN_CELL_BEACON_FILE);
                write(path, "")?;

                std::thread::sleep(Duration::from_millis(100));
                std::process::exit(0xFF);
            }
        };

        #[cfg(feature = "telemetry")]
        {
            self.telemetry
                .stored_cells
                .update(self.allocated.storage_cells.load(Ordering::Relaxed));
            self.telemetry.load_cell_from_db_time_nanos.update(load_cell_from_db_time_nanos);
            self.telemetry.loaded_cells_from_db.update(1);
        }

        log::trace!(
            target: TARGET,
            "DynamicBocDb::load_cell from DB id {cell_id:x}"
        );

        Ok(Cell::with_cell_impl_arc(storage_cell))
    }

    pub(crate) fn allocated(&self) -> &StorageAlloc {
        &self.allocated
    }

    fn cells_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(&self.cells_cf_name)
            .ok_or_else(|| error!("Can't get `{}` cf handle", self.cells_cf_name))
    }

    fn counters_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(&self.counters_cf_name)
            .ok_or_else(|| error!("Can't get `{}` cf handle", self.counters_cf_name))
    }

    // This method minimizes number of DB queries by checking internal cell type (storage or not).
    // Idea is the following:
    // 1) Traverse cells recursively from root to leaves
    // 2) For each reference check if it is maybe new
    //    (is not a storage or was not visited during this save operation)
    // 3) If at least one reference is new, then this cell is definitely new too,
    //    so we DO NOT NEED to query DB for its counter
    // Returns true if the cell is new (not existing in DB and not visited during this save operation), false otherwise.
    fn save_cells_recursive(
        self: &Arc<DynamicBocDb>,
        cell: &Cell,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
        root_id: &UInt256,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
        cells_counters: &mut Option<&mut CellsCounters>,
        counters_cf: &impl rocksdb::AsColumnFamilyRef,
    ) -> Result<(bool, Option<u32>)> {
        check_stop()?;

        let cell_id = cell.repr_hash();

        if cell.is::<StoredCell>() {
            return Ok((false, None));
        }

        let mut is_new_cell = false;
        let mut ref_verdicts = [(false, None); MAX_REFERENCES_COUNT];
        for i in 0..cell.references_count() {
            if visited.contains_key(&cell.reference_repr_hash(i)?) {
                // Reference is visited during this save operation, so it is new
                ref_verdicts[i] = (false, None);
            } else {
                let reference = cell.reference(i)?;
                ref_verdicts[i] = self.save_cells_recursive(
                    &reference,
                    visited,
                    root_id,
                    check_stop,
                    cells_counters,
                    counters_cf,
                )?;
                if ref_verdicts[i].0 {
                    is_new_cell = true;
                }
            }
        }

        if !is_new_cell {
            // This cell is possibly existing
            if let Some(c) = cells_counters.as_ref().and_then(|c| c.get(&cell_id)) {
                // Cell is existing
                return Ok((false, Some(*c)));
            }
            #[cfg(feature = "telemetry")]
            let now = Instant::now();
            if let Some(raw) = self.db.get_pinned_cf(counters_cf, cell_id.as_slice())? {
                // Cell is existing
                #[cfg(feature = "telemetry")]
                {
                    self.telemetry.load_counter_time_nanos.update(now.elapsed().as_nanos() as u64);
                    self.telemetry.loaded_counters.update(1);
                }
                let mut reader = Cursor::new(raw);
                return Ok((false, Some(reader.read_le_u32()?)));
            }
        }

        // This cell is definitely new

        // Update counters for old (existing) children
        for i in 0..cell.references_count() {
            if !ref_verdicts[i].0 {
                let ref_hash = cell.reference_repr_hash(i)?;

                if let Some(counter) = ref_verdicts[i].1 {
                    // If we already know counter - just update, do not query DB second time.
                    if let Some(counters) = cells_counters.as_mut() {
                        counters.insert(ref_hash.clone(), counter + 1);
                    }
                    match visited.entry(ref_hash.clone()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.get_mut().inc_parents_count()?;
                            log::trace!(
                                target: TARGET,
                                "DynamicBocDb::save_cells_recursive  {:x}  update visited {}  root_cell_id {:x}",
                                ref_hash, counter + 1, root_id
                            );
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(VisitedCell::with_counter(counter + 1));
                            log::trace!(
                                target: TARGET,
                                "DynamicBocDb::save_cells_recursive  {:x}  update counter {}  root_cell_id {:x}",
                                ref_hash, counter + 1, root_id
                            );
                        }
                    }
                } else {
                    // This call will query DB for counter.
                    self.try_update_cell(
                        counters_cf,
                        &ref_hash,
                        visited,
                        root_id,
                        cells_counters,
                        |visited_cell| visited_cell.inc_parents_count(),
                        "DynamicBocDb::save_cells_recursive",
                    )?;
                }
            }
        }

        // Add this cell as new
        let c = VisitedCell::with_new_cell(cell.clone());
        visited.insert(cell_id.clone(), c);
        if let Some(counters) = cells_counters.as_mut() {
            counters.insert(cell_id.clone(), 1);
        }
        log::trace!(
            target: TARGET,
            "DynamicBocDb::save_cells_recursive  {:x}  new cell  root_cell_id {:x}",
            cell_id, root_id
        );

        Ok((true, None))
    }

    fn save_one_cell(
        self: &Arc<Self>,
        cell: Cell,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
        root_id: &UInt256,
        cells_counters: &mut Option<&mut CellsCounters>,
    ) -> Result<()> {
        let counters_cf = self.counters_cf()?;

        let cell_id = cell.repr_hash();

        let (counter, _cell) = self.try_update_cell(
            &counters_cf,
            &cell_id,
            visited,
            root_id,
            cells_counters,
            |visited_cell| visited_cell.inc_parents_count(),
            "DynamicBocDb::save_cells_recursive",
        )?;
        if counter.is_none() {
            // New cell.
            let c = VisitedCell::with_new_cell(cell.clone());
            visited.insert(cell_id.clone(), c);
            if let Some(counters) = cells_counters.as_mut() {
                counters.insert(cell_id.clone(), 1);
            }
            log::trace!(
                target: TARGET,
                "DynamicBocDb::save_one_cell  {:x}  new cell  root_cell_id {:x}",
                cell_id, root_id
            );

            for i in 0..cell.references_count() {
                let ref_hash = cell.reference_repr_hash(i)?;

                let (counter, _) = self.try_update_cell(
                    &counters_cf,
                    &ref_hash,
                    visited,
                    root_id,
                    cells_counters,
                    |visited_cell| visited_cell.inc_parents_count(),
                    "DynamicBocDb::save_cells_recursive",
                )?;
                if counter.is_none() {
                    fail!("save_one_cell supports only cell with all references already saved");
                }
            }
        }
        Ok(())
    }

    fn delete_cells_recursive(
        self: &Arc<Self>,
        cell_id: &UInt256,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
        root_id: &UInt256,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
        mut cells_counters: Option<&mut CellsCounters>,
    ) -> Result<()> {
        let counters_cf = self.counters_cf()?;
        let mut stack = vec![cell_id.clone()];
        while let Some(cell_id) = stack.pop() {
            check_stop()?;

            if let (Some(counter), cell) = self.try_update_cell(
                &counters_cf,
                &cell_id,
                visited,
                root_id,
                &mut cells_counters,
                |visited_cell| visited_cell.dec_parents_count(),
                "DynamicBocDb::delete_cells_recursive",
            )? {
                if counter == 0 {
                    if let Some(counters) = cells_counters.as_mut() {
                        counters.remove(&cell_id);
                    }

                    let cell = if let Some(c) = cell {
                        c
                    } else {
                        match self.load_cell(&cell_id, true) {
                            Ok(cell) => cell,
                            Err(e) => {
                                log::warn!("DynamicBocDb::delete_cells_recursive  {:?}", e);
                                continue;
                            }
                        }
                    };

                    for i in 0..cell.references_count() {
                        stack.push(cell.reference_repr_hash(i)?);
                    }
                }
            } else {
                log::warn!(
                    "DynamicBocDb::delete_cells_recursive  unknown cell with id {:x}  root_cell_id {:x}",
                    cell_id, root_id
                );
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn try_update_cell(
        self: &Arc<Self>,
        counters_cf: &impl rocksdb::AsColumnFamilyRef,
        cell_id: &UInt256,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
        root_id: &UInt256,
        cells_counters: &mut Option<&mut CellsCounters>,
        update_cell: impl Fn(&mut VisitedCell) -> Result<u32>,
        op_name: &str,
    ) -> Result<(Option<u32>, Option<Cell>)> {
        if let Some(visited_cell) = visited.get_mut(cell_id) {
            // Cell was already updated while this operation, just update counter
            let new_counter = update_cell(visited_cell)?;
            if let Some(counters) = cells_counters {
                let counter = counters.get_mut(cell_id).ok_or_else(|| {
                    error!(
                        "INTERNAL ERROR: cell from 'visited' is not presented in `cells_counters`"
                    )
                })?;
                *counter = new_counter;
            }
            log::trace!(
                target: TARGET,
                "{}  {:x}  update visited {}  root_cell_id {:x}",
                op_name, cell_id, new_counter, root_id
            );
            return Ok((Some(new_counter), visited_cell.cell().cloned()));
        }

        if let Some(counter) = cells_counters.as_mut().and_then(|cc| cc.get_mut(cell_id)) {
            // Cell's counter is in cache - update it

            let mut visited_cell = VisitedCell::with_counter(*counter);
            *counter = update_cell(&mut visited_cell)?;
            visited.insert(cell_id.clone(), visited_cell);
            log::trace!(
                target: TARGET,
                "{}  {:x}  update counter {}  root_cell_id {:x}",
                op_name, cell_id, counter, root_id
            );

            return Ok((Some(*counter), None));
        }

        if cells_counters.is_none() {
            #[cfg(feature = "telemetry")]
            let now = Instant::now();
            if let Some(counter_raw) = self.db.get_pinned_cf(counters_cf, cell_id.as_slice())? {
                // Cell's counter is in DB - load it and update

                #[cfg(feature = "telemetry")]
                {
                    self.telemetry.load_counter_time_nanos.update(now.elapsed().as_nanos() as u64);
                    self.telemetry.loaded_counters.update(1);
                }

                let mut visited_cell = VisitedCell::with_raw_counter(&counter_raw)?;
                let counter = update_cell(&mut visited_cell)?;
                visited.insert(cell_id.clone(), visited_cell);
                if let Some(counters) = cells_counters.as_mut() {
                    counters.insert(cell_id.clone(), counter);
                }
                log::trace!(
                    target: TARGET,
                    "{}  {:x}  load counter {}  root_cell_id {:x}",
                    op_name, cell_id, counter, root_id
                );

                return Ok((Some(counter), None));
            }
        }

        Ok((None, None))
    }
}

impl CellsFactory for DynamicBocDb {
    fn create_cell(self: Arc<Self>, builder: BuilderData) -> Result<Cell> {
        let cell = StoringCell::with_cell(&*builder.into_cell()?, &self)?;
        let cell = Cell::with_cell_impl(cell);
        let repr_hash = cell.repr_hash();

        let mut result_cell = None;

        let result = self.storing_cells.insert_with(repr_hash, |_, inserted, found| {
            if let Some((_, found)) = found {
                result_cell = Some(found.clone());
                lockfree::map::Preview::Discard
            } else if let Some(inserted) = inserted {
                result_cell = Some(inserted.clone());
                lockfree::map::Preview::Keep
            } else {
                result_cell = Some(cell.clone());
                lockfree::map::Preview::New(cell.clone())
            }
        });

        let result_cell = result_cell
            .ok_or_else(|| error!("INTERNAL ERROR: result_cell {:x} is None", cell.repr_hash()))?;

        match result {
            lockfree::map::Insertion::Created => {
                log::trace!(target: TARGET, "DynamicBocDb::create_cell {:x} - created new", cell.repr_hash());
                #[cfg(feature = "telemetry")]
                {
                    let storing_cells_count =
                        self.storing_cells_count.fetch_add(1, Ordering::Relaxed);
                    self.telemetry.storing_cells.update(storing_cells_count + 1);
                }
            }
            lockfree::map::Insertion::Failed(_) => {
                log::trace!(target: TARGET, "DynamicBocDb::create_cell {:x} - already exists", cell.repr_hash());
            }
            lockfree::map::Insertion::Updated(old) => {
                fail!(
                    "INTERNAL ERROR: storing_cells.insert_with {:x} returned Updated({:?})",
                    cell.repr_hash(),
                    old
                )
            }
        }

        Ok(result_cell)
    }
}

// This wrapper-struct is added because it is impossible
// to implement foreign trait (CellByHashStorage) for foreign type (Arc)
pub struct CellByHashStorageAdapter {
    db: Arc<DynamicBocDb>,
    root_cells_data: ahash::HashMap<UInt256, Vec<u8>>,
}

impl CellByHashStorageAdapter {
    pub fn new(
        db: Arc<DynamicBocDb>,
        root_cell: Option<&Cell>,
        max_inmemory_cells: usize,
    ) -> Result<Self> {
        let mut root_cells_data = ahash::HashMap::default();
        if let Some(root_cell) = root_cell {
            if db.load_cell(&root_cell.repr_hash(), false).is_err() {
                let mut stack = vec![root_cell.clone()];
                while let Some(cell) = stack.pop() {
                    if root_cells_data.len() >= max_inmemory_cells {
                        fail!(
                            "Too many cells in boc to store in memory: {}, max_inmemory_cells: {}",
                            root_cells_data.len(),
                            max_inmemory_cells
                        );
                    }
                    let cell_data = StoredCell::serialize(cell.cell_impl().deref())?;
                    let cell_hash = cell.repr_hash();
                    root_cells_data.insert(cell_hash, cell_data);

                    for i in 0..cell.references_count() {
                        if db.load_cell(&cell.reference_repr_hash(i)?, false).is_err() {
                            stack.push(cell.reference(i)?);
                        }
                    }
                }
            }
        }
        Ok(Self { db, root_cells_data })
    }
}

impl CellsStorage for CellByHashStorageAdapter {
    fn load_cell(&self, hash: &UInt256) -> Result<Cell> {
        if let Ok(c) = self.db.clone().load_cell_uncached(hash, false) {
            Ok(c)
        } else if let Some(data) = self.root_cells_data.get(hash) {
            StoredCell::deserialize(&self.db, hash, data).map(Cell::with_cell_impl)
        } else {
            fail!("Can't load cell {:x} from db", hash);
        }
    }

    fn load_cell_data(
        &self,
        hash: &UInt256,
        write_hashes: bool,
        dest: &mut dyn Write,
    ) -> Result<()> {
        #[cfg(feature = "telemetry")]
        let now = std::time::Instant::now();
        if let Ok(Some(data)) = self.db.db.get_pinned_cf(&self.db.cells_cf()?, hash.as_slice()) {
            #[cfg(feature = "telemetry")]
            {
                self.db
                    .telemetry
                    .load_cell_from_db_time_nanos
                    .update(now.elapsed().as_nanos() as u64);
                self.db.telemetry.loaded_cells_from_db.update(1);
            }

            StoredCell::write_cell_data(&data, hash, write_hashes, dest)
        } else if let Some(data) = self.root_cells_data.get(hash) {
            StoredCell::write_cell_data(data, hash, write_hashes, dest)
        } else {
            fail!("Can't load cell {:x} from db", hash);
        }
    }
}

pub struct AsyncCellsStorageAdapter {
    boc_db: Arc<DynamicBocDb>,
    index: Vec<(UInt256, u16)>, // hash & depth.
    // TODO: consider using datatype which allown to store data in chunks, not single piece of memory
    cache: Arc<lockfree::map::Map<u32, Cell>>,
    sender: tokio::sync::mpsc::Sender<(u32, Cell)>,
    worker: tokio::task::JoinHandle<()>,
}

impl AsyncCellsStorageAdapter {
    pub fn new(boc_db: Arc<DynamicBocDb>, index: Vec<(UInt256, u16)>) -> Result<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<(u32, Cell)>(10_000);
        let cache = Arc::new(lockfree::map::Map::new());
        let cache_ = cache.clone();
        let boc_db_clone = boc_db.clone();

        let worker = tokio::task::spawn(async move {
            let r = tokio::task::spawn_blocking(move || -> Result<()> {
                let mut guard = boc_db_clone.cells_counters.as_ref().map(|m| m.lock());
                let mut cells_counters: Option<&mut CellsCounters> = guard.as_deref_mut();

                let cells_cf = boc_db_clone.cells_cf()?;
                let counters_cf = boc_db_clone.counters_cf()?;
                let mut visited = fnv::FnvHashMap::<UInt256, VisitedCell>::default();

                let max_len = 100_000;
                let mut indexes = Vec::with_capacity(max_len);
                let commit = |visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>| -> Result<()> {
                    let mut transaction = rocksdb::WriteBatch::default();
                    for (id, vc) in visited.iter() {
                        // cell
                        if let Some(data) = vc.serialize_cell()? {
                            transaction.put_cf(&cells_cf, id.as_slice(), &data);
                        }
                        // counter
                        transaction.put_cf(&counters_cf, id.as_slice(), vc.serialize_counter());
                    }
                    boc_db_clone.db.write(transaction)?;
                    visited.clear();
                    Ok(())
                };
                while let Some((cell_index, cell)) = receiver.blocking_recv() {
                    if visited.len() >= max_len {
                        commit(&mut visited)?;
                        while let Some(i) = indexes.pop() {
                            cache_.remove(&i);
                        }
                    }
                    let rh = cell.repr_hash();
                    boc_db_clone.save_one_cell(cell, &mut visited, &rh, &mut cells_counters)?;
                    indexes.push(cell_index);
                }
                commit(&mut visited)?;
                while let Some(i) = indexes.pop() {
                    cache_.remove(&i);
                }
                Ok(())
            })
            .await;

            if let Err(e) = r {
                log::error!("AsyncCellsStorageAdapter worker: {:?}", e);
            }
        });

        Ok(Self { boc_db, index, cache, sender, worker })
    }

    pub async fn finish(self) -> Result<Vec<(UInt256, u16)>> {
        drop(self.sender);
        self.worker.await?;
        Ok(self.index)
    }
}

impl CellsTempStorage for AsyncCellsStorageAdapter {
    fn load_hash_and_depth(&self, index: u32) -> Result<(UInt256, u16)> {
        if index as usize >= self.index.len() {
            fail!("AsyncCellsStorageAdapter::load_hash_and_depth index out of bounds: {}", index);
        }
        let (hash, depth) = self.index[index as usize].clone();
        if hash == UInt256::default() {
            fail!("AsyncCellsStorageAdapter::load_hash_and_depth attempt to load uninitialized cell hash");
        }
        Ok((hash, depth))
    }

    fn load_cell(&self, index: u32) -> Result<Cell> {
        if let Some(guard) = self.cache.get(&index) {
            Ok(guard.val().clone())
        } else {
            let (hash, _) = self.load_hash_and_depth(index)?;
            let cell = self.boc_db.clone().load_cell(&hash, false)?;
            self.cache.insert(index, cell.clone());
            Ok(cell)
        }
    }

    fn store_simple_cell(
        &mut self,
        index: u32,
        data: CellData,
        refs: &[(UInt256, u16)],
    ) -> Result<()> {
        if index as usize >= self.index.len() {
            fail!("AsyncCellsStorageAdapter::store_simple_cell index out of bounds: {}", index);
        }
        if data.level() != 0 {
            fail!("AsyncCellsStorageAdapter::store_simple_cell supports only zero level cells");
        }
        self.index[index as usize] = (data.hash(0), data.depth(0));
        let cell = Cell::with_cell_impl(StoredCell::with_cell_data(data, refs, &self.boc_db)?);
        self.cache.insert(index, cell.clone());
        self.sender.blocking_send((index, cell))?;
        Ok(())
    }

    fn store_cell(&mut self, index: u32, cell: &Cell) -> Result<()> {
        if index as usize >= self.index.len() {
            fail!("AsyncCellsStorageAdapter::store_simple_cell index out of bounds: {}", index);
        }
        self.index[index as usize] = (cell.repr_hash(), cell.repr_depth());
        self.cache.insert(index, cell.clone());
        self.sender.blocking_send((index, cell.clone()))?;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        self.index = vec![];
        // self.cache.clear();
        Ok(())
    }
}
