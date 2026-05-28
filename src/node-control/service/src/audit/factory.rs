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
    log::{AuditLog, NoopAuditLog},
};
use std::sync::Arc;
use thiserror::Error;

pub struct AuditLogFactory;

impl AuditLogFactory {
    pub async fn from_config(config: &AuditLogConfig) -> Result<Arc<dyn AuditLog>, AuditInitError> {
        // SMA-99.3: spawn JsonlAuditLog writer from `config`.
        let _ = config;
        tracing::info!("audit log: NoopAuditLog (JsonlAuditLog wiring is SMA-99.3)");
        Ok(Arc::new(NoopAuditLog))
    }
}

#[derive(Debug, Error)]
pub enum AuditInitError {
    #[error("audit log path is invalid: {0}")]
    InvalidPath(String),
    #[error("failed to create audit directory: {0}")]
    DirCreate(#[source] std::io::Error),
    #[error("failed to open audit file: {0}")]
    FileOpen(#[source] std::io::Error),
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
    use std::{collections::BTreeMap, path::PathBuf};
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
    async fn factory_returns_audit_log_that_accepts_events() {
        let cfg = AuditLogConfig {
            path: PathBuf::from("/tmp/custom-audit.jsonl"),
            max_size_bytes: 1,
            ..AuditLogConfig::default()
        };
        let log = AuditLogFactory::from_config(&cfg).await.expect("factory init");
        log.record(sample_event()).await;
        log.record(sample_event()).await;
    }
}
