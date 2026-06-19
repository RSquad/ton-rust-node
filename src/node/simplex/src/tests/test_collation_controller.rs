/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for
//! [`crate::collation_controller::CollationController`].
//!
//! Included directly from `collation_controller.rs` via `#[path]` so tests
//! can reach the private accessor surface and the inner precollation /
//! self-collation registry types without widening visibility. Mirrors the
//! convention used by `tests/test_candidate_book.rs` and
//! `tests/test_database_controller.rs`.
//!
//! Scope (state mechanics and pure-policy decisions only — full collation
//! orchestration is still covered by `tests/test_session_processor.rs`):
//!
//! - Constructor wiring: all maps empty, all cursors `None`.
//! - `precollated_blocks` accessor set: insert/remove/contains/iter/keys
//!   round-trip + `precollated_count` / `precollated_is_empty`.
//! - `precollated_blocks_next_request_id` monotonic post-increment.
//! - `precollated_blocks_max_slot` note/reset behaviour, including the
//!   "no high-water mark yet" case and the strict-greater-than guard.
//! - `earliest_collation_time` set/get round-trip.
//! - `local_chain_head` set/get round-trip + reference identity.
//! - `last_generated_slot` set/get.
//! - `generated_parent_cache` + `generated_parent_gen_utime_ms_cache`
//!   insert / lookup / clear.
//! - `invalidate_local_chain_head` clears the head AND both
//!   generated-parent caches.
//! - `cancel_pending_precollations` cancels every in-flight request,
//!   drains the precollation map, resets the high-water cursor, and
//!   returns the snapshot of cancelled slots.
//! - `reset` flushes the pipeline end-to-end: cancels in-flight precollations,
//!   forgets each cancelled slot's self-collation tracking via the backend
//!   (`forget_self_collation_tracking`), and invalidates the chain head.
//! - `remove_precollated_with_log` returns the actually-removed flag and
//!   leaves the rest of the pipeline untouched.
//! - `resolve_parent_block_id` lookup order: synchronous cache → book →
//!   miss.
//! - `try_begin_collation_slot` truth table: precollation pending → None,
//!   not-leader → None, leader + no pending → `Some(self_idx)`.
//! - `create_pending_collation_request` allocates id, registers entry,
//!   updates the high-water cursor.
//! - `update_collation_pacing` advances `earliest_collation_time` by the
//!   description's `target_rate` from the held description clock (no threaded
//!   time arg).
//! - `should_generate_empty_block` truth table:
//!   parent_before_split → always true, MC finalization-lag branch,
//!   shardchain MC-lag-threshold branch, "no finalization tracking" →
//!   false.
//! - `resolve_parent_gen_utime_ms` lookup order: cache → book →
//!   `local_chain_head` → miss.
//! - `prepare_collation` truth table: unresolved named parent →
//!   `WaitingForParent`; genesis (no parent) → `Ready` over the session-start
//!   prev-blocks with `is_first_session_block`; resolved parent → `Ready` with
//!   the seqno derived from the parent block id; recent parent gen-time on the
//!   masterchain → `Deferred`.
//! - `compute_collation_timing`: shardchains dispatch `target_rate` early
//!   (`dispatch_time = min_gen_time - target_rate`) while the masterchain
//!   dispatches at `min_gen_time`.
//! - `collation_deadlines`: masterchain soft cutoff = `slot_start + target_rate`,
//!   shardchain soft cutoff = `slot_start`, and the window-end hard cap shrinks as
//!   the slot advances within its leader window (clamping a zero window to 1).
//! - `block_generation_active`: `clear_block_generation_if` only releases the
//!   marker for the matching request id; `reset` clears it unconditionally;
//!   `clear_stale_block_generation` drops it (and cancels the pending request) only
//!   when the tracked window no longer matches the current leader window.
//! - single in-flight guard + per-slot deadline wake: `execute_collation_attempt`
//!   declines a second real dispatch while one is in flight; `on_slot_deadline`
//!   keeps the real alive, advances the horizon by `target_rate` and re-arms while
//!   the same collation runs in the same window, no-ops on a stale request id or
//!   once the loop slot runs past the window, and clears the stale marker when the
//!   leader window has advanced.
//! - per-slot empty fillers + late re-tag: `on_slot_deadline` publishes a
//!   state-preserving empty filler for the elapsed slot (keeping the real alive and
//!   its precollation entry intact), keeps waiting on the same slot when no filler
//!   can be placed, and no-ops once the loop slot runs past the leader window;
//!   `on_collation_complete` re-tags a late tracked real onto the current chain
//!   head (`local_chain_head.slot + 1`) rather than its dispatch slot, consuming the
//!   precollation entry and clearing the in-flight marker.
//! - `allow_empty` finalization-staleness gate: empties are suppressed once
//!   finalization stalls past `no_empty_blocks_on_error_timeout` and the wake
//!   refreshes the finalization timestamp before evaluating the gate.
//! - genuine-error path: a still-current slot schedules exactly one fixed-backoff
//!   restart that reuses the original attempt's pinned soft/hard deadlines and
//!   budget anchor (no fresh window from the advanced clock); a passed slot or an
//!   advanced leader window drops without a restart; an `allow_empty`-satisfied
//!   error recovers with one empty block (re-tagging past any fillers).
//! - `Debug` impl smoke test.
//!
//! The self-collation observability funnel (start/generated/acceptance
//! tracking + the generated-candidate validation watch) now lives in
//! [`crate::session_telemetry::SessionTelemetry`]; its state mechanics
//! and counter emission are covered by `tests/test_session_telemetry.rs`.

use super::*;
use crate::{block::WindowIndex, candidate_book::ReceivedCandidate, SessionNode, SessionOptions};
use consensus_common::{AsyncCollationRequest, ConsensusCommonFactory};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use ton_block::{Ed25519KeyOption, ShardIdent, UInt256, ZeroizingBytes};

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

fn make_parent_info(slot: u32, hash_byte: u8) -> CandidateParentInfo {
    let mut hash = [0u8; 32];
    hash[0] = hash_byte;
    CandidateParentInfo { slot: SlotIndex::new(slot), hash: UInt256::from(hash) }
}

/// Build a `SessionDescription` with the local node at index 0. The shard
/// argument lets individual tests choose masterchain vs. shardchain for
/// the empty-block-policy branch.
fn make_description_with_opts(
    node_count: u32,
    shard: ShardIdent,
    opts: SessionOptions,
) -> SessionDescription {
    let nodes: Vec<SessionNode> = (0..node_count)
        .map(|_| {
            let public_key =
                Ed25519KeyOption::<ZeroizingBytes>::generate().expect("key gen must succeed");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight: 1 }
        })
        .collect();
    let local_key = nodes[0].public_key.clone();

    SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .expect("SessionDescription::new must succeed for test fixture")
}

fn make_mc_description(node_count: u32) -> SessionDescription {
    make_description_with_opts(node_count, ShardIdent::masterchain(), SessionOptions::default())
}

/// Masterchain description with an explicit `slots_per_leader_window`. The
/// per-slot filler / late re-tag only make sense when a leader owns
/// several consecutive slots inside one window (the default `spw == 1` makes
/// every slot its own window, so a wake would immediately run past the window).
fn make_mc_description_spw(node_count: u32, slots_per_leader_window: u32) -> SessionDescription {
    let opts = SessionOptions { slots_per_leader_window, ..SessionOptions::default() };
    make_description_with_opts(node_count, ShardIdent::masterchain(), opts)
}

fn make_shard_description(node_count: u32, mc_lag_threshold: Option<u32>) -> SessionDescription {
    let opts = SessionOptions {
        empty_block_mc_lag_threshold: mc_lag_threshold,
        ..SessionOptions::default()
    };
    make_description_with_opts(
        node_count,
        ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
        opts,
    )
}

/// No-op `ControllerQueue` fake for the collation unit tests. The pure-policy /
/// state-mechanics tests here never post deferred work, so construction only
/// needs *a* queue handle; the posting paths are exercised by
/// `tests/test_session_processor.rs` against the real `CollationQueueAdapter`.
struct NoopCollationQueue;

impl crate::controller_queue::ControllerQueue<CollationController> for NoopCollationQueue {
    fn post_boxed(&self, _task: crate::controller_queue::ControllerTask<CollationController>) {}

    fn post_delayed_boxed(
        &self,
        _at: SystemTime,
        _task: crate::controller_queue::ControllerTask<CollationController>,
    ) {
    }
}

/// Queue handle the controller constructor expects, coerced to the trait
/// object pointer from the no-op fake.
fn test_collation_queue() -> crate::controller_queue::ControllerQueuePtr<CollationController> {
    Arc::new(NoopCollationQueue)
}

/// Recording `ControllerQueue` that captures `post_delayed` deadlines so the
/// per-slot deadline wake re-arm can be asserted without a
/// `SessionProcessor`. The boxed task itself is dropped - the wake tests assert on
/// the scheduled deadline, while the handler's state effects are observed directly
/// on `block_generation_active`.
#[derive(Default)]
struct RecordingCollationQueue {
    delayed: std::sync::Mutex<Vec<SystemTime>>,
}

impl RecordingCollationQueue {
    /// Snapshot of the deadlines passed to `post_delayed`.
    fn delayed(&self) -> Vec<SystemTime> {
        self.delayed.lock().expect("recording queue mutex").clone()
    }
}

impl crate::controller_queue::ControllerQueue<CollationController> for RecordingCollationQueue {
    fn post_boxed(&self, _task: crate::controller_queue::ControllerTask<CollationController>) {}

    fn post_delayed_boxed(
        &self,
        at: SystemTime,
        _task: crate::controller_queue::ControllerTask<CollationController>,
    ) {
        self.delayed.lock().expect("recording queue mutex").push(at);
    }
}

/// Build a controller wired to a [`RecordingCollationQueue`], returning the queue
/// handle so a test can assert which `post_delayed` deadlines were scheduled.
fn ctrl_with_recording_queue(
    description: Arc<SessionDescription>,
) -> (CollationController, Arc<RecordingCollationQueue>) {
    let queue = Arc::new(RecordingCollationQueue::default());
    let callbacks = make_callbacks();
    let telemetry = make_telemetry(&description);
    let ctrl = CollationController::new(queue.clone(), callbacks, description, telemetry, None);
    (ctrl, queue)
}

/// No-op [`SessionListener`](consensus_common::SessionListener) so the
/// controller's `SessionCallbacks` can be constructed in the focused unit
/// tests. The generate-slot dispatch path is covered end-to-end by
/// `tests/test_session_processor.rs`; these tests only need the controller
/// constructed, so every method is a no-op.
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

/// No-op callback task queue so `SessionCallbacks::new` is satisfied.
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

/// Build an `Arc<SessionCallbacks>` with a dangling listener + no-op queue —
/// enough for the controller constructor. Dispatch is asserted by
/// `tests/test_session_processor.rs`, not here, so the dead `Weak` makes any
/// accidental dispatch a silent no-op.
fn make_callbacks() -> Arc<SessionCallbacks> {
    let dead_listener: crate::SessionListenerPtr = {
        let listener: Arc<NoopSessionListener> = Arc::new(NoopSessionListener);
        Arc::downgrade(&listener) as crate::SessionListenerPtr
        // `listener` drops here, leaving the `Weak` dangling.
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

/// Build an `Arc<SessionTelemetry>` for the controller constructor.
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

/// Build a controller for the focused unit tests, supplying the no-op
/// callbacks + telemetry the constructor now requires.
fn mk_ctrl(description: Arc<SessionDescription>) -> CollationController {
    let callbacks = make_callbacks();
    let telemetry = make_telemetry(&description);
    CollationController::new(test_collation_queue(), callbacks, description, telemetry, None)
}

/// Build a fresh controller for the state-mechanics / pure-policy tests
/// that assert on the slot, candidate, precollation and cursor maps. Bound
/// to a default single-node masterchain description — the held description
/// only feeds session-id logging and the leader/option lookups exercised by
/// the dedicated `try_begin_collation_slot` / `update_collation_pacing` /
/// `should_generate_empty_block` tests (which build their own description).
/// The self-collation observability funnel now lives in `SessionTelemetry`,
/// so no telemetry aspect is needed here.
fn new_ctrl() -> CollationController {
    mk_ctrl(Arc::new(make_mc_description(1)))
}

/// Build a `ReceivedCandidate` carrying a `BlockIdExt` so book lookups
/// in `resolve_parent_block_id` / `resolve_parent_gen_utime_ms` can be
/// exercised without spinning up the full simplex layer.
fn make_received(slot: u32, block_id: BlockIdExt, gen_utime_ms: Option<u64>) -> ReceivedCandidate {
    ReceivedCandidate {
        slot: SlotIndex::new(slot),
        source_idx: ValidatorIndex::new(0),
        candidate_hash_data_bytes: vec![0xAB, 0xCD],
        block_id,
        root_hash: UInt256::from([2u8; 32]),
        file_hash: UInt256::from([3u8; 32]),
        data: ConsensusCommonFactory::create_block_payload(vec![]),
        collated_data: ConsensusCommonFactory::create_block_payload(vec![]),
        gen_utime_ms,
        receive_time: SystemTime::now(),
        is_empty: false,
        parent_id: None,
    }
}

/// Build a `LocalChainHead` for chain-head accessor / invalidate tests.
fn make_local_chain_head(window: u32, slot: u32, parent: CandidateParentInfo) -> LocalChainHead {
    LocalChainHead {
        window: WindowIndex::new(window),
        slot: SlotIndex::new(slot),
        parent_info: parent,
        gen_utime_ms: Some(1_700_000_000_000),
    }
}

/// Build a `PrecollatedBlock` with a fresh, non-cancelled
/// `AsyncRequestImpl` so cancel-on-drain semantics can be asserted.
fn make_precollated(request_id: u32, parent: Option<CandidateParentInfo>) -> PrecollatedBlock {
    let request = AsyncRequestImpl::new(request_id, false, SystemTime::now());
    PrecollatedBlock { request, result: None, parent }
}

/// Fake [`CollationBackend`] exposing a fixed FSM progress cursor + current
/// leader window and recording wake-horizon requests, so the collation policy
/// methods can be driven with no `SessionProcessor` — the collation analogue of
/// the `ValidationBackend` fakes used by the validation-controller tests.
struct FakeCollationBackend {
    first_non_progressed_slot: SlotIndex,
    current_leader_window_idx: WindowIndex,
    /// Slots the runtime reports as already generated (pipeline-fill guard).
    generated: Vec<SlotIndex>,
    /// Slots the runtime reports as pending generation (pipeline-fill guard).
    pending_generate: Vec<SlotIndex>,
    /// FSM available-parent base per slot (the precollation parent fallback used
    /// when no window-local chain head directly precedes the target).
    available_parents: std::collections::HashMap<SlotIndex, CandidateParentInfo>,
    /// `CandidateBook` stand-in backing the book-seam reads
    /// (`book_received_block_id` / `book_received_gen_utime_ms`), so the
    /// parent-resolution helpers can be driven with no `SessionProcessor` /
    /// real `CandidateBook`.
    received: std::collections::HashMap<RawCandidateId, ReceivedCandidate>,
    /// Per-block before-split flags backing the `before_split_flag` seam (the
    /// empty-block policy input read by `resolve_parent_before_split_flag`).
    before_split: std::collections::HashMap<BlockIdExt, bool>,
    /// Session-start prev-blocks backing the genesis / no-parent
    /// `prepare_collation` path (`session_start_prev_blocks` seam).
    session_start_prev_blocks: Vec<BlockIdExt>,
    /// Finalized-head / finalization-cursor scalars backing the empty-block
    /// policy seam (`finalized_head_before_split` / `finalized_head_seqno` /
    /// `last_consensus_finalized_seqno` / `last_mc_finalized_seqno`).
    finalized_head_before_split: bool,
    finalized_head_seqno: Option<u32>,
    last_consensus_finalized_seqno: Option<u32>,
    last_mc_finalized_seqno: Option<u32>,
    /// Records the last `request_wake_at(at)` so the pacing gate's effect can
    /// be asserted (interior-mutable because the trait method takes `&self`).
    wake_at: std::cell::Cell<Option<SystemTime>>,
    /// Records `forget_self_collation_tracking(slot, reason)` calls so the
    /// pipeline-reset effect can be asserted (interior-mutable; `&self` method).
    forgotten: std::cell::RefCell<Vec<(SlotIndex, String)>>,
}

impl FakeCollationBackend {
    fn new(first_non_progressed_slot: SlotIndex, current_leader_window_idx: WindowIndex) -> Self {
        Self {
            first_non_progressed_slot,
            current_leader_window_idx,
            generated: Vec::new(),
            pending_generate: Vec::new(),
            available_parents: std::collections::HashMap::new(),
            received: std::collections::HashMap::new(),
            before_split: std::collections::HashMap::new(),
            session_start_prev_blocks: Vec::new(),
            finalized_head_before_split: false,
            finalized_head_seqno: None,
            last_consensus_finalized_seqno: None,
            last_mc_finalized_seqno: None,
            wake_at: std::cell::Cell::new(None),
            forgotten: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Snapshot of the recorded `forget_self_collation_tracking` calls.
    fn forgotten(&self) -> Vec<(SlotIndex, String)> {
        self.forgotten.borrow().clone()
    }

    /// Seed a received-candidate book entry so the book-seam reads resolve.
    fn with_received(mut self, id: RawCandidateId, candidate: ReceivedCandidate) -> Self {
        self.received.insert(id, candidate);
        self
    }

    /// Seed a per-block before-split flag for the `before_split_flag` seam.
    #[allow(dead_code)]
    fn with_before_split(mut self, block_id: BlockIdExt, before_split: bool) -> Self {
        self.before_split.insert(block_id, before_split);
        self
    }
}

impl CollationBackend for FakeCollationBackend {
    fn first_non_progressed_slot(&self) -> SlotIndex {
        self.first_non_progressed_slot
    }

    fn current_leader_window_idx(&self) -> WindowIndex {
        self.current_leader_window_idx
    }

    fn has_available_parent(&self, slot: SlotIndex) -> bool {
        self.available_parents.contains_key(&slot)
    }

    fn get_available_parent(&self, slot: SlotIndex) -> Option<CandidateParentInfo> {
        self.available_parents.get(&slot).cloned()
    }

    fn book_received_block_id(&self, id: &RawCandidateId) -> Option<BlockIdExt> {
        self.received.get(id).map(|c| c.block_id.clone())
    }

    fn book_received_gen_utime_ms(&self, id: &RawCandidateId) -> Option<u64> {
        self.received.get(id).and_then(|c| c.gen_utime_ms)
    }

    fn before_split_flag(&self, parent_block_id: &BlockIdExt) -> Option<bool> {
        self.before_split.get(parent_block_id).copied()
    }

    fn request_wake_at(&self, at: SystemTime) {
        self.wake_at.set(Some(at));
    }

    fn is_generated(&self, slot: SlotIndex) -> bool {
        self.generated.contains(&slot)
    }

    fn is_pending_generate(&self, slot: SlotIndex) -> bool {
        self.pending_generate.contains(&slot)
    }

    // The per-slot generation setters and the candidate self-receive below are
    // `SessionProcessor`-side runtime/effect mutations driven by the
    // `generated_block` / `execute_collation_attempt` orchestration, not by any
    // `CollationController` policy method, so the controller unit tests never
    // invoke them. Covered end-to-end by `test_session_processor`.
    fn set_pending_generate(&mut self, _slot: SlotIndex, _value: bool) {}

    fn set_generated(&mut self, _slot: SlotIndex, _value: bool) {}

    fn set_sent_generated(&mut self, _slot: SlotIndex, _value: bool) {}

    fn session_start_prev_blocks(&self) -> Vec<BlockIdExt> {
        self.session_start_prev_blocks.clone()
    }

    fn finalized_head_before_split(&self) -> bool {
        self.finalized_head_before_split
    }

    fn finalized_head_seqno(&self) -> Option<u32> {
        self.finalized_head_seqno
    }

    fn last_consensus_finalized_seqno(&self) -> Option<u32> {
        self.last_consensus_finalized_seqno
    }

    fn last_mc_finalized_seqno(&self) -> Option<u32> {
        self.last_mc_finalized_seqno
    }

    fn forget_self_collation_tracking(&self, slot: SlotIndex, reason: &str) {
        self.forgotten.borrow_mut().push((slot, reason.to_string()));
    }

    // The publication effects below are likewise `SessionProcessor`-side: the
    // `generated_block` / `check_collation` orchestration drives them, not any
    // `CollationController` policy method, so the controller unit tests never
    // invoke them. Covered end-to-end by `test_session_processor`.
    fn broadcast_candidate(&self, _slot: u32, _candidate_hash: UInt256, _candidate: CandidateData) {
    }

    fn persist_candidate_info(
        &mut self,
        _slot: SlotIndex,
        _candidate_hash: UInt256,
        _self_idx: ValidatorIndex,
        _candidate_hash_data_bytes: Vec<u8>,
        _signature: Vec<u8>,
    ) {
    }

    fn request_parent_candidate(&self, _slot: SlotIndex, _hash: UInt256) {}

    fn self_receive_candidate(&self, _self_idx: u32, _candidate: CandidateData) {}
}

/*
    --------------------------------------------------------------------
    Construction / fresh-state invariants
    --------------------------------------------------------------------
*/

#[test]
fn new_starts_empty_and_all_cursors_unset() {
    let ctrl = new_ctrl();

    assert_eq!(ctrl.precollated_count(), 0);
    assert!(ctrl.precollated_is_empty());
    assert!(ctrl.precollated_max_slot().is_none());
    assert!(ctrl.earliest_collation_time().is_none());
    assert!(ctrl.local_chain_head().is_none());
    assert!(ctrl.last_generated_slot().is_none());
    assert!(ctrl.generated_parent_cache_is_empty());
}

/*
    --------------------------------------------------------------------
    precollated_blocks accessors
    --------------------------------------------------------------------
*/

#[test]
fn insert_and_remove_precollated_round_trip() {
    let mut ctrl = new_ctrl();
    let slot = SlotIndex::new(7);

    assert!(!ctrl.precollated_contains(slot));
    assert!(ctrl.insert_precollated(slot, make_precollated(0, None)).is_none());
    assert!(ctrl.precollated_contains(slot));
    assert_eq!(ctrl.precollated_count(), 1);
    assert!(ctrl.precollated(slot).is_some());

    let replaced = ctrl.insert_precollated(slot, make_precollated(1, None));
    assert!(replaced.is_some(), "second insert at the same slot must return the displaced entry");

    let removed = ctrl.remove_precollated(slot);
    assert!(removed.is_some());
    assert!(!ctrl.precollated_contains(slot));
    assert!(ctrl.precollated_is_empty());
}

#[test]
fn precollated_iter_and_keys_observe_pipeline() {
    let mut ctrl = new_ctrl();
    let slot_a = SlotIndex::new(1);
    let slot_b = SlotIndex::new(5);
    let slot_c = SlotIndex::new(9);
    ctrl.insert_precollated(slot_a, make_precollated(0, None));
    ctrl.insert_precollated(slot_b, make_precollated(1, None));
    ctrl.insert_precollated(slot_c, make_precollated(2, None));

    let mut keys: Vec<_> = ctrl.precollated_keys().collect();
    keys.sort_by_key(|s| s.value());
    assert_eq!(keys, vec![slot_a, slot_b, slot_c]);

    let mut slots: Vec<_> = ctrl.precollated_slots();
    slots.sort_by_key(|s| s.value());
    assert_eq!(slots, vec![slot_a, slot_b, slot_c]);

    let counted = ctrl.iter_precollated().count();
    assert_eq!(counted, 3);
}

#[test]
fn precollated_mut_allows_inline_update() {
    let mut ctrl = new_ctrl();
    let slot = SlotIndex::new(3);
    ctrl.insert_precollated(slot, make_precollated(0, None));

    let new_parent = make_parent_info(2, 0xCC);
    ctrl.precollated_mut(slot).expect("entry must exist").parent = Some(new_parent.clone());

    let got = ctrl.precollated(slot).unwrap().parent.as_ref().unwrap();
    assert_eq!(got.slot, new_parent.slot);
    assert_eq!(got.hash, new_parent.hash);
}

/*
    --------------------------------------------------------------------
    precollated_blocks_next_request_id
    --------------------------------------------------------------------
*/

#[test]
fn next_precollation_request_id_is_monotonic_post_increment() {
    let mut ctrl = new_ctrl();
    assert_eq!(ctrl.next_precollation_request_id(), 0);
    assert_eq!(ctrl.next_precollation_request_id(), 1);
    assert_eq!(ctrl.next_precollation_request_id(), 2);
}

/*
    --------------------------------------------------------------------
    precollated_blocks_max_slot
    --------------------------------------------------------------------
*/

#[test]
fn note_precollated_slot_only_advances_high_water_mark() {
    let mut ctrl = new_ctrl();
    assert!(ctrl.precollated_max_slot().is_none());

    ctrl.note_precollated_slot(SlotIndex::new(3));
    assert_eq!(ctrl.precollated_max_slot(), Some(SlotIndex::new(3)));

    ctrl.note_precollated_slot(SlotIndex::new(7));
    assert_eq!(ctrl.precollated_max_slot(), Some(SlotIndex::new(7)));

    // A lower slot must NOT regress the cursor.
    ctrl.note_precollated_slot(SlotIndex::new(2));
    assert_eq!(ctrl.precollated_max_slot(), Some(SlotIndex::new(7)));

    // Equal slot must NOT bump (`is_none_or(|max| slot > max)`).
    ctrl.note_precollated_slot(SlotIndex::new(7));
    assert_eq!(ctrl.precollated_max_slot(), Some(SlotIndex::new(7)));

    ctrl.reset_precollated_max_slot();
    assert!(ctrl.precollated_max_slot().is_none());
}

/*
    --------------------------------------------------------------------
    earliest_collation_time
    --------------------------------------------------------------------
*/

#[test]
fn earliest_collation_time_round_trip() {
    let mut ctrl = new_ctrl();
    assert!(ctrl.earliest_collation_time().is_none());

    let now = SystemTime::now();
    ctrl.set_earliest_collation_time(Some(now));
    assert_eq!(ctrl.earliest_collation_time(), Some(now));

    ctrl.set_earliest_collation_time(None);
    assert!(ctrl.earliest_collation_time().is_none());
}

/*
    --------------------------------------------------------------------
    local_chain_head
    --------------------------------------------------------------------
*/

#[test]
fn local_chain_head_round_trip() {
    let mut ctrl = new_ctrl();
    assert!(ctrl.local_chain_head().is_none());

    let head = make_local_chain_head(2, 5, make_parent_info(4, 0xAB));
    ctrl.set_local_chain_head(Some(head.clone()));
    let got = ctrl.local_chain_head().expect("head must be present");
    assert_eq!(got.window, head.window);
    assert_eq!(got.slot, head.slot);

    ctrl.set_local_chain_head(None);
    assert!(ctrl.local_chain_head().is_none());
}

/*
    --------------------------------------------------------------------
    last_generated_slot
    --------------------------------------------------------------------
*/

#[test]
fn last_generated_slot_round_trip() {
    let mut ctrl = new_ctrl();
    assert!(ctrl.last_generated_slot().is_none());

    ctrl.set_last_generated_slot(Some(SlotIndex::new(5)));
    assert_eq!(ctrl.last_generated_slot(), Some(SlotIndex::new(5)));

    ctrl.set_last_generated_slot(None);
    assert!(ctrl.last_generated_slot().is_none());
}

/*
    --------------------------------------------------------------------
    generated_parent_cache + generated_parent_gen_utime_ms_cache
    --------------------------------------------------------------------
*/

#[test]
fn generated_parent_cache_insert_lookup_clear() {
    let mut ctrl = new_ctrl();
    let id = make_candidate_id(7, 0xAA);
    let block_id = make_block_id(7, 0x10);

    assert!(ctrl.generated_parent_cache_is_empty());
    assert!(ctrl.resolve_generated_parent_block_id(&id).is_none());

    assert!(ctrl.insert_generated_parent(id.clone(), block_id.clone()).is_none());
    assert!(ctrl.insert_generated_parent_gen_utime_ms(id.clone(), 1_700_000_000_001).is_none());
    assert!(!ctrl.generated_parent_cache_is_empty());

    assert_eq!(ctrl.resolve_generated_parent_block_id(&id), Some(&block_id));
    assert_eq!(ctrl.find_generated_parent_id_by_block_id(&block_id), Some(id.clone()));

    ctrl.clear_generated_parent_caches();
    assert!(ctrl.generated_parent_cache_is_empty());
    assert!(ctrl.resolve_generated_parent_block_id(&id).is_none());
}

/*
    --------------------------------------------------------------------
    invalidate_local_chain_head
    --------------------------------------------------------------------
*/

#[test]
fn invalidate_local_chain_head_clears_head_and_caches() {
    let mut ctrl = new_ctrl();
    let parent = make_parent_info(2, 0xAB);
    let head = make_local_chain_head(0, 1, parent.clone());
    let candidate_id = make_candidate_id(1, 0xAB);
    let block_id = make_block_id(1, 0x10);

    ctrl.set_local_chain_head(Some(head));
    ctrl.insert_generated_parent(candidate_id.clone(), block_id);
    ctrl.insert_generated_parent_gen_utime_ms(candidate_id.clone(), 42);

    ctrl.invalidate_local_chain_head();

    assert!(ctrl.local_chain_head().is_none());
    assert!(ctrl.generated_parent_cache_is_empty());
    assert!(ctrl.resolve_generated_parent_block_id(&candidate_id).is_none());
}

/*
    --------------------------------------------------------------------
    cancel_pending_precollations
    --------------------------------------------------------------------
*/

#[test]
fn cancel_pending_precollations_drains_pipeline_and_cancels_requests() {
    let mut ctrl = new_ctrl();
    let slot_a = SlotIndex::new(1);
    let slot_b = SlotIndex::new(2);
    let block_a = make_precollated(0, None);
    let block_b = make_precollated(1, None);
    let req_a = block_a.request.clone();
    let req_b = block_b.request.clone();
    ctrl.insert_precollated(slot_a, block_a);
    ctrl.insert_precollated(slot_b, block_b);
    ctrl.note_precollated_slot(slot_b);

    assert!(!req_a.is_cancelled());
    assert!(!req_b.is_cancelled());

    let mut cancelled = ctrl.cancel_pending_precollations();
    cancelled.sort_by_key(|s| s.value());

    assert_eq!(cancelled, vec![slot_a, slot_b]);
    assert!(ctrl.precollated_is_empty());
    assert!(ctrl.precollated_max_slot().is_none());
    assert!(req_a.is_cancelled(), "in-flight requests must be cancelled on pipeline drain");
    assert!(req_b.is_cancelled());
}

/*
    --------------------------------------------------------------------
    remove_precollated_with_log
    --------------------------------------------------------------------
*/

#[test]
fn remove_precollated_with_log_returns_actually_removed_flag() {
    let mut ctrl = new_ctrl();
    let slot = SlotIndex::new(4);

    assert!(!ctrl.remove_precollated_with_log(slot));

    ctrl.insert_precollated(slot, make_precollated(0, None));
    assert!(ctrl.remove_precollated_with_log(slot));
    assert!(ctrl.precollated_is_empty());

    // Subsequent removal on the same slot must return false (idempotent).
    assert!(!ctrl.remove_precollated_with_log(slot));
}

/*
    --------------------------------------------------------------------
    resolve_parent_block_id
    --------------------------------------------------------------------
*/

#[test]
fn resolve_parent_block_id_prefers_cache_then_book_then_none() {
    let mut ctrl = new_ctrl();
    // Book entry: candidate id (slot=2, hash=BB) -> block_id(seqno=22, h=0x20).
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0))
        .with_received(make_candidate_id(2, 0xBB), make_received(2, make_block_id(22, 0x20), None));

    // Cache entry: candidate id (slot=1, hash=AA) -> block_id(seqno=11, h=0x10).
    let cache_id = make_candidate_id(1, 0xAA);
    let cache_block = make_block_id(11, 0x10);
    ctrl.insert_generated_parent(cache_id.clone(), cache_block.clone());

    // Cache hit takes precedence.
    let parent_cache = make_parent_info(1, 0xAA);
    assert_eq!(ctrl.resolve_parent_block_id(&parent_cache, &backend), Some(cache_block));

    // Cache miss: falls back to book.
    let parent_book = make_parent_info(2, 0xBB);
    assert_eq!(ctrl.resolve_parent_block_id(&parent_book, &backend), Some(make_block_id(22, 0x20)));

    // Miss in both: returns None.
    let parent_missing = make_parent_info(3, 0xCC);
    assert!(ctrl.resolve_parent_block_id(&parent_missing, &backend).is_none());
}

/*
    --------------------------------------------------------------------
    try_begin_collation_slot
    --------------------------------------------------------------------
*/

#[test]
fn try_begin_collation_slot_truth_table() {
    // 2 nodes, default slots_per_leader_window=1 => slot 0 -> leader 0 (local),
    // slot 1 -> leader 1 (not local).
    let description = Arc::new(make_mc_description(2));
    let mut ctrl = mk_ctrl(description.clone());

    let slot_leader = SlotIndex::new(0);
    let slot_not_leader = SlotIndex::new(1);

    // Leader case with no precollation pending.
    let local_idx = description.get_self_idx();
    assert_eq!(ctrl.try_begin_collation_slot(slot_leader), Some(local_idx));

    // Not-leader case: returns None.
    assert!(ctrl.try_begin_collation_slot(slot_not_leader).is_none());

    // Leader case but precollation already pending: returns None.
    ctrl.insert_precollated(slot_leader, make_precollated(0, None));
    assert!(ctrl.try_begin_collation_slot(slot_leader).is_none());
}

/// Controller bound to a masterchain description with a configurable multi-slot
/// leader window. With `node_count` 2 and window size 4, window 0 (slots 0..=3)
/// is led locally (index 0) and window 1 (slots 4..=7) is led by index 1.
fn plan_ctrl(node_count: u32, slots_per_leader_window: u32) -> CollationController {
    let opts = SessionOptions { slots_per_leader_window, ..SessionOptions::default() };
    mk_ctrl(Arc::new(make_description_with_opts(node_count, ShardIdent::masterchain(), opts)))
}

/*
    --------------------------------------------------------------------
    reset
    --------------------------------------------------------------------
*/

#[test]
fn reset_drains_pipeline_forgets_tracking_and_clears_head() {
    let mut ctrl = plan_ctrl(2, 4);

    // Seed two in-flight precollations + a window-local chain head.
    ctrl.insert_precollated(SlotIndex::new(0), make_precollated(0, None));
    ctrl.note_precollated_slot(SlotIndex::new(0));
    ctrl.insert_precollated(SlotIndex::new(1), make_precollated(1, None));
    ctrl.note_precollated_slot(SlotIndex::new(1));
    ctrl.set_local_chain_head(Some(make_local_chain_head(0, 0, make_parent_info(0, 0xAB))));
    assert_eq!(ctrl.precollated_count(), 2);

    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    ctrl.reset(&backend);

    // Pipeline drained + high-water cursor reset + chain head invalidated.
    assert!(ctrl.precollated_is_empty(), "reset drains the precollation pipeline");
    assert!(ctrl.precollated_max_slot().is_none(), "reset clears the high-water cursor");
    assert!(ctrl.local_chain_head().is_none(), "reset invalidates the chain head");

    // Each cancelled slot had its self-collation tracking forgotten with the
    // pipeline-reset reason.
    let mut forgotten = backend.forgotten();
    forgotten.sort_by_key(|(slot, _)| slot.value());
    assert_eq!(
        forgotten,
        vec![
            (SlotIndex::new(0), "precollation_pipeline_reset".to_string()),
            (SlotIndex::new(1), "precollation_pipeline_reset".to_string()),
        ],
    );
}

/*
    --------------------------------------------------------------------
    create_pending_collation_request
    --------------------------------------------------------------------
*/

#[test]
fn create_pending_collation_request_allocates_and_registers() {
    let mut ctrl = new_ctrl();
    let slot = SlotIndex::new(4);
    let parent = make_parent_info(3, 0xAB);
    let now = SystemTime::now();
    let soft = now + Duration::from_millis(1_000);
    let hard = now + Duration::from_millis(9_600);
    // The budget anchor is the dispatch instant, distinct from the creation time
    // (`now` here, the slot start): for shardchains it predates the slot start.
    let anchor = now - Duration::from_millis(500);

    let (req_id, request) = ctrl.create_pending_collation_request(
        slot,
        Some(parent.clone()),
        now,
        Some(soft),
        Some(hard),
        Some(anchor),
    );

    assert_eq!(req_id, 0, "first request must be id 0 (post-increment)");
    assert_eq!(request.get_request_id(), 0);
    assert!(!request.is_cancelled());
    assert_eq!(request.get_creation_time(), now);
    assert_eq!(request.get_collation_soft_deadline(), Some(soft), "soft deadline round-trips");
    assert_eq!(request.get_collation_hard_deadline(), Some(hard), "hard deadline round-trips");
    assert_eq!(
        request.get_collation_budget_anchor(),
        Some(anchor),
        "budget anchor round-trips and is distinct from the creation time"
    );
    assert!(ctrl.precollated_contains(slot));
    assert_eq!(ctrl.precollated_max_slot(), Some(slot));

    let stored = ctrl.precollated(slot).expect("entry must be registered");
    assert!(stored.result.is_none(), "fresh entry starts with no result");
    assert_eq!(stored.parent.as_ref().unwrap().slot, parent.slot);

    // Second request must get the next monotonic id.
    let (next_id, _) =
        ctrl.create_pending_collation_request(SlotIndex::new(5), None, now, None, None, None);
    assert_eq!(next_id, 1);
}

/*
    --------------------------------------------------------------------
    update_collation_pacing
    --------------------------------------------------------------------
*/

#[test]
fn update_collation_pacing_advances_earliest_time_by_target_rate() {
    let description = Arc::new(make_mc_description(2));
    let target_rate = description.opts().target_rate;
    // Freeze the shared session clock: pacing now self-sources time from the
    // held description rather than taking it as a parameter.
    let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
    description.set_time(now);
    let mut ctrl = mk_ctrl(description.clone());

    ctrl.update_collation_pacing();
    assert_eq!(ctrl.earliest_collation_time(), Some(now + target_rate));

    // Advancing the shared clock advances the next pacing deadline.
    let later = now + Duration::from_secs(5);
    description.set_time(later);
    ctrl.update_collation_pacing();
    assert_eq!(ctrl.earliest_collation_time(), Some(later + target_rate));
}

/*
    --------------------------------------------------------------------
    should_generate_empty_block
    --------------------------------------------------------------------
*/

#[test]
fn should_generate_empty_block_when_parent_before_split() {
    let ctrl = mk_ctrl(Arc::new(make_mc_description(2)));

    // parent_before_split=true short-circuits both branches.
    assert!(ctrl.should_generate_empty_block(
        SlotIndex::new(3),
        100,
        Some(true),
        false,
        Some(50),
        None,
    ));

    // parent_before_split=None + finalized_head_before_split=true also true.
    assert!(ctrl.should_generate_empty_block(SlotIndex::new(3), 100, None, true, Some(50), None,));
}

#[test]
fn should_generate_empty_block_masterchain_finalization_lag() {
    let ctrl = mk_ctrl(Arc::new(make_mc_description(2)));
    let slot = SlotIndex::new(3);

    // finalized=8, new_seqno=10 -> 8+1<10 -> true
    assert!(ctrl.should_generate_empty_block(slot, 10, Some(false), false, Some(8), None,));

    // finalized=8, new_seqno=9 -> 8+1<9? no -> false (tight, no lag yet)
    assert!(!ctrl.should_generate_empty_block(slot, 9, Some(false), false, Some(8), None,));

    // No finalized tracking -> false (must not trigger empty block).
    assert!(!ctrl.should_generate_empty_block(slot, 10, Some(false), false, None, None,));
}

#[test]
fn should_generate_empty_block_shardchain_uses_mc_lag_threshold() {
    let ctrl = mk_ctrl(Arc::new(make_shard_description(2, Some(4))));
    let slot = SlotIndex::new(3);

    // mc_finalized=10, threshold=4, new_seqno=15 -> 10+4<15 -> true
    assert!(ctrl.should_generate_empty_block(slot, 15, Some(false), false, None, Some(10),));

    // mc_finalized=10, threshold=4, new_seqno=14 -> 10+4<14? no -> false
    assert!(!ctrl.should_generate_empty_block(slot, 14, Some(false), false, None, Some(10),));

    // Shardchain but no threshold configured -> false.
    let ctrl_no_threshold = mk_ctrl(Arc::new(make_shard_description(2, None)));
    assert!(!ctrl_no_threshold.should_generate_empty_block(
        slot,
        100,
        Some(false),
        false,
        None,
        Some(0),
    ));
}

/*
    --------------------------------------------------------------------
    resolve_parent_gen_utime_ms
    --------------------------------------------------------------------
*/

#[test]
fn resolve_parent_gen_utime_ms_prefers_cache_then_book_then_chain_head() {
    let mut ctrl = new_ctrl();
    let parent_cache = make_parent_info(1, 0xAA);
    let parent_book = make_parent_info(2, 0xBB);
    let parent_head = make_parent_info(3, 0xCC);
    let parent_missing = make_parent_info(4, 0xDD);

    // Cache layer.
    ctrl.insert_generated_parent_gen_utime_ms(
        RawCandidateId { slot: parent_cache.slot, hash: parent_cache.hash.clone() },
        111,
    );

    // Book layer.
    let book_block = make_block_id(22, 0x20);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0)).with_received(
        RawCandidateId { slot: parent_book.slot, hash: parent_book.hash.clone() },
        make_received(2, book_block.clone(), Some(222)),
    );

    // Local-chain-head layer.
    ctrl.set_local_chain_head(Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: SlotIndex::new(3),
        parent_info: parent_head.clone(),
        gen_utime_ms: Some(333),
    }));

    assert_eq!(ctrl.resolve_parent_gen_utime_ms(&parent_cache, &backend), Some(111));
    assert_eq!(ctrl.resolve_parent_gen_utime_ms(&parent_book, &backend), Some(222));
    assert_eq!(ctrl.resolve_parent_gen_utime_ms(&parent_head, &backend), Some(333));
    assert!(ctrl.resolve_parent_gen_utime_ms(&parent_missing, &backend).is_none());
}

/*
    --------------------------------------------------------------------
    prepare_collation / compute_collation_timing
    --------------------------------------------------------------------
*/

/// Single-node controller bound to a description with an explicit `target_rate`
/// and a fixed clock, so the `prepare_collation` / `compute_collation_timing`
/// timing branches are deterministic regardless of wall-clock or option
/// defaults.
fn ctrl_with_clock(
    shard: ShardIdent,
    target_rate: Duration,
    now: SystemTime,
) -> CollationController {
    let opts = SessionOptions { target_rate, ..SessionOptions::default() };
    let desc = Arc::new(make_description_with_opts(1, shard, opts));
    desc.set_time(now);
    mk_ctrl(desc)
}

/// A named parent whose `BlockIdExt` cannot be resolved (empty cache + empty
/// book) blocks the attempt: the caller must request the parent and retry.
#[test]
fn prepare_collation_waits_for_unresolved_named_parent() {
    let ctrl = ctrl_with_clock(
        ShardIdent::masterchain(),
        Duration::from_millis(500),
        UNIX_EPOCH + Duration::from_millis(1_700_000_000_000),
    );
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    let parent = make_parent_info(2, 0x10);
    let session_start = vec![make_block_id(5, 0x01)];

    assert!(matches!(
        ctrl.prepare_collation(Some(&parent), &backend, &session_start),
        CollationPreparation::WaitingForParent
    ));
}

/// Genesis (no parent): the prev-block list is the session-start blocks, the
/// attempt is flagged first-of-session, the seqno is `max(prev seqno) + 1`, and
/// with no parent generation time the attempt is immediately `Ready`.
#[test]
fn prepare_collation_genesis_uses_session_start_prev_blocks() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    let ctrl = ctrl_with_clock(ShardIdent::masterchain(), Duration::from_millis(500), now);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    let session_start = vec![make_block_id(7, 0x01), make_block_id(9, 0x02)];

    let CollationPreparation::Ready(prepared) =
        ctrl.prepare_collation(None, &backend, &session_start)
    else {
        panic!("genesis attempt with no parent gen-time must be Ready");
    };
    assert!(prepared.is_first_session_block);
    assert_eq!(prepared.prev_block_ids, session_start);
    assert_eq!(prepared.new_seqno, 10, "new_seqno = max(prev seqno=9) + 1");
    assert_eq!(prepared.timing.parent_gen_utime_ms, None);
    assert_eq!(prepared.timing.dispatch_time, now, "no parent gen-time => dispatch == now");
}

/// A resolved named parent yields a single-element prev-block list, a non-first
/// flag, and a seqno derived from the parent block id (`parent seqno + 1`). An
/// old parent generation time leaves `min_gen_time` clamped to `now`, so the
/// attempt is `Ready`.
#[test]
fn prepare_collation_ready_with_resolved_parent_derives_seqno() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    let ctrl = ctrl_with_clock(ShardIdent::masterchain(), Duration::from_millis(500), now);

    let parent = make_parent_info(3, 0x20);
    let parent_block = make_block_id(41, 0x20);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0))
        .with_received(make_candidate_id(3, 0x20), make_received(3, parent_block.clone(), Some(0)));
    let session_start = vec![make_block_id(1, 0x01)];

    let CollationPreparation::Ready(prepared) =
        ctrl.prepare_collation(Some(&parent), &backend, &session_start)
    else {
        panic!("resolved parent with old gen-time must be Ready");
    };
    assert!(!prepared.is_first_session_block);
    assert_eq!(prepared.prev_block_ids, vec![parent_block]);
    assert_eq!(prepared.new_seqno, 42, "new_seqno = parent seqno(41) + 1");
    assert_eq!(prepared.timing.parent_gen_utime_ms, Some(0));
    assert_eq!(prepared.timing.dispatch_time, now);
}

/// On the masterchain (`start_collate_before == 0`), a recent parent generation
/// time pushes `min_gen_time` (and therefore `dispatch_time`) to
/// `now + target_rate`, so the attempt is `Deferred` until then.
#[test]
fn prepare_collation_defers_on_mc_when_parent_gen_time_is_recent() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    let target_rate = Duration::from_millis(500);
    let ctrl = ctrl_with_clock(ShardIdent::masterchain(), target_rate, now);

    let parent = make_parent_info(3, 0x30);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0)).with_received(
        make_candidate_id(3, 0x30),
        make_received(3, make_block_id(50, 0x30), Some(1_700_000_000_000)),
    );
    let session_start = vec![make_block_id(1, 0x01)];

    let CollationPreparation::Deferred(dispatch_at) =
        ctrl.prepare_collation(Some(&parent), &backend, &session_start)
    else {
        panic!("recent parent gen-time on MC must defer until dispatch time");
    };
    assert_eq!(dispatch_at, now + target_rate);
}

/// Shardchains start collating `target_rate` early
/// (`dispatch_time = min_gen_time - target_rate`): for the same recent-parent
/// input that defers on the masterchain, a shard is `Ready` immediately with
/// `dispatch_time == now` while `min_gen_time` stays in the future.
#[test]
fn prepare_collation_shard_starts_collation_target_rate_early() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    let target_rate = Duration::from_millis(500);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let ctrl = ctrl_with_clock(shard, target_rate, now);

    let parent = make_parent_info(3, 0x40);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0)).with_received(
        make_candidate_id(3, 0x40),
        make_received(3, make_block_id(50, 0x40), Some(1_700_000_000_000)),
    );
    let session_start = vec![make_block_id(1, 0x01)];

    let CollationPreparation::Ready(prepared) =
        ctrl.prepare_collation(Some(&parent), &backend, &session_start)
    else {
        panic!("shard collation starts target_rate early, so it must be Ready");
    };
    assert_eq!(prepared.timing.dispatch_time, now, "shard dispatches target_rate early");
    assert_eq!(prepared.timing.min_gen_time, now + target_rate);
    assert_eq!(prepared.timing.start_collate_before, target_rate);
}

/*
    --------------------------------------------------------------------
    Debug smoke
    --------------------------------------------------------------------
*/

#[test]
fn debug_impl_includes_field_counts() {
    let mut ctrl = new_ctrl();
    ctrl.insert_precollated(SlotIndex::new(1), make_precollated(0, None));
    ctrl.set_last_generated_slot(Some(SlotIndex::new(3)));
    ctrl.insert_generated_parent(make_candidate_id(4, 0xCC), make_block_id(44, 0x44));

    let dbg = format!("{:?}", ctrl);
    assert!(dbg.contains("CollationController"), "Debug must include struct name: {dbg}");
    assert!(dbg.contains("precollated_count: 1"), "Debug must expose precollated_count: {dbg}");
    assert!(dbg.contains("last_generated_slot"), "Debug must expose last_generated_slot: {dbg}");
    assert!(
        dbg.contains("generated_parent_cache: 1"),
        "Debug must expose generated_parent_cache size: {dbg}",
    );
}

/*
    --------------------------------------------------------------------
    collation_deadlines: absolute per-slot soft cutoff + window-end hard cap
    --------------------------------------------------------------------
*/

/// Masterchain soft cutoff is `slot_start + target_rate` (C++ block-producer.cpp);
/// the window-end hard cap for the first slot of a window spans the whole window.
#[test]
fn collation_deadlines_masterchain_soft_is_slot_plus_rate() {
    let start = UNIX_EPOCH + Duration::from_secs(1_000);
    let rate = Duration::from_millis(2_400);
    let (soft, hard) = collation_deadlines(SlotIndex::new(0), start, rate, 4, true);
    assert_eq!(soft, start + rate);
    assert_eq!(hard, start + rate * 4, "offset 0 => the whole window remains");
}

/// Shardchain soft cutoff is `slot_start` itself (C++ block-producer.cpp): shard
/// collation dispatches `target_rate` early, so message intake closes at the slot start.
#[test]
fn collation_deadlines_shardchain_soft_is_slot_start() {
    let start = UNIX_EPOCH + Duration::from_secs(1_000);
    let rate = Duration::from_millis(2_400);
    let (soft, _hard) = collation_deadlines(SlotIndex::new(0), start, rate, 4, false);
    assert_eq!(soft, start);
}

/// The hard cap shrinks toward the window end as the slot advances within its leader
/// window: `slots_to_window_end = slots_per_leader_window - offset_in_window`.
#[test]
fn collation_deadlines_hard_cap_shrinks_within_window() {
    let start = UNIX_EPOCH + Duration::from_secs(1_000);
    let rate = Duration::from_millis(2_400);
    // Last slot of a 4-slot window (offset 3): one rate remains to the window end.
    let (_soft, hard_last) = collation_deadlines(SlotIndex::new(3), start, rate, 4, true);
    assert_eq!(hard_last, start + rate);
    // Slot 6 (offset 2 of window [4, 8)): two rates remain.
    let (_s, hard_mid) = collation_deadlines(SlotIndex::new(6), start, rate, 4, true);
    assert_eq!(hard_mid, start + rate * 2);
}

/// `slots_per_leader_window == 0` is clamped to 1 (never divide by zero); a single-slot
/// window always has exactly one rate to its end.
#[test]
fn collation_deadlines_handles_zero_window() {
    let start = UNIX_EPOCH + Duration::from_secs(1_000);
    let rate = Duration::from_millis(500);
    let (_soft, hard) = collation_deadlines(SlotIndex::new(7), start, rate, 0, true);
    assert_eq!(hard, start + rate);
}

/*
    --------------------------------------------------------------------
    block_generation_active: single in-flight collation bookkeeping
    --------------------------------------------------------------------
*/

/// A placeholder deadline context for tests that only exercise the in-flight marker
/// bookkeeping (window / slot / request id / per-slot horizon) and do not assert on the
/// pinned soft/hard cutoffs or budget anchor.
fn fake_deadline_context() -> CollationDeadlineContext {
    CollationDeadlineContext {
        soft_deadline: UNIX_EPOCH + Duration::from_secs(100),
        hard_deadline: UNIX_EPOCH + Duration::from_secs(120),
        budget_anchor: UNIX_EPOCH + Duration::from_secs(98),
    }
}

fn fake_real_collation_state(request_id: u32) -> RealCollationState {
    RealCollationState {
        window: WindowIndex::new(0),
        slot_dispatched: SlotIndex::new(2),
        request_id,
        next_slot_deadline: UNIX_EPOCH + Duration::from_secs(100),
        deadlines: fake_deadline_context(),
    }
}

/// `clear_block_generation_if` only releases the marker for the in-flight collation's
/// own request id: an empty filler / chained precollation (a different request) must
/// leave a still-running real collation untouched (C++ block-producer.cpp).
#[test]
fn clear_block_generation_only_matches_request_id() {
    let mut ctrl = new_ctrl();
    ctrl.block_generation_active = Some(fake_real_collation_state(7));

    // A different request (e.g. an empty filler) must NOT clear it.
    ctrl.clear_block_generation_if(99);
    assert!(ctrl.block_generation_active.is_some(), "non-matching request must not clear");

    // The real collation's own request clears it.
    ctrl.clear_block_generation_if(7);
    assert!(ctrl.block_generation_active.is_none(), "matching request clears the marker");
}

/// `reset()` drops the in-flight marker unconditionally: it cancels the pending
/// AsyncRequests, so no completion would otherwise arrive to clear it.
#[test]
fn reset_clears_block_generation_active() {
    let mut ctrl = new_ctrl();
    ctrl.block_generation_active = Some(fake_real_collation_state(3));

    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    ctrl.reset(&backend);

    assert!(ctrl.block_generation_active.is_none());
}

/*
    --------------------------------------------------------------------
    Single in-flight guard + per-slot deadline wake + late re-tag
    --------------------------------------------------------------------
*/

/// Build an in-flight `RealCollationState` with a future per-slot horizon so the
/// wake's `next_deadline <= now` clamp does not fire (the deadline math under test
/// stays the simple `deadline + target_rate`).
fn in_flight_state(
    request_id: u32,
    window: u32,
    slot: u32,
    next_slot_deadline: SystemTime,
) -> RealCollationState {
    RealCollationState {
        window: WindowIndex::new(window),
        slot_dispatched: SlotIndex::new(slot),
        request_id,
        next_slot_deadline,
        deadlines: fake_deadline_context(),
    }
}

/// The single in-flight guard (C++ block-producer.cpp): `execute_collation_attempt`
/// must decline a second REAL collation while one is in flight, leaving the in-flight
/// marker untouched and creating no new precollation entry.
#[test]
fn single_in_flight_guard_declines_second_real_dispatch() {
    let mut ctrl = new_ctrl();
    ctrl.block_generation_active = Some(fake_real_collation_state(5));

    let mut backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    let slot = SlotIndex::new(0);
    ctrl.execute_collation_attempt(
        &mut backend,
        slot,
        None,
        ValidatorIndex::new(0),
        true,
        false,
        None,
    );

    assert!(
        !ctrl.precollated_contains(slot),
        "guard must not dispatch a second real collation while one is in flight"
    );
    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.request_id),
        Some(5),
        "the in-flight real collation must remain tracked"
    );
}

/// Per-slot deadline wake when no empty filler can be placed (no parent: the
/// first block in an epoch cannot be empty): keep the real alive, advance the
/// horizon by `target_rate`, and re-arm the SAME loop slot (C++ `--slot;
/// continue;` with `slot_start = max(slot_start, now())`). No candidate is
/// published.
#[test]
fn slot_deadline_wake_without_parent_keeps_waiting_same_slot() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let target_rate = description.opts().target_rate;
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 11;
    // Real dispatched for slot 1 (window 0, spw=4). No precollation entry and no
    // local chain head => emit_slot_filler finds no parent.
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 1, deadline));
    // Recent finalization => the allow_empty gate permits an empty, so the decline
    // under test is the first-block (no-parent) check inside emit_slot_filler.
    ctrl.last_consensus_finalized_at = Some(now);

    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0));
    ctrl.on_slot_deadline(&mut backend, SlotIndex::new(1), request_id, deadline);

    let expected = deadline + target_rate;
    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.next_slot_deadline),
        Some(expected),
        "wake must advance the per-slot horizon by target_rate"
    );
    assert_eq!(
        queue.delayed().as_slice(),
        &[expected],
        "wake must re-arm exactly once, at the advanced deadline"
    );
    assert!(
        ctrl.block_generation_active.is_some(),
        "the real collation must stay in flight (keep-alive, not cancelled)"
    );
    assert!(
        ctrl.local_chain_head().is_none(),
        "no filler may be published when no parent is available (first block cannot be empty)"
    );
}

/// Per-slot deadline wake that DOES publish an empty filler: the elapsed slot is
/// filled (chained onto the real's locked parent), the real is kept alive, its
/// precollation entry is preserved for the eventual re-tag, the horizon advances,
/// and the wake re-arms (C++ await-timeout branch publishing an empty and
/// continuing to await the same `block_generation`).
#[test]
fn slot_deadline_wake_publishes_filler_keeps_real_and_precollation() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let target_rate = description.opts().target_rate;
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 31;
    let dispatch_slot = SlotIndex::new(1);
    // The real's locked parent (a preceding candidate that resolves via the book).
    let parent = make_parent_info(0, 0xAA);
    let parent_block_id = make_block_id(42, 0xAA);
    ctrl.insert_precollated(dispatch_slot, make_precollated(request_id, Some(parent.clone())));
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 1, deadline));
    // Recent finalization => the allow_empty gate permits the filler; the
    // no_empty_blocks_on_error_timeout suppression path is covered by its own test
    // below.
    ctrl.last_consensus_finalized_at = Some(now);

    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0))
        .with_received(make_candidate_id(0, 0xAA), make_received(0, parent_block_id, None));
    ctrl.on_slot_deadline(&mut backend, dispatch_slot, request_id, deadline);

    let head = ctrl.local_chain_head().expect("filler must set the local chain head");
    assert_eq!(head.slot, dispatch_slot, "filler published at the elapsed loop slot");
    assert!(
        ctrl.block_generation_active.is_some(),
        "the real collation must stay in flight across the filler (keep-alive)"
    );
    assert!(
        ctrl.precollated(dispatch_slot).is_some(),
        "filler must NOT consume the real's precollation entry (needed for the re-tag)"
    );
    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.next_slot_deadline),
        Some(deadline + target_rate),
        "wake must advance the per-slot horizon by target_rate"
    );
    assert_eq!(
        queue.delayed().as_slice(),
        &[deadline + target_rate],
        "wake must re-arm exactly once for the next loop slot"
    );
}

/// A wake whose loop slot has run past the leader window (C++ `slot < end_slot`)
/// must not publish, advance the horizon, nor re-arm: filling stops at the window
/// boundary, leaving the still-in-flight real to re-tag on completion.
#[test]
fn slot_deadline_wake_past_window_end_is_noop() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 21;
    // Active real in window 0; the wake fires for slot 4, which is window 1.
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 0, deadline));

    let mut backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    ctrl.on_slot_deadline(&mut backend, SlotIndex::new(4), request_id, deadline);

    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.next_slot_deadline),
        Some(deadline),
        "a past-window wake must not advance the horizon"
    );
    assert!(queue.delayed().is_empty(), "a past-window wake must not re-arm");
    assert!(ctrl.local_chain_head().is_none(), "a past-window wake must not publish a filler");
}

/// A wake carrying a superseded request id (the in-flight real has since changed)
/// must not mutate the horizon nor re-arm.
#[test]
fn slot_deadline_wake_stale_request_id_is_noop() {
    let description = Arc::new(make_mc_description(1));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    ctrl.block_generation_active = Some(in_flight_state(5, 0, 2, deadline));

    let mut backend = FakeCollationBackend::new(SlotIndex::new(2), WindowIndex::new(0));
    ctrl.on_slot_deadline(&mut backend, SlotIndex::new(2), 999, deadline);

    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.next_slot_deadline),
        Some(deadline),
        "a stale-request-id wake must not advance the horizon"
    );
    assert!(queue.delayed().is_empty(), "a stale-request-id wake must not re-arm");
}

/// A wake that fires after the leader window advanced clears the now-stale in-flight
/// marker so the single-in-flight guard cannot wedge the new window: the marker is
/// dropped, its pending request cancelled, its precollation entry removed, and the
/// self-collation tracking forgotten. No filler is published and no wake re-arms.
#[test]
fn slot_deadline_wake_window_change_clears_stale_marker() {
    let description = Arc::new(make_mc_description(1));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 8;
    let stale_slot = SlotIndex::new(2);
    // The stale real still holds its dispatch-slot precollation entry; clearing must
    // cancel its request so a late callback is a no-op for the superseded window.
    let precollated = make_precollated(request_id, None);
    let req = precollated.request.clone();
    ctrl.insert_precollated(stale_slot, precollated);
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 2, deadline));

    // Backend reports a newer leader window than the one the collation belongs to.
    let mut backend = FakeCollationBackend::new(stale_slot, WindowIndex::new(1));
    ctrl.on_slot_deadline(&mut backend, stale_slot, request_id, deadline);

    assert!(
        ctrl.block_generation_active.is_none(),
        "a window-change wake must drop the stale in-flight marker"
    );
    assert!(req.is_cancelled(), "the stale collation's pending request must be cancelled");
    assert!(!ctrl.precollated_contains(stale_slot), "the stale precollation entry must be removed");
    assert_eq!(
        backend.forgotten(),
        vec![(stale_slot, "stale_window_block_generation_cleared".to_string())],
        "clearing must forget the stale slot's self-collation tracking"
    );
    assert!(queue.delayed().is_empty(), "a window-change wake must not re-arm");
}

/// Direct coverage of [`CollationController::clear_stale_block_generation`]: a marker
/// whose window matches the current leader window is preserved (returns `false`); a
/// mismatched window is cleared (returns `true`) with its request cancelled.
#[test]
fn clear_stale_block_generation_only_fires_on_window_mismatch() {
    let mut ctrl = new_ctrl();
    let slot = SlotIndex::new(2);
    let request_id = 12;
    let precollated = make_precollated(request_id, None);
    let req = precollated.request.clone();
    ctrl.insert_precollated(slot, precollated);
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 2, UNIX_EPOCH));

    // Same window => nothing to clear.
    let same_window = FakeCollationBackend::new(slot, WindowIndex::new(0));
    assert!(!ctrl.clear_stale_block_generation(&same_window), "matching window must not clear");
    assert!(ctrl.block_generation_active.is_some(), "matching window leaves the marker intact");
    assert!(!req.is_cancelled(), "matching window must not cancel the request");

    // Window moved on => clear and cancel.
    let newer_window = FakeCollationBackend::new(slot, WindowIndex::new(1));
    assert!(ctrl.clear_stale_block_generation(&newer_window), "stale window must clear");
    assert!(ctrl.block_generation_active.is_none(), "stale window drops the marker");
    assert!(req.is_cancelled(), "stale window cancels the pending request");
}

/// Late re-tag (C++ block-producer.cpp): when the tracked real collation
/// completes after the per-slot wake already filled past its dispatch slot, it is
/// published at the current chain position (`local_chain_head.slot + 1`)
/// re-parented to the last filler - NOT at its original dispatch slot, which an
/// empty filler already occupies (republishing there would equivocate). The stale
/// dispatch-slot precollation entry is consumed and the in-flight marker cleared.
///
/// Uses an `Empty` result for fixture simplicity (the real-`Block` publish needs
/// valid BOC candidate data and is integration-covered by `test_session_processor`);
/// the re-tag decision, target slot, re-parent, precollation consume, and marker
/// clear are identical for either result variant.
#[test]
fn on_collation_complete_retags_late_real_onto_filler() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let (mut ctrl, _queue) = ctrl_with_recording_queue(description);

    let request_id = 41;
    let dispatch_slot = SlotIndex::new(1);
    // A filler the per-slot wake already published at slot 2 (window 0): the chain
    // head is ahead of the real's dispatch slot.
    let head_parent = make_parent_info(2, 0xBB);
    let head_block_id = make_block_id(42, 0xBB);
    ctrl.set_local_chain_head(Some(make_local_chain_head(0, 2, head_parent)));
    // The real is tracked and still holds its (now stale) dispatch-slot precollation
    // entry, locked onto its original parent.
    ctrl.insert_precollated(
        dispatch_slot,
        make_precollated(request_id, Some(make_parent_info(0, 0x11))),
    );
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 1, now + Duration::from_secs(100)));

    // first_non_progressed has advanced past the real's dispatch slot (the fillers
    // generated slots 1..=2), but the leader window is still 0.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(3), WindowIndex::new(0));
    ctrl.on_collation_complete(
        &mut backend,
        dispatch_slot,
        request_id,
        CollationResult::Empty { parent_block_id: head_block_id },
    );

    let new_head = ctrl.local_chain_head().expect("re-tag must publish a candidate");
    assert_eq!(
        new_head.slot,
        SlotIndex::new(3),
        "late real must be re-tagged to local_chain_head.slot + 1, not its dispatch slot"
    );
    assert!(
        ctrl.precollated(dispatch_slot).is_none(),
        "re-tag must consume the dispatch-slot precollation entry"
    );
    assert!(
        ctrl.block_generation_active.is_none(),
        "the real completing must clear the in-flight marker"
    );
}

/*
    --------------------------------------------------------------------
    allow_empty finalization-staleness gate
    --------------------------------------------------------------------
*/

/// The `allow_empty` finalization-staleness gate (C++ block-producer.cpp):
/// [`CollationController::empties_allowed_by_finalization`] is true only while the
/// last finalization plus `no_empty_blocks_on_error_timeout` has not elapsed.
/// `None` (nothing finalized yet) suppresses empties, matching C++'s
/// default-constructed in-the-past `last_consensus_finalized_at_`.
#[test]
fn empties_allowed_by_finalization_gates_on_recent_finalization() {
    let description = Arc::new(make_mc_description(1));
    let now = description.get_time();
    let timeout = description.opts().no_empty_blocks_on_error_timeout;
    let (mut ctrl, _queue) = ctrl_with_recording_queue(description);

    // Nothing finalized yet => suppress (C++ default in-the-past timestamp).
    ctrl.last_consensus_finalized_at = None;
    assert!(!ctrl.empties_allowed_by_finalization(), "no finalization => suppress empties");

    // Just finalized => allow.
    ctrl.last_consensus_finalized_at = Some(now);
    assert!(ctrl.empties_allowed_by_finalization(), "recent finalization => allow empties");

    // Finalized within the window (deadline still in the future) => allow.
    ctrl.last_consensus_finalized_at = Some(now - timeout + Duration::from_secs(1));
    assert!(
        ctrl.empties_allowed_by_finalization(),
        "finalization within no_empty_blocks_on_error_timeout => allow empties"
    );

    // No finalization for longer than the timeout => suppress.
    ctrl.last_consensus_finalized_at = Some(now - timeout - Duration::from_secs(1));
    assert!(
        !ctrl.empties_allowed_by_finalization(),
        "stalled finalization beyond no_empty_blocks_on_error_timeout => suppress empties"
    );
}

/// allow_empty gate suppression on the wake (C++ timeout branch with
/// `!allow_empty`): even with a placeable parent, a wake whose last
/// finalization is older than `no_empty_blocks_on_error_timeout` publishes NO empty.
/// It keeps the real alive, preserves its precollation entry, advances the per-slot
/// horizon, and re-arms on the SAME loop slot.
#[test]
fn slot_deadline_wake_suppressed_when_finalization_stale() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let target_rate = description.opts().target_rate;
    let timeout = description.opts().no_empty_blocks_on_error_timeout;
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 51;
    let dispatch_slot = SlotIndex::new(1);
    // A placeable parent: the filler WOULD succeed if the gate allowed it.
    let parent = make_parent_info(0, 0xAA);
    let parent_block_id = make_block_id(42, 0xAA);
    ctrl.insert_precollated(dispatch_slot, make_precollated(request_id, Some(parent.clone())));
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 1, deadline));
    // Finalization stalled beyond no_empty_blocks_on_error_timeout => suppress.
    ctrl.last_consensus_finalized_at = Some(now - timeout - Duration::from_secs(1));

    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0))
        .with_received(make_candidate_id(0, 0xAA), make_received(0, parent_block_id, None));
    ctrl.on_slot_deadline(&mut backend, dispatch_slot, request_id, deadline);

    assert!(
        ctrl.local_chain_head().is_none(),
        "stalled finalization must suppress the empty filler (no publish)"
    );
    assert!(
        ctrl.precollated(dispatch_slot).is_some(),
        "suppression must leave the real's precollation entry intact"
    );
    assert!(
        ctrl.block_generation_active.is_some(),
        "the real collation must stay in flight while empties are suppressed"
    );
    assert_eq!(
        ctrl.block_generation_active.as_ref().map(|s| s.next_slot_deadline),
        Some(deadline + target_rate),
        "the per-slot horizon must still advance even when the empty is suppressed"
    );
    assert_eq!(
        queue.delayed().as_slice(),
        &[deadline + target_rate],
        "a suppressed wake must re-arm exactly once (keep waiting on the same slot)"
    );
}

/// The wake refreshes `last_consensus_finalized_at` from the backend BEFORE the gate
/// (C++ updates it via the independent FinalizeBlock handler):
/// a stale tracker is reseeded by a freshly-advanced finalized seqno, so the gate
/// then permits the filler rather than reading a value frozen at the last
/// `check_collation`.
#[test]
fn slot_deadline_wake_refreshes_finalization_and_allows_filler() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let timeout = description.opts().no_empty_blocks_on_error_timeout;
    let (mut ctrl, _queue) = ctrl_with_recording_queue(description);

    let deadline = now + Duration::from_secs(100);
    let request_id = 61;
    let dispatch_slot = SlotIndex::new(1);
    let parent = make_parent_info(0, 0xAA);
    let parent_block_id = make_block_id(42, 0xAA);
    ctrl.insert_precollated(dispatch_slot, make_precollated(request_id, Some(parent.clone())));
    ctrl.block_generation_active = Some(in_flight_state(request_id, 0, 1, deadline));
    // A stale tracker that, on its own, would suppress the filler...
    ctrl.last_consensus_finalized_at = Some(now - timeout - Duration::from_secs(1));

    // ...but the backend reports a freshly-advanced finalized seqno, which the wake
    // folds in (refresh_finalization_timestamp) before evaluating the gate.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0))
        .with_received(make_candidate_id(0, 0xAA), make_received(0, parent_block_id, None));
    backend.last_consensus_finalized_seqno = Some(7);
    ctrl.on_slot_deadline(&mut backend, dispatch_slot, request_id, deadline);

    assert!(
        ctrl.local_chain_head().is_some(),
        "an advanced finalized seqno must reseed last_consensus_finalized_at and allow the filler"
    );
    assert_eq!(
        ctrl.last_observed_finalized_seqno,
        Some(7),
        "the wake must record the freshly-observed finalized seqno"
    );
}

/*
    --------------------------------------------------------------------
    Genuine-error single restart bounded by the window budget
    --------------------------------------------------------------------
*/

/// A genuine collation error for a still-current slot releases the
/// single-in-flight marker and schedules exactly ONE restart at
/// `now + COLLATION_ERROR_RESTART_BACKOFF` (the C++ genuine-error retry). There
/// is no attempt counter and no fresh-budget storm: the restart reuses the
/// absolute window-end deadline.
#[test]
fn genuine_error_while_slot_current_schedules_single_restart() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let fixed = UNIX_EPOCH + Duration::from_secs(1_000_000);
    description.set_time(fixed);
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let request_id = 71;
    let slot = SlotIndex::new(0);
    ctrl.insert_precollated(slot, make_precollated(request_id, None));
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 0, fixed + Duration::from_secs(100)));

    // Slot 0 is still current (first_non_progressed == 0): the failure is not terminal.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0));
    ctrl.on_collation_failed_impl(
        &mut backend,
        slot,
        request_id,
        ton_block::error!("collation error"),
    );

    assert!(
        ctrl.block_generation_active.is_none(),
        "the failed real must release the single-in-flight marker"
    );
    assert!(
        !ctrl.precollated_contains(slot),
        "the failed precollation entry must be dropped (the restart re-registers it)"
    );
    assert_eq!(
        queue.delayed().as_slice(),
        &[fixed + COLLATION_ERROR_RESTART_BACKOFF],
        "a genuine error schedules exactly ONE restart at the fixed backoff - no attempt-count \
         storm and no fresh window (C++ block-producer.cpp)"
    );
}

/// A genuine error for a slot that consensus has already progressed past is
/// terminal: the marker is released, the precollation entry dropped, and NO
/// restart is scheduled (the slot is gone / the budget is spent).
#[test]
fn genuine_error_after_slot_passed_drops_without_restart() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let request_id = 72;
    let slot = SlotIndex::new(0);
    ctrl.insert_precollated(slot, make_precollated(request_id, None));
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 0, SystemTime::now() + Duration::from_secs(100)));

    // first_non_progressed == 1 => slot 0 already passed.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0));
    ctrl.on_collation_failed_impl(
        &mut backend,
        slot,
        request_id,
        ton_block::error!("collation error"),
    );

    assert!(ctrl.block_generation_active.is_none(), "the failed real must release the marker");
    assert!(!ctrl.precollated_contains(slot), "a passed slot's precollation entry must be dropped");
    assert!(
        queue.delayed().is_empty(),
        "a slot that already progressed past must NOT be restarted"
    );
}

/*
    --------------------------------------------------------------------
    Genuine-error allow_empty recovery (C++ block-producer.cpp)
    --------------------------------------------------------------------
*/

/// Genuine error with `allow_empty` satisfied (consensus finalized recently AND a
/// parent is available): recover by publishing ONE empty block for the failed slot
/// and advancing the pipeline rather than restarting (C++ block-producer.cpp).
/// With no fillers ahead, the empty publishes at the failed slot itself; the real is
/// retried at the next slot by the normal precollation pipeline, so NO restart is
/// scheduled.
#[test]
fn genuine_error_with_allow_empty_publishes_recovery_empty() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let request_id = 73;
    // Last slot of window 0 so the post-publish precollate_block(slot + 1) lands in
    // window 1 and cleanly no-ops (out of the current leader window).
    let slot = SlotIndex::new(3);
    let parent = make_parent_info(2, 0xAA);
    let parent_block_id = make_block_id(42, 0xAA);
    ctrl.insert_precollated(slot, make_precollated(request_id, Some(parent.clone())));
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 3, now + Duration::from_secs(100)));
    // Recent finalization => allow_empty holds (C++ block-producer.cpp).
    ctrl.last_consensus_finalized_at = Some(now);

    // first_non_progressed == 3 => slot 3 is still current (not passed); window 0.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(3), WindowIndex::new(0))
        .with_received(make_candidate_id(2, 0xAA), make_received(2, parent_block_id, None));
    ctrl.on_collation_failed_impl(
        &mut backend,
        slot,
        request_id,
        ton_block::error!("collation error"),
    );

    let head = ctrl.local_chain_head().expect("allow_empty recovery must publish an empty");
    assert_eq!(
        head.slot, slot,
        "with no fillers ahead, the recovery empty publishes at the failed slot"
    );
    assert!(
        !ctrl.precollated_contains(slot),
        "the failed precollation entry is consumed by the recovery"
    );
    assert!(
        ctrl.block_generation_active.is_none(),
        "the failed real released the marker; the out-of-window next slot did not re-dispatch"
    );
    assert!(
        queue.delayed().is_empty(),
        "allow_empty recovery publishes an empty and advances - it does NOT schedule a restart"
    );
}

/// Genuine error with `allow_empty` when the per-slot wake already published empty
/// fillers past the failed slot (local_chain_head advanced, but consensus has not
/// progressed past the dispatch slot): the recovery empty is re-tagged onto the
/// current chain head rather than the now-occupied failed slot, avoiding equivocation
/// (C++ block-producer.cpp) - identical to the on_collation_complete late re-tag.
#[test]
fn genuine_error_with_allow_empty_retags_onto_filler_head() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let request_id = 74;
    let dispatch_slot = SlotIndex::new(1);
    // Fillers already advanced the local chain head to slot 2 in window 0.
    let head_parent = make_parent_info(2, 0xBB);
    let head_block_id = make_block_id(42, 0xBB);
    ctrl.set_local_chain_head(Some(make_local_chain_head(0, 2, head_parent)));
    ctrl.insert_precollated(
        dispatch_slot,
        make_precollated(request_id, Some(make_parent_info(0, 0x11))),
    );
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 1, now + Duration::from_secs(100)));
    ctrl.last_consensus_finalized_at = Some(now);

    // Consensus progress cursor still at the dispatch slot (the fillers are not yet
    // notarized), so the fsm check does NOT drop it; leader window still 0.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(1), WindowIndex::new(0))
        .with_received(make_candidate_id(2, 0xBB), make_received(2, head_block_id, None));
    ctrl.on_collation_failed_impl(
        &mut backend,
        dispatch_slot,
        request_id,
        ton_block::error!("collation error"),
    );

    let head = ctrl.local_chain_head().expect("allow_empty recovery must publish an empty");
    assert_eq!(
        head.slot,
        SlotIndex::new(3),
        "the recovery empty re-tags onto local_chain_head.slot + 1, not the occupied failed slot"
    );
    assert!(
        !ctrl.precollated_contains(dispatch_slot),
        "the failed precollation entry is consumed by the recovery"
    );
    assert!(ctrl.block_generation_active.is_none(), "the failed real released the marker");
    assert!(
        queue.delayed().is_empty(),
        "the re-tagged recovery advances the pipeline - it does NOT schedule a restart"
    );
}

/// Genuine error whose leader window already moved on (C++ block-producer.cpp):
/// drop the slot - neither a recovery empty nor a restart - since the next window's
/// producer now owns the chain. Mirrors on_collation_complete's stale-window discard.
#[test]
fn genuine_error_stale_window_drops_without_restart_or_publish() {
    let description = Arc::new(make_mc_description_spw(1, 4));
    let now = description.get_time();
    let (mut ctrl, queue) = ctrl_with_recording_queue(description);

    let request_id = 75;
    let slot = SlotIndex::new(0);
    ctrl.insert_precollated(slot, make_precollated(request_id, Some(make_parent_info(0, 0xAA))));
    ctrl.block_generation_active =
        Some(in_flight_state(request_id, 0, 0, now + Duration::from_secs(100)));
    // Even with allow_empty otherwise satisfied, the stale window must take precedence.
    ctrl.last_consensus_finalized_at = Some(now);

    // Slot 0 is in window 0, but the current leader window is already 1. The fsm cursor
    // (0) does NOT drop slot 0, so the window guard is the behavior under test.
    let mut backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(1));
    ctrl.on_collation_failed_impl(
        &mut backend,
        slot,
        request_id,
        ton_block::error!("collation error"),
    );

    assert!(
        ctrl.local_chain_head().is_none(),
        "a stale-window error must not publish a recovery empty"
    );
    assert!(!ctrl.precollated_contains(slot), "a stale-window error drops the precollation entry");
    assert!(ctrl.block_generation_active.is_none(), "the failed real released the marker");
    assert!(queue.delayed().is_empty(), "a stale-window error must not schedule a restart");
}

/// A genuine-error restart must race the SAME window-end budget as the original
/// attempt: [`CollationController::restart_collation`] re-dispatches with the PINNED
/// soft/hard cutoffs and budget anchor rather than recomputing them from the (now
/// advanced) clock. Drives the restart directly - the recording queue drops the
/// scheduled closure - with a resolvable parent so it reaches a real dispatch, after
/// advancing the clock well past the original soft window.
#[test]
fn restart_collation_reuses_pinned_deadlines_across_clock_advance() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_000);
    let target_rate = Duration::from_millis(500);
    let opts = SessionOptions { target_rate, ..SessionOptions::default() };
    let description = Arc::new(make_description_with_opts(1, ShardIdent::masterchain(), opts));
    description.set_time(now);
    let (mut ctrl, _queue) = ctrl_with_recording_queue(description.clone());

    let slot = SlotIndex::new(0);
    // A resolvable parent (old gen-time) so prepare_collation is Ready and, with no
    // finalized-seqno lag, dispatch proceeds as a REAL collation.
    let parent = make_parent_info(3, 0x20);
    let parent_block = make_block_id(41, 0x20);
    let mut backend = FakeCollationBackend::new(slot, WindowIndex::new(0))
        .with_received(make_candidate_id(3, 0x20), make_received(3, parent_block, Some(0)));
    backend.available_parents.insert(slot, parent);

    // The original attempt's pinned budget, intentionally unrelated to `now + budget`.
    let pinned = CollationDeadlineContext {
        soft_deadline: now + Duration::from_secs(7),
        hard_deadline: now + Duration::from_secs(30),
        budget_anchor: now - Duration::from_millis(500),
    };

    // The clock advances well past the original soft window before the restart fires:
    // a recomputed budget would land ~20s later, so reuse is observable.
    description.set_time(now + Duration::from_secs(20));
    ctrl.restart_collation(&mut backend, slot, Some(pinned));

    let active = ctrl.block_generation_active.as_ref().expect("restart must re-dispatch a real");
    assert_eq!(
        active.deadlines, pinned,
        "the restart must reuse the pinned deadline context verbatim, not recompute from now"
    );
    let request =
        ctrl.precollated(slot).expect("restart re-registers the precollation").request.clone();
    assert_eq!(
        request.get_collation_hard_deadline(),
        Some(pinned.hard_deadline),
        "the re-dispatched request must carry the pinned hard deadline"
    );
    assert_eq!(
        request.get_collation_budget_anchor(),
        Some(pinned.budget_anchor),
        "the re-dispatched request must carry the pinned budget anchor"
    );
}
