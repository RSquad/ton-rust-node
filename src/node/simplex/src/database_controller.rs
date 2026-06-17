/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `DatabaseController` entity
//!
//! Per-session persistence-ordering controller. Owns the database-facing
//! fields that used to live inline on `SessionProcessor`:
//!
//! - `db: SimplexDbPtr` — the underlying `SimplexDb` handle. Currently
//!   exposed through `db()` so existing call sites can keep their
//!   `self.database.db().save_X_async(...)` shape; persistence helpers
//!   (`save_candidate_info_to_db`, `persist_generated_candidate_info_to_db`,
//!   `maybe_store_pool_state`, blocking `load_*_from_db`) migrate in a
//!   later commit in this stack.
//! - `first_nonannounced_window: WindowIndex` — pool-state cursor (C++
//!   `SimplexPoolImpl::first_nonannounced_window_`).
//! - Four `*_store_results` maps that track in-flight / completed async DB
//!   writes for `candidate_info`, `notar_cert`, `final_cert`, and
//!   `skip_cert`.
//!
//! Future migrations (this stack):
//!
//! - Persistence helpers listed above.
//! - The `pending_async_db_results` registry plus its
//!   `next_pending_async_db_id` allocator — the wait-and-continuation
//!   queue drained from `check_all()`.
//!
//! ## Boundary
//!
//! `DatabaseController` owns DB-port state and persistence ordering only.
//! It does NOT drive the FSM, send votes, or run finalization. Cert
//! handlers in `SessionProcessor` keep their existing structure and reach
//! the controller through accessors; the controller never reaches back
//! into `SessionProcessor`.
//!
//! Some entry points need an `&mut SessionRuntime` for wake scheduling
//! (`post_async_db_result` lowers `next_awake_time`) and an
//! `&SessionTelemetry` for error / latency counters. These are passed
//! explicitly per call (once the helpers migrate in a later commit) so the
//! controller stays independently constructible in tests.
//!
//! Callers (`SessionProcessor`, recovery, cert handlers) hold an inline
//! `DatabaseController` on `SessionProcessor` and reach it via the
//! accessor surface; tests included via `#[path]` reach the same accessor
//! surface and never inspect raw fields directly.

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    database::{CandidateInfoRecord, SimplexDbPtr},
    session_processor::{is_storage_result_already_taken, DurabilityWaitKind, SessionProcessor},
    session_telemetry::SessionTelemetry,
};
use consensus_common::{SessionId, StorageAsyncResultPtr};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};
use ton_api::deserialize_typed;
use ton_block::{error, Result, UInt256};

/*
    --------------------------------------------------------------------
    Pending async DB result registry
    --------------------------------------------------------------------
*/

/// Monotonic id assigned by [`DatabaseController::register_pending`].
///
/// Returned for traceability / diagnostics. Callers may keep the id to dedup
/// re-entrant register calls (e.g. cert handlers re-fired with the same key).
pub(crate) type PendingAsyncDbId = u64;

/// Continuation invoked exactly once on result completion (or timeout).
///
/// Runs on SXMAIN from `SessionProcessor::process_pending_async_db_results`
/// after the controller has popped the entry. Captures whatever state the
/// caller needs to fold back into `SessionProcessor` (slot map, telemetry,
/// receiver commands, etc.).
pub(crate) type PendingAsyncDbCallback = Box<dyn FnOnce(&mut SessionProcessor, Result<()>) + Send>;

/// Registry entry. Stored privately on [`DatabaseController`]; exposed only
/// through the typed accessor surface plus the drain helper
/// [`DatabaseController::step_pending_drain`].
pub(crate) struct PendingAsyncDbEntry {
    /// Monotonic id assigned by [`DatabaseController::register_pending`].
    pub(crate) id: PendingAsyncDbId,
    /// Static label for logging, metrics, and timeout error messages
    /// (e.g. `"persist_our_vote_before_broadcast"`). MUST be `'static` so
    /// it can safely be embedded in error strings without extra allocation.
    pub(crate) op_label: &'static str,
    /// In-flight async DB result. Polled via `try_get()` from
    /// [`DatabaseController::step_pending_drain`].
    pub(crate) result: StorageAsyncResultPtr<()>,
    /// Time at which this entry was registered (used for completion-latency
    /// metrics).
    pub(crate) registered_at: SystemTime,
    /// Wall-clock deadline. If `now >= deadline` and `try_get()` is still
    /// pending, the entry is popped and surfaced as
    /// [`PendingDrainStep::TimedOut`].
    pub(crate) deadline: SystemTime,
    /// One-shot continuation invoked by `SessionProcessor` after the entry
    /// is drained.
    pub(crate) on_ready: PendingAsyncDbCallback,
}

/// Outcome of a single step in
/// `SessionProcessor::process_pending_async_db_results`'s drain loop.
///
/// Encapsulates the swap-remove decision so the loop on
/// `SessionProcessor` stays trivially auditable: each call either pops an
/// entry (`Ready` / `TimedOut`) and the caller does NOT advance the
/// index, or returns `Pending` (caller advances), or returns `Done` when
/// `i >= pending_count()`.
pub(crate) enum PendingDrainStep {
    /// `i >= pending_count()`: nothing left to process at this index.
    Done,
    /// Entry at `i` is ready; popped and returned together with the
    /// `try_get()` result. The caller must invoke `entry.on_ready` and
    /// MUST NOT advance the index (a `swap_remove` happened).
    Ready { entry: PendingAsyncDbEntry, result: Result<()> },
    /// Entry at `i` passed its deadline without completing; popped and
    /// returned. The caller must invoke `entry.on_ready` with a timeout
    /// error and MUST NOT advance the index.
    TimedOut { entry: PendingAsyncDbEntry },
    /// Entry at `i` is still pending and not yet timed out; the caller
    /// must schedule a wake and advance the index.
    Pending,
}

/// Per-session persistence-ordering controller.
///
/// Owned by
/// [`SessionProcessor`](crate::session_processor::SessionProcessor) as
/// `self.database`; all access goes through the accessor methods below.
pub(crate) struct DatabaseController {
    /// The underlying `SimplexDb` handle. Exposed through [`Self::db`] so
    /// existing persistence call sites keep their
    /// `self.database.db().save_X_async(...)` shape until the helpers
    /// migrate in a later commit in this stack.
    db: SimplexDbPtr,

    /// Pool-state cursor: the next leader window for which we still need
    /// to announce a state record on this validator. Mirrors C++
    /// `SimplexPoolImpl::first_nonannounced_window_`.
    first_nonannounced_window: WindowIndex,

    /// `CandidateInfo` DB writes in-flight / completed (for
    /// `WaitCandidateInfoStored` parity).
    candidate_info_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,

    /// `NotarCert` DB writes in-flight / completed (for
    /// `WaitCandidateInfoStored` parity).
    notar_cert_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,

    /// `FinalCert` DB writes in-flight / completed.
    final_cert_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,

    /// `SkipCert` DB writes in-flight / completed.
    skip_cert_store_results: HashMap<SlotIndex, StorageAsyncResultPtr<()>>,

    /// Pending async DB result registry. Each entry is drained from
    /// [`Self::step_pending_drain`]; the orchestrator on `SessionProcessor`
    /// invokes the continuation closure after the entry has been popped.
    pending_async_db_results: Vec<PendingAsyncDbEntry>,

    /// Monotonic allocator for [`PendingAsyncDbId`]. `wrapping_add(1)` is
    /// fine because the id space (`u64`) is effectively unbounded and the
    /// id is only used for logging / dedup, never as a stable map key.
    next_pending_async_db_id: PendingAsyncDbId,
}

// ======================================================================
// Construction & handles
// ======================================================================
// Build the controller; expose the DB handle + pool-state cursor.
impl DatabaseController {
    /// Construct a database controller with the given `db` handle and an
    /// initial `first_nonannounced_window` value (typically
    /// `WindowIndex::default()`; recovery overwrites it via
    /// [`Self::set_first_nonannounced_window`]).
    pub(crate) fn new(db: SimplexDbPtr, first_nonannounced_window: WindowIndex) -> Self {
        Self {
            db,
            first_nonannounced_window,
            candidate_info_store_results: HashMap::new(),
            notar_cert_store_results: HashMap::new(),
            final_cert_store_results: HashMap::new(),
            skip_cert_store_results: HashMap::new(),
            pending_async_db_results: Vec::new(),
            next_pending_async_db_id: 0,
        }
    }

    /// Borrow the underlying `SimplexDb` handle. Used by persistence call
    /// sites that still live on `SessionProcessor`; those move into the
    /// controller in a later commit in this stack.
    pub(crate) fn db(&self) -> &SimplexDbPtr {
        &self.db
    }

    /// Current pool-state cursor value.
    pub(crate) fn first_nonannounced_window(&self) -> WindowIndex {
        self.first_nonannounced_window
    }

    /// Overwrite the pool-state cursor. Used by
    /// `recovery_set_first_nonannounced_window` and by `maybe_store_pool_state`
    /// after a successful announce.
    pub(crate) fn set_first_nonannounced_window(&mut self, window: WindowIndex) {
        self.first_nonannounced_window = window;
    }
}

// ======================================================================
// Async store-result maps
// ======================================================================
// In-flight / completed async-write dedup maps for candidate_info + the three
// certificate kinds, and the per-slot history pruning.
impl DatabaseController {
    /* Candidate-info store */

    /// Borrow a pending / completed `candidate_info` write result, if any.
    pub(crate) fn candidate_info_store_result(
        &self,
        id: &RawCandidateId,
    ) -> Option<&StorageAsyncResultPtr<()>> {
        self.candidate_info_store_results.get(id)
    }

    /// Whether a `candidate_info` write has already been registered for this id.
    /// Drives the durability-wait dedup check
    /// ([`DurabilityWaitKind::dedup_contains`](crate::session_processor)); the
    /// initial registration happens in [`Self::save_candidate_info_to_db`].
    pub(crate) fn contains_candidate_info_store(&self, id: &RawCandidateId) -> bool {
        self.candidate_info_store_results.contains_key(id)
    }

    /// Drop a `candidate_info` write result, if present. Called from the
    /// durability-wait dedup path
    /// ([`DurabilityWaitKind::dedup_remove`](crate::session_processor)) when a
    /// wait observed a real storage `Err`, so the next caller recreates the
    /// async write instead of reusing a poisoned ptr.
    pub(crate) fn remove_candidate_info_store(
        &mut self,
        id: &RawCandidateId,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.candidate_info_store_results.remove(id)
    }

    /* Notar-cert store */

    /// Borrow a pending / completed `notar_cert` write result, if any.
    pub(crate) fn notar_cert_store_result(
        &self,
        id: &RawCandidateId,
    ) -> Option<&StorageAsyncResultPtr<()>> {
        self.notar_cert_store_results.get(id)
    }

    /// Whether a `notar_cert` write has already been registered for this
    /// id.
    pub(crate) fn contains_notar_cert_store(&self, id: &RawCandidateId) -> bool {
        self.notar_cert_store_results.contains_key(id)
    }

    /// Register a fresh `notar_cert` write result; returns the previous
    /// value if any.
    pub(crate) fn insert_notar_cert_store(
        &mut self,
        id: RawCandidateId,
        result: StorageAsyncResultPtr<()>,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.notar_cert_store_results.insert(id, result)
    }

    /// Drop a `notar_cert` write result, if present.
    pub(crate) fn remove_notar_cert_store(
        &mut self,
        id: &RawCandidateId,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.notar_cert_store_results.remove(id)
    }

    /* Final-cert store */

    /// Borrow a pending / completed `final_cert` write result, if any.
    pub(crate) fn final_cert_store_result(
        &self,
        id: &RawCandidateId,
    ) -> Option<&StorageAsyncResultPtr<()>> {
        self.final_cert_store_results.get(id)
    }

    /// Whether a `final_cert` write has already been registered for this id.
    pub(crate) fn contains_final_cert_store(&self, id: &RawCandidateId) -> bool {
        self.final_cert_store_results.contains_key(id)
    }

    /// Register a fresh `final_cert` write result; returns the previous
    /// value if any.
    pub(crate) fn insert_final_cert_store(
        &mut self,
        id: RawCandidateId,
        result: StorageAsyncResultPtr<()>,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.final_cert_store_results.insert(id, result)
    }

    /// Drop a `final_cert` write result, if present.
    pub(crate) fn remove_final_cert_store(
        &mut self,
        id: &RawCandidateId,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.final_cert_store_results.remove(id)
    }

    /* Skip-cert store */

    /// Borrow a pending / completed `skip_cert` write result for this
    /// slot, if any.
    pub(crate) fn skip_cert_store_result(
        &self,
        slot: SlotIndex,
    ) -> Option<&StorageAsyncResultPtr<()>> {
        self.skip_cert_store_results.get(&slot)
    }

    /// Whether a `skip_cert` write has already been registered for this slot.
    pub(crate) fn contains_skip_cert_store(&self, slot: SlotIndex) -> bool {
        self.skip_cert_store_results.contains_key(&slot)
    }

    /// Register a fresh `skip_cert` write result for this slot; returns
    /// the previous value if any.
    pub(crate) fn insert_skip_cert_store(
        &mut self,
        slot: SlotIndex,
        result: StorageAsyncResultPtr<()>,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.skip_cert_store_results.insert(slot, result)
    }

    /// Drop a `skip_cert` write result, if present.
    pub(crate) fn remove_skip_cert_store(
        &mut self,
        slot: SlotIndex,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.skip_cert_store_results.remove(&slot)
    }

    /* History pruning */

    /// Prune all four `*_store_results` maps so that only entries for
    /// `slot >= up_to_slot` remain. Used by `cleanup_old_candidates` to
    /// garbage-collect state for finalized slots.
    pub(crate) fn prune_below(&mut self, up_to_slot: SlotIndex) {
        self.candidate_info_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.notar_cert_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.final_cert_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.skip_cert_store_results.retain(|slot, _| *slot >= up_to_slot);
    }
}

// ======================================================================
// Persistence helpers
// ======================================================================
// Synchronous candidate-info save + blocking loads.
impl DatabaseController {
    /// Persist a `CandidateInfoRecord` once per `candidate_id`, dedup'd via
    /// `candidate_info_store_results` (mirrors `WaitCandidateInfoStored`
    /// parity). Fire-and-forget: errors are logged and `telemetry.increment_error()`
    /// is called.
    ///
    /// C++ reference: `validator/consensus/simplex/candidate-resolver.cpp`
    /// (`CandidateReceived` handler calls `store_to_db(id, state).start().detach()`).
    pub(crate) fn save_candidate_info_to_db(
        &mut self,
        slot: SlotIndex,
        candidate_hash: &UInt256,
        leader_idx: ValidatorIndex,
        candidate_hash_data_bytes: &[u8],
        signature: Vec<u8>,
        session_id: &SessionId,
        telemetry: &SessionTelemetry,
    ) {
        let candidate_hash_data = match deserialize_typed(candidate_hash_data_bytes) {
            Ok(data) => data,
            Err(e) => {
                log::warn!(
                    "Session {} save_candidate_info_to_db: failed to deserialize \
                    CandidateHashData for slot={slot}: {e}",
                    &session_id.to_hex_string()[..8],
                );
                return;
            }
        };

        let record = CandidateInfoRecord {
            candidate_id: RawCandidateId { slot, hash: candidate_hash.clone() },
            leader_idx: leader_idx.value(),
            candidate_hash_data,
            signature,
        };

        if self.candidate_info_store_results.contains_key(&record.candidate_id) {
            return;
        }

        match self.db.save_candidate_info_async(&record) {
            Ok(result) => {
                self.candidate_info_store_results.insert(record.candidate_id.clone(), result);
            }
            Err(e) => {
                log::error!(
                    "Session {} store_candidate_info: failed to create candidate_info save: {e}",
                    &session_id.to_hex_string()[..8],
                );
                telemetry.increment_error();
            }
        }
    }

    /// Blocking lookup of `CandidateInfoRecord` for the given id. Used by the
    /// rare query-fallback path; returns `None` on lookup error (logged at
    /// `warn`).
    pub(crate) fn load_candidate_info_from_db(
        &self,
        candidate_id: &RawCandidateId,
        session_id: &SessionId,
    ) -> Option<CandidateInfoRecord> {
        const DB_TIMEOUT: Duration = Duration::from_secs(2);

        match self.db.load_candidate_info_by_id(candidate_id, DB_TIMEOUT) {
            Ok(record) => record,
            Err(e) => {
                log::warn!(
                    "Session {} load_candidate_info_from_db: failed for slot={}: {e}",
                    &session_id.to_hex_string()[..8],
                    candidate_id.slot,
                );
                None
            }
        }
    }

    /// Blocking lookup of notar-cert bytes for the given id. Used by the rare
    /// query-fallback path; returns an empty `Vec` on lookup miss or error
    /// (errors logged at `debug` because the fallback caller retries through
    /// other paths).
    pub(crate) fn load_notar_cert_bytes_from_db(
        &self,
        candidate_id: &RawCandidateId,
        session_id: &SessionId,
    ) -> Vec<u8> {
        const DB_TIMEOUT: Duration = Duration::from_secs(2);

        match self.db.load_notar_cert_by_id(candidate_id, DB_TIMEOUT) {
            Ok(Some(record)) => record.notar_cert_bytes,
            Ok(None) => Vec::new(),
            Err(e) => {
                log::debug!(
                    "Session {} load_notar_cert_bytes_from_db: failed for slot={}: {e}",
                    &session_id.to_hex_string()[..8],
                    candidate_id.slot,
                );
                Vec::new()
            }
        }
    }
}

// ======================================================================
// Pending async-DB registry
// ======================================================================
// Wait-and-continuation queue drained from check_all(): registration, one
// drain step, and durability-wait outcome classification.
impl DatabaseController {
    /// Number of entries currently registered (in-flight + just-completed
    /// not yet drained).
    pub(crate) fn pending_count(&self) -> usize {
        self.pending_async_db_results.len()
    }

    /// `true` iff [`Self::pending_count`] is zero.
    pub(crate) fn pending_is_empty(&self) -> bool {
        self.pending_async_db_results.is_empty()
    }

    /// Register an in-flight async DB write for SXMAIN polling.
    ///
    /// Returns a monotonic id useful for traceability / dedup; the caller
    /// may discard it. The continuation runs exactly once, invoked by the
    /// orchestrator on `SessionProcessor` after the entry has been popped
    /// via [`Self::step_pending_drain`].
    pub(crate) fn register_pending(
        &mut self,
        op_label: &'static str,
        result: StorageAsyncResultPtr<()>,
        on_ready: PendingAsyncDbCallback,
        now: SystemTime,
        timeout: Duration,
    ) -> PendingAsyncDbId {
        let id = self.next_pending_async_db_id;
        self.next_pending_async_db_id = self.next_pending_async_db_id.wrapping_add(1);

        self.pending_async_db_results.push(PendingAsyncDbEntry {
            id,
            op_label,
            result,
            registered_at: now,
            deadline: now + timeout,
            on_ready,
        });

        id
    }

    /// Inspect the entry at `i`. If ready or timed out, the entry is
    /// `swap_remove`'d and returned to the caller for continuation
    /// dispatch; if still pending, returns [`PendingDrainStep::Pending`]
    /// without modifying the queue.
    ///
    /// Mirrors the original swap-remove / non-incrementing-index pattern
    /// from `process_pending_async_db_results` so that continuations that
    /// register new pending entries get evaluated in the same drain pass
    /// (the new entry lands at the new last position and is observed when
    /// the loop returns to index `i` after the swap).
    pub(crate) fn step_pending_drain(&mut self, i: usize, now: SystemTime) -> PendingDrainStep {
        if i >= self.pending_async_db_results.len() {
            return PendingDrainStep::Done;
        }
        let try_get_result = self.pending_async_db_results[i].result.try_get();
        let timed_out = self.pending_async_db_results[i].deadline <= now;
        match (try_get_result, timed_out) {
            (Some(result), _) => {
                let entry = self.pending_async_db_results.swap_remove(i);
                PendingDrainStep::Ready { entry, result }
            }
            (None, true) => {
                let entry = self.pending_async_db_results.swap_remove(i);
                PendingDrainStep::TimedOut { entry }
            }
            (None, false) => PendingDrainStep::Pending,
        }
    }

    /// Translate a raw `try_get()` / registry callback outcome for a
    /// `candidateInfo` or `notarCert` wait into the `Ok(())` / `Err(...)`
    /// value that flows back to the caller. Centralizes the logging +
    /// error-counter side-effects so the inline-Ready and deferred-Pending
    /// branches stay identical.
    ///
    /// `try_get_result == None` only happens on the inline-Ready path
    /// when the `is_ready()` check raced against a separate consumer;
    /// treat as `Ok(())` to match `Some(Ok(()))` semantics.
    ///
    /// Method so the classifier can inspect/remove the relevant dedup-map entry
    /// while preserving the same behavior from the synchronous Ready arm and
    /// deferred continuation closure.
    pub(crate) fn classify_durability_wait_outcome(
        &mut self,
        try_get_result: Option<Result<()>>,
        kind: DurabilityWaitKind,
        candidate_id: &RawCandidateId,
        session_short: &str,
        slot_v: u32,
        block_hash_short: &str,
        deferred: bool,
        telemetry: &SessionTelemetry,
    ) -> Result<()> {
        let stage = if deferred { " (deferred)" } else { "" };
        let label = kind.name();
        match try_get_result {
            Some(Ok(())) | None => {
                log::trace!(
                    "Session {session_short} EnsureCandidateInfoStored: {label} stored for \
                     s{slot_v}:{block_hash_short}{stage}",
                );
                Ok(())
            }
            Some(Err(e)) if is_storage_result_already_taken(&e) => {
                if kind.dedup_contains(self, candidate_id) {
                    log::trace!(
                        "Session {session_short} EnsureCandidateInfoStored: {label} result already \
                         consumed for s{slot_v}:{block_hash_short}{stage}",
                    );
                    Ok(())
                } else {
                    log::error!(
                        "Session {session_short} EnsureCandidateInfoStored: {label} result already \
                         consumed but dedup entry is missing for s{slot_v}:{block_hash_short}{stage}",
                    );
                    telemetry.increment_error();
                    Err(error!(
                        "{label} wait observed already-taken result after prior failure for \
                         s{slot_v}:{block_hash_short}"
                    ))
                }
            }
            Some(Err(e)) => {
                kind.dedup_remove(self, candidate_id);
                log::error!(
                    "Session {session_short} EnsureCandidateInfoStored: {label} wait failed for \
                     s{slot_v}:{block_hash_short}{stage}: {e}",
                );
                telemetry.increment_error();
                Err(e)
            }
        }
    }
}

impl std::fmt::Debug for DatabaseController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseController")
            .field("first_nonannounced_window", &self.first_nonannounced_window)
            .field("candidate_info_store_count", &self.candidate_info_store_results.len())
            .field("notar_cert_store_count", &self.notar_cert_store_results.len())
            .field("final_cert_store_count", &self.final_cert_store_results.len())
            .field("skip_cert_store_count", &self.skip_cert_store_results.len())
            .field("pending_async_db_count", &self.pending_async_db_results.len())
            .field("next_pending_async_db_id", &self.next_pending_async_db_id)
            .finish_non_exhaustive()
    }
}

// ======================================================================
// Tests
// ======================================================================
// Test-only accessors consolidated under one `#[cfg(test)]` impl so the
// production impls carry no test scaffolding.
#[cfg(test)]
impl DatabaseController {
    /// Register a fresh `candidate_info` write result; returns the
    /// previous value if any. Currently only exercised by
    /// `#[path]`-included tests that simulate a pre-registered entry; the
    /// production insert lives inside
    /// [`Self::save_candidate_info_to_db`].
    pub(crate) fn insert_candidate_info_store(
        &mut self,
        id: RawCandidateId,
        result: StorageAsyncResultPtr<()>,
    ) -> Option<StorageAsyncResultPtr<()>> {
        self.candidate_info_store_results.insert(id, result)
    }

    /// Borrow the registry entries in insertion order. Used by
    /// `#[path]`-included tests that assert on `op_label`s or compute
    /// indices via [`Iterator::position`].
    pub(crate) fn pending_iter(&self) -> impl Iterator<Item = &PendingAsyncDbEntry> {
        self.pending_async_db_results.iter()
    }

    /// Mutable iteration over the registry entries. Used by
    /// `#[path]`-included tests that simulate a storage failure by
    /// replacing an entry's `result` with a manually-failed
    /// [`StorageAsyncResultPtr`] before re-driving the drain loop.
    pub(crate) fn pending_iter_mut(&mut self) -> impl Iterator<Item = &mut PendingAsyncDbEntry> {
        self.pending_async_db_results.iter_mut()
    }
}
