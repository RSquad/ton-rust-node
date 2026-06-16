/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `CandidateBook` entity
//!
//! Per-session in-memory candidate store. Owns the three primary maps that
//! used to live inline on `SessionProcessor`:
//!
//! - `received_candidates: HashMap<RawCandidateId, ReceivedCandidate>` — the
//!   canonical body/header table populated by `on_candidate_received` and
//!   read by parent-resolver, request-fallback, and recovery paths.
//! - `candidate_data_cache: HashMap<RawCandidateId, Vec<u8>>` — opaque TL
//!   bytes captured at ingress so the `RequestCandidate` query fallback can
//!   answer without re-serializing.
//! - `seen_broadcast_candidates: HashMap<SlotIndex, RawCandidateId>` — first
//!   broadcast-priority candidate observed per slot; used to dedup ingress
//!   and to drive the "candidate already observed for slot" guard.
//!
//! Also hosts the [`ReceivedCandidate`] type (formerly a private struct in
//! `session_processor.rs`).
//!
//! ## Boundary
//!
//! `CandidateBook` owns candidate data only. It does NOT run validation,
//! drive ingress, or talk to the network — every public entry point takes
//! pre-built records and returns lookup results.
//!
//! Callers (`SessionProcessor`, recovery, the receiver-resolver bridge)
//! hold an inline `CandidateBook` on `SessionProcessor` and reach it via
//! the accessor surface; tests included via `#[path]` reach the same
//! accessor surface and never inspect raw fields directly.
//!
//! ## Inner-type visibility
//!
//! `ReceivedCandidate` and its fields stay `pub(crate)` so the
//! `#[path]`-included tests in `tests/test_session_processor.rs` and the
//! future `tests/test_candidate_book.rs` can construct fixtures and inspect
//! state without widening the public API. This mirrors the Phase 3
//! `SessionRuntime` / `SlotRuntime` visibility convention.

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex},
    BlockPayloadPtr,
};
use std::{collections::HashMap, time::SystemTime};
use ton_block::{BlockIdExt, UInt256};

/// Soft warning threshold for deep parent-chain ancestry walks.
///
/// We keep processing beyond this depth because long empty tails are
/// possible on live networks during prolonged empty-block recovery
/// windows.
pub(crate) const EMPTY_CHAIN_WARN_DEPTH: u32 = 10_000;

/// Hard stop for parent-chain ancestry walks.
///
/// Defense-in-depth bound against corrupted / self-referential parent
/// metadata while still allowing very deep (but finite) empty tails.
pub(crate) const MAX_CHAIN_DEPTH: u32 = 100_000;

/// Outcome of resolving a candidate's parent-chain tip. See
/// [`CandidateBook::resolve_parent_tip`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParentTipResolution {
    /// First non-empty ancestor's block id, or the `accepted_head` seed
    /// for a candidate with no parent.
    Resolved(BlockIdExt),
    /// A parent in the chain is not yet present in the book; the caller
    /// should request it before retrying.
    MissingParent(RawCandidateId),
    /// No parent and no `accepted_head`, or the chain bottomed out in an
    /// all-empty tail with no non-empty ancestor.
    Unresolved,
    /// The walk exceeded [`MAX_CHAIN_DEPTH`]; treated as a corrupted or
    /// self-referential parent chain. The caller bumps error telemetry.
    TooDeep,
}

/// Per-session in-memory candidate store.
///
/// Owned by
/// [`SessionProcessor`](crate::session_processor::SessionProcessor) as
/// `self.candidate_book`; all access goes through the accessor methods
/// below.
#[derive(Default)]
pub(crate) struct CandidateBook {
    /// All block candidates received from the network are stored here.
    /// Used for finalization — finalization delivery always looks the
    /// payload up in this map. Reference: validator-session
    /// `session_processor.rs::blocks` field.
    received_candidates: HashMap<RawCandidateId, ReceivedCandidate>,

    /// Serialized `CandidateData` TL bytes cached at ingress so the
    /// `RequestCandidate` query fallback can answer without
    /// re-serializing. Populated in `on_candidate_received` and read by
    /// `handle_candidate_query_fallback`. Mirrors the C++
    /// `CandidateResolver::try_load_candidate_data_from_db()` fast path.
    candidate_data_cache: HashMap<RawCandidateId, Vec<u8>>,

    /// First broadcast-priority candidate id observed per slot.
    ///
    /// Matches the spirit of C++ `PrecheckCandidateBroadcast` slot-level
    /// conflict guard by allowing the ingress path to reject a second
    /// conflicting candidate id for the same slot.
    seen_broadcast_candidates: HashMap<SlotIndex, RawCandidateId>,
}

// ======================================================================
// Construction
// ======================================================================
// Build a fresh, empty candidate book.
impl CandidateBook {
    /// Construct a fresh, empty candidate book.
    pub(crate) fn new() -> Self {
        Self {
            received_candidates: HashMap::new(),
            candidate_data_cache: HashMap::new(),
            seen_broadcast_candidates: HashMap::new(),
        }
    }
}

// ======================================================================
// Received-candidate store
// ======================================================================
// The canonical body/header table keyed by `RawCandidateId`: lookup,
// presence, counting, iteration, reverse-by-block-id, and insertion.
impl CandidateBook {
    /// Look up a received candidate by id.
    pub(crate) fn received(&self, id: &RawCandidateId) -> Option<&ReceivedCandidate> {
        self.received_candidates.get(id)
    }

    /// Look up a received candidate by id for mutation.
    pub(crate) fn received_mut(&mut self, id: &RawCandidateId) -> Option<&mut ReceivedCandidate> {
        self.received_candidates.get_mut(id)
    }

    /// Whether a received candidate with this id is present.
    pub(crate) fn contains_received(&self, id: &RawCandidateId) -> bool {
        self.received_candidates.contains_key(id)
    }

    /// Number of received candidates currently held. Used by snapshot /
    /// telemetry paths.
    pub(crate) fn received_count(&self) -> usize {
        self.received_candidates.len()
    }

    /// Iterate over all received candidates.
    pub(crate) fn iter_received(
        &self,
    ) -> impl Iterator<Item = (&RawCandidateId, &ReceivedCandidate)> {
        self.received_candidates.iter()
    }

    /// Whether the candidate id has a **real** body (non-stub).
    ///
    /// Returns `false` for finalized-boundary stubs (entries seeded by
    /// `handle_block_finalized` with empty `candidate_hash_data_bytes` to
    /// serve as parent-resolution boundaries) — stubs must NOT suppress
    /// `requestCandidate` retries.
    pub(crate) fn has_real_body(&self, id: &RawCandidateId) -> bool {
        self.received_candidates
            .get(id)
            .map(|r| !r.candidate_hash_data_bytes.is_empty())
            .unwrap_or(false)
    }

    /// Reverse lookup: find the first received candidate whose `block_id`
    /// matches `block_id` and return its `RawCandidateId`. Used by the
    /// composite `resolve_candidate_id_by_block_id` resolver to cover the
    /// candidate-book half of the lookup.
    pub(crate) fn find_received_by_block_id(
        &self,
        block_id: &BlockIdExt,
    ) -> Option<RawCandidateId> {
        self.received_candidates.iter().find_map(|(candidate_id, received)| {
            (&received.block_id == block_id).then_some(candidate_id.clone())
        })
    }

    /// Insert (or replace) a received candidate. Returns the previous
    /// value, if any.
    pub(crate) fn insert_received(
        &mut self,
        id: RawCandidateId,
        record: ReceivedCandidate,
    ) -> Option<ReceivedCandidate> {
        self.received_candidates.insert(id, record)
    }
}

// ======================================================================
// Parent-chain resolution
// ======================================================================
// Walk `parent_id` links across the received-candidate store to find the
// nearest non-empty ancestor tip; pure, side-effect-free queries.
impl CandidateBook {
    /// Resolve a candidate's parent-chain tip: walk the `parent_id` links
    /// through `received_candidates` until the first non-empty ancestor and
    /// return its [`BlockIdExt`] (the C++ `event->state->as_normal()`
    /// reference). Empty ancestors are transparent and are walked through.
    ///
    /// Pure, data-only query — it never requests candidates or touches
    /// telemetry. The caller owns the validation decision, the
    /// `requestCandidate` side effect on
    /// [`ParentTipResolution::MissingParent`], and error telemetry on
    /// [`ParentTipResolution::TooDeep`].
    ///
    /// `accepted_head` is the caller-owned accepted normal head; it is
    /// consulted only for the no-parent (genesis) case and is never stored
    /// in the book.
    pub(crate) fn resolve_parent_tip(
        &self,
        parent_id: Option<&RawCandidateId>,
        accepted_head: Option<&BlockIdExt>,
    ) -> ParentTipResolution {
        let Some(parent_id) = parent_id else {
            return match accepted_head {
                Some(head) => ParentTipResolution::Resolved(head.clone()),
                None => ParentTipResolution::Unresolved,
            };
        };
        self.walk_parent_chain(parent_id)
    }

    /// Walk the parent chain starting at `start` until the first non-empty
    /// ancestor, a missing parent, or the end of an all-empty tail.
    ///
    /// Bounded by [`MAX_CHAIN_DEPTH`] with a soft warning at
    /// [`EMPTY_CHAIN_WARN_DEPTH`]; long empty tails are legal on live
    /// networks during empty-block recovery windows.
    fn walk_parent_chain(&self, start: &RawCandidateId) -> ParentTipResolution {
        let mut current = start.clone();
        let mut depth = 0u32;
        let mut depth_warned = false;
        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    target: "candidate_book",
                    "resolve_parent_tip: deep empty-parent chain depth={depth} \
                     (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}); continuing until hard \
                     limit={MAX_CHAIN_DEPTH}",
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    target: "candidate_book",
                    "resolve_parent_tip: exceeded hard MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} \
                     while resolving empty-parent chain",
                );
                return ParentTipResolution::TooDeep;
            }

            let Some(received) = self.received(&current) else {
                return ParentTipResolution::MissingParent(current);
            };
            if !received.is_empty {
                return ParentTipResolution::Resolved(received.block_id.clone());
            }
            let next_parent = received.parent_id.clone();
            match next_parent {
                Some(next) => current = next,
                None => return ParentTipResolution::Unresolved,
            }
        }
    }
}

// ======================================================================
// Auxiliary ingress caches
// ======================================================================
// Side tables populated at ingress: serialized `CandidateData` bytes for
// the `RequestCandidate` query fallback, and the first broadcast id seen
// per slot for the slot-conflict guard.
impl CandidateBook {
    /* Candidate-data cache */

    /// Look up cached serialized `CandidateData` TL bytes by id. Used by
    /// the `RequestCandidate` query fallback fast path.
    pub(crate) fn cached_data(&self, id: &RawCandidateId) -> Option<&Vec<u8>> {
        self.candidate_data_cache.get(id)
    }

    /// Insert (or replace) cached `CandidateData` bytes. Returns the
    /// previous value, if any.
    pub(crate) fn insert_cached_data(
        &mut self,
        id: RawCandidateId,
        bytes: Vec<u8>,
    ) -> Option<Vec<u8>> {
        self.candidate_data_cache.insert(id, bytes)
    }

    /* Seen-broadcast slot guard */

    /// Look up the first broadcast candidate id recorded for `slot`.
    pub(crate) fn seen_broadcast(&self, slot: SlotIndex) -> Option<&RawCandidateId> {
        self.seen_broadcast_candidates.get(&slot)
    }

    /// Record `id` as the first broadcast candidate observed for `slot`,
    /// preserving any id already stored for that slot. Returns the existing
    /// id when one was already recorded (callers use the "first observed"
    /// position to detect conflicting slot reuse) and `None` when `id` is the
    /// first observation.
    ///
    /// Unlike a plain `HashMap::insert`, a later call for the same slot does
    /// not overwrite the stored id: the first-observed candidate stays the
    /// canonical one for the slot, matching the module-level "first observed"
    /// contract and the ingress slot-conflict guard.
    pub(crate) fn insert_seen_broadcast(
        &mut self,
        slot: SlotIndex,
        id: RawCandidateId,
    ) -> Option<RawCandidateId> {
        match self.seen_broadcast_candidates.entry(slot) {
            std::collections::hash_map::Entry::Occupied(existing) => Some(existing.get().clone()),
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(id);
                None
            }
        }
    }
}

// ======================================================================
// Maintenance / GC
// ======================================================================
// Slot-based pruning of the auxiliary maps for finalized windows.
impl CandidateBook {
    /// Prune auxiliary maps so that only entries for `slot >= up_to_slot`
    /// remain. Used by `cleanup_old_candidates` to garbage-collect state
    /// for finalized slots.
    ///
    /// `received_candidates` is intentionally NOT pruned here yet: a
    /// finalized-stub-safe GC for that map is a separate follow-up.
    pub(crate) fn prune_below(&mut self, up_to_slot: SlotIndex) {
        self.candidate_data_cache.retain(|id, _| id.slot >= up_to_slot);
        self.seen_broadcast_candidates.retain(|slot, _| *slot >= up_to_slot);
    }
}

impl std::fmt::Debug for CandidateBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandidateBook")
            .field("received_count", &self.received_candidates.len())
            .field("cached_data_count", &self.candidate_data_cache.len())
            .field("seen_broadcast_count", &self.seen_broadcast_candidates.len())
            .finish_non_exhaustive()
    }
}

/*
    ========================================================================
    ReceivedCandidate — the canonical body/header record
    ========================================================================
*/

/// A received candidate from the network.
///
/// All block candidates received from the network are stored under their
/// `RawCandidateId` in [`CandidateBook::received_candidates`]; finalization
/// delivery always reads payload bytes from this table. Reference:
/// validator-session `session_processor.rs::blocks` field.
#[derive(Clone)]
pub(crate) struct ReceivedCandidate {
    /// Slot number (informational; the first slot where this candidate
    /// appeared).
    pub(crate) slot: SlotIndex,
    /// Source validator index (leader for this slot).
    pub(crate) source_idx: ValidatorIndex,
    /// Serialized `CandidateHashData` TL bytes. Used for building
    /// `BlockSignaturesSimplex` during finalization delivery.
    ///
    /// `SHA256(candidate_hash_data_bytes)` equals the candidate id hash
    /// (`RawCandidateId::hash`).
    pub(crate) candidate_hash_data_bytes: Vec<u8>,
    /// Full block id (`workchain`, `shard`, `seqno`, `root_hash`,
    /// `file_hash`). Used for seqno validation during batch finalization.
    pub(crate) block_id: BlockIdExt,
    /// Block root hash.
    pub(crate) root_hash: UInt256,
    /// Block file hash.
    pub(crate) file_hash: UInt256,
    /// Block data (extracted from TL, ready for callback).
    ///
    /// For non-empty blocks this carries `BlockCandidate.data`; for empty
    /// blocks it is an empty vec.
    pub(crate) data: BlockPayloadPtr,
    /// Collated data (extracted from TL).
    pub(crate) collated_data: BlockPayloadPtr,
    /// Exact generation time extracted from `ConsensusExtraData`, if
    /// available.
    pub(crate) gen_utime_ms: Option<u64>,
    /// Time when the candidate was received (for latency tracking).
    pub(crate) receive_time: SystemTime,
    /// `true` if this is an empty block (inherits the parent's
    /// `BlockIdExt`).
    pub(crate) is_empty: bool,
    /// Parent candidate id (`None` for genesis / first in epoch).
    ///
    /// Used for empty-parent tip checks, explicit-parent collation hints,
    /// and restart-seeded metadata lookups.
    pub(crate) parent_id: Option<RawCandidateId>,
}

// ======================================================================
// Tests
// ======================================================================
// Test-only seams: a presence/retain surface used exclusively by the
// `#[path]`-included unit tests. Kept out of the production impls so the
// live API stays minimal.
#[cfg(test)]
impl CandidateBook {
    /// Retain only the received candidates for which `f` returns `true`.
    /// Mirrors `HashMap::retain`; the finalized-stub-safe production GC for
    /// `received_candidates` is a separate follow-up, so this is currently
    /// exercised only by tests.
    fn retain_received(&mut self, f: impl FnMut(&RawCandidateId, &mut ReceivedCandidate) -> bool) {
        self.received_candidates.retain(f);
    }

    /// Whether cached serialized `CandidateData` bytes are present for this id.
    pub(crate) fn contains_cached_data(&self, id: &RawCandidateId) -> bool {
        self.candidate_data_cache.contains_key(id)
    }

    /// Whether a broadcast candidate has been recorded for `slot`.
    pub(crate) fn contains_seen_broadcast(&self, slot: SlotIndex) -> bool {
        self.seen_broadcast_candidates.contains_key(&slot)
    }
}

// Tests live in a sibling file but are included directly via `#[path]` so
// they can reach the `pub(crate)` accessor surface and inner structs without
// widening visibility. Mirrors `session_runtime.rs` / `session_callbacks.rs`.
#[cfg(test)]
#[path = "tests/test_candidate_book.rs"]
mod tests;
