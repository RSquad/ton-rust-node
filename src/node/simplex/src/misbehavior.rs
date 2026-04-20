/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Misbehavior detection and proof collection for Simplex consensus.
//!
//! This module provides types for collecting cryptographic proofs of validator
//! misbehavior, such as sending conflicting votes for the same slot.
//!
//! # C++ Reference
//!
//! Matches `validator/consensus/simplex/misbehavior.h`:
//! - `ConflictingVotes` - two conflicting signed votes
//! - `MisbehaviorReport` - validator ID + proof
//!
//! # Current Status
//!
//! Proofs are collected and logged but not consumed downstream.
//! This matches the C++ reference implementation state (no handler for `MisbehaviorReport`).

use crate::{
    block::{SlotIndex, ValidatorIndex},
    RawVoteData,
};
use std::fmt::{Display, Formatter};
use ton_block::UInt256;

/// Cryptographic proof of validator misbehavior.
///
/// Each variant contains the serialized signed vote data needed to independently
/// verify the misbehavior without access to internal state.
///
/// The stored bytes are TL-serialized `consensus.simplex.vote` objects:
/// ```text
/// consensus.simplex.vote vote:consensus.simplex.UnsignedVote signature:bytes
/// ```
#[derive(Debug, Clone)]
pub enum MisbehaviorProof {
    /// Validator sent two different votes of the same type for the same slot.
    ///
    /// Examples:
    /// - Two `NotarizeVote` for slot 5 with different block hashes
    /// - Two `FinalizeVote` for slot 5 with different block hashes
    ///
    /// Both votes are stored as TL-serialized signed vote objects.
    ConflictingVotes {
        /// Slot where misbehavior occurred
        slot: SlotIndex,
        /// Validator who misbehaved
        validator_idx: ValidatorIndex,
        /// Vote type that conflicted
        vote_type: ConflictingVoteType,
        /// Hash of the first vote's block
        hash1: UInt256,
        /// Hash of the second vote's block
        hash2: UInt256,
        /// First signed vote (TL-serialized `consensus.simplex.vote`)
        /// Uses Arc for memory-efficient sharing
        vote1: RawVoteData,
        /// Second conflicting signed vote (TL-serialized `consensus.simplex.vote`)
        /// Uses Arc for memory-efficient sharing
        vote2: RawVoteData,
    },

    /// Validator sent votes that violate protocol invariants.
    ///
    /// Examples:
    /// - Finalize after Skip for the same slot
    /// - Notarize after Skip for the same slot (without fallback)
    /// - Notarize and Finalize for different blocks in the same slot
    ConflictingVoteTypes {
        /// Slot where misbehavior occurred
        slot: SlotIndex,
        /// Validator who misbehaved
        validator_idx: ValidatorIndex,
        /// The existing vote that was already recorded
        existing_vote: VoteDescriptor,
        /// The new conflicting vote that triggered misbehavior detection
        new_vote: VoteDescriptor,
        /// First signed vote (TL-serialized `consensus.simplex.vote`)
        /// Uses Arc for memory-efficient sharing
        vote1: RawVoteData,
        /// Second conflicting signed vote (TL-serialized `consensus.simplex.vote`)
        /// Uses Arc for memory-efficient sharing
        vote2: RawVoteData,
        /// Description of the invariant violated
        reason: ConflictReason,
    },
}

/// Descriptor for a vote type, used in misbehavior proofs.
///
/// Captures the vote type and optionally the block hash for votes that reference a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteDescriptor {
    /// Skip vote (no hash)
    Skip,
    /// Notarize vote with block hash
    Notarize(UInt256),
    /// Finalize vote with block hash
    Finalize(UInt256),
}

impl VoteDescriptor {
    /// Format as a short string for logging (e.g., "skip", "notarize:abc12345")
    pub fn format_short(&self) -> String {
        match self {
            Self::Skip => "skip".to_string(),
            Self::Notarize(hash) => format!("notarize:{}", &hash.to_hex_string()[..8]),
            Self::Finalize(hash) => format!("finalize:{}", &hash.to_hex_string()[..8]),
        }
    }
}

impl Display for VoteDescriptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format_short())
    }
}

/// Type of vote that had conflicting block hashes.
///
/// Used in `MisbehaviorProof::ConflictingVotes` to identify which vote type
/// was sent with two different block hashes (which is a protocol violation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictingVoteType {
    /// Conflicting Notarize votes for different blocks
    Notarize,
    /// Conflicting Finalize votes for different blocks
    Finalize,
}

impl ConflictingVoteType {
    /// Get the vote type as a string for logging
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Notarize => "notarize",
            Self::Finalize => "finalize",
        }
    }
}

impl Display for ConflictingVoteType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Reason for vote type conflict (invariant violation).
///
/// These map to the invariant checks in C++ `Tsentrizbirkom::check_invariants()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// Validator sent Notarize for hash A and Finalize for hash B (same slot).
    /// C++: `notarize_->vote.id != finalize_->vote.id`
    NotarizeFinalizeHashMismatch,

    /// Validator sent both Finalize and Skip for the same slot.
    /// C++: `finalize_.has_value() && skip_.has_value()`
    FinalizeAfterSkip,
}

/// Report of misbehavior with validator identity and slot context.
///
/// Matches C++ `MisbehaviorReport` struct from `bus.h`:
/// ```cpp
/// struct MisbehaviorReport {
///   PeerValidatorId id;
///   MisbehaviorRef proof;
/// };
/// ```
#[derive(Debug, Clone)]
pub struct MisbehaviorReport {
    /// Index of the misbehaving validator
    pub validator_idx: ValidatorIndex,
    /// Slot where misbehavior occurred
    pub slot: SlotIndex,
    /// Cryptographic proof of misbehavior
    pub proof: MisbehaviorProof,
}

/// Result of processing a vote in the FSM.
///
/// Replaces `Result<()>` for `on_vote` to provide richer feedback:
/// - `Applied` - vote was accepted and weights updated
/// - `Duplicate` - vote was already seen (not an error)
/// - `SlotAlreadyFinalized` - late vote for already-finalized slot (benign, normal in distributed systems)
/// - `Misbehavior` - vote violates protocol rules (proof attached)
/// - `Rejected` - vote rejected for other reasons (actual errors)
///
/// # C++ Reference
///
/// Matches C++ `AddVoteResult` from `pool.cpp`:
/// ```cpp
/// struct AddVoteResult {
///     bool is_applied;
///     std::optional<MisbehaviorRef> misbehavior;
/// };
/// ```
#[derive(Debug, Clone)]
pub enum VoteResult {
    /// Vote was applied successfully (weights updated, thresholds may have changed)
    Applied,

    /// Vote was a duplicate (same vote from same validator already seen)
    ///
    /// This is not an error - duplicates are silently ignored.
    Duplicate,

    /// Vote arrived for an already-finalized slot
    ///
    /// This is a normal case in distributed systems - late votes are expected.
    /// Not an error, just skip processing.
    SlotAlreadyFinalized,

    /// Vote was rejected due to misbehavior (proof attached)
    ///
    /// The proof contains serialized votes that can be used to verify the misbehavior.
    Misbehavior(MisbehaviorProof),

    /// Vote was rejected for non-misbehavior reasons
    ///
    /// Examples: invalid validator index, etc.
    Rejected(String),
}

impl VoteResult {
    /// Returns true if the vote was applied (not duplicate, not rejected)
    #[inline]
    pub fn is_applied(&self) -> bool {
        matches!(self, Self::Applied)
    }

    /// Returns true if the vote was a duplicate
    #[inline]
    #[cfg(test)]
    pub fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate)
    }

    /// Returns true if the vote was rejected due to misbehavior
    #[inline]
    #[cfg(test)]
    pub fn is_misbehavior(&self) -> bool {
        matches!(self, Self::Misbehavior(_))
    }

    /// Returns the misbehavior proof if this is a misbehavior result
    #[inline]
    #[cfg(test)]
    pub fn misbehavior_proof(&self) -> Option<&MisbehaviorProof> {
        match self {
            Self::Misbehavior(proof) => Some(proof),
            _ => None,
        }
    }

    /// Returns true if the vote was applied or duplicate (not an error).
    ///
    /// Compatibility method for tests migrating from `Result<()>`.
    #[inline]
    #[cfg(test)]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Applied | Self::Duplicate)
    }

    /// Returns true if the vote was rejected (misbehavior or other reason).
    ///
    /// Compatibility method for tests migrating from `Result<()>`.
    #[inline]
    #[cfg(test)]
    pub fn is_err(&self) -> bool {
        matches!(self, Self::Misbehavior(_) | Self::Rejected(_))
    }

    /// Unwraps the result, panicking if error (misbehavior or rejected).
    ///
    /// Accepts `Applied`, `Duplicate`, and `SlotAlreadyFinalized` as success.
    /// For use in tests only. Production code should match on variants.
    #[track_caller]
    #[cfg(test)]
    pub fn unwrap(self) {
        match self {
            Self::Applied | Self::Duplicate | Self::SlotAlreadyFinalized => {}
            Self::Misbehavior(proof) => {
                panic!("VoteResult::unwrap() called on Misbehavior: {}", proof)
            }
            Self::Rejected(reason) => panic!("VoteResult::unwrap() called on Rejected: {}", reason),
        }
    }

    /// Unwraps the result, panicking with a custom message if error.
    ///
    /// Accepts `Applied`, `Duplicate`, and `SlotAlreadyFinalized` as success.
    /// For use in tests only. Production code should match on variants.
    #[track_caller]
    #[cfg(test)]
    pub fn expect(self, msg: &str) {
        match self {
            Self::Applied | Self::Duplicate | Self::SlotAlreadyFinalized => {}
            other => panic!("{}: {:?}", msg, other),
        }
    }
}

impl Display for VoteResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Applied => write!(f, "applied"),
            Self::Duplicate => write!(f, "duplicate"),
            Self::SlotAlreadyFinalized => write!(f, "slot already finalized"),
            Self::Misbehavior(proof) => write!(f, "misbehavior: {}", proof),
            Self::Rejected(reason) => write!(f, "rejected: {}", reason),
        }
    }
}

impl MisbehaviorProof {
    /// Create a proof of conflicting votes (same vote type, different content).
    ///
    /// Use when a validator sends two different votes of the same type
    /// (e.g., two NotarizeVotes with different block hashes).
    ///
    /// # Arguments
    /// * `slot` - Slot where misbehavior occurred
    /// * `validator_idx` - Validator who misbehaved
    /// * `vote_type` - Type of vote that conflicted
    /// * `hash1` - Hash of the existing vote's block
    /// * `hash2` - Hash of the new conflicting vote's block
    /// * `vote1` - First signed vote (TL-serialized)
    /// * `vote2` - Second conflicting signed vote (TL-serialized)
    #[inline]
    pub fn conflicting_votes(
        slot: SlotIndex,
        validator_idx: ValidatorIndex,
        vote_type: ConflictingVoteType,
        hash1: UInt256,
        hash2: UInt256,
        vote1: RawVoteData,
        vote2: RawVoteData,
    ) -> Self {
        Self::ConflictingVotes { slot, validator_idx, vote_type, hash1, hash2, vote1, vote2 }
    }

    /// Create a proof of conflicting vote types (invariant violation).
    ///
    /// Use when a validator sends votes that violate protocol rules
    /// (e.g., Finalize after Skip).
    ///
    /// # Arguments
    /// * `slot` - Slot where misbehavior occurred
    /// * `validator_idx` - Validator who misbehaved
    /// * `existing_vote` - The existing vote that was already recorded
    /// * `new_vote` - The new conflicting vote
    /// * `vote1` - First signed vote (TL-serialized)
    /// * `vote2` - Second conflicting signed vote (TL-serialized)
    /// * `reason` - The specific invariant violated
    #[inline]
    pub fn conflicting_types(
        slot: SlotIndex,
        validator_idx: ValidatorIndex,
        existing_vote: VoteDescriptor,
        new_vote: VoteDescriptor,
        vote1: RawVoteData,
        vote2: RawVoteData,
        reason: ConflictReason,
    ) -> Self {
        Self::ConflictingVoteTypes {
            slot,
            validator_idx,
            existing_vote,
            new_vote,
            vote1,
            vote2,
            reason,
        }
    }

    /// Returns the slot where misbehavior occurred.
    pub fn slot(&self) -> SlotIndex {
        match self {
            Self::ConflictingVotes { slot, .. } => *slot,
            Self::ConflictingVoteTypes { slot, .. } => *slot,
        }
    }

    /// Returns the validator index who misbehaved.
    #[cfg(test)]
    pub fn validator_idx(&self) -> ValidatorIndex {
        match self {
            Self::ConflictingVotes { validator_idx, .. } => *validator_idx,
            Self::ConflictingVoteTypes { validator_idx, .. } => *validator_idx,
        }
    }

    /// Returns a human-readable description of the misbehavior type.
    #[cfg(test)]
    pub fn description(&self) -> &'static str {
        match self {
            Self::ConflictingVotes { .. } => "conflicting votes for same slot",
            Self::ConflictingVoteTypes { reason, .. } => reason.description(),
        }
    }

    /// Returns the size of the proof data in bytes.
    #[cfg(test)]
    pub fn size_bytes(&self) -> usize {
        match self {
            Self::ConflictingVotes { vote1, vote2, .. } => vote1.len() + vote2.len(),
            Self::ConflictingVoteTypes { vote1, vote2, .. } => vote1.len() + vote2.len(),
        }
    }

    /// Format a hash as a short hex prefix (8 characters) for logging.
    #[inline]
    pub fn format_hash_short(hash: &UInt256) -> String {
        hash.to_hex_string()[..8].to_string()
    }

    /// Get the first hash for ConflictingVotes (returns None for other variants).
    #[cfg(test)]
    pub fn hash1(&self) -> Option<&UInt256> {
        match self {
            Self::ConflictingVotes { hash1, .. } => Some(hash1),
            _ => None,
        }
    }

    /// Get the second hash for ConflictingVotes (returns None for other variants).
    #[cfg(test)]
    pub fn hash2(&self) -> Option<&UInt256> {
        match self {
            Self::ConflictingVotes { hash2, .. } => Some(hash2),
            _ => None,
        }
    }

    /// Get the existing vote descriptor for ConflictingVoteTypes (returns None for other variants).
    #[cfg(test)]
    pub fn existing_vote(&self) -> Option<&VoteDescriptor> {
        match self {
            Self::ConflictingVoteTypes { existing_vote, .. } => Some(existing_vote),
            _ => None,
        }
    }

    /// Get the new vote descriptor for ConflictingVoteTypes (returns None for other variants).
    #[cfg(test)]
    pub fn new_vote(&self) -> Option<&VoteDescriptor> {
        match self {
            Self::ConflictingVoteTypes { new_vote, .. } => Some(new_vote),
            _ => None,
        }
    }
}

impl ConflictReason {
    /// Returns a human-readable description of the conflict reason.
    pub fn description(&self) -> &'static str {
        match self {
            Self::NotarizeFinalizeHashMismatch => "notarize and finalize for different blocks",
            Self::FinalizeAfterSkip => "finalize after skip",
        }
    }
}

impl Display for MisbehaviorProof {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingVotes {
                slot,
                validator_idx,
                vote_type,
                hash1,
                hash2,
                vote1,
                vote2,
            } => {
                write!(
                    f,
                    "conflicting {} votes from v{:03} at slot {}: {} vs {} (raw={}+{} bytes)",
                    vote_type,
                    validator_idx.value(),
                    slot.value(),
                    Self::format_hash_short(hash1),
                    Self::format_hash_short(hash2),
                    vote1.len(),
                    vote2.len()
                )
            }
            Self::ConflictingVoteTypes {
                slot,
                validator_idx,
                existing_vote,
                new_vote,
                vote1,
                vote2,
                reason,
            } => {
                write!(
                    f,
                    "{} from v{:03} at slot {}: existing={}, new={} (raw={}+{} bytes)",
                    reason,
                    validator_idx.value(),
                    slot.value(),
                    existing_vote,
                    new_vote,
                    vote1.len(),
                    vote2.len()
                )
            }
        }
    }
}

impl Display for ConflictReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl Display for MisbehaviorReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Misbehavior from validator {} at slot {}: {}",
            self.validator_idx.0,
            self.slot.value(),
            self.proof
        )
    }
}

#[cfg(test)]
#[path = "tests/test_misbehavior.rs"]
mod tests;
