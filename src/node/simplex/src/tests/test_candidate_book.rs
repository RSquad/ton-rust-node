/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for [`crate::candidate_book::CandidateBook`].
//!
//! Included directly from `candidate_book.rs` via `#[path]` so tests can
//! reach the private accessor surface and the `ReceivedCandidate` inner
//! struct without widening visibility. Mirrors the convention used by
//! `tests/test_session_runtime.rs`.
//!
//! Scope (state mechanics only — ingress orchestration / network
//! interactions stay covered by `tests/test_session_processor.rs`):
//! - `received_candidates` round-trips: insert / `received` / `received_mut`
//!   / `contains_received` / `received_count` / `iter_received` /
//!   `retain_received`.
//! - `has_real_body` distinguishes stubs (empty `candidate_hash_data_bytes`)
//!   from real bodies.
//! - `find_received_by_block_id` reverse lookup.
//! - `candidate_data_cache` round-trip + `contains_cached_data`.
//! - `seen_broadcast_candidates` round-trip + dedup-on-second-insert
//!   semantics.
//! - `prune_below` clears the two auxiliary maps but intentionally leaves
//!   `received_candidates` untouched (finalized-stub-safety constraint
//!   documented in the module header).
//! - `Debug` impl smoke test.

use super::*;
use crate::block::{RawCandidateId, SlotIndex, ValidatorIndex};
use consensus_common::ConsensusCommonFactory;
use std::time::SystemTime;
use ton_block::{BlockIdExt, ShardIdent, UInt256};

/*
    --------------------------------------------------------------------
    Test helpers
    --------------------------------------------------------------------
*/

fn make_candidate_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    let mut hash = [0u8; 32];
    hash[0] = hash_byte;
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from(hash) }
}

fn make_block_id(seqno: u32, root_hash_byte: u8) -> BlockIdExt {
    let mut root = [0u8; 32];
    root[0] = root_hash_byte;
    let mut file = [0u8; 32];
    file[1] = root_hash_byte;
    BlockIdExt::with_params(
        ShardIdent::masterchain(),
        seqno,
        UInt256::from(root),
        UInt256::from(file),
    )
}

/// Build a fresh `ReceivedCandidate` carrying a non-empty
/// `candidate_hash_data_bytes` (i.e. a "real body"). Use
/// [`make_received_stub`] for the finalized-boundary stub case.
fn make_received_real(
    slot: u32,
    source_idx: u32,
    block_id: BlockIdExt,
    parent_id: Option<RawCandidateId>,
) -> ReceivedCandidate {
    ReceivedCandidate {
        slot: SlotIndex::new(slot),
        source_idx: ValidatorIndex::new(source_idx),
        candidate_hash_data_bytes: vec![0xAB, 0xCD, 0xEF],
        block_id,
        root_hash: UInt256::from([2u8; 32]),
        file_hash: UInt256::from([3u8; 32]),
        data: ConsensusCommonFactory::create_block_payload(vec![]),
        collated_data: ConsensusCommonFactory::create_block_payload(vec![]),
        gen_utime_ms: Some(1_700_000_000_000),
        receive_time: SystemTime::now(),
        is_empty: false,
        parent_id,
    }
}

/// Build a `ReceivedCandidate` with explicit `is_empty` / `parent_id`
/// for parent-chain walk tests. `resolve_parent_tip` only reads
/// `is_empty`, `block_id`, and `parent_id`; the rest is filler.
fn make_received_chain_node(
    slot: u32,
    block_id: BlockIdExt,
    is_empty: bool,
    parent_id: Option<RawCandidateId>,
) -> ReceivedCandidate {
    ReceivedCandidate {
        slot: SlotIndex::new(slot),
        source_idx: ValidatorIndex::new(0),
        candidate_hash_data_bytes: vec![0xAB],
        block_id,
        root_hash: UInt256::from([2u8; 32]),
        file_hash: UInt256::from([3u8; 32]),
        data: ConsensusCommonFactory::create_block_payload(vec![]),
        collated_data: ConsensusCommonFactory::create_block_payload(vec![]),
        gen_utime_ms: None,
        receive_time: SystemTime::now(),
        is_empty,
        parent_id,
    }
}

/// Build a finalized-boundary stub: empty `candidate_hash_data_bytes`.
/// [`CandidateBook::has_real_body`] must return `false` for stubs.
fn make_received_stub(slot: u32, block_id: BlockIdExt) -> ReceivedCandidate {
    ReceivedCandidate {
        slot: SlotIndex::new(slot),
        source_idx: ValidatorIndex::new(0),
        candidate_hash_data_bytes: Vec::new(),
        block_id,
        root_hash: UInt256::from([0u8; 32]),
        file_hash: UInt256::from([0u8; 32]),
        data: ConsensusCommonFactory::create_block_payload(vec![]),
        collated_data: ConsensusCommonFactory::create_block_payload(vec![]),
        gen_utime_ms: None,
        receive_time: SystemTime::now(),
        is_empty: false,
        parent_id: None,
    }
}

/*
    --------------------------------------------------------------------
    Construction / fresh-state invariants
    --------------------------------------------------------------------
*/

#[test]
fn new_starts_empty() {
    let book = CandidateBook::new();

    assert_eq!(book.received_count(), 0);
    assert!(book.iter_received().next().is_none());
    assert!(!book.contains_received(&make_candidate_id(0, 0)));
    assert!(book.cached_data(&make_candidate_id(0, 0)).is_none());
    assert!(!book.contains_cached_data(&make_candidate_id(0, 0)));
    assert!(book.seen_broadcast(SlotIndex::new(0)).is_none());
    assert!(!book.contains_seen_broadcast(SlotIndex::new(0)));
}

#[test]
fn default_matches_new() {
    let from_new = CandidateBook::new();
    let from_default = CandidateBook::default();

    assert_eq!(from_new.received_count(), from_default.received_count());
    assert_eq!(
        from_new.iter_received().count(),
        from_default.iter_received().count(),
        "Default impl must produce the same empty book as ::new()",
    );
}

/*
    --------------------------------------------------------------------
    received_candidates accessors
    --------------------------------------------------------------------
*/

#[test]
fn insert_and_lookup_received_round_trip() {
    let mut book = CandidateBook::new();
    let id = make_candidate_id(7, 0xAA);
    let block_id = make_block_id(42, 0x10);
    let record = make_received_real(7, 1, block_id, None);

    let prev = book.insert_received(id.clone(), record);
    assert!(prev.is_none(), "first insert must return None");

    assert!(book.contains_received(&id));
    assert_eq!(book.received_count(), 1);
    let got = book.received(&id).expect("entry must be present after insert");
    assert_eq!(got.slot, SlotIndex::new(7));
    assert_eq!(got.source_idx, ValidatorIndex::new(1));
}

#[test]
fn insert_returns_previous_value_on_replace() {
    let mut book = CandidateBook::new();
    let id = make_candidate_id(7, 0xAA);

    let first = make_received_real(7, 1, make_block_id(42, 0x10), None);
    book.insert_received(id.clone(), first);

    let second = make_received_real(7, 2, make_block_id(42, 0x10), None);
    let prev = book.insert_received(id.clone(), second);
    assert!(prev.is_some(), "second insert at the same id must return Some(prev)");
    assert_eq!(prev.unwrap().source_idx, ValidatorIndex::new(1));
    assert_eq!(book.received(&id).unwrap().source_idx, ValidatorIndex::new(2));
}

#[test]
fn received_mut_allows_in_place_update() {
    let mut book = CandidateBook::new();
    let id = make_candidate_id(7, 0xAA);
    book.insert_received(id.clone(), make_received_real(7, 1, make_block_id(42, 0x10), None));

    let parent = make_candidate_id(6, 0xBB);
    book.received_mut(&id).expect("entry must exist").parent_id = Some(parent.clone());

    assert_eq!(book.received(&id).unwrap().parent_id.as_ref(), Some(&parent));
}

#[test]
fn iter_received_yields_all_entries() {
    let mut book = CandidateBook::new();
    let id_a = make_candidate_id(1, 0xAA);
    let id_b = make_candidate_id(2, 0xBB);
    let id_c = make_candidate_id(3, 0xCC);
    book.insert_received(id_a.clone(), make_received_real(1, 1, make_block_id(11, 0x01), None));
    book.insert_received(id_b.clone(), make_received_real(2, 1, make_block_id(22, 0x02), None));
    book.insert_received(id_c.clone(), make_received_real(3, 1, make_block_id(33, 0x03), None));

    let mut ids: Vec<_> = book.iter_received().map(|(id, _)| id.clone()).collect();
    ids.sort_by_key(|id| id.slot.value());
    assert_eq!(ids, vec![id_a, id_b, id_c]);
}

#[test]
fn retain_received_filters_in_place() {
    let mut book = CandidateBook::new();
    let id_a = make_candidate_id(1, 0xAA);
    let id_b = make_candidate_id(5, 0xBB);
    let id_c = make_candidate_id(9, 0xCC);
    book.insert_received(id_a.clone(), make_received_real(1, 1, make_block_id(11, 0x01), None));
    book.insert_received(id_b.clone(), make_received_real(5, 1, make_block_id(55, 0x02), None));
    book.insert_received(id_c.clone(), make_received_real(9, 1, make_block_id(99, 0x03), None));

    book.retain_received(|id, _| id.slot.value() >= 5);

    assert!(!book.contains_received(&id_a));
    assert!(book.contains_received(&id_b));
    assert!(book.contains_received(&id_c));
    assert_eq!(book.received_count(), 2);
}

#[test]
fn has_real_body_distinguishes_stub_from_real() {
    let mut book = CandidateBook::new();
    let real_id = make_candidate_id(1, 0xAA);
    let stub_id = make_candidate_id(2, 0xBB);
    let missing_id = make_candidate_id(3, 0xCC);
    book.insert_received(real_id.clone(), make_received_real(1, 1, make_block_id(11, 0x01), None));
    book.insert_received(stub_id.clone(), make_received_stub(2, make_block_id(22, 0x02)));

    assert!(book.has_real_body(&real_id), "non-empty candidate_hash_data_bytes is a real body");
    assert!(!book.has_real_body(&stub_id), "empty candidate_hash_data_bytes is a stub");
    assert!(!book.has_real_body(&missing_id), "absent id is not a real body");
}

#[test]
fn find_received_by_block_id_returns_first_match_or_none() {
    let mut book = CandidateBook::new();
    let target = make_block_id(7, 0x77);
    let other = make_block_id(99, 0x99);

    let id_target = make_candidate_id(5, 0xAA);
    book.insert_received(id_target.clone(), make_received_real(5, 1, target.clone(), None));
    book.insert_received(make_candidate_id(6, 0xBB), make_received_real(6, 1, other.clone(), None));

    assert_eq!(book.find_received_by_block_id(&target).as_ref(), Some(&id_target));
    assert!(book.find_received_by_block_id(&make_block_id(0, 0)).is_none());
}

/*
    --------------------------------------------------------------------
    Parent-chain resolution (resolve_parent_tip)
    --------------------------------------------------------------------
*/

fn chain_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from([hash_byte; 32]) }
}

#[test]
fn resolve_parent_tip_walks_empty_chain_to_non_empty_ancestor() {
    let mut book = CandidateBook::new();

    let root_id = chain_id(0, 0x01);
    let empty_a_id = chain_id(1, 0x02);
    let empty_b_id = chain_id(2, 0x03);
    let root_block_id = make_block_id(1, 0x31);

    book.insert_received(
        root_id.clone(),
        make_received_chain_node(0, root_block_id.clone(), false, None),
    );
    book.insert_received(
        empty_a_id.clone(),
        make_received_chain_node(1, root_block_id.clone(), true, Some(root_id.clone())),
    );
    book.insert_received(
        empty_b_id.clone(),
        make_received_chain_node(2, root_block_id.clone(), true, Some(empty_a_id.clone())),
    );

    // The candidate's parent is the last empty ancestor; the walk must skip the
    // empty tail and resolve the root non-empty ancestor's block id.
    assert_eq!(
        book.resolve_parent_tip(Some(&empty_b_id), None),
        ParentTipResolution::Resolved(root_block_id),
    );
}

#[test]
fn resolve_parent_tip_non_empty_parent_resolves_immediately() {
    let mut book = CandidateBook::new();
    let parent_id = chain_id(4, 0x41);
    let parent_block_id = make_block_id(9, 0x42);
    book.insert_received(
        parent_id.clone(),
        make_received_chain_node(4, parent_block_id.clone(), false, None),
    );

    assert_eq!(
        book.resolve_parent_tip(Some(&parent_id), None),
        ParentTipResolution::Resolved(parent_block_id),
    );
}

#[test]
fn resolve_parent_tip_no_parent_falls_back_to_accepted_head() {
    let book = CandidateBook::new();
    let accepted = make_block_id(7, 0x55);

    assert_eq!(
        book.resolve_parent_tip(None, Some(&accepted)),
        ParentTipResolution::Resolved(accepted),
    );
    assert_eq!(book.resolve_parent_tip(None, None), ParentTipResolution::Unresolved);
}

#[test]
fn resolve_parent_tip_reports_missing_parent() {
    let mut book = CandidateBook::new();
    let missing_id = chain_id(0, 0x71);
    let empty_id = chain_id(1, 0x72);
    let block_id = make_block_id(2, 0x73);

    // Only the empty child is present; its parent metadata is missing.
    book.insert_received(
        empty_id.clone(),
        make_received_chain_node(1, block_id, true, Some(missing_id.clone())),
    );

    assert_eq!(
        book.resolve_parent_tip(Some(&empty_id), None),
        ParentTipResolution::MissingParent(missing_id),
    );
}

#[test]
fn resolve_parent_tip_allows_deep_empty_chain_beyond_warn_threshold() {
    let mut book = CandidateBook::new();

    let root_id = chain_id(0, 0x61);
    let root_block_id = make_block_id(1, 0x62);
    book.insert_received(
        root_id.clone(),
        make_received_chain_node(0, root_block_id.clone(), false, None),
    );

    let chain_len = EMPTY_CHAIN_WARN_DEPTH + 32;
    let mut prev_id = root_id;
    for idx in 1..=chain_len {
        let mut hash_bytes = [0u8; 32];
        hash_bytes[..4].copy_from_slice(&idx.to_le_bytes());
        let candidate_id =
            RawCandidateId { slot: SlotIndex::new(idx), hash: UInt256::from(hash_bytes) };
        book.insert_received(
            candidate_id.clone(),
            make_received_chain_node(idx, root_block_id.clone(), true, Some(prev_id.clone())),
        );
        prev_id = candidate_id;
    }

    assert_eq!(
        book.resolve_parent_tip(Some(&prev_id), None),
        ParentTipResolution::Resolved(root_block_id),
        "deep empty chains past 10k must still resolve the normal tip (hard stop is 100k)",
    );
}

#[test]
fn resolve_parent_tip_deep_empty_chain_reports_true_missing_parent() {
    let mut book = CandidateBook::new();

    let missing_id = chain_id(0, 0x71);
    let referenced_block = make_block_id(1, 0x72);

    let chain_len = EMPTY_CHAIN_WARN_DEPTH + 32;
    let mut prev_id = missing_id.clone();
    for idx in 1..=chain_len {
        let mut hash_bytes = [0u8; 32];
        hash_bytes[..4].copy_from_slice(&(idx + 10_000).to_le_bytes());
        let candidate_id =
            RawCandidateId { slot: SlotIndex::new(idx), hash: UInt256::from(hash_bytes) };
        book.insert_received(
            candidate_id.clone(),
            make_received_chain_node(idx, referenced_block.clone(), true, Some(prev_id.clone())),
        );
        prev_id = candidate_id;
    }

    assert_eq!(
        book.resolve_parent_tip(Some(&prev_id), None),
        ParentTipResolution::MissingParent(missing_id),
        "deep empty chains past 10k must keep walking and report the true missing parent",
    );
}

/*
    --------------------------------------------------------------------
    candidate_data_cache accessors
    --------------------------------------------------------------------
*/

#[test]
fn cached_data_round_trip() {
    let mut book = CandidateBook::new();
    let id = make_candidate_id(11, 0xAA);
    let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];

    assert!(!book.contains_cached_data(&id));
    let prev = book.insert_cached_data(id.clone(), bytes.clone());
    assert!(prev.is_none());
    assert!(book.contains_cached_data(&id));
    assert_eq!(book.cached_data(&id), Some(&bytes));

    let replaced = book.insert_cached_data(id.clone(), vec![0xAA]);
    assert_eq!(replaced.as_deref(), Some(bytes.as_slice()));
    assert_eq!(book.cached_data(&id).map(|v| v.as_slice()), Some(&[0xAA][..]));
}

/*
    --------------------------------------------------------------------
    seen_broadcast_candidates accessors
    --------------------------------------------------------------------
*/

#[test]
fn seen_broadcast_round_trip_and_dedup() {
    let mut book = CandidateBook::new();
    let slot = SlotIndex::new(13);
    let id_first = make_candidate_id(13, 0xAA);
    let id_second = make_candidate_id(13, 0xBB);

    assert!(!book.contains_seen_broadcast(slot));

    let prev = book.insert_seen_broadcast(slot, id_first.clone());
    assert!(prev.is_none(), "first observation must return None");
    assert_eq!(book.seen_broadcast(slot), Some(&id_first));
    assert!(book.contains_seen_broadcast(slot));

    let prev = book.insert_seen_broadcast(slot, id_second.clone());
    assert_eq!(
        prev.as_ref(),
        Some(&id_first),
        "second observation must return the first id for dedup detection",
    );
    assert_eq!(
        book.seen_broadcast(slot),
        Some(&id_first),
        "first-observed id must be retained: a later observation must not overwrite it",
    );
}

/*
    --------------------------------------------------------------------
    Cross-map maintenance
    --------------------------------------------------------------------
*/

#[test]
fn prune_below_clears_auxiliary_maps_only() {
    let mut book = CandidateBook::new();
    let old_id = make_candidate_id(2, 0xAA);
    let new_id = make_candidate_id(10, 0xBB);

    // Seed all three maps with one old + one new entry.
    book.insert_received(old_id.clone(), make_received_real(2, 1, make_block_id(22, 0x02), None));
    book.insert_received(new_id.clone(), make_received_real(10, 1, make_block_id(99, 0x03), None));
    book.insert_cached_data(old_id.clone(), vec![1]);
    book.insert_cached_data(new_id.clone(), vec![2]);
    book.insert_seen_broadcast(SlotIndex::new(2), old_id.clone());
    book.insert_seen_broadcast(SlotIndex::new(10), new_id.clone());

    book.prune_below(SlotIndex::new(5));

    // received_candidates is intentionally NOT pruned (finalized-stub safety).
    assert!(book.contains_received(&old_id), "received_candidates must NOT be pruned");
    assert!(book.contains_received(&new_id));

    // candidate_data_cache pruned by slot.
    assert!(!book.contains_cached_data(&old_id));
    assert!(book.contains_cached_data(&new_id));

    // seen_broadcast_candidates pruned by slot.
    assert!(!book.contains_seen_broadcast(SlotIndex::new(2)));
    assert!(book.contains_seen_broadcast(SlotIndex::new(10)));
}

/*
    --------------------------------------------------------------------
    Debug smoke
    --------------------------------------------------------------------
*/

#[test]
fn debug_impl_includes_field_counts() {
    let mut book = CandidateBook::new();
    book.insert_received(
        make_candidate_id(1, 0xAA),
        make_received_real(1, 1, make_block_id(11, 0x01), None),
    );
    book.insert_cached_data(make_candidate_id(2, 0xBB), vec![1, 2, 3]);
    book.insert_seen_broadcast(SlotIndex::new(3), make_candidate_id(3, 0xCC));

    let dbg = format!("{:?}", book);
    assert!(dbg.contains("received_count: 1"), "Debug must expose received_count: {dbg}");
    assert!(dbg.contains("cached_data_count: 1"), "Debug must expose cached_data_count: {dbg}");
    assert!(
        dbg.contains("seen_broadcast_count: 1"),
        "Debug must expose seen_broadcast_count: {dbg}",
    );
}
