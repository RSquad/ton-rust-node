/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::epoch::EpochRouter;
use crate::db::rocksdb::RocksDb;
use std::{path::PathBuf, sync::Arc};
use ton_block::Result;

/// Abstracts over single-db and epoch-based db selection for archive slices.
/// Provides the correct RocksDb instance and root path for a given archive_id.
#[async_trait::async_trait]
pub trait ArchiveDbProvider: Send + Sync {
    /// Get the root path and RocksDb instance for the archive slice
    async fn db_for_archive(&self, archive_id: u32) -> Result<(Arc<RocksDb>, Arc<PathBuf>)>;

    /// Summed approximate RocksDB memory usage across all managed instances.
    fn memory_usage(&self) -> crate::RocksDbMemoryUsage;
}

/// Single shared RocksDb, single root path.
/// Used when archival_mode is not configured.
pub struct SingleDbProvider {
    db: Arc<RocksDb>,
    db_root_path: Arc<PathBuf>,
}

impl SingleDbProvider {
    pub fn new(db: Arc<RocksDb>, db_root_path: Arc<PathBuf>) -> Self {
        Self { db, db_root_path }
    }
}

#[async_trait::async_trait]
impl ArchiveDbProvider for SingleDbProvider {
    async fn db_for_archive(&self, _archive_id: u32) -> Result<(Arc<RocksDb>, Arc<PathBuf>)> {
        Ok((self.db.clone(), self.db_root_path.clone()))
    }

    fn memory_usage(&self) -> crate::RocksDbMemoryUsage {
        self.db.memory_usage()
    }
}

/// Epoch-based provider: routes archive requests to the correct epoch's RocksDb and path.
pub struct EpochDbProvider {
    router: Arc<EpochRouter>,
}

impl EpochDbProvider {
    pub fn new(router: Arc<EpochRouter>) -> Self {
        Self { router }
    }

    pub fn router(&self) -> &Arc<EpochRouter> {
        &self.router
    }
}

#[async_trait::async_trait]
impl ArchiveDbProvider for EpochDbProvider {
    async fn db_for_archive(&self, archive_id: u32) -> Result<(Arc<RocksDb>, Arc<PathBuf>)> {
        let epoch_db = self.router.resolve_or_create(archive_id).await?;
        Ok((epoch_db.db().clone(), epoch_db.path().clone()))
    }

    fn memory_usage(&self) -> crate::RocksDbMemoryUsage {
        self.router.memory_usage()
    }
}
