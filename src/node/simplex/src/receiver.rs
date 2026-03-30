/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Receiver implementation for Simplex consensus
//!
//! The Receiver is the network layer for Simplex consensus:
//! - Hides overlay operations, exposes deserialized TL objects
//! - Acts as an actor with its own thread
//! - Created by Session and passed to SessionProcessor
//!
//! Reference: node/catchain/src/receiver.rs
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │ Receiver                                                                │
//! │                                                                         │
//! │  ┌─────────────────────────────────────────────────────────────────┐    │
//! │  │ Processing Thread (SXRCV:{session_id})                          │    │
//! │  │                                                                 │    │
//! │  │  - Pull task queue for incoming messages                        │    │
//! │  │  - Deserialize TL, validate signatures                          │    │
//! │  │  - Deduplicate per-slot (avoid reprocessing same vote)          │    │
//! │  │  - Forward to ReceiverListener (SessionProcessor)               │    │
//! │  │  - Metrics dump (30s), activity tick, shuffle send order (10s)  │    │
//! │  └─────────────────────────────────────────────────────────────────┘    │
//! │                         ▲                       │                       │
//! │                         │ incoming              │ outgoing              │
//! │                         │                       ▼                       │
//! │  ┌───────────────────────────────────────────────────────────────────┐  │
//! │  │ ConsensusOverlay (from ConsensusOverlayManager)                   │  │
//! │  │  - Incoming: on_message(), on_broadcast()                         │  │
//! │  │  - Outgoing: send_message() (votes), send_broadcast_fec() (blocks)│  │
//! │  └───────────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Message Types
//!
//! - **Votes** (via `send_message`, max 1024 bytes): `simplexConsensus.vote` wrapping any vote type
//! - **Block Broadcasts** (via `send_broadcast_fec`, large): `consensus.candidate` with FEC
//!
//! # Deduplication
//!
//! Per-slot HashMap tracks received votes by `(source_idx, vote_hash)` to prevent
//! reprocessing duplicate messages from the network.

#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use crate::{
    block::{SlotIndex, ValidatorIndex},
    simplex_state::MAX_FUTURE_SLOTS,
    ActivityNodePtr, BlockPayloadPtr, ConsensusOverlayListener, ConsensusOverlayLogReplayListener,
    ConsensusOverlayManagerPtr, MetricsHandle, PrivateKey, PublicKey, PublicKeyHash, RawVoteData,
    SessionId, SessionNode, ValidatorWeight,
};
use consensus_common::{
    check_execution_time, instrument,
    utils::{get_elapsed_time, MetricsDumper},
    ConsensusCommonFactory, ConsensusNode, ConsensusOverlayPtr, QueryResponseCallback,
};
use crossbeam::channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rand::{seq::SliceRandom, Rng};
use std::{
    collections::HashMap,
    mem::discriminant,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Weak,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{
    deserialize_boxed, serialize_boxed, tag_from_data,
    ton::{
        consensus::{
            candidateid::CandidateId,
            overlayid::OverlayId,
            simplex::{
                candidateandcert::CandidateAndCert, vote::Vote as TlVote,
                CandidateAndCert as CandidateAndCertBoxed, Certificate, UnsignedVote,
                Vote as TlVoteBoxed,
            },
            CandidateData, CandidateParent,
        },
        pub_::publickey::Overlay,
        rpc::consensus::simplex::RequestCandidate,
    },
    IntoBoxed,
};
use ton_block::{base64_encode, error, fail, KeyId, Result, ShardIdent, UInt256};

/// Helper to convert PublicKeyHash to base64 string
fn key_to_base64(key: &PublicKeyHash) -> String {
    base64_encode(key.data())
}

/*
    Constants
*/

/// Thread name prefix for receiver processing thread
const RECEIVER_THREAD_NAME: &str = "SXRCV";

const RECEIVER_METRICS_DUMP_PERIOD_MS: u64 = 30000; // Period for metrics dump
const RECEIVER_METRICS_IDLE_TIMEOUT: Duration =
    Duration::from_millis(RECEIVER_METRICS_DUMP_PERIOD_MS); // Idle timeout for metrics receiver
const RECEIVER_WARN_PROCESSING_LATENCY: Duration = Duration::from_millis(1000); // Max processing latency
const RECEIVER_LATENCY_WARN_DUMP_PERIOD: Duration = Duration::from_millis(2000); // Latency warning dump period
const RECEIVER_PROCESSING_PERIOD_MS: u64 = 100; // Processing period (timeout for queue pull)
const SHUFFLE_SEND_ORDER_PERIOD: Duration = Duration::from_secs(10); // Period to shuffle send order
const ACTIVE_WEIGHT_RECOMPUTE_PERIOD: Duration = Duration::from_secs(1); // Period to recompute active weight

// Candidate request constants (block repair / candidate resolver)
// Per-request network query timeout (overlay send_query deadline)
const CANDIDATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
// C++ parity: candidate-resolver.cpp uses indefinite retry with exponential backoff.
// bus.h defaults: initial=0.5s, multiplier=1.5, max=30.0s
const CANDIDATE_REQUEST_INITIAL_TIMEOUT: Duration = Duration::from_millis(500);
const CANDIDATE_REQUEST_TIMEOUT_MULTIPLIER: f64 = 1.5;
const CANDIDATE_REQUEST_MAX_TIMEOUT: Duration = Duration::from_secs(30);
const CANDIDATE_REQUEST_MAX_RETRIES: u32 = 50;

// Standstill initial range - used before first finalization calls set_standstill_slots()
// After first finalization, SessionProcessor sets the actual range via set_standstill_slots()
const STANDSTILL_INITIAL_SLOT_BEGIN: u32 = 0;
const STANDSTILL_INITIAL_SLOT_END: u32 = 1_000_000;

// Import ACTIVITY_THRESHOLD from utils.rs for consistency with SimplexState
use crate::utils::ACTIVITY_THRESHOLD;

/*
    Standstill Certificate Types
*/

/// Certificate kind for standstill cache
///
/// Used to distinguish certificate types when caching for standstill replay.
/// Reference: C++ pool.cpp CertificateBundle (notar, skip, final)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StandstillCertificateType {
    /// Notarization certificate
    Notar,
    /// Skip certificate
    Skip,
    /// Finalization certificate
    Final,
}

/*
    Receiver trait and type aliases

    These are crate-internal - not exposed in public API.
    Moved here from lib.rs for encapsulation (CODE-2).
*/

/// Shared health counters between receiver and session processor.
///
/// The receiver atomically increments these counters when anomalies occur.
/// The session processor reads them periodically in `run_health_checks()`
/// to detect delta-based anomaly spikes.
pub(crate) struct ReceiverHealthCounters {
    pub standstill_triggers: AtomicU64,
    pub candidate_giveups: AtomicU64,
}

impl ReceiverHealthCounters {
    pub fn new() -> Self {
        Self { standstill_triggers: AtomicU64::new(0), candidate_giveups: AtomicU64::new(0) }
    }
}

/// Pointer to Receiver
pub(crate) type ReceiverPtr = Arc<dyn Receiver + Send + Sync>;

/// Pointer to ReceiverListener
pub(crate) type ReceiverListenerPtr = Weak<dyn ReceiverListener + Send + Sync>;

/// Receiver trait - interface for sending messages to the network
///
/// Used by SessionProcessor to broadcast votes, blocks, and manage standstill.
/// Implementation: ReceiverWrapper
pub(crate) trait Receiver: Send + Sync {
    /// Send vote to all validators
    ///
    /// Signs the vote, broadcasts to all validators, and processes loopback
    /// (our own vote is submitted to the listener for FSM accounting).
    ///
    /// # Arguments
    /// * `vote` - Signed TL vote to broadcast
    fn send_vote(&self, vote: TlVote);

    /// Send block candidate to all validators
    ///
    /// # Arguments
    /// * `slot` - Slot number of the candidate
    /// * `candidate_hash` - Computed candidate hash (for caching and debug verification)
    /// * `candidate` - TL candidate data to broadcast
    fn send_block_broadcast(&self, slot: u32, candidate_hash: UInt256, candidate: CandidateData);

    /// Cache notarization certificate for query handling
    ///
    /// Called by SessionProcessor when notarization threshold is reached.
    /// The certificate is cached for responding to requestCandidate queries.
    ///
    /// # Arguments
    /// * `slot` - Slot number
    /// * `block_hash` - Block candidate hash
    /// * `notar_cert_data` - Serialized notarization certificate data
    fn cache_notarization_cert(&self, slot: u32, block_hash: UInt256, notar_cert_data: Vec<u8>);

    /// Cache candidate data bytes for query handling (startup recovery)
    ///
    /// Called during session startup to restore resolver cache from bootstrap.
    /// The candidate data is cached for responding to requestCandidate queries
    /// with `want_candidate=true`.
    ///
    /// # Arguments
    /// * `slot` - Slot number
    /// * `block_hash` - Block candidate hash
    /// * `candidate_data` - Serialized CandidateData bytes
    fn cache_candidate_bytes(&self, slot: u32, block_hash: UInt256, candidate_data: Vec<u8>);

    /// Cache an already-signed local vote for standstill replay (startup recovery)
    ///
    /// This does NOT send the vote to the network and does NOT loop it back to the FSM.
    /// It is used only on restart to rebuild C++-equivalent standstill behavior where
    /// `pool.cpp::alarm()` re-serializes the local validator's vote state.
    ///
    /// Votes are later filtered/pruned by `set_standstill_slots(begin, end)`.
    ///
    /// # Arguments
    /// * `vote` - Signed TL vote (wire-compatible) previously stored in DB
    fn cache_our_vote_for_standstill(&self, vote: TlVote);

    /// Notify receiver to cleanup old slots
    ///
    /// Cleans up old votes, dedup entries, and resolver cache.
    /// Should be called by SessionProcessor when a slot is finalized or skipped.
    ///
    /// # Arguments
    /// * `up_to_slot` - Clean up all data for slots < up_to_slot
    fn cleanup(&self, up_to_slot: u32);

    /// Request a missing candidate from peers (block repair)
    ///
    /// Called by SessionProcessor when a finalization event requires a candidate
    /// that wasn't received via broadcast. The receiver will:
    /// 1. Send requestCandidate query to a peer
    /// 2. Handle response and call on_candidate_received on listener
    /// 3. Retry with different peers on timeout/failure
    ///
    /// # Arguments
    /// * `slot` - Slot number of the missing candidate
    /// * `block_hash` - Block hash of the missing candidate
    fn request_candidate(&self, slot: u32, block_hash: UInt256);

    /// Reschedule standstill alarm
    ///
    /// Resets the standstill timer to fire after `standstill_timeout`.
    /// Should be called by SessionProcessor ONLY on block finalization
    /// (not on slot skip) to match C++ behavior.
    ///
    /// Reference: C++ pool.cpp reschedule_standstill_resolution() called from on_finalization()
    fn reschedule_standstill(&self);

    /// Set the range of slots for standstill vote re-broadcast
    ///
    /// Sets `[begin, end)` range of slots whose votes will be re-broadcast on standstill.
    /// Also removes any stored votes for slots outside this range.
    /// This matches C++ behavior where `tracked_slots_interval()` = [first_non_finalized, current_window_end).
    ///
    /// # Arguments
    /// * `begin` - First slot (inclusive) - typically first_active_slot
    /// * `end` - Last slot (exclusive) - typically (current_window + 1) * slots_per_window
    ///
    /// Reference: C++ pool.cpp alarm() uses state_->tracked_slots_interval()
    fn set_standstill_slots(&self, begin: u32, end: u32);

    /// Send certificate to all validators
    ///
    /// Broadcasts a TL certificate to all validators.
    /// Called when a certificate is created locally (notar/skip threshold reached)
    /// or when relaying a newly-observed foreign certificate.
    ///
    /// # Arguments
    /// * `certificate` - TL certificate to broadcast
    ///
    /// Reference: C++ pool.cpp handle_certificate() broadcasts via OutgoingProtocolMessage
    fn send_certificate(&self, certificate: Certificate);

    /// Cache certificate bytes for standstill replay
    ///
    /// Caches serialized certificate bytes for re-broadcast during standstill.
    /// Called when a certificate is created locally or received (and stored) from peer.
    ///
    /// # Arguments
    /// * `slot` - Slot number
    /// * `kind` - Certificate kind (Notar, Skip, or Final)
    /// * `cert_bytes` - Serialized TL certificate bytes
    ///
    /// Reference: C++ pool.cpp alarm() iterates certs.serialize_to(messages)
    fn cache_standstill_certificate(
        &self,
        slot: u32,
        kind: StandstillCertificateType,
        cert_bytes: Vec<u8>,
    );

    /// Cache last finalization certificate for standstill replay
    ///
    /// Caches the most recent finalization certificate for standstill replay.
    /// This certificate is always re-broadcast during standstill, even if the slot
    /// is outside the tracked range.
    ///
    /// # Arguments
    /// * `slot` - Slot number of the finalized block
    /// * `cert_bytes` - Serialized TL certificate bytes
    ///
    /// Reference: C++ pool.cpp alarm() always includes last_final_cert_ first
    fn cache_last_final_certificate(&self, slot: u32, cert_bytes: Vec<u8>);

    /// Stop the receiver
    fn stop(&self);
}

/// Receiver listener trait - callback interface to SessionProcessor
///
/// All callbacks post closures to Session's main task queue.
/// Implementation: ReceiverListenerImpl in session.rs
pub(crate) trait ReceiverListener: Send + Sync {
    /// Incoming vote (any type)
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `vote` - Deserialized vote
    /// * `raw_vote` - Original serialized vote bytes (Arc-wrapped for memory-efficient sharing)
    fn on_vote(&self, source_idx: u32, vote: TlVoteBoxed, raw_vote: crate::RawVoteData);

    /// Incoming certificate from network
    ///
    /// Called when a `consensus.simplex.certificate` message is received.
    /// This enables Rust nodes to receive certificate broadcasts from C++ nodes.
    ///
    /// Reference: C++ pool.cpp `handle(IncomingProtocolMessage)` parses both
    /// `tl::vote` and `tl::certificate` on the same channel.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `certificate` - Deserialized TL certificate object
    fn on_certificate(&self, source_idx: u32, certificate: Certificate);

    /// Incoming block candidate (from broadcast or query response)
    ///
    /// Called when a candidate is received, either via broadcast or query response.
    /// SessionProcessor should validate the candidate and update state.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender (broadcast source or query responder)
    /// * `candidate` - Deserialized candidate data
    /// * `notar_cert` - Serialized notarization certificate signature-set bytes (None for broadcasts)
    fn on_candidate_received(
        &self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
    );

    /// Periodic activity update from receiver
    /// - active_weight: sum of weights for validators with recent activity
    /// - last_activity: last receive time per validator (None if never received)
    fn on_activity(&self, active_weight: ValidatorWeight, last_activity: Vec<Option<SystemTime>>);

    /// Fallback for RequestCandidate queries when resolver_cache misses.
    ///
    /// Called by `handle_query()` when `want_candidate=true` but the resolver_cache
    /// does not have the candidate data. Delegates to SessionProcessor which can
    /// reconstruct the response from its in-memory `candidate_data_cache`, rebuild
    /// an empty candidate from `CandidateInfo`, or load persisted payloads from SimplexDB.
    ///
    /// This achieves parity with C++ `CandidateResolver::try_load_candidate_data_from_db()`.
    ///
    /// Reference: Alpenglow-Implementation-Plan.md Section 7.14a
    fn on_candidate_query_fallback(
        &self,
        slot: SlotIndex,
        block_hash: UInt256,
        want_notar: bool,
        response_callback: QueryResponseCallback,
    );
}

/*
    Delayed Action

    Used to schedule future operations like candidate request retries.
    Similar to SessionProcessor's DelayedAction but for ReceiverImpl.
*/

/// Task type for ReceiverImpl delayed actions
type ReceiverTaskPtr = Box<dyn FnOnce(&mut ReceiverImpl) + Send>;

/// Delayed action with expiration time
///
/// Used to schedule future operations like candidate request retries,
/// query timeouts, etc.
struct ReceiverDelayedAction {
    /// Time when action should be executed
    expiration_time: SystemTime,
    /// Handler closure to execute
    handler: ReceiverTaskPtr,
}

/*
    Candidate Request State

    Tracks pending candidate requests for block repair.
    Used by the outbound candidate resolver (requesting missing blocks).
*/

/// State for a pending candidate request
///
/// Tracks retry count, timing, and which validator was queried.
/// Used to implement retry logic with peer rotation.
struct CandidateRequestState {
    /// Time when the request was initiated
    start_time: SystemTime,
    /// Number of retry attempts so far
    retry_count: u32,
    /// Current timeout for this request (grows with exponential backoff)
    current_timeout: Duration,
    /// Validator index of the peer being queried
    source_idx: ValidatorIndex,
    /// Accumulated notar bytes from partial responses (C++ CandidateAndCert::merge parity).
    /// Peers may return notar-only when the candidate body is unavailable; we cache it
    /// here so that when a body-only response arrives later, the merged result is complete.
    cached_notar: Option<Vec<u8>>,
    /// Accumulated candidate bytes from partial responses.
    /// Peers may return candidate-only while notar is still missing; cache the body so
    /// a later notar-only response can complete the merged result.
    cached_candidate: Option<Vec<u8>>,
}

/*
    CandidateResolverCache - single-threaded cache for query handling

    Reference: C++ CandidateResolver caches candidates and certificates.
    This cache is local to ReceiverImpl (single-threaded processing loop).
    All cache operations are done via post_closure from ReceiverWrapper.
*/

/// Single-threaded cache for candidate resolver
///
/// Stores serialized candidate data and notarization certificates
/// for responding to requestCandidate queries from peers.
/// Local to ReceiverImpl - no Mutex needed as it's single-threaded.
struct CandidateResolverCache {
    /// Cached candidate data: (slot, block_hash) → serialized candidate bytes
    candidates: HashMap<(SlotIndex, UInt256), Vec<u8>>,
    /// Cached notarization certificates: (slot, block_hash) → serialized VoteSignatureSet bytes
    notar_certs: HashMap<(SlotIndex, UInt256), Vec<u8>>,
}

impl CandidateResolverCache {
    fn new() -> Self {
        Self { candidates: HashMap::new(), notar_certs: HashMap::new() }
    }

    /// Cache candidate data
    fn cache_candidate(&mut self, slot: SlotIndex, block_hash: UInt256, data: Vec<u8>) {
        self.candidates.insert((slot, block_hash), data);
    }

    /// Cache notarization certificate
    fn cache_notar_cert(&mut self, slot: SlotIndex, block_hash: UInt256, data: Vec<u8>) {
        self.notar_certs.insert((slot, block_hash), data);
    }

    /// Get cached candidate data
    fn get_candidate(&self, slot: SlotIndex, block_hash: &UInt256) -> Option<&Vec<u8>> {
        self.candidates.get(&(slot, block_hash.clone()))
    }

    /// Get cached notarization certificate
    fn get_notar_cert(&self, slot: SlotIndex, block_hash: &UInt256) -> Option<&Vec<u8>> {
        let key = (slot, block_hash.clone());
        self.notar_certs.get(&key)
    }

    /// Remove a cached candidate entry (e.g. after deserialization failure)
    fn remove_candidate(&mut self, slot: SlotIndex, block_hash: &UInt256) {
        self.candidates.remove(&(slot, block_hash.clone()));
    }

    /// Cleanup old entries for slots less than the given slot
    fn cleanup_before(&mut self, up_to_slot: SlotIndex) {
        self.candidates.retain(|(s, _), _| *s >= up_to_slot);
        self.notar_certs.retain(|(s, _), _| *s >= up_to_slot);
    }
}

/*
    ReceiverTaskQueues
*/

struct TaskDesc<F: ?Sized> {
    task: Box<F>,
    creation_time: SystemTime,
}

struct ReceiverTaskQueues {
    /// Receiver for processing thread tasks
    task_receiver: CrossbeamReceiver<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>,
    /// Sender for processing thread tasks
    task_sender: CrossbeamSender<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>,
    /// Counter for queue posts
    post_counter: metrics::Counter,
    /// Counter for queue pulls
    pull_counter: metrics::Counter,
}

impl ReceiverTaskQueues {
    fn post_closure(&self, job: Box<dyn FnOnce(&mut ReceiverImpl) + Send>) {
        let desc = TaskDesc { task: job, creation_time: SystemTime::now() };
        if let Err(err) = self.task_sender.send(desc) {
            log::error!("SimplexReceiver: failed to post closure: {}", err);
        }
        self.post_counter.increment(1);
    }

    fn new(metrics_receiver: &MetricsHandle) -> Self {
        let (task_sender, task_receiver) =
            crossbeam::channel::unbounded::<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>();

        let post_counter =
            metrics_receiver.sink().register_counter(&"simplex_receiver_main_queue.posts".into());
        let pull_counter =
            metrics_receiver.sink().register_counter(&"simplex_receiver_main_queue.pulls".into());

        Self { task_receiver, task_sender, post_counter, pull_counter }
    }
}

/*
    ReceiverThreads
*/

struct ReceiverThreadDesc {
    thread_prefix: String,
    stopped: Arc<AtomicBool>,
    thread_handle: thread::JoinHandle<()>,
    _activity_node: ActivityNodePtr,
}

struct ReceiverThreads {
    threads: Vec<ReceiverThreadDesc>,
    stop_flag: Arc<AtomicBool>,
    session_id: SessionId,
    panicked_flag: Arc<AtomicBool>,
}

impl ReceiverThreads {
    fn new(session_id: SessionId, panicked_flag: Arc<AtomicBool>) -> Self {
        Self {
            threads: Vec::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            session_id,
            panicked_flag,
        }
    }

    fn start_thread(
        &mut self,
        thread_prefix: String,
        thread_fn: Box<dyn FnOnce(Arc<AtomicBool>, ActivityNodePtr) + Send>,
    ) -> Result<Arc<AtomicBool>> {
        let stop = self.stop_flag.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let session_id = self.session_id.to_hex_string();
        let panicked_flag = self.panicked_flag.clone();
        let activity_node = ConsensusCommonFactory::create_activity_node(format!(
            "{}_{}",
            thread_prefix, session_id
        ));
        let thread_prefix_clone = thread_prefix.clone();
        let stopped_clone = stopped.clone();
        let activity_node_clone = activity_node.clone();

        let handle = thread::Builder::new()
            .name(format!("{}:{}", thread_prefix, self.session_id.to_hex_string()))
            .spawn(move || {
                crate::utils::install_simplex_panic_hook_once();

                log::info!(
                    "SimplexReceiver {} thread started for session {}",
                    thread_prefix,
                    session_id
                );

                let stop_for_panic = stop.clone();
                let stopped_for_panic = stopped.clone();

                let result = catch_unwind(AssertUnwindSafe(|| {
                    thread_fn(stop, activity_node);
                }));

                if let Err(panic_payload) = result {
                    log::error!(
                        "FATAL PANIC (PANIC-1): caught panic in {}: payload=\"{}\"; forcing receiver stop",
                        thread::current().name().unwrap_or("<unnamed>"),
                        crate::utils::panic_payload_to_string(panic_payload.as_ref())
                    );
                    panicked_flag.store(true, Ordering::Release);
                    stop_for_panic.store(true, Ordering::Release);
                }

                log::info!(
                    "SimplexReceiver {} thread exited for session {}",
                    thread_prefix,
                    session_id
                );

                // Always mark thread as stopped (normal exit or panic).
                stopped_for_panic.store(true, Ordering::Relaxed);
            })?;

        self.threads.push(ReceiverThreadDesc {
            thread_prefix: thread_prefix_clone,
            stopped: stopped_clone.clone(),
            thread_handle: handle,
            _activity_node: activity_node_clone,
        });

        Ok(stopped_clone)
    }

    fn stop_threads(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);

        let all_stopped = self.threads.iter().all(|t| t.stopped.load(Ordering::Relaxed));
        if all_stopped {
            return;
        }

        let session_id = self.session_id.to_hex_string();
        log::info!("Stopping SimplexReceiver for session {}", session_id);

        loop {
            let all_stopped = self.threads.iter().all(|t| t.stopped.load(Ordering::Relaxed));
            if all_stopped {
                break;
            }

            let threads_to_dump = self
                .threads
                .iter()
                .filter(|t| !t.stopped.load(Ordering::Relaxed))
                .map(|t| t.thread_prefix.clone())
                .collect::<Vec<_>>()
                .join(", ");

            log::info!(
                "...waiting for SimplexReceiver threads for session {}: {:?}",
                session_id,
                threads_to_dump
            );

            const CHECKING_INTERVAL: Duration = Duration::from_millis(500);
            thread::sleep(CHECKING_INTERVAL);
        }

        log::info!("Stopped SimplexReceiver for session {}", session_id);
    }

    fn remove_all_threads(&mut self) {
        log::info!(
            "Removing all SimplexReceiver threads for session {}",
            self.session_id.to_hex_string()
        );

        for thread in &mut self.threads.drain(..) {
            if let Err(err) = thread.thread_handle.join() {
                log::error!(
                    "Error joining SimplexReceiver thread {} for session {}: {:?}",
                    thread.thread_prefix,
                    self.session_id.to_hex_string(),
                    err
                );
            }
        }

        log::info!(
            "Removed all SimplexReceiver threads for session {}",
            self.session_id.to_hex_string()
        );
    }
}

/*
    Per-source statistics
*/

#[derive(Clone)]
struct SourceStats {
    /// Source index
    source_idx: u32,
    /// ADNL ID
    adnl_id: PublicKeyHash,
    /// Public key (for signature verification)
    public_key: PublicKey,
    /// Weight
    weight: ValidatorWeight,
    /// Incoming messages count
    in_messages: u64,
    /// Outgoing messages count
    out_messages: u64,
    /// Incoming broadcasts count
    in_broadcasts: u64,
    /// Outgoing broadcasts count
    out_broadcasts: u64,
    /// Last receive time
    last_recv_time: Option<SystemTime>,
    /// Last send time
    last_send_time: Option<SystemTime>,
}

impl SourceStats {
    fn new(
        source_idx: u32,
        adnl_id: PublicKeyHash,
        public_key: PublicKey,
        weight: ValidatorWeight,
    ) -> Self {
        Self {
            source_idx,
            adnl_id,
            public_key,
            weight,
            in_messages: 0,
            out_messages: 0,
            in_broadcasts: 0,
            out_broadcasts: 0,
            last_recv_time: None,
            last_send_time: None,
        }
    }
}

/*
    Deduplication key
*/

#[derive(Hash, Eq, PartialEq, Clone)]
struct DeduplicationKey {
    source_idx: u32,
    vote_hash: UInt256,
}

/*
    ReceiverImpl - internal state, single-threaded operations
*/

pub(crate) struct ReceiverImpl {
    /// Session ID
    session_id: SessionId,
    /// Overlay ID (incarnation)
    overlay_id: SessionId,
    /// Overlay short ID
    overlay_short_id: PublicKeyHash,
    /// Overlay for sending messages
    overlay: ConsensusOverlayPtr,
    /// Local validator key
    local_key: PrivateKey,
    /// Local ADNL ID
    local_adnl_id: PublicKeyHash,
    /// Local source index
    local_idx: u32,
    /// Receiver listener (weak reference to SessionProcessor)
    listener: ReceiverListenerPtr,
    /// Per-source statistics
    sources: Vec<SourceStats>,
    /// ADNL ID to source index mapping
    adnl_to_idx: HashMap<PublicKeyHash, u32>,
    /// Public key hash to source index mapping
    pubkey_to_idx: HashMap<PublicKeyHash, u32>,
    /// Randomized send order (shuffled periodically)
    send_order: Vec<u32>,
    /// Last shuffle time
    last_shuffle_time: SystemTime,
    /// Deduplication: per-slot HashMaps for received votes
    /// Note: Cleaned up via cleanup_slot() when slots are finalized
    /// Key: (source_idx, signature_hash), Value: received flag
    dedup_votes: HashMap<u32, HashMap<DeduplicationKey, bool>>,
    /// Shard identifier for this consensus session (for BlockIdExt construction)
    shard: ShardIdent,
    /// Maximum block + collated data size for candidate verification
    max_candidate_size: usize,
    /// Protocol version from consensus config (determines BOC serialization flags)
    proto_version: u32,
    /// Metrics
    in_messages_bytes: metrics::Counter,
    out_messages_bytes: metrics::Counter,
    in_broadcasts_bytes: metrics::Counter,
    out_broadcasts_bytes: metrics::Counter,
    in_bytes: metrics::Counter,
    out_bytes: metrics::Counter,
    /// Metrics (counts)
    out_messages_count: metrics::Counter,
    out_broadcasts_count: metrics::Counter,
    /// Activity node for liveness tracking
    _activity_node: ActivityNodePtr,
    /// Standstill timeout duration
    standstill_timeout: Duration,
    /// Next standstill alarm timestamp (reset on finalization and after re-broadcast)
    standstill_alarm: Option<SystemTime>,
    /// Standstill slot range [begin, end) for vote re-broadcast
    /// Only votes in this range will be re-broadcast on standstill.
    /// Reference: C++ pool.cpp tracked_slots_interval() = [first_non_finalized, current_window_end)
    ///
    /// Initialized to [STANDSTILL_INITIAL_SLOT_BEGIN, STANDSTILL_INITIAL_SLOT_END) to allow
    /// standstill re-broadcast before first finalization.
    /// After first finalization, SessionProcessor calls set_standstill_slots() with the actual range.
    standstill_slot_begin: u32,
    standstill_slot_end: u32,
    /// Our votes that we've sent (for re-broadcast on standstill)
    /// Stored when send_vote_impl() is called
    /// Format: (slot, signed_vote)
    our_votes: Vec<(u32, TlVote)>,
    /// Candidate resolver cache (local to this thread)
    resolver_cache: CandidateResolverCache,
    /// Delayed actions to execute at scheduled times
    /// Used for candidate request retries, query timeouts, etc.
    delayed_actions: Vec<ReceiverDelayedAction>,
    /// Pending candidate requests (outbound): (slot, block_hash) → request state
    /// Used to track ongoing block repair requests to other validators
    pending_requests: HashMap<(SlotIndex, UInt256), CandidateRequestState>,
    /// Task queues for posting callbacks from overlay responses
    task_queues: Arc<ReceiverTaskQueues>,
    /// Standstill certificate cache: slot → certificate bundle bytes
    /// Cached for re-broadcast during standstill. Cleaned up with slot cleanup.
    /// Reference: C++ pool.cpp CertificateBundle per slot
    standstill_certs: HashMap<u32, StandstillCertificateBundleBuffers>,
    /// Last finalization certificate for standstill replay
    /// Always re-broadcast during standstill, even if slot is outside tracked range.
    /// Format: (slot, serialized_cert_bytes)
    /// Reference: C++ pool.cpp last_final_cert_
    last_final_cert: Option<(u32, Vec<u8>)>,
    /// Finalization cursor for ingress DoS protection.
    /// Updated by `cleanup()` when SessionProcessor advances finalization.
    /// Used to reject far-future votes/certificates before expensive operations
    /// (signature verification, dedup HashMap insertion).
    first_active_slot: u32,
    candidate_requests_counter: metrics::Counter,
    candidate_request_retries_counter: metrics::Counter,
    candidate_request_timeouts_counter: metrics::Counter,
    candidate_request_giveups_counter: metrics::Counter,
    standstill_triggers_counter: metrics::Counter,
    standstill_votes_rebroadcast_counter: metrics::Counter,
    standstill_certs_rebroadcast_counter: metrics::Counter,
    health_counters: Arc<ReceiverHealthCounters>,
}

/// Serialized certificate bytes bundle for standstill replay
///
/// Stores serialized TL certificate bytes per slot for standstill re-broadcast.
/// Reference: C++ pool.cpp CertificateBundle::serialize_to()
#[derive(Default, Clone)]
struct StandstillCertificateBundleBuffers {
    /// Serialized notarization certificate bytes
    notar: Option<Vec<u8>>,
    /// Serialized skip certificate bytes
    skip: Option<Vec<u8>>,
    /// Serialized finalization certificate bytes
    final_: Option<Vec<u8>>,
}

impl ReceiverImpl {
    /// Process incoming vote message
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `vote` - Deserialized vote
    /// * `raw_vote` - Original serialized vote bytes (Arc-wrapped for memory-efficient sharing)
    fn process_vote(&mut self, source_idx: u32, vote: TlVoteBoxed, raw_vote: RawVoteData) {
        //check_execution_time!(20_000); //TODO: LK: restore during performance testing
        instrument!();

        // Update source stats
        if let Some(stats) = self.sources.get_mut(source_idx as usize) {
            stats.in_messages += 1;
            stats.last_recv_time = Some(SystemTime::now());
        }

        // DoS protection: reject far-future/negative slots BEFORE expensive
        // signature verification and dedup HashMap insertion.
        let slot = Self::get_vote_slot(&vote);
        if self.is_slot_out_of_bounds(slot) {
            log::warn!(
                "SimplexReceiver {}: REJECTED vote from source {} - slot {} out of bounds [{}, {}]",
                self.session_id.to_hex_string(),
                source_idx,
                slot,
                self.first_active_slot,
                self.max_acceptable_slot()
            );
            return;
        }

        // Verify signature before processing
        // C++ simplex-pool.cpp: serialize vote (was unsignedVote), check_signature against signature
        if let Err(err) = self.verify_vote_signature(source_idx, &vote) {
            log::warn!(
                "SimplexReceiver {}: MISBEHAVIOR: Dropping invalid vote from validator {}: {}",
                self.session_id.to_hex_string(),
                source_idx,
                err
            );
            return;
        }

        // Deduplicate based on signature (unique per vote, no serialization needed)
        let signature_hash = Self::compute_signature_hash(&vote);
        let dedup_key = DeduplicationKey { source_idx, vote_hash: signature_hash };

        let slot_dedup = self.dedup_votes.entry(slot).or_default();
        if slot_dedup.contains_key(&dedup_key) {
            log::trace!(
                "SimplexReceiver {}: duplicate vote from source {} slot {}, skipping",
                self.session_id.to_hex_string(),
                source_idx,
                slot
            );
            return;
        }
        slot_dedup.insert(dedup_key, true);

        // Forward to listener with raw bytes for misbehavior proof storage
        if let Some(listener) = self.listener.upgrade() {
            listener.on_vote(source_idx, vote, raw_vote);
        }
    }

    /// Process incoming certificate
    ///
    /// Handles `consensus.simplex.certificate` messages received on the vote channel.
    /// C++ nodes broadcast certificates when thresholds are reached (notarization, skip, finalize).
    ///
    /// Reference: C++ pool.cpp `handle(IncomingProtocolMessage)` parses `tl::certificate`
    /// and calls `handle_foreign_certificate(cert)`.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `certificate` - Deserialized TL certificate object
    fn process_incoming_certificate(&mut self, source_idx: u32, certificate: Certificate) {
        instrument!();

        // Update source stats
        if let Some(stats) = self.sources.get_mut(source_idx as usize) {
            stats.in_messages += 1;
            stats.last_recv_time = Some(SystemTime::now());
        }

        // Avoid logging full TL certificate (includes signature bytes) on the hot path.
        let (kind, slot, hash_prefix, sigs) = match &certificate {
            Certificate::Consensus_Simplex_Certificate(c) => {
                let sigs = c.signatures.votes().len();
                match &c.vote {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(v) => (
                        "notarize",
                        *v.id.slot() as u32,
                        hex::encode(&v.id.hash().as_slice()[..4]),
                        sigs,
                    ),
                    UnsignedVote::Consensus_Simplex_FinalizeVote(v) => (
                        "finalize",
                        *v.id.slot() as u32,
                        hex::encode(&v.id.hash().as_slice()[..4]),
                        sigs,
                    ),
                    UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                        ("skip", v.slot as u32, "-".to_string(), sigs)
                    }
                }
            }
        };
        log::trace!(
            "SimplexReceiver {}: received certificate from source {} kind={} slot={} hash={} sigs={}",
            self.session_id.to_hex_string(),
            source_idx,
            kind,
            slot,
            hash_prefix,
            sigs
        );

        // DoS protection: reject far-future/negative slots before forwarding.
        if self.is_slot_out_of_bounds(slot) {
            log::warn!(
                "SimplexReceiver {}: REJECTED certificate from source {} - slot {} out of bounds [{}, {}] kind={}",
                self.session_id.to_hex_string(),
                source_idx,
                slot,
                self.first_active_slot,
                self.max_acceptable_slot(),
                kind
            );
            return;
        }

        // Forward to listener for verification and application
        // SessionProcessor will verify the certificate signatures and update SimplexState
        if let Some(listener) = self.listener.upgrade() {
            listener.on_certificate(source_idx, certificate);
        }
    }

    /// Verify vote signature with session-scoped signature verification
    ///
    /// Uses the same session-scoped signature scheme as sign_vote() in utils.
    fn verify_vote_signature(&self, source_idx: u32, vote: &TlVoteBoxed) -> Result<()> {
        // Get source public key
        let source = self
            .sources
            .get(source_idx as usize)
            .ok_or_else(|| error!("Unknown source index: {}", source_idx))?;

        // Verify with session-scoped signature (matches sign_vote)
        if !crate::utils::verify_vote_signature(vote, &self.session_id, &source.public_key) {
            fail!("signature error: Verification equation was not satisfied")
        }

        Ok(())
    }

    /// Process incoming block candidate
    ///
    /// Deserializes, verifies signature, caches for resolver, and forwards to listener.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `candidate_bytes` - Serialized candidate data (TL)
    fn process_block_broadcast(&mut self, source_idx: u32, candidate_bytes: Vec<u8>) {
        check_execution_time!(50_000);
        instrument!();

        // Deserialize TL message
        let candidate = match deserialize_boxed(&candidate_bytes) {
            Ok(message) => match message.downcast::<CandidateData>() {
                Ok(c) => c,
                Err(_) => {
                    log::warn!(
                        "SimplexReceiver {}: unknown broadcast type from source {}",
                        self.session_id.to_hex_string(),
                        source_idx
                    );
                    return;
                }
            },
            Err(err) => {
                log::warn!(
                    "SimplexReceiver {}: failed to deserialize broadcast from source {}: {}",
                    self.session_id.to_hex_string(),
                    source_idx,
                    err
                );
                return;
            }
        };

        log::trace!(
            "SimplexReceiver {}: received block candidate from source {}, slot={}",
            self.session_id.to_hex_string(),
            source_idx,
            candidate.slot()
        );

        // Extract common fields and compute hash based on variant type
        // Empty blocks use different TL type for hash (candidateHashDataEmpty vs candidateHashDataOrdinary)
        let (slot, signature, candidate_hash): (i32, &[u8], UInt256) = match &candidate {
            CandidateData::Consensus_Block(block) => {
                // Non-empty block: extract parent and candidate bytes
                let parent_info = match &block.parent {
                    CandidateParent::Consensus_CandidateWithoutParents => None,
                    CandidateParent::Consensus_CandidateParent(p) => {
                        let id_slot = *p.id.slot();
                        let id_hash = p.id.hash().clone();
                        Some((id_slot, id_hash))
                    }
                };

                // Check candidate size
                if block.candidate.len() > self.max_candidate_size {
                    log::warn!(
                        "SimplexReceiver {}: REJECT candidate from source {} - size {} exceeds max {}",
                        self.session_id.to_hex_string(),
                        source_idx,
                        block.candidate.len(),
                        self.max_candidate_size
                    );
                    return;
                }

                // Extract block info from candidate bytes
                let (block_id, collated_file_hash) =
                    match crate::utils::extract_block_info_from_candidate(
                        &block.candidate,
                        &self.shard,
                        self.max_candidate_size,
                        self.proto_version,
                    ) {
                        Ok(Some(info)) => (Some(info.block_id), Some(info.collated_file_hash)),
                        Ok(None) => (None, None),
                        Err(e) => {
                            log::warn!(
                                "SimplexReceiver {}: failed to extract block info from candidate: {}",
                                self.session_id.to_hex_string(),
                                e
                            );
                            return;
                        }
                    };

                // Compute hash using candidateHashDataOrdinary
                let slot_idx = crate::block::SlotIndex(block.slot as u32);
                let hash = crate::utils::compute_candidate_id_hash(
                    slot_idx,
                    block_id.as_ref(),
                    collated_file_hash.as_ref(),
                    parent_info.as_ref().map(|(s, h)| (crate::block::SlotIndex(*s as u32), h)),
                );

                (block.slot, &block.signature[..], hash)
            }
            CandidateData::Consensus_Empty(empty) => {
                // Empty block: use candidateHashDataEmpty with parent CandidateId
                let parent_slot = crate::block::SlotIndex(*empty.parent.slot() as u32);
                let parent_hash = empty.parent.hash();

                // Compute hash using candidateHashDataEmpty
                // Reference: C++ CandidateId::create_hash_data() uses consensus_candidateHashDataEmpty
                let hash = crate::utils::compute_candidate_id_hash_empty(
                    &empty.block,
                    (parent_slot, parent_hash),
                );

                (empty.slot, &empty.signature[..], hash)
            }
        };

        // Get source public key for signature verification
        let public_key = match self.sources.get(source_idx as usize) {
            Some(stats) => stats.public_key.clone(),
            None => {
                log::warn!(
                    "SimplexReceiver {}: received candidate from unknown source {}",
                    self.session_id.to_hex_string(),
                    source_idx
                );
                return;
            }
        };

        // Convert slot for utility functions
        let slot_idx = crate::block::SlotIndex(slot as u32);

        // Verify signature
        if !crate::utils::check_candidate_signature(
            &self.session_id,
            slot_idx,
            &candidate_hash,
            signature,
            &public_key,
        ) {
            log::warn!(
                "SimplexReceiver {}: MISBEHAVIOR: Invalid candidate signature from validator {}, slot={}",
                self.session_id.to_hex_string(),
                source_idx,
                slot
            );
            return;
        }

        // Update source stats
        if let Some(stats) = self.sources.get_mut(source_idx as usize) {
            stats.in_broadcasts += 1;
            stats.last_recv_time = Some(SystemTime::now());
        }

        log::trace!(
            "SimplexReceiver {}: verified candidate signature from validator {}, slot={}",
            self.session_id.to_hex_string(),
            source_idx,
            slot
        );

        // Cache candidate for query responses (candidate resolver)
        // Reference: C++ CandidateResolver caches candidates on CandidateReceived event
        self.resolver_cache.cache_candidate(slot_idx, candidate_hash.clone(), candidate_bytes);

        // Forward to listener (no deduplication for blocks - SessionProcessor handles it)
        // None notar_cert for broadcasts - certificate comes separately or via query
        if let Some(listener) = self.listener.upgrade() {
            listener.on_candidate_received(source_idx, candidate, None);
        }
    }

    /// Receive message from overlay
    fn receive_message_from_overlay(&mut self, adnl_id: PublicKeyHash, data: BlockPayloadPtr) {
        check_execution_time!(50_000);
        instrument!();

        // Find source index
        let source_idx = match self.adnl_to_idx.get(&adnl_id) {
            Some(idx) => *idx,
            None => {
                log::warn!(
                    "SimplexReceiver {}: received message from unknown ADNL ID {}",
                    self.session_id.to_hex_string(),
                    key_to_base64(&adnl_id)
                );
                return;
            }
        };

        // Capture raw bytes before deserialization (for misbehavior proofs)
        // Wrap in RawVoteData (Arc) early for memory-efficient sharing
        let raw_vote: RawVoteData = data.data().to_vec().into();

        // Deserialize TL message
        // Reference: C++ pool.cpp `handle(IncomingProtocolMessage)` parses both
        // `tl::vote` and `tl::certificate` on the same channel.
        let message = match deserialize_boxed(data.data()) {
            Ok(msg) => msg,
            Err(err) => {
                log::warn!(
                    "SimplexReceiver {}: failed to deserialize message from source {}: {}",
                    self.session_id.to_hex_string(),
                    source_idx,
                    err
                );
                return;
            }
        };

        // Try Vote first (most common message type)
        // downcast returns Err(self) on failure, so we can try the next type
        let message = match message.downcast::<TlVoteBoxed>() {
            Ok(vote) => {
                // Avoid logging full TL vote (includes signature bytes) on the hot path.
                let (kind, slot, hash_prefix) = match vote.vote() {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
                        ("notarize", *v.id.slot() as u32, hex::encode(&v.id.hash().as_slice()[..4]))
                    }
                    UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
                        ("finalize", *v.id.slot() as u32, hex::encode(&v.id.hash().as_slice()[..4]))
                    }
                    UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                        ("skip", v.slot as u32, "-".to_string())
                    }
                };
                log::trace!(
                    "SimplexReceiver {}: received vote from source {} kind={} slot={} hash={}",
                    self.session_id.to_hex_string(),
                    source_idx,
                    kind,
                    slot,
                    hash_prefix
                );
                self.process_vote(source_idx, vote, raw_vote);
                return;
            }
            Err(message) => message,
        };

        // Try Certificate
        let _message = match message.downcast::<Certificate>() {
            Ok(cert) => {
                // Avoid logging full TL certificate (includes signature bytes) on the hot path.
                let sigs = cert.signatures().votes().len();
                let (kind, slot, hash_prefix) = match cert.vote() {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
                        ("notarize", *v.id.slot() as u32, hex::encode(&v.id.hash().as_slice()[..4]))
                    }
                    UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
                        ("finalize", *v.id.slot() as u32, hex::encode(&v.id.hash().as_slice()[..4]))
                    }
                    UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                        ("skip", v.slot as u32, "-".to_string())
                    }
                };
                log::trace!(
                    "SimplexReceiver {}: received certificate from source {} kind={} slot={} hash={} sigs={}",
                    self.session_id.to_hex_string(),
                    source_idx,
                    kind,
                    slot,
                    hash_prefix,
                    sigs
                );
                self.process_incoming_certificate(source_idx, cert);
                return;
            }
            Err(message) => message,
        };

        // Unknown message type
        log::warn!(
            "SimplexReceiver {}: unknown message type from source {}",
            self.session_id.to_hex_string(),
            source_idx
        );
    }

    /// Receive broadcast from overlay
    fn receive_broadcast_from_overlay(
        &mut self,
        source_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
    ) {
        check_execution_time!(50_000);
        instrument!();

        // Find source index by public key hash
        let source_idx = match self.pubkey_to_idx.get(&source_key_hash) {
            Some(idx) => *idx,
            None => {
                log::warn!(
                    "SimplexReceiver {}: received broadcast from unknown source {}",
                    self.session_id.to_hex_string(),
                    key_to_base64(&source_key_hash)
                );
                return;
            }
        };

        // Process broadcast (deserialization happens inside)
        self.process_block_broadcast(source_idx, data.data().to_vec());
    }

    /// Handle incoming query (requestCandidate)
    ///
    /// Reference: C++ CandidateResolver processes requestCandidate queries.
    /// On cache miss, delegates to SessionProcessor via `on_candidate_query_fallback`
    /// which can reconstruct the response from in-memory or DB-backed storage.
    fn handle_query(
        &mut self,
        _adnl_id: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        check_execution_time!(50_000);

        let request_data = data.data();
        let object = match deserialize_boxed(request_data) {
            Ok(object) => object,
            Err(e) => {
                log::warn!(
                    "SimplexReceiver {}: on_query failed to deserialize: {}",
                    self.session_id.to_hex_string(),
                    e
                );
                response_callback(Err(error!("Failed to deserialize query: {}", e)));
                return;
            }
        };

        let _object = match object.downcast::<RequestCandidate>() {
            Ok(req) => {
                let slot = SlotIndex::new(*req.id.slot() as u32);
                let block_hash = UInt256::from_slice(req.id.hash().as_slice());
                let want_candidate: bool = req.want_candidate.into();
                let want_notar: bool = req.want_notar.into();

                log::trace!(
                    "SimplexReceiver {}: requestCandidate slot={} hash={} want_candidate={} want_notar={}",
                    self.session_id.to_hex_string(),
                    slot,
                    &block_hash.to_hex_string()[..8],
                    want_candidate,
                    want_notar
                );

                let candidate_bytes = if want_candidate {
                    self.resolver_cache.get_candidate(slot, &block_hash).cloned()
                } else {
                    None
                };

                let cache_miss = want_candidate && candidate_bytes.is_none();

                if cache_miss {
                    if let Some(listener) = self.listener.upgrade() {
                        log::debug!(
                            "SimplexReceiver {}: requestCandidate cache MISS \
                            for slot={slot} hash={}, delegating to SessionProcessor",
                            self.session_id.to_hex_string(),
                            &block_hash.to_hex_string()[..8],
                        );
                        listener.on_candidate_query_fallback(
                            slot,
                            block_hash,
                            want_notar,
                            response_callback,
                        );
                    } else {
                        log::warn!(
                            "SimplexReceiver {}: requestCandidate cache MISS but listener dropped",
                            self.session_id.to_hex_string(),
                        );
                        response_callback(Err(error!("Session listener dropped")));
                    }
                    return;
                }

                let candidate_bytes = candidate_bytes.unwrap_or_default();
                let notar_bytes = if want_notar {
                    self.resolver_cache
                        .get_notar_cert(slot, &block_hash)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

                let response = CandidateAndCert {
                    candidate: candidate_bytes.into(),
                    notar: notar_bytes.into(),
                };

                let result = match serialize_boxed(&response.into_boxed()) {
                    Ok(response_bytes) => {
                        log::trace!(
                            "SimplexReceiver {}: requestCandidate response size={}",
                            self.session_id.to_hex_string(),
                            response_bytes.len()
                        );
                        Ok(ConsensusCommonFactory::create_block_payload(response_bytes))
                    }
                    Err(e) => {
                        log::error!(
                            "SimplexReceiver {}: failed to serialize requestCandidate response: {}",
                            self.session_id.to_hex_string(),
                            e
                        );
                        Err(error!("Failed to serialize response: {}", e))
                    }
                };

                response_callback(result);
                return;
            }
            Err(object) => object,
        };

        log::warn!(
            "SimplexReceiver {}: on_query unknown request type (tl_tag=#{:08x})",
            self.session_id.to_hex_string(),
            tag_from_data(request_data),
        );
        response_callback(Err(error!("Unknown query type")));
    }

    /// Cache candidate data for resolver queries
    fn cache_candidate(&mut self, slot: SlotIndex, block_hash: UInt256, data: Vec<u8>) {
        log::trace!(
            "SimplexReceiver {}: caching candidate for slot={} hash={}",
            self.session_id.to_hex_string(),
            slot,
            &block_hash.to_hex_string()[..8]
        );
        self.resolver_cache.cache_candidate(slot, block_hash, data);
    }

    /// Cache notarization certificate for resolver queries
    fn cache_notarization_cert(&mut self, slot: SlotIndex, block_hash: UInt256, data: Vec<u8>) {
        log::trace!(
            "SimplexReceiver {}: caching notarization cert for slot={} hash={}",
            self.session_id.to_hex_string(),
            slot,
            &block_hash.to_hex_string()[..8]
        );
        self.resolver_cache.cache_notar_cert(slot, block_hash, data);
    }

    /// Cache candidate bytes for resolver queries (startup recovery)
    ///
    /// Called from ReceiverWrapper::cache_candidate_bytes during session startup
    /// to restore resolver cache from bootstrap data.
    fn cache_candidate_bytes(&mut self, slot: SlotIndex, block_hash: UInt256, data: Vec<u8>) {
        log::trace!(
            "SimplexReceiver {}: caching candidate bytes for slot={} hash={} (startup recovery)",
            self.session_id.to_hex_string(),
            slot,
            &block_hash.to_hex_string()[..8]
        );
        self.resolver_cache.cache_candidate(slot, block_hash, data);
    }

    /// Cleanup resolver cache for old slots
    ///
    /// Called from cleanup_slot_impl with up_to_slot from SessionProcessor
    fn cleanup_resolver_cache(&mut self, up_to_slot: SlotIndex) {
        self.resolver_cache.cleanup_before(up_to_slot);
        log::trace!(
            "SimplexReceiver {}: cleaned up resolver cache for slots < {}",
            self.session_id.to_hex_string(),
            up_to_slot
        );
    }

    /*
        Candidate Request (Block Repair)

        This implements the client-side candidate resolver for requesting missing blocks
        from peers. The complete flow involves both ReceiverImpl and SessionProcessor:

        ┌─────────────────────────────────────────────────────────────────────────────────┐
        │ Candidate Request Flow (with delayed request for slow broadcasts)               │
        │                                                                                 │
        │  1. SessionProcessor::process_simplex_events():                                 │
        │     ├── BlockFinalized event received                                           │
        │     ├── can_finalize_block(&e) returns false (no candidate data)                │
        │     ├── schedule_request_candidate(slot, block_hash)                            │
        │     │   ├── Check requested_candidates set → skip if already scheduled          │
        │     │   ├── Add to requested_candidates set (mark as scheduled)                 │
        │     │   └── Post delayed action (CANDIDATE_REQUEST_DELAY = 1s)                  │
        │     └── Push event back for later processing                                    │
        │                                                                                 │
        │  1b. SessionProcessor delayed action fires (after 1 second):                    │
        │     ├── Check if received_candidates now has candidate                          │
        │     │   ├── YES: Broadcast arrived while waiting → remove from requested, done  │
        │     │   └── NO: Still missing → call receiver.request_candidate()               │
        │                                                                                 │
        │  2. ReceiverWrapper::request_candidate() posts to task queue                    │
        │                                                                                 │
        │  3. ReceiverImpl::request_candidate_impl():                                     │
        │     ├── Check pending_requests → skip if already pending                        │
        │     ├── Create CandidateRequestState                                            │
        │     ├── Pick peer (random selection)                                            │
        │     ├── Build RequestCandidate TL, send query                                   │
        │     └── Post delayed action for timeout                                         │
        │                                                                                 │
        │  4. On response:                                                                │
        │     ├── Deserialize CandidateAndCert                                            │
        │     ├── Remove from pending_requests                                            │
        │     └── Call listener.on_candidate_received(source_idx, candidate, Some(cert))  │
        │                                                                                 │
        │  5. On timeout (via process_delayed_actions in Receiver):                       │
        │     ├── If retry_count < max → retry with next peer (random)                    │
        │     └── Else → log warning, remove from pending_requests                        │
        │                                                                                 │
        │  6. SessionProcessor::on_candidate_received():                                  │
        │     ├── Store in received_candidates                                            │
        │     ├── If notar_cert:                                                          │
        │     │   ├── Deserialize TL → parse with signature verification                  │
        │     │   ├── Verify has_sufficient_weight (≥2/3) — reject if invalid             │
        │     │   └── If valid: set_notarize_certificate()                                │
        │     ├── Remove from requested_candidates set                                    │
        │     └── Next check_all() will process deferred BlockFinalized event             │
        └─────────────────────────────────────────────────────────────────────────────────┘

        Reference: C++ CandidateResolver in validator/consensus/candidate-resolver.cpp
    */

    /// Request a missing candidate from peers
    ///
    /// Sends a requestCandidate query to a peer and schedules retry on timeout.
    /// On successful response, calls on_candidate_received on the listener.
    fn request_candidate_impl(&mut self, slot: SlotIndex, block_hash: UInt256) {
        check_execution_time!(50_000);

        let key = (slot, block_hash.clone());

        // Check if already pending
        if self.pending_requests.contains_key(&key) {
            log::trace!(
                "SimplexReceiver {}: request_candidate slot={} hash={} - already pending",
                self.session_id.to_hex_string(),
                slot,
                &block_hash.to_hex_string()[..8],
            );
            return;
        }

        // Select a random peer to query (skip self)
        let source_idx = match self.select_peer_for_candidate_request(None) {
            Some(idx) => idx,
            None => {
                log::warn!(
                    "SimplexReceiver {}: request_candidate slot={} hash={} - no peers available",
                    self.session_id.to_hex_string(),
                    slot,
                    &block_hash.to_hex_string()[..8]
                );
                return;
            }
        };

        self.candidate_requests_counter.increment(1);

        // Create request state
        let request_state = CandidateRequestState {
            start_time: SystemTime::now(),
            retry_count: 0,
            current_timeout: CANDIDATE_REQUEST_INITIAL_TIMEOUT,
            source_idx,
            cached_notar: None,
            cached_candidate: None,
        };
        self.pending_requests.insert(key.clone(), request_state);

        // Send the query
        self.send_candidate_request(slot, block_hash.clone(), source_idx);

        // Schedule timeout handler
        let slot_clone = slot;
        let hash_clone = block_hash.clone();
        self.post_delayed_action(
            SystemTime::now() + CANDIDATE_REQUEST_INITIAL_TIMEOUT,
            move |receiver: &mut ReceiverImpl| {
                receiver.handle_candidate_request_timeout(slot_clone, hash_clone);
            },
        );
    }

    /// Select a peer for candidate request, skipping self and (optionally) a specific peer.
    ///
    /// This reduces repeated queries to the same peer across retries, improving
    /// convergence when only a subset of peers have the requested candidate.
    fn select_peer_for_candidate_request(&self, exclude: Option<u32>) -> Option<ValidatorIndex> {
        let len = self.send_order.len();
        if len <= 1 {
            return None; // Only self or empty
        }

        // Start from a random position and find first non-self peer
        let mut rng = rand::thread_rng();
        let start_idx = rng.gen_range(0..len);

        // First pass: skip self and excluded peer (if provided)
        for offset in 0..len {
            let idx = (start_idx + offset) % len;
            let validator_idx = self.send_order[idx];
            if validator_idx == self.local_idx {
                continue;
            }
            if let Some(ex) = exclude {
                if validator_idx == ex {
                    continue;
                }
            }
            return Some(ValidatorIndex::new(validator_idx));
        }

        // Fallback: ignore exclude, but still skip self
        for offset in 0..len {
            let idx = (start_idx + offset) % len;
            let validator_idx = self.send_order[idx];
            if validator_idx != self.local_idx {
                return Some(ValidatorIndex::new(validator_idx));
            }
        }
        None
    }

    /// Send the actual requestCandidate query to a peer
    fn send_candidate_request(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        source_idx: ValidatorIndex,
    ) {
        let peer_adnl_id = match self.sources.get(source_idx.value() as usize) {
            Some(stats) => stats.adnl_id.clone(),
            None => {
                log::error!(
                    "SimplexReceiver {}: send_candidate_request - invalid validator idx {}",
                    self.session_id.to_hex_string(),
                    source_idx
                );
                return;
            }
        };

        let candidate_id = CandidateId { slot: slot.value() as i32, hash: block_hash.clone() };
        let request = RequestCandidate {
            id: candidate_id.into_boxed(),
            want_candidate: true.into(),
            want_notar: true.into(),
        };
        let (serialized, query_name) = (serialize_boxed(&request), "requestCandidate");

        let serialized = match serialized {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "SimplexReceiver {}: failed to serialize {}: {}",
                    self.session_id.to_hex_string(),
                    query_name,
                    e
                );
                return;
            }
        };
        let payload = ConsensusCommonFactory::create_block_payload(serialized);

        log::trace!(
            "SimplexReceiver {}: sending {} slot={} hash={} to validator {}",
            self.session_id.to_hex_string(),
            query_name,
            slot,
            &block_hash.to_hex_string()[..8],
            source_idx,
        );

        // Capture data for callback (we need to move these into the closure)
        let slot_for_cb = slot;
        let hash_for_cb = block_hash.clone();
        let session_id = self.session_id.clone();
        let task_queues = self.get_task_queues();

        // Send query via overlay
        self.overlay.send_query(
            &peer_adnl_id,
            &self.local_adnl_id,
            query_name,
            CANDIDATE_REQUEST_TIMEOUT,
            &payload,
            Box::new(move |result: Result<consensus_common::BlockPayloadPtr>| {
                // Post response handling to receiver thread
                task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
                    receiver.handle_candidate_response(
                        slot_for_cb,
                        hash_for_cb,
                        result,
                        session_id,
                    );
                }));
            }),
        );
    }

    /// Get task queues for callback posting
    /// Note: This requires storing a reference to task_queues in ReceiverImpl
    fn get_task_queues(&self) -> Arc<ReceiverTaskQueues> {
        // This will be initialized during creation
        // For now, we need to add this field
        self.task_queues.clone()
    }

    /// Merge partial `requestCandidate` response pieces with pending-request state.
    ///
    /// C++ parity:
    /// - cache partial candidate/notar parts as they arrive;
    /// - merge with previously cached parts;
    /// - completion is checked by caller via non-empty merged candidate+notar.
    fn merge_candidate_response_parts(
        resolver_cache: &mut CandidateResolverCache,
        pending_state: Option<&mut CandidateRequestState>,
        slot: SlotIndex,
        block_hash: &UInt256,
        candidate_bytes: &[u8],
        notar_bytes: &[u8],
    ) -> (Vec<u8>, Vec<u8>) {
        let candidate_vec = candidate_bytes.to_vec();
        let notar_vec = notar_bytes.to_vec();

        if !candidate_vec.is_empty() {
            resolver_cache.cache_candidate(slot, block_hash.clone(), candidate_vec.clone());
        }
        if !notar_vec.is_empty() {
            resolver_cache.cache_notar_cert(slot, block_hash.clone(), notar_vec.clone());
        }

        if let Some(state) = pending_state {
            if !candidate_vec.is_empty() {
                state.cached_candidate = Some(candidate_vec);
            }
            if !notar_vec.is_empty() {
                state.cached_notar = Some(notar_vec.clone());
            }

            let merged_candidate = state.cached_candidate.clone().unwrap_or_default();
            let merged_notar = if !notar_vec.is_empty() {
                notar_vec
            } else if let Some(cached_notar) = state.cached_notar.clone() {
                cached_notar
            } else {
                resolver_cache.get_notar_cert(slot, block_hash).cloned().unwrap_or_default()
            };
            return (merged_candidate, merged_notar);
        }

        let merged_notar = if !notar_vec.is_empty() {
            notar_vec
        } else {
            resolver_cache.get_notar_cert(slot, block_hash).cloned().unwrap_or_default()
        };
        (candidate_bytes.to_vec(), merged_notar)
    }

    /// Handle response from requestCandidate query
    fn handle_candidate_response(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        result: Result<consensus_common::BlockPayloadPtr>,
        _session_id: SessionId,
    ) {
        check_execution_time!(50_000);

        let key = (slot, block_hash.clone());

        // Check if request is still pending (might have been fulfilled by broadcast)
        if !self.pending_requests.contains_key(&key) {
            log::trace!(
                "SimplexReceiver {}: candidate response for slot={} hash={} - no longer pending",
                self.session_id.to_hex_string(),
                slot,
                &block_hash.to_hex_string()[..8]
            );
            return;
        }

        match result {
            Ok(response_payload) => {
                // Deserialize response
                let response_data = response_payload.data();
                match deserialize_boxed(response_data) {
                    Ok(message) => {
                        if let Ok(response) = message.downcast::<CandidateAndCertBoxed>() {
                            // Get source_idx from pending request before removing
                            let source_idx = self
                                .pending_requests
                                .get(&key)
                                .map(|state| state.source_idx.value())
                                .unwrap_or(0);

                            // Successfully received response
                            let candidate_bytes = response.candidate();
                            let notar_bytes = response.notar();

                            log::trace!(
                                "SimplexReceiver {}: received candidate response slot={} hash={} \
                                candidate_len={} notar_len={} from validator {}",
                                self.session_id.to_hex_string(),
                                slot,
                                &block_hash.to_hex_string()[..8],
                                candidate_bytes.len(),
                                notar_bytes.len(),
                                source_idx
                            );

                            // C++ CandidateAndCert::merge parity: cache both partial fields and
                            // complete only when the merged result has both candidate+notar.
                            let (merged_candidate_bytes, merged_notar) =
                                Self::merge_candidate_response_parts(
                                    &mut self.resolver_cache,
                                    self.pending_requests.get_mut(&key),
                                    slot,
                                    &block_hash,
                                    candidate_bytes,
                                    notar_bytes,
                                );

                            // If body is still missing after merge, keep pending for retry.
                            if merged_candidate_bytes.is_empty() {
                                log::debug!(
                                    "SimplexReceiver {}: body-empty response for slot={} hash={} \
                                    (notar_len={}), will retry on timeout",
                                    self.session_id.to_hex_string(),
                                    slot,
                                    &block_hash.to_hex_string()[..8],
                                    notar_bytes.len(),
                                );
                                return;
                            }

                            if merged_notar.is_empty() {
                                log::debug!(
                                    "SimplexReceiver {}: candidate-only partial response for \
                                    slot={} hash={}, keep pending until notar arrives",
                                    self.session_id.to_hex_string(),
                                    slot,
                                    &block_hash.to_hex_string()[..8],
                                );
                                return;
                            }

                            let candidate = match deserialize_boxed(
                                merged_candidate_bytes.as_slice(),
                            ) {
                                Ok(msg) => match msg.downcast::<CandidateData>() {
                                    Ok(c) => c,
                                    Err(_) => {
                                        // Drop cached candidate so retry can fetch a fresh body;
                                        // also purge resolver_cache to avoid serving bad data to peers.
                                        self.resolver_cache.remove_candidate(slot, &block_hash);
                                        if let Some(state) = self.pending_requests.get_mut(&key) {
                                            state.cached_candidate = None;
                                        }
                                        log::warn!(
                                            "SimplexReceiver {}: unexpected candidate type in response",
                                            self.session_id.to_hex_string()
                                        );
                                        return;
                                    }
                                },
                                Err(e) => {
                                    // Drop cached candidate so retry can fetch a fresh body;
                                    // also purge resolver_cache to avoid serving bad data to peers.
                                    self.resolver_cache.remove_candidate(slot, &block_hash);
                                    if let Some(state) = self.pending_requests.get_mut(&key) {
                                        state.cached_candidate = None;
                                    }
                                    log::warn!(
                                        "SimplexReceiver {}: failed to deserialize candidate: {}",
                                        self.session_id.to_hex_string(),
                                        e
                                    );
                                    return;
                                }
                            };

                            // Remove from pending only when merged candidate+notar is complete.
                            self.pending_requests.remove(&key);

                            // Call listener with source_idx, using merged notar.
                            if let Some(listener) = self.listener.upgrade() {
                                listener.on_candidate_received(
                                    source_idx,
                                    candidate,
                                    Some(merged_notar),
                                );
                            }
                        } else {
                            log::warn!(
                                "SimplexReceiver {}: unexpected response type for requestCandidate",
                                self.session_id.to_hex_string()
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "SimplexReceiver {}: failed to deserialize candidate response: {}",
                            self.session_id.to_hex_string(),
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log::trace!(
                    "SimplexReceiver {}: candidate request failed slot={} hash={}: {}",
                    self.session_id.to_hex_string(),
                    slot,
                    &block_hash.to_hex_string()[..8],
                    e
                );
                // Error will be handled by timeout - don't retry here to avoid duplicates
            }
        }
    }

    /// Handle request timeout - retry with next peer using exponential backoff.
    /// C++ parity: candidate-resolver.cpp retries indefinitely until resolved.
    fn handle_candidate_request_timeout(&mut self, slot: SlotIndex, block_hash: UInt256) {
        let key = (slot, block_hash.clone());

        // Check if request is still pending and get current state
        let (retry_count, prev_source_idx, current_timeout) = match self.pending_requests.get(&key)
        {
            Some(state) => (state.retry_count, state.source_idx.value(), state.current_timeout),
            None => {
                log::trace!(
                    "SimplexReceiver {}: handle_candidate_request_timeout slot={} hash={} - request already fulfilled or cancelled",
                    self.session_id.to_hex_string(),
                    slot,
                    &block_hash.to_hex_string()[..8]
                );
                return;
            }
        };
        self.candidate_request_timeouts_counter.increment(1);

        let new_retry_count = retry_count + 1;
        if new_retry_count % CANDIDATE_REQUEST_MAX_RETRIES == 0 {
            log::warn!(
                "SimplexReceiver {}: candidate request slot={slot} hash={} \
                still pending after {new_retry_count} retries, continuing",
                self.session_id.to_hex_string(),
                &block_hash.to_hex_string()[..8]
            );
        }

        // Exponential backoff: timeout * multiplier, capped at max
        let next_timeout_ms =
            (current_timeout.as_millis() as f64 * CANDIDATE_REQUEST_TIMEOUT_MULTIPLIER) as u128;
        let next_timeout = Duration::from_millis(
            next_timeout_ms.min(CANDIDATE_REQUEST_MAX_TIMEOUT.as_millis()) as u64,
        );

        // Select next peer (random, excluding previous)
        let next_source_idx = match self.select_peer_for_candidate_request(Some(prev_source_idx)) {
            Some(idx) => idx,
            None => {
                // No peers available right now -- schedule a retry after backoff anyway,
                // peers may come back online.
                self.candidate_request_retries_counter.increment(1);
                log::warn!(
                    "SimplexReceiver {}: no peers for candidate request slot={slot} hash={}, \
                    will retry in {next_timeout:?}",
                    self.session_id.to_hex_string(),
                    &block_hash.to_hex_string()[..8]
                );
                if let Some(state) = self.pending_requests.get_mut(&key) {
                    state.retry_count = new_retry_count;
                    state.current_timeout = next_timeout;
                }
                let slot_clone = slot;
                let hash_clone = block_hash;
                self.post_delayed_action(
                    SystemTime::now() + next_timeout,
                    move |receiver: &mut ReceiverImpl| {
                        receiver.handle_candidate_request_timeout(slot_clone, hash_clone);
                    },
                );
                return;
            }
        };

        // Update request state
        self.candidate_request_retries_counter.increment(1);
        if let Some(state) = self.pending_requests.get_mut(&key) {
            state.retry_count = new_retry_count;
            state.source_idx = next_source_idx;
            state.current_timeout = next_timeout;
        }

        log::trace!(
            "SimplexReceiver {}: retrying candidate request slot={slot} hash={} \
            to validator {next_source_idx} (retry {new_retry_count}, timeout {next_timeout:?})",
            self.session_id.to_hex_string(),
            &block_hash.to_hex_string()[..8]
        );

        // Send to next peer
        self.send_candidate_request(slot, block_hash.clone(), next_source_idx);

        // Schedule next timeout with backoff
        let slot_clone = slot;
        let hash_clone = block_hash;
        self.post_delayed_action(
            SystemTime::now() + next_timeout,
            move |receiver: &mut ReceiverImpl| {
                receiver.handle_candidate_request_timeout(slot_clone, hash_clone);
            },
        );
    }

    /// Send a signed vote to all validators
    ///
    /// # Arguments
    /// * `vote` - Already signed TL vote to broadcast
    /// * `is_rebroadcast` - If true, this is a re-broadcast (no loopback, no save, marked as retransmission)
    fn send_vote_impl(&mut self, vote: TlVote, is_rebroadcast: bool) {
        check_execution_time!(20_000);
        instrument!();

        // Store vote for potential standstill re-broadcast (only on first send)
        if !is_rebroadcast {
            let slot = Self::get_vote_slot_from_inner(&vote);
            self.our_votes.push((slot, vote.clone()));
            // Keep standstill end large enough to include newly sent votes.
            // This avoids relying on external range updates for window growth (C++ parity: alarm() uses current state).
            self.standstill_slot_end = self.standstill_slot_end.max(slot.saturating_add(1));
        }

        // Serialize vote for network transmission
        let serialized = consensus_common::serialize_tl_boxed_object!(&vote.clone().into_boxed());
        let raw_vote: RawVoteData = serialized.into();
        let payload = consensus_common::ConsensusCommonFactory::create_block_payload(
            raw_vote.to_raw_buffer(),
        );

        log::trace!(
            "SimplexReceiver {}: {} vote to {} validators, size={}",
            self.session_id.to_hex_string(),
            if is_rebroadcast { "re-broadcasting" } else { "sending" },
            self.send_order.len(),
            payload.data().len()
        );

        // Update metrics (bytes are per-message, multiplied by recipient count)
        let msg_size = payload.data().len() as u64;
        let recipient_count = self.send_order.iter().filter(|&&idx| idx != self.local_idx).count();
        self.out_messages_bytes.increment(msg_size * recipient_count as u64);
        self.out_bytes.increment(msg_size * recipient_count as u64);
        self.out_messages_count.increment(recipient_count as u64);

        // Send to all validators in shuffled order
        for &target_idx in &self.send_order {
            if target_idx == self.local_idx {
                continue; // Skip self
            }

            if let Some(stats) = self.sources.get_mut(target_idx as usize) {
                stats.out_messages += 1;
                stats.last_send_time = Some(SystemTime::now());

                // Send via overlay
                // Note: is_retransmission=false because we never relay other validators' votes
                let is_retransmission = false;
                self.overlay.send_message(
                    &stats.adnl_id,
                    &self.local_adnl_id,
                    &payload,
                    is_retransmission,
                );
            }
        }

        // Process loopback - submit our own vote to the listener for FSM accounting
        // Only on first send (re-broadcast votes were already accounted for)
        if !is_rebroadcast {
            log::trace!(
                "SimplexReceiver {}: processing loopback for own vote",
                self.session_id.to_hex_string()
            );
            if let Some(listener) = self.listener.upgrade() {
                listener.on_vote(self.local_idx, vote.into_boxed(), raw_vote);
            }
        }
    }

    /// Cache a signed local vote for standstill replay (startup recovery only)
    ///
    /// This mirrors the C++ behavior where the pool keeps the local voting state
    /// and re-serializes it during standstill (`pool.cpp::alarm()`).
    ///
    /// This does NOT send the vote to the network.
    fn cache_our_vote_for_standstill_impl(&mut self, vote: TlVote) {
        let slot = Self::get_vote_slot_from_inner(&vote);
        log::trace!(
            "SimplexReceiver {}: caching local vote for standstill slot={} kind={:?}",
            self.session_id.to_hex_string(),
            slot,
            discriminant(&vote.vote)
        );
        self.our_votes.push((slot, vote));
        self.standstill_slot_end = self.standstill_slot_end.max(slot.saturating_add(1));
    }

    /// Send block candidate to all validators
    ///
    /// Handles both regular blocks (`Consensus_Block`) and empty blocks (`Consensus_Empty`).
    /// Also caches the candidate locally for the candidate resolver to serve to peers.
    ///
    /// # Arguments
    /// * `slot` - Slot number of the candidate
    /// * `candidate_hash` - Precomputed candidate hash (verified in debug builds)
    /// * `candidate` - TL candidate data to broadcast
    fn send_block_broadcast_impl(
        &mut self,
        slot: u32,
        candidate_hash: UInt256,
        candidate: CandidateData,
    ) {
        check_execution_time!(50_000);
        instrument!();

        // In debug builds, verify the passed slot and hash match what we'd compute
        #[cfg(debug_assertions)]
        {
            let (computed_slot, computed_hash): (i32, UInt256) = match &candidate {
                CandidateData::Consensus_Block(block) => {
                    let parent_info = match &block.parent {
                        CandidateParent::Consensus_CandidateWithoutParents => None,
                        CandidateParent::Consensus_CandidateParent(p) => {
                            let id_slot = *p.id.slot();
                            let id_hash = p.id.hash().clone();
                            Some((id_slot, id_hash))
                        }
                    };

                    let (block_id, collated_file_hash) =
                        match crate::utils::extract_block_info_from_candidate(
                            &block.candidate,
                            &self.shard,
                            self.max_candidate_size,
                            self.proto_version,
                        ) {
                            Ok(Some(info)) => (Some(info.block_id), Some(info.collated_file_hash)),
                            Ok(None) => (None, None),
                            Err(_) => (None, None),
                        };

                    let slot_idx = crate::block::SlotIndex(block.slot as u32);
                    let hash = crate::utils::compute_candidate_id_hash(
                        slot_idx,
                        block_id.as_ref(),
                        collated_file_hash.as_ref(),
                        parent_info.as_ref().map(|(s, h)| (crate::block::SlotIndex(*s as u32), h)),
                    );

                    (block.slot, hash)
                }
                CandidateData::Consensus_Empty(empty) => {
                    let parent_slot = crate::block::SlotIndex(*empty.parent.slot() as u32);
                    let parent_hash = empty.parent.hash();
                    let hash = crate::utils::compute_candidate_id_hash_empty(
                        &empty.block,
                        (parent_slot, parent_hash),
                    );
                    (empty.slot, hash)
                }
            };

            debug_assert_eq!(
                slot as i32, computed_slot,
                "send_block_broadcast_impl: slot mismatch (passed={}, computed={})",
                slot, computed_slot
            );
            debug_assert_eq!(
                candidate_hash,
                computed_hash,
                "send_block_broadcast_impl: hash mismatch (passed={}, computed={})",
                candidate_hash.to_hex_string(),
                computed_hash.to_hex_string()
            );
        }

        // Serialize candidate (CandidateData is already boxed)
        let serialized = consensus_common::serialize_tl_boxed_object!(&candidate);

        // Cache candidate for query responses (candidate resolver)
        // This allows other validators to request the candidate from us via requestCandidate
        let slot_idx = crate::block::SlotIndex(slot as u32);
        self.resolver_cache.cache_candidate(slot_idx, candidate_hash.clone(), serialized.clone());

        log::trace!(
            "SimplexReceiver {}: cached own candidate slot={} hash={} for resolver",
            self.session_id.to_hex_string(),
            slot,
            &candidate_hash.to_hex_string()[..8]
        );

        let payload = consensus_common::ConsensusCommonFactory::create_block_payload(serialized);

        log::trace!(
            "SimplexReceiver {}: sending block candidate, slot={}, size={}",
            self.session_id.to_hex_string(),
            slot,
            payload.data().len()
        );

        // Update metrics
        let msg_size = payload.data().len() as u64;
        self.out_broadcasts_count.increment(1);
        self.out_broadcasts_bytes.increment(msg_size);
        self.out_bytes.increment(msg_size);

        // Update local source stats
        if let Some(stats) = self.sources.get_mut(self.local_idx as usize) {
            stats.out_broadcasts += 1;
            stats.last_send_time = Some(SystemTime::now());
        }

        // Send via overlay FEC broadcast
        self.overlay.send_broadcast_fec_ex(&self.local_adnl_id, self.local_key.id(), payload);
    }

    /// Shuffle send order for fairness
    fn shuffle_send_order(&mut self) {
        let mut rng = rand::thread_rng();
        self.send_order.shuffle(&mut rng);
        self.last_shuffle_time = SystemTime::now();

        log::trace!("SimplexReceiver {}: shuffled send order", self.session_id.to_hex_string());
    }

    /// Calculate active weight (sum of weights for nodes with recent activity)
    ///
    /// Always includes our own weight since we're always active locally.
    fn calculate_active_weight(&self, activity_threshold: Duration) -> ValidatorWeight {
        let now = SystemTime::now();
        let mut active_weight: ValidatorWeight = 0;

        for stats in &self.sources {
            // Always count our own weight - we're always active locally
            // (we don't receive messages from ourselves, so last_recv_time is None)
            if stats.source_idx == self.local_idx {
                active_weight += stats.weight;
                continue;
            }

            if let Some(last_recv) = stats.last_recv_time {
                if let Ok(elapsed) = now.duration_since(last_recv) {
                    if elapsed < activity_threshold {
                        active_weight += stats.weight;
                    }
                }
            }
        }

        active_weight
    }

    /// Get last activity time for each validator
    ///
    /// Reports self as always-active (consistent with calculate_active_weight).
    fn get_last_activity(&self) -> Vec<Option<SystemTime>> {
        let now = SystemTime::now();
        self.sources
            .iter()
            .map(|s| if s.source_idx == self.local_idx { Some(now) } else { s.last_recv_time })
            .collect()
    }

    /// Debug dump of receiver state
    fn debug_dump(&self) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }

        let session_id_short = &self.session_id.to_hex_string()[..8];
        let sources_count = self.sources.len();

        log::debug!(
            "SimplexReceiver {} debug dump (local_idx={}, sources_count={}):",
            session_id_short,
            self.local_idx,
            sources_count
        );

        let now = SystemTime::now();
        for stats in &self.sources {
            let last_recv_ago = stats
                .last_recv_time
                .and_then(|t| now.duration_since(t).ok())
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_else(|| "never".to_string());
            let last_send_ago = stats
                .last_send_time
                .and_then(|t| now.duration_since(t).ok())
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_else(|| "never".to_string());

            let local_marker = if stats.source_idx == self.local_idx { " (local)" } else { "" };
            let prefix = if stats.source_idx == self.local_idx { ">" } else { " " };

            log::debug!(
                "{} {}v{:03}/{:03}: msgs={:4}/{:4}, bcasts={:4}/{:4}, \
                last_recv={:>7}, last_send={:>7}, adnl_id={}{}",
                session_id_short,
                prefix,
                stats.source_idx,
                sources_count,
                stats.in_messages,
                stats.out_messages,
                stats.in_broadcasts,
                stats.out_broadcasts,
                last_recv_ago,
                last_send_ago,
                &key_to_base64(&stats.adnl_id)[..11],
                local_marker
            );
        }
    }

    /// Compute hash from vote signature for deduplication
    /// Using signature directly as it's unique per vote and avoids serialization overhead
    fn compute_signature_hash(vote: &TlVoteBoxed) -> UInt256 {
        let signature = vote.signature();
        UInt256::calc_file_hash(signature)
    }

    /// Get slot from vote (boxed enum version)
    fn get_vote_slot(vote: &TlVoteBoxed) -> u32 {
        match vote.vote() {
            UnsignedVote::Consensus_Simplex_NotarizeVote(v) => *v.id.slot() as u32,
            UnsignedVote::Consensus_Simplex_FinalizeVote(v) => *v.id.slot() as u32,
            UnsignedVote::Consensus_Simplex_SkipVote(v) => v.slot as u32,
        }
    }

    /// Get slot from inner vote struct (avoids clone+box overhead)
    fn get_vote_slot_from_inner(vote: &TlVote) -> u32 {
        match &vote.vote {
            UnsignedVote::Consensus_Simplex_NotarizeVote(v) => *v.id.slot() as u32,
            UnsignedVote::Consensus_Simplex_FinalizeVote(v) => *v.id.slot() as u32,
            UnsignedVote::Consensus_Simplex_SkipVote(v) => v.slot as u32,
        }
    }

    /// Maximum slot the receiver will accept (inclusive).
    /// Mirrors `SimplexState::max_acceptable_slot()` using the receiver's own
    /// finalization cursor, which is updated by `cleanup()`.
    fn max_acceptable_slot(&self) -> u32 {
        self.first_active_slot.saturating_add(MAX_FUTURE_SLOTS)
    }

    /// Returns `true` if `slot` is outside the acceptable range
    /// `[first_active_slot, first_active_slot + MAX_FUTURE_SLOTS]`.
    /// Rejects both already-finalized slots and far-future slots.
    fn is_slot_out_of_bounds(&self, slot: u32) -> bool {
        slot < self.first_active_slot || slot > self.max_acceptable_slot()
    }

    /// Cleanup old slots data
    ///
    /// Called when SessionProcessor finalizes or skips a slot.
    /// Cleans up old votes, dedup entries, and resolver cache.
    ///
    ///
    /// # Arguments
    ///
    /// * `up_to_slot` - Clean up all data for slots < up_to_slot
    ///
    /// Reference: C++ pool.cpp notify_finalized()
    fn cleanup(&mut self, up_to_slot: SlotIndex) {
        log::trace!(
            "SimplexReceiver {}: cleanup up_to_slot={}",
            self.session_id.to_hex_string(),
            up_to_slot
        );

        self.first_active_slot = up_to_slot.value();

        // Clean up old votes (keep votes for slot >= up_to_slot)
        let old_count = self.our_votes.len();
        self.our_votes.retain(|(s, _)| *s >= up_to_slot.value());
        if self.our_votes.len() < old_count {
            log::trace!(
                "SimplexReceiver {}: cleaned up {} old votes (up_to_slot={})",
                self.session_id.to_hex_string(),
                old_count - self.our_votes.len(),
                up_to_slot
            );
        }

        // Clean up deduplication entries (keep entries for slot >= up_to_slot)
        self.dedup_votes.retain(|&slot, _| slot >= up_to_slot.value());

        // Clean up resolver cache for old slots
        self.cleanup_resolver_cache(up_to_slot);

        // Clean up standstill certificate cache
        let old_cert_count = self.standstill_certs.len();
        self.standstill_certs.retain(|&slot, _| slot >= up_to_slot.value());
        if self.standstill_certs.len() < old_cert_count {
            log::trace!(
                "SimplexReceiver {}: cleaned up {} old standstill cert bundles (up_to_slot={})",
                self.session_id.to_hex_string(),
                old_cert_count - self.standstill_certs.len(),
                up_to_slot
            );
        }

        // Note: standstill timer is NOT reset here - it's done separately via
        // reschedule_standstill() which is only called on finalization, not on skip.
        // Reference: C++ pool.cpp only calls reschedule_standstill_resolution() in on_finalization()
    }

    /// Reschedule standstill alarm
    ///
    /// Reference: C++ pool.cpp reschedule_standstill_resolution()
    fn reschedule_standstill(&mut self) {
        self.standstill_alarm = Some(SystemTime::now() + self.standstill_timeout);
        log::trace!(
            "SimplexReceiver {}: standstill alarm rescheduled to {:?}",
            self.session_id.to_hex_string(),
            self.standstill_timeout
        );
    }

    /// Check and handle standstill
    ///
    /// Called periodically from the main processing loop.
    /// If standstill alarm has expired, re-broadcast votes in the tracked slot range.
    ///
    /// Reference: C++ pool.cpp alarm()
    fn check_standstill(&mut self) {
        check_execution_time!(10_000);

        let now = SystemTime::now();

        // Check if standstill alarm has expired
        let should_trigger = match self.standstill_alarm {
            Some(alarm_time) => now >= alarm_time,
            None => false,
        };

        if !should_trigger {
            return;
        }

        // Filter votes to only those in tracked range [begin, end)
        // Reference: C++ pool.cpp alarm() iterates tracked_slots_interval()
        let begin = self.standstill_slot_begin;
        let end = self.standstill_slot_end;

        // 1. Re-broadcast cached certificates
        let cert_count = self.rebroadcast_standstill_certificates(begin, end);

        // 2. Re-broadcast our votes in tracked range, but ONLY if matching cert doesn't exist
        // Reference: C++ Tsentrizbirkom::serialize_to(messages, bundle):
        //   if (notarize_.has_value() && !bundle.notarize_.has_value()) { ... }
        //   if (skip_.has_value() && !bundle.skip_.has_value()) { ... }
        //   if (finalize_.has_value() && !bundle.finalize_.has_value()) { ... }
        let votes_to_rebroadcast: Vec<_> = self
            .our_votes
            .iter()
            .filter(|(slot, vote)| {
                if *slot < begin || *slot >= end {
                    return false;
                }
                // Check if we have the matching cert cached
                let bundle = self.standstill_certs.get(slot);
                match &vote.vote {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(_) => {
                        // Only send notar vote if no notar cert cached
                        bundle.map_or(true, |b| b.notar.is_none())
                    }
                    UnsignedVote::Consensus_Simplex_SkipVote(_) => {
                        // Only send skip vote if no skip cert cached
                        bundle.map_or(true, |b| b.skip.is_none())
                    }
                    UnsignedVote::Consensus_Simplex_FinalizeVote(_) => {
                        // Only send finalize vote if no final cert cached
                        bundle.map_or(true, |b| b.final_.is_none())
                    }
                }
            })
            .map(|(_, v)| v.clone())
            .collect();

        // Standstill detected - log summary
        self.standstill_triggers_counter.increment(1);
        self.health_counters.standstill_triggers.fetch_add(1, Ordering::Relaxed);
        self.standstill_certs_rebroadcast_counter.increment(cert_count as u64);
        self.standstill_votes_rebroadcast_counter.increment(votes_to_rebroadcast.len() as u64);

        log::warn!(
            "SimplexReceiver {}: Standstill detected, re-broadcasting {} certs + {} votes \
            (range [{}, {}))",
            self.session_id.to_hex_string(),
            cert_count,
            votes_to_rebroadcast.len(),
            begin,
            end
        );

        // Re-broadcast each vote in range (already signed, no loopback)
        for vote in votes_to_rebroadcast {
            self.send_vote_impl(vote, true /* is_rebroadcast */);
        }

        // Reschedule standstill timer (reschedule after re-broadcast)
        self.reschedule_standstill();
    }

    /// Set the range of slots for standstill vote re-broadcast
    ///
    /// Sets `[begin, end)` range and removes votes outside this range.
    /// Reference: C++ pool.cpp tracked_slots_interval() = [first_non_finalized, current_window_end)
    fn set_standstill_slots_impl(&mut self, begin: u32, end: u32) {
        log::trace!(
            "SimplexReceiver {}: set_standstill_slots [{}, {})",
            self.session_id.to_hex_string(),
            begin,
            end
        );

        self.standstill_slot_begin = begin;
        self.standstill_slot_end = end;

        // Remove votes outside the range
        let old_count = self.our_votes.len();
        self.our_votes.retain(|(slot, _)| *slot >= begin && *slot < end);
        if self.our_votes.len() < old_count {
            log::trace!(
                "SimplexReceiver {}: removed {} votes outside standstill range",
                self.session_id.to_hex_string(),
                old_count - self.our_votes.len()
            );
        }
    }

    /// Send certificate to all validators
    ///
    /// Broadcasts a TL certificate to all validators using the same channel as votes.
    ///
    /// # Arguments
    /// * `certificate` - TL certificate to broadcast
    fn send_certificate_impl(&mut self, certificate: Certificate) {
        check_execution_time!(20_000);
        instrument!();

        // Serialize TL boxed object (Certificate is already boxed enum type)
        let serialized = consensus_common::serialize_tl_boxed_object!(&certificate);
        let payload = ConsensusCommonFactory::create_block_payload(serialized.into());

        log::trace!(
            "SimplexReceiver {}: sending certificate to {} validators, size={}",
            self.session_id.to_hex_string(),
            self.send_order.len(),
            payload.data().len()
        );

        // Update metrics
        let msg_size = payload.data().len() as u64;
        let recipient_count = self.send_order.iter().filter(|&&idx| idx != self.local_idx).count();
        self.out_messages_bytes.increment(msg_size * recipient_count as u64);
        self.out_bytes.increment(msg_size * recipient_count as u64);
        self.out_messages_count.increment(recipient_count as u64);

        // Send to all validators except self
        for &target_idx in &self.send_order {
            if target_idx == self.local_idx {
                continue;
            }

            if let Some(stats) = self.sources.get_mut(target_idx as usize) {
                stats.out_messages += 1;
                stats.last_send_time = Some(SystemTime::now());

                self.overlay.send_message(
                    &stats.adnl_id,
                    &self.local_adnl_id,
                    &payload,
                    false, // is_retransmission not used in simplex
                );
            }
        }
    }

    /// Cache certificate bytes for standstill replay
    fn cache_standstill_certificate_impl(
        &mut self,
        slot: u32,
        kind: StandstillCertificateType,
        cert_bytes: Vec<u8>,
    ) {
        let bundle = self.standstill_certs.entry(slot).or_default();
        // Keep standstill end large enough to include cached certs for new slots.
        self.standstill_slot_end = self.standstill_slot_end.max(slot.saturating_add(1));

        match kind {
            StandstillCertificateType::Notar => {
                if bundle.notar.is_none() {
                    log::trace!(
                        "SimplexReceiver {}: caching notar cert for slot {} ({}B)",
                        self.session_id.to_hex_string(),
                        slot,
                        cert_bytes.len()
                    );
                    bundle.notar = Some(cert_bytes);
                }
            }
            StandstillCertificateType::Skip => {
                if bundle.skip.is_none() {
                    log::trace!(
                        "SimplexReceiver {}: caching skip cert for slot {} ({}B)",
                        self.session_id.to_hex_string(),
                        slot,
                        cert_bytes.len()
                    );
                    bundle.skip = Some(cert_bytes);
                }
            }
            StandstillCertificateType::Final => {
                if bundle.final_.is_none() {
                    log::trace!(
                        "SimplexReceiver {}: caching final cert for slot {} ({}B)",
                        self.session_id.to_hex_string(),
                        slot,
                        cert_bytes.len()
                    );
                    bundle.final_ = Some(cert_bytes);
                }
            }
        }
    }

    /// Cache last finalization certificate for standstill replay
    fn cache_last_final_certificate_impl(&mut self, slot: u32, cert_bytes: Vec<u8>) {
        log::trace!(
            "SimplexReceiver {}: caching last final cert for slot {} ({}B)",
            self.session_id.to_hex_string(),
            slot,
            cert_bytes.len()
        );
        self.last_final_cert = Some((slot, cert_bytes));
    }

    /// Re-broadcast cached certificates during standstill
    ///
    /// Sends all cached certificates to all validators:
    /// 1. Last finalization certificate (always, even if outside tracked range)
    /// 2. All cached certificates in tracked range [begin, end)
    ///
    /// Reference: C++ pool.cpp alarm() iterates certs.serialize_to(messages)
    ///
    /// Returns the number of certificates sent.
    fn rebroadcast_standstill_certificates(&mut self, begin: u32, end: u32) -> u32 {
        // Collect all certificate bytes to send (avoid borrow conflicts)
        let mut cert_bytes_list: Vec<Vec<u8>> = Vec::new();

        // 1. Last final certificate (always, even if outside tracked range)
        // Reference: C++ pool.cpp alarm() always includes last_final_cert_ first
        if let Some((slot, bytes)) = &self.last_final_cert {
            log::trace!(
                "SimplexReceiver {}: standstill re-broadcast last_final_cert slot={} ({}B)",
                self.session_id.to_hex_string(),
                slot,
                bytes.len()
            );
            cert_bytes_list.push(bytes.clone());
        }

        // 2. All cached certificates in tracked range [begin, end)
        //
        // C++ iterates slots linearly in `[begin,end)` and serializes per-slot cert bundle.
        // Here we iterate only cached bundles to avoid range work when the range is
        // sparsely populated (receiver initializes with a wide range before FSM sync).
        let mut slots: Vec<u32> = self
            .standstill_certs
            .keys()
            .copied()
            .filter(|slot| *slot >= begin && *slot < end)
            .collect();
        slots.sort_unstable();
        for slot in slots {
            if let Some(bundle) = self.standstill_certs.get(&slot) {
                if let Some(bytes) = &bundle.notar {
                    cert_bytes_list.push(bytes.clone());
                }
                if let Some(bytes) = &bundle.skip {
                    cert_bytes_list.push(bytes.clone());
                }
                if let Some(bytes) = &bundle.final_ {
                    cert_bytes_list.push(bytes.clone());
                }
            }
        }

        let cert_count = cert_bytes_list.len() as u32;
        if cert_count == 0 {
            return 0;
        }

        // Calculate total bytes and recipient count for metrics
        let total_bytes: u64 = cert_bytes_list.iter().map(|b| b.len() as u64).sum();
        let recipient_count =
            self.send_order.iter().filter(|&&idx| idx != self.local_idx).count() as u64;

        // Update metrics (each cert sent to each recipient)
        self.out_messages_bytes.increment(total_bytes * recipient_count);
        self.out_bytes.increment(total_bytes * recipient_count);
        self.out_messages_count.increment(cert_count as u64 * recipient_count);

        // Send each certificate to all validators
        for bytes in cert_bytes_list {
            let payload = ConsensusCommonFactory::create_block_payload(bytes.into());

            for &target_idx in &self.send_order {
                if target_idx == self.local_idx {
                    continue;
                }

                if let Some(stats) = self.sources.get_mut(target_idx as usize) {
                    stats.out_messages += 1;
                    stats.last_send_time = Some(SystemTime::now());

                    self.overlay.send_message(
                        &stats.adnl_id,
                        &self.local_adnl_id,
                        &payload,
                        false, // is_retransmission=false for simplex
                    );
                }
            }
        }

        cert_count
    }

    /*
        Delayed Actions
    */

    /// Post a delayed action to be executed at a future time
    ///
    /// The handler will be called when `expiration_time` is reached during
    /// the main processing loop (via `process_delayed_actions()`).
    ///
    /// # Arguments
    /// * `expiration_time` - When to execute the action
    /// * `handler` - Closure to execute (takes `&mut ReceiverImpl`)
    fn post_delayed_action<F>(&mut self, expiration_time: SystemTime, handler: F)
    where
        F: FnOnce(&mut ReceiverImpl) + Send + 'static,
    {
        let delayed_action = ReceiverDelayedAction { expiration_time, handler: Box::new(handler) };

        self.delayed_actions.push(delayed_action);
    }

    /// Process all expired delayed actions
    ///
    /// Iterates through delayed actions and executes those whose expiration time
    /// has been reached. Called from the main processing loop.
    fn process_delayed_actions(&mut self) {
        let now = SystemTime::now();
        let mut i = 0;

        while i < self.delayed_actions.len() {
            if self.delayed_actions[i].expiration_time <= now {
                // Remove and execute expired action
                let delayed_action = self.delayed_actions.swap_remove(i);
                (delayed_action.handler)(self);
                // Don't increment i - swap_remove moved last element to position i
            } else {
                // Not expired yet, move to next
                i += 1;
            }
        }
    }

    /// Compute the next timeout for the processing loop
    ///
    /// Returns the minimum of the default processing period and the time
    /// until the next delayed action should fire.
    fn compute_next_timeout(&self) -> Duration {
        let default_timeout = Duration::from_millis(RECEIVER_PROCESSING_PERIOD_MS);
        let now = SystemTime::now();

        // Find the earliest delayed action
        let earliest_action = self
            .delayed_actions
            .iter()
            .filter_map(|a| a.expiration_time.duration_since(now).ok())
            .min();

        match earliest_action {
            Some(until_action) => default_timeout.min(until_action),
            None => default_timeout,
        }
    }

    /// Stop the receiver
    fn stop(&mut self) {
        log::info!("Stopping ReceiverImpl for session {}", self.session_id.to_hex_string());

        // Note: Overlay cleanup is handled by overlay manager
    }
}

impl Drop for ReceiverImpl {
    fn drop(&mut self) {
        log::info!("Dropping ReceiverImpl for session {}", self.session_id.to_hex_string());

        self.stop();

        log::info!("Dropped ReceiverImpl for session {}", self.session_id.to_hex_string());
    }
}

/*
    OverlayListenerImpl - implementation of CatchainOverlayListener
*/

struct OverlayListenerImpl {
    session_id: SessionId,
    task_queues: Arc<ReceiverTaskQueues>,
    in_messages_bytes: metrics::Counter,
    in_broadcasts_bytes: metrics::Counter,
    in_queries_bytes: metrics::Counter,
    in_bytes: metrics::Counter,
    in_messages_count: metrics::Counter,
    in_broadcasts_count: metrics::Counter,
    in_queries_count: metrics::Counter,
}

impl ConsensusOverlayLogReplayListener for OverlayListenerImpl {
    fn on_time_changed(&self, _timestamp: SystemTime) {
        // TODO: Implement time replay support if needed
    }
}

impl ConsensusOverlayListener for OverlayListenerImpl {
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr) {
        instrument!();

        self.in_messages_count.increment(1);
        self.in_messages_bytes.increment(data.data().len() as u64);
        self.in_bytes.increment(data.data().len() as u64);

        let adnl_id = adnl_id.clone();
        let data = data.clone();

        if log::log_enabled!(log::Level::Trace) {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::new(0, 0))
                .as_millis();
            log::trace!(
                "SimplexReceiver {}: on_message, size={}, source={}, timestamp={}",
                self.session_id.to_hex_string(),
                data.data().len(),
                key_to_base64(&adnl_id),
                elapsed
            );
        }

        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.receive_message_from_overlay(adnl_id, data);
        }));
    }

    fn on_broadcast(&self, source_key_hash: PublicKeyHash, data: &BlockPayloadPtr) {
        instrument!();

        self.in_broadcasts_count.increment(1);
        self.in_broadcasts_bytes.increment(data.data().len() as u64);
        self.in_bytes.increment(data.data().len() as u64);

        let source_key_hash = source_key_hash.clone();
        let data = data.clone();

        if log::log_enabled!(log::Level::Trace) {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::new(0, 0))
                .as_millis();
            log::trace!(
                "SimplexReceiver {}: on_broadcast, size={}, source={}, timestamp={}",
                self.session_id.to_hex_string(),
                data.data().len(),
                key_to_base64(&source_key_hash),
                elapsed
            );
        }

        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.receive_broadcast_from_overlay(source_key_hash, data);
        }));
    }

    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        instrument!();

        self.in_queries_count.increment(1);
        self.in_queries_bytes.increment(data.data().len() as u64);
        self.in_bytes.increment(data.data().len() as u64);

        let adnl_id = adnl_id.clone();
        let data = data.clone();

        if log::log_enabled!(log::Level::Trace) {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::new(0, 0))
                .as_millis();
            log::trace!(
                "SimplexReceiver {}: on_query, size={}, source={}, timestamp={}",
                self.session_id.to_hex_string(),
                data.data().len(),
                key_to_base64(&adnl_id),
                elapsed
            );
        }

        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.handle_query(adnl_id, data, response_callback);
        }));
    }
}

impl Drop for OverlayListenerImpl {
    fn drop(&mut self) {
        log::info!("Dropped OverlayListenerImpl for session {}", self.session_id.to_hex_string());
    }
}

/*
    ReceiverWrapper - crate-internal interface, thread management
*/

pub(crate) struct ReceiverWrapper {
    session_id: SessionId,
    task_queues: Arc<ReceiverTaskQueues>,
    receiver_threads: ReceiverThreads,
    _metrics_receiver: MetricsHandle,
    out_messages_bytes: metrics::Counter,
    out_broadcasts_bytes: metrics::Counter,
    out_bytes: metrics::Counter,
    local_adnl_id: PublicKeyHash,
    _local_key: PrivateKey,
    overlay: ConsensusOverlayPtr,
    overlay_short_id: PublicKeyHash,
    overlay_manager: ConsensusOverlayManagerPtr,
    _overlay_listener: Arc<dyn ConsensusOverlayListener + Send + Sync>,
}

impl Receiver for ReceiverWrapper {
    fn send_vote(&self, vote: TlVote) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.send_vote_impl(vote, false /* is_rebroadcast */);
        }));
    }

    fn cache_our_vote_for_standstill(&self, vote: TlVote) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cache_our_vote_for_standstill_impl(vote);
        }));
    }

    fn send_block_broadcast(&self, slot: u32, candidate_hash: UInt256, candidate: CandidateData) {
        let candidate = candidate.clone();
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.send_block_broadcast_impl(slot, candidate_hash, candidate);
        }));
    }

    fn cleanup(&self, up_to_slot: u32) {
        let up_to = SlotIndex::new(up_to_slot);
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cleanup(up_to);
        }));
    }

    fn cache_notarization_cert(&self, slot: u32, block_hash: UInt256, notar_cert_data: Vec<u8>) {
        let slot_idx = SlotIndex::new(slot);
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cache_notarization_cert(slot_idx, block_hash, notar_cert_data);
        }));
    }

    fn request_candidate(&self, slot: u32, block_hash: UInt256) {
        let slot_idx = SlotIndex::new(slot);
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.request_candidate_impl(slot_idx, block_hash);
        }));
    }

    fn reschedule_standstill(&self) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.reschedule_standstill();
        }));
    }

    fn set_standstill_slots(&self, begin: u32, end: u32) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.set_standstill_slots_impl(begin, end);
        }));
    }

    fn cache_candidate_bytes(&self, slot: u32, block_hash: UInt256, candidate_data: Vec<u8>) {
        let slot_idx = SlotIndex::new(slot);
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cache_candidate_bytes(slot_idx, block_hash, candidate_data);
        }));
    }

    fn send_certificate(&self, certificate: Certificate) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.send_certificate_impl(certificate);
        }));
    }

    fn cache_standstill_certificate(
        &self,
        slot: u32,
        kind: StandstillCertificateType,
        cert_bytes: Vec<u8>,
    ) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cache_standstill_certificate_impl(slot, kind, cert_bytes);
        }));
    }

    fn cache_last_final_certificate(&self, slot: u32, cert_bytes: Vec<u8>) {
        self.task_queues.post_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            receiver.cache_last_final_certificate_impl(slot, cert_bytes);
        }));
    }

    fn stop(&self) {
        self.receiver_threads.stop_threads();
        self.overlay_manager.stop_overlay(&self.overlay_short_id, &self.overlay);
    }
}

impl Drop for ReceiverWrapper {
    fn drop(&mut self) {
        log::info!("Dropping ReceiverWrapper for session {}", self.session_id.to_hex_string());

        self.receiver_threads.stop_threads();
        self.receiver_threads.remove_all_threads();

        log::trace!("Stopping overlay for session {}", self.session_id.to_hex_string());
        self.overlay_manager.stop_overlay(&self.overlay_short_id, &self.overlay);
        log::trace!("Stopped overlay for session {}", self.session_id.to_hex_string());

        log::info!("Dropped ReceiverWrapper for session {}", self.session_id.to_hex_string());
    }
}

impl ReceiverWrapper {
    /// Create new receiver
    ///
    /// The notar certificate cache starts empty and is populated via
    /// `cache_notarization_cert()` calls from the startup recovery processor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(
        session_id: SessionId,
        shard: &ShardIdent,
        max_candidate_size: usize,
        proto_version: u32,
        ids: &[SessionNode],
        local_key: &PrivateKey,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: ReceiverListenerPtr,
        standstill_timeout: Duration,
        panicked_flag: Arc<AtomicBool>,
        use_quic: bool,
        health_counters: Arc<ReceiverHealthCounters>,
    ) -> Result<ReceiverPtr> {
        log::info!(
            "Creating SimplexReceiver for session {} (shard={}) with {} nodes",
            session_id.to_hex_string(),
            shard,
            ids.len()
        );

        // Create metrics receiver (owned by this receiver instance)
        let metrics_receiver = MetricsHandle::new(Some(RECEIVER_METRICS_IDLE_TIMEOUT));

        // Compute overlay ID (must match C++ implementation)
        let (overlay_id, overlay_short_id) = Self::compute_overlay_id(&session_id, ids)?;

        log::debug!(
            "SimplexReceiver {}: overlay_id={}, overlay_short_id={}",
            session_id.to_hex_string(),
            overlay_id.to_hex_string(),
            overlay_short_id
        );

        // Create task queues
        let task_queues = Arc::new(ReceiverTaskQueues::new(&metrics_receiver));

        // Create metrics counters
        let in_messages_bytes =
            metrics_receiver.sink().register_counter(&"simplex_receiver_in_messages_bytes".into());
        let out_messages_bytes =
            metrics_receiver.sink().register_counter(&"simplex_receiver_out_messages_bytes".into());
        let in_broadcasts_bytes = metrics_receiver
            .sink()
            .register_counter(&"simplex_receiver_in_broadcasts_bytes".into());
        let out_broadcasts_bytes = metrics_receiver
            .sink()
            .register_counter(&"simplex_receiver_out_broadcasts_bytes".into());
        let in_bytes =
            metrics_receiver.sink().register_counter(&"simplex_receiver_in_bytes".into());
        let out_bytes =
            metrics_receiver.sink().register_counter(&"simplex_receiver_out_bytes".into());
        let in_queries_bytes =
            metrics_receiver.sink().register_counter(&"simplex_receiver_in_queries_bytes".into());

        // Create metrics counters (counts)
        let in_messages_count =
            metrics_receiver.sink().register_counter(&"simplex_receiver_in_messages_count".into());
        let out_messages_count =
            metrics_receiver.sink().register_counter(&"simplex_receiver_out_messages_count".into());
        let in_broadcasts_count = metrics_receiver
            .sink()
            .register_counter(&"simplex_receiver_in_broadcasts_count".into());
        let out_broadcasts_count = metrics_receiver
            .sink()
            .register_counter(&"simplex_receiver_out_broadcasts_count".into());
        let in_queries_count =
            metrics_receiver.sink().register_counter(&"simplex_receiver_in_queries_count".into());

        // Create overlay listener
        let overlay_listener = Arc::new(OverlayListenerImpl {
            session_id: session_id.clone(),
            task_queues: task_queues.clone(),
            in_messages_bytes: in_messages_bytes.clone(),
            in_broadcasts_bytes: in_broadcasts_bytes.clone(),
            in_queries_bytes,
            in_bytes: in_bytes.clone(),
            in_messages_count,
            in_broadcasts_count,
            in_queries_count,
        });

        let overlay_data_listener: Arc<dyn ConsensusOverlayListener + Send + Sync> =
            overlay_listener.clone();
        let overlay_replay_listener: Arc<dyn ConsensusOverlayLogReplayListener + Send + Sync> =
            overlay_listener;

        // Convert SessionNode to ConsensusNode for overlay manager
        let consensus_nodes: Vec<ConsensusNode> = ids
            .iter()
            .map(|n| ConsensusNode { adnl_id: n.adnl_id.clone(), public_key: n.public_key.clone() })
            .collect();

        // Start overlay
        let transport_type = if use_quic {
            consensus_common::OverlayTransportType::SimplexQuic
        } else {
            consensus_common::OverlayTransportType::Simplex
        };
        let overlay = overlay_manager.start_overlay(
            local_key,
            &overlay_short_id,
            &consensus_nodes,
            Arc::downgrade(&overlay_data_listener),
            Arc::downgrade(&overlay_replay_listener),
            transport_type,
        )?;

        // Find local index
        let local_key_hash = local_key.id();
        let local_idx =
            ids.iter()
                .position(|n| n.public_key.id() == local_key_hash)
                .ok_or_else(|| error!("Local key not found in node list"))? as u32;

        let local_adnl_id = ids[local_idx as usize].adnl_id.clone();

        // Build sources and mappings
        let mut sources = Vec::with_capacity(ids.len());
        let mut adnl_to_idx = HashMap::new();
        let mut pubkey_to_idx: HashMap<PublicKeyHash, u32> = HashMap::new();
        let mut send_order = Vec::with_capacity(ids.len());

        for (idx, node) in ids.iter().enumerate() {
            let idx = idx as u32;
            let pubkey_hash = node.public_key.id();

            sources.push(SourceStats::new(
                idx,
                node.adnl_id.clone(),
                node.public_key.clone(),
                node.weight,
            ));
            adnl_to_idx.insert(node.adnl_id.clone(), idx);
            pubkey_to_idx.insert(pubkey_hash.clone(), idx);
            send_order.push(idx);
        }

        // Create thread management
        let mut receiver_threads = ReceiverThreads::new(session_id.clone(), panicked_flag);

        // Clone overlay_short_id for wrapper (before it's moved into the thread)
        let overlay_short_id_for_wrapper = overlay_short_id.clone();

        // Clone data for processing thread
        let session_id_clone = session_id.clone();
        let shard_clone = shard.clone();
        let overlay_clone = overlay.clone();
        let task_queues_clone = task_queues.clone();
        let listener_clone = listener.clone();
        let local_key_clone = local_key.clone();
        let metrics_receiver_clone = metrics_receiver.clone();
        let in_messages_bytes_clone = in_messages_bytes.clone();
        let out_messages_bytes_clone = out_messages_bytes.clone();
        let in_broadcasts_bytes_clone = in_broadcasts_bytes.clone();
        let out_broadcasts_bytes_clone = out_broadcasts_bytes.clone();
        let in_bytes_clone = in_bytes.clone();
        let out_bytes_clone = out_bytes.clone();
        let out_messages_count_clone = out_messages_count.clone();
        let out_broadcasts_count_clone = out_broadcasts_count.clone();
        let health_counters_clone = health_counters.clone();

        // Start processing thread
        receiver_threads.start_thread(
            RECEIVER_THREAD_NAME.to_string(),
            Box::new(move |stop_flag: Arc<AtomicBool>, activity_node: ActivityNodePtr| {
                // Create resolver cache (empty - populated via cache_notarization_cert)
                let resolver_cache = CandidateResolverCache::new();

                // Create ReceiverImpl inside the processing thread
                let mut receiver_impl = ReceiverImpl {
                    session_id: session_id_clone.clone(),
                    overlay_id,
                    overlay_short_id,
                    overlay: overlay_clone,
                    local_key: local_key_clone,
                    local_adnl_id: local_adnl_id.clone(),
                    local_idx,
                    listener: listener_clone,
                    sources,
                    adnl_to_idx,
                    pubkey_to_idx,
                    send_order,
                    last_shuffle_time: SystemTime::now(),
                    dedup_votes: HashMap::new(),
                    shard: shard_clone,
                    max_candidate_size,
                    proto_version,
                    in_messages_bytes: in_messages_bytes_clone,
                    out_messages_bytes: out_messages_bytes_clone,
                    in_broadcasts_bytes: in_broadcasts_bytes_clone,
                    out_broadcasts_bytes: out_broadcasts_bytes_clone,
                    in_bytes: in_bytes_clone,
                    out_bytes: out_bytes_clone,
                    out_messages_count: out_messages_count_clone,
                    out_broadcasts_count: out_broadcasts_count_clone,
                    _activity_node: activity_node.clone(),
                    standstill_timeout,
                    standstill_alarm: Some(SystemTime::now() + standstill_timeout), // Initial scheduling
                    standstill_slot_begin: STANDSTILL_INITIAL_SLOT_BEGIN,
                    standstill_slot_end: STANDSTILL_INITIAL_SLOT_END,
                    our_votes: Vec::new(),
                    resolver_cache,
                    delayed_actions: Vec::new(),
                    pending_requests: HashMap::new(),
                    task_queues: task_queues_clone.clone(),
                    standstill_certs: HashMap::new(),
                    last_final_cert: None,
                    first_active_slot: 0,
                    candidate_requests_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_candidate_requests".into()),
                    candidate_request_retries_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_candidate_request_retries".into()),
                    candidate_request_timeouts_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_candidate_request_timeouts".into()),
                    candidate_request_giveups_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_candidate_request_giveups".into()),
                    standstill_triggers_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_standstill_triggers".into()),
                    standstill_votes_rebroadcast_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_standstill_votes_rebroadcast".into()),
                    standstill_certs_rebroadcast_counter: metrics_receiver_clone
                        .sink()
                        .register_counter(&"simplex_standstill_certs_rebroadcast".into()),
                    health_counters: health_counters_clone,
                };

                // Create metrics dumper
                let mut metrics_dumper = MetricsDumper::new();
                // Byte metrics
                metrics_dumper.add_derivative_metric("simplex_receiver_in_messages_bytes");
                metrics_dumper.add_derivative_metric("simplex_receiver_out_messages_bytes");
                metrics_dumper.add_derivative_metric("simplex_receiver_in_broadcasts_bytes");
                metrics_dumper.add_derivative_metric("simplex_receiver_out_broadcasts_bytes");
                metrics_dumper.add_derivative_metric("simplex_receiver_in_bytes");
                metrics_dumper.add_derivative_metric("simplex_receiver_out_bytes");
                // Count metrics
                metrics_dumper.add_derivative_metric("simplex_receiver_in_messages_count");
                metrics_dumper.add_derivative_metric("simplex_receiver_out_messages_count");
                metrics_dumper.add_derivative_metric("simplex_receiver_in_broadcasts_count");
                metrics_dumper.add_derivative_metric("simplex_receiver_out_broadcasts_count");
                metrics_dumper.add_derivative_metric("simplex_receiver_in_queries_count");
                // Queue metrics
                metrics_dumper.add_derivative_metric("simplex_receiver_main_queue.posts");
                metrics_dumper.add_derivative_metric("simplex_receiver_main_queue.pulls");
                metrics_dumper.add_derivative_metric("simplex_candidate_requests");
                metrics_dumper.add_derivative_metric("simplex_candidate_request_retries");
                metrics_dumper.add_derivative_metric("simplex_candidate_request_timeouts");
                metrics_dumper.add_derivative_metric("simplex_candidate_request_giveups");
                metrics_dumper.add_derivative_metric("simplex_standstill_triggers");
                metrics_dumper.add_derivative_metric("simplex_standstill_votes_rebroadcast");
                metrics_dumper.add_derivative_metric("simplex_standstill_certs_rebroadcast");

                // Processing loop
                let mut last_warn_dump_time = SystemTime::now();
                let mut next_metrics_dump_time = SystemTime::now();
                let mut next_active_weight_time = SystemTime::now();
                let loop_iterations_counter = metrics_receiver_clone
                    .sink()
                    .register_counter(&"simplex_receiver_main_loop_iterations".into());

                loop {
                    activity_node.tick();
                    loop_iterations_counter.increment(1);

                    // Check stop flag
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    // Pull task from queue with timeout (computed based on pending delayed actions)
                    let timeout = receiver_impl.compute_next_timeout();
                    //todo: check timeouts
                    match task_queues_clone.task_receiver.recv_timeout(timeout) {
                        Ok(task_desc) => {
                            check_execution_time!(50_000);

                            // Check latency and queue size
                            let processing_latency = get_elapsed_time(&task_desc.creation_time);
                            let queue_size = task_queues_clone.task_receiver.len();

                            // Trace log queue size for debugging
                            log::trace!(
                                "SimplexReceiver {}: processing task, queue_size={}, latency={:.3}ms",
                                &session_id_clone.to_hex_string()[..8],
                                queue_size,
                                processing_latency.as_secs_f64() * 1000.0
                            );

                            if processing_latency > RECEIVER_WARN_PROCESSING_LATENCY {
                                if let Ok(warn_elapsed) = last_warn_dump_time.elapsed() {
                                    if warn_elapsed > RECEIVER_LATENCY_WARN_DUMP_PERIOD {
                                        log::warn!(
                                            "SimplexReceiver {} task queue latency is {:.3}s \
                                            (expected max latency is {:.3}s), queue_size={}",
                                            session_id_clone.to_hex_string(),
                                            processing_latency.as_secs_f64(),
                                            RECEIVER_WARN_PROCESSING_LATENCY.as_secs_f64(),
                                            queue_size
                                        );
                                        last_warn_dump_time = SystemTime::now();
                                    }
                                }
                            }

                            task_queues_clone.pull_counter.increment(1);

                            // Execute task
                            {
                                check_execution_time!(20_000);
                                (task_desc.task)(&mut receiver_impl);
                            }
                        }
                        Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                            // Timeout - continue loop
                        }
                        Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                            log::warn!(
                                "SimplexReceiver {} task queue disconnected",
                                session_id_clone.to_hex_string()
                            );
                            break;
                        }
                    }

                    check_execution_time!(100_000);

                    // Process expired delayed actions
                    {
                        check_execution_time!(10_000);
                        receiver_impl.process_delayed_actions();
                    }

                    // Shuffle send order periodically
                    if let Ok(elapsed) = receiver_impl.last_shuffle_time.elapsed() {
                        if elapsed > SHUFFLE_SEND_ORDER_PERIOD {
                            check_execution_time!(1_000);
                            receiver_impl.shuffle_send_order();
                        }
                    }

                    // Recompute and report activity periodically (separate from metrics dump)
                    if next_active_weight_time.elapsed().is_ok() {
                        check_execution_time!(10_000);
                        let active_weight =
                            receiver_impl.calculate_active_weight(ACTIVITY_THRESHOLD);
                        let last_activity = receiver_impl.get_last_activity();
                        if let Some(listener) = receiver_impl.listener.upgrade() {
                            listener.on_activity(active_weight, last_activity);
                        }
                        next_active_weight_time =
                            SystemTime::now() + ACTIVE_WEIGHT_RECOMPUTE_PERIOD;
                    }

                    // Dump metrics periodically
                    if next_metrics_dump_time.elapsed().is_ok() {
                        check_execution_time!(10_000);
                        metrics_dumper.update(&metrics_receiver_clone);

                        if log::log_enabled!(log::Level::Debug) {
                            let session_id_str = session_id_clone.to_hex_string();
                            log::debug!("SimplexReceiver {} metrics:", &session_id_str);

                            metrics_dumper.dump(|string| {
                                log::debug!("{}{}", session_id_str, string);
                            });
                        }

                        receiver_impl.debug_dump();

                        next_metrics_dump_time = SystemTime::now()
                            + Duration::from_millis(RECEIVER_METRICS_DUMP_PERIOD_MS);
                    }

                    // Check for standstill and re-broadcast if needed
                    {
                        check_execution_time!(10_000);
                        receiver_impl.check_standstill();
                    }
                }
            }),
        )?;

        // Create wrapper
        let wrapper = ReceiverWrapper {
            session_id,
            task_queues,
            receiver_threads,
            _metrics_receiver: metrics_receiver,
            out_messages_bytes,
            out_broadcasts_bytes,
            out_bytes,
            local_adnl_id: ids[local_idx as usize].adnl_id.clone(),
            _local_key: local_key.clone(),
            overlay,
            overlay_short_id: overlay_short_id_for_wrapper,
            overlay_manager,
            _overlay_listener: overlay_data_listener,
        };

        log::info!("Created SimplexReceiver for session {}", wrapper.session_id.to_hex_string());

        Ok(Arc::new(wrapper))
    }

    /// Compute overlay ID matching C++ consensus.overlayId
    ///
    /// CRITICAL: Must match C++ implementation exactly.
    /// See: docs/ton-node-cpp-alpenglow/validator/consensus/private-overlay.cpp
    fn compute_overlay_id(
        session_id: &SessionId,
        nodes: &[SessionNode],
    ) -> Result<(SessionId, PublicKeyHash)> {
        // C++ reference (`validator/consensus/private-overlay.cpp`):
        // - overlay_seed = tl::overlayId(session_id, nodes_short_ids_in_validator_set_order)
        // - overlay_full_id = OverlayIdFull{ serialize_tl_object(overlay_seed, true) }  (name bytes)
        // - overlay_short_id = overlay_full_id.compute_short_id()
        //
        // IMPORTANT: Do NOT sort nodes here; order must match validator set (SessionDescription).
        let nodes_int256: Vec<UInt256> =
            nodes.iter().map(|n| UInt256::from(*n.public_key.id().data())).collect();

        // Create overlay seed TL object (consensus.overlayId)
        let overlay_seed =
            OverlayId { session_id: session_id.clone(), nodes: nodes_int256.into_iter().collect() };

        // Serialize overlay seed (this is OverlayIdFull "name" in C++)
        let serialized = consensus_common::serialize_tl_boxed_object!(&overlay_seed.into_boxed());

        // For diagnostics we also keep a compact 32-byte hash of the seed.
        let overlay_id = UInt256::calc_file_hash(&serialized);

        // Compute overlay short id exactly like C++ OverlayIdFull::compute_short_id():
        // pubkey = pub.overlay(name = serialized_overlay_seed_bytes)
        // short_id = sha256(serialize(pubkey))
        let overlay_pubkey = Overlay { name: serialized }.into_boxed();
        let overlay_short_id = KeyId::from_data(adnl::common::hash_boxed(&overlay_pubkey)?);

        Ok((overlay_id, overlay_short_id))
    }
}

/*
    ============================================================================
    Tests
    ============================================================================

    Tests are in a separate file but included directly to access private internals.
*/

#[cfg(test)]
#[path = "tests/test_candidate_resolver.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/test_slot_bounds.rs"]
mod slot_bounds_tests;
