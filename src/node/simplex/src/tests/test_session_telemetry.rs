/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for `SessionTelemetry`.
//!
//! Included directly from `session_telemetry.rs` via `#[path]` so tests can
//! reach private internals without widening visibility. Mirrors the
//! convention used by `test_simplex_state.rs`.
//!
//! Per-commit test surface:
//! - Commit 1: smoke test asserting `SessionTelemetry::new` constructs
//!   cleanly and metric handles are registered.
//! - Commit 2: neutral snapshot type construction tests.
//! - Commit 3: pure metric recorder method tests.
//! - Commit 4: self-collation observability tests.
//! - Commit 6: health-check classification (one test per
//!   [`HealthFindingKind`]) plus cooldown / counter behavior of the
//!   emission path.
//! - Commit 7: `format_validation_inventory` formatter tests against
//!   hand-built snapshots (full buckets, empty buckets, long-hash
//!   truncation, flag rendering).
//! - Commit 8: debug-dump migration coverage — stall path
//!   error-counter bookkeeping, full-dump log-level gating, and full
//!   dump text content (status label, conclusion, standstill diagnostic
//!   inclusion, health findings).
//! - Commit 9 (this commit): `format_consensus_state` formatter test
//!   ensuring the byte-for-byte legacy `ConsensusState` line is
//!   preserved.

use super::*;
use crate::{
    receiver::ReceiverHealthCounters, session_description::SessionDescription, MetricsHandle,
    SessionId, SessionNode, SessionOptions,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime},
};
use ton_block::{Ed25519KeyOption, ShardIdent, UInt256, ZeroizingBytes};

/// Build a minimal `SessionDescription` for telemetry tests.
///
/// Mirrors `tests/test_simplex_state.rs::create_test_desc` but keeps the
/// helper local so telemetry tests do not depend on simplex-state test
/// scaffolding.
fn make_test_description(node_count: u32) -> SessionDescription {
    make_test_description_at(node_count, SystemTime::now())
}

/// Variant of [`make_test_description`] that anchors `session_creation_time`
/// to a caller-supplied wall clock. Commit 6 tests use this to make
/// `session_age` (used by `ValidatorIsolated`) deterministic.
fn make_test_description_at(node_count: u32, creation_time: SystemTime) -> SessionDescription {
    let nodes: Vec<SessionNode> = (0..node_count)
        .map(|_| {
            let public_key =
                Ed25519KeyOption::<ZeroizingBytes>::generate().expect("Failed to generate key");
            let adnl_id = public_key.id().clone();
            SessionNode { public_key, adnl_id, weight: 1 }
        })
        .collect();

    let local_key = nodes[0].public_key.clone();
    let shard = ShardIdent::masterchain();
    let opts = SessionOptions::default();

    SessionDescription::new(
        &opts,
        SessionId::default(),
        1,
        &nodes,
        local_key,
        &shard,
        creation_time,
        None,
    )
    .expect("SessionDescription::new must succeed for test fixtures")
}

/// Build a `SessionTelemetry` instance for unit tests.
fn make_telemetry(description: &SessionDescription, initial_errors: u32) -> SessionTelemetry {
    SessionTelemetry::new(
        MetricsHandle::new(None),
        description,
        Arc::new(ReceiverHealthCounters::new()),
        initial_errors,
        Duration::from_secs(30),
        SystemTime::now(),
    )
}

#[test]
fn session_telemetry_constructs_without_panicking() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    // Stall cursors initialized to slot zero.
    let cursors = telemetry.stall_cursors_snapshot();
    assert_eq!(cursors.prev_first_non_finalized, SlotIndex(0));
    assert_eq!(cursors.prev_first_non_progressed, SlotIndex(0));

    // No errors seeded.
    assert_eq!(telemetry.session_errors_count.load(Ordering::Relaxed), 0);

    // No vote totals accumulated yet.
    assert_eq!(telemetry.votes_in_notarize_total.load(Ordering::Relaxed), 0);
    assert_eq!(telemetry.votes_in_finalize_total.load(Ordering::Relaxed), 0);
    assert_eq!(telemetry.votes_in_skip_total.load(Ordering::Relaxed), 0);
    assert_eq!(telemetry.cert_verify_fails_total.load(Ordering::Relaxed), 0);

    // Late-bound receiver snapshot starts empty.
    assert!(telemetry.receiver_snapshot().is_none());

    // Missing-body dedup set starts empty.
    assert!(telemetry.missing_body_log_is_empty());

    // Round-debug trigger time is in the future from construction now.
    assert!(telemetry.round_debug_at() > telemetry.last_finalization_time());
}

#[test]
fn session_telemetry_seeds_initial_errors() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 7);

    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        7,
        "constructor must seed the running error count from `initial_errors`"
    );
}

// ============================================================================
// Commit 2: neutral snapshot type construction.
//
// These tests assert each snapshot can be assembled from typed scalars without
// any string preformatting or `SessionProcessor` private types, validating the
// snapshot boundary contract before behavior moves.
// ============================================================================

#[test]
fn health_check_snapshot_builds_from_typed_scalars() {
    let snapshot = HealthCheckSnapshot {
        active_weight: 5,
        first_non_finalized: SlotIndex(10),
        first_non_progressed: SlotIndex(9),
        finalized_head_slot: Some(SlotIndex(8)),
        peers_never_seen: 1,
    };

    assert_eq!(snapshot.active_weight, 5);
    assert_eq!(snapshot.first_non_finalized, SlotIndex(10));
    assert_eq!(snapshot.first_non_progressed, SlotIndex(9));
    assert_eq!(snapshot.finalized_head_slot, Some(SlotIndex(8)));
    assert_eq!(snapshot.peers_never_seen, 1);
}

#[test]
fn consensus_state_snapshot_uses_static_trigger() {
    let snapshot = ConsensusStateSnapshot {
        trigger: "check_all",
        first_non_finalized: SlotIndex(3),
        first_non_progressed: SlotIndex(2),
        generated: true,
        pending_generate: false,
        pending_validations_count: 4,
        validated_count: 2,
        has_notarized: true,
        is_finalized: false,
    };

    // Trigger is a borrowed static string so snapshot construction is
    // allocation-free.
    assert_eq!(snapshot.trigger, "check_all");
    assert!(snapshot.generated);
    assert!(snapshot.has_notarized);
    assert!(!snapshot.is_finalized);
}

#[test]
fn validation_inventory_snapshot_uses_typed_fields() {
    let entry = ValidationInventoryEntry {
        slot: SlotIndex(5),
        source_idx: ValidatorIndex::from(1u32),
        candidate_hash: UInt256::default(),
        block_id: BlockIdExt::default(),
        received_at: SystemTime::now(),
        is_pending: true,
        is_approved: false,
        is_rejected: false,
        is_notarized: false,
        is_finalized: false,
        is_empty: false,
    };

    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 4,
            received_unvalidated: 1,
            validated_not_notarized: 1,
            notarized_not_finalized: 1,
            finalized_recent: 1,
            other_omitted: 0,
        },
        received: vec![entry],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    assert_eq!(inventory.received.len(), 1);
    assert_eq!(inventory.received[0].slot, SlotIndex(5));
    assert!(inventory.received[0].is_pending);
    assert_eq!(inventory.totals.received_total, 4);
    // Spot-check that the totals helper still works for percentage formatting.
    assert!((inventory.totals.pct(2) - 50.0).abs() < f64::EPSILON);
}

#[test]
fn dump_status_snapshot_excludes_description_derived_fields() {
    let now = SystemTime::now();
    let snapshot = DumpStatusSnapshot {
        observed_at: now,
        active_weight: 3,
        slot_duration_secs: 1.5,
        first_non_finalized: SlotIndex(20),
        first_non_progressed: SlotIndex(19),
        finalized_head_seqno: Some(42),
    };

    assert_eq!(snapshot.observed_at, now);
    assert_eq!(snapshot.active_weight, 3);
    assert_eq!(snapshot.finalized_head_seqno, Some(42));
}

// ============================================================================
// Commit 3: pure metric recorder methods.
// ============================================================================

#[test]
fn increment_error_bumps_count_and_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.increment_error();
    telemetry.increment_error();
    telemetry.increment_error();

    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        3,
        "running error count must reflect each increment_error call",
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        snapshot.counters.get("simplex_errors").copied(),
        Some(3),
        "simplex_errors counter must mirror session_errors_count",
    );
}

#[test]
fn record_candidate_ingress_distinguishes_broadcast_and_query() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let self_idx = description.get_self_idx();
    let other_idx =
        ValidatorIndex::from(if self_idx == ValidatorIndex::from(0u32) { 1u32 } else { 0u32 });

    telemetry.record_candidate_ingress(other_idx, self_idx, /* is_broadcast */ true);
    telemetry.record_candidate_ingress(other_idx, self_idx, /* is_broadcast */ true);
    telemetry.record_candidate_ingress(other_idx, self_idx, /* is_broadcast */ false);

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get("simplex_candidate_received_broadcast").copied(), Some(2),);
    assert_eq!(snapshot.counters.get("simplex_candidate_received_query").copied(), Some(1),);
}

#[test]
fn record_candidate_ingress_ignores_self_loopback() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let self_idx = description.get_self_idx();

    // Locally generated candidates loop back through `on_candidate_received`
    // but are not network ingress: counters must stay at zero.
    telemetry.record_candidate_ingress(self_idx, self_idx, true);
    telemetry.record_candidate_ingress(self_idx, self_idx, false);

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert!(
        snapshot.counters.get("simplex_candidate_received_broadcast").copied().unwrap_or(0) == 0,
        "self-ingress must not bump broadcast counter",
    );
    assert!(
        snapshot.counters.get("simplex_candidate_received_query").copied().unwrap_or(0) == 0,
        "self-ingress must not bump query counter",
    );
}

#[test]
fn record_collation_start_increments_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    for _ in 0..4 {
        telemetry.record_collation_start();
    }

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        snapshot.counters.get("simplex_collation_starts").copied(),
        Some(4),
        "simplex_collation_starts must reflect each record_collation_start call",
    );
}

// ============================================================================
// Commit 4: self-collation observability effects.
//
// Self-collation flow ownership stays in `SessionProcessor`; these tests
// exercise the telemetry side of the boundary directly, by calling
// `SessionTelemetry::record_self_collation_*` and asserting the counters
// `simplex_self_collates_*` and histogram `time:self_collation_accept_latency`
// are updated as the C++ reference expects.
// ============================================================================

/// Metric name prefix used by `ResultStatusCounter` for the self-collation
/// counter. Aligning the names here with the C++ reference is part of the
/// metric stability contract.
const SELF_COLLATES_TOTAL: &str = "simplex_self_collates.total";
const SELF_COLLATES_SUCCESS: &str = "simplex_self_collates.success";
const SELF_COLLATES_FAILURE: &str = "simplex_self_collates.failure";

#[test]
fn record_self_collation_start_initial_bumps_total_counter_only() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.emit_self_collation_start(
        &description,
        SlotIndex(7),
        42,
        None, // initial attempt
        None,
        &[],
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        snapshot.counters.get(SELF_COLLATES_TOTAL).copied(),
        Some(1),
        "initial attempt must bump simplex_self_collates.total",
    );
    assert!(
        snapshot.counters.get(SELF_COLLATES_SUCCESS).copied().unwrap_or(0) == 0,
        "initial attempt must not bump success",
    );
    assert!(
        snapshot.counters.get(SELF_COLLATES_FAILURE).copied().unwrap_or(0) == 0,
        "initial attempt must not bump failure",
    );
}

#[test]
fn record_self_collation_start_retry_does_not_bump_total() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.emit_self_collation_start(&description, SlotIndex(7), 42, Some(2), None, &[]);

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert!(
        snapshot.counters.get(SELF_COLLATES_TOTAL).copied().unwrap_or(0) == 0,
        "retry must not bump simplex_self_collates.total",
    );
}

#[test]
fn record_self_collation_acceptance_bumps_success_and_latency() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_hash = UInt256::default();

    telemetry.emit_self_collation_acceptance(
        &description,
        42,
        SlotIndex(7),
        &candidate_hash,
        &BlockIdExt::default(),
        /* has_final_cert */ true,
        /* acceptance_ms */ 1234,
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get(SELF_COLLATES_SUCCESS).copied(), Some(1));
    let acceptance_samples =
        snapshot.histograms.get("time:self_collation_accept_latency").cloned().unwrap_or_default();
    assert!(
        !acceptance_samples.is_empty(),
        "acceptance must record a `time:self_collation_accept_latency` sample",
    );
}

#[test]
fn record_self_collation_final_failure_bumps_failure_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.emit_self_collation_final_failure(&description, SlotIndex(7), 42, 100, "max_retries");

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get(SELF_COLLATES_FAILURE).copied(), Some(1),);
}

#[test]
fn record_self_collation_candidate_failure_bumps_failure_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_hash = UInt256::default();

    telemetry.emit_self_collation_candidate_failure(
        &description,
        42,
        SlotIndex(7),
        &candidate_hash,
        500,
        "rejected_by_validator",
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get(SELF_COLLATES_FAILURE).copied(), Some(1));
}

#[test]
fn record_self_collation_ignored_emits_no_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.emit_self_collation_ignored(&description, SlotIndex(7), 42, 25, "callback_cancel");

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert!(snapshot.counters.get(SELF_COLLATES_TOTAL).copied().unwrap_or(0) == 0);
    assert!(snapshot.counters.get(SELF_COLLATES_SUCCESS).copied().unwrap_or(0) == 0);
    assert!(snapshot.counters.get(SELF_COLLATES_FAILURE).copied().unwrap_or(0) == 0);
}

#[test]
fn record_self_collation_generated_logs_only() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);

    telemetry.emit_self_collation_generated(&description, SlotIndex(7), 42, "success", 250);

    let snapshot = telemetry.metrics_receiver.snapshot();
    // No counter changes; flow lifecycle counters are only touched on
    // initial start, acceptance, and final/candidate failure paths.
    assert!(snapshot.counters.get(SELF_COLLATES_TOTAL).copied().unwrap_or(0) == 0);
    assert!(snapshot.counters.get(SELF_COLLATES_SUCCESS).copied().unwrap_or(0) == 0);
    assert!(snapshot.counters.get(SELF_COLLATES_FAILURE).copied().unwrap_or(0) == 0);
}

#[test]
fn record_self_collation_candidate_linked_logs_only() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_hash = UInt256::default();

    telemetry.emit_self_collation_candidate_linked(
        &description,
        SlotIndex(7),
        42,
        SlotIndex(7),
        &candidate_hash,
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    // Linkage is a pure logging hook; no metric should change.
    assert!(snapshot.counters.get(SELF_COLLATES_TOTAL).copied().unwrap_or(0) == 0);
}

#[test]
fn record_generated_candidate_validation_missed_bumps_missed_counter() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_hash = UInt256::default();

    telemetry.emit_generated_candidate_validation_missed(
        &description,
        SlotIndex(7),
        Some(&candidate_hash),
        Some(true),
        Some(750),
        "timeout",
    );
    telemetry.emit_generated_candidate_validation_missed(
        &description,
        SlotIndex(8),
        None,
        None,
        None,
        "no_watch_entry",
    );

    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        snapshot.counters.get("simplex_generated_candidate_validation_missed").copied(),
        Some(2),
        "missed-validation counter must bump on every call regardless of watch payload",
    );
}

// ----------------------------------------------------------------------
// Self-collation funnel state mechanics (moved here from
// CollationController). These exercise the in-flight start /
// pending-acceptance maps and the generated-candidate validation watch
// through the public stateful recorders + accessors.
// ----------------------------------------------------------------------

#[test]
fn record_self_collation_start_tracks_initial_and_preserves_on_retry() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let slot = SlotIndex(7);
    let t0 = SystemTime::now();

    telemetry.record_self_collation_start(&description, slot, 42, None, None, &[], t0);
    let initial = telemetry.self_collation_start(slot);
    assert_eq!(
        initial,
        Some((t0, 42)),
        "Initial must insert the (started_at, expected_seqno) record"
    );

    // Retry MUST NOT overwrite the original start record — the whole flow
    // (initial + retries) is one self-collation.
    telemetry.record_self_collation_start(
        &description,
        slot,
        42,
        Some(1),
        None,
        &[],
        t0 + Duration::from_secs(1),
    );
    assert_eq!(
        telemetry.self_collation_start(slot),
        initial,
        "retry must reuse the original start record"
    );
}

#[test]
fn record_self_collation_final_failure_clears_start_and_counts_failure() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let slot = SlotIndex(7);
    let t0 = SystemTime::now();

    telemetry.record_self_collation_start(&description, slot, 42, None, None, &[], t0);
    telemetry.record_self_collation_final_failure(
        slot,
        "max_retries",
        &description,
        t0 + Duration::from_millis(50),
    );

    assert!(
        telemetry.self_collation_start(slot).is_none(),
        "final failure must clear the start record"
    );
    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get(SELF_COLLATES_FAILURE).copied(), Some(1));
}

#[test]
fn forget_self_collation_tracking_clears_start_without_failure() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let slot = SlotIndex(7);
    let t0 = SystemTime::now();

    telemetry.record_self_collation_start(&description, slot, 42, None, None, &[], t0);
    telemetry.forget_self_collation_tracking(
        slot,
        "callback_cancel",
        &description,
        t0 + Duration::from_millis(10),
    );

    assert!(telemetry.self_collation_start(slot).is_none(), "forget must clear the start record");
    let snapshot = telemetry.metrics_receiver.snapshot();
    assert_eq!(snapshot.counters.get(SELF_COLLATES_TOTAL).copied(), Some(1));
    assert!(
        snapshot.counters.get(SELF_COLLATES_FAILURE).copied().unwrap_or(0) == 0,
        "ignore path must NOT count a failure",
    );
}

#[test]
fn link_then_acceptance_drains_pending_and_counts_success_once() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let slot = SlotIndex(7);
    let candidate_id = RawCandidateId { slot, hash: UInt256::default() };
    let t0 = SystemTime::now();

    telemetry.record_self_collation_start(&description, slot, 42, None, None, &[], t0);
    telemetry.link_self_collation_candidate(slot, &candidate_id, &description);
    assert!(
        telemetry.self_collation_start(slot).is_none(),
        "linking must move the start record into the pending-acceptance map",
    );

    telemetry.record_self_collation_acceptance(
        &candidate_id,
        &BlockIdExt::default(),
        true,
        &description,
        t0 + Duration::from_millis(100),
    );
    assert_eq!(
        telemetry.metrics_receiver.snapshot().counters.get(SELF_COLLATES_SUCCESS).copied(),
        Some(1)
    );

    // Pending was drained: a second acceptance is a no-op.
    telemetry.record_self_collation_acceptance(
        &candidate_id,
        &BlockIdExt::default(),
        true,
        &description,
        t0 + Duration::from_millis(200),
    );
    assert_eq!(
        telemetry.metrics_receiver.snapshot().counters.get(SELF_COLLATES_SUCCESS).copied(),
        Some(1),
        "a second acceptance for an already-drained candidate must be a no-op",
    );
}

#[test]
fn retain_self_collation_starts_drops_old_slots() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let t0 = SystemTime::now();
    telemetry.record_self_collation_start(&description, SlotIndex(1), 1, None, None, &[], t0);
    telemetry.record_self_collation_start(&description, SlotIndex(5), 5, None, None, &[], t0);

    telemetry.retain_self_collation_starts(|slot| slot >= SlotIndex(3));
    assert!(telemetry.self_collation_start(SlotIndex(1)).is_none());
    assert!(telemetry.self_collation_start(SlotIndex(5)).is_some());
}

#[test]
fn generated_candidate_watch_track_mark_and_note_missed() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_id = RawCandidateId { slot: SlotIndex(7), hash: UInt256::default() };
    let t0 = SystemTime::now();

    telemetry.track_generated_candidate_for_validation(candidate_id.clone(), t0);
    assert!(telemetry.waiting_validation_contains(&candidate_id));
    assert_eq!(telemetry.waiting_validation_count(), 1);

    telemetry.mark_generated_candidate_validation_started(&candidate_id);

    telemetry.note_generated_candidate_validation_missed(
        &candidate_id,
        "timeout",
        &description,
        t0 + Duration::from_millis(500),
    );
    assert!(
        !telemetry.waiting_validation_contains(&candidate_id),
        "noting a miss must drop the watch entry",
    );
    assert_eq!(telemetry.waiting_validation_count(), 0);
    assert_eq!(
        telemetry
            .metrics_receiver
            .snapshot()
            .counters
            .get("simplex_generated_candidate_validation_missed")
            .copied(),
        Some(1),
    );
}

#[test]
fn mark_generated_candidate_validation_succeeded_drops_watch() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let candidate_id = RawCandidateId { slot: SlotIndex(7), hash: UInt256::default() };

    telemetry.track_generated_candidate_for_validation(candidate_id.clone(), SystemTime::now());
    assert_eq!(telemetry.waiting_validation_count(), 1);

    telemetry.mark_generated_candidate_validation_succeeded(&candidate_id);
    assert_eq!(telemetry.waiting_validation_count(), 0);
}

#[test]
fn stale_generated_candidate_ids_filters_below_slot() {
    let description = make_test_description(3);
    let telemetry = make_telemetry(&description, 0);
    let now = SystemTime::now();
    telemetry.track_generated_candidate_for_validation(
        RawCandidateId { slot: SlotIndex(1), hash: UInt256::default() },
        now,
    );
    telemetry.track_generated_candidate_for_validation(
        RawCandidateId { slot: SlotIndex(5), hash: UInt256::from([1u8; 32]) },
        now,
    );

    let stale = telemetry.stale_generated_candidate_ids(SlotIndex(3));
    assert_eq!(stale.len(), 1, "only slots below up_to_slot are stale");
    assert_eq!(stale[0].slot, SlotIndex(1));
}

#[test]
fn full_dump_snapshot_holds_owned_heavy_sections() {
    let validation_inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 0,
            received_unvalidated: 0,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    let snapshot = FullDumpSnapshot {
        finalized_head_slot: Some(SlotIndex(7)),
        finalized_head_block_id: None,
        last_consensus_finalized_seqno: Some(100),
        accepted_normal_head_seqno: 99,
        accepted_normal_head_block_id: None,
        last_mc_finalized_seqno: Some(50),
        pending_validations_count: 0,
        approved_count: 0,
        rejected_count: 0,
        finalized_pending_body_count: 0,
        current_slot: SlotIndex(8),
        current_slot_pending_generate: false,
        current_slot_generated: true,
        current_slot_sent_generated: true,
        precollated_count: 2,
        generated_waiting_validation_count: 1,
        validation_inventory,
        window_diagnostics: vec![],
        standstill_diagnostic_dump: None,
        health_snapshot: HealthCheckSnapshot {
            active_weight: 3,
            first_non_finalized: SlotIndex(8),
            first_non_progressed: SlotIndex(8),
            finalized_head_slot: Some(SlotIndex(7)),
            peers_never_seen: 0,
        },
    };

    assert_eq!(snapshot.finalized_head_slot, Some(SlotIndex(7)));
    assert_eq!(snapshot.accepted_normal_head_seqno, 99);
    assert_eq!(snapshot.current_slot, SlotIndex(8));
    assert!(snapshot.standstill_diagnostic_dump.is_none());
    assert!(snapshot.window_diagnostics.is_empty());
}

// ============================================================================
// Commit 6: health-check classification (`collect_health_findings`) and
// emission (`run_health_checks`).
//
// Each `HealthFindingKind` is exercised in isolation by constructing an
// otherwise-healthy snapshot and pushing exactly one channel into the
// anomaly band. This keeps every test independent of the others and makes
// regression localisation obvious when a new finding is added.
// ============================================================================

/// Deterministic baseline `SystemTime` for Commit 6 tests. Far enough past
/// `UNIX_EPOCH` that we can subtract minutes for session-age maths and far
/// enough below `SystemTime::now()` that running tests in real-time does
/// not perturb the math.
const T0_EPOCH_SECS: u64 = 1_700_000_000;

fn baseline_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(T0_EPOCH_SECS)
}

/// Build a `SessionDescription` + `SessionTelemetry` pair sharing the same
/// construction timestamp `now`. The description's replay clock is also
/// pinned to `now` so `description.get_time()` is deterministic.
///
/// Returns `(description, telemetry, now)`.
fn make_health_fixture(
    node_count: u32,
    sim_now: SystemTime,
) -> (SessionDescription, SessionTelemetry, SystemTime) {
    let description = make_test_description_at(node_count, sim_now);
    description.set_time(sim_now);
    let telemetry = SessionTelemetry::new(
        MetricsHandle::new(None),
        &description,
        Arc::new(ReceiverHealthCounters::new()),
        0,
        Duration::from_secs(30),
        sim_now,
    );
    (description, telemetry, sim_now)
}

#[test]
fn collect_health_findings_returns_empty_for_healthy_session() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    let snapshot = HealthCheckSnapshot {
        active_weight: 3, // full participation
        first_non_finalized: SlotIndex(10),
        first_non_progressed: SlotIndex(10), // no gap
        finalized_head_slot: Some(SlotIndex(9)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    assert!(
        findings.is_empty(),
        "a fully healthy snapshot must produce zero findings, got {:?}",
        findings.iter().map(|f| f.kind).collect::<Vec<_>>(),
    );
}

#[test]
fn collect_health_findings_returns_progress_gap() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // window_size defaults to 1, so a gap > 1 triggers ProgressGap.
    // A gap > 2 * window_size escalates severity to Error.
    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(10),
        first_non_progressed: SlotIndex(13), // gap = 3, window = 1 -> Error
        finalized_head_slot: Some(SlotIndex(9)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let progress_gap = findings.iter().find(|f| f.kind == HealthFindingKind::ProgressGap);
    let finding = progress_gap.expect("ProgressGap finding must be present");
    assert_eq!(finding.severity, log::Level::Error, "gap > 2*window must escalate to Error");
    assert!(finding.summary.contains("gap=3"));
}

#[test]
fn collect_health_findings_returns_zero_finalization_speed() {
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);

    // Advance the simulated wall clock 20s past the construction time so
    // stall_duration crosses the default `health_stall_warning_secs = 15`.
    description.set_time(now + Duration::from_secs(20));

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let stall = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::ZeroFinalizationSpeed)
        .expect("ZeroFinalizationSpeed finding must fire after stall threshold");
    assert_eq!(
        stall.severity,
        log::Level::Warn,
        "20s stall must produce Warn (60s would escalate to Error)",
    );
}

#[test]
fn collect_health_findings_returns_zero_finalization_speed_error_at_long_stall() {
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);

    // 70s ≥ default `health_stall_error_secs = 60` → Error severity.
    description.set_time(now + Duration::from_secs(70));

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let stall = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::ZeroFinalizationSpeed)
        .expect("ZeroFinalizationSpeed finding must fire after long stall");
    assert_eq!(stall.severity, log::Level::Error);
}

#[test]
fn collect_health_findings_returns_low_activity_warn() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // total_weight = 3 → threshold_66 = 3, threshold_33 = 2 (see
    // `utils::threshold_*`: `n*2/3 + 1` and `n/3 + 1` respectively, mirroring
    // the C++ "strictly greater than" semantics).
    // active_weight = 2 satisfies `< threshold_66` (Warn) but
    // not `< threshold_33` (Error).
    let snapshot = HealthCheckSnapshot {
        active_weight: 2,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let low = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::LowActivity)
        .expect("LowActivity finding must fire when active < threshold_66");
    assert_eq!(low.severity, log::Level::Warn);
    assert!(low.summary.contains("active_weight=2"));
}

#[test]
fn collect_health_findings_returns_low_activity_error_below_threshold_33() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // active_weight = 1 < threshold_33 = 2 → Error severity.
    let snapshot = HealthCheckSnapshot {
        active_weight: 1,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let low = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::LowActivity)
        .expect("LowActivity finding must fire");
    assert_eq!(low.severity, log::Level::Error);
}

#[test]
fn collect_health_findings_returns_cert_verify_failures() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());
    telemetry.cert_verify_fails_total.store(5, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let cert = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::CertVerifyFailures)
        .expect("CertVerifyFailures finding must fire on delta > 0");
    assert_eq!(cert.severity, log::Level::Warn);
    assert!(cert.summary.contains("delta=5"));
}

#[test]
fn collect_health_findings_returns_standstill_triggers() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());
    telemetry.receiver_health_counters.standstill_triggers.store(2, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let st = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::StandstillTriggers)
        .expect("StandstillTriggers finding must fire on delta > 0");
    assert!(st.summary.contains("delta=2"));
}

#[test]
fn collect_health_findings_returns_candidate_giveups() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());
    telemetry.receiver_health_counters.candidate_giveups.store(4, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let gv = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::CandidateGiveups)
        .expect("CandidateGiveups finding must fire on delta > 0");
    assert!(gv.summary.contains("delta=4"));
}

#[test]
fn collect_health_findings_returns_skip_vote_dominance_warn() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // delta_skip=4 vs delta_notar=1 → progress_votes=1, skip_to_progress=4.0
    // (>= 3.0 Warn, < 8.0 Error; progress_votes > 0 keeps it at Warn).
    telemetry.votes_in_skip_total.store(4, Ordering::Relaxed);
    telemetry.votes_in_notarize_total.store(1, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let dom = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::SkipVoteDominance)
        .expect("SkipVoteDominance finding must fire above ratio threshold");
    assert_eq!(dom.severity, log::Level::Warn);
}

#[test]
fn collect_health_findings_returns_skip_vote_dominance_error_when_no_progress() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // delta_skip=10, delta_notar=delta_final=0 → progress_votes=0,
    // skip_to_progress=10.0 >= 8.0 and progress_votes==0 → Error.
    telemetry.votes_in_skip_total.store(10, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let dom = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::SkipVoteDominance)
        .expect("SkipVoteDominance finding must fire when only skip traffic arrives");
    assert_eq!(dom.severity, log::Level::Error);
}

#[test]
fn collect_health_findings_returns_validator_isolated() {
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);

    // Advance simulated wall clock 70s past creation so session_age > 60s
    // and pair it with active_weight ≤ 1 to trip the isolation guard.
    description.set_time(now + Duration::from_secs(70));

    let snapshot = HealthCheckSnapshot {
        active_weight: 1,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 2,
    };

    let findings = telemetry.collect_health_findings(&description, &snapshot);
    let iso = findings
        .iter()
        .find(|f| f.kind == HealthFindingKind::ValidatorIsolated)
        .expect("ValidatorIsolated finding must fire when only self active for >60s");
    assert_eq!(iso.severity, log::Level::Error);
    assert!(iso.summary.contains("only self active"));
}

#[test]
fn run_health_checks_first_call_increments_warnings_and_updates_baselines() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // Trip cert_verify_failures: prev=0, current=3.
    telemetry.cert_verify_fails_total.store(3, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    telemetry.run_health_checks(&description, &snapshot);

    let metrics = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        metrics.counters.get("simplex_health_warnings").copied(),
        Some(1),
        "first cert-verify anomaly must bump simplex_health_warnings",
    );
    assert_eq!(
        telemetry.health_alert_state_for_test().prev_cert_verify_fails,
        3,
        "delta baseline must advance after emission",
    );
}

#[test]
fn run_health_checks_respects_cooldown_within_window() {
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    telemetry.cert_verify_fails_total.store(3, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    telemetry.run_health_checks(&description, &snapshot);

    // Second call with a *fresh* delta but still inside the 30s cooldown
    // window must not bump `simplex_health_warnings` again.
    telemetry.cert_verify_fails_total.store(6, Ordering::Relaxed);
    description.set_time(now + Duration::from_secs(5));
    telemetry.run_health_checks(&description, &snapshot);

    let metrics = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        metrics.counters.get("simplex_health_warnings").copied(),
        Some(1),
        "warnings counter must remain at 1 while inside cooldown",
    );
}

#[test]
fn run_health_checks_skip_dominance_error_bumps_session_errors() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());

    // Skip-only traffic (progress_votes==0, skip_to_progress=10/1=10) is
    // the only branch in `run_health_checks` that also bumps
    // `session_errors_count`, mirroring the C++ reference.
    telemetry.votes_in_skip_total.store(10, Ordering::Relaxed);

    let snapshot = HealthCheckSnapshot {
        active_weight: 3,
        first_non_finalized: SlotIndex(5),
        first_non_progressed: SlotIndex(5),
        finalized_head_slot: Some(SlotIndex(4)),
        peers_never_seen: 0,
    };

    telemetry.run_health_checks(&description, &snapshot);

    let metrics = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        metrics.counters.get("simplex_health_warnings").copied(),
        Some(1),
        "skip-vote-dominance Error branch must still bump health warnings",
    );
    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        1,
        "skip-vote-dominance Error branch must also bump session_errors_count",
    );
}

// ============================================================================
// Commit 7: validation inventory formatting.
//
// `SessionTelemetry::format_validation_inventory` is a pure formatter
// over a pre-bucketed `ValidationInventorySnapshot`. These tests exercise
// the formatter directly with hand-built snapshots, asserting the
// byte-for-byte structure the dump consumers depend on.
// ============================================================================

/// Construct a `ValidationInventoryEntry` with safe defaults; tests
/// override only the fields they care about.
fn make_inventory_entry(
    slot: SlotIndex,
    source_idx: u32,
    hash_byte: u8,
) -> ValidationInventoryEntry {
    let mut bytes = [0u8; 32];
    bytes[0] = hash_byte;
    ValidationInventoryEntry {
        slot,
        source_idx: ValidatorIndex::from(source_idx),
        candidate_hash: UInt256::from(bytes),
        block_id: BlockIdExt::default(),
        received_at: SystemTime::UNIX_EPOCH + Duration::from_secs(T0_EPOCH_SECS),
        is_pending: false,
        is_approved: false,
        is_rejected: false,
        is_notarized: false,
        is_finalized: false,
        is_empty: false,
    }
}

#[test]
fn format_validation_inventory_renders_all_buckets() {
    let (description, telemetry, _) = make_health_fixture(3, baseline_time());
    let _ = description; // only used for telemetry construction below.

    let now = baseline_time() + Duration::from_secs(2);

    let mut received = make_inventory_entry(SlotIndex(10), 1, 0x11);
    received.is_pending = true;

    let mut validated = make_inventory_entry(SlotIndex(11), 2, 0x22);
    validated.is_approved = true;

    let mut notarized = make_inventory_entry(SlotIndex(12), 0, 0x33);
    notarized.is_approved = true;
    notarized.is_notarized = true;

    let mut finalized = make_inventory_entry(SlotIndex(13), 0, 0x44);
    finalized.is_approved = true;
    finalized.is_notarized = true;
    finalized.is_finalized = true;

    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 4,
            received_unvalidated: 1,
            validated_not_notarized: 1,
            notarized_not_finalized: 1,
            finalized_recent: 1,
            other_omitted: 0,
        },
        received: vec![received],
        validated: vec![validated],
        notarized: vec![notarized],
        finalized: vec![finalized],
    };

    let mut buf = String::new();
    telemetry.format_validation_inventory(&mut buf, now, &inventory);

    // Header is fixed.
    assert!(buf.starts_with("  validation:\n"), "buf was: {buf}");
    // Each bucket header is present with a percentage rendered to one
    // decimal place from the totals/received_total ratio.
    assert!(buf.contains("    received (25.0%):\n"), "missing received header in {buf}");
    assert!(buf.contains("    validated (25.0%):\n"));
    assert!(buf.contains("    notarized (25.0%):\n"));
    assert!(buf.contains("    finalized (25.0%):\n"));
    // Per-entry rows carry the bucket-specific flag set. `SlotIndex` and
    // `ValidatorIndex` use the `s<NNN>` / `v<NNN>` `Display` impls — the
    // formatter must inherit them so the dump output stays consistent
    // with consumers downstream.
    assert!(buf.contains("slot s10 src=v001"), "missing received row in {buf}");
    assert!(buf.contains("flags=[pending_validation]"));
    assert!(buf.contains("slot s11 src=v002"));
    assert!(buf.contains("flags=[approved]"));
    assert!(buf.contains("slot s12 src=v000"));
    assert!(buf.contains("flags=[approved,notarized]"));
    assert!(buf.contains("slot s13 src=v000"));
    assert!(buf.contains("flags=[approved,notarized,finalized]"));
    // Summary trailer is fixed.
    assert!(buf.contains("    other: omitted=0 total_received=4\n"));
}

#[test]
fn format_validation_inventory_handles_empty_buckets() {
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 0,
            received_unvalidated: 0,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    let mut buf = String::new();
    telemetry.format_validation_inventory(&mut buf, baseline_time(), &inventory);

    assert!(buf.contains("    received (0.0%):\n"));
    assert!(buf.contains("    validated (0.0%):\n"));
    assert!(buf.contains("    notarized (0.0%):\n"));
    assert!(buf.contains("    finalized (0.0%):\n"));
    assert!(buf.contains("    other: omitted=0 total_received=0\n"));
    // No `slot ` rows must be emitted for empty buckets.
    assert!(!buf.contains("      slot "), "empty buckets must emit no rows, got {buf}");
}

#[test]
fn format_validation_inventory_truncates_long_hashes() {
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    // Build a candidate hash whose first 8 hex chars are easy to recognise
    // (`deadbeef`) and whose tail differs, so we can assert truncation.
    let mut bytes = [0u8; 32];
    bytes[0] = 0xde;
    bytes[1] = 0xad;
    bytes[2] = 0xbe;
    bytes[3] = 0xef;
    bytes[4] = 0xff; // tail byte that must NOT appear in the formatted row
    let mut entry = make_inventory_entry(SlotIndex(5), 0, 0);
    entry.candidate_hash = UInt256::from(bytes);
    entry.is_pending = true;

    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 1,
            received_unvalidated: 1,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![entry],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    let mut buf = String::new();
    telemetry.format_validation_inventory(&mut buf, baseline_time(), &inventory);

    assert!(buf.contains("candidate=deadbeef "), "expected short hash in {buf}");
    // Full 64-char hex string must not leak into the dump.
    assert!(!buf.contains("deadbeefff"), "long hash must be truncated, got {buf}");
}

#[test]
fn format_validation_inventory_flags_empty_marker_when_no_flags() {
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    // No flags set => formatter must render `-` to match the legacy dump.
    let entry = make_inventory_entry(SlotIndex(0), 0, 0);
    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 1,
            received_unvalidated: 1,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![entry],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    let mut buf = String::new();
    telemetry.format_validation_inventory(&mut buf, baseline_time(), &inventory);

    assert!(buf.contains("flags=[-]"), "unflagged entries must render `-`, got {buf}");
}

// ============================================================================
// Commit 8: debug-dump migration.
//
// `SessionTelemetry::{log_dump_status, should_build_full_dump,
// debug_dump_full, build_debug_dump_text, health_check_dump}` together
// own the dump aspect. These tests exercise:
//   * stall vs healthy error-counter bookkeeping
//   * full-dump gating against the global log level
//   * dump text content (status label, stall conclusion, standstill
//     diagnostic, health findings).
// ============================================================================

/// Anchor the global logger at INFO level once so:
///   - `should_build_full_dump(true)` returns true (Info enabled).
///   - `should_build_full_dump(false)` returns false (Debug disabled).
///
/// Uses `try_init` so the call is idempotent across parallel tests. Other
/// telemetry tests inherit the same level.
fn ensure_dump_logger_at_info() {
    let _ =
        env_logger::Builder::new().filter_level(log::LevelFilter::Info).is_test(true).try_init();
}

fn make_default_dump_status(now: SystemTime) -> DumpStatusSnapshot {
    DumpStatusSnapshot {
        observed_at: now,
        active_weight: 3,
        slot_duration_secs: 1.0,
        first_non_finalized: SlotIndex(7),
        first_non_progressed: SlotIndex(7),
        finalized_head_seqno: Some(42),
    }
}

fn make_default_full_dump(active_weight: u64) -> FullDumpSnapshot {
    let validation_inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 0,
            received_unvalidated: 0,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };
    FullDumpSnapshot {
        finalized_head_slot: Some(SlotIndex(6)),
        finalized_head_block_id: None,
        last_consensus_finalized_seqno: Some(42),
        accepted_normal_head_seqno: 42,
        accepted_normal_head_block_id: None,
        last_mc_finalized_seqno: Some(10),
        pending_validations_count: 0,
        approved_count: 0,
        rejected_count: 0,
        finalized_pending_body_count: 0,
        current_slot: SlotIndex(7),
        current_slot_pending_generate: false,
        current_slot_generated: false,
        current_slot_sent_generated: false,
        precollated_count: 0,
        generated_waiting_validation_count: 0,
        validation_inventory,
        window_diagnostics: vec![],
        standstill_diagnostic_dump: None,
        health_snapshot: HealthCheckSnapshot {
            active_weight,
            first_non_finalized: SlotIndex(7),
            first_non_progressed: SlotIndex(7),
            finalized_head_slot: Some(SlotIndex(6)),
            peers_never_seen: 0,
        },
    }
}

#[test]
fn log_dump_status_stall_increments_error_counter() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);

    telemetry.log_dump_status(&description, &status, /* is_stalled */ true);

    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        1,
        "stall path must bump session_errors_count exactly once",
    );
    let metrics = telemetry.metrics_receiver.snapshot();
    assert_eq!(
        metrics.counters.get("simplex_errors").copied(),
        Some(1),
        "stall path must mirror the bump on the simplex_errors counter",
    );
}

#[test]
fn log_dump_status_healthy_does_not_increment_error() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);

    telemetry.log_dump_status(&description, &status, /* is_stalled */ false);

    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        0,
        "non-stall status line must not touch session_errors_count",
    );
}

#[test]
fn health_check_dump_is_alias_for_healthy_log_dump_status() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);

    telemetry.health_check_dump(&description, &status);

    assert_eq!(
        telemetry.session_errors_count.load(Ordering::Relaxed),
        0,
        "health_check_dump must dispatch the healthy branch (no error bump)",
    );
}

#[test]
fn should_build_full_dump_stalled_branch_mirrors_info_level() {
    // The gate must be exactly `log::log_enabled!(Info)`. We sample the
    // current state instead of asserting an absolute value because the
    // global logger may be initialised at any level by another test in
    // the same binary (env_logger::try_init is first-write-wins).
    ensure_dump_logger_at_info();
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    assert_eq!(
        telemetry.should_build_full_dump(/* is_stalled */ true),
        log::log_enabled!(log::Level::Info),
        "stall branch must gate full-dump building on the INFO log level",
    );
}

#[test]
fn should_build_full_dump_healthy_branch_mirrors_debug_level() {
    // Same reasoning as the stalled-branch test: assert the gate
    // matches the log macro rather than a hard-coded boolean.
    ensure_dump_logger_at_info();
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    assert_eq!(
        telemetry.should_build_full_dump(/* is_stalled */ false),
        log::log_enabled!(log::Level::Debug),
        "healthy branch must gate full-dump building on the DEBUG log level",
    );
}

#[test]
fn debug_dump_full_text_labels_stall_status() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);
    let full = make_default_full_dump(/* active_weight */ 3);

    let text = telemetry.build_debug_dump_text(
        &description,
        &status,
        &full,
        /* is_stalled */ true,
        /* health_findings */ &[],
    );

    assert!(text.contains("dump [STALLED]"), "stall text must label header as STALLED: {text}");
    assert!(
        text.contains("conclusion:"),
        "stalled dumps must include a conclusion block, got {text}",
    );
    assert!(
        text.contains("- none"),
        "empty health findings + stall must render `- none`, got {text}",
    );
}

#[test]
fn debug_dump_full_text_labels_healthy_status_and_omits_conclusion() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);
    let full = make_default_full_dump(3);

    let text = telemetry.build_debug_dump_text(
        &description,
        &status,
        &full,
        /* is_stalled */ false,
        &[],
    );

    assert!(text.contains("dump [OK]"), "healthy header must be `dump [OK]`");
    assert!(
        !text.contains("conclusion:"),
        "non-stall dumps must omit the conclusion section, got {text}",
    );
}

#[test]
fn debug_dump_full_text_includes_standstill_dump_when_present() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);
    let mut full = make_default_full_dump(3);
    full.standstill_diagnostic_dump = Some("row1\nrow2".to_string());

    let text = telemetry.build_debug_dump_text(
        &description,
        &status,
        &full,
        /* is_stalled */ true,
        &[],
    );

    assert!(text.contains("standstill_diagnostic:\n"));
    assert!(text.contains("    row1\n"));
    assert!(text.contains("    row2\n"));
}

#[test]
fn debug_dump_full_text_skips_standstill_dump_when_not_stalled() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);
    let mut full = make_default_full_dump(3);
    full.standstill_diagnostic_dump = Some("row1".to_string());

    let text = telemetry.build_debug_dump_text(
        &description,
        &status,
        &full,
        /* is_stalled */ false,
        &[],
    );

    assert!(
        !text.contains("standstill_diagnostic:"),
        "non-stall dumps must not render the standstill diagnostic block, got {text}",
    );
}

#[test]
fn debug_dump_full_text_lists_health_findings_section() {
    ensure_dump_logger_at_info();
    let now = baseline_time();
    let (description, telemetry, _) = make_health_fixture(3, now);
    let status = make_default_dump_status(now);
    let full = make_default_full_dump(3);
    let findings = vec![HealthFinding {
        kind: HealthFindingKind::LowActivity,
        severity: log::Level::Warn,
        summary: "active_weight=1 (33%) < th66=2".to_string(),
    }];

    let text = telemetry.build_debug_dump_text(
        &description,
        &status,
        &full,
        /* is_stalled */ true,
        &findings,
    );

    // Stalled => conclusion block lists findings (with kind), and there
    // is also a dedicated `health_findings:` section with severity tag.
    assert!(text.contains("    - LowActivity: active_weight=1"));
    assert!(text.contains("  health_findings:\n"));
    assert!(text.contains("    - [Warn] LowActivity:"));
}

#[test]
fn format_validation_inventory_age_uses_caller_now() {
    let (_description, telemetry, _) = make_health_fixture(3, baseline_time());

    let received_at = baseline_time();
    let mut entry = make_inventory_entry(SlotIndex(1), 0, 0);
    entry.received_at = received_at;

    let inventory = ValidationInventorySnapshot {
        totals: CandidateTotals {
            received_total: 1,
            received_unvalidated: 1,
            validated_not_notarized: 0,
            notarized_not_finalized: 0,
            finalized_recent: 0,
            other_omitted: 0,
        },
        received: vec![entry],
        validated: vec![],
        notarized: vec![],
        finalized: vec![],
    };

    let mut buf = String::new();
    telemetry.format_validation_inventory(
        &mut buf,
        received_at + Duration::from_millis(2500),
        &inventory,
    );

    assert!(buf.contains("recv=2.5s ago"), "expected `2.5s ago`, got {buf}");
}

// ============================================================================
// Commit 9: `log_consensus_state` formatter.
//
// Single pure-function test: `format_consensus_state` must produce the
// byte-for-byte same `ConsensusState` trace line as the pre-migration
// `SessionProcessor::log_consensus_state` so existing log post-processing
// keeps working.
// ============================================================================

#[test]
fn format_consensus_state_matches_legacy_byte_for_byte() {
    let (description, _telemetry, _) = make_health_fixture(3, baseline_time());
    let snapshot = ConsensusStateSnapshot {
        trigger: "check_all",
        first_non_finalized: SlotIndex(7),
        first_non_progressed: SlotIndex(9),
        generated: true,
        pending_generate: false,
        pending_validations_count: 2,
        validated_count: 4,
        has_notarized: true,
        is_finalized: false,
    };

    let line = SessionTelemetry::format_consensus_state(&description, &snapshot);

    let session_id_hex = description.get_session_id().to_hex_string();
    // Slot indices render via `SlotIndex` `Display` (e.g. `s7`), so the
    // legacy line shape is `slot_nf=s<N>, slot_np=s<N>`. The pre-Track-G
    // formatter used `{:03}` width specs here, but `SlotIndex` `Display`
    // ignores the width hint, so the spec was a no-op — Track G (906-B)
    // dropped it to remove the misleading width. The expected line below
    // is byte-for-byte identical to the pre-drop output, so the legacy
    // log post-processing keeps working.
    let expected = format!(
        "Session {session_id_hex} ConsensusState: trigger=check_all, slot_nf=s7, \
        slot_np=s9, generated=true , pending_gen=false, pending_val=2, validated=4, \
        notarized=true, finalized=false",
    );
    assert_eq!(line, expected, "`ConsensusState` line must match legacy format byte-for-byte");
}
