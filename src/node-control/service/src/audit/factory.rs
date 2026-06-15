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
    log::{AuditLog, NoopAuditLog},
    ring_buffer::AuditEventBuffer,
};
use std::sync::Arc;

/// Output of [`AuditLogFactory::from_config`]: the write handle and a separate ring
/// buffer for the REST read-path. Both are always present; the ring is empty (capacity 0
/// is normalised to 1) when `config.enabled` is false.
pub struct AuditComponents {
    pub log: Arc<dyn AuditLog>,
    pub ring: Arc<AuditEventBuffer>,
}

pub struct AuditLogFactory;

impl AuditLogFactory {
    pub async fn from_config(config: &AuditLogConfig) -> Result<AuditComponents, AuditInitError> {
        if !config.enabled {
            return Ok(AuditComponents {
                log: Arc::new(NoopAuditLog),
                ring: AuditEventBuffer::new(0),
            });
        }
        let log = JsonlAuditLog::start(config.clone()).await?;
        let ring = log.ring();
        Ok(AuditComponents { log, ring })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditEvent;
    use tempfile::tempdir;

    fn sample_event() -> AuditEvent {
        AuditEvent::system_service_started("test")
    }

    #[tokio::test]
    async fn factory_returns_noop_when_disabled() {
        let cfg = AuditLogConfig { enabled: false, ..AuditLogConfig::default() };
        let AuditComponents { log, ring } =
            AuditLogFactory::from_config(&cfg).await.expect("factory init");
        log.record(sample_event()).await;
        // Noop ring: event not pushed (NoopAuditLog doesn't touch the ring).
        let _ = ring;
    }

    #[tokio::test]
    async fn factory_starts_jsonl_when_enabled() {
        let dir = tempdir().unwrap();
        let mut cfg = AuditLogConfig { enabled: true, ..AuditLogConfig::default() };
        cfg.path = dir.path().join("audit.jsonl");
        let AuditComponents { log, ring } =
            AuditLogFactory::from_config(&cfg).await.expect("factory init");
        log.record(sample_event()).await;
        assert_eq!(ring.len(), 1, "ring should contain the recorded event");
        log.shutdown().await;
    }
}
