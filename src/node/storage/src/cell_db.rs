/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    db::rocksdb::RocksDb, shardstate_db_async::CellsDbConfig, types::deserialize_stored_cell,
    StorageAlloc, TARGET,
};
#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "telemetry")]
use std::time::Instant;
use std::{
    io::Write,
    sync::{Arc, OnceLock},
};
use ton_block::{
    error, fail, merkle_update::CellsFactory, BuilderData, Cell, CellLoader, CellsStorage, Result,
    UInt256,
};

pub const BROKEN_CELL_BEACON_FILE: &str = "ton_node.broken_cell";

pub struct CellDb {
    db: Arc<RocksDb>,
    cells_cf_name: String,
    storing_cells: Arc<lockfree::map::Map<UInt256, Cell>>,
    #[cfg(feature = "telemetry")]
    storing_cells_count: AtomicU64,
    #[cfg(feature = "telemetry")]
    storing_cells_bytes: AtomicU64,
    cell_cache: quick_cache::sync::Cache<UInt256, Cell>,
    /// Loader for cells loaded from DB (StoredCell)
    stored_loader: OnceLock<CellLoader>,
    /// Loader for cells being stored (StoringCell)
    storing_loader: OnceLock<CellLoader>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<StorageTelemetry>,
    allocated: Arc<StorageAlloc>,
}

impl CellDb {
    pub fn with_db(
        db: Arc<RocksDb>,
        cell_db_cf: &str,
        config: &CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Self> {
        if db.cf_handle(cell_db_cf).is_none() {
            let (options, cache) = Self::build_cf_options(config.cells_cache_size_bytes);
            db.create_cf(cell_db_cf, &options)?;
            db.register_cache(cache);
        }
        Ok(Self {
            db,
            cells_cf_name: cell_db_cf.to_string(),
            storing_cells: Arc::new(lockfree::map::Map::new()),
            #[cfg(feature = "telemetry")]
            storing_cells_count: AtomicU64::new(0),
            #[cfg(feature = "telemetry")]
            storing_cells_bytes: AtomicU64::new(0),
            cell_cache: quick_cache::sync::Cache::new(config.cells_lru_cache_capacity),
            stored_loader: OnceLock::new(),
            storing_loader: OnceLock::new(),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        })
    }

    pub fn build_cf_options(cache_size: u64) -> (rocksdb::Options, rocksdb::Cache) {
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

        // Use two-level index with partitioned filters so that only the relevant
        // partition of index/filter is loaded into the block cache on each lookup,
        // instead of the full index/filter block for the entire SST file.
        block_opts.set_index_type(rocksdb::BlockBasedIndexType::TwoLevelIndexSearch);
        block_opts.set_partition_filters(true);

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
        options.set_write_buffer_size(64 * 1024 * 1024);

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

        (options, cache)
    }

    pub fn db(&self) -> &Arc<RocksDb> {
        &self.db
    }

    pub fn allocated(&self) -> &StorageAlloc {
        &self.allocated
    }

    pub fn cells_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(&self.cells_cf_name)
            .ok_or_else(|| error!("Can't get `{}` cf handle", self.cells_cf_name))
    }

    pub fn storing_cells(&self) -> &Arc<lockfree::map::Map<UInt256, Cell>> {
        &self.storing_cells
    }

    #[cfg(feature = "telemetry")]
    pub fn telemetry(&self) -> &Arc<StorageTelemetry> {
        &self.telemetry
    }

    pub fn stored_loader(self: &Arc<Self>) -> &CellLoader {
        &self.stored_loader.get_or_init(|| {
            let weak = Arc::downgrade(self);
            Arc::new(move |hash: &UInt256| -> Result<Cell> {
                let db = weak.upgrade().ok_or_else(|| error!("BocDb is dropped"))?;
                db.load_cell(hash)
            })
        })
    }

    pub fn storing_loader(self: &Arc<Self>) -> &CellLoader {
        &self.storing_loader.get_or_init(|| {
            let weak = Arc::downgrade(self);
            Arc::new(move |hash: &UInt256| -> Result<Cell> {
                let db = weak.upgrade().ok_or_else(|| error!("BocDb is dropped"))?;
                db.load_cell(hash)
            })
        })
    }

    /// Check if a cell was created with this DynamicBocDb's stored_loader
    pub fn is_stored_cell(&self, cell: &Cell) -> bool {
        if let (Some(cell_ptr), Some(loader)) = (cell.loader_data_ptr(), self.stored_loader.get()) {
            cell_ptr == Arc::as_ptr(loader) as *const ()
        } else {
            false
        }
    }

    /// If root cell already exists in DB, load and return it. Otherwise return None.
    pub fn try_load_existing_root(
        self: &Arc<Self>,
        root_id: &UInt256,
        cells_cf: &impl rocksdb::AsColumnFamilyRef,
    ) -> Result<Option<Cell>> {
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        if let Some(val) = self.db.get_pinned_cf(cells_cf, root_id.as_slice())? {
            let cell = deserialize_stored_cell(root_id, &val, self.stored_loader())?;
            #[cfg(feature = "telemetry")]
            {
                self.telemetry.loaded_cells_from_db.update(1);
                self.telemetry.load_cell_from_db_time_nanos.update(now.elapsed().as_nanos() as u64);
            }
            Ok(Some(cell))
        } else {
            Ok(None)
        }
    }

    /// Remove saved cell hashes from the storing_cells in-memory cache.
    pub fn cleanup_storing_cells<'a>(&self, saved_ids: impl Iterator<Item = &'a UInt256>) {
        for id in saved_ids {
            let mut stack = vec![id.clone()];
            while let Some(id) = stack.pop() {
                if let Some(removed) = self.storing_cells.remove(&id) {
                    log::trace!(
                        target: TARGET,
                        "CellDb::cleanup_storing_cells  {:x}  removed from storing_cells", id
                    );
                    #[cfg(feature = "telemetry")]
                    {
                        let alloc_size = removed.val().alloc_size() as u64;
                        let _count = self.storing_cells_count.fetch_sub(1, Ordering::Relaxed);
                        let _bytes =
                            self.storing_cells_bytes.fetch_sub(alloc_size, Ordering::Relaxed);
                        self.telemetry.storing_cells.update(_count - 1);
                        self.telemetry
                            .storing_cells_bytes
                            .update(_bytes.saturating_sub(alloc_size));
                    }
                    for i in 0..removed.val().references_count() {
                        if let Ok(ref_hash) = removed.val().reference_repr_hash(i) {
                            stack.push(ref_hash);
                        }
                    }
                }
            }
        }
    }

    /// Check if a cell is present in the in-memory LRU cache.
    pub fn is_in_cache(&self, cell_id: &UInt256) -> bool {
        self.cell_cache.get(cell_id).is_some()
    }

    /// Remove a cell from the in-memory LRU cache.
    pub fn remove_from_cache(&self, cell_id: &UInt256) {
        self.cell_cache.remove(cell_id);
    }

    pub fn add_to_cache(&self, cell_id: UInt256, cell: Cell) {
        self.cell_cache.insert(cell_id, cell);
        #[cfg(feature = "telemetry")]
        self.telemetry.cell_cache_len.update(self.cell_cache.len() as u64);
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        if let Ok(cf) = self.cells_cf() {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start).count()
        } else {
            0
        }
    }

    pub(crate) fn load_cell(self: &Arc<Self>, cell_id: &UInt256) -> Result<Cell> {
        if let Some(cell) = self.cell_cache.get(cell_id) {
            #[cfg(feature = "telemetry")]
            self.telemetry.cell_cache_hits.update(1);
            Ok(cell)
        } else {
            #[cfg(feature = "telemetry")]
            self.telemetry.cell_cache_misses.update(1);

            let cell = self.load_cell_uncached(cell_id)?;

            if self.is_stored_cell(&cell) {
                // do not cache storing cells, they are already in storing_cells map
                self.add_to_cache(cell_id.clone(), cell.clone());
            }

            Ok(cell)
        }
    }

    fn load_cell_uncached(self: &Arc<Self>, cell_id: &UInt256) -> Result<Cell> {
        // Check storing_cells before the DB. save_boc inserts the cell via
        // create_cell before traversal and removes it from storing_cells strictly
        // after the DB commit. If we checked the DB first, a reader could hit DB
        // before the commit (miss) and then hit storing_cells after the cleanup
        // (also miss), even though the cell is already in the DB by then.
        if let Some(guard) = self.storing_cells.get(cell_id) {
            log::trace!(
                target: TARGET,
                "CellDb::load_cell from storing_cells by id {cell_id:x}",
            );
            return Ok(guard.val().clone());
        }

        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let storage_cell_data =
            match self.db.get_pinned_cf(&self.cells_cf()?, cell_id.as_slice())? {
                Some(data) => data,
                None => fail!("Can't load cell {:x} from db", cell_id),
            };

        #[cfg(feature = "telemetry")]
        let load_cell_from_db_time_nanos = now.elapsed().as_nanos() as u64;

        let storage_cell =
            match deserialize_stored_cell(cell_id, &storage_cell_data, self.stored_loader()) {
                Ok(cell) => cell,
                Err(e) => {
                    fail!("Can't deserialize cell {:x} from db, error: {:?}", cell_id, e);
                }
            };

        #[cfg(feature = "telemetry")]
        {
            self.telemetry.load_cell_from_db_time_nanos.update(load_cell_from_db_time_nanos);
            self.telemetry.loaded_cells_from_db.update(1);
        }

        log::trace!(
            target: TARGET,
            "CellDb::load_cell from DB id {cell_id:x}"
        );

        Ok(storage_cell)
    }
}

impl CellsFactory for CellDb {
    fn create_cell(self: Arc<Self>, builder: BuilderData) -> Result<Cell> {
        let built_cell = builder.into_cell()?;
        let cell = Cell::with_cell_and_loader(built_cell, self.storing_loader(), None)?;
        let repr_hash = cell.repr_hash().clone();

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
                log::trace!(target: TARGET, "CellDb::create_cell {:x} - created new", cell.repr_hash());
                #[cfg(feature = "telemetry")]
                {
                    let cell_size = cell.alloc_size() as u64;
                    let storing_cells_count =
                        self.storing_cells_count.fetch_add(1, Ordering::Relaxed);
                    let storing_cells_bytes =
                        self.storing_cells_bytes.fetch_add(cell_size, Ordering::Relaxed);
                    self.telemetry.storing_cells.update(storing_cells_count + 1);
                    self.telemetry.storing_cells_bytes.update(storing_cells_bytes + cell_size);
                }
            }
            lockfree::map::Insertion::Failed(_) => {
                log::trace!(target: TARGET, "CellDb::create_cell {:x} - already exists", cell.repr_hash());
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
    fn create_lazy_load_cell(self: Arc<Self>, pruned: &Cell, merkle_depth: u8) -> Result<Cell> {
        Cell::lazy_from_pruned(pruned, self.stored_loader().clone(), merkle_depth)
    }
}

// This wrapper-struct is added because it is impossible
// to implement foreign trait (CellByHashStorage) for foreign type (Arc)
pub struct CellByHashStorageAdapter {
    db: Arc<CellDb>,
    root_cells: ahash::HashMap<UInt256, Cell>,
}

impl CellByHashStorageAdapter {
    pub fn new(
        db: Arc<CellDb>,
        root_cell: Option<&Cell>,
        max_inmemory_cells: usize,
    ) -> Result<Self> {
        let mut root_cells = ahash::HashMap::default();
        if let Some(root_cell) = root_cell {
            if db.load_cell(&root_cell.repr_hash()).is_err() {
                let mut stack = vec![root_cell.clone()];
                while let Some(cell) = stack.pop() {
                    if root_cells.len() >= max_inmemory_cells {
                        fail!(
                            "Too many cells in boc to store in memory: {}, max_inmemory_cells: {}",
                            root_cells.len(),
                            max_inmemory_cells
                        );
                    }
                    let cell_hash = cell.repr_hash().clone();
                    root_cells.insert(cell_hash, cell.clone());

                    for i in 0..cell.references_count() {
                        if db.load_cell(&cell.reference_repr_hash(i)?).is_err() {
                            stack.push(cell.reference(i)?);
                        }
                    }
                }
            }
        }
        Ok(Self { db, root_cells })
    }
}

impl CellsStorage for CellByHashStorageAdapter {
    fn load_cell(&self, hash: &UInt256) -> Result<Cell> {
        if let Ok(c) = self.db.clone().load_cell_uncached(hash) {
            Ok(c)
        } else if let Some(cell) = self.root_cells.get(hash) {
            Ok(cell.clone())
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
        let now = Instant::now();
        if let Ok(Some(data)) = self.db.db.get_pinned_cf(&self.db.cells_cf()?, hash.as_slice()) {
            #[cfg(feature = "telemetry")]
            {
                self.db
                    .telemetry
                    .load_cell_from_db_time_nanos
                    .update(now.elapsed().as_nanos() as u64);
                self.db.telemetry.loaded_cells_from_db.update(1);
            }

            deserialize_stored_cell(hash, &data, self.db.stored_loader())?
                .write_data(write_hashes, dest)
        } else if let Some(cell) = self.root_cells.get(hash) {
            cell.write_data(write_hashes, dest)
        } else {
            fail!("Can't load cell {:x} from db", hash);
        }
    }
}
