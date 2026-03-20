/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::telemetry::{Metric, MetricBuilder};
use rand::Rng;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const PERIOD_NANOS: u64 = 200_000_000; // 200ms per period

fn make_builder() -> Arc<MetricBuilder> {
    MetricBuilder::with_metric_and_period(Metric::with_total_amount("test", 5), PERIOD_NANOS)
}

#[test]
fn test_adnl_ongoing_value_accumulates_within_period() {
    let b = make_builder();
    b.update(1000);
    b.update(2000);
    b.update(3000);
    assert_eq!(b.ongoing_value(), 6000);
}

#[test]
fn test_adnl_ongoing_value_resets_after_period_boundary() {
    let b = make_builder();
    b.update(1000);
    b.update(2000);
    b.update(3000);
    assert_eq!(b.ongoing_value(), 6000);

    thread::sleep(Duration::from_millis(250));
    // Flush via update(0) — value resets, previous total moves to metric.current
    b.update(0);
    assert_eq!(b.ongoing_value(), 0);
    assert_eq!(b.metric().current(), 6000);
}

#[test]
fn test_adnl_ongoing_value_independent_of_previous_period() {
    let b = make_builder();
    b.update(30_000);

    thread::sleep(Duration::from_millis(250));
    b.update(500);
    // metric.current = 30000 (previous period), ongoing_value = 500 (current period)
    assert_eq!(b.metric().current(), 30_000);
    assert_eq!(b.ongoing_value(), 500);
}

#[test]
fn test_adnl_ongoing_value_across_two_period_boundaries() {
    let b = make_builder();
    b.update(8000);

    thread::sleep(Duration::from_millis(250));
    b.update(400);
    assert_eq!(b.ongoing_value(), 400);

    thread::sleep(Duration::from_millis(250));
    b.update(0);
    assert_eq!(b.ongoing_value(), 0);
    assert_eq!(b.metric().current(), 400);
}

#[test]
fn test_adnl_ongoing_value_zero_when_idle() {
    let b = make_builder();
    thread::sleep(Duration::from_millis(250));
    b.update(0);
    assert_eq!(b.ongoing_value(), 0);
}

#[test]
fn test_adnl_ongoing_value_no_overlimit_under_sustained_load() {
    const LIMIT: u64 = 1_000_000;
    const TARGET_PER_PERIOD: u64 = 900_000; // 90% of limit
    const SLEEP_BETWEEN_UPDATES_MS: u64 = 2;

    let b = make_builder();
    let mut rng = rand::thread_rng();
    let start = Instant::now();
    let mut total_updates = 0u64;

    while start.elapsed() < Duration::from_secs(10) {
        let period_start = Instant::now();
        let mut sent_this_period = 0u64;

        while sent_this_period < TARGET_PER_PERIOD {
            let remaining = TARGET_PER_PERIOD - sent_this_period;
            let packet_size: u64 = rng.gen_range(500..=5000).min(remaining);

            // Mirrors the production check: flush + ongoing_value
            b.update(0);
            let cur = b.ongoing_value();
            assert!(
                cur + packet_size <= LIMIT,
                "False overlimit at {:?}, update {total_updates}: \
                 ongoing_value()={cur}, packet={packet_size}, limit={LIMIT}, \
                 sent_this_period={sent_this_period}",
                start.elapsed(),
            );

            b.update(packet_size);
            sent_this_period += packet_size;
            total_updates += 1;

            thread::sleep(Duration::from_millis(SLEEP_BETWEEN_UPDATES_MS));
        }

        // Wait for the period boundary
        let elapsed_in_period = period_start.elapsed();
        if elapsed_in_period < Duration::from_millis(210) {
            thread::sleep(Duration::from_millis(210) - elapsed_in_period);
        }
    }

    assert!(total_updates > 100, "Expected many updates, got {total_updates}");
}
