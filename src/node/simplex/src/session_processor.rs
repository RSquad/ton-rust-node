/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Session processor implementation for Simplex consensus
//!
//! `SessionProcessor` is the single-threaded (SXMAIN) coordinator that wires the
//! [`SimplexState`] FSM kernel to the network layer and the higher-level
//! validator callbacks. It owns no consensus *policy* of its own: each consensus
//! phase lives on a dedicated controller, and the cross-cutting session state
//! lives on focused aspects. The processor drives the main loop, routes FSM
//! events, and keeps only the orchestration that genuinely spans subsystems.
//! This module is crate-private.
//!
//! C++ cross-reference: [ton-blockchain/ton](https://github.com/ton-blockchain/ton)
//! `validator/consensus/simplex` (testnet). The main-loop analogue is
//! `consensus.cpp` / `pool.cpp`; per-phase parity notes live on the owning
//! controller and on individual methods.
//!
//! # Composition
//!
//! - Kernel — [`SimplexState`]: the deterministic FSM (votes/certs in,
//!   [`SimplexEvent`]s out).
//! - Phase controllers, reached through the `with_*_backend` split-borrow seams:
//!   - [`CollationController`] — block generation, precollation, candidate
//!     publishing.
//!   - [`ValidationController`] — the candidate-validation pipeline.
//!   - [`ConsensusController`] — vote/cert ingress + outbound, FSM finalization
//!     handlers, the recursive finalization walk, and MC applied-top tracking.
//! - Session aspects, reached through accessors: [`SessionRuntime`] (slot map,
//!   delayed-action scheduler, wake horizon, bootstrap handles),
//!   [`SessionTelemetry`] (metrics + diagnostics), [`CandidateBook`] (received
//!   candidates + data caches), [`DatabaseController`] (async-DB write registry +
//!   DB handle), and [`SessionCallbacks`] (listener dispatch). Network I/O lives
//!   on [`crate::receiver`].
//!
//! ```text
//!   network ─▶ Receiver ─▶ ReceiverListener
//!                            on_vote / on_certificate / on_candidate_received
//!                                   │
//!                                   ▼
//!   SessionProcessor (SXMAIN coordinator)
//!     check_all() main loop ──drives──▶ SimplexState (FSM kernel)
//!                                              │ emits SimplexEvent
//!     process_simplex_events() ◀───────────────┘
//!        ├─ BroadcastVote .......................▶ broadcast_vote
//!        ├─ SlotSkipped .........................▶ handle_slot_skipped
//!        └─ BlockFinalized / NotarizationReached /
//!           SkipCertificateReached / FinalizationReached ─▶ ConsensusController
//!
//!     phase controllers : Collation · Validation · Consensus  (via with_*_backend)
//!     session aspects   : Runtime · Telemetry · CandidateBook · DatabaseController · Callbacks
//!
//!   SessionProcessor ─▶ Receiver ─▶ network  (broadcast votes / candidates / certs)
//! ```
//!
//! # `check_all()` ordering
//!
//! The main loop runs, in order: release delayed gates
//! (`process_delayed_actions`) → drain completed async-DB continuations
//! (`process_pending_async_db_results`) → `check_validation` → feed validated
//! candidates to the FSM (`process_validated_candidates`) →
//! `SimplexState::check_all` (timeouts + pending blocks) →
//! `process_simplex_events` → re-sync receiver standstill slots → recompute the
//! wake horizon → `maybe_store_pool_state` → `check_collation`. Validated
//! candidates are fed *before* timeout processing (mirrors C++ `process_blocks()`
//! running before the round timer), and collation runs last so it observes the
//! freshest progress cursor.
//!
//! # Consensus loop
//!
//! Each slot: `Collate → Broadcast → Validate → Notarize → Vote → Collect →
//! Finalize → Deliver`. See `README.md` "Consensus Loop" for the
//! phase-to-method mapping table.
//!
//! # FSM event routing
//!
//! `process_simplex_events` keeps `BroadcastVote` and `SlotSkipped` on
//! `SessionProcessor` — they drive cross-subsystem orchestration (vote
//! durability + relay; per-slot reset + candidate-repair cancel + standstill
//! sync + precollation prune) that no single controller owns. The four
//! finalization/cert events route into `ConsensusController` through the
//! borrowing backend.

// Collation-domain types live on `CollationController`; the session processor only
// references `CollationResult` from the consolidated `#[cfg(test)] impl
// SessionProcessor` block at the end of this file.
#[cfg(test)]
use crate::collation_controller::CollationResult;
// `BlockFinalizedEvent` is named only by the consolidated `#[cfg(test)] impl
// SessionProcessor` block at the end of this file; production dispatch matches
// `SimplexEvent::BlockFinalized(_)` without naming the inner type.
#[cfg(test)]
use crate::simplex_state::BlockFinalizedEvent;
// `TaskPtr` is referenced only by the `#[path]`-included unit tests (via
// `super::*`); production validation/collation callbacks now post through the
// controller queues rather than naming `TaskPtr` directly.
#[cfg(test)]
use crate::task_queue::TaskPtr;
use crate::{
    block::{
        CandidateId as BlockCandidateId, RawCandidate, RawCandidateId, SlotIndex, ValidatorIndex,
        WindowIndex,
    },
    candidate_book::{
        CandidateBook, ParentTipResolution, ReceivedCandidate, EMPTY_CHAIN_WARN_DEPTH,
        MAX_CHAIN_DEPTH,
    },
    collation_controller::{CollationBackend, CollationController},
    consensus_controller::{ConsensusBackend, ConsensusController},
    controller_queue::{ControllerQueue, ControllerQueuePtr, ControllerTask},
    database::{FinalizedBlockRecord, PoolStateRecord, SimplexDbPtr, VoteRecord},
    database_controller::{DatabaseController, PendingAsyncDbId, PendingDrainStep},
    receiver::{ReceiverPtr, StandstillCertificateType, StandstillTriggerNotification},
    session_callbacks::SessionCallbacks,
    session_description::SessionDescription,
    session_runtime::{SessionRuntime, MAX_AWAKE_TIMEOUT},
    session_telemetry::{
        CandidateTotals, ConsensusStateSnapshot, DumpStatusSnapshot, FullDumpSnapshot,
        HealthCheckSnapshot, SessionTelemetry, ValidationInventoryEntry,
        ValidationInventorySnapshot,
    },
    simplex_state::{
        FinalizationReachedEvent, NotarizationReachedEvent, SimplexEvent, SimplexState,
        SkipCertificateReachedEvent, Vote,
    },
    startup_recovery::StartupRecoveryBackend,
    task_queue::TaskQueuePtr,
    trace_collector::TraceCollector,
    utils::extract_consensus_gen_utime_ms,
    validation_controller::{ValidationBackend, ValidationController},
    MetricsHandle, RawVoteData, SessionId, ValidatorWeight,
};
use consensus_common::{
    check_execution_time, instrument, CandidateObservedFlags, EnsureCandidateAvailabilityOptions,
    StorageAsyncResultPtr, StorageResultAlreadyTaken,
};
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime},
};
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::consensus::{
        candidatedata::Empty as CandidateDataEmpty,
        candidateid::CandidateId,
        simplex::{
            candidateandcert::CandidateAndCert, vote::Vote as TlVote, Certificate,
            Vote as TlVoteBoxed, VoteSignatureSet as VoteSignatureSetBoxed,
        },
        CandidateData, CandidateHashData,
    },
    IntoBoxed,
};
use ton_block::{error, sha256_digest, BlockIdExt, Error, Result, UInt256};

/*
    Constants
*/

/// Period without finalizations before triggering debug dump (stalled consensus detection)
/// Matches validator-session ROUND_DEBUG_PERIOD
pub(crate) const ROUND_DEBUG_PERIOD: Duration = Duration::from_secs(15);

/// Maximum history slots to keep in candidate/certificate caches
/// Old entries are cleaned up when slot is finalized
const MAX_HISTORY_SLOTS: u32 = 1024;

/// Delay before requesting a missing candidate from peers
/// This allows time for the broadcast to arrive naturally before triggering a query
const CANDIDATE_REQUEST_DELAY: Duration = Duration::from_secs(1);

/// Minimum interval between repeated `requestCandidate` attempts for the same (slot,hash).
///
/// Under network partitions, a single request may time out; we must retry, but not spam.
const CANDIDATE_REQUEST_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Delay between deferred retries of `ensure_candidate_available` when the
/// `BlockIdExt → RawCandidateId` mapping is not yet known.
const RESOLVER_AVAILABILITY_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Maximum number of deferred retries before giving up on resolving a
/// `BlockIdExt` to `RawCandidateId` for the resolver.
const RESOLVER_AVAILABILITY_MAX_RETRIES: u32 = 6;

/// Polling cadence for the SXMAIN pending-async-DB-results registry.
///
/// When a registered async DB op is not yet ready, `process_pending_async_db_results()`
/// schedules the next `check_all()` wake at `now + ASYNC_DB_POLL_DELAY` so SXMAIN can
/// re-poll without blocking. This mirrors the `delayed_actions` cadence pattern.
const ASYNC_DB_POLL_DELAY: Duration = Duration::from_millis(5);

/// Default deadline for write-durability continuations registered through
/// `post_async_db_result()`. Long enough to absorb routine RocksDB latency
/// spikes without false-flagging a stuck write, short enough to surface a
/// genuinely hung writer well before the next finalization stalls.
const DEFAULT_ASYNC_DB_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum wall-clock time `SessionProcessor::stop()` waits for the SXMAIN
/// `pending_async_db_results` registry to drain before forcing teardown.
///
/// C++ parity: `bridge.cpp::destroy_inner()` publishes `StopRequested` and then
/// `co_await`s `bus_->db->close()`, which forces the db actor to drain its
/// queued `co_await db->set(...)` tasks before the bus is dropped. Mirroring
/// that here lets continuations registered via `post_async_db_result()` run
/// (with Ok or Err) on SXMAIN before the session is dropped, instead of being
/// silently abandoned together with the registry.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Polling interval used by `drain_pending_async_db_results_for_shutdown()`
/// while waiting for the storage thread to flush in-flight writes.
const SHUTDOWN_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Maximum wall-clock time `SessionProcessor::stop()` blocks waiting for
/// `SimplexDb::sync()` (the underlying `AsyncKeyValueStorage::sync()`).
///
/// `SimplexDb::drop()` also calls `sync()` as a safety net (with the storage
/// layer's `DEFAULT_SYNC_TIMEOUT = 5s`); the longer deadline here is
/// intentional, because we want a stuck writer to surface at the
/// SessionProcessor layer (with explicit logging + error counter increment)
/// instead of being absorbed by the implicit Drop sync.
const SHUTDOWN_DB_SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// SessionProcessor always enforces C++ `WaitForParent` readiness before dispatching validation.
///
/// Masterchain stale-parent protection remains validator-side, matching the C++ split where
/// simplex waits for parent/skip readiness and `block-validator.cpp` owns accepted-head checks.

/// Maximum number of recently-finalized blocks to show in validation section dump.
const RECENT_FINALIZED_DUMP_WINDOW: Duration = Duration::from_secs(10);

/// SXMAIN coordinator for a Simplex consensus session.
///
/// Wires the `SimplexState` FSM to the phase controllers (collation / validation
/// / consensus) and the session aspects (runtime / telemetry / candidate-book /
/// database / callbacks); see the module docs for the architecture. All
/// operations are single-threaded.
///
/// Per-slot scratch (`SlotMap` / `SlotEntry` / `SlotRuntime`) lives in
/// [`crate::session_runtime`]; call sites reach it through the slot operations on
/// `SessionRuntime` (e.g. `self.runtime.is_generated(slot)`).
pub(crate) struct SessionProcessor {
    /// Task queue for main processing
    task_queue: TaskQueuePtr,
    /// Receiver for network communication
    receiver: ReceiverPtr,
    /// SimplexState FSM - core consensus state machine
    simplex_state: SimplexState,

    /// Per-session consensus controller (see [`crate::consensus_controller`]).
    /// Owns the finalization journal (finalized-head cursor + before-split map +
    /// last-consensus-finalized seqno + finalized/pending/delivered dedup sets),
    /// the FSM finalization/cert handlers + recursive parent-chain walk, the MC
    /// applied-top pipeline (`set_mc_finalized_block` + `last_mc_finalized_seqno`
    /// / accepted-normal-head), vote/certificate ingress (`on_vote` /
    /// `on_certificate` + `misbehavior_reports`), and outbound vote sign/send
    /// (`broadcast_vote_after_persist`). Driven through `with_consensus_backend`
    /// (see [`ConsensusBackend`]); the journal is read via the controller's
    /// `pub(crate)` accessors. `SessionProcessor` keeps only the `broadcast_vote`
    /// durability-wait shell (it crosses the `DatabaseController` async-DB registry).
    consensus: ConsensusController,

    /// Candidate-request throttle map: `RawCandidateId(slot, hash)` → next allowed
    /// request time. Used for block repair — avoids duplicate `requestCandidate`s
    /// and implements delayed-request logic (wait for the broadcast to arrive
    /// before querying peers). Populated when a `BlockFinalized` event arrives for
    /// a still-missing candidate; a delayed action then re-checks `candidate_book`
    /// and calls `receiver.request_candidate()` only if it is still missing.
    requested_candidates: HashMap<RawCandidateId, SystemTime>,

    /// Per-session telemetry aspect (see [`crate::session_telemetry`]).
    ///
    /// Held as an `Arc` so the same instance can be shared with the
    /// validation/collation controllers. `SessionTelemetry` is fully
    /// interior-mutable (atomics + spin mutexes), so every access goes
    /// through `&self` via the `Arc` deref.
    telemetry: Arc<SessionTelemetry>,

    /// Per-session runtime context — owns both the immutable bootstrap
    /// handles (description, session-start parents, stop flag) and the
    /// mutable cross-controller scratch state (slot map, delayed-action
    /// queue, next-wake bookkeeping, receiver-activity mirror). See
    /// [`crate::session_runtime`].
    runtime: SessionRuntime,

    /// Per-session callback delivery aspect (see
    /// [`crate::session_callbacks`]). Held as `Arc` so `SessionImpl`
    /// can keep a sibling reference, spawn/join the `SXCB` worker
    /// thread in the correct order, and dispatch listener
    /// notifications without going through the `SessionProcessor`
    /// borrow.
    ///
    /// Owns the `SXCB` worker transport (callback task queue +
    /// `use_callback_thread` flag), the shutdown-suppression gate, and
    /// the four `notify_*` wrappers (`notify_candidate`,
    /// `notify_candidate_observed`, `notify_generate_slot`,
    /// `notify_block_finalized`).
    callbacks: Arc<SessionCallbacks>,

    /// Per-session in-memory candidate store (see
    /// [`crate::candidate_book`]). Owns `received_candidates`, the
    /// `candidate_data_cache` TL-bytes lookup, and the
    /// `seen_broadcast_candidates` slot-dedup map.
    candidate_book: CandidateBook,

    /// Per-session persistence-ordering controller (see
    /// [`crate::database_controller`]). Owns the `db: SimplexDbPtr` handle, the
    /// `first_nonannounced_window` pool-state cursor, the four `*_store_results`
    /// async-DB-write dedup maps, and the `pending_async_db_results` registry. The
    /// persistence *helpers* that drive that registry (`post_async_db_result`,
    /// `process_pending_async_db_results`, `ensure_candidate_info_stored`, …) stay
    /// on `SessionProcessor` because their continuations take `&mut SessionProcessor`.
    database: DatabaseController,

    /// Per-session collation-phase controller (see
    /// [`crate::collation_controller`]). Owns the precollation pipeline maps,
    /// `earliest_collation_time`, `local_chain_head`, the
    /// `generated_parent_cache` / `generated_parent_gen_utime_ms_cache`
    /// lookups, and `last_generated_slot`, plus the full collation pipeline
    /// (`check_collation`, `precollate_block`, `on_collation_complete` /
    /// `on_collation_failed`, `generated_block`, …). Driven on the session
    /// main loop via `with_collation_backend` and the `self.collation.*`
    /// accessors.
    collation: CollationController,

    /// Per-session candidate-validation controller (see
    /// [`crate::validation_controller`]). Owns the `pending_validations` /
    /// `pending_approve` / `rejected` / `approved` queues, the
    /// `validation_attempt_map`, and the `validated_candidates` FSM submission
    /// queue, driven via `with_validation_backend` and the `self.validation.*`
    /// accessors.
    validation: ValidationController,

    /// Trace collector for recording consensus lifecycle events. `None` when
    /// stats collection is disabled. Held by `SessionProcessor` only for the
    /// session-identity event (`record_id` at construction) and the
    /// session-end event (`record_session_end` in `Drop`); the per-event hot
    /// path lives on the controllers, which each hold their own clone
    trace_collector: Option<TraceCollector>,
}

// ======================================================================
// Construction & teardown
// ======================================================================
//
// `new` builds an empty processor; bootstrap state is restored separately
// by `SessionStartupRecoveryProcessor::apply_bootstrap()`. `Drop` only logs.

impl SessionProcessor {
    /// Create new session processor
    ///
    /// The processor is created with empty state. Bootstrap state is applied
    /// separately via `SessionStartupRecoveryProcessor::apply_bootstrap()`.
    ///
    /// # Parameters
    /// * `description` - Pre-built session description with all immutable config
    /// * `initial_errors` - Error count from startup phase (before processor was created)
    pub(crate) fn new(
        description: Arc<SessionDescription>,
        session_start_prev_blocks: Vec<BlockIdExt>,
        task_queue: TaskQueuePtr,
        receiver: ReceiverPtr,
        stop_flag: Arc<AtomicBool>,
        db: SimplexDbPtr,
        initial_errors: u32,
        receiver_health_counters: Arc<crate::receiver::ReceiverHealthCounters>,
        callbacks: Arc<SessionCallbacks>,
        trace_collector: Option<TraceCollector>,
        catchain_seqno: u32,
    ) -> Result<Self> {
        // Extract immutable values from description before it's moved
        let session_id = description.get_session_id().clone();
        let initial_block_seqno = description.get_initial_block_seqno();
        assert!(
            !session_start_prev_blocks.is_empty() && session_start_prev_blocks.len() <= 2,
            "INVARIANT VIOLATION: SessionProcessor::new requires one or two session start prev blocks, got {}",
            session_start_prev_blocks.len()
        );
        assert_eq!(
            session_start_prev_blocks.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1,
            initial_block_seqno,
            "INVARIANT VIOLATION: session start prevs imply initial seqno {} != SessionDescription initial seqno {}",
            session_start_prev_blocks.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1,
            initial_block_seqno
        );

        // INVARIANT: initial_block_seqno must be > 0.
        // Block seqno 0 is reserved for the zerostate (genesis), so the first real block is seqno 1.
        // This invariant ensures finalized_head_seqno initialization (initial_block_seqno - 1) is valid.
        assert!(
            initial_block_seqno > 0,
            "INVARIANT VIOLATION: initial_block_seqno must be > 0, got {}",
            initial_block_seqno
        );

        // Initialize SimplexState FSM.
        // SIMPLEX_ROUNDLESS:
        // - We pass `SIMPLEX_ROUNDLESS` in callbacks to bypass round-based invariants.
        let simplex_state = SimplexState::new(&description)?;
        let initial_standstill_slots = simplex_state.get_tracked_slots_interval();
        let initial_progress_slot = simplex_state.get_first_non_progressed_slot().value();

        // Initialize receiver standstill tracked range to the FSM-tracked interval (C++ parity).
        // Receiver defaults to a broad range, but we can set the precise initial interval immediately
        // because `SimplexState::new()` creates window 0 (so end = slots_per_leader_window).
        receiver.set_ingress_slot_begin(initial_standstill_slots.0);
        receiver.set_ingress_progress_slot(initial_progress_slot);
        receiver.set_standstill_slots(initial_standstill_slots.0, initial_standstill_slots.1);

        log::info!(
            "Session {} SIMPLEX MODE: C++ parenting enabled (notarized parents accepted). \
            Candidate-native validation enabled. \
            WaitForParent gating=strict, MC stale protection=validator-side.",
            session_id.to_hex_string(),
        );

        log::info!(
            "Session {} SimplexState FSM initialized: slots_per_window={}",
            session_id.to_hex_string(),
            description.opts().slots_per_leader_window,
        );
        log::warn!(
            "Session {}: TEMP experimental MAX_AWAKE_TIMEOUT={}ms is enabled; restore the old \
            far-future fallback after timing validation is complete",
            session_id.to_hex_string(),
            MAX_AWAKE_TIMEOUT.as_millis(),
        );

        let now = description.get_time();
        let num_validators = description.get_total_nodes() as usize;

        // first_nonannounced_window starts at 0, set via recovery_set_first_nonannounced_window()
        let first_nonannounced_window = WindowIndex::default();

        // Emit session identity event (once at session start)
        if let Some(tc) = &trace_collector {
            tc.record_id(&session_id, &description, catchain_seqno);
        }

        // Build the telemetry aspect: all metric handles, stall cursors,
        // health-alert dedup state, error counters, and stall-debug bookkeeping.
        // Seeds `errors_counter` with `initial_errors` for metric consistency.
        let telemetry = Arc::new(SessionTelemetry::new(
            description.get_metrics_receiver().clone(),
            &description,
            receiver_health_counters,
            initial_errors,
            description.opts().health_alert_cooldown,
            now,
        ));

        // Cheap-to-clone handle (Arc) handed to the validation controller
        // so it can dispatch candidate-validation callbacks directly via
        // `SessionCallbacks` instead of routing through `SessionProcessor`.
        // Cloned before the struct literal because the original is moved
        // into the `callbacks` field below. The session-listener handle
        // lives inside `SessionCallbacks`, so the controller needs no
        // separate listener.
        let validation_callbacks = callbacks.clone();
        // Same `Arc` the runtime holds; cloned before `description` is moved
        // into `SessionRuntime` below so the validation controller can read
        // session id / options / timing without per-call threading.
        let validation_description = description.clone();
        // Shared session description handle for the collation controller, cloned
        // before `description` is moved into `SessionRuntime` below. Lets the
        // controller read session id / options / shard / leader without taking a
        // `&SessionDescription` argument per call.
        let collation_description = description.clone();
        // Cheap-to-clone callbacks handle (Arc) handed to the collation
        // controller so it can dispatch the generate-slot notification directly
        // via `SessionCallbacks`. Cloned before the original is moved into the
        // `callbacks` field below.
        let collation_callbacks = callbacks.clone();
        // Shared per-session telemetry `Arc` (interior-mutable), cloned for the
        // validation controller before the original is moved into the
        // `telemetry` field.
        let validation_telemetry = telemetry.clone();
        // Same `Arc`, cloned for the collation controller's self-collation
        // observability before the original is moved into the `telemetry` field.
        let collation_telemetry = telemetry.clone();
        // Shared callbacks / description / telemetry handles for the consensus
        // controller, cloned before the originals are moved into `callbacks` /
        // `SessionRuntime` / `telemetry` below. The controller dispatches
        // finalized + candidate-observed callbacks, reads session id / shard /
        // weights / timing, and records finalization milestones directly.
        let consensus_callbacks = callbacks.clone();
        let consensus_description = description.clone();
        let consensus_telemetry = telemetry.clone();
        // Trace collector handle (mpsc sender + Arc) cloned for each controller
        let collation_trace_collector = trace_collector.clone();
        let validation_trace_collector = trace_collector.clone();
        let consensus_trace_collector = trace_collector.clone();
        // Generic deferred-work handle for the validation controller. The
        // adapter projects `&mut SessionProcessor -> &mut p.validation`, so the
        // controller can post follow-up / async work without naming this type.
        // Cloned from `task_queue` before it is moved into the struct below.
        let validation_queue: ControllerQueuePtr<ValidationController> =
            Arc::new(ValidationQueueAdapter { task_queue: task_queue.clone() });
        // Generic deferred-work handle for the collation controller. The
        // adapter projects `&mut SessionProcessor -> &mut p.collation`, so the
        // controller can post follow-up / async collation work without naming
        // this type. Cloned from `task_queue` before it is moved into the
        // struct below.
        let collation_queue: ControllerQueuePtr<CollationController> =
            Arc::new(CollationQueueAdapter { task_queue: task_queue.clone() });

        let processor = Self {
            task_queue,
            receiver,
            simplex_state,
            // Consensus finalization aspect. Seeds the finalized-head + producer
            // finalization cursors from `initial_block_seqno - 1`: the block
            // *before* session start is treated as the finalized head, which is
            // required for empty-block generation gating (non-finalized parent /
            // ValidatorGroup limitation) and validation gating
            // (expected_seqno = finalized_head_seqno + 1), matching C++ where the
            // block producer tracks the parent seqno from `Start` / `base`.
            consensus: ConsensusController::new(
                consensus_callbacks,
                consensus_description,
                consensus_telemetry,
                initial_block_seqno.checked_sub(1),
                initial_block_seqno.checked_sub(1),
                // Empty-block support: MC applied-top floor + accepted-normal-head
                // seqno, seeded from the block before session start.
                initial_block_seqno.checked_sub(1),
                initial_block_seqno.saturating_sub(1),
                consensus_trace_collector,
            ),
            // Candidate request tracking
            requested_candidates: HashMap::new(),
            telemetry,
            runtime: SessionRuntime::new(
                now,
                num_validators,
                description,
                session_start_prev_blocks,
                stop_flag,
            ),
            callbacks,
            candidate_book: CandidateBook::new(),
            database: DatabaseController::new(db, first_nonannounced_window),
            collation: CollationController::new(
                collation_queue,
                collation_callbacks,
                collation_description,
                collation_telemetry,
                collation_trace_collector,
            ),
            validation: ValidationController::new(
                validation_queue,
                validation_callbacks,
                validation_description,
                validation_telemetry,
                validation_trace_collector,
            ),
            trace_collector,
        };

        if initial_errors > 0 {
            log::debug!(
                "Session {} initialized with {} startup errors",
                processor.session_id().to_hex_string(),
                initial_errors
            );
        }

        Ok(processor)

        // Note: C++ simplex resolves candidates from its own consensus DB, not via
        // validator manager. The Rust implementation uses in-memory candidate_data_cache
        // and peer overlay for candidate resolution. No get_approved_candidate delegation.
    }
}

impl Drop for SessionProcessor {
    fn drop(&mut self) {
        let session_id = self.session_id();
        log::info!("Dropping SessionProcessor for session {}", session_id.to_hex_string());
        if let Some(tc) = &self.trace_collector {
            tc.record_session_end(session_id);
        }
    }
}

// ======================================================================
// Accessors & counters
// ======================================================================
//
// Session identity / description / metrics handles, the source-validity
// check, and the error / generated-candidate telemetry counters.

impl SessionProcessor {
    /* Session identity & handles */

    /// Get session description
    pub(crate) fn get_description(&self) -> &SessionDescription {
        self.runtime.description()
    }

    /// Get metrics receiver.
    ///
    /// Delegates to the telemetry aspect — see [`crate::session_telemetry`].
    /// Used by `SessionImpl::create_metrics_dumper` and integration tests.
    pub(crate) fn get_metrics_receiver(&self) -> &MetricsHandle {
        &self.telemetry.metrics_receiver
    }

    /// Get session identifier (convenience accessor)
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.runtime.description().get_session_id()
    }

    /* Source check & error counter */

    /// Check if validator index is valid (within bounds)
    #[inline]
    fn is_valid_source(&self, source_idx: ValidatorIndex) -> bool {
        source_idx.is_valid(self.runtime.description().get_total_nodes())
    }

    /// Increment the session error counter. Delegates to the telemetry aspect.
    ///
    /// Called when an error occurs during session processing. Uses atomic
    /// increment so it can be called with `&self`. State and the metric
    /// counter both live in [`crate::session_telemetry::SessionTelemetry`].
    fn increment_error(&self) {
        self.telemetry.increment_error();
    }
}

// ======================================================================
// Slot & candidate cleanup
// ======================================================================
//
// History pruning after finalize / skip: drop receiver votes, dedup and
// resolver state for slots / candidates consensus no longer needs.

impl SessionProcessor {
    /// Clean up old slot data (receiver cache + validated candidates)
    ///
    /// Removes:
    /// - Receiver: votes, dedup entries, resolver cache for slots < up_to_slot
    /// - SessionProcessor: validated candidates, received candidates
    ///
    /// Reference: validator-session/src/session_processor.rs new_round()
    ///
    /// # Arguments
    /// * `finalized_slot` - The slot that was just finalized/skipped
    fn cleanup_old_slots(&mut self, finalized_slot: SlotIndex) {
        // Calculate up_to_slot for cleanup (finalized_slot - MAX_HISTORY_SLOTS)
        let up_to_slot = if finalized_slot.value() >= MAX_HISTORY_SLOTS {
            SlotIndex::new(finalized_slot.value() - MAX_HISTORY_SLOTS + 1)
        } else {
            SlotIndex::new(0) // Don't clean up if we haven't reached MAX_HISTORY_SLOTS yet
        };

        if up_to_slot.value() == 0 {
            log::trace!(
                "Session {} cleanup_old_slots: finalized_slot={finalized_slot}, skipping cleanup \
                (not enough history yet)",
                self.session_id().to_hex_string(),
            );
            return;
        }

        log::trace!(
            "Session {} cleanup_old_slots: finalized_slot={}, cleaning up slots < {}",
            self.session_id().to_hex_string(),
            finalized_slot,
            up_to_slot
        );

        // Clean up SimplexState FSM (old windows and vote accounting)
        self.simplex_state.cleanup_slots(up_to_slot);

        // Notify receiver to cleanup old data (votes, dedup, resolver cache)
        self.receiver.cleanup(up_to_slot.value());

        // Clean up session processor's validated candidates
        self.cleanup_old_candidates(up_to_slot);
    }

    /// Clean up candidates for slots that are now old
    ///
    /// Removes both validated candidates and received candidates for old slots.
    /// Also cleans up validation state collections (keyed by RawCandidateId which contains slot).
    /// Reference: validator-session/src/session_processor.rs blocks.retain
    fn cleanup_old_candidates(&mut self, up_to_slot: SlotIndex) {
        let first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        let first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();

        let stale_generated_candidates = self.telemetry.stale_generated_candidate_ids(up_to_slot);
        for candidate_id in stale_generated_candidates {
            self.note_generated_candidate_validation_missed(
                &candidate_id,
                format!("cleanup_old_candidates up_to_slot={up_to_slot}"),
            );
        }

        // Clean up validation state collections (session-level, keyed by RawCandidateId).
        // Single fan-out into the controller; the validation maps (incl. the
        // `validated_candidates` VecDeque) are pruned together so the seven
        // previously inlined `retain` predicates stay in lock-step.
        self.validation.prune_below(up_to_slot);
        self.telemetry.retain_self_collation_starts(|slot| slot >= up_to_slot);
        self.telemetry.retain_self_collation_pending(|id| id.slot >= up_to_slot);
        self.database.prune_below(up_to_slot);

        // Prune CandidateBook auxiliary maps in sync. received_candidates
        // GC is intentionally deferred; see CandidateBook::prune_below /
        // CandidateBook::retain_received.
        self.candidate_book.prune_below(up_to_slot);

        // Prune the consensus finalization journal for old slots. Single fan-out
        // into the controller: it prunes the transient `finalized_pending_body`
        // buffer + refreshes the gauge and the per-id `finalized_delivery_sent`
        // dedup together. The seqno-level dedup is intentionally NOT pruned while
        // received_candidates are still retained for old slots (a later recursive
        // parent-chain walk could otherwise re-emit an already-delivered finalized
        // callback) — see `ConsensusController::prune_below`.
        self.consensus.prune_below(up_to_slot);

        // Prune log-throttle set to prevent unbounded growth over long sessions
        self.telemetry.prune_missing_body_log_below(up_to_slot.value());

        // Remove pending candidate requests for slots < up_to_slot
        self.requested_candidates.retain(|id, _| id.slot >= up_to_slot);

        self.runtime.clear_runtimes_below(up_to_slot);

        log::trace!(
            "Session {} cleanup_old_candidates: cleaned up slots < {up_to_slot}, \
            first_non_progressed={first_non_progressed_slot}, \
            first_non_finalized={first_non_finalized_slot}",
            self.session_id().to_hex_string(),
        );
    }

    /// Reset per-slot state after finalization or skip
    ///
    /// Called when a slot is finalized or skipped to clean up state.
    /// Reference: validator-session/src/session_processor.rs new_round()
    ///
    /// # Arguments
    /// * `slot` - The slot that was just finalized/skipped
    fn reset_slot_state(&mut self, slot: SlotIndex) {
        check_execution_time!(10_000);

        // Validate the slot is actually progressed (finalized OR notarized OR skipped).
        //
        // Note: We check the individual slot state rather than the progress cursor position,
        // because under network failures slots can be skipped out of order.
        let is_progressed = self.simplex_state.is_slot_progressed(self.runtime.description(), slot);
        debug_assert!(
            is_progressed,
            "SessionProcessor: reset_slot_state called for non-progressed slot {} (first_non_progressed={})",
            slot,
            self.simplex_state.get_first_non_progressed_slot()
        );

        log::trace!(
            "Session {} reset_slot_state: slot={slot}, is_progressed={is_progressed}, \
            fsm_first_non_progressed_slot={}, fsm_first_non_finalized_slot={}",
            self.session_id().to_hex_string(),
            self.simplex_state.get_first_non_progressed_slot(),
            self.simplex_state.get_first_non_finalized_slot(),
        );

        // Cleanup old slot data (receiver cache + validated candidates)
        self.cleanup_old_slots(slot);

        // Self-collation safety net: if the flow never reached success or final
        // failure (e.g. crash, race, missing callback), drop the tracking entry
        // so it auto-counts as ignore (auto-derived as `total - success - failure`).
        let now = self.now();
        self.telemetry.forget_self_collation_tracking(
            slot,
            "reset_slot_state",
            self.runtime.description(),
            now,
        );

        //TODO: LK: check if this is really needed here
        self.remove_precollated_block(slot);
    }
}

// ======================================================================
// Lifecycle, time control & scheduling
// ======================================================================
//
// Session bring-up / teardown and the SXMAIN main loop, the wake-horizon +
// delayed-action scheduler, and the session clock.

impl SessionProcessor {
    /* Lifecycle (bring-up / teardown / main loop) */

    /// Arm FSM timeouts and prepare the processor for the main loop.
    ///
    /// Must be called exactly once, after overlay warmup and bootstrap
    /// recovery, right before the main loop begins.  The FSM is created
    /// with unarmed timeouts (`skip_timestamp = None`) so that no skip
    /// cascade fires during the startup delay.
    ///
    /// C++ reference: `start_up()` initialises state and processes
    /// bootstrap votes; timeouts are armed through the event flow
    /// (`LeaderWindowObserved` → `alarm_timestamp()`).  In Rust the
    /// equivalent arming point is this explicit `start()` call.
    pub(crate) fn start(&mut self) {
        self.simplex_state.finish_startup_replay(self.runtime.description());
        self.simplex_state.reset_timeouts_on_start(self.runtime.description());

        log::info!(
            "Session {} started: skip timeouts armed",
            &self.session_id().to_hex_string()[..8],
        );
    }

    /// Stop the session processor.
    ///
    /// C++ parity (`validator/consensus/bridge.cpp::destroy_inner()`):
    ///
    /// ```cpp
    /// bus_->publish<consensus::StopRequested>();
    /// co_await bus_->db->close();   // drains queued co_await db->set(...)
    /// if (destroy) bus_->db->destroy();
    /// bus_ = nullptr;
    /// ```
    ///
    /// We mirror it in three phases:
    ///
    /// 1. `drain_pending_async_db_results_for_shutdown()` polls the SXMAIN
    ///    `pending_async_db_results` registry until empty (or
    ///    `SHUTDOWN_DRAIN_TIMEOUT` elapses), so any continuation registered via
    ///    `post_async_db_result()` runs on SXMAIN with an Ok / Err result
    ///    instead of being silently abandoned. This is the "process mailbox"
    ///    equivalent before the bus is torn down.
    /// 2. `db.close(SHUTDOWN_DB_SYNC_TIMEOUT)` drains the storage task + callback
    ///    queues AND flips the `is_closed` gate so any still-running higher
    ///    layer that races the shutdown (e.g. a precollation worker) gets
    ///    `DB_CLOSED_ERROR` instead of silently enqueuing onto a thread that
    ///    is about to exit.
    /// 3. When `destroy_db == true`, `db.mark_for_destroy()` instructs the
    ///    storage's DB thread to `remove_dir_all(path)` once the final
    ///    `Arc<SimplexDb>` reference is dropped. We do *not* drop the Arc
    ///    here — ownership may be shared with a still-living helper (e.g.
    ///    `ensure_candidate_info_stored` callbacks registered just before
    ///    stop), so we let the regular drop chain (`SessionProcessor` ->
    ///    `SimplexDb::Drop` -> `AsyncKeyValueStorage::Drop`) perform the
    ///    actual teardown. C++ behaves identically: `bus_ = nullptr` after
    ///    `destroy_inner()` returns.
    ///
    /// `SimplexDb::Drop` keeps a safety-net `sync()` for the case where a
    /// panicking caller reaches `Drop` without `stop()`, but under normal
    /// shutdown the `close()` above has already drained the queue and the
    /// safety-net short-circuits with `DB_CLOSED_ERROR` (logged at `debug`).
    pub(crate) fn stop(&mut self, destroy_db: bool) {
        log::info!(
            "Stopping SessionProcessor for session {} (destroy_db={})",
            self.session_id().to_hex_string(),
            destroy_db
        );

        // Phase 1: drain SXMAIN pending DB-result continuations.
        let drain_elapsed = self.drain_pending_async_db_results_for_shutdown();

        // Phase 2: close the storage — drains the queue AND flips the
        // is_closed gate so subsequent ops from late continuations fail fast.
        // Wall-clock used deliberately for the timing — see `wall_now()` doc;
        // shutdown timing must advance even under a frozen manual session
        // clock.
        let close_started_at = self.wall_now();
        match self.database.db().close(Some(SHUTDOWN_DB_SYNC_TIMEOUT)) {
            Ok(()) => {
                log::info!(
                    "Session {} stop: db.close completed in {}ms (after {}ms drain)",
                    &self.session_id().to_hex_string()[..8],
                    self.wall_now()
                        .duration_since(close_started_at)
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                    drain_elapsed.as_millis(),
                );
            }
            Err(e) => {
                log::error!(
                    "Session {} stop: db.close(timeout={}s) failed after {}ms (drain elapsed \
                     {}ms): {}",
                    &self.session_id().to_hex_string()[..8],
                    SHUTDOWN_DB_SYNC_TIMEOUT.as_secs(),
                    self.wall_now()
                        .duration_since(close_started_at)
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                    drain_elapsed.as_millis(),
                    e,
                );
                self.increment_error();
            }
        }

        // Phase 3: optional destroy — schedules on-disk removal for when the
        // last Arc<SimplexDb> drops. Safe to call even if close() failed: the
        // gate is already set, so no new writes can race the removal.
        if destroy_db {
            log::info!(
                "Session {} stop: destroy_db=true, marking storage for destruction",
                &self.session_id().to_hex_string()[..8]
            );
            self.database.db().mark_for_destroy();
        }

        // Stop receiver
        self.receiver.stop();

        // Cancel pending precollations
        self.reset_precollations();
    }

    /// Check all pending operations
    ///
    /// Called periodically from main loop when awake time is reached.
    /// Implements the core consensus event loop.
    ///
    /// Reference: validator-session/src/session_processor.rs check_all
    pub(crate) fn check_all(&mut self) {
        check_execution_time!(10_000);

        // Increment metrics counter
        self.telemetry.check_all_counter.increment(1);

        let now = self.now();
        let wake_slip = now.duration_since(self.runtime.get_next_awake_time()).unwrap_or_default();
        self.telemetry.check_all_wake_slip_histogram.record(wake_slip.as_millis() as f64);

        // Reset awake time to far future, will be updated by various checks
        self.reset_next_awake_time();

        // Stalled consensus detection
        // Debug dump if no finalizations for ROUND_DEBUG_PERIOD (stalled consensus)
        if now >= self.telemetry.round_debug_at() {
            self.debug_dump(true); // is_stalled=true: full dump to INFO level
            self.telemetry.set_round_debug_at(now + ROUND_DEBUG_PERIOD);
        }

        // Release delayed gates before evaluating validation readiness so retries
        // can re-enter validation in the same `check_all()` pass.
        self.process_delayed_actions();

        // Drain any async DB results that completed since the last pass.
        // Continuations registered via `post_async_db_result()` run here.
        self.process_pending_async_db_results();

        // Check validation (process pending validations)
        self.check_validation();

        // Feed validated candidates to FSM BEFORE timeout processing so that
        // the FSM has all available candidates before it evaluates timeouts
        // (mirrors C++ where process_blocks() feeds candidates before the
        // round timer is checked).
        self.process_validated_candidates();

        // Call SimplexState FSM check_all (processes timeouts, pending blocks)
        self.simplex_state.check_all(self.runtime.description());

        // Process all events produced by FSM
        self.process_simplex_events();

        // Keep receiver standstill slots aligned even when the FSM tracked
        // interval changes outside the finalization / skip hooks.
        self.sync_standstill_slots_from_state();

        // Update awake time from FSM timeout
        if let Some(fsm_timeout) = self.simplex_state.get_next_timeout() {
            self.runtime.set_next_awake_time(fsm_timeout);
        }

        // Ensure we wake up for stall detection even when FSM has no pending timeouts
        self.runtime.set_next_awake_time(self.telemetry.round_debug_at());

        // Persist pool state (first_nonannounced_window) when window advances
        self.maybe_store_pool_state();

        // Drive the collation pipeline (leader/pacing/parent decisions live in CollationController)
        self.check_collation();

        let cur_nf = self.simplex_state.get_first_non_finalized_slot();
        let cur_np = self.simplex_state.get_first_non_progressed_slot();
        self.telemetry.note_frontier_progress(now, cur_nf, cur_np);
        self.telemetry.first_non_finalized_slot_gauge.set(cur_nf.0 as f64);
        self.telemetry.first_non_progressed_slot_gauge.set(cur_np.0 as f64);

        // Debug state dump
        self.log_consensus_state("check_all");
    }

    /* Wake horizon & delayed-action scheduler */

    /// Current wake horizon (delegates to `SessionRuntime`).
    pub(crate) fn get_next_awake_time(&self) -> SystemTime {
        self.runtime.get_next_awake_time()
    }

    /// Reset the wake horizon to the fallback poll horizon
    /// (`now + MAX_AWAKE_TIMEOUT`). Called at the beginning of `check_all()`
    /// before collecting timeouts from all sources.
    pub(crate) fn reset_next_awake_time(&mut self) {
        let now = self.now();
        self.runtime.reset_next_awake_time(now);
    }

    /// Post a delayed action to be executed at a future time.
    ///
    /// The handler runs when `expiration_time` is reached during
    /// `check_all()`. Storage and drain ordering live on `SessionRuntime`;
    /// this wrapper boxes the handler, pushes it onto the queue, and lowers
    /// the wake horizon to `expiration_time`.
    fn post_delayed_action<F>(&mut self, expiration_time: SystemTime, handler: F)
    where
        F: FnOnce(&mut SessionProcessor) + Send + 'static,
    {
        self.runtime.post_delayed_action(expiration_time, Box::new(handler));
        self.runtime.set_next_awake_time(expiration_time);
    }

    /// Process all expired delayed actions.
    ///
    /// Drains due actions via the runtime and invokes each handler against
    /// `&mut self`. Re-entrant: handlers that schedule new actions land at
    /// the queue tail and are observed by subsequent drain iterations in
    /// the same loop. After the drain finishes, the next pending
    /// expiration (if any) is registered so the main loop wakes on time.
    fn process_delayed_actions(&mut self) {
        let now = self.now();
        while let Some(action) = self.runtime.drain_due_delayed_action(now) {
            (action.handler)(self);
        }
        if let Some(next) = self.runtime.min_pending_delayed_expiration() {
            self.runtime.set_next_awake_time(next);
        }
    }

    /* Session clock */

    /// Current session time (real-time or manually overridden for tests/log replay).
    ///
    /// IMPORTANT: SessionProcessor must not call `SystemTime::now()` directly.
    /// All time access goes through `SessionDescription::get_time()` so tests can
    /// deterministically control time. The single deliberate exception is
    /// [`Self::wall_now`] — see that method for the rationale and
    /// allowed call sites.
    #[inline]
    fn now(&self) -> SystemTime {
        self.runtime.description().get_time()
    }

    /// Wall-clock `SystemTime::now()` — the **deliberate** exception to the
    /// "no direct `SystemTime::now()`" rule documented on [`Self::now`].
    ///
    /// Use ONLY for:
    /// 1. Shutdown drain deadlines / sync timing in [`Self::stop`] and
    ///    [`Self::drain_pending_async_db_results_for_shutdown`]. These must
    ///    advance even when tests freeze the manual session clock; otherwise
    ///    a stuck writer combined with a frozen clock would park shutdown
    ///    forever and never trip the bounded deadline.
    /// 2. Future shutdown / teardown timing helpers.
    ///
    /// Do NOT use anywhere else. Anything FSM- or protocol-visible (timeouts,
    /// scheduling, validation deadlines, log timestamps that may drive replay)
    /// must keep going through `now()` so manual-clock tests stay
    /// deterministic.
    ///
    /// Centralising the call here also makes it greppable / lintable in the
    /// future if we want to enforce the rule mechanically.
    #[inline]
    fn wall_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

// ======================================================================
// Candidates (ingress, repair & serving)
// ======================================================================
//
// Inbound candidate reception (`on_candidate_received`), outbound block-repair
// request scheduling / resolver availability, and the SXRCV-side query fallback
// that answers peers' `RequestCandidate`.

impl SessionProcessor {
    /* Ingress (candidate reception) */

    /// Handle incoming block candidate (from broadcast or query response)
    ///
    /// Called by ReceiverListenerImpl when a block candidate is received,
    /// either via broadcast or from a requestCandidate query response.
    /// See [`ValidationController`] module docs for the full validation pipeline.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `candidate` - Deserialized candidate data
    /// * `notar_cert` - Serialized notarization certificate signature-set bytes (None for broadcasts)
    ///
    /// Reference: validator-session/src/session_processor.rs process_broadcast()
    /// Reference: C++ block-validator.cpp handle(ValidationRequest)
    pub(crate) fn on_candidate_received(
        &mut self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
    ) {
        check_execution_time!(20_000);

        // Extract slot and parent info from CandidateData variant.
        // TL uses i32 for slots; reject negative values at the boundary.
        let (slot, tl_parent_str) = match &candidate {
            CandidateData::Consensus_Block(block) => {
                if block.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Block",
                        self.session_id().to_hex_string(),
                        block.slot
                    );
                    return;
                }
                let parent_str = match block.parent.id() {
                    None => "genesis".to_string(),
                    Some(id) => {
                        if *id.slot() < 0 {
                            log::warn!(
                                "Session {} on_candidate_received: REJECTED - \
                                negative parent slot {} in Block",
                                self.session_id().to_hex_string(),
                                id.slot()
                            );
                            return;
                        }
                        format!("s{}:{}", id.slot(), &hex::encode(id.hash().as_slice())[..8])
                    }
                };
                (block.slot as u32, parent_str)
            }
            CandidateData::Consensus_Empty(empty) => {
                if empty.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.slot
                    );
                    return;
                }
                if *empty.parent.slot() < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - \
                        negative parent slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.parent.slot()
                    );
                    return;
                }
                let id_slot = *empty.parent.slot();
                let id_hash = empty.parent.hash();
                let parent_str = format!("s{}:{}", id_slot, &hex::encode(id_hash.as_slice())[..8]);
                (empty.slot as u32, parent_str)
            }
        };

        let sender_idx = ValidatorIndex::new(source_idx);
        let slot = SlotIndex::new(slot);
        let is_broadcast_candidate = notar_cert.is_none();
        let is_local_self_candidate =
            is_broadcast_candidate && sender_idx == self.runtime.description().get_self_idx();
        self.record_candidate_ingress(sender_idx, is_broadcast_candidate);

        // Reject far-future slots (DoS protection) — before any signature verification
        if self.simplex_state.is_slot_too_far_ahead(slot) {
            if is_broadcast_candidate {
                self.telemetry.candidate_precheck_future_slot_drop_counter.increment(1);
            }
            if is_local_self_candidate {
                self.note_generated_candidate_validation_missed_for_slot(
                    slot,
                    format!(
                        "candidate_precheck_too_far_ahead max_acceptable_slot={}",
                        self.simplex_state.max_acceptable_slot()
                    ),
                );
            }
            log::warn!(
                "Session {} on_candidate_received: REJECTED precheck_drop_reason=too_far_ahead \
                slot={} max={} origin={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                self.simplex_state.max_acceptable_slot(),
                if is_broadcast_candidate { "broadcast" } else { "query" },
            );
            return;
        }

        // Candidate signatures are always created by the slot leader (not by the relay / query responder).
        // For requestCandidate responses, `sender_idx` is the responder, which can differ from the leader.
        let leader_idx = self.runtime.description().get_leader(slot);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "Session {} on_candidate_received: \
                sender_idx={sender_idx}, leader_idx={leader_idx}, \
                slot={slot}, tl_parent={tl_parent_str}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 1. Check sender_idx is valid
        if !self.is_valid_source(sender_idx) {
            log::warn!(
                "Session {} on_candidate_received: unknown sender_idx={} (max={})",
                self.session_id().to_hex_string(),
                sender_idx,
                self.runtime.description().get_total_nodes()
            );
            return;
        }

        // NOTE: A broadcast candidate (no attached notar cert) is authenticated
        // by the slot leader's signature, which is verified against `leader_key` in
        // `RawCandidate::from_tl(...)` below. The delivering peer (`sender_idx`) may be a
        // relay / gossip hop rather than the leader itself. Dropping a relayed broadcast
        // here strands any node that missed the leader's direct delivery: it can never
        // notarize the slot, is forced to skip, and a single such node is enough to wedge
        // finalization on a notarized slot.
        //
        // C++ parity: overlay broadcasts carry the leader as their signed source (preserved
        // across relays), and `consensus.cpp handle(CandidateReceived)` applies no
        // sender==leader gate; the resolver also serves candidate bodies with no
        // leader-source restriction. We therefore accept relayed broadcasts and let the
        // leader-signature verification below authenticate the body.
        if is_broadcast_candidate && sender_idx != leader_idx {
            self.telemetry.candidate_relayed_broadcast_counter.increment(1);
            log::debug!(
                "Session {} on_candidate_received: received relayed broadcast candidate \
                slot={} leader={} sender={} (leader-signature verification pending below)",
                &self.session_id().to_hex_string()[..8],
                slot,
                leader_idx,
                sender_idx
            );
        }

        // Broadcast path must reject stale slots eagerly to avoid stale-body/db churn.
        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        if slot < fsm_first_non_finalized_slot {
            if is_broadcast_candidate {
                self.telemetry.candidate_precheck_old_slot_drop_counter.increment(1);
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!(
                            "candidate_precheck_old_slot first_non_finalized_slot={fsm_first_non_finalized_slot}"
                        ),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: REJECTED precheck_drop_reason=old_slot \
                    slot={} first_non_finalized={} origin=broadcast",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    fsm_first_non_finalized_slot
                );
                return;
            }

            log::trace!(
                "Session {} on_candidate_received: old slot received {} (current={}) origin=query",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_finalized_slot,
            );
        }

        // Get leader public key for signature verification
        let leader_key = self.runtime.description().get_source_public_key(leader_idx).clone();

        // 2. Create RawCandidate directly from TL (no serialization needed)
        // Note: max_size check is done in receiver
        let max_size = self.runtime.description().opts().max_block_size
            + self.runtime.description().opts().max_collated_data_size;

        let raw_candidate = match RawCandidate::from_tl(
            &candidate,
            &self.session_id(),
            &leader_key,
            leader_idx,
            self.runtime.description().get_shard(),
            max_size,
            self.runtime.description().opts().proto_version,
        ) {
            Ok(c) => c,
            Err(e) => {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!("candidate_deserialization_failed error={e}"),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: failed to deserialize candidate from \
                    sender={}, leader={}, slot={}: {}",
                    self.session_id().to_hex_string(),
                    sender_idx,
                    leader_idx,
                    slot,
                    e
                );
                return;
            }
        };

        // Trace log incoming candidate details for debugging
        log::trace!(
            "Session {} on_candidate_received: parsed candidate {}:{} parent={} leader=v{:03}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &raw_candidate.id.hash.to_hex_string()[..8],
            raw_candidate
                .parent_id
                .as_ref()
                .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
                .unwrap_or_else(|| "genesis".to_string()),
            leader_idx
        );

        // 5. Candidates can be received via relay or requestCandidate (query response),
        // so the sender can differ from the slot leader. The signature is verified against
        // the leader's key above, so a mismatch here is not an error.
        if sender_idx != leader_idx && !is_broadcast_candidate {
            log::trace!(
                "Session {} on_candidate_received: received leader candidate via relay/query: \
                slot={slot} leader={leader_idx} sender={sender_idx}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 6. Check if we already have this candidate
        let candidate_id = raw_candidate.id.clone();
        let id_hash = candidate_id.hash.clone();
        debug_assert!(
            candidate_id.slot == slot,
            "RawCandidateId slot mismatch: tl_slot={} raw_candidate.id.slot={}",
            slot,
            candidate_id.slot
        );

        if is_broadcast_candidate {
            match self.candidate_book.seen_broadcast(slot).cloned() {
                Some(existing) if existing != candidate_id => {
                    self.telemetry.candidate_precheck_conflicting_slot_drop_counter.increment(1);
                    if is_local_self_candidate {
                        self.note_generated_candidate_validation_missed(
                            &candidate_id,
                            format!(
                                "candidate_precheck_conflicting_slot first_seen_slot_hash={}:{}",
                                existing.slot,
                                &existing.hash.to_hex_string()[..8]
                            ),
                        );
                    }
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED \
                        precheck_drop_reason=conflicting_slot_candidate \
                        slot={} first_seen={:?} new_candidate={:?} origin=broadcast",
                        &self.session_id().to_hex_string()[..8],
                        slot,
                        existing,
                        candidate_id
                    );
                    return;
                }
                Some(_) => {}
                None => {
                    self.candidate_book.insert_seen_broadcast(slot, candidate_id.clone());
                }
            }
        }

        // Check if candidate already known.
        // A finalized-boundary stub (seeded by handle_block_finalized with empty data) is NOT
        // "already known" for this purpose -- we want the real body to overwrite it.
        let is_finalized_stub = self
            .candidate_book
            .received(&candidate_id)
            .map(|r| r.candidate_hash_data_bytes.is_empty())
            .unwrap_or(false);
        if !is_finalized_stub
            && (self.validation.pending_validation_contains(&candidate_id)
                || self.validation.pending_approve_contains(&candidate_id)
                || self.validation.approved_contains(&candidate_id)
                || self.validation.is_rejected(&candidate_id)
                || self.candidate_book.contains_received(&candidate_id))
        {
            log::trace!(
                "Session {} on_candidate_received: candidate already known: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );

            // CandidateResolver parity: query responses can carry NotarCert bytes even when we
            // already have the candidate body (e.g., we missed the certificate broadcast).
            // Do NOT drop notar_cert in this case, otherwise the node can get permanently stuck
            // waiting for NotarCert while repeatedly receiving bodies.
            if let Some(ref cert_bytes) = notar_cert {
                self.process_received_notar_cert(slot, &id_hash, cert_bytes);
            }
            if notar_cert.is_some() {
                self.check_all();
            }
            return;
        }

        // 7. Store candidate in received_candidates for finalization (even if not validated)
        // This allows us to accept blocks that are finalized before validation completes
        // Reference: validator-session/src/session_processor.rs set_block_candidate
        let receive_time = self.now();
        let block_id = raw_candidate.block.block_id();

        // Trace: candidate received from network (also fires for self-loop after own block)
        if let Some(tc) = &self.trace_collector {
            let trace_id = BlockCandidateId {
                slot: raw_candidate.id.slot,
                hash: raw_candidate.id.hash.clone(),
                block: block_id.clone(),
            };
            let trace_parent = raw_candidate.parent_id.as_ref().map(|p| BlockCandidateId {
                slot: p.slot,
                hash: p.hash.clone(),
                block: ton_block::BlockIdExt::default(),
            });
            tc.record_candidate_received(
                self.session_id(),
                &trace_id,
                trace_parent.as_ref(),
                Some(block_id),
                false,
            );
        }

        let root_hash = block_id.root_hash.clone();
        let file_hash = block_id.file_hash.clone();

        // Determine if this is an empty block from the TL variant
        let is_empty = matches!(candidate, CandidateData::Consensus_Empty(_));

        // Cache serialized CandidateData for RequestCandidate query fallback (C++ parity).
        // This provides a secondary in-memory store that persists independently of
        // the receiver's resolver_cache, enabling peers to retrieve candidates even
        // after the resolver_cache is cleaned up.
        match serialize_boxed(&candidate) {
            Ok(bytes) => {
                self.candidate_book.insert_cached_data(candidate_id.clone(), bytes.clone());
                // Persist to DB for restart serving (C++ CandidateResolver::store_candidate parity)
                if let Err(e) =
                    self.database.db().save_candidate_payload_async(&candidate_id, &bytes)
                {
                    log::error!(
                        "Session {} on_candidate_received: failed to persist candidate payload: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.increment_error();
                }
            }
            Err(e) => {
                log::warn!(
                    "Session {} on_candidate_received: failed to serialize CandidateData for cache: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
            }
        }

        // Seqno validation for on_candidate_received
        // Validate seqno is consistent with parent (if parent is already received)
        let received_seqno = block_id.seq_no;
        if let Some(ref parent) = raw_candidate.parent_id {
            if let Some(parent_received) = self.candidate_book.received(parent) {
                let parent_seqno = parent_received.block_id.seq_no;
                let expected_seqno = if is_empty { parent_seqno } else { parent_seqno + 1 };

                if received_seqno != expected_seqno {
                    // NOTE: We no longer reject candidates for seqno mismatch at receive time.
                    // The seqno in a candidate is based on the collator's prev_blocks_ids (their chain view),
                    // while the parent slot is from the Simplex FSM. These can legitimately diverge when:
                    // 1. The FSM parent is an older notarized block
                    // 2. The collator's chain has more finalized blocks
                    // Seqno validation is deferred until finalized state is materialized.
                    log::debug!(
                        "Session {} on_candidate_received: seqno differs from parent-based \
                        expectation for slot={slot}, received seqno={received_seqno}, \
                        expected={expected_seqno} (parent_seqno={parent_seqno}, \
                        is_empty={is_empty}). Allowing through - finalized path will resolve it.",
                        &self.session_id().to_hex_string()[..8],
                    );
                }
            }
            // If parent not yet received, we can't validate seqno - allow it through
            // Validation will happen when finalized state is applied.
        } else {
            // No parent (first block in epoch) - seqno is based on the session's initial_block_seqno
            // which may be > 1 if this is not the first session (e.g., after zerostate, seqno=1, but
            // subsequent sessions continue from their start seqno).
            // We don't validate first block seqno at receive time - defer to finalized application.

            // INVARIANT: First block (no parent) cannot be empty
            // Empty blocks inherit parent's BlockIdExt, so they require a parent
            if is_empty {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "first_block_cannot_be_empty",
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: INVARIANT VIOLATION - first block (slot={}) \
                    cannot be empty (empty blocks require parent). Rejecting.",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
                return;
            }

            // Genesis-parent candidates at slot > 0 are normal in Simplex: when early
            // slots are skipped, subsequent leaders produce blocks with parent_id=None.
            if slot.value() != 0 {
                log::trace!(
                    "Session {} on_candidate_received: genesis-parent block at slot={} \
                    (early slots were skipped)",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
            }

            log::debug!(
                "Session {} on_candidate_received: first block (slot={}) has seqno={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                received_seqno
            );
        }

        // Extract actual block data from RawCandidate (not the TL wrapper)
        // This is what validation/finalization callbacks consume.
        let gen_utime_ms = raw_candidate
            .block
            .as_block()
            .and_then(|block| extract_consensus_gen_utime_ms(&block.collated_data));
        let (block_data, collated_data) = match raw_candidate.block.as_block() {
            Some(block) => (
                consensus_common::ConsensusCommonFactory::create_block_payload(block.data.clone()),
                consensus_common::ConsensusCommonFactory::create_block_payload(
                    block.collated_data.clone(),
                ),
            ),
            None => (
                // Empty block - no data
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
            ),
        };
        let observed_data = block_data.clone();
        let observed_collated_data = collated_data.clone();

        let parent_id = raw_candidate.parent_id.clone();

        // Build CandidateHashData TL bytes for signature verification
        // This is the data that was hashed to produce candidate_id_hash
        let candidate_hash_data_bytes = if is_empty {
            // Empty blocks use candidateHashDataEmpty with CandidateId parent
            let Some(parent) = parent_id.as_ref() else {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "empty_candidate_missing_parent",
                    );
                }
                log::error!(
                    "Session {} on_candidate_received: empty block must have parent",
                    &self.session_id().to_hex_string()[..8]
                );
                return;
            };
            crate::utils::build_candidate_hash_data_bytes_empty(
                &block_id,
                (parent.slot, &parent.hash),
            )
        } else {
            // Non-empty blocks use candidateHashDataOrdinary
            let collated_file_hash = match raw_candidate.block.as_block() {
                Some(block) => block.collated_file_hash.clone(),
                None => UInt256::default(),
            };
            let parent_info = parent_id.as_ref().map(|p| (p.slot, &p.hash));
            crate::utils::build_candidate_hash_data_bytes(
                Some(&block_id),
                Some(&collated_file_hash),
                parent_info,
            )
        };

        let parent_metadata_present =
            parent_id.as_ref().is_none_or(|parent| self.candidate_book.contains_received(parent));
        log::trace!(
            "Session {} on_candidate_received: slot={} parent={:?} parent_metadata_present={}",
            self.session_id().to_hex_string(),
            slot,
            parent_id.as_ref().map(|p| p.slot),
            parent_metadata_present,
        );

        // Clone data needed for DB save before moving into ReceivedCandidate
        let candidate_hash_data_bytes_for_db = candidate_hash_data_bytes.clone();
        let signature_for_db = raw_candidate.signature.clone();

        self.candidate_book.insert_received(
            candidate_id.clone(),
            ReceivedCandidate {
                slot,
                source_idx: leader_idx,
                candidate_hash_data_bytes,
                block_id: block_id.clone(),
                root_hash,
                file_hash,
                data: block_data,
                collated_data,
                gen_utime_ms,
                receive_time,
                is_empty,
                parent_id: parent_id.clone(),
            },
        );

        // Save candidate info to DB (fire-and-forget, matching C++ `.start().detach()` pattern)
        self.database.save_candidate_info_to_db(
            slot,
            &id_hash,
            leader_idx,
            &candidate_hash_data_bytes_for_db,
            signature_for_db,
            self.runtime.description().get_session_id(),
            &self.telemetry,
        );

        // Remove from requested_candidates if we were waiting for this
        self.requested_candidates.remove(&candidate_id);

        if !is_empty {
            match crate::utils::extract_before_split_flag(observed_data.data()) {
                Ok(before_split) => {
                    self.consensus.insert_before_split(block_id.clone(), before_split);
                }
                Err(e) => {
                    log::trace!(
                        "Session {} on_candidate_received: failed to extract before_split flag \
                        for block_id={}: {}",
                        self.session_id().to_hex_string(),
                        block_id,
                        e
                    );
                }
            }

            let observed_flags = CandidateObservedFlags {
                body_present: true,
                parent_ready: self.simplex_state.get_notarize_certificate(slot, &id_hash).is_some(),
                local_collated: is_local_self_candidate,
            };
            self.callbacks.notify_candidate_observed(
                block_id.clone(),
                observed_data,
                observed_collated_data,
                observed_flags,
            );
        }

        // Candidate arrival can unblock deferred recursive finalization chains.
        self.with_consensus_backend(|consensus, backend| {
            consensus.retry_pending_recursive_finalization(backend)
        });

        // DEBUG: Short pattern for quick grep (RECV = candidate received)
        log::debug!(
            "Session {} RECV candidate: slot={slot}, hash={}, seqno={received_seqno}, \
            from=v{:03}, empty={is_empty}, parent_metadata_present={parent_metadata_present}",
            &self.session_id().to_hex_string()[..8],
            &id_hash.to_hex_string()[..8],
            leader_idx,
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} on_candidate_received: slot={slot}, hash={}, seqno={received_seqno}, \
            source={leader_idx}, empty={is_empty}, parent={:?}, parent_metadata_present={parent_metadata_present}",
            self.session_id().to_hex_string(),
            id_hash.to_hex_string(),
            parent_id.as_ref().map(|p| format!("{}:{}", p.slot, p.hash.to_hex_string())),
        );

        // 8. Process notarization/finalization signature-sets if provided (from query response)
        // This can be done immediately, regardless of parent-metadata availability.
        // Clone id_hash before use for certificates
        let id_hash_for_cert = id_hash.clone();
        if let Some(ref cert_bytes) = notar_cert {
            self.process_received_notar_cert(slot, &id_hash_for_cert, cert_bytes);
        }

        // 9. Admit the candidate immediately; check_validation() owns the remaining
        // WaitForParent gate and, for empties, waits until the expected normal tip can be
        // reconstructed from locally known parent metadata.
        if !parent_metadata_present {
            log::debug!(
                "Session {} on_candidate_received: slot={} hash={} is missing parent metadata, \
                but ingress no longer parks candidates behind a simplex-local resolution queue",
                &self.session_id().to_hex_string()[..8],
                slot,
                &id_hash.to_hex_string()[..8],
            );
        }
        let now = self.now();
        self.validation.register_candidate_for_validation(
            raw_candidate,
            slot,
            leader_idx,
            receive_time,
            now,
            &self.simplex_state,
            &mut self.runtime,
        );

        // Immediately process the new candidate (don't wait for next awake)
        self.check_all();
    }

    /// Record receipt of a candidate. Delegates to the telemetry aspect.
    ///
    /// Keeps ingress counters focused on peer-delivered traffic: locally
    /// generated blocks loop back through `on_candidate_received` but are not
    /// network ingress. The self-index filter lives inside
    /// [`SessionTelemetry::record_candidate_ingress`].
    #[inline]
    fn record_candidate_ingress(&self, sender_idx: ValidatorIndex, is_broadcast_candidate: bool) {
        self.telemetry.record_candidate_ingress(
            sender_idx,
            self.runtime.description().get_self_idx(),
            is_broadcast_candidate,
        );
    }

    /* Outbound repair (request scheduling / resolver availability) */

    /// Handle a reverse-bridge request from the validator layer to ensure a
    /// candidate body (and optionally its parent chain) is available.
    ///
    /// The validator calls this when collation/validation needs a parent
    /// state that hasn't been applied by the engine yet. Simplex resolves
    /// `BlockIdExt` to the internal `RawCandidateId` and triggers repair.
    ///
    /// Important: a slot may be skipped but still have a notarized candidate.
    /// The repair must handle that case — the candidate body might not have
    /// been received via the normal broadcast path.
    ///
    /// C++ equivalent: demand-driven path in `BlockProducerImpl::produce()`
    /// that triggers `StateResolverImpl::resolve()`.
    pub(crate) fn ensure_candidate_available(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        self.ensure_candidate_available_impl(block_id, opts, 0);
    }

    fn ensure_candidate_available_impl(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
        attempt: u32,
    ) {
        log::info!(
            target: "simplex_resolver",
            "SessionProcessor::ensure_candidate_available session_id={} block_id={} \
            purpose={:?} include_parent_chain={} attempt={}/{}",
            self.session_id().to_hex_string(),
            block_id,
            opts.purpose,
            opts.include_parent_chain,
            attempt,
            RESOLVER_AVAILABILITY_MAX_RETRIES,
        );

        let Some(candidate_id) =
            self.with_collation_backend(|c, b| c.resolve_candidate_id_by_block_id(&block_id, b))
        else {
            if attempt < RESOLVER_AVAILABILITY_MAX_RETRIES {
                let next_attempt = attempt + 1;
                let expiration_time = self.now() + RESOLVER_AVAILABILITY_RETRY_DELAY;
                log::info!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; scheduling deferred retry {}/{} in {}ms",
                    block_id,
                    opts.purpose,
                    next_attempt,
                    RESOLVER_AVAILABILITY_MAX_RETRIES,
                    RESOLVER_AVAILABILITY_RETRY_DELAY.as_millis(),
                );
                self.post_delayed_action(expiration_time, move |processor| {
                    processor.ensure_candidate_available_impl(block_id, opts, next_attempt);
                });
            } else {
                log::warn!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; exhausted {RESOLVER_AVAILABILITY_MAX_RETRIES} retries, giving up",
                    block_id,
                    opts.purpose,
                );
            }
            return;
        };

        self.request_candidate_body_for_resolver(candidate_id.clone());

        if !opts.include_parent_chain {
            return;
        }

        let mut current = candidate_id;
        let mut depth = 0u32;
        let mut depth_warned = false;
        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: deep parent chain depth={} \
                    (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) for block_id={}; \
                    continuing until hard limit={MAX_CHAIN_DEPTH}",
                    depth,
                    block_id,
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: exceeded \
                    hard MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} while resolving parents for block_id={}",
                    block_id,
                );
                self.increment_error();
                break;
            }

            let parent_id = match self
                .candidate_book
                .received(&current)
                .and_then(|received| received.parent_id.clone())
            {
                Some(parent_id) => parent_id,
                None => break,
            };

            self.request_candidate_body_for_resolver(parent_id.clone());

            if !self.candidate_book.contains_received(&parent_id) {
                log::trace!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: parent metadata missing at \
                    slot={} hash={} while resolving block_id={}; stopping chain traversal",
                    parent_id.slot,
                    &parent_id.hash.to_hex_string()[..8],
                    block_id,
                );
                break;
            }

            current = parent_id;
        }
    }

    /// Schedule a candidate request with delay if not already requested
    ///
    /// Called when we need to repair missing candidate data after learning about a
    /// finalized or otherwise required block before all body/notar data is present.
    /// Adds the (slot, hash) to `requested_candidates` and schedules a delayed action.
    /// After the delay, if the candidate is still not in `received_candidates`, requests
    /// it from peers (with want_notar=true to get NotarCert).
    ///
    /// The delay allows time for the broadcast to arrive naturally before triggering
    /// a P2P query, reducing unnecessary network traffic.
    ///
    /// Request a candidate with optional initial delay.
    ///
    /// # Parameters
    /// - `initial_delay`: Optional delay before sending the request.
    ///   - `None`: Use default `CANDIDATE_REQUEST_DELAY` (allows broadcast to arrive first)
    ///   - `Some(Duration::ZERO)`: Request immediately (for repair-critical paths)
    ///   - `Some(dur)`: Custom delay
    fn request_candidate(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        initial_delay: Option<Duration>,
    ) {
        let delay = initial_delay.unwrap_or(CANDIDATE_REQUEST_DELAY);

        let key = RawCandidateId { slot, hash: block_hash.clone() };

        if self.simplex_state.has_skip_certificate_for_slot(self.runtime.description(), slot) {
            log::trace!(
                "Session {} request_candidate: slot={} hash={} - skipped already, not requesting",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
            );
            self.requested_candidates.remove(&key);
            return;
        }

        // Throttle repeated requests for the same (slot,hash) to survive transient partitions.
        let now = self.now();
        if let Some(next_allowed_at) = self.requested_candidates.get(&key) {
            if *next_allowed_at > now {
                log::trace!(
                    "Session {} request_candidate: slot={} hash={} - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    &block_hash.to_hex_string()[..8],
                    next_allowed_at
                );
                return;
            }
        }

        // Check if we already have what we need (stubs don't count as real bodies)
        let have_body = self.candidate_book.has_real_body(&key);
        let have_notar = self.simplex_state.get_notarize_certificate(slot, &block_hash).is_some();

        if have_body && have_notar {
            return;
        }

        if delay.is_zero() {
            self.requested_candidates.insert(key.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::debug!(
                "Session {} request_candidate: requesting slot={slot} hash={} immediately \
                (body={}, notar={})",
                &self.session_id().to_hex_string()[..8],
                &block_hash.to_hex_string()[..8],
                !have_body,
                !have_notar,
            );

            self.receiver.request_candidate(slot.value(), block_hash);
        } else {
            self.requested_candidates
                .insert(key.clone(), now + delay + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::trace!(
                "Session {} request_candidate: scheduling request for slot={} hash={} in {:?}",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
                delay,
            );

            let session_id = self.session_id().clone();
            let expiration_time = now + delay;

            self.post_delayed_action(expiration_time, move |processor: &mut SessionProcessor| {
                let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
                if !processor.requested_candidates.contains_key(&candidate_id) {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - cancelled before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }
                if processor
                    .simplex_state
                    .has_skip_certificate_for_slot(processor.runtime.description(), slot)
                {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - skipped before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    processor.requested_candidates.remove(&candidate_id);
                    return;
                }
                let have_body = processor.candidate_book.has_real_body(&candidate_id);
                let have_notar =
                    processor.simplex_state.get_notarize_certificate(slot, &block_hash).is_some();

                if have_body && have_notar {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} - already have \
                        what we need",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }

                log::debug!(
                    "Session {} delayed_request_candidate: requesting slot={slot} hash={} from \
                    peers (body={}, notar={})",
                    &session_id.to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                    !have_body,
                    !have_notar,
                );

                processor.receiver.request_candidate(slot.value(), block_hash);
                processor
                    .requested_candidates
                    .insert(candidate_id, processor.now() + CANDIDATE_REQUEST_RETRY_INTERVAL);
            });
        }
    }

    /// Resolver-driven candidate body request.
    ///
    /// Unlike `request_candidate`, this path is used by validator-side state resolution and
    /// must still request a candidate even when the slot already has a skip certificate.
    /// (A slot can be skipped and still have a notarized block body needed for parent state.)
    fn request_candidate_body_for_resolver(&mut self, candidate_id: RawCandidateId) {
        let now = self.now();

        if self.candidate_book.has_real_body(&candidate_id) {
            return;
        }

        if let Some(next_allowed_at) = self.requested_candidates.get(&candidate_id) {
            if *next_allowed_at > now {
                log::trace!(
                    target: "simplex_resolver",
                    "Session {} request_candidate_body_for_resolver: slot={} hash={} \
                    - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                    next_allowed_at,
                );
                return;
            }
        }

        let skipped = self
            .simplex_state
            .has_skip_certificate_for_slot(self.runtime.description(), candidate_id.slot);

        log::debug!(
            target: "simplex_resolver",
            "Session {} request_candidate_body_for_resolver: requesting slot={} hash={} \
            immediately (skipped_slot={})",
            &self.session_id().to_hex_string()[..8],
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            skipped,
        );

        self.requested_candidates
            .insert(candidate_id.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);
        self.receiver.request_candidate(candidate_id.slot.value(), candidate_id.hash.clone());
    }

    fn cancel_candidate_repairs_for_slot(&mut self, slot: SlotIndex) {
        let before = self.requested_candidates.len();
        self.requested_candidates.retain(|candidate_id, _| candidate_id.slot != slot);
        let removed_requests = before.saturating_sub(self.requested_candidates.len());
        let removed_missing_body = self.telemetry.forget_missing_body_log(slot.value());

        self.receiver.cancel_candidate_requests_for_slot(slot.value());

        if removed_requests > 0 || removed_missing_body {
            log::trace!(
                "Session {} cancel_candidate_repairs_for_slot: slot={slot} \
                removed_requests={removed_requests} removed_missing_body={removed_missing_body}",
                &self.session_id().to_hex_string()[..8]
            );
        }
    }

    /* Serving inbound candidate queries (SXRCV fallback) */

    /// Handle RequestCandidate query fallback when receiver's resolver_cache misses.
    ///
    /// Called from SXRCV thread via ReceiverListener when a peer's RequestCandidate query
    /// cannot be fully answered from the in-memory resolver_cache. Attempts to reconstruct
    /// requested candidate body and/or notar parts from:
    ///   1. `candidate_data_cache` (in-memory, fast path)
    ///   2. SimplexDB `CandidateInfoRecord` (empty blocks only -- reconstructed from metadata)
    ///
    /// Non-empty blocks not in the in-memory cache return an empty response; the
    /// querying peer will retry with other validators. This matches C++ behavior
    /// where `CandidateResolver` only loads from its own consensus DB, never from
    /// the validator manager.
    ///
    /// Reference: C++ `CandidateResolver::try_load_candidate_data_from_db()`
    /// TODO: LK: move DB operations to background thread
    pub(crate) fn handle_candidate_query_fallback(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        want_candidate: bool,
        want_notar: bool,
        response_callback: crate::QueryResponseCallback,
    ) {
        check_execution_time!(50_000);

        let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
        let session_hex = &self.session_id().to_hex_string()[..8];

        // Candidate and notar can be requested independently. Build each part
        // from the best available source and return partials when only one part exists.
        let mut candidate_bytes = Vec::new();

        if want_candidate {
            // 1. Fast path: in-memory candidate_data_cache
            if let Some(bytes) = self.candidate_book.cached_data(&candidate_id) {
                log::debug!(
                    "Session {session_hex} candidate_query_fallback: \
                    candidate cache HIT for slot={slot} hash={} ({}B)",
                    &block_hash.to_hex_string()[..8],
                    bytes.len()
                );
                candidate_bytes.clone_from(bytes);
            } else {
                // 2. DB path: candidate metadata
                let candidate_info = self.database.load_candidate_info_from_db(
                    &candidate_id,
                    self.runtime.description().get_session_id(),
                );

                // 3. Persisted payload (works for both empty and non-empty blocks)
                const DB_TIMEOUT: Duration = Duration::from_secs(2);
                match self.database.db().load_candidate_payload_by_id(&candidate_id, DB_TIMEOUT) {
                    Ok(Some(payload_bytes)) => {
                        log::debug!(
                            "Session {session_hex} candidate_query_fallback: \
                            loaded payload from DB for slot={slot} ({}B)",
                            payload_bytes.len()
                        );
                        candidate_bytes = payload_bytes;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!(
                            "Session {session_hex} candidate_query_fallback: \
                            DB payload load error for slot={slot}: {e}"
                        );
                    }
                }

                // 4. Metadata reconstruction for empty blocks when payload missing.
                if candidate_bytes.is_empty() {
                    if let Some(info) = candidate_info.as_ref() {
                        let is_empty = matches!(
                            info.candidate_hash_data,
                            CandidateHashData::Consensus_CandidateHashDataEmpty(_)
                        );
                        if is_empty {
                            match self
                                .reconstruct_empty_candidate_data_from_info(&candidate_id, info)
                            {
                                Ok(bytes) => {
                                    log::debug!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        reconstructed empty block for slot={slot} ({}B)",
                                        bytes.len()
                                    );
                                    candidate_bytes = bytes;
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        failed to reconstruct empty block for slot={slot}: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let notar_bytes = if want_notar {
            self.database.load_notar_cert_bytes_from_db(
                &candidate_id,
                self.runtime.description().get_session_id(),
            )
        } else {
            Vec::new()
        };

        if candidate_bytes.is_empty() && notar_bytes.is_empty() {
            log::debug!(
                "Session {} candidate_query_fallback: NOT FOUND for slot={} hash={} \
                (want_candidate={}, want_notar={})",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                want_candidate,
                want_notar,
            );
        } else {
            log::debug!(
                "Session {} candidate_query_fallback: responding slot={} hash={} \
                candidate_bytes={} notar_bytes={}",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                candidate_bytes.len(),
                notar_bytes.len()
            );
        }

        Self::send_candidate_and_cert_response(candidate_bytes, notar_bytes, response_callback);
    }

    /// Build and send CandidateAndCert response.
    fn send_candidate_and_cert_response(
        candidate_bytes: Vec<u8>,
        notar_bytes: Vec<u8>,
        response_callback: crate::QueryResponseCallback,
    ) {
        use consensus_common::ConsensusCommonFactory;

        let response =
            CandidateAndCert { candidate: candidate_bytes.into(), notar: notar_bytes.into() };

        let result = match serialize_boxed(&response.into_boxed()) {
            Ok(bytes) => Ok(ConsensusCommonFactory::create_block_payload(bytes)),
            Err(e) => Err(error!("Failed to serialize fallback response: {}", e)),
        };
        response_callback(result);
    }

    /// Reconstruct CandidateData::Consensus_Empty bytes from CandidateInfoRecord.
    fn reconstruct_empty_candidate_data_from_info(
        &self,
        candidate_id: &RawCandidateId,
        candidate_info: &crate::database::CandidateInfoRecord,
    ) -> Result<Vec<u8>> {
        let parent_id = match &candidate_info.candidate_hash_data {
            CandidateHashData::Consensus_CandidateHashDataEmpty(empty) => {
                let slot = SlotIndex(empty.parent.slot as u32);
                let hash = empty.parent.hash.clone();
                (slot, hash)
            }
            _ => return Err(error!("Expected empty hash data")),
        };

        let block_id = if let Some(rc) = self.candidate_book.received(candidate_id) {
            rc.block_id.clone()
        } else {
            return Err(error!(
                "Cannot reconstruct empty block: no block_id available for slot={}",
                candidate_id.slot
            ));
        };

        let parent =
            CandidateId { slot: parent_id.0.value() as i32, hash: parent_id.1 }.into_boxed();

        let tl_empty = CandidateDataEmpty {
            slot: candidate_id.slot.value() as i32,
            parent,
            block: block_id,
            signature: candidate_info.signature.clone(),
        };

        let candidate_data = CandidateData::Consensus_Empty(tl_empty);
        serialize_boxed(&candidate_data)
            .map_err(|e| error!("Failed to serialize empty CandidateData: {}", e))
    }
}

// ======================================================================
// Collation
// ======================================================================
//
// Session-side collation: the `CollationController` seam + adapter types and the
// `check_all` dispatch entry points. Pipeline internals live on `CollationController`.

/// Composition-root adapter bridging a
/// [`ControllerQueue<CollationController>`] to the real SXMAIN task queue.
///
/// The collation analogue of [`ValidationQueueAdapter`]: the one place allowed
/// to name *both* `SessionProcessor` and `CollationController`, owning the
/// `&mut SessionProcessor -> &mut p.collation` projection so the controller
/// never has to. The controller holds only the generic
/// `ControllerQueuePtr<CollationController>` handle and stays independently
/// constructible (and testable) without this type.
struct CollationQueueAdapter {
    /// Shared handle to the main session task queue (the SXMAIN mailbox).
    task_queue: TaskQueuePtr,
}

impl ControllerQueue<CollationController> for CollationQueueAdapter {
    fn post_boxed(&self, task: ControllerTask<CollationController>) {
        crate::task_queue::post_closure(&self.task_queue, move |p: &mut SessionProcessor| {
            p.with_collation_backend(move |collation, backend| task(collation, backend));
        });
    }

    fn post_delayed_boxed(&self, at: SystemTime, task: ControllerTask<CollationController>) {
        // Delayed actions live on the stack-bound `SessionRuntime` scheduler,
        // reachable only on SXMAIN with `&mut SessionProcessor`. Bounce through
        // the immediate queue: the immediate task (run on SXMAIN) schedules the
        // projected delayed task, mirroring [`ValidationQueueAdapter`].
        crate::task_queue::post_closure(&self.task_queue, move |p: &mut SessionProcessor| {
            p.runtime.post_delayed_action(
                at,
                Box::new(move |p2: &mut SessionProcessor| {
                    p2.with_collation_backend(move |collation, backend| task(collation, backend));
                }),
            );
        });
    }
}

/// Borrowing [`CollationBackend`] view built fresh at drain.
///
/// The collation analogue of [`ValidationBackendAdapter`]:
/// [`SessionProcessor::with_collation_backend`] constructs one from disjoint
/// `&mut SessionProcessor` fields immediately before a collation re-entry,
/// hands `&mut dyn CollationBackend` to the controller, and drops it when the
/// call returns (RAII). Borrows the FSM state (progress cursor / current leader
/// window) and the runtime (wake horizon, slot-generation state, session id),
/// the network sender and candidate-info DB store for the *synchronous*
/// broadcast / persist publication effects, the telemetry sink, and the SXMAIN
/// mailbox so the *deferred* effects (`request_parent_candidate`,
/// `self_receive_candidate`) can bounce back onto the main loop.
struct CollationBackendAdapter<'a> {
    /// Consensus FSM state, read for the collation progress cursor
    /// (`first_non_progressed_slot`) and the current leader window.
    simplex_state: &'a SimplexState,
    /// Received-candidate book. Backs the "book seam" reads
    /// (`book_received_block_id`, `book_received_gen_utime_ms`,
    /// `book_candidate_id_by_block_id`) so the controller's parent-resolution
    /// helpers can fall back to the book without it crossing the seam as a
    /// borrowed collection. Shared (`&`) — the collation re-entries never mutate
    /// the book.
    candidate_book: &'a CandidateBook,
    /// Per-block before-split flags, read by the `before_split_flag` seam for
    /// the empty-block policy (`resolve_parent_before_split_flag`). Shared (`&`).
    before_split_by_block_id: &'a HashMap<BlockIdExt, bool>,
    /// Finalized-head block id + before-split bit, the fallback for
    /// `before_split_flag` when the parent is exactly the finalized head.
    /// Shared (`&`) / copied.
    finalized_head_block_id: &'a Option<BlockIdExt>,
    finalized_head_before_split: bool,
    /// Finalized-head / finalization-cursor seqnos — copied scalars read by the
    /// empty-block policy ([`CollationController::should_generate_empty_block`])
    /// and its invariant diagnostics in `execute_collation_attempt`.
    finalized_head_seqno: Option<u32>,
    last_consensus_finalized_seqno: Option<u32>,
    last_mc_finalized_seqno: Option<u32>,
    /// Runtime handle: lowers the main-loop wake horizon (pacing gate), reports
    /// *and mutates* per-slot generated / pending-generate state (pipeline fill +
    /// publication), and supplies the session-start prev-blocks plus the session
    /// id for the candidate-info DB write. `&mut` for the per-slot setters
    /// (`set_pending_generate` / `set_generated` / `set_sent_generated`).
    runtime: &'a mut SessionRuntime,
    /// Network sender. The `broadcast_candidate` effect forwards directly to
    /// `Receiver::send_block_broadcast` (a `&self` call), so the generated
    /// candidate is broadcast *synchronously* — preserving the C++-parity
    /// "broadcast immediately" invariant rather than slipping a drain.
    receiver: &'a ReceiverPtr,
    /// Candidate-info store. The `persist_candidate_info` effect writes through
    /// `DatabaseController::save_candidate_info_to_db` (a `&mut self` call), so
    /// this is the one mutable borrow the backend holds. Kept synchronous so the
    /// DB write starts before the broadcast, exactly as the previous in-line call
    /// did (the early `WaitCandidateInfoStored` signal for our own vote).
    database: &'a mut DatabaseController,
    /// Telemetry sink threaded into the candidate-info DB write.
    telemetry: &'a SessionTelemetry,
    /// SXMAIN mailbox handle. The deferred effects (`request_parent_candidate`,
    /// `self_receive_candidate`) need `&mut SessionProcessor` to run their
    /// dispatch machinery, which the borrowing backend cannot provide, so they
    /// bounce a closure onto the session main loop through this handle. The
    /// collation retry and next-slot self-loop re-dispatch use the controller
    /// task queue for the same reason.
    task_queue: &'a TaskQueuePtr,
}

impl CollationBackend for CollationBackendAdapter<'_> {
    fn first_non_progressed_slot(&self) -> SlotIndex {
        self.simplex_state.get_first_non_progressed_slot()
    }

    fn current_leader_window_idx(&self) -> WindowIndex {
        self.simplex_state.get_current_leader_window_idx()
    }

    fn has_available_parent(&self, slot: SlotIndex) -> bool {
        self.simplex_state.has_available_parent(self.runtime.description(), slot)
    }

    fn get_available_parent(&self, slot: SlotIndex) -> Option<crate::block::CandidateParentInfo> {
        self.simplex_state.get_available_parent(self.runtime.description(), slot)
    }

    fn book_received_block_id(&self, id: &RawCandidateId) -> Option<BlockIdExt> {
        self.candidate_book.received(id).map(|c| c.block_id.clone())
    }

    fn book_received_gen_utime_ms(&self, id: &RawCandidateId) -> Option<u64> {
        self.candidate_book.received(id).and_then(|c| c.gen_utime_ms)
    }

    fn book_candidate_id_by_block_id(&self, block_id: &BlockIdExt) -> Option<RawCandidateId> {
        self.candidate_book.find_received_by_block_id(block_id)
    }

    fn before_split_flag(&self, parent_block_id: &BlockIdExt) -> Option<bool> {
        self.before_split_by_block_id.get(parent_block_id).copied().or_else(|| {
            self.finalized_head_block_id
                .as_ref()
                .filter(|finalized| *finalized == parent_block_id)
                .map(|_| self.finalized_head_before_split)
        })
    }

    fn request_wake_at(&self, at: SystemTime) {
        self.runtime.set_next_awake_time(at);
    }

    fn is_generated(&self, slot: SlotIndex) -> bool {
        self.runtime.is_generated(slot)
    }

    fn is_pending_generate(&self, slot: SlotIndex) -> bool {
        self.runtime.is_pending_generate(slot)
    }

    fn set_pending_generate(&mut self, slot: SlotIndex, value: bool) {
        let now = self.runtime.description().get_time();
        self.runtime.set_pending_generate(slot, value, now);
    }

    fn set_generated(&mut self, slot: SlotIndex, value: bool) {
        let now = self.runtime.description().get_time();
        self.runtime.set_generated(slot, value, now);
    }

    fn set_sent_generated(&mut self, slot: SlotIndex, value: bool) {
        let now = self.runtime.description().get_time();
        self.runtime.set_sent_generated(slot, value, now);
    }

    fn session_start_prev_blocks(&self) -> Vec<BlockIdExt> {
        self.runtime.session_start_prev_blocks().to_vec()
    }

    fn finalized_head_before_split(&self) -> bool {
        self.finalized_head_before_split
    }

    fn finalized_head_seqno(&self) -> Option<u32> {
        self.finalized_head_seqno
    }

    fn last_consensus_finalized_seqno(&self) -> Option<u32> {
        self.last_consensus_finalized_seqno
    }

    fn last_mc_finalized_seqno(&self) -> Option<u32> {
        self.last_mc_finalized_seqno
    }

    fn forget_self_collation_tracking(&self, slot: SlotIndex, reason: &str) {
        // Synchronous telemetry forget: `reset` already holds `&mut
        // SessionProcessor` through this borrowing backend and the underlying
        // call is `&self`, so there is no SXMAIN bounce — it runs from the
        // on-loop pipeline reset, not the off-thread collation callback.
        let now = self.runtime.description().get_time();
        self.telemetry.forget_self_collation_tracking(
            slot,
            reason,
            self.runtime.description(),
            now,
        );
    }

    fn broadcast_candidate(&self, slot: u32, candidate_hash: UInt256, candidate: CandidateData) {
        // `Receiver::send_block_broadcast` is a `&self` call (it posts internally
        // to the receiver's own queue), so forward synchronously — the candidate
        // is broadcast immediately, matching C++ parity, with no SXMAIN slip.
        self.receiver.send_block_broadcast(slot, candidate_hash, candidate);
    }

    fn persist_candidate_info(
        &mut self,
        slot: SlotIndex,
        candidate_hash: UInt256,
        self_idx: ValidatorIndex,
        candidate_hash_data_bytes: Vec<u8>,
        signature: Vec<u8>,
    ) {
        // Synchronous DB write (the hash-data bytes were assembled by the caller
        // while `GeneratedBlockDesc` was still borrowed). Kept synchronous so the
        // write starts before the broadcast, preserving the early
        // `WaitCandidateInfoStored` signal for our own NotarizeVote.
        self.database.save_candidate_info_to_db(
            slot,
            &candidate_hash,
            self_idx,
            &candidate_hash_data_bytes,
            signature,
            self.runtime.description().get_session_id(),
            self.telemetry,
        );
    }

    fn request_parent_candidate(&self, slot: SlotIndex, hash: UInt256) {
        // The fetch runs the full `request_candidate` machinery, which needs
        // `&mut SessionProcessor`; bounce it onto SXMAIN from the
        // `check_collation` parent-resolution gate.
        crate::task_queue::post_closure(self.task_queue, move |p: &mut SessionProcessor| {
            p.request_candidate(slot, hash, None);
        });
    }

    fn self_receive_candidate(&self, self_idx: u32, candidate: CandidateData) {
        // Loop the locally generated candidate back through the normal receive
        // path. `on_candidate_received` needs `&mut SessionProcessor`, so bounce
        // it onto SXMAIN from the `generated_block` publication path.
        crate::task_queue::post_closure(self.task_queue, move |p: &mut SessionProcessor| {
            p.on_candidate_received(self_idx, candidate, None);
        });
    }
}

impl SessionProcessor {
    /* Controller seam (split-borrow) */

    /// Run `f` against `&mut self.collation` with a freshly built borrowing
    /// [`CollationBackend`].
    ///
    /// Splits `&mut self` into the disjoint pieces a collation re-entry needs —
    /// `&mut self.collation` plus a fresh [`CollationBackendAdapter`] over the
    /// FSM state — invokes `f`, then drops the backend (RAII). Mirrors
    /// [`Self::with_validation_backend`]; the single place that assembles the
    /// collation controller's backend view from `SessionProcessor`. Used by the
    /// collation retry-gate re-entry in [`CollationController::on_collation_failed_impl`] and by
    /// the `check_collation` policy gates (pacing, stale-window, stale
    /// precollations, pipeline fill).
    fn with_collation_backend<R>(
        &mut self,
        f: impl for<'b> FnOnce(&mut CollationController, &'b mut dyn CollationBackend) -> R,
    ) -> R {
        let Self {
            collation,
            consensus,
            simplex_state,
            runtime,
            receiver,
            database,
            telemetry,
            task_queue,
            candidate_book,
            ..
        } = self;
        // The finalized-head cursor + before-split map + finalization-cursor
        // seqnos + MC applied-top floor that the empty-block policy reads now
        // live on the consensus controller; source them through its `pub(crate)`
        // accessors (shared reborrows of `consensus`, disjoint from the `&mut
        // collation` borrow).
        let mut backend = CollationBackendAdapter {
            simplex_state: &*simplex_state,
            candidate_book: &*candidate_book,
            before_split_by_block_id: consensus.before_split_by_block_id(),
            finalized_head_block_id: consensus.finalized_head_block_id(),
            finalized_head_before_split: consensus.finalized_head_before_split(),
            finalized_head_seqno: consensus.finalized_head_seqno(),
            last_consensus_finalized_seqno: consensus.last_consensus_finalized_seqno(),
            last_mc_finalized_seqno: consensus.last_mc_finalized_seqno(),
            runtime,
            receiver: &*receiver,
            database,
            telemetry: &**telemetry,
            task_queue: &*task_queue,
        };
        f(collation, &mut backend)
    }

    /* Session-side dispatch */

    /// Production collation entry point, called from `check_all` on the session
    /// main loop. Thin dispatch onto [`CollationController::check_collation`],
    /// which owns the leader / pacing / pipeline-fill / parent-resolution
    /// decision and the collation dispatch (see the controller module docs).
    fn check_collation(&mut self) {
        self.with_collation_backend(|collation, backend| collation.check_collation(backend));
    }

    /// Remove the precollation entry for `slot`. Thin wrapper over
    /// [`CollationController::remove_precollated_block`]; self-collation tracking
    /// in [`SessionTelemetry`] is intentionally left untouched (its terminal
    /// semantics are managed explicitly by callers).
    fn remove_precollated_block(&mut self, slot: SlotIndex) {
        self.collation.remove_precollated_block(slot);
    }

    /// Flush the precollation pipeline (session stop, leader-window change, or a
    /// progress-cursor jump past queued slots). Thin dispatch onto
    /// [`CollationController::reset`], which cancels in-flight precollations,
    /// forgets their self-collation telemetry, and invalidates the window-local
    /// chain head.
    fn reset_precollations(&mut self) {
        self.with_collation_backend(|collation, backend| collation.reset(backend));
    }
}

// ======================================================================
// Validation
// ======================================================================
//
// Session-side validation: the `ValidationController` seam + adapter types and the
// scheduling -> FSM hand-off. Pipeline internals live on `ValidationController`.

/// Composition-root adapter bridging a
/// [`ControllerQueue<ValidationController>`] to the real SXMAIN task queue.
///
/// This is the one place allowed to name *both* `SessionProcessor` and
/// `ValidationController`: it owns the `&mut SessionProcessor -> &mut
/// p.validation` projection so the controller never has to. The controller
/// holds only the generic `ControllerQueuePtr<ValidationController>` handle and
/// stays independently constructible (and testable) without this type.
struct ValidationQueueAdapter {
    /// Shared handle to the main session task queue (the SXMAIN mailbox).
    task_queue: TaskQueuePtr,
}

impl ControllerQueue<ValidationController> for ValidationQueueAdapter {
    fn post_boxed(&self, task: ControllerTask<ValidationController>) {
        crate::task_queue::post_closure(&self.task_queue, move |p: &mut SessionProcessor| {
            p.with_validation_backend(move |validation, backend| task(validation, backend));
        });
    }

    fn post_delayed_boxed(&self, at: SystemTime, task: ControllerTask<ValidationController>) {
        // Delayed actions live on the stack-bound `SessionRuntime` scheduler,
        // reachable only on SXMAIN with `&mut SessionProcessor`. Bounce through
        // the immediate queue: the immediate task (run on SXMAIN) schedules the
        // projected delayed task, mirroring how off-thread callbacks today post
        // an immediate closure that then calls `post_delayed_action`.
        crate::task_queue::post_closure(&self.task_queue, move |p: &mut SessionProcessor| {
            p.runtime.post_delayed_action(
                at,
                Box::new(move |p2: &mut SessionProcessor| {
                    p2.with_validation_backend(move |validation, backend| {
                        task(validation, backend)
                    });
                }),
            );
        });
    }
}

/// Borrowing [`ValidationBackend`] view built fresh at drain.
///
/// The collation analogue of [`CollationBackendAdapter`]:
/// [`SessionProcessor::with_validation_backend`] constructs one from disjoint
/// `&mut SessionProcessor` fields immediately before a synchronous call
/// (`check_validation`) or a deferred re-entry (`candidate_decision_*`), hands
/// `&mut dyn ValidationBackend` to the controller, and drops it when the call
/// returns (RAII). Borrows the FSM state (for the `WaitForParent` walk), the
/// candidate book + collation controller + accepted normal head (parent-tip
/// resolution and parent gen-utime), and the runtime (wake horizon); holds the
/// SXMAIN mailbox so `request_candidate` can bounce back onto the main loop, and
/// copies the finalized-head seqno by value so the read is current.
struct ValidationBackendAdapter<'a> {
    /// Consensus FSM state, borrowed for the read-only `WaitForParent`
    /// parent-gating walk (`evaluate_wait_for_parent`).
    simplex_state: &'a SimplexState,
    /// Received-candidate book, backing `resolve_parent_tip` (parent-chain walk)
    /// and the book half of `parent_gen_utime_ms`. Shared (`&`).
    candidate_book: &'a CandidateBook,
    /// Collation controller, read for the self-collation cache / local-chain-head
    /// half of `parent_gen_utime_ms`. Shared (`&`) — validation never mutates it.
    collation: &'a CollationController,
    /// Accepted normal head block id, the seed `resolve_parent_tip` uses for the
    /// no-parent / finalized-boundary case. Shared (`&`).
    accepted_normal_head_block_id: &'a Option<BlockIdExt>,
    /// Runtime handle, used to lower the main-loop wake horizon (`request_wake` /
    /// `request_wake_at`).
    runtime: &'a SessionRuntime,
    /// SXMAIN mailbox handle. `request_candidate` needs `&mut SessionProcessor`
    /// for the shared throttle map, so it bounces a closure onto the main loop
    /// through this handle (mirrors `CollationBackend::request_parent_candidate`).
    task_queue: &'a TaskQueuePtr,
    /// Finalized head seqno copied by value (so it reflects drain-time state).
    /// TODO: LK: move to SessionRuntime
    finalized_head_seqno: Option<u32>,
}

impl ValidationBackend for ValidationBackendAdapter<'_> {
    fn finalized_head_seqno(&self) -> Option<u32> {
        self.finalized_head_seqno
    }

    fn request_wake(&self) {
        let now = self.runtime.description().get_time();
        self.runtime.set_next_awake_time(now);
    }

    fn request_wake_at(&self, at: SystemTime) {
        self.runtime.set_next_awake_time(at);
    }

    fn simplex_state(&self) -> &SimplexState {
        self.simplex_state
    }

    fn resolve_parent_tip(&self, parent_id: Option<&RawCandidateId>) -> ParentTipResolution {
        self.candidate_book
            .resolve_parent_tip(parent_id, self.accepted_normal_head_block_id.as_ref())
    }

    fn parent_gen_utime_ms(&self, parent: &crate::block::CandidateParentInfo) -> Option<u64> {
        self.collation.resolve_parent_gen_utime_ms_via_book(parent, self.candidate_book)
    }

    fn request_candidate(&self, slot: SlotIndex, hash: UInt256, delay: Option<Duration>) {
        // The fetch runs the full `request_candidate` machinery (shared throttle
        // map), which needs `&mut SessionProcessor`; bounce it onto SXMAIN.
        crate::task_queue::post_closure(self.task_queue, move |p: &mut SessionProcessor| {
            p.request_candidate(slot, hash, delay);
        });
    }
}

impl SessionProcessor {
    /* Controller seam (split-borrow) */

    /// Run `f` against `&mut self.validation` with a freshly built borrowing
    /// [`ValidationBackend`].
    ///
    /// Splits `&mut self` into the disjoint pieces the validation pipeline needs
    /// — `&mut self.validation` plus a fresh [`ValidationBackendAdapter`] over the
    /// FSM state, candidate book, collation controller, accepted normal head,
    /// runtime, SXMAIN mailbox, and a copy of the finalized head — invokes `f`,
    /// then drops the backend (RAII). Mirrors [`Self::with_collation_backend`];
    /// the single place that assembles the validation controller's backend view
    /// from `SessionProcessor`. Used by the thin `check_validation` dispatch and
    /// the `candidate_decision_*` synchronous calls, and by
    /// [`ValidationQueueAdapter`] when draining a queued task.
    fn with_validation_backend<R>(
        &mut self,
        f: impl for<'b> FnOnce(&mut ValidationController, &'b mut dyn ValidationBackend) -> R,
    ) -> R {
        let Self {
            validation,
            simplex_state,
            candidate_book,
            collation,
            consensus,
            runtime,
            task_queue,
            ..
        } = self;
        let mut backend = ValidationBackendAdapter {
            simplex_state: &*simplex_state,
            candidate_book: &*candidate_book,
            collation: &*collation,
            // Accepted-normal-head + finalized-head seqno now live on the
            // consensus controller; source them through its `pub(crate)`
            // accessors (shared reborrow of `consensus`, disjoint from the `&mut
            // validation` borrow). The seqno is copied by value so the read
            // reflects drain-time state.
            accepted_normal_head_block_id: consensus.accepted_normal_head_block_id(),
            runtime: &*runtime,
            task_queue: &*task_queue,
            finalized_head_seqno: consensus.finalized_head_seqno(),
        };
        f(validation, &mut backend)
    }

    /* Validation pipeline (scheduling -> FSM hand-off) */

    /// Run the candidate-validation scheduling loop.
    ///
    /// Thin composition-root dispatch: builds the borrowing [`ValidationBackend`]
    /// and delegates to [`ValidationController::check_validation`], where the loop
    /// (and `try_approve_block` / empty-parent-tip resolution) now lives. Called
    /// from [`Self::check_all`].
    fn check_validation(&mut self) {
        self.with_validation_backend(|validation, backend| validation.check_validation(backend));
    }

    /// Process validated candidates and feed to FSM
    ///
    /// Called from check_all() after check_validation().
    fn process_validated_candidates(&mut self) {
        check_execution_time!(10_000);

        // Process validated candidates (slot tracking available for future use)
        let _current_slot = self.simplex_state.get_first_non_progressed_slot();

        while let Some(candidate) = self.validation.pop_validated() {
            let candidate_id =
                RawCandidateId { slot: candidate.id.slot, hash: candidate.id.hash.clone() };

            let session_short = self.session_id().to_hex_string()[..8].to_string();
            let candidate_id_for_log = candidate_id.clone();
            self.ensure_candidate_info_stored(&candidate_id, true, false, move |processor, res| {
                match res {
                    Ok(()) => processor.feed_validated_candidate_to_fsm(candidate),
                    Err(e) => log::warn!(
                        "Session {session_short} process_validated_candidates: skipping \
                         candidate slot={} hash={} — candidateInfo not durable: {e}",
                        candidate_id_for_log.slot.value(),
                        &candidate_id_for_log.hash.to_hex_string()[..8],
                    ),
                }
            });
            // Loop continues draining the rest of the validated_candidates queue;
            // each candidate's callback fires either inline or from the registry.
        }
    }
}

// ======================================================================
// Consensus
// ======================================================================
//
// Session-side consensus: the `ConsensusController` seam + adapter types, the
// vote / certificate / notar-cert / standstill / skip flow, and the FSM event pump
// that drives it. Finalization internals live on `ConsensusController`.

/// Borrowing [`ConsensusBackend`] view built fresh at drain.
///
/// The finalization analogue of [`ValidationBackendAdapter`] /
/// [`CollationBackendAdapter`]: [`SessionProcessor::with_consensus_backend`]
/// constructs one from disjoint `&mut SessionProcessor` fields immediately
/// before a finalization/cert handler or recursive-walk re-entry, hands
/// `&mut dyn ConsensusBackend` to the controller, and drops it when the call
/// returns (RAII). Borrows the FSM state mutably (notar-cert lookup +
/// slot-progressed check for the walk; `SimplexState::on_vote` /
/// `set_{notarize,skip,finalize}_certificate` for ingress), the candidate book
/// (body/metadata) read-only, the runtime (per-slot latency flags + wake
/// horizon) and cert / finalized DB store mutably for the *synchronous* persist
/// effects, the telemetry sink for sync-path error counting, the receiver for
/// the synchronous certificate-accept notification + bad-signature peer ban, and
/// the SXMAIN mailbox so the *deferred* effects (`request_candidate`,
/// `reset_slot_state`) can bounce back onto the main loop.
struct ConsensusBackendAdapter<'a> {
    /// Consensus FSM state. Read for the recursive-walk notar-cert lookup
    /// (`get_notarize_certificate`) and the `is_slot_progressed` reset gate;
    /// mutated by vote/certificate ingress (`SimplexState::on_vote` and
    /// `set_{notarize,skip,finalize}_certificate`), hence `&mut`.
    simplex_state: &'a mut SimplexState,
    /// Received-candidate book (body/metadata holder) backing the walk. Shared.
    candidate_book: &'a CandidateBook,
    /// Runtime handle: per-slot started-at + first-candidate-finalized latency
    /// flags (read + set) and the async-DB poll wake horizon. `&mut` for the
    /// per-slot setter and `register_pending` re-arm.
    runtime: &'a mut SessionRuntime,
    /// Cert + finalized-block DB store. The persist effects write through
    /// `DatabaseController` (a `&mut self` surface), so this is the mutable
    /// borrow the backend holds. Kept synchronous so the persist registers on
    /// the SXMAIN async-DB registry before the relay continuation runs, exactly
    /// as the previous in-line code did.
    database: &'a mut DatabaseController,
    /// Telemetry sink, used by the synchronous persist error paths to bump the
    /// error counter (the controller records milestones through its own
    /// `Arc<SessionTelemetry>`).
    telemetry: &'a SessionTelemetry,
    /// SXMAIN mailbox handle. The deferred effects (`request_candidate`,
    /// `reset_slot_state`) need `&mut SessionProcessor` to run their dispatch /
    /// cross-controller cleanup machinery, which the borrowing backend cannot
    /// provide, so they bounce a closure onto the session main loop.
    task_queue: &'a TaskQueuePtr,
    /// Receiver handle for the synchronous certificate-ingress effects:
    /// `notify_certificate_accepted` (standstill tracking on first store) and
    /// `ban_source_for_bad_signature` (temporary peer ban on a failed-verify
    /// certificate). Shared.
    receiver: &'a ReceiverPtr,
}

impl ConsensusBackendAdapter<'_> {
    /// Register an in-flight async DB write with the SXMAIN async-DB-results
    /// registry, run-from-the-borrowing-backend twin of
    /// [`SessionProcessor::post_async_db_result`].
    ///
    /// Identical semantics — `register_pending` + re-arm `next_awake_time` to
    /// `now + ASYNC_DB_POLL_DELAY` — but over the adapter's borrowed `database`
    /// + `runtime` so the cert/finalized-block persist effects keep their exact
    /// pre-refactor timing without needing `&mut SessionProcessor`. The
    /// continuation still runs `SessionProcessor`-side post-durability.
    fn post_async_db_result<F>(
        &mut self,
        op_label: &'static str,
        result: StorageAsyncResultPtr<()>,
        timeout: Duration,
        on_ready: F,
    ) where
        F: FnOnce(&mut SessionProcessor, Result<()>) + Send + 'static,
    {
        let now = self.runtime.description().get_time();
        let id = self.database.register_pending(op_label, result, Box::new(on_ready), now, timeout);

        log::trace!(
            "Session {} post_async_db_result: id={id} label='{op_label}' \
             timeout={}ms (pending_count={})",
            &self.runtime.description().get_session_id().to_hex_string()[..8],
            timeout.as_millis(),
            self.database.pending_count(),
        );

        self.runtime.set_next_awake_time(now + ASYNC_DB_POLL_DELAY);
    }
}

impl ConsensusBackend for ConsensusBackendAdapter<'_> {
    fn simplex_state(&self) -> &SimplexState {
        &*self.simplex_state
    }

    fn simplex_state_mut(&mut self) -> &mut SimplexState {
        &mut *self.simplex_state
    }

    fn save_incoming_vote(&self, record: &VoteRecord) -> Result<()> {
        // Fire-and-forget: discard the async write handle (mirrors the C++
        // `store_vote_to_db(...).detach()`); only surface an immediate
        // registration error so the caller can bump the error counter.
        self.database.db().save_vote_async(record).map(|_| ())
    }

    fn notify_certificate_accepted(&self, slot: u32, kind: StandstillCertificateType) {
        self.receiver.notify_certificate_accepted(slot, kind);
    }

    fn ban_source_for_bad_signature(&self, source_idx: u32) {
        self.receiver.ban_source_for_bad_signature(source_idx);
    }

    fn send_vote(&self, signed_vote: TlVote) {
        // Receiver serializes the TL vote, broadcasts to all validators, and
        // loops it back through the listener for our own FSM accounting.
        self.receiver.send_vote(signed_vote);
    }

    fn persist_our_vote(&mut self, tl_vote: &TlVote) {
        // Run-from-the-borrowing-backend twin of the former
        // `SessionProcessor::persist_our_vote_before_broadcast`: serialize the
        // signed vote, register the async `save_vote_async` write on the SXMAIN
        // registry (continuation only logs / bumps the error counter), and
        // return without gating the broadcast.
        let serialized =
            consensus_common::serialize_tl_boxed_object!(&tl_vote.clone().into_boxed());
        let vote_hash = UInt256::from_slice(&sha256_digest(&serialized));
        let record = VoteRecord {
            vote_hash,
            data: serialized.into(),
            node_idx: self.runtime.description().get_self_idx(),
            seqno: 0, // assigned by save_vote_async
        };

        let result = match self.database.db().save_vote_async(&record) {
            Ok(r) => r,
            Err(e) => {
                log::error!(
                    "Session {} broadcast_vote: failed to create vote save: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };

        let session_short =
            self.runtime.description().get_session_id().to_hex_string()[..8].to_string();
        let vote_hash_short = record.vote_hash.to_hex_string()[..8].to_string();
        let node_idx = record.node_idx;

        log::trace!(
            "Session {session_short} broadcast_vote: scheduling vote db.set \
             (hash={vote_hash_short}, node_idx={node_idx})",
        );

        self.post_async_db_result(
            "persist_our_vote_before_broadcast",
            result,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, res| match res {
                Ok(()) => {
                    log::trace!(
                        "Session {session_short} broadcast_vote: stored vote \
                         (hash={vote_hash_short}, node_idx={node_idx})",
                    );
                }
                Err(e) => {
                    log::error!(
                        "Session {session_short} broadcast_vote: failed to store vote \
                         (hash={vote_hash_short}): {e}",
                    );
                    processor.increment_error();
                }
            },
        );
    }

    fn candidate_book(&self) -> &CandidateBook {
        self.candidate_book
    }

    fn runtime(&self) -> &SessionRuntime {
        self.runtime
    }

    fn runtime_mut(&mut self) -> &mut SessionRuntime {
        self.runtime
    }

    fn request_candidate(&self, slot: SlotIndex, hash: UInt256, delay: Option<Duration>) {
        // The fetch runs the full `request_candidate` machinery (shared throttle
        // map), which needs `&mut SessionProcessor`; bounce it onto SXMAIN.
        crate::task_queue::post_closure(self.task_queue, move |p: &mut SessionProcessor| {
            p.request_candidate(slot, hash, delay);
        });
    }

    fn reset_slot_state(&self, slot: SlotIndex) {
        // `reset_slot_state` -> `cleanup_old_candidates` re-enters
        // `self.consensus.prune_below`, so it cannot run while the controller
        // borrow is live; bounce it onto SXMAIN. Behaviour-neutral: it is
        // ancient-slot GC (only touches slots `MAX_HISTORY_SLOTS` behind the
        // finalized one), so the one-turn deferral changes nothing observable.
        crate::task_queue::post_closure(self.task_queue, move |p: &mut SessionProcessor| {
            p.reset_slot_state(slot);
        });
    }

    fn persist_notar_cert_then_relay(&mut self, event: &NotarizationReachedEvent) {
        // Save notarization certificate to DB (store async result for write ordering)
        // Reference: C++ candidate-resolver.cpp NotarizationObserved handler:
        //   store_to_db(event->id, state).start().detach()
        let candidate_id = RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };

        let notar_store_result =
            if let Some(existing) = self.database.notar_cert_store_result(&candidate_id) {
                existing.clone()
            } else {
                match self.database.db().save_notar_cert_async(&candidate_id, &event.certificate) {
                    Ok(result) => {
                        self.database.insert_notar_cert_store(candidate_id.clone(), result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                        "Session {} handle_notarization_reached: failed to create notar_cert save \
                        slot={}: {e}",
                        &self.runtime.description().get_session_id().to_hex_string()[..8],
                        event.slot,
                    );
                        self.telemetry.increment_error();
                        return;
                    }
                }
            };

        // Pre-serialize the cached + relayed cert payloads SYNCHRONOUSLY, before posting
        // the persist. Serialization is cheap and predictable; doing it now keeps the
        // continuation closure small (no `Arc<NotarCert>.clone()` needed) and surfaces
        // any malformed-cert errors before we register a wait we cannot satisfy.
        //
        // VoteSignatureSet is the C++ wire format for `candidateAndCert.notar`
        // (see C++ `candidate-resolver.cpp::to_tl()`).
        let tl_sigs = event.certificate.to_tl_vote_signature_set();
        let notar_cert_bytes = match serialize_boxed(&tl_sigs) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to serialize \
                    VoteSignatureSet: {e}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                );
                self.telemetry.increment_error();
                return;
            }
        };
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to convert to TL: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };
        let cert_bytes = match serialize_boxed(&tl_cert) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to serialize certificate: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };

        // Wait-for-store guarantee: certificate must be durable before broadcast/cache.
        // Hand the wait off to the SXMAIN async-DB-results registry; the continuation runs
        // the post-persist work (cache notar VoteSignatureSet + relay full cert + cache standstill).
        //
        // C++ parity (`pool.cpp::handle_prospective_certificate`):
        //   co_await owning_bus().publish<SaveCertificate>(cert);
        //   handle_saved_certificate(*slot, cert);  // broadcast + cache
        // The persist still gates the broadcast — same as before this migration, just
        // non-blocking on SXMAIN.
        let session_short =
            self.runtime.description().get_session_id().to_hex_string()[..8].to_string();
        let block_hash_short = event.block_hash.to_hex_string()[..8].to_string();
        let slot = event.slot;
        let block_hash = event.block_hash.clone();
        let candidate_id_for_cb = candidate_id.clone();
        self.post_async_db_result(
            "handle_notarization_reached",
            notar_store_result,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, res| {
                if let Err(e) = res {
                    if is_storage_result_already_taken(&e) {
                        if processor.database.contains_notar_cert_store(&candidate_id_for_cb) {
                            // Redundant FSM re-emission: the first callback already
                            // persisted + broadcast the cert (dedup-map held the same
                            // `StorageAsyncResultPtr`, whose inner result has already
                            // been `take`n). Skip side-effects, do NOT bump the error
                            // counter. Mirrors `classify_durability_wait_outcome`.
                            log::trace!(
                                "Session {session_short} handle_notarization_reached: notar cert \
                                 result already consumed for slot={slot} (redundant FSM re-emit, \
                                 skipping cache + broadcast)",
                            );
                        } else {
                            log::error!(
                                "Session {session_short} handle_notarization_reached: notar cert \
                                 result already consumed but dedup entry is missing for slot={slot}",
                            );
                            processor.increment_error();
                        }
                    } else {
                        processor.database.remove_notar_cert_store(&candidate_id_for_cb);
                        log::error!(
                            "Session {session_short} handle_notarization_reached: failed to \
                             store notar cert slot={slot}: {e}",
                        );
                        processor.increment_error();
                    }
                    return;
                }

                log::trace!(
                    "Session {session_short} handle_notarization_reached: caching VoteSignatureSet \
                     for slot={slot} hash={block_hash_short} ({}B)",
                    notar_cert_bytes.len(),
                );
                processor.receiver.cache_notarization_cert(
                    slot.value(),
                    block_hash.clone(),
                    notar_cert_bytes,
                );

                log::trace!(
                    "Session {session_short} handle_notarization_reached: broadcasting notar cert \
                     for slot={slot} ({}B)",
                    cert_bytes.len(),
                );
                // C++ parity (pool.cpp handle_saved_certificate): relay every newly
                // accepted certificate to all validators. Dedup is in SimplexState.
                processor.telemetry.certs_relayed_counter.increment(1);
                processor.receiver.send_certificate(tl_cert);
                // Cache for standstill re-broadcast
                processor.receiver.cache_standstill_certificate(
                    slot.value(),
                    StandstillCertificateType::Notar,
                    cert_bytes,
                );
            },
        );
    }

    fn persist_skip_cert_then_relay(&mut self, event: &SkipCertificateReachedEvent) {
        let skip_store_result =
            if let Some(existing) = self.database.skip_cert_store_result(event.slot) {
                existing.clone()
            } else {
                match self.database.db().save_skip_cert_async(event.slot, &event.certificate) {
                    Ok(result) => {
                        self.database.insert_skip_cert_store(event.slot, result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                            "Session {} handle_skip_certificate_reached: failed to create skip \
                            cert save slot={}: {e}",
                            &self.runtime.description().get_session_id().to_hex_string()[..8],
                            event.slot,
                        );
                        self.telemetry.increment_error();
                        return;
                    }
                }
            };

        // Pre-serialize the relayed + cached cert payload SYNCHRONOUSLY, before posting
        // the persist (see `handle_notarization_reached` for rationale).
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_skip_certificate_reached: failed to convert to TL: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };
        let cert_bytes = match serialize_boxed(&tl_cert) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Session {} handle_skip_certificate_reached: failed to serialize certificate: \
                    {e}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                );
                self.telemetry.increment_error();
                return;
            }
        };

        // Wait-for-store guarantee: certificate must be durable before broadcast/cache.
        // Hand the wait off to the SXMAIN async-DB-results registry; the continuation runs
        // the post-persist work (relay full cert + cache standstill).
        //
        // C++ parity (`pool.cpp::handle_prospective_certificate`): persist gates broadcast.
        let session_short =
            self.runtime.description().get_session_id().to_hex_string()[..8].to_string();
        let slot = event.slot;
        let slot_for_cb = event.slot;
        self.post_async_db_result(
            "handle_skip_certificate_reached",
            skip_store_result,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, res| {
                if let Err(e) = res {
                    if is_storage_result_already_taken(&e) {
                        if processor.database.contains_skip_cert_store(slot_for_cb) {
                            // Redundant FSM re-emission: see `handle_notarization_reached`
                            // callback for the full rationale. Skip broadcast + standstill
                            // cache without bumping the error counter.
                            log::trace!(
                                "Session {session_short} handle_skip_certificate_reached: skip cert \
                                 result already consumed for slot={slot} (redundant FSM re-emit, \
                                 skipping broadcast)",
                            );
                        } else {
                            log::error!(
                                "Session {session_short} handle_skip_certificate_reached: skip cert \
                                 result already consumed but dedup entry is missing for slot={slot}",
                            );
                            processor.increment_error();
                        }
                    } else {
                        processor.database.remove_skip_cert_store(slot_for_cb);
                        log::error!(
                            "Session {session_short} handle_skip_certificate_reached: failed \
                             to store skip cert slot={slot}: {e}",
                        );
                        processor.increment_error();
                    }
                    return;
                }

                log::trace!(
                    "Session {session_short} handle_skip_certificate_reached: broadcasting skip \
                     cert for slot={slot} ({}B)",
                    cert_bytes.len(),
                );
                // Send certificate to all validators.
                processor.telemetry.certs_relayed_counter.increment(1);
                processor.receiver.send_certificate(tl_cert);
                // Cache for standstill re-broadcast.
                processor.receiver.cache_standstill_certificate(
                    slot.value(),
                    StandstillCertificateType::Skip,
                    cert_bytes,
                );
            },
        );
    }

    fn persist_final_cert_then_relay(&mut self, event: &FinalizationReachedEvent) {
        let candidate_id = RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };
        let final_store_result =
            if let Some(existing) = self.database.final_cert_store_result(&candidate_id) {
                existing.clone()
            } else {
                match self.database.db().save_final_cert_async(&candidate_id, &event.certificate) {
                    Ok(result) => {
                        self.database.insert_final_cert_store(candidate_id, result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                            "Session {} handle_finalization_reached: failed to create final cert \
                            save slot={}: {e}",
                            &self.runtime.description().get_session_id().to_hex_string()[..8],
                            event.slot,
                        );
                        self.telemetry.increment_error();
                        return;
                    }
                }
            };

        // Pre-serialize the relayed + cached cert payload SYNCHRONOUSLY, before posting
        // the persist (see `handle_notarization_reached` for rationale).
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_finalization_reached: failed to convert to TL: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };
        let cert_bytes = match serialize_boxed(&tl_cert) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Session {} handle_finalization_reached: failed to serialize certificate: {}",
                    &self.runtime.description().get_session_id().to_hex_string()[..8],
                    e
                );
                self.telemetry.increment_error();
                return;
            }
        };

        // Wait-for-store guarantee: certificate must be durable before broadcast/cache.
        // Hand the wait off to the SXMAIN async-DB-results registry; the continuation runs
        // the post-persist work (relay full cert + cache per-slot standstill + cache last final
        // + update standstill state).
        //
        // C++ parity (`pool.cpp::handle_prospective_certificate` -> `handle_saved_certificate`):
        // persist gates broadcast and the standstill side-effects. `update_standstill_after_final_cert`
        // also runs in the continuation so the standstill timer + tracked-slots range update happen
        // only after the cert is durable, matching the pre-migration ordering.
        let session_short =
            self.runtime.description().get_session_id().to_hex_string()[..8].to_string();
        let slot = event.slot;
        let candidate_id_for_cb =
            RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };
        self.post_async_db_result(
            "handle_finalization_reached",
            final_store_result,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, res| {
                if let Err(e) = res {
                    if is_storage_result_already_taken(&e) {
                        if processor.database.contains_final_cert_store(&candidate_id_for_cb) {
                            // Redundant FSM re-emission: see `handle_notarization_reached`
                            // callback for the full rationale. Skip broadcast +
                            // standstill cache + last-final cache + standstill state
                            // update without bumping the error counter.
                            log::trace!(
                                "Session {session_short} handle_finalization_reached: final cert \
                                 result already consumed for slot={slot} (redundant FSM re-emit, \
                                 skipping broadcast + standstill side-effects)",
                            );
                        } else {
                            log::error!(
                                "Session {session_short} handle_finalization_reached: final cert \
                                 result already consumed but dedup entry is missing for slot={slot}",
                            );
                            processor.increment_error();
                        }
                    } else {
                        processor.database.remove_final_cert_store(&candidate_id_for_cb);
                        log::error!(
                            "Session {session_short} handle_finalization_reached: failed to \
                             store final cert slot={slot}: {e}",
                        );
                        processor.increment_error();
                    }
                    return;
                }

                // C++ parity (pool.cpp handle_saved_certificate): relay every newly
                // accepted certificate to all validators. Dedup is in SimplexState.
                log::trace!(
                    "Session {session_short} handle_finalization_reached: broadcasting final cert \
                     for slot={slot} ({}B)",
                    cert_bytes.len(),
                );
                processor.telemetry.certs_relayed_counter.increment(1);
                processor.receiver.send_certificate(tl_cert);

                // Cache per-slot final certificate (for bundle replay).
                processor.receiver.cache_standstill_certificate(
                    slot.value(),
                    StandstillCertificateType::Final,
                    cert_bytes.clone(),
                );

                // Cache last final certificate (always replayed first on standstill).
                processor.receiver.cache_last_final_certificate(slot.value(), cert_bytes);

                // Update standstill state (timer + tracked slots range).
                processor.update_standstill_after_final_cert(slot);
            },
        );
    }

    fn persist_finalized_block(&mut self, record: FinalizedBlockRecord) -> bool {
        let slot = record.candidate_id.slot;

        if self.runtime.description().get_shard().is_masterchain() {
            // MC path: register the in-flight write with the SXMAIN async-DB-results
            // registry instead of `wait()`-ing here. The continuation logs the
            // outcome on Ok and bumps the error counter on Err; in-memory state
            // is applied by the controller regardless (matches C++ callback-before-persist
            // ordering — see ConsensusController::maybe_apply_finalized_state doc).
            let result = match self.database.db().save_finalized_block_async(&record) {
                Ok(r) => r,
                Err(e) => {
                    log::error!(
                        "Session {} maybe_apply_finalized_state: failed to create finalized \
                         block save for slot={}: {e}",
                        &self.runtime.description().get_session_id().to_hex_string()[..8],
                        slot.value(),
                    );
                    self.telemetry.increment_error();
                    return false;
                }
            };

            let session_short =
                self.runtime.description().get_session_id().to_hex_string()[..8].to_string();
            let slot_v = slot.value();
            self.post_async_db_result(
                "maybe_apply_finalized_state",
                result,
                DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
                move |processor, res| match res {
                    Ok(()) => {
                        log::trace!(
                            "Session {session_short} maybe_apply_finalized_state: stored \
                             finalized block for slot={slot_v}",
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "Session {session_short} maybe_apply_finalized_state: failed to \
                             store finalized block for slot={slot_v}: {e}",
                        );
                        processor.increment_error();
                    }
                },
            );
        } else if let Err(e) = self.database.db().save_finalized_block(&record) {
            log::error!(
                "Session {} maybe_apply_finalized_state: failed to store finalized block for slot={}: {e}",
                &self.runtime.description().get_session_id().to_hex_string()[..8],
                slot.value(),
            );
            self.telemetry.increment_error();
            return false;
        }

        true
    }
}

impl SessionProcessor {
    /* Controller seam (split-borrow) */

    /// Run `f` against `&mut self.consensus` with a freshly built borrowing
    /// [`ConsensusBackend`].
    ///
    /// Splits `&mut self` into the disjoint pieces the finalization pipeline
    /// needs — `&mut self.consensus` plus a fresh [`ConsensusBackendAdapter`]
    /// over the FSM state, candidate book, runtime, cert/finalized DB store,
    /// telemetry, and the SXMAIN mailbox — invokes `f`, then drops the backend
    /// (RAII). The MC applied-top floor + accepted-normal-head cursor are now
    /// controller-owned, so they no longer cross this seam. Mirrors
    /// [`Self::with_validation_backend`] / [`Self::with_collation_backend`]; the
    /// single place that assembles the consensus controller's backend view from
    /// `SessionProcessor`. Used by the `process_simplex_events` finalization/cert
    /// dispatch and the `retry_pending_recursive_finalization` re-entries.
    fn with_consensus_backend<R>(
        &mut self,
        f: impl for<'b> FnOnce(&mut ConsensusController, &'b mut dyn ConsensusBackend) -> R,
    ) -> R {
        let Self {
            consensus,
            simplex_state,
            candidate_book,
            runtime,
            database,
            telemetry,
            task_queue,
            receiver,
            ..
        } = self;
        let mut backend = ConsensusBackendAdapter {
            simplex_state,
            candidate_book: &*candidate_book,
            runtime,
            database,
            telemetry: &**telemetry,
            task_queue: &*task_queue,
            receiver: &*receiver,
        };
        f(consensus, &mut backend)
    }

    /* Inbound votes & certificates (SXMAIN entry points) */

    /// Receiver callback: an incoming vote arrived from the network.
    ///
    /// Called by `ReceiverListenerImpl` when a vote is received. Builds a
    /// [`ConsensusBackend`] split-borrow and delegates the verify / FSM-apply /
    /// fire-and-forget persist work to [`ConsensusController::on_vote`]. Pumps
    /// `check_all()` only when the vote was newly applied (the controller
    /// returns `true`), preserving the pre-refactor behaviour where duplicate /
    /// late / misbehaving / rejected votes do not drive the main loop.
    pub(crate) fn on_vote(&mut self, source_idx: u32, tl_vote: TlVoteBoxed, raw_vote: RawVoteData) {
        //check_execution_time!(30_000); //TODO: LK: restore during performance testing
        let applied = self.with_consensus_backend(|consensus, backend| {
            consensus.on_vote(backend, source_idx, tl_vote, raw_vote)
        });
        if applied {
            // Immediately process the vote (don't wait for next awake)
            self.check_all();
        }
    }

    /// Receiver callback: an incoming certificate arrived from the network.
    ///
    /// Called by `ReceiverListenerImpl` when a certificate is received (C++
    /// nodes broadcast certificates when thresholds are reached). Builds a
    /// [`ConsensusBackend`] split-borrow and delegates source/slot gating,
    /// strict `Certificate::from_tl` verification, the FSM store, and the
    /// standstill-accept / bad-signature-ban notifications to
    /// [`ConsensusController::on_certificate`]. Pumps `check_all()` only when the
    /// certificate passed verification (the controller returns `true`), matching
    /// the pre-refactor control flow.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `tl_certificate` - Deserialized TL certificate object
    pub(crate) fn on_certificate(&mut self, source_idx: u32, tl_certificate: Certificate) {
        let verified = self.with_consensus_backend(|consensus, backend| {
            consensus.on_certificate(backend, source_idx, tl_certificate)
        });
        if verified {
            // Immediately process any state changes
            self.check_all();
        }
    }

    /* FSM event pump, candidate feed & outbound vote */

    /// Process all pending events from the `SimplexState` FSM.
    ///
    /// Pulls events from the FSM queue and dispatches each (see the module-level
    /// "FSM event routing" section): `BroadcastVote` and `SlotSkipped` are handled
    /// here on `SessionProcessor`; the four finalization/cert events route into
    /// `ConsensusController`. Called from `check_all()` after FSM processing.
    ///
    /// Late bodies are not blocked here: a `BlockFinalized` whose candidate body
    /// has not arrived yet is recorded by
    /// `ConsensusController::handle_block_finalized` in `finalized_pending_body`
    /// and materialized later, once the body is repaired (via `request_candidate`
    /// / `receiver.request_candidate()`).
    fn process_simplex_events(&mut self) {
        let mut event_count = 0u64;

        while let Some(event) = self.simplex_state.pull_event() {
            log::trace!("SimplexState::event: {:?}", event);

            match event {
                SimplexEvent::BroadcastVote(vote) => {
                    // Trace: if this is a finalize vote, it means notarize cert was observed
                    if let (Vote::Finalize(ref fv), Some(tc)) = (&vote, &self.trace_collector) {
                        let notarize_vote = Vote::Notarize(crate::simplex_state::NotarizeVote {
                            slot: fv.slot,
                            block_hash: fv.block_hash.clone(),
                        });
                        tc.record_cert_observed(self.session_id(), &notarize_vote);
                    }
                    // Send vote to receiver which will:
                    // 1. Sign it with session-scoped signature
                    // 2. Broadcast to all validators
                    // 3. Process loopback (our own vote submitted via listener for FSM accounting)
                    self.broadcast_vote(vote);
                }
                // Consensus-finalization events route into ConsensusController via
                // the borrowing backend; the pump itself stays on SessionProcessor
                // (the BroadcastVote + SlotSkipped branches below own SP state).
                SimplexEvent::BlockFinalized(e) => {
                    self.with_consensus_backend(|consensus, backend| {
                        consensus.handle_block_finalized(backend, e)
                    });
                }
                SimplexEvent::SlotSkipped(event) => {
                    self.handle_slot_skipped(event.slot);
                }
                SimplexEvent::NotarizationReached(event) => {
                    self.with_consensus_backend(|consensus, backend| {
                        consensus.handle_notarization_reached(backend, event)
                    });
                }
                SimplexEvent::SkipCertificateReached(event) => {
                    self.with_consensus_backend(|consensus, backend| {
                        consensus.handle_skip_certificate_reached(backend, event)
                    });
                }
                SimplexEvent::FinalizationReached(event) => {
                    self.with_consensus_backend(|consensus, backend| {
                        consensus.handle_finalization_reached(backend, event)
                    });
                }
            }

            event_count += 1;
        }

        if event_count > 0 {
            self.telemetry.process_events_counter.increment(event_count);
        }
    }

    // Finalization-driven delivery — the `BlockFinalized` recursive-finalization
    // walk (`finalized_pending_body` trigger tracking, parent-chain evaluation,
    // and emit/apply-vs-defer-on-missing-body decisions) — lives on
    // `ConsensusController::handle_block_finalized`. `SlotSkipped` is handled by
    // `handle_slot_skipped`. Both are dispatched from `process_simplex_events`.

    /// Feed one validated candidate into `SimplexState` and surface FSM rejections.
    ///
    /// Extracted from `process_validated_candidates` (Validation section) so the
    /// same FSM-feed logic runs whether `ensure_candidate_info_stored`'s callback
    /// fires inline (the `candidateInfo` dedup-map result was already done) or from
    /// the registry continuation (still in flight). Callers MUST route through
    /// `ensure_candidate_info_stored` first — that's the only place that guarantees
    /// the candidateInfo is durable in the DB.
    fn feed_validated_candidate_to_fsm(&mut self, candidate: crate::block::Candidate) {
        log::trace!(
            "Session {} process_validated_candidates: feeding candidate to FSM, slot={}",
            self.session_id().to_hex_string(),
            candidate.id.slot,
        );

        if let Err(e) = self.simplex_state.on_candidate(self.runtime.description(), candidate) {
            log::warn!(
                "Session {} process_validated_candidates: FSM rejected candidate: {}",
                self.session_id().to_hex_string(),
                e
            );
        }
    }

    /// Sign a vote with the session-scoped signature and send it via the receiver.
    ///
    /// The receiver broadcasts to all validators and performs loopback (our own
    /// vote is fed back through the listener for FSM accounting). Notarize/Finalize
    /// votes first satisfy the candidateInfo/notarCert durability prerequisite via
    /// `ensure_candidate_info_stored` (Persistence & durability section); Skip
    /// votes have none and go out immediately.
    fn broadcast_vote(&mut self, vote: Vote) {
        log::trace!("Session {} broadcast_vote: {:?}", self.session_id().to_hex_string(), vote);

        match &vote {
            Vote::Notarize(_) => {
                self.telemetry.votes_out_total_counter.increment(1);
                self.telemetry.votes_out_notarize_counter.increment(1);
            }
            Vote::Finalize(_) => {
                self.telemetry.votes_out_total_counter.increment(1);
                self.telemetry.votes_out_finalize_counter.increment(1);
            }
            Vote::Skip(_) => {
                self.telemetry.votes_out_total_counter.increment(1);
                self.telemetry.votes_out_skip_counter.increment(1);
            }
        }

        // WaitCandidateInfoStored parity (C++ consensus.cpp):
        // - before NotarizeVote: wait candidateInfo stored
        // - before FinalizeVote: wait notarCert stored
        // - SkipVote: no durability prerequisite
        //
        // The wait now runs through the SXMAIN async-DB-results registry via
        // `ensure_candidate_info_stored(...)` — inline if the dedup-map result is
        // already done, deferred via `post_async_db_result` if it's still in flight.
        // Either way, the same callback body runs `broadcast_vote_after_persist(vote)`
        // on Ok and the abort path on Err.
        let (id, wait_info, wait_notar) = match &vote {
            Vote::Notarize(v) => {
                (RawCandidateId { slot: v.slot, hash: v.block_hash.clone() }, true, false)
            }
            Vote::Finalize(v) => {
                (RawCandidateId { slot: v.slot, hash: v.block_hash.clone() }, false, true)
            }
            Vote::Skip(_) => {
                // Skip votes have no durability prerequisite; broadcast immediately
                // (re-enter the controller's outbound path within the borrow scope).
                self.with_consensus_backend(|consensus, backend| {
                    consensus.broadcast_vote_after_persist(backend, vote)
                });
                return;
            }
        };

        let session_short = self.session_id().to_hex_string()[..8].to_string();
        self.ensure_candidate_info_stored(&id, wait_info, wait_notar, move |processor, res| {
            match res {
                Ok(()) => {
                    // Durability confirmed — re-enter the controller's outbound
                    // path through a fresh `with_consensus_backend` borrow.
                    processor.with_consensus_backend(|consensus, backend| {
                        consensus.broadcast_vote_after_persist(backend, vote)
                    });
                }
                Err(e) => {
                    log::error!(
                        "Session {session_short} broadcast_vote: aborting vote send due to \
                         durability wait failure: {e} ({:?})",
                        vote,
                    );
                    processor.telemetry.votes_out_persist_fail_counter.increment(1);
                    processor.increment_error();
                }
            }
        });
    }

    /* Notarization-certificate ingress */

    /// Handle notar-only progress from requestCandidate repair path.
    ///
    /// This callback is used when receiver-side merge logic obtains notarization
    /// signatures before candidate body completeness. It allows the processor/FSM to
    /// ingest the certificate immediately and unblock parent-gated validations.
    pub(crate) fn on_candidate_notar_received(
        &mut self,
        source_idx: u32,
        slot: SlotIndex,
        block_hash: UInt256,
        notar_cert: Vec<u8>,
    ) {
        if !self.is_valid_source(ValidatorIndex::new(source_idx)) {
            log::warn!(
                "Session {} on_candidate_notar_received: invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.runtime.description().get_total_nodes(),
            );
            return;
        }

        self.process_received_notar_cert(slot, &block_hash, &notar_cert);
        self.check_all();
    }

    /// Process notarization certificate received from query response
    ///
    /// Deserializes, verifies, and stores the certificate in SimplexState.
    ///
    /// Parse VoteSignatureSet bytes (not full Certificate) to match C++ wire format.
    /// C++ `candidateAndCert.notar` contains serialized `voteSignatureSet`, not `certificate`.
    /// Reference: C++ candidate-resolver.cpp from_tl():
    ///   TRY_RESULT(signatures, fetch_tl_object<tl::voteSignatureSet>(entry.notar_, true));
    ///   TRY_RESULT_ASSIGN(result.notar_cert, NotarCert::from_tl(std::move(*signatures), vote, bus));
    fn process_received_notar_cert(
        &mut self,
        slot: SlotIndex,
        block_hash: &UInt256,
        notar_cert_bytes: &[u8],
    ) {
        log::trace!(
            "Session {} process_received_notar_cert: slot={} hash={} bytes={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &block_hash.to_hex_string()[..8],
            notar_cert_bytes.len()
        );

        // Deserialize VoteSignatureSet (C++ wire format for candidateAndCert.notar)
        let tl_sigs = match deserialize_boxed(notar_cert_bytes) {
            Ok(msg) => match msg.downcast::<VoteSignatureSetBoxed>() {
                Ok(sigs) => sigs,
                Err(_) => {
                    log::warn!(
                        "Session {} process_received_notar_cert: unexpected type, expected \
                        VoteSignatureSet for slot={slot} hash={}",
                        &self.session_id().to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }
            },
            Err(e) => {
                log::warn!(
                    "Session {} process_received_notar_cert: failed to deserialize \
                    VoteSignatureSet for slot={slot} hash={}: {e}",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                );
                return;
            }
        };

        // Verify certificate using from_tl_signatures (matches C++ NotarCert::from_tl)
        match self.verify_notar_cert_from_vote_signature_set(slot, block_hash, &tl_sigs) {
            Ok(notar_cert_ptr) => {
                log::trace!(
                    "Session {} process_received_notar_cert: verified notar cert for slot={slot} \
                    hash={} with {} sigs",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                    notar_cert_ptr.signatures.len(),
                );

                // Ensure cert is persisted before updating FSM state.
                let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
                if !self.database.contains_notar_cert_store(&candidate_id) {
                    match self
                        .database
                        .db()
                        .save_notar_cert_async(&candidate_id, notar_cert_ptr.as_ref())
                    {
                        Ok(result) => {
                            self.database.insert_notar_cert_store(candidate_id.clone(), result);
                        }
                        Err(e) => {
                            log::error!(
                                "Session {} process_received_notar_cert: failed to create \
                                notar_cert save slot={slot}: {e}",
                                &self.session_id().to_hex_string()[..8],
                            );
                            self.increment_error();
                            return;
                        }
                    }
                }

                let session_short = self.session_id().to_hex_string()[..8].to_string();
                let block_hash_short = block_hash.to_hex_string()[..8].to_string();
                let block_hash_owned = block_hash.clone();
                let notar_cert_for_cb = notar_cert_ptr.clone();
                self.ensure_candidate_info_stored(
                    &candidate_id,
                    false,
                    true,
                    move |processor, res| match res {
                        Ok(()) => {
                            processor.feed_notar_cert_to_fsm(
                                slot,
                                &block_hash_owned,
                                notar_cert_for_cb,
                            );
                            // Newly ingested NotarCert can unblock deferred
                            // recursive-finalization chains where ancestors require
                            // notar-signature callback mode. Pre-migration this call
                            // ran from the function tail; here we run it from the
                            // callback so it observes the cert in the FSM (matters
                            // when the persist had to defer through the registry).
                            processor.with_consensus_backend(|consensus, backend| {
                                consensus.retry_pending_recursive_finalization(backend)
                            });
                        }
                        Err(e) => {
                            log::warn!(
                                "Session {session_short} process_received_notar_cert: skipping \
                                 FSM feed because notar cert is not durable for \
                                 s{}:{block_hash_short}: {e}",
                                slot.value(),
                            );
                        }
                    },
                );
            }
            Err(e) => {
                log::warn!(
                    "Session {} process_received_notar_cert: invalid notar cert for slot={slot} \
                    hash={}: {e}",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                );
                // Verify-Err path: original code unconditionally ran the recursive
                // retry at the function tail regardless of cert validity. Preserve
                // that on this branch only — verify-Ok is fully owned by the
                // callback above (which runs retry on Ok and skips it on Err).
                self.with_consensus_backend(|consensus, backend| {
                    consensus.retry_pending_recursive_finalization(backend)
                });
            }
        }
    }

    /// Store a verified notar cert into `SimplexState` and surface FSM conflicts.
    ///
    /// Extracted from `process_received_notar_cert` so the same FSM-ingestion logic
    /// runs whether `ensure_candidate_info_stored`'s callback fires inline (the
    /// dedup-map result was already done) or from the registry continuation (the
    /// result was still in flight). Callers MUST route through
    /// `ensure_candidate_info_stored` first — that's the only place that
    /// guarantees the notar cert is durable in the DB.
    fn feed_notar_cert_to_fsm(
        &mut self,
        slot: SlotIndex,
        block_hash: &UInt256,
        notar_cert_ptr: crate::certificate::NotarCertPtr,
    ) {
        let store_result = self.simplex_state.set_notarize_certificate(
            self.runtime.description(),
            slot,
            block_hash,
            notar_cert_ptr,
        );
        match store_result {
            Ok(true) => {
                // Cert accepted into FSM; DB persistence guaranteed by caller.
            }
            Ok(false) => {
                // Already stored for the same block — idempotent.
            }
            Err(e) => {
                log::warn!(
                    "Session {} process_received_notar_cert: notar cert conflict slot={slot} \
                     hash={}: {e}",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                );
            }
        }
    }

    /// Verify notarization certificate from VoteSignatureSet (C++ wire format)
    ///
    /// Parse VoteSignatureSet and verify signatures.
    /// Reference: C++ NotarCert::from_tl(voteSignatureSet&&, vote, bus)
    fn verify_notar_cert_from_vote_signature_set(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
        tl_sigs: &VoteSignatureSetBoxed,
    ) -> Result<crate::certificate::NotarCertPtr> {
        // Build the vote being certified
        let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };

        // Verify signatures and build certificate
        let cert = crate::certificate::NotarCert::from_tl_signatures(
            tl_sigs,
            vote,
            self.runtime.description(),
            &self.session_id(),
        )?;

        Ok(Arc::new(cert))
    }

    /* Activity & standstill */

    /// Handle activity update from the receiver
    ///
    /// Called periodically by ReceiverListenerImpl with active weight and per-validator activity times.
    pub(crate) fn on_activity(
        &mut self,
        active_weight: ValidatorWeight,
        last_activity: Vec<Option<SystemTime>>,
        snapshot: crate::receiver::ReceiverActivitySnapshot,
    ) {
        let prev = self.runtime.active_weight();
        let changed = self.runtime.record_activity(active_weight, last_activity);
        if changed {
            log::debug!(
                "Session {} on_activity: active_weight {} -> {}",
                self.session_id().to_hex_string(),
                prev,
                active_weight
            );
            self.telemetry.active_weight_gauge.set(active_weight as f64);
        }
        self.telemetry.set_receiver_snapshot(snapshot);
    }

    pub(crate) fn on_standstill_trigger(&mut self, notification: StandstillTriggerNotification) {
        log::warn!("{}", self.build_standstill_trigger_log(&notification));
    }

    fn build_standstill_trigger_log(&self, notification: &StandstillTriggerNotification) -> String {
        let mut result = format!(
            "Session {}: Standstill detected, re-broadcasting \
            {} certs + {} votes (range [{}, {})). Current pool state:\n",
            &self.session_id().to_hex_string()[..8],
            notification.cert_count,
            notification.vote_count,
            notification.begin,
            notification.end,
        );
        result.push_str(&self.simplex_state.standstill_diagnostic_dump(self.runtime.description()));
        result
    }

    fn sync_standstill_slots_from_state(&self) {
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        // Keep receiver ingress progress within tracked interval bounds.
        // This mirrors C++ behavior where `now_` never stays behind finalized frontier.
        let progress = self.simplex_state.get_first_non_progressed_slot().value().max(begin);
        self.receiver.set_ingress_slot_begin(begin);
        self.receiver.set_ingress_progress_slot(progress);
        self.receiver.set_standstill_slots(begin, end);
    }

    /// Update standstill state after storing a finalization certificate
    ///
    /// Called when a final certificate is stored (local or foreign).
    /// Reschedules standstill timer and updates tracked slots range.
    ///
    /// Reference: C++ handle_certificate(FinalCertRef) calls reschedule_standstill_resolution()
    /// and updates first_nonfinalized_slot_ which affects tracked_slots_interval()
    fn update_standstill_after_final_cert(&self, slot: SlotIndex) {
        // Reschedule standstill timer
        self.receiver.reschedule_standstill();

        // Update standstill tracked slots range
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        self.sync_standstill_slots_from_state();

        log::trace!(
            "Session {} update_standstill_after_final_cert: slot={} tracked_slots=[{}, {})",
            &self.session_id().to_hex_string()[..8],
            slot,
            begin,
            end
        );
    }

    /* Slot-skip & MC-applied-top */

    /// Handle a `SlotSkipped` FSM event: the FSM has decided finalization is no
    /// longer possible for `slot` (a skip certificate formed). Dispatched directly
    /// from [`Self::process_simplex_events`].
    ///
    /// Stays on `SessionProcessor` rather than `ConsensusController` because it is
    /// cross-subsystem cleanup, not finalization-journal logic: it records skip
    /// telemetry, resets per-slot state, cancels in-flight candidate repairs,
    /// re-syncs receiver standstill slots, and prunes stale-window precollations.
    fn handle_slot_skipped(&mut self, slot: SlotIndex) {
        check_execution_time!(10_000);
        instrument!();

        self.telemetry.skip_total_counter.increment(1);

        log::debug!(
            "Session {} SKIP: slot={} (no ValidatorGroup callback in roundless mode)",
            &self.session_id().to_hex_string()[..8],
            slot
        );

        // Record slot duration metric
        let now = self.now();
        if let Ok(duration) = now.duration_since(self.runtime.started_at(slot, now)) {
            self.telemetry.slot_duration_histogram.record(duration.as_millis() as f64);
        }

        // FSM already updated first_non_finalized_slot and cleaned up internally
        // Reset per-slot state for this slot
        self.reset_slot_state(slot);
        self.cancel_candidate_repairs_for_slot(slot);

        // Update standstill tracked slots range (but DO NOT reschedule standstill on skip)
        // Reference: C++ pool.cpp on_skip() does NOT call reschedule_standstill_resolution()
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        self.sync_standstill_slots_from_state();

        // C++ parity: do NOT remove precollated blocks for same-window slots.
        // C++ block-producer.cpp only cancels collations at window transitions
        // (via CancellationTokenSource replacement), not on per-slot skip events.
        // Removing the entry here destroys the locked parent context that a
        // late same-window callback needs to publish the candidate.
        let slot_window = self.runtime.description().get_window_idx(slot);
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if slot_window != current_window {
            let now = self.now();
            self.telemetry.forget_self_collation_tracking(
                slot,
                "skipped_window_mismatch",
                self.runtime.description(),
                now,
            );
            self.remove_precollated_block(slot);
        }

        log::trace!(
            "Session {} handle_slot_skipped: completed slot={} tracked_slots=[{}, {})",
            &self.session_id().to_hex_string()[..8],
            slot,
            begin,
            end
        );
    }

    /// Update applied-top tracking from a manager notification (delegates to
    /// [`ConsensusController::set_mc_finalized_block`]).
    ///
    /// Called when the manager forwards the current applied top for this session
    /// shard (see `session.rs`). The seqno feeds `should_generate_empty_block()`
    /// and the exact block id seeds the accepted-head cursor when known.
    pub(crate) fn set_mc_finalized_block(&mut self, applied_top: BlockIdExt) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.set_mc_finalized_block(backend, applied_top)
        });
    }
}

// ======================================================================
// Diagnostics & telemetry
// ======================================================================
//
// Consensus-state logging, health-check / stall dumps, the snapshot builders that
// project live state for the dumps, and the generated-candidate telemetry recorders.

impl SessionProcessor {
    /* Health checks & stall dumps (entry points) */

    /// Public health check dump for periodic monitoring.
    ///
    /// Called from session main loop for periodic health checks.
    /// Runs anomaly detection and logs brief health status.
    pub(crate) fn health_check_dump(&mut self) {
        let status = self.build_dump_status_snapshot();
        self.telemetry.health_check_dump(self.runtime.description(), &status);
        if self.telemetry.should_build_full_dump(false) {
            let full = self.build_full_dump_snapshot(status.observed_at, false);
            self.telemetry.debug_dump_full(self.runtime.description(), &status, &full, false);
        }
        self.run_health_checks();
    }

    /// Run anomaly detection checks. Delegates to the telemetry aspect.
    ///
    /// Each check emits a single-line WARN or ERROR log with the
    /// `SIMPLEX_HEALTH` prefix and increments `simplex_health_warnings`
    /// (never `session_errors_count`, except the skip-vote-dominance error
    /// path which bumps the session error counter once).
    fn run_health_checks(&mut self) {
        let snapshot = self.build_health_check_snapshot();
        self.telemetry.run_health_checks(self.runtime.description(), &snapshot);
    }

    /// Produce detailed debug dump of session state.
    ///
    /// Includes:
    /// - Stall conclusion with health findings
    /// - Session header, shard info, frontiers with cursor ages
    /// - Consensus milestone timestamps (finalization, notarization, cert times)
    /// - Heads (finalized, accepted, MC applied)
    /// - Candidate funnel statistics with percentages
    /// - Collation state with per-window grouping and leader identity
    /// - Validation inventory with lifecycle buckets
    /// - Peer diagnostics with typed vote/cert/candidate stats
    /// - Standstill diagnostic grid (on stall)
    ///
    /// # Arguments
    /// * `is_stalled` - If true, consensus is stalled (no finalizations for ROUND_DEBUG_PERIOD).
    ///   In stall mode, full details are logged to INFO level for immediate visibility.
    ///   In normal mode (health check), brief status goes to INFO, full details to DEBUG.
    fn debug_dump(&mut self, is_stalled: bool) {
        instrument!();
        let status = self.build_dump_status_snapshot();
        self.telemetry.log_dump_status(self.runtime.description(), &status, is_stalled);
        if !self.telemetry.should_build_full_dump(is_stalled) {
            return;
        }
        let full = self.build_full_dump_snapshot(status.observed_at, is_stalled);
        self.telemetry.debug_dump_full(self.runtime.description(), &status, &full, is_stalled);
    }

    /// Log current consensus state for debugging.
    ///
    /// Builds a [`ConsensusStateSnapshot`] from `SessionProcessor`
    /// private state and delegates the actual formatting / emission to
    /// [`SessionTelemetry::log_consensus_state`]. All callers pass a
    /// `&'static str` trigger so snapshot construction is
    /// allocation-free. Used after incoming/outgoing messages for
    /// observability.
    fn log_consensus_state(&self, trigger: &'static str) {
        let snapshot = self.build_consensus_state_snapshot(trigger);
        self.telemetry.log_consensus_state(self.runtime.description(), &snapshot);
    }

    /* Generated-candidate validation telemetry */

    fn note_generated_candidate_validation_missed(
        &mut self,
        candidate_id: &RawCandidateId,
        reason: impl Into<String>,
    ) {
        self.telemetry.note_generated_candidate_validation_missed(
            candidate_id,
            reason,
            self.runtime.description(),
            self.now(),
        );
    }

    fn note_generated_candidate_validation_missed_for_slot(
        &mut self,
        slot: SlotIndex,
        reason: impl Into<String>,
    ) {
        self.telemetry.note_generated_candidate_validation_missed_for_slot(
            slot,
            reason,
            self.runtime.description(),
            self.now(),
        );
    }

    /* Snapshot builders */

    /// Build the expensive `FullDumpSnapshot` used by `debug_dump_full`.
    ///
    /// Must only be called after
    /// [`SessionTelemetry::should_build_full_dump`] returns true. Walks
    /// candidate inventory, window diagnostics, and (on stall) the
    /// standstill diagnostic dump. `now` should be reused from the cheap
    /// status snapshot so the two samples align.
    fn build_full_dump_snapshot(&self, now: SystemTime, is_stalled: bool) -> FullDumpSnapshot {
        let first_non_progressed = self.simplex_state.get_first_non_progressed_slot();
        let standstill_diagnostic_dump = if is_stalled {
            let raw = self.simplex_state.standstill_diagnostic_dump(self.runtime.description());
            if raw.is_empty() {
                None
            } else {
                Some(raw)
            }
        } else {
            None
        };

        FullDumpSnapshot {
            finalized_head_slot: self.consensus.finalized_head_slot(),
            finalized_head_block_id: self.consensus.finalized_head_block_id().clone(),
            last_consensus_finalized_seqno: self.consensus.last_consensus_finalized_seqno(),
            accepted_normal_head_seqno: self.consensus.accepted_normal_head_seqno(),
            accepted_normal_head_block_id: self.consensus.accepted_normal_head_block_id().clone(),
            last_mc_finalized_seqno: self.consensus.last_mc_finalized_seqno(),
            pending_validations_count: self.validation.pending_validation_count(),
            approved_count: self.validation.approved_count(),
            rejected_count: self.validation.rejected_count(),
            finalized_pending_body_count: self.consensus.finalized_pending_body_len(),
            current_slot: first_non_progressed,
            current_slot_pending_generate: self.runtime.is_pending_generate(first_non_progressed),
            current_slot_generated: self.runtime.is_generated(first_non_progressed),
            current_slot_sent_generated: self.runtime.is_sent_generated(first_non_progressed),
            precollated_count: self.collation.precollated_count(),
            generated_waiting_validation_count: self.telemetry.waiting_validation_count(),
            validation_inventory: self.build_validation_inventory_snapshot(now),
            window_diagnostics: self
                .simplex_state
                .collect_window_diagnostics(self.runtime.description()),
            standstill_diagnostic_dump,
            health_snapshot: self.build_health_check_snapshot(),
        }
    }

    /// Build a cheap `DumpStatusSnapshot` for status / stall logging.
    ///
    /// Always safe to call: scalar reads only, no map traversal. Pass the
    /// returned `observed_at` into [`Self::build_full_dump_snapshot`] so the
    /// cheap and expensive samples share one timestamp.
    fn build_dump_status_snapshot(&self) -> DumpStatusSnapshot {
        let now = self.now();
        let first_non_progressed = self.simplex_state.get_first_non_progressed_slot();
        let slot_duration_secs = now
            .duration_since(self.runtime.started_at(first_non_progressed, now))
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        DumpStatusSnapshot {
            observed_at: now,
            active_weight: self.runtime.active_weight(),
            slot_duration_secs,
            first_non_finalized: self.simplex_state.get_first_non_finalized_slot(),
            first_non_progressed,
            finalized_head_seqno: self.consensus.finalized_head_seqno(),
        }
    }

    /// Build a `ValidationInventorySnapshot` for
    /// [`SessionTelemetry::format_validation_inventory`] / debug-dump.
    ///
    /// Expensive: walks every received candidate. Only built when the
    /// telemetry aspect has decided a full dump should be emitted.
    /// `now` is taken as an argument so the cheap status line and the full
    /// dump share one sampling timestamp.
    fn build_validation_inventory_snapshot(&self, now: SystemTime) -> ValidationInventorySnapshot {
        let totals = self.compute_candidate_totals(now);
        let recent_finalized_by_time = now
            .duration_since(self.telemetry.last_finalization_time())
            .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
            .unwrap_or(false);

        let mut received = Vec::new();
        let mut validated = Vec::new();
        let mut notarized = Vec::new();
        let mut finalized = Vec::new();

        for (id, rc) in self.candidate_book.iter_received() {
            let is_finalized = self.consensus.is_finalized_block(id);
            let is_notarized = self.simplex_state.has_notarized_block(id.slot);
            let is_approved = self.validation.approved_contains(id);
            let is_pending = self.validation.pending_validation_contains(id);
            let is_rejected = self.validation.is_rejected(id);

            let entry = ValidationInventoryEntry {
                slot: rc.slot,
                source_idx: rc.source_idx,
                candidate_hash: id.hash.clone(),
                block_id: rc.block_id.clone(),
                received_at: rc.receive_time,
                is_pending,
                is_approved,
                is_rejected,
                is_notarized,
                is_finalized,
                is_empty: rc.is_empty,
            };

            if is_finalized {
                // Match the existing `dump_validation_inventory` policy:
                // only show finalized rows within the recent window.
                if recent_finalized_by_time {
                    finalized.push(entry);
                }
            } else if is_notarized {
                notarized.push(entry);
            } else if is_approved || (!is_pending && !is_rejected) {
                validated.push(entry);
            } else {
                received.push(entry);
            }
        }

        ValidationInventorySnapshot { totals, received, validated, notarized, finalized }
    }

    /// Build a slim `HealthCheckSnapshot` for [`SessionTelemetry::run_health_checks`].
    ///
    /// Cheap; safe to call on every health tick. Description-derived values
    /// (session id, total nodes / weight, thresholds, session age) are
    /// intentionally absent because `SessionTelemetry` receives
    /// `&SessionDescription` and reads them there.
    fn build_health_check_snapshot(&self) -> HealthCheckSnapshot {
        let peers_never_seen = self
            .runtime
            .last_activity()
            .iter()
            .enumerate()
            .filter(|(i, ts)| {
                *i != self.runtime.description().get_self_idx().0 as usize && ts.is_none()
            })
            .count();
        HealthCheckSnapshot {
            active_weight: self.runtime.active_weight(),
            first_non_finalized: self.simplex_state.get_first_non_finalized_slot(),
            first_non_progressed: self.simplex_state.get_first_non_progressed_slot(),
            finalized_head_slot: self.consensus.finalized_head_slot(),
            peers_never_seen,
        }
    }

    /// Build a `ConsensusStateSnapshot` for [`SessionTelemetry::log_consensus_state`].
    ///
    /// `trigger` is a `&'static str` (every existing call site passes a
    /// literal) so snapshot construction stays allocation-free.
    fn build_consensus_state_snapshot(&self, trigger: &'static str) -> ConsensusStateSnapshot {
        let first_non_finalized = self.simplex_state.get_first_non_finalized_slot();
        let first_non_progressed = self.simplex_state.get_first_non_progressed_slot();
        ConsensusStateSnapshot {
            trigger,
            first_non_finalized,
            first_non_progressed,
            generated: self.runtime.is_generated(first_non_progressed),
            pending_generate: self.runtime.is_pending_generate(first_non_progressed),
            pending_validations_count: self.validation.pending_validation_count(),
            validated_count: self.validation.validated_count(),
            has_notarized: self.simplex_state.has_notarized_block(first_non_finalized),
            is_finalized: self.simplex_state.is_slot_finalized(first_non_finalized),
        }
    }

    /// Compute candidate funnel totals for validation inventory dump.
    fn compute_candidate_totals(&self, now: SystemTime) -> CandidateTotals {
        let received_total = self.candidate_book.received_count();
        let mut received_unvalidated = 0usize;
        let mut validated_not_notarized = 0usize;
        let mut notarized_not_finalized = 0usize;
        let mut finalized_recent = 0usize;
        let mut other_omitted = 0usize;

        for (id, _rc) in self.candidate_book.iter_received() {
            let is_finalized = self.consensus.is_finalized_block(id);
            let is_notarized = self
                .simplex_state
                .get_notarized_block_hash(self.runtime.description(), id.slot)
                .as_ref()
                == Some(&id.hash);
            let is_approved = self.validation.approved_contains(id);
            let is_pending = self.validation.pending_validation_contains(id);

            if is_finalized {
                let is_recent = self.consensus.finalized_pending_finalized_at(id).map_or_else(
                    || {
                        // Already materialized: check receive time as proxy
                        false
                    },
                    |finalized_at| {
                        now.duration_since(finalized_at)
                            .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
                            .unwrap_or(false)
                    },
                );
                // Also check if it was recently finalized by checking last_finalization_time proximity
                let recent_by_time = now
                    .duration_since(self.telemetry.last_finalization_time())
                    .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
                    .unwrap_or(false);
                if is_recent || recent_by_time {
                    finalized_recent += 1;
                } else {
                    other_omitted += 1;
                }
            } else if is_notarized {
                notarized_not_finalized += 1;
            } else if is_approved || (!is_pending && !self.validation.is_rejected(id)) {
                validated_not_notarized += 1;
            } else {
                received_unvalidated += 1;
            }
        }

        CandidateTotals {
            received_total,
            received_unvalidated,
            validated_not_notarized,
            notarized_not_finalized,
            finalized_recent,
            other_omitted,
        }
    }

    // Snapshot builders for `SessionTelemetry`.
    //
    // These are the only observability-related methods that read private
    // `SessionProcessor` maps. They produce neutral, owned snapshot types that
    // telemetry methods consume read-only via parameter — never via a
    // back-reference to `SessionProcessor`.
}

// ======================================================================
// Persistence & durability
// ======================================================================
//
// SXMAIN-side persistence orchestration: pool-state publish on leader-window
// boundaries and the candidate-info / notar-cert durability waits.

impl SessionProcessor {
    /* Pool-state publication (leader-window boundary) */

    /// Persist pool state (`first_nonannounced_window`) when leader window advances.
    ///
    /// C++ reference: `SimplexPoolImpl::maybe_publish_new_leader_window()`:
    /// - computes `new_window = now_ / slots_per_leader_window_`
    /// - if `new_window >= first_nonannounced_window_` then
    ///   sets `first_nonannounced_window_ = new_window + 1` and `co_await store_pool_state_to_db()`
    fn maybe_store_pool_state(&mut self) {
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if current_window < self.database.first_nonannounced_window() {
            log::trace!(
                "Session {} maybe_store_pool_state: no-op (current_window={current_window}, \
                first_nonannounced_window={})",
                &self.session_id().to_hex_string()[..8],
                self.database.first_nonannounced_window(),
            );
            return;
        }

        log::trace!(
            "Session {} maybe_store_pool_state: window advanced (current_window={current_window}, \
            first_nonannounced_window={}), storing",
            &self.session_id().to_hex_string()[..8],
            self.database.first_nonannounced_window(),
        );

        // Advance the in-memory cursor first (matches C++ ordering: state advances
        // before `co_await store_pool_state_to_db()`). Subsequent calls will exit
        // early via the no-op guard above until the FSM crosses the next window.
        let new_window = current_window + 1;
        self.database.set_first_nonannounced_window(new_window);

        let record = PoolStateRecord { first_nonannounced_window: new_window };
        let result = match self.database.db().save_pool_state_async(&record) {
            Ok(r) => r,
            Err(e) => {
                log::error!(
                    "Session {} maybe_store_pool_state: failed to create pool_state save ({}): {}",
                    &self.session_id().to_hex_string()[..8],
                    new_window,
                    e
                );
                self.increment_error();
                return;
            }
        };

        // C++ `co_await`s this write; the registry runs the continuation on
        // SXMAIN once RocksDB confirms (or on timeout). The in-memory cursor
        // is already advanced, so a slow disk no longer parks SXMAIN here.
        let session_short = self.session_id().to_hex_string()[..8].to_string();
        self.post_async_db_result(
            "maybe_store_pool_state",
            result,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, res| match res {
                Ok(()) => {
                    log::trace!(
                        "Session {session_short} maybe_store_pool_state: stored pool_state \
                        (first_nonannounced_window={new_window})",
                    );
                }
                Err(e) => {
                    log::error!(
                        "Session {session_short} maybe_store_pool_state: failed to store \
                        pool_state ({new_window}): {e}",
                    );
                    processor.increment_error();
                }
            },
        );
    }

    /* Candidate-info / notar-cert durability waits */

    /// Ensure CandidateResolver-related DB writes (candidateInfo / notarCert) are
    /// durable, then invoke `on_complete` exactly once with `Ok(())` on success
    /// or `Err(...)` on failure (missing dedup-map entry / storage error).
    ///
    /// Continuation-style replacement for the legacy blocking
    /// `wait_candidate_info_stored(...) -> bool`. Behaves uniformly across the
    /// three completion paths:
    ///
    /// * **Ready inline** — the dedup-map result is already done; the callback
    ///   runs synchronously before this method returns.
    /// * **Pending** — the result is still in flight; the wait is handed to the
    ///   SXMAIN async-DB-results registry via `post_async_db_result(...)` and
    ///   the callback runs when the registry drains the entry.
    /// * **Missing dedup-map entry / storage error** — the callback runs
    ///   immediately with `Err(...)` and `increment_error()` has already been
    ///   bumped by this method.
    ///
    /// "result already taken" (the storage layer's single-take sentinel) is
    /// treated as `Ok(())` because a prior caller already observed the real
    /// outcome; the persist itself succeeded (otherwise that prior caller would
    /// have routed through the `Err` branch and bumped the error counter).
    ///
    /// C++ parity (`validator/consensus/simplex/consensus.cpp::try_notarize` +
    /// `candidate-resolver.cpp::store_candidate`): the consensus actor `co_await`s
    /// `StoreCandidate` and only then submits the vote / proceeds with FSM
    /// ingestion. The callback body here corresponds to the code that runs
    /// *after* the C++ `co_await`, regardless of whether the persist resolved
    /// inline or required a real wait — same uniform semantic, same caller body
    /// either way.
    ///
    /// # Single-wait callsite assumption
    ///
    /// In practice every caller asks for either `wait_candidate_info` xor
    /// `wait_notar_cert` (never both) — the dual-wait combination has no current
    /// caller and is `debug_assert!`ed against to surface any future misuse.
    /// Add chained/parallel handling here (and a test) before lifting the assert.
    fn ensure_candidate_info_stored<F>(
        &mut self,
        id: &RawCandidateId,
        wait_candidate_info: bool,
        wait_notar_cert: bool,
        on_complete: F,
    ) where
        F: FnOnce(&mut SessionProcessor, Result<()>) + Send + 'static,
    {
        // Fast-path the no-op case before any hex formatting: callers may invoke
        // with neither prerequisite requested (so they don't have to special-case
        // "no wait" themselves), and that path neither logs nor needs the id
        // prefixes — returning here keeps it allocation-free.
        if !wait_candidate_info && !wait_notar_cert {
            on_complete(self, Ok(()));
            return;
        }

        // Single allocation per id: keep the `to_hex_string()` buffer and
        // truncate it in place instead of re-allocating via
        // `to_hex_string()[..8].to_string()`. `String::truncate` is O(1) on
        // an ASCII-only hex string (every byte is a single UTF-8 scalar),
        // so this just clips the length field. The buffer keeps its
        // original capacity, but no extra heap allocation is needed.
        let mut session_short = self.session_id().to_hex_string();
        session_short.truncate(8);
        let mut block_hash_short = id.hash.to_hex_string();
        block_hash_short.truncate(8);
        let slot_v = id.slot.value();

        // Hard-fail at runtime on the unsupported dual-wait combination so a
        // future caller cannot silently weaken durability under release
        // builds. `debug_assert!` would have masked this in production. The
        // legacy (pre-callback) `wait_candidate_info_stored` did support both
        // simultaneously; if a real caller ever needs that again, implement
        // the combined wait path here (and add a regression test) before
        // lifting this guard.
        if wait_candidate_info && wait_notar_cert {
            log::error!(
                "Session {session_short} EnsureCandidateInfoStored: combined wait \
                 (candidateInfo+notarCert) is not supported for s{slot_v}:{block_hash_short}",
            );
            self.increment_error();
            debug_assert!(
                false,
                "ensure_candidate_info_stored: combined wait unsupported (no current caller \
                 needs both candidateInfo and notarCert in one invocation)",
            );
            on_complete(
                self,
                Err(error!(
                    "ensure_candidate_info_stored: combined wait unsupported for \
                     s{slot_v}:{block_hash_short}"
                )),
            );
            return;
        }

        log::trace!(
            "Session {session_short} EnsureCandidateInfoStored: poll s{slot_v}:{block_hash_short} \
             info={wait_candidate_info} notar={wait_notar_cert}",
        );

        // Pick the wait kind from the caller's request. Logic (dedup-map
        // lookup, registry op label) is dispatched on this enum; the kind's
        // `name()` is used only for log message formatting.
        // Exactly one prerequisite is set here: the no-op (neither) and the
        // unsupported dual-wait (both) cases already returned above.
        let kind = if wait_candidate_info {
            DurabilityWaitKind::CandidateInfo
        } else {
            DurabilityWaitKind::NotarCert
        };

        let res = match kind.dedup_lookup(self, id) {
            Some(r) => r,
            None => {
                let label = kind.name();
                log::error!(
                    "Session {session_short} EnsureCandidateInfoStored: missing {label} store \
                     result for s{slot_v}:{block_hash_short}",
                );
                self.increment_error();
                let err = error!("missing {label} store result for s{slot_v}:{block_hash_short}");
                on_complete(self, Err(err));
                return;
            }
        };

        if res.is_ready() {
            // Result already done — consume + classify + dispatch synchronously.
            // After this `try_get`, any subsequent observer (e.g., a stale registry
            // entry from a prior call) can observe the "result already taken"
            // sentinel and is classified via dedup-map state below.
            let outcome = self.database.classify_durability_wait_outcome(
                res.try_get(),
                kind,
                id,
                &session_short,
                slot_v,
                &block_hash_short,
                /* deferred */ false,
                &self.telemetry,
            );
            on_complete(self, outcome);
            return;
        }

        // Defer to the SXMAIN async-DB-results registry; the continuation classifies
        // the actual storage outcome and invokes `on_complete` from there.
        log::trace!(
            "Session {session_short} EnsureCandidateInfoStored: {} pending for \
             s{slot_v}:{block_hash_short}, deferring callback",
            kind.name(),
        );
        let session_short_cb = session_short;
        let block_hash_short_cb = block_hash_short;
        let id_cb = id.clone();
        self.post_async_db_result(
            kind.op_label(),
            res,
            DEFAULT_ASYNC_DB_WRITE_TIMEOUT,
            move |processor, registry_res| {
                // Convert registry's `Result<()>` (raw storage outcome) into the
                // post-classification `Result<()>` we hand to the caller.
                let outcome = processor.database.classify_durability_wait_outcome(
                    Some(registry_res),
                    kind,
                    &id_cb,
                    &session_short_cb,
                    slot_v,
                    &block_hash_short_cb,
                    /* deferred */ true,
                    &processor.telemetry,
                );
                on_complete(processor, outcome);
            },
        );
    }
}

// Durability-wait classification helpers (shared with `DatabaseController`).
/// Kind of durability wait dispatched by
/// [`SessionProcessor::ensure_candidate_info_stored`].
///
/// Each variant carries the dispatch semantics (which dedup-map to consult /
/// mutate, which registry op label to use) so logic never branches on string
/// labels. The associated [`Self::name`] / [`Self::op_label`] helpers return
/// `&'static str` for use in log messages and error formatting only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DurabilityWaitKind {
    /// Wait on `candidate_info_store_results` (CandidateInfoRecord persist).
    CandidateInfo,
    /// Wait on `notar_cert_store_results` (NotarCert persist).
    NotarCert,
}

impl DurabilityWaitKind {
    /// Human-readable label used purely in log lines and error strings.
    #[inline]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::CandidateInfo => "candidateInfo",
            Self::NotarCert => "notarCert",
        }
    }

    /// Registry op label used by `post_async_db_result`.
    #[inline]
    fn op_label(self) -> &'static str {
        match self {
            Self::CandidateInfo => "ensure_candidate_info_stored:candidate_info",
            Self::NotarCert => "ensure_candidate_info_stored:notar_cert",
        }
    }

    /// Clone the in-flight persist `StorageAsyncResultPtr` for `id` from the
    /// dedup-map that corresponds to this wait kind, if any.
    fn dedup_lookup(
        self,
        processor: &SessionProcessor,
        id: &RawCandidateId,
    ) -> Option<StorageAsyncResultPtr<()>> {
        match self {
            Self::CandidateInfo => processor.database.candidate_info_store_result(id).cloned(),
            Self::NotarCert => processor.database.notar_cert_store_result(id).cloned(),
        }
    }

    /// Whether the dedup-map still holds an entry for `id`.
    pub(crate) fn dedup_contains(self, database: &DatabaseController, id: &RawCandidateId) -> bool {
        match self {
            Self::CandidateInfo => database.contains_candidate_info_store(id),
            Self::NotarCert => database.contains_notar_cert_store(id),
        }
    }

    /// Remove the dedup-map entry for `id` (called when a wait observed a
    /// real storage `Err`, so the next caller recreates the async write
    /// instead of reusing a poisoned ptr).
    pub(crate) fn dedup_remove(self, database: &mut DatabaseController, id: &RawCandidateId) {
        match self {
            Self::CandidateInfo => {
                database.remove_candidate_info_store(id);
            }
            Self::NotarCert => {
                database.remove_notar_cert_store(id);
            }
        }
    }
}

/// Returns `true` iff `err` is the typed
/// [`consensus_common::StorageResultAlreadyTaken`] sentinel emitted by
/// `StorageAsyncResultImpl::{try_get, wait_timeout}` after the inner
/// `Result` was consumed by a prior caller (`AsyncResultState::Taken`).
///
/// Handler callbacks that may be invoked more than once for the same
/// `StorageAsyncResultPtr` use this helper to detect the typed sentinel and
/// then consult caller-specific dedup bookkeeping (e.g. whether the matching
/// dedup-map entry is still present). The sentinel itself does not encode
/// whether the first consumer observed `Ok` or `Err`.
///
/// Implementation note: detection is via `anyhow::Error::downcast_ref` — no
/// string allocation, no false positives on unrelated errors that happen to
/// mention "result already taken".
///
/// See `handle_notarization_reached` / `handle_skip_certificate_reached` /
/// `handle_finalization_reached` callbacks, and
/// `DatabaseController::classify_durability_wait_outcome` (candidate-info /
/// notar-cert durability waits).
pub(crate) fn is_storage_result_already_taken(err: &Error) -> bool {
    err.downcast_ref::<StorageResultAlreadyTaken>().is_some()
}

// ======================================================================
// Async-DB result registry
// ======================================================================
//
// Lets `SessionProcessor` register an in-flight `*_async()` DB result and continue
// without blocking SXMAIN. Drained from `check_all()` next to
// `process_delayed_actions()`, and synchronously at shutdown.

impl SessionProcessor {
    /// Register an async DB write result for SXMAIN polling.
    ///
    /// `on_ready` runs on `SXMAIN` exactly once when the result is ready
    /// (with `Ok(())` or `Err(...)` from the storage layer) or when `timeout`
    /// elapses (with `Err(...timed out...)`).
    ///
    /// Callers MUST NOT `wait()` on `result` after handing it off — the
    /// registry takes exclusive ownership of the completion via `try_get()`.
    ///
    /// # Arguments
    /// * `op_label` - Static label for logs / metrics / timeout error message.
    /// * `result`   - In-flight async DB result handle from `SimplexDb`.
    /// * `timeout`  - Session-time relative deadline for completion (`deadline =
    ///                self.now() + timeout`, where `self.now()` routes through
    ///                [`SessionDescription::get_time()`] — same clock used
    ///                throughout `SessionProcessor`, not raw wall-clock).
    /// * `on_ready` - One-shot continuation invoked on `SXMAIN`.
    ///
    /// # Returns
    /// A monotonic id, useful for traceability/dedup; the caller may discard it.
    fn post_async_db_result<F>(
        &mut self,
        op_label: &'static str,
        result: StorageAsyncResultPtr<()>,
        timeout: Duration,
        on_ready: F,
    ) -> PendingAsyncDbId
    where
        F: FnOnce(&mut SessionProcessor, Result<()>) + Send + 'static,
    {
        let now = self.now();
        let id = self.database.register_pending(op_label, result, Box::new(on_ready), now, timeout);

        log::trace!(
            "Session {} post_async_db_result: id={id} label='{op_label}' \
             timeout={}ms (pending_count={})",
            &self.session_id().to_hex_string()[..8],
            timeout.as_millis(),
            self.database.pending_count(),
        );

        // Wake immediately on the next loop iteration; if still pending, the loop
        // will re-arm `next_awake_time` to `now + ASYNC_DB_POLL_DELAY`.
        self.runtime.set_next_awake_time(now + ASYNC_DB_POLL_DELAY);

        id
    }

    /// Drain ready / timed-out entries from `pending_async_db_results`.
    ///
    /// For each entry:
    /// - `try_get() == Some(res)` (Ready): remove + invoke continuation with `res`.
    /// - `deadline <= now` (Pending past deadline): remove + invoke continuation
    ///   with `Err("<label>: db wait timed out")` and bump the timeout counter.
    /// - Otherwise (Pending, not yet timed out): leave in queue and schedule
    ///   `set_next_awake_time(now + ASYNC_DB_POLL_DELAY)`.
    ///
    /// Uses order-preserving removal (`Vec::remove`) so ready continuations are
    /// invoked in registration order (FIFO for callers that registered from
    /// ordered queues).
    ///
    /// Continuations may register new pending entries; those land at the end of
    /// the queue and are evaluated by the same loop iteration if already ready.
    /// Total work per pass is bounded by `check_all()`'s `check_execution_time!` budget.
    fn process_pending_async_db_results(&mut self) {
        let now = self.now();
        let mut i = 0;

        loop {
            match self.database.step_pending_drain(i, now) {
                PendingDrainStep::Done => break,
                PendingDrainStep::Ready { entry, result } => {
                    if let Ok(latency) = now.duration_since(entry.registered_at) {
                        self.telemetry
                            .async_db_completion_latency_histogram
                            .record(latency.as_millis() as f64);
                    }
                    log::trace!(
                        "Session {} process_pending_async_db_results: ready \
                         id={} label='{}' result_ok={}",
                        &self.session_id().to_hex_string()[..8],
                        entry.id,
                        entry.op_label,
                        result.is_ok(),
                    );
                    (entry.on_ready)(self, result);
                    // do not advance i - swap_remove moved the last entry into this slot
                }
                PendingDrainStep::TimedOut { entry } => {
                    self.telemetry.async_db_timeout_counter.increment(1);
                    log::warn!(
                        "Session {} process_pending_async_db_results: TIMEOUT \
                         id={} label='{}' after {}ms",
                        &self.session_id().to_hex_string()[..8],
                        entry.id,
                        entry.op_label,
                        now.duration_since(entry.registered_at).map(|d| d.as_millis()).unwrap_or(0),
                    );
                    let err = error!("{}: db wait timed out", entry.op_label);
                    (entry.on_ready)(self, Err(err));
                }
                PendingDrainStep::Pending => {
                    self.runtime.set_next_awake_time(now + ASYNC_DB_POLL_DELAY);
                    i += 1;
                }
            }
        }

        self.telemetry.async_db_pending_count_gauge.set(self.database.pending_count() as f64);
    }

    /// Drain the SXMAIN `pending_async_db_results` registry on shutdown.
    ///
    /// Polls each entry via `process_pending_async_db_results()` and sleeps
    /// briefly between passes to let the storage thread make progress. Bounded
    /// by `SHUTDOWN_DRAIN_TIMEOUT` so a stuck writer cannot park
    /// `SessionProcessor::stop()` indefinitely; the loop logs a warning if it
    /// hits the deadline with entries still queued.
    ///
    /// Continuations may schedule new entries (e.g., a chained `*_async()`
    /// write); those land at the end of the queue and are processed by the
    /// next pass — same semantics as `process_delayed_actions()`.
    ///
    /// **Clock source**: deadline + elapsed timing both go through
    /// [`Self::wall_now`] (real `SystemTime`). The session's manual clock is
    /// intentionally bypassed here so a test that freezes time cannot also
    /// freeze the shutdown deadline. Inside the body
    /// `process_pending_async_db_results()` keeps using `now()` for per-entry
    /// timeouts so per-entry FSM semantics are unchanged.
    ///
    /// Returns the elapsed wall-clock duration for diagnostics.
    fn drain_pending_async_db_results_for_shutdown(&mut self) -> Duration {
        let drain_started_at = self.wall_now();

        if self.database.pending_is_empty() {
            return Duration::ZERO;
        }

        let initial_count = self.database.pending_count();
        let deadline = drain_started_at + SHUTDOWN_DRAIN_TIMEOUT;
        log::info!(
            "Session {} stop: draining {} pending async DB result entries (deadline {}ms)",
            &self.session_id().to_hex_string()[..8],
            initial_count,
            SHUTDOWN_DRAIN_TIMEOUT.as_millis(),
        );

        while !self.database.pending_is_empty() {
            if self.wall_now() >= deadline {
                log::warn!(
                    "Session {} stop: drain deadline elapsed after {}ms, {} entries still pending",
                    &self.session_id().to_hex_string()[..8],
                    SHUTDOWN_DRAIN_TIMEOUT.as_millis(),
                    self.database.pending_count(),
                );
                break;
            }

            self.process_pending_async_db_results();

            if !self.database.pending_is_empty() {
                std::thread::sleep(SHUTDOWN_DRAIN_POLL_INTERVAL);
            }
        }

        let elapsed = self.wall_now().duration_since(drain_started_at).unwrap_or(Duration::ZERO);
        log::info!(
            "Session {} stop: drained pending_async_db_results in {}ms ({} -> {})",
            &self.session_id().to_hex_string()[..8],
            elapsed.as_millis(),
            initial_count,
            self.database.pending_count(),
        );
        elapsed
    }
}

// ======================================================================
// Startup recovery
// ======================================================================
//
// `StartupRecoveryBackend` impl: thin accessors exposing the kernel substates that
// `SessionStartupRecoveryProcessor` mutates while replaying bootstrap (restore logic
// lives on the recovery processor).

/*
    StartupRecoveryBackend implementation

    Exposes the kernel substates that `SessionStartupRecoveryProcessor` mutates
    while replaying bootstrap, plus two effect methods. The restore logic itself
    lives on the recovery processor (`startup_recovery.rs`); recovery runs once
    at bootstrap with a full `&mut SessionProcessor`, so the backend is
    implemented directly (no adapter), and each restore step touches a single
    substate at a time.
*/

impl StartupRecoveryBackend for SessionProcessor {
    fn simplex_state(&self) -> &SimplexState {
        &self.simplex_state
    }

    fn simplex_state_mut(&mut self) -> &mut SimplexState {
        &mut self.simplex_state
    }

    fn receiver(&self) -> &ReceiverPtr {
        &self.receiver
    }

    fn consensus_mut(&mut self) -> &mut ConsensusController {
        &mut self.consensus
    }

    fn candidate_book(&self) -> &CandidateBook {
        &self.candidate_book
    }

    fn candidate_book_mut(&mut self) -> &mut CandidateBook {
        &mut self.candidate_book
    }

    fn database(&self) -> &DatabaseController {
        &self.database
    }

    fn database_mut(&mut self) -> &mut DatabaseController {
        &mut self.database
    }

    fn increment_error(&self) {
        self.telemetry.increment_error();
    }

    fn sync_standstill_after_restore(&mut self) {
        // Range-clamp the receiver ingress cursor to the tracked interval and
        // reschedule the standstill timer. Goes through the SessionProcessor
        // helper, which spans state the recovery processor does not hold; it
        // also prunes cached votes outside the tracked interval.
        self.sync_standstill_slots_from_state();
        self.receiver.reschedule_standstill();
    }
}

// ======================================================================
// Tests
// ======================================================================
//
// Test-only `SessionProcessor` shims consolidated under one `#[cfg(test)]` so the
// production `impl` blocks carry no test scaffolding. Each is a thin delegator the
// `#[path]`-included unit tests call directly; production dispatch reaches the same
// controller methods through the `with_*_backend` seams.

#[cfg(test)]
impl SessionProcessor {
    /* Listener injection (test) */

    /// Rebind the session listener that callbacks dispatch to.
    ///
    /// Test-only seam. The listener lives inside the shared
    /// `Arc<SessionCallbacks>`; production sets it once at session
    /// creation. Tests build the processor with a placeholder listener and
    /// later inject a recording listener to observe callback delivery, so
    /// this rebuilds the shared callbacks aspect and re-points every holder
    /// (the processor and the validation controller's clone) at it.
    fn set_listener_for_test(&mut self, listener: crate::SessionListenerPtr) {
        let rebuilt = Arc::new(self.callbacks.with_listener(listener));
        self.callbacks = rebuilt.clone();
        self.validation.set_callbacks_for_test(rebuilt.clone());
        self.collation.set_callbacks_for_test(rebuilt.clone());
        self.consensus.set_callbacks_for_test(rebuilt);
    }

    // The generated-candidate validation watch + self-collation failure
    // bookkeeping live in `SessionTelemetry`; these thin wrappers inject the
    // session clock / description into the clockless telemetry recorders.

    /* Clock controls (test/replay) */

    /// Override session time (used for tests / log replay).
    fn set_time(&self, time: SystemTime) {
        self.runtime.description().set_time(time);
    }

    /// Advance session time by a duration (used for tests).
    fn advance_time(&self, delta: Duration) {
        self.runtime.description().set_time(self.now() + delta);
    }

    /* Telemetry wrappers */

    /// Thin telemetry wrapper kept for the validation-tracking unit test; the
    /// production path calls [`SessionTelemetry::track_generated_candidate_for_validation`]
    /// directly from [`CollationController::generated_block`].
    fn track_generated_candidate_for_validation(&mut self, candidate_id: RawCandidateId) {
        self.telemetry.track_generated_candidate_for_validation(candidate_id, self.now());
    }

    /// Thin telemetry wrapper kept for the validation-tracking unit test; the
    /// production path calls
    /// [`SessionTelemetry::mark_generated_candidate_validation_started`] directly
    /// from [`ValidationController::try_approve_block`].
    fn mark_generated_candidate_validation_started(&mut self, candidate_id: &RawCandidateId) {
        self.telemetry.mark_generated_candidate_validation_started(candidate_id);
    }

    /* Collation shims */

    /// Thin shim onto
    /// [`CollationController::resolve_parent_before_split_flag`], kept for the
    /// empty-block-policy unit test. The controller owns the parent
    /// `before_split` resolution (C++ `should_generate_empty_block` parity).
    fn resolve_parent_before_split_flag(
        &mut self,
        parent: Option<&crate::block::CandidateParentInfo>,
        prev_block_ids: &[BlockIdExt],
    ) -> Option<bool> {
        self.with_collation_backend(|c, b| {
            c.resolve_parent_before_split_flag(b, parent, prev_block_ids)
        })
    }

    fn compute_collation_start_time(
        &mut self,
        parent: Option<&crate::block::CandidateParentInfo>,
    ) -> SystemTime {
        self.with_collation_backend(|c, b| c.compute_collation_timing(parent, b).dispatch_time)
    }

    fn compute_collation_min_gen_time(
        &mut self,
        parent: Option<&crate::block::CandidateParentInfo>,
    ) -> SystemTime {
        self.with_collation_backend(|c, b| c.compute_collation_timing(parent, b).min_gen_time)
    }

    /// Thin dispatch onto
    /// [`CollationController::on_collation_complete`] (normal + empty blocks),
    /// kept for the publication unit tests that drive it directly.
    fn on_collation_complete(&mut self, slot: SlotIndex, request_id: u32, result: CollationResult) {
        self.with_collation_backend(|collation, backend| {
            collation.on_collation_complete(backend, slot, request_id, result)
        });
    }

    /// Thin dispatch onto
    /// [`CollationController::on_collation_failed_impl`] (drop-vs-retry
    /// classification + retry scheduling live there), kept for the failure-path
    /// unit tests that drive it directly.
    fn on_collation_failed_impl(
        &mut self,
        slot: SlotIndex,
        request_id: u32,
        err: Error,
        retry_count: u32,
    ) {
        self.with_collation_backend(|collation, backend| {
            collation.on_collation_failed_impl(backend, slot, request_id, err, retry_count)
        });
    }

    /// Thin shim onto [`CollationController::create_normal_block_desc`] (kept
    /// for the genesis-seqno unit test that drives it directly).
    fn create_normal_block_desc(
        &mut self,
        slot: SlotIndex,
        candidate: &crate::ValidatorBlockCandidate,
        parent: &Option<crate::block::CandidateParentInfo>,
    ) -> Result<crate::collation_controller::GeneratedBlockDesc> {
        self.with_collation_backend(|c, b| c.create_normal_block_desc(b, slot, candidate, parent))
    }

    /// Thin dispatch onto
    /// [`CollationController::precollate_block`] (parent-selection gates +
    /// `invoke_collation` dispatch live there), kept for the precollation unit
    /// tests that drive it directly. The full precollation pipeline (C++
    /// `generate_candidates()` candidate-chaining parity) is documented on the
    /// controller module.
    fn precollate_block(&mut self, slot: SlotIndex) {
        self.with_collation_backend(|collation, backend| collation.precollate_block(backend, slot));
    }

    /* Validation shims */

    /// Handle a successful higher-layer validation decision.
    ///
    /// Production routes the validator decision callback straight to
    /// [`ValidationController::candidate_decision_ok`] through the controller's
    /// deferred-work queue (built in `ValidationController::try_approve_block`).
    /// This thin `with_validation_backend` wrapper is retained only as an
    /// integration entry point for the `#[path]` unit tests, so they exercise the
    /// real `ValidationBackendAdapter` without assembling a backend by hand.
    fn candidate_decision_ok(
        &mut self,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        validity_start_time: SystemTime,
        receive_time: SystemTime,
    ) {
        self.with_validation_backend(move |validation, backend| {
            validation.candidate_decision_ok(
                backend,
                slot,
                candidate_id,
                validity_start_time,
                receive_time,
            )
        });
    }

    /// Internal helper for a successful validation, normal + empty block paths.
    ///
    /// Production calls [`ValidationController::candidate_decision_ok_internal`]
    /// directly from the controller; this wrapper is retained only as a `#[path]`
    /// unit-test integration entry point.
    fn candidate_decision_ok_internal(
        &mut self,
        candidate_id: RawCandidateId,
        slot: SlotIndex,
        receive_time: SystemTime,
    ) {
        self.validation.candidate_decision_ok_internal(candidate_id, slot, receive_time);
    }

    /// Handle a failed higher-layer validation decision.
    ///
    /// Counterpart of [`Self::candidate_decision_ok`]: production routes through
    /// the controller's queue, this wrapper is retained only as a `#[path]`
    /// unit-test integration entry point.
    fn candidate_decision_fail(
        &mut self,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        err: Error,
    ) {
        self.with_validation_backend(move |validation, backend| {
            validation.candidate_decision_fail(backend, slot, candidate_id, err)
        });
    }

    /* Consensus shims */

    /// Test-only delegator: drive [`ConsensusController::handle_block_finalized`]
    /// through the borrowing backend. Production dispatches this from
    /// [`Self::process_simplex_events`]; the `#[path]`-included unit tests call
    /// it directly on the processor.
    fn handle_block_finalized(&mut self, event: BlockFinalizedEvent) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.handle_block_finalized(backend, event)
        });
    }

    /// Test-only delegator: drive
    /// [`ConsensusController::handle_notarization_reached`] through the borrowing
    /// backend. Production dispatches this from [`Self::process_simplex_events`].
    fn handle_notarization_reached(&mut self, event: NotarizationReachedEvent) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.handle_notarization_reached(backend, event)
        });
    }

    /// Test-only delegator: drive
    /// [`ConsensusController::handle_skip_certificate_reached`] through the
    /// borrowing backend. Production dispatches this from
    /// [`Self::process_simplex_events`].
    fn handle_skip_certificate_reached(&mut self, event: SkipCertificateReachedEvent) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.handle_skip_certificate_reached(backend, event)
        });
    }

    /// Test-only delegator: drive
    /// [`ConsensusController::handle_finalization_reached`] through the borrowing
    /// backend. Production dispatches this from [`Self::process_simplex_events`].
    fn handle_finalization_reached(&mut self, event: FinalizationReachedEvent) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.handle_finalization_reached(backend, event)
        });
    }

    /// Test-only delegators for the recursive-finalization machinery that now
    /// lives on [`ConsensusController`]. These thin shims keep the
    /// `#[path]`-included unit tests calling through the processor.
    #[allow(clippy::too_many_arguments)]
    fn try_emit_recursive_finalized_callback(
        &mut self,
        candidate_id: &RawCandidateId,
        received: &ReceivedCandidate,
        event: &BlockFinalizedEvent,
        has_final_cert: bool,
        trigger_slot: SlotIndex,
        trigger_candidate_hash_data: &[u8],
        complete: &mut bool,
    ) {
        self.with_consensus_backend(|consensus, backend| {
            consensus.try_emit_recursive_finalized_callback(
                backend,
                candidate_id,
                received,
                event,
                has_final_cert,
                trigger_slot,
                trigger_candidate_hash_data,
                complete,
            )
        });
    }

    fn maybe_apply_finalized_state(
        &mut self,
        finalized_id: &RawCandidateId,
        is_final: bool,
    ) -> bool {
        self.with_consensus_backend(|consensus, backend| {
            consensus.maybe_apply_finalized_state(backend, finalized_id, is_final)
        })
    }
}

#[cfg(test)]
#[path = "tests/test_session_processor.rs"]
mod tests;
