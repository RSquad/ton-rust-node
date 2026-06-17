/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `ValidationController` entity
//!
//! Per-session candidate-validation controller. Owns the validation
//! scheduling and bookkeeping state previously inlined on
//! `SessionProcessor`:
//!
//! - `pending_validations: HashMap<RawCandidateId, PendingValidation>` —
//!   block candidates received from the network awaiting validation
//!   scheduling.
//! - `pending_approve: HashSet<RawCandidateId>` — set of blocks currently
//!   being validated (awaiting callback).
//! - `pending_reject: HashMap<RawCandidateId, BlockPayloadPtr>` — blocks
//!   pending rejection (with rejection reason payload).
//! - `rejected: HashSet<RawCandidateId>` — terminal rejected set.
//! - `approved: HashMap<RawCandidateId, (SystemTime, BlockPayloadPtr)>` —
//!   approved blocks with `(validity_start_time, signature)`.
//! - `validation_attempt_map: HashMap<RawCandidateId, u32>` — block hash
//!   to validation attempt index (NOT reset per slot).
//! - `validated_candidates: VecDeque<Candidate>` — validated candidates
//!   ready for FSM submission.
//!
//! Hosts the [`PendingValidation`] inner struct that the maps reference.
//!
//! ## Validation pipeline
//!
//! The controller owns the candidate-validation pipeline end to end:
//!
//! - `register_candidate_for_validation` admits a network-received candidate
//!   (signature already verified by the receiver) into `pending_validations`;
//!   the candidate-storage half of ingress lives on `CandidateBook`.
//! - `check_validation` is the scheduling loop, run from `check_all()` via the
//!   thin `SessionProcessor::check_validation` dispatch. For each pending
//!   candidate it applies, in order: the `evaluate_wait_for_parent` parent-chain
//!   gate (C++ `pool.cpp maybe_resolve_request` `WaitForParent` parity), the
//!   per-candidate attempt limit, the min-block-interval pacing gate, and — for
//!   empty candidates — `ensure_empty_parent_tip_ready`, then hands the
//!   survivors to `try_approve_block`.
//! - `try_approve_block` auto-decides empty candidates against the resolved
//!   parent normal tip, and for normal candidates dispatches to the higher-layer
//!   validator via the owned `callbacks`, wiring the decision callback to post
//!   back through the controller's own deferred-work `queue`.
//! - `candidate_decision_ok` / `candidate_decision_fail` handle the asynchronous
//!   validator verdict (re-entered on SXMAIN through `queue`), pushing approved
//!   candidates onto `validated_candidates` or retrying / terminally rejecting
//!   failures.
//!
//! ```text
//! register_candidate_for_validation   (ingress; from on_candidate_received)
//!         │
//!         ▼  pending_validations
//! check_validation  ── WaitForParent / attempts / min-interval / empty-tip gates
//!         │
//!         ▼
//! try_approve_block ──▶ notify_candidate (higher-layer validator)
//!         │                         │ async verdict (off-thread)
//!         │                         ▼  queue.post  → SXMAIN re-entry
//!         │              candidate_decision_ok / candidate_decision_fail
//!         ▼                         │
//!  (empty auto-decide)              ▼  validated_candidates (FIFO)
//!                          SessionProcessor::process_validated_candidates
//!                                    │  (DB candidate-info durability)
//!                                    ▼
//!                          feed_validated_candidate_to_fsm → SimplexState
//! ```
//!
//! `SessionProcessor`-owned reads and `&mut self` effects the pipeline needs —
//! FSM state for the parent walk, candidate-book parent-tip resolution, parent
//! gen-utime, the wake horizon, and peer candidate requests — are reached
//! through the borrowing [`ValidationBackend`] seam (built per call by
//! `SessionProcessor::with_validation_backend`), mirroring `CollationBackend`.
//! Session id / options / clock / source keys come from the owned `description`
//! and counters from the owned `telemetry`, so neither crosses the seam.
//!
//! Draining `validated_candidates` into the FSM
//! (`process_validated_candidates` + `feed_validated_candidate_to_fsm`) stays on
//! `SessionProcessor`: it first crosses the `DatabaseController` async
//! candidate-info durability registry, and only feeds the FSM once the
//! candidateInfo is durable. The controller exposes the FIFO via
//! `push_validated` / `pop_validated`.
//!
//! ## Out of scope (deferred to Phase 7 — `ConsensusController`)
//!
//! Vote/cert ingress + outbound, FSM event handlers (`NotarizationReached`,
//! `SkipCertificateReached`, `FinalizationReached`, `BlockFinalized`),
//! recursive finalization walk, MC-applied-top tracking, and
//! `misbehavior_reports` move to `ConsensusController` in Phase 7 —
//! **not** here. Phase 5 `ValidationController` is candidate-validation-only.
//!
//! ## Boundary
//!
//! `ValidationController` owns validation-phase state and decisions only.
//! It does NOT drive collation, vote ingress, or finalization. Pipeline
//! methods take the borrowing [`ValidationBackend`] for the few
//! `SessionProcessor`-owned reads/effects they need; the controller never
//! reaches into other controllers directly. FSM submission of validated
//! candidates is left to `SessionProcessor` (see the FIFO note above).
//!
//! The immutable per-session `Arc<SessionDescription>`, the shared
//! `Arc<SessionCallbacks>`, and the `Arc<SessionTelemetry>` are held directly on
//! the controller (cloned from the same `Arc`s `SessionProcessor` holds), so
//! session id / options / timing / source-key reads, higher-layer validation
//! dispatch, and metric recording need no per-call threading. The remaining
//! `SessionProcessor`-owned dependencies are reached either through the
//! [`ValidationBackend`] seam (`check_validation` / `try_approve_block` /
//! `candidate_decision_*`) or, for the receiver-side ingress entry point
//! `register_candidate_for_validation`, passed explicitly (`&SimplexState`,
//! `&mut SessionRuntime`) so the controller stays independently constructible in
//! tests — mirroring `CollationController`.
//!
//! Callers (`SessionProcessor`, the receiver-side candidate-ingress
//! handoff) hold an inline `ValidationController` on `SessionProcessor`
//! and reach it via the accessor surface; tests included via `#[path]`
//! reach the same accessor surface and never inspect raw fields.

use crate::{
    block::{
        Candidate, CandidateParentInfo, RawCandidate, RawCandidateId, SlotIndex, ValidatorIndex,
    },
    candidate_book::ParentTipResolution,
    controller_queue::{Controlled, ControllerQueueExt, ControllerQueuePtr},
    session_callbacks::SessionCallbacks,
    session_description::SessionDescription,
    session_runtime::SessionRuntime,
    session_telemetry::SessionTelemetry,
    simplex_state::SimplexState,
    BlockCandidatePriority, BlockHash, BlockPayloadPtr, BlockSourceInfo, SessionId,
    ValidatorBlockCandidateDecisionCallback, SIMPLEX_ROUNDLESS,
};
use consensus_common::{check_execution_time, instrument};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_block::{error, Error, Result, UInt256};

/// C++ `WaitForParent`-equivalent parent-gating decision returned by
/// [`ValidationController::evaluate_wait_for_parent`].
///
/// Mirrors `pool.cpp::maybe_resolve_request` semantics:
/// - `Ready`: candidate can proceed to higher-layer validation now.
/// - `Wait`: keep candidate pending until more certificate state arrives.
/// - `Reject`: parent chain conflicts with already-established consensus
///   state; the candidate must be terminally rejected (with reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitForParentDecision {
    Ready,
    Wait,
    Reject(String),
}

/// Pending validation entry.
///
/// Tracks a block candidate that has been received from the network and
/// is awaiting validation scheduling on the local validator. Lives inside
/// `ValidationController::pending_validations`. Field visibility stays
/// `pub(crate)` so the `#[path]`-included unit tests in
/// `tests/test_validation_controller.rs` (and the existing
/// `tests/test_session_processor.rs`) can construct fixtures without
/// widening the public API.
#[derive(Debug)]
pub(crate) struct PendingValidation {
    /// The raw candidate (signature already verified by the receiver).
    pub(crate) raw_candidate: RawCandidate,
    /// Slot number this candidate targets.
    pub(crate) slot: SlotIndex,
    /// Wall-clock time when the candidate was received from the network.
    pub(crate) receive_time: SystemTime,
    /// Source validator index (leader for the slot).
    pub(crate) source_idx: ValidatorIndex,
}

/// Session-owned reads/effects the validation pipeline needs but cannot reach
/// from `&mut ValidationController` alone.
///
/// This is the controller's [`Controlled::Backend`] view: a *borrowing*,
/// drain-scoped handle built fresh from `&mut SessionProcessor` by
/// `session_processor`'s `with_validation_backend` split-borrow immediately
/// before a synchronous call (`check_validation`) or a deferred re-entry
/// (`candidate_decision_*`) runs, then dropped (RAII). Because it is rebuilt per
/// call, every read is current — no captured snapshot, no shared mutable handle.
/// The controller's own state lives on `&mut Self`; everything genuinely owned
/// by `SessionProcessor` is reached only through this trait, keeping the
/// controller (and its unit tests) free of any `SessionProcessor` dependency.
///
/// Reads available from handles the controller already holds are NOT duplicated
/// here: session id / options / clock / source keys come from the owned
/// `description`, and counters (`mark_generated_*`, `note_*_missed`,
/// `increment_error`) from the owned `telemetry`. What remains are the FSM /
/// candidate-book reads and the `SessionProcessor`-mediated effects below.
///
/// All methods are `&self`: the reads are immutable and the effects either
/// target interior-mutable handles (the wake horizon) or bounce a closure onto
/// SXMAIN (`request_candidate`, which needs `&mut SessionProcessor`).
///
/// The production view is `session_processor`'s `ValidationBackendAdapter`.
pub(crate) trait ValidationBackend {
    /// Seqno of the current finalized head, if any (`SessionProcessor`-owned;
    /// advances as blocks finalize). Used to drop validation results for
    /// candidates the chain has already finalized past.
    fn finalized_head_seqno(&self) -> Option<u32>;

    /// Lower the main-loop wake horizon to "now" so `check_all()` runs on the
    /// next iteration (e.g. once a decision lands, to act on it promptly).
    fn request_wake(&self);

    /// Lower the main-loop wake horizon to `at` so `check_all()` re-runs once
    /// that time is reached. Used by the min-block-interval pacing gate in
    /// `check_validation` to defer a candidate until its parent is old enough.
    fn request_wake_at(&self, at: SystemTime);

    /// Borrow the consensus FSM state for the read-only `WaitForParent`
    /// parent-gating walk ([`ValidationController::evaluate_wait_for_parent`]).
    fn simplex_state(&self) -> &SimplexState;

    /// Resolve the expected normal-chain tip for a candidate's parent chain via
    /// [`CandidateBook::resolve_parent_tip`](crate::candidate_book::CandidateBook::resolve_parent_tip),
    /// seeding the no-parent / finalized-boundary case with the
    /// `SessionProcessor`-owned accepted normal head. Drives the empty-block
    /// parent-tip readiness and referenced-block match checks.
    fn resolve_parent_tip(&self, parent_id: Option<&RawCandidateId>) -> ParentTipResolution;

    /// Resolve a parent candidate's `gen_utime_ms` for the min-block-interval
    /// pacing gate (delegates to
    /// [`CollationController::resolve_parent_gen_utime_ms_via_book`](crate::collation_controller::CollationController::resolve_parent_gen_utime_ms_via_book)).
    fn parent_gen_utime_ms(&self, parent: &CandidateParentInfo) -> Option<u64>;

    /// Request a missing candidate body/metadata from peers through the shared
    /// `SessionProcessor::request_candidate` throttle map. Bounces a closure onto
    /// SXMAIN (the fetch needs `&mut SessionProcessor`), mirroring
    /// `CollationBackend::request_parent_candidate`.
    fn request_candidate(&self, slot: SlotIndex, hash: UInt256, delay: Option<Duration>);
}

/// Declares the controller's deferred-task backend view for the generic
/// [`ControllerQueue`](crate::controller_queue::ControllerQueue) seam: a
/// borrowing `dyn ValidationBackend + 'b` rebuilt per re-entry (see
/// [`ValidationBackend`]).
impl Controlled for ValidationController {
    type Backend<'b> = dyn ValidationBackend + 'b;
}

/// Per-session candidate-validation controller.
///
/// Owned by
/// [`SessionProcessor`](crate::session_processor::SessionProcessor) as
/// `self.validation`; all access goes through the accessor methods
/// declared below — internal fields stay non-`pub` so the boundary is
/// enforceable.
pub(crate) struct ValidationController {
    /// Generic handle for posting deferred / async validation work back to the
    /// session main loop, targeting `&mut Self`. The concrete `&mut
    /// SessionProcessor -> &mut ValidationController` projection lives in the
    /// `ValidationQueueAdapter` at the composition root, so this controller
    /// names only the generic queue and never depends on `SessionProcessor` —
    /// which is what lets its unit tests drive it with a recording fake (see
    /// `controller_queue::testing::RecordingQueue`) instead of a real session.
    ///
    /// Read by [`Self::try_approve_block`], which wires the off-thread validator
    /// decision callback to post `candidate_decision_ok` / `candidate_decision_fail`
    /// back onto SXMAIN through this handle, and by
    /// [`Self::candidate_decision_fail`], which posts the delayed
    /// validation-retry re-entry the same way.
    queue: ControllerQueuePtr<Self>,
    /// Callback-delivery aspect, shared with `SessionProcessor` and
    /// `SessionImpl` as `Arc<SessionCallbacks>`. Held directly so the
    /// controller can dispatch higher-layer validation requests
    /// (`notify_candidate`) without routing the call back through
    /// `SessionProcessor`. `SessionCallbacks` owns the session-listener
    /// handle internally and never calls back into `SessionProcessor`, so
    /// this introduces no cycle.
    callbacks: Arc<SessionCallbacks>,
    /// Immutable per-session configuration handle, the same `Arc` held by
    /// `SessionProcessor`/`SessionRuntime`. Held directly so the controller
    /// can read session id, options, and timing (`get_session_id`, `opts`,
    /// …) without threading a `&SessionDescription` through every entry
    /// point.
    description: Arc<SessionDescription>,
    /// Per-session telemetry aspect, the same `Arc` held by
    /// `SessionProcessor`. Held directly so validation paths can record
    /// latency histograms / counters without threading a
    /// `&SessionTelemetry` through every entry point. `SessionTelemetry`
    /// is interior-mutable, so a shared `Arc` is sufficient.
    telemetry: Arc<SessionTelemetry>,
    /// Block candidates received from the network awaiting validation
    /// scheduling. Drained by `check_validation` once their parent chain
    /// is `WaitForParent`-ready.
    pending_validations: HashMap<RawCandidateId, PendingValidation>,
    /// Candidates currently in flight to the higher-layer validator
    /// (between `try_approve_block` and the validation callback firing).
    pending_approve: HashSet<RawCandidateId>,
    /// Candidates that have been rejected by validation and are waiting
    /// for the rejection payload to be assembled for outbound delivery.
    pending_reject: HashMap<RawCandidateId, BlockPayloadPtr>,
    /// Terminal rejected set (mirror of C++ `rejected_candidates`).
    rejected: HashSet<RawCandidateId>,
    /// Approved candidates with `(validity_start_time, signature)`.
    approved: HashMap<RawCandidateId, (SystemTime, BlockPayloadPtr)>,
    /// Validation attempt counter per candidate (NOT reset per slot).
    /// Used to thread the attempt index through retry telemetry.
    validation_attempt_map: HashMap<RawCandidateId, u32>,
    /// Validated candidates ready for FSM submission. Consumed by
    /// `process_validated_candidates` in FIFO order.
    validated_candidates: VecDeque<Candidate>,
}

// ======================================================================
// Construction & handles
// ======================================================================
// Build the controller and read its shared session handles.
impl ValidationController {
    /// Construct a fresh, empty validation controller.
    ///
    /// `callbacks` is a cheap-to-clone shared handle (`Arc`) cloned from
    /// `SessionProcessor` at construction so the controller can dispatch
    /// candidate-validation notifications itself via
    /// [`Self::notify_candidate_for_validation`]. The session-listener
    /// handle lives inside `SessionCallbacks`, so no separate listener is
    /// needed here.
    ///
    /// `description` is the immutable per-session config handle (the same
    /// `Arc` the runtime holds), used for session-id / options / timing
    /// reads so those no longer need to be threaded in per call.
    ///
    /// `telemetry` is the shared per-session telemetry aspect (the same
    /// `Arc` `SessionProcessor` holds), so validation paths record metrics
    /// without a `&SessionTelemetry` parameter.
    ///
    /// `queue` is the generic deferred-work handle (`ControllerQueuePtr<Self>`)
    /// the controller uses to post follow-up / async work targeting `&mut
    /// Self`. In production it is a `ValidationQueueAdapter` projecting from
    /// `SessionProcessor`; in tests it is a recording fake — so the controller
    /// stays constructible without a `SessionProcessor`.
    pub(crate) fn new(
        queue: ControllerQueuePtr<Self>,
        callbacks: Arc<SessionCallbacks>,
        description: Arc<SessionDescription>,
        telemetry: Arc<SessionTelemetry>,
    ) -> Self {
        Self {
            queue,
            callbacks,
            description,
            telemetry,
            pending_validations: HashMap::new(),
            pending_approve: HashSet::new(),
            pending_reject: HashMap::new(),
            rejected: HashSet::new(),
            approved: HashMap::new(),
            validation_attempt_map: HashMap::new(),
            validated_candidates: VecDeque::new(),
        }
    }

    /// Session identifier, from the held description. Mirrors
    /// `SessionProcessor::session_id`.
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.description.get_session_id()
    }

    /// Current session time, read from the held description's clock. Mirrors
    /// `SessionProcessor::now`.
    #[inline]
    fn now(&self) -> SystemTime {
        self.description.get_time()
    }
}

// ======================================================================
// Validation bookkeeping state
// ======================================================================
// Per-candidate validation maps/sets and the validated-candidate FIFO.
impl ValidationController {
    /* Pending validations */

    /// Number of candidates currently awaiting validation scheduling.
    pub(crate) fn pending_validation_count(&self) -> usize {
        self.pending_validations.len()
    }

    /// True if `candidate_id` is currently sitting in `pending_validations`.
    pub(crate) fn pending_validation_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.pending_validations.contains_key(candidate_id)
    }

    /// Look up the pending-validation record for `candidate_id`.
    fn pending_validation(&self, candidate_id: &RawCandidateId) -> Option<&PendingValidation> {
        self.pending_validations.get(candidate_id)
    }

    /// Snapshot of the candidate ids currently awaiting validation
    /// scheduling. `check_validation` consumes this so it can drive
    /// `&mut self` mutations without holding a borrow over the map.
    fn pending_validation_ids_snapshot(&self) -> Vec<RawCandidateId> {
        self.pending_validations.keys().cloned().collect()
    }

    /// Insert a pending-validation record, returning the displaced entry
    /// if `candidate_id` already had one.
    pub(crate) fn insert_pending_validation(
        &mut self,
        candidate_id: RawCandidateId,
        pending: PendingValidation,
    ) -> Option<PendingValidation> {
        self.pending_validations.insert(candidate_id, pending)
    }

    /// Remove and return the pending-validation record for `candidate_id`.
    fn remove_pending_validation(
        &mut self,
        candidate_id: &RawCandidateId,
    ) -> Option<PendingValidation> {
        self.pending_validations.remove(candidate_id)
    }

    /* Pending approve (in flight) */

    /// True if `candidate_id` is currently flagged as "in validation".
    pub(crate) fn pending_approve_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.pending_approve.contains(candidate_id)
    }

    /// Mark `candidate_id` as being validated by the higher layer
    /// (callback pending). Returns `true` if it was not already flagged.
    pub(crate) fn insert_pending_approve(&mut self, candidate_id: RawCandidateId) -> bool {
        self.pending_approve.insert(candidate_id)
    }

    /// Clear the "in validation" flag for `candidate_id`. Returns `true`
    /// if a flag was actually cleared.
    pub(crate) fn remove_pending_approve(&mut self, candidate_id: &RawCandidateId) -> bool {
        self.pending_approve.remove(candidate_id)
    }

    /* Pending reject */

    /// Stage a rejection payload for `candidate_id`. Returns the
    /// displaced payload if one was already staged.
    fn insert_pending_reject(
        &mut self,
        candidate_id: RawCandidateId,
        payload: BlockPayloadPtr,
    ) -> Option<BlockPayloadPtr> {
        self.pending_reject.insert(candidate_id, payload)
    }

    /* Rejected (terminal) */

    /// True if `candidate_id` is in the terminal rejected set.
    pub(crate) fn is_rejected(&self, candidate_id: &RawCandidateId) -> bool {
        self.rejected.contains(candidate_id)
    }

    /// Number of rejected candidates currently retained (used by
    /// telemetry / diagnostics).
    pub(crate) fn rejected_count(&self) -> usize {
        self.rejected.len()
    }

    /// Add `candidate_id` to the terminal rejected set. Returns `true`
    /// if it was not already present.
    pub(crate) fn insert_rejected(&mut self, candidate_id: RawCandidateId) -> bool {
        self.rejected.insert(candidate_id)
    }

    /* Approved */

    /// True if `candidate_id` has an approved record.
    pub(crate) fn approved_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.approved.contains_key(candidate_id)
    }

    /// Number of approved candidates currently retained (used by
    /// telemetry / diagnostics).
    pub(crate) fn approved_count(&self) -> usize {
        self.approved.len()
    }

    /// Insert an approved-candidate record. Returns the displaced entry
    /// if `candidate_id` was already approved (should be impossible —
    /// `try_approve_block` guards against re-approval — but the API
    /// returns the displaced entry for symmetry with the underlying
    /// `HashMap::insert`).
    pub(crate) fn insert_approved(
        &mut self,
        candidate_id: RawCandidateId,
        value: (SystemTime, BlockPayloadPtr),
    ) -> Option<(SystemTime, BlockPayloadPtr)> {
        self.approved.insert(candidate_id, value)
    }

    /* Validation attempts */

    /// Look up the validation attempt counter for `candidate_id`.
    fn validation_attempt(&self, candidate_id: &RawCandidateId) -> Option<u32> {
        self.validation_attempt_map.get(candidate_id).copied()
    }

    /// Insert (or replace) the attempt counter for `candidate_id`.
    pub(crate) fn insert_validation_attempt(
        &mut self,
        candidate_id: RawCandidateId,
        attempt: u32,
    ) -> Option<u32> {
        self.validation_attempt_map.insert(candidate_id, attempt)
    }

    /// Remove the attempt-counter entry for `candidate_id`.
    fn remove_validation_attempt(&mut self, candidate_id: &RawCandidateId) -> Option<u32> {
        self.validation_attempt_map.remove(candidate_id)
    }

    /// Bump the attempt counter for `candidate_id`, initializing it to
    /// `0` if not yet present. Mirrors the
    /// `entry().and_modify(|c| *c += 1).or_insert(0)` pattern previously
    /// inlined in `try_approve_block`.
    fn bump_or_init_validation_attempt(&mut self, candidate_id: RawCandidateId) {
        self.validation_attempt_map.entry(candidate_id).and_modify(|c| *c += 1).or_insert(0);
    }

    /* Validated FIFO (FSM hand-off) */

    /// Number of validated candidates ready for FSM submission.
    pub(crate) fn validated_count(&self) -> usize {
        self.validated_candidates.len()
    }

    /// Append a validated candidate to the FIFO submission queue.
    fn push_validated(&mut self, candidate: Candidate) {
        self.validated_candidates.push_back(candidate);
    }

    /// Pop the next validated candidate off the FIFO submission queue.
    pub(crate) fn pop_validated(&mut self) -> Option<Candidate> {
        self.validated_candidates.pop_front()
    }
}

// ======================================================================
// Validation pipeline
// ======================================================================
// Ingress -> scheduling -> approval dispatch -> async verdict. Re-entrant
// verdicts bounce through the controller queue back onto SXMAIN.
impl ValidationController {
    /* Ingress */

    /// Admit a candidate received from the network (signature already
    /// verified by the receiver) into `pending_validations` so it can
    /// later be picked up by `check_validation`.
    ///
    /// Deduplicates against all four terminal/active sets
    /// (`pending_validations`, `pending_approve`, `approved`,
    /// `rejected`). When the candidate is the first received for the
    /// `first_non_progressed_slot`, also stamps the first-candidate-
    /// received marker on the runtime and records the
    /// slot-start-to-first-candidate latency histogram sample.
    ///
    /// `simplex_state` / `runtime` are passed explicitly so the controller
    /// stays independently constructible in tests, mirroring
    /// `CollationController` self-collation helpers and
    /// `evaluate_wait_for_parent`. Telemetry is the controller-owned
    /// `self.telemetry`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register_candidate_for_validation(
        &mut self,
        raw_candidate: RawCandidate,
        slot: SlotIndex,
        source_idx: ValidatorIndex,
        receive_time: SystemTime,
        now: SystemTime,
        simplex_state: &SimplexState,
        runtime: &mut SessionRuntime,
    ) {
        let candidate_id = raw_candidate.id.clone();
        let session_id = self.description.get_session_id().clone();

        // Check if already processed
        if self.pending_validation_contains(&candidate_id)
            || self.pending_approve_contains(&candidate_id)
            || self.approved_contains(&candidate_id)
            || self.is_rejected(&candidate_id)
        {
            log::trace!(
                "Session {} register_candidate_for_validation: candidate already known: {:?}",
                session_id.to_hex_string(),
                candidate_id,
            );
            return;
        }

        log::trace!(
            "Session {} register_candidate_for_validation: registering candidate slot={} hash={}",
            session_id.to_hex_string(),
            slot,
            &candidate_id.hash.to_hex_string()[..8],
        );

        self.insert_pending_validation(
            candidate_id,
            PendingValidation { raw_candidate, slot, receive_time, source_idx },
        );

        // Track first candidate received in this slot (for latency metrics).
        let first_non_progressed_slot = simplex_state.get_first_non_progressed_slot();
        if !runtime.first_candidate_received(slot) && slot == first_non_progressed_slot {
            runtime.set_first_candidate_received(slot, true, now);

            // Track latency from slot start
            if let Ok(elapsed) = now.duration_since(runtime.started_at(slot, now)) {
                self.telemetry
                    .first_candidate_received_latency_histogram
                    .record(elapsed.as_millis() as f64);
            }
        }
    }

    /* Scheduling */

    /// Check pending validations and forward each eligible candidate to the
    /// higher-layer validator.
    ///
    /// Called from `check_all()` via the thin `SessionProcessor::check_validation`
    /// dispatch. Drives candidates out of `pending_validations` whose parent
    /// chain is C++ `WaitForParent`-ready in the FSM, applying the attempt-limit,
    /// min-block-interval pacing, and empty-candidate parent-tip gates before
    /// handing each to [`Self::try_approve_block`]. `SessionProcessor`-owned
    /// reads/effects (FSM state, parent-tip resolution, parent gen-utime,
    /// candidate requests, the wake horizon) go through `backend`; session
    /// options/clock come from the owned `description`.
    ///
    /// A candidate is eligible when:
    /// 1. It has been admitted into `pending_validations`.
    /// 2. Its parent chain is C++ `WaitForParent`-ready (notar/final parent + gap
    ///    skip coverage).
    /// 3. Empty candidates can resolve the expected `event->state->as_normal()`
    ///    tip from locally known metadata (requesting the next missing parent on
    ///    demand if needed).
    /// 4. MC stale-parent protection is handled in validator-side
    ///    candidate-native validation.
    /// 5. It is not already being validated, approved, or rejected.
    ///
    /// Reference: validator-session/src/session_processor.rs check_validation()
    pub(crate) fn check_validation(&mut self, backend: &dyn ValidationBackend) {
        check_execution_time!(10_000);
        instrument!();
        let now = self.now();

        let mut to_validate: Vec<(RawCandidateId, SlotIndex, ValidatorIndex, SystemTime)> =
            Vec::new();

        let candidate_ids: Vec<RawCandidateId> = self.pending_validation_ids_snapshot();
        for candidate_id in candidate_ids {
            let (slot, source_idx, receive_time, raw_candidate, wait_for_parent_decision) =
                match self.pending_validation(&candidate_id) {
                    Some(p) => (
                        p.slot,
                        p.source_idx,
                        p.receive_time,
                        p.raw_candidate.clone(),
                        self.evaluate_wait_for_parent(p, backend.simplex_state()),
                    ),
                    None => continue,
                };

            // Skip if already being validated or decided
            if self.pending_approve_contains(&candidate_id) {
                continue;
            }
            if self.is_rejected(&candidate_id) {
                continue;
            }
            if self.approved_contains(&candidate_id) {
                continue;
            }

            match wait_for_parent_decision {
                WaitForParentDecision::Ready => {}
                WaitForParentDecision::Wait => continue,
                WaitForParentDecision::Reject(reason) => {
                    log::warn!(
                        "Session {} check_validation: rejecting candidate {:?} by WaitForParent \
                         parity gate: {}",
                        self.session_id().to_hex_string(),
                        candidate_id,
                        reason
                    );
                    self.insert_validation_attempt(
                        candidate_id.clone(),
                        self.description.opts().validation_retry_attempts,
                    );
                    self.candidate_decision_fail(backend, slot, candidate_id, error!("{reason}"));
                    continue;
                }
            }

            // Check validation attempt count
            if let Some(attempt_idx) = self.validation_attempt(&candidate_id) {
                if attempt_idx >= self.description.opts().validation_retry_attempts {
                    log::trace!(
                        "Session {} check_validation: max attempts reached for {:?}",
                        self.session_id().to_hex_string(),
                        candidate_id,
                    );
                    continue;
                }
            }

            if !raw_candidate.block.is_empty() {
                if let Some(parent_id) = raw_candidate.parent_id.as_ref() {
                    let parent_info =
                        CandidateParentInfo { slot: parent_id.slot, hash: parent_id.hash.clone() };
                    if let Some(parent_gen_utime_ms) = backend.parent_gen_utime_ms(&parent_info) {
                        let earliest_validation_time = UNIX_EPOCH
                            .checked_add(Duration::from_millis(parent_gen_utime_ms))
                            .and_then(|parent_gen_time| {
                                parent_gen_time
                                    .checked_add(self.description.opts().min_block_interval)
                            });
                        let Some(earliest_validation_time) = earliest_validation_time else {
                            log::warn!(
                                "Session {} check_validation: invalid parent_gen_utime_ms {} for \
                                parent slot {}",
                                self.session_id().to_hex_string(),
                                parent_gen_utime_ms,
                                parent_info.slot,
                            );
                            continue;
                        };
                        if now < earliest_validation_time {
                            backend.request_wake_at(earliest_validation_time);
                            continue;
                        }
                    }
                }
            }

            if raw_candidate.block.is_empty() {
                if !self.ensure_empty_parent_tip_ready(backend, &raw_candidate, slot) {
                    continue;
                }

                to_validate.push((candidate_id.clone(), slot, source_idx, receive_time));
                continue;
            }

            to_validate.push((candidate_id.clone(), slot, source_idx, receive_time));
        }
        // Process each candidate
        for (candidate_id, slot, source_idx, receive_time) in to_validate {
            self.try_approve_block(backend, &candidate_id, slot, source_idx, receive_time);
        }
    }

    /// C++ `WaitForParent`-equivalent parent-gating decision used by
    /// `check_validation` to drive candidates from `pending_validations`
    /// to `pending_approve`.
    ///
    /// Mirrors `pool.cpp::maybe_resolve_request` semantics:
    /// - `Ready`: candidate can proceed to higher-layer validation now.
    /// - `Wait`: keep candidate pending until more cert state arrives.
    /// - `Reject`: parent chain conflicts with already-established
    ///   consensus state.
    ///
    /// Pure policy — reads `simplex_state` and the controller-owned
    /// `description` only; does not mutate any controller state. Takes
    /// `&self` for parity with
    /// `CollationController::should_generate_empty_block` and so that
    /// future migrations can opt into reading validation-controller-owned
    /// state without churning every call site.
    fn evaluate_wait_for_parent(
        &self,
        pending: &PendingValidation,
        simplex_state: &SimplexState,
    ) -> WaitForParentDecision {
        let description = self.description.as_ref();
        let slot = pending.slot;
        let candidate_id = &pending.raw_candidate.id;
        let first_non_finalized = simplex_state.get_first_non_finalized_slot();
        let parent_id = pending.raw_candidate.parent_id.as_ref();

        // C++ parity (pool.cpp maybe_resolve_request):
        // next_slot_after_parent = parent.has_value() ? parent->slot + 1 : 0
        let next_slot_after_parent = match parent_id {
            Some(pid) => {
                if pid.slot >= slot {
                    return WaitForParentDecision::Reject(format!(
                        "invalid parent slot {} for candidate slot {}",
                        pid.slot, slot
                    ));
                }
                pid.slot + 1
            }
            None => SlotIndex::new(0),
        };

        if slot < first_non_finalized {
            return WaitForParentDecision::Reject(format!(
                "candidate slot {} is already finalized (first_non_finalized={})",
                slot, first_non_finalized
            ));
        }
        if next_slot_after_parent < first_non_finalized {
            return WaitForParentDecision::Reject(format!(
                "candidate parent frontier {} is below first_non_finalized={}",
                next_slot_after_parent, first_non_finalized
            ));
        }

        // C++ parity (pool.cpp maybe_resolve_request):
        // reject candidates conflicting with already notarized/finalized candidate in this slot.
        if let Some(notarized_hash) = simplex_state.get_notarized_block_hash(description, slot) {
            if notarized_hash != candidate_id.hash {
                return WaitForParentDecision::Reject(format!(
                    "slot {} already notarized/finalized with different hash {} (candidate={})",
                    slot,
                    notarized_hash.to_hex_string(),
                    candidate_id.hash.to_hex_string()
                ));
            }
        }

        // C++ parity (pool.cpp maybe_resolve_request):
        // - if parent is at finalized boundary, it must match last finalized block;
        // - otherwise parent slot must be notarized with the same candidate hash.
        if next_slot_after_parent == first_non_finalized {
            match parent_id {
                None => {
                    // C++ parity: only genesis frontier (`first_non_finalized == 0`) can have
                    // `parent_id=None` at boundary check.
                    if first_non_finalized.value() != 0 {
                        return WaitForParentDecision::Reject(format!(
                            "expected finalized-boundary parent at slot {}, got genesis parent",
                            first_non_finalized.value().saturating_sub(1)
                        ));
                    }
                }
                Some(pid) => {
                    let Some((last_finalized_slot, final_cert)) =
                        simplex_state.get_last_finalize_certificate()
                    else {
                        return WaitForParentDecision::Reject(format!(
                            "finalized-boundary parent mismatch: no finalized certificate for parent {}:{}",
                            pid.slot,
                            pid.hash.to_hex_string()
                        ));
                    };
                    if last_finalized_slot != pid.slot || final_cert.vote.block_hash != pid.hash {
                        return WaitForParentDecision::Reject(format!(
                            "finalized-boundary parent mismatch: expected {}:{} got {}:{}",
                            last_finalized_slot,
                            final_cert.vote.block_hash.to_hex_string(),
                            pid.slot,
                            pid.hash.to_hex_string()
                        ));
                    }
                }
            }
        } else {
            // next_slot_after_parent > first_non_finalized, so parent must exist and be notarized.
            // Genesis parent has next_slot_after_parent=0 which can't exceed first_non_finalized.
            let Some(pid) = parent_id else {
                return WaitForParentDecision::Reject(format!(
                    "missing parent id for non-boundary candidate at slot {}",
                    slot
                ));
            };
            match simplex_state.get_notarized_block_hash(description, pid.slot) {
                Some(notarized_hash) => {
                    if notarized_hash != pid.hash {
                        return WaitForParentDecision::Reject(format!(
                            "parent notarized hash mismatch at slot {}: expected {} got {}",
                            pid.slot,
                            pid.hash.to_hex_string(),
                            notarized_hash.to_hex_string()
                        ));
                    }
                }
                None => return WaitForParentDecision::Wait,
            }
        }

        if next_slot_after_parent == slot {
            return WaitForParentDecision::Ready;
        }

        // All intermediate slots must have Skip certificates.
        let mut gap_slot = next_slot_after_parent;
        while gap_slot < slot {
            if !simplex_state.has_skip_certificate_for_slot(description, gap_slot) {
                return WaitForParentDecision::Wait;
            }
            gap_slot += 1;
        }

        WaitForParentDecision::Ready
    }

    /// Ensure the expected normal-chain tip for an empty candidate is resolvable
    /// from locally known metadata, requesting the next missing parent on demand.
    ///
    /// Returns `true` when the tip resolves (the candidate may proceed to
    /// `try_approve_block`), `false` when it is still waiting (kept pending).
    /// Moved from `SessionProcessor`; parent-tip resolution and the
    /// missing-parent fetch go through `backend`, error telemetry through the
    /// owned `telemetry`.
    fn ensure_empty_parent_tip_ready(
        &self,
        backend: &dyn ValidationBackend,
        raw_candidate: &RawCandidate,
        slot: SlotIndex,
    ) -> bool {
        match backend.resolve_parent_tip(raw_candidate.parent_id.as_ref()) {
            ParentTipResolution::Resolved(_) => true,
            ParentTipResolution::MissingParent(missing_parent) => {
                log::trace!(
                    "Session {} ensure_empty_parent_tip_ready: requesting missing parent metadata \
                    slot={} hash={} for empty candidate slot={}",
                    self.session_id().to_hex_string(),
                    missing_parent.slot,
                    &missing_parent.hash.to_hex_string()[..8],
                    slot,
                );
                backend.request_candidate(
                    missing_parent.slot,
                    missing_parent.hash,
                    Some(Duration::ZERO),
                );
                false
            }
            ParentTipResolution::Unresolved => {
                log::trace!(
                    "Session {} ensure_empty_parent_tip_ready: empty candidate slot={} is still \
                    waiting for the accepted normal head or restart-seeded parent metadata",
                    self.session_id().to_hex_string(),
                    slot,
                );
                false
            }
            ParentTipResolution::TooDeep => {
                self.telemetry.increment_error();
                false
            }
        }
    }

    /* Approval dispatch */

    /// Try to approve a block candidate by sending it to the higher layer.
    ///
    /// Moved from `SessionProcessor`. For empty candidates, resolves the parent
    /// normal tip via `backend` and either auto-approves (referenced block
    /// matches), terminally rejects (mismatch), or leaves the candidate pending
    /// and requests the missing parent. For normal candidates, builds the
    /// validator-decision callback — which posts the result back through the
    /// controller's own deferred-work `queue` (so the controller never names
    /// `SessionProcessor`) — and dispatches via the owned `callbacks`.
    ///
    /// Reference: validator-session/src/session_processor.rs try_approve_block()
    fn try_approve_block(
        &mut self,
        backend: &dyn ValidationBackend,
        candidate_id: &RawCandidateId,
        slot: SlotIndex,
        source_idx: ValidatorIndex,
        receive_time: SystemTime,
    ) {
        check_execution_time!(10_000);
        instrument!();

        // Check if pending validation exists (session-level)
        if !self.pending_validation_contains(candidate_id) {
            return;
        }

        // Mark as pending approval
        self.insert_pending_approve(candidate_id.clone());
        self.bump_or_init_validation_attempt(candidate_id.clone());
        self.telemetry.mark_generated_candidate_validation_started(candidate_id);

        // Get pending validation (now safe to borrow after mutable operations)
        let Some(pending) = self.pending_validation(candidate_id) else {
            log::error!(
                "Session {} try_approve_block: candidate not in pending_validations: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            return;
        };

        // Handle empty blocks: C++ block-validator.cpp rejects unless the referenced
        // block equals event->state->as_normal(). We resolve the expected block from
        // the parent chain and compare before approving; if metadata is still missing,
        // the candidate stays pending and waits for the next repair round.
        if pending.raw_candidate.block.is_empty() {
            let referenced_block = pending.raw_candidate.block.block_id().clone();
            let cid = candidate_id.clone();
            let resolution = backend.resolve_parent_tip(pending.raw_candidate.parent_id.as_ref());

            match resolution {
                ParentTipResolution::Resolved(expected_block)
                    if referenced_block == expected_block =>
                {
                    log::trace!(
                        "Session {} try_approve_block: empty block matches parent normal tip, \
                        approving {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                    self.candidate_decision_ok_internal(cid, slot, receive_time);
                }
                ParentTipResolution::Resolved(expected_block) => {
                    log::warn!(
                        "Session {} try_approve_block: empty block REJECTED — wrong referenced \
                        block (got seqno={}, expected seqno={}) for {:?}",
                        self.session_id().to_hex_string(),
                        referenced_block.seq_no,
                        expected_block.seq_no,
                        cid,
                    );
                    self.candidate_decision_fail(
                        backend,
                        slot,
                        cid,
                        error!("Wrong referenced block in empty candidate"),
                    );
                }
                ParentTipResolution::MissingParent(missing_parent) => {
                    backend.request_candidate(
                        missing_parent.slot,
                        missing_parent.hash,
                        Some(Duration::ZERO),
                    );
                    self.remove_pending_approve(candidate_id);
                    log::trace!(
                        "Session {} try_approve_block: empty block still waiting for parent \
                        normal tip for {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                }
                ParentTipResolution::Unresolved => {
                    self.remove_pending_approve(candidate_id);
                    log::trace!(
                        "Session {} try_approve_block: empty block still waiting for parent \
                        normal tip for {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                }
                ParentTipResolution::TooDeep => {
                    self.telemetry.increment_error();
                    self.remove_pending_approve(candidate_id);
                    log::trace!(
                        "Session {} try_approve_block: empty block still waiting for parent \
                        normal tip for {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                }
            }
            return;
        }

        // Get block data for validation
        let Some(block) = pending.raw_candidate.block.as_block() else {
            log::error!(
                "Session {} try_approve_block: non-empty block has no block data: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            return;
        };

        let root_hash = block.id.root_hash.clone();
        let data =
            consensus_common::ConsensusCommonFactory::create_block_payload(block.data.clone());
        let collated_data = consensus_common::ConsensusCommonFactory::create_block_payload(
            block.collated_data.clone(),
        );

        // Create source info for callback
        // Note: source_idx was already validated in on_candidate_received
        // SIMPLEX_ROUNDLESS: bypass ValidatorGroup round invariants
        let source_public_key = self.description.get_source_public_key(source_idx).clone();

        let source_info = BlockSourceInfo {
            source: source_public_key,
            priority: BlockCandidatePriority {
                round: SIMPLEX_ROUNDLESS,             // Simplex roundless mode
                first_block_round: SIMPLEX_ROUNDLESS, // Must match round for consistency
                priority: 0,                          // Leader priority
            },
        };

        // DEBUG: Short pattern for quick grep (VALIDATION = block validation flow)
        log::debug!(
            "Session {} VALIDATION request: slot={}, hash={}, from=v{:03}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &candidate_id.hash.to_hex_string()[..8],
            source_idx
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} try_approve_block: requesting validation for slot={}, hash={}, source={}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id.hash.to_hex_string(),
            source_idx
        );

        // Create callback for validation result. The decision arrives on another
        // thread; route it back through the controller's deferred-work queue (the
        // adapter projects onto SXMAIN and wakes the main loop), so the callback
        // names only `ControllerQueuePtr` and never `SessionProcessor`.
        let queue = self.queue.clone();
        let candidate_id_copy = candidate_id.clone();

        let callback: ValidatorBlockCandidateDecisionCallback =
            Box::new(move |decision: Result<SystemTime>| {
                let candidate_id = candidate_id_copy.clone();
                let slot_copy = slot;
                let receive_time_copy = receive_time;

                match decision {
                    Ok(validity_start_time) => {
                        queue.post(move |validation, backend| {
                            validation.candidate_decision_ok(
                                backend,
                                slot_copy,
                                candidate_id,
                                validity_start_time,
                                receive_time_copy,
                            );
                        });
                    }
                    Err(err) => {
                        queue.post(move |validation, backend| {
                            validation.candidate_decision_fail(
                                backend,
                                slot_copy,
                                candidate_id,
                                err,
                            );
                        });
                    }
                }
            });

        self.notify_candidate_for_validation(source_info, root_hash, data, collated_data, callback);
    }

    /// Dispatch a block candidate to the higher-layer validator.
    ///
    /// Thin pass-through to [`SessionCallbacks::notify_candidate`]; the
    /// listener handle it dispatches to lives inside `SessionCallbacks`.
    /// The decision-result `callback` is still built by the caller (it
    /// posts a `TaskPtr` back onto the main queue); only the transport hop
    /// moves here, removing the `SessionProcessor::callbacks` access from
    /// the validation flow.
    fn notify_candidate_for_validation(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        self.callbacks.notify_candidate(source_info, root_hash, data, collated_data, callback);
    }

    /* Async verdict */

    /// Handle a successful higher-layer validation decision for `candidate_id`.
    ///
    /// Moved from `SessionProcessor`, which keeps a thin wrapper that builds the
    /// backend. `SessionProcessor`-owned reads/effects go through `backend`
    /// (`finalized_head_seqno`, `request_wake`); validation state is mutated on
    /// `&mut self`; counters and the self-collation funnel use the held
    /// `telemetry` handle.
    ///
    /// Reference: validator-session/src/session_processor.rs candidate_decision_ok()
    pub(crate) fn candidate_decision_ok(
        &mut self,
        backend: &dyn ValidationBackend,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        validity_start_time: SystemTime,
        receive_time: SystemTime,
    ) {
        check_execution_time!(10_000);
        instrument!();

        self.telemetry.validates_counter.success();

        let now = self.now();

        // Record validation latency (time spent in validator callback)
        if let Ok(latency) = now.duration_since(receive_time) {
            self.telemetry.validation_latency_histogram.record(latency.as_millis() as f64);
        }

        // Record broadcast-to-validation-complete latency (full round-trip from network receive)
        if let Ok(broadcast_latency) = now.duration_since(receive_time) {
            self.telemetry
                .broadcast_validation_latency_histogram
                .record(broadcast_latency.as_millis() as f64);
        }

        // DEBUG: Short pattern for quick grep (VALIDATION = block validation flow)
        let latency_ms = now.duration_since(receive_time).map(|d| d.as_millis()).unwrap_or(0);
        log::debug!(
            "Session {} VALIDATION success: slot={}, hash={}, latency={}ms",
            &self.session_id().to_hex_string()[..8],
            slot,
            &candidate_id.hash.to_hex_string()[..8],
            latency_ms
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} candidate_decision_ok: slot={}, hash={}, latency={}ms, validity_start={:?}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id.hash.to_hex_string(),
            latency_ms,
            validity_start_time
        );

        // Ignore late validation callbacks for already processed candidates (validator-session
        // has round gating; in roundless Simplex we gate by "still pending").
        if !self.pending_validation_contains(&candidate_id) {
            self.telemetry.validation_late_callback_counter.increment(1);
            self.telemetry.note_generated_candidate_validation_missed(
                &candidate_id,
                "validation_late_callback_without_pending_entry",
                self.description.as_ref(),
                now,
            );
            self.remove_pending_approve(&candidate_id);
            self.remove_validation_attempt(&candidate_id);
            return;
        }

        // If the block is already finalized by the time validation completes, drop the result.
        if let (Some(finalized_seqno), Some(cand_seqno)) = (
            backend.finalized_head_seqno(),
            self.pending_validation(&candidate_id)
                .and_then(|p| p.raw_candidate.block.as_block().map(|b| b.id.seq_no)),
        ) {
            if cand_seqno <= finalized_seqno {
                self.telemetry.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!(
                        "validation_succeeded_after_finalization finalized_seqno={finalized_seqno} cand_seqno={cand_seqno}"
                    ),
                    self.description.as_ref(),
                    now,
                );
                log::warn!(
                    "Session {} candidate_decision_ok: slot={slot}, hash={:?}, \
                    finalized_seqno={finalized_seqno}, cand_seqno={cand_seqno} (drop because \
                    block is already finalized)",
                    self.session_id().to_hex_string(),
                    candidate_id,
                );
                self.remove_pending_approve(&candidate_id);
                self.remove_pending_validation(&candidate_id);
                self.remove_validation_attempt(&candidate_id);
                return;
            }
        }

        self.candidate_decision_ok_internal(candidate_id, slot, receive_time);

        // Wake immediately so check_all() runs in the very next main-loop iteration
        backend.request_wake();
    }

    /// Internal helper for a successful validation, shared by the normal and
    /// empty-block paths. Needs no [`ValidationBackend`] (no finalized-head /
    /// wake interaction).
    pub(crate) fn candidate_decision_ok_internal(
        &mut self,
        candidate_id: RawCandidateId,
        _slot: SlotIndex,
        _receive_time: SystemTime,
    ) {
        self.remove_pending_approve(&candidate_id);

        // Get and remove from pending_validations (per-slot state)
        let pending = match self.remove_pending_validation(&candidate_id) {
            Some(p) => p,
            None => {
                let now = self.now();
                self.telemetry.note_generated_candidate_validation_missed(
                    &candidate_id,
                    "validation_success_missing_pending_entry",
                    self.description.as_ref(),
                    now,
                );
                log::warn!(
                    "Session {} candidate_decision_ok_internal: no pending validation for {:?}",
                    self.session_id().to_hex_string(),
                    candidate_id,
                );
                return;
            }
        };

        // Resolve RawCandidate to Candidate
        // For empty blocks: inherit parent's BlockIdExt from parent (requires lookup)
        // For normal blocks: resolve() uses the block's BlockIdExt and self.parent_id
        // Note: For non-empty blocks, resolve(None) now correctly uses RawCandidate.parent_id
        let candidate = match pending.raw_candidate.resolve(None) {
            Ok(c) => c,
            Err(e) => {
                let now = self.now();
                self.telemetry.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!("validation_success_resolve_failed error={e}"),
                    self.description.as_ref(),
                    now,
                );
                log::warn!(
                    "Session {} candidate_decision_ok: failed to resolve candidate: {}",
                    self.session_id().to_hex_string(),
                    e
                );
                return;
            }
        };

        self.telemetry.mark_generated_candidate_validation_succeeded(&candidate_id);

        let now = self.now();
        self.insert_approved(
            candidate_id,
            (now, consensus_common::ConsensusCommonFactory::create_empty_block_payload()),
        );

        // Push to validated queue for FSM processing
        self.push_validated(candidate);
    }

    /// Handle a failed higher-layer validation decision for `candidate_id`,
    /// retrying through the deferred-work queue while attempts remain, then
    /// terminally rejecting.
    ///
    /// Moved from `SessionProcessor`, which keeps a thin wrapper that builds the
    /// backend. The delayed retry is posted via the controller's
    /// [`ControllerQueue`](crate::controller_queue::ControllerQueue) handle, so
    /// the re-entry runs against `&mut Self` with a freshly built backend;
    /// `finalized_head_seqno` / `request_wake` come through `backend`.
    ///
    /// Reference: validator-session/src/session_processor.rs candidate_decision_fail()
    pub(crate) fn candidate_decision_fail(
        &mut self,
        backend: &dyn ValidationBackend,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        err: Error,
    ) {
        check_execution_time!(10_000);
        instrument!();

        self.telemetry.validates_counter.failure();
        self.telemetry.validation_reject_counter.increment(1);

        let mut reason = format!("{}", err);

        // Ignore late validation callbacks for already processed candidates (validator-session
        // has round gating; in roundless Simplex we gate by "still pending").
        if !self.pending_validation_contains(&candidate_id) {
            self.telemetry.validation_late_callback_counter.increment(1);
            let now = self.now();
            self.telemetry.note_generated_candidate_validation_missed(
                &candidate_id,
                "validation_fail_late_callback_without_pending_entry",
                self.description.as_ref(),
                now,
            );
            self.remove_pending_approve(&candidate_id);
            self.remove_validation_attempt(&candidate_id);
            return;
        }

        // If the block is already finalized by the time validation fails, drop it without retries.
        if let (Some(finalized_seqno), Some(cand_seqno)) = (
            backend.finalized_head_seqno(),
            self.pending_validation(&candidate_id)
                .and_then(|p| p.raw_candidate.block.as_block().map(|b| b.id.seq_no)),
        ) {
            log::warn!(
                "Session {} candidate_decision_fail: slot={slot}, hash={:?}, \
                finalized_seqno={finalized_seqno}, cand_seqno={cand_seqno} (drop)",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            if cand_seqno <= finalized_seqno {
                let now = self.now();
                self.telemetry.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!(
                        "validation_failed_after_finalization finalized_seqno={finalized_seqno} cand_seqno={cand_seqno}"
                    ),
                    self.description.as_ref(),
                    now,
                );
                self.remove_pending_approve(&candidate_id);
                self.remove_pending_validation(&candidate_id);
                self.remove_validation_attempt(&candidate_id);
                return;
            }
        }

        // Check if we should retry
        if let Some(attempt_idx) = self.validation_attempt(&candidate_id) {
            if attempt_idx < self.description.opts().validation_retry_attempts {
                let retry_timeout = self.description.opts().validation_retry_timeout;
                let expiration_time = self.now() + retry_timeout;

                log::warn!(
                    "Session {} candidate_decision_fail: slot={}, hash={:?}, attempt={}/{}, \
                    reason={}. Will retry in {}ms.",
                    self.session_id().to_hex_string(),
                    slot,
                    candidate_id,
                    attempt_idx,
                    self.description.opts().validation_retry_attempts,
                    reason,
                    retry_timeout.as_millis(),
                );

                let candidate_id_copy = candidate_id.clone();
                self.queue.post_delayed(expiration_time, move |controller, backend| {
                    log::trace!(
                        "Session {} allowing validation retry for {:?}",
                        controller.session_id().to_hex_string(),
                        candidate_id_copy,
                    );
                    controller.remove_pending_approve(&candidate_id_copy);
                    backend.request_wake();
                });

                return;
            }
        }

        log::warn!(
            "Session {} candidate_decision_fail: slot={}, hash={:?}, no attempts left, reason={}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id,
            reason,
        );
        let now = self.now();
        self.telemetry.note_generated_candidate_validation_missed(
            &candidate_id,
            format!("validation_failed_final reason={reason}"),
            self.description.as_ref(),
            now,
        );

        self.remove_pending_approve(&candidate_id);
        self.remove_pending_validation(&candidate_id);

        // Truncate reason if too long
        const MAX_REJECT_REASON_SIZE: usize = 1024;
        if reason.len() > MAX_REJECT_REASON_SIZE {
            reason = reason[..MAX_REJECT_REASON_SIZE].to_string();
        }

        self.insert_pending_reject(
            candidate_id.clone(),
            consensus_common::ConsensusCommonFactory::create_block_payload(
                reason.as_bytes().to_vec(),
            ),
        );
        self.insert_rejected(candidate_id);
    }
}

// ======================================================================
// History pruning
// ======================================================================
// Drop per-candidate validation state below the finalized/skip boundary.
impl ValidationController {
    /// Drop every entry in every validation map whose `slot` is below
    /// `up_to_slot`. Collapses the seven `retain(|id| id.slot >=
    /// up_to_slot)` calls previously inlined in `cleanup_old_candidates`.
    ///
    /// `received_candidates` is intentionally NOT pruned here —
    /// `CandidateBook` keeps its own discipline (finalized-stub safety;
    /// see `CandidateBook::prune_below`).
    pub(crate) fn prune_below(&mut self, up_to_slot: SlotIndex) {
        self.pending_validations.retain(|id, _| id.slot >= up_to_slot);
        self.pending_approve.retain(|id| id.slot >= up_to_slot);
        self.pending_reject.retain(|id, _| id.slot >= up_to_slot);
        self.rejected.retain(|id| id.slot >= up_to_slot);
        self.approved.retain(|id, _| id.slot >= up_to_slot);
        self.validation_attempt_map.retain(|id, _| id.slot >= up_to_slot);
        self.validated_candidates.retain(|c| c.id.slot >= up_to_slot);
    }
}

impl std::fmt::Debug for ValidationController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationController")
            .field("pending_validation_count", &self.pending_validations.len())
            .field("pending_approve_count", &self.pending_approve.len())
            .field("pending_reject_count", &self.pending_reject.len())
            .field("rejected_count", &self.rejected.len())
            .field("approved_count", &self.approved.len())
            .field("validation_attempt_count", &self.validation_attempt_map.len())
            .field("validated_count", &self.validated_candidates.len())
            .finish_non_exhaustive()
    }
}

// ======================================================================
// Tests
// ======================================================================
// Test-only accessors / seams consolidated under one `#[cfg(test)]` impl so the
// production impls carry no test scaffolding.
#[cfg(test)]
impl ValidationController {
    /* Callback injection */

    /// Re-point the controller at a rebuilt callbacks aspect.
    ///
    /// Test-only seam used by `SessionProcessor::set_listener_for_test`:
    /// when a test swaps the session listener it rebuilds the shared
    /// `Arc<SessionCallbacks>`, so this controller's clone must be
    /// re-pointed at the new instance to keep candidate-validation
    /// dispatch reaching the test's recording listener.
    pub(crate) fn set_callbacks_for_test(&mut self, callbacks: Arc<SessionCallbacks>) {
        self.callbacks = callbacks;
    }

    /* State probes */

    /// True if `candidate_id` is currently flagged for rejection
    /// delivery.
    pub(crate) fn pending_reject_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.pending_reject.contains_key(candidate_id)
    }

    /// True if `candidate_id` has an attempt-counter entry. Test-only —
    /// production callers always go through `validation_attempt(...)` or
    /// `bump_or_init_validation_attempt(...)`.
    pub(crate) fn validation_attempt_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.validation_attempt_map.contains_key(candidate_id)
    }

    /// True if `validated_candidates` is empty. Used by FSM-drain
    /// short-circuits and by tests asserting that drained-out fixtures
    /// no longer have anything queued for `process_validated_candidates`.
    pub(crate) fn validated_is_empty(&self) -> bool {
        self.validated_candidates.is_empty()
    }
}

/*
    Tests live in a sibling file but are included directly via `#[path]` so
    they can reach the private accessor surface and the inner
    `PendingValidation` type without widening visibility. Mirrors the
    convention used by `collation_controller.rs` / `controller_queue.rs`.
*/

#[cfg(test)]
#[path = "tests/test_validation_controller.rs"]
mod tests;
