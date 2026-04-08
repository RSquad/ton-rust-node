/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Block candidate types for Simplex consensus
//!
//! This module implements the block candidate type hierarchy for Simplex consensus,
//! matching the C++ reference implementation (`consensus-types.h`, `consensus-types.cpp`).
//!
//! ## Index Newtypes (Type Safety)
//!
//! To prevent parameter mixing bugs, this module provides newtype wrappers:
//! - [`SlotIndex`] - Consensus slot number (0-based round)
//! - [`WindowIndex`] - Leader window index
//! - [`ValidatorIndex`] - Validator position in validator set
//!
//! ## Type Hierarchy
//!
//! ```text
//! RawCandidateId (hash-based identifier)
//! ├── slot: SlotIndex               // Slot number (newtype)
//! └── hash: UInt256                 // SHA256 of TL CandidateHashData
//!
//! CandidateId (resolved identifier with BlockIdExt)
//! ├── slot: SlotIndex               // Slot number
//! ├── hash: UInt256                 // Same hash as RawCandidateId
//! └── block: BlockIdExt             // Resolved block ID
//!
//! RawCandidate (from network, parent may be unresolved)
//! ├── id: RawCandidateId            // Hash-based ID
//! ├── parent_id: Option<RawCandidateId>  // Parent (None for genesis)
//! ├── leader: ValidatorIndex        // Leader validator index (newtype)
//! ├── block: Option<BlockCandidate> // None for empty blocks
//! ├── referenced_block: Option<BlockIdExt>  // For empty: inherited BlockIdExt
//! └── signature: Vec<u8>            // Ed25519 signature
//!
//! Candidate (resolved, parent fully known)
//! ├── id: CandidateId               // Resolved ID
//! ├── parent_id: Option<CandidateId>  // Resolved parent
//! ├── leader: ValidatorIndex        // Leader validator index
//! ├── block: Option<BlockCandidate> // None for empty blocks
//! └── signature: Vec<u8>            // Ed25519 signature
//! ```
//!
//! ## Empty Blocks (C++ Reference: consensus-types.cpp)
//!
//! Empty blocks are used for finalization recovery when the chain is behind.
//! They have `block = None` and must have a parent.
//!
//! ### C++ Structure:
//! ```cpp
//! std::variant<BlockIdExt, BlockCandidate> block;
//! // For empty: variant holds parent's BlockIdExt
//! // For non-empty: variant holds BlockCandidate
//! ```
//!
//! ### Rust Representation:
//! - `block: Option<BlockCandidate>` - None for empty blocks
//! - `referenced_block: Option<BlockIdExt>` - Parent's BlockIdExt for empty blocks
//!
//! ### Invariants:
//! - **Invariant 1**: Either `block.is_some()` OR `parent_id.is_some()`
//! - **Invariant 2**: If `block.is_none()`, then `referenced_block.is_some()`
//! - **First block in epoch cannot be empty**
//!
//! ## Hash Computation
//!
//! Candidate ID hash uses **different TL types** based on block type:
//!
//! - **Non-empty blocks**: `candidateHashDataOrdinary(block, collated_file_hash, parent:CandidateParent)`
//! - **Empty blocks**: `candidateHashDataEmpty(block, parent:CandidateId)` - note: parent is CandidateId directly!
//!

use crate::{PrivateKey, PublicKey, SessionId};
use std::{
    fmt,
    ops::{Add, AddAssign, Mul, Rem, Sub},
    sync::Arc,
};
use ton_api::{
    ton::{
        consensus::{
            candidatedata::{Block as CandidateDataBlock, Empty as CandidateDataEmpty},
            candidateid::CandidateId as TlCandidateId,
            candidateparent::CandidateParent as TlCandidateParent,
            CandidateData, CandidateParent as TlCandidateParentBoxed,
        },
        validator_session::candidate::{Candidate as TlCandidate, CompressedCandidate},
    },
    IntoBoxed,
};
use ton_block::{error, fail, BlockIdExt, Result, ShardIdent, UInt256};

/*
    Index Newtypes

    These provide type safety to prevent parameter mixing bugs.
*/

/// Consensus slot index
///
/// Represents a position in the consensus round sequence.
/// Different from block seqno (blockchain sequence number).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotIndex(pub u32);

impl SlotIndex {
    /// Create a new slot index
    #[inline]
    pub fn new(slot: u32) -> Self {
        Self(slot)
    }

    /// Get the raw u32 value
    #[inline]
    pub fn value(self) -> u32 {
        self.0
    }

    /// Get the window index for this slot
    #[inline]
    pub fn window_index(self, slots_per_window: u32) -> WindowIndex {
        WindowIndex(self.0 / slots_per_window)
    }

    /// Get the offset within the window (0-based)
    #[inline]
    pub fn offset_in_window(self, slots_per_window: u32) -> u32 {
        self.0 % slots_per_window
    }

    /// Check if this is the first slot in a window
    #[inline]
    pub fn is_first_in_window(self, slots_per_window: u32) -> bool {
        self.offset_in_window(slots_per_window) == 0
    }

    /// Check if this is the last slot in a window
    #[inline]
    pub fn is_last_in_window(self, slots_per_window: u32) -> bool {
        self.offset_in_window(slots_per_window) == slots_per_window - 1
    }

    /// Get the first slot of this slot's window
    #[inline]
    pub fn window_start(self, slots_per_window: u32) -> SlotIndex {
        SlotIndex((self.0 / slots_per_window) * slots_per_window)
    }
}

impl From<u32> for SlotIndex {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl From<SlotIndex> for u32 {
    fn from(v: SlotIndex) -> Self {
        v.0
    }
}

impl From<i32> for SlotIndex {
    fn from(v: i32) -> Self {
        Self(v as u32)
    }
}

impl From<SlotIndex> for i32 {
    fn from(v: SlotIndex) -> Self {
        v.0 as i32
    }
}

impl fmt::Debug for SlotIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

impl fmt::Display for SlotIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

impl Add<u32> for SlotIndex {
    type Output = SlotIndex;

    #[inline]
    fn add(self, rhs: u32) -> Self::Output {
        SlotIndex(self.0 + rhs)
    }
}

impl Sub<u32> for SlotIndex {
    type Output = SlotIndex;

    #[inline]
    fn sub(self, rhs: u32) -> Self::Output {
        SlotIndex(self.0.saturating_sub(rhs))
    }
}

impl Sub<SlotIndex> for SlotIndex {
    type Output = u32;

    #[inline]
    fn sub(self, rhs: SlotIndex) -> Self::Output {
        self.0.saturating_sub(rhs.0)
    }
}

impl AddAssign<u32> for SlotIndex {
    #[inline]
    fn add_assign(&mut self, rhs: u32) {
        self.0 += rhs;
    }
}

impl Rem<u32> for SlotIndex {
    type Output = u32;

    #[inline]
    fn rem(self, rhs: u32) -> Self::Output {
        self.0 % rhs
    }
}

/// Leader window index
///
/// Windows group consecutive slots under a single leader.
/// Computed as: slot / slots_per_leader_window
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowIndex(pub u32);

impl WindowIndex {
    /// Create a new window index
    #[inline]
    pub fn new(idx: u32) -> Self {
        Self(idx)
    }

    /// Get the raw u32 value
    #[inline]
    pub fn value(self) -> u32 {
        self.0
    }

    /// Get the first slot of this window
    #[inline]
    pub fn first_slot(self, slots_per_window: u32) -> SlotIndex {
        SlotIndex(self.0 * slots_per_window)
    }

    /// Get the last slot of this window
    #[inline]
    pub fn last_slot(self, slots_per_window: u32) -> SlotIndex {
        SlotIndex(self.0 * slots_per_window + slots_per_window - 1)
    }

    /// Alias for `first_slot` - get the first slot of this window
    #[inline]
    pub fn window_start(self, slots_per_window: u32) -> SlotIndex {
        self.first_slot(slots_per_window)
    }
}

impl From<u32> for WindowIndex {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl From<WindowIndex> for u32 {
    fn from(v: WindowIndex) -> Self {
        v.0
    }
}

impl fmt::Debug for WindowIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "w{}", self.0)
    }
}

impl fmt::Display for WindowIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "w{}", self.0)
    }
}

impl Add<u32> for WindowIndex {
    type Output = WindowIndex;

    #[inline]
    fn add(self, rhs: u32) -> Self::Output {
        WindowIndex(self.0 + rhs)
    }
}

impl Sub<u32> for WindowIndex {
    type Output = WindowIndex;

    #[inline]
    fn sub(self, rhs: u32) -> Self::Output {
        WindowIndex(self.0.saturating_sub(rhs))
    }
}

impl Sub<WindowIndex> for WindowIndex {
    type Output = u32;

    #[inline]
    fn sub(self, rhs: WindowIndex) -> Self::Output {
        self.0.saturating_sub(rhs.0)
    }
}

impl AddAssign<u32> for WindowIndex {
    #[inline]
    fn add_assign(&mut self, rhs: u32) {
        self.0 += rhs;
    }
}

impl Mul<u32> for WindowIndex {
    type Output = SlotIndex;

    #[inline]
    fn mul(self, rhs: u32) -> Self::Output {
        SlotIndex(self.0 * rhs)
    }
}

/// Validator index (position in validator set)
///
/// 0-based index into the validator set.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValidatorIndex(pub u32);

impl ValidatorIndex {
    /// Create a new validator index
    #[inline]
    pub fn new(idx: u32) -> Self {
        Self(idx)
    }

    /// Get the raw u32 value
    #[inline]
    pub fn value(self) -> u32 {
        self.0
    }

    /// Check if this validator index is valid for the given number of validators
    ///
    /// # Example
    /// ```ignore
    /// let idx = ValidatorIndex::new(5);
    /// assert!(idx.is_valid(10));  // 5 < 10
    /// assert!(!idx.is_valid(5));  // 5 >= 5
    /// ```
    #[inline]
    pub fn is_valid(self, num_validators: usize) -> bool {
        (self.0 as usize) < num_validators
    }

    /// Check if this validator index is out of bounds for the given number of validators
    ///
    /// Opposite of `is_valid()`. Returns true if `validator_idx >= num_validators`.
    #[inline]
    pub fn is_out_of_bounds(self, num_validators: usize) -> bool {
        (self.0 as usize) >= num_validators
    }
}

impl From<u32> for ValidatorIndex {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl From<ValidatorIndex> for u32 {
    fn from(v: ValidatorIndex) -> Self {
        v.0
    }
}

impl From<usize> for ValidatorIndex {
    fn from(v: usize) -> Self {
        Self(v as u32)
    }
}

impl From<ValidatorIndex> for usize {
    fn from(v: ValidatorIndex) -> Self {
        v.0 as usize
    }
}

impl fmt::Debug for ValidatorIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{:03}", self.0)
    }
}

impl fmt::Display for ValidatorIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{:03}", self.0)
    }
}

impl Add<u32> for ValidatorIndex {
    type Output = ValidatorIndex;

    #[inline]
    fn add(self, rhs: u32) -> Self::Output {
        ValidatorIndex(self.0 + rhs)
    }
}

impl Sub<u32> for ValidatorIndex {
    type Output = ValidatorIndex;

    #[inline]
    fn sub(self, rhs: u32) -> Self::Output {
        ValidatorIndex(self.0.saturating_sub(rhs))
    }
}

impl Sub<ValidatorIndex> for ValidatorIndex {
    type Output = u32;

    #[inline]
    fn sub(self, rhs: ValidatorIndex) -> Self::Output {
        self.0.saturating_sub(rhs.0)
    }
}

impl AddAssign<u32> for ValidatorIndex {
    #[inline]
    fn add_assign(&mut self, rhs: u32) {
        self.0 += rhs;
    }
}

impl Rem<u32> for ValidatorIndex {
    type Output = u32;

    #[inline]
    fn rem(self, rhs: u32) -> Self::Output {
        self.0 % rhs
    }
}

/// Raw candidate ID (hash-based, before parent resolution)
///
/// Reference: C++ `RawCandidateId` in `consensus-types.h`
///
/// The hash is computed from TL-serialized `consensus.CandidateHashData`:
/// - For non-empty blocks: `candidateHashDataOrdinary(block, collated_file_hash, parent:CandidateParent)`
/// - For empty blocks: `candidateHashDataEmpty(block, parent:CandidateId)` - note different parent type!
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawCandidateId {
    /// Slot number (consensus round)
    pub slot: SlotIndex,

    /// SHA256 hash of TL-serialized CandidateHashData
    pub hash: UInt256,
}

impl RawCandidateId {
    /// Create candidate ID for non-empty blocks
    ///
    /// Uses `candidateHashDataOrdinary` TL type with parent as `CandidateParent`.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `block` - Block candidate
    /// * `parent` - Parent candidate ID (None for first block in epoch)
    pub fn create(
        slot: SlotIndex,
        block: Option<&BlockCandidate>,
        parent: Option<&RawCandidateId>,
    ) -> Self {
        let hash = crate::utils::compute_candidate_id_hash(
            slot,
            block.map(|b| &b.id),
            block.map(|b| &b.collated_file_hash),
            parent.map(|p| (p.slot, &p.hash)),
        );
        Self { slot, hash }
    }

    /// Create candidate ID for empty blocks
    ///
    /// Uses `candidateHashDataEmpty` TL type with parent as `CandidateId` (not CandidateParent).
    /// This matches C++ `CandidateId::create_hash_data()` in `consensus-types.cpp`.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `referenced_block` - The inherited BlockIdExt from parent
    /// * `parent` - Parent candidate ID (REQUIRED for empty blocks)
    pub fn create_empty(
        slot: SlotIndex,
        referenced_block: &BlockIdExt,
        parent: &RawCandidateId,
    ) -> Self {
        let hash = crate::utils::compute_candidate_id_hash_empty(
            referenced_block,
            (parent.slot, &parent.hash),
        );
        Self { slot, hash }
    }

    /// Convert to CandidateParentInfo for FSM operations
    #[allow(dead_code)]
    pub fn as_parent_info(&self) -> CandidateParentInfo {
        CandidateParentInfo { slot: self.slot, hash: self.hash.clone() }
    }

    /// Create from slot and hash directly (for deserialization)
    #[allow(dead_code)]
    pub fn from_parts(slot: SlotIndex, hash: UInt256) -> Self {
        Self { slot, hash }
    }
}

impl fmt::Debug for RawCandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RawCandidateId {{ slot: {}, hash: {} }}", self.slot, self.hash.to_hex_string())
    }
}

impl fmt::Display for RawCandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{slot={}, hash={}}}", self.slot, &self.hash.to_hex_string()[..8])
    }
}

/// Type alias for optional parent (None = genesis/first in epoch)
#[allow(dead_code)]
pub type RawParentId = Option<RawCandidateId>;

/// Resolved candidate ID with full BlockIdExt
///
/// Created from RawCandidateId once the parent is resolved.
/// For empty blocks, the block field is inherited from parent.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CandidateId {
    /// Slot number
    pub slot: SlotIndex,

    /// Hash (same as RawCandidateId.hash)
    pub hash: UInt256,

    /// Resolved block ID
    /// - For non-empty blocks: from BlockCandidate
    /// - For empty blocks: inherited from parent
    pub block: BlockIdExt,
}

impl CandidateId {
    /// Create from RawCandidateId and block info
    pub fn from_raw(raw: &RawCandidateId, block: BlockIdExt) -> Self {
        Self { slot: raw.slot, hash: raw.hash.clone(), block }
    }

    /// Convert to RawCandidateId (losing block info)
    #[allow(dead_code)]
    pub fn to_raw(&self) -> RawCandidateId {
        RawCandidateId { slot: self.slot, hash: self.hash.clone() }
    }

    /// Convert to CandidateParentInfo for FSM operations
    #[allow(dead_code)]
    pub fn as_parent_info(&self) -> CandidateParentInfo {
        CandidateParentInfo { slot: self.slot, hash: self.hash.clone() }
    }
}

impl From<CandidateId> for RawCandidateId {
    fn from(id: CandidateId) -> Self {
        RawCandidateId { slot: id.slot, hash: id.hash }
    }
}

impl From<&CandidateId> for RawCandidateId {
    fn from(id: &CandidateId) -> Self {
        RawCandidateId { slot: id.slot, hash: id.hash.clone() }
    }
}

impl fmt::Debug for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CandidateId {{ slot: {}, hash: {}, block: {} }}",
            self.slot,
            self.hash.to_hex_string(),
            self.block
        )
    }
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{slot={}, hash={}, block={}}}",
            self.slot,
            &self.hash.to_hex_string()[..8],
            self.block.seq_no
        )
    }
}

/// Type alias for optional resolved parent
#[allow(dead_code)]
pub type ParentId = Option<CandidateId>;

/// Block candidate data
///
/// Contains the actual block data and metadata.
#[derive(Clone, Debug)]
pub struct BlockCandidate {
    /// Block ID (shard, seqno, root_hash, file_hash)
    pub id: BlockIdExt,

    /// Hash of collated data
    pub collated_file_hash: UInt256,

    /// Block data
    pub data: Vec<u8>,

    /// Collated data
    pub collated_data: Vec<u8>,

    /// Creator's public key (leader)
    #[allow(dead_code)]
    pub creator: PublicKey,
}

/// Block content variant for RawCandidate
///
/// Matches C++ `std::variant<BlockIdExt, BlockCandidate>`.
///
/// # Variants
///
/// - `Empty(BlockIdExt)`: For empty blocks - the parent's BlockIdExt being re-signed
/// - `NonEmpty(BlockCandidate)`: For non-empty blocks - full block data
#[derive(Clone, Debug)]
pub enum CandidateBlockData {
    /// Empty block - contains the parent's BlockIdExt that is being re-signed
    /// for finalization recovery when the chain is behind
    Empty(BlockIdExt),

    /// Non-empty block - contains full block candidate data
    NonEmpty(BlockCandidate),
}

impl CandidateBlockData {
    /// Check if this is an empty block
    pub fn is_empty(&self) -> bool {
        matches!(self, CandidateBlockData::Empty(_))
    }

    /// Get BlockIdExt from the block data
    ///
    /// - For empty blocks: returns the referenced BlockIdExt
    /// - For non-empty blocks: returns the block's id
    pub fn block_id(&self) -> &BlockIdExt {
        match self {
            CandidateBlockData::Empty(id) => id,
            CandidateBlockData::NonEmpty(block) => &block.id,
        }
    }

    /// Get BlockCandidate if this is a non-empty block
    pub fn as_block(&self) -> Option<&BlockCandidate> {
        match self {
            CandidateBlockData::Empty(_) => None,
            CandidateBlockData::NonEmpty(block) => Some(block),
        }
    }

    /// Get BlockIdExt if this is an empty block
    #[allow(dead_code)]
    pub fn as_empty(&self) -> Option<&BlockIdExt> {
        match self {
            CandidateBlockData::Empty(id) => Some(id),
            CandidateBlockData::NonEmpty(_) => None,
        }
    }
}

/// Raw candidate from network (parent may be unresolved)
///
/// # Invariant
///
/// For empty blocks (CandidateBlockData::Empty), `parent_id` MUST be Some.
/// First block in epoch CANNOT be empty (must have block data).
///
/// # C++ Reference
///
/// Matches C++ `RawCandidate` structure from `consensus-types.h`:
/// ```cpp
/// struct RawCandidate {
///   CandidateId id;
///   RawParentId parent_id;
///   PeerValidatorId leader;
///   std::variant<BlockIdExt, BlockCandidate> block;
///   td::BufferSlice signature;
/// };
/// ```
///
/// # Serialization
///
/// - Non-empty blocks: `consensus.block` TL variant
/// - Empty blocks: `consensus.empty` TL variant
#[derive(Clone, Debug)]
pub struct RawCandidate {
    /// Hash-based candidate ID
    pub id: RawCandidateId,

    /// Parent candidate ID (None for genesis/first in epoch)
    /// MUST be Some for empty blocks
    pub parent_id: Option<RawCandidateId>,

    /// Leader validator index
    pub leader: ValidatorIndex,

    /// Block data - either empty (BlockIdExt) or non-empty (BlockCandidate)
    /// Matches C++ `std::variant<BlockIdExt, BlockCandidate>`
    pub block: CandidateBlockData,

    /// Ed25519 signature over session-scoped candidate ID
    /// Signed data: consensus.dataToSign(session_id, consensus.candidateId(slot, hash))
    pub signature: Vec<u8>,
}

#[allow(dead_code)]
impl RawCandidate {
    /// Create a new non-empty RawCandidate
    ///
    /// For empty blocks, use `new_empty()` instead.
    ///
    /// # Arguments
    ///
    /// * `id` - Candidate ID (hash computed using `candidateHashDataOrdinary`)
    /// * `parent_id` - Parent candidate ID (optional for first block in epoch)
    /// * `leader` - Leader validator index
    /// * `block` - Block candidate data
    /// * `signature` - Ed25519 signature
    pub fn new(
        id: RawCandidateId,
        parent_id: Option<RawCandidateId>,
        leader: ValidatorIndex,
        block: BlockCandidate,
        signature: Vec<u8>,
    ) -> Self {
        Self { id, parent_id, leader, block: CandidateBlockData::NonEmpty(block), signature }
    }

    /// Create a new empty RawCandidate
    ///
    /// Empty blocks are used for finalization recovery when the chain is behind.
    ///
    /// # Arguments
    ///
    /// * `id` - Candidate ID (hash computed using `candidateHashDataEmpty`)
    /// * `parent_id` - Parent candidate ID (REQUIRED for empty blocks)
    /// * `leader` - Leader validator index
    /// * `referenced_block` - The inherited BlockIdExt from parent
    /// * `signature` - Ed25519 signature
    ///
    /// # Panics
    ///
    /// Panics if parent_id is None (empty blocks MUST have parent).
    /// Note: This function takes parent_id by value (not Option) to enforce this at compile time.
    pub fn new_empty(
        id: RawCandidateId,
        parent_id: RawCandidateId,
        leader: ValidatorIndex,
        referenced_block: BlockIdExt,
        signature: Vec<u8>,
    ) -> Self {
        // Invariant: Empty blocks MUST have a parent
        // This is enforced at compile time by taking parent_id by value,
        // but we add a debug_assert for documentation purposes
        debug_assert!(
            true, // parent_id is not Option, so it's always present
            "RawCandidate invariant: empty block must have parent"
        );

        Self {
            id,
            parent_id: Some(parent_id),
            leader,
            block: CandidateBlockData::Empty(referenced_block),
            signature,
        }
    }

    /// Check if this is an empty block
    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }

    /// Resolve to Candidate once parent is known
    ///
    /// # Block ID Resolution
    ///
    /// - For non-empty blocks: use block.id from CandidateBlockData::NonEmpty
    /// - For empty blocks: use BlockIdExt from CandidateBlockData::Empty
    ///
    /// # Arguments
    ///
    /// * `parent` - Optional full parent CandidateId (used for parent_id resolution)
    pub fn resolve(&self, parent: Option<&CandidateId>) -> Result<Candidate> {
        // Get block_id from the enum - works for both empty and non-empty
        let block_id = self.block.block_id().clone();

        // For parent_id, prefer explicit parameter if provided, otherwise use self.parent_id
        // The explicit parameter is needed for empty blocks where we need the full CandidateId
        // For non-empty blocks, we can construct CandidateId from self.parent_id with a placeholder BlockIdExt
        // (FSM only uses slot and hash from parent, not the BlockIdExt)
        let resolved_parent_id = if parent.is_some() {
            parent.cloned()
        } else {
            // Convert self.parent_id (RawCandidateId) to CandidateId with placeholder BlockIdExt
            self.parent_id
                .as_ref()
                .map(|raw_parent| CandidateId::from_raw(raw_parent, BlockIdExt::default()))
        };

        // Extract block for Candidate (Option<BlockCandidate>)
        let block_opt = self.block.as_block().cloned();

        Ok(Candidate::new(
            CandidateId::from_raw(&self.id, block_id),
            resolved_parent_id,
            self.leader,
            block_opt,
            self.signature.clone(),
        ))
    }

    /// Deserialize from TL bytes
    ///
    /// # Arguments
    ///
    /// * `data` - TL-serialized consensus.candidate
    /// * `session_id` - Session ID for signature verification
    /// * `leader_key` - Leader's public key for signature verification
    /// * `leader_idx` - Leader's validator index
    /// * `shard` - Shard identifier for BlockIdExt construction
    /// * `max_size` - Maximum candidate size (for decompression limit)
    pub fn deserialize(
        data: &[u8],
        session_id: &SessionId,
        leader_key: &PublicKey,
        leader_idx: ValidatorIndex,
        shard: &ShardIdent,
        max_size: usize,
    ) -> Result<Self> {
        // Parse TL object
        let data_vec = data.to_vec();
        let candidate_tl =
            consensus_common::utils::deserialize_tl_boxed_object::<CandidateData>(&data_vec)?;

        Self::from_tl(&candidate_tl, session_id, leader_key, leader_idx, shard, max_size)
    }

    /// Create from already-parsed TL object
    ///
    /// Avoids serialization/deserialization when TL object is already available.
    /// Verifies signature and extracts block and parent.
    ///
    /// # Arguments
    ///
    /// * `candidate_tl` - Parsed TL candidate object
    /// * `session_id` - Session ID for signature verification
    /// * `leader_key` - Leader's public key for signature verification
    /// * `leader_idx` - Leader validator index
    /// * `shard` - Shard identifier
    /// * `max_size` - Maximum block + collated data size (checked by caller)
    pub fn from_tl(
        candidate_tl: &CandidateData,
        session_id: &SessionId,
        leader_key: &PublicKey,
        leader_idx: ValidatorIndex,
        shard: &ShardIdent,
        max_size: usize,
    ) -> Result<Self> {
        // Extract parent
        let parent_id = Self::extract_parent(candidate_tl)?;

        // Extract block data - returns CandidateBlockData
        let block_data = Self::extract_block_data(candidate_tl, leader_key, shard, max_size)?;

        // Validate invariant: empty blocks must have parent
        if block_data.is_empty() && parent_id.is_none() {
            fail!("Empty candidate must have a parent")
        }

        // Validate slot is non-negative (TL uses i32)
        let raw_slot = *candidate_tl.slot();
        if raw_slot < 0 {
            fail!("Negative slot {} in candidate TL", raw_slot);
        }
        let slot = SlotIndex(raw_slot as u32);
        let id = match &block_data {
            CandidateBlockData::NonEmpty(block) => {
                RawCandidateId::create(slot, Some(block), parent_id.as_ref())
            }
            CandidateBlockData::Empty(referenced_block) => {
                // For empty blocks, we must have parent_id
                let parent =
                    parent_id.as_ref().ok_or_else(|| error!("Empty block must have parent"))?;
                RawCandidateId::create_empty(slot, referenced_block, parent)
            }
        };

        // Verify signature
        let id_to_sign = crate::utils::create_candidate_id_to_sign(id.slot, &id.hash);
        if !crate::utils::check_session_signature(
            session_id,
            &id_to_sign,
            candidate_tl.signature(),
            leader_key,
        ) {
            fail!("Candidate broadcast signature is not valid")
        }

        Ok(Self {
            id,
            parent_id,
            leader: leader_idx,
            block: block_data,
            signature: candidate_tl.signature().to_vec(),
        })
    }

    /// Extract parent from TL candidate
    fn extract_parent(candidate_tl: &CandidateData) -> Result<Option<RawCandidateId>> {
        match candidate_tl {
            CandidateData::Consensus_Block(block) => match &block.parent.id() {
                None => Ok(None),
                Some(id) => {
                    if *id.slot() < 0 {
                        fail!("Negative parent slot {} in Block candidate", id.slot());
                    }
                    let id_slot = SlotIndex(*id.slot() as u32);
                    let id_hash = UInt256::from_slice(id.hash().as_slice());
                    Ok(Some(RawCandidateId { slot: id_slot, hash: id_hash }))
                }
            },
            CandidateData::Consensus_Empty(empty) => {
                if *empty.parent.slot() < 0 {
                    fail!("Negative parent slot {} in Empty candidate", empty.parent.slot());
                }
                let id_slot = SlotIndex(*empty.parent.slot() as u32);
                let id_hash = UInt256::from_slice(empty.parent.hash().as_slice());
                Ok(Some(RawCandidateId { slot: id_slot, hash: id_hash }))
            }
        }
    }

    /// Extract block data from TL candidate
    ///
    /// Returns `CandidateBlockData` enum:
    /// - For `consensus.block`: extracts BlockCandidate and returns NonEmpty variant
    /// - For `consensus.empty`: extracts BlockIdExt and returns Empty variant
    ///
    /// Matches C++ reference: `RawCandidate::deserialize()` in `consensus-types.cpp`
    fn extract_block_data(
        candidate_tl: &CandidateData,
        leader_key: &PublicKey,
        shard: &ShardIdent,
        max_size: usize,
    ) -> Result<CandidateBlockData> {
        match candidate_tl {
            CandidateData::Consensus_Block(block) => {
                // Non-empty block: extract BlockCandidate from candidate bytes
                let candidate_bytes = &block.candidate[..];

                if candidate_bytes.is_empty() {
                    fail!("consensus.block has empty candidate bytes")
                }

                let block_info = crate::utils::extract_block_info_from_candidate(
                    candidate_bytes,
                    shard,
                    max_size,
                )?;

                match block_info {
                    Some(info) => Ok(CandidateBlockData::NonEmpty(BlockCandidate {
                        id: info.block_id,
                        collated_file_hash: info.collated_file_hash,
                        data: info.data,
                        collated_data: info.collated_data,
                        creator: leader_key.clone(),
                    })),
                    None => Err(error!("Failed to extract block info from candidate bytes")),
                }
            }
            CandidateData::Consensus_Empty(empty) => {
                // Empty block: extract BlockIdExt from block field
                Ok(CandidateBlockData::Empty(empty.block.clone()))
            }
        }
    }

    /// Serialize to TL bytes
    ///
    /// # Arguments
    ///
    /// * `compress` - If true, compress block data using LZ4
    ///
    /// # Serialization Format
    ///
    /// - Non-empty blocks: `consensus.block slot:int candidate:bytes parent:CandidateParent signature:bytes`
    /// - Empty blocks: `consensus.empty slot:int block:tonNode.blockIdExt parent:CandidateId signature:bytes`
    pub fn serialize(&self, compress: bool) -> Result<Vec<u8>> {
        match &self.block {
            CandidateBlockData::Empty(referenced_block) => {
                self.serialize_empty_block(referenced_block)
            }
            CandidateBlockData::NonEmpty(block) => self.serialize_non_empty_block(block, compress),
        }
    }

    /// Serialize non-empty block to TL bytes
    ///
    /// Non-empty blocks use `consensus.block` variant
    ///
    /// # Panics
    ///
    /// Panics if self.block is not NonEmpty variant (invariant violation)
    fn serialize_non_empty_block(&self, block: &BlockCandidate, compress: bool) -> Result<Vec<u8>> {
        // Invariant: this should only be called for non-empty blocks
        debug_assert!(!self.block.is_empty(), "serialize_non_empty_block called on empty block");

        // Create parent TL object
        let parent_tl = match &self.parent_id {
            Some(p) => {
                let parent_id = TlCandidateId { slot: p.slot.value() as i32, hash: p.hash.clone() };
                TlCandidateParent { id: parent_id.into_boxed() }.into_boxed()
            }
            None => TlCandidateParentBoxed::Consensus_CandidateWithoutParents,
        };

        // Serialize block candidate
        let candidate_bytes: Vec<u8> = if compress {
            // Compress block data using validator-session's compression utility
            let (compressed, decompressed_size) =
                consensus_common::compression::compress_candidate_data(
                    &block.data,
                    &block.collated_data,
                )?;

            let compressed_tl = CompressedCandidate {
                src: UInt256::default(),
                round: block.id.seq_no as i32,
                root_hash: block.id.root_hash.clone(),
                data: compressed,
                decompressed_size: decompressed_size as i32,
            };
            consensus_common::serialize_tl_boxed_object!(&compressed_tl.into_boxed())
        } else {
            // Uncompressed format
            let candidate_tl = TlCandidate {
                src: UInt256::default(),
                round: block.id.seq_no as i32,
                root_hash: block.id.root_hash.clone(),
                data: block.data.clone(),
                collated_data: block.collated_data.clone(),
            };
            consensus_common::serialize_tl_boxed_object!(&candidate_tl.into_boxed())
        };

        // Create consensus.block TL object (variant of CandidateData)
        let consensus_candidate = CandidateDataBlock {
            slot: self.id.slot.value() as i32,
            candidate: candidate_bytes,
            parent: parent_tl,
            signature: self.signature.clone(),
        };

        Ok(consensus_common::serialize_tl_boxed_object!(&consensus_candidate.into_boxed()))
    }

    /// Serialize empty block to TL bytes
    ///
    /// Empty blocks use `consensus.empty` variant
    ///
    /// # Panics
    ///
    /// Panics if self.block is not Empty variant (invariant violation)
    fn serialize_empty_block(&self, referenced_block: &BlockIdExt) -> Result<Vec<u8>> {
        // Invariant: this should only be called for empty blocks
        debug_assert!(self.block.is_empty(), "serialize_empty_block called on non-empty block");

        // Get parent (required for empty blocks)
        let parent = self
            .parent_id
            .as_ref()
            .ok_or_else(|| error!("Empty block must have parent for serialization"))?;

        // Create parent CandidateId TL object (boxed enum)
        let parent_tl =
            TlCandidateId { slot: parent.slot.value() as i32, hash: parent.hash.clone() }
                .into_boxed();

        // Create consensus.empty TL object
        // The block field is BlockIdExt (type alias in TL)
        let consensus_empty = CandidateDataEmpty {
            slot: self.id.slot.value() as i32,
            block: referenced_block.clone(),
            parent: parent_tl,
            signature: self.signature.clone(),
        };

        Ok(consensus_common::serialize_tl_boxed_object!(&consensus_empty.into_boxed()))
    }

    /// Sign a non-empty candidate (for block production)
    ///
    /// For empty blocks, use `create_empty_and_sign()` instead.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `block` - Block candidate data (required for non-empty blocks)
    /// * `parent` - Parent candidate ID (optional for first block in epoch)
    /// * `leader_idx` - Leader's validator index
    /// * `session_id` - Session ID
    /// * `private_key` - Leader's private key for signing
    pub fn create_and_sign(
        slot: SlotIndex,
        block: BlockCandidate,
        parent: Option<RawCandidateId>,
        leader_idx: ValidatorIndex,
        session_id: &SessionId,
        private_key: &PrivateKey,
    ) -> Result<Self> {
        // Compute candidate ID
        let id = RawCandidateId::create(slot, Some(&block), parent.as_ref());

        // Sign the candidate
        let signature = crate::utils::sign_candidate(session_id, slot, &id.hash, private_key)?;

        Ok(Self::new(id, parent, leader_idx, block, signature))
    }

    /// Sign an empty candidate (for finalization recovery)
    ///
    /// Empty blocks are used when the chain is behind and we need to
    /// progress consensus without new block data.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `parent` - Parent candidate ID (REQUIRED for empty blocks)
    /// * `referenced_block` - The BlockIdExt being re-signed (parent's block)
    /// * `leader_idx` - Leader's validator index
    /// * `session_id` - Session ID
    /// * `private_key` - Leader's private key for signing
    pub fn create_empty_and_sign(
        slot: SlotIndex,
        parent: RawCandidateId,
        referenced_block: BlockIdExt,
        leader_idx: ValidatorIndex,
        session_id: &SessionId,
        private_key: &PrivateKey,
    ) -> Result<Self> {
        // Compute candidate ID using empty block hash
        let id = RawCandidateId::create_empty(slot, &referenced_block, &parent);

        // Sign the candidate
        let signature = crate::utils::sign_candidate(session_id, slot, &id.hash, private_key)?;

        Ok(Self::new_empty(id, parent, leader_idx, referenced_block, signature))
    }
}

/// Pointer type for RawCandidate
#[allow(dead_code)]
pub type RawCandidatePtr = Arc<RawCandidate>;

/// Pointer type for Candidate
#[allow(dead_code)]
pub type CandidatePtr = Arc<Candidate>;

/// Resolved candidate with full parent information
///
/// # Invariants
///
/// - If block.is_some(): block.id == id.block (non-empty block uses block's ID)
/// - If block.is_none(): parent_id must exist (empty block requires parent)
/// - If block.is_none(): id.block is inherited from parent (set during resolution)
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Resolved candidate ID with BlockIdExt
    pub id: CandidateId,

    /// Resolved parent ID
    pub parent_id: Option<CandidateId>,

    /// Leader validator index
    pub leader: ValidatorIndex,

    /// Block candidate data (None for empty blocks)
    #[allow(dead_code)]
    pub block: Option<BlockCandidate>,

    /// Ed25519 signature
    #[allow(dead_code)]
    pub signature: Vec<u8>,
}

#[allow(dead_code)]
impl Candidate {
    /// Create a new Candidate with invariant validation
    ///
    /// # Invariants Checked
    ///
    /// - Non-empty blocks: block.id must match id.block
    /// - Empty blocks: parent_id must be present
    ///
    /// # Panics
    ///
    /// Panics in debug builds if invariants are violated
    pub fn new(
        id: CandidateId,
        parent_id: Option<CandidateId>,
        leader: ValidatorIndex,
        block: Option<BlockCandidate>,
        signature: Vec<u8>,
    ) -> Self {
        // Invariant checks
        if let Some(ref b) = block {
            debug_assert!(
                b.id == id.block,
                "Candidate invariant: block.id ({:?}) must match id.block ({:?})",
                b.id,
                id.block
            );
        } else {
            debug_assert!(
                parent_id.is_some(),
                "Candidate invariant: empty block must have parent_id"
            );
        }

        Self { id, parent_id, leader, block, signature }
    }

    /// Check if this is an empty block
    pub fn is_empty(&self) -> bool {
        self.block.is_none()
    }

    /// Convert to RawCandidate
    pub fn to_raw(&self) -> RawCandidate {
        // Create CandidateBlockData from block and id.block
        let block_data = match &self.block {
            Some(block) => CandidateBlockData::NonEmpty(block.clone()),
            None => CandidateBlockData::Empty(self.id.block.clone()),
        };

        RawCandidate {
            id: self.id.to_raw(),
            parent_id: self.parent_id.as_ref().map(|p| p.to_raw()),
            leader: self.leader,
            block: block_data,
            signature: self.signature.clone(),
        }
    }
}

/// Lightweight parent info for FSM operations
///
/// Used in SlotState for tracking voted blocks. Contains only slot and hash,
/// not the full BlockIdExt.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CandidateParentInfo {
    /// Slot number
    pub slot: SlotIndex,

    /// Candidate ID hash
    pub hash: UInt256,
}

impl CandidateParentInfo {
    /// Create from slot and hash
    #[allow(dead_code)]
    pub fn new(slot: SlotIndex, hash: UInt256) -> Self {
        Self { slot, hash }
    }
}

impl fmt::Debug for CandidateParentInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CandidateParentInfo {{ slot: {}, hash: {} }}",
            self.slot,
            self.hash.to_hex_string()
        )
    }
}

impl fmt::Display for CandidateParentInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{slot={}, hash={}}}", self.slot, &self.hash.to_hex_string()[..8])
    }
}

/// FSM parent type: None represents genesis/no parent
pub type CandidateParent = Option<CandidateParentInfo>;
