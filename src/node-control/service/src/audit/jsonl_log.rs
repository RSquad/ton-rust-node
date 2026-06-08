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
    ring_buffer::AuditEventBuffer,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const WRITER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum AuditInitError {
    #[error("failed to create audit directory: {0}")]
    DirCreate(#[source] std::io::Error),
    #[error("failed to open audit file: {0}")]
    FileOpen(#[source] std::io::Error),
    #[error("failed to set audit file permissions: {0}")]
    SetPermissions(#[source] std::io::Error),
    #[error("failed to read audit file metadata: {0}")]
    Metadata(#[source] std::io::Error),
}

pub struct JsonlAuditLog {
    sender: tokio::sync::mpsc::Sender<AuditCommand>,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Serializes `shutdown()` so all concurrent callers observe completion.
    shutdown_gate: tokio::sync::Mutex<()>,
    dropped_events: Arc<AtomicU64>,
    config: Arc<AuditLogConfig>,
    /// In-memory ring buffer for the REST read-path. Populated in `record()` before
    /// the channel send so events appear immediately and survive queue overflow.
    ring: Arc<AuditEventBuffer>,
    /// Writer task handle, consumed by the first [`AuditLog::shutdown`] call so
    /// callers can await the final drain/flush. `None` after shutdown.
    writer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl JsonlAuditLog {
    pub async fn start(config: AuditLogConfig) -> Result<Arc<Self>, AuditInitError> {
        Self::start_inner(config).await
    }

    #[cfg(test)]
    pub(crate) async fn start_with_write_delay(
        config: AuditLogConfig,
        write_delay: Duration,
    ) -> Result<Arc<Self>, AuditInitError> {
        let config = Arc::new(config);
        Self::ensure_parent_dir(&config).await?;
        let dropped_events = Arc::new(AtomicU64::new(0));
        let writer =
            AuditWriter::open_with_write_delay(config.clone(), dropped_events.clone(), write_delay)
                .await?;
        Ok(Self::spawn_writer(config, dropped_events, writer))
    }

    async fn start_inner(config: AuditLogConfig) -> Result<Arc<Self>, AuditInitError> {
        let config = Arc::new(config);
        Self::ensure_parent_dir(&config).await?;
        let dropped_events = Arc::new(AtomicU64::new(0));
        let writer = AuditWriter::open(config.clone(), dropped_events.clone()).await?;
        Ok(Self::spawn_writer(config, dropped_events, writer))
    }

    async fn ensure_parent_dir(config: &AuditLogConfig) -> Result<(), AuditInitError> {
        match config.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                tokio::fs::create_dir_all(parent).await.map_err(AuditInitError::DirCreate)
            }
            Some(_) => Ok(()),
            None => Err(AuditInitError::DirCreate(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("audit log path has no parent: {}", config.path.to_string_lossy()),
            ))),
        }
    }

    /// Returns a handle to the ring buffer for the REST read-path.
    pub fn ring(&self) -> Arc<AuditEventBuffer> {
        self.ring.clone()
    }

    /// Wires the channels and spawns the writer task for an already-opened writer.
    fn spawn_writer(
        config: Arc<AuditLogConfig>,
        dropped_events: Arc<AtomicU64>,
        writer: AuditWriter,
    ) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::channel(config.queue_capacity.max(1));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(writer.run(rx, shutdown_rx));
        let ring = AuditEventBuffer::new(config.ring_buffer_capacity);

        Arc::new(Self {
            sender: tx,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            shutdown_gate: tokio::sync::Mutex::new(()),
            dropped_events,
            ring,
            config,
            writer: Mutex::new(Some(handle)),
        })
    }

    #[cfg(test)]
    pub(crate) fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl AuditLog for JsonlAuditLog {
    async fn shutdown(&self) {
        let _shutdown_guard = self.shutdown_gate.lock().await;

        // Use a dedicated shutdown signal that cannot block behind a full event
        // queue. The writer still handles channel-close and Shutdown command
        // for compatibility with existing tests.
        let shutdown = self.shutdown_tx.lock().expect("audit shutdown lock poisoned").take();
        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(());
        }

        // Take the handle without holding the lock across the await.
        let handle = self.writer.lock().expect("audit writer lock poisoned").take();
        if let Some(mut handle) = handle {
            match tokio::time::timeout(WRITER_SHUTDOWN_TIMEOUT, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!("audit writer task join failed: {e}");
                }
                Err(_) => {
                    tracing::warn!(
                        timeout_secs = WRITER_SHUTDOWN_TIMEOUT.as_secs(),
                        "audit writer shutdown timed out; aborting writer task"
                    );
                    handle.abort();
                    if let Err(e) = handle.await {
                        tracing::error!("audit writer task join failed after abort: {e}");
                    }
                }
            }
        }
    }

    async fn record(&self, event: AuditEvent) {
        // Deduplication: drop events whose key already exists in the ring.
        // Prevents e.g. repeated elections.stake_skipped for the same (node, election, reason).
        if let Some(key) = event.dedup_key()
            && self.ring.contains_dedup_key(&key)
        {
            return;
        }

        // Push into ring first: readers see the event immediately and even
        // queue-dropped events remain accessible on the REST read-path.
        self.ring.push(event.clone());

        let event_id = event.id;
        let source = event.payload.source();
        let cmd = AuditCommand::Event(Box::new(event));

        match self.sender.try_send(cmd) {
            Ok(()) => return,
            Err(tokio::sync::mpsc::error::TrySendError::Full(cmd)) => {
                let timeout = Duration::from_millis(self.config.queue_full_timeout_ms);
                match tokio::time::timeout(timeout, self.sender.send(cmd)).await {
                    Ok(Ok(())) => return,
                    Ok(Err(_)) => {
                        tracing::error!("audit log channel closed; service likely shutting down");
                    }
                    Err(_) => {
                        self.dropped_events.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            ?event_id,
                            ?source,
                            "audit event dropped: queue full after timeout"
                        );
                    }
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.dropped_events.fetch_add(1, Ordering::Relaxed);
                tracing::error!("audit log channel closed; service likely shutting down");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEvent, log::AuditLog};
    use std::{path::PathBuf, time::Instant};
    use tempfile::tempdir;

    fn sample_event(tag: &str) -> AuditEvent {
        AuditEvent::system_service_started(tag)
    }

    fn test_config(path: PathBuf) -> AuditLogConfig {
        AuditLogConfig {
            path,
            queue_capacity: 1,
            queue_full_timeout_ms: 10,
            batch_interval_ms: 60_000,
            batch_max_events: 100,
            ..AuditLogConfig::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_does_not_block_when_writer_is_slow_and_queue_is_full() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path().join("audit.jsonl"));
        let log =
            JsonlAuditLog::start_with_write_delay(cfg, Duration::from_secs(30)).await.unwrap();

        for i in 0..50 {
            log.record(sample_event(&format!("ev-{i}"))).await;
        }

        let started = Instant::now();
        log.shutdown().await;
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "shutdown should not block indefinitely behind queue backpressure"
        );
    }

    /// Ring buffer must contain the event even when the writer queue is full and
    /// the event is dropped from the channel (dropped_events increments).
    ///
    /// Uses `write_delay = 500ms` so the writer is slow relative to the 10ms
    /// channel timeout, guaranteeing that most events are dropped from the channel
    /// while still being captured in the ring (populated before the send attempt).
    #[tokio::test(flavor = "current_thread")]
    async fn record_pushes_to_ring_even_when_queue_full() {
        let dir = tempdir().unwrap();
        let cfg = AuditLogConfig {
            path: dir.path().join("audit.jsonl"),
            queue_capacity: 1,
            queue_full_timeout_ms: 10,
            ring_buffer_capacity: 200,
            batch_interval_ms: 60_000,
            batch_max_events: 1,
            ..AuditLogConfig::default()
        };
        let log =
            JsonlAuditLog::start_with_write_delay(cfg, Duration::from_millis(500)).await.unwrap();

        for i in 0..50 {
            log.record(sample_event(&format!("ev-{i}"))).await;
        }
        // Let the runtime settle so the dropped_events counter is up to date.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Ring captures every record() call regardless of channel state.
        assert_eq!(log.ring().len(), 50, "ring must contain every record() call");
        // Writer is ~500 ms/event; timeout is 10 ms → most events dropped from channel.
        assert!(log.dropped_events() > 0, "some events must have been dropped from channel");

        log.shutdown().await;
    }
}
