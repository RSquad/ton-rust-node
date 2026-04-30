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
use crate::{
    block::{BlockIdExtExtention, BlockStuff},
    block_proof::BlockProofStuff,
    boot,
    config::{
        CollatorConfig, CollatorTestBundlesGeneralConfig, TonNodeConfig, ValidatorManagerConfig,
    },
    engine_traits::{EngineAlloc, EngineOperations, PrivateOverlayOperations},
    ext_messages::MessagesPool,
    full_node::{
        apply_block::{self, apply_block},
        counters::TpsCounter,
        shard_client::{start_masterchain_client, start_shards_client, SHARD_BROADCAST_WINDOW},
    },
    internal_db::{
        InternalDb, InternalDbConfig, ARCHIVES_GC_BLOCK, INITIAL_MC_BLOCK, LAST_APPLIED_MC_BLOCK,
        PSS_KEEPER_MC_BLOCK,
    },
    network::{
        control::{ControlServer, DataSource, StatusReporter},
        full_node_overlay_client::FullNodeOverlayClient,
        full_node_overlays::FullNodeOverlaysRouter,
        liteserver::LiteServer,
        node_network::NodeNetwork,
    },
    rpc_server::RpcServer,
    shard_blocks::{
        resend_top_shard_blocks_worker, save_top_shard_blocks_worker, ShardBlockProcessingResult,
        ShardBlocksPool,
    },
    shard_state::ShardStateStuff,
    shard_states_keeper::ShardStatesKeeper,
    types::awaiters_pool::AwaitersPool,
    validator::{
        accept_block::create_new_proof_link,
        candidate_db::{CandidateDb, CandidateDbPool},
        out_msg_queue_manager::OutMsgQueueManager,
        validator_manager::{start_validator_manager, ValidationStatus},
    },
};
#[cfg(feature = "telemetry")]
use crate::{
    engine_traits::{EngineTelemetry, Stoppable},
    full_node::telemetry::FullNodeTelemetry,
    network::telemetry::FullNodeNetworkTelemetry,
    validator::telemetry::CollatorValidatorTelemetry,
};
#[cfg(feature = "telemetry")]
use adnl::telemetry::{Metric, MetricBuilder, TelemetryItem, TelemetryPrinter};
use adnl::DhtSearchPolicy;
use catchain::SessionId;
#[cfg(feature = "telemetry")]
use std::fmt::Write;
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{block_handle_db::BlockHandle, StorageAlloc};
use ton_api::ton::ton_node::broadcast::NewShardBlockBroadcast;
use ton_block::{
    error, fail, BlockIdExt, Cell, ConfigParams, OutMsgQueue, Result, ShardIdent, UInt256,
    UnixTime, SHARD_FULL,
};

#[cfg(test)]
#[path = "tests/test_engine.rs"]
mod tests;

struct StorageDictInfo {
    dict: Cell,
    size: u64,
}

pub type SplitQueues = Option<(OutMsgQueue, OutMsgQueue, HashSet<UInt256>)>;
pub struct Engine {
    db: Arc<InternalDb>,
    candidate_db: CandidateDbPool,
    network: Arc<NodeNetwork>,
    overlays_router: OnceLock<Arc<FullNodeOverlaysRouter>>,

    shard_states_awaiters: AwaitersPool<BlockIdExt, Arc<ShardStateStuff>>,
    block_applying_awaiters: AwaitersPool<BlockIdExt, ()>,
    next_block_applying_awaiters: AwaitersPool<BlockIdExt, BlockIdExt>,
    download_block_awaiters: AwaitersPool<BlockIdExt, (BlockStuff, BlockProofStuff)>,
    external_messages: Arc<MessagesPool>,

    servers: lockfree::queue::Queue<Box<dyn Stoppable>>,
    stopper: Arc<Stopper>,

    zerostate_id: BlockIdExt,
    init_mc_block_id: BlockIdExt,
    hardforks: Vec<BlockIdExt>,
    flags: EngineFlags,
    archives_life_time_hours: Option<u32>,
    shard_blocks: ShardBlocksPool,
    candidates_cache: parking_lot::Mutex<lru::LruCache<BlockIdExt, Arc<Vec<u8>>>>,
    last_applied_mc_block_seqno: AtomicU32,
    last_known_mc_block_seqno: AtomicU32,
    last_known_keyblock_seqno: AtomicU32,
    will_validate: AtomicBool,
    sync_status: AtomicU32,
    monitor_min_split: Arc<AtomicU8>,
    // Each bit represents a corresponding shard. For monitor_min_split 2:
    //     bit:  64 | ... | 3          | 2          | 1          | 0
    // monitor:             0b111(0xe0)  0b101(0xa0)  0b011(0x60)  0b001(0x20)
    // So that the maximum supported monitor_min_split is MAX_MONITOR_MIN_SPLIT=6 (64 shards).
    monitored_shards: AtomicU64,

    test_bundles_config: CollatorTestBundlesGeneralConfig,
    collator_config: CollatorConfig,
    collator_config_mc: CollatorConfig,

    shard_states_keeper: Arc<ShardStatesKeeper>,
    out_msg_queue_manager: OnceLock<Arc<OutMsgQueueManager>>,

    // None - queue calculating is in progress
    split_queues_cache: lockfree::map::Map<BlockIdExt, SplitQueues>,
    validation_status: AtomicU8,
    last_validation_time: lockfree::map::Map<ShardIdent, u64>,
    last_collation_time: lockfree::map::Map<ShardIdent, u64>,

    storage_dicts_cache: parking_lot::Mutex<(u64, lru::LruCache<UInt256, StorageDictInfo>)>,

    #[cfg(feature = "telemetry")]
    full_node_telemetry: FullNodeTelemetry,
    #[cfg(feature = "telemetry")]
    collator_telemetry: CollatorValidatorTelemetry,
    #[cfg(feature = "telemetry")]
    validator_telemetry: CollatorValidatorTelemetry,
    #[cfg(feature = "telemetry")]
    full_node_service_telemetry: FullNodeNetworkTelemetry,
    #[cfg(feature = "telemetry")]
    engine_telemetry: Arc<EngineTelemetry>,
    engine_allocated: Arc<EngineAlloc>,
    #[cfg(feature = "telemetry")]
    telemetry_printer: TelemetryPrinter,

    tps_counter: TpsCounter,
}

struct DownloadContext<'a, T> {
    engine: &'a Engine,
    client: Arc<FullNodeOverlayClient>,
    downloader: Arc<dyn Downloader<Item = T>>,
    id: &'a BlockIdExt,
    limit: Option<u32>,
    log_error_limit: u32,
    name: &'a str,
    timeout: Option<(u64, u64, u64)>, // (current, multiplier*10, max)
}

impl<T> DownloadContext<'_, T> {
    async fn download(&mut self) -> Result<T> {
        let mut attempt = 1;
        loop {
            if self.engine.check_stop() {
                fail!("{} id: {}, stop flag was set", self.name, self.id);
            }
            match self.downloader.try_download(self).await {
                Err(e) => self.log(e.to_string().as_str(), attempt),
                Ok(ret) => break Ok(ret),
            }
            attempt += 1;
            if let Some(limit) = &self.limit {
                if &attempt > limit {
                    fail!("Downloader: out of attempts");
                }
            }
            if let Some((current, mult, max)) = &mut self.timeout {
                *current = (*max).min(*current * *mult / 10);
                futures_timer::Delay::new(Duration::from_millis(*current)).await;
            } else {
                tokio::task::yield_now().await;
            }
        }
    }

    fn log(&self, msg: &str, attempt: u32) {
        log::log!(
            if attempt > self.log_error_limit { log::Level::Warn } else { log::Level::Debug },
            "{} (attempt {}): id: {}, {}",
            self.name,
            attempt,
            self.id,
            msg
        )
    }
}

#[async_trait::async_trait]
trait Downloader: Send + Sync {
    type Item;
    async fn try_download(&self, context: &DownloadContext<'_, Self::Item>) -> Result<Self::Item>;
}

struct BlockDownloader;

#[async_trait::async_trait]
impl Downloader for BlockDownloader {
    type Item = (BlockStuff, BlockProofStuff);
    async fn try_download(&self, context: &DownloadContext<'_, Self::Item>) -> Result<Self::Item> {
        if let Some(handle) = context.engine.db.load_block_handle(context.id)? {
            let mut is_link = false;
            if handle.has_data() && handle.has_proof_or_link(&mut is_link) {
                let block = match context.engine.db.load_block_data(&handle).await {
                    Err(e) => {
                        if !handle.has_data() {
                            None
                        } else {
                            return Err(e);
                        }
                    }
                    Ok(block) => Some(block),
                };
                let proof = if block.is_none() {
                    None
                } else {
                    match context.engine.db.load_block_proof(&handle, is_link).await {
                        #[allow(clippy::if_same_then_else)]
                        Err(e) => {
                            if is_link && !handle.has_proof_link() {
                                None
                            } else if !is_link && !handle.has_proof() {
                                None
                            } else {
                                return Err(e);
                            }
                        }
                        Ok(proof) => Some(proof),
                    }
                };
                if let Some(block) = block {
                    if let Some(proof) = proof {
                        return Ok((block, proof));
                    }
                }
            }
        }
        #[cfg(feature = "telemetry")]
        context.engine.full_node_telemetry.new_downloading_block_attempt(context.id);
        let ret = context.client.download_block_full(context.id).await;
        #[cfg(feature = "telemetry")]
        if ret.is_ok() {
            context.engine.full_node_telemetry.new_downloaded_block(context.id);
        }
        ret
    }
}

struct BlockProofDownloader {
    is_link: bool,
    key_block: bool,
}

#[async_trait::async_trait]
impl Downloader for BlockProofDownloader {
    type Item = BlockProofStuff;
    async fn try_download(&self, context: &DownloadContext<'_, Self::Item>) -> Result<Self::Item> {
        if let Some(handle) = context.engine.db.load_block_handle(context.id)? {
            let mut is_link = false;
            if handle.has_proof_or_link(&mut is_link) {
                return context.engine.db.load_block_proof(&handle, is_link).await;
            }
        }
        context.client.download_block_proof(context.id, self.is_link, self.key_block).await
    }
}

struct NextBlockDownloader;

#[async_trait::async_trait]
impl Downloader for NextBlockDownloader {
    type Item = (BlockStuff, BlockProofStuff);
    async fn try_download(&self, context: &DownloadContext<'_, Self::Item>) -> Result<Self::Item> {
        if let Some(prev_handle) = context.engine.db.load_block_handle(context.id)? {
            if prev_handle.has_next1() {
                let next_id = context.engine.db.load_block_next1(context.id)?;
                if let Some(next_handle) = context.engine.db.load_block_handle(&next_id)? {
                    let mut is_link = false;
                    if next_handle.has_data() && next_handle.has_proof_or_link(&mut is_link) {
                        return Ok((
                            context.engine.db.load_block_data(&next_handle).await?,
                            context.engine.db.load_block_proof(&next_handle, is_link).await?,
                        ));
                    }
                }
            }
        }
        context.client.download_next_block_full(context.id).await
    }
}

struct ZeroStateDownloader;

#[async_trait::async_trait]
impl Downloader for ZeroStateDownloader {
    type Item = (Arc<ShardStateStuff>, Vec<u8>);
    async fn try_download(&self, context: &DownloadContext<'_, Self::Item>) -> Result<Self::Item> {
        if let Some(handle) = context.engine.db.load_block_handle(context.id)? {
            if handle.has_state() {
                let zs = context.engine.db.load_shard_state_dynamic(context.id)?;
                let mut data = vec![];
                zs.write_to(&mut data)?;
                return Ok((zs, data));
            }
        }
        context.client.download_zero_state(context.id).await
    }
}

#[derive(Default)]
pub struct Stopper {
    stop: Arc<AtomicU32>,
    token: tokio_util::sync::CancellationToken,
}

impl Stopper {
    pub fn new() -> Self {
        Stopper {
            stop: Arc::new(AtomicU32::new(0)),
            token: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn set_stop(&self) {
        let stop = self.stop.fetch_or(Engine::MASK_STOP, Ordering::Relaxed);
        Self::log_stop_status(stop);
        self.token.cancel();
    }

    pub async fn wait_stop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_millis(Engine::TIMEOUT_STOP_MS)).await;
            let stop = self.stop.load(Ordering::Relaxed) & !Engine::MASK_STOP;
            Self::log_stop_status(stop);
            if (self.stop.load(Ordering::Relaxed) & !Engine::MASK_STOP) == 0 {
                break;
            }
        }
    }

    pub fn log_stop_status(bitmap: u32) {
        let mut ss = String::new();
        if bitmap & Engine::MASK_SERVICE_BOOT != 0 {
            ss.push_str("boot, ");
        }
        if bitmap & Engine::MASK_SERVICE_DB_RESTORE != 0 {
            ss.push_str("DB restore, ");
        }
        if bitmap & Engine::MASK_SERVICE_MASTERCHAIN_BROADCAST_LISTENER != 0 {
            ss.push_str("masterchain broadcasts listener, ");
        }
        if bitmap & Engine::MASK_SERVICE_MASTERCHAIN_CLIENT != 0 {
            ss.push_str("masterchain client, ");
        }
        if bitmap & Engine::MASK_SERVICE_PSS_KEEPER != 0 {
            ss.push_str("persistent states storer, ");
        }
        if bitmap & Engine::MASK_SERVICE_SS_CACHE_KEEPER != 0 {
            ss.push_str("states cache cleaner, ");
        }
        if bitmap & Engine::MASK_SERVICE_SHARDCHAIN_BROADCAST_LISTENER != 0 {
            ss.push_str("shardchains broadcasts listener, ");
        }
        if bitmap & Engine::MASK_SERVICE_SHARDCHAIN_CLIENT != 0 {
            ss.push_str("shardchains client, ");
        }
        if bitmap & Engine::MASK_SERVICE_SHARDSTATE_GC != 0 {
            ss.push_str("shard states GC, ");
        }
        if bitmap & Engine::MASK_SERVICE_TOP_SHARDBLOCKS_SENDER != 0 {
            ss.push_str("top shard blocks sender, ");
        }
        if bitmap & Engine::MASK_SERVICE_VALIDATOR_MANAGER != 0 {
            ss.push_str("validator manager, ");
        }
        if bitmap & Engine::MASK_SERVICE_ARCHIVES_GC != 0 {
            ss.push_str("archives gc, ");
        }
        log::warn!("These services are still stopping ({:04x}): {}", bitmap, ss);
    }

    pub fn acquire_stop(&self, mask: u32) {
        self.stop.fetch_or(mask, Ordering::Relaxed);
    }

    pub fn check_stop(&self) -> bool {
        (self.stop.load(Ordering::Relaxed) & Engine::MASK_STOP) != 0
    }

    pub fn release_stop(&self, mask: u32) {
        self.stop.fetch_and(!mask, Ordering::Relaxed);
    }

    pub fn _token(&self) -> tokio_util::sync::CancellationToken {
        self.token.clone()
    }
}

impl Engine {
    // Masks for services
    pub const MASK_SERVICE_MASTERCHAIN_BROADCAST_LISTENER: u32 = 0x0002;
    pub const MASK_SERVICE_MASTERCHAIN_CLIENT: u32 = 0x0004;
    pub const MASK_SERVICE_PSS_KEEPER: u32 = 0x0008;
    pub const MASK_SERVICE_SHARDCHAIN_BROADCAST_LISTENER: u32 = 0x0010;
    pub const MASK_SERVICE_SHARDCHAIN_CLIENT: u32 = 0x0020;
    pub const MASK_SERVICE_SHARDSTATE_GC: u32 = 0x0040;
    pub const MASK_SERVICE_TOP_SHARDBLOCKS_SENDER: u32 = 0x0080;
    pub const MASK_SERVICE_VALIDATOR_MANAGER: u32 = 0x0100;
    pub const MASK_SERVICE_BOOT: u32 = 0x0200;
    pub const MASK_SERVICE_DB_RESTORE: u32 = 0x0400;
    pub const MASK_SERVICE_ARCHIVES_GC: u32 = 0x0800;
    pub const MASK_SERVICE_SS_CACHE_KEEPER: u32 = 0x1000;

    // Sync status (ordered by normal flow: boot → states → finish boot → archives → blocks → synced)
    pub const SYNC_STATUS_START_BOOT: u32 = 1;
    pub const SYNC_STATUS_LOAD_STATES: u32 = 2;
    pub const SYNC_STATUS_FINISH_BOOT: u32 = 3;
    pub const SYNC_STATUS_SYNC_ARCHIVES: u32 = 4;
    pub const SYNC_STATUS_SYNC_BLOCKS: u32 = 5;
    pub const SYNC_STATUS_FINISH_SYNC: u32 = 6;
    pub const SYNC_STATUS_CHECKING_DB: u32 = 7;
    pub const SYNC_STATUS_DB_BROKEN: u32 = 8;

    const MASK_STOP: u32 = 0x80000000;
    const TIMEOUT_STOP_MS: u64 = 1000;
    const CANDIDATES_CACHE_SIZE: usize = 128;
    #[cfg(feature = "telemetry")]
    const TIMEOUT_TELEMETRY_SEC: u64 = 30;
    const MAX_MONITOR_MIN_SPLIT: u8 = 6;
    const STORAGE_DICTS_CACHE_SIZE: u64 = 1 << 24;
    const MIN_CACHED_STORAGE_DICT_CELLS: u64 = 4000;

    pub async fn new(
        general_config: TonNodeConfig,
        flags: EngineFlags,
        stopper: Arc<Stopper>,
    ) -> Result<Arc<Self>> {
        struct DbStatusReporter {
            is_broken: AtomicBool,
        }

        impl StatusReporter for DbStatusReporter {
            fn get_report(&self) -> u32 {
                if self.is_broken.load(Ordering::Relaxed) {
                    Engine::SYNC_STATUS_DB_BROKEN
                } else {
                    Engine::SYNC_STATUS_CHECKING_DB
                }
            }
        }

        async fn open_db(
            db_config: InternalDbConfig,
            restore_db_enabled: bool,
            force_check_db: bool,
            is_broken: Option<&AtomicBool>,
            stopper: &Arc<Stopper>,
            monitor_min_split: Arc<AtomicU8>,
            truncate_db: Option<u32>,
            #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
            allocated: Arc<EngineAlloc>,
        ) -> Result<Arc<InternalDb>> {
            let check_stop = || {
                if stopper.check_stop() {
                    fail!("DB restore was stopped")
                }
                Ok(())
            };
            let db = InternalDb::with_update(
                db_config,
                restore_db_enabled,
                force_check_db,
                true,
                truncate_db,
                &check_stop,
                is_broken,
                monitor_min_split,
                None,
                #[cfg(feature = "telemetry")]
                telemetry,
                allocated,
            )
            .await?;
            Ok(Arc::new(db))
        }

        log::info!("Creating engine...");

        #[cfg(feature = "telemetry")]
        let (metrics, engine_telemetry) = Self::create_telemetry();
        let storage_allocated = Arc::new(StorageAlloc::default());
        let engine_allocated = Arc::new(EngineAlloc {
            storage: storage_allocated,
            awaiters: Arc::new(AtomicU64::new(0)),
            catchain_clients: Arc::new(AtomicU64::new(0)),
            shard_states: Arc::new(AtomicU64::new(0)),
            top_blocks: Arc::new(AtomicU64::new(0)),
            validator_adnl_keys: Arc::new(AtomicU64::new(0)),
            validator_peers: Arc::new(AtomicU64::new(0)),
            validator_sets: Arc::new(AtomicU64::new(0)),
            account_state_cache_bytes: Arc::new(AtomicU64::new(0)),
        });

        let archives_life_time_hours = general_config.gc_archives_life_time_hours();
        let cells_lifetime_sec = if general_config.archival_mode().is_none() {
            general_config.cells_gc_config().cells_lifetime_sec
        } else {
            u64::MAX
        };
        let enable_shard_state_persistent_gc = general_config.enable_shard_state_persistent_gc();
        let skip_saving_persistent_states = general_config.skip_saving_persistent_states();
        let pss_cells_cache_max_count = general_config.pss_cells_cache_max_count();
        let pss_prev_part_max_size = general_config.pss_prev_part_max_size();
        let states_cache_mode = general_config.states_cache_mode();
        let restore_db = general_config.restore_db();

        let cells_db_config = general_config.cells_db_config().clone();
        let db_config = InternalDbConfig {
            db_directory: general_config.internal_db_path().to_string(),
            cells_gc_interval_sec: general_config.cells_gc_config().gc_interval_sec,
            cells_db_config: cells_db_config.clone(),
            archival_mode: general_config.archival_mode().cloned(),
        };
        let control_config = general_config.control_server()?;
        let collator_config = general_config.collator_config().clone();
        let collator_config_mc = general_config.collator_config_mc().clone();
        let boot_from_zerostate = general_config.boot_from_zerostate();
        let global_config = general_config.load_global_config()?;
        let test_bundles_config = general_config.test_bundles_config().clone();
        let external_messages_maximum_queue_length =
            collator_config.external_messages_maximum_queue_length;

        let network = NodeNetwork::new(
            general_config,
            stopper.token.clone(),
            #[cfg(feature = "telemetry")]
            engine_telemetry.clone(),
            engine_allocated.clone(),
        )
        .await?;
        network.start().await?;

        let (status_reporter, status_server) = if let Some(control_config) = control_config {
            log::info!("Invoking DB status control server");
            let status_reporter = Arc::new(DbStatusReporter { is_broken: AtomicBool::new(false) });
            let status_server = ControlServer::with_params(
                control_config,
                DataSource::Status(status_reporter.clone()),
                network.config_handler(),
                network.config_handler(),
                Some(&network),
            )
            .await?;
            (Some(status_reporter), Some(status_server))
        } else {
            (None, None)
        };

        stopper.acquire_stop(Self::MASK_SERVICE_DB_RESTORE);
        let monitor_min_split = Arc::new(AtomicU8::new(0));
        let db = open_db(
            db_config,
            restore_db,
            flags.force_check_db,
            if let Some(status_reporter) = status_reporter.as_ref() {
                Some(&status_reporter.is_broken)
            } else {
                None
            },
            &stopper,
            monitor_min_split.clone(),
            flags.truncate_db,
            #[cfg(feature = "telemetry")]
            engine_telemetry.clone(),
            engine_allocated.clone(),
        )
        .await;
        if let Some(status_server) = status_server {
            log::info!("Stopping DB status control server...");
            status_server.shutdown().await;
            log::info!("Stopped DB status control server");
        }
        stopper.release_stop(Self::MASK_SERVICE_DB_RESTORE);
        let db = db?;

        let zero_state_id = global_config.zero_state().expect("check zero state settings");
        let mut init_mc_block_id = match global_config.init_block()? {
            Some(init_mc_block_id) if !boot_from_zerostate => {
                log::info!("zero state substitued by init block {}", init_mc_block_id);
                init_mc_block_id
            }
            _ => zero_state_id.clone(),
        };
        let mut hardforks = global_config.hardforks()?;
        hardforks.sort_by(|a, b| a.seq_no.cmp(&b.seq_no));
        if !boot_from_zerostate {
            if let Some(block_id) = hardforks.last() {
                if block_id.seq_no > init_mc_block_id.seq_no {
                    fail!(
                        "global config contains init_block and hardforks sections so int_block {} \
                        must be after last hard_fork {}",
                        init_mc_block_id.seq_no,
                        block_id.seq_no
                    )
                }
            }
            if let Ok(Some(block_id)) = db.load_full_node_state(INITIAL_MC_BLOCK) {
                if block_id.seq_no > init_mc_block_id.seq_no {
                    init_mc_block_id = block_id.deref().clone()
                }
            }
        }

        log::info!("load_all_top_shard_blocks");
        let shard_blocks = match db.load_all_top_shard_blocks() {
            Ok(tsbs) => tsbs,
            Err(e) => {
                log::error!("Can't load top shard blocks from db (continue without ones): {:?}", e);
                HashMap::default()
            }
        };
        log::info!("load_node_state");
        let last_mc_seqno =
            db.load_full_node_state(LAST_APPLIED_MC_BLOCK)?.map_or(0, |id| id.seq_no);
        let (shard_blocks_pool, shard_blocks_receiver) = ShardBlocksPool::new(
            shard_blocks,
            last_mc_seqno,
            false,
            #[cfg(feature = "telemetry")]
            &engine_telemetry,
            &engine_allocated,
        )?;

        let shard_states_keeper = ShardStatesKeeper::new(
            db.clone(),
            enable_shard_state_persistent_gc,
            skip_saving_persistent_states,
            pss_cells_cache_max_count,
            pss_prev_part_max_size,
            states_cache_mode,
            cells_lifetime_sec,
            stopper.clone(),
            cells_db_config.states_db_queue_len + 10,
            #[cfg(feature = "telemetry")]
            engine_telemetry.clone(),
            engine_allocated.clone(),
        )?;

        log::info!("Engine is created.");

        let now = UnixTime::now() as u32;
        let (ext_messages_pool, applied_blocks_rx) =
            MessagesPool::new(now, external_messages_maximum_queue_length);
        let candidate_db = CandidateDbPool::with_path(db.db_root_dir()?);
        let engine = Arc::new(Engine {
            db,
            candidate_db,
            shard_states_awaiters: AwaitersPool::new(
                "shard_states_awaiters",
                #[cfg(feature = "telemetry")]
                engine_telemetry.clone(),
                engine_allocated.clone(),
            ),
            block_applying_awaiters: AwaitersPool::new(
                "block_applying_awaiters",
                #[cfg(feature = "telemetry")]
                engine_telemetry.clone(),
                engine_allocated.clone(),
            ),
            next_block_applying_awaiters: AwaitersPool::new(
                "next_block_applying_awaiters",
                #[cfg(feature = "telemetry")]
                engine_telemetry.clone(),
                engine_allocated.clone(),
            ),
            download_block_awaiters: AwaitersPool::new(
                "download_block_awaiters",
                #[cfg(feature = "telemetry")]
                engine_telemetry.clone(),
                engine_allocated.clone(),
            ),
            external_messages: Arc::new(ext_messages_pool),

            servers: lockfree::queue::Queue::new(),
            stopper,
            zerostate_id: zero_state_id,
            init_mc_block_id,
            hardforks,
            flags,
            archives_life_time_hours,
            network,
            overlays_router: OnceLock::new(),
            shard_blocks: shard_blocks_pool,
            candidates_cache: parking_lot::Mutex::new(lru::LruCache::new(
                Self::CANDIDATES_CACHE_SIZE.try_into()?,
            )),
            last_applied_mc_block_seqno: AtomicU32::new(0),
            last_known_mc_block_seqno: AtomicU32::new(0),
            last_known_keyblock_seqno: AtomicU32::new(0),
            will_validate: AtomicBool::new(false),
            sync_status: AtomicU32::new(0),
            monitor_min_split,
            monitored_shards: AtomicU64::new(u64::MAX), // all shards are monitored by default
            test_bundles_config,
            collator_config,
            collator_config_mc,
            shard_states_keeper: shard_states_keeper.clone(),
            out_msg_queue_manager: OnceLock::new(),
            split_queues_cache: lockfree::map::Map::new(),
            validation_status: AtomicU8::new(0),
            last_validation_time: lockfree::map::Map::new(),
            last_collation_time: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            full_node_telemetry: FullNodeTelemetry::default(),
            #[cfg(feature = "telemetry")]
            collator_telemetry: CollatorValidatorTelemetry::default(),
            #[cfg(feature = "telemetry")]
            validator_telemetry: CollatorValidatorTelemetry::default(),
            #[cfg(feature = "telemetry")]
            full_node_service_telemetry: FullNodeNetworkTelemetry::new_service(),
            #[cfg(feature = "telemetry")]
            engine_telemetry,
            engine_allocated,
            #[cfg(feature = "telemetry")]
            telemetry_printer: TelemetryPrinter::with_params(
                "TON node",
                Self::TIMEOUT_TELEMETRY_SEC,
                metrics,
            ),
            tps_counter: TpsCounter::default(),
            storage_dicts_cache: parking_lot::Mutex::new((0, lru::LruCache::unbounded())),
        });

        engine.acquire_stop(Self::MASK_SERVICE_SHARDSTATE_GC);
        save_top_shard_blocks_worker(engine.clone(), shard_blocks_receiver);
        engine.external_messages().clone().start_applied_blocks_worker(applied_blocks_rx);
        Ok(engine)
    }

    pub fn set_last_applied_mc_seqno(&self, seqno: u32) {
        self.last_applied_mc_block_seqno.store(seqno, Ordering::Relaxed);
        metrics::gauge!("ton_node_engine_last_mc_block_seqno").set(seqno as f64);
    }

    pub fn get_last_applied_mc_seqno(&self) -> u32 {
        self.last_applied_mc_block_seqno.load(Ordering::Relaxed)
    }

    pub fn set_sync_status(&self, status: u32) {
        log::info!("sync status now is: {status}");
        self.sync_status.store(status, Ordering::Relaxed);
        metrics::gauge!("ton_node_engine_sync_status").set(status as f64);
    }

    pub fn get_sync_status(&self) -> u32 {
        self.sync_status.load(Ordering::Relaxed)
    }

    pub fn get_monitor_min_split(&self) -> u8 {
        self.monitor_min_split.load(Ordering::Relaxed)
    }

    pub async fn wait_stop(self: Arc<Self>) {
        // set stop flag
        self.stopper.set_stop();
        if let Ok(or) = self.overlays_router() {
            or.delete_overlays();
        }

        // stop servers
        let mut tasks = Vec::new();
        while let Some(server) = self.servers.pop() {
            tasks.push(async move {
                let name = server.name();
                log::info!("Stopping {name} server...");
                server.shutdown().await;
                log::info!("Stopped {name} server");
            });
        }
        futures::future::join_all(tasks).await;

        // stop states GC
        let engine = self.clone();
        tokio::spawn(async move {
            engine.db.stop_states_db().await;
            engine.stopper.release_stop(Self::MASK_SERVICE_SHARDSTATE_GC);
        });

        // wait while all node's services will stop
        self.stopper.clone().wait_stop().await;
        self.network.stop_adnl().await;
    }

    pub fn stopper(&self) -> &Stopper {
        &self.stopper
    }

    pub fn db(&self) -> &Arc<InternalDb> {
        &self.db
    }

    pub fn validator_network(&self) -> Arc<dyn PrivateOverlayOperations> {
        self.network.clone()
    }

    pub fn network(&self) -> &NodeNetwork {
        &self.network
    }

    pub fn zerostate_id(&self) -> &BlockIdExt {
        &self.zerostate_id
    }

    pub fn init_mc_block_id(&self) -> &BlockIdExt {
        &self.init_mc_block_id
    }

    pub fn flags(&self) -> &EngineFlags {
        &self.flags
    }

    pub fn hardforks(&self) -> &[BlockIdExt] {
        &self.hardforks
    }

    pub fn shard_states_keeper(&self) -> Arc<ShardStatesKeeper> {
        self.shard_states_keeper.clone()
    }

    pub async fn overlay_client(&self, shard: &ShardIdent) -> Result<Arc<FullNodeOverlayClient>> {
        self.overlays_router()?.overlay_client(shard).await
    }

    pub fn overlays_router(&self) -> Result<&Arc<FullNodeOverlaysRouter>> {
        self.overlays_router
            .get()
            .ok_or_else(|| error!("INTERNAL ERROR: overlays router is not initialized"))
    }

    pub fn shard_states_awaiters(&self) -> &AwaitersPool<BlockIdExt, Arc<ShardStateStuff>> {
        &self.shard_states_awaiters
    }

    pub fn block_applying_awaiters(&self) -> &AwaitersPool<BlockIdExt, ()> {
        &self.block_applying_awaiters
    }

    pub fn next_block_applying_awaiters(&self) -> &AwaitersPool<BlockIdExt, BlockIdExt> {
        &self.next_block_applying_awaiters
    }

    pub fn download_block_awaiters(
        &self,
    ) -> &AwaitersPool<BlockIdExt, (BlockStuff, BlockProofStuff)> {
        &self.download_block_awaiters
    }

    pub fn external_messages(&self) -> &Arc<MessagesPool> {
        &self.external_messages
    }

    pub fn shard_blocks(&self) -> &ShardBlocksPool {
        &self.shard_blocks
    }

    pub fn cache_block_candidate(&self, id: &BlockIdExt, block_data: Vec<u8>) -> Result<()> {
        let block_data = Arc::new(block_data);
        {
            let mut contains = true;
            let mut cache = self.candidates_cache.lock();
            cache.try_get_or_insert(id.clone(), || {
                if *id.file_hash() != UInt256::calc_file_hash(&block_data) {
                    fail!("file hash mismatch");
                }
                contains = false;
                Ok(block_data.clone())
            })?;
            if contains {
                log::trace!("Block candidate {} is already cached", id);
                return Ok(());
            }
            log::trace!("Cached block candidate {}", id);
        }

        self.download_block_awaiters().shunt(id, || {
            let block = BlockStuff::deserialize_block(id.clone(), block_data.clone())?;
            let proof = create_new_proof_link(&block)?;
            Ok((block, proof))
        })?;

        Ok(())
    }

    pub fn try_get_cached_block_candidate(&self, id: &BlockIdExt) -> Option<Arc<Vec<u8>>> {
        self.candidates_cache.lock().get(id).cloned()
    }

    pub fn set_will_validate(&self, will_validate: bool) {
        self.will_validate.store(will_validate, Ordering::SeqCst);
        metrics::gauge!("ton_node_engine_will_validate").set(if will_validate {
            1.0f64
        } else {
            0.0f64
        });
    }

    pub fn will_validate(&self) -> bool {
        self.will_validate.load(Ordering::SeqCst)
    }

    pub fn update_last_known_mc_block_seqno(&self, seqno: u32) -> bool {
        self.last_known_mc_block_seqno.fetch_max(seqno, Ordering::SeqCst) < seqno
    }

    pub fn update_last_known_keyblock_seqno(&self, seqno: u32) -> bool {
        self.last_known_keyblock_seqno.fetch_max(seqno, Ordering::SeqCst) < seqno
    }

    pub fn test_bundles_config(&self) -> &CollatorTestBundlesGeneralConfig {
        &self.test_bundles_config
    }

    pub fn collator_config(&self) -> &CollatorConfig {
        &self.collator_config
    }

    pub fn collator_config_mc(&self) -> &CollatorConfig {
        &self.collator_config_mc
    }

    #[cfg(feature = "telemetry")]
    pub fn full_node_telemetry(&self) -> &FullNodeTelemetry {
        &self.full_node_telemetry
    }

    #[cfg(feature = "telemetry")]
    pub fn collator_telemetry(&self) -> &CollatorValidatorTelemetry {
        &self.collator_telemetry
    }

    #[cfg(feature = "telemetry")]
    pub fn validator_telemetry(&self) -> &CollatorValidatorTelemetry {
        &self.validator_telemetry
    }

    #[cfg(feature = "telemetry")]
    pub fn full_node_service_telemetry(&self) -> &FullNodeNetworkTelemetry {
        &self.full_node_service_telemetry
    }

    #[cfg(feature = "telemetry")]
    pub fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.engine_telemetry
    }

    pub fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.engine_allocated
    }

    pub fn validation_status(&self) -> ValidationStatus {
        ValidationStatus::from_u8(self.validation_status.load(Ordering::Relaxed))
    }

    pub fn set_validation_status(&self, status: ValidationStatus) {
        self.validation_status.store(status as u8, Ordering::Relaxed);
        metrics::gauge!("ton_node_validator_status").set(status as u8 as f64);
    }

    pub fn last_validation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        &self.last_validation_time
    }

    pub fn set_last_validation_time(&self, shard: ShardIdent, time: u64) {
        self.last_validation_time.insert(shard, time);
    }

    pub fn remove_last_validation_time(&self, shard: &ShardIdent) {
        self.last_validation_time.remove(shard);
    }

    pub fn last_collation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        &self.last_collation_time
    }

    pub fn set_last_collation_time(&self, shard: ShardIdent, time: u64) {
        self.last_collation_time.insert(shard, time);
    }

    pub fn remove_last_collation_time(&self, shard: &ShardIdent) {
        self.last_collation_time.remove(shard);
    }

    pub fn tps_counter(&self) -> &TpsCounter {
        &self.tps_counter
    }

    pub fn get_candidate_table(&self, session_id: &SessionId) -> Result<Arc<CandidateDb>> {
        self.candidate_db.get_db(session_id)
    }

    pub fn destroy_candidate_table(&self, session_id: &SessionId) -> Result<bool> {
        self.candidate_db.destroy_db(session_id)
    }

    pub fn split_queues_cache(&self) -> &lockfree::map::Map<BlockIdExt, SplitQueues> {
        &self.split_queues_cache
    }

    pub fn need_monitor(&self, shard: &ShardIdent) -> Result<bool> {
        if shard.is_masterchain() {
            return Ok(true);
        }
        if shard.is_base_workchain() {
            let monitor_min_split = self.monitor_min_split.load(Ordering::Relaxed);
            let monitored_shards = self.monitored_shards.load(Ordering::Relaxed);
            for i in 0..(1 << monitor_min_split) as u64 {
                if (1 << i) & monitored_shards != 0 {
                    let shard_prefix = i << (64 - monitor_min_split);
                    let monitored_shard = ShardIdent::with_prefix_len(
                        monitor_min_split,
                        shard.workchain_id(),
                        shard_prefix,
                    )?;
                    if monitored_shard.intersect_with(shard) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub async fn download_and_apply_block_worker(
        self: Arc<Self>,
        id: &BlockIdExt,
        mc_seq_no: u32,
        pre_apply: bool,
        recursion_depth: u32,
    ) -> Result<()> {
        if recursion_depth > apply_block::MAX_RECURSION_DEPTH {
            fail!(
                "Download and apply block {} - too deep recursion ({} >= {})",
                id,
                recursion_depth,
                apply_block::MAX_RECURSION_DEPTH
            );
        }

        loop {
            if let Some(handle) = self.load_block_handle(id)? {
                if handle.is_applied() || pre_apply && handle.has_state() {
                    log::trace!(
                        "download_and_apply_block_worker(pre_apply: {}): block is already applied {}",
                        pre_apply,
                        handle.id()
                    );
                    return Ok(());
                }
                let mut is_link = false;
                if handle.has_data() && handle.has_proof_or_link(&mut is_link) {
                    while !((pre_apply && handle.has_state()) || handle.is_applied()) {
                        let s = self.clone();
                        let res = self
                            .block_applying_awaiters()
                            .do_or_wait(handle.id(), None, async {
                                let block = s.load_block(&handle).await?;
                                s.apply_block_worker(
                                    &handle,
                                    &block,
                                    mc_seq_no,
                                    pre_apply,
                                    recursion_depth,
                                )
                                .await?;
                                Ok(())
                            })
                            .await;
                        if res.is_err() {
                            if !handle.has_data() {
                                break;
                            }
                            res?;
                        }
                    }
                    if handle.has_data() {
                        return Ok(());
                    }
                }
            }

            let now = std::time::Instant::now();
            log::trace!(
                "Start downloading block for {}apply... {}",
                if pre_apply { "pre-" } else { "" },
                id
            );

            let block_n_proof = loop {
                // MC blocks require full proof verification (signatures + key-block chain).
                // A proof link created from raw block data provides only shallow verification,
                // so skip the candidates cache for masterchain blocks and download a real proof.
                if !id.shard().is_masterchain() {
                    if let Some(block_data) = self.try_get_cached_block_candidate(id) {
                        let block = BlockStuff::deserialize_block(id.clone(), block_data.clone())?;
                        let proof = create_new_proof_link(&block)?;
                        log::trace!(
                            "download_and_apply_block_worker {} loaded from candidates cache",
                            id
                        );
                        break Some((block, proof));
                    }
                }
                // for pre-apply only 10 attempts, for apply - infinity
                let (attempts, timeout) =
                    if pre_apply { (Some(10), Some((50, 15, 500))) } else { (None, None) };

                break self
                    .download_block_awaiters()
                    .do_or_wait(id, None, self.download_block_worker(id, attempts, timeout))
                    .await?;
            };

            if let Some((block, proof)) = block_n_proof {
                let downloading_time = now.elapsed().as_millis();

                let now = std::time::Instant::now();
                proof.check_proof(self.deref()).await?;
                let handle = self.store_block(&block).await?;
                let handle =
                    if let Some(handle) = handle.to_non_created() { handle } else { continue };
                let handle = self.store_block_proof(id, Some(handle), &proof).await?;
                let handle = handle.to_non_created().ok_or_else(|| {
                    error!("INTERNAL ERROR: bad result for store block {} proof", id)
                })?;
                log::trace!(
                    "Downloaded block for {}apply {} TIME download: {}ms, check & save: {}",
                    if pre_apply { "pre-" } else { "" },
                    block.id(),
                    downloading_time,
                    now.elapsed().as_millis(),
                );
                self.apply_block(&handle, &block, mc_seq_no, pre_apply).await?;
                return Ok(());
            }
        }
    }

    pub async fn apply_hardfork_block(
        self: Arc<Self>,
        handle: &Arc<BlockHandle>,
        block: &BlockStuff,
    ) -> Result<()> {
        if !block.id().is_masterchain() {
            fail!("we apply only masterchain hardfork blocks, but {}", block.id())
        }
        if !block.is_key_block()? {
            fail!("we apply only keyblock hardfork {}", block.id())
        }

        /*let (prev_block_id, _) = block.construct_prev_id()?;  // TODO remove check?
        let prev_state = self.load_state(&prev_block_id).await?;
        let (old_set, _) = prev_state.read_cur_validator_set_and_cc_conf()?;
        let (new_set, _) = block.read_cur_validator_set_and_cc_conf()?;
        for old in old_set.list() {
            if new_set.list().iter().position(|new| new.public_key == old.public_key).is_some() {
                fail!("old and new validator sets contain at least one common node with id {}",
                    hex::encode(old.public_key.key_bytes()))
            }
        }*/

        if handle.is_applied() {
            log::trace!("apply_hardfork_block: block is already applied {}", handle.id());
            return Ok(());
        }

        log::debug!("Start applying block... {}", block.id());

        apply_block(
            handle,
            block,
            block.id().seq_no,
            &(self.clone() as Arc<dyn EngineOperations>),
            false,
            0,
        )
        .await?;

        log::debug!("After applying block... {}", block.id());
        let gen_utime = block.gen_utime()?;
        let ago = UnixTime::now() as i32 - gen_utime as i32;
        self.save_last_applied_mc_block_id(block.id())?;
        metrics::gauge!("ton_node_engine_last_mc_block_seqno").set(block.id().seq_no() as f64);
        metrics::gauge!("ton_node_engine_timediff_seconds").set(ago as f64);
        metrics::gauge!("ton_node_engine_last_mc_block_utime").set(gen_utime as f64);
        self.shard_blocks().update_shard_blocks(&self.load_state(block.id()).await?).await?;

        let first_time_applied = self.set_applied(handle, block.id().seq_no).await?;

        if let Err(e) = self.mc_block_post_apply(block, gen_utime, first_time_applied).await {
            log::error!("Error after apply block {}: {}", block.id(), e);
        }

        log::info!("Applied block {}, {}s old", block.id(), ago);

        Ok(())
    }

    pub async fn apply_block_worker(
        self: Arc<Self>,
        handle: &Arc<BlockHandle>,
        block: &BlockStuff,
        mc_seq_no: u32,
        pre_apply: bool,
        recursion_depth: u32,
    ) -> Result<()> {
        if handle.is_applied() || pre_apply && handle.has_state() {
            log::trace!(
                "apply_block_worker(pre_apply: {}): block is already applied {}",
                pre_apply,
                handle.id()
            );
            return Ok(());
        }

        if recursion_depth > apply_block::MAX_RECURSION_DEPTH {
            fail!(
                "Apply block {} - too deep recursion ({} >= {})",
                handle.id(),
                recursion_depth,
                apply_block::MAX_RECURSION_DEPTH
            );
        }

        let op_name = if pre_apply { "pre-applying" } else { "applying" };
        let id = block.id();
        log::debug!("Start {op_name} block {id}");

        let mut is_link = false;
        if !handle.has_data() || !handle.has_proof_or_link(&mut is_link) {
            fail!("Block must have proof and data saved before applying");
        }

        apply_block(
            handle,
            block,
            mc_seq_no,
            &(self.clone() as Arc<dyn EngineOperations>),
            pre_apply,
            recursion_depth,
        )
        .await?;

        if !pre_apply {
            self.external_messages().push_applied_block(Arc::new(block.clone()));
        }

        let gen_utime = block.gen_utime()?;
        let ago = UnixTime::now() as i32 - gen_utime as i32;
        let mut transactions = 0;
        let mut message;
        if block.id().shard().is_masterchain() {
            if !pre_apply {
                self.shard_blocks()
                    .update_shard_blocks(&self.load_state(block.id()).await?)
                    .await?;

                let first_time_applied = self.set_applied(handle, block.id().seq_no()).await?;

                if first_time_applied {
                    if let Err(e) = self.save_last_applied_mc_block_id(block.id()) {
                        log::error!("Can't save last applied mc block {}: {}", block.id(), e);
                    }
                }
                metrics::gauge!("ton_node_engine_last_mc_block_seqno")
                    .set(block.id().seq_no() as f64);
                metrics::gauge!("ton_node_engine_timediff_seconds").set(ago as f64);
                metrics::gauge!("ton_node_engine_last_mc_block_utime").set(gen_utime as f64);

                match self.mc_block_post_apply(block, gen_utime, first_time_applied).await {
                    Err(e) => log::error!("Error after apply block {}: {}", block.id(), e),
                    Ok(t) => transactions = t,
                }
            }

            let op_name = if pre_apply { "Pre-applied" } else { "Applied" };
            message =
                format!("{op_name} master block {id}, {ago}s old, {}bytes", block.data().len());
        } else {
            if !pre_apply {
                let first_time_applied = self.set_applied(handle, mc_seq_no).await?;
                if first_time_applied {
                    transactions = block.calculate_tr_count()?;
                    self.tps_counter.submit_transactions(gen_utime as u64, transactions);
                    metrics::counter!("ton_node_engine_applied_transactions_total")
                        .increment(transactions as u64);
                }
            }

            let op_name = if pre_apply { "Pre-applied" } else { "Applied" };
            message = format!(
                "{op_name} block {id} ref_mc_block: {mc_seq_no}, {ago}s old, {}bytes",
                block.data().len()
            );
        }
        if transactions > 0 {
            message.push_str(&format!(", {}tr", transactions));
        }
        log::info!("{}", message);
        Ok(())
    }

    async fn mc_block_post_apply(
        &self,
        block: &BlockStuff,
        gen_utime: u32,
        first_time_applied: bool,
    ) -> Result<usize> {
        let mut transactions = 0;
        if first_time_applied {
            transactions = block.calculate_tr_count()?;
            self.tps_counter.submit_transactions(gen_utime as u64, transactions);
            metrics::counter!("ton_node_engine_applied_transactions_total")
                .increment(transactions as u64);
            if block.is_key_block()? {
                self.update_public_overlays(block.id(), &block.read_config_params()?).await?;
            }
        }

        let (prev_id, prev2_id_opt) = block.construct_prev_id()?;
        if prev2_id_opt.is_some() {
            fail!("UNEXPECTED error: master block refers two previous blocks");
        }
        let id = block.id().clone();
        self.next_block_applying_awaiters.do_or_wait(&prev_id, None, async move { Ok(id) }).await?;

        let mut total = 0;
        let mut removed = 0;
        for guard in &self.split_queues_cache {
            total += 1;
            if self.shard_states_keeper.allow_state_gc(guard.key())? {
                self.split_queues_cache.remove(guard.key());
                removed += 1;
            }
        }
        log::debug!("Split queues cache GC: total {total}, removed {removed}");

        Ok(transactions)
    }

    pub async fn update_public_overlays(
        &self,
        keyblock_id: &BlockIdExt,
        config: &ConfigParams,
    ) -> Result<()> {
        let new_monitor_min_split = config.base_workchain()?.monitor_min_split();
        if new_monitor_min_split > Self::MAX_MONITOR_MIN_SPLIT {
            fail!(
                "CRITICAL! New monitor_min_split is too big ({new_monitor_min_split}), \
                max supported value is {}",
                Self::MAX_MONITOR_MIN_SPLIT
            )
        }
        self.monitor_min_split.store(new_monitor_min_split, Ordering::Relaxed);
        // Here we can update only public overlays.
        // Private overlays updated in set_validator_list function,
        // because we don't have the needed keys in adnl before set_validator_list call.
        self.overlays_router()?.update_public_overlays(keyblock_id, new_monitor_min_split).await?;
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn log_workers_stats(&self) -> Result<()> {
        // Node workers stats:
        //                  seqno      diff    gen utime  diff    root hash                                        file hash
        // mc client        14000000      0    1682084115    1    000000000000000000000000000000000000000000000000 000000000000000000000000000000000000000000000000
        // "{:<17} {:>10} {:>7} {:>10} {:>7} {:<64} {:<64}"

        let now = self.now();
        let mut report = String::default();
        writeln!(
            &mut report,
            "Node workers stats:\n{:<17} {:>10} {:>7} {:>10} {:>7} {:<64} {:<64}",
            "", "seqno", "diff", "gen utime", "diff", "root hash", "file hash"
        )?;
        let mut mc_seqno = None;

        let mut log_worker_stat = |result: Result<Option<Arc<BlockIdExt>>>,
                                   name: &str|
         -> Result<()> {
            write!(report, "{:<17} ", name)?;
            match result {
                Ok(Some(block_id)) => {
                    // seqno
                    write!(report, "{:>10} ", block_id.seq_no())?;
                    // seqno diff
                    if let Some(mc_seqno) = mc_seqno.as_ref() {
                        let diff = *mc_seqno - block_id.seq_no();
                        write!(report, "{:>7} ", diff)?;
                    } else {
                        write!(report, "{:>7} ", "")?;
                        mc_seqno = Some(block_id.seq_no());
                    }
                    // timestamp, timediff
                    match self.load_block_handle(&block_id) {
                        Ok(Some(handle)) => {
                            let utime = handle.gen_utime();
                            write!(report, "{:>10} {:>7} ", utime, now - utime)?
                        }
                        Ok(None) => write!(report, "handle is none     ")?,
                        Err(_) => write!(report, "can't load handle  ")?,
                    };
                    // hashes
                    write!(report, "{:<64x} {:<64x}", block_id.root_hash(), block_id.file_hash())?
                }
                Ok(None) => write!(report, "none")?,
                Err(e) => write!(report, "can't load id: {}", e)?,
            };
            report.push('\n');
            Ok(())
        };

        log_worker_stat(self.load_last_applied_mc_block_id(), "mc client")?;
        log_worker_stat(self.load_shard_client_mc_block_id(), "shard client")?;
        log_worker_stat(self.load_pss_keeper_mc_block_id(), "pss keeper")?;
        log_worker_stat(self.load_archives_gc_mc_block_id(), "archives gc")?;
        log_worker_stat(self.load_last_rotation_block_id(), "last rotation")?;
        log::info!("{}", report);
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn create_telemetry() -> (Vec<TelemetryItem>, Arc<EngineTelemetry>) {
        fn create_metric(name: &str) -> Arc<Metric> {
            Metric::without_totals(name, Engine::TIMEOUT_TELEMETRY_SEC)
        }
        fn create_metric_with_total_average(name: &str) -> Arc<Metric> {
            Metric::with_total_average(name, Engine::TIMEOUT_TELEMETRY_SEC)
        }
        fn create_metric_per_sec(name: &str) -> Arc<MetricBuilder> {
            MetricBuilder::with_metric_and_period(
                Metric::with_total_amount(name, Engine::TIMEOUT_TELEMETRY_SEC),
                1_000_000_000, // 1 sec in nanos
            )
        }

        let storage_telemetry = Arc::new(StorageTelemetry {
            file_entries: create_metric("Alloc NODE file entries"),
            handles: create_metric("Alloc NODE block handles"),
            packages: create_metric("Alloc NODE packages"),
            storing_cells: create_metric("Alloc NODE storing cells"),
            shardstates_queue: create_metric("Alloc NODE shardstates queue"),
            cached_cells_counters: create_metric("Alloc NODE cells counters"),

            loaded_cells_from_db: create_metric_per_sec("NODE loaded from db cells/sec"),
            load_cell_from_db_time_nanos: create_metric_with_total_average(
                "NODE cell load time from db, nanos",
            ),
            load_cell_from_cache_time_nanos: create_metric_with_total_average(
                "NODE cell load time from cache, nanos",
            ),
            store_cell_to_cache_time_nanos: create_metric_with_total_average(
                "NODE cell store time to cache, nanos",
            ),
            stored_new_cells: create_metric_per_sec("NODE stored new cells & counters/sec"),
            deleted_cells: create_metric_per_sec("NODE deleted cells & counters/sec"),

            loaded_counters: create_metric_per_sec("NODE loaded from db counters/sec"),
            load_counter_time_nanos: create_metric_with_total_average(
                "NODE counter load time from db, nanos",
            ),
            updated_counters: create_metric_per_sec("NODE updated counters/sec"),

            boc_db_element_write_nanos: create_metric_with_total_average(
                "NODE boc db element write, nanos",
            ),
            save_boc_total_micros: create_metric_with_total_average(
                "NODE save boc: total time, micros",
            ),
            save_boc_traverse_micros: create_metric_with_total_average(
                "NODE save boc: traverse time, micros",
            ),
            save_boc_tr_build_micros: create_metric_with_total_average(
                "NODE save boc: trans build time, micros",
            ),
            save_boc_commit_micros: create_metric_with_total_average(
                "NODE save boc: trans commit time, micros",
            ),
            save_boc_cleanup_micros: create_metric_with_total_average(
                "NODE save boc: cells cleanup time, micros",
            ),
            delete_boc_total_micros: create_metric_with_total_average(
                "NODE delete boc: total time, micros",
            ),
            delete_boc_traverse_micros: create_metric_with_total_average(
                "NODE delete boc: traverse time, micros",
            ),
            delete_boc_tr_build_micros: create_metric_with_total_average(
                "NODE delete boc: trans build time, micros",
            ),
            delete_boc_commit_micros: create_metric_with_total_average(
                "NODE delete boc: commit time, micros",
            ),
            cell_cache_hits: create_metric_per_sec("NODE cell cache hits/sec"),
            cell_cache_misses: create_metric_per_sec("NODE cell cache misses/sec"),
            cell_cache_len: create_metric("NODE cell cache len"),
            rocksdb_mem_table_mb: create_metric("Alloc NODE RocksDB mem tables, MB"),
            rocksdb_block_cache_mb: create_metric("Alloc NODE RocksDB block cache, MB"),
        });
        let engine_telemetry = Arc::new(EngineTelemetry {
            storage: storage_telemetry,
            awaiters: create_metric("Alloc NODE awaiters"),
            catchain_clients: create_metric("Alloc NODE catchains"),
            cells: create_metric("Alloc NODE cells"),
            shard_states: create_metric("Alloc NODE shard states"),
            top_blocks: create_metric("Alloc NODE top blocks"),
            validator_adnl_keys: create_metric("Alloc NODE validator ADNL keys"),
            validator_peers: create_metric("Alloc NODE validator peers"),
            validator_sets: create_metric("Alloc NODE validator sets"),
            account_state_cache_mb: create_metric("Alloc NODE account state cache, MB"),
            storage_dicts_cache_cells: create_metric("Alloc NODE storage dicts cache cells"),
        });
        let metrics = vec![
            TelemetryItem::Metric(engine_telemetry.storage.file_entries.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.handles.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.packages.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.storing_cells.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.shardstates_queue.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.cached_cells_counters.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.loaded_cells_from_db.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.load_cell_from_db_time_nanos.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.load_cell_from_cache_time_nanos.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.store_cell_to_cache_time_nanos.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.stored_new_cells.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.deleted_cells.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.loaded_counters.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.load_counter_time_nanos.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.updated_counters.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.boc_db_element_write_nanos.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.save_boc_total_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.save_boc_traverse_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.save_boc_tr_build_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.save_boc_commit_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.save_boc_cleanup_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.delete_boc_total_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.delete_boc_traverse_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.delete_boc_tr_build_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.delete_boc_commit_micros.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.rocksdb_mem_table_mb.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.rocksdb_block_cache_mb.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.cell_cache_hits.clone()),
            TelemetryItem::MetricBuilder(engine_telemetry.storage.cell_cache_misses.clone()),
            TelemetryItem::Metric(engine_telemetry.storage.cell_cache_len.clone()),
            TelemetryItem::Metric(engine_telemetry.awaiters.clone()),
            TelemetryItem::Metric(engine_telemetry.catchain_clients.clone()),
            TelemetryItem::Metric(engine_telemetry.cells.clone()),
            TelemetryItem::Metric(engine_telemetry.shard_states.clone()),
            TelemetryItem::Metric(engine_telemetry.top_blocks.clone()),
            TelemetryItem::Metric(engine_telemetry.validator_adnl_keys.clone()),
            TelemetryItem::Metric(engine_telemetry.validator_peers.clone()),
            TelemetryItem::Metric(engine_telemetry.validator_sets.clone()),
            TelemetryItem::Metric(engine_telemetry.account_state_cache_mb.clone()),
            TelemetryItem::Metric(engine_telemetry.storage_dicts_cache_cells.clone()),
        ];
        (metrics, engine_telemetry)
    }

    pub async fn process_new_shard_block(
        self: Arc<Self>,
        broadcast: NewShardBlockBroadcast,
    ) -> Result<BlockIdExt> {
        let id = broadcast.block.block;
        let cc_seqno = broadcast.block.cc_seqno as u32;
        let data = broadcast.block.data;

        // check only
        let result = self
            .shard_blocks
            .process_shard_block_raw(&id, cc_seqno, data, false, true, self.deref())
            .await?;
        let ShardBlockProcessingResult::MightBeAdded(tbd) = result else {
            log::trace!("Skipped new shard block broadcast {} because it is already known", id);
            return Ok(id);
        };

        let (mc_seqno, _created_by) = tbd.top_block_mc_seqno_and_creator()?;
        let shard_client_mc_block_id = self
            .load_shard_client_mc_block_id()?
            .ok_or_else(|| error!("INTERNAL ERROR: No shard client MC block set after boot"))?;
        if shard_client_mc_block_id.seq_no() + SHARD_BROADCAST_WINDOW < mc_seqno {
            log::debug!(
                "Skipped new shard block broadcast {} because it refers to master block {}, but shard client is on {}",
                id, mc_seqno, shard_client_mc_block_id.seq_no()
            );
            return Ok(id);
        }

        tokio::spawn({
            let id = id.clone();
            async move {
                // just passively waiting for 1s...
                if let Err(e) = self.clone().wait_state(&id, Some(1_000), false).await {
                    log::error!(
                        "Error in wait_state after top-block-broadcast false {}: {}",
                        id,
                        e
                    );
                    // ...and then allow to download needed blocks forced
                    if let Err(e) = self.clone().wait_state(&id, Some(10_000), true).await {
                        log::error!(
                            "Error in wait_state after top-block-broadcast true {}: {}",
                            id,
                            e
                        );
                        return;
                    }
                }

                // if we are validator and in sync, add tsbd to list for collator
                if self.is_validator() && matches!(self.check_sync().await, Ok(true)) {
                    if let Err(e) = self
                        .shard_blocks
                        .process_shard_block(&id, cc_seqno, || Ok(tbd.clone()), false, self.deref())
                        .await
                    {
                        log::error!("Error in process_shard_block after wait_state {}: {}", id, e);
                    }
                }
            }
        });

        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_download_context<'a, T>(
        &'a self,
        downloader: Arc<dyn Downloader<Item = T>>,
        id: &'a BlockIdExt,
        limit: Option<u32>,
        log_error_limit: u32,
        name: &'a str,
        timeout: Option<(u64, u64, u64)>,
    ) -> Result<DownloadContext<'a, T>> {
        let ret = DownloadContext {
            client: self.overlay_client(id.shard()).await?,
            engine: self,
            downloader,
            id,
            limit,
            log_error_limit,
            name,
            timeout,
        };
        Ok(ret)
    }

    pub async fn download_next_block_worker(
        &self,
        prev_id: &BlockIdExt,
        limit: Option<u32>,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        if !prev_id.is_masterchain() {
            fail!("download_next_block is allowed only for masterchain")
        }
        self.create_download_context(
            Arc::new(NextBlockDownloader),
            prev_id,
            limit,
            30,
            "download_next_block_worker",
            Some((50, 11, 1000)),
        )
        .await?
        .download()
        .await
    }

    pub async fn download_block_worker(
        &self,
        id: &BlockIdExt,
        limit: Option<u32>,
        timeout: Option<(u64, u64, u64)>,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        self.create_download_context(
            Arc::new(BlockDownloader),
            id,
            limit,
            0,
            "download_block_worker",
            timeout,
        )
        .await?
        .download()
        .await
    }

    pub async fn download_block_proof_worker(
        &self,
        id: &BlockIdExt,
        is_link: bool,
        key_block: bool,
        limit: Option<u32>,
    ) -> Result<BlockProofStuff> {
        if id.seq_no() == 0 {
            fail!("cannot download block proof for zero state")
        }
        self.create_download_context(
            Arc::new(BlockProofDownloader { is_link, key_block }),
            id,
            limit,
            0,
            "download_block_proof_worker",
            None,
        )
        .await?
        .download()
        .await
    }

    pub async fn download_zerostate_worker(
        &self,
        id: &BlockIdExt,
        limit: Option<u32>,
    ) -> Result<(Arc<ShardStateStuff>, Vec<u8>)> {
        self.create_download_context(
            Arc::new(ZeroStateDownloader),
            id,
            limit,
            0,
            "download_zerostate_worker",
            Some((10, 12, 3000)),
        )
        .await?
        .download()
        .await
    }

    pub(crate) async fn check_sync(&self) -> Result<bool> {
        let last_applied_mc_id = if let Some(id) = self.load_last_applied_mc_block_id()? {
            id
        } else {
            fail!("INTERNAL ERROR: No last applied MC block set after boot")
        };
        let shard_client_mc_id = if let Some(id) = self.load_shard_client_mc_block_id()? {
            id
        } else {
            fail!("INTERNAL ERROR: No shard client MC block set after boot")
        };
        if shard_client_mc_id.seq_no() + 16 < last_applied_mc_id.seq_no() {
            return Ok(false);
        }

        let last_mc_handle = self.load_block_handle(&last_applied_mc_id)?.ok_or_else(|| {
            error!("Cannot load handle for last masterchain block {}", last_applied_mc_id)
        })?;
        if last_mc_handle.gen_utime() + 600 > self.now() {
            return Ok(true);
        }

        if self.last_known_keyblock_seqno.load(Ordering::Relaxed) > last_applied_mc_id.seq_no() {
            return Ok(false);
        }

        // experimental check. t-node doesn't have one
        //if self.last_known_mc_block_seqno.load(Ordering::Relaxed) > last_mc_id.seq_no() + 16 {
        //    return Ok(false);
        //}

        Ok(self.is_validator())
    }

    pub fn load_pss_keeper_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db().load_full_node_state(PSS_KEEPER_MC_BLOCK)
    }

    pub fn save_pss_keeper_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
        self.db().save_full_node_state(PSS_KEEPER_MC_BLOCK, id)
    }

    pub fn load_archives_gc_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db().load_full_node_state(ARCHIVES_GC_BLOCK)
    }

    pub fn save_archives_gc_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
        self.db().save_full_node_state(ARCHIVES_GC_BLOCK, id)
    }

    pub fn start_archives_gc(
        engine: Arc<Engine>,
        archives_gc_block: BlockIdExt,
    ) -> Result<tokio::task::JoinHandle<()>> {
        log::info!("start_archives_gc");
        let join_handle = tokio::spawn(async move {
            engine.acquire_stop(Engine::MASK_SERVICE_ARCHIVES_GC);
            if let Err(e) = Self::archives_gc_worker(&engine, archives_gc_block).await {
                log::error!("CRITICAL!!! Unexpected error in archives gc: {:?}", e);
            }
            engine.release_stop(Engine::MASK_SERVICE_ARCHIVES_GC);
        });
        Ok(join_handle)
    }

    pub async fn archives_gc_worker(
        engine: &Arc<Engine>,
        archives_gc_block: BlockIdExt,
    ) -> Result<()> {
        fn check_unapplied_block_id(
            mut id: BlockIdExt,
            engine: &Arc<Engine>,
        ) -> Result<Option<BlockIdExt>> {
            loop {
                let handle = engine
                    .load_block_handle(&id)?
                    .ok_or_else(|| error!("No handle for block {} in DB", id))?;
                if handle.is_archived() {
                    return Ok(Some(id));
                }
                if engine.load_block_prev2(&id).is_ok() {
                    // Skip drop unapplied block right after merge
                    return Ok(None);
                }
                id = engine.load_block_prev1(&id)?
            }
        }

        if !archives_gc_block.shard().is_masterchain() {
            fail!("'archives_gc_block' must belong master chain");
        }
        let mut handle = engine.load_block_handle(&archives_gc_block)?.ok_or_else(|| {
            error!("Cannot load handle for archives_gc_block {}", archives_gc_block)
        })?;
        let mut last_clean_unapplied_time = std::time::Instant::now();
        'm: loop {
            let mc_state = engine.load_state(handle.id()).await?;
            if engine.check_stop() {
                break 'm;
            }
            if handle.is_key_block()? {
                let mc_state = engine.load_state(handle.id()).await?;
                if let Err(e) = Self::check_gc_for_archives(engine, &handle, &mc_state).await {
                    log::error!("Archives GC: {}", e);
                }
            }
            // clean unapplied blocks every 15 seconds
            if last_clean_unapplied_time.elapsed().as_secs() > 15 {
                let mut ids = Vec::new();
                for id in mc_state.top_blocks_all()? {
                    match check_unapplied_block_id(id, engine) {
                        Ok(Some(id)) => ids.push(id),
                        Err(e) => log::warn!("unapplied files gc: {}", e),
                        _ => (),
                    }
                }
                engine.db().clean_unapplied_files(&ids).await;
                last_clean_unapplied_time = std::time::Instant::now();
            }
            handle = loop {
                match engine.wait_next_applied_mc_block(&handle, Some(500)).await {
                    Ok(r) => break r.0,
                    Err(_) => {
                        if engine.check_stop() {
                            break 'm;
                        }
                    }
                }
            };
            engine.save_archives_gc_mc_block_id(handle.id())?;
        }
        Ok(())
    }

    async fn check_gc_for_archives(
        engine: &Arc<Engine>,
        last_keyblock: &Arc<BlockHandle>,
        mc_state: &ShardStateStuff,
    ) -> Result<()> {
        let mut gc_max_date = UnixTime::now();
        match &engine.archives_life_time_hours {
            None => return Ok(()),
            Some(life_time) => match gc_max_date.checked_sub(*life_time as u64 * 3600) {
                Some(date) => {
                    log::info!("archive gc: checked date {}.", date);
                    gc_max_date = date
                }
                None => {
                    log::info!(
                        "archive gc: life_time in config is bad, actual checked date: {}",
                        &gc_max_date
                    );
                }
            },
        }

        let mut visited_pss_blocks = 0;
        let mut keyblock = last_keyblock.clone();
        let prev_blocks = &mc_state.shard_state_extra()?.prev_blocks;
        loop {
            match prev_blocks.get_prev_key_block(keyblock.id().seq_no() - 1)? {
                None => return Ok(()),
                Some(prev_keyblock) => {
                    let prev_keyblock = BlockIdExt::from_ext_blk(prev_keyblock);
                    let prev_keyblock =
                        engine.load_block_handle(&prev_keyblock)?.ok_or_else(|| {
                            error!(
                                "Cannot load handle for PSS keeper prev key block {}",
                                prev_keyblock
                            )
                        })?;
                    if engine.is_persistent_state(
                        keyblock.gen_utime(),
                        prev_keyblock.gen_utime(),
                        boot::PSS_PERIOD_BITS,
                    ) {
                        visited_pss_blocks += 1;

                        // Due to boot process specific (pss period and key_block_utime_step combinations)
                        // we shouldn't delete last 4 pss blocks
                        // ....................pss_block....pss_block....pss_block....pss_block...
                        // visited_pss_blocks:         4            3            2            1
                        //                    ↑ we may delete blocks starting at least here (before 4th pss)

                        let pss_block = engine
                            .load_pss_keeper_mc_block_id()?
                            .ok_or_else(|| error!("Cannot load pss keeper mc block id"))?;
                        if keyblock.id().seq_no() < pss_block.seq_no() && visited_pss_blocks >= 4 {
                            let gen_time = keyblock.gen_utime() as u64;
                            let gc_max_date = gc_max_date;
                            if gen_time < gc_max_date {
                                log::info!(
                                    "gc for archives: found block (gen time: {}, seq_no: {}), gc max date: {}",
                                    &gen_time, keyblock.id().seq_no(), &gc_max_date
                                );
                                log::info!("start gc for archives..");
                                engine.db.archive_gc(keyblock.id()).await?;
                                log::info!("finish gc for archives.");
                                return Ok(());
                            }
                        }
                    }
                    if prev_keyblock.id().seq_no() == 0 {
                        return Ok(());
                    }
                    keyblock = prev_keyblock;
                }
            }
        }
    }

    fn check_finish_sync(self: Arc<Self>) {
        tokio::spawn(async move {
            const SLEEP_TIME: u64 = 30;
            loop {
                if let Ok(true) = self.check_sync().await {
                    self.set_sync_status(Engine::SYNC_STATUS_FINISH_SYNC);
                    return;
                };
                tokio::time::sleep(Duration::from_secs(SLEEP_TIME)).await;
            }
        });
    }

    #[allow(dead_code)]
    pub async fn truncate_database(&self, mc_seq_no: u32) -> Result<()> {
        log::warn!("Truncate database at master block seqno {}", mc_seq_no);
        let mc_state = self.load_last_applied_mc_state().await?;
        let block_id = mc_state.find_block_id(mc_seq_no)?;
        let prev_block_id = self.load_block_prev1(&block_id)?;
        log::warn!(
            "Truncate database: {} and newer will be deleted. New last mc will be {}",
            block_id,
            prev_block_id
        );
        // check if previous state is present
        self.load_state(&prev_block_id)
            .await
            .map_err(|err| error!("no previous block state present for {}", err))?;

        self.db().truncate_database(&block_id).await?;
        self.save_last_applied_mc_block_id(&prev_block_id)?;

        if let Some(block_id) = self.load_pss_keeper_mc_block_id()? {
            if block_id.seq_no > prev_block_id.seq_no {
                self.save_pss_keeper_mc_block_id(&prev_block_id)?;
            }
        }
        if let Some(block_id) = self.load_shard_client_mc_block_id()? {
            if block_id.seq_no > prev_block_id.seq_no {
                self.save_shard_client_mc_block_id(&prev_block_id)?;
            }
        }
        if let Some(block_id) = self.load_last_rotation_block_id()? {
            if block_id.seq_no > prev_block_id.seq_no {
                self.save_last_rotation_block_id(&prev_block_id)?;
            }
        }
        if let Some(block_id) = self.load_archives_gc_mc_block_id()? {
            if block_id.seq_no > prev_block_id.seq_no {
                self.save_archives_gc_mc_block_id(&prev_block_id)?;
            }
        }
        log::warn!("Database successfully truncated, new last mc block is {}", prev_block_id);
        Ok(())
    }

    pub fn get_account_storage_dict(&self, dict_hash: &UInt256) -> Option<Cell> {
        self.storage_dicts_cache.lock().1.get(dict_hash).map(|d| d.dict.clone())
    }

    pub fn add_account_storage_dict(&self, dict: Cell, size: u64) {
        if size < Self::MIN_CACHED_STORAGE_DICT_CELLS {
            return;
        }
        let mut cache = self.storage_dicts_cache.lock();
        if cache.1.push(dict.repr_hash().clone(), StorageDictInfo { dict, size }).is_none() {
            cache.0 += size;
        }
        while cache.0 > Self::STORAGE_DICTS_CACHE_SIZE {
            if let Some((_, dict)) = cache.1.pop_lru() {
                cache.0 -= dict.size;
            } else {
                break;
            }
        }
    }
}

pub(crate) async fn load_zero_state(engine: &Arc<Engine>, path: &str) -> Result<bool> {
    let zero_id = engine.zerostate_id();
    log::trace!("loading mc static zero state {} from path {}", zero_id, path);

    if let Some(handle) = engine.load_block_handle(zero_id)? {
        if handle.is_applied() {
            log::trace!("zero state already applied");
            return Ok(false);
        }
    }

    let (mc_zero_state, mc_zs_bytes) = {
        let path = format!("{}/{:x}.boc", path, zero_id.file_hash());
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|err| error!("Cannot read mc zerostate {}: {}", path, err))?;
        (
            ShardStateStuff::deserialize_zerostate(
                zero_id.clone(),
                &bytes,
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry(),
                engine.engine_allocated(),
            )?,
            bytes,
        )
    };

    let workchains = mc_zero_state.workchains()?;
    for (wc_id, wc_info) in workchains {
        let id = BlockIdExt {
            shard_id: ShardIdent::with_tagged_prefix(wc_id, SHARD_FULL)?,
            seq_no: 0,
            root_hash: wc_info.zerostate_root_hash,
            file_hash: wc_info.zerostate_file_hash,
        };
        if let Some(handle) = engine.load_block_handle(&id)? {
            if handle.is_applied() {
                continue;
            }
        }
        log::trace!("loading wc static zero state {}", id);
        let path = format!("{}/{:x}.boc", path, id.file_hash());
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|err| error!("Cannot read zerostate {}: {}", path, err))?;
        let zs = ShardStateStuff::deserialize_zerostate(
            id.clone(),
            &bytes,
            #[cfg(feature = "telemetry")]
            engine.engine_telemetry(),
            engine.engine_allocated(),
        )?;
        let (_zs, handle) = engine.store_zerostate(zs, &bytes).await?;
        engine.set_applied(&handle, id.seq_no()).await?;
    }

    let (_mc_zero_state, handle) = engine.store_zerostate(mc_zero_state, &mc_zs_bytes).await?;
    engine.set_applied(&handle, zero_id.seq_no()).await?;

    log::trace!("All static zero states had been load");
    Ok(true)
}

struct BootInfo {
    archives_gc_block: BlockIdExt,
    last_applied_mc_block: BlockIdExt,
    shard_client_mc_block: BlockIdExt,
    ss_keeper_mc_block: BlockIdExt,
}

async fn boot(
    engine: &Arc<Engine>,
    zerostate_path: Option<&str>,
    hardfork_path: impl AsRef<Path>,
    pss_downloading_threads: usize,
) -> Result<BootInfo> {
    log::info!("Booting...");
    engine.set_sync_status(Engine::SYNC_STATUS_START_BOOT);

    if let Some(zerostate_path) = zerostate_path {
        load_zero_state(engine, zerostate_path).await?;
    }

    let result = match engine.load_last_applied_mc_block_id() {
        Ok(Some(id)) => crate::boot::warm_boot(engine.clone(), id, hardfork_path).await,
        Ok(None) => Err(error!("No last applied MC block, warm boot is not possible")),
        Err(x) => Err(x),
    };

    let (last_applied_mc_block, cold) = match result {
        Ok(block_id) => (block_id.clone(), false),
        Err(err) => {
            log::warn!("Before cold boot: {err}");
            engine.acquire_stop(Engine::MASK_SERVICE_BOOT);
            let result = boot::cold_boot(engine.clone(), pss_downloading_threads).await;
            engine.release_stop(Engine::MASK_SERVICE_BOOT);
            let id = result?.id().clone();
            engine.save_last_applied_mc_block_id(&id)?;

            (id, true)
        }
    };

    let shard_client_mc_block = match engine.load_shard_client_mc_block_id() {
        Ok(Some(id)) => id.deref().clone(),
        _ => {
            if !cold {
                fail!("INTERNAL ERROR: No shard client MC block in warm boot")
            }
            engine.save_shard_client_mc_block_id(&last_applied_mc_block)?;
            log::info!("Shard client MC block reset to last applied MC block");
            last_applied_mc_block.clone()
        }
    };

    let ss_keeper_mc_block = match engine.db().load_full_node_state(PSS_KEEPER_MC_BLOCK) {
        Ok(Some(id)) => id.deref().clone(),
        _ => {
            if !cold {
                fail!("INTERNAL ERROR: No shard states keeper MC block in warm boot")
            }
            engine.save_pss_keeper_mc_block_id(&last_applied_mc_block)?;
            log::info!("SS keeper MC block reset to last applied MC block");
            last_applied_mc_block.clone()
        }
    };

    let archives_gc_block = match engine.db().load_full_node_state(ARCHIVES_GC_BLOCK) {
        Ok(Some(id)) => id.deref().clone(),
        _ => {
            // for compatibility don't catch error if there isn't state
            engine.save_archives_gc_mc_block_id(&last_applied_mc_block)?;
            log::info!("Archives gc MC block reset to last_applied_mc_block");
            last_applied_mc_block.clone()
        }
    };

    let state = engine.load_last_applied_mc_state().await?;
    engine.update_public_overlays(&state.last_keyblock_id()?, state.config_params()?).await?;

    engine.set_sync_status(Engine::SYNC_STATUS_FINISH_BOOT);
    log::info!("Boot complete.");
    log::info!("last_applied_mc_block: {}", last_applied_mc_block);
    log::info!("shard_client_mc_block: {}", shard_client_mc_block);
    log::info!("ss_keeper_mc_block: {}", ss_keeper_mc_block);
    log::info!("archives_gc_block: {}", archives_gc_block);
    // engine.db().db().create_checkpoint("/node/after_boot")?; // For debug

    let ret = BootInfo {
        archives_gc_block,
        last_applied_mc_block,
        shard_client_mc_block,
        ss_keeper_mc_block,
    };
    Ok(ret)
}

#[derive(Default)]
pub struct EngineFlags {
    pub initial_sync_disabled: bool,
    pub force_check_db: bool,
    pub truncate_db: Option<u32>,
}

pub async fn run(
    node_config: TonNodeConfig,
    zerostate_path: Option<&str>,
    validator_runtime: tokio::runtime::Handle,
    liteserver_runtime: tokio::runtime::Handle,
    flags: EngineFlags,
    stopper: Arc<Stopper>,
    metrics: Option<(std::net::SocketAddr, metrics_exporter_prometheus::PrometheusHandle)>,
) -> Result<(Arc<Engine>, tokio::task::JoinHandle<()>)> {
    log::info!("Engine::run");

    let control_server_config = node_config.control_server()?;
    let lite_server_config = node_config.lite_server()?;
    let json_rpc_server_config = node_config.json_rpc_server()?;
    let vm_config = ValidatorManagerConfig::read_configs(
        node_config.unsafe_catchain_patches_files(),
        node_config.validation_countdown_mode(),
        node_config.is_accelerated_consensus_disabled(),
    );
    let configs_dir = node_config.build_config_path("");
    let sync_by_archives = node_config.sync_by_archives();
    let custom_overlays_config = node_config.custom_overlays_config().to_vec();
    let pss_downloading_threads = node_config.pss_downloading_threads();

    // Create engine
    let truncate_db = flags.truncate_db;
    let engine = Engine::new(node_config, flags, stopper.clone()).await?;
    if let Some(mc_seq_no) = truncate_db {
        engine.truncate_database(mc_seq_no).await?;
    }
    let engine_ret = engine.clone();
    let result = async move {
        #[cfg(feature = "telemetry")]
        telemetry_logger(engine.clone());

        // control server
        if let Some(config) = control_server_config {
            let server = ControlServer::with_params(
                config,
                DataSource::Engine(engine.clone()),
                engine.network().config_handler(),
                engine.network().config_handler(),
                Some(engine.network()),
            )
            .await?;
            engine.servers.push(Box::new(server));
        }

        // liteserver
        if let Some(config) = lite_server_config {
            let server =
                LiteServer::with_params(config, liteserver_runtime, engine.clone()).await?;
            engine.servers.push(Box::new(server));
        }

        // jsonrpc server
        if let Some(config) = json_rpc_server_config {
            let server = RpcServer::start(config, engine.clone()).await?;
            engine.servers.push(Box::new(server));
        }

        // metrics server (Prometheus + health probes)
        if let Some((address, handle)) = metrics {
            let server = start_metrics_server(engine.clone(), address, handle).await;
            engine.servers.push(Box::new(server));
        }

        // Overlays
        let overlays_router = FullNodeOverlaysRouter::new(
            engine.clone(),
            engine.network.clone(),
            DhtSearchPolicy::default(),
        )
        .await?;
        overlays_router.update_custom_overlays(Some(&custom_overlays_config)).await?;
        engine
            .overlays_router
            .set(overlays_router)
            .map_err(|_| error!("Overlays router already set"))?;

        // Boot
        let mut boot_info =
            boot(&engine, zerostate_path, configs_dir, pss_downloading_threads).await?;

        let out_msg_queue_manager = OutMsgQueueManager::new(engine.clone()).await?;
        engine
            .out_msg_queue_manager
            .set(out_msg_queue_manager.clone())
            .map_err(|_| error!("OutMsgQueueManager already set"))?;

        Engine::start_archives_gc(engine.clone(), boot_info.archives_gc_block)?;

        engine
            .shard_states_keeper
            .clone()
            .start(
                engine.clone(),
                boot_info.last_applied_mc_block.clone(),
                boot_info.shard_client_mc_block.clone(),
                boot_info.ss_keeper_mc_block,
            )
            .await?;

        // Start validator manager, which will start validator sessions when necessary
        start_validator_manager(
            Arc::clone(&engine) as Arc<dyn EngineOperations>,
            validator_runtime,
            vm_config,
        );

        // Sync by archives
        if sync_by_archives && !engine.check_sync().await? {
            engine.set_sync_status(Engine::SYNC_STATUS_SYNC_ARCHIVES);
            struct Checker;
            #[async_trait::async_trait]
            impl crate::sync::StopSyncChecker for Checker {
                async fn check(&self, engine: &Arc<dyn EngineOperations>) -> bool {
                    engine.check_sync().await.unwrap_or(false)
                }
            }
            crate::sync::start_sync(
                Arc::clone(&engine) as Arc<dyn EngineOperations>,
                Some(&Checker),
                None,
            )
            .await?;
            boot_info.last_applied_mc_block = engine
                .load_last_applied_mc_block_id()?
                .ok_or_else(|| error!("INTERNAL ERROR: No last applied MC block after sync"))?
                .deref()
                .clone();
            boot_info.shard_client_mc_block = engine
                .load_shard_client_mc_block_id()?
                .ok_or_else(|| error!("INTERNAL ERROR: No shard client MC block after sync"))?
                .deref()
                .clone();
        }

        // top shard blocks
        resend_top_shard_blocks_worker(engine.clone());

        // blocks download clients
        engine.set_sync_status(Engine::SYNC_STATUS_SYNC_BLOCKS);
        Engine::check_finish_sync(Arc::clone(&engine));
        let join_shards = start_shards_client(engine.clone(), boot_info.shard_client_mc_block)?;
        let join_master =
            start_masterchain_client(engine.clone(), boot_info.last_applied_mc_block)?;
        Ok((join_shards, join_master))
    }
    .await;

    match result {
        Err(e) => {
            // Always wait for engine stop
            engine_ret.wait_stop().await;
            Err(e)
        }
        Ok((join_shards, join_master)) => {
            let join_engine = tokio::spawn(async move {
                let (_, _) = tokio::join!(join_master, join_shards);
            });
            Ok((engine_ret, join_engine))
        }
    }
}

#[cfg(feature = "telemetry")]
fn telemetry_logger(engine: Arc<Engine>) {
    tokio::spawn(async move {
        let mut elapsed = 0;
        let millis = 500;
        loop {
            tokio::time::sleep(Duration::from_millis(millis)).await;

            // update metrics

            engine
                .engine_telemetry
                .storage
                .file_entries
                .update(engine.engine_allocated.storage.file_entries.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .storage
                .handles
                .update(engine.engine_allocated.storage.handles.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .storage
                .packages
                .update(engine.engine_allocated.storage.packages.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .awaiters
                .update(engine.engine_allocated.awaiters.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .catchain_clients
                .update(engine.engine_allocated.catchain_clients.load(Ordering::Relaxed));
            engine.engine_telemetry.cells.update(Cell::cell_count());
            engine
                .engine_telemetry
                .shard_states
                .update(engine.engine_allocated.shard_states.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .top_blocks
                .update(engine.engine_allocated.top_blocks.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .validator_adnl_keys
                .update(engine.engine_allocated.validator_adnl_keys.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .validator_peers
                .update(engine.engine_allocated.validator_peers.load(Ordering::Relaxed));
            engine
                .engine_telemetry
                .validator_sets
                .update(engine.engine_allocated.validator_sets.load(Ordering::Relaxed));
            engine.engine_telemetry.account_state_cache_mb.update(
                engine.engine_allocated.account_state_cache_bytes.load(Ordering::Relaxed)
                    / (1024 * 1024),
            );
            engine
                .engine_telemetry
                .storage_dicts_cache_cells
                .update(engine.storage_dicts_cache.lock().0);

            // check timeout

            elapsed += millis;
            if elapsed < Engine::TIMEOUT_TELEMETRY_SEC * 1000 {
                continue;
            } else {
                elapsed = 0
            }

            // print telemetry

            {
                let usage = engine.db().rocksdb_memory_usage();
                engine
                    .engine_telemetry
                    .storage
                    .rocksdb_mem_table_mb
                    .update(usage.mem_tables / (1024 * 1024));
                engine
                    .engine_telemetry
                    .storage
                    .rocksdb_block_cache_mb
                    .update(usage.block_cache / (1024 * 1024));
            }

            let period = crate::full_node::telemetry::TPS_PERIOD_1;
            let tps_1 = engine.tps_counter.calc_tps(period).unwrap_or_else(|e| {
                log::error!("Can't calc tps for {}sec period: {}", period, e);
                0
            });
            let period = crate::full_node::telemetry::TPS_PERIOD_2;
            let tps_2 = engine.tps_counter.calc_tps(period).unwrap_or_else(|e| {
                log::error!("Can't calc tps for {}sec period: {}", period, e);
                0
            });
            log::debug!(
                target: "telemetry",
                "Full node's telemetry:\n{}",
                engine.full_node_telemetry().report(tps_1, tps_2)
            );
            log::debug!(
                target: "telemetry",
                "Collator's telemetry:\n{}",
                engine.collator_telemetry().report()
            );
            log::debug!(
                target: "telemetry",
                "Validator's telemetry:\n{}",
                engine.validator_telemetry().report()
            );
            log::debug!(
                target: "telemetry",
                "Full node service's telemetry:\n{}",
                engine.full_node_service_telemetry().report(Engine::TIMEOUT_TELEMETRY_SEC)
            );
            log::debug!(
                target: "telemetry",
                "Full node client's telemetry:\n{}",
                engine.network.telemetry().report(Engine::TIMEOUT_TELEMETRY_SEC)
            );
            log::debug!(
                target: "telemetry",
                "Full node neighbours's telemetry:",
            );
            if let Ok(or) = engine.overlays_router() {
                or.log_stat();
            }
            if let Err(e) = engine.log_workers_stats() {
                log::warn!("Can't log workers stats: {}", e);
            }
            {
                let hits = engine
                    .engine_telemetry
                    .storage
                    .cell_cache_hits
                    .metric()
                    .total_amount()
                    .unwrap_or(0);
                let misses = engine
                    .engine_telemetry
                    .storage
                    .cell_cache_misses
                    .metric()
                    .total_amount()
                    .unwrap_or(0);
                let total = hits + misses;
                let hit_rate = if total > 0 { hits * 100 / total } else { 0 };
                log::info!(
                    target: "telemetry",
                    "Cell cache hit_rate: {}%",
                    hit_rate
                );
            }
            engine.telemetry_printer.try_print();
        }
    });
}

pub fn init_prometheus_recorder(
    config: &crate::config::MetricsConfig,
) -> metrics_exporter_prometheus::PrometheusHandle {
    use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};

    let mut builder = PrometheusBuilder::new();

    for (suffix, buckets) in &config.histogram_buckets {
        builder = builder
            .set_buckets_for_metric(Matcher::Suffix(suffix.clone()), buckets)
            .expect("bucket values are not empty");
    }

    for (key, value) in &config.global_labels {
        builder = builder.add_global_label(key, value);
    }

    let handle = builder.install_recorder().expect("Could not create PrometheusRecorder");

    // -- build info
    metrics::describe_gauge!(
        "ton_node_build_info",
        "Build metadata (version, commit, branch, rust version, arch, os). Always 1."
    );
    let build_labels = [
        ("version", std::option_env!("CARGO_PKG_VERSION").unwrap_or("unknown")),
        ("commit", std::option_env!("BUILD_GIT_COMMIT").unwrap_or("unknown")),
        ("branch", std::option_env!("BUILD_GIT_BRANCH").unwrap_or("unknown")),
        ("build_time", std::option_env!("BUILD_TIME").unwrap_or("unknown")),
        ("rustversion", std::option_env!("BUILD_RUST_VERSION").unwrap_or("unknown")),
        ("arch", std::env::consts::ARCH),
        ("os", std::env::consts::OS),
    ];
    metrics::gauge!("ton_node_build_info", &build_labels).set(1.0);

    // -- engine
    metrics::describe_gauge!(
        "ton_node_engine_sync_status",
        "Sync state (0=not_set, 1=boot, 2=load_states, 3=finish_boot, \
        4=sync_archives, 5=sync_blocks, 6=synced, 7=checking_db, 8=db_broken)"
    );
    metrics::describe_gauge!(
        "ton_node_engine_timediff_seconds",
        "Seconds between now and last applied masterchain block"
    );
    metrics::describe_gauge!(
        "ton_node_engine_last_mc_block_utime",
        "Unix timestamp of last applied masterchain block"
    );
    metrics::describe_gauge!(
        "ton_node_engine_last_mc_block_seqno",
        "Seqno of last applied masterchain block"
    );
    metrics::describe_gauge!(
        "ton_node_engine_shards_mc_seqno",
        "MC block seqno last processed by shard client"
    );
    metrics::describe_gauge!(
        "ton_node_engine_shards_timediff_seconds",
        "Seconds between now and MC block last processed by shard client"
    );
    metrics::describe_gauge!(
        "ton_node_engine_will_validate",
        "1 if node intends to validate, 0 otherwise"
    );
    // -- validator
    metrics::describe_gauge!(
        "ton_node_validator_status",
        "Validation state (0=Disabled, 1=Waiting, 2=Active)"
    );
    metrics::describe_gauge!(
        "ton_node_validator_in_current_set",
        "1 if node is in current validator set (p34)"
    );
    metrics::describe_gauge!(
        "ton_node_validator_in_next_set",
        "1 if node is in next validator set (p36)"
    );
    metrics::describe_gauge!(
        "ton_node_validator_active",
        "Number of validation queries currently running"
    );
    metrics::describe_counter!(
        "ton_node_validator_successes_total",
        "Successful block validations"
    );
    metrics::describe_counter!("ton_node_validator_failures_total", "Failed block validations");
    metrics::describe_counter!(
        "ton_node_validator_ref_block_failures_total",
        "Failed ref shard block validations"
    );
    metrics::describe_histogram!(
        "ton_node_validator_gas_rate_ratio",
        "Gas rate ratio from validation"
    );
    // check_refs metrics are behind #[cfg(feature = "xp25")] in validate_query.rs

    // -- collator
    metrics::describe_gauge!(
        "ton_node_collator_active",
        "Number of collation queries currently running"
    );
    metrics::describe_counter!("ton_node_collator_successes_total", "Successful block collations");
    metrics::describe_counter!("ton_node_collator_failures_total", "Failed block collations");
    metrics::describe_histogram!("ton_node_collator_duration_seconds", "Block collation duration");
    metrics::describe_histogram!(
        "ton_node_collator_process_ext_messages_seconds",
        "Time to process inbound external messages"
    );
    metrics::describe_histogram!(
        "ton_node_collator_process_new_messages_seconds",
        "Time to process new messages"
    );
    metrics::describe_histogram!("ton_node_collator_gas_used", "Gas used per collated block");
    metrics::describe_histogram!(
        "ton_node_collator_gas_rate_ratio",
        "Gas rate ratio from collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_dequeued_messages_total",
        "Messages dequeued during collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_enqueued_messages_total",
        "Messages enqueued during collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_inbound_messages_total",
        "Inbound messages processed during collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_outbound_messages_total",
        "Outbound messages produced during collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_transit_messages_total",
        "Transit messages during collation"
    );
    metrics::describe_counter!(
        "ton_node_collator_executed_transactions_total",
        "Transactions executed during collation"
    );

    // -- outqueue
    metrics::describe_gauge!(
        "ton_node_outqueue_clean_partial",
        "1 if last outqueue clean was partial"
    );
    metrics::describe_gauge!(
        "ton_node_outqueue_clean_duration_seconds",
        "Duration of last outqueue clean"
    );
    metrics::describe_gauge!(
        "ton_node_outqueue_clean_processed",
        "Messages processed in last outqueue clean"
    );
    metrics::describe_gauge!(
        "ton_node_outqueue_clean_deleted",
        "Messages deleted in last outqueue clean"
    );

    // -- ext_messages
    metrics::describe_gauge!(
        "ton_node_ext_messages_queue_size",
        "Current external messages queue size"
    );
    metrics::describe_counter!(
        "ton_node_ext_messages_expired_total",
        "Expired external messages removed"
    );

    // -- network
    metrics::describe_histogram!(
        "ton_node_network_adnl_roundtrip_seconds",
        "ADNL query roundtrip time"
    );
    metrics::describe_histogram!(
        "ton_node_network_catchain_overlay_query_seconds",
        "Catchain overlay query time"
    );
    metrics::describe_histogram!(
        "ton_node_network_catchain_send_seconds",
        "Catchain send message time"
    );
    metrics::describe_histogram!(
        "ton_node_network_catchain_client_query_seconds",
        "Catchain client query time"
    );
    metrics::describe_histogram!(
        "ton_node_network_consensus_overlay_query_seconds",
        "Consensus overlay query time"
    );
    metrics::describe_counter!(
        "ton_node_network_neighbour_failures_total",
        "Failed queries to neighbours"
    );
    metrics::describe_gauge!(
        "ton_node_network_neighbour_unreliability",
        "Neighbour unreliability score"
    );

    // -- db
    metrics::describe_gauge!(
        "ton_node_db_shardstate_queue_size",
        "Shard state processing queue size"
    );
    metrics::describe_histogram!("ton_node_db_shardstate_gc_seconds", "Shard state GC duration");
    metrics::describe_histogram!(
        "ton_node_db_persistent_state_write_seconds",
        "Persistent state write duration"
    );
    metrics::describe_histogram!(
        "ton_node_db_restore_merkle_update_seconds",
        "Merkle update duration during chain restore"
    );
    metrics::describe_histogram!(
        "ton_node_db_calc_merkle_update_seconds",
        "Merkle update calculation duration"
    );

    // -- block
    metrics::describe_histogram!(
        "ton_node_block_accounts_parsing_seconds",
        "Block accounts parsing duration"
    );
    metrics::describe_histogram!(
        "ton_node_block_parsed_accounts",
        "Number of accounts parsed per block"
    );
    metrics::describe_histogram!("ton_node_block_size_bytes", "Block size in bytes");

    handle
}

pub struct MetricsServer {
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

#[async_trait::async_trait]
impl Stoppable for MetricsServer {
    fn name(&self) -> &'static str {
        "metrics"
    }
    async fn shutdown(self: Box<Self>) {
        self.shutdown.send(()).ok();
        self.join.await.ok();
    }
}

pub async fn start_metrics_server(
    engine: Arc<Engine>,
    address: std::net::SocketAddr,
    handle: metrics_exporter_prometheus::PrometheusHandle,
) -> MetricsServer {
    use warp::{Filter, Reply};

    let metrics_handle = handle.clone();
    let metrics_route = warp::path("metrics")
        .and(warp::get())
        .map(move || {
            metrics_handle.run_upkeep();
            warp::reply::with_header(
                metrics_handle.render(),
                "content-type",
                "text/plain; version=0.0.4; charset=utf-8",
            )
            .into_response()
        })
        .boxed();

    let probe_response = move || {
        let sync_status = engine.get_sync_status();
        let mc_seqno = engine.get_last_applied_mc_seqno();
        let validation_status = engine.validation_status() as u8;
        warp::reply::json(&serde_json::json!({
            "status": "ok",
            "sync_status": sync_status,
            "last_mc_block_seqno": mc_seqno,
            "validation_status": validation_status,
        }))
        .into_response()
    };

    let healthz_route = warp::path("healthz").and(warp::get()).map(probe_response.clone()).boxed();

    let readyz_route = warp::path("readyz").and(warp::get()).map(probe_response).boxed();

    let routes = metrics_route.or(healthz_route).unify().or(readyz_route).unify().boxed();

    let listener =
        tokio::net::TcpListener::bind(address).await.expect("Failed to bind metrics server");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        warp::serve(routes)
            .incoming(listener)
            .graceful(async move {
                rx.await.ok();
            })
            .run()
            .await;
    });

    MetricsServer { shutdown: tx, join }
}
