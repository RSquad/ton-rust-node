/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for SimplexState FSM
//!
//! These tests are included directly from simplex_state.rs via #[path] attribute
//! to access private internals without requiring pub(crate) visibility.

use super::*;
use crate::{
    block::{SlotIndex, ValidatorIndex, WindowIndex},
    misbehavior::VoteResult,
    RawVoteData, SessionId, SessionNode,
};
use std::{
    iter::from_fn,
    sync::Arc,
    time::{Duration, SystemTime},
};
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, UInt256};

/// Test helper trait to provide on_vote with default raw_vote
trait SimplexStateTestExt {
    /// Call on_vote with default raw_vote (for tests that don't need serialized bytes)
    fn on_vote_test(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: Vote,
        signature: Vec<u8>,
    ) -> VoteResult;
}

impl SimplexStateTestExt for SimplexState {
    fn on_vote_test(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: Vote,
        signature: Vec<u8>,
    ) -> VoteResult {
        // Use default raw_vote for tests - actual bytes not needed for most test scenarios
        self.on_vote(desc, validator_idx, vote, signature, RawVoteData::default())
    }
}

/// Create test validators with equal weights
fn create_test_validators(count: u32) -> Vec<SessionNode> {
    (0..count)
        .map(|_i| {
            let public_key = Ed25519KeyOption::generate().expect("Failed to generate key");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight: 1 }
        })
        .collect()
}

/// Create test SessionDescription
fn create_test_desc(count: u32, slots_per_window: u32) -> SessionDescription {
    let nodes = create_test_validators(count);
    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::masterchain();

    let mut opts = crate::SessionOptions::default();
    opts.slots_per_leader_window = slots_per_window;

    SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .unwrap()
}

/// Create test SessionDescription with specific weights
fn create_test_desc_weights(
    count: u32,
    slots_per_window: u32,
    weights: Vec<u64>,
) -> SessionDescription {
    assert_eq!(count as usize, weights.len(), "weights count must match validator count");

    let nodes: Vec<SessionNode> = weights
        .into_iter()
        .map(|weight| {
            let public_key = Ed25519KeyOption::generate().expect("Failed to generate key");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight }
        })
        .collect();

    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::masterchain();

    let mut opts = crate::SessionOptions::default();
    opts.slots_per_leader_window = slots_per_window;

    SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .unwrap()
}

/// Helper to create SimplexStateOptions for C++ compatible mode
fn opts_cpp() -> SimplexStateOptions {
    SimplexStateOptions::cpp_compatible()
}

/// Helper to create SimplexStateOptions with notarized-parent chain enabled (pool.cpp parity mode)
fn opts_notarized_parent_chain() -> SimplexStateOptions {
    opts_cpp()
}

/// Helper to create test candidate for FSM tests
///
/// Creates a minimal candidate for testing. Uses stub block to avoid
/// invariant violations (test candidates are not real empty blocks).
fn create_test_candidate(
    slot: u32,
    hash: UInt256,
    block_id: BlockIdExt,
    parent: Option<(u32, UInt256)>,
    leader: u32,
) -> Candidate {
    let parent_id = parent.map(|(p_slot, p_hash)| crate::block::CandidateId {
        slot: SlotIndex::new(p_slot),
        hash: p_hash,
        block: BlockIdExt::default(),
    });

    // Create stub block for test - uses the block_id we provide
    let stub_block = crate::block::BlockCandidate {
        id: block_id.clone(),
        collated_file_hash: UInt256::default(),
        data: vec![],
        collated_data: vec![],
        creator: Ed25519KeyOption::generate().unwrap(),
    };

    Candidate::new(
        crate::block::CandidateId { slot: SlotIndex::new(slot), hash, block: block_id },
        parent_id,
        ValidatorIndex::new(leader),
        Some(stub_block),
        vec![],
    )
}

/// Helper to create a stub block for tests
fn create_stub_block(block_id: BlockIdExt) -> crate::block::BlockCandidate {
    crate::block::BlockCandidate {
        id: block_id,
        collated_file_hash: UInt256::default(),
        data: vec![],
        collated_data: vec![],
        creator: Ed25519KeyOption::generate().unwrap(),
    }
}

/// Helper to create SimplexStateOptions for Alpenglow mode
fn opts_alpenglow() -> SimplexStateOptions {
    SimplexStateOptions::alpenglow()
}

#[test]
fn test_new_creates_initial_state() {
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(0));
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));
    assert!(!state.has_pending_events());
    assert!(state.get_next_timeout().is_none());

    // Window 0 should have None (genesis) as available base
    assert!(state.get_window(WindowIndex::new(0)).unwrap().available_bases.contains(&None));
}

#[test]
fn test_new_validates_slots_per_leader_window() {
    // Create a description with 0 slots_per_leader_window
    // Note: This requires manually constructing with invalid parameters
    let nodes = create_test_validators(4);
    let local_key = nodes[0].public_key.clone();

    let mut opts = crate::SessionOptions::default();
    opts.slots_per_leader_window = 0; // Invalid: must be > 0

    // SessionDescription::new may or may not validate this
    // If it does, the test still passes (we can't create invalid state)
    let shard = ShardIdent::masterchain();
    if let Ok(desc) = SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    ) {
        match SimplexState::new(&desc, opts_cpp()) {
            Ok(_) => panic!("SimplexState::new should fail with slots_per_leader_window=0"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("slots_per_leader_window"),
                    "Error should mention slots_per_leader_window, got: {}",
                    msg
                );
            }
        }
    }
}

#[test]
fn test_on_candidate_first_slot_with_genesis_parent() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Create candidate for slot 0 with genesis parent (None)
    let candidate = create_test_candidate(0, UInt256::default(), BlockIdExt::default(), None, 0);

    state.on_candidate(&desc, candidate).expect("on_candidate should succeed");

    // Should have broadcast NotarVote
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(_)))),
        "Expected NotarVote broadcast, got {:?}",
        events
    );
}

#[test]
fn test_on_candidate_stores_pending_when_no_parent() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Candidate for slot 1 with parent at slot 0, but parent isn't notarized yet
    // so it can't be resolved → candidate stored as pending
    let parent_hash = UInt256::from_slice(&[1u8; 32]);

    let candidate = create_test_candidate(
        1,
        UInt256::default(),
        BlockIdExt::default(),
        Some((0, parent_hash)),
        0,
    );

    state.on_candidate(&desc, candidate).expect("on_candidate should succeed");

    // Should NOT broadcast (parent not available in window state)
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(_)))),
        "Should not broadcast NotarVote without parent, got {:?}",
        events
    );

    // Should have pending block (slot 1 = offset 1 in window 0)
    assert!(state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some());
}

#[test]
fn test_on_vote_notarize_updates_weights() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // Submit votes from validators
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new())
        .expect("on_vote should succeed");
    state
        .on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new())
        .expect("on_vote should succeed");
    state
        .on_vote_test(&desc, ValidatorIndex::new(2), vote.clone(), Vec::new())
        .expect("on_vote should succeed");

    // Check weight accumulated (threshold_66 for 4 validators = 3)
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert_eq!(sv.notarize_weight_by_block.get(&block.root_hash).copied().unwrap(), 3);
}

#[test]
fn test_on_vote_skip_after_notarize_allowed_without_fallback() {
    // When enable_fallback_protocol=false (C++ compatible mode):
    // Skip after Notarize is ALLOWED per C++ pool.cpp check_invariants()
    // C++ only checks finalize+skip as misbehavior, not notarize+skip
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let notarize = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // Notarize first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new())
        .expect("on_vote should succeed");

    // Skip should be ALLOWED (matches C++ behavior)
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new());
    assert!(
        result.is_ok(),
        "Skip after notarize should succeed with enable_fallback_protocol=false (C++ mode)"
    );

    // Should have both notarize and skip
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(sv.votes[0].notarize.is_some());
    assert!(sv.votes[0].skip.is_some());
}

#[test]
fn test_on_vote_skip_after_notarize_misbehavior_with_fallback() {
    // When enable_fallback_protocol=true (full Alpenglow):
    // Skip after Notarize is MISBEHAVIOR
    // In Alpenglow, once you vote notarize (fast path), you shouldn't also vote skip
    let desc = create_test_desc(4, 1);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let notarize = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // Notarize first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new())
        .expect("on_vote should succeed");

    // Skip should be REJECTED (misbehavior in full Alpenglow mode)
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new());
    assert!(
        result.is_err(),
        "Skip after notarize should fail with enable_fallback_protocol=true (Alpenglow mode)"
    );

    // Should only have notarize
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(sv.votes[0].notarize.is_some());
    assert!(sv.votes[0].skip.is_none());
}

#[test]
fn test_debug_dump_format() {
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Test full dump format
    let full_dump = state.debug_dump(&desc, true);
    assert!(full_dump.contains("SimplexState dump:"));
    assert!(full_dump.contains("validators: 4"));
    assert!(full_dump.contains("leader_windows:"));

    // Test compact dump format
    let compact_dump = state.debug_dump(&desc, false);
    assert!(compact_dump.starts_with("SimplexState: w"));
    assert!(compact_dump.contains("flags="));
    assert!(compact_dump.contains("th66/33="));
    assert!(compact_dump.contains("bases=["));
    assert!(compact_dump.contains("evts=["));
}

/*
    ========================================================================
    Misbehavior Detection Tests
    ========================================================================
    Reference: C++ pool.cpp Votes::check_invariants
*/

#[test]
fn test_notarize_after_skip_allowed_without_fallback() {
    // When enable_fallback_protocol=false (C++ compatible mode):
    // Notarize after Skip is ALLOWED per C++ pool.cpp check_invariants()
    // C++ only checks finalize+skip as misbehavior, not notarize+skip
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let notarize = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // Skip first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new())
        .expect("skip should succeed");

    // Notarize should be ALLOWED (matches C++ behavior)
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new());
    assert!(
        result.is_ok(),
        "Notarize after skip should succeed with enable_fallback_protocol=false (C++ mode)"
    );

    // Should have both skip and notarize
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(sv.votes[0].skip.is_some());
    assert!(sv.votes[0].notarize.is_some());
}

#[test]
fn test_notarize_after_skip_misbehavior_with_fallback() {
    // When enable_fallback_protocol=true (full Alpenglow):
    // Notarize after Skip is MISBEHAVIOR
    // In Alpenglow, once you vote skip, you shouldn't also vote notarize
    let desc = create_test_desc(4, 1);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let notarize = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // Skip first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new())
        .expect("skip should succeed");

    // Notarize should be REJECTED (misbehavior in full Alpenglow mode)
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new());
    assert!(
        result.is_err(),
        "Notarize after skip should fail with enable_fallback_protocol=true (Alpenglow mode)"
    );

    // Should only have skip
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(sv.votes[0].skip.is_some());
    assert!(sv.votes[0].notarize.is_none());
}

#[test]
fn test_misbehavior_conflicting_notarize_votes() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block1 = BlockIdExt::default();
    let mut block2 = BlockIdExt::default();
    // Use different root_hash - that's what we compare for conflicts
    block2.root_hash = UInt256::from([0xABu8; 32]);

    let notarize1 = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block1.root_hash.clone(),
    });
    let notarize2 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block2.root_hash });

    // First notarize vote succeeds
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), notarize1, Vec::new())
        .expect("first notarize should succeed");

    // Second different notarize vote should be rejected
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), notarize2, Vec::new());
    assert!(result.is_err(), "Conflicting notarize should return error");

    // Should still have first block (compare block_hash since that's what matters)
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert_eq!(sv.votes[0].notarize.as_ref().unwrap().block_hash, block1.root_hash);
}

#[test]
fn test_misbehavior_finalize_after_skip() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let finalize =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block.root_hash });

    // Skip first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new())
        .expect("skip should succeed");

    // Finalize should be rejected
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), finalize, Vec::new());
    assert!(result.is_err(), "Finalize after skip should return error");
}

#[test]
fn test_misbehavior_finalize_after_notar_fallback() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let notar_fb = Vote::NotarizeFallback(NotarizeFallbackVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    let finalize =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block.root_hash });

    // Notar-fallback first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), notar_fb, Vec::new())
        .expect("notar-fallback should succeed");

    // Finalize should be rejected
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), finalize, Vec::new());
    assert!(result.is_err(), "Finalize after notar-fallback should return error");
}

#[test]
fn test_misbehavior_finalize_after_skip_fallback() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let skip_fb = Vote::SkipFallback(SkipFallbackVote { slot: SlotIndex::new(0) });
    let finalize =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block.root_hash });

    // Skip-fallback first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), skip_fb, Vec::new())
        .expect("skip-fallback should succeed");

    // Finalize should be rejected
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), finalize, Vec::new());
    assert!(result.is_err(), "Finalize after skip-fallback should return error");
}

#[test]
fn test_misbehavior_too_many_notar_fallback() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Send MAX_NOTAR_FALLBACK_VOTES_PER_VALIDATOR (3) notar-fallback votes
    for i in 0..3u8 {
        let hash = UInt256::from([i; 32]);
        let vote = Vote::NotarizeFallback(NotarizeFallbackVote {
            slot: SlotIndex::new(0),
            block_hash: hash,
        });
        state
            .on_vote_test(&desc, ValidatorIndex::new(0), vote, Vec::new())
            .expect("notar-fallback should succeed");
    }

    // 4th vote should be rejected
    let hash = UInt256::from([0xFFu8; 32]);
    let vote =
        Vote::NotarizeFallback(NotarizeFallbackVote { slot: SlotIndex::new(0), block_hash: hash });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), vote, Vec::new());
    assert!(result.is_err(), "Too many notar-fallback should return error");
}

#[test]
fn test_misbehavior_invalid_skip_range() {
    // SkipVote now uses a single slot field, not slot_begin/slot_end
    // This test verifies that skip votes work correctly with the new structure
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Valid skip vote for slot 0
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), skip, Vec::new());
    assert!(result.is_ok(), "Valid skip vote should succeed");
}

#[test]
fn test_misbehavior_invalid_skip_fallback_range() {
    // SkipFallbackVote now uses a single slot field, not slot_begin/slot_end
    // This test verifies that skip fallback votes work correctly with the new structure
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Valid skip fallback vote for slot 0
    let skip_fb = Vote::SkipFallback(SkipFallbackVote { slot: SlotIndex::new(0) });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), skip_fb, Vec::new());
    assert!(result.is_ok(), "Valid skip fallback vote should succeed");
}

#[test]
fn test_misbehavior_notarize_finalize_hash_mismatch() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let hash_a = UInt256::from([0xAAu8; 32]);
    let hash_b = UInt256::from([0xBBu8; 32]);

    let finalize = Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: hash_a });
    let notarize = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: hash_b });

    // Finalize for hash A first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), finalize, Vec::new())
        .expect("finalize should succeed");

    // Notarize for different hash B should be rejected as misbehavior
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new());
    assert!(
        result.is_err(),
        "Notarize for different hash than existing finalize should be misbehavior"
    );
}

#[test]
fn test_misbehavior_finalize_notarize_hash_mismatch() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let hash_a = UInt256::from([0xAAu8; 32]);
    let hash_b = UInt256::from([0xBBu8; 32]);

    let notarize = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: hash_a });
    let finalize = Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: hash_b });

    // Notarize for hash A first
    state
        .on_vote_test(&desc, ValidatorIndex::new(0), notarize, Vec::new())
        .expect("notarize should succeed");

    // Finalize for different hash B should be rejected as misbehavior
    let result = state.on_vote_test(&desc, ValidatorIndex::new(0), finalize, Vec::new());
    assert!(
        result.is_err(),
        "Finalize for different hash than existing notarize should be misbehavior"
    );
}

/*
    ========================================================================
    Vote Accounting Tests - Threshold Triggers
    ========================================================================
    Reference: C++ pool.cpp check_and_publish_events
*/

#[test]
fn test_notarize_threshold_66_triggers_block_notarized() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();

    // After 2 votes, should NOT have triggered BlockNotarized
    assert!(!state.slot_votes.get(&SlotIndex::new(0)).unwrap().block_notarized_published);

    // 3rd vote triggers threshold
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Should have triggered BlockNotarized
    assert!(state.slot_votes.get(&SlotIndex::new(0)).unwrap().block_notarized_published);
}

#[test]
fn test_safe_to_notar_requires_both_thresholds() {
    // SafeToNotar requires ALL conditions (Alpenglow White Paper):
    // 1. notar(b) >= 1/3 (threshold_33)
    // 2. skip(s) + notar(b) >= 2/3 (threshold_66)
    // 3. notar(b) < 2/3 (threshold_66) - fallback only when normal path fails
    //
    // SafeToNotar is a FALLBACK mechanism - it only triggers when notar alone
    // isn't enough for BlockNotarized. If notar >= 2/3, BlockNotarized handles
    // it via the normal path.
    //
    // NOTE: enable_fallback_protocol=true to test fallback threshold logic
    let desc = create_test_desc(4, 1);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // With 4 validators: threshold_33 = 2, threshold_66 = 3

    // 1 vote: threshold_33 not met
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    assert!(!state
        .slot_votes
        .get(&SlotIndex::new(0))
        .unwrap()
        .safe_to_notar_blocks
        .contains(&block.root_hash));

    // 2 votes: threshold_33 met, but skip + notar = 0 + 2 < 3 (threshold_66)
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    assert!(
        !state
            .slot_votes
            .get(&SlotIndex::new(0))
            .unwrap()
            .safe_to_notar_blocks
            .contains(&block.root_hash),
        "SafeToNotar should NOT trigger at threshold_33 alone"
    );

    // 3 votes: notar = 3 >= threshold_66
    // BlockNotarized triggers via normal path, so SafeToNotar should NOT trigger
    // (fallback is not needed when normal path succeeds)
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();
    assert!(
        !state
            .slot_votes
            .get(&SlotIndex::new(0))
            .unwrap()
            .safe_to_notar_blocks
            .contains(&block.root_hash),
        "SafeToNotar should NOT trigger when notar >= 2/3 (BlockNotarized handles it)"
    );

    // Verify that BlockNotarized WAS triggered
    assert!(
        state.slot_votes.get(&SlotIndex::new(0)).unwrap().block_notarized_published,
        "BlockNotarized should have triggered"
    );
}

#[test]
fn test_safe_to_notar_with_skip_votes() {
    // Test that skip votes contribute to the skip + notar >= 2/3 condition
    // NOTE: enable_fallback_protocol=true to test SafeToNotar logic
    let desc = create_test_desc(4, 1);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let notar_vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // With 4 validators: threshold_33 = 2, threshold_66 = 3

    // 1 skip vote + 1 notar vote: notar = 1 < threshold_33
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), notar_vote.clone(), Vec::new()).unwrap();
    assert!(
        !state
            .slot_votes
            .get(&SlotIndex::new(0))
            .unwrap()
            .safe_to_notar_blocks
            .contains(&block.root_hash),
        "SafeToNotar should NOT trigger: notar < threshold_33"
    );

    // Add another notar vote: notar = 2 >= threshold_33, skip + notar = 1 + 2 = 3 >= threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(2), notar_vote, Vec::new()).unwrap();
    assert!(
        state
            .slot_votes
            .get(&SlotIndex::new(0))
            .unwrap()
            .safe_to_notar_blocks
            .contains(&block.root_hash),
        "SafeToNotar should trigger: notar >= 1/3 AND skip + notar >= 2/3"
    );
}

#[test]
fn test_finalize_threshold_66_triggers_block_finalized() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();

    let vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Should have emitted BlockFinalized event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::BlockFinalized(_))),
        "Expected BlockFinalized event"
    );

    // slot_votes is cleaned up after finalization, so first_non_finalized_slot advances
    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1));
}

#[test]
fn test_safe_to_skip_broadcasts_skip_fallback_but_no_slot_skipped() {
    // SafeToSkip triggers at 1/3 threshold and calls try_skip_window
    // which broadcasts Skip votes for unvoted slots.
    // SlotSkipped is only emitted when skip certificate (2/3) is reached.
    // NOTE: enable_fallback_protocol=true to test SafeToSkip logic
    let desc = create_test_desc(4, 1);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // threshold_33 for 4 validators = 2
    // SafeToSkip condition: notarize_or_skip_weight >= threshold_33 + max_notarize
    // After 1 vote: notarize_or_skip_weight=1, threshold=2, max_notar=0, so 1 >= 2 is false
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();

    // After 1 vote, SafeToSkip should NOT have triggered
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Should not trigger SlotSkipped after 1 skip vote"
    );

    // 2nd vote: notarize_or_skip_weight=2, 2 >= 2 is true → SafeToSkip triggers
    // SafeToSkip calls try_skip_window which broadcasts Skip for unvoted slots
    // But SlotSkipped is NOT emitted yet - only at skip certificate (2/3)
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    // Check that we have a SkipFallback vote broadcast (from SafeToSkip)
    // or Skip vote (from try_skip_window for unvoted slots)
    assert!(
        events.iter().any(|e| matches!(
            e,
            SimplexEvent::BroadcastVote(Vote::Skip(_))
                | SimplexEvent::BroadcastVote(Vote::SkipFallback(_))
        )),
        "Expected Skip or SkipFallback vote broadcast"
    );

    // SlotSkipped should NOT be emitted yet (need 3 votes for threshold_66)
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "SlotSkipped should not be emitted until skip certificate (2/3)"
    );
}

#[test]
fn test_skip_certificate_threshold_66_triggers_slot_skipped() {
    // Skip certificate (>=2/3) triggers SlotSkipped event in check_thresholds_and_trigger
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    // Clear events from first vote
    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    // After 2 votes: SafeToSkip triggered, but NOT SlotSkipped (need 3 for skip certificate)
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "SlotSkipped should not trigger until skip certificate (2/3)"
    );

    // 3rd vote triggers skip certificate (threshold_66)
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Should have emitted SlotSkipped event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Expected SlotSkipped event at skip certificate threshold"
    );

    // C++ parity: skip does NOT advance first_non_finalized_slot (only finalization does).
    // But first_non_progressed_slot (C++ `now_`) does advance on skip.
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "first_non_finalized_slot should NOT advance on skip (C++ parity)"
    );
    assert_eq!(
        state.first_non_progressed_slot,
        SlotIndex::new(1),
        "first_non_progressed_slot should advance after skip certificate"
    );
}

#[test]
fn test_skip_certificate_reached_event_emitted_in_cpp_mode() {
    // In C++ compatible mode, reaching skip threshold should emit:
    // - SlotSkipped (internal progress)
    // - SkipCertificateReached (for broadcasting the skip certificate)
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let vote = Vote::Skip(SkipVote { slot });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![0]).unwrap();
    while state.pull_event().is_some() {}
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![1]).unwrap();
    while state.pull_event().is_some() {}

    // 3rd vote triggers skip certificate
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![2]).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Expected SlotSkipped at skip threshold"
    );

    let skip_cert_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::SkipCertificateReached(ev) => Some(ev),
            _ => None,
        })
        .collect();

    assert_eq!(skip_cert_events.len(), 1, "Expected exactly one SkipCertificateReached event");

    let ev = skip_cert_events[0];
    assert_eq!(ev.slot, slot, "event slot must match");
    assert_eq!(ev.certificate.vote.slot, slot, "certificate vote slot must match");
    assert_eq!(
        ev.certificate.signatures.len(),
        3,
        "certificate should include signatures from 3 validators"
    );

    let signer_idxs: Vec<u32> =
        ev.certificate.signatures.iter().map(|s| s.validator_idx.value()).collect();
    assert_eq!(signer_idxs, vec![0, 1, 2], "certificate should include signers 0,1,2");
}

#[test]
fn test_skip_certificate_reached_event_not_emitted_in_fallback_mode() {
    // In full Alpenglow (fallback) mode, SlotSkipped is still emitted at threshold_66,
    // but SkipCertificateReached should NOT be emitted (explicit skip cert broadcast is C++-mode-only).
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, SimplexStateOptions::alpenglow())
        .expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let vote = Vote::Skip(SkipVote { slot });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![0]).unwrap();
    while state.pull_event().is_some() {}
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![1]).unwrap();
    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![2]).unwrap();
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Expected SlotSkipped at skip threshold"
    );
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SkipCertificateReached(_))),
        "SkipCertificateReached must not be emitted in fallback mode"
    );
}

#[test]
fn test_slot_skipped_not_emitted_if_finalized() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();

    // First, finalize the slot
    let finalize_vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(0), finalize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), finalize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), finalize_vote, Vec::new()).unwrap();

    // Check that BlockFinalized event was emitted
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::BlockFinalized(_))),
        "Should have emitted BlockFinalized event"
    );

    // Now try to skip the same slot - but this slot is already past first_non_finalized_slot
    // so votes for it will be ignored
    // Note: BlockFinalized advances first_non_finalized_slot and cleans up slot_votes
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(1),
        "Finalized slot should advance first_non_finalized"
    );

    // Skip vote for slot 0 should return SlotAlreadyFinalized (slot < first_non_finalized_slot)
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(3), skip_vote, Vec::new());
    assert!(
        matches!(result, VoteResult::SlotAlreadyFinalized),
        "Late vote for finalized slot should return SlotAlreadyFinalized"
    );

    // Should NOT emit SlotSkipped since slot is already finalized
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Should not emit SlotSkipped for already finalized slot"
    );
}

#[test]
fn test_slot_skipped_not_emitted_twice() {
    // SlotSkipped only emits at skip certificate (2/3), not at SafeToSkip
    // With 5 validators, threshold_66 = (5*2+2)/3 = 4
    let desc = create_test_desc(5, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // threshold_66 for 5 validators = 4
    // Send 3 votes - not enough for skip certificate
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(2), vote.clone(), Vec::new()).unwrap();
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Should not emit SlotSkipped with 3 votes (need 4 for threshold_66)"
    );

    // 4th vote triggers skip certificate (threshold_66 = 4)
    state.on_vote_test(&desc, ValidatorIndex::new(3), vote.clone(), Vec::new()).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let skip_count = events.iter().filter(|e| matches!(e, SimplexEvent::SlotSkipped(_))).count();
    assert_eq!(skip_count, 1, "Should emit exactly one SlotSkipped at skip certificate");

    // 5th vote: C++ parity -- first_non_finalized_slot does NOT advance on skip,
    // so the slot is still "open" for vote reception (additional votes are accepted
    // but won't re-trigger SlotSkipped since the cert is already formed).
    let result = state.on_vote_test(&desc, ValidatorIndex::new(4), vote, Vec::new());
    assert!(
        matches!(result, VoteResult::Applied),
        "Vote for skipped slot should still be accepted (first_non_finalized_slot unchanged), got: {:?}",
        result
    );

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "Should not emit SlotSkipped twice for same slot"
    );
}

/*
    ========================================================================
    Corner Case Tests
    ========================================================================
*/

#[test]
fn test_ignore_finalized_slot_candidate() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Finalize slot 0 via normal path (3 finalize votes)
    let block = BlockIdExt::default();

    let vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Clear events and verify finalization
    while state.pull_event().is_some() {}
    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1));

    // Try to send candidate for finalized slot
    let candidate = create_test_candidate(0, UInt256::default(), BlockIdExt::default(), None, 0);

    // Should succeed but do nothing
    state.on_candidate(&desc, candidate).unwrap();

    // Should NOT have broadcast any vote
    assert!(!state.has_pending_events());
}

#[test]
fn test_ignore_finalized_slot_vote() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Finalize slot 0 via normal path (3 finalize votes)
    let block = BlockIdExt::default();

    let vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Clear events and verify finalization
    while state.pull_event().is_some() {}
    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1));

    // Try to send vote for finalized slot - should be rejected (benign, slot already finalized)
    let notarize_vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block.root_hash });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(3), notarize_vote, Vec::new());
    assert!(
        matches!(result, VoteResult::SlotAlreadyFinalized),
        "Late vote for finalized slot should return SlotAlreadyFinalized"
    );

    // Vote accounting for slot 0 still exists but shouldn't be updated further
    // (The vote was ignored because slot < first_non_finalized_slot)
    // Check no new events
    assert!(!state.has_pending_events());
}

#[test]
fn test_candidate_without_parent_accepted() {
    // C++ consensus.cpp:173 — C++ never rejects a candidate for missing parent.
    // It only validates parent_slot < candidate_slot when parent exists.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let candidate = Candidate::new(
        crate::block::CandidateId {
            slot: SlotIndex::new(1), // Non-first slot
            hash: UInt256::default(),
            block: BlockIdExt::default(),
        },
        None, // No parent — valid per C++
        ValidatorIndex::new(0),
        Some(create_stub_block(BlockIdExt::default())),
        vec![],
    );

    let result = state.on_candidate(&desc, candidate);
    assert!(result.is_ok(), "Candidate without parent must be accepted (C++ parity)");
}

#[test]
fn test_candidate_with_parent_slot_ge_rejected() {
    // C++ consensus.cpp:173: parent_slot >= candidate_slot → misbehavior
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let candidate = Candidate::new(
        crate::block::CandidateId {
            slot: SlotIndex::new(1),
            hash: UInt256::from([0xAA; 32]),
            block: BlockIdExt::default(),
        },
        Some(crate::block::CandidateId {
            slot: SlotIndex::new(1), // parent_slot == candidate_slot
            hash: UInt256::from([0xBB; 32]),
            block: BlockIdExt::default(),
        }),
        ValidatorIndex::new(0),
        Some(create_stub_block(BlockIdExt::default())),
        vec![],
    );

    let result = state.on_candidate(&desc, candidate);
    assert!(result.is_err(), "Candidate with parent_slot >= candidate_slot must be rejected");
}

#[test]
fn test_candidate_with_valid_parent_accepted() {
    // C++ consensus.cpp:173: parent_slot < candidate_slot → accepted
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let candidate = Candidate::new(
        crate::block::CandidateId {
            slot: SlotIndex::new(1),
            hash: UInt256::from([0xAA; 32]),
            block: BlockIdExt::default(),
        },
        Some(crate::block::CandidateId {
            slot: SlotIndex::new(0), // parent_slot < candidate_slot
            hash: UInt256::from([0xBB; 32]),
            block: BlockIdExt::default(),
        }),
        ValidatorIndex::new(0),
        Some(create_stub_block(BlockIdExt::default())),
        vec![],
    );

    let result = state.on_candidate(&desc, candidate);
    assert!(result.is_ok(), "Candidate with parent_slot < candidate_slot must be accepted");
}

#[test]
fn test_window_cleanup_after_finalization() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Ensure windows 0 and 1 exist
    state.ensure_window_exists(WindowIndex::new(0));
    state.ensure_window_exists(WindowIndex::new(1));
    assert_eq!(state.leader_windows.len(), 2);

    let block = BlockIdExt::default();

    // Finalize slot 0 first (slots must be finalized/skipped in order)
    // This will advance first_non_finalized_slot to 1
    let vote0 = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, Vec::new()).unwrap();

    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1));

    // Finalize slot 1 (3 finalize votes)
    // This will advance first_non_finalized_slot to 2
    let vote1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block.root_hash });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, Vec::new()).unwrap();

    // Clear events
    while state.pull_event().is_some() {}

    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(2));

    // Explicitly call cleanup (now called externally from SessionProcessor)
    // Clean up slots < 2 (i.e., slots 0 and 1)
    state.cleanup_slots(SlotIndex::new(2));

    // Window 0 should be cleaned up (slots 0..1 are both < 2)
    assert_eq!(state.leader_windows.len(), 1);
    assert_eq!(state.leader_window_offset, WindowIndex::new(1));
}

#[test]
fn test_duplicate_vote_same_block_ok() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: block.root_hash.clone(),
    });

    // First vote succeeds
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();

    // Second identical vote should succeed (duplicate, not error)
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote, Vec::new()).unwrap();

    // Weight should still be 1 (not double-counted)
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert_eq!(sv.notarize_weight_by_block.get(&block.root_hash).copied().unwrap(), 1);
}

#[test]
fn test_invalid_validator_idx() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block = BlockIdExt::default();
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block.root_hash });

    // validator_idx = 99 is out of bounds
    let result = state.on_vote_test(&desc, ValidatorIndex::new(99), vote, Vec::new());
    assert!(result.is_err(), "Invalid validator_idx should return error");
}

#[test]
fn test_invalid_leader_in_candidate() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Create candidate with invalid leader (construct directly to test FSM validation)
    let candidate = Candidate::new(
        crate::block::CandidateId {
            slot: SlotIndex::new(0),
            hash: UInt256::default(),
            block: BlockIdExt::default(),
        },
        None,
        ValidatorIndex::new(99), // Invalid leader index
        Some(create_stub_block(BlockIdExt::default())),
        vec![],
    );

    let result = state.on_candidate(&desc, candidate);
    assert!(result.is_err(), "Invalid leader should return error");
}

/*
    ========================================================================
    Timeout Tests
    ========================================================================
*/

#[test]
fn test_timeout_management() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // FSM is created with unarmed timeouts (skip_timestamp = None).
    // SessionProcessor::start() is responsible for calling reset_timeouts_on_start().
    assert!(state.get_next_timeout().is_none(), "FSM must start with unarmed timeouts");
    assert_eq!(state.skip_slot, SlotIndex::new(0));

    // Arm timeouts (simulating start())
    state.reset_timeouts_on_start(&desc);
    assert!(state.get_next_timeout().is_some(), "reset_timeouts_on_start must set skip_timestamp");
    assert_eq!(state.skip_slot, SlotIndex::new(0));
}

#[test]
fn test_unarmed_fsm_no_skip_cascade_after_delay() {
    // Regression: the FSM must NOT fire skip votes when check_all() runs
    // with unarmed timeouts, even after an arbitrary delay.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Simulate 60 s overlay warmup delay without arming timeouts
    let future = desc.get_time() + Duration::from_secs(60);
    desc.set_time(future);

    state.check_all(&desc);

    let mut skip_count = 0;
    while let Some(event) = state.pull_event() {
        if matches!(event, SimplexEvent::BroadcastVote(Vote::Skip(_))) {
            skip_count += 1;
        }
    }
    assert_eq!(skip_count, 0, "unarmed FSM must produce NO skip votes regardless of clock delay");
}

#[test]
fn test_armed_timeouts_enable_skip_after_expiry() {
    // After reset_timeouts_on_start() the skip timer fires once the deadline elapses.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let t0 = desc.get_time();

    // Arm at t0
    state.reset_timeouts_on_start(&desc);
    assert!(state.get_next_timeout().is_some());

    // Immediately after arming, check_all should produce no skips
    state.check_all(&desc);
    let mut skip_count = 0;
    while let Some(event) = state.pull_event() {
        if matches!(event, SimplexEvent::BroadcastVote(Vote::Skip(_))) {
            skip_count += 1;
        }
    }
    assert_eq!(skip_count, 0, "no skip votes before timeout expires");

    // Advance past first_block_timeout + target_rate (defaults: 3s + 1s = 4s)
    desc.set_time(t0 + Duration::from_secs(5));
    state.check_all(&desc);

    let mut skip_count = 0;
    while let Some(event) = state.pull_event() {
        if matches!(event, SimplexEvent::BroadcastVote(Vote::Skip(_))) {
            skip_count += 1;
        }
    }
    assert!(skip_count > 0, "skip votes must fire after timeout expires");
}

#[test]
fn test_try_skip_window_preserves_pending_block_cpp_mode() {
    // In C++ mode, alarm() only sets voted_skip=true and does NOT clear
    // pending_block.  The async try_notarize() coroutine can still complete
    // after a skip vote, producing both Skip and Notar for the same slot.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Store a candidate as pending at slot 0
    let hash = UInt256::from([1u8; 32]);
    let block_id = BlockIdExt::default();
    let candidate = create_test_candidate(0, hash.clone(), block_id, None, 0);
    let _ = state.on_candidate(&desc, candidate);
    // Drain events from on_candidate
    while state.pull_event().is_some() {}

    // Verify pending_block is set
    let pending_before = state
        .get_window(WindowIndex::new(0))
        .and_then(|w| w.slots[0].pending_block.as_ref())
        .is_some();

    // Fire skip for the entire window (simulates timeout -> try_skip_window)
    state.try_skip_window_for_test(WindowIndex::new(0));

    // voted_skip must be set
    let voted_skip =
        state.get_window(WindowIndex::new(0)).map(|w| w.slots[0].voted_skip).unwrap_or(false);
    assert!(voted_skip, "slot 0 must have voted_skip after try_skip_window");

    // In C++ mode, pending_block must be PRESERVED (not cleared)
    let pending_after = state
        .get_window(WindowIndex::new(0))
        .and_then(|w| w.slots[0].pending_block.as_ref())
        .is_some();
    assert_eq!(
        pending_before, pending_after,
        "C++ mode: pending_block must be preserved after skip (was={}, now={})",
        pending_before, pending_after
    );
}

#[test]
fn test_try_skip_window_clears_pending_block_alpenglow_mode() {
    // Alpenglow mode: pendingBlocks[k] <- bottom after skip
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create SimplexState");

    let hash = UInt256::from([1u8; 32]);
    let block_id = BlockIdExt::default();
    let candidate = create_test_candidate(0, hash.clone(), block_id, None, 0);
    let _ = state.on_candidate(&desc, candidate);
    while state.pull_event().is_some() {}

    state.try_skip_window_for_test(WindowIndex::new(0));

    let pending_after = state
        .get_window(WindowIndex::new(0))
        .and_then(|w| w.slots[0].pending_block.as_ref())
        .is_some();
    assert!(!pending_after, "Alpenglow mode: pending_block must be cleared after skip");
}

/*
    ========================================================================
    Available Parent Tests
    ========================================================================
*/

#[test]
fn test_has_available_parent_first_slot_with_genesis() {
    // First slot should have parent available when per-slot available_base is genesis (Some(None)).
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Slot 0 starts with genesis base (C++ SlotState::available_base = RawParentId{}).
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(0)),
        "First slot should have parent available (genesis)"
    );
}

#[test]
fn test_has_available_parent_first_slot_no_bases() {
    // If a slot's available_base is unknown, it should not have a parent.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Create window 1 (slot 2 is first in window 1). Base is unknown by default.
    state.ensure_window_exists(WindowIndex::new(1));

    assert!(
        !state.has_available_parent(&desc, SlotIndex::new(2)),
        "First slot in new window should NOT have parent (base unknown)"
    );
}

#[test]
fn test_has_available_parent_non_first_slot_no_prev_voted() {
    // Non-first slot without propagated available_base should not have parent.
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Slot 1 is second in window 0, but slot 0 hasn't progressed yet, so base is unknown.
    assert!(
        !state.has_available_parent(&desc, SlotIndex::new(1)),
        "Second slot should NOT have parent when base is unknown"
    );
}

#[test]
fn test_has_available_parent_non_first_slot_with_prev_notarized() {
    // Non-first slot has available parent when previous slot is notarized.
    // In C++ mode, parent must be notarized (reach threshold), not just voted.
    // Reference: C++ pool.cpp checks parent_slot->state->notarized.has_value()
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Submit a candidate for slot 0 to trigger voting
    let candidate_hash = UInt256::default();
    let candidate = create_test_candidate(
        0,
        candidate_hash.clone(),
        BlockIdExt::default(),
        None, // Genesis parent
        0,
    );
    state.on_candidate(&desc, candidate).unwrap();

    // Now slot 0 should have voted_notar set (we voted for it)
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[0].voted_notar.is_some(),
        "Slot 0 should have voted_notar after candidate"
    );

    // But slot 1 should NOT have parent yet (slot 0 not notarized - no threshold reached)
    assert!(
        !state.has_available_parent(&desc, SlotIndex::new(1)),
        "Second slot should NOT have parent before slot 0 reaches notarization threshold"
    );

    // Notarize slot 0 (reach 2/3 threshold)
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });
    for idx in 0..3 {
        state.on_vote_test(&desc, ValidatorIndex::new(idx), vote.clone(), Vec::new()).unwrap();
    }

    // Clear events
    while state.pull_event().is_some() {}

    // Now slot 0 is notarized, slot 1 should have parent available
    assert!(
        state.has_notarized_block(SlotIndex::new(0)),
        "Slot 0 should be notarized (observed_notar_certificate set)"
    );
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(1)),
        "Second slot should have parent when first slot is notarized"
    );
}

#[test]
fn test_get_available_parent_first_slot_returns_genesis() {
    // get_available_parent for first slot should return genesis (None) when base is genesis.
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Slot 0 has genesis base (available_base = Some(None)).
    let parent = state.get_available_parent(&desc, SlotIndex::new(0));
    assert!(parent.is_none(), "First slot parent should be None (genesis), got {:?}", parent);
}

#[test]
fn test_get_available_parent_window_start_after_skipped_last_slot_uses_previous_base() {
    // Base propagation across window boundary follows per-slot available_base (C++ pool.cpp):
    // - on notarization: next slot's available_base is set to notarized id
    // - on skip cert: base is propagated forward if next slot has no base yet
    //
    // Scenario (slots_per_window=2):
    // - slot 0 notarized (base for slot 1 becomes slot 0 id)
    // - slot 1 skipped (base propagates to slot 2, the first slot of window 1)
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Notarize slot 0 (reach 2/3 threshold)
    let parent_hash = UInt256::from_slice(&[0xAA; 32]);
    let notar_vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: parent_hash.clone() });
    for idx in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), notar_vote.clone(), Vec::new())
            .unwrap();
    }

    // Clear events
    while state.pull_event().is_some() {}

    // Skip slot 1 (reach 2/3 threshold), propagating base forward to slot 2.
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    for idx in 0..3 {
        state.on_vote_test(&desc, ValidatorIndex::new(idx), skip_vote.clone(), Vec::new()).unwrap();
    }

    // Clear events
    while state.pull_event().is_some() {}

    // Slot 2 is first slot of window 1. Its base should be inherited from slot 1,
    // which was set to slot 0 on notarization.
    let parent = state.get_available_parent(&desc, SlotIndex::new(2));
    assert!(parent.is_some(), "First slot of window 1 should have parent after base propagation");
    let parent_info = parent.unwrap();
    assert_eq!(parent_info.slot, SlotIndex::new(0), "Parent slot should be 0");
    assert_eq!(parent_info.hash, parent_hash, "Parent hash should match slot 0 notarized id");
}

#[test]
fn test_get_available_parent_non_first_slot_returns_notarized_block() {
    // Non-first slot should get notarized block from previous slot.
    // In C++ mode, parent must be notarized (reach threshold), not just voted.
    // Reference: C++ pool.cpp checks parent_slot->state->notarized.has_value()
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Submit a candidate for slot 0 to trigger voting
    // Note: voted_notar uses the candidate hash (id.hash), not the block's root_hash
    let candidate_hash = UInt256::from_slice(&[0xCC; 32]);
    let mut block_id = BlockIdExt::default();
    block_id.root_hash = UInt256::from_slice(&[0xBB; 32]);
    let candidate = create_test_candidate(
        0,
        candidate_hash.clone(),
        block_id,
        None, // Genesis parent
        0,
    );
    state.on_candidate(&desc, candidate).unwrap();

    // Get parent for slot 1 - should be None (slot 0 not notarized yet)
    let parent = state.get_available_parent(&desc, SlotIndex::new(1));
    assert!(
        parent.is_none(),
        "Second slot should NOT have parent before slot 0 reaches notarization threshold"
    );

    // Notarize slot 0 (reach 2/3 threshold)
    let vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });
    for idx in 0..3 {
        state.on_vote_test(&desc, ValidatorIndex::new(idx), vote.clone(), Vec::new()).unwrap();
    }

    // Clear events
    while state.pull_event().is_some() {}

    // Now slot 0 is notarized, get_available_parent should return the parent
    let parent = state.get_available_parent(&desc, SlotIndex::new(1));
    assert!(parent.is_some(), "Second slot should have parent from first slot's notarized block");
    let parent_info = parent.unwrap();
    assert_eq!(parent_info.slot, SlotIndex::new(0), "Parent slot should be 0");
    // Note: voted_notar stores the candidate hash, not the block's root_hash
    assert_eq!(parent_info.hash, candidate_hash, "Parent hash should match candidate hash");
}

#[test]
fn test_get_available_parent_non_first_slot_returns_none_when_not_voted() {
    // Non-first slot without voted_notar should return None
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Slot 1 is second in window 0, but slot 0 hasn't voted yet
    let parent = state.get_available_parent(&desc, SlotIndex::new(1));
    assert!(parent.is_none(), "Second slot should have no parent when first slot hasn't voted");
}

#[test]
fn test_get_available_parent_nonexistent_window() {
    // get_available_parent for a slot in non-existent window should return None
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Window 5 doesn't exist
    let parent = state.get_available_parent(&desc, SlotIndex::new(10));
    assert!(parent.is_none(), "Slot in non-existent window should have no parent");

    // has_available_parent should also return false
    assert!(
        !state.has_available_parent(&desc, SlotIndex::new(10)),
        "has_available_parent should be false for non-existent window"
    );
}

/*
    ========================================================================
    Late Candidate Tests
    ========================================================================

    Tests for scenarios where votes arrive before the block candidate.
*/

#[test]
fn test_late_candidate_with_notarize_votes_also_proceeds() {
    // More comprehensive test: Both notarize AND finalize votes arrive before candidate
    // This tests that the full voting pipeline works correctly

    let desc = create_test_desc(4, 2); // 4 validators, 2 slots per window
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let candidate_hash = UInt256::from_slice(&[0xCC; 32]);

    // Step 1: Send notarize votes first (reach threshold)
    let notarize_vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });

    state.on_vote_test(&desc, ValidatorIndex::new(0), notarize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), notarize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), notarize_vote, Vec::new()).unwrap();

    // This should trigger BlockNotarized internal event
    // Clear events from notarize phase
    let notarize_events: Vec<_> = from_fn(|| state.pull_event()).collect();

    // No BlockFinalized yet, just internal state updates
    assert!(
        !notarize_events.iter().any(|e| matches!(e, SimplexEvent::BlockFinalized(_))),
        "Should not finalize with only notarize votes"
    );

    // Step 2: Send finalize votes
    let finalize_vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });

    state.on_vote_test(&desc, ValidatorIndex::new(0), finalize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), finalize_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), finalize_vote, Vec::new()).unwrap();

    // Step 3: Verify finalization
    let finalize_events: Vec<_> = from_fn(|| state.pull_event()).collect();

    assert!(
        finalize_events.iter().any(|e| matches!(e, SimplexEvent::BlockFinalized(_))),
        "Should emit BlockFinalized after finalize threshold reached"
    );

    // Step 4: Verify state advanced
    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1), "Should advance to slot 1");

    // Step 5: Late candidate arrives
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        candidate_hash.clone(),
        UInt256::default(),
    );
    let late_candidate = create_test_candidate(0, candidate_hash.clone(), block_id, None, 0);

    state.on_candidate(&desc, late_candidate).unwrap();

    // No new events for finalized slot
    let late_events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(late_events.is_empty(), "No events for late candidate in finalized slot");

    // Step 6: Process next slot to verify progression continues
    // Finalize slot 1 as well to verify the chain continues
    let slot1_hash = UInt256::from_slice(&[0xDD; 32]);

    let finalize_slot1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: slot1_hash.clone() });

    state.on_vote_test(&desc, ValidatorIndex::new(0), finalize_slot1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), finalize_slot1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), finalize_slot1, Vec::new()).unwrap();

    let slot1_events: Vec<_> = from_fn(|| state.pull_event()).collect();

    assert!(
        slot1_events
            .iter()
            .any(|e| matches!(e, SimplexEvent::BlockFinalized(ev) if ev.slot == SlotIndex::new(1))),
        "Slot 1 should finalize normally after late candidate scenario"
    );

    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "Should advance to slot 2 after slot 1 finalization"
    );
}

/*
    ========================================================================
    Certificate Tests
    ========================================================================

    Tests for certificate creation and caching functionality (P2.3, U4.9).

    Reference: C++ pool.cpp check_and_publish_events, certificate.cpp
*/

#[test]
fn test_notarization_certificate_created_at_threshold() {
    // When notarization threshold (2/3) is reached, a notarization certificate
    // should be created and cached in slot_votes.notarize_certificates
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xAA; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Need 3 out of 4 for threshold_66
    // Submit with dummy signatures (Vec::new() for test)
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1, 2, 3]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![4, 5, 6]).unwrap();

    // After 2 votes, certificate should NOT exist yet
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(
        sv.notarize_certificate.is_none(),
        "Notarization certificate should not be created before threshold"
    );

    // 3rd vote triggers threshold
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![7, 8, 9]).unwrap();

    // Now certificate should exist
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    assert!(
        sv.notarize_certificate.is_some(),
        "Notarization certificate should be created at threshold"
    );

    // Check certificate has 3 signatures and is for the correct block
    let cert = sv.notarize_certificate.as_ref().unwrap();
    assert_eq!(&cert.vote.block_hash, &block_hash, "Certificate should be for the correct block");
    assert_eq!(cert.signatures.len(), 3, "Certificate should have 3 signatures");
}

#[test]
fn test_notarization_reached_event_emitted() {
    // When notarization threshold is reached, NotarizationReached event should be emitted
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xBB; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Clear any initial events
    while state.pull_event().is_some() {}

    // Submit votes with dummy signatures
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // Check for NotarizationReached event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    let notar_reached = events.iter().find_map(|e| {
        if let SimplexEvent::NotarizationReached(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(notar_reached.is_some(), "NotarizationReached event should be emitted");
    let event = notar_reached.unwrap();
    assert_eq!(event.slot, SlotIndex::new(0));
    assert_eq!(event.block_hash, block_hash);
    assert_eq!(event.certificate.signatures.len(), 3, "Certificate should have 3 signatures");
}

#[test]
fn test_finalization_certificate_in_block_finalized_event() {
    // BlockFinalizedEvent should contain a finalization certificate with signatures
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xCC; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Clear any initial events
    while state.pull_event().is_some() {}

    // Submit votes with dummy signatures
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![10, 11]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![20, 21]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![30, 31]).unwrap();

    // Check for BlockFinalized event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    let finalized = events.iter().find_map(|e| {
        if let SimplexEvent::BlockFinalized(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(finalized.is_some(), "BlockFinalized event should be emitted");
    let event = finalized.unwrap();
    assert_eq!(event.slot, SlotIndex::new(0));
    assert_eq!(event.block_hash, block_hash);

    // Check certificate
    assert_eq!(
        event.certificate.signatures.len(),
        3,
        "Finalization certificate should have 3 signatures"
    );

    // Verify signature validator indices
    let validator_indices: Vec<_> =
        event.certificate.signatures.iter().map(|s| s.validator_idx.value()).collect();
    assert!(validator_indices.contains(&0), "Should have signature from validator 0");
    assert!(validator_indices.contains(&1), "Should have signature from validator 1");
    assert!(validator_indices.contains(&2), "Should have signature from validator 2");
}

#[test]
fn test_finalization_reached_event_emitted() {
    // When finalization threshold is reached, FinalizationReached event should be emitted
    // (in addition to BlockFinalized)
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xEE; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Clear any initial events
    while state.pull_event().is_some() {}

    // Submit votes with dummy signatures
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![10, 11]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![20, 21]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![30, 31]).unwrap();

    // Check for events
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    // Should have FinalizationReached event
    let final_reached = events.iter().find_map(|e| {
        if let SimplexEvent::FinalizationReached(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(final_reached.is_some(), "FinalizationReached event should be emitted");
    let event = final_reached.unwrap();
    assert_eq!(event.slot, SlotIndex::new(0));
    assert_eq!(event.block_hash, block_hash);
    assert_eq!(event.certificate.signatures.len(), 3, "Certificate should have 3 signatures");

    // Should also have BlockFinalized event (emitted after FinalizationReached)
    let finalized = events.iter().find_map(|e| {
        if let SimplexEvent::BlockFinalized(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(finalized.is_some(), "BlockFinalized event should also be emitted");
    assert_eq!(finalized.unwrap().slot, SlotIndex::new(0));
}

#[test]
fn test_finalization_reached_event_emitted_only_once() {
    // FinalizationReached should only be emitted once per block, even if more votes arrive
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xFF; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Clear any initial events
    while state.pull_event().is_some() {}

    // Submit 3 votes (threshold reached)
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote.clone(), vec![3]).unwrap();

    // Collect events after threshold
    let events1: Vec<_> = from_fn(|| state.pull_event()).collect();
    let count1 =
        events1.iter().filter(|e| matches!(e, SimplexEvent::FinalizationReached(_))).count();
    assert_eq!(count1, 1, "Should have exactly one FinalizationReached event");

    // Submit 4th vote (after threshold already reached)
    state.on_vote_test(&desc, ValidatorIndex::new(3), vote, vec![4]).unwrap();

    // No new FinalizationReached events
    let events2: Vec<_> = from_fn(|| state.pull_event()).collect();
    let count2 =
        events2.iter().filter(|e| matches!(e, SimplexEvent::FinalizationReached(_))).count();
    assert_eq!(count2, 0, "No new FinalizationReached after threshold already reached");
}

#[test]
fn test_finalization_certificate_has_sufficient_weight() {
    // The finalization certificate should have weight >= threshold_66
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xDD; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Clear any initial events
    while state.pull_event().is_some() {}

    // Submit votes
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // Get BlockFinalized event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let finalized = events.iter().find_map(|e| {
        if let SimplexEvent::BlockFinalized(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    let event = finalized.expect("BlockFinalized event should be emitted");

    // Calculate total weight from certificate
    let total_weight: u64 =
        event.certificate.signatures.iter().map(|s| desc.get_node_weight(s.validator_idx)).sum();

    // threshold_66 for 4 validators with weight 1 each = 3
    assert!(
        total_weight >= desc.get_threshold_66(),
        "Certificate weight ({}) should be >= threshold_66 ({})",
        total_weight,
        desc.get_threshold_66()
    );
}

#[test]
fn test_certificate_signatures_match_voters() {
    // Each signature in the certificate should correspond to a validator who voted
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xEE; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Create distinct signatures for each validator
    let sig0 = vec![0xA0, 0xA1, 0xA2];
    let sig1 = vec![0xB0, 0xB1, 0xB2];
    let sig2 = vec![0xC0, 0xC1, 0xC2];

    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), sig0.clone()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), sig1.clone()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, sig2.clone()).unwrap();

    // Get BlockFinalized event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let event = events
        .iter()
        .find_map(|e| if let SimplexEvent::BlockFinalized(ev) = e { Some(ev) } else { None })
        .expect("BlockFinalized event should be emitted");

    // Verify each signature matches the validator's submitted signature
    for sig in &event.certificate.signatures {
        let expected_sig = match sig.validator_idx.value() {
            0 => &sig0,
            1 => &sig1,
            2 => &sig2,
            _ => panic!("Unexpected validator index"),
        };
        assert_eq!(
            &sig.signature,
            expected_sig,
            "Signature for validator {} should match submitted signature",
            sig.validator_idx.value()
        );
    }
}

#[test]
fn test_notarization_certificate_not_duplicated() {
    // Multiple votes for the same block from same validator should not create
    // duplicate signatures in the certificate
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0xFF; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Submit duplicate vote from validator 0
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap(); // Duplicate

    // Submit from other validators to reach threshold
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // Check certificate has exactly 3 signatures (not 4)
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();
    let cert = sv.notarize_certificate.as_ref().unwrap();
    assert_eq!(&cert.vote.block_hash, &block_hash, "Certificate should be for the correct block");
    assert_eq!(
        cert.signatures.len(),
        3,
        "Certificate should have 3 unique signatures, not duplicates"
    );

    // Verify no duplicate validator indices
    let validator_indices: Vec<_> =
        cert.signatures.iter().map(|s| s.validator_idx.value()).collect();
    let mut unique_indices = validator_indices.clone();
    unique_indices.sort();
    unique_indices.dedup();
    assert_eq!(
        validator_indices.len(),
        unique_indices.len(),
        "No duplicate validator indices in certificate"
    );
}

#[test]
fn test_multiple_blocks_votes_tracked_separately() {
    // Votes for different blocks in the same slot should be tracked separately.
    // Note: In BFT consensus, a validator can only vote for ONE block per slot.
    // Voting for two different blocks is misbehavior. This test verifies that
    // votes from different validators for different blocks are tracked correctly.
    //
    // With 7 validators (weight 1 each):
    //   threshold_66 = (7 * 2) / 3 + 1 = 5  (strict > 2/3, matches C++)
    let desc = create_test_desc(7, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash_a = UInt256::from_slice(&[0xAA; 32]);
    let block_hash_b = UInt256::from_slice(&[0xBB; 32]);

    let vote_a =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash_a.clone() });
    let vote_b =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash_b.clone() });

    // Vote for block A from validators 0..5 (6 votes - reaches threshold at 5)
    // Certificate is created when the 5th vote is received (threshold = 5 reached)
    // The 6th vote comes after certificate is already cached
    for i in 0..6 {
        state.on_vote_test(&desc, ValidatorIndex::new(i), vote_a.clone(), vec![i as u8]).unwrap();
    }

    // Vote for block B from validator 6 only (1 vote - doesn't reach threshold)
    // Validators 0-5 already voted for block A, so only validator 6 is available
    state.on_vote_test(&desc, ValidatorIndex::new(6), vote_b.clone(), vec![0x55]).unwrap();

    // Clear events
    while state.pull_event().is_some() {}

    // Check that block A has a certificate (reached threshold first)
    let sv = state.slot_votes.get(&SlotIndex::new(0)).unwrap();

    assert!(sv.notarize_certificate.is_some(), "Slot should have a notarization certificate");
    let cert = sv.notarize_certificate.as_ref().unwrap();
    assert_eq!(
        &cert.vote.block_hash, &block_hash_a,
        "Certificate should be for block A (reached threshold first)"
    );

    // Block A certificate should have 5 signatures (captured at threshold)
    // Note: threshold_66(7) = 5, so certificate is created after 5th vote
    assert_eq!(cert.signatures.len(), 5, "Certificate A should have 5 signatures (threshold)");

    // Verify the notarize_weight tracks both blocks separately (all votes counted)
    assert_eq!(sv.notarize_weight_by_block.get(&block_hash_a), Some(&6_u64));
    assert_eq!(sv.notarize_weight_by_block.get(&block_hash_b), Some(&1_u64));
}

#[test]
fn test_certificate_stores_vote_type() {
    // The certificate should store the correct vote type (Notarize vs Finalize)
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0x11; 32]);
    let vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // Get BlockFinalized event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let event = events
        .iter()
        .find_map(|e| if let SimplexEvent::BlockFinalized(ev) = e { Some(ev) } else { None })
        .expect("BlockFinalized event should be emitted");

    // The certificate vote should be a FinalizeVote
    assert!(
        matches!(event.certificate.vote, FinalizeVote { .. }),
        "Finalization certificate should contain FinalizeVote"
    );
    assert_eq!(event.certificate.vote.slot, SlotIndex::new(0));
    assert_eq!(event.certificate.vote.block_hash, block_hash);
}

#[test]
fn test_notarization_certificate_vote_type() {
    // Notarization certificate should contain NotarizeVote
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0x22; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    while state.pull_event().is_some() {}

    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // Get NotarizationReached event
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let event = events
        .iter()
        .find_map(|e| if let SimplexEvent::NotarizationReached(ev) = e { Some(ev) } else { None })
        .expect("NotarizationReached event should be emitted");

    // The certificate vote should be a NotarizeVote
    assert!(
        matches!(event.certificate.vote, NotarizeVote { .. }),
        "Notarization certificate should contain NotarizeVote"
    );
    assert_eq!(event.certificate.vote.slot, SlotIndex::new(0));
    assert_eq!(event.certificate.vote.block_hash, block_hash);
}

#[test]
fn test_certificate_get_notarize_certificate() {
    // Test the get_notarize_certificate public API
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let block_hash = UInt256::from_slice(&[0x33; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });

    // Before threshold, should return None
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    assert!(
        state.get_notarize_certificate(SlotIndex::new(0), &block_hash).is_none(),
        "Should return None before threshold"
    );

    // Reach threshold
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // After threshold, should return the certificate
    let cert = state.get_notarize_certificate(SlotIndex::new(0), &block_hash);
    assert!(cert.is_some(), "Should return certificate after threshold");
    assert_eq!(cert.unwrap().signatures.len(), 3);

    // Non-existent block should return None
    let other_hash = UInt256::from_slice(&[0x44; 32]);
    assert!(
        state.get_notarize_certificate(SlotIndex::new(0), &other_hash).is_none(),
        "Should return None for non-existent block"
    );

    // Non-existent slot should return None
    assert!(
        state.get_notarize_certificate(SlotIndex::new(5), &block_hash).is_none(),
        "Should return None for non-existent slot"
    );
}

#[test]
fn test_skip_certificate_created_at_threshold() {
    // When skip threshold (2/3) is reached, internal skip_weight should be tracked
    // Note: Skip certificates are implicit in the FSM (via SlotSkipped event)
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });

    // Need 3 out of 4 for threshold_66
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();

    // After 2 votes, skip_or_skip_fallback_weight = 2, which is < 3 (threshold_66)
    // Clear events
    while state.pull_event().is_some() {}

    // 3rd vote triggers skip certificate threshold
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();

    // SlotSkipped should be emitted
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimplexEvent::SlotSkipped(ev) if ev.slot == SlotIndex::new(0))),
        "SlotSkipped event should be emitted when skip threshold reached"
    );

    // C++ parity: skip does NOT advance first_non_finalized_slot (only finalization does).
    // But first_non_progressed_slot (C++ `now_`) does advance on skip.
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "first_non_finalized_slot should NOT advance on skip (C++ parity)"
    );
    assert_eq!(
        state.first_non_progressed_slot,
        SlotIndex::new(1),
        "first_non_progressed_slot should advance after skip certificate"
    );
}

/*
    ========================================================================
    set_notarize_certificate Tests (U4.5a)

    Tests for setting notarization certificate from external source (query response).
    ========================================================================
*/

#[test]
fn test_set_notarize_certificate_updates_vote_accounting() {
    // When setting a certificate from external source, vote accounting should be updated
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from_slice(&[0xAA; 32]);

    // Create a certificate with 3 validators (>2/3 threshold)
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![0x10, 0x11]),
        VoteSignature::new(ValidatorIndex::new(1), vec![0x20, 0x21]),
        VoteSignature::new(ValidatorIndex::new(2), vec![0x30, 0x31]),
    ];
    let cert: NotarCertPtr = Arc::new(Certificate::new(vote, signatures));

    // Before setting: should have no notarize weight
    let notar_weight_before = state.get_notarize_weight(slot, &block_hash);
    assert_eq!(notar_weight_before, 0, "Should have no notarize weight before setting certificate");

    // Set the certificate
    let stored = state
        .set_notarize_certificate(&desc, slot, &block_hash, cert.clone())
        .expect("should not conflict");
    assert!(stored, "certificate should be stored");

    // After setting: should have notarize weight from 3 validators (3 weight)
    // Each validator has weight 1 in create_test_desc(4, 1)
    let notar_weight_after = state.get_notarize_weight(slot, &block_hash);
    assert_eq!(notar_weight_after, 3, "Should have 3 weight from 3 validators");
}

#[test]
fn test_set_notarize_certificate_idempotent() {
    // Calling set_notarize_certificate multiple times should not increase vote weight
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from_slice(&[0xBB; 32]);

    // Create a certificate with 3 validators
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![0x10]),
        VoteSignature::new(ValidatorIndex::new(1), vec![0x20]),
        VoteSignature::new(ValidatorIndex::new(2), vec![0x30]),
    ];
    let cert: NotarCertPtr = Arc::new(Certificate::new(vote, signatures));

    // Set the certificate twice
    let stored1 = state
        .set_notarize_certificate(&desc, slot, &block_hash, cert.clone())
        .expect("should not conflict");
    let weight_after_first = state.get_notarize_weight(slot, &block_hash);
    // Drain first-store events so we can assert duplicate store emits none.
    while state.pull_event().is_some() {}

    let stored2 = state
        .set_notarize_certificate(&desc, slot, &block_hash, cert.clone())
        .expect("should not conflict");
    let weight_after_second = state.get_notarize_weight(slot, &block_hash);

    assert!(stored1, "first application should store");
    assert!(!stored2, "second application should be deduplicated");

    // Weight should be the same after both calls
    assert_eq!(
        weight_after_first, weight_after_second,
        "Weight should not change on second call (idempotent)"
    );
    assert_eq!(weight_after_first, 3, "Weight should be 3");
    assert!(
        !state.has_pending_events(),
        "duplicate notar cert must not emit relay-triggering events"
    );
}

#[test]
fn test_set_notarize_certificate_does_not_overwrite_existing() {
    // If there's already a certificate (from local votes), set should not overwrite
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from_slice(&[0xCC; 32]);

    // First, vote normally to create a certificate from local votes
    let vote = Vote::Notarize(NotarizeVote { slot, block_hash: block_hash.clone() });

    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![0xA0]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![0xB0]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![0xC0]).unwrap();

    // Get the locally-created certificate
    let local_cert = state.get_notarize_certificate(slot, &block_hash);
    assert!(local_cert.is_some(), "Should have certificate from local votes");
    let local_cert = local_cert.unwrap();
    let local_sig_count = local_cert.signatures.len();

    // Now try to set a different certificate (only 2 validators)
    let external_vote = NotarizeVote { slot, block_hash: block_hash.clone() };
    let external_signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![0xFF, 0xEE]),
        VoteSignature::new(ValidatorIndex::new(3), vec![0xDD, 0xCC]),
    ];
    let external_cert: NotarCertPtr =
        Arc::new(Certificate::new(external_vote, external_signatures));

    // Set should not overwrite
    let stored = state
        .set_notarize_certificate(&desc, slot, &block_hash, external_cert)
        .expect("should not conflict");
    assert!(!stored, "should not overwrite existing certificate (idempotent)");

    // Certificate should still be the original one
    let cert_after = state.get_notarize_certificate(slot, &block_hash).unwrap();
    assert_eq!(
        cert_after.signatures.len(),
        local_sig_count,
        "Certificate should not be overwritten"
    );
}

#[test]
fn test_set_notarize_certificate_sets_notarized_flag() {
    // Setting a certificate should set the block_notarized_published flag
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from_slice(&[0xDD; 32]);

    // Before setting: has_notarized_block should be false
    assert!(
        !state.has_notarized_block(slot),
        "Should not have notarized block before setting certificate"
    );

    // Create and set a certificate
    let vote = NotarizeVote { slot, block_hash: block_hash.clone() };
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![0x10]),
        VoteSignature::new(ValidatorIndex::new(1), vec![0x20]),
        VoteSignature::new(ValidatorIndex::new(2), vec![0x30]),
    ];
    let cert: NotarCertPtr = Arc::new(Certificate::new(vote, signatures));

    let stored = state
        .set_notarize_certificate(&desc, slot, &block_hash, cert)
        .expect("should not conflict");
    assert!(stored, "certificate should be stored");

    // After setting: has_notarized_block should be true
    assert!(
        state.has_notarized_block(slot),
        "Should have notarized block after setting certificate"
    );

    // The certificate should be retrievable
    let retrieved = state.get_notarize_certificate(slot, &block_hash);
    assert!(retrieved.is_some(), "Certificate should be retrievable");
    assert_eq!(retrieved.unwrap().signatures.len(), 3, "Should have 3 signatures");
}

#[test]
fn test_set_notarize_certificate_emits_notarization_reached_for_tracked_slot() {
    // External notar cert ingestion must emit NotarizationReached so SessionProcessor can
    // persist + cache + relay the cert (same observable behavior as threshold-driven path).
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Clear any initial events
    while state.pull_event().is_some() {}

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from([0xAB; 32]);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let cert = create_test_notar_cert(&desc, slot, block_hash.clone(), &signers);

    let stored = state
        .set_notarize_certificate(&desc, slot, &block_hash, cert.clone())
        .expect("should not conflict");
    assert!(stored, "certificate should be stored");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let notar_reached = events.iter().find_map(|e| {
        if let SimplexEvent::NotarizationReached(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(notar_reached.is_some(), "Expected NotarizationReached event, got {:?}", events);
    let ev = notar_reached.unwrap();
    assert_eq!(ev.slot, slot);
    assert_eq!(ev.block_hash, block_hash);
    assert!(Arc::ptr_eq(&ev.certificate, &cert), "Event should carry the stored cert");
}

#[test]
fn test_set_notarize_certificate_does_not_emit_notarization_reached_for_old_slot() {
    // For slots already finalized (slot < first_non_finalized_slot), SimplexState stores the cert
    // for restart/recommit support but must not emit NotarizationReached.
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Mark slot 0 as already finalized/old
    state.set_first_non_finalized_slot_for_test(SlotIndex::new(1));

    // Clear any initial events
    while state.pull_event().is_some() {}

    let slot0 = SlotIndex::new(0);
    let block_hash = UInt256::from([0xCD; 32]);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let cert = create_test_notar_cert(&desc, slot0, block_hash.clone(), &signers);

    let stored = state
        .set_notarize_certificate(&desc, slot0, &block_hash, cert)
        .expect("should not conflict");
    assert!(stored, "certificate should be stored even for old slots");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::NotarizationReached(_))),
        "NotarizationReached must not be emitted for old slots, got {:?}",
        events
    );
}

#[test]
fn test_set_notarize_certificate_propagates_available_base_to_next_slot() {
    // When a notarization certificate is received via query/repair path, we must
    // update per-slot `available_base` and advance the progress cursor, matching
    // C++ pool.cpp behavior.
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot0 = SlotIndex::new(0);
    let slot1 = SlotIndex::new(1);
    let block_hash = UInt256::from_slice(&[0xEE; 32]);

    // Create and set a certificate (3 validators > 2/3 threshold)
    let vote = NotarizeVote { slot: slot0, block_hash: block_hash.clone() };
    let signatures = vec![
        VoteSignature::new(ValidatorIndex::new(0), vec![0x10]),
        VoteSignature::new(ValidatorIndex::new(1), vec![0x20]),
        VoteSignature::new(ValidatorIndex::new(2), vec![0x30]),
    ];
    let cert: NotarCertPtr = Arc::new(Certificate::new(vote, signatures));

    let stored = state
        .set_notarize_certificate(&desc, slot0, &block_hash, cert)
        .expect("should not conflict");
    assert!(stored, "certificate should be stored");

    // Progress cursor should advance past the notarized slot.
    assert_eq!(
        state.get_first_non_progressed_slot(),
        slot1,
        "Progress cursor should advance after notarization certificate is applied"
    );

    // Next slot should have parent available via propagated available_base.
    assert!(
        state.has_available_parent(&desc, slot1),
        "Slot 1 should have parent available after applying notar cert for slot 0"
    );
    let parent =
        state.get_available_parent(&desc, slot1).expect("Slot 1 parent should be available");
    assert_eq!(parent.slot, slot0, "Parent slot should be 0");
    assert_eq!(parent.hash, block_hash, "Parent hash should match slot 0 notarized id");
}

#[test]
fn test_set_notarize_certificate_conflict_different_block() {
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let slot = SlotIndex::new(0);
    let hash_a = UInt256::from_slice(&[0xA1; 32]);
    let hash_b = UInt256::from_slice(&[0xB2; 32]);

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let cert_a = create_test_notar_cert(&desc, slot, hash_a.clone(), &signers);
    let cert_b = create_test_notar_cert(&desc, slot, hash_b.clone(), &signers);

    state.set_notarize_certificate(&desc, slot, &hash_a, cert_a).expect("should not conflict");

    let err = state
        .set_notarize_certificate(&desc, slot, &hash_b, cert_b)
        .expect_err("expected conflict when setting notar cert for another block");

    match err {
        CertificateStoreError::ConflictingBlock { existing_block, new_block } => {
            assert_eq!(existing_block, hash_a);
            assert_eq!(new_block, hash_b);
        }
    }
}

/*
    ========================================================================
    Parent Validation Mode Tests
    ========================================================================

    These tests verify the require_finalized_parent option behavior.

    - require_finalized_parent=false (C++ mode, default):
      Parent can be notarized OR finalized to build child block.
      This prevents deadlock when some validators vote skip while others vote finalize.

    - require_finalized_parent=true (strict mode):
      Parent must be finalized before child block can be generated.
      WARNING: Can cause deadlock in certain scenarios.
*/

/// Helper to create options with C++ mode (notarized parent OK)
fn opts_cpp_parent_mode() -> SimplexStateOptions {
    SimplexStateOptions { require_finalized_parent: false, ..opts_cpp() }
}

/// Helper to create options with strict mode (finalized parent required)
fn opts_strict_parent_mode() -> SimplexStateOptions {
    SimplexStateOptions { require_finalized_parent: true, ..opts_cpp() }
}

#[test]
fn test_cpp_mode_notarized_parent_valid() {
    // Test that with require_finalized_parent=false (C++ mode), a notarized block
    // is a valid parent for the next slot, even if not finalized.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Notarize slot 0 (but don't finalize)
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Drain events
    while state.pull_event().is_some() {}

    // Slot 0 is notarized but NOT finalized
    assert!(state.has_notarized_block(SlotIndex::new(0)), "Slot 0 should be notarized");
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "first_non_finalized_slot should still be 0 (not finalized)"
    );

    // In C++ mode, slot 0 should be valid parent for slot 1
    assert!(
        state.is_parent_valid(SlotIndex::new(0)),
        "C++ mode: notarized slot should be valid parent"
    );
}

#[test]
fn test_strict_mode_notarized_parent_invalid() {
    // Test that with require_finalized_parent=true (strict mode), a notarized block
    // is NOT a valid parent until it is finalized.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_strict_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Notarize slot 0 (but don't finalize)
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

    // Drain events
    while state.pull_event().is_some() {}

    // Slot 0 is notarized but NOT finalized
    assert!(state.has_notarized_block(SlotIndex::new(0)), "Slot 0 should be notarized");

    // In strict mode, slot 0 should NOT be valid parent (not finalized)
    assert!(
        !state.is_parent_valid(SlotIndex::new(0)),
        "Strict mode: notarized-only slot should NOT be valid parent"
    );
}

#[test]
fn test_finalized_parent_valid_in_both_modes() {
    // Test that a finalized slot is a valid parent in both modes
    let desc = create_test_desc(4, 2);
    let block_hash = UInt256::default();

    for (mode_name, opts) in
        [("C++ mode", opts_cpp_parent_mode()), ("strict mode", opts_strict_parent_mode())]
    {
        let mut state = SimplexState::new(&desc, opts).expect("Failed to create SimplexState");

        // Finalize slot 0
        let vote = Vote::Finalize(FinalizeVote {
            slot: SlotIndex::new(0),
            block_hash: block_hash.clone(),
        });
        state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), Vec::new()).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), Vec::new()).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(2), vote, Vec::new()).unwrap();

        // Drain events
        while state.pull_event().is_some() {}

        // first_non_finalized_slot should have advanced past slot 0
        assert_eq!(
            state.first_non_finalized_slot,
            SlotIndex::new(1),
            "{}: first_non_finalized_slot should be 1 after finalization",
            mode_name
        );

        // Finalized slot should be valid parent in both modes
        assert!(
            state.is_parent_valid(SlotIndex::new(0)),
            "{}: finalized slot should be valid parent",
            mode_name
        );
    }
}

#[test]
fn test_events_emitted_when_threshold_reached() {
    // Test that BlockFinalized/SlotSkipped events are emitted immediately when
    // threshold is reached, regardless of slot order (no sequential gating).
    // This matches C++ behavior where events are emitted as thresholds are reached.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Finalize slot 1 first (before slot 0)
    let vote1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, Vec::new()).unwrap();

    // Collect events - slot 1 should be finalized immediately
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();
    assert_eq!(
        finalized_slots,
        vec![SlotIndex::new(1)],
        "BlockFinalized(1) should be emitted immediately when threshold reached"
    );

    // first_non_finalized_slot should have advanced past slot 1
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "first_non_finalized_slot should advance to 2"
    );
}

#[test]
fn test_skip_events_emitted_when_threshold_reached() {
    // Test that SlotSkipped events are emitted immediately when
    // threshold is reached, regardless of slot order.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Skip slot 1 first (before slot 0)
    let vote1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, Vec::new()).unwrap();

    // Collect events - slot 1 should be skipped immediately
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let skipped_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::SlotSkipped(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();
    assert_eq!(
        skipped_slots,
        vec![SlotIndex::new(1)],
        "SlotSkipped(1) should be emitted immediately when threshold reached"
    );

    // C++ parity: first_non_finalized_slot does NOT advance on skip.
    // It stays at 0 since nothing was finalized.
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "first_non_finalized_slot should NOT advance on skip (C++ parity)"
    );
}

/*
    ========================================================================
    Deadlock Detection Tests
    ========================================================================

    These tests verify that C++ mode (require_finalized_parent=false) prevents
    deadlock in scenarios where some validators vote skip while others vote finalize.

    Deadlock scenario:
    - 5 validators, threshold is 4 (80%)
    - Slot 0: 4 validators notarize, 2 skip, 3 finalize
    - With strict mode: parent must be finalized, but only 3/5 finalize votes = no finalization
    - Slot 1 cannot start because parent (slot 0) is not finalized = DEADLOCK
    - With C++ mode: slot 0 is notarized, so it's valid parent for slot 1 = NO DEADLOCK
*/

#[test]
fn test_deadlock_scenario_with_strict_mode() {
    // Test that strict mode (require_finalized_parent=true) can deadlock
    // when finalization is blocked by skip votes.
    let desc = create_test_desc_weights(5, 2, vec![1, 1, 1, 1, 1]); // 5 validators, threshold=4
    let mut state =
        SimplexState::new(&desc, opts_strict_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Slot 0: 4 validators notarize (threshold reached)
    let notar_vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    for idx in 0..4 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), notar_vote.clone(), Vec::new())
            .unwrap();
    }

    // Slot 0: 2 validators vote skip
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(3), skip_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(4), skip_vote, Vec::new()).unwrap();

    // Slot 0: 3 validators vote finalize (NOT enough for threshold=4)
    let finalize_vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    for idx in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), finalize_vote.clone(), Vec::new())
            .unwrap();
    }

    // Drain events
    while state.pull_event().is_some() {}

    // Slot 0 is notarized but NOT finalized (only 3/5 finalize votes, need 4)
    assert!(state.has_notarized_block(SlotIndex::new(0)), "Slot 0 should be notarized");
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "Slot 0 should NOT be finalized (only 3/5 votes)"
    );

    // In strict mode: slot 0 is NOT valid parent (not finalized)
    // This would cause DEADLOCK - cannot proceed to slot 1
    assert!(
        !state.is_parent_valid(SlotIndex::new(0)),
        "Strict mode: notarized-but-not-finalized slot should NOT be valid parent (DEADLOCK)"
    );
}

#[test]
fn test_no_deadlock_in_cpp_mode() {
    // Test that C++ mode (require_finalized_parent=false) prevents deadlock
    // by allowing notarized blocks as parents.
    let desc = create_test_desc_weights(5, 2, vec![1, 1, 1, 1, 1]); // 5 validators, threshold=4
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Slot 0: 4 validators notarize (threshold reached)
    let notar_vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    for idx in 0..4 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), notar_vote.clone(), Vec::new())
            .unwrap();
    }

    // Slot 0: 2 validators vote skip
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(3), skip_vote.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(4), skip_vote, Vec::new()).unwrap();

    // Slot 0: 3 validators vote finalize (NOT enough for threshold=4)
    let finalize_vote =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    for idx in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), finalize_vote.clone(), Vec::new())
            .unwrap();
    }

    // Drain events
    while state.pull_event().is_some() {}

    // Slot 0 is notarized but NOT finalized (only 3/5 finalize votes, need 4)
    assert!(state.has_notarized_block(SlotIndex::new(0)), "Slot 0 should be notarized");
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(0),
        "Slot 0 should NOT be finalized (only 3/5 votes)"
    );

    // In C++ mode: slot 0 IS valid parent (notarized is enough)
    // This prevents DEADLOCK - can proceed to slot 1
    assert!(
        state.is_parent_valid(SlotIndex::new(0)),
        "C++ mode: notarized slot should be valid parent (NO DEADLOCK)"
    );
}

#[test]
fn test_is_parent_valid_with_notarization() {
    // Test is_parent_valid with notarization - C++ mode allows notarized parent
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Slot 0 is neither notarized nor finalized
    assert!(
        !state.is_parent_valid(SlotIndex::new(0)),
        "Slot 0 should NOT be valid parent initially"
    );

    // Notarize slot 0 (but don't finalize)
    let notar_vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    for idx in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(idx), notar_vote.clone(), Vec::new())
            .unwrap();
    }

    // Drain events
    while state.pull_event().is_some() {}

    // Slot 0 is now notarized - in C++ mode, it should be valid parent
    assert!(state.has_notarized_block(SlotIndex::new(0)), "Slot 0 should be notarized");
    assert!(
        state.is_parent_valid(SlotIndex::new(0)),
        "C++ mode: notarized slot should be valid parent"
    );
}

#[test]
fn test_out_of_order_finalization_abandons_earlier_slots() {
    // Test that when a later slot is finalized before an earlier slot,
    // the earlier slot is effectively "abandoned" (no events emitted for it).
    // This matches C++ behavior: first_nonfinalized_slot_ = id.slot + 1
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Finalize slot 1 first - should emit immediately
    let vote1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, Vec::new()).unwrap();

    // Slot 1 should be finalized immediately
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();
    assert_eq!(finalized_slots, vec![SlotIndex::new(1)], "Slot 1 should be finalized immediately");

    // first_non_finalized_slot advances past slot 1
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "first_non_finalized_slot should be 2 after slot 1 finalization"
    );

    // Now try to skip slot 0 - should return SlotAlreadyFinalized (slot 0 is now "in the past")
    let vote0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    let r0 = state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), Vec::new());
    let r1 = state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), Vec::new());
    let r2 = state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, Vec::new());
    assert!(
        matches!(r0, VoteResult::SlotAlreadyFinalized)
            && matches!(r1, VoteResult::SlotAlreadyFinalized)
            && matches!(r2, VoteResult::SlotAlreadyFinalized),
        "Votes for finalized slot 0 should return SlotAlreadyFinalized"
    );

    // Collect events - slot 0 skip should NOT be emitted (already past)
    events.clear();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let skipped_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::SlotSkipped(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();
    assert!(skipped_slots.is_empty(), "Slot 0 skip should NOT be emitted (slot is in the past)");

    // first_non_finalized_slot remains at 2
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "first_non_finalized_slot should remain 2"
    );
}

#[test]
fn test_sequential_finalization_order() {
    // Test that when slots are finalized in order (0, then 1),
    // both events are emitted correctly.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash = UInt256::default();

    // Finalize slot 0 first
    let vote0 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, Vec::new()).unwrap();

    // Drain events
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(1),
        "first_non_finalized_slot should be 1 after slot 0 finalization"
    );

    // Then finalize slot 1
    let vote1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, Vec::new()).unwrap();

    // Drain events
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();
    assert_eq!(
        finalized_slots,
        vec![SlotIndex::new(0), SlotIndex::new(1)],
        "Both slots should be finalized in order"
    );

    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "first_non_finalized_slot should be 2 after both slots finalized"
    );
}

/*
    ========================================================================
    V4.11: Batch Finalization Tests
    ========================================================================
*/

#[test]
fn test_batch_finalization_later_slot_finalized_first() {
    // Test scenario: Slot 2 is finalized before slot 1, both using slot 0 as parent chain
    // This tests the batch finalization behavior where finalizing a later slot
    // should trigger finalization of its parent chain.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash_0 = UInt256::from([0u8; 32]);
    let block_hash_1 = UInt256::from([1u8; 32]);
    let block_hash_2 = UInt256::from([2u8; 32]);

    // Step 1: Submit candidate for slot 0, notarize it
    let candidate_0 =
        create_test_candidate(0, block_hash_0.clone(), BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate_0).unwrap();

    // Notarize slot 0
    let notar_vote_0 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash_0.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_0.clone(), Vec::new())
            .unwrap();
    }
    drain_events(&mut state);

    // Step 2: Submit candidate for slot 1, notarize it (parent = slot 0)
    let candidate_1 = create_test_candidate(
        1,
        block_hash_1.clone(),
        BlockIdExt::default(),
        Some((0, block_hash_0.clone())),
        1,
    );
    state.on_candidate(&desc, candidate_1).unwrap();

    let notar_vote_1 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: block_hash_1.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_1.clone(), Vec::new())
            .unwrap();
    }
    drain_events(&mut state);

    // Step 3: Submit candidate for slot 2, notarize it (parent = slot 1)
    let candidate_2 = create_test_candidate(
        2,
        block_hash_2.clone(),
        BlockIdExt::default(),
        Some((1, block_hash_1.clone())),
        2,
    );
    state.on_candidate(&desc, candidate_2).unwrap();

    let notar_vote_2 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(2), block_hash: block_hash_2.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_2.clone(), Vec::new())
            .unwrap();
    }
    drain_events(&mut state);

    // Now finalize slot 2 directly (without finalizing slot 0 or 1 first)
    let finalize_vote_2 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(2), block_hash: block_hash_2.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), finalize_vote_2.clone(), Vec::new())
            .unwrap();
    }

    // Collect events
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    // Only slot 2's BlockFinalized event should be emitted
    // (Slots 0 and 1 are not finalized because we didn't send finalize votes for them)
    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();

    assert_eq!(
        finalized_slots,
        vec![SlotIndex::new(2)],
        "Only slot 2 should have BlockFinalized event (slots 0,1 were notarized but not finalized)"
    );

    // first_non_finalized_slot should advance to 3
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(3),
        "first_non_finalized_slot should be 3 after slot 2 finalization"
    );
}

#[test]
fn test_batch_finalization_multiple_slots_finalized_together() {
    // Test scenario: Finalize votes for slots 0, 1, 2 arrive in rapid succession
    // Each should trigger its own BlockFinalized event
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash_0 = UInt256::from([0u8; 32]);
    let block_hash_1 = UInt256::from([1u8; 32]);
    let block_hash_2 = UInt256::from([2u8; 32]);

    // Setup: Notarize all three slots first
    // Slot 0
    let candidate_0 =
        create_test_candidate(0, block_hash_0.clone(), BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate_0).unwrap();
    let notar_vote_0 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash_0.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_0.clone(), Vec::new())
            .unwrap();
    }

    // Slot 1
    let candidate_1 = create_test_candidate(
        1,
        block_hash_1.clone(),
        BlockIdExt::default(),
        Some((0, block_hash_0.clone())),
        1,
    );
    state.on_candidate(&desc, candidate_1).unwrap();
    let notar_vote_1 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: block_hash_1.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_1.clone(), Vec::new())
            .unwrap();
    }

    // Slot 2
    let candidate_2 = create_test_candidate(
        2,
        block_hash_2.clone(),
        BlockIdExt::default(),
        Some((1, block_hash_1.clone())),
        2,
    );
    state.on_candidate(&desc, candidate_2).unwrap();
    let notar_vote_2 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(2), block_hash: block_hash_2.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_2.clone(), Vec::new())
            .unwrap();
    }

    // Clear all notarize events
    drain_events(&mut state);

    // Now send finalize votes for all three slots (in order)
    let finalize_vote_0 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: block_hash_0.clone() });
    let finalize_vote_1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block_hash_1.clone() });
    let finalize_vote_2 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(2), block_hash: block_hash_2.clone() });

    // Finalize all three
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), finalize_vote_0.clone(), Vec::new())
            .unwrap();
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), finalize_vote_1.clone(), Vec::new())
            .unwrap();
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), finalize_vote_2.clone(), Vec::new())
            .unwrap();
    }

    // Collect events
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();

    // All three slots should be finalized
    assert!(finalized_slots.contains(&SlotIndex::new(0)), "Slot 0 should be finalized");
    assert!(finalized_slots.contains(&SlotIndex::new(1)), "Slot 1 should be finalized");
    assert!(finalized_slots.contains(&SlotIndex::new(2)), "Slot 2 should be finalized");

    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(3),
        "first_non_finalized_slot should be 3"
    );
}

#[test]
fn test_notarized_parent_enables_child_finalization() {
    // Test that in C++ mode (require_finalized_parent=false), a notarized parent
    // is sufficient for a child block to proceed to finalization
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_cpp_parent_mode()).expect("Failed to create SimplexState");

    let block_hash_0 = UInt256::from([0u8; 32]);
    let block_hash_1 = UInt256::from([1u8; 32]);

    // Setup slot 0: Submit and notarize (but NOT finalize)
    let candidate_0 =
        create_test_candidate(0, block_hash_0.clone(), BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate_0).unwrap();

    let notar_vote_0 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash_0.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_0.clone(), Vec::new())
            .unwrap();
    }
    drain_events(&mut state);

    // Verify slot 0 is notarized (valid parent in C++ mode)
    assert!(
        state.is_parent_valid(SlotIndex::new(0)),
        "Slot 0 should be valid parent (notarized in C++ mode)"
    );

    // Setup slot 1: Submit with slot 0 as parent
    let candidate_1 = create_test_candidate(
        1,
        block_hash_1.clone(),
        BlockIdExt::default(),
        Some((0, block_hash_0.clone())),
        1,
    );
    state.on_candidate(&desc, candidate_1).unwrap();

    // Notarize and finalize slot 1 (parent is notarized, not finalized)
    let notar_vote_1 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: block_hash_1.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), notar_vote_1.clone(), Vec::new())
            .unwrap();
    }
    drain_events(&mut state);

    let finalize_vote_1 =
        Vote::Finalize(FinalizeVote { slot: SlotIndex::new(1), block_hash: block_hash_1.clone() });
    for i in 0..3 {
        state
            .on_vote_test(&desc, ValidatorIndex::new(i), finalize_vote_1.clone(), Vec::new())
            .unwrap();
    }

    // Collect events
    let mut events = Vec::new();
    while let Some(event) = state.pull_event() {
        events.push(event);
    }

    let finalized_slots: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SimplexEvent::BlockFinalized(ev) => Some(ev.slot),
            _ => None,
        })
        .collect();

    // Slot 1 should be finalized even though slot 0 is only notarized
    assert_eq!(
        finalized_slots,
        vec![SlotIndex::new(1)],
        "Slot 1 should be finalized (parent slot 0 is notarized)"
    );

    // first_non_finalized_slot should advance to 2 (skipping slot 0)
    assert_eq!(
        state.first_non_finalized_slot,
        SlotIndex::new(2),
        "first_non_finalized_slot should be 2"
    );
}

/// Helper to drain all events from state
fn drain_events(state: &mut SimplexState) {
    while state.pull_event().is_some() {}
}

/*
    ========================================================================
    Restart Support (Phase 6)
    ========================================================================
*/

#[test]
fn test_restart_local_vote_flags() {
    // Verify local bootstrap flags are applied with correct semantics.
    // Reference: C++ consensus.cpp start_up() vote loop:
    // - voted_notar = notar_vote.id
    // - voted_skip = true
    // - voted_final = true
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    let hash0 = UInt256::from([0x11u8; 32]);

    state.mark_slot_voted_on_restart(
        &desc,
        &Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: hash0.clone() }),
    );

    let slot0 = &state.get_window(WindowIndex::new(0)).unwrap().slots[0];
    assert!(slot0.is_voted, "notar vote should set is_voted");
    assert_eq!(
        slot0.voted_notar,
        Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: hash0.clone() }),
        "notar vote should set voted_notar(slot,hash)"
    );
    assert!(!slot0.voted_skip, "notar vote should NOT set voted_skip");
    assert!(!slot0.its_over, "notar vote should NOT set its_over");

    state.mark_slot_voted_on_restart(&desc, &Vote::Skip(SkipVote { slot: SlotIndex::new(1) }));
    let slot1 = &state.get_window(WindowIndex::new(0)).unwrap().slots[1];
    assert!(slot1.is_voted, "skip vote should set is_voted");
    assert!(slot1.voted_skip, "skip vote should set voted_skip");
    assert!(slot1.is_bad_window, "skip vote should set is_bad_window");

    state.mark_slot_voted_on_restart(
        &desc,
        &Vote::Finalize(FinalizeVote { slot: SlotIndex::new(0), block_hash: hash0 }),
    );
    let slot0 = &state.get_window(WindowIndex::new(0)).unwrap().slots[0];
    assert!(slot0.its_over, "final vote should set its_over (voted_final)");
}

#[test]
fn test_restart_skip_marks_state() {
    // Restart skip generation must mark local skip state before broadcasting.
    // Reference: C++ consensus.cpp start_up() L74-77.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Pretend slot 0 is already finalized (should not be skipped).
    state.get_window_mut(WindowIndex::new(0)).unwrap().slots[0].its_over = true;

    // first_nonannounced_window = 1 => previous window is 0 => slots [0,1]
    let queued = state.generate_restart_skip_votes(WindowIndex::new(1), 2);
    assert_eq!(queued, 1, "should queue skip only for non-finalized slots");

    // Slot 1 must be marked before enqueueing
    let slot1 = &state.get_window(WindowIndex::new(0)).unwrap().slots[1];
    assert!(slot1.is_voted, "restart skip should set is_voted");
    assert!(slot1.voted_skip, "restart skip should set voted_skip");
    assert!(slot1.is_bad_window, "restart skip should set is_bad_window");

    // Should enqueue a Skip vote for slot 1
    let mut seen_skip_1 = false;
    while let Some(ev) = state.pull_event() {
        if matches!(ev, SimplexEvent::BroadcastVote(Vote::Skip(SkipVote { slot })) if slot == SlotIndex::new(1))
        {
            seen_skip_1 = true;
        }
    }
    assert!(seen_skip_1, "expected BroadcastVote(SkipVote(slot=1))");
}

#[test]
fn test_restart_finalize_blocked_by_skip() {
    // After restart with skip vote, try_final() must be blocked by local skip state.
    // Reference: C++ consensus.cpp L230: `!voted_skip && !voted_final && voted_notar==id`.
    let desc = create_test_desc(4, 1);
    let hash0 = UInt256::from([0x22u8; 32]);

    // Baseline: without voted_skip, try_final should broadcast Finalize after notar cert observed.
    {
        let mut state =
            SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");
        state.mark_slot_voted_on_restart(
            &desc,
            &Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: hash0.clone() }),
        );
        state.on_block_notarized(&desc, SlotIndex::new(0), hash0.clone());

        let mut saw_finalize = false;
        while let Some(ev) = state.pull_event() {
            if matches!(ev, SimplexEvent::BroadcastVote(Vote::Finalize(FinalizeVote { slot, .. })) if slot == SlotIndex::new(0))
            {
                saw_finalize = true;
            }
        }
        assert!(saw_finalize, "expected auto-finalize broadcast when not skipped");
    }

    // With voted_skip=true, try_final must not broadcast Finalize.
    {
        let mut state =
            SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");
        state.mark_slot_voted_on_restart(
            &desc,
            &Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: hash0.clone() }),
        );

        // Simulate persisted local skip state (without forcing BadWindow).
        let slot0 = &mut state.get_window_mut(WindowIndex::new(0)).unwrap().slots[0];
        slot0.voted_skip = true;

        state.on_block_notarized(&desc, SlotIndex::new(0), hash0);

        let mut saw_finalize = false;
        while let Some(ev) = state.pull_event() {
            if matches!(ev, SimplexEvent::BroadcastVote(Vote::Finalize(_))) {
                saw_finalize = true;
            }
        }
        assert!(!saw_finalize, "must not auto-finalize after local skip vote");
    }
}

#[test]
fn test_cpp_mode_local_notarize_after_skip() {
    // C++ allows Notarize after Skip from the same validator (skip is not a notar block).
    // Reference: C++ consensus.cpp on_candidate_to_notarize checks only voted_notar, not voted_skip.
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create SimplexState");

    // Locally vote skip for slot 0 (window 0).
    state.try_skip_window(WindowIndex::new(0));
    drain_events(&mut state);

    // Now we should still be able to notarize slot 0.
    let hash0 = UInt256::from([0x33u8; 32]);
    let candidate0 =
        create_test_candidate(0, hash0.clone(), BlockIdExt::default(), None, /*leader=*/ 0);
    state.on_candidate(&desc, candidate0).unwrap();

    let mut saw_notar = false;
    while let Some(ev) = state.pull_event() {
        if matches!(ev, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if slot == SlotIndex::new(0))
        {
            saw_notar = true;
        }
    }
    assert!(saw_notar, "expected notarize broadcast after local skip in C++ mode");
}

#[test]
fn test_notarized_parent_chain_state_tracked_in_default_mode_on_notarization() {
    // Notarized-parent chain fields (`available_base`, `skipped`,
    // `first_non_progressed_slot`) are maintained in the active C++-parity mode.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let h0 = UInt256::from([0xC0u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // Tracking: slot0 notarized => progress cursor advances to 1 and slot1 base is set.
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert_eq!(
        w0.slots[1].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0 })),
        "slot1 base must be set from notarized slot0 (tracking only)"
    );

    // Slot 0 is still inside window 0, so the leader window does not advance yet.
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));
}

#[test]
fn test_notarized_parent_chain_state_tracked_in_default_mode_on_skip_cert() {
    // Skip certificates must update the active C++-parity tracking state too.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let vote0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // Tracking: slot0 skipped => progress cursor advances to 1 and slot1 base is propagated from slot0.
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(w0.slots[0].skipped, "slot0 must be marked skipped (skip cert)");
    assert_eq!(
        w0.slots[1].available_base,
        Some(None),
        "slot1 base must be propagated genesis from skipped slot0"
    );

    // Slot 0 is still inside window 0, so the leader window does not advance yet.
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));
}

/*
    ========================================================================
    Alpenglow Mode Compatibility Verification
    ========================================================================
*/

#[test]
fn test_alpenglow_mode_with_notarized_parent_chain_tracking() {
    // Verify that Alpenglow's fallback protocol (`enable_fallback_protocol=true`)
    // doesn't conflict with notarized-parent chain tracking (which is always maintained).
    // The two features should be orthogonal.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create state");

    // Progress slot0 via notarization
    let h0 = UInt256::from([0xF0u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // Notarized-parent chain tracking should update progress cursor and propagate base
    assert_eq!(
        state.first_non_progressed_slot,
        SlotIndex::new(1),
        "first_non_progressed_slot should advance in Alpenglow mode"
    );
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert_eq!(
        w0.slots[1].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0 })),
        "slot1 base must be set in Alpenglow mode"
    );

    // Slot 0 is still inside window 0, so the leader window does not advance yet.
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(0),
        "Alpenglow mode should still remain in window 0 until progress crosses the boundary"
    );
}

/*
    ========================================================================
    Pool.cpp Parity Harness: notarized-parent chain (`first_non_progressed_slot` + `available_base`)
    ========================================================================
*/

#[test]
fn test_notarized_parent_chain_startup_sets_slot0_base_and_first_non_progressed_slot() {
    // Mirrors C++ pool.cpp start_up():
    // - slot 0 available_base = RawParentId{} (genesis)
    // - now_ starts at 0
    let desc = create_test_desc(4, 2);
    let state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(0));

    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert_eq!(w0.slots[0].available_base, Some(None), "slot0 base must be genesis");
    assert!(!w0.slots[0].skipped, "slot0 must not be skipped at startup");
}

#[test]
fn test_notarized_parent_chain_on_notarization_sets_next_base_and_advances_progress_cursor() {
    // Mirrors C++ pool.cpp on_notarization():
    // - set available_base for next non-skipped slot to notarized id
    // - maybe_publish_new_leader_windows() advances now_ on notarized slots
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let h0 = UInt256::from([0xA0u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });

    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // progress cursor should advance from 0 -> 1 because slot0 is notarized
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));

    // slot1.available_base should be set to id(slot0,h0)
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert_eq!(
        w0.slots[1].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0 })),
        "slot1 base must be set from notarized slot0"
    );
}

#[test]
fn test_notarized_parent_chain_advances_to_next_window_only_after_window_progressed() {
    // window size = 2 slots: [0,1] in window0, [2,3] in window1.
    // progress cursor should cross to 2 only after slot1 is progressed.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Progress slot0 via notarization
    let h0 = UInt256::from([0xA1u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));

    // Progress slot1 via notarization
    let h1 = UInt256::from([0xA2u8; 32]);
    let vote1 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: h1.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), vec![4]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), vec![5]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, vec![6]).unwrap();

    // progress cursor should advance through slot1 to start of window1 (slot2)
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "notarized-parent chain mode must advance leader window on progress cursor progression"
    );
    assert_eq!(
        state.skip_slot,
        SlotIndex::new(2),
        "timeouts must be scheduled from the new window start slot"
    );

    // slot2.available_base should be set from notarized slot1
    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(1), hash: h1 })),
        "slot2 base must be set from notarized slot1"
    );
}

#[test]
fn test_notarized_parent_chain_on_skip_propagates_base_and_advances_progress_cursor() {
    // Mirrors C++ pool.cpp on_skip():
    // - mark slot skipped
    // - propagate available_base forward when next base is unknown
    // - advance now_ on skipped slots
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Skip slot0 (genesis base) -> slot1 base should become genesis
    let vote0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    assert_eq!(
        state.first_non_progressed_slot,
        SlotIndex::new(1),
        "first_non_progressed_slot must advance after skip"
    );

    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(w0.slots[0].skipped, "slot0 must be marked skipped");
    assert_eq!(w0.slots[1].available_base, Some(None), "slot1 base must be propagated genesis");
}

#[test]
fn test_notarized_parent_chain_skipped_slot_is_skipped_by_next_nonskipped_on_notarization() {
    // Scenario:
    // 1) slot1 gets skipped before slot0 is progressed
    // 2) notarization of slot0 must set base for slot2 (skipping slot1)
    // This mirrors pool.cpp use of skip_intervals_ + next_nonskipped_slot_after().
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Skip slot1 first (out-of-order)
    let vote1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, vec![3]).unwrap();

    assert_eq!(
        state.first_non_progressed_slot,
        SlotIndex::new(0),
        "first_non_progressed_slot must not advance until slot0 progressed"
    );
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].skipped,
        "slot1 must be skipped"
    );

    // Now notarize slot0
    let h0 = UInt256::from([0xB0u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![4]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![5]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![6]).unwrap();

    // progress cursor should advance 0 -> 1 (notarized) -> 2 (slot1 skipped)
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));

    // slot2 (window1, offset 0) base must be set from slot0 notarization (skipping slot1)
    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0 })),
        "slot2 base must come from notarized slot0 when slot1 is skipped"
    );
}

/*
    ========================================================================
    External Certificate Handling Tests
    ========================================================================

    Tests for set_finalize_certificate and
    set_skip_certificate APIs
*/

/// Create a test FinalCert for a given slot and block hash
fn create_test_final_cert(
    _desc: &SessionDescription,
    slot: SlotIndex,
    block_hash: UInt256,
    signers: &[ValidatorIndex],
) -> crate::certificate::FinalCertPtr {
    use super::FinalizeVote;
    use crate::certificate::{Certificate, VoteSignature};

    let vote = FinalizeVote { slot, block_hash };
    let signatures: Vec<VoteSignature> = signers
        .iter()
        .map(|&idx| VoteSignature {
            validator_idx: idx,
            signature: vec![idx.value() as u8; 64], // dummy signature
        })
        .collect();

    Arc::new(Certificate { vote, signatures })
}

/// Create a test SkipCert for a given slot
fn create_test_skip_cert(
    _desc: &SessionDescription,
    slot: SlotIndex,
    signers: &[ValidatorIndex],
) -> crate::certificate::SkipCertPtr {
    use super::SkipVote;
    use crate::certificate::{Certificate, VoteSignature};

    let vote = SkipVote { slot };
    let signatures: Vec<VoteSignature> = signers
        .iter()
        .map(|&idx| VoteSignature {
            validator_idx: idx,
            signature: vec![idx.value() as u8; 64], // dummy signature
        })
        .collect();

    Arc::new(Certificate { vote, signatures })
}

/// Create a test NotarCert for a given slot and block hash
fn create_test_notar_cert(
    _desc: &SessionDescription,
    slot: SlotIndex,
    block_hash: UInt256,
    signers: &[ValidatorIndex],
) -> crate::certificate::NotarCertPtr {
    use super::NotarizeVote;
    use crate::certificate::{Certificate, VoteSignature};

    let vote = NotarizeVote { slot, block_hash };
    let signatures: Vec<VoteSignature> = signers
        .iter()
        .map(|&idx| VoteSignature {
            validator_idx: idx,
            signature: vec![idx.value() as u8; 64], // dummy signature
        })
        .collect();

    Arc::new(Certificate { vote, signatures })
}

#[test]
fn test_collect_cached_certificates_in_range_filters_and_sorts() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Slot 0: notar + final
    let slot0 = SlotIndex::new(0);
    let h0 = UInt256::from([0x10; 32]);
    let notar0 = create_test_notar_cert(&desc, slot0, h0.clone(), &signers);
    let final0 = create_test_final_cert(&desc, slot0, h0.clone(), &signers);
    {
        // Use low-level slot storage to avoid triggering notarized-parent chain progression invariants.
        let sv = state.slot_votes_at(slot0);
        sv.store_notarize_certificate(&h0, notar0).expect("should not conflict");
        sv.store_finalize_certificate(&h0, final0).expect("should not conflict");
    }

    // Slot 3: final only
    let slot3 = SlotIndex::new(3);
    let h3 = UInt256::from([0x33; 32]);
    let final3 = create_test_final_cert(&desc, slot3, h3.clone(), &signers);
    state
        .slot_votes_at(slot3)
        .store_finalize_certificate(&h3, final3)
        .expect("should not conflict");

    // Slot 5: skip only
    let slot5 = SlotIndex::new(5);
    let skip5 = create_test_skip_cert(&desc, slot5, &signers);
    state.slot_votes_at(slot5).store_skip_certificate(skip5).expect("should not conflict");

    // Slot 12: notar only (outside our tested range)
    let slot12 = SlotIndex::new(12);
    let h12 = UInt256::from([0xCC; 32]);
    let cert12 = create_test_notar_cert(&desc, slot12, h12.clone(), &signers);
    state
        .slot_votes_at(slot12)
        .store_notarize_certificate(&h12, cert12)
        .expect("should not conflict");

    // Range [0,10) should include slots 0,3,5 only, sorted.
    let bundles = state.collect_cached_certificates_in_range(0, 10);
    let slots: Vec<u32> = bundles.iter().map(|(s, _, _, _)| s.value()).collect();
    assert_eq!(slots, vec![0, 3, 5]);

    // slot0 has notar + final
    assert!(bundles[0].1.is_some(), "slot0 notar must be present");
    assert!(bundles[0].2.is_none(), "slot0 skip must be absent");
    assert!(bundles[0].3.is_some(), "slot0 final must be present");

    // slot3 has final only
    assert!(bundles[1].1.is_none(), "slot3 notar must be absent");
    assert!(bundles[1].2.is_none(), "slot3 skip must be absent");
    assert!(bundles[1].3.is_some(), "slot3 final must be present");

    // slot5 has skip only
    assert!(bundles[2].1.is_none(), "slot5 notar must be absent");
    assert!(bundles[2].2.is_some(), "slot5 skip must be present");
    assert!(bundles[2].3.is_none(), "slot5 final must be absent");

    // Range [4,10) should include only slot5.
    let bundles_narrow = state.collect_cached_certificates_in_range(4, 10);
    assert_eq!(bundles_narrow.len(), 1);
    assert_eq!(bundles_narrow[0].0, slot5);
}

#[test]
fn test_get_last_finalize_certificate_returns_highest_slot() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    assert!(state.get_last_finalize_certificate().is_none(), "no final certs yet");

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    let slot1 = SlotIndex::new(1);
    let h1 = UInt256::from([0x11; 32]);
    let final1 = create_test_final_cert(&desc, slot1, h1.clone(), &signers);
    state
        .slot_votes_at(slot1)
        .store_finalize_certificate(&h1, final1)
        .expect("should not conflict");

    let (s, cert) = state.get_last_finalize_certificate().expect("must have last final cert");
    assert_eq!(s, slot1);
    assert_eq!(cert.vote.slot, slot1);
    assert_eq!(cert.vote.block_hash, h1);

    let slot7 = SlotIndex::new(7);
    let h7 = UInt256::from([0x77; 32]);
    let final7 = create_test_final_cert(&desc, slot7, h7.clone(), &signers);
    state
        .slot_votes_at(slot7)
        .store_finalize_certificate(&h7, final7)
        .expect("should not conflict");

    let (s2, cert2) = state.get_last_finalize_certificate().expect("must have last final cert");
    assert_eq!(s2, slot7, "should pick highest slot");
    assert_eq!(cert2.vote.slot, slot7);
    assert_eq!(cert2.vote.block_hash, h7);
}

#[test]
fn test_set_finalize_certificate_stores_old_slot_without_tracking() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Simulate a "cleaned up" old slot: advance the finalized cursor beyond slot1
    // without having any FinalCert stored for slot1.
    let slot1 = SlotIndex::new(1);
    state.first_non_finalized_slot = SlotIndex::new(2);
    state.first_non_progressed_slot = SlotIndex::new(2);

    let before_nf = state.first_non_finalized_slot;
    let before_np = state.first_non_progressed_slot;

    let hash_a = UInt256::from([0xAA; 32]);

    state
        .set_finalize_certificate(
            &desc,
            slot1,
            &hash_a,
            create_test_final_cert(&desc, slot1, hash_a.clone(), &signers),
        )
        .expect("should not conflict");

    // Old-slot FinalCert must be stored for gapless commit / restart support,
    // but must NOT affect FSM tracking or emit events.
    assert!(
        state.get_finalize_certificate(slot1, &hash_a).is_some(),
        "old-slot FinalCert must be retrievable"
    );
    assert_eq!(
        state.first_non_finalized_slot, before_nf,
        "old-slot FinalCert must not change first_non_finalized_slot"
    );
    assert_eq!(
        state.first_non_progressed_slot, before_np,
        "old-slot FinalCert must not change first_non_progressed_slot"
    );
    assert!(!state.has_pending_events(), "old-slot FinalCert must not emit events");
}

#[test]
fn test_set_finalize_certificate_updates_vote_accounting() {
    // Test that set_finalize_certificate correctly updates
    // vote accounting when receiving a finalize certificate from an external source.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from([0xAA; 32]);

    // Create a FinalCert with signatures from validators 0, 1, 2 (3/4 = 75% > 2/3)
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let final_cert = create_test_final_cert(&desc, slot, block_hash.clone(), &signers);

    // Apply the external certificate
    let stored = state
        .set_finalize_certificate(&desc, slot, &block_hash, final_cert)
        .expect("should not conflict");

    assert!(stored, "certificate should be stored");
    assert!(state.has_finalize_certificate(slot), "should have finalize certificate");
    assert_eq!(
        state.get_finalize_weight(slot, &block_hash),
        3, // validators 0,1,2 each have weight 1
        "finalize weight should be 3"
    );
}

#[test]
fn test_set_finalize_certificate_deduplicates() {
    // Test that applying the same certificate twice doesn't change state.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from([0xBB; 32]);

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let final_cert = create_test_final_cert(&desc, slot, block_hash.clone(), &signers);

    // Apply first time
    let stored1 = state
        .set_finalize_certificate(&desc, slot, &block_hash, final_cert.clone())
        .expect("should not conflict");
    assert!(stored1, "first application should store");
    // Drain first-store events so we can assert duplicate store emits none.
    while state.pull_event().is_some() {}

    // Apply second time
    let stored2 = state
        .set_finalize_certificate(&desc, slot, &block_hash, final_cert)
        .expect("should not conflict");
    assert!(!stored2, "second application should be deduplicated");

    // Weight should still be 3
    assert_eq!(state.get_finalize_weight(slot, &block_hash), 3);
    assert!(
        !state.has_pending_events(),
        "duplicate finalize cert must not emit relay-triggering events"
    );
}

#[test]
fn test_set_skip_certificate_deduplicates_without_events() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let slot = SlotIndex::new(2);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot, &signers);

    let stored1 = state
        .set_skip_certificate(&desc, slot, skip_cert.clone())
        .expect("first set_skip_certificate should succeed");
    assert!(stored1, "first skip certificate application should store");
    while state.pull_event().is_some() {}

    let stored2 = state
        .set_skip_certificate(&desc, slot, skip_cert)
        .expect("second set_skip_certificate should succeed");
    assert!(!stored2, "second skip certificate application should be deduplicated");
    assert!(
        !state.has_pending_events(),
        "duplicate skip cert must not emit relay-triggering events"
    );
}

#[test]
fn test_set_skip_certificate_updates_vote_accounting() {
    // Test that set_skip_certificate correctly updates
    // vote accounting when receiving a skip certificate from an external source.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let slot = SlotIndex::new(0);

    // Create a SkipCert with signatures from validators 0, 1, 2 (3/4 = 75% > 2/3)
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot, &signers);

    // Apply the external certificate
    let stored = state.set_skip_certificate(&desc, slot, skip_cert).expect("should not error");

    assert!(stored, "certificate should be stored");
    assert!(state.has_skip_certificate(slot), "should have skip certificate");
    assert_eq!(
        state.get_skip_weight(slot),
        3, // validators 0,1,2 each have weight 1
        "skip weight should be 3"
    );
}

#[test]
fn test_set_skip_certificate_marks_slot_skipped() {
    // Test that set_skip_certificate marks the slot as skipped
    // in the window state.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let slot = SlotIndex::new(1);

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot, &signers);

    // Slot should not be skipped initially
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(!w0.slots[1].skipped, "slot should not be skipped initially");

    // Apply the external certificate
    state.set_skip_certificate(&desc, slot, skip_cert).expect("should not error");

    // Slot should now be marked as skipped
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(w0.slots[1].skipped, "slot should be marked as skipped");
}

#[test]
fn test_set_skip_certificate_propagates_base() {
    // Test that set_skip_certificate propagates available_base
    // to the next slot (C++ pool.cpp parity).
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // First, notarize slot 0 to establish a base
    let slot0 = SlotIndex::new(0);
    let block_hash0 = UInt256::from([0xCC; 32]);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let notar_cert = create_test_notar_cert(&desc, slot0, block_hash0.clone(), &signers);

    state
        .set_notarize_certificate(&desc, slot0, &block_hash0, notar_cert)
        .expect("should not conflict");

    // Now skip slot 1
    let slot1 = SlotIndex::new(1);
    let skip_cert = create_test_skip_cert(&desc, slot1, &signers);
    state.set_skip_certificate(&desc, slot1, skip_cert).expect("should not error");

    // Slot 2 (window 1, offset 0) should have the base from slot 0
    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: slot0, hash: block_hash0 })),
        "slot2 base should come from notarized slot0 when slot1 is skipped"
    );
}

#[test]
fn test_set_skip_certificate_emits_slot_skipped_event_for_tracked_slot() {
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Clear any initial events
    while state.pull_event().is_some() {}

    let slot = SlotIndex::new(1);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot, &signers);

    let stored = state.set_skip_certificate(&desc, slot, skip_cert).expect("should not error");
    assert!(stored, "skip certificate should be stored");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(ev) if ev.slot == slot)),
        "Expected SlotSkipped event for slot {}, got {:?}",
        slot,
        events
    );
}

/// C++ parity (pool.cpp handle_saved_certificate): set_skip_certificate must emit
/// SkipCertificateReached so SessionProcessor relays foreign skip certificates.
#[test]
fn test_set_skip_certificate_emits_skip_cert_reached() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    while state.pull_event().is_some() {}

    let slot = SlotIndex::new(1);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot, &signers);

    let stored = state.set_skip_certificate(&desc, slot, skip_cert).expect("should not error");
    assert!(stored, "skip certificate should be stored");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    let skip_reached = events
        .iter()
        .find_map(|e| match e {
            SimplexEvent::SkipCertificateReached(ev) if ev.slot == slot => Some(ev),
            _ => None,
        })
        .expect("Expected SkipCertificateReached event for foreign skip cert");
    assert_eq!(skip_reached.slot, slot);
}

#[test]
fn test_set_skip_certificate_does_not_emit_slot_skipped_event_for_old_slot() {
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Mark slot 0 as already finalized/old
    state.set_first_non_finalized_slot_for_test(SlotIndex::new(1));

    // Clear any initial events
    while state.pull_event().is_some() {}

    let slot0 = SlotIndex::new(0);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, slot0, &signers);

    let stored = state.set_skip_certificate(&desc, slot0, skip_cert).expect("should not error");
    assert!(!stored, "old skip certificates should be ignored (no-op)");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SlotSkipped(_))),
        "SlotSkipped must not be emitted for old slots, got {:?}",
        events
    );
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::SkipCertificateReached(_))),
        "SkipCertificateReached must not be emitted for old slots, got {:?}",
        events
    );
}

#[test]
fn test_external_finalize_certificate_for_missed_finalization_recovery() {
    // Scenario: Simulate a node that missed finalization votes but receives
    // the finalize certificate from a peer. This tests the recovery path.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Simulate: We have notarization for slot 0 but missed finalize votes
    let slot0 = SlotIndex::new(0);
    let block_hash = UInt256::from([0xDD; 32]);

    // First establish notarization
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let notar_cert = create_test_notar_cert(&desc, slot0, block_hash.clone(), &signers);
    state
        .set_notarize_certificate(&desc, slot0, &block_hash, notar_cert)
        .expect("should not conflict");

    // Verify we don't have finalize yet
    assert!(!state.has_finalize_certificate(slot0));
    assert_eq!(state.get_finalize_weight(slot0, &block_hash), 0);

    // Now receive finalize certificate from peer
    let final_cert = create_test_final_cert(&desc, slot0, block_hash.clone(), &signers);
    let stored = state
        .set_finalize_certificate(&desc, slot0, &block_hash, final_cert)
        .expect("should not conflict");

    assert!(stored, "finalize certificate should be stored");
    assert!(state.has_finalize_certificate(slot0));
    assert_eq!(state.get_finalize_weight(slot0, &block_hash), 3);

    // first_non_finalized_slot should be updated to slot0 + 1
    assert_eq!(state.first_non_finalized_slot, SlotIndex::new(1));
}

#[test]
fn test_set_finalize_certificate_advances_progress_cursor_past_pre_skipped_slots() {
    // Regression: if slots after the finalized slot are already skipped,
    // finalization must run progress-cursor advancement (`advance_present` parity)
    // before leader-window advancement so we don't stop on a baseless skipped slot.
    let desc = create_test_desc(4, 4); // 4 slots per window
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Pre-mark slots 1..3 as skipped (out of order, before slot 0 finalization).
    for s in 1..=3u32 {
        let cert = create_test_skip_cert(&desc, SlotIndex::new(s), &signers);
        state.set_skip_certificate(&desc, SlotIndex::new(s), cert).unwrap();
    }
    drain_events(&mut state);

    // Slot 0 is still not progressed, so cursor must stay at 0.
    assert_eq!(state.get_first_non_progressed_slot(), SlotIndex::new(0));

    // Finalize slot 0 (without prior explicit notarization event ingestion).
    let slot0 = SlotIndex::new(0);
    let block_hash = UInt256::from([0xA5; 32]);
    let final_cert = create_test_final_cert(&desc, slot0, block_hash.clone(), &signers);
    let stored = state
        .set_finalize_certificate(&desc, slot0, &block_hash, final_cert)
        .expect("set_finalize_certificate should succeed");
    assert!(stored, "finalize certificate should be stored");

    // Cursor must skip over already-progressed slots 1..3 and land on slot 4.
    assert_eq!(
        state.get_first_non_progressed_slot(),
        SlotIndex::new(4),
        "progress cursor must advance past pre-skipped slots after finalization"
    );

    // Slot 4 must have finalized parent as available base.
    let expected_parent = Some(CandidateParentInfo { slot: slot0, hash: block_hash });
    assert_eq!(
        state.get_slot_available_base(&desc, SlotIndex::new(4)),
        Some(expected_parent),
        "slot 4 must inherit base from finalized slot 0"
    );

    // Window should advance from 0 to 1 (slot 4 is first slot of window 1).
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(1));
}

#[test]
fn test_set_finalize_certificate_emits_block_finalized_and_finalization_reached_for_tracked_slot() {
    // External finalize cert ingestion must emit:
    // - BlockFinalized (commit trigger), and
    // - FinalizationReached (standstill caching)
    // for tracked (non-old) slots.
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    // Clear any initial events
    while state.pull_event().is_some() {}

    let slot = SlotIndex::new(0);
    let block_hash = UInt256::from([0xEF; 32]);
    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let cert = create_test_final_cert(&desc, slot, block_hash.clone(), &signers);

    let stored = state
        .set_finalize_certificate(&desc, slot, &block_hash, cert.clone())
        .expect("should not conflict");
    assert!(stored, "finalize certificate should be stored");

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();

    let finalized = events.iter().find_map(|e| {
        if let SimplexEvent::BlockFinalized(ev) = e {
            Some(ev)
        } else {
            None
        }
    });
    let final_reached = events.iter().find_map(|e| {
        if let SimplexEvent::FinalizationReached(ev) = e {
            Some(ev)
        } else {
            None
        }
    });

    assert!(finalized.is_some(), "Expected BlockFinalized event, got {:?}", events);
    assert!(final_reached.is_some(), "Expected FinalizationReached event, got {:?}", events);

    let finalized = finalized.unwrap();
    assert_eq!(finalized.slot, slot);
    assert_eq!(finalized.block_hash, block_hash);
    assert!(Arc::ptr_eq(&finalized.certificate, &cert));

    let final_reached = final_reached.unwrap();
    assert_eq!(final_reached.slot, slot);
    assert_eq!(final_reached.block_hash, block_hash);
    assert!(Arc::ptr_eq(&final_reached.certificate, &cert));
}

/*
    ========================================================================
    TODO: Additional Tests to Implement
    ========================================================================

    These tests require more complex setup or time control:

    - test_try_final_blocked_by_bad_window:
      - Set `BadWindow` and verify we do NOT broadcast Finalize in that window.

    - test_try_skip_window_broadcasts_for_unvoted_on_timeout:
      - Trigger Timeout(s) via `check_all()` (time manipulation / deterministic clock) and verify
        `try_skip_window()` broadcasts Skip for unvoted slots.

    - test_check_pending_blocks_processes_in_order:
      - Queue multiple pending blocks in different slots and verify heap ordering + FIFO behavior.

    - test_multiple_pending_slots_across_windows:
      - Pending blocks spanning windows; ensure window creation/pruning doesn't break processing.

    - test_adaptive_timeout_backoff_increases_after_timeouts:
      - Force `LeaderWindow.had_timeouts=true` and verify timeout backoff increases.

    - test_restore_default_timeouts_after_successful_window:
      - First increase backoff, then complete a window without timeouts and verify restore path.

    - test_notarized_parent_chain_advances_window_on_full_window_skip:
      - With the default C++-parity progression path, skip-cert all slots in a window and verify
        `first_non_progressed_slot` crosses the boundary and timeouts are scheduled for the next window.

    - test_notarized_parent_chain_base_propagation_with_multiple_skipped_intervals:
      - Combine non-contiguous skip certificates + notarizations and verify bases jump to the
        next non-skipped slot (C++ skip_intervals_ parity).
*/

// ============================================================================
// Slot bounds hardening tests
// ============================================================================

#[test]
fn test_reject_far_future_vote() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let far_slot = state.first_too_new_vote_slot();

    let vote = Vote::Notarize(NotarizeVote { slot: far_slot, block_hash: UInt256::rand() });

    let result = state.on_vote_test(&desc, ValidatorIndex::new(1), vote, vec![]);
    match result {
        VoteResult::Rejected(reason) => {
            assert!(reason.contains("too far ahead"), "unexpected reason: {}", reason);
        }
        other => panic!("Expected Rejected, got {:?}", other),
    }
}

#[test]
fn test_accept_vote_at_boundary() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let boundary_slot = state.first_too_new_vote_slot() - 1;

    // Slot immediately before first_too_new should be accepted (not rejected by bounds check).
    // It may still return Rejected/Applied depending on FSM state, but NOT "too far ahead".
    let vote = Vote::Notarize(NotarizeVote { slot: boundary_slot, block_hash: UInt256::rand() });

    let result = state.on_vote_test(&desc, ValidatorIndex::new(1), vote, vec![]);
    match result {
        VoteResult::Rejected(reason) => {
            assert!(
                !reason.contains("too far ahead"),
                "boundary slot should not be rejected as too far ahead: {}",
                reason
            );
        }
        _ => {} // Applied/Duplicate are also fine
    }
}

#[test]
fn test_reject_far_future_candidate() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let max_future_slots =
        desc.opts().max_leader_window_desync.saturating_mul(desc.opts().slots_per_leader_window);

    let far_slot = max_future_slots + 1;
    let candidate =
        create_test_candidate(far_slot, UInt256::rand(), BlockIdExt::default(), None, 0);

    // on_candidate returns Ok(()) for silently dropped candidates
    let initial_len = state.leader_windows.len();
    let result = state.on_candidate(&desc, candidate);
    assert!(result.is_ok(), "far-future candidate should be silently dropped, not error");

    // Verify no new windows were allocated
    assert_eq!(
        state.leader_windows.len(),
        initial_len,
        "far-future candidate should not allocate any new windows"
    );
}

#[test]
fn test_reject_far_future_window_base_ready() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let max_future_slots =
        desc.opts().max_leader_window_desync.saturating_mul(desc.opts().slots_per_leader_window);

    let far_window = WindowIndex::new((max_future_slots / 2) + 100);
    let initial_len = state.leader_windows.len();
    let result = state.on_window_base_ready(&desc, far_window, None);
    assert!(result.is_ok(), "far-future window base should be silently dropped");

    assert_eq!(
        state.leader_windows.len(),
        initial_len,
        "far-future window base should not allocate any new windows"
    );
}

#[test]
fn test_ensure_window_exists_capped() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");

    let initial_len = state.leader_windows.len();

    // Try to create a window way beyond the cap
    let huge_window = WindowIndex::new(1_000_000);
    state.ensure_window_exists(huge_window);

    // Window count should NOT have grown to 1M
    assert_eq!(
        state.leader_windows.len(),
        initial_len,
        "ensure_window_exists should refuse to allocate beyond configured desync cap"
    );
}

#[test]
fn test_vote_bound_with_advanced_finalization() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let expected_first_too_new = SlotIndex::new(
        ((5000 / desc.opts().slots_per_leader_window) + desc.opts().max_leader_window_desync + 1)
            * desc.opts().slots_per_leader_window,
    );

    // Advance finalization cursor
    state.set_first_non_finalized_slot(SlotIndex::new(5000));

    let first_too_new = state.first_too_new_vote_slot();
    assert_eq!(first_too_new, expected_first_too_new);

    // Vote at first_too_new must be rejected.
    let vote = Vote::Skip(SkipVote { slot: first_too_new });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(1), vote, vec![]);
    match result {
        VoteResult::Rejected(reason) => {
            assert!(reason.contains("too far ahead"), "unexpected reason: {}", reason);
        }
        other => panic!("Expected Rejected, got {:?}", other),
    }

    // Vote immediately before first_too_new should still pass the bounds check.
    let vote = Vote::Skip(SkipVote { slot: first_too_new - 1 });
    let result = state.on_vote_test(&desc, ValidatorIndex::new(1), vote, vec![]);
    match result {
        VoteResult::Rejected(reason) => {
            assert!(
                !reason.contains("too far ahead"),
                "boundary slot should not be rejected as too far ahead: {}",
                reason
            );
        }
        _ => {}
    }
}

#[test]
fn test_is_slot_too_far_ahead_helper() {
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let max_future_slots =
        desc.opts().max_leader_window_desync.saturating_mul(desc.opts().slots_per_leader_window);

    assert!(!state.is_slot_too_far_ahead(SlotIndex::new(0)));
    assert!(!state.is_slot_too_far_ahead(SlotIndex::new(max_future_slots)));
    assert!(state.is_slot_too_far_ahead(SlotIndex::new(max_future_slots + 1)));
    assert!(state.is_slot_too_far_ahead(SlotIndex::new(u32::MAX)));
}

#[test]
fn test_is_vote_slot_too_far_ahead_helper() {
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("create");
    let first_too_new = state.first_too_new_vote_slot();

    assert!(!state.is_vote_slot_too_far_ahead(first_too_new - 1));
    assert!(state.is_vote_slot_too_far_ahead(first_too_new));
    assert!(state.is_vote_slot_too_far_ahead(SlotIndex::new(u32::MAX)));
}

#[test]
fn test_max_acceptable_slot_uses_progress_cursor_after_skip() {
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");
    let max_future_slots =
        desc.opts().max_leader_window_desync.saturating_mul(desc.opts().slots_per_leader_window);

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, SlotIndex::new(0), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(0), skip_cert).expect("should not error");

    assert_eq!(state.get_first_non_finalized_slot(), SlotIndex::new(0));
    assert_eq!(state.get_first_non_progressed_slot(), SlotIndex::new(1));
    assert_eq!(state.max_acceptable_slot(), SlotIndex::new(1 + max_future_slots));
    assert!(!state.is_slot_too_far_ahead(SlotIndex::new(1 + max_future_slots)));
    assert!(state.is_slot_too_far_ahead(SlotIndex::new(2 + max_future_slots)));
}

#[test]
fn test_vote_bound_uses_progress_cursor_after_skip() {
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");
    let expected_first_too_new = SlotIndex::new(
        ((1 / desc.opts().slots_per_leader_window) + desc.opts().max_leader_window_desync + 1)
            * desc.opts().slots_per_leader_window,
    );

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];
    let skip_cert = create_test_skip_cert(&desc, SlotIndex::new(0), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(0), skip_cert).expect("should not error");

    assert_eq!(state.get_first_non_finalized_slot(), SlotIndex::new(0));
    assert_eq!(state.get_first_non_progressed_slot(), SlotIndex::new(1));
    assert_eq!(state.first_too_new_vote_slot(), expected_first_too_new);
    assert!(!state.is_vote_slot_too_far_ahead(expected_first_too_new - 1));
    assert!(state.is_vote_slot_too_far_ahead(expected_first_too_new));
}

#[test]
fn test_standstill_slot_grid_dump_empty_state() {
    let desc = create_test_desc(4, 2);
    let state = SimplexState::new(&desc, opts_cpp()).expect("create");

    let grid = state.standstill_slot_grid_dump(&desc);

    // With 4 validators and 2 slots per window (window 0 created), expect 2 slot lines
    let lines: Vec<&str> = grid.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 slot lines, got {}", lines.len());
    assert_eq!(lines[0], "0: ....");
    assert_eq!(lines[1], "1: ....");
}

#[test]
fn test_standstill_slot_grid_dump_with_votes() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");

    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([1u8; 32]),
        UInt256::from([2u8; 32]),
    );
    let candidate_hash = UInt256::from([0xAAu8; 32]);
    let candidate = create_test_candidate(0, candidate_hash.clone(), block_id, None, 0);
    state.on_candidate(&desc, candidate).unwrap();

    // Validator 0 notarizes
    let notar_vote = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(0), notar_vote, vec![0x10]).unwrap();

    // Validator 1 skips
    let skip_vote = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip_vote, vec![0x20]).unwrap();

    // Validator 2 notarizes AND skips → 'I'
    let notar_vote2 = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(0),
        block_hash: candidate_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(2), notar_vote2, vec![0x30]).unwrap();
    let skip_vote2 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip_vote2, vec![0x31]).unwrap();

    let grid = state.standstill_slot_grid_dump(&desc);
    let lines: Vec<&str> = grid.lines().collect();
    assert_eq!(lines.len(), 2);

    // Slot 0: v0=N, v1=S, v2=I (notarize+skip), v3=.
    assert_eq!(lines[0], "0: NSI.");
    // Slot 1: no votes
    assert_eq!(lines[1], "1: ....");
}

#[test]
fn test_standstill_slot_grid_dump_with_certs() {
    let desc = create_test_desc_weights(5, 2, vec![1, 1, 1, 1, 1]);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");

    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([1u8; 32]),
        UInt256::from([2u8; 32]),
    );
    let candidate_hash = UInt256::from([0xBBu8; 32]);
    let candidate = create_test_candidate(0, candidate_hash.clone(), block_id, None, 0);
    state.on_candidate(&desc, candidate).unwrap();

    // 4 out of 5 validators notarize → exceeds 2/3 threshold → notar cert created
    for v in 0..4 {
        let vote = Vote::Notarize(NotarizeVote {
            slot: SlotIndex::new(0),
            block_hash: candidate_hash.clone(),
        });
        state.on_vote_test(&desc, ValidatorIndex::new(v), vote, vec![v as u8]).unwrap();
    }

    // Drain events to prevent confusion
    while state.has_pending_events() {
        let _ = state.pull_event();
    }

    let grid = state.standstill_slot_grid_dump(&desc);
    let lines: Vec<&str> = grid.lines().collect();
    assert!(lines.len() >= 1);
    // Slot 0 should have 4 N's, 1 dot, and " notar" cert flag
    assert!(lines[0].contains("notar"), "Expected notar cert flag in: {}", lines[0]);
    assert!(lines[0].starts_with("0: NNNN."), "Expected NNNN. in: {}", lines[0]);
}

#[test]
fn test_standstill_diagnostic_dump_includes_last_final_cert_summary() {
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("create");

    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([3u8; 32]),
        UInt256::from([4u8; 32]),
    );
    let candidate_hash = UInt256::from([0xCCu8; 32]);
    let candidate = create_test_candidate(0, candidate_hash.clone(), block_id, None, 0);
    state.on_candidate(&desc, candidate).unwrap();

    for v in 0..3 {
        let notar_vote = Vote::Notarize(NotarizeVote {
            slot: SlotIndex::new(0),
            block_hash: candidate_hash.clone(),
        });
        state.on_vote_test(&desc, ValidatorIndex::new(v), notar_vote, vec![v as u8]).unwrap();
    }
    for v in 0..3 {
        let finalize_vote = Vote::Finalize(FinalizeVote {
            slot: SlotIndex::new(0),
            block_hash: candidate_hash.clone(),
        });
        state
            .on_vote_test(&desc, ValidatorIndex::new(v), finalize_vote, vec![10 + v as u8])
            .unwrap();
    }

    while state.has_pending_events() {
        let _ = state.pull_event();
    }

    let dump = state.standstill_diagnostic_dump(&desc);
    assert!(
        dump.contains("Last final cert is for slot=0"),
        "expected last-final summary in diagnostic dump: {dump}"
    );
    assert!(
        dump.contains(&candidate_hash.to_hex_string()),
        "expected final-cert hash in diagnostic dump: {dump}"
    );
    assert!(
        dump.lines().any(|line| line.starts_with("1: ")),
        "expected slot-grid line in diagnostic dump: {dump}"
    );
}

#[test]
fn test_available_base_max_merge_keeps_higher_slot() {
    // When two propagations compete for the same target slot, max-merge must
    // keep the higher parent (slot first, then hash), mirroring C++ ordering.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_notarized_parent_chain()).expect("create");

    let h0 = UInt256::from([0xB0u8; 32]);
    let h1 = UInt256::from([0xB1u8; 32]);

    // Notarize slot 0 (3 votes → notar cert)
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // slot1.available_base = id(slot0, h0) — set by notarization
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert_eq!(
        w0.slots[1].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0.clone() })),
    );

    // Notarize slot 1 (propagates slot1's id to slot2)
    let vote1 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: h1.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), vec![4]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), vec![5]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, vec![6]).unwrap();

    // slot2.available_base = id(slot1, h1) — the higher parent
    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(1), hash: h1.clone() })),
    );

    // Skip slot 1 so a late duplicate notarization for slot 0 now targets slot 2.
    // This creates direct competition for the same target base:
    // - existing slot2 base: id(slot1, h1) (higher)
    // - late propagation:    id(slot0, h0) (lower)
    let skip1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip1.clone(), vec![7]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip1.clone(), vec![8]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip1, vec![9]).unwrap();

    // Late duplicate notarization for slot 0 should not regress slot2 base.
    state.propagate_base_after_notarization(
        &desc,
        crate::block::CandidateParentInfo { slot: SlotIndex::new(0), hash: h0 },
    );

    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(crate::block::CandidateParentInfo { slot: SlotIndex::new(1), hash: h1 })),
        "max-merge must keep the higher-slot parent, not regress to slot 0"
    );
}

#[test]
fn test_available_base_skip_propagates_max_merge() {
    // Skip-propagation must max-merge into target slot:
    // if target has lower base and skipped slot has higher base, target must upgrade.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_notarized_parent_chain()).expect("create");

    let h_low = UInt256::from([0xC0u8; 32]);
    let h_high = UInt256::from([0xC1u8; 32]);
    let low_base = crate::block::CandidateParentInfo { slot: SlotIndex::new(1), hash: h_low };
    let high_base = crate::block::CandidateParentInfo { slot: SlotIndex::new(2), hash: h_high };

    // Seed direct competing bases:
    // - slot1 (to be skipped) has higher base
    // - slot2 (target) already has lower base
    state.get_slot_mut(&desc, SlotIndex::new(1)).expect("slot1 exists").available_base =
        Some(Some(high_base.clone()));
    state.get_slot_mut(&desc, SlotIndex::new(2)).expect("slot2 exists").available_base =
        Some(Some(low_base));

    // After skip-cert propagation from slot1 -> slot2, max-merge must upgrade slot2.
    state.propagate_base_after_skip_cert(&desc, SlotIndex::new(1));

    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].available_base,
        Some(Some(high_base)),
        "skip-propagation max-merge must upgrade to the higher-slot parent"
    );
}

// ==========================================================================
// Stale window guard tests
// ==========================================================================

#[test]
fn test_stale_window_guard_current_leader_window_idx_updated_before_collation_check() {
    // Verifies the ordering guarantee for leader window state:
    // `current_leader_window_idx` must be up-to-date after notarization advances
    // the progress cursor across a window boundary, BEFORE any code can check
    // leader status (the stale-window guard in SessionProcessor::check_collation
    // compares slot_window vs current_leader_window_idx).
    //
    // Setup: 4 validators, 2 slots per window.
    // Progress both slots in window 0 via notarization -> cursor crosses to window 1.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(0));

    // Notarize slot 0 (3 out of 4 validators -> quorum)
    let h0 = UInt256::from([0xD0u8; 32]);
    let vote0 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: h0.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote0, vec![3]).unwrap();

    // Slot 0 notarized -> cursor at slot 1, still in window 0
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(0),
        "window must not advance until full window is progressed"
    );

    // Notarize slot 1 (3 out of 4)
    let h1 = UInt256::from([0xD1u8; 32]);
    let vote1 = Vote::Notarize(NotarizeVote { slot: SlotIndex::new(1), block_hash: h1.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote1.clone(), vec![4]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote1.clone(), vec![5]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote1, vec![6]).unwrap();

    // Both slots in window 0 are notarized -> cursor crosses to slot 2 (window 1)
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "current_leader_window_idx must advance when progress cursor crosses window boundary"
    );

    // The stale-window guard: slot 0 is in window 0, but current window is 1.
    // SessionProcessor::check_collation would see slot_window(0) < current_window(1) -> skip.
    let slot0_window = desc.get_window_idx(SlotIndex::new(0));
    assert!(
        slot0_window < state.current_leader_window_idx,
        "slot 0 (window {slot0_window}) must be stale relative to current window {}",
        state.current_leader_window_idx
    );

    // Slot 2 is in the current window -> not stale
    let slot2_window = desc.get_window_idx(SlotIndex::new(2));
    assert_eq!(
        slot2_window, state.current_leader_window_idx,
        "slot 2 must be in the current window"
    );
}

#[test]
fn test_stale_window_guard_skip_also_advances_window() {
    // Same as above but using skip votes instead of notarization.
    // Window advancement via skips must also update current_leader_window_idx.
    let desc = create_test_desc(4, 2);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Skip slot 0 (3 out of 4 validators)
    let skip0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip0, vec![3]).unwrap();

    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));

    // Skip slot 1 (3 out of 4)
    let skip1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip1.clone(), vec![4]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip1.clone(), vec![5]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip1, vec![6]).unwrap();

    // Both slots in window 0 skipped -> cursor at slot 2, window must advance
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "current_leader_window_idx must advance after full-window skip"
    );
}

/*
    ========================================================================
    C++ parity: candidate pending storage despite local skip vote

    Regression tests for the fix to on_candidate() where candidates were
    permanently dropped after a local skip vote. C++ consensus.cpp only
    gates on voted_notar — a skip vote must NOT prevent storing a candidate
    as pending_block for later retry via check_pending_blocks.
    ========================================================================
*/

#[test]
fn test_candidate_stored_as_pending_despite_skip_vote_cpp_mode() {
    // A local skip vote must NOT prevent storing a candidate as pending_block
    // when try_notar fails (base not propagated yet).
    // Reference: C++ consensus.cpp CandidateReceived only checks voted_notar.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Cast local skip for all of window 1 (slots 4-7).
    state.try_skip_window(WindowIndex::new(1));
    drain_events(&mut state);

    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert!(w1.slots[0].voted_skip, "voted_skip must be set for slot 4");
    assert!(w1.slots[0].voted_notar.is_none(), "voted_notar must NOT be set");
    assert!(
        w1.slots[0].available_base.is_none(),
        "available_base for slot 4 must be None (not propagated)"
    );

    // Submit candidate for slot 4 with genesis parent
    let hash4 = UInt256::from([0xAA; 32]);
    let candidate = create_test_candidate(4, hash4, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(_)))),
        "must NOT broadcast NotarVote — base not propagated yet, got: {:?}",
        events
    );

    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert!(
        w1.slots[0].pending_block.is_some(),
        "candidate must be stored as pending_block despite local skip vote (C++ parity)"
    );
}

#[test]
fn test_cpp_mode_try_skip_window_preserves_existing_pending_block() {
    // Regression: in C++ mode, Skip must NOT drop an already buffered candidate.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let parent_hash = UInt256::from([0x91; 32]);
    let child_hash = UInt256::from([0x92; 32]);
    let candidate =
        create_test_candidate(1, child_hash, BlockIdExt::default(), Some((0, parent_hash)), 0);
    state.on_candidate(&desc, candidate).unwrap();

    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "precondition: slot 1 candidate must be pending before skip"
    );

    state.try_skip_window(WindowIndex::new(0));

    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "C++ mode must preserve pending_block on skip"
    );
}

#[test]
fn test_cpp_mode_restart_skip_paths_preserve_existing_pending_block() {
    // Regression: restart skip paths in C++ mode must preserve pending candidates.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    let parent_hash = UInt256::from([0xA1; 32]);
    let child_hash = UInt256::from([0xA2; 32]);
    let candidate =
        create_test_candidate(1, child_hash, BlockIdExt::default(), Some((0, parent_hash)), 0);
    state.on_candidate(&desc, candidate).unwrap();

    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "precondition: slot 1 candidate must be pending before restart skips"
    );

    // 1) Direct restart-skip replay
    state.mark_slot_voted_on_restart(&desc, &Vote::Skip(SkipVote { slot: SlotIndex::new(1) }));
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "mark_slot_voted_on_restart(skip) must preserve pending_block in C++ mode"
    );

    // 2) Startup-generated restart skips for previous window [0,1]
    let _ = state.generate_restart_skip_votes(WindowIndex::new(1), 2);
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "generate_restart_skip_votes must preserve pending_block in C++ mode"
    );
}

#[test]
fn test_cold_start_delayed_parent_recovery_notarizes_pending_cpp_mode() {
    // Regression scenario:
    // - cold startup delay before first active tick
    // - candidate buffered while parent/state is unavailable
    // - later parent availability must notarize buffered candidate (no deadlock)
    let desc = create_test_desc(4, 2);
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    desc.set_time(base_time);

    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");
    assert!(state.get_next_timeout().is_none(), "constructor path must not arm startup timeout");

    // Candidate for slot 1 depends on slot 0 parent that is not available yet.
    let parent_hash = UInt256::from([0xB1; 32]);
    let child_hash = UInt256::from([0xB2; 32]);
    let candidate = create_test_candidate(
        1,
        child_hash.clone(),
        BlockIdExt::default(),
        Some((0, parent_hash.clone())),
        0,
    );
    state.on_candidate(&desc, candidate).unwrap();
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some(),
        "candidate should be buffered while parent/state is unavailable"
    );

    // Simulate long cold-start delay before the first active processing tick.
    desc.set_time(base_time + Duration::from_secs(120));
    state.reset_timeouts_on_start(&desc);
    state.check_all(&desc);

    // Timeout must be anchored at startup readiness, so there is no immediate skip storm.
    let early_events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !early_events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Skip(_)))),
        "must not emit immediate SkipVote right after startup timeout reset"
    );

    // Delayed parent availability: notarization for slot 0 arrives later.
    let notarize_slot0 =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: parent_hash });
    state.on_vote_test(&desc, ValidatorIndex::new(0), notarize_slot0.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), notarize_slot0.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), notarize_slot0, vec![3]).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(
            e,
            SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, block_hash }))
                if *slot == SlotIndex::new(1) && *block_hash == child_hash
        )),
        "pending child candidate must be retried and notarized after parent becomes available"
    );
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_none(),
        "pending_block must be cleared after successful notarization"
    );
}

#[test]
fn test_alpenglow_mode_skip_clears_existing_pending_block() {
    // Guardrail: fallback/Alpenglow mode keeps pendingBlocks[k] <- ⊥ on skip.
    let desc = create_test_desc(4, 2);
    let mut state = SimplexState::new(&desc, opts_alpenglow()).expect("Failed to create state");

    let parent_hash = UInt256::from([0xC1; 32]);
    let child_hash = UInt256::from([0xC2; 32]);
    let candidate =
        create_test_candidate(1, child_hash, BlockIdExt::default(), Some((0, parent_hash)), 0);
    state.on_candidate(&desc, candidate).unwrap();
    assert!(state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_some());

    state.try_skip_window(WindowIndex::new(0));
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[1].pending_block.is_none(),
        "Alpenglow mode must clear pending_block on skip"
    );
}

#[test]
fn test_pending_block_notarized_after_base_propagates_via_skip_certs() {
    // Full lifecycle: candidate stored as pending after skip vote, then notarized
    // when skip certs propagate the genesis base through to the candidate's slot.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Cast local skip for window 1 (slots 4-7)
    state.try_skip_window(WindowIndex::new(1));
    drain_events(&mut state);

    // Store candidate at slot 4 (pending — base not propagated)
    let hash4 = UInt256::from([0xBB; 32]);
    let candidate = create_test_candidate(4, hash4, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate).unwrap();
    drain_events(&mut state);

    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_some(),
        "precondition: candidate stored as pending"
    );

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Issue skip certs for s0, s1, s2, s3 — each propagates genesis base one hop forward
    for s in 0..4u32 {
        let skip_cert = create_test_skip_cert(&desc, SlotIndex::new(s), &signers);
        state.set_skip_certificate(&desc, SlotIndex::new(s), skip_cert).unwrap();
    }

    // After all 4 skip certs, genesis base should have reached slot 4.
    // check_pending_blocks (called by propagate_base_after_skip_cert) must
    // retry the pending candidate → try_notar succeeds → NotarVote emitted.
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if *slot == SlotIndex::new(4))
        ),
        "must emit NotarVote for pending candidate at slot 4 after base propagates, got: {:?}",
        events
    );

    // Pending block should be cleared after successful notarization
    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_none(),
        "pending_block must be cleared after notarization"
    );
}

#[test]
fn test_candidate_dropped_when_voted_notar_cpp_mode() {
    // When voted_notar is already set for a slot, a second candidate with a different
    // hash must be correctly dropped (not stored as pending).
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    // Slot 0 has genesis base → first candidate succeeds immediately
    let h1 = UInt256::from([0x11; 32]);
    let candidate1 = create_test_candidate(0, h1.clone(), BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate1).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { block_hash, .. })) if *block_hash == h1)
        ),
        "first candidate must trigger NotarVote"
    );

    // Now send a second candidate with a different hash for the same slot
    let h2 = UInt256::from([0x22; 32]);
    let candidate2 = create_test_candidate(0, h2, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate2).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(_)))),
        "second candidate must NOT trigger NotarVote (voted_notar already set)"
    );

    // Candidate must NOT be stored as pending — voted_notar gates it
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(
        w0.slots[0].pending_block.is_none(),
        "candidate must NOT be stored as pending when voted_notar is set"
    );
}

#[test]
fn test_out_of_order_skip_certs_still_propagate_base_to_pending() {
    // Out-of-order skip cert arrival: s3 arrives first but has no base, so
    // nothing propagates. Later s0, s1, s2 arrive in order — when s2 is
    // processed, find_next_nonskipped_slot skips over s3 (already marked
    // skipped) and propagates genesis base directly to s4.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Cast local skip for window 1 (slots 4-7)
    state.try_skip_window(WindowIndex::new(1));
    drain_events(&mut state);

    // Store candidate at slot 4 (pending — no base)
    let hash4 = UInt256::from([0xCC; 32]);
    let candidate = create_test_candidate(4, hash4, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate).unwrap();
    drain_events(&mut state);

    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_some(),
        "precondition: candidate stored as pending"
    );

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Issue skip cert for s3 FIRST (out of order)
    let skip3 = create_test_skip_cert(&desc, SlotIndex::new(3), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(3), skip3).unwrap();

    // s3 has no base → nothing propagates → no vote yet
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        !events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if *slot == SlotIndex::new(4))
        ),
        "no NotarVote yet — s3 had no base to propagate"
    );

    // Issue skip certs for s0, s1
    for s in 0..2u32 {
        let skip = create_test_skip_cert(&desc, SlotIndex::new(s), &signers);
        state.set_skip_certificate(&desc, SlotIndex::new(s), skip).unwrap();
        drain_events(&mut state);
    }

    // Verify slot 4 still has no base (propagated to s2 only so far)
    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_some(),
        "candidate still pending after s0+s1 skip certs"
    );

    // Issue skip cert for s2 — propagation chain: s2 skipped, find_next_nonskipped(s2)
    // skips over s3 (already skipped) → lands on s4 → base arrives → pending block retried
    let skip2 = create_test_skip_cert(&desc, SlotIndex::new(2), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(2), skip2).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if *slot == SlotIndex::new(4))
        ),
        "must emit NotarVote for slot 4 after out-of-order skip certs propagate base, got: {:?}",
        events
    );

    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_none(),
        "pending_block must be cleared after successful notarization"
    );
}

/*
    ========================================================================
    Base propagation chaining through already-skipped slots

    When skip certs arrive out of order, `propagate_base_after_skip_cert`
    must chain the base forward through all consecutive already-skipped
    intermediate slots. Without this, the base jumps from the cert's slot
    to the first non-skipped slot, leaving intermediate slots baseless
    and pending blocks stuck forever (no backward-walk like C++ has).
    ========================================================================
*/

#[test]
fn test_base_chains_through_already_skipped_slots() {
    // Scenario: skip certs for slots 1-6 arrive BEFORE slot 0's cert.
    // When slot 0's cert is finally processed, the chaining loop must
    // propagate the genesis base through slots 1→2→3→4→5→6→7.
    let desc = create_test_desc(4, 8); // 4 validators, 8 slots/window
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Issue skip certs for slots 1-6 first (out of order — slot 0 last)
    for s in 1..=6u32 {
        let cert = create_test_skip_cert(&desc, SlotIndex::new(s), &signers);
        state.set_skip_certificate(&desc, SlotIndex::new(s), cert).unwrap();
    }
    drain_events(&mut state);

    // Verify: slots 1-6 are skipped but have no available_base (no source yet)
    for s in 1..=6u32 {
        let base = state.get_slot_ref(&desc, SlotIndex::new(s)).unwrap().available_base.clone();
        assert!(
            base.is_none(),
            "slot {} should have no base before slot 0's cert chains through",
            s
        );
    }

    // Now issue skip cert for slot 0 — triggers chaining through 1→2→3→4→5→6→7
    let cert0 = create_test_skip_cert(&desc, SlotIndex::new(0), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(0), cert0).unwrap();
    drain_events(&mut state);

    // Every intermediate skipped slot must now have the genesis base
    for s in 1..=6u32 {
        let base = state.get_slot_ref(&desc, SlotIndex::new(s)).unwrap().available_base.clone();
        assert_eq!(
            base,
            Some(None), // genesis
            "slot {} must have genesis base after chaining from slot 0",
            s
        );
    }

    // Slot 7 (first non-skipped after the chain) must also have the base
    let base7 = state.get_slot_ref(&desc, SlotIndex::new(7)).unwrap().available_base.clone();
    assert_eq!(base7, Some(None), "slot 7 (first non-skipped) must have genesis base");
}

#[test]
fn test_base_chaining_enables_pending_block_at_intermediate_skipped_slot() {
    // Regression test for the real-network failure mode:
    // A pending block sits at a slot whose skip cert arrived before the base
    // propagated. The old code would never set `available_base` on that slot
    // because `find_next_nonskipped_slot` jumped past it. The chaining fix
    // ensures the base reaches it.
    let desc = create_test_desc(4, 8);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let signers = vec![ValidatorIndex::new(0), ValidatorIndex::new(1), ValidatorIndex::new(2)];

    // Skip-vote slot 4 locally so candidate can be stored as pending
    state.try_skip_window(WindowIndex::new(0));
    drain_events(&mut state);

    // Store a pending candidate at slot 4 (parent = genesis)
    let hash4 = UInt256::from([0xDD; 32]);
    let candidate = create_test_candidate(4, hash4, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate).unwrap();
    drain_events(&mut state);

    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[4].pending_block.is_some(),
        "precondition: candidate stored as pending at slot 4"
    );

    // Skip certs for slots 1-3 arrive BEFORE slot 0.
    // Slot 4 doesn't get a skip cert (it only has a local skip vote).
    for s in 1..=3u32 {
        let cert = create_test_skip_cert(&desc, SlotIndex::new(s), &signers);
        state.set_skip_certificate(&desc, SlotIndex::new(s), cert).unwrap();
    }
    drain_events(&mut state);

    // Verify no notarize vote yet — slot 4 still has no base
    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[4].pending_block.is_some(),
        "candidate still pending — base hasn't reached slot 4 yet"
    );

    // Now process slot 0's skip cert → chain: 0→1→2→3→4 (slot 4 not skipped-cert)
    let cert0 = create_test_skip_cert(&desc, SlotIndex::new(0), &signers);
    state.set_skip_certificate(&desc, SlotIndex::new(0), cert0).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if *slot == SlotIndex::new(4))
        ),
        "must emit NotarVote for pending slot 4 after base chains through, got: {:?}",
        events
    );

    assert!(
        state.get_window(WindowIndex::new(0)).unwrap().slots[4].pending_block.is_none(),
        "pending_block must be cleared after notarization"
    );
}

#[test]
fn test_pending_block_not_overwritten_by_second_candidate_cpp_mode() {
    // C++ parity: first pending candidate wins. A second candidate with a different
    // hash for the same slot must be rejected (equivocation), keeping the original.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Cast local skip for window 1 (slots 4-7) so candidates go to pending
    state.try_skip_window(WindowIndex::new(1));
    drain_events(&mut state);

    // Store candidate A at slot 4 as pending (no base → try_notar fails)
    let hash_a = UInt256::from([0xAA; 32]);
    let candidate_a = create_test_candidate(4, hash_a.clone(), BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate_a).unwrap();
    drain_events(&mut state);

    assert!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0].pending_block.is_some(),
        "precondition: candidate A stored as pending"
    );
    assert_eq!(
        state.get_window(WindowIndex::new(1)).unwrap().slots[0]
            .pending_block
            .as_ref()
            .unwrap()
            .id
            .hash,
        hash_a,
        "precondition: pending_block is candidate A"
    );

    let pending_count_before = state.pending_slots.len();

    // Submit candidate B with a different hash for the same slot 4
    let hash_b = UInt256::from([0xBB; 32]);
    let candidate_b = create_test_candidate(4, hash_b, BlockIdExt::default(), None, 1);
    state.on_candidate(&desc, candidate_b).unwrap();
    drain_events(&mut state);

    // pending_block must still hold candidate A (not B)
    let w1 = state.get_window(WindowIndex::new(1)).unwrap();
    assert_eq!(
        w1.slots[0].pending_block.as_ref().unwrap().id.hash,
        hash_a,
        "pending_block must still be candidate A — first candidate wins"
    );

    // No additional PendingSlot should have been pushed
    assert_eq!(
        state.pending_slots.len(),
        pending_count_before,
        "no additional PendingSlot should be pushed for duplicate/equivocating candidate"
    );
}

#[test]
fn test_try_notar_not_blocked_by_its_over_after_finalize_restart_cpp_mode() {
    // C++ parity: after restart with a persisted Finalize vote, its_over=true and
    // voted_final=true are set, but voted_notar remains None. C++ try_notarize()
    // does NOT check voted_final, so notarization must still proceed.
    let desc = create_test_desc(4, 1);
    let mut state = SimplexState::new(&desc, opts_cpp()).expect("Failed to create state");

    // Simulate restart recovery: mark slot 0 as having a persisted Finalize vote
    let finalize_vote = Vote::Finalize(FinalizeVote {
        slot: SlotIndex::new(0),
        block_hash: UInt256::from([0xFF; 32]),
    });
    state.mark_slot_voted_on_restart(&desc, &finalize_vote);

    // Verify preconditions
    let w0 = state.get_window(WindowIndex::new(0)).unwrap();
    assert!(w0.slots[0].its_over, "precondition: its_over must be true after Finalize restart");
    assert!(
        w0.slots[0].voted_final,
        "precondition: voted_final must be true after Finalize restart"
    );
    assert!(
        w0.slots[0].voted_notar.is_none(),
        "precondition: voted_notar must be None (Finalize does not set it)"
    );

    // Submit candidate for slot 0 (has genesis base → should succeed)
    let hash = UInt256::from([0xCC; 32]);
    let candidate = create_test_candidate(0, hash, BlockIdExt::default(), None, 0);
    state.on_candidate(&desc, candidate).unwrap();

    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(
            |e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(NotarizeVote { slot, .. })) if *slot == SlotIndex::new(0))
        ),
        "must emit NotarVote for slot 0 — its_over must NOT block try_notar in C++ mode, got: {:?}",
        events
    );
}

#[test]
fn test_notarized_parent_chain_genesis_base_propagates_across_skipped_windows() {
    // Regression test for bootstrap deadlock: in the default C++-parity progression path,
    // skipping an entire window must propagate the available base to the
    // next window via advance_leader_window_on_progress_cursor().
    //
    // Without the fix, advance_leader_window_on_progress_cursor() only advanced the window
    // index and set timeouts but never populated the new window's available_bases, causing
    // has_available_parent() to return false and blocking all collation permanently.
    //
    // Reference: C++ pool.cpp advance_present() reads slot_at(now_)->state->available_base
    // and publishes it via LeaderWindowObserved(now_, base).
    let desc = create_test_desc(4, 2); // 2 slots per window
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // Window 0 starts with genesis base
    assert!(state.has_available_parent(&desc, SlotIndex::new(0)));
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));

    // Skip slot 0 (need 3 out of 4 for threshold_66)
    let skip_vote_0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip_vote_0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip_vote_0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip_vote_0, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(1));

    // Skip slot 1 (last slot in window 0) -> should trigger window advancement
    let skip_vote_1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip_vote_1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip_vote_1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip_vote_1, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    // Progress cursor should be at slot 2 (start of window 1)
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));

    // Window must have advanced to window 1
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "leader window must advance to window 1 after all window 0 slots skipped"
    );

    // Window 1's available_bases must contain the genesis base (None)
    let w1 = state.get_window(WindowIndex::new(1));
    assert!(w1.is_some(), "window 1 must exist");
    assert!(
        w1.unwrap().available_bases.contains(&None),
        "window 1 must have genesis (None) base propagated from window 0 via \
        advance_leader_window_on_progress_cursor(). Got: {:?}",
        w1.unwrap().available_bases
    );

    // Slot 2 (first slot of window 1) must have available_base set
    let slot2_base = state.get_slot_available_base(&desc, SlotIndex::new(2));
    assert_eq!(slot2_base, Some(None), "slot 2 available_base must be genesis (Some(None))");

    // has_available_parent must return true for collation to proceed
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(2)),
        "has_available_parent must be true for slot 2 after genesis base propagated"
    );

    // get_available_parent must return None (genesis = no parent info)
    let parent = state.get_available_parent(&desc, SlotIndex::new(2));
    assert_eq!(parent, None, "genesis parent should return None (no parent id)");
}

#[test]
fn test_notarized_parent_chain_base_propagates_across_multiple_skipped_windows() {
    // Verify that base propagation works across multiple consecutive skipped windows.
    // This is the sustained stall scenario: window 0 -> 1 -> 2 all skip without finalization.
    let desc = create_test_desc(4, 1); // 1 slot per window for simplicity
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    assert!(state.has_available_parent(&desc, SlotIndex::new(0)));
    assert_eq!(state.current_leader_window_idx, WindowIndex::new(0));

    // Skip window 0 (slot 0)
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    assert_eq!(state.current_leader_window_idx, WindowIndex::new(1));
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(1)),
        "window 1 must have available parent after window 0 skipped"
    );

    // Skip window 1 (slot 1)
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    assert_eq!(state.current_leader_window_idx, WindowIndex::new(2));
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(2)),
        "window 2 must have available parent after windows 0+1 skipped"
    );

    // Skip window 2 (slot 2)
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(2) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    assert_eq!(state.current_leader_window_idx, WindowIndex::new(3));
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(3)),
        "window 3 must have available parent after windows 0+1+2 all skipped"
    );
}

// =========================================================================
// Fixed-base deadline tests (C++ timeout_base_ parity)
//
// C++ consensus.cpp stores a fixed per-window timeout_base_ and computes
// all slot deadlines as:  timeout_base + (offset) * target_rate.
// These tests verify that Rust reproduces the exact same schedule.
// =========================================================================

#[test]
fn test_set_timeouts_arms_timeout_base() {
    // set_timeouts must set timeout_base = now + first_block_timeout
    // and skip_timestamp = timeout_base + target_rate.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let first_block = desc.opts().first_block_timeout; // 3s default
    let target_rate = desc.opts().target_rate; // 1s default

    assert_eq!(
        state.timeout_base,
        Some(t0 + first_block),
        "timeout_base must be t0 + first_block_timeout"
    );
    assert_eq!(
        state.skip_timestamp,
        Some(t0 + first_block + target_rate),
        "skip_timestamp must be timeout_base + target_rate"
    );
    assert_eq!(state.skip_slot, SlotIndex::new(0));
}

#[test]
fn test_notarization_rearm_uses_fixed_base_not_sliding() {
    // Concrete scenario from C++ parity analysis:
    // first_block_timeout=3s, target_rate=1s, 4 slots per window.
    //
    // Window starts at t0:
    //   timeout_base = t0 + 3s
    //   slot 0 deadline = t0 + 4s  (base + 1*rate)
    //
    // Slot 0 notarizes "early" at t0 + 2s:
    //   C++ deadline for slot 1 = base + 2*rate = t0 + 5s  (anchored to base)
    //   Old Rust would give:      max(t0+4, t0+3) = t0 + 4s  (sliding from now)
    //
    // After fix, Rust must produce the C++ answer: t0 + 5s.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let first_block = desc.opts().first_block_timeout; // 3s
    let target_rate = desc.opts().target_rate; // 1s

    // Advance to t0+2s and notarize slot 0 (3 out of 4 validators)
    desc.set_time(t0 + Duration::from_secs(2));
    let block_hash = UInt256::from_slice(&[0xAA; 32]);
    let vote =
        Vote::Notarize(NotarizeVote { slot: SlotIndex::new(0), block_hash: block_hash.clone() });
    state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();
    while state.pull_event().is_some() {}

    // skip_slot should advance to 1 (watching slot 1 now)
    assert_eq!(
        state.skip_slot,
        SlotIndex::new(1),
        "skip_slot must advance to 1 after notarization"
    );

    // C++ formula: alarm = timeout_base + (timeout_slot - window_start) * target_rate
    // timeout_slot = slot+2 = 2, window_start = 0 → offset = 2
    // deadline = (t0+3) + 2*1 = t0+5
    let expected_deadline = t0 + first_block + target_rate * 2;
    assert_eq!(
        state.skip_timestamp,
        Some(expected_deadline),
        "deadline must be anchored to timeout_base, not to 'now' (expected t0+5s, NOT t0+3s)"
    );

    // timeout_base must NOT change on notarization
    assert_eq!(
        state.timeout_base,
        Some(t0 + first_block),
        "timeout_base must remain fixed within the window"
    );
}

#[test]
fn test_notarization_rearm_successive_slots() {
    // Notarize slots 0, 1, 2 in rapid succession — deadlines must follow the
    // fixed schedule: base+2*rate, base+3*rate, base+4*rate.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let first_block = desc.opts().first_block_timeout;
    let target_rate = desc.opts().target_rate;
    let base = t0 + first_block;

    for slot_num in 0u32..3 {
        desc.set_time(t0 + Duration::from_millis(500 * (slot_num as u64 + 1)));
        let hash = UInt256::from_slice(&[slot_num as u8 + 1; 32]);

        let parent = if slot_num == 0 {
            None
        } else {
            Some((slot_num - 1, UInt256::from_slice(&[slot_num as u8; 32])))
        };
        let candidate =
            create_test_candidate(slot_num, hash.clone(), BlockIdExt::default(), parent, 0);
        let _ = state.on_candidate(&desc, candidate);
        while state.pull_event().is_some() {}

        let vote =
            Vote::Notarize(NotarizeVote { slot: SlotIndex::new(slot_num), block_hash: hash });
        state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();
        while state.pull_event().is_some() {}

        // C++ timeout_slot_ = slot+2 (non-end-of-window) → offset = slot+2
        let expected = base + target_rate * (slot_num + 2);
        assert_eq!(
            state.skip_timestamp,
            Some(expected),
            "slot {} deadline must be base + {}*rate",
            slot_num,
            slot_num + 2
        );
    }
}

#[test]
fn test_notarization_window_end_transitions_to_new_window() {
    // When the last slot of a window is notarized, the progress cursor crosses
    // into the next window. C++ handles this via LeaderWindowObserved which
    // resets the timer. In Rust, advance_leader_window_on_progress_cursor →
    // set_timeouts re-arms with fresh timeout_base for the new window.
    //
    // The guard `skip_slot <= slot` (C++ parity) prevents the per-notarization
    // timer update from overwriting the freshly set window 1 schedule.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let target_rate = desc.opts().target_rate;

    // Notarize all 4 slots in window 0
    for slot_num in 0u32..4 {
        desc.set_time(t0 + Duration::from_millis(500 * (slot_num as u64 + 1)));
        let hash = UInt256::from_slice(&[slot_num as u8 + 1; 32]);

        let parent = if slot_num == 0 {
            None
        } else {
            Some((slot_num - 1, UInt256::from_slice(&[slot_num as u8; 32])))
        };
        let candidate =
            create_test_candidate(slot_num, hash.clone(), BlockIdExt::default(), parent, 0);
        let _ = state.on_candidate(&desc, candidate);
        while state.pull_event().is_some() {}

        let vote =
            Vote::Notarize(NotarizeVote { slot: SlotIndex::new(slot_num), block_hash: hash });
        state.on_vote_test(&desc, ValidatorIndex::new(0), vote.clone(), vec![1]).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(1), vote.clone(), vec![2]).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(2), vote, vec![3]).unwrap();
        while state.pull_event().is_some() {}
    }

    // Window transition should have occurred
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "window must advance to 1 after all slots notarized"
    );

    // set_timeouts for window 1 was called at t0+2s (time of last notarization).
    // No adaptive backoff because window 0 had no timeouts (had_timeouts=false).
    let t_last = t0 + Duration::from_millis(2000);
    let first_block = desc.opts().first_block_timeout; // restored to default (no backoff)
    let new_base = t_last + first_block;
    assert_eq!(state.timeout_base, Some(new_base), "timeout_base must be freshly set for window 1");
    assert_eq!(
        state.skip_timestamp,
        Some(new_base + target_rate),
        "skip_timestamp must be base + target_rate for window 1"
    );
    assert_eq!(state.skip_slot, SlotIndex::new(4), "skip_slot must be at window 1 start");
}

#[test]
fn test_skip_cert_does_not_move_timer() {
    // C++ does NOT touch the consensus alarm when a skip certificate arrives.
    // Skip certs flow through the pool layer only.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let first_block = desc.opts().first_block_timeout;
    let target_rate = desc.opts().target_rate;

    let skip_slot_before = state.skip_slot;
    let skip_ts_before = state.skip_timestamp;
    let base_before = state.timeout_base;

    assert_eq!(skip_slot_before, SlotIndex::new(0));
    assert_eq!(skip_ts_before, Some(t0 + first_block + target_rate));

    // Advance time and submit skip cert for slot 0 (3 out of 4)
    desc.set_time(t0 + Duration::from_millis(500));
    let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    // Timer state must be UNCHANGED (skip_slot, skip_timestamp, timeout_base)
    assert_eq!(state.skip_slot, skip_slot_before, "skip_slot must NOT advance on skip cert");
    assert_eq!(state.skip_timestamp, skip_ts_before, "skip_timestamp must NOT change on skip cert");
    assert_eq!(state.timeout_base, base_before, "timeout_base must NOT change on skip cert");
}

#[test]
fn test_window_skip_clears_timeout_base() {
    // When process_timeouts fires the C++ window-skip, both skip_timestamp
    // and timeout_base must be cleared (None).
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    assert!(state.timeout_base.is_some(), "base must be armed after start");

    // Advance well past the first deadline to trigger process_timeouts
    desc.set_time(t0 + Duration::from_secs(10));
    state.check_all(&desc);
    while state.pull_event().is_some() {}

    assert!(state.skip_timestamp.is_none(), "skip_timestamp must be None after C++ window-skip");
    assert!(state.timeout_base.is_none(), "timeout_base must be None after C++ window-skip");
    // skip_slot should be at window end (4)
    assert_eq!(
        state.skip_slot,
        SlotIndex::new(4),
        "skip_slot must be at window end after C++ window-skip"
    );
}

#[test]
fn test_new_window_rearms_timeout_base() {
    // After a window skip, when progress crosses into a new window,
    // advance_leader_window_on_progress_cursor → set_timeouts must
    // re-arm timeout_base with (possibly backed-off) first_block_timeout.
    let desc = create_test_desc(4, 4);
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    desc.set_time(t0);
    state.reset_timeouts_on_start(&desc);

    let target_rate = desc.opts().target_rate;
    let backoff_factor = desc.opts().timeout_increase_factor; // 1.05 default

    // Trigger timeout to skip window 0
    desc.set_time(t0 + Duration::from_secs(10));
    state.check_all(&desc);
    while state.pull_event().is_some() {}
    assert!(state.timeout_base.is_none(), "base cleared after window skip");

    // Now feed skip certs for all 4 slots (to let progress cursor cross window boundary)
    let t1 = t0 + Duration::from_secs(11);
    desc.set_time(t1);
    for slot_num in 0u32..4 {
        let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(slot_num) });
        state.on_vote_test(&desc, ValidatorIndex::new(0), skip.clone(), Vec::new()).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(1), skip.clone(), Vec::new()).unwrap();
        state.on_vote_test(&desc, ValidatorIndex::new(2), skip, Vec::new()).unwrap();
    }
    while state.pull_event().is_some() {}

    // Window 0 had timeouts (had_timeouts=true), so adaptive backoff applies:
    // first_block_timeout *= timeout_increase_factor (1.05)
    let backed_off_first_block = desc.opts().first_block_timeout.mul_f64(backoff_factor);

    // Progress cursor should have advanced past window 0, triggering
    // advance_leader_window_on_progress_cursor → set_timeouts for window 1
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "window must advance to 1 after skip certs"
    );
    assert_eq!(
        state.timeout_base,
        Some(t1 + backed_off_first_block),
        "timeout_base must be re-armed for new window (with backoff)"
    );
    assert_eq!(
        state.skip_timestamp,
        Some(t1 + backed_off_first_block + target_rate),
        "skip_timestamp must be armed for new window (with backoff)"
    );
    assert_eq!(state.skip_slot, SlotIndex::new(4), "skip_slot must be at start of window 1");
}

/// End-to-end: first leader absent -> full first-window skip -> second leader collates & notarizes.
///
/// Scenario (4 validators, 2 slots/window):
///   Window 0 (leader=v0): skip slot 0, skip slot 1 → window 0 fully skipped.
///   Window 1 (leader=v1): candidate at slot 2 with genesis parent → notarized by quorum.
///
/// This closes the test gap identified in the plan: existing tests verify base propagation
/// across skipped windows but do NOT assert that the second leader can successfully
/// submit a candidate and achieve notarization after the first window is entirely skipped.
#[test]
fn test_second_leader_collates_after_full_first_window_skip() {
    let desc = create_test_desc(4, 2); // 4 validators, 2 slots per window
    let mut state =
        SimplexState::new(&desc, opts_notarized_parent_chain()).expect("Failed to create state");

    // -- Skip entire window 0 (leader=v0 absent) --

    // Skip slot 0 (quorum = 3 out of 4)
    let skip0 = Vote::Skip(SkipVote { slot: SlotIndex::new(0) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip0.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip0, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    // Skip slot 1 (last slot in window 0)
    let skip1 = Vote::Skip(SkipVote { slot: SlotIndex::new(1) });
    state.on_vote_test(&desc, ValidatorIndex::new(0), skip1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(1), skip1.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), skip1, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    // Verify window advanced and second leader has an available parent
    assert_eq!(
        state.current_leader_window_idx,
        WindowIndex::new(1),
        "window must advance to 1 after full first-window skip"
    );
    assert_eq!(state.first_non_progressed_slot, SlotIndex::new(2));
    assert!(
        state.has_available_parent(&desc, SlotIndex::new(2)),
        "second leader must have available parent (genesis base propagated)"
    );

    // -- Second leader (v1) submits candidate for slot 2 with genesis parent --

    let block2 = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        1,
        UInt256::from([0x22; 32]),
        UInt256::from([0x33; 32]),
    );
    let candidate2 = create_test_candidate(2, UInt256::from([0x22; 32]), block2.clone(), None, 1);
    state.on_candidate(&desc, candidate2).expect("second leader candidate must be accepted");

    // Drain events — expect our own NotarVote to be broadcast
    let events: Vec<_> = from_fn(|| state.pull_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, SimplexEvent::BroadcastVote(Vote::Notarize(v))
            if v.slot == SlotIndex::new(2))),
        "our node must broadcast a NotarVote for slot 2 after second leader's candidate"
    );

    // -- Notarize slot 2 with quorum votes --

    let notar2 = Vote::Notarize(NotarizeVote {
        slot: SlotIndex::new(2),
        block_hash: block2.root_hash.clone(),
    });
    state.on_vote_test(&desc, ValidatorIndex::new(1), notar2.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(2), notar2.clone(), Vec::new()).unwrap();
    state.on_vote_test(&desc, ValidatorIndex::new(3), notar2, Vec::new()).unwrap();
    while state.pull_event().is_some() {}

    // Verify slot 2 is notarized
    assert!(
        state.has_notarized_block(SlotIndex::new(2)),
        "slot 2 must be notarized after quorum votes — second leader recovery works"
    );

    // Progress cursor must advance past slot 2
    assert!(
        state.first_non_progressed_slot > SlotIndex::new(2),
        "progress cursor must advance past notarized slot 2, got {}",
        state.first_non_progressed_slot
    );
}
