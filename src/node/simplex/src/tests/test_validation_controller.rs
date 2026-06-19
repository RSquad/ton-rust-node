/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for
//! [`crate::validation_controller::ValidationController`].
//!
//! Included directly from `validation_controller.rs` via `#[path]` so tests
//! can reach the private accessor surface and the inner [`PendingValidation`]
//! type without widening visibility. Mirrors the convention used by
//! `tests/test_collation_controller.rs` and `tests/test_controller_queue.rs`.
//!
//! Scope — state mechanics, pure-policy decisions, and the decision handlers
//! that need only the borrowing [`ValidationBackend`] seam (full orchestration
//! with a real FSM / notarization is still covered by
//! `tests/test_session_processor.rs`):
//!
//! - Constructor wiring: every map empty.
//! - Accessor round-trips across `pending_validations` / `pending_approve` /
//!   `rejected` / `approved` / `validation_attempt_map` / `validated_candidates`.
//! - `prune_below` drops every entry below the cutoff across all maps and the
//!   validated FIFO, keeping the rest.
//! - `evaluate_wait_for_parent` truth table on a fresh FSM: genesis `Ready`,
//!   bad-parent-slot `Reject`, un-notarized-parent `Wait`.
//! - `check_validation` gating: `WaitForParent::Reject` → terminal reject;
//!   attempt-limit → skip (stays pending); a genesis-`Ready` normal candidate is
//!   dispatched to the validator (lands in `pending_approve`, attempt bumped,
//!   no synchronous decision since the listener is a no-op).
//! - `try_approve_block` empty-block paths driven through the fake
//!   `resolve_parent_tip`: matching tip auto-approves, mismatching tip rejects,
//!   `MissingParent` requests the parent and keeps the candidate pending.
//! - `candidate_decision_ok`: approves + enqueues a validated candidate + wakes;
//!   drops a late callback with no pending entry; drops an already-finalized
//!   candidate.
//! - `candidate_decision_fail`: terminal reject when attempts are exhausted;
//!   posts a delayed retry through the `queue` while attempts remain (replayed
//!   against the recording fake to confirm it clears `pending_approve` + wakes).
//!
//! The validator-decision callback wiring (`try_approve_block`'s normal-block
//! `queue.post`) and the notarization-gated `Ready` paths of `check_validation`
//! are exercised end-to-end by `tests/test_session_processor.rs` against the
//! real `ValidationQueueAdapter` / FSM, so they are not duplicated here.

use super::*;
use crate::{
    block::BlockCandidate,
    controller_queue::{ControllerQueue, ControllerTask},
    SessionNode, SessionOptions,
};
use crossbeam::queue::SegQueue;
use std::cell::{Cell, RefCell};
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, ZeroizingBytes};

/*
    --------------------------------------------------------------------
    Test helpers
    --------------------------------------------------------------------
*/

fn make_candidate_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    let mut hash = [0u8; 32];
    hash[0] = hash_byte;
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from(hash) }
}

fn make_block_id(seqno: u32, root_hash_byte: u8) -> BlockIdExt {
    let mut root = [0u8; 32];
    root[0] = root_hash_byte;
    let mut file = [0u8; 32];
    file[1] = root_hash_byte;
    BlockIdExt::with_params(
        ShardIdent::masterchain(),
        seqno,
        UInt256::from(root),
        UInt256::from(file),
    )
}

/// Build a single-node masterchain `SessionDescription` (local node at index 0)
/// plus the node set, so candidate fixtures can reuse the leader public key the
/// description resolves through `get_source_public_key`.
fn make_desc(opts: SessionOptions) -> (Arc<SessionDescription>, Vec<SessionNode>) {
    let nodes: Vec<SessionNode> = (0..1)
        .map(|_| {
            let public_key =
                Ed25519KeyOption::<ZeroizingBytes>::generate().expect("key gen must succeed");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight: 1 }
        })
        .collect();
    let local_key = nodes[0].public_key.clone();
    let desc = SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &ShardIdent::masterchain(),
        SystemTime::now(),
        None,
    )
    .expect("SessionDescription::new must succeed for test fixture");
    (Arc::new(desc), nodes)
}

/// A non-empty `RawCandidate` whose synthetic block id carries `slot + 1` as its
/// seqno (matching `tests/test_session_processor.rs`), so the finalized-head
/// drop gate in the decision handlers has a seqno to compare against.
fn make_non_empty_raw(
    candidate_id: RawCandidateId,
    parent_id: Option<RawCandidateId>,
    nodes: &[SessionNode],
) -> RawCandidate {
    let block = BlockCandidate {
        id: make_block_id(candidate_id.slot.value() + 1, 0x11),
        collated_file_hash: UInt256::rand(),
        data: vec![1, 2, 3],
        collated_data: vec![4, 5, 6],
        creator: nodes[0].public_key.clone(),
    };
    RawCandidate::new(candidate_id, parent_id, ValidatorIndex::new(0), block, vec![])
}

/// An empty `RawCandidate` referencing `referenced_block` — the input the
/// `try_approve_block` empty-block path compares against the resolved parent tip.
fn make_empty_raw(
    candidate_id: RawCandidateId,
    parent_id: RawCandidateId,
    referenced_block: BlockIdExt,
) -> RawCandidate {
    RawCandidate::new_empty(
        candidate_id,
        parent_id,
        ValidatorIndex::new(0),
        referenced_block,
        vec![],
    )
}

fn make_pending(raw_candidate: RawCandidate, slot: u32) -> PendingValidation {
    PendingValidation {
        raw_candidate,
        slot: SlotIndex::new(slot),
        receive_time: SystemTime::now(),
        source_idx: ValidatorIndex::new(0),
    }
}

/// An empty `BlockPayloadPtr` for the maps that store a payload.
fn empty_payload() -> BlockPayloadPtr {
    consensus_common::ConsensusCommonFactory::create_empty_block_payload()
}

/*
    --------------------------------------------------------------------
    No-op session aspects required by `ValidationController::new`
    (copied from the collation-controller unit tests — the controller
    constructor needs *a* callbacks/telemetry handle, but these focused
    tests assert on controller state, not dispatch).
    --------------------------------------------------------------------
*/

struct NoopSessionListener;

impl consensus_common::SessionListener for NoopSessionListener {
    fn on_candidate(
        &self,
        _source_info: consensus_common::BlockSourceInfo,
        _root_hash: UInt256,
        _data: consensus_common::BlockPayloadPtr,
        _collated_data: consensus_common::BlockPayloadPtr,
        _callback: consensus_common::ValidatorBlockCandidateDecisionCallback,
    ) {
    }

    fn on_generate_slot(
        &self,
        _source_info: consensus_common::BlockSourceInfo,
        _request: consensus_common::AsyncCollationRequestPtr,
        _parent: consensus_common::CollationParentHint,
        _callback: consensus_common::ValidatorBlockCandidateCallback,
    ) {
    }

    fn on_block_committed(
        &self,
        _source_info: consensus_common::BlockSourceInfo,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _data: consensus_common::BlockPayloadPtr,
        _signatures: ton_block::BlockSignaturesVariant,
        _approve_signatures: Vec<(
            consensus_common::PublicKeyHash,
            consensus_common::BlockPayloadPtr,
        )>,
        _stats: consensus_common::SessionStats,
    ) {
    }

    fn on_block_skipped(&self, _round: u32) {}

    fn get_approved_candidate(
        &self,
        _source: consensus_common::PublicKey,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: consensus_common::ValidatorBlockCandidateCallback,
    ) {
    }
}

struct NoopCallbackQueue;

impl crate::task_queue::TaskQueue<crate::task_queue::CallbackTaskPtr> for NoopCallbackQueue {
    fn is_overloaded(&self) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        true
    }

    fn post_closure(&self, _task: crate::task_queue::CallbackTaskPtr) {}

    fn pull_closure(
        &self,
        _timeout: Duration,
        _last_warn_dump_time: &mut SystemTime,
    ) -> Option<crate::task_queue::CallbackTaskPtr> {
        None
    }

    fn flush(&self) {}
}

/// `Arc<SessionCallbacks>` with a dangling listener + no-op queue. Dispatch is
/// asserted by `tests/test_session_processor.rs`, so the dead `Weak` makes any
/// accidental `notify_candidate` a silent no-op.
fn make_callbacks() -> Arc<SessionCallbacks> {
    let dead_listener: crate::SessionListenerPtr = {
        let listener: Arc<NoopSessionListener> = Arc::new(NoopSessionListener);
        Arc::downgrade(&listener) as crate::SessionListenerPtr
    };
    let queue: crate::task_queue::CallbackTaskQueuePtr = Arc::new(NoopCallbackQueue);
    Arc::new(SessionCallbacks::new(
        SessionId::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        false,
        queue,
        dead_listener,
    ))
}

fn make_telemetry(description: &SessionDescription) -> Arc<SessionTelemetry> {
    Arc::new(SessionTelemetry::new(
        crate::MetricsHandle::new(None),
        description,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
        0,
        Duration::from_secs(30),
        SystemTime::now(),
    ))
}

/*
    --------------------------------------------------------------------
    Recording `ControllerQueue` fake (concrete to `ValidationController`).
    Mirrors `tests/test_controller_queue.rs::RecordingQueue<C>`: enqueues
    posted tasks into lock-free crossbeam queues instead of running them,
    so a test can replay them against a `&mut ValidationController` plus a
    freshly-built backend — no `SessionProcessor` anywhere.
    --------------------------------------------------------------------
*/

struct RecordingValidationQueue {
    immediate: SegQueue<ControllerTask<ValidationController>>,
    delayed: SegQueue<(SystemTime, ControllerTask<ValidationController>)>,
}

impl RecordingValidationQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self { immediate: SegQueue::new(), delayed: SegQueue::new() })
    }

    fn immediate_count(&self) -> usize {
        self.immediate.len()
    }

    fn delayed_count(&self) -> usize {
        self.delayed.len()
    }

    /// Drain every recorded delayed task (FIFO) via `run_one`, returning the
    /// scheduled `at` times in post order. `run_one` builds a fresh borrowing
    /// backend and runs the task against its `&mut ValidationController` —
    /// exactly what the production composition root does per re-entry.
    fn run_delayed(
        &self,
        mut run_one: impl FnMut(ControllerTask<ValidationController>),
    ) -> Vec<SystemTime> {
        let mut times = Vec::new();
        while let Some((at, task)) = self.delayed.pop() {
            times.push(at);
            run_one(task);
        }
        times
    }
}

impl ControllerQueue<ValidationController> for RecordingValidationQueue {
    fn post_boxed(&self, task: ControllerTask<ValidationController>) {
        self.immediate.push(task);
    }

    fn post_delayed_boxed(&self, at: SystemTime, task: ControllerTask<ValidationController>) {
        self.delayed.push((at, task));
    }
}

fn queue_ptr(q: &Arc<RecordingValidationQueue>) -> ControllerQueuePtr<ValidationController> {
    q.clone()
}

/*
    --------------------------------------------------------------------
    Fake `ValidationBackend` — the validation analogue of
    `FakeCollationBackend`. Owns a real `SimplexState` (so the
    `evaluate_wait_for_parent` walk reads true FSM state), returns
    configurable scalars, and records the effects (`request_wake`,
    `request_candidate`) so the gates can be asserted with no
    `SessionProcessor`.
    --------------------------------------------------------------------
*/

struct FakeValidationBackend {
    simplex_state: SimplexState,
    finalized_head_seqno: Option<u32>,
    parent_tip: ParentTipResolution,
    parent_gen_utime_ms: Option<u64>,
    wake: Cell<u32>,
    wake_at: RefCell<Vec<SystemTime>>,
    requested: RefCell<Vec<(SlotIndex, UInt256, Option<Duration>)>>,
}

impl FakeValidationBackend {
    fn new(description: &SessionDescription) -> Self {
        Self {
            simplex_state: SimplexState::new(description).expect("SimplexState::new must succeed"),
            finalized_head_seqno: None,
            parent_tip: ParentTipResolution::Unresolved,
            parent_gen_utime_ms: None,
            wake: Cell::new(0),
            wake_at: RefCell::new(Vec::new()),
            requested: RefCell::new(Vec::new()),
        }
    }

    fn with_finalized_head_seqno(mut self, seqno: Option<u32>) -> Self {
        self.finalized_head_seqno = seqno;
        self
    }

    fn with_parent_tip(mut self, resolution: ParentTipResolution) -> Self {
        self.parent_tip = resolution;
        self
    }

    fn wake_count(&self) -> u32 {
        self.wake.get()
    }

    fn requested_count(&self) -> usize {
        self.requested.borrow().len()
    }
}

impl ValidationBackend for FakeValidationBackend {
    fn finalized_head_seqno(&self) -> Option<u32> {
        self.finalized_head_seqno
    }

    fn request_wake(&self) {
        self.wake.set(self.wake.get() + 1);
    }

    fn request_wake_at(&self, at: SystemTime) {
        self.wake_at.borrow_mut().push(at);
    }

    fn simplex_state(&self) -> &SimplexState {
        &self.simplex_state
    }

    fn resolve_parent_tip(&self, _parent_id: Option<&RawCandidateId>) -> ParentTipResolution {
        self.parent_tip.clone()
    }

    fn parent_gen_utime_ms(&self, _parent: &CandidateParentInfo) -> Option<u64> {
        self.parent_gen_utime_ms
    }

    fn request_candidate(&self, slot: SlotIndex, hash: UInt256, delay: Option<Duration>) {
        self.requested.borrow_mut().push((slot, hash, delay));
    }
}

/*
    --------------------------------------------------------------------
    Constructor wiring + accessor round-trips
    --------------------------------------------------------------------
*/

#[test]
fn new_controller_starts_empty() {
    let (desc, _nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    assert_eq!(ctrl.pending_validation_count(), 0);
    assert_eq!(ctrl.rejected_count(), 0);
    assert_eq!(ctrl.approved_count(), 0);
    assert_eq!(ctrl.validated_count(), 0);
    assert!(ctrl.validated_is_empty());
}

#[test]
fn accessor_roundtrips_across_maps() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    let id = make_candidate_id(4, 0xA0);

    // pending_validations
    let raw = make_non_empty_raw(id.clone(), None, &nodes);
    assert!(ctrl.insert_pending_validation(id.clone(), make_pending(raw, 4)).is_none());
    assert!(ctrl.pending_validation_contains(&id));
    assert!(ctrl.remove_pending_validation(&id).is_some());
    assert!(!ctrl.pending_validation_contains(&id));

    // pending_approve
    assert!(ctrl.insert_pending_approve(id.clone()));
    assert!(ctrl.pending_approve_contains(&id));
    assert!(ctrl.remove_pending_approve(&id));
    assert!(!ctrl.pending_approve_contains(&id));

    // rejected
    assert!(ctrl.insert_rejected(id.clone()));
    assert!(ctrl.is_rejected(&id));
    assert_eq!(ctrl.rejected_count(), 1);

    // approved
    assert!(ctrl.insert_approved(id.clone(), (SystemTime::now(), empty_payload())).is_none());
    assert!(ctrl.approved_contains(&id));

    // validation_attempt_map
    assert!(ctrl.insert_validation_attempt(id.clone(), 0).is_none());
    assert_eq!(ctrl.validation_attempt(&id), Some(0));
    ctrl.bump_or_init_validation_attempt(id.clone());
    assert_eq!(ctrl.validation_attempt(&id), Some(1));
    assert_eq!(ctrl.remove_validation_attempt(&id), Some(1));

    // validated FIFO
    let candidate = make_non_empty_raw(id.clone(), None, &nodes).resolve(None).unwrap();
    ctrl.push_validated(candidate);
    assert_eq!(ctrl.validated_count(), 1);
    assert!(ctrl.pop_validated().is_some());
    assert!(ctrl.validated_is_empty());
}

#[test]
fn prune_below_drops_stale_slots_across_all_maps() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    // Two cohorts: slot 3 (below cutoff) and slot 7 (kept).
    let low = make_candidate_id(3, 0x10);
    let high = make_candidate_id(7, 0x20);

    ctrl.insert_pending_validation(
        low.clone(),
        make_pending(make_non_empty_raw(low.clone(), None, &nodes), 3),
    );
    ctrl.insert_pending_validation(
        high.clone(),
        make_pending(make_non_empty_raw(high.clone(), None, &nodes), 7),
    );
    ctrl.insert_pending_approve(low.clone());
    ctrl.insert_pending_approve(high.clone());
    ctrl.insert_pending_reject(low.clone(), empty_payload());
    ctrl.insert_pending_reject(high.clone(), empty_payload());
    ctrl.insert_rejected(low.clone());
    ctrl.insert_rejected(high.clone());
    ctrl.insert_approved(low.clone(), (SystemTime::now(), empty_payload()));
    ctrl.insert_approved(high.clone(), (SystemTime::now(), empty_payload()));
    ctrl.insert_validation_attempt(low.clone(), 0);
    ctrl.insert_validation_attempt(high.clone(), 0);
    ctrl.push_validated(make_non_empty_raw(low.clone(), None, &nodes).resolve(None).unwrap());
    ctrl.push_validated(make_non_empty_raw(high.clone(), None, &nodes).resolve(None).unwrap());

    ctrl.prune_below(SlotIndex::new(5));

    // Slot-3 cohort gone.
    assert!(!ctrl.pending_validation_contains(&low));
    assert!(!ctrl.pending_approve_contains(&low));
    assert!(!ctrl.pending_reject_contains(&low));
    assert!(!ctrl.is_rejected(&low));
    assert!(!ctrl.approved_contains(&low));
    assert!(!ctrl.validation_attempt_contains(&low));

    // Slot-7 cohort kept.
    assert!(ctrl.pending_validation_contains(&high));
    assert!(ctrl.pending_approve_contains(&high));
    assert!(ctrl.pending_reject_contains(&high));
    assert!(ctrl.is_rejected(&high));
    assert!(ctrl.approved_contains(&high));
    assert!(ctrl.validation_attempt_contains(&high));

    // Only the slot-7 validated candidate survives.
    assert_eq!(ctrl.validated_count(), 1);
    assert_eq!(ctrl.pop_validated().map(|c| c.id.slot), Some(SlotIndex::new(7)));
}

/*
    --------------------------------------------------------------------
    evaluate_wait_for_parent — pure policy over a fresh FSM
    (first_non_finalized = 0, nothing notarized)
    --------------------------------------------------------------------
*/

#[test]
fn evaluate_wait_for_parent_ready_for_genesis_candidate() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let state = SimplexState::new(&desc).unwrap();

    let id = make_candidate_id(0, 0x01);
    let pending = make_pending(make_non_empty_raw(id, None, &nodes), 0);

    assert_eq!(ctrl.evaluate_wait_for_parent(&pending, &state), WaitForParentDecision::Ready);
}

#[test]
fn evaluate_wait_for_parent_rejects_parent_at_or_after_candidate_slot() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let state = SimplexState::new(&desc).unwrap();

    // parent slot (1) >= candidate slot (1) is structurally invalid.
    let id = make_candidate_id(1, 0x02);
    let parent = make_candidate_id(1, 0x03);
    let pending = make_pending(make_non_empty_raw(id, Some(parent), &nodes), 1);

    assert!(matches!(
        ctrl.evaluate_wait_for_parent(&pending, &state),
        WaitForParentDecision::Reject(_)
    ));
}

#[test]
fn evaluate_wait_for_parent_waits_for_unnotarized_parent() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let state = SimplexState::new(&desc).unwrap();

    // Candidate at slot 2 with a parent at slot 0 that is not yet notarized:
    // the gate must wait for the parent's notarization certificate.
    let id = make_candidate_id(2, 0x04);
    let parent = make_candidate_id(0, 0x05);
    let pending = make_pending(make_non_empty_raw(id, Some(parent), &nodes), 2);

    assert_eq!(ctrl.evaluate_wait_for_parent(&pending, &state), WaitForParentDecision::Wait);
}

/*
    --------------------------------------------------------------------
    check_validation gating
    --------------------------------------------------------------------
*/

#[test]
fn check_validation_terminally_rejects_failing_wait_for_parent() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    // parent slot >= candidate slot => WaitForParentDecision::Reject; with the
    // default `validation_retry_attempts = 0`, the reject is terminal.
    let id = make_candidate_id(1, 0x06);
    let parent = make_candidate_id(1, 0x07);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), Some(parent), &nodes), 1),
    );

    ctrl.check_validation(&backend);

    assert!(ctrl.is_rejected(&id), "candidate failing the WaitForParent gate must be rejected");
    assert!(ctrl.pending_reject_contains(&id), "a rejection payload must be staged");
    assert!(
        !ctrl.pending_validation_contains(&id),
        "rejected candidate must leave pending_validations"
    );
    assert_eq!(q.immediate_count(), 0);
    assert_eq!(q.delayed_count(), 0);
}

#[test]
fn check_validation_skips_when_attempts_exhausted() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    // Genesis-Ready candidate, but its attempt counter already sits at the
    // default cap (0), so the attempt-limit gate must skip it untouched.
    let id = make_candidate_id(0, 0x08);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );
    ctrl.insert_validation_attempt(id.clone(), 0);

    ctrl.check_validation(&backend);

    assert!(ctrl.pending_validation_contains(&id), "candidate must stay pending");
    assert!(!ctrl.pending_approve_contains(&id), "candidate must not be dispatched");
    assert!(!ctrl.is_rejected(&id));
    assert_eq!(q.immediate_count(), 0);
}

#[test]
fn check_validation_dispatches_ready_normal_candidate() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    // Genesis-Ready, no parent (so the min-block-interval gate is skipped), no
    // attempt entry yet: it should be handed to `try_approve_block`, which marks
    // it pending_approve and bumps the attempt. The no-op listener never invokes
    // the decision callback, so nothing is posted synchronously.
    let id = make_candidate_id(0, 0x09);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );

    ctrl.check_validation(&backend);

    assert!(
        ctrl.pending_approve_contains(&id),
        "ready normal candidate must be dispatched for approval"
    );
    assert_eq!(
        ctrl.validation_attempt(&id),
        Some(0),
        "dispatch must initialise the attempt counter"
    );
    assert!(!ctrl.approved_contains(&id), "no synchronous approval without a validator verdict");
    assert!(!ctrl.is_rejected(&id));
    assert_eq!(q.immediate_count(), 0, "no decision is posted until the listener fires");
}

/*
    --------------------------------------------------------------------
    try_approve_block — empty-block auto-decision paths
    --------------------------------------------------------------------
*/

#[test]
fn try_approve_block_empty_auto_approves_on_matching_tip() {
    let (desc, _nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    let referenced = make_block_id(7, 0x30);
    let backend = FakeValidationBackend::new(&desc)
        .with_parent_tip(ParentTipResolution::Resolved(referenced.clone()));

    let id = make_candidate_id(2, 0x0A);
    let parent = make_candidate_id(1, 0x0B);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_empty_raw(id.clone(), parent, referenced), 2),
    );

    ctrl.try_approve_block(
        &backend,
        &id,
        SlotIndex::new(2),
        ValidatorIndex::new(0),
        SystemTime::now(),
    );

    assert!(ctrl.approved_contains(&id), "empty block matching the parent tip must auto-approve");
    assert_eq!(ctrl.validated_count(), 1, "approved candidate must be queued for the FSM");
    assert!(
        !ctrl.pending_validation_contains(&id),
        "approved candidate leaves pending_validations"
    );
    assert!(!ctrl.pending_approve_contains(&id), "the in-flight flag is cleared on decision");
}

#[test]
fn try_approve_block_empty_rejects_on_tip_mismatch() {
    let (desc, _nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    // Referenced block differs from the resolved expected tip; with default
    // `validation_retry_attempts = 0` the failure is terminal.
    let referenced = make_block_id(7, 0x30);
    let expected = make_block_id(8, 0x31);
    let backend =
        FakeValidationBackend::new(&desc).with_parent_tip(ParentTipResolution::Resolved(expected));

    let id = make_candidate_id(2, 0x0C);
    let parent = make_candidate_id(1, 0x0D);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_empty_raw(id.clone(), parent, referenced), 2),
    );

    ctrl.try_approve_block(
        &backend,
        &id,
        SlotIndex::new(2),
        ValidatorIndex::new(0),
        SystemTime::now(),
    );

    assert!(ctrl.is_rejected(&id), "empty block referencing the wrong tip must be rejected");
    assert!(ctrl.pending_reject_contains(&id));
    assert!(!ctrl.approved_contains(&id));
    assert!(!ctrl.pending_validation_contains(&id));
}

#[test]
fn try_approve_block_empty_requests_missing_parent_and_stays_pending() {
    let (desc, _nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );

    let missing = make_candidate_id(1, 0x0E);
    let backend = FakeValidationBackend::new(&desc)
        .with_parent_tip(ParentTipResolution::MissingParent(missing.clone()));

    let id = make_candidate_id(2, 0x0F);
    let referenced = make_block_id(7, 0x30);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_empty_raw(id.clone(), missing.clone(), referenced), 2),
    );

    ctrl.try_approve_block(
        &backend,
        &id,
        SlotIndex::new(2),
        ValidatorIndex::new(0),
        SystemTime::now(),
    );

    assert_eq!(backend.requested_count(), 1, "the missing parent must be requested");
    assert!(
        ctrl.pending_validation_contains(&id),
        "candidate stays pending while the parent is missing"
    );
    assert!(!ctrl.pending_approve_contains(&id), "the in-flight flag is cleared while waiting");
    assert!(!ctrl.approved_contains(&id));
    assert!(!ctrl.is_rejected(&id));
}

/*
    --------------------------------------------------------------------
    candidate_decision_ok / candidate_decision_fail
    --------------------------------------------------------------------
*/

#[test]
fn candidate_decision_ok_approves_enqueues_and_wakes() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    let id = make_candidate_id(0, 0x21);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );
    ctrl.insert_pending_approve(id.clone());

    ctrl.candidate_decision_ok(
        &backend,
        SlotIndex::new(0),
        id.clone(),
        SystemTime::now(),
        SystemTime::now(),
    );

    assert!(ctrl.approved_contains(&id), "successful verdict must approve the candidate");
    assert_eq!(ctrl.validated_count(), 1, "approved candidate must be queued for the FSM");
    assert!(!ctrl.pending_validation_contains(&id));
    assert!(!ctrl.pending_approve_contains(&id));
    assert_eq!(backend.wake_count(), 1, "a successful decision wakes the main loop");
}

#[test]
fn candidate_decision_ok_drops_late_callback_without_pending_entry() {
    let (desc, _nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    // No pending_validation entry: the callback arrived after the candidate was
    // already cleaned up. It must be dropped (not approved) and must not wake.
    let id = make_candidate_id(0, 0x22);
    ctrl.insert_pending_approve(id.clone());
    ctrl.insert_validation_attempt(id.clone(), 0);

    ctrl.candidate_decision_ok(
        &backend,
        SlotIndex::new(0),
        id.clone(),
        SystemTime::now(),
        SystemTime::now(),
    );

    assert!(!ctrl.approved_contains(&id));
    assert_eq!(ctrl.validated_count(), 0);
    assert!(!ctrl.pending_approve_contains(&id), "late callback clears the in-flight flag");
    assert_eq!(ctrl.validation_attempt(&id), None, "late callback clears the attempt counter");
    assert_eq!(backend.wake_count(), 0, "a dropped late callback does not wake");
}

#[test]
fn candidate_decision_ok_drops_already_finalized_candidate() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    // Candidate at slot 0 has block seqno 1; a finalized head at seqno 5 is
    // strictly ahead, so the verdict must be dropped.
    let backend = FakeValidationBackend::new(&desc).with_finalized_head_seqno(Some(5));

    let id = make_candidate_id(0, 0x23);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );
    ctrl.insert_pending_approve(id.clone());

    ctrl.candidate_decision_ok(
        &backend,
        SlotIndex::new(0),
        id.clone(),
        SystemTime::now(),
        SystemTime::now(),
    );

    assert!(!ctrl.approved_contains(&id), "finalized-past candidate must not approve");
    assert_eq!(ctrl.validated_count(), 0);
    assert!(!ctrl.pending_validation_contains(&id), "finalized-past candidate is dropped");
}

#[test]
fn candidate_decision_fail_terminally_rejects_when_attempts_exhausted() {
    let (desc, nodes) = make_desc(SessionOptions::default());
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    // Default `validation_retry_attempts = 0`: attempt 0 is not < 0, so the
    // failure is terminal on the first verdict.
    let id = make_candidate_id(0, 0x24);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );
    ctrl.insert_pending_approve(id.clone());
    ctrl.insert_validation_attempt(id.clone(), 0);

    ctrl.candidate_decision_fail(&backend, SlotIndex::new(0), id.clone(), error!("boom"));

    assert!(ctrl.is_rejected(&id), "exhausted failure must terminally reject");
    assert!(ctrl.pending_reject_contains(&id), "a rejection payload must be staged");
    assert!(!ctrl.pending_validation_contains(&id));
    assert!(!ctrl.pending_approve_contains(&id));
    assert_eq!(q.delayed_count(), 0, "a terminal failure posts no retry");
}

#[test]
fn candidate_decision_fail_retries_through_queue_while_attempts_remain() {
    // Allow one retry so the retry branch is reached.
    let opts = SessionOptions { validation_retry_attempts: 2, ..SessionOptions::default() };
    let (desc, nodes) = make_desc(opts);
    let q = RecordingValidationQueue::new();
    let mut ctrl = ValidationController::new(
        queue_ptr(&q),
        make_callbacks(),
        desc.clone(),
        make_telemetry(&desc),
        None,
    );
    let backend = FakeValidationBackend::new(&desc);

    let id = make_candidate_id(0, 0x25);
    ctrl.insert_pending_validation(
        id.clone(),
        make_pending(make_non_empty_raw(id.clone(), None, &nodes), 0),
    );
    ctrl.insert_pending_approve(id.clone());
    ctrl.insert_validation_attempt(id.clone(), 0);

    ctrl.candidate_decision_fail(&backend, SlotIndex::new(0), id.clone(), error!("transient"));

    // A delayed retry is posted; the candidate is neither rejected nor dropped.
    assert_eq!(q.delayed_count(), 1, "a failure with attempts remaining schedules a retry");
    assert!(ctrl.pending_validation_contains(&id), "retrying candidate stays pending");
    assert!(!ctrl.is_rejected(&id));
    assert!(
        ctrl.pending_approve_contains(&id),
        "the in-flight flag is still set until the retry runs"
    );

    // Replay the delayed retry against the controller + a freshly built backend,
    // exactly as the SXMAIN composition root would: it clears the in-flight flag
    // and wakes the main loop so the candidate is re-dispatched next iteration.
    let times = q.run_delayed(|task| {
        let mut replay_backend = FakeValidationBackend::new(&desc);
        task(&mut ctrl, &mut replay_backend);
        assert_eq!(replay_backend.wake_count(), 1, "the retry wakes the main loop");
    });
    assert_eq!(times.len(), 1, "exactly one retry was scheduled");
    assert!(!ctrl.pending_approve_contains(&id), "the replayed retry clears the in-flight flag");
}
