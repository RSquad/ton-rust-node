/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `CollationController` entity
//!
//! Per-session collation-phase controller. Owns the in-memory state that
//! drives the precollation pipeline, the synchronous generated-parent
//! metadata caches, the empty-block recovery policy, the precollation
//! pipeline reset helpers, parent / candidate lookup, the leader-gate,
//! request allocation, and the collation pacing helper. It also owns the
//! full collation pipeline (`check_collation`, `invoke_collation`,
//! `execute_collation_attempt`, `dispatch_collation_request`,
//! `on_collation_complete`, `on_collation_failed_impl`, `generated_block`,
//! `precollate_block`, `prepare_collation`): these read `SessionProcessor`
//! state and apply effects only through the [`CollationBackend`] seam, and
//! bounce re-entrant work (genuine-error restarts via `restart_collation`,
//! the next-slot self-loop) back onto `SessionProcessor` through the
//! controller task queue rather than recursing synchronously.
//!
//! Self-collation observability (the in-flight start / pending-acceptance
//! maps and the generated-candidate validation watch) is NOT owned here:
//! it lives entirely in [`SessionTelemetry`](crate::session_telemetry) as
//! an interior-mutable funnel, driven by `SessionProcessor`.
//!
//! ## Owned state
//!
//! - `precollated_blocks` / `precollated_blocks_next_request_id` /
//!   `precollated_blocks_max_slot` — the in-flight precollation pipeline.
//! - `earliest_collation_time: Option<SystemTime>` — collation rate gate
//!   (C++ `block-producer.cpp coro_sleep(target_time)` parity).
//! - `local_chain_head: Option<LocalChainHead>` — synchronous window-local
//!   chain head, populated in `generated_block()` so `precollate_block()`
//!   can chain the next slot without waiting for the async self-loop.
//! - `last_generated_slot: Option<SlotIndex>` — collation invariant tracker
//!   (slot sequence monotonicity for generation).
//! - `generated_parent_cache: HashMap<RawCandidateId, BlockIdExt>` and
//!   `generated_parent_gen_utime_ms_cache: HashMap<RawCandidateId, u64>`
//!   — synchronous caches of locally-generated parent metadata, populated
//!   in `generated_block()` before the async self-loop so the next slot
//!   can chain immediately. Owned together because they are written and
//!   cleared as a pair (see `clear_generated_parent_caches`).
//!
//! ## Owned methods
//!
//! - `resolve_parent_gen_utime_ms(parent, &CandidateBook)` — three-way
//!   lookup across the synchronous cache, `CandidateBook`, and the
//!   window-local chain head; mirrors the C++ block-producer.cpp parent
//!   timestamp resolution.
//! - `should_generate_empty_block(...)` — empty-block recovery policy,
//!   pure over `parent_before_split`, current seqno, and finalization
//!   cursors supplied by `SessionProcessor`.
//! - `invalidate_local_chain_head()` — clears the window-local
//!   chain head and both generated-parent caches; called when the leader
//!   window changes, the progress cursor jumps, or a consensus event
//!   invalidates the locally generated parent chain.
//! - `cancel_pending_precollations() -> Vec<SlotIndex>` —
//!   cancels every in-flight precollation request, drains the
//!   precollation map and high-water-mark cursor, and returns the list
//!   of cancelled slots so the caller can drive
//!   `forget_self_collation_tracking` (telemetry) for each one.
//! - `remove_precollated_with_log(slot) -> bool` — removes
//!   a single precollation entry with the standard trace log; returns
//!   whether anything was actually removed so the caller can update
//!   `precollation_results_counter` without inspecting the inner map.
//! - `resolve_parent_block_id(parent, &CandidateBook) -> Option<BlockIdExt>`
//!   — composite parent lookup over the synchronous generated-parent cache
//!   and the candidate book.
//! - `try_begin_collation_slot(slot) -> Option<ValidatorIndex>` —
//!   gate keeping the local collation entry point: returns the local
//!   validator index iff no precollation entry already exists for `slot`
//!   and the local node is the leader for `slot`.
//! - `create_pending_collation_request(slot, parent, min_gen_time) ->
//!   (u32, Arc<AsyncRequestImpl>)` — allocate a new precollation
//!   request and register it under `slot`.
//! - `update_collation_pacing()` — advance `earliest_collation_time`
//!   by `target_rate` from the held description clock; mirrors C++
//!   `block-producer.cpp target_time += target_rate_ms`.
//! - The collation pipeline proper — `check_collation`, `invoke_collation` /
//!   `restart_collation`, `execute_collation_attempt`,
//!   `dispatch_collation_request`, `on_collation_complete` /
//!   `on_collation_failed_impl`, `generated_block`, and `precollate_block` —
//!   ports the original `SessionProcessor` method bodies, reading FSM and
//!   finalization state and applying effects through the [`CollationBackend`]
//!   seam. Re-entrant steps (genuine-error restart, the next-slot self-loop)
//!   enqueue a closure on the controller task queue so `SessionProcessor`
//!   drains them without synchronous recursion.
//!
//! ## Collation flow
//!
//! ```text
//! 1. check_collation()
//!    ├── Am I leader for current slot? (description.get_leader(slot))
//!    ├── Have I already generated? (pending_generate || generated)
//!    └── Check for precollated block first
//!
//! 2. If no precollated block:
//!    ├── pending_generate = true
//!    └── invoke_collation(slot) → notify_generate_slot(source_info, request, cb)
//!
//! 3. Callback receives candidate from higher layer:
//!    └── on_collation_complete(slot, request_id, candidate)
//!
//! 4. generated_block():
//!    ├── Validate sizes, compute candidate hash
//!    ├── Sign candidate: utils::sign_candidate()
//!    ├── Broadcast via receiver.send_block_broadcast()
//!    ├── Convert to Candidate for FSM
//!    └── simplex_state.on_candidate(&desc, candidate)
//! ```
//!
//! ## Precollation pipeline (C++ candidate-chaining parity)
//!
//! Reference: C++ `block-producer.cpp generate_candidates()` loop. The
//! precollation pipeline chains candidates across slots within a single leader
//! window. After `generated_block()` completes slot N it updates
//! `local_chain_head` and calls `precollate_block(N+1)`, which uses the local
//! chain head as the explicit parent instead of waiting for FSM `available_base`
//! propagation via notarization. The pipeline is flushed by `reset()` on session
//! stop, on a leader-window change (via `invalidate_local_chain_head()`), or when
//! the progress cursor jumps past queued slots.
//!
//! ```text
//! 1. check_collation() — first slot in window:
//!    ├── Use FSM available_base as parent
//!    └── invoke_collation(slot, parent)
//!
//! 2. generated_block() — after broadcast:
//!    ├── Update local_chain_head + generated_parent_cache
//!    └── precollate_block(slot + 1) — chain next slot
//!
//! 3. precollate_block(slot):
//!    ├── Prefer local_chain_head for parent (same window)
//!    ├── Fall back to FSM available_base
//!    └── invoke_collation(slot, parent)
//!
//! 4. check_collation() finds precollated block:
//!    ├── Use precollated candidate directly
//!    └── Remove from pipeline, start next precollation
//!
//! 5. Window change / progress jump:
//!    └── invalidate_local_chain_head() + reset()
//! ```
//!
//! ## Backend seam
//!
//! The reads and effects the collation pipeline needs from `SessionProcessor`
//! are reached through the borrowing [`CollationBackend`] trait, built fresh
//! per call by `session_processor`'s `with_collation_backend` split-borrow:
//! FSM / leader-window reads (`first_non_progressed_slot`,
//! `current_leader_window_idx`, the available-parent and candidate-book
//! lookups, `before_split_flag`), the finalization scalars
//! (`finalized_head_before_split` / `finalized_head_seqno`,
//! `last_consensus_finalized_seqno`, `last_mc_finalized_seqno`,
//! `session_start_prev_blocks`), the generate-slot flags (`is_generated` /
//! `is_pending_generate` and their `set_*` writers), and the side effects
//! (`request_wake_at`, `forget_self_collation_tracking`, `broadcast_candidate`,
//! `request_parent_candidate`, `self_receive_candidate`,
//! `persist_candidate_info`). This mirrors
//! [`ValidationBackend`](crate::validation_controller::ValidationBackend) and
//! keeps the controller (and its tests) free of any `SessionProcessor`
//! dependency.
//!
//! ## Boundary
//!
//! `CollationController` owns collation-phase state and decisions only. It
//! does NOT drive validation, run the FSM, or send votes / certs. Methods
//! return phase outcomes (e.g. `CollationResult`) and call narrow
//! `SessionProcessor` methods for FSM submission and network ordering; the
//! controller never reaches back into other controllers directly.
//!
//! Some entry points need `&mut SessionRuntime` for slot/wake scheduling;
//! these are passed explicitly per call so the controller stays
//! independently constructible in tests.
//!
//! Callers (`SessionProcessor`, the receiver-side generate-slot callback)
//! hold an inline `CollationController` on `SessionProcessor` and reach it
//! via the accessor surface; tests included via `#[path]` reach the same
//! accessor surface and never inspect raw fields.

use crate::{
    block::{
        CandidateId as BlockCandidateId, CandidateParentInfo, RawCandidateId, SlotIndex,
        ValidatorIndex, WindowIndex,
    },
    candidate_book::CandidateBook,
    controller_queue::{Controlled, ControllerQueueExt, ControllerQueuePtr},
    session_callbacks::SessionCallbacks,
    session_description::SessionDescription,
    session_telemetry::SessionTelemetry,
    trace_collector::TraceCollector,
    utils::AsyncRequestImpl,
    SessionId, ValidatorBlockCandidatePtr,
};
use consensus_common::{check_execution_time, instrument, AsyncRequest, CollationParentHint};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{
    ton::{
        consensus::{
            candidatedata::{Block as CandidateDataBlock, Empty as CandidateDataEmpty},
            candidateid::CandidateId,
            candidateparent::CandidateParent,
            CandidateData, CandidateParent as CandidateParentBoxed,
        },
        validator_session::candidate::CompressedCandidate,
    },
    IntoBoxed,
};
use ton_block::{error, fail, sha256_digest, BlockIdExt, BocFlags, Error, Result, UInt256};

/// Warn threshold for self-collation generation latency. Moved verbatim from
/// `SessionProcessor`; read only by [`CollationController::make_collation_callback`].
const MAX_GENERATION_TIME: Duration = Duration::from_millis(1000);

/// Backoff before a single genuine-error collation restart, mirroring the C++
/// producer's `coro_sleep(td::Timestamp::in(0.1))` in `block-producer.cpp`. There
/// is no attempt counter: a restart reuses the SAME absolute window-end deadline,
/// so retries race the same budget and the window-end hard stop bounds them.
const COLLATION_ERROR_RESTART_BACKOFF: Duration = Duration::from_millis(100);

/*
    Precollated block
    Reference: validator-session/src/session_processor.rs PrecollatedBlock
*/

/// Precollated block - stores pending or completed collation result
///
/// Parent is captured at collation start to avoid races between collation
/// and consensus events (e.g., notarization advancing the parent chain).
///
/// Reference: C++ block-producer.cpp locks parent in the collation loop.
pub(crate) struct PrecollatedBlock {
    /// Request for tracking/cancellation
    pub(crate) request: Arc<AsyncRequestImpl>,
    /// Completed collation result - None if still pending
    pub(crate) result: Option<CollationResult>,
    /// Parent captured at collation start (avoids race vs consensus events)
    ///
    /// This is the parent that was available when collation was initiated.
    /// We use this in `generated_block()` instead of recomputing from FSM state,
    /// ensuring the candidate is signed with the same parent that was assumed.
    pub(crate) parent: Option<crate::block::CandidateParentInfo>,
}

/// Map of slot -> precollated block
pub(crate) type PrecollatedBlockMap = HashMap<SlotIndex, PrecollatedBlock>;

/// Window-local chain head for candidate chaining (C++ block-producer.cpp parity).
///
/// In C++, `generate_candidates()` carries mutable `parent` and `state` across slots
/// within a single leader window, so slot N+1 chains off slot N's locally generated
/// candidate without waiting for notarization. This struct tracks the same chain head
/// on the Rust side: after `generated_block()` completes for a slot, we record the
/// produced candidate's identity here so that `precollate_block()` for the next slot
/// in the same window can use it immediately as an explicit parent.
///
/// Reset when the leader window changes or the progress cursor jumps.
#[derive(Clone, Debug)]
pub(crate) struct LocalChainHead {
    /// Window this chain head belongs to
    pub(crate) window: WindowIndex,
    /// Slot of the last locally generated candidate
    pub(crate) slot: SlotIndex,
    /// Candidate parent info (slot + candidate-id hash) for the next slot
    pub(crate) parent_info: crate::block::CandidateParentInfo,
    /// Exact generation time extracted from ConsensusExtraData, if available.
    pub(crate) gen_utime_ms: Option<u64>,
}

/*
    Collation result

    Represents the outcome of a collation request - either a normal block
    with transactions or an empty block for finalization recovery.
*/

/// Result of block collation
///
/// Used to differentiate between normal blocks (with transactions) and
/// empty blocks (for finalization recovery when consensus gets ahead).
///
/// Reference: C++ block-producer.cpp generate_candidates() loop
#[derive(Clone)]
pub(crate) enum CollationResult {
    /// Normal block with candidate data from collator
    Block(ValidatorBlockCandidatePtr),

    /// Empty block for finalization recovery
    ///
    /// When consensus gets ahead of blockchain finalization, we generate
    /// an "empty" block that references the parent's BlockIdExt instead of
    /// collating new transactions. This helps the previous block get finalized.
    ///
    /// Contains the parent's BlockIdExt to inherit.
    Empty {
        /// Parent block identifier (empty block inherits this)
        parent_block_id: BlockIdExt,
    },
}

/// Generated block descriptor for broadcast and FSM submission.
///
/// Contains all computed data needed after validation, signing, and TL
/// construction. Produced by [`CollationController::create_normal_block_desc`]
/// and [`CollationController::create_empty_block_desc`] and consumed by
/// `generated_block`. Moved verbatim from `SessionProcessor`.
pub(crate) struct GeneratedBlockDesc {
    /// Block identifier (for FSM CandidateId)
    pub(crate) block_id_ext: BlockIdExt,
    /// Block candidate for FSM (None for empty blocks)
    pub(crate) block_candidate: Option<crate::block::BlockCandidate>,
    /// Candidate hash (used in FSM CandidateId)
    pub(crate) candidate_hash: UInt256,
    /// TL candidate data for network broadcast
    pub(crate) tl_candidate_data: CandidateData,
    /// Signature for FSM Candidate
    pub(crate) signature: Vec<u8>,
    /// Exact generation time extracted from ConsensusExtraData, if available.
    pub(crate) gen_utime_ms: Option<u64>,
}

/// A collation attempt prepared by [`CollationController::prepare_collation`]:
/// the resolved prev-block chain, dispatch/min-gen [`CollationTiming`], the
/// derived seqno, and whether this is the first block of the session (genesis).
pub(crate) struct PreparedCollation {
    pub(crate) prev_block_ids: Vec<BlockIdExt>,
    pub(crate) timing: CollationTiming,
    pub(crate) new_seqno: u32,
    pub(crate) is_first_session_block: bool,
}

/// Dispatch / generation timing for a collation attempt
/// (see [`CollationController::compute_collation_timing`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CollationTiming {
    /// Wall-clock time when Rust should dispatch the collation request.
    ///
    /// C++ shardchain parity: `slot_start - target_rate`.
    pub(crate) dispatch_time: SystemTime,
    /// Minimum block generation time passed to the validator/collator layer.
    ///
    /// C++ keeps this as `slot_start` even when shard collation starts early.
    pub(crate) min_gen_time: SystemTime,
    pub(crate) start_collate_before: Duration,
    pub(crate) parent_gen_utime_ms: Option<u64>,
}

/// Absolute per-slot SOFT cutoff and window-end HARD cap for a real collation.
///
/// Mirrors the C++ producer in `block-producer.cpp`: the soft cutoff is
/// `slot_start + target_rate` for the masterchain and `slot_start` for shardchains;
/// the collator bounds message intake to it (further clamped by its static
/// `cutoff_timeout_ms`). The hard cap is bounded to the leader-window end rather
/// than C++'s `slot_start + max(3*target_rate, 60s)`: a real collation may outlive
/// its dispatch slot (the per-slot wake covers the elapsed slots with empty fillers
/// and re-tags the late real when it completes) but must not outlive its leader
/// window, which the next leader's producer owns. `min_gen_time` is the slot start
/// (C++ keeps it at `slot_start` even when shard collation dispatches early); `slot`
/// locates it within its window.
pub(crate) fn collation_deadlines(
    slot: SlotIndex,
    min_gen_time: SystemTime,
    target_rate: Duration,
    slots_per_leader_window: u32,
    is_masterchain: bool,
) -> (SystemTime, SystemTime) {
    let soft_deadline = if is_masterchain {
        min_gen_time.checked_add(target_rate).unwrap_or(min_gen_time)
    } else {
        min_gen_time
    };

    let spw = slots_per_leader_window.max(1);
    // Slots from this slot's start to the leader-window end: `1..=spw`.
    let slots_to_window_end = spw - slot.offset_in_window(spw);
    let hard_deadline = min_gen_time
        .checked_add(target_rate.saturating_mul(slots_to_window_end))
        .unwrap_or(min_gen_time);

    (soft_deadline, hard_deadline)
}

/// Outcome of [`CollationController::prepare_collation`]: ready to dispatch,
/// deferred until the carried dispatch time, or blocked on an unresolved parent.
pub(crate) enum CollationPreparation {
    Ready(PreparedCollation),
    Deferred(SystemTime),
    WaitingForParent,
}

/// Absolute deadline context for a single real collation attempt: the soft / hard
/// cutoffs handed to the collator plus the budget anchor the collator measures its
/// percentage sub-budgets from. Computed once when a slot is first dispatched and
/// then PINNED across a genuine-error restart so every attempt for the slot races the
/// SAME window-end budget (rather than each restart recomputing a fresh window-length
/// budget from the advanced clock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollationDeadlineContext {
    /// Absolute SOFT cutoff (message-intake) — C++ `soft_timeout`.
    pub(crate) soft_deadline: SystemTime,
    /// Absolute window-end HARD cap — C++ `hard_timeout` (bounded to the leader window).
    pub(crate) hard_deadline: SystemTime,
    /// Absolute start of the budget window (the dispatch instant); the collator's soft
    /// sub-budgets span `soft_deadline - budget_anchor`.
    pub(crate) budget_anchor: SystemTime,
}

/// Bookkeeping for the single in-flight REAL collation, mirroring the C++ block
/// producer's `block_generation_active` + `block_generation` locals in
/// `block-producer.cpp`. Held in [`CollationController::block_generation_active`]
/// from real dispatch until the collation completes, fails, or the pipeline is
/// `reset()`. It lets one collation stay alive across slots while the per-slot wake
/// covers the elapsed slots with empty fillers and re-tags the late real.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RealCollationState {
    /// Leader window the collation was dispatched in; a window change drops it via
    /// `reset()`.
    window: WindowIndex,
    /// Slot the collation was dispatched for.
    slot_dispatched: SlotIndex,
    /// `AsyncRequest` id of the in-flight collation, so that completion/failure of a
    /// different request (e.g. an empty filler) does not clear this marker.
    request_id: u32,
    /// Next per-slot filler horizon, mirroring the C++
    /// `await_with_timeout(slot_start + target_rate)`: when it elapses while the
    /// collation is still running, the per-slot wake emits an empty filler and
    /// re-arms the next one. The live horizon is carried by the scheduled wake, so
    /// this stored copy is bookkeeping only asserted by the unit tests.
    #[allow(dead_code)]
    next_slot_deadline: SystemTime,
    /// Absolute soft/hard cutoffs and budget anchor handed to the collator. Read on a
    /// genuine-error failure so the restart can reuse the SAME budget instead of
    /// recomputing it from the (now advanced) clock.
    deadlines: CollationDeadlineContext,
}

/// Session-owned reads a deferred collation re-entry needs but cannot reach
/// from `&mut CollationController` alone.
///
/// Mirrors [`ValidationBackend`](crate::validation_controller::ValidationBackend):
/// a *borrowing*, drain-scoped view built fresh from `&mut SessionProcessor` by
/// the composition root (`session_processor`'s `with_collation_backend`)
/// immediately before a collation re-entry runs, then dropped (RAII). Because
/// it is rebuilt per call, every read is current — no captured snapshot, no
/// shared mutable handle. The controller's own precollation state lives on
/// `&mut Self`; everything genuinely owned by `SessionProcessor` is reached
/// only through this trait, keeping the controller (and its unit tests) free of
/// any `SessionProcessor` dependency.
///
/// Handed to the controller by `&mut` (per the [`ControllerQueue`] seam's
/// backend convention) so re-entries can drive disjoint `SessionProcessor`
/// *effects*. Two flavours of effect live below:
/// - *Synchronous* effects the adapter performs in place because the underlying
///   call does not need `&mut SessionProcessor`: `broadcast_candidate` (forwards
///   to the `&self` `Receiver::send_block_broadcast`) and `persist_candidate_info`
///   (the lone `&mut self` method — writes through the `&mut`-borrowed database).
///   Keeping these synchronous preserves the C++-parity publication ordering
///   (DB write started, then immediate broadcast).
/// - *Deferred* effects the adapter bounces onto the session main loop because
///   they re-enter heavy `&mut SessionProcessor` machinery:
///   `request_parent_candidate` and `self_receive_candidate`. Collation retries
///   and the next-slot self-loop are likewise deferred, but through the
///   controller task queue rather than a backend method.
///
/// Kept deliberately narrow: it exposes only what the collation pipeline and
/// the generated-candidate publication path need today (the FSM / leader-window
/// reads, the `check_collation` entry guards, the pipeline-fill idle-slot
/// reads, and the broadcast / DB-persist / parent-request / self-receive
/// effects). Reads
/// available from handles the controller already holds (session id / options /
/// shard / leader / clock via `description`) are NOT duplicated here.
///
/// [`ControllerQueue`]: crate::controller_queue::ControllerQueue
pub(crate) trait CollationBackend {
    /// Lowest slot the FSM has NOT yet progressed past (notarized or skipped).
    ///
    /// Collation follows notarized/skipped progress, not finalization, so the
    /// retry gate drops attempts for slots `< first_non_progressed_slot`.
    /// `SessionProcessor`-owned (the `SimplexState` progress cursor).
    fn first_non_progressed_slot(&self) -> SlotIndex;

    /// Current leader window index (the FSM's `current_leader_window_idx`).
    ///
    /// `SessionProcessor`-owned. Used by the stale-window guards: a progress
    /// cursor still pointing into a window older than this must not collate,
    /// and precollations from an older window must be reset.
    fn current_leader_window_idx(&self) -> WindowIndex;

    /// Does the FSM have an available parent (notarized/skip base) for `slot`?
    ///
    /// `SessionProcessor`-owned (`SimplexState::has_available_parent`). Used by
    /// the precollation planner to fall back to the FSM base when no window-local
    /// chain head directly precedes the target slot.
    fn has_available_parent(&self, slot: SlotIndex) -> bool;

    /// The FSM's available parent (notarized/skip base) for `slot`, if any.
    ///
    /// `SessionProcessor`-owned (`SimplexState::get_available_parent`). The
    /// precollation planner uses this as the parent when the window-local chain
    /// head does not directly precede the target slot.
    fn get_available_parent(&self, slot: SlotIndex) -> Option<CandidateParentInfo>;

    /// Resolved `BlockIdExt` of a received candidate `id`, if the `CandidateBook`
    /// knows it.
    ///
    /// `SessionProcessor`-owned (`CandidateBook::received(...).block_id`). The
    /// "book seam": the parent-resolution helpers
    /// ([`CollationController::resolve_parent_block_id`]) check the controller's
    /// own synchronous generated-parent cache first and fall back to this read,
    /// so the book never has to cross the seam as a borrowed collection.
    fn book_received_block_id(&self, id: &RawCandidateId) -> Option<BlockIdExt>;

    /// Generation timestamp (ms) recorded for received candidate `id`, if the
    /// `CandidateBook` knows it.
    ///
    /// `SessionProcessor`-owned (`CandidateBook::received(...).gen_utime_ms`).
    /// Book-seam companion to [`Self::book_received_block_id`], used by
    /// [`CollationController::resolve_parent_gen_utime_ms`] for collation timing.
    fn book_received_gen_utime_ms(&self, id: &RawCandidateId) -> Option<u64>;

    /// Before-split flag recorded for the block identified by `parent_block_id`,
    /// if known.
    ///
    /// `SessionProcessor`-owned: reads `before_split_by_block_id`, falling back
    /// to the finalized-head before-split bit when `parent_block_id` is exactly
    /// the finalized head block id. Backs
    /// [`CollationController::resolve_parent_before_split_flag`], the empty-block
    /// policy input that selects whether a slot may collate an empty block.
    fn before_split_flag(&self, parent_block_id: &BlockIdExt) -> Option<bool>;

    /// Lower the main-loop wake horizon to `at` (a no-op if it is already
    /// earlier — `SessionRuntime::set_next_awake_time` only lowers).
    ///
    /// `SessionProcessor`-owned. Used by the pacing gate to defer the next
    /// collation start until the rate limit elapses. The collation analogue of
    /// [`ValidationBackend::request_wake`](crate::validation_controller::ValidationBackend::request_wake),
    /// which targets "now".
    fn request_wake_at(&self, at: SystemTime);

    /// True once a block has been generated for `slot` (the
    /// `SessionRuntime` slot-generation tracker).
    ///
    /// `SessionProcessor`-owned. Used by the pipeline-fill policy to avoid
    /// re-precollating a slot already produced.
    fn is_generated(&self, slot: SlotIndex) -> bool;

    /// True while generation for `slot` is in flight (`SessionRuntime`
    /// pending-generate tracker).
    ///
    /// `SessionProcessor`-owned. Companion to [`Self::is_generated`] for the
    /// pipeline-fill policy: a slot already pending must not be precollated again.
    fn is_pending_generate(&self, slot: SlotIndex) -> bool;

    /// Mark slot generation as in-flight / cleared (`SessionRuntime`
    /// pending-generate tracker).
    ///
    /// `SessionProcessor`-owned (`SessionRuntime::set_pending_generate`, a `&mut`
    /// call — hence the `&mut self`). Set `true` when `check_collation` claims a
    /// slot and cleared (`false`) when an attempt defers / waits for its parent,
    /// or once the block is generated. The session clock is read inside the
    /// adapter, so callers thread no timestamp.
    fn set_pending_generate(&mut self, slot: SlotIndex, value: bool);

    /// Mark slot generation as complete (`SessionRuntime` generated tracker).
    ///
    /// `SessionProcessor`-owned (`SessionRuntime::set_generated`, `&mut`). Set
    /// once `generated_block` finishes the publication path for `slot`.
    fn set_generated(&mut self, slot: SlotIndex, value: bool);

    /// Mark the generated candidate for `slot` as broadcast (`SessionRuntime`
    /// sent-generated tracker).
    ///
    /// `SessionProcessor`-owned (`SessionRuntime::set_sent_generated`, `&mut`).
    /// Set alongside [`Self::set_generated`] at the end of `generated_block`.
    fn set_sent_generated(&mut self, slot: SlotIndex, value: bool);

    /// Session-start previous block ids (the genesis / no-parent prev-block
    /// seed).
    ///
    /// `SessionProcessor`-owned (`SessionRuntime::session_start_prev_blocks`).
    /// Read by [`CollationController::prepare_collation`] for the first block in
    /// a session, when no FSM parent is named. Returned by value (a small,
    /// session-lifetime list) so it does not cross the seam as a borrow.
    fn session_start_prev_blocks(&self) -> Vec<BlockIdExt>;

    /// Before-split bit of the finalized head, an empty-block-policy input.
    ///
    /// `SessionProcessor`-owned scalar. Fallback for
    /// [`CollationController::should_generate_empty_block`] when no
    /// parent-derived before-split value is available.
    fn finalized_head_before_split(&self) -> bool;

    /// Seqno of the finalized head, if finalization tracking is initialized.
    ///
    /// `SessionProcessor`-owned scalar, read only for the invariant-assertion
    /// diagnostics in [`CollationController::execute_collation_attempt`].
    fn finalized_head_seqno(&self) -> Option<u32>;

    /// Consensus-finalized seqno cursor, if known.
    ///
    /// `SessionProcessor`-owned scalar. Masterchain empty-block-policy input for
    /// [`CollationController::should_generate_empty_block`].
    fn last_consensus_finalized_seqno(&self) -> Option<u32>;

    /// Masterchain-finalized seqno cursor, if known.
    ///
    /// `SessionProcessor`-owned scalar. Shardchain empty-block-policy input for
    /// [`CollationController::should_generate_empty_block`].
    fn last_mc_finalized_seqno(&self) -> Option<u32>;

    /// Forget self-collation telemetry tracking for `slot` with `reason` while
    /// the precollation pipeline is flushed.
    ///
    /// `SessionProcessor`-owned *effect* (the telemetry sink + clock live there).
    /// Performed *synchronously*: [`CollationController::reset`] runs inside the
    /// borrowing backend (already on the session main loop) and the underlying
    /// `SessionTelemetry::forget_self_collation_tracking` is a `&self` call, so no
    /// SXMAIN bounce is needed — it runs from the on-loop pipeline reset, not the
    /// off-thread collation callback. Invoked by
    /// [`CollationController::reset`] for each cancelled precollation slot.
    fn forget_self_collation_tracking(&self, slot: SlotIndex, reason: &str);

    /// Broadcast the locally generated `candidate` for `slot` to the network.
    ///
    /// `SessionProcessor`-owned *effect*: forwards to
    /// `Receiver::send_block_broadcast`. That is a `&self` call (the receiver
    /// posts internally to its own queue), so the adapter performs it
    /// *synchronously* rather than bouncing onto SXMAIN — the candidate is
    /// broadcast immediately, preserving the C++-parity "broadcast immediately"
    /// invariant the publication tests assert. Invoked from the `generated_block`
    /// publication path once the candidate is signed.
    fn broadcast_candidate(&self, slot: u32, candidate_hash: UInt256, candidate: CandidateData);

    /// Persist the locally generated candidate-info record for `slot` (leader
    /// `self_idx`, the precomputed `candidate_hash_data_bytes`, and the candidate
    /// `signature`) to the session database.
    ///
    /// `SessionProcessor`-owned *effect*: writes through
    /// `DatabaseController::save_candidate_info_to_db` (a `&mut self` call —
    /// hence the `&mut self` here, the one mutating method on this trait). The
    /// adapter holds the database, session id, and telemetry directly, so the
    /// write runs *synchronously*, starting the DB store before the broadcast as
    /// the previous in-line call did (the early `WaitCandidateInfoStored` signal
    /// for our own vote). The `candidate_hash_data_bytes` are assembled
    /// synchronously by the caller (while the heavy `GeneratedBlockDesc` is still
    /// borrowed), so only owned scalars cross the seam — no candidate-body clone.
    fn persist_candidate_info(
        &mut self,
        slot: SlotIndex,
        candidate_hash: UInt256,
        self_idx: ValidatorIndex,
        candidate_hash_data_bytes: Vec<u8>,
        signature: Vec<u8>,
    );

    /// Request the named parent candidate (`slot`, `hash`) from peers.
    ///
    /// `SessionProcessor`-owned *effect*: the fetch runs the full
    /// `request_candidate` machinery (skip-cert / throttle / body+notar guards,
    /// then `receiver.request_candidate`), which needs `&mut SessionProcessor`,
    /// so — unlike broadcast/persist — the adapter bounces it onto the session
    /// main loop. Invoked from the `check_collation` parent-resolution gate once
    /// the named parent's block id is still unresolved (cache + book miss).
    fn request_parent_candidate(&self, slot: SlotIndex, hash: UInt256);

    /// Re-ingest a locally generated candidate through the normal receive path
    /// (`on_candidate_received`).
    ///
    /// `SessionProcessor`-owned *effect*: `generated_block` loops the local
    /// candidate back through `on_candidate_received` so it lands in
    /// `received_candidates` exactly like a peer-delivered one. That needs `&mut
    /// SessionProcessor`, so — like [`Self::request_parent_candidate`] — the
    /// adapter bounces it onto the session main loop. `self_idx` is the local
    /// validator index; `candidate` is the TL candidate body.
    fn self_receive_candidate(&self, self_idx: u32, candidate: CandidateData);
}

/// Declares the controller's deferred-task backend view for the generic
/// [`ControllerQueue`](crate::controller_queue::ControllerQueue) seam: a
/// borrowing `dyn CollationBackend + 'b` rebuilt per re-entry (see
/// [`CollationBackend`]). Mirrors the `ValidationController` impl.
impl Controlled for CollationController {
    type Backend<'b> = dyn CollationBackend + 'b;
}

/// Per-session collation-phase controller.
///
/// Owned by [`SessionProcessor`](crate::session_processor::SessionProcessor)
/// as `self.collation`; all access goes through the accessor methods
/// declared below — internal fields stay non-`pub` so the boundary is
/// enforceable.
pub(crate) struct CollationController {
    /// Generic handle for posting deferred / async collation work back to the
    /// session main loop, targeting `&mut Self`. The concrete `&mut
    /// SessionProcessor -> &mut CollationController` projection lives in the
    /// `CollationQueueAdapter` at the composition root, so this controller
    /// names only the generic queue and never depends on `SessionProcessor` —
    /// mirroring
    /// [`ValidationController`](crate::validation_controller::ValidationController).
    /// Read via [`Self::queue`] by `SessionProcessor`'s collation failure-retry
    /// re-entry, which posts the delayed retry through this handle.
    queue: ControllerQueuePtr<Self>,
    /// Callback-delivery aspect, shared with `SessionProcessor` and
    /// `SessionImpl` as `Arc<SessionCallbacks>`. Held directly so the
    /// controller can dispatch the higher-layer generate-slot request
    /// (`notify_generate_slot`) without routing the call back through
    /// `SessionProcessor`. `SessionCallbacks` owns the session-listener handle
    /// internally and never calls back into `SessionProcessor`, so this
    /// introduces no cycle. Mirrors
    /// [`ValidationController`](crate::validation_controller::ValidationController).
    callbacks: Arc<SessionCallbacks>,
    /// Immutable per-session configuration handle — the same `Arc` held by
    /// [`SessionProcessor`](crate::session_processor::SessionProcessor) and
    /// `SessionRuntime`. Held directly so collation policy can read the
    /// session id, options, shard, and leader/self index without threading
    /// `&SessionDescription` / `&SessionId` through every call. Mirrors
    /// [`ValidationController`](crate::validation_controller::ValidationController).
    description: Arc<SessionDescription>,
    /// Per-session telemetry aspect, the same `Arc` held by
    /// `SessionProcessor`. Held directly so collation paths can record
    /// self-collation observability / counters without threading a
    /// `&SessionTelemetry` through every entry point. `SessionTelemetry` is
    /// interior-mutable, so a shared `Arc` is sufficient. Mirrors
    /// [`ValidationController`](crate::validation_controller::ValidationController).
    telemetry: Arc<SessionTelemetry>,
    /// Precollated blocks pipeline, keyed by slot.
    precollated_blocks: PrecollatedBlockMap,
    /// Monotonic id assigned to each new precollation request.
    precollated_blocks_next_request_id: u32,
    /// Highest slot currently enqueued in the precollation pipeline.
    ///
    /// Used to chain `precollate_block(next_slot)` when the caller's
    /// preferred slot is already in flight.
    precollated_blocks_max_slot: Option<SlotIndex>,
    /// Earliest wall-clock time when the next collation start is allowed.
    /// Set to `now + target_rate` when a collation is initiated (or a
    /// precollated block is consumed). Checked at the top of
    /// `check_collation`. C++ parity: `block-producer.cpp
    /// coro_sleep(target_time)`.
    earliest_collation_time: Option<SystemTime>,
    /// Window-local chain head for candidate chaining across slots in the
    /// same leader window. C++ parity: `block-producer.cpp` carries
    /// mutable `parent` and `state` across slots within a single leader
    /// window.
    local_chain_head: Option<LocalChainHead>,
    /// Last slot for which generation was requested. Must be
    /// monotonically increasing (gaps allowed) — enforced by an assertion
    /// in `invoke_collation`.
    last_generated_slot: Option<SlotIndex>,
    /// Synchronous cache of locally generated parent metadata, keyed by
    /// `RawCandidateId`. Populated in `generated_block()` *before* the
    /// async `on_candidate_received` self-loop, so `resolve_parent_block_id`
    /// can find the parent immediately for chained precollation.
    generated_parent_cache: HashMap<RawCandidateId, BlockIdExt>,
    /// Exact generation timestamps for locally generated parents before
    /// the async self-receive path populates `CandidateBook`. Cleared
    /// alongside `generated_parent_cache` whenever the window-local chain
    /// head is invalidated.
    generated_parent_gen_utime_ms_cache: HashMap<RawCandidateId, u64>,
    /// Trace collector for recording collation lifecycle events. `None` when
    /// stats collection is disabled — every hot-path call site is guarded with
    /// `if let Some(tc) = &self.trace_collector`. Cheap-to-clone handle cloned
    /// from `SessionProcessor` at construction
    trace_collector: Option<TraceCollector>,
    /// In-flight REAL collation, mirroring C++ `block_generation_active` /
    /// `block_generation` in `block-producer.cpp`. `Some` from real dispatch until
    /// the collation completes, fails, or the pipeline is `reset()`. The
    /// single-in-flight guard reads it to keep one collation alive across slots
    /// while the per-slot wake covers the elapsed slots with empty fillers.
    block_generation_active: Option<RealCollationState>,
    /// Wall-clock time of the last observed consensus-finalization advance, mirroring
    /// C++ `last_consensus_finalized_at_` in `block-producer.cpp`. Refreshed by
    /// [`Self::refresh_finalization_timestamp`] from both `check_collation` and the
    /// per-slot wake whenever the consensus-finalized seqno increases; the
    /// `allow_empty` gate ([`Self::empties_allowed_by_finalization`], bounded by
    /// `no_empty_blocks_on_error_timeout`) reads it.
    last_consensus_finalized_at: Option<SystemTime>,
    /// Highest consensus-finalized seqno observed so far. Only used to detect the
    /// advance that bumps `last_consensus_finalized_at`.
    last_observed_finalized_seqno: Option<u32>,
}

// ======================================================================
// Construction & handles
// ======================================================================
// Build the controller and read its shared session handles.
impl CollationController {
    /// Construct a fresh, empty collation controller bound to the shared
    /// session `description`.
    ///
    /// `callbacks` is a cheap-to-clone shared handle (`Arc`) cloned from
    /// `SessionProcessor` at construction so the controller can dispatch the
    /// generate-slot notification itself via [`Self::notify_generate_slot`].
    /// The session-listener handle lives inside `SessionCallbacks`, so no
    /// separate listener is needed here.
    ///
    /// `telemetry` is the shared per-session telemetry aspect (the same `Arc`
    /// `SessionProcessor` holds), so collation paths record metrics without a
    /// `&SessionTelemetry` parameter.
    ///
    /// `queue` is the generic deferred-work handle (`ControllerQueuePtr<Self>`)
    /// the controller uses to post follow-up / async work targeting `&mut
    /// Self`. In production it is a `CollationQueueAdapter` projecting from
    /// `SessionProcessor`; in tests it is a fake — so the controller stays
    /// constructible without a `SessionProcessor`. Mirrors
    /// `ValidationController::new`.
    pub(crate) fn new(
        queue: ControllerQueuePtr<Self>,
        callbacks: Arc<SessionCallbacks>,
        description: Arc<SessionDescription>,
        telemetry: Arc<SessionTelemetry>,
        trace_collector: Option<TraceCollector>,
    ) -> Self {
        Self {
            queue,
            callbacks,
            description,
            telemetry,
            precollated_blocks: PrecollatedBlockMap::new(),
            precollated_blocks_next_request_id: 0,
            precollated_blocks_max_slot: None,
            earliest_collation_time: None,
            local_chain_head: None,
            last_generated_slot: None,
            generated_parent_cache: HashMap::new(),
            generated_parent_gen_utime_ms_cache: HashMap::new(),
            trace_collector,
            block_generation_active: None,
            last_consensus_finalized_at: None,
            last_observed_finalized_seqno: None,
        }
    }

    /// Session identifier, read from the held [`SessionDescription`]. Mirrors
    /// `ValidationController::session_id` so collation methods need not take a
    /// `&SessionId` argument.
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.description.get_session_id()
    }

    /// Current session time, read from the held [`SessionDescription`]
    /// (real-time or a manual override for tests / log replay). Identical to
    /// `SessionProcessor::now`, which reads the same description. Mirrors
    /// `ValidationController::now`.
    #[inline]
    fn now(&self) -> SystemTime {
        self.description.get_time()
    }

    /// The deferred-work queue handle, used by `SessionProcessor` to post
    /// collation re-entries (today: the failure-retry) that target `&mut Self`
    /// with a freshly built [`CollationBackend`]. Mirrors how
    /// `ValidationController` posts to its own `queue`.
    fn queue(&self) -> &ControllerQueuePtr<Self> {
        &self.queue
    }

    // ----- precollated_blocks -----
}

// ======================================================================
// Precollation pipeline state
// ======================================================================
// The in-flight precollation map plus its request-id and high-water-mark cursors.
impl CollationController {
    /* Precollated-block map */

    /// Number of slots currently in the precollation pipeline.
    pub(crate) fn precollated_count(&self) -> usize {
        self.precollated_blocks.len()
    }

    /// True if `slot` already has a precollation entry.
    pub(crate) fn precollated_contains(&self, slot: SlotIndex) -> bool {
        self.precollated_blocks.contains_key(&slot)
    }

    /// Look up the precollation entry for `slot`.
    pub(crate) fn precollated(&self, slot: SlotIndex) -> Option<&PrecollatedBlock> {
        self.precollated_blocks.get(&slot)
    }

    /// Snapshot all slots currently in the precollation pipeline (used by
    /// `reset_precollations` to drive `forget_self_collation_tracking`).
    pub(crate) fn precollated_slots(&self) -> Vec<SlotIndex> {
        self.precollated_blocks.keys().copied().collect()
    }

    /// Iterator over `precollated_blocks` keys (used by leader-window
    /// staleness checks in `check_collation`).
    fn precollated_keys(&self) -> impl Iterator<Item = SlotIndex> + '_ {
        self.precollated_blocks.keys().copied()
    }

    /// Iterator over `(slot, &PrecollatedBlock)` pairs (used by
    /// `reset_precollations` to cancel in-flight requests).
    fn iter_precollated(&self) -> impl Iterator<Item = (&SlotIndex, &PrecollatedBlock)> + '_ {
        self.precollated_blocks.iter()
    }

    /// Insert a precollation entry for `slot`, returning the displaced
    /// entry if any.
    pub(crate) fn insert_precollated(
        &mut self,
        slot: SlotIndex,
        block: PrecollatedBlock,
    ) -> Option<PrecollatedBlock> {
        self.precollated_blocks.insert(slot, block)
    }

    /// Remove and return the precollation entry for `slot`.
    fn remove_precollated(&mut self, slot: SlotIndex) -> Option<PrecollatedBlock> {
        self.precollated_blocks.remove(&slot)
    }

    /// Drop every precollation entry (called from `reset_precollations`).
    fn clear_precollated(&mut self) {
        self.precollated_blocks.clear();
    }

    // ----- precollated_blocks_next_request_id -----

    /* Request id & high-water mark */

    /// Return the next precollation request id and bump the internal
    /// counter (post-increment semantics).
    fn next_precollation_request_id(&mut self) -> u32 {
        let id = self.precollated_blocks_next_request_id;
        self.precollated_blocks_next_request_id += 1;
        id
    }

    // ----- precollated_blocks_max_slot -----

    /// Highest slot currently enqueued in the precollation pipeline.
    fn precollated_max_slot(&self) -> Option<SlotIndex> {
        self.precollated_blocks_max_slot
    }

    /// Bump the `precollated_blocks_max_slot` cursor if `slot` is greater
    /// than the current high-water mark (or no high-water mark is set).
    fn note_precollated_slot(&mut self, slot: SlotIndex) {
        if self.precollated_blocks_max_slot.is_none_or(|max| slot > max) {
            self.precollated_blocks_max_slot = Some(slot);
        }
    }

    /// Clear the `precollated_blocks_max_slot` cursor (called from
    /// `reset_precollations`).
    fn reset_precollated_max_slot(&mut self) {
        self.precollated_blocks_max_slot = None;
    }

    // ----- earliest_collation_time -----
}

// ======================================================================
// Collation pacing (rate gate)
// ======================================================================
// `earliest_collation_time` rate gate (C++ block-producer.cpp target_time parity).
impl CollationController {
    /// Earliest wall-clock time when the next collation start is allowed.
    pub(crate) fn earliest_collation_time(&self) -> Option<SystemTime> {
        self.earliest_collation_time
    }

    /// Set the earliest-collation-time gate (set to `Some(now + rate)`
    /// after every collation start; tests also call this with arbitrary
    /// values to exercise pacing).
    pub(crate) fn set_earliest_collation_time(&mut self, time: Option<SystemTime>) {
        self.earliest_collation_time = time;
    }

    // ----- local_chain_head -----

    /// Advance `earliest_collation_time` by `target_rate` from the current
    /// session time.
    ///
    /// Called after every collation start (invoke, retry, precollated
    /// hit, empty-block short-circuit) to pace the next one. C++ parity:
    /// `block-producer.cpp target_time += target_rate_ms`. Time is read from
    /// the held [`SessionDescription`] via [`Self::now`] (the same clock
    /// `SessionProcessor` would pass in), so callers need not thread it.
    pub(crate) fn update_collation_pacing(&mut self) {
        let target_rate = self.description.opts().target_rate;
        self.set_earliest_collation_time(Some(self.now() + target_rate));
    }
}

// ======================================================================
// Local chain head & generated-parent caches
// ======================================================================
// Window-local chain head + the synchronous locally-generated-parent metadata
// caches that let the next slot chain without waiting for the async self-loop.
impl CollationController {
    /* Window-local chain head */

    /// Current window-local chain head, if any.
    pub(crate) fn local_chain_head(&self) -> Option<&LocalChainHead> {
        self.local_chain_head.as_ref()
    }

    /// Set or clear the window-local chain head.
    pub(crate) fn set_local_chain_head(&mut self, head: Option<LocalChainHead>) {
        self.local_chain_head = head;
    }

    // ----- last_generated_slot -----

    /// Last slot for which generation was requested.
    fn last_generated_slot(&self) -> Option<SlotIndex> {
        self.last_generated_slot
    }

    /// Update the last-generated-slot cursor (called inside
    /// `invoke_collation` after the monotonicity assertion).
    fn set_last_generated_slot(&mut self, slot: Option<SlotIndex>) {
        self.last_generated_slot = slot;
    }

    // ----- generated_parent_cache -----

    /* Generated-parent metadata caches */

    /// Insert a locally-generated parent into the synchronous cache,
    /// returning the displaced entry if any.
    pub(crate) fn insert_generated_parent(
        &mut self,
        candidate_id: RawCandidateId,
        block_id: BlockIdExt,
    ) -> Option<BlockIdExt> {
        self.generated_parent_cache.insert(candidate_id, block_id)
    }

    /// Insert the exact generation timestamp for a locally-generated parent.
    fn insert_generated_parent_gen_utime_ms(
        &mut self,
        candidate_id: RawCandidateId,
        gen_utime_ms: u64,
    ) -> Option<u64> {
        self.generated_parent_gen_utime_ms_cache.insert(candidate_id, gen_utime_ms)
    }

    /// Resolve a locally-generated parent's `BlockIdExt` from the
    /// synchronous cache.
    fn resolve_generated_parent_block_id(
        &self,
        candidate_id: &RawCandidateId,
    ) -> Option<&BlockIdExt> {
        self.generated_parent_cache.get(candidate_id)
    }

    /// Find a generated-parent `RawCandidateId` by `BlockIdExt`.
    pub(crate) fn find_generated_parent_id_by_block_id(
        &self,
        block_id: &BlockIdExt,
    ) -> Option<RawCandidateId> {
        self.generated_parent_cache.iter().find_map(|(candidate_id, candidate_block_id)| {
            (candidate_block_id == block_id).then_some(candidate_id.clone())
        })
    }

    /// Clear both generated-parent caches (called from
    /// `invalidate_local_chain_head`).
    fn clear_generated_parent_caches(&mut self) {
        self.generated_parent_cache.clear();
        self.generated_parent_gen_utime_ms_cache.clear();
    }

    // ----- pipeline reset -----

    /* Invalidation */

    /// Invalidate the window-local chain head and clear the synchronous
    /// generated-parent metadata caches.
    ///
    /// Called when the leader window changes, the progress cursor jumps,
    /// or a consensus event (skip/notarize) invalidates the locally
    /// generated parent chain. After invalidation, the next collation
    /// will start fresh from the FSM `available_base`.
    pub(crate) fn invalidate_local_chain_head(&mut self) {
        if let Some(head) = self.local_chain_head.as_ref() {
            log::trace!(
                "Session {} invalidate_local_chain_head: clearing (was window={}, slot={})",
                &self.session_id().to_hex_string()[..8],
                head.window,
                head.slot,
            );
        }
        self.local_chain_head = None;
        self.clear_generated_parent_caches();
    }
}

// ======================================================================
// Parent / candidate resolution
// ======================================================================
// Composite parent/candidate lookups across the generated-parent cache, the
// `CandidateBook`, and the window-local chain head.
impl CollationController {
    /// Resolve a parent candidate's `BlockIdExt` using the synchronous
    /// generated-parent cache first, then falling back to the supplied
    /// `CandidateBook`.
    ///
    /// The cache is populated in `generated_block()` *before* the async
    /// `on_candidate_received` self-loop runs, so precollation chaining
    /// can find the parent immediately. The book covers everything else
    /// (received candidates, repaired candidates, recovery seeds).
    pub(crate) fn resolve_parent_block_id(
        &self,
        parent: &CandidateParentInfo,
        backend: &dyn CollationBackend,
    ) -> Option<BlockIdExt> {
        let parent_id = RawCandidateId { slot: parent.slot, hash: parent.hash.clone() };
        self.resolve_generated_parent_block_id(&parent_id)
            .cloned()
            .or_else(|| backend.book_received_block_id(&parent_id))
    }

    // ----- collation gating -----

    /// Resolve the exact `gen_utime_ms` of a parent candidate.
    ///
    /// Lookup order mirrors the C++ block-producer.cpp parent-timestamp
    /// resolution:
    ///
    /// 1. `generated_parent_gen_utime_ms_cache` — synchronous cache seeded
    ///    by `generated_block()` before the async self-loop completes.
    /// 2. `candidate_book.received(...)` — populated from the network
    ///    receive path (or the async self-loop).
    /// 3. `local_chain_head` — covers the brief window when only the head
    ///    is set but the cache entry has not been written yet.
    fn resolve_parent_gen_utime_ms(
        &self,
        parent: &CandidateParentInfo,
        backend: &dyn CollationBackend,
    ) -> Option<u64> {
        let parent_id = RawCandidateId { slot: parent.slot, hash: parent.hash.clone() };
        self.generated_parent_gen_utime_ms_cache
            .get(&parent_id)
            .copied()
            .or_else(|| backend.book_received_gen_utime_ms(&parent_id))
            .or_else(|| self.local_chain_head_gen_utime_ms(parent))
    }

    /// Parent `gen_utime_ms` resolution for the validation min-block-interval
    /// pacing gate.
    ///
    /// Reads the same three sources, in the same order, as
    /// [`Self::resolve_parent_gen_utime_ms`] — the self-collation gen-utime
    /// cache, the received-candidate book, and the local chain head — but takes
    /// a `&CandidateBook` directly instead of a [`CollationBackend`], because the
    /// validation seam
    /// ([`ValidationBackend::parent_gen_utime_ms`](crate::validation_controller::ValidationBackend::parent_gen_utime_ms))
    /// has a `&CandidateBook` in hand and no collation backend.
    pub(crate) fn resolve_parent_gen_utime_ms_via_book(
        &self,
        parent: &CandidateParentInfo,
        candidate_book: &CandidateBook,
    ) -> Option<u64> {
        let parent_id = RawCandidateId { slot: parent.slot, hash: parent.hash.clone() };
        self.generated_parent_gen_utime_ms_cache
            .get(&parent_id)
            .copied()
            .or_else(|| candidate_book.received(&parent_id).and_then(|c| c.gen_utime_ms))
            .or_else(|| self.local_chain_head_gen_utime_ms(parent))
    }

    /// Local-chain-head `gen_utime_ms` for `parent`, if the head's parent
    /// matches. Shared tail of [`Self::resolve_parent_gen_utime_ms`] and
    /// [`Self::resolve_parent_gen_utime_ms_via_book`].
    fn local_chain_head_gen_utime_ms(&self, parent: &CandidateParentInfo) -> Option<u64> {
        self.local_chain_head.as_ref().and_then(|head| {
            if head.parent_info.slot == parent.slot && head.parent_info.hash == parent.hash {
                head.gen_utime_ms
            } else {
                None
            }
        })
    }

    // ----- collation-input preparation -----

    /// Before-split flag for the collation parent, used by the empty-block
    /// policy.
    ///
    /// Resolves the parent's `BlockIdExt` (controller generated-parent cache
    /// then the book seam, or the lone genesis prev-block) and reads its
    /// before-split bit through [`CollationBackend::before_split_flag`]. Moved
    /// verbatim from `SessionProcessor`.
    pub(crate) fn resolve_parent_before_split_flag(
        &self,
        backend: &dyn CollationBackend,
        parent: Option<&CandidateParentInfo>,
        prev_block_ids: &[BlockIdExt],
    ) -> Option<bool> {
        let parent_block_id = match parent {
            Some(parent_info) => self.resolve_parent_block_id(parent_info, backend),
            None if prev_block_ids.len() == 1 => Some(prev_block_ids[0].clone()),
            _ => None,
        }?;
        backend.before_split_flag(&parent_block_id)
    }
}

// ======================================================================
// Leader gate, request allocation & reset
// ======================================================================
// The local-collation entry gate, precollation-request allocation, and the
// pipeline cancel/reset helpers.
impl CollationController {
    /// Decide whether the local validator may begin collation for `slot`.
    ///
    /// Returns the local `ValidatorIndex` when (a) no precollation entry
    /// already exists for `slot` and (b) the local validator is the
    /// leader for `slot`. Returns `None` otherwise, emitting the matching
    /// trace log so call-site visibility is preserved.
    fn try_begin_collation_slot(&self, slot: SlotIndex) -> Option<ValidatorIndex> {
        if self.precollated_contains(slot) {
            log::trace!(
                "Session {} invoke_collation: slot {} already pending",
                self.session_id().to_hex_string(),
                slot
            );
            return None;
        }

        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(slot);
        if leader != self_idx {
            log::trace!(
                "Session {} invoke_collation: not leader for slot {} (leader={})",
                self.session_id().to_hex_string(),
                slot,
                leader
            );
            return None;
        }

        Some(self_idx)
    }

    /// Allocate a new precollation request, register it under `slot`, and
    /// return the assigned `(request_id, AsyncRequestImpl)` pair so the
    /// caller can dispatch the underlying collator request.
    fn create_pending_collation_request(
        &mut self,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
        min_gen_time: SystemTime,
        soft_deadline: Option<SystemTime>,
        hard_deadline: Option<SystemTime>,
        budget_anchor: Option<SystemTime>,
    ) -> (u32, Arc<AsyncRequestImpl>) {
        self.note_precollated_slot(slot);

        let request_id = self.next_precollation_request_id();
        let request = AsyncRequestImpl::new_with_deadlines(
            request_id,
            true,
            min_gen_time,
            soft_deadline,
            hard_deadline,
            budget_anchor,
        );
        let precollated_block = PrecollatedBlock { request: request.clone(), result: None, parent };
        self.insert_precollated(slot, precollated_block);

        (request_id, request)
    }

    /// Cancel every in-flight precollation request, drain the precollation
    /// map, and reset the high-water-mark cursor.
    ///
    /// Returns the list of slots whose entries were cancelled. The caller
    /// must drive `forget_self_collation_tracking` for each returned slot
    /// (telemetry path) and follow up with `invalidate_local_chain_head`
    /// to finish flushing the pipeline (see
    /// `SessionProcessor::reset_precollations`).
    fn cancel_pending_precollations(&mut self) -> Vec<SlotIndex> {
        log::debug!(
            "Session {} cancel_pending_precollations: cancelling {} pending precollations",
            self.session_id().to_hex_string(),
            self.precollated_count()
        );

        let cancelled_slots = self.precollated_slots();
        for (_slot, precollated_block) in self.iter_precollated() {
            precollated_block.request.cancel();
        }

        self.clear_precollated();
        self.reset_precollated_max_slot();

        cancelled_slots
    }

    /// Remove the precollation entry for `slot` and emit a trace log.
    ///
    /// Returns `true` if an entry was actually removed so the caller can
    /// drive telemetry counter increments without inspecting controller
    /// internals.
    fn remove_precollated_with_log(&mut self, slot: SlotIndex) -> bool {
        if self.remove_precollated(slot).is_some() {
            log::trace!(
                "Session {} remove_precollated_block: removed slot {}",
                self.session_id().to_hex_string(),
                slot
            );
            true
        } else {
            false
        }
    }

    // ----- parent / candidate resolution -----

    /// Flush the precollation pipeline: cancel every in-flight precollation,
    /// forget its self-collation telemetry tracking, and invalidate the
    /// window-local chain head.
    ///
    /// C++ parity: `block-producer.cpp` replaces `cancellation_source_` on each
    /// new `OurLeaderWindowStarted`; the Rust pipeline is reset on session stop,
    /// leader-window change, or when the progress cursor jumps past queued
    /// precollation slots. The pipeline-data cleanup is controller state
    /// ([`Self::cancel_pending_precollations`] +
    /// [`Self::invalidate_local_chain_head`]); the per-cancelled-slot telemetry
    /// forget is a `SessionProcessor` effect reached through
    /// [`CollationBackend::forget_self_collation_tracking`].
    pub(crate) fn reset(&mut self, backend: &dyn CollationBackend) {
        for slot in self.cancel_pending_precollations() {
            backend.forget_self_collation_tracking(slot, "precollation_pipeline_reset");
        }
        self.invalidate_local_chain_head();
        // Drop the in-flight REAL-collation marker: reset() cancels the pending
        // AsyncRequests, so no completion will arrive to clear it. C++ replaces
        // `cancellation_source_` / `block_generation` on each new leader window.
        self.block_generation_active = None;
    }
}

// ======================================================================
// Empty-block recovery policy
// ======================================================================
// Pure policy deciding whether to generate an empty block this slot.
impl CollationController {
    /// Decide whether the next block to be generated must be empty.
    ///
    /// Empty-block generation is required when:
    ///
    /// 1. The current parent (or, as a fallback, the finalized head) has
    ///    `before_split=true` — i.e. the chain is at a shard split/merge
    ///    boundary and a recovery empty block is mandatory.
    /// 2. Otherwise, finalization lag exceeds the configured budget. The
    ///    masterchain path uses `last_consensus_finalized_seqno + 1 <
    ///    new_seqno`; shardchains use `last_mc_finalized_seqno +
    ///    empty_block_mc_lag_threshold < new_seqno`.
    ///
    /// Returns `false` if finalization tracking has not been initialized.
    ///
    /// C++ parity: `block-producer.cpp::should_generate_empty_block()`,
    /// extended with the per-slot `parent_before_split` hint so split/merge
    /// recovery does not rely on the more conservative finalized-head
    /// snapshot when a fresh parent state is available.
    pub(crate) fn should_generate_empty_block(
        &self,
        slot: SlotIndex,
        new_seqno: u32,
        parent_before_split: Option<bool>,
        finalized_head_before_split: bool,
        last_consensus_finalized_seqno: Option<u32>,
        last_mc_finalized_seqno: Option<u32>,
    ) -> bool {
        // C++ parity: ALWAYS generate empty if current parent state has
        // before_split=true. Prefer parent-derived value; fall back to
        // finalized head only when parent-specific metadata is unavailable.
        if parent_before_split.unwrap_or(finalized_head_before_split) {
            log::debug!(
                "Session {} should_generate_empty_block: slot={}, seqno={} - generating EMPTY \
                (parent before_split=true, required for split/merge)",
                &self.session_id().to_hex_string()[..8],
                slot,
                new_seqno
            );
            return true;
        }

        if self.description.get_shard().is_masterchain() {
            // Masterchain: consensus-finalized seqno must be at most 1
            // behind new seqno. C++ parity: external notify via
            // `set_mc_finalized_block()` also advances this
            // producer-side cursor through `last_mc_finalized_seqno`.
            match last_consensus_finalized_seqno {
                Some(finalized) => finalized + 1 < new_seqno,
                None => false,
            }
        } else {
            // Shardchain: MC finalized can be up to threshold behind.
            // C++ parity: `last_mc_finalized_seqno_ + 8 < new_seqno`.
            match (last_mc_finalized_seqno, self.description.opts().empty_block_mc_lag_threshold) {
                (Some(mc_finalized), Some(threshold)) => mc_finalized + threshold < new_seqno,
                _ => false,
            }
        }
    }
}

// ======================================================================
// Collation pipeline
// ======================================================================
// The collation pipeline proper: scheduling -> attempt execution -> completion
// -> block generation / next-slot chaining. Re-entrant steps bounce through the
// controller queue rather than recursing.
impl CollationController {
    /* Entry & scheduling */

    /// Check whether the local validator should generate a block for the
    /// current progress slot.
    ///
    /// Moved from `SessionProcessor` matching the origin/master structure: the
    /// pacing gate, stale-window guard, stale-precollation reset, pipeline-fill /
    /// leadership / parent-availability checks, parent-resolvability gate, and
    /// the precollated-result-vs-fresh-collation split are all inline here. FSM
    /// reads (`first_non_progressed_slot`, `current_leader_window_idx`,
    /// `has_available_parent`, `get_available_parent`), the wake horizon, the
    /// per-slot generated / pending state, the parent peer-fetch, and the
    /// pending-generate write all go through the borrowing [`CollationBackend`].
    pub(crate) fn check_collation(&mut self, backend: &mut dyn CollationBackend) {
        instrument!();

        // Maintain the consensus-finalization timestamp (C++ block-producer.cpp):
        // bump it whenever the consensus-finalized seqno advances.
        self.refresh_finalization_timestamp(backend);

        // Drop a stuck in-flight real-collation marker left over from a superseded leader
        // window before any of the stale-window early-returns below (which can bypass the
        // `reset()` further down when the progress cursor lags). Otherwise the
        // single-in-flight guard would decline every real collation in the new window.
        self.clear_stale_block_generation(backend);

        // Don't collate faster than target_rate (block-producer.cpp
        // coro_sleep(target_time)).
        if let Some(earliest) = self.earliest_collation_time() {
            if self.now() < earliest {
                backend.request_wake_at(earliest);
                return;
            }
        }

        // Use the FSM progress cursor (first non-progressed slot) for collation
        // decisions. Collation follows notarized/skipped progress, not
        // finalization (C++ block-producer.cpp collates on the notarized chain).
        let current_slot = backend.first_non_progressed_slot();

        // Stale window guard (C++ parity: consensus.cpp LeaderWindowObserved sets
        // current_window_ BEFORE the leader check). Skip collation when the
        // progress cursor still points at a slot in a superseded window.
        let slot_window = self.description.get_window_idx(current_slot);
        let current_window = backend.current_leader_window_idx();
        if slot_window < current_window {
            log::trace!(
                "Session {} check_collation: skipping stale slot {} (window {} < current {})",
                &self.session_id().to_hex_string()[..8],
                current_slot,
                slot_window,
                current_window
            );
            return;
        }

        // C++ parity: cancel stale precollations when the leader window changes
        // (block-producer.cpp replaces cancellation_source_ on each new
        // OurLeaderWindowStarted). Detect even when local_chain_head is None
        // (bootstrap) by scanning the queued precollation slots.
        let has_stale_precollations = if let Some(head) = self.local_chain_head() {
            head.window != current_window
        } else {
            self.precollated_keys().any(|s| self.description.get_window_idx(s) != current_window)
        };
        if has_stale_precollations {
            log::debug!(
                "Session {} check_collation: leader window changed to {}, \
                resetting precollation pipeline",
                &self.session_id().to_hex_string()[..8],
                current_window,
            );
            self.reset(backend);
        }

        // Don't generate if already generated or pending for this slot. If we
        // have a local chain head and no queued precollation for the next slot in
        // this window, keep the pipeline full by retrying it here.
        if backend.is_generated(current_slot) || backend.is_pending_generate(current_slot) {
            if let Some(head) = self.local_chain_head() {
                let next_slot = SlotIndex(head.slot.0 + 1);
                let head_window = head.window;
                if head_window == current_window
                    && !backend.is_generated(next_slot)
                    && !backend.is_pending_generate(next_slot)
                    && !self.precollated_contains(next_slot)
                {
                    self.precollate_block(backend, next_slot);
                }
            }
            return;
        }

        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(current_slot);

        // Only the slot leader collates.
        if leader != self_idx {
            return;
        }

        // A valid parent (per-slot available_base on the notarized/skip chain,
        // not finalization) must exist in the FSM.
        if !backend.has_available_parent(current_slot) {
            log::trace!(
                "Session {} check_collation: waiting for parent for slot {current_slot} (no \
                available parent in FSM)",
                self.session_id().to_hex_string(),
            );
            return;
        }

        let parent = backend.get_available_parent(current_slot);
        log::trace!(
            "Session {} check_collation: we are leader for slot {}, parent={:?}",
            self.session_id().to_hex_string(),
            current_slot,
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
        );

        // Resolve the parent BlockIdExt (required for explicit-parent hints and
        // seqno derivation). If a named parent is still unresolved, request it
        // from peers and defer this pass.
        if let Some(parent_info) = parent.as_ref() {
            if self.resolve_parent_block_id(parent_info, backend).is_none() {
                log::trace!(
                    "Session {} check_collation: waiting for resolved parent BlockIdExt for slot \
                    {current_slot} (parent={parent_info}), requesting parent",
                    self.session_id().to_hex_string(),
                );
                backend.request_parent_candidate(parent_info.slot, parent_info.hash.clone());
                return;
            }
        }

        // Mark pending_generate for this slot.
        backend.set_pending_generate(current_slot, true);

        // Check for a precollated block first.
        self.telemetry.collates_precollated_counter.total_increment();
        let precollated_result = self.precollated(current_slot).and_then(|pb| pb.result.clone());

        if let Some(result) = precollated_result {
            log::trace!(
                "Session {} check_collation: precollated result found for slot {}",
                self.session_id().to_hex_string(),
                current_slot
            );

            self.telemetry.collates_precollated_counter.success();
            self.telemetry.record_collation_start();

            // C++ parity: future slots may be precollated as normal or empty blocks.
            self.generated_block(backend, current_slot, result);
            self.update_collation_pacing();

            // Precollate the next block.
            self.precollate_block(backend, current_slot + 1);

            return;
        }

        self.telemetry.collates_precollated_counter.failure();

        // No precollated block, invoke collation.
        self.invoke_collation(backend, current_slot, parent);
    }

    /// Invoke block collation for a slot (initial attempt). Moved from
    /// `SessionProcessor` matching origin/master: the leadership/pending gate
    /// (`try_begin_collation_slot`) and the monotonic-slot invariant stay inline,
    /// then the attempt is handed to [`Self::execute_collation_attempt`].
    fn invoke_collation(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
    ) {
        instrument!();
        let Some(self_idx) = self.try_begin_collation_slot(slot) else {
            return;
        };

        // INVARIANT: generation slots must be monotonically increasing (gaps
        // allowed).
        if let Some(last_slot) = self.last_generated_slot() {
            assert!(
                slot >= last_slot,
                "CollationController INVARIANT VIOLATION: generation requested for slot {} but \
                last generation was slot {} (generation slots must be monotonically increasing)",
                slot,
                last_slot
            );
        }
        self.set_last_generated_slot(Some(slot));
        self.execute_collation_attempt(backend, slot, parent, self_idx, true, true, None);
    }

    /// Restart a collation for `slot` after a genuine error (the C++ genuine-error
    /// retry in `block-producer.cpp`). Re-captures the FSM available parent and
    /// re-enters [`Self::execute_collation_attempt`] as a normal dispatch - so it
    /// re-arms the per-slot deadline wake exactly like the initial attempt - but
    /// with the lenient flags (no pending-generate clear, no progress-slot
    /// assertion) since the caller has already re-gated the slot. `pinned_deadlines`
    /// carries the ORIGINAL attempt's soft/hard cutoffs + budget anchor so the restart
    /// races the SAME window-end budget rather than a fresh window recomputed from the
    /// (now advanced) clock.
    fn restart_collation(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        pinned_deadlines: Option<CollationDeadlineContext>,
    ) {
        instrument!();
        let Some(self_idx) = self.try_begin_collation_slot(slot) else {
            return;
        };

        // Capture the parent at collation start (same as invoke_collation).
        let parent = backend.get_available_parent(slot);
        self.execute_collation_attempt(
            backend,
            slot,
            parent,
            self_idx,
            false,
            false,
            pinned_deadlines,
        );
    }

    /* Attempt execution */

    /// Prepare and dispatch (or defer / short-circuit to empty) a single
    /// collation attempt for `slot`. Moved from `SessionProcessor`: resolves the
    /// prev-block chain + dispatch timing through [`Self::prepare_collation`],
    /// applies the empty-block policy ([`Self::should_generate_empty_block`]),
    /// and otherwise hands off to [`Self::dispatch_collation_request`]. The
    /// session-start prev-blocks, finalized scalars, wake horizon, and
    /// pending-generate clear-on-defer all go through the borrowing backend.
    fn execute_collation_attempt(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
        self_idx: ValidatorIndex,
        clear_pending_generate_on_not_ready: bool,
        enforce_progress_slot_invariant: bool,
        pinned_deadlines: Option<CollationDeadlineContext>,
    ) {
        // Trace: collation attempt started for this slot (fires on every attempt).
        if let Some(tc) = &self.trace_collector {
            tc.record_collate_started(self.session_id(), slot);
        }
        // Single in-flight guard (C++ block-producer.cpp): never begin a new REAL
        // collation while one is already running. Release is guaranteed by
        // on_collation_complete / on_collation_failed_impl / reset() plus the
        // window-end hard stop_flag and the outer collation timeout, so a wedged
        // collator can never block the pipeline indefinitely. Empty fillers are NOT
        // dispatched here - the per-slot deadline wake emits them without
        // re-entering this path. Clearing pending_generate lets a later tick
        // re-evaluate the slot once the in-flight collation resolves.
        if let Some((active_slot, active_request_id)) =
            self.block_generation_active.as_ref().map(|s| (s.slot_dispatched, s.request_id))
        {
            log::trace!(
                "Session {} invoke_collation: declining real collation for slot {} - collation \
                slot {} (request_id={}) already in flight (single in-flight)",
                &self.session_id().to_hex_string()[..8],
                slot,
                active_slot,
                active_request_id,
            );
            if clear_pending_generate_on_not_ready {
                backend.set_pending_generate(slot, false);
            }
            return;
        }

        let session_start_prev_blocks = backend.session_start_prev_blocks();
        let prepared =
            match self.prepare_collation(parent.as_ref(), backend, &session_start_prev_blocks) {
                CollationPreparation::Ready(prepared) => prepared,
                CollationPreparation::Deferred(slot_start_time) => {
                    log::trace!(
                        "Session {} invoke_collation: deferring slot {} until {:?}",
                        self.session_id().to_hex_string(),
                        slot,
                        slot_start_time
                    );
                    if clear_pending_generate_on_not_ready {
                        backend.set_pending_generate(slot, false);
                    }
                    backend.request_wake_at(slot_start_time);
                    return;
                }
                CollationPreparation::WaitingForParent => {
                    if let Some(parent_info) = parent.as_ref() {
                        log::trace!(
                            "Session {} invoke_collation: waiting for resolved parent BlockIdExt \
                            for slot {slot} (parent={parent_info})",
                            self.session_id().to_hex_string(),
                        );
                    }
                    if clear_pending_generate_on_not_ready {
                        backend.set_pending_generate(slot, false);
                    }
                    return;
                }
            };

        let prev_block_ids = prepared.prev_block_ids.clone();
        let timing = prepared.timing;
        let new_seqno = prepared.new_seqno;
        let is_first_session_block = prepared.is_first_session_block;
        let parent_before_split =
            self.resolve_parent_before_split_flag(backend, parent.as_ref(), &prev_block_ids);

        if self.should_generate_empty_block(
            slot,
            new_seqno,
            parent_before_split,
            backend.finalized_head_before_split(),
            backend.last_consensus_finalized_seqno(),
            backend.last_mc_finalized_seqno(),
        ) {
            assert!(
                !is_first_session_block,
                "CollationController INVARIANT VIOLATION: invoke_collation \
                should_generate_empty_block({}) returned true but no parent available. First \
                block in epoch cannot be empty. finalized_head_seqno={:?}, \
                last_mc_finalized_seqno={:?}",
                new_seqno,
                backend.finalized_head_seqno(),
                backend.last_mc_finalized_seqno()
            );
            let parent_block_id = prev_block_ids.first().cloned().expect(
                "non-first Simplex collation attempt must have one resolved parent block id",
            );
            log::debug!(
                "Session {} invoke_collation: generating EMPTY block for slot {}! \
                new_seqno={}, finalized_head_seqno={:?}, last_mc_finalized_seqno={:?}",
                self.session_id().to_hex_string(),
                slot,
                new_seqno,
                backend.finalized_head_seqno(),
                backend.last_mc_finalized_seqno()
            );
            self.finish_empty_collation_attempt(
                backend,
                slot,
                parent,
                timing.min_gen_time,
                parent_block_id,
            );
            return;
        }

        if enforce_progress_slot_invariant {
            let first_non_progressed = backend.first_non_progressed_slot();
            assert!(
                slot >= first_non_progressed,
                "CollationController INVARIANT VIOLATION: invoke_collation for slot {} but \
                first_non_progressed_slot is {} (cannot generate for progressed slot)",
                slot,
                first_non_progressed
            );
        }

        self.dispatch_collation_request(
            backend,
            slot,
            parent,
            prev_block_ids,
            timing,
            new_seqno,
            self_idx,
            pinned_deadlines,
        );
    }

    /// Dispatch a prepared collation request to the higher layer via the
    /// [`SessionCallbacks`] generate-slot handoff. Moved from `SessionProcessor`:
    /// registers the pending request, paces the pipeline, emits the
    /// COLLATION_TIMING / request log lines, builds the source-info + off-thread
    /// callback, records self-collation start telemetry, and calls
    /// [`Self::notify_generate_slot`].
    #[allow(clippy::too_many_arguments)]
    fn dispatch_collation_request(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
        prev_block_ids: Vec<BlockIdExt>,
        timing: CollationTiming,
        new_seqno: u32,
        self_idx: ValidatorIndex,
        pinned_deadlines: Option<CollationDeadlineContext>,
    ) {
        // PRECONDITION (C++ block-producer.cpp single-in-flight): no REAL collation
        // may already be active when we dispatch another. The explicit dispatch
        // guard in `execute_collation_attempt` enforces it, backed by
        // clear-before-redispatch in `on_collation_complete` /
        // `on_collation_failed_impl`, forward-only precollation chaining, and
        // `reset()`. `debug_assert!` so a future regression trips in CI/tests
        // without aborting a validator in release.
        debug_assert!(
            self.block_generation_active.is_none(),
            "single in-flight real collation invariant violated: \
             block_generation_active={:?} at real dispatch of slot {}",
            self.block_generation_active,
            slot,
        );

        // Absolute soft/hard cutoffs + budget anchor for this attempt. A genuine-error
        // restart passes the ORIGINAL context (`pinned_deadlines`) so every attempt for
        // the slot races the SAME window-end budget; a fresh dispatch derives it from the
        // slot timing. `collation_deadlines` gives the C++ soft cutoff
        // (`slot_start[+ target_rate]`) and the window-end hard cap; the budget anchor is
        // the dispatch instant (shard `slot_start - target_rate`, MC `slot_start`) so the
        // collator's percentage sub-budgets span the real intake window instead of
        // collapsing to zero on shardchains where the soft cutoff equals the slot start.
        let deadlines = pinned_deadlines.unwrap_or_else(|| {
            let (soft_deadline, hard_deadline) = collation_deadlines(
                slot,
                timing.min_gen_time,
                self.description.opts().target_rate,
                self.description.opts().slots_per_leader_window,
                self.description.get_shard().is_masterchain(),
            );
            CollationDeadlineContext {
                soft_deadline,
                hard_deadline,
                budget_anchor: timing.dispatch_time,
            }
        });
        let (request_id, request) = self.create_pending_collation_request(
            slot,
            parent.clone(),
            timing.min_gen_time,
            Some(deadlines.soft_deadline),
            Some(deadlines.hard_deadline),
            Some(deadlines.budget_anchor),
        );

        // Track this as the single in-flight REAL collation (C++
        // `block_generation_active` in block-producer.cpp). `next_slot_deadline`
        // mirrors the C++ `await_with_timeout` horizon (`slot_start + target_rate`):
        // the first slot boundary at which, if the collation is still running, the
        // per-slot wake keeps it alive and emits an empty filler.
        let next_slot_deadline = timing
            .min_gen_time
            .checked_add(self.description.opts().target_rate)
            .unwrap_or(timing.min_gen_time);
        self.block_generation_active = Some(RealCollationState {
            window: slot.window_index(self.description.opts().slots_per_leader_window.max(1)),
            slot_dispatched: slot,
            request_id,
            next_slot_deadline,
            deadlines,
        });

        // Arm the per-slot deadline wake (C++ `await_with_timeout(block_generation,
        // slot_start + target_rate)` in block-producer.cpp). Every real dispatch
        // arms it - including a genuine-error restart, which re-enters this path via
        // restart_collation and so gets its own fresh wake. The wake re-arms itself
        // for the next loop slot while the same real collation stays in flight,
        // publishing a state-preserving empty filler on each elapsed slot.
        self.arm_slot_deadline_wake(slot, request_id, next_slot_deadline);

        self.telemetry.precollation_requests_counter.increment(1);
        let now = self.now();
        self.update_collation_pacing();
        self.telemetry.collates_expire_counter.total_increment();

        log::info!(
            "Session {} COLLATION_TIMING: slot={}, request_id={}, shard={:?}, \
            parent={:?}, parent_gen_utime_ms={:?}, dispatch_at_ms={}, min_gen_time_ms={}, \
            start_collate_before_ms={}, min_gen_from_now_ms={}, dispatch_lag_ms={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            request_id,
            self.description.get_shard(),
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8])),
            timing.parent_gen_utime_ms,
            Self::system_time_ms(timing.dispatch_time),
            Self::system_time_ms(timing.min_gen_time),
            timing.start_collate_before.as_millis(),
            Self::system_time_delta_ms(timing.min_gen_time, now),
            Self::system_time_delta_ms(now, timing.dispatch_time),
        );

        log::debug!(
            "Session {} COLLATION request: slot={}, expected_seqno={}, parent={:?}",
            &self.session_id().to_hex_string()[..8],
            slot,
            new_seqno,
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
        );
        log::trace!(
            "Session {} invoke_collation: requesting block for slot={slot}, \
            expected_seqno={new_seqno}, request_id={request_id}",
            self.session_id().to_hex_string(),
        );

        let source_info = self.make_roundless_collation_source_info(self_idx);
        let callback = self.make_collation_callback(slot, request_id, request.clone());

        self.telemetry.record_collation_start();
        let now = self.now();
        self.telemetry.record_self_collation_start(
            &self.description,
            slot,
            new_seqno,
            None,
            parent.as_ref().map(|p| (p.slot, &p.hash)),
            &prev_block_ids,
            now,
        );
        self.telemetry.collates_counter.total_increment();
        self.notify_generate_slot(
            backend,
            slot,
            source_info,
            request,
            parent,
            prev_block_ids,
            callback,
        );
    }

    /// Refresh [`Self::last_consensus_finalized_at`] from the backend's
    /// consensus-finalized seqno.
    ///
    /// Mirrors C++, where the `Start` / `FinalizeBlock` handlers stamp
    /// `last_consensus_finalized_at_ = now()` independently of the producer loop
    /// in `block-producer.cpp`. Bumps the timestamp only when the finalized seqno
    /// strictly advances, so it is cheap and idempotent. Called from both the main
    /// pipeline ([`Self::check_collation`]) and the per-slot wake
    /// ([`Self::on_slot_deadline`]) so the `allow_empty` gate always reads the
    /// freshest finalization state, not a value frozen at the last
    /// `check_collation`.
    fn refresh_finalization_timestamp(&mut self, backend: &dyn CollationBackend) {
        let finalized_seqno = backend.last_consensus_finalized_seqno();
        if finalized_seqno > self.last_observed_finalized_seqno {
            self.last_observed_finalized_seqno = finalized_seqno;
            self.last_consensus_finalized_at = Some(self.now());
        }
    }

    /// C++ `allow_empty` finalization-staleness gate in `block-producer.cpp`:
    /// `!(last_consensus_finalized_at_ + no_empty_blocks_on_error_timeout_).is_in_past()`.
    ///
    /// Empty fillers are suppressed once consensus has not finalized anything for
    /// `no_empty_blocks_on_error_timeout` (default 15s): continuing to fill while
    /// finalization is wedged only deepens the stall, so the producer instead
    /// keeps waiting for a real block. `last_consensus_finalized_at` is seeded at
    /// the first observed finalization (C++ `Start`) and refreshed on every advance
    /// (C++ `FinalizeBlock`); `None` (nothing finalized yet) keeps empties
    /// suppressed, matching C++'s default-constructed (in-the-past) timestamp.
    ///
    /// The `is_first_block` half of the C++ gate is enforced separately by
    /// [`Self::emit_slot_filler`] (no parent => cannot place an empty).
    fn empties_allowed_by_finalization(&self) -> bool {
        match self.last_consensus_finalized_at {
            Some(at) => {
                match at.checked_add(self.description.opts().no_empty_blocks_on_error_timeout) {
                    // !(deadline).is_in_past()
                    Some(deadline) => self.now() <= deadline,
                    // Overflow => deadline is effectively never in the past.
                    None => true,
                }
            }
            None => false,
        }
    }

    /// Arm the per-slot deadline wake for the in-flight real collation at
    /// `loop_slot`.
    ///
    /// Mirrors the C++ `co_await await_with_timeout(block_generation.get(),
    /// slot_start + target_rate)` horizon in `block-producer.cpp`: the wake fires
    /// at `deadline`, and if the real collation `request_id` is still the active
    /// one, [`Self::on_slot_deadline`] publishes an empty filler for `loop_slot`,
    /// keeps the real alive, and re-arms the next loop slot's deadline. Posted onto
    /// the controller queue so it runs serialized against the rest of the pipeline
    /// rather than recursing.
    fn arm_slot_deadline_wake(&self, loop_slot: SlotIndex, request_id: u32, deadline: SystemTime) {
        self.queue().clone().post_delayed(deadline, move |collation, backend| {
            collation.on_slot_deadline(backend, loop_slot, request_id, deadline);
        });
    }

    /// Per-slot deadline wake handler (the C++ `await_with_timeout` timeout branch
    /// in `block-producer.cpp`).
    ///
    /// Fires once the in-flight real collation has overrun the `loop_slot`
    /// boundary. The wake is stale - and does nothing - if the tracked real
    /// collation already resolved (completion / failure / `reset`), a different
    /// real is now active, the leader window moved on, or `loop_slot` ran past the
    /// leader window (C++ `slot < end_slot`). Otherwise the slot elapsed while the
    /// real is still running: subject to the `allow_empty` gate, emit a
    /// state-preserving empty filler for `loop_slot` (keeping the real alive),
    /// advance the per-slot horizon by `target_rate`, and re-arm for the next loop
    /// slot - exactly the C++ loop publishing an empty at `slot`, advancing
    /// `parent = id`, and continuing to `await` the same `block_generation`. When
    /// the gate (or `emit_slot_filler`'s first-block check) declines the empty, the
    /// horizon still advances but the wake re-arms on the SAME loop slot, mirroring
    /// the C++ `--slot; continue;` retry.
    fn on_slot_deadline(
        &mut self,
        backend: &mut dyn CollationBackend,
        loop_slot: SlotIndex,
        request_id: u32,
        deadline: SystemTime,
    ) {
        instrument!();

        // Stale wake: the tracked real collation changed (completed / failed /
        // reset) or a different real is now active.
        let (active_window, slot_dispatched) = match self.block_generation_active.as_ref() {
            Some(active) if active.request_id == request_id => {
                (active.window, active.slot_dispatched)
            }
            _ => return,
        };

        // Leader window moved on under us: drop the stale in-flight marker (cancelling
        // its pending request) so the single-in-flight guard cannot wedge the new window,
        // then stop. `reset()` usually clears it first, but the per-slot wake can fire
        // before the next `check_collation`, so clear it here too.
        if active_window != backend.current_leader_window_idx() {
            self.clear_stale_block_generation(backend);
            return;
        }

        // The loop slot ran past the leader window (C++ `slot < end_slot`): stop
        // filling. The still-in-flight real will re-tag onto the head when it
        // completes, or be discarded if the window has changed by then.
        if self.description.get_window_idx(loop_slot) != active_window {
            return;
        }

        // Refresh the finalization timestamp from the backend so the allow_empty
        // gate reads the freshest consensus state (C++ stamps
        // last_consensus_finalized_at_ via the independent FinalizeBlock handler),
        // not a value frozen at the last check_collation.
        self.refresh_finalization_timestamp(backend);

        // The slot elapsed while the real collation is still in flight: publish a
        // state-preserving empty filler for it and keep the real alive. Two gates
        // can decline the filler, in which case we keep waiting on the SAME slot,
        // mirroring the C++ `--slot; continue;` timeout branch:
        //   * allow_empty: empties are suppressed once consensus has not finalized
        //     within no_empty_blocks_on_error_timeout;
        //   * is_first_block: a slot with no available parent cannot be empty -
        //     enforced inside emit_slot_filler.
        let emitted = if self.empties_allowed_by_finalization() {
            self.emit_slot_filler(backend, loop_slot, slot_dispatched, request_id)
        } else {
            log::debug!(
                "Session {} on_slot_deadline: suppressing empty filler for slot {} - no \
                consensus finalization within no_empty_blocks_on_error_timeout; keep waiting \
                for the real collation (request_id={})",
                &self.session_id().to_hex_string()[..8],
                loop_slot,
                request_id
            );
            false
        };
        let next_loop_slot = if emitted { loop_slot + 1 } else { loop_slot };

        // Advance the per-slot horizon by target_rate and re-arm, clamping to the
        // future so a late wake cannot busy-spin (C++ `slot_start = max(slot_start,
        // now())`).
        let target_rate = self.description.opts().target_rate;
        let now = self.now();
        let mut next_deadline = deadline.checked_add(target_rate).unwrap_or(deadline);
        if next_deadline <= now {
            next_deadline = now.checked_add(target_rate).unwrap_or(now);
        }

        if let Some(active) = self.block_generation_active.as_mut() {
            active.next_slot_deadline = next_deadline;
        }
        self.arm_slot_deadline_wake(next_loop_slot, request_id, next_deadline);
    }

    /// Publish a state-preserving empty filler for `loop_slot` while the real
    /// collation `request_id` (dispatched for `slot_dispatched`) keeps running.
    ///
    /// Mirrors the C++ await-timeout branch publishing an empty at the current
    /// loop slot and advancing `parent = id` WITHOUT disturbing the real's
    /// in-flight collation: the empty goes through
    /// [`Self::publish_candidate`] (no precollation entry), so
    /// `precollated_blocks[slot_dispatched]` and `block_generation_active` are left
    /// intact for the eventual late re-tag. The empty re-uses the parent block's
    /// `BlockIdExt` (seqno preserved), so the real - collated against that same
    /// state - re-parents cleanly onto the last filler when it completes.
    ///
    /// Returns `true` if a filler was published; `false` if none could be placed
    /// (first block in an epoch cannot be empty, the parent `BlockIdExt` is not
    /// resolvable yet, or - for a non-first filler - the head does not chain to
    /// `loop_slot`, which would fork). On `false` the caller keeps waiting on the
    /// same slot.
    fn emit_slot_filler(
        &mut self,
        backend: &mut dyn CollationBackend,
        loop_slot: SlotIndex,
        slot_dispatched: SlotIndex,
        request_id: u32,
    ) -> bool {
        // Chain off the current head when it directly precedes this loop slot in
        // the same window (C++ `parent = id`). For the very first filler fall back
        // to the parent the real itself locked at dispatch (the same underlying
        // block). A non-first filler whose head does not chain cannot place a
        // state-preserving empty without forking, so it declines.
        let loop_window = self.description.get_window_idx(loop_slot);
        let head_parent = self.local_chain_head().and_then(|head| {
            if head.window == loop_window && head.slot + 1 == loop_slot {
                Some(head.parent_info.clone())
            } else {
                None
            }
        });
        let parent = match head_parent {
            Some(p) => Some(p),
            None if loop_slot == slot_dispatched => {
                self.precollated(slot_dispatched).and_then(|pb| pb.parent.clone())
            }
            None => {
                log::warn!(
                    "Session {} emit_slot_filler: head does not chain to filler slot {} \
                    (request_id={}); declining to fill (would fork)",
                    &self.session_id().to_hex_string()[..8],
                    loop_slot,
                    request_id
                );
                return false;
            }
        };

        let Some(parent) = parent else {
            // First block in the epoch cannot be empty (C++ is_first_block).
            log::debug!(
                "Session {} emit_slot_filler: no parent for slot {} (first block in epoch \
                cannot be empty); keep waiting for real collation (request_id={})",
                &self.session_id().to_hex_string()[..8],
                loop_slot,
                request_id
            );
            return false;
        };

        let Some(parent_block_id) = self.resolve_parent_block_id(&parent, backend) else {
            log::warn!(
                "Session {} emit_slot_filler: parent BlockIdExt not resolved yet for slot {} \
                (parent={}); keep waiting (request_id={})",
                &self.session_id().to_hex_string()[..8],
                loop_slot,
                parent,
                request_id
            );
            return false;
        };

        log::debug!(
            "Session {} emit_slot_filler: real collation (request_id={}, dispatched slot {}) \
            overran slot {}; publishing state-preserving empty filler (parent={}) and keeping \
            the real alive",
            &self.session_id().to_hex_string()[..8],
            request_id,
            slot_dispatched,
            loop_slot,
            parent
        );

        // `None` funnel slot: an empty filler is not a self-collation and must not
        // consume the real's start record (keyed at `slot_dispatched`), which the
        // late re-tag needs intact.
        self.publish_candidate(
            backend,
            loop_slot,
            Some(parent),
            CollationResult::Empty { parent_block_id },
            None,
        );
        true
    }

    /// Generate an empty block for `slot` directly (no `on_generate_slot`
    /// dispatch). Moved from `SessionProcessor`: registers the pending request,
    /// records the collation start, and feeds the synthesized empty result
    /// straight into [`Self::on_collation_complete`].
    ///
    /// NOTE: the empty-block fast path is generated by simplex itself, so it is
    /// NOT a self-collation and must not be tracked by `simplex_self_collates`.
    fn finish_empty_collation_attempt(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
        min_gen_time: SystemTime,
        parent_block_id: BlockIdExt,
    ) {
        let (request_id, _request) =
            self.create_pending_collation_request(slot, parent, min_gen_time, None, None, None);
        self.telemetry.record_collation_start();
        self.on_collation_complete(
            backend,
            slot,
            request_id,
            CollationResult::Empty { parent_block_id },
        );
        self.update_collation_pacing();
    }

    /* Completion & failure */

    /// Clear the single in-flight REAL-collation marker, but only when `request_id`
    /// is that collation's request: empty fillers (and any chained precollation)
    /// carry their own request ids and must leave a still-running real collation
    /// untouched. C++ parity: `block_generation_active` is reset only when the real
    /// `block_generation` future resolves in `block-producer.cpp`.
    fn clear_block_generation_if(&mut self, request_id: u32) {
        if matches!(&self.block_generation_active, Some(s) if s.request_id == request_id) {
            self.block_generation_active = None;
        }
    }

    /// Drop the in-flight real-collation marker (and cancel its pending request) when
    /// its leader window is no longer the current one.
    ///
    /// `reset()` already clears the marker on the normal window-change path, but the
    /// stale-window early-returns in [`Self::check_collation`] and
    /// [`Self::on_slot_deadline`] can be reached BEFORE that `reset()` when the FSM
    /// progress cursor (or the local chain head) lags the consensus leader window.
    /// Without this, the marker would stay set for the dead window and the
    /// single-in-flight guard in [`Self::execute_collation_attempt`] would decline every
    /// real collation in the new window, wedging block production. Mirrors the C++ block
    /// producer replacing `cancellation_source_` / `block_generation` on each new
    /// `OurLeaderWindowStarted`. Returns `true` when a stale marker was cleared.
    fn clear_stale_block_generation(&mut self, backend: &dyn CollationBackend) -> bool {
        let current_window = backend.current_leader_window_idx();
        let Some((stale_slot, stale_window, stale_request_id)) = self
            .block_generation_active
            .as_ref()
            .filter(|s| s.window != current_window)
            .map(|s| (s.slot_dispatched, s.window, s.request_id))
        else {
            return false;
        };

        log::debug!(
            "Session {} clearing stale in-flight real collation: dispatch slot {} window {} != \
            current window {} (request_id={})",
            &self.session_id().to_hex_string()[..8],
            stale_slot,
            stale_window,
            current_window,
            stale_request_id,
        );

        // Cancel the pending AsyncRequest so the collator stops and its (now stale)
        // completion/failure callback is a no-op for the superseded window, then drop the
        // precollation entry and its self-collation telemetry like `reset()` does.
        if let Some(precollated) = self.precollated(stale_slot) {
            precollated.request.cancel();
        }
        self.remove_precollated_block(stale_slot);
        backend.forget_self_collation_tracking(stale_slot, "stale_window_block_generation_cleared");
        self.block_generation_active = None;
        true
    }

    /// Handle a successful collation result. Moved from `SessionProcessor`
    /// matching origin/master: classifies the candidate against the FSM progress
    /// cursor and current leader window inline (discard-stale / publish-current /
    /// store-future / publish-late-same-window) and drives the publication,
    /// precollation store/advance, and telemetry effects.
    pub(crate) fn on_collation_complete(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        request_id: u32,
        result: CollationResult,
    ) {
        instrument!();
        check_execution_time!(50_000);

        // The real collation has resolved; release the single-in-flight marker. Empty
        // fillers carry a different request id, so this leaves them untouched.
        // Capture whether this result IS that tracked real collation BEFORE clearing,
        // so the late re-tag below can tell a real that overran its slot (and had
        // empty fillers published past it) apart from any other late callback.
        let was_tracked_real =
            matches!(&self.block_generation_active, Some(s) if s.request_id == request_id);
        self.clear_block_generation_if(request_id);

        // C++ parity: the post-collation publication gate is the current leader
        // window, not the Rust progress cursor alone. The C++ block producer
        // continues to publish CandidateGenerated/CandidateReceived as long as
        // `current_leader_window_ == window`, even if consensus has already
        // timeout-skipped earlier slots inside that same window.
        let fsm_first_non_progressed_slot = backend.first_non_progressed_slot();
        let current_window = backend.current_leader_window_idx();
        let slot_window = self.description.get_window_idx(slot);

        if slot_window != current_window {
            log::warn!(
                "Session {} on_collation_complete: discarding stale candidate for slot {} \
                (window {} != current {}, request_id={})",
                self.session_id().to_hex_string(),
                slot,
                slot_window,
                current_window,
                request_id
            );
            backend.forget_self_collation_tracking(
                slot,
                "stale_window_result_discarded_before_publish",
            );
            self.telemetry.collates_expire_counter.success();
            self.remove_precollated_block(slot);
            return;
        }

        // Late re-tag (C++ block-producer.cpp). If this is the tracked real
        // collation and the per-slot wake already published empty fillers at/past
        // its dispatch slot, the real cannot publish at its original `slot` - an
        // empty filler already occupies it, and a second candidate there would
        // equivocate. Publish the real at the current chain position re-parented to
        // the last filler, exactly as the C++ loop emits the real at the
        // then-current `slot` with `parent = id`. The fillers preserve the parent
        // block's seqno, so the real (collated against that same state) re-parents
        // cleanly. The locked precollation entry is consumed here.
        if was_tracked_real {
            let retag = self.local_chain_head().and_then(|head| {
                if head.window == current_window && head.slot >= slot {
                    Some((head.slot + 1, head.parent_info.clone()))
                } else {
                    None
                }
            });
            if let Some((retag_slot, retag_parent)) = retag {
                log::info!(
                    "Session {} on_collation_complete: re-tagging late real from dispatch slot \
                    {} to current slot {} re-parented to filler {} (request_id={})",
                    self.session_id().to_hex_string(),
                    slot,
                    retag_slot,
                    retag_parent,
                    request_id
                );
                self.telemetry.collates_counter.success();
                // The self-collation funnel start is keyed at the dispatch `slot`;
                // emit "generated" and link against that slot, but publish at
                // `retag_slot`.
                self.telemetry.record_self_collation_generated(
                    slot,
                    "published_late_retagged_real",
                    &self.description,
                    self.now(),
                );
                self.telemetry.collates_expire_counter.failure();
                self.publish_candidate(backend, retag_slot, Some(retag_parent), result, Some(slot));
                self.remove_precollated_block(slot);
                // C++ parity: after publishing, start precollation for the next slot
                // in the same window (block-producer.cpp `++slot; parent = id;`).
                self.precollate_block(backend, retag_slot + 1);
                return;
            }
        }

        if slot == fsm_first_non_progressed_slot {
            // Process the block for the current slot immediately.
            self.telemetry.collates_counter.success();
            self.telemetry.record_self_collation_generated(
                slot,
                "published_current_slot",
                &self.description,
                self.now(),
            );

            // Track expiry: failure() means NOT expired (which is good).
            self.telemetry.collates_expire_counter.failure();

            self.generated_block(backend, slot, result);

            // C++ parity: after generating a candidate, start precollation for the
            // next slot in the same leader window (block-producer.cpp
            // `++slot; parent = id;`).
            self.precollate_block(backend, slot + 1);
        } else if slot > fsm_first_non_progressed_slot {
            // Store as precollated for a future slot.
            let mut publish_now = false;
            if let Some(precollated_block) = self.precollated_blocks.get_mut(&slot) {
                if precollated_block.result.is_some() {
                    log::error!(
                        "Session {} on_collation_complete: precollated result for slot {} \
                        already exists! (request_id={})",
                        self.session_id().to_hex_string(),
                        slot,
                        request_id
                    );
                    self.telemetry.increment_error();
                    return;
                }
                precollated_block.result = Some(result.clone());
                self.telemetry.collates_counter.success();
                self.telemetry.record_self_collation_generated(
                    slot,
                    "stored_future_slot",
                    &self.description,
                    self.now(),
                );

                log::trace!(
                    "Session {} on_collation_complete: stored precollated result for slot {} \
                    (request_id={})",
                    self.session_id().to_hex_string(),
                    slot,
                    request_id
                );

                // C++ parity: if this slot is still in the current leader window,
                // publish the candidate immediately instead of waiting until it
                // becomes `first_non_progressed_slot`.
                publish_now = self.description.get_window_idx(slot) == current_window;

                if publish_now {
                    log::trace!(
                        "Session {} on_collation_complete: publishing in-window future candidate \
                        for slot {} immediately (C++ parity)",
                        self.session_id().to_hex_string(),
                        slot
                    );
                }

                if !publish_now {
                    self.precollate_block(backend, slot + 1);
                }
            } else {
                log::warn!(
                    "Session {} on_collation_complete: no precollated entry for slot {} \
                    (request_id={})",
                    self.session_id().to_hex_string(),
                    slot,
                    request_id
                );
                backend
                    .forget_self_collation_tracking(slot, "future_slot_missing_precollation_entry");
            }

            if publish_now {
                if let Some(precollated_result) =
                    self.precollated(slot).and_then(|pb| pb.result.clone())
                {
                    self.generated_block(backend, slot, precollated_result);
                    self.precollate_block(backend, slot + 1);
                }
            }
        } else {
            // C++ parity: if the leader window is still current, publish the late
            // result anyway instead of suppressing it just because Rust's
            // `first_non_progressed_slot` already advanced inside the window.
            self.telemetry.collates_counter.success();
            self.telemetry.record_self_collation_generated(
                slot,
                "published_late_same_window",
                &self.description,
                self.now(),
            );
            self.telemetry.collates_expire_counter.failure();

            log::warn!(
                "Session {} on_collation_complete: slot {} already passed (current={}) \
                but window {} is still current; publishing late same-window candidate",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot,
                current_window
            );

            self.generated_block(backend, slot, result);
            self.precollate_block(backend, slot + 1);
        }
    }

    /// Handle a genuinely failed collation attempt — the collator returned an
    /// error, as opposed to the per-slot deadline wake, which keeps a
    /// slow-but-healthy collation alive and emits empty fillers.
    ///
    /// Faithful port of the C++ genuine-error branch in `block-producer.cpp`,
    /// after releasing the single-in-flight marker and dropping the slot if it has
    /// already progressed or its leader window moved on:
    ///   * `allow_empty` — consensus finalized within
    ///     `no_empty_blocks_on_error_timeout` AND a parent is available
    ///     (`is_first_block == false`): recover by publishing ONE empty block for
    ///     the failed slot and advancing, so the real is retried at the next slot
    ///     through the normal pipeline. If the per-slot wake already published empty
    ///     fillers past this slot, the recovery empty is re-tagged onto the current
    ///     chain head to avoid equivocating with the filler that already occupies
    ///     the dispatch slot.
    ///   * `!allow_empty` — first block, or finalization stalled past
    ///     `no_empty_blocks_on_error_timeout`: schedule one delayed restart of the
    ///     SAME slot after a short backoff ([`COLLATION_ERROR_RESTART_BACKOFF`], C++
    ///     `coro_sleep(0.1s)`).
    ///
    /// There is no attempt counter and no fixed restart cap: a restart re-dispatches
    /// through [`Self::restart_collation`] reusing the ORIGINAL attempt's soft/hard
    /// cutoffs + budget anchor ([`CollationDeadlineContext`]), so a further genuine error
    /// schedules another delayed restart, but every attempt races the SAME absolute
    /// window-end hard deadline — the leader window, not a counter, bounds the total. A
    /// genuine hard-timeout is therefore end-of-window and is not retried; there is no
    /// configurable collation retry loop.
    pub(crate) fn on_collation_failed_impl(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        request_id: u32,
        err: Error,
    ) {
        instrument!();

        // Capture the original attempt's deadline context (soft/hard cutoffs + budget
        // anchor) BEFORE releasing the single-in-flight marker, so a genuine-error
        // restart reuses the SAME window-end budget instead of recomputing a fresh
        // window-length budget from the (now advanced) clock.
        let pinned_deadlines = self
            .block_generation_active
            .as_ref()
            .filter(|s| s.request_id == request_id)
            .map(|s| s.deadlines);

        // The real collation has resolved (with an error); release the
        // single-in-flight marker before the restart re-dispatches.
        self.clear_block_generation_if(request_id);

        // `simplex_collates` counts every attempt; the self-collation funnel counts
        // the whole slot as ONE, so it records a terminal failure only when the slot
        // is dropped (progressed past) below — a restart is not terminal.
        self.telemetry.collates_counter.failure();

        // Use the FSM progress cursor to check if the slot has already progressed.
        // Collation follows notarized/skipped progress, not finalization.
        let fsm_first_non_progressed_slot = backend.first_non_progressed_slot();
        if slot < fsm_first_non_progressed_slot {
            log::warn!(
                "Session {} on_collation_failed: slot {} already passed (current={}), not \
                restarting (error: {}, request_id={})",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot,
                err,
                request_id
            );
            // Trace: collation failed because the slot progressed during the attempt.
            if let Some(tc) = &self.trace_collector {
                tc.record_collate_failed(
                    self.session_id(),
                    slot,
                    "slot_progressed_past_during_attempt",
                );
            }
            self.telemetry.record_self_collation_final_failure(
                slot,
                "slot_progressed_past_during_attempt",
                &self.description,
                self.now(),
            );
            self.remove_precollated_block(slot);
            return;
        }

        // Window moved on (C++ block-producer.cpp): a real that errored after its
        // leader window ended cannot publish anything — neither a recovery empty nor a
        // restart, since the next window's producer owns the chain now. Drop it,
        // mirroring the stale-window discard in on_collation_complete.
        let current_window = backend.current_leader_window_idx();
        let slot_window = self.description.get_window_idx(slot);
        if slot_window != current_window {
            log::warn!(
                "Session {} on_collation_failed: discarding slot {} - window {} != current {} \
                (error: {}, request_id={})",
                self.session_id().to_hex_string(),
                slot,
                slot_window,
                current_window,
                err,
                request_id
            );
            // Trace: collation dropped because its leader window moved on.
            if let Some(tc) = &self.trace_collector {
                tc.record_collate_failed(
                    self.session_id(),
                    slot,
                    &format!("stale_window_error_discarded: {}", err),
                );
            }
            self.telemetry.record_self_collation_final_failure(
                slot,
                "stale_window_error_discarded",
                &self.description,
                self.now(),
            );
            self.remove_precollated_block(slot);
            return;
        }

        // Trace: a genuine collation error for this slot. This is not a per-attempt
        // retry — the slot is either recovered with an empty block (allow_empty) or
        // restarted once below.
        if let Some(tc) = &self.trace_collector {
            tc.record_collate_failed(
                self.session_id(),
                slot,
                &format!("recover_or_restart: {}", err),
            );
        }

        log::info!(
            "Session {} COLLATION_FLOW collation_failed: expected_block_id={} slot={} reason={}",
            &self.session_id().to_hex_string()[..8],
            self.telemetry
                .self_collation_start(slot)
                .map(|(_, exp)| self.format_expected_block_id(exp))
                .unwrap_or_else(|| "unknown".to_string()),
            slot,
            err,
        );

        // Capture the parent locked at dispatch before dropping the failed entry, so
        // the recovery empty / restart can chain off the same parent the real used.
        let locked_parent = self.precollated(slot).and_then(|pb| pb.parent.clone());
        self.remove_precollated_block(slot);

        // allow_empty (C++ block-producer.cpp): when consensus has finalized within
        // no_empty_blocks_on_error_timeout AND a parent is available
        // (is_first_block == false), recover from the genuine error by publishing ONE
        // empty block for the failed slot and advancing — the real is retried at the
        // next slot via the normal pipeline. Refresh the finalization timestamp first
        // so the gate reads the freshest state (as the per-slot wake does).
        self.refresh_finalization_timestamp(backend);
        if self.empties_allowed_by_finalization() {
            // Equivocation guard / re-tag (C++ block-producer.cpp): if the
            // per-slot wake already published empty fillers at/past this slot
            // (advancing local_chain_head within the current window), the failed slot
            // is already occupied — publish the recovery empty at the current chain
            // position re-parented to the last filler instead, exactly like the
            // on_collation_complete late re-tag. Otherwise publish at the failed slot
            // chained onto the real's locked parent.
            let (publish_slot, parent) = match self.local_chain_head() {
                Some(head) if head.window == current_window && head.slot >= slot => {
                    (head.slot + 1, Some(head.parent_info.clone()))
                }
                _ => (slot, locked_parent),
            };
            if let Some(parent) = parent {
                if let Some(parent_block_id) = self.resolve_parent_block_id(&parent, backend) {
                    log::warn!(
                        "Session {} on_collation_failed: collation error for slot {}, recovering \
                        with an empty block at slot {} (allow_empty) (error: {}, request_id={})",
                        self.session_id().to_hex_string(),
                        slot,
                        publish_slot,
                        err,
                        request_id
                    );
                    // The real self-collation for `slot` produced no block; close its
                    // funnel as a (recovered) failure and publish the empty as a
                    // non-self-collation filler (None funnel), mirroring the wake filler.
                    self.telemetry.record_self_collation_final_failure(
                        slot,
                        &format!("collation_error_recovered_with_empty: {err}"),
                        &self.description,
                        self.now(),
                    );
                    self.publish_candidate(
                        backend,
                        publish_slot,
                        Some(parent),
                        CollationResult::Empty { parent_block_id },
                        None,
                    );
                    // C++ parity: after publishing, start precollation for the next
                    // slot (block-producer.cpp `++slot; parent = id;`), which
                    // re-dispatches the real.
                    self.precollate_block(backend, publish_slot + 1);
                    return;
                }
            }
            // allow_empty held but no resolvable parent (is_first_block): fall through
            // to the restart path below.
        }

        let restart_at = self.now() + COLLATION_ERROR_RESTART_BACKOFF;
        log::warn!(
            "Session {} on_collation_failed: collation error for slot {}, scheduling one delayed \
            restart in {:?} bounded by the shared window-end budget (error: {}, request_id={})",
            self.session_id().to_hex_string(),
            slot,
            COLLATION_ERROR_RESTART_BACKOFF,
            err,
            request_id
        );

        // Schedule one delayed restart for this error after the backoff. The gate — slot
        // progressed, a real collation already back in flight (single in-flight),
        // superseded by a later precollated slot, or already completed — is re-evaluated
        // when the delayed task fires; when it clears, the re-dispatch runs through
        // `restart_collation` (which re-arms a fresh per-slot wake) reusing the ORIGINAL
        // `pinned_deadlines`. The restart is not capped at a fixed count: a further
        // genuine error reschedules another delayed restart, but every attempt shares the
        // same absolute window-end hard deadline, so the leader window bounds the total.
        self.queue().clone().post_delayed(restart_at, move |collation, backend| {
            let fsm_first_non_progressed_slot = backend.first_non_progressed_slot();

            // Slot already passed.
            if slot < fsm_first_non_progressed_slot {
                log::trace!(
                    "Session {} on_collation_failed restart: slot {} already passed (current={}), \
                    skipping",
                    collation.session_id().to_hex_string(),
                    slot,
                    fsm_first_non_progressed_slot
                );
                return;
            }

            // A real collation is already back in flight (single in-flight guard);
            // restarting would only be declined by `execute_collation_attempt`.
            if collation.block_generation_active.is_some() {
                log::trace!(
                    "Session {} on_collation_failed restart: a real collation is already in \
                    flight, skipping restart of slot {}",
                    collation.session_id().to_hex_string(),
                    slot
                );
                return;
            }

            // Not the max precollated slot (another slot was started after this one).
            if let Some(max_slot) = collation.precollated_blocks_max_slot {
                if slot != max_slot {
                    log::trace!(
                        "Session {} on_collation_failed restart: slot {} is not max precollated \
                        slot (max={}), skipping",
                        collation.session_id().to_hex_string(),
                        slot,
                        max_slot
                    );
                    return;
                }
            }

            // Already precollated (completed while we were waiting).
            if let Some(precollated) = collation.precollated_blocks.get(&slot) {
                if precollated.result.is_some() {
                    log::trace!(
                        "Session {} on_collation_failed restart: slot {} already precollated, \
                        skipping",
                        collation.session_id().to_hex_string(),
                        slot
                    );
                    return;
                }
            }

            log::trace!(
                "Session {} on_collation_failed restart: restarting slot {}",
                collation.session_id().to_hex_string(),
                slot
            );
            collation.restart_collation(backend, slot, pinned_deadlines);
        });
    }

    /* Block generation & chaining */

    /// Process a successfully generated block for its original request slot.
    ///
    /// Thin precollation-aware wrapper over [`Self::publish_candidate`]: resolve
    /// the parent locked at collation start, drop the precollation entry, then
    /// publish at the request `slot` chained onto that locked parent. This is the
    /// common path (a collation that completed before its slot's soft horizon).
    /// The per-slot empty filler and the late-real re-tag bypass this wrapper and
    /// call [`Self::publish_candidate`] directly with an explicit slot/parent so
    /// they can publish off the current chain head without disturbing
    /// `precollated_blocks`.
    ///
    /// Reference: C++ block-producer.cpp generate_candidates() loop.
    fn generated_block(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        result: CollationResult,
    ) {
        // Get the parent from the precollated block BEFORE removing it. The
        // parent was locked at collation start to avoid races with consensus
        // events.
        let parent = self.precollated(slot).and_then(|pb| pb.parent.clone());

        // Remove from precollated blocks.
        self.remove_precollated_block(slot);

        // A real collation completing at its own request slot: the self-collation
        // funnel start record is keyed at that slot.
        self.publish_candidate(backend, slot, parent, result, Some(slot));
    }

    /// Publish a generated candidate at `slot` chained onto `parent`,
    /// independent of the precollation store.
    ///
    /// Carved out of [`Self::generated_block`] so the per-slot empty filler and
    /// the late-real re-tag can publish at a slot and parent that differ from the
    /// collation's original request slot / locked parent — exactly the C++
    /// `build_id_with(slot)` / `parent = id` decoupling in `block-producer.cpp` —
    /// without touching `precollated_blocks`.
    ///
    /// Validates leader-window freshness, builds the descriptor (normal or
    /// empty), persists candidate-info, seeds the synchronous generated-parent
    /// cache + window-local chain head BEFORE the async self-receive loop,
    /// broadcasts, loops the candidate back through `on_candidate_received`, and
    /// updates the per-slot generated state. DB persist / broadcast /
    /// self-receive / per-slot writes go through the borrowing
    /// [`CollationBackend`].
    ///
    /// `self_collation_funnel_slot` selects the slot whose self-collation start
    /// record this publish links to (so end-to-end self-collation acceptance can
    /// be matched on the candidate id): `Some(request_slot)` for a real collation
    /// (the common path and the late re-tag both pass the original request slot,
    /// not `slot`), `None` for an empty filler (an empty is NOT a self-collation
    /// and must never consume a real's start record).
    fn publish_candidate(
        &mut self,
        backend: &mut dyn CollationBackend,
        slot: SlotIndex,
        parent: Option<CandidateParentInfo>,
        result: CollationResult,
        self_collation_funnel_slot: Option<SlotIndex>,
    ) {
        instrument!();
        check_execution_time!(100_000);

        // Stale window guard (C++ parity: block-producer.cpp generation loop,
        // consensus.cpp start_generation). Discard candidates whose leader window
        // has already been superseded — the collation callback arrived too late.
        let slot_window = self.description.get_window_idx(slot);
        let current_window = backend.current_leader_window_idx();
        if slot_window != current_window {
            log::warn!(
                "Session {} generated_block: discarding stale candidate for slot {} \
                (window {} != current {})",
                &self.session_id().to_hex_string()[..8],
                slot,
                slot_window,
                current_window
            );
            // Trace: generated candidate discarded because its leader window is stale.
            if let Some(tc) = &self.trace_collector {
                tc.record_collate_failed(
                    self.session_id(),
                    slot,
                    &format!(
                        "stale_leader_window slot_window={} current_window={}",
                        slot_window, current_window
                    ),
                );
            }
            self.telemetry.note_generated_candidate_validation_missed_for_slot(
                slot,
                format!(
                    "generated_block_stale_window window={slot_window} \
                    current_window={current_window}"
                ),
                &self.description,
                self.now(),
            );
            self.invalidate_local_chain_head();
            return;
        }

        // C++ parity: only the leader-window freshness gate applies here. If the
        // callback is late but still belongs to the current leader window, C++
        // still publishes CandidateGenerated/CandidateReceived.
        let fsm_first_non_progressed_slot = backend.first_non_progressed_slot();
        if slot < fsm_first_non_progressed_slot {
            log::trace!(
                "Session {} generated_block: publishing late same-window slot {} \
                (first_non_progressed_slot={})",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot
            );
        } else if slot > fsm_first_non_progressed_slot {
            log::trace!(
                "Session {} generated_block: publishing future in-window slot {} \
                (first_non_progressed_slot={})",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot
            );
        }

        log::trace!(
            "Session {} generated_block: parent for slot {}: {:?}",
            self.session_id().to_hex_string(),
            slot,
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
        );

        // Determine if this is an empty block.
        let is_empty = matches!(result, CollationResult::Empty { .. });

        // INVARIANT: empty block must have a parent (first block in epoch cannot
        // be empty).
        if is_empty && parent.is_none() {
            log::error!(
                "Session {} generated_block: empty block for slot {} has no parent \
                (first block in epoch cannot be empty)",
                self.session_id().to_hex_string(),
                slot
            );
            // Trace: empty block could not be built because no parent is available.
            if let Some(tc) = &self.trace_collector {
                tc.record_collate_failed(self.session_id(), slot, "empty_block_no_parent");
            }
            self.telemetry.increment_error();
            return;
        }

        // Process the block based on type: validate, sign, build the TL structure.
        let prepared = match &result {
            CollationResult::Block(candidate) => {
                self.create_normal_block_desc(backend, slot, candidate, &parent)
            }
            CollationResult::Empty { parent_block_id } => {
                self.create_empty_block_desc(slot, parent_block_id, &parent)
            }
        };

        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                log::error!(
                    "Session {} generated_block: failed to generate block for slot {}: {}",
                    self.session_id().to_hex_string(),
                    slot,
                    e
                );
                // Trace: preparing/building the generated candidate errored.
                if let Some(tc) = &self.trace_collector {
                    tc.record_collate_failed(
                        self.session_id(),
                        slot,
                        &format!("prepare_failed: {}", e),
                    );
                }
                self.telemetry.increment_error();
                return;
            }
        };

        // Persist candidate info (synchronously, via the collation backend). The
        // hash-data bytes are built here while `prepared` is still borrowed, so
        // only owned scalars cross the seam.
        if let Some((candidate_hash, self_idx, candidate_hash_data_bytes, signature)) =
            self.build_candidate_info_for_db(&prepared, &parent, is_empty)
        {
            backend.persist_candidate_info(
                slot,
                candidate_hash,
                self_idx,
                candidate_hash_data_bytes,
                signature,
            );
        }

        // Trace: own candidate successfully generated, persisted, and about to be
        // published. `prepared` is a `GeneratedBlockDesc` (candidate_hash +
        // block_id_ext); `parent` is `Option<CandidateParentInfo>` (slot + hash).
        if let Some(tc) = &self.trace_collector {
            let trace_id = BlockCandidateId {
                slot,
                hash: prepared.candidate_hash.clone(),
                block: prepared.block_id_ext.clone(),
            };
            let parent_id = parent.as_ref().map(|p| BlockCandidateId {
                slot: p.slot,
                hash: p.hash.clone(),
                block: ton_block::BlockIdExt::default(),
            });
            if is_empty {
                tc.record_collated_empty(self.session_id(), &trace_id);
            } else {
                tc.record_collate_finished(self.session_id(), slot, &trace_id);
            }
            tc.record_candidate_received(
                self.session_id(),
                &trace_id,
                parent_id.as_ref(),
                Some(&prepared.block_id_ext),
                true,
            );
        }

        // --- C++ candidate-chaining parity (block-producer.cpp `parent = id`) ---
        // Synchronously seed the generated-parent cache and local chain head
        // BEFORE the async on_candidate_received self-loop, so that
        // precollate_block() for the next slot in the same window can resolve the
        // parent immediately.
        let candidate_parent_info =
            CandidateParentInfo { slot, hash: prepared.candidate_hash.clone() };
        let raw_id = RawCandidateId { slot, hash: prepared.candidate_hash.clone() };
        // Link the self-collation funnel start (real collations only) to this
        // candidate id; empty fillers pass `None` so they never consume a real's
        // start record (the re-tag links the dispatch slot's record to the
        // re-tagged candidate id here).
        if let Some(funnel_slot) = self_collation_funnel_slot {
            self.telemetry.link_self_collation_candidate(funnel_slot, &raw_id, &self.description);
        }
        self.insert_generated_parent(raw_id.clone(), prepared.block_id_ext.clone());
        if let Some(gen_utime_ms) = prepared.gen_utime_ms {
            self.insert_generated_parent_gen_utime_ms(
                RawCandidateId { slot, hash: prepared.candidate_hash.clone() },
                gen_utime_ms,
            );
        }
        self.telemetry.track_generated_candidate_for_validation(raw_id.clone(), self.now());

        self.set_local_chain_head(Some(LocalChainHead {
            window: slot_window,
            slot,
            parent_info: candidate_parent_info,
            gen_utime_ms: prepared.gen_utime_ms,
        }));

        log::trace!(
            "Session {} generated_block: updated local_chain_head: window={}, slot={}, hash={}",
            &self.session_id().to_hex_string()[..8],
            slot_window,
            slot,
            &prepared.candidate_hash.to_hex_string()[..8],
        );

        // Clone the TL candidate data before broadcasting (needed for
        // on_candidate_received).
        let tl_candidate_data_for_self = prepared.tl_candidate_data.clone();

        // Broadcast to the network (synchronously, via the collation backend).
        let broadcast_slot = slot.value();
        let broadcast_hash = prepared.candidate_hash.clone();
        let broadcast_data = prepared.tl_candidate_data;
        backend.broadcast_candidate(broadcast_slot, broadcast_hash, broadcast_data);

        // DEBUG: short pattern for quick grep (COLLATION = block generation flow).
        log::debug!(
            "Session {} COLLATION success: slot={}, hash={}, empty={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &prepared.candidate_hash.to_hex_string()[..8],
            is_empty
        );
        // TRACE: method-name pattern for detailed tracking.
        log::trace!(
            "Session {} generated_block: broadcast complete for slot={slot}, hash={}, \
            empty={is_empty}, block_id={:?}",
            self.session_id().to_hex_string(),
            prepared.candidate_hash.to_hex_string(),
            prepared.block_id_ext,
        );

        // Simulate receiving our own block via on_candidate_received so it goes
        // through the same path as network-received blocks and lands in
        // received_candidates uniformly (deferred onto SXMAIN via the backend).
        let self_idx = self.description.get_self_idx().value();
        backend.self_receive_candidate(self_idx, tl_candidate_data_for_self);

        log::trace!(
            "Session {} generated_block: posted on_candidate_received for own block slot {}",
            &self.session_id().to_hex_string()[..8],
            slot
        );

        // Update per-slot state.
        backend.set_pending_generate(slot, false);
        backend.set_generated(slot, true);
        backend.set_sent_generated(slot, true);
    }

    /// Precollate a block for a future slot, keeping the collation pipeline full
    /// to minimize latency. Moved from `SessionProcessor` matching origin/master:
    /// the pipeline-depth limit, queued-slot bump, leader-window gate, leadership
    /// gate, and parent selection (window-local chain head vs FSM available base)
    /// are inline, followed by the candidate-book parent-resolvability gate and
    /// the [`Self::invoke_collation`] dispatch.
    ///
    /// Reference: validator-session/src/session_processor.rs precollate_block.
    pub(crate) fn precollate_block(&mut self, backend: &mut dyn CollationBackend, slot: SlotIndex) {
        // Pipeline-depth limit.
        let max_precollated =
            self.description.opts().slots_per_leader_window.saturating_sub(1) as usize;
        if self.precollated_count() >= max_precollated {
            log::trace!(
                "Session {} precollate_block: max precollated blocks limit {} reached",
                self.session_id().to_hex_string(),
                max_precollated
            );
            return;
        }

        // If the requested slot is already queued, advance past the pipeline head.
        let mut target_slot = slot;
        if self.precollated_contains(target_slot) {
            if let Some(max_slot) = self.precollated_max_slot() {
                if let Some(precollated) = self.precollated(max_slot) {
                    if precollated.result.is_some() {
                        target_slot = max_slot + 1;
                    }
                }
            }
        }

        // Must still be in the current leader window.
        let target_window = self.description.get_window_idx(target_slot);
        let current_window = backend.current_leader_window_idx();
        if target_window != current_window {
            log::trace!(
                "Session {} precollate_block: slot {} is in window {} (current={}), skipping",
                self.session_id().to_hex_string(),
                target_slot,
                target_window,
                current_window
            );
            return;
        }

        // Must be the leader for the target slot.
        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(target_slot);
        if leader != self_idx {
            log::trace!(
                "Session {} precollate_block: not leader for slot {} (leader={})",
                self.session_id().to_hex_string(),
                target_slot,
                leader
            );
            return;
        }

        // C++ parity: prefer the window-local chain head as parent when the
        // previous slot in the same window was just generated locally
        // (block-producer.cpp `parent = id`). Fall back to the FSM available base
        // for the first slot in a window or if the local chain head is stale.
        let parent = if let Some(head) = self.local_chain_head() {
            if head.window == target_window && head.slot + 1 == target_slot {
                log::trace!(
                    "Session {} precollate_block: using local_chain_head for slot {} \
                    (parent=s{}:{})",
                    &self.session_id().to_hex_string()[..8],
                    target_slot,
                    head.parent_info.slot,
                    &head.parent_info.hash.to_hex_string()[..8],
                );
                Some(head.parent_info.clone())
            } else {
                None
            }
        } else {
            None
        };

        let parent = if parent.is_some() {
            parent
        } else {
            // Fall back to the FSM available base.
            if !backend.has_available_parent(target_slot) {
                log::trace!(
                    "Session {} precollate_block: parent is not available for slot {}",
                    self.session_id().to_hex_string(),
                    target_slot
                );
                return;
            }
            backend.get_available_parent(target_slot)
        };

        // Parent BlockIdExt must already be resolvable before precollating.
        if let Some(ref parent_info) = parent {
            if self.resolve_parent_block_id(parent_info, backend).is_none() {
                log::trace!(
                    "Session {} precollate_block: parent BlockIdExt is not resolved yet for slot \
                    {target_slot} (parent={parent_info})",
                    self.session_id().to_hex_string(),
                );
                return;
            }
        }

        self.invoke_collation(backend, target_slot, parent);
    }

    // ----- listener notification -----

    /// Remove the precollation entry for `slot`, bumping the results counter
    /// when an entry is actually removed.
    ///
    /// Does NOT touch `self_collation_starts_by_slot` — self-collation tracking
    /// has multiple terminal semantics (success, final failure, ignore) and is
    /// managed explicitly by callers. Moved verbatim from `SessionProcessor`;
    /// reads the controller's own pipeline + telemetry directly.
    pub(crate) fn remove_precollated_block(&mut self, slot: SlotIndex) {
        if self.remove_precollated_with_log(slot) {
            self.telemetry.precollation_results_counter.increment(1);
        }
    }

    /* Receiver generate-slot dispatch */

    /// Notify the higher layer that the local validator should generate a block
    /// for `slot`.
    ///
    /// Ported from `SessionProcessor::notify_generate_slot`. Runs the inline
    /// invariant checks (one-or-two explicit prevs; for a named parent the
    /// single prev must equal the resolved parent `BlockIdExt`), logs the
    /// collation-flow handoff, then dispatches `on_generate_slot` through the
    /// held [`SessionCallbacks`]. The parent `BlockIdExt` is resolved via the
    /// book seam (`backend`); the expected-block-id log line reads
    /// `self_collation_start` from the held telemetry. Pure dispatch — no
    /// controller state is mutated, so this is `&self`.
    fn notify_generate_slot(
        &self,
        backend: &dyn CollationBackend,
        slot: SlotIndex,
        source_info: crate::BlockSourceInfo,
        request: crate::AsyncCollationRequestPtr,
        parent: Option<CandidateParentInfo>,
        prev_block_ids: Vec<BlockIdExt>,
        callback: crate::ValidatorBlockCandidateCallback,
    ) {
        check_execution_time!(20_000);

        assert!(
            !prev_block_ids.is_empty() && prev_block_ids.len() <= 2,
            "CollationController INVARIANT VIOLATION: notify_generate_slot requires one or two explicit prev blocks for slot {}",
            slot
        );
        if let Some(parent_info) = parent.as_ref() {
            let parent_block_id =
                self.resolve_parent_block_id(parent_info, backend).unwrap_or_else(|| {
                    log::error!(
                        "Session {} notify_generate_slot: parent BlockIdExt is not resolved \
                        for slot {slot} (parent={parent_info})",
                        self.session_id().to_hex_string(),
                    );
                    panic!(
                        "CollationController INVARIANT VIOLATION: unresolved parent BlockIdExt for slot {} (parent={})",
                        slot, parent_info
                    );
                });
            assert_eq!(
                prev_block_ids.len(),
                1,
                "CollationController INVARIANT VIOLATION: non-bootstrap Simplex collation must have exactly one parent"
            );
            assert_eq!(
                prev_block_ids[0], parent_block_id,
                "CollationController INVARIANT VIOLATION: explicit prev block does not match resolved parent for slot {}",
                slot
            );
        }
        log::trace!(
            "Session {} notify_generate_slot: explicit prevs for slot {}: {}",
            self.session_id().to_hex_string(),
            slot,
            prev_block_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
        );
        let expected_block_id = self
            .telemetry
            .self_collation_start(slot)
            .map(|(_, exp)| self.format_expected_block_id(exp))
            .unwrap_or_else(|| "unknown".to_string());
        log::info!(
            "Session {} COLLATION_FLOW handoff: expected_block_id={} slot={} prev_count={} \
            target=SessionListener::on_generate_slot",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            prev_block_ids.len(),
        );
        let parent_hint = CollationParentHint::Explicit(prev_block_ids);

        self.callbacks.notify_generate_slot(source_info, request, parent_hint, callback);
    }

    // ----- generated-block builders -----
}

// ======================================================================
// Collation preparation & descriptors
// ======================================================================
// Builders that turn a slot+parent into collation timing/source-info and the
// signed normal/empty block descriptors + candidate-info DB rows.
impl CollationController {
    /* Timing & source info */

    /// Resolve the parent prev-block chain and dispatch timing for a collation
    /// attempt, returning whether the attempt is [`Ready`], must be [`Deferred`]
    /// until its dispatch time, or is still [`WaitingForParent`].
    ///
    /// Pure (`&self`, side-effect-free): the caller
    /// (`SessionProcessor::execute_collation_attempt`) owns the resulting
    /// dispatch / defer / wait side effects. `candidate_book` resolves a named
    /// parent to its `BlockIdExt`; `session_start_prev_blocks` seeds the genesis
    /// (no-parent) case. The clock is read from the held `description`.
    ///
    /// [`Ready`]: CollationPreparation::Ready
    /// [`Deferred`]: CollationPreparation::Deferred
    /// [`WaitingForParent`]: CollationPreparation::WaitingForParent
    fn prepare_collation(
        &self,
        parent: Option<&CandidateParentInfo>,
        backend: &dyn CollationBackend,
        session_start_prev_blocks: &[BlockIdExt],
    ) -> CollationPreparation {
        let Some(prev_block_ids) =
            self.resolve_collation_prev_block_ids(parent, backend, session_start_prev_blocks)
        else {
            return CollationPreparation::WaitingForParent;
        };

        let timing = self.compute_collation_timing(parent, backend);
        let now = self.now();
        if timing.dispatch_time > now {
            return CollationPreparation::Deferred(timing.dispatch_time);
        }

        let new_seqno = prev_block_ids.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1;
        let is_first_session_block = parent.is_none();

        CollationPreparation::Ready(PreparedCollation {
            prev_block_ids,
            timing,
            new_seqno,
            is_first_session_block,
        })
    }

    /// Compute the dispatch / min-generation timing for a collation attempt.
    ///
    /// Pure (`&self`). `min_gen_time` is clamped to `[now, now + target_rate]`
    /// around `parent_gen_utime_ms + target_rate`; shardchains start collating
    /// `target_rate` early (`dispatch_time = min_gen_time - target_rate`) while
    /// the masterchain dispatches at `min_gen_time` (C++ parity:
    /// block-producer.cpp shard `slot_start - target_rate`, MC `slot_start`).
    pub(crate) fn compute_collation_timing(
        &self,
        parent: Option<&CandidateParentInfo>,
        backend: &dyn CollationBackend,
    ) -> CollationTiming {
        let now = self.now();
        let target_rate = self.description.opts().target_rate;

        let parent_gen_utime_ms =
            parent.and_then(|parent| self.resolve_parent_gen_utime_ms(parent, backend));
        let min_gen_time = parent_gen_utime_ms
            .and_then(|parent_gen_utime_ms| {
                UNIX_EPOCH.checked_add(
                    Duration::from_millis(parent_gen_utime_ms).saturating_add(target_rate),
                )
            })
            .map_or(now, |earliest_from_parent| {
                let latest_reasonable = now.checked_add(target_rate).unwrap_or(now);
                earliest_from_parent.max(now).min(latest_reasonable)
            });

        let start_collate_before = if self.description.get_shard().is_masterchain() {
            Duration::ZERO
        } else {
            target_rate
        };
        let dispatch_time = min_gen_time.checked_sub(start_collate_before).unwrap_or(UNIX_EPOCH);
        let dispatch_time = dispatch_time.max(now);

        CollationTiming { dispatch_time, min_gen_time, start_collate_before, parent_gen_utime_ms }
    }

    /// Resolve the previous-block id list for a collation attempt: the resolved
    /// parent `BlockIdExt` (via the generated-parent cache / `candidate_book`)
    /// for a named parent, or the session-start prev-blocks for the genesis
    /// case. Returns `None` when a named parent's block id is not resolvable yet.
    fn resolve_collation_prev_block_ids(
        &self,
        parent: Option<&CandidateParentInfo>,
        backend: &dyn CollationBackend,
        session_start_prev_blocks: &[BlockIdExt],
    ) -> Option<Vec<BlockIdExt>> {
        match parent {
            Some(parent_info) => Some(vec![self.resolve_parent_block_id(parent_info, backend)?]),
            None => Some(session_start_prev_blocks.to_vec()),
        }
    }

    // ----- collation dispatch builders -----

    /// Build the roundless [`BlockSourceInfo`](crate::BlockSourceInfo) for a
    /// self-collation request.
    ///
    /// Ported from `SessionProcessor`: the source is the local validator's
    /// public key (read from the held [`SessionDescription`]) and the priority
    /// is pinned to [`SIMPLEX_ROUNDLESS`](crate::SIMPLEX_ROUNDLESS) so
    /// `ValidatorGroup` bypasses the round-based invariants.
    fn make_roundless_collation_source_info(
        &self,
        self_idx: ValidatorIndex,
    ) -> crate::BlockSourceInfo {
        crate::BlockSourceInfo {
            source: self.description.get_source_public_key(self_idx).clone(),
            priority: crate::BlockCandidatePriority {
                round: crate::SIMPLEX_ROUNDLESS,
                first_block_round: crate::SIMPLEX_ROUNDLESS,
                priority: 0,
            },
        }
    }

    /// Build the off-thread collation completion callback.
    ///
    /// Ported from `SessionProcessor`. The returned closure runs on the
    /// collator's thread when generation finishes: it records the latency
    /// histogram, warns on slow generation, and bounces the
    /// success / failure / cancellation outcome back onto SXMAIN through the
    /// held controller queue. The success / failure re-entries run the
    /// COLLATION_FLOW callback logging (folded in from the old
    /// `complete_self_collation` / `fail_self_collation` SXMAIN trampolines) and
    /// then call [`Self::on_collation_complete`] / [`Self::on_collation_failed_impl`]
    /// directly against the freshly rebuilt `(&mut self, backend)` view; the
    /// cancellation re-entry forgets the slot's self-collation tracking through
    /// [`CollationBackend::forget_self_collation_tracking`]. Captures cheap `Arc`
    /// clones of the session description, telemetry histogram, and queue so the
    /// closure is `Send` and outlives the borrow.
    fn make_collation_callback(
        &self,
        slot: SlotIndex,
        request_id: u32,
        request: Arc<AsyncRequestImpl>,
    ) -> crate::ValidatorBlockCandidateCallback {
        let session_id = self.session_id().clone();
        let description = self.description.clone();
        let collation_latency_histogram = self.telemetry.collation_latency_histogram.clone();
        let start_time = self.now();
        let queue = self.queue().clone();
        let request_clone = request.clone();

        Box::new(move |result: ton_block::Result<ValidatorBlockCandidatePtr>| {
            if request_clone.is_cancelled() {
                log::warn!(
                    "Session {} invoke_collation: request {} for slot {} was cancelled",
                    session_id.to_hex_string(),
                    request_id,
                    slot
                );
                queue.post(move |_collation, backend| {
                    backend.forget_self_collation_tracking(
                        slot,
                        "callback_cancelled_before_generation_result",
                    )
                });
                return;
            }

            let generation_duration =
                description.get_time().duration_since(start_time).unwrap_or_default();
            collation_latency_histogram.record(generation_duration.as_millis() as f64);

            if generation_duration > MAX_GENERATION_TIME {
                log::warn!(
                    "Session {} invoke_collation: block generation took {:.3}s (expected <{:.3}s) \
                    for slot {}",
                    session_id.to_hex_string(),
                    generation_duration.as_secs_f64(),
                    MAX_GENERATION_TIME.as_secs_f64(),
                    slot
                );
            }

            queue.post(move |collation, backend| match result {
                Ok(candidate) => {
                    let expected_block_id = collation
                        .telemetry
                        .self_collation_start(slot)
                        .map(|(_, exp)| collation.format_expected_block_id(exp))
                        .unwrap_or_else(|| "unknown".to_string());
                    log::info!(
                        "Session {} COLLATION_FLOW callback: expected_block_id={} slot={} \
                        outcome=generated generation_ms={} candidate_block_id={}",
                        &collation.session_id().to_hex_string()[..8],
                        expected_block_id,
                        slot,
                        generation_duration.as_millis(),
                        candidate.id,
                    );
                    log::trace!(
                        "Session {} invoke_collation: block generated for slot {} (request_id={})",
                        collation.session_id().to_hex_string(),
                        slot,
                        request_id
                    );
                    collation.on_collation_complete(
                        backend,
                        slot,
                        request_id,
                        CollationResult::Block(candidate),
                    );
                }
                Err(err) => {
                    let expected_block_id = collation
                        .telemetry
                        .self_collation_start(slot)
                        .map(|(_, exp)| collation.format_expected_block_id(exp))
                        .unwrap_or_else(|| "unknown".to_string());
                    log::info!(
                        "Session {} COLLATION_FLOW callback: expected_block_id={} slot={} \
                        outcome=callback_failure generation_ms={} error={}",
                        &collation.session_id().to_hex_string()[..8],
                        expected_block_id,
                        slot,
                        generation_duration.as_millis(),
                        err,
                    );
                    log::warn!(
                        "Session {} invoke_collation: block generation failed for slot {slot}: \
                        {err}",
                        collation.session_id().to_hex_string(),
                    );
                    collation.on_collation_failed_impl(backend, slot, request_id, err);
                }
            });
        })
    }

    // ----- collation pipeline (orchestration) -----

    /* Block descriptors & candidate info */

    /// Create normal (non-empty) block descriptor for broadcast and FSM
    /// submission.
    ///
    /// Validates block size and seqno, computes hashes, signs, and builds the TL
    /// structure. Moved verbatim from `SessionProcessor`; parent `BlockIdExt`
    /// resolution now goes through the controller's own
    /// [`Self::resolve_parent_block_id`] (cache then book seam), and the session
    /// id / local key / options are read from the held [`SessionDescription`].
    pub(crate) fn create_normal_block_desc(
        &self,
        backend: &dyn CollationBackend,
        slot: SlotIndex,
        candidate: &crate::ValidatorBlockCandidate,
        parent: &Option<CandidateParentInfo>,
    ) -> Result<GeneratedBlockDesc> {
        let root_hash = &candidate.id.root_hash;
        let data = &candidate.data;
        let collated_data = &candidate.collated_data;
        // Compute hashes from canonical BOC representation to match C++ simplex behavior.
        // C++ leader hashes the original serialized bytes; C++ receiver hashes decompressed
        // bytes — they match because BOC serialization is deterministic given the same mode
        // flags (mode 31 for block data, mode 2 for collated data).
        // We explicitly canonicalize (deserialize → re-serialize with target flags) to
        // guarantee matching hashes even if the input BOC was serialized with different flags.
        //
        // Falls back to raw bytes if canonicalization fails (e.g., in unit tests with
        // mock data that's not valid BOC). In production, all data is valid BOC.
        let file_hash =
            match consensus_common::compression::canonicalize_boc(data.data(), BocFlags::all()) {
                Ok(canonical) => UInt256::from_slice(&sha256_digest(&canonical)),
                Err(_) => UInt256::from_slice(&sha256_digest(data.data())),
            };

        let collated_file_hash = match consensus_common::compression::canonicalize_boc(
            collated_data.data(),
            BocFlags::Crc32,
        ) {
            Ok(canonical) => UInt256::from_slice(&sha256_digest(&canonical)),
            Err(_) => UInt256::from_slice(&sha256_digest(collated_data.data())),
        };
        log::trace!(
            "Session {} create_normal_block_desc: slot={}, root_hash={:x}",
            self.session_id().to_hex_string(),
            slot,
            root_hash
        );

        // Validate sizes
        let max_block_size = self.description.opts().max_block_size;
        let max_collated_size = self.description.opts().max_collated_data_size;

        if data.data().len() > max_block_size || collated_data.data().len() > max_collated_size {
            fail!(
                "block too large ({}+{} > {max_block_size}+{max_collated_size})",
                data.data().len(),
                collated_data.data().len()
            );
        }

        // Derive expected seqno from locked parent (C++ behavior).
        // Seqno = parent_seqno + 1, or initial_block_seqno for genesis.
        //
        // Reference: C++ block-producer.cpp derive_seqno() uses parent block's seqno.
        let expected_seqno = match parent {
            None => {
                // Genesis block: use initial_block_seqno from session initialization.
                // This is the seqno of the first block in the epoch.
                let initial_seqno = self.description.get_initial_block_seqno();
                log::trace!(
                    "Session {} create_normal_block_desc: genesis block, seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    initial_seqno
                );
                initial_seqno
            }
            Some(parent_info) => {
                // Non-genesis: derive seqno from parent's BlockIdExt (parent_seqno + 1).
                // Look up parent's BlockIdExt from the generated-parent cache / book seam.
                let parent_block_id =
                    self.resolve_parent_block_id(parent_info, backend).ok_or_else(|| {
                        // Parent BlockIdExt not resolved - should not happen (checked in check_collation)
                        error!(
                            "parent BlockIdExt not resolved \
                            for parent {parent_info} at slot {slot}"
                        )
                    })?;
                parent_block_id.seq_no + 1
            }
        };

        // Validate seqno matches expected
        let candidate_seqno = candidate.id.seq_no;
        if candidate_seqno != expected_seqno {
            fail!(
                "seqno mismatch: candidate has seqno={candidate_seqno}, \
                expected={expected_seqno} (derived from parent={:?})",
                parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
            );
        }

        // Construct BlockIdExt for hash computation
        let block_id = BlockIdExt {
            shard_id: self.description.get_shard().clone(),
            seq_no: expected_seqno,
            root_hash: root_hash.clone(),
            file_hash: file_hash.clone(),
        };

        // Compute parent info for hash
        let parent_info: Option<(SlotIndex, &UInt256)> = parent.as_ref().map(|p| (p.slot, &p.hash));

        // Compute candidate hash
        let candidate_hash = crate::utils::compute_candidate_id_hash(
            slot,
            Some(&block_id),
            Some(&collated_file_hash),
            parent_info,
        );

        // Sign candidate
        let signature = crate::utils::sign_candidate(
            &self.session_id(),
            slot,
            &candidate_hash,
            self.description.get_local_key(),
        )
        .map_err(|e| error!("failed to sign candidate: {e}"))?;

        // Build TL candidate for broadcast
        // C++ simplex always uses compressed candidates (compression_enabled=true hardcoded).
        // Serialize as validatorSession.compressedCandidate (LZ4+BOC merged roots).
        let (compressed, decompressed_size) =
            consensus_common::compression::compress_candidate_data(
                data.data(),
                collated_data.data(),
            )?;
        let tl_block_candidate = CompressedCandidate {
            src: UInt256::default(),
            round: candidate_seqno as i32,
            root_hash: root_hash.clone(),
            data: compressed,
            decompressed_size: decompressed_size as i32,
        };
        let candidate_bytes =
            consensus_common::serialize_tl_boxed_object!(&tl_block_candidate.into_boxed());

        // Parent info for TL - use CandidateParent wrapper
        let tl_parent = match parent {
            Some(p) => CandidateParent {
                id: CandidateId { slot: p.slot.value() as i32, hash: p.hash.clone() }.into_boxed(),
            }
            .into_boxed(),
            None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
        };

        let tl_candidate_data = CandidateData::Consensus_Block(CandidateDataBlock {
            slot: slot.value() as i32,
            candidate: candidate_bytes,
            parent: tl_parent,
            signature: signature.clone(),
        });

        // Compute actual file hashes for FSM
        let computed_file_hash = consensus_common::utils::get_hash_from_block_payload(data);
        let computed_collated_file_hash =
            consensus_common::utils::get_hash_from_block_payload(collated_data);
        let gen_utime_ms = crate::utils::extract_consensus_gen_utime_ms(collated_data.data());

        let block_id_ext = BlockIdExt {
            shard_id: self.description.get_shard().clone(),
            seq_no: expected_seqno,
            root_hash: root_hash.clone(),
            file_hash: computed_file_hash,
        };

        // Create block candidate for FSM
        let block_candidate = crate::block::BlockCandidate {
            id: block_id_ext.clone(),
            collated_file_hash: computed_collated_file_hash,
            data: data.data().to_vec(),
            collated_data: collated_data.data().to_vec(),
            creator: self
                .description
                .get_source_public_key(self.description.get_self_idx())
                .clone(),
        };

        Ok(GeneratedBlockDesc {
            block_id_ext,
            block_candidate: Some(block_candidate),
            candidate_hash,
            tl_candidate_data,
            signature,
            gen_utime_ms,
        })
    }

    /// Create empty block descriptor for broadcast and FSM submission.
    ///
    /// Empty blocks re-sign the previous block's `BlockIdExt` for finalization
    /// recovery. Moved verbatim from `SessionProcessor`; session id / local key
    /// are read from the held [`SessionDescription`]. Reference: C++
    /// `block-producer.cpp` generate_candidates() empty block branch.
    fn create_empty_block_desc(
        &self,
        slot: SlotIndex,
        parent_block_id: &BlockIdExt,
        parent: &Option<CandidateParentInfo>,
    ) -> Result<GeneratedBlockDesc> {
        log::debug!(
            "Session {} create_empty_block_desc: slot={}, parent_block_id={:?}",
            &self.session_id().to_hex_string()[..8],
            slot,
            parent_block_id
        );

        // INVARIANT: Empty blocks require parent (checked in generated_block)
        let p = parent
            .as_ref()
            .ok_or_else(|| error!("empty block must have parent for hash computation"))?;

        // For empty blocks, use candidateHashDataEmpty TL type (different from candidateHashDataOrdinary)
        // Reference: C++ CandidateId::create_hash_data() uses consensus_candidateHashDataEmpty
        let candidate_hash =
            crate::utils::compute_candidate_id_hash_empty(parent_block_id, (p.slot, &p.hash));

        // Sign candidate
        let signature = crate::utils::sign_candidate(
            &self.session_id(),
            slot,
            &candidate_hash,
            self.description.get_local_key(),
        )
        .map_err(|e| error!("failed to sign candidate: {e}"))?;

        // Build TL candidate for broadcast
        // consensus.empty uses CandidateId directly (not CandidateParent wrapper)
        let parent = CandidateId { slot: p.slot.value() as i32, hash: p.hash.clone() }.into_boxed();

        let tl_candidate_data = CandidateData::Consensus_Empty(CandidateDataEmpty {
            slot: slot.value() as i32,
            parent,
            block: parent_block_id.clone(),
            signature: signature.clone(),
        });

        Ok(GeneratedBlockDesc {
            block_id_ext: parent_block_id.clone(),
            block_candidate: None,
            candidate_hash,
            tl_candidate_data,
            signature,
            gen_utime_ms: None,
        })
    }

    /// Assemble the candidate-info record persisted for a locally generated
    /// block.
    ///
    /// Synchronous companion to the [`CollationBackend::persist_candidate_info`]
    /// effect: runs in `generated_block` while the heavy [`GeneratedBlockDesc`]
    /// is still borrowed and returns only the owned scalars the DB write needs —
    /// `(candidate_hash, self_idx, candidate_hash_data_bytes, signature)` — so no
    /// candidate body is cloned across the backend seam. Returns `None` (logging
    /// the reason) when the descriptor is internally inconsistent (an empty block
    /// without a parent, or a normal block without a candidate body). Moved
    /// verbatim from `SessionProcessor`; self index / session id read from the
    /// held [`SessionDescription`].
    fn build_candidate_info_for_db(
        &self,
        prepared: &GeneratedBlockDesc,
        parent: &Option<CandidateParentInfo>,
        is_empty: bool,
    ) -> Option<(UInt256, ValidatorIndex, Vec<u8>, Vec<u8>)> {
        let self_idx = self.description.get_self_idx();
        let parent_info = parent.as_ref().map(|p| (p.slot, &p.hash));
        let candidate_hash_data_bytes = if is_empty {
            let Some(p) = parent.as_ref() else {
                log::error!(
                    "Session {} build_candidate_info_for_db: empty block must have parent",
                    &self.session_id().to_hex_string()[..8],
                );
                return None;
            };
            crate::utils::build_candidate_hash_data_bytes_empty(
                &prepared.block_id_ext,
                (p.slot, &p.hash),
            )
        } else {
            let Some(block_candidate) = prepared.block_candidate.as_ref() else {
                log::error!(
                    "Session {} build_candidate_info_for_db: normal block must have \
                    block_candidate",
                    &self.session_id().to_hex_string()[..8],
                );
                return None;
            };
            let collated_file_hash = block_candidate.collated_file_hash.clone();
            crate::utils::build_candidate_hash_data_bytes(
                Some(&prepared.block_id_ext),
                Some(&collated_file_hash),
                parent_info,
            )
        };

        Some((
            prepared.candidate_hash.clone(),
            self_idx,
            candidate_hash_data_bytes,
            prepared.signature.clone(),
        ))
    }

    /// Expected block id for collation-flow logging: `{shard}:{expected_seqno}`.
    ///
    /// Reads the shard from the held [`SessionDescription`]. Mirrors the
    /// `SessionProcessor` helper of the same name.
    fn format_expected_block_id(&self, expected_seqno: u32) -> String {
        format!("{}:{}", self.description.get_shard(), expected_seqno)
    }
}

// ======================================================================
// Small helpers
// ======================================================================
// Stateless formatting / time-arithmetic helpers.
impl CollationController {
    /// Milliseconds since the Unix epoch (collation-timing log helper). Moved
    /// verbatim from `SessionProcessor`.
    fn system_time_ms(time: SystemTime) -> u128 {
        time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
    }

    /// Signed millisecond delta `later - earlier` (collation-timing log helper).
    /// Moved verbatim from `SessionProcessor`.
    fn system_time_delta_ms(later: SystemTime, earlier: SystemTime) -> i128 {
        match later.duration_since(earlier) {
            Ok(delta) => delta.as_millis() as i128,
            Err(err) => -(err.duration().as_millis() as i128),
        }
    }
}

impl std::fmt::Debug for CollationController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollationController")
            .field("precollated_count", &self.precollated_blocks.len())
            .field("precollated_blocks_max_slot", &self.precollated_blocks_max_slot)
            .field("earliest_collation_time", &self.earliest_collation_time)
            .field("local_chain_head_window", &self.local_chain_head.as_ref().map(|h| h.window))
            .field("last_generated_slot", &self.last_generated_slot)
            .field("generated_parent_cache", &self.generated_parent_cache.len())
            .field(
                "generated_parent_gen_utime_ms_cache",
                &self.generated_parent_gen_utime_ms_cache.len(),
            )
            .field("block_generation_active", &self.block_generation_active)
            .field("last_consensus_finalized_at", &self.last_consensus_finalized_at)
            .finish_non_exhaustive()
    }
}

// ======================================================================
// Tests
// ======================================================================
// Test-only accessors / seams consolidated under one `#[cfg(test)]` impl so the
// production impls carry no test scaffolding.
#[cfg(test)]
impl CollationController {
    /* Callback injection */

    /// Re-point the controller at a rebuilt callbacks aspect.
    ///
    /// Test-only seam used by `SessionProcessor::set_listener_for_test`: when a
    /// test swaps the session listener it rebuilds the shared
    /// `Arc<SessionCallbacks>`, so this controller's clone must be re-pointed at
    /// the new instance to keep generate-slot dispatch reaching the test's
    /// recording listener. Mirrors `ValidationController::set_callbacks_for_test`.
    pub(crate) fn set_callbacks_for_test(&mut self, callbacks: Arc<SessionCallbacks>) {
        self.callbacks = callbacks;
    }

    /* State probes */

    /// True if the precollation pipeline is empty.
    ///
    /// Used by tests; non-test code goes through `precollated_count()`.
    pub(crate) fn precollated_is_empty(&self) -> bool {
        self.precollated_blocks.is_empty()
    }

    /// Look up a mutable reference to the precollation entry for `slot`.
    ///
    /// Used by tests; `on_collation_complete` takes `&mut self.precollated_blocks`
    /// directly so it can keep mutating the entry while also borrowing the
    /// disjoint `telemetry` / `description` fields (which this whole-`&mut self`
    /// accessor would forbid).
    fn precollated_mut(&mut self, slot: SlotIndex) -> Option<&mut PrecollatedBlock> {
        self.precollated_blocks.get_mut(&slot)
    }

    /// True if the generated-parent metadata caches are both empty.
    ///
    /// Used by tests to verify `invalidate_local_chain_head` /
    /// `reset_precollations` cleared the caches.
    pub(crate) fn generated_parent_cache_is_empty(&self) -> bool {
        self.generated_parent_cache.is_empty()
            && self.generated_parent_gen_utime_ms_cache.is_empty()
    }
}

/*
    Tests live in a sibling file but are included directly via `#[path]` so
    they can reach the private accessor surface and the inner registry
    types without widening visibility. Mirrors the convention used by
    `session_runtime.rs` / `candidate_book.rs` / `database_controller.rs`.
*/

#[cfg(test)]
#[path = "tests/test_collation_controller.rs"]
mod tests;
