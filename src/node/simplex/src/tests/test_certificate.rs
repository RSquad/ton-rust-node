/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for certificate module
//!
//! Tests vote signature and certificate serialization/deserialization,
//! signature verification, and weight threshold checking.

use crate::{
    block::{SlotIndex, ValidatorIndex},
    certificate::{Certificate, NotarCert, ToTlUnsignedVote, VoteSignature},
    session_description::SessionDescription,
    simplex_state::{NotarizeVote, Vote},
    utils::sign_vote,
    PrivateKey, SessionId, SessionNode, SessionOptions,
};
use std::time::SystemTime;
use ton_api::{
    serialize_boxed,
    ton::consensus::simplex::{UnsignedVote, VoteSignatureSet},
};
use ton_block::{Ed25519KeyOption, ShardIdent, UInt256, ZeroizingBytes};

/// Create test session description with given number of validators
fn create_test_session_desc(num_validators: usize) -> (SessionDescription, Vec<PrivateKey>) {
    let mut nodes = Vec::with_capacity(num_validators);
    let mut keys = Vec::with_capacity(num_validators);

    for _ in 0..num_validators {
        let key = Ed25519KeyOption::<ZeroizingBytes>::generate().expect("Failed to generate key");
        let adnl_id = key.id().clone();
        nodes.push(SessionNode {
            public_key: key.clone(),
            adnl_id,
            weight: 100, // Equal weight for simplicity
        });
        keys.push(key);
    }

    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();

    let desc = SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .expect("Failed to create session description");

    (desc, keys)
}

/// Create a session ID for testing
fn create_test_session_id() -> SessionId {
    UInt256::from_slice(&[0xAB; 32])
}

/// Create a test notarize vote
fn create_test_vote(slot: u32) -> NotarizeVote {
    NotarizeVote { slot: SlotIndex::new(slot), block_hash: UInt256::from_slice(&[0x42; 32]) }
}

/*
    ============================================================================
    VoteSignature Tests
    ============================================================================
*/

#[test]
fn test_vote_signature_creation() {
    let sig = VoteSignature::new(ValidatorIndex::new(5), vec![1, 2, 3, 4]);
    assert_eq!(sig.validator_idx.value(), 5);
    assert_eq!(sig.signature, vec![1, 2, 3, 4]);
}

#[test]
fn test_vote_signature_tl_roundtrip() {
    let original = VoteSignature::new(ValidatorIndex::new(42), vec![0xDE, 0xAD, 0xBE, 0xEF]);

    // Convert to TL
    let tl = original.to_tl();

    // Convert back
    let restored = VoteSignature::from_tl(&tl);

    assert_eq!(restored.validator_idx, original.validator_idx);
    assert_eq!(restored.signature, original.signature);
}

/*
    ============================================================================
    Certificate Tests
    ============================================================================
*/

#[test]
fn test_certificate_creation() {
    let vote = create_test_vote(10);
    let signatures = vec![VoteSignature::new(ValidatorIndex::new(0), vec![1, 2, 3])];

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);

    assert_eq!(cert.vote.slot.value(), 10);
    assert_eq!(cert.signatures.len(), 1);
}

#[test]
fn test_certificate_total_weight() {
    let (desc, _keys) = create_test_session_desc(4);
    let vote = create_test_vote(5);

    // Create certificate with 2 validators (each weight 100)
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![]),
        VoteSignature::new(ValidatorIndex::new(1), vec![]),
    ];

    let cert: NotarCert = Certificate::new(vote, signatures);

    // Total weight should be 200 (2 validators * 100 weight each)
    assert_eq!(cert.total_weight(&desc), 200);
}

#[test]
fn test_notar_cert_from_tl_bytes_for_candidate_binds_vote_context() {
    // Build VoteSignatureSet bytes (boxed) from any certificate
    let dummy_vote =
        NotarizeVote { slot: SlotIndex::new(0), block_hash: UInt256::from([0x11; 32]) };
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(1), vec![1, 2, 3]),
        VoteSignature::new(ValidatorIndex::new(2), vec![4, 5, 6]),
    ];
    let cert: NotarCert = Certificate::new(dummy_vote, signatures.clone());
    let tl_set = cert.to_tl_vote_signature_set();
    let bytes = serialize_boxed(&tl_set).expect("serialize VoteSignatureSet");

    // Parse and bind to a specific candidate context
    let slot = SlotIndex::new(123);
    let block_hash = UInt256::from([0xAB; 32]);
    let parsed = NotarCert::from_tl_bytes_for_candidate(&bytes, slot, block_hash.clone())
        .expect("parse notar cert from VoteSignatureSet bytes");

    assert_eq!(parsed.vote.slot, slot);
    assert_eq!(parsed.vote.block_hash, block_hash);
    assert_eq!(parsed.signatures, signatures);
}

#[test]
fn test_certificate_weight_threshold() {
    let (desc, _keys) = create_test_session_desc(4);
    let vote = create_test_vote(5);

    // 2 of 4 validators = 50% - not sufficient (need > 2/3)
    let cert_insufficient: NotarCert = Certificate::new(
        vote.clone(),
        vec![
            VoteSignature::new(ValidatorIndex::new(0), vec![]),
            VoteSignature::new(ValidatorIndex::new(1), vec![]),
        ],
    );
    assert!(!cert_insufficient.has_sufficient_weight(&desc));

    // 3 of 4 validators = 75% - sufficient (> 2/3)
    let cert_sufficient: NotarCert = Certificate::new(
        vote,
        vec![
            VoteSignature::new(ValidatorIndex::new(0), vec![]),
            VoteSignature::new(ValidatorIndex::new(1), vec![]),
            VoteSignature::new(ValidatorIndex::new(2), vec![]),
        ],
    );
    assert!(cert_sufficient.has_sufficient_weight(&desc));
}

#[test]
fn test_certificate_to_tl_vote_signature_set() {
    let vote = create_test_vote(5);
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![1, 2, 3]),
        VoteSignature::new(ValidatorIndex::new(1), vec![4, 5, 6]),
    ];

    let cert: NotarCert = Certificate::new(vote, signatures);
    let tl_sig_set = cert.to_tl_vote_signature_set();

    // Check it's the right type
    match &tl_sig_set {
        VoteSignatureSet::Consensus_Simplex_VoteSignatureSet(inner) => {
            assert_eq!(inner.votes.len(), 2);
        }
    }
}

#[test]
fn test_certificate_tl_roundtrip() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign the vote with 3 validators (>= 2/3 threshold)
    let mut signatures = Vec::new();
    for i in 0..3 {
        let fsm_vote = Vote::Notarize(vote.clone());
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }

    let original: NotarCert = Certificate::new(vote.clone(), signatures);

    // Convert to TL
    let tl_cert = original.to_tl().expect("to_tl failed");

    // Parse back with verification
    let restored =
        Certificate::<Vote>::from_tl(&tl_cert, &desc, &session_id).expect("from_tl failed");

    // Check vote matches
    match &restored.vote {
        Vote::Notarize(v) => {
            assert_eq!(v.slot, vote.slot);
            assert_eq!(v.block_hash, vote.block_hash);
        }
        _ => panic!("Expected NotarizeVote"),
    }

    // Check signatures count
    assert_eq!(restored.signatures.len(), 3);
}

#[test]
fn test_certificate_from_tl_signatures() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(7);

    // Sign the vote with 3 validators
    let mut signatures = Vec::new();
    for i in 0..3 {
        let fsm_vote = Vote::Notarize(vote.clone());
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }

    // Create TL signature set
    let temp_cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_sigs = temp_cert.to_tl_vote_signature_set();

    // Parse from TL with separate vote
    let parsed: NotarCert =
        Certificate::from_tl_signatures(&tl_sigs, vote.clone(), &desc, &session_id)
            .expect("from_tl_signatures failed");

    assert_eq!(parsed.vote.slot, vote.slot);
    assert_eq!(parsed.signatures.len(), 3);
    assert!(parsed.has_sufficient_weight(&desc));
}

/*
    ============================================================================
    Error Cases
    ============================================================================
*/

#[test]
fn test_certificate_invalid_validator_index() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign with valid key
    let fsm_vote = Vote::Notarize(vote.clone());
    let signed = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote failed");

    // Create signature with invalid validator index (>= num_validators)
    let signatures = vec![VoteSignature::new(
        ValidatorIndex::new(100), // Invalid: only 4 validators
        signed.signature().to_vec(),
    )];

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_sigs = cert.to_tl_vote_signature_set();

    // Should fail due to invalid validator index
    let result = NotarCert::from_tl_signatures(&tl_sigs, vote, &desc, &session_id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid validator index"));
}

#[test]
fn test_certificate_duplicate_validator_rejected() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign with validator 0
    let fsm_vote = Vote::Notarize(vote.clone());
    let signed = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote failed");

    // Create certificate with duplicate validator
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), signed.signature().to_vec()),
        VoteSignature::new(ValidatorIndex::new(0), signed.signature().to_vec()), // Duplicate!
    ];

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_sigs = cert.to_tl_vote_signature_set();

    // Should fail due to duplicate validator
    let result = NotarCert::from_tl_signatures(&tl_sigs, vote, &desc, &session_id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate validator"));
}

#[test]
fn test_certificate_invalid_signature() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign with validator 0's key but claim it's from validator 1
    let fsm_vote = Vote::Notarize(vote.clone());
    let signed = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote failed");

    // Create signature claiming to be from validator 1 (but signed by validator 0)
    let signatures = vec![VoteSignature::new(
        ValidatorIndex::new(1), // Wrong validator!
        signed.signature().to_vec(),
    )];

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_sigs = cert.to_tl_vote_signature_set();

    // Should fail signature verification
    let result = NotarCert::from_tl_signatures(&tl_sigs, vote, &desc, &session_id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid vote signature"));
}

#[test]
fn test_certificate_insufficient_weight() {
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign with only 1 validator (25% < 2/3)
    let fsm_vote = Vote::Notarize(vote.clone());
    let signed = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote failed");

    let signatures = vec![VoteSignature::new(ValidatorIndex::new(0), signed.signature().to_vec())];

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_sigs = cert.to_tl_vote_signature_set();

    // Should fail weight threshold check
    let result = NotarCert::from_tl_signatures(&tl_sigs, vote, &desc, &session_id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Not enough signatures"));
}

/*
    ============================================================================
    ToTlUnsignedVote Trait Tests
    ============================================================================
*/

#[test]
fn test_to_tl_unsigned_vote_notarize() {
    let vote = create_test_vote(5);
    let tl = vote.to_tl_unsigned().expect("to_tl_unsigned failed");

    match tl {
        UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
            assert_eq!(*v.id.slot(), 5);
        }
        _ => panic!("Expected NotarizeVote"),
    }
}

#[test]
fn test_to_vote_roundtrip() {
    let original = create_test_vote(10);
    let vote_enum = original.to_vote();

    match vote_enum {
        Vote::Notarize(v) => {
            assert_eq!(v.slot, original.slot);
            assert_eq!(v.block_hash, original.block_hash);
        }
        _ => panic!("Expected NotarizeVote"),
    }
}

/*
    ============================================================================
    Strict Certificate Parsing Tests (from_tl for network certificates)
    ============================================================================

    Tests for C++-compatible strict handling in `Certificate::<Vote>::from_tl()`
    (used for parsing incoming `consensus.simplex.certificate` messages).

    Reference: C++ `certificate.cpp::Certificate<T>::from_tl(...)`:
    - invalid validator index => reject
    - duplicate validator index => reject
    - invalid signature => reject
    - insufficient weight => reject
*/

#[test]
fn test_certificate_from_tl_rejects_duplicates() {
    // C++ strict: duplicates are rejected
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(5);

    // Sign with validators 0, 1, 2 + duplicate validator 0
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..3 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }
    // Add duplicate signature from validator 0
    let signed0_dup = sign_vote(&fsm_vote, &session_id, &keys[0]).expect("sign_vote failed");
    signatures.push(VoteSignature::new(ValidatorIndex::new(0), signed0_dup.signature().to_vec()));

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_cert = cert.to_tl().expect("to_tl failed");

    let result = Certificate::<Vote>::from_tl(&tl_cert, &desc, &session_id);
    assert!(result.is_err(), "Certificate with duplicates must be rejected");
    assert!(result.unwrap_err().to_string().contains("Duplicate validator index"));
}

#[test]
fn test_certificate_from_tl_rejects_invalid_validator_index() {
    // C++ strict: invalid validator indices are rejected
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(6);

    // Sign with validators 0, 1, 2 (enough for 2/3 threshold)
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..3 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }
    // Add signature from invalid validator index 100 (must be rejected)
    signatures.push(VoteSignature::new(ValidatorIndex::new(100), vec![0xAB; 64]));

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_cert = cert.to_tl().expect("to_tl failed");

    let result = Certificate::<Vote>::from_tl(&tl_cert, &desc, &session_id);
    assert!(result.is_err(), "Certificate with invalid validator index must be rejected");
    assert!(result.unwrap_err().to_string().contains("Invalid validator index"));
}

#[test]
fn test_certificate_from_tl_rejects_insufficient_weight() {
    // C++ strict: reject if weight < 2/3 threshold
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(7);

    // Sign with only 2 validators (2/3 threshold requires 3 out of 4 with equal weights)
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..2 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_cert = cert.to_tl().expect("to_tl failed");

    let result = Certificate::<Vote>::from_tl(&tl_cert, &desc, &session_id);
    assert!(result.is_err(), "Certificate with insufficient weight should be rejected");
    assert!(result.unwrap_err().to_string().contains("Not enough signatures"));
}

#[test]
fn test_certificate_from_tl_rejects_invalid_signature() {
    // Test that certificate is rejected if any valid-indexed signature fails verification
    let (desc, keys) = create_test_session_desc(4);
    let session_id = create_test_session_id();
    let vote = create_test_vote(8);

    // Sign with validators 0, 1 correctly
    let fsm_vote = Vote::Notarize(vote.clone());
    let mut signatures = Vec::new();
    for i in 0..2 {
        let signed = sign_vote(&fsm_vote, &session_id, &keys[i]).expect("sign_vote failed");
        signatures
            .push(VoteSignature::new(ValidatorIndex::new(i as u32), signed.signature().to_vec()));
    }
    // Add invalid signature: validator 2 claims to sign but uses wrong key (validator 3's)
    let wrong_sig = sign_vote(&fsm_vote, &session_id, &keys[3]).expect("sign_vote failed");
    signatures.push(VoteSignature::new(ValidatorIndex::new(2), wrong_sig.signature().to_vec()));

    let cert: NotarCert = Certificate::new(vote.clone(), signatures);
    let tl_cert = cert.to_tl().expect("to_tl failed");

    // from_tl should fail - invalid signature
    let result = Certificate::<Vote>::from_tl(&tl_cert, &desc, &session_id);
    assert!(result.is_err(), "Certificate with invalid signature should be rejected");
    assert!(result.unwrap_err().to_string().contains("Invalid vote signature"));
}
