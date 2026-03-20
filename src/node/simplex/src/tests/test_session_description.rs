/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for SessionDescription

use crate::{
    block::{SlotIndex, ValidatorIndex, WindowIndex},
    session_description::SessionDescription,
    SessionId, SessionNode, SessionOptions,
};
use std::time::{Duration, SystemTime};
use ton_block::{Ed25519KeyOption, ShardIdent};

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

/// Create test validators with custom weights
fn create_test_validators_weighted(weights: &[u64]) -> Vec<SessionNode> {
    weights
        .iter()
        .map(|&weight| {
            let public_key = Ed25519KeyOption::generate().expect("Failed to generate key");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight }
        })
        .collect()
}

/// Create test SessionDescription with default options
fn create_test_desc(nodes: &[SessionNode], local_idx: usize) -> SessionDescription {
    let local_key = nodes[local_idx].public_key.clone();
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();
    SessionDescription::new(
        &opts,
        SessionId::default(),
        1, // initial_block_seqno
        nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .unwrap()
}

/// Create test SessionDescription with custom options
fn create_test_desc_with_opts(
    nodes: &[SessionNode],
    local_idx: usize,
    opts: &SessionOptions,
) -> SessionDescription {
    let local_key = nodes[local_idx].public_key.clone();
    let shard = ShardIdent::masterchain();
    SessionDescription::new(
        opts,
        SessionId::default(),
        1, // initial_block_seqno
        nodes,
        local_key,
        &shard,
        SystemTime::now(),
        None,
    )
    .unwrap()
}

// ============================================================================
// Constructor tests
// ============================================================================

#[test]
fn test_new_creates_session_description() {
    let nodes = create_test_validators(4);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_nodes(), 4);
    assert_eq!(desc.get_self_idx(), ValidatorIndex::new(0));
    assert_eq!(desc.get_total_weight(), 4);
}

#[test]
fn test_new_with_different_local_idx() {
    let nodes = create_test_validators(4);
    let desc = create_test_desc(&nodes, 2);

    assert_eq!(desc.get_self_idx(), ValidatorIndex::new(2));
}

#[test]
fn test_new_fails_with_unknown_local_id() {
    let nodes = create_test_validators(4);
    let unknown_key = Ed25519KeyOption::generate().expect("Failed to generate key");
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();

    let result = SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        unknown_key, // This key is not in the nodes list
        &shard,
        SystemTime::now(),
        None,
    );
    assert!(result.is_err());
}

// ============================================================================
// Validator management tests
// ============================================================================

#[test]
fn test_get_source_public_key_hash() {
    let nodes = create_test_validators(3);
    let expected_hash = nodes[1].public_key.id().clone();
    let desc = create_test_desc(&nodes, 0);

    let hash = desc.get_source_public_key_hash(ValidatorIndex::new(1));
    assert_eq!(*hash, expected_hash);
}

#[test]
fn test_get_source_public_key() {
    let nodes = create_test_validators(3);
    let expected_key = nodes[2].public_key.clone();
    let desc = create_test_desc(&nodes, 0);

    let key = desc.get_source_public_key(ValidatorIndex::new(2));
    assert_eq!(key.id(), expected_key.id());
}

#[test]
fn test_get_node_weight() {
    let nodes = create_test_validators_weighted(&[10, 20, 30]);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_node_weight(ValidatorIndex::new(0)), 10);
    assert_eq!(desc.get_node_weight(ValidatorIndex::new(1)), 20);
    assert_eq!(desc.get_node_weight(ValidatorIndex::new(2)), 30);
}

#[test]
fn test_get_total_nodes() {
    let nodes = create_test_validators(7);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_nodes(), 7);
}

#[test]
fn test_is_self() {
    let nodes = create_test_validators(4);
    let desc = create_test_desc(&nodes, 2);

    assert!(!desc.is_self(ValidatorIndex::new(0)));
    assert!(!desc.is_self(ValidatorIndex::new(1)));
    assert!(desc.is_self(ValidatorIndex::new(2)));
    assert!(!desc.is_self(ValidatorIndex::new(3)));
}

// ============================================================================
// Weight and threshold tests
// ============================================================================

#[test]
fn test_get_total_weight() {
    let nodes = create_test_validators_weighted(&[10, 20, 30, 40]);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_weight(), 100);
}

#[test]
fn test_get_threshold_66() {
    // With total weight 100, strict >2/3 threshold = 67
    let nodes = create_test_validators_weighted(&[25, 25, 25, 25]);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_weight(), 100);
    assert_eq!(desc.get_threshold_66(), 67);
}

#[test]
fn test_get_threshold_33() {
    // With total weight 100, strict >1/3 threshold = 34
    let nodes = create_test_validators_weighted(&[25, 25, 25, 25]);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_weight(), 100);
    assert_eq!(desc.get_threshold_33(), 34);
}

#[test]
fn test_thresholds_with_equal_weights() {
    // 4 validators with weight 1 each, total = 4
    // threshold_66 = (4 * 2) / 3 + 1 = 3  (strict >2/3)
    // threshold_33 = 4 / 3 + 1 = 2        (strict >1/3)
    let nodes = create_test_validators(4);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_weight(), 4);
    assert_eq!(desc.get_threshold_66(), 3);
    assert_eq!(desc.get_threshold_33(), 2);
}

#[test]
fn test_thresholds_when_total_divisible_by_3() {
    // This is the critical boundary case for ton-simnet WC0:
    // total_weight divisible by 3 must still require STRICT >2/3 and >1/3.
    //
    // For 6 validators with weight 1 each:
    // threshold_66 = (6 * 2) / 3 + 1 = 5
    // threshold_33 = 6 / 3 + 1 = 3
    let nodes = create_test_validators(6);
    let desc = create_test_desc(&nodes, 0);

    assert_eq!(desc.get_total_weight(), 6);
    assert_eq!(desc.get_threshold_66(), 5);
    assert_eq!(desc.get_threshold_33(), 3);
}

// ============================================================================
// Slot and window tests
// ============================================================================

#[test]
fn test_get_window_idx_single_slot_window() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 1;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    assert_eq!(desc.get_window_idx(SlotIndex::new(0)), WindowIndex::new(0));
    assert_eq!(desc.get_window_idx(SlotIndex::new(1)), WindowIndex::new(1));
    assert_eq!(desc.get_window_idx(SlotIndex::new(5)), WindowIndex::new(5));
}

#[test]
fn test_get_window_idx_multi_slot_window() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 3;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    // Window 0: slots 0, 1, 2
    assert_eq!(desc.get_window_idx(SlotIndex::new(0)), WindowIndex::new(0));
    assert_eq!(desc.get_window_idx(SlotIndex::new(1)), WindowIndex::new(0));
    assert_eq!(desc.get_window_idx(SlotIndex::new(2)), WindowIndex::new(0));
    // Window 1: slots 3, 4, 5
    assert_eq!(desc.get_window_idx(SlotIndex::new(3)), WindowIndex::new(1));
    assert_eq!(desc.get_window_idx(SlotIndex::new(4)), WindowIndex::new(1));
    assert_eq!(desc.get_window_idx(SlotIndex::new(5)), WindowIndex::new(1));
}

#[test]
fn test_get_slot_offset_in_window() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 3;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    assert_eq!(desc.get_slot_offset_in_window(SlotIndex::new(0)), 0);
    assert_eq!(desc.get_slot_offset_in_window(SlotIndex::new(1)), 1);
    assert_eq!(desc.get_slot_offset_in_window(SlotIndex::new(2)), 2);
    assert_eq!(desc.get_slot_offset_in_window(SlotIndex::new(3)), 0);
    assert_eq!(desc.get_slot_offset_in_window(SlotIndex::new(4)), 1);
}

#[test]
fn test_is_first_in_window() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 3;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    assert!(desc.is_first_in_window(SlotIndex::new(0)));
    assert!(!desc.is_first_in_window(SlotIndex::new(1)));
    assert!(!desc.is_first_in_window(SlotIndex::new(2)));
    assert!(desc.is_first_in_window(SlotIndex::new(3)));
    assert!(!desc.is_first_in_window(SlotIndex::new(4)));
}

// ============================================================================
// Leader selection tests
// ============================================================================

#[test]
fn test_get_leader_round_robin() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 1;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    // With 4 validators and 1 slot per window:
    // slot 0 -> window 0 -> leader 0
    // slot 1 -> window 1 -> leader 1
    // slot 2 -> window 2 -> leader 2
    // slot 3 -> window 3 -> leader 3
    // slot 4 -> window 4 -> leader 0 (wraps around)
    assert_eq!(desc.get_leader(SlotIndex::new(0)), ValidatorIndex::new(0));
    assert_eq!(desc.get_leader(SlotIndex::new(1)), ValidatorIndex::new(1));
    assert_eq!(desc.get_leader(SlotIndex::new(2)), ValidatorIndex::new(2));
    assert_eq!(desc.get_leader(SlotIndex::new(3)), ValidatorIndex::new(3));
    assert_eq!(desc.get_leader(SlotIndex::new(4)), ValidatorIndex::new(0));
    assert_eq!(desc.get_leader(SlotIndex::new(5)), ValidatorIndex::new(1));
}

#[test]
fn test_get_leader_multi_slot_window() {
    let nodes = create_test_validators(3);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 2;
    let desc = create_test_desc_with_opts(&nodes, 0, &opts);

    // With 3 validators and 2 slots per window:
    // slots 0,1 -> window 0 -> leader 0
    // slots 2,3 -> window 1 -> leader 1
    // slots 4,5 -> window 2 -> leader 2
    // slots 6,7 -> window 3 -> leader 0 (wraps around)
    assert_eq!(desc.get_leader(SlotIndex::new(0)), ValidatorIndex::new(0));
    assert_eq!(desc.get_leader(SlotIndex::new(1)), ValidatorIndex::new(0));
    assert_eq!(desc.get_leader(SlotIndex::new(2)), ValidatorIndex::new(1));
    assert_eq!(desc.get_leader(SlotIndex::new(3)), ValidatorIndex::new(1));
    assert_eq!(desc.get_leader(SlotIndex::new(4)), ValidatorIndex::new(2));
    assert_eq!(desc.get_leader(SlotIndex::new(5)), ValidatorIndex::new(2));
    assert_eq!(desc.get_leader(SlotIndex::new(6)), ValidatorIndex::new(0));
}

#[test]
fn test_is_self_leader() {
    let nodes = create_test_validators(4);
    let mut opts = SessionOptions::default();
    opts.slots_per_leader_window = 1;

    // Local validator is index 2
    let desc = create_test_desc_with_opts(&nodes, 2, &opts);

    assert!(!desc.is_self_leader(SlotIndex::new(0))); // leader 0
    assert!(!desc.is_self_leader(SlotIndex::new(1))); // leader 1
    assert!(desc.is_self_leader(SlotIndex::new(2))); // leader 2 (self)
    assert!(!desc.is_self_leader(SlotIndex::new(3))); // leader 3
    assert!(!desc.is_self_leader(SlotIndex::new(4))); // leader 0
    assert!(desc.is_self_leader(SlotIndex::new(6))); // leader 2 (self)
}

// ============================================================================
// Time management tests
// ============================================================================

#[test]
fn test_get_time_returns_current_time_by_default() {
    let nodes = create_test_validators(3);
    let desc = create_test_desc(&nodes, 0);

    let before = SystemTime::now();
    let desc_time = desc.get_time();
    let after = SystemTime::now();

    assert!(desc_time >= before);
    assert!(desc_time <= after);
}

#[test]
fn test_set_time_overrides_current_time() {
    let nodes = create_test_validators(3);
    let desc = create_test_desc(&nodes, 0);

    let fixed_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1000000);
    desc.set_time(fixed_time);

    assert_eq!(desc.get_time(), fixed_time);
}

#[test]
fn test_is_in_future() {
    let nodes = create_test_validators(3);
    let desc = create_test_desc(&nodes, 0);

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    desc.set_time(now);

    let past = SystemTime::UNIX_EPOCH + Duration::from_secs(500);
    let future = SystemTime::UNIX_EPOCH + Duration::from_secs(1500);

    assert!(!desc.is_in_future(past));
    assert!(!desc.is_in_future(now));
    assert!(desc.is_in_future(future));
}

#[test]
fn test_is_in_past() {
    let nodes = create_test_validators(3);
    let desc = create_test_desc(&nodes, 0);

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    desc.set_time(now);

    let past = SystemTime::UNIX_EPOCH + Duration::from_secs(500);
    let future = SystemTime::UNIX_EPOCH + Duration::from_secs(1500);

    assert!(desc.is_in_past(past));
    assert!(!desc.is_in_past(now));
    assert!(!desc.is_in_past(future));
}

// ============================================================================
// Shard tests
// ============================================================================

#[test]
fn test_get_shard_masterchain() {
    let nodes = create_test_validators(3);
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
    .unwrap();

    assert!(desc.get_shard().is_masterchain());
}

#[test]
fn test_get_shard_workchain() {
    let nodes = create_test_validators(3);
    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000u64).unwrap();
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
    .unwrap();

    assert!(!desc.get_shard().is_masterchain());
    assert_eq!(desc.get_shard().workchain_id(), 0);
}

// ============================================================================
// Display tests
// ============================================================================

#[test]
fn test_display() {
    let nodes = create_test_validators(5);
    let desc = create_test_desc(&nodes, 2);

    let display = format!("{}", desc);
    assert!(display.contains("nodes=5"));
    assert!(display.contains("total_weight=5"));
    assert!(display.contains("self_idx=v002"));
}
