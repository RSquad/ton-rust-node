/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Cryptographic tests for Simplex consensus
//!
//! These tests verify:
//! - Threshold calculations (2/3 and 1/3)
//! - Session-scoped signature creation and verification
//! - Candidate signature creation and verification
//! - Candidate ID hash computation

use crate::{
    block::SlotIndex,
    utils::{
        check_candidate_signature, check_session_signature, compute_candidate_id_hash,
        create_candidate_id_to_sign, sign_candidate, sign_with_session, threshold_33, threshold_66,
    },
};
use ton_api::ton::consensus::CandidateId;
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, UInt256};

/*
    ============================================================================
    Threshold calculation tests
    ============================================================================
*/

/// Test 2/3 and 1/3 threshold calculations
///
/// Verifies that threshold functions match C++ implementation:
/// - threshold_66: (total * 2) / 3 + 1
/// - threshold_33: total / 3 + 1
#[test]
fn test_threshold_calculations() {
    // Test strict 2/3 threshold (> 2/3)
    // Formula: (total * 2) / 3 + 1
    assert_eq!(threshold_66(100), 67); // (200 / 3) + 1 = 67
    assert_eq!(threshold_66(3), 3); // (6 / 3) + 1 = 3
    assert_eq!(threshold_66(99), 67); // (198 / 3) + 1 = 67

    // Test strict 1/3 threshold (> 1/3)
    // Formula: total / 3 + 1
    assert_eq!(threshold_33(100), 34); // (100 / 3) + 1 = 34
    assert_eq!(threshold_33(3), 2); // (3 / 3) + 1 = 2
    assert_eq!(threshold_33(99), 34); // (99 / 3) + 1 = 34
}

/*
    ============================================================================
    Session signature tests
    ============================================================================
*/

/// Test session-scoped signature roundtrip
///
/// Verifies that:
/// - Signatures can be created and verified
/// - Wrong session ID causes verification failure
/// - Wrong data causes verification failure
#[test]
fn test_session_signature_roundtrip() {
    let private_key = Ed25519KeyOption::generate().expect("Failed to generate key");
    let session_id = UInt256::from([1u8; 32]);
    let data = b"test data to sign";

    // Sign
    let signature = sign_with_session(&session_id, data, &private_key).expect("Failed to sign");

    // Verify
    assert!(check_session_signature(&session_id, data, &signature, &private_key));

    // Verify fails with different session
    let wrong_session = UInt256::from([2u8; 32]);
    assert!(!check_session_signature(&wrong_session, data, &signature, &private_key));

    // Verify fails with different data
    assert!(!check_session_signature(&session_id, b"wrong data", &signature, &private_key));
}

/*
    ============================================================================
    Candidate signature tests
    ============================================================================
*/

/// Test candidate signature roundtrip
///
/// Verifies that:
/// - Candidate signatures can be created and verified
/// - Wrong slot causes verification failure
/// - Wrong hash causes verification failure
#[test]
fn test_candidate_signature_roundtrip() {
    let private_key = Ed25519KeyOption::generate().expect("Failed to generate key");
    let session_id = UInt256::from([1u8; 32]);
    let slot = SlotIndex::new(42);
    let candidate_hash = UInt256::from([3u8; 32]);

    // Sign
    let signature = sign_candidate(&session_id, slot, &candidate_hash, &private_key)
        .expect("Failed to sign candidate");

    // Verify
    assert!(check_candidate_signature(
        &session_id,
        slot,
        &candidate_hash,
        &signature,
        &private_key
    ));

    // Verify fails with wrong slot
    assert!(!check_candidate_signature(
        &session_id,
        slot + 1,
        &candidate_hash,
        &signature,
        &private_key
    ));

    // Verify fails with wrong hash
    let wrong_hash = UInt256::from([4u8; 32]);
    assert!(!check_candidate_signature(&session_id, slot, &wrong_hash, &signature, &private_key));
}

/// Regression test: verify that create_candidate_id_to_sign serializes bare
/// `consensus.candidateId` (not wrapped in `candidateParent`).
///
/// The serialized bytes must deserialize as `consensus.CandidateId` and round-trip
/// back to the original slot and hash values.
#[test]
fn test_candidate_id_to_sign_is_bare_candidate_id() {
    let slot = SlotIndex::new(7);
    let hash = UInt256::from([0xAB; 32]);

    let bytes = create_candidate_id_to_sign(slot, &hash);

    // Must deserialize as CandidateId (would fail if wrapped in CandidateParent)
    let deserialized = consensus_common::utils::deserialize_tl_boxed_object::<CandidateId>(&bytes)
        .expect("should deserialize as bare CandidateId, not CandidateParent");

    // Verify round-trip values
    match deserialized {
        CandidateId::Consensus_CandidateId(id) => {
            assert_eq!(id.slot, 7);
            assert_eq!(id.hash, hash);
        }
    }
}

/*
    ============================================================================
    Candidate ID hash computation tests
    ============================================================================
*/

/// Test candidate ID hash computation without parent
///
/// Verifies that:
/// - Same inputs produce same hash (deterministic)
/// - Slot is NOT used in hash computation (per TL schema)
#[test]
fn test_compute_candidate_id_hash_no_parent() {
    // Empty block with no parent - just verify it produces a deterministic hash
    let slot = SlotIndex::new(1);
    let hash1 = compute_candidate_id_hash(slot, None, None, None);
    let hash2 = compute_candidate_id_hash(slot, None, None, None);

    // Same inputs should produce same hash
    assert_eq!(hash1, hash2);

    let hash3 = compute_candidate_id_hash(slot + 1, None, None, None);
    assert_eq!(hash1, hash3); // Same hash because slot is not used
}

/// Test candidate ID hash computation with parent
///
/// Verifies that:
/// - Same inputs produce same hash (deterministic)
/// - Different parent produces different hash
#[test]
fn test_compute_candidate_id_hash_with_parent() {
    let slot = SlotIndex::new(2);
    let parent_hash = UInt256::from([5u8; 32]);

    let hash1 =
        compute_candidate_id_hash(slot, None, None, Some((SlotIndex::new(1), &parent_hash)));
    let hash2 =
        compute_candidate_id_hash(slot, None, None, Some((SlotIndex::new(1), &parent_hash)));

    // Same inputs should produce same hash
    assert_eq!(hash1, hash2);

    // Different parent should produce different hash
    let other_parent = UInt256::from([6u8; 32]);
    let hash3 =
        compute_candidate_id_hash(slot, None, None, Some((SlotIndex::new(1), &other_parent)));
    assert_ne!(hash1, hash3);
}

/// Test candidate ID hash with block ID
///
/// Verifies hash computation with actual block ID data
#[test]
fn test_compute_candidate_id_hash_with_block_id() {
    let slot = SlotIndex::new(3);
    let block_id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 12345,
        root_hash: UInt256::from([7u8; 32]),
        file_hash: UInt256::from([8u8; 32]),
    };
    let collated_hash = UInt256::from([9u8; 32]);

    let hash1 = compute_candidate_id_hash(slot, Some(&block_id), Some(&collated_hash), None);
    let hash2 = compute_candidate_id_hash(slot, Some(&block_id), Some(&collated_hash), None);

    // Same inputs should produce same hash
    assert_eq!(hash1, hash2);

    // Different collated hash should produce different hash
    let other_collated = UInt256::from([10u8; 32]);
    let hash3 = compute_candidate_id_hash(slot, Some(&block_id), Some(&other_collated), None);
    assert_ne!(hash1, hash3);

    // Empty block (no block_id) should produce different hash
    let hash4 = compute_candidate_id_hash(slot, None, None, None);
    assert_ne!(hash1, hash4);
}

/*
    ============================================================================
    Vote TL serialization tests
    ============================================================================
*/

use crate::{
    simplex_state::{FinalizeVote, NotarizeVote, SkipVote, Vote},
    utils::{
        extract_vote, sign_vote, tl_unsigned_to_vote, verify_vote_signature, vote_to_tl_unsigned,
    },
};

/// Test NotarizeVote roundtrip (FSM -> TL -> FSM)
#[test]
fn test_notarize_vote_roundtrip() {
    let block_hash = UInt256::from([1u8; 32]);

    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(42), block_hash: block_hash.clone() });

    // Convert to TL
    let tl_unsigned = vote_to_tl_unsigned(&vote).unwrap();

    // Convert back to FSM
    let vote_back = tl_unsigned_to_vote(&tl_unsigned).unwrap();

    assert_eq!(vote, vote_back);
}

/// Test FinalizeVote roundtrip
#[test]
fn test_finalize_vote_roundtrip() {
    let block_hash = UInt256::from([3u8; 32]);

    // Note: mc_signature was removed from FinalizeVote (C++ protocol doesn't have it)
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(43), block_hash: block_hash.clone() });

    let tl_unsigned = vote_to_tl_unsigned(&vote).unwrap();
    let vote_back = tl_unsigned_to_vote(&tl_unsigned).unwrap();

    assert_eq!(vote, vote_back);
}

/// Test FinalizeVote roundtrip (same as above, but kept for API completeness)
#[test]
fn test_finalize_vote_no_mc_signature_roundtrip() {
    let block_hash = UInt256::from([5u8; 32]);

    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(44), block_hash: block_hash.clone() });

    let tl_unsigned = vote_to_tl_unsigned(&vote).unwrap();
    let vote_back = tl_unsigned_to_vote(&tl_unsigned).unwrap();

    assert_eq!(vote, vote_back);
}

/// Test SkipVote roundtrip
#[test]
fn test_skip_vote_roundtrip() {
    // C++ uses single slot for skip vote
    let vote = Vote::Skip(SkipVote { slot: SlotIndex::new(50) });

    let tl_unsigned = vote_to_tl_unsigned(&vote).unwrap();
    let vote_back = tl_unsigned_to_vote(&tl_unsigned).unwrap();

    assert_eq!(vote, vote_back);
}

/// Test vote signing and verification
///
/// # C++ Reference (pool.cpp)
///
/// Unlike candidates, votes are signed WITHOUT session ID wrapper.
/// The session_id parameter is kept for API consistency but not used.
#[test]
fn test_vote_sign_and_verify() {
    // Generate key pair (Ed25519KeyOption contains both private and public key)
    let key = Ed25519KeyOption::generate().unwrap();

    // Create session ID (not used for votes, kept for API consistency)
    let session_id = UInt256::from([0xABu8; 32]);

    // Create a vote - use block_hash only (new format)
    let block_hash = UInt256::from([9u8; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(100), block_hash: block_hash.clone() });

    // Sign the vote
    let signed_vote = sign_vote(&vote, &session_id, &key).unwrap();

    // Verify signature (Ed25519KeyOption contains public key)
    assert!(verify_vote_signature(&signed_vote, &session_id, &key));

    // Verify with different session ID fails (session_id IS used for votes now - matches C++)
    let other_session_id = UInt256::from([0xCDu8; 32]);
    assert!(!verify_vote_signature(&signed_vote, &other_session_id, &key));

    // Verify with different key fails
    let other_key = Ed25519KeyOption::generate().unwrap();
    assert!(!verify_vote_signature(&signed_vote, &session_id, &other_key));

    // Extract vote and verify contents
    let extracted = extract_vote(&signed_vote).unwrap();
    assert_eq!(vote, extracted);
}
