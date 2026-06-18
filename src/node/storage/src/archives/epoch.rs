/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    archives::ARCHIVE_SLICE_SIZE,
    db::rocksdb::{AccessType, RocksDb},
    TARGET,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use ton_block::{error, fail, Result};

const EPOCH_META_FILENAME: &str = "epoch_meta.json";

/// Persisted metadata for an epoch directory
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct EpochMeta {
    pub mc_seq_no_start: u32,
    pub mc_seq_no_end: u32,
}

async fn read_epoch_meta(epoch_path: &Path) -> Result<EpochMeta> {
    let meta_path = epoch_path.join(EPOCH_META_FILENAME);
    let data = tokio::fs::read_to_string(&meta_path)
        .await
        .map_err(|e| error!("Cannot read {}: {}", meta_path.display(), e))?;
    serde_json::from_str(&data).map_err(|e| error!("Cannot parse {}: {}", meta_path.display(), e))
}

pub(crate) async fn write_epoch_meta(epoch_path: &Path, meta: &EpochMeta) -> Result<()> {
    let meta_path = epoch_path.join(EPOCH_META_FILENAME);
    let data = serde_json::to_string_pretty(meta)
        .map_err(|e| error!("Cannot serialize epoch meta: {}", e))?;
    tokio::fs::write(&meta_path, data.as_bytes())
        .await
        .map_err(|e| error!("Cannot write {}: {}", meta_path.display(), e))
}

/// Configuration for a single existing epoch directory
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EpochEntry {
    pub path: PathBuf,
}

/// Archival mode configuration.
/// When present, archives are split into epochs and GC is disabled.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ArchivalModeConfig {
    /// Number of MC blocks per epoch. Must be a positive multiple of ARCHIVE_SLICE_SIZE (20_000).
    pub epoch_size: u32,
    /// Path where new epoch directories will be created
    pub new_epochs_path: PathBuf,
    /// List of existing epoch directories, ordered by ascending MC seq_no.
    #[serde(default)]
    pub existing_epochs: Vec<EpochEntry>,
}

/// Runtime state for a single epoch
pub struct Epoch {
    mc_seq_no_start: u32,
    mc_seq_no_end: u32,
    path: Arc<PathBuf>,
    db: Arc<RocksDb>,
}

impl Epoch {
    pub fn mc_seq_no_start(&self) -> u32 {
        self.mc_seq_no_start
    }

    pub fn mc_seq_no_end(&self) -> u32 {
        self.mc_seq_no_end
    }

    pub fn path(&self) -> &Arc<PathBuf> {
        &self.path
    }

    pub fn db(&self) -> &Arc<RocksDb> {
        &self.db
    }
}

/// Routes mc_seq_no to the appropriate epoch's RocksDb and filesystem path.
///
/// All epochs must have the same size (`epoch_size`), which allows O(1) arithmetic lookup
/// without any map search.
pub struct EpochRouter {
    epochs: lockfree::map::Map<u32, Arc<Epoch>>,
    epoch_size: u32,
    new_epochs_path: PathBuf,
    creation_mutex: tokio::sync::Mutex<()>,
}

impl EpochRouter {
    pub async fn new(config: &ArchivalModeConfig) -> Result<Self> {
        if config.epoch_size == 0 || config.epoch_size % ARCHIVE_SLICE_SIZE != 0 {
            fail!(
                "epoch_size must be a positive multiple of ARCHIVE_SLICE_SIZE ({}), got {}",
                ARCHIVE_SLICE_SIZE,
                config.epoch_size
            );
        }

        let epochs = lockfree::map::Map::new();

        for (i, entry) in config.existing_epochs.iter().enumerate() {
            if !entry.path.exists() {
                fail!("Epoch {} path does not exist: {}", i, entry.path.display());
            }

            let meta = read_epoch_meta(&entry.path).await?;
            Self::validate_epoch_meta(&meta, config.epoch_size, &entry.path)?;

            let db = RocksDb::new(&entry.path, "archive_db", None, AccessType::ReadWrite)?;

            log::info!(
                target: TARGET,
                "Opened epoch {}: mc_seq_no [{}, {}], path: {}",
                i, meta.mc_seq_no_start, meta.mc_seq_no_end, entry.path.display()
            );

            epochs.insert(
                meta.mc_seq_no_start,
                Arc::new(Epoch {
                    mc_seq_no_start: meta.mc_seq_no_start,
                    mc_seq_no_end: meta.mc_seq_no_end,
                    path: Arc::new(entry.path.clone()),
                    db,
                }),
            );
        }

        tokio::fs::create_dir_all(&config.new_epochs_path).await.map_err(|e| {
            error!("Cannot create new_epochs_path {}: {}", config.new_epochs_path.display(), e)
        })?;

        // Discover epochs previously created in new_epochs_path (survive restarts)
        let mut read_dir = tokio::fs::read_dir(&config.new_epochs_path).await.map_err(|e| {
            error!("Cannot read new_epochs_path {}: {}", config.new_epochs_path.display(), e)
        })?;
        let mut discovered = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| error!("Error reading new_epochs_path: {}", e))?
        {
            let epoch_path = entry.path();
            if epoch_path.is_dir() && epoch_path.join(EPOCH_META_FILENAME).exists() {
                discovered.push(epoch_path);
            }
        }

        for epoch_path in discovered {
            let meta = read_epoch_meta(&epoch_path).await?;
            Self::validate_epoch_meta(&meta, config.epoch_size, &epoch_path)?;

            // Skip if already loaded from existing_epochs
            if epochs.get(&meta.mc_seq_no_start).is_some() {
                continue;
            }

            let db = RocksDb::new(&epoch_path, "archive_db", None, AccessType::ReadWrite)?;

            log::info!(
                target: TARGET,
                "Discovered epoch: mc_seq_no [{}, {}], path: {}",
                meta.mc_seq_no_start, meta.mc_seq_no_end, epoch_path.display()
            );

            epochs.insert(
                meta.mc_seq_no_start,
                Arc::new(Epoch {
                    mc_seq_no_start: meta.mc_seq_no_start,
                    mc_seq_no_end: meta.mc_seq_no_end,
                    path: Arc::new(epoch_path),
                    db,
                }),
            );
        }

        Ok(Self {
            epochs,
            epoch_size: config.epoch_size,
            new_epochs_path: config.new_epochs_path.clone(),
            creation_mutex: tokio::sync::Mutex::new(()),
        })
    }

    pub fn resolve(&self, mc_seq_no: u32) -> Option<Arc<Epoch>> {
        let start = (mc_seq_no / self.epoch_size) * self.epoch_size;
        self.epochs.get(&start).map(|g| Arc::clone(g.val()))
    }

    /// Resolve the epoch for a given mc_seq_no, creating a new one if needed.
    pub async fn resolve_or_create(&self, mc_seq_no: u32) -> Result<Arc<Epoch>> {
        if let Some(epoch) = self.resolve(mc_seq_no) {
            return Ok(epoch);
        }

        // Serialize creation to prevent concurrent RocksDb::new() on the same path
        let _creation_guard = self.creation_mutex.lock().await;

        // Double-check after acquiring the mutex — another caller may have created the epoch
        if let Some(epoch) = self.resolve(mc_seq_no) {
            return Ok(epoch);
        }

        let epoch_index = mc_seq_no / self.epoch_size;
        let start = epoch_index * self.epoch_size;
        let end = start + self.epoch_size - 1;

        let epoch_dir = self.new_epochs_path.join(format!("epoch_{}", epoch_index));
        tokio::fs::create_dir_all(&epoch_dir)
            .await
            .map_err(|e| error!("Cannot create epoch directory {}: {}", epoch_dir.display(), e))?;

        let meta = EpochMeta { mc_seq_no_start: start, mc_seq_no_end: end };
        write_epoch_meta(&epoch_dir, &meta).await?;

        let db = RocksDb::new(&epoch_dir, "archive_db", None, AccessType::ReadWrite)?;

        log::info!(
            target: TARGET,
            "Created new epoch {}: mc_seq_no [{}, {}], path: {}",
            epoch_index, start, end, epoch_dir.display()
        );

        let epoch = Arc::new(Epoch {
            mc_seq_no_start: start,
            mc_seq_no_end: end,
            path: Arc::new(epoch_dir),
            db,
        });
        self.epochs.insert(start, Arc::clone(&epoch));

        Ok(epoch)
    }

    /// Returns summed approximate RocksDB memory usage across all epochs.
    pub fn memory_usage(&self) -> crate::RocksDbMemoryUsage {
        let mut total = crate::RocksDbMemoryUsage::default();
        for guard in self.epochs.iter() {
            total += guard.val().db().memory_usage();
        }
        total
    }

    pub fn epoch_size(&self) -> u32 {
        self.epoch_size
    }

    fn validate_epoch_meta(meta: &EpochMeta, epoch_size: u32, path: &Path) -> Result<()> {
        if meta.mc_seq_no_start % epoch_size != 0 {
            fail!(
                "Epoch at {} has mc_seq_no_start={} which is not aligned to epoch_size={}",
                path.display(),
                meta.mc_seq_no_start,
                epoch_size
            );
        }
        let expected_end = meta.mc_seq_no_start + epoch_size - 1;
        if meta.mc_seq_no_end != expected_end {
            fail!(
                "Epoch at {} has mc_seq_no_end={} but expected {} for epoch_size={}",
                path.display(),
                meta.mc_seq_no_end,
                expected_end,
                epoch_size
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/test_epoch.rs"]
mod tests;
