/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{AuditEvent, AuditFileHeader, AuditLogConfig, jsonl_log::AuditInitError};
use chrono::Utc;
use std::{
    sync::{
        Arc, Once,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Schema version stamped into the per-file [`AuditFileHeader`].
const AUDIT_SCHEMA_VERSION: u16 = 1;

static HOSTNAME_FALLBACK_WARNED: Once = Once::new();

#[derive(Debug)]
pub(crate) enum AuditCommand {
    Event(Box<AuditEvent>),

    /// Explicit shutdown signal on the event channel. Production drives shutdown
    /// through a dedicated `oneshot` (see [`JsonlAuditLog::shutdown`]) and via
    /// channel close, so this command exists only for tests that need to stop
    /// the writer deterministically in event order on the mpsc channel.
    #[cfg(test)]
    Shutdown,

    /// Forces an immediate flush of buffered events. Used only by tests to make
    /// assertions deterministic without waiting for the batch interval.
    #[cfg(test)]
    Flush,
}

pub(crate) struct AuditWriter {
    /// Host identity stamped into each new file's [`AuditFileHeader`].
    host: String,
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
    fn rotated_path(path: &std::path::Path, n: usize) -> std::path::PathBuf {
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audit".to_string());
        path.with_file_name(format!("{filename}.{n}"))
    }

    pub(crate) async fn open(
        config: Arc<AuditLogConfig>,
        dropped: Arc<AtomicU64>,
    ) -> Result<Self, AuditInitError> {
        Self::open_inner(config, dropped, Duration::ZERO).await
    }

    #[cfg(test)]
    pub(crate) async fn open_with_write_delay(
        config: Arc<AuditLogConfig>,
        dropped: Arc<AtomicU64>,
        write_delay: Duration,
    ) -> Result<Self, AuditInitError> {
        Self::open_inner(config, dropped, write_delay).await
    }

    async fn open_inner(
        config: Arc<AuditLogConfig>,
        dropped: Arc<AtomicU64>,
        write_delay: Duration,
    ) -> Result<Self, AuditInitError> {
        if config.max_size_bytes == 0 {
            tracing::warn!(
                "audit_log.max_size_bytes=0 is invalid for rotation; clamping to 1 byte at runtime"
            );
        }
        if config.batch_max_events == 0 {
            tracing::warn!(
                "audit_log.batch_max_events=0 is invalid for batching; clamping to 1 at runtime"
            );
        }
        if config.max_files <= 1 {
            tracing::warn!(
                "audit_log.max_files=1 truncates the live file in place on rotation; \
                 external tailers may lose position (prefer max_files > 1)"
            );
        }
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
                .map_err(AuditInitError::SetPermissions)?;
        }

        let current_size = file.metadata().await.map_err(AuditInitError::Metadata)?.len();

        let mut writer = Self {
            host: resolve_hostname(),
            config,
            file: Some(file),
            current_size,
            batch: Vec::with_capacity(64 * 1024),
            dropped_events: dropped,
            last_dropped_seen: 0,
            write_delay,
        };
        writer.write_header_if_empty().await.map_err(AuditInitError::FileOpen)?;
        Ok(writer)
    }

    fn file_header(&self) -> AuditFileHeader {
        AuditFileHeader {
            schema_version: AUDIT_SCHEMA_VERSION,
            service: "nodectl".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
            host: self.host.clone(),
            started_at: Utc::now(),
        }
    }

    async fn write_header_if_empty(&mut self) -> std::io::Result<()> {
        if self.current_size != 0 {
            return Ok(());
        }
        use tokio::io::AsyncWriteExt;
        let mut line = serde_json::to_vec(&self.file_header())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("audit file handle not open"))?;
        file.write_all(&line).await?;
        if self.config.fsync_on_batch {
            file.sync_data().await?;
        }
        self.current_size += line.len() as u64;
        Ok(())
    }

    async fn drain_pending_commands(
        &mut self,
        rx: &mut tokio::sync::mpsc::Receiver<AuditCommand>,
        buffered: &mut Vec<AuditEvent>,
    ) {
        let batch_max_events = self.config.batch_max_events.max(1);
        loop {
            match rx.try_recv() {
                Ok(AuditCommand::Event(ev)) => {
                    buffered.push(*ev);
                    if buffered.len() >= batch_max_events {
                        self.flush(buffered).await;
                    }
                }
                #[cfg(test)]
                Ok(AuditCommand::Shutdown) => {}
                #[cfg(test)]
                Ok(AuditCommand::Flush) => self.flush(buffered).await,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
    }

    async fn finish_shutdown(
        &mut self,
        rx: &mut tokio::sync::mpsc::Receiver<AuditCommand>,
        buffered: &mut Vec<AuditEvent>,
    ) {
        self.drain_pending_commands(rx, buffered).await;
        self.flush(buffered).await;
        self.maybe_emit_dropped_recovery().await;
    }

    pub(crate) async fn run(
        mut self,
        mut rx: tokio::sync::mpsc::Receiver<AuditCommand>,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let interval_ms = self.config.batch_interval_ms.max(1);
        let batch_max_events = self.config.batch_max_events.max(1);
        let interval_duration = Duration::from_millis(interval_ms);
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + interval_duration,
            interval_duration,
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut buffered: Vec<AuditEvent> = Vec::with_capacity(batch_max_events);

        loop {
            tokio::select! {
                biased;
                shutdown = &mut shutdown_rx => {
                    if shutdown.is_ok() {
                        self.finish_shutdown(&mut rx, &mut buffered).await;
                        return;
                    }
                }
                cmd = rx.recv() => {
                    match cmd {
                        Some(AuditCommand::Event(ev)) => {
                            buffered.push(*ev);
                            if buffered.len() >= batch_max_events {
                                self.flush(&mut buffered).await;
                            }
                        }
                        None => {
                            self.finish_shutdown(&mut rx, &mut buffered).await;
                            return;
                        }
                        #[cfg(test)]
                        Some(AuditCommand::Shutdown) => {
                            self.finish_shutdown(&mut rx, &mut buffered).await;
                            return;
                        }
                        #[cfg(test)]
                        Some(AuditCommand::Flush) => self.flush(&mut buffered).await,
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
        let max_size_bytes = self.config.max_size_bytes.max(1);

        self.batch.clear();
        for ev in buffered.drain(..) {
            let batch_before = self.batch.len();
            if let Err(e) = serde_json::to_writer(&mut self.batch, &ev) {
                self.batch.truncate(batch_before);
                tracing::error!(error = %e, "failed to serialize audit event");
                continue;
            }
            self.batch.push(b'\n');
            let needed = self.batch.len() - batch_before;

            if self.current_size + needed as u64 > max_size_bytes {
                // This event belongs to the next segment: rollback from the
                // current batch, flush current segment, rotate, then append again.
                self.batch.truncate(batch_before);
                if let Err(e) = self.write_batch_and_clear().await {
                    self.recover_after_write_failure().await;
                    Self::io_failed("audit write before rotation failed", e);
                }
                if let Err(e) = self.rotate().await {
                    Self::io_failed("audit rotation failed", e);
                    continue;
                }
                let rotated_before = self.batch.len();
                if let Err(e) = serde_json::to_writer(&mut self.batch, &ev) {
                    self.batch.truncate(rotated_before);
                    tracing::error!(error = %e, "failed to serialize audit event");
                    continue;
                }
                self.batch.push(b'\n');
                let rotated_needed = self.batch.len() - rotated_before;
                self.current_size += rotated_needed as u64;
                continue;
            }

            self.current_size += needed as u64;
        }

        if let Err(e) = self.write_batch_and_clear().await {
            self.recover_after_write_failure().await;
            Self::io_failed("audit batch write failed", e);
        }
    }

    /// On a write failure the batch may be dropped by the caller, so restore
    /// `current_size` to an on-disk value and clear staged bytes.
    async fn recover_after_write_failure(&mut self) {
        self.batch.clear();
        if let Some(file) = self.file.as_ref() {
            if let Ok(meta) = file.metadata().await {
                self.current_size = meta.len();
            } else {
                tracing::warn!("audit: failed to read file size after write failure");
            }
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
        // `tokio::fs::File::write_all` can return while the final chunk's blocking
        // write is still in flight in the file's internal state machine. Flush to
        // force that pending operation to complete, so the bytes are visible to
        // other readers (e.g. log tailers, or `std::fs` reads in tests) even when
        // `fsync_on_batch` is disabled.
        file.flush().await?;
        if self.config.fsync_on_batch {
            file.sync_data().await?;
        }
        self.batch.clear();
        Ok(())
    }

    fn io_failed(context: &str, err: std::io::Error) {
        tracing::error!(error = %err, "{context}");
    }

    async fn reopen_live_append(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let mut opts = tokio::fs::OpenOptions::new();
        opts.append(true).create(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let file = opts.open(path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        self.current_size = file.metadata().await?.len();
        self.file = Some(file);
        Ok(())
    }

    async fn rotate(&mut self) -> std::io::Result<()> {
        let path = self.config.path.clone();
        // Total retained files (including the live one) is at least 1; the
        // number of rotated history segments is `max - 1`. Guarding against 0
        // avoids an arithmetic underflow on `max - 1`.
        let max = self.config.max_files.max(1);

        if max > 1 {
            // Remove the oldest segment (.{max-1}) if present.
            let oldest = Self::rotated_path(&path, max - 1);
            if tokio::fs::try_exists(&oldest).await? {
                tokio::fs::remove_file(&oldest).await?;
            }

            // Shift .n -> .n+1, from the oldest down to .1.
            for n in (1..max - 1).rev() {
                let from = Self::rotated_path(&path, n);
                let to = Self::rotated_path(&path, n + 1);
                if tokio::fs::try_exists(&from).await? {
                    tokio::fs::rename(&from, &to).await?;
                }
            }
        }

        // Close the live handle before touching the file on disk, so the rename
        // is valid on platforms that forbid renaming an open file.
        self.file = None;

        if let Err(e) = self.rotate_inner(&path, max).await {
            // Best-effort: reopen the live file so subsequent writes can continue
            // instead of failing forever with a `None` handle until restart.
            if let Err(reopen_err) = self.reopen_live_append(&path).await {
                Self::io_failed("audit reopen after rotation failure failed", reopen_err);
            }
            return Err(e);
        }
        Ok(())
    }

    /// Performs the on-disk swap and opens a fresh live file. The caller is
    /// responsible for recovering the handle if this returns an error.
    async fn rotate_inner(&mut self, path: &std::path::Path, max: usize) -> std::io::Result<()> {
        let mut opts = tokio::fs::OpenOptions::new();
        if max > 1 {
            // Preserve history: rename current -> .1, then open a fresh live file.
            let rotated = Self::rotated_path(path, 1);
            tokio::fs::rename(path, &rotated).await?;
            opts.append(true).create(true);
        } else {
            // No history retained: truncate the live file in place.
            opts.write(true).create(true).truncate(true);
        }
        #[cfg(unix)]
        opts.mode(0o600);
        let file = opts.open(path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        self.file = Some(file);
        self.current_size = 0;
        self.write_header_if_empty().await?;
        Ok(())
    }

    async fn maybe_emit_dropped_recovery(&mut self) {
        let current = self.dropped_events.load(Ordering::Relaxed);
        let delta = current.saturating_sub(self.last_dropped_seen);
        if delta == 0 {
            return;
        }
        self.last_dropped_seen = current;

        let mut buf = vec![AuditEvent::system_audit_events_dropped(delta)];
        self.flush(&mut buf).await;
    }
}

/// Resolves the host identity for audit file headers (`HOSTNAME`, then `COMPUTERNAME`).
fn resolve_hostname() -> String {
    if let Some(host) = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.is_empty())
    {
        return host;
    }
    HOSTNAME_FALLBACK_WARNED.call_once(|| {
        tracing::warn!(
            "audit log host identity unavailable (HOSTNAME/COMPUTERNAME unset); \
             file header will use host=\"unknown\" — set HOSTNAME for forensics"
        );
    });
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{jsonl_log::JsonlAuditLog, log::AuditLog};
    use serde_json::Value;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    async fn run_writer_test<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        f().await;
    }

    fn sample_event(tag: &str) -> AuditEvent {
        AuditEvent::system_service_started(tag)
    }

    fn large_event(payload_kb: usize) -> AuditEvent {
        AuditEvent::system_service_started("x".repeat(payload_kb * 1024))
    }

    fn test_config(dir: &Path, mut cfg: AuditLogConfig) -> AuditLogConfig {
        cfg.path = dir.join("audit.jsonl");
        cfg
    }

    async fn run_writer_session<F, Fut>(
        config: AuditLogConfig,
        write_delay: Duration,
        f: F,
    ) -> (Arc<AtomicU64>, PathBuf)
    where
        F: FnOnce(tokio::sync::mpsc::Sender<AuditCommand>, PathBuf, Arc<AtomicU64>) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let config = Arc::new(config);
        let dropped = Arc::new(AtomicU64::new(0));
        let path = config.path.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(config.queue_capacity);
        // Keep test shutdown signal unsent; tests stop the writer via
        // `AuditCommand::Shutdown` on the mpsc channel to avoid races with
        // producer commands.
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let writer =
            AuditWriter::open_with_write_delay(config, dropped.clone(), write_delay).await.unwrap();

        // Drive the writer on the same task set as the producer (via `join!`) so
        // shutdown/flush completes before assertions — no spawned-task scheduling
        // races under CI parallel test load.
        let dropped_out = dropped.clone();
        let path_out = path.clone();
        tokio::join!(async move { writer.run(rx, shutdown_rx).await }, async move {
            f(tx, path, dropped).await
        },);
        drop(shutdown_tx);

        (dropped_out, path_out)
    }

    async fn send_event(tx: &tokio::sync::mpsc::Sender<AuditCommand>, event: AuditEvent) {
        tx.send(AuditCommand::Event(Box::new(event))).await.unwrap();
    }

    async fn flush(tx: &tokio::sync::mpsc::Sender<AuditCommand>) {
        tx.send(AuditCommand::Flush).await.unwrap();
    }

    async fn stop(tx: &tokio::sync::mpsc::Sender<AuditCommand>) {
        tx.send(AuditCommand::Shutdown).await.unwrap();
    }

    /// Reads event lines, skipping the per-file [`AuditFileHeader`] (no `event_type`).
    fn read_json_lines(path: &Path) -> Vec<Value> {
        assert!(path.exists(), "audit file missing at {}", path.display());
        let content = std::fs::read_to_string(path).unwrap();
        content
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str::<Value>(line).expect("valid json line"))
            .filter(|value| value.get("event_type").is_some())
            .collect()
    }

    fn count_rotated_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("audit.jsonl."))
            .count()
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    send_event(&tx, sample_event("one")).await;
                    flush(&tx).await;
                    stop(&tx).await;
                })
                .await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0]["data"]["version"], "one");
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    for i in 0..5 {
                        send_event(&tx, sample_event(&format!("ev-{i}"))).await;
                    }
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    stop(&tx).await;
                })
                .await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 5);
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    for _ in 0..8 {
                        send_event(&tx, large_event(1)).await;
                        flush(&tx).await;
                    }
                    stop(&tx).await;
                })
                .await;

            let rotated = path.with_extension("jsonl.1");
            assert!(rotated.exists(), "expected rotated file at {}", rotated.display());
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    for _ in 0..12 {
                        send_event(&tx, large_event(1)).await;
                        flush(&tx).await;
                    }
                    stop(&tx).await;
                })
                .await;

            let rotated_count = count_rotated_files(dir.path());
            assert!(
                rotated_count <= 2,
                "expected at most max_files-1 rotated segments, got {rotated_count}"
            );
            assert!(path.exists());
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    for _ in 0..6 {
                        send_event(&tx, large_event(1)).await;
                        flush(&tx).await;
                    }
                    stop(&tx).await;
                })
                .await;

            assert!(path.exists(), "live audit.jsonl must exist");
            assert_eq!(
                count_rotated_files(dir.path()),
                0,
                "max_files=1 must keep no rotated segments"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            run_writer_session(cfg.clone(), Duration::ZERO, |tx, _path, _dropped| async move {
                send_event(&tx, sample_event("first-run")).await;
                flush(&tx).await;
                stop(&tx).await;
            })
            .await;
            std::fs::rename(&path, path.with_extension("jsonl.1")).unwrap();
            assert!(!path.exists(), "precondition: live file moved aside");

            // Second run must recreate the live file and write into it.
            let (_dropped, _) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    send_event(&tx, sample_event("after-restart")).await;
                    flush(&tx).await;
                    stop(&tx).await;
                })
                .await;

            assert!(path.exists(), "writer must recreate the live file on restart");
            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0]["data"]["version"], "after-restart");
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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

    #[tokio::test(flavor = "current_thread")]
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

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    for i in 0..3 {
                        send_event(&tx, sample_event(&format!("pending-{i}"))).await;
                    }
                    stop(&tx).await;
                })
                .await;

            let lines = read_json_lines(&path);
            assert_eq!(lines.len(), 3);
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn file_mode_0600_on_unix() {
        run_writer_test(|| async {
            let dir = tempdir().unwrap();
            let cfg = test_config(
                dir.path(),
                AuditLogConfig { batch_interval_ms: 60_000, ..AuditLogConfig::default() },
            );
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, _dropped| async move {
                    send_event(&tx, sample_event("perm")).await;
                    flush(&tx).await;
                    stop(&tx).await;
                })
                .await;

            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
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
            let (_dropped, path) =
                run_writer_session(cfg, Duration::ZERO, |tx, _path, dropped| async move {
                    dropped.store(7, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    stop(&tx).await;
                })
                .await;

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
