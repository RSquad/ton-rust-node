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
    cell_db::CellDb, db::rocksdb::RocksDb, shardstate_db_async::CellsDbConfig,
    types::serialize_stored_cell, StorageAlloc, TARGET,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::Instant,
};
use ton_block::{
    error, fail, Cell, CellLoader, CellsFactory, CellsTempStorage, Result, UInt256, MAX_LEVEL,
    MAX_REFERENCES_COUNT,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VisitedCell {
    /// Brand new cell — not in DB before this operation.
    /// `inc` is the number of parent references added in this operation.
    /// On commit: Put cell data + Merge(inc) to counters_cf.
    New { cell: Cell, inc: u32 },
    /// Existing-in-DB cell touched by save. `inc` is the number of parent references
    /// added in this operation. On commit: Merge(inc) to counters_cf — no base value needed.
    Inc { inc: u32 },
    /// Existing-in-DB cell touched by delete. `refs` is the absolute
    /// new counter after the decrements applied during traversal.
    /// On commit: Put(refs) if > 0, Delete if 0.
    Updated { refs: u32 },
}

impl VisitedCell {
    /// Placeholder used during save traversal for a cell known/suspected to be in DB.
    /// It will either become `Inc { inc: >0 }` via `inc()` from parent's Phase B,
    /// or be replaced by `New` if the recursion finds the cell is actually new.
    fn placeholder() -> Self {
        Self::Inc { inc: 0 }
    }

    fn with_new_cell(cell: Cell) -> Self {
        Self::New { cell, inc: 1 }
    }

    /// Increment the reference counter on the save path.
    /// Works on `New` and `Inc` (both bump `inc`).
    fn inc(&mut self) -> Result<()> {
        match self {
            Self::New { inc, .. } => {
                *inc = inc.checked_add(1).ok_or_else(|| error!("inc overflow"))?;
            }
            Self::Inc { inc } => {
                *inc = inc.checked_add(1).ok_or_else(|| error!("inc overflow"))?;
            }
            Self::Updated { .. } => fail!("inc on Updated variant"),
        }
        Ok(())
    }

    /// Decrement the reference counter on the delete path. Only valid for `Updated`.
    fn dec(&mut self) -> Result<u32> {
        match self {
            Self::Updated { refs } => {
                *refs = refs.checked_sub(1).ok_or_else(|| error!("refs underflow"))?;
                Ok(*refs)
            }
            _ => fail!("dec on non-Updated variant"),
        }
    }
}

const VISITED_MAP_CHUNKS: usize = 512;
/// Sentinel value stored in the counter cache to mean "counter is unknown
/// (e.g. evicted between save start and post-commit flush)". Never appears in DB.
const UNKNOWN_COUNTER: u32 = u32::MAX;
/// Maximum refcnt value that can be stored on disk. One short of `u32::MAX` so that
/// a valid counter is never confused with `UNKNOWN_COUNTER` after a cache reload.
const MAX_REFCNT: u32 = u32::MAX - 1;

type VisitedMap = dashmap::DashMap<UInt256, VisitedCell, ahash::RandomState>;

pub struct DynamicBocDb {
    cell_db: Arc<CellDb>,
    counters_cf_name: String,
    cell_counter_cache: quick_cache::sync::Cache<UInt256, Arc<AtomicU32>>,
    save_cells_threadpool: rayon::ThreadPool,
}

impl DynamicBocDb {
    pub(crate) fn with_db(
        db: Arc<RocksDb>,
        cell_db_cf: &str,
        counters_cf_name: &str,
        config: &CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Self> {
        let cell_db = CellDb::with_db(
            db.clone(),
            cell_db_cf,
            config,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )?;
        if db.cf_handle(counters_cf_name).is_none() {
            let (options, cache) = Self::build_counters_cf_options(config);
            db.create_cf(counters_cf_name, &options)?;
            db.register_cache(cache);
        }

        let save_cells_threadpool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("save-cells-worker-{i}"))
            .stack_size(8 * 1024 * 1024)
            .panic_handler(|payload| {
                log::error!(target: TARGET, "save-cells-worker panic: {payload:?}");
            })
            .build()?;

        Ok(Self {
            cell_db: Arc::new(cell_db),
            counters_cf_name: counters_cf_name.to_string(),
            cell_counter_cache: quick_cache::sync::Cache::new(config.counters_lru_cache_capacity),
            save_cells_threadpool,
        })
    }

    pub fn cell_db(&self) -> &Arc<CellDb> {
        &self.cell_db
    }

    pub fn build_cells_cf_options(config: &CellsDbConfig) -> (rocksdb::Options, rocksdb::Cache) {
        CellDb::build_cf_options(config.cells_cache_size_bytes)
    }

    pub fn build_counters_cf_options(config: &CellsDbConfig) -> (rocksdb::Options, rocksdb::Cache) {
        let (mut opts, cache) = CellDb::build_cf_options(config.counters_cache_size_bytes);
        opts.set_merge_operator_associative("refcnt_merge", Self::refcnt_merge);
        (opts, cache)
    }

    /// Merge operator for refcnt counters.
    /// Existing value (if any) and each operand are 4-byte little-endian u32.
    /// Result is the sum, saturated to `MAX_REFCNT`.
    fn refcnt_merge(
        _key: &[u8],
        existing: Option<&[u8]>,
        operands: &rocksdb::MergeOperands,
    ) -> Option<Vec<u8>> {
        let mut counter: u32 = match existing {
            Some(bytes) if bytes.len() == 4 => u32::from_le_bytes(bytes.try_into().unwrap()),
            Some(_) => {
                log::error!(target: TARGET, "refcnt_merge: corrupted existing value (len != 4)");
                return None;
            }
            None => 0,
        };
        for op in operands {
            if op.len() == 4 {
                let diff = u32::from_le_bytes(op.try_into().unwrap());
                let sum = counter.saturating_add(diff);
                if sum >= MAX_REFCNT {
                    log::error!(target: TARGET, "CRITICAL: refcnt_merge: counter saturated at MAX_REFCNT");
                    counter = MAX_REFCNT;
                } else {
                    counter = sum;
                }
            } else {
                log::error!(target: TARGET, "refcnt_merge: corrupted operand (len != 4)");
            }
        }
        Some(counter.to_le_bytes().to_vec())
    }

    pub(crate) fn load_cell(&self, cell_id: &UInt256) -> Result<Cell> {
        self.cell_db.load_cell(cell_id)
    }

    #[allow(dead_code)]
    fn allocated(&self) -> &StorageAlloc {
        self.cell_db.allocated()
    }

    pub fn cells_factory(&self) -> Arc<dyn CellsFactory> {
        self.cell_db.clone() as Arc<dyn CellsFactory>
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        if let Ok(cf) = self.counters_cf() {
            self.cell_db.db().iterator_cf(&cf, rocksdb::IteratorMode::Start).count()
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
        // Ensure stored_loader OnceLock is initialized before traversal.
        // is_stored_cell() uses stored_loader.get() (without init) to compare loader pointers.
        // Without this, is_stored_cell() always returns false when all cell loads are cache hits,
        // causing save_cells_recursive to traverse the entire tree instead of stopping at stored cells.
        let _ = self.cell_db().stored_loader();

        let root_id = root_cell.hash(MAX_LEVEL);
        log::debug!(target: TARGET, "DynamicBocDb::save_boc  {:x}", root_id);

        let cells_cf = self.cell_db.cells_cf()?;

        if let Some(existing) = self.cell_db.try_load_existing_root(&root_id, &cells_cf)? {
            log::info!(target: TARGET, "DynamicBocDb::save_boc  ALREADY EXISTS  {:x}", root_id);
            return Ok(existing);
        }

        //
        // Traverse cells
        //
        let now = std::time::Instant::now();
        let counters_cf = self.counters_cf()?;
        let visited =
            VisitedMap::with_hasher_and_shard_amount(ahash::RandomState::new(), VISITED_MAP_CHUNKS);
        let inc_error = AtomicBool::new(false);
        self.save_cells_threadpool.scope(|scope| {
            self.save_cells_parallel(
                &root_cell,
                &visited,
                &root_id,
                check_stop,
                scope,
                &inc_error,
                counters_cf.clone(),
            )
        })?;
        if inc_error.load(Ordering::Relaxed) {
            fail!("save_cells_parallel: update_child_inc failed for some child cell")
        }
        let cells_traverse_time = now.elapsed().as_micros();
        let visited = visited.into_read_only();

        // Single pass over `visited` (dashmap iteration is the expensive part).
        // During this pass:
        //   - Build the RocksDB write batch directly.
        //   - Capture the root cell if it appears as a `New` entry.
        //   - Record per-entry post-commit work into a plain Vec so subsequent
        //     iterations (counter cache flush, storing-cells cleanup) traverse
        //     a contiguous structure instead of the sharded dashmap.
        //
        // Write batch semantics:
        // - New cells: Put cell data + Merge counter (from base 0, gives absolute inc).
        // - Inc entries: Merge counter only. `inc == 0` is a leftover placeholder
        //   for a known-in-DB cell that no new parent ended up referencing —
        //   shouldn't happen often, and merging 0 is harmless.
        let now2 = std::time::Instant::now();
        let mut wrote_cells: usize = 0;
        #[cfg(feature = "telemetry")]
        let mut wrote_counters: usize = 0;
        let mut transaction = rocksdb::WriteBatch::default();
        let mut saved_root: Option<Cell> = None;
        let mut stored_refs_inc = Vec::new();
        let mut stored_cells = Vec::new();
        let mut stored_ids = Vec::new();
        for (id, vc) in visited.iter() {
            match vc {
                VisitedCell::New { cell, inc } => {
                    let data = serialize_stored_cell(cell)?;
                    transaction.put_cf(&cells_cf, id.as_slice(), &data);
                    // For brand-new cells the counters_cf has no entry, so Merge from base 0
                    // produces exactly `inc`.
                    transaction.merge_cf(&counters_cf, id.as_slice(), inc.to_le_bytes());
                    wrote_cells += 1;
                    #[cfg(feature = "telemetry")]
                    {
                        wrote_counters += 1;
                    }
                    if id == root_id {
                        saved_root = Some(cell.clone());
                    }
                    let stored_cell = Cell::with_cell_and_loader(
                        cell.clone(),
                        self.cell_db.stored_loader(),
                        None,
                    )?;
                    stored_cells.push((id.clone(), (stored_cell), *inc));
                    stored_ids.push(id.clone());
                }
                VisitedCell::Inc { inc } => {
                    if *inc > 0 {
                        transaction.merge_cf(&counters_cf, id.as_slice(), inc.to_le_bytes());
                        #[cfg(feature = "telemetry")]
                        {
                            wrote_counters += 1;
                        }
                        stored_refs_inc.push((id.clone(), *inc));
                    }
                    // It is used to cleanup storing cells. This cells may be with zero inc.
                    stored_ids.push(id.clone());
                }
                VisitedCell::Updated { .. } => {
                    fail!(
                        "DynamicBocDb::save_boc  {:x}  unexpected Updated variant for {:x}",
                        root_id,
                        id,
                    );
                }
            };
        }
        let tr_build_time = now2.elapsed().as_micros();

        //
        // Commit transaction
        //
        let now3 = Instant::now();
        self.cell_db.db().write(transaction)?;
        #[cfg(feature = "telemetry")]
        if wrote_counters > 0 {
            self.cell_db.telemetry().boc_db_element_write_nanos.update(
                now3.elapsed().as_nanos() as u64 / (wrote_cells as u64 + wrote_counters as u64),
            );
        }
        let tr_commit_time = now3.elapsed().as_micros();

        //
        // Update caches and cleanup.
        //
        let now4 = Instant::now();

        for (id, inc) in stored_refs_inc {
            if let Err(e) = self.set_cached_counter_inc(&id, inc) {
                // It is better to log and continue than fail here,
                // because the BOC is already stored in DB,
                log::error!(
                    target: TARGET,
                    "DynamicBocDb::save_boc  {:x}  update counter cache failed for {id:x}: {e}",
                    root_id
                );
            }
        }
        for (id, stored_cell, refs) in stored_cells {
            self.set_cached_counter(&id, refs);
            self.cell_db.add_to_cache(id, stored_cell);
        }
        self.cell_db.cleanup_storing_cells(stored_ids.iter());
        let storing_cells_cleanup_time = now4.elapsed().as_micros();

        let saved_root = match saved_root {
            Some(c) => c,
            // Root was already saved (counter-only update) — load it back.
            None => self.cell_db.load_cell(&root_id)?,
        };

        let updated = visited.len() - wrote_cells;
        let total_time = now.elapsed().as_micros() as u64;
        #[cfg(feature = "telemetry")]
        {
            self.cell_db.telemetry().stored_new_cells.update(wrote_cells as u64);
            self.cell_db.telemetry().updated_counters.update((wrote_counters - wrote_cells) as u64);
            self.cell_db.telemetry().save_boc_total_micros.update(total_time);
            self.cell_db.telemetry().save_boc_traverse_micros.update(cells_traverse_time as u64);
            self.cell_db.telemetry().save_boc_tr_build_micros.update(tr_build_time as u64);
            self.cell_db.telemetry().save_boc_commit_micros.update(tr_commit_time as u64);
            self.cell_db
                .telemetry()
                .save_boc_cleanup_micros
                .update(storing_cells_cleanup_time as u64);
        }

        log::debug!(
            target: TARGET, "DynamicBocDb::save_boc  {:x}  created {}  updated {}  TIME: {} (tr:{}|blt:{}|cmt:{}|scc:{})",
            root_id, wrote_cells, updated, total_time, cells_traverse_time, tr_build_time,
            tr_commit_time, storing_cells_cleanup_time
        );

        Ok(saved_root)
    }

    // Is not thread-safe!
    /// `extra_ops` is appended to the same batch and committed atomically
    /// with BOC operations — prevents double-decrement on retry between
    /// separate commits (e.g. BOC delete vs shardstate entry delete).
    pub fn delete_boc(
        self: &Arc<Self>,
        root_cell_id: &UInt256,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
        extra_ops: impl FnOnce(&mut rocksdb::WriteBatch) -> Result<()>,
    ) -> Result<()> {
        log::debug!(target: TARGET, "DynamicBocDb::delete_boc  {:x}", root_cell_id);

        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let visited =
            VisitedMap::with_hasher_and_shard_amount(ahash::RandomState::new(), VISITED_MAP_CHUNKS);
        let dec_error = AtomicBool::new(false);
        self.save_cells_threadpool.scope(|scope| {
            self.delete_cells_parallel(
                root_cell_id,
                &visited,
                root_cell_id,
                check_stop,
                scope,
                &dec_error,
            )
        })?;
        if dec_error.load(Ordering::Relaxed) {
            fail!("delete_cells_parallel: failed for some child cell")
        }
        #[cfg(feature = "telemetry")]
        let traverse_time = now.elapsed().as_micros();
        let visited = visited.into_read_only();

        // Single pass over `visited` (dashmap iteration is the expensive part).
        // During this pass:
        //   - Build the RocksDB write batch directly.
        //   - Record per-entry post-commit cache actions into a plain Vec, so the
        //     post-commit loop iterates a contiguous structure instead of the
        //     sharded dashmap.
        //
        // Write batch semantics (delete path):
        // - Updated{0}: Delete cell + Delete counter.
        // - Updated{n>0}: Put counter = n.
        // delete_cells_parallel only produces Updated entries with the final
        // absolute counter computed during traversal.
        #[cfg(feature = "telemetry")]
        let now2 = std::time::Instant::now();
        let cells_cf = self.cell_db.cells_cf()?;
        let counters_cf = self.counters_cf()?;
        let mut deleted = 0;
        let mut transaction = rocksdb::WriteBatch::default();
        let mut deleted_cells = Vec::new();
        for (id, vc) in visited.iter() {
            let refs = match vc {
                VisitedCell::Updated { refs } => *refs,
                _ => {
                    fail!(
                        "DynamicBocDb::delete_boc  {:x}  unexpected variant for {:x}",
                        root_cell_id,
                        id,
                    );
                }
            };
            if refs == 0 {
                transaction.delete_cf(&cells_cf, id.as_slice());
                transaction.delete_cf(&counters_cf, id.as_slice());
                deleted += 1;
            } else {
                transaction.put_cf(&counters_cf, id.as_slice(), refs.to_le_bytes());
            };
            deleted_cells.push((id.clone(), refs));
        }
        #[cfg(feature = "telemetry")]
        let tr_build_time = now2.elapsed().as_micros();

        // append caller-provided ops to the same batch for atomic commit
        extra_ops(&mut transaction)?;

        #[cfg(feature = "telemetry")]
        let now3 = Instant::now();
        self.cell_db.db().write(transaction)?;

        #[cfg(feature = "telemetry")]
        let tr_commit_time = now3.elapsed().as_micros();

        // Counter and cell caches are flushed only after the DB commit succeeds.
        for (id, refs) in deleted_cells.iter() {
            if *refs == 0 {
                self.cell_db.remove_from_cache(id);
                self.cell_counter_cache.remove(id);
            } else {
                self.set_cached_counter(id, *refs);
            }
        }
        #[cfg(feature = "telemetry")]
        let total_time = now.elapsed().as_micros() as u64;

        let updated = visited.len() - deleted;
        #[cfg(feature = "telemetry")]
        if !visited.is_empty() {
            self.cell_db
                .telemetry()
                .boc_db_element_write_nanos
                .update(now3.elapsed().as_nanos() as u64 / (visited.len() as u64 + deleted as u64));
            self.cell_db.telemetry().deleted_cells.update(deleted as u64);
            self.cell_db.telemetry().updated_counters.update(updated as u64);
            self.cell_db.telemetry().delete_boc_total_micros.update(total_time);
            self.cell_db.telemetry().delete_boc_traverse_micros.update(traverse_time as u64);
            self.cell_db.telemetry().delete_boc_tr_build_micros.update(tr_build_time as u64);
            self.cell_db.telemetry().delete_boc_commit_micros.update(tr_commit_time as u64);
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

    fn counters_cf(&self) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.cell_db
            .db()
            .cf_handle(&self.counters_cf_name)
            .ok_or_else(|| error!("Can't get `{}` cf handle", self.counters_cf_name))
    }

    fn get_cached_counter(&self, cell_id: &UInt256) -> Option<u32> {
        let result = self.cell_counter_cache.get(cell_id).map(|a| a.load(Ordering::Relaxed));
        #[cfg(feature = "telemetry")]
        if result.is_some() {
            self.cell_db.telemetry().counter_cache_hits.update(1);
        } else {
            self.cell_db.telemetry().counter_cache_misses.update(1);
        }
        result
    }

    fn set_cached_counter(&self, cell_id: &UInt256, value: u32) {
        match self.cell_counter_cache.get_value_or_guard(cell_id, None) {
            quick_cache::GuardResult::Value(atomic) => {
                atomic.store(value, Ordering::Relaxed);
            }
            quick_cache::GuardResult::Guard(guard) => {
                let _ = guard.insert(Arc::new(AtomicU32::new(value)));
            }
            quick_cache::GuardResult::Timeout => unreachable!(),
        }
        #[cfg(feature = "telemetry")]
        self.cell_db.telemetry().counter_cache_len.update(self.cell_counter_cache.len() as u64);
    }

    fn set_cached_counter_inc(&self, cell_id: &UInt256, inc: u32) -> Result<()> {
        match self.cell_counter_cache.get_value_or_guard(cell_id, None) {
            quick_cache::GuardResult::Value(atomic) => {
                if let Err(prev) =
                    atomic.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                        if current == UNKNOWN_COUNTER {
                            None // evicted between save start and now, skip
                        } else {
                            if let Some(new) = current.checked_add(inc) {
                                if new <= MAX_REFCNT {
                                    return Some(new);
                                }
                            }
                            None
                        }
                    })
                {
                    if prev != UNKNOWN_COUNTER {
                        fail!("counter overflow for {cell_id:x} during inc update: {prev} + {inc}");
                    }
                }
            }
            quick_cache::GuardResult::Guard(guard) => {
                let _ = guard.insert(Arc::new(AtomicU32::new(UNKNOWN_COUNTER)));
            }
            quick_cache::GuardResult::Timeout => unreachable!(),
        }
        #[cfg(feature = "telemetry")]
        self.cell_db.telemetry().counter_cache_len.update(self.cell_counter_cache.len() as u64);
        Ok(())
    }

    /// Returns true if the cell is known to be in DB (cache hit or DB Get confirms).
    /// Counter cache is NOT populated here — that's deferred to the post-commit flush.
    fn check_in_db(
        &self,
        cell_id: &UInt256,
        counters_cf: &impl rocksdb::AsColumnFamilyRef,
    ) -> Result<bool> {
        if self.cell_counter_cache.peek(cell_id).is_some() {
            return Ok(true);
        }
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let exists = self.cell_db.db().get_pinned_cf(counters_cf, cell_id.as_slice())?.is_some();
        #[cfg(feature = "telemetry")]
        if exists {
            self.cell_db
                .telemetry()
                .load_counter_time_nanos
                .update(now.elapsed().as_nanos() as u64);
            self.cell_db.telemetry().loaded_counters.update(1);
        }
        Ok(exists)
    }

    // This method minimizes number of DB queries by checking internal cell type (storage or not).
    // Idea is the following:
    // 1) Traverse cells recursively from root to leaves
    // 2) For each reference check if it is maybe new
    //    (is not a storage or was not visited during this save operation)
    // 3) If at least one reference is new, then this cell is definitely new too,
    //    so we DO NOT NEED to query DB for its counter
    /// Returns: `true` if cell is new (not in DB before this operation), `false` otherwise.
    fn save_cells_parallel<'scope>(
        self: &'scope Arc<DynamicBocDb>,
        cell: &Cell,
        visited: &'scope VisitedMap,
        root_id: &'scope UInt256,
        check_stop: &'scope (dyn Fn() -> Result<()> + Sync),
        scope: &rayon::Scope<'scope>,
        inc_error: &'scope AtomicBool,
        counters_cf: Arc<rocksdb::BoundColumnFamily<'scope>>,
    ) -> Result<bool> {
        /// Inserts a placeholder for a cell known to be in DB, without bumping its inc.
        /// If a placeholder was already inserted (e.g. by parent's claim), nothing happens.
        fn mark_in_db(cell_id: &UInt256, visited: &VisitedMap) {
            if let dashmap::mapref::entry::Entry::Vacant(entry) = visited.entry(cell_id.clone()) {
                entry.insert(VisitedCell::placeholder());
            }
        }

        /// Increment inc for an existing-in-DB child cell.
        /// Creates an `Inc { inc: 1 }` if not in visited yet.
        fn update_child_inc(cell_id: &UInt256, visited: &VisitedMap) -> Result<()> {
            match visited.entry(cell_id.clone()) {
                dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                    entry.get_mut().inc()?;
                }
                dashmap::mapref::entry::Entry::Vacant(entry) => {
                    entry.insert(VisitedCell::Inc { inc: 1 });
                }
            }
            Ok(())
        }

        check_stop()?;

        let cell_id = cell.repr_hash();
        let mut skip_db_check = false;

        //
        // If cell is lazy or stored — it may exist in DB.
        //
        if cell.is_lazy() {
            // Lazy cell is pruned branch from merkle update, so it must be in DB.
            mark_in_db(cell_id, visited);
            return Ok(false);
        }
        if self.cell_db().is_stored_cell(cell) {
            if self.check_in_db(cell_id, &counters_cf)? {
                mark_in_db(cell_id, visited);
                return Ok(false);
            }
            // Confirmed not in DB
            skip_db_check = true;
        }

        //
        // The cell seems new (not in DB)
        // Make recursive calls for refs (in parallel)
        //
        let ref_count = cell.references_count();
        let mut ref_results: [Result<bool>; MAX_REFERENCES_COUNT] =
            std::array::from_fn(|_| Ok(false));

        self.save_cells_threadpool.scope(|s| {
            for (i, verdict) in ref_results.iter_mut().take(ref_count).enumerate() {
                let counters_cf = counters_cf.clone();
                let task = move || -> Result<bool> {
                    let ref_hash = cell.reference_repr_hash(i)?;
                    match visited.entry(ref_hash) {
                        dashmap::mapref::entry::Entry::Occupied(_) => return Ok(false),
                        dashmap::mapref::entry::Entry::Vacant(entry) => {
                            // Placeholder to prevent parallel processing of the same cell.
                            entry.insert(VisitedCell::placeholder());
                        }
                    }
                    let reference = cell.reference(i)?;
                    self.save_cells_parallel(
                        &reference,
                        visited,
                        root_id,
                        check_stop,
                        scope,
                        inc_error,
                        counters_cf,
                    )
                };

                if i == ref_count - 1 {
                    *verdict = task();
                } else {
                    s.spawn(move |_| *verdict = task());
                }
            }
        });

        let mut is_new_cell = false;
        let mut old_refs = 0usize;
        let mut ref_verdicts = [false; MAX_REFERENCES_COUNT];
        for (verdict, result) in ref_verdicts.iter_mut().zip(ref_results).take(ref_count) {
            *verdict = result?;
            if *verdict {
                is_new_cell = true;
            } else {
                old_refs += 1;
            }
        }

        //
        // Late membership check: if cell wasn't lazy/stored but all its children are old,
        // it might still be in DB (e.g. constructed in memory with a hash that already exists).
        //
        if !is_new_cell && !skip_db_check && self.check_in_db(cell_id, &counters_cf)? {
            mark_in_db(cell_id, visited);
            return Ok(false);
        }

        //
        // The cell is definitely new.
        // Increment inc counters for its existing (in-DB) children in parallel.
        //
        let mut processed = 0usize;
        for i in 0..ref_count {
            if !ref_verdicts[i] {
                let ref_hash = cell.reference_repr_hash(i)?;
                if processed + 1 == old_refs {
                    // Last non-new ref — handle inline (avoid unnecessary spawn)
                    update_child_inc(&ref_hash, visited)?;
                } else {
                    scope.spawn(move |_| {
                        if let Err(e) = update_child_inc(&ref_hash, visited) {
                            log::error!(
                                target: TARGET,
                                "DynamicBocDb::save_cells_parallel  {:x}  update child inc failed: {e}",
                                root_id
                            );
                            inc_error.store(true, Ordering::Relaxed);
                        }
                    });
                    processed += 1;
                }
            }
        }

        //
        // Add this cell to visited as new, or bump inc if a placeholder/parallel visit got here first.
        //
        match visited.entry(cell_id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                // The placeholder must be Inc (from claim or sibling inc).
                let prev_inc = match entry.get() {
                    VisitedCell::Inc { inc } => *inc,
                    other => {
                        fail!("unexpected variant before new claim for {cell_id:x}: {other:?}")
                    }
                };
                let inc = prev_inc
                    .checked_add(1)
                    .ok_or_else(|| error!("inc overflow for {cell_id:x}"))?;
                *entry.get_mut() = VisitedCell::New { cell: cell.clone(), inc };
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(VisitedCell::with_new_cell(cell.clone()));
            }
        }

        Ok(true)
    }

    fn save_one_cell(
        self: &Arc<Self>,
        cell: Cell,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
        root_id: &UInt256,
    ) -> Result<()> {
        let counters_cf = self.counters_cf()?;
        let cell_id = cell.repr_hash();

        if self.touch_cell_for_save(cell_id, &counters_cf, visited)? {
            // Already known — inc incremented, nothing else to do.
            return Ok(());
        }

        // Brand new cell. Insert as New with inc = 1.
        visited.insert(cell_id.clone(), VisitedCell::with_new_cell(cell.clone()));
        log::trace!(
            target: TARGET,
            "DynamicBocDb::save_one_cell  {:x}  new cell  root_cell_id {:x}",
            cell_id, root_id
        );

        // Every reference must be already saved (precondition of save_one_cell).
        for i in 0..cell.references_count() {
            let ref_hash = cell.reference_repr_hash(i)?;
            if !self.touch_cell_for_save(&ref_hash, &counters_cf, visited)? {
                fail!("save_one_cell supports only cells with all references already saved");
            }
        }
        Ok(())
    }

    /// For save_one_cell path: increment inc in visited if cell is known to be in DB.
    /// Returns true if the cell was found (either in visited, in cache, or in DB).
    /// Returns false if the cell is brand new (visited unchanged).
    /// Counter cache is NOT populated here — that's deferred to the post-commit flush.
    fn touch_cell_for_save(
        &self,
        cell_id: &UInt256,
        counters_cf: &impl rocksdb::AsColumnFamilyRef,
        visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>,
    ) -> Result<bool> {
        if let Some(vc) = visited.get_mut(cell_id) {
            vc.inc()?;
            return Ok(true);
        }
        if self.check_in_db(cell_id, counters_cf)? {
            visited.insert(cell_id.clone(), VisitedCell::Inc { inc: 1 });
            return Ok(true);
        }
        Ok(false)
    }

    /// Loads the base DB value of a counter for the delete path: cache first, then RocksDB.
    /// Counter cache is NOT populated here — that's deferred to the post-commit flush.
    fn load_counter_base(
        &self,
        cell_id: &UInt256,
        counters_cf: &impl rocksdb::AsColumnFamilyRef,
    ) -> Result<u32> {
        if let Some(counter) = self.get_cached_counter(cell_id) {
            if counter != UNKNOWN_COUNTER {
                return Ok(counter);
            }
        }
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let raw = self
            .cell_db
            .db()
            .get_pinned_cf(counters_cf, cell_id.as_slice())?
            .ok_or_else(|| error!("DB doesn't have counter for existing cell {cell_id:x}"))?;
        if raw.len() != 4 {
            fail!("malformed counter for {cell_id:x}: expected 4 bytes, got {}", raw.len());
        }
        #[cfg(feature = "telemetry")]
        {
            self.cell_db
                .telemetry()
                .load_counter_time_nanos
                .update(now.elapsed().as_nanos() as u64);
            self.cell_db.telemetry().loaded_counters.update(1);
        }
        let counter = u32::from_le_bytes(
            raw[..4].try_into().map_err(|_| error!("malformed counter for {cell_id:x}"))?,
        );
        Ok(counter)
    }

    fn delete_cells_parallel<'scope>(
        self: &'scope Arc<DynamicBocDb>,
        cell_id: &UInt256,
        visited: &'scope VisitedMap,
        root_id: &'scope UInt256,
        check_stop: &'scope (dyn Fn() -> Result<()> + Sync),
        scope: &rayon::Scope<'scope>,
        dec_error: &'scope AtomicBool,
    ) -> Result<()> {
        check_stop()?;

        let counters_cf = self.counters_cf()?;

        // Decrement under the dashmap entry lock so threads reaching the same
        // cell via different paths within this delete operation can't race.
        // First touch loads the base value and stores Updated{base - 1};
        // subsequent touches just decrement the stored absolute counter.
        let new_count = match visited.entry(cell_id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => entry.get_mut().dec()?,
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let base = self.load_counter_base(cell_id, &counters_cf)?;
                let new_count = base
                    .checked_sub(1)
                    .ok_or_else(|| error!("refcnt underflow for {cell_id:x} (root {root_id:x})"))?;
                entry.insert(VisitedCell::Updated { refs: new_count });
                new_count
            }
        };

        if new_count > 0 {
            return Ok(());
        }

        // Counter dropped to zero — recurse into children to decrement them too.
        let cell = match self.cell_db.load_cell(cell_id) {
            Ok(cell) => cell,
            Err(e) => {
                log::warn!(target: TARGET, "DynamicBocDb::delete_cells_parallel  load_cell failed: {e:?}");
                return Ok(());
            }
        };

        let ref_count = cell.references_count();
        for i in 0..ref_count {
            let ref_hash = cell.reference_repr_hash(i)?;
            if i == ref_count - 1 {
                // Process the last reference in the current thread to avoid unnecessary task spawn.
                self.delete_cells_parallel(
                    &ref_hash, visited, root_id, check_stop, scope, dec_error,
                )?;
            } else {
                scope.spawn(move |s| {
                    if let Err(e) = self.delete_cells_parallel(
                        &ref_hash, visited, root_id, check_stop, s, dec_error,
                    ) {
                        log::error!(
                            target: TARGET,
                            "DynamicBocDb::delete_cells_parallel  {:x}  child delete failed with error: {e}",
                            root_id
                        );
                        dec_error.store(true, Ordering::Relaxed);
                    }
                });
            }
        }

        Ok(())
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
                let cells_cf = boc_db_clone.cell_db.cells_cf()?;
                let counters_cf = boc_db_clone.counters_cf()?;
                let mut visited = fnv::FnvHashMap::<UInt256, VisitedCell>::default();

                let max_len = 100_000;
                let mut indexes = Vec::with_capacity(max_len);
                let commit = |visited: &mut fnv::FnvHashMap<UInt256, VisitedCell>| -> Result<()> {
                    let mut transaction = rocksdb::WriteBatch::default();
                    for (id, vc) in visited.iter() {
                        match vc {
                            VisitedCell::New { cell, inc } => {
                                let data = serialize_stored_cell(cell)?;
                                transaction.put_cf(&cells_cf, id.as_slice(), &data);
                                transaction.merge_cf(
                                    &counters_cf,
                                    id.as_slice(),
                                    inc.to_le_bytes(),
                                );
                            }
                            VisitedCell::Inc { inc } => {
                                if *inc > 0 {
                                    transaction.merge_cf(
                                        &counters_cf,
                                        id.as_slice(),
                                        inc.to_le_bytes(),
                                    );
                                }
                            }
                            VisitedCell::Updated { .. } => {
                                log::error!(
                                    target: TARGET,
                                    "AsyncCellsStorageAdapter::commit  unexpected Updated variant for {:x}",
                                    id,
                                );
                            }
                        }
                    }
                    boc_db_clone.cell_db.db().write(transaction)?;
                    // Counter cache flush is done only after the DB commit succeeds.
                    for (id, vc) in visited.iter() {
                        match vc {
                            VisitedCell::New { inc, .. } => {
                                boc_db_clone.set_cached_counter(id, *inc);
                            }
                            VisitedCell::Inc { inc } if *inc > 0 => {
                                if let Some(atomic) = boc_db_clone.cell_counter_cache.get(id) {
                                    let _ = atomic.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                                        if v == UNKNOWN_COUNTER {
                                            None
                                        } else {
                                            v.checked_add(*inc)
                                        }
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
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
                    let rh = cell.repr_hash().clone();
                    boc_db_clone.save_one_cell(cell, &mut visited, &rh)?;
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
            let cell = self.boc_db.cell_db.load_cell(&hash)?;
            self.cache.insert(index, cell.clone());
            Ok(cell)
        }
    }

    fn store_cell(&mut self, index: u32, cell: &Cell) -> Result<()> {
        if index as usize >= self.index.len() {
            fail!("AsyncCellsStorageAdapter::store_cell index out of bounds: {}", index);
        }
        self.index[index as usize] = (cell.repr_hash().clone(), cell.repr_depth());
        self.cache.insert(index, cell.clone());
        self.sender.blocking_send((index, cell.clone()))?;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        self.index = vec![];
        // self.cache.clear();
        Ok(())
    }

    fn loader(&self) -> &CellLoader {
        self.boc_db.cell_db().storing_loader()
    }
}
