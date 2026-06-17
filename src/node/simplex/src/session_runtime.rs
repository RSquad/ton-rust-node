/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `SessionRuntime` entity
//!
//! Per-session runtime context that will own both the immutable bootstrap
//! handles (description, session-start parents, stop flag) and the mutable
//! cross-controller scratch state (slot map, delayed-action queue, next-wake
//! bookkeeping, receiver-activity mirror). Pairs with [`SessionDescription`],
//! which keeps the immutable protocol/topology config.
//!
//! ## Boundary
//!
//! `SessionRuntime` owns data only. It does NOT call back into
//! `SessionProcessor`, controllers, or aspects; callers borrow
//! `&SessionRuntime` or `&mut SessionRuntime` per method.
//!
//! ## Inclusion test (the discipline rule)
//!
//! - **Immutable bootstrap fields** belong here if they are session-scoped,
//!   set at construction, and read by ≥2 components (or are conceptually
//!   session-shaped handles like `Arc<SessionDescription>` or `stop_flag`).
//! - **Mutable scratch fields** belong here iff ALL three hold: (a) mutable
//!   during `check_all`, (b) read by ≥2 distinct components, (c) not FSM
//!   state and not pure observability.
//! - Single-component fields stay in the owning component (preferred) or its
//!   reader (`SessionTelemetry` for diagnostics-only single-reader patterns).
//!
//! ## Encapsulation
//!
//! Substructs are stored as private fields and exposed through accessor
//! methods. Callers — including `SessionProcessor` and inline tests included
//! via `#[path]` — go through the accessor surface; they never reach into
//! `SessionRuntime`'s fields directly.

use crate::{
    block::SlotIndex, session_description::SessionDescription, task_queue::TaskPtr, ValidatorWeight,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_block::BlockIdExt;

/// Fallback poll horizon used by `reset_next_awake_time`.
///
/// TODO(simplex-timing): experimental 10ms wake fallback for simnet/testnet validation.
/// Restore the old "far future" behavior after the wake-discipline fixes are validated.
pub(crate) const MAX_AWAKE_TIMEOUT: Duration = Duration::from_millis(10);

/// Encode a wake instant as nanoseconds since `UNIX_EPOCH` for storage in the
/// atomic `next_awake_time`. The encoding is monotone (earlier instant → smaller
/// `u64`), which is what lets `set_next_awake_time` use `fetch_min` to preserve
/// its "only lowers" semantics. Saturates instead of overflowing; the `u64`
/// nanosecond range only runs out circa year 2554, far beyond any wake horizon.
#[inline]
fn wake_to_nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_nanos().min(u64::MAX as u128) as u64).unwrap_or(0)
}

/// Inverse of [`wake_to_nanos`]. Round-trips exactly for any instant with
/// nanosecond resolution at or after `UNIX_EPOCH`.
#[inline]
fn wake_from_nanos(nanos: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos)
}

// ======================================================================
// SessionRuntime — per-session runtime context
// ======================================================================

/// Per-session runtime context.
///
/// Owned by
/// [`SessionProcessor`](crate::session_processor::SessionProcessor)
/// as `self.runtime`; all access goes through the accessor methods below.
pub(crate) struct SessionRuntime {
    /* Immutable bootstrap handles */
    /// Session description — validators, weights, options, identity, clock.
    description: Arc<SessionDescription>,
    /// Explicit session-start parents supplied by `ValidatorGroup`. Used as
    /// the fallback collation parent set whenever `SimplexState` reports
    /// the session-base sentinel (`parent=None`).
    session_start_prev_blocks: Vec<BlockIdExt>,
    /// Stop flag shared with the `SimplexSession` main loop. Read by
    /// `invoke_session_callback` to suppress late callbacks once the
    /// session is winding down.
    stop_flag: Arc<AtomicBool>,

    /* Mutable cross-controller scratch */
    slots: SlotMap,
    scheduling: DelayedActionQueue,
    /// Wake horizon for the main loop, stored as nanoseconds since
    /// `UNIX_EPOCH` (see [`wake_to_nanos`]). Lowered monotonically by
    /// `set_next_awake_time` (`fetch_min`) over the course of a `check_all()`
    /// pass; reset to `now + MAX_AWAKE_TIMEOUT` at the start of each pass via
    /// `reset_next_awake_time`. Held as an atomic so the setters are `&self`,
    /// letting a read-only backend lower the horizon without a `&mut` borrow
    /// of the runtime. All writes today occur on the SXMAIN thread, so
    /// `Ordering::Relaxed` suffices.
    next_awake_time: AtomicU64,
    /// Mirror of the receiver-side activity figures refreshed via
    /// `SessionProcessor::on_activity`.
    receiver_activity: ReceiverActivityState,
}

impl std::fmt::Debug for SessionRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `SessionDescription` does not implement `Debug`; elide it and
        // print the rest of the cross-controller scratch state.
        f.debug_struct("SessionRuntime")
            .field("session_start_prev_blocks", &self.session_start_prev_blocks)
            .field("stop_flag", &self.stop_flag)
            .field("slots", &self.slots)
            .field("scheduling", &self.scheduling)
            .field("next_awake_time", &self.get_next_awake_time())
            .field("receiver_activity", &self.receiver_activity)
            .finish_non_exhaustive()
    }
}

// ======================================================================
// SessionRuntime — accessor surface
// ======================================================================
impl SessionRuntime {
    /* Construction */

    /// Construct a fresh runtime.
    ///
    /// * `now` seeds the initial wake horizon so the very first
    ///   `check_all()` runs immediately.
    /// * `num_validators` sizes the receiver-activity mirror.
    /// * `description`, `session_start_prev_blocks`, `stop_flag` are the
    ///   immutable bootstrap handles previously owned by
    ///   `SessionProcessor` and now reached through the accessor methods
    ///   below.
    pub(crate) fn new(
        now: SystemTime,
        num_validators: usize,
        description: Arc<SessionDescription>,
        session_start_prev_blocks: Vec<BlockIdExt>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            description,
            session_start_prev_blocks,
            stop_flag,
            slots: SlotMap::default(),
            scheduling: DelayedActionQueue::default(),
            next_awake_time: AtomicU64::new(wake_to_nanos(now)),
            receiver_activity: ReceiverActivityState::new(num_validators),
        }
    }

    /* Immutable bootstrap handles — accessors */

    /// Borrow the `Arc<SessionDescription>` so callers can either deref to
    /// `&SessionDescription` for the common method calls or `.clone()` to
    /// pass ownership into spawned tasks/closures.
    #[inline]
    pub(crate) fn description(&self) -> &Arc<SessionDescription> {
        &self.description
    }

    /// Session-start parent block IDs supplied by `ValidatorGroup`.
    #[inline]
    pub(crate) fn session_start_prev_blocks(&self) -> &[BlockIdExt] {
        &self.session_start_prev_blocks
    }

    /* Slot operations — passthroughs to the internal `SlotMap`, so call sites
    spell per-slot ops as `self.runtime.<op>(slot, ...)` instead of leaking
    the inner `slots` field. */

    /// `true` iff the slot has local `generated=true`.
    #[inline]
    pub(crate) fn is_generated(&self, slot: SlotIndex) -> bool {
        self.slots.is_generated(slot)
    }

    /// `true` iff the slot has a pending generation request.
    #[inline]
    pub(crate) fn is_pending_generate(&self, slot: SlotIndex) -> bool {
        self.slots.is_pending_generate(slot)
    }

    /// `true` iff the slot has `sent_generated=true`.
    #[inline]
    pub(crate) fn is_sent_generated(&self, slot: SlotIndex) -> bool {
        self.slots.is_sent_generated(slot)
    }

    /// Returns the slot's start time, falling back to `fallback_now` if the
    /// slot has no runtime yet.
    #[inline]
    pub(crate) fn started_at(&self, slot: SlotIndex, fallback_now: SystemTime) -> SystemTime {
        self.slots.started_at(slot, fallback_now)
    }

    /// `true` iff the first candidate for this slot has been received.
    #[inline]
    pub(crate) fn first_candidate_received(&self, slot: SlotIndex) -> bool {
        self.slots.first_candidate_received(slot)
    }

    /// `true` iff the first candidate for this slot has been notarized.
    #[inline]
    pub(crate) fn first_candidate_notarized(&self, slot: SlotIndex) -> bool {
        self.slots.first_candidate_notarized(slot)
    }

    /// `true` iff the first candidate for this slot has been finalized.
    #[inline]
    pub(crate) fn first_candidate_finalized(&self, slot: SlotIndex) -> bool {
        self.slots.first_candidate_finalized(slot)
    }

    /// Set the `pending_generate` flag for `slot`.
    #[inline]
    pub(crate) fn set_pending_generate(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.slots.set_pending_generate(slot, value, now);
    }

    /// Set the `generated` flag for `slot`.
    #[inline]
    pub(crate) fn set_generated(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.slots.set_generated(slot, value, now);
    }

    /// Set the `sent_generated` flag for `slot`.
    #[inline]
    pub(crate) fn set_sent_generated(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.slots.set_sent_generated(slot, value, now);
    }

    /// Set the `first_candidate_received` flag for `slot`.
    #[inline]
    pub(crate) fn set_first_candidate_received(
        &mut self,
        slot: SlotIndex,
        value: bool,
        now: SystemTime,
    ) {
        self.slots.set_first_candidate_received(slot, value, now);
    }

    /// Set the `first_candidate_notarized` flag for `slot`.
    #[inline]
    pub(crate) fn set_first_candidate_notarized(
        &mut self,
        slot: SlotIndex,
        value: bool,
        now: SystemTime,
    ) {
        self.slots.set_first_candidate_notarized(slot, value, now);
    }

    /// Set the `first_candidate_finalized` flag for `slot`.
    #[inline]
    pub(crate) fn set_first_candidate_finalized(
        &mut self,
        slot: SlotIndex,
        value: bool,
        now: SystemTime,
    ) {
        self.slots.set_first_candidate_finalized(slot, value, now);
    }

    /// Clear `SlotRuntime` for every slot strictly below `up_to_slot`.
    /// Preserves the `SlotEntry` shell for outcome emission.
    #[inline]
    pub(crate) fn clear_runtimes_below(&mut self, up_to_slot: SlotIndex) {
        self.slots.clear_runtimes_below(up_to_slot);
    }

    /* Delayed actions — passthroughs to the internal `DelayedActionQueue`. */

    /// Schedule `handler` to run when `now() >= expiration_time` during a
    /// future `check_all()` pass. Callers must also update
    /// `next_awake_time` (handled by the corresponding orchestrator wrapper
    /// on `SessionProcessor`).
    #[inline]
    pub(crate) fn post_delayed_action(&mut self, expiration_time: SystemTime, handler: TaskPtr) {
        self.scheduling.push(expiration_time, handler);
    }

    /// Pop the first delayed action with `expiration_time <= now`, if any.
    ///
    /// Returns `None` once no due actions remain. Re-entrant: handlers may
    /// invoke `post_delayed_action` during execution and any newly-due
    /// entries will be observed by subsequent `drain_due_delayed_action`
    /// calls in the same drain loop.
    #[inline]
    pub(crate) fn drain_due_delayed_action(&mut self, now: SystemTime) -> Option<DelayedAction> {
        self.scheduling.drain_due_one(now)
    }

    /// Earliest pending `expiration_time` across not-yet-due delayed actions.
    /// Used to drive `next_awake_time` after the drain loop finishes.
    #[inline]
    pub(crate) fn min_pending_delayed_expiration(&self) -> Option<SystemTime> {
        self.scheduling.min_pending_expiration()
    }

    /* Next wake horizon — `next_awake_time` is the monotone-lowered wake horizon
    consulted by the main loop in `session.rs`. Each `check_all()` pass resets
    it to `now + MAX_AWAKE_TIMEOUT`, then controllers lower it via
    `set_next_awake_time` to the earliest pending deadline (delayed actions,
    FSM timeouts, async DB polls, slot starts, validation gates, ...). */

    /// Current wake horizon.
    #[inline]
    pub(crate) fn get_next_awake_time(&self) -> SystemTime {
        wake_from_nanos(self.next_awake_time.load(Ordering::Relaxed))
    }

    /// Lower the wake horizon to `time` if it is earlier than the current
    /// value. No-op otherwise. This is the primary setter used by
    /// controllers to register a pending deadline.
    ///
    /// `&self` via `fetch_min`: the monotone nanosecond encoding means the
    /// atomic minimum is exactly the earliest pending deadline, so this never
    /// needs a `&mut` borrow of the runtime.
    #[inline]
    pub(crate) fn set_next_awake_time(&self, time: SystemTime) {
        self.next_awake_time.fetch_min(wake_to_nanos(time), Ordering::Relaxed);
    }

    /// Reset the wake horizon to the fallback poll horizon
    /// (`now + MAX_AWAKE_TIMEOUT`). Called at the start of each
    /// `check_all()` pass before controllers re-collect their deadlines.
    #[inline]
    pub(crate) fn reset_next_awake_time(&self, now: SystemTime) {
        self.next_awake_time.store(wake_to_nanos(now + MAX_AWAKE_TIMEOUT), Ordering::Relaxed);
    }

    /* Receiver activity — passthroughs to the internal `ReceiverActivityState`,
    mirroring the receiver-side aggregate figures (active weight + per-
    validator last-activity timestamps) refreshed from
    `SessionProcessor::on_activity`. The receiver writes; SessionTelemetry
    snapshot builders read. */

    /// Current active weight reported by the receiver.
    #[inline]
    pub(crate) fn active_weight(&self) -> ValidatorWeight {
        self.receiver_activity.active_weight()
    }

    /// Per-validator last-activity timestamps, indexed by `ValidatorIndex`.
    /// Length matches `num_validators` passed at construction.
    #[inline]
    pub(crate) fn last_activity(&self) -> &[Option<SystemTime>] {
        self.receiver_activity.last_activity()
    }

    /// Refresh both `active_weight` and `last_activity` in one call.
    /// Returns `true` if `active_weight` differs from the previous value,
    /// so callers can gate change-only telemetry side-effects (gauge update,
    /// debug log).
    #[inline]
    pub(crate) fn record_activity(
        &mut self,
        active_weight: ValidatorWeight,
        last_activity: Vec<Option<SystemTime>>,
    ) -> bool {
        self.receiver_activity.record(active_weight, last_activity)
    }
}

// ======================================================================
// SlotMap — per-slot scratch state (`SlotEntry` / `SlotRuntime`)
// ======================================================================
// Owns the slot-aware accessor surface (getters/setters + lifetime helper).
// Setters take `now: SystemTime` so the runtime can be created lazily without
// `SlotMap` owning a clock reference — call sites supply the timestamp from
// `SessionProcessor::now()`.

/// Per-slot state map.
///
/// Newtype around `BTreeMap<SlotIndex, SlotEntry>`. Internal storage is
/// module-private; cross-module callers (including inline `#[path]`-included
/// tests of `SessionProcessor`) interact via the `pub(crate)` methods
/// declared below.
#[derive(Debug, Default)]
pub(crate) struct SlotMap {
    entries: BTreeMap<SlotIndex, SlotEntry>,
}

impl SlotMap {
    /// Returns the slot entry if it exists.
    #[inline]
    fn get(&self, slot: &SlotIndex) -> Option<&SlotEntry> {
        self.entries.get(slot)
    }

    /// Returns a mutable `SlotRuntime`, creating the entry and the runtime
    /// (with `now` as start time) if missing.
    #[inline]
    fn runtime_mut(&mut self, slot: SlotIndex, now: SystemTime) -> &mut SlotRuntime {
        self.entries.entry(slot).or_default().runtime.get_or_insert_with(|| SlotRuntime::new(now))
    }

    /* Slot stage getters (read-only checks; no allocation). */

    /// `true` iff the slot has local `generated=true`.
    #[inline]
    fn is_generated(&self, slot: SlotIndex) -> bool {
        self.get(&slot).map_or(false, |e| e.is_generated())
    }

    /// `true` iff the slot has a pending generation request.
    #[inline]
    fn is_pending_generate(&self, slot: SlotIndex) -> bool {
        self.get(&slot).and_then(|e| e.runtime.as_ref()).map_or(false, |rt| rt.pending_generate)
    }

    /// `true` iff the slot has `sent_generated=true`.
    #[inline]
    fn is_sent_generated(&self, slot: SlotIndex) -> bool {
        self.get(&slot).and_then(|e| e.runtime.as_ref()).map_or(false, |rt| rt.sent_generated)
    }

    /// Returns the slot's start time, falling back to `fallback_now` if the
    /// slot has no runtime yet (slot not locally activated).
    #[inline]
    fn started_at(&self, slot: SlotIndex, fallback_now: SystemTime) -> SystemTime {
        self.get(&slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(fallback_now, |rt| rt.slot_started_at)
    }

    /// `true` iff the first candidate for this slot has been received.
    #[inline]
    fn first_candidate_received(&self, slot: SlotIndex) -> bool {
        self.get(&slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_received)
    }

    /// `true` iff the first candidate for this slot has been notarized.
    #[inline]
    fn first_candidate_notarized(&self, slot: SlotIndex) -> bool {
        self.get(&slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_notarized)
    }

    /// `true` iff the first candidate for this slot has been finalized.
    #[inline]
    fn first_candidate_finalized(&self, slot: SlotIndex) -> bool {
        self.get(&slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_finalized)
    }

    /* Slot stage setters (each takes `now` so the runtime can be created lazily
    without `SlotMap` owning a clock reference). */

    /// Set the `pending_generate` flag for `slot`.
    #[inline]
    fn set_pending_generate(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).pending_generate = value;
    }

    /// Set the `generated` flag for `slot`.
    #[inline]
    fn set_generated(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).generated = value;
    }

    /// Set the `sent_generated` flag for `slot`.
    #[inline]
    fn set_sent_generated(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).sent_generated = value;
    }

    /// Set the `first_candidate_received` flag for `slot`.
    #[inline]
    fn set_first_candidate_received(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).first_candidate_received = value;
    }

    /// Set the `first_candidate_notarized` flag for `slot`.
    #[inline]
    fn set_first_candidate_notarized(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).first_candidate_notarized = value;
    }

    /// Set the `first_candidate_finalized` flag for `slot`.
    #[inline]
    fn set_first_candidate_finalized(&mut self, slot: SlotIndex, value: bool, now: SystemTime) {
        self.runtime_mut(slot, now).first_candidate_finalized = value;
    }

    /* Lifetime / cleanup helpers. */

    /// Clear `SlotRuntime` for every slot strictly below `up_to_slot`.
    ///
    /// Preserves the `SlotEntry` shell for outcome emission. Replaces the
    /// inline loop previously embedded in `cleanup_old_candidates`.
    fn clear_runtimes_below(&mut self, up_to_slot: SlotIndex) {
        for slot_idx in 0..up_to_slot.value() {
            let slot = SlotIndex::new(slot_idx);
            if let Some(entry) = self.entries.get_mut(&slot) {
                entry.runtime = None;
            }
        }
    }
}

/// Slot entry — outer container that may or may not have a live `SlotRuntime`.
///
/// A `SlotEntry` without `runtime` represents a slot that the FSM
/// acknowledges (e.g., for outcome emission) but for which no per-slot
/// scratch state has been activated. The runtime is reset to `None` by
/// [`SlotMap::clear_runtimes_below`] once the slot ages past
/// `MAX_HISTORY_SLOTS`.
#[derive(Debug, Default)]
pub(crate) struct SlotEntry {
    /// Per-slot mutable runtime — `None` for non-locally-activated or aged-out slots.
    pub(crate) runtime: Option<SlotRuntime>,
}

impl SlotEntry {
    /// `true` iff the slot has `runtime.generated == true`.
    #[inline]
    fn is_generated(&self) -> bool {
        self.runtime.as_ref().map_or(false, |rt| rt.generated)
    }
}

/// Per-slot runtime state (optional: exists only if the slot had local activity).
///
/// Contains per-slot state for collation, timing, and stage tracking.
#[derive(Debug)]
pub(crate) struct SlotRuntime {
    // Collation state
    pub(crate) slot_started_at: SystemTime,
    pub(crate) pending_generate: bool,
    pub(crate) generated: bool,
    pub(crate) sent_generated: bool,

    // Slot stage flags (for latency metrics)
    pub(crate) first_candidate_received: bool,
    pub(crate) first_candidate_notarized: bool,
    pub(crate) first_candidate_finalized: bool,
}

impl SlotRuntime {
    #[inline]
    fn new(now: SystemTime) -> Self {
        Self {
            slot_started_at: now,
            pending_generate: false,
            generated: false,
            sent_generated: false,
            first_candidate_received: false,
            first_candidate_notarized: false,
            first_candidate_finalized: false,
        }
    }
}

// ======================================================================
// DelayedActionQueue — append-ordered buffer of scheduled actions
// ======================================================================
// Entries are scheduled via `SessionRuntime::post_delayed_action` and drained
// from `check_all()` via `SessionRuntime::drain_due_delayed_action`. The wrapper
// `SessionProcessor::process_delayed_actions` runs the handlers against
// `&mut SessionProcessor` — the re-entrancy boundary. `DelayedAction` (below) is
// the per-entry record.

/// Delayed-action queue.
///
/// Holds pending [`DelayedAction`] entries appended in insertion order
/// (`push` appends to the tail). `drain_due_one` performs a linear scan and
/// uses `swap_remove` to dequeue, matching the historical
/// `process_delayed_actions` semantics: handlers that re-post during
/// execution land at the tail and are observed by subsequent drain calls
/// in the same loop.
///
/// Because `swap_remove` moves the last entry into the drained slot, actions
/// that are simultaneously due are not necessarily drained in insertion
/// order. This is intentional — it preserves the pre-refactor
/// `process_delayed_actions` behavior — so callers must not rely on strict
/// FIFO ordering across due actions.
#[derive(Debug, Default)]
pub(crate) struct DelayedActionQueue {
    actions: Vec<DelayedAction>,
}

impl DelayedActionQueue {
    /// Append a new delayed action to the queue tail.
    #[inline]
    fn push(&mut self, expiration_time: SystemTime, handler: TaskPtr) {
        self.actions.push(DelayedAction { expiration_time, handler });
    }

    /// Find the first action with `expiration_time <= now` and remove it
    /// via `swap_remove` (matches the original drain semantics so that a
    /// handler re-posting via `push` lands at the tail and the new entry
    /// is observed by subsequent drain calls).
    #[inline]
    fn drain_due_one(&mut self, now: SystemTime) -> Option<DelayedAction> {
        let idx = self.actions.iter().position(|a| a.expiration_time <= now)?;
        Some(self.actions.swap_remove(idx))
    }

    /// Smallest `expiration_time` across all pending actions.
    #[inline]
    fn min_pending_expiration(&self) -> Option<SystemTime> {
        self.actions.iter().map(|a| a.expiration_time).min()
    }
}

/// Single delayed-action entry.
///
/// Owned by [`DelayedActionQueue`]; produced by
/// `SessionRuntime::drain_due_delayed_action` so that
/// `SessionProcessor::process_delayed_actions` can invoke the boxed
/// handler against `&mut SessionProcessor`.
pub(crate) struct DelayedAction {
    /// Time when the handler is ready to run.
    pub(crate) expiration_time: SystemTime,
    /// Boxed handler; invoked once and dropped.
    pub(crate) handler: TaskPtr,
}

impl std::fmt::Debug for DelayedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelayedAction")
            .field("expiration_time", &self.expiration_time)
            .field("handler", &"<closure>")
            .finish()
    }
}

// ======================================================================
// ReceiverActivityState — receiver-side activity mirror
// ======================================================================
// Mirror of the receiver-side aggregate figures (active weight + per-validator
// last-activity timestamps) refreshed via `SessionProcessor::on_activity`. The
// receiver thread is the sole writer; `SessionTelemetry` snapshot builders read.

/// Receiver-activity mirror.
#[derive(Debug)]
pub(crate) struct ReceiverActivityState {
    /// Latest active weight reported by the receiver.
    active_weight: ValidatorWeight,
    /// Per-validator last-activity timestamps (indexed by `ValidatorIndex`).
    last_activity: Vec<Option<SystemTime>>,
}

impl ReceiverActivityState {
    /// Initialise with zero active weight and `num_validators` `None`
    /// last-activity slots.
    #[inline]
    fn new(num_validators: usize) -> Self {
        Self { active_weight: 0, last_activity: vec![None; num_validators] }
    }

    /// Active weight (read-only).
    #[inline]
    fn active_weight(&self) -> ValidatorWeight {
        self.active_weight
    }

    /// Per-validator last-activity timestamps (read-only).
    #[inline]
    fn last_activity(&self) -> &[Option<SystemTime>] {
        &self.last_activity
    }

    /// Refresh both fields. Returns `true` if `active_weight` differs from
    /// the previous value so the caller can gate change-only side effects
    /// (telemetry gauge update, debug log).
    #[inline]
    fn record(
        &mut self,
        active_weight: ValidatorWeight,
        last_activity: Vec<Option<SystemTime>>,
    ) -> bool {
        let changed = self.active_weight != active_weight;
        self.active_weight = active_weight;
        self.last_activity = last_activity;
        changed
    }
}

// ======================================================================
// Tests
// ======================================================================
// Test-only seeders / inspectors for `SessionRuntime` and its inner types are
// consolidated here (one `#[cfg(test)] impl` per type) so the production impls
// above carry no test scaffolding. The unit tests themselves live in a sibling
// file included directly via `#[path]` so they reach the `pub(crate)` accessor
// surface and inner structs without widening visibility. Mirrors
// `session_telemetry.rs` / `simplex_state.rs`.
#[cfg(test)]
impl SessionRuntime {
    /// Stop flag shared with the `SimplexSession` main loop.
    fn stop_flag(&self) -> &Arc<AtomicBool> {
        &self.stop_flag
    }

    /// Unconditionally assign `active_weight`, bypassing the change-detecting
    /// behavior of [`record_activity`], to seed a value before exercising
    /// snapshot builders.
    pub(crate) fn force_active_weight(&mut self, active_weight: ValidatorWeight) {
        self.receiver_activity.force_active_weight(active_weight);
    }

    /// Returns the slot entry if it exists.
    pub(crate) fn slot_entry(&self, slot: SlotIndex) -> Option<&SlotEntry> {
        self.slots.get(&slot)
    }

    /// Returns a mutable `SlotRuntime`, creating the entry and the runtime
    /// (with `now` as start time) if missing.
    pub(crate) fn slot_runtime_mut(
        &mut self,
        slot: SlotIndex,
        now: SystemTime,
    ) -> &mut SlotRuntime {
        self.slots.runtime_mut(slot, now)
    }

    /// Number of currently-queued delayed actions (both due and pending).
    pub(crate) fn delayed_actions_count(&self) -> usize {
        self.scheduling.len()
    }

    /// Unconditionally assign the wake horizon to `time`, bypassing the
    /// pick-the-min behavior of [`set_next_awake_time`], to seed a future
    /// horizon before exercising operations that should lower it.
    pub(crate) fn force_next_awake_time(&self, time: SystemTime) {
        self.next_awake_time.store(wake_to_nanos(time), Ordering::Relaxed);
    }
}

#[cfg(test)]
impl DelayedActionQueue {
    /// Number of queued actions (both due and pending).
    fn len(&self) -> usize {
        self.actions.len()
    }
}

#[cfg(test)]
impl ReceiverActivityState {
    /// Test-only unconditional setter for `active_weight`.
    fn force_active_weight(&mut self, active_weight: ValidatorWeight) {
        self.active_weight = active_weight;
    }
}

#[cfg(test)]
#[path = "tests/test_session_runtime.rs"]
mod tests;
