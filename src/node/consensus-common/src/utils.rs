/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Common utility functions shared between consensus implementations.
//!
//! This module contains utilities that don't depend on catchain-specific types.

use crate::PublicKeyHash;
use std::collections::{BTreeMap, HashMap, HashSet};
use ton_block::{KeyId, UInt256};

mod metrics;
pub use self::metrics::MetricsHandle;

/*
    serialization macros
*/

/// Macro for serialization of TL bare objects
#[macro_export]
macro_rules! serialize_tl_bare_object {
    ($($args:expr),*) => {{
        let mut ret = ton_api::ton::bytes::default();
        let mut serializer = ton_api::Serializer::new(&mut ret.0);
        $(serializer.write_bare($args).unwrap();)*
        ret
    }};
}

/// Macro for serialization of TL boxed objects
#[macro_export]
macro_rules! serialize_tl_boxed_object {
    ($($args:expr),*) => {{
        let mut ret = ton_api::ton::bytes::default();
        let mut serializer = ton_api::Serializer::new(&mut ret);
        $(serializer.write_boxed($args).unwrap();)*
        ret
    }};
}

/// Deserialize a TL bare object from raw bytes
pub fn deserialize_tl_bare_object<T: ::ton_api::BareDeserialize>(
    bytes: &crate::RawBuffer,
) -> crate::Result<T> {
    let mut cursor = std::io::Cursor::new(bytes);
    ton_api::Deserializer::new(&mut cursor).read_bare()
}

/// Deserialize a TL boxed object from raw bytes
pub fn deserialize_tl_boxed_object<T: ::ton_api::BoxedDeserialize>(
    bytes: &crate::RawBuffer,
) -> crate::Result<T> {
    let mut cursor = std::io::Cursor::new(bytes);
    ton_api::Deserializer::new(&mut cursor).read_boxed()
}

/*
    hash utilities
*/

/// Calculate hash of raw bytes data
pub fn get_hash(data: &crate::RawBuffer) -> crate::BlockHash {
    UInt256::calc_file_hash(data)
}

/// Calculate hash of block payload data
pub fn get_hash_from_block_payload(data: &crate::BlockPayloadPtr) -> crate::BlockHash {
    UInt256::calc_file_hash(data.data())
}

/*
    type conversions
*/

pub(crate) fn int256_to_public_key_hash(public_key: &UInt256) -> PublicKeyHash {
    KeyId::from_data(*public_key.as_slice())
}

pub(crate) fn public_key_hashes_to_string(v: &[PublicKeyHash]) -> String {
    let mut result: String = "[".to_string();
    let mut first = true;

    for key in v {
        if !first {
            result += ", ";
        } else {
            first = false;
        }

        result = format!("{}{}", result, key);
    }

    result + "]"
}

/*
    to string conversions
*/

pub fn bytes_to_string(v: &::ton_api::ton::bytes) -> String {
    hex::encode(v)
}

pub fn time_to_string(time: &std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::offset::Utc> = (*time).into();
    datetime.format("%Y-%m-%d %T.%f").to_string()
}

pub fn time_to_timestamp_string(time: &std::time::SystemTime) -> String {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(timestamp) => format!("{:.3}", timestamp.as_millis() as f64 / 1000.0),
        Err(err) => err.to_string(),
    }
}

/*
    hex parsing utilities
*/

pub fn parse_hex(hex_asm: &str) -> Vec<u8> {
    let mut hex_bytes = hex_asm
        .as_bytes()
        .iter()
        .filter_map(|b| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        })
        .fuse();

    let mut bytes = Vec::new();
    while let (Some(h), Some(l)) = (hex_bytes.next(), hex_bytes.next()) {
        bytes.push(h << 4 | l)
    }

    bytes
}

pub fn parse_hex_to_array(hex_asm: &str, dst: &mut [u8]) {
    assert!(dst.len() * 2 >= hex_asm.len());
    dst.iter_mut().for_each(|x| *x = 0);

    for (i, c) in hex_asm.chars().enumerate() {
        dst[i / 2] = dst[i / 2] * 16 + u8::from_str_radix(&c.to_string(), 16).unwrap();
    }
}

pub fn parse_hex_as_int256(hex_asm: &str) -> ton_block::UInt256 {
    let hex_bytes = parse_hex(hex_asm);
    ton_block::UInt256::from_slice(hex_bytes.as_slice())
}

pub fn parse_hex_as_bytes(hex_asm: &str) -> ::ton_api::ton::bytes {
    parse_hex(hex_asm)
}

pub(crate) fn parse_hex_as_public_key(hex_asm: &str) -> crate::PublicKey {
    use ton_api::deserialize_typed;
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    let key = deserialize_typed::<ton_api::ton::PublicKey>(key_slice).unwrap();
    (&key).try_into().unwrap()
}

pub(crate) fn parse_hex_as_public_key_hash(hex_asm: &str) -> crate::PublicKeyHash {
    let mut key_slice = [0; 32];
    parse_hex_to_array(hex_asm, &mut key_slice);
    ton_block::KeyId::from_data(key_slice)
}

pub(crate) fn parse_hex_as_session_id(hex_asm: &str) -> crate::SessionId {
    parse_hex_as_int256(hex_asm)
}

pub(crate) fn parse_hex_as_private_key(hex_asm: &str) -> crate::PrivateKey {
    use ton_block::Ed25519KeyOption;
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    assert!(key_slice.len() == 32);
    Ed25519KeyOption::from_private_key(key_slice.as_slice().try_into().unwrap()).unwrap()
}

pub(crate) fn parse_hex_as_expanded_private_key(hex_asm: &str) -> crate::PrivateKey {
    use ton_block::Ed25519KeyOption;
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    assert!(key_slice.len() == 64);
    Ed25519KeyOption::from_expanded_key(key_slice.as_slice().try_into().unwrap()).unwrap()
}

/*
    time utilities
*/

pub fn get_elapsed_time(from_time: &std::time::SystemTime) -> std::time::Duration {
    if let Ok(latency) = from_time.elapsed() {
        latency
    } else {
        std::time::Duration::ZERO
    }
}

/*
   metrics
*/

/// Classification of a metric stored in [`Metric`].
///
/// Surfaced through [`MetricsDumper::enumerate_with_usage`] so downstream
/// consumers (e.g. the Prometheus publisher) can route counters and gauges
/// to the appropriate sink without re-reading the metrics registry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MetricUsage {
    /// Monotonically non-decreasing counter (raw `u64` value).
    Counter,
    /// Per-second derivative computed by the dumper.
    Derivative,
    /// Percentage in `[0.0, 100.0]`.
    Percents,
    /// Generic floating-point gauge.
    Float,
    /// Latency expressed in seconds.
    Latency,
}

pub struct Metric {
    value: u64,
    usage: MetricUsage,
}

type MetricsTree = Box<dyn Fn(&str, &BTreeMap<String, Metric>) -> Option<Metric>>;

pub struct MetricsDumper {
    prev_metrics: BTreeMap<String, Metric>,
    compute_handlers: HashMap<String, MetricsTree>,
    derivative_metrics: HashSet<String>,
    last_dump_time: std::time::SystemTime,
}

impl Default for MetricsDumper {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsDumper {
    pub const METRIC_DERIVATIVE_MULTIPLIER: f64 = 1000000.0;
    pub const METRIC_FLOAT_MULTIPLIER: f64 = 10000.0;
    pub const METRIC_TIME_MULTIPLIER: f64 = 1000.0;

    pub fn add_compute_handler<F>(&mut self, key: impl ToString, handler: F)
    where
        F: Fn(&str, &BTreeMap<String, Metric>) -> Option<Metric>,
        F: 'static,
    {
        self.compute_handlers.insert(key.to_string(), Box::new(handler));
    }

    pub fn add_derivative_metric(&mut self, key: impl ToString) {
        self.derivative_metrics.insert(key.to_string());
    }

    fn update_histogram(
        &mut self,
        metrics: &mut BTreeMap<String, Metric>,
        mut basic_key: String,
        mut values: Vec<u64>,
    ) {
        let (mut last, mut avg, mut med, mut min, mut max) = if !values.is_empty() {
            let last = values[values.len() - 1] as f64;
            let avg = values.iter().sum::<u64>() as f64 / values.len() as f64;

            values.sort();

            let med = values[values.len() / 2] as f64;
            let min = values[0] as f64;
            let max = values[values.len() - 1] as f64;

            (last, avg, med, min, max)
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };

        let mut usage = MetricUsage::Float;

        if let Some(stripped_basic_key) = basic_key.strip_prefix("time:") {
            basic_key = stripped_basic_key.to_string();
            usage = MetricUsage::Latency;
            last /= Self::METRIC_TIME_MULTIPLIER;
            avg /= Self::METRIC_TIME_MULTIPLIER;
            med /= Self::METRIC_TIME_MULTIPLIER;
            min /= Self::METRIC_TIME_MULTIPLIER;
            max /= Self::METRIC_TIME_MULTIPLIER;
        }

        metrics.insert(
            format!("{}.last", basic_key),
            Metric { value: (last * Self::METRIC_FLOAT_MULTIPLIER) as u64, usage },
        );
        metrics.insert(
            format!("{}.avg", basic_key),
            Metric { value: (avg * Self::METRIC_FLOAT_MULTIPLIER) as u64, usage },
        );
        metrics.insert(
            format!("{}.med", basic_key),
            Metric { value: (med * Self::METRIC_FLOAT_MULTIPLIER) as u64, usage },
        );
        metrics.insert(
            format!("{}.min", basic_key),
            Metric { value: (min * Self::METRIC_FLOAT_MULTIPLIER) as u64, usage },
        );
        metrics.insert(
            format!("{}.max", basic_key),
            Metric { value: (max * Self::METRIC_FLOAT_MULTIPLIER) as u64, usage },
        );
        metrics.insert(
            format!("{}.cnt", basic_key),
            Metric { value: values.len() as u64, usage: MetricUsage::Counter },
        );
    }

    pub fn update(&mut self, metrics_receiver: &metrics::MetricsHandle) {
        //convert metrics

        let mut metrics: BTreeMap<String, Metric> = BTreeMap::new();

        let snapshot = metrics_receiver.snapshot();

        for (k, v) in snapshot.counters {
            metrics.insert(k.to_string(), Metric { value: v, usage: MetricUsage::Counter });
        }

        for (mut key, v) in snapshot.gauges {
            let mut usage = MetricUsage::Counter;

            if let Some(stripped_basic_key) = key.strip_prefix("percents:") {
                usage = MetricUsage::Percents;
                key = stripped_basic_key.to_string();
            } else if let Some(stripped_basic_key) = key.strip_prefix("float:") {
                key = stripped_basic_key.to_string();
                usage = MetricUsage::Float;
            }
            metrics.insert(key, Metric { value: v as u64, usage });
        }

        for (k, v) in snapshot.histograms {
            self.update_histogram(&mut metrics, k.to_string(), v);
        }

        //snapshot time

        let duration = get_elapsed_time(&self.last_dump_time).as_secs_f64();
        self.last_dump_time = std::time::SystemTime::now();

        //compute metrics

        let mut unprocessed_handlers = HashSet::new();

        for (key, handler) in &self.compute_handlers {
            if let Some(value) = handler(key, &metrics) {
                metrics.insert(key.to_string(), value);
            } else {
                unprocessed_handlers.insert(key.to_string());
            }
        }

        //compute derivative metrics

        for key in &self.derivative_metrics {
            if let Some(value) = metrics.get(key) {
                if let Some(prev_value) = self.prev_metrics.get(key) {
                    let delta = (value.value as isize - prev_value.value as isize) as f64;
                    let derivative = (delta / duration * Self::METRIC_DERIVATIVE_MULTIPLIER) as u64;

                    metrics.insert(
                        format!("{}.speed", key),
                        Metric { value: derivative, usage: MetricUsage::Derivative },
                    );
                }
            }
        }

        //second pass for recursive compute handlers

        for key in unprocessed_handlers {
            if let Some(handler) = self.compute_handlers.get(&key) {
                if let Some(value) = handler(&key, &metrics) {
                    metrics.insert(key.to_string(), value);
                }
            }
        }

        //update state

        self.prev_metrics = metrics;
    }

    pub fn enumerate_as_f64<F>(&self, handler: F)
    where
        F: Fn(String, f64),
    {
        self.enumerate_with_usage(|key, value, _kind| handler(key, value));
    }

    /// Like [`Self::enumerate_as_f64`] but also yields the metric's
    /// [`MetricUsage`], so consumers (e.g. the Prometheus publisher) can
    /// distinguish counters from gauges without re-reading the registry.
    pub fn enumerate_with_usage<F>(&self, mut handler: F)
    where
        F: FnMut(String, f64, MetricUsage),
    {
        for (key, metric) in &self.prev_metrics {
            use MetricUsage::*;

            let value = match metric.usage {
                Counter => metric.value as f64,
                Derivative => metric.value as f64 / Self::METRIC_DERIVATIVE_MULTIPLIER,
                Percents => (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER * 100.0,
                Float => (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER,
                Latency => (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER,
            };

            handler(key.clone(), value, metric.usage);
        }
    }

    pub fn dump<F>(&self, handler: F)
    where
        F: Fn(String),
    {
        for (key, metric) in &self.prev_metrics {
            use MetricUsage::*;

            let metric_dump = match metric.usage {
                Counter => format!("{}", metric.value),
                Derivative => {
                    let value = metric.value as f64 / Self::METRIC_DERIVATIVE_MULTIPLIER;

                    let (multiplier, suffix) = if value > 1000000.0 {
                        (1000000.0, "M")
                    } else if value > 1000.0 {
                        (1000.0, "K")
                    } else {
                        (1.0, "")
                    };

                    format!("{:.2}{}/s", value / multiplier, suffix)
                }
                Percents => {
                    format!("{:.1}%", (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER * 100.0)
                }
                Float => format!("{:.2}", (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER),
                Latency => format!("{:.3}s", (metric.value as f64) / Self::METRIC_FLOAT_MULTIPLIER),
            };

            handler(format!("    {:12} - {}", metric_dump, key));
        }
    }

    pub fn new() -> MetricsDumper {
        MetricsDumper {
            last_dump_time: std::time::SystemTime::now(),
            prev_metrics: BTreeMap::new(),
            compute_handlers: HashMap::new(),
            derivative_metrics: HashSet::new(),
        }
    }
}

fn get_metrics_counters_pair(
    metrics: &BTreeMap<String, Metric>,
    key1: &str,
    key2: &str,
) -> Option<(u64, u64)> {
    let value1 = metrics.get(key1);
    let value1 = match value1 {
        Some(value) => value,
        _ => return None,
    };

    let value2 = metrics.get(key2);
    let value2 = match value2 {
        Some(value) => value,
        _ => return None,
    };

    if matches!(value1.usage, MetricUsage::Counter) && matches!(value2.usage, MetricUsage::Counter)
    {
        Some((value1.value, value2.value))
    } else {
        None
    }
}

pub fn compute_diff_counter(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
    add_suffix: &str,
    sub_suffix: &str,
) -> Option<Metric> {
    let create_key = basic_key.to_string() + add_suffix;
    let drop_key = basic_key.to_string() + sub_suffix;

    if let Some((create_value, drop_value)) =
        get_metrics_counters_pair(metrics, &create_key, &drop_key)
    {
        let instance_count = if create_value > drop_value { create_value - drop_value } else { 0 };

        return Some(Metric { value: instance_count, usage: MetricUsage::Counter });
    }

    Some(Metric { value: 0, usage: MetricUsage::Counter })
}

pub fn compute_instance_counter(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    compute_diff_counter(basic_key, metrics, ".create", ".drop")
}

pub fn compute_queue_size_counter(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    compute_diff_counter(basic_key, metrics, ".posts", ".pulls")
}

pub fn add_compute_percentage_metric(
    metrics_dumper: &mut MetricsDumper,
    key: &str,
    value_key: &str,
    total_key: &str,
    bias: f64,
) {
    add_compute_relative_metric_impl(
        metrics_dumper,
        key,
        value_key,
        total_key,
        bias,
        MetricUsage::Percents,
    );
}

pub fn add_compute_relative_metric(
    metrics_dumper: &mut MetricsDumper,
    key: &str,
    value_key: impl ToString,
    total_key: impl ToString,
    bias: f64,
) {
    add_compute_relative_metric_impl(
        metrics_dumper,
        key,
        value_key,
        total_key,
        bias,
        MetricUsage::Float,
    );
}

fn add_compute_relative_metric_impl(
    metrics_dumper: &mut MetricsDumper,
    key: &str,
    value_key: impl ToString,
    total_key: impl ToString,
    bias: f64,
    usage: MetricUsage,
) {
    let value_key = value_key.to_string();
    let total_key = total_key.to_string();
    metrics_dumper.add_compute_handler(key.to_string(), move |_key, metrics| -> Option<Metric> {
        if let Some((value, total_value)) =
            get_metrics_counters_pair(metrics, &value_key, &total_key)
        {
            if total_value != 0 {
                let percentage = (value as f64) / (total_value as f64) + bias;

                return Some(Metric {
                    value: (percentage * MetricsDumper::METRIC_FLOAT_MULTIPLIER) as u64,
                    usage,
                });
            }
        }

        None
    });
}

pub fn add_compute_result_metric(metrics_dumper: &mut MetricsDumper, basic_key: &str) {
    metrics_dumper.add_compute_handler(
        format!("{}.success.frequency", basic_key),
        compute_result_success_metric,
    );
    metrics_dumper.add_compute_handler(
        format!("{}.failure.frequency", basic_key),
        compute_result_failure_metric,
    );
    metrics_dumper.add_compute_handler(
        format!("{}.ignore.frequency", basic_key),
        compute_result_ignore_metric,
    );
}

pub fn compute_result_status_metric(
    basic_key: &str,
    success: bool,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    let suffix = if success { ".success.frequency" } else { ".failure.frequency" };
    let basic_key = basic_key.trim_end_matches(suffix).to_string();
    let key1 = basic_key.clone() + if success { ".success" } else { ".failure" };
    let key2 = basic_key.clone() + ".total";

    if let Some((value, total_value)) = get_metrics_counters_pair(metrics, &key1, &key2) {
        let percentage = (value as f64) / (total_value as f64);

        return Some(Metric {
            value: (percentage * MetricsDumper::METRIC_FLOAT_MULTIPLIER) as u64,
            usage: MetricUsage::Percents,
        });
    }

    None
}

pub fn compute_result_success_metric(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    compute_result_status_metric(basic_key, true, metrics)
}

pub fn compute_result_failure_metric(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    compute_result_status_metric(basic_key, false, metrics)
}

pub fn compute_result_ignore_metric(
    basic_key: &str,
    metrics: &BTreeMap<String, Metric>,
) -> Option<Metric> {
    let basic_key = basic_key.trim_end_matches(".ignore.frequency").to_string();
    let key1 = basic_key.clone() + ".success";
    let key2 = basic_key.clone() + ".failure";
    let key3 = basic_key.clone() + ".total";

    let success = get_metrics_counters_pair(metrics, &key1, &key3);
    let failure = get_metrics_counters_pair(metrics, &key2, &key3);

    if success.is_none() || failure.is_none() {
        return None;
    }

    let total_value = success.unwrap().1 as f64;
    let success = success.unwrap().0 as f64;
    let failure = failure.unwrap().0 as f64;
    let reports_count = success + failure;

    let percentage = (total_value - reports_count as f64) / (total_value as f64);

    Some(Metric {
        value: (percentage * MetricsDumper::METRIC_FLOAT_MULTIPLIER) as u64,
        usage: MetricUsage::Percents,
    })
}
