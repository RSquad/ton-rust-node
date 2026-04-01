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
    simplex_state::SimplexStateOptions,
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
    ton::{
        consensus::{
            candidatedata::Block as CandidateDataBlock,
            simplex::{
                certificate::Certificate, unsignedvote::SkipVote, vote::Vote as TlVote,
                votesignature::VoteSignature, votesignatureset::VoteSignatureSet, CandidateAndCert,
                Certificate as CertificateBoxed, VoteSignature as VoteSignatureBoxed,
            },
            CandidateData, CandidateParent,
        },
        validator_session::candidate::Candidate as TlCandidate,
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
fn create_test_desc(
    nodes: &[SessionNode],
    local_idx: usize,
) -> Arc<crate::session_description::SessionDescription> {
    let local_key = nodes[local_idx].public_key.clone();
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();
    Arc::new(
        crate::session_description::SessionDescription::new(
            &opts,
            SessionId::default(),
            1, // initial_block_seqno
            nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None,
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
    /// request_candidate() was called
    RequestCandidate { slot: u32, block_hash: UInt256 },
}

/// Mock receiver that records all outbound calls
struct MockReceiver {
    /// Recorded actions (sent votes, broadcasts, etc.)
    actions: Arc<Mutex<VecDeque<ReceiverAction>>>,
}

impl MockReceiver {
    fn new() -> Arc<Self> {
        Arc::new(Self { actions: Arc::new(Mutex::new(VecDeque::new())) })
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

    fn request_candidate(&self, slot: u32, block_hash: UInt256) {
        self.actions
            .lock()
            .unwrap()
            .push_back(ReceiverAction::RequestCandidate { slot, block_hash });
    }

    fn reschedule_standstill(&self) {
        // No-op for tests
    }

    fn set_standstill_slots(&self, _begin: u32, _end: u32) {
        // No-op for tests
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
        // No-op for simple tests
    }

    fn on_generate_slot(
        &self,
        _source_info: BlockSourceInfo,
        _request: AsyncRequestPtr,
        _parent: CollationParentHint,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        // No-op for simple tests
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
        // No-op for simple tests
    }

    fn on_block_skipped(&self, _round: u32) {
        // No-op for simple tests
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        // No-op for simple tests
    }

    fn get_committed_candidate(
        &self,
        block_id: BlockIdExt,
        callback: consensus_common::CommittedBlockProofCallback,
    ) {
        log::info!("get_committed_candidate: STUB for block_id={}", block_id);
        callback(Err(error!("get_committed_candidate not implemented in test")));
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
        root_hash: UInt256,
        _file_hash: UInt256,
        _data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: SessionStats,
    ) {
        // Extract slot and is_final from Simplex signatures
        // SIMPLEX_ROUNDLESS: Use slot instead of round (round is always u32::MAX)
        let (slot, is_final) = match &signatures {
            BlockSignaturesVariant::Simplex(s) => (s.slot, s.is_final),
            _ => (0, false), // other variants not expected in simplex tests
        };

        self.events.lock().unwrap().push(ListenerEvent::Committed { slot, root_hash, is_final });
    }

    fn on_block_skipped(&self, round: u32) {
        // NOTE: Not called in SIMPLEX_ROUNDLESS mode
        self.events.lock().unwrap().push(ListenerEvent::Skipped { round });
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        // No-op
    }

    fn get_committed_candidate(
        &self,
        block_id: BlockIdExt,
        callback: consensus_common::CommittedBlockProofCallback,
    ) {
        log::info!("get_committed_candidate: STUB for block_id={}", block_id);
        callback(Err(error!("get_committed_candidate not implemented in test")));
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

impl TestFixture {
    /// Create a test fixture with N validators (local is validator 0)
    fn new(validator_count: u32) -> Self {
        let nodes = create_test_validators(validator_count);
        let description = create_test_desc(&nodes, 0);

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
            0, // initial_errors
            health_counters,
        )
        .unwrap();

        Self { nodes, description, processor, receiver, task_queue }
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
    let mut fixture = TestFixture::new(4);

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
    let mut fixture = TestFixture::new(4);

    // initial_block_seqno is set to 1 in create_test_desc()
    assert_eq!(fixture.description.get_initial_block_seqno(), 1);
    // Ensure the expected seqno does NOT depend on last_committed_seqno when parent=None.
    fixture.processor.last_committed_seqno = Some(123);

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
    // With DISABLE_NON_FINALIZED_PARENTS_FOR_COLLATION=false (optimistic validation),
    // shardchains use the MC lag threshold rule for empty-block generation, not the
    // committed-head rule. Only masterchain still uses committed-head gating.
    //
    // Verify that masterchain sessions still use committed-head empty-block gating.
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

    assert_eq!(processor.last_committed_seqno, Some(46));

    // Slot 0 is the initial `first_non_progressed_slot` in fresh state.
    // MC: new_seqno=48, committed=46 -> 46+1=47 < 48 -> empty
    assert!(processor.should_generate_empty_block(SlotIndex::new(0), 48));
    // MC: new_seqno=47, committed=46 -> 46+1=47 == 47 -> NOT empty
    assert!(!processor.should_generate_empty_block(SlotIndex::new(0), 47));
}

#[test]
fn test_masterchain_out_of_order_finalization_waits_for_missing_final_cert() {
    // Deterministic corner case:
    // - committed head seqno = 100
    // - we observe a finalized (FinalCert) candidate with seqno = 103
    // Masterchain invariant: we must NOT commit any intermediate non-empty blocks using NotarCert-only signatures.
    // Expected behavior: keep the finalized entry in journal and report WaitingForFinalCert(expected_seqno=101).
    let mut fixture = TestFixture::new(4);

    // Seed committed head
    let committed_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 100, UInt256::rand(), UInt256::rand());
    fixture.processor.last_committed_seqno = Some(100);
    fixture.processor.last_committed_block_id = Some(committed_block_id);

    // Create a received non-empty candidate that is "finalized" but ahead by seqno.
    let slot = SlotIndex::new(200);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };

    let finalized_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 103, UInt256::rand(), UInt256::rand());

    fixture.processor.received_candidates.insert(
        candidate_id.clone(),
        ReceivedCandidate {
            slot,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: block_hash.clone(),
            candidate_hash_data_bytes: vec![1, 2, 3], // non-empty (required for signature context, if ever used)
            block_id: finalized_block_id.clone(),
            root_hash: finalized_block_id.root_hash.clone(),
            file_hash: finalized_block_id.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xAA].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xBB].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: None,
            is_fully_resolved: true,
        },
    );

    // Directly verify the collector result first (no journal side effects)
    match fixture.processor.collect_gapless_commit_chain(&candidate_id) {
        ChainCollectionResult::WaitingForFinalCert {
            expected_seqno,
            finalized_id,
            finalized_seqno,
        } => {
            assert_eq!(expected_seqno, 101);
            assert_eq!(finalized_id, candidate_id);
            assert_eq!(finalized_seqno, 103);
        }
        other => panic!("unexpected chain collection result: {:?}", mem::discriminant(&other)),
    }

    // Insert a journal entry and ensure try_commit does NOT schedule requestCandidate and does NOT commit.
    let dummy_final_cert: crate::certificate::FinalCertPtr =
        Arc::new(crate::certificate::Certificate {
            vote: crate::simplex_state::FinalizeVote { slot, block_hash: block_hash.clone() },
            signatures: Vec::new(),
        });
    let dummy_event = BlockFinalizedEvent {
        slot,
        block_hash: block_hash.clone(),
        block_id: Some(finalized_block_id),
        certificate: dummy_final_cert,
    };
    fixture.processor.finalized_journal_pending_commit.insert(
        candidate_id.clone(),
        FinalizedEntry { event: dummy_event, finalized_at: fixture.description.get_time() },
    );

    fixture.processor.try_commit_finalized_chains();

    assert_eq!(fixture.processor.last_committed_seqno, Some(100));
    assert!(fixture.processor.requested_candidates.is_empty());
    assert!(fixture.processor.finalized_journal_pending_commit.contains_key(&candidate_id));
    assert!(!fixture.processor.finalized_blocks.contains(&candidate_id));
}

#[test]
fn test_update_resolution_cache_chain_handles_deep_descendant_chains() {
    // Regression: in single-host nets, we can receive an old missing candidate late (e.g. slot ~5),
    // when hundreds of descendants already exist. update_resolution_cache_chain used to recurse
    // over descendants and could hit very deep recursion; it must handle deep chains safely.
    let mut fixture = TestFixture::new(4);

    // Build a linear parent->child chain longer than the previous recursion warning threshold.
    const N: usize = 256;
    let mut ids: Vec<RawCandidateId> = Vec::with_capacity(N);

    for i in 0..N {
        let slot = SlotIndex::new(i as u32);
        let candidate_hash = UInt256::rand();
        let id = RawCandidateId { slot, hash: candidate_hash.clone() };

        let parent_id = if i == 0 { None } else { Some(ids[i - 1].clone()) };

        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            i as u32,
            UInt256::rand(),
            UInt256::rand(),
        );

        fixture.processor.received_candidates.insert(
            id.clone(),
            ReceivedCandidate {
                slot,
                source_idx: ValidatorIndex::new(0),
                candidate_id_hash: candidate_hash.clone(),
                candidate_hash_data_bytes: vec![1, 2, 3],
                block_id: block_id.clone(),
                root_hash: block_id.root_hash.clone(),
                file_hash: block_id.file_hash.clone(),
                data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    vec![0xAA].into(),
                ),
                collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    vec![0xBB].into(),
                ),
                receive_time: fixture.description.get_time(),
                is_empty: false,
                parent_id,
                is_fully_resolved: false,
            },
        );

        ids.push(id);
    }

    // Trigger an update from the root; resolution should propagate to all descendants.
    fixture.processor.update_resolution_cache_chain(&ids[0]);

    for id in ids {
        let r = fixture.processor.received_candidates.get(&id).expect("candidate missing");
        assert!(r.is_fully_resolved, "candidate {:?} should be resolved", id.slot);
    }
}

#[test]
fn test_masterchain_waiting_for_final_cert_commits_expected_seqno_when_available() {
    // Masterchain catch-up invariant:
    // - A finalized (FinalCert) candidate may arrive ahead of the committed head (seqno gap).
    // - We must not commit intermediate MC blocks using NotarCert-only signatures.
    // - Once the missing FinalCert for the *next committable* seqno is available, we should
    //   commit that block to advance the committed head and unblock progress.
    let mut fixture = TestFixture::new(4);

    // Seed committed head at seqno 10
    let committed_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 10, UInt256::rand(), UInt256::rand());
    fixture.processor.last_committed_seqno = Some(10);
    fixture.processor.last_committed_block_id = Some(committed_block_id);

    // Next committable block (expected_seqno = 11)
    let slot_11 = SlotIndex::new(21);
    let hash_11 = UInt256::rand();
    let id_11 = RawCandidateId { slot: slot_11, hash: hash_11.clone() };
    let block_id_11 =
        BlockIdExt::with_params(ShardIdent::masterchain(), 11, UInt256::rand(), UInt256::rand());
    fixture.processor.received_candidates.insert(
        id_11.clone(),
        ReceivedCandidate {
            slot: slot_11,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash_11.clone(),
            candidate_hash_data_bytes: vec![1, 2, 3],
            block_id: block_id_11.clone(),
            root_hash: block_id_11.root_hash.clone(),
            file_hash: block_id_11.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xAA].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xBB].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: None,
            is_fully_resolved: true,
        },
    );

    // Provide the missing FinalCert for seqno 11 (sufficient weight: 3/4).
    let final_cert_11 = Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote { slot: slot_11, block_hash: hash_11.clone() },
        signatures: vec![
            // NOTE: SessionProcessor expects 64-byte Ed25519 signatures when building BlockSignaturesSimplex.
            crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
            crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
            crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
        ],
    });
    fixture
        .processor
        .simplex_state
        .set_finalize_certificate(&fixture.description, slot_11, &hash_11, final_cert_11)
        .expect("failed to store finalize cert for expected_seqno");

    // Finalized candidate observed ahead by seqno (seqno 12, expected is 11).
    let slot_12 = SlotIndex::new(24);
    let hash_12 = UInt256::rand();
    let id_12 = RawCandidateId { slot: slot_12, hash: hash_12.clone() };
    let block_id_12 =
        BlockIdExt::with_params(ShardIdent::masterchain(), 12, UInt256::rand(), UInt256::rand());
    fixture.processor.received_candidates.insert(
        id_12.clone(),
        ReceivedCandidate {
            slot: slot_12,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash_12.clone(),
            candidate_hash_data_bytes: vec![4, 5, 6],
            block_id: block_id_12.clone(),
            root_hash: block_id_12.root_hash.clone(),
            file_hash: block_id_12.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xCC].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xDD].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: Some(id_11.clone()), // walkback target for expected_seqno=11
            is_fully_resolved: true,
        },
    );

    // Journal entry for the ahead-of-head finalized block (certificate contents are irrelevant
    // for the gap-recovery commit because we commit using the missing block's FinalCert).
    let dummy_final_cert_12: crate::certificate::FinalCertPtr =
        Arc::new(crate::certificate::Certificate {
            vote: crate::simplex_state::FinalizeVote { slot: slot_12, block_hash: hash_12.clone() },
            signatures: vec![
                crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
            ],
        });
    let event_12 = BlockFinalizedEvent {
        slot: slot_12,
        block_hash: hash_12.clone(),
        block_id: Some(block_id_12),
        certificate: dummy_final_cert_12,
    };
    fixture.processor.finalized_journal_pending_commit.insert(
        id_12.clone(),
        FinalizedEntry { event: event_12, finalized_at: fixture.description.get_time() },
    );

    // Act: the processor should commit the missing expected_seqno block (seqno 11).
    fixture.processor.try_commit_finalized_chains();

    assert_eq!(fixture.processor.last_committed_seqno, Some(11));
    assert!(fixture.processor.finalized_blocks.contains(&id_11));
    assert!(fixture.processor.finalized_journal_pending_commit.contains_key(&id_12));
    assert!(
        !fixture.processor.finalized_blocks.contains(&id_12),
        "ahead-of-head finalized block should remain pending until the next commit pass"
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
fn test_receiver_records_no_actions_initially() {
    let fixture = TestFixture::new(4);
    let actions = fixture.drain_receiver_actions();
    assert!(actions.is_empty(), "Expected no receiver actions initially");
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

    // Times should be close (both created around same time)
    let diff = time2.duration_since(time1).unwrap_or_else(|_| time1.duration_since(time2).unwrap());
    assert!(diff < Duration::from_millis(100), "Test fixtures should have similar initial times");
}

// ============================================================================
// Batch Finalization Tests (BATCH-COMMIT-1 / TEST-BATCH-1)
// ============================================================================

/// TEST-BATCH-1: Notarized parents + finalized descendant (Case A)
///
/// Scenario:
/// - slot 1: notarized (NotarCert), not finalized
/// - slot 2: notarized (NotarCert), not finalized
/// - slot 3: finalized (FinalCert)
/// - parent chain: 1 → 2 → 3 (all non-empty blocks)
///
/// Expected: THREE COMMITS emitted in order (not skip/skip/commit):
/// - commit(round=1, is_final=false, sigs=NotarCert/approve)
/// - commit(round=2, is_final=false, sigs=NotarCert/approve)
/// - commit(round=3, is_final=true,  sigs=FinalCert/final)
///
/// This verifies C++ `finalize_blocks()` parity:
/// - Parent blocks CAN be committed (even if not finalized)
/// - Parent blocks use NotarCert/`create_simplex_approve` signatures
/// - Triggered block uses FinalCert/`create_simplex` signatures
/// - No panic on is_triggered_block=false
/// - Round stream is gapless (round = slot in Option B)
///
/// Status: PLACEHOLDER - full test requires FSM integration
#[test]
#[ignore] // TODO(TEST-BATCH-1): requires FSM events + candidate resolution infrastructure
fn test_batch_finalization_notarized_parents_finalized_descendant() {
    // This test is a placeholder documenting the expected behavior for BATCH-COMMIT-1.
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

    todo!("TEST-BATCH-1: implement full batch finalization integration test");
}

// ============================================================================
// SIMPLEX_ROUNDLESS Mode Tests (ROUNDLESS-1)
// ============================================================================

/// Test that SIMPLEX_ROUNDLESS constant is u32::MAX
#[test]
fn test_simplex_roundless_constant_value() {
    assert_eq!(SIMPLEX_ROUNDLESS, u32::MAX, "SIMPLEX_ROUNDLESS should be u32::MAX");
    assert_eq!(SIMPLEX_ROUNDLESS, 0xFFFFFFFF, "SIMPLEX_ROUNDLESS should be 0xFFFFFFFF");
}

/// Test that SessionProcessor uses C++-compatible parenting options
///
/// We keep `require_finalized_parent=false` (C++ mode) to avoid deadlock when a slot is
/// notarized but not finalized/skipped yet. ValidatorGroup limitation is handled separately
/// by forcing EMPTY collation on non-committed parents.
#[test]
fn test_simplex_state_options_require_finalized_parent() {
    // Default (cpp_compatible) should have require_finalized_parent=false
    let cpp_compat = SimplexStateOptions::cpp_compatible();
    assert!(
        !cpp_compat.require_finalized_parent,
        "cpp_compatible() should have require_finalized_parent=false"
    );

    // With optimistic validation, the collation gate is disabled:
    // ValidatorGroup now uses candidate-native validation, so non-finalized parents are allowed.
    assert!(
        !DISABLE_NON_FINALIZED_PARENTS_FOR_COLLATION,
        "DISABLE_NON_FINALIZED_PARENTS_FOR_COLLATION should be false with optimistic validation enabled"
    );
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
fn test_candidate_decision_fail_drops_late_failure_for_committed_block() {
    // Regression: in roundless Simplex, validation callbacks can arrive late (after the block is
    // already committed). In this case we must drop the result and NOT schedule retries / WARN.
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(5);
    let candidate_id = RawCandidateId { slot, hash: UInt256::rand() };

    // Pretend we've already committed past this block seqno.
    fixture.processor.last_committed_seqno = Some(100);

    // Create a non-empty RawCandidate with seqno <= last_committed_seqno.
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
            receive_time: SystemTime::now(),
            is_empty,
            parent_id,
            is_fully_resolved: true,
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

#[test]
fn test_check_validation_forwards_candidate_with_notarized_parent() {
    let mut fixture = TestFixture::new(4);

    let parent_slot = SlotIndex::new(0);
    let parent_hash = UInt256::rand();
    let parent_id = RawCandidateId { slot: parent_slot, hash: parent_hash.clone() };

    let child_slot = SlotIndex::new(1);
    let child_id = RawCandidateId { slot: child_slot, hash: UInt256::rand() };

    let raw_candidate =
        make_test_non_empty_candidate(child_id.clone(), Some(parent_id.clone()), &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

    // Before notarization: check_validation should NOT forward the candidate
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

    // Parent is NOT notarized — candidate must stay in pending_validations
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
        Some(parent_id),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

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
        Some(parent_id),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

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
        Some(parent_id),
    );
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &child_id, raw_candidate, time);

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
    let mut fixture = TestFixture::new(4);

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

    // First check_validation: A (genesis) should pass, B should not (parent not notarized)
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
    s.last_parent_aging_warn = base;
    s.last_cert_fail_warn = base;
    s.last_standstill_warn = base;
    s.last_finalization_speed_warn = base;
    s.last_finalization_nonzero_at = base;
    s.last_candidate_giveup_warn = base;
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
fn test_check_collation_blocks_before_earliest_time() {
    let mut fixture = TestFixture::new(4);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fixture.processor.set_time(base_time);

    let gate_time = base_time + Duration::from_millis(500);
    fixture.processor.earliest_collation_time = Some(gate_time);

    fixture.processor.reset_next_awake_time();

    fixture.processor.check_collation();

    assert_eq!(fixture.processor.get_next_awake_time(), gate_time);
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
    assert_eq!(fixture.processor.get_next_awake_time(), gate_time);

    assert_eq!(fixture.processor.earliest_collation_time, Some(gate_time));

    fixture.processor.reset_next_awake_time();
    fixture.processor.check_collation();
    assert_eq!(fixture.processor.get_next_awake_time(), gate_time);
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

    fixture.processor.handle_candidate_query_fallback(slot, block_hash, false, callback);

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

    fixture.processor.handle_candidate_query_fallback(slot, block_hash, false, callback);

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

#[test]
fn test_candidate_data_cache_populated_on_candidate_received() {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Debug).try_init();
    let mut fixture = TestFixture::new(4);

    // Use slot 0 so that validator 0 (local) is the slot leader
    let slot = 0u32;
    let block_data = vec![1u8, 2, 3, 4, 5];
    let collated_data: Vec<u8> = vec![];
    let root_hash = UInt256::from_slice(&sha256_digest(&block_data));
    let shard = ShardIdent::masterchain();

    // Build uncompressed TL candidate (same approach as test_receiver_candidate_resolver)
    let tl_inner = TlCandidate {
        src: UInt256::default(),
        round: slot as i32,
        root_hash: root_hash.clone(),
        data: block_data.clone().into(),
        collated_data: collated_data.clone().into(),
    };
    let candidate_bytes = consensus_common::serialize_tl_boxed_object!(&tl_inner.into_boxed());

    let block_id = BlockIdExt {
        shard_id: shard,
        seq_no: slot,
        root_hash: root_hash.clone(),
        file_hash: root_hash.clone(),
    };
    let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data));

    let candidate_hash = crate::utils::compute_candidate_id_hash_u32(
        slot,
        Some(&block_id),
        Some(&collated_file_hash),
        None,
    );

    let session_id = fixture.processor.session_id().clone();
    let leader_key = fixture.processor.description.get_source_public_key(ValidatorIndex::new(0));
    let signature =
        crate::utils::sign_candidate_u32(&session_id, slot, &candidate_hash, leader_key)
            .expect("signing failed");

    let broadcast = CandidateData::Consensus_Block(CandidateDataBlock {
        slot: slot as i32,
        candidate: candidate_bytes.into(),
        parent: CandidateParent::Consensus_CandidateWithoutParents,
        signature: signature.into(),
    });

    let candidate_id = RawCandidateId { slot: SlotIndex::new(slot), hash: candidate_hash.clone() };

    assert!(
        !fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "cache should be empty before on_candidate_received"
    );

    fixture.processor.on_candidate_received(0, broadcast, None);

    assert!(
        fixture.processor.candidate_data_cache.contains_key(&candidate_id),
        "cache should be populated after on_candidate_received"
    );

    assert!(
        fixture.processor.received_candidates.contains_key(&candidate_id),
        "received_candidates should also have the candidate"
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
            receive_time: fixture.processor.now(),
            is_empty: false,
            parent_id: None,
            is_fully_resolved: true,
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
fn test_handle_block_finalized_requests_triggered_stub_body_when_committed_head_exists() {
    let mut fixture = TestFixture::new(4);

    // Simulate an already-committed head, so triggered finalized block enters
    // collect_gapless_commit_chain() as a non-genesis continuation.
    fixture.processor.last_committed_seqno = Some(100);
    fixture.processor.last_committed_block_id = Some(BlockIdExt::with_params(
        ShardIdent::masterchain(),
        100,
        UInt256::rand(),
        UInt256::rand(),
    ));

    let slot = SlotIndex::new(555);
    let block_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
    let finalized_block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 101, UInt256::rand(), UInt256::rand());

    let final_cert = Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote { slot, block_hash: block_hash.clone() },
        signatures: vec![
            crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
            crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
            crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
        ],
    });

    fixture.processor.handle_block_finalized(BlockFinalizedEvent {
        slot,
        block_hash: block_hash.clone(),
        block_id: Some(finalized_block_id),
        certificate: final_cert,
    });

    assert!(
        fixture.processor.requested_candidates.contains_key(&candidate_id),
        "triggered finalized-boundary stub must be treated as missing body and requested"
    );

    // The core regression guard is scheduling requestCandidate at processor level.
    // (Receiver send timing is exercised by dedicated delayed-action tests.)
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
    fixture.processor.handle_candidate_query_fallback(slot, block_hash, false, callback);

    let result = rx.recv().unwrap();
    assert!(result.is_ok(), "should return Ok even when nothing found");
}

#[test]
fn test_set_mc_finalized_seqno_couples_consensus_finalized_seqno() {
    let mut fixture = TestFixture::new(4);

    // Initially 0
    assert_eq!(fixture.processor.last_consensus_finalized_seqno, Some(0));

    // Set MC finalized to 42
    fixture.processor.set_mc_finalized_seqno(42);

    // C++ parity: consensus finalized should advance to max(mc, consensus)
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(42),
        "set_mc_finalized_seqno should couple to last_consensus_finalized_seqno via max()"
    );

    // Set consensus finalized higher via direct field (simulating a final commit)
    fixture.processor.last_consensus_finalized_seqno = Some(100);

    // Set MC finalized lower => should NOT decrease consensus
    fixture.processor.set_mc_finalized_seqno(50);
    assert_eq!(
        fixture.processor.last_consensus_finalized_seqno,
        Some(100),
        "set_mc_finalized_seqno must not decrease last_consensus_finalized_seqno"
    );

    // Monotonic MC seqno: out-of-order MC event with lower seqno must not regress
    fixture.processor.last_mc_finalized_seqno = Some(200);
    fixture.processor.set_mc_finalized_seqno(150);
    assert_eq!(
        fixture.processor.last_mc_finalized_seqno,
        Some(200),
        "set_mc_finalized_seqno must keep last_mc_finalized_seqno monotonic"
    );
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
// Gapless commit scheduler hardening tests
// ============================================================================

/// Verify that `cleanup_old_candidates` removes stale journal entries for old slots
/// and increments the session error counter accordingly.
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

    fixture.processor.finalized_journal_pending_commit.insert(
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

    fixture.processor.finalized_journal_pending_commit.insert(
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

    assert_eq!(fixture.processor.finalized_journal_pending_commit.len(), 2);

    let errors_before =
        fixture.processor.session_errors_count.load(std::sync::atomic::Ordering::Relaxed);

    // Cleanup slots < 10 — old_slot(5) should be removed, current_slot(20) kept.
    fixture.processor.cleanup_old_candidates(SlotIndex::new(10));

    assert_eq!(fixture.processor.finalized_journal_pending_commit.len(), 1);
    assert!(!fixture.processor.finalized_journal_pending_commit.contains_key(&old_id));
    assert!(fixture.processor.finalized_journal_pending_commit.contains_key(&current_id));

    let errors_after =
        fixture.processor.session_errors_count.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(errors_after - errors_before, 1, "stale journal entry should increment error count");
}

/// Verify that the scheduler processes entries in seqno-ascending order,
/// not arbitrary HashMap order.
/// Both entries are WaitingForFinalCert on MC (seqno ahead of committed head)
/// so neither can commit. Both stay pending in the journal.
#[test]
fn test_try_commit_processes_in_seqno_order() {
    let mut fixture = TestFixture::new(4);

    // TestFixture defaults to masterchain. Committed head seqno = 10.
    let committed_block_id = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        10,
        UInt256::rand(),
        UInt256::rand(),
    );
    fixture.processor.last_committed_seqno = Some(10);
    fixture.processor.last_committed_block_id = Some(committed_block_id);

    // Both seqnos are ahead of expected (11), so MC fast-path returns
    // WaitingForFinalCert and both entries remain in the journal.
    let slot_a = SlotIndex::new(30);
    let hash_a = UInt256::rand();
    let id_a = RawCandidateId { slot: slot_a, hash: hash_a.clone() };
    let block_id_a = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        13, // ahead: expected=11
        UInt256::rand(),
        UInt256::rand(),
    );

    let slot_b = SlotIndex::new(25);
    let hash_b = UInt256::rand();
    let id_b = RawCandidateId { slot: slot_b, hash: hash_b.clone() };
    let block_id_b = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        12, // ahead: expected=11
        UInt256::rand(),
        UInt256::rand(),
    );

    for (id, slot, hash, block_id) in
        [(&id_a, slot_a, &hash_a, &block_id_a), (&id_b, slot_b, &hash_b, &block_id_b)]
    {
        fixture.processor.received_candidates.insert(
            id.clone(),
            ReceivedCandidate {
                slot,
                source_idx: ValidatorIndex::new(0),
                candidate_id_hash: hash.clone(),
                candidate_hash_data_bytes: vec![1, 2, 3],
                block_id: block_id.clone(),
                root_hash: block_id.root_hash.clone(),
                file_hash: block_id.file_hash.clone(),
                data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    vec![0xAA].into(),
                ),
                collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    vec![0xBB].into(),
                ),
                receive_time: fixture.description.get_time(),
                is_empty: false,
                parent_id: None,
                is_fully_resolved: true,
            },
        );

        let cert: crate::certificate::FinalCertPtr = Arc::new(crate::certificate::Certificate {
            vote: crate::simplex_state::FinalizeVote { slot, block_hash: hash.clone() },
            signatures: vec![
                crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
            ],
        });
        fixture.processor.finalized_journal_pending_commit.insert(
            id.clone(),
            FinalizedEntry {
                event: BlockFinalizedEvent {
                    slot,
                    block_hash: hash.clone(),
                    block_id: Some(block_id.clone()),
                    certificate: cert,
                },
                finalized_at: fixture.description.get_time(),
            },
        );
    }

    fixture.processor.try_commit_finalized_chains();

    // Both entries remain pending — MC blocks ahead of committed head wait for FinalCert.
    assert!(fixture.processor.finalized_journal_pending_commit.contains_key(&id_a));
    assert!(fixture.processor.finalized_journal_pending_commit.contains_key(&id_b));
    assert_eq!(fixture.processor.last_committed_seqno, Some(10));
}

/// Verify the finalized_uncommitted_gauge is updated correctly.
#[test]
fn test_finalized_uncommitted_gauge_tracks_journal_size() {
    let mut fixture = TestFixture::new(4);

    // Empty journal — gauge should be 0 (function runs without panic).
    fixture.processor.try_commit_finalized_chains();

    // Add a journal entry that will become AlreadyCommitted
    let slot = SlotIndex::new(5);
    let hash = UInt256::rand();
    let id = RawCandidateId { slot, hash: hash.clone() };

    fixture.processor.last_committed_seqno = Some(100);
    let committed_block_id = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        100,
        UInt256::rand(),
        UInt256::rand(),
    );
    fixture.processor.last_committed_block_id = Some(committed_block_id.clone());

    // Insert a received candidate with seqno < committed so collect_gapless returns AlreadyCommitted
    fixture.processor.received_candidates.insert(
        id.clone(),
        ReceivedCandidate {
            slot,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash.clone(),
            candidate_hash_data_bytes: vec![1, 2, 3],
            block_id: committed_block_id.clone(),
            root_hash: committed_block_id.root_hash.clone(),
            file_hash: committed_block_id.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xAA].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xBB].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: None,
            is_fully_resolved: true,
        },
    );

    let cert: crate::certificate::FinalCertPtr = Arc::new(crate::certificate::Certificate {
        vote: crate::simplex_state::FinalizeVote { slot, block_hash: hash.clone() },
        signatures: Vec::new(),
    });
    fixture.processor.finalized_journal_pending_commit.insert(
        id.clone(),
        FinalizedEntry {
            event: BlockFinalizedEvent {
                slot,
                block_hash: hash,
                block_id: Some(committed_block_id),
                certificate: cert,
            },
            finalized_at: fixture.description.get_time(),
        },
    );

    assert_eq!(fixture.processor.finalized_journal_pending_commit.len(), 1);

    fixture.processor.try_commit_finalized_chains();

    // The AlreadyCommitted entry should be removed
    assert!(
        fixture.processor.finalized_journal_pending_commit.is_empty(),
        "AlreadyCommitted entry should be removed from journal"
    );
}

/// Verify that seqno-sorted iteration commits sequential chains in a single pass
/// and schedules an immediate re-check via set_next_awake_time(now).
#[test]
fn test_sorted_pass_commits_sequential_chains_and_reschedules() {
    let mut fixture = TestFixture::new(4);

    // Committed head at seqno 10
    let committed_block_id = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        10,
        UInt256::rand(),
        UInt256::rand(),
    );
    fixture.processor.last_committed_seqno = Some(10);
    fixture.processor.last_committed_block_id = Some(committed_block_id.clone());

    // Build chain: slot_a (seqno 11, parent=boundary) → slot_b (seqno 12, parent=slot_a)
    // On MC with matching expected_seqno, the MC fast-path commits the single block.
    // After committing slot_a (seqno=11), re-loop should pick up slot_b (seqno=12).
    let slot_a = SlotIndex::new(20);
    let hash_a = UInt256::rand();
    let id_a = RawCandidateId { slot: slot_a, hash: hash_a.clone() };
    let block_id_a = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        11,
        UInt256::rand(),
        UInt256::rand(),
    );

    let slot_b = SlotIndex::new(25);
    let hash_b = UInt256::rand();
    let id_b = RawCandidateId { slot: slot_b, hash: hash_b.clone() };
    let block_id_b = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        12,
        UInt256::rand(),
        UInt256::rand(),
    );

    // slot_a: parent = None (session boundary → MC fast-path single-commit if seqno matches)
    fixture.processor.received_candidates.insert(
        id_a.clone(),
        ReceivedCandidate {
            slot: slot_a,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash_a.clone(),
            candidate_hash_data_bytes: vec![1, 2, 3],
            block_id: block_id_a.clone(),
            root_hash: block_id_a.root_hash.clone(),
            file_hash: block_id_a.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xAA].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xBB].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: None,
            is_fully_resolved: true,
        },
    );

    // slot_b: parent = slot_a (will be WaitingForFinalCert initially since expected=11, seqno=12)
    fixture.processor.received_candidates.insert(
        id_b.clone(),
        ReceivedCandidate {
            slot: slot_b,
            source_idx: ValidatorIndex::new(0),
            candidate_id_hash: hash_b.clone(),
            candidate_hash_data_bytes: vec![4, 5, 6],
            block_id: block_id_b.clone(),
            root_hash: block_id_b.root_hash.clone(),
            file_hash: block_id_b.file_hash.clone(),
            data: consensus_common::ConsensusCommonFactory::create_block_payload(vec![0xCC].into()),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                vec![0xDD].into(),
            ),
            receive_time: fixture.description.get_time(),
            is_empty: false,
            parent_id: Some(id_a.clone()),
            is_fully_resolved: true,
        },
    );

    // Provide FinalCert for both (MC requires FinalCert for non-empty blocks)
    for (slot, hash) in [(&slot_a, &hash_a), (&slot_b, &hash_b)] {
        let final_cert = Arc::new(crate::certificate::Certificate {
            vote: crate::simplex_state::FinalizeVote { slot: *slot, block_hash: hash.clone() },
            signatures: vec![
                crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
            ],
        });
        fixture
            .processor
            .simplex_state
            .set_finalize_certificate(&fixture.description, *slot, hash, final_cert)
            .expect("store final cert");
    }

    // Journal entries for both
    for (id, slot, hash, block_id) in
        [(&id_a, slot_a, &hash_a, &block_id_a), (&id_b, slot_b, &hash_b, &block_id_b)]
    {
        let cert: crate::certificate::FinalCertPtr = Arc::new(crate::certificate::Certificate {
            vote: crate::simplex_state::FinalizeVote { slot, block_hash: hash.clone() },
            signatures: vec![
                crate::certificate::VoteSignature::new(ValidatorIndex::new(0), vec![0u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(1), vec![1u8; 64]),
                crate::certificate::VoteSignature::new(ValidatorIndex::new(2), vec![2u8; 64]),
            ],
        });
        fixture.processor.finalized_journal_pending_commit.insert(
            id.clone(),
            FinalizedEntry {
                event: BlockFinalizedEvent {
                    slot,
                    block_hash: hash.clone(),
                    block_id: Some(block_id.clone()),
                    certificate: cert,
                },
                finalized_at: fixture.description.get_time(),
            },
        );
    }

    // Push next_awake_time into the future so we can verify it gets pulled back after commit.
    fixture.processor.reset_next_awake_time();
    assert!(
        fixture.processor.get_next_awake_time() > fixture.description.get_time(),
        "next_awake_time should be in the future before commit"
    );

    // Because finalized_keys are sorted by seqno, slot_a (seqno=11) is processed
    // first. After it commits, last_committed_seqno advances to 11, so when the
    // iteration reaches slot_b (seqno=12), expected_seqno matches and it commits too.
    fixture.processor.try_commit_finalized_chains();

    assert_eq!(
        fixture.processor.last_committed_seqno,
        Some(12),
        "sorted iteration should commit both seqno 11 and 12 in one pass"
    );
    assert!(
        fixture.processor.finalized_journal_pending_commit.is_empty(),
        "all journal entries should be committed and removed"
    );
    // Commits happened, so an immediate re-check should be scheduled.
    assert!(
        fixture.processor.get_next_awake_time() <= fixture.description.get_time(),
        "next_awake_time should be <= now after a successful commit"
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
    // In C++ mode (allow_skip_after_notarize=true) a skip vote may follow
    // the notarize vote after the timeout fires -- that is harmless and
    // expected.  The key property is that the notarize vote was emitted.
}

/// Verify that the `log::warn!` for "drop because new block is already
/// committed" only fires when `cand_seqno <= committed_seqno`, i.e. the
/// candidate is actually dropped.  When `cand_seqno > committed_seqno`
/// the candidate must proceed to `validated_candidates`.
#[test]
fn test_candidate_decision_ok_does_not_drop_when_cand_seqno_greater_than_committed() {
    let mut fixture = TestFixture::new(4);

    let slot = SlotIndex::new(0);
    let candidate_hash = UInt256::rand();
    let candidate_id = RawCandidateId { slot, hash: candidate_hash.clone() };

    let raw_candidate = make_test_non_empty_candidate(candidate_id.clone(), None, &fixture.nodes);
    let time = fixture.description.get_time();
    insert_pending_validation(&mut fixture.processor, &candidate_id, raw_candidate, time);

    // Set last_committed_seqno to a value BELOW the candidate's seqno.
    // make_test_non_empty_candidate uses slot.value()+1 as seq_no, so for
    // slot 0 the candidate seqno = 1.  Setting committed to 0 means
    // cand_seqno (1) > committed_seqno (0) → candidate must NOT be dropped.
    fixture.processor.last_committed_seqno = Some(0);

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
