/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! In-process consensus emulator (TN-1054).
//!
//! Drives the [`SessionListener`] callbacks (`on_generate_slot`,
//! `on_candidate`, `on_candidate_observed`, `on_block_finalized`) on a fixed
//! two-thread runtime, with controllable per-callback delays and skip
//! probabilities. Holds the full set of validator keypairs (each
//! [`SessionNode::public_key`] is treated as a full
//! [`KeyOption`](ton_block::KeyOption) capable of signing) so finalized
//! blocks carry real Ed25519 signatures from any configured subset of
//! validators.
//!
//! Public surface lives in [`crate`]: see [`Emulator`], [`EmulatorOptions`],
//! [`EmulatorParams`], [`EmulatorDelaySpec`], [`EmulatorLeaderRotation`],
//! [`EmulatorSignerSubset`]; construct via
//! [`ConsensusCommonFactory::create_consensus_emulator`].
//!
//! # Threading model
//!
//! Two threads communicate exclusively via two crossbeam channels — there
//! are no `Mutex`es or `Condvar`s on the hot path:
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────────┐
//! │ ConsensusEmulatorImpl                                                 │
//! │                                                                       │
//! │  ┌─────────────────────────────────┐   ┌────────────────────────────┐ │
//! │  │ EMUMAIN:{session_id}            │   │ EMUCB:{session_id}         │ │
//! │  │ (Main thread, single-threaded   │   │ (Callbacks thread)         │ │
//! │  │  state)                         │   │                            │ │
//! │  │                                 │   │  pull_closure(timeout)     │ │
//! │  │  pull_closure(timeout):         │   │  ↓                         │ │
//! │  │    main_task_queue (FIFO)       │   │  invoke SessionListener:   │ │
//! │  │  ↓ skip + delay roll            │   │  ├─ on_generate_slot       │ │
//! │  │  push to local delayed_heap     │   │  ├─ on_candidate           │ │
//! │  │  ↓ pop due tasks                │   │  ├─ on_candidate_observed  │ │
//! │  │  fn(&mut EmulatorCore)          │   │  └─ on_block_finalized     │ │
//! │  │  ↓                              │   │                            │ │
//! │  │  post_closure callbacks_queue   │───→  callbacks_task_queue (FIFO)│ │
//! │  └─────────────────────────────────┘   └────────────────────────────┘ │
//! │            ▲                                                          │
//! │            │ post_closure(spec, work) ← from any thread               │
//! │            │ (e.g. listener-reentry callbacks)                        │
//! └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key components
//!
//! - [`TaskQueue`] / [`TaskQueueImpl`]: simplex-style FIFO crossbeam-channel
//!   queue with `post_closure` / `pull_closure` / `flush`. Two instances:
//!   one for main-thread tasks (closures over `&mut EmulatorCore`), one for
//!   listener invocations (`FnOnce()`).
//! - [`EmulatorCore`]: per-emulator state owned by the main thread; closures
//!   pulled from the main queue receive `&mut EmulatorCore`. The struct is
//!   **never** shared across threads, so it requires no synchronization.
//! - [`ConsensusEmulatorImpl`]: public Emulator implementation; owns both
//!   threads and both task queues, exposes [`Session`] and [`Emulator`].
//!
//! # Threading invariant
//!
//! Listener-reentry callbacks (the closures handed to `on_generate_slot` and
//! `on_candidate`) MUST NOT touch [`EmulatorCore`] directly; they post a
//! zero-delay closure onto the main task queue so all state mutation happens
//! on the main thread. The listener may invoke its callback from any thread
//! synchronously without deadlocking.

use crate::{
    AsyncRequest, AsyncRequestPtr, BlockCandidatePriority, BlockHash, BlockPayloadPtr,
    BlockSourceInfo, CandidateObservedFlags, CollationParentHint, ConsensusCommonFactory, Emulator,
    EmulatorDelaySpec, EmulatorLeaderRotation, EmulatorOptions, EmulatorParams, EmulatorPtr,
    EmulatorSignerSubset, EnsureCandidateAvailabilityOptions, PrivateKey, PublicKeyHash, Result,
    Session, SessionId, SessionListenerPtr, SessionNode, ValidatorBlockCandidateCallback,
    ValidatorBlockCandidateDecisionCallback, ValidatorBlockCandidatePtr, ValidatorWeight,
};
use crossbeam::channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use rand::{rngs::SmallRng, thread_rng, Rng, SeedableRng};
use std::{
    any::Any,
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap, HashMap, HashSet},
    fmt,
    panic::resume_unwind,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant, SystemTime},
};
use ton_api::{
    ton::consensus::{
        candidatehashdata::CandidateHashDataOrdinary, candidateid::CandidateId,
        candidateparent::CandidateParent, datatosign::DataToSign,
        simplex::unsignedvote::FinalizeVote, CandidateParent as CandidateParentBoxed,
    },
    IntoBoxed,
};
use ton_block::{
    error, sha256_digest, BlockIdExt, BlockSignaturesPure, BlockSignaturesSimplex,
    BlockSignaturesVariant, CryptoSignature, CryptoSignaturePair, UInt256, ValidatorBaseInfo,
};

/*
===================================================================================================
    Constants
===================================================================================================
*/

const MAIN_LOOP_NAME: &str = "EMUMAIN"; // Emulator main processing thread
const CALLBACKS_LOOP_NAME: &str = "EMUCB"; // Emulator callbacks thread

/// Maximum sleep on the main task queue when nothing is due. Bounded so the
/// main loop polls `stop_flag` often enough for prompt shutdown.
const MAIN_LOOP_MAX_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum blocking pull on the callbacks queue. Same rationale.
const CALLBACKS_LOOP_MAX_TIMEOUT: Duration = Duration::from_millis(100);

/// Threshold above which a task's queueing latency is treated as an overload
/// signal. Mirrors `simplex/session.rs::TASK_QUEUE_WARN_PROCESSING_LATENCY`.
const TASK_QUEUE_WARN_PROCESSING_LATENCY: Duration = Duration::from_millis(1000);

/// `stop()` polls the thread-stopped flags at this interval. Mirrors
/// `simplex/session.rs::CHECKING_INTERVAL`.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Per-slot candidate-cache retention window (in slots).
///
/// On every slot tick the emulator trims `candidates` / `accepted_parents`
/// to retain only entries within the most recent `[current_slot -
/// CANDIDATE_RETENTION_SLOTS, current_slot]` range so long-running
/// emulators (and tests that run many slots) do not grow unboundedly. The
/// window is generous so it covers the longest in-flight downstream
/// callback chain (observed/finalized + leader-window chaining), well
/// beyond any expected [`EmulatorOptions::slots_per_leader_window`].
const CANDIDATE_RETENTION_SLOTS: u32 = 256;

/*
    Task types
*/

/// Task posted to the main task queue: closure over the emulator core state.
type MainTaskFn = Box<dyn FnOnce(&mut EmulatorCore) + Send + 'static>;

/// Task posted to the callbacks task queue: a side-effecting closure that
/// invokes one [`SessionListener`] method.
type CallbackTaskFn = Box<dyn FnOnce() + Send + 'static>;

/// Pointer to the main task queue.
type MainTaskQueuePtr = Arc<dyn TaskQueue<MainTaskMsg>>;

/// Pointer to the callbacks task queue.
type CallbackTaskQueuePtr = Arc<dyn TaskQueue<CallbackTaskFn>>;

/// One message on the main task queue. Carrying the [`EmulatorDelaySpec`]
/// alongside the closure lets the main thread apply skip-probability and
/// random delay uniformly when the message is pulled, without leaking any
/// scheduler state to other threads.
struct MainTaskMsg {
    spec: EmulatorDelaySpec,
    work: MainTaskFn,
}

/*
===================================================================================================
    Cloned signing helpers
===================================================================================================

CLONED FROM `node/simplex/src/utils.rs` and
`node/simplex/src/session_processor.rs::build_simplex_signatures_variant`.
`consensus-common` deliberately does not depend on `simplex`, so the emulator
keeps a private copy. Keep semantics in sync with the simplex originals if
those change.
*/

fn create_data_to_sign(session_id: &SessionId, data: &[u8]) -> Vec<u8> {
    let payload = DataToSign { session_id: session_id.clone(), data: data.to_vec() };
    crate::serialize_tl_boxed_object!(&payload.into_boxed())
}

fn create_finalize_vote_to_sign(slot: u32, hash: &UInt256) -> Vec<u8> {
    let cid = CandidateId { slot: slot as i32, hash: hash.clone() };
    let vote = FinalizeVote { id: cid.into_boxed() };
    crate::serialize_tl_boxed_object!(&vote.into_boxed())
}

fn sign_with_session(session_id: &SessionId, data: &[u8], key: &PrivateKey) -> Result<Vec<u8>> {
    let to_sign = create_data_to_sign(session_id, data);
    Ok(key.sign(&to_sign)?.to_vec())
}

/// Sign `consensus.simplex.finalizeVote(slot, hash)` wrapped in
/// `consensus.dataToSign(session_id, ...)` for finalized block proofs.
fn sign_candidate(
    session_id: &SessionId,
    slot: u32,
    candidate_hash: &UInt256,
    key: &PrivateKey,
) -> Result<Vec<u8>> {
    let inner = create_finalize_vote_to_sign(slot, candidate_hash);
    sign_with_session(session_id, &inner, key)
}

/// Parent candidate id used in Simplex `CandidateHashDataOrdinary`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ParentCandidateRef {
    slot: u32,
    hash: UInt256,
    block_id: BlockIdExt,
}

/// TL-serialized `CandidateHashDataOrdinary` bytes.
fn build_candidate_hash_data_bytes(
    block_id: &BlockIdExt,
    collated_file_hash: &UInt256,
    parent: Option<&ParentCandidateRef>,
) -> Vec<u8> {
    let parent_tl = match parent {
        Some(parent) => {
            let parent_id = CandidateId { slot: parent.slot as i32, hash: parent.hash.clone() };
            CandidateParent { id: parent_id.into_boxed() }.into_boxed()
        }
        None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
    };
    let block_id_tl = BlockIdExt {
        shard_id: block_id.shard_id.clone(),
        seq_no: block_id.seq_no,
        root_hash: block_id.root_hash.clone(),
        file_hash: block_id.file_hash.clone(),
    };
    let chd = CandidateHashDataOrdinary {
        block: block_id_tl,
        collated_file_hash: collated_file_hash.clone(),
        parent: parent_tl,
    };
    crate::serialize_tl_boxed_object!(&chd.into_boxed())
}

/// `sha256(CandidateHashDataOrdinary(block_id, collated_file_hash, parent))`.
fn compute_candidate_id_hash(
    block_id: &BlockIdExt,
    collated_file_hash: &UInt256,
    parent: Option<&ParentCandidateRef>,
) -> UInt256 {
    let bytes = build_candidate_hash_data_bytes(block_id, collated_file_hash, parent);
    UInt256::from_slice(&sha256_digest(&bytes))
}

/// Build a `BlockSignaturesVariant::Simplex` from per-signer sigs and weights.
/// Each `(node_id, sig_payload)` payload must hold a 64-byte Ed25519 signature.
fn build_simplex_signatures_variant(
    session_id: &SessionId,
    slot: u32,
    candidate_hash_data_bytes: Vec<u8>,
    signed: &[(PublicKeyHash, BlockPayloadPtr)],
    weights: &[u64],
    is_final: bool,
) -> Result<BlockSignaturesVariant> {
    if signed.is_empty() {
        return Err(error!("build_simplex_signatures_variant: empty signers for slot={slot}"));
    }
    if signed.len() != weights.len() {
        return Err(error!(
            "build_simplex_signatures_variant: signed/weights length mismatch ({} vs {})",
            signed.len(),
            weights.len()
        ));
    }

    let mut pure = BlockSignaturesPure::new();
    let mut total_weight: u64 = 0;
    for ((node_id, payload), &w) in signed.iter().zip(weights.iter()) {
        let bytes = payload.data();
        if bytes.len() < 64 {
            return Err(error!(
                "build_simplex_signatures_variant: bad sig length {} for node {node_id}",
                bytes.len()
            ));
        }
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[0..32]);
        s.copy_from_slice(&bytes[32..64]);
        pure.add_sigpair(CryptoSignaturePair {
            node_id_short: (*node_id.data()).into(),
            sign: CryptoSignature::with_r_s(&r, &s),
        });
        total_weight = total_weight.saturating_add(w);
    }
    pure.set_weight(total_weight);

    let candidate_data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_hash_data_bytes)?;
    let sigs = BlockSignaturesSimplex::with_params(
        ValidatorBaseInfo::with_params(0, 0),
        pure,
        session_id.clone(),
        slot,
        candidate_data,
        is_final,
    );
    Ok(BlockSignaturesVariant::Simplex(sigs))
}

/// Strict 2/3 threshold (mirrors simplex `threshold_66`).
fn threshold_66(total_weight: ValidatorWeight) -> ValidatorWeight {
    if total_weight == 0 {
        return 0;
    }
    (((total_weight as u128) * 2) / 3 + 1) as ValidatorWeight
}

/// First 8 hex characters of the session id, for log prefixes.
/// Clamp a sequence of `Option<Instant>` trigger times so they are
/// monotonically non-decreasing.
///
/// Used by [`EmulatorCore::register_candidate`] to enforce the documented
/// per-slot observation order (body-observed -> notar-observed -> finalized)
/// after each delay spec has been rolled independently.
///
/// `None` entries (skipped callbacks) are passed through unchanged and are
/// **not** used as the floor for subsequent entries, so a skipped step does
/// not retroactively widen the next step's delay.
fn clamp_trigger_instants_monotonic<const N: usize>(
    mut times: [Option<Instant>; N],
) -> [Option<Instant>; N] {
    let mut floor: Option<Instant> = None;
    for slot in times.iter_mut() {
        if let Some(inst) = *slot {
            let clamped = match floor {
                Some(f) => inst.max(f),
                None => inst,
            };
            *slot = Some(clamped);
            floor = Some(clamped);
        }
    }
    times
}

fn short_session_id(session_id: &SessionId) -> String {
    let hex = session_id.to_hex_string();
    if hex.len() <= 8 {
        hex
    } else {
        hex[..8].to_string()
    }
}

/*
===================================================================================================
    AsyncRequest implementation (one per local-leader on_generate_slot call)
===================================================================================================
*/

/// Monotonic id source for [`EmulatorAsyncRequest`].
static REQUEST_ID_GEN: AtomicU32 = AtomicU32::new(1);

/// Implementation of [`AsyncRequest`] returned to a [`SessionListener`] from
/// `on_generate_slot`. The listener may call `is_cancelled()` to abandon
/// collation work; the emulator marks it cancelled when stopping or when
/// the optional [`EmulatorOptions::local_collation_timeout`] expires.
struct EmulatorAsyncRequest {
    id: u32,
    cancelled: AtomicBool,
    creation_time: SystemTime,
}

impl EmulatorAsyncRequest {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            id: REQUEST_ID_GEN.fetch_add(1, Ordering::Relaxed),
            cancelled: AtomicBool::new(false),
            creation_time: SystemTime::now(),
        })
    }
}

impl AsyncRequest for EmulatorAsyncRequest {
    fn get_request_id(&self) -> u32 {
        self.id
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn get_creation_time(&self) -> SystemTime {
        self.creation_time
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Marks an emulator worker thread as stopped on normal return and during
/// panic unwinding, without catching the panic. `stop()` later joins the
/// thread and re-raises any panic on the caller thread.
struct ThreadStoppedGuard {
    stopped: Arc<AtomicBool>,
}

impl Drop for ThreadStoppedGuard {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/*
===================================================================================================
    TaskQueue (simplex-style FIFO crossbeam channel)
===================================================================================================
*/

/// Task queue interface. Mirrors the trait of the same name in
/// `node/simplex/src/task_queue.rs` minus the metrics-related members; the
/// emulator's tests do not rely on metrics-receiver wiring.
trait TaskQueue<FuncPtr: Send + 'static>: Send + Sync {
    /// Post a closure for asynchronous execution by the consumer thread.
    fn post_closure(&self, task: FuncPtr);

    /// Block on the queue up to `timeout`. Returns `Some(closure)` when a
    /// task arrived, `None` on timeout or disconnect. The `last_warn_dump`
    /// reference is updated whenever the function logs a queue-overload
    /// warning, mirroring the simplex pattern.
    fn pull_closure(&self, timeout: Duration, last_warn_dump: &mut SystemTime) -> Option<FuncPtr>;

    /// Returns `true` if no tasks are currently pending.
    fn is_empty(&self) -> bool;

    /// Drop all pending tasks. Called once the consumer loop has exited so
    /// any captured Arc refs are released promptly.
    fn flush(&self);
}

/// FIFO crossbeam-channel-backed task queue with simple latency telemetry.
struct TaskQueueImpl<FuncPtr> {
    /// Display name for log lines (e.g. `"EMUMAIN:abcdef01"`).
    name: String,
    /// Producer side of the FIFO channel.
    sender: Sender<TaskDesc<FuncPtr>>,
    /// Consumer side of the FIFO channel.
    receiver: Receiver<TaskDesc<FuncPtr>>,
}

/// Wraps a queued task with its creation time so the consumer can detect
/// queueing latency overload.
struct TaskDesc<FuncPtr> {
    task: FuncPtr,
    creation_time: SystemTime,
}

impl<FuncPtr> TaskQueueImpl<FuncPtr>
where
    FuncPtr: Send + 'static,
{
    fn new(name: String) -> Arc<Self> {
        let (sender, receiver) = unbounded::<TaskDesc<FuncPtr>>();
        Arc::new(Self { name, sender, receiver })
    }
}

impl<FuncPtr> TaskQueue<FuncPtr> for TaskQueueImpl<FuncPtr>
where
    FuncPtr: Send + 'static,
{
    fn post_closure(&self, task: FuncPtr) {
        let desc = TaskDesc { task, creation_time: SystemTime::now() };
        if let Err(send_error) = self.sender.send(desc) {
            // The consumer thread has exited (channel disconnected). This is
            // expected during shutdown; log at debug level only.
            log::debug!("ConsensusEmulator/{} post_closure: {send_error}", self.name);
        }
    }

    fn pull_closure(&self, timeout: Duration, last_warn_dump: &mut SystemTime) -> Option<FuncPtr> {
        match self.receiver.recv_timeout(timeout) {
            Ok(desc) => {
                let latency =
                    SystemTime::now().duration_since(desc.creation_time).unwrap_or(Duration::ZERO);
                if latency > TASK_QUEUE_WARN_PROCESSING_LATENCY {
                    if let Ok(elapsed) = last_warn_dump.elapsed() {
                        if elapsed > TASK_QUEUE_WARN_PROCESSING_LATENCY {
                            log::warn!(
                                "ConsensusEmulator/{}: task queue latency is {:.3}s \
                                 (expected max {:.3}s)",
                                self.name,
                                latency.as_secs_f64(),
                                TASK_QUEUE_WARN_PROCESSING_LATENCY.as_secs_f64()
                            );
                            *last_warn_dump = SystemTime::now();
                        }
                    }
                }
                Some(desc.task)
            }
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    fn flush(&self) {
        while self.receiver.try_recv().is_ok() {}
    }
}

/*
===================================================================================================
    EmulatorCore (per-emulator state owned by the main thread)
===================================================================================================

The core is created on the main thread and **never** shared with another
thread. Closures pulled from `main_task_queue` receive `&mut EmulatorCore`;
they are free to mutate any field without synchronization. Listener
invocations are produced as `FnOnce()` closures and posted to
`callbacks_queue` so they run on the EMUCB thread.

The local `delayed_heap` is the emulator's "timer wheel": each pulled
[`MainTaskMsg`] either fires immediately (delay rolled to zero) or is pushed
to the heap with its computed `trigger_at`; the main loop drains all due
heap entries on every iteration.
*/

/// Per-slot candidate snapshot kept on the main thread.
///
/// Both code paths (local-leader collation and remote-leader synthesis)
/// converge on this record before scheduling the downstream listener
/// callbacks. It carries everything required to deliver
/// `on_candidate_observed` and `on_block_finalized` without consulting the
/// listener again, so once a candidate has been cached the slot can be
/// finalized even if the listener becomes slow or unreachable.
#[derive(Clone)]
struct CachedCandidate {
    /// Full block identity used for `on_candidate_observed` and
    /// `on_block_finalized` payloads.
    block_id: BlockIdExt,
    /// Optional parent candidate id encoded into `CandidateHashDataOrdinary`.
    parent: Option<ParentCandidateRef>,
    /// Canonical Simplex candidate hash, including the parent above.
    candidate_hash: UInt256,
    /// Convenience copy of `block_id.root_hash` (signed candidate hash).
    root_hash: BlockHash,
    /// Convenience copy of `block_id.file_hash` (always zero for emulator).
    file_hash: BlockHash,
    /// Hash of the collated data; mixed into the candidate-hash computation.
    collated_file_hash: BlockHash,
    /// Block body payload, surfaced via `on_candidate_observed` and
    /// `on_block_finalized`.
    data: BlockPayloadPtr,
    /// Collated data payload, surfaced via `on_candidate_observed`.
    collated_data: BlockPayloadPtr,
    /// Validator-set index of the slot's leader; used to recompute
    /// [`BlockSourceInfo`] and to determine whether the listener's
    /// `local_collated` flag should be set.
    leader_idx: usize,
}

/// Single entry of the main thread's delayed-task wheel.
///
/// Ordered by `trigger_at` first (earliest deadline pops first when wrapped
/// in `Reverse`); ties at the same instant are broken by `seq`, a monotonic
/// counter, so the heap behaves as FIFO at equal deadlines.
struct DelayedTaskDesc {
    /// Absolute instant at which the closure becomes due to run.
    trigger_at: Instant,
    /// FIFO tiebreaker for entries with identical `trigger_at`.
    seq: u64,
    /// Closure to invoke on the main thread with `&mut EmulatorCore`.
    work: MainTaskFn,
}

impl PartialEq for DelayedTaskDesc {
    fn eq(&self, other: &Self) -> bool {
        self.trigger_at == other.trigger_at && self.seq == other.seq
    }
}
impl Eq for DelayedTaskDesc {}
impl PartialOrd for DelayedTaskDesc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for DelayedTaskDesc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Wrapped in `Reverse` for the BinaryHeap: smallest `trigger_at` first.
        self.trigger_at.cmp(&other.trigger_at).then_with(|| self.seq.cmp(&other.seq))
    }
}

/// Either a deterministic seeded `SmallRng` (for replayable tests) or
/// `thread_rng()`.
enum RngState {
    Seeded(SmallRng),
    Thread,
}

impl RngState {
    fn gen_f64(&mut self) -> f64 {
        match self {
            RngState::Seeded(r) => r.gen::<f64>(),
            RngState::Thread => thread_rng().gen::<f64>(),
        }
    }

    /// Uniform integer in `lo..=hi` (inclusive on both ends).
    ///
    /// Caller must ensure `lo <= hi`. Used by [`EmulatorCore::roll_trigger_instant`]
    /// so the documented `max_ms` upper bound is actually reachable and the
    /// distribution is unbiased (the prior float-multiply-and-truncate path
    /// never yielded `max_ms` and was biased toward `min_ms`).
    fn gen_range_u64_inclusive(&mut self, lo: u64, hi: u64) -> u64 {
        debug_assert!(lo <= hi, "RngState::gen_range_u64_inclusive: lo ({lo}) > hi ({hi})");
        match self {
            RngState::Seeded(r) => r.gen_range(lo..=hi),
            RngState::Thread => thread_rng().gen_range(lo..=hi),
        }
    }
}

/// Per-emulator state owned by the main thread.
///
/// `EmulatorCore` is constructed inside the main loop and **never** moved
/// or shared across threads. All state mutation happens single-threadedly
/// from closures pulled out of the main task queue, so each field below is
/// free of synchronization primitives.
///
/// Cross-thread communication uses the two task queues
/// (`main_queue`, `callbacks_queue`); the only handles shared with other
/// threads are the `Arc`-wrapped queue endpoints, the `Mutex<EmulatorParams>`
/// snapshot accessor, and the `Arc<AtomicBool>` stop flag.
struct EmulatorCore {
    // ---- Immutable configuration (cloned/derived from EmulatorOptions) ----
    /// Construction-time emulator configuration, including session id,
    /// shard, leader rotation policy, signer subset and timing defaults.
    opts: Arc<EmulatorOptions>,

    /// Full validator set, parallel to the keypairs stored on each
    /// [`SessionNode::public_key`]. Indexed by validator number; never
    /// reordered for the lifetime of the emulator.
    validators: Arc<Vec<SessionNode>>,

    /// Index into [`Self::validators`] of the local node — the only
    /// validator that ever receives `on_generate_slot`.
    local_idx: usize,

    // ---- Thread communication endpoints ----
    /// Producer end of the main task queue. Used to re-post zero-delay
    /// follow-up work from listener-reentry callbacks (which run on the
    /// callbacks thread, not the main thread).
    main_queue: MainTaskQueuePtr,

    /// Producer end of the callbacks task queue. Every listener invocation
    /// is wrapped in an `FnOnce()` and pushed here so the EMUCB thread can
    /// run it without blocking EMUMAIN.
    callbacks_queue: CallbackTaskQueuePtr,

    /// Listener pointer, held weakly per the [`SessionListenerPtr`]
    /// contract — the caller of `create_consensus_emulator` owns the
    /// strong `Arc<dyn SessionListener>` and the emulator silently stops
    /// invoking callbacks when that strong reference is dropped.
    listener: SessionListenerPtr,

    // ---- Block-candidate cache ----
    /// Per-slot snapshot of the candidate currently being driven through
    /// `observed → finalized`. Inserted in `register_candidate`, read by
    /// `notify_candidate_observed` and `notify_block_finalized`. Cleared
    /// implicitly by `destroy()`.
    candidates: HashMap<u32, CachedCandidate>,

    /// Accepted candidate ids by slot, used to select parents for later
    /// candidates. This deliberately survives candidate-cache pruning so
    /// parent selection remains correct in long-running tests.
    accepted_parents: BTreeMap<u32, ParentCandidateRef>,

    /// Weak handles to outstanding `EmulatorAsyncRequest`s issued via
    /// `on_generate_slot`. Cancelled on stop and on
    /// `local_collation_timeout` expiry so a stalled listener cannot pin
    /// in-flight work indefinitely.
    inflight: Vec<Weak<EmulatorAsyncRequest>>,

    /// `BlockIdExt` of the last finalized block. Currently informational;
    /// kept for downstream consumers that may want a stable cursor over
    /// the emulator's progress.
    last_finalized_id: Option<BlockIdExt>,

    /// `true` once [`Session::start`] has been called and the main loop
    /// has scheduled slot 0.
    started: bool,

    // ---- Runtime-mutable knobs ----
    /// Shared, runtime-mutable timing/skip parameters. The `Mutex` is used
    /// only by `Emulator::get_params` / `Emulator::set_params` accessors;
    /// it is **not** taken on the scheduling hot path — `read_params` just
    /// clones a snapshot whenever the main loop needs current values.
    params: Arc<Mutex<EmulatorParams>>,

    // ---- Scheduler state (main-thread-local) ----
    /// Timer wheel: delayed main-thread closures keyed by absolute
    /// `trigger_at`. Strictly local to the main thread, no Mutex required.
    delayed_heap: BinaryHeap<Reverse<DelayedTaskDesc>>,

    /// Monotonic counter providing FIFO ordering for `DelayedTaskDesc`s
    /// scheduled at the same `Instant`.
    delayed_seq: u64,

    /// Emulator-local RNG used for delay and skip rolls. Seeded if
    /// [`EmulatorOptions::deterministic_seed`] is set, otherwise wraps
    /// `thread_rng()` for non-reproducible runs.
    rng: RngState,

    // ---- Lifecycle ----
    /// Shared stop-signal flag, cloned from
    /// [`ConsensusEmulatorImpl::stop_flag`]. Once set, [`Self::process_slot_tick`]
    /// short-circuits — no more slot ticks are scheduled, no more listener
    /// invocations are posted — so the main loop can drain its pending
    /// heap and queue and reach a quiescent state before exiting.
    stop_flag: Arc<AtomicBool>,
}

impl EmulatorCore {
    /*
    ===================================================================================================
        Configuration

        Pure helpers that derive values from the immutable [`EmulatorOptions`]
        and the validator set. None of these touch the heap, the channels, or
        the RNG, so they are safe to call from any of the sections below
        without ordering concerns.
    ===================================================================================================
    */

    /// Returns a clone of the current runtime-mutable [`EmulatorParams`].
    ///
    /// Called once per scheduling decision; the snapshot is independent of
    /// subsequent [`Emulator::set_params`] calls so all the random rolls for
    /// a single slot use the same parameter values.
    fn read_params(&self) -> EmulatorParams {
        self.params.lock().unwrap().clone()
    }

    /// Resolves which validator is the leader for `slot` per the configured
    /// [`EmulatorLeaderRotation`] policy. Always returns a valid index into
    /// [`Self::validators`] thanks to the construction-time checks in
    /// [`ConsensusEmulatorImpl::create`].
    fn leader_index_for_slot(&self, slot: u32) -> usize {
        match &self.opts.leader_rotation {
            EmulatorLeaderRotation::RoundRobin => {
                ((slot / self.opts.slots_per_leader_window) as usize) % self.validators.len()
            }
            EmulatorLeaderRotation::FixedLocal => self.local_idx,
            EmulatorLeaderRotation::FixedIndex(i) => *i,
        }
    }

    fn leader_window_start(&self, slot: u32) -> u32 {
        (slot / self.opts.slots_per_leader_window) * self.opts.slots_per_leader_window
    }

    /// Selects the parent for an accepted candidate.
    ///
    /// The first accepted candidate in a leader window uses the latest
    /// accepted candidate before that window. Later accepted candidates in the
    /// same window chain to the latest accepted earlier slot in that window;
    /// if intermediate candidates were rejected, the chain skips over them.
    fn parent_for_slot(&self, slot: u32) -> Option<ParentCandidateRef> {
        if slot == 0 {
            return self.accepted_parents.get(&0).cloned();
        }
        let window_start = self.leader_window_start(slot);
        let in_window_parent = self
            .accepted_parents
            .range(window_start..slot)
            .next_back()
            .map(|(_slot, parent)| parent.clone());
        in_window_parent.or_else(|| {
            self.accepted_parents
                .range(..window_start)
                .next_back()
                .map(|(_slot, parent)| parent.clone())
        })
    }

    /// Builds the [`BlockSourceInfo`] handed to the listener for a slot
    /// whose leader sits at `leader_idx`.
    ///
    /// `priority.round` is hard-coded to `u32::MAX`, matching Simplex's
    /// roundless mode (slots are not grouped into Catchain-style rounds).
    fn build_source_info(&self, leader_idx: usize) -> BlockSourceInfo {
        BlockSourceInfo {
            source: self.validators[leader_idx].public_key.clone(),
            priority: BlockCandidatePriority {
                round: u32::MAX, // Simplex roundless mode
                first_block_round: 0,
                priority: 0,
            },
        }
    }

    /// Computes a deterministic synthetic `root_hash` for a remote-leader
    /// candidate as `sha256(session_id || slot || leader_adnl_id)`.
    ///
    /// Determinism lets tests (and future replay tooling) re-derive the
    /// candidate hash from publicly available inputs and verify per-validator
    /// signatures without inspecting emulator state.
    fn synthetic_root_hash(&self, slot: u32, leader_idx: usize) -> UInt256 {
        let leader_id = self.validators[leader_idx].adnl_id.data();
        let mut buf = Vec::with_capacity(32 + 4 + 32);
        buf.extend_from_slice(self.opts.session_id.as_slice());
        buf.extend_from_slice(&slot.to_be_bytes());
        buf.extend_from_slice(leader_id);
        UInt256::from_slice(&sha256_digest(&buf))
    }

    /// Materialises [`EmulatorSignerSubset`] into the concrete validator
    /// indices that should sign each finalized block.
    ///
    /// Construction-time validation in [`ConsensusEmulatorImpl::create`]
    /// guarantees the resulting weight already clears `threshold_66`, so
    /// callers can rely on the returned set producing a valid
    /// [`BlockSignaturesVariant::Simplex`].
    fn finalize_signer_indices(&self) -> Vec<usize> {
        match &self.opts.signer_subset {
            EmulatorSignerSubset::All => (0..self.validators.len()).collect(),
            EmulatorSignerSubset::First(k) => (0..*k).collect(),
            EmulatorSignerSubset::ByIndex(v) => v.clone(),
        }
    }

    /// Seeds the parent-selection map from Session::start's previous blocks.
    /// ValidatorGroup's Simplex collation path requires explicit parents even
    /// for the first emulated slot, so the last started parent must be visible
    /// to parent_for_slot(0).
    fn seed_started_parents(&mut self, prev_blocks: Vec<BlockIdExt>) {
        let Some(block_id) = prev_blocks.into_iter().rev().find(|id| *id != BlockIdExt::default())
        else {
            return;
        };
        self.last_finalized_id = Some(block_id.clone());
        self.accepted_parents
            .insert(0, ParentCandidateRef { slot: 0, hash: block_id.root_hash.clone(), block_id });
    }

    /*
    ===================================================================================================
        Scheduler

        The emulator's "timer wheel" plus the slot-tick handler. The main
        thread owns a private [`BinaryHeap`] keyed by absolute `trigger_at`
        [`Instant`]s; entries are pushed by [`Self::schedule_delayed`] /
        [`Self::schedule_at`] and drained at the top of every main-loop
        iteration by [`Self::drain_due_tasks`]. The slot-tick handler
        [`Self::process_slot_tick`] is invoked from a delayed-heap entry and
        dispatches into collation or validation depending on who leads the
        slot.
    ===================================================================================================
    */

    /// Rolls a single [`EmulatorDelaySpec`] against the emulator RNG.
    ///
    /// Returns:
    /// * `None` — `skip_probability` rolled and the task should be dropped.
    /// * `Some(t)` — absolute instant at which the task becomes due.
    ///
    /// The RNG is consumed once for the skip roll (only if `skip_probability
    /// > 0.0`) and once for the delay roll (only when the delay window is
    /// non-zero). This keeps the seeded-RNG byte stream deterministic across
    /// runs with identical `EmulatorDelaySpec`s.
    fn roll_trigger_instant(&mut self, spec: EmulatorDelaySpec) -> Option<Instant> {
        if spec.skip_probability > 0.0 && self.rng.gen_f64() < spec.skip_probability {
            return None;
        }
        let delay_ms = if spec.min_ms == 0 && spec.max_ms == 0 {
            0
        } else {
            // `min(...)` / `max(...)` guard the inclusive range against a
            // caller-side accidental bound swap, even though
            // `EmulatorDelaySpec::validate` would normally have rejected it.
            let lo = spec.min_ms.min(spec.max_ms);
            let hi = spec.min_ms.max(spec.max_ms);
            if lo == hi {
                lo
            } else {
                // Uniform integer sample in `lo..=hi` so the documented
                // `max_ms` upper bound is reachable and the distribution is
                // unbiased (prior float-multiply-and-truncate never reached
                // `max_ms` and slightly skewed toward `min_ms`).
                self.rng.gen_range_u64_inclusive(lo, hi)
            }
        };
        Some(Instant::now() + Duration::from_millis(delay_ms))
    }

    /// Schedules `work` for execution on the main thread after a delay
    /// drawn from `spec`. Skip-probability and delay are rolled in place;
    /// a skip silently drops the closure.
    fn schedule_delayed(&mut self, spec: EmulatorDelaySpec, work: MainTaskFn) {
        let trigger_at = match self.roll_trigger_instant(spec) {
            Some(t) => t,
            None => return,
        };
        let seq = self.delayed_seq;
        self.delayed_seq = self.delayed_seq.wrapping_add(1);
        self.delayed_heap.push(Reverse(DelayedTaskDesc { trigger_at, seq, work }));
    }

    /// Schedules `work` at an absolute instant — no skip roll, no random
    /// delay. Reserved for safety-net timers (e.g.
    /// [`EmulatorOptions::local_collation_timeout`]) that must fire
    /// deterministically.
    fn schedule_at(&mut self, when: Instant, work: MainTaskFn) {
        let seq = self.delayed_seq;
        self.delayed_seq = self.delayed_seq.wrapping_add(1);
        self.delayed_heap.push(Reverse(DelayedTaskDesc { trigger_at: when, seq, work }));
    }

    /// Pops and executes every heap entry whose `trigger_at <= now`.
    ///
    /// A closure may itself push new entries onto the heap. They participate
    /// in this same drain pass only if their newly computed `trigger_at` is
    /// already past; otherwise they wait for a later main-loop iteration.
    fn drain_due_tasks(&mut self) {
        loop {
            let now = Instant::now();
            let due =
                matches!(self.delayed_heap.peek(), Some(Reverse(top)) if top.trigger_at <= now);
            if !due {
                break;
            }
            let entry = self.delayed_heap.pop().unwrap().0;
            (entry.work)(self);
        }
    }

    /// Returns how long the main loop should block on the FIFO main queue
    /// before re-checking the heap.
    ///
    /// The value is clamped to [`MAIN_LOOP_MAX_TIMEOUT`] so the loop wakes
    /// up often enough to poll `stop_flag` for prompt shutdown.
    fn next_pull_timeout(&self) -> Duration {
        match self.delayed_heap.peek() {
            Some(Reverse(top)) => {
                top.trigger_at.saturating_duration_since(Instant::now()).min(MAIN_LOOP_MAX_TIMEOUT)
            }
            None => MAIN_LOOP_MAX_TIMEOUT,
        }
    }

    /// Schedules a slot tick for `slot` after the current
    /// [`EmulatorParams::slot_interval`]. This is the only path that
    /// extends the slot timeline; called from the main loop's ignition
    /// step and from [`Self::process_slot_tick`] itself.
    fn schedule_slot_tick(&mut self, slot: u32, params: &EmulatorParams) {
        let work: MainTaskFn = Box::new(move |core| Self::process_slot_tick(core, slot));
        self.schedule_delayed(params.slot_interval, work);
    }

    /// Handles a slot tick popped off the delayed heap.
    ///
    /// Resolves the leader for `slot`, schedules the *next* slot tick first
    /// so cadence is independent of listener latency, then dispatches into
    /// either the collation path (when the local node is leader) or the
    /// validation path (when a peer is leader).
    ///
    /// During shutdown (`stop_flag` set) this is a no-op: no rescheduling,
    /// no listener post. In-flight slots' downstream callbacks continue to
    /// drain naturally from the heap.
    fn process_slot_tick(core: &mut EmulatorCore, slot: u32) {
        if core.stop_flag.load(Ordering::Relaxed) {
            return;
        }
        let params = core.read_params();
        core.schedule_slot_tick(slot.wrapping_add(1), &params);

        // Per-slot cache trim. Bounds the size of `candidates` /
        // `accepted_parents` / `inflight` for long-running emulators that
        // would otherwise accumulate entries indefinitely (each
        // `CachedCandidate` retains `BlockPayloadPtr`s and parent metadata).
        // The window is large enough to cover the longest in-flight
        // downstream callback chain plus leader-window chaining; entries
        // outside it cannot be referenced anymore.
        if let Some(cutoff) = slot.checked_sub(CANDIDATE_RETENTION_SLOTS) {
            let before = core.candidates.len();
            core.candidates.retain(|&k, _| k >= cutoff);
            core.accepted_parents.retain(|&k, _| k >= cutoff);
            let evicted = before.saturating_sub(core.candidates.len());
            if evicted > 0 {
                log::trace!(
                    "ConsensusEmulator/{}: slot {slot} cache trim, evicted {evicted} \
                     candidate entries below s{cutoff}",
                    short_session_id(&core.opts.session_id)
                );
            }
        }
        core.inflight.retain(|w| w.strong_count() > 0);

        let leader_idx = core.leader_index_for_slot(slot);
        let seqno = core.opts.initial_seqno.wrapping_add(slot);
        log::trace!(
            "ConsensusEmulator/{}: slot {slot} leader_idx={leader_idx}",
            short_session_id(&core.opts.session_id)
        );

        if leader_idx == core.local_idx {
            core.start_local_leader_slot(slot);
        } else {
            core.start_remote_leader_slot(slot, seqno, leader_idx, &params);
        }
    }

    /*
    ===================================================================================================
        Collation

        The "local validator is leader" code path. The emulator drives the
        listener through `on_generate_slot`, optionally arming a timeout
        safety net, and consumes the [`ValidatorBlockCandidate`] the
        listener returns.
    ===================================================================================================
    */

    /// Starts a local-leader slot: arms the optional collation timeout,
    /// then posts `listener.on_generate_slot(...)` to the callbacks queue.
    ///
    /// Steps:
    /// 1. Allocate an [`EmulatorAsyncRequest`] and track it on `inflight`
    ///    so [`Session::stop`] can cancel any in-flight collation.
    /// 2. If [`EmulatorOptions::local_collation_timeout`] is set, schedule
    ///    a deadline on the heap that cancels the request if no candidate
    ///    has been registered by then.
    /// 3. Build the listener-reentry callback. When the listener finishes
    ///    collation, the callback re-posts a zero-delay [`MainTaskMsg`]
    ///    onto the main queue — see the threading invariant in the module
    ///    doc-comment.
    /// 4. Post the listener invocation to the callbacks queue.
    fn start_local_leader_slot(&mut self, slot: u32) {
        let request = EmulatorAsyncRequest::new();
        self.inflight.push(Arc::downgrade(&request));

        if let Some(timeout) = self.opts.local_collation_timeout {
            let req_weak = Arc::downgrade(&request);
            let work: MainTaskFn = Box::new(move |core| {
                if let Some(req) = req_weak.upgrade() {
                    if !core.candidates.contains_key(&slot) {
                        req.cancel();
                        log::warn!(
                            "ConsensusEmulator/{}: slot {slot} local collation timed out",
                            short_session_id(&core.opts.session_id)
                        );
                    }
                }
            });
            self.schedule_at(Instant::now() + timeout, work);
        }

        let listener = match self.listener.upgrade() {
            Some(l) => l,
            None => {
                log::debug!(
                    "ConsensusEmulator/{}: listener gone; dropping local slot {slot}",
                    short_session_id(&self.opts.session_id)
                );
                return;
            }
        };
        let source_info = self.build_source_info(self.local_idx);
        let parent_hint = self
            .parent_for_slot(slot)
            .map(|parent| CollationParentHint::Explicit(vec![parent.block_id]))
            .unwrap_or(CollationParentHint::Implicit);
        let local_idx = self.local_idx;
        let main_for_cb = self.main_queue.clone();
        let request_for_cb = request.clone();
        let cb: ValidatorBlockCandidateCallback = Box::new(move |res| {
            // Listener-reentry: bounce back to the main thread for
            // single-threaded state mutation. See the threading invariant
            // in the module doc-comment.
            let work: MainTaskFn = Box::new(move |core| {
                if request_for_cb.is_cancelled() || core.stop_flag.load(Ordering::Relaxed) {
                    log::debug!(
                        "ConsensusEmulator/{}: slot {slot} local candidate arrived after \
                         cancellation; dropping",
                        short_session_id(&core.opts.session_id)
                    );
                    return;
                }
                core.accept_local_candidate(slot, local_idx, res);
            });
            main_for_cb.post_closure(MainTaskMsg { spec: EmulatorDelaySpec::ZERO, work });
        });
        let request_ptr: AsyncRequestPtr = request;
        self.callbacks_queue.post_closure(Box::new(move || {
            listener.on_generate_slot(source_info, request_ptr, parent_hint, cb);
        }));
    }

    /// Processes the `Result` returned by the local listener's
    /// `on_generate_slot`.
    ///
    /// On success, the listener-supplied [`ValidatorBlockCandidate`] is
    /// flattened into a [`CachedCandidate`] and handed off to
    /// [`Self::register_candidate`] for caching and downstream scheduling.
    /// On error, the slot is logged and dropped — no observed/finalized
    /// callbacks will fire for it.
    fn accept_local_candidate(
        &mut self,
        slot: u32,
        leader_idx: usize,
        res: Result<ValidatorBlockCandidatePtr>,
    ) {
        let candidate = match res {
            Ok(c) => c,
            Err(e) => {
                log::debug!(
                    "ConsensusEmulator/{}: slot {slot} local candidate generation failed: {e}",
                    short_session_id(&self.opts.session_id)
                );
                return;
            }
        };
        let cached = CachedCandidate {
            block_id: candidate.id.clone(),
            parent: None,
            candidate_hash: UInt256::default(),
            root_hash: candidate.id.root_hash.clone(),
            file_hash: candidate.id.file_hash.clone(),
            collated_file_hash: candidate.collated_file_hash.clone(),
            data: candidate.data.clone(),
            collated_data: candidate.collated_data.clone(),
            leader_idx,
        };
        self.register_candidate(slot, cached);
    }

    /*
    ===================================================================================================
        Validation

        The "a peer is leader" code path. The emulator synthesises a
        deterministic candidate on the leader's behalf and delivers it to
        the listener via `on_candidate`; the listener's `Ok` / `Err`
        decision determines whether the candidate proceeds to caching.
    ===================================================================================================
    */

    /// Starts a remote-leader slot: synthesises a deterministic candidate
    /// (root hash from [`Self::synthetic_root_hash`], empty file hash and
    /// payloads) and schedules its delivery on the heap after the current
    /// [`EmulatorParams::on_candidate_delay`].
    fn start_remote_leader_slot(
        &mut self,
        slot: u32,
        seqno: u32,
        leader_idx: usize,
        params: &EmulatorParams,
    ) {
        let root_hash = self.synthetic_root_hash(slot, leader_idx);
        let file_hash = UInt256::default();
        let collated_file_hash = UInt256::default();
        let block_id = BlockIdExt {
            shard_id: self.opts.shard.clone(),
            seq_no: seqno,
            root_hash: root_hash.clone(),
            file_hash: file_hash.clone(),
        };
        let cached = CachedCandidate {
            block_id,
            parent: None,
            candidate_hash: UInt256::default(),
            root_hash,
            file_hash,
            collated_file_hash,
            data: ConsensusCommonFactory::create_empty_block_payload(),
            collated_data: ConsensusCommonFactory::create_empty_block_payload(),
            leader_idx,
        };

        let work: MainTaskFn = Box::new(move |core| core.notify_candidate(slot, cached));
        self.schedule_delayed(params.on_candidate_delay, work);
    }

    /// Delivers a synthesised remote candidate to the listener by posting
    /// `listener.on_candidate(...)` to the callbacks queue. The
    /// listener-reentry callback bounces the decision back to the main
    /// thread so caching happens single-threadedly:
    ///
    /// * `Ok(_)` — the listener accepted the candidate; the main thread
    ///   runs [`Self::register_candidate`] which schedules the downstream
    ///   `observed`/`finalized` notifications.
    /// * `Err(_)` — the listener rejected the candidate; the slot is
    ///   silently dropped.
    fn notify_candidate(&mut self, slot: u32, cached: CachedCandidate) {
        let listener = match self.listener.upgrade() {
            Some(l) => l,
            None => return,
        };
        let source_info = self.build_source_info(cached.leader_idx);
        let cached_for_cb = cached.clone();
        let main_for_cb = self.main_queue.clone();
        let cb: ValidatorBlockCandidateDecisionCallback = Box::new(move |res| {
            let cached2 = cached_for_cb.clone();
            let work: MainTaskFn = Box::new(move |core| match res {
                Ok(_ts) => core.register_candidate(slot, cached2),
                Err(e) => {
                    log::debug!(
                        "ConsensusEmulator/{}: slot {slot} remote candidate rejected: {e}",
                        short_session_id(&core.opts.session_id)
                    );
                }
            });
            main_for_cb.post_closure(MainTaskMsg { spec: EmulatorDelaySpec::ZERO, work });
        });
        let cached_for_call = cached;
        self.callbacks_queue.post_closure(Box::new(move || {
            listener.on_candidate(
                source_info,
                cached_for_call.root_hash.clone(),
                cached_for_call.data.clone(),
                cached_for_call.collated_data.clone(),
                cb,
            );
        }));
    }

    /*
    ===================================================================================================
        Block-candidates management

        The single entry point for caching a fully-determined candidate and
        scheduling the three downstream listener notifications. Both the
        collation path and the validation path converge here, so the rest
        of the emulator can treat local- and remote-leader slots
        identically once the candidate is registered.
    ===================================================================================================
    */

    /// Caches `cached` under `slot` and schedules the three downstream
    /// listener notifications:
    ///
    /// * `on_candidate_observed(parent_ready = false)` after
    ///   [`EmulatorParams::observed_body_delay`]
    /// * `on_candidate_observed(parent_ready = true)`  after
    ///   [`EmulatorParams::observed_notar_delay`]
    /// * `on_block_finalized(...)`                     after
    ///   [`EmulatorParams::finalized_delay`]
    ///
    /// Each spec is rolled independently against the RNG, so a skip on any
    /// one notification does not affect the others.
    ///
    /// **Ordering**: after rolling, the three trigger instants are clamped
    /// to be monotonically non-decreasing — body-observed
    /// (`parent_ready = false`) <= notar-observed (`parent_ready = true`)
    /// <= block-finalized — so the listener observes them in the documented
    /// sequence even when the rolled delays would have inverted the order.
    /// A skipped (`None`) entry is preserved as-skipped and is not used as
    /// the floor for the subsequent entry.
    fn register_candidate(&mut self, slot: u32, mut cached: CachedCandidate) {
        let parent = self.parent_for_slot(slot);
        let candidate_hash = compute_candidate_id_hash(
            &cached.block_id,
            &cached.collated_file_hash,
            parent.as_ref(),
        );
        cached.parent = parent.clone();
        cached.candidate_hash = candidate_hash.clone();
        self.candidates.insert(slot, cached);
        let block_id =
            self.candidates.get(&slot).expect("candidate just inserted").block_id.clone();
        self.accepted_parents
            .insert(slot, ParentCandidateRef { slot, hash: candidate_hash, block_id });
        self.inflight.retain(|w| w.strong_count() > 0);

        let params = self.read_params();
        let body_at = self.roll_trigger_instant(params.observed_body_delay);
        let notar_at = self.roll_trigger_instant(params.observed_notar_delay);
        let final_at = self.roll_trigger_instant(params.finalized_delay);
        let [body_at, notar_at, final_at] =
            clamp_trigger_instants_monotonic([body_at, notar_at, final_at]);

        if let Some(when) = body_at {
            let work: MainTaskFn =
                Box::new(move |core| core.notify_candidate_observed(slot, false));
            self.schedule_at(when, work);
        }
        if let Some(when) = notar_at {
            let work: MainTaskFn = Box::new(move |core| core.notify_candidate_observed(slot, true));
            self.schedule_at(when, work);
        }
        if let Some(when) = final_at {
            let work: MainTaskFn = Box::new(move |core| core.notify_block_finalized(slot));
            self.schedule_at(when, work);
        }
    }

    /*
    ===================================================================================================
        Listener callbacks

        The methods that actually post `SessionListener` invocations to the
        callbacks queue (in addition to those posted from the collation
        and validation sections). Each one looks up the cached candidate,
        builds the listener-call payload, and hands the closure to EMUCB.
        These functions are scheduled by [`Self::register_candidate`].
    ===================================================================================================
    */

    /// Posts `SessionListener::on_candidate_observed` for `slot` with the
    /// supplied `parent_ready` flag.
    ///
    /// Silently dropped if the slot's candidate has been evicted (it
    /// should not happen with the current code paths, but the check keeps
    /// the function robust if `destroy()` clears the cache concurrently)
    /// or if the listener has been released.
    fn notify_candidate_observed(&self, slot: u32, parent_ready: bool) {
        let cached = match self.candidates.get(&slot) {
            Some(c) => c.clone(),
            None => return,
        };
        let listener = match self.listener.upgrade() {
            Some(l) => l,
            None => return,
        };
        let local_collated = cached.leader_idx == self.local_idx;
        self.callbacks_queue.post_closure(Box::new(move || {
            listener.on_candidate_observed(
                cached.block_id,
                cached.data,
                cached.collated_data,
                CandidateObservedFlags { body_present: true, parent_ready, local_collated },
            );
        }));
    }

    /// Posts `SessionListener::on_block_finalized` for `slot`.
    ///
    /// Steps:
    /// 1. Look up the cached candidate; drop silently if missing.
    /// 2. Compute the canonical Simplex candidate hash from the cached
    ///    `block_id` and `collated_file_hash`.
    /// 3. For every signer in [`EmulatorSignerSubset`], sign the candidate
    ///    hash with that validator's keypair (carried by
    ///    [`SessionNode::public_key`]).
    /// 4. Assemble the per-validator sigs into both `approve_signatures`
    ///    (the `Vec<(PublicKeyHash, BlockPayloadPtr)>` payload) and a
    ///    [`BlockSignaturesVariant::Simplex`] cell tree.
    /// 5. Post the listener invocation to the callbacks queue and update
    ///    [`Self::last_finalized_id`].
    ///
    /// Signing or signature-aggregation errors abort the finalize for this
    /// slot — they are logged at `error` level and the slot is dropped.
    fn notify_block_finalized(&mut self, slot: u32) {
        let cached = match self.candidates.get(&slot) {
            Some(c) => c.clone(),
            None => return,
        };

        // Sign with the configured signer subset. Each validator's
        // `public_key` is a full keypair (private key included) for
        // emulator use — see `ConsensusCommonFactory::create_consensus_emulator`.
        let signer_indices = self.finalize_signer_indices();
        let candidate_hash = cached.candidate_hash.clone();
        let mut signed: Vec<(PublicKeyHash, BlockPayloadPtr)> =
            Vec::with_capacity(signer_indices.len());
        let mut weights: Vec<u64> = Vec::with_capacity(signer_indices.len());
        for &i in &signer_indices {
            let key = &self.validators[i].public_key;
            let sig = match sign_candidate(&self.opts.session_id, slot, &candidate_hash, key) {
                Ok(s) => s,
                Err(e) => {
                    log::error!(
                        "ConsensusEmulator/{}: slot {slot} signer {i} sign failed: {e}; \
                         dropping finalize",
                        short_session_id(&self.opts.session_id)
                    );
                    return;
                }
            };
            signed.push((
                self.validators[i].public_key.id().clone(),
                ConsensusCommonFactory::create_block_payload(sig),
            ));
            weights.push(self.validators[i].weight);
        }

        let candidate_hash_data_bytes = build_candidate_hash_data_bytes(
            &cached.block_id,
            &cached.collated_file_hash,
            cached.parent.as_ref(),
        );
        let signatures = match build_simplex_signatures_variant(
            &self.opts.session_id,
            slot,
            candidate_hash_data_bytes,
            &signed,
            &weights,
            true,
        ) {
            Ok(v) => v,
            Err(e) => {
                log::error!(
                    "ConsensusEmulator/{}: slot {slot} build sigs failed: {e}",
                    short_session_id(&self.opts.session_id)
                );
                return;
            }
        };

        let listener = match self.listener.upgrade() {
            Some(l) => l,
            None => return,
        };
        let source_info = self.build_source_info(cached.leader_idx);
        let block_id_for_state = cached.block_id.clone();
        let block_id = cached.block_id;
        let root_hash = cached.root_hash;
        let file_hash = cached.file_hash;
        let data = cached.data;
        self.callbacks_queue.post_closure(Box::new(move || {
            listener.on_block_finalized(
                block_id,
                source_info,
                root_hash,
                file_hash,
                data,
                signatures,
                signed,
            );
        }));
        if !self.opts.retain_finalized_candidates {
            self.candidates.remove(&slot);
        }
        self.inflight.retain(|w| w.strong_count() > 0);
        self.last_finalized_id = Some(block_id_for_state);
        log::debug!(
            "ConsensusEmulator/{}: slot {slot} finalized",
            short_session_id(&self.opts.session_id)
        );
    }
}

/*
===================================================================================================
    ConsensusEmulatorImpl
===================================================================================================
*/

pub(crate) struct ConsensusEmulatorImpl {
    /// Session identifier (used in log prefixes and signature payloads).
    session_id: SessionId,
    /// Display name for `Display` impl and log lines.
    display_name: String,
    /// Atomic flag honoured by both internal threads' loops.
    stop_flag: Arc<AtomicBool>,
    /// Atomic flag: main_loop has stopped and joined.
    main_processing_thread_stopped: Arc<AtomicBool>,
    /// Atomic flag: callbacks_loop has stopped and joined.
    callbacks_processing_thread_stopped: Arc<AtomicBool>,
    /// Atomic flag set by `Session::start`; the main loop's start gate.
    started_flag: Arc<AtomicBool>,
    /// Runtime-mutable timing/skip parameters. Shared with
    /// [`EmulatorCore::params`].
    params: Arc<Mutex<EmulatorParams>>,
    /// Main task queue. Closures pulled here run on `EMUMAIN` with
    /// `&mut EmulatorCore`.
    main_queue: MainTaskQueuePtr,
    /// Callbacks task queue. Closures pulled here run on `EMUCB`; each one
    /// invokes exactly one [`SessionListener`] method. Held on the impl so
    /// the queue's lifetime is bounded by the emulator's; producers (the
    /// main thread) receive their own clone.
    #[allow(dead_code)]
    callbacks_queue: CallbackTaskQueuePtr,
    /// Join handles, taken on stop.
    main_thread: Mutex<Option<JoinHandle<()>>>,
    callbacks_thread: Mutex<Option<JoinHandle<()>>>,
    /// [`ThreadId`] of the EMUMAIN thread, captured at spawn.
    ///
    /// `stop_impl(wait=true)` compares against `thread::current().id()` to
    /// detect self-calls (a listener invocation on EMUCB calling
    /// [`Session::stop`], or the last `Arc<dyn Emulator>` being dropped from
    /// inside an EMUMAIN closure). Without this check the wait-on-stopped-
    /// flags loop and the subsequent `JoinHandle::join` would deadlock
    /// indefinitely (and `join` on the current thread would also panic).
    main_thread_id: ThreadId,
    /// [`ThreadId`] of the EMUCB thread, captured at spawn. See
    /// [`Self::main_thread_id`] for the rationale.
    callbacks_thread_id: ThreadId,
    /// `Session::start`'s payload, consumed by the main loop's start gate.
    deferred_start: Arc<Mutex<Option<Vec<BlockIdExt>>>>,
}

impl ConsensusEmulatorImpl {
    pub(crate) fn create(
        opts: EmulatorOptions,
        validators: Vec<SessionNode>,
        local_idx: usize,
        listener: SessionListenerPtr,
    ) -> Result<EmulatorPtr> {
        // ---- argument validation ----
        if validators.is_empty() {
            return Err(error!("ConsensusEmulator: validators must not be empty"));
        }
        opts.initial_params.validate()?;
        if opts.slots_per_leader_window == 0 {
            return Err(error!("ConsensusEmulator: slots_per_leader_window must be > 0"));
        }
        if local_idx >= validators.len() {
            return Err(error!(
                "ConsensusEmulator: local_idx {} out of range (n={})",
                local_idx,
                validators.len()
            ));
        }
        if let EmulatorLeaderRotation::FixedIndex(idx) = opts.leader_rotation {
            if idx >= validators.len() {
                return Err(error!(
                    "ConsensusEmulator: EmulatorLeaderRotation::FixedIndex({idx}) out of range \
                     (n={})",
                    validators.len()
                ));
            }
        }

        // ---- signer subset validation (against threshold_66) ----
        let n = validators.len();
        let signer_indices: Vec<usize> = match &opts.signer_subset {
            EmulatorSignerSubset::All => (0..n).collect(),
            EmulatorSignerSubset::First(k) => {
                if *k == 0 || *k > n {
                    return Err(error!(
                        "ConsensusEmulator: EmulatorSignerSubset::First({k}) invalid for n={n}"
                    ));
                }
                (0..*k).collect()
            }
            EmulatorSignerSubset::ByIndex(v) => {
                if v.is_empty() {
                    return Err(error!(
                        "ConsensusEmulator: EmulatorSignerSubset::ByIndex must not be empty"
                    ));
                }
                let mut seen = HashSet::new();
                for &i in v {
                    if i >= n {
                        return Err(error!(
                            "ConsensusEmulator: EmulatorSignerSubset::ByIndex contains {i} \
                             (n={n})"
                        ));
                    }
                    if !seen.insert(i) {
                        return Err(error!(
                            "ConsensusEmulator: EmulatorSignerSubset::ByIndex duplicate {i}"
                        ));
                    }
                }
                v.clone()
            }
        };
        let total_weight: u64 = validators.iter().map(|v| v.weight).sum();
        let signer_weight: u64 = signer_indices.iter().map(|&i| validators[i].weight).sum();
        let needed = threshold_66(total_weight);
        if signer_weight < needed {
            return Err(error!(
                "ConsensusEmulator: signer_subset weight {signer_weight} < threshold_66 \
                 {needed} (total {total_weight}); on_block_finalized would panic"
            ));
        }

        // ---- queues + flags ----
        let session_id = opts.session_id.clone();
        let session_short = short_session_id(&session_id);
        let display_name = format!("ConsensusEmulator({session_short}, n={n}, local={local_idx})");

        let stop_flag = Arc::new(AtomicBool::new(false));
        let main_processing_thread_stopped = Arc::new(AtomicBool::new(false));
        let callbacks_processing_thread_stopped = Arc::new(AtomicBool::new(false));
        let started_flag = Arc::new(AtomicBool::new(false));
        let params = Arc::new(Mutex::new(opts.initial_params.clone()));
        let main_queue: MainTaskQueuePtr =
            TaskQueueImpl::<MainTaskMsg>::new(format!("{MAIN_LOOP_NAME}:{session_short}"));
        let callbacks_queue: CallbackTaskQueuePtr =
            TaskQueueImpl::<CallbackTaskFn>::new(format!("{CALLBACKS_LOOP_NAME}:{session_short}"));
        let deferred_start: Arc<Mutex<Option<Vec<BlockIdExt>>>> = Arc::new(Mutex::new(None));

        log::info!("{display_name}: creating");

        // ---- spawn callbacks thread ----
        // The callbacks thread also watches `main_processing_thread_stopped`
        // so that when stop is requested it keeps draining the queue until
        // the main thread has finished posting and the queue is empty.
        let callbacks_thread = {
            let stop_flag = stop_flag.clone();
            let main_stopped_for_cb = main_processing_thread_stopped.clone();
            let stopped = callbacks_processing_thread_stopped.clone();
            let cb_queue = callbacks_queue.clone();
            let session_id_for_thread = session_id.clone();
            thread::Builder::new()
                .name(format!("{CALLBACKS_LOOP_NAME}:{session_short}"))
                .spawn(move || {
                    let _stopped_guard = ThreadStoppedGuard { stopped };
                    Self::callbacks_loop(
                        stop_flag,
                        main_stopped_for_cb,
                        cb_queue,
                        session_id_for_thread,
                    );
                })
                .map_err(|e| error!("ConsensusEmulator: spawn {CALLBACKS_LOOP_NAME} failed: {e}"))?
        };

        // ---- spawn main thread ----
        let main_thread = {
            let stop_flag = stop_flag.clone();
            let started_flag = started_flag.clone();
            let stopped = main_processing_thread_stopped.clone();
            let callbacks_stopped_for_main = callbacks_processing_thread_stopped.clone();
            let main_queue = main_queue.clone();
            let callbacks = callbacks_queue.clone();
            let deferred = deferred_start.clone();
            let opts = Arc::new(opts);
            let validators = Arc::new(validators);
            let listener_ref = listener;
            let params_for_main = params.clone();
            let session_id_for_thread = session_id.clone();
            thread::Builder::new()
                .name(format!("{MAIN_LOOP_NAME}:{session_short}"))
                .spawn(move || {
                    let _stopped_guard = ThreadStoppedGuard { stopped };
                    Self::main_loop(
                        stop_flag,
                        started_flag,
                        callbacks_stopped_for_main,
                        opts,
                        validators,
                        local_idx,
                        listener_ref,
                        main_queue,
                        callbacks,
                        params_for_main,
                        deferred,
                        session_id_for_thread,
                    );
                })
                .map_err(|e| error!("ConsensusEmulator: spawn {MAIN_LOOP_NAME} failed: {e}"))?
        };

        // Capture thread IDs before moving the JoinHandles so `stop_impl`
        // can detect self-calls without locking the JoinHandle mutexes.
        let main_thread_id = main_thread.thread().id();
        let callbacks_thread_id = callbacks_thread.thread().id();
        let me = Arc::new(Self {
            session_id,
            display_name: display_name.clone(),
            stop_flag,
            main_processing_thread_stopped,
            callbacks_processing_thread_stopped,
            started_flag,
            params,
            main_queue,
            callbacks_queue,
            main_thread: Mutex::new(Some(main_thread)),
            callbacks_thread: Mutex::new(Some(callbacks_thread)),
            main_thread_id,
            callbacks_thread_id,
            deferred_start,
        });
        log::info!("{display_name}: created");
        Ok(me as EmulatorPtr)
    }

    /*
        Main loop & callbacks loop
    */

    #[allow(clippy::too_many_arguments)]
    fn main_loop(
        stop_flag: Arc<AtomicBool>,
        started_flag: Arc<AtomicBool>,
        callbacks_stopped: Arc<AtomicBool>,
        opts: Arc<EmulatorOptions>,
        validators: Arc<Vec<SessionNode>>,
        local_idx: usize,
        listener: SessionListenerPtr,
        main_queue: MainTaskQueuePtr,
        callbacks_queue: CallbackTaskQueuePtr,
        params: Arc<Mutex<EmulatorParams>>,
        deferred_start: Arc<Mutex<Option<Vec<BlockIdExt>>>>,
        session_id: SessionId,
    ) {
        let session_short = short_session_id(&session_id);
        log::info!("ConsensusEmulator/{session_short}: main loop started");

        let rng = match opts.deterministic_seed {
            Some(s) => RngState::Seeded(SmallRng::seed_from_u64(s)),
            None => RngState::Thread,
        };

        let mut core = EmulatorCore {
            opts,
            validators,
            local_idx,
            main_queue: main_queue.clone(),
            callbacks_queue,
            listener,
            candidates: HashMap::new(),
            accepted_parents: BTreeMap::new(),
            inflight: Vec::new(),
            last_finalized_id: None,
            started: false,
            params,
            delayed_heap: BinaryHeap::new(),
            delayed_seq: 0,
            rng,
            stop_flag: stop_flag.clone(),
        };

        let mut last_warn_dump = SystemTime::now();

        // -- start gate: wait until Session::start releases `started_flag`.
        //    The callbacks thread is already running so on_generate_slot
        //    fires immediately once we ignite. Mirrors simplex's start gate.
        while !stop_flag.load(Ordering::Relaxed) {
            if started_flag.load(Ordering::Acquire) && !core.started {
                if let Some(prev) = deferred_start.lock().unwrap().take() {
                    core.seed_started_parents(prev);
                }
                core.started = true;
                let init_params = core.read_params();
                log::info!("ConsensusEmulator/{session_short}: igniting; slot 0 will be scheduled");
                core.schedule_slot_tick(0, &init_params);
                break;
            }
            // Block on the main queue with a short timeout so any pre-start
            // post_closure call is processed promptly. We don't actually
            // expect main-queue posts before start, but this also keeps
            // stop_flag responsive during the gate phase.
            let _ = main_queue.pull_closure(MAIN_LOOP_MAX_TIMEOUT, &mut last_warn_dump);
        }

        // -- main work loop --
        //
        // The loop has two regimes that differ only in their exit condition:
        //
        // * Normal (`!stop_flag`): keep running indefinitely.
        // * Shutdown (`stop_flag` set): keep running until the heap, the main
        //   queue, and the callbacks queue are all empty for several
        //   consecutive iterations. New slot ticks short-circuit in
        //   `process_slot_tick` so the heap drains naturally, while
        //   in-flight slots' downstream callbacks continue to flow through
        //   the cb queue and back into main_queue (via listener-reentry
        //   posts). Requiring `K`-times-in-a-row quiescence with a short
        //   between-check timeout gives the cb→main chain time to flush its
        //   last items without racing.
        const SHUTDOWN_QUIESCE_COUNT: u32 = 4;
        const SHUTDOWN_TICK: Duration = Duration::from_millis(10);
        let mut quiescent_streak: u32 = 0;
        loop {
            let stopping = stop_flag.load(Ordering::Relaxed);

            // 1. Drain all due delayed-heap entries first.
            core.drain_due_tasks();

            // 2. Shutdown-quiescence check.
            if stopping {
                let all_empty = core.delayed_heap.is_empty()
                    && main_queue.is_empty()
                    && (core.callbacks_queue.is_empty()
                        || callbacks_stopped.load(Ordering::Relaxed));
                if all_empty {
                    quiescent_streak = quiescent_streak.saturating_add(1);
                    if quiescent_streak >= SHUTDOWN_QUIESCE_COUNT {
                        break;
                    }
                } else {
                    quiescent_streak = 0;
                }
            }

            // 3. Block on the main task queue with a timeout that wakes us at
            //    the next delayed-heap entry's `trigger_at` (or a fallback).
            //    During shutdown we shorten the timeout so we don't sit idle
            //    while waiting for the chain to flush.
            let mut timeout = core.next_pull_timeout();
            if stopping {
                timeout = timeout.min(SHUTDOWN_TICK);
            }
            if let Some(msg) = main_queue.pull_closure(timeout, &mut last_warn_dump) {
                // Apply skip + delay roll, push to heap or fire immediately.
                let trigger_at = match core.roll_trigger_instant(msg.spec) {
                    Some(t) => t,
                    None => continue, // skip rolled
                };
                if trigger_at <= Instant::now() {
                    (msg.work)(&mut core);
                } else {
                    core.schedule_at(trigger_at, msg.work);
                }
            }
        }

        // -- finalization: cancel any remaining in-flight requests --
        for w in core.inflight.iter() {
            if let Some(req) = w.upgrade() {
                req.cancel();
            }
        }
        main_queue.flush();
        log::info!("ConsensusEmulator/{session_short}: main loop finished");
    }

    fn callbacks_loop(
        stop_flag: Arc<AtomicBool>,
        main_stopped: Arc<AtomicBool>,
        callbacks_queue: CallbackTaskQueuePtr,
        session_id: SessionId,
    ) {
        let session_short = short_session_id(&session_id);
        log::info!("ConsensusEmulator/{session_short}: callbacks loop started");

        let mut last_warn_dump = SystemTime::now();

        // Loop while either (a) the emulator has not been asked to stop, or
        // (b) the main thread is still running so it may still post new
        // tasks, or (c) the queue is non-empty (drain pending tasks). Once
        // the main thread has stopped *and* the queue is empty, exit. This
        // guarantees that listener events posted late in the run are still
        // delivered, mirroring the deterministic shutdown ordering used in
        // simplex `SessionImpl::stop_impl`.
        loop {
            let should_keep_running =
                !stop_flag.load(Ordering::Relaxed) || !main_stopped.load(Ordering::Relaxed);
            let queue_has_work = !callbacks_queue.is_empty();
            if !should_keep_running && !queue_has_work {
                break;
            }

            // Use a short timeout in the drain phase so we don't sit idle
            // waiting on a quiescent queue while shutting down.
            let timeout = if should_keep_running {
                CALLBACKS_LOOP_MAX_TIMEOUT
            } else {
                Duration::from_millis(5)
            };
            if let Some(task) = callbacks_queue.pull_closure(timeout, &mut last_warn_dump) {
                task();
            }
        }

        callbacks_queue.flush();
        log::info!("ConsensusEmulator/{session_short}: callbacks loop finished");
    }

    /*
        Stop / lifecycle
    */

    /// Idempotent. Stops both threads. Polls the thread-stopped flags rather
    /// than calling `JoinHandle::join` directly, mirroring simplex
    /// `SessionImpl::stop_impl`.
    fn stop_impl(&self, wait: bool, propagate_panics: bool) {
        log::info!("{}: stopping (wait={wait})", self.display_name);
        self.stop_flag.store(true, Ordering::Release);

        if !wait {
            return;
        }

        // Self-call detection: if we're running on EMUMAIN or EMUCB the wait
        // loop would deadlock waiting for the calling thread to mark itself
        // stopped, and `join()` on the current thread would panic. Both
        // hazards are triggered by:
        //
        // * A listener invocation on EMUCB calling [`Session::stop`] /
        //   [`Session::destroy`] (the `wait=true` paths).
        // * The last `Arc<dyn Emulator>` being dropped from inside an
        //   EMUMAIN or EMUCB closure, which triggers `Drop` -> `stop_impl(true, ..)`.
        //
        // In both cases we set `stop_flag` above and return. The calling
        // thread completes its current closure, sees `stop_flag` on the next
        // loop iteration, and exits. The peer thread observes the
        // `*_stopped` flag and exits too. The detached JoinHandles are
        // released by `Drop` of `ConsensusEmulatorImpl` (which is non-
        // blocking — `JoinHandle::drop` detaches).
        let current = thread::current().id();
        if current == self.main_thread_id || current == self.callbacks_thread_id {
            log::warn!(
                "{}: stop_impl(wait=true) called from internal thread {:?}; skipping \
                 wait/join to avoid self-deadlock — calling thread will observe \
                 stop_flag on next loop iteration",
                self.display_name,
                current,
            );
            return;
        }

        loop {
            let main_done = self.main_processing_thread_stopped.load(Ordering::Relaxed);
            let cb_done = self.callbacks_processing_thread_stopped.load(Ordering::Relaxed);
            if main_done && cb_done {
                break;
            }
            log::debug!(
                "{}: waiting for threads (main_done={main_done}, cb_done={cb_done})",
                self.display_name
            );
            thread::sleep(STOP_POLL_INTERVAL);
        }

        if let Some(h) = self.callbacks_thread.lock().unwrap().take() {
            if let Err(e) = h.join() {
                if propagate_panics {
                    resume_unwind(e);
                }
                log::error!("{}: callbacks thread panicked during drop", self.display_name);
            }
        }
        if let Some(h) = self.main_thread.lock().unwrap().take() {
            if let Err(e) = h.join() {
                if propagate_panics {
                    resume_unwind(e);
                }
                log::error!("{}: main thread panicked during drop", self.display_name);
            }
        }
        log::info!("{}: stopped", self.display_name);
    }
}

/*
===================================================================================================
    Trait implementations
===================================================================================================
*/

impl Session for ConsensusEmulatorImpl {
    fn start(&self, prev_blocks: Vec<BlockIdExt>, _min_masterchain_block_id: BlockIdExt) {
        log::info!("{}: start(prev_blocks_count={})", self.display_name, prev_blocks.len());
        *self.deferred_start.lock().unwrap() = Some(prev_blocks);
        self.started_flag.store(true, Ordering::Release);
        // Wake the main loop out of its FIFO-channel `recv_timeout` by
        // posting a no-op task; the main loop notices `started_flag` next.
        self.main_queue.post_closure(MainTaskMsg {
            spec: EmulatorDelaySpec::ZERO,
            work: Box::new(|_core| {}),
        });
    }

    fn stop(&self) {
        self.stop_impl(true, true);
    }
    fn stop_async(&self) {
        self.stop_impl(false, false);
    }
    fn destroy(&self) {
        self.stop_impl(true, true);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Emulator for ConsensusEmulatorImpl {
    fn get_params(&self) -> EmulatorParams {
        self.params.lock().unwrap().clone()
    }

    fn set_params(&self, params: EmulatorParams) -> Result<()> {
        params.validate()?;
        log::debug!("{}: set_params({params:?})", self.display_name);
        *self.params.lock().unwrap() = params;
        Ok(())
    }

    fn notify_mc_finalized(&self, applied_top: BlockIdExt) {
        log::debug!("{}: notify_mc_finalized({applied_top})", self.display_name);
        self.main_queue.post_closure(MainTaskMsg {
            spec: EmulatorDelaySpec::ZERO,
            work: Box::new(move |core| {
                core.last_finalized_id = Some(applied_top);
            }),
        });
    }

    fn ensure_candidate_available(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        log::debug!("{}: ensure_candidate_available({block_id}, {opts:?})", self.display_name);
    }

    fn is_stopped(&self) -> bool {
        self.main_processing_thread_stopped.load(Ordering::Relaxed)
            && self.callbacks_processing_thread_stopped.load(Ordering::Relaxed)
    }

    fn is_panicked(&self) -> bool {
        false
    }
}

impl fmt::Display for ConsensusEmulatorImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name)
    }
}

impl Drop for ConsensusEmulatorImpl {
    fn drop(&mut self) {
        log::info!("{}: dropping", self.display_name);
        self.stop_impl(true, false);
    }
}

impl ConsensusEmulatorImpl {
    /// Returns the underlying session id (kept for diagnostic and
    /// downcasting use).
    #[allow(dead_code)]
    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

#[cfg(test)]
#[path = "tests/test_consensus_emulator.rs"]
mod tests;
