/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for [`crate::session_runtime::SessionRuntime`].
//!
//! Included directly from `session_runtime.rs` via `#[path]` so tests can
//! reach the private accessor surface and the inner structs (`SlotMap`,
//! `DelayedActionQueue`, `ReceiverActivityState`) without widening
//! visibility. Mirrors the convention used by
//! `tests/test_session_telemetry.rs`.
//!
//! Scope:
//! - Bootstrap-handle accessors (`description`, `session_start_prev_blocks`,
//!   `stop_flag`).
//! - `SlotMap` passthroughs — lazy runtime creation, stage getters/setters,
//!   `clear_runtimes_below` preserves the entry shell.
//! - `DelayedActionQueue` passthroughs — `push`/`drain_due_one` ordering,
//!   `min_pending_expiration`, `delayed_actions_count`.
//! - `next_awake_time` discipline — `set_next_awake_time` lowers only,
//!   `reset_next_awake_time` re-seeds to `now + MAX_AWAKE_TIMEOUT`,
//!   `force_next_awake_time` is unconditional.
//! - `ReceiverActivityState` passthroughs — `record_activity` change-signal
//!   semantics, `force_active_weight` test-seeding behavior.
//! - `Debug` impl smoke test (verifies the manual implementation handles
//!   `SessionDescription` correctly without panicking).

use super::*;
use crate::{
    block::SlotIndex, session_description::SessionDescription, SessionId, SessionNode,
    SessionOptions,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime},
};
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, UInt256, ZeroizingBytes};

/*
    Test helpers
*/

/// Build a minimal `SessionDescription` for runtime tests.
///
/// Mirrors `tests/test_session_telemetry.rs::make_test_description` so the
/// runtime tests do not depend on telemetry test scaffolding.
fn make_test_description(node_count: u32) -> Arc<SessionDescription> {
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

    Arc::new(
        SessionDescription::new(
            &opts,
            SessionId::default(),
            1,
            &nodes,
            local_key,
            &shard,
            SystemTime::now(),
            None,
        )
        .expect("SessionDescription::new must succeed for test fixtures"),
    )
}

/// Construct a fresh `SessionRuntime` with the supplied parents/flag.
fn make_runtime(
    num_validators: usize,
    session_start_prev_blocks: Vec<BlockIdExt>,
    stop_flag: Arc<AtomicBool>,
) -> SessionRuntime {
    let description = make_test_description(num_validators as u32);
    SessionRuntime::new(
        SystemTime::now(),
        num_validators,
        description,
        session_start_prev_blocks,
        stop_flag,
    )
}

/// Convenience wrapper for tests that do not exercise bootstrap handles.
fn make_default_runtime(num_validators: usize) -> SessionRuntime {
    make_runtime(num_validators, Vec::new(), Arc::new(AtomicBool::new(false)))
}

/// Build a `BlockIdExt` with the supplied seqno so tests can compare
/// `session_start_prev_blocks` without depending on real hashes.
fn make_block_id(seqno: u32) -> BlockIdExt {
    BlockIdExt::with_params(
        ShardIdent::masterchain(),
        seqno,
        UInt256::default(),
        UInt256::default(),
    )
}

/*
    Bootstrap-handle accessors
*/

#[test]
fn description_accessor_returns_shared_arc() {
    let description = make_test_description(3);
    let runtime = SessionRuntime::new(
        SystemTime::now(),
        3,
        description.clone(),
        Vec::new(),
        Arc::new(AtomicBool::new(false)),
    );

    // The Arc handed back must point at the same allocation that was
    // supplied at construction — callers rely on `.clone()` producing an
    // identical `Arc<SessionDescription>` for closures spawned by the
    // callback aspect.
    assert!(Arc::ptr_eq(runtime.description(), &description));

    // Deref must reach the same data — node count is the cheapest cross-
    // check that survives `Arc::ptr_eq` regressions.
    assert_eq!(runtime.description().get_total_nodes(), 3);
}

#[test]
fn session_start_prev_blocks_accessor_returns_supplied_slice() {
    let prev = vec![make_block_id(1), make_block_id(2)];
    let runtime = make_runtime(3, prev.clone(), Arc::new(AtomicBool::new(false)));

    assert_eq!(runtime.session_start_prev_blocks(), prev.as_slice());
}

#[test]
fn stop_flag_observes_external_writes() {
    let flag = Arc::new(AtomicBool::new(false));
    let runtime = make_runtime(3, Vec::new(), flag.clone());

    assert!(!runtime.stop_flag().load(Ordering::Relaxed));

    flag.store(true, Ordering::Relaxed);

    // The runtime borrows the same `Arc<AtomicBool>`; setting through one
    // handle must be visible through the other.
    assert!(runtime.stop_flag().load(Ordering::Relaxed));
}

/*
    Slot operations — lazy runtime creation, stage getters/setters
*/

#[test]
fn slot_runtime_lazy_create_on_first_setter() {
    let mut runtime = make_default_runtime(3);
    let slot = SlotIndex::new(0);

    // Before any setter the slot has no entry at all.
    assert!(runtime.slot_entry(slot).is_none());

    let now = SystemTime::now();
    runtime.set_pending_generate(slot, true, now);

    // First setter creates both the entry and its `SlotRuntime`.
    let entry = runtime.slot_entry(slot).expect("slot entry must exist after setter");
    assert!(entry.runtime.is_some());
    assert!(runtime.is_pending_generate(slot));
}

#[test]
fn slot_stage_setters_round_trip_through_getters() {
    let mut runtime = make_default_runtime(3);
    let slot = SlotIndex::new(1);
    let now = SystemTime::now();

    runtime.set_pending_generate(slot, true, now);
    runtime.set_generated(slot, true, now);
    runtime.set_sent_generated(slot, true, now);
    runtime.set_first_candidate_received(slot, true, now);
    runtime.set_first_candidate_notarized(slot, true, now);
    runtime.set_first_candidate_finalized(slot, true, now);

    assert!(runtime.is_pending_generate(slot));
    assert!(runtime.is_generated(slot));
    assert!(runtime.is_sent_generated(slot));
    assert!(runtime.first_candidate_received(slot));
    assert!(runtime.first_candidate_notarized(slot));
    assert!(runtime.first_candidate_finalized(slot));
}

#[test]
fn slot_stage_getters_default_false_for_missing_slot() {
    let runtime = make_default_runtime(3);
    let slot = SlotIndex::new(42);

    assert!(!runtime.is_pending_generate(slot));
    assert!(!runtime.is_generated(slot));
    assert!(!runtime.is_sent_generated(slot));
    assert!(!runtime.first_candidate_received(slot));
    assert!(!runtime.first_candidate_notarized(slot));
    assert!(!runtime.first_candidate_finalized(slot));
}

#[test]
fn started_at_uses_runtime_when_present_and_fallback_otherwise() {
    let mut runtime = make_default_runtime(3);
    let slot = SlotIndex::new(0);
    let slot_creation_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let fallback = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);

    // No runtime yet — falls back.
    assert_eq!(runtime.started_at(slot, fallback), fallback);

    // After creating the runtime via a setter the start time is the value
    // supplied to the setter, not the fallback.
    runtime.set_pending_generate(slot, true, slot_creation_time);
    assert_eq!(runtime.started_at(slot, fallback), slot_creation_time);
}

#[test]
fn clear_runtimes_below_preserves_entry_shell() {
    let mut runtime = make_default_runtime(3);
    let now = SystemTime::now();

    for seqno in 0..5 {
        runtime.set_pending_generate(SlotIndex::new(seqno), true, now);
    }
    assert_eq!(runtime.slot_entry(SlotIndex::new(0)).unwrap().runtime.is_some(), true);

    runtime.clear_runtimes_below(SlotIndex::new(3));

    // [0, 3) — runtime is gone, but the `SlotEntry` shell remains so
    // outcome emission and history queries can still see the slot.
    for seqno in 0..3 {
        let entry = runtime.slot_entry(SlotIndex::new(seqno)).expect("entry shell must survive");
        assert!(entry.runtime.is_none(), "slot {} runtime should be cleared", seqno);
    }

    // [3, 5) — runtimes still alive.
    for seqno in 3..5 {
        let entry = runtime.slot_entry(SlotIndex::new(seqno)).expect("entry must exist");
        assert!(entry.runtime.is_some(), "slot {} runtime should still be present", seqno);
    }
}

/*
    Delayed-action queue
*/

#[test]
fn delayed_action_push_and_drain_returns_due_entries() {
    let mut runtime = make_default_runtime(3);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    runtime.post_delayed_action(now - Duration::from_secs(5), Box::new(|_| {}));
    runtime.post_delayed_action(now + Duration::from_secs(60), Box::new(|_| {}));
    runtime.post_delayed_action(now - Duration::from_secs(1), Box::new(|_| {}));

    assert_eq!(runtime.delayed_actions_count(), 3);

    // First drain — picks the first action whose expiration <= now (FIFO
    // linear scan, swap_remove). Both due entries should drain; the future
    // entry must remain.
    let first = runtime.drain_due_delayed_action(now).expect("first due drain");
    assert!(first.expiration_time <= now);

    let second = runtime.drain_due_delayed_action(now).expect("second due drain");
    assert!(second.expiration_time <= now);

    // Third drain returns None because the only remaining action is in
    // the future.
    assert!(runtime.drain_due_delayed_action(now).is_none());
    assert_eq!(runtime.delayed_actions_count(), 1);
}

#[test]
fn delayed_action_min_pending_expiration_reports_earliest() {
    let mut runtime = make_default_runtime(3);
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    runtime.post_delayed_action(base + Duration::from_secs(30), Box::new(|_| {}));
    runtime.post_delayed_action(base + Duration::from_secs(10), Box::new(|_| {}));
    runtime.post_delayed_action(base + Duration::from_secs(20), Box::new(|_| {}));

    assert_eq!(runtime.min_pending_delayed_expiration(), Some(base + Duration::from_secs(10)),);
}

#[test]
fn delayed_action_min_pending_expiration_none_when_queue_empty() {
    let runtime = make_default_runtime(3);
    assert!(runtime.min_pending_delayed_expiration().is_none());
}

/*
    `next_awake_time` discipline
*/

#[test]
fn set_next_awake_time_only_lowers() {
    let runtime = make_default_runtime(3);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    runtime.force_next_awake_time(now);

    // Earlier — lowers.
    let earlier = now - Duration::from_secs(5);
    runtime.set_next_awake_time(earlier);
    assert_eq!(runtime.get_next_awake_time(), earlier);

    // Later — no-op.
    let later = now + Duration::from_secs(10);
    runtime.set_next_awake_time(later);
    assert_eq!(runtime.get_next_awake_time(), earlier);
}

#[test]
fn reset_next_awake_time_seeds_to_now_plus_max_timeout() {
    let runtime = make_default_runtime(3);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    runtime.reset_next_awake_time(now);

    assert_eq!(runtime.get_next_awake_time(), now + MAX_AWAKE_TIMEOUT);
    assert_eq!(MAX_AWAKE_TIMEOUT, Duration::from_millis(10));
}

#[test]
fn force_next_awake_time_is_unconditional() {
    let runtime = make_default_runtime(3);
    let early = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let late = early + Duration::from_secs(3_600);

    runtime.force_next_awake_time(early);
    assert_eq!(runtime.get_next_awake_time(), early);

    // `force_*` overrides even when the supplied value is in the future —
    // unlike `set_next_awake_time` which would no-op.
    runtime.force_next_awake_time(late);
    assert_eq!(runtime.get_next_awake_time(), late);
}

/*
    Receiver-activity mirror
*/

#[test]
fn record_activity_signals_change_only_on_weight_diff() {
    let mut runtime = make_default_runtime(4);

    let ts = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let last = vec![Some(ts), None, Some(ts + Duration::from_secs(1)), None];

    // First write: previous weight 0 -> 100; counts as a change.
    assert!(runtime.record_activity(100, last.clone()));
    assert_eq!(runtime.active_weight(), 100);
    assert_eq!(runtime.last_activity(), last.as_slice());

    // Same weight, different `last_activity` payload: NOT a change (the
    // weight-equality test is what gates change-only telemetry side
    // effects; `last_activity` is read in snapshot builders independently).
    let mut last2 = last.clone();
    last2[1] = Some(ts + Duration::from_secs(2));
    assert!(!runtime.record_activity(100, last2.clone()));
    assert_eq!(runtime.active_weight(), 100);
    assert_eq!(runtime.last_activity(), last2.as_slice());

    // Different weight: change reported again.
    assert!(runtime.record_activity(150, last2.clone()));
    assert_eq!(runtime.active_weight(), 150);
}

#[test]
fn force_active_weight_does_not_emit_change_signal() {
    let mut runtime = make_default_runtime(2);

    runtime.force_active_weight(42);
    assert_eq!(runtime.active_weight(), 42);

    // After a force-seed the next `record_activity` with the same weight
    // is a no-change: it must compare against the seeded baseline.
    assert!(!runtime.record_activity(42, vec![None, None]));
}

#[test]
fn last_activity_indexed_by_validator() {
    let mut runtime = make_default_runtime(3);
    let ts = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // Initial state: all-None, length matches validator count.
    assert_eq!(runtime.last_activity().len(), 3);
    assert!(runtime.last_activity().iter().all(|s| s.is_none()));

    runtime.record_activity(1, vec![Some(ts), None, None]);
    assert_eq!(runtime.last_activity()[0], Some(ts));
    assert_eq!(runtime.last_activity()[1], None);
    assert_eq!(runtime.last_activity()[2], None);
}

/*
    Debug smoke test — the manual `Debug` impl must avoid the missing
    `Debug for SessionDescription` and still surface the rest of the
    scratch state without panicking.
*/

#[test]
fn debug_impl_renders_without_panicking() {
    let runtime = make_default_runtime(3);
    let formatted = format!("{:?}", runtime);

    // We do not pin the exact format string — it is informational — but
    // the renderer must include the entity name and the scratch fields,
    // and must elide the description.
    assert!(formatted.contains("SessionRuntime"));
    assert!(formatted.contains("session_start_prev_blocks"));
    assert!(formatted.contains("next_awake_time"));
    assert!(formatted.contains("scheduling"));
    assert!(formatted.contains("slots"));
    assert!(formatted.contains("receiver_activity"));
}
