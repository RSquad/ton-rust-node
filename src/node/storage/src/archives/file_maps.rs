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
use super::ARCHIVE_SLICE_SIZE;
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    archives::{
        archive_slice::ArchiveSlice,
        db_provider::{ArchiveDbProvider, SingleDbProvider},
        package_id::{PackageId, PackageType},
        package_index_db::{PackageIndexDb, PackageIndexEntry},
    },
    block_handle_db::BlockHandle,
    db::rocksdb::RocksDb,
    StorageAlloc, TARGET,
};
use adnl::{
    common::{add_unbound_object_to_map_with_update, CountedObject, Counter},
    declare_counted,
};
use std::{
    fmt::Display,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};
use ton_block::{error, fail, BlockIdExt, Result, ShardIdent, LT_ALIGN};

pub const FILES_DB_NAME: &str = "files";
pub const KEY_FILES_DB_NAME: &str = "key_files";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct BlockRanges {
    pub min_seqno: AtomicU32,
    pub max_seqno: AtomicU32,
    pub min_utime: AtomicU32,
    pub max_utime: AtomicU32,
    pub min_lt: AtomicU64,
    pub max_lt: AtomicU64,
}
impl Display for BlockRanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "seqno: {}-{}, utime: {}-{}, lt: {}-{}",
            self.min_seqno.load(Ordering::Relaxed),
            self.max_seqno.load(Ordering::Relaxed),
            self.min_utime.load(Ordering::Relaxed),
            self.max_utime.load(Ordering::Relaxed),
            self.min_lt.load(Ordering::Relaxed),
            self.max_lt.load(Ordering::Relaxed)
        )
    }
}
impl Clone for BlockRanges {
    fn clone(&self) -> Self {
        Self {
            min_seqno: AtomicU32::new(self.min_seqno.load(Ordering::Relaxed)),
            max_seqno: AtomicU32::new(self.max_seqno.load(Ordering::Relaxed)),
            min_utime: AtomicU32::new(self.min_utime.load(Ordering::Relaxed)),
            max_utime: AtomicU32::new(self.max_utime.load(Ordering::Relaxed)),
            min_lt: AtomicU64::new(self.min_lt.load(Ordering::Relaxed)),
            max_lt: AtomicU64::new(self.max_lt.load(Ordering::Relaxed)),
        }
    }
}
impl BlockRanges {
    pub fn compare_seqno(&self, seqno: &u32) -> std::cmp::Ordering {
        let min_sn = self.min_seqno.load(Ordering::Relaxed);
        let max_sn = self.max_seqno.load(Ordering::Relaxed);
        log::trace!(target: TARGET, "Comparing seqno {} with range {} - {}", seqno, min_sn, max_sn);
        if seqno < &min_sn {
            std::cmp::Ordering::Greater
        } else if seqno > &max_sn {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }

    pub fn compare_utime(&self, utime: &u32) -> std::cmp::Ordering {
        let min_ut = self.min_utime.load(Ordering::Relaxed);
        let max_ut = self.max_utime.load(Ordering::Relaxed);
        if utime < &min_ut {
            std::cmp::Ordering::Greater
        } else if utime > &max_ut {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }

    pub fn compare_lt(&self, lt: &u64) -> std::cmp::Ordering {
        let min_lt = self.min_lt.load(Ordering::Relaxed);
        let max_lt = self.max_lt.load(Ordering::Relaxed);
        if lt < &min_lt {
            std::cmp::Ordering::Greater
        } else if lt > &max_lt {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

pub struct FileDescription {
    id: PackageId,
    deleted: bool,
    archive_slice: ArchiveSlice,
    blocks_ranges: lockfree::map::Map<ShardIdent, BlockRanges>,
}

impl FileDescription {
    pub fn with_data(
        id: PackageId,
        archive_slice: ArchiveSlice,
        deleted: bool,
        blocks_ranges: lockfree::map::Map<ShardIdent, BlockRanges>,
    ) -> Self {
        Self { id, deleted, archive_slice, blocks_ranges }
    }

    pub const fn id(&self) -> &PackageId {
        &self.id
    }

    pub const fn deleted(&self) -> bool {
        self.deleted
    }

    pub const fn archive_slice(&self) -> &ArchiveSlice {
        &self.archive_slice
    }

    pub fn update_block_ranges(&self, handle: &BlockHandle) -> bool {
        self.update_block_ranges_raw(
            handle.id().shard(),
            handle.id().seq_no(),
            handle.gen_utime(),
            handle.end_lt(),
        )
    }

    pub fn update_block_ranges_raw(
        &self,
        shard: &ShardIdent,
        seq_no: u32,
        gen_utime: u32,
        end_lt: u64,
    ) -> bool {
        macro_rules! update_atomic {
            ($atomic:expr, $new:expr, $cmp_fn:expr) => {{
                let mut prev = $atomic.load(Ordering::Relaxed);
                while $cmp_fn($new, prev) {
                    match $atomic.compare_exchange_weak(
                        prev,
                        $new,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return true,
                        Err(next) => prev = next,
                    }
                }
                false
            }};
        }
        fn update_min_32(atomic: &AtomicU32, new: u32) -> bool {
            update_atomic!(atomic, new, |new, prev| new < prev)
        }
        fn update_max_32(atomic: &AtomicU32, new: u32) -> bool {
            update_atomic!(atomic, new, |new, prev| new > prev)
        }
        fn update_min_64(atomic: &AtomicU64, new: u64) -> bool {
            update_atomic!(atomic, new, |new, prev| new < prev)
        }
        fn update_max_64(atomic: &AtomicU64, new: u64) -> bool {
            update_atomic!(atomic, new, |new, prev| new > prev)
        }

        let mut updated = false;
        let _ = add_unbound_object_to_map_with_update(&self.blocks_ranges, shard.clone(), |prev| {
            if let Some(prev) = prev {
                updated |= update_min_32(&prev.min_seqno, seq_no);
                updated |= update_max_32(&prev.max_seqno, seq_no);
                updated |= update_min_32(&prev.min_utime, gen_utime);
                updated |= update_max_32(&prev.max_utime, gen_utime);
                updated |= update_min_64(&prev.min_lt, end_lt - end_lt % LT_ALIGN);
                updated |= update_max_64(&prev.max_lt, end_lt);
                Ok(None)
            } else {
                updated = true;
                Ok(Some(BlockRanges {
                    min_seqno: AtomicU32::new(seq_no),
                    max_seqno: AtomicU32::new(seq_no),
                    min_utime: AtomicU32::new(gen_utime),
                    max_utime: AtomicU32::new(gen_utime),
                    min_lt: AtomicU64::new(end_lt - end_lt % LT_ALIGN),
                    max_lt: AtomicU64::new(end_lt),
                }))
            }
        });

        updated
    }

    async fn destroy(&mut self) -> Result<()> {
        self.archive_slice.destroy().await
    }

    async fn trunc<F: Fn(&BlockIdExt) -> bool>(
        &mut self,
        block_id: &BlockIdExt,
        delete_condition: &F,
    ) -> Result<()> {
        self.archive_slice.trunc(block_id, delete_condition).await
    }

    pub fn blocks_ranges(&self) -> &lockfree::map::Map<ShardIdent, BlockRanges> {
        &self.blocks_ranges
    }
}

declare_counted!(
    pub struct FileMapEntry {
        key: u32,
        value: Arc<FileDescription>,
    }
);
impl FileMapEntry {
    pub fn value(&self) -> &FileDescription {
        &self.value
    }
}

pub struct FileMap {
    storage: PackageIndexDb,
    elements: tokio::sync::RwLock<Vec<FileMapEntry>>, // new FileMapEntry every key_block
}

impl FileMap {
    pub async fn new(
        index_db: Arc<RocksDb>,
        db_provider: &Arc<dyn ArchiveDbProvider>,
        path: impl ToString,
        package_type: PackageType,
        last_unneeded_key_block: u32,
        #[cfg(feature = "telemetry")] telemetry: &Arc<StorageTelemetry>,
        allocated: &Arc<StorageAlloc>,
    ) -> Result<Self> {
        let storage = PackageIndexDb::with_db(index_db, path, true)?;
        let mut index_pairs = Vec::new();

        storage.for_each_deserialized(|key, value| {
            index_pairs.push((key, value));
            Ok(true)
        })?;

        index_pairs.sort_by_key(|pair| pair.0);
        let last = index_pairs.last().map(|pair| pair.0);

        let mut elements = Vec::new();
        for (key, value) in index_pairs {
            let unneeded = key < last_unneeded_key_block;
            let finalized = value.finalized() && Some(key) != last;
            log::info!(
                target: TARGET,
                "Opening archive slice {}, finalized {}, unneeded {}",
                key, finalized, unneeded
            );
            let (slice_db, slice_root_path) = db_provider.db_for_archive(key).await?;
            let archive_slice = match ArchiveSlice::with_data(
                slice_db,
                slice_root_path,
                key,
                package_type,
                finalized,
                unneeded,
                #[cfg(feature = "telemetry")]
                telemetry.clone(),
                allocated.clone(),
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    log::warn!(target: TARGET, "Can't read archive slice {}: {}", key, e);
                    if unneeded {
                        match storage.delete(&key.into()) {
                            Ok(_) => {
                                log::info!(target: TARGET, "Deleted archive slice from index {}", key)
                            }
                            Err(e) => {
                                log::info!(target: TARGET, "Can't delete archive slice from index {}: {}", key, e)
                            }
                        }
                    }
                    continue;
                }
            };
            let value = Arc::new(FileDescription::with_data(
                PackageId::with_values(key, package_type),
                archive_slice,
                value.deleted(),
                value.blocks_ranges()?,
            ));
            elements.push(FileMapEntry {
                key,
                value,
                counter: allocated.file_entries.clone().into(),
            });
            #[cfg(feature = "telemetry")]
            telemetry.file_entries.update(allocated.file_entries.load(Ordering::Relaxed))
        }

        Ok(Self { storage, elements: tokio::sync::RwLock::new(elements) })
    }

    pub async fn put(
        &self,
        mc_seq_no: u32,
        file_description: Arc<FileDescription>,
        #[cfg(feature = "telemetry")] telemetry: &Arc<StorageTelemetry>,
        allocated: &Arc<StorageAlloc>,
    ) -> Result<()> {
        let entry = FileMapEntry {
            key: mc_seq_no,
            value: file_description,
            counter: allocated.file_entries.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        telemetry.file_entries.update(allocated.file_entries.load(Ordering::Relaxed));
        let mut guard = self.elements.write().await;
        match guard.binary_search_by(|entry| entry.key.cmp(&mc_seq_no)) {
            Ok(index) => guard[index] = entry,
            Err(index) => guard.insert(index, entry),
        }
        self.storage.put_value(&mc_seq_no.into(), &PackageIndexEntry::new())?;
        Ok(())
    }

    pub async fn update(&self, key: u32, fd: &FileDescription) -> Result<()> {
        let val = PackageIndexEntry::with_data(
            fd.deleted(),
            fd.archive_slice().finalized(),
            &fd.blocks_ranges,
        );
        self.storage.put_value(&key.into(), &val)?;
        Ok(())
    }

    async fn get_unneeded_entries(&self, last_unneeded_key_block: &BlockIdExt) -> Vec<u32> {
        let elements = self.elements.read().await;
        let mut marked_packages = Vec::new();

        for i in 0..elements.len() {
            let next_id = if i == elements.len() - 1 {
                elements[i].value.archive_slice.archive_id() + ARCHIVE_SLICE_SIZE
            } else {
                elements[i + 1].value.archive_slice.archive_id()
            };

            if elements[i].value.archive_slice.package_type() == PackageType::Blocks
                && next_id <= last_unneeded_key_block.seq_no()
            {
                marked_packages.push(elements[i].key);
            }
        }
        marked_packages
    }

    pub async fn gc(&self, last_unneeded_key_block: &BlockIdExt) -> Result<()> {
        log::info!(
            target: TARGET,
            "Archives GC started, last_unneeded_key_block: {}",
            last_unneeded_key_block
        );
        let mut slices = self.get_unneeded_entries(last_unneeded_key_block).await;
        log::info!(
            target: TARGET,
            "Archives GC: found {} unneeded slices",
            slices.len()
        );

        'a: while let Some(key) = slices.pop() {
            let mut guard = self.elements.write().await;
            let mut position = None;
            for (p, entry) in guard.iter_mut().enumerate() {
                if entry.key == key {
                    position = Some(p);
                    match Arc::get_mut(&mut entry.value) {
                        Some(file_description) => {
                            if let Err(e) = file_description.destroy().await {
                                log::error!(target: TARGET, "Archives GC: can't destroy archive slice {}: {:?}", key, e);
                                continue 'a;
                            } else {
                                if let Err(e) = self.storage.delete(&key.into()) {
                                    log::error!(target: TARGET, "Archives GC: can't delete {} from index: {:?}", key, e);
                                    continue 'a;
                                }
                                log::info!(target: TARGET, "Archives GC: collected {}.", key);
                            }
                        }
                        None => {
                            log::error!(target: TARGET, "Archives GC: unable to get mutable reference to file_description");
                            continue 'a;
                        }
                    }
                }
            }
            if let Some(p) = position {
                guard.remove(p);
            } else {
                fail!("Slice {} not found", key)
            }
        }
        log::info!(target: TARGET, "Archives GC finished.");
        Ok(())
    }

    pub async fn get(&self, mc_seq_no: u32) -> Option<Arc<FileDescription>> {
        let guard = self.elements.read().await;
        log::trace!(target: TARGET, "Searching for file description (elements count = {})", guard.len());
        match guard.binary_search_by(|entry| entry.key.cmp(&mc_seq_no)) {
            Ok(index) => Some(Arc::clone(&guard[index].value)),
            Err(_) => None,
        }
    }

    pub async fn get_closest_by(
        &self,
        f: impl FnMut(&FileMapEntry) -> std::cmp::Ordering,
    ) -> Option<Arc<FileDescription>> {
        let guard = self.elements.read().await;
        log::trace!(target: TARGET, "Searching for file description (elements count = {})", guard.len());
        let index = match guard.binary_search_by(f) {
            Ok(index) => index,
            Err(index) => (guard.len() - 1).min(index),
        };
        Some(Arc::clone(&guard[index].value))
    }

    pub async fn get_closest(&self, mc_seq_no: u32) -> Option<Arc<FileDescription>> {
        let guard = self.elements.read().await;
        log::trace!(target: TARGET, "Searching for file description (elements count = {})", guard.len());
        let index = match guard.binary_search_by(|entry| entry.key.cmp(&mc_seq_no)) {
            Ok(index) => index,
            Err(0) => return None,
            Err(index) => index - 1,
        };
        Some(Arc::clone(&guard[index].value))
    }

    pub async fn get_closest_id(&self, mc_seq_no: u32) -> Option<u32> {
        self.get_closest(mc_seq_no).await.map(|fd| fd.id().id())
    }

    pub async fn get_closest_archive_id(&self, mc_seq_no: u32, shard: &ShardIdent) -> Option<u64> {
        match self.get_closest(mc_seq_no).await {
            Some(fd) => fd.archive_slice().get_archive_id(mc_seq_no, shard).await,
            None => None,
        }
    }

    pub async fn trunc<F: Fn(&BlockIdExt) -> bool>(
        &self,
        block_id: &BlockIdExt,
        delete_condition: &F,
    ) -> Result<()> {
        let mut guard = self.elements.write().await;
        log::trace!(
            target: TARGET,
            "Searching for file description (elements count = {})", guard.len()
        );
        // TODO: may be iterate from end
        let index = match guard.binary_search_by(|entry| entry.key.cmp(&block_id.seq_no)) {
            Ok(index) => index,
            Err(0) => return Ok(()),
            Err(index) => index - 1,
        };
        if guard.len() > index + 1 {
            for mut entry in guard.drain(index + 1..) {
                if let Some(entry) = Arc::get_mut(&mut entry.value) {
                    if let Err(e) = entry.destroy().await {
                        log::warn!(target: TARGET, "Can't destroy entry {}: {}", index, e);
                    }
                } else {
                    log::warn!(target: TARGET, "Can't get_mut entry {}", index);
                }
                self.storage.delete(&entry.key.into())?;
            }
        }
        debug_assert_eq!(guard.len(), index + 1);
        let entry =
            guard.last_mut().ok_or_else(|| error!("internal error during trunc {index}"))?;
        let fd = Arc::get_mut(&mut entry.value)
            .ok_or_else(|| error!("unable to get FileDescription as mutable"))?;
        // clear finalized flag
        self.storage.put_value(&entry.key.into(), &PackageIndexEntry::new())?;
        fd.trunc(block_id, delete_condition).await
    }
}

pub struct FileMaps {
    files: FileMap,
    key_files: FileMap, // temp_files: FileMap,
}

impl FileMaps {
    pub async fn new(
        db: Arc<RocksDb>,
        db_root_path: &Arc<PathBuf>,
        db_provider: &Arc<dyn ArchiveDbProvider>,
        last_unneeded_key_block: u32,
        #[cfg(feature = "telemetry")] telemetry: &Arc<StorageTelemetry>,
        allocated: &Arc<StorageAlloc>,
    ) -> Result<Self> {
        let key_db_provider: Arc<dyn ArchiveDbProvider> =
            Arc::new(SingleDbProvider::new(db.clone(), db_root_path.clone()));
        Ok(Self {
            files: FileMap::new(
                db.clone(),
                db_provider,
                FILES_DB_NAME,
                PackageType::Blocks,
                last_unneeded_key_block,
                #[cfg(feature = "telemetry")]
                telemetry,
                allocated,
            )
            .await?,
            key_files: FileMap::new(
                db.clone(),
                &key_db_provider,
                KEY_FILES_DB_NAME,
                PackageType::KeyBlocks,
                0,
                #[cfg(feature = "telemetry")]
                telemetry,
                allocated,
            )
            .await?,
        })
    }

    pub fn files(&self) -> &FileMap {
        &self.files
    }

    pub fn key_files(&self) -> &FileMap {
        &self.key_files
    }

    pub fn get(&self, package_type: PackageType) -> &FileMap {
        match package_type {
            PackageType::Blocks => &self.files,
            PackageType::KeyBlocks => &self.key_files, //PackageType::Temp => &self.temp_files,
        }
    }

    pub async fn trunc<F: Fn(&BlockIdExt) -> bool>(
        &self,
        block_id: &BlockIdExt,
        delete_condition: &F,
    ) -> Result<()> {
        self.files.trunc(block_id, delete_condition).await?;
        self.key_files.trunc(block_id, delete_condition).await
    }
}
