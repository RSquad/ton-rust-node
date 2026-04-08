/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Startup Recovery for Simplex Consensus
//!
//! This module implements the session startup recovery stage that runs after
//! bootstrap is loaded and before normal event processing begins.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │ Session Main Loop (startup stage)                               │
//! │                                                                 │
//! │  1) SimplexDb::open()                                           │
//! │  2) SessionStartupRecoveryProcessor::new(...)                   │
//! │     - loads bootstrap (cancellable)                             │
//! │     - computes recovery identity (self_idx, keys)               │
//! │  3) ReceiverWrapper::create(...)                                │
//! │  4) SessionProcessor::new(...)                                  │
//! │  5) recovery.apply_bootstrap(&mut processor)                    │
//! │     - vote replay (Phase 6.6 order)                             │
//! │     - set finalized boundary + apply local flags                │
//! │     - restore receiver caches                                   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Components
//!
//! - [`SessionStartupRecoveryListener`]: Object-safe trait for recovery operations
//! - [`SessionStartupRecoveryProcessor`]: Coordinator that loads bootstrap and drives recovery
//!
//! See [`RESTART-RECOMMIT-PLAN.md`] for detailed design documentation.

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    database::{
        Bootstrap, CandidateInfoRecord, FinalizedBlockRecord, NotarCertRecord, PoolStateRecord,
        VoteRecord,
    },
    misbehavior::VoteResult,
    session_description::SessionDescription,
    simplex_state::Vote,
    utils::extract_vote_and_signature,
    RawVoteData, SessionId,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::consensus::{
        candidatedata::Empty as CandidateDataEmpty, candidateid::CandidateId,
        simplex::Vote as TlVoteBoxed, CandidateData, CandidateHashData,
    },
    IntoBoxed,
};
use ton_block::{error, BlockIdExt, Result, UInt256};

/*
    Constants
*/

/// Logging target for recovery processor
const LOG_TARGET: &str = "startup_recovery";

/*
    Types
*/

/// Candidate hash type (SHA256 of CandidateHashData)
pub(crate) type CandidateHash = UInt256;

/// Signature bytes (Ed25519 signature)
pub(crate) type SignatureBytes = Vec<u8>;

/*
    SessionStartupRecoveryListener - object-safe trait for recovery operations
*/

/// Object-safe trait for startup recovery operations.
///
/// Implemented by `SessionProcessor` and used by `SessionStartupRecoveryProcessor`
/// to apply bootstrap state. This trait provides a clean boundary:
///
/// - Recovery logic (in `SessionStartupRecoveryProcessor`) sees only this trait
/// - `SessionProcessor` owns FSM invariants + receiver delegation internally
/// - Easy to mock/test in isolation
///
/// All methods should log TRACE at entry for debugging.
pub(crate) trait SessionStartupRecoveryListener {
    // ========================================================================
    // Bootstrap state restoration
    // ========================================================================

    /// Set the first non-finalized slot boundary.
    ///
    /// This advances `leader_window_offset` and prunes old `leader_windows`.
    /// Must be called AFTER vote replay so certificates are reconstructed.
    fn recovery_set_first_non_finalized_slot(&mut self, slot: SlotIndex);

    /// Process a vote during bootstrap replay.
    ///
    /// Restores vote accounting (weights, certificates) in SimplexState.
    /// Called for ALL votes during global replay pass.
    fn recovery_on_vote(
        &mut self,
        node_idx: ValidatorIndex,
        vote: Vote,
        signature: SignatureBytes,
        raw_vote: RawVoteData,
    ) -> VoteResult;

    /// Mark slot as voted for local validator (prevents double-voting).
    ///
    /// Called only for OUR votes during local flags pass.
    /// Sets `voted_notar`, `voted_skip`, `voted_final` flags as appropriate.
    fn recovery_mark_slot_voted_on_restart(&mut self, vote: &Vote);

    /// Set the first non-announced window index (from pool state).
    ///
    /// This is used for skip vote generation and pool state persistence.
    fn recovery_set_first_nonannounced_window(&mut self, window: WindowIndex);

    /// Generate skip votes for windows before `first_nonannounced_window`.
    ///
    /// Returns the number of skip votes generated.
    fn recovery_generate_restart_skip_votes(&mut self) -> usize;

    // ========================================================================
    // Startup event hygiene (drain/restore)
    // ========================================================================

    /// Drain FSM events produced by bootstrap replay.
    ///
    /// Keeps only `BroadcastVote` events; drops `BlockFinalized`, `SlotSkipped`,
    /// `NotarizationReached` (these would interfere with restart recovery).
    ///
    /// Returns the kept votes for later restoration.
    fn recovery_drain_startup_events(&mut self) -> Vec<Vote>;

    /// Restore kept `BroadcastVote` events to the front of the queue.
    ///
    /// Called after startup cache restoration so votes are broadcast on first `check_all()`.
    fn recovery_restore_startup_votes(&mut self, votes: Vec<Vote>);

    // ========================================================================
    // Round alignment (current_round seeding)
    // ========================================================================

    /// Seed the current round counter from finalized block count.
    ///
    /// After restart, `current_round` should reflect the number
    /// of finalized blocks so the first new block uses the correct round number.
    ///
    /// Reference: C++ publishes `BlockFinalized(last, true)` after loading finalized
    /// blocks; this Rust callback aligns the round counter without re-accepting blocks.
    ///
    /// # Arguments
    ///
    /// * `round` - The round number to set (typically = finalized block count)
    fn recovery_seed_current_round(&mut self, round: u32);

    // ========================================================================
    // Finalized block tracking (prevents parent-chain walk into missing data)
    // ========================================================================

    /// Seed a finalized block into the tracking set.
    ///
    /// After restart, `collect_parent_chain` walks the parent chain until it hits
    /// a block in `finalized_blocks`. Without seeding, the walk would fail because
    /// `received_candidates` is empty after restart.
    ///
    /// # Arguments
    ///
    /// * `slot` - The slot of the finalized block
    /// * `block_hash` - The hash of the finalized block
    fn recovery_seed_finalized_block(&mut self, slot: SlotIndex, block_hash: CandidateHash);

    /// Seed ALL finalized blocks into received_candidates for parent resolution.
    ///
    /// After restart, `received_candidates` is empty, but collation/validation require
    /// parent `BlockIdExt` to be resolvable. This seeds all finalized blocks so their
    /// Block idExt can be looked up during parent resolution.
    ///
    /// # Arguments
    ///
    /// * `finalized_blocks` - All finalized blocks from bootstrap
    fn recovery_seed_received_candidates(&mut self, finalized_blocks: &[FinalizedBlockRecord]);

    /// Seed a candidate into `received_candidates` for parent resolution.
    ///
    /// After restart, collation uses the FSM progress cursor (`first_non_progressed_slot`)
    /// and can require a notarized (but not finalized) parent candidate's `BlockIdExt`.
    /// In single-validator setups (or during partitions) we may have no peers to query,
    /// so we must seed enough metadata locally to allow collation to proceed.
    ///
    /// This seeds a minimal `ReceivedCandidate` entry (block id + parent link + hash data bytes),
    /// without requiring the full candidate body to be present.
    fn recovery_seed_candidate_for_parent_resolution(
        &mut self,
        candidate_id: RawCandidateId,
        leader_idx: ValidatorIndex,
        block_id: BlockIdExt,
        parent: Option<RawCandidateId>,
        is_empty: bool,
        candidate_hash_data_bytes: Vec<u8>,
    );

    /// Notify about last finalized block after restart (Phase 6.5a).
    ///
    /// C++ equivalent: `consensus.cpp::load_from_db()` publishes
    /// `BlockFinalized(last_known_finalized_block, true)` after loading.
    ///
    /// This notification informs internal components (e.g., block producer state)
    /// about the restart finalization point WITHOUT re-accepting blocks.
    ///
    /// # Arguments
    ///
    /// * `slot` - The slot of the last finalized block
    /// * `block_hash` - The hash of the last finalized block
    /// * `seqno` - The block seqno of the last finalized block
    fn recovery_notify_last_finalized(
        &mut self,
        slot: SlotIndex,
        block_hash: CandidateHash,
        seqno: u32,
    );

    /// Finalize parent chain setup after all recovery steps complete.
    ///
    /// This must be called AFTER `recovery_restore_startup_votes` because the
    /// kept votes may finalize additional slots, advancing `first_non_finalized_slot`.
    /// This method sets the `available_base` for the CURRENT `first_non_finalized_slot`.
    ///
    /// Without this step, the first non-finalized slot after restart would have
    /// `available_base = None`, causing new blocks to be unvoteable.
    fn recovery_finalize_parent_chain(&mut self);

    // ========================================================================
    // Receiver cache delegation
    // ========================================================================

    /// Cache notarization certificate bytes in receiver.
    ///
    /// Used to answer `requestCandidate(want_notar=true)` after restart.
    fn recovery_cache_notarization_cert(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        notar_cert_bytes: Vec<u8>,
    );

    /// Cache candidate data bytes in receiver resolver cache.
    ///
    /// Used to answer `requestCandidate(want_candidate=true)` after restart.
    fn recovery_cache_candidate_bytes(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        candidate_data_bytes: Vec<u8>,
    );

    /// Seed notarization certificate into simplex_state.
    ///
    /// Used during restart to populate simplex_state.slot_votes with parsed
    /// notar certs. This is separate from `recovery_cache_notarization_cert`
    /// which only caches raw bytes in receiver for network queries.
    fn recovery_seed_notarize_certificate(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        certificate: crate::certificate::NotarCertPtr,
    );

    /// Rebuild Receiver standstill caches on restart (no network send).
    ///
    /// Restores C++-equivalent standstill state by rebuilding:
    /// - cached certificate bundles for `tracked_slots_interval()`,
    /// - `last_final_cert` (C++ `PoolImpl::last_final_cert_`),
    /// - local validator votes for standstill replay.
    ///
    /// This is intentionally separate from `recovery_restore_startup_votes`: it restores
    /// historical state from DB, whereas startup votes are freshly generated on restart.
    fn recovery_restore_receiver_standstill_cache(&mut self, votes: &[VoteRecord]);
}

/*
    SessionStartupRecoveryOptions - subset of SessionOptions for recovery
*/

/// Options for startup recovery (subset of SessionOptions).
///
/// Extracted to simplify testing and reduce coupling.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionStartupRecoveryOptions {
    /// Initial block seqno passed by session start; kept for future policy hooks.
    #[allow(dead_code)] // Reserved for future restart policies.
    pub initial_block_seqno: u32,
}

impl SessionStartupRecoveryOptions {
    /// Create from SessionOptions and initial_block_seqno
    #[allow(dead_code)] // Available for future use
    pub fn new(initial_block_seqno: u32) -> Self {
        Self { initial_block_seqno }
    }
}

/*
    SessionStartupRecoveryProcessor - coordinator that loads bootstrap and drives recovery
*/

/// Session startup recovery processor.
///
/// Coordinates the startup recovery stage:
/// 1. Loads bootstrap from DB (in constructor, cancellable)
/// 2. Computes recovery identity (self_idx, validator keys)
/// 3. Builds restore plans
/// 4. Drives recovery through `SessionStartupRecoveryListener`
///
/// Dropped before entering the main processing loop.
pub(crate) struct SessionStartupRecoveryProcessor {
    /// Session ID for logging
    session_id: SessionId,

    /// Session description (for leader key lookup during candidate reconstruction)
    _description: Arc<SessionDescription>,

    /// Self validator index (cached from description)
    self_idx: ValidatorIndex,

    /// Loaded bootstrap data (consumed during apply_bootstrap)
    bootstrap: Option<Bootstrap>,

    /// Pre-built candidate info map (candidate_hash -> CandidateInfoRecord)
    candidate_info_map: HashMap<CandidateHash, CandidateInfoRecord>,
}

impl SessionStartupRecoveryProcessor {
    /// Create a new recovery processor from pre-loaded bootstrap data.
    ///
    /// # Arguments
    ///
    /// * `session_id` - Session identifier
    /// * `description` - Session description (for self_idx, leader key lookup)
    /// * `options` - Recovery options
    /// * `bootstrap` - Pre-loaded bootstrap data
    ///
    /// # Returns
    ///
    /// * `Self` - Processor ready to apply bootstrap
    pub fn new(
        session_id: SessionId,
        description: Arc<SessionDescription>,
        _options: SessionStartupRecoveryOptions,
        bootstrap: Bootstrap,
    ) -> Self {
        let self_idx = description.get_self_idx();

        log::info!(
            target: LOG_TARGET,
            "Session {}: creating recovery processor (self_idx={}, {} finalized)",
            session_id.to_hex_string(),
            self_idx.value(),
            bootstrap.finalized_blocks.len()
        );

        // Build candidate info map for fast lookup (keyed by candidate_id.hash)
        let candidate_info_map = Self::build_candidate_info_map(&bootstrap.candidate_infos);

        Self {
            session_id,
            _description: description,
            self_idx,
            bootstrap: Some(bootstrap),
            candidate_info_map,
        }
    }

    /// Build a map from candidate_hash to CandidateInfoRecord for fast lookup.
    fn build_candidate_info_map(
        infos: &[CandidateInfoRecord],
    ) -> HashMap<CandidateHash, CandidateInfoRecord> {
        let mut map = HashMap::with_capacity(infos.len());
        for info in infos {
            // The hash is already in candidate_id.hash
            let hash = info.candidate_id.hash.clone();
            map.insert(hash, info.clone());
        }
        map
    }

    /// Check if this is a fresh start (no persisted bootstrap data).
    #[allow(dead_code)] // Available for logging/diagnostics
    pub fn is_fresh_start(&self) -> bool {
        self.bootstrap.as_ref().map(|b| b.is_empty()).unwrap_or(true)
    }

    /// Get the number of finalized blocks in bootstrap.
    pub fn finalized_block_count(&self) -> usize {
        self.bootstrap.as_ref().map(|b| b.finalized_blocks.len()).unwrap_or(0)
    }

    /// Apply bootstrap state and run startup recovery.
    ///
    /// This method:
    /// 1. Replays votes (Phase 6.6 order: global pass, set boundary, local flags)
    /// 2. Generates restart skip votes
    /// 3. Drains startup events (keeps BroadcastVote only)
    /// 4. Restores receiver caches (notar certs, candidate bytes)
    /// 5. Restores kept votes
    ///
    /// After this method returns, the processor is consumed and can be dropped.
    ///
    /// # Arguments
    ///
    /// * `listener` - SessionProcessor implementing SessionStartupRecoveryListener
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Recovery completed successfully
    /// * `Err` - Recovery failed (e.g., candidate fetch timeout)
    pub fn apply_bootstrap(
        mut self,
        listener: &mut dyn SessionStartupRecoveryListener,
    ) -> Result<()> {
        // Take bootstrap (consumes it)
        let bootstrap = match self.bootstrap.take() {
            Some(b) => b,
            None => {
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: apply_bootstrap called but bootstrap already consumed",
                    self.session_id.to_hex_string()
                );
                return Ok(());
            }
        };

        // Fresh start - nothing to recover
        if bootstrap.is_empty() {
            log::info!(
                target: LOG_TARGET,
                "Session {}: fresh start, skipping recovery",
                self.session_id.to_hex_string()
            );
            return Ok(());
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: applying bootstrap recovery",
            self.session_id.to_hex_string()
        );

        // Split bootstrap into session, receiver, and candidate payload parts
        let (session_boot, receiver_boot, candidate_payloads) = bootstrap.split();

        // Step 1: Replay ALL votes (global pass - restores weights, certificates)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 1/12 - replaying {} votes (global pass)",
            self.session_id.to_hex_string(),
            session_boot.votes.len()
        );
        self.replay_votes_global(listener, &session_boot.votes)?;

        // Step 2: Set first_non_finalized_slot boundary
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 2/12 - setting finalized boundary from {} blocks",
            self.session_id.to_hex_string(),
            session_boot.finalized_blocks.len()
        );
        self.apply_finalized_boundary(listener, &session_boot.finalized_blocks)?;

        // Step 3: Apply local vote flags (prevents double-voting)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 3/12 - applying local vote flags",
            self.session_id.to_hex_string()
        );
        self.apply_local_vote_flags(listener, &session_boot.votes)?;

        // Step 4: Set first_nonannounced_window and generate restart skip votes
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 4/12 - applying pool state and generating skip votes",
            self.session_id.to_hex_string()
        );
        self.apply_pool_state_and_skip_votes(listener, &session_boot.pool_state)?;

        // Step 5: Drain startup events (keep BroadcastVote only)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 5/12 - draining startup events",
            self.session_id.to_hex_string()
        );
        let kept_votes = listener.recovery_drain_startup_events();
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 5/12 complete - kept {} votes",
            self.session_id.to_hex_string(),
            kept_votes.len()
        );

        // Step 6: Seed current_round compatibility hook.
        //
        // Simplex now restores finalized state directly without historical recommit,
        // so this remains a compatibility callback for the recovery pipeline. The
        // slot-based SessionProcessor currently treats it as a no-op.
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 6/12 - seeding current_round=0 (finalized_blocks={})",
            self.session_id.to_hex_string(),
            session_boot.finalized_blocks.len()
        );
        listener.recovery_seed_current_round(0);

        // Step 7: Seed finalized_blocks set to prevent parent-chain walk into missing data
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 7/12 - seeding {} finalized blocks into tracking set",
            self.session_id.to_hex_string(),
            session_boot.finalized_blocks.len()
        );
        self.seed_finalized_blocks_set(listener, &session_boot.finalized_blocks);

        // Step 8: Notify last finalized block
        // C++ equivalent: consensus.cpp::load_from_db() publishes BlockFinalized(last, true)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 8/12 - notifying last finalized block",
            self.session_id.to_hex_string()
        );
        self.notify_last_finalized_block(listener, &session_boot.finalized_blocks);

        // Step 9: Restore receiver notar cert cache
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 9/12 - restoring {} notar certs to cache",
            self.session_id.to_hex_string(),
            receiver_boot.notar_certs.len()
        );
        self.restore_notar_cert_cache(listener, &receiver_boot.notar_certs)?;

        // Step 9b: Seed notarized candidates into received_candidates for parent resolution
        //
        // This ensures post-restart collation can resolve `BlockIdExt` for notarized parents
        // without relying on `requestCandidate` (which may be impossible in single-node tests).
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 9b/12 - seeding {} candidate infos into received_candidates for parent resolution",
            self.session_id.to_hex_string(),
            self.candidate_info_map.len()
        );
        self.seed_candidate_infos_for_parent_resolution(listener);

        // Step 10: Restore receiver candidate bytes cache
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 10/12 - restoring candidate bytes cache",
            self.session_id.to_hex_string()
        );
        self.restore_candidate_cache(
            listener,
            &session_boot.finalized_blocks,
            &candidate_payloads,
        )?;

        // Step 10b: Rebuild receiver standstill caches (votes + cert bundles + last_final_cert)
        //
        // C++ pool.cpp `alarm()` re-broadcasts:
        // - last_final_cert_
        // - per-slot certificate bundles in tracked range
        // - local votes not already covered by certificates
        //
        // Rust receiver caches are not persisted, so rebuild them from restored pool state here.
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 10b/12 - restoring receiver standstill caches",
            self.session_id.to_hex_string()
        );
        listener.recovery_restore_receiver_standstill_cache(&session_boot.votes);

        // Step 11: Restore kept votes
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 11/12 - restoring {} kept votes",
            self.session_id.to_hex_string(),
            kept_votes.len()
        );
        listener.recovery_restore_startup_votes(kept_votes);

        // Step 12: Finalize parent chain setup
        // IMPORTANT: This must happen AFTER step 11 (kept votes restoration) because
        // the kept votes may finalize additional slots, advancing first_non_finalized_slot.
        // We need to set available_base for the CURRENT first_non_finalized_slot, not the
        // one from the DB (which was outdated).
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 12/12 - finalizing parent chain setup",
            self.session_id.to_hex_string()
        );
        listener.recovery_finalize_parent_chain();

        log::info!(
            target: LOG_TARGET,
            "Session {}: bootstrap recovery complete",
            self.session_id.to_hex_string()
        );

        Ok(())
    }

    /// Seed notarized candidates into `received_candidates` for parent resolution.
    ///
    /// Uses `candidate_info_map` to reconstruct minimal metadata (BlockIdExt + parent id + hash data bytes)
    /// for candidates that have a stored NotarCert record.
    fn seed_candidate_infos_for_parent_resolution(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
    ) {
        let mut seeded = 0usize;
        let mut serialize_errors = 0usize;

        for candidate_info in self.candidate_info_map.values() {
            let candidate_id = candidate_info.candidate_id.clone();

            // Determine block_id, parent and empty-ness from CandidateHashData.
            let (block_id, parent, is_empty) = match &candidate_info.candidate_hash_data {
                CandidateHashData::Consensus_CandidateHashDataEmpty(empty) => {
                    let parent = RawCandidateId {
                        slot: SlotIndex::new(empty.parent.slot as u32),
                        hash: empty.parent.hash.clone(),
                    };
                    (empty.block.clone(), Some(parent), true)
                }
                CandidateHashData::Consensus_CandidateHashDataOrdinary(ordinary) => {
                    let parent = Self::extract_parent_id_from_ordinary_hash_data(
                        &candidate_info.candidate_hash_data,
                    )
                    .ok()
                    .flatten()
                    .map(|(slot, hash)| RawCandidateId { slot, hash });
                    (ordinary.block.clone(), parent, false)
                }
            };

            // Serialize CandidateHashData bytes (used by commit/signature contexts and DB parity).
            let candidate_hash_data_bytes = match serialize_boxed(
                &candidate_info.candidate_hash_data,
            ) {
                Ok(bytes) => bytes,
                Err(e) => {
                    serialize_errors += 1;
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: seed_candidate_infos_for_parent_resolution: failed to serialize CandidateHashData for slot={} hash={}: {}",
                        self.session_id.to_hex_string(),
                        candidate_id.slot.value(),
                        &candidate_id.hash.to_hex_string()[..8],
                        e
                    );
                    continue;
                }
            };

            let leader_idx = ValidatorIndex(candidate_info.leader_idx);
            listener.recovery_seed_candidate_for_parent_resolution(
                candidate_id,
                leader_idx,
                block_id,
                parent,
                is_empty,
                candidate_hash_data_bytes,
            );
            seeded += 1;
        }

        log::debug!(
            target: LOG_TARGET,
            "Session {}: seed_candidate_infos_for_parent_resolution: seeded={}, serialize_errors={}",
            self.session_id.to_hex_string(),
            seeded,
            serialize_errors
        );
    }

    /// Replay ALL votes to restore global state (weights, certificates).
    fn replay_votes_global(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        votes: &[VoteRecord],
    ) -> Result<()> {
        let mut applied = 0u32;
        let mut skipped = 0u32;
        let mut failed = 0u32;

        for vote_record in votes {
            // Deserialize vote
            let (vote, signature) = match Self::deserialize_vote_record(vote_record) {
                Some((v, s)) => (v, s),
                None => {
                    failed += 1;
                    continue;
                }
            };

            // Create RawVoteData from serialized bytes
            let raw_vote = RawVoteData::from(vote_record.data.clone());

            // Replay through listener
            let result = listener.recovery_on_vote(vote_record.node_idx, vote, signature, raw_vote);

            match result {
                VoteResult::Applied => applied += 1,
                VoteResult::Duplicate | VoteResult::SlotAlreadyFinalized => skipped += 1,
                VoteResult::Misbehavior(_) | VoteResult::Rejected(_) => failed += 1,
            }
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: replayed {} votes ({} skipped, {} failed)",
            self.session_id.to_hex_string(),
            applied,
            skipped,
            failed
        );

        Ok(())
    }

    /// Set first_non_finalized_slot from finalized blocks.
    fn apply_finalized_boundary(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        finalized_blocks: &[FinalizedBlockRecord],
    ) -> Result<()> {
        if finalized_blocks.is_empty() {
            return Ok(());
        }

        let max_slot =
            finalized_blocks.iter().map(|b| b.candidate_id.slot).max().unwrap_or(SlotIndex(0));

        let first_non_finalized = max_slot + 1;
        listener.recovery_set_first_non_finalized_slot(first_non_finalized);

        log::info!(
            target: LOG_TARGET,
            "Session {}: set first_non_finalized_slot={} (max finalized={})",
            self.session_id.to_hex_string(),
            first_non_finalized.value(),
            max_slot.value()
        );

        Ok(())
    }

    /// Apply local vote flags for OUR votes only.
    fn apply_local_vote_flags(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        votes: &[VoteRecord],
    ) -> Result<()> {
        let mut our_votes = 0u32;

        for vote_record in votes {
            // Only process OUR votes
            if vote_record.node_idx != self.self_idx {
                continue;
            }

            // Deserialize vote
            let (vote, _signature) = match Self::deserialize_vote_record(vote_record) {
                Some(v) => v,
                None => continue,
            };

            listener.recovery_mark_slot_voted_on_restart(&vote);
            our_votes += 1;
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: applied local flags for {} of our votes",
            self.session_id.to_hex_string(),
            our_votes
        );

        Ok(())
    }

    /// Set first_nonannounced_window and generate restart skip votes.
    fn apply_pool_state_and_skip_votes(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        pool_state: &Option<PoolStateRecord>,
    ) -> Result<()> {
        let first_nonannounced_window =
            pool_state.as_ref().map(|p| p.first_nonannounced_window).unwrap_or_default();

        // Set first_nonannounced_window in SessionProcessor
        listener.recovery_set_first_nonannounced_window(first_nonannounced_window);

        if first_nonannounced_window.value() == 0 {
            return Ok(());
        }

        // Generate skip votes for windows before first_nonannounced_window
        let skip_count = listener.recovery_generate_restart_skip_votes();

        log::info!(
            target: LOG_TARGET,
            "Session {}: generated {} restart skip votes for window {}",
            self.session_id.to_hex_string(),
            skip_count,
            first_nonannounced_window.value()
        );

        Ok(())
    }

    /// Restore notarization certificate cache in receiver AND seed into simplex_state.
    ///
    /// This does two things:
    /// 1. Cache raw bytes in receiver for network queries (`requestCandidate(want_notar=true)`)
    /// 2. Parse and seed into simplex_state for restored certificate state
    fn restore_notar_cert_cache(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        notar_certs: &[NotarCertRecord],
    ) -> Result<()> {
        let mut parsed_count = 0u32;
        let mut parse_errors = 0u32;

        for cert in notar_certs {
            // 1. Cache raw bytes in receiver for network queries
            listener.recovery_cache_notarization_cert(
                cert.candidate_id.slot,
                cert.candidate_id.hash.clone(),
                cert.notar_cert_bytes.to_vec(),
            );

            // 2. Parse and seed into simplex_state for restored certificate state
            match crate::certificate::NotarCert::from_tl_bytes_for_candidate(
                &cert.notar_cert_bytes,
                cert.candidate_id.slot,
                cert.candidate_id.hash.clone(),
            ) {
                Ok(parsed) => {
                    listener.recovery_seed_notarize_certificate(
                        cert.candidate_id.slot,
                        cert.candidate_id.hash.clone(),
                        Arc::new(parsed),
                    );
                    parsed_count += 1;
                }
                Err(e) => {
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: failed to parse notar cert for slot={} hash={}: {}",
                        self.session_id.to_hex_string(),
                        cert.candidate_id.slot.value(),
                        &cert.candidate_id.hash.to_hex_string()[..8],
                        e
                    );
                    parse_errors += 1;
                }
            }
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: restored {} notar certs to receiver cache, {} parsed to simplex_state, {} parse errors",
            self.session_id.to_hex_string(),
            notar_certs.len(),
            parsed_count,
            parse_errors
        );

        Ok(())
    }

    /// Seed finalized blocks into SessionProcessor's tracking set.
    ///
    /// This prevents `collect_parent_chain` from walking into missing `received_candidates`
    /// after restart. The walk stops when it hits a block in `finalized_blocks`.
    fn seed_finalized_blocks_set(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        finalized_blocks: &[FinalizedBlockRecord],
    ) {
        if finalized_blocks.is_empty() {
            log::debug!(
                target: LOG_TARGET,
                "Session {}: no finalized blocks to seed",
                self.session_id.to_hex_string()
            );
            return;
        }

        for block in finalized_blocks {
            let slot = block.candidate_id.slot;
            let block_hash = block.candidate_id.hash.clone();

            log::trace!(
                target: LOG_TARGET,
                "Session {}: seeding finalized block slot={}, hash={}",
                self.session_id.to_hex_string(),
                slot.value(),
                block_hash.to_hex_string()
            );

            listener.recovery_seed_finalized_block(slot, block_hash);
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: seeded {} finalized blocks into tracking set",
            self.session_id.to_hex_string(),
            finalized_blocks.len()
        );
    }

    /// Notify about the last finalized block (Phase 6.5a).
    ///
    /// C++ equivalent: `consensus.cpp::load_from_db()` publishes
    /// `BlockFinalized(last_known_finalized_block, true)` after loading.
    ///
    /// This notification seeds ALL finalized blocks into `received_candidates`
    /// for parent resolution, then notifies about the last finalized block.
    fn notify_last_finalized_block(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        finalized_blocks: &[FinalizedBlockRecord],
    ) {
        // First, seed ALL finalized blocks into received_candidates for parent resolution
        listener.recovery_seed_received_candidates(finalized_blocks);

        // Find the last block with is_final=true
        // Iterate in reverse since the last one is typically at the end
        let last_final = finalized_blocks.iter().rev().find(|block| block.is_final);

        match last_final {
            Some(block) => {
                let slot = block.candidate_id.slot;
                let block_hash = block.candidate_id.hash.clone();
                let seqno = block.block_id.seq_no();

                log::info!(
                    target: LOG_TARGET,
                    "Session {}: notifying last finalized block on restart: slot={}, seqno={}, hash={}",
                    self.session_id.to_hex_string(),
                    slot.value(),
                    seqno,
                    block_hash.to_hex_string()
                );

                listener.recovery_notify_last_finalized(slot, block_hash, seqno);
            }
            None => {
                log::debug!(
                    target: LOG_TARGET,
                    "Session {}: no is_final=true block found, skipping last-finalized-cert notification",
                    self.session_id.to_hex_string()
                );
            }
        }
    }

    /// Restore candidate bytes cache in receiver.
    ///
    /// For each finalized block, reconstructs the CandidateData bytes and caches
    /// them so `requestCandidate(want_candidate=true)` queries can be answered.
    ///
    /// Reference: C++ candidate-resolver.cpp loads full candidate bytes from its
    /// own consensus DB. The Rust implementation only reconstructs empty blocks
    /// from metadata; non-empty blocks are skipped and will be resolved via peer
    /// overlay when requested.
    ///
    /// # Empty vs Non-empty blocks
    ///
    /// - **Empty blocks**: Reconstruct `CandidateData::Consensus_Empty` from FinalizedBlockRecord
    ///   (block_id, parent info, signature from leader)
    /// - **Non-empty blocks**: Skipped (will be served from in-memory cache during
    ///   normal operation, or peers will query other validators)
    fn restore_candidate_cache(
        &self,
        listener: &mut dyn SessionStartupRecoveryListener,
        finalized_blocks: &[FinalizedBlockRecord],
        candidate_payloads: &[(RawCandidateId, Vec<u8>)],
    ) -> Result<()> {
        let mut restored_empty = 0u32;
        let mut restored_payload = 0u32;
        let mut skipped = 0u32;
        let mut errors = 0u32;

        // 1. Restore from persisted candidate payloads (both empty and non-empty).
        //    C++ parity: CandidateResolver loads full candidate bytes from DB.
        let payload_ids: HashSet<_> = candidate_payloads.iter().map(|(id, _)| id.clone()).collect();
        for (id, bytes) in candidate_payloads {
            listener.recovery_cache_candidate_bytes(id.slot, id.hash.clone(), bytes.clone());
            restored_payload += 1;
        }

        // 2. For finalized empty blocks not already covered by payloads,
        //    reconstruct from metadata (backward-compat for DBs without payloads).
        for block in finalized_blocks {
            let slot = block.candidate_id.slot;
            let candidate_hash = &block.candidate_id.hash;

            if payload_ids.contains(&block.candidate_id) {
                continue;
            }

            let candidate_info = match self.candidate_info_map.get(candidate_hash) {
                Some(info) => info,
                None => {
                    log::trace!(
                        target: LOG_TARGET,
                        "Session {}: no candidate info for slot={}, skipping cache restore",
                        self.session_id.to_hex_string(),
                        slot.value()
                    );
                    skipped += 1;
                    continue;
                }
            };

            let is_empty =
                Self::is_empty_block_candidate_hash_data(&candidate_info.candidate_hash_data);

            if !is_empty {
                skipped += 1;
                continue;
            }

            let candidate_data_bytes =
                match self.reconstruct_empty_candidate_data(block, candidate_info) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log::warn!(
                            target: LOG_TARGET,
                            "Session {}: failed to reconstruct empty candidate for slot={}: {}",
                            self.session_id.to_hex_string(),
                            slot.value(),
                            e
                        );
                        errors += 1;
                        continue;
                    }
                };

            listener.recovery_cache_candidate_bytes(
                slot,
                candidate_hash.clone(),
                candidate_data_bytes,
            );
            restored_empty += 1;
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: restored candidate cache: {} from payload DB, {} empty reconstructed, \
            {} skipped, {} errors",
            self.session_id.to_hex_string(),
            restored_payload,
            restored_empty,
            skipped,
            errors
        );

        Ok(())
    }

    /// Check if candidate_hash_data represents an empty block.
    ///
    /// Empty blocks use `candidateHashDataEmpty` TL type, non-empty use `candidateHashDataOrdinary`.
    fn is_empty_block_candidate_hash_data(candidate_hash_data: &CandidateHashData) -> bool {
        matches!(candidate_hash_data, CandidateHashData::Consensus_CandidateHashDataEmpty(_))
    }

    /// Reconstruct CandidateData::Consensus_Empty bytes for an empty block.
    ///
    /// Reference: C++ RawCandidate::serialize() for empty variant
    fn reconstruct_empty_candidate_data(
        &self,
        block: &FinalizedBlockRecord,
        candidate_info: &CandidateInfoRecord,
    ) -> Result<Vec<u8>> {
        let slot = block.candidate_id.slot;

        // Get parent candidate id from candidate_hash_data
        // For empty blocks, parent is in the candidateHashDataEmpty structure
        let parent_id =
            Self::extract_parent_id_from_empty_hash_data(&candidate_info.candidate_hash_data)?;

        // Use signature from candidate_info (leader's original signature)
        let signature = candidate_info.signature.clone();

        // Build TL parent CandidateId (boxed enum)
        let parent = CandidateId { slot: parent_id.0.value() as i32, hash: parent_id.1.clone() }
            .into_boxed();

        let tl_empty = CandidateDataEmpty {
            slot: slot.value() as i32,
            parent,
            block: block.block_id.clone(),
            signature,
        };

        let candidate_data = CandidateData::Consensus_Empty(tl_empty);

        // Serialize
        let bytes = serialize_boxed(&candidate_data)
            .map_err(|e| error!("Failed to serialize empty CandidateData: {}", e))?;

        log::trace!(
            target: LOG_TARGET,
            "Session {}: reconstructed empty CandidateData for slot={} ({}B)",
            self.session_id.to_hex_string(),
            slot.value(),
            bytes.len()
        );

        Ok(bytes)
    }

    /// Extract parent (slot, hash) from candidateHashDataEmpty.
    fn extract_parent_id_from_empty_hash_data(
        candidate_hash_data: &CandidateHashData,
    ) -> Result<(SlotIndex, UInt256)> {
        match candidate_hash_data {
            CandidateHashData::Consensus_CandidateHashDataEmpty(empty) => {
                // parent is candidateid::CandidateId struct with fields slot and hash
                let slot = SlotIndex(empty.parent.slot as u32);
                let hash = empty.parent.hash.clone();
                Ok((slot, hash))
            }
            CandidateHashData::Consensus_CandidateHashDataOrdinary(_) => {
                Err(error!("Expected empty hash data, got ordinary"))
            }
        }
    }

    /// Extract parent (slot, hash) from candidateHashDataOrdinary.
    ///
    /// Returns `None` if no parent (genesis/first block).
    fn extract_parent_id_from_ordinary_hash_data(
        candidate_hash_data: &CandidateHashData,
    ) -> Result<Option<(SlotIndex, UInt256)>> {
        match candidate_hash_data {
            CandidateHashData::Consensus_CandidateHashDataOrdinary(ordinary) => {
                // parent field is CandidateParent enum
                match ordinary.parent.id() {
                    None => Ok(None),
                    Some(id) => {
                        let slot = SlotIndex(*id.slot() as u32);
                        let hash = id.hash().clone();
                        Ok(Some((slot, hash)))
                    }
                }
            }
            CandidateHashData::Consensus_CandidateHashDataEmpty(_) => {
                Err(error!("Expected ordinary hash data, got empty"))
            }
        }
    }

    /// Deserialize a vote record into (Vote, SignatureBytes).
    fn deserialize_vote_record(vote_record: &VoteRecord) -> Option<(Vote, SignatureBytes)> {
        let msg = match deserialize_boxed(vote_record.data.as_slice()) {
            Ok(m) => m,
            Err(e) => {
                log::error!(
                    target: LOG_TARGET,
                    "Failed to deserialize vote: {}",
                    e
                );
                return None;
            }
        };

        let tl_vote = match msg.downcast::<TlVoteBoxed>() {
            Ok(v) => v,
            Err(_) => return None,
        };

        match extract_vote_and_signature(&tl_vote) {
            Ok((vote, signature)) => Some((vote, signature.to_vec())),
            Err(e) => {
                log::error!(
                    target: LOG_TARGET,
                    "Failed to extract vote and signature: {}",
                    e
                );
                None
            }
        }
    }
}
