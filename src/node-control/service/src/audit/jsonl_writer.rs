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
    enums::{
        AuditActorKind, AuditEventPayload, AuditOutcome, AuditSeverity, AuditSource,
        AuditSubjectKind,
    },
    jsonl_log::AuditInitError,
    participant::{AuditActor, AuditSubject},
};
use chrono::Utc;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use uuid::Uuid;

pub(crate) enum AuditCommand {
    Event(Box<AuditEvent>),
    /// Forces an immediate flush of buffered events. Used only by tests to make
    /// assertions deterministic without waiting for the batch interval.
    #[cfg(test)]
    Flush,
    Shutdown,
}

pub(crate) struct AuditWriter {
    config: Arc<AuditLogConfig>,
    /// Live append handle. `None` only transiently during rotation (the old
    /// handle is closed before the on-disk rename so the swap is portable to
    /// platforms that forbid renaming open files, e.g. Windows).
    file: Option<tokio::fs::File>,
    current_size: u64,
    batch: Vec<u8>,
    dropped_events: Arc<AtomicU64>,
    last_dropped_seen: u64,
    /// Artificial per-write delay. Zero in production (no effect); set non-zero
    /// only by tests exercising the queue-full / backpressure path.
    write_delay: Duration,
}

impl AuditWriter {
    pub(crate) async fn open(
        config: Arc<AuditLogConfig>,
        dropped: Arc<AtomicU64>,
        write_delay: Duration,
    ) -> Result<Self, AuditInitError> {
        let path = &config.path;
        let mut opts = tokio::fs::OpenOptions::new();
        opts.append(true).create(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let file = opts.open(path).await.map_err(AuditInitError::FileOpen)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(AuditInitError::FileOpen)?;
        }

        let current_size = file.metadata().await.map_err(AuditInitError::FileOpen)?.len();

        Ok(Self {
            config,
            file: Some(file),
            current_size,
            batch: Vec::with_capacity(64 * 1024),
            dropped_events: dropped,
            last_dropped_seen: 0,
            write_delay,
        })
    }

    pub(crate) async fn run(mut self, mut rx: mpsc::Receiver<AuditCommand>) {
        let mut interval =
            tokio::time::interval(Duration::from_millis(self.config.batch_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut buffered: Vec<AuditEvent> = Vec::with_capacity(self.config.batch_max_events);

        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    match cmd {
                        Some(AuditCommand::Event(ev)) => {
                            buffered.push(*ev);
                            if buffered.len() >= self.config.batch_max_events {
                                self.flush(&mut buffered).await;
                            }
                        }
                        #[cfg(test)]
                        Some(AuditCommand::Flush) => self.flush(&mut buffered).await,
                        Some(AuditCommand::Shutdown) | None => {
                            self.flush(&mut buffered).await;
                            self.maybe_emit_dropped_recovery().await;
                            return;
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffered.is_empty() {
                        self.flush(&mut buffered).await;
                    }
                    self.maybe_emit_dropped_recovery().await;
                }
            }
        }
    }

    async fn flush(&mut self, buffered: &mut Vec<AuditEvent>) {
        if buffered.is_empty() {
            return;
        }

        self.batch.clear();
        for ev in buffered.drain(..) {
            let line = match serde_json::to_vec(&ev) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "failed to serialize audit event");
                    continue;
                }
            };
            let needed = line.len() + 1;
            if self.current_size + needed as u64 > self.config.max_size_bytes {
                if let Err(e) = self.write_batch_and_clear().await {
                    tracing::error!(error = %e, "audit write before rotation failed");
                }
                if let Err(e) = self.rotate().await {
                    tracing::error!(error = %e, "audit rotation failed");
                    continue;
                }
            }
            self.batch.extend_from_slice(&line);
            self.batch.push(b'\n');
            self.current_size += needed as u64;
        }

        if let Err(e) = self.write_batch_and_clear().await {
            tracing::error!(error = %e, "audit batch write failed");
        }
    }

    async fn write_batch_and_clear(&mut self) -> std::io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        if !self.write_delay.is_zero() {
            tokio::time::sleep(self.write_delay).await;
        }
        use tokio::io::AsyncWriteExt;
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("audit file handle not open"))?;
        file.write_all(&self.batch).await?;
        if self.config.fsync_on_batch {
            file.sync_data().await?;
        }
        self.batch.clear();
        Ok(())
    }

    async fn rotate(&mut self) -> std::io::Result<()> {
        let path = &self.config.path;
        // Total retained files (including the live one) is at least 1; the
        // number of rotated history segments is `max - 1`. Guarding against 0
        // avoids an arithmetic underflow on `max - 1`.
        let max = self.config.max_files.max(1);

        if max > 1 {
            // Remove the oldest segment (.{max-1}) if present.
            let oldest = path.with_extension(format!("jsonl.{}", max - 1));
            if tokio::fs::try_exists(&oldest).await? {
                tokio::fs::remove_file(&oldest).await?;
            }

            // Shift .n -> .n+1, from the oldest down to .1.
            for n in (1..max - 1).rev() {
                let from = path.with_extension(format!("jsonl.{}", n));
                let to = path.with_extension(format!("jsonl.{}", n + 1));
                if tokio::fs::try_exists(&from).await? {
                    tokio::fs::rename(&from, &to).await?;
                }
            }
        }

        // Close the live handle before touching the file on disk, so the rename
        // is valid on platforms that forbid renaming an open file.
        self.file = None;

        let mut opts = tokio::fs::OpenOptions::new();
        if max > 1 {
            // Preserve history: rename current -> .1, then open a fresh live file.
            let rotated = path.with_extension("jsonl.1");
            tokio::fs::rename(path, &rotated).await?;
            opts.append(true).create(true);
        } else {
            // No history retained: truncate the live file in place.
            opts.write(true).create(true).truncate(true);
        }
        #[cfg(unix)]
        opts.mode(0o600);
        self.file = Some(opts.open(path).await?);
        self.current_size = 0;
        Ok(())
    }

    async fn maybe_emit_dropped_recovery(&mut self) {
        let current = self.dropped_events.load(Ordering::Relaxed);
        let delta = current.saturating_sub(self.last_dropped_seen);
        if delta == 0 {
            return;
        }
        self.last_dropped_seen = current;

        let event = AuditEvent {
            schema_version: 1,
            id: Uuid::new_v4(),
            ts: Utc::now(),
            source: AuditSource::System,
            severity: AuditSeverity::Warn,
            outcome: AuditOutcome::Failure,
            actor: AuditActor { kind: AuditActorKind::System, id: None, role: None, ip: None },
            subject: AuditSubject {
                kind: AuditSubjectKind::Config,
                id: None,
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: Some("audit events dropped".into()),
            payload: AuditEventPayload::SystemAuditEventsDropped {
                dropped_events: delta,
                reason: "queue_full_after_timeout".into(),
            },
        };
        let mut buf = vec![event];
        self.flush(&mut buf).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{jsonl_log::JsonlAuditLog, log::AuditLog};
    use serde_json::Value;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    /// Writer tests perform real async file I/O. When the full `service` crate
    /// runs in parallel (as in CI), contention on Tokio's blocking pool can
    /// cause rare empty reads after an otherwise successful shutdown.
    static WRITER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    async fn run_writer_test<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let _guard = WRITER_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f().await;
    }

    fn sample_event(tag: &str) -> AuditEvent {
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
                id: Some(tag.into()),
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::SystemServiceStarted { version: tag.into() },
        }
    }

    fn large_event(payload_kb: usize) -> AuditEvent {
        let mut event = sample_event("large");
        event.payload = AuditEventPayload::RestApiConfigUpdated {
            operation: "update".into(),
            changes: serde_json::json!({ "blob": "x".repeat(payload_kb * 1024) }),
        };
        event
    }

    fn test_config(dir: &Path, mut cfg: AuditLogConfig) -> AuditLogConfig {
        cfg.path = dir.join("audit.jsonl");
        cfg
    }

    async fn spawn_writer(
        config: AuditLogConfig,
        write_delay: Duration,
    ) -> (mpsc::Sender<AuditCommand>, Arc<AtomicU64>, tokio::task::JoinHandle<()>, PathBuf) {
        let config = Arc::new(config);
        let dropped = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel(config.queue_capacity);
        let writer = AuditWriter::open(config.clone(), dropped.clone(), write_delay).await.unwrap();
        let path = config.path.clone();
        let handle = tokio::spawn(writer.run(rx));
        (tx, dropped, handle, path)
    }

    async fn send_event(tx: &mpsc::Sender<AuditCommand>, event: AuditEvent) {
        tx.send(AuditCommand::Event(Box::new(event))).await.unwrap();
    }

    async fn flush(tx: &mpsc::Sender<AuditCommand>) {
        tx.send(AuditCommand::Flush).await.unwrap();
    }

    async fn shutdown(tx: mpsc::Sender<AuditCommand>, handle: tokio::task::JoinHandle<()>) {
        let _ = tx.send(AuditCommand::Shutdown).await;
        let _ = handle.await;
    }

    fn read_json_lines(path: &Path) -> Vec<Value> {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect()
    }

    fn count_rotated_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("audit.jsonl."))
            .count()
    }

    #[tokio::test]
    async fn writes_single_event_to_file() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    batch_interval_ms: 60_000,
                    batch_max_events: 100,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            send_event(&tx, sample_event("one")).await;
            flush(&tx).await;
            shutdown(tx, handle).await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0]["data"]["version"], "one");
        })
        .await;
    }

    #[tokio::test]
    async fn batches_events_within_interval() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    batch_interval_ms: 50,
                    batch_max_events: 100,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            for i in 0..5 {
                send_event(&tx, sample_event(&format!("ev-{i}"))).await;
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
            shutdown(tx, handle).await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 5);
        })
        .await;
    }

    #[tokio::test]
    async fn rotates_at_max_size() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    max_size_bytes: 1024,
                    batch_interval_ms: 60_000,
                    batch_max_events: 1,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            for _ in 0..8 {
                send_event(&tx, large_event(1)).await;
                flush(&tx).await;
            }
            shutdown(tx, handle).await;

            let rotated = path.with_extension("jsonl.1");
            assert!(rotated.exists(), "expected rotated file at {}", rotated.display());
        })
        .await;
    }

    #[tokio::test]
    async fn retains_only_max_files() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    max_size_bytes: 512,
                    max_files: 3,
                    batch_interval_ms: 60_000,
                    batch_max_events: 1,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            for _ in 0..12 {
                send_event(&tx, large_event(1)).await;
                flush(&tx).await;
            }
            shutdown(tx, handle).await;

            let rotated_count = count_rotated_files(dir.path());
            assert!(
                rotated_count <= 2,
                "expected at most max_files-1 rotated segments, got {rotated_count}"
            );
            assert!(path.exists());
        })
        .await;
    }

    #[tokio::test]
    async fn max_files_one_keeps_no_history() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    max_size_bytes: 512,
                    max_files: 1,
                    batch_interval_ms: 60_000,
                    batch_max_events: 1,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            for _ in 0..6 {
                send_event(&tx, large_event(1)).await;
                flush(&tx).await;
            }
            shutdown(tx, handle).await;

            assert!(path.exists(), "live audit.jsonl must exist");
            assert_eq!(
                count_rotated_files(dir.path()),
                0,
                "max_files=1 must keep no rotated segments"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn recovers_when_live_file_missing_on_restart() {
        run_writer_test(|| async {
            // Models the crash window between `rename(path -> .1)` and opening the
            // new live file: on restart only the rotated segment exists. A fresh
            // writer must recreate `audit.jsonl` via create(true) and write to it.
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    batch_interval_ms: 60_000,
                    batch_max_events: 1,
                    ..AuditLogConfig::default()
                },
            );
            let path = dir.path().join("audit.jsonl");

            // First run persists one event, then we simulate the post-rename state:
            // move the live file aside so no `audit.jsonl` exists.
            let (tx, _dropped, handle, _) = spawn_writer(cfg.clone(), Duration::ZERO).await;
            send_event(&tx, sample_event("first-run")).await;
            flush(&tx).await;
            shutdown(tx, handle).await;
            std::fs::rename(&path, path.with_extension("jsonl.1")).unwrap();
            assert!(!path.exists(), "precondition: live file moved aside");

            // Second run must recreate the live file and write into it.
            let (tx, _dropped, handle, _) = spawn_writer(cfg, Duration::ZERO).await;
            send_event(&tx, sample_event("after-restart")).await;
            flush(&tx).await;
            shutdown(tx, handle).await;

            assert!(path.exists(), "writer must recreate the live file on restart");
            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0]["data"]["version"], "after-restart");
        })
        .await;
    }

    #[tokio::test]
    async fn concurrent_writers_no_data_loss() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    queue_capacity: 10_000,
                    batch_interval_ms: 20,
                    batch_max_events: 200,
                    max_size_bytes: 50 * 1024 * 1024,
                    ..AuditLogConfig::default()
                },
            );
            let log = JsonlAuditLog::start(cfg).await.unwrap();

            let mut tasks = Vec::new();
            for producer in 0..10 {
                let log = log.clone();
                tasks.push(tokio::spawn(async move {
                    for seq in 0..100 {
                        log.record(sample_event(&format!("p{producer}-s{seq}"))).await;
                    }
                }));
            }
            for task in tasks {
                task.await.unwrap();
            }

            log.shutdown().await;

            let path = dir.path().join("audit.jsonl");
            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 1000, "expected 1000 audit lines, got {}", lines.len());
        })
        .await;
    }

    #[tokio::test]
    async fn queue_full_drops_and_increments_counter() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    queue_capacity: 1,
                    queue_full_timeout_ms: 10,
                    batch_interval_ms: 60_000,
                    batch_max_events: 1,
                    ..AuditLogConfig::default()
                },
            );
            let log = JsonlAuditLog::start_with_write_delay(cfg, Duration::from_millis(500))
                .await
                .unwrap();

            for i in 0..50 {
                log.record(sample_event(&format!("drop-{i}"))).await;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(log.dropped_events() > 0, "expected dropped events counter > 0");
            log.shutdown().await;
        })
        .await;
    }

    #[tokio::test]
    async fn shutdown_flushes_pending() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    batch_interval_ms: 60_000,
                    batch_max_events: 100,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            for i in 0..3 {
                send_event(&tx, sample_event(&format!("pending-{i}"))).await;
            }
            shutdown(tx, handle).await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 3);
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_mode_0600_on_unix() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig { batch_interval_ms: 60_000, ..AuditLogConfig::default() },
            );
            let (tx, _dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;
            send_event(&tx, sample_event("perm")).await;
            flush(&tx).await;
            shutdown(tx, handle).await;

            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        })
        .await;
    }

    #[tokio::test]
    async fn synthetic_dropped_event_emitted_after_drops() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig {
                    batch_interval_ms: 50,
                    batch_max_events: 100,
                    ..AuditLogConfig::default()
                },
            );
            let (tx, dropped, handle, path) = spawn_writer(cfg, Duration::ZERO).await;

            dropped.store(7, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(120)).await;
            shutdown(tx, handle).await;

            let lines = read_json_lines(&path);
            let dropped_line = lines
                .iter()
                .find(|line| {
                    line.get("event_type")
                        == Some(&Value::String("system.audit_events_dropped".into()))
                })
                .expect("system.audit_events_dropped line");
            assert_eq!(dropped_line["data"]["dropped_events"], 7);
            assert_eq!(dropped_line["data"]["reason"], "queue_full_after_timeout");
        })
        .await;
    }
}
