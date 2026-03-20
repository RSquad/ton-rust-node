/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for receiver-level slot bounds checking
//!
//! Verifies that the receiver rejects far-future and already-finalized slots
//! before expensive operations (signature verification, dedup HashMap insertion).

use super::*;
use ton_api::{
    ton::consensus::{
        candidateid::CandidateId,
        simplex::{
            unsignedvote::{NotarizeVote, SkipVote},
            vote::Vote as TlVote,
        },
    },
    IntoBoxed,
};
use ton_block::UInt256;

/// Mirrors `ReceiverImpl::max_acceptable_slot` logic.
fn max_acceptable(first_active: u32) -> u32 {
    first_active.saturating_add(MAX_FUTURE_SLOTS)
}

/// Mirrors `ReceiverImpl::is_slot_out_of_bounds` logic.
fn is_out_of_bounds(first_active: u32, slot: u32) -> bool {
    slot < first_active || slot > max_acceptable(first_active)
}

#[test]
fn test_max_acceptable_slot_at_zero() {
    assert_eq!(max_acceptable(0), MAX_FUTURE_SLOTS);
}

#[test]
fn test_max_acceptable_slot_with_offset() {
    assert_eq!(max_acceptable(5000), 5000 + MAX_FUTURE_SLOTS);
}

#[test]
fn test_max_acceptable_slot_saturates() {
    assert_eq!(max_acceptable(u32::MAX - 100), u32::MAX);
    assert_eq!(max_acceptable(u32::MAX), u32::MAX);
}

#[test]
fn test_bounds_rejects_far_future_vote() {
    assert!(is_out_of_bounds(0, MAX_FUTURE_SLOTS + 1));
    assert!(is_out_of_bounds(0, u32::MAX));
}

#[test]
fn test_bounds_accepts_boundary_slot() {
    assert!(!is_out_of_bounds(0, MAX_FUTURE_SLOTS));
    assert!(!is_out_of_bounds(0, 0));
}

#[test]
fn test_bounds_rejects_old_finalized_slot() {
    assert!(is_out_of_bounds(5000, 4999));
    assert!(is_out_of_bounds(5000, 0));
}

#[test]
fn test_bounds_accepts_range_with_advanced_finalization() {
    assert!(!is_out_of_bounds(5000, 5000));
    assert!(!is_out_of_bounds(5000, 5000 + MAX_FUTURE_SLOTS));
}

#[test]
fn test_bounds_rejects_beyond_range_with_advanced_finalization() {
    assert!(is_out_of_bounds(5000, 5001 + MAX_FUTURE_SLOTS));
}

#[test]
fn test_get_vote_slot_extracts_notarize() {
    let vote = TlVote {
        vote: (NotarizeVote {
            id: (CandidateId { slot: 42, hash: UInt256::default().into() }).into_boxed(),
        })
        .into_boxed(),
        signature: Vec::new().into(),
    }
    .into_boxed();

    assert_eq!(ReceiverImpl::get_vote_slot(&vote), 42);
}

#[test]
fn test_get_vote_slot_extracts_skip() {
    let vote = TlVote { vote: (SkipVote { slot: 99 }).into_boxed(), signature: Vec::new().into() }
        .into_boxed();

    assert_eq!(ReceiverImpl::get_vote_slot(&vote), 99);
}

#[test]
fn test_negative_tl_slot_wraps_to_large_u32_and_is_rejected() {
    // TL uses i32; negative values cast via `as u32` produce large numbers
    // that must be rejected by the bounds check.
    let negative_slot: i32 = -1;
    let slot_as_u32 = negative_slot as u32; // wraps to u32::MAX
    assert!(is_out_of_bounds(0, slot_as_u32));
    assert!(is_out_of_bounds(5000, slot_as_u32));
}
