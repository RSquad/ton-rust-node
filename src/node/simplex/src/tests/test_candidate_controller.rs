/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for
//! [`crate::candidate_controller::CandidateController`].
//!
//! Included directly from `candidate_controller.rs` via `#[path]` so tests can
//! reach the private repair constants and the `#[cfg(test)]`
//! `requested_candidates` accessor without widening visibility. Mirrors the
//! convention used by `tests/test_collation_controller.rs`.
//!
//! Scope (outbound-repair state mechanics only — full ingress / serving
//! orchestration is still covered end-to-end by
//! `tests/test_session_processor.rs`, which drives the real
//! `CandidateQueueAdapter` / `CandidateBackendAdapter`):
//!
//! - `ensure_candidate_available` with an unresolved `BlockIdExt` schedules a
//!   single deferred retry on the controller queue (the post-extraction
//!   `queue.post_delayed` path) and emits no request.
//! - `ensure_candidate_available` with a resolved mapping requests once and arms
//!   the throttle; a same-tick repeat is suppressed.
//! - `request_candidate` sends immediately for a zero delay and defers onto the
//!   controller queue for a non-zero delay, arming the throttle either way.
//! - `cancel_repairs_for_slot` clears the throttle entry and forwards the
//!   receiver cancel effect.
//! - `prune_requested_below` drops throttle entries strictly below the cursor.

use super::*;
use crate::{
    certificate::{Certificate, NotarCertPtr, VoteSignature},
    controller_queue::{ControllerQueue, ControllerTask},
    receiver::ReceiverHealthCounters,
    simplex_state::NotarizeVote,
    MetricsHandle, SessionNode, SessionOptions,
};
use consensus_common::{ConsensusCommonFactory, ResolverPurpose};
use std::{cell::RefCell, sync::Mutex};
use ton_api::{deserialize_boxed, ton::consensus::candidatehashdata::CandidateHashDataEmpty};
use ton_block::{Ed25519KeyOption, ShardIdent, ZeroizingBytes};

/*
    --------------------------------------------------------------------
    Test helpers
    --------------------------------------------------------------------
*/

/// Deterministic session clock base for the throttle / deferred-retry math.
fn base_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Build a single-node masterchain `SessionDescription` (local node at index 0).
fn make_description(node_count: u32) -> SessionDescription {
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
        &SessionOptions::default(),
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &ShardIdent::masterchain(),
        SystemTime::now(),
        None,
    )
    .expect("SessionDescription::new must succeed for test fixture")
}

/// Build an `Arc<SessionTelemetry>` for the controller constructor.
fn make_telemetry(description: &SessionDescription) -> Arc<SessionTelemetry> {
    Arc::new(SessionTelemetry::new(
        MetricsHandle::new(None),
        description,
        Arc::new(ReceiverHealthCounters::new()),
        0,
        Duration::from_secs(30),
        SystemTime::now(),
    ))
}

/// Recording [`ControllerQueue`] fake: counts immediate posts and captures the
/// expiration time of every `post_delayed` so the deferred-retry / delayed
/// request paths can be asserted without a real task queue. The boxed tasks are
/// dropped — re-entry through the queue is exercised against the real adapter by
/// `tests/test_session_processor.rs`.
#[derive(Default)]
struct RecordingQueue {
    immediate: Mutex<usize>,
    delayed: Mutex<Vec<SystemTime>>,
}

impl ControllerQueue<CandidateController> for RecordingQueue {
    fn post_boxed(&self, _task: ControllerTask<CandidateController>) {
        *self.immediate.lock().unwrap() += 1;
    }

    fn post_delayed_boxed(&self, at: SystemTime, _task: ControllerTask<CandidateController>) {
        self.delayed.lock().unwrap().push(at);
    }
}

impl RecordingQueue {
    fn delayed_times(&self) -> Vec<SystemTime> {
        self.delayed.lock().unwrap().clone()
    }
}

/// Build a controller bound to a recording queue. Returns the shared
/// `SessionDescription` (for clock control) and the queue handle (for
/// scheduling assertions) alongside the controller.
fn mk_ctrl(node_count: u32) -> (CandidateController, Arc<SessionDescription>, Arc<RecordingQueue>) {
    let description = Arc::new(make_description(node_count));
    let telemetry = make_telemetry(&description);
    let queue = Arc::new(RecordingQueue::default());
    let ctrl = CandidateController::new(
        description.clone(),
        telemetry,
        queue.clone() as ControllerQueuePtr<CandidateController>,
        None,
    );
    (ctrl, description, queue)
}

/// Narrow [`CandidateBackend`] fake for the repair tests. Holds a real (empty)
/// `SimplexState` so the skip-cert / notar-cert probes behave, a resolver
/// `BlockIdExt -> RawCandidateId` map, and records the receiver repair effects.
/// `database()` is intentionally unstubbed — the serving fallback is covered by
/// `tests/test_session_processor.rs`.
struct FakeCandidateBackend {
    simplex_state: SimplexState,
    mapping: HashMap<BlockIdExt, RawCandidateId>,
    requested: RefCell<Vec<(u32, UInt256)>>,
    cancelled: RefCell<Vec<u32>>,
    known_in_validation: bool,
    database: Option<DatabaseController>,
}

impl FakeCandidateBackend {
    fn new(description: &SessionDescription) -> Self {
        Self {
            simplex_state: SimplexState::new(description).expect("SimplexState::new must succeed"),
            mapping: HashMap::new(),
            requested: RefCell::new(Vec::new()),
            cancelled: RefCell::new(Vec::new()),
            known_in_validation: false,
            database: None,
        }
    }

    fn with_mapping(mut self, block_id: BlockIdExt, candidate_id: RawCandidateId) -> Self {
        self.mapping.insert(block_id, candidate_id);
        self
    }

    fn requested(&self) -> Vec<(u32, UInt256)> {
        self.requested.borrow().clone()
    }

    fn cancelled(&self) -> Vec<u32> {
        self.cancelled.borrow().clone()
    }
}

impl CandidateBackend for FakeCandidateBackend {
    fn simplex_state(&self) -> &SimplexState {
        &self.simplex_state
    }

    fn database(&self) -> &DatabaseController {
        self.database.as_ref().expect(
            "database() is not stubbed: serving paths are covered by tests/test_session_processor.rs",
        )
    }

    fn candidate_known_in_validation(&self, _candidate_id: &RawCandidateId) -> bool {
        self.known_in_validation
    }

    fn collation_candidate_id_by_block_id(&self, block_id: &BlockIdExt) -> Option<RawCandidateId> {
        self.mapping.get(block_id).cloned()
    }

    fn persist_candidate_info(
        &mut self,
        _slot: SlotIndex,
        _candidate_hash: &UInt256,
        _leader_idx: ValidatorIndex,
        _candidate_hash_data_bytes: &[u8],
        _signature: Vec<u8>,
    ) {
    }

    fn request_candidate_from_peers(&self, slot: u32, hash: UInt256) {
        self.requested.borrow_mut().push((slot, hash));
    }

    fn cancel_candidate_requests_for_slot(&self, slot: u32) {
        self.cancelled.borrow_mut().push(slot);
    }
}

fn make_block_id(seqno: u32, hash_byte: u8) -> BlockIdExt {
    BlockIdExt::with_params(
        ShardIdent::masterchain(),
        seqno,
        UInt256::from([hash_byte; 32]),
        UInt256::from([hash_byte ^ 0xFF; 32]),
    )
}

fn make_candidate_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from([hash_byte; 32]) }
}

fn collation_parent_opts() -> EnsureCandidateAvailabilityOptions {
    EnsureCandidateAvailabilityOptions {
        purpose: ResolverPurpose::SimplexCollationParent,
        include_parent_chain: false,
    }
}

/*
    --------------------------------------------------------------------
    ensure_candidate_available
    --------------------------------------------------------------------
*/

#[test]
fn ensure_candidate_available_schedules_deferred_retry_on_missing_mapping() {
    let (mut ctrl, desc, queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);
    let mut backend = FakeCandidateBackend::new(&desc);

    // No mapping on either the collation cache half or the owned book half.
    ctrl.ensure_candidate_available(
        make_block_id(150, 0xEF),
        collation_parent_opts(),
        &mut backend,
    );

    let delayed = queue.delayed_times();
    assert_eq!(delayed.len(), 1, "missing mapping must schedule exactly one deferred retry");
    assert_eq!(
        delayed[0],
        base + RESOLVER_AVAILABILITY_RETRY_DELAY,
        "retry must be armed one resolver-availability delay into the future"
    );
    assert!(backend.requested().is_empty(), "no request may be sent before the mapping resolves");
    assert!(
        ctrl.requested_candidates().is_empty(),
        "throttle must stay empty until a candidate id is known"
    );
}

#[test]
fn ensure_candidate_available_requests_when_mapping_resolves() {
    let (mut ctrl, desc, queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);

    let block_id = make_block_id(150, 0xEF);
    let candidate_id = make_candidate_id(15, 0xDE);
    let mut backend =
        FakeCandidateBackend::new(&desc).with_mapping(block_id.clone(), candidate_id.clone());

    ctrl.ensure_candidate_available(block_id, collation_parent_opts(), &mut backend);

    assert_eq!(
        backend.requested(),
        vec![(candidate_id.slot.value(), candidate_id.hash.clone())],
        "resolved mapping must request the candidate body once"
    );
    assert!(
        ctrl.requested_candidates().contains_key(&candidate_id),
        "a resolved request must arm the throttle"
    );
    assert!(queue.delayed_times().is_empty(), "a resolved mapping must not defer");
}

#[test]
fn ensure_candidate_available_throttles_repeat_within_window() {
    let (mut ctrl, desc, _queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);

    let block_id = make_block_id(150, 0xEF);
    let candidate_id = make_candidate_id(15, 0xDE);
    let mut backend = FakeCandidateBackend::new(&desc).with_mapping(block_id.clone(), candidate_id);

    ctrl.ensure_candidate_available(block_id.clone(), collation_parent_opts(), &mut backend);
    // Second ensure at the same logical time: still inside the throttle window.
    ctrl.ensure_candidate_available(block_id, collation_parent_opts(), &mut backend);

    assert_eq!(
        backend.requested().len(),
        1,
        "a second ensure inside the throttle window must be suppressed"
    );
}

/*
    --------------------------------------------------------------------
    request_candidate
    --------------------------------------------------------------------
*/

#[test]
fn request_candidate_immediate_sends_and_arms_throttle() {
    let (mut ctrl, desc, queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);
    let mut backend = FakeCandidateBackend::new(&desc);

    let slot = SlotIndex::new(7);
    let hash = UInt256::from([0x11; 32]);
    ctrl.request_candidate(slot, hash.clone(), Some(Duration::ZERO), &mut backend);

    assert_eq!(
        backend.requested(),
        vec![(slot.value(), hash.clone())],
        "a zero-delay request must hit the network synchronously"
    );
    assert!(ctrl.requested_candidates().contains_key(&RawCandidateId { slot, hash }));
    assert!(queue.delayed_times().is_empty(), "a zero-delay request must not defer");
}

#[test]
fn request_candidate_delayed_posts_to_queue() {
    let (mut ctrl, desc, queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);
    let mut backend = FakeCandidateBackend::new(&desc);

    let slot = SlotIndex::new(7);
    let hash = UInt256::from([0x11; 32]);
    let delay = Duration::from_millis(250);
    ctrl.request_candidate(slot, hash.clone(), Some(delay), &mut backend);

    let delayed = queue.delayed_times();
    assert_eq!(delayed.len(), 1, "a non-zero delay must schedule on the controller queue");
    assert_eq!(delayed[0], base + delay, "delayed request must fire after the requested delay");
    assert!(
        backend.requested().is_empty(),
        "a deferred request must not hit the network synchronously"
    );
    assert!(
        ctrl.requested_candidates().contains_key(&RawCandidateId { slot, hash }),
        "scheduling a delayed request must still arm the throttle"
    );
}

/*
    --------------------------------------------------------------------
    cancel_repairs_for_slot / prune_requested_below
    --------------------------------------------------------------------
*/

#[test]
fn cancel_repairs_for_slot_clears_throttle_and_cancels_receiver() {
    let (mut ctrl, desc, _queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);
    let mut backend = FakeCandidateBackend::new(&desc);

    let slot = SlotIndex::new(7);
    let hash = UInt256::from([0x11; 32]);
    ctrl.request_candidate(slot, hash.clone(), Some(Duration::ZERO), &mut backend);
    assert!(ctrl.requested_candidates().contains_key(&RawCandidateId { slot, hash: hash.clone() }));

    ctrl.cancel_repairs_for_slot(slot, &mut backend);

    assert!(
        !ctrl.requested_candidates().contains_key(&RawCandidateId { slot, hash }),
        "cancel must drop the throttle entry for the slot"
    );
    assert_eq!(backend.cancelled(), vec![slot.value()], "cancel must forward the receiver effect");
}

#[test]
fn prune_requested_below_drops_stale_slots() {
    let (mut ctrl, desc, _queue) = mk_ctrl(4);
    let base = base_time();
    desc.set_time(base);
    let mut backend = FakeCandidateBackend::new(&desc);

    let low = SlotIndex::new(5);
    let high = SlotIndex::new(10);
    let low_hash = UInt256::from([0x05; 32]);
    let high_hash = UInt256::from([0x10; 32]);
    ctrl.request_candidate(low, low_hash.clone(), Some(Duration::ZERO), &mut backend);
    ctrl.request_candidate(high, high_hash.clone(), Some(Duration::ZERO), &mut backend);
    assert_eq!(ctrl.requested_candidates().len(), 2);

    ctrl.prune_requested_below(SlotIndex::new(8));

    assert!(
        !ctrl.requested_candidates().contains_key(&RawCandidateId { slot: low, hash: low_hash }),
        "entries strictly below the cursor must be pruned"
    );
    assert!(
        ctrl.requested_candidates().contains_key(&RawCandidateId { slot: high, hash: high_hash }),
        "entries at or above the cursor must be retained"
    );
}

/*
    --------------------------------------------------------------------
    serve_query_fallback empty-block reconstruction
    --------------------------------------------------------------------
*/

/// Build a `CandidateInfoRecord` for an empty block that embeds its own
/// `BlockIdExt` (the `consensus.candidateHashDataEmpty.block` field), exactly as
/// `persist_candidate_info` stores it.
fn make_empty_candidate_info(
    candidate_id: &RawCandidateId,
    parent: &RawCandidateId,
    block_id: BlockIdExt,
) -> CandidateInfoRecord {
    let hash_data = CandidateHashDataEmpty {
        block: block_id,
        parent: CandidateId { slot: parent.slot.value() as i32, hash: parent.hash.clone() },
    };
    CandidateInfoRecord {
        candidate_id: candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: CandidateHashData::Consensus_CandidateHashDataEmpty(hash_data),
        signature: vec![0xA1, 0xB2, 0xC3],
    }
}

#[test]
fn reconstruct_empty_candidate_data_works_from_db_metadata_without_book_entry() {
    // The RequestCandidate fallback DB path must reconstruct an empty block from
    // the `CandidateInfoRecord` alone -- e.g. when a peer requests repair after
    // this node restarted and its in-memory `CandidateBook` is empty. The block id
    // is embedded in the metadata, so no book entry is required.
    let (ctrl, _desc, _queue) = mk_ctrl(4);

    let candidate_id = make_candidate_id(15, 0xDE);
    let parent_id = make_candidate_id(14, 0xCA);
    let block_id = make_block_id(150, 0xAB);
    let info = make_empty_candidate_info(&candidate_id, &parent_id, block_id.clone());

    // Precondition: the in-memory book has no entry for this candidate.
    assert!(
        ctrl.book.received(&candidate_id).is_none(),
        "fixture must exercise the empty-book (post-restart) path"
    );

    let bytes = ctrl
        .reconstruct_empty_candidate_data_from_info(&candidate_id, &info)
        .expect("empty block must reconstruct from DB metadata alone");

    let decoded = deserialize_boxed(&bytes)
        .expect("reconstructed bytes must deserialize")
        .downcast::<CandidateData>()
        .expect("reconstructed payload must be CandidateData");
    match decoded {
        CandidateData::Consensus_Empty(empty) => {
            assert_eq!(empty.slot, candidate_id.slot.value() as i32, "slot must round-trip");
            assert_eq!(
                empty.block, block_id,
                "reconstructed block id must come from the embedded metadata, not the book"
            );
        }
        other => panic!("expected CandidateData::Consensus_Empty, got {other:?}"),
    }
}

/*
    --------------------------------------------------------------------
    delayed request_candidate throttle cleanup
    --------------------------------------------------------------------
*/

/// Capturing queue that retains posted delayed tasks so the test can replay the
/// `request_candidate` deferred closure against a `&mut CandidateController` plus
/// a backend view (mirrors `tests/test_controller_queue.rs`). The default
/// `RecordingQueue` above intentionally drops task bodies; this one keeps them.
#[derive(Default)]
struct ReplayQueue {
    delayed: Mutex<Vec<ControllerTask<CandidateController>>>,
}

impl ControllerQueue<CandidateController> for ReplayQueue {
    fn post_boxed(&self, _task: ControllerTask<CandidateController>) {}

    fn post_delayed_boxed(&self, _at: SystemTime, task: ControllerTask<CandidateController>) {
        self.delayed.lock().unwrap().push(task);
    }
}

impl ReplayQueue {
    fn take_delayed(&self) -> Vec<ControllerTask<CandidateController>> {
        std::mem::take(&mut *self.delayed.lock().unwrap())
    }
}

/// Controller bound to a [`ReplayQueue`] so the delayed task can be replayed.
fn mk_ctrl_replay(
    node_count: u32,
) -> (CandidateController, Arc<SessionDescription>, Arc<ReplayQueue>) {
    let description = Arc::new(make_description(node_count));
    let telemetry = make_telemetry(&description);
    let queue = Arc::new(ReplayQueue::default());
    let ctrl = CandidateController::new(
        description.clone(),
        telemetry,
        queue.clone() as ControllerQueuePtr<CandidateController>,
        None,
    );
    (ctrl, description, queue)
}

/// Build a real (non-stub) `ReceivedCandidate` so `has_real_body` returns `true`.
fn make_real_body(slot: SlotIndex, block_id: BlockIdExt) -> ReceivedCandidate {
    ReceivedCandidate {
        slot,
        source_idx: ValidatorIndex::new(0),
        candidate_hash_data_bytes: vec![0xAB, 0xCD, 0xEF],
        block_id,
        root_hash: UInt256::from([2u8; 32]),
        file_hash: UInt256::from([3u8; 32]),
        data: ConsensusCommonFactory::create_block_payload(vec![]),
        collated_data: ConsensusCommonFactory::create_block_payload(vec![]),
        gen_utime_ms: Some(1_700_000_000_000),
        receive_time: SystemTime::now(),
        is_empty: false,
        parent_id: None,
    }
}

#[test]
fn delayed_request_candidate_clears_throttle_when_already_satisfied() {
    // When the delayed repair task finds the body and notar cert already present,
    // it must drop the (slot,hash) throttle entry instead of leaving it behind for
    // later slot-level cleanup.
    let (mut ctrl, desc, queue) = mk_ctrl_replay(4);
    let base = base_time();
    desc.set_time(base);

    let slot = SlotIndex::new(7);
    let block_hash = UInt256::from([0x11; 32]);
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
    let mut backend = FakeCandidateBackend::new(&desc);

    // Schedule a delayed repair: arms the throttle and captures exactly one task.
    ctrl.request_candidate(
        slot,
        block_hash.clone(),
        Some(Duration::from_millis(250)),
        &mut backend,
    );
    assert!(
        ctrl.requested_candidates().contains_key(&candidate_id),
        "scheduling a delayed request must arm the throttle"
    );
    let tasks = queue.take_delayed();
    assert_eq!(tasks.len(), 1, "a non-zero delay must post exactly one deferred task");

    // Before the deferred task fires, the body and the notar cert both arrive.
    ctrl.book.insert_received(candidate_id.clone(), make_real_body(slot, make_block_id(70, 0x70)));
    let cert: NotarCertPtr = Arc::new(Certificate::new(
        NotarizeVote { slot, block_hash: block_hash.clone() },
        vec![VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 8])],
    ));
    backend
        .simplex_state
        .set_notarize_certificate(&desc, slot, &block_hash, cert)
        .expect("storing a fresh notar cert must succeed");
    assert!(ctrl.book.has_real_body(&candidate_id), "fixture must present a real body");
    assert!(
        backend.simplex_state.get_notarize_certificate(slot, &block_hash).is_some(),
        "fixture must present a notar cert"
    );

    // Replay the deferred task: it must take the "already have what we need" path.
    for task in tasks {
        task(&mut ctrl, &mut backend);
    }

    assert!(
        !ctrl.requested_candidates().contains_key(&candidate_id),
        "an already-satisfied delayed request must clear its throttle entry"
    );
    assert!(
        backend.requested().is_empty(),
        "no peer request may be sent once body+notar are already present"
    );
}
