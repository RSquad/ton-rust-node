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
    cell_db::CellByHashStorageAdapter,
    db::{
        rocksdb::{RocksDb, RocksDbTable},
        DbKey,
    },
    dynamic_boc_rc_db::{AsyncCellsStorageAdapter, DynamicBocDb},
    error::StorageError,
    traits::Serializable,
    StorageAlloc, TARGET,
};
use std::{
    sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_block::{
    error, fail, BlockIdExt, Cell, CellsFactory, CellsStorage, Result, UInt256, UnixTime,
};

pub trait AllowStateGcResolver: Send + Sync {
    fn allow_state_gc(&self, block_id: &BlockIdExt, save_utime: u64, gc_utime: u64)
        -> Result<bool>;
}

pub(crate) struct DbEntry {
    // Because key in db is not a full BlockIdExt it's need to store it here to use while GC.
    pub block_id: BlockIdExt,
    pub cell_id: UInt256,
    pub save_utime: u64,
}

impl DbEntry {
    pub fn with_params(block_id: BlockIdExt, cell_id: UInt256, save_utime: u64) -> Self {
        Self { block_id, cell_id, save_utime }
    }
}

impl Serializable for DbEntry {
    const SIZE: usize = BlockIdExt::SIZE + 40;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        let mut ret = [0u8; Self::SIZE];
        ret[..BlockIdExt::SIZE].copy_from_slice(&self.block_id.serialize());
        ret[BlockIdExt::SIZE..BlockIdExt::SIZE + 32].copy_from_slice(self.cell_id.key());
        ret[BlockIdExt::SIZE + 32..].copy_from_slice(&self.save_utime.serialize());
        ret
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let block_id = BlockIdExt::deserialize_checked(data)?;
        let cell_id = UInt256::from(&data[BlockIdExt::SIZE..BlockIdExt::SIZE + 32]);
        let save_utime = u64::deserialize_checked(&data[BlockIdExt::SIZE + 32..])?;
        let ret = Self { block_id, cell_id, save_utime };
        Ok(ret)
    }
}

pub enum Job {
    PutState(Cell, BlockIdExt),
    DeleteState(BlockIdExt),
}

#[async_trait::async_trait]
pub trait Callback: Sync + Send {
    /// Invoked only on successful apply; failures are retried, stop skips it.
    async fn invoke(&self, job: Job);
}

pub struct SsNotificationCallback(tokio::sync::Notify);

#[async_trait::async_trait]
impl Callback for SsNotificationCallback {
    async fn invoke(&self, _job: Job) {
        self.0.notify_one();
    }
}

impl SsNotificationCallback {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(tokio::sync::Notify::new()))
    }
    pub async fn wait(&self) {
        self.0.notified().await;
    }
}

impl Job {
    pub fn block_id(&self) -> &BlockIdExt {
        match self {
            Job::PutState(_cell, id) => id,
            Job::DeleteState(id) => id,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct CellsDbConfig {
    #[serde(deserialize_with = "CellsDbConfig::deserialize_states_db_queue_len")]
    pub states_db_queue_len: u32,
    #[serde(default, skip_serializing, rename = "prefill_cells_counters")]
    _prefill_cells_counters: Option<bool>,
    pub cells_cache_size_bytes: u64,
    pub counters_cache_size_bytes: u64,
    #[serde(default = "CellsDbConfig::default_cells_lru_cache_capacity")]
    pub cells_lru_cache_capacity: usize,
    #[serde(default = "CellsDbConfig::default_counters_lru_cache_capacity")]
    pub counters_lru_cache_capacity: usize,
}

impl CellsDbConfig {
    fn default_cells_lru_cache_capacity() -> usize {
        5_000_000
    }
    fn default_counters_lru_cache_capacity() -> usize {
        5_000_000
    }
    fn deserialize_states_db_queue_len<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<u32, D::Error> {
        let v = <u32 as serde::Deserialize>::deserialize(d)?;
        if v < Self::min_states_db_queue_len() {
            return Err(serde::de::Error::custom(format!(
                "states_db_queue_len must be >= {}, got {v}",
                Self::min_states_db_queue_len()
            )));
        }
        Ok(v)
    }
    fn min_states_db_queue_len() -> u32 {
        100
    }
}

impl Default for CellsDbConfig {
    fn default() -> Self {
        Self {
            states_db_queue_len: 1000,
            _prefill_cells_counters: None,
            cells_cache_size_bytes: 2_000_000_000,
            counters_cache_size_bytes: 1_000_000_000,
            cells_lru_cache_capacity: Self::default_cells_lru_cache_capacity(),
            counters_lru_cache_capacity: Self::default_counters_lru_cache_capacity(),
        }
    }
}

pub struct ShardStateDb {
    shardstate_db: Arc<RocksDbTable<BlockIdExt>>,
    dynamic_boc_db: Arc<DynamicBocDb>,
    storer: tokio::sync::mpsc::UnboundedSender<(Job, Option<Arc<dyn Callback>>)>,
    in_queue: AtomicU32,
    stop: Arc<AtomicU8>,
    config: CellsDbConfig,
    gc_resolver: tokio::sync::OnceCell<Arc<dyn AllowStateGcResolver>>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<StorageTelemetry>,
}

impl ShardStateDb {
    const MASK_GC: u8 = 0x01;
    const MASK_WORKER: u8 = 0x02;
    pub(crate) const MASK_STOPPED: u8 = 0x80;
    const GC_QUEUE_POLL_INTERVAL_MS: u64 = 10_000;
    const PUT_QUEUE_POLL_INTERVAL_MS: u64 = 100;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<RocksDb>,
        shardstate_db_cf: &str,
        cell_db_cf: &str,
        counters_cf_name: &str,
        config: CellsDbConfig,
        #[cfg(feature = "telemetry")] telemetry: Arc<StorageTelemetry>,
        allocated: Arc<StorageAlloc>,
    ) -> Result<Arc<Self>> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        let dynamic_boc_db = DynamicBocDb::with_db(
            db.clone(),
            cell_db_cf,
            counters_cf_name,
            &config,
            #[cfg(feature = "telemetry")]
            telemetry.clone(),
            allocated.clone(),
        )?;

        let ss_db = Arc::new(Self {
            shardstate_db: Arc::new(RocksDbTable::with_db(db.clone(), shardstate_db_cf, true)?),
            dynamic_boc_db: Arc::new(dynamic_boc_db),
            storer: sender,
            in_queue: AtomicU32::new(0),
            stop: Arc::new(AtomicU8::new(0)),
            config,
            gc_resolver: tokio::sync::OnceCell::new(),
            #[cfg(feature = "telemetry")]
            telemetry,
        });

        tokio::spawn({
            let ss_db = ss_db.clone();
            async move {
                ss_db.worker(receiver).await;
            }
        });

        Ok(ss_db)
    }

    pub fn start_gc(
        self: Arc<Self>,
        gc_resolver: Arc<dyn AllowStateGcResolver>,
        run_interval_adjustable_sec: Arc<AtomicU32>,
    ) {
        if self.gc_resolver.set(gc_resolver.clone()).is_err() {
            log::error!(target: TARGET, "INTERNAL ERROR: Attempt to set GC resolver twice");
        }
        self.stop.fetch_or(Self::MASK_GC, Ordering::Relaxed);
        tokio::spawn(async move {
            fn check_and_stop(stop: &AtomicU8) -> bool {
                if (stop.load(Ordering::Relaxed) & ShardStateDb::MASK_STOPPED) != 0 {
                    stop.fetch_and(!ShardStateDb::MASK_GC, Ordering::Relaxed);
                    log::warn!(target: TARGET, "ShardStateDb GC: stopped");
                    true
                } else {
                    false
                }
            }

            async fn sleep_nicely(stop: &AtomicU8, mut sleep_for_ms: u64) -> bool {
                const TIMEOUT_STEP: u64 = 1000;
                loop {
                    let interval =
                        if sleep_for_ms > TIMEOUT_STEP { TIMEOUT_STEP } else { sleep_for_ms };
                    tokio::time::sleep(Duration::from_millis(interval)).await;
                    if check_and_stop(stop) {
                        return false;
                    }
                    if sleep_for_ms <= TIMEOUT_STEP {
                        return true;
                    }
                    sleep_for_ms -= TIMEOUT_STEP;
                }
            }

            async fn wait_queue(in_queue: &AtomicU32, stop: &AtomicU8, max_queue_len: u32) -> bool {
                loop {
                    let in_queue = in_queue.load(Ordering::Relaxed);
                    metrics::gauge!("ton_node_db_shardstate_queue_size").set(in_queue as f64);
                    if in_queue >= max_queue_len / 2 {
                        log::warn!(
                            target: TARGET,
                            "ShardStateDb GC: queue is half full (length: {}), waiting...",
                            in_queue
                        );
                        if !sleep_nicely(stop, ShardStateDb::GC_QUEUE_POLL_INTERVAL_MS).await {
                            return false;
                        }
                    } else {
                        return true;
                    }
                }
            }

            log::debug!(target: TARGET, "ShardStateDb GC: started worker");

            let mut to_delete: Vec<BlockIdExt> = vec![];
            loop {
                let run_gc_interval = run_interval_adjustable_sec.load(Ordering::Relaxed) as u64;
                if to_delete.is_empty() {
                    log::debug!(target: TARGET, "ShardStateDb GC: waiting for {run_gc_interval}sec...");
                    if !sleep_nicely(&self.stop, run_gc_interval * 1000).await {
                        return;
                    }
                } else {
                    let interval_ms = (run_gc_interval * 1000) / (to_delete.len() + 1) as u64;
                    while let Some(id) = to_delete.pop() {
                        if !wait_queue(&self.in_queue, &self.stop, self.config.states_db_queue_len)
                            .await
                        {
                            return;
                        }
                        let in_queue = self.in_queue.fetch_add(1, Ordering::Relaxed) + 1;
                        metrics::gauge!("ton_node_db_shardstate_queue_size").set(in_queue as f64);

                        let now = std::time::Instant::now();
                        let callback = SsNotificationCallback::new();
                        if let Err(e) =
                            self.storer.send((Job::DeleteState(id.clone()), Some(callback.clone())))
                        {
                            log::error!(
                                target: TARGET,
                                "Can't send state to delete from db, id {}",
                                e.0.0.block_id()
                            );
                        } else {
                            #[cfg(feature = "telemetry")]
                            self.telemetry
                                .shardstates_queue
                                .update(std::cmp::max(0, in_queue) as u64);
                            log::trace!(target: TARGET, "ShardStateDb GC: in_queue {}", in_queue);

                            loop {
                                let result = tokio::time::timeout(
                                    Duration::from_millis(1000),
                                    callback.wait(),
                                )
                                .await;
                                if let Err(tokio::time::error::Elapsed { .. }) = result {
                                    if check_and_stop(&self.stop) {
                                        return;
                                    }
                                } else {
                                    break;
                                }
                            }

                            let elapsed = now.elapsed();
                            metrics::histogram!("ton_node_db_shardstate_gc_seconds")
                                .record(elapsed);
                            let elapsed = elapsed.as_millis() as u64;
                            if elapsed > interval_ms {
                                log::debug!(
                                    target: TARGET,
                                    "ShardStateDb GC: deleting state {} was slower then given \
                                    interval, TIME {} ms, slot {} ms",
                                    id, elapsed, interval_ms,
                                );
                                if check_and_stop(&self.stop) {
                                    return;
                                }
                            } else if !sleep_nicely(&self.stop, interval_ms - elapsed).await {
                                return;
                            }
                        }
                    }
                }

                log::debug!(target: TARGET, "ShardStateDb GC: collecting states to delete");

                let mut kept = 0;
                self.shardstate_db
                    .for_each(&mut |_key, value| {
                        if check_and_stop(&self.stop) {
                            return Ok(false);
                        }
                        let entry = DbEntry::deserialize(value)?;
                        let time = UnixTime::now();
                        match gc_resolver.allow_state_gc(&entry.block_id, entry.save_utime, time) {
                            Ok(true) => {
                                log::debug!(
                                target: TARGET, "ShardStateDb GC: delete  id {}", entry.block_id);
                                to_delete.push(entry.block_id);
                            }
                            Ok(false) => {
                                kept += 1;
                                log::debug!(
                                target: TARGET, "ShardStateDb GC: keep  id {}", entry.block_id);
                            }
                            Err(e) => log::warn!(
                                target: TARGET,
                                "ShardStateDb  allow_state_gc  id {}  root_cell_id {:x}  error {}",
                                entry.block_id, entry.cell_id, e
                            ),
                        }
                        Ok(true)
                    })
                    .expect("Can't return error");

                if check_and_stop(&self.stop) {
                    return;
                }

                log::info!(
                    target: TARGET,
                    "ShardStateDb GC: collected {} states to delete, kept {}",
                    to_delete.len(), kept
                );

                // Sort ids by decreasing seqno. This way differences between
                // states will be smaller, so each delete operation will be faster
                // (last in the vector - the earliest state - will be deleted first)
                to_delete.sort_by_key(|b| std::cmp::Reverse(b.seq_no()));
            }
        });
    }

    pub async fn stop(&self) {
        self.stop.fetch_or(Self::MASK_STOPPED, Ordering::Relaxed);
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !self.is_run() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                break;
            }
        }
    }

    pub fn is_gc_run(&self) -> bool {
        self.stop.load(Ordering::Relaxed) & Self::MASK_GC != 0
    }

    fn is_run(&self) -> bool {
        (self.stop.load(Ordering::Relaxed) & !Self::MASK_STOPPED) != 0
    }

    pub fn shardstate_db(&self) -> Arc<RocksDbTable<BlockIdExt>> {
        Arc::clone(&self.shardstate_db)
    }

    #[cfg(test)]
    pub fn enum_shardstate_db(&self) -> Result<()> {
        self.shardstate_db.for_each(&mut |_key, val| {
            let db_entry = DbEntry::deserialize(val)?;
            println!("{}", db_entry.block_id);
            Ok(true)
        })?;
        Ok(())
    }

    pub async fn put(
        &self,
        id: &BlockIdExt,
        state_root: Cell,
        callback: Option<Arc<dyn Callback>>,
    ) -> Result<()> {
        let root_id = state_root.repr_hash().clone();
        log::debug!(
            target: TARGET,
            "ShardStateDb::put  id {}  root_cell_id {:x}",
            id, root_id
        );
        let mut attempt = 0usize;
        loop {
            let in_queue = self.in_queue.load(Ordering::Relaxed);
            if in_queue >= self.config.states_db_queue_len {
                if attempt % 10 == 0 {
                    log::warn!(
                        target: TARGET,
                        "ShardStateDb::put id {id}  root_cell_id {root_id:x} \
                        waiting for queue (current queue length: {in_queue})"
                    );
                }
                tokio::time::sleep(Duration::from_millis(Self::PUT_QUEUE_POLL_INTERVAL_MS)).await;
                attempt += 1;

                if self.stop.load(Ordering::Relaxed) & Self::MASK_STOPPED != 0 {
                    fail!("Stopped");
                }
            } else {
                break;
            }
        }

        let in_queue = self.in_queue.fetch_add(1, Ordering::Relaxed) + 1;

        self.storer.send((Job::PutState(state_root, id.clone()), callback)).map_err(|_| {
            error!("Can't send state to put into db, id {}, root {:x}", id, root_id)
        })?;

        metrics::gauge!("ton_node_db_shardstate_queue_size").set(in_queue as f64);
        #[cfg(feature = "telemetry")]
        self.telemetry.shardstates_queue.update(std::cmp::max(0, in_queue) as u64);
        log::trace!("ShardStateDb put: in_queue {}", in_queue);

        Ok(())
    }

    pub fn get(&self, id: &BlockIdExt) -> Result<Cell> {
        let data = match self.shardstate_db.get(id) {
            Ok(data) => data,
            Err(e) => {
                if let Some(gc_resolver) = self.gc_resolver.get() {
                    if gc_resolver.allow_state_gc(id, 0, 0)? {
                        fail!(StorageError::StateIsAllowedToGc(id.clone()))
                    }
                }
                return Err(e);
            }
        };
        let db_entry = DbEntry::deserialize(&data)?;

        log::debug!(
            target: TARGET,
            "ShardStateDb::get  id {}  cell_id {:x}",
            id, db_entry.cell_id
        );

        if let Some(gc_resolver) = self.gc_resolver.get() {
            let utime_now = UnixTime::now();
            if gc_resolver.allow_state_gc(&db_entry.block_id, db_entry.save_utime, utime_now)? {
                fail!(StorageError::StateIsAllowedToGc(db_entry.block_id))
            }
        }

        let root_cell = self.dynamic_boc_db.load_cell(&db_entry.cell_id)?;
        Ok(root_cell)
    }

    pub fn get_cell(&self, id: &UInt256) -> Result<Cell> {
        log::trace!(target: TARGET, "ShardStateDb::get_cell  id {:x}", id);
        self.dynamic_boc_db.load_cell(id)
    }

    pub fn create_hashed_cell_storage(
        &self,
        root: Option<&Cell>,
        max_inmemory_cells: usize,
    ) -> Result<impl CellsStorage> {
        CellByHashStorageAdapter::new(
            self.dynamic_boc_db.cell_db().clone(),
            root,
            max_inmemory_cells,
        )
    }

    pub fn create_fast_cell_storage(
        &self,
        index: Vec<(UInt256, u16)>,
    ) -> Result<AsyncCellsStorageAdapter> {
        AsyncCellsStorageAdapter::new(self.dynamic_boc_db.clone(), index)
    }

    pub fn cells_factory(&self) -> Result<Arc<dyn CellsFactory>> {
        Ok(self.dynamic_boc_db.cells_factory())
    }

    pub fn enumerate_ids(
        &self,
        callback: &mut dyn FnMut(&BlockIdExt) -> Result<bool>,
    ) -> Result<()> {
        self.shardstate_db.for_each(&mut |_key, value| {
            let entry = DbEntry::deserialize(value)?;
            callback(&entry.block_id)
        })?;
        Ok(())
    }

    async fn worker(
        self: Arc<Self>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<(Job, Option<Arc<dyn Callback>>)>,
    ) {
        self.stop.fetch_or(Self::MASK_WORKER, Ordering::Relaxed);

        let check_stop = || {
            if self.stop.load(Ordering::Relaxed) & Self::MASK_STOPPED != 0 {
                self.stop.fetch_and(!ShardStateDb::MASK_WORKER, Ordering::Relaxed);
                true
            } else {
                false
            }
        };

        loop {
            if check_stop() {
                return;
            }
            if let Ok(Some((mut job, callback))) =
                tokio::time::timeout(Duration::from_millis(500), receiver.recv()).await
            {
                let in_queue = self.in_queue.fetch_sub(1, Ordering::Relaxed) - 1;
                metrics::gauge!("ton_node_db_shardstate_queue_size").set(in_queue as f64);
                #[cfg(feature = "telemetry")]
                self.telemetry.shardstates_queue.update(std::cmp::max(0, in_queue) as u64);
                log::debug!("ShardStateDb worker: in_queue {}", in_queue);

                loop {
                    match &mut job {
                        Job::PutState(cell, id) => {
                            match self.clone().put_internal(id, cell.clone()) {
                                Err(e) => {
                                    if check_stop() {
                                        return;
                                    }
                                    log::error!(
                                        target: TARGET, "CRITICAL! ShardStateDb::put_internal  {}", e
                                    );
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
                                Ok(saved_root) => {
                                    *cell = saved_root;
                                }
                            }
                        }
                        Job::DeleteState(id) => {
                            if let Err(e) = self.clone().delete_internal(id) {
                                if check_stop() {
                                    return;
                                }
                                log::error!(
                                    target: TARGET, "CRITICAL! ShardStateDb::delete_internal  {}", e
                                );
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        }
                    }
                    break;
                }
                if let Some(callback) = callback {
                    let _ = callback.invoke(job).await;
                }
            }
        }
    }

    fn put_internal(self: Arc<Self>, id: &BlockIdExt, state_root: Cell) -> Result<Cell> {
        let cell_id = state_root.repr_hash().clone();

        log::trace!(
            target: TARGET,
            "ShardStateDb::put_internal  id {}  root_cell_id {:x}",
            id, cell_id
        );

        if self.shardstate_db.contains(id)? {
            log::warn!(
                target: TARGET,
                "ShardStateDb::put_internal  ALREADY EXISTS  id {}  root_cell_id {:x}",
                id, cell_id
            );
            return self.dynamic_boc_db.load_cell(&cell_id);
        }

        let ss_db = self.clone();
        let saved_root = tokio::task::block_in_place(|| {
            let check_stop = || {
                if ss_db.stop.load(Ordering::Relaxed) & Self::MASK_STOPPED != 0 {
                    fail!("Stopped")
                } else {
                    Ok(())
                }
            };
            ss_db.dynamic_boc_db.save_boc(state_root, &check_stop)
        })?;

        let save_utime = UnixTime::now();
        let db_entry = DbEntry::with_params(id.clone(), cell_id.clone(), save_utime);
        self.shardstate_db.put(id, &db_entry.serialize())?;

        log::trace!(
            target: TARGET,
            "ShardStateDb::put_internal DONE  id {}  root_cell_id {:x}",
            id, cell_id
        );

        Ok(saved_root)
    }

    fn delete_internal(self: Arc<Self>, id: &BlockIdExt) -> Result<()> {
        log::trace!(target: TARGET, "ShardStateDb::delete_internal  id {}", id);

        let db_entry = DbEntry::deserialize(&self.shardstate_db.get(id)?)?;

        let ss_db = self.clone();
        let shardstate_db = self.shardstate_db.clone();
        let id_for_batch = id.clone();
        tokio::task::block_in_place(|| {
            let check_stop = || {
                if ss_db.stop.load(Ordering::Relaxed) & Self::MASK_STOPPED != 0 {
                    fail!("Stopped")
                } else {
                    Ok(())
                }
            };
            ss_db.dynamic_boc_db.delete_boc(&db_entry.cell_id, &check_stop, |batch| {
                shardstate_db.add_delete_to_batch(batch, &id_for_batch)
            })
        })?;

        log::trace!(target: TARGET, "ShardStateDb::delete_internal  DONE  id {}", id);
        Ok(())
    }
}
