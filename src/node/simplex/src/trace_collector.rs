/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

//! Global consensus trace collector for statistics and observability
//!
//! Collects consensus lifecycle events (collation, validation, voting, acceptance)
//! from all sessions and writes them as JSONL to a session-logs file.
//!
//! # Architecture
//!
//! Two-part design:
//! - [`TraceCollector`] - Public handle (cloneable, Send+Sync). Shared across all sessions.
//! - `TraceCollectorImpl` - Private worker running on a single dedicated thread.
//!
//! Communication via `std::sync::mpsc` channel. Events are tagged with `session_id`
//! and buffered per-session for correct JSONL grouping.

use crate::{
    block::{CandidateId, SlotIndex},
    session_description::SessionDescription,
    simplex_state::Vote,
    SessionId,
};
use base64::Engine;
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{ton::consensus::stats, IntoBoxed};

/// Consensus trace event (TL-derived enum)
pub type TraceEvent = stats::Event;

/// Flush interval for writing buffered events to disk
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Receive timeout for the impl thread loop
const RECV_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum number of events buffered per session before oldest are dropped
const MAX_EVENTS_PER_SESSION: usize = 10_000;

/// How long a `never_ran` lifecycle record is held before being written. If a
/// real `sessionStarted` for the same (deterministic) session_id arrives within
/// this window, the never_ran is a superseded-future duplicate and is dropped.
/// Comfortably longer than a validator-set rotation, shorter than the
/// consumer's poll interval so genuine never-rans still surface promptly
const NEVER_RAN_DEFER: Duration = Duration::from_secs(300);

/// Maximum age (seconds) of events to keep on persistent flush failure
const MAX_EVENT_AGE_SECS: f64 = 300.0;

/// Backoff window after a per-path I/O failure. Suppresses repeat ERROR logs
/// for the same file while events accumulate in memory
const FILE_ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// Max length for the free-form `reason` field on `*Failed` events. Failure
/// reasons can include arbitrary nested error chains — cap them so each
/// JSONL line stays under a reasonable size
const MAX_REASON_LEN: usize = 256;

/// Convert a `Duration` to an `i32` count of whole milliseconds. Saturates
/// at `i32::MAX` (~24 days) which is far above any sane consensus interval
fn duration_to_ms_i32(d: Duration) -> i32 {
    let ms = d.as_millis();
    if ms > i32::MAX as u128 {
        i32::MAX
    } else {
        ms as i32
    }
}

/// Truncate a free-form reason string to `MAX_REASON_LEN`. Respects char
/// boundaries (so we don't slice a multi-byte UTF-8 sequence in half)
fn truncate_reason(reason: &str) -> String {
    if reason.len() <= MAX_REASON_LEN {
        return reason.to_string();
    }
    let mut end = MAX_REASON_LEN;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

/// One validator entry in a lifecycle `sessionStarted` record. All hex values
/// are lowercase with no `0x` prefix
pub struct LifecycleValidator {
    /// Index in the session validator set (contiguous from 0)
    pub idx: u32,
    /// 64 hex chars = the RAW Ed25519 public key (the `created_by` / ConfigParam
    /// 34 `public_key` value), NOT the key-id and NOT the ADNL id
    pub pubkey: String,
    /// 64 hex chars = the ADNL id (optional per entry)
    pub adnl: Option<String>,
    /// uint64 weight as a decimal string (does not fit JS-safe integers)
    pub weight: String,
}

/// Terminal status for a lifecycle `sessionStopped` record
#[derive(Clone, Copy)]
pub enum LifecycleFinalStatus {
    /// Session validated and stopped normally; `trace_paths` are complete
    Stopped,
    /// Pre-created future dropped without ever validating; no trace expected
    NeverRan,
    /// Trace known incomplete (e.g. closing out an orphaned session at shutdown)
    Aborted,
}

impl LifecycleFinalStatus {
    fn as_str(self) -> &'static str {
        match self {
            LifecycleFinalStatus::Stopped => "stopped",
            LifecycleFinalStatus::NeverRan => "never_ran",
            LifecycleFinalStatus::Aborted => "aborted",
        }
    }
}

/// Payload for a `consensus.lifecycle.sessionStarted` record. `ts` is stamped by
/// `record_lifecycle_started`; `trace_path` is resolved by the collector worker
/// from the `session_logs_file` template
pub struct LifecycleStarted {
    pub ts: f64,
    pub workchain: i32,
    pub shard_hex: String,
    pub cc_seqno: u32,
    pub session_id: String,
    pub our_idx: u32,
    pub our_pubkey: String,
    pub epoch_utime_since: u32,
    pub validators: Vec<LifecycleValidator>,
}

/// Payload for a `consensus.lifecycle.sessionStopped` record. `ts` is stamped by
/// `record_lifecycle_stopped`; `trace_paths` is resolved by the collector worker
pub struct LifecycleStopped {
    pub ts: f64,
    pub workchain: i32,
    pub shard_hex: String,
    pub cc_seqno: u32,
    pub session_id: String,
    pub our_idx: u32,
    pub our_pubkey: String,
    pub epoch_utime_since: u32,
    pub final_status: LifecycleFinalStatus,
}

/// Commands sent from TraceCollector handle to TraceCollectorImpl
enum Command {
    /// Record a consensus event for a specific session
    Record(ton_block::UInt256, TraceEvent),
    /// Notify that a session has ended. Flushes its pending events and
    /// evicts cached metadata
    SessionEnded(ton_block::UInt256),
    /// Append a `sessionStarted` lifecycle record to `lifecycle_{epoch}.jsonl`
    LifecycleStarted(Box<LifecycleStarted>),
    /// Flush the session trace then append a `sessionStopped` lifecycle record
    LifecycleStopped(Box<LifecycleStopped>),
    /// Stop the collector (flush and exit)
    Stop,
}

/// Public handle to the global trace collector.
///
/// Cloneable and Send+Sync (via mpsc::Sender). Created once, shared across all
/// sessions via clone. Each event is tagged with session_id.
///
/// The `join_handle` is shared via `Arc<Mutex<>>` so that any clone can call
/// `stop()` and join the thread. Only the first `stop()` call actually joins;
/// subsequent calls are no-ops.
pub struct TraceCollector {
    sender: mpsc::Sender<Command>,
    join_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl Clone for TraceCollector {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), join_handle: self.join_handle.clone() }
    }
}

impl TraceCollector {
    /// Create a new global TraceCollector and spawn the impl thread.
    ///
    /// - `session_logs_file`: Path to JSONL file. If None, buffered events are dropped on flush.
    ///
    /// If thread spawning fails (rare, resource exhaustion), returns a no-op collector
    /// that silently drops all events. This avoids crashing the node.
    pub fn new(session_logs_file: Option<String>) -> Self {
        let (sender, receiver) = mpsc::channel();

        let join_handle =
            match std::thread::Builder::new().name("SXTRACE".to_string()).spawn(move || {
                TraceCollectorImpl::run(session_logs_file, receiver);
            }) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    log::error!("Failed to spawn trace collector thread: {}", e);
                    None
                }
            };

        Self { sender, join_handle: Arc::new(Mutex::new(join_handle)) }
    }

    /// Record a raw TL event for a session. Prefer the typed `record_*` helpers below.
    pub fn record(&self, session_id: &SessionId, event: TraceEvent) {
        let _ = self.sender.send(Command::Record(session_id.clone(), event));
    }

    /// Mark a session as ended. Flushes any remaining buffered events and
    /// drops the session's cached metadata
    pub fn record_session_end(&self, session_id: &SessionId) {
        let _ = self.sender.send(Command::SessionEnded(session_id.clone()));
    }

    /// Append a `consensus.lifecycle.sessionStarted` record to the session's
    /// epoch file. The `trace_path` is filled in by the worker
    pub fn record_lifecycle_started(&self, mut p: LifecycleStarted) {
        p.ts = unix_now_f64();
        let _ = self.sender.send(Command::LifecycleStarted(Box::new(p)));
    }

    /// Append a `consensus.lifecycle.sessionStopped` record. For non-`never_ran`
    /// statuses the worker first flushes the session's trace buffer so the file
    /// referenced by `trace_paths` is complete before the record becomes visible
    pub fn record_lifecycle_stopped(&self, mut p: LifecycleStopped) {
        p.ts = unix_now_f64();
        let _ = self.sender.send(Command::LifecycleStopped(Box::new(p)));
    }

    /// Stop the collector thread. Flushes remaining buffered events and blocks
    /// until the thread exits.
    pub fn stop(&self) {
        let _ = self.sender.send(Command::Stop);
        if let Ok(mut handle) = self.join_handle.lock() {
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }

    // ========================================================================
    // Typed event helpers
    // ========================================================================

    /// Record session identity event (emitted once at session start).
    ///
    /// C++ reference: `simplex/pool.cpp`
    pub(crate) fn record_id(
        &self,
        session_id: &SessionId,
        desc: &SessionDescription,
        cc_seqno: u32,
    ) {
        let shard = desc.get_shard();
        let self_idx = desc.get_self_idx();
        let weight = desc.get_node_weight(self_idx);
        let opts = desc.opts();

        let options = stats::sessionoptions::SessionOptions {
            proto_version: opts.proto_version as i32,
            target_rate_ms: duration_to_ms_i32(opts.target_rate),
            min_block_interval_ms: duration_to_ms_i32(opts.min_block_interval),
            first_block_timeout_ms: duration_to_ms_i32(opts.first_block_timeout),
            max_block_size: opts.max_block_size as i32,
            max_collated_data_size: opts.max_collated_data_size as i32,
            standstill_timeout_ms: duration_to_ms_i32(opts.standstill_timeout),
            validation_retry_attempts: opts.validation_retry_attempts as i32,
            // The configurable collation retry loop was removed (the
            // `collation_retry_max_attempts` SessionOptions field no longer exists);
            // a genuine collation error now recovers with an empty block or a single
            // fixed-backoff restart. This telemetry field is retained for wire
            // compatibility and reported as 0 (no configured retry attempts).
            collation_retry_max_attempts: 0,
            use_quic: opts.use_quic.into(),
        };

        let event = stats::event::Id {
            workchain: shard.workchain_id(),
            shard: shard.shard_prefix_with_tag() as i64,
            cc_seqno: cc_seqno as i32,
            idx: self_idx.value() as i32,
            total_validators: desc.get_total_nodes() as i32,
            weight: weight as i64,
            total_weight: desc.get_total_weight() as i64,
            slots_per_leader_window: opts.slots_per_leader_window as i32,
            options,
        };
        self.record(session_id, event.into_boxed());
    }

    /// Record collation started event.
    ///
    /// C++ reference: `block-producer.cpp`
    pub(crate) fn record_collate_started(&self, session_id: &SessionId, slot: SlotIndex) {
        let event = stats::event::CollateStarted { target_slot: slot.value() as i32 };
        self.record(session_id, event.into_boxed());
    }

    /// Record collation finished event (normal block).
    ///
    /// C++ reference: `block-producer.cpp`
    pub(crate) fn record_collate_finished(
        &self,
        session_id: &SessionId,
        slot: SlotIndex,
        id: &CandidateId,
    ) {
        let event = stats::event::CollateFinished {
            target_slot: slot.value() as i32,
            id: make_tl_candidate_id(id),
        };
        self.record(session_id, event.into_boxed());
    }

    /// Record empty block collated event.
    ///
    /// C++ reference: `block-producer.cpp`
    pub(crate) fn record_collated_empty(&self, session_id: &SessionId, id: &CandidateId) {
        let event = stats::event::CollatedEmpty { id: make_tl_candidate_id(id) };
        self.record(session_id, event.into_boxed());
    }

    /// Record candidate received event (from collator or network).
    ///
    /// C++ reference: `block-producer.cpp` (collator), `private-overlay.cpp` (network)
    pub(crate) fn record_candidate_received(
        &self,
        session_id: &SessionId,
        id: &CandidateId,
        parent: Option<&CandidateId>,
        block_id: Option<&ton_block::BlockIdExt>,
        is_collator: bool,
    ) {
        let tl_parent = match parent {
            Some(p) => ton_api::ton::consensus::CandidateParent::Consensus_CandidateParent(
                ton_api::ton::consensus::candidateparent::CandidateParent {
                    id: make_tl_candidate_id(p).into_boxed(),
                },
            ),
            None => ton_api::ton::consensus::CandidateParent::Consensus_CandidateWithoutParents,
        };

        let tl_block = match block_id {
            Some(bid) => {
                stats::CandidateBlock::Consensus_Stats_Block(stats::candidateblock::Block {
                    id: bid.clone(),
                })
            }
            None => stats::CandidateBlock::Consensus_Stats_Empty,
        };

        let event = stats::event::CandidateReceived {
            id: make_tl_candidate_id(id),
            parent: tl_parent,
            block: tl_block,
            is_collator: ton_api::ton::Bool::from(is_collator),
        };
        self.record(session_id, event.into_boxed());
    }

    /// Record validation started event.
    ///
    /// C++ reference: `block-validator.cpp`
    pub(crate) fn record_validation_started(&self, session_id: &SessionId, id: &CandidateId) {
        let event = stats::event::ValidationStarted { id: make_tl_candidate_id(id) };
        self.record(session_id, event.into_boxed());
    }

    /// Record validation finished event.
    ///
    /// C++ reference: `block-validator.cpp`
    pub(crate) fn record_validation_finished(&self, session_id: &SessionId, id: &CandidateId) {
        let event = stats::event::ValidationFinished { id: make_tl_candidate_id(id) };
        self.record(session_id, event.into_boxed());
    }

    /// Record block accepted event.
    ///
    /// C++ reference: `block-accepter.cpp`
    pub(crate) fn record_block_accepted(&self, session_id: &SessionId, id: &CandidateId) {
        let event = stats::event::BlockAccepted { id: make_tl_candidate_id(id) };
        self.record(session_id, event.into_boxed());
    }

    /// Record voted event (we broadcast a vote).
    ///
    /// C++ reference: `simplex/pool.cpp`
    pub(crate) fn record_voted(&self, session_id: &SessionId, vote: &Vote) {
        if let Some(unsigned_vote) = vote_to_tl_unsigned(vote) {
            let event = stats::consensus::simplex::stats::event::Voted { vote: unsigned_vote };
            self.record(session_id, event.into_boxed());
        }
    }

    /// Record certificate observed event (notarize or finalize threshold reached).
    ///
    /// C++ reference: `simplex/pool.cpp`
    pub(crate) fn record_cert_observed(&self, session_id: &SessionId, vote: &Vote) {
        if let Some(unsigned_vote) = vote_to_tl_unsigned(vote) {
            let event =
                stats::consensus::simplex::stats::event::CertObserved { vote: unsigned_vote };
            self.record(session_id, event.into_boxed());
        }
    }

    /// Record collation failure event. `reason` is free-form and truncated
    /// to `MAX_REASON_LEN` to keep JSONL lines reasonable
    pub(crate) fn record_collate_failed(
        &self,
        session_id: &SessionId,
        slot: SlotIndex,
        reason: &str,
    ) {
        let event = stats::event::CollateFailed {
            target_slot: slot.value() as i32,
            reason: truncate_reason(reason),
        };
        self.record(session_id, event.into_boxed());
    }

    /// Record validation failure event. `reason` is free-form and truncated
    /// to `MAX_REASON_LEN`
    pub(crate) fn record_validation_failed(
        &self,
        session_id: &SessionId,
        id: &CandidateId,
        reason: &str,
    ) {
        let event = stats::event::ValidationFailed {
            id: make_tl_candidate_id(id),
            reason: truncate_reason(reason),
        };
        self.record(session_id, event.into_boxed());
    }

    /// Record a vote received from another validator (post signature verify).
    /// Caller is expected to skip self-loopback so this fires only for peer
    /// votes
    pub(crate) fn record_vote_received(
        &self,
        session_id: &SessionId,
        sender_idx: i32,
        unsigned_vote: ton_api::ton::consensus::simplex::UnsignedVote,
    ) {
        let event = stats::consensus::simplex::stats::event::VoteReceived {
            sender_idx,
            vote: unsigned_vote,
        };
        self.record(session_id, event.into_boxed());
    }
}

// ============================================================================
// Helper conversion functions
// ============================================================================

/// Convert Rust FSM CandidateId to TL candidateId.
///
/// Note: Only `slot` and `hash` are mapped. The `block` field of CandidateId is not part
/// of the TL candidateId schema — it is passed separately via `record_candidate_received`'s
/// `block_id` parameter when available.
fn make_tl_candidate_id(id: &CandidateId) -> ton_api::ton::consensus::candidateid::CandidateId {
    ton_api::ton::consensus::candidateid::CandidateId {
        slot: id.slot.value() as i32,
        hash: id.hash.clone(),
    }
}

/// Convert Rust FSM Vote to TL UnsignedVote.
/// Returns None for vote types that don't map to TL (fallback votes).
fn vote_to_tl_unsigned(vote: &Vote) -> Option<ton_api::ton::consensus::simplex::UnsignedVote> {
    use ton_api::ton::consensus::simplex::{unsignedvote, UnsignedVote};

    match vote {
        Vote::Notarize(v) => {
            let tl_id = ton_api::ton::consensus::candidateid::CandidateId {
                slot: v.slot.value() as i32,
                hash: v.block_hash.clone(),
            };
            Some(UnsignedVote::Consensus_Simplex_NotarizeVote(unsignedvote::NotarizeVote {
                id: tl_id.into_boxed(),
            }))
        }
        Vote::Finalize(v) => {
            let tl_id = ton_api::ton::consensus::candidateid::CandidateId {
                slot: v.slot.value() as i32,
                hash: v.block_hash.clone(),
            };
            Some(UnsignedVote::Consensus_Simplex_FinalizeVote(unsignedvote::FinalizeVote {
                id: tl_id.into_boxed(),
            }))
        }
        Vote::Skip(v) => Some(UnsignedVote::Consensus_Simplex_SkipVote(unsignedvote::SkipVote {
            slot: v.slot.value() as i32,
        })),
    }
}

// ============================================================================
// TraceCollectorImpl (private, runs on single dedicated thread)
// ============================================================================

/// Per-session identity captured from the `consensus.stats.id` event.
/// Used to resolve placeholders in the `session_logs_file` template
#[derive(Clone, Copy)]
struct SessionMeta {
    workchain: i32,
    shard_tag: u64,
    cc_seqno: u32,
}

struct TraceCollectorImpl {
    receiver: mpsc::Receiver<Command>,
    /// Per-session event buffers
    events: HashMap<ton_block::UInt256, Vec<stats::timestampedevent::TimestampedEvent>>,
    session_logs_file: Option<String>,
    /// True when `session_logs_file` contains `{...}` placeholders. In that mode
    /// each session writes to a separate, template-resolved path
    template_mode: bool,
    /// Per-session identity, set on first observed `consensus.stats.id` event
    session_meta: HashMap<ton_block::UInt256, SessionMeta>,
    /// Per-path error-backoff. While `now < error_until[path]` writes to that
    /// path are skipped silently to avoid log spam; events stay buffered
    file_error_until: HashMap<PathBuf, SystemTime>,
    /// Directory for `lifecycle_{epoch}.jsonl` files = the directory portion of
    /// the `session_logs_file` template (placeholders are filename-only, so the
    /// directory is static). `None` when no file template is configured
    lifecycle_dir: Option<PathBuf>,
    /// `never_ran` records held pending until their deadline, keyed by
    /// session_id hex. Dropped if a `started` for the same id arrives first
    /// (superseded-future duplicate). See [`NEVER_RAN_DEFER`]
    pending_never_ran: HashMap<String, (LifecycleStopped, SystemTime)>,
    /// session_ids (hex) that have produced a real `started`/`stopped`/`aborted`
    /// record this process, used to cancel/suppress spurious never_ran
    started_sessions: HashSet<String>,
    last_flush: SystemTime,
}

impl TraceCollectorImpl {
    fn run(session_logs_file: Option<String>, receiver: mpsc::Receiver<Command>) {
        let template_mode = session_logs_file.as_deref().map_or(false, |s| s.contains('{'));
        // Lifecycle files live in the directory portion of the template; an empty
        // parent (filename-only template) maps to the current directory
        let lifecycle_dir = session_logs_file.as_deref().map(|t| match Path::new(t).parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        });
        log::info!(
            "TraceCollector started (file: {:?}, template_mode: {}, lifecycle_dir: {:?})",
            session_logs_file,
            template_mode,
            lifecycle_dir
        );

        let mut collector = TraceCollectorImpl {
            receiver,
            events: HashMap::new(),
            session_logs_file,
            template_mode,
            session_meta: HashMap::new(),
            file_error_until: HashMap::new(),
            lifecycle_dir,
            pending_never_ran: HashMap::new(),
            started_sessions: HashSet::new(),
            last_flush: SystemTime::now(),
        };

        collector.event_loop();

        log::info!("TraceCollector stopped");
    }

    fn event_loop(&mut self) {
        loop {
            match self.receiver.recv_timeout(RECV_TIMEOUT) {
                Ok(Command::Record(session_id, event)) => {
                    self.handle_record(session_id, event);
                }
                Ok(Command::SessionEnded(session_id)) => {
                    self.handle_session_ended(session_id);
                }
                Ok(Command::LifecycleStarted(p)) => {
                    self.handle_lifecycle_started(*p);
                }
                Ok(Command::LifecycleStopped(p)) => {
                    self.handle_lifecycle_stopped(*p);
                }
                Ok(Command::Stop) => {
                    self.flush_all_never_ran();
                    self.flush();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Check if it's time to flush
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // All senders dropped — flush and exit
                    self.flush_all_never_ran();
                    self.flush();
                    return;
                }
            }

            // Emit any deferred never_ran records past their deadline
            self.flush_due_never_ran();

            // Periodic flush
            if let Ok(elapsed) = self.last_flush.elapsed() {
                if elapsed >= FLUSH_INTERVAL {
                    self.flush();
                }
            }
        }
    }

    fn handle_record(&mut self, session_id: ton_block::UInt256, event: TraceEvent) {
        // Capture per-session identity from the Id event for template path
        // resolution. Idempotent — the Id event values are deterministic
        if let Some(meta) = extract_session_meta(&event) {
            self.session_meta.insert(session_id.clone(), meta);
        }

        // Get current timestamp as f64 seconds since epoch
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();

        // Wrap in TimestampedEvent and push to per-session buffer
        let timestamped = stats::timestampedevent::TimestampedEvent { ts: ts.into(), event };
        let buffer = self.events.entry(session_id).or_default();
        buffer.push(timestamped);

        // Cap per-session buffer to prevent unbounded growth.
        // Drain 10% at a time to amortize the O(n) shift cost.
        if buffer.len() > MAX_EVENTS_PER_SESSION {
            let drain_count = MAX_EVENTS_PER_SESSION / 10;
            buffer.drain(..drain_count);
        }
    }

    /// Emit a terminal `sessionEnd` event so on-disk consumers know the
    /// session has stopped, then flush and drop the cached metadata
    fn handle_session_ended(&mut self, session_id: ton_block::UInt256) {
        // 1. Synthesize and route the sessionEnd event through the normal
        //    handle_record path so it lands in this session's per-session
        //    buffer and the next flush writes it to disk
        // Fieldless TL constructor → built as a direct enum variant
        let end_event = stats::Event::Consensus_Stats_SessionEnd;
        self.handle_record(session_id.clone(), end_event);

        // 2. Flush so the sessionEnd line lands on disk in the same batch as
        //    any other pending events for this session
        if !self.events.is_empty() {
            self.flush();
        }

        // 3. Evict cached metadata and drop any stragglers that arrived
        //    between the last flush and now. Also drop the started-session
        //    marker: once a session has fully ended no further (duplicate
        //    future) never_ran for its id can arrive, so the marker is no
        //    longer needed — this bounds `started_sessions` on long-running
        //    nodes instead of letting it grow unbounded across rotations
        let had_meta = self.session_meta.remove(&session_id).is_some();
        self.started_sessions.remove(&hex256(&session_id));
        self.events.remove(&session_id);
        if had_meta {
            log::debug!(
                "TraceCollector: session {} ended — evicted cached identity",
                session_id.to_hex_string()
            );
        }
    }

    /// Resolve the trace-file path for a session from the `session_logs_file`
    /// template using explicit identity fields. Empty when no template is set
    fn resolve_trace_path(
        &self,
        workchain: i32,
        shard_hex: &str,
        cc_seqno: u32,
        session_id_hex: &str,
    ) -> String {
        match &self.session_logs_file {
            Some(t) => resolve_path_from_parts(t, workchain, shard_hex, cc_seqno, session_id_hex)
                .to_string_lossy()
                .into_owned(),
            None => String::new(),
        }
    }

    fn handle_lifecycle_started(&mut self, p: LifecycleStarted) {
        if self.lifecycle_dir.is_none() {
            return;
        }
        // A real session with this id ran: remember it and cancel any pending
        // (deferred) never_ran for the same id. Pre-created futures are keyed by
        // a deterministic session_id; a stale future culled before the real run
        // would otherwise emit a spurious never_ran ahead of this started record.
        self.started_sessions.insert(p.session_id.clone());
        self.pending_never_ran.remove(&p.session_id);

        let trace_path =
            self.resolve_trace_path(p.workchain, &p.shard_hex, p.cc_seqno, &p.session_id);
        let val = lifecycle_started_json(&p, &trace_path);
        if let Ok(line) = serde_json::to_string(&val) {
            self.write_lifecycle_line(p.epoch_utime_since, line);
        }
    }

    fn handle_lifecycle_stopped(&mut self, p: LifecycleStopped) {
        if self.lifecycle_dir.is_none() {
            return;
        }
        match p.final_status {
            LifecycleFinalStatus::NeverRan => {
                // If a real session with this id already started, the never_ran
                // is spurious (a superseded duplicate future) — drop it.
                if self.started_sessions.contains(&p.session_id) {
                    return;
                }
                // Otherwise defer it: a real session with the same id may still
                // be promoted within a rotation. The pending record is emitted
                // only if no `started` for this id arrives before the deadline
                // (see flush_due_never_ran) or at shutdown (flush_all_never_ran).
                let deadline = SystemTime::now() + NEVER_RAN_DEFER;
                self.pending_never_ran.insert(p.session_id.clone(), (p, deadline));
            }
            LifecycleFinalStatus::Stopped | LifecycleFinalStatus::Aborted => {
                // A session that started: record it, cancel any pending never_ran
                // for the id, and write the terminal record now.
                self.started_sessions.insert(p.session_id.clone());
                self.pending_never_ran.remove(&p.session_id);
                self.write_lifecycle_stopped(&p);
            }
        }
    }

    /// Write a `sessionStopped` line. For terminal statuses that own a trace,
    /// flush first so the file is complete on disk before the record becomes
    /// visible (the single-worker FIFO guarantees this ordering). `never_ran`
    /// has no trace
    fn write_lifecycle_stopped(&mut self, p: &LifecycleStopped) {
        let trace_paths = match p.final_status {
            LifecycleFinalStatus::NeverRan => Vec::new(),
            LifecycleFinalStatus::Stopped | LifecycleFinalStatus::Aborted => {
                self.flush();
                let path =
                    self.resolve_trace_path(p.workchain, &p.shard_hex, p.cc_seqno, &p.session_id);
                if path.is_empty() {
                    Vec::new()
                } else {
                    vec![path]
                }
            }
        };
        let val = lifecycle_stopped_json(p, &trace_paths);
        if let Ok(line) = serde_json::to_string(&val) {
            self.write_lifecycle_line(p.epoch_utime_since, line);
        }
    }

    /// Emit deferred never_ran records whose deadline has passed and for which
    /// no real `started` ever arrived
    fn flush_due_never_ran(&mut self) {
        if self.pending_never_ran.is_empty() {
            return;
        }
        let now = SystemTime::now();
        let due: Vec<String> = self
            .pending_never_ran
            .iter()
            .filter(|(_, (_, deadline))| now >= *deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some((p, _)) = self.pending_never_ran.remove(&id) {
                if !self.started_sessions.contains(&id) {
                    self.write_lifecycle_stopped(&p);
                }
            }
        }
    }

    /// Emit all remaining deferred never_ran records (shutdown). A session id
    /// that started in the meantime is still skipped
    fn flush_all_never_ran(&mut self) {
        let ids: Vec<String> = self.pending_never_ran.keys().cloned().collect();
        for id in ids {
            if let Some((p, _)) = self.pending_never_ran.remove(&id) {
                if !self.started_sessions.contains(&id) {
                    self.write_lifecycle_stopped(&p);
                }
            }
        }
    }

    /// Append one JSONL line to `lifecycle_{epoch}.jsonl`. The whole line plus
    /// its newline is issued in a single `write_all` so a concurrent reader
    /// never observes a torn line (line-atomic on local filesystems; NFS does
    /// not guarantee this)
    fn write_lifecycle_line(&self, epoch: u32, line: String) {
        let dir = match &self.lifecycle_dir {
            Some(d) => d,
            None => return,
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::error!("LifecycleWriter: create_dir_all({dir:?}): {e}");
            return;
        }
        let path = dir.join(format!("lifecycle_{epoch}.jsonl"));
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(mut file) => {
                let mut buf = line;
                buf.push('\n');
                if let Err(e) = file.write_all(buf.as_bytes()) {
                    log::error!("LifecycleWriter: write {path:?}: {e}");
                }
                let _ = file.flush();
            }
            Err(e) => log::error!("LifecycleWriter: open {path:?}: {e}"),
        }
    }

    fn flush(&mut self) {
        self.last_flush = SystemTime::now();

        if self.events.is_empty() {
            return;
        }

        let session_logs_file = match self.session_logs_file.clone() {
            Some(f) => f,
            None => {
                // No file configured — just clear the buffers
                self.events.clear();
                return;
            }
        };

        // Take all events out for writing
        let all_events = std::mem::take(&mut self.events);

        if self.template_mode {
            self.flush_per_session(&session_logs_file, all_events);
        } else {
            self.flush_single_file(&session_logs_file, all_events);
        }
    }

    /// Single-file mode: all sessions write to the same path
    fn flush_single_file(
        &mut self,
        path: &str,
        all_events: HashMap<ton_block::UInt256, Vec<stats::timestampedevent::TimestampedEvent>>,
    ) {
        let path_buf = PathBuf::from(path);
        let entries: Vec<(ton_block::UInt256, Vec<_>)> = all_events.into_iter().collect();
        self.try_write_to_path(path_buf, entries);
    }

    /// Template mode: resolve the template per session and group writes by
    /// resolved path
    fn flush_per_session(
        &mut self,
        template: &str,
        all_events: HashMap<ton_block::UInt256, Vec<stats::timestampedevent::TimestampedEvent>>,
    ) {
        // Sessions whose Id event has not arrived yet — keep their events
        // buffered for the next flush
        let mut deferred: HashMap<
            ton_block::UInt256,
            Vec<stats::timestampedevent::TimestampedEvent>,
        > = HashMap::new();

        // Group resolved-path → sessions
        let mut grouped: HashMap<PathBuf, Vec<(ton_block::UInt256, Vec<_>)>> = HashMap::new();

        for (session_id, events) in all_events {
            match self.session_meta.get(&session_id) {
                Some(meta) => {
                    let path = resolve_path(template, &session_id, meta);
                    grouped.entry(path).or_default().push((session_id, events));
                }
                None => {
                    deferred.insert(session_id, events);
                }
            }
        }

        // Put unresolved sessions back into the buffer
        for (session_id, events) in deferred {
            self.events.entry(session_id).or_default().extend(events);
        }

        for (path, entries) in grouped {
            self.try_write_to_path(path, entries);
        }
    }

    /// Attempt to flush a batch to a single file path, honoring the per-path
    /// error backoff. On failure: log once, set the backoff window, put the
    /// events back and prune old ones
    fn try_write_to_path(
        &mut self,
        path: PathBuf,
        entries: Vec<(ton_block::UInt256, Vec<stats::timestampedevent::TimestampedEvent>)>,
    ) {
        // Backoff check — events stay buffered, no log spam
        if let Some(until) = self.file_error_until.get(&path) {
            if SystemTime::now() < *until {
                for (session_id, events) in entries {
                    self.events.entry(session_id).or_default().extend(events);
                }
                self.prune_old_events();
                return;
            }
            self.file_error_until.remove(&path);
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.mark_path_failed(&path, format!("create_dir_all({parent:?}): {e}"));
                    for (session_id, events) in entries {
                        self.events.entry(session_id).or_default().extend(events);
                    }
                    self.prune_old_events();
                    return;
                }
            }
        }

        let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                self.mark_path_failed(&path, format!("open: {e}"));
                for (session_id, events) in entries {
                    self.events.entry(session_id).or_default().extend(events);
                }
                self.prune_old_events();
                return;
            }
        };

        // `file` is an append-mode `std::fs::File` with no buffering, so each
        // successful `writeln!` is durably on disk. Track the first failing
        // index so we re-buffer ONLY the failed entry and the ones after it —
        // re-buffering already-written entries would duplicate their JSONL
        // lines on the next flush.
        let mut failed_from: Option<usize> = None;
        for (i, (session_id, events)) in entries.iter().enumerate() {
            let json = events_to_json(session_id, events);
            if let Err(e) = writeln!(file, "{}", json) {
                self.mark_path_failed(&path, format!("write: {e}"));
                failed_from = Some(i);
                break;
            }
        }

        if let Some(start) = failed_from {
            for (session_id, events) in entries.into_iter().skip(start) {
                self.events.entry(session_id).or_default().extend(events);
            }
            self.prune_old_events();
        }
        // Success path: file is dropped here, fd closed
    }

    fn mark_path_failed(&mut self, path: &PathBuf, reason: String) {
        log::error!("TraceCollector: {} failed for {:?}", reason, path);
        self.file_error_until.insert(path.clone(), SystemTime::now() + FILE_ERROR_BACKOFF);
    }

    /// Remove events older than MAX_EVENT_AGE_SECS to prevent unbounded growth
    /// when file writes persistently fail.
    fn prune_old_events(&mut self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        for buffer in self.events.values_mut() {
            buffer.retain(|e| (now - *e.ts) < MAX_EVENT_AGE_SECS);
        }
        self.events.retain(|_, v| !v.is_empty());
    }
}

// ============================================================================
// Template path resolution and metadata extraction
// ============================================================================

/// Current Unix time as float seconds since epoch
fn unix_now_f64() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

/// Resolve `{workchain}`, `{shard_hex}`, `{cc_seq}`, `{sessionid}` placeholders
/// in a `session_logs_file` template from explicit identity parts
fn resolve_path_from_parts(
    template: &str,
    workchain: i32,
    shard_hex: &str,
    cc_seqno: u32,
    session_id_hex: &str,
) -> PathBuf {
    let resolved = template
        .replace("{workchain}", &workchain.to_string())
        .replace("{shard_hex}", shard_hex)
        .replace("{cc_seq}", &cc_seqno.to_string())
        .replace("{sessionid}", session_id_hex);
    PathBuf::from(resolved)
}

/// Resolve `{workchain}`, `{shard_hex}`, `{cc_seq}`, `{sessionid}` placeholders
/// in a `session_logs_file` template
fn resolve_path(template: &str, session_id: &ton_block::UInt256, meta: &SessionMeta) -> PathBuf {
    resolve_path_from_parts(
        template,
        meta.workchain,
        &format!("{:016x}", meta.shard_tag),
        meta.cc_seqno,
        &hex256(session_id),
    )
}

/// Build the JSON value for a `consensus.lifecycle.sessionStarted` record
fn lifecycle_started_json(p: &LifecycleStarted, trace_path: &str) -> serde_json::Value {
    let validators: Vec<serde_json::Value> = p
        .validators
        .iter()
        .map(|v| {
            let mut o = serde_json::json!({
                "idx": v.idx,
                "pubkey": v.pubkey,
                "weight": v.weight,
            });
            if let Some(adnl) = &v.adnl {
                o["adnl"] = serde_json::Value::String(adnl.clone());
            }
            o
        })
        .collect();
    serde_json::json!({
        "@type": "consensus.lifecycle.sessionStarted",
        "ts": p.ts,
        "workchain": p.workchain,
        "shard_hex": p.shard_hex,
        "cc_seqno": p.cc_seqno,
        "session_id": p.session_id,
        "our_idx": p.our_idx,
        "our_pubkey": p.our_pubkey,
        "epoch_utime_since": p.epoch_utime_since,
        "trace_path": trace_path,
        "validators": validators,
    })
}

/// Build the JSON value for a `consensus.lifecycle.sessionStopped` record
fn lifecycle_stopped_json(p: &LifecycleStopped, trace_paths: &[String]) -> serde_json::Value {
    serde_json::json!({
        "@type": "consensus.lifecycle.sessionStopped",
        "ts": p.ts,
        "workchain": p.workchain,
        "shard_hex": p.shard_hex,
        "cc_seqno": p.cc_seqno,
        "session_id": p.session_id,
        "our_idx": p.our_idx,
        "our_pubkey": p.our_pubkey,
        "epoch_utime_since": p.epoch_utime_since,
        "final_status": p.final_status.as_str(),
        "trace_paths": trace_paths,
    })
}

/// Extract `SessionMeta` from a `consensus.stats.id` event. Returns `None` for
/// any other event variant
fn extract_session_meta(event: &TraceEvent) -> Option<SessionMeta> {
    match event {
        stats::Event::Consensus_Stats_Id(e) => Some(SessionMeta {
            workchain: e.workchain,
            shard_tag: e.shard as u64,
            cc_seqno: e.cc_seqno as u32,
        }),
        _ => None,
    }
}

// ============================================================================
// JSON serialization using serde_json::json!() macro
// ============================================================================

/// Format a UInt256 as hex string
pub fn hex256(h: &ton_block::UInt256) -> String {
    hex::encode(h.as_slice())
}

/// Format a UInt256 as base64 string (matches C++ `td::base64_encode` for `BitArray<256>`)
fn b64_256(h: &ton_block::UInt256) -> String {
    base64::engine::general_purpose::STANDARD.encode(h.as_slice())
}

/// Serialize a CandidateId (TL boxed struct) to JSON value.
fn candidate_id_json(id: &ton_api::ton::consensus::candidateid::CandidateId) -> serde_json::Value {
    serde_json::json!({
        "@type": "consensus.candidateId",
        "slot": id.slot,
        "hash": b64_256(&id.hash)
    })
}

/// Serialize the embedded `sessionOptions` struct to JSON
fn session_options_json(
    opts: &ton_api::ton::consensus::stats::sessionoptions::SessionOptions,
) -> serde_json::Value {
    serde_json::json!({
        "@type": "consensus.stats.sessionOptions",
        "proto_version": opts.proto_version,
        "target_rate_ms": opts.target_rate_ms,
        "min_block_interval_ms": opts.min_block_interval_ms,
        "first_block_timeout_ms": opts.first_block_timeout_ms,
        "max_block_size": opts.max_block_size,
        "max_collated_data_size": opts.max_collated_data_size,
        "standstill_timeout_ms": opts.standstill_timeout_ms,
        "validation_retry_attempts": opts.validation_retry_attempts,
        "collation_retry_max_attempts": opts.collation_retry_max_attempts,
        "use_quic": bool::from(opts.use_quic.clone()),
    })
}

/// Serialize a BlockIdExt to JSON value.
fn block_id_ext_json(bid: &ton_block::BlockIdExt) -> serde_json::Value {
    serde_json::json!({
        "@type": "tonNode.blockIdExt",
        "workchain": bid.shard_id.workchain_id(),
        "shard": (bid.shard_id.shard_prefix_with_tag() as i64).to_string(),
        "seqno": bid.seq_no,
        "root_hash": b64_256(&bid.root_hash),
        "file_hash": b64_256(&bid.file_hash)
    })
}

/// Serialize an UnsignedVote to JSON value.
fn unsigned_vote_json(vote: &ton_api::ton::consensus::simplex::UnsignedVote) -> serde_json::Value {
    use ton_api::ton::consensus::simplex::UnsignedVote;
    match vote {
        UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
            serde_json::json!({
                "@type": "consensus.simplex.notarizeVote",
                "id": candidate_id_enum_json(&v.id)
            })
        }
        UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
            serde_json::json!({
                "@type": "consensus.simplex.finalizeVote",
                "id": candidate_id_enum_json(&v.id)
            })
        }
        UnsignedVote::Consensus_Simplex_SkipVote(v) => {
            serde_json::json!({
                "@type": "consensus.simplex.skipVote",
                "slot": v.slot
            })
        }
    }
}

/// Serialize a CandidateId enum (boxed TL type with accessor methods) to JSON value.
fn candidate_id_enum_json(id: &ton_api::ton::consensus::CandidateId) -> serde_json::Value {
    serde_json::json!({
        "@type": "consensus.candidateId",
        "slot": id.slot(),
        "hash": b64_256(id.hash())
    })
}

/// Serialize a single TraceEvent to JSON value (matching C++ td::json_encode format with @type).
pub fn event_json(event: &TraceEvent) -> serde_json::Value {
    match event {
        stats::Event::Consensus_Stats_Id(e) => {
            serde_json::json!({
                "@type": "consensus.stats.id",
                "workchain": e.workchain,
                "shard": e.shard.to_string(),
                "cc_seqno": e.cc_seqno,
                "idx": e.idx,
                "total_validators": e.total_validators,
                "weight": e.weight.to_string(),
                "total_weight": e.total_weight.to_string(),
                "slots_per_leader_window": e.slots_per_leader_window,
                "options": session_options_json(&e.options)
            })
        }
        stats::Event::Consensus_Stats_CollateStarted(e) => {
            serde_json::json!({
                "@type": "consensus.stats.collateStarted",
                "target_slot": e.target_slot
            })
        }
        stats::Event::Consensus_Stats_CollateFinished(e) => {
            serde_json::json!({
                "@type": "consensus.stats.collateFinished",
                "target_slot": e.target_slot,
                "id": candidate_id_json(&e.id)
            })
        }
        stats::Event::Consensus_Stats_CollatedEmpty(e) => {
            serde_json::json!({
                "@type": "consensus.stats.collatedEmpty",
                "id": candidate_id_json(&e.id)
            })
        }
        stats::Event::Consensus_Stats_CandidateReceived(e) => {
            let parent_json = match &e.parent {
                ton_api::ton::consensus::CandidateParent::Consensus_CandidateWithoutParents => {
                    serde_json::json!({"@type": "consensus.candidateWithoutParents"})
                }
                ton_api::ton::consensus::CandidateParent::Consensus_CandidateParent(p) => {
                    serde_json::json!({
                        "@type": "consensus.candidateParent",
                        "id": candidate_id_enum_json(&p.id)
                    })
                }
            };
            let block_json = match &e.block {
                stats::CandidateBlock::Consensus_Stats_Block(b) => {
                    serde_json::json!({
                        "@type": "consensus.stats.block",
                        "id": block_id_ext_json(&b.id)
                    })
                }
                stats::CandidateBlock::Consensus_Stats_Empty => {
                    serde_json::json!({"@type": "consensus.stats.empty"})
                }
            };
            let is_collator = matches!(e.is_collator, ton_api::ton::Bool::BoolTrue);
            serde_json::json!({
                "@type": "consensus.stats.candidateReceived",
                "id": candidate_id_json(&e.id),
                "parent": parent_json,
                "block": block_json,
                "is_collator": is_collator
            })
        }
        stats::Event::Consensus_Stats_ValidationStarted(e) => {
            serde_json::json!({
                "@type": "consensus.stats.validationStarted",
                "id": candidate_id_json(&e.id)
            })
        }
        stats::Event::Consensus_Stats_ValidationFinished(e) => {
            serde_json::json!({
                "@type": "consensus.stats.validationFinished",
                "id": candidate_id_json(&e.id)
            })
        }
        stats::Event::Consensus_Stats_BlockAccepted(e) => {
            serde_json::json!({
                "@type": "consensus.stats.blockAccepted",
                "id": candidate_id_json(&e.id)
            })
        }
        stats::Event::Consensus_Simplex_Stats_Voted(e) => {
            serde_json::json!({
                "@type": "consensus.simplex.stats.voted",
                "vote": unsigned_vote_json(&e.vote)
            })
        }
        stats::Event::Consensus_Simplex_Stats_CertObserved(e) => {
            serde_json::json!({
                "@type": "consensus.simplex.stats.certObserved",
                "vote": unsigned_vote_json(&e.vote)
            })
        }
        stats::Event::Consensus_Simplex_Stats_VoteReceived(e) => {
            serde_json::json!({
                "@type": "consensus.simplex.stats.voteReceived",
                "sender_idx": e.sender_idx,
                "vote": unsigned_vote_json(&e.vote)
            })
        }
        stats::Event::Consensus_Stats_CollateFailed(e) => {
            serde_json::json!({
                "@type": "consensus.stats.collateFailed",
                "target_slot": e.target_slot,
                "reason": e.reason
            })
        }
        stats::Event::Consensus_Stats_ValidationFailed(e) => {
            serde_json::json!({
                "@type": "consensus.stats.validationFailed",
                "id": candidate_id_json(&e.id),
                "reason": e.reason
            })
        }
        stats::Event::Consensus_Stats_SessionEnd => {
            serde_json::json!({ "@type": "consensus.stats.sessionEnd" })
        }
    }
}

/// Serialize a batch of timestamped events to a JSONL line.
///
/// Output format matches C++ `write_session_stats` / `td::json_encode(td::ToJson(...))`:
/// `{"@type":"consensus.stats.events","id":"<b64>","events":[{"@type":"consensus.stats.timestampedEvent","ts":1234.567,"event":{...}}, ...]}`
fn events_to_json(
    session_id: &ton_block::UInt256,
    events: &[stats::timestampedevent::TimestampedEvent],
) -> String {
    let events_json: Vec<serde_json::Value> = events
        .iter()
        .map(|te| {
            serde_json::json!({
                "@type": "consensus.stats.timestampedEvent",
                "ts": *te.ts,
                "event": event_json(&te.event)
            })
        })
        .collect();

    let val = serde_json::json!({
        "@type": "consensus.stats.events",
        "id": b64_256(session_id),
        "events": events_json
    });
    // serde_json::to_string produces compact single-line JSON (no newlines)
    serde_json::to_string(&val).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_collector_basic() {
        let session_id = ton_block::UInt256::default();
        let collector = TraceCollector::new(None);

        // Recording events with no file configured must not panic
        let event = stats::event::CollateStarted { target_slot: 1 };
        collector.record(&session_id, event.into_boxed());
        let event2 = stats::event::CollateStarted { target_slot: 2 };
        collector.record(&session_id, event2.into_boxed());

        collector.stop();
    }

    /// Drop guard for temp directories — cleans up even on test panic.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(name);
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_trace_collector_file_output() {
        let dir = TempDir::new("trace_collector_test_file_output");
        let file_path = dir.path().join("session-stats.jsonl");

        let session_id = ton_block::UInt256::default();
        let collector = TraceCollector::new(Some(file_path.to_string_lossy().to_string()));

        // Record events
        let event = stats::event::CollateStarted { target_slot: 42 };
        collector.record(&session_id, event.into_boxed());

        // stop() flushes and joins the thread — file is guaranteed written after this returns
        collector.stop();

        // Verify file was written
        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(!contents.is_empty(), "Session stats file should not be empty");
        assert!(contents.contains("42"), "File should contain the event data");
    }

    #[test]
    fn test_trace_collector_multi_session() {
        let dir = TempDir::new("trace_collector_test_multi_session");
        let file_path = dir.path().join("session-stats.jsonl");
        let collector = TraceCollector::new(Some(file_path.to_string_lossy().to_string()));

        let session_a = ton_block::UInt256::from([1u8; 32]);
        let session_b = ton_block::UInt256::from([2u8; 32]);

        collector.record(&session_a, stats::event::CollateStarted { target_slot: 10 }.into_boxed());
        collector.record(&session_b, stats::event::CollateStarted { target_slot: 20 }.into_boxed());

        collector.stop();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(contents.contains("10"), "session A event must be written");
        assert!(contents.contains("20"), "session B event must be written");
    }

    #[test]
    fn test_vote_to_tl_unsigned() {
        use crate::simplex_state::{FinalizeVote, NotarizeVote, SkipVote};

        let notarize = Vote::Notarize(NotarizeVote {
            slot: SlotIndex::new(5),
            block_hash: ton_block::UInt256::default(),
        });
        assert!(vote_to_tl_unsigned(&notarize).is_some());

        let finalize = Vote::Finalize(FinalizeVote {
            slot: SlotIndex::new(5),
            block_hash: ton_block::UInt256::default(),
        });
        assert!(vote_to_tl_unsigned(&finalize).is_some());

        let skip = Vote::Skip(SkipVote { slot: SlotIndex::new(5) });
        assert!(vote_to_tl_unsigned(&skip).is_some());
    }

    #[test]
    fn test_trace_collector_buffer_overflow() {
        let collector = TraceCollector::new(None);
        let session_id = ton_block::UInt256::default();

        // Record more events than the buffer cap — the per-session buffer cap
        // must not panic the worker
        let total = MAX_EVENTS_PER_SESSION + 500;
        for i in 0..total {
            let event = stats::event::CollateStarted { target_slot: i as i32 };
            collector.record(&session_id, event.into_boxed());
        }

        // Collector should not have panicked — stop cleanly
        collector.stop();
    }

    #[test]
    fn test_trace_collector_json_format() {
        let session_id = ton_block::UInt256::from([0xABu8; 32]);

        // Test event_json for CollateStarted
        let event = stats::event::CollateStarted { target_slot: 7 }.into_boxed();
        let json = event_json(&event);
        assert_eq!(json["@type"], "consensus.stats.collateStarted");
        assert_eq!(json["target_slot"], 7);

        // Test event_json for CandidateReceived (nested objects)
        let candidate_id = ton_api::ton::consensus::candidateid::CandidateId {
            slot: 3,
            hash: ton_block::UInt256::from([0x11u8; 32]),
        };
        let event = stats::event::CandidateReceived {
            id: candidate_id,
            parent: ton_api::ton::consensus::CandidateParent::Consensus_CandidateWithoutParents,
            block: stats::CandidateBlock::Consensus_Stats_Empty,
            is_collator: ton_api::ton::Bool::BoolTrue,
        }
        .into_boxed();
        let json = event_json(&event);
        assert_eq!(json["@type"], "consensus.stats.candidateReceived");
        assert_eq!(json["id"]["@type"], "consensus.candidateId");
        assert_eq!(json["id"]["slot"], 3);
        assert_eq!(json["parent"]["@type"], "consensus.candidateWithoutParents");
        assert_eq!(json["block"]["@type"], "consensus.stats.empty");
        assert_eq!(json["is_collator"], true);

        // Test event_json for Voted (nested UnsignedVote)
        let tl_id = ton_api::ton::consensus::candidateid::CandidateId {
            slot: 5,
            hash: ton_block::UInt256::default(),
        };
        let vote = ton_api::ton::consensus::simplex::UnsignedVote::Consensus_Simplex_NotarizeVote(
            ton_api::ton::consensus::simplex::unsignedvote::NotarizeVote { id: tl_id.into_boxed() },
        );
        let event = stats::consensus::simplex::stats::event::Voted { vote }.into_boxed();
        let json = event_json(&event);
        assert_eq!(json["@type"], "consensus.simplex.stats.voted");
        assert_eq!(json["vote"]["@type"], "consensus.simplex.notarizeVote");
        assert_eq!(json["vote"]["id"]["slot"], 5);

        // Test events_to_json output format
        let ts = 1700000000.123f64;
        let te = stats::timestampedevent::TimestampedEvent {
            ts: ts.into(),
            event: stats::event::CollateStarted { target_slot: 99 }.into_boxed(),
        };
        let json_str = events_to_json(&session_id, &[te]);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Top-level structure
        assert_eq!(parsed["@type"], "consensus.stats.events");
        assert!(parsed["id"].is_string());
        assert_eq!(parsed["id"], b64_256(&session_id));
        let events_arr = parsed["events"].as_array().unwrap();
        assert_eq!(events_arr.len(), 1);

        // Event entry structure
        let entry = &events_arr[0];
        assert_eq!(entry["@type"], "consensus.stats.timestampedEvent");
        assert!(entry["ts"].is_number());
        assert_eq!(entry["event"]["@type"], "consensus.stats.collateStarted");
        assert_eq!(entry["event"]["target_slot"], 99);

        // Verify no newlines in output (JSONL requirement)
        assert!(!json_str.contains('\n'), "JSONL output must not contain newlines");
    }

    #[test]
    fn test_template_resolution() {
        let session_id = ton_block::UInt256::from_be_bytes(&[
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78,
            0x9a, 0xbc, 0xde, 0xf0,
        ]);
        let meta = SessionMeta { workchain: -1, shard_tag: 0x8000_0000_0000_0000, cc_seqno: 42 };
        let template = "/tmp/{workchain}:{shard_hex}/{cc_seq}_{sessionid}.jsonl";
        let resolved = resolve_path(template, &session_id, &meta);
        let expected = format!("/tmp/-1:8000000000000000/42_{}.jsonl", hex256(&session_id));
        assert_eq!(resolved, PathBuf::from(expected));

        // No placeholders → literal pass-through
        let plain = resolve_path("/tmp/plain.jsonl", &session_id, &meta);
        assert_eq!(plain, PathBuf::from("/tmp/plain.jsonl"));
    }

    #[test]
    fn test_extract_session_meta() {
        let id_event = stats::event::Id {
            workchain: -1,
            shard: 0x4000_0000_0000_0000,
            cc_seqno: 7,
            idx: 0,
            total_validators: 1,
            weight: 1,
            total_weight: 1,
            slots_per_leader_window: 4,
            options: stats::sessionoptions::SessionOptions::default(),
        }
        .into_boxed();
        let meta = extract_session_meta(&id_event).expect("Id event should yield meta");
        assert_eq!(meta.workchain, -1);
        assert_eq!(meta.shard_tag, 0x4000_0000_0000_0000);
        assert_eq!(meta.cc_seqno, 7);

        // Non-Id events return None
        let other = stats::event::CollateStarted { target_slot: 1 }.into_boxed();
        assert!(extract_session_meta(&other).is_none());
    }

    /// Build a `TraceCollectorImpl` directly (not via the worker thread) so we
    /// can call its internal methods and inspect state
    fn make_impl_for_test(session_logs_file: Option<String>) -> TraceCollectorImpl {
        let (_sender, receiver) = mpsc::channel();
        let template_mode = session_logs_file.as_deref().map_or(false, |s| s.contains('{'));
        TraceCollectorImpl {
            receiver,
            events: HashMap::new(),
            session_logs_file,
            template_mode,
            session_meta: HashMap::new(),
            file_error_until: HashMap::new(),
            lifecycle_dir: None,
            pending_never_ran: HashMap::new(),
            started_sessions: HashSet::new(),
            last_flush: SystemTime::now(),
        }
    }

    #[test]
    fn test_backoff_suppresses_repeated_errors() {
        // Build a target path under a regular file so create_dir_all is
        // guaranteed to fail portably (darwin/linux): mkdir P/sub where P is
        // a file, not a directory
        let dir = TempDir::new("trace_collector_test_backoff");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a dir").unwrap();
        let target = blocker.join("subdir").join("file.jsonl");

        let session_id = ton_block::UInt256::default();
        let event = stats::event::CollateStarted { target_slot: 1 }.into_boxed();
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        let entries = vec![(
            session_id,
            vec![stats::timestampedevent::TimestampedEvent { ts: ts.into(), event }],
        )];

        let mut imp = make_impl_for_test(None);

        // First attempt — should fail, set backoff window
        imp.try_write_to_path(target.clone(), entries.clone());
        let first_until =
            imp.file_error_until.get(&target).copied().expect("first failure should set backoff");
        assert!(first_until > SystemTime::now(), "backoff must be in the future");
        assert!(!imp.events.is_empty(), "failed entries must be re-buffered");

        // Second attempt immediately after — must be skipped (no log, no
        // backoff bump). We verify the deadline is unchanged
        imp.try_write_to_path(target.clone(), entries);
        let second_until = imp.file_error_until.get(&target).copied().unwrap();
        assert_eq!(
            first_until, second_until,
            "second attempt within backoff window must not reset the deadline"
        );
    }

    #[test]
    fn test_template_mode_creates_parent_dir_and_writes() {
        let dir = TempDir::new("trace_collector_test_template_write");
        let template =
            format!("{}/{{workchain}}/{{cc_seq}}_{{sessionid}}.jsonl", dir.path().display());
        let collector = TraceCollector::new(Some(template));

        let session_id = ton_block::UInt256::from_be_bytes(&[0xAA; 32]);

        // Identity event first so the template can be resolved on flush
        let id_event = stats::event::Id {
            workchain: 0,
            shard: 0x8000_0000_0000_0000u64 as i64,
            cc_seqno: 13,
            idx: 0,
            total_validators: 1,
            weight: 1,
            total_weight: 1,
            slots_per_leader_window: 4,
            options: stats::sessionoptions::SessionOptions::default(),
        };
        collector.record(&session_id, id_event.into_boxed());

        let event = stats::event::CollateStarted { target_slot: 99 };
        collector.record(&session_id, event.into_boxed());

        // stop() flushes and joins
        collector.stop();

        let expected = dir.path().join("0").join(format!("13_{}.jsonl", hex256(&session_id)));
        let contents = std::fs::read_to_string(&expected)
            .unwrap_or_else(|_| panic!("expected file {expected:?} to exist"));
        assert!(contents.contains("99"), "event payload must be present");
        assert!(contents.contains("\"cc_seqno\":13"), "id event must be present in same file");
    }

    #[test]
    fn test_record_collate_failed_round_trip() {
        let dir = TempDir::new("trace_collector_test_collate_failed");
        let file_path = dir.path().join("s.jsonl");
        let collector = TraceCollector::new(Some(file_path.to_string_lossy().to_string()));
        let session_id = ton_block::UInt256::from_be_bytes(&[0xEE; 32]);

        collector.record_collate_failed(
            &session_id,
            SlotIndex::new(7),
            "retry_scheduled (attempt 1/3): timeout",
        );

        collector.stop();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(contents.contains("consensus.stats.collateFailed"));
        assert!(contents.contains("\"target_slot\":7"));
        assert!(contents.contains("retry_scheduled (attempt 1/3): timeout"));
    }

    #[test]
    fn test_record_validation_failed_round_trip() {
        let dir = TempDir::new("trace_collector_test_validation_failed");
        let file_path = dir.path().join("s.jsonl");
        let collector = TraceCollector::new(Some(file_path.to_string_lossy().to_string()));
        let session_id = ton_block::UInt256::from_be_bytes(&[0xEF; 32]);

        let id = CandidateId {
            slot: SlotIndex::new(11),
            hash: ton_block::UInt256::from_be_bytes(&[0xAB; 32]),
            block: ton_block::BlockIdExt::default(),
        };
        collector.record_validation_failed(&session_id, &id, "rejected: bad signature");

        collector.stop();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(contents.contains("consensus.stats.validationFailed"));
        assert!(contents.contains("\"slot\":11"));
        assert!(contents.contains("rejected: bad signature"));
    }

    #[test]
    fn test_truncate_reason_respects_char_boundaries() {
        // Below the cap — unchanged
        let s = "short reason";
        assert_eq!(truncate_reason(s), s);

        // Exactly at cap — unchanged
        let exact = "a".repeat(MAX_REASON_LEN);
        assert_eq!(truncate_reason(&exact).len(), MAX_REASON_LEN);

        // Above cap — truncated
        let long = "a".repeat(MAX_REASON_LEN + 100);
        assert_eq!(truncate_reason(&long).len(), MAX_REASON_LEN);

        // Multi-byte UTF-8 (3-byte glyph) — must not split a glyph mid-byte.
        // We force the slice boundary to land mid-glyph by prefixing the cap
        let mut s = "a".repeat(MAX_REASON_LEN - 1);
        s.push('日'); // 3 bytes, lands at byte MAX_REASON_LEN..MAX_REASON_LEN+2
        s.push_str("tail");
        let out = truncate_reason(&s);
        // Should backtrack to the start of '日' i.e. MAX_REASON_LEN - 1
        assert!(out.len() < MAX_REASON_LEN);
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn test_session_end_writes_event() {
        let dir = TempDir::new("trace_collector_test_session_end");
        let file_path = dir.path().join("s.jsonl");
        let collector = TraceCollector::new(Some(file_path.to_string_lossy().to_string()));
        let session_id = ton_block::UInt256::from_be_bytes(&[0xF0; 32]);

        collector.record_session_end(&session_id);

        collector.stop();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            contents.contains("consensus.stats.sessionEnd"),
            "sessionEnd must be written, got: {contents}"
        );
    }

    #[test]
    fn test_trace_collector_noop_on_failed_sender() {
        // Simulate the no-op fallback: create a TraceCollector with a disconnected sender
        // (as if thread spawn failed — receiver is dropped, sends silently fail)
        let (sender, receiver) = mpsc::channel::<Command>();
        drop(receiver); // Drop receiver to simulate failed thread

        let collector = TraceCollector { sender, join_handle: Arc::new(Mutex::new(None)) };

        // All operations should silently succeed (no panics)
        let session_id = ton_block::UInt256::default();
        let event = stats::event::CollateStarted { target_slot: 1 };
        collector.record(&session_id, event.into_boxed());

        // stop() with no thread handle should not panic
        collector.stop();
    }

    #[test]
    fn test_resolve_path_from_parts() {
        let p = resolve_path_from_parts(
            "/d/{cc_seq}_wc{workchain}.{shard_hex}_{sessionid}.jsonl",
            0,
            "8000000000000000",
            542305,
            "abc",
        );
        assert_eq!(p, PathBuf::from("/d/542305_wc0.8000000000000000_abc.jsonl"));
    }

    #[test]
    fn test_lifecycle_started_json_shape() {
        let p = LifecycleStarted {
            ts: 123.5,
            workchain: 0,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 542305,
            session_id: "ab".repeat(32),
            our_idx: 4,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: 1780633352,
            validators: vec![
                LifecycleValidator {
                    idx: 0,
                    pubkey: "11".repeat(32),
                    adnl: Some("22".repeat(32)),
                    weight: "17".to_string(),
                },
                LifecycleValidator {
                    idx: 1,
                    pubkey: "33".repeat(32),
                    adnl: None,
                    weight: "18".to_string(),
                },
            ],
        };
        let v = lifecycle_started_json(&p, "/logs/sessions/x.jsonl");
        assert_eq!(v["@type"], "consensus.lifecycle.sessionStarted");
        assert_eq!(v["our_idx"], 4);
        assert_eq!(v["shard_hex"], "8000000000000000");
        assert_eq!(v["epoch_utime_since"], 1780633352u32);
        assert_eq!(v["trace_path"], "/logs/sessions/x.jsonl");
        // weight is a decimal STRING (uint64 does not fit JS-safe integers)
        assert_eq!(v["validators"][0]["weight"], "17");
        assert_eq!(v["validators"][0]["adnl"], "22".repeat(32));
        // adnl omitted when None
        assert!(v["validators"][1].get("adnl").is_none());
        // single-line JSON
        let line = serde_json::to_string(&v).unwrap();
        assert!(!line.contains('\n'));
    }

    #[test]
    fn test_lifecycle_stopped_json_shape() {
        let p = LifecycleStopped {
            ts: 1.0,
            workchain: -1,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 1,
            session_id: "ab".repeat(32),
            our_idx: 0,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: 100,
            final_status: LifecycleFinalStatus::NeverRan,
        };
        let v = lifecycle_stopped_json(&p, &[]);
        assert_eq!(v["@type"], "consensus.lifecycle.sessionStopped");
        assert_eq!(v["final_status"], "never_ran");
        assert_eq!(v["trace_paths"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_lifecycle_files_routed_by_epoch_and_complete() {
        let dir = TempDir::new("trace_collector_test_lifecycle");
        let template = format!(
            "{}/{{cc_seq}}_wc{{workchain}}.{{shard_hex}}_{{sessionid}}.jsonl",
            dir.path().display()
        );
        let collector = TraceCollector::new(Some(template));

        let sid = "ab".repeat(32);
        collector.record_lifecycle_started(LifecycleStarted {
            ts: 0.0,
            workchain: 0,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 542305,
            session_id: sid.clone(),
            our_idx: 0,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: 1780633352,
            validators: vec![LifecycleValidator {
                idx: 0,
                pubkey: "11".repeat(32),
                adnl: None,
                weight: "17".to_string(),
            }],
        });
        collector.record_lifecycle_stopped(LifecycleStopped {
            ts: 0.0,
            workchain: 0,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 542305,
            session_id: sid.clone(),
            our_idx: 0,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: 1780633352,
            final_status: LifecycleFinalStatus::Stopped,
        });
        // stop() flushes and joins the worker — file guaranteed written after
        collector.stop();

        // Routed to lifecycle_{epoch}.jsonl in the template directory
        let path = dir.path().join("lifecycle_1780633352.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "expected started + stopped lines");
        // Every line parses standalone
        for l in &lines {
            let _: serde_json::Value = serde_json::from_str(l).unwrap();
        }
        let started: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(started["@type"], "consensus.lifecycle.sessionStarted");
        let expected_trace =
            format!("{}/542305_wc0.8000000000000000_{}.jsonl", dir.path().display(), sid);
        assert_eq!(started["trace_path"], expected_trace);
        let stopped: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(stopped["@type"], "consensus.lifecycle.sessionStopped");
        assert_eq!(stopped["final_status"], "stopped");
        assert_eq!(stopped["trace_paths"][0], expected_trace);
    }

    fn never_ran(session_id: &str, epoch: u32) -> LifecycleStopped {
        LifecycleStopped {
            ts: 0.0,
            workchain: 0,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 1,
            session_id: session_id.to_string(),
            our_idx: 0,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: epoch,
            final_status: LifecycleFinalStatus::NeverRan,
        }
    }

    fn started(session_id: &str, epoch: u32) -> LifecycleStarted {
        LifecycleStarted {
            ts: 0.0,
            workchain: 0,
            shard_hex: "8000000000000000".to_string(),
            cc_seqno: 1,
            session_id: session_id.to_string(),
            our_idx: 0,
            our_pubkey: "cd".repeat(32),
            epoch_utime_since: epoch,
            validators: vec![],
        }
    }

    #[test]
    fn test_never_ran_cancelled_by_later_started() {
        let dir = TempDir::new("trace_collector_test_never_ran_cancel");
        let template = format!("{}/x_{{sessionid}}.jsonl", dir.path().display());
        let collector = TraceCollector::new(Some(template));

        let cancelled = "aa".repeat(32); // never_ran then started -> cancelled
        let genuine = "bb".repeat(32); // never_ran, no start -> flushed at stop

        // never_ran arrives first for both
        collector.record_lifecycle_stopped(never_ran(&cancelled, 7));
        collector.record_lifecycle_stopped(never_ran(&genuine, 7));
        // a real session starts for `cancelled` within the defer window
        collector.record_lifecycle_started(started(&cancelled, 7));
        // stop() flushes remaining (genuine) never_ran and joins
        collector.stop();

        let path = dir.path().join("lifecycle_7.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap();
        let recs: Vec<serde_json::Value> =
            contents.lines().map(|l| serde_json::from_str(l).unwrap()).collect();

        // `cancelled`: exactly one record, a started, and NO never_ran
        let cancelled_recs: Vec<&serde_json::Value> =
            recs.iter().filter(|r| r["session_id"] == cancelled).collect();
        assert_eq!(cancelled_recs.len(), 1, "cancelled id must have only the started record");
        assert_eq!(cancelled_recs[0]["@type"], "consensus.lifecycle.sessionStarted");

        // `genuine`: exactly one never_ran record (flushed at stop)
        let genuine_recs: Vec<&serde_json::Value> =
            recs.iter().filter(|r| r["session_id"] == genuine).collect();
        assert_eq!(genuine_recs.len(), 1, "genuine never_ran must be emitted at shutdown");
        assert_eq!(genuine_recs[0]["@type"], "consensus.lifecycle.sessionStopped");
        assert_eq!(genuine_recs[0]["final_status"], "never_ran");
    }
}
