/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogConfig {
    pub path: PathBuf,
    pub max_size_bytes: u64,
    pub max_files: usize,
    pub batch_interval_ms: u64,
    pub batch_max_events: usize,
    pub queue_capacity: usize,
    pub queue_full_timeout_ms: u64,
    pub fsync_on_batch: bool,
    pub include_payload: bool,
    pub record_client_ip: bool,
    pub ip_anonymize: bool,
    pub ring_buffer_capacity: usize,
}

impl Default for AuditLogConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("./logs/audit.jsonl"),
            max_size_bytes: 100 * 1024 * 1024,
            max_files: 10,
            batch_interval_ms: 1000,
            batch_max_events: 100,
            queue_capacity: 10_000,
            queue_full_timeout_ms: 250,
            fsync_on_batch: false,
            include_payload: true,
            record_client_ip: false,
            ip_anonymize: false,
            ring_buffer_capacity: 10_000,
        }
    }
}
