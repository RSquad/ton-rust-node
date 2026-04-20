/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for SessionProcessor
//!
//! These tests are included directly from session_processor.rs via #[path] attribute
//! to access private internals without requiring pub(crate) visibility.

use super::*;
use crate::{
    block::ValidatorIndex,
    receiver::Receiver,
    task_queue::{CallbackTaskQueuePtr, TaskQueuePtr},
    SessionId, SessionNode, SessionOptions, SIMPLEX_ROUNDLESS,
};
use consensus_common::{
    AsyncRequestPtr, BlockPayloadPtr, BlockSourceInfo, CollationParentHint, PublicKey,
    PublicKeyHash, SessionStats, ValidatorBlockCandidateCallback,
    ValidatorBlockCandidateDecisionCallback,
};
use std::{
    collections::VecDeque,
    env, fs, mem,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};
use ton_api::{
    deserialize_boxed,
    ton::consensus::{
        simplex::{
            certificate::Certificate, unsignedvote::SkipVote, vote::Vote as TlVote,
            votesignature::VoteSignature, votesignatureset::VoteSignatureSet, CandidateAndCert,
            Certificate as CertificateBoxed, VoteSignature as VoteSignatureBoxed,
        },
        CandidateData,
    },
    IntoBoxed,
};
use ton_block::{
    error, sha256_digest, signature::BlockSignaturesVariant, BlockIdExt, BocFlags, BocWriter,
    BuilderData, Ed25519KeyOption, ShardIdent, UInt256,
};

// ============================================================================
// Test Helpers
// ============================================================================

/// Create valid BOC bytes from raw data (for tests that need valid BOC input).
///
/// The compress/decompress pipeline requires valid BOC, so mock data must be
/// wrapped in a cell + serialized as BOC with appropriate flags.
fn make_test_boc(data: &[u8], flags: BocFlags) -> Vec<u8> {
    let mut b = BuilderData::new();
    b.append_raw(data, data.len() * 8).unwrap();
    let cell = b.into_cell().unwrap();
    let mut buf = Vec::new();
    BocWriter::with_flags([cell], flags).unwrap().write(&mut buf).unwrap();
    buf
}

/// Create test validators with equal weights
fn create_test_validators(count: u32) -> Vec<SessionNode> {
    (0..count)
        .map(|_| {
            let public_key = Ed25519KeyOption::generate().expect("Failed to generate key");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight: 1 }
        })
        .collect()
}

/// Create test SessionDescription with default options
#[allow(dead_code)]
fn create_test_desc(
    nodes: &[SessionNode],
    local_idx: usize,
) -> Arc<crate::session_description::SessionDescription> {
    create_test_desc_with_opts(nodes, local_idx, &SessionOptions::default())
}

fn create_test_desc_for_shard_with_opts(
    nodes: &[SessionNode],
    local_idx: usize,
    shard: ShardIdent,
    opts: &SessionOptions,
) -> Arc<crate::session_description::SessionDescription> {
    let local_key = nodes[local_idx].public_key.clone();
    Arc::new(
        crate::session_description::SessionDescription::new(
            opts,
            SessionId::default(),
            1, // initial_block_seqno
            nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None, // metrics
        )
        .unwrap(),
    )
}

// ============================================================================
// Mock Receiver
// ============================================================================

/// Recorded action from SessionProcessor → Receiver
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum ReceiverAction {
    /// send_vote() was called
    SendVote { vote: TlVote },
    /// cache_our_vote_for_standstill() was called
    CacheOurVoteForStandstill { vote: TlVote },
    /// send_block_broadcast() was called
    SendBlockBroadcast { slot: u32, candidate_hash: UInt256 },
    /// cache_notarization_cert() was called
    CacheNotarizationCert { slot: u32, block_hash: UInt256 },
    /// send_certificate() was called
    SendCertificate { certificate: CertificateBoxed },
    /// cache_standstill_certificate() was called
    CacheStandstillCertificate {
        slot: u32,
        kind: crate::receiver::StandstillCertificateType,
        bytes_len: usize,
    },
    /// cache_last_final_certificate() was called
    CacheLastFinalCertificate { slot: u32, bytes_len: usize },
    /// cleanup() was called
    Cleanup { up_to_slot: u32 },
    /// set_ingress_slot_begin() was called
    SetIngressSlotBegin { slot: u32 },
    /// set_ingress_progress_slot() was called
    SetIngressProgressSlot { slot: u32 },
    /// cancel_candidate_requests_for_slot() was called
    CancelCandidateRequestsForSlot { slot: u32 },
    /// set_standstill_slots() was called
    SetStandstillSlots { begin: u32, end: u32 },
    /// request_candidate() was called
    RequestCandidate { slot: u32, block_hash: UInt256 },
}

/// Mock receiver that records all outbound calls
struct MockReceiver {
    /// Recorded actions (sent votes, broadcasts, etc.)
    actions: Arc<Mutex<VecDeque<ReceiverAction>>>,
    last_standstill_slots: Arc<Mutex<Option<(u32, u32)>>>,
}

impl MockReceiver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            actions: Arc::new(Mutex::new(VecDeque::new())),
            last_standstill_slots: Arc::new(Mutex::new(None)),
        })
    }

    /// Get all recorded actions (drains the queue)
    fn drain_actions(&self) -> Vec<ReceiverAction> {
        self.actions.lock().unwrap().drain(..).collect()
    }

    /// Get count of pending actions
    #[allow(dead_code)]
    fn action_count(&self) -> usize {
        self.actions.lock().unwrap().len()
    }
}

impl Receiver for MockReceiver {
    fn send_vote(&self, vote: TlVote) {
        self.actions.lock().unwrap().push_back(ReceiverAction::SendVote { vote });
    }

    fn cache_our_vote_for_standstill(&self, vote: TlVote) {
        self.actions.lock().unwrap().push_back(ReceiverAction::CacheOurVoteForStandstill { vote });
    }

    fn send_block_broadcast(&self, slot: u32, candidate_hash: UInt256, _candidate: CandidateData) {
        self.actions
            .lock()
            .unwrap()
            .push_back(ReceiverAction::SendBlockBroadcast { slot, candidate_hash });
    }

    fn cache_notarization_cert(&self, slot: u32, block_hash: UInt256, _notar_cert_data: Vec<u8>) {
        self.actions
            .lock()
            .unwrap()
            .push_back(ReceiverAction::CacheNotarizationCert { slot, block_hash });
    }

    fn cache_candidate_bytes(&self, _slot: u32, _block_hash: UInt256, _candidate_data: Vec<u8>) {
        // No-op for tests
    }

    fn cleanup(&self, up_to_slot: u32) {
        self.actions.lock().unwrap().push_back(ReceiverAction::Cleanup { up_to_slot });
    }

    fn set_ingress_slot_begin(&self, slot: u32) {
        self.actions.lock().unwrap().push_back(ReceiverAction::SetIngressSlotBegin { slot });
    }

    fn set_ingress_progress_slot(&self, slot: u32) {
        self.actions.lock().unwrap().push_back(ReceiverAction::SetIngressProgressSlot { slot });
    }

    fn cancel_candidate_requests_for_slot(&self, slot: u32) {
        self.actions
            .lock()
            .unwrap()
            .push_back(ReceiverAction::CancelCandidateRequestsForSlot { slot });
    }

    fn request_candidate(&self, slot: u32, block_hash: UInt256) {
        self.actions
            .lock()
            .unwrap()
            .push_back(ReceiverAction::RequestCandidate { slot, block_hash });
    }

    fn start(&self) {}

    fn reschedule_standstill(&self) {
        // No-op for tests
    }

    fn set_standstill_slots(&self, begin: u32, end: u32) {
        let mut last = self.last_standstill_slots.lock().unwrap();
        if *last == Some((begin, end)) {
            return;
        }
        *last = Some((begin, end));
        self.actions.lock().unwrap().push_back(ReceiverAction::SetStandstillSlots { begin, end });
    }

    fn send_certificate(&self, certificate: CertificateBoxed) {
        self.actions.lock().unwrap().push_back(ReceiverAction::SendCertificate { certificate });
    }

    fn cache_standstill_certificate(
        &self,
        slot: u32,
        kind: crate::receiver::StandstillCertificateType,
        cert_bytes: Vec<u8>,
    ) {
        self.actions.lock().unwrap().push_back(ReceiverAction::CacheStandstillCertificate {
            slot,
            kind,
            bytes_len: cert_bytes.len(),
        });
    }

    fn cache_last_final_certificate(&self, slot: u32, cert_bytes: Vec<u8>) {
        self.actions.lock().unwrap().push_back(ReceiverAction::CacheLastFinalCertificate {
            slot,
            bytes_len: cert_bytes.len(),
        });
    }

    fn stop(&self) {
        // No-op for tests
    }
}

// ============================================================================
// Mock Listener
// ============================================================================

/// Mock listener for testing (no-op implementation)
struct MockListener;

impl consensus_common::SessionListener for MockListener {
    fn on_candidate(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: UInt256,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        _callback: ValidatorBlockCandidateDecisionCallback,
    ) {
    }

    fn on_generate_slot(
        &self,
        _source_info: BlockSourceInfo,
        _request: AsyncRequestPtr,
        _parent: CollationParentHint,
        _callback: ValidatorBlockCandidateCallback,
    ) {
    }

    fn on_block_committed(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: SessionStats,
    ) {
        panic!(
            "on_block_committed must not be called for Simplex sessions (finalized-driven only)"
        );
    }

    fn on_block_skipped(&self, _round: u32) {}

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        panic!(
            "unexpected legacy get_approved_candidate request in session_processor MockListener \
             (root_hash={}); active simplex tests must not use this callback",
            root_hash.to_hex_string()
        );
    }
}

// ============================================================================
// Recording Listener (for emission model tests)
// ============================================================================

/// Recorded listener event
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum ListenerEvent {
    /// on_block_committed was called
    Committed {
        /// Slot from BlockSignaturesSimplex (meaningful identifier)
        slot: u32,
        root_hash: UInt256,
        is_final: bool,
    },
    /// on_block_skipped was called (not used in SIMPLEX_ROUNDLESS mode)
    Skipped { round: u32 },
    /// on_block_finalized was called (out-of-order delivery)
    Finalized { block_id: BlockIdExt, root_hash: UInt256 },
}

// NOTE: dummy_source_info helper removed - was only used by emission model tests

/// Listener that records all commits and skips for verification
struct RecordingListener {
    events: Arc<Mutex<Vec<ListenerEvent>>>,
}

#[allow(dead_code)]
impl RecordingListener {
    fn new() -> Arc<Self> {
        Arc::new(Self { events: Arc::new(Mutex::new(Vec::new())) })
    }

    /// Get all recorded events (drains the list)
    fn drain_events(&self) -> Vec<ListenerEvent> {
        mem::take(&mut *self.events.lock().unwrap())
    }

    /// Get count of recorded events
    fn event_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl consensus_common::SessionListener for RecordingListener {
    fn on_candidate(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: UInt256,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        _callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        // No-op
    }

    fn on_generate_slot(
        &self,
        _source_info: BlockSourceInfo,
        _request: AsyncRequestPtr,
        _parent: CollationParentHint,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        // No-op
    }

    fn on_block_committed(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: SessionStats,
    ) {
        panic!(
            "on_block_committed must not be called for Simplex sessions (finalized-driven only)"
        );
    }

    fn on_block_skipped(&self, round: u32) {
        self.events.lock().unwrap().push(ListenerEvent::Skipped { round });
    }

    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        _source_info: BlockSourceInfo,
        root_hash: UInt256,
        _file_hash: UInt256,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        self.events.lock().unwrap().push(ListenerEvent::Finalized { block_id, root_hash });
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        panic!(
            "unexpected legacy get_approved_candidate request in session_processor \
             RecordingListener (root_hash={}); active simplex tests must not use this callback",
            root_hash.to_hex_string()
        );
    }
}

// ============================================================================
// Mock Task Queues
// ============================================================================

/// Simple in-memory task queue for tests
struct TestTaskQueue {
    tasks: Arc<Mutex<VecDeque<TaskPtr>>>,
}

impl TestTaskQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self { tasks: Arc::new(Mutex::new(VecDeque::new())) })
    }

    /// Execute all pending tasks
    #[allow(dead_code)]
    fn execute_all(&self, processor: &mut SessionProcessor) {
        while let Some(task) = self.tasks.lock().unwrap().pop_front() {
            task(processor);
        }
    }
}

impl crate::task_queue::TaskQueue<TaskPtr> for TestTaskQueue {
    fn is_overloaded(&self) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.tasks.lock().unwrap().is_empty()
    }

    fn post_closure(&self, task: TaskPtr) {
        self.tasks.lock().unwrap().push_back(task);
    }

    fn pull_closure(
        &self,
        _timeout: Duration,
        _last_warn_dump_time: &mut SystemTime,
    ) -> Option<TaskPtr> {
        self.tasks.lock().unwrap().pop_front()
    }

    fn flush(&self) {
        self.tasks.lock().unwrap().clear();
    }
}

/// Callback task queue (no-op for simple tests)
struct TestCallbackQueue;

impl crate::task_queue::TaskQueue<crate::task_queue::CallbackTaskPtr> for TestCallbackQueue {
    fn is_overloaded(&self) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        true
    }

    fn post_closure(&self, _task: crate::task_queue::CallbackTaskPtr) {
        // No-op
    }

    fn pull_closure(
        &self,
        _timeout: Duration,
        _last_warn_dump_time: &mut SystemTime,
    ) -> Option<crate::task_queue::CallbackTaskPtr> {
        None
    }

    fn flush(&self) {}
}

// ============================================================================
// Mock Database
// ============================================================================

/// Mock database for tests (in-memory, no persistence)
struct MockDb;

impl MockDb {
    fn new() -> crate::database::SimplexDbPtr {
        // Create a temporary database in memory
        let temp_dir = env::temp_dir().join(format!("test_db_{}", rand::random::<u64>()));
        fs::create_dir_all(&temp_dir).unwrap();
        crate::database::SimplexDb::open(&temp_dir, "test").unwrap()
    }
}

// ============================================================================
// Test Fixture
// ============================================================================

struct TestFixture {
    nodes: Vec<SessionNode>,
    description: Arc<crate::session_description::SessionDescription>,
    processor: SessionProcessor,
    receiver: Arc<MockReceiver>,
    #[allow(dead_code)]
    task_queue: Arc<TestTaskQueue>,
}

/// Create test SessionDescription with custom options
fn create_test_desc_with_opts(
    nodes: &[SessionNode],
    local_idx: usize,
    opts: &SessionOptions,
) -> Arc<crate::session_description::SessionDescription> {
    create_test_desc_for_shard_with_opts(nodes, local_idx, ShardIdent::masterchain(), opts)
}

impl TestFixture {
    /// Create a test fixture with N validators (local is validator 0)
    fn new(validator_count: u32) -> Self {
        Self::new_with_opts(validator_count, SessionOptions::default())
    }

    fn new_shard(validator_count: u32) -> Self {
        Self::new_with_shard_and_local_idx(
            validator_count,
            0,
            ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
            SessionOptions::default(),
        )
    }

    /// Create a test fixture with N validators, custom options, and local_idx
    fn new_with_local_idx(validator_count: u32, local_idx: usize, opts: SessionOptions) -> Self {
        Self::new_with_shard_and_local_idx(
            validator_count,
            local_idx,
            ShardIdent::masterchain(),
            opts,
        )
    }

    fn new_with_shard_and_local_idx(
        validator_count: u32,
        local_idx: usize,
        shard: ShardIdent,
        opts: SessionOptions,
    ) -> Self {
        let nodes = create_test_validators(validator_count);
        let description = create_test_desc_for_shard_with_opts(&nodes, local_idx, shard, &opts);

        let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> =
            Arc::new(MockListener);
        let listener_weak = Arc::downgrade(&listener);

        let task_queue = TestTaskQueue::new();
        let callback_queue: CallbackTaskQueuePtr = Arc::new(TestCallbackQueue);
        let overlay_manager =
            consensus_common::ConsensusCommonFactory::create_dummy_overlay_manager();
        let receiver = MockReceiver::new();
        let db = MockDb::new();

        let stop_flag = Arc::new(AtomicBool::new(false));
        let health_counters = Arc::new(crate::receiver::ReceiverHealthCounters::new());
        let processor = SessionProcessor::new(
            description.clone(),
            listener_weak,
            task_queue.clone() as TaskQueuePtr,
            callback_queue,
            overlay_manager,
            receiver.clone() as crate::receiver::ReceiverPtr,
            stop_flag,
            db,
            0,
            health_counters,
        )
        .unwrap();

        Self { nodes, description, processor, receiver, task_queue }
    }

    /// Create a test fixture with N validators and custom session options
    fn new_with_opts(validator_count: u32, opts: SessionOptions) -> Self {
        Self::new_with_shard_and_local_idx(validator_count, 0, ShardIdent::masterchain(), opts)
    }

    /// Advance time by duration
    fn advance_time(&mut self, delta: Duration) {
        self.processor.advance_time(delta);
    }

    /// Execute all pending tasks
    #[allow(dead_code)]
    fn execute_pending_tasks(&mut self) {
        self.task_queue.execute_all(&mut self.processor);
    }

    /// Get all actions sent to receiver
    fn drain_receiver_actions(&self) -> Vec<ReceiverAction> {
        self.receiver.drain_actions()
    }
}

fn metrics_counter(processor: &SessionProcessor, name: &str) -> u64 {
    processor.get_metrics_receiver().snapshot().counters.get(name).copied().unwrap_or(0)
}

// ============================================================================
// Certificate helpers
// ============================================================================

fn build_skip_certificate_tl(
    session_id: &SessionId,
    nodes: &[SessionNode],
    slot: u32,
    signers: &[usize],
) -> CertificateBoxed {
    let unsigned_vote = SkipVote { slot: slot as i32 }.into_boxed();

    // Sign exactly the same bytes Certificate::from_tl verifies:
    // create_data_to_sign(session_id, serialize(UnsignedVote))
    let raw_vote_bytes = crate::utils::serialize_unsigned_vote(&unsigned_vote);
    let to_sign = crate::utils::create_data_to_sign(session_id, &raw_vote_bytes);

    let tl_sigs: Vec<VoteSignatureBoxed> = signers
        .iter()
        .map(|&idx| {
            let sig = nodes[idx].public_key.sign(&to_sign).expect("failed to sign skip vote");
            VoteSignature { who: idx as i32, signature: sig.to_vec().into() }.into_boxed()
        })
        .collect();

    let sig_set = VoteSignatureSet { votes: tl_sigs.into() }.into_boxed();

    Certificate { vote: unsigned_vote, signatures: sig_set }.into_boxed()
}

fn make_test_final_cert(slot: SlotIndex, block_hash: UInt256) -> crate::certificate::FinalCertPtr {
    Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote { slot, block_hash },
        signatures: vec![
            crate::certificate::VoteSignature {
                validator_idx: ValidatorIndex::new(0),
                signature: vec![1u8; 64],
            },
            crate::certificate::VoteSignature {
                validator_idx: ValidatorIndex::new(1),
                signature: vec![2u8; 64],
            },
            crate::certificate::VoteSignature {
                validator_idx: ValidatorIndex::new(2),
                signature: vec![3u8; 64],
            },
        ],
    })
}

// ============================================================================
// Basic Tests
// ============================================================================

#[test]
fn test_session_processor_creation() {
    let fixture = TestFixture::new(4);
    assert_eq!(fixture.description.get_total_nodes(), 4);
    assert_eq!(fixture.description.get_self_idx(), ValidatorIndex::new(0));
}

#[test]
fn test_manual_clock_control() {
    let mut fixture = TestFixture::new_shard(4);

    // Seed manual time to a deterministic value first.
    //
    // Without this, `get_time()` starts as real time and the elapsed duration would
    // include scheduler/overhead jitter between reads.
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let initial_time = fixture.description.get_time();

    // Advance time by 1 second
    fixture.advance_time(Duration::from_secs(1));

    let new_time = fixture.description.get_time();
    let elapsed = new_time.duration_since(initial_time).unwrap();
    assert_eq!(elapsed, Duration::from_secs(1));

    // Advance time by 5 more seconds
    fixture.advance_time(Duration::from_secs(5));

    let final_time = fixture.description.get_time();
    let total_elapsed = final_time.duration_since(initial_time).unwrap();
    assert_eq!(total_elapsed, Duration::from_secs(6));
}

#[test]
fn test_genesis_collation_expected_seqno_uses_initial_block_seqno() {
    // Regression test for single-host nets:
    // when collation starts with parent=None (first block in epoch), we must use
    // SessionDescription::initial_block_seqno instead of defaulting to 0.
    //
    // Without this, collation fails with:
    // `seqno mismatch: candidate has seqno=1, expected=0 (derived from parent=None)`
    let mut fixture = TestFixture::new_shard(4);

    // initial_block_seqno is set to 1 in create_test_desc()
    assert_eq!(fixture.description.get_initial_block_seqno(), 1);
    // Ensure the expected seqno does NOT depend on finalized_head_seqno when parent=None.
    fixture.processor.finalized_head_seqno = Some(123);

    let slot = SlotIndex::new(132);

    let genesis_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());

    // Use valid BOC bytes — compress_candidate_data requires valid BOC input
    let block_boc = make_test_boc(&[0xAA], BocFlags::all());
    let collated_boc = make_test_boc(&[0xBB], BocFlags::Crc32);
    let candidate = crate::ValidatorBlockCandidate {
        public_key: fixture.nodes[0].public_key.clone(),
        id: genesis_block_id,
        collated_file_hash: UInt256::from_slice(&sha256_digest(&collated_boc)),
        data: consensus_common::ConsensusCommonFactory::create_block_payload(block_boc),
        collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(collated_boc),
    };

    fixture
        .processor
        .create_normal_block_desc(slot, &candidate, &None)
        .expect("genesis collation must accept initial_block_seqno when parent is None");
}

#[test]
fn test_should_generate_empty_block_uses_committed_head_at_session_start() {
    // Masterchain uses finalized-head gating for empty-block generation (C++ parity:
    // `last_consensus_finalized_seqno_ + 1 < new_seqno`), while shardchains use
    // the MC lag threshold rule. Verify masterchain path.
    let nodes = create_test_validators(4);
    let local_idx = 0;
    let initial_block_seqno = 47;
    let local_key = nodes[local_idx].public_key.clone();
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();
    let description = Arc::new(
        crate::session_description::SessionDescription::new(
            &opts,
            SessionId::default(),
            initial_block_seqno,
            &nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None,
        )
        .unwrap(),
    );

    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = Arc::new(MockListener);
    let listener_weak = Arc::downgrade(&listener);

    let task_queue = TestTaskQueue::new();
    let callback_queue: CallbackTaskQueuePtr = Arc::new(TestCallbackQueue);
    let overlay_manager = consensus_common::ConsensusCommonFactory::create_dummy_overlay_manager();
    let receiver = MockReceiver::new();
    let db = MockDb::new();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let health_counters = Arc::new(crate::receiver::ReceiverHealthCounters::new());
    let processor = SessionProcessor::new(
        description,
        listener_weak,
        task_queue.clone() as TaskQueuePtr,
        callback_queue,
        overlay_manager,
        receiver.clone() as crate::receiver::ReceiverPtr,
        stop_flag,
        db,
        0,
        health_counters,
    )
    .unwrap();

    assert_eq!(processor.finalized_head_seqno, Some(46));

    // Slot 0 is the initial `first_non_progressed_slot` in fresh state.
    // MC: new_seqno=48, finalized_head=46 -> 46+1=47 < 48 -> empty
    assert!(processor.should_generate_empty_block(SlotIndex::new(0), 48));
    // MC: new_seqno=47, finalized_head=46 -> 46+1=47 == 47 -> NOT empty
    assert!(!processor.should_generate_empty_block(SlotIndex::new(0), 47));
}

#[test]
fn test_out_of_order_finalized_delivery_emits_immediately_when_body_present() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    // Set finalized head high so this test isolates finalized-delivery callback.
    fixture.processor.finalized_head_seqno = Some(100);
    fixture.processor.finalized_head_block_id = Some(BlockIdExt::with_params(
        ShardIdent::masterchain(),
        100,
        UInt256::rand(),
        UInt256::rand(),
    ));

    let slot = 103u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![7u8, 8, 9, 10]);
    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    let received = fixture
        .processor
        .received_candidates
        .get(&candidate_id)
        .expect("candidate should be present")
        .clone();

    let event = BlockFinalizedEvent {
        slot: candidate_id.slot,
        block_hash: candidate_id.hash.clone(),
        block_id: Some(received.block_id.clone()),
        certificate: make_test_final_cert(candidate_id.slot, candidate_id.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event);

    let events = recording.drain_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            ListenerEvent::Finalized { block_id, root_hash }
                if block_id == &received.block_id && root_hash == &received.root_hash
        )),
        "out-of-order finalized callback must be emitted when body is already known"
    );
    assert!(
        !events.iter().any(|e| matches!(e, ListenerEvent::Committed { .. })),
        "on_block_committed must be suppressed in out-of-order mode"
    );
    assert!(
        fixture.processor.finalized_pending_body.is_empty(),
        "no pending-body retention expected when finalized body is already present"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "finalized-driven mode must not request missing candidates"
    );
}

#[test]
fn test_out_of_order_finalized_delivery_emits_when_body_arrives_late_and_dedups() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    // Set finalized head high so this test isolates finalized-delivery callback.
    fixture.processor.finalized_head_seqno = Some(70);
    fixture.processor.finalized_head_block_id = Some(BlockIdExt::with_params(
        ShardIdent::masterchain(),
        70,
        UInt256::rand(),
        UInt256::rand(),
    ));

    let slot = 77u32;
    let block_data = vec![0xA5, 0x5A, 0xCC, 0x33];
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, block_data.clone());

    // Finalization observed first (before full candidate body is known).
    let root_hash = UInt256::from_slice(&sha256_digest(&block_data));
    let block_id = BlockIdExt {
        shard_id: fixture.processor.description.get_shard().clone(),
        seq_no: slot,
        root_hash: root_hash.clone(),
        file_hash: root_hash,
    };
    let event = BlockFinalizedEvent {
        slot: candidate_id.slot,
        block_hash: candidate_id.hash.clone(),
        block_id: Some(block_id.clone()),
        certificate: make_test_final_cert(candidate_id.slot, candidate_id.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event.clone());

    assert!(
        recording.drain_events().is_empty(),
        "no finalized callback before candidate body is available"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "out-of-order mode must not request candidates on finalization"
    );

    // Body arrives later: delayed finalized-delivery path should emit callback now.
    fixture.processor.on_candidate_received(leader_source, broadcast, None);
    let events_after_body = recording.drain_events();
    let finalized_count = events_after_body
        .iter()
        .filter(|e| matches!(e, ListenerEvent::Finalized { block_id: id, .. } if id == &block_id))
        .count();
    assert_eq!(
        finalized_count, 1,
        "exactly one finalized callback expected after late body arrival"
    );
    assert!(
        !events_after_body.iter().any(|e| matches!(e, ListenerEvent::Committed { .. })),
        "on_block_committed must stay suppressed in out-of-order mode"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "finalized-driven mode must not request missing candidates after body arrival"
    );

    // Duplicate finalization observation should not re-emit callback.
    fixture.processor.handle_block_finalized(event);
    let events_after_duplicate = recording.drain_events();
    assert!(
        !events_after_duplicate
            .iter()
            .any(|e| matches!(e, ListenerEvent::Finalized { block_id: id, .. } if id == &block_id)),
        "duplicate finalized observation must be deduplicated"
    );
}

#[test]
fn test_out_of_order_mode_does_not_run_commit_chain_recovery_for_missing_body() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    // No candidate body is inserted.
    let slot = SlotIndex::new(55);
    let block_hash = UInt256::rand();
    let block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 55, UInt256::rand(), UInt256::rand());
    let finalized_id = RawCandidateId { slot, hash: block_hash.clone() };

    let event = BlockFinalizedEvent {
        slot,
        block_hash: block_hash.clone(),
        block_id: Some(block_id),
        certificate: make_test_final_cert(slot, block_hash),
    };
    fixture.processor.handle_block_finalized(event);

    assert!(
        fixture.processor.finalized_pending_body.contains_key(&finalized_id),
        "finalization should be buffered until body arrival"
    );
    assert!(
        !fixture.processor.received_candidates.contains_key(&finalized_id),
        "finalized-driven mode must not seed stubs for missing bodies"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "finalized-driven mode must not request missing candidates"
    );

    // Periodic scheduler path should also avoid candidate recovery.
    fixture.processor.check_all();
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "check_all must not trigger candidate requests in finalized-driven mode"
    );
}

#[test]
fn test_check_all_updates_awake_time() {
    let mut fixture = TestFixture::new(4);

    let before = fixture.processor.get_next_awake_time();

    // Call check_all (should reset awake time)
    fixture.processor.check_all();

    let after = fixture.processor.get_next_awake_time();

    // Awake time should be updated (pushed into future)
    assert!(after >= before);
}

#[test]
fn test_start_arms_timeout_from_current_time_after_cold_delay() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    // Constructor path must not arm startup skip timers.
    assert!(
        fixture.processor.simplex_state.get_next_timeout().is_none(),
        "startup timeout must not be armed before start()"
    );

    // Simulate prolonged cold startup / delayed readiness.
    fixture.processor.advance_time(Duration::from_secs(120));
    let now = fixture.description.get_time();

    fixture.processor.start();

    let timeout = fixture
        .processor
        .simplex_state
        .get_next_timeout()
        .expect("startup timeout must be scheduled after start()");
    let opts = fixture.description.opts();
    let expected = now + opts.first_block_timeout + opts.target_rate;
    assert_eq!(
        timeout, expected,
        "startup timeout must be anchored to start() time, not constructor time \
        (C++ ref: alarm = now + first_block_timeout + target_rate)"
    );
}

#[test]
fn test_receiver_records_no_actions_initially() {
    let fixture = TestFixture::new(4);
    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().all(|a| {
            matches!(a, ReceiverAction::SetIngressSlotBegin { slot: 0 })
                || matches!(a, ReceiverAction::SetIngressProgressSlot { slot: 0 })
                || matches!(a, ReceiverAction::SetStandstillSlots { begin: 0, .. })
        }),
        "Expected only initial ingress/standstill sync actions"
    );
}

#[test]
fn test_check_all_syncs_standstill_slots_when_tracked_interval_changes() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    fixture.processor.check_all();
    let initial_actions = fixture.drain_receiver_actions();
    assert!(
        !initial_actions.iter().any(|a| matches!(a, ReceiverAction::SetStandstillSlots { .. })),
        "unchanged tracked interval should not resync standstill slots"
    );

    fixture.processor.simplex_state.set_first_non_finalized_slot(crate::block::SlotIndex::new(4));
    let (expected_begin, expected_end) =
        fixture.processor.simplex_state.get_tracked_slots_interval();

    fixture.processor.check_all();
    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(
            a,
            ReceiverAction::SetStandstillSlots { begin, end }
                if *begin == expected_begin && *end == expected_end
        )),
        "check_all must resync receiver standstill slots when the FSM tracked interval changes"
    );
}

// ============================================================================
// Certificate relay / standstill cache tests
// ============================================================================

#[test]
fn test_on_certificate_relays_and_caches_skip_certificate_once() {
    let mut fixture = TestFixture::new(4);

    // Build a valid skip certificate (threshold_66 for 4 validators is 3)
    let slot = 0u32;
    let tl_cert =
        build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, slot, &[0, 1, 2]);

    // First application should store + relay + cache
    fixture.processor.on_certificate(1, tl_cert.clone());

    let actions = fixture.drain_receiver_actions();
    let send_cert_count =
        actions.iter().filter(|a| matches!(a, ReceiverAction::SendCertificate { .. })).count();
    let cache_standstill_count = actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                ReceiverAction::CacheStandstillCertificate {
                    slot: s,
                    kind: crate::receiver::StandstillCertificateType::Skip,
                    ..
                } if *s == slot
            )
        })
        .count();

    // C++ parity (pool.cpp handle_saved_certificate): every newly accepted
    // certificate is relayed once, regardless of origin.
    assert_eq!(send_cert_count, 1, "C++ parity: foreign skip cert must be relayed once");
    assert_eq!(
        cache_standstill_count, 1,
        "expected exactly one cache_standstill_certificate on first apply"
    );

    // Second application should be ignored (already have skip certificate), so no relay/caching
    fixture.processor.on_certificate(1, tl_cert);
    let actions2 = fixture.drain_receiver_actions();
    assert!(
        !actions2.iter().any(|a| matches!(a, ReceiverAction::SendCertificate { .. })),
        "should not relay duplicate skip certificate"
    );
    assert!(
        !actions2.iter().any(|a| matches!(a, ReceiverAction::CacheStandstillCertificate { .. })),
        "should not cache duplicate skip certificate"
    );
}

#[test]
fn test_future_certificate_is_not_rejected_like_cpp() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    let slot = fixture.processor.simplex_state.first_too_new_vote_slot().value();
    let tl_cert =
        build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, slot, &[0, 1, 2]);

    fixture.processor.on_certificate(1, tl_cert);

    assert!(
        fixture.processor.simplex_state.has_skip_certificate(SlotIndex::new(slot)),
        "certificate at the vote too-new boundary should still be stored"
    );
    assert_eq!(
        fixture.processor.simplex_state.get_first_non_progressed_slot(),
        SlotIndex::new(0),
        "out-of-order future certificate must not advance progress past unresolved earlier slots"
    );

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SendCertificate { .. })),
        "accepted future certificate should still be relayed"
    );
}

#[test]
fn test_handle_finalization_reached_caches_final_certificate_for_standstill() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(7);
    let block_hash = UInt256::rand();

    // Create a finalization certificate with sufficient weight (3 out of 4)
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2]),
    ];
    let vote = crate::simplex_state::FinalizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    let event = crate::simplex_state::FinalizationReachedEvent {
        slot,
        block_hash: block_hash.clone(),
        certificate: cert,
    };

    fixture.processor.handle_finalization_reached(event);

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(
            a,
            ReceiverAction::CacheStandstillCertificate {
                slot: s,
                kind: crate::receiver::StandstillCertificateType::Final,
                ..
            } if *s == slot.value()
        )),
        "expected final cert to be cached in standstill bundle"
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, ReceiverAction::CacheLastFinalCertificate { slot: s, .. } if *s == slot.value())),
        "expected last final certificate to be cached"
    );
}

#[test]
fn test_update_standstill_after_final_cert_updates_ingress_when_cleanup_is_skipped() {
    let mut fixture = TestFixture::new(4);

    // Regression: for early slots, history cleanup is skipped (up_to_slot == 0).
    fixture.processor.cleanup_old_slots(crate::block::SlotIndex::new(8));
    let cleanup_actions = fixture.drain_receiver_actions();
    assert!(
        !cleanup_actions.iter().any(|a| matches!(a, ReceiverAction::Cleanup { .. })),
        "receiver.cleanup must not run before MAX_HISTORY_SLOTS"
    );

    // Simulate finalized frontier advancement and final-cert hook.
    fixture.processor.simplex_state.set_first_non_finalized_slot(crate::block::SlotIndex::new(9));
    fixture.processor.update_standstill_after_final_cert(crate::block::SlotIndex::new(8));

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressSlotBegin { slot: 9 })),
        "final-cert path must advance receiver ingress lower bound even when cleanup is skipped"
    );
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressProgressSlot { slot: 9 })),
        "final-cert path must advance receiver ingress progress cursor"
    );
}

#[test]
fn test_recovery_set_first_non_finalized_slot_updates_receiver_ingress() {
    let mut fixture = TestFixture::new(4);

    fixture.processor.recovery_set_first_non_finalized_slot(crate::block::SlotIndex::new(9));

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressSlotBegin { slot: 9 })),
        "recovery must synchronize receiver ingress lower bound with restored frontier"
    );
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressProgressSlot { slot: 9 })),
        "recovery must synchronize receiver ingress progress cursor with restored frontier"
    );
}

#[test]
fn test_skip_certificate_syncs_progress_cursor_without_advancing_finalized_frontier() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    let slot = 0u32;
    let tl_cert =
        build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, slot, &[0, 1, 2]);

    fixture.processor.on_certificate(1, tl_cert);

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressSlotBegin { slot: 0 })),
        "skip progress must keep ingress lower bound at the finalized frontier"
    );
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SetIngressProgressSlot { slot: 1 })),
        "skip certificate must advance ingress progress cursor to the next slot"
    );
}

#[test]
fn test_skip_certificate_cancels_stale_candidate_request_repairs() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    let slot = crate::block::SlotIndex::new(0);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };

    fixture.processor.request_candidate(slot, block_hash.clone(), Some(Duration::from_secs(1)));
    assert!(
        fixture.processor.requested_candidates.contains_key(&candidate_id),
        "request should be scheduled before skip"
    );

    let tl_cert =
        build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, slot.value(), &[0, 1, 2]);
    fixture.processor.on_certificate(1, tl_cert);

    assert!(
        !fixture.processor.requested_candidates.contains_key(&candidate_id),
        "skip must cancel scheduled requestCandidate repairs for the skipped slot"
    );

    let actions_after_skip = fixture.drain_receiver_actions();
    assert!(
        actions_after_skip
            .iter()
            .any(|a| matches!(a, ReceiverAction::CancelCandidateRequestsForSlot { slot: 0 })),
        "skip must cancel receiver-side pending requests for the skipped slot"
    );

    fixture.advance_time(Duration::from_secs(2));
    fixture.processor.check_all();

    let actions_after_delay = fixture.drain_receiver_actions();
    assert!(
        !actions_after_delay
            .iter()
            .any(|a| matches!(a, ReceiverAction::RequestCandidate { slot: 0, .. })),
        "cancelled delayed request must not fire after the slot is skipped"
    );
}

#[test]
fn test_handle_notarization_reached_requests_missing_candidate_body() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(7);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };

    // Ensure the candidate body is missing.
    assert!(
        !fixture.processor.received_candidates.contains_key(&candidate_id),
        "test setup: candidate body must be missing"
    );
    assert!(
        !fixture.processor.requested_candidates.contains_key(&candidate_id),
        "test setup: candidate must not be pre-requested"
    );

    // Create a notarization certificate (content doesn't need to be valid for this unit test).
    // threshold_66 for 4 validators is 3.
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2]),
    ];
    let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    let event = crate::simplex_state::NotarizationReachedEvent {
        slot,
        block_hash: block_hash.clone(),
        certificate: cert,
    };

    // Act: should schedule requestCandidate for missing body.
    fixture.processor.handle_notarization_reached(event);

    assert!(
        fixture.processor.requested_candidates.contains_key(&candidate_id),
        "expected SessionProcessor to schedule requestCandidate for missing notarized block body"
    );
}

// ============================================================================
// Time-based Tests
// ============================================================================

#[test]
fn test_delayed_action_execution() {
    let mut fixture = TestFixture::new(4);

    // Post a delayed action 5 seconds in the future
    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();

    let delay = Duration::from_secs(5);
    let expiration_time = fixture.description.get_time() + delay;

    fixture.processor.post_delayed_action(expiration_time, move |_processor| {
        *executed_clone.lock().unwrap() = true;
    });

    // Action should not execute yet
    fixture.processor.check_all();
    assert!(!*executed.lock().unwrap(), "Action executed too early");

    // Advance time by 4 seconds (still not enough)
    fixture.advance_time(Duration::from_secs(4));
    fixture.processor.check_all();
    assert!(!*executed.lock().unwrap(), "Action executed too early");

    // Advance time by 2 more seconds (total 6, past the 5-second mark)
    fixture.advance_time(Duration::from_secs(2));
    fixture.processor.check_all();
    assert!(*executed.lock().unwrap(), "Action should have executed");
}

#[test]
fn test_time_isolation_between_tests() {
    // Each test should have independent time
    let fixture1 = TestFixture::new(4);
    let time1 = fixture1.description.get_time();

    let fixture2 = TestFixture::new(4);
    let time2 = fixture2.description.get_time();

    // Times should be reasonably close (both created around same time).
    // Fixture creation can occasionally be slow on contended CI hosts.
    let diff = time2.duration_since(time1).unwrap_or_else(|_| time1.duration_since(time2).unwrap());
    assert!(
        diff < Duration::from_secs(2),
        "Test fixtures should have similar initial times (diff={:?})",
        diff
    );
}

// ============================================================================
// Batch Finalization Tests
// ============================================================================

/// Notarized parents + finalized descendant (Case A)
///
/// Scenario:
/// - slot 1: notarized (NotarCert), not finalized
/// - slot 2: notarized (NotarCert), not finalized
/// - slot 3: finalized (FinalCert)
/// - parent chain: 1 → 2 → 3 (all non-empty blocks)
///
/// Expected: THREE finalization deliveries emitted in order:
/// - delivery(round=1, is_final=false, sigs=NotarCert/approve)
/// - delivery(round=2, is_final=false, sigs=NotarCert/approve)
/// - delivery(round=3, is_final=true,  sigs=FinalCert/final)
///
/// This verifies C++ `finalize_blocks()` parity:
/// - Parent blocks CAN be finalized (even if body not yet received)
/// - Parent blocks use NotarCert/`create_simplex_approve` signatures
/// - Triggered block uses FinalCert/`create_simplex` signatures
/// - No panic on is_triggered_block=false
/// - Round stream is gapless (round = slot in Option B)
///
/// Status: PLACEHOLDER - full test requires FSM integration
#[test]
#[ignore] // TODO: requires FSM events + candidate resolution infrastructure
fn test_batch_finalization_notarized_parents_finalized_descendant() {
    // This test is a placeholder documenting the expected behavior.
    //
    // Full implementation requires:
    // 1. Creating a SessionProcessor with RecordingListener that captures is_final
    // 2. Registering 3 candidates with proper parent_id links in received_candidates
    // 3. Simulating FSM votes (NotarizeVote for slots 1/2, FinalizeVote for slot 3)
    // 4. Triggering handle_block_finalized(slot=3) which should:
    //    - collect_parent_chain(3) → [1, 2, 3]
    //    - commit_single_block for each (parents use NotarCert, triggered uses FinalCert)
    // 5. Asserting listener.drain_events() returns exactly:
    //    - Committed { round: 1, is_final: false }
    //    - Committed { round: 2, is_final: false }
    //    - Committed { round: 3, is_final: true }
    //
    // For now, this serves as documentation of the expected behavior and a reminder
    // to implement the full test once the FSM integration infrastructure is in place.

    todo!("implement full batch finalization integration test");
}

// ============================================================================
// SIMPLEX_ROUNDLESS Mode Tests
// ============================================================================

/// Test that SIMPLEX_ROUNDLESS constant is u32::MAX
#[test]
fn test_simplex_roundless_constant_value() {
    assert_eq!(SIMPLEX_ROUNDLESS, u32::MAX, "SIMPLEX_ROUNDLESS should be u32::MAX");
    assert_eq!(SIMPLEX_ROUNDLESS, 0xFFFFFFFF, "SIMPLEX_ROUNDLESS should be 0xFFFFFFFF");
}

// ============================================================================
// Direct Emission Model Tests (SIMPLEX_ROUNDLESS cleanup)
// ============================================================================
//
// These tests verify the simplified direct emission architecture:
// - notify_block_committed() is called directly from commit_single_block()
// - No SlotOutcome buffering (mark/emit pattern removed)
// - Empty blocks don't trigger ValidatorGroup callbacks
// - slots BTreeMap is only used for SlotRuntime (collation/validation state)

/// Test that SlotEntry structure only contains runtime (no outcome field)
///
/// Documents the simplified SlotEntry structure after SIMPLEX_ROUNDLESS cleanup.
#[test]
fn test_slot_entry_structure_simplified() {
    let fixture = TestFixture::new(4);

    // Create a slot entry via slot_runtime_mut (the only way to create entries now)
    let slot = SlotIndex::new(5);

    // Initially no slot entry exists
    assert!(fixture.processor.slots.get(&slot).is_none());
}

/// Test that slot_entry_mut creates entry with runtime on first access
#[test]
fn test_slot_entry_mut_creates_runtime() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(7);

    // Access runtime (should create entry)
    fixture.processor.slot_runtime_mut(slot).pending_generate = true;

    // Verify entry exists with runtime
    let entry = fixture.processor.slots.get(&slot);
    assert!(entry.is_some(), "Slot entry should exist after slot_runtime_mut");
    assert!(entry.unwrap().runtime.is_some(), "Runtime should be created");
    assert!(
        entry.unwrap().runtime.as_ref().unwrap().pending_generate,
        "pending_generate should be true"
    );
}

/// Test that SlotRuntime can be used independently for collation tracking
#[test]
fn test_slot_runtime_collation_tracking() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(3);

    // Set collation state
    fixture.processor.slot_set_pending_generate(slot, true);
    assert!(fixture.processor.slot_is_pending_generate(slot));

    fixture.processor.slot_set_generated(slot, true);
    assert!(fixture.processor.slot_is_generated(slot));

    fixture.processor.slot_set_sent_generated(slot, true);
    assert!(fixture.processor.slot_is_sent_generated(slot));
}

/// Test that slot_started_at tracks when slot processing began
#[test]
fn test_slot_started_at_tracking() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(10);

    // Access slot to create runtime with default start time
    fixture.processor.slot_runtime_mut(slot);

    let started_at = fixture.processor.slot_started_at(slot);
    let now = fixture.description.get_time();

    // Started time should be close to now (within 1 second)
    let diff = now.duration_since(started_at).unwrap_or(Duration::ZERO);
    assert!(diff < Duration::from_secs(1), "Slot start time should be close to current time");
}

#[test]
fn test_candidate_decision_fail_drops_late_failure_for_finalized_block() {
    // Regression: in roundless Simplex, validation callbacks can arrive late (after the block is
    // already finalized). In this case we must drop the result and NOT schedule retries / WARN.
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(5);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    // Pretend we've already finalized past this block seqno.
    fixture.processor.finalized_head_seqno = Some(100);

    // Create a non-empty RawCandidate with seqno <= finalized_head_seqno.
    let block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 42, UInt256::rand(), UInt256::rand());
    let creator = fixture.nodes[0].public_key.clone();
    let block = crate::block::BlockCandidate {
        id: block_id,
        collated_file_hash: UInt256::rand(),
        data: vec![1, 2, 3],
        collated_data: vec![4, 5, 6],
        creator,
    };
    let raw_candidate = crate::block::RawCandidate::new(
        candidate_id.clone(),
        None,
        ValidatorIndex::new(0),
        block,
        vec![],
    );

    fixture.processor.pending_validations.insert(
        candidate_id.clone(),
        PendingValidation {
            raw_candidate,
            slot,
            receive_time: fixture.description.get_time(),
            source_idx: ValidatorIndex::new(0),
        },
    );
    fixture.processor.pending_approve.insert(candidate_id.clone());
    fixture.processor.validation_attempt_map.insert(candidate_id.clone(), 0);

    fixture.processor.candidate_decision_fail(slot, candidate_id.clone(), error!("boom"));

    // Must be dropped (no retry scheduling, no rejection bookkeeping).
    assert!(!fixture.processor.pending_validations.contains_key(&candidate_id));
    assert!(!fixture.processor.pending_approve.contains(&candidate_id));
    assert!(!fixture.processor.validation_attempt_map.contains_key(&candidate_id));
    assert!(!fixture.processor.pending_reject.contains_key(&candidate_id));
    assert!(!fixture.processor.rejected.contains(&candidate_id));
}

// ============================================================================
// Optimistic Validation Tests
// ============================================================================

/// Helper: create a non-empty RawCandidate for check_validation tests.
/// The block data is synthetic (not a valid TON block) — sufficient for
/// testing the gating logic in check_validation which doesn't parse it.
fn make_test_non_empty_candidate(
    candidate_id: RawCandidateId,
    parent_id: Option<RawCandidateId>,
    nodes: &[SessionNode],
) -> crate::block::RawCandidate {
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        candidate_id.slot.value() + 1,
        UInt256::rand(),
        UInt256::rand(),
    );
    let creator = nodes[0].public_key.clone();
    let block = crate::block::BlockCandidate {
        id: block_id,
        collated_file_hash: UInt256::rand(),
        data: vec![1, 2, 3],
        collated_data: vec![4, 5, 6],
        creator,
    };
    crate::block::RawCandidate::new(candidate_id, parent_id, ValidatorIndex::new(0), block, vec![])
}

/// Helper: create an empty RawCandidate with a specific referenced BlockIdExt.
fn make_test_empty_candidate_with_block(
    candidate_id: RawCandidateId,
    parent_id: RawCandidateId,
    referenced_block: BlockIdExt,
) -> crate::block::RawCandidate {
    crate::block::RawCandidate::new_empty(
        candidate_id,
        parent_id,
        ValidatorIndex::new(0),
        referenced_block,
        vec![],
    )
}

/// Helper: insert a minimal ReceivedCandidate into the processor's received_candidates map.
fn insert_received_candidate(
    processor: &mut SessionProcessor,
    candidate_id: &RawCandidateId,
    block_id: BlockIdExt,
    is_empty: bool,
    parent_id: Option<RawCandidateId>,
) {
    insert_received_candidate_with_gen_utime_ms(
        processor,
        candidate_id,
        block_id,
        is_empty,
        parent_id,
        None,
    );
}

fn insert_received_candidate_with_gen_utime_ms(
    processor: &mut SessionProcessor,
    candidate_id: &RawCandidateId,
    block_id: BlockIdExt,
    is_empty: bool,
    parent_id: Option<RawCandidateId>,
    gen_utime_ms: Option<u64>,
) {
    processor.received_candidates.insert(
        candidate_id.clone(),
        ReceivedCandidate {
            slot: candidate_id.slot,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: candidate_id.hash.clone(),
            candidate_hash_data_bytes: Vec::new(),
            block_id: block_id.clone(),
            root_hash: block_id.root_hash.clone(),
            file_hash: block_id.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                Vec::new(),
            ),
            gen_utime_ms,
            receive_time: SystemTime::now(),
            is_empty,
            parent_id,
        },
    );
}

/// Helper: insert a PendingValidation into the processor.
fn insert_pending_validation(
    processor: &mut SessionProcessor,
    candidate_id: &RawCandidateId,
    raw_candidate: crate::block::RawCandidate,
    time: SystemTime,
) {
    processor.pending_validations.insert(
        candidate_id.clone(),
        PendingValidation {
            raw_candidate,
            slot: candidate_id.slot,
            receive_time: time,
            source_idx: ValidatorIndex::new(0),
        },
    );
}

/// Helper: drive the FSM to notarize a slot by storing a notarization certificate.
fn notarize_slot(fixture: &mut TestFixture, slot: SlotIndex, block_hash: &UInt256) {
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2]),
    ];
    let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    fixture
        .processor
        .simplex_state
        .set_notarize_certificate(&fixture.description, slot, block_hash, cert)
        .expect("set_notarize_certificate failed");
}

/// Helper: drive the FSM to skip-cert a slot.
fn skip_slot(fixture: &mut TestFixture, slot: SlotIndex) {
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2]),
    ];
    let vote = crate::simplex_state::SkipVote { slot };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    fixture
        .processor
        .simplex_state
        .set_skip_certificate(&fixture.description, slot, cert)
        .expect("set_skip_certificate failed");
}

#[test]
fn test_check_validation_forwards_candidate_with_notarized_parent() {
    let mut fixture = TestFixture::new_shard(4);

    let parent_slot = SlotIndex::new(0);
    let parent_hash = UInt256::rand();
    let parent_id = RawCandidateId { slot: parent_slot, hash: parent_hash.clone() };

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // Before notarization the candidate must stay blocked by WaitForParent.
    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate must not be forwarded when parent is not notarized"
    );

    // Notarize the parent slot
    notarize_slot(&mut fixture, parent_slot, &parent_hash);
    assert!(
        fixture.processor.simplex_state.has_notarized_block(parent_slot),
        "parent slot must be notarized after set_notarize_certificate"
    );

    // After notarization: check_validation should forward the candidate
    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "candidate must be forwarded to validation when parent is notarized"
    );
}

#[test]
fn test_check_validation_blocks_candidate_with_non_notarized_parent() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // Parent is NOT notarized — candidate must stay in pending_validations.
    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate must NOT be forwarded when parent slot is not notarized"
    );
    assert!(
        fixture.processor.pending_validations.contains_key(&child_id),
        "candidate must remain in pending_validations"
    );
}

#[test]
fn test_check_validation_forwards_genesis_candidate_without_parent() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Genesis (no parent) should always be forwarded
    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&candidate_id),
        "genesis candidate (no parent) must be forwarded unconditionally"
    );
}

#[test]
fn test_check_validation_auto_approves_empty_blocks() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };

    let parent_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());

    insert_received_candidate(
        &mut fixture.processor,
        &parent_id,
        parent_block_id.clone(),
        false,
        None,
    );

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let raw_candidate = make_test_empty_candidate_with_block(
        child_id.clone(),
        parent_id.clone(),
        parent_block_id.clone(),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &child_id,
        parent_block_id,
        true,
        Some(parent_id.clone()),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // WaitForParent parity: parent must be notarized before candidate is eligible.
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    // Empty blocks with a matching referenced block should be auto-approved.
    // C++ block-validator.cpp accepts when block == event->state->as_normal().
    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_validations.contains_key(&child_id),
        "empty block must be approved when referenced block matches parent normal tip"
    );
}

#[test]
fn test_empty_block_accepted_when_referenced_block_matches_parent() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };

    let parent_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());

    insert_received_candidate(
        &mut fixture.processor,
        &parent_id,
        parent_block_id.clone(),
        false,
        None,
    );

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let raw_candidate = make_test_empty_candidate_with_block(
        child_id.clone(),
        parent_id.clone(),
        parent_block_id.clone(),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &child_id,
        parent_block_id,
        true,
        Some(parent_id.clone()),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // WaitForParent parity: parent must be notarized before candidate is eligible.
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_validations.contains_key(&child_id),
        "empty block must be approved when referenced block matches parent normal tip"
    );
    assert!(
        fixture.processor.approved.contains_key(&child_id),
        "empty block must appear in approved set after matching reference check"
    );
}

#[test]
fn test_empty_block_rejected_when_referenced_block_differs() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };

    let parent_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());

    insert_received_candidate(&mut fixture.processor, &parent_id, parent_block_id, false, None);

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let wrong_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 99, UInt256::rand(), UInt256::rand());

    let raw_candidate = make_test_empty_candidate_with_block(
        child_id.clone(),
        parent_id.clone(),
        wrong_block_id.clone(),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &child_id,
        wrong_block_id,
        true,
        Some(parent_id.clone()),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // WaitForParent parity: parent must be notarized before candidate is eligible.
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    // C++ block-validator.cpp rejects empty candidates whose referenced block
    // does not match event->state->as_normal(). Rust must do the same.
    fixture.processor.check_validation();
    assert!(
        fixture.processor.rejected.contains(&child_id),
        "empty block must be rejected when referenced block differs from parent normal tip"
    );
    assert!(
        !fixture.processor.approved.contains_key(&child_id),
        "rejected empty block must not appear in approved set"
    );
}

#[test]
fn test_check_validation_skips_already_approved_candidates() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Pre-mark as approved (value is (validity_start_time, signature_payload))
    let dummy_payload = consensus_common::ConsensusCommonFactory::create_block_payload(vec![]);
    fixture.processor.approved.insert(candidate_id.clone(), (time, dummy_payload));

    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&candidate_id),
        "already-approved candidate must not be re-submitted for validation"
    );
}

#[test]
fn test_check_validation_skips_already_rejected_candidates() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Pre-mark as rejected
    fixture.processor.rejected.insert(candidate_id.clone());

    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&candidate_id),
        "already-rejected candidate must not be re-submitted for validation"
    );
}

#[test]
fn test_check_validation_skips_pending_approve_candidates() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Pre-mark as pending_approve (already being validated)
    fixture.processor.pending_approve.insert(candidate_id.clone());

    // check_validation should not double-insert
    fixture.processor.check_validation();

    // validation_attempt_map should NOT have been updated (try_approve_block not called)
    assert!(
        !fixture.processor.validation_attempt_map.contains_key(&candidate_id),
        "candidate already in pending_approve must not be re-submitted"
    );
}

#[test]
fn test_check_validation_chains_notarized_parent_to_descendant() {
    // Validates that a candidate chain A -> B works:
    // - A has no parent (genesis), B has A as parent
    // - Only B's parent slot needs to be notarized for B to pass
    let mut fixture = TestFixture::new_shard(4);

    let slot_a = SlotIndex::new(0);
    let hash_a = UInt256::rand();
    let id_a = RawCandidateId { slot: slot_a, hash: hash_a.clone() };

    let slot_b = SlotIndex::new(1);
    let id_b = RawCandidateId { slot: slot_b, hash: UInt256::rand() };

    // Insert both candidates
    let raw_a = make_test_non_empty_candidate(id_a.clone(), None, &fixture.nodes);
    let raw_b = make_test_non_empty_candidate(id_b.clone(), Some(id_a.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &id_a, raw_a, time);
    insert_pending_validation(&mut fixture.processor, &id_b, raw_b, time);

    // First check_validation:
    // - A (genesis) should pass
    // - B should wait until parent slot A is notarized
    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&id_a),
        "genesis candidate A must be forwarded"
    );
    assert!(
        !fixture.processor.pending_approve.contains(&id_b),
        "candidate B must wait until parent slot is notarized"
    );

    // Notarize slot 0 (A's slot)
    notarize_slot(&mut fixture, slot_a, &hash_a);

    // Second check_validation: B should now pass
    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&id_b),
        "candidate B must be forwarded after parent slot is notarized"
    );
}

#[test]
fn test_check_validation_wait_for_parent_requires_gap_skip_certificates() {
    let mut fixture = TestFixture::new_shard(4);

    // Candidate at slot 3 references parent at slot 0.
    // Readiness requires:
    // - parent slot 0 notarized
    // - skip certificates for slots 1 and 2
    let parent_slot = SlotIndex::new(0);
    let parent_hash = UInt256::rand();
    let parent_id = RawCandidateId { slot: parent_slot, hash: parent_hash.clone() };

    let child_slot = SlotIndex::new(3);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // Parent notarized, but gap skips missing: still blocked.
    notarize_slot(&mut fixture, parent_slot, &parent_hash);
    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate must be blocked until all intermediate slots are skip-certified"
    );

    // Add skip cert for slot 1 only: still blocked.
    skip_slot(&mut fixture, SlotIndex::new(1));
    fixture.processor.check_validation();
    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate must remain blocked when gap skip coverage is partial"
    );

    // Add skip cert for slot 2 -> now eligible.
    skip_slot(&mut fixture, SlotIndex::new(2));
    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "candidate must be forwarded once parent and full skip-gap readiness are satisfied"
    );
}

#[test]
fn test_check_validation_wait_for_parent_rejects_parent_hash_mismatch() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let notarized_parent_hash = UInt256::from([0x21; 32]);
    let mismatched_parent_hash = UInt256::from([0x99; 32]);
    let parent_id = RawCandidateId { slot: parent_slot, hash: mismatched_parent_hash };

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // Notarize the slot with a different hash than candidate.parent_id.
    notarize_slot(&mut fixture, parent_slot, &notarized_parent_hash);
    fixture.processor.check_validation();

    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate must not be forwarded when parent hash mismatches notarized block"
    );
    assert!(
        fixture.processor.pending_validations.contains_key(&child_id),
        "candidate should remain pending while WaitForParent readiness cannot be proven"
    );
}

#[test]
fn test_on_candidate_received_non_empty_does_not_wait_for_unresolved_ancestor_chain() {
    let mut fixture = TestFixture::new(4);

    let grandparent_id =
        RawCandidateId { slot: SlotIndex::new(0), hash: UInt256::from([0x71; 32]) };
    let parent_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0x72; 32]) };
    let parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x73; 32]),
        UInt256::from([0x74; 32]),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &parent_id,
        parent_block_id,
        false,
        Some(grandparent_id),
    );

    let (leader_source, child_id, broadcast) = make_signed_block_broadcast_with_parent(
        &fixture,
        2,
        vec![0x55, 0x66, 0x77],
        Some(parent_id.clone()),
    );

    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert!(
        fixture.processor.pending_validations.contains_key(&child_id),
        "non-empty candidate must be admitted immediately even if only ancestor metadata is missing"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "non-empty admission must not trigger ancestor prefetch requests"
    );

    let received = fixture
        .processor
        .received_candidates
        .get(&child_id)
        .expect("child candidate must be stored");
    assert!(
        received.parent_id.as_ref() == Some(&parent_id),
        "stored child candidate must preserve the explicit parent link"
    );
}

#[test]
fn test_on_candidate_received_empty_waits_in_pending_validation_and_requests_missing_parent() {
    let mut fixture = TestFixture::new(4);

    let parent_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0x81; 32]) };
    let referenced_block = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        20,
        UInt256::from([0x82; 32]),
        UInt256::from([0x83; 32]),
    );
    let (leader_source, child_id, broadcast) = make_signed_empty_block_broadcast_with_parent(
        &fixture,
        2,
        parent_id.clone(),
        referenced_block,
    );

    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert!(
        fixture.processor.pending_validations.contains_key(&child_id),
        "empty candidate must enter pending_validations immediately after ingress"
    );
    assert!(
        fixture.processor.requested_candidates.is_empty(),
        "ingress must not prefetch parent metadata before WaitForParent is satisfied"
    );

    notarize_slot(&mut fixture, parent_id.slot, &parent_id.hash);
    fixture.processor.check_validation();

    assert!(
        fixture.processor.pending_validations.contains_key(&child_id),
        "empty candidate must remain pending while parent metadata is still missing"
    );
    assert!(
        fixture.processor.requested_candidates.contains_key(&parent_id),
        "validation path must request the missing parent metadata on demand"
    );
    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "empty candidate must not enter pending_approve until the parent normal tip is resolvable"
    );
    assert!(
        !fixture.processor.rejected.contains(&child_id),
        "missing parent metadata must defer empty approval instead of rejecting it"
    );
}

// ============================================================================
// Health check anomaly tests
// ============================================================================

/// Reset health alert timestamps to a deterministic base time so that
/// cooldown checks work correctly in tests (HealthAlertState is initialized
/// with real SystemTime::now() during SessionProcessor::new).
fn reset_health_alert_time(processor: &mut SessionProcessor, base: SystemTime) {
    let s = &mut processor.health_alert_state;
    s.last_progress_warn = base;
    s.last_activity_warn = base;
    s.last_cert_fail_warn = base;
    s.last_skip_ratio_warn = base;
    s.last_standstill_warn = base;
    s.last_finalization_speed_warn = base;
    s.last_finalization_nonzero_at = base;
    s.last_candidate_giveup_warn = base;
    s.prev_votes_in_notarize = 0;
    s.prev_votes_in_finalize = 0;
    s.prev_votes_in_skip = 0;
}

#[test]
fn test_health_check_cert_verify_fail_anomaly() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    // Initially no warnings
    fixture.processor.run_health_checks();

    // Simulate cert verify failure
    fixture.processor.cert_verify_fails_total = 1;

    // Advance past cooldown (default 30s)
    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();

    assert_eq!(fixture.processor.health_alert_state.prev_cert_verify_fails, 1);
}

#[test]
fn test_health_check_standstill_anomaly() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    fixture.processor.run_health_checks();

    fixture.processor.receiver_health_counters.standstill_triggers.store(3, Ordering::Relaxed);

    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();

    assert_eq!(fixture.processor.health_alert_state.prev_standstill_triggers, 3);
}

#[test]
fn test_health_check_candidate_giveup_anomaly() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    fixture.processor.run_health_checks();

    fixture.processor.receiver_health_counters.candidate_giveups.store(2, Ordering::Relaxed);

    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();

    assert_eq!(fixture.processor.health_alert_state.prev_candidate_giveups, 2);
}

#[test]
fn test_health_check_skip_vote_dominance_anomaly() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    fixture.processor.run_health_checks();

    // High skip-dominant window (delta-based): should trigger skip ratio anomaly.
    fixture.processor.votes_in_skip_total = 24;
    fixture.processor.votes_in_notarize_total = 2;
    fixture.processor.votes_in_finalize_total = 1;

    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();

    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_skip, 24);
    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_notarize, 2);
    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_finalize, 1);
    assert_eq!(
        fixture.processor.health_alert_state.last_skip_ratio_warn,
        base_time + Duration::from_secs(31)
    );
}

#[test]
fn test_health_check_skip_vote_dominance_ignores_sparse_zero_denominator() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    fixture.processor.run_health_checks();

    // One stream is absent in the current window, but the overall progress vote
    // stream is still healthy enough that skip traffic is not dominant.
    fixture.processor.votes_in_skip_total = 3;
    fixture.processor.votes_in_notarize_total = 0;
    fixture.processor.votes_in_finalize_total = 10;

    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();

    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_skip, 3);
    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_notarize, 0);
    assert_eq!(fixture.processor.health_alert_state.prev_votes_in_finalize, 10);
    assert_eq!(fixture.processor.health_alert_state.last_skip_ratio_warn, base_time);
}

#[test]
fn test_health_check_cooldown_prevents_spam() {
    let mut fixture = TestFixture::new(4);

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    reset_health_alert_time(&mut fixture.processor, base_time);

    // First cert verify fail — advance past initial cooldown
    fixture.processor.cert_verify_fails_total = 1;
    fixture.processor.set_time(base_time + Duration::from_secs(31));
    fixture.processor.run_health_checks();
    assert_eq!(fixture.processor.health_alert_state.prev_cert_verify_fails, 1);

    // More cert verify fails, but within cooldown since last warn at t+31
    fixture.processor.cert_verify_fails_total = 5;
    fixture.processor.set_time(base_time + Duration::from_secs(35));
    fixture.processor.run_health_checks();
    assert_eq!(fixture.processor.health_alert_state.prev_cert_verify_fails, 1);

    // After cooldown passes (31 + 30 = 61, use 62)
    fixture.processor.set_time(base_time + Duration::from_secs(62));
    fixture.processor.run_health_checks();
    assert_eq!(fixture.processor.health_alert_state.prev_cert_verify_fails, 5);
}

#[test]
fn test_health_check_configurable_cooldown() {
    let nodes = create_test_validators(4);
    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::masterchain();

    let mut opts = SessionOptions::default();
    opts.health_alert_cooldown = Duration::from_secs(10);

    let description = Arc::new(
        crate::session_description::SessionDescription::new(
            &opts,
            SessionId::default(),
            1,
            &nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None,
        )
        .unwrap(),
    );

    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = Arc::new(MockListener);
    let listener_weak = Arc::downgrade(&listener);
    let task_queue = TestTaskQueue::new();
    let callback_queue: CallbackTaskQueuePtr = Arc::new(TestCallbackQueue);
    let overlay_manager = consensus_common::ConsensusCommonFactory::create_dummy_overlay_manager();
    let receiver = MockReceiver::new();
    let db = MockDb::new();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let health_counters = Arc::new(crate::receiver::ReceiverHealthCounters::new());

    let mut processor = SessionProcessor::new(
        description,
        listener_weak,
        task_queue as TaskQueuePtr,
        callback_queue,
        overlay_manager,
        receiver as crate::receiver::ReceiverPtr,
        stop_flag,
        db,
        0,
        health_counters,
    )
    .unwrap();

    assert_eq!(processor.health_alert_state.cooldown, Duration::from_secs(10));

    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    processor.set_time(base_time);
    reset_health_alert_time(&mut processor, base_time);

    processor.cert_verify_fails_total = 1;

    // After 11s (> 10s cooldown), should trigger
    processor.set_time(base_time + Duration::from_secs(11));
    processor.run_health_checks();
    assert_eq!(processor.health_alert_state.prev_cert_verify_fails, 1);
}

// ============================================================================
// Collation pacing tests
// ============================================================================

#[test]
fn test_update_collation_pacing_sets_earliest_time() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    assert!(fixture.processor.earliest_collation_time.is_none());

    fixture.processor.update_collation_pacing();

    let target_rate = fixture.description.opts().target_rate;
    assert_eq!(fixture.processor.earliest_collation_time, Some(base_time + target_rate),);
}

#[test]
fn test_update_collation_pacing_advances_on_repeated_calls() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let target_rate = fixture.description.opts().target_rate;

    fixture.processor.update_collation_pacing();
    assert_eq!(fixture.processor.earliest_collation_time, Some(base_time + target_rate));

    // Advance time by half the target_rate and pace again
    fixture.advance_time(target_rate / 2);
    fixture.processor.update_collation_pacing();
    assert_eq!(
        fixture.processor.earliest_collation_time,
        Some(base_time + target_rate / 2 + target_rate),
    );
}

#[test]
fn test_compute_collation_start_time_caps_parent_delay_to_target_rate() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let parent_id = RawCandidateId { slot: SlotIndex::new(0), hash: UInt256::rand() };
    let parent_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());
    let parent_gen_utime_ms =
        base_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64 + 500;

    insert_received_candidate_with_gen_utime_ms(
        &mut fixture.processor,
        &parent_id,
        parent_block_id,
        false,
        None,
        Some(parent_gen_utime_ms),
    );

    let parent_info =
        crate::block::CandidateParentInfo { slot: parent_id.slot, hash: parent_id.hash.clone() };
    let start_time = fixture.processor.compute_collation_start_time(Some(&parent_info));

    assert_eq!(
        start_time,
        base_time + fixture.description.opts().target_rate,
        "collation start time should not be delayed by more than one target_rate from now"
    );
}

#[test]
fn test_check_collation_blocks_before_earliest_time() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let gate_time = base_time + Duration::from_millis(500);
    fixture.processor.earliest_collation_time = Some(gate_time);

    fixture.processor.reset_next_awake_time();

    fixture.processor.check_collation();

    assert_eq!(
        fixture.processor.get_next_awake_time(),
        base_time + MAX_AWAKE_TIMEOUT,
        "with the temporary 10ms fallback poll, next_awake_time stays at the earlier fallback wake"
    );
}

#[test]
fn test_check_collation_proceeds_after_pacing_expires() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let gate_time = base_time + Duration::from_millis(500);
    fixture.processor.earliest_collation_time = Some(gate_time);

    fixture.advance_time(Duration::from_millis(600));

    fixture.processor.reset_next_awake_time();
    fixture.processor.check_collation();

    assert_ne!(
        fixture.processor.get_next_awake_time(),
        gate_time,
        "check_collation should proceed past pacing gate after time expires"
    );
}

#[test]
fn test_check_collation_pacing_gate_is_idempotent() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let gate_time = base_time + Duration::from_millis(500);
    fixture.processor.earliest_collation_time = Some(gate_time);

    fixture.processor.reset_next_awake_time();
    fixture.processor.check_collation();
    assert_eq!(fixture.processor.get_next_awake_time(), base_time + MAX_AWAKE_TIMEOUT);

    assert_eq!(fixture.processor.earliest_collation_time, Some(gate_time));

    fixture.processor.reset_next_awake_time();
    fixture.processor.check_collation();
    assert_eq!(fixture.processor.get_next_awake_time(), base_time + MAX_AWAKE_TIMEOUT);
}

#[test]
fn test_collation_starts_metric_tracks_async_generation_requests() {
    let opts = SessionOptions { slots_per_leader_window: 1, ..Default::default() };
    let mut fixture = TestFixture::new_with_opts(4, opts);

    fixture.processor.check_collation();

    assert_eq!(metrics_counter(&fixture.processor, "simplex_collation_starts"), 1);
    assert_eq!(metrics_counter(&fixture.processor, "simplex_collates.total"), 1);
}

#[test]
fn test_collation_starts_metric_tracks_precollated_fast_path() {
    let opts = SessionOptions { slots_per_leader_window: 1, ..Default::default() };
    let mut fixture = TestFixture::new_with_opts(4, opts);
    let slot = SlotIndex::new(0);
    let request = AsyncRequestImpl::new(11, false, fixture.description.get_time());
    let candidate = make_local_collated_candidate(&fixture, 1, 0x41);

    fixture
        .processor
        .precollated_blocks
        .insert(slot, PrecollatedBlock { request, candidate: Some(candidate), parent: None });

    fixture.processor.check_collation();

    assert_eq!(metrics_counter(&fixture.processor, "simplex_collation_starts"), 1);
    assert_eq!(metrics_counter(&fixture.processor, "simplex_collates.total"), 0);
    assert_eq!(metrics_counter(&fixture.processor, "simplex_collates_precollated.success"), 1);
}

// ============================================================================
// Candidate Query Fallback Tests (C++ parity: CandidateResolver DB fallback)
// ============================================================================

#[test]
fn test_candidate_query_fallback_cache_hit() {
    let mut fixture = TestFixture::new(4);
    let slot = SlotIndex::new(5);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };

    let fake_candidate_bytes = vec![0xCA, 0xFE, 0xBA, 0xBE];
    fixture.processor.candidate_data_cache.insert(candidate_id, fake_candidate_bytes.clone());

    let (tx, rx) = channel();
    let callback: crate::QueryResponseCallback = Box::new(move |result| {
        tx.send(result).unwrap();
    });

    fixture.processor.handle_candidate_query_fallback(slot, block_hash, true, false, callback);

    let result = rx.recv_timeout(Duration::from_secs(2)).expect("callback not called");
    let payload = result.expect("response should be Ok");
    let response_bytes = payload.data();

    assert!(!response_bytes.is_empty(), "response should contain serialized CandidateAndCert");

    let deserialized = deserialize_boxed(response_bytes)
        .expect("should deserialize response")
        .downcast::<CandidateAndCert>()
        .expect("should be CandidateAndCert");

    let inner = match deserialized {
        CandidateAndCert::Consensus_Simplex_CandidateAndCert(inner) => inner,
    };

    assert_eq!(
        &inner.candidate[..],
        &fake_candidate_bytes[..],
        "candidate bytes should match the cached data"
    );
}

#[test]
fn test_candidate_query_fallback_miss_returns_empty() {
    let mut fixture = TestFixture::new(4);
    let slot = SlotIndex::new(99);
    let block_hash = UInt256::rand();

    let (tx, rx) = channel();
    let callback: crate::QueryResponseCallback = Box::new(move |result| {
        tx.send(result).unwrap();
    });

    fixture.processor.handle_candidate_query_fallback(slot, block_hash, true, false, callback);

    let result = rx.recv_timeout(Duration::from_secs(5)).expect("callback not called");
    let payload = result.expect("response should be Ok even for empty");
    let response_bytes = payload.data();

    let deserialized = deserialize_boxed(response_bytes)
        .expect("should deserialize response")
        .downcast::<CandidateAndCert>()
        .expect("should be CandidateAndCert");

    let inner = match deserialized {
        CandidateAndCert::Consensus_Simplex_CandidateAndCert(inner) => inner,
    };

    assert!(inner.candidate.is_empty(), "candidate bytes should be empty when not found");
}

fn make_signed_block_broadcast_with_parent(
    fixture: &TestFixture,
    slot: u32,
    block_data: Vec<u8>,
    parent_id: Option<RawCandidateId>,
) -> (u32, RawCandidateId, CandidateData) {
    let collated_data: Vec<u8> = vec![];
    let root_hash = UInt256::from_slice(&sha256_digest(&block_data));
    let shard = fixture.processor.description.get_shard().clone();

    let block_id = BlockIdExt {
        shard_id: shard,
        seq_no: slot,
        root_hash: root_hash.clone(),
        file_hash: root_hash,
    };
    let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data));
    let data_bytes = block_data;
    let collated_data_bytes = collated_data;

    let candidate_hash = crate::utils::compute_candidate_id_hash_u32(
        slot,
        Some(&block_id),
        Some(&collated_file_hash),
        parent_id.as_ref().map(|p| (p.slot.value(), &p.hash)),
    );

    let session_id = fixture.processor.session_id().clone();
    let leader_idx = fixture.processor.description.get_leader(SlotIndex::new(slot));
    let leader_key = fixture.processor.description.get_source_public_key(leader_idx);
    let signature =
        crate::utils::sign_candidate_u32(&session_id, slot, &candidate_hash, leader_key)
            .expect("signing failed");

    let candidate_id = RawCandidateId { slot: SlotIndex::new(slot), hash: candidate_hash };
    let block = crate::block::BlockCandidate {
        id: block_id,
        collated_file_hash,
        data: data_bytes,
        collated_data: collated_data_bytes,
        creator: leader_key.clone(),
    };
    let raw_candidate = crate::block::RawCandidate::new(
        candidate_id.clone(),
        parent_id,
        leader_idx,
        block,
        signature,
    );
    let serialized = raw_candidate.serialize(false).expect("serialize RawCandidate");
    let msg = deserialize_boxed(&serialized).expect("deserialize CandidateData");
    let broadcast = msg.downcast::<CandidateData>().expect("downcast CandidateData");

    (leader_idx.value(), candidate_id, broadcast)
}

fn make_signed_block_broadcast(
    fixture: &TestFixture,
    slot: u32,
    block_data: Vec<u8>,
) -> (u32, RawCandidateId, CandidateData) {
    make_signed_block_broadcast_with_parent(fixture, slot, block_data, None)
}

fn make_signed_empty_block_broadcast_with_parent(
    fixture: &TestFixture,
    slot: u32,
    parent_id: RawCandidateId,
    referenced_block: BlockIdExt,
) -> (u32, RawCandidateId, CandidateData) {
    let candidate_hash = crate::utils::compute_candidate_id_hash_empty(
        &referenced_block,
        (parent_id.slot, &parent_id.hash),
    );
    let session_id = fixture.processor.session_id().clone();
    let leader_idx = fixture.processor.description.get_leader(SlotIndex::new(slot));
    let leader_key = fixture.processor.description.get_source_public_key(leader_idx);
    let signature =
        crate::utils::sign_candidate_u32(&session_id, slot, &candidate_hash, leader_key)
            .expect("signing failed");

    let candidate_id = RawCandidateId { slot: SlotIndex::new(slot), hash: candidate_hash };
    let raw_candidate = crate::block::RawCandidate::new_empty(
        candidate_id.clone(),
        parent_id,
        leader_idx,
        referenced_block,
        signature,
    );
    let serialized = raw_candidate.serialize(false).expect("serialize RawCandidate");
    let msg = deserialize_boxed(&serialized).expect("deserialize CandidateData");
    let broadcast = msg.downcast::<CandidateData>().expect("downcast CandidateData");

    (leader_idx.value(), candidate_id, broadcast)
}

fn make_local_collated_candidate(
    fixture: &TestFixture,
    seqno: u32,
    tag: u8,
) -> Arc<crate::ValidatorBlockCandidate> {
    let block_boc = make_test_boc(&[tag], BocFlags::all());
    let collated_boc = make_test_boc(&[tag.wrapping_add(1)], BocFlags::Crc32);
    let root_hash = UInt256::from_slice(&sha256_digest(&block_boc));
    let block_id = BlockIdExt {
        shard_id: fixture.processor.description.get_shard().clone(),
        seq_no: seqno,
        root_hash: root_hash.clone(),
        file_hash: root_hash,
    };
    let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_boc));
    let self_idx = fixture.description.get_self_idx().value() as usize;

    Arc::new(crate::ValidatorBlockCandidate {
        public_key: fixture.nodes[self_idx].public_key.clone(),
        id: block_id,
        collated_file_hash,
        data: consensus_common::ConsensusCommonFactory::create_block_payload(block_boc),
        collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(collated_boc),
    })
}

#[test]
fn test_candidate_data_cache_populated_on_candidate_received() {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Debug).try_init();
    let mut fixture = TestFixture::new(4);

    // Use slot 0 so that validator 0 (local) is the slot leader
    let slot = 0u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![1u8, 2, 3, 4, 5]);

    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "cache should be empty before on_candidate_received"
    );

    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert!(
        fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "cache should be populated after on_candidate_received"
    );

    assert!(
        fixture.processor.received_candidates.contains_key(&candidate_id),
        "received_candidates should also have the candidate"
    );
}

#[test]
fn test_candidate_ingress_metrics_split_broadcast_and_query() {
    let mut fixture = TestFixture::new(4);

    let (leader_source, _, broadcast) =
        make_signed_block_broadcast(&fixture, 1, vec![1u8, 2, 3, 4, 5]);
    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert_eq!(metrics_counter(&fixture.processor, "simplex_candidate_received_broadcast"), 1);
    assert_eq!(metrics_counter(&fixture.processor, "simplex_candidate_received_query"), 0);

    let (_, _, query_candidate) = make_signed_block_broadcast(&fixture, 2, vec![9u8, 8, 7, 6]);
    fixture.processor.on_candidate_received(
        ValidatorIndex::new(3).value(),
        query_candidate,
        Some(Vec::new()),
    );

    assert_eq!(metrics_counter(&fixture.processor, "simplex_candidate_received_broadcast"), 1);
    assert_eq!(metrics_counter(&fixture.processor, "simplex_candidate_received_query"), 1);
}

#[test]
fn test_old_slot_broadcast_is_dropped_without_persistence_side_effects() {
    let mut fixture = TestFixture::new(4);
    fixture.processor.simplex_state.set_first_non_finalized_slot(SlotIndex::new(1));

    let slot = 0u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![9u8, 8, 7, 6]);

    fixture.processor.on_candidate_received(leader_source, broadcast, None /* broadcast */);

    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "old-slot broadcast must not populate candidate_data_cache"
    );
    assert!(
        !fixture.processor.received_candidates.contains_key(&candidate_id),
        "old-slot broadcast must not populate received_candidates"
    );
    assert!(
        !fixture.processor.seen_broadcast_candidates.contains_key(&SlotIndex::new(slot)),
        "old-slot broadcast should be dropped before broadcast dedup state is updated"
    );
}

#[test]
fn test_candidate_precheck_keeps_simple_addition_rule() {
    let mut fixture = TestFixture::new(4);

    let slot = fixture.processor.simplex_state.max_acceptable_slot().value().saturating_add(1);
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![7u8, 7, 7, 7]);

    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "candidate above the simple-addition bound must be dropped before caching"
    );
    assert_eq!(
        metrics_counter(&fixture.processor, "simplex_candidate_precheck_drop_future_slot"),
        1
    );
}

#[test]
fn test_candidate_precheck_progress_gap_uses_progress_cursor() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    let skip_cert = build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, 0, &[0, 1, 2]);
    fixture.processor.on_certificate(1, skip_cert);
    fixture.drain_receiver_actions();

    assert_eq!(fixture.processor.simplex_state.get_first_non_finalized_slot(), SlotIndex::new(0));
    assert_eq!(fixture.processor.simplex_state.get_first_non_progressed_slot(), SlotIndex::new(1));

    let slot = fixture.processor.simplex_state.max_acceptable_slot().value();
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![8u8, 8, 8, 8]);

    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    assert!(
        fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "candidate at the progress-anchored boundary should survive precheck even when finalization lags"
    );
}

#[test]
fn test_register_candidate_for_validation_keeps_slot_behind_progress_cursor_until_finalized() {
    let mut fixture = TestFixture::new(4);
    fixture.drain_receiver_actions();

    let skip_cert = build_skip_certificate_tl(&SessionId::default(), &fixture.nodes, 0, &[0, 1, 2]);
    fixture.processor.on_certificate(1, skip_cert);
    fixture.drain_receiver_actions();

    assert_eq!(fixture.processor.simplex_state.get_first_non_finalized_slot(), SlotIndex::new(0));
    assert_eq!(fixture.processor.simplex_state.get_first_non_progressed_slot(), SlotIndex::new(1));

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };
    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let receive_time = fixture.description.get_time();

    fixture.processor.register_candidate_for_validation(
        raw_candidate,
        slot,
        fixture.description.get_self_idx(),
        receive_time,
    );

    assert!(
        fixture.processor.pending_validations.contains_key(&candidate_id),
        "candidate behind first_non_progressed_slot must stay eligible until the slot is finalized"
    );
}

#[test]
fn test_conflicting_second_broadcast_same_slot_is_dropped_by_precheck() {
    let mut fixture = TestFixture::new(4);
    let slot = 0u32;

    let (leader_source, first_id, first_broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![1u8, 1, 1, 1]);
    fixture.processor.on_candidate_received(
        leader_source,
        first_broadcast,
        None, /* broadcast */
    );

    assert!(
        fixture.processor.received_candidates.contains_key(&first_id),
        "first broadcast candidate should be accepted"
    );

    let (_, second_id, second_broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![2u8, 2, 2, 2]);
    assert_ne!(first_id, second_id, "test setup must create conflicting candidate ids");

    fixture.processor.on_candidate_received(
        leader_source,
        second_broadcast,
        None, /* broadcast */
    );

    assert!(
        !fixture.processor.received_candidates.contains_key(&second_id),
        "conflicting second broadcast for same slot must be dropped"
    );
    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&second_id),
        "conflicting second broadcast must not be persisted in candidate_data_cache"
    );
    assert_eq!(
        fixture.processor.seen_broadcast_candidates.get(&SlotIndex::new(slot)).cloned(),
        Some(first_id),
        "slot dedup state should keep first accepted broadcast candidate id"
    );
}

#[test]
fn test_broadcast_from_unexpected_sender_is_dropped_by_precheck() {
    let mut fixture = TestFixture::new(4);
    let slot = 0u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![3u8, 4, 5, 6]);
    let unexpected_sender = (leader_source + 1) % 4;

    fixture.processor.on_candidate_received(
        unexpected_sender,
        broadcast,
        None, /* broadcast */
    );

    assert!(
        !fixture.processor.received_candidates.contains_key(&candidate_id),
        "broadcast from non-leader sender must be dropped by precheck"
    );
    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "broadcast from non-leader sender must not be persisted"
    );
}

// ============================================================================
// Protocol Parity Tests (stub body, partial merge, finalized seqno)
// ============================================================================

#[test]
fn test_has_real_candidate_body_returns_false_for_stub() {
    let mut fixture = TestFixture::new(4);
    let slot = SlotIndex::new(10);
    let hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: hash.clone() };

    // No entry => false
    assert!(!fixture.processor.has_real_candidate_body(&candidate_id));

    // Insert a finalized-boundary stub (empty candidate_hash_data_bytes)
    fixture.processor.received_candidates.insert(
        candidate_id.clone(),
        ReceivedCandidate {
            slot,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash.clone(),
            candidate_hash_data_bytes: Vec::new(), // stub marker
            block_id: BlockIdExt::default(),
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                Vec::new(),
            ),
            gen_utime_ms: None,
            receive_time: fixture.processor.now(),
            is_empty: false,
            parent_id: None,
        },
    );

    // Stub => false
    assert!(
        !fixture.processor.has_real_candidate_body(&candidate_id),
        "finalized-boundary stub must NOT count as real body"
    );

    // Overwrite with real data
    fixture
        .processor
        .received_candidates
        .get_mut(&candidate_id)
        .unwrap()
        .candidate_hash_data_bytes = vec![1, 2, 3];

    // Now should be true
    assert!(
        fixture.processor.has_real_candidate_body(&candidate_id),
        "entry with non-empty candidate_hash_data_bytes must count as real body"
    );
}

#[test]
fn test_candidate_query_fallback_returns_notar_only_when_body_missing() {
    let mut fixture = TestFixture::new(4);
    let slot = SlotIndex::new(99);
    let block_hash = UInt256::rand();

    let (tx, rx) = channel();
    let callback: crate::QueryResponseCallback = Box::new(move |result| {
        tx.send(result).unwrap();
    });

    // No candidate in cache or DB => should return empty/empty
    fixture.processor.handle_candidate_query_fallback(slot, block_hash, false, true, callback);

    let result = rx.recv().unwrap();
    assert!(result.is_ok(), "should return Ok even when nothing found");
}

#[test]
fn test_initial_mc_tracking_seeds_from_initial_block_seqno() {
    let fixture = TestFixture::new(4);

    assert_eq!(fixture.processor.last_mc_finalized_seqno, Some(0));
    assert_eq!(fixture.processor.last_consensus_finalized_seqno, Some(0));
    assert_eq!(fixture.processor.accepted_normal_head_seqno, 0);
    assert!(
        fixture.processor.accepted_normal_head_block_id.is_none(),
        "exact accepted head should be unknown until manager/recovery/finalization supplies a block id"
    );
}

#[test]
fn test_set_mc_finalized_block_couples_consensus_finalized_seqno() {
    let mut fixture = TestFixture::new(4);

    // Initially 0
    assert_eq!(fixture.processor.last_mc_finalized_seqno, Some(0));
    assert_eq!(fixture.processor.last_consensus_finalized_seqno, Some(0));

    // Set MC-registered top to seqno 42 for this session shard.
    let mc_registered_top =
        BlockIdExt::with_params(ShardIdent::masterchain(), 42, UInt256::rand(), UInt256::rand());
    fixture.processor.set_mc_finalized_block(mc_registered_top.clone());

    // C++ parity: consensus finalized should advance to max(mc, consensus)
    assert_eq!(fixture.processor.last_mc_finalized_seqno, Some(42));
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(42),
        "set_mc_finalized_block should couple to last_consensus_finalized_seqno via max()"
    );
    assert_eq!(fixture.processor.accepted_normal_head_seqno, 42);
    assert_eq!(fixture.processor.accepted_normal_head_block_id.as_ref(), Some(&mc_registered_top));

    // Set consensus finalized higher via direct field (simulating a finalization)
    fixture.processor.last_consensus_finalized_seqno = Some(100);

    // Set MC finalized lower => should NOT decrease consensus
    let older_top =
        BlockIdExt::with_params(ShardIdent::masterchain(), 50, UInt256::rand(), UInt256::rand());
    fixture.processor.set_mc_finalized_block(older_top);
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(100),
        "set_mc_finalized_block must not decrease last_consensus_finalized_seqno"
    );

    // Monotonic MC seqno: out-of-order MC event with lower seqno must not regress
    fixture.processor.last_mc_finalized_seqno = Some(200);
    let out_of_order_top =
        BlockIdExt::with_params(ShardIdent::masterchain(), 150, UInt256::rand(), UInt256::rand());
    fixture.processor.set_mc_finalized_block(out_of_order_top);
    assert_eq!(
        fixture.processor.last_mc_finalized_seqno,
        Some(200),
        "set_mc_finalized_block must keep last_mc_finalized_seqno monotonic"
    );
}

#[test]
fn test_set_mc_finalized_block_ignores_mismatched_shard() {
    let mut fixture = TestFixture::new(4);
    assert!(fixture.description.get_shard().is_masterchain());

    fixture.processor.last_mc_finalized_seqno = Some(123);
    fixture.processor.last_consensus_finalized_seqno = Some(123);

    let shard_block = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
        777,
        UInt256::rand(),
        UInt256::rand(),
    );
    fixture.processor.set_mc_finalized_block(shard_block);

    assert_eq!(
        fixture.processor.last_mc_finalized_seqno,
        Some(123),
        "mismatched shard update must be ignored"
    );
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(123),
        "consensus finalized must not change on mismatched shard update"
    );
}

#[test]
fn test_set_mc_finalized_block_wakes_processor() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    fixture.processor.next_awake_time = base_time + Duration::from_secs(60);

    let mc_registered_top =
        BlockIdExt::with_params(ShardIdent::masterchain(), 42, UInt256::rand(), UInt256::rand());
    fixture.processor.set_mc_finalized_block(mc_registered_top);

    assert_eq!(
        fixture.processor.get_next_awake_time(),
        base_time,
        "MC finalization should wake the FSM immediately"
    );
}

#[test]
fn test_check_validation_does_not_wait_for_mc_applied_head_before_submitting() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let child_slot = SlotIndex::new(1);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let parent_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());

    insert_received_candidate(
        &mut fixture.processor,
        &parent_id,
        parent_block_id.clone(),
        false,
        None,
    );

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "candidate should be submitted without a SessionProcessor wait on the accepted MC head"
    );
    assert!(
        !fixture.processor.rejected.contains(&child_id),
        "SessionProcessor should not reject while validator-side MC stale protection owns this check"
    );
}

#[test]
fn test_check_all_releases_validation_retry_before_revalidation() {
    let mut fixture = TestFixture::new_shard(4);

    let parent_slot = SlotIndex::new(0);
    let child_slot = SlotIndex::new(1);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let parent_block_id = BlockIdExt::with_params(
        fixture.description.get_shard().clone(),
        1,
        UInt256::rand(),
        UInt256::rand(),
    );

    insert_received_candidate(&mut fixture.processor, &parent_id, parent_block_id, false, None);

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    fixture.processor.pending_approve.insert(child_id.clone());
    let child_id_for_release = child_id.clone();
    fixture.processor.post_delayed_action(time, move |processor| {
        processor.pending_approve.remove(&child_id_for_release);
    });

    fixture.processor.check_all();

    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "retry gate release should happen before check_validation so the candidate is resubmitted in the same pass"
    );
}

#[test]
fn test_check_validation_waits_for_min_block_interval() {
    let opts =
        SessionOptions { min_block_interval: Duration::from_millis(500), ..Default::default() };
    let mut fixture = TestFixture::new_with_shard_and_local_idx(
        4,
        0,
        ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
        opts,
    );
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    fixture.processor.next_awake_time = base_time + Duration::from_secs(60);

    let parent_slot = SlotIndex::new(0);
    let child_slot = SlotIndex::new(1);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let parent_block_id = BlockIdExt::with_params(
        fixture.description.get_shard().clone(),
        1,
        UInt256::rand(),
        UInt256::rand(),
    );
    let parent_gen_utime_ms =
        base_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64;

    insert_received_candidate_with_gen_utime_ms(
        &mut fixture.processor,
        &parent_id,
        parent_block_id,
        false,
        None,
        Some(parent_gen_utime_ms),
    );

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, base_time);
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    fixture.processor.check_validation();

    assert!(
        !fixture.processor.pending_approve.contains(&child_id),
        "candidate should not be submitted before the min_block_interval elapses"
    );
    assert_eq!(
        fixture.processor.get_next_awake_time(),
        base_time + Duration::from_millis(500),
        "validation should arm a wake for the min_block_interval deadline"
    );

    fixture.advance_time(Duration::from_millis(600));
    fixture.processor.next_awake_time = base_time + Duration::from_secs(60);
    fixture.processor.check_validation();

    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "candidate should be submitted after the min_block_interval elapses"
    );
}

#[test]
fn test_check_validation_does_not_reject_mc_candidate_with_wrong_exact_parent_head() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let child_slot = SlotIndex::new(1);
    let parent_id = RawCandidateId { slot: parent_slot, hash: UInt256::rand() };
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };
    let accepted_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x11; 32]),
        UInt256::from([0x12; 32]),
    );
    let different_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x21; 32]),
        UInt256::from([0x22; 32]),
    );

    fixture.processor.set_mc_finalized_block(accepted_block_id);
    insert_received_candidate(
        &mut fixture.processor,
        &parent_id,
        different_parent_block_id,
        false,
        None,
    );

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);
    notarize_slot(&mut fixture, parent_slot, &parent_id.hash);

    fixture.processor.check_validation();
    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "candidate should be submitted without exact-head rejection in SessionProcessor"
    );
    assert!(
        !fixture.processor.rejected.contains(&child_id),
        "SessionProcessor should not reject on exact-head mismatch; validator-side MC fork prevention covers this"
    );
}

#[test]
fn test_resolve_parent_normal_tip_walks_empty_parent_chain() {
    let mut fixture = TestFixture::new(4);

    let root_id = RawCandidateId { slot: SlotIndex::new(0), hash: UInt256::from([0x01; 32]) };
    let empty_a_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0x02; 32]) };
    let empty_b_id = RawCandidateId { slot: SlotIndex::new(2), hash: UInt256::from([0x03; 32]) };
    let child_id = RawCandidateId { slot: SlotIndex::new(3), hash: UInt256::from([0x04; 32]) };
    let root_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x31; 32]),
        UInt256::from([0x32; 32]),
    );

    insert_received_candidate(&mut fixture.processor, &root_id, root_block_id.clone(), false, None);
    insert_received_candidate(
        &mut fixture.processor,
        &empty_a_id,
        root_block_id.clone(),
        true,
        Some(root_id.clone()),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &empty_b_id,
        root_block_id.clone(),
        true,
        Some(empty_a_id.clone()),
    );

    let raw_candidate =
        make_test_empty_candidate_with_block(child_id, empty_b_id, root_block_id.clone());
    assert_eq!(fixture.processor.resolve_parent_normal_tip(&raw_candidate), Some(root_block_id));
}

#[test]
fn test_recovery_seed_received_candidates_preserves_persisted_empty_records() {
    let mut fixture = TestFixture::new(4);

    let c1 = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0xA1; 32]) };
    let c2 = RawCandidateId { slot: SlotIndex::new(2), hash: UInt256::from([0xA2; 32]) };
    let c3 = RawCandidateId { slot: SlotIndex::new(3), hash: UInt256::from([0xA3; 32]) };
    let b1 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0xB1; 32]),
        UInt256::from([0xB2; 32]),
    );
    let b2 = b1.clone();
    let b3 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0xB3; 32]),
        UInt256::from([0xB4; 32]),
    );

    fixture.processor.recovery_seed_received_candidates(&[
        FinalizedBlockRecord {
            candidate_id: c1.clone(),
            block_id: b1.clone(),
            parent: None,
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: c2.clone(),
            block_id: b2,
            parent: Some(c1.clone()),
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: c3.clone(),
            block_id: b3.clone(),
            parent: Some(c2.clone()),
            is_final: true,
        },
    ]);

    let root = fixture.processor.received_candidates.get(&c1).expect("root record");
    assert!(!root.is_empty);

    let empty = fixture.processor.received_candidates.get(&c2).expect("empty record");
    assert!(empty.is_empty, "persisted empty MC record must remain marked empty on recovery");
    assert_eq!(empty.parent_id.as_ref(), Some(&c1));
    assert_eq!(empty.block_id, b1);

    let child = fixture.processor.received_candidates.get(&c3).expect("child record");
    assert!(!child.is_empty);
    assert_eq!(child.parent_id.as_ref(), Some(&c2));
    assert_eq!(child.block_id, b3);
}

// ============================================================================
// Foreign Certificate Relay Regression Tests (C++ parity)
// ============================================================================

/// Verify that a notarization certificate ingested via set_notarize_certificate
/// (foreign path) triggers relay to peers.
#[test]
fn test_foreign_notarization_cert_is_relayed() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(3);
    let block_hash = UInt256::rand();

    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![10]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![11]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![12]),
    ];
    let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    let event = crate::simplex_state::NotarizationReachedEvent {
        slot,
        block_hash: block_hash.clone(),
        certificate: cert,
    };

    fixture.processor.handle_notarization_reached(event);

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SendCertificate { .. })),
        "foreign notarization cert must be relayed (C++ parity: handle_saved_certificate)"
    );
}

/// Verify that a finalization certificate ingested via set_finalize_certificate
/// (foreign path) triggers relay to peers.
#[test]
fn test_foreign_finalization_cert_is_relayed() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(5);
    let block_hash = UInt256::rand();

    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![20]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![21]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![22]),
    ];
    let vote = crate::simplex_state::FinalizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });

    let event = crate::simplex_state::FinalizationReachedEvent {
        slot,
        block_hash: block_hash.clone(),
        certificate: cert,
    };

    fixture.processor.handle_finalization_reached(event);

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(|a| matches!(a, ReceiverAction::SendCertificate { .. })),
        "foreign finalization cert must be relayed (C++ parity: handle_saved_certificate)"
    );
}

#[test]
fn test_foreign_vote_is_not_rebroadcast() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(2);
    let block_hash = UInt256::from([0xAB; 32]);
    let vote = crate::simplex_state::Vote::Notarize(crate::simplex_state::NotarizeVote {
        slot,
        block_hash,
    });
    let tl_vote = crate::utils::sign_vote(
        &vote,
        fixture.description.get_session_id(),
        &fixture.nodes[1].public_key,
    )
    .expect("failed to sign foreign vote");
    let raw_vote: crate::RawVoteData =
        consensus_common::serialize_tl_boxed_object!(&tl_vote).into();

    fixture.processor.on_vote(1, tl_vote, raw_vote);

    let actions = fixture.drain_receiver_actions();
    assert!(
        !actions.iter().any(|a| matches!(a, ReceiverAction::SendVote { .. })),
        "foreign votes must not be re-broadcast"
    );
}

#[test]
fn test_recovery_drain_startup_events_drops_certificate_relay_events() {
    let mut fixture = TestFixture::new(4);

    let slot = crate::block::SlotIndex::new(3);
    let block_hash = UInt256::from([0xCD; 32]);
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![1]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![2]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![3]),
    ];
    let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };
    let cert = Arc::new(crate::certificate::Certificate { vote, signatures });
    let stored = fixture
        .processor
        .simplex_state
        .set_notarize_certificate(&fixture.description, slot, &block_hash, cert)
        .expect("set_notarize_certificate should succeed");
    assert!(stored, "notar cert should be stored before startup drain");

    let kept_votes =
        crate::startup_recovery::SessionStartupRecoveryListener::recovery_drain_startup_events(
            &mut fixture.processor,
        );
    assert!(
        kept_votes.is_empty(),
        "this setup should produce only certificate events, no startup votes"
    );

    fixture.processor.check_all();
    let actions = fixture.drain_receiver_actions();
    assert!(
        !actions.iter().any(|a| matches!(a, ReceiverAction::SendCertificate { .. })),
        "drained startup certificate events must not be re-broadcast on first normal tick"
    );
}

// ============================================================================
// Finalized journal cleanup tests
// ============================================================================

/// Verify that `cleanup_old_candidates` prunes stale finalized-journal entries for old slots
/// without treating them as session errors.
#[test]
fn test_journal_cleanup_removes_stale_entries() {
    let mut fixture = TestFixture::new(4);

    let old_slot = SlotIndex::new(5);
    let current_slot = SlotIndex::new(20);
    let old_hash = UInt256::rand();
    let current_hash = UInt256::rand();

    let old_id = RawCandidateId { slot: old_slot, hash: old_hash.clone() };
    let current_id = RawCandidateId { slot: current_slot, hash: current_hash.clone() };

    let dummy_cert: crate::certificate::FinalCertPtr = Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote { slot: old_slot, block_hash: old_hash.clone() },
        signatures: Vec::new(),
    });

    let dummy_cert2: crate::certificate::FinalCertPtr = Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote {
            slot: current_slot,
            block_hash: current_hash.clone(),
        },
        signatures: Vec::new(),
    });

    let now = fixture.description.get_time();

    fixture.processor.finalized_pending_body.insert(
        old_id.clone(),
        FinalizedEntry {
            event: BlockFinalizedEvent {
                slot: old_slot,
                block_hash: old_hash,
                block_id: None,
                certificate: dummy_cert,
            },
            finalized_at: now - Duration::from_secs(60),
        },
    );

    fixture.processor.finalized_pending_body.insert(
        current_id.clone(),
        FinalizedEntry {
            event: BlockFinalizedEvent {
                slot: current_slot,
                block_hash: current_hash,
                block_id: None,
                certificate: dummy_cert2,
            },
            finalized_at: now,
        },
    );

    assert_eq!(fixture.processor.finalized_pending_body.len(), 2);

    let errors_before =
        fixture.processor.session_errors_count.load(std::sync::atomic::Ordering::Relaxed);

    // Cleanup slots < 10 — old_slot(5) should be removed, current_slot(20) kept.
    fixture.processor.cleanup_old_candidates(SlotIndex::new(10));

    assert_eq!(fixture.processor.finalized_pending_body.len(), 1);
    assert!(!fixture.processor.finalized_pending_body.contains_key(&old_id));
    assert!(fixture.processor.finalized_pending_body.contains_key(&current_id));

    let errors_after =
        fixture.processor.session_errors_count.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        errors_after, errors_before,
        "stale finalized-journal entries should be pruned without incrementing error count"
    );
}

// ============================================================================
// on_block_finalized / maybe_apply_finalized_state tests
// ============================================================================

/// Finalized block with body present must advance `finalized_head_seqno`
/// and `last_consensus_finalized_seqno`.
#[test]
fn test_finalized_with_body_advances_committed_seqno() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    assert_eq!(fixture.processor.finalized_head_seqno, Some(0));
    assert_eq!(fixture.processor.last_consensus_finalized_seqno, Some(0));

    let slot = 5u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![1, 2, 3, 4]);
    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    let received = fixture
        .processor
        .received_candidates
        .get(&candidate_id)
        .expect("candidate must be present")
        .clone();

    let event = BlockFinalizedEvent {
        slot: candidate_id.slot,
        block_hash: candidate_id.hash.clone(),
        block_id: Some(received.block_id.clone()),
        certificate: make_test_final_cert(candidate_id.slot, candidate_id.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event);

    assert_eq!(
        fixture.processor.finalized_head_seqno,
        Some(received.block_id.seq_no()),
        "finalized_head_seqno must advance to finalized block seqno"
    );
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(received.block_id.seq_no()),
        "last_consensus_finalized_seqno must advance to finalized block seqno"
    );
    assert!(
        fixture.processor.finalized_blocks.contains(&candidate_id),
        "candidate must be in finalized_blocks set"
    );

    let events = recording.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, ListenerEvent::Finalized { .. })),
        "on_block_finalized callback must be emitted"
    );
}

/// Out-of-order finalization: higher seqno finalized first, then lower seqno.
/// Both must advance cursors monotonically (never decrease).
#[test]
fn test_finalized_out_of_order_seqno_advances_monotonically() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    let slot_high = 10u32;
    let (leader_high, id_high, broadcast_high) =
        make_signed_block_broadcast(&fixture, slot_high, vec![10, 20, 30]);
    fixture.processor.on_candidate_received(leader_high, broadcast_high, None);

    let received_high = fixture.processor.received_candidates.get(&id_high).unwrap().clone();

    let event_high = BlockFinalizedEvent {
        slot: id_high.slot,
        block_hash: id_high.hash.clone(),
        block_id: Some(received_high.block_id.clone()),
        certificate: make_test_final_cert(id_high.slot, id_high.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event_high);

    let seqno_after_high = fixture.processor.finalized_head_seqno;
    let consensus_after_high = fixture.processor.last_consensus_finalized_seqno;

    let slot_low = 3u32;
    let (leader_low, id_low, broadcast_low) =
        make_signed_block_broadcast(&fixture, slot_low, vec![40, 50, 60]);
    fixture.processor.on_candidate_received(leader_low, broadcast_low, None);

    let received_low = fixture.processor.received_candidates.get(&id_low).unwrap().clone();

    let event_low = BlockFinalizedEvent {
        slot: id_low.slot,
        block_hash: id_low.hash.clone(),
        block_id: Some(received_low.block_id.clone()),
        certificate: make_test_final_cert(id_low.slot, id_low.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event_low);

    assert!(
        fixture.processor.finalized_head_seqno >= seqno_after_high,
        "finalized_head_seqno must not decrease after lower-seqno finalization: \
        before={:?} after={:?}",
        seqno_after_high,
        fixture.processor.finalized_head_seqno,
    );
    assert!(
        fixture.processor.last_consensus_finalized_seqno >= consensus_after_high,
        "last_consensus_finalized_seqno must not decrease after lower-seqno finalization: \
        before={:?} after={:?}",
        consensus_after_high,
        fixture.processor.last_consensus_finalized_seqno,
    );
    assert!(fixture.processor.finalized_blocks.contains(&id_high));
    assert!(fixture.processor.finalized_blocks.contains(&id_low));

    let events = recording.drain_events();
    let finalized_count =
        events.iter().filter(|e| matches!(e, ListenerEvent::Finalized { .. })).count();
    assert_eq!(finalized_count, 2, "both blocks must emit on_block_finalized callbacks");
}

/// Duplicate finalization for the same candidate must be deduplicated:
/// second call must not re-emit callback and must not modify cursors.
#[test]
fn test_finalized_duplicate_is_idempotent() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    let slot = 7u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![0xDE, 0xAD]);
    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    let received = fixture.processor.received_candidates.get(&candidate_id).unwrap().clone();

    let event = BlockFinalizedEvent {
        slot: candidate_id.slot,
        block_hash: candidate_id.hash.clone(),
        block_id: Some(received.block_id.clone()),
        certificate: make_test_final_cert(candidate_id.slot, candidate_id.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event.clone());

    let seqno_after_first = fixture.processor.finalized_head_seqno;
    let consensus_after_first = fixture.processor.last_consensus_finalized_seqno;
    let _ = recording.drain_events();

    fixture.processor.handle_block_finalized(event);

    assert_eq!(fixture.processor.finalized_head_seqno, seqno_after_first);
    assert_eq!(fixture.processor.last_consensus_finalized_seqno, consensus_after_first);

    let events_after_dup = recording.drain_events();
    assert!(
        !events_after_dup.iter().any(|e| matches!(e, ListenerEvent::Finalized { .. })),
        "duplicate finalization must not re-emit on_block_finalized callback"
    );
}

/// Empty-block finalization must NOT advance `finalized_head_seqno` (empty blocks
/// keep parent seqno), but the candidate must still be recorded in `finalized_blocks`.
#[test]
fn test_finalized_empty_block_does_not_advance_seqno() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    fixture.processor.finalized_head_seqno = Some(50);
    fixture.processor.finalized_head_block_id = Some(BlockIdExt::with_params(
        ShardIdent::masterchain(),
        50,
        UInt256::rand(),
        UInt256::rand(),
    ));

    let slot = SlotIndex::new(22);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };

    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        50, // same seqno as parent — empty block
        UInt256::rand(),
        UInt256::rand(),
    );

    fixture.processor.received_candidates.insert(
        candidate_id.clone(),
        ReceivedCandidate {
            slot,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: block_hash.clone(),
            candidate_hash_data_bytes: vec![1, 2, 3],
            block_id: block_id.clone(),
            root_hash: block_id.root_hash.clone(),
            file_hash: block_id.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xAA].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xBB].into(),
            ),
            gen_utime_ms: None,
            receive_time: fixture.description.get_time(),
            is_empty: true,
            parent_id: None,
        },
    );

    let event = BlockFinalizedEvent {
        slot,
        block_hash: block_hash.clone(),
        block_id: Some(block_id),
        certificate: make_test_final_cert(slot, block_hash),
    };
    fixture.processor.handle_block_finalized(event);

    assert_eq!(
        fixture.processor.finalized_head_seqno,
        Some(50),
        "finalized_head_seqno must not advance for empty-block finalization"
    );
    assert!(
        fixture.processor.finalized_blocks.contains(&candidate_id),
        "empty-block candidate must be recorded in finalized_blocks"
    );
}

/// Multiple finalized blocks with bodies arriving in reverse seqno order:
/// `finalized_head_seqno` must reflect the highest seqno seen.
#[test]
fn test_finalized_reverse_order_keeps_highest_seqno() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let recording = RecordingListener::new();
    let listener: Arc<dyn consensus_common::SessionListener + Send + Sync> = recording.clone();
    fixture.processor.listener = Arc::downgrade(&listener);

    let slots: Vec<u32> = vec![20, 15, 10, 5];
    let mut highest_seqno = 0u32;

    for &slot in &slots {
        let (leader, cid, bcast) =
            make_signed_block_broadcast(&fixture, slot, vec![slot as u8, 0xFF]);
        fixture.processor.on_candidate_received(leader, bcast, None);

        let received = fixture.processor.received_candidates.get(&cid).unwrap().clone();
        let seqno = received.block_id.seq_no();
        if seqno > highest_seqno {
            highest_seqno = seqno;
        }

        let event = BlockFinalizedEvent {
            slot: cid.slot,
            block_hash: cid.hash.clone(),
            block_id: Some(received.block_id.clone()),
            certificate: make_test_final_cert(cid.slot, cid.hash.clone()),
        };
        fixture.processor.handle_block_finalized(event);
    }

    assert_eq!(
        fixture.processor.finalized_head_seqno,
        Some(highest_seqno),
        "finalized_head_seqno must be the highest seqno across all out-of-order finalizations"
    );
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(highest_seqno),
        "last_consensus_finalized_seqno must be the highest seqno across all out-of-order finalizations"
    );

    let events = recording.drain_events();
    let finalized_count =
        events.iter().filter(|e| matches!(e, ListenerEvent::Finalized { .. })).count();
    assert_eq!(finalized_count, slots.len(), "all blocks must emit on_block_finalized");
}

/// Verify that `finalized_pending_body` is cleaned up when
/// `maybe_apply_finalized_state` runs (body present at finalization time).
#[test]
fn test_finalized_clears_journal_entry_on_apply() {
    let mut opts = SessionOptions::default();
    opts.use_callback_thread = false;
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let slot = 12u32;
    let (leader_source, candidate_id, broadcast) =
        make_signed_block_broadcast(&fixture, slot, vec![0xCA, 0xFE]);
    fixture.processor.on_candidate_received(leader_source, broadcast, None);

    let received = fixture.processor.received_candidates.get(&candidate_id).unwrap().clone();

    let event = BlockFinalizedEvent {
        slot: candidate_id.slot,
        block_hash: candidate_id.hash.clone(),
        block_id: Some(received.block_id.clone()),
        certificate: make_test_final_cert(candidate_id.slot, candidate_id.hash.clone()),
    };
    fixture.processor.handle_block_finalized(event);

    assert!(
        fixture.processor.finalized_pending_body.is_empty(),
        "journal must be empty after finalization with body present"
    );
    assert!(
        fixture.processor.finalized_blocks.contains(&candidate_id),
        "candidate must be in finalized_blocks set"
    );
}

/// Verify that the correct processing order (validated candidates BEFORE
/// FSM timeouts) allows a candidate to be notarized even when the clock
/// has advanced past the skip timeout.
///
/// Without Fix A (processing-order), calling `simplex_state.check_all()`
/// first would fire the `first_block_timeout` and skip-vote the slot
/// before the already-validated candidate is fed to the FSM.
#[test]
fn test_process_validated_candidates_before_fsm_timeout() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: candidate_hash.clone() };

    // Create a non-empty candidate for slot 0 with no parent (genesis).
    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();

    // Insert pending validation so candidate_decision_ok_internal can find it.
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Simulate validation success: push the resolved candidate into the queue.
    fixture.processor.candidate_decision_ok_internal(candidate_id.clone(), slot, time);
    assert!(
        !fixture.processor.validated_candidates.is_empty(),
        "candidate must be in the validated_candidates queue"
    );

    // Advance time past first_block_timeout + target_rate (defaults: 3s + 1s = 4s).
    fixture.advance_time(Duration::from_secs(5));

    // --- Correct order (Fix A): feed candidates, THEN run FSM timeouts ---
    fixture.processor.process_validated_candidates();
    fixture.processor.simplex_state.check_all(&fixture.description);

    // Collect FSM events produced by the two calls above.
    let mut has_notarize = false;
    while let Some(event) = fixture.processor.simplex_state.pull_event() {
        if let crate::simplex_state::SimplexEvent::BroadcastVote(
            crate::simplex_state::Vote::Notarize(ref v),
        ) = event
        {
            if v.slot == slot {
                has_notarize = true;
            }
        }
    }

    // The critical invariant: the candidate was notarized because
    // process_validated_candidates() ran before simplex_state.check_all().
    assert!(
        has_notarize,
        "slot 0 must be notarized (candidate was fed to FSM before timeout evaluation)"
    );
    // In C++ mode a skip vote may follow
    // the notarize vote after the timeout fires -- that is harmless and
    // expected.  The key property is that the notarize vote was emitted.
}

/// Verify that the `log::warn!` for "drop because block is already
/// finalized" only fires when `cand_seqno <= finalized_seqno`, i.e. the
/// candidate is actually dropped.  When `cand_seqno > finalized_seqno`
/// the candidate must proceed to `validated_candidates`.
#[test]
fn test_candidate_decision_ok_does_not_drop_when_cand_seqno_greater_than_finalized() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: candidate_hash.clone() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Set finalized_head_seqno to a value BELOW the candidate's seqno.
    // make_test_non_empty_candidate uses slot.value()+1 as seq_no, so for
    // slot 0 the candidate seqno = 1.  Setting finalized_head to 0 means
    // cand_seqno (1) > finalized_seqno (0) → candidate must NOT be dropped.
    fixture.processor.finalized_head_seqno = Some(0);

    // Call the public wrapper which contains the guard.
    let validity_start = time;
    fixture.processor.candidate_decision_ok(slot, candidate_id.clone(), validity_start, time);

    // The candidate must have been pushed to validated_candidates (not dropped).
    assert!(
        !fixture.processor.validated_candidates.is_empty(),
        "candidate with cand_seqno > committed_seqno must NOT be dropped"
    );
    // And it must have been removed from pending_validations (consumed, not leaked).
    assert!(
        !fixture.processor.pending_validations.contains_key(&candidate_id),
        "pending_validations entry must be consumed"
    );
}

#[test]
fn test_generated_candidate_validation_missed_metric_increments_on_final_rejection() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };
    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    fixture.processor.track_generated_candidate_for_validation(candidate_id.clone());
    fixture.processor.mark_generated_candidate_validation_started(&candidate_id);
    fixture.processor.candidate_decision_fail(
        slot,
        candidate_id.clone(),
        error!("validator rejected"),
    );

    assert_eq!(
        metrics_counter(&fixture.processor, "simplex_generated_candidate_validation_missed"),
        1
    );
    assert!(
        !fixture.processor.generated_candidates_waiting_validation.contains_key(&candidate_id),
        "tracking entry must be removed after the miss is recorded"
    );
    assert!(
        fixture.processor.rejected.contains(&candidate_id),
        "final rejection should still mark the candidate as rejected"
    );
}

// ============================================================================
// Candidate Chaining Tests (C++ parity)
// ============================================================================

/// Test that local_chain_head and generated_parent_cache start empty.
#[test]
fn test_local_chain_head_initial_state() {
    let fixture = TestFixture::new(4);
    assert!(fixture.processor.local_chain_head.is_none());
    assert!(fixture.processor.generated_parent_cache.is_empty());
}

/// Test that invalidate_local_chain_head clears both the chain head and cache.
#[test]
fn test_invalidate_local_chain_head_clears_state() {
    let mut fixture = TestFixture::new(4);

    let hash = UInt256::from([0xAA; 32]);
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0xBB; 32]),
        UInt256::from([0xCC; 32]),
    );
    let parent_info =
        crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: hash.clone() };
    let raw_id = RawCandidateId { slot: SlotIndex::new(0), hash: hash.clone() };

    fixture.processor.local_chain_head = Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: SlotIndex::new(0),
        parent_info,
        block_id: block_id.clone(),
        gen_utime_ms: None,
    });
    fixture.processor.generated_parent_cache.insert(raw_id.clone(), block_id);

    assert!(fixture.processor.local_chain_head.is_some());
    assert!(!fixture.processor.generated_parent_cache.is_empty());

    fixture.processor.invalidate_local_chain_head();

    assert!(fixture.processor.local_chain_head.is_none());
    assert!(fixture.processor.generated_parent_cache.is_empty());
}

/// Test that resolve_parent_block_id finds parents in generated_parent_cache
/// even before the async on_candidate_received self-loop populates received_candidates.
#[test]
fn test_resolve_parent_from_generated_cache() {
    let mut fixture = TestFixture::new(4);

    let hash = UInt256::from([0xDD; 32]);
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0xEE; 32]),
        UInt256::from([0xFF; 32]),
    );
    let parent_info =
        crate::block::CandidateParentInfo { slot: SlotIndex::new(5), hash: hash.clone() };
    let raw_id = RawCandidateId { slot: SlotIndex::new(5), hash: hash.clone() };

    // Not in received_candidates yet
    assert!(fixture.processor.resolve_parent_block_id(&parent_info).is_none());

    // Seed the generated_parent_cache (as generated_block would)
    fixture.processor.generated_parent_cache.insert(raw_id, block_id.clone());

    // Now resolvable
    let resolved = fixture.processor.resolve_parent_block_id(&parent_info);
    assert_eq!(resolved, Some(block_id));
}

/// Test that reset_precollations clears the local chain head.
#[test]
fn test_reset_precollations_clears_chain_head() {
    let mut fixture = TestFixture::new(4);

    let hash = UInt256::from([0x11; 32]);
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x22; 32]),
        UInt256::from([0x33; 32]),
    );
    let parent_info =
        crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: hash.clone() };

    fixture.processor.local_chain_head = Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: SlotIndex::new(0),
        parent_info,
        block_id,
        gen_utime_ms: None,
    });

    fixture.processor.reset_precollations();

    assert!(fixture.processor.local_chain_head.is_none());
    assert!(fixture.processor.generated_parent_cache.is_empty());
}

/// Test that multi-slot window options produce correct precollation depth.
#[test]
fn test_slots_per_leader_window_precollation_depth() {
    // Single-slot window: no precollation
    let opts1 = SessionOptions { slots_per_leader_window: 1, ..Default::default() };
    assert_eq!(opts1.slots_per_leader_window.saturating_sub(1), 0);

    // 4-slot window: up to 3 precollated
    let opts4 = SessionOptions { slots_per_leader_window: 4, ..Default::default() };
    assert_eq!(opts4.slots_per_leader_window.saturating_sub(1), 3);

    // 8-slot window: up to 7 precollated
    let opts8 = SessionOptions { slots_per_leader_window: 8, ..Default::default() };
    assert_eq!(opts8.slots_per_leader_window.saturating_sub(1), 7);
}

/// Test that creating a SessionProcessor with multi-slot window succeeds.
#[test]
fn test_multi_slot_window_session_creation() {
    let opts = SessionOptions { slots_per_leader_window: 4, ..Default::default() };
    let fixture = TestFixture::new_with_opts(4, opts);
    assert_eq!(fixture.description.opts().slots_per_leader_window, 4);
    assert!(fixture.processor.local_chain_head.is_none());
}

#[test]
fn test_on_collation_complete_publishes_future_slot_in_current_window() {
    // C++ parity: for multi-slot windows, candidates generated for future slots in
    // the same active leader window are published immediately.
    let opts = SessionOptions { slots_per_leader_window: 4, ..Default::default() };
    let mut fixture = TestFixture::new_with_opts(4, opts);

    let slot = SlotIndex::new(1);
    assert_eq!(
        fixture.processor.simplex_state.get_first_non_progressed_slot(),
        SlotIndex::new(0),
        "precondition: progress cursor starts at slot 0"
    );
    assert_eq!(
        fixture.description.get_window_idx(slot),
        fixture.processor.simplex_state.get_current_leader_window_idx(),
        "precondition: slot 1 is in current leader window"
    );

    let request_id = 77;
    let request = AsyncRequestImpl::new(request_id, false, fixture.description.get_time());
    fixture
        .processor
        .precollated_blocks
        .insert(slot, PrecollatedBlock { request, candidate: None, parent: None });

    let block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 1, UInt256::rand(), UInt256::rand());
    let block_boc = make_test_boc(&[0x31], BocFlags::all());
    let collated_boc = make_test_boc(&[0x32], BocFlags::Crc32);
    let candidate = crate::ValidatorBlockCandidate {
        public_key: fixture.nodes[0].public_key.clone(),
        id: block_id,
        collated_file_hash: UInt256::from_slice(&sha256_digest(&collated_boc)),
        data: consensus_common::ConsensusCommonFactory::create_block_payload(block_boc),
        collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(collated_boc),
    };

    fixture.processor.on_collation_complete(
        slot,
        request_id,
        CollationResult::Block(Arc::new(candidate)),
    );

    let actions = fixture.drain_receiver_actions();
    assert!(
        actions.iter().any(
            |a| matches!(a, ReceiverAction::SendBlockBroadcast { slot: s, .. } if *s == slot.value())
        ),
        "future in-window candidate must be broadcast immediately (C++ parity)"
    );
    assert!(
        fixture.processor.slot_is_generated(slot),
        "future in-window slot must be marked generated after immediate publish"
    );
}

// ============================================================================
// C++ in-window collation parity tests
// ============================================================================

#[test]
fn test_precollate_same_window_slot_uses_local_chain_head_before_parent_notarization() {
    let opts = SessionOptions { slots_per_leader_window: 4, ..Default::default() };
    let probe = TestFixture::new_with_opts(4, opts.clone());
    let local_idx = probe.description.get_leader(SlotIndex::new(1)).value() as usize;
    let mut fixture = TestFixture::new_with_local_idx(4, local_idx, opts);

    let parent_hash = UInt256::from([0xB1; 32]);
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0xB2; 32]),
        UInt256::from([0xB3; 32]),
    );

    fixture.processor.local_chain_head = Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: SlotIndex::new(0),
        parent_info: crate::block::CandidateParentInfo {
            slot: SlotIndex::new(0),
            hash: parent_hash.clone(),
        },
        block_id: block_id.clone(),
        gen_utime_ms: None,
    });
    fixture.processor.generated_parent_cache.insert(
        RawCandidateId { slot: SlotIndex::new(0), hash: parent_hash.clone() },
        block_id.clone(),
    );
    fixture.processor.last_consensus_finalized_seqno = Some(block_id.seq_no);

    assert!(
        !fixture.processor.simplex_state.has_notarized_block(SlotIndex::new(0)),
        "precondition: slot 0 is not notarized yet"
    );

    fixture.processor.precollate_block(SlotIndex::new(1));

    let precollated = fixture
        .processor
        .precollated_blocks
        .get(&SlotIndex::new(1))
        .expect("same-window child slot must be precollated immediately");
    let parent = precollated.parent.as_ref().expect("precollated slot must capture parent");
    assert_eq!(parent.slot, SlotIndex::new(0));
    assert_eq!(parent.hash, parent_hash);
}

#[test]
fn test_precollate_first_slot_in_new_window_uses_fsm_available_base() {
    let opts = SessionOptions { slots_per_leader_window: 2, ..Default::default() };
    let probe = TestFixture::new_with_opts(4, opts.clone());
    let local_idx = probe.description.get_leader(SlotIndex::new(2)).value() as usize;
    let mut fixture = TestFixture::new_with_local_idx(4, local_idx, opts);

    let stale_hash = UInt256::from([0xC0; 32]);
    fixture.processor.local_chain_head = Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: SlotIndex::new(1),
        parent_info: crate::block::CandidateParentInfo {
            slot: SlotIndex::new(1),
            hash: stale_hash.clone(),
        },
        block_id: BlockIdExt::with_params(
            ShardIdent::masterchain(),
            50,
            UInt256::from([0xC1; 32]),
            UInt256::from([0xC2; 32]),
        ),
        gen_utime_ms: None,
    });

    let fsm_parent_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0xC3; 32]) };
    let fsm_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0xC4; 32]),
        UInt256::from([0xC5; 32]),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &fsm_parent_id,
        fsm_parent_block_id.clone(),
        false,
        Some(RawCandidateId { slot: SlotIndex::new(0), hash: UInt256::from([0xC6; 32]) }),
    );

    fixture.processor.simplex_state.on_block_notarized_for_test(
        &fixture.description,
        SlotIndex::new(0),
        UInt256::from([0xC7; 32]),
    );
    fixture.processor.simplex_state.on_block_notarized_for_test(
        &fixture.description,
        SlotIndex::new(1),
        fsm_parent_id.hash.clone(),
    );
    fixture.processor.last_consensus_finalized_seqno = Some(fsm_parent_block_id.seq_no);

    assert_eq!(
        fixture.processor.simplex_state.get_current_leader_window_idx(),
        fixture.description.get_window_idx(SlotIndex::new(2)),
        "precondition: slot 2 is now the first slot of the current leader window"
    );

    fixture.processor.precollate_block(SlotIndex::new(2));

    let precollated = fixture
        .processor
        .precollated_blocks
        .get(&SlotIndex::new(2))
        .expect("first slot in the new window must use the FSM base");
    let parent = precollated.parent.as_ref().expect("new-window slot must capture parent");
    assert_eq!(parent.slot, fsm_parent_id.slot);
    assert_eq!(parent.hash, fsm_parent_id.hash);
    assert_ne!(
        parent.hash, stale_hash,
        "new-window collation must ignore the stale local_chain_head and use FSM available_base"
    );
}

#[test]
fn test_check_collation_resets_stale_local_chain_head_on_window_change() {
    let opts = SessionOptions { slots_per_leader_window: 2, ..Default::default() };
    let probe = TestFixture::new_with_opts(4, opts.clone());
    let local_idx = probe.description.get_leader(SlotIndex::new(2)).value() as usize;
    let mut fixture = TestFixture::new_with_local_idx(4, local_idx, opts);

    let stale_raw_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0xD0; 32]) };
    let stale_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        51,
        UInt256::from([0xD1; 32]),
        UInt256::from([0xD2; 32]),
    );
    fixture.processor.local_chain_head = Some(LocalChainHead {
        window: WindowIndex::new(0),
        slot: stale_raw_id.slot,
        parent_info: crate::block::CandidateParentInfo {
            slot: stale_raw_id.slot,
            hash: stale_raw_id.hash.clone(),
        },
        block_id: stale_block_id.clone(),
        gen_utime_ms: None,
    });
    fixture.processor.generated_parent_cache.insert(stale_raw_id.clone(), stale_block_id);

    let fsm_parent_id = RawCandidateId { slot: SlotIndex::new(1), hash: UInt256::from([0xD3; 32]) };
    let fsm_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        12,
        UInt256::from([0xD4; 32]),
        UInt256::from([0xD5; 32]),
    );
    insert_received_candidate(
        &mut fixture.processor,
        &fsm_parent_id,
        fsm_parent_block_id.clone(),
        false,
        Some(RawCandidateId { slot: SlotIndex::new(0), hash: UInt256::from([0xD6; 32]) }),
    );

    fixture.processor.simplex_state.on_block_notarized_for_test(
        &fixture.description,
        SlotIndex::new(0),
        UInt256::from([0xD7; 32]),
    );
    fixture.processor.simplex_state.on_block_notarized_for_test(
        &fixture.description,
        SlotIndex::new(1),
        fsm_parent_id.hash.clone(),
    );
    fixture.processor.last_consensus_finalized_seqno = Some(fsm_parent_block_id.seq_no);

    fixture.processor.check_collation();

    assert!(
        fixture.processor.local_chain_head.is_none(),
        "window change must invalidate the stale local chain head"
    );
    let precollated = fixture
        .processor
        .precollated_blocks
        .get(&SlotIndex::new(2))
        .expect("after clearing stale local state, collation must fall back to the FSM base");
    let parent = precollated.parent.as_ref().expect("precollated slot must capture parent");
    assert_eq!(parent.slot, fsm_parent_id.slot);
    assert_eq!(parent.hash, fsm_parent_id.hash);
    assert!(
        fixture.processor.generated_parent_cache.is_empty(),
        "reset_precollations must also clear the old generated_parent_cache"
    );
}

/// End-to-end session processor test: first leader absent, timeout fires,
/// skip certificate arrives, window advances, second leader (us) collates.
///
/// Uses time simulation via set_time/advance_time and injects skip certificates
/// from other validators to drive the full timeout → skip → collation pipeline.
#[test]
fn test_second_leader_collates_after_timeout_skip() {
    // 4 validators, 1 slot per window → window 0 = slot 0 (leader v0), window 1 = slot 1 (leader v1).
    // We are v1 (second leader).
    let opts = SessionOptions {
        slots_per_leader_window: 1,
        first_block_timeout: Duration::from_secs(3),
        target_rate: Duration::from_millis(500),
        ..Default::default()
    };
    let mut fixture = TestFixture::new_with_local_idx(4, 1, opts.clone());

    assert_eq!(
        fixture.description.get_self_idx(),
        ValidatorIndex::new(1),
        "precondition: we must be validator 1"
    );
    assert_eq!(
        fixture.description.get_leader(SlotIndex::new(0)),
        ValidatorIndex::new(0),
        "precondition: v0 leads window 0"
    );
    assert_eq!(
        fixture.description.get_leader(SlotIndex::new(1)),
        ValidatorIndex::new(1),
        "precondition: v1 leads window 1"
    );

    // Set deterministic base time and start the session (arms timeouts).
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);
    fixture.processor.start();
    fixture.drain_receiver_actions(); // clear startup actions

    // Advance time past first_block_timeout + target_rate to trigger timeout for slot 0.
    // C++ timeout fires at: base + first_block_timeout + target_rate = 3.5s
    fixture.advance_time(opts.first_block_timeout + opts.target_rate + Duration::from_millis(1));

    // check_all: processes timeout → our node broadcasts SkipVote for slot 0.
    fixture.processor.check_all();

    let actions_after_timeout = fixture.drain_receiver_actions();
    assert!(
        actions_after_timeout.iter().any(|a| matches!(a, ReceiverAction::SendVote { .. })),
        "we must broadcast a skip vote after timeout"
    );

    // Inject skip certificate from other validators for slot 0 (quorum = 3 of 4).
    // We already voted skip, so we include ourselves (v1) plus v2, v3.
    let skip_cert = build_skip_certificate_tl(
        fixture.description.get_session_id(),
        &fixture.nodes,
        0,
        &[1, 2, 3],
    );
    fixture.processor.on_certificate(2, skip_cert);

    // check_all: processes skip cert → window advances to 1 → check_collation sees
    // we are leader for slot 1 → invoke_collation creates a collation request.
    fixture.processor.check_all();

    // The second leader must have initiated collation for slot 1.
    let slot1 = SlotIndex::new(1);
    assert!(
        fixture.processor.precollated_blocks.contains_key(&slot1)
            || fixture.processor.slot_is_pending_generate(slot1),
        "second leader (v1) must initiate collation for slot 1 after window 0 skip. \
        precollated_blocks={:?}, pending_generate={}",
        fixture.processor.precollated_blocks.keys().collect::<Vec<_>>(),
        fixture.processor.slot_is_pending_generate(slot1),
    );
}

/// Late-join scenario: node receives only a finalization certificate for a far slot
/// (no candidate body), then receives a child candidate whose parent is that
/// bodyless-but-finalized block. The node must not stall and must start validation.
///
/// Real-network flow:
/// 1. Node joins late, receives FinalCert for slot 50 (no body)
/// 2. FSM advances first_non_finalized_slot past slot 50
/// 3. The finalization is recorded in finalized_pending_body (body missing)
/// 4. Skip certificates are injected for intermediate slots (51..54)
/// 5. A new candidate for slot 55 arrives with parent = slot 50 block
/// 6. is_wait_for_parent_ready passes: parent matches get_last_finalize_certificate()
/// 7. check_validation proceeds — candidate enters validation pipeline
#[test]
fn test_late_join_finalization_cert_without_body_then_child_validates() {
    let mut fixture = TestFixture::new_shard(4);

    let far_slot = SlotIndex::new(50);
    let far_block_hash = UInt256::rand();

    // 1. Inject finalization certificate for slot 50 into the FSM (simulating
    //    a late-join node receiving a FinalCert from the network without the body).
    let signatures = vec![
        crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![10]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![11]),
        crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![12]),
    ];
    let finalize_vote =
        crate::simplex_state::FinalizeVote { slot: far_slot, block_hash: far_block_hash.clone() };
    let final_cert = Arc::new(crate::certificate::Certificate { vote: finalize_vote, signatures });

    fixture
        .processor
        .simplex_state
        .set_finalize_certificate(
            &fixture.description,
            far_slot,
            &far_block_hash,
            final_cert.clone(),
        )
        .expect("set_finalize_certificate failed");

    // Verify FSM advanced past slot 50.
    let first_nf = fixture.processor.simplex_state.get_first_non_finalized_slot();
    assert!(
        first_nf > far_slot,
        "first_non_finalized_slot must advance past the finalized slot: got {first_nf}, expected > {far_slot}"
    );

    // Process events emitted by set_finalize_certificate (BlockFinalized + FinalizationReached).
    fixture.processor.check_all();

    // The finalization should be in finalized_pending_body since no body exists.
    let far_id = RawCandidateId { slot: far_slot, hash: far_block_hash.clone() };
    assert!(
        fixture.processor.finalized_pending_body.contains_key(&far_id),
        "finalization for bodyless slot 50 must be recorded in finalized_pending_body"
    );

    // finalized_head_seqno must NOT advance (no body to materialize).
    let head_before = fixture.processor.finalized_head_seqno;

    // 2. Inject skip certificates for the gap slots 51..55 (exclusive).
    for gap in 51..55u32 {
        skip_slot(&mut fixture, SlotIndex::new(gap));
    }

    // 3. Create and insert a child candidate for slot 55, parented on slot 50.
    let child_slot = SlotIndex::new(55);
    let child_hash = UInt256::rand();
    let child_id = RawCandidateId { slot: child_slot, hash: child_hash.clone() };
    let parent_id = RawCandidateId { slot: far_slot, hash: far_block_hash.clone() };

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // 4. Drive validation: check_validation should find the child candidate eligible
    //    because its parent matches the last finalize certificate and gaps are skip-covered.
    fixture.processor.check_validation();

    assert!(
        fixture.processor.pending_approve.contains(&child_id),
        "child candidate at slot 55 must enter validation pipeline (not stall). \
        The parent at slot 50 has a finalization certificate even though the body is missing."
    );

    // finalized_head_seqno must remain unchanged (parent body still missing).
    assert_eq!(
        fixture.processor.finalized_head_seqno, head_before,
        "finalized_head_seqno must NOT advance when the parent body is still missing"
    );

    // finalized_pending_body must still contain the slot 50 entry.
    assert!(
        fixture.processor.finalized_pending_body.contains_key(&far_id),
        "finalized_pending_body must retain slot 50 entry until body arrives"
    );
}
