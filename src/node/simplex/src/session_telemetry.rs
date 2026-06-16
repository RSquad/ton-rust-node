/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `SessionTelemetry` aspect entity
//!
//! Owns all per-session observability state: ~50 metric handles (counters,
//! histograms, gauges), the stall-tracking cursors (formerly an inner
//! private struct), the health-alert dedup baseline, error counters, and the
//! latest receiver activity snapshot. Held by `SessionProcessor` as
//! `self.telemetry`.
//!
//! ## Boundary
//!
//! `SessionTelemetry` is an isolated downstream aspect entity. It must not
//! import, name, or receive `SessionProcessor`, and it must not depend on
//! private `session_processor.rs` types such as `ReceivedCandidate`,
//! `PendingValidation`, `FinalizedEntry`, `SlotEntry`, `SlotRuntime`, or
//! `PrecollatedBlockMap`. It may depend on `SessionDescription` because that
//! is a stable session-level domain object, not a `SessionProcessor`
//! implementation detail.
//!
//! It owns the self-collation observability funnel end-to-end: the in-flight
//! start / pending-acceptance maps and the `GeneratedCandidateValidationWatch`
//! registry (all interior-mutable behind `spin::Mutex`), plus the lifecycle
//! recorders that mutate them and emit the COLLATION_FLOW / missed-validation
//! logs and counters. The recorders are clockless — callers thread
//! `now: SystemTime` and `&SessionDescription` in from `SessionProcessor`.
//!
//! ## History
//!
//! Extracted from `SessionProcessor` under the consensus-controller split
//! (TN-998 / TN-1342): all recorder, health-check, and dump logic now lives
//! here, and `SessionProcessor` builds neutral snapshots from its private maps
//! and delegates through `self.telemetry.X`. See the architecture artifact:
//! `docs/local-docs/features/simplex-consensus/architecture/simplex-architecture-rework-plan.md`.

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex},
    receiver::ReceiverHealthCounters,
    session_description::SessionDescription,
    simplex_state::WindowDiagnostic,
    utils::{threshold_33, threshold_66},
    MetricsHandle, ValidatorWeight,
};
use consensus_common::profiling::ResultStatusCounter;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use ton_block::{base64_encode, BlockIdExt, UInt256};

// ======================================================================
// Telemetry support types
// ======================================================================
// Small value types backing the recorders below — the generated-candidate
// watch, structured health findings, the candidate funnel totals, the
// health-alert dedup baseline, and the stall-progress cursors. Each carries
// its own inherent impl (where any) and is defined ahead of `SessionTelemetry`.

/// Tracks a locally generated candidate until it validates successfully.
///
/// Held in [`SessionTelemetry`]'s `generated_candidates_waiting_validation`
/// registry behind a `spin::Mutex`. Surfaces metrics / warnings when our own
/// candidate is dropped before a successful validation outcome. Private to
/// this module: the watch lifecycle is driven entirely through the
/// `track_` / `mark_` / `note_` recorder methods below.
#[derive(Debug)]
struct GeneratedCandidateValidationWatch {
    /// When the local candidate was generated.
    generated_at: SystemTime,
    /// Whether it has already entered higher-layer validation.
    validation_started: bool,
}

/// Health check finding kind for structured stall diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HealthFindingKind {
    ZeroFinalizationSpeed,
    ProgressGap,
    LowActivity,
    StandstillTriggers,
    CandidateGiveups,
    SkipVoteDominance,
    ValidatorIsolated,
    CertVerifyFailures,
}

/// Single health check finding with severity and human-readable summary.
#[derive(Debug, Clone)]
pub(crate) struct HealthFinding {
    pub(crate) kind: HealthFindingKind,
    pub(crate) severity: log::Level,
    pub(crate) summary: String,
}

/// Candidate funnel totals for validation inventory.
pub(crate) struct CandidateTotals {
    pub(crate) received_total: usize,
    pub(crate) received_unvalidated: usize,
    pub(crate) validated_not_notarized: usize,
    pub(crate) notarized_not_finalized: usize,
    pub(crate) finalized_recent: usize,
    pub(crate) other_omitted: usize,
}

impl CandidateTotals {
    fn pct(&self, value: usize) -> f64 {
        if self.received_total == 0 {
            0.0
        } else {
            100.0 * value as f64 / self.received_total as f64
        }
    }
}

/// Tracks per-anomaly cooldowns and delta baselines for health alert deduplication.
/// All timestamps use `SystemTime` (via `self.now()`) for deterministic testing.
pub(crate) struct HealthAlertState {
    pub(crate) last_activity_warn: SystemTime,
    pub(crate) last_candidate_giveup_warn: SystemTime,
    pub(crate) last_cert_fail_warn: SystemTime,
    pub(crate) last_finalization_speed_warn: SystemTime,
    pub(crate) last_finalization_nonzero_at: SystemTime,
    pub(crate) last_progress_warn: SystemTime,
    pub(crate) last_skip_ratio_warn: SystemTime,
    pub(crate) last_standstill_warn: SystemTime,
    pub(crate) last_isolation_warn: SystemTime,
    pub(crate) prev_candidate_giveups: u64,
    pub(crate) prev_cert_verify_fails: u64,
    pub(crate) prev_last_finalized_slot: f64,
    pub(crate) prev_votes_in_notarize: u64,
    pub(crate) prev_votes_in_finalize: u64,
    pub(crate) prev_votes_in_skip: u64,
    pub(crate) prev_standstill_triggers: u64,
    pub(crate) cooldown: Duration,
}

impl HealthAlertState {
    fn new(now: SystemTime, cooldown: Duration) -> Self {
        // Prime warning timestamps in the past so first anomaly can be emitted immediately.
        let warn_base = now.checked_sub(cooldown).unwrap_or(SystemTime::UNIX_EPOCH);
        Self {
            last_activity_warn: warn_base,
            last_candidate_giveup_warn: warn_base,
            last_cert_fail_warn: warn_base,
            last_finalization_speed_warn: warn_base,
            last_finalization_nonzero_at: now,
            last_progress_warn: warn_base,
            last_skip_ratio_warn: warn_base,
            last_standstill_warn: warn_base,
            last_isolation_warn: warn_base,
            prev_candidate_giveups: 0,
            prev_cert_verify_fails: 0,
            prev_last_finalized_slot: 0.0,
            prev_votes_in_notarize: 0,
            prev_votes_in_finalize: 0,
            prev_votes_in_skip: 0,
            prev_standstill_triggers: 0,
            cooldown,
        }
    }
}

/// Stall-tracking cursors: frontier-change timestamps and consensus
/// milestone times consumed by the stall/debug dump to report how long
/// since each frontier moved and when the last notarization / cert / MC
/// apply happened.
///
/// Held inside [`SessionTelemetry`] behind a `spin::Mutex` so it can be
/// updated through `&self`. The mutex is a private field; all access goes
/// through the batch methods on `SessionTelemetry`, each of which takes the
/// lock exactly once, so locking is fully owned by `SessionTelemetry` and
/// the critical sections stay minimal. `Clone` exists so the dump (and
/// tests) can take a one-shot snapshot under a single lock.
#[derive(Debug, Clone)]
pub(crate) struct StallCursors {
    /// First-non-finalized frontier observed at the last `note_frontier_progress`.
    pub(crate) prev_first_non_finalized: SlotIndex,
    /// First-non-progressed frontier observed at the last `note_frontier_progress`.
    pub(crate) prev_first_non_progressed: SlotIndex,
    /// Wall-clock time the finalized frontier last advanced.
    pub(crate) last_finalized_cursor_change_at: SystemTime,
    /// Wall-clock time the progressed frontier last advanced.
    pub(crate) last_progression_change_at: SystemTime,
    pub(crate) last_notarization_at: Option<SystemTime>,
    pub(crate) last_notarization_slot: Option<SlotIndex>,
    pub(crate) last_notar_cert_at: Option<SystemTime>,
    pub(crate) last_notar_cert_slot: Option<SlotIndex>,
    pub(crate) last_final_cert_at: Option<SystemTime>,
    pub(crate) last_final_cert_slot: Option<SlotIndex>,
    pub(crate) last_mc_applied_block_id: Option<BlockIdExt>,
}

impl StallCursors {
    /// Seed the cursors at session start: frontiers at slot zero, change
    /// times at `now`, all milestones unset.
    fn new(now: SystemTime) -> Self {
        Self {
            prev_first_non_finalized: SlotIndex(0),
            prev_first_non_progressed: SlotIndex(0),
            last_finalized_cursor_change_at: now,
            last_progression_change_at: now,
            last_notarization_at: None,
            last_notarization_slot: None,
            last_notar_cert_at: None,
            last_notar_cert_slot: None,
            last_final_cert_at: None,
            last_final_cert_slot: None,
            last_mc_applied_block_id: None,
        }
    }
}

// ======================================================================
// SessionTelemetry — state & construction
// ======================================================================
// The aspect struct (interior-mutable observability state behind `spin::Mutex`)
// and its `new` constructor, which registers every metric handle and primes the
// health-alert dedup and stall-cursor baselines.

/// Aspect entity owning all per-session observability state.
///
/// Held by `SessionProcessor` as `self.telemetry`. Owns the metric handles,
/// stall-tracking cursors, health-alert dedup baseline, self-collation
/// observability funnel, and receiver activity snapshot; all interior-mutable
/// state sits behind `spin::Mutex` so recorders can run through `&self`.
pub(crate) struct SessionTelemetry {
    /* Stall-tracking cursors — frontier-change timestamps and consensus
    milestone times used by the stall/debug dump. Private and guarded by a
    `spin::Mutex` so they can be updated through `&self`; every mutation goes
    through a batch method (`note_frontier_progress`,
    `record_notarization_milestone`, `record_final_cert_milestone`,
    `note_mc_applied_top`) that takes the lock exactly once, and reads go through
    `stall_cursors_snapshot`. */
    stall_cursors: spin::Mutex<StallCursors>,

    /* Metric handles (counters, gauges, histograms) — metrics receiver retained
    for late-bound registration (used by `SessionImpl::create_metrics_dumper`)
    and integration tests. */
    pub(crate) metrics_receiver: MetricsHandle,

    pub(crate) check_all_counter: metrics::Counter,
    pub(crate) process_events_counter: metrics::Counter,
    pub(crate) slot_duration_histogram: metrics::Histogram,
    pub(crate) validation_latency_histogram: metrics::Histogram,
    pub(crate) collation_latency_histogram: metrics::Histogram,
    pub(crate) check_all_wake_slip_histogram: metrics::Histogram,
    pub(crate) active_weight_gauge: metrics::Gauge,
    pub(crate) validates_counter: ResultStatusCounter,
    pub(crate) collates_counter: ResultStatusCounter,
    pub(crate) self_collates_counter: ResultStatusCounter,
    pub(crate) precollation_requests_counter: metrics::Counter,
    pub(crate) precollation_results_counter: metrics::Counter,
    pub(crate) collates_precollated_counter: ResultStatusCounter,
    pub(crate) collates_expire_counter: ResultStatusCounter,
    pub(crate) collation_starts_counter: metrics::Counter,
    pub(crate) self_collation_accept_latency_histogram: metrics::Histogram,
    pub(crate) broadcast_validation_latency_histogram: metrics::Histogram,
    pub(crate) errors_counter: metrics::Counter,
    pub(crate) finalized_pending_body_gauge: metrics::Gauge,
    pub(crate) async_db_pending_count_gauge: metrics::Gauge,
    pub(crate) async_db_timeout_counter: metrics::Counter,
    pub(crate) async_db_completion_latency_histogram: metrics::Histogram,
    pub(crate) first_candidate_received_latency_histogram: metrics::Histogram,
    pub(crate) first_candidate_notarized_latency_histogram: metrics::Histogram,
    pub(crate) first_candidate_finalized_latency_histogram: metrics::Histogram,
    pub(crate) misbehavior_counter: metrics::Counter,
    pub(crate) last_finalized_slot_gauge: metrics::Gauge,
    pub(crate) first_non_finalized_slot_gauge: metrics::Gauge,
    pub(crate) first_non_progressed_slot_gauge: metrics::Gauge,
    pub(crate) skip_total_counter: metrics::Counter,
    pub(crate) votes_in_total_counter: metrics::Counter,
    pub(crate) votes_in_notarize_counter: metrics::Counter,
    pub(crate) votes_in_finalize_counter: metrics::Counter,
    pub(crate) votes_in_skip_counter: metrics::Counter,
    pub(crate) votes_out_total_counter: metrics::Counter,
    pub(crate) votes_out_notarize_counter: metrics::Counter,
    pub(crate) votes_out_finalize_counter: metrics::Counter,
    pub(crate) votes_out_skip_counter: metrics::Counter,
    pub(crate) votes_out_persist_fail_counter: metrics::Counter,
    pub(crate) certs_in_counter: metrics::Counter,
    pub(crate) certs_relayed_counter: metrics::Counter,
    pub(crate) cert_conflict_counter: metrics::Counter,
    pub(crate) cert_verify_fail_counter: metrics::Counter,
    pub(crate) validation_reject_counter: metrics::Counter,
    pub(crate) validation_late_callback_counter: metrics::Counter,
    pub(crate) health_warnings_counter: metrics::Counter,
    pub(crate) candidate_precheck_old_slot_drop_counter: metrics::Counter,
    pub(crate) candidate_precheck_future_slot_drop_counter: metrics::Counter,
    /// Relayed leader-signed broadcasts observed at precheck (delivering peer != slot
    /// leader). Counted pre-verification as a relay-hop delivery-volume diagnostic; the
    /// body is authenticated downstream by the leader signature in `RawCandidate::from_tl`.
    pub(crate) candidate_relayed_broadcast_counter: metrics::Counter,
    pub(crate) candidate_precheck_conflicting_slot_drop_counter: metrics::Counter,
    pub(crate) candidate_received_broadcast_counter: metrics::Counter,
    pub(crate) candidate_received_query_counter: metrics::Counter,
    pub(crate) generated_candidate_validation_missed_counter: metrics::Counter,

    /* Error tracking — `AtomicU32` so `increment_error()` can be called from
    `&self` paths. */
    pub(crate) session_errors_count: AtomicU32,

    /* Health-alert dedup state — cooldowns and per-anomaly delta baselines for
    cooldown-driven health anomaly detection. Vote-stream totals and cert-verify
    totals are kept here so the deltas live next to the alert state that consumes
    them. */
    // Guarded by a `spin::Mutex` so the `&self` `run_health_checks` and
    // `collect_health_findings` paths can read and update cooldown baselines.
    // Private: the only accessors are those two methods (plus a `#[cfg(test)]`
    // seam), each of which locks exactly once, so locking stays fully owned by
    // `SessionTelemetry`.
    health_alert_state: spin::Mutex<HealthAlertState>,
    pub(crate) receiver_health_counters: Arc<ReceiverHealthCounters>,
    // Vote-stream / cert-verify lifetime totals. `AtomicU64` (mirroring
    // `session_errors_count`) so the ingress paths bump them through `&self`.
    pub(crate) cert_verify_fails_total: AtomicU64,
    pub(crate) votes_in_notarize_total: AtomicU64,
    pub(crate) votes_in_finalize_total: AtomicU64,
    pub(crate) votes_in_skip_total: AtomicU64,

    /* Stall debug / round-debug bookkeeping — trigger time for the periodic
    stall debug dump and the wall-clock time of the most recent finalization
    (used both by the dump and by the anomaly windows). Stored as nanoseconds
    since `UNIX_EPOCH` in atomics so the `&self` check_all / finalize paths
    advance them lock-free; read and write them through `round_debug_at()` /
    `last_finalization_time()` and their setters. */
    round_debug_at_nanos: AtomicU64,
    last_finalization_time_nanos: AtomicU64,
    // Vestigial per-slot "missing body already logged" dedup set: today it is
    // only pruned (`forget_missing_body_log` / `prune_missing_body_log_below`),
    // never inserted, but the bookkeeping is preserved. Behind a `spin::Mutex`
    // so those mutations stay `&self`.
    missing_body_logged: spin::Mutex<HashSet<u32>>,

    /* Latest receiver activity snapshot — refreshed via `on_activity` callbacks.
    Surfaces simplex/cert traffic counters in the stall dump. Behind a
    `spin::Mutex` so `set_receiver_snapshot` (driven by the `&self` activity
    path) and the dump reader share it. */
    last_receiver_snapshot: spin::Mutex<Option<crate::receiver::ReceiverActivitySnapshot>>,

    /* Self-collation observability funnel state — the full lifecycle of a
    locally produced block is tracked here so the recorders below can measure
    end-to-end latency and surface dropped candidates. All three maps are
    interior-mutable behind a `spin::Mutex` so every recorder stays `&self`:
    - `self_collation_starts_by_slot` — in-flight self-collations keyed by slot,
      carrying `(started_at, expected_seqno)` for the FIRST `on_generate_slot()`
      dispatch (retries collapse onto the same entry).
    - `self_collation_pending_acceptance` — self-collations whose candidate has
      been generated and is awaiting `notify_block_finalized`, keyed by
      `RawCandidateId`, carrying the original `(started_at, expected_seqno)`.
    - `generated_candidates_waiting_validation` — locally generated candidates
      that have not yet validated successfully. */
    self_collation_starts_by_slot: spin::Mutex<HashMap<SlotIndex, (SystemTime, u32)>>,
    self_collation_pending_acceptance: spin::Mutex<HashMap<RawCandidateId, (SystemTime, u32)>>,
    generated_candidates_waiting_validation:
        spin::Mutex<HashMap<RawCandidateId, GeneratedCandidateValidationWatch>>,
}

impl SessionTelemetry {
    /// Build the telemetry aspect for a new session.
    ///
    /// Registers all metric handles against `metrics_receiver`, seeds the
    /// startup-error counter from `initial_errors`, primes the health-alert
    /// cooldown state, and initializes the stall cursors to `now`.
    pub(crate) fn new(
        metrics_receiver: MetricsHandle,
        description: &SessionDescription,
        receiver_health_counters: Arc<ReceiverHealthCounters>,
        initial_errors: u32,
        health_alert_cooldown: Duration,
        now: SystemTime,
    ) -> Self {
        let sink = metrics_receiver.sink();

        // Counters
        let check_all_counter = sink.register_counter(&"simplex_check_all_calls".into());
        let process_events_counter = sink.register_counter(&"simplex_process_events_calls".into());

        // Histograms (latency tracking)
        let slot_duration_histogram = sink.register_histogram(&"time:slot_duration".into());
        let validation_latency_histogram =
            sink.register_histogram(&"time:validation_latency".into());
        let collation_latency_histogram = sink.register_histogram(&"time:collation_latency".into());
        let self_collation_accept_latency_histogram =
            sink.register_histogram(&"time:self_collation_accept_latency".into());
        let check_all_wake_slip_histogram =
            sink.register_histogram(&"time:check_all_wake_slip_ms".into());
        let broadcast_validation_latency_histogram =
            sink.register_histogram(&"time:broadcast_validation_latency".into());

        // Slot stage latency histograms (analogous to round stages in validator-session)
        let first_candidate_received_latency_histogram =
            sink.register_histogram(&"time:slot_stage1_received_latency".into());
        let first_candidate_notarized_latency_histogram =
            sink.register_histogram(&"time:slot_stage2_notarized_latency".into());
        let first_candidate_finalized_latency_histogram =
            sink.register_histogram(&"time:slot_stage3_finalized_latency".into());

        // Gauges
        let active_weight_gauge = sink.register_gauge(&"simplex_active_weight".into());
        let total_weight_gauge = sink.register_gauge(&"simplex_total_weight".into());
        let threshold_66_gauge = sink.register_gauge(&"simplex_threshold_66".into());

        // Set initial gauge values
        total_weight_gauge.set(description.get_total_weight() as f64);
        threshold_66_gauge.set(description.get_threshold_66() as f64);

        // Result status counters
        let validates_counter = ResultStatusCounter::new(&metrics_receiver, "simplex_validates");
        let collates_counter = ResultStatusCounter::new(&metrics_receiver, "simplex_collates");
        let self_collates_counter =
            ResultStatusCounter::new(&metrics_receiver, "simplex_self_collates");

        // Precollation metrics
        let precollation_requests_counter =
            sink.register_counter(&"simplex_precollation_requests".into());
        let precollation_results_counter =
            sink.register_counter(&"simplex_precollation_results".into());
        let collates_precollated_counter =
            ResultStatusCounter::new(&metrics_receiver, "simplex_collates_precollated");
        let collates_expire_counter =
            ResultStatusCounter::new(&metrics_receiver, "simplex_collates_expire");
        let collation_starts_counter = sink.register_counter(&"simplex_collation_starts".into());

        // Error tracking for ValidatorSessionStats
        let errors_counter = sink.register_counter(&"simplex_errors".into());

        let misbehavior_counter = sink.register_counter(&"simplex_misbehavior".into());

        let last_finalized_slot_gauge = sink.register_gauge(&"simplex_last_finalized_slot".into());
        let first_non_finalized_slot_gauge =
            sink.register_gauge(&"simplex_first_non_finalized_slot".into());
        let first_non_progressed_slot_gauge =
            sink.register_gauge(&"simplex_first_non_progressed_slot".into());
        let skip_total_counter = sink.register_counter(&"simplex_skip_total".into());

        let votes_in_total_counter = sink.register_counter(&"simplex_votes_in_total".into());
        let votes_in_notarize_counter = sink.register_counter(&"simplex_votes_in_notarize".into());
        let votes_in_finalize_counter = sink.register_counter(&"simplex_votes_in_finalize".into());
        let votes_in_skip_counter = sink.register_counter(&"simplex_votes_in_skip".into());
        let votes_out_total_counter = sink.register_counter(&"simplex_votes_out_total".into());
        let votes_out_notarize_counter =
            sink.register_counter(&"simplex_votes_out_notarize".into());
        let votes_out_finalize_counter =
            sink.register_counter(&"simplex_votes_out_finalize".into());
        let votes_out_skip_counter = sink.register_counter(&"simplex_votes_out_skip".into());
        let votes_out_persist_fail_counter =
            sink.register_counter(&"simplex_votes_out_persist_fail".into());

        let certs_in_counter = sink.register_counter(&"simplex_certs_in".into());
        let certs_relayed_counter = sink.register_counter(&"simplex_certs_relayed".into());
        let cert_conflict_counter = sink.register_counter(&"simplex_cert_conflict".into());
        let cert_verify_fail_counter = sink.register_counter(&"simplex_cert_verify_fail".into());

        let validation_reject_counter = sink.register_counter(&"simplex_validation_reject".into());
        let validation_late_callback_counter =
            sink.register_counter(&"simplex_validation_late_callback".into());

        let health_warnings_counter = sink.register_counter(&"simplex_health_warnings".into());
        let candidate_precheck_old_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_old_slot".into());
        let candidate_precheck_future_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_future_slot".into());
        let candidate_relayed_broadcast_counter =
            sink.register_counter(&"simplex_candidate_relayed_broadcast".into());
        let candidate_precheck_conflicting_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_conflicting_slot".into());
        let candidate_received_broadcast_counter =
            sink.register_counter(&"simplex_candidate_received_broadcast".into());
        let candidate_received_query_counter =
            sink.register_counter(&"simplex_candidate_received_query".into());
        let generated_candidate_validation_missed_counter =
            sink.register_counter(&"simplex_generated_candidate_validation_missed".into());

        let finalized_pending_body_gauge =
            sink.register_gauge(&"simplex_finalized_pending_body_count".into());
        let async_db_pending_count_gauge =
            sink.register_gauge(&"simplex_async_db_pending_count".into());
        let async_db_timeout_counter =
            sink.register_counter(&"simplex_async_db_timeout_total".into());
        let async_db_completion_latency_histogram =
            sink.register_histogram(&"simplex_async_db_completion_latency_ms".into());

        // Seed the startup-errors counter so the metric is consistent with the
        // atomic running count below. Logged by the caller after the processor
        // is built so the session id is available.
        if initial_errors > 0 {
            errors_counter.increment(initial_errors as u64);
        }

        // Round debug period is inlined here to keep this module self-contained;
        // mirror it in session_processor.rs (`ROUND_DEBUG_PERIOD`).
        let round_debug_at = now + ROUND_DEBUG_PERIOD;

        Self {
            // Stall cursors (frontier change-times + consensus milestones),
            // guarded by a spin mutex so they can be updated through `&self`.
            stall_cursors: spin::Mutex::new(StallCursors::new(now)),

            metrics_receiver,
            check_all_counter,
            process_events_counter,
            slot_duration_histogram,
            validation_latency_histogram,
            collation_latency_histogram,
            check_all_wake_slip_histogram,
            active_weight_gauge,
            validates_counter,
            collates_counter,
            self_collates_counter,
            precollation_requests_counter,
            precollation_results_counter,
            collates_precollated_counter,
            collates_expire_counter,
            collation_starts_counter,
            self_collation_accept_latency_histogram,
            broadcast_validation_latency_histogram,
            errors_counter,
            finalized_pending_body_gauge,
            async_db_pending_count_gauge,
            async_db_timeout_counter,
            async_db_completion_latency_histogram,
            first_candidate_received_latency_histogram,
            first_candidate_notarized_latency_histogram,
            first_candidate_finalized_latency_histogram,
            misbehavior_counter,
            last_finalized_slot_gauge,
            first_non_finalized_slot_gauge,
            first_non_progressed_slot_gauge,
            skip_total_counter,
            votes_in_total_counter,
            votes_in_notarize_counter,
            votes_in_finalize_counter,
            votes_in_skip_counter,
            votes_out_total_counter,
            votes_out_notarize_counter,
            votes_out_finalize_counter,
            votes_out_skip_counter,
            votes_out_persist_fail_counter,
            certs_in_counter,
            certs_relayed_counter,
            cert_conflict_counter,
            cert_verify_fail_counter,
            validation_reject_counter,
            validation_late_callback_counter,
            health_warnings_counter,
            candidate_precheck_old_slot_drop_counter,
            candidate_precheck_future_slot_drop_counter,
            candidate_relayed_broadcast_counter,
            candidate_precheck_conflicting_slot_drop_counter,
            candidate_received_broadcast_counter,
            candidate_received_query_counter,
            generated_candidate_validation_missed_counter,

            session_errors_count: AtomicU32::new(initial_errors),

            health_alert_state: spin::Mutex::new(HealthAlertState::new(now, health_alert_cooldown)),
            receiver_health_counters,
            cert_verify_fails_total: AtomicU64::new(0),
            votes_in_notarize_total: AtomicU64::new(0),
            votes_in_finalize_total: AtomicU64::new(0),
            votes_in_skip_total: AtomicU64::new(0),

            round_debug_at_nanos: AtomicU64::new(system_time_to_nanos(round_debug_at)),
            last_finalization_time_nanos: AtomicU64::new(system_time_to_nanos(now)),
            missing_body_logged: spin::Mutex::new(HashSet::new()),

            last_receiver_snapshot: spin::Mutex::new(None),

            self_collation_starts_by_slot: spin::Mutex::new(HashMap::new()),
            self_collation_pending_acceptance: spin::Mutex::new(HashMap::new()),
            generated_candidates_waiting_validation: spin::Mutex::new(HashMap::new()),
        }
    }
}

/// Period without finalizations before triggering debug dump (stalled consensus detection).
///
/// Mirrors the constant of the same name in `session_processor.rs`. Kept local
/// here so `SessionTelemetry::new` is fully self-contained.
const ROUND_DEBUG_PERIOD: Duration = Duration::from_secs(15);

/// Encode a `SystemTime` as nanoseconds since `UNIX_EPOCH`.
///
/// Telemetry timestamps are always wall-clock samples at or after the epoch, so
/// the saturating `unwrap_or(0)` only guards the theoretical pre-epoch case.
/// u64 nanoseconds span ~584 years past 1970, far beyond any session lifetime.
fn system_time_to_nanos(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

/// Inverse of [`system_time_to_nanos`].
fn nanos_to_system_time(nanos: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos)
}

// ======================================================================
// Description-derived formatting helpers
// ======================================================================
//
// Centralised so every `record_self_collation_*` method renders short hashes,
// linkage block ids, and prev-block lists identically. None of the helpers
// store the description; they take it as a parameter and return owned
// `String`s that the caller passes straight into `log::info!` / `log::warn!`.

/// First eight hex characters of the session id, used as a grep tag.
fn session_short(description: &SessionDescription) -> String {
    let hex = description.get_session_id().to_hex_string();
    hex[..hex.len().min(8)].to_string()
}

/// Synthesise the `COLLATION_FLOW` linkage handle for a self-collation flow.
///
/// Self-collation flows do not yet have a final `BlockIdExt` (root/file hashes
/// are produced by the collator), so we use `<workchain>:<shard>:<expected_seqno>`
/// formed from the session shard and the seqno the FSM is asking us to produce.
fn expected_block_id_label(description: &SessionDescription, expected_seqno: u32) -> String {
    format!("{}:{}", description.get_shard(), expected_seqno)
}

/// Render a list of previous block ids as `<seqno>:<short_hash>` pairs, comma-joined.
///
/// Returns `"none"` if the slice is empty so log grepping is deterministic.
fn format_prev_block_ids(prev_block_ids: &[BlockIdExt]) -> String {
    if prev_block_ids.is_empty() {
        return "none".to_string();
    }
    prev_block_ids
        .iter()
        .map(|id| format!("{}:{}", id.seq_no, &id.root_hash.to_hex_string()[..8]))
        .collect::<Vec<_>>()
        .join(",")
}

// ======================================================================
// Snapshot & event types
// ======================================================================
//
// Neutral snapshot and event types passed across the
// `SessionProcessor -> SessionTelemetry` boundary. Built by `SessionProcessor`
// from its private maps and consumed read-only by `SessionTelemetry`. They
// never expose `SessionProcessor` private types and they intentionally exclude
// any value that `SessionTelemetry` can derive from `&SessionDescription`.

/// Slim snapshot for health checks.
///
/// Contains only dynamic processor-owned state that `SessionTelemetry` cannot
/// derive from `&SessionDescription` (session id, self index, total nodes /
/// weight, thresholds, slot-window size, session age). `SessionTelemetry`
/// methods receive `&SessionDescription` and read description-derived values
/// directly there.
pub(crate) struct HealthCheckSnapshot {
    /// Aggregate weight of validators currently considered active by the receiver.
    pub(crate) active_weight: ValidatorWeight,
    /// Lowest non-finalized slot index (frontier of the finalized prefix).
    pub(crate) first_non_finalized: SlotIndex,
    /// Lowest slot that has not yet made progress toward notarization.
    pub(crate) first_non_progressed: SlotIndex,
    /// Finalized head slot, if any block has been finalized this session.
    pub(crate) finalized_head_slot: Option<SlotIndex>,
    /// Number of peers from which no activity has been observed since session start.
    pub(crate) peers_never_seen: usize,
}

/// Slim snapshot for `log_consensus_state`.
///
/// Captures only state that `SessionTelemetry` cannot derive from
/// `&SessionDescription`. Booleans (`generated`, `pending_generate`,
/// `has_notarized`, `is_finalized`) are computed by `SessionProcessor` from its
/// private slot runtime helpers before this snapshot is built.
pub(crate) struct ConsensusStateSnapshot {
    /// Caller-supplied tag describing what caused the dump. Borrowed
    /// `&'static str` (every existing call site passes a string literal) so
    /// snapshot construction is allocation-free.
    pub(crate) trigger: &'static str,
    /// Lowest non-finalized slot (frontier of the finalized prefix).
    pub(crate) first_non_finalized: SlotIndex,
    /// Lowest slot that has not yet made progress toward notarization.
    pub(crate) first_non_progressed: SlotIndex,
    /// Whether the current slot's candidate has been generated locally.
    pub(crate) generated: bool,
    /// Whether self-collation for the current slot is pending.
    pub(crate) pending_generate: bool,
    /// Number of pending validation entries.
    pub(crate) pending_validations_count: usize,
    /// Number of validated entries (approved + rejected).
    pub(crate) validated_count: usize,
    /// Whether the current slot has reached notarization.
    pub(crate) has_notarized: bool,
    /// Whether the current slot has been finalized.
    pub(crate) is_finalized: bool,
}

/// Neutral projection of the validation-lifecycle inventory dump section.
///
/// Built only for full debug dumps (`should_build_full_dump` true), never per
/// health check. Each bucket is the typed projection of the corresponding
/// `SessionProcessor` private cache; `SessionTelemetry` formats short hashes,
/// flags, and age strings.
pub(crate) struct ValidationInventorySnapshot {
    /// Cross-bucket totals for percentage formatting.
    pub(crate) totals: CandidateTotals,
    pub(crate) received: Vec<ValidationInventoryEntry>,
    pub(crate) validated: Vec<ValidationInventoryEntry>,
    pub(crate) notarized: Vec<ValidationInventoryEntry>,
    pub(crate) finalized: Vec<ValidationInventoryEntry>,
}

/// One entry in a validation-lifecycle bucket.
///
/// Use typed values, not preformatted strings. `SessionTelemetry` derives
/// short hashes, age strings, and flag labels while formatting.
pub(crate) struct ValidationInventoryEntry {
    pub(crate) slot: SlotIndex,
    pub(crate) source_idx: ValidatorIndex,
    pub(crate) candidate_hash: UInt256,
    pub(crate) block_id: BlockIdExt,
    pub(crate) received_at: SystemTime,
    pub(crate) is_pending: bool,
    pub(crate) is_approved: bool,
    pub(crate) is_rejected: bool,
    pub(crate) is_notarized: bool,
    pub(crate) is_finalized: bool,
    pub(crate) is_empty: bool,
}

/// Cheap status snapshot for periodic health checks and stall logs.
///
/// Always safe to build per `health_check_dump` or stall check. Intentionally
/// excludes session id, shard, self index, node counts, total weight,
/// thresholds, session age, candidate inventory, and window diagnostics. The
/// `observed_at` field is the sampling timestamp so the matching full dump
/// uses the same time base.
pub(crate) struct DumpStatusSnapshot {
    /// Sampling timestamp used both for the cheap status line and any
    /// subsequent full dump so both share a single time base.
    pub(crate) observed_at: SystemTime,
    /// Aggregate weight of validators currently considered active.
    pub(crate) active_weight: ValidatorWeight,
    /// Realized slot cadence in seconds (rolling average).
    pub(crate) slot_duration_secs: f64,
    /// Lowest non-finalized slot index.
    pub(crate) first_non_finalized: SlotIndex,
    /// Lowest slot that has not yet made progress toward notarization.
    pub(crate) first_non_progressed: SlotIndex,
    /// Block seqno of the finalized head, if any block has been finalized.
    pub(crate) finalized_head_seqno: Option<u32>,
}

/// Expensive snapshot for the full debug dump.
///
/// Built by `SessionProcessor::build_full_dump_snapshot` only after
/// `SessionTelemetry::should_build_full_dump(is_stalled)` returns true. Holds
/// owned copies of inventory and window diagnostics so `SessionTelemetry` can
/// format without re-reading `SessionProcessor` private maps.
///
/// Kept flat (no nested `HeadDumpSnapshot` / `PendingDumpSnapshot` /
/// `CollationDumpSnapshot` sub-structs) per design review: nested types would
/// only split scalar fields without changing ownership or dependency
/// boundaries.
pub(crate) struct FullDumpSnapshot {
    /* Heads */
    /// Finalized head slot, if any block has been finalized this session.
    pub(crate) finalized_head_slot: Option<SlotIndex>,
    /// Block id of the finalized head, if any.
    pub(crate) finalized_head_block_id: Option<BlockIdExt>,
    /// Most recent consensus-finalized block seqno reported to the engine.
    pub(crate) last_consensus_finalized_seqno: Option<u32>,
    /// Block seqno of the most recent accepted-normal head.
    pub(crate) accepted_normal_head_seqno: u32,
    /// Block id of the most recent accepted-normal head, if any.
    pub(crate) accepted_normal_head_block_id: Option<BlockIdExt>,
    /// Most recent masterchain finalized seqno observed by this session.
    pub(crate) last_mc_finalized_seqno: Option<u32>,

    /* Pending / validation counts */
    /// Number of pending validation entries.
    pub(crate) pending_validations_count: usize,
    /// Number of approved candidates not yet finalized.
    pub(crate) approved_count: usize,
    /// Number of rejected candidates retained for diagnostics.
    pub(crate) rejected_count: usize,
    /// Number of finalized entries still awaiting body delivery.
    pub(crate) finalized_pending_body_count: usize,

    /* Collation summary (scalars) */
    /// Current consensus slot.
    pub(crate) current_slot: SlotIndex,
    /// Whether self-collation for the current slot is pending.
    pub(crate) current_slot_pending_generate: bool,
    /// Whether a candidate for the current slot has been generated locally.
    pub(crate) current_slot_generated: bool,
    /// Whether the locally generated candidate has been broadcast.
    pub(crate) current_slot_sent_generated: bool,
    /// Precollated candidate cache size.
    pub(crate) precollated_count: usize,
    /// Locally generated candidates waiting for self-validation.
    pub(crate) generated_waiting_validation_count: usize,

    /* Heavy sections */
    /// Validation-lifecycle inventory bucketed by funnel stage.
    pub(crate) validation_inventory: ValidationInventorySnapshot,
    /// Per-window leader / standstill diagnostics for the visible range.
    pub(crate) window_diagnostics: Vec<WindowDiagnostic>,
    /// Raw output of `SimplexState::standstill_diagnostic_dump`. `None` if the
    /// raw output was empty; `SessionTelemetry` owns line splitting and
    /// indentation while formatting.
    pub(crate) standstill_diagnostic_dump: Option<String>,
    /// Health-snapshot input used to recompute findings for the dump
    /// conclusion / health_findings section. Embedded here (rather than
    /// passed as a separate argument) to keep
    /// [`SessionTelemetry::debug_dump_full`] signature narrow and to
    /// guarantee health and dump samples share one wall clock.
    pub(crate) health_snapshot: HealthCheckSnapshot,
}

// ======================================================================
// SessionTelemetry — recorders, health checks & dumps
// ======================================================================

#[allow(clippy::too_many_arguments)]
impl SessionTelemetry {
    /* Pure metric methods */

    /// Increment the running error count and the `simplex_errors` counter.
    ///
    /// Uses atomic increment on `session_errors_count` so callers can invoke
    /// this from `&self` paths. Mirrors the C++ reference's lifetime error
    /// bookkeeping.
    pub(crate) fn increment_error(&self) {
        self.session_errors_count.fetch_add(1, Ordering::Relaxed);
        self.errors_counter.increment(1);
    }

    /* Stall-cursor batch updates — each method takes the `stall_cursors` spin
    lock exactly once and updates a related group of fields, so the lock is
    fully owned by `SessionTelemetry` and critical sections are minimal. All run
    on `&self`. */

    /// Record the latest applied masterchain top block id.
    pub(crate) fn note_mc_applied_top(&self, block_id: BlockIdExt) {
        self.stall_cursors.lock().last_mc_applied_block_id = Some(block_id);
    }

    /// Note the current consensus frontiers, stamping the change time for
    /// whichever frontier advanced since the last call. The compare-and-set
    /// for both frontiers happens under a single lock.
    pub(crate) fn note_frontier_progress(
        &self,
        now: SystemTime,
        first_non_finalized: SlotIndex,
        first_non_progressed: SlotIndex,
    ) {
        let mut cursors = self.stall_cursors.lock();
        if first_non_finalized != cursors.prev_first_non_finalized {
            cursors.last_finalized_cursor_change_at = now;
            cursors.prev_first_non_finalized = first_non_finalized;
        }
        if first_non_progressed != cursors.prev_first_non_progressed {
            cursors.last_progression_change_at = now;
            cursors.prev_first_non_progressed = first_non_progressed;
        }
    }

    /// Record a notarization milestone, stamping both the notarization and
    /// the cached notar-certificate times/slots in one critical section.
    pub(crate) fn record_notarization_milestone(&self, now: SystemTime, slot: SlotIndex) {
        let mut cursors = self.stall_cursors.lock();
        cursors.last_notarization_at = Some(now);
        cursors.last_notarization_slot = Some(slot);
        cursors.last_notar_cert_at = Some(now);
        cursors.last_notar_cert_slot = Some(slot);
    }

    /// Record a finalization-certificate milestone.
    pub(crate) fn record_final_cert_milestone(&self, now: SystemTime, slot: SlotIndex) {
        let mut cursors = self.stall_cursors.lock();
        cursors.last_final_cert_at = Some(now);
        cursors.last_final_cert_slot = Some(slot);
    }

    /// Snapshot the stall cursors under a single lock for read-only use by
    /// the dump (and tests). Cloning keeps the lock critical section to one
    /// acquisition per dump instead of one per field read.
    fn stall_cursors_snapshot(&self) -> StallCursors {
        self.stall_cursors.lock().clone()
    }

    /* Round-debug / finalization timestamps (atomic nanos since epoch) */

    /// Trigger time for the next periodic stall debug dump.
    pub(crate) fn round_debug_at(&self) -> SystemTime {
        nanos_to_system_time(self.round_debug_at_nanos.load(Ordering::Relaxed))
    }

    /// Reschedule the next stall debug dump.
    pub(crate) fn set_round_debug_at(&self, at: SystemTime) {
        self.round_debug_at_nanos.store(system_time_to_nanos(at), Ordering::Relaxed);
    }

    /// Wall-clock time of the most recent finalization.
    pub(crate) fn last_finalization_time(&self) -> SystemTime {
        nanos_to_system_time(self.last_finalization_time_nanos.load(Ordering::Relaxed))
    }

    /// Record the wall-clock time of a finalization.
    pub(crate) fn set_last_finalization_time(&self, at: SystemTime) {
        self.last_finalization_time_nanos.store(system_time_to_nanos(at), Ordering::Relaxed);
    }

    /* Vestigial missing-body dedup set + latest receiver snapshot, both guarded
    by a `spin::Mutex` so their mutations stay `&self`. */

    /// Drop `slot` from the missing-body dedup set, returning whether it was
    /// present (used to gate a trace log on repair cancellation).
    pub(crate) fn forget_missing_body_log(&self, slot: u32) -> bool {
        self.missing_body_logged.lock().remove(&slot)
    }

    /// Prune the missing-body dedup set, keeping only slots `>= min_slot`.
    pub(crate) fn prune_missing_body_log_below(&self, min_slot: u32) {
        self.missing_body_logged.lock().retain(|&slot| slot >= min_slot);
    }

    /// Replace the latest receiver activity snapshot.
    pub(crate) fn set_receiver_snapshot(
        &self,
        snapshot: crate::receiver::ReceiverActivitySnapshot,
    ) {
        *self.last_receiver_snapshot.lock() = Some(snapshot);
    }

    /// Clone the latest receiver activity snapshot for read-only dump use.
    fn receiver_snapshot(&self) -> Option<crate::receiver::ReceiverActivitySnapshot> {
        self.last_receiver_snapshot.lock().clone()
    }

    /// Record receipt of a candidate (broadcast/query, self-vs-other).
    ///
    /// Ingress counters intentionally exclude locally generated candidates
    /// that loop back through `on_candidate_received`: those are not network
    /// ingress. `self_idx` is passed in so this method does not need a
    /// `&SessionDescription` parameter for a single field lookup.
    pub(crate) fn record_candidate_ingress(
        &self,
        sender_idx: ValidatorIndex,
        self_idx: ValidatorIndex,
        is_broadcast_candidate: bool,
    ) {
        if sender_idx == self_idx {
            return;
        }
        if is_broadcast_candidate {
            self.candidate_received_broadcast_counter.increment(1);
        } else {
            self.candidate_received_query_counter.increment(1);
        }
    }

    /// Record that a collation attempt has started.
    pub(crate) fn record_collation_start(&self) {
        self.collation_starts_counter.increment(1);
    }

    /* Self-collation observability funnel: private emitters — these `emit_*`
    helpers are the leaf layer of the funnel; they only touch metric counters /
    histograms and emit COLLATION_FLOW / missed-validation logs. They are driven
    exclusively by the public stateful recorders further below, which own the
    in-flight maps and compute the elapsed-time arguments passed in here. */

    /// Emit the start of a self-collation flow.
    ///
    /// Counter semantics: only `retry_count == None` (initial attempt)
    /// increments `simplex_self_collates.total`. Retries reuse the same flow
    /// so end-to-end latency is reported, not per-attempt latency.
    fn emit_self_collation_start(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        retry_count: Option<u32>,
        parent: Option<(SlotIndex, &UInt256)>,
        prev_block_ids: &[BlockIdExt],
    ) {
        let session_short = session_short(description);
        let expected_block_id = expected_block_id_label(description, expected_seqno);
        let parent_label = parent
            .map(|(parent_slot, hash)| format!("{}:{}", parent_slot, &hash.to_hex_string()[..8]))
            .unwrap_or_else(|| "none".to_string());
        let prevs_label = format_prev_block_ids(prev_block_ids);

        match retry_count {
            None => {
                self.self_collates_counter.total_increment();
                log::info!(
                    "Session {} COLLATION_FLOW start: expected_block_id={} slot={} \
                    expected_seqno={} attempt=initial parent={} prevs={}",
                    session_short,
                    expected_block_id,
                    slot,
                    expected_seqno,
                    parent_label,
                    prevs_label,
                );
            }
            Some(retry) => {
                log::info!(
                    "Session {} COLLATION_FLOW retry: expected_block_id={} slot={} \
                    expected_seqno={} attempt=retry-{} parent={} prevs={}",
                    session_short,
                    expected_block_id,
                    slot,
                    expected_seqno,
                    retry,
                    parent_label,
                    prevs_label,
                );
            }
        }
    }

    /// Log that the collator produced a candidate for this slot.
    ///
    /// This is not a terminal event for the self-collates metric; the
    /// success counter fires in [`Self::emit_self_collation_acceptance`]
    /// once the finalized callback links the flow to its block id.
    fn emit_self_collation_generated(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        outcome: &str,
        generation_ms: u128,
    ) {
        log::info!(
            "Session {} COLLATION_FLOW generated: expected_block_id={} slot={} \
            expected_seqno={} outcome={} generation_ms={}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            slot,
            expected_seqno,
            outcome,
            generation_ms,
        );
    }

    /// Record a terminal failure of the self-collation flow (no more retries).
    fn emit_self_collation_final_failure(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        elapsed_ms: u128,
        reason: &str,
    ) {
        self.self_collates_counter.failure();
        log::info!(
            "Session {} COLLATION_FLOW failure: expected_block_id={} slot={} expected_seqno={} \
            elapsed_ms={} reason={}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            slot,
            expected_seqno,
            elapsed_ms,
            reason,
        );
    }

    /// Log abandonment of self-collation flow tracking without counting failure.
    ///
    /// The "ignored" bucket is derived as `total - success - failure` by
    /// `add_compute_result_metric`, so we only need to keep the underlying
    /// counters consistent and log for grep visibility.
    fn emit_self_collation_ignored(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        elapsed_ms: u128,
        reason: &str,
    ) {
        log::info!(
            "Session {} COLLATION_FLOW ignore: expected_block_id={} slot={} expected_seqno={} \
            elapsed_ms={} reason={}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            slot,
            expected_seqno,
            elapsed_ms,
            reason,
        );
    }

    /// Log linkage of an in-flight self-collation flow to a concrete candidate id.
    fn emit_self_collation_candidate_linked(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        candidate_slot: SlotIndex,
        candidate_hash: &UInt256,
    ) {
        log::info!(
            "Session {} COLLATION_FLOW candidate: expected_block_id={} slot={} \
            expected_seqno={} candidate={}:{}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            slot,
            expected_seqno,
            candidate_slot,
            &candidate_hash.to_hex_string()[..8],
        );
    }

    /// Record successful acceptance of a self-collated candidate.
    ///
    /// Increments the success counter and records end-to-end acceptance
    /// latency on `time:self_collation_accept_latency`.
    fn emit_self_collation_acceptance(
        &self,
        description: &SessionDescription,
        expected_seqno: u32,
        candidate_slot: SlotIndex,
        candidate_hash: &UInt256,
        block_id: &BlockIdExt,
        has_final_cert: bool,
        acceptance_ms: u128,
    ) {
        self.self_collates_counter.success();
        self.self_collation_accept_latency_histogram.record(acceptance_ms as f64);

        log::info!(
            "Session {} COLLATION_FLOW acceptance: expected_block_id={} slot={} \
            expected_seqno={} candidate={}:{} block_id={} has_final_cert={} acceptance_ms={}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            candidate_slot,
            expected_seqno,
            candidate_slot,
            &candidate_hash.to_hex_string()[..8],
            block_id,
            has_final_cert,
            acceptance_ms,
        );
    }

    /// Record terminal failure of a self-collation flow already linked to a candidate id.
    fn emit_self_collation_candidate_failure(
        &self,
        description: &SessionDescription,
        expected_seqno: u32,
        candidate_slot: SlotIndex,
        candidate_hash: &UInt256,
        elapsed_ms: u128,
        reason: &str,
    ) {
        self.self_collates_counter.failure();

        log::info!(
            "Session {} COLLATION_FLOW failure: expected_block_id={} slot={} expected_seqno={} \
            candidate={}:{} elapsed_ms={} reason={}",
            session_short(description),
            expected_block_id_label(description, expected_seqno),
            candidate_slot,
            expected_seqno,
            candidate_slot,
            &candidate_hash.to_hex_string()[..8],
            elapsed_ms,
            reason,
        );
    }

    /// Record that a locally generated candidate did not receive its validation
    /// callback (either dropped before validation started, or started but never
    /// completed within the watch period).
    fn emit_generated_candidate_validation_missed(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        candidate_hash: Option<&UInt256>,
        validation_started: Option<bool>,
        waited_ms: Option<u128>,
        reason: &str,
    ) {
        self.generated_candidate_validation_missed_counter.increment(1);

        let session_short = session_short(description);
        match (candidate_hash, validation_started, waited_ms) {
            (Some(hash), Some(started), Some(waited)) => {
                log::warn!(
                    "Session {} local_generated_candidate_missed_validation: slot={} hash={} \
                    validation_started={} waited={}ms reason={}",
                    session_short,
                    slot,
                    &hash.to_hex_string()[..8],
                    started,
                    waited,
                    reason,
                );
            }
            _ => {
                log::warn!(
                    "Session {} local_generated_candidate_missed_validation: slot={} reason={}",
                    session_short,
                    slot,
                    reason,
                );
            }
        }
    }

    /* Self-collation observability funnel: public stateful recorders — these own
    the in-flight maps declared on the struct and delegate to the private
    `emit_*` helpers above for metrics / logging. They are clockless:
    `SessionProcessor` threads `now` and `&SessionDescription` in. All are
    `&self` because the maps are interior-mutable. */

    /// Record the start of a self-collation flow triggered by an
    /// `on_generate_slot()` dispatch.
    ///
    /// `retry_count == None` marks the FIRST (initial) attempt: it inserts
    /// the `(started_at, expected_seqno)` start record and bumps
    /// `simplex_self_collates.total`. Retries (`Some`) reuse the existing
    /// start record so latency reflects end-to-end time, not per-attempt
    /// time.
    ///
    /// Empty blocks created internally by the simplex layer (without going
    /// through `on_generate_slot()`) MUST NOT call this — they are not
    /// self-collations.
    pub(crate) fn record_self_collation_start(
        &self,
        description: &SessionDescription,
        slot: SlotIndex,
        expected_seqno: u32,
        retry_count: Option<u32>,
        parent: Option<(SlotIndex, &UInt256)>,
        prev_block_ids: &[BlockIdExt],
        now: SystemTime,
    ) {
        if retry_count.is_none() {
            self.self_collation_starts_by_slot.lock().insert(slot, (now, expected_seqno));
        }
        self.emit_self_collation_start(
            description,
            slot,
            expected_seqno,
            retry_count,
            parent,
            prev_block_ids,
        );
    }

    /// Record that the collator produced a candidate for `slot`.
    ///
    /// Not a terminal event for the metric — the success counter fires later
    /// in [`Self::record_self_collation_acceptance`]. No-op if the start
    /// record was already dropped.
    pub(crate) fn record_self_collation_generated(
        &self,
        slot: SlotIndex,
        outcome: &str,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let started = self.self_collation_starts_by_slot.lock().get(&slot).copied();
        let Some((started_at, expected_seqno)) = started else {
            return;
        };
        let generation_ms = now.duration_since(started_at).unwrap_or_default().as_millis();
        self.emit_self_collation_generated(
            description,
            slot,
            expected_seqno,
            outcome,
            generation_ms,
        );
    }

    /// Record a TERMINAL failure of the self-collation flow (no more
    /// retries), clearing the start record and bumping the failure counter.
    pub(crate) fn record_self_collation_final_failure(
        &self,
        slot: SlotIndex,
        reason: &str,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let removed = self.self_collation_starts_by_slot.lock().remove(&slot);
        let Some((started_at, expected_seqno)) = removed else {
            return;
        };
        let elapsed_ms = now.duration_since(started_at).unwrap_or_default().as_millis();
        self.emit_self_collation_final_failure(
            description,
            slot,
            expected_seqno,
            elapsed_ms,
            reason,
        );
    }

    /// Drop self-collation tracking for `slot` WITHOUT counting a failure
    /// (the "ignore" bucket is auto-derived as `total - success - failure`).
    pub(crate) fn forget_self_collation_tracking(
        &self,
        slot: SlotIndex,
        reason: &str,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let removed = self.self_collation_starts_by_slot.lock().remove(&slot);
        let Some((started_at, expected_seqno)) = removed else {
            return;
        };
        let elapsed_ms = now.duration_since(started_at).unwrap_or_default().as_millis();
        self.emit_self_collation_ignored(description, slot, expected_seqno, elapsed_ms, reason);
    }

    /// Link an in-flight self-collation flow to the concrete
    /// `RawCandidateId` the collator produced, moving the start record into
    /// the pending-acceptance map so acceptance can be matched on
    /// `notify_block_finalized`.
    pub(crate) fn link_self_collation_candidate(
        &self,
        slot: SlotIndex,
        candidate_id: &RawCandidateId,
        description: &SessionDescription,
    ) {
        let removed = self.self_collation_starts_by_slot.lock().remove(&slot);
        let Some((started_at, expected_seqno)) = removed else {
            return;
        };
        self.self_collation_pending_acceptance
            .lock()
            .insert(candidate_id.clone(), (started_at, expected_seqno));
        self.emit_self_collation_candidate_linked(
            description,
            slot,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash,
        );
    }

    /// Record successful acceptance of our self-collated candidate when its
    /// finalized callback fires. Bumps the success counter and records
    /// end-to-end acceptance latency.
    pub(crate) fn record_self_collation_acceptance(
        &self,
        candidate_id: &RawCandidateId,
        block_id: &BlockIdExt,
        has_final_cert: bool,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let removed = self.self_collation_pending_acceptance.lock().remove(candidate_id);
        let Some((started_at, expected_seqno)) = removed else {
            return;
        };
        let acceptance_ms = now.duration_since(started_at).unwrap_or_default().as_millis();
        self.emit_self_collation_acceptance(
            description,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash,
            block_id,
            has_final_cert,
            acceptance_ms,
        );
    }

    /// Record terminal failure for a self-collation tracked by
    /// `RawCandidateId` (already past [`Self::link_self_collation_candidate`]).
    fn record_self_collation_candidate_failure(
        &self,
        candidate_id: &RawCandidateId,
        reason: &str,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let removed = self.self_collation_pending_acceptance.lock().remove(candidate_id);
        let Some((started_at, expected_seqno)) = removed else {
            return;
        };
        let elapsed_ms = now.duration_since(started_at).unwrap_or_default().as_millis();
        self.emit_self_collation_candidate_failure(
            description,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash,
            elapsed_ms,
            reason,
        );
    }

    /// Look up the start record `(started_at, expected_seqno)` for an
    /// in-flight self-collation on `slot` (copied out from behind the lock).
    pub(crate) fn self_collation_start(&self, slot: SlotIndex) -> Option<(SystemTime, u32)> {
        self.self_collation_starts_by_slot.lock().get(&slot).copied()
    }

    /// Retain only self-collation starts matching `predicate` (GC hook for
    /// `cleanup_old_candidates`).
    pub(crate) fn retain_self_collation_starts(
        &self,
        mut predicate: impl FnMut(SlotIndex) -> bool,
    ) {
        self.self_collation_starts_by_slot.lock().retain(|slot, _| predicate(*slot));
    }

    /// Retain only pending-acceptance entries matching `predicate` (GC hook
    /// for `cleanup_old_candidates`).
    pub(crate) fn retain_self_collation_pending(
        &self,
        mut predicate: impl FnMut(&RawCandidateId) -> bool,
    ) {
        self.self_collation_pending_acceptance.lock().retain(|id, _| predicate(id));
    }

    /* Generated-candidate validation watch — tracks locally generated candidates
    until they validate successfully, so a dropped candidate can be surfaced via
    the missed-validation counter / warning. The watch family below shares the
    funnel's pending-acceptance map through
    `record_self_collation_candidate_failure`. */

    /// Begin watching a locally generated candidate for a successful
    /// validation outcome.
    pub(crate) fn track_generated_candidate_for_validation(
        &self,
        candidate_id: RawCandidateId,
        now: SystemTime,
    ) {
        self.generated_candidates_waiting_validation.lock().insert(
            candidate_id,
            GeneratedCandidateValidationWatch { generated_at: now, validation_started: false },
        );
    }

    /// Mark that a watched candidate has entered higher-layer validation.
    pub(crate) fn mark_generated_candidate_validation_started(
        &self,
        candidate_id: &RawCandidateId,
    ) {
        if let Some(watch) =
            self.generated_candidates_waiting_validation.lock().get_mut(candidate_id)
        {
            watch.validation_started = true;
        }
    }

    /// Stop watching a candidate that validated successfully.
    pub(crate) fn mark_generated_candidate_validation_succeeded(
        &self,
        candidate_id: &RawCandidateId,
    ) {
        self.generated_candidates_waiting_validation.lock().remove(candidate_id);
    }

    /// Number of locally generated candidates still awaiting a successful
    /// validation outcome.
    pub(crate) fn waiting_validation_count(&self) -> usize {
        self.generated_candidates_waiting_validation.lock().len()
    }

    /// Snapshot the watched candidate ids whose slot is `< up_to_slot`
    /// (used by `cleanup_old_candidates` to drain stale entries).
    pub(crate) fn stale_generated_candidate_ids(
        &self,
        up_to_slot: SlotIndex,
    ) -> Vec<RawCandidateId> {
        self.generated_candidates_waiting_validation
            .lock()
            .keys()
            .filter(|candidate_id| candidate_id.slot < up_to_slot)
            .cloned()
            .collect()
    }

    /// Note that a watched candidate (looked up by id) did not reach a
    /// successful validation outcome: drop the watch, record the linked
    /// self-collation failure, and bump the missed-validation counter.
    pub(crate) fn note_generated_candidate_validation_missed(
        &self,
        candidate_id: &RawCandidateId,
        reason: impl Into<String>,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let reason = reason.into();
        let removed = self.generated_candidates_waiting_validation.lock().remove(candidate_id);
        let Some(watch) = removed else {
            return;
        };

        self.record_self_collation_candidate_failure(
            candidate_id,
            &format!("generated_candidate_validation_missed: {}", reason),
            description,
            now,
        );

        let waited_ms = now.duration_since(watch.generated_at).unwrap_or_default().as_millis();
        self.emit_generated_candidate_validation_missed(
            description,
            candidate_id.slot,
            Some(&candidate_id.hash),
            Some(watch.validation_started),
            Some(waited_ms),
            &reason,
        );
    }

    /// Note that the watched candidate for `slot` (if any) did not reach a
    /// successful validation outcome. Mirrors
    /// [`Self::note_generated_candidate_validation_missed`] but resolves the
    /// candidate by slot; still bumps the counter (with slot-only payload)
    /// when no watch entry exists.
    pub(crate) fn note_generated_candidate_validation_missed_for_slot(
        &self,
        slot: SlotIndex,
        reason: impl Into<String>,
        description: &SessionDescription,
        now: SystemTime,
    ) {
        let reason = reason.into();
        let taken = {
            let mut watch_map = self.generated_candidates_waiting_validation.lock();
            let candidate_id = watch_map.keys().find(|id| id.slot == slot).cloned();
            candidate_id.and_then(|id| watch_map.remove(&id).map(|watch| (id, watch)))
        };
        if let Some((candidate_id, watch)) = taken {
            self.record_self_collation_candidate_failure(
                &candidate_id,
                &format!("generated_candidate_validation_missed_for_slot: {}", reason),
                description,
                now,
            );

            let waited_ms = now.duration_since(watch.generated_at).unwrap_or_default().as_millis();
            self.emit_generated_candidate_validation_missed(
                description,
                candidate_id.slot,
                Some(&candidate_id.hash),
                Some(watch.validation_started),
                Some(waited_ms),
                &reason,
            );
            return;
        }

        self.emit_generated_candidate_validation_missed(
            description,
            slot,
            None,
            None,
            None,
            &reason,
        );
    }

    /* Health checks & stall diagnosis */

    /// Compute structured health findings for the current state.
    ///
    /// Pure: no cooldown gating, no log emission, no counter updates. Used
    /// for both the stall conclusion block in [`Self::debug_dump_full`] and
    /// the cooldown-gated alert path in [`Self::run_health_checks`].
    fn collect_health_findings(
        &self,
        description: &SessionDescription,
        snapshot: &HealthCheckSnapshot,
    ) -> Vec<HealthFinding> {
        let now = description.get_time();
        let mut findings = Vec::new();
        // Read the dedup baselines under a single lock for the whole pass.
        let alert = self.health_alert_state.lock();

        let first_non_finalized = snapshot.first_non_finalized.0;
        let first_non_progressed = snapshot.first_non_progressed.0;
        let window_size = description.opts().slots_per_leader_window;
        let total_weight = description.get_total_weight();
        let active_weight = snapshot.active_weight;

        // Progress gap
        if first_non_progressed > first_non_finalized {
            let gap = first_non_progressed - first_non_finalized;
            if gap > window_size {
                let sev = if gap > 2 * window_size { log::Level::Error } else { log::Level::Warn };
                findings.push(HealthFinding {
                    kind: HealthFindingKind::ProgressGap,
                    severity: sev,
                    summary: format!(
                        "progress gap={gap} (nf={first_non_finalized} np={first_non_progressed} \
                        window={window_size})"
                    ),
                });
            }
        }

        // Zero finalization speed
        let stall_warn_secs = description.opts().health_stall_warning_secs;
        let stall_duration =
            now.duration_since(alert.last_finalization_nonzero_at).unwrap_or_default();
        if stall_duration >= Duration::from_secs(stall_warn_secs) {
            let stall_err_secs = description.opts().health_stall_error_secs;
            let sev = if stall_duration >= Duration::from_secs(stall_err_secs) {
                log::Level::Error
            } else {
                log::Level::Warn
            };
            findings.push(HealthFinding {
                kind: HealthFindingKind::ZeroFinalizationSpeed,
                severity: sev,
                summary: format!("no local finalization for {:.1}s", stall_duration.as_secs_f64()),
            });
        }

        // Low activity
        let t66 = threshold_66(total_weight);
        if active_weight < t66 {
            let t33 = threshold_33(total_weight);
            let sev = if active_weight < t33 { log::Level::Error } else { log::Level::Warn };
            let pct = if total_weight > 0 {
                (active_weight as f64 / total_weight as f64) * 100.0
            } else {
                0.0
            };
            findings.push(HealthFinding {
                kind: HealthFindingKind::LowActivity,
                severity: sev,
                summary: format!("active_weight={active_weight} ({pct:.0}%) < th66={t66}"),
            });
        }

        // Cert verify failures
        let current_cert_fails = self.cert_verify_fails_total.load(Ordering::Relaxed);
        let prev_cert_fails = alert.prev_cert_verify_fails;
        if current_cert_fails > prev_cert_fails {
            findings.push(HealthFinding {
                kind: HealthFindingKind::CertVerifyFailures,
                severity: log::Level::Warn,
                summary: format!(
                    "cert_verify_fail delta={} total={}",
                    current_cert_fails - prev_cert_fails,
                    current_cert_fails
                ),
            });
        }

        // Standstill triggers
        let current_standstill =
            self.receiver_health_counters.standstill_triggers.load(Ordering::Relaxed);
        let prev_standstill = alert.prev_standstill_triggers;
        if current_standstill > prev_standstill {
            findings.push(HealthFinding {
                kind: HealthFindingKind::StandstillTriggers,
                severity: log::Level::Warn,
                summary: format!(
                    "standstill_triggers delta={} total={}",
                    current_standstill - prev_standstill,
                    current_standstill
                ),
            });
        }

        // Candidate giveups
        let current_giveups =
            self.receiver_health_counters.candidate_giveups.load(Ordering::Relaxed);
        let prev_giveups = alert.prev_candidate_giveups;
        if current_giveups > prev_giveups {
            findings.push(HealthFinding {
                kind: HealthFindingKind::CandidateGiveups,
                severity: log::Level::Warn,
                summary: format!(
                    "candidate_giveups delta={} total={}",
                    current_giveups - prev_giveups,
                    current_giveups
                ),
            });
        }

        // Skip vote dominance
        let delta_notar = self
            .votes_in_notarize_total
            .load(Ordering::Relaxed)
            .saturating_sub(alert.prev_votes_in_notarize);
        let delta_final = self
            .votes_in_finalize_total
            .load(Ordering::Relaxed)
            .saturating_sub(alert.prev_votes_in_finalize);
        let delta_skip = self
            .votes_in_skip_total
            .load(Ordering::Relaxed)
            .saturating_sub(alert.prev_votes_in_skip);
        let delta_total = delta_notar + delta_final + delta_skip;
        let skip_ratio_min_delta = (description.get_total_nodes() as u64).max(2) / 2;
        if delta_total >= skip_ratio_min_delta {
            let progress_votes = delta_notar + delta_final;
            let skip_to_progress = delta_skip as f64 / (progress_votes.max(1) as f64);
            if skip_to_progress >= 3.0 {
                let sev = if skip_to_progress >= 8.0 && progress_votes == 0 {
                    log::Level::Error
                } else {
                    log::Level::Warn
                };
                let skip_share = if delta_total > 0 {
                    100.0 * delta_skip as f64 / delta_total as f64
                } else {
                    0.0
                };
                findings.push(HealthFinding {
                    kind: HealthFindingKind::SkipVoteDominance,
                    severity: sev,
                    summary: format!(
                        "skip_share={skip_share:.0}% skip={delta_skip} notar={delta_notar} \
                        final={delta_final}"
                    ),
                });
            }
        }

        // Validator isolation
        let session_age =
            now.duration_since(description.get_session_creation_time()).unwrap_or_default();
        if session_age > Duration::from_secs(60) && active_weight <= 1 && total_weight > 1 {
            findings.push(HealthFinding {
                kind: HealthFindingKind::ValidatorIsolated,
                severity: log::Level::Error,
                summary: format!("only self active, session_age={:.0}s", session_age.as_secs_f64()),
            });
        }

        findings
    }

    /// Emit cooldown-gated health alert log lines and update dedup baselines.
    ///
    /// Each anomaly emits a single-line WARN or ERROR log with the
    /// `SIMPLEX_HEALTH` prefix and increments `simplex_health_warnings`
    /// (never `session_errors_count`, except for the skip-vote-dominance
    /// error path which mirrors the C++ behaviour of bumping the session
    /// error counter once).
    pub(crate) fn run_health_checks(
        &self,
        description: &SessionDescription,
        snapshot: &HealthCheckSnapshot,
    ) {
        const SKIP_RATIO_WARN_THRESHOLD: f64 = 3.0;
        const SKIP_RATIO_ERROR_THRESHOLD: f64 = 8.0;

        let now = description.get_time();
        let session_id_hex = description.get_session_id().to_hex_string();
        let session_prefix = &session_id_hex[..8.min(session_id_hex.len())];
        // Hold the dedup-baseline lock once for the whole pass; all mutations
        // below go through this guard (single acquisition, SXMAIN-only path).
        let mut alert = self.health_alert_state.lock();
        let cooldown = alert.cooldown;
        let skip_ratio_min_delta_votes = (description.get_total_nodes() as u64).max(2) / 2;

        // 1. Progress gap.
        let first_non_finalized = snapshot.first_non_finalized.0;
        let first_non_progressed = snapshot.first_non_progressed.0;
        let window_size = description.opts().slots_per_leader_window;
        if first_non_progressed > first_non_finalized {
            let gap = first_non_progressed - first_non_finalized;
            if gap > window_size
                && now.duration_since(alert.last_progress_warn).unwrap_or_default() >= cooldown
            {
                alert.last_progress_warn = now;
                self.health_warnings_counter.increment(1);
                if gap > 2 * window_size {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=progress_gap session={session_prefix} gap={gap} \
                        first_non_finalized={first_non_finalized} \
                        first_non_progressed={first_non_progressed} window={window_size}",
                    );
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=progress_gap session={session_prefix} gap={gap} \
                        first_non_finalized={first_non_finalized} \
                        first_non_progressed={first_non_progressed} window={window_size}",
                    );
                }
            }
        }

        // 2. Zero finalization speed.
        let stall_warn_secs = description.opts().health_stall_warning_secs;
        let stall_err_secs = description.opts().health_stall_error_secs;
        let current_finalized = snapshot.finalized_head_slot.map(|s| s.0 as f64).unwrap_or(0.0);
        if current_finalized != alert.prev_last_finalized_slot {
            alert.last_finalization_nonzero_at = now;
            alert.prev_last_finalized_slot = current_finalized;
        } else {
            let stall_duration =
                now.duration_since(alert.last_finalization_nonzero_at).unwrap_or_default();
            if stall_duration >= Duration::from_secs(stall_warn_secs)
                && now.duration_since(alert.last_finalization_speed_warn).unwrap_or_default()
                    >= cooldown
            {
                alert.last_finalization_speed_warn = now;
                self.health_warnings_counter.increment(1);
                if stall_duration >= Duration::from_secs(stall_err_secs) {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=zero_finalization_speed session={session_prefix} \
                        stall_secs={:.0} last_finalized_slot={current_finalized}",
                        stall_duration.as_secs_f64(),
                    );
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=zero_finalization_speed session={session_prefix} \
                        stall_secs={:.0} last_finalized_slot={current_finalized}",
                        stall_duration.as_secs_f64(),
                    );
                }
            }
        }

        // 3. Low activity.
        let active_weight = snapshot.active_weight;
        let total_weight = description.get_total_weight();
        let t66 = threshold_66(total_weight);
        if active_weight < t66
            && now.duration_since(alert.last_activity_warn).unwrap_or_default() >= cooldown
        {
            alert.last_activity_warn = now;
            self.health_warnings_counter.increment(1);
            let pct = if total_weight > 0 {
                (active_weight as f64 / total_weight as f64) * 100.0
            } else {
                0.0
            };
            let t33 = threshold_33(total_weight);
            if active_weight < t33 {
                log::error!(
                    "SIMPLEX_HEALTH anomaly=low_activity session={session_prefix} \
                    active_weight={active_weight} threshold_66={t66} pct={pct:.0}%"
                );
            } else {
                log::warn!(
                    "SIMPLEX_HEALTH anomaly=low_activity session={session_prefix} \
                    active_weight={active_weight} threshold_66={t66} pct={pct:.0}%"
                );
            }
        }

        // 4. Cert verify failures (delta-based).
        let current_cert_fails = self.cert_verify_fails_total.load(Ordering::Relaxed);
        let prev_cert_fails = alert.prev_cert_verify_fails;
        if current_cert_fails > prev_cert_fails
            && now.duration_since(alert.last_cert_fail_warn).unwrap_or_default() >= cooldown
        {
            let delta = current_cert_fails - prev_cert_fails;
            alert.prev_cert_verify_fails = current_cert_fails;
            alert.last_cert_fail_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=cert_verify_fail session={} delta={} total={}",
                session_prefix,
                delta,
                current_cert_fails
            );
        }

        // 5. Standstill trigger rate.
        let current_standstill =
            self.receiver_health_counters.standstill_triggers.load(Ordering::Relaxed);
        let prev_standstill = alert.prev_standstill_triggers;
        if current_standstill > prev_standstill
            && now.duration_since(alert.last_standstill_warn).unwrap_or_default() >= cooldown
        {
            let delta = current_standstill - prev_standstill;
            alert.prev_standstill_triggers = current_standstill;
            alert.last_standstill_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=standstill_triggers session={} delta={} total={}",
                session_prefix,
                delta,
                current_standstill
            );
        }

        // 6. Candidate request giveups.
        let current_giveups =
            self.receiver_health_counters.candidate_giveups.load(Ordering::Relaxed);
        let prev_giveups = alert.prev_candidate_giveups;
        if current_giveups > prev_giveups
            && now.duration_since(alert.last_candidate_giveup_warn).unwrap_or_default() >= cooldown
        {
            let delta = current_giveups - prev_giveups;
            alert.prev_candidate_giveups = current_giveups;
            alert.last_candidate_giveup_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=candidate_giveups session={} delta={} total={}",
                session_prefix,
                delta,
                current_giveups
            );
        }

        // 7. Skip/notar/final ratio anomaly.
        let current_notar = self.votes_in_notarize_total.load(Ordering::Relaxed);
        let current_final = self.votes_in_finalize_total.load(Ordering::Relaxed);
        let current_skip = self.votes_in_skip_total.load(Ordering::Relaxed);
        let delta_notar = current_notar.saturating_sub(alert.prev_votes_in_notarize);
        let delta_final = current_final.saturating_sub(alert.prev_votes_in_finalize);
        let delta_skip = current_skip.saturating_sub(alert.prev_votes_in_skip);
        let delta_total = delta_notar + delta_final + delta_skip;

        alert.prev_votes_in_notarize = current_notar;
        alert.prev_votes_in_finalize = current_final;
        alert.prev_votes_in_skip = current_skip;

        if delta_total >= skip_ratio_min_delta_votes
            && now.duration_since(alert.last_skip_ratio_warn).unwrap_or_default() >= cooldown
        {
            let progress_votes = delta_notar + delta_final;
            let skip_to_progress = delta_skip as f64 / (progress_votes.max(1) as f64);
            let skip_to_notar = if delta_notar > 0 {
                delta_skip as f64 / (delta_notar as f64)
            } else {
                f64::INFINITY
            };
            let skip_to_final = if delta_final > 0 {
                delta_skip as f64 / (delta_final as f64)
            } else {
                f64::INFINITY
            };
            let skip_share = if delta_total > 0 {
                100.0 * (delta_skip as f64) / (delta_total as f64)
            } else {
                0.0
            };

            if skip_to_progress >= SKIP_RATIO_WARN_THRESHOLD {
                alert.last_skip_ratio_warn = now;
                self.health_warnings_counter.increment(1);

                if skip_to_progress >= SKIP_RATIO_ERROR_THRESHOLD && progress_votes == 0 {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=skip_vote_dominance session={session_prefix} \
                        delta_skip={delta_skip} delta_notar={delta_notar} delta_final={delta_final} \
                        skip_share={skip_share:.0}% skip_to_progress={skip_to_progress:.2} \
                        skip_to_notar={skip_to_notar:.2} \
                        skip_to_final={skip_to_final:.2}"
                    );
                    self.increment_error();
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=skip_vote_dominance session={session_prefix} \
                        delta_skip={delta_skip} delta_notar={delta_notar} delta_final={delta_final} \
                        skip_share={skip_share:.0}% skip_to_progress={skip_to_progress:.2} \
                        skip_to_notar={skip_to_notar:.2} \
                        skip_to_final={skip_to_final:.2}"
                    );
                }
            }
        }

        // 8. Validator isolation.
        let isolation_threshold = Duration::from_secs(60);
        let session_age =
            now.duration_since(description.get_session_creation_time()).unwrap_or_default();
        if session_age > isolation_threshold
            && active_weight <= 1
            && total_weight > 1
            && now.duration_since(alert.last_isolation_warn).unwrap_or_default()
                >= Duration::from_secs(300)
        {
            alert.last_isolation_warn = now;
            self.health_warnings_counter.increment(1);
            let peers_never_seen = snapshot.peers_never_seen;
            log::error!(
                "SIMPLEX_HEALTH anomaly=validator_isolated session={session_prefix} \
                active_weight={active_weight} total={total_weight} \
                session_age={:.0}s peers_never_seen={peers_never_seen}/{} — \
                possible validator key mismatch or overlay connectivity failure",
                session_age.as_secs_f64(),
                total_weight - 1,
            );
        }
    }

    /// Emit the periodic health-check dump line (cheap; runs every check).
    ///
    /// Equivalent to `log_dump_status(..., is_stalled = false)`. Provided
    /// as a separate entry point so `SessionProcessor::health_check_dump`
    /// has an obvious, intent-named delegate; full-dump building and
    /// health-check anomaly emission remain the caller's responsibility
    /// (`should_build_full_dump` + `debug_dump_full` + `run_health_checks`).
    pub(crate) fn health_check_dump(
        &self,
        description: &SessionDescription,
        status: &DumpStatusSnapshot,
    ) {
        self.log_dump_status(description, status, false);
    }

    // ------------------------------------------------------------------
    // Dump (Commits 7-9).
    // ------------------------------------------------------------------

    /// Emit the `ConsensusState` diagnostic line at `DEBUG`.
    ///
    /// Cheap, called after every consensus event from `check_all`. Gated
    /// by `log_enabled!(Debug)` so production logging stays quiet by
    /// default; the emission level matches the gate so the line actually
    /// appears whenever `DEBUG` is enabled. The text construction is delegated to
    /// [`Self::format_consensus_state`] so unit tests can assert on the
    /// rendered output without needing a global logger.
    pub(crate) fn log_consensus_state(
        &self,
        description: &SessionDescription,
        snapshot: &ConsensusStateSnapshot,
    ) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        log::debug!("{}", Self::format_consensus_state(description, snapshot));
    }

    /// Render the `ConsensusState` diagnostic line without emitting it.
    ///
    /// Pure formatter used by [`Self::log_consensus_state`] and by unit
    /// tests. Output mirrors the pre-migration `SessionProcessor`
    /// formatter byte-for-byte so existing log post-processing keeps
    /// working.
    fn format_consensus_state(
        description: &SessionDescription,
        snapshot: &ConsensusStateSnapshot,
    ) -> String {
        format!(
            "Session {} ConsensusState: trigger={}, slot_nf={}, slot_np={}, \
            generated={:<5}, pending_gen={:<5}, pending_val={}, validated={}, \
            notarized={}, finalized={}",
            description.get_session_id().to_hex_string(),
            snapshot.trigger,
            snapshot.first_non_finalized,
            snapshot.first_non_progressed,
            snapshot.generated,
            snapshot.pending_generate,
            snapshot.pending_validations_count,
            snapshot.validated_count,
            snapshot.has_notarized,
            snapshot.is_finalized,
        )
    }

    /// Decide whether the expensive `FullDumpSnapshot` should be built.
    ///
    /// Stalled sessions emit the full dump at `INFO`; healthy sessions emit
    /// it at `DEBUG`. Either way, the full snapshot is only built when the
    /// log level is enabled, so the candidate inventory and window
    /// diagnostics walks stay off the hot path in production.
    pub(crate) fn should_build_full_dump(&self, is_stalled: bool) -> bool {
        if is_stalled {
            log::log_enabled!(log::Level::Info)
        } else {
            log::log_enabled!(log::Level::Debug)
        }
    }

    /// Emit the cheap dump status / stall line and update error counters.
    ///
    /// Always emits the compact INFO-level health status line. On stall,
    /// additionally emits a single-line `Session ... stalled (...)` ERROR
    /// log and bumps `session_errors_count` (matching the C++ reference's
    /// behaviour of counting every stall as a session-level error).
    pub(crate) fn log_dump_status(
        &self,
        description: &SessionDescription,
        status: &DumpStatusSnapshot,
        is_stalled: bool,
    ) {
        let session_id_hex = description.get_session_id().to_hex_string();
        let session_prefix = &session_id_hex[..8.min(session_id_hex.len())];

        if is_stalled {
            let time_since_finalization = status
                .observed_at
                .duration_since(self.last_finalization_time())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            log::error!(
                "Session {session_prefix} stalled (no finalizations for {:.1}s, slot_dur={:.1}s, \
                threshold {:.0}s), slot_nf={}, slot_np={}",
                time_since_finalization,
                status.slot_duration_secs,
                ROUND_DEBUG_PERIOD.as_secs_f64(),
                status.first_non_finalized,
                status.first_non_progressed,
            );
            self.increment_error();
        }

        if log::log_enabled!(log::Level::Info) {
            let label = if is_stalled { "STALLED" } else { "OK" };
            let shard = description.get_shard();
            let head_seqno = status
                .finalized_head_seqno
                .map(|s| s.to_string())
                .unwrap_or_else(|| "?".to_string());
            log::info!(
                "Session {session_prefix} health [{label}]: shard={}:{:016x} slot_nf={} \
                slot_np={} finalized_head_seqno={head_seqno}",
                shard.workchain_id(),
                shard.shard_prefix_with_tag(),
                status.first_non_finalized,
                status.first_non_progressed,
            );
        }
    }

    /// Append the validation-inventory section to the dump output buffer.
    ///
    /// Pure formatter: iterates the pre-bucketed entries on
    /// `inventory` (built upstream by
    /// [`SessionProcessor::build_validation_inventory_snapshot`][bvs]) and
    /// writes them as the lifecycle-bucketed report consumed by stall and
    /// debug dumps. No `SessionProcessor` state is read here; `now` is taken
    /// as a parameter so the formatter can render per-entry "ago" ages
    /// without depending on a wall clock.
    ///
    /// The on-the-wire layout (header + bucket headers + rows + summary
    /// line) is preserved byte-for-byte from the pre-migration
    /// `SessionProcessor::dump_validation_inventory` so existing log
    /// post-processing keeps working.
    ///
    /// [bvs]: crate::session_processor::SessionProcessor::build_validation_inventory_snapshot
    fn format_validation_inventory(
        &self,
        r: &mut String,
        now: SystemTime,
        inventory: &ValidationInventorySnapshot,
    ) {
        r.push_str("  validation:\n");

        let totals = &inventory.totals;
        append_inventory_bucket(
            r,
            "received",
            totals.pct(totals.received_unvalidated),
            now,
            &inventory.received,
        );
        append_inventory_bucket(
            r,
            "validated",
            totals.pct(totals.validated_not_notarized),
            now,
            &inventory.validated,
        );
        append_inventory_bucket(
            r,
            "notarized",
            totals.pct(totals.notarized_not_finalized),
            now,
            &inventory.notarized,
        );
        append_inventory_bucket(
            r,
            "finalized",
            totals.pct(totals.finalized_recent),
            now,
            &inventory.finalized,
        );

        r.push_str(&format!(
            "    other: omitted={} total_received={}\n",
            totals.other_omitted, totals.received_total,
        ));
    }

    /// Emit the full debug dump using the expensive `FullDumpSnapshot`.
    ///
    /// Renders the multi-section dump (header / frontiers / milestones /
    /// heads / statistics / collation / validation inventory / peers /
    /// health findings / standstill diagnostic) and emits a single
    /// `ERROR`-level log line on stall or a single `DEBUG`-level line
    /// otherwise. Pure formatter: all dynamic state arrives via
    /// `description` (session-level domain object), `status` (cheap
    /// scalars sampled by `SessionProcessor::build_dump_status_snapshot`),
    /// `full` (owned snapshot built only after `should_build_full_dump`
    /// has returned true), and `self.*` telemetry fields (vote totals,
    /// milestone timestamps, receiver activity snapshot).
    pub(crate) fn debug_dump_full(
        &self,
        description: &SessionDescription,
        status: &DumpStatusSnapshot,
        full: &FullDumpSnapshot,
        is_stalled: bool,
    ) {
        let health_findings = self.collect_health_findings(description, &full.health_snapshot);
        let text =
            self.build_debug_dump_text(description, status, full, is_stalled, &health_findings);
        if is_stalled {
            log::error!("{text}");
        } else {
            log::debug!("{text}");
        }
    }

    /// Render the full debug dump text without emitting it.
    ///
    /// Split from [`Self::debug_dump_full`] so unit tests can assert
    /// on the buffer contents directly (capturing `log::error!` output
    /// would require a global logger which is awkward in parallel
    /// test runs).
    fn build_debug_dump_text(
        &self,
        description: &SessionDescription,
        status: &DumpStatusSnapshot,
        full: &FullDumpSnapshot,
        is_stalled: bool,
        health_findings: &[HealthFinding],
    ) -> String {
        let now = status.observed_at;
        let session_id_hex = description.get_session_id().to_hex_string();
        let shard = description.get_shard();
        let total_weight = description.get_total_weight();
        let session_time = now
            .duration_since(description.get_session_creation_time())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let mut r = String::with_capacity(4096);
        let status_str = if is_stalled { "STALLED" } else { "OK" };

        // ---- Conclusion (stalled only) ----
        r.push_str(&format!("Session {session_id_hex} dump [{status_str}]:\n"));
        if is_stalled {
            r.push_str("  conclusion:\n");
            for f in health_findings {
                r.push_str(&format!("    - {:?}: {}\n", f.kind, f.summary));
            }
            if let Some(wd) = full.window_diagnostics.first() {
                if let Some(sd) = wd.slots.first() {
                    r.push_str(&format!(
                        "    - frontier_reason: slot {} {} ({})\n",
                        sd.slot, sd.phase, sd.reason
                    ));
                }
            }
            if health_findings.is_empty() {
                r.push_str("    - none\n");
            }
        }

        // ---- Shard ----
        r.push_str(&format!(
            "  shard={}:{:016x}\n",
            shard.workchain_id(),
            shard.shard_prefix_with_tag(),
        ));

        // ---- Header ----
        let t66 = threshold_66(total_weight);
        let t33 = threshold_33(total_weight);
        let active_pct = if total_weight > 0 {
            100.0 * status.active_weight as f64 / total_weight as f64
        } else {
            0.0
        };
        r.push_str("  header:\n");
        r.push_str(&format!(
            "    validators={} local={} session_time={session_time:.1}s slot_duration={:.1}s\n",
            description.get_total_nodes(),
            description.get_self_idx(),
            status.slot_duration_secs,
        ));
        r.push_str(&format!(
            "    total_weight={total_weight} th66={t66} th33={t33} active_weight={} \
            ({active_pct:.1}%)\n",
            status.active_weight,
        ));

        // ---- Frontiers ----
        // One snapshot of the stall cursors for the whole dump (single lock).
        let cursors = self.stall_cursors_snapshot();
        let nf_age = fmt_dur(now, cursors.last_finalized_cursor_change_at);
        let np_age = fmt_dur(now, cursors.last_progression_change_at);
        r.push_str("  frontiers:\n");
        r.push_str(&format!(
            "    first_non_finalized={} (unchanged {nf_age})\n",
            status.first_non_finalized,
        ));
        r.push_str(&format!(
            "    first_non_progressed={} (unchanged {np_age})\n",
            status.first_non_progressed,
        ));

        // ---- Milestones ----
        let fmt_milestone =
            |label: &str, seqno: Option<u32>, slot: Option<SlotIndex>, ts: Option<SystemTime>| {
                let seqno_str =
                    seqno.map(|s| format!("seqno={s}")).unwrap_or_else(|| "seqno=?".to_string());
                let slot_str = slot.map(|s| format!(" slot={s}")).unwrap_or_default();
                let age = fmt_ago(now, ts);
                format!("    {label}: {seqno_str}{slot_str}, {age}\n")
            };
        r.push_str(&fmt_milestone(
            "last_finalization",
            status.finalized_head_seqno,
            full.finalized_head_slot,
            Some(self.last_finalization_time()),
        ));
        r.push_str(&fmt_milestone(
            "last_notarization",
            None,
            cursors.last_notarization_slot,
            cursors.last_notarization_at,
        ));
        r.push_str(&fmt_milestone(
            "last_final_cert",
            None,
            cursors.last_final_cert_slot,
            cursors.last_final_cert_at,
        ));
        r.push_str(&fmt_milestone(
            "last_notar_cert",
            None,
            cursors.last_notar_cert_slot,
            cursors.last_notar_cert_at,
        ));

        // ---- Heads ----
        r.push_str("  heads:\n");
        r.push_str(&format!(
            "    finalized_head_seqno={}\n",
            status.finalized_head_seqno.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
        ));
        if let Some(ref bid) = full.finalized_head_block_id {
            r.push_str(&format!(
                "    finalized_head=slot {} id=({})\n",
                full.finalized_head_slot.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
                bid,
            ));
        }
        r.push_str(&format!(
            "    last_consensus_finalized_seqno={}\n",
            full.last_consensus_finalized_seqno
                .map(|s| s.to_string())
                .unwrap_or_else(|| "?".to_string()),
        ));
        if let Some(ref bid) = full.accepted_normal_head_block_id {
            r.push_str(&format!(
                "    accepted_normal_head=seqno {} id=({})\n",
                full.accepted_normal_head_seqno, bid
            ));
        } else {
            r.push_str(&format!(
                "    accepted_normal_head=seqno {}\n",
                full.accepted_normal_head_seqno
            ));
        }
        if let Some(ref bid) = cursors.last_mc_applied_block_id {
            r.push_str(&format!("    last_mc_applied=({bid})\n"));
        }
        r.push_str(&format!(
            "    last_mc_finalized_seqno={}\n",
            full.last_mc_finalized_seqno.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
        ));

        // ---- Statistics ----
        let totals = &full.validation_inventory.totals;
        r.push_str("  statistics:\n");
        r.push_str(&format!(
            "    candidates: received={} validated={} ({:.1}%) notarized={} ({:.1}%) \
            finalized={} ({:.1}%) other={} ({:.1}%)\n",
            totals.received_total,
            totals.received_total - totals.received_unvalidated,
            totals.pct(totals.received_total - totals.received_unvalidated),
            totals.notarized_not_finalized + totals.finalized_recent + totals.other_omitted,
            totals.pct(
                totals.notarized_not_finalized + totals.finalized_recent + totals.other_omitted
            ),
            totals.finalized_recent,
            totals.pct(totals.finalized_recent),
            totals.other_omitted,
            totals.pct(totals.other_omitted),
        ));
        if let Some(ref snap) = self.receiver_snapshot() {
            let total_in_msgs: u64 = snap.sources.iter().map(|s| s.in_messages).sum();
            let total_out_msgs: u64 = snap.sources.iter().map(|s| s.out_messages).sum();
            let total_in_bcasts: u64 = snap.sources.iter().map(|s| s.in_broadcasts).sum();
            let total_out_bcasts: u64 = snap.sources.iter().map(|s| s.out_broadcasts).sum();
            let total_dup_votes: u64 = snap.sources.iter().map(|s| s.duplicate_votes).sum();
            let total_dup_bcasts: u64 = snap.sources.iter().map(|s| s.duplicate_broadcasts).sum();
            let total_req_sent: u64 = snap.sources.iter().map(|s| s.candidate_requests_sent).sum();
            let total_req_recv: u64 =
                snap.sources.iter().map(|s| s.candidate_requests_received).sum();
            r.push_str(&format!(
                "    traffic: msgs_in={total_in_msgs} msgs_out={total_out_msgs} \
                bcasts_in={total_in_bcasts} bcasts_out={total_out_bcasts}\n"
            ));
            r.push_str(&format!(
                "    votes_in: notar={} final={} skip={}\n",
                self.votes_in_notarize_total.load(Ordering::Relaxed),
                self.votes_in_finalize_total.load(Ordering::Relaxed),
                self.votes_in_skip_total.load(Ordering::Relaxed),
            ));
            r.push_str(&format!(
                "    duplicates: votes={total_dup_votes} broadcasts={total_dup_bcasts} \
                request_candidates_sent={total_req_sent} request_candidates_recv={total_req_recv}\n"
            ));
        }
        r.push_str(&format!(
            "    pending: validations={} approvals={} rejections={} \
            finalized_pending_body={}\n",
            full.pending_validations_count,
            full.approved_count,
            full.rejected_count,
            full.finalized_pending_body_count,
        ));

        // ---- Collation (per-window) ----
        r.push_str("  collation:\n");
        r.push_str(&format!(
            "    current_slot={} pending_gen={} generated={} sent_gen={} precollated={} \
            generated_waiting_validation={}\n",
            full.current_slot,
            full.current_slot_pending_generate,
            full.current_slot_generated,
            full.current_slot_sent_generated,
            full.precollated_count,
            full.generated_waiting_validation_count,
        ));
        for wd in &full.window_diagnostics {
            let leader_pubkey =
                base64_encode(description.get_source_public_key_hash(wd.leader_idx).data());
            let leader_adnl = base64_encode(description.get_source_adnl_id(wd.leader_idx).data());
            r.push_str(&format!(
                "    window {} slots=[{}..{}] leader={} pubkey_b64={leader_pubkey} \
                adnl_b64={leader_adnl}\n",
                wd.window_idx, wd.slot_begin, wd.slot_end, wd.leader_idx,
            ));
            for sd in &wd.slots {
                let mut flags = Vec::new();
                if sd.voted_notar {
                    flags.push("Voted");
                }
                if sd.voted_skip {
                    flags.push("VotedSkip");
                }
                if sd.voted_final {
                    flags.push("VotedFinal");
                }
                if sd.has_pending_block {
                    flags.push("Pending");
                }
                if sd.is_timeout_skipped {
                    flags.push("TimeoutSkipped");
                }
                let flags_str = if flags.is_empty() { "none".to_string() } else { flags.join("|") };
                let mut certs = Vec::new();
                if sd.has_notar_cert {
                    certs.push("notar");
                }
                if sd.has_final_cert {
                    certs.push("final");
                }
                if sd.has_skip_cert {
                    certs.push("skip");
                }
                let certs_str = if certs.is_empty() { "none".to_string() } else { certs.join("|") };
                r.push_str(&format!(
                    "      {} phase={} reason={} notar={:.0}% final={:.0}% skip={:.0}% \
                    flags=[{flags_str}] certs=[{certs_str}]\n",
                    sd.slot,
                    sd.phase,
                    sd.reason,
                    sd.notar_weight_pct,
                    sd.final_weight_pct,
                    sd.skip_weight_pct,
                ));
            }
        }

        // ---- Validation inventory ----
        self.format_validation_inventory(&mut r, now, &full.validation_inventory);

        // ---- Peers ----
        if let Some(ref snap) = self.receiver_snapshot() {
            r.push_str("  peers:\n");
            for src in &snap.sources {
                let is_self = src.source_idx == description.get_self_idx().0 as u32;
                let vi = ValidatorIndex::from(src.source_idx);
                let weight = description.get_node_weight(vi);
                let weight_pct = if total_weight > 0 {
                    100.0 * weight as f64 / total_weight as f64
                } else {
                    0.0
                };
                let pubkey_b64 = base64_encode(description.get_source_public_key_hash(vi).data());
                let last_act = fmt_ago(now, src.last_recv_time);
                let last_vote = fmt_ago(now, src.last_vote_recv_time);
                let last_final_cert = fmt_ago(now, src.last_final_cert_recv_time);
                let last_notar_cert = fmt_ago(now, src.last_notar_cert_recv_time);
                let last_cand = fmt_ago(now, src.last_candidate_recv_time);
                let marker = if is_self { " (self)" } else { "" };
                r.push_str(&format!(
                    "    {vi} adnl_b64={} pubkey_b64={pubkey_b64} weight={weight} \
                    ({weight_pct:.1}%) last_activity={last_act} last_vote={last_vote} \
                    last_final_cert={last_final_cert} last_notar_cert={last_notar_cert} \
                    last_candidate={last_cand} votes[n/f/s]={}/{}/{} certs[n/f/s]={}/{}/{} \
                    candidates={} req[s/r]={}/{}{marker}\n",
                    src.adnl_id_base64,
                    src.votes_in_notarize,
                    src.votes_in_finalize,
                    src.votes_in_skip,
                    src.certs_in_notar,
                    src.certs_in_final,
                    src.certs_in_skip,
                    src.candidates_received,
                    src.candidate_requests_sent,
                    src.candidate_requests_received,
                ));
            }
        }

        // ---- Health findings ----
        if !health_findings.is_empty() {
            r.push_str("  health_findings:\n");
            for f in health_findings {
                r.push_str(&format!("    - [{:?}] {:?}: {}\n", f.severity, f.kind, f.summary));
            }
        }

        // ---- Standstill diagnostic (stall only) ----
        if is_stalled {
            if let Some(ref diagnostic) = full.standstill_diagnostic_dump {
                r.push_str("  standstill_diagnostic:\n");
                for line in diagnostic.lines() {
                    r.push_str(&format!("    {line}\n"));
                }
            }
        }

        r
    }
}

// ======================================================================
// Private formatting helpers
// ======================================================================

/// Format an optional `SystemTime` relative to `now` as `"<x.x>s ago"`,
/// rendering `None` as `"never"`. Mirrors the legacy
/// `SessionProcessor::fmt_ago` so the dump output is byte-for-byte
/// compatible.
fn fmt_ago(now: SystemTime, t: Option<SystemTime>) -> String {
    t.and_then(|t| now.duration_since(t).ok())
        .map(|d| format!("{:.1}s ago", d.as_secs_f64()))
        .unwrap_or_else(|| "never".to_string())
}

/// Format a duration since `t` as `"<x.x>s"`, rendering clock skew as `"?"`.
fn fmt_dur(now: SystemTime, t: SystemTime) -> String {
    now.duration_since(t)
        .ok()
        .map(|d| format!("{:.1}s", d.as_secs_f64()))
        .unwrap_or_else(|| "?".to_string())
}

/// Render the per-entry flag bag for the validation inventory dump.
///
/// Mirrors the original `SessionProcessor::dump_validation_inventory`
/// ordering so the formatted output is byte-for-byte compatible.
fn format_inventory_flags(entry: &ValidationInventoryEntry) -> String {
    let mut flags = Vec::new();
    if entry.is_pending {
        flags.push("pending_validation");
    }
    if entry.is_approved {
        flags.push("approved");
    }
    if entry.is_rejected {
        flags.push("rejected");
    }
    if entry.is_notarized {
        flags.push("notarized");
    }
    if entry.is_finalized {
        flags.push("finalized");
    }
    if entry.is_empty {
        flags.push("empty");
    }
    if flags.is_empty() {
        "-".to_string()
    } else {
        flags.join(",")
    }
}

/// Append a `"<bucket> (<pct>%):\n"` header followed by one indented row
/// per `entry`, matching the layout of the pre-migration
/// `dump_validation_inventory`.
fn append_inventory_bucket(
    r: &mut String,
    name: &str,
    pct: f64,
    now: SystemTime,
    entries: &[ValidationInventoryEntry],
) {
    r.push_str(&format!("    {name} ({pct:.1}%):\n"));
    for entry in entries {
        let flags_str = format_inventory_flags(entry);
        let age = fmt_ago(now, Some(entry.received_at));
        r.push_str(&format!(
            "      slot {} src={} candidate={} block=({}) flags=[{}] recv={}\n",
            entry.slot,
            entry.source_idx,
            &entry.candidate_hash.to_hex_string()[..8],
            entry.block_id,
            flags_str,
            age,
        ));
    }
}

// ======================================================================
// Unit tests
// ======================================================================

#[cfg(test)]
impl SessionTelemetry {
    /// Test-only access to the health-alert dedup state behind the spin
    /// mutex. Returns the lock guard so unit tests can both read the dedup
    /// baselines and seed deterministic cooldown timestamps. Not compiled
    /// into release builds, so the lock stays fully owned by
    /// `SessionTelemetry` in production.
    pub(crate) fn health_alert_state_for_test(
        &self,
    ) -> impl std::ops::DerefMut<Target = HealthAlertState> + '_ {
        self.health_alert_state.lock()
    }

    /// Test-only: whether the vestigial missing-body dedup set is empty.
    fn missing_body_log_is_empty(&self) -> bool {
        self.missing_body_logged.lock().is_empty()
    }

    /// Test-only: whether `candidate_id` is currently being watched for a
    /// successful validation outcome.
    pub(crate) fn waiting_validation_contains(&self, candidate_id: &RawCandidateId) -> bool {
        self.generated_candidates_waiting_validation.lock().contains_key(candidate_id)
    }
}

#[cfg(test)]
#[path = "tests/test_session_telemetry.rs"]
mod tests;
