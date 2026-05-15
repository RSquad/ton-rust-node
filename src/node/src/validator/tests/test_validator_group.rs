/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use super::*;
use crate::validator::consensus::ConsensusCommonFactory;
use std::{
    any::Any,
    sync::{atomic::AtomicBool, Mutex},
    time::Duration,
};
use ton_block::{signature::SigPubKey, validators::ValidatorDescr, Ed25519KeyOption, KeyId};

#[derive(Default)]
struct DummyEngine;

#[async_trait::async_trait]
impl EngineOperations for DummyEngine {}

#[derive(Default)]
struct MockSimplexSession {
    requests: Mutex<Vec<(BlockIdExt, EnsureCandidateAvailabilityOptions)>>,
    stopped: AtomicBool,
}

impl Display for MockSimplexSession {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "mock_simplex_session")
    }
}

impl Session for MockSimplexSession {
    fn start(&self, _prev_blocks: Vec<BlockIdExt>, _min_masterchain_block_id: BlockIdExt) {}

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn stop_async(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn destroy(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl super::consensus::SimplexSession for MockSimplexSession {
    fn notify_mc_finalized(&self, _applied_top: BlockIdExt) {}

    fn ensure_candidate_available(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        self.requests.lock().expect("requests lock poisoned").push((block_id, opts));
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    fn is_panicked(&self) -> bool {
        false
    }
}

fn make_group_impl_for_start_tests() -> ValidatorGroupImpl {
    ValidatorGroupImpl::new(
        &KeyId::from_data([0u8; 32]),
        ShardIdent::masterchain(),
        1,
        UInt256::default(),
        false,
        false,
        ConsensusType::Catchain,
    )
}

fn make_simplex_group_for_resolver_tests() -> Arc<ValidatorGroup> {
    let local_key: PrivateKey = Ed25519KeyOption::generate().expect("key must be generated");
    let validator_descr = ValidatorDescr::with_params(
        SigPubKey::from_bytes(local_key.pub_key().expect("pubkey bytes"))
            .expect("valid sig pubkey"),
        1,
        None,
    );
    let validator_set =
        ValidatorSet::with_cc_seqno(0, 0, 0, 1, vec![validator_descr]).expect("validator set");

    let session_info = Arc::new(GeneralSessionInfo {
        shard: ShardIdent::masterchain(),
        opts_hash: UInt256::default(),
        catchain_seqno: 1,
        key_seqno: 0,
        max_vertical_seqno: 0,
    });

    let group = ValidatorGroup::new(
        session_info,
        local_key,
        UInt256::rand(),
        UInt256::rand(),
        validator_set,
        ConsensusOptions::Simplex(Default::default()),
        Arc::new(DummyEngine),
        false,
    );
    Arc::new(group)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resolver_cache_bridge_requests_simplex_candidate_availability() {
    let group = make_simplex_group_for_resolver_tests();
    let mock_session = Arc::new(MockSimplexSession::default());
    let session_ptr: super::consensus::SimplexSessionPtr = mock_session.clone();
    let session_holder: SessionHolderPtr =
        Arc::new(super::consensus::SessionHolder::simplex(session_ptr));
    group
        .group_impl
        .execute_sync(move |group_impl| {
            group_impl.session = Some(session_holder.clone());
        })
        .await;

    let backend: Arc<dyn ResolverBackend> = group.clone();
    group.state_resolver_cache.lock().await.set_backend(Arc::downgrade(&backend));

    let block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 77, UInt256::rand(), UInt256::rand());
    group
        .on_candidate_observed(
            block_id.clone(),
            ConsensusCommonFactory::create_block_payload(vec![1, 2, 3]),
            ConsensusCommonFactory::create_block_payload(Vec::new()),
            CandidateObservedFlags {
                body_present: true,
                parent_ready: false,
                local_collated: false,
            },
        )
        .await;

    {
        let cache = group.state_resolver_cache.lock().await;
        assert!(cache.try_get_entry(&block_id).is_some(), "observed candidate must be cached");
        cache.request_availability(&block_id, ResolverPurpose::SimplexValidationParent);
    }

    let mut attempts = 0u32;
    loop {
        let requests = mock_session.requests.lock().expect("requests lock poisoned");
        if !requests.is_empty() {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].0, block_id);
            assert_eq!(requests[0].1.purpose, ResolverPurpose::SimplexValidationParent);
            assert!(
                requests[0].1.include_parent_chain,
                "resolver requests must include parent chain for repair"
            );
            break;
        }
        drop(requests);
        attempts += 1;
        assert!(attempts < 50, "timed out waiting for resolver bridge request");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn test_stale_finalized_rebroadcast_suppression_none_allows() {
    assert!(!should_suppress_stale_finalized_rebroadcast(None, 0));
    assert!(!should_suppress_stale_finalized_rebroadcast(None, 100));
}

#[test]
fn test_stale_finalized_rebroadcast_suppression_matches_cpp_cutoff() {
    assert!(!should_suppress_stale_finalized_rebroadcast(Some(0), 0));
    assert!(!should_suppress_stale_finalized_rebroadcast(Some(1), 0));
    assert!(!should_suppress_stale_finalized_rebroadcast(Some(2), 0));

    // C++ condition uses strict `< last_mc_finalized_seqno - 2`.
    assert!(!should_suppress_stale_finalized_rebroadcast(Some(10), 8));
    assert!(should_suppress_stale_finalized_rebroadcast(Some(10), 7));
}

#[test]
fn test_sync_last_notified_mc_finalized_seqno_is_monotonic() {
    let mut group = make_group_impl_for_start_tests();

    let top10 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x03; 32]),
        UInt256::from([0x04; 32]),
    );
    sync_last_notified_mc_finalized_seqno(&mut group, &top10);
    assert_eq!(group.last_notified_mc_finalized_seqno, Some(10));

    let top7 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        7,
        UInt256::from([0x05; 32]),
        UInt256::from([0x06; 32]),
    );
    sync_last_notified_mc_finalized_seqno(&mut group, &top7);
    assert_eq!(
        group.last_notified_mc_finalized_seqno,
        Some(10),
        "external notify cursor must stay monotonic"
    );
}

#[test]
fn test_mc_fork_prevention_none_allows() {
    let parent0 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0x01; 32]),
        UInt256::from([0x02; 32]),
    );
    let parent100 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        100,
        UInt256::from([0x11; 32]),
        UInt256::from([0x12; 32]),
    );
    assert!(!should_reject_stale_mc_candidate(None, &parent0));
    assert!(!should_reject_stale_mc_candidate(None, &parent100));
}

#[test]
fn test_mc_fork_prevention_equal_allows() {
    let block = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x21; 32]),
        UInt256::from([0x22; 32]),
    );
    assert!(!should_reject_stale_mc_candidate(Some(&block), &block));
}

#[test]
fn test_mc_fork_prevention_ahead_allows() {
    let accepted = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x31; 32]),
        UInt256::from([0x32; 32]),
    );
    let parent11 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0x41; 32]),
        UInt256::from([0x42; 32]),
    );
    let parent100 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        100,
        UInt256::from([0x51; 32]),
        UInt256::from([0x52; 32]),
    );
    assert!(!should_reject_stale_mc_candidate(Some(&accepted), &parent11));
    assert!(!should_reject_stale_mc_candidate(Some(&accepted), &parent100));
}

#[test]
fn test_mc_fork_prevention_stale_rejects() {
    let accepted10 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x61; 32]),
        UInt256::from([0x62; 32]),
    );
    let parent9 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        9,
        UInt256::from([0x71; 32]),
        UInt256::from([0x72; 32]),
    );
    let parent0 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0x81; 32]),
        UInt256::from([0x82; 32]),
    );
    let accepted100 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        100,
        UInt256::from([0x91; 32]),
        UInt256::from([0x92; 32]),
    );
    let parent50 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        50,
        UInt256::from([0xa1; 32]),
        UInt256::from([0xa2; 32]),
    );
    assert!(should_reject_stale_mc_candidate(Some(&accepted10), &parent9));
    assert!(should_reject_stale_mc_candidate(Some(&accepted10), &parent0));
    assert!(should_reject_stale_mc_candidate(Some(&accepted100), &parent50));
}

#[test]
fn test_mc_fork_prevention_same_seqno_conflicting_parent_rejects() {
    let accepted = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0xb1; 32]),
        UInt256::from([0xb2; 32]),
    );
    let stale_same_seqno = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0xb0; 32]),
        UInt256::from([0xb2; 32]),
    );
    let ahead_same_seqno = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0xb2; 32]),
        UInt256::from([0xb2; 32]),
    );
    assert!(should_reject_stale_mc_candidate(Some(&accepted), &stale_same_seqno));
    assert!(
        !should_reject_stale_mc_candidate(Some(&accepted), &ahead_same_seqno),
        "strict C++ parity compares full BlockIdExt order: only parent<head is stale"
    );
}

#[tokio::test]
async fn test_wait_for_mc_validation_parent_waits_for_accepted_head() {
    let candidate_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        8,
        UInt256::from([0x41; 32]),
        UInt256::from([0x42; 32]),
    );
    let candidate_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        7,
        UInt256::from([0x51; 32]),
        UInt256::from([0x52; 32]),
    );
    let (tx, rx) = tokio::sync::watch::channel(None);
    let wait_parent_block_id = candidate_parent_block_id.clone();

    let wait_task = tokio::spawn(async move {
        wait_for_mc_validation_parent(rx, &candidate_block_id, &wait_parent_block_id).await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !wait_task.is_finished(),
        "C++ parity: validation must wait until accepted head reaches the candidate parent"
    );

    tx.send(Some(candidate_parent_block_id)).expect("watch receiver should still be alive");
    assert!(wait_task.await.expect("wait task join").is_ok());
}

#[tokio::test]
async fn test_wait_for_mc_validation_parent_genesis_parent_waits_until_accepted_head_arrives() {
    let candidate_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x53; 32]),
        UInt256::from([0x54; 32]),
    );
    let genesis_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0xdb; 32]),
        UInt256::from([0xdc; 32]),
    );
    let (tx, rx) = tokio::sync::watch::channel(None);

    let wait_parent_block_id = genesis_parent_block_id.clone();
    let wait_task = tokio::spawn(async move {
        wait_for_mc_validation_parent(rx, &candidate_block_id, &wait_parent_block_id).await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !wait_task.is_finished(),
        "strict C++ parity waits while accepted_head < expected_parent (including genesis)"
    );

    tx.send(Some(genesis_parent_block_id)).expect("watch receiver should still be alive");
    assert!(wait_task.await.expect("wait task join").is_ok());
}

#[tokio::test]
async fn test_wait_for_mc_validation_parent_rejects_stale_branch() {
    let candidate_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0x61; 32]),
        UInt256::from([0x62; 32]),
    );
    let candidate_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        9,
        UInt256::from([0x71; 32]),
        UInt256::from([0x72; 32]),
    );
    let accepted_head = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x81; 32]),
        UInt256::from([0x82; 32]),
    );
    let (_tx, rx) = tokio::sync::watch::channel(Some(accepted_head));

    let err = wait_for_mc_validation_parent(rx, &candidate_block_id, &candidate_parent_block_id)
        .await
        .expect_err("stale parent must be rejected");
    assert!(
        err.to_string().contains("MC fork prevention"),
        "error should explain why the stale branch was rejected"
    );
}

#[tokio::test]
async fn test_wait_for_mc_validation_parent_rejects_same_seqno_conflicting_parent() {
    let candidate_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0x91; 32]),
        UInt256::from([0x92; 32]),
    );
    let candidate_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x11; 32]),
        UInt256::from([0x12; 32]),
    );
    let accepted_head = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x11; 32]),
        UInt256::from([0x13; 32]),
    );
    let (_tx, rx) = tokio::sync::watch::channel(Some(accepted_head));

    let err = wait_for_mc_validation_parent(rx, &candidate_block_id, &candidate_parent_block_id)
        .await
        .expect_err("same-seqno conflicting parent must be rejected");
    assert!(
        err.to_string().contains("MC fork prevention"),
        "error should explain why the conflicting same-seqno branch was rejected"
    );
}

#[tokio::test]
async fn test_wait_for_mc_validation_parent_same_seqno_higher_hash_waits_then_resolves() {
    let candidate_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0xa1; 32]),
        UInt256::from([0xa2; 32]),
    );
    let candidate_parent_block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x33; 32]),
        UInt256::from([0x12; 32]),
    );
    let accepted_head = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x11; 32]),
        UInt256::from([0x13; 32]),
    );
    let (tx, rx) = tokio::sync::watch::channel(Some(accepted_head));
    let wait_parent_block_id = candidate_parent_block_id.clone();

    let wait_task = tokio::spawn(async move {
        wait_for_mc_validation_parent(rx, &candidate_block_id, &wait_parent_block_id).await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !wait_task.is_finished(),
        "when candidate_parent > accepted_head in full BlockIdExt order, strict C++ parity waits"
    );

    tx.send(Some(candidate_parent_block_id)).expect("watch receiver should still be alive");
    assert!(wait_task.await.expect("wait task join").is_ok());
}

#[test]
fn test_sync_last_accepted_mc_head_from_block_is_monotonic() {
    let mut group = make_group_impl_for_start_tests();

    let top10 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x11; 32]),
        UInt256::from([0x12; 32]),
    );
    sync_last_accepted_mc_head_from_block(&mut group, &top10);
    assert_eq!(group.last_accepted_mc_seqno, Some(10));
    assert_eq!(group.last_accepted_mc_block_id.as_ref(), Some(&top10));

    let top7 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        7,
        UInt256::from([0x21; 32]),
        UInt256::from([0x22; 32]),
    );
    sync_last_accepted_mc_head_from_block(&mut group, &top7);
    assert_eq!(group.last_accepted_mc_seqno, Some(10));
    assert_eq!(group.last_accepted_mc_block_id.as_ref(), Some(&top10));

    let top10_new_hash = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        10,
        UInt256::from([0x11; 32]),
        UInt256::from([0x13; 32]),
    );
    sync_last_accepted_mc_head_from_block(&mut group, &top10_new_hash);
    assert_eq!(group.last_accepted_mc_seqno, Some(10));
    assert_eq!(group.last_accepted_mc_block_id.as_ref(), Some(&top10_new_hash));

    let top11 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        11,
        UInt256::from([0x31; 32]),
        UInt256::from([0x32; 32]),
    );
    sync_last_accepted_mc_head_from_block(&mut group, &top11);
    assert_eq!(group.last_accepted_mc_seqno, Some(11));
    assert_eq!(group.last_accepted_mc_block_id.as_ref(), Some(&top11));
}

#[test]
fn test_sync_last_accepted_mc_head_from_block_seeds_genesis_head() {
    let mut group = make_group_impl_for_start_tests();

    let genesis = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0xdd; 32]),
        UInt256::from([0xde; 32]),
    );
    sync_last_accepted_mc_head_from_block(&mut group, &genesis);

    assert_eq!(group.last_accepted_mc_seqno, Some(0));
    assert_eq!(group.last_accepted_mc_block_id.as_ref(), Some(&genesis));
}

#[test]
fn test_initial_accepted_mc_head_from_start_inputs_falls_back_to_min_mc_when_prev_empty() {
    let shard = ShardIdent::masterchain();
    let min_mc = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0xca; 32]),
        UInt256::from([0xcb; 32]),
    );
    let prev: Vec<BlockIdExt> = Vec::new();

    let initial = initial_accepted_mc_head_from_start_inputs(&shard, &prev, &min_mc);
    assert_eq!(
        initial.as_ref(),
        Some(&min_mc),
        "strict C++ parity: startup MC accepted head must not remain None when prevs are empty"
    );
}

#[test]
fn test_initial_accepted_mc_head_from_start_inputs_prefers_prev_head_when_available() {
    let shard = ShardIdent::masterchain();
    let min_mc = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0xca; 32]),
        UInt256::from([0xcb; 32]),
    );
    let prev_head = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0xda; 32]),
        UInt256::from([0xdb; 32]),
    );
    let prev = vec![prev_head.clone()];

    let initial = initial_accepted_mc_head_from_start_inputs(&shard, &prev, &min_mc);
    assert_eq!(
        initial.as_ref(),
        Some(&prev_head),
        "startup MC accepted head should use exact prev head when present"
    );
}

#[test]
fn test_initial_accepted_mc_head_from_start_inputs_non_masterchain_returns_none() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let min_mc = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::from([0xca; 32]),
        UInt256::from([0xcb; 32]),
    );
    let prev = vec![BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0xda; 32]),
        UInt256::from([0xdb; 32]),
    )];

    let initial = initial_accepted_mc_head_from_start_inputs(&shard, &prev, &min_mc);
    assert!(initial.is_none(), "non-masterchain groups must not seed MC accepted head");
}

#[test]
fn test_publish_accepted_mc_head_persists_value_without_active_receivers() {
    let group = make_simplex_group_for_resolver_tests();
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        7,
        UInt256::from([0x41; 32]),
        UInt256::from([0x42; 32]),
    );

    // No receiver is held across this call, so plain watch::send would drop the update.
    group.publish_accepted_mc_head(Some(block_id.clone()));

    let rx = group.accepted_mc_block_tx.subscribe();
    assert_eq!(
        rx.borrow().as_ref(),
        Some(&block_id),
        "accepted MC head must be retained for future subscribers"
    );
}

#[test]
fn test_publish_accepted_mc_seqno_persists_value_without_active_receivers() {
    let group = make_simplex_group_for_resolver_tests();
    group.publish_accepted_mc_seqno(Some(11));

    let rx = group.accepted_mc_seqno_tx.subscribe();
    assert_eq!(*rx.borrow(), Some(11), "accepted MC seqno must be retained for future subscribers");
}

#[test]
fn test_prepare_start_immediate_keeps_created_and_marks_pending() {
    let mut group = make_group_impl_for_start_tests();

    assert!(group.prepare_start());
    assert!(group.status == ValidatorGroupStatus::Created);
    assert!(group.start_pending);
}

#[test]
fn test_prepare_start_keeps_created_status() {
    let mut group = make_group_impl_for_start_tests();

    assert!(group.prepare_start());
    assert!(group.status == ValidatorGroupStatus::Created);
    assert!(group.start_pending);
}

#[test]
fn test_prepare_start_rejects_duplicate_pending_start() {
    let mut group = make_group_impl_for_start_tests();

    assert!(group.prepare_start());
    assert!(!group.prepare_start());
    assert!(group.status == ValidatorGroupStatus::Created);
    assert!(group.start_pending);
}

#[test]
fn test_reset_after_start_failure_restores_retryable_state() {
    let mut group = make_group_impl_for_start_tests();

    assert!(group.prepare_start());
    group.reset_after_start_failure();

    assert!(group.status == ValidatorGroupStatus::Created);
    assert!(!group.start_pending);
    assert!(group.session.is_none());
}

// --- Status ordering / transition table tests (WS6) ---

#[test]
fn test_status_ordering_is_monotonic() {
    let states = [
        ValidatorGroupStatus::Created,
        ValidatorGroupStatus::EngineCreated,
        ValidatorGroupStatus::Sync,
        ValidatorGroupStatus::Active,
        ValidatorGroupStatus::Stopping,
        ValidatorGroupStatus::Stopped,
    ];
    for i in 0..states.len() {
        for j in i + 1..states.len() {
            assert!(states[i] < states[j], "{} must be < {}", states[i], states[j]);
        }
    }
}

#[test]
fn test_before_allows_forward_transitions() {
    let created = ValidatorGroupStatus::Created;
    let engine_created = ValidatorGroupStatus::EngineCreated;
    let sync = ValidatorGroupStatus::Sync;
    let active = ValidatorGroupStatus::Active;
    let stopping = ValidatorGroupStatus::Stopping;

    assert!(created.before(&engine_created));
    assert!(engine_created.before(&sync));
    assert!(sync.before(&active));
    assert!(active.before(&stopping));
}

#[test]
fn test_before_rejects_backward_transitions() {
    let sync = ValidatorGroupStatus::Sync;
    let active = ValidatorGroupStatus::Active;
    let created = ValidatorGroupStatus::Created;

    assert!(!active.before(&sync));
    assert!(!sync.before(&created));
}

#[test]
fn test_engine_created_state_between_created_and_sync() {
    let created = ValidatorGroupStatus::Created;
    let engine_created = ValidatorGroupStatus::EngineCreated;
    let sync = ValidatorGroupStatus::Sync;

    assert!(created < engine_created);
    assert!(engine_created < sync);
    assert!(created.before(&engine_created));
    assert!(engine_created.before(&sync));
}

#[test]
fn test_prepare_start_accepts_engine_created_state() {
    let mut group = make_group_impl_for_start_tests();
    group.status = ValidatorGroupStatus::EngineCreated;

    assert!(group.prepare_start());
    assert!(group.status == ValidatorGroupStatus::EngineCreated);
    assert!(group.start_pending);
}

#[test]
fn test_prepare_start_rejects_sync_and_later_states() {
    for status in [
        ValidatorGroupStatus::Sync,
        ValidatorGroupStatus::Active,
        ValidatorGroupStatus::Stopping,
        ValidatorGroupStatus::Stopped,
    ] {
        let mut group = make_group_impl_for_start_tests();
        group.status = status;
        assert!(!group.prepare_start(), "prepare_start should reject status {}", status);
    }
}

// --- Stale-future culling predicate tests (mirrors manager.cpp equal+related) ---

/// Reproduces the stale-future culling predicate from validator_manager.rs
/// to verify correctness in isolation with various shard topologies.
fn should_cull_future(
    active_shard: &ShardIdent,
    active_cc: u32,
    future_shard: &ShardIdent,
    future_cc: u32,
) -> bool {
    let shards_equal = active_shard == future_shard;
    let shards_related =
        active_shard.is_ancestor_for(future_shard) || future_shard.is_ancestor_for(active_shard);
    let equal_condition = shards_equal && active_cc >= future_cc;
    let related_condition = shards_related && active_cc > future_cc;
    equal_condition || related_condition
}

#[test]
fn test_cull_same_shard_equal_seqno() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    assert!(should_cull_future(&shard, 5, &shard, 5));
}

#[test]
fn test_cull_same_shard_higher_active_seqno() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    assert!(should_cull_future(&shard, 6, &shard, 5));
}

#[test]
fn test_no_cull_same_shard_lower_active_seqno() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    assert!(!should_cull_future(&shard, 4, &shard, 5));
}

#[test]
fn test_cull_ancestor_shard_higher_seqno() {
    let parent = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let child = ShardIdent::with_tagged_prefix(0, 0x4000_0000_0000_0000).unwrap();
    assert!(parent.is_ancestor_for(&child));
    assert!(should_cull_future(&parent, 6, &child, 5));
}

#[test]
fn test_cull_descendant_shard_higher_seqno() {
    let parent = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let child = ShardIdent::with_tagged_prefix(0, 0x4000_0000_0000_0000).unwrap();
    assert!(should_cull_future(&child, 6, &parent, 5));
}

#[test]
fn test_no_cull_related_shard_equal_seqno() {
    let parent = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let child = ShardIdent::with_tagged_prefix(0, 0x4000_0000_0000_0000).unwrap();
    // For related (non-equal) shards, the condition is strict >
    assert!(!should_cull_future(&parent, 5, &child, 5));
}

#[test]
fn test_no_cull_unrelated_shards() {
    let shard_a = ShardIdent::with_tagged_prefix(0, 0x4000_0000_0000_0000).unwrap();
    let shard_b = ShardIdent::with_tagged_prefix(0, 0xC000_0000_0000_0000).unwrap();
    assert!(!shard_a.is_ancestor_for(&shard_b));
    assert!(!shard_b.is_ancestor_for(&shard_a));
    assert!(!should_cull_future(&shard_a, 100, &shard_b, 1));
}

#[test]
fn test_no_cull_different_workchain() {
    let shard_wc0 = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let shard_wc1 = ShardIdent::with_tagged_prefix(1, 0x8000_0000_0000_0000).unwrap();
    assert!(!should_cull_future(&shard_wc0, 10, &shard_wc1, 5));
}

#[test]
fn test_metric_label_covers_all_states() {
    let states = vec![
        (ValidatorGroupStatus::Created, "created"),
        (ValidatorGroupStatus::EngineCreated, "engine_created"),
        (ValidatorGroupStatus::Sync, "sync"),
        (ValidatorGroupStatus::Active, "active"),
        (ValidatorGroupStatus::Stopping, "stopping"),
        (ValidatorGroupStatus::Stopped, "stopped"),
    ];
    for (status, expected_label) in states {
        assert_eq!(status.metric_label(), expected_label, "metric_label mismatch for {}", status);
    }
}
