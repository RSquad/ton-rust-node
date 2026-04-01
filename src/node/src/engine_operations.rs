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
    block::BlockStuff,
    block_proof::BlockProofStuff,
    config::{CollatorConfig, CollatorTestBundlesGeneralConfig},
    engine::{Engine, EngineFlags, SplitQueues},
    engine_traits::{
        EngineAlloc, EngineOperations, PrivateOverlayOperations, ValidatorKeyBinding,
        ValidatorListOutcome,
    },
    error::NodeError,
    ext_messages::{create_ext_message, EXT_MESSAGES_TRACE_TARGET},
    full_node::shard_client::{process_block_broadcast, process_block_broadcast_v2},
    internal_db::{
        BlockResult, DESTROYED_VALIDATOR_SESSIONS, INITIAL_MC_BLOCK, LAST_APPLIED_MC_BLOCK,
        LAST_ROTATION_MC_BLOCK, SHARD_CLIENT_MC_BLOCK,
    },
    shard_state::ShardStateStuff,
    shard_states_keeper::PinnedShardStateGuard,
    types::top_block_descr::{TopBlockDescrId, TopBlockDescrStuff},
    validator::{accept_block::create_new_proof_link, validator_manager::ValidationStatus},
};
#[cfg(feature = "telemetry")]
use crate::{
    engine_traits::EngineTelemetry, full_node::telemetry::FullNodeTelemetry,
    network::telemetry::FullNodeNetworkTelemetry, validator::telemetry::CollatorValidatorTelemetry,
};
use adnl::PrivateOverlayShortId;
use catchain::{
    CatchainNode, CatchainOverlay, CatchainOverlayListenerPtr, CatchainOverlayLogReplayListenerPtr,
    PrivateKey,
};
use std::{collections::HashSet, ops::Deref, sync::Arc};
use storage::{block_handle_db::BlockHandle, types::PersistentStatePartId};
use ton_api::ton::{
    engine::validator::customoverlay::CustomOverlay,
    ton_node::broadcast::{BlockBroadcast, ExternalMessageBroadcast, NewShardBlockBroadcast},
};
use ton_block::{
    error, fail, AccountIdPrefixFull, BlockIdExt, BlockSignaturesVariant, Cell, CellsFactory,
    ConfigParams, CryptoSignaturePair, KeyId, Message, OutMsgQueue, Result, ShardIdent, UInt256,
};
use validator_session::{BlockHash, SessionId, ValidatorBlockCandidate};

fn serialize_destroyed_session_ids(ids: &HashSet<UInt256>) -> Vec<u8> {
    let mut sorted_ids = ids.iter().cloned().collect::<Vec<_>>();
    sorted_ids.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));

    let mut data = Vec::with_capacity(4 + sorted_ids.len() * 32);
    data.extend_from_slice(&(sorted_ids.len() as u32).to_le_bytes());
    for id in sorted_ids {
        data.extend_from_slice(id.as_slice());
    }
    data
}

fn deserialize_destroyed_session_ids(data: &[u8]) -> Result<Vec<UInt256>> {
    if data.len() < 4 {
        fail!("Destroyed-session payload is too short: {}", data.len());
    }

    let count = u32::from_le_bytes(data[..4].try_into()?) as usize;
    let expected_len = 4 + count * 32;
    if data.len() != expected_len {
        fail!(
            "Destroyed-session payload has invalid length: expected {}, got {}",
            expected_len,
            data.len()
        );
    }

    let mut ids = Vec::with_capacity(count);
    for chunk in data[4..].chunks_exact(32) {
        ids.push(UInt256::from_slice(chunk));
    }
    Ok(ids)
}

#[async_trait::async_trait]
impl EngineOperations for Engine {
    // Global node's state

    fn set_sync_status(&self, status: u32) {
        self.set_sync_status(status);
    }

    fn get_sync_status(&self) -> u32 {
        self.get_sync_status()
    }

    fn need_monitor(&self, shard: &ShardIdent) -> Result<bool> {
        self.need_monitor(shard)
    }

    async fn check_sync(&self) -> Result<bool> {
        Engine::check_sync(self).await
    }

    fn set_will_validate(&self, will_validate: bool) {
        Engine::set_will_validate(self, will_validate);
    }

    fn is_validator(&self) -> bool {
        self.will_validate()
    }

    fn get_monitor_min_split(&self) -> u8 {
        self.get_monitor_min_split()
    }

    // Validator specific operations

    fn get_validator_status(&self) -> bool {
        self.network().config_handler().get_validator_status()
    }

    fn validator_network(&self) -> Arc<dyn PrivateOverlayOperations> {
        Engine::validator_network(self)
    }

    /// Register the local node's participation in a validator list and update network overlays.
    ///
    /// Delegates to [`PrivateOverlayOperations::set_validator_list`] for key matching and
    /// ADNL setup, then refreshes private and custom overlays **only** when the network
    /// layer is fully ready (`network_ready == true`). Overlay updates require the ADNL key
    /// to be loaded into the ADNL stack first, which is why they happen here rather than
    /// at the call site.
    async fn set_validator_list(
        &self,
        validator_list_id: UInt256,
        validators: &[CatchainNode],
    ) -> Result<ValidatorListOutcome> {
        let outcome =
            self.validator_network().set_validator_list(validator_list_id, validators).await?;

        if matches!(&outcome, ValidatorListOutcome::Selected { network_ready: true, .. }) {
            let state = self.load_last_applied_mc_state().await?;
            let config = state.config_params()?;
            self.overlays_router()?.update_private_overlays(config).await?;
            self.overlays_router()?.update_custom_overlays(None).await?;
        }
        Ok(outcome)
    }

    fn activate_validator_list(&self, validator_list_id: UInt256) -> Result<()> {
        self.network().activate_validator_list(validator_list_id)
    }

    // fn calc_overlay_id(&self, workchain: i32, shard: u64) -> Result<(Arc<adnl::OverlayShortId>, adnl::OverlayId)> {
    //     self.calc_overlay_id(workchain, shard)
    // }

    fn validation_status(&self) -> ValidationStatus {
        self.validation_status()
    }

    fn get_validator_key_bindings(&self) -> Result<Vec<ValidatorKeyBinding>> {
        let keys = self.network().config_handler().get_actual_validator_keys()?;
        Ok(keys
            .into_iter()
            .map(|k| ValidatorKeyBinding {
                election_id: k.election_id,
                validator_key_id: k.validator_key_id,
                validator_adnl_key_id: k.validator_adnl_key_id,
                expire_at: k.expire_at,
            })
            .collect())
    }

    fn set_validation_status(&self, status: ValidationStatus) {
        self.set_validation_status(status)
    }

    fn last_validation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        self.last_validation_time()
    }

    fn set_last_validation_time(&self, shard: ShardIdent, time: u64) {
        self.set_last_validation_time(shard, time)
    }

    fn remove_last_validation_time(&self, shard: &ShardIdent) {
        self.remove_last_validation_time(shard)
    }

    fn last_collation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        self.last_collation_time()
    }

    fn set_last_collation_time(&self, shard: ShardIdent, time: u64) {
        self.set_last_collation_time(shard, time)
    }

    fn remove_last_collation_time(&self, shard: &ShardIdent) {
        self.remove_last_collation_time(shard)
    }

    fn remove_validator_list(&self, validator_list_id: UInt256) -> Result<bool> {
        self.validator_network().remove_validator_list(validator_list_id)
    }

    fn create_catchain_client(
        &self,
        validator_list_id: UInt256,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes_public_keys: &[CatchainNode],
        listener: CatchainOverlayListenerPtr,
        _log_replay_listener: CatchainOverlayLogReplayListenerPtr,
        broadcast_hops: Option<u8>,
        transport_type: consensus_common::OverlayTransportType,
    ) -> Result<Arc<dyn CatchainOverlay + Send>> {
        self.validator_network().create_catchain_client(
            validator_list_id,
            local_validator_key,
            overlay_short_id,
            nodes_public_keys,
            listener,
            _log_replay_listener,
            broadcast_hops,
            transport_type,
        )
    }

    fn stop_catchain_client(&self, overlay_short_id: &Arc<PrivateOverlayShortId>) {
        self.validator_network().stop_catchain_client(overlay_short_id)
    }

    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        self.db().load_block_handle(id)
    }

    async fn load_applied_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        // TODO make cache?
        if handle.is_applied() {
            self.load_block(handle).await
        } else if handle.has_data() {
            fail!("Block is not applied yet")
        } else {
            fail!("No block")
        }
    }

    async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        self.db().load_block_data(handle).await
    }

    async fn load_block_raw(&self, handle: &BlockHandle) -> Result<Vec<u8>> {
        self.db().load_block_data_raw(handle).await
    }

    async fn wait_applied_block(
        &self,
        id: &BlockIdExt,
        timeout_ms: Option<u64>,
    ) -> Result<Arc<BlockHandle>> {
        loop {
            let is_applied = || {
                if let Some(handle) = self.load_block_handle(id)? {
                    Ok(handle.is_applied())
                } else {
                    Ok(false)
                }
            };

            if let Some(handle) = self.load_block_handle(id)? {
                if handle.is_applied() {
                    return Ok(handle);
                }
            }

            self.block_applying_awaiters().wait(id, timeout_ms, &is_applied).await?;
        }
    }

    async fn wait_next_applied_mc_block(
        &self,
        prev_handle: &BlockHandle,
        timeout_ms: Option<u64>,
    ) -> Result<(Arc<BlockHandle>, BlockStuff)> {
        if !prev_handle.id().shard().is_masterchain() {
            fail!(NodeError::InvalidArg("`prev_handle` doesn't belong masterchain".to_string()))
        }
        let handle = loop {
            if prev_handle.has_next1() {
                let id = self.load_block_next1(prev_handle.id())?;
                break self.wait_applied_block(&id, timeout_ms).await?;
            } else if let Some(id) = self
                .next_block_applying_awaiters()
                .wait(prev_handle.id(), timeout_ms, || Ok(prev_handle.has_next1()))
                .await?
            {
                if let Some(handle) = self.load_block_handle(&id)? {
                    break handle;
                }
            }
        };
        let block = self.load_block(&handle).await?;
        Ok((handle, block))
    }

    async fn lookup_block_by_seqno(
        &self,
        prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        self.db().lookup_block_by_seqno(prefix, seqno).await
    }
    async fn lookup_block_by_lt(
        &self,
        prefix: &AccountIdPrefixFull,
        lt: u64,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        self.db().lookup_block_by_lt(prefix, lt).await
    }
    async fn lookup_blocks_by_utime<'a>(
        &self,
        prefix: &AccountIdPrefixFull,
        utime: u32,
        f: Box<dyn FnMut(BlockIdExt, Vec<u8>) -> Result<bool> + Send + 'a>,
    ) -> Result<()> {
        self.db().lookup_blocks_by_utime(prefix, utime, f).await
    }

    fn find_full_block_id(&self, root_hash: &UInt256) -> Result<Option<BlockIdExt>> {
        self.db().find_full_block_id(root_hash)
    }

    async fn load_last_applied_mc_block(&self) -> Result<BlockStuff> {
        match self.load_last_applied_mc_block_id()? {
            Some(block_id) => {
                let handle = self.load_block_handle(&block_id)?.ok_or_else(|| {
                    error!("Cannot load handle for last applied master block {}", block_id)
                })?;
                self.load_applied_block(&handle).await
            }
            None => fail!("INTERNAL ERROR: No last applied MC block set"),
        }
    }

    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        match self.load_last_applied_mc_block_id()? {
            Some(block_id) => self.load_state(&block_id).await,
            None => fail!("INTERNAL ERROR: No last applied MC block set"),
        }
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db().load_full_node_state(LAST_APPLIED_MC_BLOCK)
    }

    fn save_last_applied_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
        self.set_last_applied_mc_seqno(id.seq_no());
        if let Ok(Some(handle)) = self.load_block_handle(id) {
            let diff = self.now() as i64 - handle.gen_utime() as i64;
            metrics::gauge!("ton_node_engine_timediff_seconds").set(diff as f64);
            metrics::gauge!("ton_node_engine_last_mc_block_utime").set(handle.gen_utime() as f64);
        }
        self.db().save_full_node_state(LAST_APPLIED_MC_BLOCK, id)
    }

    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db().load_full_node_state(SHARD_CLIENT_MC_BLOCK)
    }

    fn save_shard_client_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
        metrics::gauge!("ton_node_engine_shards_mc_seqno").set(id.seq_no() as f64);
        if let Ok(Some(handle)) = self.load_block_handle(id) {
            let diff = self.now() as i64 - handle.gen_utime() as i64;
            metrics::gauge!("ton_node_engine_shards_timediff_seconds").set(diff as f64);
        }
        self.db().save_full_node_state(SHARD_CLIENT_MC_BLOCK, id)
    }

    fn load_last_rotation_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db().load_validator_state(LAST_ROTATION_MC_BLOCK)
    }

    fn save_last_rotation_block_id(&self, id: &BlockIdExt) -> Result<()> {
        self.db().save_validator_state(LAST_ROTATION_MC_BLOCK, id)
    }

    fn clear_last_rotation_block_id(&self) -> Result<()> {
        self.db().drop_validator_state(LAST_ROTATION_MC_BLOCK)
    }

    fn load_destroyed_session_ids(&self) -> Result<Vec<UInt256>> {
        match self.db().load_validator_state_raw(DESTROYED_VALIDATOR_SESSIONS)? {
            Some(data) => deserialize_destroyed_session_ids(&data),
            None => Ok(Vec::new()),
        }
    }

    fn save_destroyed_session_ids(&self, ids: &HashSet<UInt256>) -> Result<()> {
        if ids.is_empty() {
            self.db().drop_validator_state_raw(DESTROYED_VALIDATOR_SESSIONS)
        } else {
            let data = serialize_destroyed_session_ids(ids);
            self.db().save_validator_state_raw(DESTROYED_VALIDATOR_SESSIONS, &data)
        }
    }

    fn clear_destroyed_session_ids(&self) -> Result<()> {
        self.db().drop_validator_state_raw(DESTROYED_VALIDATOR_SESSIONS)
    }

    fn save_block_candidate(
        &self,
        session_id: &SessionId,
        candidate: Arc<ValidatorBlockCandidate>,
    ) -> Result<()> {
        self.get_candidate_table(session_id)?.save(candidate)
    }

    fn load_block_candidate(
        &self,
        session_id: &SessionId,
        root_hash: &BlockHash,
    ) -> Result<Arc<ValidatorBlockCandidate>> {
        self.get_candidate_table(session_id)?.load(root_hash)
    }

    fn destroy_block_candidates(&self, session_id: &SessionId) -> Result<bool> {
        self.destroy_candidate_table(session_id)
    }

    async fn apply_block_internal(
        self: Arc<Self>,
        handle: &Arc<BlockHandle>,
        block: &BlockStuff,
        mc_seq_no: u32,
        pre_apply: bool,
        recursion_depth: u32,
    ) -> Result<()> {
        // if it is pre-apply we are waiting for `state_inited` or `applied`
        // otherwise - only for applied
        while !((pre_apply && handle.has_state()) || handle.is_applied()) {
            self.block_applying_awaiters()
                .do_or_wait(
                    handle.id(),
                    Some(1000),
                    self.clone().apply_block_worker(
                        handle,
                        block,
                        mc_seq_no,
                        pre_apply,
                        recursion_depth,
                    ),
                )
                .await?;
        }
        Ok(())
    }

    fn cache_block_candidate(
        &self,
        id: &BlockIdExt,
        _cc_seqno: u32,
        _validator_set_hash: u32,
        _signature: CryptoSignaturePair,
        block_data: Vec<u8>,
    ) -> Result<()> {
        self.cache_block_candidate(id, block_data)?;
        Ok(())
    }

    async fn download_and_apply_block_internal(
        self: Arc<Self>,
        id: &BlockIdExt,
        mc_seq_no: u32,
        pre_apply: bool,
        recursion_depth: u32,
    ) -> Result<()> {
        self.download_and_apply_block_worker(id, mc_seq_no, pre_apply, recursion_depth).await
    }

    async fn download_block(
        &self,
        id: &BlockIdExt,
        limit: Option<u32>,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        loop {
            if let Some(handle) = self.load_block_handle(id)? {
                let mut is_link = false;
                if handle.has_data() && handle.has_proof_or_link(&mut is_link) {
                    let block = self.load_block(&handle).await?;
                    let proof =
                        self.load_block_proof(&handle, !id.shard().is_masterchain()).await?;
                    log::trace!("download_block {} loaded from DB", id);
                    return Ok((block, proof));
                }
            }

            if !id.shard().is_masterchain() {
                if let Some(block_data) = self.try_get_cached_block_candidate(id) {
                    let block = BlockStuff::deserialize_block(id.clone(), block_data.clone())?;
                    let proof = create_new_proof_link(&block)?;
                    log::trace!("download_block {} loaded from candidates cache", id);
                    return Ok((block, proof));
                }
            }

            if let Some((block, proof)) = self
                .download_block_awaiters()
                .do_or_wait(id, None, self.download_block_worker(id, limit, None))
                .await?
            {
                log::trace!("download_block {} downloaded", id);
                return Ok((block, proof));
            }
        }
    }

    async fn download_block_proof(
        &self,
        id: &BlockIdExt,
        is_link: bool,
        key_block: bool,
    ) -> Result<BlockProofStuff> {
        self.download_block_proof_worker(id, is_link, key_block, Some(1)).await
    }

    async fn download_next_block(
        &self,
        prev_id: &BlockIdExt,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        self.download_next_block_worker(prev_id, None).await
    }

    async fn apply_persistent_state(
        &self,
        handle: &Arc<BlockHandle>,
        root_hash: &UInt256,
        data: Arc<Vec<u8>>,
        cells_index: Vec<(UInt256, u16)>,
    ) -> Result<(Arc<ShardStateStuff>, Vec<(UInt256, u16)>)> {
        self.shard_states_keeper().check_and_store_state(handle, root_hash, data, cells_index).await
    }

    async fn apply_persistent_state_part(
        &self,
        id: &PersistentStatePartId,
        data: Arc<Vec<u8>>,
        cells_index: Vec<(UInt256, u16)>,
    ) -> Result<(Cell, Vec<(UInt256, u16)>)> {
        self.shard_states_keeper().store_state_part(id, data, cells_index).await
    }

    async fn apply_persistent_state_header(
        &self,
        handle: &Arc<BlockHandle>,
        root_hash: &UInt256,
        part_root_hashes: &[UInt256],
        header_boc: Arc<Vec<u8>>,
    ) -> Result<Arc<ShardStateStuff>> {
        self.shard_states_keeper()
            .merge_and_check_state(handle, root_hash, part_root_hashes, header_boc)
            .await
    }

    async fn download_persistent_state(
        &self,
        id: PersistentStatePartId,
        master_id: &BlockIdExt,
        attempts: Option<usize>,
    ) -> Result<(usize, usize)> {
        // (bytes, cells_count)
        let overlay = self.overlay_client(id.block_id().shard()).await?;

        let total_attempts = attempts.unwrap_or(1);
        for attempt in 1..=total_attempts {
            let mut dest_file = self.db().shard_state_persistent_write_obj(&id)?;
            match crate::full_node::state_helper::download_pss_part(
                &id,
                master_id,
                &mut dest_file,
                overlay.deref(),
                attempts,
                &|| {
                    if self.check_stop() {
                        fail!("Persistent state downloading was stopped")
                    }
                    Ok(())
                },
            )
            .await
            {
                Ok(r) => {
                    self.db().finalize_shard_state_persistent(&id, dest_file)?;
                    return Ok(r);
                }
                Err(e) => {
                    log::warn!(
                        "download_persistent_state attempt {} for {} failed: {}",
                        attempt,
                        id,
                        e
                    );
                    continue;
                }
            }
        }
        fail!("download_persistent_state: all {} attempts for {} failed", total_attempts, id)
    }

    // Cleanup all persistent states except given.
    // Used for cleaning partially downloaded states after unsuccesfull boot attempts.
    async fn cleanup_persistent_states(&self) -> Result<()> {
        self.db().cleanup_shard_states_persistent().await
    }

    async fn download_zerostate(&self, id: &BlockIdExt) -> Result<(Arc<ShardStateStuff>, Vec<u8>)> {
        self.download_zerostate_worker(id, None).await
    }

    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        Ok(Engine::zerostate_id(self))
    }

    async fn store_block(&self, block: &BlockStuff) -> Result<BlockResult> {
        let result = self.db().store_block_data(block, None).await?;
        if let Some(handle) = result.clone().to_updated() {
            let id = block.id();
            if id.shard().is_masterchain() {
                let seq_no = id.seq_no();
                if handle.is_key_block()? {
                    self.update_last_known_keyblock_seqno(seq_no);
                }
                self.update_last_known_mc_block_seqno(seq_no);
            }
        }
        Ok(result)
    }

    async fn store_block_proof(
        &self,
        id: &BlockIdExt,
        handle: Option<Arc<BlockHandle>>,
        proof: &BlockProofStuff,
    ) -> Result<BlockResult> {
        self.db().store_block_proof(id, handle, proof, None).await
    }

    async fn load_block_proof(
        &self,
        handle: &Arc<BlockHandle>,
        is_link: bool,
    ) -> Result<BlockProofStuff> {
        // TODO make cache?
        self.db().load_block_proof(handle, is_link).await
    }

    async fn load_block_proof_raw(&self, handle: &BlockHandle, is_link: bool) -> Result<Vec<u8>> {
        self.db().load_block_proof_raw(handle, is_link).await
    }

    async fn load_mc_zero_state(&self) -> Result<Arc<ShardStateStuff>> {
        let block_id = self.zerostate_id();
        self.load_state(block_id).await
    }

    async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        self.shard_states_keeper().load_state(block_id).await
    }

    // It is prohibited to use any cell from the state after the guard's disposal.
    async fn load_and_pin_state(&self, block_id: &BlockIdExt) -> Result<PinnedShardStateGuard> {
        self.shard_states_keeper().load_and_pin_state(block_id).await
    }

    async fn load_persistent_state_size(&self, id: &PersistentStatePartId) -> Result<u64> {
        self.db().load_shard_state_persistent_size(id).await
    }

    async fn load_persistent_state_slice(
        &self,
        id: &PersistentStatePartId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        self.db().load_shard_state_persistent_slice(id, offset, length).await
    }

    async fn load_persistent_state_to(
        &self,
        id: &PersistentStatePartId,
        buffer: &mut Vec<u8>,
    ) -> Result<()> {
        self.db().load_shard_state_persistent_to(id, buffer).await
    }

    async fn wait_state(
        self: Arc<Self>,
        id: &BlockIdExt,
        timeout_ms: Option<u64>,
        allow_block_downloading: bool,
    ) -> Result<Arc<ShardStateStuff>> {
        loop {
            let has_state =
                || Ok(self.load_block_handle(id)?.map(|h| h.has_state()).unwrap_or(false));

            if has_state()? {
                break self.load_state(id).await;
            }
            let id1 = id.clone();
            let engine = self.clone();
            if allow_block_downloading {
                tokio::spawn(async move {
                    if let Err(e) = engine.download_and_apply_block(&id1, 0, true).await {
                        log::error!(
                            "Error while pre-apply block (while wait_state) {}: {:?}",
                            id1,
                            e
                        );
                    }
                });
            }
            if let Some(ss) = self.shard_states_awaiters().wait(id, timeout_ms, &has_state).await? {
                break Ok(ss);
            }
        }
    }

    async fn store_state(
        &self,
        handle: &Arc<BlockHandle>,
        state: Arc<ShardStateStuff>,
    ) -> Result<Arc<ShardStateStuff>> {
        let (state, saved) =
            self.shard_states_keeper().store_state(handle, state, None, false).await?;
        if saved {
            #[cfg(feature = "telemetry")]
            self.full_node_telemetry().new_pre_applied_block(handle.got_by_broadcast());
        }
        self.shard_states_awaiters()
            .do_or_wait(state.block_id(), None, async { Ok(state.clone()) })
            .await?;
        Ok(state)
    }

    async fn store_zerostate(
        &self,
        state: Arc<ShardStateStuff>,
        state_bytes: &[u8],
    ) -> Result<(Arc<ShardStateStuff>, Arc<BlockHandle>)> {
        let handle = self
            .db()
            .create_or_load_block_handle(
                state.block_id(),
                None,
                Some(state.state()?.gen_time()),
                None,
            )?
            .to_non_updated()
            .ok_or_else(|| error!("INTERNAL ERROR: mismatch in zerostate storing"))?;
        let (state, _) = self
            .shard_states_keeper()
            .store_state(&handle, state, Some(state_bytes), false)
            .await?;
        self.shard_states_awaiters()
            .do_or_wait(state.block_id(), None, async { Ok(state.clone()) })
            .await?;
        Ok((state, handle))
    }

    fn store_block_prev1(&self, handle: &Arc<BlockHandle>, prev: &BlockIdExt) -> Result<()> {
        self.db().store_block_prev1(handle, prev, None)
    }

    fn load_block_prev1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db().load_block_prev1(id)
    }

    fn store_block_prev2(&self, handle: &Arc<BlockHandle>, prev2: &BlockIdExt) -> Result<()> {
        self.db().store_block_prev2(handle, prev2, None)
    }

    fn load_block_prev2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.db().load_block_prev2(id)
    }

    fn store_block_next1(&self, handle: &Arc<BlockHandle>, next: &BlockIdExt) -> Result<()> {
        self.db().store_block_next1(handle, next, None)
    }

    fn load_block_next1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db().load_block_next1(id)
    }

    fn store_block_next2(&self, handle: &Arc<BlockHandle>, next2: &BlockIdExt) -> Result<()> {
        self.db().store_block_next2(handle, next2, None)
    }

    fn load_block_next2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.db().load_block_next2(id)
    }

    async fn download_next_key_blocks_ids(&self, block_id: &BlockIdExt) -> Result<Vec<BlockIdExt>> {
        let mc_overlay = self.overlay_client(&ShardIdent::masterchain()).await?;
        mc_overlay.download_next_key_blocks_ids(block_id, 5).await
    }

    fn process_block_broadcast(self: Arc<Self>, broadcast: BlockBroadcast, src: Arc<KeyId>) {
        // because of ALL blocks-broadcasts received in one task - spawn for each block
        log::trace!("Processing block broadcast {}", broadcast.id);
        let engine = self.clone() as Arc<dyn EngineOperations>;
        tokio::spawn(async move {
            let id = broadcast.id.clone();
            match process_block_broadcast(&engine, broadcast).await {
                Err(e) => {
                    log::error!(
                        "Error while processing block broadcast {} from {}: {:?}",
                        id,
                        src,
                        e
                    )
                }
                Ok(_block_opt) => {
                    log::trace!("Processed block broadcast {} from {}", id, src);
                }
            }
        });
    }

    fn process_block_broadcast_v2(
        self: Arc<Self>,
        broadcast: crate::network::BlockBroadcastV2,
        src: Arc<KeyId>,
    ) {
        // V2 broadcast processing - spawn for async processing
        log::trace!("Processing block broadcast V2 {}", broadcast.id);
        let engine = self.clone() as Arc<dyn EngineOperations>;
        tokio::spawn(async move {
            let id = broadcast.id.clone();
            match process_block_broadcast_v2(&engine, broadcast).await {
                Err(e) => {
                    log::error!(
                        "Error while processing block broadcast V2 {} from {}: {:?}",
                        id,
                        src,
                        e
                    )
                }
                Ok(_block_opt) => {
                    log::trace!("Processed block broadcast V2 {} from {}", id, src);
                }
            }
        });
    }

    async fn process_ext_msg_broadcast(
        &self,
        broadcast: ExternalMessageBroadcast,
        src: Arc<KeyId>,
    ) {
        // just add to list
        if !self.is_validator() {
            log::trace!(
                target: EXT_MESSAGES_TRACE_TARGET,
                "Skipped ext message broadcast {}bytes from {}: NOT A VALIDATOR",
                broadcast.message.data.len(), src
            );
        } else {
            let bytes_len = broadcast.message.data.len();
            let result =
                self.external_messages().new_message_raw(&broadcast.message.data, self.now());
            match result {
                Err(e) => {
                    log::error!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "Error while processing ext message broadcast {}bytes from {}: {}",
                        bytes_len, src, e
                    );
                }
                Ok(_) => {
                    log::debug!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "Processed ext message broadcast {}bytes from {}",
                        bytes_len, src,
                    );
                }
            }
        }
    }

    fn process_new_shard_block_broadcast(
        self: Arc<Self>,
        broadcast: NewShardBlockBroadcast,
        src: Arc<KeyId>,
    ) {
        let id = broadcast.block.block.clone();
        log::trace!("Processing new shard block broadcast {} from {}", id, src);
        tokio::spawn(async move {
            match self.clone().process_new_shard_block(broadcast).await {
                Err(e) => {
                    log::warn!(
                        "Couldn't process new shard block broadcast {} from {}: {}",
                        id,
                        src,
                        e
                    );
                    #[cfg(feature = "telemetry")]
                    self.full_node_telemetry().bad_top_block_broadcast();
                }
                Ok(id) => {
                    log::trace!("Processed new shard block broadcast {} from {}", id, src);
                    #[cfg(feature = "telemetry")]
                    self.full_node_telemetry().good_top_block_broadcast(&id);
                }
            }
        });
    }

    async fn set_applied(&self, handle: &Arc<BlockHandle>, mc_seq_no: u32) -> Result<bool> {
        if handle.is_applied() {
            return Ok(false);
        }
        self.db().assign_mc_ref_seq_no(handle, mc_seq_no, None)?;
        if handle.id().seq_no() != 0 {
            self.db().archive_block(handle.id(), None).await?;
        }
        if self.db().store_block_applied(handle, None)? {
            #[cfg(feature = "telemetry")]
            self.full_node_telemetry().new_applied_block();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn hardforks(&self) -> &[BlockIdExt] {
        self.hardforks()
    }

    fn flags(&self) -> &EngineFlags {
        Engine::flags(self)
    }

    fn init_mc_block_id(&self) -> &BlockIdExt {
        (self as &Engine).init_mc_block_id()
    }

    fn save_init_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
        self.db().save_full_node_state(INITIAL_MC_BLOCK, id)
    }

    async fn send_ext_message_broadcast(
        &self,
        to: &AccountIdPrefixFull,
        data: &[u8],
    ) -> Result<()> {
        self.overlays_router()?.send_ext_message_broadcast(to, data).await
    }

    async fn redirect_external_message(&self, message_data: &[u8]) -> Result<()> {
        if !self.check_sync().await? {
            fail!("Can't process external message because node is out of sync");
        }
        match create_ext_message(message_data) {
            Err(e) => {
                let err = format!(
                    "Can't deserialize external message with len {}: {}",
                    message_data.len(),
                    e,
                );
                log::warn!(target: EXT_MESSAGES_TRACE_TARGET, "{}", &err);
                fail!("{}", err);
            }
            Ok((id, message)) => {
                match redirect_external_message(self, message, id.clone(), message_data).await {
                    Err(e) => {
                        let err = format!("Can't redirect external message {:x}: {}", id, e,);
                        log::error!(target: EXT_MESSAGES_TRACE_TARGET, "{}", &err);
                        fail!("{}", err);
                    }
                    Ok(_) => {
                        log::debug!(
                            target: EXT_MESSAGES_TRACE_TARGET,
                            "Redirected external message {:x}",
                            id,
                        );
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn get_archive_id(&self, mc_seq_no: u32, shard: &ShardIdent) -> Option<u64> {
        self.db().get_archive_id(mc_seq_no, shard).await
    }

    async fn get_archive_slice(&self, archive_id: u64, offset: u64, limit: u32) -> Result<Vec<u8>> {
        self.db().get_archive_slice(archive_id, offset, limit).await
    }

    async fn download_archive(
        &self,
        shard: Option<ShardIdent>,
        masterchain_seqno: u32,
    ) -> Result<Option<Vec<u8>>> {
        let shard = shard.unwrap_or_else(|| ShardIdent::masterchain());
        self.overlay_client(&shard).await?.download_archive(masterchain_seqno, &shard).await
    }

    async fn send_block_broadcast(
        &self,
        block: &BlockStuff,
        proof: &BlockProofStuff,
        signatures: &BlockSignaturesVariant,
    ) -> Result<()> {
        log::trace!("send_block_broadcast {}", block.id());
        self.overlays_router()?.send_block_broadcast(block, proof, signatures).await?;
        #[cfg(feature = "telemetry")]
        self.full_node_telemetry().sent_block_broadcast();
        Ok(())
    }

    async fn send_top_shard_block_description(
        &self,
        tbd: Arc<TopBlockDescrStuff>,
        cc_seqno: u32,
        is_resend: bool,
    ) -> Result<()> {
        if !is_resend {
            let id = tbd.proof_for();
            if let Err(e) = self
                .shard_blocks()
                .process_shard_block(id, cc_seqno, || Ok(tbd.clone()), false, self)
                .await
            {
                log::error!("Can't add own shard top block {}: {}", id, e);
            }
        }

        self.overlays_router()?.send_top_shard_block_description(tbd).await?;

        #[cfg(feature = "telemetry")]
        self.full_node_telemetry().sent_top_block_broadcast();
        Ok(())
    }

    async fn send_block_candidate_broadcast(
        &self,
        id: &BlockIdExt,
        cc_seqno: u32,
        validator_set_hash: u32,
        block_root: &Cell,
    ) -> Result<()> {
        log::trace!("send_block_candidate_broadcast {}", id);
        self.overlays_router()?
            .send_block_candidate_broadcast(id, cc_seqno, validator_set_hash, block_root)
            .await?;
        #[cfg(feature = "telemetry")]
        self.full_node_telemetry().sent_block_candidate_broadcast();
        Ok(())
    }

    fn new_external_message(&self, id: &UInt256, message: Arc<Message>) -> Result<()> {
        if !self.is_validator() {
            return Ok(());
        }
        self.external_messages().new_message(id, message, self.now())
    }

    fn get_external_messages_iterator(
        &self,
        shard: ShardIdent,
        finish_time_ms: u64,
    ) -> Box<dyn Iterator<Item = (Arc<Message>, UInt256)> + Send + Sync> {
        Box::new(self.external_messages().clone().iter(shard, self.now(), finish_time_ms))
    }

    fn get_external_messages_len(&self) -> u32 {
        self.external_messages().total_messages()
    }

    fn complete_external_messages(
        &self,
        to_delay: Vec<(UInt256, String)>,
        to_delete: Vec<(UInt256, i32)>,
    ) -> Result<()> {
        self.external_messages().complete_messages(to_delay, to_delete, self.now())
    }

    // Get current list of new shard blocks with respect to last mc block.
    // If given mc_seq_no is not equal to last mc seq_no - function fails.
    async fn get_shard_blocks(
        &self,
        mc_state: &Arc<ShardStateStuff>,
        actual_last_mc_seqno: Option<&mut u32>,
    ) -> Result<Vec<Arc<TopBlockDescrStuff>>> {
        self.shard_blocks().get_shard_blocks(mc_state, self, false, actual_last_mc_seqno).await
    }
    async fn get_own_shard_blocks(
        &self,
        mc_state: &Arc<ShardStateStuff>,
        actual_last_mc_seqno: Option<&mut u32>,
    ) -> Result<Vec<Arc<TopBlockDescrStuff>>> {
        self.shard_blocks().get_shard_blocks(mc_state, self, true, actual_last_mc_seqno).await
    }

    // Save tsb into persistent storage
    fn save_top_shard_block(&self, id: &TopBlockDescrId, tsb: &TopBlockDescrStuff) -> Result<()> {
        self.db().save_top_shard_block(id, tsb)
    }

    // Remove tsb from persistent storage
    fn remove_top_shard_block(&self, id: &TopBlockDescrId) -> Result<()> {
        self.db().remove_top_shard_block(id)
    }

    fn test_bundles_config(&self) -> &CollatorTestBundlesGeneralConfig {
        Engine::test_bundles_config(self)
    }

    fn collator_config(&self) -> &CollatorConfig {
        Engine::collator_config(self)
    }

    fn collator_config_mc(&self) -> &CollatorConfig {
        Engine::collator_config_mc(self)
    }

    fn db_root_dir(&self) -> Result<&str> {
        self.db().db_root_dir()
    }

    #[cfg(feature = "telemetry")]
    fn full_node_telemetry(&self) -> &FullNodeTelemetry {
        Engine::full_node_telemetry(self)
    }

    #[cfg(feature = "telemetry")]
    fn collator_telemetry(&self) -> &CollatorValidatorTelemetry {
        Engine::collator_telemetry(self)
    }

    #[cfg(feature = "telemetry")]
    fn validator_telemetry(&self) -> &CollatorValidatorTelemetry {
        Engine::validator_telemetry(self)
    }

    #[cfg(feature = "telemetry")]
    fn full_node_service_telemetry(&self) -> &FullNodeNetworkTelemetry {
        Engine::full_node_service_telemetry(self)
    }

    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        Engine::engine_telemetry(self)
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        Engine::engine_allocated(self)
    }

    fn calc_tps(&self, period: u64) -> Result<u32> {
        self.tps_counter().calc_tps(period)
    }

    fn adjust_states_gc_interval(&self, interval_ms: u32) {
        self.db().adjust_states_gc_interval(interval_ms)
    }

    fn acquire_stop(&self, mask: u32) {
        self.stopper().acquire_stop(mask);
    }

    fn check_stop(&self) -> bool {
        self.stopper().check_stop()
    }

    fn release_stop(&self, mask: u32) {
        self.stopper().release_stop(mask);
    }

    // returns true if there were no either calculating or done queues before
    fn set_split_queues_calculating(&self, before_split_block: &BlockIdExt) -> bool {
        // insert None is there was not value before and return true
        // return false if there was any value (None or Some) before
        adnl::common::add_unbound_object_to_map_with_update(
            self.split_queues_cache(),
            before_split_block.clone(),
            |v| {
                if v.is_some() {
                    fail!("");
                }
                Ok(Some(None))
            },
        )
        .is_ok()
    }

    fn set_split_queues(
        &self,
        before_split_block: &BlockIdExt,
        queue0: OutMsgQueue,
        queue1: OutMsgQueue,
        visited_cells: HashSet<UInt256>,
    ) {
        self.split_queues_cache()
            .insert(before_split_block.clone(), Some((queue0, queue1, visited_cells)));
    }

    fn get_split_queues(&self, before_split_block: &BlockIdExt) -> SplitQueues {
        if let Some(guard) = self.split_queues_cache().get(before_split_block) {
            if let Some(q) = guard.val() {
                return Some(q.clone());
            }
        }
        None
    }

    fn db_cells_factory(&self) -> Result<Arc<dyn CellsFactory>> {
        self.db().cells_factory()
    }

    fn db_cells_loader(&self) -> Result<Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>> {
        self.db().cells_loader()
    }

    fn get_account_storage_dict(&self, dict_hash: &UInt256) -> Option<Cell> {
        self.get_account_storage_dict(dict_hash)
    }

    fn add_account_storage_dict(&self, dict: Cell, size: u64) {
        self.add_account_storage_dict(dict, size)
    }

    async fn update_custom_overlays(&self, configs: Option<&[CustomOverlay]>) -> Result<()> {
        self.overlays_router()?.update_custom_overlays(configs).await
    }

    async fn update_public_overlays(
        &self,
        keyblock_id: &BlockIdExt,
        config: &ConfigParams,
    ) -> Result<()> {
        self.update_public_overlays(keyblock_id, config).await
    }
}

async fn redirect_external_message(
    engine: &dyn EngineOperations,
    message: Message,
    id: UInt256,
    message_data: &[u8],
) -> Result<()> {
    let message = Arc::new(message);
    engine.new_external_message(&id, message.clone())?;
    if let Some(header) = message.ext_in_header() {
        let res = engine
            .send_ext_message_broadcast(
                &AccountIdPrefixFull::checked_prefix(&header.dst)?,
                message_data,
            )
            .await;
        #[cfg(feature = "telemetry")]
        engine.full_node_telemetry().sent_ext_msg_broadcast();
        res
    } else {
        fail!("External message is not properly formatted: {}", message)
    }
}

#[cfg(test)]
#[path = "tests/test_engine_operations.rs"]
mod tests;
