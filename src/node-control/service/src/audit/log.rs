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
    async fn shutdown(&self) {}
}

pub struct NoopAuditLog;

#[async_trait]
impl AuditLog for NoopAuditLog {
    async fn record(&self, _event: AuditEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditEvent;

    #[tokio::test]
    async fn noop_audit_log_record_completes() {
        let log = NoopAuditLog;
        log.record(AuditEvent::system_service_started("test")).await;
    }
}
