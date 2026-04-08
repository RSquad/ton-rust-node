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

use crate::{block::SlotIndex, SessionId, SessionNode};
use ton_api::{
    ton::{consensus::overlayid::OverlayId, pub_::publickey::Overlay},
    IntoBoxed,
};
use ton_block::{Ed25519KeyOption, KeyId, UInt256};

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

    let keys: Vec<_> =
        (0..4).map(|_| Ed25519KeyOption::generate().expect("failed to generate key")).collect();

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
