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
    boot,
    config::ShardStatesCacheMode,
    engine::{Engine, Stopper},
    engine_traits::{EngineAlloc, EngineOperations},
    internal_db::{
        state_gc_resolver::AllowStateGcSmartResolver, InternalDb, LAST_APPLIED_MC_BLOCK,
    },
    shard_state::ShardStateStuff,
};
use adnl::common::add_unbound_object_to_map_with_update;
use std::{
    collections::HashSet,
    io::Cursor,
    ops::Deref,
    sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use storage::{
    block_handle_db::BlockHandle,
    dynamic_boc_rc_db::AsyncCellsStorageAdapter,
    error::StorageError,
    shardstate_db_async::{AllowStateGcResolver, SsNotificationCallback},
    types::PersistentStatePartId,
};
use ton_block::{
    error, fail, BlockIdExt, BocReader, Cell, CellsArena, Deserializable, HashmapSubtree,
    MerkleProof, OldMcBlocksInfo, Result, Serializable, ShardAccounts, ShardIdent,
    ShardStateUnsplit, SliceData, UInt256, UnixTime, UsageTree, BASE_WORKCHAIN_ID,
};
pub struct PinnedShardStateGuard {
    state: Arc<ShardStateStuff>,
    gc_resolver: Arc<AllowStateGcSmartResolver>,
}
impl PinnedShardStateGuard {
    pub fn new(
        state: Arc<ShardStateStuff>,
        gc_resolver: Arc<AllowStateGcSmartResolver>,
    ) -> Result<Self> {
        let now = UnixTime::now();
        // used a gen time as a save time because can't get save time here
        if !gc_resolver.pin_state(state.block_id(), state.state()?.gen_time() as u64, now)? {
            fail!(StorageError::StateIsAllowedToGc(state.block_id().clone()))
        }
        Ok(Self { state, gc_resolver })
    }
    pub fn state(&self) -> &ShardStateStuff {
        &self.state
    }
}
impl Clone for PinnedShardStateGuard {
    fn clone(&self) -> Self {
        if let Err(e) = self.gc_resolver.add_pin_for_state(self.state.block_id()) {
            log::error!("INTERNAL ERROR: {}", e);
        }
        Self { state: self.state.clone(), gc_resolver: self.gc_resolver.clone() }
    }
}
impl Drop for PinnedShardStateGuard {
    fn drop(&mut self) {
        if let Err(e) = self.gc_resolver.unpin_state(self.state.block_id()) {
            log::error!("INTERNAL ERROR: {}", e);
        }
    }
}

/// This structs works beetween engine and db.
/// ValidatorManager  Collator  ValidatorQuery  etc.   <- high level node commponents
///       ↓              ↓             ↓
///                 Engine
///                  ↓ ↑
///           **ShardStatesKeeper**
///                    ↓
///          InternalDb  NodeNetwork                   <- low level node components
///              ↓            ↓
///       databases  network protocols
pub struct ShardStatesKeeper {
    db: Arc<InternalDb>,
    gc_resolver: Arc<AllowStateGcSmartResolver>,
    cache_resolver: Arc<AllowStateGcSmartResolver>,
    states: lockfree::map::Map<BlockIdExt, (Arc<ShardStateStuff>, Arc<BlockHandle>)>,
    enable_persistent_gc: bool,
    stopper: Arc<Stopper>,
    max_catch_up_depth: u32,
    skip_saving_pss: bool,
    pss_cells_cache_max_count: usize,
    pss_prev_part_max_size: usize,
    states_cache_mode: ShardStatesCacheMode,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl ShardStatesKeeper {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<InternalDb>,
        enable_shard_state_persistent_gc: bool,
        skip_saving_pss: bool,
        pss_cells_cache_max_count: usize,
        pss_prev_part_max_size: usize,
        states_cache_mode: ShardStatesCacheMode,
        cells_lifetime_sec: u64,
        stopper: Arc<Stopper>,
        max_catch_up_depth: u32,
        #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
        allocated: Arc<EngineAlloc>,
    ) -> Result<Arc<Self>> {
        log::trace!("start_states_gc");
        let gc_resolver = Arc::new(AllowStateGcSmartResolver::new(cells_lifetime_sec));
        let cache_resolver = Arc::new(AllowStateGcSmartResolver::new(0));
        db.start_states_gc(gc_resolver.clone());

        Ok(Arc::new(ShardStatesKeeper {
            db,
            gc_resolver,
            cache_resolver,
            enable_persistent_gc: enable_shard_state_persistent_gc,
            states: lockfree::map::Map::new(),
            stopper,
            max_catch_up_depth,
            skip_saving_pss,
            pss_cells_cache_max_count,
            pss_prev_part_max_size,
            states_cache_mode,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        }))
    }

    pub async fn start(
        self: Arc<Self>,
        engine: Arc<Engine>,
        last_applied_mc_block: BlockIdExt,
        shard_client_mc_block: BlockIdExt,
        mut ss_keeper_block: BlockIdExt,
    ) -> Result<()> {
        log::trace!("start");

        self.restore_states(last_applied_mc_block, shard_client_mc_block.clone()).await?;

        let engine_ = engine.clone();
        let self_ = self.clone();
        tokio::spawn(async move {
            engine_.acquire_stop(Engine::MASK_SERVICE_PSS_KEEPER);
            while let Err(e) = self_.pss_worker(&engine_, &ss_keeper_block).await {
                if engine_.check_stop() {
                    break;
                }
                log::error!("pss worker: CRITICAL!!! Unexpected error: {:?}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
                ss_keeper_block = 'a: loop {
                    match engine_.load_pss_keeper_mc_block_id() {
                        Err(e) => {
                            log::error!(
                                "pss worker: CRITICAL!!! load_pss_keeper_mc_block_id: {:?}",
                                e
                            );
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Ok(None) => {
                            log::error!("CRITICAL!!! load_pss_keeper_mc_block_id returned None");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Ok(Some(id)) => break 'a (*id).clone(),
                    }
                };
            }
            engine_.release_stop(Engine::MASK_SERVICE_PSS_KEEPER);
        });

        tokio::spawn(async move {
            engine.acquire_stop(Engine::MASK_SERVICE_SS_CACHE_KEEPER);
            let mut states_cache_mc_block = shard_client_mc_block;
            while let Err(e) = self.clean_cache_worker(&engine, &states_cache_mc_block).await {
                log::error!("CRITICAL!!! Unexpected error in clean states cache worker: {:?}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
                states_cache_mc_block = 'a: loop {
                    match engine.load_shard_client_mc_block_id() {
                        Err(e) => {
                            log::error!("CRITICAL!!! load_shard_client_mc_block_id: {:?}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Ok(None) => {
                            log::error!("CRITICAL!!! load_shard_client_mc_block_id returned None");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Ok(Some(id)) => break 'a (*id).clone(),
                    }
                };
            }
            engine.release_stop(Engine::MASK_SERVICE_SS_CACHE_KEEPER);
        });

        Ok(())
    }

    pub fn allow_state_gc(&self, block_id: &BlockIdExt) -> Result<bool> {
        self.gc_resolver.allow_state_gc(block_id, 0, u64::MAX)
    }

    #[async_recursion::async_recursion]
    pub async fn load_state(
        self: &'async_recursion Arc<Self>,
        block_id: &BlockIdExt,
    ) -> Result<Arc<ShardStateStuff>> {
        log::trace!("load_state {}", block_id);
        if let Some(state) = self.states.get(block_id) {
            log::trace!("load_state {} FROM CACHE", block_id);
            return Ok(state.val().0.clone());
        } else {
            let state = match self.db.load_shard_state_dynamic(block_id) {
                Ok(s) => {
                    let handle = self
                        .db
                        .load_block_handle(block_id)?
                        .ok_or_else(|| error!("Cannot load block handle for {}", block_id))?;
                    self.states.insert(block_id.clone(), (s.clone(), handle));
                    log::trace!("load_state {} FROM DB", block_id);
                    s
                }
                Err(error) => {
                    if let Ok(error) = error.downcast::<StorageError>() {
                        if matches!(error, StorageError::StateIsAllowedToGc(_)) {
                            fail!(error)
                        }
                    }
                    let s = self.catch_up_state(block_id).await?;
                    log::trace!("load_state {} RESTORED", block_id);
                    s
                }
            };
            Ok(state)
        }
    }

    // It is prohibited to use any cell from the state after the guard's disposal.
    pub async fn load_and_pin_state(
        self: &Arc<Self>,
        block_id: &BlockIdExt,
    ) -> Result<PinnedShardStateGuard> {
        log::trace!("load_and_pin_state {}", block_id);
        let state = self.load_state(block_id).await?;
        PinnedShardStateGuard::new(state, self.gc_resolver.clone())
    }

    pub async fn store_state(
        self: &Arc<Self>,
        handle: &Arc<BlockHandle>,
        state: Arc<ShardStateStuff>,
        persistent_state: Option<&[u8]>,
        force: bool,
    ) -> Result<(Arc<ShardStateStuff>, bool)> {
        if handle.id() != state.block_id() {
            fail!("BlockIdExt and ShardStateStuff block_id mismatch");
        }

        let (cb1, cb2) = if persistent_state.is_some() || self.states_cache_mode.is_disabled() {
            let cb = SsNotificationCallback::new();
            (Some(cb.clone() as Arc<dyn storage::shardstate_db_async::Callback>), Some(cb))
        } else {
            (None, None)
        };
        let (mut state, saving) =
            self.db.store_shard_state_dynamic(handle, &state, None, cb1, force).await?;

        if let (true, Some(cb)) = (saving, cb2) {
            let now = Instant::now();
            log::debug!("store_state {}: waiting for callback...", handle.id());
            while tokio::time::timeout(Duration::from_secs(1), cb.wait()).await.is_err() {
                log::warn!(
                    "store_state {}: yet waiting for a callback: TIME {}ms",
                    handle.id(),
                    now.elapsed().as_millis()
                );
            }
            let millis = now.elapsed().as_millis();
            if millis > 100 {
                log::warn!("store_state {}: callback done TIME {}ms", millis, handle.id());
            } else {
                log::debug!("store_state {}: callback done TIME {}ms", millis, handle.id());
            }
            // reload state after saving just to free fully loaded tree
            // and use lazy loaded cells futher
            state = self.db.load_shard_state_dynamic(handle.id())?;
        }

        if let Some(state_data) = persistent_state {
            // while boot - zerostate and init persistent state are saved using this parameter
            self.db.store_shard_state_persistent_raw(handle, state_data, None).await?;
        }

        // if state was already saved (by callback) - do nothitng
        let saved =
            add_unbound_object_to_map_with_update(&self.states, handle.id().clone(), |found| {
                if found.is_some() {
                    Ok(None)
                } else {
                    Ok(Some((state.clone(), handle.clone())))
                }
            })?;

        Ok((state, saved))
    }

    // Returns root cell, cells index to reuse.
    pub async fn store_state_part(
        self: &Arc<Self>,
        id: &PersistentStatePartId,
        data: Arc<Vec<u8>>,
        cells_index: Vec<(UInt256, u16)>,
    ) -> Result<(Cell, Vec<(UInt256, u16)>)> {
        let now = std::time::Instant::now();
        log::info!("store_state_part: deserialize, length: {}, {}...", data.len(), id);

        let ssk = self.clone();
        let (state_root, fast_cell_storage) =
            tokio::task::spawn_blocking(move || -> Result<(Cell, AsyncCellsStorageAdapter)> {
                let mut fast_cell_storage = ssk.db.create_fast_cell_storage(cells_index)?;
                let r = BocReader::new()
                    .set_abort(&|| ssk.stopper.check_stop())
                    .read_inmem_to_storage(data.as_slice(), &mut fast_cell_storage)?
                    .withdraw_single_root()?;
                Ok((r, fast_cell_storage))
            })
            .await??;
        let cells_index = fast_cell_storage.finish().await?;

        log::info!("store_state_part: deserialized {} TIME {:#?}", id, now.elapsed());

        Ok((state_root, cells_index))
    }

    // Save header of split persistent state. Before calling this method,
    // all parts of the persistent state must be already saved by store_state_part.
    pub async fn merge_and_check_state(
        self: &Arc<Self>,
        handle: &Arc<BlockHandle>,
        root_hash: &UInt256,
        part_root_hashes: &[UInt256],
        header_boc: Arc<Vec<u8>>,
    ) -> Result<Arc<ShardStateStuff>> {
        if handle.has_state() && handle.has_persistent_state() {
            return Ok(self.load_state(handle.id()).await?);
        }
        log::info!(
            "check_and_store_state_header: restore head, length: {}, {}...",
            header_boc.len(),
            handle.id()
        );
        let now = std::time::Instant::now();

        // deserialize state head in memory
        let ssk = self.clone();
        let data_clone = header_boc.clone();
        let proof_root = tokio::task::spawn_blocking(move || -> Result<Cell> {
            // Do not use read_inmem here because data buffer must be released soon after this call.
            BocReader::new()
                .set_abort(&|| ssk.stopper.check_stop())
                .read(&mut Cursor::new(&data_clone[..]))?
                .withdraw_single_root()
        })
        .await??;
        let proof = MerkleProof::construct_from_cell(proof_root)?;
        let mut state: ShardStateUnsplit = proof.virtualize()?;

        // merge all parts together
        let mut accounts_dict = ShardAccounts::default();
        for part_id in part_root_hashes {
            let root = self.db.load_cell(part_id).map_err(|e| {
                error!(
                    "Failed to load persistent state part {} of state {}: {}",
                    part_id,
                    handle.id(),
                    e
                )
            })?;
            let dict = ShardAccounts::construct_from_cell(root)?;
            accounts_dict.merge(&dict, &SliceData::default())?;
        }

        // replace accounts root in the state
        state.replace_accounts_cell(accounts_dict.serialize()?);

        // check and save result
        let state_root = state.serialize()?;
        if state_root.repr_hash() != root_hash {
            fail!("Invalid state hash {:x} != {:x}", state_root.repr_hash(), root_hash);
        }
        let state_stuff = ShardStateStuff::from_state(
            handle.id().clone(),
            state,
            #[cfg(feature = "telemetry")]
            &self.telemetry,
            &self.allocated,
        )?;
        let (state_stuff, _) = self.store_state(handle, state_stuff, None, false).await?;

        log::info!(
            "check_and_store_state_header: restored {} TIME {:#?}",
            handle.id(),
            now.elapsed()
        );

        Ok(state_stuff)
    }

    pub async fn check_and_store_state(
        self: &Arc<Self>,
        handle: &Arc<BlockHandle>,
        root_hash: &UInt256,
        data: Arc<Vec<u8>>,
        cells_index: Vec<(UInt256, u16)>,
    ) -> Result<(Arc<ShardStateStuff>, Vec<(UInt256, u16)>)> {
        if handle.has_state() && handle.has_persistent_state() {
            return Ok((self.load_state(handle.id()).await?, cells_index));
        }

        let now = std::time::Instant::now();
        log::info!(
            "check_and_store_state: deserialize, length: {}, {}...",
            data.len(),
            handle.id()
        );

        let ssk = self.clone();
        let data_ = data.clone();
        let (state_root, fast_cell_storage) =
            tokio::task::spawn_blocking(move || -> Result<(Cell, AsyncCellsStorageAdapter)> {
                let mut fast_cell_storage = ssk.db.create_fast_cell_storage(cells_index)?;
                let r = BocReader::new()
                    .set_abort(&|| ssk.stopper.check_stop())
                    .read_inmem_to_storage(data_.as_slice(), &mut fast_cell_storage)?
                    .withdraw_single_root()?;
                Ok((r, fast_cell_storage))
            })
            .await??;
        let cells_index = fast_cell_storage.finish().await?;

        if state_root.repr_hash() != root_hash {
            fail!("Invalid state hash {:x} != {:x}", state_root.repr_hash(), root_hash);
        }

        log::info!("check_and_store_state: deserialized {} TIME {:#?}", handle.id(), now.elapsed());

        let state = ShardStateStuff::from_root_cell(
            handle.id().clone(),
            state_root,
            #[cfg(feature = "telemetry")]
            &self.telemetry,
            &self.allocated,
        )?;

        let (state, _) = self.store_state(handle, state, Some(&data), false).await?;

        Ok((state, cells_index))
    }

    fn check_stop(&self) -> Result<()> {
        if self.stopper.check_stop() {
            fail!("Stopped")
        } else {
            Ok(())
        }
    }

    async fn restore_states(
        self: &Arc<Self>,
        last_applied_mc_block: BlockIdExt,
        shard_client_mc_block: BlockIdExt,
    ) -> Result<()> {
        log::trace!(
            "restore_states  last_applied_mc_block: {}  shard_client_mc_block: {}...",
            last_applied_mc_block,
            shard_client_mc_block
        );

        let _ = self.load_state(&last_applied_mc_block).await?;

        let mc_state = self.load_state(&shard_client_mc_block).await?;
        let shard_blocks = mc_state.shard_hashes()?.top_blocks_all()?;
        for block_id in &shard_blocks {
            self.load_state(block_id).await?;
        }

        Ok(())
    }

    const MAX_NEW_STATE_OFFSET: u32 = 50;
    async fn catch_up_state(self: &Arc<Self>, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        log::trace!("catch_up_state {}...", id);
        let now = std::time::Instant::now();

        // load latest block we know
        let latest_mc_id = self
            .db
            .load_full_node_state(LAST_APPLIED_MC_BLOCK)?
            .ok_or_else(|| error!("Can't load LAST_APPLIED_MC_BLOCK in catch_up_state"))?;
        let latest_id = if id.shard().is_masterchain() {
            (*latest_mc_id).clone()
        } else {
            // load latest known block in needed shard
            let latest_mc_state = self.load_state(&latest_mc_id).await?;
            let mut latest_id = None;
            latest_mc_state.shards()?.iterate_shards(|ident, descr| {
                if ident.intersect_with(id.shard()) {
                    latest_id = Some(BlockIdExt {
                        shard_id: ident,
                        seq_no: descr.seq_no,
                        root_hash: descr.root_hash,
                        file_hash: descr.file_hash,
                    });
                }
                Ok(true)
            })?;
            match latest_id {
                None => fail!("Can't find latest known shard for {}", id),
                Some(id) => id,
            }
        };

        // check the state is not too new
        if id.seq_no() > latest_id.seq_no() + Self::MAX_NEW_STATE_OFFSET {
            fail!("Attempt to load too new state {}, latest known block {}", id, latest_id);
        }

        let state = self.restore_state_recursive(id).await?;

        log::trace!("catch_up_state {} CATCHED UP - TIME {}ms", id, now.elapsed().as_millis());
        Ok(state)
    }

    async fn restore_state_recursive(
        self: &Arc<Self>,
        id: &BlockIdExt,
    ) -> Result<Arc<ShardStateStuff>> {
        let try_get_state = |handle: &Arc<BlockHandle>| {
            if let Some(state) = self.states.get(handle.id()) {
                log::trace!("load_state {} FROM CACHE", handle.id());
                Some(state.val().0.clone())
            } else if handle.has_saved_state() {
                if let Ok(state) = self.db.load_shard_state_dynamic(handle.id()) {
                    self.states.insert(handle.id().clone(), (state.clone(), handle.clone()));
                    Some(state)
                } else {
                    log::warn!(
                        "Can't load state for {} from DB, but handle.has_saved_state() == true",
                        handle.id()
                    );
                    None
                }
            } else {
                log::trace!("there is no saved state for {}", handle.id());
                None
            }
        };

        log::trace!("restore_state_recursive {}...", id);

        let handle = self
            .db
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for {}", id))?;
        if let Some(state) = try_get_state(&handle) {
            return Ok(state);
        }

        let top_id = id.clone();
        let mut stack = vec![handle];
        loop {
            self.check_stop()?;

            let handle = stack
                .last()
                .ok_or_else(|| error!("INTERNAL ERROR: restore_state_recursive: stask is empty"))?;
            log::trace!(
                "restore_state_recursive: stack size: {}, handle: {}",
                stack.len(),
                handle.id()
            );

            if stack.len() as u32 >= self.max_catch_up_depth {
                fail!("restore_state_recursive: max depth achived on id {}", handle.id());
            }

            let block = self.db.load_block_data(handle).await?;
            let prev_root = match block.construct_prev_id()? {
                (prev, None) => {
                    let handle = self
                        .db
                        .load_block_handle(&prev)?
                        .ok_or_else(|| error!("Cannot load handle for {}", prev))?;
                    if let Some(pr) = try_get_state(&handle) {
                        pr.root_cell().clone()
                    } else {
                        stack.push(handle);
                        continue;
                    }
                }
                (prev1, Some(prev2)) => {
                    let handle1 = self
                        .db
                        .load_block_handle(&prev1)?
                        .ok_or_else(|| error!("Cannot load handle for {}", prev1))?;
                    let root1 = if let Some(pr) = try_get_state(&handle1) {
                        pr.root_cell().clone()
                    } else {
                        stack.push(handle1);
                        continue;
                    };

                    let handle2 = self
                        .db
                        .load_block_handle(&prev2)?
                        .ok_or_else(|| error!("Cannot load handle for {}", prev2))?;
                    let root2 = if let Some(pr) = try_get_state(&handle2) {
                        pr.root_cell().clone()
                    } else {
                        stack.push(handle2);
                        continue;
                    };
                    ShardStateStuff::construct_split_root(root1, root2)?
                }
            };

            let merkle_update = block.block()?.read_state_update()?;
            let id = block.id().clone();
            let keeper = self.clone();
            let cf = self.db.cells_factory()?;
            let state = tokio::task::spawn_blocking(move || -> Result<Arc<ShardStateStuff>> {
                let now = std::time::Instant::now();
                let (root, _) = merkle_update.apply_with_factory(&prev_root, &cf)?;
                log::trace!(
                    "TIME: restore_state_recursive: applied Merkle update {}ms   {}",
                    now.elapsed().as_millis(),
                    id
                );
                ShardStateStuff::from_root_cell(
                    id,
                    root,
                    #[cfg(feature = "telemetry")]
                    &keeper.telemetry,
                    &keeper.allocated,
                )
            })
            .await??;

            self.store_state(handle, state.clone(), None, true).await?;

            stack.pop().ok_or_else(|| {
                error!("INTERNAL ERROR: restore_state_recursive: stask is empty when pop()")
            })?;

            if stack.is_empty() {
                if *state.block_id() != top_id {
                    fail!(
                        "INTERNAL ERROR: restore_state_recursive: found state is wrong {} != {}",
                        state.block_id(),
                        top_id
                    );
                }
                return Ok(state);
            }
        }
    }

    async fn clean_cache_worker(&self, engine: &Arc<Engine>, id: &BlockIdExt) -> Result<()> {
        let mut handle = engine
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for states cache cleaner block {}", id))?;
        let mut min_id = 0;
        loop {
            if engine.check_stop() {
                return Ok(());
            }

            // update gc resolver
            let new_min_id = Self::get_min_processed_mc_id(engine)?;
            if min_id < new_min_id.seq_no() {
                min_id = new_min_id.seq_no();
                let advanced = self.cache_resolver.advance(&new_min_id, engine.deref()).await?;
                if advanced {
                    // clear cache
                    let mut total = 0;
                    let mut cleaned = 0;
                    let now = std::time::Instant::now();
                    for guard in &self.states {
                        total += 1;
                        if self.cache_resolver.allow_state_gc(&guard.0, 0, 0)?
                            && guard.val().1.has_saved_state()
                        {
                            self.states.remove(&guard.0);
                            cleaned += 1;
                        }
                    }
                    log::debug!(
                        "clean_cache_worker: TIME {time}ms, cleaned: {cleaned}, total: {total}",
                        time = now.elapsed().as_millis(),
                    );
                }
            }

            // wait next mc block
            handle = loop {
                if let Ok(h) = engine.wait_next_applied_mc_block(&handle, Some(500)).await {
                    break h.0;
                } else if engine.check_stop() {
                    return Ok(());
                }
            };
        }
    }

    async fn pss_worker(&self, engine: &Arc<Engine>, ss_keeper_block: &BlockIdExt) -> Result<()> {
        if !ss_keeper_block.shard().is_masterchain() {
            fail!("'ss_keeper_block' mast belong master chain");
        }
        let mut handle = engine
            .load_block_handle(ss_keeper_block)?
            .ok_or_else(|| error!("Cannot load handle for ss keeper block {}", ss_keeper_block))?;
        loop {
            let mc_state = engine.load_state(handle.id()).await?;
            let mut is_persistent_state = false;
            if handle.id().seq_no() != 0 && handle.is_key_block()? {
                if let Some(prev_key_block_id) = mc_state
                    .shard_state_extra()?
                    .prev_blocks
                    .get_prev_key_block(handle.id().seq_no() - 1)?
                {
                    let block_id = BlockIdExt {
                        shard_id: ShardIdent::masterchain(),
                        seq_no: prev_key_block_id.seq_no,
                        root_hash: prev_key_block_id.root_hash,
                        file_hash: prev_key_block_id.file_hash,
                    };
                    let prev_handle = engine.load_block_handle(&block_id)?.ok_or_else(|| {
                        error!("Cannot load handle for ss keeper prev key block {}", block_id)
                    })?;
                    is_persistent_state = engine.is_persistent_state(
                        handle.gen_utime(),
                        prev_handle.gen_utime(),
                        boot::PSS_PERIOD_BITS,
                    );
                }
            }

            if is_persistent_state {
                if self.skip_saving_pss {
                    log::trace!("pss worker: state is skipped due to config {}", handle.id());
                } else {
                    // store states
                    let storer = PssStorer::new(
                        self.db.clone(),
                        engine.clone(),
                        handle.clone(),
                        self.pss_cells_cache_max_count,
                        self.pss_prev_part_max_size,
                    )
                    .await?;
                    storer.store().await?;
                    // gc iteration for persistent/stored states

                    if engine.check_stop() {
                        return Ok(());
                    }

                    if self.enable_persistent_gc {
                        let calc_ttl = |t| {
                            let ttl = engine.persistent_state_ttl(t, boot::PSS_PERIOD_BITS);
                            let expired = ttl <= engine.now();
                            (ttl, expired)
                        };
                        let zerostate_id = engine.zerostate_id();
                        if let Err(e) =
                            self.db.shard_state_persistent_gc(calc_ttl, zerostate_id).await
                        {
                            log::warn!("pss worker: gc: {}", e);
                        }
                    }

                    if engine.check_stop() {
                        return Ok(());
                    }
                }
            }

            // update gc resolver
            let min_id = Self::get_min_processed_mc_id(engine)?;
            if min_id.seq_no() > handle.id().seq_no() {
                self.gc_resolver.advance(handle.id(), engine.deref()).await?;
            } else {
                self.gc_resolver.advance(&min_id, engine.deref()).await?;
            }

            // wait next mc block
            handle = loop {
                if let Ok(h) = engine.wait_next_applied_mc_block(&handle, Some(500)).await {
                    break h.0;
                } else if engine.check_stop() {
                    return Ok(());
                }
            };
            engine.save_pss_keeper_mc_block_id(handle.id())?;
        }
    }

    fn get_min_processed_mc_id(engine: &Engine) -> Result<Arc<BlockIdExt>> {
        let mut min_id = engine.load_shard_client_mc_block_id()?.ok_or_else(|| {
            error!("INTERNAL ERROR: No shard client MC block id in ss keeper worker")
        })?;

        let last_rotation_block_id = engine.load_last_rotation_block_id()?;
        if let Some(id) = last_rotation_block_id {
            if min_id.seq_no() > id.seq_no() {
                min_id = id
            }
        }

        let archives_gc = engine
            .load_archives_gc_mc_block_id()?
            .ok_or_else(|| error!("INTERNAL ERROR: No archives GC block id in ss keeper worker"))?;
        if archives_gc.seq_no() < min_id.seq_no() {
            min_id = archives_gc;
        }

        Ok(min_id.clone())
    }
}

pub fn calc_pss_split_parts(
    shard: ShardIdent,
    split_depth: u8,
    parts: &mut Vec<ShardIdent>,
) -> Result<()> {
    if shard.prefix_len() >= split_depth {
        parts.push(shard);
    } else {
        let (s1, s2) = shard.split()?;
        calc_pss_split_parts(s1, split_depth, parts)?;
        calc_pss_split_parts(s2, split_depth, parts)?;
    }
    Ok(())
}

pub async fn find_prev_pss(
    db: &InternalDb,
    mut handle: Arc<BlockHandle>,
    prev_blocks: &OldMcBlocksInfo,
) -> Result<Option<(Arc<BlockHandle>, Vec<BlockIdExt>)>> {
    let mut iter = 100;
    loop {
        let Some(prev_key_block_id) = prev_blocks
            .get_prev_key_block(handle.id().seq_no() - 1)?
            .map(|id| id.master_block_id().1)
        else {
            return Ok(None);
        };

        log::trace!("find_prev_pss: prev key block is {}", prev_key_block_id);

        let Some(prev_handle) = db.load_block_handle(&prev_key_block_id)? else {
            log::warn!(
                "Cannot load handle for prev key block {} when searching prev pss",
                prev_key_block_id
            );
            return Ok(None);
        };
        if prev_handle.has_persistent_state() {
            let block = db.load_block_data(&prev_handle).await?;
            let top_blocks = block.top_blocks(0)?;
            let mut all_have_persistent = true;
            for prev_id in &top_blocks {
                let Some(prev_handle) = db.load_block_handle(&prev_id)? else {
                    log::warn!(
                        "Cannot load handle for prev block {} when searching prev pss",
                        prev_id
                    );
                    return Ok(None);
                };
                if !prev_handle.has_persistent_state() {
                    log::trace!(
                        "find_prev_pss: prev block {} has no persistent state, continue searching",
                        prev_id
                    );
                    all_have_persistent = false;
                    break;
                }
            }
            if all_have_persistent {
                return Ok(Some((prev_handle, top_blocks)));
            }
        }
        log::trace!(
            "find_prev_pss: prev key block {} has no persistent state, continue searching",
            prev_key_block_id
        );
        handle = prev_handle;
        iter -= 1;
        if iter == 0 {
            log::warn!(
                "Too many iterations while searching for prev PSS block, something is wrong"
            );
            return Ok(None);
        }
    }
}

// This cache is designed to work in non-concurrent (single thread) environment.
// It uses spin::Mutex for synchronization and assumes the mutex is not locked any other thread.
struct CellsCache {
    // Cache is split into 256 maps to use smaller hash maps.
    // Hash maps reallocations (when map grows) needs x2 memory any time.
    // The first byte of the hash is used as a map index.
    cache: spin::Mutex<Vec<ahash::HashMap<[u8; 32 - 1], Cell>>>,
    arena: spin::Mutex<Arc<CellsArena>>,
    db_loader: Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>,
    cache_loader: OnceLock<Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>>,
    loaded_from_db: AtomicU64,
    cache_size: AtomicUsize,
    cells_max_count: usize,
}
impl CellsCache {
    const ARENA_CHUNK_SIZE: usize = 16 * 1024 * 1024; // 4 MB
    const ARENA_BYTES_PER_CELL: usize = 2048; // conservative estimate

    pub fn new(
        load_cell_callback: Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>,
        cells_max_count: usize,
    ) -> Arc<Self> {
        let mut cache = Vec::new();
        for _ in 0..256 {
            cache.push(ahash::HashMap::default());
        }
        let cache = Arc::new(Self {
            cache: spin::Mutex::new(cache),
            arena: spin::Mutex::new(Self::create_arena(cells_max_count)),
            db_loader: load_cell_callback,
            cache_loader: OnceLock::new(),
            loaded_from_db: AtomicU64::new(0),
            cache_size: AtomicUsize::new(0),
            cells_max_count,
        });
        let cache_weak = Arc::downgrade(&cache);
        let _ = cache.cache_loader.set(Arc::new(move |hash| {
            let cache =
                cache_weak.upgrade().ok_or_else(|| error!("CellsCache has been dropped"))?;
            cache.get(hash)
        }));
        cache
    }
    pub fn arena(&self) -> Arc<CellsArena> {
        self.arena.lock().clone()
    }

    fn create_arena(cells_max_count: usize) -> Arc<CellsArena> {
        Arc::new(CellsArena::new(
            Self::ARENA_CHUNK_SIZE,
            cells_max_count * Self::ARENA_BYTES_PER_CELL,
        ))
    }

    pub fn get(self: &Arc<Self>, hash: &UInt256) -> Result<Cell> {
        let arena = self.arena.lock().clone();
        let part = hash.as_slice()[0] as usize;
        let key = hash.as_slice()[1..].try_into().unwrap();
        match self.cache.lock()[part].entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => Ok(e.get().clone()),
            std::collections::hash_map::Entry::Vacant(e) => {
                if let Ok(storage_cell) = (self.db_loader)(hash) {
                    let loader = self.cache_loader.get().unwrap();
                    let cached_cell =
                        Cell::with_cell_and_loader(storage_cell, loader, Some(arena))?;
                    if self.cache_size() < self.cells_max_count {
                        e.insert(cached_cell.clone());
                        self.cache_size.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    self.loaded_from_db.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(cached_cell)
                } else {
                    fail!("Failed to load cell with hash {:x}", hash);
                }
            }
        }
    }
    pub fn insert(&self, cell: Cell, ignore_limit: bool) -> bool {
        if ignore_limit || self.cache_size() < self.cells_max_count {
            let hash = cell.repr_hash();
            let part = hash.as_slice()[0] as usize;
            let key = hash.as_slice()[1..].try_into().unwrap();
            self.cache_size.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.cache.lock()[part].insert(key, cell);
            true
        } else {
            false
        }
    }
    pub fn loaded_from_db(&self) -> u64 {
        self.loaded_from_db.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn cache_size(&self) -> usize {
        self.cache_size.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn clear(&self) {
        for m in self.cache.lock().iter_mut() {
            m.clear();
        }
        let mut arena = self.arena.lock();
        let strong = Arc::strong_count(&arena);
        if strong > 1 {
            log::warn!("pss worker: CellsCache: arena strong count is {}!!!", strong);
        }
        *arena = Self::create_arena(self.cells_max_count);
        self.loaded_from_db.store(0, std::sync::atomic::Ordering::Relaxed);
        self.cache_size.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

struct PssStorerPrevStuff {
    mc_id: BlockIdExt,
    top_blocks: Vec<BlockIdExt>,
    split_depth: u8,
    cells_cache: Arc<CellsCache>,
    cached_states: HashSet<PersistentStatePartId>,
    prev_part_max_size: usize,
}

impl PssStorerPrevStuff {
    fn needed_parts(
        &self,
        part_id: &PersistentStatePartId,
    ) -> Result<HashSet<PersistentStatePartId>> {
        let mut result = HashSet::new();

        // Masterchain is always saved as WholeState
        if part_id.block_id().shard().is_masterchain() {
            result.insert(PersistentStatePartId::WholeState(self.mc_id.clone()));
            return Ok(result);
        }

        let workchain = part_id.block_id().shard().workchain_id();
        let curr_part_shard = match part_id {
            PersistentStatePartId::WholeState(id) => id.shard().clone(),
            PersistentStatePartId::Part(_, prefix) => {
                ShardIdent::with_tagged_prefix(workchain, *prefix)?
            }
            PersistentStatePartId::Head(_) => {
                fail!("INTERNAL ERROR: Head part should not be used in needed_parts()")
            }
        };

        for prev_block in &self.top_blocks {
            if prev_block.shard().workchain_id() != workchain {
                continue;
            }
            if !curr_part_shard.intersect_with(prev_block.shard()) {
                continue;
            }
            if prev_block.shard().prefix_len() >= self.split_depth {
                // Prev shard is not split — stored as WholeState
                result.insert(PersistentStatePartId::WholeState(prev_block.clone()));
            } else {
                // Prev shard is split — enumerate its parts and keep those that intersect
                let mut prev_part_shards = Vec::new();
                calc_pss_split_parts(
                    prev_block.shard().clone(),
                    self.split_depth,
                    &mut prev_part_shards,
                )?;
                for prev_part_shard in prev_part_shards {
                    if curr_part_shard.intersect_with(&prev_part_shard) {
                        result.insert(PersistentStatePartId::Part(
                            prev_block.clone(),
                            prev_part_shard.shard_prefix_with_tag(),
                        ));
                    }
                }
            }
        }
        Ok(result)
    }
}

pub struct PssStorer {
    db: Arc<InternalDb>,
    engine: Arc<dyn EngineOperations>,
    mc_handle: Arc<BlockHandle>,
    top_blocks: Vec<BlockIdExt>,
    split_depth: u8,
    prev_stuff: Option<PssStorerPrevStuff>,
    abort: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl PssStorer {
    async fn new(
        db: Arc<InternalDb>,
        engine: Arc<dyn EngineOperations>,
        handle: Arc<BlockHandle>,
        calls_max_count: usize,
        prev_part_max_size: usize,
    ) -> Result<Self> {
        let mc_state = engine.load_state(handle.id()).await?;
        let split_depth = mc_state.config_params()?.base_workchain()?.persistent_state_split_depth;
        let prev_blocks_dict = &mc_state.shard_state_extra()?.prev_blocks;
        let prev_stuff = if let Some((prev_handle, tops)) =
            find_prev_pss(&db, handle.clone(), prev_blocks_dict).await?
        {
            let prev_block = db.load_block_data(&prev_handle).await?;
            let prev_split_depth =
                prev_block.read_config_params()?.base_workchain()?.persistent_state_split_depth;
            let db_ = db.clone();
            let load_cell_callback = Arc::new(move |hash: &UInt256| db_.load_cell(hash));
            let cells_cache = CellsCache::new(load_cell_callback, calls_max_count);
            Some(PssStorerPrevStuff {
                mc_id: prev_handle.id().clone(),
                top_blocks: tops,
                split_depth: prev_split_depth,
                cells_cache,
                cached_states: HashSet::new(),
                prev_part_max_size,
            })
        } else {
            None
        };
        let top_blocks = mc_state.shard_hashes()?.top_blocks(&[BASE_WORKCHAIN_ID])?;
        let engine_ = engine.clone();
        let abort = Arc::new(move || engine_.check_stop()) as Arc<dyn Fn() -> bool + Send + Sync>;
        Ok(Self { db, engine, mc_handle: handle, top_blocks, split_depth, prev_stuff, abort })
    }

    pub async fn store(mut self) -> Result<()> {
        let started_at = Instant::now();
        log::debug!("pss worker: saving {}", self.mc_handle.id());

        let mc_handle = self.mc_handle.clone();
        self.store_one_shard_attempts(mc_handle).await?;

        let top_blocks = self.top_blocks.clone();
        for block_id in top_blocks {
            log::debug!("pss worker: saving {}", block_id);
            let handle = 'a: loop {
                match self.engine.wait_applied_block(&block_id, Some(1000)).await {
                    Ok(h) => break 'a h,
                    Err(e) => {
                        if self.engine.check_stop() {
                            return Ok(());
                        }
                        log::debug!(
                            "pss worker: haven't got shard block handle {} yet: {:?}",
                            block_id,
                            e
                        );
                    }
                }
            };
            self.store_one_shard_attempts(handle).await?;
        }

        let total_time = started_at.elapsed();
        metrics::histogram!("ton_node_db_persistent_state_write_seconds").record(total_time);
        log::info!(
            "pss worker: saved mc state {} and all related shards, TIME: {:#?}",
            self.mc_handle.id().seq_no(),
            total_time
        );
        Ok(())
    }

    async fn store_one_shard_attempts(&mut self, handle: Arc<BlockHandle>) -> Result<()> {
        if handle.has_persistent_state() {
            log::debug!("pss worker: state for {} is already stored, skipping", handle.id());
            return Ok(());
        }
        let mut attempts = 1;
        while let Err(e) = self.store_one_shard(&handle).await {
            if self.engine.check_stop() {
                fail!("pss worker: stopped while saving {}", handle.id());
            }
            log::error!(
                "pss worker: CRITICAL Error saving for {} (attempt: {}): {:?}",
                handle.id(),
                attempts,
                e
            );
            attempts += 1;
            futures_timer::Delay::new(Duration::from_millis(5000)).await;
        }
        Ok(())
    }

    async fn store_one_shard(&mut self, handle: &Arc<BlockHandle>) -> Result<()> {
        let id = handle.id();
        let ss = self.engine.load_state(id).await?;
        // masterchain is always saved as a whole regardless of split_depth
        let split_depth = if id.shard().is_masterchain() { 0 } else { self.split_depth };

        if id.shard().prefix_len() >= split_depth {
            self.store_one_part(
                handle,
                &PersistentStatePartId::WholeState(id.clone()),
                ss.root_cell().clone(),
            )
            .await?;
        } else {
            // Calc needed parts
            let mut part_prefixes = Vec::new();
            calc_pss_split_parts(id.shard().clone(), split_depth, &mut part_prefixes)?;
            if part_prefixes.len() <= 1 {
                fail!("INTERNAL ERROR: no parts for persistent state {}", id);
            }

            let mut state = ss.state()?.clone();
            let accounts_dict_nonused = state.read_accounts()?;
            let accounts_root = state.accounts_cell();
            let usage_tree = UsageTree::with_root(accounts_root.clone());

            // Collect parts
            let accounts_dict = ShardAccounts::construct_from_cell(usage_tree.root_cell())?;
            let mut parts = vec![];
            for part_prefix in &part_prefixes {
                // Parts mustnot be a usage cells because later it is checked by type.
                parts.push((
                    part_prefix.shard_prefix_with_tag(),
                    accounts_dict_nonused
                        .subtree_with_prefix(&part_prefix.shard_key(false), &mut 0)?
                        .write_to_new_cell()?
                        .into_cell()?,
                ));
                // Make same to mark cells as usage
                let _subtree =
                    accounts_dict.subtree_with_prefix(&part_prefix.shard_key(false), &mut 0)?;
            }

            // Build header
            let accounts_dict_proof = MerkleProof::create_raw(
                &accounts_root,
                &|h| usage_tree.contains(h),
                &|_| false,
                0,
                &mut None,
                &mut ahash::AHashMap::new(),
            )?;
            state.replace_accounts_cell(accounts_dict_proof);
            let header = state.serialize()?;

            // Store all parts
            for (part_prefix, part_root) in parts {
                let part_id = PersistentStatePartId::Part(id.clone(), part_prefix);
                self.store_one_part(handle, &part_id, part_root).await?;
            }

            // Store the header
            self.store_one_part(handle, &PersistentStatePartId::Head(id.clone()), header).await?;
        }
        Ok(())
    }

    async fn store_one_part(
        &mut self,
        handle: &Arc<BlockHandle>,
        part_id: &PersistentStatePartId,
        root: Cell,
    ) -> Result<()> {
        if !part_id.is_head() {
            if let Some(prev_stuff) = &mut self.prev_stuff {
                if let Err(e) = Self::store_one_part_fast(
                    prev_stuff,
                    &self.db,
                    handle,
                    part_id,
                    root.clone(),
                    self.abort.clone(),
                )
                .await
                {
                    log::warn!(
                        "pss worker: fast PSS saving failed for {}, falling back to slow: {}",
                        part_id,
                        e
                    );
                    if self.engine.check_stop() {
                        fail!("pss worker: stopped while saving {}", handle.id());
                    }
                } else {
                    return Ok(());
                }
            }
        }
        self.db
            .store_shard_state_persistent_part(handle, part_id, root, None, self.abort.clone())
            .await
    }

    async fn store_one_part_fast(
        prev_stuff: &mut PssStorerPrevStuff,
        db: &Arc<InternalDb>,
        handle: &Arc<BlockHandle>,
        part_id: &PersistentStatePartId,
        root: Cell,
        abort: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<()> {
        log::info!("pss worker: starting fast attempt for {}", part_id);
        let started_at = Instant::now();

        // 1. Determine which prev PSS parts are needed
        let needed = prev_stuff.needed_parts(part_id)?;

        // 2. Reload cache if the required set changed
        if needed != prev_stuff.cached_states {
            log::debug!(
                "pss worker: reloading cache for {}, needed: [{}]",
                part_id,
                needed.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
            );
            prev_stuff.cells_cache.clear();
            prev_stuff.cached_states.clear();
            for prev_part_id in &needed {
                let size = db.load_shard_state_persistent_size(prev_part_id).await? as usize;
                if size > prev_stuff.prev_part_max_size {
                    fail!(
                        "Prev part {} is too big for fast saving ({} bytes), max allowed is {}",
                        prev_part_id,
                        size,
                        prev_stuff.prev_part_max_size
                    );
                }

                let now = Instant::now();
                let mut read_obj = db.load_shard_state_persistent_obj(prev_part_id)?;

                let cells_cache = prev_stuff.cells_cache.clone();
                let arena = cells_cache.arena();
                let result = BocReader::new()
                    .set_abort(abort.deref())
                    .set_arena(arena)
                    .set_load_cell_callback(&|cell| cells_cache.insert(cell.clone(), false))
                    .read(&mut read_obj)?;
                let load_time = now.elapsed();
                log::debug!("pss worker: loaded prev {} in {:#?}", prev_part_id, load_time,);
                prev_stuff.cached_states.insert(prev_part_id.clone());
                if result.interrupted {
                    log::warn!(
                        "pss worker: cache size limit reached! Continue saving without additional caching.",
                    );
                    break;
                }
            }
        } else {
            log::debug!(
                "pss worker: reusing cache for {}, cache size: {}",
                part_id,
                prev_stuff.cells_cache.cache_size()
            );
        }

        let cells_cache = prev_stuff.cells_cache.clone();

        // 3. Add new (non-StoredCell) cells to cache, wrapping them as CachedCell
        let mut stack = vec![root.clone()];
        let mut max_inmemory_cells = 100;
        let loader = cells_cache.cache_loader.get().unwrap();
        let arena = cells_cache.arena();
        while let Some(cell) = stack.pop() {
            // Newly constructed cells has loaded refs, so they don't have refs loader
            if cell.loader_data_ptr().is_none() {
                let cached_cell =
                    Cell::with_cell_and_loader(cell.clone(), loader, Some(arena.clone()))?;

                if cells_cache.insert(cached_cell, true) {
                    for r in cell.clone_references()? {
                        stack.push(r);
                    }
                }
            }
            max_inmemory_cells -= 1;
            if max_inmemory_cells == 0 {
                fail!("Too many cells to store in memory for fast saving");
            }
        }

        // 4. Wrap root as CachedCell and write with BocWriter
        let cached_root = Cell::with_cell_and_loader(root, loader, Some(arena))?;

        db.store_shard_state_persistent_part_fast(handle, part_id, cached_root, abort).await?;

        log::info!(
            "pss worker: fast saving done {} in {:#?}; cache size: {}, loaded from DB: {}",
            part_id,
            started_at.elapsed(),
            prev_stuff.cells_cache.cache_size(),
            prev_stuff.cells_cache.loaded_from_db()
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/test_shard_states_keeper.rs"]
mod tests;
