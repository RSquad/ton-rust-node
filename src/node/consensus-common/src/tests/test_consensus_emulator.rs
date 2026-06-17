/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for `consensus_emulator.rs` helpers.
//!
//! The integration suite in
//! `node/consensus-common/tests/test_consensus_emulator.rs` exercises full
//! emulator lifecycles end-to-end. It is currently affected by an upstream
//! `Ed25519KeyOption<S: SecretBytes>` generic-parameter change that is
//! patched in PR #904's rebase-compat commit, so on PR #901's standalone
//! branch it does not compile from the integration-test root. The tests
//! below cover the pure helpers and validators introduced for Track C
//! (review findings 901-A / 901-B / 901-C / 901-F) so the behavior is
//! verifiable from `cargo test -p consensus-common --lib` even on PR #901's
//! standalone branch.

use super::*;

#[test]
fn test_roll_trigger_instant_inclusive_max_is_reachable() {
    // Regression for 901-A: prior `lo + (rng.gen_f64() * range) as u64`
    // never yielded `max_ms`. With integer `lo..=hi` sampling both
    // endpoints are reachable. We use a tight `[0, 1]` window and a
    // seeded RNG so the test is deterministic.
    let mut rng = RngState::Seeded(SmallRng::seed_from_u64(0xC0FFEE));
    let spec = EmulatorDelaySpec::new(0, 1, 0.0);
    let mut seen_min = false;
    let mut seen_max = false;
    // 64 rolls is enough to hit both endpoints with seed=0xC0FFEE;
    // bump the bound if you reseed.
    for _ in 0..64 {
        let lo = spec.min_ms.min(spec.max_ms);
        let hi = spec.min_ms.max(spec.max_ms);
        let v = rng.gen_range_u64_inclusive(lo, hi);
        if v == 0 {
            seen_min = true;
        }
        if v == 1 {
            seen_max = true;
        }
        if seen_min && seen_max {
            break;
        }
    }
    assert!(seen_min, "min_ms (0) was never sampled");
    assert!(seen_max, "max_ms (1) was never sampled — 901-A regression");
}

#[test]
fn test_roll_trigger_instant_constant_delay_when_min_eq_max() {
    let mut rng = RngState::Seeded(SmallRng::seed_from_u64(7));
    let v = rng.gen_range_u64_inclusive(42, 42);
    assert_eq!(v, 42);
}

#[test]
fn test_emulator_params_rejects_slot_interval_skip() {
    // Regression for 901-B: a non-zero skip probability on the slot
    // tick would permanently stall slot generation.
    let mut p = EmulatorParams::default();
    p.slot_interval = EmulatorDelaySpec::new(10, 20, 0.1);
    let err = p.validate().expect_err("expected validation to reject");
    let msg = format!("{err}");
    assert!(
        msg.contains("slot_interval") && msg.contains("skip_probability"),
        "unexpected error message: {msg}",
    );
}

#[test]
fn test_emulator_params_accepts_zero_skip_on_slot_interval() {
    let mut p = EmulatorParams::default();
    p.slot_interval = EmulatorDelaySpec::new(10, 20, 0.0);
    p.observed_body_delay = EmulatorDelaySpec::new(1, 2, 0.5);
    p.observed_notar_delay = EmulatorDelaySpec::new(2, 3, 0.5);
    p.finalized_delay = EmulatorDelaySpec::new(3, 4, 0.5);
    p.validate().expect("zero skip on slot_interval + non-zero on others must be Ok");
}

#[test]
fn test_clamp_trigger_instants_monotonic_orders_inverted() {
    // Regression for 901-F: when rolled delays invert the documented
    // observation order, the clamp restores it.
    let now = Instant::now();
    let t0 = now + Duration::from_millis(30); // body
    let t1 = now + Duration::from_millis(10); // notar (would fire before body)
    let t2 = now + Duration::from_millis(20); // finalized (would fire before body)
    let [body, notar, finalized] = clamp_trigger_instants_monotonic([Some(t0), Some(t1), Some(t2)]);
    let body = body.unwrap();
    let notar = notar.unwrap();
    let finalized = finalized.unwrap();
    assert_eq!(body, t0);
    assert_eq!(notar, t0, "notar must be clamped to body");
    assert_eq!(finalized, t0, "finalized must be clamped to notar (which was clamped to body)");
}

#[test]
fn test_clamp_trigger_instants_monotonic_passthrough_when_in_order() {
    let now = Instant::now();
    let t0 = now + Duration::from_millis(10);
    let t1 = now + Duration::from_millis(20);
    let t2 = now + Duration::from_millis(30);
    let [a, b, c] = clamp_trigger_instants_monotonic([Some(t0), Some(t1), Some(t2)]);
    assert_eq!(a, Some(t0));
    assert_eq!(b, Some(t1));
    assert_eq!(c, Some(t2));
}

#[test]
fn test_clamp_trigger_instants_monotonic_skips_do_not_set_floor() {
    // A skipped (None) entry must NOT advance the floor for subsequent
    // entries: if `body` is skipped but `notar` and `finalized` are not,
    // those should still be ordered relative to each other, not to a
    // phantom `body` floor that was never picked.
    let now = Instant::now();
    let body = None;
    let notar = Some(now + Duration::from_millis(50));
    let finalized = Some(now + Duration::from_millis(20));
    let [b, n, f] = clamp_trigger_instants_monotonic([body, notar, finalized]);
    assert_eq!(b, None);
    assert_eq!(n, notar);
    // finalized was earlier than notar; clamp must push it to notar's instant.
    assert_eq!(f, notar);
}

#[test]
fn test_clamp_trigger_instants_monotonic_propagates_skip() {
    let now = Instant::now();
    let body = Some(now + Duration::from_millis(10));
    let notar = None;
    let finalized = Some(now + Duration::from_millis(5));
    let [b, n, f] = clamp_trigger_instants_monotonic([body, notar, finalized]);
    assert_eq!(b, body);
    assert_eq!(n, None, "skipped entries pass through unchanged");
    // After the None gap, floor is still `body`, so finalized is clamped to body.
    assert_eq!(f, body);
}

#[test]
fn test_candidate_retention_slots_constant_is_large_enough_for_default_window() {
    // 901-C: the retention window must be safely larger than any
    // reasonable `slots_per_leader_window` so leader-window chaining
    // never references an evicted parent.
    assert!(
        CANDIDATE_RETENTION_SLOTS >= 16,
        "retention window {CANDIDATE_RETENTION_SLOTS} is too small for safe chaining"
    );
}
