/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{AuditEvent, log::AuditLog};
use async_trait::async_trait;
use std::sync::Mutex;

/// Captures audit events in memory for unit tests.
pub struct InMemoryAuditLog {
    pub events: Mutex<Vec<AuditEvent>>,
}

impl InMemoryAuditLog {
    pub fn new() -> Self {
        Self { events: Mutex::new(Vec::new()) }
    }

    pub fn drain(&self) -> Vec<AuditEvent> {
        std::mem::take(&mut *self.events.lock().expect("in-memory audit lock"))
    }
}

impl Default for InMemoryAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditLog for InMemoryAuditLog {
    async fn record(&self, event: AuditEvent) {
        self.events.lock().expect("in-memory audit lock").push(event);
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
    use uuid::Uuid;

    #[tokio::test]
    async fn records_and_drains_events() {
        let log = InMemoryAuditLog::new();
        let event = AuditEvent {
            schema_version: 1,
            id: Uuid::new_v4(),
            ts: Utc::now(),
            source: AuditSource::Elections,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor { kind: AuditActorKind::System, id: None, role: None, ip: None },
            subject: AuditSubject {
                kind: AuditSubjectKind::Node,
                id: Some("n1".into()),
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::SystemServiceStarted { version: "test".into() },
        };
        log.record(event.clone()).await;
        let drained = log.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, event.id);
        assert!(log.drain().is_empty());
    }
}
