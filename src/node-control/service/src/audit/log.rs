/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::AuditEvent;
use async_trait::async_trait;

#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn record(&self, event: AuditEvent);
}

pub struct NoopAuditLog;

#[async_trait]
impl AuditLog for NoopAuditLog {
    async fn record(&self, _event: AuditEvent) {}
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
    async fn noop_audit_log_record_completes() {
        let log = NoopAuditLog;
        log.record(sample_event()).await;
    }
}
