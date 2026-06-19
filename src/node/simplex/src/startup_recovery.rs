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
//! │     - vote replay (ordered passes)                              │
//! │     - set finalized boundary + apply local flags                │
//! │     - restore receiver caches                                   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Components
//!
//! - [`StartupRecoveryBackend`]: Object-safe accessor trait for the kernel substates
//! - [`SessionStartupRecoveryProcessor`]: Coordinator that loads bootstrap and drives recovery
//!
//! See the startup recovery section in crate-level docs for design details.

use crate::{
    block::{CandidateParentInfo, RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    candidate_book::{CandidateBook, ReceivedCandidate},
    certificate::{Certificate as SimplexCertificate, FinalCertPtr, NotarCertPtr, SkipCertPtr},
    consensus_controller::ConsensusController,
    database::{
        Bootstrap, CandidateInfoRecord, FinalizedBlockRecord, NotarCertRecord, PoolStateRecord,
        VoteRecord,
    },
    database_controller::DatabaseController,
    misbehavior::VoteResult,
    receiver::{ReceiverPtr, StandstillCertificateType},
    session_description::SessionDescription,
    simplex_state::{SimplexEvent, SimplexState, Vote},
    utils::extract_vote_and_signature,
    RawVoteData, SessionId,
};
use std::{
    collections::{HashMap, HashSet},
    mem::discriminant,
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

// ======================================================================
// StartupRecoveryBackend — accessor seam for recovery state mutation
// ======================================================================

/// Object-safe accessor trait exposing the kernel substates that startup
/// recovery mutates while replaying bootstrap.
///
/// Implemented by `SessionProcessor`; [`SessionStartupRecoveryProcessor`] holds a
/// `&mut dyn StartupRecoveryBackend` for the duration of `apply_bootstrap` and
/// reaches `simplex_state` / `receiver` / `consensus` / `candidate_book` /
/// `database` through it. The recovery processor owns the session `description`
/// itself, so the backend only surfaces the mutable substates plus two effects
/// (`increment_error` and the post-restore standstill sync).
///
/// Unlike [`crate::consensus_controller::ConsensusBackend`] no borrowing adapter
/// is required: recovery runs once at bootstrap with a full `&mut SessionProcessor`,
/// so `SessionProcessor` implements this trait directly and every restore step
/// touches a single substate at a time (sequential re-borrows, NLL releases
/// between statements).
pub(crate) trait StartupRecoveryBackend {
    /// Borrow the consensus FSM state (reads during restore + parent-chain setup).
    fn simplex_state(&self) -> &SimplexState;

    /// Mutably borrow the consensus FSM state for vote/cert replay and seeding.
    fn simplex_state_mut(&mut self) -> &mut SimplexState;

    /// Shared receiver handle for cache restoration (notar/candidate bytes,
    /// standstill bundles, ingress cursor).
    fn receiver(&self) -> &ReceiverPtr;

    /// Mutably borrow the finalization controller (finalized-block journal,
    /// delivery dedup, finalized-head cursor seeding).
    fn consensus_mut(&mut self) -> &mut ConsensusController;

    /// Borrow the received-candidate book (parent lookups during chain setup).
    fn candidate_book(&self) -> &CandidateBook;

    /// Mutably borrow the received-candidate book (parent-resolution seeding).
    fn candidate_book_mut(&mut self) -> &mut CandidateBook;

    /// Borrow the database controller (read `first_nonannounced_window`).
    fn database(&self) -> &DatabaseController;

    /// Mutably borrow the database controller (set `first_nonannounced_window`).
    fn database_mut(&mut self) -> &mut DatabaseController;

    /// Bump the session error counter on a restore failure path
    /// (cert conflict / serialize / vote-parse errors).
    fn increment_error(&self);

    /// Run the post-restore standstill sync: range-clamp the receiver ingress
    /// cursor to the tracked interval and reschedule the standstill timer. Kept
    /// as an effect because it spans `SessionProcessor`-level helper state.
    fn sync_standstill_after_restore(&mut self);
}

// ======================================================================
// SessionStartupRecoveryProcessor — bootstrap loader / recovery driver
// ======================================================================

/// Session startup recovery processor.
///
/// Coordinates the startup recovery stage:
/// 1. Loads bootstrap from DB (in constructor, cancellable)
/// 2. Computes recovery identity (self_idx, validator keys)
/// 3. Builds restore plans
/// 4. Drives recovery, mutating kernel state through `StartupRecoveryBackend`
///
/// Dropped before entering the main processing loop.
pub(crate) struct SessionStartupRecoveryProcessor {
    /// Session ID for logging
    session_id: SessionId,

    /// Session description: owned source of session id / shard / self index /
    /// leader-window options / `now` (`get_time`) consumed by the relocated
    /// restore steps (was held on `SessionProcessor`).
    description: Arc<SessionDescription>,

    /// Self validator index (cached from description)
    self_idx: ValidatorIndex,

    /// Loaded bootstrap data (consumed during apply_bootstrap)
    bootstrap: Option<Bootstrap>,

    /// Pre-built candidate info map (candidate_hash -> CandidateInfoRecord)
    candidate_info_map: HashMap<CandidateHash, CandidateInfoRecord>,
}

// ======================================================================
// Construction & inspection
// ======================================================================
// Build the recovery processor from pre-loaded bootstrap and expose the
// read-only finalized-block counter.
impl SessionStartupRecoveryProcessor {
    /// Create a new recovery processor from pre-loaded bootstrap data.
    ///
    /// # Arguments
    ///
    /// * `session_id` - Session identifier
    /// * `description` - Session description (for self_idx, leader key lookup)
    /// * `bootstrap` - Pre-loaded bootstrap data
    ///
    /// # Returns
    ///
    /// * `Self` - Processor ready to apply bootstrap
    pub(crate) fn new(
        session_id: SessionId,
        description: Arc<SessionDescription>,
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

        Self { session_id, description, self_idx, bootstrap: Some(bootstrap), candidate_info_map }
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

    /// Get the number of finalized blocks in bootstrap.
    pub(crate) fn finalized_block_count(&self) -> usize {
        self.bootstrap.as_ref().map(|b| b.finalized_blocks.len()).unwrap_or(0)
    }
}

// ======================================================================
// Bootstrap application — replay → boundary → cert restore → cache seed
// ======================================================================
// `apply_bootstrap` is the ordered driver; the rest are its restore steps
// (vote replay, finalized boundary, cert replay, cache/candidate seeding).
impl SessionStartupRecoveryProcessor {
    /// Apply bootstrap state and run startup recovery.
    ///
    /// This method:
    /// 1. Replays votes (order: global pass, set boundary, local flags)
    /// 2. Generates restart skip votes
    /// 3. Drains startup events (keeps BroadcastVote only)
    /// 4. Restores receiver caches (notar certs, candidate bytes)
    /// 5. Restores kept votes
    ///
    /// After this method returns, the processor is consumed and can be dropped.
    ///
    /// # Arguments
    ///
    /// * `backend` - SessionProcessor implementing `StartupRecoveryBackend`
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Recovery completed successfully
    /// * `Err` - Recovery failed (e.g., candidate fetch timeout)
    pub(crate) fn apply_bootstrap(
        mut self,
        backend: &mut dyn StartupRecoveryBackend,
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

        self.recovery_begin_startup_replay(backend);

        // Split bootstrap into session, receiver, and candidate payload parts
        let (session_boot, receiver_boot, candidate_payloads) = bootstrap.split();

        // Step 1: Replay ALL votes (global pass - restores weights, certificates)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 1/12 - replaying {} votes (global pass)",
            self.session_id.to_hex_string(),
            session_boot.votes.len()
        );
        self.replay_votes_global(backend, &session_boot.votes)?;

        // Step 2: Set first_non_finalized_slot boundary
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 2/12 - setting finalized boundary from {} blocks",
            self.session_id.to_hex_string(),
            session_boot.finalized_blocks.len()
        );
        self.apply_finalized_boundary(backend, &session_boot.finalized_blocks)?;

        // Step 2b: Replay persisted FinalCert records before restart skip generation.
        //
        // C++ Pool replays saved certificates during startup and FinalCert replay advances
        // `first_nonfinalized_slot_`. Rust must do the same before it looks at the saved
        // `first_nonannounced_window`, otherwise a filtered/truncated finalized-block table
        // can make recovery emit skip votes for slots that already have FinalCerts.
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 2b/12 - restoring {} final certificates",
            self.session_id.to_hex_string(),
            receiver_boot.final_certs.len()
        );
        self.restore_final_cert_state(backend, &receiver_boot.final_certs)?;

        // Step 2c: Replay persisted SkipCert records before restart skip generation.
        //
        // C++ Pool replays saved certificates during startup and SkipCert replay advances
        // the present/progress cursor over skipped slots. Rust must restore that cursor
        // before accepting live ingress; otherwise a restarted validator may reject rescue
        // candidates as too_far_ahead even though the skipped-slot progress is in DB.
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 2c/12 - restoring {} skip certificates",
            self.session_id.to_hex_string(),
            receiver_boot.skip_certs.len()
        );
        self.restore_skip_cert_state(backend, &receiver_boot.skip_certs)?;

        // Step 3: Apply local vote flags (prevents double-voting)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 3/12 - applying local vote flags",
            self.session_id.to_hex_string()
        );
        self.apply_local_vote_flags(backend, &session_boot.votes)?;

        // Step 4: Set first_nonannounced_window and generate restart skip votes
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 4/12 - applying pool state and generating skip votes",
            self.session_id.to_hex_string()
        );
        self.apply_pool_state_and_skip_votes(backend, &session_boot.pool_state)?;

        // Step 5: Drain startup events (keep BroadcastVote only)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 5/12 - draining startup events",
            self.session_id.to_hex_string()
        );
        let kept_votes = self.recovery_drain_startup_events(backend);
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
        self.recovery_seed_current_round(0);

        // Step 7: Seed finalized_blocks set to prevent parent-chain walk into missing data
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 7/12 - seeding {} finalized blocks into tracking set",
            self.session_id.to_hex_string(),
            session_boot.finalized_blocks.len()
        );
        self.seed_finalized_blocks_set(backend, &session_boot.finalized_blocks);

        // Step 8: Notify last finalized block
        // C++ equivalent: consensus.cpp::load_from_db() publishes BlockFinalized(last, true)
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 8/12 - notifying last finalized block",
            self.session_id.to_hex_string()
        );
        self.notify_last_finalized_block(backend, &session_boot.finalized_blocks);

        // Step 9: Restore receiver notar cert cache
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 9/12 - restoring {} notar certs to cache",
            self.session_id.to_hex_string(),
            receiver_boot.notar_certs.len()
        );
        self.restore_notar_cert_cache(backend, &receiver_boot.notar_certs)?;

        // Step 9b: Seed notarized candidates into `received_candidates` for post-restart lookups.
        //
        // This ensures post-restart collation can resolve `BlockIdExt` for notarized parents
        // without relying on `requestCandidate` (which may be impossible in single-node tests).
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 9b/12 - seeding {} candidate infos into received_candidates for post-restart parent/tip lookups",
            self.session_id.to_hex_string(),
            self.candidate_info_map.len()
        );
        self.seed_candidate_infos_for_parent_resolution(backend);

        // Step 10: Restore receiver candidate bytes cache
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 10/12 - restoring candidate bytes cache",
            self.session_id.to_hex_string()
        );
        self.restore_candidate_cache(backend, &session_boot.finalized_blocks, &candidate_payloads)?;

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
        self.recovery_restore_receiver_standstill_cache(backend, &session_boot.votes);

        // Step 11: Restore kept votes
        log::debug!(
            target: LOG_TARGET,
            "Session {}: step 11/12 - restoring {} kept votes",
            self.session_id.to_hex_string(),
            kept_votes.len()
        );
        self.recovery_restore_startup_votes(backend, kept_votes);

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
        self.recovery_finalize_parent_chain(backend);

        log::info!(
            target: LOG_TARGET,
            "Session {}: bootstrap recovery complete",
            self.session_id.to_hex_string()
        );

        Ok(())
    }

    /// Restore skip certificates into SimplexState before restart skips.
    fn restore_skip_cert_state(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        skip_certs: &[SkipCertRecord],
    ) -> Result<()> {
        let mut parsed_count = 0u32;
        let mut skipped_count = 0u32;

        for cert in skip_certs {
            let tl_cert: CertificateBoxed = match deserialize_typed(cert.cert_bytes.as_slice()) {
                Ok(tl_cert) => tl_cert,
                Err(e) => {
                    skipped_count += 1;
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: failed to deserialize skip cert for slot={}: {}",
                        self.session_id.to_hex_string(),
                        cert.slot.value(),
                        e
                    );
                    continue;
                }
            };

            let parsed = match SimplexCertificate::<Vote>::from_tl(
                &tl_cert,
                self.description.as_ref(),
                &self.session_id,
            ) {
                Ok(parsed) => parsed,
                Err(e) => {
                    skipped_count += 1;
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: failed to verify skip cert for slot={}: {}",
                        self.session_id.to_hex_string(),
                        cert.slot.value(),
                        e
                    );
                    continue;
                }
            };

            let Vote::Skip(skip_vote) = parsed.vote else {
                skipped_count += 1;
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: persisted skip cert record decoded as non-skip vote for key=s{}",
                    self.session_id.to_hex_string(),
                    cert.slot.value(),
                );
                continue;
            };

            if skip_vote.slot != cert.slot {
                skipped_count += 1;
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: persisted skip cert slot mismatch key=s{} vote=s{}",
                    self.session_id.to_hex_string(),
                    cert.slot.value(),
                    skip_vote.slot.value(),
                );
                continue;
            }

            self.recovery_seed_skip_certificate(
                backend,
                skip_vote.slot,
                Arc::new(SimplexCertificate { vote: skip_vote, signatures: parsed.signatures }),
            );
            parsed_count += 1;
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: restored {} skip certs to simplex_state, {} skipped",
            self.session_id.to_hex_string(),
            parsed_count,
            skipped_count,
        );

        Ok(())
    }

    /// Restore full finalization certificates into SimplexState before restart skips.
    fn restore_final_cert_state(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        final_certs: &[FinalCertRecord],
    ) -> Result<()> {
        let mut parsed_count = 0u32;
        let mut skipped_count = 0u32;

        for cert in final_certs {
            let tl_cert: CertificateBoxed = match deserialize_typed(cert.cert_bytes.as_slice()) {
                Ok(tl_cert) => tl_cert,
                Err(e) => {
                    skipped_count += 1;
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: failed to deserialize final cert for slot={} hash={}: {}",
                        self.session_id.to_hex_string(),
                        cert.candidate_id.slot.value(),
                        &cert.candidate_id.hash.to_hex_string()[..8],
                        e
                    );
                    continue;
                }
            };

            let parsed = match SimplexCertificate::<Vote>::from_tl(
                &tl_cert,
                self.description.as_ref(),
                &self.session_id,
            ) {
                Ok(parsed) => parsed,
                Err(e) => {
                    skipped_count += 1;
                    log::warn!(
                        target: LOG_TARGET,
                        "Session {}: failed to verify final cert for slot={} hash={}: {}",
                        self.session_id.to_hex_string(),
                        cert.candidate_id.slot.value(),
                        &cert.candidate_id.hash.to_hex_string()[..8],
                        e
                    );
                    continue;
                }
            };

            let Vote::Finalize(final_vote) = parsed.vote else {
                skipped_count += 1;
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: persisted final cert record decoded as non-finalize vote \
                    for key=s{}:{}",
                    self.session_id.to_hex_string(),
                    cert.candidate_id.slot.value(),
                    &cert.candidate_id.hash.to_hex_string()[..8],
                );
                continue;
            };

            if final_vote.slot != cert.candidate_id.slot
                || final_vote.block_hash != cert.candidate_id.hash
            {
                skipped_count += 1;
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: final cert key mismatch: key=s{}:{} cert=s{}:{}",
                    self.session_id.to_hex_string(),
                    cert.candidate_id.slot.value(),
                    &cert.candidate_id.hash.to_hex_string()[..8],
                    final_vote.slot.value(),
                    &final_vote.block_hash.to_hex_string()[..8],
                );
                continue;
            }

            let slot = final_vote.slot;
            let block_hash = final_vote.block_hash.clone();
            let final_cert = Arc::new(SimplexCertificate::new(final_vote, parsed.signatures));
            self.recovery_seed_finalize_certificate(backend, slot, block_hash, final_cert);
            parsed_count += 1;
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: restored {} final certs to simplex_state ({} skipped)",
            self.session_id.to_hex_string(),
            parsed_count,
            skipped_count
        );

        Ok(())
    }

    /// Seed notarized candidates into `received_candidates` for post-restart parent/tip lookups.
    ///
    /// Uses `candidate_info_map` to reconstruct minimal metadata (BlockIdExt + parent id + hash data bytes)
    /// for candidates that have a stored NotarCert record.
    fn seed_candidate_infos_for_parent_resolution(&self, backend: &mut dyn StartupRecoveryBackend) {
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
            self.recovery_seed_candidate_for_parent_resolution(
                backend,
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
        backend: &mut dyn StartupRecoveryBackend,
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

            // Replay through the backend
            let result =
                self.recovery_on_vote(backend, vote_record.node_idx, vote, signature, raw_vote);

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
        backend: &mut dyn StartupRecoveryBackend,
        finalized_blocks: &[FinalizedBlockRecord],
    ) -> Result<()> {
        if finalized_blocks.is_empty() {
            return Ok(());
        }

        let max_slot =
            finalized_blocks.iter().map(|b| b.candidate_id.slot).max().unwrap_or(SlotIndex(0));

        let first_non_finalized = max_slot + 1;
        self.recovery_set_first_non_finalized_slot(backend, first_non_finalized);

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
        backend: &mut dyn StartupRecoveryBackend,
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

            self.recovery_mark_slot_voted_on_restart(backend, &vote);
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
        backend: &mut dyn StartupRecoveryBackend,
        pool_state: &Option<PoolStateRecord>,
    ) -> Result<()> {
        let first_nonannounced_window =
            pool_state.as_ref().map(|p| p.first_nonannounced_window).unwrap_or_default();

        // Set first_nonannounced_window in SessionProcessor
        self.recovery_set_first_nonannounced_window(backend, first_nonannounced_window);

        if first_nonannounced_window.value() == 0 {
            return Ok(());
        }

        // Generate skip votes for windows before first_nonannounced_window
        let skip_count = self.recovery_generate_restart_skip_votes(backend);

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
        backend: &mut dyn StartupRecoveryBackend,
        notar_certs: &[NotarCertRecord],
    ) -> Result<()> {
        let mut parsed_count = 0u32;
        let mut parse_errors = 0u32;

        for cert in notar_certs {
            // 1. Cache raw bytes in receiver for network queries
            self.recovery_cache_notarization_cert(
                backend,
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
                    self.recovery_seed_notarize_certificate(
                        backend,
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
        backend: &mut dyn StartupRecoveryBackend,
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

            self.recovery_seed_finalized_block(backend, slot, block_hash);
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: seeded {} finalized blocks into tracking set",
            self.session_id.to_hex_string(),
            finalized_blocks.len()
        );
    }

    /// Notify about the last finalized block.
    ///
    /// C++ equivalent: `consensus.cpp::load_from_db()` publishes
    /// `BlockFinalized(last_known_finalized_block, true)` after loading.
    ///
    /// This notification seeds ALL finalized blocks into `received_candidates`
    /// for restart-side parent/tip lookups, then notifies about the last finalized block.
    fn notify_last_finalized_block(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        finalized_blocks: &[FinalizedBlockRecord],
    ) {
        // First, seed ALL finalized blocks into `received_candidates` for restart-side lookups.
        self.recovery_seed_received_candidates(backend, finalized_blocks);

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

                self.recovery_notify_last_finalized(backend, slot, block_hash, seqno);
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
        backend: &mut dyn StartupRecoveryBackend,
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
            self.recovery_cache_candidate_bytes(backend, id.slot, id.hash.clone(), bytes.clone());
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

            self.recovery_cache_candidate_bytes(
                backend,
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
}

// ======================================================================
// Hash-data & vote-record decoding helpers
// ======================================================================
// Pure decoders over TL `CandidateHashData` / `VoteRecord`: empty-block
// detection, empty-candidate reconstruction, parent-id extraction, and
// vote deserialization. No `backend` access.
impl SessionStartupRecoveryProcessor {
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

// ======================================================================
// Kernel-state mutation seam (run through `StartupRecoveryBackend`)
// ======================================================================
// Formerly `impl SessionStartupRecoveryListener for SessionProcessor`;
// these now live on the recovery processor and reach kernel substates via
// `backend`, with `description` / `session_id` / `now` from owned handles.
// Verbatim relocation — no control-flow change (only borrow-driven splits).
impl SessionStartupRecoveryProcessor {
    fn recovery_begin_startup_replay(&self, backend: &mut dyn StartupRecoveryBackend) {
        log::debug!(
            target: LOG_TARGET,
            "Session {}: entering startup replay mode",
            self.session_id.to_hex_string()
        );
        backend.simplex_state_mut().begin_startup_replay();
    }

    fn recovery_set_first_non_finalized_slot(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
    ) {
        log::trace!(
            "Session {}: recovery_set_first_non_finalized_slot({})",
            self.session_id.to_hex_string(),
            slot.value()
        );
        backend.simplex_state_mut().set_first_non_finalized_slot(slot);
        let (begin, _) = backend.simplex_state().get_tracked_slots_interval();
        let progress = backend.simplex_state().get_first_non_progressed_slot().value().max(begin);
        backend.receiver().set_ingress_slot_begin(begin);
        backend.receiver().set_ingress_progress_slot(progress);
    }

    fn recovery_on_vote(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        node_idx: ValidatorIndex,
        vote: Vote,
        signature: SignatureBytes,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        log::trace!(
            "Session {}: recovery_on_vote(node={}, vote={:?})",
            self.session_id.to_hex_string(),
            node_idx.value(),
            discriminant(&vote)
        );
        backend.simplex_state_mut().on_vote(
            self.description.as_ref(),
            node_idx,
            vote,
            signature,
            raw_vote,
        )
    }

    fn recovery_mark_slot_voted_on_restart(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        vote: &Vote,
    ) {
        let slot = match vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
        };
        log::trace!(
            "Session {}: recovery_mark_slot_voted_on_restart(slot={})",
            self.session_id.to_hex_string(),
            slot.value()
        );
        backend.simplex_state_mut().mark_slot_voted_on_restart(self.description.as_ref(), vote);
    }

    fn recovery_set_first_nonannounced_window(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        window: WindowIndex,
    ) {
        log::trace!(
            "Session {}: recovery_set_first_nonannounced_window({})",
            self.session_id.to_hex_string(),
            window.value()
        );
        backend.database_mut().set_first_nonannounced_window(window);
    }

    fn recovery_generate_restart_skip_votes(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
    ) -> usize {
        let window = backend.database().first_nonannounced_window();
        log::trace!(
            "Session {}: recovery_generate_restart_skip_votes(window={})",
            self.session_id.to_hex_string(),
            window.value()
        );
        let slots_per_window = self.description.opts().slots_per_leader_window;
        backend.simplex_state_mut().generate_restart_skip_votes(window, slots_per_window) as usize
    }

    fn recovery_drain_startup_events(&self, backend: &mut dyn StartupRecoveryBackend) -> Vec<Vote> {
        log::trace!("Session {}: recovery_drain_startup_events", self.session_id.to_hex_string());

        // Drain all events, keeping only BroadcastVote
        let mut kept_votes = Vec::new();
        let mut dropped_finalized = 0u32;
        let mut dropped_skipped = 0u32;
        let mut dropped_notarization = 0u32;
        let mut dropped_skip_cert_reached = 0u32;
        let mut dropped_finalization_reached = 0u32;

        while let Some(event) = backend.simplex_state_mut().pull_event() {
            match event {
                SimplexEvent::BroadcastVote(vote) => {
                    kept_votes.push(vote);
                }
                SimplexEvent::BlockFinalized(_) => {
                    dropped_finalized += 1;
                }
                SimplexEvent::SlotSkipped(_) => {
                    dropped_skipped += 1;
                }
                SimplexEvent::NotarizationReached(_) => {
                    dropped_notarization += 1;
                }
                SimplexEvent::SkipCertificateReached(_) => {
                    dropped_skip_cert_reached += 1;
                }
                SimplexEvent::FinalizationReached(_) => {
                    dropped_finalization_reached += 1;
                }
            }
        }

        log::info!(
            "Session {}: drained startup events: kept {} votes, dropped {dropped_finalized} \
            finalized, {dropped_skipped} skipped, {dropped_notarization} notarization, \
            {dropped_skip_cert_reached} skip_cert_reached, \
            {dropped_finalization_reached} finalization_reached",
            self.session_id.to_hex_string(),
            kept_votes.len(),
        );

        kept_votes
    }

    fn recovery_restore_startup_votes(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        votes: Vec<Vote>,
    ) {
        log::trace!(
            "Session {}: recovery_restore_startup_votes(count={})",
            self.session_id.to_hex_string(),
            votes.len()
        );

        // Push votes back to the front of the queue in reverse order
        // so they come out in the original order when pulled
        for vote in votes.into_iter().rev() {
            backend.simplex_state_mut().push_event_front(SimplexEvent::BroadcastVote(vote));
        }
    }

    fn recovery_seed_current_round(&self, round: u32) {
        // NOTE(Option B): current_round removed - round is now derived from slot at emit time.
        // This remains a no-op kept for the recovery pipeline (round=slot model).
        log::debug!(
            target: LOG_TARGET,
            "Session {}: recovery_seed_current_round({}) - no-op (round=slot model)",
            self.session_id.to_hex_string(),
            round
        );
    }

    fn recovery_seed_finalized_block(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        block_hash: CandidateHash,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "Session {}: seeding finalized block slot={}, hash={}",
            self.session_id.to_hex_string(),
            slot.value(),
            block_hash.to_hex_string()
        );

        backend.consensus_mut().insert_finalized_block(RawCandidateId { slot, hash: block_hash });
    }

    fn recovery_seed_received_candidates(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        finalized_blocks: &[FinalizedBlockRecord],
    ) {
        log::info!(
            target: LOG_TARGET,
            "Session {}: seeding {} finalized blocks into received_candidates for parent \
            resolution",
            self.session_id.to_hex_string(),
            finalized_blocks.len(),
        );

        for block in finalized_blocks {
            let slot = block.candidate_id.slot;
            let block_hash = block.candidate_id.hash.clone();
            let block_id = block.block_id.clone();
            let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
            let is_empty = block
                .parent
                .as_ref()
                .and_then(|parent_id| backend.candidate_book().received(parent_id))
                .is_some_and(|parent| parent.block_id == block_id);

            // Skip if already present (shouldn't happen, but be safe)
            if backend.candidate_book().contains_received(&candidate_id) {
                if !is_empty {
                    backend.consensus_mut().insert_finalized_delivery_sent(candidate_id.clone());
                    let seqno = block_id.seq_no();
                    if let Some(existing_block_id) =
                        backend.consensus_mut().finalized_delivery_sent_seqno_block_id(seqno)
                    {
                        if existing_block_id == block_id {
                            continue;
                        }
                        assert!(
                            false,
                            "Session {} protocol breach: duplicate finalized seqno={} while seeding \
                             existing_block_id={} new_block_id={}",
                            &self.session_id.to_hex_string()[..8],
                            seqno,
                            existing_block_id,
                            block_id,
                        );
                    }
                    backend
                        .consensus_mut()
                        .insert_finalized_delivery_sent_seqno(seqno, slot, block_id);
                }
                continue;
            }

            // Seed a minimal received candidate record for restart-side parent/tip lookups.
            backend.candidate_book_mut().insert_received(
                candidate_id.clone(),
                ReceivedCandidate {
                    slot,
                    source_idx: self.description.get_self_idx(),
                    candidate_hash_data_bytes: Vec::new(),
                    block_id: block_id.clone(),
                    root_hash: block_id.root_hash.clone(),
                    file_hash: block_id.file_hash.clone(),
                    data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
                    collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                        Vec::new(),
                    ),
                    gen_utime_ms: None,
                    receive_time: self.description.get_time(),
                    is_empty,
                    parent_id: block.parent.clone(),
                },
            );

            if !is_empty {
                backend.consensus_mut().insert_finalized_delivery_sent(candidate_id);
                let seqno = block_id.seq_no();
                if let Some(existing_block_id) =
                    backend.consensus_mut().finalized_delivery_sent_seqno_block_id(seqno)
                {
                    if existing_block_id == block_id {
                        continue;
                    }
                    assert!(
                        false,
                        "Session {} protocol breach: duplicate finalized seqno={} while seeding \
                         existing_block_id={} new_block_id={}",
                        &self.session_id.to_hex_string()[..8],
                        seqno,
                        existing_block_id,
                        block_id,
                    );
                }
                backend.consensus_mut().insert_finalized_delivery_sent_seqno(seqno, slot, block_id);
            }
        }

        log::debug!(
            target: LOG_TARGET,
            "Session {}: seeded {} received candidates",
            self.session_id.to_hex_string(),
            finalized_blocks.len()
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn recovery_seed_candidate_for_parent_resolution(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        candidate_id: RawCandidateId,
        leader_idx: ValidatorIndex,
        block_id: BlockIdExt,
        parent: Option<RawCandidateId>,
        is_empty: bool,
        candidate_hash_data_bytes: Vec<u8>,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "Session {}: recovery_seed_candidate_for_parent_resolution(slot=s{}, hash={}, \
            leader=v{:03}, parent={:?}, is_empty={is_empty})",
            &self.session_id.to_hex_string()[..8],
            candidate_id.slot.value(),
            &candidate_id.hash.to_hex_string()[..8],
            leader_idx.value(),
            parent.as_ref()
                .map(|p| format!("s{}:{}", p.slot.value(), &p.hash.to_hex_string()[..8])),
        );

        if let Some(existing) = backend.candidate_book_mut().received_mut(&candidate_id) {
            existing.source_idx = leader_idx;
            existing.candidate_hash_data_bytes = candidate_hash_data_bytes;
            existing.block_id.clone_from(&block_id);
            existing.root_hash.clone_from(&block_id.root_hash);
            existing.file_hash.clone_from(&block_id.file_hash);
            existing.gen_utime_ms = None;
            existing.is_empty = is_empty;
            existing.parent_id = parent;
            return;
        }

        backend.candidate_book_mut().insert_received(
            candidate_id.clone(),
            ReceivedCandidate {
                slot: candidate_id.slot,
                source_idx: leader_idx,
                candidate_hash_data_bytes,
                block_id: block_id.clone(),
                root_hash: block_id.root_hash.clone(),
                file_hash: block_id.file_hash.clone(),
                data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
                collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    Vec::new(),
                ),
                gen_utime_ms: None,
                receive_time: self.description.get_time(),
                is_empty,
                parent_id: parent,
            },
        );
    }

    fn recovery_notify_last_finalized(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        block_hash: CandidateHash,
        seqno: u32,
    ) {
        log::info!(
            target: LOG_TARGET,
            "Session {}: last finalized notification on restart: slot={}, seqno={}, hash={}",
            self.session_id.to_hex_string(),
            slot.value(),
            seqno,
            block_hash.to_hex_string()
        );

        // Look up the BlockIdExt from received_candidates (already seeded by
        // recovery_seed_received_candidates).
        let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
        let block_id = backend
            .candidate_book()
            .received(&candidate_id)
            .map(|r| r.block_id.clone())
            .unwrap_or_else(|| {
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: recovery_notify_last_finalized: block not found in \
                    received_candidates (slot={}, hash={})",
                    self.session_id.to_hex_string(),
                    slot.value(),
                    block_hash.to_hex_string(),
                );
                // Fallback: construct minimal BlockIdExt
                BlockIdExt {
                    shard_id: self.description.get_shard().clone(),
                    seq_no: seqno,
                    root_hash: block_hash.clone(),
                    file_hash: block_hash.clone(),
                }
            });

        // Update last_committed tracking to reflect the restart state
        backend.consensus_mut().set_finalized_head(seqno, slot, block_id.clone());
        let last_mc = backend.consensus_mut().last_mc_finalized_seqno().unwrap_or(0).max(seqno);
        backend.consensus_mut().set_last_mc_finalized_seqno(Some(last_mc));
        backend.consensus_mut().set_last_consensus_finalized_seqno(Some(seqno));
        backend.consensus_mut().advance_accepted_normal_head_block(block_id.clone());

        // Note: We do NOT set available_base here anymore. This is now done in
        // recovery_finalize_parent_chain() after all kept votes are restored,
        // because the kept votes may finalize additional slots.

        // Note: We do NOT notify ValidatorGroup here because:
        // 1. C++ only republishes finalized state, not a fresh accept callback
        // 2. The block was already accepted before restart
        // 3. Restart recovery now restores state only; no historical replay callbacks remain
    }

    fn recovery_finalize_parent_chain(&self, backend: &mut dyn StartupRecoveryBackend) {
        // After all recovery steps complete (including kept votes restoration),
        // set up the parent chain for the first non-finalized slot.
        //
        // The kept votes may have finalized additional slots beyond what was in the DB,
        // so we must use the CURRENT first_non_finalized_slot, not the one from boot.
        let first_non_finalized = backend.simplex_state().get_first_non_finalized_slot();

        // Find the parent for this slot (the last finalized block)
        let parent_slot = if first_non_finalized.value() > 0 {
            SlotIndex::new(first_non_finalized.value() - 1)
        } else {
            // Genesis case - no parent
            log::debug!(
                target: LOG_TARGET,
                "Session {}: recovery_finalize_parent_chain: first_non_finalized=s0, using \
                genesis base",
                &self.session_id.to_hex_string()[..8],
            );
            return;
        };

        // Determine the parent/base candidate for `first_non_finalized`.
        //
        // On masterchain, empty candidates are not persisted as finalizedBlock records, so the
        // immediately preceding slot may be missing from `received_candidates` after bootstrap.
        // Fall back to the latest notarized candidate <= parent_slot from simplex_state.
        let from_book = backend
            .candidate_book()
            .iter_received()
            .find(|(id, _)| id.slot == parent_slot)
            .map(|(id, _)| CandidateParentInfo { slot: id.slot, hash: id.hash.clone() });
        let parent_info = from_book
            .or_else(|| backend.simplex_state().get_latest_notarized_candidate_up_to(parent_slot));

        match parent_info {
            Some(parent_info) => {
                backend.simplex_state_mut().set_available_base_after_restart(
                    self.description.as_ref(),
                    parent_info.clone(),
                );
                log::info!(
                    target: LOG_TARGET,
                    "Session {}: recovery_finalize_parent_chain: set available_base for slot {} \
                    (parent=s{}:{})",
                    &self.session_id.to_hex_string()[..8],
                    first_non_finalized.value(),
                    parent_info.slot.value(),
                    &parent_info.hash.to_hex_string()[..8],
                );
            }
            None => {
                log::warn!(
                    target: LOG_TARGET,
                    "Session {}: recovery_finalize_parent_chain: no parent found for slot {} \
                    (parent_slot=s{})",
                    &self.session_id.to_hex_string()[..8],
                    first_non_finalized.value(),
                    parent_slot.value(),
                );
            }
        }
    }

    fn recovery_cache_notarization_cert(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        notar_cert_bytes: Vec<u8>,
    ) {
        log::trace!(
            "Session {}: recovery_cache_notarization_cert(slot={}, hash={})",
            self.session_id.to_hex_string(),
            slot.value(),
            candidate_hash.to_hex_string()
        );
        backend.receiver().cache_notarization_cert(slot.value(), candidate_hash, notar_cert_bytes);
    }

    fn recovery_seed_notarize_certificate(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        certificate: NotarCertPtr,
    ) {
        log::trace!(
            "Session {}: recovery_seed_notarize_certificate(slot={}, hash={}, sigs={})",
            self.session_id.to_hex_string(),
            slot.value(),
            &candidate_hash.to_hex_string()[..8],
            certificate.signatures.len()
        );
        let result = backend.simplex_state_mut().set_notarize_certificate(
            self.description.as_ref(),
            slot,
            &candidate_hash,
            certificate,
        );
        if let Err(e) = result {
            log::error!(
                "Session {}: recovery_seed_notarize_certificate conflict slot={} hash={}: {}",
                &self.session_id.to_hex_string()[..8],
                slot.value(),
                &candidate_hash.to_hex_string()[..8],
                e
            );
            backend.increment_error();
        }
    }

    fn recovery_seed_finalize_certificate(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        certificate: FinalCertPtr,
    ) {
        log::trace!(
            "Session {}: recovery_seed_finalize_certificate(slot={}, hash={}, sigs={})",
            self.session_id.to_hex_string(),
            slot.value(),
            &candidate_hash.to_hex_string()[..8],
            certificate.signatures.len()
        );
        let result = backend.simplex_state_mut().set_finalize_certificate(
            self.description.as_ref(),
            slot,
            &candidate_hash,
            certificate,
        );
        if let Err(e) = result {
            log::error!(
                "Session {}: recovery_seed_finalize_certificate conflict slot={} hash={}: {}",
                &self.session_id.to_hex_string()[..8],
                slot.value(),
                &candidate_hash.to_hex_string()[..8],
                e
            );
            backend.increment_error();
        }
    }

    fn recovery_seed_skip_certificate(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        certificate: SkipCertPtr,
    ) {
        log::trace!(
            "Session {}: recovery_seed_skip_certificate(slot={}, sigs={})",
            self.session_id.to_hex_string(),
            slot.value(),
            certificate.signatures.len(),
        );
        let result = backend.simplex_state_mut().set_skip_certificate(
            self.description.as_ref(),
            slot,
            certificate,
        );
        if let Err(e) = result {
            log::error!(
                "Session {}: recovery_seed_skip_certificate conflict slot={}: {}",
                &self.session_id.to_hex_string()[..8],
                slot.value(),
                e
            );
            backend.increment_error();
        }
    }

    fn recovery_cache_candidate_bytes(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        candidate_data_bytes: Vec<u8>,
    ) {
        log::trace!(
            "Session {}: recovery_cache_candidate_bytes(slot={}, hash={})",
            self.session_id.to_hex_string(),
            slot.value(),
            candidate_hash.to_hex_string()
        );
        backend.receiver().cache_candidate_bytes(
            slot.value(),
            candidate_hash,
            candidate_data_bytes,
        );
    }

    fn recovery_restore_receiver_standstill_cache(
        &self,
        backend: &mut dyn StartupRecoveryBackend,
        votes: &[VoteRecord],
    ) {
        log::trace!(
            target: LOG_TARGET,
            "Session {}: recovery_restore_receiver_standstill_cache(votes={})",
            self.session_id.to_hex_string(),
            votes.len()
        );

        // 1) Cache per-slot certificates for standstill (tracked range only)
        let (begin, end) = backend.simplex_state().get_tracked_slots_interval();
        let bundles = backend.simplex_state().collect_cached_certificates_in_range(begin, end);
        let mut cached_certs = 0u32;

        for (slot, notar, skip, final_) in bundles {
            let slot_u32 = slot.value();

            if let Some(cert) = notar {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        backend.receiver().cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Notar,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "Session {}: failed to serialize restart notar cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id.to_hex_string()[..8],
                        );
                        backend.increment_error();
                    }
                }
            }

            if let Some(cert) = skip {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        backend.receiver().cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Skip,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "Session {}: failed to serialize restart skip cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id.to_hex_string()[..8],
                        );
                        backend.increment_error();
                    }
                }
            }

            if let Some(cert) = final_ {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        backend.receiver().cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Final,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "Session {}: failed to serialize restart final cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id.to_hex_string()[..8],
                        );
                        backend.increment_error();
                    }
                }
            }
        }

        // 2) Cache last final certificate (C++ pool.cpp last_final_cert_)
        if let Some((slot, cert)) = backend.simplex_state().get_last_finalize_certificate() {
            let slot_u32 = slot.value();
            match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                Ok(bytes) => {
                    // Keep per-slot bundle for completeness (even if slot is outside tracked range)
                    backend.receiver().cache_standstill_certificate(
                        slot_u32,
                        StandstillCertificateType::Final,
                        bytes.clone(),
                    );
                    backend.receiver().cache_last_final_certificate(slot_u32, bytes);
                }
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "Session {}: failed to serialize restart last_final_cert slot={}: {}",
                        &self.session_id.to_hex_string()[..8],
                        slot_u32,
                        e
                    );
                    backend.increment_error();
                }
            }
        }

        // 3) Cache our historical votes for standstill replay
        let self_idx = self.description.get_self_idx();
        let mut cached_votes = 0u32;
        let mut vote_parse_errors = 0u32;

        for record in votes {
            if record.node_idx != self_idx {
                continue;
            }

            let msg = match deserialize_boxed(record.data.as_slice()) {
                Ok(m) => m,
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "Session {}: failed to deserialize restart vote for standstill: {}",
                        &self.session_id.to_hex_string()[..8],
                        e
                    );
                    backend.increment_error();
                    vote_parse_errors += 1;
                    continue;
                }
            };

            let tl_vote = match msg.downcast::<TlVoteBoxed>() {
                Ok(v) => v,
                Err(_) => {
                    vote_parse_errors += 1;
                    continue;
                }
            };

            let signed = tl_vote.only();
            backend.receiver().cache_our_vote_for_standstill(signed);
            cached_votes += 1;
        }

        log::info!(
            target: LOG_TARGET,
            "Session {}: restored receiver standstill cache: certs_cached={cached_certs} \
            our_votes_cached={cached_votes} vote_parse_errors={vote_parse_errors} \
            tracked_slots=[{begin}, {end})",
            self.session_id.to_hex_string(),
        );

        // Step 4 (range sync + reschedule_standstill) runs through the
        // SessionProcessor-level helper, which spans state the recovery
        // processor does not hold.
        backend.sync_standstill_after_restore();
    }
}
