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
};
use std::sync::Arc;

pub struct AuditLogFactory;

impl AuditLogFactory {
    pub async fn from_config(config: &AuditLogConfig) -> Result<Arc<dyn AuditLog>, AuditInitError> {
        if !config.enabled {
            return Ok(Arc::new(NoopAuditLog));
        }
        let log = JsonlAuditLog::start(config.clone()).await?;
        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{
        AuditEvent,
        enums::{
            AuditActorKind, AuditEventPayload, AuditOutcome, AuditSeverity, AuditSource,
            AuditSubjectKind,
        },
        participant::{AuditActor, AuditSubject},
    };
    use chrono::Utc;
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn sample_event() -> AuditEvent {
        AuditEvent {
            schema_version: 1,
            id: Uuid::new_v4(),
            ts: Utc::now(),
            source: AuditSource::System,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor { kind: AuditActorKind::System, id: None, role: None, ip: None },
            subject: AuditSubject {
                kind: AuditSubjectKind::Config,
                id: None,
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::SystemServiceStarted { version: "test".into() },
        }
    }

    #[tokio::test]
    async fn factory_returns_noop_when_disabled() {
        let cfg = AuditLogConfig { enabled: false, ..AuditLogConfig::default() };
        let log = AuditLogFactory::from_config(&cfg).await.expect("factory init");
        log.record(sample_event()).await;
    }

    #[tokio::test]
    async fn factory_starts_jsonl_when_enabled() {
        let dir = tempdir().unwrap();
        let mut cfg = AuditLogConfig { enabled: true, ..AuditLogConfig::default() };
        cfg.path = dir.path().join("audit.jsonl");
        let log = AuditLogFactory::from_config(&cfg).await.expect("factory init");
        log.record(sample_event()).await;
        log.shutdown().await;
    }
}
