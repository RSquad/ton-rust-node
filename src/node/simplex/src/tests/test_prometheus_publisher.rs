/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

//! Unit tests for [`crate::prometheus_publisher`].
//!
//! The tests target the internal [`MetricsSink`] seam so we never touch the
//! global `metrics-rs` recorder; this keeps the suite deterministic and
//! parallel-safe.

use super::{
    build_labels, publish_with_sink, sanitize_name, MetricsSink, SessionIdentity, SPEED_SUFFIX,
};
use crate::PrometheusLabels;
use consensus_common::utils::MetricsHandle;
use std::cell::RefCell;

/// Captured metric emission, owned by [`MockSink`].
#[derive(Clone, Debug, PartialEq)]
struct Captured {
    kind: CapturedKind,
    name: String,
    value_u64: u64,
    value_f64: f64,
    labels: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
enum CapturedKind {
    Counter,
    Gauge,
}

#[derive(Default)]
struct MockSink {
    inner: RefCell<Vec<Captured>>,
}

impl MetricsSink for MockSink {
    fn counter(&mut self, name: &str, value: u64, labels: &[(&str, String)]) {
        self.inner.borrow_mut().push(Captured {
            kind: CapturedKind::Counter,
            name: name.to_string(),
            value_u64: value,
            value_f64: 0.0,
            labels: labels.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect(),
        });
    }

    fn gauge(&mut self, name: &str, value: f64, labels: &[(&str, String)]) {
        self.inner.borrow_mut().push(Captured {
            kind: CapturedKind::Gauge,
            name: name.to_string(),
            value_u64: 0,
            value_f64: value,
            labels: labels.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect(),
        });
    }
}

impl MockSink {
    fn captured(&self) -> Vec<Captured> {
        self.inner.borrow().clone()
    }
}

const SHARD: &str = "0:8000000000000000";
const SESSION_ID8: &str = "2a5ea688";

fn identity() -> SessionIdentity<'static> {
    SessionIdentity { shard: SHARD, session_id8: SESSION_ID8 }
}

/// Build a real [`MetricsHandle`] populated with the given counters and
/// gauges, then snapshot it into a fresh [`MetricsDumper`]. We go through the
/// real `metrics-rs` registry so the test exercises the same
/// `enumerate_with_usage` path the production code uses, without forging
/// internal `Metric` values directly.
///
/// All names are `&'static str` because `metrics::Key::from_name` requires a
/// `SharedString = Cow<'static, str>` for cheap registration.
fn make_dumper(
    counters: &[(&'static str, u64)],
    gauges: &[(&'static str, f64)],
    derivatives: &[&'static str],
) -> consensus_common::utils::MetricsDumper {
    let handle = MetricsHandle::new(None);
    for (name, value) in counters {
        let counter = handle.sink().register_counter(&metrics::Key::from_name(*name));
        counter.absolute(*value);
    }
    for (name, value) in gauges {
        let gauge = handle.sink().register_gauge(&metrics::Key::from_name(*name));
        gauge.set(*value);
    }

    let mut dumper = consensus_common::utils::MetricsDumper::new();
    for name in derivatives {
        dumper.add_derivative_metric(*name);
    }
    // First update primes the snapshot; for derivative metrics we need a
    // second update to compute `.speed`. Tests that exercise `.speed`
    // semantics call `update` twice manually before invoking the publisher.
    dumper.update(&handle);
    dumper
}

#[test]
fn publishes_counter_with_shard_label() {
    let dumper = make_dumper(&[("simplex_votes_in_total", 22_060)], &[], &[]);
    let mut sink = MockSink::default();
    publish_with_sink(&dumper, PrometheusLabels::ShardOnly, identity(), &mut sink);

    let captured = sink.captured();
    assert_eq!(captured.len(), 1, "expected exactly one published series");
    let item = &captured[0];
    assert_eq!(item.kind, CapturedKind::Counter);
    assert_eq!(item.name, "ton_node_simplex_votes_in_total");
    assert_eq!(item.value_u64, 22_060);
    assert_eq!(item.labels, vec![("shard".to_string(), SHARD.to_string())]);
}

#[test]
fn publishes_gauge_with_shard_label() {
    // The dumper classifies a raw gauge as `MetricUsage::Float` only when the
    // registered name starts with `float:`; otherwise gauges are stored as
    // `Counter` and republished accordingly. Use the `float:` prefix here to
    // exercise the gauge branch of the publisher.
    //
    // Note: producers of `Float`-tagged values must pre-multiply by
    // `METRIC_FLOAT_MULTIPLIER` (10_000) because the dumper truncates the
    // gauge bits to `u64` and then divides by the multiplier on read. We
    // mirror that convention here so the round-trip yields `17.0`.
    let dumper = make_dumper(&[], &[("float:simplex_active_weight", 170_000.0)], &[]);
    let mut sink = MockSink::default();
    publish_with_sink(&dumper, PrometheusLabels::ShardOnly, identity(), &mut sink);

    let captured = sink.captured();
    assert_eq!(captured.len(), 1);
    let item = &captured[0];
    assert_eq!(item.kind, CapturedKind::Gauge);
    assert_eq!(item.name, "ton_node_simplex_active_weight");
    assert!((item.value_f64 - 17.0).abs() < f64::EPSILON, "expected 17.0, got {}", item.value_f64);
    assert_eq!(item.labels, vec![("shard".to_string(), SHARD.to_string())]);
}

#[test]
fn unprefixed_gauges_publish_as_counters_matching_dumper_behavior() {
    // Documents the existing `MetricsDumper` quirk: gauges registered without
    // a `float:` / `percents:` prefix are classified as `MetricUsage::Counter`
    // and therefore republished via `metrics::counter!`. This is intentional
    // for monotonic gauges like `simplex_last_finalized_slot` (which only
    // ever moves forward) but consumers must `irate()` / `rate()` accordingly.
    let dumper = make_dumper(&[], &[("simplex_last_finalized_slot", 12345.0)], &[]);
    let mut sink = MockSink::default();
    publish_with_sink(&dumper, PrometheusLabels::ShardOnly, identity(), &mut sink);

    let captured = sink.captured();
    assert_eq!(captured.len(), 1);
    let item = &captured[0];
    assert_eq!(item.kind, CapturedKind::Counter);
    assert_eq!(item.name, "ton_node_simplex_last_finalized_slot");
    assert_eq!(item.value_u64, 12345);
}

#[test]
fn drops_speed_keys() {
    // First update populates `prev_metrics`; second update with the same
    // handle promotes the registered derivative_metric into a `.speed` entry.
    let handle = MetricsHandle::new(None);
    let counter = handle.sink().register_counter(&"simplex_certs_in".into());
    let mut dumper = consensus_common::utils::MetricsDumper::new();
    dumper.add_derivative_metric("simplex_certs_in");

    counter.absolute(100);
    dumper.update(&handle);
    counter.absolute(220);
    dumper.update(&handle);

    let mut sink = MockSink::default();
    publish_with_sink(&dumper, PrometheusLabels::ShardOnly, identity(), &mut sink);

    let names: Vec<String> = sink.captured().into_iter().map(|c| c.name).collect();
    assert!(
        names.contains(&"ton_node_simplex_certs_in".to_string()),
        "raw counter should be published; got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("_speed")),
        "no `.speed` keys should reach Prometheus; got {names:?}"
    );
}

#[test]
fn label_strategy_shard_and_session_id_attaches_session_id() {
    let dumper = make_dumper(&[("simplex_votes_in_total", 1)], &[], &[]);
    let mut sink = MockSink::default();
    publish_with_sink(&dumper, PrometheusLabels::ShardAndSessionId, identity(), &mut sink);

    let captured = sink.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].labels,
        vec![
            ("shard".to_string(), SHARD.to_string()),
            ("session_id".to_string(), SESSION_ID8.to_string()),
        ]
    );
}

#[test]
fn name_sanitization_replaces_dot_and_strips_simplex_prefix() {
    assert_eq!(sanitize_name("simplex_collates.success"), "ton_node_simplex_collates_success");
    assert_eq!(
        sanitize_name("simplex_receiver_main_queue.posts"),
        "ton_node_simplex_receiver_main_queue_posts"
    );
    // Bare-suffix simplex_ key: stripped before the namespace is reapplied —
    // result is namespaced once, not `ton_node_simplex_simplex_…`.
    assert_eq!(sanitize_name("simplex_health_warnings"), "ton_node_simplex_health_warnings");
    // Unknown prefix: still gets the namespace.
    assert_eq!(sanitize_name("foreign_metric"), "ton_node_simplex_foreign_metric");
}

#[test]
fn name_sanitization_is_idempotent_on_already_namespaced_input() {
    // Calling sanitize_name on an already-namespaced output must not
    // double-prefix it (defensive guard for callers that pass post-publish
    // keys).
    assert_eq!(
        sanitize_name("ton_node_simplex_collates_success"),
        "ton_node_simplex_collates_success",
    );
    assert_eq!(
        sanitize_name("ton_node_simplex_simplex_health_warnings"),
        "ton_node_simplex_health_warnings",
        "the inner simplex_ is also stripped, idempotent on chained-prefix corner case",
    );
}

#[test]
fn build_labels_default_strategy_is_shard_only() {
    let labels = build_labels(PrometheusLabels::default(), identity());
    assert_eq!(labels, vec![("shard", SHARD.to_string())]);
}

#[test]
fn multiple_sessions_same_shard_distinct_labels() {
    // Two distinct sessions of the same shard, published with the
    // ShardAndSessionId strategy, should produce two distinct `(name, labels)`
    // tuples. We feed the same metric value through twice and assert the
    // captured emissions differ only in the `session_id` label.
    let dumper_a = make_dumper(&[("simplex_votes_in_total", 10)], &[], &[]);
    let dumper_b = make_dumper(&[("simplex_votes_in_total", 20)], &[], &[]);

    let mut sink = MockSink::default();
    publish_with_sink(
        &dumper_a,
        PrometheusLabels::ShardAndSessionId,
        SessionIdentity { shard: SHARD, session_id8: "11111111" },
        &mut sink,
    );
    publish_with_sink(
        &dumper_b,
        PrometheusLabels::ShardAndSessionId,
        SessionIdentity { shard: SHARD, session_id8: "22222222" },
        &mut sink,
    );

    let captured = sink.captured();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0].labels,
        vec![
            ("shard".to_string(), SHARD.to_string()),
            ("session_id".to_string(), "11111111".to_string()),
        ]
    );
    assert_eq!(captured[0].value_u64, 10);
    assert_eq!(
        captured[1].labels,
        vec![
            ("shard".to_string(), SHARD.to_string()),
            ("session_id".to_string(), "22222222".to_string()),
        ]
    );
    assert_eq!(captured[1].value_u64, 20);
}

#[test]
fn speed_suffix_constant_matches_dumper_convention() {
    // Sanity-check the constant against the convention used by
    // `MetricsDumper::update` when emitting derivative metrics. If this ever
    // drifts, the publisher will silently start re-publishing rates.
    assert_eq!(SPEED_SUFFIX, ".speed");
}
