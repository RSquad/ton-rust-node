/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for misbehavior detection and proof collection.

use super::{ConflictingVoteType, VoteDescriptor, *};
use ton_block::UInt256;

#[test]
fn test_conflicting_votes_creation() {
    let slot = SlotIndex::new(42);
    let validator_idx = ValidatorIndex(5);
    let hash1 = UInt256::rand();
    let hash2 = UInt256::rand();
    let vote1: RawVoteData = vec![1, 2, 3, 4].into();
    let vote2: RawVoteData = vec![5, 6, 7, 8].into();
    let proof = MisbehaviorProof::conflicting_votes(
        slot,
        validator_idx,
        ConflictingVoteType::Notarize,
        hash1.clone(),
        hash2.clone(),
        vote1.clone(),
        vote2.clone(),
    );

    match proof {
        MisbehaviorProof::ConflictingVotes {
            slot: s,
            validator_idx: v,
            vote_type,
            hash1: h1,
            hash2: h2,
            vote1: v1,
            vote2: v2,
        } => {
            assert_eq!(s, slot);
            assert_eq!(v, validator_idx);
            assert_eq!(vote_type, ConflictingVoteType::Notarize);
            assert_eq!(h1, hash1);
            assert_eq!(h2, hash2);
            assert_eq!(v1, vote1);
            assert_eq!(v2, vote2);
        }
        _ => panic!("Expected ConflictingVotes"),
    }
}

#[test]
fn test_conflicting_types_creation() {
    let slot = SlotIndex::new(100);
    let validator_idx = ValidatorIndex(3);
    let finalize_hash = UInt256::rand();
    let vote1: RawVoteData = vec![1, 2, 3].into();
    let vote2: RawVoteData = vec![4, 5, 6].into();
    let proof = MisbehaviorProof::conflicting_types(
        slot,
        validator_idx,
        VoteDescriptor::Skip,
        VoteDescriptor::Finalize(finalize_hash.clone()),
        vote1.clone(),
        vote2.clone(),
        ConflictReason::FinalizeAfterSkip,
    );

    match proof {
        MisbehaviorProof::ConflictingVoteTypes {
            slot: s,
            validator_idx: v,
            existing_vote,
            new_vote,
            vote1: v1,
            vote2: v2,
            reason,
        } => {
            assert_eq!(s, slot);
            assert_eq!(v, validator_idx);
            assert_eq!(existing_vote, VoteDescriptor::Skip);
            assert_eq!(new_vote, VoteDescriptor::Finalize(finalize_hash));
            assert_eq!(v1, vote1);
            assert_eq!(v2, vote2);
            assert_eq!(reason, ConflictReason::FinalizeAfterSkip);
        }
        _ => panic!("Expected ConflictingVoteTypes"),
    }
}

#[test]
fn test_conflicting_votes_description() {
    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(0),
        ValidatorIndex(0),
        ConflictingVoteType::Notarize,
        UInt256::rand(),
        UInt256::rand(),
        RawVoteData::default(),
        RawVoteData::default(),
    );
    assert_eq!(proof.description(), "conflicting votes for same slot");
}

#[test]
fn test_conflict_reason_descriptions() {
    assert_eq!(
        ConflictReason::NotarizeFinalizeHashMismatch.description(),
        "notarize and finalize for different blocks"
    );
    assert_eq!(ConflictReason::FinalizeAfterSkip.description(), "finalize after skip");
    assert_eq!(ConflictReason::NotarizeAfterSkip.description(), "notarize after skip");
    assert_eq!(
        ConflictReason::FinalizeAfterNotarFallback.description(),
        "finalize after notar-fallback"
    );
    assert_eq!(
        ConflictReason::FinalizeAfterSkipFallback.description(),
        "finalize after skip-fallback"
    );
}

#[test]
fn test_proof_size_bytes() {
    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(0),
        ValidatorIndex(0),
        ConflictingVoteType::Notarize,
        UInt256::rand(),
        UInt256::rand(),
        vec![1, 2, 3].into(),
        vec![4, 5, 6, 7].into(),
    );
    assert_eq!(proof.size_bytes(), 7);
}

#[test]
fn test_report_display() {
    let hash1 = UInt256::rand();
    let hash2 = UInt256::rand();
    let report = MisbehaviorReport {
        validator_idx: ValidatorIndex(5),
        slot: SlotIndex::new(42),
        proof: MisbehaviorProof::conflicting_votes(
            SlotIndex::new(42),
            ValidatorIndex(5),
            ConflictingVoteType::Notarize,
            hash1.clone(),
            hash2.clone(),
            RawVoteData::default(),
            RawVoteData::default(),
        ),
    };
    let display = report.to_string();
    assert!(display.contains("validator 5"), "got: {}", display);
    assert!(display.contains("slot 42"), "got: {}", display);
    assert!(display.contains("notarize"), "got: {}", display);
    // Verify hash1 prefix is in display
    assert!(display.contains(&hash1.to_hex_string()[..8]), "got: {}", display);
}

#[test]
fn test_report_with_conflict_reason() {
    let finalize_hash = UInt256::rand();
    let report = MisbehaviorReport {
        validator_idx: ValidatorIndex(3),
        slot: SlotIndex::new(100),
        proof: MisbehaviorProof::conflicting_types(
            SlotIndex::new(100),
            ValidatorIndex(3),
            VoteDescriptor::Skip,
            VoteDescriptor::Finalize(finalize_hash.clone()),
            RawVoteData::default(),
            RawVoteData::default(),
            ConflictReason::FinalizeAfterSkip,
        ),
    };
    let display = report.to_string();
    assert!(display.contains("finalize after skip"), "got: {}", display);
    assert!(display.contains("v003"), "got: {}", display);
    assert!(display.contains("slot 100"), "got: {}", display);
}

// =============================================================================
// VoteResult Tests
// =============================================================================

#[test]
fn test_vote_result_applied() {
    let result = VoteResult::Applied;
    assert!(result.is_applied());
    assert!(!result.is_duplicate());
    assert!(!result.is_misbehavior());
    assert!(result.misbehavior_proof().is_none());
    assert_eq!(result.to_string(), "applied");
}

#[test]
fn test_vote_result_duplicate() {
    let result = VoteResult::Duplicate;
    assert!(!result.is_applied());
    assert!(result.is_duplicate());
    assert!(!result.is_misbehavior());
    assert!(result.misbehavior_proof().is_none());
    assert_eq!(result.to_string(), "duplicate");
}

#[test]
fn test_vote_result_misbehavior() {
    let hash1 = UInt256::rand();
    let hash2 = UInt256::rand();
    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(10),
        ValidatorIndex(2),
        ConflictingVoteType::Finalize,
        hash1.clone(),
        hash2.clone(),
        vec![1, 2, 3].into(),
        vec![4, 5, 6].into(),
    );
    let result = VoteResult::Misbehavior(proof.clone());

    assert!(!result.is_applied());
    assert!(!result.is_duplicate());
    assert!(result.is_misbehavior());

    let extracted_proof = result.misbehavior_proof().unwrap();
    assert_eq!(extracted_proof.description(), "conflicting votes for same slot");
    assert_eq!(extracted_proof.slot(), SlotIndex::new(10));
    assert_eq!(extracted_proof.validator_idx(), ValidatorIndex(2));
    // Verify hashes are accessible
    assert_eq!(extracted_proof.hash1(), Some(&hash1));
    assert_eq!(extracted_proof.hash2(), Some(&hash2));

    let display = result.to_string();
    assert!(display.starts_with("misbehavior:"), "got: {}", display);
    assert!(display.contains("v002"), "got: {}", display);
    assert!(display.contains("slot 10"), "got: {}", display);
}

#[test]
fn test_vote_result_rejected() {
    let result = VoteResult::Rejected("invalid validator".to_string());
    assert!(!result.is_applied());
    assert!(!result.is_duplicate());
    assert!(!result.is_misbehavior());
    assert!(result.misbehavior_proof().is_none());
    assert_eq!(result.to_string(), "rejected: invalid validator");
}

#[test]
fn test_vote_result_is_ok_is_err() {
    assert!(VoteResult::Applied.is_ok());
    assert!(!VoteResult::Applied.is_err());

    assert!(VoteResult::Duplicate.is_ok());
    assert!(!VoteResult::Duplicate.is_err());

    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(0),
        ValidatorIndex(0),
        ConflictingVoteType::Notarize,
        UInt256::rand(),
        UInt256::rand(),
        Default::default(),
        Default::default(),
    );
    assert!(!VoteResult::Misbehavior(proof).is_ok());
    assert!(VoteResult::Misbehavior(MisbehaviorProof::conflicting_votes(
        SlotIndex::new(0),
        ValidatorIndex(0),
        ConflictingVoteType::Notarize,
        UInt256::rand(),
        UInt256::rand(),
        Default::default(),
        Default::default()
    ))
    .is_err());

    assert!(!VoteResult::Rejected("test".to_string()).is_ok());
    assert!(VoteResult::Rejected("test".to_string()).is_err());
}

#[test]
fn test_vote_result_unwrap_applied() {
    // Should not panic
    VoteResult::Applied.unwrap();
}

#[test]
fn test_vote_result_unwrap_duplicate_succeeds() {
    // Duplicate is treated as success for unwrap() (compatible with old Ok(()) behavior)
    VoteResult::Duplicate.unwrap();
}

#[test]
#[should_panic(expected = "VoteResult::unwrap() called on Rejected")]
fn test_vote_result_unwrap_rejected_panics() {
    VoteResult::Rejected("test".to_string()).unwrap();
}

#[test]
fn test_vote_result_expect_applied() {
    // Should not panic
    VoteResult::Applied.expect("vote should be applied");
}

#[test]
#[should_panic(expected = "custom message")]
fn test_vote_result_expect_rejected_panics() {
    VoteResult::Rejected("test".to_string()).expect("custom message");
}

#[test]
fn test_misbehavior_proof_display_conflicting_votes() {
    let hash1 = UInt256::rand();
    let hash2 = UInt256::rand();
    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(42),
        ValidatorIndex(5),
        ConflictingVoteType::Notarize,
        hash1.clone(),
        hash2.clone(),
        RawVoteData::default(),
        RawVoteData::default(),
    );
    let display = proof.to_string();
    // Should contain: vote type, validator, slot, both hash prefixes
    assert!(display.contains("notarize"), "got: {}", display);
    assert!(display.contains("v005"), "got: {}", display);
    assert!(display.contains("slot 42"), "got: {}", display);
    // Hashes are formatted as 8-char prefixes
    assert!(display.contains(&hash1.to_hex_string()[..8]), "got: {}", display);
    assert!(display.contains(&hash2.to_hex_string()[..8]), "got: {}", display);
}

#[test]
fn test_misbehavior_proof_display_conflicting_types() {
    let finalize_hash = UInt256::rand();
    let proof = MisbehaviorProof::conflicting_types(
        SlotIndex::new(100),
        ValidatorIndex(3),
        VoteDescriptor::Skip,
        VoteDescriptor::Finalize(finalize_hash.clone()),
        RawVoteData::default(),
        RawVoteData::default(),
        ConflictReason::FinalizeAfterSkip,
    );
    let display = proof.to_string();
    // Should contain: reason, validator, slot, existing and new vote descriptions
    assert!(display.contains("finalize after skip"), "got: {}", display);
    assert!(display.contains("v003"), "got: {}", display);
    assert!(display.contains("slot 100"), "got: {}", display);
    assert!(display.contains("existing=skip"), "got: {}", display);
    // New vote is formatted with hash prefix
    assert!(display.contains("new=finalize:"), "got: {}", display);
    assert!(display.contains(&finalize_hash.to_hex_string()[..8]), "got: {}", display);
}

#[test]
fn test_misbehavior_proof_accessors() {
    let hash1 = UInt256::rand();
    let hash2 = UInt256::rand();
    let proof = MisbehaviorProof::conflicting_votes(
        SlotIndex::new(77),
        ValidatorIndex(11),
        ConflictingVoteType::Finalize,
        hash1.clone(),
        hash2.clone(),
        RawVoteData::default(),
        RawVoteData::default(),
    );
    assert_eq!(proof.slot(), SlotIndex::new(77));
    assert_eq!(proof.validator_idx(), ValidatorIndex(11));
    assert_eq!(proof.hash1(), Some(&hash1));
    assert_eq!(proof.hash2(), Some(&hash2));

    let notarize_hash = UInt256::rand();
    let proof2 = MisbehaviorProof::conflicting_types(
        SlotIndex::new(88),
        ValidatorIndex(22),
        VoteDescriptor::Skip,
        VoteDescriptor::Notarize(notarize_hash.clone()),
        RawVoteData::default(),
        RawVoteData::default(),
        ConflictReason::NotarizeAfterSkip,
    );
    assert_eq!(proof2.slot(), SlotIndex::new(88));
    assert_eq!(proof2.validator_idx(), ValidatorIndex(22));
    // ConflictingVoteTypes doesn't have hash accessors (those are for ConflictingVotes)
    assert_eq!(proof2.hash1(), None);
    assert_eq!(proof2.hash2(), None);
    // But we can access the vote descriptors
    assert_eq!(proof2.existing_vote(), Some(&VoteDescriptor::Skip));
    assert_eq!(proof2.new_vote(), Some(&VoteDescriptor::Notarize(notarize_hash)));
}
