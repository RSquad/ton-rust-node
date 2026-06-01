/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{
    AuditEvent, AuditLogConfig,
    jsonl_writer::{AuditCommand, AuditWriter},
    log::AuditLog,
};
use async_trait::async_trait;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Debug, Error)]
pub enum AuditInitError {
    #[error("audit log path is invalid: {0}")]
    InvalidPath(String),
    #[error("failed to create audit directory: {0}")]
    DirCreate(#[source] std::io::Error),
    #[error("failed to open audit file: {0}")]
    FileOpen(#[source] std::io::Error),
}

pub struct JsonlAuditLog {
    sender: mpsc::Sender<AuditCommand>,
    dropped_events: Arc<AtomicU64>,
    config: Arc<AuditLogConfig>,
    /// Writer task handle, consumed by the first [`AuditLog::shutdown`] call so
    /// callers can await the final drain/flush. `None` after shutdown.
    writer: Mutex<Option<JoinHandle<()>>>,
}

impl JsonlAuditLog {
    pub async fn start(config: AuditLogConfig) -> Result<Arc<Self>, AuditInitError> {
        Self::start_inner(config, Duration::ZERO).await
    }

    #[cfg(test)]
    pub(crate) async fn start_with_write_delay(
        config: AuditLogConfig,
        write_delay: Duration,
    ) -> Result<Arc<Self>, AuditInitError> {
        Self::start_inner(config, write_delay).await
    }

    async fn start_inner(
        config: AuditLogConfig,
        write_delay: Duration,
    ) -> Result<Arc<Self>, AuditInitError> {
        let config = Arc::new(config);
        if let Some(parent) = config.path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(AuditInitError::DirCreate)?;
            }
        } else {
            return Err(AuditInitError::InvalidPath(config.path.to_string_lossy().to_string()));
        }

        let (tx, rx) = mpsc::channel(config.queue_capacity.max(1));
        let dropped_events = Arc::new(AtomicU64::new(0));

        let writer = AuditWriter::open(config.clone(), dropped_events.clone(), write_delay).await?;
        let handle = tokio::spawn(writer.run(rx));

        Ok(Arc::new(Self { sender: tx, dropped_events, config, writer: Mutex::new(Some(handle)) }))
    }

    #[cfg(test)]
    pub(crate) fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl AuditLog for JsonlAuditLog {
    async fn shutdown(&self) {
        // Signal the writer to drain and flush the final batch. The writer also
        // stops on channel close, but sending an explicit command lets us await
        // the flush deterministically.
        let _ = self.sender.send(AuditCommand::Shutdown).await;

        // Take the handle without holding the lock across the await.
        let handle = self.writer.lock().expect("audit writer lock poisoned").take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    async fn record(&self, event: AuditEvent) {
        let cmd = AuditCommand::Event(Box::new(event));

        match self.sender.try_send(cmd) {
            Ok(()) => return,
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                // Capture diagnostics before `cmd` is moved into the send future,
                // so a dropped event can be attributed to its source/subject.
                let diag = match &cmd {
                    AuditCommand::Event(ev) => Some((ev.id, ev.source, ev.subject.kind)),
                    _ => None,
                };
                let timeout = Duration::from_millis(self.config.queue_full_timeout_ms);
                match tokio::time::timeout(timeout, self.sender.send(cmd)).await {
                    Ok(Ok(())) => return,
                    _ => {
                        self.dropped_events.fetch_add(1, Ordering::Relaxed);
                        let (event_id, source, subject) = match diag {
                            Some((id, source, subject)) => (Some(id), Some(source), Some(subject)),
                            None => (None, None, None),
                        };
                        tracing::warn!(
                            ?event_id,
                            ?source,
                            ?subject,
                            "audit event dropped: queue full after timeout"
                        );
                    }
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("audit log channel closed; service likely shutting down");
            }
        }
    }
}
