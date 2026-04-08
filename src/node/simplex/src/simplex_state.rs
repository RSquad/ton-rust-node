/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

//! SimplexState - Core Consensus State Machine
//!
//! This module implements the core consensus state machine based on:
//! - Solana Alpenglow White Paper (May 2025), Algorithm 1 (page 21) and Algorithm 2 (page 22)
//! - C++ reference: `simplex/consensus.cpp`, `simplex/pool.cpp`
//!
//! ## White Paper Algorithm
//!
//! ### Algorithm 1: Event Handlers
//!
//! ```text
//! upon Block(s, hash, hashparent) do
//!     if tryNotar(Block(s, hash, hashparent)) then
//!         checkPendingBlocks()
//!     else if Voted ∉ state[s] then
//!         pendingBlocks[s] ← Block(s, hash, hashparent)
//!
//! upon Timeout(s) do
//!     if Voted ∉ state[s] then
//!         trySkipWindow(s)
//!
//! upon BlockNotarized(s, hash(b)) do
//!     state[s] ← state[s] ∪ {BlockNotarized(hash(b))}
//!     tryFinal(s, hash(b))
//!
//! upon ParentReady(window, hash(b)) do
//!     state[window.first_slot] ← state[window.first_slot] ∪ {ParentReady(hash(b))}
//!     checkPendingBlocks()
//!     setTimeouts(window)
//!
//! upon SafeToNotar(s, hash(b)) do
//!     trySkipWindow(s)
//!     if ItsOver ∉ state[s] then
//!         broadcast NotarFallbackVote(s, hash(b))
//!         state[s] ← state[s] ∪ {BadWindow}
//!
//! upon SafeToSkip(s) do
//!     trySkipWindow(s)
//!     if ItsOver ∉ state[s] then
//!         broadcast SkipFallbackVote(s)
//!         state[s] ← state[s] ∪ {BadWindow}
//! ```
//!
//! ### Algorithm 2: Helper Functions
//!
//! ```text
//! function tryNotar(Block(s, hash, hashparent))
//!     if Voted ∈ state[s] then return false
//!     firstSlot ← (s is the first slot in leader window)
//!     if firstSlot then
//!         canVote ← ParentReady(hashparent) ∈ state[s]
//!     else
//!         canVote ← VotedNotar(hashparent) ∈ state[s-1]
//!     if canVote then
//!         broadcast NotarVote(s, hash)
//!         state[s] ← state[s] ∪ {Voted, VotedNotar(hash)}
//!         pendingBlocks[s] ← ⊥
//!         tryFinal(s, hash)
//!         return true
//!     return false
//!
//! function tryFinal(s, hash(b))
//!     if BlockNotarized(hash(b)) ∈ state[s] and VotedNotar(hash(b)) ∈ state[s]
//!        and BadWindow ∉ state[s] then
//!         broadcast FinalVote(s)
//!         state[s] ← state[s] ∪ {ItsOver}
//!
//! function trySkipWindow(s)
//!     for k ∈ windowSlots(s) do
//!         if Voted ∉ state[k] then
//!             broadcast SkipVote(k)
//!             state[k] ← state[k] ∪ {Voted, BadWindow}
//!             pendingBlocks[k] ← ⊥
//!
//! function checkPendingBlocks()
//!     for s : pendingBlocks[s] ≠ ⊥ do
//!         tryNotar(pendingBlocks[s])
//! ```
//!
//! ## C++ Implementation vs Alpenglow White Paper
//!
//! The C++ reference uses a **simplified protocol** without fallback votes.
//! Rust supports both modes via `enable_fallback_protocol` constructor parameter.
//!
//! ### Vote Types
//!
//! | Vote Type | White Paper | C++ (wire) | Rust (internal) |
//! |-----------|-------------|------------|-----------------|
//! | Notarize  | ✅          | ✅         | ✅              |
//! | Finalize  | ✅          | ✅         | ✅              |
//! | Skip      | ✅          | ✅         | ✅              |
//! | NotarizeFallback | ✅   | ❌         | ✅ (filtered)   |
//! | SkipFallback     | ✅   | ❌         | ✅ (filtered)   |
//!
//! ### `enable_fallback_protocol` Option
//!
//! - **`false` (default, C++ compatible)**: 3 vote types only (Notarize, Finalize, Skip).
//!   - Fallback votes are filtered in `broadcast_vote()`
//!   - `SafeToNotar` / `SafeToSkip` events are NOT processed
//!   - `Notarize + Skip` from same validator is **allowed** (C++ pool.cpp behavior)
//!   - Timeout check: `its_over` (voted_final in C++) blocks timeout
//!
//! - **`true` (full Alpenglow)**: All 5 vote types.
//!   - Full White Paper algorithm with fallback mechanism
//!   - `Notarize + Skip` from same validator is **misbehavior**
//!   - Timeout check: `is_voted` (any vote) blocks timeout
//!
//! ### C++ Differences from White Paper
//!
//! 1. **No fallback votes**: C++ TL schema has no `NotarizeFallback` / `SkipFallback`
//!
//! 2. **Leader Windows**: C++ uses explicit `LeaderWindow` struct with `available_bases`,
//!    `slots[]`, and `had_timeouts` flag. Windows are created lazily.
//!
//! 3. **ParentReady is per-window**: In C++, `ParentReady` event is received per-window,
//!    not per-slot. The `available_bases` set in `LeaderWindow` tracks valid parents.
//!
//! 4. **Timeout behavior**:
//!    - C++ `alarm()` checks `voted_final` (not `is_voted`), allowing Skip after Notarize
//!    - C++ uses `multimap<Timestamp, slot>` for timeouts
//!    - Fresh timeout scheduled on `LeaderWindowObserved` (= `on_window_base_ready`)
//!
//! 5. **trySkipWindow**: C++ iterates all window slots and checks `voted_final`,
//!    not `is_voted`, allowing Skip after Notarize
//!
//! 6. **Vote thresholds**:
//!    - `BlockNotarized`: notar(b) >= 2/3 (certificate threshold)
//!    - `SafeToNotar`: notar(b) >= 1/3 AND notar(b) + skip >= 2/3
//!    - `SafeToSkip`: skip + sum(notar) - max(notar) >= 1/3
//!
//! 7. **Block identification**: Uses `BlockIdExt` (full block ID) in votes,
//!    not just hash. Parent references use `CandidateParentInfo` (slot + hash).
//!
//! 8. **Empty blocks**: C++ supports empty blocks for finalization recovery
//!    (not in White Paper). Empty block has `block = None`, must have parent.
//!
//! ## Design Principles
//!
//! 1. **No external dependencies** - TL, network, etc. are not used directly
//! 2. **Event-based output** - All actions produce `SimplexEvent` (no callbacks)
//! 3. **Self-contained timing** - FSM manages its own timeouts via `check_all()` + `get_next_timeout()`
//! 4. **Independent testing** - FSM can be unit tested without network or TL dependencies
//! 5. **C++ compatible** - Block types match C++ implementation (BlockIdExt in votes)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ SimplexState FSM                                                    │
//! │                                                                     │
//! │  ┌─────────────────────────────┐  ┌─────────────────────────────┐   │
//! │  │ Consensus State             │  │ Vote Accounting             │   │
//! │  │ (from SimplexConsensusImpl) │  │ (from SimplexPoolImpl)      │   │
//! │  │                             │  │                             │   │
//! │  │ - leader_windows            │  │ - slot_votes                │   │
//! │  │ - pending_slots             │  │ - notarize weights          │   │
//! │  │ - first_non_finalized_slot  │  │ - skip weights              │   │
//! │  │ - timeout state             │  │ - certificate tracking      │   │
//! │  └─────────────────────────────┘  └─────────────────────────────┘   │
//! │                                                                     │
//! │  ┌─────────────────────────────────────────────────────────────┐    │
//! │  │ Event Queue (VecDeque<SimplexEvent>)                        │    │
//! │  │                                                             │    │
//! │  │  - BroadcastVote(vote)                                      │    │
//! │  │  - BlockFinalized(slot, block)                              │    │
//! │  │  - SlotSkipped(slot)                                        │    │
//! │  └─────────────────────────────────────────────────────────────┘    │
//! │                                                                     │
//! │  Input API:                      Output (pull events):              │
//! │  - on_candidate(desc, ...)       - pull_event() -> SimplexEvent     │
//! │  - on_vote(desc, ...)            - pending_event_count()            │
//! │  - check_all(desc)               - has_pending_events()             │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Event Model
//!
//! Instead of callbacks, SimplexState produces events that are queued internally:
//! 1. Call FSM methods (`on_candidate`, `on_vote`, `check_all`)
//! 2. Pull events with `pull_event()` until it returns `None`
//! 3. Process each event (broadcast vote, notify listener, etc.)
//!
//! This enables:
//! - **Testing**: Inspect produced events without mocking
//! - **Debugging**: Dump event queue for diagnostics
//! - **Tracing**: All FSM outputs go through a single mechanism
//!
//! ## Timeout Model
//!
//! The FSM controls its own timing. Clients should:
//! 1. Call `check_all(desc)` whenever `get_next_timeout()` has elapsed (or earlier is ok)
//! 2. `check_all()` processes pending timeouts and updates `next_timeout`
//! 3. After any event (`on_candidate`, `on_vote`), `next_timeout` may change
//!
//! ## Usage
//!
//! ```ignore
//! use simplex::{SimplexState, SimplexEvent};
//!
//! // Create FSM
//! let mut state = SimplexState::new(&session_description, false).expect("Invalid session config");
//!
//! // Main loop
//! loop {
//!     let timeout = state.get_next_timeout();
//!     // wait for timeout or incoming event...
//!     
//!     // Process incoming events (handle errors as needed)
//!     if let Err(e) = state.on_candidate(&session_description, candidate) {
//!         log::warn!("Misbehavior: {}", e);
//!     }
//!     if let Err(e) = state.on_vote(&session_description, validator_idx, vote) {
//!         log::warn!("Misbehavior: {}", e);
//!     }
//!     
//!     // Check for timeouts and pending actions
//!     state.check_all(&session_description);
//!     
//!     // Process all produced events
//!     while let Some(event) = state.pull_event() {
//!         match event {
//!             SimplexEvent::BroadcastVote(vote) => {
//!                 receiver.send_vote(vote);
//!             }
//!             SimplexEvent::BlockFinalized(e) => {
//!                 listener.on_block_committed(e.slot, e.block, ...);
//!             }
//!             SimplexEvent::SlotSkipped(e) => {
//!                 listener.on_block_skipped(e.slot);
//!             }
//!         }
//!     }
//! }
//! ```

use crate::{
    block::{
        Candidate, CandidateId, CandidateParent, CandidateParentInfo, SlotIndex, ValidatorIndex,
        WindowIndex,
    },
    certificate::{
        Certificate, FinalCert, FinalCertPtr, NotarCert, NotarCertPtr, SkipCert, SkipCertPtr,
        VoteSignature,
    },
    misbehavior::{
        ConflictReason, ConflictingVoteType, MisbehaviorProof, VoteDescriptor, VoteResult,
    },
    session_description::SessionDescription,
    RawVoteData, ValidatorWeight,
};

/// Maximum number of slots ahead of `first_non_finalized_slot` that the FSM
/// will accept. Any vote, candidate, or certificate referencing a slot beyond
/// this horizon is rejected to prevent a Byzantine validator from triggering
/// unbounded window/slot allocation (DoS).
///
/// Note: C++ has no equivalent cap. This is a Rust-only defense-in-depth measure.
/// 10,000 is generous enough to never affect liveness under normal conditions.
pub const MAX_FUTURE_SLOTS: u32 = 10_000;

use std::{
    cmp,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
    fmt::{Display, Formatter},
    mem,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_block::{error, fail, BlockIdExt, Result, UInt256};

/*
    ============================================================================
    SimplexState Options
    ============================================================================
*/

/// Configuration options for SimplexState
///
/// Controls behavior of the consensus FSM, particularly around fallback
/// protocol and C++ compatibility.
#[derive(Clone, Debug)]
pub struct SimplexStateOptions {
    /// Enable fallback protocol (SafeToNotar/SafeToSkip and fallback votes)
    ///
    /// When `false` (default, C++ compatible):
    /// - SafeToNotar/SafeToSkip events are not processed
    /// - Fallback votes (NotarizeFallback, SkipFallback) are not broadcast
    ///
    /// When `true` (full Alpenglow):
    /// - Full White Paper algorithm with fallback mechanism
    pub enable_fallback_protocol: bool,

    /// Allow skip vote after notarize for same slot (and vice versa)
    ///
    /// When `true` (default, C++ compatible):
    /// - Notarize + Skip from same validator is ALLOWED
    /// - Matches C++ pool.cpp behavior
    ///
    /// When `false` (Alpenglow strict mode):
    /// - Notarize + Skip from same validator is MISBEHAVIOR
    pub allow_skip_after_notarize: bool,

    /// Require parent to be finalized (not just notarized) for block generation
    ///
    /// When `false` (default, C++ compatible):
    /// - Parent can be notarized OR finalized to build child block
    /// - Matches C++ pool.cpp behavior (parent_slot->state->notarized.has_value())
    /// - Allows progress even when finalization is delayed
    /// - Prevents deadlock when some validators vote skip while others vote finalize
    ///
    /// When `true` (strict mode):
    /// - Parent must be finalized before child block can be generated
    /// - Use for testing sequential finalization scenarios
    /// - WARNING: Can cause deadlock if finalization is blocked
    ///
    /// C++ uses notarized parent check (not finalized) for collation availability.
    pub require_finalized_parent: bool,

    /// Use notarized-parent chain semantics for parenting/progress (C++ pool `now_` model).
    ///
    /// When `false` (legacy ParentReady-driven window progression):
    /// - Leader window advancement / timeout scheduling is driven via `on_window_base_ready()` (finalization)
    /// - First-slot parent readiness is tracked per-window (`LeaderWindow.available_bases`)
    /// - `first_non_progressed_slot` / `Slot.available_base` / `Slot.skipped` are still tracked for consistency,
    ///   but do not drive leader-window progression
    ///
    /// When `true` (C++ pool.cpp parity, default for `cpp_compatible()`):
    /// - Progress cursor `first_non_progressed_slot` advances on **(notarized OR skipped)**,
    ///   like C++ `PoolImpl::advance_present()` / `maybe_publish_new_leader_windows()`
    /// - Per-slot `available_base` (optional-of-optional) is the canonical parent chain:
    ///   - `None` = base unknown
    ///   - `Some(None)` = genesis base
    ///   - `Some(Some(id))` = concrete parent candidate id
    /// - Leader window advancement / timeout scheduling follows the progress cursor
    ///
    /// Both modes maintain the tracking state (`available_base`, `skipped`, `first_non_progressed_slot`)
    /// to keep `SimplexState` internally consistent.
    pub use_notarized_parent_chain: bool,
}

impl Default for SimplexStateOptions {
    fn default() -> Self {
        Self {
            // C++ compatible mode by default
            enable_fallback_protocol: false,
            allow_skip_after_notarize: true,
            // C++ allows notarized blocks as parents (not just finalized)
            require_finalized_parent: false,
            // C++ pool.cpp parity: notarized-parent chain drives window progression.
            use_notarized_parent_chain: true,
        }
    }
}

impl SimplexStateOptions {
    /// Create options for C++ compatible mode (default)
    pub fn cpp_compatible() -> Self {
        Self::default()
    }

    /// Create options for full Alpenglow mode
    #[allow(dead_code)]
    pub fn alpenglow() -> Self {
        Self {
            enable_fallback_protocol: true,
            allow_skip_after_notarize: false,
            require_finalized_parent: false,
            use_notarized_parent_chain: false,
        }
    }

    /// Create options for strict sequential mode (for testing deadlock scenarios)
    ///
    /// WARNING: This mode requires parent to be finalized, which can cause deadlock
    /// if some validators vote skip while others vote finalize. Use only for testing.
    #[allow(dead_code)]
    pub fn strict_sequential() -> Self {
        Self {
            enable_fallback_protocol: false,
            allow_skip_after_notarize: true,
            require_finalized_parent: true,
            use_notarized_parent_chain: false,
        }
    }
}

/*
    ============================================================================
    Constants
    ============================================================================
*/

/// Maximum number of notar-fallback votes allowed per validator per slot
/// Reference: C++ pool.cpp TooManyFallbackVotesMisbehaviorProof (>3 = misbehavior)
const MAX_NOTAR_FALLBACK_VOTES_PER_VALIDATOR: usize = 3;

/*
    ============================================================================
    Vote Types for FSM
    ============================================================================

    Reference: C++ simplex/consensus-bus.h

    These are internal FSM vote types, not TL wire types.
    Conversion to/from TL happens in SessionProcessor.

    Block identification uses BlockIdExt (matching C++ implementation).
*/

/// Notarization vote - vote to notarize a block in a slot
///
/// Reference: C++ NotarizeVote in consensus-bus.h
/// TL: `consensus.simplex.notarizeVote id:consensus.CandidateId`
///
/// Algorithm 2: broadcast NotarVote(s, hash)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotarizeVote {
    /// Slot number (from CandidateId.slot)
    pub slot: SlotIndex,
    /// Candidate hash (from CandidateId.hash, computed by compute_candidate_id_hash)
    pub block_hash: UInt256,
}

/// Finalization vote - vote to finalize a notarized block
///
/// Reference: C++ FinalizeVote in consensus-bus.h
/// TL: `consensus.simplex.finalizeVote id:consensus.CandidateId`
///
/// Algorithm 2: broadcast FinalVote(s)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizeVote {
    /// Slot number (from CandidateId.slot)
    pub slot: SlotIndex,
    /// Candidate hash (from CandidateId.hash, computed by compute_candidate_id_hash)
    pub block_hash: UInt256,
}

/// Skip vote - vote to skip a slot
///
/// Reference: C++ SkipVote in consensus-bus.h
/// TL: `consensus.simplex.skipVote slot:int`
///
/// Algorithm 2: broadcast SkipVote(k) for k in windowSlots(s)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipVote {
    /// Slot to skip
    pub slot: SlotIndex,
}

/// Notarization fallback vote - fallback notarization for a block
///
/// Algorithm 1: upon SafeToNotar(s, hash(b)) do broadcast NotarFallbackVote(s, hash(b))
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotarizeFallbackVote {
    /// Slot number
    pub slot: SlotIndex,
    /// Candidate hash being notarized via fallback
    pub block_hash: UInt256,
}

/// Skip fallback vote - fallback skip for a slot
///
/// Algorithm 1: upon SafeToSkip(s) do broadcast SkipFallbackVote(s)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipFallbackVote {
    /// Slot to skip
    pub slot: SlotIndex,
}

/// Vote enum - all vote types for FSM processing
///
/// Reference: C++ Vote variant in consensus-bus.h
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vote {
    /// Notarization vote
    Notarize(NotarizeVote),
    /// Finalization vote
    Finalize(FinalizeVote),
    /// Skip vote
    Skip(SkipVote),
    /// Notarization fallback vote
    NotarizeFallback(NotarizeFallbackVote),
    /// Skip fallback vote
    SkipFallback(SkipFallbackVote),
}

/*
    ============================================================================
    SimplexEvent - Output Events from FSM
    ============================================================================

    Events produced by SimplexState that need to be processed by the caller
    (SessionProcessor). This event-based approach replaces the callback-based
    SimplexEnvironment trait.

    Benefits:
    - Better testability: events can be inspected without mocking
    - Better debugging: event queue can be dumped for diagnostics
    - Cleaner separation: FSM produces events, caller handles them
    - Easier to trace: all outputs go through a single mechanism
*/

/// Block finalized event data
///
/// Reference: C++ ConsensusBus::SlotFinalized, FinalizationObserved
///
/// Emitted when FSM determines a block has FinalizationCertificate.
/// Caller should notify SessionListener::on_block_committed.
///
/// Contains:
/// - `slot`: The slot number
/// - `block_hash`: The candidate hash (computed by compute_candidate_id_hash, for vote weight lookup)
/// - `block_id`: Optional BlockIdExt with seqno (None if candidate wasn't tracked)
/// - `certificate`: Finalization certificate with signatures (P2.3)
///
/// The certificate contains aggregated signatures from validators who voted finalize.
/// Used by SessionProcessor to pass real signatures to on_block_committed callback.
#[derive(Clone, Debug)]
pub struct BlockFinalizedEvent {
    /// Slot number
    pub slot: SlotIndex,
    /// Candidate hash (computed by compute_candidate_id_hash, NOT the block's root_hash)
    pub block_hash: UInt256,
    /// Optional full block ID with seqno (None if on_candidate wasn't called for this block)
    pub block_id: Option<BlockIdExt>,
    /// Finalization certificate with signatures (P2.3)
    ///
    /// Reference: C++ FinalizationObserved event includes FinalCertRef
    ///
    /// Contains aggregated signatures from validators who voted finalize.
    /// Used by SessionProcessor to create signature set for on_block_committed.
    pub certificate: FinalCertPtr,
}

impl PartialEq for BlockFinalizedEvent {
    fn eq(&self, other: &Self) -> bool {
        // Compare slot and block_hash (certificate is derived from these)
        self.slot == other.slot && self.block_hash == other.block_hash
    }
}

impl Eq for BlockFinalizedEvent {}

/// Slot skipped event
///
/// Emitted when FSM determines finalization is no longer possible for a slot.
/// This happens when:
/// - Skip certificate reached (>=2/3 skip votes)
/// - We vote Skip for a slot in a bad window
///
/// Caller should notify SessionListener::on_block_skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSkippedEvent {
    /// Skipped slot number
    pub slot: SlotIndex,
}

/// Event: Notarization threshold reached for a block
///
/// Reference: C++ ConsensusBus::NotarizationObserved
///
/// This event is emitted when a block receives enough notarize votes
/// (2/3 + 1 of validator weight). The candidate resolver uses this
/// to cache notarization certificates for responding to queries.
#[derive(Clone, Debug)]
pub struct NotarizationReachedEvent {
    /// Slot number
    pub slot: SlotIndex,
    /// Candidate hash (computed by compute_candidate_id_hash)
    pub block_hash: UInt256,
    /// Notarization certificate with signatures
    pub certificate: NotarCertPtr,
}

/// Event: Skip certificate threshold reached for a slot
///
/// Reference: C++ pool.cpp creates skip certificate when threshold reached.
///
/// This event is emitted when a slot receives enough skip votes
/// (2/3 + 1 of validator weight). Used for broadcasting the skip
/// certificate to other validators (C++ mode only - Alpenglow paper
/// doesn't have explicit skip certificates).
#[derive(Clone, Debug)]
pub struct SkipCertificateReachedEvent {
    /// Slot number
    pub slot: SlotIndex,
    /// Skip certificate with signatures
    pub certificate: SkipCertPtr,
}

/// Event: Finalization threshold reached for a block
///
/// Reference: C++ ConsensusBus::FinalizationObserved
///
/// This event is emitted when a block receives enough finalize votes
/// (2/3 + 1 of validator weight). Used for caching the finalization
/// certificate for standstill replay.
///
/// Note: This is separate from `BlockFinalized` which triggers commit logic.
/// `FinalizationReached` is emitted when threshold is reached (certificate created),
/// while `BlockFinalized` is emitted when we're ready to commit.
#[derive(Clone, Debug)]
pub struct FinalizationReachedEvent {
    /// Slot number
    pub slot: SlotIndex,
    /// Candidate hash (computed by compute_candidate_id_hash)
    pub block_hash: UInt256,
    /// Finalization certificate with signatures
    pub certificate: FinalCertPtr,
}

/// Events produced by SimplexState
///
/// These events are queued internally and can be pulled by the caller.
/// The caller (SessionProcessor) processes these events by:
/// - `BroadcastVote` → Sign and send via Receiver
/// - `BlockFinalized` → Notify SessionListener::on_block_committed
/// - `SlotSkipped` → Notify SessionListener::on_block_skipped
/// - `NotarizationReached` → Cache serialized notarization certificate in receiver
/// - `SkipCertificateReached` → Broadcast skip certificate to validators (C++ mode only)
/// - `FinalizationReached` → Cache finalization certificate and relay to peers
#[derive(Clone, Debug)]
pub enum SimplexEvent {
    /// Broadcast a vote to all validators
    ///
    /// Reference: Algorithm 2, various "broadcast XVote(...)" statements
    BroadcastVote(Vote),

    /// A block has been finalized
    ///
    /// Reference: C++ ConsensusBus::SlotFinalized
    BlockFinalized(BlockFinalizedEvent),

    /// A slot has been skipped (finalization no longer possible)
    ///
    /// Emitted when skip certificate reached or when we vote Skip.
    SlotSkipped(SlotSkippedEvent),

    /// A block has been notarized (threshold reached, certificate created)
    ///
    /// Reference: C++ ConsensusBus::NotarizationObserved
    /// Used by candidate resolver to cache notarization certificates.
    NotarizationReached(NotarizationReachedEvent),

    /// A skip certificate threshold was reached
    ///
    /// Reference: C++ pool.cpp skip certificate creation
    /// Used to broadcast skip certificate to validators.
    /// Only emitted in C++ compatibility mode.
    SkipCertificateReached(SkipCertificateReachedEvent),

    /// A finalization threshold was reached (certificate created)
    ///
    /// Reference: C++ ConsensusBus::FinalizationObserved / handle_saved_certificate
    /// Caches finalization certificate for standstill replay and relays to peers.
    FinalizationReached(FinalizationReachedEvent),
}

/*
    ============================================================================
    Slot State
    ============================================================================

    Reference: C++ Slot struct in consensus.cpp
*/

/// Per-slot consensus state
///
/// Reference: C++ `struct Slot` in consensus.cpp
#[derive(Clone, Debug, Default)]
struct Slot {
    /// Per-slot available base (C++ pool.cpp `SlotState::available_base`)
    ///
    /// This is an optional-of-optional to match C++:
    /// - `None` = base unknown (not yet determined)
    /// - `Some(None)` = genesis base (RawParentId{})
    /// - `Some(Some(id))` = concrete parent candidate id
    ///
    /// This field is always maintained for state consistency. When
    /// `SimplexStateOptions::use_notarized_parent_chain` is enabled, it is used
    /// to propagate bases across (notarized OR skipped) slots like C++ pool.cpp.
    available_base: Option<CandidateParent>,

    /// Pending block candidate waiting for parent/conditions
    /// Alpenglow: pendingBlocks[s]
    pending_block: Option<Candidate>,

    /// Has this node voted in this slot?
    /// Alpenglow: Voted ∈ state[s]
    is_voted: bool,

    /// Block we voted to notarize (if any)
    /// Alpenglow: VotedNotar(hash) ∈ state[s]
    voted_notar: Option<CandidateParentInfo>,

    /// Have we voted to skip this slot?
    ///
    /// Reference: C++ `struct SlotState` in `consensus.cpp` (`voted_skip`).
    ///
    /// This is a **local** flag (our node only) used to:
    /// - prevent local auto-finalize after we already voted skip
    /// - restore local skip state on restart (bootstrap)
    voted_skip: bool,

    /// Have we voted to finalize this slot?
    ///
    /// Reference: C++ `struct SlotState` in `consensus.cpp` (`voted_final`).
    ///
    /// This is a **local** flag (our node only). In C++, `alarm()` checks
    /// `!voted_final` before voting skip — once a node votes final, it cannot
    /// vote skip for that slot. This prevents split-brain deadlocks where some
    /// nodes vote skip and others vote final, neither reaching the 67% threshold.
    voted_final: bool,

    /// Observed notarization certificate for a block
    /// Alpenglow: BlockNotarized(hash(b)) ∈ state[s]
    observed_notar_certificate: Option<CandidateParentInfo>,

    /// Has this slot reached skip certificate threshold (2/3)?
    ///
    /// C++ pool.cpp: `SlotState::skipped`
    ///
    skipped: bool,

    /// Is consensus finished for this slot?
    /// Alpenglow: ItsOver ∈ state[s]
    its_over: bool,

    /// Have we entered fallback mode for this slot's window?
    /// Alpenglow: BadWindow ∈ state[s]
    is_bad_window: bool,
}

impl Slot {
    /// Merge a new parent into `available_base`, keeping the maximum.
    ///
    /// Reference: C++ pool.cpp `SlotState::add_available_base(RawParentId parent)`:
    /// ```cpp
    /// if (!available_base.has_value() || parent >= *available_base) {
    ///     available_base = parent;
    /// }
    /// ```
    ///
    /// Ordering mirrors C++ `RawCandidateId::operator<=>` (default): slot first, then hash.
    /// `None` (genesis) < `Some(_)` (real parent), matching `std::optional` ordering.
    fn add_available_base_max(&mut self, parent: CandidateParent) {
        match &self.available_base {
            None => {
                self.available_base = Some(parent);
            }
            Some(existing) => {
                if candidate_parent_ge(&parent, existing) {
                    self.available_base = Some(parent);
                }
            }
        }
    }
}

/// Compare two `CandidateParent` values: `a >= b`.
///
/// Mirrors C++ `RawParentId` (= `optional<RawCandidateId>`) ordering:
/// - `None` (genesis) < `Some(_)` (real parent)
/// - `Some(a)` vs `Some(b)`: compare slot first, then hash
fn candidate_parent_ge(a: &CandidateParent, b: &CandidateParent) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (Some(a_info), Some(b_info)) => (a_info.slot, &a_info.hash) >= (b_info.slot, &b_info.hash),
    }
}

/*
    ============================================================================
    Leader Window
    ============================================================================

    Reference: C++ LeaderWindow struct in consensus.cpp
*/

/// Leader window containing slots
///
/// Reference: C++ `struct LeaderWindow` in consensus.cpp
#[derive(Debug)]
struct LeaderWindow {
    /// Window index
    window_idx: WindowIndex,

    /// First slot in this window
    start_slot: SlotIndex,

    /// Set of available parent candidates
    /// ParentReady events add to this set
    available_bases: HashSet<CandidateParent>,

    /// Slots in this window
    slots: Vec<Slot>,

    /// Did any timeout fire in this window? (for adaptive backoff)
    had_timeouts: bool,
}

impl LeaderWindow {
    fn new(window_idx: WindowIndex, start_slot: SlotIndex, slots_per_window: u32) -> Self {
        let mut slots = Vec::with_capacity(slots_per_window as usize);
        slots.resize_with(slots_per_window as usize, Slot::default);

        Self { window_idx, start_slot, available_bases: HashSet::new(), slots, had_timeouts: false }
    }
}

/*
    ============================================================================
    Vote Accounting per Validator
    ============================================================================

    Reference: C++ Votes struct in pool.cpp
*/

/// Votes from a single validator for a slot
///
/// Reference: C++ `struct Votes` in pool.cpp
///
/// Stores vote content, signature for certificate creation, and raw bytes
/// for misbehavior proof generation. Raw bytes allow verifiable proofs
/// of conflicting votes.
#[derive(Clone, Debug, Default)]
struct ValidatorVotes {
    /// Notarize vote (at most one per validator)
    notarize: Option<NotarizeVote>,

    /// Notarize vote signature (stored for certificate creation)
    notarize_signature: Option<Vec<u8>>,

    /// Notarize vote raw bytes (stored for misbehavior proofs)
    /// Uses Arc<RawBuffer> for memory-efficient sharing
    notarize_raw: Option<RawVoteData>,

    /// Skip vote (at most one per validator)
    skip: Option<SkipVote>,

    /// Skip vote signature (stored for certificate creation)
    skip_signature: Option<Vec<u8>>,

    /// Skip vote raw bytes (stored for misbehavior proofs)
    /// Uses Arc<RawBuffer> for memory-efficient sharing
    skip_raw: Option<RawVoteData>,

    /// Finalize vote (at most one per validator)
    finalize: Option<FinalizeVote>,

    /// Finalize vote signature (stored for certificate creation)
    finalize_signature: Option<Vec<u8>>,

    /// Finalize vote raw bytes (stored for misbehavior proofs)
    /// Uses Arc<RawBuffer> for memory-efficient sharing
    finalize_raw: Option<RawVoteData>,

    /// Skip fallback vote (at most one per validator)
    fallback_skip: Option<SkipFallbackVote>,

    /// Skip fallback vote signature (stored for certificate creation, if fallback is added to TL)
    #[allow(dead_code)]
    fallback_skip_signature: Option<Vec<u8>>,

    /// Skip fallback vote raw bytes (stored for misbehavior proofs)
    /// Uses Arc<RawBuffer> for memory-efficient sharing
    fallback_skip_raw: Option<RawVoteData>,

    /// Notar fallback votes (up to MAX_NOTAR_FALLBACK_VOTES_PER_VALIDATOR)
    /// Key is candidate hash, value is raw bytes for misbehavior proofs
    /// Uses Arc<RawBuffer> for memory-efficient sharing
    fallback_notarize: HashMap<UInt256, RawVoteData>,

    /// Notar fallback vote signatures (stored for certificate creation, if fallback is added to TL)
    /// Key is candidate hash
    #[allow(dead_code)]
    fallback_notarize_signatures: HashMap<UInt256, Vec<u8>>,
}

/*
    ============================================================================
    Vote Accounting per Slot
    ============================================================================

    Reference: C++ Slot struct in pool.cpp
*/

/// Error when storing a certificate conflicts with existing certificate
#[derive(Debug, Clone)]
pub enum CertificateStoreError {
    /// Certificate already stored for a different block hash
    ConflictingBlock {
        /// Block hash of the existing certificate
        existing_block: UInt256,
        /// Block hash of the new certificate being stored
        new_block: UInt256,
    },
}

impl Display for CertificateStoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CertificateStoreError::ConflictingBlock { existing_block, new_block } => {
                write!(
                    f,
                    "certificate conflict: existing block {} vs new block {}",
                    &existing_block.to_hex_string()[..8],
                    &new_block.to_hex_string()[..8]
                )
            }
        }
    }
}

impl std::error::Error for CertificateStoreError {}

/// Vote accounting for a slot
///
/// Reference: C++ `struct Slot` in pool.cpp
///
/// Note: All block keys use `UInt256` (candidate_hash) to match TL schema's
/// `consensus.CandidateId` which only contains `(slot, hash)`.
/// The candidate_hash is computed by `compute_candidate_id_hash()` and includes
/// the block's root_hash, file_hash, collated_file_hash, slot, and parent info.
#[derive(Debug)]
struct SlotVotes {
    /// Per-validator votes
    votes: Vec<ValidatorVotes>,

    /// Notarize weight by candidate hash → total weight
    notarize_weight_by_block: HashMap<UInt256, ValidatorWeight>,

    /// Finalize weight by candidate hash → total weight
    finalize_weight_by_block: HashMap<UInt256, ValidatorWeight>,

    /// Total weight that voted notarize OR skip
    notarize_or_skip_weight: ValidatorWeight,

    /// Total weight that voted skip OR skip-fallback
    skip_or_skip_fallback_weight: ValidatorWeight,

    /// Have we published BlockNotarized event?
    block_notarized_published: bool,

    /// Have we published SafeToSkip event?
    safe_to_skip_published: bool,

    /// Have we published BlockFinalized event?
    block_finalized_published: bool,

    /// Have we published SlotSkipped event?
    slot_skipped_published: bool,

    /// Blocks (by candidate_hash) for which we've published SafeToNotar
    safe_to_notar_blocks: HashSet<UInt256>,

    /// Cached notarization certificate (only one per slot)
    /// Created when notarization threshold (2/3) is reached.
    /// Used by candidate resolver to respond to requestCandidate queries.
    /// Reference: C++ pool.cpp `SlotState::certs.notarize_` - single optional
    notarize_certificate: Option<NotarCertPtr>,

    /// Cached finalization certificate (only one per slot)
    /// Created when finalization threshold (2/3) is reached.
    /// Reference: C++ pool.cpp `SlotState::certs.finalize_` - single optional
    finalize_certificate: Option<FinalCertPtr>,

    /// Cached skip certificate for this slot (only one per slot)
    /// Created when skip threshold (2/3) is reached.
    /// Reference: C++ pool.cpp `SlotState::certs.skip_`
    skip_certificate: Option<SkipCertPtr>,
}

impl SlotVotes {
    fn new(num_validators: usize) -> Self {
        Self {
            votes: vec![ValidatorVotes::default(); num_validators],
            notarize_weight_by_block: HashMap::new(),
            finalize_weight_by_block: HashMap::new(),
            notarize_or_skip_weight: 0,
            skip_or_skip_fallback_weight: 0,
            block_notarized_published: false,
            safe_to_skip_published: false,
            block_finalized_published: false,
            slot_skipped_published: false,
            safe_to_notar_blocks: HashSet::new(),
            notarize_certificate: None,
            finalize_certificate: None,
            skip_certificate: None,
        }
    }

    /// Store notarization certificate
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists for the same block (idempotent)
    /// - `Err` if certificate already exists for a different block
    fn store_notarize_certificate(
        &mut self,
        block_hash: &UInt256,
        certificate: NotarCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        if let Some(existing) = &self.notarize_certificate {
            if &existing.vote.block_hash == block_hash {
                // Same block - idempotent, already stored
                return Ok(false);
            } else {
                // Different block - conflict
                return Err(CertificateStoreError::ConflictingBlock {
                    existing_block: existing.vote.block_hash.clone(),
                    new_block: block_hash.clone(),
                });
            }
        }
        self.notarize_certificate = Some(certificate);
        Ok(true)
    }

    /// Store finalization certificate
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists for the same block (idempotent)
    /// - `Err` if certificate already exists for a different block
    fn store_finalize_certificate(
        &mut self,
        block_hash: &UInt256,
        certificate: FinalCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        if let Some(existing) = &self.finalize_certificate {
            if &existing.vote.block_hash == block_hash {
                // Same block - idempotent, already stored
                return Ok(false);
            } else {
                // Different block - conflict
                return Err(CertificateStoreError::ConflictingBlock {
                    existing_block: existing.vote.block_hash.clone(),
                    new_block: block_hash.clone(),
                });
            }
        }
        self.finalize_certificate = Some(certificate);
        Ok(true)
    }

    /// Store skip certificate
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists (idempotent - skip has no block hash)
    fn store_skip_certificate(
        &mut self,
        certificate: SkipCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        if self.skip_certificate.is_some() {
            // Skip certificate already exists - idempotent (no block hash to compare)
            return Ok(false);
        }
        self.skip_certificate = Some(certificate);
        Ok(true)
    }

    /// Get validator votes with bounds checking (returns None if out of bounds)
    fn get_validator_votes(&self, validator_idx: ValidatorIndex) -> Option<&ValidatorVotes> {
        self.votes.get(validator_idx.value() as usize)
    }

    /// Get mutable validator votes with bounds checking (returns None if out of bounds)
    fn get_validator_votes_mut(
        &mut self,
        validator_idx: ValidatorIndex,
    ) -> Option<&mut ValidatorVotes> {
        self.votes.get_mut(validator_idx.value() as usize)
    }

    /*
        ========================================================================
        Certificate Creation
        ========================================================================

        Reference: C++ pool.cpp SlotState::create_cert()

        Creates certificates from stored vote signatures when threshold is reached.
        Certificates are used for:
        - on_block_committed callback (finalization signatures)
        - Candidate resolver responses (notarization certificates)
    */

    /// Create notarization certificate from stored signatures
    ///
    /// Reference: C++ `SlotState::create_cert<NotarizeVote>(vote)`
    ///
    /// Collects all notarize signatures for the given block hash and creates a certificate.
    /// Called when notarization threshold is reached.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `block_hash` - Block candidate hash
    ///
    /// # Returns
    ///
    /// NotarCert containing the vote and all matching signatures
    fn create_notarize_cert(&self, slot: SlotIndex, block_hash: &UInt256) -> NotarCert {
        let vote = NotarizeVote { slot, block_hash: block_hash.clone() };
        let signatures = self.collect_notarize_signatures(block_hash);
        Certificate::new(vote, signatures)
    }

    /// Create finalization certificate from stored signatures
    ///
    /// Reference: C++ `SlotState::create_cert<FinalizeVote>(vote)`
    ///
    /// Collects all finalize signatures for the given block hash and creates a certificate.
    /// Called when finalization threshold is reached.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    /// * `block_hash` - Block candidate hash
    ///
    /// # Returns
    ///
    /// FinalCert containing the vote and all matching signatures
    fn create_finalize_cert(&self, slot: SlotIndex, block_hash: &UInt256) -> FinalCert {
        let vote = FinalizeVote { slot, block_hash: block_hash.clone() };
        let signatures = self.collect_finalize_signatures(block_hash);
        Certificate::new(vote, signatures)
    }

    /// Create skip certificate from stored signatures
    ///
    /// Reference: C++ `SlotState::create_cert<SkipVote>(vote)`
    ///
    /// Collects all skip signatures and creates a certificate.
    /// Called when skip threshold is reached.
    ///
    /// # Arguments
    ///
    /// * `slot` - Slot number
    ///
    /// # Returns
    ///
    /// SkipCert containing the vote and all matching signatures
    #[allow(dead_code)]
    fn create_skip_cert(&self, slot: SlotIndex) -> SkipCert {
        let vote = SkipVote { slot };
        let signatures = self.collect_skip_signatures();
        Certificate::new(vote, signatures)
    }

    /// Get cached notarization certificate for a block
    ///
    /// Returns the cached certificate if notarization threshold was reached
    /// and the certificate is for the requested block, or None otherwise.
    ///
    /// Used by candidate resolver to respond to requestCandidate queries.
    fn get_notarize_certificate(&self, block_hash: &UInt256) -> Option<NotarCertPtr> {
        self.notarize_certificate
            .as_ref()
            .filter(|cert| &cert.vote.block_hash == block_hash)
            .cloned()
    }

    /// Collect notarize signatures for a specific block hash
    fn collect_notarize_signatures(&self, block_hash: &UInt256) -> Vec<VoteSignature> {
        self.votes
            .iter()
            .enumerate()
            .filter_map(|(idx, v)| {
                // Check if this validator voted notarize for this block
                if let (Some(ref vote), Some(ref sig)) = (&v.notarize, &v.notarize_signature) {
                    if &vote.block_hash == block_hash {
                        return Some(VoteSignature::new(
                            ValidatorIndex::new(idx as u32),
                            sig.clone(),
                        ));
                    }
                }
                None
            })
            .collect()
    }

    /// Collect finalize signatures for a specific block hash
    fn collect_finalize_signatures(&self, block_hash: &UInt256) -> Vec<VoteSignature> {
        self.votes
            .iter()
            .enumerate()
            .filter_map(|(idx, v)| {
                // Check if this validator voted finalize for this block
                if let (Some(ref vote), Some(ref sig)) = (&v.finalize, &v.finalize_signature) {
                    if &vote.block_hash == block_hash {
                        return Some(VoteSignature::new(
                            ValidatorIndex::new(idx as u32),
                            sig.clone(),
                        ));
                    }
                }
                None
            })
            .collect()
    }

    /// Collect skip signatures
    fn collect_skip_signatures(&self) -> Vec<VoteSignature> {
        self.votes
            .iter()
            .enumerate()
            .filter_map(|(idx, v)| {
                // Check if this validator voted skip
                if let Some(ref sig) = v.skip_signature {
                    if v.skip.is_some() {
                        return Some(VoteSignature::new(
                            ValidatorIndex::new(idx as u32),
                            sig.clone(),
                        ));
                    }
                }
                None
            })
            .collect()
    }
}

/*
    ============================================================================
    Pending Slot Priority Queue
    ============================================================================
*/

/// Wrapper for BinaryHeap to get min-heap behavior (increasing slot order)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSlot(SlotIndex);

impl Ord for PendingSlot {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        // Reverse ordering for min-heap
        other.0.cmp(&self.0)
    }
}

impl PartialOrd for PendingSlot {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/*
    ============================================================================
    SimplexState - FSM State
    ============================================================================

    Reference:
    - C++ SimplexConsensusImpl + SimplexPoolImpl (merged in Rust)
    - White Paper Algorithm 1 (page 21) - event handlers
    - White Paper Algorithm 2 (page 22) - helper functions

    Design:
    - Single struct combining consensus FSM and vote accounting
    - Self-contained timeout management via check_all() + get_next_timeout()
    - Uses block.rs types (Candidate, CandidateParentInfo, BlockIdExt)
    - Event-based output via internal event queue (pull with pull_event())
    - Independently testable without network or callbacks
*/

/// SimplexState - core consensus state machine with vote accounting
///
/// Combines logic from C++ SimplexConsensusImpl and SimplexPoolImpl.
///
/// # Event Model
///
/// Instead of callbacks, SimplexState produces events that are queued internally.
/// The caller should:
/// 1. Call FSM methods (`on_candidate`, `on_vote`, `check_all`)
/// 2. Pull events with `pull_event()` until it returns `None`
/// 3. Process each event (broadcast vote, notify listener, etc.)
///
/// This design enables:
/// - **Testing**: Inspect produced events without mocking
/// - **Debugging**: Dump event queue for diagnostics
/// - **Tracing**: All FSM outputs go through a single mechanism
///
/// # Timeout Model
///
/// The FSM manages its own timeouts. Clients should:
/// 1. Call `get_next_timeout()` to know when `check_all()` should be called
/// 2. It's safe to call `check_all()` earlier than the timeout
/// 3. Each `check_all()` resets internal timer and updates `next_timeout`
///
/// # Thread Safety
///
/// SimplexState is NOT thread-safe. It should only be accessed from a single thread
/// (the SessionProcessor main loop). All external events are posted to the
/// session's task queue and processed sequentially.
///
/// # Visibility
///
/// This struct is `pub(crate)` - used internally by SessionProcessor.
/// Vote types and `SimplexEvent` are publicly exported from lib.rs.
pub(crate) struct SimplexState {
    /*
        ========================================================================
        Event Queue
        ========================================================================
    */
    /// Event queue for output events
    events: VecDeque<SimplexEvent>,

    /*
        ========================================================================
        Consensus State (from C++ SimplexConsensusImpl)
        ========================================================================
    */
    /// Leader windows (lazily created)
    leader_windows: VecDeque<LeaderWindow>,

    /// Offset for window indexing (first window index = offset)
    leader_window_offset: WindowIndex,

    /// Current leader window index (for timeout management)
    current_leader_window_idx: WindowIndex,

    /// First non-finalized slot
    first_non_finalized_slot: SlotIndex,

    /// Slots with pending blocks (min-heap by slot number)
    pending_slots: BinaryHeap<PendingSlot>,

    /*
        ========================================================================
        Vote Accounting (from C++ SimplexPoolImpl)
        ========================================================================
    */
    /// Per-slot vote accounting
    slot_votes: HashMap<SlotIndex, SlotVotes>,

    /// Number of validators
    num_validators: usize,

    /// Mapping from candidate hash to CandidateId
    ///
    /// Populated by `on_candidate` when a candidate is received.
    /// Used by `BlockFinalizedEvent` to provide full CandidateId including seqno.
    candidate_ids: HashMap<UInt256, CandidateId>,

    /*
        ========================================================================
        Notarized-Parent Progress Cursor (C++ pool `now_`)
        ========================================================================
    */
    /// Progress cursor: the first slot that is NOT progressed yet.
    ///
    /// A slot is considered progressed if it is:
    /// - finalized (slot < first_non_finalized_slot), OR
    /// - has observed notarization certificate, OR
    /// - has skip certificate (`Slot.skipped = true`)
    ///
    /// Mirrors C++ `PoolImpl::now_` (pool.cpp maybe_publish_new_leader_windows()).
    ///
    /// This field is always maintained for state consistency. When
    /// `SimplexStateOptions::use_notarized_parent_chain` is enabled, it is used
    /// to drive leader-window progression / timeout scheduling.
    first_non_progressed_slot: SlotIndex,

    /*
        ========================================================================
        Timeout State
        ========================================================================
    */
    /// Current skip slot (for timeout progression)
    skip_slot: SlotIndex,

    /// Timestamp when current skip_slot times out
    skip_timestamp: Option<SystemTime>,

    /// First block timeout (adaptive)
    first_block_timeout: Duration,

    /// Target rate timeout (adaptive)
    target_rate_timeout: Duration,

    /*
        ========================================================================
        Configuration Cache
        ========================================================================
    */
    /// Slots per leader window
    slots_per_leader_window: u32,

    /// SimplexState options (fallback protocol, etc.)
    opts: SimplexStateOptions,

    /// Throttle counter for `ensure_window_exists` rejection warnings.
    /// Prevents log flooding when standstill re-broadcasts reference far-future windows.
    window_reject_count: u64,
}

impl SimplexState {
    /*
        ========================================================================
        Constructor
        ========================================================================
    */

    /// Create new SimplexState instance
    ///
    /// Initializes the FSM with:
    /// - Empty event queue
    /// - First leader window with genesis as available base
    /// - Default timeout state
    /// - Empty vote accounting
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description with validators and options
    /// * `opts` - SimplexState-specific options (fallback protocol, etc.)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - `slots_per_leader_window` is 0 (would cause division by zero)
    /// - `num_validators` is 0 (no validators in session)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // C++ compatible mode (default)
    /// let state = SimplexState::new(&desc, SimplexStateOptions::default())?;
    /// ```
    pub fn new(desc: &SessionDescription, opts: SimplexStateOptions) -> Result<Self> {
        let slots_per_window = desc.opts().slots_per_leader_window;
        let num_validators = desc.get_total_nodes() as usize;

        // Validate parameters at construction time
        if slots_per_window == 0 {
            fail!("SimplexState::new: slots_per_leader_window must be > 0");
        }

        if num_validators == 0 {
            fail!("SimplexState::new: num_validators must be > 0");
        }

        log::trace!(
            "SimplexState::new: initializing FSM with {} validators, {} slots/window, opts={:?}",
            num_validators,
            slots_per_window,
            opts
        );

        let first_block_timeout = desc.opts().first_block_timeout;
        let target_rate_timeout = desc.opts().target_rate;

        let mut state = Self {
            events: VecDeque::new(),
            leader_windows: VecDeque::new(),
            leader_window_offset: WindowIndex(0),
            current_leader_window_idx: WindowIndex(0),
            first_non_finalized_slot: SlotIndex(0),
            pending_slots: BinaryHeap::new(),
            slot_votes: HashMap::new(),
            num_validators,
            candidate_ids: HashMap::new(),
            first_non_progressed_slot: SlotIndex(0),
            skip_slot: SlotIndex(0),
            skip_timestamp: None,
            first_block_timeout,
            target_rate_timeout,
            slots_per_leader_window: slots_per_window,
            opts,
            window_reject_count: 0,
        };

        // Initialize first window with genesis (None) as available base
        // Reference: C++ start_up() → window.available_bases.insert(std::nullopt)
        let window = state
            .window_at_mut(WindowIndex(0))
            .ok_or_else(|| error!("SimplexState::new: failed to initialize first window"))?;
        window.available_bases.insert(None);
        // C++ pool.cpp: state_->slot_at(0)->state->available_base = RawParentId{};
        // (available_base is optional-of-optional; RawParentId{} = nullopt = genesis)
        window.slots[0].available_base = Some(None);

        // Timeouts are NOT armed here.  The FSM starts with skip_timestamp=None
        // so that no skip cascade fires before the session is actually started.
        // SessionProcessor::start() calls set_timeouts() at the correct moment
        // (after overlay warmup and bootstrap recovery), matching C++ where
        // timeouts are only armed after the Start event.

        Ok(state)
    }

    /*
        ========================================================================
        Slot / Window Internal Helpers
        ========================================================================
    */

    /// Returns a reference to per-slot state (if the window is still tracked).
    fn get_slot_ref(&self, desc: &SessionDescription, slot: SlotIndex) -> Option<&Slot> {
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;
        self.get_window(window_idx).map(|w| &w.slots[offset])
    }

    /// Returns a mutable reference to per-slot state (if the window is still tracked).
    ///
    /// Ensures the window exists when `slot` is in the tracked range.
    fn get_slot_mut(&mut self, desc: &SessionDescription, slot: SlotIndex) -> Option<&mut Slot> {
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;
        self.ensure_window_exists(window_idx);
        self.get_window_mut(window_idx).map(|w| &mut w.slots[offset])
    }

    /// Get per-slot available base (C++ `SlotState::available_base`)
    ///
    /// This is an optional-of-optional to match C++ `RawParentId`:
    /// - `None` = base unknown (not yet determined)
    /// - `Some(None)` = genesis base (`RawParentId{}`)
    /// - `Some(Some(id))` = concrete parent candidate id
    ///
    /// Reference: C++ pool.cpp `SlotState::available_base`, `types.h RawParentId`.
    fn get_slot_available_base(
        &self,
        desc: &SessionDescription,
        slot: SlotIndex,
    ) -> Option<CandidateParent> {
        self.get_slot_ref(desc, slot).and_then(|s| s.available_base.clone())
    }

    /// Check if a slot has reached skip certificate threshold (2/3)
    ///
    /// NOTE: This reflects **skip certificate** state, not local `voted_skip`.
    ///
    /// Reference: C++ pool.cpp `SlotState::skipped`.
    fn is_slot_skipped_cert(&self, desc: &SessionDescription, slot: SlotIndex) -> bool {
        self.get_slot_ref(desc, slot).map(|s| s.skipped).unwrap_or(false)
    }

    /// Ensure window exists at index.
    ///
    /// Defense-in-depth: refuses to allocate beyond `MAX_FUTURE_SLOTS` horizon
    /// even if the caller forgot to pre-validate.
    fn ensure_window_exists(&mut self, idx: WindowIndex) {
        let max_slot = self.first_non_finalized_slot.value() + MAX_FUTURE_SLOTS;
        let max_window = WindowIndex(max_slot / self.slots_per_leader_window + 1);
        if idx > max_window {
            self.window_reject_count += 1;
            if self.window_reject_count <= 3 || self.window_reject_count % 10000 == 0 {
                log::warn!(
                    "SimplexState::ensure_window_exists: REJECTED window {} > max {} \
                    (defense-in-depth, occurrence #{})",
                    idx,
                    max_window,
                    self.window_reject_count,
                );
            }
            return;
        }

        while idx >= self.leader_window_offset + self.leader_windows.len() as u32 {
            let new_idx = self.leader_window_offset + self.leader_windows.len() as u32;
            let start_slot = new_idx * self.slots_per_leader_window;
            let end_slot = start_slot + self.slots_per_leader_window - 1;

            log::trace!(
                "SimplexState::ensure_window_exists: created {} ({}..{})",
                new_idx,
                start_slot,
                end_slot
            );

            self.leader_windows.push_back(LeaderWindow::new(
                new_idx,
                start_slot,
                self.slots_per_leader_window,
            ));
        }
    }

    /// Get window by index (read-only)
    fn get_window(&self, idx: WindowIndex) -> Option<&LeaderWindow> {
        if idx < self.leader_window_offset {
            return None;
        }
        let offset = (idx - self.leader_window_offset) as usize;
        self.leader_windows.get(offset)
    }

    /// Get window by index (mutable)
    fn get_window_mut(&mut self, idx: WindowIndex) -> Option<&mut LeaderWindow> {
        if idx < self.leader_window_offset {
            return None;
        }
        let offset = (idx - self.leader_window_offset) as usize;
        self.leader_windows.get_mut(offset)
    }

    /// Alias for get_window_mut that also ensures window exists
    fn window_at_mut(&mut self, idx: WindowIndex) -> Option<&mut LeaderWindow> {
        self.ensure_window_exists(idx);
        self.get_window_mut(idx)
    }

    /// Get or create slot votes
    fn slot_votes_at(&mut self, slot: SlotIndex) -> &mut SlotVotes {
        let num_validators = self.num_validators;
        self.slot_votes.entry(slot).or_insert_with(|| SlotVotes::new(num_validators))
    }

    /*
        ========================================================================
        Bootstrap State Application (Restart Support)
        ========================================================================
    */

    /// Set first non-finalized slot from bootstrap data and prune old windows
    ///
    /// Reference: C++ state.h notify_finalized() - sets first_non_finalized_slot_
    /// and advances offset_ while pruning old windows from deque.
    ///
    /// This is called during bootstrap to set the starting slot based on
    /// max finalized slot + 1. It also prunes leader_windows and advances
    /// leader_window_offset to avoid O(N) window creation from slot 0.
    ///
    /// # Arguments
    /// * `slot` - The first slot that has NOT been finalized (max finalized + 1)
    pub fn set_first_non_finalized_slot(&mut self, slot: SlotIndex) {
        // C++: first_non_finalized_slot_ = std::max(first_non_finalized_slot_, slot + 1);
        // Use max to prevent going backwards
        if slot > self.first_non_finalized_slot {
            self.first_non_finalized_slot = slot;
        }

        // Keep notarized-parent chain tracking consistent with finalized boundary:
        // any slot < first_non_finalized_slot is already finalized and therefore progressed.
        // `first_non_progressed_slot` should never point into the finalized region.
        if self.first_non_finalized_slot > self.first_non_progressed_slot {
            self.first_non_progressed_slot = self.first_non_finalized_slot;
        }

        log::trace!(
            "SimplexState::set_first_non_finalized_slot: setting to {} (slots_per_window={})",
            self.first_non_finalized_slot.value(),
            self.slots_per_leader_window
        );

        // Calculate needed window
        // C++: td::uint32 needed_window = first_non_finalized_slot_ / slots_per_leader_window_;
        let needed_window =
            WindowIndex(self.first_non_finalized_slot.value() / self.slots_per_leader_window);

        // Prune old windows from deque
        // C++: while (!windows_.empty() && offset_ < needed_window) { windows_.pop_front(); ++offset_; }
        while !self.leader_windows.is_empty() && self.leader_window_offset < needed_window {
            log::trace!(
                "SimplexState::set_first_non_finalized_slot: pruning window {} (offset advancing to {})",
                self.leader_window_offset.value(),
                self.leader_window_offset.value() + 1
            );
            self.leader_windows.pop_front();
            self.leader_window_offset += 1;
        }

        // Advance offset even if deque is empty (bootstrap case)
        // C++: if (offset_ < needed_window) { offset_ = needed_window; }
        if self.leader_window_offset < needed_window {
            log::trace!(
                "SimplexState::set_first_non_finalized_slot: advancing offset from {} to {} (no windows to prune)",
                self.leader_window_offset.value(),
                needed_window.value()
            );
            self.leader_window_offset = needed_window;
        }

        log::trace!(
            "SimplexState::set_first_non_finalized_slot: done, first_non_finalized={}, leader_window_offset={}, windows.len={}",
            self.first_non_finalized_slot.value(),
            self.leader_window_offset.value(),
            self.leader_windows.len()
        );
    }

    /// Mark a slot as having been voted on by us (prevents double-voting on restart)
    ///
    /// Reference: C++ consensus.cpp start_up() - loops over bootstrap_votes
    /// for local validator and marks voted_notar/voted_final/voted_skip.
    ///
    /// This is called for OUR OWN votes loaded from DB to prevent
    /// re-voting for the same slot after restart.
    ///
    /// # Arguments
    /// * `vote` - Our previously persisted vote
    pub fn mark_slot_voted_on_restart(&mut self, desc: &SessionDescription, vote: &Vote) {
        let slot = match vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
            Vote::NotarizeFallback(v) => v.slot,
            Vote::SkipFallback(v) => v.slot,
        };
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;

        // After restart recovery sets `first_non_finalized_slot`, we may prune old leader windows
        // by advancing `leader_window_offset`. Votes for slots in pruned windows are irrelevant
        // (they are already finalized / outside of tracked state), so ignore them to avoid panics.
        if window_idx < self.leader_window_offset {
            log::trace!(
                "SimplexState::mark_slot_voted_on_restart: ignoring vote for slot {} (window {} < leader_window_offset={})",
                slot.value(),
                window_idx.value(),
                self.leader_window_offset.value()
            );
            return;
        }

        // Ensure window exists
        let _ = self.window_at_mut(window_idx);

        if let Some(window) = self.get_window_mut(window_idx) {
            if offset < window.slots.len() {
                match vote {
                    Vote::Notarize(v) => {
                        // C++: slot->state->voted_notar = notar_vote.id
                        window.slots[offset].is_voted = true;
                        window.slots[offset].voted_notar =
                            Some(CandidateParentInfo { slot, hash: v.block_hash.clone() });
                        log::trace!(
                            "SimplexState::mark_slot_voted_on_restart: slot {} marked voted_notar={}:{}",
                            slot.value(),
                            slot.value(),
                            &v.block_hash.to_hex_string()[..8]
                        );
                    }
                    Vote::NotarizeFallback(v) => {
                        window.slots[offset].is_voted = true;
                        window.slots[offset].voted_notar =
                            Some(CandidateParentInfo { slot, hash: v.block_hash.clone() });
                        log::trace!(
                            "SimplexState::mark_slot_voted_on_restart: slot {} marked voted_notar_fb={}:{}",
                            slot.value(),
                            slot.value(),
                            &v.block_hash.to_hex_string()[..8]
                        );
                    }
                    Vote::Finalize(_) => {
                        // C++: slot->state->voted_final = true
                        window.slots[offset].is_voted = true;
                        window.slots[offset].its_over = true;
                        window.slots[offset].voted_final = true;
                        log::trace!(
                            "SimplexState::mark_slot_voted_on_restart: slot {} marked voted_final=true",
                            slot.value()
                        );
                    }
                    Vote::Skip(_) | Vote::SkipFallback(_) => {
                        // C++: slot->state->voted_skip = true
                        window.slots[offset].is_voted = true;
                        window.slots[offset].voted_skip = true;
                        window.slots[offset].is_bad_window = true;
                        window.slots[offset].pending_block = None;
                        log::trace!(
                            "SimplexState::mark_slot_voted_on_restart: slot {} marked voted_skip=true",
                            slot.value()
                        );
                    }
                };
            }
        }
    }

    /// Generate skip votes for missed window slots on restart
    ///
    /// Reference: C++ consensus.cpp start_up() - if first_nonannounced_window > 0,
    /// broadcasts SkipVote for all slots in the previous window that haven't been finalized.
    ///
    /// This is called during restart to "catch up" on missed voting opportunities.
    /// Queues BroadcastVote events for each slot that needs a skip vote.
    ///
    /// # Arguments
    /// * `first_nonannounced_window` - The window index we need to catch up from
    /// * `slots_per_window` - Number of slots per leader window
    ///
    /// # Returns
    /// Number of skip votes queued
    pub fn generate_restart_skip_votes(
        &mut self,
        first_nonannounced_window: WindowIndex,
        slots_per_window: u32,
    ) -> u32 {
        if first_nonannounced_window.value() == 0 {
            return 0;
        }

        // C++: start_slot = (window - 1) * slots_per_leader_window
        //      end_slot = window * slots_per_leader_window
        let start_slot = (first_nonannounced_window.value() - 1) * slots_per_window;
        let end_slot = first_nonannounced_window.value() * slots_per_window;

        let mut skip_count = 0u32;
        for slot_num in start_slot..end_slot {
            let slot = SlotIndex(slot_num);
            let window_idx = WindowIndex(slot_num / slots_per_window);
            let offset = (slot_num % slots_per_window) as usize;

            // If the window is already pruned (leader_window_offset advanced), there's nothing to do.
            if window_idx < self.leader_window_offset {
                continue;
            }

            // Ensure window exists
            let _ = self.window_at_mut(window_idx);

            // Check if slot is already finalized (its_over in C++)
            let should_skip = if let Some(window) = self.get_window(window_idx) {
                offset < window.slots.len() && !window.slots[offset].its_over
            } else {
                false
            };

            if should_skip {
                if let Some(window) = self.get_window_mut(window_idx) {
                    if offset < window.slots.len() {
                        // Mark local skip state BEFORE enqueueing the broadcast.
                        // Reference: C++ consensus.cpp start_up() sets voted_skip=true before publishing SkipVote.
                        window.slots[offset].is_voted = true;
                        window.slots[offset].voted_skip = true;
                        window.slots[offset].is_bad_window = true;
                        window.slots[offset].pending_block = None;

                        log::trace!(
                            "SimplexState::generate_restart_skip_votes: queueing skip for slot {}",
                            slot.value()
                        );

                        // Queue broadcast event
                        self.push_event_back(SimplexEvent::BroadcastVote(Vote::Skip(SkipVote {
                            slot,
                        })));
                        skip_count += 1;
                    }
                }
            }
        }

        if skip_count > 0 {
            log::info!(
                "SimplexState::generate_restart_skip_votes: queued {} skip votes for window {} (slots {}..{})",
                skip_count,
                first_nonannounced_window.value(),
                start_slot,
                end_slot
            );
        }

        skip_count
    }

    /*
        ========================================================================
        Event Queue Management
        ========================================================================
    */

    /// Pull the next event from the queue
    ///
    /// Returns `Some(event)` if there are pending events, `None` otherwise.
    /// Events should be pulled and processed after any FSM operation.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let _ = state.on_candidate(&desc, candidate);
    /// while let Some(event) = state.pull_event() {
    ///     match event {
    ///         SimplexEvent::BroadcastVote(vote) => receiver.send_vote(vote),
    ///         SimplexEvent::BlockFinalized(e) => listener.on_block_committed(...),
    ///         // ... handle other events
    ///     }
    /// }
    /// ```
    pub fn pull_event(&mut self) -> Option<SimplexEvent> {
        self.events.pop_front()
    }

    /// Get the number of pending events in the queue
    #[allow(dead_code)]
    pub fn pending_event_count(&self) -> usize {
        self.events.len()
    }

    /// Check if there are any pending events
    #[cfg(test)]
    pub fn has_pending_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// Set first_non_finalized_slot for testing (bypasses normal FSM flow)
    ///
    /// Used in unit tests to satisfy SessionProcessor assertions when
    /// injecting events without running full FSM vote accumulation.
    #[cfg(test)]
    pub fn try_skip_window_for_test(&mut self, window_idx: WindowIndex) {
        self.try_skip_window(window_idx);
    }

    #[cfg(test)]
    pub fn set_first_non_finalized_slot_for_test(&mut self, slot: SlotIndex) {
        self.first_non_finalized_slot = slot;
        // Also advance first_non_progressed_slot to match (finalized implies progressed)
        if self.first_non_progressed_slot < slot {
            self.first_non_progressed_slot = slot;
        }
    }

    /*
        ========================================================================
        Trace Logging Helpers
        ========================================================================
    */

    /// Format parent info for trace logging: "{slot}:{hash_prefix}" or "genesis"
    fn format_parent(parent: Option<&CandidateParentInfo>) -> String {
        parent
            .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
            .unwrap_or_else(|| "genesis".to_string())
    }

    /// Format CandidateId parent for trace logging: "{slot}:{hash_prefix}" or "genesis"
    fn format_parent_id(parent: Option<&crate::block::CandidateId>) -> String {
        parent
            .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
            .unwrap_or_else(|| "genesis".to_string())
    }

    /// Format block reference for trace logging: "{slot}:{hash_prefix}"
    fn format_block(slot: SlotIndex, hash: &UInt256) -> String {
        format!("{}:{}", slot, &hash.to_hex_string()[..8])
    }

    /// Format vote for trace logging
    fn format_vote(vote: &Vote) -> String {
        match vote {
            Vote::Notarize(v) => {
                format!("notarize {}", Self::format_block(v.slot, &v.block_hash))
            }
            Vote::Finalize(v) => {
                format!("finalize {}", Self::format_block(v.slot, &v.block_hash))
            }
            Vote::Skip(v) => format!("skip {}", v.slot),
            Vote::NotarizeFallback(v) => {
                format!("notar-fb {}", Self::format_block(v.slot, &v.block_hash))
            }
            Vote::SkipFallback(v) => format!("skip-fb {}", v.slot),
        }
    }

    /// Format event for trace logging
    fn format_event(event: &SimplexEvent) -> String {
        match event {
            SimplexEvent::BroadcastVote(v) => format!("BroadcastVote({})", Self::format_vote(v)),
            SimplexEvent::BlockFinalized(e) => {
                let seqno_str = e
                    .block_id
                    .as_ref()
                    .map(|id| format!("seqno={}", id.seq_no))
                    .unwrap_or_else(|| "seqno=?".to_string());
                format!(
                    "BlockFinalized({}, {})",
                    Self::format_block(e.slot, &e.block_hash),
                    seqno_str
                )
            }
            SimplexEvent::SlotSkipped(e) => format!("SlotSkipped({})", e.slot),
            SimplexEvent::NotarizationReached(e) => {
                format!(
                    "NotarizationReached({}, {} sigs)",
                    Self::format_block(e.slot, &e.block_hash),
                    e.certificate.signatures.len()
                )
            }
            SimplexEvent::SkipCertificateReached(e) => {
                format!(
                    "SkipCertificateReached(slot={}, {} sigs)",
                    e.slot,
                    e.certificate.signatures.len()
                )
            }
            SimplexEvent::FinalizationReached(e) => {
                format!(
                    "FinalizationReached({}, {} sigs)",
                    Self::format_block(e.slot, &e.block_hash),
                    e.certificate.signatures.len()
                )
            }
        }
    }

    /*
        ========================================================================
        Event Queue
        ========================================================================
    */

    /// Push an event to the back of the queue (internal use)
    fn push_event_back(&mut self, event: SimplexEvent) {
        log::trace!("SimplexState::push_event_back: {}", Self::format_event(&event));
        self.events.push_back(event);
    }

    /// Push an event to the front of the queue
    ///
    /// Used to return an event that cannot be processed yet (e.g., BlockFinalized
    /// when we haven't received the block body via broadcast).
    /// The event will be processed on the next `pull_event()` call.
    pub fn push_event_front(&mut self, event: SimplexEvent) {
        log::trace!("SimplexState::push_event_front: {}", Self::format_event(&event));
        self.events.push_front(event);
    }

    /// Push a broadcast vote event
    ///
    /// Filters out fallback votes (NotarizeFallback, SkipFallback) when
    /// enable_fallback_protocol is false.
    fn broadcast_vote(&mut self, vote: Vote) {
        // Filter fallback votes in C++ compatible mode
        if !self.opts.enable_fallback_protocol {
            match &vote {
                Vote::NotarizeFallback(v) => {
                    log::warn!(
                        "SimplexState::broadcast_vote: FILTERED notar-fb (fallback disabled) slot={}",
                        v.slot
                    );
                    return;
                }
                Vote::SkipFallback(v) => {
                    log::warn!(
                        "SimplexState::broadcast_vote: FILTERED skip-fb (fallback disabled) slot={}",
                        v.slot
                    );
                    return;
                }
                _ => {} // Allow Notarize, Finalize, Skip
            }
        }
        self.push_event_back(SimplexEvent::BroadcastVote(vote));
    }

    /*
        ========================================================================
        Invariants Checking
        ========================================================================

        Reference: C++ pool.cpp check_invariants()

        The C++ implementation (enable_fallback_protocol=false) checks:
        1. notarize + finalize must have same candidate id (if both exist)
        2. finalize + skip is misbehavior (conflicting votes)

        Full Alpenglow (enable_fallback_protocol=true) is stricter:
        1. notarize + skip is misbehavior (a validator cannot hedge)
        2. notarize + finalize must have same candidate id
        3. finalize + skip is misbehavior

        These invariants are checked at the start of check_all() and
        check_thresholds_and_trigger() to ensure state consistency.
    */

    /// Check all invariants on the current state
    ///
    /// This function asserts that all per-validator vote invariants hold.
    /// It should be called at the start of `check_all()` and `check_thresholds_and_trigger()`.
    ///
    /// # Panics
    /// Panics if any invariant is violated - this indicates a bug in the FSM logic.
    ///
    /// Reference: C++ pool.cpp check_invariants()
    fn check_invariants(&self, desc: &SessionDescription) {
        for (&slot_id, slot_votes) in &self.slot_votes {
            for (validator_idx, votes) in slot_votes.votes.iter().enumerate() {
                self.check_validator_invariants(
                    desc,
                    slot_id,
                    ValidatorIndex::from(validator_idx),
                    votes,
                );
            }
        }
    }

    /// Check invariants for a single validator's votes in a slot
    ///
    /// # Invariants (C++ compatible, enable_fallback_protocol=false)
    ///
    /// 1. If both notarize and finalize exist, they must be for the same candidate:
    ///    `notarize.id == finalize.id`
    ///
    /// 2. Finalize + Skip is conflicting (misbehavior):
    ///    `!(finalize.is_some() && skip.is_some())`
    ///
    /// # Invariants (Full Alpenglow, enable_fallback_protocol=true)
    ///
    /// All of the above, plus:
    ///
    /// 3. Notarize + Skip is conflicting (a validator cannot hedge):
    ///    `!(notarize.is_some() && skip.is_some())`
    ///
    /// Reference: C++ pool.cpp check_invariants()
    /// Reference: Solana Alpenglow White Paper (May 2025), voting rules
    fn check_validator_invariants(
        &self,
        _desc: &SessionDescription,
        slot: SlotIndex,
        validator_idx: ValidatorIndex,
        votes: &ValidatorVotes,
    ) {
        // Invariant 1: If both notarize and finalize exist, they must be for the same block
        // (same slot and same block_hash)
        if let (Some(notarize), Some(finalize)) = (&votes.notarize, &votes.finalize) {
            assert!(
                notarize.slot == finalize.slot && notarize.block_hash == finalize.block_hash,
                "SimplexState INVARIANT VIOLATION: {}/{} has conflicting notarize/finalize: \
                notarize=({}, hash={}) != finalize=({}, hash={})",
                validator_idx,
                slot,
                notarize.slot,
                notarize.block_hash.to_hex_string(),
                finalize.slot,
                finalize.block_hash.to_hex_string()
            );
        }

        // Invariant 2: Finalize + Skip is misbehavior (applies to both modes)
        assert!(
            !(votes.finalize.is_some() && votes.skip.is_some()),
            "SimplexState INVARIANT VIOLATION: {}/{} has both finalize and skip votes \
            (finalize={:?}, skip={:?})",
            validator_idx,
            slot,
            votes.finalize,
            votes.skip
        );

        // Invariant 3: Notarize + Skip is misbehavior when not allowed
        // When allow_skip_after_notarize=false (Alpenglow strict mode):
        //   A validator cannot hedge by voting both notarize and skip
        if !self.opts.allow_skip_after_notarize {
            assert!(
                !(votes.notarize.is_some() && votes.skip.is_some()),
                "SimplexState INVARIANT VIOLATION: {}/{} has both notarize \
                and skip votes (notarize={:?}, skip={:?})",
                validator_idx,
                slot,
                votes.notarize,
                votes.skip
            );
        }
    }

    /*
        ========================================================================
        Timeout Management
        ========================================================================
    */

    /// Get the next timeout timestamp
    ///
    /// Returns the timestamp when `check_all()` should be called.
    pub fn get_next_timeout(&self) -> Option<SystemTime> {
        self.skip_timestamp
    }

    /// Set timeouts for the current window
    ///
    /// Reference: C++ set_timeouts()
    ///
    /// Alpenglow Algorithm 2:
    /// ```text
    /// function setTimeouts(s)   // s is first slot of window
    ///   for i ∈ windowSlots(s) do   // set timeouts for all slots
    ///     schedule event Timeout(i) at time clock()+Δtimeout+(i−s+1)·Δblock
    /// ```
    pub(crate) fn set_timeouts(&mut self, desc: &SessionDescription) {
        let window_start = self.current_leader_window_idx * self.slots_per_leader_window;

        self.skip_slot = window_start;
        //TODO: LK: in C++ first slot in a window has timeout first_block_timeout without target_rate_timeout
        self.skip_timestamp =
            Some(desc.get_time() + self.first_block_timeout + self.target_rate_timeout);

        log::warn!(
            "SimplexState::set_timeouts: ({}/{}) scheduling timeout in {:.3}s \
            (first_block={:.3}s, target_rate={:.3}s)",
            self.current_leader_window_idx,
            self.skip_slot,
            (self.first_block_timeout + self.target_rate_timeout).as_secs_f64(),
            self.first_block_timeout.as_secs_f64(),
            self.target_rate_timeout.as_secs_f64(),
        );
    }

    /// Restore default timeouts (reset adaptive backoff)
    fn restore_default_timeouts(&mut self, desc: &SessionDescription) {
        self.target_rate_timeout = desc.opts().target_rate;
        self.first_block_timeout = desc.opts().first_block_timeout;

        log::trace!(
            "SimplexState::restore_default_timeouts: reset to first_block={:.3}s, \
            target_rate={:.3}s",
            self.first_block_timeout.as_secs_f64(),
            self.target_rate_timeout.as_secs_f64()
        );
    }

    /// Check all pending actions and timeouts
    ///
    /// This is the main FSM tick. Should be called:
    /// - When `get_next_timeout()` has elapsed
    /// - After processing incoming events
    pub fn check_all(&mut self, desc: &SessionDescription) {
        // Check invariants at the start of check_all
        self.check_invariants(desc);

        if log::log_enabled!(log::Level::Trace) {
            let time = desc.get_time();
            let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            log::trace!("SimplexState::check_all: time={:.6}", secs);
        }

        // Process expired timeouts
        // Reference: C++ alarm() → upon Timeout(s) do
        self.process_timeouts(desc);

        // Dump state at trace level (compact one-line format)
        if log::log_enabled!(log::Level::Trace) {
            log::trace!("SimplexState::check_all: done {}", self.debug_dump(desc, false));
        }
    }

    /// Process expired timeouts
    ///
    /// Reference: C++ alarm()
    ///
    /// Alpenglow Algorithm 1:
    /// ```text
    /// upon Timeout(s) do
    ///   if Voted ∉ state[s] then
    ///     trySkipWindow(s)
    /// ```
    fn process_timeouts(&mut self, desc: &SessionDescription) {
        // Check if we have a timeout pending
        let Some(mut skip_timestamp) = self.skip_timestamp else {
            log::trace!("SimplexState::process_timeouts: no timeout pending");
            return;
        };

        // Process all elapsed timeouts (time <= current time means expired)
        while !desc.is_in_future(skip_timestamp) {
            let slot_id = self.skip_slot;

            log::trace!("SimplexState::process_timeouts: timeout for slot {}", slot_id);

            // Advance to next slot's timeout
            skip_timestamp = skip_timestamp + self.target_rate_timeout;
            self.skip_timestamp = Some(skip_timestamp);
            self.skip_slot += 1;

            // Skip if slot is already finalized
            if slot_id < self.first_non_finalized_slot {
                log::trace!(
                    "SimplexState::process_timeouts: slot {} is already finalized, skipping",
                    slot_id
                );
                continue;
            }

            // Get slot state
            let window_idx = desc.get_window_idx(slot_id);
            let offset = desc.get_slot_offset_in_window(slot_id) as usize;

            // Ensure window exists
            self.ensure_window_exists(window_idx);

            // Check if we should skip the timeout:
            // - Alpenglow (enable_fallback_protocol=true): Check is_voted (any vote blocks skip)
            // - C++ compatible (enable_fallback_protocol=false): Check voted_final OR voted_skip
            //
            // C++ alarm() checks voted_final and fires once per window (one-shot alarm).
            // Rust process_timeouts fires per-slot, so we must also check voted_skip to
            // prevent repeated skip vote broadcasts for the same window.
            // Reference: C++ consensus.cpp alarm(): if (!affected_slot->voted_final)
            let should_skip_timeout = {
                let window = self.get_window(window_idx);
                if let Some(window) = window {
                    if self.opts.enable_fallback_protocol {
                        // Alpenglow: Any vote blocks timeout (Voted ∈ state[s])
                        window.slots[offset].is_voted
                    } else {
                        // C++: voted_final or voted_skip blocks timeout.
                        // C++ alarm is one-shot so only checks voted_final, but Rust fires
                        // per-slot so we also check voted_skip to avoid re-broadcasting.
                        window.slots[offset].voted_final || window.slots[offset].voted_skip
                    }
                } else {
                    continue;
                }
            };

            // Skip if condition is met
            if !should_skip_timeout {
                // Get slot state for logging
                let (is_voted, its_over) = self
                    .get_window(window_idx)
                    .map(|w| (w.slots[offset].is_voted, w.slots[offset].its_over))
                    .unwrap_or((false, false));

                log::trace!(
                    "SimplexState::process_timeouts: ({}/{}) timeout expired, voted={}, its_over={} -> skip window",
                    window_idx,
                    slot_id,
                    is_voted,
                    its_over
                );

                // Mark window as having timeouts
                if let Some(window) = self.get_window_mut(window_idx) {
                    window.had_timeouts = true;
                }

                // Alpenglow: trySkipWindow(s)
                self.try_skip_window(window_idx);

                // C++ compatibility: skip entire remaining window at once, then BREAK.
                // Reference: C++ consensus.cpp alarm() lines 120-133:
                //   C++ fires alarm once and skips ALL remaining slots in the window,
                //   then sets timeout_slot_ = window_end and reschedules.
                //   Between alarm firings, incoming events (NotarizationObserved,
                //   skip certs from peers) can advance timeout_slot_ past active slots.
                //   We break after one window to give incoming events a chance to
                //   advance skip_slot before we vote skip for more slots.
                if !self.opts.enable_fallback_protocol {
                    let window_end_slot = (window_idx + 1) * self.slots_per_leader_window;
                    if self.skip_slot < window_end_slot {
                        log::debug!(
                            "SimplexState::process_timeouts: C++ window skip: \
                            advancing skip_slot {} -> {} (window_end)",
                            self.skip_slot,
                            window_end_slot
                        );
                        self.skip_slot = window_end_slot;
                    }
                    // Schedule next timeout at target_rate from now (not accumulated)
                    skip_timestamp = desc.get_time() + self.target_rate_timeout;
                    self.skip_timestamp = Some(skip_timestamp);
                    break;
                }
            }
        }
    }

    /// Apply adaptive timeout backoff based on previous window's timeout history
    ///
    /// This is used by both:
    /// - `on_window_base_ready()` (legacy finalization-driven window progression)
    /// - `advance_leader_window_on_progress_cursor()` (notarized-parent-chain mode)
    ///
    /// Reference: C++ pool.cpp (adaptive backoff logic in window progression)
    ///
    /// If previous window had timeouts, increase timeout by factor (with max cap).
    /// Otherwise, restore defaults.
    fn apply_adaptive_timeout_backoff(
        &mut self,
        desc: &SessionDescription,
        window_idx: WindowIndex,
        log_context: &str,
    ) {
        let start_slot = window_idx.window_start(self.slots_per_leader_window);
        let had_timeouts = self
            .get_window(self.current_leader_window_idx)
            .map(|w| w.had_timeouts)
            .unwrap_or(false);

        if had_timeouts {
            let factor = desc.opts().timeout_increase_factor;
            let max_delay = desc.opts().max_backoff_delay;

            // Only back off first_block_timeout, not target_rate_timeout.
            // C++ reference (consensus.cpp:98-99) only backs off first_block_timeout_s_,
            // keeping target_rate_s_ constant. Backing off target_rate causes the full
            // rotation of 16 slots to take 16s instead of 8s, making blocks from remote
            // leaders arrive after the skip timeout and preventing finalization.
            self.first_block_timeout = (self.first_block_timeout.mul_f64(factor)).min(max_delay);

            log::trace!(
                "{}: ({}/{}) adaptive backoff applied -> first={:.3}s target={:.3}s",
                log_context,
                window_idx,
                start_slot,
                self.first_block_timeout.as_secs_f64(),
                self.target_rate_timeout.as_secs_f64()
            );
        } else {
            log::trace!(
                "{}: ({}/{}) no timeouts in prev window, restoring defaults",
                log_context,
                window_idx,
                start_slot
            );
            self.restore_default_timeouts(desc);
        }
    }

    /*
        ========================================================================
        External Event Handlers (Algorithm 1)
        ========================================================================
    */

    /// Handle incoming block candidate
    ///
    /// Reference: Alpenglow Algorithm 1, "upon Block(s, hash, hashparent) do"
    ///
    /// ```text
    /// if tryNotar(Block(s, hash, hashparent)) then
    ///     checkPendingBlocks()
    /// else if Voted ∉ state[s] then
    ///     pendingBlocks[s] ← Block(s, hash, hashparent)
    /// ```
    ///
    /// # Errors
    ///
    /// Returns error if candidate has invalid parameters (misbehavior).
    pub fn on_candidate(&mut self, desc: &SessionDescription, candidate: Candidate) -> Result<()> {
        let slot = candidate.id.slot;
        let leader = candidate.leader;
        let window_idx = desc.get_window_idx(slot);

        log::trace!(
            "SimplexState::on_candidate: ({}/{}/{}) received {} parent={} leader={}",
            desc.get_self_idx(),
            window_idx,
            slot,
            Self::format_block(slot, &candidate.id.hash),
            Self::format_parent_id(candidate.parent_id.as_ref()),
            leader
        );

        // Validate leader index
        // Alpenglow: Each slot has a designated leader from the set of nodes
        if leader.value() >= self.num_validators as u32 {
            log::trace!(
                "SimplexState::on_candidate: ({}/{}) INVALID leader {} >= max {}, dropping",
                window_idx,
                slot,
                leader,
                self.num_validators
            );
            fail!(
                "SimplexState::on_candidate: invalid leader {} (max={}), dropping candidate for slot {}",
                leader,
                self.num_validators,
                slot
            );
        }

        // Ignore finalized slots (not an error, just skip)
        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::on_candidate: ({}/{}) slot already finalized (first_non_finalized={}), ignoring",
                window_idx,
                slot,
                self.first_non_finalized_slot
            );
            return Ok(());
        }

        // Reject far-future slots (DoS protection)
        if self.is_slot_too_far_ahead(slot) {
            log::warn!(
                "SimplexState::on_candidate: ({}/{}) REJECTED - slot too far ahead (max={})",
                window_idx,
                slot,
                self.max_acceptable_slot()
            );
            return Ok(());
        }

        // C++ consensus.cpp: if parent exists, parent_slot must be < candidate_slot
        if let Some(ref parent) = candidate.parent_id {
            if parent.slot >= slot {
                fail!(
                    "SimplexState::on_candidate: MISBEHAVIOR: parent slot {} >= candidate slot {}",
                    parent.slot,
                    slot
                );
            }
        }

        // Convert parent to CandidateParent for matching
        let parent: CandidateParent = candidate
            .parent_id
            .as_ref()
            .map(|p| CandidateParentInfo { slot: p.slot, hash: p.hash.clone() });

        // Save candidate hash -> CandidateId mapping for BlockFinalizedEvent
        self.candidate_ids.insert(candidate.id.hash.clone(), candidate.id.clone());

        log::trace!(
            "SimplexState::on_candidate: slot={}, parent={:?}, calling try_notar",
            slot,
            parent
        );

        // Alpenglow: if tryNotar(Block(s, hash, hashparent)) then
        if self.try_notar(desc, slot, &candidate.id.hash, parent.as_ref()) {
            log::trace!(
                "SimplexState::on_candidate: ({}/{}) try_notar succeeded, checking pending blocks",
                window_idx,
                slot
            );
            // Alpenglow: checkPendingBlocks()
            self.check_pending_blocks(desc);
        }
        // Alpenglow: else if Voted ∉ state[s] then
        else {
            let offset = desc.get_slot_offset_in_window(slot) as usize;

            self.ensure_window_exists(window_idx);

            // C++ consensus.cpp CandidateReceived only gates on voted_notar (line 170),
            // NOT voted_skip. A local skip vote must NOT prevent storing a candidate as
            // pending — the pending retry (`check_pending_blocks`) will notarize it once
            // the parent base propagates through skip certs.
            //
            // Alpenglow uses the stricter `is_voted` (any local vote blocks storage).
            let dominated = if let Some(window) = self.get_window(window_idx) {
                if self.opts.enable_fallback_protocol {
                    window.slots[offset].is_voted
                } else {
                    window.slots[offset].voted_notar.is_some()
                }
            } else {
                false
            };

            if !dominated {
                log::trace!(
                    "SimplexState::on_candidate: ({}/{}) try_notar=false, storing as pending block",
                    window_idx,
                    slot
                );
                // C++ parity: first pending candidate wins. If a pending block already
                // exists for this slot, reject any different candidate (equivocation).
                if let Some(window) = self.get_window_mut(window_idx) {
                    if let Some(ref existing) = window.slots[offset].pending_block {
                        if existing.id.hash != candidate.id.hash {
                            log::warn!(
                                "SimplexState::on_candidate: ({window_idx}/{slot}) \
                                pending_block already set with different hash, ignoring"
                            );
                        }
                        return Ok(());
                    }
                    window.slots[offset].pending_block = Some(candidate);
                    self.pending_slots.push(PendingSlot(slot));
                }
            } else {
                log::trace!(
                    "SimplexState::on_candidate: ({}/{}) already notarized, ignoring candidate",
                    window_idx,
                    slot
                );
            }
        }

        // Dump state at trace level (compact one-line format)
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "SimplexState::on_candidate: ({}/{}) done {}",
                window_idx,
                slot,
                self.debug_dump(desc, false)
            );
        }

        Ok(())
    }

    /// Handle incoming vote from a validator
    ///
    /// Reference: C++ SimplexPoolImpl vote handling
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description
    /// * `validator_idx` - Validator index who sent the vote
    /// * `vote` - The vote content (notarize, skip, finalize, etc.)
    /// * `signature` - Vote signature bytes (stored for certificate creation)
    ///
    /// # Returns
    ///
    /// - `VoteResult::Applied` - vote was accepted and weights updated
    /// - `VoteResult::Duplicate` - vote was already seen (same vote from same validator)
    /// - `VoteResult::Misbehavior(proof)` - vote violates protocol rules
    /// - `VoteResult::Rejected(reason)` - vote rejected for other reasons
    pub fn on_vote(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: Vote,
        signature: Vec<u8>,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        // Extract slot from vote
        let slot = match &vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
            Vote::NotarizeFallback(v) => v.slot,
            Vote::SkipFallback(v) => v.slot,
        };
        let window_idx = desc.get_window_idx(slot);

        log::trace!(
            "SimplexState::on_vote: ({}/{}/{}) from {} {}",
            desc.get_self_idx(),
            window_idx,
            slot,
            validator_idx,
            Self::format_vote(&vote)
        );

        // Validate validator index
        if validator_idx.is_out_of_bounds(self.num_validators) {
            log::trace!(
                "SimplexState::on_vote: ({}/{}) INVALID {} >= max {}, dropping",
                window_idx,
                slot,
                validator_idx,
                self.num_validators
            );
            return VoteResult::Rejected(format!(
                "invalid validator_idx {} (max={})",
                validator_idx, self.num_validators
            ));
        }

        // Reject far-future slots (DoS protection)
        if self.is_slot_too_far_ahead(slot) {
            log::warn!(
                "SimplexState::on_vote: ({}/{}) REJECTED - slot too far ahead (max={})",
                window_idx,
                slot,
                self.max_acceptable_slot()
            );
            return VoteResult::Rejected(format!(
                "slot {} too far ahead (max={})",
                slot,
                self.max_acceptable_slot()
            ));
        }

        // Process the vote with signature and raw bytes for certificate/misbehavior proof creation
        let result = match vote {
            Vote::Notarize(v) => {
                self.handle_notarize_vote(desc, validator_idx, v, signature, raw_vote)
            }
            Vote::Skip(v) => self.handle_skip_vote(desc, validator_idx, v, signature, raw_vote),
            Vote::Finalize(v) => {
                self.handle_finalize_vote(desc, validator_idx, v, signature, raw_vote)
            }
            Vote::NotarizeFallback(v) => {
                self.handle_notar_fallback_vote(validator_idx, v, raw_vote)
            }
            Vote::SkipFallback(v) => {
                self.handle_skip_fallback_vote(desc, validator_idx, v, raw_vote)
            }
        };

        // Check thresholds after successful vote processing
        // This is called once for all vote types, including notar-fallback
        if result.is_applied() && slot >= self.first_non_finalized_slot {
            self.check_thresholds_and_trigger(desc, slot);
        }

        // Dump state at trace level (compact one-line format)
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "SimplexState::on_vote: ({}/{}) done {}",
                window_idx,
                slot,
                self.debug_dump(desc, false)
            );
        }

        result
    }

    /// Cleanup finalized slots up to (but not including) `up_to_slot`
    ///
    /// Cleans up:
    /// - Old leader windows that are fully finalized
    /// - Old vote accounting for finalized slots
    /// - Old candidate_ids mapping for finalized slots
    ///
    /// The actual cleanup boundary is min(up_to_slot, first_non_finalized_slot) to ensure
    /// we never remove data for slots that haven't actually been finalized yet.
    ///
    /// This method should be called externally from SessionProcessor after
    /// finalization/skip events, not automatically from within SimplexState.
    ///
    /// # Arguments
    ///
    /// * `up_to_slot` - Requested cleanup boundary (slots < this value)
    pub fn cleanup_slots(&mut self, up_to_slot: SlotIndex) {
        // Use minimum of up_to_slot and first_non_finalized_slot
        // This ensures we never clean up slots that haven't been finalized
        let cleanup_boundary = cmp::min(up_to_slot, self.first_non_finalized_slot);

        if cleanup_boundary == SlotIndex::new(0) {
            // Nothing to clean up
            return;
        }

        // Clean up old windows (only if all slots in the window are < cleanup_boundary)
        while let Some(front) = self.leader_windows.front() {
            // Window is fully old if its last slot < cleanup_boundary
            // Window contains [start_slot, start_slot + slots_per_leader_window)
            if front.start_slot + self.slots_per_leader_window <= cleanup_boundary {
                log::trace!(
                    "SimplexState::cleanup_slots: removing w{} (s{}..s{}) boundary={}",
                    front.window_idx,
                    front.start_slot,
                    front.start_slot + self.slots_per_leader_window - 1,
                    cleanup_boundary
                );
                self.leader_windows.pop_front();
                self.leader_window_offset += 1;
            } else {
                break;
            }
        }

        // Clean up old vote accounting
        self.slot_votes.retain(|&slot_id, _| slot_id >= cleanup_boundary);

        // Clean up old candidate_ids mapping
        self.candidate_ids.retain(|_, id| id.slot >= cleanup_boundary);
    }

    /*
        ========================================================================
        Vote Handlers (from C++ SimplexPoolImpl)
        ========================================================================
    */

    /// Handle notarize vote
    ///
    /// Reference: C++ handle_vote<NotarizeVote>
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description
    /// * `validator_idx` - Validator index
    /// * `vote` - Notarize vote content
    /// * `signature` - Vote signature (stored for certificate creation)
    /// * `raw_vote` - Serialized vote bytes (stored for misbehavior proofs)
    fn handle_notarize_vote(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: NotarizeVote,
        signature: Vec<u8>,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        let slot = vote.slot;
        let window_idx = desc.get_window_idx(slot);

        // Skip finalized slots (not an error)
        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::handle_notarize_vote: {} < first_non_finalized={}, ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return VoteResult::SlotAlreadyFinalized;
        }

        // Capture before mutable borrow
        let allow_skip_after_notarize = self.opts.allow_skip_after_notarize;

        let slot_votes = self.slot_votes_at(slot);
        let Some(votes) = slot_votes.get_validator_votes_mut(validator_idx) else {
            return VoteResult::Rejected(format!(
                "validator {} out of bounds for slot {}",
                validator_idx, slot
            ));
        };

        // Check for conflicting votes
        if let Some(ref existing) = votes.notarize {
            if existing.block_hash != vote.block_hash {
                log::trace!(
                    "SimplexState::handle_notarize_vote: ({}/{}) {} CONFLICT {}:{} vs {}:{}",
                    window_idx,
                    slot,
                    validator_idx,
                    slot,
                    &existing.block_hash.to_hex_string()[..8],
                    slot,
                    &vote.block_hash.to_hex_string()[..8]
                );
                // Use stored raw bytes from existing vote and new raw bytes for proof
                let existing_raw = votes.notarize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_votes(
                    slot,
                    validator_idx,
                    ConflictingVoteType::Notarize,
                    existing.block_hash.clone(),
                    vote.block_hash.clone(),
                    existing_raw,
                    raw_vote,
                ));
            }
            // Already voted same block, ignore (not an error)
            log::trace!(
                "SimplexState::handle_notarize_vote: {}, {} duplicate vote for same block, ignoring",
                slot,
                validator_idx
            );
            return VoteResult::Duplicate;
        }

        // Check notarize/finalize hash consistency: reject if a finalize vote
        // for a different block already exists for this validator+slot.
        if let Some(ref finalize) = votes.finalize {
            if finalize.block_hash != vote.block_hash {
                log::trace!(
                    "SimplexState::handle_notarize_vote: ({}/{}) {} NOTARIZE/FINALIZE MISMATCH \
                    finalize={} notarize={}",
                    window_idx,
                    slot,
                    validator_idx,
                    &finalize.block_hash.to_hex_string()[..8],
                    &vote.block_hash.to_hex_string()[..8]
                );
                let existing_raw = votes.finalize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                    slot,
                    validator_idx,
                    VoteDescriptor::Finalize(finalize.block_hash.clone()),
                    VoteDescriptor::Notarize(vote.block_hash.clone()),
                    existing_raw,
                    raw_vote,
                    ConflictReason::NotarizeFinalizeHashMismatch,
                ));
            }
        }

        // C++ pool.cpp check_invariants() does NOT check notarize+skip conflict.
        // Only finalize+skip is misbehavior in C++.
        //
        // When allow_skip_after_notarize=true (C++ compatible mode):
        //   Notarize + Skip is ALLOWED (matches C++ behavior)
        //
        // When allow_skip_after_notarize=false (Alpenglow strict mode):
        //   Notarize + Skip is MISBEHAVIOR (in Alpenglow, once you vote notarize
        //   on the fast path, you shouldn't also vote skip)
        if !allow_skip_after_notarize && votes.skip.is_some() {
            log::trace!(
                "SimplexState::handle_notarize_vote: ({}/{}) {} has skip, rejecting notarize",
                window_idx,
                slot,
                validator_idx
            );
            // Use stored raw bytes from existing skip vote and new raw bytes for proof
            let existing_raw = votes.skip_raw.clone().unwrap_or_default();
            return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                slot,
                validator_idx,
                VoteDescriptor::Skip,
                VoteDescriptor::Notarize(vote.block_hash.clone()),
                existing_raw,
                raw_vote,
                ConflictReason::NotarizeAfterSkip,
            ));
        }

        // Record vote, signature, and raw bytes (for certificate creation and misbehavior proofs)
        let had_notarize_or_skip = votes.notarize.is_some() || votes.skip.is_some();
        votes.notarize = Some(vote.clone());
        votes.notarize_signature = Some(signature);
        votes.notarize_raw = Some(raw_vote);

        // Update weights
        let weight = desc.get_node_weight(validator_idx);
        *slot_votes.notarize_weight_by_block.entry(vote.block_hash.clone()).or_insert(0) += weight;
        if !had_notarize_or_skip {
            slot_votes.notarize_or_skip_weight += weight;
        }

        if log::log_enabled!(log::Level::Trace) {
            let total_notar =
                slot_votes.notarize_weight_by_block.get(&vote.block_hash).copied().unwrap_or(0);
            let total_weight = desc.get_total_weight();
            log::trace!(
                "SimplexState::handle_notarize_vote: ({window_idx}/{slot}) {validator_idx}+{weight} \
                -> notar={total_notar}({:.0}%) n|s={}({:.0}%) for {}:{}",
                100.0 * total_notar as f64 / total_weight as f64,
                slot_votes.notarize_or_skip_weight,
                100.0 * slot_votes.notarize_or_skip_weight as f64 / total_weight as f64,
                slot,
                &vote.block_hash.to_hex_string()[..8]
            );
        }

        VoteResult::Applied
    }

    /// Handle skip vote
    ///
    /// Reference: C++ handle_vote<SkipVote>
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description
    /// * `validator_idx` - Validator index
    /// * `vote` - Skip vote content
    /// * `signature` - Vote signature (stored for certificate creation)
    /// * `raw_vote` - Serialized vote bytes (stored for misbehavior proofs)
    fn handle_skip_vote(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: SkipVote,
        signature: Vec<u8>,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        let slot = vote.slot;
        let window_idx = desc.get_window_idx(slot);

        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::handle_skip_vote: {} < first_non_finalized={}, ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return VoteResult::SlotAlreadyFinalized;
        }

        // Capture before mutable borrow
        let allow_skip_after_notarize = self.opts.allow_skip_after_notarize;

        let slot_votes = self.slot_votes_at(slot);
        let Some(votes) = slot_votes.get_validator_votes_mut(validator_idx) else {
            return VoteResult::Rejected(format!(
                "validator {} out of bounds for slot {}",
                validator_idx, slot
            ));
        };

        // Already voted skip (not an error)
        if votes.skip.is_some() {
            log::trace!(
                "SimplexState::handle_skip_vote: {}, {} duplicate skip vote, ignoring",
                slot,
                validator_idx
            );
            return VoteResult::Duplicate;
        }

        // C++ pool.cpp check_invariants() does NOT check notarize+skip conflict.
        // Only finalize+skip is misbehavior in C++.
        //
        // When allow_skip_after_notarize=true (C++ compatible mode):
        //   Skip + Notarize is ALLOWED (matches C++ behavior)
        //
        // When allow_skip_after_notarize=false (Alpenglow strict mode):
        //   Skip + Notarize is MISBEHAVIOR (in Alpenglow, once you vote skip
        //   you shouldn't also vote notarize for the same slot)
        if !allow_skip_after_notarize && votes.notarize.is_some() {
            let existing_notar = votes.notarize.as_ref().unwrap();
            log::trace!(
                "SimplexState::handle_skip_vote: ({}/{}) {} has notarize, rejecting skip",
                window_idx,
                slot,
                validator_idx
            );
            // Use stored raw bytes from existing notarize vote and new raw bytes for proof
            let existing_raw = votes.notarize_raw.clone().unwrap_or_default();
            return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                slot,
                validator_idx,
                VoteDescriptor::Notarize(existing_notar.block_hash.clone()),
                VoteDescriptor::Skip,
                existing_raw,
                raw_vote,
                ConflictReason::NotarizeAfterSkip,
            ));
        }

        // Record vote, signature, and raw bytes (for certificate creation and misbehavior proofs)
        let had_notarize_or_skip = votes.notarize.is_some() || votes.skip.is_some();
        let had_skip_or_skip_fallback = votes.skip.is_some() || votes.fallback_skip.is_some();
        votes.skip = Some(vote);
        votes.skip_signature = Some(signature);
        votes.skip_raw = Some(raw_vote);

        // Update weights
        let weight = desc.get_node_weight(validator_idx);
        if !had_notarize_or_skip {
            slot_votes.notarize_or_skip_weight += weight;
        }
        if !had_skip_or_skip_fallback {
            slot_votes.skip_or_skip_fallback_weight += weight;
        }

        if log::log_enabled!(log::Level::Trace) {
            let total_weight = desc.get_total_weight();
            log::trace!(
                "SimplexState::handle_skip_vote: ({}/{}) {} +{} -> n|s={}({:.0}%) s|fb={}({:.0}%)",
                window_idx,
                slot,
                validator_idx,
                weight,
                slot_votes.notarize_or_skip_weight,
                100.0 * slot_votes.notarize_or_skip_weight as f64 / total_weight as f64,
                slot_votes.skip_or_skip_fallback_weight,
                100.0 * slot_votes.skip_or_skip_fallback_weight as f64 / total_weight as f64
            );
        }

        VoteResult::Applied
    }

    /// Handle finalize vote
    ///
    /// Reference: C++ handle_vote<TrueFinalizeVote>
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description
    /// * `validator_idx` - Validator index
    /// * `vote` - Finalize vote content
    /// * `signature` - Vote signature (stored for certificate creation)
    /// * `raw_vote` - Serialized vote bytes (stored for misbehavior proofs)
    fn handle_finalize_vote(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: FinalizeVote,
        signature: Vec<u8>,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        let slot = vote.slot;
        let window_idx = desc.get_window_idx(slot);

        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::handle_finalize_vote: {} < first_non_finalized={}, ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return VoteResult::SlotAlreadyFinalized;
        }

        let slot_votes = self.slot_votes_at(slot);
        let Some(votes) = slot_votes.get_validator_votes_mut(validator_idx) else {
            return VoteResult::Rejected(format!(
                "validator {} out of bounds for slot {}",
                validator_idx, slot
            ));
        };

        // Check for conflicting finalize votes
        if let Some(ref existing) = votes.finalize {
            if existing.block_hash != vote.block_hash {
                log::trace!(
                    "SimplexState::handle_finalize_vote: ({}/{}) {} CONFLICT {}:{} vs {}:{}",
                    window_idx,
                    slot,
                    validator_idx,
                    slot,
                    &existing.block_hash.to_hex_string()[..8],
                    slot,
                    &vote.block_hash.to_hex_string()[..8]
                );
                // Use stored raw bytes from existing vote and new raw bytes for proof
                let existing_raw = votes.finalize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_votes(
                    slot,
                    validator_idx,
                    ConflictingVoteType::Finalize,
                    existing.block_hash.clone(),
                    vote.block_hash.clone(),
                    existing_raw,
                    raw_vote,
                ));
            }
            // Duplicate vote for same block (not an error)
            log::trace!(
                "SimplexState::handle_finalize_vote: {}, {} duplicate vote for same block, ignoring",
                slot,
                validator_idx
            );
            return VoteResult::Duplicate;
        }

        // Check notarize/finalize hash consistency: a validator must not
        // notarize hash A and then finalize hash B for the same slot.
        if let Some(ref notarize) = votes.notarize {
            if notarize.block_hash != vote.block_hash {
                log::trace!(
                    "SimplexState::handle_finalize_vote: ({}/{}) {} NOTARIZE/FINALIZE MISMATCH \
                    notarize={} finalize={}",
                    window_idx,
                    slot,
                    validator_idx,
                    &notarize.block_hash.to_hex_string()[..8],
                    &vote.block_hash.to_hex_string()[..8]
                );
                let existing_raw = votes.notarize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                    slot,
                    validator_idx,
                    VoteDescriptor::Notarize(notarize.block_hash.clone()),
                    VoteDescriptor::Finalize(vote.block_hash.clone()),
                    existing_raw,
                    raw_vote,
                    ConflictReason::NotarizeFinalizeHashMismatch,
                ));
            }
        }

        // Check conflicts with skip
        if votes.skip.is_some() {
            log::trace!(
                "SimplexState::handle_finalize_vote: ({}/{}) {} has skip, rejecting finalize",
                window_idx,
                slot,
                validator_idx
            );
            // Use stored raw bytes from existing skip vote and new raw bytes for proof
            let existing_raw = votes.skip_raw.clone().unwrap_or_default();
            return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                slot,
                validator_idx,
                VoteDescriptor::Skip,
                VoteDescriptor::Finalize(vote.block_hash.clone()),
                existing_raw,
                raw_vote,
                ConflictReason::FinalizeAfterSkip,
            ));
        }

        // Check conflicts with fallback votes
        if let Some((first_fb_hash, first_fb_raw)) = votes.fallback_notarize.iter().next() {
            log::trace!(
                "SimplexState::handle_finalize_vote: ({}/{}) {} has notar-fb, rejecting finalize",
                window_idx,
                slot,
                validator_idx
            );
            return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                slot,
                validator_idx,
                VoteDescriptor::NotarizeFallback(first_fb_hash.clone()),
                VoteDescriptor::Finalize(vote.block_hash.clone()),
                first_fb_raw.clone(),
                raw_vote,
                ConflictReason::FinalizeAfterNotarFallback,
            ));
        }

        if votes.fallback_skip.is_some() {
            log::trace!(
                "SimplexState::handle_finalize_vote: ({}/{}) {} has skip-fb, rejecting finalize",
                window_idx,
                slot,
                validator_idx
            );
            // Use stored raw bytes from existing skip-fallback vote and new raw bytes for proof
            let existing_raw = votes.fallback_skip_raw.clone().unwrap_or_default();
            return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                slot,
                validator_idx,
                VoteDescriptor::SkipFallback,
                VoteDescriptor::Finalize(vote.block_hash.clone()),
                existing_raw,
                raw_vote,
                ConflictReason::FinalizeAfterSkipFallback,
            ));
        }

        // Record vote, signature, and raw bytes (for certificate creation and misbehavior proofs)
        votes.finalize = Some(vote.clone());
        votes.finalize_signature = Some(signature);
        votes.finalize_raw = Some(raw_vote);

        // Update weights
        let weight = desc.get_node_weight(validator_idx);
        *slot_votes.finalize_weight_by_block.entry(vote.block_hash.clone()).or_insert(0) += weight;

        if log::log_enabled!(log::Level::Trace) {
            let total_finalize =
                slot_votes.finalize_weight_by_block.get(&vote.block_hash).copied().unwrap_or(0);
            let total_weight = desc.get_total_weight();
            log::trace!(
                "SimplexState::handle_finalize_vote: ({}/{}) {} +{} -> final={}({:.0}%) for {}:{}",
                window_idx,
                slot,
                validator_idx,
                weight,
                total_finalize,
                100.0 * total_finalize as f64 / total_weight as f64,
                slot,
                &vote.block_hash.to_hex_string()[..8]
            );
        }

        VoteResult::Applied
    }

    /// Handle notar-fallback vote
    ///
    /// Reference: C++ handle_vote<NotarizeFallbackVote>
    ///
    /// # Arguments
    ///
    /// * `validator_idx` - Validator index
    /// * `vote` - Notar-fallback vote content
    /// * `raw_vote` - Serialized vote bytes (stored for misbehavior proofs)
    fn handle_notar_fallback_vote(
        &mut self,
        validator_idx: ValidatorIndex,
        vote: NotarizeFallbackVote,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        let slot = vote.slot;
        let window_idx = slot.window_index(self.slots_per_leader_window);

        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::handle_notar_fallback_vote: {} < first_non_finalized={}, ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return VoteResult::SlotAlreadyFinalized;
        }

        // First pass: check conditions
        {
            let slot_votes = self.slot_votes_at(slot);
            let Some(votes) = slot_votes.get_validator_votes(validator_idx) else {
                return VoteResult::Rejected(format!(
                    "validator {} out of bounds for slot {}",
                    validator_idx, slot
                ));
            };

            if votes.fallback_notarize.len() >= MAX_NOTAR_FALLBACK_VOTES_PER_VALIDATOR {
                log::trace!(
                    "SimplexState::handle_notar_fallback_vote: ({}/{}) {} too many notar-fb ({})",
                    window_idx,
                    slot,
                    validator_idx,
                    votes.fallback_notarize.len()
                );
                // Note: Exceeding max votes is rejected but not classic misbehavior
                return VoteResult::Rejected(format!(
                    "validator {} exceeded max notar-fallback votes ({}) for {}",
                    validator_idx, MAX_NOTAR_FALLBACK_VOTES_PER_VALIDATOR, slot
                ));
            }

            if let Some(ref finalize) = votes.finalize {
                log::trace!(
                    "SimplexState::handle_notar_fallback_vote: ({}/{}) {} has finalize, rejecting notar-fb",
                    window_idx,
                    slot,
                    validator_idx
                );
                // Use stored raw bytes from existing finalize vote and new raw bytes for proof
                let existing_raw = votes.finalize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                    slot,
                    validator_idx,
                    VoteDescriptor::Finalize(finalize.block_hash.clone()),
                    VoteDescriptor::NotarizeFallback(vote.block_hash.clone()),
                    existing_raw,
                    raw_vote,
                    ConflictReason::NotarFallbackAfterFinalize,
                ));
            }
        }

        // Second pass: insert vote
        let slot_votes = self.slot_votes_at(slot);
        let Some(votes) = slot_votes.get_validator_votes_mut(validator_idx) else {
            return VoteResult::Rejected(format!(
                "validator {} out of bounds for slot {}",
                validator_idx, slot
            ));
        };

        // Check if already voted for this block hash
        if votes.fallback_notarize.contains_key(&vote.block_hash) {
            log::trace!(
                "SimplexState::handle_notar_fallback_vote: {}, {}, duplicate hash={}, ignoring",
                slot,
                validator_idx,
                vote.block_hash.to_hex_string()
            );
            return VoteResult::Duplicate;
        }

        // Insert vote with raw bytes
        votes.fallback_notarize.insert(vote.block_hash.clone(), raw_vote);
        log::trace!(
            "SimplexState::handle_notar_fallback_vote: {}, {}, hash={}, fallback_count={}",
            slot,
            validator_idx,
            vote.block_hash.to_hex_string(),
            votes.fallback_notarize.len()
        );
        VoteResult::Applied
    }

    /// Handle skip-fallback vote
    ///
    /// Reference: C++ handle_vote<SkipFallbackVote>
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description
    /// * `validator_idx` - Validator index
    /// * `vote` - Skip-fallback vote content
    /// * `raw_vote` - Serialized vote bytes (stored for misbehavior proofs)
    fn handle_skip_fallback_vote(
        &mut self,
        desc: &SessionDescription,
        validator_idx: ValidatorIndex,
        vote: SkipFallbackVote,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        let slot = vote.slot;
        let window_idx = desc.get_window_idx(slot);

        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::handle_skip_fallback_vote: {} < first_non_finalized={}, ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return VoteResult::SlotAlreadyFinalized;
        }

        // First pass: check conditions
        let weight = desc.get_node_weight(validator_idx);
        {
            let slot_votes = self.slot_votes_at(slot);
            let Some(votes) = slot_votes.get_validator_votes(validator_idx) else {
                return VoteResult::Rejected(format!(
                    "validator {} out of bounds for slot {}",
                    validator_idx, slot
                ));
            };

            // Already voted (not an error)
            if votes.fallback_skip.is_some() {
                log::trace!(
                    "SimplexState::handle_skip_fallback_vote: {}, {} duplicate skip-fallback, ignoring",
                    slot,
                    validator_idx
                );
                return VoteResult::Duplicate;
            }

            if let Some(ref finalize) = votes.finalize {
                log::trace!(
                    "SimplexState::handle_skip_fallback_vote: ({}/{}) {} has finalize, rejecting skip-fb",
                    window_idx,
                    slot,
                    validator_idx
                );
                // Use stored raw bytes from existing finalize vote and new raw bytes for proof
                let existing_raw = votes.finalize_raw.clone().unwrap_or_default();
                return VoteResult::Misbehavior(MisbehaviorProof::conflicting_types(
                    slot,
                    validator_idx,
                    VoteDescriptor::Finalize(finalize.block_hash.clone()),
                    VoteDescriptor::SkipFallback,
                    existing_raw,
                    raw_vote,
                    ConflictReason::SkipFallbackAfterFinalize,
                ));
            }
        }

        // Second pass: update state
        let slot_votes = self.slot_votes_at(slot);
        let Some(votes) = slot_votes.get_validator_votes_mut(validator_idx) else {
            return VoteResult::Rejected(format!(
                "validator {} out of bounds for slot {}",
                validator_idx, slot
            ));
        };

        let had_skip_or_skip_fallback = votes.skip.is_some() || votes.fallback_skip.is_some();
        votes.fallback_skip = Some(vote);
        votes.fallback_skip_raw = Some(raw_vote);

        // Update weights
        if !had_skip_or_skip_fallback {
            slot_votes.skip_or_skip_fallback_weight += weight;
        }

        if log::log_enabled!(log::Level::Trace) {
            let total_weight = desc.get_total_weight();
            log::trace!(
                "SimplexState::handle_skip_fallback_vote: ({}/{}) {} +{} -> s|fb={}({:.0}%)",
                window_idx,
                slot,
                validator_idx,
                weight,
                slot_votes.skip_or_skip_fallback_weight,
                100.0 * slot_votes.skip_or_skip_fallback_weight as f64 / total_weight as f64
            );
        }

        VoteResult::Applied
    }

    /// Check thresholds and trigger internal FSM events
    ///
    /// Reference: C++ check_and_publish_events
    fn check_thresholds_and_trigger(&mut self, desc: &SessionDescription, slot_id: SlotIndex) {
        // Check invariants at the start of threshold processing
        self.check_invariants(desc);

        let threshold_66 = desc.get_threshold_66();
        let threshold_33 = desc.get_threshold_33();

        // Clone data we need to avoid borrow issues
        let (notarize_weights, finalize_weights, flags) = {
            let Some(sv) = self.slot_votes.get(&slot_id) else {
                return;
            };
            (
                sv.notarize_weight_by_block.clone(),
                sv.finalize_weight_by_block.clone(),
                (
                    sv.block_notarized_published,
                    sv.safe_to_skip_published,
                    sv.block_finalized_published,
                    sv.slot_skipped_published,
                    sv.notarize_or_skip_weight,
                    sv.skip_or_skip_fallback_weight,
                    sv.safe_to_notar_blocks.clone(),
                ),
            )
        };

        let (
            block_notarized_published,
            safe_to_skip_published,
            block_finalized_published,
            slot_skipped_published,
            notarize_or_skip_weight,
            skip_or_skip_fallback_weight,
            safe_to_notar_blocks,
        ) = flags;

        let window_idx = desc.get_window_idx(slot_id);
        let total_weight = desc.get_total_weight();

        // Check notarize thresholds
        for (block, weight) in &notarize_weights {
            // BlockNotarized: notar(b) >= 2/3
            if !block_notarized_published && *weight >= threshold_66 {
                log::trace!(
                    "SimplexState::check_thresholds: ({}/{}) NOTARIZED {}:{} at {}({:.0}%)",
                    window_idx,
                    slot_id,
                    slot_id,
                    &block.to_hex_string()[..8],
                    weight,
                    100.0 * *weight as f64 / total_weight as f64
                );

                if let Some(sv) = self.slot_votes.get_mut(&slot_id) {
                    sv.block_notarized_published = true;

                    // Cache notarization certificate for candidate resolver
                    // Reference: C++ CandidateResolver caches NotarCertRef when NotarizationObserved
                    let cert = Arc::new(sv.create_notarize_cert(slot_id, block));
                    match sv.store_notarize_certificate(block, cert.clone()) {
                        Ok(true) => {
                            log::trace!(
                                "SimplexState::check_thresholds: ({}/{}) cached notarization cert for {}:{}",
                                window_idx,
                                slot_id,
                                slot_id,
                                &block.to_hex_string()[..8]
                            );
                            // Emit event for session processor to cache serialized cert in receiver
                            self.push_event_back(SimplexEvent::NotarizationReached(
                                NotarizationReachedEvent {
                                    slot: slot_id,
                                    block_hash: block.clone(),
                                    certificate: cert,
                                },
                            ));
                        }
                        Ok(false) => {
                            // Already stored for same block - idempotent
                        }
                        Err(e) => {
                            // Invariant violation: only one block can reach notarization threshold per slot
                            panic!(
                                "SimplexState invariant violation: notarize cert conflict at ({}/{}) - {}",
                                window_idx, slot_id, e
                            );
                        }
                    }
                }
                self.on_block_notarized(desc, slot_id, block.clone());
            }

            // SafeToNotar: skip(s) + notar(b) >= 2/3 AND notar(b) >= 1/3
            // Reference: Alpenglow White Paper Section 2.5
            // "SafeToNotar(s, hash(b)): Moreover: skip(s) + notar(b) ≥ 2/3 and notar(b) ≥ 1/3"
            //
            // Only relevant when notar alone isn't enough for BlockNotarized.
            // If notar(b) >= 2/3, BlockNotarized triggers via normal path - no fallback needed.
            //
            // SKIP when enable_fallback_protocol = false (C++ compatible mode)
            if self.opts.enable_fallback_protocol {
                let skip_plus_notar_b = skip_or_skip_fallback_weight + *weight;
                if !safe_to_notar_blocks.contains(block)
                    && *weight < threshold_66  // notar alone isn't enough for normal path
                    && *weight >= threshold_33
                    && skip_plus_notar_b >= threshold_66
                {
                    log::trace!(
                        "SimplexState::check_thresholds: ({}/{}) SAFE_TO_NOTAR {}:{} notar={}({:.0}%) skip+notar={}({:.0}%)",
                        window_idx,
                        slot_id,
                        slot_id,
                        &block.to_hex_string()[..8],
                        weight,
                        100.0 * *weight as f64 / total_weight as f64,
                        skip_plus_notar_b,
                        100.0 * skip_plus_notar_b as f64 / total_weight as f64
                    );

                    if let Some(sv) = self.slot_votes.get_mut(&slot_id) {
                        sv.safe_to_notar_blocks.insert(block.clone());
                    }
                    self.on_safe_to_notar(slot_id, block.clone());
                }
            }
        }

        // SafeToSkip: skip(s) + sum(notar(b)) - max(notar(b)) >= 1/3
        // Reference: Alpenglow White Paper Section 2.5
        // "SafeToSkip(s): Moreover: skip(s) + sum(notar(b)) − max_b(notar(b)) >= 1/3"
        //
        // notarize_or_skip_weight = skip + sum(notar) because each validator votes skip OR notar
        // So the condition: skip + sum(notar) - max(notar) >= 1/3
        // Becomes: notarize_or_skip_weight - max(notar) >= threshold_33
        // Or: notarize_or_skip_weight >= threshold_33 + max(notar)
        //
        // Only relevant when skip alone isn't enough for SlotSkipped.
        // If skip >= 2/3, SlotSkipped triggers via normal path - no fallback needed.
        //
        // SKIP when enable_fallback_protocol = false (C++ compatible mode)
        if self.opts.enable_fallback_protocol && !safe_to_skip_published {
            let max_notarize = notarize_weights.values().max().copied().unwrap_or(0);
            if skip_or_skip_fallback_weight < threshold_66  // skip alone isn't enough for normal path
                && notarize_or_skip_weight >= threshold_33 + max_notarize
            {
                log::trace!(
                    "SimplexState::check_thresholds: ({}/{}) SAFE_TO_SKIP n|s={}({:.0}%) max_notar={}",
                    window_idx,
                    slot_id,
                    notarize_or_skip_weight,
                    100.0 * notarize_or_skip_weight as f64 / total_weight as f64,
                    max_notarize
                );

                if let Some(sv) = self.slot_votes.get_mut(&slot_id) {
                    sv.safe_to_skip_published = true;
                }
                self.on_safe_to_skip(slot_id);
            }
        }

        // BlockFinalized: finalize(b) >= 2/3
        // Emit BlockFinalized when threshold is reached, regardless of slot order.
        // C++ doesn't gate on sequential order - events are emitted as thresholds are reached.
        // SessionProcessor handles batch finalization to ensure commit order.
        if !block_finalized_published {
            for (block, weight) in &finalize_weights {
                if *weight >= threshold_66 {
                    log::trace!(
                        "SimplexState::check_thresholds: ({}/{}) FINALIZED {}:{} at {}({:.0}%)",
                        window_idx,
                        slot_id,
                        slot_id,
                        &block.to_hex_string()[..8],
                        weight,
                        100.0 * *weight as f64 / total_weight as f64
                    );

                    // Create finalization certificate
                    // Reference: C++ pool.cpp SlotState::create_cert<FinalizeVote>(vote)
                    // Invariant: SlotVotes must exist if we reached finalization threshold
                    let Some(sv) = self.slot_votes.get_mut(&slot_id) else {
                        log::error!(
                            "SimplexState invariant violated: SlotVotes must exist for finalized slot={}",
                            slot_id
                        );
                        return;
                    };
                    sv.block_finalized_published = true;
                    let certificate = Arc::new(sv.create_finalize_cert(slot_id, block));

                    // Cache finalization certificate for standstill replay
                    // Reference: C++ pool.cpp SlotState::certs.finalize_ - only one per slot
                    let store_result = sv.store_finalize_certificate(block, certificate.clone());
                    let is_new_cert = matches!(store_result, Ok(true));
                    if let Err(e) = store_result {
                        // Invariant violation: only one block can reach finalization threshold per slot
                        panic!(
                            "SimplexState invariant violation: finalize cert conflict at ({}/{}) - {}",
                            window_idx, slot_id, e
                        );
                    } else if is_new_cert {
                        log::trace!(
                            "SimplexState::check_thresholds: ({}/{}) cached finalization cert for {}:{}",
                            window_idx,
                            slot_id,
                            slot_id,
                            &block.to_hex_string()[..8]
                        );
                    }

                    // Look up full CandidateId from our mapping (if available)
                    // The candidate hash was saved in on_candidate()
                    // If not found, block_id will be None (can happen in tests or edge cases)
                    let block_id = self.candidate_ids.get(block).map(|c| c.block.clone());

                    // Emit BlockFinalized first (for commit logic)
                    self.push_event_back(SimplexEvent::BlockFinalized(BlockFinalizedEvent {
                        slot: slot_id,
                        block_hash: block.clone(),
                        block_id,
                        certificate: certificate.clone(),
                    }));

                    // Then emit FinalizationReached (for broadcast + standstill caching)
                    // Reference: C++ pool.cpp handle_our_certificate → OutgoingProtocolMessage
                    if is_new_cert {
                        self.push_event_back(SimplexEvent::FinalizationReached(
                            FinalizationReachedEvent {
                                slot: slot_id,
                                block_hash: block.clone(),
                                certificate,
                            },
                        ));
                    }

                    // Update first_non_finalized_slot for cleanup tracking
                    // Note: This is only for cleanup purposes - events are emitted as thresholds are reached
                    // Reference: C++ pool.cpp on_finalization() updates first_nonfinalized_slot_
                    if slot_id >= self.first_non_finalized_slot {
                        log::trace!(
                            "SimplexState::check_thresholds: ({}/{}) advancing first_non_finalized to {}",
                            window_idx,
                            slot_id,
                            slot_id + 1
                        );
                        self.first_non_finalized_slot = slot_id + 1;

                        // Keep progress cursor consistent with finalized boundary
                        if self.first_non_finalized_slot > self.first_non_progressed_slot {
                            self.first_non_progressed_slot = self.first_non_finalized_slot;

                            log::trace!(
                                "SimplexState::check_thresholds: ({}/{}) advanced first_non_progressed_slot to {} (finalized boundary)",
                                window_idx,
                                slot_id,
                                self.first_non_progressed_slot
                            );
                        }
                    }

                    // Update notarized-parent chain tracking (always maintained).
                    // Finalization implies notarization; if we missed `BlockNotarized`,
                    // record it now to keep state consistent.
                    let parent_info = CandidateParentInfo { slot: slot_id, hash: block.clone() };
                    let missing_notar = self
                        .get_slot_ref(desc, slot_id)
                        .map(|s| s.observed_notar_certificate.is_none())
                        .unwrap_or(true);
                    if missing_notar {
                        log::trace!(
                            "SimplexState::check_thresholds: ({}/{}) finalized without prior notarization, recording cert",
                            window_idx,
                            slot_id
                        );
                        if let Some(s) = self.get_slot_mut(desc, slot_id) {
                            s.observed_notar_certificate = Some(parent_info.clone());
                        }
                        self.propagate_base_after_notarization(desc, parent_info.clone());
                    }

                    // Choose window advancement strategy based on mode
                    log::trace!(
                        "SimplexState::check_thresholds: ({}/{}) window advancement strategy: \
                        use_notarized_parent_chain={} current_window={} first_non_progressed_slot={}",
                        window_idx,
                        slot_id,
                        self.opts.use_notarized_parent_chain,
                        self.current_leader_window_idx,
                        self.first_non_progressed_slot
                    );
                    if self.opts.use_notarized_parent_chain {
                        // Behavioral mode: advance leader window based on progress cursor (not finalization).
                        // Reference: C++ pool.cpp maybe_publish_new_leader_windows()
                        self.advance_leader_window_on_progress_cursor(desc);
                    } else {
                        // Trigger ParentReady for the next window
                        // When a block is finalized, it becomes a valid parent for the next window's first slot
                        // Reference: Alpenglow Algorithm 1 "upon ParentReady(window, hash(b))"
                        //
                        // Note: C++ reference has this in pool.cpp comment but NOT implemented.
                        // We implement it here: finalized block in window W becomes parent for window W+1.
                        //
                        // No recursion risk: on_window_base_ready -> check_pending_blocks -> try_notar
                        // only broadcasts votes, doesn't call check_thresholds_and_trigger.
                        let next_window_idx =
                            slot_id.window_index(self.slots_per_leader_window) + 1;

                        log::trace!(
                            "SimplexState::check_thresholds: ({}/{}) triggering ParentReady for {} parent={}:{}",
                            window_idx,
                            slot_id,
                            next_window_idx,
                            slot_id,
                            &block.to_hex_string()[..8]
                        );

                        // Call on_window_base_ready to handle all the logic:
                        // - Add to available_bases
                        // - Check pending blocks
                        // - Update timeouts with adaptive backoff
                        // Note: This cannot fail because:
                        // - next_window_idx is small (no overflow)
                        // - parent slot < next window start slot (by construction)
                        if let Err(e) =
                            self.on_window_base_ready(desc, next_window_idx, Some(parent_info))
                        {
                            log::error!(
                                "SimplexState::check_thresholds: ({}/{}) ParentReady failed: {}",
                                window_idx,
                                slot_id,
                                e
                            );
                        }
                    }

                    break;
                }
            }
        }

        // SlotSkipped: skip_or_skip_fallback >= 2/3 (skip certificate)
        // This means finalization is no longer possible for this slot.
        // We only emit this if we haven't already finalized the slot.
        // C++ doesn't gate on sequential order - events are emitted as thresholds are reached.
        // SessionProcessor handles skip tracking internally (no external callback in C++).
        //
        // Note: Don't emit for slots already in the past (slot_id < first_non_finalized_slot)
        // This happens when a later slot is finalized before an earlier one.
        let can_emit_skip = slot_id >= self.first_non_finalized_slot;
        if !slot_skipped_published
            && !block_finalized_published
            && can_emit_skip
            && skip_or_skip_fallback_weight >= threshold_66
        {
            log::trace!(
                "SimplexState::check_thresholds: ({}/{}) SKIPPED s|fb={}({:.0}%)",
                window_idx,
                slot_id,
                skip_or_skip_fallback_weight,
                100.0 * skip_or_skip_fallback_weight as f64 / total_weight as f64
            );

            // Create and cache skip certificate, emit event (C++ mode only for broadcast)
            let skip_cert = if let Some(sv) = self.slot_votes.get_mut(&slot_id) {
                sv.slot_skipped_published = true;

                // Create skip certificate if not already cached
                let cert = Arc::new(sv.create_skip_cert(slot_id));
                match sv.store_skip_certificate(cert.clone()) {
                    Ok(true) => {
                        log::trace!(
                            "SimplexState::check_thresholds: ({}/{}) created skip cert with {} sigs",
                            window_idx,
                            slot_id,
                            cert.signatures.len()
                        );
                        Some(cert)
                    }
                    Ok(false) => {
                        // Already stored - use existing
                        sv.skip_certificate.clone()
                    }
                    Err(e) => {
                        // Invariant violation: skip certs don't have block hash conflicts
                        panic!(
                            "SimplexState invariant violation: skip cert error at ({}/{}) - {}",
                            window_idx, slot_id, e
                        );
                    }
                }
            } else {
                None
            };

            self.push_event_back(SimplexEvent::SlotSkipped(SlotSkippedEvent { slot: slot_id }));

            // Emit SkipCertificateReached event for broadcasting (C++ mode only)
            // Alpenglow paper doesn't require explicit skip certificate broadcast
            if !self.opts.enable_fallback_protocol {
                if let Some(cert) = skip_cert {
                    self.push_event_back(SimplexEvent::SkipCertificateReached(
                        SkipCertificateReachedEvent { slot: slot_id, certificate: cert },
                    ));
                }
            }

            // Update notarized-parent chain tracking (C++ pool.cpp parity, always maintained):
            // - mark slot skipped (skip certificate)
            // - propagate `available_base` forward
            // - advance progress cursor (`first_non_progressed_slot`)
            // Reference: C++ pool.cpp on_skip() → slot.skipped=true, propagate base if needed
            self.propagate_base_after_skip_cert(desc, slot_id);

            // When notarized-parent chain mode is enabled, trigger leader window advancement
            // based on progress cursor instead of waiting for finalization.
            // Otherwise, use the legacy per-window propagation approach.
            log::trace!(
                "SimplexState::check_thresholds: ({}/{}) window advancement after skip: \
                use_notarized_parent_chain={} current_window={} first_non_progressed_slot={}",
                window_idx,
                slot_id,
                self.opts.use_notarized_parent_chain,
                self.current_leader_window_idx,
                self.first_non_progressed_slot
            );
            // C++ parity: skip certificates do NOT advance first_non_finalized_slot.
            // Only finalization advances it (see C++ state.h notify_finalized()).
            // However, the progress cursor (first_non_progressed_slot, C++ `now_`)
            // DOES advance on skip -- it tracks notarized-or-skipped progress.
            // Only advance sequentially to avoid jumping past unresolved earlier slots.
            if slot_id == self.first_non_progressed_slot {
                self.first_non_progressed_slot = slot_id + 1;
                log::trace!(
                    "SimplexState::check_thresholds: ({window_idx}/{slot_id}) \
                    advanced first_non_progressed_slot to {} (skip)",
                    self.first_non_progressed_slot
                );
            }

            if self.opts.use_notarized_parent_chain {
                self.advance_leader_window_on_progress_cursor(desc);
            } else {
                // Check if this is the last slot in the window BEFORE cleanup
                // If so, and if no block was finalized in this window, we need to
                // propagate the available bases to the next window (including genesis/None)
                // This handles the startup case where an entire window is skipped.
                let current_window_idx = slot_id.window_index(self.slots_per_leader_window);
                let slot_offset_in_window = slot_id.offset_in_window(self.slots_per_leader_window);
                let is_last_slot_in_window =
                    slot_offset_in_window == self.slots_per_leader_window - 1;

                // Capture bases BEFORE cleanup (window may be removed by cleanup)
                let bases_to_propagate: Option<Vec<CandidateParent>> = if is_last_slot_in_window {
                    let next_window_idx = current_window_idx + 1;

                    // Check if next window already has available bases (from finalization)
                    let next_window_has_bases = self
                        .get_window(next_window_idx)
                        .map(|w| !w.available_bases.is_empty())
                        .unwrap_or(false);

                    if !next_window_has_bases {
                        // Capture current window's available bases before cleanup
                        self.get_window(current_window_idx)
                            .map(|w| w.available_bases.iter().cloned().collect())
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Propagate bases to next window after cleanup
                if let Some(bases) = bases_to_propagate {
                    if !bases.is_empty() {
                        let next_window_idx = current_window_idx + 1;

                        log::trace!(
                            "SimplexState: Last slot {} of window {} skipped without finalization, \
                                propagating {} available base(s) to window {}",
                            slot_id,
                            current_window_idx,
                            bases.len(),
                            next_window_idx
                        );

                        for parent in bases {
                            // Use on_window_base_ready to handle the logic consistently
                            // Note: No recursion risk (same as BlockFinalized case)
                            if let Err(e) =
                                self.on_window_base_ready(desc, next_window_idx, parent.clone())
                            {
                                log::error!(
                                    "SimplexState: SlotSkipped failed to propagate parent {:?} to window {}: {}",
                                    parent,
                                    next_window_idx,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /*
        ========================================================================
        Internal FSM Events (triggered by thresholds)
        ========================================================================
    */

    /// upon BlockNotarized(s, hash(b)) do
    ///
    /// Reference: Alpenglow Algorithm 1
    ///
    /// ```text
    /// state[s] ← state[s] ∪ {BlockNotarized(hash(b))}
    /// tryFinal(s, hash(b))
    /// ```
    fn on_block_notarized(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
        block_hash: UInt256,
    ) {
        log::trace!(
            "SimplexState::on_block_notarized: slot={}, block_hash={}",
            slot,
            block_hash.to_hex_string()
        );

        if slot < self.first_non_finalized_slot {
            return;
        }

        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;

        log::trace!(
            "SimplexState::on_block_notarized: ({}/{}) storing cert for {}:{}, calling try_final",
            window_idx,
            slot,
            slot,
            &block_hash.to_hex_string()[..8]
        );

        self.ensure_window_exists(window_idx);

        // Alpenglow: state[s] ← state[s] ∪ {BlockNotarized(hash(b))}
        if let Some(window) = self.get_window_mut(window_idx) {
            window.slots[offset].observed_notar_certificate =
                Some(CandidateParentInfo { slot, hash: block_hash.clone() });
        }

        // Update notarized-parent chain tracking (C++ pool.cpp parity, always maintained):
        // - propagate `available_base` to next non-skipped slot
        // - advance progress cursor (`first_non_progressed_slot`)
        // Reference: C++ pool.cpp on_notarization() → next_nonskipped_slot_after(...).available_base = id
        let parent_info = CandidateParentInfo { slot, hash: block_hash.clone() };
        self.propagate_base_after_notarization(desc, parent_info.clone());

        // When notarized-parent chain mode is enabled, trigger leader window advancement
        // based on progress cursor instead of waiting for finalization.
        // Reference: C++ pool.cpp maybe_publish_new_leader_windows()
        log::trace!(
            "SimplexState::on_block_notarized: ({}/{}) window advancement check: \
            use_notarized_parent_chain={} first_non_progressed_slot={}",
            window_idx,
            slot,
            self.opts.use_notarized_parent_chain,
            self.first_non_progressed_slot
        );
        if self.opts.use_notarized_parent_chain {
            self.advance_leader_window_on_progress_cursor(desc);
        }

        // C++ compatibility: advance skip timer when NotarCert arrives
        // Reference: C++ consensus.cpp lines 228-243 (NotarizationObserved handler)
        // When a NotarCert is observed, C++ advances timeout_slot_ to slot+1 and
        // reschedules the alarm to now + target_rate. This prevents the skip cascade
        // from racing ahead of active block production.
        //
        // Important: do NOT shrink skip_timestamp below the current scheduled value.
        // During the first_block_timeout window, the skip timer is intentionally set
        // far in the future to give all nodes time to join the overlay. Setting it to
        // now + target_rate here would bypass that protection entirely.
        if !self.opts.enable_fallback_protocol {
            let next_slot = slot + 1;
            if self.skip_slot <= next_slot {
                let new_timestamp = desc.get_time() + self.target_rate_timeout;
                // Only update skip_timestamp if it would be later than current,
                // preserving the first_block_timeout window.
                let effective_timestamp = match self.skip_timestamp {
                    Some(current) if current > new_timestamp => current,
                    _ => new_timestamp,
                };
                log::debug!(
                    "SimplexState::on_block_notarized: advancing skip timer: \
                    skip_slot {} -> {next_slot}, new timeout in {:?}{}",
                    self.skip_slot,
                    self.target_rate_timeout,
                    if effective_timestamp != new_timestamp {
                        " (preserved first_block_timeout)"
                    } else {
                        ""
                    }
                );
                self.skip_slot = next_slot;
                self.skip_timestamp = Some(effective_timestamp);
            }
        }

        // Alpenglow: tryFinal(s, hash(b))
        self.try_final(desc, slot, &block_hash);
    }

    /// upon SafeToNotar(s, hash(b)) do
    ///
    /// Reference: Alpenglow Algorithm 1
    ///
    /// ```text
    /// trySkipWindow(s)
    /// if ItsOver ∉ state[s] then
    ///     broadcast NotarFallbackVote(s, hash(b))
    ///     state[s] ← state[s] ∪ {BadWindow}
    /// ```
    fn on_safe_to_notar(&mut self, slot: SlotIndex, block_hash: UInt256) {
        log::trace!(
            "SimplexState::on_safe_to_notar: slot={}, block_hash={}",
            slot,
            block_hash.to_hex_string()
        );

        if slot < self.first_non_finalized_slot {
            return;
        }

        let window_idx = slot.window_index(self.slots_per_leader_window);
        let offset = slot.offset_in_window(self.slots_per_leader_window) as usize;

        // Alpenglow: trySkipWindow(s)
        self.try_skip_window(window_idx);

        // Alpenglow: if ItsOver ∉ state[s] then
        self.ensure_window_exists(window_idx);

        // Check if we should broadcast (without holding mutable borrow)
        let should_broadcast =
            self.get_window(window_idx).map(|w| !w.slots[offset].its_over).unwrap_or(false);

        if should_broadcast {
            // Alpenglow: broadcast NotarFallbackVote(s, hash(b))
            log::trace!(
                "SimplexState::on_safe_to_notar: ({}/{}) broadcasting notar-fb for {}:{}, marking BadWindow",
                window_idx,
                slot,
                slot,
                &block_hash.to_hex_string()[..8]
            );

            self.broadcast_vote(Vote::NotarizeFallback(NotarizeFallbackVote { slot, block_hash }));

            // Alpenglow: state[s] ← state[s] ∪ {BadWindow}
            if let Some(window) = self.get_window_mut(window_idx) {
                window.slots[offset].is_bad_window = true;
            }
        }
    }

    /// upon SafeToSkip(s) do
    ///
    /// Reference: Alpenglow Algorithm 1
    ///
    /// ```text
    /// trySkipWindow(s)
    /// if ItsOver ∉ state[s] then
    ///     broadcast SkipFallbackVote(s)
    ///     state[s] ← state[s] ∪ {BadWindow}
    /// ```
    fn on_safe_to_skip(&mut self, slot: SlotIndex) {
        log::trace!("SimplexState::on_safe_to_skip: slot={}", slot);

        if slot < self.first_non_finalized_slot {
            return;
        }

        let window_idx = slot.window_index(self.slots_per_leader_window);
        let offset = slot.offset_in_window(self.slots_per_leader_window) as usize;

        // Alpenglow: trySkipWindow(s)
        self.try_skip_window(window_idx);

        // Alpenglow: if ItsOver ∉ state[s] then
        self.ensure_window_exists(window_idx);

        // Check if we should broadcast (without holding mutable borrow)
        let should_broadcast =
            self.get_window(window_idx).map(|w| !w.slots[offset].its_over).unwrap_or(false);

        if should_broadcast {
            // Alpenglow: broadcast SkipFallbackVote(s)
            log::trace!(
                "SimplexState::on_safe_to_skip: ({}/{}) broadcasting skip-fb, marking BadWindow",
                window_idx,
                slot
            );

            self.broadcast_vote(Vote::SkipFallback(SkipFallbackVote { slot }));

            // Alpenglow: state[s] ← state[s] ∪ {BadWindow}
            if let Some(window) = self.get_window_mut(window_idx) {
                window.slots[offset].is_bad_window = true;
            }
        }
    }

    /// upon ParentReady(window, hash(b)) do
    ///
    /// Reference: C++ handle ParentReady event, Alpenglow Algorithm 1
    ///
    /// # Errors
    ///
    /// Returns error if window_idx would cause overflow or parent slot is invalid.
    pub fn on_window_base_ready(
        &mut self,
        desc: &SessionDescription,
        window_idx: WindowIndex,
        parent: CandidateParent,
    ) -> Result<()> {
        let start_slot = window_idx.window_start(self.slots_per_leader_window);

        log::trace!(
            "SimplexState::on_window_base_ready: ({}/{}) received parent={}",
            window_idx,
            start_slot,
            Self::format_parent(parent.as_ref())
        );

        // Check for potential overflow in window_idx * slots_per_leader_window
        if window_idx.value().checked_mul(self.slots_per_leader_window).is_none() {
            fail!(
                "SimplexState::on_window_base_ready: \
                w{window_idx} would overflow with {} slots/window",
                self.slots_per_leader_window
            );
        }

        // Validate parent slot if present
        if let Some(ref parent_info) = parent {
            // Parent slot should be less than the window's start slot
            let window_start = window_idx.window_start(self.slots_per_leader_window);
            if parent_info.slot >= window_start {
                fail!(
                    "SimplexState::on_window_base_ready: \
                    parent s{} >= window start s{start_slot} for w{window_idx}",
                    parent_info.slot
                );
            }
        }

        // Ignore if window is fully finalized
        if start_slot < self.first_non_finalized_slot
            && (window_idx + 1).window_start(self.slots_per_leader_window)
                <= self.first_non_finalized_slot
        {
            log::trace!(
                "SimplexState::on_window_base_ready: ({window_idx}/{start_slot}) ignored: \
                window fully finalized (first_non_finalized={})",
                self.first_non_finalized_slot
            );
            return Ok(());
        }

        // Reject far-future windows (DoS protection)
        if self.is_slot_too_far_ahead(start_slot) {
            log::warn!(
                "SimplexState::on_window_base_ready: ({window_idx}/{start_slot}) REJECTED - \
                window too far ahead (max={})",
                self.max_acceptable_slot()
            );
            return Ok(());
        }

        self.ensure_window_exists(window_idx);

        // Alpenglow: state[window.first_slot] ← state[window.first_slot] ∪ {ParentReady(hash(b))}
        if let Some(window) = self.get_window_mut(window_idx) {
            let is_new = window.available_bases.insert(parent.clone());
            log::trace!(
                "SimplexState::on_window_base_ready: ({window_idx}/{start_slot}) \
                {} parent={} to available_bases (count={})",
                if is_new { "added" } else { "duplicate" },
                Self::format_parent(parent.as_ref()),
                window.available_bases.len()
            );
        }

        // Check if pending block can now be notarized
        if let Some(window) = self.get_window(window_idx) {
            if let Some(ref pending) = window.slots[0].pending_block {
                let pending_parent: CandidateParent = pending
                    .parent_id
                    .as_ref()
                    .map(|p| CandidateParentInfo { slot: p.slot, hash: p.hash.clone() });
                if pending_parent == parent {
                    log::trace!(
                        "SimplexState::on_window_base_ready: ({window_idx}/{start_slot}) \
                        pending block {} matched parent, queuing for notarization",
                        Self::format_block(pending.id.slot, &pending.id.hash)
                    );
                    self.pending_slots.push(PendingSlot(start_slot));
                } else {
                    log::trace!(
                        "SimplexState::on_window_base_ready: ({window_idx}/{start_slot}) \
                        pending block {} has different parent (expected={}, got={})",
                        Self::format_block(pending.id.slot, &pending.id.hash),
                        Self::format_parent(parent.as_ref()),
                        Self::format_parent(pending_parent.as_ref())
                    );
                }
            }
        }

        // Alpenglow: checkPendingBlocks()
        self.check_pending_blocks(desc);

        // Alpenglow: setTimeouts(window) with adaptive backoff
        if self.current_leader_window_idx < window_idx {
            log::trace!(
                "SimplexState::on_window_base_ready: ({}/{}) advancing window {}->{}",
                window_idx,
                start_slot,
                self.current_leader_window_idx,
                window_idx
            );

            // Apply adaptive timeout backoff based on previous window's timeout history
            self.apply_adaptive_timeout_backoff(
                desc,
                window_idx,
                "SimplexState::on_window_base_ready",
            );

            // Advance to new window and schedule timeouts
            self.current_leader_window_idx = window_idx;
            self.set_timeouts(desc);
        } else {
            log::trace!(
                "SimplexState::on_window_base_ready: ({}/{}) not advancing window (current={})",
                window_idx,
                start_slot,
                self.current_leader_window_idx
            );
        }

        // Dump state at trace level (compact one-line format)
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "SimplexState::on_window_base_ready: ({}/{}) done {}",
                window_idx,
                start_slot,
                self.debug_dump(desc, false)
            );
        }

        Ok(())
    }

    /*
        ========================================================================
        Helper Functions (Algorithm 2)
        ========================================================================
    */

    /// function tryNotar(Block(s, hash, hashparent))
    ///
    /// Reference: Alpenglow Algorithm 2
    ///
    /// ```text
    /// if Voted ∈ state[s] then return false
    /// firstSlot ← (s is the first slot in leader window)
    /// if firstSlot then
    ///     canVote ← ParentReady(hashparent) ∈ state[s]
    /// else
    ///     canVote ← VotedNotar(hashparent) ∈ state[s-1]
    /// if canVote then
    ///     broadcast NotarVote(s, hash)
    ///     state[s] ← state[s] ∪ {Voted, VotedNotar(hash)}
    ///     pendingBlocks[s] ← ⊥
    ///     tryFinal(s, hash)
    ///     return true
    /// return false
    /// ```
    fn try_notar(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
        block_hash: &UInt256,
        parent: Option<&CandidateParentInfo>,
    ) -> bool {
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;
        let is_first = desc.is_first_in_window(slot);

        self.ensure_window_exists(window_idx);

        // "Already voted" semantics differ by mode:
        // - Alpenglow (enable_fallback_protocol=true): any local vote blocks notar
        // - C++ compatible (enable_fallback_protocol=false): skip does NOT block notar
        //
        // Reference (C++): consensus.cpp on_candidate_to_notarize checks only voted_notar,
        // allowing Notarize after Skip.
        if let Some(window) = self.get_window(window_idx) {
            let slot_state = &window.slots[offset];
            let already_voted = if self.opts.enable_fallback_protocol {
                // Alpenglow: Voted ∈ state[s]
                //
                // Invariant (debug only): if any "local decision" flag is set,
                // then `is_voted` must also be set.
                debug_assert!(
                    !slot_state.voted_skip || slot_state.is_voted,
                    "SimplexState invariant violated: voted_skip implies is_voted (slot={})",
                    slot.value()
                );
                debug_assert!(
                    slot_state.voted_notar.is_none() || slot_state.is_voted,
                    "SimplexState invariant violated: voted_notar implies is_voted (slot={})",
                    slot.value()
                );
                debug_assert!(
                    !slot_state.its_over || slot_state.is_voted,
                    "SimplexState invariant violated: its_over implies is_voted (slot={})",
                    slot.value()
                );

                slot_state.is_voted
            } else {
                // C++ parity: only voted_notar gates notarization. C++ try_notarize()
                // does NOT check voted_final/its_over — a slot that was finalized on a
                // previous run can still be re-notarized after restart (the later
                // auto-finalize simply skips re-broadcasting).
                slot_state.voted_notar.is_some()
            };

            if already_voted {
                log::trace!("SimplexState::try_notar: slot {} already voted", slot);
                return false;
            }
        }

        // Check can_vote_notar
        let can_vote_notar = if self.opts.use_notarized_parent_chain {
            // C++ pool.cpp parity:
            // Parent readiness is determined by per-slot `available_base` chain (not ParentReady/available_bases).
            //
            // Reference: C++ pool.cpp `SlotState::available_base` and request/resolve logic that only
            // allows extending the chain from the known base.
            let expected_base = self.get_slot_available_base(desc, slot);
            let (base_known, expected_parent): (bool, CandidateParent) = match expected_base {
                Some(parent) => (true, parent),
                None => (false, None),
            };

            let candidate_parent: CandidateParent = parent.cloned();
            let matches_parent = base_known && expected_parent == candidate_parent;

            log::trace!(
                "SimplexState::try_notar: ({window_idx}/{slot}) notarized-parent chain: \
                base_known={base_known} expected_base={} candidate_parent={} matches={}",
                Self::format_parent(expected_parent.as_ref()),
                Self::format_parent(parent),
                matches_parent
            );

            matches_parent
        } else if is_first {
            // Alpenglow: firstSlot: ParentReady(hashparent) ∈ state[s]
            let parent_key: CandidateParent = parent.cloned();
            let has_parent = self
                .get_window(window_idx)
                .map(|w| w.available_bases.contains(&parent_key))
                .unwrap_or(false);

            log::trace!(
                "SimplexState::try_notar: ({}/{}) first_in_window, parent={} in_bases={}",
                window_idx,
                slot,
                Self::format_parent(parent),
                has_parent
            );
            has_parent
        } else {
            // Alpenglow: not firstSlot: VotedNotar(hashparent) ∈ state[s-1]
            let Some(parent) = parent else {
                log::trace!(
                    "SimplexState::try_notar: ({}/{}) non-first slot, no parent -> cannot vote",
                    window_idx,
                    slot
                );
                return false;
            };
            let prev_slot = slot - 1;
            let prev_window_idx = desc.get_window_idx(prev_slot);
            let prev_offset = desc.get_slot_offset_in_window(prev_slot) as usize;

            let voted_notar = self
                .get_window(prev_window_idx)
                .and_then(|w| w.slots[prev_offset].voted_notar.as_ref());
            let matches_parent = voted_notar.map(|voted| voted == parent).unwrap_or(false);

            log::trace!(
                "SimplexState::try_notar: ({}/{}) parent={} prev_voted={} matches={}",
                window_idx,
                slot,
                Self::format_parent(Some(parent)),
                Self::format_parent(voted_notar),
                matches_parent
            );
            matches_parent
        };

        if can_vote_notar {
            log::trace!(
                "SimplexState::try_notar: ({}/{}) broadcasting notarize for {}:{}",
                window_idx,
                slot,
                slot,
                &block_hash.to_hex_string()[..8]
            );

            // Alpenglow: broadcast NotarVote(s, hash)
            self.broadcast_vote(Vote::Notarize(NotarizeVote {
                slot,
                block_hash: block_hash.clone(),
            }));

            // Alpenglow: state[s] ← state[s] ∪ {Voted, VotedNotar(hash)}
            if let Some(window) = self.get_window_mut(window_idx) {
                window.slots[offset].is_voted = true;
                window.slots[offset].voted_notar =
                    Some(CandidateParentInfo { slot, hash: block_hash.clone() });
                // Alpenglow: pendingBlocks[s] ← ⊥
                window.slots[offset].pending_block = None;
            }

            // Alpenglow: tryFinal(s, hash)
            self.try_final(desc, slot, block_hash);

            return true;
        }

        false
    }

    /// function tryFinal(s, hash(b))
    ///
    /// Reference: Alpenglow Algorithm 2
    ///
    /// ```text
    /// if BlockNotarized(hash(b)) ∈ state[s] and VotedNotar(hash(b)) ∈ state[s]
    ///    and BadWindow ∉ state[s] then
    ///     broadcast FinalVote(s)
    ///     state[s] ← state[s] ∪ {ItsOver}
    /// ```
    fn try_final(&mut self, desc: &SessionDescription, slot: SlotIndex, block_hash: &UInt256) {
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;

        self.ensure_window_exists(window_idx);

        let should_vote_final = if let Some(window) = self.get_window(window_idx) {
            let slot_state = &window.slots[offset];

            // Alpenglow: BlockNotarized(hash(b)) ∈ state[s]
            let has_notar_cert = slot_state
                .observed_notar_certificate
                .as_ref()
                .map(|c| c.hash == *block_hash)
                .unwrap_or(false);

            // Alpenglow: VotedNotar(hash(b)) ∈ state[s]
            let voted_notar =
                slot_state.voted_notar.as_ref().map(|c| c.hash == *block_hash).unwrap_or(false);

            // Alpenglow: BadWindow ∉ state[s]
            // C++ try_vote_final does NOT check bad_window — it only checks
            // voted_skip, voted_final, and voted_notar==notar_cert.
            let not_bad_window = if self.opts.enable_fallback_protocol {
                !slot_state.is_bad_window
            } else {
                true // C++ doesn't check bad_window in try_vote_final
            };
            let not_its_over = !slot_state.its_over;
            // C++: do not auto-finalize if we already voted skip for this slot.
            // Reference: C++ consensus.cpp: `!voted_skip && !voted_final && voted_notar==id`
            // Both modes now match C++ strictly: once voted_skip, never finalize.
            let not_voted_skip = !slot_state.voted_skip;

            let result =
                has_notar_cert && voted_notar && not_bad_window && not_its_over && not_voted_skip;

            // Log when finalize is blocked specifically by voted_skip (Alpenglow mode only)
            if has_notar_cert && voted_notar && !not_voted_skip {
                log::warn!(
                    "SimplexState::try_final: ({}/{}) FINALIZE BLOCKED by voted_skip! \
                     cert={} notar={} bad_window={} its_over={} voted_skip={}",
                    window_idx,
                    slot,
                    has_notar_cert,
                    voted_notar,
                    slot_state.is_bad_window,
                    slot_state.its_over,
                    slot_state.voted_skip,
                );
            }

            // Only format debug info if trace logging is enabled
            if log::log_enabled!(log::Level::Trace) {
                // Build compact flags string
                let mut flags = Vec::new();
                if slot_state.is_voted {
                    flags.push("V");
                }
                if slot_state.is_bad_window {
                    flags.push("Bad");
                }
                if slot_state.voted_skip {
                    flags.push("Skip");
                }
                if slot_state.its_over {
                    flags.push("Over");
                }
                if slot_state.pending_block.is_some() {
                    flags.push("Pending");
                }
                let flags_str = if flags.is_empty() { "-".to_string() } else { flags.join("|") };

                let voted_str = slot_state
                    .voted_notar
                    .as_ref()
                    .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..8]))
                    .unwrap_or_else(|| "-".to_string());
                let cert_str = slot_state
                    .observed_notar_certificate
                    .as_ref()
                    .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..8]))
                    .unwrap_or_else(|| "-".to_string());

                log::trace!(
                    "SimplexState::try_final: ({}/{}) {}:{} cert={} voted={} flags=[{}] -> vote={}",
                    window_idx,
                    slot,
                    slot,
                    &block_hash.to_hex_string()[..8],
                    has_notar_cert,
                    voted_notar,
                    flags_str,
                    result
                );

                // Additional detailed dump if not voting
                if !result {
                    let total_weight = desc.get_total_weight();
                    if let Some(sv) = self.slot_votes.get(&slot) {
                        let notar_blocks: Vec<String> = sv
                            .notarize_weight_by_block
                            .iter()
                            .map(|(b, w)| {
                                format!(
                                    "{}:{}={}({:.0}%)",
                                    slot,
                                    &b.to_hex_string()[..6],
                                    w,
                                    100.0 * *w as f64 / total_weight as f64
                                )
                            })
                            .collect();
                        log::trace!(
                            "SimplexState::try_final: ({}/{}) details: voted={} cert={} notar=[{}]",
                            window_idx,
                            slot,
                            voted_str,
                            cert_str,
                            if notar_blocks.is_empty() {
                                "-".to_string()
                            } else {
                                notar_blocks.join(",")
                            }
                        );
                    }
                }
            }

            result
        } else {
            log::trace!(
                "SimplexState::try_final: slot={}, block_hash={}, should_vote=false, no window",
                slot,
                block_hash.to_hex_string()
            );
            false
        };

        if should_vote_final {
            log::trace!(
                "SimplexState::try_final: ({}/{}) broadcasting finalize for {}:{}",
                window_idx,
                slot,
                slot,
                &block_hash.to_hex_string()[..8]
            );

            // Alpenglow: broadcast FinalVote(s)
            self.broadcast_vote(Vote::Finalize(FinalizeVote {
                slot,
                block_hash: block_hash.clone(),
            }));

            // Alpenglow: state[s] ← state[s] ∪ {ItsOver}
            // C++: slot->state->voted_final = true
            if let Some(window) = self.get_window_mut(window_idx) {
                window.slots[offset].its_over = true;
                window.slots[offset].voted_final = true;
            }
        }
    }

    /// function trySkipWindow(s)
    ///
    /// Reference: Alpenglow Algorithm 2
    ///
    /// ```text
    /// for k ∈ windowSlots(s) do
    ///     if Voted ∉ state[k] then
    ///         broadcast SkipVote(k)
    ///         state[k] ← state[k] ∪ {Voted, BadWindow}
    ///         pendingBlocks[k] ← ⊥
    /// ```
    fn try_skip_window(&mut self, window_idx: WindowIndex) {
        self.ensure_window_exists(window_idx);

        let start_slot = window_idx * self.slots_per_leader_window;
        let num_slots = self.slots_per_leader_window as usize;
        let enable_fallback = self.opts.enable_fallback_protocol;

        // Collect slots to skip
        // - Alpenglow (enable_fallback_protocol=true): Skip only unvoted slots (Voted ∉ state[k])
        // - C++ compatible (enable_fallback_protocol=false): Skip all non-finalized slots
        //
        // C++ alarm() checks voted_final, not voted_notar:
        // Reference: C++ consensus.cpp alarm(): if (!affected_slot->voted_final)
        let mut slots_to_skip = Vec::new();
        if let Some(window) = self.get_window(window_idx) {
            // Alpenglow: for k ∈ windowSlots(s) do
            for i in 0..num_slots {
                let should_skip = if enable_fallback {
                    // Alpenglow: if Voted ∉ state[k] then
                    !window.slots[i].is_voted
                } else {
                    // C++: if !voted_final — once this node votes final, it cannot
                    // vote skip. This prevents split-brain deadlocks where some
                    // nodes vote skip and others vote final.
                    // Reference: C++ consensus.cpp alarm(): if (!affected_slot->voted_final)
                    !window.slots[i].voted_final
                };
                if should_skip {
                    slots_to_skip.push(start_slot + i as u32);
                }
            }
        }

        if slots_to_skip.is_empty() {
            return;
        }

        {
            let slots_str: Vec<String> = slots_to_skip.iter().map(|s| format!("{}", s)).collect();
            log::warn!(
                "SimplexState::try_skip_window: ({}) SKIP VOTING for {} slots: [{}]",
                window_idx,
                slots_to_skip.len(),
                slots_str.join(",")
            );
        }

        // Skip each unvoted slot
        // Alpenglow: broadcast SkipVote(k)
        for slot in slots_to_skip {
            self.broadcast_vote(Vote::Skip(SkipVote { slot }));

            // Alpenglow: state[k] ← state[k] ∪ {Voted, BadWindow}
            // Alpenglow: pendingBlocks[k] ← ⊥
            let offset = slot.offset_in_window(self.slots_per_leader_window) as usize;
            if let Some(window) = self.get_window_mut(window_idx) {
                window.slots[offset].is_voted = true;
                window.slots[offset].voted_skip = true;
                window.slots[offset].is_bad_window = true;
                // C++ alarm() only sets voted_skip — it does NOT clear pending_block.
                // The async try_notarize() coroutine can still complete after a skip
                // vote, producing both Skip and Notar votes for the same slot.
                // Only clear pending_block in Alpenglow mode (strict Voted gate).
                if enable_fallback {
                    window.slots[offset].pending_block = None;
                }
            }
        }
    }

    /// function checkPendingBlocks()
    ///
    /// Reference: Alpenglow Algorithm 2
    ///
    /// ```text
    /// for s : pendingBlocks[s] ≠ ⊥ do   // iterate with increasing s
    ///     tryNotar(pendingBlocks[s])
    /// ```
    fn check_pending_blocks(&mut self, desc: &SessionDescription) {
        // Alpenglow: for s : pendingBlocks[s] ≠ ⊥ do (iterate with increasing s)
        //
        // Take ownership of pending slots for processing. Slots that still need retry
        // are pushed directly to `self.pending_slots` (which is now empty).
        let mut slots_to_check = mem::take(&mut self.pending_slots);

        while let Some(PendingSlot(slot_id)) = slots_to_check.pop() {
            // Skip already finalized slots
            if slot_id < self.first_non_finalized_slot {
                continue;
            }

            let window_idx = desc.get_window_idx(slot_id);
            let offset = desc.get_slot_offset_in_window(slot_id) as usize;

            // Get pending block info
            let pending_info = self
                .get_window(window_idx)
                .and_then(|w| w.slots[offset].pending_block.as_ref())
                .map(|c| {
                    (
                        c.id.hash.clone(),
                        c.parent_id
                            .as_ref()
                            .map(|p| CandidateParentInfo { slot: p.slot, hash: p.hash.clone() }),
                    )
                });

            if let Some((candidate_hash, parent)) = pending_info {
                // Alpenglow: tryNotar(pendingBlocks[s])
                log::trace!(
                    "SimplexState::check_pending_blocks: ({}/{}) trying pending {}",
                    window_idx,
                    slot_id,
                    Self::format_block(slot_id, &candidate_hash)
                );
                let ok = self.try_notar(desc, slot_id, &candidate_hash, parent.as_ref());
                if !ok {
                    // Still pending (could not vote yet) - re-queue for later retry.
                    let still_pending = self
                        .get_window(window_idx)
                        .and_then(|w| w.slots[offset].pending_block.as_ref())
                        .is_some();
                    if still_pending {
                        self.pending_slots.push(PendingSlot(slot_id));
                    }
                }
            }
        }
    }

    /*
        ========================================================================
        Query Methods
        ========================================================================
    */

    /// Get first non-finalized slot (finalization cursor)
    ///
    /// This is the first slot that is NOT finalized yet.
    pub fn get_first_non_finalized_slot(&self) -> SlotIndex {
        self.first_non_finalized_slot
    }

    /// Returns the maximum slot the FSM will accept (inclusive).
    pub fn max_acceptable_slot(&self) -> SlotIndex {
        self.first_non_finalized_slot + MAX_FUTURE_SLOTS
    }

    /// Returns `true` if `slot` exceeds the acceptable future horizon.
    pub fn is_slot_too_far_ahead(&self, slot: SlotIndex) -> bool {
        slot > self.max_acceptable_slot()
    }

    /// Get first non-progressed slot (progress cursor)
    ///
    /// This is the first slot that has NOT progressed yet, where "progressed" means
    /// finalized OR notarized OR skipped (skip certificate).
    ///
    /// Reference: C++ pool.cpp `PoolImpl::now_` (maybe_publish_new_leader_windows()).
    pub fn get_first_non_progressed_slot(&self) -> SlotIndex {
        self.first_non_progressed_slot
    }

    /// Get current leader window index
    #[allow(dead_code)]
    pub fn get_current_leader_window_idx(&self) -> WindowIndex {
        self.current_leader_window_idx
    }

    /// Get tracked slots interval for standstill vote re-broadcast
    ///
    /// Returns `[begin, end)` range of slots that should be included in standstill.
    /// This matches C++ `tracked_slots_interval()`:
    /// - begin = first_non_finalized_slot
    /// - end = (offset + windows.len()) * slots_per_leader_window
    ///
    /// Reference: C++ pool.cpp alarm() uses state_->tracked_slots_interval()
    pub fn get_tracked_slots_interval(&self) -> (u32, u32) {
        let begin = self.first_non_finalized_slot.value();
        let end = (self.leader_window_offset.value() + self.leader_windows.len() as u32)
            * self.slots_per_leader_window;
        (begin, end)
    }

    /// Collect cached certificates for standstill re-broadcast within a slot range.
    ///
    /// This is used by startup recovery to rebuild Receiver standstill caches without
    /// re-running threshold logic or re-emitting events. The returned vector includes
    /// only slots that currently have at least one cached certificate (notar/skip/final)
    /// and are within `[begin, end)`.
    ///
    /// Reference: C++ pool.cpp `alarm()` calls `certs.serialize_to(messages)` for each
    /// slot in `tracked_slots_interval()`.
    pub fn collect_cached_certificates_in_range(
        &self,
        begin: u32,
        end: u32,
    ) -> Vec<(SlotIndex, Option<NotarCertPtr>, Option<SkipCertPtr>, Option<FinalCertPtr>)> {
        let mut out = Vec::new();

        for (&slot, sv) in &self.slot_votes {
            let s = slot.value();
            if s < begin || s >= end {
                continue;
            }

            let notar = sv.notarize_certificate.clone();
            let skip = sv.skip_certificate.clone();
            let final_ = sv.finalize_certificate.clone();

            if notar.is_some() || skip.is_some() || final_.is_some() {
                out.push((slot, notar, skip, final_));
            }
        }

        // Deterministic ordering helps tests/debugging.
        out.sort_by_key(|(slot, _, _, _)| slot.value());
        out
    }

    /// Get the latest observed finalization certificate (if any).
    ///
    /// C++ stores this as `PoolImpl::last_final_cert_` and always re-broadcasts it
    /// first during standstill.
    ///
    /// On restart, this is used to rebuild the Receiver `last_final_cert` cache.
    pub fn get_last_finalize_certificate(&self) -> Option<(SlotIndex, FinalCertPtr)> {
        let mut best: Option<(SlotIndex, FinalCertPtr)> = None;

        for (&slot, sv) in &self.slot_votes {
            let Some(cert) = sv.finalize_certificate.clone() else {
                continue;
            };

            match &best {
                None => best = Some((slot, cert)),
                Some((best_slot, _)) => {
                    if slot > *best_slot {
                        best = Some((slot, cert));
                    }
                }
            }
        }

        best
    }

    /// Find the latest notarized candidate at or before `up_to_slot`.
    ///
    /// Used by restart recovery when the last finalized slot may be an empty candidate
    /// (which is not persisted on masterchain), but we still must restore the parent/base
    /// chain so new blocks are voteable.
    pub fn get_latest_notarized_candidate_up_to(
        &self,
        up_to_slot: SlotIndex,
    ) -> Option<CandidateParentInfo> {
        let mut best: Option<CandidateParentInfo> = None;

        for (&slot, sv) in &self.slot_votes {
            if slot > up_to_slot {
                continue;
            }
            let Some(cert) = sv.notarize_certificate.as_ref() else {
                continue;
            };

            match &best {
                None => {
                    best = Some(CandidateParentInfo { slot, hash: cert.vote.block_hash.clone() });
                }
                Some(current_best) => {
                    if slot > current_best.slot {
                        best =
                            Some(CandidateParentInfo { slot, hash: cert.vote.block_hash.clone() });
                    }
                }
            }
        }

        best
    }

    /// Check if a slot has a notarized block (observed notarization certificate)
    ///
    /// Used for debug logging to show consensus progress.
    pub fn has_notarized_block(&self, slot: SlotIndex) -> bool {
        if slot < self.first_non_finalized_slot {
            // Finalized slots are by definition notarized
            return true;
        }

        // Check current windows for the slot
        for window in &self.leader_windows {
            let window_start = window.start_slot;
            let window_end = window_start + window.slots.len() as u32;

            if slot >= window_start && slot < window_end {
                let offset = (slot - window_start) as usize;
                return window.slots[offset].observed_notar_certificate.is_some();
            }
        }

        false
    }

    /// Check if a slot is finalized (ItsOver flag)
    ///
    /// Used for debug logging to show consensus progress.
    pub fn is_slot_finalized(&self, slot: SlotIndex) -> bool {
        // Slots before first_non_finalized are definitionally finalized
        if slot < self.first_non_finalized_slot {
            return true;
        }

        // Check current windows for the slot
        for window in &self.leader_windows {
            let window_start = window.start_slot;
            let window_end = window_start + window.slots.len() as u32;

            if slot >= window_start && slot < window_end {
                let offset = (slot - window_start) as usize;
                return window.slots[offset].its_over;
            }
        }

        false
    }

    /// Check if this node should generate a block for the current slot
    #[allow(dead_code)]
    pub fn should_generate_block(
        &self,
        desc: &SessionDescription,
    ) -> Option<(SlotIndex, Option<CandidateParentInfo>)> {
        let slot = self.first_non_finalized_slot;
        let window_idx = desc.get_window_idx(slot);
        let offset = desc.get_slot_offset_in_window(slot) as usize;

        // Check if we're the leader
        if !desc.is_self_leader(slot) {
            return None;
        }

        // Check if already voted
        if let Some(window) = self.get_window(window_idx) {
            if window.slots[offset].is_voted {
                return None;
            }

            // Get parent from available bases or previous slot
            let parent = if desc.is_first_in_window(slot) {
                // For first slot, pick any available base
                window.available_bases.iter().next().cloned().flatten()
            } else {
                // For other slots, use voted_notar from previous slot
                let prev_offset = offset - 1;
                window.slots[prev_offset].voted_notar.clone()
            };

            return Some((slot, parent));
        }

        None
    }

    /// Get available parent for block generation at a given slot
    ///
    /// This is derived from per-slot `Slot.available_base` (C++ pool.cpp `SlotState::available_base`)
    /// and represents the canonical parent chain for collation:
    ///
    /// - `available_base == None` → base unknown yet (no parent available)
    /// - `available_base == Some(None)` → genesis base (parent is None)
    /// - `available_base == Some(Some(id))` → use `id` as parent
    ///
    /// Parent validity depends on `require_finalized_parent` option:
    /// - `require_finalized_parent=false` (C++ mode, default): parent can be notarized OR finalized
    /// - `require_finalized_parent=true` (strict mode): parent must be finalized
    ///
    /// Reference: C++ pool.cpp `SlotState::available_base`, block-producer.cpp `get_parent()`.
    pub fn get_available_parent(
        &self,
        desc: &SessionDescription,
        slot: SlotIndex,
    ) -> Option<CandidateParentInfo> {
        let base = self.get_slot_available_base(desc, slot)?;
        match base {
            // Genesis base: no parent id.
            None => None,
            Some(parent_info) => {
                if self.is_parent_valid(parent_info.slot) {
                    Some(parent_info)
                } else {
                    None
                }
            }
        }
    }

    /// Check if parent is available for block generation at a given slot
    ///
    /// Parent availability is derived from per-slot `Slot.available_base` (C++ pool.cpp `SlotState::available_base`).
    ///
    /// - `available_base == None` → base unknown (no parent available)
    /// - `available_base == Some(None)` → genesis base (parent is available)
    /// - `available_base == Some(Some(id))` → parent is available if it is valid
    ///
    /// Parent validity depends on the `require_finalized_parent` option:
    /// - `require_finalized_parent=false` (C++ mode, default): parent can be notarized OR finalized
    /// - `require_finalized_parent=true` (strict mode): parent must be finalized
    pub fn has_available_parent(&self, desc: &SessionDescription, slot: SlotIndex) -> bool {
        let base = self.get_slot_available_base(desc, slot);
        match base {
            None => false,
            Some(None) => true, // genesis base
            Some(Some(parent_info)) => self.is_parent_valid(parent_info.slot),
        }
    }

    /// Check if a slot's block is valid as a parent for child blocks
    ///
    /// A parent is valid if:
    /// - Slot is finalized (< first_non_finalized_slot), OR
    /// - When `require_finalized_parent = false`: slot has observed notarization certificate
    pub fn is_parent_valid(&self, parent_slot: SlotIndex) -> bool {
        // Finalized slots are always valid parents
        if parent_slot < self.first_non_finalized_slot {
            return true;
        }

        // Strict mode: require finalized parent
        if self.opts.require_finalized_parent {
            return false;
        }

        // C++ mode: notarized block is valid parent
        // Check if the slot has observed notarization certificate
        self.has_notarized_block(parent_slot)
    }

    /// Get indices of validators who voted finalize for a block in a slot
    ///
    /// Returns indices of validators who have finalize votes matching the block.
    /// Used by SessionProcessor to collect signatures for on_block_committed.
    #[allow(dead_code)] // Replaced by certificate.signatures
    pub fn get_finalize_voters(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
    ) -> Vec<ValidatorIndex> {
        if let Some(slot_votes) = self.slot_votes.get(&slot) {
            slot_votes
                .votes
                .iter()
                .enumerate()
                .filter_map(|(idx, v)| {
                    if let Some(ref finalize) = v.finalize {
                        if finalize.block_hash == *block_hash {
                            return Some(ValidatorIndex::from(idx));
                        }
                    }
                    None
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get indices of validators who voted notarize for a block in a slot
    ///
    /// Returns indices of validators who have notarize votes matching the block.
    /// Note: Prefer `get_notarize_certificate` which includes actual signatures.
    #[allow(dead_code)]
    pub fn get_notarize_voters(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
    ) -> Vec<ValidatorIndex> {
        if let Some(slot_votes) = self.slot_votes.get(&slot) {
            slot_votes
                .votes
                .iter()
                .enumerate()
                .filter_map(|(idx, v)| {
                    if let Some(ref notarize) = v.notarize {
                        if notarize.block_hash == *block_hash {
                            return Some(ValidatorIndex::from(idx));
                        }
                    }
                    None
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get cached notarization certificate for a block in a slot
    ///
    /// Returns the cached certificate if notarization threshold (2/3) was reached,
    /// or None if no certificate exists for this block.
    ///
    /// Used by candidate resolver to respond to requestCandidate queries.
    /// Reference: C++ CandidateResolver returns cached NotarCertRef
    pub fn get_notarize_certificate(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
    ) -> Option<NotarCertPtr> {
        self.slot_votes.get(&slot).and_then(|sv| sv.get_notarize_certificate(block_hash))
    }

    /// Set notarization certificate from external source (query response)
    ///
    /// Updates vote accounting with votes from the certificate so FSM recognizes
    /// the block as notarized. Called when we receive a candidate + notar cert
    /// via requestCandidate query.
    ///
    /// # Arguments
    /// * `desc` - Session description (for validator weights)
    /// * `slot` - Slot index
    /// * `block_hash` - Block hash
    /// * `certificate` - The notarization certificate
    ///
    /// # Reference
    ///
    /// When C++ receives a candidate via requestCandidate, it also receives the
    /// NotarCertRef and must update pool state accordingly. This method implements
    /// that update for the Rust FSM.
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists for the same block (idempotent)
    /// - `Err` if certificate already exists for a different block (conflict)
    pub fn set_notarize_certificate(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
        block_hash: &UInt256,
        certificate: NotarCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        let first_non_finalized_slot = self.first_non_finalized_slot;
        let sv = self.slot_votes_at(slot);

        // Try to store the certificate first to check for conflicts
        match sv.store_notarize_certificate(block_hash, certificate.clone()) {
            Ok(true) => {
                // New certificate - update vote accounting
            }
            Ok(false) => {
                log::trace!(
                    "SimplexState::set_notarize_certificate: slot={} block={} - certificate already exists, skipping",
                    slot,
                    &block_hash.to_hex_string()[..8]
                );
                return Ok(false);
            }
            Err(e) => {
                log::warn!(
                    "SimplexState::set_notarize_certificate: slot={} block={} - {}",
                    slot,
                    &block_hash.to_hex_string()[..8],
                    e
                );
                return Err(e);
            }
        }

        // Update vote accounting from certificate signatures
        for vote_sig in &certificate.signatures {
            let idx = vote_sig.validator_idx;
            if idx.value() as usize >= sv.votes.len() {
                log::warn!(
                    "SimplexState::set_notarize_certificate: invalid validator index {} >= {}",
                    idx,
                    sv.votes.len()
                );
                continue;
            }

            let votes = &mut sv.votes[idx.value() as usize];

            // Only add if this validator hasn't already voted notarize
            if votes.notarize.is_none() {
                let notarize_vote = NotarizeVote { slot, block_hash: block_hash.clone() };

                // Track if this is the first notarize/skip vote
                let had_notarize_or_skip = votes.notarize.is_some() || votes.skip.is_some();

                votes.notarize = Some(notarize_vote);
                votes.notarize_signature = Some(vote_sig.signature.clone());

                // Update weight tracking
                let weight = desc.get_node_weight(idx);
                *sv.notarize_weight_by_block.entry(block_hash.clone()).or_insert(0) += weight;
                if !had_notarize_or_skip {
                    sv.notarize_or_skip_weight += weight;
                }
            }
        }

        // Mark as notarized (for block_notarized_published flag)
        sv.block_notarized_published = true;

        // C++ `state.slot_at(slot)` returns nullopt for finalized slots (slot < first_non_finalized_slot_),
        // so pool state is not mutated for old slots. In Rust we may still need the notar certificate
        // for restart recommit signature extraction, so we store it in SlotVotes but skip any
        // window/base/progress tracking updates for finalized slots.
        if slot < first_non_finalized_slot {
            log::trace!(
                "SimplexState::set_notarize_certificate: \
                slot={slot} < first_non_finalized={first_non_finalized_slot} - \
                stored cert without slot tracking"
            );
            return Ok(true);
        }

        log::trace!(
            "SimplexState::set_notarize_certificate: \
            slot={slot} block={} - stored certificate with {} signatures",
            &block_hash.to_hex_string()[..8],
            certificate.signatures.len()
        );

        // Emit NotarizationReached to let SessionProcessor perform:
        // - DB persistence (WaitCandidateInfoStored parity)
        // - receiver resolver cache updates (requestCandidate)
        // - certificate relay/standstill caching
        //
        // This mirrors the threshold-driven path (`check_thresholds_and_trigger`) where
        // notarization threshold stores the cert and emits NotarizationReached.
        // C++ parity (pool.cpp handle_saved_certificate): re-gossip every newly
        // accepted certificate regardless of origin.  SimplexState deduplication
        // (returns Ok(false) for already-stored certs) prevents amplification loops.
        self.push_event_back(SimplexEvent::NotarizationReached(NotarizationReachedEvent {
            slot,
            block_hash: block_hash.clone(),
            certificate: certificate.clone(),
        }));

        // Trigger the same internal FSM transition as the threshold-driven path.
        // This updates window slot state, propagates notarized-parent tracking,
        // and may broadcast Finalize votes via `try_final`.
        self.on_block_notarized(desc, slot, block_hash.clone());

        Ok(true)
    }

    /// Set finalization certificate from external source
    ///
    /// Updates FSM state as if we had received enough finalize votes to create
    /// the certificate. This is used when receiving a `consensus.simplex.certificate`
    /// protocol message containing a finalize certificate.
    ///
    /// Reference: C++ pool.cpp `handle_foreign_certificate(Certificate<Vote>&& cert)`
    /// which calls `slot->state->certs.store(cert)` and updates vote accounting.
    ///
    /// # Arguments
    /// * `desc` - Session description (for validator weights)
    /// * `slot` - Slot index
    /// * `block_hash` - Block hash
    /// * `certificate` - The finalization certificate
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists for the same block (idempotent)
    /// - `Err` if certificate already exists for a different block (conflict)
    pub fn set_finalize_certificate(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
        block_hash: &UInt256,
        certificate: FinalCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        // C++ `state.slot_at(slot)` returns nullopt for finalized slots,
        // so pool state is not mutated for old slots.
        //
        // In Rust we may still need the FinalCert for:
        // - gapless masterchain commit (ValidatorGroup compatibility), and
        // - restart recommit signature extraction.
        //
        // Store the certificate in SlotVotes, but skip any window/base/progress tracking updates
        // and DO NOT emit events for finalized slots (prevents state resurrection / regressions).
        let first_non_finalized_slot = self.first_non_finalized_slot;
        let is_old_slot = slot < first_non_finalized_slot;

        let sv = self.slot_votes_at(slot);

        // Try to store the certificate first to check for conflicts
        match sv.store_finalize_certificate(block_hash, certificate.clone()) {
            Ok(true) => {
                // New certificate - update vote accounting
            }
            Ok(false) => {
                log::trace!(
                    "SimplexState::set_finalize_certificate: \
                    slot={slot} - certificate already exists, skipping"
                );
                return Ok(false);
            }
            Err(e) => {
                log::warn!(
                    "SimplexState::set_finalize_certificate: slot={slot} block={} - {e}",
                    &block_hash.to_hex_string()[..8]
                );
                return Err(e);
            }
        }

        // Update vote accounting from certificate signatures
        for vote_sig in &certificate.signatures {
            let idx = vote_sig.validator_idx;
            if idx.value() as usize >= sv.votes.len() {
                log::warn!(
                    "SimplexState::set_finalize_certificate: invalid validator index {idx} >= {}",
                    sv.votes.len()
                );
                continue;
            }

            let votes = &mut sv.votes[idx.value() as usize];

            // Only add if this validator hasn't already voted finalize
            if votes.finalize.is_none() {
                let finalize_vote = FinalizeVote { slot, block_hash: block_hash.clone() };

                votes.finalize = Some(finalize_vote);
                votes.finalize_signature = Some(vote_sig.signature.clone());

                // Update weight tracking
                let weight = desc.get_node_weight(idx);
                *sv.finalize_weight_by_block.entry(block_hash.clone()).or_insert(0) += weight;
            }
        }

        // Mark as finalized (for block_finalized_published flag)
        sv.block_finalized_published = true;

        log::trace!(
            "SimplexState::set_finalize_certificate: \
            slot={slot} block={} - stored certificate with {} signatures",
            &block_hash.to_hex_string()[..8],
            certificate.signatures.len()
        );

        // For old slots, store cert only (no tracking / no events).
        if is_old_slot {
            log::trace!(
                "SimplexState::set_finalize_certificate: \
                slot={slot} < first_non_finalized={first_non_finalized_slot} - \
                stored cert without slot tracking"
            );
            return Ok(true);
        }

        // Emit events (commit trigger + standstill caching) for externally provided certs.
        // This makes external certificate ingestion consistent with the threshold-driven path.
        //
        // NOTE: BlockFinalized is required to trigger commit attempts even when
        // no finalize votes were observed locally.
        let block_id = self.candidate_ids.get(block_hash).map(|c| c.block.clone());
        self.push_event_back(SimplexEvent::BlockFinalized(BlockFinalizedEvent {
            slot,
            block_hash: block_hash.clone(),
            block_id,
            certificate: certificate.clone(),
        }));
        // C++ parity (pool.cpp handle_saved_certificate): re-gossip every newly
        // accepted certificate regardless of origin.
        self.push_event_back(SimplexEvent::FinalizationReached(FinalizationReachedEvent {
            slot,
            block_hash: block_hash.clone(),
            certificate: certificate.clone(),
        }));

        // C++ parity (pool.cpp handle_certificate(FinalCertRef)):
        // Final certificate implies notarization for parent-chain tracking.
        // If we missed notarization earlier, record and propagate it now to keep state consistent.
        let parent_info = CandidateParentInfo { slot, hash: block_hash.clone() };
        let missing_notar_marker = self
            .get_slot_ref(desc, slot)
            .map(|s| s.observed_notar_certificate.is_none())
            .unwrap_or(true);
        if missing_notar_marker {
            log::trace!(
                "SimplexState::set_finalize_certificate: slot={slot} block={} -> \
                treat FinalCert as notarization for parent-chain tracking (missing marker)",
                &block_hash.to_hex_string()[..8],
            );

            let mut observed_marker_set = false;
            if let Some(s) = self.get_slot_mut(desc, slot) {
                if s.observed_notar_certificate.is_none() {
                    s.observed_notar_certificate = Some(parent_info.clone());
                    observed_marker_set = true;
                }
            } else {
                // Should not happen for non-old slots; keep trace only (avoid panic in foreign cert ingestion).
                log::trace!(
                    "SimplexState::set_finalize_certificate: \
                    slot={slot} block={} missing notar marker but slot state is missing",
                    &block_hash.to_hex_string()[..8],
                );
            }

            self.propagate_base_after_notarization(desc, parent_info.clone());

            log::trace!(
                "SimplexState::set_finalize_certificate: slot={slot} block={} \
                FinalCert-as-notar applied (observed_marker_set={observed_marker_set}, \
                first_non_progressed_slot={}, first_non_finalized_slot={})",
                &block_hash.to_hex_string()[..8],
                self.first_non_progressed_slot,
                self.first_non_finalized_slot,
            );
        }

        // Update finalized boundary (C++ notify_finalized/handle_certificate parity).
        let next_slot = SlotIndex::new(slot.value() + 1);
        if next_slot > self.first_non_finalized_slot {
            self.first_non_finalized_slot = next_slot;
        }
        if self.first_non_finalized_slot > self.first_non_progressed_slot {
            self.first_non_progressed_slot = self.first_non_finalized_slot;
        }

        // Advance leader windows using the active mode's strategy.
        if self.opts.use_notarized_parent_chain {
            self.advance_leader_window_on_progress_cursor(desc);
        } else {
            let next_window_idx = slot.window_index(self.slots_per_leader_window) + 1;
            if let Err(e) = self.on_window_base_ready(desc, next_window_idx, Some(parent_info)) {
                log::error!(
                    "SimplexState::set_finalize_certificate: ParentReady failed for w{} parent={}:{}: {}",
                    next_window_idx,
                    slot,
                    &block_hash.to_hex_string()[..8],
                    e
                );
            }
        }

        Ok(true)
    }

    /// Set skip certificate from external source (C++ parity)
    ///
    /// Updates FSM state as if we had received enough skip votes to create
    /// the certificate. This is used when receiving a `consensus.simplex.certificate`
    /// protocol message containing a skip certificate.
    ///
    /// Reference: C++ pool.cpp `handle_foreign_certificate(Certificate<Vote>&& cert)`
    /// which calls `slot->state->certs.store(cert)` and updates vote accounting.
    ///
    /// # Arguments
    /// * `desc` - Session description (for validator weights)
    /// * `slot` - Slot index
    /// * `certificate` - The skip certificate
    ///
    /// # Returns
    /// - `Ok(true)` if certificate was stored (new)
    /// - `Ok(false)` if certificate already exists (idempotent - skip has no block hash)
    pub fn set_skip_certificate(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
        certificate: SkipCertPtr,
    ) -> std::result::Result<bool, CertificateStoreError> {
        // C++ `state.slot_at(slot)` returns nullopt for finalized slots, so foreign certificates
        // for old slots are ignored.
        if slot < self.first_non_finalized_slot {
            log::trace!(
                "SimplexState::set_skip_certificate: slot={} < first_non_finalized={} - ignoring",
                slot,
                self.first_non_finalized_slot
            );
            return Ok(false);
        }

        let sv = self.slot_votes_at(slot);

        // Try to store the certificate first
        match sv.store_skip_certificate(certificate.clone()) {
            Ok(true) => {
                // New certificate - update vote accounting
            }
            Ok(false) => {
                log::trace!(
                    "SimplexState::set_skip_certificate: \
                    slot={slot} - certificate already exists, skipping"
                );
                return Ok(false);
            }
            Err(e) => {
                // Skip certs don't have block hash, so this shouldn't happen
                log::warn!("SimplexState::set_skip_certificate: slot={} - {}", slot, e);
                return Err(e);
            }
        }

        // Update vote accounting from certificate signatures
        for vote_sig in &certificate.signatures {
            let idx = vote_sig.validator_idx;
            if idx.value() as usize >= sv.votes.len() {
                log::warn!(
                    "SimplexState::set_skip_certificate: invalid validator index {} >= {}",
                    idx,
                    sv.votes.len()
                );
                continue;
            }

            let votes = &mut sv.votes[idx.value() as usize];

            // Track if this is the first notarize/skip vote
            let had_notarize_or_skip = votes.notarize.is_some() || votes.skip.is_some();

            // Only add if this validator hasn't already voted skip
            if votes.skip.is_none() {
                let skip_vote = SkipVote { slot };
                votes.skip = Some(skip_vote);
                votes.skip_signature = Some(vote_sig.signature.clone());

                // Update weight tracking
                let weight = desc.get_node_weight(idx);
                if !had_notarize_or_skip {
                    sv.notarize_or_skip_weight += weight;
                }
                sv.skip_or_skip_fallback_weight += weight;
            }
        }

        // Mark slot as skipped
        sv.slot_skipped_published = true;

        log::trace!(
            "SimplexState::set_skip_certificate: \
            slot={slot} - stored certificate with {} signatures",
            certificate.signatures.len()
        );

        // Update slot state to mark as skipped
        let window_idx = slot.window_index(self.slots_per_leader_window);
        let offset = slot.offset_in_window(self.slots_per_leader_window) as usize;
        self.ensure_window_exists(window_idx);
        if let Some(window) = self.get_window_mut(window_idx) {
            if offset < window.slots.len() {
                window.slots[offset].skipped = true;
            }
        }

        // Propagate base after skip (C++ pool.cpp parity)
        self.propagate_base_after_skip_cert(desc, slot);

        // C++ parity: skip certificates do NOT advance first_non_finalized_slot.
        // Only finalization advances it (C++ state.h notify_finalized()).
        // The progress cursor (first_non_progressed_slot) DOES advance on skip.

        // Advance progress cursor
        if self.opts.use_notarized_parent_chain {
            // Advance first_non_progressed_slot if this slot was blocking progress
            if slot == self.first_non_progressed_slot {
                self.advance_progress_cursor(desc);
            }
            self.advance_leader_window_on_progress_cursor(desc);
        }

        // Emit SlotSkipped event so SessionProcessor can progress/cleanup state.
        // This mirrors the threshold-driven path which emits SlotSkipped when the
        // skip certificate is created from votes.
        self.push_event_back(SimplexEvent::SlotSkipped(SlotSkippedEvent { slot }));

        // C++ parity (pool.cpp handle_saved_certificate): re-gossip every newly
        // accepted certificate regardless of origin.
        //
        // SkipCertificateReached is only relevant in C++-compatible mode
        // (Alpenglow paper does not require explicit skip certificate broadcast).
        if !self.opts.enable_fallback_protocol {
            self.push_event_back(SimplexEvent::SkipCertificateReached(
                SkipCertificateReachedEvent { slot, certificate: certificate.clone() },
            ));
        }

        Ok(true)
    }

    /// Get finalization vote weight for a block
    ///
    /// Returns the total weight of validators who voted finalize for this block.
    /// Used for testing vote accounting.
    #[cfg(test)]
    pub fn get_finalize_weight(&self, slot: SlotIndex, block_hash: &UInt256) -> ValidatorWeight {
        self.slot_votes
            .get(&slot)
            .and_then(|sv| sv.finalize_weight_by_block.get(block_hash))
            .copied()
            .unwrap_or(0)
    }

    /// Get skip vote weight for a slot
    ///
    /// Returns the total weight of validators who voted skip for this slot.
    /// Used for testing vote accounting.
    #[cfg(test)]
    pub fn get_skip_weight(&self, slot: SlotIndex) -> ValidatorWeight {
        self.slot_votes.get(&slot).map_or(0, |sv| sv.skip_or_skip_fallback_weight)
    }

    /// Check if a slot has a finalize certificate
    #[cfg(test)]
    pub fn has_finalize_certificate(&self, slot: SlotIndex) -> bool {
        self.slot_votes.get(&slot).map_or(false, |sv| sv.finalize_certificate.is_some())
    }

    /// Get finalization certificate for a specific candidate (slot, hash), if present.
    pub fn get_finalize_certificate(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
    ) -> Option<FinalCertPtr> {
        let sv = self.slot_votes.get(&slot)?;
        let cert = sv.finalize_certificate.as_ref()?;
        (cert.vote.block_hash == *block_hash).then(|| cert.clone())
    }

    /// Check if a slot has a skip certificate
    #[cfg(test)]
    pub fn has_skip_certificate(&self, slot: SlotIndex) -> bool {
        self.slot_votes.get(&slot).map_or(false, |sv| sv.skip_certificate.is_some())
    }

    /// Get notarization vote weight for a block
    ///
    /// Returns the total weight of validators who voted notarize for this block.
    /// Used for testing vote accounting.
    #[cfg(test)]
    pub fn get_notarize_weight(&self, slot: SlotIndex, block_hash: &UInt256) -> ValidatorWeight {
        self.slot_votes
            .get(&slot)
            .and_then(|sv| sv.notarize_weight_by_block.get(block_hash))
            .copied()
            .unwrap_or(0)
    }

    /// Get the candidate stored in slot state (if any)
    ///
    /// Returns the pending block or the voted_notar block info.
    /// Used for retrieving block data during finalization.
    #[allow(dead_code)]
    pub fn get_slot_candidate(&self, slot: SlotIndex) -> Option<&Candidate> {
        let window_idx = slot.window_index(self.slots_per_leader_window);
        let offset = slot.offset_in_window(self.slots_per_leader_window) as usize;

        if let Some(window) = self.get_window(window_idx) {
            if offset < window.slots.len() {
                return window.slots[offset].pending_block.as_ref();
            }
        }
        None
    }

    /*
        ========================================================================
        Notarized-Parent Chain Base Propagation (C++ pool.cpp parity)
        ========================================================================

        These methods maintain the notarized-parent chain tracking state:
        - `Slot.available_base` (per-slot parent, C++ `SlotState::available_base`)
        - `Slot.skipped` (skip certificate flag, C++ `SlotState::skipped`)
        - `SimplexState.first_non_progressed_slot` (progress cursor, C++ `PoolImpl::now_`)

        The tracking state is **always maintained** for consistency, regardless of
        the `SimplexStateOptions::use_notarized_parent_chain` flag.

        When `use_notarized_parent_chain` is **disabled** (legacy ParentReady-driven mode):
        - Tracking state is updated but does not drive leader-window progression
        - Leader window advancement / timeout scheduling is driven by `on_window_base_ready()` (finalization)

        When `use_notarized_parent_chain` is **enabled** (C++ pool.cpp parity, default for `cpp_compatible()`):
        - Tracking state drives leader window advancement / timeout scheduling
        - `first_non_progressed_slot` cursor determines when to advance `current_leader_window_idx`
        - Parent readiness for notarization follows `available_base` chain (not `available_bases`)

        This design allows:
        - State consistency: no mode-dependent null/partial state
        - Easy testing: always inspect tracking state in tests
        - Clean migration: flip flag without restructuring core FSM logic

        Reference: C++ pool.cpp `PoolImpl::now_`, `SlotState::available_base`,
        `on_notarization()`, `on_skip()`, `maybe_publish_new_leader_windows()`
    */

    /// Propagate available base to next non-skipped slot after notarization
    ///
    /// Reference: C++ pool.cpp on_notarization():
    ///   `next_nonskipped_slot_after(id.slot).available_base = id`
    /// Note: the Rust implementation uses max-merge (`add_available_base_max`) instead of
    /// unconditional assignment, to prevent regression when duplicate/late notarizations arrive.
    ///
    /// This is always called when a block is notarized, regardless of mode.
    /// The tracked state is used for progress when `use_notarized_parent_chain` is enabled.
    fn propagate_base_after_notarization(
        &mut self,
        desc: &SessionDescription,
        parent_info: CandidateParentInfo,
    ) {
        let next_slot = self.find_next_nonskipped_slot(desc, parent_info.slot);
        if let Some(slot_state) = self.get_slot_mut(desc, next_slot) {
            log::trace!(
                "SimplexState: propagating base {}:{} -> slot {} (after notarization, max-merge)",
                parent_info.slot,
                &parent_info.hash.to_hex_string()[..8],
                next_slot
            );
            slot_state.add_available_base_max(Some(parent_info));
        }

        // Advance progress cursor through any progressed slots
        self.advance_progress_cursor(desc);

        // In notarized-parent chain mode, base propagation can make pending blocks voteable.
        // Retry pending blocks immediately to match C++ pool behavior.
        if self.opts.use_notarized_parent_chain {
            self.check_pending_blocks(desc);
        }
    }

    /// Set available base for the first non-finalized slot after restart recovery
    ///
    /// This is called during restart recovery to establish the parent chain for
    /// new blocks. The last finalized block becomes the available_base for the
    /// first non-finalized slot.
    ///
    /// Without this, the first non-finalized slot would have `available_base = None`,
    /// causing new blocks to be unvoteable (no valid parent chain).
    pub fn set_available_base_after_restart(
        &mut self,
        desc: &SessionDescription,
        parent_info: CandidateParentInfo,
    ) {
        let next_slot = self.first_non_finalized_slot;
        if let Some(slot_state) = self.get_slot_mut(desc, next_slot) {
            log::trace!(
                "SimplexState::set_available_base_after_restart: setting base {}:{} for slot {}",
                parent_info.slot,
                &parent_info.hash.to_hex_string()[..8],
                next_slot
            );
            slot_state.available_base = Some(Some(parent_info));
        } else {
            log::warn!(
                "SimplexState::set_available_base_after_restart: slot {} not found in FSM",
                next_slot
            );
        }
    }

    /// Propagate available base forward after skip certificate
    ///
    /// Reference: C++ pool.cpp on_skip():
    ///   `slot.skipped = true`
    ///   `if (auto base = slot.state->available_base) next_slot.state->add_available_base(*base);`
    /// Note: C++ uses `add_available_base` (max-merge), not a conditional assignment.
    ///
    /// C++ also calls `maybe_resolve_requests()` (pool.cpp) after every certificate,
    /// which does a backward walk to resolve pending parent-wait requests even if
    /// `available_base` was not set on intermediate slots. Rust has no backward walk,
    /// so instead we chain the base forward through all consecutive already-skipped
    /// slots, ensuring every intermediate slot gets its `available_base` set. This
    /// allows `check_pending_blocks` / `try_notar` to find the base for any pending
    /// block regardless of skip-cert arrival order.
    ///
    /// This is always called when a slot is skipped, regardless of mode.
    /// The tracked state is used for progress when `use_notarized_parent_chain` is enabled.
    fn propagate_base_after_skip_cert(&mut self, desc: &SessionDescription, slot: SlotIndex) {
        // Mark slot as skipped (skip certificate reached)
        if let Some(slot_state) = self.get_slot_mut(desc, slot) {
            slot_state.skipped = true;

            log::trace!(
                "SimplexState: slot {} marked skipped (skip cert), propagating base forward",
                slot
            );
        }

        // Chain base forward: propagate slot-by-slot through consecutive already-skipped
        // slots. Unlike the previous `find_next_nonskipped_slot` approach which jumped
        // directly to the first non-skipped slot (potentially hundreds of slots away),
        // this ensures every intermediate skipped slot gets its `available_base` set.
        //
        // Without this chaining, skip certs arriving out-of-order leave gaps:
        //   cert(5) arrives first → slot 5 has no base → nothing propagates
        //   cert(0) arrives → base jumps from 0 to 388 (next non-skipped) → slots 1-387 have no base
        // With chaining:
        //   cert(0) → base set on slot 1 → slot 1 already skipped → chain to slot 2 → ... → slot 388
        let mut current = slot;
        loop {
            let current_base = self.get_slot_available_base(desc, current);
            let Some(base) = current_base else {
                break;
            };
            let next = current + 1;
            self.ensure_window_exists(desc.get_window_idx(next));
            if let Some(next_state) = self.get_slot_mut(desc, next) {
                log::trace!(
                    "SimplexState: propagating base from skipped slot {} -> slot {} (max-merge)",
                    current,
                    next
                );
                next_state.add_available_base_max(base);
            }
            if self.is_slot_skipped_cert(desc, next) {
                current = next;
            } else {
                break;
            }
        }

        // C++ compatibility: advance skip timer when SkipCert arrives
        // Reference: C++ consensus.cpp lines 228-248 (NotarizationObserved handler)
        // C++ advances timeout_slot_ on both NotarCert and SkipCert (via LeaderWindowObserved).
        // Without this, the Rust skip cascade takes ~27s for 27 slots (1s/slot) while
        // C++ processes entire windows at once and advances the timer on each event.
        //
        // Important: do NOT shrink skip_timestamp below the current scheduled value
        // to preserve the first_block_timeout window.
        if !self.opts.enable_fallback_protocol {
            let next_slot = slot + 1;
            if self.skip_slot <= next_slot {
                let new_timestamp = desc.get_time() + self.target_rate_timeout;
                let effective_timestamp = match self.skip_timestamp {
                    Some(current) if current > new_timestamp => current,
                    _ => new_timestamp,
                };
                log::debug!(
                    "SimplexState::propagate_base_after_skip_cert: advancing skip timer: \
                    skip_slot {} -> {}, new timeout in {:?}{}",
                    self.skip_slot,
                    next_slot,
                    self.target_rate_timeout,
                    if effective_timestamp != new_timestamp {
                        " (preserved first_block_timeout)"
                    } else {
                        ""
                    }
                );
                self.skip_slot = next_slot;
                self.skip_timestamp = Some(effective_timestamp);
            }
        }

        // Advance progress cursor through any progressed slots
        self.advance_progress_cursor(desc);

        // In notarized-parent chain mode, base propagation can make pending blocks voteable.
        // Retry pending blocks immediately to match C++ pool behavior.
        if self.opts.use_notarized_parent_chain {
            self.check_pending_blocks(desc);
        }
    }

    /// Advance progress cursor through all progressed slots
    ///
    /// Reference: C++ pool.cpp maybe_publish_new_leader_windows():
    ///   `while (slot(now_).notarized || slot(now_).skipped) ++now_`
    ///
    /// This helper is always called to keep `first_non_progressed_slot` up-to-date with consensus progress.
    fn advance_progress_cursor(&mut self, desc: &SessionDescription) {
        while self.is_slot_progressed(desc, self.first_non_progressed_slot) {
            self.first_non_progressed_slot += 1;
        }

        log::trace!(
            "SimplexState: advanced first_non_progressed_slot cursor to {}",
            self.first_non_progressed_slot
        );
    }

    /// Check if a slot has progressed (notarized or skipped or finalized)
    ///
    /// Used for `first_non_progressed_slot` cursor advancement.
    pub fn is_slot_progressed(&self, desc: &SessionDescription, slot: SlotIndex) -> bool {
        // Finalized slots are always progressed
        if slot < self.first_non_finalized_slot {
            return true;
        }

        self.get_slot_ref(desc, slot)
            .map(|s| s.skipped || s.observed_notar_certificate.is_some())
            .unwrap_or(false)
    }

    /// Find next non-skipped slot after a given slot
    ///
    /// Reference: C++ pool.cpp next_nonskipped_slot_after() uses skip_intervals_.lower_bound()
    ///
    /// This is a simplified linear scan (sufficient for correctness + tests).
    /// TODO: Implement a more efficient search algorithm.
    fn find_next_nonskipped_slot(
        &mut self,
        desc: &SessionDescription,
        slot: SlotIndex,
    ) -> SlotIndex {
        const MAX_SCAN: u32 = 10_000;

        let mut s = slot + 1;
        for _ in 0..MAX_SCAN {
            if !self.is_slot_skipped_cert(desc, s) {
                return s;
            }
            s += 1;
        }

        // Should never happen under normal operation
        log::error!(
            "SimplexState::find_next_nonskipped_slot: \
            exceeded scan limit (MAX_SCAN={MAX_SCAN}) from slot {slot} \
            (first_non_finalized={}, first_non_progressed_slot={}, slots_per_window={})",
            self.first_non_finalized_slot,
            self.first_non_progressed_slot,
            self.slots_per_leader_window
        );
        panic!("SimplexState::find_next_nonskipped_slot: exceeded scan limit from slot {}", slot);
    }

    /// Advance leader window when progress cursor crosses window boundary.
    ///
    /// Reference: C++ pool.cpp maybe_publish_new_leader_windows()
    ///
    /// This triggers timeout scheduling for the new window and applies adaptive backoff.
    /// Only called when `SimplexStateOptions::use_notarized_parent_chain` is enabled.
    ///
    /// # Ordering guarantee (C++ parity: PR #2195)
    ///
    /// `current_leader_window_idx` is updated here, inside `check_all()` ->
    /// notarization/skip handlers -> `advance_progress_cursor()` -> this method.
    /// `SessionProcessor::check_collation()` runs strictly after `check_all()`
    /// returns, so the leader-status check always sees the up-to-date window.
    /// This mirrors C++ consensus.cpp where `current_window_` is set BEFORE
    /// the leader check in the `LeaderWindowObserved` handler.
    fn advance_leader_window_on_progress_cursor(&mut self, desc: &SessionDescription) {
        let now_window = desc.get_window_idx(self.first_non_progressed_slot);
        if now_window <= self.current_leader_window_idx {
            log::trace!(
                "SimplexState::advance_leader_window_on_progress_cursor: not advancing window \
                (current={}, now_window={now_window}, first_non_progressed_slot={})",
                self.current_leader_window_idx,
                self.first_non_progressed_slot
            );
            return;
        }

        log::trace!(
            "SimplexState: first_non_progressed_slot {} crossed into window {now_window}, \
            advancing leader window",
            self.first_non_progressed_slot,
        );

        // C++ parity: read available_base from the progress cursor slot.
        // Reference: pool.cpp advance_present():
        //   ParentId base = {};
        //   if (now_ != 0) { base = slot_at(now_)->state->available_base.value(); }
        //   publish<LeaderWindowObserved>(now_, base);
        //
        // For genesis (slot 0), base is None (matches C++ ParentId{} = std::nullopt).
        // For later slots, base comes from the per-slot available_base propagated
        // by notarization/skip handlers.
        let base: CandidateParent = if self.first_non_progressed_slot.value() == 0 {
            None
        } else {
            let slot_base = self.get_slot_available_base(desc, self.first_non_progressed_slot);
            assert!(
                slot_base.is_some(),
                "SimplexState: notarized-parent chain invariant violated — \
                 base unknown for progress cursor slot {} (now_window={}). \
                 C++ CHECK(maybe_base.has_value()) in pool.cpp advance_present()",
                self.first_non_progressed_slot,
                now_window
            );
            slot_base.unwrap()
        };

        // Apply adaptive timeout backoff (reuse existing logic)
        self.apply_adaptive_timeout_backoff(
            desc,
            now_window,
            "SimplexState::advance_leader_window_on_progress_cursor",
        );

        // Advance to new window and schedule timeouts
        self.current_leader_window_idx = now_window;
        self.set_timeouts(desc);

        // C++ parity: populate new window's available_bases and first slot base.
        // In C++ this happens via LeaderWindowObserved -> consensus.cpp handler which
        // calls start_generation(event->base, ...). In Rust the FSM handles this
        // directly: the base is inserted into the window's available_bases set so
        // that check_collation() -> has_available_parent() sees it.
        self.ensure_window_exists(now_window);
        if let Some(window) = self.get_window_mut(now_window) {
            window.available_bases.insert(base.clone());
        }
        let first_slot = now_window.window_start(self.slots_per_leader_window);
        if let Some(slot) = self.get_slot_mut(desc, first_slot) {
            if slot.available_base.is_none() {
                slot.available_base = Some(base.clone());
            }
        }

        log::trace!(
            "SimplexState: advanced to window {}, base={}, scheduling timeouts from slot {}",
            now_window,
            Self::format_parent(base.as_ref()),
            self.skip_slot
        );
    }

    /*
        ========================================================================
        Debug and Diagnostics
        ========================================================================
    */

    /// Dump FSM state for debugging
    ///
    /// # Arguments
    /// * `desc` - Session description for weight calculations
    /// * `full_dump` - If false, returns compact one-line summary for trace logs.
    ///                 If true, returns detailed multi-line dump for debug dumps.
    ///
    /// Format inspired by validator-session session_processor debug_dump.
    pub fn debug_dump(&self, desc: &SessionDescription, full_dump: bool) -> String {
        let total_weight = desc.get_total_weight();
        let threshold_66 = desc.get_threshold_66();
        let threshold_33 = desc.get_threshold_33();

        // Get current slot info
        let current_slot = self.first_non_finalized_slot;
        let current_window_idx = self.current_leader_window_idx;

        // Get current slot state flags
        let (slot_flags, voted_notar_short, notar_cert_short) = self
            .get_window(current_window_idx)
            .and_then(|w| {
                let offset = (current_slot % self.slots_per_leader_window) as usize;
                if offset < w.slots.len() {
                    let slot = &w.slots[offset];
                    let mut flags = Vec::new();
                    if slot.is_voted {
                        flags.push("V");
                    }
                    if slot.is_bad_window {
                        flags.push("Bad");
                    }
                    if slot.voted_skip {
                        flags.push("Skip");
                    }
                    if slot.its_over {
                        flags.push("Over");
                    }
                    if slot.pending_block.is_some() {
                        flags.push("Pend");
                    }
                    let flags_str =
                        if flags.is_empty() { "-".to_string() } else { flags.join("|") };

                    let voted = slot
                        .voted_notar
                        .as_ref()
                        .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..6]))
                        .unwrap_or_else(|| "-".to_string());
                    let cert = slot
                        .observed_notar_certificate
                        .as_ref()
                        .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..6]))
                        .unwrap_or_else(|| "-".to_string());

                    Some((flags_str, voted, cert))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ("-".to_string(), "-".to_string(), "-".to_string()));

        // Get current slot vote weights
        let (notar_weight, skip_weight, final_weight, notar_or_skip, skip_or_fb) = self
            .slot_votes
            .get(&current_slot)
            .map(|sv| {
                let max_notar = sv.notarize_weight_by_block.values().max().copied().unwrap_or(0);
                let max_final = sv.finalize_weight_by_block.values().max().copied().unwrap_or(0);
                (
                    max_notar,
                    sv.skip_or_skip_fallback_weight,
                    max_final,
                    sv.notarize_or_skip_weight,
                    sv.skip_or_skip_fallback_weight,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0));

        // Get available bases for current window (formatted list)
        let bases_list: String = self
            .get_window(current_window_idx)
            .map(|w| {
                if w.available_bases.is_empty() {
                    "none".to_string()
                } else {
                    w.available_bases
                        .iter()
                        .map(|b| Self::format_parent(b.as_ref()))
                        .collect::<Vec<_>>()
                        .join(",")
                }
            })
            .unwrap_or_else(|| "none".to_string());

        // Format events list
        let events_list: String = if self.events.is_empty() {
            "none".to_string()
        } else {
            self.events.iter().map(Self::format_event).collect::<Vec<_>>().join(",")
        };

        // Format percentage helper
        let pct = |w: u64| -> f64 { 100.0 * w as f64 / total_weight as f64 };

        if !full_dump {
            // Compact one-line format for trace logs
            format!(
                "SimplexState: {current_window_idx}/{current_slot} \
                first_non_finalized={} first_non_progressed={} flags=[{slot_flags}] \
                notar={}({:.0}%) skip={}({:.0}%) final={}({:.0}%) n|s={}({:.0}%) \
                s|fb={}({:.0}%) th66/33={}({:.0}%)/{}({:.0}%) bases=[{bases_list}] \
                voted={voted_notar_short} cert={notar_cert_short} evts=[{events_list}]",
                self.first_non_finalized_slot,
                self.first_non_progressed_slot,
                notar_weight,
                pct(notar_weight),
                skip_weight,
                pct(skip_weight),
                final_weight,
                pct(final_weight),
                notar_or_skip,
                pct(notar_or_skip),
                skip_or_fb,
                pct(skip_or_fb),
                threshold_66,
                pct(threshold_66),
                threshold_33,
                pct(threshold_33)
            )
        } else {
            // Full multi-line format for debug dumps
            let mut result = String::new();

            // Header with same info as one-line
            result.push_str(&format!(
                "SimplexState dump:\n  - current: {}/{}, flags=[{}]\n",
                current_window_idx, current_slot, slot_flags
            ));
            result.push_str(&format!(
                "  - validators: {}, local_idx: {}\n",
                self.num_validators,
                desc.get_self_idx()
            ));

            // Thresholds
            result.push_str(&format!(
                "  - thresholds: total_weight={}, th66={}({:.1}%), th33={}({:.1}%)\n",
                total_weight,
                threshold_66,
                pct(threshold_66),
                threshold_33,
                pct(threshold_33)
            ));

            // Current slot weights
            result.push_str(&format!(
                "  - {current_slot} weights: notar={notar_weight}({:.1}%), \
                skip={skip_weight}({:.1}%), final={final_weight}({:.1}%), \
                n|s={notar_or_skip}({:.1}%), s|fb={skip_or_fb}({:.1}%)\n",
                pct(notar_weight),
                pct(skip_weight),
                pct(final_weight),
                pct(notar_or_skip),
                pct(skip_or_fb)
            ));

            // State info
            result.push_str(&format!(
                "  - first_non_finalized: {}, first_non_progressed: {}, \
                skip_slot: {}, pending_events: {}\n",
                self.first_non_finalized_slot,
                self.first_non_progressed_slot,
                self.skip_slot,
                self.events.len()
            ));
            result.push_str(&format!(
                "  - timeouts: first_block={:?}, target_rate={:?}\n",
                self.first_block_timeout, self.target_rate_timeout
            ));

            // Leader windows
            result.push_str("  - leader_windows:\n");
            for window in &self.leader_windows {
                // Format available bases
                let bases: Vec<String> = window
                    .available_bases
                    .iter()
                    .map(|b| match b {
                        None => "genesis".to_string(),
                        Some(p) => format!("{}:{}", p.slot, &p.hash.to_hex_string()[..6]),
                    })
                    .collect();
                let bases_str = if bases.is_empty() { "none".to_string() } else { bases.join(",") };

                result.push_str(&format!(
                    "    - {} ({}..{}): timeouts={}, bases=[{}]\n",
                    window.window_idx,
                    window.start_slot,
                    window.start_slot + self.slots_per_leader_window - 1,
                    window.had_timeouts,
                    bases_str
                ));

                for (i, slot) in window.slots.iter().enumerate() {
                    let slot_id = window.start_slot + i as u32;

                    // Build flags string
                    let mut flags = Vec::new();
                    if slot.is_voted {
                        flags.push("Voted");
                    }
                    if slot.is_bad_window {
                        flags.push("BadWindow");
                    }
                    if slot.voted_skip {
                        flags.push("VotedSkip");
                    }
                    if slot.its_over {
                        flags.push("ItsOver");
                    }
                    if slot.pending_block.is_some() {
                        flags.push("Pending");
                    }
                    let flags_str =
                        if flags.is_empty() { "none".to_string() } else { flags.join("|") };

                    let voted_notar = slot
                        .voted_notar
                        .as_ref()
                        .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..8]))
                        .unwrap_or_else(|| "-".to_string());
                    let notar_cert = slot
                        .observed_notar_certificate
                        .as_ref()
                        .map(|c| format!("{}:{}", c.slot, &c.hash.to_hex_string()[..8]))
                        .unwrap_or_else(|| "-".to_string());

                    result.push_str(&format!(
                        "      {}: flags=[{}], voted_notar={}, notar_cert={}\n",
                        slot_id, flags_str, voted_notar, notar_cert
                    ));
                }
            }

            // Vote accounting for recent slots
            result.push_str("  - slot_votes:\n");
            let mut slots: Vec<_> = self.slot_votes.keys().copied().collect();
            slots.sort();
            for slot_id in slots.iter().rev().take(5) {
                if let Some(sv) = self.slot_votes.get(slot_id) {
                    // Build published flags
                    let mut pub_flags = Vec::new();
                    if sv.block_notarized_published {
                        pub_flags.push("Notarized");
                    }
                    if sv.safe_to_skip_published {
                        pub_flags.push("SafeToSkip");
                    }
                    if sv.block_finalized_published {
                        pub_flags.push("Finalized");
                    }
                    if sv.slot_skipped_published {
                        pub_flags.push("Skipped");
                    }
                    let pub_flags_str =
                        if pub_flags.is_empty() { "none".to_string() } else { pub_flags.join("|") };

                    result.push_str(&format!(
                        "    - s{}: n|s={}({:.1}%), s|fb={}({:.1}%), published=[{}]\n",
                        slot_id,
                        sv.notarize_or_skip_weight,
                        pct(sv.notarize_or_skip_weight),
                        sv.skip_or_skip_fallback_weight,
                        pct(sv.skip_or_skip_fallback_weight),
                        pub_flags_str
                    ));

                    for (block, weight) in &sv.notarize_weight_by_block {
                        let fin_weight =
                            sv.finalize_weight_by_block.get(block).copied().unwrap_or(0);
                        result.push_str(&format!(
                            "      block {}: notar={}({:.1}%), final={}({:.1}%)\n",
                            &block.to_hex_string()[..12],
                            weight,
                            pct(*weight),
                            fin_weight,
                            pct(fin_weight)
                        ));
                    }
                }
            }

            result
        }
    }

    /// Produce C++-style standstill slot-grid dump.
    ///
    /// For each slot in the tracked range [begin, end), outputs one line:
    ///   `<slot>: <per-validator markers> [cert flags]`
    ///
    /// Per-validator markers (one character per validator):
    ///   F = finalize vote, I = notarize+skip, N = notarize, S = skip, . = none
    ///
    /// Cert flags: `notar`, `skip`, `final` (when the corresponding certificate exists).
    ///
    /// Reference: C++ pool.cpp alarm() slot-grid output
    pub fn standstill_slot_grid_dump(&self, desc: &SessionDescription) -> String {
        let (begin, end) = self.get_tracked_slots_interval();
        let num_validators = desc.get_total_nodes();
        let mut sb = String::new();

        for slot_num in begin..end {
            let slot_idx = SlotIndex::from(slot_num);
            sb.push_str(&format!("{}: ", slot_num));

            if let Some(sv) = self.slot_votes.get(&slot_idx) {
                for j in 0..num_validators {
                    let vv = &sv.votes[j];
                    let has_skip = vv.skip.is_some() || vv.fallback_skip.is_some();
                    if vv.finalize.is_some() {
                        sb.push('F');
                    } else if vv.notarize.is_some() && has_skip {
                        sb.push('I');
                    } else if vv.notarize.is_some() {
                        sb.push('N');
                    } else if has_skip {
                        sb.push('S');
                    } else {
                        sb.push('.');
                    }
                }

                if sv.notarize_certificate.is_some() {
                    sb.push_str(" notar");
                }
                if sv.skip_certificate.is_some() {
                    sb.push_str(" skip");
                }
                if sv.finalize_certificate.is_some() {
                    sb.push_str(" final");
                }
            } else {
                for _ in 0..num_validators {
                    sb.push('.');
                }
            }

            sb.push('\n');
        }

        sb
    }
}

/*
    ============================================================================
    Tests
    ============================================================================

    Tests are in a separate file but included directly to access private internals.
*/

#[cfg(test)]
#[path = "tests/test_simplex_state.rs"]
mod tests;
