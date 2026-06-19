/*
 * Copyright (C) 2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Block-sync overlay observer: passive listener that caches inbound candidates
//!
//! C++ ref: `block-sync-overlay.cpp` `BlockSyncObserver` (off-shard / non-validator path)
//!
//! Differences from the validator-side block-sync wiring: no consensus private overlay,
//! no simplex stack, same precheck callback. Created per (shard, session_id) by
//! `validator_manager` and stopped on session end or role transition

use crate::engine_traits::EngineOperations;
use adnl::{BroadcastCheck, OverlayNode, OverlayParams, PrivateOverlayShortId};
use consensus_common::{BlockSyncCheck, BlockSyncOverlayParams, BlockSyncRole, BlockSyncStats};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use ton_api::{
    deserialize_boxed,
    ton::consensus::{candidatedata::Block as ConsensusBlock, CandidateData},
};
use ton_block::{
    fail, BlockIdExt, CryptoSignaturePair, KeyId, KeyOption, Result, ShardIdent, UInt256,
};

const LOG_TARGET: &str = "block_sync_observer";

/// Read-only subscriber of a per-session block-sync overlay. Drops received
/// candidates into the engine's block-candidate cache. See module-level doc
pub struct BlockSyncObserver {
    overlay_node: Arc<OverlayNode>,
    overlay_short_id: Arc<PrivateOverlayShortId>,
    shard: ShardIdent,
    session_id: UInt256,
    local_adnl_key: Arc<dyn KeyOption>,
    engine: Arc<dyn EngineOperations>,
    stop_requested: Arc<AtomicBool>,
    runtime_handle: tokio::runtime::Handle,
    /// Post-reassembly sender check, parallel to the precheck callback
    authorized: HashSet<Arc<KeyId>>,
    max_candidate_size: usize,
    proto_version: u32,
    stats: Arc<BlockSyncStats>,
    /// JoinHandle for the listener task spawned by `run_listener`. Awaited
    /// from a cleanup task spawned in `stop` so the listener exits before
    /// any subsequent observer for the same session_id is constructed
    listener_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl BlockSyncObserver {
    /// Create and start a new observer; `Err` if our ADNL key is not in `params.members`
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        runtime_handle: tokio::runtime::Handle,
        overlay_node: Arc<OverlayNode>,
        local_adnl_key: Arc<dyn KeyOption>,
        params: BlockSyncOverlayParams,
        session_id: UInt256,
        shard: ShardIdent,
        engine: Arc<dyn EngineOperations>,
        use_quic: bool,
        broadcast_hops: Option<u8>,
        proto_version: u32,
    ) -> Result<Arc<Self>> {
        if !params.members.contains(local_adnl_key.id()) {
            fail!(
                "BlockSyncObserver: local ADNL key {} not in block-sync members ({} entries); \
                 observer cannot be admitted to the overlay",
                local_adnl_key.id(),
                params.members.len(),
            );
        }
        let overlay_short_id = simplex::utils::compute_block_sync_overlay_short_id(&session_id)?;

        let stats = BlockSyncStats::new(
            overlay_short_id.clone(),
            BlockSyncRole::Observer,
            Some(shard.clone()),
            Some(session_id.clone()),
        );

        let authorized: HashSet<Arc<KeyId>> = params.authorized_keys.keys().cloned().collect();
        let check: Arc<dyn BroadcastCheck> = Arc::new(BlockSyncCheck {
            overlay_id: overlay_short_id.clone(),
            authorized: authorized.clone(),
            max_payload_size: params.max_broadcast_size,
            current_set: params.current_set.clone(),
            slots_per_leader_window: params.slots_per_leader_window,
            stats: Some(stats.clone()),
        });

        let overlay_params = OverlayParams {
            flags: 0,
            hops: broadcast_hops,
            overlay_id: &overlay_short_id,
            runtime: Some(runtime_handle.clone()),
        };
        let added = overlay_node.add_private_overlay(
            overlay_params,
            &local_adnl_key,
            &params.members,
            use_quic,
            Some(check),
        )?;
        if !added {
            fail!(
                "BlockSyncObserver: overlay {overlay_short_id} already registered \
                 (session={session_id:x}); refusing to attach observer to a stale overlay"
            );
        }

        log::info!(
            target: LOG_TARGET,
            "created observer for shard={shard} session={:x} overlay={overlay_short_id} \
             (members={}, authorized={}, leader_window={})",
            session_id,
            params.members.len(),
            authorized.len(),
            params.slots_per_leader_window,
        );

        let observer = Arc::new(Self {
            overlay_node,
            overlay_short_id,
            shard,
            session_id,
            local_adnl_key,
            engine,
            stop_requested: Arc::new(AtomicBool::new(false)),
            runtime_handle: runtime_handle.clone(),
            authorized,
            max_candidate_size: params.max_broadcast_size as usize,
            proto_version,
            stats: stats.clone(),
            listener_handle: Mutex::new(None),
        });
        stats.spawn_ticker(runtime_handle, observer.stop_requested.clone());
        let handle = observer.clone().run_listener();
        *observer.listener_handle.lock().expect("listener_handle poisoned") = Some(handle);
        Ok(observer)
    }

    /// Stop the observer. Removes the overlay from the ADNL node and signals
    /// the listener task to exit at its next loop iteration
    pub fn stop(&self) {
        if self.stop_requested.swap(true, Ordering::Relaxed) {
            return;
        }
        // Final telemetry dump so short-lived sessions always log at least once
        self.stats.dump();
        log::info!(
            target: LOG_TARGET,
            "stopping observer for shard={} session={:x}",
            self.shard, self.session_id,
        );
        if let Err(e) = self.overlay_node.delete_private_overlay(&self.overlay_short_id) {
            log::warn!(
                target: LOG_TARGET,
                "observer for shard={}: delete_private_overlay({}) failed: {e}",
                self.shard, self.overlay_short_id,
            );
        }
        // Spawn an async cleanup task that awaits the listener exit (the
        // overlay deletion above causes `wait_for_broadcast` to return None)
        let handle = self.listener_handle.lock().expect("listener_handle poisoned").take();
        if let Some(handle) = handle {
            let shard = self.shard.clone();
            let overlay_short_id = self.overlay_short_id.clone();
            self.runtime_handle.spawn(async move {
                match handle.await {
                    Ok(()) => log::trace!(
                        target: LOG_TARGET,
                        "observer shard={shard}: listener task exited cleanly (overlay={overlay_short_id})"
                    ),
                    Err(e) => log::warn!(
                        target: LOG_TARGET,
                        "observer shard={shard}: listener task join failed: {e}"
                    ),
                }
            });
        }
    }

    pub fn shard(&self) -> &ShardIdent {
        &self.shard
    }

    fn run_listener(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak: Weak<Self> = Arc::downgrade(&self);
        let overlay_node = self.overlay_node.clone();
        let overlay_short_id = self.overlay_short_id.clone();
        let stop_requested = self.stop_requested.clone();
        let shard = self.shard.clone();
        self.runtime_handle.spawn(async move {
            log::trace!(
                target: LOG_TARGET,
                "observer shard={shard}: broadcast listener started on {overlay_short_id}"
            );
            loop {
                if stop_requested.load(Ordering::Relaxed) {
                    break;
                }
                match overlay_node.wait_for_broadcast(&overlay_short_id).await {
                    Ok(Some(message)) => {
                        let Some(this) = weak.upgrade() else { return };
                        this.process_inbound(message.recv_from, message.data);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "observer shard={shard}: wait_for_broadcast error: {e}"
                        );
                    }
                }
            }
            log::trace!(
                target: LOG_TARGET,
                "observer shard={shard}: broadcast listener stopped"
            );
        })
    }

    fn process_inbound(&self, recv_from: Arc<KeyId>, data: Vec<u8>) {
        if recv_from == *self.local_adnl_key.id() {
            return;
        }
        // Post-reassembly sender check; the precheck callback gates earlier
        if !self.authorized.contains(&recv_from) {
            self.stats.bump_auth_drop();
            log::warn!(
                target: LOG_TARGET,
                "observer shard={}: dropping broadcast from non-validator {recv_from}",
                self.shard
            );
            return;
        }
        self.stats.bump_recv();
        // Parse as consensus.CandidateData
        let obj = match deserialize_boxed(&data) {
            Ok(o) => o,
            Err(e) => {
                log::warn!(
                    target: LOG_TARGET,
                    "observer shard={}: failed to deserialize broadcast: {e}",
                    self.shard
                );
                return;
            }
        };
        let cand: CandidateData = match obj.downcast() {
            Ok(c) => c,
            Err(_) => {
                log::warn!(
                    target: LOG_TARGET,
                    "observer shard={}: non-CandidateData payload on block-sync overlay (dropped)",
                    self.shard
                );
                return;
            }
        };
        let block: ConsensusBlock = match cand {
            CandidateData::Consensus_Block(b) => b,
            // Empty candidates carry no body to cache; skip silently
            CandidateData::Consensus_Empty(_) => return,
        };
        if let Err(e) = self.cache_candidate(&block) {
            log::warn!(
                target: LOG_TARGET,
                "observer shard={}: cache_candidate failed: {e}",
                self.shard
            );
        }
    }

    fn cache_candidate(&self, block: &ConsensusBlock) -> Result<()> {
        let info = simplex::utils::extract_block_info_from_candidate(
            &block.candidate,
            &self.shard,
            self.max_candidate_size,
            self.proto_version,
        )?;
        let Some(info) = info else {
            // Empty candidate body inside consensus.block (shouldn't normally
            // happen for non-empty candidates; bail quietly)
            return Ok(());
        };
        let block_id: BlockIdExt = info.block_id;
        let block_data = info.data;
        log::trace!(
            target: LOG_TARGET,
            "observer shard={}: caching candidate {block_id} ({} bytes)",
            self.shard, block_data.len(),
        );
        // The EngineOperations trait impl ignores cc_seqno/validator_set_hash/
        // signature and delegates to the inner Engine::cache_block_candidate
        // (id + block_data + file_hash check). Dummy zeros are safe here
        self.engine.cache_block_candidate(
            &block_id,
            /*cc_seqno*/ 0,
            /*validator_set_hash*/ 0,
            CryptoSignaturePair::default(),
            block_data,
        )?;
        Ok(())
    }
}
