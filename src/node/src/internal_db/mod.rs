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
use crate::engine_traits::EngineTelemetry;
use crate::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    engine_traits::EngineAlloc,
    error::NodeError,
    internal_db::restore::check_db,
    shard_state::ShardStateStuff,
    types::top_block_descr::{TopBlockDescrId, TopBlockDescrStuff},
};
use std::{
    cmp::min,
    collections::{HashMap, HashSet},
    io::{Read, Seek, Write},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, UNIX_EPOCH},
};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{
    archive_shardstate_db::ArchiveShardStateDb,
    archives::{
        archive_manager::ArchiveManager,
        db_provider::{ArchiveDbProvider, EpochDbProvider, SingleDbProvider},
        epoch::{ArchivalModeConfig, EpochRouter},
        package_entry_id::PackageEntryId,
    },
    block_handle_db::{
        self, BlockHandle, BlockHandleDb, BlockHandleStorage, NodeStateDb, BLOCK_HANDLE_DB_NAME,
        VALIDATOR_STATE_DB_NAME,
    },
    block_info_db::{
        BlockInfoDb, NEXT1_BLOCK_DB_NAME, NEXT2_BLOCK_DB_NAME, PREV1_BLOCK_DB_NAME,
        PREV2_BLOCK_DB_NAME,
    },
    db::{
        filedb::FileDb,
        rocksdb::{AccessType, RocksDb, CATCHAINS_DB_NAME, NODE_DB_NAME},
    },
    dynamic_boc_rc_db::{AsyncCellsStorageAdapter, DynamicBocDb},
    shard_top_blocks_db::{ShardTopBlocksDb, SHARD_TOP_BLOCKS_DB_NAME},
    shardstate_db_async::{AllowStateGcResolver, CellsDbConfig, Job, ShardStateDb},
    traits::Serializable,
    types::{BlockMeta, PersistentStatePartId, PersistentStatePartKey},
    StorageAlloc, TimeChecker,
};
use ton_block::{
    error, fail, AccountIdPrefixFull, BigBocWriter, Block, BlockIdExt, BocFlags, BocWriter, Cell,
    CellsFactory, CellsStorage, Result, ShardIdent, UInt256, INVALID_WORKCHAIN_ID, MAX_SAFE_DEPTH,
};

/// Full node state keys
pub const INITIAL_MC_BLOCK: &str = "InitMcBlockId";
pub const LAST_APPLIED_MC_BLOCK: &str = "LastMcBlockId";
pub const PSS_KEEPER_MC_BLOCK: &str = "PssKeeperBlockId";
pub const SHARD_CLIENT_MC_BLOCK: &str = "ShardsClientMcBlockId";
pub const ARCHIVES_GC_BLOCK: &str = "ArchivesGcMcBlockId";
pub const LAST_UNNEEDED_KEY_BLOCK: &str = storage::db::rocksdb::LAST_UNNEEDED_KEY_BLOCK;

pub const DB_VERSION: &str = "DbVersion";

pub const DB_VERSION_7: u32 = 7; // with block indexes
pub const CURRENT_DB_VERSION: u32 = DB_VERSION_7;

pub const SHARDSTATE_DB_NAME: &str = "shardstate_db";
const CELLS_CF_NAME: &str = "cells_db_v6";
const CELLSCOUNTERS_CF_NAME: &str = "cells_db_v6_counters";
const SHARD_STATE_PERSISTENT_DB_NAME: &str = "shard_state_persistent_db";
pub const ARCHIVE_STATES_DB_NAME: &str = "archive_states";
pub const ARCHIVE_CELLS_CF_NAME: &str = "archive_cells_db";
pub const ARCHIVE_SHARDSTATE_CF_NAME: &str = "archive_shardstate_db";

/// Validator state keys
pub(crate) const LAST_ROTATION_MC_BLOCK: &str = "LastRotationBlockId";
pub(crate) const DESTROYED_VALIDATOR_SESSIONS: &str = "DestroyedValidatorSessions";

#[derive(Clone, Debug)]
pub enum DataStatus {
    Created, // Just created
    Fetched, // Read from DB as is
    Updated, // Read from DB or created and then updated with new data
}

#[derive(Clone, Debug)]
pub struct BlockResult {
    handle: Arc<BlockHandle>,
    status: DataStatus,
}

impl BlockResult {
    /// Constructor
    pub fn with_status(handle: Arc<BlockHandle>, status: DataStatus) -> Self {
        Self { handle, status }
    }

    /// Any result
    pub fn to_any(&self) -> Arc<BlockHandle> {
        self.handle.clone()
    }

    /// Assert creation
    pub fn _to_created(self) -> Option<Arc<BlockHandle>> {
        match self.status {
            DataStatus::Created => Some(self.handle),
            _ => None,
        }
    }

    /// Assert non-creation
    pub fn to_non_created(&self) -> Option<Arc<BlockHandle>> {
        match self.status {
            DataStatus::Created => None,
            _ => Some(self.handle.clone()),
        }
    }

    /// Assert non-update
    pub fn to_non_updated(&self) -> Option<Arc<BlockHandle>> {
        match self.status {
            DataStatus::Updated => None,
            _ => Some(self.handle.clone()),
        }
    }

    /// Assert update
    pub fn to_updated(&self) -> Option<Arc<BlockHandle>> {
        match self.status {
            DataStatus::Updated => Some(self.handle.clone()),
            _ => None,
        }
    }

    /// Check update
    pub fn _is_updated(&self) -> bool {
        matches!(self.status, DataStatus::Updated)
    }
}

pub mod restore;
pub mod state_gc_resolver;
mod update;

struct SsCallback {
    pub handle: Arc<BlockHandle>,
    pub block_handle_storage: Arc<BlockHandleStorage>,
    pub inner: Option<Arc<dyn storage::shardstate_db_async::Callback>>,
}

impl SsCallback {
    pub fn new(
        handle: Arc<BlockHandle>,
        block_handle_storage: Arc<BlockHandleStorage>,
        inner: Option<Arc<dyn storage::shardstate_db_async::Callback>>,
    ) -> Self {
        Self { handle, block_handle_storage, inner }
    }
}

#[async_trait::async_trait]
impl storage::shardstate_db_async::Callback for SsCallback {
    async fn invoke(&self, job: storage::shardstate_db_async::Job, ok: bool) {
        if ok {
            self.handle.set_state_saved();
            if let Err(e) = self.block_handle_storage.save_handle(&self.handle, None) {
                log::error!("SsCallback: failed to save block handle: {}", e);
            }
        }
        if let Some(inner) = &self.inner {
            inner.invoke(job, ok).await;
        }
    }
}

#[derive(serde::Deserialize, Default)]
pub struct InternalDbConfig {
    pub db_directory: String,
    pub cells_gc_interval_sec: u32,
    pub cells_db_config: CellsDbConfig,
    pub archival_mode: Option<ArchivalModeConfig>,
}

pub enum StateDb {
    Dynamic(Arc<ShardStateDb>),
    Archive(Arc<ArchiveShardStateDb>),
}

impl StateDb {
    pub fn get(&self, id: &BlockIdExt) -> Result<Cell> {
        match self {
            StateDb::Dynamic(db) => db.get(id),
            StateDb::Archive(db) => db.get(id),
        }
    }

    pub fn get_cell(&self, id: &UInt256) -> Result<Cell> {
        match self {
            StateDb::Dynamic(db) => db.get_cell(id),
            StateDb::Archive(db) => db.get_cell(id),
        }
    }

    pub fn cells_factory(&self) -> Result<Arc<dyn CellsFactory>> {
        match self {
            StateDb::Dynamic(db) => db.cells_factory(),
            StateDb::Archive(db) => Ok(db.cells_factory()),
        }
    }

    pub fn create_hashed_cell_storage(
        &self,
        root: Option<&Cell>,
        max_inmemory_cells: usize,
    ) -> Result<Arc<dyn CellsStorage + Send + Sync>> {
        match self {
            StateDb::Dynamic(db) => {
                Ok(Arc::new(db.create_hashed_cell_storage(root, max_inmemory_cells)?))
            }
            StateDb::Archive(db) => {
                Ok(Arc::new(db.create_hashed_cell_storage(root, max_inmemory_cells)?))
            }
        }
    }
}

impl Clone for StateDb {
    fn clone(&self) -> Self {
        match self {
            StateDb::Dynamic(db) => StateDb::Dynamic(db.clone()),
            StateDb::Archive(db) => StateDb::Archive(db.clone()),
        }
    }
}

pub struct InternalDb {
    block_handle_storage: Arc<BlockHandleStorage>,
    prev1_block_db: BlockInfoDb,
    prev2_block_db: BlockInfoDb,
    next1_block_db: BlockInfoDb,
    next2_block_db: BlockInfoDb,
    shard_state_persistent_db: Arc<FileDb>,
    state_db: StateDb,
    archive_manager: Arc<ArchiveManager>,
    shard_top_blocks_db: ShardTopBlocksDb,
    full_node_state_db: Arc<NodeStateDb>,

    config: InternalDbConfig,
    cells_gc_interval: Arc<AtomicU32>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl InternalDb {
    #[allow(clippy::too_many_arguments)]
    pub async fn with_update(
        config: InternalDbConfig,
        restore_db_enabled: bool,
        force_check_db: bool,
        allow_update: bool,
        truncate_db: Option<u32>,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
        is_broken: Option<&AtomicBool>,
        monitor_min_split: Arc<AtomicU8>,
        access_type: Option<AccessType>,
        #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
        allocated: Arc<EngineAlloc>,
    ) -> Result<Self> {
        let mut db = Self::construct(
            config,
            allow_update,
            monitor_min_split,
            access_type,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )
        .await?;
        let version = db.resolve_db_version()?;
        if version != CURRENT_DB_VERSION {
            if allow_update {
                db = update::update(
                    db,
                    version,
                    check_stop,
                    is_broken,
                    force_check_db,
                    restore_db_enabled,
                )
                .await?
            } else {
                fail!(
                    "DB version {} does not correspond to current supported one {}.",
                    version,
                    CURRENT_DB_VERSION
                )
            }
        } else if let Some(mc_seqno) = truncate_db {
            let (id, _) = db
                .lookup_block_by_seqno(&AccountIdPrefixFull::any_masterchain(), mc_seqno)
                .await?
                .ok_or_else(|| {
                    error!("there is no block with seqno {} in masterchain", mc_seqno)
                })?;
            log::info!("Truncating database at block {}", id);
            db.truncate_database(&id).await?;
        } else {
            log::info!("DB VERSION {}", version);
            // TODO correct workchain id needed here, but it will be known later
            db = check_db(db, 0, restore_db_enabled, force_check_db, check_stop, is_broken).await?;
        }
        Ok(db)
    }

    async fn construct(
        config: InternalDbConfig,
        _allow_update: bool,
        monitor_min_split: Arc<AtomicU8>,
        access_type: Option<AccessType>,
        #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
        allocated: Arc<EngineAlloc>,
    ) -> Result<Self> {
        let mut cfs_opts = HashMap::new();
        if config.archival_mode.is_none() {
            cfs_opts.insert(
                CELLS_CF_NAME.to_string(),
                DynamicBocDb::build_cells_cf_options(&config.cells_db_config),
            );
            cfs_opts.insert(
                CELLSCOUNTERS_CF_NAME.to_string(),
                DynamicBocDb::build_counters_cf_options(&config.cells_db_config),
            );
        }
        let access_type = access_type.unwrap_or(AccessType::ReadWrite);
        let can_create_db = access_type == AccessType::ReadWrite;
        let db = RocksDb::new(
            config.db_directory.as_str(),
            NODE_DB_NAME,
            cfs_opts,
            access_type.clone(),
        )?;
        let db_catchain = RocksDb::new(
            config.db_directory.as_str(),
            CATCHAINS_DB_NAME,
            None,
            access_type.clone(),
        )?;
        let block_handle_db =
            Arc::new(BlockHandleDb::with_db(db.clone(), BLOCK_HANDLE_DB_NAME, can_create_db)?);
        let full_node_state_db = Arc::new(NodeStateDb::with_db(
            db.clone(),
            storage::db::rocksdb::NODE_STATE_DB_NAME,
            can_create_db,
        )?);
        let validator_state_db =
            Arc::new(NodeStateDb::with_db(db_catchain, VALIDATOR_STATE_DB_NAME, can_create_db)?);
        let block_handle_storage = Arc::new(BlockHandleStorage::with_dbs(
            block_handle_db.clone(),
            full_node_state_db.clone(),
            validator_state_db,
            #[cfg(feature = "telemetry")]
            telemetry.storage.clone(),
            allocated.storage.clone(),
        ));

        let state_db = if config.archival_mode.is_some() {
            let states_db = RocksDb::new(
                &config.db_directory,
                ARCHIVE_STATES_DB_NAME,
                std::collections::HashMap::from([(
                    ARCHIVE_CELLS_CF_NAME.to_string(),
                    storage::cell_db::CellDb::build_cf_options(
                        config.cells_db_config.cells_cache_size_bytes,
                    ),
                )]),
                access_type.clone(),
            )?;
            StateDb::Archive(Arc::new(ArchiveShardStateDb::new(
                states_db,
                ARCHIVE_SHARDSTATE_CF_NAME,
                ARCHIVE_CELLS_CF_NAME,
                &config.cells_db_config,
                #[cfg(feature = "telemetry")]
                telemetry.storage.clone(),
                allocated.storage.clone(),
            )?))
        } else {
            StateDb::Dynamic(Self::create_shard_state_dynamic_db(
                db.clone(),
                &config,
                #[cfg(feature = "telemetry")]
                telemetry.storage.clone(),
                allocated.storage.clone(),
            )?)
        };
        let last_unneeded_key_block_id =
            block_handle_storage.load_full_node_state(LAST_UNNEEDED_KEY_BLOCK)?.unwrap_or_default();
        let db_root_path = Arc::new(PathBuf::from(&config.db_directory));
        let db_provider: Arc<dyn ArchiveDbProvider> =
            if let Some(ref archival_config) = config.archival_mode {
                let router = Arc::new(EpochRouter::new(archival_config).await?);
                Arc::new(EpochDbProvider::new(router))
            } else {
                Arc::new(SingleDbProvider::new(db.clone(), db_root_path.clone()))
            };
        let archive_manager = Arc::new(
            ArchiveManager::with_data(
                db.clone(),
                db_root_path,
                db_provider,
                last_unneeded_key_block_id.seq_no(),
                monitor_min_split,
                #[cfg(feature = "telemetry")]
                telemetry.storage.clone(),
                allocated.storage.clone(),
            )
            .await?,
        );

        let db = Self {
            block_handle_storage,
            prev1_block_db: BlockInfoDb::with_db(db.clone(), PREV1_BLOCK_DB_NAME, can_create_db)?,
            prev2_block_db: BlockInfoDb::with_db(db.clone(), PREV2_BLOCK_DB_NAME, can_create_db)?,
            next1_block_db: BlockInfoDb::with_db(db.clone(), NEXT1_BLOCK_DB_NAME, can_create_db)?,
            next2_block_db: BlockInfoDb::with_db(db.clone(), NEXT2_BLOCK_DB_NAME, can_create_db)?,
            shard_state_persistent_db: Arc::new(FileDb::with_path(
                Path::new(config.db_directory.as_str()).join(SHARD_STATE_PERSISTENT_DB_NAME),
            )),
            state_db,
            archive_manager,
            shard_top_blocks_db: ShardTopBlocksDb::with_db(
                db.clone(),
                SHARD_TOP_BLOCKS_DB_NAME,
                can_create_db,
            )?,
            full_node_state_db,
            cells_gc_interval: Arc::new(AtomicU32::new(config.cells_gc_interval_sec)),
            config,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        };

        Ok(db)
    }

    fn resolve_db_version(&self) -> Result<u32> {
        if self.block_handle_storage.is_empty()? {
            self.store_db_version(CURRENT_DB_VERSION)?;
            Ok(CURRENT_DB_VERSION)
        } else {
            self.load_db_version()
        }
    }

    fn store_db_version(&self, v: u32) -> Result<()> {
        self.full_node_state_db.put(&DB_VERSION, &v.serialize())
    }

    pub fn load_db_version(&self) -> Result<u32> {
        let db_slice = self.full_node_state_db.get(&DB_VERSION)?;
        u32::deserialize(db_slice.as_ref())
    }

    fn create_shard_state_dynamic_db(
        db: Arc<RocksDb>,
        config: &InternalDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Arc<ShardStateDb>> {
        ShardStateDb::new(
            db,
            SHARDSTATE_DB_NAME,
            CELLS_CF_NAME,
            CELLSCOUNTERS_CF_NAME,
            config.cells_db_config.clone(),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )
    }

    /// Returns approximate RocksDB memory usage summing main DB,
    /// archive states DB (if archival), and all epoch DBs.
    pub fn rocksdb_memory_usage(&self) -> storage::RocksDbMemoryUsage {
        let mut usage = self.archive_manager.rocksdb_memory_usage();
        if let StateDb::Archive(db) = &self.state_db {
            usage += db.rocksdb_memory_usage();
        }
        usage
    }

    pub fn start_states_gc(&self, resolver: Arc<dyn AllowStateGcResolver>) {
        if let StateDb::Dynamic(db) = &self.state_db {
            db.clone().start_gc(resolver, self.cells_gc_interval.clone())
        }
    }

    pub async fn stop_states_db(&self) {
        if let StateDb::Dynamic(db) = &self.state_db {
            db.stop().await
        }
    }

    fn store_block_handle(
        &self,
        handle: &Arc<BlockHandle>,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("store_block_handle {}", handle.id()), 30);
        self.block_handle_storage.save_handle(handle, callback)
    }

    fn load_block_linkage(
        &self,
        id: &BlockIdExt,
        db: &BlockInfoDb,
        msg: &str,
    ) -> Result<Option<BlockIdExt>> {
        let _tc = TimeChecker::new(format!("{} {}", msg, id), 30);
        let Some(bytes) = db.try_get(id)? else { return Ok(None) };
        Ok(Some(BlockIdExt::deserialize(&bytes)?))
    }

    #[allow(clippy::too_many_arguments)]
    fn store_block_linkage(
        &self,
        handle: &Arc<BlockHandle>,
        linkage: &BlockIdExt,
        db: &BlockInfoDb,
        msg: &str,
        check_has: impl Fn(&Arc<BlockHandle>) -> bool,
        check_set: impl Fn(&Arc<BlockHandle>) -> bool,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("{} {}", msg, handle.id()), 30);
        if !check_has(handle) {
            db.put(handle.id(), &linkage.serialize())?;
            if check_set(handle) {
                self.store_block_handle(handle, callback)?;
            }
        }
        Ok(())
    }

    pub fn create_or_load_block_handle(
        &self,
        id: &BlockIdExt,
        block: Option<&Block>,
        utime: Option<u32>,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<BlockResult> {
        let _tc = TimeChecker::new(format!("create_or_load_block_handle {id}"), 30);

        if let Some(handle) = self.load_block_handle(id)? {
            return Ok(BlockResult::with_status(handle, DataStatus::Fetched));
        }
        let meta = if let Some(block) = block {
            BlockMeta::from_block(block)?
        } else if id.seq_no == 0 {
            if let Some(utime) = utime {
                BlockMeta::with_data(0, utime, 0, 0, INVALID_WORKCHAIN_ID as u32)
            } else {
                fail!("Cannot create handle for zero block {id} without UNIX time")
            }
        } else {
            fail!("Cannot create handle for block {id} without data")
        };
        if let Some(handle) = self.block_handle_storage.create_handle(id.clone(), meta, callback)? {
            Ok(BlockResult::with_status(handle, DataStatus::Created))
        } else if let Some(handle) = self.load_block_handle(id)? {
            Ok(BlockResult::with_status(handle, DataStatus::Fetched))
        } else {
            fail!("Cannot create handle for block {id}")
        }
    }

    pub fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        let _tc = TimeChecker::new(format!("load_block_handle {}", id), 30);
        self.block_handle_storage.load_handle_by_id(id)
    }

    pub async fn store_block_data(
        &self,
        block: &BlockStuff,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<BlockResult> {
        let _tc = TimeChecker::new(format!("store_block_data {}", block.id()), 100);
        let mut result = self.create_or_load_block_handle(
            block.id(),
            Some(block.block()?),
            None,
            callback.clone(),
        )?;
        let handle = result.clone().to_non_updated().ok_or_else(|| {
            error!(
                "INTERNAL ERROR: block {} result mismatch in store_block_data {:?}",
                block.id(),
                result
            )
        })?;
        let entry_id = PackageEntryId::Block(block.id());
        if !handle.has_data() || !self.archive_manager.check_file(&handle, &entry_id) {
            let _lock = handle.block_file_lock().write().await;
            if !handle.has_data() || !self.archive_manager.check_file(&handle, &entry_id) {
                self.archive_manager.add_file(&entry_id, block.data()).await?;
                if handle.set_data() {
                    self.store_block_handle(&handle, callback)?;
                    result = BlockResult::with_status(handle.clone(), DataStatus::Updated)
                }
            }
        }
        Ok(result)
    }

    pub async fn load_block_data(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        let _tc = TimeChecker::new(format!("load_block_data {}", handle.id()), 100);
        let raw_block = self.load_block_data_raw(handle).await?;
        BlockStuff::deserialize_block(handle.id().clone(), Arc::new(raw_block))
    }

    pub async fn load_block_data_raw(&self, handle: &BlockHandle) -> Result<Vec<u8>> {
        let _tc = TimeChecker::new(format!("load_block_data_raw {}", handle.id()), 100);
        if !handle.has_data() {
            fail!("This block is not stored yet: {:?}", handle);
        }
        let entry_id = PackageEntryId::Block(handle.id());
        self.archive_manager.get_file(handle, &entry_id).await
    }

    pub async fn lookup_block_by_seqno(
        &self,
        prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        let _tc = TimeChecker::new(format!("lookup_block_by_seqno {} {}", prefix, seqno), 20);
        self.archive_manager.lookup_block_by_seqno(prefix, seqno).await
    }

    pub async fn lookup_block_by_lt(
        &self,
        prefix: &AccountIdPrefixFull,
        lt: u64,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        let _tc = TimeChecker::new(format!("lookup_block_by_lt {} {}", prefix, lt), 20);
        self.archive_manager.lookup_block_by_lt(prefix, lt).await
    }

    pub async fn lookup_blocks_by_utime<'a>(
        &self,
        prefix: &AccountIdPrefixFull,
        utime: u32,
        f: Box<dyn FnMut(BlockIdExt, Vec<u8>) -> Result<bool> + Send + 'a>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("lookup_blocks_by_utime {} {}", prefix, utime), 20);
        self.archive_manager.lookup_blocks_by_utime(prefix, utime, f).await
    }

    pub async fn store_block_proof(
        &self,
        id: &BlockIdExt,
        handle: Option<Arc<BlockHandle>>,
        proof: &BlockProofStuff,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<BlockResult> {
        let _tc = TimeChecker::new(format!("store_block_proof {}", proof.id()), 100);

        if let Some(handle) = &handle {
            if handle.id() != id {
                fail!("Block handle and id mismatch: {} vs {}", handle.id(), id)
            }
        }
        if id != proof.id() {
            fail!(NodeError::InvalidArg("`proof` and `id` mismatch".to_string()))
        }

        let mut result = if let Some(handle) = handle {
            BlockResult::with_status(handle, DataStatus::Fetched)
        } else {
            let (virt_block, _) = proof.virtualize_block()?;
            self.create_or_load_block_handle(id, Some(&virt_block), None, callback.clone())?
        };
        let handle = result.clone().to_non_updated().ok_or_else(|| {
            error!("INTERNAL ERROR: block {} result mismatch in store_block_proof", id)
        })?;
        if proof.is_link() {
            let entry_id = PackageEntryId::ProofLink(id);
            if !handle.has_proof_link() || !self.archive_manager.check_file(&handle, &entry_id) {
                let _lock = handle.proof_file_lock().write().await;
                if !handle.has_proof_link() || !self.archive_manager.check_file(&handle, &entry_id)
                {
                    let entry_id = PackageEntryId::ProofLink(id);
                    self.archive_manager.add_file(&entry_id, proof.data()).await?;
                    if handle.set_proof_link() {
                        self.store_block_handle(&handle, callback)?;
                        result = BlockResult::with_status(handle.clone(), DataStatus::Updated)
                    }
                }
            }
        } else {
            let entry_id = PackageEntryId::Proof(id);
            if !handle.has_proof() || !self.archive_manager.check_file(&handle, &entry_id) {
                let _lock = handle.proof_file_lock().write().await;
                if !handle.has_proof() || !self.archive_manager.check_file(&handle, &entry_id) {
                    let entry_id = PackageEntryId::Proof(id);
                    self.archive_manager.add_file(&entry_id, proof.data()).await?;
                    if handle.set_proof() {
                        self.store_block_handle(&handle, callback)?;
                        result = BlockResult::with_status(handle.clone(), DataStatus::Updated)
                    }
                }
            }
        }
        Ok(result)
    }

    pub async fn load_block_proof(
        &self,
        handle: &BlockHandle,
        is_link: bool,
    ) -> Result<BlockProofStuff> {
        let _tc = TimeChecker::new(
            format!("load_block_proof {} {}", if is_link { "link" } else { "" }, handle.id()),
            100,
        );
        let raw_proof = self.load_block_proof_raw_(handle, is_link).await?;
        BlockProofStuff::deserialize(handle.id(), raw_proof, is_link)
    }

    pub async fn load_block_proof_raw(
        &self,
        handle: &BlockHandle,
        is_link: bool,
    ) -> Result<Vec<u8>> {
        let _tc = TimeChecker::new(
            format!("load_block_proof_raw {} {}", if is_link { "link" } else { "" }, handle.id()),
            100,
        );
        self.load_block_proof_raw_(handle, is_link).await
    }

    async fn load_block_proof_raw_(&self, handle: &BlockHandle, is_link: bool) -> Result<Vec<u8>> {
        let (entry_id, inited) = if is_link {
            (PackageEntryId::ProofLink(handle.id()), handle.has_proof_link())
        } else {
            (PackageEntryId::Proof(handle.id()), handle.has_proof())
        };
        if !inited {
            fail!(
                "This proof{} is not in the archive: {:?}",
                if is_link { "link" } else { "" },
                handle
            );
        }
        self.archive_manager.get_file(handle, &entry_id).await
    }

    pub async fn store_shard_state_dynamic(
        &self,
        handle: &Arc<BlockHandle>,
        state: &Arc<ShardStateStuff>,
        callback_handle: Option<Arc<dyn block_handle_db::Callback>>,
        callback_ss: Option<Arc<dyn storage::shardstate_db_async::Callback>>,
        force: bool,
    ) -> Result<(Arc<ShardStateStuff>, bool)> {
        let timeout = 30;
        let _tc =
            TimeChecker::new(format!("store_shard_state_dynamic {}", state.block_id()), timeout);

        if handle.id() != state.block_id() {
            fail!(NodeError::InvalidArg("`state` and `handle` mismatch".to_string()))
        }
        let _lock = handle.saving_state_lock().lock().await;
        if force || !handle.has_saved_state() {
            match &self.state_db {
                StateDb::Archive(db) => {
                    let state = state.clone();
                    let db = db.clone();
                    let state_root = state.root_cell().clone();
                    tokio::task::spawn_blocking(move || {
                        db.put(state.block_id(), state.root_cell().clone())
                    })
                    .await??;
                    if let Some(callback) = callback_ss {
                        callback.invoke(Job::PutState(state_root, handle.id().clone()), true).await;
                    }
                    if handle.set_state() | handle.set_state_saved() {
                        self.store_block_handle(handle, callback_handle)?;
                    }
                }
                StateDb::Dynamic(db) => {
                    let callback = SsCallback::new(
                        handle.clone(),
                        self.block_handle_storage.clone(),
                        callback_ss,
                    );
                    let callback =
                        Some(Arc::new(callback) as Arc<dyn storage::shardstate_db_async::Callback>);
                    db.put(state.block_id(), state.root_cell().clone(), callback).await?;
                    if handle.set_state() {
                        self.store_block_handle(handle, callback_handle)?;
                    }
                }
            }
            Ok((state.clone(), true))
        } else {
            Ok((self.load_shard_state_dynamic(handle.id())?, false))
        }
    }

    pub async fn store_state_update(
        &self,
        handle: &Arc<BlockHandle>,
        state_update: Cell,
    ) -> Result<()> {
        let timeout = 30;
        let _tc = TimeChecker::new(format!("store_state_update {}", handle.id()), timeout);

        let _lock = handle.saving_state_lock().lock().await;
        if !handle.has_saved_state() {
            match &self.state_db {
                StateDb::Archive(db) => {
                    let db = db.clone();
                    let id = handle.id().clone();
                    tokio::task::spawn_blocking(move || db.put_update(&id, state_update)).await??;
                    if handle.set_state() | handle.set_state_saved() {
                        self.store_block_handle(handle, None)?;
                    }
                }
                _ => {
                    fail!("store_state_update is only supported in archival mode")
                }
            }
        }

        Ok(())
    }

    pub fn load_shard_state_dynamic(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        let _tc = TimeChecker::new(format!("load_shard_state_dynamic {}", id), 30);

        let handle = self
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for block {}", id))?;

        if !handle.has_saved_state() {
            fail!("ShardState is not saved for {}", id);
        }

        let root_cell = self.state_db.get(handle.id())?;

        ShardStateStuff::from_root_cell(
            handle.id().clone(),
            root_cell,
            #[cfg(feature = "telemetry")]
            &self.telemetry,
            &self.allocated,
        )
    }

    pub fn load_cell(&self, id: &UInt256) -> Result<Cell> {
        let _tc = TimeChecker::new(format!("load_cell {}", id), 30);
        self.state_db.get_cell(id)
    }

    pub fn shard_state_persistent_write_obj(
        &self,
        id: &PersistentStatePartId,
    ) -> Result<impl Write + Seek> {
        let id: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.get_write_object(&id)
    }

    pub fn finalize_shard_state_persistent(
        &self,
        id: &PersistentStatePartId,
        _obj: impl Write + Seek,
    ) -> Result<()> {
        let id: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.finalize_write_object(&id)
    }

    #[allow(dead_code)]
    pub fn shard_state_persistent_read_obj(
        &self,
        id: &PersistentStatePartId,
    ) -> Result<impl Read + Seek> {
        let id: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.get_read_object(&id)
    }

    pub async fn store_shard_state_persistent_part_fast(
        &self,
        handle: &Arc<BlockHandle>,
        id: &PersistentStatePartId,
        root: Cell,
        abort: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<()> {
        log::info!("store_shard_state_persistent_part_fast {}", id);
        let shard_state_persistent_db = self.shard_state_persistent_db.clone();
        let db_key: PersistentStatePartKey = id.into();
        let id_owned = id.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let now = std::time::Instant::now();
            let writer =
                BocWriter::with_params([root], MAX_SAFE_DEPTH, BocFlags::all(), abort.deref())?;
            let cells_count = writer.cells_count();
            let arrange_time = now.elapsed();
            let mut dest = shard_state_persistent_db.get_write_object(&db_key)?;
            writer.write(&mut dest)?;
            let write_time = now.elapsed();
            log::info!(
                "store_shard_state_persistent_part_fast {id_owned} DONE; cells {cells_count}, \
                TIME: arrange {arrange_time:#?}, write {:#?}, total {write_time:#?}",
                write_time - arrange_time
            );
            drop(dest);
            shard_state_persistent_db.finalize_write_object(&db_key)?;
            Ok(())
        })
        .await??;
        if (id.is_whole_state() || id.is_head()) && handle.set_persistent_state() {
            self.store_block_handle(handle, None)?;
        }
        Ok(())
    }

    pub async fn store_shard_state_persistent_part(
        &self,
        handle: &Arc<BlockHandle>,
        id: &PersistentStatePartId,
        part: Cell,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
        abort: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<()> {
        log::info!("store_shard_state_persistent_part {}", id);
        if handle.id() != id.block_id() {
            fail!("part id and handle id mismatch")
        }
        if handle.has_persistent_state() {
            log::info!("store_shard_state_persistent_part {}: already saved", id);
        } else {
            tokio::task::spawn_blocking({
                let state_db = self.state_db.clone();
                let shard_state_persistent_db = self.shard_state_persistent_db.clone();
                let db_key: PersistentStatePartKey = id.into();
                let id = id.clone();

                move || -> Result<()> {
                    log::debug!(
                        "store_shard_state_persistent_part {id}, root: {:x}",
                        part.repr_hash()
                    );
                    let now = std::time::Instant::now();
                    let mut dest = shard_state_persistent_db.get_write_object(&db_key)?;
                    if id.is_head() {
                        // Header's cells aren't stored in cells db, so we use simple boc writer
                        let writer = BocWriter::with_params(
                            [part],
                            MAX_SAFE_DEPTH,
                            BocFlags::all(),
                            abort.deref(),
                        )?;
                        writer.write(&mut dest)?;
                        log::info!("store_shard_state_persistent_part (head) {} DONE", id);
                    } else {
                        let max_inmemory_cells = 100;

                        // Other parts' cells are stored in cells db, so we use big boc writer
                        // which is optimized to use existing cells key-value storage

                        // In case of pss part the root cell and some number of refs
                        // may not be stored in cells db, due to hashmap split procedure.
                        // So we pass the root into the adapter.
                        // The adapter determines which cells are not stored in the DB
                        // and remembers their data in memory.
                        // The adapter does not store the cell (don't keep references), only data.
                        // The maximum number of cells to store in memory is limited
                        let cells_storage =
                            state_db.create_hashed_cell_storage(Some(&part), max_inmemory_cells)?;
                        let writer = BigBocWriter::with_params(
                            [part],
                            MAX_SAFE_DEPTH,
                            BocFlags::all(),
                            abort.deref(),
                            cells_storage,
                        )?;
                        let arrange_time = now.elapsed();
                        let cells_count = writer.cells_count();
                        writer.write(&mut dest)?;
                        let total_time = now.elapsed();
                        log::info!(
                            "store_shard_state_persistent_part {} DONE; \
                        cells {}, TIME: arrange {:#?}, write {:#?}, total {:#?}",
                            id,
                            cells_count,
                            arrange_time,
                            total_time - arrange_time,
                            total_time
                        );
                        metrics::histogram!("ton_node_db_persistent_state_write_seconds")
                            .record(total_time);
                    }
                    drop(dest);
                    shard_state_persistent_db.finalize_write_object(&db_key)?;
                    Ok(())
                }
            })
            .await??;
            if (id.is_whole_state() || id.is_head()) && handle.set_persistent_state() {
                self.store_block_handle(handle, callback)?;
            }
        }
        Ok(())
    }

    pub async fn store_shard_state_persistent_raw(
        &self,
        handle: &Arc<BlockHandle>,
        state_data: &[u8],
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(
            format!("store_shard_state_persistent_raw {}", handle.id()),
            state_data.len() as u64 / 1000 + 10,
        );
        if !handle.has_persistent_state() {
            self.shard_state_persistent_db.write_whole_file(handle.id(), state_data).await?;
            if handle.set_persistent_state() {
                self.store_block_handle(handle, callback)?;
            }
        }
        Ok(())
    }

    pub async fn load_shard_state_persistent_slice(
        &self,
        id: &PersistentStatePartId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        let key: PersistentStatePartKey = id.into();
        let _tc = TimeChecker::new(format!("load_shard_state_persistent_slice {}", id), 200);
        let full_lenth = self.load_shard_state_persistent_size(id).await?;
        if offset > full_lenth {
            fail!("offset is greater than full length");
        }
        if offset == full_lenth {
            Ok(vec![])
        } else {
            let length = min(length, full_lenth - offset);
            let data = self.shard_state_persistent_db.read_file_part(&key, offset, length).await?;
            Ok(data)
        }
    }

    pub async fn load_shard_state_persistent_to(
        &self,
        id: &PersistentStatePartId,
        dest: &mut Vec<u8>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("load_shard_state_persistent_to {}", id), 1000);
        let id: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.read_whole_file_to(&id, dest).await
    }

    pub fn load_shard_state_persistent_obj(
        &self,
        id: &PersistentStatePartId,
    ) -> Result<impl Read + Seek> {
        let _tc = TimeChecker::new(format!("load_shard_state_persistent_obj {}", id), 10);
        let key: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.get_read_object(&key)
    }

    pub async fn load_shard_state_persistent_size(
        &self,
        id: &PersistentStatePartId,
    ) -> Result<u64> {
        let _tc = TimeChecker::new(format!("load_shard_state_persistent_size {}", id), 50);
        let id: PersistentStatePartKey = id.into();
        self.shard_state_persistent_db.get_file_size(&id).await
    }

    pub async fn cleanup_shard_states_persistent(&self) -> Result<()> {
        let _tc = TimeChecker::new("shard_state_persistent_gc".to_string(), 5000);
        self.shard_state_persistent_db.cleanup_tmp()?;
        Ok(())
    }

    pub async fn shard_state_persistent_gc(
        &self,
        calc_ttl: impl Fn(u32) -> (u32, bool),
        zerostate_id: &BlockIdExt,
    ) -> Result<()> {
        let _tc = TimeChecker::new("shard_state_persistent_gc".to_string(), 5000);
        let mut for_delete = HashSet::new();
        self.shard_state_persistent_db.for_each_key(&mut |key| {
            let root_hash = UInt256::from(&key[..32]);

            if &root_hash == zerostate_id.root_hash() {
                log::info!("  Zerostate: {:x}", zerostate_id.root_hash());
                return Ok(true);
            }

            let convert_to_utc = |t| {
                chrono::prelude::DateTime::<chrono::Utc>::from(
                    UNIX_EPOCH + Duration::from_secs(t as u64),
                )
                .naive_utc()
            };

            match self.block_handle_storage.load_handle_by_root_hash(&root_hash)? {
                None => {
                    log::warn!("shard_state_persistent_gc: can't load handle for {:x}", root_hash)
                }
                Some(handle) => {
                    let gen_utime = handle.gen_utime();
                    let (ttl, expired) = calc_ttl(gen_utime);
                    log::info!(
                        "{} Persistent state: {:x}, mc block: {}, \
                        gen_utime: {} UTC ({}), expired at: {} UTC ({})",
                        if expired { "X" } else { " " },
                        root_hash,
                        handle.masterchain_ref_seq_no(),
                        convert_to_utc(gen_utime),
                        handle.gen_utime(),
                        convert_to_utc(ttl),
                        ttl
                    );
                    if expired {
                        for_delete.insert(handle.id().clone());
                    }
                }
            }
            Ok(true)
        })?;

        for id in for_delete {
            match self.shard_state_persistent_db.delete_file(&id).await {
                Ok(_) => log::debug!("shard_state_persistent_gc: {:x} deleted", id.root_hash()),
                Err(e) => log::warn!(
                    "shard_state_persistent_gc: can't delete {:x}: {}",
                    id.root_hash(),
                    e
                ),
            }
        }

        Ok(())
    }

    pub fn store_block_prev1(
        &self,
        handle: &Arc<BlockHandle>,
        prev: &BlockIdExt,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        self.store_block_linkage(
            handle,
            prev,
            &self.prev1_block_db,
            "store_block_prev1",
            |handle| handle.has_prev1(),
            |handle| handle.set_prev1(),
            callback,
        )
    }

    pub fn load_block_prev1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.load_block_linkage(id, &self.prev1_block_db, "load_block_prev1")?
            .ok_or_else(|| error!("No prev1 block for {}", id))
    }

    pub fn store_block_prev2(
        &self,
        handle: &Arc<BlockHandle>,
        prev2: &BlockIdExt,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        self.store_block_linkage(
            handle,
            prev2,
            &self.prev2_block_db,
            "store_block_prev2",
            |handle| handle.has_prev2(),
            |handle| handle.set_prev2(),
            callback,
        )
    }

    pub fn load_block_prev2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.load_block_linkage(id, &self.prev2_block_db, "load_block_prev2")
    }

    pub fn store_block_next1(
        &self,
        handle: &Arc<BlockHandle>,
        next: &BlockIdExt,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        self.store_block_linkage(
            handle,
            next,
            &self.next1_block_db,
            "store_block_next1",
            |handle| handle.has_next1(),
            |handle| handle.set_next1(),
            callback,
        )
    }

    pub fn load_block_next1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.load_block_linkage(id, &self.next1_block_db, "load_block_next1")?
            .ok_or_else(|| error!("No next1 block for {}", id))
    }

    pub fn store_block_next2(
        &self,
        handle: &Arc<BlockHandle>,
        next2: &BlockIdExt,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        self.store_block_linkage(
            handle,
            next2,
            &self.next2_block_db,
            "store_block_next2",
            |handle| handle.has_next2(),
            |handle| handle.set_next2(),
            callback,
        )
    }

    pub fn load_block_next2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.load_block_linkage(id, &self.next2_block_db, "load_block_next2")
    }

    pub fn store_block_applied(
        &self,
        handle: &Arc<BlockHandle>,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<bool> {
        let _tc = TimeChecker::new(format!("store_block_applied {}", handle.id()), 30);
        if handle.set_block_applied() {
            self.store_block_handle(handle, callback)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn archive_block(
        &self,
        id: &BlockIdExt,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("archive_block {}", id), 200);
        let handle = self
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for archiving block {}", id))?;
        if handle.is_archived() {
            return Ok(());
        }
        self.archive_manager
            .move_to_archive(&handle, || {
                if handle.set_archived() {
                    self.store_block_handle(&handle, callback.clone())?;
                }
                Ok(())
            })
            .await
            .map_err(|err| error!("Failed to move block to archive: {}. Error: {}", id, err))
    }

    #[allow(dead_code)]
    pub fn drop_full_node_state(&self, key: &'static str) -> Result<()> {
        let _tc = TimeChecker::new(format!("drop_full_node_state {}", key), 30);
        self.block_handle_storage.drop_full_node_state(key.to_string())
    }

    pub fn load_full_node_state(&self, key: &'static str) -> Result<Option<Arc<BlockIdExt>>> {
        let _tc = TimeChecker::new(format!("load_full_node_state {}", key), 30);
        self.block_handle_storage.load_full_node_state(key)
    }

    pub fn save_full_node_state(&self, key: &'static str, block_id: &BlockIdExt) -> Result<()> {
        let _tc = TimeChecker::new(format!("save_full_node_state {}", key), 30);
        self.block_handle_storage.save_full_node_state(key.to_string(), block_id)
    }

    pub fn drop_validator_state(&self, key: &'static str) -> Result<()> {
        let _tc = TimeChecker::new(format!("drop_validator_state {}", key), 30);
        self.block_handle_storage.drop_validator_state(key.to_string())
    }

    pub fn load_validator_state(&self, key: &'static str) -> Result<Option<Arc<BlockIdExt>>> {
        let _tc = TimeChecker::new(format!("load_validator_state {}", key), 30);
        self.block_handle_storage.load_validator_state(key)
    }

    pub fn save_validator_state(&self, key: &'static str, block_id: &BlockIdExt) -> Result<()> {
        let _tc = TimeChecker::new(format!("save_validator_state {}", key), 30);
        self.block_handle_storage.save_validator_state(key.to_string(), block_id)
    }

    pub fn drop_validator_state_raw(&self, key: &'static str) -> Result<()> {
        let _tc = TimeChecker::new(format!("drop_validator_state_raw {}", key), 30);
        self.block_handle_storage.drop_validator_state_raw(key)
    }

    pub fn load_validator_state_raw(&self, key: &'static str) -> Result<Option<Vec<u8>>> {
        let _tc = TimeChecker::new(format!("load_validator_state_raw {}", key), 30);
        self.block_handle_storage.load_validator_state_raw(key)
    }

    pub fn save_validator_state_raw(&self, key: &'static str, data: &[u8]) -> Result<()> {
        let _tc = TimeChecker::new(format!("save_validator_state_raw {}", key), 30);
        self.block_handle_storage.save_validator_state_raw(key, data)
    }

    pub async fn get_archive_id(&self, mc_seq_no: u32, shard: &ShardIdent) -> Option<u64> {
        let _tc = TimeChecker::new(format!("get_archive_id {mc_seq_no} {shard}"), 30);
        self.archive_manager.get_archive_id(mc_seq_no, shard).await
    }

    pub async fn get_archive_slice(
        &self,
        archive_id: u64,
        offset: u64,
        limit: u32,
    ) -> Result<Vec<u8>> {
        let _tc = TimeChecker::new(
            format!("get_archive_slice id: {}, offset: {}, limit: {}", archive_id, offset, limit),
            300,
        );
        self.archive_manager.get_archive_slice(archive_id, offset, limit).await
    }

    pub async fn clean_unapplied_files(&self, ids: &[BlockIdExt]) {
        let _tc = TimeChecker::new("clean_unapplied_files".to_owned(), 300);
        self.archive_manager.clean_unapplied_files(ids).await;
    }

    pub async fn archive_gc(&self, last_unneeded_key_block: &BlockIdExt) -> Result<()> {
        let _tc = TimeChecker::new(format!("archive_gc {}", last_unneeded_key_block), 300);
        self.archive_manager.gc(last_unneeded_key_block).await;
        self.save_full_node_state(LAST_UNNEEDED_KEY_BLOCK, last_unneeded_key_block)
    }

    pub fn assign_mc_ref_seq_no(
        &self,
        handle: &Arc<BlockHandle>,
        mc_seq_no: u32,
        callback: Option<Arc<dyn block_handle_db::Callback>>,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("assign_mc_ref_seq_no {}", handle.id()), 30);
        if handle.set_masterchain_ref_seq_no(mc_seq_no)? {
            self.store_block_handle(handle, callback)?;
        }
        Ok(())
    }

    pub fn save_top_shard_block(
        &self,
        id: &TopBlockDescrId,
        tsb: &TopBlockDescrStuff,
    ) -> Result<()> {
        let _tc = TimeChecker::new(format!("save_top_shard_block {}", id), 50);
        self.shard_top_blocks_db.put(&id.to_bytes()?, &tsb.to_bytes()?)
    }

    pub fn load_all_top_shard_blocks(
        &self,
    ) -> Result<HashMap<TopBlockDescrId, TopBlockDescrStuff>> {
        let _tc = TimeChecker::new("load_all_top_shard_blocks".to_string(), 100);
        let mut result = HashMap::<TopBlockDescrId, TopBlockDescrStuff>::new();

        let mut invalid_entries = Vec::new();
        self.shard_top_blocks_db.for_each(&mut |id_bytes, tsb_bytes| {
            let id = TopBlockDescrId::from_bytes(id_bytes);
            let tbds = TopBlockDescrStuff::from_bytes(tsb_bytes, false);

            match &id {
                Ok(id) => {
                    if let Err(e) = &tbds {
                        log::error!("Skipping invalid top block description for {id}: {e:?}");
                    }
                }
                Err(e) => log::error!("Skipping invalid top block description: {e:?}"),
            }

            if let (Ok(id), Ok(tbds)) = (id, tbds) {
                result.insert(id, tbds);
            } else {
                invalid_entries.push(id_bytes.to_owned());
            }
            Ok(true)
        })?;

        for id in invalid_entries {
            self.shard_top_blocks_db.delete(&id)?;
        }

        Ok(result)
    }

    #[cfg(test)]
    pub fn load_all_top_shard_blocks_raw(&self) -> Result<HashMap<TopBlockDescrId, Vec<u8>>> {
        let _tc = TimeChecker::new("load_all_top_shard_blocks_raw".to_string(), 100);
        let mut result = HashMap::<TopBlockDescrId, Vec<u8>>::new();
        self.shard_top_blocks_db.for_each(&mut |id_bytes, tsb_bytes| {
            let id = TopBlockDescrId::from_bytes(id_bytes)?;
            result.insert(id, tsb_bytes.to_vec());
            Ok(true)
        })?;
        Ok(result)
    }

    pub fn remove_top_shard_block(&self, id: &TopBlockDescrId) -> Result<()> {
        let _tc = TimeChecker::new(format!("remove_top_shard_block {}", id), 50);
        self.shard_top_blocks_db.delete(&id.to_bytes()?)
    }

    pub fn db_root_dir(&self) -> Result<&str> {
        Ok(&self.config.db_directory)
    }

    #[allow(dead_code)]
    pub fn adjust_states_gc_interval(&self, interval_ms: u32) {
        let prev = self.cells_gc_interval.swap(interval_ms, Ordering::Relaxed);
        log::info!("Adjusted states gc interval {} -> {}", prev, interval_ms);
    }

    pub async fn truncate_database(&self, mc_block_id: &BlockIdExt) -> Result<()> {
        // store shard blocks to truncate
        let prev_id = self.load_block_prev1(mc_block_id)?;
        let prev_handle = self
            .load_block_handle(&prev_id)?
            .ok_or_else(|| error!("there is no handle for block {}", prev_id))?;
        let prev_block = self.load_block_data(&prev_handle).await?;
        let top_blocks = prev_block.shard_hashes()?.top_blocks_all()?;

        // truncate archives
        self.archive_manager
            .trunc(mc_block_id, &|id: &BlockIdExt| {
                if id.shard().is_masterchain() {
                    return id.seq_no() >= mc_block_id.seq_no();
                } else {
                    for tb in &top_blocks {
                        if id.shard().intersect_with(tb.shard()) && id.seq_no() > tb.seq_no() {
                            return true;
                        }
                    }
                }
                false
            })
            .await?;

        // truncate handles and prev/next links
        fn clear_dbs(db: &InternalDb, id: BlockIdExt) {
            log::trace!("truncate_database: trying to drop handle {}", id);
            let _ = db.block_handle_storage.drop_handle(id.clone(), None);
            let _ = db.prev2_block_db.delete(&id);
            let _ = db.prev1_block_db.delete(&id);
            let _ = db.next2_block_db.delete(&id);
            let _ = db.next1_block_db.delete(&id);
        }

        self.next1_block_db.for_each(&mut |_key, val| {
            let id = BlockIdExt::deserialize(&val)?;
            if id.shard().is_masterchain() && id.seq_no() >= mc_block_id.seq_no() {
                clear_dbs(self, id);
            } else {
                for tb in &top_blocks {
                    if id.shard().intersect_with(tb.shard()) && id.seq_no() > tb.seq_no() {
                        clear_dbs(self, id);
                        break;
                    }
                }
            }
            Ok(true)
        })?;
        self.next2_block_db.for_each(&mut |_key, val| {
            let id = BlockIdExt::deserialize(&val)?;
            for tb in &top_blocks {
                if id.shard().intersect_with(tb.shard()) && id.seq_no() > tb.seq_no() {
                    clear_dbs(self, id);
                    break;
                }
            }
            Ok(true)
        })?;

        // truncate info related with last handles
        fn clear_last_handle(db: &InternalDb, id: &BlockIdExt) {
            log::trace!("truncate_database: clear_last_handle {}", id);
            let _ = db.next1_block_db.delete(id);
            if let Ok(Some(handle)) = db.load_block_handle(id) {
                handle.reset_next1();
                handle.reset_next2();
                let _ = db.store_block_handle(&handle, None);
            }
        }

        clear_last_handle(self, &prev_id);
        for id in &top_blocks {
            clear_last_handle(self, id);
        }

        Ok(())
    }

    pub fn create_fast_cell_storage(
        &self,
        index: Vec<(UInt256, u16)>,
    ) -> Result<AsyncCellsStorageAdapter> {
        match &self.state_db {
            StateDb::Dynamic(db) => db.create_fast_cell_storage(index),
            StateDb::Archive(_) => {
                fail!("create_fast_cell_storage is not supported in archival mode")
            }
        }
    }

    pub fn find_full_block_id(&self, root_hash: &UInt256) -> Result<Option<BlockIdExt>> {
        self.block_handle_storage.load_full_block_id(root_hash)
    }

    pub fn cells_factory(&self) -> Result<Arc<dyn CellsFactory>> {
        self.state_db.cells_factory()
    }

    pub fn cells_loader(&self) -> Result<Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>> {
        let cs = self.state_db.create_hashed_cell_storage(None, 0)?;
        Ok(Arc::new(move |hash| cs.load_cell(hash)))
    }

    pub fn is_archival_mode(&self) -> bool {
        matches!(self.state_db, StateDb::Archive(_))
    }
}

#[cfg(test)]
impl InternalDb {
    // return previous block berore split
    pub fn find_all_split(&self) -> Result<Vec<BlockIdExt>> {
        log::trace!("find_all_split");
        let mut res = vec![];
        self.next2_block_db.for_each(&mut |_handle_bytes, id_bytes| {
            let block_id = BlockIdExt::deserialize(&id_bytes)?;
            if let Ok(id_bytes) = self.prev1_block_db.get(&block_id) {
                res.push(BlockIdExt::deserialize(&id_bytes)?);
            }
            Ok(true)
        })?;
        res.sort_by_key(|a| a.seq_no());
        Ok(res)
    }
    // return next block after merge
    pub fn find_all_merge(&self) -> Result<Vec<BlockIdExt>> {
        log::trace!("find_all_merge");
        let mut res = vec![];
        self.prev2_block_db.for_each(&mut |_handle_bytes, id_bytes| {
            let block_id = BlockIdExt::deserialize(&id_bytes)?;
            if let Ok(id_bytes) = self.next1_block_db.get(&block_id) {
                res.push(BlockIdExt::deserialize(&id_bytes)?);
            } else {
                res.push(block_id);
            }
            Ok(true)
        })?;
        res.sort_by_key(|a| a.seq_no());
        Ok(res)
    }
}

#[cfg(test)]
#[path = "../tests/test_internal_db.rs"]
mod tests;
