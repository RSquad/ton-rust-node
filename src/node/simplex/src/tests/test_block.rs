/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Block candidate type tests for Simplex consensus
//!
//! These tests verify:
//! - RawCandidateId creation and display
//! - CandidateId creation and conversions
//! - RawCandidate invariants and resolution
//! - Empty block handling

use crate::{
    block::{
        BlockCandidate, CandidateId, CandidateParentInfo, RawCandidate, RawCandidateId, SlotIndex,
        ValidatorIndex, WindowIndex,
    },
    PublicKey,
};
use std::collections::HashMap;
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, UInt256};

/*
    ============================================================================
    Test Helpers
    ============================================================================
*/

fn create_test_public_key() -> PublicKey {
    Ed25519KeyOption::generate().expect("Failed to generate key")
}

/*
    ============================================================================
    SlotIndex tests
    ============================================================================
*/

/// Test SlotIndex creation and value extraction
#[test]
fn test_slot_index_basic() {
    let slot = SlotIndex::new(42);
    assert_eq!(slot.value(), 42);
    assert_eq!(slot.0, 42);

    let slot2 = SlotIndex(100);
    assert_eq!(slot2.value(), 100);
}

/// Test SlotIndex display formatting (s0, s1, s42, etc.)
#[test]
fn test_slot_index_display() {
    let slot = SlotIndex::new(42);
    assert_eq!(format!("{}", slot), "s42");
    assert_eq!(format!("{:?}", slot), "s42");

    let slot_zero = SlotIndex::new(0);
    assert_eq!(format!("{}", slot_zero), "s0");
}

/// Test SlotIndex arithmetic operations
#[test]
fn test_slot_index_arithmetic() {
    let slot = SlotIndex::new(10);

    // Add u32
    let slot_plus = slot + 5;
    assert_eq!(slot_plus.value(), 15);

    // Sub u32
    let slot_minus = slot - 3;
    assert_eq!(slot_minus.value(), 7);

    // Sub SlotIndex returns u32
    let slot2 = SlotIndex::new(7);
    let diff: u32 = slot - slot2;
    assert_eq!(diff, 3);

    // Saturating subtraction (no underflow)
    let slot3 = SlotIndex::new(5);
    let result = slot3 - 10;
    assert_eq!(result.value(), 0);

    // AddAssign
    let mut slot4 = SlotIndex::new(5);
    slot4 += 3;
    assert_eq!(slot4.value(), 8);

    // Remainder
    let slot5 = SlotIndex::new(17);
    assert_eq!(slot5 % 5, 2);
}

/// Test SlotIndex comparison operations
#[test]
fn test_slot_index_comparison() {
    let slot1 = SlotIndex::new(10);
    let slot2 = SlotIndex::new(20);
    let slot3 = SlotIndex::new(10);

    assert!(slot1 < slot2);
    assert!(slot2 > slot1);
    assert!(slot1 <= slot3);
    assert!(slot1 >= slot3);
    assert_eq!(slot1, slot3);
    assert_ne!(slot1, slot2);
}

/// Test SlotIndex conversion traits
#[test]
fn test_slot_index_conversions() {
    // From u32
    let slot: SlotIndex = 42u32.into();
    assert_eq!(slot.value(), 42);

    // Into u32
    let val: u32 = slot.into();
    assert_eq!(val, 42);

    // From i32
    let slot2: SlotIndex = 100i32.into();
    assert_eq!(slot2.value(), 100);

    // Into i32
    let val2: i32 = slot2.into();
    assert_eq!(val2, 100);
}

/// Test SlotIndex window calculations
#[test]
fn test_slot_index_window_calculations() {
    let slots_per_window = 4u32;

    // Slot 0 is in window 0, offset 0
    let slot0 = SlotIndex::new(0);
    assert_eq!(slot0.window_index(slots_per_window).value(), 0);
    assert_eq!(slot0.offset_in_window(slots_per_window), 0);
    assert!(slot0.is_first_in_window(slots_per_window));
    assert!(!slot0.is_last_in_window(slots_per_window));

    // Slot 3 is in window 0, offset 3 (last in window)
    let slot3 = SlotIndex::new(3);
    assert_eq!(slot3.window_index(slots_per_window).value(), 0);
    assert_eq!(slot3.offset_in_window(slots_per_window), 3);
    assert!(!slot3.is_first_in_window(slots_per_window));
    assert!(slot3.is_last_in_window(slots_per_window));

    // Slot 4 is in window 1, offset 0 (first in window)
    let slot4 = SlotIndex::new(4);
    assert_eq!(slot4.window_index(slots_per_window).value(), 1);
    assert_eq!(slot4.offset_in_window(slots_per_window), 0);
    assert!(slot4.is_first_in_window(slots_per_window));

    // Slot 10 is in window 2, offset 2
    let slot10 = SlotIndex::new(10);
    assert_eq!(slot10.window_index(slots_per_window).value(), 2);
    assert_eq!(slot10.offset_in_window(slots_per_window), 2);

    // window_start returns the first slot of the window
    assert_eq!(slot10.window_start(slots_per_window).value(), 8);
}

/// Test SlotIndex hashing (for use in HashMap)
#[test]
fn test_slot_index_hash() {
    let mut map: HashMap<SlotIndex, &str> = HashMap::new();
    map.insert(SlotIndex::new(0), "slot_0");
    map.insert(SlotIndex::new(42), "slot_42");

    assert_eq!(map.get(&SlotIndex::new(0)), Some(&"slot_0"));
    assert_eq!(map.get(&SlotIndex::new(42)), Some(&"slot_42"));
    assert_eq!(map.get(&SlotIndex::new(1)), None);
}

/*
    ============================================================================
    WindowIndex tests
    ============================================================================
*/

/// Test WindowIndex creation and value extraction
#[test]
fn test_window_index_basic() {
    let window = WindowIndex::new(5);
    assert_eq!(window.value(), 5);
    assert_eq!(window.0, 5);
}

/// Test WindowIndex display formatting (w0, w1, w42, etc.)
#[test]
fn test_window_index_display() {
    let window = WindowIndex::new(42);
    assert_eq!(format!("{}", window), "w42");
    assert_eq!(format!("{:?}", window), "w42");
}

/// Test WindowIndex arithmetic operations
#[test]
fn test_window_index_arithmetic() {
    let window = WindowIndex::new(10);

    // Add u32
    let window_plus = window + 5;
    assert_eq!(window_plus.value(), 15);

    // Sub u32
    let window_minus = window - 3;
    assert_eq!(window_minus.value(), 7);

    // Sub WindowIndex returns u32
    let window2 = WindowIndex::new(7);
    let diff: u32 = window - window2;
    assert_eq!(diff, 3);

    // Saturating subtraction (no underflow)
    let window3 = WindowIndex::new(5);
    let result = window3 - 10;
    assert_eq!(result.value(), 0);

    // Mul returns SlotIndex
    let window4 = WindowIndex::new(3);
    let slot: SlotIndex = window4 * 4; // 3 * 4 = 12
    assert_eq!(slot.value(), 12);
}

/// Test WindowIndex comparison operations
#[test]
fn test_window_index_comparison() {
    let window1 = WindowIndex::new(10);
    let window2 = WindowIndex::new(20);
    let window3 = WindowIndex::new(10);

    assert!(window1 < window2);
    assert!(window2 > window1);
    assert!(window1 <= window3);
    assert!(window1 >= window3);
    assert_eq!(window1, window3);
    assert_ne!(window1, window2);
}

/// Test WindowIndex slot calculations
#[test]
fn test_window_index_slot_calculations() {
    let slots_per_window = 4u32;

    let window0 = WindowIndex::new(0);
    assert_eq!(window0.first_slot(slots_per_window).value(), 0);
    assert_eq!(window0.last_slot(slots_per_window).value(), 3);
    assert_eq!(window0.window_start(slots_per_window).value(), 0);

    let window2 = WindowIndex::new(2);
    assert_eq!(window2.first_slot(slots_per_window).value(), 8);
    assert_eq!(window2.last_slot(slots_per_window).value(), 11);
    assert_eq!(window2.window_start(slots_per_window).value(), 8);
}

/// Test WindowIndex hashing (for use in HashMap)
#[test]
fn test_window_index_hash() {
    let mut map: HashMap<WindowIndex, &str> = HashMap::new();
    map.insert(WindowIndex::new(0), "window_0");
    map.insert(WindowIndex::new(42), "window_42");

    assert_eq!(map.get(&WindowIndex::new(0)), Some(&"window_0"));
    assert_eq!(map.get(&WindowIndex::new(42)), Some(&"window_42"));
    assert_eq!(map.get(&WindowIndex::new(1)), None);
}

/*
    ============================================================================
    ValidatorIndex tests
    ============================================================================
*/

/// Test ValidatorIndex creation and value extraction
#[test]
fn test_validator_index_basic() {
    let validator = ValidatorIndex::new(5);
    assert_eq!(validator.value(), 5);
    assert_eq!(validator.0, 5);
}

/// Test ValidatorIndex display formatting (v000, v001, v042, etc.)
#[test]
fn test_validator_index_display() {
    let v0 = ValidatorIndex::new(0);
    assert_eq!(format!("{}", v0), "v000");
    assert_eq!(format!("{:?}", v0), "v000");

    let v42 = ValidatorIndex::new(42);
    assert_eq!(format!("{}", v42), "v042");

    let v123 = ValidatorIndex::new(123);
    assert_eq!(format!("{}", v123), "v123");
}

/// Test ValidatorIndex comparison operations
#[test]
fn test_validator_index_comparison() {
    let v1 = ValidatorIndex::new(10);
    let v2 = ValidatorIndex::new(20);
    let v3 = ValidatorIndex::new(10);

    assert!(v1 < v2);
    assert!(v2 > v1);
    assert!(v1 <= v3);
    assert!(v1 >= v3);
    assert_eq!(v1, v3);
    assert_ne!(v1, v2);
}

/// Test ValidatorIndex conversion traits
#[test]
fn test_validator_index_conversions() {
    // From u32
    let v: ValidatorIndex = 42u32.into();
    assert_eq!(v.value(), 42);

    // Into u32
    let val: u32 = v.into();
    assert_eq!(val, 42);

    // From usize
    let v2: ValidatorIndex = 100usize.into();
    assert_eq!(v2.value(), 100);

    // Into usize
    let val2: usize = v2.into();
    assert_eq!(val2, 100);
}

/// Test ValidatorIndex hashing (for use in HashMap)
#[test]
fn test_validator_index_hash() {
    let mut map: HashMap<ValidatorIndex, &str> = HashMap::new();
    map.insert(ValidatorIndex::new(0), "validator_0");
    map.insert(ValidatorIndex::new(42), "validator_42");

    assert_eq!(map.get(&ValidatorIndex::new(0)), Some(&"validator_0"));
    assert_eq!(map.get(&ValidatorIndex::new(42)), Some(&"validator_42"));
    assert_eq!(map.get(&ValidatorIndex::new(1)), None);
}

/// Test ValidatorIndex arithmetic operations
#[test]
fn test_validator_index_arithmetic() {
    let v5 = ValidatorIndex::new(5);
    let v10 = ValidatorIndex::new(10);

    // Add<u32>
    assert_eq!(v5 + 3, ValidatorIndex::new(8));
    assert_eq!(v5 + 0, v5);

    // Sub<u32> (saturating)
    assert_eq!(v10 - 3, ValidatorIndex::new(7));
    assert_eq!(v5 - 10, ValidatorIndex::new(0)); // saturating at 0

    // Sub<ValidatorIndex> (saturating, returns u32)
    assert_eq!(v10 - v5, 5u32);
    assert_eq!(v5 - v10, 0u32); // saturating at 0

    // Rem<u32> (modulo - useful for round-robin)
    let v17 = ValidatorIndex::new(17);
    assert_eq!(v17 % 5, 2); // 17 % 5 = 2
    assert_eq!(v5 % 10, 5); // 5 % 10 = 5
    assert_eq!(v10 % 3, 1); // 10 % 3 = 1

    // AddAssign<u32>
    let mut v_mut = ValidatorIndex::new(5);
    v_mut += 3;
    assert_eq!(v_mut, ValidatorIndex::new(8));
}

/// Test ValidatorIndex modulo for round-robin leader selection
#[test]
fn test_validator_index_round_robin() {
    // Simulates leader selection: window_idx % num_validators
    let num_validators = 4u32;

    // Window 0 -> Validator 0
    let w0_leader = ValidatorIndex::new(0 % num_validators);
    assert_eq!(w0_leader, ValidatorIndex::new(0));

    // Window 5 -> Validator 1 (5 % 4 = 1)
    let w5_leader = ValidatorIndex::new(5 % num_validators);
    assert_eq!(w5_leader, ValidatorIndex::new(1));

    // Alternative: use Rem with ValidatorIndex
    let window_idx = ValidatorIndex::new(7);
    let leader = window_idx % num_validators; // returns u32
    assert_eq!(leader, 3); // 7 % 4 = 3
}

/// Test ValidatorIndex bounds checking methods
#[test]
fn test_validator_index_bounds_checking() {
    let num_validators = 10usize;

    // Valid indices (0..9)
    assert!(ValidatorIndex::new(0).is_valid(num_validators));
    assert!(ValidatorIndex::new(5).is_valid(num_validators));
    assert!(ValidatorIndex::new(9).is_valid(num_validators));

    // Invalid indices (>= 10)
    assert!(!ValidatorIndex::new(10).is_valid(num_validators));
    assert!(!ValidatorIndex::new(100).is_valid(num_validators));

    // is_out_of_bounds is the opposite
    assert!(!ValidatorIndex::new(0).is_out_of_bounds(num_validators));
    assert!(!ValidatorIndex::new(9).is_out_of_bounds(num_validators));
    assert!(ValidatorIndex::new(10).is_out_of_bounds(num_validators));
    assert!(ValidatorIndex::new(100).is_out_of_bounds(num_validators));

    // Edge case: empty validator set
    assert!(!ValidatorIndex::new(0).is_valid(0));
    assert!(ValidatorIndex::new(0).is_out_of_bounds(0));
}

/*
    ============================================================================
    RawCandidateId tests
    ============================================================================
*/

/// Test RawCandidateId display formatting
#[test]
fn test_raw_candidate_id_display() {
    let id = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::from([0x11u8; 32]) };
    let display = format!("{}", id);
    assert!(display.contains("42"));
    assert!(display.contains("11111111"));
}

/*
    ============================================================================
    CandidateId tests
    ============================================================================
*/

/// Test CandidateId creation from RawCandidateId
#[test]
fn test_candidate_id_from_raw() {
    let raw = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::from([0x22u8; 32]) };
    let block = BlockIdExt::default();
    let id = CandidateId::from_raw(&raw, block.clone());
    assert_eq!(id.slot.value(), 42);
    assert_eq!(id.hash, raw.hash);
    assert_eq!(id.block, block);
}

/// Test CandidateId to/from RawCandidateId conversions
#[test]
fn test_candidate_id_conversions() {
    let raw = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::from([0x33u8; 32]) };
    let block = BlockIdExt::default();
    let id = CandidateId::from_raw(&raw, block);

    // Test to_raw
    let back_to_raw = id.to_raw();
    assert_eq!(back_to_raw.slot, raw.slot);
    assert_eq!(back_to_raw.hash, raw.hash);

    // Test From trait
    let from_ref: RawCandidateId = (&id).into();
    assert_eq!(from_ref.slot, raw.slot);
}

/*
    ============================================================================
    RawCandidate tests
    ============================================================================
*/

/// Test RawCandidate invariant: block or parent must be present
///
/// This test verifies that creating a RawCandidate with both block=None
/// and parent=None panics.
// NOTE: The invariant test was removed because the new RawCandidate API enforces
// the invariant at compile time:
// - RawCandidate::new() requires BlockCandidate (not Option)
// - RawCandidate::new_empty() requires both parent_id and referenced_block
// There's no way to create a RawCandidate with both None at compile time now.

/// Test RawCandidate with block data
#[test]
fn test_raw_candidate_with_block() {
    let id = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::default() };
    let block = BlockCandidate {
        id: BlockIdExt::default(),
        collated_file_hash: UInt256::default(),
        data: vec![1, 2, 3],
        collated_data: vec![4, 5, 6],
        creator: create_test_public_key(),
    };
    let candidate = RawCandidate::new(id, None, ValidatorIndex::new(0), block, vec![0xAA; 64]);
    assert!(!candidate.is_empty());
}

/// Test RawCandidate empty block with parent
#[test]
fn test_raw_candidate_empty_with_parent() {
    let id = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::default() };
    let parent = RawCandidateId { slot: SlotIndex::new(41), hash: UInt256::from([0x44u8; 32]) };
    let referenced_block = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        41,
        UInt256::from([0x11u8; 32]),
        UInt256::from([0x22u8; 32]),
    );
    let candidate = RawCandidate::new_empty(
        id,
        parent,
        ValidatorIndex::new(0),
        referenced_block,
        vec![0xBB; 64],
    );
    assert!(candidate.is_empty());
}

/// Test RawCandidate resolve with block
#[test]
fn test_raw_candidate_resolve() {
    let id = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::from([0x55u8; 32]) };
    let block = BlockCandidate {
        id: BlockIdExt::with_params(
            ShardIdent::masterchain(),
            42,
            UInt256::from([0x11u8; 32]),
            UInt256::from([0x22u8; 32]),
        ),
        collated_file_hash: UInt256::from([0x33u8; 32]),
        data: vec![],
        collated_data: vec![],
        creator: create_test_public_key(),
    };
    let block_id_expected = block.id.clone();
    let raw_candidate = RawCandidate::new(id, None, ValidatorIndex::new(0), block, vec![0xCC; 64]);

    let resolved = raw_candidate.resolve(None).expect("should resolve non-empty block");
    assert_eq!(resolved.id.slot.value(), 42);
    assert_eq!(resolved.id.block, block_id_expected);
    assert!(resolved.parent_id.is_none());
}

/*
    ============================================================================
    Empty block tests
    ============================================================================
*/

/// Test empty block resolution uses the referenced BlockIdExt
#[test]
fn test_empty_block_resolve() {
    let id = RawCandidateId { slot: SlotIndex::new(42), hash: UInt256::from([0x66u8; 32]) };
    let parent_id = RawCandidateId { slot: SlotIndex::new(41), hash: UInt256::from([0x77u8; 32]) };

    // The referenced block that the empty block inherits
    let referenced_block = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        41,
        UInt256::from([0x88u8; 32]),
        UInt256::from([0x99u8; 32]),
    );
    let raw_candidate = RawCandidate::new_empty(
        id,
        parent_id,
        ValidatorIndex::new(0),
        referenced_block.clone(),
        vec![0xDD; 64],
    );

    // Resolve empty block - should use the referenced_block from CandidateBlockData::Empty
    let resolved = raw_candidate.resolve(None).expect("should resolve empty block");
    assert_eq!(resolved.id.slot.value(), 42);
    assert_eq!(resolved.id.block, referenced_block); // Uses referenced_block
    assert!(resolved.is_empty());
}

// NOTE: test_empty_block_resolve_fails_without_parent was removed because
// with the new CandidateBlockData enum, empty blocks ALWAYS have the referenced BlockIdExt
// stored in the Empty variant. There's no way to create an empty block without it.

/// Test RawCandidateId::create() for empty block (block=None)
///
/// Verifies that:
/// - Empty block hash computation uses default BlockIdExt and zero collated_file_hash
/// - Hash is deterministic
/// - Parent is included in hash computation
/// - Non-empty block with actual data produces different hash
#[test]
fn test_raw_candidate_id_create_empty_block() {
    let parent = RawCandidateId { slot: SlotIndex::new(41), hash: UInt256::from([0xABu8; 32]) };

    // Create empty block candidate ID (block=None, has parent)
    let id1 = RawCandidateId::create(SlotIndex::new(42), None, Some(&parent));
    let id2 = RawCandidateId::create(SlotIndex::new(42), None, Some(&parent));

    // Same inputs produce same hash (deterministic)
    assert_eq!(id1.slot.value(), 42);
    assert_eq!(id1.hash, id2.hash);

    // Different parent produces different hash
    let other_parent =
        RawCandidateId { slot: SlotIndex::new(41), hash: UInt256::from([0xCDu8; 32]) };
    let id3 = RawCandidateId::create(SlotIndex::new(42), None, Some(&other_parent));
    assert_ne!(id1.hash, id3.hash);

    // Non-empty block with actual block data produces different hash
    // NOTE: We use non-default BlockIdExt values because empty block hash uses
    // default BlockIdExt and zero collated_file_hash
    let block = BlockCandidate {
        id: BlockIdExt::with_params(
            ShardIdent::masterchain(),
            12345,
            UInt256::from([0x11u8; 32]),
            UInt256::from([0x22u8; 32]),
        ),
        collated_file_hash: UInt256::from([0x33u8; 32]),
        data: vec![1, 2, 3],
        collated_data: vec![4, 5, 6],
        creator: create_test_public_key(),
    };
    let id4 = RawCandidateId::create(SlotIndex::new(42), Some(&block), Some(&parent));
    assert_ne!(id1.hash, id4.hash);

    // Verify the hash is non-zero for empty blocks
    assert_ne!(id1.hash, UInt256::default());
}

/*
    ============================================================================
    CandidateParentInfo tests
    ============================================================================
*/

/// Test CandidateParentInfo creation and display
#[test]
fn test_candidate_parent_info() {
    let info = CandidateParentInfo::new(SlotIndex::new(42), UInt256::from([0xAAu8; 32]));
    assert_eq!(info.slot.value(), 42);
    let display = format!("{}", info);
    assert!(display.contains("42"));
}
