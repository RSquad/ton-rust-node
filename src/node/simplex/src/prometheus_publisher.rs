/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

//! Prometheus republisher for per-session Simplex metrics.
//!
//! # Overview
//!
//! Each Simplex session keeps its own private [`MetricsHandle`] (one for the
//! `SessionProcessor`, one for the `Receiver`) and dumps it through
//! [`MetricsDumper`] every 15 s / 30 s. Those dumps live only in the log
//! stream; nothing reaches the global [`metrics_exporter_prometheus`] recorder
//! that backs the node's `/metrics` HTTP endpoint.
//!
//! [`publish_snapshot`] bridges the two: it walks the dumper's most-recent
//! snapshot, drops the `.speed` derivative variants (Prometheus computes those
//! itself with `rate()` / `irate()`), sanitises metric names to the
//! `ton_node_simplex_*` family and forwards each `(name, value)` pair to the
//! global recorder via the `metrics::counter!` / `metrics::gauge!` macros with
//! the supplied labels attached.
//!
//! # Multi-session disambiguation
//!
//! Many Simplex sessions can run in parallel (one per shard, rotating per
//! validator-set epoch). The caller selects a label strategy via
//! [`crate::PrometheusLabels`] in [`crate::SessionOptions`]; the resulting
//! `(name, value)` pairs are demultiplexed in Prometheus by the configured
//! label set (e.g. `shard="0:8000000000000000"`).
//!
//! # Why not push directly via `metrics::*` everywhere
//!
//! Per-session `MetricsHandle`s use the `metrics-rs` `Recency` filter, which
//! prunes counters that have not been incremented for the configured idle
//! timeout. Replacing them with the global recorder would lose that pruning
//! and require touching every increment site. Republishing snapshots keeps the
//! existing dump infrastructure intact (logs still work, warmup-aware first
//! dump still applies) while exporting the same data.

use crate::PrometheusLabels;
use consensus_common::utils::{MetricUsage, MetricsDumper};

/// Metric-name prefix for every Simplex series republished to Prometheus.
///
/// Matches the `ton_node_*` convention used by other engine-level metrics
/// declared in [`init_prometheus_recorder`](../node/src/engine.rs).
const METRIC_PREFIX: &str = "ton_node_simplex_";

/// Tag identifying a `.speed` derivative key in the dumper output. Such keys
/// are skipped because Prometheus computes rates with `rate()` / `irate()`
/// from the raw counter value.
const SPEED_SUFFIX: &str = ".speed";

/// Identity describing one Simplex session for labelling purposes.
///
/// The publisher holds borrowed references; values are formatted lazily based
/// on the configured [`PrometheusLabels`] strategy so call sites can build the
/// identity once per session and reuse it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionIdentity<'a> {
    /// Shard identifier, e.g. `0:8000000000000000` or `-1:8000000000000000`.
    pub shard: &'a str,
    /// First 8 hex characters of the session id; used by
    /// [`PrometheusLabels::ShardAndSessionId`]. Matches the `sid8` prefix
    /// shown in log dumps and monitoring reports.
    pub session_id8: &'a str,
}

/// Republish every metric in `dumper`'s latest snapshot to the global
/// Prometheus recorder, attaching labels derived from `identity` and
/// `strategy`.
///
/// Invariants:
/// * `*.speed` keys are silently dropped (Prometheus computes rates itself).
/// * [`MetricUsage::Counter`] becomes a Prometheus counter via
///   `metrics::counter!` `.absolute(value)`; everything else becomes a gauge.
/// * Names are mapped to `ton_node_simplex_<sanitised>` where `.` and `:` in
///   the original key are replaced with `_` to satisfy Prometheus naming rules.
///
/// This function is safe to call from any thread; it must be invoked after
/// [`MetricsDumper::update`] (so `prev_metrics` is populated) and is intended
/// to share the existing dump cadence in the receiver / session loops.
pub(crate) fn publish_snapshot(
    dumper: &MetricsDumper,
    strategy: PrometheusLabels,
    identity: SessionIdentity<'_>,
) {
    publish_with_sink(dumper, strategy, identity, &mut GlobalMetricsSink);
}

/// Internal sink so that unit tests can capture the published metrics without
/// touching the global recorder. Production code uses [`GlobalMetricsSink`].
trait MetricsSink {
    fn counter(&mut self, name: &str, value: u64, labels: &[(&str, String)]);
    fn gauge(&mut self, name: &str, value: f64, labels: &[(&str, String)]);
}

struct GlobalMetricsSink;

impl MetricsSink for GlobalMetricsSink {
    fn counter(&mut self, name: &str, value: u64, labels: &[(&str, String)]) {
        // `metrics` 0.24's `counter!` macro accepts variadic key-value labels
        // but cannot consume a slice directly; build a `Vec<Label>` instead so
        // we can pass any number of labels picked by the strategy.
        // `metrics::Label::new` requires `Into<SharedString>` (i.e. owned),
        // so the keys are converted via `String::from` here.
        let labels: Vec<metrics::Label> =
            labels.iter().map(|(k, v)| metrics::Label::new(String::from(*k), v.clone())).collect();
        metrics::counter!(name.to_string(), labels).absolute(value);
    }

    fn gauge(&mut self, name: &str, value: f64, labels: &[(&str, String)]) {
        let labels: Vec<metrics::Label> =
            labels.iter().map(|(k, v)| metrics::Label::new(String::from(*k), v.clone())).collect();
        metrics::gauge!(name.to_string(), labels).set(value);
    }
}

fn publish_with_sink<S: MetricsSink>(
    dumper: &MetricsDumper,
    strategy: PrometheusLabels,
    identity: SessionIdentity<'_>,
    sink: &mut S,
) {
    let labels = build_labels(strategy, identity);

    dumper.enumerate_with_usage(|key, value, usage| {
        if key.ends_with(SPEED_SUFFIX) {
            return;
        }
        let name = sanitize_name(&key);
        match usage {
            MetricUsage::Counter => {
                // `value` is f64 here only because `enumerate_with_usage`
                // unifies the API; for counters we know it carries an integer
                // count. Saturate at 0 if rounding yields negatives (cannot
                // happen for non-decreasing counters but defends against
                // future MetricUsage additions).
                let int_value = if value.is_finite() && value >= 0.0 { value as u64 } else { 0 };
                sink.counter(&name, int_value, &labels);
            }
            MetricUsage::Derivative
            | MetricUsage::Percents
            | MetricUsage::Float
            | MetricUsage::Latency => {
                sink.gauge(&name, value, &labels);
            }
        }
    });
}

fn build_labels(
    strategy: PrometheusLabels,
    identity: SessionIdentity<'_>,
) -> Vec<(&'static str, String)> {
    let mut labels: Vec<(&'static str, String)> = Vec::with_capacity(2);
    labels.push(("shard", identity.shard.to_string()));
    match strategy {
        PrometheusLabels::ShardOnly => {}
        PrometheusLabels::ShardAndSessionId => {
            labels.push(("session_id", identity.session_id8.to_string()));
        }
    }
    labels
}

/// Map an internal metric key (e.g. `simplex_collates.success`,
/// `simplex_receiver_main_queue.posts`) to a Prometheus-safe name with the
/// shared `ton_node_simplex_` prefix.
///
/// Idempotent: callers normally pass `simplex_*` keys from the dumper, but
/// an already-namespaced `ton_node_simplex_*` input is also handled — both
/// are stripped before the prefix is reapplied so the published name always
/// has exactly one `ton_node_simplex_` namespace.
///
/// Only `.` and `:` are replaced because the existing simplex keyspace is
/// already lowercase ASCII alphanumeric + `_` + the two listed separators.
/// If new key shapes are introduced later, extend [`is_invalid`] to keep the
/// invariant that the published name matches `[A-Za-z_][A-Za-z0-9_:]*`.
fn sanitize_name(key: &str) -> String {
    // Strip the namespace first so we don't double-prefix an
    // already-namespaced key, then strip the inner `simplex_` so the
    // expected dumper-key shape collapses to its bare suffix.
    let after_namespace = key.strip_prefix(METRIC_PREFIX).unwrap_or(key);
    let stripped = after_namespace.strip_prefix("simplex_").unwrap_or(after_namespace);
    let mut out = String::with_capacity(METRIC_PREFIX.len() + stripped.len());
    out.push_str(METRIC_PREFIX);
    for ch in stripped.chars() {
        if is_invalid(ch) {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_invalid(ch: char) -> bool {
    matches!(ch, '.' | ':')
}

#[cfg(test)]
#[path = "tests/test_prometheus_publisher.rs"]
mod tests;
