/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{AuditEvent, log::AuditLog};
/// Captures audit events in memory for unit tests.
pub struct InMemoryAuditLog {
    events: std::sync::Mutex<Vec<AuditEvent>>,
}

impl InMemoryAuditLog {
    pub fn new() -> Self {
        Self { events: std::sync::Mutex::new(Vec::new()) }
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

#[async_trait::async_trait]
impl AuditLog for InMemoryAuditLog {
    async fn record(&self, event: AuditEvent) {
        self.events.lock().expect("in-memory audit lock").push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditEvent;

    #[tokio::test]
    async fn records_and_drains_events() {
        let log = InMemoryAuditLog::new();
        let event = AuditEvent::system_service_started("test");
        log.record(event.clone()).await;
        let drained = log.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, event.id);
        assert!(log.drain().is_empty());
    }
}
