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
    cell_db::CellDb, db::rocksdb::RocksDb, shardstate_db_async::CellsDbConfig, types::StoredCell,
    StorageAlloc, TARGET,
};
use std::{ops::Deref, path::Path, sync::Arc};
use ton_block::{Cell, Result, UInt256, MAX_LEVEL};

pub struct DynamicBocArchiveDb {
    cell_db: Arc<CellDb>,
}

impl DynamicBocArchiveDb {
    pub fn with_db(
        db: Arc<RocksDb>,
        cell_db_cf: &str,
        db_root_path: impl AsRef<Path>,
        config: &CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Self> {
        let cell_db = Arc::new(CellDb::with_db(
            db,
            cell_db_cf,
            db_root_path,
            config,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )?);
        Ok(Self { cell_db })
    }

    pub fn cell_db(&self) -> &Arc<CellDb> {
        &self.cell_db
    }

    /// Thread-safe append-only save.
    pub fn save_boc(
        &self,
        root_cell: Cell,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Cell> {
        let root_id = root_cell.hash(MAX_LEVEL);
        let cells_cf = self.cell_db.cells_cf()?;

        log::debug!(target: TARGET, "DynamicBocArchiveDb::save_boc  {:x}", root_id);

        if let Some(existing) = self.cell_db.try_load_existing_root(&root_id, &cells_cf)? {
            log::info!(target: TARGET, "DynamicBocArchiveDb::save_boc  ALREADY EXISTS  {:x}", root_id);
            return Ok(existing);
        }

        let start = std::time::Instant::now();

        // Traverse cell tree, collect new cells
        let mut new_cells = fnv::FnvHashMap::default();
        let mut visited = fnv::FnvHashSet::default();
        self.collect_new_cells(&root_cell, &mut new_cells, &mut visited, &cells_cf, check_stop)?;
        let cells_traverse_time = start.elapsed().as_micros();

        // Batch write all new cells
        let wrote_cells = new_cells.len();
        let write_start = std::time::Instant::now();
        if !new_cells.is_empty() {
            let mut batch = rocksdb::WriteBatch::default();
            for (id, data) in &new_cells {
                batch.put_cf(&cells_cf, id.as_slice(), data);
            }
            self.cell_db.db().write(batch)?;
        }
        #[cfg(feature = "telemetry")]
        if wrote_cells > 0 {
            self.cell_db
                .telemetry()
                .boc_db_element_write_nanos
                .update(write_start.elapsed().as_nanos() as u64 / wrote_cells as u64);
        }
        let write_time = write_start.elapsed().as_micros();

        let now4 = std::time::Instant::now();
        self.cell_db.cleanup_storing_cells(new_cells.keys());
        let storing_cells_cleanup_time = now4.elapsed().as_micros();

        let total_time = start.elapsed().as_micros() as u64;
        #[cfg(feature = "telemetry")]
        {
            self.cell_db.telemetry().stored_new_cells.update(wrote_cells as u64);
            self.cell_db.telemetry().save_boc_total_micros.update(total_time);
            self.cell_db.telemetry().save_boc_traverse_micros.update(cells_traverse_time as u64);
            self.cell_db.telemetry().save_boc_commit_micros.update(write_time as u64);
            self.cell_db
                .telemetry()
                .save_boc_cleanup_micros
                .update(storing_cells_cleanup_time as u64);
        }

        log::debug!(
            target: TARGET,
            "DynamicBocArchiveDb::save_boc  {:x}  wrote {}, visited {}  TIME: {} (tr:{}|cmt:{}|scc:{})",
            root_id, wrote_cells, visited.len(), total_time, cells_traverse_time, write_time,
            storing_cells_cleanup_time
        );

        self.cell_db.load_cell(&root_id, true)
    }

    fn collect_new_cells(
        &self,
        cell: &Cell,
        new_cells: &mut fnv::FnvHashMap<UInt256, Vec<u8>>,
        visited: &mut fnv::FnvHashSet<UInt256>,
        cells_cf: &impl rocksdb::AsColumnFamilyRef,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<()> {
        check_stop()?;
        let cell_id = cell.repr_hash();

        // Already visited in this traversal (new or existing) — skip
        if !visited.insert(cell_id.clone()) {
            return Ok(());
        }

        // Already a StoredCell (loaded from DB)
        if cell.is::<StoredCell>() {
            return Ok(());
        }

        // Recurse into children first
        for i in 0..cell.references_count() {
            let reference = cell.reference(i)?;
            self.collect_new_cells(&reference, new_cells, visited, cells_cf, check_stop)?;
        }

        // Check if cell exists in DB
        if self.cell_db.db().get_pinned_cf(cells_cf, cell_id.as_slice())?.is_some() {
            return Ok(());
        }

        // Serialize and add to batch
        let data = StoredCell::serialize(cell.deref())?;
        new_cells.insert(cell_id, data);
        Ok(())
    }

    /// Fast import-only save: writes all non-pruned cells from state update unconditionally,
    /// without checking the DB.
    pub fn save_update(&self, root_cell: Cell) -> Result<()> {
        let root_id = root_cell.hash(MAX_LEVEL);
        let cells_cf = self.cell_db.cells_cf()?;

        log::debug!(target: TARGET, "DynamicBocArchiveDb::save_update  {:x}", root_id);

        let start = std::time::Instant::now();

        let mut new_cells = fnv::FnvHashMap::default();
        Self::collect_cells_from_update(&root_cell, &mut new_cells)?;
        let cells_traverse_time = start.elapsed().as_micros();

        let wrote_cells = new_cells.len();
        let write_start = std::time::Instant::now();
        if !new_cells.is_empty() {
            let mut batch = rocksdb::WriteBatch::default();
            for (id, data) in &new_cells {
                batch.put_cf(&cells_cf, id.as_slice(), data);
            }
            self.cell_db.db().write(batch)?;
        }
        let write_time = write_start.elapsed().as_micros();

        log::debug!(
            target: TARGET,
            "DynamicBocArchiveDb::save_update  {:x}  wrote {}  TIME: {} (tr:{}|cmt:{})",
            root_id, wrote_cells, start.elapsed().as_micros(), cells_traverse_time, write_time,
        );

        Ok(())
    }

    /// Collect all non-pruned cells from the tree. No DB lookups — pruned branches
    /// are the boundary (they represent unchanged subtrees already in the DB).
    fn collect_cells_from_update(
        cell: &Cell,
        new_cells: &mut fnv::FnvHashMap<UInt256, Vec<u8>>,
    ) -> Result<()> {
        let cell_id = cell.repr_hash();

        if new_cells.contains_key(&cell_id) {
            return Ok(());
        }

        // PrunedBranch = unchanged subtree, already in DB
        if cell.is_pruned() && cell.level() == 0 {
            return Ok(());
        }

        for i in 0..cell.references_count() {
            let reference = cell.reference(i)?;
            Self::collect_cells_from_update(&reference, new_cells)?;
        }

        let data = StoredCell::serialize_virtual(cell.deref())?;
        new_cells.insert(cell_id, data);
        Ok(())
    }

    pub fn load_cell(self: &Arc<Self>, cell_id: &UInt256, panic: bool) -> Result<Cell> {
        self.cell_db.load_cell(cell_id, panic)
    }
}
