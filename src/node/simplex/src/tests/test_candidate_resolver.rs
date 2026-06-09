/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for candidate resolver cache functionality
//!
//! These tests verify the `CandidateResolverCache` correctly stores and retrieves
//! candidate data and notarization certificates for responding to queries.

use crate::{
    block::{SlotIndex, ValidatorIndex},
    certificate::{Certificate, NotarCert, VoteSignature},
    simplex_state::{NotarizeVote, Vote},
    utils::{
        compute_candidate_id_hash, compute_candidate_id_hash_empty, sign_candidate_u32, sign_vote,
    },
    PublicKey, SessionId, SessionNode,
};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};
use ton_api::{
    serialize_boxed,
    ton::{
        consensus::{
            candidatedata::{Block as CandidateDataBlock, Empty as CandidateDataEmpty},
            candidateid::CandidateId as TlCandidateId,
            overlayid::OverlayId,
            CandidateData, CandidateParent,
        },
        pub_::publickey::Overlay,
    },
    IntoBoxed,
};
use ton_block::{BlockIdExt, Ed25519KeyOption, KeyId, ShardIdent, UInt256, ZeroizingBytes};

#[test]
fn test_candidate_resolver_cache_new() {
    // Test that cache can be created
    let cache = super::CandidateResolverCache::new();
    let hash = UInt256::rand();

    // Cache should be empty initially
    assert!(cache.get_candidate(SlotIndex::new(0), &hash).is_none());
    assert!(cache.get_notar_cert(SlotIndex::new(0), &hash).is_none());
}

#[test]
fn test_candidate_resolver_cache_candidate() {
    let mut cache = super::CandidateResolverCache::new();
    let hash = UInt256::rand();
    let data = vec![1, 2, 3, 4, 5];

    // Cache candidate
    cache.cache_candidate(SlotIndex::new(5), hash.clone(), data.clone());

    // Retrieve cached candidate
    let retrieved = cache.get_candidate(SlotIndex::new(5), &hash);
    assert_eq!(retrieved, Some(&data));

    // Different slot should not find it
    assert!(cache.get_candidate(SlotIndex::new(4), &hash).is_none());
    assert!(cache.get_candidate(SlotIndex::new(6), &hash).is_none());

    // Different hash should not find it
    let other_hash = UInt256::rand();
    assert!(cache.get_candidate(SlotIndex::new(5), &other_hash).is_none());
}

#[test]
fn test_candidate_resolver_cache_notar_cert() {
    let mut cache = super::CandidateResolverCache::new();
    let hash = UInt256::rand();
    let cert_data = vec![10, 20, 30, 40];

    // Cache notarization certificate
    cache.cache_notar_cert(SlotIndex::new(3), hash.clone(), cert_data.clone());

    // Retrieve cached certificate
    let retrieved = cache.get_notar_cert(SlotIndex::new(3), &hash);
    assert_eq!(retrieved.cloned(), Some(cert_data.clone()));

    // Different slot should not find it
    assert!(cache.get_notar_cert(SlotIndex::new(2), &hash).is_none());
    assert!(cache.get_notar_cert(SlotIndex::new(4), &hash).is_none());

    // Different hash should not find it
    let other_hash = UInt256::rand();
    assert!(cache.get_notar_cert(SlotIndex::new(3), &other_hash).is_none());
}

#[test]
fn test_candidate_resolver_cache_separate_storage() {
    // Test that candidates and notar certs are stored separately
    let mut cache = super::CandidateResolverCache::new();
    let hash = UInt256::rand();
    let candidate_data = vec![1, 1, 1];
    let notar_data = vec![2, 2, 2];

    // Store both types for same slot and hash
    cache.cache_candidate(SlotIndex::new(7), hash.clone(), candidate_data.clone());
    cache.cache_notar_cert(SlotIndex::new(7), hash.clone(), notar_data.clone());

    // Both should be retrievable independently
    assert_eq!(cache.get_candidate(SlotIndex::new(7), &hash), Some(&candidate_data));
    assert_eq!(cache.get_notar_cert(SlotIndex::new(7), &hash).cloned(), Some(notar_data));
}

#[test]
fn test_candidate_resolver_cache_overwrite() {
    let mut cache = super::CandidateResolverCache::new();
    let hash = UInt256::rand();
    let data1 = vec![1, 2, 3];
    let data2 = vec![4, 5, 6, 7, 8];

    // Store initial data
    cache.cache_candidate(SlotIndex::new(0), hash.clone(), data1.clone());
    assert_eq!(cache.get_candidate(SlotIndex::new(0), &hash), Some(&data1));

    // Overwrite with new data
    cache.cache_candidate(SlotIndex::new(0), hash.clone(), data2.clone());
    assert_eq!(cache.get_candidate(SlotIndex::new(0), &hash), Some(&data2));
}

#[test]
fn test_candidate_resolver_cache_multiple_entries() {
    let mut cache = super::CandidateResolverCache::new();

    // Store multiple entries
    for i in 0u32..10u32 {
        let hash = UInt256::from([i as u8; 32]);
        let data = vec![i as u8; (i as usize + 1) * 10];
        cache.cache_candidate(SlotIndex::new(i), hash.clone(), data.clone());
        cache.cache_notar_cert(SlotIndex::new(i), hash.clone(), data.clone());
    }

    // Verify all entries are retrievable
    for i in 0u32..10u32 {
        let hash = UInt256::from([i as u8; 32]);
        let expected_data = vec![i as u8; (i as usize + 1) * 10];
        assert_eq!(cache.get_candidate(SlotIndex::new(i), &hash), Some(&expected_data));
        assert_eq!(cache.get_notar_cert(SlotIndex::new(i), &hash).cloned(), Some(expected_data));
    }
}

#[test]
fn test_candidate_resolver_cache_cleanup() {
    let mut cache = super::CandidateResolverCache::new();

    // Store entries for slots 0-9
    for i in 0u32..10u32 {
        let hash = UInt256::from([i as u8; 32]);
        cache.cache_candidate(SlotIndex::new(i), hash.clone(), vec![i as u8]);
        cache.cache_notar_cert(SlotIndex::new(i), hash.clone(), vec![i as u8 + 100]);
    }

    // Cleanup slots < 5 (keep 5-9)
    cache.cleanup_before(SlotIndex::new(5));

    // Slots 0-4 should be gone
    for i in 0u32..5u32 {
        let hash = UInt256::from([i as u8; 32]);
        assert!(cache.get_candidate(SlotIndex::new(i), &hash).is_none());
        assert!(cache.get_notar_cert(SlotIndex::new(i), &hash).is_none());
    }

    // Slots 5-9 should still exist
    for i in 5u32..10u32 {
        let hash = UInt256::from([i as u8; 32]);
        assert_eq!(cache.get_candidate(SlotIndex::new(i), &hash), Some(&vec![i as u8]));
        assert_eq!(
            cache.get_notar_cert(SlotIndex::new(i), &hash).cloned(),
            Some(vec![i as u8 + 100])
        );
    }
}

#[test]
fn test_receiver_compute_overlay_id_matches_cpp_private_overlay() {
    // C++ reference: `validator/consensus/private-overlay.cpp`
    //
    // overlay_seed = consensus.overlayId(session_id, nodes_short_ids_in_validator_set_order)
    // overlay_full_id = OverlayIdFull{ serialize_tl_object(overlay_seed, true) }  (name bytes)
    // overlay_short_id = overlay_full_id.compute_short_id()

    let session_id: SessionId = UInt256::rand();

    let keys: Vec<_> = (0..4)
        .map(|_| Ed25519KeyOption::<ZeroizingBytes>::generate().expect("failed to generate key"))
        .collect();

    let nodes: Vec<SessionNode> = keys
        .iter()
        .map(|k| SessionNode { adnl_id: k.id().clone(), public_key: k.clone(), weight: 1 })
        .collect();

    let (overlay_id, overlay_short_id) =
        super::ReceiverWrapper::compute_overlay_id(&session_id, &nodes)
            .expect("compute_overlay_id failed");

    let nodes_int256: Vec<UInt256> =
        nodes.iter().map(|n| UInt256::from(*n.public_key.id().data())).collect();

    let overlay_seed =
        OverlayId { session_id: session_id.clone(), nodes: nodes_int256.into_iter().collect() };

    let overlay_full_name_bytes =
        consensus_common::serialize_tl_boxed_object!(&overlay_seed.into_boxed());

    let expected_overlay_id = UInt256::calc_file_hash(&overlay_full_name_bytes);
    assert_eq!(overlay_id, expected_overlay_id);

    let overlay_pubkey = Overlay { name: overlay_full_name_bytes }.into_boxed();
    let expected_overlay_short_id = KeyId::from_data(
        adnl::common::hash_boxed(&overlay_pubkey).expect("hash_boxed(pub.overlay) failed"),
    );

    assert_eq!(overlay_short_id, expected_overlay_short_id);
}

#[test]
fn test_candidate_resolver_cache_cleanup_all() {
    let mut cache = super::CandidateResolverCache::new();

    // Store some entries
    for i in 0u32..5u32 {
        let hash = UInt256::from([i as u8; 32]);
        cache.cache_candidate(SlotIndex::new(i), hash.clone(), vec![i as u8]);
    }

    // Cleanup all (slots < 10, but we only have 0-4)
    cache.cleanup_before(SlotIndex::new(10));

    // All should be gone
    for i in 0u32..5u32 {
        let hash = UInt256::from([i as u8; 32]);
        assert!(cache.get_candidate(SlotIndex::new(i), &hash).is_none());
    }
}

#[test]
fn test_merge_candidate_response_parts_body_then_notar_completes_merge() {
    let slot = SlotIndex::new(42);
    let block_hash = UInt256::rand();
    let candidate_bytes = vec![1, 2, 3, 4];
    let notar_bytes = vec![9, 8, 7];

    let mut cache = super::CandidateResolverCache::new();
    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(0),
        cached_notar: None,
        cached_candidate: None,
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    // First partial response: candidate body only -> notar remains missing.
    let (merged_candidate_1, merged_notar_1) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &candidate_bytes,
        &[],
    );
    assert_eq!(merged_candidate_1, candidate_bytes);
    assert!(
        merged_notar_1.is_empty(),
        "body-only partial response must not be considered complete"
    );
    assert_eq!(state.cached_candidate.as_ref(), Some(&candidate_bytes));
    assert!(state.cached_notar.is_none());

    // Second partial response: notar only -> merged output must include cached body + new notar.
    let (merged_candidate_2, merged_notar_2) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &[],
        &notar_bytes,
    );
    assert_eq!(merged_candidate_2, candidate_bytes);
    assert_eq!(merged_notar_2, notar_bytes);
    assert_eq!(state.cached_candidate.as_ref(), Some(&candidate_bytes));
    assert_eq!(state.cached_notar.as_ref(), Some(&notar_bytes));
}

#[test]
fn test_merge_candidate_response_parts_uses_locally_cached_notar() {
    let slot = SlotIndex::new(7);
    let block_hash = UInt256::rand();
    let candidate_bytes = vec![11, 22, 33];
    let cached_notar = vec![44, 55];

    let mut cache = super::CandidateResolverCache::new();
    cache.cache_notar_cert(slot, block_hash.clone(), cached_notar.clone());

    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(1),
        cached_notar: None,
        cached_candidate: None,
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    // No notar in this response, but resolver cache already has one.
    let (merged_candidate, merged_notar) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &candidate_bytes,
        &[],
    );
    assert_eq!(merged_candidate, candidate_bytes);
    assert_eq!(
        merged_notar, cached_notar,
        "candidate-only response should complete when notar already exists in local cache"
    );
}

#[test]
fn test_merge_candidate_response_parts_uses_locally_cached_candidate() {
    let slot = SlotIndex::new(8);
    let block_hash = UInt256::rand();
    let cached_candidate = vec![11, 22, 33];
    let notar_bytes = vec![44, 55];

    let mut cache = super::CandidateResolverCache::new();
    cache.cache_candidate(slot, block_hash.clone(), cached_candidate.clone());

    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(1),
        cached_notar: None,
        cached_candidate: None,
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    // No candidate in this response, but resolver cache already has one.
    let (merged_candidate, merged_notar) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &[],
        &notar_bytes,
    );
    assert_eq!(
        merged_candidate, cached_candidate,
        "notar-only response should complete when candidate already exists in local cache"
    );
    assert_eq!(merged_notar, notar_bytes);
}

#[test]
fn test_merge_candidate_response_parts_notar_then_body_completes_merge() {
    let slot = SlotIndex::new(99);
    let block_hash = UInt256::rand();
    let candidate_bytes = vec![5, 4, 3, 2, 1];
    let notar_bytes = vec![9, 9, 9];

    let mut cache = super::CandidateResolverCache::new();
    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(1),
        cached_notar: None,
        cached_candidate: None,
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    // First partial response: notar only.
    let (merged_candidate_1, merged_notar_1) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &[],
        &notar_bytes,
    );
    assert!(
        merged_candidate_1.is_empty(),
        "notar-only partial response must not be considered complete"
    );
    assert_eq!(merged_notar_1, notar_bytes);

    // Second partial response: candidate only, merged result should include cached notar.
    let (merged_candidate_2, merged_notar_2) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &candidate_bytes,
        &[],
    );
    assert_eq!(merged_candidate_2, candidate_bytes);
    assert_eq!(merged_notar_2, notar_bytes);
}

#[test]
fn test_merge_candidate_response_parts_preserves_existing_parts() {
    let slot = SlotIndex::new(100);
    let block_hash = UInt256::rand();
    let existing_candidate = vec![1, 1, 1];
    let existing_notar = vec![2, 2, 2];
    let later_candidate = vec![3, 3, 3];
    let later_notar = vec![4, 4, 4];

    let mut cache = super::CandidateResolverCache::new();
    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(1),
        cached_notar: Some(existing_notar.clone()),
        cached_candidate: Some(existing_candidate.clone()),
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    let (merged_candidate, merged_notar) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &later_candidate,
        &later_notar,
    );

    assert_eq!(
        merged_candidate, existing_candidate,
        "C++ CandidateAndCert::merge fills missing body only; it must not replace known body"
    );
    assert_eq!(
        merged_notar, existing_notar,
        "C++ CandidateAndCert::merge fills missing notar only; it must not replace known notar"
    );
    assert_eq!(state.cached_candidate.as_ref(), Some(&existing_candidate));
    assert_eq!(state.cached_notar.as_ref(), Some(&existing_notar));
}

#[test]
fn test_sliding_window_rate_limiter_enforces_window_limit() {
    let mut limiter = super::SlidingWindowRateLimiter::default();
    let window = Duration::from_secs(1);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    assert!(limiter.allow(now, window, 2));
    assert!(limiter.allow(now, window, 2));
    assert!(!limiter.allow(now, window, 2));
    assert!(limiter.allow(now + Duration::from_millis(1_001), window, 2));
    assert!(!limiter.allow(now + Duration::from_millis(1_001), window, 0));
}

// ============================================================================
// TN-1034 / NODE-75: BadSignatureBanState (temporary peer ban) tests
// ============================================================================
//
// C++ parity: PoolImpl::bad_signature_bans_ in pool.cpp arms a ban for
// `params_.bad_signature_ban_duration` whenever a peer sends a bad
// vote/certificate signature; subsequent traffic from that peer is dropped
// until the ban expires.

/// A freshly constructed ban state must not consider any source banned.
#[test]
fn test_bad_signature_ban_state_starts_empty() {
    let mut state = super::BadSignatureBanState::new(Duration::from_secs(5));
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);

    assert!(!state.is_banned(0, now));
    assert!(!state.is_banned(7, now));
    assert_eq!(state.active_bans(), 0);
}

/// `record()` must arm a ban that is active until `now + duration` and then
/// auto-expires.
#[test]
fn test_bad_signature_ban_state_record_and_expire() {
    let duration = Duration::from_secs(5);
    let mut state = super::BadSignatureBanState::new(duration);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    state.record(3, now);

    assert!(state.is_banned(3, now), "source must be banned immediately after record()");
    assert!(state.is_banned(3, now + duration / 2), "source must remain banned mid-window");

    let expired_at = now + duration + Duration::from_millis(1);
    assert!(!state.is_banned(3, expired_at), "ban must lift at expiry");
    assert_eq!(
        state.active_bans(),
        0,
        "expired ban entries must be evicted by is_banned() so the map stays bounded"
    );
}

/// Re-recording extends the existing ban window without producing duplicate
/// entries (parity with C++ `bad_signature_bans_[peer] = ts::in(duration)`).
#[test]
fn test_bad_signature_ban_state_record_refreshes_existing_entry() {
    let duration = Duration::from_secs(5);
    let mut state = super::BadSignatureBanState::new(duration);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    state.record(2, t0);
    let t1 = t0 + Duration::from_secs(3);
    state.record(2, t1);

    assert_eq!(state.active_bans(), 1, "second record() must not create a new entry");
    let after_first_expiry = t0 + duration + Duration::from_millis(1);
    assert!(
        state.is_banned(2, after_first_expiry),
        "refreshed ban must extend past the original expiry"
    );
}

/// Bans are scoped per source — banning one source must not affect others.
#[test]
fn test_bad_signature_ban_state_is_per_source() {
    let mut state = super::BadSignatureBanState::new(Duration::from_secs(5));
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    state.record(4, now);

    assert!(state.is_banned(4, now));
    assert!(!state.is_banned(5, now));
    assert!(!state.is_banned(0, now));
}

// ============================================================================
// requestCandidate response validation regression tests
// ============================================================================
//
// These tests lock in three properties of the repair channel:
//
//   1. `merge_candidate_response_parts` never persists unverified response
//      bytes into the shared `resolver_cache`; the cache is only ever read
//      to fall back to previously trusted candidate/notar parts.
//   2. `validate_repair_candidate_identity` recomputes `(slot, candidate_hash)`
//      from the response body and rejects any payload whose identity does not
//      match the requested `(slot, block_hash)` (wrong-hash, wrong-slot, or
//      malformed bytes).
//   3. `validate_repair_notar_signature_set` mirrors the strict policy of
//      `Certificate::from_tl_signatures` (the FSM-side acceptance gate in
//      `SessionProcessor::process_received_notar_cert`): invalid validator
//      indices, duplicate indices, signature failures, and aggregate weight
//      below the 2/3 threshold are all rejected.

/// Builder for a minimal repair-channel fixture (validator set + session id +
/// keys) shared by the notar-cert regression tests.
fn build_repair_validator_set(
    num_validators: usize,
    weight: crate::ValidatorWeight,
) -> (SessionId, Vec<super::SourceStats>, Vec<crate::PrivateKey>) {
    let session_id: SessionId = UInt256::from_slice(&[0x37; 32]);
    let mut sources = Vec::with_capacity(num_validators);
    let mut keys = Vec::with_capacity(num_validators);
    for idx in 0..num_validators {
        let key: crate::PrivateKey = Ed25519KeyOption::<ZeroizingBytes>::generate()
            .expect("failed to generate validator key");
        let public_key: PublicKey = key.clone();
        let adnl_id = key.id().clone();
        sources.push(super::SourceStats::new(idx as u32, adnl_id, public_key, weight));
        keys.push(key);
    }
    (session_id, sources, keys)
}

fn build_notar_signature_set_bytes(
    session_id: &SessionId,
    vote: &NotarizeVote,
    signing_keys: &[(u32, &crate::PrivateKey)],
) -> Vec<u8> {
    let mut signatures = Vec::with_capacity(signing_keys.len());
    let fsm_vote = Vote::Notarize(vote.clone());
    for (idx, key) in signing_keys {
        let signed = sign_vote(&fsm_vote, session_id, key).expect("sign_vote failed");
        signatures.push(VoteSignature::new(ValidatorIndex::new(*idx), signed.signature().to_vec()));
    }
    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_set = cert.to_tl_vote_signature_set();
    serialize_boxed(&tl_set).expect("serialize VoteSignatureSet")
}

fn build_empty_candidate_for_repair(
    slot: u32,
    parent_slot: u32,
    parent_hash: UInt256,
    session_id: &SessionId,
    leader_key: &crate::PrivateKey,
) -> (CandidateData, UInt256, Vec<u8>) {
    let block_id = BlockIdExt::default();
    let parent_tl = TlCandidateId { slot: parent_slot as i32, hash: parent_hash.clone() };
    let candidate_hash =
        compute_candidate_id_hash_empty(&block_id, (SlotIndex::new(parent_slot), &parent_hash));
    let signature = sign_candidate_u32(session_id, slot, &candidate_hash, leader_key)
        .expect("sign empty candidate");
    let empty = CandidateDataEmpty {
        slot: slot as i32,
        parent: parent_tl.into_boxed(),
        block: block_id.clone(),
        signature,
    };
    let candidate = CandidateData::Consensus_Empty(empty);
    let bytes = serialize_boxed(&candidate).expect("serialize CandidateData::Empty");
    (candidate, candidate_hash, bytes)
}

/// A `merge_candidate_response_parts` call with non-empty body/notar bytes
/// must NOT write into the shared resolver cache. Only the per-request pending
/// state may be updated. Unverified peer responses must never populate the
/// cache under the requested `(slot, hash)` and permanently suppress future
/// repairs.
#[test]
fn test_merge_candidate_response_parts_does_not_write_resolver_cache() {
    let slot = SlotIndex::new(1234);
    let block_hash = UInt256::rand();
    let candidate_bytes = vec![0xAA; 64];
    let notar_bytes = vec![0xBB; 64];

    let mut cache = super::CandidateResolverCache::new();
    let mut state = super::CandidateRequestState {
        start_time: SystemTime::now(),
        retry_count: 0,
        current_timeout: Duration::from_millis(500),
        attempt_id: 0,
        in_flight: false,
        in_flight_want_candidate: false,
        in_flight_want_notar: false,
        source_idx: ValidatorIndex::new(2),
        cached_notar: None,
        cached_candidate: None,
        giveup_reports: 0,
        peer_retry_not_before: HashMap::new(),
    };

    let (merged_candidate, merged_notar) = super::ReceiverImpl::merge_candidate_response_parts(
        &mut cache,
        Some(&mut state),
        slot,
        &block_hash,
        &candidate_bytes,
        &notar_bytes,
    );

    // Merge surfaces the pieces back to the caller and to the pending state.
    assert_eq!(merged_candidate, candidate_bytes);
    assert_eq!(merged_notar, notar_bytes);
    assert_eq!(state.cached_candidate.as_ref(), Some(&candidate_bytes));
    assert_eq!(state.cached_notar.as_ref(), Some(&notar_bytes));

    // But the resolver cache (which is shared and served back to peers) must
    // remain untouched until a trusted path (broadcast verification, startup
    // recovery, or `SessionProcessor::cache_notarization_cert`) writes to it.
    assert!(
        cache.get_candidate(slot, &block_hash).is_none(),
        "unverified response candidate bytes must not be cached"
    );
    assert!(
        cache.get_notar_cert(slot, &block_hash).is_none(),
        "unverified response notar bytes must not be cached"
    );
}

/// A well-formed `Consensus_Empty` response with the expected `(slot, hash)`
/// is accepted and its parsed `CandidateData` is returned for reuse by the
/// listener forwarding path.
#[test]
fn test_validate_repair_candidate_identity_accepts_matching_empty_block() {
    let (session_id, sources, keys) =
        build_repair_validator_set(/* nodes */ 1, /* weight */ 100);
    let parent_hash = UInt256::from_slice(&[0x11; 32]);
    let (_candidate, candidate_hash, bytes) = build_empty_candidate_for_repair(
        /* slot */ 42,
        /* parent_slot */ 41,
        parent_hash,
        &session_id,
        &keys[0],
    );

    let shard = ShardIdent::masterchain();
    let parsed = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        /* max_candidate_size */ 1 << 20,
        /* max_candidate_query_answer_size */ 1 << 24,
        /* proto_version */ 5,
        SlotIndex::new(42),
        &candidate_hash,
        &bytes,
    )
    .expect("matching identity must be accepted");

    match parsed {
        CandidateData::Consensus_Empty(empty) => {
            assert_eq!(empty.slot, 42);
        }
        _ => panic!("expected Consensus_Empty"),
    }
}

/// A response whose recomputed candidate hash differs from the requested
/// `block_hash` is rejected.
#[test]
fn test_validate_repair_candidate_identity_rejects_wrong_hash() {
    let (session_id, sources, keys) = build_repair_validator_set(1, 100);
    let parent_hash = UInt256::from_slice(&[0x22; 32]);
    let (_candidate, _candidate_hash, bytes) =
        build_empty_candidate_for_repair(7, 6, parent_hash, &session_id, &keys[0]);

    let shard = ShardIdent::masterchain();
    let wrong_hash = UInt256::from_slice(&[0xEE; 32]);
    let err = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        1 << 20,
        1 << 24,
        5,
        SlotIndex::new(7),
        &wrong_hash,
        &bytes,
    )
    .expect_err("wrong-hash response must be rejected");
    assert!(err.contains("hash mismatch"), "unexpected error: {err}");
}

/// A response whose body claims a different slot than the requested one is
/// rejected even before the hash check.
#[test]
fn test_validate_repair_candidate_identity_rejects_wrong_slot() {
    let (session_id, sources, keys) = build_repair_validator_set(1, 100);
    let parent_hash = UInt256::from_slice(&[0x33; 32]);
    let (_candidate, candidate_hash, bytes) =
        build_empty_candidate_for_repair(9, 8, parent_hash, &session_id, &keys[0]);

    let shard = ShardIdent::masterchain();
    let err = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        1 << 20,
        1 << 24,
        5,
        SlotIndex::new(10),
        &candidate_hash,
        &bytes,
    )
    .expect_err("slot-mismatch response must be rejected");
    assert!(err.contains("slot mismatch"), "unexpected error: {err}");
}

/// Bytes exceeding the receiver's per-query answer budget are rejected without
/// invoking the heavier TL/hash recomputation path.
#[test]
fn test_validate_repair_candidate_identity_rejects_oversize() {
    let (session_id, sources, _keys) = build_repair_validator_set(1, 100);
    let shard = ShardIdent::masterchain();
    let bytes = vec![0u8; 64];
    let err = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        /* max_candidate_size */ 1 << 20,
        /* max_candidate_query_answer_size */ 32,
        5,
        SlotIndex::new(1),
        &UInt256::default(),
        &bytes,
    )
    .expect_err("oversize response must be rejected");
    assert!(err.contains("exceeds answer budget"), "unexpected error: {err}");
}

/// Candidate bytes with a valid `(slot, hash)` identity but signed by a
/// non-leader key must be rejected at receiver validation time.
#[test]
fn test_validate_repair_candidate_identity_rejects_invalid_leader_signature() {
    let (session_id, sources, keys) =
        build_repair_validator_set(/* nodes */ 2, /* weight */ 100);
    let parent_hash = UInt256::from_slice(&[0x44; 32]);
    // slot=0 with slots_per_window=1 => leader is validator 0. Sign with validator 1 instead.
    let (_candidate, candidate_hash, bytes) =
        build_empty_candidate_for_repair(0, 0, parent_hash, &session_id, &keys[1]);

    let shard = ShardIdent::masterchain();
    let err = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        1 << 20,
        1 << 24,
        5,
        SlotIndex::new(0),
        &candidate_hash,
        &bytes,
    )
    .expect_err("non-leader signature must be rejected");
    assert!(err.contains("invalid candidate leader signature"), "unexpected error: {err}");
}

/// `Consensus_Block` with empty inner candidate bytes is malformed; only
/// `Consensus_Empty` may represent an empty block over the repair channel.
#[test]
fn test_validate_repair_candidate_identity_rejects_empty_block_candidate_bytes() {
    let (session_id, sources, keys) = build_repair_validator_set(1, 100);
    let slot = 17u32;
    let candidate_hash = compute_candidate_id_hash(SlotIndex::new(slot), None, None, None);
    let signature =
        sign_candidate_u32(&session_id, slot, &candidate_hash, &keys[0]).expect("sign candidate");
    let candidate = CandidateData::Consensus_Block(CandidateDataBlock {
        slot: slot as i32,
        candidate: Vec::new(),
        parent: CandidateParent::Consensus_CandidateWithoutParents,
        signature,
    });
    let bytes = serialize_boxed(&candidate).expect("serialize CandidateData::Block");

    let shard = ShardIdent::masterchain();
    let err = super::ReceiverImpl::validate_repair_candidate_identity(
        &session_id,
        &sources,
        1,
        &shard,
        1 << 20,
        1 << 24,
        5,
        SlotIndex::new(slot),
        &candidate_hash,
        &bytes,
    )
    .expect_err("empty consensus.block candidate bytes must be rejected");
    assert!(err.contains("empty candidate bytes"), "unexpected error: {err}");
}

/// A VoteSignatureSet covering 2/3 of the validator weight for the requested
/// `NotarizeVote{slot, block_hash}` must be accepted.
#[test]
fn test_validate_repair_notar_signature_set_accepts_quorum() {
    let (session_id, sources, keys) =
        build_repair_validator_set(/* nodes */ 4, /* weight */ 100);
    let slot = SlotIndex::new(11);
    let block_hash = UInt256::from_slice(&[0x42; 32]);
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };

    // 3 of 4 validators = 75% weight, above the 66% threshold.
    let signing: Vec<(u32, &crate::PrivateKey)> =
        (0..3u32).map(|i| (i, &keys[i as usize])).collect();
    let notar_bytes = build_notar_signature_set_bytes(&session_id, &vote, &signing);

    super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        /* max_candidate_query_answer_size */ 1 << 24,
        slot,
        &block_hash,
        &notar_bytes,
    )
    .expect("valid quorum must be accepted");
}

/// A notar set that names an out-of-range validator index is rejected (matches
/// `Certificate::from_tl_signatures` strict policy).
#[test]
fn test_validate_repair_notar_signature_set_rejects_invalid_validator_index() {
    let (session_id, sources, keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(12);
    let block_hash = UInt256::from_slice(&[0x55; 32]);
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };

    // 3 valid signatures + 1 forged entry pointing at validator index 100.
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..3u32 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i as usize]).expect("sign_vote");
        signatures.push(VoteSignature::new(ValidatorIndex::new(i), signed.signature().to_vec()));
    }
    signatures.push(VoteSignature::new(ValidatorIndex::new(100), vec![0xAA; 64]));
    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_set = cert.to_tl_vote_signature_set();
    let notar_bytes = serialize_boxed(&tl_set).expect("serialize VoteSignatureSet");

    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash,
        &notar_bytes,
    )
    .expect_err("invalid validator index must be rejected");
    assert!(err.contains("invalid validator index"), "unexpected error: {err}");
}

/// Duplicate validator indices in the notar set are rejected.
#[test]
fn test_validate_repair_notar_signature_set_rejects_duplicate_validator() {
    let (session_id, sources, keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(13);
    let block_hash = UInt256::from_slice(&[0x66; 32]);
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };

    let signing: Vec<(u32, &crate::PrivateKey)> =
        vec![(0, &keys[0]), (1, &keys[1]), (2, &keys[2]), (0, &keys[0])];
    let notar_bytes = build_notar_signature_set_bytes(&session_id, &vote, &signing);

    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash,
        &notar_bytes,
    )
    .expect_err("duplicate validator index must be rejected");
    assert!(err.contains("duplicate validator index"), "unexpected error: {err}");
}

/// A notar set with an invalid signature (validator claims to have signed but
/// the bytes don't verify against its key) is rejected.
#[test]
fn test_validate_repair_notar_signature_set_rejects_invalid_signature() {
    let (session_id, sources, keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(14);
    let block_hash = UInt256::from_slice(&[0x77; 32]);
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };

    // 3 valid sigs from validators 0,1,2 + one fake sig where validator 3 is
    // attributed but the signature bytes come from validator 0 (so verification
    // against validator 3's public key fails).
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..3u32 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i as usize]).expect("sign_vote");
        signatures.push(VoteSignature::new(ValidatorIndex::new(i), signed.signature().to_vec()));
    }
    let imposter_signed = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote");
    signatures
        .push(VoteSignature::new(ValidatorIndex::new(3), imposter_signed.signature().to_vec()));
    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_set = cert.to_tl_vote_signature_set();
    let notar_bytes = serialize_boxed(&tl_set).expect("serialize VoteSignatureSet");

    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash,
        &notar_bytes,
    )
    .expect_err("invalid signature must be rejected");
    assert!(err.contains("invalid signature"), "unexpected error: {err}");
}

/// Aggregate signature weight below 2/3 of the validator set is rejected,
/// even when individual signatures are otherwise well-formed.
#[test]
fn test_validate_repair_notar_signature_set_rejects_insufficient_weight() {
    let (session_id, sources, keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(15);
    let block_hash = UInt256::from_slice(&[0x88; 32]);
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };

    // Only 2 of 4 sign: total 200 / 400 = 50% < 66%.
    let signing: Vec<(u32, &crate::PrivateKey)> = vec![(0, &keys[0]), (1, &keys[1])];
    let notar_bytes = build_notar_signature_set_bytes(&session_id, &vote, &signing);

    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash,
        &notar_bytes,
    )
    .expect_err("insufficient weight must be rejected");
    assert!(err.contains("insufficient weight"), "unexpected error: {err}");
}

/// When the notar set is signed for a different `(slot, block_hash)` than the
/// receiver requested, signature verification cryptographically rejects it
/// because the `dataToSign` payload binds the session id and the unsigned
/// vote. This blocks replay of an unrelated cert against the requested key.
#[test]
fn test_validate_repair_notar_signature_set_rejects_mismatched_vote() {
    let (session_id, sources, keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(16);
    let block_hash_signed = UInt256::from_slice(&[0x99; 32]);
    let block_hash_requested = UInt256::from_slice(&[0xAA; 32]);
    let signed_vote = NotarizeVote { slot, block_hash: block_hash_signed.clone() };

    // Validators correctly sign for the wrong vote.
    let signing: Vec<(u32, &crate::PrivateKey)> =
        (0..3u32).map(|i| (i, &keys[i as usize])).collect();
    let notar_bytes = build_notar_signature_set_bytes(&session_id, &signed_vote, &signing);

    // Receiver requested a different block_hash; verification must fail because
    // the dataToSign payload is bound to the requested NotarizeVote.
    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash_requested,
        &notar_bytes,
    )
    .expect_err("notar bound to a different vote must be rejected");
    assert!(err.contains("invalid signature"), "unexpected error: {err}");
}

/// Notar bytes exceeding the receiver's per-query answer budget are rejected
/// without invoking signature verification.
#[test]
fn test_validate_repair_notar_signature_set_rejects_oversize() {
    let (session_id, sources, _keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(17);
    let block_hash = UInt256::from_slice(&[0xBB; 32]);
    let bytes = vec![0u8; 128];
    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        /* max_candidate_query_answer_size */ 64,
        slot,
        &block_hash,
        &bytes,
    )
    .expect_err("oversize notar response must be rejected");
    assert!(err.contains("exceeds answer budget"), "unexpected error: {err}");
}

/// Bytes that decode as a different TL type must be rejected with the
/// dedicated "unexpected TL type" path; signature verification must not run
/// on them. Uses the lower-cost path of submitting an `OverlayId` (a TL type
/// readily available in this test module).
#[test]
fn test_validate_repair_notar_signature_set_rejects_wrong_tl_type() {
    let (session_id, sources, _keys) = build_repair_validator_set(4, 100);
    let slot = SlotIndex::new(18);
    let block_hash = UInt256::from_slice(&[0xCC; 32]);

    // Serialize an unrelated TL boxed object (OverlayId) and try to feed it as
    // notar bytes.
    let bogus = OverlayId { session_id: UInt256::default(), nodes: Vec::new().into() };
    let bytes = serialize_boxed(&bogus.into_boxed()).expect("serialize OverlayId");

    let err = super::ReceiverImpl::validate_repair_notar_signature_set(
        &session_id,
        &sources,
        1 << 24,
        slot,
        &block_hash,
        &bytes,
    )
    .expect_err("wrong TL type must be rejected");
    assert!(
        err.contains("unexpected TL type") || err.contains("deserialize VoteSignatureSet"),
        "unexpected error: {err}",
    );
}
