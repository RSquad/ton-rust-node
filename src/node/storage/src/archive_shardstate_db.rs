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
    cell_db::CellByHashStorageAdapter,
    db::rocksdb::{RocksDb, RocksDbTable},
    dynamic_boc_archive_db::DynamicBocArchiveDb,
    shardstate_db_async::{CellsDbConfig, DbEntry},
    traits::Serializable,
    StorageAlloc, TARGET,
};
use std::sync::Arc;
use ton_block::{BlockIdExt, Cell, CellsFactory, CellsStorage, Result, UInt256, UnixTime};

pub struct ArchiveShardStateDb {
    index: Arc<RocksDbTable<BlockIdExt>>,
    boc_db: Arc<DynamicBocArchiveDb>,
}

impl ArchiveShardStateDb {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<RocksDb>,
        index_cf: &str,
        cells_cf: &str,
        config: &CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Self> {
        let boc_db = Arc::new(DynamicBocArchiveDb::with_db(
            db.clone(),
            cells_cf,
            config,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )?);
        let index = Arc::new(RocksDbTable::with_db(db, index_cf, true)?);
        Ok(Self { index, boc_db })
    }

    pub fn put(&self, id: &BlockIdExt, state_root: Cell) -> Result<Cell> {
        let cell_id = state_root.repr_hash().clone();
        log::debug!(
            target: TARGET,
            "ArchiveShardStateDb::put  id {}  root_cell_id {:x}", id, cell_id
        );

        if self.index.contains(id)? {
            log::debug!(
                target: TARGET,
                "ArchiveShardStateDb::put  ALREADY EXISTS  id {}", id
            );
            let data = self.index.get(id)?;
            let db_entry = DbEntry::deserialize(&data)?;
            return self.boc_db.cell_db().load_cell(&db_entry.cell_id);
        }

        let saved = self.boc_db.save_boc(state_root, &|| Ok(()))?;
        let save_utime = UnixTime::now();
        let db_entry = DbEntry::with_params(id.clone(), cell_id, save_utime);
        self.index.put(id, &db_entry.serialize())?;
        Ok(saved)
    }

    pub fn put_update(&self, id: &BlockIdExt, state_root: Cell) -> Result<()> {
        let state_root = state_root.virtualize(1);
        let cell_id = state_root.repr_hash().clone();
        log::debug!(
            target: TARGET,
            "ArchiveShardStateDb::put_update  id {}  root_cell_id {:x}", id, cell_id
        );

        if self.index.contains(id)? {
            log::info!(
                target: TARGET,
                "ArchiveShardStateDb::put_update  ALREADY EXISTS  id {}", id
            );
            return Ok(());
        }

        self.boc_db.save_update(state_root)?;
        let save_utime = UnixTime::now();
        let db_entry = DbEntry::with_params(id.clone(), cell_id, save_utime);
        self.index.put(id, &db_entry.serialize())?;
        Ok(())
    }

    pub fn get(&self, id: &BlockIdExt) -> Result<Cell> {
        let data = self.index.get(id)?;
        let db_entry = DbEntry::deserialize(&data)?;
        log::debug!(
            target: TARGET,
            "ArchiveShardStateDb::get  id {}  cell_id {:x}", id, db_entry.cell_id
        );
        self.boc_db.cell_db().load_cell(&db_entry.cell_id)
    }

    pub fn get_cell(&self, id: &UInt256) -> Result<Cell> {
        self.boc_db.cell_db().load_cell(id)
    }

    pub fn contains(&self, id: &BlockIdExt) -> Result<bool> {
        self.index.contains(id)
    }

    pub fn cells_factory(&self) -> Arc<dyn CellsFactory> {
        self.boc_db.cell_db().clone() as Arc<dyn CellsFactory>
    }

    pub fn create_hashed_cell_storage(
        &self,
        root: Option<&Cell>,
        max_inmemory_cells: usize,
    ) -> Result<impl CellsStorage> {
        CellByHashStorageAdapter::new(self.boc_db.cell_db().clone(), root, max_inmemory_cells)
    }
}
