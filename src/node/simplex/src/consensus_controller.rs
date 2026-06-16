/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `ConsensusController` entity
//!
//! Per-session consensus finalization controller. Owns the
//! finalization-journal state previously inlined on `SessionProcessor` plus
//! the FSM finalization/certificate event handlers and the recursive
//! parent-chain finalization walk.
//!
//! ## Owned state (the finalization journal)
//!
//! - `finalized_head_seqno` / `finalized_head_slot` / `finalized_head_block_id`
//!   / `finalized_head_before_split` — the locally materialized finalized head
//!   cursor (parent context for empty-block generation + validation shortcuts).
//! - `before_split_by_block_id` — observed `before_split` flags for non-empty
//!   block ids, the empty-block policy source.
//! - `last_consensus_finalized_seqno` — producer-side highest finalized seqno.
//! - `finalized_blocks` — blocks whose finalized state is already materialized
//!   locally (dedup of repeat events / late body arrivals).
//! - `finalized_pending_body` — finalized blocks (FinalCert observed) whose
//!   candidate body has not yet arrived; materialization is deferred.
//! - `finalized_delivery_sent` / `finalized_delivery_sent_seqno` — the
//!   at-most-once finalized-callback dedup (per candidate id and per seqno).
//!
//! Hosts the [`FinalizedEntry`] / [`FinalizedSeqnoRecord`] inner types.
//!
//! ## Finalization pipeline
//!
//! The controller owns the certificate-observed and finalization-driven
//! handlers dispatched by `SessionProcessor::process_simplex_events` (which
//! stays the event pump):
//!
//! - [`Self::handle_notarization_reached`] / [`Self::handle_skip_certificate_reached`]
//!   / [`Self::handle_finalization_reached`] react to certificate observation:
//!   record milestones, request missing bodies, emit candidate-observed, and
//!   persist+relay the cert (the persist/relay/standstill IO is the synchronous
//!   [`ConsensusBackend::persist_notar_cert_then_relay`] /
//!   `persist_skip_cert_then_relay` / `persist_final_cert_then_relay` effect,
//!   which registers the durability wait on the SXMAIN async-DB registry — the
//!   continuation stays `SessionProcessor`-side).
//! - [`Self::handle_block_finalized`] records the trigger in
//!   `finalized_pending_body` and kicks the recursive walk.
//! - [`Self::try_finalize_recursive_chain`] /
//!   [`Self::try_emit_recursive_finalized_callback`] /
//!   [`Self::maybe_apply_finalized_state`] are the recursive parent-chain walk:
//!   they apply local finalized state (persist via
//!   [`ConsensusBackend::persist_finalized_block`], advance the accepted normal
//!   head, update the head cursor) and emit the at-most-once
//!   `on_block_finalized` callback through the owned `callbacks`.
//!
//! ```text
//! process_simplex_events  (pump; stays on SessionProcessor)
//!   ├─ NotarizationReached  ─▶ handle_notarization_reached  ─▶ persist_notar_cert_then_relay
//!   ├─ SkipCertificateReached ▶ handle_skip_certificate_reached ▶ persist_skip_cert_then_relay
//!   ├─ FinalizationReached  ─▶ handle_finalization_reached  ─▶ persist_final_cert_then_relay
//!   └─ BlockFinalized       ─▶ handle_block_finalized
//!                                 │  finalized_pending_body.insert
//!                                 ▼
//!                       retry_pending_recursive_finalization_for
//!                                 ▼
//!                       try_finalize_recursive_chain (walk parents)
//!                                 ├─ maybe_apply_finalized_state ─▶ persist_finalized_block
//!                                 └─ try_emit_recursive_finalized_callback ─▶ notify_block_finalized
//! ```
//!
//! ## Boundary
//!
//! `ConsensusController` owns the finalization-journal state + finalization/cert
//! handlers, the MC applied-top pipeline, vote/certificate **ingress** (`on_vote`
//! / `on_certificate` + `misbehavior_reports`), and the consensus-meaningful
//! **outbound** action (`broadcast_vote_after_persist`: sign, persist, send). It
//! does NOT drive collation or candidate validation, and `SimplexState` stays
//! owned by `SessionProcessor` (the shared kernel, reached via
//! `simplex_state{,_mut}`). The `broadcast_vote` durability-wait shell also stays
//! on `SessionProcessor` because it crosses the `DatabaseController` async-DB
//! registry, re-entering the controller once the prerequisite is durable. The
//! `SessionProcessor`-owned reads (FSM state, candidate book, runtime slot flags)
//! and `&mut self` effects (cert/finalized-block persist+relay, slot reset, peer
//! candidate requests, vote persist + send) are reached through the borrowing
//! [`ConsensusBackend`] seam, built per call by
//! `SessionProcessor::with_consensus_backend`, mirroring `CollationBackend` /
//! `ValidationBackend`.
//!
//! The immutable per-session `Arc<SessionDescription>`, the shared
//! `Arc<SessionCallbacks>`, and the `Arc<SessionTelemetry>` are held directly
//! on the controller (cloned from the same `Arc`s `SessionProcessor` holds), so
//! session id / shard / weights / timing reads, finalized-callback dispatch,
//! and metric recording need no per-call threading.

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex},
    candidate_book::{CandidateBook, ReceivedCandidate, EMPTY_CHAIN_WARN_DEPTH, MAX_CHAIN_DEPTH},
    database::{FinalizedBlockRecord, VoteRecord},
    misbehavior::{MisbehaviorReport, VoteResult},
    receiver::StandstillCertificateType,
    session_callbacks::SessionCallbacks,
    session_description::SessionDescription,
    session_processor::ROUND_DEBUG_PERIOD,
    session_runtime::SessionRuntime,
    session_telemetry::SessionTelemetry,
    simplex_state::{
        BlockFinalizedEvent, FinalizationReachedEvent, NotarizationReachedEvent, SimplexState,
        SkipCertificateReachedEvent, Vote,
    },
    utils::{extract_vote_and_signature, sign_vote, threshold_66, verify_vote_signature},
    BlockCandidatePriority, BlockHash, BlockPayloadPtr, BlockSourceInfo, PublicKeyHash,
    RawVoteData, SessionId, SIMPLEX_ROUNDLESS,
};
use consensus_common::{check_execution_time, instrument, CandidateObservedFlags};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime},
};
use ton_api::ton::consensus::simplex::{
    vote::Vote as TlVote, Certificate, UnsignedVote, Vote as TlVoteBoxed,
};
use ton_block::{
    sha256_digest, BlockIdExt, BlockSignaturesPure, BlockSignaturesSimplex, BlockSignaturesVariant,
    CryptoSignature, CryptoSignaturePair, Result, UInt256, ValidatorBaseInfo,
};

/// Finalization journal entry.
///
/// Records that a FinalCert was observed for (slot, hash), but local finalized
/// state has not been materialized yet because the candidate body is still
/// missing.
#[derive(Clone)]
pub(crate) struct FinalizedEntry {
    /// The finalization event from `SimplexState`.
    event: BlockFinalizedEvent,
    /// Time when finalization was first observed (for timeout / diagnostics).
    finalized_at: SystemTime,
}

/// Value of `finalized_delivery_sent_seqno`: the delivered `BlockIdExt` plus the
/// originating slot. The slot is kept so this map can later be pruned together
/// with the retained candidate metadata; until then, keeping this map
/// session-long is what prevents old retained `received_candidates` from
/// re-emitting callbacks.
#[derive(Debug, Clone)]
struct FinalizedSeqnoRecord {
    slot: SlotIndex,
    block_id: BlockIdExt,
}

/// Session-owned reads/effects the finalization pipeline needs but cannot reach
/// from `&mut ConsensusController` alone.
///
/// This is the controller's borrowing backend view: a drain-scoped handle built
/// fresh from `&mut SessionProcessor` by `session_processor`'s
/// `with_consensus_backend` split-borrow immediately before a handler / walk
/// runs, then dropped (RAII). Because it is rebuilt per call, every read is
/// current — no captured snapshot, no shared mutable handle. The controller's
/// own state lives on `&mut Self`; everything genuinely owned by
/// `SessionProcessor` is reached only through this trait.
///
/// Reads available from handles the controller already holds are NOT duplicated
/// here: session id / shard / weights / timing come from the owned
/// `description`, and counters from the owned `telemetry`. The applied-top floor
/// and accepted-normal-head cursor are now controller-owned state (TN-1408
/// Phase 7 Sub-PR 2), so what remains are the FSM / candidate-book reads and the
/// `SessionProcessor`-mediated effects below.
///
/// ## Effect timing
///
/// The `persist_*` effects run
/// **synchronously** in the production adapter (the cert/finalized-block
/// persist registers a wait on the SXMAIN async-DB registry exactly as the
/// pre-refactor in-line code did, preserving the persist→relay ordering and the
/// `WaitFor*` semantics). The `request_candidate` and `reset_slot_state` effects
/// are **deferred** — they need `&mut SessionProcessor` machinery the borrowing
/// adapter cannot provide, so they bounce a closure onto SXMAIN (mirroring
/// `CollationBackend::request_parent_candidate` /
/// `ValidationBackend::request_candidate`). `reset_slot_state` re-enters
/// `self.consensus.prune_below`, so it must run after the borrow scope ends;
/// it is ancient-slot GC, so the one-turn deferral is behaviour-neutral.
pub(crate) trait ConsensusBackend {
    /// Borrow the consensus FSM state (notarize-cert lookup, slot-progressed
    /// check) for the recursive walk.
    fn simplex_state(&self) -> &SimplexState;

    /// Mutably borrow the consensus FSM state for vote/certificate ingress:
    /// `SimplexState::on_vote` and `set_{notarize,skip,finalize}_certificate`.
    /// `SimplexState` itself stays owned by `SessionProcessor` (the shared
    /// kernel); the controller reaches it only for the duration of a single
    /// `with_consensus_backend` borrow scope.
    fn simplex_state_mut(&mut self) -> &mut SimplexState;

    /// Borrow the received-candidate book (body/metadata holder) for the walk.
    fn candidate_book(&self) -> &CandidateBook;

    /// Borrow the per-session runtime (per-slot started-at + first-finalized
    /// latency flags read by `maybe_apply_finalized_state`).
    fn runtime(&self) -> &SessionRuntime;

    /// Mutably borrow the runtime to set the per-slot first-candidate-finalized
    /// latency flag, and lower the wake horizon from
    /// [`ConsensusController::set_mc_finalized_block`].
    fn runtime_mut(&mut self) -> &mut SessionRuntime;

    /// Request a missing candidate body/metadata from peers through the shared
    /// `SessionProcessor::request_candidate` throttle map. Bounces a closure
    /// onto SXMAIN (the fetch needs `&mut SessionProcessor`).
    fn request_candidate(&self, slot: SlotIndex, hash: UInt256, delay: Option<Duration>);

    /// Reset per-slot state after a slot is materially finalized (FSM/receiver
    /// GC + precollation cleanup). Bounces onto SXMAIN: the underlying
    /// `SessionProcessor::reset_slot_state` re-enters `self.consensus`
    /// (`cleanup_old_candidates` → `ConsensusController::prune_below`), so it
    /// must run after the controller borrow scope ends.
    fn reset_slot_state(&self, slot: SlotIndex);

    /// Persist the notarization certificate then (after durability) cache the
    /// VoteSignatureSet, relay the full cert, and cache it for standstill.
    /// Synchronous registration on the SXMAIN async-DB registry; the relay
    /// continuation runs `SessionProcessor`-side post-durability.
    fn persist_notar_cert_then_relay(&mut self, event: &NotarizationReachedEvent);

    /// Persist the skip certificate then (after durability) relay + cache it
    /// for standstill. See [`Self::persist_notar_cert_then_relay`].
    fn persist_skip_cert_then_relay(&mut self, event: &SkipCertificateReachedEvent);

    /// Persist the finalization certificate then (after durability) relay,
    /// cache per-slot + last-final, and update standstill tracking. See
    /// [`Self::persist_notar_cert_then_relay`].
    fn persist_final_cert_then_relay(&mut self, event: &FinalizationReachedEvent);

    /// Persist a finalized-block record for restart recovery. On masterchain
    /// the write is registered with the SXMAIN async-DB registry (logging
    /// continuation); on shardchains it is a fire-and-forget queued write.
    ///
    /// Returns `false` only when the synchronous registration failed
    /// (`save_finalized_block[_async]` returned `Err` immediately) — the caller
    /// then leaves the trigger queued in `finalized_pending_body`.
    fn persist_finalized_block(&mut self, record: FinalizedBlockRecord) -> bool;

    /// Persist an applied incoming vote to the session DB (fire-and-forget).
    /// Mirrors C++ `store_vote_to_db(message.data, source).detach()` after
    /// `handle_vote(...)` returns true: the async write handle is discarded and
    /// only an immediate `Err` is surfaced so [`ConsensusController::on_vote`]
    /// can bump the error counter.
    fn save_incoming_vote(&self, record: &VoteRecord) -> Result<()>;

    /// Notify the receiver's standstill tracker that a certificate of `kind`
    /// was accepted for `slot` (forwards to `Receiver::notify_certificate_accepted`).
    /// Must be called only after the matching `SimplexState::set_*_certificate`
    /// returned `Ok(true)` (newly stored), matching the pre-refactor in-line order.
    fn notify_certificate_accepted(&self, slot: u32, kind: StandstillCertificateType);

    /// Temporarily ban a peer whose certificate failed signature / weight
    /// verification (forwards to `Receiver::ban_source_for_bad_signature`,
    /// C++ `PoolImpl::ban`), so repeated forged traffic cannot starve the
    /// receiver/processor pipeline.
    fn ban_source_for_bad_signature(&self, source_idx: u32);

    /// Broadcast a locally produced, session-signed vote to all validators
    /// (forwards to `Receiver::send_vote`, which serializes the TL vote,
    /// fans it out, and loops it back through the listener for FSM accounting).
    /// Mirrors C++ `pool.cpp::handle(BroadcastVote)` publishing
    /// `OutgoingProtocolMessage`.
    fn send_vote(&self, signed_vote: TlVote);

    /// Persist a locally produced signed vote to the session DB (non-blocking):
    /// register the in-flight `save_vote_async` write on the SXMAIN async-DB
    /// registry, whose continuation only logs / bumps the error counter. The
    /// broadcast is **not** gated on it — mirrors C++
    /// `db.cpp::process(BroadcastVote)`, where the `co_await db->set(...)` lives
    /// in the db actor only and does not gate pool's publish.
    fn persist_our_vote(&mut self, signed_vote: &TlVote);
}

/// Per-session consensus finalization controller.
///
/// Owned by [`SessionProcessor`](crate::session_processor::SessionProcessor) as
/// `self.consensus`; all access goes through the accessor methods declared
/// below — internal fields stay non-`pub` so the boundary is enforceable.
pub(crate) struct ConsensusController {
    /// Callback-delivery aspect, shared with `SessionProcessor` and
    /// `SessionImpl` as `Arc<SessionCallbacks>`. Held directly so the
    /// controller can dispatch `notify_block_finalized` / `notify_candidate_observed`
    /// without routing the call back through `SessionProcessor`.
    callbacks: Arc<SessionCallbacks>,
    /// Immutable per-session configuration handle, the same `Arc` held by
    /// `SessionProcessor` / `SessionRuntime`. Held directly for session id /
    /// shard / weights / source keys / timing reads.
    description: Arc<SessionDescription>,
    /// Per-session telemetry aspect, the same `Arc` held by `SessionProcessor`.
    /// Held directly so finalization paths record milestones / latencies /
    /// counters without threading a `&SessionTelemetry`. Interior-mutable.
    telemetry: Arc<SessionTelemetry>,

    /// Highest finalized non-empty block seqno materialized locally.
    finalized_head_seqno: Option<u32>,
    /// Slot of the latest locally materialized finalized head.
    finalized_head_slot: Option<SlotIndex>,
    /// Last finalized non-empty block id (parent for empty blocks).
    finalized_head_block_id: Option<BlockIdExt>,
    /// Last finalized head block's `before_split` flag (split/merge fallback).
    finalized_head_before_split: bool,
    /// Observed `before_split` values for non-empty block ids (empty-block
    /// policy source).
    before_split_by_block_id: HashMap<BlockIdExt, bool>,
    /// Producer-side highest finalized seqno tracked for this session.
    last_consensus_finalized_seqno: Option<u32>,
    /// Blocks whose finalized state has already been materialized locally
    /// (dedup of repeat finalization events + late body arrivals).
    finalized_blocks: HashSet<RawCandidateId>,
    /// Finalized blocks (FinalCert observed) still waiting for candidate body
    /// arrival; materialization is deferred until the body arrives.
    finalized_pending_body: HashMap<RawCandidateId, FinalizedEntry>,
    /// Dedup set for finalized delivery: emit at most once per finalized
    /// candidate id.
    finalized_delivery_sent: HashSet<RawCandidateId>,
    /// Seqno-level finalized callback tracking (at most one callback per seqno
    /// for a distinct block; idempotent for the same block id).
    finalized_delivery_sent_seqno: HashMap<u32, FinalizedSeqnoRecord>,

    /*
        ====================================================================
        Masterchain applied-top tracking (TN-1408 Phase 7, Sub-PR 2)

        The validator-manager finalization pipeline. Seeded from
        `initial_block_seqno - 1` and advanced by `set_mc_finalized_block`
        (manager applied-top notifications), restart recovery, and finalized
        non-empty blocks (the recursive walk). Mirrors the C++
        `block-producer.cpp` / `block-validator.cpp` applied-top handling.
        ====================================================================
    */
    /// Last MC-registered applied-top seqno for this shard session.
    ///
    /// The recursive-walk parent-climb floor and the shard callback-emission
    /// gate (`applied_floor`), and the shardchain empty-block lag input
    /// (`CollationController::should_generate_empty_block`). Kept monotonic.
    last_mc_finalized_seqno: Option<u32>,
    /// Seqno fallback for the accepted normal head used by MC validation
    /// ordering. Seeded from `initial_block_seqno - 1`, then advanced by
    /// applied-top notifications, restart recovery, and finalized non-empty
    /// blocks.
    accepted_normal_head_seqno: u32,
    /// Exact accepted normal head when known (mirrors the C++
    /// `last_accepted_block_` semantics closely enough to reject stale
    /// same-seqno forks once an exact `BlockIdExt` has been observed). Read by
    /// validation parent-tip resolution.
    accepted_normal_head_block_id: Option<BlockIdExt>,

    /// Misbehavior proofs collected from vote ingress (`SimplexState::on_vote`
    /// returning [`VoteResult::Misbehavior`]). Write-only accumulator today —
    /// kept for the future ValidatorGroup slashing/reporting hook; moved here
    /// with vote ingress in TN-1408 Phase 7 Sub-PR 3.
    misbehavior_reports: Vec<MisbehaviorReport>,
}

// ======================================================================
// Construction & handles
// ======================================================================
// Build the controller and read its shared session handles.
impl ConsensusController {
    /// Construct a fresh consensus controller.
    ///
    /// `callbacks` / `description` / `telemetry` are cheap-to-clone shared
    /// handles (`Arc`) cloned from `SessionProcessor` at construction.
    ///
    /// `finalized_head_seqno` / `last_consensus_finalized_seqno` /
    /// `last_mc_finalized_seqno` / `accepted_normal_head_seqno` are seeded from
    /// `initial_block_seqno - 1` (the block before session start is treated as
    /// the finalized head + applied top), matching the pre-refactor
    /// `SessionProcessor::new`.
    pub(crate) fn new(
        callbacks: Arc<SessionCallbacks>,
        description: Arc<SessionDescription>,
        telemetry: Arc<SessionTelemetry>,
        finalized_head_seqno: Option<u32>,
        last_consensus_finalized_seqno: Option<u32>,
        last_mc_finalized_seqno: Option<u32>,
        accepted_normal_head_seqno: u32,
    ) -> Self {
        Self {
            callbacks,
            description,
            telemetry,
            finalized_head_seqno,
            finalized_head_slot: None,
            finalized_head_block_id: None,
            finalized_head_before_split: false,
            before_split_by_block_id: HashMap::new(),
            last_consensus_finalized_seqno,
            finalized_blocks: HashSet::new(),
            finalized_pending_body: HashMap::new(),
            finalized_delivery_sent: HashSet::new(),
            finalized_delivery_sent_seqno: HashMap::new(),
            last_mc_finalized_seqno,
            accepted_normal_head_seqno,
            accepted_normal_head_block_id: None,
            misbehavior_reports: Vec::new(),
        }
    }

    /// Session identifier (via the owned description handle).
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.description.get_session_id()
    }

    /// Current session time (real-time or manually overridden for tests/replay),
    /// via the owned description handle.
    #[inline]
    fn now(&self) -> SystemTime {
        self.description.get_time()
    }
}

// ======================================================================
// Finalization journal state
// ======================================================================
// The locally-materialized finalized-head cursor, masterchain applied-top
// cursor, finalized-block/delivery dedup journal, and history pruning.
impl ConsensusController {
    /* Finalized-head cursor */

    /// Highest finalized non-empty block seqno materialized locally.
    pub(crate) fn finalized_head_seqno(&self) -> Option<u32> {
        self.finalized_head_seqno
    }

    /// Slot of the latest locally materialized finalized head.
    pub(crate) fn finalized_head_slot(&self) -> Option<SlotIndex> {
        self.finalized_head_slot
    }

    /// Last finalized non-empty block id (parent for empty blocks).
    pub(crate) fn finalized_head_block_id(&self) -> &Option<BlockIdExt> {
        &self.finalized_head_block_id
    }

    /// Last finalized head block's `before_split` flag.
    pub(crate) fn finalized_head_before_split(&self) -> bool {
        self.finalized_head_before_split
    }

    /// Observed `before_split` values for non-empty block ids.
    pub(crate) fn before_split_by_block_id(&self) -> &HashMap<BlockIdExt, bool> {
        &self.before_split_by_block_id
    }

    /// Producer-side highest finalized seqno tracked for this session.
    pub(crate) fn last_consensus_finalized_seqno(&self) -> Option<u32> {
        self.last_consensus_finalized_seqno
    }

    /// Set the finalized head cursor (recovery republish).
    pub(crate) fn set_finalized_head(&mut self, seqno: u32, slot: SlotIndex, block_id: BlockIdExt) {
        self.finalized_head_seqno = Some(seqno);
        self.finalized_head_slot = Some(slot);
        self.finalized_head_block_id = Some(block_id);
    }

    /// Set the producer-side last-consensus-finalized seqno (set_mc / recovery).
    pub(crate) fn set_last_consensus_finalized_seqno(&mut self, value: Option<u32>) {
        self.last_consensus_finalized_seqno = value;
    }

    /// Record an observed `before_split` flag for a non-empty block id
    /// (`on_candidate_received`).
    pub(crate) fn insert_before_split(&mut self, block_id: BlockIdExt, before_split: bool) {
        self.before_split_by_block_id.insert(block_id, before_split);
    }

    /* Masterchain applied-top cursor */

    /// Last MC-registered applied-top seqno for this shard session.
    pub(crate) fn last_mc_finalized_seqno(&self) -> Option<u32> {
        self.last_mc_finalized_seqno
    }

    /// Set the MC applied-top seqno (restart recovery + tests). The monotonic
    /// max is applied by the caller / [`Self::set_mc_finalized_block`].
    pub(crate) fn set_last_mc_finalized_seqno(&mut self, value: Option<u32>) {
        self.last_mc_finalized_seqno = value;
    }

    /// Seqno of the accepted normal head used by MC validation ordering.
    pub(crate) fn accepted_normal_head_seqno(&self) -> u32 {
        self.accepted_normal_head_seqno
    }

    /// Set the accepted-normal-head seqno fallback (tests).
    #[cfg(test)]
    pub(crate) fn set_accepted_normal_head_seqno(&mut self, value: u32) {
        self.accepted_normal_head_seqno = value;
    }

    /// Exact accepted normal head when known (validation parent-tip seed).
    pub(crate) fn accepted_normal_head_block_id(&self) -> &Option<BlockIdExt> {
        &self.accepted_normal_head_block_id
    }

    /* Finalized-block & delivery journal */

    /// True if `id`'s finalized state has been materialized locally.
    pub(crate) fn is_finalized_block(&self, id: &RawCandidateId) -> bool {
        self.finalized_blocks.contains(id)
    }

    /// Number of finalized triggers still waiting for candidate body arrival.
    pub(crate) fn finalized_pending_body_len(&self) -> usize {
        self.finalized_pending_body.len()
    }

    /// Time a still-pending finalized trigger was first observed (debug dump).
    pub(crate) fn finalized_pending_finalized_at(&self, id: &RawCandidateId) -> Option<SystemTime> {
        self.finalized_pending_body.get(id).map(|entry| entry.finalized_at)
    }

    /// Block id previously delivered at `seqno`, if any (recovery-seed dedup).
    pub(crate) fn finalized_delivery_sent_seqno_block_id(&self, seqno: u32) -> Option<BlockIdExt> {
        self.finalized_delivery_sent_seqno.get(&seqno).map(|record| record.block_id.clone())
    }

    /// Mark a candidate id as having its finalized state materialized locally
    /// (recovery seed).
    pub(crate) fn insert_finalized_block(&mut self, id: RawCandidateId) {
        self.finalized_blocks.insert(id);
    }

    /// Mark a candidate id as finalized-callback-delivered (recovery seed).
    pub(crate) fn insert_finalized_delivery_sent(&mut self, id: RawCandidateId) {
        self.finalized_delivery_sent.insert(id);
    }

    /// Record the seqno-level finalized-callback dedup entry (recovery seed).
    pub(crate) fn insert_finalized_delivery_sent_seqno(
        &mut self,
        seqno: u32,
        slot: SlotIndex,
        block_id: BlockIdExt,
    ) {
        self.finalized_delivery_sent_seqno.insert(seqno, FinalizedSeqnoRecord { slot, block_id });
    }

    /* History pruning */

    /// Prune finalized-journal entries for slots `< up_to_slot`.
    ///
    /// Fan-out target from `SessionProcessor::cleanup_old_candidates`. Prunes
    /// the transient `finalized_pending_body` buffer and the per-id
    /// `finalized_delivery_sent` dedup, then refreshes the pending-body gauge.
    ///
    /// `finalized_delivery_sent_seqno` is intentionally NOT pruned here: while
    /// old `received_candidates` remain retained, dropping the seqno dedup
    /// could let a later recursive parent-chain walk re-emit an
    /// already-delivered finalized callback.
    pub(crate) fn prune_below(&mut self, up_to_slot: SlotIndex) {
        self.finalized_pending_body.retain(|id, _| id.slot >= up_to_slot);
        self.telemetry.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);
        self.finalized_delivery_sent.retain(|id| id.slot >= up_to_slot);
    }
}

// ======================================================================
// Vote & certificate ingress
// ======================================================================
// SXMAIN entry points for inbound votes / certificates (signature checks,
// persistence, FSM submission, misbehavior reports).
impl ConsensusController {
    /// Handle an incoming vote from the network.
    ///
    /// Verifies the signature, converts the TL vote to an FSM vote, and passes
    /// it to [`SimplexState::on_vote`] via
    /// [`ConsensusBackend::simplex_state_mut`]. The signature is retained by the
    /// FSM for certificate creation and the raw bytes are threaded through for
    /// misbehavior-proof storage.
    ///
    /// Returns `true` iff the vote was newly applied (`VoteResult::Applied`) so
    /// the caller pumps `check_all()`; duplicate / late / misbehavior / rejected
    /// votes return `false` (no pump).
    pub(crate) fn on_vote(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        source_idx: u32,
        tl_vote: TlVoteBoxed,
        raw_vote: RawVoteData,
    ) -> bool {
        //check_execution_time!(30_000); //TODO: LK: restore during performance testing

        let source_idx = ValidatorIndex::new(source_idx);

        // Validate source index
        if !source_idx.is_valid(self.description.get_total_nodes()) {
            log::warn!(
                "Session {} on_vote: invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.description.get_total_nodes()
            );
            return false;
        }

        // Fast-path: drop votes that reference already-finalized slots BEFORE signature verification.
        // This avoids wasted crypto verification for late / duplicated votes.
        //
        // C++ parity: `state.slot_at(slot)` returns nullopt for `slot < first_non_finalized_slot_`.
        let (tl_kind, tl_slot, tl_hash_opt) = match tl_vote.vote() {
            UnsignedVote::Consensus_Simplex_NotarizeVote(u) => {
                if *u.id.slot() < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in NotarizeVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.id.slot()
                    );
                    return false;
                }
                let slot = SlotIndex::new(*u.id.slot() as u32);
                let hash = UInt256::from_slice(u.id.hash().as_slice());
                ("notarize", slot, Some(hash))
            }
            UnsignedVote::Consensus_Simplex_FinalizeVote(u) => {
                if *u.id.slot() < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in FinalizeVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.id.slot()
                    );
                    return false;
                }
                let slot = SlotIndex::new(*u.id.slot() as u32);
                let hash = UInt256::from_slice(u.id.hash().as_slice());
                ("finalize", slot, Some(hash))
            }
            UnsignedVote::Consensus_Simplex_SkipVote(u) => {
                if u.slot < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in SkipVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.slot
                    );
                    return false;
                }
                ("skip", SlotIndex::new(u.slot as u32), None)
            }
        };

        let fsm_first_non_finalized_slot = backend.simplex_state().get_first_non_finalized_slot();
        if tl_slot < fsm_first_non_finalized_slot {
            log::trace!(
                "Session {} on_vote: dropping old vote slot={tl_slot} (< \
                first_non_finalized={fsm_first_non_finalized_slot}) kind={tl_kind} from \
                source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
            );
            return false;
        }

        // Mirror C++ vote ingress: reject slots at or beyond `first_too_new_slot`.
        if backend.simplex_state().is_vote_slot_too_far_ahead(tl_slot) {
            log::warn!(
                "Session {} on_vote: REJECTED - slot {tl_slot} too far ahead (first_too_new={}) \
                kind={tl_kind} from source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
                backend.simplex_state().first_too_new_vote_slot(),
            );
            return false;
        }

        let tl_hash_prefix = tl_hash_opt
            .as_ref()
            .map(|h| hex::encode(&h.as_slice()[..4]))
            .unwrap_or_else(|| "-".to_string());
        log::trace!(
            "Session {} on_vote: source_idx={} kind={} slot={} hash={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            tl_slot,
            tl_hash_prefix.as_str(),
        );

        // Get source's public key for signature verification
        let source_public_key = self.description.get_source_public_key(source_idx);

        // Verify signature
        if !verify_vote_signature(&tl_vote, self.session_id(), source_public_key) {
            log::warn!(
                "Session {} on_vote: invalid signature from source_idx={}",
                self.session_id().to_hex_string(),
                source_idx
            );
            return false;
        }

        // Extract FSM vote AND signature from TL (signature stored for certificate creation)
        let (vote, signature) = match extract_vote_and_signature(&tl_vote) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "Session {} on_vote: failed to extract vote from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    e
                );
                return false;
            }
        };

        // Extract slot for logging before vote is moved
        let vote_slot = match &vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
        };

        log::trace!(
            "Session {} on_vote: verified vote from source_idx={} kind={} slot={} hash={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            vote_slot,
            tl_hash_prefix.as_str(),
        );

        match tl_kind {
            "notarize" => {
                self.telemetry.votes_in_total_counter.increment(1);
                self.telemetry.votes_in_notarize_counter.increment(1);
                self.telemetry.votes_in_notarize_total.fetch_add(1, Ordering::Relaxed);
            }
            "finalize" => {
                self.telemetry.votes_in_total_counter.increment(1);
                self.telemetry.votes_in_finalize_counter.increment(1);
                self.telemetry.votes_in_finalize_total.fetch_add(1, Ordering::Relaxed);
            }
            "skip" => {
                self.telemetry.votes_in_total_counter.increment(1);
                self.telemetry.votes_in_skip_counter.increment(1);
                self.telemetry.votes_in_skip_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        // Preserve raw bytes for DB persistence (simplex_state.on_vote consumes raw_vote)
        let raw_vote_for_db = raw_vote.clone();

        // Pass to FSM with signature and raw bytes (for certificate creation and misbehavior proofs)
        let result = backend.simplex_state_mut().on_vote(
            self.description.as_ref(),
            source_idx,
            vote,
            signature,
            raw_vote,
        );

        match result {
            VoteResult::Applied => {
                // Vote applied successfully.
                //
                // Persist vote to DB (fire-and-forget), matching C++:
                // `if (handle_vote(...)) store_vote_to_db(message->message.data.clone(), source).detach();`
                let vote_hash = UInt256::from_slice(&sha256_digest(raw_vote_for_db.as_bytes()));
                let record = VoteRecord {
                    vote_hash,
                    data: raw_vote_for_db.to_raw_buffer(),
                    node_idx: source_idx,
                    seqno: 0, // assigned by save_vote_async
                };
                if let Err(e) = backend.save_incoming_vote(&record) {
                    log::error!(
                        "Session {} on_vote: failed to create vote save: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.telemetry.increment_error();
                }

                // Proactively request missing candidate when receiving a NotarizeVote
                // for a block we don't have. This handles the case where the candidate
                // broadcast was lost (e.g., due to QUIC congestion stall with C++ ngtcp2).
                // Without this, the node can't vote and NotarizationReached is never triggered,
                // which is the normal trigger for candidate requests.
                if let Some(ref hash) = tl_hash_opt {
                    let candidate_id = RawCandidateId { slot: tl_slot, hash: hash.clone() };
                    if !backend.candidate_book().has_real_body(&candidate_id) {
                        log::debug!(
                            "Session {} on_vote: NotarizeVote for missing candidate \
                            slot={tl_slot} hash={} from source_idx={source_idx}, requesting",
                            &self.session_id().to_hex_string()[..8],
                            &hash.to_hex_string()[..8]
                        );
                        backend.request_candidate(tl_slot, hash.clone(), None);
                    }
                }
            }
            VoteResult::Duplicate => {
                // Duplicate vote, silently ignore
                log::trace!(
                    "Session {} on_vote: duplicate vote from source_idx={} slot={}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    vote_slot
                );
                return false;
            }
            VoteResult::SlotAlreadyFinalized => {
                // Late vote for already-finalized slot - completely normal in distributed systems
                log::trace!(
                    "Session {} on_vote: late vote from source_idx={source_idx} slot={vote_slot} \
                    (slot already finalized)",
                    self.session_id().to_hex_string(),
                );
                return false;
            }
            VoteResult::Misbehavior(proof) => {
                log::warn!(
                    "Session {} on_vote: MISBEHAVIOR from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    proof
                );

                // Collect misbehavior report for potential downstream processing
                let slot = proof.slot();
                let report = MisbehaviorReport { validator_idx: source_idx, slot, proof };
                self.misbehavior_reports.push(report);
                self.telemetry.misbehavior_counter.increment(1);

                // TODO: Callback to ValidatorGroup for slashing/reporting
                return false;
            }
            VoteResult::Rejected(reason) => {
                log::warn!(
                    "Session {} on_vote: FSM rejected vote from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    reason
                );
                return false;
            }
        }

        // Vote applied — caller pumps the main loop.
        true
    }

    /// Handle an incoming certificate from the network.
    ///
    /// C++ nodes broadcast certificates when thresholds are reached. Validates
    /// the source, drops certs for already-finalized slots, verifies the
    /// certificate (`Certificate::from_tl` enforces the strict C++ policy: valid
    /// and unique validator indices, every signature valid, at least 2/3 weight),
    /// then stores it through [`ConsensusBackend::simplex_state_mut`] and relays
    /// via the standstill tracker on first store.
    ///
    /// Returns `true` iff the certificate passed verification (so the caller
    /// pumps `check_all()`); rejected / late / invalid-source certificates
    /// return `false`.
    ///
    /// Reference: C++ pool.cpp `handle(IncomingProtocolMessage)` →
    /// `handle_foreign_certificate(cert)`: look up the slot, store the cert
    /// (notar/skip/final), update per-validator vote accounting, propagate.
    pub(crate) fn on_certificate(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        source_idx: u32,
        tl_certificate: Certificate,
    ) -> bool {
        let source_idx = ValidatorIndex::new(source_idx);

        // Avoid logging the full TL certificate (includes signature bytes) on the hot path.
        // It is extremely verbose and materially slows down trace-enabled test runs.
        let (tl_slot, tl_kind, tl_hash_opt, tl_sig_count) = match &tl_certificate {
            Certificate::Consensus_Simplex_Certificate(c) => {
                let sig_count = c.signatures.votes().len();
                match &c.vote {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
                        if *v.id.slot() < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in NotarizeVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.id.slot()
                            );
                            return false;
                        }
                        let slot = SlotIndex::new(*v.id.slot() as u32);
                        let hash = UInt256::from_slice(v.id.hash().as_slice());
                        (slot, "notarize", Some(hash), sig_count)
                    }
                    UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
                        if *v.id.slot() < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in FinalizeVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.id.slot()
                            );
                            return false;
                        }
                        let slot = SlotIndex::new(*v.id.slot() as u32);
                        let hash = UInt256::from_slice(v.id.hash().as_slice());
                        (slot, "finalize", Some(hash), sig_count)
                    }
                    UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                        if v.slot < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in SkipVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.slot
                            );
                            return false;
                        }
                        (SlotIndex::new(v.slot as u32), "skip", None, sig_count)
                    }
                }
            }
        };

        let tl_hash_prefix = tl_hash_opt
            .as_ref()
            .map(|h| hex::encode(&h.as_slice()[..4]))
            .unwrap_or_else(|| "-".to_string());
        log::debug!(
            "Session {} on_certificate: source_idx={} kind={} slot={} hash={} sigs={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            tl_slot,
            tl_hash_prefix.as_str(),
            tl_sig_count
        );

        // Validate source index - must be a known validator in the session
        if !source_idx.is_valid(self.description.get_total_nodes()) {
            log::warn!(
                "Session {} on_certificate: REJECTED - invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.description.get_total_nodes()
            );
            return false;
        }

        // C++ parity: drop certificates that reference finalized slots BEFORE signature verification.
        // This avoids wasted crypto verification for late / duplicated certificates.
        //
        // Reference: C++ `state.slot_at(slot)` returns nullopt for `slot < first_non_finalized_slot_`,
        // so `handle_foreign_certificate` ignores them (prevents state resurrection / last_final regressions).
        let fsm_first_non_finalized_slot = backend.simplex_state().get_first_non_finalized_slot();
        if tl_slot < fsm_first_non_finalized_slot {
            log::trace!(
                "Session {} on_certificate: dropping old certificate slot={tl_slot} \
                (< first_non_finalized={fsm_first_non_finalized_slot}) kind={tl_kind} \
                from source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
            );
            return false;
        }

        // Parse and verify the certificate (C++ strict policy)
        // Certificate::from_tl performs comprehensive validation:
        // - Rejects invalid validator indices
        // - Rejects duplicate validator indices
        // - Rejects if any signature is invalid
        // - Rejects if total weight < 2/3 threshold
        let cert = match crate::certificate::Certificate::<Vote>::from_tl(
            &tl_certificate,
            self.description.as_ref(),
            self.session_id(),
        ) {
            Ok(c) => c,
            Err(e) => {
                self.telemetry.cert_verify_fail_counter.increment(1);
                self.telemetry.cert_verify_fails_total.fetch_add(1, Ordering::Relaxed);
                log::warn!(
                    "Session {} on_certificate: REJECTED from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    e
                );
                // C++ parity (`pool.cpp`): bad-signature certificates trigger a
                // temporary peer ban so repeated forged traffic cannot starve
                // the receiver/processor pipeline.
                backend.ban_source_for_bad_signature(source_idx.value());
                return false;
            }
        };

        self.telemetry.certs_in_counter.increment(1);

        log::debug!(
            "Session {} on_certificate: verified certificate with {} valid signatures",
            self.session_id().to_hex_string(),
            cert.signatures.len()
        );

        // Proactively request missing candidate when receiving a certificate
        // for a block we don't have. This handles the case where the candidate
        // broadcast was lost (e.g., due to QUIC congestion stall with C++ ngtcp2).
        if let Some(ref hash) = tl_hash_opt {
            let candidate_id = RawCandidateId { slot: tl_slot, hash: hash.clone() };
            if !backend.candidate_book().has_real_body(&candidate_id) {
                log::debug!(
                    "Session {} on_certificate: {tl_kind} cert for missing candidate \
                    slot={tl_slot} hash={} from source_idx={source_idx}, requesting",
                    &self.session_id().to_hex_string()[..8],
                    &hash.to_hex_string()[..8]
                );
                backend.request_candidate(tl_slot, hash.clone(), None);
            }
        }

        // Dispatch based on vote type in certificate
        // If stored (new certificate), relay to other validators and cache for standstill
        match &cert.vote {
            Vote::Notarize(notar_vote) => {
                log::debug!(
                    "Session {} on_certificate: NotarCert slot={} block={} sigs={}",
                    self.session_id().to_hex_string(),
                    notar_vote.slot,
                    &notar_vote.block_hash.to_hex_string()[..8],
                    cert.signatures.len()
                );
                let notar_cert = Arc::new(crate::certificate::Certificate {
                    vote: notar_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match backend.simplex_state_mut().set_notarize_certificate(
                    self.description.as_ref(),
                    notar_vote.slot,
                    &notar_vote.block_hash,
                    notar_cert.clone(),
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored NotarCert slot={} block={} ({} \
                            sigs)",
                            self.session_id().to_hex_string(),
                            notar_vote.slot,
                            &notar_vote.block_hash.to_hex_string()[..8],
                            cert.signatures.len(),
                        );
                        backend.notify_certificate_accepted(
                            notar_vote.slot.value(),
                            StandstillCertificateType::Notar,
                        );
                    }
                    Ok(false) => {
                        // Already stored for same block - idempotent
                    }
                    Err(e) => {
                        self.telemetry.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: NotarCert conflict slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            notar_vote.slot,
                            e
                        );
                    }
                }
            }
            Vote::Finalize(final_vote) => {
                log::debug!(
                    "Session {} on_certificate: FinalCert slot={} block={} sigs={}",
                    self.session_id().to_hex_string(),
                    final_vote.slot,
                    &final_vote.block_hash.to_hex_string()[..8],
                    cert.signatures.len()
                );
                let final_cert = Arc::new(crate::certificate::Certificate {
                    vote: final_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match backend.simplex_state_mut().set_finalize_certificate(
                    self.description.as_ref(),
                    final_vote.slot,
                    &final_vote.block_hash,
                    final_cert,
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored FinalCert slot={} block={} ({} \
                            sigs)",
                            self.session_id().to_hex_string(),
                            final_vote.slot,
                            &final_vote.block_hash.to_hex_string()[..8],
                            cert.signatures.len(),
                        );
                        backend.notify_certificate_accepted(
                            final_vote.slot.value(),
                            StandstillCertificateType::Final,
                        );
                    }
                    Ok(false) => {
                        // Already stored for same block - idempotent
                    }
                    Err(e) => {
                        self.telemetry.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: FinalCert conflict slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            final_vote.slot,
                            e
                        );
                    }
                }
            }
            Vote::Skip(skip_vote) => {
                log::debug!(
                    "Session {} on_certificate: SkipCert slot={} sigs={}",
                    self.session_id().to_hex_string(),
                    skip_vote.slot,
                    cert.signatures.len()
                );
                let skip_cert = Arc::new(crate::certificate::Certificate {
                    vote: skip_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match backend.simplex_state_mut().set_skip_certificate(
                    self.description.as_ref(),
                    skip_vote.slot,
                    skip_cert,
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored SkipCert slot={} ({} sigs)",
                            self.session_id().to_hex_string(),
                            skip_vote.slot,
                            cert.signatures.len()
                        );
                        backend.notify_certificate_accepted(
                            skip_vote.slot.value(),
                            StandstillCertificateType::Skip,
                        );
                    }
                    Ok(false) => {
                        // Already stored - idempotent
                    }
                    Err(e) => {
                        self.telemetry.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: SkipCert error slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            skip_vote.slot,
                            e
                        );
                    }
                }
            }
        }

        // Certificate verified — caller pumps the main loop.
        true
    }
}

// ======================================================================
// Outbound vote
// ======================================================================
// The consensus-meaningful outbound action: sign + persist + send, re-entered
// once the durability prerequisite is satisfied SessionProcessor-side.
impl ConsensusController {
    /// Sign and broadcast a locally produced vote whose durability prerequisite
    /// is confirmed.
    ///
    /// Reached from the `SessionProcessor::broadcast_vote` wait-shell: that shell
    /// owns the votes-out telemetry and the C++ `WaitCandidateInfoStored` gate
    /// (candidate-info before a `NotarizeVote`, notar-cert before a
    /// `FinalizeVote`; `SkipVote` has no prerequisite), and re-enters here the
    /// moment durability is satisfied — inline for the dedup-hit / `SkipVote`
    /// fast path, or from the async-DB registry continuation otherwise. The wait
    /// orchestration stays on `SessionProcessor` because it crosses the
    /// `DatabaseController` registry; the controller owns the consensus-meaningful
    /// outbound action below: sign, persist, send.
    ///
    /// First-notarize-vote latency is sampled here (not in the shell) so the
    /// timestamp reflects the moment the vote is actually sent — the meaningful
    /// end-of-latency point, matching the pre-refactor ordering where the
    /// tracking ran after the (synchronous) wait succeeded.
    ///
    /// Reference: C++ `pool.cpp::handle(BroadcastVote)` → `handle_our_vote(...)`:
    /// sign, persist (db actor), publish `OutgoingProtocolMessage`.
    pub(crate) fn broadcast_vote_after_persist(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        vote: Vote,
    ) {
        // Track first notarize vote in this slot (stage 2 latency).
        if let Vote::Notarize(v) = &vote {
            let vote_slot = v.slot;
            if !backend.runtime().first_candidate_notarized(vote_slot) {
                let now = self.now();
                backend.runtime_mut().set_first_candidate_notarized(vote_slot, true, now);
                if let Ok(latency) =
                    now.duration_since(backend.runtime().started_at(vote_slot, now))
                {
                    self.telemetry
                        .first_candidate_notarized_latency_histogram
                        .record(latency.as_millis() as f64);
                    log::trace!(
                        "Session {}: first notarize vote in {:.3}ms",
                        &self.session_id().to_hex_string()[..8],
                        latency.as_secs_f64() * 1000.0
                    );
                }
            }
        }

        // Sign the vote with the session-scoped signature.
        let signed_vote =
            match sign_vote(&vote, self.session_id(), self.description.get_local_key()) {
                Ok(v) => v.only(), // Extract inner Vote from Vote_
                Err(e) => {
                    log::error!(
                        "Session {} broadcast_vote: failed to sign vote: {}",
                        self.session_id().to_hex_string(),
                        e
                    );
                    self.telemetry.increment_error();
                    return;
                }
            };

        backend.persist_our_vote(&signed_vote);

        log::trace!(
            "Session {} broadcast_vote: sending signed vote",
            self.session_id().to_hex_string()
        );

        // Send via receiver to all validators (serializes + broadcasts + loopback).
        backend.send_vote(signed_vote);
    }
}

// ======================================================================
// Masterchain applied-top pipeline
// ======================================================================
// Validator-manager finalization: advance the accepted-normal head and apply
// the masterchain finalized block.
impl ConsensusController {
    /// Advance the accepted-normal-head seqno fallback monotonically, clearing a
    /// now-stale exact block id when the seqno moves past it.
    fn advance_accepted_normal_head_seqno(&mut self, seqno: u32) {
        if seqno > self.accepted_normal_head_seqno {
            self.accepted_normal_head_seqno = seqno;
            if self.accepted_normal_head_block_id.as_ref().is_some_and(|b| b.seq_no < seqno) {
                self.accepted_normal_head_block_id = None;
            }
        }
    }

    /// Advance the accepted-normal-head cursor (seqno + exact block id) to
    /// `block_id`. Used by [`Self::set_mc_finalized_block`], the recursive walk
    /// ([`Self::maybe_apply_finalized_state`]), and restart recovery.
    pub(crate) fn advance_accepted_normal_head_block(&mut self, block_id: BlockIdExt) {
        self.advance_accepted_normal_head_seqno(block_id.seq_no);
        match self.accepted_normal_head_block_id.as_ref() {
            Some(current) if current >= &block_id => {}
            _ => self.accepted_normal_head_block_id = Some(block_id),
        }
    }

    /// Update applied-top tracking from a manager notification.
    ///
    /// Called when the manager forwards the current applied top for this session
    /// shard. Mirrors the C++ external notify path:
    /// - `block-producer.cpp::handle(BlockFinalizedInMasterchain)` updates
    ///   `last_mc_finalized_seqno_` and `last_consensus_finalized_seqno_`
    /// - `block-validator.cpp::handle(BlockFinalizedInMasterchain)` advances the
    ///   exact accepted head when `seqno != 0`
    ///
    /// The seqno feeds `should_generate_empty_block()` and the exact block id
    /// seeds the accepted-head cursor when known. The wake horizon is lowered
    /// through the borrowing [`ConsensusBackend::runtime_mut`] so the session
    /// main loop re-evaluates promptly.
    pub(crate) fn set_mc_finalized_block(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        applied_top: BlockIdExt,
    ) {
        self.telemetry.note_mc_applied_top(applied_top.clone());
        let session_shard = self.description.get_shard();
        if applied_top.shard() != session_shard {
            log::trace!(
                "Session {}: ignoring MC finalization update for mismatched shard {} \
                (session shard {})",
                &self.session_id().to_hex_string()[..8],
                applied_top.shard(),
                session_shard
            );
            return;
        }
        let seqno = applied_top.seq_no;
        log::trace!(
            "Session {}: set_applied_top_seqno={} (was {:?})",
            &self.session_id().to_hex_string()[..8],
            seqno,
            self.last_mc_finalized_seqno
        );
        // Keep last_mc_finalized_seqno monotonic, mirroring C++ behavior:
        // last_mc_finalized_seqno_ = std::max(event->block.seqno(), last_mc_finalized_seqno_);
        let prev_mc = self.last_mc_finalized_seqno.unwrap_or(0);
        let updated_mc = seqno.max(prev_mc);
        self.last_mc_finalized_seqno = Some(updated_mc);

        // C++ block-producer.cpp parity:
        // last_consensus_finalized_seqno_ = std::max(last_mc_finalized_seqno_,
        //                                            last_consensus_finalized_seqno_);
        let prev_consensus = self.last_consensus_finalized_seqno.unwrap_or(0);
        self.last_consensus_finalized_seqno = Some(updated_mc.max(prev_consensus));

        // C++ block-validator.cpp ignores seqno 0 on external notify, so only seed
        // the exact accepted head when the applied top is a non-zerostate block.
        if seqno != 0 {
            self.advance_accepted_normal_head_block(applied_top);
        } else {
            self.advance_accepted_normal_head_seqno(seqno);
        }
        let now = self.now();
        backend.runtime_mut().set_next_awake_time(now);
    }
}

// ======================================================================
// FSM finalization / certificate handlers
// ======================================================================
// Certificate-observed + finalization-driven events dispatched by
// SessionProcessor::process_simplex_events (which stays the pump).
impl ConsensusController {
    /// Handle block finalized event.
    ///
    /// Records the trigger in `finalized_pending_body` (kept until recursive
    /// parent-chain materialization succeeds end-to-end) and kicks the
    /// recursive walk. Always processes (never blocks FSM event processing); if
    /// bodies are missing, materialization is deferred until they arrive.
    pub(crate) fn handle_block_finalized(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        event: BlockFinalizedEvent,
    ) {
        check_execution_time!(50_000);
        instrument!();

        let slot = event.slot;
        let block_hash = &event.block_hash;
        let finalized_id = RawCandidateId { slot, hash: block_hash.clone() };

        // INVARIANT CHECK - Certificate must have sufficient weight (>=2/3+1).
        // Reference: C++ pool.cpp - certificate is only created when threshold is reached.
        let certificate = &event.certificate;
        let cert_weight = certificate.total_weight(&self.description);
        let total_weight = self.description.get_total_weight();
        let threshold = threshold_66(total_weight);
        debug_assert!(
            cert_weight >= threshold,
            "ConsensusController INVARIANT VIOLATION: finalization certificate weight {} \
            is below threshold {} (total={}). This should never happen - FSM only emits \
            BlockFinalized when threshold is reached.",
            cert_weight,
            threshold,
            total_weight
        );
        if cert_weight < threshold {
            log::error!(
                "Session {} handle_block_finalized: INVARIANT VIOLATION: certificate weight {} \
                below threshold {} (total={})",
                &self.session_id().to_hex_string()[..8],
                cert_weight,
                threshold,
                total_weight
            );
            self.telemetry.increment_error();
        }

        // Always keep a pending entry until recursive parent-chain materialization
        // succeeds end-to-end. This provides deterministic retries on late body/cert
        // arrivals and ensures ancestor parity is eventually reached.
        let entry = FinalizedEntry { event: event.clone(), finalized_at: self.now() };
        self.finalized_pending_body.insert(finalized_id.clone(), entry);
        self.telemetry.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);

        log::debug!(
            "Session {} FINALIZED: slot={}, hash={} - recorded in journal, weight={}/{} ({:.0}%)",
            &self.session_id().to_hex_string()[..8],
            slot,
            &block_hash.to_hex_string()[..8],
            cert_weight,
            total_weight,
            100.0 * cert_weight as f64 / total_weight as f64
        );

        // Note: Certificate caching for standstill is handled in handle_finalization_reached()
        // which is triggered by SimplexEvent::FinalizationReached (emitted after BlockFinalized).

        // Attempt recursive parent-chain finalization immediately.
        self.retry_pending_recursive_finalization_for(backend, std::slice::from_ref(&finalized_id));

        // Continue FSM event processing (do NOT push event back to queue)
    }

    /// Handle notarization reached event.
    ///
    /// Records the milestone, proactively requests a missing candidate body,
    /// emits candidate-observed for a present non-empty body, then persists +
    /// relays the notarization certificate (via the synchronous backend
    /// effect). Caching VoteSignatureSet bytes matches the C++ wire format.
    pub(crate) fn handle_notarization_reached(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        event: NotarizationReachedEvent,
    ) {
        check_execution_time!(1_000);

        let now = self.now();
        self.telemetry.record_notarization_milestone(now, event.slot);

        log::trace!(
            "Session {} notarization reached: slot={} block={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            &event.block_hash.to_hex_string()[..8],
            event.certificate.signatures.len()
        );

        let candidate_id = RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };

        // If we learned notarization via foreign votes/cert but the candidate body is missing,
        // proactively request it. Otherwise, the next leader may be unable to collate due to
        // unresolved parent chain, causing timeouts and skip cascades in single-host tests.
        if !backend.candidate_book().has_real_body(&candidate_id) {
            backend.request_candidate(event.slot, event.block_hash.clone(), None);
        }

        // Extract owned candidate fields, releasing the book borrow before the
        // mutable persist effect runs.
        let observed = backend
            .candidate_book()
            .received(&candidate_id)
            .filter(|received| !received.is_empty)
            .map(|received| {
                (
                    received.block_id.clone(),
                    received.data.clone(),
                    received.collated_data.clone(),
                    received.source_idx,
                )
            });
        if let Some((block_id, data, collated_data, source_idx)) = observed {
            let observed_flags = CandidateObservedFlags {
                body_present: true,
                parent_ready: true,
                local_collated: source_idx == self.description.get_self_idx(),
            };
            self.callbacks.notify_candidate_observed(block_id, data, collated_data, observed_flags);
        }

        backend.persist_notar_cert_then_relay(&event);
    }

    /// Handle skip certificate reached event.
    ///
    /// Persists + broadcasts the skip certificate to all validators (via the
    /// synchronous backend effect). Reference: C++ pool.cpp creates a skip
    /// certificate and broadcasts it.
    pub(crate) fn handle_skip_certificate_reached(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        event: SkipCertificateReachedEvent,
    ) {
        check_execution_time!(1_000);

        log::trace!(
            "Session {} skip certificate reached: slot={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            event.certificate.signatures.len()
        );

        backend.persist_skip_cert_then_relay(&event);
    }

    /// Handle finalization reached event.
    ///
    /// Records the milestone, then persists + relays the finalization
    /// certificate and updates standstill tracking (via the synchronous backend
    /// effect). Reference: C++ parity handle_saved_certificate.
    pub(crate) fn handle_finalization_reached(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        event: FinalizationReachedEvent,
    ) {
        check_execution_time!(1_000);

        let now = self.now();
        self.telemetry.record_final_cert_milestone(now, event.slot);

        log::trace!(
            "Session {} finalization reached: slot={} block={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            &event.block_hash.to_hex_string()[..8],
            event.certificate.signatures.len()
        );

        backend.persist_final_cert_then_relay(&event);
    }
}

// ======================================================================
// Recursive parent-chain finalization walk
// ======================================================================
// Materialize finalized state up the parent chain and emit the at-most-once
// on_block_finalized callback.
impl ConsensusController {
    /* Walk & application */

    /// Retry all triggers currently stored in `finalized_pending_body`.
    ///
    /// Broad "scan everything pending" entrypoint used from periodic / auxiliary
    /// paths (candidate arrivals, notar cert arrivals, etc), reached from
    /// `SessionProcessor::retry_pending_recursive_finalization`. Sorted by
    /// `(slot, hash)` to keep retries deterministic across runs.
    pub(crate) fn retry_pending_recursive_finalization(
        &mut self,
        backend: &mut dyn ConsensusBackend,
    ) {
        // Snapshot current trigger ids from the pending journal.
        let mut pending_ids: Vec<RawCandidateId> =
            self.finalized_pending_body.keys().cloned().collect();
        if pending_ids.is_empty() {
            return;
        }

        // Stable order helps reproducible diagnostics/callback behavior.
        pending_ids.sort_by(|left, right| {
            left.slot
                .value()
                .cmp(&right.slot.value())
                .then_with(|| left.hash.as_slice().cmp(right.hash.as_slice()))
        });

        self.retry_pending_recursive_finalization_for(backend, &pending_ids);
    }

    /// Retry recursive finalization for an explicit set of trigger ids.
    ///
    /// Completed triggers are removed from the pending journal; unresolved ones
    /// remain queued and will be retried when missing data/certs arrive.
    fn retry_pending_recursive_finalization_for(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        pending_ids: &[RawCandidateId],
    ) {
        if pending_ids.is_empty() {
            return;
        }

        // Collect completed triggers first; remove after the pass.
        let mut completed = Vec::new();
        for trigger_id in pending_ids {
            // Clone event payload to avoid holding a map borrow across processing.
            let Some(event) =
                self.finalized_pending_body.get(trigger_id).map(|entry| entry.event.clone())
            else {
                continue;
            };

            if self.try_finalize_recursive_chain(backend, trigger_id, &event) {
                completed.push(trigger_id.clone());
            }
        }

        if completed.is_empty() {
            return;
        }

        for trigger_id in completed {
            self.finalized_pending_body.remove(&trigger_id);
        }
        self.telemetry.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);
    }

    /// Incrementally resolve finalized trigger + parent chain.
    ///
    /// Processing model:
    /// - The trigger is evaluated immediately (out-of-order friendly).
    /// - Each ancestor is handled independently in the same walk.
    /// - Missing data/certs for one block do not block already-ready blocks.
    ///
    /// Return value:
    /// - `true`  => all currently relevant blocks are resolved/emitted/applied.
    /// - `false` => at least one block still waits for requested data/certs.
    ///
    /// C++ parity (`consensus.cpp::finalize_blocks_inner`):
    /// The `FinalCert` context (`maybe_final_cert`) flows through empty candidates
    /// unchanged and is consumed by the first non-empty candidate. On masterchain,
    /// once the cert is consumed the walk stops (null cert + MC → early return).
    fn try_finalize_recursive_chain(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        trigger_id: &RawCandidateId,
        event: &BlockFinalizedEvent,
    ) -> bool {
        let applied_floor = self.last_mc_finalized_seqno.unwrap_or(0);
        let mut complete = true;
        let mut visited = HashSet::new();
        let mut depth = 0u32;
        let mut depth_warned = false;
        let mut current_id = trigger_id.clone();

        // C++ parity: `maybe_final_cert` flows through empty candidates and is
        // consumed (dropped to null) by the first non-empty candidate.
        let mut has_final_cert = true;

        // Capture trigger's slot and candidate hash data for FinalCert signature
        // verification context (C++ `maybe_final_candidate->id.slot` /
        // `maybe_final_candidate->hash_data()`).
        let trigger_slot = trigger_id.slot;
        let trigger_candidate_hash_data: Vec<u8> = backend
            .candidate_book()
            .received(trigger_id)
            .map(|r| r.candidate_hash_data_bytes.clone())
            .unwrap_or_default();

        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                depth_warned = true;
                log::warn!(
                    "Session {} recursive finalization: deep chain depth={} \
                     (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    depth,
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    "Session {} recursive finalization: exceeded MAX_CHAIN_DEPTH={} \
                     while resolving trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    MAX_CHAIN_DEPTH,
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.telemetry.increment_error();
                return true;
            }
            if !visited.insert(current_id.clone()) {
                log::error!(
                    "Session {} recursive finalization: parent-cycle detected at s{}:{} \
                     while resolving trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.telemetry.increment_error();
                return true;
            }

            // Step 1: resolve candidate metadata/body holder.
            let Some(received) = backend.candidate_book().received(&current_id).cloned() else {
                log::debug!(
                    "Session {} recursive finalization: missing candidate metadata for \
                     s{}:{}, requesting body+cert and deferring trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                backend.request_candidate(
                    current_id.slot,
                    current_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                complete = false;
                break;
            };

            // C++ parity (`consensus.cpp`):
            // if (maybe_final_cert.is_null() && shard.is_masterchain()) { co_return; }
            // On MC, once the final cert is consumed, stop the walk.
            if self.description.get_shard().is_masterchain() && !has_final_cert {
                if !received.is_empty && !self.finalized_delivery_sent.contains(&current_id) {
                    self.finalized_delivery_sent.insert(current_id.clone());
                }
                break;
            }

            // Step 2: evaluate local readiness + floor constraints for this block only.
            // C++ parity: cert-carrying walks bypass the floor check because the
            // recursive finalization in C++ has no applied-floor guard.
            let block_below_applied_floor =
                !has_final_cert && received.block_id.seq_no() < applied_floor;
            let body_available = !received.candidate_hash_data_bytes.is_empty();

            if body_available {
                // Step 3: persist/apply local finalized state before listener callback emission.
                if !self.maybe_apply_finalized_state(backend, &current_id, has_final_cert) {
                    complete = false;
                    break;
                }

                // Step 4: callback emission policy + signature mode is handled per block.
                self.try_emit_recursive_finalized_callback(
                    backend,
                    &current_id,
                    &received,
                    event,
                    has_final_cert,
                    trigger_slot,
                    &trigger_candidate_hash_data,
                    &mut complete,
                );
            } else if !block_below_applied_floor {
                log::debug!(
                    "Session {} recursive finalization: candidate body missing for s{}:{}, \
                     requesting and deferring trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                backend.request_candidate(
                    current_id.slot,
                    current_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                complete = false;
            }

            // Step 5: stop climbing once we reached already-applied floor.
            if block_below_applied_floor {
                if !received.is_empty && !self.finalized_delivery_sent.contains(&current_id) {
                    log::debug!(
                        "Session {} recursive finalization: stopping parent walk below applied-top \
                         floor for block_id={} seqno={} floor={}",
                        &self.session_id().to_hex_string()[..8],
                        received.block_id,
                        received.block_id.seq_no(),
                        applied_floor,
                    );
                    self.finalized_delivery_sent.insert(current_id.clone());
                }
                break;
            }

            // C++ parity: final cert context flows through empty candidates,
            // is consumed by the first non-empty candidate.
            // `if (is_empty) { recurse(parent, maybe_final_cert); }`
            // `else { recurse(parent, {}); }`
            if !received.is_empty {
                has_final_cert = false;
            }

            let Some(parent_id) = received.parent_id else {
                break;
            };

            current_id = parent_id;
        }

        complete
    }

    /// Attempt to emit `on_block_finalized()` for one block in recursive walk.
    ///
    /// C++ parity (`consensus.cpp::finalize_blocks_inner`):
    /// - Empty candidates never get `BlockFinalized` / `do_finalize_block`.
    /// - Non-empty candidates with `maybe_final_cert` use FinalCert signatures
    ///   and the **trigger** candidate's slot/hash_data for the signature set.
    /// - Non-empty candidates without cert use NotarCert signatures and their own
    ///   slot/hash_data.
    ///
    /// `complete` is set to `false` when dependencies are missing and were requested.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_emit_recursive_finalized_callback(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        candidate_id: &RawCandidateId,
        received: &ReceivedCandidate,
        event: &BlockFinalizedEvent,
        has_final_cert: bool,
        trigger_slot: SlotIndex,
        trigger_candidate_hash_data: &[u8],
        complete: &mut bool,
    ) {
        // C++ parity: empty candidates never emit BlockFinalized / do_finalize_block.
        if received.is_empty || self.finalized_delivery_sent.contains(candidate_id) {
            return;
        }

        // Protocol invariant: at most one finalized callback per seqno for a
        // *distinct* block. Two callbacks for the same `block_id` are idempotent
        // (typically a recursive parent-chain walk re-entering an ancestor whose
        // slot-keyed dedup entry was already pruned by `cleanup_old_slots`) and
        // must NOT panic. Only a different `block_id` at the same seqno is a real
        // protocol breach.
        let seqno = received.block_id.seq_no();
        if let Some(existing) = self.finalized_delivery_sent_seqno.get(&seqno) {
            if existing.block_id == received.block_id {
                log::debug!(
                    "Session {} recursive finalization: idempotent re-entry for \
                     block_id={} seqno={} previous_slot={} current_slot={} (slot dedup pruned)",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                    existing.slot,
                    candidate_id.slot,
                );
                // Re-insert the candidate_id so subsequent walks short-circuit at
                // the cheaper dedup before reaching here.
                self.finalized_delivery_sent.insert(candidate_id.clone());
                return;
            }
            assert!(
                false,
                "Session {} protocol breach: multiple finalized callbacks for seqno={} \
                 existing_block_id={} new_block_id={}",
                &self.session_id().to_hex_string()[..8],
                seqno,
                existing.block_id,
                received.block_id,
            );
        }

        // Apply shard/MC callback emission policy for this block.
        // C++ parity: on MC, emit only when final cert context is present.
        let applied_floor = self.last_mc_finalized_seqno.unwrap_or(0);
        let should_emit = self.should_emit_on_block_finalized_for_block(
            &received.block_id,
            has_final_cert,
            applied_floor,
        );

        if !should_emit {
            if self.description.get_shard().is_masterchain() {
                assert!(
                    !has_final_cert,
                    "Session {} invariant breach: masterchain FinalCert callback unexpectedly suppressed \
                     for block_id={} seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                );
                log::debug!(
                    "Session {} recursive finalization: skip masterchain ancestor callback \
                     without FinalCert context for block_id={} seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                );
            } else {
                log::warn!(
                    "Session {} recursive finalization: skip callback below applied-top \
                     floor for block_id={} seqno={} floor={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                    applied_floor,
                );
            }
            self.finalized_delivery_sent.insert(candidate_id.clone());
            return;
        }

        // C++ parity: FinalCert context → use FinalCert signatures from the trigger
        // event; otherwise use NotarCert from this candidate's slot.
        let signatures = if has_final_cert {
            self.build_finalization_raw_signatures(&event.certificate.signatures)
        } else {
            let Some(notar_cert) = backend
                .simplex_state()
                .get_notarize_certificate(candidate_id.slot, &candidate_id.hash)
            else {
                log::debug!(
                    "Session {} recursive finalization: missing ancestor NotarCert for \
                     s{}:{}, requesting and deferring callback",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                );
                backend.request_candidate(
                    candidate_id.slot,
                    candidate_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                *complete = false;
                return;
            };
            self.build_finalization_raw_signatures(&notar_cert.signatures)
        };

        // C++ parity: when using FinalCert, the signature set references the
        // *trigger* candidate's slot/hash_data, not the current ancestor's.
        let (sig_slot, sig_hash_data) = if has_final_cert {
            (trigger_slot, trigger_candidate_hash_data.to_vec())
        } else {
            (received.slot, received.candidate_hash_data_bytes.clone())
        };

        let source_info = self.build_source_info_for_received(received);
        let delivered = self.notify_block_finalized(
            received.block_id.clone(),
            source_info,
            received.root_hash.clone(),
            received.file_hash.clone(),
            received.data.clone(),
            signatures,
            Vec::new(),
            sig_slot,
            sig_hash_data,
            has_final_cert,
        );
        if self.description.get_shard().is_masterchain() && has_final_cert {
            assert!(
                delivered,
                "Session {} protocol breach: failed to emit masterchain FinalCert callback \
                 for block_id={} seqno={}",
                &self.session_id().to_hex_string()[..8],
                received.block_id,
                seqno,
            );
        }
        if delivered {
            let now = self.now();
            self.telemetry.record_self_collation_acceptance(
                candidate_id,
                &received.block_id,
                has_final_cert,
                &self.description,
                now,
            );
            self.finalized_delivery_sent.insert(candidate_id.clone());
            let previous = self.finalized_delivery_sent_seqno.insert(
                seqno,
                FinalizedSeqnoRecord {
                    slot: candidate_id.slot,
                    block_id: received.block_id.clone(),
                },
            );
            assert!(
                previous.is_none(),
                "Session {} protocol breach: duplicate finalized callback seqno={} \
                 previous_block_id={} new_block_id={}",
                &self.session_id().to_hex_string()[..8],
                seqno,
                previous.map(|r| r.block_id).unwrap_or_else(|| received.block_id.clone()),
                received.block_id,
            );
        } else {
            *complete = false;
        }
    }

    /// Apply finalized-driven local state once the candidate body is available.
    ///
    /// Updates local finalized/head cursors, persists finalized records for
    /// restart recovery (via [`ConsensusBackend::persist_finalized_block`]), and
    /// clears per-slot runtime once finalization is materially applied.
    ///
    /// Return semantics:
    /// * `true`  — local state applied (and, on MC, persist registered with the
    ///             registry; the actual write completes asynchronously).
    /// * `false` — synchronous persist registration failed; the in-memory state
    ///             was NOT applied and the caller's retry mechanism
    ///             (`finalized_pending_body`) keeps the trigger queued.
    pub(crate) fn maybe_apply_finalized_state(
        &mut self,
        backend: &mut dyn ConsensusBackend,
        finalized_id: &RawCandidateId,
        is_final: bool,
    ) -> bool {
        if self.finalized_blocks.contains(finalized_id) {
            return true;
        }

        let Some(received) = backend.candidate_book().received(finalized_id).cloned() else {
            return true;
        };

        if received.candidate_hash_data_bytes.is_empty() {
            return true;
        }

        let slot = received.slot;
        let seqno = received.block_id.seq_no();

        let slot_started_at = backend.runtime().started_at(slot, self.now());
        if let Ok(duration) = self.now().duration_since(slot_started_at) {
            self.telemetry.slot_duration_histogram.record(duration.as_millis() as f64);
        }

        if !backend.runtime().first_candidate_finalized(slot) {
            let now = self.now();
            backend.runtime_mut().set_first_candidate_finalized(slot, true, now);
            if let Ok(latency) = now.duration_since(slot_started_at) {
                self.telemetry
                    .first_candidate_finalized_latency_histogram
                    .record(latency.as_millis() as f64);
            }
        }

        let record = FinalizedBlockRecord {
            candidate_id: finalized_id.clone(),
            block_id: received.block_id.clone(),
            parent: received.parent_id.clone(),
            is_final,
        };

        if !backend.persist_finalized_block(record) {
            return false;
        }

        let previous_head_seqno = self.finalized_head_seqno.unwrap_or(0);
        let should_replace_head = match self.finalized_head_slot {
            Some(current_slot) => {
                seqno > previous_head_seqno
                    || (seqno == previous_head_seqno && slot >= current_slot)
            }
            None => true,
        };

        if !received.is_empty {
            if seqno > previous_head_seqno {
                self.finalized_head_seqno = Some(seqno);
            }

            self.advance_accepted_normal_head_block(received.block_id.clone());

            if should_replace_head {
                if let Ok(before_split) =
                    crate::utils::extract_before_split_flag(received.data.data())
                {
                    self.finalized_head_before_split = before_split;
                    self.before_split_by_block_id.insert(received.block_id.clone(), before_split);
                    if before_split {
                        log::info!(
                            "Session {} maybe_apply_finalized_state: block at slot={slot} seqno={seqno} has \
                            before_split=true (next block MUST be empty for split/merge)",
                            &self.session_id().to_hex_string()[..8],
                        );
                    }
                } else {
                    log::trace!(
                        "Session {} maybe_apply_finalized_state: failed to extract before_split flag for \
                        slot={slot}, assuming false",
                        &self.session_id().to_hex_string()[..8],
                    );
                    self.finalized_head_before_split = false;
                }
            }
        }

        if should_replace_head {
            self.finalized_head_slot = Some(slot);
            self.finalized_head_block_id = Some(received.block_id.clone());
        }

        if seqno > self.last_consensus_finalized_seqno.unwrap_or(0) {
            self.last_consensus_finalized_seqno = Some(seqno);
        }

        self.finalized_blocks.insert(finalized_id.clone());

        self.telemetry.last_finalized_slot_gauge.set(slot.0 as f64);
        let now = self.now();
        self.telemetry.set_round_debug_at(now + ROUND_DEBUG_PERIOD);
        self.telemetry.set_last_finalization_time(now);

        if backend.simplex_state().is_slot_progressed(&self.description, slot) {
            backend.reset_slot_state(slot);
        } else {
            log::trace!(
                "Session {} maybe_apply_finalized_state: skipping reset_slot_state for \
                non-progressed slot={slot}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        true
    }

    /* Finalized-callback emission helpers */

    /// Shard/MC callback-emission policy: on MC emit only with FinalCert
    /// context; on shardchains emit only at/above the applied-top floor.
    fn should_emit_on_block_finalized_for_block(
        &self,
        block_id: &BlockIdExt,
        has_final_cert: bool,
        applied_floor: u32,
    ) -> bool {
        if self.description.get_shard().is_masterchain() {
            return has_final_cert;
        }
        block_id.seq_no() >= applied_floor
    }

    /// Emit `on_block_finalized` to the listener for finalized-driven acceptance.
    #[allow(clippy::too_many_arguments)]
    fn notify_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        slot: SlotIndex,
        candidate_hash_data_bytes: Vec<u8>,
        is_final: bool,
    ) -> bool {
        check_execution_time!(20_000);

        let signatures_variant = match self.build_simplex_signatures_variant(
            &signatures,
            slot,
            candidate_hash_data_bytes,
            is_final,
        ) {
            Ok(v) => v,
            Err(e) => {
                log::error!(
                    "Session {} notify_block_finalized: failed to build signatures variant: {}",
                    self.session_id().to_hex_string(),
                    e
                );
                self.telemetry.increment_error();
                return false;
            }
        };

        self.callbacks.notify_block_finalized(
            block_id,
            source_info,
            root_hash,
            file_hash,
            data,
            signatures_variant,
            approve_signatures,
        );
        true
    }

    fn build_finalization_raw_signatures(
        &self,
        signatures: &[crate::certificate::VoteSignature],
    ) -> Vec<(PublicKeyHash, BlockPayloadPtr)> {
        signatures
            .iter()
            .map(|s| {
                (
                    self.description.get_source_public_key_hash(s.validator_idx).clone(),
                    consensus_common::ConsensusCommonFactory::create_block_payload(
                        s.signature.clone(),
                    ),
                )
            })
            .collect()
    }

    fn build_source_info_for_received(&self, received: &ReceivedCandidate) -> BlockSourceInfo {
        let source_public_key = self.description.get_source_public_key(received.source_idx).clone();
        BlockSourceInfo {
            source: source_public_key,
            priority: BlockCandidatePriority {
                round: SIMPLEX_ROUNDLESS,
                priority: 0,
                first_block_round: SIMPLEX_ROUNDLESS,
            },
        }
    }

    /// Build `BlockSignaturesVariant::Simplex` from raw signature pairs with context
    /// (session_id, slot, candidate_data, is_final) for accept_block verification.
    ///
    /// # Invariants (checked with assert)
    /// - raw_signatures must not be empty
    /// - candidate_hash_data_bytes must not be empty
    /// - All signatures must have valid format (64 bytes for Ed25519)
    /// - Total weight must meet threshold_66 for finalized blocks
    fn build_simplex_signatures_variant(
        &self,
        raw_signatures: &[(PublicKeyHash, BlockPayloadPtr)],
        slot: SlotIndex,
        candidate_hash_data_bytes: Vec<u8>,
        is_final: bool,
    ) -> Result<BlockSignaturesVariant> {
        // INVARIANT: Must have at least one signature
        assert!(
            !raw_signatures.is_empty(),
            "build_simplex_signatures_variant: raw_signatures must not be empty for slot={}",
            slot
        );

        // INVARIANT: candidate_hash_data_bytes must not be empty (needed for signature verification)
        assert!(
            !candidate_hash_data_bytes.is_empty(),
            "build_simplex_signatures_variant: candidate_hash_data_bytes must not be empty for slot={}",
            slot
        );

        let mut pure_signatures = BlockSignaturesPure::new();
        let mut valid_sig_count = 0u32;
        let mut invalid_sig_count = 0u32;

        // Calculate total weight and add signature pairs
        let mut total_weight: u64 = 0;
        for (node_id, sig_payload) in raw_signatures {
            // Get validator weight by looking up the source index
            if let Ok(src_idx) = self.description.get_source_index(node_id) {
                total_weight += self.description.get_node_weight(src_idx);
            }

            // Convert raw signature bytes to CryptoSignaturePair
            let sig_bytes = sig_payload.data().to_vec();
            if sig_bytes.len() >= 64 {
                let mut r = [0u8; 32];
                let mut s = [0u8; 32];
                r.copy_from_slice(&sig_bytes[0..32]);
                s.copy_from_slice(&sig_bytes[32..64]);

                pure_signatures.add_sigpair(CryptoSignaturePair {
                    node_id_short: (*node_id.data()).into(),
                    sign: CryptoSignature::with_r_s(&r, &s),
                });
                valid_sig_count += 1;
            } else {
                invalid_sig_count += 1;
                log::warn!(
                    "build_simplex_signatures_variant: invalid signature length {} for node \
                    {node_id} at slot={slot}",
                    sig_bytes.len(),
                );
            }
        }

        // INVARIANT: All signatures must be valid (no invalid signatures)
        assert!(
            invalid_sig_count == 0,
            "build_simplex_signatures_variant: {} invalid signatures found for slot={} (valid={})",
            invalid_sig_count,
            slot,
            valid_sig_count
        );

        // INVARIANT: Must have at least one valid signature added
        assert!(
            valid_sig_count > 0,
            "build_simplex_signatures_variant: no valid signatures added for slot={}",
            slot
        );

        // INVARIANT: For finalized blocks, total weight must meet threshold_66
        let threshold = threshold_66(self.description.get_total_weight());
        assert!(
            total_weight >= threshold,
            "build_simplex_signatures_variant: total_weight {} < threshold {} for slot={} (is_final={})",
            total_weight,
            threshold,
            slot,
            is_final
        );

        pure_signatures.set_weight(total_weight);

        log::trace!(
            "build_simplex_signatures_variant: slot={} sigs={} weight={}/{} ({:.1}%)",
            slot,
            valid_sig_count,
            total_weight,
            self.description.get_total_weight(),
            100.0 * total_weight as f64 / self.description.get_total_weight() as f64
        );

        // Build BlockSignaturesSimplex with full context for signature verification
        let candidate_data =
            BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_hash_data_bytes)?;
        let simplex_signatures = BlockSignaturesSimplex::with_params(
            ValidatorBaseInfo::with_params(0, 0), // Placeholder - will be replaced in accept_block
            pure_signatures,
            self.session_id().clone(),
            slot.value() as u32,
            candidate_data,
            is_final,
        );

        Ok(BlockSignaturesVariant::Simplex(simplex_signatures))
    }

    /*
        Receiver ingress (vote / certificate)

        Network-delivered votes and certificates arrive here from
        `SessionProcessor::on_vote` / `on_certificate` (the `ReceiverListener`
        callbacks), each building a `ConsensusBackend` split-borrow and
        delegating. `SimplexState` stays owned by `SessionProcessor` (the shared
        kernel); both methods reach it only through
        `ConsensusBackend::simplex_state{,_mut}` for the borrow scope.

        Both return whether the caller should pump `check_all()` — `true` only
        when the FSM actually advanced (a newly applied vote / a verified
        certificate). Duplicate / late / rejected / misbehaving ingress returns
        `false` so the main loop is not pumped, preserving the pre-refactor
        control flow exactly.
    */
}

// ======================================================================
// Tests
// ======================================================================
// Test-only field seams consolidated under one `#[cfg(test)]` impl so the
// production impls carry no test scaffolding (called from the `#[path]`
// session-processor unit tests; production fields stay private).
#[cfg(test)]
impl ConsensusController {
    /* Callback injection */

    /// Re-point the controller at a rebuilt callbacks aspect.
    ///
    /// Test-only seam used by `SessionProcessor::set_listener_for_test`,
    /// mirroring `CollationController` / `ValidationController`.
    pub(crate) fn set_callbacks_for_test(&mut self, callbacks: Arc<SessionCallbacks>) {
        self.callbacks = callbacks;
    }

    /* Finalization-journal field seams */

    pub(crate) fn set_finalized_head_seqno(&mut self, value: Option<u32>) {
        self.finalized_head_seqno = value;
    }

    pub(crate) fn set_finalized_head_slot(&mut self, value: Option<SlotIndex>) {
        self.finalized_head_slot = value;
    }

    pub(crate) fn set_finalized_head_block_id(&mut self, value: Option<BlockIdExt>) {
        self.finalized_head_block_id = value;
    }

    pub(crate) fn set_finalized_head_before_split(&mut self, value: bool) {
        self.finalized_head_before_split = value;
    }

    pub(crate) fn finalized_pending_body_is_empty(&self) -> bool {
        self.finalized_pending_body.is_empty()
    }

    pub(crate) fn finalized_pending_body_contains(&self, id: &RawCandidateId) -> bool {
        self.finalized_pending_body.contains_key(id)
    }

    pub(crate) fn finalized_delivery_sent_contains(&self, id: &RawCandidateId) -> bool {
        self.finalized_delivery_sent.contains(id)
    }

    pub(crate) fn finalized_delivery_sent_seqno_contains(&self, seqno: u32) -> bool {
        self.finalized_delivery_sent_seqno.contains_key(&seqno)
    }

    /// Seed a `finalized_pending_body` entry (FinalCert observed, body pending)
    /// without driving the FSM. Builds the private [`FinalizedEntry`] internally
    /// so the type stays module-private.
    pub(crate) fn insert_pending_body_for_test(
        &mut self,
        id: RawCandidateId,
        event: BlockFinalizedEvent,
        finalized_at: SystemTime,
    ) {
        self.finalized_pending_body.insert(id, FinalizedEntry { event, finalized_at });
        self.telemetry.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);
    }
}
