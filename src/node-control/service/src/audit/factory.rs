/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{
    AuditLogConfig,
    jsonl_log::{AuditInitError, JsonlAuditLog},
    log::AuditLog,
};
use std::sync::Arc;

pub struct AuditLogFactory;

impl AuditLogFactory {
    pub async fn from_config(config: &AuditLogConfig) -> Result<Arc<dyn AuditLog>, AuditInitError> {
        let log = JsonlAuditLog::start(config.clone()).await?;
        Ok(log)
    }
}
