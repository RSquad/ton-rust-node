/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use consensus_common::{BlockCandidatePriority, BlockSourceInfo, PublicKey};
use ton_block::Ed25519KeyOption;

#[test]
fn test_block_candidate_priority_equality() {
    let p1 = BlockCandidatePriority { round: 5, first_block_round: 3, priority: 0 };
    let p2 = BlockCandidatePriority { round: 5, first_block_round: 3, priority: 0 };
    let p3 = BlockCandidatePriority { round: 5, first_block_round: 3, priority: 1 };

    assert_eq!(p1, p2);
    assert_ne!(p1, p3);
}

#[test]
fn test_block_source_info_creation() {
    let private_key = Ed25519KeyOption::generate().expect("Failed to generate key");
    let public_key: PublicKey = private_key;
    let priority = BlockCandidatePriority { round: 5, first_block_round: 3, priority: 0 };
    let info = BlockSourceInfo { source: public_key.clone(), priority };

    assert_eq!(info.priority.round, 5);
    assert_eq!(info.priority.first_block_round, 3);
    assert_eq!(info.priority.priority, 0);
    assert_eq!(info.source.id(), public_key.id());
}

#[test]
fn test_first_block_round_tracking() {
    // Test scenario: first_block_round progression
    let private_key = Ed25519KeyOption::generate().expect("Failed to generate key");
    let public_key: PublicKey = private_key;

    // Round 0, first_block_round = 0 (initial state)
    let p0 = BlockCandidatePriority { round: 0, first_block_round: 0, priority: 0 };
    let info0 = BlockSourceInfo { source: public_key.clone(), priority: p0 };
    assert_eq!(info0.priority.first_block_round, 0);

    // After committing round 0, first_block_round becomes 1
    let p1 = BlockCandidatePriority { round: 1, first_block_round: 1, priority: 0 };
    let info1 = BlockSourceInfo { source: public_key.clone(), priority: p1 };
    assert_eq!(info1.priority.first_block_round, 1);
    assert_eq!(info1.priority.round, info1.priority.first_block_round);

    // Later rounds
    let p2 = BlockCandidatePriority {
        round: 2,
        first_block_round: 1, // first_block_round stays at 1
        priority: 0,
    };
    let info2 = BlockSourceInfo { source: public_key, priority: p2 };
    assert_eq!(info2.priority.first_block_round, 1);
    assert!(info2.priority.round > info2.priority.first_block_round);
}
