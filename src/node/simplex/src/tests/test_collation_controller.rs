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
//! - `resolve_candidate_id_by_block_id` lookup order: synchronous cache
//!   → book → miss.
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
//! - `Debug` impl smoke test.
//!
//! The self-collation observability funnel (start/generated/acceptance
//! tracking + the generated-candidate validation watch) now lives in
//! [`crate::session_telemetry::SessionTelemetry`]; its state mechanics
//! and counter emission are covered by `tests/test_session_telemetry.rs`.

use super::*;
use crate::{block::WindowIndex, candidate_book::ReceivedCandidate, SessionNode, SessionOptions};
use consensus_common::ConsensusCommonFactory;
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
        _request: consensus_common::AsyncRequestPtr,
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
    CollationController::new(test_collation_queue(), callbacks, description, telemetry)
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
    /// (`book_received_block_id` / `book_received_gen_utime_ms` /
    /// `book_candidate_id_by_block_id`), so the parent-resolution helpers can be
    /// driven with no `SessionProcessor` / real `CandidateBook`.
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

    fn book_candidate_id_by_block_id(&self, block_id: &BlockIdExt) -> Option<RawCandidateId> {
        self.received.iter().find(|(_, c)| &c.block_id == block_id).map(|(id, _)| id.clone())
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
    resolve_parent_block_id / resolve_candidate_id_by_block_id
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

#[test]
fn resolve_candidate_id_by_block_id_prefers_cache_then_book_then_none() {
    let mut ctrl = new_ctrl();
    let book_id = make_candidate_id(2, 0xBB);
    let book_block = make_block_id(22, 0x20);
    let backend = FakeCollationBackend::new(SlotIndex::new(0), WindowIndex::new(0))
        .with_received(book_id.clone(), make_received(2, book_block.clone(), None));
    let cache_id = make_candidate_id(1, 0xAA);
    let cache_block = make_block_id(11, 0x10);
    ctrl.insert_generated_parent(cache_id.clone(), cache_block.clone());

    assert_eq!(ctrl.resolve_candidate_id_by_block_id(&cache_block, &backend), Some(cache_id));
    assert_eq!(ctrl.resolve_candidate_id_by_block_id(&book_block, &backend), Some(book_id));
    assert!(ctrl.resolve_candidate_id_by_block_id(&make_block_id(999, 0x99), &backend).is_none());
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
    assert_eq!(
        ctrl.try_begin_collation_slot(slot_leader, CollationAttempt::Initial),
        Some(local_idx),
    );

    // Not-leader case: returns None.
    assert!(ctrl.try_begin_collation_slot(slot_not_leader, CollationAttempt::Initial).is_none());

    // Leader case but precollation already pending: returns None.
    ctrl.insert_precollated(slot_leader, make_precollated(0, None));
    assert!(ctrl.try_begin_collation_slot(slot_leader, CollationAttempt::Initial).is_none());
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

    let (req_id, request) = ctrl.create_pending_collation_request(slot, Some(parent.clone()), now);

    assert_eq!(req_id, 0, "first request must be id 0 (post-increment)");
    assert_eq!(request.get_request_id(), 0);
    assert!(!request.is_cancelled());
    assert!(ctrl.precollated_contains(slot));
    assert_eq!(ctrl.precollated_max_slot(), Some(slot));

    let stored = ctrl.precollated(slot).expect("entry must be registered");
    assert!(stored.result.is_none(), "fresh entry starts with no result");
    assert_eq!(stored.parent.as_ref().unwrap().slot, parent.slot);

    // Second request must get the next monotonic id.
    let (next_id, _) = ctrl.create_pending_collation_request(SlotIndex::new(5), None, now);
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
