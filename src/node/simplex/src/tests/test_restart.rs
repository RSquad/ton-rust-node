/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Simplex restart and recovery unit tests
//!
//! Tests for startup recovery and related functionality.
//!
//! ## Test Categories
//!
//! - `SessionStartupRecoveryOptions` construction
//! - Future: `SessionStartupRecoveryProcessor` tests with mock listener

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    database::{
        Bootstrap, CandidateInfoRecord, FinalizedBlockRecord, NotarCertRecord, PoolStateRecord,
        VoteRecord,
    },
    misbehavior::VoteResult,
    session_description::SessionDescription,
    simplex_state::{NotarizeVote, Vote},
    startup_recovery::{
        SessionStartupRecoveryListener, SessionStartupRecoveryOptions,
        SessionStartupRecoveryProcessor,
    },
    utils::sign_vote,
    SessionId, SessionNode, SessionOptions,
};
use std::{sync::Arc, time::SystemTime};
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::{
        consensus::{
            candidatehashdata::{CandidateHashDataEmpty, CandidateHashDataOrdinary},
            candidateid::CandidateId as TlCandidateId,
            candidateparent::CandidateParent as TlCandidateParent,
            CandidateData, CandidateHashData, CandidateId as CandidateIdBoxed, CandidateParent,
        },
        validator_session::candidate::Candidate as TlCandidate,
    },
    IntoBoxed,
};
use ton_block::{
    sha256_digest, BlockIdExt, BocFlags, BocWriter, BuilderData, Ed25519KeyOption, ShardIdent,
    UInt256,
};

#[test]
fn test_session_startup_recovery_options_new() {
    let opts = SessionStartupRecoveryOptions::new(100);
    assert_eq!(opts.initial_block_seqno, 100);
}

// ============================================================================
// Test helpers
// ============================================================================

/// Create a minimal test session description with single validator (self_idx = 0)
fn create_test_desc() -> Arc<SessionDescription> {
    create_test_desc_with_validators(1, 0)
}

/// Create a test session description with specified number of validators.
/// `local_idx` determines which validator is the local one.
fn create_test_desc_with_validators(count: usize, local_idx: usize) -> Arc<SessionDescription> {
    let mut nodes = Vec::with_capacity(count);
    let mut local_key: Option<crate::PublicKey> = None;

    for i in 0..count {
        let key = Ed25519KeyOption::generate().expect("key gen");
        let public_key: crate::PublicKey = key.clone().into();
        if i == local_idx {
            local_key = Some(public_key.clone());
        }
        nodes.push(SessionNode { public_key, adnl_id: key.id().clone(), weight: 1 });
    }

    let local_key = local_key.expect("local_idx out of range");
    let shard = ShardIdent::masterchain();
    let session_id = SessionId::default();
    let options = SessionOptions::default();
    Arc::new(
        SessionDescription::new(
            &options,
            session_id,
            0,
            &nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None,
        )
        .expect("create SessionDescription"),
    )
}

// ============================================================================
// Startup recovery helpers
// ============================================================================

fn make_candidate_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    let mut hash = [0u8; 32];
    hash[0] = hash_byte;
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from(hash) }
}

fn make_block_id(seqno: u32) -> BlockIdExt {
    BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: seqno,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    }
}

fn make_candidate_hash_data_with_parent(
    block_id: BlockIdExt,
    collated_file_hash: UInt256,
    parent: Option<RawCandidateId>,
) -> CandidateHashData {
    let parent_tl = match parent {
        None => CandidateParent::Consensus_CandidateWithoutParents,
        Some(parent_id) => {
            let tl_parent_id =
                TlCandidateId { slot: parent_id.slot.value() as i32, hash: parent_id.hash };
            CandidateParent::Consensus_CandidateParent(TlCandidateParent {
                id: CandidateIdBoxed::Consensus_CandidateId(tl_parent_id),
            })
        }
    };

    CandidateHashData::Consensus_CandidateHashDataOrdinary(CandidateHashDataOrdinary {
        block: block_id,
        collated_file_hash: collated_file_hash.into(),
        parent: parent_tl,
    })
}

fn make_candidate_hash_data_empty(
    referenced_block: BlockIdExt,
    parent: RawCandidateId,
) -> CandidateHashData {
    let tl_parent_id = TlCandidateId { slot: parent.slot.value() as i32, hash: parent.hash };

    CandidateHashData::Consensus_CandidateHashDataEmpty(CandidateHashDataEmpty {
        block: referenced_block,
        parent: tl_parent_id,
    })
}

/// Create valid BOC bytes from raw data (for tests that need valid BOC input).
fn make_test_boc(data: &[u8], flags: BocFlags) -> Vec<u8> {
    let mut b = BuilderData::new();
    b.append_raw(data, data.len() * 8).unwrap();
    let cell = b.into_cell().unwrap();
    let mut buf = Vec::new();
    BocWriter::with_flags([cell], flags).unwrap().write(&mut buf).unwrap();
    buf
}

fn make_validator_session_candidate_bytes(
    round: i32,
    root_hash: UInt256,
    data: Vec<u8>,
    collated_data: Vec<u8>,
) -> Vec<u8> {
    let tl_candidate =
        TlCandidate { src: UInt256::default(), round, root_hash, data, collated_data };

    consensus_common::serialize_tl_boxed_object!(&tl_candidate.into_boxed())
}

// ============================================================================
// Startup recovery orchestration (apply_bootstrap) with a mock listener
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryCall {
    OnVote,
    SetFirstNonFinalized,
    MarkLocalFlag,
    SetFirstNonannouncedWindow,
    GenerateRestartSkipVotes,
    DrainStartupEvents,
    SeedCurrentRound,
    SeedFinalizedBlock,
    SeedReceivedCandidates,
    NotifyLastFinalized,
    CacheNotarCert,
    CacheCandidateBytes,
    RestoreStandstillCache,
    RestoreStartupVotes,
}

#[derive(Default)]
struct MockRecoveryListener {
    // Calls
    call_log: Vec<RecoveryCall>,
    on_vote_calls: Vec<(ValidatorIndex, Vote, Vec<u8>)>,
    local_flag_votes: Vec<Vote>,
    set_first_non_finalized: Option<SlotIndex>,
    set_first_nonannounced_window: Option<WindowIndex>,
    generate_skip_calls: usize,
    cached_notar: Vec<(SlotIndex, UInt256, Vec<u8>)>,
    cached_candidates: Vec<(SlotIndex, UInt256, Vec<u8>)>,
    restored_standstill_cache_votes_len: Option<usize>,
    drained_votes: Vec<Vote>,
    restored_votes: Vec<Vote>,
    seeded_current_round: Option<u32>,
    seeded_finalized_blocks: Vec<(SlotIndex, UInt256)>,
    seeded_received_candidates: Vec<FinalizedBlockRecord>,
    last_finalized_notification: Option<(SlotIndex, UInt256, u32)>,
}

impl MockRecoveryListener {
    fn with_drained_votes(mut self, votes: Vec<Vote>) -> Self {
        self.drained_votes = votes;
        self
    }
}

impl SessionStartupRecoveryListener for MockRecoveryListener {
    fn recovery_set_first_non_finalized_slot(&mut self, slot: SlotIndex) {
        self.call_log.push(RecoveryCall::SetFirstNonFinalized);
        self.set_first_non_finalized = Some(slot);
    }

    fn recovery_on_vote(
        &mut self,
        node_idx: ValidatorIndex,
        vote: Vote,
        _signature: Vec<u8>,
        raw_vote: crate::RawVoteData,
    ) -> VoteResult {
        self.call_log.push(RecoveryCall::OnVote);
        self.on_vote_calls.push((node_idx, vote, raw_vote.as_bytes().to_vec()));
        VoteResult::Applied
    }

    fn recovery_mark_slot_voted_on_restart(&mut self, vote: &Vote) {
        self.call_log.push(RecoveryCall::MarkLocalFlag);
        self.local_flag_votes.push(vote.clone());
    }

    fn recovery_set_first_nonannounced_window(&mut self, window: WindowIndex) {
        self.call_log.push(RecoveryCall::SetFirstNonannouncedWindow);
        self.set_first_nonannounced_window = Some(window);
    }

    fn recovery_generate_restart_skip_votes(&mut self) -> usize {
        self.call_log.push(RecoveryCall::GenerateRestartSkipVotes);
        self.generate_skip_calls += 1;
        123
    }

    fn recovery_drain_startup_events(&mut self) -> Vec<Vote> {
        self.call_log.push(RecoveryCall::DrainStartupEvents);
        self.drained_votes.clone()
    }

    fn recovery_restore_startup_votes(&mut self, votes: Vec<Vote>) {
        self.call_log.push(RecoveryCall::RestoreStartupVotes);
        self.restored_votes = votes;
    }

    fn recovery_seed_current_round(&mut self, round: u32) {
        self.call_log.push(RecoveryCall::SeedCurrentRound);
        self.seeded_current_round = Some(round);
    }

    fn recovery_seed_finalized_block(&mut self, slot: SlotIndex, block_hash: UInt256) {
        self.call_log.push(RecoveryCall::SeedFinalizedBlock);
        self.seeded_finalized_blocks.push((slot, block_hash));
    }

    fn recovery_seed_received_candidates(&mut self, finalized_blocks: &[FinalizedBlockRecord]) {
        self.call_log.push(RecoveryCall::SeedReceivedCandidates);
        self.seeded_received_candidates = finalized_blocks.to_vec();
    }

    fn recovery_seed_candidate_for_parent_resolution(
        &mut self,
        _candidate_id: RawCandidateId,
        _leader_idx: ValidatorIndex,
        _block_id: BlockIdExt,
        _parent: Option<RawCandidateId>,
        _is_empty: bool,
        _candidate_hash_data_bytes: Vec<u8>,
    ) {
        // Mock: no-op, parent resolution seeding not tracked in unit tests
    }

    fn recovery_notify_last_finalized(&mut self, slot: SlotIndex, block_hash: UInt256, seqno: u32) {
        self.call_log.push(RecoveryCall::NotifyLastFinalized);
        self.last_finalized_notification = Some((slot, block_hash, seqno));
    }

    fn recovery_finalize_parent_chain(&mut self) {
        // Mock: no-op for tests
    }

    fn recovery_cache_notarization_cert(
        &mut self,
        slot: SlotIndex,
        candidate_hash: UInt256,
        notar_cert_bytes: Vec<u8>,
    ) {
        self.call_log.push(RecoveryCall::CacheNotarCert);
        self.cached_notar.push((slot, candidate_hash, notar_cert_bytes));
    }

    fn recovery_seed_notarize_certificate(
        &mut self,
        _slot: SlotIndex,
        _candidate_hash: UInt256,
        _certificate: crate::certificate::NotarCertPtr,
    ) {
        // Mock: no-op, simplex_state is not available in tests
    }

    fn recovery_restore_receiver_standstill_cache(&mut self, votes: &[VoteRecord]) {
        self.call_log.push(RecoveryCall::RestoreStandstillCache);
        self.restored_standstill_cache_votes_len = Some(votes.len());
    }

    fn recovery_cache_candidate_bytes(
        &mut self,
        slot: SlotIndex,
        candidate_hash: UInt256,
        candidate_data_bytes: Vec<u8>,
    ) {
        self.call_log.push(RecoveryCall::CacheCandidateBytes);
        self.cached_candidates.push((slot, candidate_hash, candidate_data_bytes));
    }
}

fn make_vote_record(
    node_idx: ValidatorIndex,
    vote: Vote,
    session_id: &SessionId,
    key: &crate::PrivateKey,
) -> VoteRecord {
    let tl_vote = sign_vote(&vote, session_id, key).expect("sign_vote failed");
    let serialized = serialize_boxed(&tl_vote).expect("serialize vote failed");
    let vote_hash = UInt256::from_slice(&sha256_digest(&serialized));
    VoteRecord { vote_hash, data: serialized.into(), node_idx, seqno: 0 }
}

#[test]
fn test_apply_bootstrap_calls_expected_listener_methods_first_commit_strategy() {
    let session_id = SessionId::default();
    let options = SessionStartupRecoveryOptions::new(1);
    // Create 2-validator description where self is validator 1
    let self_idx = ValidatorIndex::new(1);
    let desc = create_test_desc_with_validators(2, 1);

    let key0: crate::PrivateKey = Ed25519KeyOption::generate().unwrap();
    let key1: crate::PrivateKey = Ed25519KeyOption::generate().unwrap();

    let vote0 = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(10),
        block_hash: UInt256::from([0xAA; 32]),
    });
    let vote1 = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(11),
        block_hash: UInt256::from([0xBB; 32]),
    });

    let votes = vec![
        make_vote_record(ValidatorIndex::new(0), vote0.clone(), &session_id, &key0),
        make_vote_record(self_idx, vote1.clone(), &session_id, &key1),
    ];

    let finalized_blocks = vec![
        FinalizedBlockRecord {
            candidate_id: make_candidate_id(5, 0x55),
            block_id: make_block_id(100),
            parent: None,
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: make_candidate_id(7, 0x77),
            block_id: make_block_id(101),
            parent: None,
            is_final: true,
        },
    ];

    let notar_certs = vec![NotarCertRecord {
        candidate_id: make_candidate_id(5, 0x55),
        notar_cert_bytes: vec![1, 2, 3].into(),
    }];

    let pool_state = Some(PoolStateRecord { first_nonannounced_window: WindowIndex::new(2) });

    let bootstrap = Bootstrap {
        finalized_blocks: finalized_blocks.clone(),
        candidate_infos: vec![],
        notar_certs,
        votes: votes.clone(),
        pool_state,
        candidate_payloads: vec![],
    };

    let proc = SessionStartupRecoveryProcessor::new(session_id, desc, options, bootstrap);

    let drained_votes =
        vec![Vote::Skip(crate::simplex_state::SkipVote { slot: SlotIndex::new(99) })];
    let mut listener = MockRecoveryListener::default().with_drained_votes(drained_votes.clone());

    proc.apply_bootstrap(&mut listener).expect("apply_bootstrap failed");

    // ------------------------------------------------------------------------
    // Ordering invariants (Phase 6.6 + last-finalized-cert sequencing)
    // ------------------------------------------------------------------------
    //
    // 1) All votes must be replayed BEFORE setting finalized boundary (Phase 6.6)
    let boundary_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::SetFirstNonFinalized)
        .expect("expected SetFirstNonFinalized call");
    let last_vote_pos = listener
        .call_log
        .iter()
        .rposition(|c| *c == RecoveryCall::OnVote)
        .expect("expected OnVote calls");
    assert!(
        last_vote_pos < boundary_pos,
        "Phase 6.6 invariant violated: boundary set before finishing vote replay"
    );

    // 2) Local flags must be applied AFTER boundary
    let first_local_flag_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::MarkLocalFlag)
        .expect("expected MarkLocalFlag call");
    assert!(
        boundary_pos < first_local_flag_pos,
        "Local flags must be applied after finalized boundary is set"
    );

    // 3) Last-finalized-cert notification must happen AFTER seeding finalized tracking set
    let last_seed_final_pos = listener
        .call_log
        .iter()
        .rposition(|c| *c == RecoveryCall::SeedFinalizedBlock)
        .expect("expected SeedFinalizedBlock calls");
    let cert1_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::NotifyLastFinalized)
        .expect("expected NotifyLastFinalized call");
    assert!(
        last_seed_final_pos < cert1_pos,
        "Last-finalized-cert notification must happen after seeding finalized blocks set"
    );

    // 4) Standstill cache rebuild must happen after last-finalized-cert notification.
    let standstill_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::RestoreStandstillCache)
        .expect("expected RestoreStandstillCache call");
    assert!(
        cert1_pos < standstill_pos,
        "Standstill cache rebuild must happen after last-finalized-cert notification"
    );

    // Step 1: global replay
    assert_eq!(listener.on_vote_calls.len(), 2);
    assert_eq!(listener.on_vote_calls[0].0, ValidatorIndex::new(0));
    assert_eq!(listener.on_vote_calls[0].1, vote0);
    assert_eq!(listener.on_vote_calls[1].0, self_idx);
    assert_eq!(listener.on_vote_calls[1].1, vote1);

    // Step 2: boundary set to max_slot + 1 (7 + 1 = 8)
    assert_eq!(listener.set_first_non_finalized, Some(SlotIndex::new(8)));

    // Step 3: local flags applied only for our votes
    assert_eq!(
        listener.local_flag_votes,
        vec![Vote::Notarize(NotarizeVote {
            slot: SlotIndex::new(11),
            block_hash: UInt256::from([0xBB; 32])
        })]
    );

    // Step 4: pool state + skip votes
    assert_eq!(listener.set_first_nonannounced_window, Some(WindowIndex::new(2)));
    assert_eq!(listener.generate_skip_calls, 1);

    // Step 5 & 11: drain/restore votes
    assert_eq!(listener.restored_votes, drained_votes);

    // Step 6: current_round compatibility hook seeded to 0
    assert_eq!(listener.seeded_current_round, Some(0));

    // Step 7: finalized_blocks set seeded (2 blocks)
    assert_eq!(listener.seeded_finalized_blocks.len(), 2);
    assert_eq!(listener.seeded_finalized_blocks[0].0, SlotIndex::new(5));
    assert_eq!(listener.seeded_finalized_blocks[1].0, SlotIndex::new(7));

    // Step 8: last finalized notification (must pick the last is_final=true record)
    assert_eq!(
        listener.last_finalized_notification,
        Some((SlotIndex::new(7), make_candidate_id(7, 0x77).hash, 101))
    );

    // Step 9: notar cert cache restored
    assert_eq!(listener.cached_notar.len(), 1);
    assert_eq!(listener.cached_notar[0].0, SlotIndex::new(5));
    assert_eq!(listener.cached_notar[0].2, vec![1, 2, 3]);

    // Step 10b: standstill cache rebuild invoked with full bootstrap votes slice
    assert_eq!(listener.restored_standstill_cache_votes_len, Some(2));

    assert!(listener.cached_candidates.is_empty());
}

#[test]
fn test_apply_bootstrap_seeds_persisted_empty_mc_chain_before_last_finalized_notification() {
    let session_id = SessionId::default();
    let options = SessionStartupRecoveryOptions::new(1);

    let c1 = make_candidate_id(10, 0xA1);
    let c2 = make_candidate_id(11, 0xA2);
    let c3 = make_candidate_id(12, 0xA3);
    let finalized_blocks = vec![
        FinalizedBlockRecord {
            candidate_id: c1.clone(),
            block_id: make_block_id(100),
            parent: None,
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: c2.clone(),
            block_id: make_block_id(100),
            parent: Some(c1.clone()),
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: c3.clone(),
            block_id: make_block_id(101),
            parent: Some(c2.clone()),
            is_final: true,
        },
    ];

    let bootstrap = Bootstrap {
        finalized_blocks: finalized_blocks.clone(),
        candidate_infos: vec![],
        notar_certs: vec![],
        votes: vec![],
        pool_state: None,
        candidate_payloads: vec![],
    };

    let desc = create_test_desc();
    let proc = SessionStartupRecoveryProcessor::new(session_id, desc, options, bootstrap);
    let mut listener = MockRecoveryListener::default().with_drained_votes(vec![]);

    proc.apply_bootstrap(&mut listener).expect("apply_bootstrap failed");

    assert_eq!(listener.seeded_received_candidates.len(), finalized_blocks.len());
    for (seeded, expected) in
        listener.seeded_received_candidates.iter().zip(finalized_blocks.iter())
    {
        assert_eq!(seeded.candidate_id, expected.candidate_id);
        assert_eq!(seeded.block_id, expected.block_id);
        assert_eq!(seeded.parent, expected.parent);
        assert_eq!(seeded.is_final, expected.is_final);
    }

    let seed_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::SeedReceivedCandidates)
        .expect("expected SeedReceivedCandidates call");
    let notify_pos = listener
        .call_log
        .iter()
        .position(|c| *c == RecoveryCall::NotifyLastFinalized)
        .expect("expected NotifyLastFinalized call");
    assert!(
        seed_pos < notify_pos,
        "received-candidate seeding must happen before the last-finalized notification"
    );
    assert_eq!(
        listener.last_finalized_notification,
        Some((c3.slot, c3.hash, 101)),
        "the last-finalized notification must still target the newest final record"
    );
}

#[test]
fn test_apply_bootstrap_does_not_generate_skip_votes_when_first_nonannounced_window_zero() {
    let session_id = SessionId::default();
    let options = SessionStartupRecoveryOptions::new(1);

    let bootstrap = Bootstrap {
        finalized_blocks: vec![FinalizedBlockRecord {
            candidate_id: make_candidate_id(1, 0x11),
            block_id: make_block_id(1),
            parent: None,
            is_final: true,
        }],
        candidate_infos: vec![],
        notar_certs: vec![],
        votes: vec![],
        pool_state: Some(PoolStateRecord { first_nonannounced_window: WindowIndex::new(0) }),
        candidate_payloads: vec![],
    };

    let desc = create_test_desc();
    let proc = SessionStartupRecoveryProcessor::new(session_id, desc, options, bootstrap);
    let mut listener = MockRecoveryListener::default().with_drained_votes(vec![]);

    proc.apply_bootstrap(&mut listener).expect("apply_bootstrap failed");
    assert_eq!(listener.set_first_nonannounced_window, Some(WindowIndex::new(0)));
    assert_eq!(listener.generate_skip_calls, 0);
    // Step 6: current_round compatibility hook seeded to 0
    assert_eq!(listener.seeded_current_round, Some(0));
    // Step 7: finalized_blocks set seeded (1 block)
    assert_eq!(listener.seeded_finalized_blocks.len(), 1);
    assert_eq!(listener.seeded_finalized_blocks[0].0, SlotIndex::new(1));

    // Step 8: last finalized notification for the only block
    assert_eq!(
        listener.last_finalized_notification,
        Some((SlotIndex::new(1), make_candidate_id(1, 0x11).hash, 1))
    );

    // No candidate bytes cached because candidate_infos is empty
    assert!(listener.cached_candidates.is_empty());

    // Invariant: first_nonannounced_window=0 => no restart skip generation call
    assert!(
        !listener.call_log.iter().any(|c| *c == RecoveryCall::GenerateRestartSkipVotes),
        "GenerateRestartSkipVotes must not be called when first_nonannounced_window=0"
    );
}

// ============================================================================
// Candidate bytes cache restoration: TL roundtrip + invariants
// ============================================================================

/// Listener that captures cached CandidateData bytes during candidate cache restoration.
#[derive(Default)]
struct CandidateCacheListener {
    cached_candidates: Vec<(SlotIndex, UInt256, Vec<u8>)>,
}

impl SessionStartupRecoveryListener for CandidateCacheListener {
    fn recovery_set_first_non_finalized_slot(&mut self, _slot: SlotIndex) {}

    fn recovery_on_vote(
        &mut self,
        _node_idx: ValidatorIndex,
        _vote: Vote,
        _signature: Vec<u8>,
        _raw_vote: crate::RawVoteData,
    ) -> VoteResult {
        VoteResult::Applied
    }

    fn recovery_mark_slot_voted_on_restart(&mut self, _vote: &Vote) {}

    fn recovery_set_first_nonannounced_window(&mut self, _window: WindowIndex) {}

    fn recovery_generate_restart_skip_votes(&mut self) -> usize {
        0
    }

    fn recovery_drain_startup_events(&mut self) -> Vec<Vote> {
        vec![]
    }

    fn recovery_restore_startup_votes(&mut self, _votes: Vec<Vote>) {}

    fn recovery_seed_current_round(&mut self, _round: u32) {}

    fn recovery_seed_finalized_block(&mut self, _slot: SlotIndex, _block_hash: UInt256) {}

    fn recovery_seed_received_candidates(&mut self, _finalized_blocks: &[FinalizedBlockRecord]) {}

    fn recovery_seed_candidate_for_parent_resolution(
        &mut self,
        _candidate_id: RawCandidateId,
        _leader_idx: ValidatorIndex,
        _block_id: BlockIdExt,
        _parent: Option<RawCandidateId>,
        _is_empty: bool,
        _candidate_hash_data_bytes: Vec<u8>,
    ) {
        // Not needed for candidate cache restoration tests
    }

    fn recovery_notify_last_finalized(
        &mut self,
        _slot: SlotIndex,
        _block_hash: UInt256,
        _seqno: u32,
    ) {
    }

    fn recovery_finalize_parent_chain(&mut self) {}

    fn recovery_cache_notarization_cert(
        &mut self,
        _slot: SlotIndex,
        _candidate_hash: UInt256,
        _notar_cert_bytes: Vec<u8>,
    ) {
    }

    fn recovery_seed_notarize_certificate(
        &mut self,
        _slot: SlotIndex,
        _candidate_hash: UInt256,
        _certificate: crate::certificate::NotarCertPtr,
    ) {
    }

    fn recovery_restore_receiver_standstill_cache(&mut self, _votes: &[VoteRecord]) {
        // No-op for candidate cache restoration tests
    }

    fn recovery_cache_candidate_bytes(
        &mut self,
        slot: SlotIndex,
        candidate_hash: UInt256,
        candidate_data_bytes: Vec<u8>,
    ) {
        self.cached_candidates.push((slot, candidate_hash, candidate_data_bytes));
    }
}

#[test]
fn test_restart_restore_candidate_bytes_roundtrip_empty_and_non_empty() {
    let session_id = SessionId::default();
    let options = SessionStartupRecoveryOptions::new(0);
    let desc = create_test_desc(); // single validator is enough for this test

    let shard = ShardIdent::masterchain();
    let max_size = 1_000_000;

    // Parent candidate id (used by both empty and non-empty for parent linkage)
    let parent_id = RawCandidateId { slot: SlotIndex::new(9), hash: UInt256::from([0xAA; 32]) };

    // ------------------------------------------------------------------------
    // Empty block record (CandidateHashDataEmpty)
    // ------------------------------------------------------------------------
    let empty_referenced_block = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: 50,
        root_hash: UInt256::from([0x10; 32]),
        file_hash: UInt256::from([0x11; 32]),
    };
    let empty_hash = crate::utils::compute_candidate_id_hash_empty(
        &empty_referenced_block,
        (parent_id.slot, &parent_id.hash),
    );
    let empty_candidate_id = RawCandidateId { slot: SlotIndex::new(10), hash: empty_hash };

    // ------------------------------------------------------------------------
    // Non-empty block record (CandidateHashDataOrdinary + real candidate TL bytes)
    // ------------------------------------------------------------------------
    let non_empty_round_seqno: i32 = 51; // used as block seqno by extract_block_info_from_candidate
    let non_empty_root_hash = UInt256::from([0x22; 32]);
    // Use valid BOC bytes — compress_candidate_data requires valid BOC input
    let non_empty_data = make_test_boc(b"block_data_bytes", BocFlags::all());
    let non_empty_collated = make_test_boc(b"collated_data_bytes", BocFlags::Crc32);
    let candidate_payload_bytes = make_validator_session_candidate_bytes(
        non_empty_round_seqno,
        non_empty_root_hash.clone(),
        non_empty_data.clone(),
        non_empty_collated.clone(),
    );

    let non_empty_hash = crate::utils::compute_candidate_id_hash_from_bytes(
        SlotIndex::new(11),
        &candidate_payload_bytes,
        Some((parent_id.slot, &parent_id.hash)),
        &shard,
        max_size,
        0,
    )
    .expect("compute_candidate_id_hash_from_bytes failed");
    let non_empty_candidate_id = RawCandidateId { slot: SlotIndex::new(11), hash: non_empty_hash };

    // Build block_id + collated_file_hash for CandidateHashDataOrdinary
    let non_empty_file_hash = UInt256::from_slice(&sha256_digest(&non_empty_data));
    let non_empty_collated_file_hash = UInt256::from_slice(&sha256_digest(&non_empty_collated));
    let non_empty_block_id = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: non_empty_round_seqno as u32,
        root_hash: non_empty_root_hash.clone(),
        file_hash: non_empty_file_hash.clone(),
    };

    // Bootstrap finalized blocks: empty + non-empty
    let finalized_blocks = vec![
        FinalizedBlockRecord {
            candidate_id: empty_candidate_id.clone(),
            block_id: empty_referenced_block.clone(),
            parent: Some(parent_id.clone()),
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: non_empty_candidate_id.clone(),
            block_id: non_empty_block_id.clone(),
            parent: Some(empty_candidate_id.clone()),
            is_final: true,
        },
    ];

    // CandidateInfo records for both candidates
    let empty_info = CandidateInfoRecord {
        candidate_id: empty_candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: make_candidate_hash_data_empty(
            empty_referenced_block.clone(),
            parent_id.clone(),
        ),
        signature: vec![0xE1; 64].into(),
    };

    let non_empty_info = CandidateInfoRecord {
        candidate_id: non_empty_candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: make_candidate_hash_data_with_parent(
            non_empty_block_id.clone(),
            non_empty_collated_file_hash.clone(),
            Some(parent_id.clone()),
        ),
        signature: vec![0xB1; 64].into(),
    };

    let bootstrap = Bootstrap {
        finalized_blocks: finalized_blocks.clone(),
        candidate_infos: vec![empty_info.clone(), non_empty_info.clone()],
        notar_certs: vec![],
        votes: vec![],
        pool_state: None,
        candidate_payloads: vec![],
    };

    let proc = SessionStartupRecoveryProcessor::new(session_id, desc, options, bootstrap);

    let mut listener = CandidateCacheListener::default();

    proc.apply_bootstrap(&mut listener).expect("apply_bootstrap failed");

    // Post-condition: only empty candidate cached; non-empty candidates are skipped
    // (simplex resolves non-empty candidates via peer overlay, not validator manager)
    assert_eq!(listener.cached_candidates.len(), 1);

    let (slot, _hash, bytes) = &listener.cached_candidates[0];
    let msg = deserialize_boxed(bytes).expect("deserialize CandidateData");
    let candidate_data = msg.downcast::<CandidateData>().expect("downcast CandidateData");

    match candidate_data {
        CandidateData::Consensus_Empty(empty) => {
            assert_eq!(SlotIndex::new(empty.slot as u32), *slot);
            assert_eq!(empty.signature, empty_info.signature);
            assert_eq!(empty.block, empty_referenced_block);

            // Parent is a CandidateId (boxed), verify it matches the empty hash data parent
            assert_eq!(SlotIndex::new(*empty.parent.slot() as u32), parent_id.slot);
            assert_eq!(empty.parent.hash(), &parent_id.hash);
        }
        CandidateData::Consensus_Block(_) => {
            panic!("non-empty block should not be cached during startup recovery");
        }
    }
}

#[test]
fn test_restart_restore_candidate_bytes_skips_non_empty_and_keeps_empty() {
    let session_id = SessionId::default();
    let options = SessionStartupRecoveryOptions::new(0);
    let desc = create_test_desc();

    let shard = ShardIdent::masterchain();
    let parent_id = RawCandidateId { slot: SlotIndex::new(9), hash: UInt256::from([0xAA; 32]) };

    // Empty candidate (valid)
    let empty_block = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: 7,
        root_hash: UInt256::from([0x10; 32]),
        file_hash: UInt256::from([0x11; 32]),
    };
    let empty_hash = crate::utils::compute_candidate_id_hash_empty(
        &empty_block,
        (parent_id.slot, &parent_id.hash),
    );
    let empty_candidate_id = RawCandidateId { slot: SlotIndex::new(10), hash: empty_hash };

    // Non-empty candidate (fetch will fail)
    let non_empty_candidate_id =
        RawCandidateId { slot: SlotIndex::new(11), hash: UInt256::from([0x22; 32]) };
    let non_empty_block_id = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: 8,
        root_hash: UInt256::from([0x33; 32]),
        file_hash: UInt256::from([0x44; 32]),
    };

    let finalized_blocks = vec![
        FinalizedBlockRecord {
            candidate_id: empty_candidate_id.clone(),
            block_id: empty_block.clone(),
            parent: Some(parent_id.clone()),
            is_final: true,
        },
        FinalizedBlockRecord {
            candidate_id: non_empty_candidate_id.clone(),
            block_id: non_empty_block_id.clone(),
            parent: Some(empty_candidate_id.clone()),
            is_final: true,
        },
    ];

    let empty_info = CandidateInfoRecord {
        candidate_id: empty_candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: make_candidate_hash_data_empty(empty_block.clone(), parent_id.clone()),
        signature: vec![0xE1; 64].into(),
    };

    let non_empty_info = CandidateInfoRecord {
        candidate_id: non_empty_candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: make_candidate_hash_data_with_parent(
            non_empty_block_id.clone(),
            UInt256::from([0x55; 32]),
            Some(parent_id.clone()),
        ),
        signature: vec![0xB1; 64].into(),
    };

    let bootstrap = Bootstrap {
        finalized_blocks,
        candidate_infos: vec![empty_info.clone(), non_empty_info],
        notar_certs: vec![],
        votes: vec![],
        pool_state: None,
        candidate_payloads: vec![],
    };

    let proc = SessionStartupRecoveryProcessor::new(session_id, desc, options, bootstrap);

    // Listener that always fails fetch by calling callback(Err(_))
    #[derive(Default)]
    struct FailFetchListener {
        cached: Vec<(SlotIndex, UInt256, Vec<u8>)>,
    }

    impl SessionStartupRecoveryListener for FailFetchListener {
        fn recovery_set_first_non_finalized_slot(&mut self, _slot: SlotIndex) {}
        fn recovery_on_vote(
            &mut self,
            _node_idx: ValidatorIndex,
            _vote: Vote,
            _signature: Vec<u8>,
            _raw_vote: crate::RawVoteData,
        ) -> VoteResult {
            VoteResult::Applied
        }
        fn recovery_mark_slot_voted_on_restart(&mut self, _vote: &Vote) {}
        fn recovery_set_first_nonannounced_window(&mut self, _window: WindowIndex) {}
        fn recovery_generate_restart_skip_votes(&mut self) -> usize {
            0
        }
        fn recovery_drain_startup_events(&mut self) -> Vec<Vote> {
            vec![]
        }
        fn recovery_restore_startup_votes(&mut self, _votes: Vec<Vote>) {}
        fn recovery_seed_current_round(&mut self, _round: u32) {}
        fn recovery_seed_finalized_block(&mut self, _slot: SlotIndex, _block_hash: UInt256) {}
        fn recovery_seed_received_candidates(
            &mut self,
            _finalized_blocks: &[FinalizedBlockRecord],
        ) {
        }
        fn recovery_seed_candidate_for_parent_resolution(
            &mut self,
            _candidate_id: RawCandidateId,
            _leader_idx: ValidatorIndex,
            _block_id: BlockIdExt,
            _parent: Option<RawCandidateId>,
            _is_empty: bool,
            _candidate_hash_data_bytes: Vec<u8>,
        ) {
        }
        fn recovery_notify_last_finalized(
            &mut self,
            _slot: SlotIndex,
            _block_hash: UInt256,
            _seqno: u32,
        ) {
        }
        fn recovery_finalize_parent_chain(&mut self) {}
        fn recovery_cache_notarization_cert(
            &mut self,
            _slot: SlotIndex,
            _candidate_hash: UInt256,
            _notar_cert_bytes: Vec<u8>,
        ) {
        }
        fn recovery_seed_notarize_certificate(
            &mut self,
            _slot: SlotIndex,
            _candidate_hash: UInt256,
            _certificate: crate::certificate::NotarCertPtr,
        ) {
        }
        fn recovery_restore_receiver_standstill_cache(&mut self, _votes: &[VoteRecord]) {
            // No-op for this test
        }
        fn recovery_cache_candidate_bytes(
            &mut self,
            slot: SlotIndex,
            candidate_hash: UInt256,
            candidate_data_bytes: Vec<u8>,
        ) {
            self.cached.push((slot, candidate_hash, candidate_data_bytes));
        }
    }

    let mut listener = FailFetchListener::default();
    proc.apply_bootstrap(&mut listener).expect("apply_bootstrap failed");

    // Post-condition: empty candidate cached, non-empty skipped (not fetched)
    assert_eq!(listener.cached.len(), 1);
    assert_eq!(listener.cached[0].0, SlotIndex::new(10));

    // And bytes decode as CandidateData::Consensus_Empty
    let msg = deserialize_boxed(&listener.cached[0].2).expect("deserialize CandidateData");
    let candidate_data = msg.downcast::<CandidateData>().expect("downcast CandidateData");
    assert!(
        matches!(candidate_data, CandidateData::Consensus_Empty(_)),
        "expected empty candidate to be reconstructed"
    );
}
