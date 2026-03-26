/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod archives;
pub mod block_handle_db;
pub mod block_info_db;
pub mod catchain_persistent_db;
pub mod db;
pub mod dynamic_boc_rc_db;
pub mod error;
mod macros;
pub mod shard_top_blocks_db;
pub mod shardstate_db_async;
#[cfg(test)]
mod tests;
pub mod traits;
pub mod types;

#[cfg(feature = "telemetry")]
use adnl::telemetry::{Metric, MetricBuilder};
use std::{
    sync::{atomic::AtomicU64, Arc},
    time::{Duration, Instant},
};

pub struct TimeChecker {
    operation: String,
    threshold: Duration,
    start: Instant,
}

impl TimeChecker {
    pub fn new(operation: String, threshold_ms: u64) -> Self {
        let start = std::time::Instant::now();
        log::trace!("{} - started", operation);
        Self { operation, threshold: Duration::from_millis(threshold_ms), start }
    }
}

impl Drop for TimeChecker {
    fn drop(&mut self) {
        let time = self.start.elapsed();
        if time < self.threshold {
            log::trace!("{} - finished, TIME: {}", self.operation, time.as_millis());
        } else {
            log::warn!(
                "{} - finished too slow, TIME: {}ms, expected: {}ms",
                self.operation,
                time.as_millis(),
                self.threshold.as_millis()
            );
        }
    }
}

#[cfg(feature = "telemetry")]
pub struct StorageTelemetry {
    pub file_entries: Arc<Metric>,
    pub handles: Arc<Metric>,
    pub packages: Arc<Metric>,

    pub stored_cells: Arc<Metric>,
    pub storing_cells: Arc<Metric>,
    pub shardstates_queue: Arc<Metric>,
    pub cached_cells_counters: Arc<Metric>,

    pub loaded_cells_from_db: Arc<MetricBuilder>,
    pub load_cell_from_db_time_nanos: Arc<Metric>,
    pub load_cell_from_cache_time_nanos: Arc<Metric>,
    pub store_cell_to_cache_time_nanos: Arc<Metric>,
    pub stored_new_cells: Arc<MetricBuilder>,
    pub deleted_cells: Arc<MetricBuilder>,

    pub loaded_counters: Arc<MetricBuilder>,
    pub load_counter_time_nanos: Arc<Metric>,
    pub updated_counters: Arc<MetricBuilder>,

    pub boc_db_element_write_nanos: Arc<Metric>,

    pub save_boc_total_micros: Arc<Metric>,
    pub save_boc_traverse_micros: Arc<Metric>,
    pub save_boc_tr_build_micros: Arc<Metric>,
    pub save_boc_commit_micros: Arc<Metric>,
    pub save_boc_cleanup_micros: Arc<Metric>,

    pub delete_boc_total_micros: Arc<Metric>,
    pub delete_boc_traverse_micros: Arc<Metric>,
    pub delete_boc_tr_build_micros: Arc<Metric>,
    pub delete_boc_commit_micros: Arc<Metric>,

    pub cell_cache_hits: Arc<MetricBuilder>,
    pub cell_cache_misses: Arc<MetricBuilder>,
    pub cell_cache_len: Arc<Metric>,
}
#[cfg(feature = "telemetry")]
impl Default for StorageTelemetry {
    fn default() -> Self {
        Self {
            file_entries: Metric::without_totals("", 1),
            handles: Metric::without_totals("", 1),
            packages: Metric::without_totals("", 1),
            stored_cells: Metric::without_totals("", 1),
            storing_cells: Metric::without_totals("", 1),
            shardstates_queue: Metric::without_totals("", 1),
            cached_cells_counters: Metric::without_totals("", 1),
            loaded_cells_from_db: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            load_cell_from_db_time_nanos: Metric::with_total_average("", 1),
            load_cell_from_cache_time_nanos: Metric::with_total_average("", 1),
            store_cell_to_cache_time_nanos: Metric::with_total_average("", 1),
            stored_new_cells: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            deleted_cells: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            loaded_counters: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            load_counter_time_nanos: Metric::with_total_average("", 1),
            updated_counters: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            boc_db_element_write_nanos: Metric::with_total_average("", 1),
            save_boc_total_micros: Metric::with_total_average("", 1),
            save_boc_traverse_micros: Metric::with_total_average("", 1),
            save_boc_tr_build_micros: Metric::with_total_average("", 1),
            save_boc_commit_micros: Metric::with_total_average("", 1),
            save_boc_cleanup_micros: Metric::with_total_average("", 1),
            delete_boc_total_micros: Metric::with_total_average("", 1),
            delete_boc_traverse_micros: Metric::with_total_average("", 1),
            delete_boc_tr_build_micros: Metric::with_total_average("", 1),
            delete_boc_commit_micros: Metric::with_total_average("", 1),
            cell_cache_hits: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            cell_cache_misses: MetricBuilder::with_metric_and_period(
                Metric::with_total_amount("", 1),
                1000000000,
            ),
            cell_cache_len: Metric::without_totals("", 1),
        }
    }
}

#[derive(Default)]
pub struct StorageAlloc {
    pub file_entries: Arc<AtomicU64>,
    pub handles: Arc<AtomicU64>,
    pub packages: Arc<AtomicU64>,
    pub storage_cells: Arc<AtomicU64>,
}

pub(crate) const TARGET: &str = "storage";
