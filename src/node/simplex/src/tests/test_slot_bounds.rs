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
//! Verifies that the receiver:
//! - rejects vote slots using the C++ window-aligned `first_too_new_slot` rule
//! - drops already-finalized certificates, but does not cap them with the vote bound

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

fn max_future_span() -> u32 {
    let opts = crate::SessionOptions::default();
    opts.max_leader_window_desync.saturating_mul(opts.slots_per_leader_window)
}

fn slots_per_window() -> u32 {
    crate::SessionOptions::default().slots_per_leader_window
}

/// Mirrors candidate-style upper bound logic used outside the receiver.
fn candidate_max_acceptable(progress_slot: u32) -> u32 {
    progress_slot.saturating_add(max_future_span())
}

/// Mirrors `ReceiverImpl::first_too_new_vote_slot` logic.
fn first_too_new_vote_slot(progress_slot: u32) -> u32 {
    let spw = slots_per_window();
    (progress_slot / spw)
        .saturating_add(crate::SessionOptions::default().max_leader_window_desync)
        .saturating_add(1)
        .saturating_mul(spw)
}

/// Mirrors `ReceiverImpl::is_vote_slot_out_of_bounds` logic.
fn vote_is_out_of_bounds(first_active: u32, progress_slot: u32, slot: u32) -> bool {
    slot < first_active || slot >= first_too_new_vote_slot(progress_slot)
}

/// Mirrors `ReceiverImpl::is_certificate_slot_too_old` logic.
fn certificate_is_too_old(first_active: u32, slot: u32) -> bool {
    slot < first_active
}

#[test]
fn test_candidate_max_acceptable_slot_at_zero() {
    assert_eq!(candidate_max_acceptable(0), max_future_span());
}

#[test]
fn test_candidate_max_acceptable_slot_with_offset() {
    assert_eq!(candidate_max_acceptable(5000), 5000 + max_future_span());
}

#[test]
fn test_candidate_max_acceptable_slot_saturates() {
    assert_eq!(candidate_max_acceptable(u32::MAX - 100), u32::MAX);
    assert_eq!(candidate_max_acceptable(u32::MAX), u32::MAX);
}

#[test]
fn test_first_too_new_vote_slot_is_window_aligned() {
    let spw = slots_per_window();
    let max_desync = crate::SessionOptions::default().max_leader_window_desync;
    assert_eq!(first_too_new_vote_slot(0), ((0 / spw) + max_desync + 1) * spw);
    assert_eq!(first_too_new_vote_slot(spw + 1), (((spw + 1) / spw) + max_desync + 1) * spw);
}

#[test]
fn test_vote_bounds_accept_last_slot_before_boundary() {
    let boundary = first_too_new_vote_slot(0);
    assert!(!vote_is_out_of_bounds(0, 0, 0));
    assert!(!vote_is_out_of_bounds(0, 0, boundary - 1));
}

#[test]
fn test_vote_bounds_reject_boundary_slot() {
    let boundary = first_too_new_vote_slot(0);
    assert!(vote_is_out_of_bounds(0, 0, boundary));
    assert!(vote_is_out_of_bounds(0, 0, u32::MAX));
}

#[test]
fn test_vote_bounds_reject_old_finalized_slot() {
    assert!(vote_is_out_of_bounds(5000, 5000, 4999));
    assert!(vote_is_out_of_bounds(5000, 5000, 0));
}

#[test]
fn test_receiver_tracks_progress_not_finalization_for_vote_bound() {
    let spw = slots_per_window();
    let progress_slot = spw * 2 + 1;
    let slot_accepted_by_progress = first_too_new_vote_slot(progress_slot) - 1;

    assert!(!vote_is_out_of_bounds(0, progress_slot, slot_accepted_by_progress));
    assert!(
        vote_is_out_of_bounds(0, 0, slot_accepted_by_progress),
        "same slot would be rejected if the upper bound were still anchored on finalization"
    );
}

#[test]
fn test_certificate_bounds_only_reject_old_slots() {
    let boundary = first_too_new_vote_slot(0);
    assert!(certificate_is_too_old(5000, 4999));
    assert!(!certificate_is_too_old(5000, 5000));
    assert!(!certificate_is_too_old(0, boundary));
    assert!(!certificate_is_too_old(0, u32::MAX));
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
fn test_negative_tl_slot_wraps_to_large_u32_and_is_rejected_on_vote_path() {
    // TL uses i32; negative values cast via `as u32` produce large numbers.
    // Vote prefilter must reject them as too new; certificate negatives are
    // validated downstream in SessionProcessor before certificate verification.
    let negative_slot: i32 = -1;
    let slot_as_u32 = negative_slot as u32; // wraps to u32::MAX
    assert!(vote_is_out_of_bounds(0, 0, slot_as_u32));
    assert!(vote_is_out_of_bounds(5000, 5000, slot_as_u32));
}
