/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Session processor implementation for Simplex consensus
//!
//! Contains the core consensus algorithm in a single-threaded context.
//! This module is crate-private.
//! C++ cross-reference: [ton-blockchain/ton](https://github.com/ton-blockchain/ton) (`testnet/validator/consensus/simplex`).
//!
//! # Architecture
//!
//! SessionProcessor integrates SimplexState FSM with the network layer and higher-level
//! callbacks. It runs in a single thread (SXMAIN) and processes events from both the
//! network (via Receiver) and the FSM.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────────┐
//! │ SessionProcessor                                                                │
//! │                                                                                 │
//! │  ┌───────────────────────────────────────────────────────────────────────────┐  │
//! │  │ SimplexState (FSM)                                                        │  │
//! │  │                                                                           │  │
//! │  │  Input:                              Output (SimplexEvent):               │  │
//! │  │  - on_candidate(desc, candidate)     - BroadcastVote(vote)                │  │
//! │  │  - on_vote(desc, idx, vote)          - BlockFinalized(slot, block)        │  │
//! │  │  - on_window_base_ready(desc, window, p)  - SlotSkipped(slot)             │  │
//! │  │  - check_all(desc)                                                        │  │
//! │  └───────────────────────────────────────────────────────────────────────────┘  │
//! │                           │                         │                           │
//! │                           ▼                         ▼                           │
//! │  ┌─────────────────────────────┐    ┌─────────────────────────────────────┐     │
//! │  │ check_all() loop:           │    │ Event dispatch:                     │     │
//! │  │ 1. check_collation()        │    │   BroadcastVote → sign & send       │     │
//! │  │ 2. check_validation()       │    │   BlockFinalized → apply_finalized  │     │
//! │  │ 3. simplex_state.check_all()│    │   SlotSkipped → cleanup             │     │
//! │  │ 4. process_simplex_events() │    └─────────────────────────────────────┘     │
//! │  │ 5. update next awake time   │                                                │
//! │  └─────────────────────────────┘                                                │
//! │                                                                                 │
//! │                      ▲ ReceiverListener        ▼ Receiver                       │
//! │  ┌───────────────────────────────────────────────────────────────────────────┐  │
//! │  │ Network Layer                                                             │  │
//! │  │  - on_vote() → verify sig → simplex_state.on_vote()                       │  │
//! │  │  - on_candidate_received() → verify sig → validate → simplex_state.on_cand() │  │
//! │  │  - broadcast_vote() → sign → receiver.send_vote()                         │  │
//! │  └───────────────────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Consensus Loop
//!
//! Each slot: `Collate → Broadcast → Validate → Notarize → Vote → Collect → Finalize → Deliver`
//!
//! See `README.md` "Consensus Loop" section for the phase-to-method mapping table.
//!
//! # Key Methods
//!
//! - `check_all()` - Main loop entry point, calls FSM and processes events
//! - `check_collation()` - Check if we should generate a block
//! - `invoke_collation()` - Request block generation from higher layer
//! - `generated_block()` - Process collated block, sign, broadcast, submit to FSM
//! - `on_vote()` - Handle incoming vote from network
//! - `broadcast_vote()` - Sign and send vote via receiver
//! - `process_simplex_events()` - Dispatch FSM events to handlers
//! - `debug_dump()` - Dump session and FSM state for debugging

use crate::{
    block::{Candidate, RawCandidate, RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    database::{
        CandidateInfoRecord, FinalizedBlockRecord, PoolStateRecord, SimplexDbPtr, VoteRecord,
    },
    misbehavior::{MisbehaviorReport, VoteResult},
    receiver::{ReceiverPtr, StandstillCertificateType, StandstillTriggerNotification},
    session_description::SessionDescription,
    simplex_state::{
        BlockFinalizedEvent, FinalizationReachedEvent, NotarizationReachedEvent, SimplexEvent,
        SimplexState, SkipCertificateReachedEvent, SlotSkippedEvent, Vote,
    },
    startup_recovery::{CandidateHash, SessionStartupRecoveryListener, SignatureBytes},
    task_queue::{post_callback_closure, CallbackTaskQueuePtr, TaskPtr, TaskQueuePtr},
    utils::{
        extract_consensus_gen_utime_ms, extract_vote_and_signature, sign_vote, threshold_33,
        threshold_66, verify_vote_signature,
    },
    BlockCandidatePriority, ConsensusOverlayManagerPtr, MetricsHandle, PrivateKey, RawVoteData,
    SessionId, SessionListenerPtr, ValidatorWeight, SIMPLEX_ROUNDLESS,
};
use consensus_common::{
    check_execution_time, instrument, profiling::ResultStatusCounter, CandidateObservedFlags,
    EnsureCandidateAvailabilityOptions, StorageAsyncResultPtr,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fmt::{Display, Formatter},
    mem::discriminant,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{
    deserialize_boxed, deserialize_typed, serialize_boxed,
    ton::{
        consensus::{
            candidatedata::{Block as CandidateDataBlock, Empty as CandidateDataEmpty},
            candidateid::CandidateId,
            candidateparent::CandidateParent,
            simplex::{
                candidateandcert::CandidateAndCert, vote::Vote as TlVote, Certificate,
                UnsignedVote, Vote as TlVoteBoxed, VoteSignatureSet as VoteSignatureSetBoxed,
            },
            CandidateData, CandidateHashData, CandidateParent as CandidateParentBoxed,
        },
        validator_session::candidate::CompressedCandidate,
    },
    IntoBoxed,
};
use ton_block::{
    base64_encode, error, fail, sha256_digest, BlockIdExt, BlockSignaturesPure,
    BlockSignaturesSimplex, BlockSignaturesVariant, BocFlags, CryptoSignature, CryptoSignaturePair,
    Error, Result, UInt256, ValidatorBaseInfo,
};

/*
    Constants
*/

/// Maximum timeout for next awake time.
///
/// TODO(simplex-timing): experimental 10ms wake fallback for simnet/testnet validation.
/// Restore the old "far future" behavior after the wake-discipline fixes are validated.
const MAX_AWAKE_TIMEOUT: Duration = Duration::from_millis(10);

/// Maximum generation time for collation - warn if exceeded
const MAX_GENERATION_TIME: Duration = Duration::from_millis(1000);

/// Period without finalizations before triggering debug dump (stalled consensus detection)
/// Matches validator-session ROUND_DEBUG_PERIOD
const ROUND_DEBUG_PERIOD: Duration = Duration::from_secs(15);

/// Maximum history slots to keep in candidate/certificate caches
/// Old entries are cleaned up when slot is finalized
const MAX_HISTORY_SLOTS: u32 = 1024;

/// Delay before requesting a missing candidate from peers
/// This allows time for the broadcast to arrive naturally before triggering a query
const CANDIDATE_REQUEST_DELAY: Duration = Duration::from_secs(1);

/// Minimum interval between repeated `requestCandidate` attempts for the same (slot,hash).
///
/// Under network partitions, a single request may time out; we must retry, but not spam.
const CANDIDATE_REQUEST_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Soft warning threshold for deep empty-parent ancestry walks.
///
/// We keep processing beyond this depth because long empty tails are possible on
/// live networks during prolonged empty-block recovery windows.
const EMPTY_CHAIN_WARN_DEPTH: u32 = 10_000;

/// Hard stop for empty-parent ancestry walks.
///
/// This is a defense-in-depth bound against corrupted/self-referential parent
/// metadata while still allowing very deep (but finite) empty tails.
const MAX_CHAIN_DEPTH: u32 = 100_000;

/// Delay between deferred retries of `ensure_candidate_available` when the
/// `BlockIdExt → RawCandidateId` mapping is not yet known.
const RESOLVER_AVAILABILITY_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Maximum number of deferred retries before giving up on resolving a
/// `BlockIdExt` to `RawCandidateId` for the resolver.
const RESOLVER_AVAILABILITY_MAX_RETRIES: u32 = 6;

/// SessionProcessor always enforces C++ `WaitForParent` readiness before dispatching validation.
///
/// Masterchain stale-parent protection remains validator-side, matching the C++ split where
/// simplex waits for parent/skip readiness and `block-validator.cpp` owns accepted-head checks.

/// Maximum number of recently-finalized blocks to show in validation section dump.
const RECENT_FINALIZED_DUMP_WINDOW: Duration = Duration::from_secs(10);

/// Observability state for stall diagnostics.
///
/// Tracks cursor-change timestamps and consensus milestone times so the dump
/// can report how long since each frontier moved and when the last cert arrived.
struct SessionObservability {
    prev_first_non_finalized: SlotIndex,
    prev_first_non_progressed: SlotIndex,
    last_finalized_cursor_change_at: SystemTime,
    last_progression_change_at: SystemTime,
    last_notarization_at: Option<SystemTime>,
    last_notarization_slot: Option<SlotIndex>,
    last_notar_cert_at: Option<SystemTime>,
    last_notar_cert_slot: Option<SlotIndex>,
    last_final_cert_at: Option<SystemTime>,
    last_final_cert_slot: Option<SlotIndex>,
    last_mc_applied_block_id: Option<BlockIdExt>,
}

impl SessionObservability {
    fn new(now: SystemTime) -> Self {
        Self {
            prev_first_non_finalized: SlotIndex(0),
            prev_first_non_progressed: SlotIndex(0),
            last_finalized_cursor_change_at: now,
            last_progression_change_at: now,
            last_notarization_at: None,
            last_notarization_slot: None,
            last_notar_cert_at: None,
            last_notar_cert_slot: None,
            last_final_cert_at: None,
            last_final_cert_slot: None,
            last_mc_applied_block_id: None,
        }
    }
}

/// Health check finding kind for structured stall diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthFindingKind {
    ZeroFinalizationSpeed,
    ProgressGap,
    LowActivity,
    StandstillTriggers,
    CandidateGiveups,
    SkipVoteDominance,
    ValidatorIsolated,
    CertVerifyFailures,
}

/// Single health check finding with severity and human-readable summary.
#[derive(Debug, Clone)]
struct HealthFinding {
    kind: HealthFindingKind,
    severity: log::Level,
    summary: String,
}

/// Lifecycle phase of a non-finalized slot for stall diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SlotWaitPhase {
    WaitingForCandidate,
    WaitingForParentBase,
    WaitingForNotarization,
    NotarizedWaitingForFinalization,
    Skipped,
    Finalized,
    TimeoutSkipped,
}

impl Display for SlotWaitPhase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaitingForCandidate => write!(f, "WaitingForCandidate"),
            Self::WaitingForParentBase => write!(f, "WaitingForParentBase"),
            Self::WaitingForNotarization => write!(f, "WaitingForNotarization"),
            Self::NotarizedWaitingForFinalization => write!(f, "NotarizedWaitFinalization"),
            Self::Skipped => write!(f, "Skipped"),
            Self::Finalized => write!(f, "Finalized"),
            Self::TimeoutSkipped => write!(f, "TimeoutSkipped"),
        }
    }
}

/// Per-slot diagnostic for non-finalized slots.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SlotDiagnostic {
    pub slot: SlotIndex,
    pub window_idx: WindowIndex,
    pub phase: SlotWaitPhase,
    pub reason: String,
    pub has_pending_block: bool,
    pub available_parent: bool,
    pub voted_notar: bool,
    pub voted_skip: bool,
    pub voted_final: bool,
    pub has_notar_cert: bool,
    pub has_final_cert: bool,
    pub has_skip_cert: bool,
    pub notar_weight_pct: f64,
    pub final_weight_pct: f64,
    pub skip_weight_pct: f64,
    pub notar_or_skip_weight_pct: f64,
    pub is_timeout_skipped: bool,
}

/// Window-level summary for dump grouping.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct WindowDiagnostic {
    pub window_idx: WindowIndex,
    pub slot_begin: SlotIndex,
    pub slot_end: SlotIndex,
    pub leader_idx: ValidatorIndex,
    pub had_timeouts: bool,
    pub slots: Vec<SlotDiagnostic>,
}

/// Candidate funnel totals for validation inventory.
struct CandidateTotals {
    received_total: usize,
    received_unvalidated: usize,
    validated_not_notarized: usize,
    notarized_not_finalized: usize,
    finalized_recent: usize,
    other_omitted: usize,
}

impl CandidateTotals {
    fn pct(&self, value: usize) -> f64 {
        if self.received_total == 0 {
            0.0
        } else {
            100.0 * value as f64 / self.received_total as f64
        }
    }
}

/// Tracks per-anomaly cooldowns and delta baselines for health alert deduplication.
/// All timestamps use `SystemTime` (via `self.now()`) for deterministic testing.
pub(crate) struct HealthAlertState {
    last_activity_warn: SystemTime,
    last_candidate_giveup_warn: SystemTime,
    last_cert_fail_warn: SystemTime,
    last_finalization_speed_warn: SystemTime,
    last_finalization_nonzero_at: SystemTime,
    last_progress_warn: SystemTime,
    last_skip_ratio_warn: SystemTime,
    last_standstill_warn: SystemTime,
    last_isolation_warn: SystemTime,
    prev_candidate_giveups: u64,
    prev_cert_verify_fails: u64,
    prev_last_finalized_slot: f64,
    prev_votes_in_notarize: u64,
    prev_votes_in_finalize: u64,
    prev_votes_in_skip: u64,
    prev_standstill_triggers: u64,
    cooldown: Duration,
}

impl HealthAlertState {
    fn new(now: SystemTime, cooldown: Duration) -> Self {
        // Prime warning timestamps in the past so first anomaly can be emitted immediately.
        let warn_base = now.checked_sub(cooldown).unwrap_or(SystemTime::UNIX_EPOCH);
        Self {
            last_activity_warn: warn_base,
            last_candidate_giveup_warn: warn_base,
            last_cert_fail_warn: warn_base,
            last_finalization_speed_warn: warn_base,
            last_finalization_nonzero_at: now,
            last_progress_warn: warn_base,
            last_skip_ratio_warn: warn_base,
            last_standstill_warn: warn_base,
            last_isolation_warn: warn_base,
            prev_candidate_giveups: 0,
            prev_cert_verify_fails: 0,
            prev_last_finalized_slot: 0.0,
            prev_votes_in_notarize: 0,
            prev_votes_in_finalize: 0,
            prev_votes_in_skip: 0,
            prev_standstill_triggers: 0,
            cooldown,
        }
    }
}

/*
    Async request implementation
    Reference: validator-session/src/session_processor.rs AsyncRequestImpl
*/
use crate::AsyncRequest;

/// Async request implementation for tracking collation requests
struct AsyncRequestImpl {
    /// Request identifier
    request_id: u32,
    /// Time when request was created
    creation_time: SystemTime,
    /// Flag indicating request was cancelled
    cancelled: Arc<AtomicBool>,
    /// Whether to cancel on drop
    cancel_on_drop: bool,
}

impl AsyncRequestImpl {
    fn new(request_id: u32, cancel_on_drop: bool, creation_time: SystemTime) -> Arc<Self> {
        Arc::new(Self {
            request_id,
            creation_time,
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_on_drop,
        })
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl AsyncRequest for AsyncRequestImpl {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn get_request_id(&self) -> u32 {
        self.request_id
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn get_creation_time(&self) -> SystemTime {
        self.creation_time
    }
}

impl Drop for AsyncRequestImpl {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

/*
    Precollated block
    Reference: validator-session/src/session_processor.rs PrecollatedBlock
*/

use crate::ValidatorBlockCandidatePtr;

/// Precollated block - stores pending or completed collation result
///
/// Parent is captured at collation start to avoid races between collation
/// and consensus events (e.g., notarization advancing the parent chain).
///
/// Reference: C++ block-producer.cpp locks parent in the collation loop.
struct PrecollatedBlock {
    /// Request for tracking/cancellation
    request: Arc<AsyncRequestImpl>,
    /// Completed collation result - None if still pending
    result: Option<CollationResult>,
    /// Parent captured at collation start (avoids race vs consensus events)
    ///
    /// This is the parent that was available when collation was initiated.
    /// We use this in `generated_block()` instead of recomputing from FSM state,
    /// ensuring the candidate is signed with the same parent that was assumed.
    parent: Option<crate::block::CandidateParentInfo>,
}

/// Map of slot -> precollated block
type PrecollatedBlockMap = HashMap<SlotIndex, PrecollatedBlock>;

/// Window-local chain head for candidate chaining (C++ block-producer.cpp parity).
///
/// In C++, `generate_candidates()` carries mutable `parent` and `state` across slots
/// within a single leader window, so slot N+1 chains off slot N's locally generated
/// candidate without waiting for notarization. This struct tracks the same chain head
/// on the Rust side: after `generated_block()` completes for a slot, we record the
/// produced candidate's identity here so that `precollate_block()` for the next slot
/// in the same window can use it immediately as an explicit parent.
///
/// Reset when the leader window changes or the progress cursor jumps.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct LocalChainHead {
    /// Window this chain head belongs to
    window: WindowIndex,
    /// Slot of the last locally generated candidate
    slot: SlotIndex,
    /// Candidate parent info (slot + candidate-id hash) for the next slot
    parent_info: crate::block::CandidateParentInfo,
    /// Resolved BlockIdExt of the generated candidate (for seqno derivation and explicit parent hint)
    block_id: BlockIdExt,
    /// Exact generation time extracted from ConsensusExtraData, if available.
    gen_utime_ms: Option<u64>,
}

struct PreparedCollation {
    prev_block_ids: Vec<BlockIdExt>,
    slot_start_time: SystemTime,
    new_seqno: u32,
    is_first_session_block: bool,
}

enum CollationPreparation {
    Ready(PreparedCollation),
    Deferred(SystemTime),
    WaitingForParent,
}

#[derive(Clone, Copy)]
enum CollationAttempt {
    Initial,
    Retry { retry_count: u32 },
}

impl CollationAttempt {
    fn log_context(self) -> &'static str {
        match self {
            Self::Initial => "invoke_collation",
            Self::Retry { .. } => "invoke_collation_retry",
        }
    }

    fn failure_retry_count(self) -> u32 {
        match self {
            Self::Initial => 0,
            Self::Retry { retry_count } => retry_count,
        }
    }
}

/*
    Collation result

    Represents the outcome of a collation request - either a normal block
    with transactions or an empty block for finalization recovery.
*/

/// Result of block collation
///
/// Used to differentiate between normal blocks (with transactions) and
/// empty blocks (for finalization recovery when consensus gets ahead).
///
/// Reference: C++ block-producer.cpp generate_candidates() loop
#[derive(Clone)]
enum CollationResult {
    /// Normal block with candidate data from collator
    Block(ValidatorBlockCandidatePtr),

    /// Empty block for finalization recovery
    ///
    /// When consensus gets ahead of blockchain finalization, we generate
    /// an "empty" block that references the parent's BlockIdExt instead of
    /// collating new transactions. This helps the previous block get finalized.
    ///
    /// Contains the parent's BlockIdExt to inherit.
    Empty {
        /// Parent block identifier (empty block inherits this)
        parent_block_id: BlockIdExt,
    },
}

/// Generated block descriptor for broadcast and FSM submission
///
/// Contains all computed data needed after validation, signing, and TL construction.
/// Used by both `create_normal_block_desc` and `create_empty_block_desc`.
struct GeneratedBlockDesc {
    /// Block identifier (for FSM CandidateId)
    block_id_ext: BlockIdExt,
    /// Block candidate for FSM (None for empty blocks)
    block_candidate: Option<crate::block::BlockCandidate>,
    /// Candidate hash (used in FSM CandidateId)
    candidate_hash: UInt256,
    /// TL candidate data for network broadcast
    tl_candidate_data: CandidateData,
    /// Signature for FSM Candidate
    signature: Vec<u8>,
    /// Exact generation time extracted from ConsensusExtraData, if available.
    gen_utime_ms: Option<u64>,
}

/*
    Delayed action
*/

/// Delayed action with expiration time
///
/// Used to schedule future operations like collation retries, validation retries, etc.
/// Reference: validator-session/src/session_processor.rs DelayedAction
struct DelayedAction {
    /// Time when action should be executed
    expiration_time: SystemTime,
    /// Handler closure to execute
    handler: TaskPtr,
}

/// Validated block candidate for finalization
///
/// Contains validated candidate data stored after successful validation.
/// Note: Currently stored but not used - we use received_candidates for finalization
#[derive(Debug)]
#[allow(dead_code)]
struct ValidatedCandidate {
    /// Source validator index (leader)
    source_idx: ValidatorIndex,
    /// Root hash of the block
    root_hash: UInt256,
    /// File hash of the block
    file_hash: UInt256,
    /// Block data (serialized)
    data: crate::BlockPayloadPtr,
}

/// Received block candidate
///
/// All block candidates received from the network are stored here.
/// Used for finalization - we always use this map to get candidate data.
/// Reference: validator-session/src/session_processor.rs blocks field
#[derive(Clone)]
struct ReceivedCandidate {
    /// Slot number (informational; the first slot where this candidate appeared)
    #[allow(dead_code)]
    slot: SlotIndex,
    /// Source validator index (leader)
    source_idx: ValidatorIndex,
    /// Candidate ID hash (from RawCandidateId.hash)
    /// This is computed from TL candidateHashData, NOT the block's root_hash
    /// Used for matching parent references during candidate metadata lookups
    #[allow(dead_code)] // May be used for debugging/diagnostics
    candidate_id_hash: UInt256,
    /// Serialized CandidateHashData TL bytes
    /// Used for building BlockSignaturesSimplex during finalization delivery
    /// SHA256(candidate_hash_data_bytes) == candidate_id_hash
    candidate_hash_data_bytes: Vec<u8>,
    /// Full block ID (workchain, shard, seqno, root_hash, file_hash)
    /// Used for seqno validation during batch finalization
    block_id: BlockIdExt,
    /// Root hash of the block
    root_hash: UInt256,
    /// File hash of the block
    file_hash: UInt256,
    /// Actual block data (extracted from TL, ready for callback)
    /// For non-empty blocks: BlockCandidate.data
    /// For empty blocks: empty vec
    data: crate::BlockPayloadPtr,
    /// Collated data (extracted from TL)
    #[allow(dead_code)]
    collated_data: crate::BlockPayloadPtr,
    /// Exact generation time extracted from ConsensusExtraData, if available.
    gen_utime_ms: Option<u64>,
    /// Time when candidate was received (for latency tracking)
    #[allow(dead_code)]
    receive_time: SystemTime,
    /// True if this is an empty block (inherits parent's BlockIdExt)
    is_empty: bool,
    /// Parent candidate ID (None for genesis/first in epoch).
    /// Used for empty-parent tip checks, explicit-parent collation hints, and
    /// restart-seeded metadata lookups.
    parent_id: Option<crate::block::RawCandidateId>,
}

/// Pending validation entry
///
/// Tracks a block candidate that has been received from the network
/// and is awaiting validation from the higher layer.
#[derive(Debug)]
struct PendingValidation {
    /// The raw candidate (signature already verified)
    raw_candidate: RawCandidate,
    /// Slot number
    slot: SlotIndex,
    /// Time when candidate was received
    receive_time: SystemTime,
    /// Source validator index
    source_idx: ValidatorIndex,
}

/// Tracks a locally generated candidate until it validates successfully.
#[derive(Debug)]
struct GeneratedCandidateValidationWatch {
    /// When the local candidate was generated.
    generated_at: SystemTime,
    /// Whether it has already entered higher-layer validation.
    validation_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum McValidationReadiness {
    Ready,
    WaitingForAcceptedHead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WaitForParentDecision {
    Ready,
    Wait,
    Reject(String),
}

/*
    Finalization Journal

    Tracks finalized blocks that cannot be fully materialized yet because the
    candidate body has not arrived. Once the body is known, we can deliver the
    finalized callback, persist restart state, and clear local slot runtime.
*/

/// Finalization journal entry
///
/// Records that a FinalCert was observed for (slot, hash), but we haven't
/// materialized local finalized state yet because the candidate body is still missing.
#[derive(Clone)]
struct FinalizedEntry {
    /// The finalization event from SimplexState
    event: BlockFinalizedEvent,
    /// Time when finalization was first observed (for timeout / diagnostics)
    #[allow(dead_code)]
    finalized_at: SystemTime,
}

/*
    Slot runtime + outcome gating (future wiring)

    This is infrastructure for:
    - per-slot runtime state (instead of "global state reset per slot"), and
    - "mark ready" then "emit when contiguous" semantics.
*/

/// Per-slot runtime state (optional: exists only if the slot had local activity).
///
/// Contains per-slot state for collation, timing, and stage tracking.
#[derive(Debug)]
struct SlotRuntime {
    // Collation state
    slot_started_at: SystemTime,
    pending_generate: bool,
    generated: bool,
    sent_generated: bool,

    // Validated block candidate data for finalization callback (slot → data)
    validated_candidate_data: Option<ValidatedCandidate>,

    // Slot stage flags (for latency metrics)
    first_candidate_received: bool,
    first_candidate_notarized: bool,
    first_candidate_finalized: bool,
}

impl SlotRuntime {
    fn new(now: SystemTime) -> Self {
        Self {
            slot_started_at: now,
            pending_generate: false,
            generated: false,
            sent_generated: false,
            validated_candidate_data: None,
            first_candidate_received: false,
            first_candidate_notarized: false,
            first_candidate_finalized: false,
        }
    }
}

#[derive(Debug, Default)]
struct SlotEntry {
    runtime: Option<SlotRuntime>,
}

impl SlotEntry {
    /// Returns `true` if this slot has local `generated=true`, else `false`.
    fn is_generated(&self) -> bool {
        self.runtime.as_ref().map_or(false, |rt| rt.generated)
    }
}

/*
    SessionProcessor
*/

/// Session processor for Simplex consensus
///
/// Contains the core consensus algorithm. All operations are single-threaded.
/// Based on validator-session SessionProcessor pattern.
pub(crate) struct SessionProcessor {
    /// Session description (validators, weights, options, identity)
    /// Contains all immutable session-level configuration including session_id,
    /// local_key, initial_block_seqno, session_creation_time.
    description: Arc<SessionDescription>,
    /// Explicit session start parents from ValidatorGroup.
    ///
    /// `SimplexState` still models the first slot as "session base" internally
    /// (`parent=None`), but all validator-facing collation requests must remap
    /// that sentinel back to these concrete start parents.
    session_start_prev_blocks: Vec<BlockIdExt>,
    /// Task queue for main processing
    task_queue: TaskQueuePtr,
    /// Task queue for callbacks
    callbacks_task_queue: CallbackTaskQueuePtr,
    /// Session listener (weak reference)
    listener: SessionListenerPtr,
    /// Next awake time for timer-based processing
    next_awake_time: SystemTime,
    /// Overlay manager for network communication
    #[allow(dead_code)]
    overlay_manager: ConsensusOverlayManagerPtr,
    /// Receiver for network communication
    receiver: ReceiverPtr,
    /// Stop flag shared with Session main loop.
    ///
    /// Used to suppress late callbacks/finalizations during session shutdown (validator-group compatibility).
    stop_flag: Arc<AtomicBool>,
    /// Simplex database for persistent storage
    db: SimplexDbPtr,
    /// First leader window index that has not been announced yet (persisted as poolState)
    ///
    /// C++: `SimplexPoolImpl::first_nonannounced_window_`
    first_nonannounced_window: WindowIndex,
    /// CandidateInfo DB writes in-flight/completed (for WaitCandidateInfoStored parity)
    candidate_info_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,
    /// NotarCert DB writes in-flight/completed (for WaitCandidateInfoStored parity)
    notar_cert_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,
    /// FinalCert DB writes in-flight/completed.
    final_cert_store_results: HashMap<RawCandidateId, StorageAsyncResultPtr<()>>,
    /// SkipCert DB writes in-flight/completed.
    skip_cert_store_results: HashMap<SlotIndex, StorageAsyncResultPtr<()>>,
    /// Use separate callback thread for session callbacks
    use_callback_thread: bool,
    /// Current active weight from receiver
    active_weight: ValidatorWeight,
    /// Last activity time per validator (from receiver)
    /// Updated periodically via on_activity callback
    last_activity: Vec<Option<SystemTime>>,
    /// List of delayed actions to execute in the future
    delayed_actions: Vec<DelayedAction>,
    /// SimplexState FSM - core consensus state machine
    simplex_state: SimplexState,
    /// Slots for which "missing body" has already been logged (throttle).
    /// Prevents multi-million-line log floods when a slot body never arrives.
    missing_body_logged: HashSet<u32>,

    /*
        Collation state (session-level only)

        Per-slot collation state (pending_generate, generated, sent_generated, slot_started_at)
        is now in SlotRuntime, accessed via slot-aware accessors.
    */
    /// Precollated blocks map (slot -> block)
    precollated_blocks: PrecollatedBlockMap,
    /// Next request ID for precollation
    precollated_blocks_next_request_id: u32,
    /// Max slot in precollation pipeline
    precollated_blocks_max_slot: Option<SlotIndex>,
    /// Earliest wall-clock time when the next collation start is allowed.
    /// Set to `now + target_rate` when a collation is initiated (or a
    /// precollated block is consumed). Checked at the top of `check_collation`.
    /// Reference: C++ block-producer.cpp coro_sleep(target_time)
    earliest_collation_time: Option<SystemTime>,
    /// Window-local chain head for candidate chaining across slots in the same
    /// leader window (C++ block-producer.cpp parity). Updated synchronously in
    /// `generated_block()` so that `precollate_block()` can chain the next slot
    /// without waiting for the async `on_candidate_received` self-loop.
    local_chain_head: Option<LocalChainHead>,
    /// Synchronous cache of locally generated parent metadata, keyed by
    /// `RawCandidateId`. Populated in `generated_block()` *before* the async
    /// `on_candidate_received` self-loop, so `resolve_parent_block_id()` can
    /// find the parent immediately for chained precollation.
    generated_parent_cache: HashMap<RawCandidateId, BlockIdExt>,
    /// Exact generation timestamps for locally generated parents before the
    /// async self-receive path populates `received_candidates`.
    generated_parent_gen_utime_ms_cache: HashMap<RawCandidateId, u64>,
    /// Locally generated candidates that have not yet validated successfully.
    ///
    /// Used to surface warnings and metrics when the self-loop or validation
    /// pipeline drops our own candidate before it reaches a successful
    /// `candidate_decision_ok_internal()` outcome.
    generated_candidates_waiting_validation:
        HashMap<RawCandidateId, GeneratedCandidateValidationWatch>,
    /// Self-collation flows currently in progress, keyed by slot.
    ///
    /// Tracks `(started_at, expected_seqno)` for the FIRST `on_generate_slot()`
    /// dispatch on this slot. Retry attempts collapse onto the same entry, so
    /// the whole collation flow (initial + retries) is counted as ONE collation.
    /// Removed when the flow ends — either by success (moved to
    /// `self_collation_pending_acceptance`), by final failure (no more retries),
    /// or by an ignore path (e.g. cancelled callback / pipeline reset).
    self_collation_starts_by_slot: HashMap<SlotIndex, (SystemTime, u32)>,
    /// Self-collations whose candidate has been generated and is waiting for
    /// `notify_block_finalized` to fire on `(slot, hash)`. Carries
    /// `(started_at, expected_seqno)` so acceptance latency is measured from
    /// the original `on_generate_slot()` dispatch.
    self_collation_pending_acceptance: HashMap<RawCandidateId, (SystemTime, u32)>,

    /*
        Validation state
        Reference: validator-session/src/session_processor.rs validation fields

        Collections with RawCandidateId keys are session-level since RawCandidateId
        already contains slot info. They are cleaned up in cleanup_old_slots().
    */
    /// Pending validations: block candidates received from network awaiting validation
    /// Maps RawCandidateId(slot, hash) → PendingValidation
    pending_validations: HashMap<RawCandidateId, PendingValidation>,
    /// Set of blocks that are currently being validated (awaiting callback)
    pending_approve: HashSet<RawCandidateId>,
    /// Map of blocks pending rejection (with rejection reason)
    pending_reject: HashMap<RawCandidateId, crate::BlockPayloadPtr>,
    /// Set of rejected blocks
    rejected: HashSet<RawCandidateId>,
    /// Map of approved blocks: RawCandidateId → (validity_start_time, signature)
    approved: HashMap<RawCandidateId, (SystemTime, crate::BlockPayloadPtr)>,
    /// Map block hash to validation attempt index (NOT reset per slot)
    validation_attempt_map: HashMap<RawCandidateId, u32>,
    /// Candidates that have been validated and are ready for FSM
    validated_candidates: VecDeque<Candidate>,
    /// All received block candidates: RawCandidateId(slot, candidate_id_hash) → candidate data
    received_candidates: HashMap<RawCandidateId, ReceivedCandidate>,
    /// Serialized CandidateData bytes cache for RequestCandidate query fallback.
    ///
    /// Populated in `on_candidate_received()` by re-serializing the TL `CandidateData` object.
    /// Used by `handle_candidate_query_fallback()` when the receiver's `resolver_cache` misses.
    /// This provides C++ parity with `CandidateResolver::try_load_candidate_data_from_db()`.
    candidate_data_cache: HashMap<RawCandidateId, Vec<u8>>,
    /// Broadcast ingress dedup: slot -> first candidate seen via broadcast path.
    ///
    /// Matches the spirit of C++ `PrecheckCandidateBroadcast` slot-level conflict
    /// guard by rejecting a second conflicting candidate id for the same slot.
    seen_broadcast_candidates: HashMap<SlotIndex, RawCandidateId>,

    /*
        Metrics

        Infrastructure for tracking consensus performance.
        Individual metrics will be added as flows are implemented.
        Reference: validator-session/src/session_processor.rs metrics fields
    */
    /// Metrics receiver for creating metrics
    metrics_receiver: MetricsHandle,
    /// Counter for check_all() calls
    check_all_counter: metrics::Counter,
    /// Counter for process_simplex_events() calls
    process_events_counter: metrics::Counter,
    /// Histogram for slot duration (time from slot start to finalization)
    slot_duration_histogram: metrics::Histogram,
    /// Histogram for validation latency (time to validate a block candidate)
    validation_latency_histogram: metrics::Histogram,
    /// Histogram for collation latency (time to generate a block)
    collation_latency_histogram: metrics::Histogram,
    /// Histogram for how late `check_all()` runs relative to its scheduled wake.
    check_all_wake_slip_histogram: metrics::Histogram,
    /// Gauge for current active weight from network
    active_weight_gauge: metrics::Gauge,
    /// Result status counter for validation requests
    validates_counter: ResultStatusCounter,
    /// Result status counter for collation requests
    collates_counter: ResultStatusCounter,
    /// Result status counter for self-collation attempts.
    self_collates_counter: ResultStatusCounter,
    /// Counter for precollation requests
    precollation_requests_counter: metrics::Counter,
    /// Counter for precollation results
    precollation_results_counter: metrics::Counter,
    /// Counter for precollated block hits
    collates_precollated_counter: ResultStatusCounter,
    /// Result status counter for expired collation time slots
    collates_expire_counter: ResultStatusCounter,
    /// Counter for all collation entry attempts, including async, retry, precollated,
    /// and empty-block fast paths.
    collation_starts_counter: metrics::Counter,
    /// Histogram for self-collation acceptance latency (request start to
    /// finalized callback emission).
    self_collation_accept_latency_histogram: metrics::Histogram,
    /// Histogram for broadcast-to-validation complete latency
    broadcast_validation_latency_histogram: metrics::Histogram,
    /// Counter for errors during session (for SessionStats)
    errors_counter: metrics::Counter,
    /// Gauge for finalized blocks still waiting for candidate body arrival
    finalized_pending_body_gauge: metrics::Gauge,

    /*
        Error tracking
    */
    /// Total errors count during this session (used for session statistics)
    /// Atomic to allow increment_error(&self) without requiring &mut self
    session_errors_count: AtomicU32,

    /*
        Slot stage tracking (for latency histograms)

        Per-slot stage flags (first_candidate_received/notarized/finalized) are now
        in SlotRuntime, accessed via slot-aware accessors.
        Histograms remain session-level.
    */
    /// Histogram for first candidate received latency
    first_candidate_received_latency_histogram: metrics::Histogram,
    /// Histogram for first candidate notarized latency
    first_candidate_notarized_latency_histogram: metrics::Histogram,
    /// Histogram for first candidate finalized latency
    first_candidate_finalized_latency_histogram: metrics::Histogram,

    /*
        Debug
        Reference: validator-session/src/session_processor.rs round_debug_at
    */
    /// Next time for stalled round debug dump (reset on each finalization)
    /// If current time >= round_debug_at, no finalizations occurred for ROUND_DEBUG_PERIOD
    round_debug_at: SystemTime,
    /// Time of last finalization (for accurate stall duration reporting)
    last_finalization_time: SystemTime,

    /*
        Slot Sequence Invariants
        Ensures correct ordering of slots for finalizations, skips, and generation
    */
    /// Last slot for which generation was requested
    /// Must be monotonically increasing (gaps allowed)
    last_generated_slot: Option<SlotIndex>,

    /*
        Block SeqNo Tracking
        Tracks expected blockchain sequence number for next block
    */
    /// Highest finalized non-empty block seqno materialized locally in this session.
    /// Used for validation shortcuts and local progress tracking.
    finalized_head_seqno: Option<u32>,

    /// Slot of the latest locally materialized finalized head.
    /// Used to retrieve parent `BlockIdExt` for empty block generation.
    finalized_head_slot: Option<SlotIndex>,
    /// Last finalized non-empty block id (parent for empty blocks)
    ///
    /// Empty blocks inherit parent's BlockIdExt (C++ behavior), so we must keep the
    /// last non-empty finalized block id available for empty block generation.
    finalized_head_block_id: Option<BlockIdExt>,

    /// Last finalized head block's before_split flag (for split/merge handling).
    ///
    /// Kept as a fallback when parent-specific `before_split` metadata is unavailable.
    /// Primary empty-policy source is now parent-derived (`resolve_parent_before_split_flag`).
    ///
    /// Reference: C++ block-producer.cpp `is_before_split()` + `should_generate_empty_block()`
    finalized_head_before_split: bool,

    /// Observed `before_split` values for non-empty block ids.
    ///
    /// This is used to evaluate empty-block policy from the current collation parent
    /// (including parent chains with empty candidates that all reference the same
    /// normal-tip `BlockIdExt`), matching C++ `state->is_before_split()` semantics.
    before_split_by_block_id: HashMap<BlockIdExt, bool>,

    /// Last consensus-finalized seqno tracked by the producer-side state for this session.
    ///
    /// Tracks the highest seqno considered finalized for empty-block generation on
    /// masterchain sessions.
    ///
    /// Updated by:
    /// - local finalized delivery paths (`maybe_apply_finalized_state()`, recovery, etc.), and
    /// - manager-applied top notifications via `set_mc_finalized_block()`, mirroring
    ///   C++ `block-producer.cpp::handle(BlockFinalizedInMasterchain)`.
    last_consensus_finalized_seqno: Option<u32>,

    /// Blocks whose finalized state has already been materialized locally.
    ///
    /// Used to deduplicate repeated finalization events and late body arrivals,
    /// avoiding duplicate DB writes and repeated finalized bookkeeping.
    ///
    /// Cleaned up in cleanup_old_slots() for slots older than MAX_HISTORY_SLOTS.
    finalized_blocks: HashSet<RawCandidateId>,

    /*
        Finalized Pending Body

        Tracks finalized blocks (FinalCert observed) whose candidate body has not
        yet been received locally. Materialization is deferred until the
        corresponding candidate body arrives.

        Two materialization triggers:
        - BlockFinalizedEvent: records entry + applies immediately when body is present
        - on_candidate_received: late body arrival triggers deferred finalization
    */
    /// Finalized blocks still waiting for candidate body arrival
    ///
    /// Keyed by RawCandidateId = { slot, hash }
    /// Inserted when BlockFinalizedEvent arrives (even if body missing)
    /// Removed when body arrives and finalization is applied, or cleaned up (old slots)
    finalized_pending_body: HashMap<RawCandidateId, FinalizedEntry>,

    /*
        Slot outcome emission gating (future wiring)

        Tracks per-slot runtime state (SlotRuntime) for collation/validation.
    */
    /// Per-slot state map (slot -> entry).
    ///
    /// Uses BTreeMap so iterating in slot order is natural and stable.
    /// Used for SlotRuntime tracking (collation/validation state per slot).
    slots: BTreeMap<SlotIndex, SlotEntry>,

    /*
        ========================================================================
        Empty Block Support (TON-specific extension for finalization recovery)

        Reference: C++ block-producer.cpp should_generate_empty_block()
        ========================================================================
    */
    /// Last MC-registered top seqno for this shard session (for empty block decisions)
    ///
    /// Updated via `set_mc_finalized_block()` when manager notifications arrive.
    /// The value is the current applied-top seqno for this session shard.
    /// Used by `should_generate_empty_block()` for shardchain sessions.
    /// Masterchain sessions also receive manager-applied top updates here so startup and
    /// recovery share the same monotonic applied-top cursor.
    last_mc_finalized_seqno: Option<u32>,
    /// Seqno fallback for the accepted normal head used by MC validation ordering.
    ///
    /// Seeded from `initial_block_seqno - 1`, then advanced by applied-top notifications,
    /// restart recovery, and finalized non-empty blocks.
    accepted_normal_head_seqno: u32,
    /// Exact accepted normal head when known.
    ///
    /// This mirrors the C++ `last_accepted_block_` semantics closely enough to reject
    /// stale same-seqno forks once an exact BlockIdExt has been observed.
    accepted_normal_head_block_id: Option<BlockIdExt>,

    /*
        ========================================================================
        Candidate Request Tracking (Block Repair)

        Tracks pending candidate requests to avoid duplicate requests and
        implement delayed request logic (wait for broadcast before querying).
        ========================================================================
    */
    /// Candidates we've requested from peers: RawCandidateId(slot, hash)
    ///
    /// When a BlockFinalized event arrives but the candidate is missing,
    /// we add the key here and schedule a delayed action. After the delay,
    /// if the candidate is still missing (not in received_candidates),
    /// we call receiver.request_candidate().
    /// Candidate request throttling: (slot, hash) → next allowed request time.
    requested_candidates: HashMap<RawCandidateId, SystemTime>,

    /*
        ========================================================================
        Misbehavior Tracking

        Collects misbehavior reports for validators that violate protocol rules.

        Reference: C++ bus.h MisbehaviorReport
        ========================================================================
    */
    /// Dedup set for finalized delivery.
    ///
    /// We emit at most once per finalized candidate id, even if:
    /// - finalization is observed before candidate body (delayed emit path), or
    /// - repeated finalization/candidate events arrive from network.
    finalized_delivery_sent: HashSet<RawCandidateId>,
    /// Seqno-level finalized callback tracking.
    ///
    /// Protocol invariant: a session must never emit more than one
    /// `on_block_finalized()` callback for the same block seqno.
    /// Any second attempt is treated as a protocol breach and asserted.
    finalized_delivery_sent_seqno: HashMap<u32, BlockIdExt>,

    /// Collected misbehavior reports from this session
    ///
    /// When a vote is detected as misbehavior (e.g., conflicting votes for same slot),
    /// a report is created and stored here for potential future downstream processing.
    ///
    misbehavior_reports: Vec<MisbehaviorReport>,
    /// Counter for detected misbehavior events
    misbehavior_counter: metrics::Counter,

    /// Gauge: last finalized slot index (set on each finalization)
    last_finalized_slot_gauge: metrics::Gauge,
    /// Gauge: first non-finalized slot from FSM (set in check_all)
    first_non_finalized_slot_gauge: metrics::Gauge,
    /// Gauge: first non-progressed slot from FSM (set in check_all)
    first_non_progressed_slot_gauge: metrics::Gauge,
    /// Counter: total skip events
    skip_total_counter: metrics::Counter,
    /// Vote pipeline counters (in)
    votes_in_total_counter: metrics::Counter,
    votes_in_notarize_counter: metrics::Counter,
    votes_in_finalize_counter: metrics::Counter,
    votes_in_skip_counter: metrics::Counter,
    /// Vote pipeline counters (out)
    votes_out_total_counter: metrics::Counter,
    votes_out_notarize_counter: metrics::Counter,
    votes_out_finalize_counter: metrics::Counter,
    votes_out_skip_counter: metrics::Counter,
    /// Outgoing votes aborted because required persistence confirmation failed.
    votes_out_persist_fail_counter: metrics::Counter,
    /// Local vote totals for health anomaly delta checks (inbound stream).
    votes_in_notarize_total: u64,
    votes_in_finalize_total: u64,
    votes_in_skip_total: u64,
    /// Certificate counters
    certs_in_counter: metrics::Counter,
    certs_relayed_counter: metrics::Counter,
    cert_conflict_counter: metrics::Counter,
    cert_verify_fail_counter: metrics::Counter,
    /// Validation quality counters
    validation_reject_counter: metrics::Counter,
    validation_late_callback_counter: metrics::Counter,
    /// Health warnings counter (separate from session_errors_count)
    health_warnings_counter: metrics::Counter,
    /// Broadcast precheck drops: old slot (< first_non_finalized)
    candidate_precheck_old_slot_drop_counter: metrics::Counter,
    /// Broadcast precheck drops: too-far-future slot
    candidate_precheck_future_slot_drop_counter: metrics::Counter,
    /// Broadcast precheck drops: sender is not expected slot leader
    candidate_precheck_unexpected_sender_drop_counter: metrics::Counter,
    /// Broadcast precheck drops: conflicting second candidate for same slot
    candidate_precheck_conflicting_slot_drop_counter: metrics::Counter,
    /// Peer-delivered candidate bodies received via broadcast.
    /// Excludes the local generated-block self-loop.
    candidate_received_broadcast_counter: metrics::Counter,
    /// Peer-delivered candidate bodies received via requestCandidate/query responses.
    /// Excludes the local generated-block self-loop.
    candidate_received_query_counter: metrics::Counter,
    /// Locally generated candidates that failed to validate successfully.
    generated_candidate_validation_missed_counter: metrics::Counter,
    /// Health alert state for cooldown-based anomaly detection
    pub(crate) health_alert_state: HealthAlertState,
    /// Shared health counters from receiver (standstill triggers, candidate giveups)
    pub(crate) receiver_health_counters: Arc<crate::receiver::ReceiverHealthCounters>,
    /// Local cert verify fail total (for delta-based anomaly detection)
    pub(crate) cert_verify_fails_total: u64,
    /// Observability state for stall diagnostics (cursor ages, milestone timestamps)
    observability: SessionObservability,
    /// Latest receiver activity snapshot (updated periodically via on_activity)
    last_receiver_snapshot: Option<crate::receiver::ReceiverActivitySnapshot>,
}

impl SessionProcessor {
    /// Current session time (real-time or manually overridden for tests/log replay).
    ///
    /// IMPORTANT: SessionProcessor must not call `SystemTime::now()` directly.
    /// All time access goes through `SessionDescription::get_time()` so tests can
    /// deterministically control time.
    #[inline]
    fn now(&self) -> SystemTime {
        self.description.get_time()
    }

    /// Override session time (used for tests / log replay).
    #[allow(dead_code)]
    pub(crate) fn set_time(&self, time: SystemTime) {
        self.description.set_time(time);
    }

    /// Advance session time by a duration (used for tests).
    #[allow(dead_code)]
    pub(crate) fn advance_time(&self, delta: Duration) {
        self.description.set_time(self.now() + delta);
    }

    #[inline]
    fn record_candidate_ingress(&self, sender_idx: ValidatorIndex, is_broadcast_candidate: bool) {
        // Keep ingress counters focused on peer-delivered traffic: locally generated
        // blocks loop back through on_candidate_received() but are not network ingress.
        if sender_idx == self.description.get_self_idx() {
            return;
        }

        if is_broadcast_candidate {
            self.candidate_received_broadcast_counter.increment(1);
        } else {
            self.candidate_received_query_counter.increment(1);
        }
    }

    #[inline]
    fn record_collation_start(&self) {
        self.collation_starts_counter.increment(1);
    }

    #[inline]
    fn collation_attempt_label(attempt: CollationAttempt) -> String {
        match attempt {
            CollationAttempt::Initial => "initial".to_string(),
            CollationAttempt::Retry { retry_count } => format!("retry-{retry_count}"),
        }
    }

    /// Format the linkage key used to grep `COLLATION_FLOW` log lines together.
    ///
    /// Self-collation flows do not yet have a final `BlockIdExt` (root_hash/file_hash
    /// are produced by collation), so we synthesize an "expected block id" of the form
    /// `<workchain>:<shard>:<expected_seqno>` from the session shard and the seqno
    /// the FSM is asking us to produce. This is the linkage handle used in every
    /// `COLLATION_FLOW` log message.
    fn format_expected_block_id(&self, expected_seqno: u32) -> String {
        format!("{}:{}", self.description.get_shard(), expected_seqno)
    }

    /// Record the start of a self-collation flow triggered by an `on_generate_slot()`
    /// dispatch.
    ///
    /// The whole flow (initial attempt + any retries) counts as ONE self-collation:
    /// the metric counter is incremented only on `CollationAttempt::Initial`. Retries
    /// keep the same start time so latency reflects end-to-end time, not just the
    /// last attempt's time.
    ///
    /// Empty blocks created internally by the simplex layer (without going through
    /// `on_generate_slot()`) MUST NOT call this — they are not self-collations.
    fn record_self_collation_start(
        &mut self,
        slot: SlotIndex,
        expected_seqno: u32,
        attempt: CollationAttempt,
        parent: Option<&crate::block::CandidateParentInfo>,
        prev_block_ids: &[BlockIdExt],
    ) {
        let expected_block_id = self.format_expected_block_id(expected_seqno);
        let parent_label = parent
            .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
            .unwrap_or_else(|| "none".to_string());
        let prevs_label = if prev_block_ids.is_empty() {
            "none".to_string()
        } else {
            prev_block_ids
                .iter()
                .map(|id| format!("{}:{}", id.seq_no, &id.root_hash.to_hex_string()[..8]))
                .collect::<Vec<_>>()
                .join(",")
        };

        match attempt {
            CollationAttempt::Initial => {
                let started_at = self.now();
                self.self_collation_starts_by_slot.insert(slot, (started_at, expected_seqno));
                self.self_collates_counter.total_increment();
                log::info!(
                    "Session {} COLLATION_FLOW start: expected_block_id={} slot={} \
                    expected_seqno={} attempt=initial parent={} prevs={}",
                    &self.session_id().to_hex_string()[..8],
                    expected_block_id,
                    slot,
                    expected_seqno,
                    parent_label,
                    prevs_label,
                );
            }
            CollationAttempt::Retry { retry_count } => {
                log::info!(
                    "Session {} COLLATION_FLOW retry: expected_block_id={} slot={} \
                    expected_seqno={} attempt=retry-{} parent={} prevs={}",
                    &self.session_id().to_hex_string()[..8],
                    expected_block_id,
                    slot,
                    expected_seqno,
                    retry_count,
                    parent_label,
                    prevs_label,
                );
            }
        }
    }

    /// Log that the collator produced a candidate for this slot.
    ///
    /// This is NOT a terminal event for the metric — the success counter is
    /// incremented later in `record_self_collation_acceptance()` once
    /// `notify_block_finalized` fires. We only log here so the flow can be linked
    /// in logs by `expected_block_id`.
    fn record_self_collation_generated(&self, slot: SlotIndex, outcome: &str) {
        let Some((started_at, expected_seqno)) = self.self_collation_starts_by_slot.get(&slot)
        else {
            return;
        };
        let elapsed_ms = self.now().duration_since(*started_at).unwrap_or_default().as_millis();
        let expected_block_id = self.format_expected_block_id(*expected_seqno);

        log::info!(
            "Session {} COLLATION_FLOW generated: expected_block_id={} slot={} \
            expected_seqno={} outcome={} generation_ms={}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            expected_seqno,
            outcome,
            elapsed_ms,
        );
    }

    /// Record a TERMINAL failure of the self-collation flow (no more retries).
    ///
    /// Called only when collation attempts have ended without producing an accepted
    /// block — either max retries reached, slot has progressed past, or our generated
    /// candidate was rejected before reaching the finalized callback.
    /// Intermediate per-attempt failures that will be retried do NOT call this.
    fn record_self_collation_final_failure(&mut self, slot: SlotIndex, reason: &str) {
        let Some((started_at, expected_seqno)) = self.self_collation_starts_by_slot.remove(&slot)
        else {
            return;
        };

        self.self_collates_counter.failure();

        let elapsed_ms = self.now().duration_since(started_at).unwrap_or_default().as_millis();
        let expected_block_id = self.format_expected_block_id(expected_seqno);

        log::info!(
            "Session {} COLLATION_FLOW failure: expected_block_id={} slot={} expected_seqno={} \
            elapsed_ms={} reason={}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            expected_seqno,
            elapsed_ms,
            reason,
        );
    }

    /// Drop self-collation tracking for a slot WITHOUT incrementing failure.
    ///
    /// Used when the flow is abandoned (e.g. cancelled callback, pipeline reset,
    /// stale-window result, future-slot precollation entry removed). The "ignore"
    /// metric is not tracked explicitly — `add_compute_result_metric()` derives it
    /// automatically as `total - success - failure`, so we just need to leave the
    /// counters consistent.
    fn forget_self_collation_tracking(&mut self, slot: SlotIndex, reason: &str) {
        let Some((started_at, expected_seqno)) = self.self_collation_starts_by_slot.remove(&slot)
        else {
            return;
        };

        let elapsed_ms = self.now().duration_since(started_at).unwrap_or_default().as_millis();
        let expected_block_id = self.format_expected_block_id(expected_seqno);

        log::info!(
            "Session {} COLLATION_FLOW ignore: expected_block_id={} slot={} expected_seqno={} \
            elapsed_ms={} reason={}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            expected_seqno,
            elapsed_ms,
            reason,
        );
    }

    /// Link an in-flight self-collation flow to the concrete `RawCandidateId`
    /// produced by the collator, so acceptance can be matched on `notify_block_finalized`.
    fn link_self_collation_candidate(&mut self, slot: SlotIndex, candidate_id: &RawCandidateId) {
        let Some((started_at, expected_seqno)) = self.self_collation_starts_by_slot.remove(&slot)
        else {
            return;
        };

        self.self_collation_pending_acceptance
            .insert(candidate_id.clone(), (started_at, expected_seqno));

        let expected_block_id = self.format_expected_block_id(expected_seqno);
        log::info!(
            "Session {} COLLATION_FLOW candidate: expected_block_id={} slot={} \
            expected_seqno={} candidate={}:{}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
        );
    }

    /// Record successful acceptance of our self-collated candidate when the
    /// finalized callback fires for it. This increments the success counter and
    /// records end-to-end acceptance latency.
    fn record_self_collation_acceptance(
        &mut self,
        candidate_id: &RawCandidateId,
        block_id: &BlockIdExt,
        has_final_cert: bool,
    ) {
        let Some((started_at, expected_seqno)) =
            self.self_collation_pending_acceptance.remove(candidate_id)
        else {
            return;
        };

        let acceptance_ms = self.now().duration_since(started_at).unwrap_or_default().as_millis();
        let expected_block_id = self.format_expected_block_id(expected_seqno);

        self.self_collates_counter.success();
        self.self_collation_accept_latency_histogram.record(acceptance_ms as f64);

        log::info!(
            "Session {} COLLATION_FLOW acceptance: expected_block_id={} slot={} \
            expected_seqno={} candidate={}:{} block_id={} has_final_cert={} acceptance_ms={}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            candidate_id.slot,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            block_id,
            has_final_cert,
            acceptance_ms,
        );
    }

    /// Record terminal failure for a self-collation tracked by `RawCandidateId`
    /// (i.e. the candidate already passed `link_self_collation_candidate`). Used
    /// when a generated candidate is rejected before reaching the finalized callback.
    fn record_self_collation_candidate_failure(
        &mut self,
        candidate_id: &RawCandidateId,
        reason: &str,
    ) {
        let Some((started_at, expected_seqno)) =
            self.self_collation_pending_acceptance.remove(candidate_id)
        else {
            return;
        };

        self.self_collates_counter.failure();

        let elapsed_ms = self.now().duration_since(started_at).unwrap_or_default().as_millis();
        let expected_block_id = self.format_expected_block_id(expected_seqno);

        log::info!(
            "Session {} COLLATION_FLOW failure: expected_block_id={} slot={} expected_seqno={} \
            candidate={}:{} elapsed_ms={} reason={}",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            candidate_id.slot,
            expected_seqno,
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            elapsed_ms,
            reason,
        );
    }

    /// Clear manual time override (return to real-time mode).
    #[allow(dead_code)]
    pub(crate) fn clear_time(&self) {
        self.description.clear_time();
    }

    /*
        Slot runtime + outcome gating helpers (future wiring)

        These helpers are intentionally not used yet by finalization/skip flow.
    */

    #[inline]
    fn slot_entry(&self, slot: SlotIndex) -> Option<&SlotEntry> {
        self.slots.get(&slot)
    }

    #[inline]
    fn slot_entry_mut(&mut self, slot: SlotIndex) -> &mut SlotEntry {
        self.slots.entry(slot).or_default()
    }

    /// Get mutable slot runtime, creating it if needed.
    ///
    /// This is the preferred accessor for "per-slot" state instead of global flags.
    #[inline]
    fn slot_runtime_mut(&mut self, slot: SlotIndex) -> &mut SlotRuntime {
        let now = self.now();
        let entry = self.slot_entry_mut(slot);
        entry.runtime.get_or_insert_with(|| SlotRuntime::new(now))
    }

    #[inline]
    fn slot_is_generated(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot).map_or(false, |e| e.is_generated())
    }

    /// Check if a candidate is rejected (uses session-level rejected set).
    #[inline]
    #[allow(dead_code)] // Available for future use
    fn is_rejected(&self, candidate_id: &RawCandidateId) -> bool {
        self.rejected.contains(candidate_id)
    }

    /*
        ========================================================================
        Per-slot collation state accessors

        These accessors provide per-slot access to collation state, replacing
        the global fields. Each slot maintains its own collation state.
        ========================================================================
    */

    /// Check if a slot has pending generation request.
    #[inline]
    fn slot_is_pending_generate(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.pending_generate)
    }

    /// Set pending_generate flag for a slot.
    #[inline]
    fn slot_set_pending_generate(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).pending_generate = value;
    }

    /// Set generated flag for a slot.
    #[inline]
    fn slot_set_generated(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).generated = value;
    }

    /// Check if a slot has sent_generated=true.
    #[inline]
    fn slot_is_sent_generated(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot).and_then(|e| e.runtime.as_ref()).map_or(false, |rt| rt.sent_generated)
    }

    /// Set sent_generated flag for a slot.
    #[inline]
    fn slot_set_sent_generated(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).sent_generated = value;
    }

    /// Get slot_started_at time for a slot (defaults to now if no runtime).
    #[inline]
    fn slot_started_at(&self, slot: SlotIndex) -> SystemTime {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or_else(|| self.now(), |rt| rt.slot_started_at)
    }

    /*
        ========================================================================
        Per-slot stage tracking accessors (for latency metrics)

        These accessors track milestone events within a slot for latency
        measurement: first candidate received, first notarized, first finalized.
        ========================================================================
    */

    /// Check if first candidate has been received for this slot.
    #[inline]
    fn slot_first_candidate_received(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_received)
    }

    /// Set first_candidate_received flag for a slot.
    #[inline]
    fn slot_set_first_candidate_received(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).first_candidate_received = value;
    }

    /// Check if first candidate has been notarized for this slot.
    #[inline]
    fn slot_first_candidate_notarized(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_notarized)
    }

    /// Set first_candidate_notarized flag for a slot.
    #[inline]
    fn slot_set_first_candidate_notarized(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).first_candidate_notarized = value;
    }

    /// Check if first candidate has been finalized for this slot.
    #[inline]
    fn slot_first_candidate_finalized(&self, slot: SlotIndex) -> bool {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .map_or(false, |rt| rt.first_candidate_finalized)
    }

    /// Set first_candidate_finalized flag for a slot.
    #[inline]
    fn slot_set_first_candidate_finalized(&mut self, slot: SlotIndex, value: bool) {
        self.slot_runtime_mut(slot).first_candidate_finalized = value;
    }

    /*
        ========================================================================
        Per-slot validated candidate data accessors

        validated_candidate_data remains per-slot since it's keyed by slot
        (not RawCandidateId). Other validation collections are session-level.
        ========================================================================
    */

    /// Store validated candidate data for finalization callback.
    #[inline]
    fn slot_set_validated_candidate_data(&mut self, slot: SlotIndex, data: ValidatedCandidate) {
        self.slot_runtime_mut(slot).validated_candidate_data = Some(data);
    }

    /// Get validated candidate data for finalization callback.
    #[inline]
    #[allow(dead_code)] // Available for future use
    fn slot_get_validated_candidate_data(&self, slot: SlotIndex) -> Option<&ValidatedCandidate> {
        self.slot_entry(slot)
            .and_then(|e| e.runtime.as_ref())
            .and_then(|rt| rt.validated_candidate_data.as_ref())
    }

    /// Create new session processor
    ///
    /// The processor is created with empty state. Bootstrap state is applied
    /// separately via `SessionStartupRecoveryProcessor::apply_bootstrap()`.
    ///
    /// # Parameters
    /// * `description` - Pre-built session description with all immutable config
    /// * `initial_errors` - Error count from startup phase (before processor was created)
    pub fn new(
        description: Arc<SessionDescription>,
        session_start_prev_blocks: Vec<BlockIdExt>,
        listener: SessionListenerPtr,
        task_queue: TaskQueuePtr,
        callbacks_task_queue: CallbackTaskQueuePtr,
        overlay_manager: ConsensusOverlayManagerPtr,
        receiver: ReceiverPtr,
        stop_flag: Arc<AtomicBool>,
        db: SimplexDbPtr,
        initial_errors: u32,
        receiver_health_counters: Arc<crate::receiver::ReceiverHealthCounters>,
    ) -> Result<Self> {
        // Extract immutable values from description before it's moved
        let session_id = description.get_session_id().clone();
        let initial_block_seqno = description.get_initial_block_seqno();
        let use_callback_thread = description.opts().use_callback_thread;
        assert!(
            !session_start_prev_blocks.is_empty() && session_start_prev_blocks.len() <= 2,
            "INVARIANT VIOLATION: SessionProcessor::new requires one or two session start prev blocks, got {}",
            session_start_prev_blocks.len()
        );
        assert_eq!(
            session_start_prev_blocks.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1,
            initial_block_seqno,
            "INVARIANT VIOLATION: session start prevs imply initial seqno {} != SessionDescription initial seqno {}",
            session_start_prev_blocks.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1,
            initial_block_seqno
        );

        // INVARIANT: initial_block_seqno must be > 0.
        // Block seqno 0 is reserved for the zerostate (genesis), so the first real block is seqno 1.
        // This invariant ensures finalized_head_seqno initialization (initial_block_seqno - 1) is valid.
        assert!(
            initial_block_seqno > 0,
            "INVARIANT VIOLATION: initial_block_seqno must be > 0, got {}",
            initial_block_seqno
        );

        // Initialize SimplexState FSM.
        // SIMPLEX_ROUNDLESS:
        // - We pass `SIMPLEX_ROUNDLESS` in callbacks to bypass round-based invariants.
        let simplex_state = SimplexState::new(&description)?;
        let initial_standstill_slots = simplex_state.get_tracked_slots_interval();
        let initial_progress_slot = simplex_state.get_first_non_progressed_slot().value();

        // Initialize receiver standstill tracked range to the FSM-tracked interval (C++ parity).
        // Receiver defaults to a broad range, but we can set the precise initial interval immediately
        // because `SimplexState::new()` creates window 0 (so end = slots_per_leader_window).
        receiver.set_ingress_slot_begin(initial_standstill_slots.0);
        receiver.set_ingress_progress_slot(initial_progress_slot);
        receiver.set_standstill_slots(initial_standstill_slots.0, initial_standstill_slots.1);

        log::info!(
            "Session {} SIMPLEX MODE: C++ parenting enabled (notarized parents accepted). \
            Candidate-native validation enabled. \
            WaitForParent gating=strict, MC stale protection=validator-side.",
            session_id.to_hex_string(),
        );

        log::info!(
            "Session {} SimplexState FSM initialized: slots_per_window={}",
            session_id.to_hex_string(),
            description.opts().slots_per_leader_window,
        );
        log::warn!(
            "Session {}: TEMP experimental MAX_AWAKE_TIMEOUT={}ms is enabled; restore the old \
            far-future fallback after timing validation is complete",
            session_id.to_hex_string(),
            MAX_AWAKE_TIMEOUT.as_millis(),
        );

        // Initialize metrics
        let metrics_receiver = description.get_metrics_receiver().clone();
        let (
            check_all_counter,
            process_events_counter,
            slot_duration_histogram,
            validation_latency_histogram,
            collation_latency_histogram,
            check_all_wake_slip_histogram,
            active_weight_gauge,
            validates_counter,
            collates_counter,
            self_collates_counter,
            precollation_requests_counter,
            precollation_results_counter,
            collates_precollated_counter,
            collates_expire_counter,
            collation_starts_counter,
            self_collation_accept_latency_histogram,
            broadcast_validation_latency_histogram,
            first_candidate_received_latency_histogram,
            first_candidate_notarized_latency_histogram,
            first_candidate_finalized_latency_histogram,
            errors_counter,
            misbehavior_counter,
            last_finalized_slot_gauge,
            first_non_finalized_slot_gauge,
            first_non_progressed_slot_gauge,
            skip_total_counter,
            votes_in_total_counter,
            votes_in_notarize_counter,
            votes_in_finalize_counter,
            votes_in_skip_counter,
            votes_out_total_counter,
            votes_out_notarize_counter,
            votes_out_finalize_counter,
            votes_out_skip_counter,
            votes_out_persist_fail_counter,
            certs_in_counter,
            certs_relayed_counter,
            cert_conflict_counter,
            cert_verify_fail_counter,
            validation_reject_counter,
            validation_late_callback_counter,
            health_warnings_counter,
            candidate_precheck_old_slot_drop_counter,
            candidate_precheck_future_slot_drop_counter,
            candidate_precheck_unexpected_sender_drop_counter,
            candidate_precheck_conflicting_slot_drop_counter,
            candidate_received_broadcast_counter,
            candidate_received_query_counter,
            generated_candidate_validation_missed_counter,
        ) = Self::init_metrics(&metrics_receiver, &description);

        let finalized_pending_body_gauge =
            metrics_receiver.sink().register_gauge(&"simplex_finalized_pending_body_count".into());

        let now = description.get_time();
        let num_validators = description.get_total_nodes() as usize;

        // first_nonannounced_window starts at 0, set via recovery_set_first_nonannounced_window()
        let first_nonannounced_window = WindowIndex::default();

        let health_alert_cooldown = description.opts().health_alert_cooldown;

        let processor = Self {
            description,
            session_start_prev_blocks,
            task_queue,
            callbacks_task_queue,
            listener,
            next_awake_time: now,
            overlay_manager,
            receiver,
            stop_flag,
            db,
            first_nonannounced_window,
            candidate_info_store_results: HashMap::new(),
            notar_cert_store_results: HashMap::new(),
            final_cert_store_results: HashMap::new(),
            skip_cert_store_results: HashMap::new(),
            use_callback_thread,
            active_weight: 0,
            last_activity: vec![None; num_validators],
            delayed_actions: Vec::new(),
            simplex_state,
            missing_body_logged: HashSet::new(),
            // Collation state
            precollated_blocks: PrecollatedBlockMap::new(),
            precollated_blocks_next_request_id: 0,
            precollated_blocks_max_slot: None,
            earliest_collation_time: None,
            local_chain_head: None,
            generated_parent_cache: HashMap::new(),
            generated_parent_gen_utime_ms_cache: HashMap::new(),
            generated_candidates_waiting_validation: HashMap::new(),
            self_collation_starts_by_slot: HashMap::new(),
            self_collation_pending_acceptance: HashMap::new(),
            // Validation state
            pending_validations: HashMap::new(),
            pending_approve: HashSet::new(),
            pending_reject: HashMap::new(),
            rejected: HashSet::new(),
            approved: HashMap::new(),
            validation_attempt_map: HashMap::new(),
            validated_candidates: VecDeque::new(),
            received_candidates: HashMap::new(),
            candidate_data_cache: HashMap::new(),
            seen_broadcast_candidates: HashMap::new(),
            // Metrics
            metrics_receiver,
            check_all_counter,
            process_events_counter,
            slot_duration_histogram,
            validation_latency_histogram,
            collation_latency_histogram,
            check_all_wake_slip_histogram,
            active_weight_gauge,
            validates_counter,
            collates_counter,
            self_collates_counter,
            precollation_requests_counter,
            precollation_results_counter,
            collates_precollated_counter,
            collates_expire_counter,
            collation_starts_counter,
            self_collation_accept_latency_histogram,
            broadcast_validation_latency_histogram,
            errors_counter,
            finalized_pending_body_gauge,
            // Error tracking (includes startup errors from before processor was created)
            session_errors_count: AtomicU32::new(initial_errors),
            // Slot stage tracking
            first_candidate_received_latency_histogram,
            first_candidate_notarized_latency_histogram,
            first_candidate_finalized_latency_histogram,
            // Debug
            round_debug_at: now + ROUND_DEBUG_PERIOD,
            last_finalization_time: now,
            // Slot/round tracking
            last_generated_slot: None,
            // Treat the block *before* `initial_block_seqno` as the finalized head at session start.
            //
            // This is required for:
            // - empty-block generation gating (non-finalized parent / ValidatorGroup limitation),
            // - validation gating (expected_seqno = finalized_head_seqno + 1),
            // and matches C++ where the block producer tracks the parent seqno from `Start` / `base`.
            finalized_head_seqno: initial_block_seqno.checked_sub(1),
            finalized_head_slot: None,
            finalized_head_block_id: None,
            finalized_head_before_split: false,
            before_split_by_block_id: HashMap::new(),
            last_consensus_finalized_seqno: initial_block_seqno.checked_sub(1),
            // Batch finalization tracking
            finalized_blocks: HashSet::new(),
            finalized_pending_body: HashMap::new(),
            slots: BTreeMap::new(),
            // Empty block support
            last_mc_finalized_seqno: initial_block_seqno.checked_sub(1),
            accepted_normal_head_seqno: initial_block_seqno.saturating_sub(1),
            accepted_normal_head_block_id: None,
            // Candidate request tracking
            requested_candidates: HashMap::new(),
            finalized_delivery_sent: HashSet::new(),
            finalized_delivery_sent_seqno: HashMap::new(),
            // Misbehavior tracking
            misbehavior_reports: Vec::new(),
            misbehavior_counter,
            last_finalized_slot_gauge,
            first_non_finalized_slot_gauge,
            first_non_progressed_slot_gauge,
            skip_total_counter,
            votes_in_total_counter,
            votes_in_notarize_counter,
            votes_in_finalize_counter,
            votes_in_skip_counter,
            votes_out_total_counter,
            votes_out_notarize_counter,
            votes_out_finalize_counter,
            votes_out_skip_counter,
            votes_out_persist_fail_counter,
            votes_in_notarize_total: 0,
            votes_in_finalize_total: 0,
            votes_in_skip_total: 0,
            certs_in_counter,
            certs_relayed_counter,
            cert_conflict_counter,
            cert_verify_fail_counter,
            validation_reject_counter,
            validation_late_callback_counter,
            health_warnings_counter,
            candidate_precheck_old_slot_drop_counter,
            candidate_precheck_future_slot_drop_counter,
            candidate_precheck_unexpected_sender_drop_counter,
            candidate_precheck_conflicting_slot_drop_counter,
            candidate_received_broadcast_counter,
            candidate_received_query_counter,
            generated_candidate_validation_missed_counter,
            health_alert_state: HealthAlertState::new(now, health_alert_cooldown),
            receiver_health_counters,
            cert_verify_fails_total: 0,
            observability: SessionObservability::new(now),
            last_receiver_snapshot: None,
        };

        // Increment errors_counter metric with startup errors (for metrics consistency)
        if initial_errors > 0 {
            processor.errors_counter.increment(initial_errors as u64);
            log::debug!(
                "Session {} initialized with {} startup errors",
                processor.session_id().to_hex_string(),
                initial_errors
            );
        }

        Ok(processor)

        // Note: C++ simplex resolves candidates from its own consensus DB, not via
        // validator manager. The Rust implementation uses in-memory candidate_data_cache
        // and peer overlay for candidate resolution. No get_approved_candidate delegation.
    }

    /*
        Session identity helpers
    */

    /// Get session identifier (convenience accessor)
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.description.get_session_id()
    }

    /// Get local validator's private key (convenience accessor)
    #[inline]
    fn local_key(&self) -> &PrivateKey {
        self.description.get_local_key()
    }

    /// Get session creation time (convenience accessor)
    #[inline]
    fn session_creation_time(&self) -> SystemTime {
        self.description.get_session_creation_time()
    }

    /// Format a duration as "X.Ys ago" or "never".
    fn fmt_ago(now: SystemTime, t: Option<SystemTime>) -> String {
        t.and_then(|t| now.duration_since(t).ok())
            .map(|d| format!("{:.1}s ago", d.as_secs_f64()))
            .unwrap_or_else(|| "never".to_string())
    }

    /// Format a duration as "X.Ys" or "never".
    fn fmt_dur(now: SystemTime, t: SystemTime) -> String {
        now.duration_since(t)
            .ok()
            .map(|d| format!("{:.1}s", d.as_secs_f64()))
            .unwrap_or_else(|| "?".to_string())
    }

    /*
        Metrics initialization
    */

    /// Initialize metrics for the session processor
    ///
    /// Creates all counters, histograms, and gauges used for performance tracking.
    /// Reference: validator-session/src/session_processor.rs metrics initialization
    #[allow(clippy::type_complexity)]
    #[allow(clippy::too_many_arguments)]
    fn init_metrics(
        metrics_receiver: &MetricsHandle,
        description: &SessionDescription,
    ) -> (
        metrics::Counter,    // check_all_counter
        metrics::Counter,    // process_events_counter
        metrics::Histogram,  // slot_duration_histogram
        metrics::Histogram,  // validation_latency_histogram
        metrics::Histogram,  // collation_latency_histogram
        metrics::Histogram,  // check_all_wake_slip_histogram
        metrics::Gauge,      // active_weight_gauge
        ResultStatusCounter, // validates_counter
        ResultStatusCounter, // collates_counter
        ResultStatusCounter, // self_collates_counter
        metrics::Counter,    // precollation_requests_counter
        metrics::Counter,    // precollation_results_counter
        ResultStatusCounter, // collates_precollated_counter
        ResultStatusCounter, // collates_expire_counter
        metrics::Counter,    // collation_starts_counter
        metrics::Histogram,  // self_collation_accept_latency_histogram
        metrics::Histogram,  // broadcast_validation_latency_histogram
        metrics::Histogram,  // first_candidate_received_latency_histogram
        metrics::Histogram,  // first_candidate_notarized_latency_histogram
        metrics::Histogram,  // first_candidate_finalized_latency_histogram
        metrics::Counter,    // errors_counter
        metrics::Counter,    // misbehavior_counter
        metrics::Gauge,      // last_finalized_slot_gauge
        metrics::Gauge,      // first_non_finalized_slot_gauge
        metrics::Gauge,      // first_non_progressed_slot_gauge
        metrics::Counter,    // skip_total_counter
        metrics::Counter,    // votes_in_total_counter
        metrics::Counter,    // votes_in_notarize_counter
        metrics::Counter,    // votes_in_finalize_counter
        metrics::Counter,    // votes_in_skip_counter
        metrics::Counter,    // votes_out_total_counter
        metrics::Counter,    // votes_out_notarize_counter
        metrics::Counter,    // votes_out_finalize_counter
        metrics::Counter,    // votes_out_skip_counter
        metrics::Counter,    // votes_out_persist_fail_counter
        metrics::Counter,    // certs_in_counter
        metrics::Counter,    // certs_relayed_counter
        metrics::Counter,    // cert_conflict_counter
        metrics::Counter,    // cert_verify_fail_counter
        metrics::Counter,    // validation_reject_counter
        metrics::Counter,    // validation_late_callback_counter
        metrics::Counter,    // health_warnings_counter
        metrics::Counter,    // candidate_precheck_old_slot_drop_counter
        metrics::Counter,    // candidate_precheck_future_slot_drop_counter
        metrics::Counter,    // candidate_precheck_unexpected_sender_drop_counter
        metrics::Counter,    // candidate_precheck_conflicting_slot_drop_counter
        metrics::Counter,    // candidate_received_broadcast_counter
        metrics::Counter,    // candidate_received_query_counter
        metrics::Counter,    // generated_candidate_validation_missed_counter
    ) {
        let sink = metrics_receiver.sink();

        // Counters
        let check_all_counter = sink.register_counter(&"simplex_check_all_calls".into());
        let process_events_counter = sink.register_counter(&"simplex_process_events_calls".into());

        // Histograms (latency tracking)
        let slot_duration_histogram = sink.register_histogram(&"time:slot_duration".into());
        let validation_latency_histogram =
            sink.register_histogram(&"time:validation_latency".into());
        let collation_latency_histogram = sink.register_histogram(&"time:collation_latency".into());
        let self_collation_accept_latency_histogram =
            sink.register_histogram(&"time:self_collation_accept_latency".into());
        let check_all_wake_slip_histogram =
            sink.register_histogram(&"time:check_all_wake_slip_ms".into());
        let broadcast_validation_latency_histogram =
            sink.register_histogram(&"time:broadcast_validation_latency".into());

        // Slot stage latency histograms (analogous to round stages in validator-session)
        let first_candidate_received_latency_histogram =
            sink.register_histogram(&"time:slot_stage1_received_latency".into());
        let first_candidate_notarized_latency_histogram =
            sink.register_histogram(&"time:slot_stage2_notarized_latency".into());
        let first_candidate_finalized_latency_histogram =
            sink.register_histogram(&"time:slot_stage3_finalized_latency".into());

        // Gauges
        let active_weight_gauge = sink.register_gauge(&"simplex_active_weight".into());
        let total_weight_gauge = sink.register_gauge(&"simplex_total_weight".into());
        let threshold_66_gauge = sink.register_gauge(&"simplex_threshold_66".into());

        // Set initial gauge values
        total_weight_gauge.set(description.get_total_weight() as f64);
        threshold_66_gauge.set(description.get_threshold_66() as f64);

        // Result status counters
        let validates_counter = ResultStatusCounter::new(metrics_receiver, "simplex_validates");
        let collates_counter = ResultStatusCounter::new(metrics_receiver, "simplex_collates");
        let self_collates_counter =
            ResultStatusCounter::new(metrics_receiver, "simplex_self_collates");

        // Precollation metrics
        let precollation_requests_counter =
            sink.register_counter(&"simplex_precollation_requests".into());
        let precollation_results_counter =
            sink.register_counter(&"simplex_precollation_results".into());
        let collates_precollated_counter =
            ResultStatusCounter::new(metrics_receiver, "simplex_collates_precollated");
        let collates_expire_counter =
            ResultStatusCounter::new(metrics_receiver, "simplex_collates_expire");
        let collation_starts_counter = sink.register_counter(&"simplex_collation_starts".into());

        // Error tracking for ValidatorSessionStats
        let errors_counter = sink.register_counter(&"simplex_errors".into());

        let misbehavior_counter = sink.register_counter(&"simplex_misbehavior".into());

        let last_finalized_slot_gauge = sink.register_gauge(&"simplex_last_finalized_slot".into());
        let first_non_finalized_slot_gauge =
            sink.register_gauge(&"simplex_first_non_finalized_slot".into());
        let first_non_progressed_slot_gauge =
            sink.register_gauge(&"simplex_first_non_progressed_slot".into());
        let skip_total_counter = sink.register_counter(&"simplex_skip_total".into());

        let votes_in_total_counter = sink.register_counter(&"simplex_votes_in_total".into());
        let votes_in_notarize_counter = sink.register_counter(&"simplex_votes_in_notarize".into());
        let votes_in_finalize_counter = sink.register_counter(&"simplex_votes_in_finalize".into());
        let votes_in_skip_counter = sink.register_counter(&"simplex_votes_in_skip".into());
        let votes_out_total_counter = sink.register_counter(&"simplex_votes_out_total".into());
        let votes_out_notarize_counter =
            sink.register_counter(&"simplex_votes_out_notarize".into());
        let votes_out_finalize_counter =
            sink.register_counter(&"simplex_votes_out_finalize".into());
        let votes_out_skip_counter = sink.register_counter(&"simplex_votes_out_skip".into());
        let votes_out_persist_fail_counter =
            sink.register_counter(&"simplex_votes_out_persist_fail".into());

        let certs_in_counter = sink.register_counter(&"simplex_certs_in".into());
        let certs_relayed_counter = sink.register_counter(&"simplex_certs_relayed".into());
        let cert_conflict_counter = sink.register_counter(&"simplex_cert_conflict".into());
        let cert_verify_fail_counter = sink.register_counter(&"simplex_cert_verify_fail".into());

        let validation_reject_counter = sink.register_counter(&"simplex_validation_reject".into());
        let validation_late_callback_counter =
            sink.register_counter(&"simplex_validation_late_callback".into());

        let health_warnings_counter = sink.register_counter(&"simplex_health_warnings".into());
        let candidate_precheck_old_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_old_slot".into());
        let candidate_precheck_future_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_future_slot".into());
        let candidate_precheck_unexpected_sender_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_unexpected_sender".into());
        let candidate_precheck_conflicting_slot_drop_counter =
            sink.register_counter(&"simplex_candidate_precheck_drop_conflicting_slot".into());
        let candidate_received_broadcast_counter =
            sink.register_counter(&"simplex_candidate_received_broadcast".into());
        let candidate_received_query_counter =
            sink.register_counter(&"simplex_candidate_received_query".into());
        let generated_candidate_validation_missed_counter =
            sink.register_counter(&"simplex_generated_candidate_validation_missed".into());

        (
            check_all_counter,
            process_events_counter,
            slot_duration_histogram,
            validation_latency_histogram,
            collation_latency_histogram,
            check_all_wake_slip_histogram,
            active_weight_gauge,
            validates_counter,
            collates_counter,
            self_collates_counter,
            precollation_requests_counter,
            precollation_results_counter,
            collates_precollated_counter,
            collates_expire_counter,
            collation_starts_counter,
            self_collation_accept_latency_histogram,
            broadcast_validation_latency_histogram,
            first_candidate_received_latency_histogram,
            first_candidate_notarized_latency_histogram,
            first_candidate_finalized_latency_histogram,
            errors_counter,
            misbehavior_counter,
            last_finalized_slot_gauge,
            first_non_finalized_slot_gauge,
            first_non_progressed_slot_gauge,
            skip_total_counter,
            votes_in_total_counter,
            votes_in_notarize_counter,
            votes_in_finalize_counter,
            votes_in_skip_counter,
            votes_out_total_counter,
            votes_out_notarize_counter,
            votes_out_finalize_counter,
            votes_out_skip_counter,
            votes_out_persist_fail_counter,
            certs_in_counter,
            certs_relayed_counter,
            cert_conflict_counter,
            cert_verify_fail_counter,
            validation_reject_counter,
            validation_late_callback_counter,
            health_warnings_counter,
            candidate_precheck_old_slot_drop_counter,
            candidate_precheck_future_slot_drop_counter,
            candidate_precheck_unexpected_sender_drop_counter,
            candidate_precheck_conflicting_slot_drop_counter,
            candidate_received_broadcast_counter,
            candidate_received_query_counter,
            generated_candidate_validation_missed_counter,
        )
    }

    /*
        Session description access
    */

    /// Get session description
    #[allow(dead_code)]
    pub fn get_description(&self) -> &SessionDescription {
        &self.description
    }

    /// Get metrics receiver
    ///
    /// Used for registering additional metrics in the future.
    pub fn get_metrics_receiver(&self) -> &MetricsHandle {
        &self.metrics_receiver
    }

    fn track_generated_candidate_for_validation(&mut self, candidate_id: RawCandidateId) {
        self.generated_candidates_waiting_validation.insert(
            candidate_id,
            GeneratedCandidateValidationWatch {
                generated_at: self.now(),
                validation_started: false,
            },
        );
    }

    fn mark_generated_candidate_validation_started(&mut self, candidate_id: &RawCandidateId) {
        if let Some(watch) = self.generated_candidates_waiting_validation.get_mut(candidate_id) {
            watch.validation_started = true;
        }
    }

    fn mark_generated_candidate_validation_succeeded(&mut self, candidate_id: &RawCandidateId) {
        self.generated_candidates_waiting_validation.remove(candidate_id);
    }

    fn take_generated_candidate_watch_by_slot(
        &mut self,
        slot: SlotIndex,
    ) -> Option<(RawCandidateId, GeneratedCandidateValidationWatch)> {
        let candidate_id = self
            .generated_candidates_waiting_validation
            .keys()
            .find(|candidate_id| candidate_id.slot == slot)
            .cloned()?;
        let watch = self.generated_candidates_waiting_validation.remove(&candidate_id)?;
        Some((candidate_id, watch))
    }

    fn note_generated_candidate_validation_missed(
        &mut self,
        candidate_id: &RawCandidateId,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        let Some(watch) = self.generated_candidates_waiting_validation.remove(candidate_id) else {
            return;
        };

        self.generated_candidate_validation_missed_counter.increment(1);
        self.record_self_collation_candidate_failure(
            candidate_id,
            &format!("generated_candidate_validation_missed: {}", reason),
        );

        let waited_ms =
            self.now().duration_since(watch.generated_at).unwrap_or_default().as_millis();
        log::warn!(
            "Session {} local_generated_candidate_missed_validation: slot={} hash={} \
            validation_started={} waited={}ms reason={}",
            &self.session_id().to_hex_string()[..8],
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            watch.validation_started,
            waited_ms,
            reason,
        );
    }

    fn note_generated_candidate_validation_missed_for_slot(
        &mut self,
        slot: SlotIndex,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        if let Some((candidate_id, watch)) = self.take_generated_candidate_watch_by_slot(slot) {
            self.generated_candidate_validation_missed_counter.increment(1);
            self.record_self_collation_candidate_failure(
                &candidate_id,
                &format!("generated_candidate_validation_missed_for_slot: {}", reason),
            );

            let waited_ms =
                self.now().duration_since(watch.generated_at).unwrap_or_default().as_millis();
            log::warn!(
                "Session {} local_generated_candidate_missed_validation: slot={} hash={} \
                validation_started={} waited={}ms reason={}",
                &self.session_id().to_hex_string()[..8],
                candidate_id.slot,
                &candidate_id.hash.to_hex_string()[..8],
                watch.validation_started,
                waited_ms,
                reason,
            );
            return;
        }

        self.generated_candidate_validation_missed_counter.increment(1);
        log::warn!(
            "Session {} local_generated_candidate_missed_validation: slot={} reason={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            reason,
        );
    }

    /*
        Validator index validation
    */

    /// Check if validator index is valid (within bounds)
    #[inline]
    fn is_valid_source(&self, source_idx: ValidatorIndex) -> bool {
        source_idx.is_valid(self.description.get_total_nodes())
    }

    /*
        Error tracking
    */

    /// Increment the session error counter
    ///
    /// Called when an error occurs during session processing.
    /// Uses atomic increment to allow calling with &self (no &mut self required).
    fn increment_error(&self) {
        self.session_errors_count.fetch_add(1, Ordering::Relaxed);
        self.errors_counter.increment(1);
    }

    // =========================================================================
    // DB Helper Methods
    // =========================================================================

    /// Persist pool state (`first_nonannounced_window`) when leader window advances.
    ///
    /// C++ reference: `SimplexPoolImpl::maybe_publish_new_leader_window()`:
    /// - computes `new_window = now_ / slots_per_leader_window_`
    /// - if `new_window >= first_nonannounced_window_` then
    ///   sets `first_nonannounced_window_ = new_window + 1` and `co_await store_pool_state_to_db()`
    fn maybe_store_pool_state(&mut self) {
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if current_window < self.first_nonannounced_window {
            log::trace!(
                "Session {} maybe_store_pool_state: no-op (current_window={current_window}, \
                first_nonannounced_window={})",
                &self.session_id().to_hex_string()[..8],
                self.first_nonannounced_window,
            );
            return;
        }

        log::trace!(
            "Session {} maybe_store_pool_state: window advanced (current_window={current_window}, \
            first_nonannounced_window={}), storing",
            &self.session_id().to_hex_string()[..8],
            self.first_nonannounced_window,
        );

        // Persist "next unannounced window" = current_window + 1 (matches C++).
        self.first_nonannounced_window = current_window + 1;

        let record = PoolStateRecord { first_nonannounced_window: self.first_nonannounced_window };
        let result = match self.db.save_pool_state_async(&record) {
            Ok(r) => r,
            Err(e) => {
                log::error!(
                    "Session {} maybe_store_pool_state: failed to create pool_state save ({}): {}",
                    &self.session_id().to_hex_string()[..8],
                    self.first_nonannounced_window,
                    e
                );
                self.increment_error();
                return;
            }
        };

        // C++ awaits this write; we do the same (blocking).
        let wait_started_at = self.now();
        log::trace!(
            "Session {} maybe_store_pool_state: waiting poolState db.set \
            (first_nonannounced_window={})",
            &self.session_id().to_hex_string()[..8],
            self.first_nonannounced_window,
        );
        if let Err(e) = result.wait() {
            log::error!(
                "Session {} maybe_store_pool_state: failed to store pool_state ({}) after {}ms: {}",
                &self.session_id().to_hex_string()[..8],
                self.first_nonannounced_window,
                self.now().duration_since(wait_started_at).map(|d| d.as_millis()).unwrap_or(0),
                e
            );
            self.increment_error();
        } else {
            log::trace!(
                "Session {} maybe_store_pool_state: stored pool_state \
                (first_nonannounced_window={}) in {}ms",
                &self.session_id().to_hex_string()[..8],
                self.first_nonannounced_window,
                self.now().duration_since(wait_started_at).map(|d| d.as_millis()).unwrap_or(0),
            );
        }
    }

    /// Wait for CandidateResolver-related DB writes (candidateInfo / notarCert).
    ///
    /// Parity with C++ `WaitCandidateInfoStored(id, wait_candidate_info, wait_notar_cert)`.
    ///
    fn wait_candidate_info_stored(
        &mut self,
        id: &RawCandidateId,
        wait_candidate_info: bool,
        wait_notar_cert: bool,
    ) -> bool {
        let mut candidate_info_ok = true;
        let mut notar_cert_ok = true;

        log::trace!(
            "Session {} WaitCandidateInfoStored: start s{}:{} info={} notar={}",
            &self.session_id().to_hex_string()[..8],
            id.slot.value(),
            &id.hash.to_hex_string()[..8],
            wait_candidate_info,
            wait_notar_cert
        );

        if wait_candidate_info {
            match self.candidate_info_store_results.get(id) {
                Some(res) => {
                    let wait_started_at = self.now();
                    log::trace!(
                        "Session {} WaitCandidateInfoStored: waiting candidateInfo db.set for \
                        s{}:{} (ready={})",
                        &self.session_id().to_hex_string()[..8],
                        id.slot.value(),
                        &id.hash.to_hex_string()[..8],
                        res.is_ready(),
                    );
                    if let Err(e) = res.wait() {
                        if e.to_string().contains("result already taken") {
                            log::trace!(
                                "Session {} WaitCandidateInfoStored: candidateInfo result already \
                                consumed for s{}:{}",
                                &self.session_id().to_hex_string()[..8],
                                id.slot.value(),
                                &id.hash.to_hex_string()[..8],
                            );
                        } else {
                            log::error!(
                                "Session {} WaitCandidateInfoStored: candidateInfo wait failed \
                                for s{}:{} after {}ms: {e}",
                                &self.session_id().to_hex_string()[..8],
                                id.slot.value(),
                                &id.hash.to_hex_string()[..8],
                                self.now()
                                    .duration_since(wait_started_at)
                                    .map(|d| d.as_millis())
                                    .unwrap_or(0),
                            );
                            self.increment_error();
                            candidate_info_ok = false;
                        }
                    } else {
                        log::trace!(
                            "Session {} WaitCandidateInfoStored: candidateInfo stored for s{}:{} \
                            in {}ms",
                            &self.session_id().to_hex_string()[..8],
                            id.slot.value(),
                            &id.hash.to_hex_string()[..8],
                            self.now()
                                .duration_since(wait_started_at)
                                .map(|d| d.as_millis())
                                .unwrap_or(0),
                        );
                    }
                }
                None => {
                    // We can't reconstruct CandidateInfo here without additional context.
                    // Treat as persistence error and let the caller decide whether to abort.
                    log::error!(
                        "Session {} WaitCandidateInfoStored: missing candidateInfo store result \
                        for s{}:{}",
                        &self.session_id().to_hex_string()[..8],
                        id.slot.value(),
                        &id.hash.to_hex_string()[..8],
                    );
                    self.increment_error();
                    candidate_info_ok = false;
                }
            }
        }

        if wait_notar_cert {
            match self.notar_cert_store_results.get(id) {
                Some(res) => {
                    let wait_started_at = self.now();
                    log::trace!(
                        "Session {} WaitCandidateInfoStored: waiting notarCert db.set for s{}:{} \
                        (ready={})",
                        &self.session_id().to_hex_string()[..8],
                        id.slot.value(),
                        &id.hash.to_hex_string()[..8],
                        res.is_ready(),
                    );
                    if let Err(e) = res.wait() {
                        if e.to_string().contains("result already taken") {
                            log::trace!(
                                "Session {} WaitCandidateInfoStored: notarCert result already \
                                consumed for s{}:{}",
                                &self.session_id().to_hex_string()[..8],
                                id.slot.value(),
                                &id.hash.to_hex_string()[..8],
                            );
                        } else {
                            log::error!(
                                "Session {} WaitCandidateInfoStored: notarCert wait failed for \
                                s{}:{} after {}ms: {e}",
                                &self.session_id().to_hex_string()[..8],
                                id.slot.value(),
                                &id.hash.to_hex_string()[..8],
                                self.now()
                                    .duration_since(wait_started_at)
                                    .map(|d| d.as_millis())
                                    .unwrap_or(0),
                            );
                            self.increment_error();
                            notar_cert_ok = false;
                        }
                    } else {
                        log::trace!(
                            "Session {} WaitCandidateInfoStored: notarCert stored for s{}:{} in \
                            {}ms",
                            &self.session_id().to_hex_string()[..8],
                            id.slot.value(),
                            &id.hash.to_hex_string()[..8],
                            self.now()
                                .duration_since(wait_started_at)
                                .map(|d| d.as_millis())
                                .unwrap_or(0),
                        );
                    }
                }
                None => {
                    log::error!(
                        "Session {} WaitCandidateInfoStored: missing notarCert store result for \
                        s{}:{}",
                        &self.session_id().to_hex_string()[..8],
                        id.slot.value(),
                        &id.hash.to_hex_string()[..8],
                    );
                    self.increment_error();
                    notar_cert_ok = false;
                }
            }
        }

        candidate_info_ok && notar_cert_ok
    }

    /// Save candidate info to database (fire-and-forget)
    ///
    /// Deserializes candidate_hash_data bytes and saves to DB.
    /// Matches C++ candidate-resolver.cpp: `store_to_db(id, state).start().detach()`
    fn save_candidate_info_to_db(
        &mut self,
        slot: SlotIndex,
        candidate_hash: &UInt256,
        leader_idx: ValidatorIndex,
        candidate_hash_data_bytes: &[u8],
        signature: Vec<u8>,
    ) {
        // Deserialize CandidateHashData from bytes
        let candidate_hash_data =
            match Self::deserialize_candidate_hash_data(candidate_hash_data_bytes) {
                Ok(data) => data,
                Err(e) => {
                    log::warn!(
                        "Session {} save_candidate_info_to_db: failed to deserialize \
                    CandidateHashData for slot={slot}: {e}",
                        &self.session_id().to_hex_string()[..8],
                    );
                    return;
                }
            };

        let record = CandidateInfoRecord {
            candidate_id: RawCandidateId { slot, hash: candidate_hash.clone() },
            leader_idx: leader_idx.value(),
            candidate_hash_data,
            signature,
        };

        // Store only once per candidate_id (WaitCandidateInfoStored parity)
        if self.candidate_info_store_results.contains_key(&record.candidate_id) {
            return;
        }

        match self.db.save_candidate_info_async(&record) {
            Ok(result) => {
                self.candidate_info_store_results.insert(record.candidate_id.clone(), result);
            }
            Err(e) => {
                log::error!(
                    "Session {} store_candidate_info: failed to create candidate_info save: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
            }
        }
    }

    /// Deserialize CandidateHashData TL bytes
    fn deserialize_candidate_hash_data(bytes: &[u8]) -> Result<CandidateHashData> {
        deserialize_typed(bytes)
    }

    /*
        ========================================================================
        Empty Block Support (TON-specific extension for finalization recovery)

        Reference: C++ block-producer.cpp should_generate_empty_block()

        Empty blocks are used when the consensus chain gets ahead of blockchain
        finalization. Instead of generating a new block with transactions,
        validators re-sign the previous block to help it get a FinalizeCertificate.
        ========================================================================
    */

    fn advance_accepted_normal_head_seqno(&mut self, seqno: u32) {
        if seqno > self.accepted_normal_head_seqno {
            self.accepted_normal_head_seqno = seqno;
            if self
                .accepted_normal_head_block_id
                .as_ref()
                .is_some_and(|block_id| block_id.seq_no < seqno)
            {
                self.accepted_normal_head_block_id = None;
            }
        }
    }

    fn advance_accepted_normal_head_block(&mut self, block_id: BlockIdExt) {
        self.advance_accepted_normal_head_seqno(block_id.seq_no);
        match self.accepted_normal_head_block_id.as_ref() {
            Some(current) if current >= &block_id => {}
            _ => self.accepted_normal_head_block_id = Some(block_id),
        }
    }

    /// Update applied-top tracking from manager notification.
    ///
    /// This should be called when manager forwards the current applied top for this
    /// session shard.
    ///
    /// Mirrors the C++ external notify path:
    /// - `block-producer.cpp::handle(BlockFinalizedInMasterchain)` updates
    ///   `last_mc_finalized_seqno_` and `last_consensus_finalized_seqno_`
    /// - `block-validator.cpp::handle(BlockFinalizedInMasterchain)` advances the exact
    ///   accepted head when `seqno != 0`
    ///
    /// The seqno is used in `should_generate_empty_block()` and the exact block id is
    /// also used to seed the accepted-head cursor when known.
    ///
    /// # Arguments
    ///
    /// * `applied_top` - Current applied top for this session shard
    ///
    pub fn set_mc_finalized_block(&mut self, applied_top: BlockIdExt) {
        self.observability.last_mc_applied_block_id = Some(applied_top.clone());
        let session_shard = self.description.get_shard();
        if applied_top.shard() != session_shard {
            log::trace!(
                "Session {}: ignoring MC finalization update for mismatched shard {} \
                (session shard {})",
                &self.session_id().to_hex_string()[..8],
                applied_top.shard(),
                session_shard
            );
            return;
        }
        let seqno = applied_top.seq_no;
        log::trace!(
            "Session {}: set_applied_top_seqno={} (was {:?})",
            &self.session_id().to_hex_string()[..8],
            seqno,
            self.last_mc_finalized_seqno
        );
        // Keep last_mc_finalized_seqno monotonic, mirroring C++ behavior:
        // last_mc_finalized_seqno_ = std::max(event->block.seqno(), last_mc_finalized_seqno_);
        let prev_mc = self.last_mc_finalized_seqno.unwrap_or(0);
        let updated_mc = seqno.max(prev_mc);
        self.last_mc_finalized_seqno = Some(updated_mc);

        // C++ block-producer.cpp parity:
        // last_consensus_finalized_seqno_ = std::max(last_mc_finalized_seqno_,
        //                                            last_consensus_finalized_seqno_);
        let prev_consensus = self.last_consensus_finalized_seqno.unwrap_or(0);
        self.last_consensus_finalized_seqno = Some(updated_mc.max(prev_consensus));

        // C++ block-validator.cpp ignores seqno 0 on external notify, so only seed the
        // exact accepted head when the applied top is a non-zerostate block.
        if seqno != 0 {
            self.advance_accepted_normal_head_block(applied_top);
        } else {
            self.advance_accepted_normal_head_seqno(seqno);
        }
        self.wake_now();
    }

    /// Get the last masterchain finalized seqno
    ///
    /// Returns the last known applied-top seqno for this session shard.
    #[allow(dead_code)]
    pub fn last_mc_finalized_seqno(&self) -> Option<u32> {
        self.last_mc_finalized_seqno
    }

    /// Determines if an empty block should be generated for finalization recovery
    ///
    /// Empty blocks are a TON-specific extension that
    /// allows the consensus to continue when the blockchain finalization is lagging
    /// behind. Instead of generating a new block with transactions, validators
    /// re-sign the previous block to help it get a FinalizeCertificate.
    ///
    /// # Arguments
    ///
    /// * `parent_before_split` - Optional `before_split` flag for the current
    ///   collation parent (resolved from current parent state)
    /// * `new_seqno` - The seqno of the block that would be generated
    ///
    /// # Logic
    ///
    /// - **Masterchain**: Generate empty if `last_consensus_finalized_seqno + 1 < new_seqno`
    ///   (i.e., consensus-finalized is more than 1 behind)
    /// - **Shardchain**: Generate empty if `last_mc_finalized_seqno + 8 < new_seqno`
    ///   (i.e., MC is more than 8 behind)
    ///
    /// Returns `false` if finalization tracking is not yet initialized.
    ///
    /// # Reference
    ///
    /// C++ `block-producer.cpp`:
    /// ```cpp
    /// bool should_generate_empty_block(BlockSeqno new_seqno) {
    ///     if (owning_bus()->shard.is_masterchain()) {
    ///         return last_consensus_finalized_seqno_ + 1 < new_seqno;
    ///     } else {
    ///         return last_mc_finalized_seqno_ + 8 < new_seqno;
    ///     }
    /// }
    /// ```
    pub fn should_generate_empty_block(
        &self,
        slot: SlotIndex,
        new_seqno: u32,
        parent_before_split: Option<bool>,
    ) -> bool {
        // C++ parity: ALWAYS generate empty if current parent state has
        // before_split=true. Prefer parent-derived value; fall back to finalized
        // head only when parent-specific metadata is unavailable.
        if parent_before_split.unwrap_or(self.finalized_head_before_split) {
            log::debug!(
                "Session {} should_generate_empty_block: slot={}, seqno={} - generating EMPTY \
                (parent before_split=true, required for split/merge)",
                &self.session_id().to_hex_string()[..8],
                slot,
                new_seqno
            );
            return true;
        }

        if self.description.get_shard().is_masterchain() {
            // Masterchain: consensus-finalized seqno must be at most 1 behind new seqno.
            // C++ parity: external notify via `set_mc_finalized_block()` also advances
            // this producer-side cursor through `last_mc_finalized_seqno`.
            match self.last_consensus_finalized_seqno {
                Some(finalized) => finalized + 1 < new_seqno,
                None => false,
            }
        } else {
            // Shardchain: MC finalized can be up to threshold behind.
            // C++ parity: block-producer.cpp `last_mc_finalized_seqno_ + 8 < new_seqno`.
            match (
                self.last_mc_finalized_seqno,
                self.description.opts().empty_block_mc_lag_threshold,
            ) {
                (Some(mc_finalized), Some(threshold)) => mc_finalized + threshold < new_seqno,
                _ => false,
            }
        }
    }

    /*
        Timer management
    */

    /// Get next awake time
    pub fn get_next_awake_time(&self) -> SystemTime {
        self.next_awake_time
    }

    /// Set next awake time
    pub fn set_next_awake_time(&mut self, time: SystemTime) {
        if time < self.next_awake_time {
            self.next_awake_time = time;
        }
    }

    /// Reset next awake time to the fallback poll horizon.
    ///
    /// Called at the beginning of check_all() before collecting timeouts from all sources.
    pub fn reset_next_awake_time(&mut self) {
        self.next_awake_time = self.now() + MAX_AWAKE_TIMEOUT;
    }

    /// Force the main loop to run `check_all()` again immediately.
    fn wake_now(&mut self) {
        self.set_next_awake_time(self.now());
    }

    /*
        Delayed actions
    */

    /// Post a delayed action to be executed at a future time
    ///
    /// The handler will be called when `expiration_time` is reached during `check_all()`.
    /// Reference: validator-session/src/session_processor.rs post_delayed_action
    ///
    /// # Arguments
    /// * `expiration_time` - When to execute the action
    /// * `handler` - Closure to execute (takes `&mut SessionProcessor`)
    fn post_delayed_action<F>(&mut self, expiration_time: SystemTime, handler: F)
    where
        F: FnOnce(&mut SessionProcessor) + Send + 'static,
    {
        let delayed_action = DelayedAction { expiration_time, handler: Box::new(handler) };

        self.delayed_actions.push(delayed_action);
        self.set_next_awake_time(expiration_time);
    }

    /// Process all expired delayed actions
    ///
    /// Iterates through delayed actions and executes those whose expiration time
    /// has been reached. Remaining actions update `next_awake_time` to ensure
    /// timely wakeup.
    ///
    /// Reference: validator-session/src/session_processor.rs process_delayed_actions
    fn process_delayed_actions(&mut self) {
        let now = self.now();
        let mut i = 0;

        while i < self.delayed_actions.len() {
            if self.delayed_actions[i].expiration_time <= now {
                // Remove and execute expired action
                let delayed_action = self.delayed_actions.swap_remove(i);
                (delayed_action.handler)(self);
                // Don't increment i - swap_remove moved last element to position i
            } else {
                // Not expired yet, update awake time and move to next
                self.set_next_awake_time(self.delayed_actions[i].expiration_time);
                i += 1;
            }
        }
    }

    /*
        Debug logging
    */

    /// Log current consensus state for debugging
    ///
    /// When trace logging is enabled, dumps brief consensus state.
    /// Used after incoming/outgoing messages for observability.
    ///
    /// Reference: validator-session/src/session_processor.rs "VirtualState check:" log
    fn log_consensus_state(&self, trigger: &str) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }

        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        let fsm_first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();

        // Use session-level validation state for logging
        let pending_validations_count = self.pending_validations.len();
        let validated_count = self.validated_candidates.len();

        let has_notarized = self.simplex_state.has_notarized_block(fsm_first_non_finalized_slot);
        let is_finalized = self.simplex_state.is_slot_finalized(fsm_first_non_finalized_slot);

        log::trace!(
            "Session {} ConsensusState: trigger={}, slot_nf={:03}, slot_np={:03}, \
            generated={:<5}, pending_gen={:<5}, \
            pending_val={}, validated={}, \
            notarized={}, finalized={}",
            self.session_id().to_hex_string(),
            trigger,
            fsm_first_non_finalized_slot,
            fsm_first_non_progressed_slot,
            self.slot_is_generated(fsm_first_non_progressed_slot),
            self.slot_is_pending_generate(fsm_first_non_progressed_slot),
            pending_validations_count,
            validated_count,
            has_notarized,
            is_finalized,
        );
    }

    /// Collect current health findings without logging or cooldown gating.
    ///
    /// Used by both `debug_dump()` (for the stall conclusion) and `run_health_checks()`
    /// (for cooldown-gated alerts).
    fn collect_health_findings(&self) -> Vec<HealthFinding> {
        let now = self.now();
        let mut findings = Vec::new();

        let first_non_finalized = self.simplex_state.get_first_non_finalized_slot().0;
        let first_non_progressed = self.simplex_state.get_first_non_progressed_slot().0;
        let window_size = self.description.opts().slots_per_leader_window;
        let total_weight = self.description.get_total_weight();
        let active_weight = self.active_weight;

        // Progress gap
        if first_non_progressed > first_non_finalized {
            let gap = first_non_progressed - first_non_finalized;
            if gap > window_size {
                let sev = if gap > 2 * window_size { log::Level::Error } else { log::Level::Warn };
                findings.push(HealthFinding {
                    kind: HealthFindingKind::ProgressGap,
                    severity: sev,
                    summary: format!(
                        "progress gap={gap} (nf={first_non_finalized} np={first_non_progressed} \
                        window={window_size})"
                    ),
                });
            }
        }

        // Zero finalization speed
        let stall_warn_secs = self.description.opts().health_stall_warning_secs;
        let stall_duration = now
            .duration_since(self.health_alert_state.last_finalization_nonzero_at)
            .unwrap_or_default();
        if stall_duration >= Duration::from_secs(stall_warn_secs) {
            let stall_err_secs = self.description.opts().health_stall_error_secs;
            let sev = if stall_duration >= Duration::from_secs(stall_err_secs) {
                log::Level::Error
            } else {
                log::Level::Warn
            };
            findings.push(HealthFinding {
                kind: HealthFindingKind::ZeroFinalizationSpeed,
                severity: sev,
                summary: format!("no local finalization for {:.1}s", stall_duration.as_secs_f64()),
            });
        }

        // Low activity
        let t66 = threshold_66(total_weight);
        if active_weight < t66 {
            let t33 = threshold_33(total_weight);
            let sev = if active_weight < t33 { log::Level::Error } else { log::Level::Warn };
            let pct = if total_weight > 0 {
                (active_weight as f64 / total_weight as f64) * 100.0
            } else {
                0.0
            };
            findings.push(HealthFinding {
                kind: HealthFindingKind::LowActivity,
                severity: sev,
                summary: format!("active_weight={active_weight} ({pct:.0}%) < th66={t66}"),
            });
        }

        // Cert verify failures
        let current_cert_fails = self.cert_verify_fails_total;
        let prev_cert_fails = self.health_alert_state.prev_cert_verify_fails;
        if current_cert_fails > prev_cert_fails {
            findings.push(HealthFinding {
                kind: HealthFindingKind::CertVerifyFailures,
                severity: log::Level::Warn,
                summary: format!(
                    "cert_verify_fail delta={} total={}",
                    current_cert_fails - prev_cert_fails,
                    current_cert_fails
                ),
            });
        }

        // Standstill triggers
        let current_standstill =
            self.receiver_health_counters.standstill_triggers.load(Ordering::Relaxed);
        let prev_standstill = self.health_alert_state.prev_standstill_triggers;
        if current_standstill > prev_standstill {
            findings.push(HealthFinding {
                kind: HealthFindingKind::StandstillTriggers,
                severity: log::Level::Warn,
                summary: format!(
                    "standstill_triggers delta={} total={}",
                    current_standstill - prev_standstill,
                    current_standstill
                ),
            });
        }

        // Candidate giveups
        let current_giveups =
            self.receiver_health_counters.candidate_giveups.load(Ordering::Relaxed);
        let prev_giveups = self.health_alert_state.prev_candidate_giveups;
        if current_giveups > prev_giveups {
            findings.push(HealthFinding {
                kind: HealthFindingKind::CandidateGiveups,
                severity: log::Level::Warn,
                summary: format!(
                    "candidate_giveups delta={} total={}",
                    current_giveups - prev_giveups,
                    current_giveups
                ),
            });
        }

        // Skip vote dominance
        let delta_notar = self
            .votes_in_notarize_total
            .saturating_sub(self.health_alert_state.prev_votes_in_notarize);
        let delta_final = self
            .votes_in_finalize_total
            .saturating_sub(self.health_alert_state.prev_votes_in_finalize);
        let delta_skip =
            self.votes_in_skip_total.saturating_sub(self.health_alert_state.prev_votes_in_skip);
        let delta_total = delta_notar + delta_final + delta_skip;
        let skip_ratio_min_delta = (self.description.get_total_nodes() as u64).max(2) / 2;
        if delta_total >= skip_ratio_min_delta {
            let progress_votes = delta_notar + delta_final;
            let skip_to_progress = delta_skip as f64 / (progress_votes.max(1) as f64);
            if skip_to_progress >= 3.0 {
                let sev = if skip_to_progress >= 8.0 && progress_votes == 0 {
                    log::Level::Error
                } else {
                    log::Level::Warn
                };
                let skip_share = if delta_total > 0 {
                    100.0 * delta_skip as f64 / delta_total as f64
                } else {
                    0.0
                };
                findings.push(HealthFinding {
                    kind: HealthFindingKind::SkipVoteDominance,
                    severity: sev,
                    summary: format!(
                        "skip_share={skip_share:.0}% skip={delta_skip} notar={delta_notar} \
                        final={delta_final}"
                    ),
                });
            }
        }

        // Validator isolation
        let session_age = now.duration_since(self.session_creation_time()).unwrap_or_default();
        if session_age > Duration::from_secs(60) && active_weight <= 1 && total_weight > 1 {
            findings.push(HealthFinding {
                kind: HealthFindingKind::ValidatorIsolated,
                severity: log::Level::Error,
                summary: format!("only self active, session_age={:.0}s", session_age.as_secs_f64()),
            });
        }

        findings
    }

    /// Public health check dump for periodic monitoring
    ///
    /// Called from session main loop for periodic health checks.
    /// Runs anomaly detection and logs brief health status.
    pub fn health_check_dump(&mut self) {
        self.debug_dump(false);
        self.run_health_checks();
    }

    /// Run anomaly detection checks with cooldown-based deduplication.
    ///
    /// Each check emits a single-line WARN or ERROR log with the `SIMPLEX_HEALTH` prefix.
    /// Health warnings increment `simplex_health_warnings` (NOT `session_errors_count`).
    pub(crate) fn run_health_checks(&mut self) {
        let now = self.now();
        let session_id = self.session_id().to_hex_string();
        let session_prefix = &session_id[..8.min(session_id.len())];
        let cooldown = self.health_alert_state.cooldown;
        let skip_ratio_min_delta_votes = (self.description.get_total_nodes() as u64).max(2) / 2;
        const SKIP_RATIO_WARN_THRESHOLD: f64 = 3.0;
        const SKIP_RATIO_ERROR_THRESHOLD: f64 = 8.0;

        // 1. Progress gap: first_non_progressed - first_non_finalized > window size
        let first_non_finalized = self.simplex_state.get_first_non_finalized_slot().0;
        let first_non_progressed = self.simplex_state.get_first_non_progressed_slot().0;
        let window_size = self.description.opts().slots_per_leader_window;
        if first_non_progressed > first_non_finalized {
            let gap = first_non_progressed - first_non_finalized;
            if gap > window_size
                && now
                    .duration_since(self.health_alert_state.last_progress_warn)
                    .unwrap_or_default()
                    >= cooldown
            {
                self.health_alert_state.last_progress_warn = now;
                self.health_warnings_counter.increment(1);
                if gap > 2 * window_size {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=progress_gap session={session_prefix} gap={gap} \
                        first_non_finalized={first_non_finalized} \
                        first_non_progressed={first_non_progressed} window={window_size}",
                    );
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=progress_gap session={session_prefix} gap={gap} \
                        first_non_finalized={first_non_finalized} \
                        first_non_progressed={first_non_progressed} window={window_size}",
                    );
                }
            }
        }

        // 2. Zero finalization speed: finalized slot unchanged for too long
        let stall_warn_secs = self.description.opts().health_stall_warning_secs;
        let stall_err_secs = self.description.opts().health_stall_error_secs;
        let current_finalized = self.finalized_head_slot.map(|s| s.0 as f64).unwrap_or(0.0);
        if current_finalized != self.health_alert_state.prev_last_finalized_slot {
            self.health_alert_state.last_finalization_nonzero_at = now;
            self.health_alert_state.prev_last_finalized_slot = current_finalized;
        } else {
            let stall_duration = now
                .duration_since(self.health_alert_state.last_finalization_nonzero_at)
                .unwrap_or_default();
            if stall_duration >= Duration::from_secs(stall_warn_secs)
                && now
                    .duration_since(self.health_alert_state.last_finalization_speed_warn)
                    .unwrap_or_default()
                    >= cooldown
            {
                self.health_alert_state.last_finalization_speed_warn = now;
                self.health_warnings_counter.increment(1);
                if stall_duration >= Duration::from_secs(stall_err_secs) {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=zero_finalization_speed session={session_prefix} \
                        stall_secs={:.0} last_finalized_slot={current_finalized}",
                        stall_duration.as_secs_f64(),
                    );
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=zero_finalization_speed session={session_prefix} \
                        stall_secs={:.0} last_finalized_slot={current_finalized}",
                        stall_duration.as_secs_f64(),
                    );
                }
            }
        }

        // 3. Low activity: active_weight below thresholds
        let active_weight = self.active_weight;
        let total_weight = self.description.get_total_weight();
        let t66 = threshold_66(total_weight);
        if active_weight < t66
            && now.duration_since(self.health_alert_state.last_activity_warn).unwrap_or_default()
                >= cooldown
        {
            self.health_alert_state.last_activity_warn = now;
            self.health_warnings_counter.increment(1);
            let pct = if total_weight > 0 {
                (active_weight as f64 / total_weight as f64) * 100.0
            } else {
                0.0
            };
            let t33 = threshold_33(total_weight);
            if active_weight < t33 {
                log::error!(
                    "SIMPLEX_HEALTH anomaly=low_activity session={session_prefix} \
                    active_weight={active_weight} threshold_66={t66} pct={pct:.0}%"
                );
            } else {
                log::warn!(
                    "SIMPLEX_HEALTH anomaly=low_activity session={session_prefix} \
                    active_weight={active_weight} threshold_66={t66} pct={pct:.0}%"
                );
            }
        }

        // 4. Cert verify failures (delta-based)
        let current_cert_fails = self.cert_verify_fails_total;
        let prev_cert_fails = self.health_alert_state.prev_cert_verify_fails;
        if current_cert_fails > prev_cert_fails
            && now.duration_since(self.health_alert_state.last_cert_fail_warn).unwrap_or_default()
                >= cooldown
        {
            let delta = current_cert_fails - prev_cert_fails;
            self.health_alert_state.prev_cert_verify_fails = current_cert_fails;
            self.health_alert_state.last_cert_fail_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=cert_verify_fail session={} delta={} total={}",
                session_prefix,
                delta,
                current_cert_fails
            );
        }

        // 5. Standstill trigger rate (delta-based, from receiver)
        let current_standstill =
            self.receiver_health_counters.standstill_triggers.load(Ordering::Relaxed);
        let prev_standstill = self.health_alert_state.prev_standstill_triggers;
        if current_standstill > prev_standstill
            && now.duration_since(self.health_alert_state.last_standstill_warn).unwrap_or_default()
                >= cooldown
        {
            let delta = current_standstill - prev_standstill;
            self.health_alert_state.prev_standstill_triggers = current_standstill;
            self.health_alert_state.last_standstill_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=standstill_triggers session={} delta={} total={}",
                session_prefix,
                delta,
                current_standstill
            );
        }

        // 6. Candidate request giveups (delta-based, from receiver)
        let current_giveups =
            self.receiver_health_counters.candidate_giveups.load(Ordering::Relaxed);
        let prev_giveups = self.health_alert_state.prev_candidate_giveups;
        if current_giveups > prev_giveups
            && now
                .duration_since(self.health_alert_state.last_candidate_giveup_warn)
                .unwrap_or_default()
                >= cooldown
        {
            let delta = current_giveups - prev_giveups;
            self.health_alert_state.prev_candidate_giveups = current_giveups;
            self.health_alert_state.last_candidate_giveup_warn = now;
            self.health_warnings_counter.increment(1);
            log::warn!(
                "SIMPLEX_HEALTH anomaly=candidate_giveups session={} delta={} total={}",
                session_prefix,
                delta,
                current_giveups
            );
        }

        // 7. Skip/notar/final ratio anomaly (delta-based, inbound vote stream).
        let current_notar = self.votes_in_notarize_total;
        let current_final = self.votes_in_finalize_total;
        let current_skip = self.votes_in_skip_total;
        let delta_notar =
            current_notar.saturating_sub(self.health_alert_state.prev_votes_in_notarize);
        let delta_final =
            current_final.saturating_sub(self.health_alert_state.prev_votes_in_finalize);
        let delta_skip = current_skip.saturating_sub(self.health_alert_state.prev_votes_in_skip);
        let delta_total = delta_notar + delta_final + delta_skip;

        self.health_alert_state.prev_votes_in_notarize = current_notar;
        self.health_alert_state.prev_votes_in_finalize = current_final;
        self.health_alert_state.prev_votes_in_skip = current_skip;

        if delta_total >= skip_ratio_min_delta_votes
            && now.duration_since(self.health_alert_state.last_skip_ratio_warn).unwrap_or_default()
                >= cooldown
        {
            let progress_votes = delta_notar + delta_final;
            // Compare skip traffic against the full progress-vote stream to avoid
            // false positives when only one denominator is sparse in this window.
            let skip_to_progress = delta_skip as f64 / (progress_votes.max(1) as f64);
            let skip_to_notar = if delta_notar > 0 {
                delta_skip as f64 / (delta_notar as f64)
            } else {
                f64::INFINITY
            };
            let skip_to_final = if delta_final > 0 {
                delta_skip as f64 / (delta_final as f64)
            } else {
                f64::INFINITY
            };
            let skip_share = if delta_total > 0 {
                100.0 * (delta_skip as f64) / (delta_total as f64)
            } else {
                0.0
            };

            if skip_to_progress >= SKIP_RATIO_WARN_THRESHOLD {
                self.health_alert_state.last_skip_ratio_warn = now;
                self.health_warnings_counter.increment(1);

                if skip_to_progress >= SKIP_RATIO_ERROR_THRESHOLD && progress_votes == 0 {
                    log::error!(
                        "SIMPLEX_HEALTH anomaly=skip_vote_dominance session={session_prefix} \
                        delta_skip={delta_skip} delta_notar={delta_notar} delta_final={delta_final} \
                        skip_share={skip_share:.0}% skip_to_progress={skip_to_progress:.2} \
                        skip_to_notar={skip_to_notar:.2} \
                        skip_to_final={skip_to_final:.2}"
                    );
                    self.increment_error();
                } else {
                    log::warn!(
                        "SIMPLEX_HEALTH anomaly=skip_vote_dominance session={session_prefix} \
                        delta_skip={delta_skip} delta_notar={delta_notar} delta_final={delta_final} \
                        skip_share={skip_share:.0}% skip_to_progress={skip_to_progress:.2} \
                        skip_to_notar={skip_to_notar:.2} \
                        skip_to_final={skip_to_final:.2}"
                    );
                }
            }
        }

        // 8. Validator isolation: only self is active for extended period
        let isolation_threshold = Duration::from_secs(60);
        let session_age = now.duration_since(self.session_creation_time()).unwrap_or_default();
        if session_age > isolation_threshold
            && active_weight <= 1
            && total_weight > 1
            && now.duration_since(self.health_alert_state.last_isolation_warn).unwrap_or_default()
                >= Duration::from_secs(300)
        {
            self.health_alert_state.last_isolation_warn = now;
            self.health_warnings_counter.increment(1);
            let peers_never_seen = self
                .last_activity
                .iter()
                .enumerate()
                .filter(|(i, ts)| *i != self.description.get_self_idx().0 as usize && ts.is_none())
                .count();
            log::error!(
                "SIMPLEX_HEALTH anomaly=validator_isolated session={session_prefix} \
                active_weight={active_weight} total={total_weight} \
                session_age={:.0}s peers_never_seen={peers_never_seen}/{} — \
                possible validator key mismatch or overlay connectivity failure",
                session_age.as_secs_f64(),
                total_weight - 1,
            );
        }
    }

    /// Compute candidate funnel totals for validation inventory dump.
    fn compute_candidate_totals(&self, now: SystemTime) -> CandidateTotals {
        let received_total = self.received_candidates.len();
        let mut received_unvalidated = 0usize;
        let mut validated_not_notarized = 0usize;
        let mut notarized_not_finalized = 0usize;
        let mut finalized_recent = 0usize;
        let mut other_omitted = 0usize;

        for (id, _rc) in &self.received_candidates {
            let is_finalized = self.finalized_blocks.contains(id);
            let is_notarized =
                self.simplex_state.get_notarized_block_hash(&self.description, id.slot).as_ref()
                    == Some(&id.hash);
            let is_approved = self.approved.contains_key(id);
            let is_pending = self.pending_validations.contains_key(id);

            if is_finalized {
                let is_recent = self.finalized_pending_body.get(id).map_or_else(
                    || {
                        // Already materialized: check receive time as proxy
                        false
                    },
                    |entry| {
                        now.duration_since(entry.finalized_at)
                            .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
                            .unwrap_or(false)
                    },
                );
                // Also check if it was recently finalized by checking last_finalization_time proximity
                let recent_by_time = now
                    .duration_since(self.last_finalization_time)
                    .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
                    .unwrap_or(false);
                if is_recent || recent_by_time {
                    finalized_recent += 1;
                } else {
                    other_omitted += 1;
                }
            } else if is_notarized {
                notarized_not_finalized += 1;
            } else if is_approved || (!is_pending && !self.rejected.contains(id)) {
                validated_not_notarized += 1;
            } else {
                received_unvalidated += 1;
            }
        }

        CandidateTotals {
            received_total,
            received_unvalidated,
            validated_not_notarized,
            notarized_not_finalized,
            finalized_recent,
            other_omitted,
        }
    }

    /// Dump validation inventory with lifecycle-bucketed blocks.
    fn dump_validation_inventory(&self, r: &mut String, now: SystemTime, totals: &CandidateTotals) {
        r.push_str("  validation:\n");

        let mut received_rows = Vec::new();
        let mut validated_rows = Vec::new();
        let mut notarized_rows = Vec::new();
        let mut finalized_rows = Vec::new();

        for (id, rc) in &self.received_candidates {
            let is_finalized = self.finalized_blocks.contains(id);
            let is_notarized = self.simplex_state.has_notarized_block(id.slot);
            let is_approved = self.approved.contains_key(id);
            let is_pending = self.pending_validations.contains_key(id);
            let is_rejected = self.rejected.contains(id);

            let mut flags = Vec::new();
            if is_pending {
                flags.push("pending_validation");
            }
            if is_approved {
                flags.push("approved");
            }
            if is_rejected {
                flags.push("rejected");
            }
            if is_notarized {
                flags.push("notarized");
            }
            if is_finalized {
                flags.push("finalized");
            }
            if rc.is_empty {
                flags.push("empty");
            }
            let flags_str = if flags.is_empty() { "-".to_string() } else { flags.join(",") };

            let age = Self::fmt_ago(now, Some(rc.receive_time));
            let line = format!(
                "      slot {} src={} candidate={} block=({}) flags=[{}] recv={}\n",
                rc.slot,
                rc.source_idx,
                &id.hash.to_hex_string()[..8],
                rc.block_id,
                flags_str,
                age,
            );

            if is_finalized {
                let is_recent = now
                    .duration_since(self.last_finalization_time)
                    .map(|d| d <= RECENT_FINALIZED_DUMP_WINDOW)
                    .unwrap_or(false);
                if is_recent {
                    finalized_rows.push(line);
                }
                // older finalized: omitted
            } else if is_notarized {
                notarized_rows.push(line);
            } else if is_approved || (!is_pending && !is_rejected) {
                validated_rows.push(line);
            } else {
                received_rows.push(line);
            }
        }

        r.push_str(&format!("    received ({:.1}%):\n", totals.pct(totals.received_unvalidated),));
        for row in &received_rows {
            r.push_str(row);
        }

        r.push_str(&format!(
            "    validated ({:.1}%):\n",
            totals.pct(totals.validated_not_notarized),
        ));
        for row in &validated_rows {
            r.push_str(row);
        }

        r.push_str(&format!(
            "    notarized ({:.1}%):\n",
            totals.pct(totals.notarized_not_finalized),
        ));
        for row in &notarized_rows {
            r.push_str(row);
        }

        r.push_str(&format!("    finalized ({:.1}%):\n", totals.pct(totals.finalized_recent),));
        for row in &finalized_rows {
            r.push_str(row);
        }

        r.push_str(&format!(
            "    other: omitted={} total_received={}\n",
            totals.other_omitted, totals.received_total,
        ));
    }

    /// Produce detailed debug dump of session state
    ///
    /// Includes:
    /// - Stall conclusion with health findings
    /// - Session header, shard info, frontiers with cursor ages
    /// - Consensus milestone timestamps (finalization, notarization, cert times)
    /// - Heads (finalized, accepted, MC applied)
    /// - Candidate funnel statistics with percentages
    /// - Collation state with per-window grouping and leader identity
    /// - Validation inventory with lifecycle buckets
    /// - Peer diagnostics with typed vote/cert/candidate stats
    /// - Standstill diagnostic grid (on stall)
    ///
    /// # Arguments
    /// * `is_stalled` - If true, consensus is stalled (no finalizations for ROUND_DEBUG_PERIOD).
    ///   In stall mode, full details are logged to INFO level for immediate visibility.
    ///   In normal mode (health check), brief status goes to INFO, full details to DEBUG.
    ///
    /// Reference: validator-session/src/session_processor.rs debug_dump()
    fn debug_dump(&self, is_stalled: bool) {
        instrument!();

        let now = self.now();
        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        let fsm_first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        let slot_duration = now.duration_since(self.slot_started_at(fsm_first_non_progressed_slot));
        let total_weight = self.description.get_total_weight();
        let slot_dur_secs = slot_duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let session_time = now
            .duration_since(self.session_creation_time())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let shard = self.description.get_shard();

        if is_stalled {
            let time_since_finalization = now
                .duration_since(self.last_finalization_time)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            log::error!(
                "Session {} stalled (no finalizations for {:.1}s, slot_dur={:.1}s, threshold {:.0}s), \
                slot_nf={}, slot_np={}",
                &self.session_id().to_hex_string()[..8],
                time_since_finalization,
                slot_dur_secs,
                ROUND_DEBUG_PERIOD.as_secs_f64(),
                fsm_first_non_finalized_slot,
                fsm_first_non_progressed_slot
            );
            self.increment_error();
        }

        // INFO level: compact health status line
        if log::log_enabled!(log::Level::Info) {
            let status = if is_stalled { "STALLED" } else { "OK" };
            let head_seqno =
                self.finalized_head_seqno.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string());
            log::info!(
                "Session {} health [{}]: shard={}:{:016x} slot_nf={} slot_np={} finalized_head_seqno={}",
                &self.session_id().to_hex_string()[..8],
                status,
                shard.workchain_id(),
                shard.shard_prefix_with_tag(),
                fsm_first_non_finalized_slot,
                fsm_first_non_progressed_slot,
                head_seqno,
            );
        }

        let should_dump_full = if is_stalled {
            log::log_enabled!(log::Level::Info)
        } else {
            log::log_enabled!(log::Level::Debug)
        };

        if !should_dump_full {
            return;
        }

        let health_findings = self.collect_health_findings();
        let window_diags = self.simplex_state.collect_window_diagnostics(&self.description);

        let mut r = String::with_capacity(4096);
        let status_str = if is_stalled { "STALLED" } else { "OK" };

        // ---- Conclusion (stalled only) ----
        r.push_str(&format!(
            "Session {} dump [{}]:\n",
            self.session_id().to_hex_string(),
            status_str,
        ));
        if is_stalled {
            r.push_str("  conclusion:\n");
            for f in &health_findings {
                r.push_str(&format!("    - {:?}: {}\n", f.kind, f.summary));
            }
            // Frontier-based reason from first non-finalized slot diagnostic
            if let Some(wd) = window_diags.first() {
                if let Some(sd) = wd.slots.first() {
                    r.push_str(&format!(
                        "    - frontier_reason: slot {} {} ({})\n",
                        sd.slot, sd.phase, sd.reason
                    ));
                }
            }
            if health_findings.is_empty() {
                r.push_str("    - none\n");
            }
        }

        // ---- Shard ----
        r.push_str(&format!(
            "  shard={}:{:016x}\n",
            shard.workchain_id(),
            shard.shard_prefix_with_tag(),
        ));

        // ---- Header ----
        let t66 = threshold_66(total_weight);
        let t33 = threshold_33(total_weight);
        let active_pct = if total_weight > 0 {
            100.0 * self.active_weight as f64 / total_weight as f64
        } else {
            0.0
        };
        r.push_str("  header:\n");
        r.push_str(&format!(
            "    validators={} local={} session_time={:.1}s slot_duration={:.1}s\n",
            self.description.get_total_nodes(),
            self.description.get_self_idx(),
            session_time,
            slot_dur_secs,
        ));
        r.push_str(&format!(
            "    total_weight={total_weight} th66={t66} th33={t33} active_weight={} ({active_pct:.1}%)\n",
            self.active_weight,
        ));

        // ---- Frontiers ----
        let nf_age = Self::fmt_dur(now, self.observability.last_finalized_cursor_change_at);
        let np_age = Self::fmt_dur(now, self.observability.last_progression_change_at);
        r.push_str("  frontiers:\n");
        r.push_str(&format!(
            "    first_non_finalized={} (unchanged {})\n",
            fsm_first_non_finalized_slot, nf_age,
        ));
        r.push_str(&format!(
            "    first_non_progressed={} (unchanged {})\n",
            fsm_first_non_progressed_slot, np_age,
        ));

        // Milestone timestamps
        let fmt_milestone =
            |label: &str, seqno: Option<u32>, slot: Option<SlotIndex>, ts: Option<SystemTime>| {
                let seqno_str =
                    seqno.map(|s| format!("seqno={s}")).unwrap_or_else(|| "seqno=?".to_string());
                let slot_str = slot.map(|s| format!(" slot={s}")).unwrap_or_default();
                let age = Self::fmt_ago(now, ts);
                format!("    {label}: {seqno_str}{slot_str}, {age}\n")
            };
        r.push_str(&fmt_milestone(
            "last_finalization",
            self.finalized_head_seqno,
            self.finalized_head_slot,
            Some(self.last_finalization_time),
        ));
        r.push_str(&fmt_milestone(
            "last_notarization",
            None,
            self.observability.last_notarization_slot,
            self.observability.last_notarization_at,
        ));
        r.push_str(&fmt_milestone(
            "last_final_cert",
            None,
            self.observability.last_final_cert_slot,
            self.observability.last_final_cert_at,
        ));
        r.push_str(&fmt_milestone(
            "last_notar_cert",
            None,
            self.observability.last_notar_cert_slot,
            self.observability.last_notar_cert_at,
        ));

        // ---- Heads ----
        r.push_str("  heads:\n");
        r.push_str(&format!(
            "    finalized_head_seqno={}\n",
            self.finalized_head_seqno.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
        ));
        if let Some(ref bid) = self.finalized_head_block_id {
            r.push_str(&format!(
                "    finalized_head=slot {} id=({})\n",
                self.finalized_head_slot.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
                bid,
            ));
        }
        r.push_str(&format!(
            "    last_consensus_finalized_seqno={}\n",
            self.last_consensus_finalized_seqno
                .map(|s| s.to_string())
                .unwrap_or_else(|| "?".to_string()),
        ));
        if let Some(ref bid) = self.accepted_normal_head_block_id {
            r.push_str(&format!(
                "    accepted_normal_head=seqno {} id=({})\n",
                self.accepted_normal_head_seqno, bid
            ));
        } else {
            r.push_str(&format!(
                "    accepted_normal_head=seqno {}\n",
                self.accepted_normal_head_seqno
            ));
        }
        if let Some(ref bid) = self.observability.last_mc_applied_block_id {
            r.push_str(&format!("    last_mc_applied=({})\n", bid));
        }
        r.push_str(&format!(
            "    last_mc_finalized_seqno={}\n",
            self.last_mc_finalized_seqno.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
        ));

        // ---- Statistics ----
        let totals = self.compute_candidate_totals(now);
        r.push_str("  statistics:\n");
        r.push_str(&format!(
            "    candidates: received={} validated={} ({:.1}%) notarized={} \
            ({:.1}%) finalized={} ({:.1}%) other={} ({:.1}%)\n",
            totals.received_total,
            totals.received_total - totals.received_unvalidated,
            totals.pct(totals.received_total - totals.received_unvalidated),
            totals.notarized_not_finalized + totals.finalized_recent + totals.other_omitted,
            totals.pct(
                totals.notarized_not_finalized + totals.finalized_recent + totals.other_omitted
            ),
            totals.finalized_recent,
            totals.pct(totals.finalized_recent),
            totals.other_omitted,
            totals.pct(totals.other_omitted),
        ));
        if let Some(ref snap) = self.last_receiver_snapshot {
            let total_in_msgs: u64 = snap.sources.iter().map(|s| s.in_messages).sum();
            let total_out_msgs: u64 = snap.sources.iter().map(|s| s.out_messages).sum();
            let total_in_bcasts: u64 = snap.sources.iter().map(|s| s.in_broadcasts).sum();
            let total_out_bcasts: u64 = snap.sources.iter().map(|s| s.out_broadcasts).sum();
            let total_dup_votes: u64 = snap.sources.iter().map(|s| s.duplicate_votes).sum();
            let total_dup_bcasts: u64 = snap.sources.iter().map(|s| s.duplicate_broadcasts).sum();
            let total_req_sent: u64 = snap.sources.iter().map(|s| s.candidate_requests_sent).sum();
            let total_req_recv: u64 =
                snap.sources.iter().map(|s| s.candidate_requests_received).sum();
            r.push_str(&format!(
                "    traffic: msgs_in={total_in_msgs} msgs_out={total_out_msgs} \
                bcasts_in={total_in_bcasts} bcasts_out={total_out_bcasts}\n"
            ));
            r.push_str(&format!(
                "    votes_in: notar={} final={} skip={}\n",
                self.votes_in_notarize_total,
                self.votes_in_finalize_total,
                self.votes_in_skip_total,
            ));
            r.push_str(&format!(
                "    duplicates: votes={total_dup_votes} broadcasts={total_dup_bcasts} \
                request_candidates_sent={total_req_sent} request_candidates_recv={total_req_recv}\n"
            ));
        }
        r.push_str(&format!(
            "    pending: validations={} approvals={} rejections={} finalized_pending_body={}\n",
            self.pending_validations.len(),
            self.approved.len(),
            self.rejected.len(),
            self.finalized_pending_body.len(),
        ));

        // ---- Collation (per-window) ----
        let current_slot = fsm_first_non_progressed_slot;
        r.push_str("  collation:\n");
        r.push_str(&format!(
            "    current_slot={} pending_gen={} generated={} sent_gen={} precollated={} \
            generated_waiting_validation={}\n",
            current_slot,
            self.slot_is_pending_generate(current_slot),
            self.slot_is_generated(current_slot),
            self.slot_is_sent_generated(current_slot),
            self.precollated_blocks.len(),
            self.generated_candidates_waiting_validation.len(),
        ));
        for wd in &window_diags {
            let leader_pubkey =
                base64_encode(self.description.get_source_public_key_hash(wd.leader_idx).data());
            let leader_adnl =
                base64_encode(self.description.get_source_adnl_id(wd.leader_idx).data());
            r.push_str(&format!(
                "    window {} slots=[{}..{}] leader={} pubkey_b64={} adnl_b64={}\n",
                wd.window_idx,
                wd.slot_begin,
                wd.slot_end,
                wd.leader_idx,
                leader_pubkey,
                leader_adnl,
            ));
            for sd in &wd.slots {
                let mut flags = Vec::new();
                if sd.voted_notar {
                    flags.push("Voted");
                }
                if sd.voted_skip {
                    flags.push("VotedSkip");
                }
                if sd.voted_final {
                    flags.push("VotedFinal");
                }
                if sd.has_pending_block {
                    flags.push("Pending");
                }
                if sd.is_timeout_skipped {
                    flags.push("TimeoutSkipped");
                }
                let flags_str = if flags.is_empty() { "none".to_string() } else { flags.join("|") };
                let mut certs = Vec::new();
                if sd.has_notar_cert {
                    certs.push("notar");
                }
                if sd.has_final_cert {
                    certs.push("final");
                }
                if sd.has_skip_cert {
                    certs.push("skip");
                }
                let certs_str = if certs.is_empty() { "none".to_string() } else { certs.join("|") };
                r.push_str(&format!(
                    "      {} phase={} reason={} notar={:.0}% final={:.0}% \
                    skip={:.0}% flags=[{}] certs=[{}]\n",
                    sd.slot,
                    sd.phase,
                    sd.reason,
                    sd.notar_weight_pct,
                    sd.final_weight_pct,
                    sd.skip_weight_pct,
                    flags_str,
                    certs_str,
                ));
            }
        }

        // ---- Validation inventory ----
        self.dump_validation_inventory(&mut r, now, &totals);

        // ---- Peers ----
        if let Some(ref snap) = self.last_receiver_snapshot {
            r.push_str("  peers:\n");
            for src in &snap.sources {
                let is_self = src.source_idx == self.description.get_self_idx().0 as u32;
                let vi = ValidatorIndex::from(src.source_idx);
                let weight = self.description.get_node_weight(vi);
                let weight_pct = if total_weight > 0 {
                    100.0 * weight as f64 / total_weight as f64
                } else {
                    0.0
                };
                let pubkey_b64 =
                    base64_encode(self.description.get_source_public_key_hash(vi).data());
                let last_act = Self::fmt_ago(now, src.last_recv_time);
                let last_vote = Self::fmt_ago(now, src.last_vote_recv_time);
                let last_final_cert = Self::fmt_ago(now, src.last_final_cert_recv_time);
                let last_notar_cert = Self::fmt_ago(now, src.last_notar_cert_recv_time);
                let last_cand = Self::fmt_ago(now, src.last_candidate_recv_time);
                let marker = if is_self { " (self)" } else { "" };
                r.push_str(&format!(
                    "    {} adnl_b64={} pubkey_b64={} weight={} ({weight_pct:.1}%) \
                    last_activity={last_act} last_vote={last_vote} last_final_cert={last_final_cert} \
                    last_notar_cert={last_notar_cert} last_candidate={last_cand} \
                    votes[n/f/s]={}/{}/{} certs[n/f/s]={}/{}/{} candidates={} \
                    req[s/r]={}/{}{marker}\n",
                    vi,
                    src.adnl_id_base64,
                    pubkey_b64,
                    weight,
                    src.votes_in_notarize,
                    src.votes_in_finalize,
                    src.votes_in_skip,
                    src.certs_in_notar,
                    src.certs_in_final,
                    src.certs_in_skip,
                    src.candidates_received,
                    src.candidate_requests_sent,
                    src.candidate_requests_received,
                ));
            }
        }

        // ---- Health findings ----
        if !health_findings.is_empty() {
            r.push_str("  health_findings:\n");
            for f in &health_findings {
                r.push_str(&format!("    - [{:?}] {:?}: {}\n", f.severity, f.kind, f.summary));
            }
        }

        // ---- Standstill diagnostic (C++ parity, stall only) ----
        if is_stalled {
            let diagnostic = self.simplex_state.standstill_diagnostic_dump(&self.description);
            if !diagnostic.is_empty() {
                r.push_str("  standstill_diagnostic:\n");
                for line in diagnostic.lines() {
                    r.push_str(&format!("    {}\n", line));
                }
            }
        }

        if is_stalled {
            log::error!("{}", r);
        } else {
            log::debug!("{}", r);
        }
    }

    /*
        Core consensus operations
    */

    /// Arm FSM timeouts and prepare the processor for the main loop.
    ///
    /// Must be called exactly once, after overlay warmup and bootstrap
    /// recovery, right before the main loop begins.  The FSM is created
    /// with unarmed timeouts (`skip_timestamp = None`) so that no skip
    /// cascade fires during the startup delay.
    ///
    /// C++ reference: `start_up()` initialises state and processes
    /// bootstrap votes; timeouts are armed through the event flow
    /// (`LeaderWindowObserved` → `alarm_timestamp()`).  In Rust the
    /// equivalent arming point is this explicit `start()` call.
    pub(crate) fn start(&mut self) {
        self.simplex_state.reset_timeouts_on_start(&self.description);

        log::info!(
            "Session {} started: skip timeouts armed",
            &self.session_id().to_hex_string()[..8],
        );
    }

    /// Check all pending operations
    ///
    /// Called periodically from main loop when awake time is reached.
    /// Implements the core consensus event loop.
    ///
    /// Reference: validator-session/src/session_processor.rs check_all
    pub fn check_all(&mut self) {
        check_execution_time!(10_000);

        // Increment metrics counter
        self.check_all_counter.increment(1);

        let now = self.now();
        let wake_slip = now.duration_since(self.next_awake_time).unwrap_or_default();
        self.check_all_wake_slip_histogram.record(wake_slip.as_millis() as f64);

        // Reset awake time to far future, will be updated by various checks
        self.reset_next_awake_time();

        // Stalled consensus detection
        // Debug dump if no finalizations for ROUND_DEBUG_PERIOD (stalled consensus)
        if now >= self.round_debug_at {
            self.debug_dump(true); // is_stalled=true: full dump to INFO level
            self.round_debug_at = now + ROUND_DEBUG_PERIOD;
        }

        // Release delayed gates before evaluating validation readiness so retries
        // can re-enter validation in the same `check_all()` pass.
        self.process_delayed_actions();

        // Check validation (process pending validations)
        self.check_validation();

        // Feed validated candidates to FSM BEFORE timeout processing so that
        // the FSM has all available candidates before it evaluates timeouts
        // (mirrors C++ where process_blocks() feeds candidates before the
        // round timer is checked).
        self.process_validated_candidates();

        // Call SimplexState FSM check_all (processes timeouts, pending blocks)
        self.simplex_state.check_all(&self.description);

        // Process all events produced by FSM
        self.process_simplex_events();

        // Keep receiver standstill slots aligned even when the FSM tracked
        // interval changes outside the finalization / skip hooks.
        self.sync_standstill_slots_from_state();

        // Update awake time from FSM timeout
        if let Some(fsm_timeout) = self.simplex_state.get_next_timeout() {
            self.set_next_awake_time(fsm_timeout);
        }

        // Ensure we wake up for stall detection even when FSM has no pending timeouts
        self.set_next_awake_time(self.round_debug_at);

        // Persist pool state (first_nonannounced_window) when window advances
        self.maybe_store_pool_state();

        // Check collation (am I leader? should I generate?)
        self.check_collation();

        let cur_nf = self.simplex_state.get_first_non_finalized_slot();
        let cur_np = self.simplex_state.get_first_non_progressed_slot();
        if cur_nf != self.observability.prev_first_non_finalized {
            self.observability.last_finalized_cursor_change_at = now;
            self.observability.prev_first_non_finalized = cur_nf;
        }
        if cur_np != self.observability.prev_first_non_progressed {
            self.observability.last_progression_change_at = now;
            self.observability.prev_first_non_progressed = cur_np;
        }
        self.first_non_finalized_slot_gauge.set(cur_nf.0 as f64);
        self.first_non_progressed_slot_gauge.set(cur_np.0 as f64);

        // Debug state dump
        self.log_consensus_state("check_all");
    }

    /// Stop the session processor
    pub fn stop(&mut self, _destroy_db: bool) {
        log::info!("Stopping SessionProcessor for session {}", self.session_id().to_hex_string());

        // Stop receiver
        self.receiver.stop();

        // Cancel pending precollations
        self.reset_precollations();

        // TODO: Optionally destroy database
    }

    /*
        Collation management
        Reference: validator-session/src/session_processor.rs collation methods

        ┌─────────────────────────────────────────────────────────────────────────────────┐
        │ Collation Flow                                                                   │
        │                                                                                 │
        │  1. check_collation()                                                           │
        │     ├── Am I leader for current slot? (description.get_leader(slot))            │
        │     ├── Have I already generated? (pending_generate || generated)               │
        │     └── Check for precollated block first                                       │
        │                                                                                 │
        │  2. If no precollated block:                                                    │
        │     ├── pending_generate = true                                                 │
        │     └── invoke_collation(slot) → notify_generate_slot(source_info, request, cb) │
        │                                                                                 │
        │  3. Callback receives candidate from higher layer:                              │
        │     └── on_collation_complete(slot, request_id, candidate)                      │
        │                                                                                 │
        │  4. generated_block():                                                          │
        │     ├── Validate sizes, compute candidate hash                                  │
        │     ├── Sign candidate: utils::sign_candidate()                                 │
        │     ├── Broadcast via receiver.send_block_broadcast()                           │
        │     ├── Convert to Candidate for FSM                                            │
        │     └── simplex_state.on_candidate(&desc, candidate)                            │
        └─────────────────────────────────────────────────────────────────────────────────┘
    */

    /// Resolve full parent `BlockIdExt` from FSM parent info.
    ///
    /// Simplex FSM tracks parents as `(slot, candidate_id_hash)` only (`CandidateParentInfo`).
    /// For `CollationParentHint::Explicit(...)` we must provide the full `BlockIdExt`.
    ///
    /// Resolution uses `received_candidates` (keyed by RawCandidateId=(slot, hash)) which stores
    /// the `BlockIdExt` for each received candidate.
    ///
    /// Returns None if the parent candidate is not yet received (body missing).
    fn resolve_parent_block_id(
        &self,
        parent: &crate::block::CandidateParentInfo,
    ) -> Option<BlockIdExt> {
        let parent_id = RawCandidateId { slot: parent.slot, hash: parent.hash.clone() };
        // Check synchronous generated-parent cache first (populated in generated_block()
        // before the async on_candidate_received self-loop), then fall back to the
        // received_candidates map which is populated asynchronously.
        self.generated_parent_cache
            .get(&parent_id)
            .cloned()
            .or_else(|| self.received_candidates.get(&parent_id).map(|c| c.block_id.clone()))
    }

    #[allow(dead_code)]
    fn resolve_candidate_id_by_block_id(&self, block_id: &BlockIdExt) -> Option<RawCandidateId> {
        self.generated_parent_cache
            .iter()
            .find_map(|(candidate_id, candidate_block_id)| {
                (candidate_block_id == block_id).then_some(candidate_id.clone())
            })
            .or_else(|| {
                self.received_candidates.iter().find_map(|(candidate_id, received)| {
                    (&received.block_id == block_id).then_some(candidate_id.clone())
                })
            })
    }

    /// Resolve the best-known `before_split` flag for the current collation parent.
    ///
    /// C++ parity: `block-producer.cpp::should_generate_empty_block()` uses
    /// `state->is_before_split()` where `state` is the resolved parent state for the
    /// slot being generated (not necessarily the latest finalized head).
    fn resolve_parent_before_split_flag(
        &self,
        parent: Option<&crate::block::CandidateParentInfo>,
        prev_block_ids: &[BlockIdExt],
    ) -> Option<bool> {
        let parent_block_id = match parent {
            Some(parent_info) => self.resolve_parent_block_id(parent_info),
            None if prev_block_ids.len() == 1 => Some(prev_block_ids[0].clone()),
            _ => None,
        }?;

        self.before_split_by_block_id.get(&parent_block_id).copied().or_else(|| {
            self.finalized_head_block_id
                .as_ref()
                .filter(|finalized| *finalized == &parent_block_id)
                .map(|_| self.finalized_head_before_split)
        })
    }

    fn resolve_collation_prev_block_ids(
        &self,
        parent: Option<&crate::block::CandidateParentInfo>,
    ) -> Option<Vec<BlockIdExt>> {
        match parent {
            Some(parent_info) => Some(vec![self.resolve_parent_block_id(parent_info)?]),
            None => Some(self.session_start_prev_blocks.clone()),
        }
    }

    fn prepare_collation(
        &self,
        _slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
    ) -> CollationPreparation {
        let Some(prev_block_ids) = self.resolve_collation_prev_block_ids(parent.as_ref()) else {
            return CollationPreparation::WaitingForParent;
        };

        let slot_start_time = self.compute_collation_start_time(parent.as_ref());
        let now = self.now();
        if slot_start_time > now {
            return CollationPreparation::Deferred(slot_start_time);
        }

        let new_seqno = prev_block_ids.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1;
        let is_first_session_block = parent.is_none();

        CollationPreparation::Ready(PreparedCollation {
            prev_block_ids,
            slot_start_time,
            new_seqno,
            is_first_session_block,
        })
    }

    fn try_begin_collation_slot(
        &self,
        slot: SlotIndex,
        attempt: CollationAttempt,
    ) -> Option<ValidatorIndex> {
        if self.precollated_blocks.contains_key(&slot) {
            log::trace!(
                "Session {} {}: slot {} already pending",
                self.session_id().to_hex_string(),
                attempt.log_context(),
                slot
            );
            return None;
        }

        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(slot);
        if leader != self_idx {
            log::trace!(
                "Session {} {}: not leader for slot {} (leader={})",
                self.session_id().to_hex_string(),
                attempt.log_context(),
                slot,
                leader
            );
            return None;
        }

        Some(self_idx)
    }

    fn update_max_precollated_slot(&mut self, slot: SlotIndex) {
        if self.precollated_blocks_max_slot.map_or(true, |max| slot > max) {
            self.precollated_blocks_max_slot = Some(slot);
        }
    }

    fn create_pending_collation_request(
        &mut self,
        slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
        slot_start_time: SystemTime,
    ) -> (u32, Arc<AsyncRequestImpl>) {
        self.update_max_precollated_slot(slot);

        let request_id = self.precollated_blocks_next_request_id;
        self.precollated_blocks_next_request_id += 1;

        let request = AsyncRequestImpl::new(request_id, true, slot_start_time);
        let precollated_block = PrecollatedBlock { request: request.clone(), result: None, parent };
        self.precollated_blocks.insert(slot, precollated_block);

        (request_id, request)
    }

    fn finish_empty_collation_attempt(
        &mut self,
        slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
        slot_start_time: SystemTime,
        parent_block_id: BlockIdExt,
    ) {
        // NOTE: empty-block fast path is generated by simplex itself (no
        // `on_generate_slot()` dispatch to the validator), so this is NOT a
        // self-collation and must not be tracked by `simplex_self_collates`.
        let (request_id, _request) =
            self.create_pending_collation_request(slot, parent, slot_start_time);
        self.record_collation_start();
        self.on_collation_complete(slot, request_id, CollationResult::Empty { parent_block_id });
        self.update_collation_pacing();
    }

    fn make_roundless_collation_source_info(
        &self,
        self_idx: ValidatorIndex,
    ) -> crate::BlockSourceInfo {
        crate::BlockSourceInfo {
            source: self.description.get_source_public_key(self_idx).clone(),
            priority: BlockCandidatePriority {
                round: SIMPLEX_ROUNDLESS,
                first_block_round: SIMPLEX_ROUNDLESS,
                priority: 0,
            },
        }
    }

    fn make_collation_callback(
        &self,
        slot: SlotIndex,
        request_id: u32,
        request: Arc<AsyncRequestImpl>,
        attempt: CollationAttempt,
    ) -> crate::ValidatorBlockCandidateCallback {
        let session_id = self.session_id().clone();
        let description = self.description.clone();
        let collation_latency_histogram = self.collation_latency_histogram.clone();
        let start_time = self.now();
        let task_queue = self.task_queue.clone();
        let request_clone = request.clone();
        let attempt_label = Self::collation_attempt_label(attempt);

        Box::new(move |result: Result<ValidatorBlockCandidatePtr>| {
            if request_clone.is_cancelled() {
                log::warn!(
                    "Session {} {}: request {} for slot {} was cancelled",
                    session_id.to_hex_string(),
                    attempt.log_context(),
                    request_id,
                    slot
                );
                crate::task_queue::post_closure(
                    &task_queue,
                    move |processor: &mut SessionProcessor| {
                        processor.forget_self_collation_tracking(
                            slot,
                            "callback_cancelled_before_generation_result",
                        );
                    },
                );
                return;
            }

            let generation_duration =
                description.get_time().duration_since(start_time).unwrap_or_default();
            collation_latency_histogram.record(generation_duration.as_millis() as f64);

            if generation_duration > MAX_GENERATION_TIME {
                log::warn!(
                    "Session {} {}: block generation took {:.3}s (expected <{:.3}s) for slot {}",
                    session_id.to_hex_string(),
                    attempt.log_context(),
                    generation_duration.as_secs_f64(),
                    MAX_GENERATION_TIME.as_secs_f64(),
                    slot
                );
            }

            let session_id_clone = session_id.clone();
            crate::task_queue::post_closure(
                &task_queue,
                move |processor: &mut SessionProcessor| {
                    let expected_block_id = processor
                        .self_collation_starts_by_slot
                        .get(&slot)
                        .map(|(_, exp)| processor.format_expected_block_id(*exp))
                        .unwrap_or_else(|| "unknown".to_string());
                    match result {
                        Ok(candidate) => {
                            log::info!(
                                "Session {} COLLATION_FLOW callback: expected_block_id={} slot={} \
                                attempt={} outcome=generated generation_ms={} candidate_block_id={}",
                                &session_id_clone.to_hex_string()[..8],
                                expected_block_id,
                                slot,
                                attempt_label,
                                generation_duration.as_millis(),
                                candidate.id,
                            );
                            match attempt {
                                CollationAttempt::Initial => {
                                    log::trace!(
                                        "Session {} invoke_collation: block generated for slot {} \
                                        (request_id={})",
                                        session_id_clone.to_hex_string(),
                                        slot,
                                        request_id
                                    );
                                }
                                CollationAttempt::Retry { retry_count } => {
                                    log::trace!(
                                        "Session {} invoke_collation_retry: block generated for \
                                        slot {} (request_id={}, retry={})",
                                        session_id_clone.to_hex_string(),
                                        slot,
                                        request_id,
                                        retry_count
                                    );
                                }
                            }
                            processor.on_collation_complete(
                                slot,
                                request_id,
                                CollationResult::Block(candidate),
                            );
                        }
                        Err(err) => {
                            log::info!(
                                "Session {} COLLATION_FLOW callback: expected_block_id={} slot={} \
                                attempt={} outcome=callback_failure generation_ms={} error={}",
                                &session_id_clone.to_hex_string()[..8],
                                expected_block_id,
                                slot,
                                attempt_label,
                                generation_duration.as_millis(),
                                err,
                            );
                            match attempt {
                                CollationAttempt::Initial => {
                                    log::warn!(
                                        "Session {} invoke_collation: block generation failed for \
                                        slot {slot}: {err}",
                                        session_id_clone.to_hex_string(),
                                    );
                                }
                                CollationAttempt::Retry { retry_count } => {
                                    log::warn!(
                                        "Session {} invoke_collation_retry: block generation \
                                        failed for slot {} (retry={}): {}",
                                        session_id_clone.to_hex_string(),
                                        slot,
                                        retry_count,
                                        err
                                    );
                                }
                            }
                            processor.on_collation_failed_impl(
                                slot,
                                request_id,
                                err,
                                attempt.failure_retry_count(),
                            );
                        }
                    }
                },
            );
        })
    }

    fn dispatch_collation_request(
        &mut self,
        slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
        prev_block_ids: Vec<BlockIdExt>,
        slot_start_time: SystemTime,
        new_seqno: u32,
        self_idx: ValidatorIndex,
        attempt: CollationAttempt,
    ) {
        let (request_id, request) =
            self.create_pending_collation_request(slot, parent.clone(), slot_start_time);

        self.precollation_requests_counter.increment(1);
        self.update_collation_pacing();
        self.collates_expire_counter.total_increment();

        match attempt {
            CollationAttempt::Initial => {
                log::debug!(
                    "Session {} COLLATION request: slot={}, expected_seqno={}, parent={:?}",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    new_seqno,
                    parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
                );
                log::trace!(
                    "Session {} invoke_collation: requesting block for slot={slot}, \
                    expected_seqno={new_seqno}, request_id={request_id}",
                    self.session_id().to_hex_string(),
                );
            }
            CollationAttempt::Retry { retry_count } => {
                log::trace!(
                    "Session {} invoke_collation_retry: requesting block for slot {} \
                    (request_id={}, retry={}/{}, parent={:?})",
                    self.session_id().to_hex_string(),
                    slot,
                    request_id,
                    retry_count,
                    self.description.opts().collation_retry_max_attempts,
                    parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
                );
            }
        }

        let source_info = self.make_roundless_collation_source_info(self_idx);
        let callback = self.make_collation_callback(slot, request_id, request.clone(), attempt);

        self.record_collation_start();
        self.record_self_collation_start(
            slot,
            new_seqno,
            attempt,
            parent.as_ref(),
            &prev_block_ids,
        );
        self.collates_counter.total_increment();
        self.notify_generate_slot(slot, source_info, request, parent, prev_block_ids, callback);
    }

    fn execute_collation_attempt(
        &mut self,
        slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
        self_idx: ValidatorIndex,
        attempt: CollationAttempt,
        clear_pending_generate_on_not_ready: bool,
        enforce_progress_slot_invariant: bool,
    ) {
        let prepared = match self.prepare_collation(slot, parent.clone()) {
            CollationPreparation::Ready(prepared) => prepared,
            CollationPreparation::Deferred(slot_start_time) => {
                log::trace!(
                    "Session {} {}: deferring slot {} until {:?}",
                    self.session_id().to_hex_string(),
                    attempt.log_context(),
                    slot,
                    slot_start_time
                );
                if clear_pending_generate_on_not_ready {
                    self.slot_set_pending_generate(slot, false);
                }
                self.set_next_awake_time(slot_start_time);
                return;
            }
            CollationPreparation::WaitingForParent => {
                if let Some(parent_info) = parent.as_ref() {
                    log::trace!(
                        "Session {} {}: waiting for resolved parent BlockIdExt for slot {slot} \
                        (parent={parent_info})",
                        self.session_id().to_hex_string(),
                        attempt.log_context(),
                    );
                }
                if clear_pending_generate_on_not_ready {
                    self.slot_set_pending_generate(slot, false);
                }
                return;
            }
        };

        let prev_block_ids = prepared.prev_block_ids.clone();
        let slot_start_time = prepared.slot_start_time;
        let new_seqno = prepared.new_seqno;
        let is_first_session_block = prepared.is_first_session_block;
        let parent_before_split =
            self.resolve_parent_before_split_flag(parent.as_ref(), &prev_block_ids);

        if self.should_generate_empty_block(slot, new_seqno, parent_before_split) {
            assert!(
                !is_first_session_block,
                "Session {} INVARIANT VIOLATION: {} should_generate_empty_block({}) returned \
                true but no parent available. First block in epoch cannot be empty. \
                finalized_head_seqno={:?}, last_mc_finalized_seqno={:?}",
                self.session_id().to_hex_string(),
                attempt.log_context(),
                new_seqno,
                self.finalized_head_seqno,
                self.last_mc_finalized_seqno
            );
            let parent_block_id = prev_block_ids.first().cloned().expect(
                "non-first Simplex collation attempt must have one resolved parent block id",
            );
            match attempt {
                CollationAttempt::Initial => {
                    log::debug!(
                        "Session {} invoke_collation: generating EMPTY block for slot {}! \
                        new_seqno={}, finalized_head_seqno={:?}, last_mc_finalized_seqno={:?}",
                        self.session_id().to_hex_string(),
                        slot,
                        new_seqno,
                        self.finalized_head_seqno,
                        self.last_mc_finalized_seqno
                    );
                }
                CollationAttempt::Retry { .. } => {
                    log::debug!(
                        "Session {} invoke_collation_retry: generating EMPTY block for slot {} \
                        on retry! new_seqno={}, finalized_head_seqno={:?}, last_mc_finalized_seqno={:?}",
                        self.session_id().to_hex_string(),
                        slot,
                        new_seqno,
                        self.finalized_head_seqno,
                        self.last_mc_finalized_seqno
                    );
                }
            }
            self.finish_empty_collation_attempt(slot, parent, slot_start_time, parent_block_id);
            return;
        }

        if enforce_progress_slot_invariant {
            let first_non_progressed = self.simplex_state.get_first_non_progressed_slot();
            assert!(
                slot >= first_non_progressed,
                "SessionProcessor INVARIANT VIOLATION: invoke_collation for slot {} \
                but first_non_progressed_slot is {} (cannot generate for progressed slot)",
                slot,
                first_non_progressed
            );
        }

        self.dispatch_collation_request(
            slot,
            parent,
            prev_block_ids,
            slot_start_time,
            new_seqno,
            self_idx,
            attempt,
        );
    }

    fn resolve_parent_gen_utime_ms(
        &self,
        parent: &crate::block::CandidateParentInfo,
    ) -> Option<u64> {
        let parent_id = RawCandidateId { slot: parent.slot, hash: parent.hash.clone() };
        self.generated_parent_gen_utime_ms_cache
            .get(&parent_id)
            .copied()
            .or_else(|| self.received_candidates.get(&parent_id).and_then(|c| c.gen_utime_ms))
            .or_else(|| {
                self.local_chain_head.as_ref().and_then(|head| {
                    if head.parent_info.slot == parent.slot && head.parent_info.hash == parent.hash
                    {
                        head.gen_utime_ms
                    } else {
                        None
                    }
                })
            })
    }

    fn compute_collation_start_time(
        &self,
        parent: Option<&crate::block::CandidateParentInfo>,
    ) -> SystemTime {
        let now = self.now();
        let Some(parent) = parent else {
            return now;
        };
        let Some(parent_gen_utime_ms) = self.resolve_parent_gen_utime_ms(parent) else {
            return now;
        };

        let target_rate = self.description.opts().target_rate;
        let earliest_from_parent = UNIX_EPOCH
            .checked_add(Duration::from_millis(parent_gen_utime_ms).saturating_add(target_rate))
            .unwrap_or(now);
        let latest_reasonable = now.checked_add(target_rate).unwrap_or(now);
        earliest_from_parent.max(now).min(latest_reasonable)
    }

    /// Advance `earliest_collation_time` by `target_rate` from now.
    /// Called after every collation start (invoke, retry, precollated hit,
    /// empty-block short-circuit) to pace the next one.
    /// Reference: C++ block-producer.cpp `target_time += target_rate_ms`
    fn update_collation_pacing(&mut self) {
        let target_rate = self.description.opts().target_rate;
        self.earliest_collation_time = Some(self.now() + target_rate);
    }

    /// Check if we should generate a block for current slot
    ///
    /// Called from check_all(). Checks:
    /// 1. Are we the leader for current slot?
    /// 2. Have we already generated or is generation pending?
    /// 3. Do we have a valid parent available in FSM?
    fn check_collation(&mut self) {
        instrument!();

        // Don't collate faster than target_rate
        // block-producer.cpp coro_sleep(target_time)
        if let Some(earliest) = self.earliest_collation_time {
            let now = self.now();
            if now < earliest {
                // Schedule wakeup at earliest_collation_time
                self.set_next_awake_time(earliest);
                return;
            }
        }

        // Use FSM's progress cursor (first non-progressed slot) for collation decisions.
        // Collation follows notarized/skipped progress, not finalization.
        // Reference: C++ block-producer.cpp collates on notarized chain.
        let current_slot = self.simplex_state.get_first_non_progressed_slot();

        // Stale window guard (C++ parity: consensus.cpp LeaderWindowObserved handler sets
        // current_window_ BEFORE the leader check). Skip collation when the progress
        // cursor still points at a slot in a window that has already been superseded.
        let slot_window = self.description.get_window_idx(current_slot);
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if slot_window < current_window {
            log::trace!(
                "Session {} check_collation: skipping stale slot {} (window {} < current {})",
                &self.session_id().to_hex_string()[..8],
                current_slot,
                slot_window,
                current_window
            );
            return;
        }

        // C++ parity: cancel stale precollations when the leader window changes.
        // C++ block-producer.cpp replaces cancellation_source_ on each new
        // OurLeaderWindowStarted, which cancels all previous-window collations.
        //
        // Rust must detect stale-window precollations even when local_chain_head
        // is None (common during bootstrap when no block was generated yet).
        let has_stale_precollations = if let Some(ref head) = self.local_chain_head {
            head.window != current_window
        } else {
            self.precollated_blocks
                .keys()
                .any(|&s| self.description.get_window_idx(s) != current_window)
        };
        if has_stale_precollations {
            log::debug!(
                "Session {} check_collation: leader window changed to {}, \
                resetting precollation pipeline",
                &self.session_id().to_hex_string()[..8],
                current_window,
            );
            self.reset_precollations();
        }

        // Don't generate if already generated or pending for this slot.
        // However, if we have a local chain head and no queued precollation for the
        // next slot in this window, keep the pipeline full by retrying it here.
        if self.slot_is_generated(current_slot) || self.slot_is_pending_generate(current_slot) {
            if let Some(ref head) = self.local_chain_head {
                let next_slot = SlotIndex(head.slot.0 + 1);
                if head.window == current_window
                    && !self.slot_is_generated(next_slot)
                    && !self.slot_is_pending_generate(next_slot)
                    && !self.precollated_blocks.contains_key(&next_slot)
                {
                    self.precollate_block(next_slot);
                }
            }
            return;
        }

        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(current_slot);

        // Check if we're the leader for current slot
        if leader != self_idx {
            return;
        }

        // Verify we have a valid parent available in FSM.
        // Parent is derived from per-slot `available_base` (notarized/skip chain),
        // not from finalization.
        if !self.simplex_state.has_available_parent(&self.description, current_slot) {
            log::trace!(
                "Session {} check_collation: waiting for parent for slot {current_slot} (no \
                available parent in FSM)",
                self.session_id().to_hex_string(),
            );
            return;
        }

        let parent = self.simplex_state.get_available_parent(&self.description, current_slot);
        log::trace!(
            "Session {} check_collation: we are leader for slot {}, parent={:?}",
            self.session_id().to_hex_string(),
            current_slot,
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
        );

        // Resolve parent BlockIdExt (required for explicit-parent hints and seqno derivation).
        let resolved_parent_block_id = match parent.as_ref() {
            None => None,
            Some(parent_info) => self.resolve_parent_block_id(parent_info),
        };

        if let Some(parent_info) = parent.as_ref() {
            if resolved_parent_block_id.is_none() {
                log::trace!(
                    "Session {} check_collation: waiting for resolved parent BlockIdExt for slot \
                    {current_slot} (parent={parent_info}), requesting parent",
                    self.session_id().to_hex_string(),
                );

                // Request the missing parent candidate from peers
                self.request_candidate(parent_info.slot, parent_info.hash.clone(), None);

                return;
            }
        }

        // Mark pending_generate for this slot
        self.slot_set_pending_generate(current_slot, true);

        // Check for precollated block first
        self.collates_precollated_counter.total_increment();

        // Clone precollated result before consuming (avoids borrow checker issues)
        let precollated_result =
            self.precollated_blocks.get(&current_slot).and_then(|pb| pb.result.clone());

        if let Some(result) = precollated_result {
            log::trace!(
                "Session {} check_collation: precollated result found for slot {}",
                self.session_id().to_hex_string(),
                current_slot
            );

            self.collates_precollated_counter.success();
            self.record_collation_start();

            // C++ parity: future slots may be precollated as either normal or empty blocks.
            self.generated_block(current_slot, result);
            self.update_collation_pacing();

            // Precollate next block
            self.precollate_block(current_slot + 1);

            return;
        }

        self.collates_precollated_counter.failure();

        // No precollated block, invoke collation
        self.invoke_collation(current_slot, parent);
    }

    /// Invoke block collation for a slot
    ///
    /// Creates an async request and sends to higher layer via SessionListener.
    /// Reference: validator-session/src/session_processor.rs invoke_collation
    fn invoke_collation(
        &mut self,
        slot: SlotIndex,
        parent: Option<crate::block::CandidateParentInfo>,
    ) {
        instrument!();
        let Some(self_idx) = self.try_begin_collation_slot(slot, CollationAttempt::Initial) else {
            return;
        };

        // INVARIANT: Generation slots must be monotonically increasing (gaps allowed)
        if let Some(last_slot) = self.last_generated_slot {
            assert!(
                slot >= last_slot,
                "SessionProcessor INVARIANT VIOLATION: generation requested for slot {} but last \
                generation was slot {} (generation slots must be monotonically increasing)",
                slot,
                last_slot
            );
        }
        //TODO: LK: implement precollation pipeline reset
        self.last_generated_slot = Some(slot);
        self.execute_collation_attempt(
            slot,
            parent,
            self_idx,
            CollationAttempt::Initial,
            true,
            true,
        );
    }

    /// Handle successful collation callback
    ///
    /// Accepts `CollationResult` to handle both normal blocks and empty blocks.
    fn on_collation_complete(&mut self, slot: SlotIndex, request_id: u32, result: CollationResult) {
        instrument!();
        check_execution_time!(50_000);

        // C++ parity: the post-collation publication gate is the current leader
        // window, not the Rust progress cursor alone. The C++ block producer
        // continues to publish CandidateGenerated/CandidateReceived as long as
        // `current_leader_window_ == window`, even if consensus has already
        // timeout-skipped earlier slots inside that same window.
        let fsm_first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        let current_window = self.simplex_state.get_current_leader_window_idx();
        let slot_window = self.description.get_window_idx(slot);

        if slot_window != current_window {
            log::warn!(
                "Session {} on_collation_complete: discarding stale candidate for slot {} \
                (window {} != current {}, request_id={})",
                self.session_id().to_hex_string(),
                slot,
                slot_window,
                current_window,
                request_id
            );
            self.forget_self_collation_tracking(
                slot,
                "stale_window_result_discarded_before_publish",
            );
            self.collates_expire_counter.success();
            self.remove_precollated_block(slot);
            return;
        }

        if slot == fsm_first_non_progressed_slot {
            // Process block for current slot immediately
            self.collates_counter.success();
            self.record_self_collation_generated(slot, "published_current_slot");

            // Track expiry: failure() means NOT expired (which is good)
            self.collates_expire_counter.failure();

            self.generated_block(slot, result);

            // C++ parity: after generating a candidate, start precollation for the next
            // slot in the same leader window (block-producer.cpp `++slot; parent = id;`).
            self.precollate_block(slot + 1);
        } else if slot > fsm_first_non_progressed_slot {
            // Store as precollated for future slot
            let mut publish_now = false;
            if let Some(precollated_block) = self.precollated_blocks.get_mut(&slot) {
                if precollated_block.result.is_some() {
                    log::error!(
                        "Session {} on_collation_complete: precollated result for slot {} \
                        already exists! (request_id={})",
                        self.session_id().to_hex_string(),
                        slot,
                        request_id
                    );
                    self.increment_error();
                    return;
                }
                precollated_block.result = Some(result.clone());
                self.collates_counter.success();
                self.record_self_collation_generated(slot, "stored_future_slot");

                log::trace!(
                    "Session {} on_collation_complete: stored precollated result for slot {} \
                    (request_id={})",
                    self.session_id().to_hex_string(),
                    slot,
                    request_id
                );

                // Precollate next block
                // C++ parity: if this slot is still in the current leader window,
                // publish the candidate immediately instead of waiting until it
                // becomes `first_non_progressed_slot`.
                let current_window = self.simplex_state.get_current_leader_window_idx();
                publish_now = self.description.get_window_idx(slot) == current_window;

                if publish_now {
                    log::trace!(
                        "Session {} on_collation_complete: publishing in-window future candidate \
                        for slot {} immediately (C++ parity)",
                        self.session_id().to_hex_string(),
                        slot
                    );
                }

                if !publish_now {
                    self.precollate_block(slot + 1);
                }
            } else {
                log::warn!(
                    "Session {} on_collation_complete: no precollated entry for slot {} \
                    (request_id={})",
                    self.session_id().to_hex_string(),
                    slot,
                    request_id
                );
                self.forget_self_collation_tracking(slot, "future_slot_missing_precollation_entry");
            }

            if publish_now {
                if let Some(precollated_result) =
                    self.precollated_blocks.get(&slot).and_then(|pb| pb.result.clone())
                {
                    self.generated_block(slot, precollated_result);
                    self.precollate_block(slot + 1);
                }
            }
        } else {
            // C++ parity: if the leader window is still current, publish the
            // late result anyway instead of suppressing it just because Rust's
            // `first_non_progressed_slot` already advanced inside the window.
            self.collates_counter.success();
            self.record_self_collation_generated(slot, "published_late_same_window");
            self.collates_expire_counter.failure();

            log::warn!(
                "Session {} on_collation_complete: slot {} already passed (current={}) \
                but window {} is still current; publishing late same-window candidate",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot,
                current_window
            );

            self.generated_block(slot, result);
            self.precollate_block(slot + 1);
        }
    }

    /// Internal implementation of collation failure handling with retry count tracking
    fn on_collation_failed_impl(
        &mut self,
        slot: SlotIndex,
        request_id: u32,
        err: Error,
        retry_count: u32,
    ) {
        instrument!();

        // `simplex_collates` increments per attempt for legacy reasons.
        // Self-collation flows count attempts as ONE (initial + retries), so we
        // only mark the self-collation flow as failed once we know no further
        // retry will be scheduled (slot passed or max retries reached).
        self.collates_counter.failure();

        // Use FSM's progress cursor to check if slot has already progressed.
        // Collation follows notarized/skipped progress, not finalization.
        let fsm_first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();

        if slot < fsm_first_non_progressed_slot {
            log::warn!(
                "Session {} on_collation_failed: slot {} already passed, ignoring",
                self.session_id().to_hex_string(),
                slot
            );
            self.record_self_collation_final_failure(slot, "slot_progressed_past_during_attempt");
            self.remove_precollated_block(slot);
            return;
        }

        let retry_timeout = self.description.opts().collation_retry_timeout;
        let retry_max = self.description.opts().collation_retry_max_attempts;

        // Check if we've exceeded max retries
        if retry_count >= retry_max {
            log::warn!(
                "Session {} on_collation_failed: max retries ({}) reached for slot {}, \
                not scheduling retry (error: {}, request_id={})",
                self.session_id().to_hex_string(),
                retry_max,
                slot,
                err,
                request_id
            );
            self.record_self_collation_final_failure(
                slot,
                &format!("max_retries_exhausted: {}", err),
            );
            self.remove_precollated_block(slot);
            return;
        }

        log::info!(
            "Session {} COLLATION_FLOW attempt_failed: expected_block_id={} slot={} \
            attempt_failure={}/{} reason={}",
            &self.session_id().to_hex_string()[..8],
            self.self_collation_starts_by_slot
                .get(&slot)
                .map(|(_, exp)| self.format_expected_block_id(*exp))
                .unwrap_or_else(|| "unknown".to_string()),
            slot,
            retry_count + 1,
            retry_max + 1,
            err,
        );

        let next_retry_count = retry_count + 1;
        let expiration_time = self.now() + retry_timeout;

        log::warn!(
            "Session {} on_collation_failed: scheduling retry {}/{} for slot {} in {:?} \
            (error: {}, request_id={})",
            self.session_id().to_hex_string(),
            next_retry_count,
            retry_max,
            slot,
            retry_timeout,
            err,
            request_id
        );

        // Remove failed precollation entry
        self.remove_precollated_block(slot);

        // Schedule retry
        let session_id = self.session_id().clone();
        self.post_delayed_action(expiration_time, move |processor| {
            // Use FSM's progress cursor to check if slot has already progressed.
            // Collation follows notarized/skipped progress, not finalization.
            let fsm_first_non_progressed_slot =
                processor.simplex_state.get_first_non_progressed_slot();

            // Slot already passed
            if slot < fsm_first_non_progressed_slot {
                log::trace!(
                    "Session {} on_collation_failed retry: slot {} already passed \
                    (current={}), skipping",
                    session_id.to_hex_string(),
                    slot,
                    fsm_first_non_progressed_slot
                );
                return;
            }

            // Not the max precollated slot (another slot was started after this one)
            if let Some(max_slot) = processor.precollated_blocks_max_slot {
                if slot != max_slot {
                    log::trace!(
                        "Session {} on_collation_failed retry: slot {} is not max \
                        precollated slot (max={}), skipping",
                        session_id.to_hex_string(),
                        slot,
                        max_slot
                    );
                    return;
                }
            }

            // Already precollated (completed successfully while we were waiting)
            if let Some(precollated) = processor.precollated_blocks.get(&slot) {
                if precollated.result.is_some() {
                    log::trace!(
                        "Session {} on_collation_failed retry: slot {} already \
                        precollated, skipping",
                        session_id.to_hex_string(),
                        slot
                    );
                    return;
                }
            }

            log::trace!(
                "Session {} on_collation_failed retry: retrying slot {} (attempt {}/{})",
                session_id.to_hex_string(),
                slot,
                next_retry_count,
                processor.description.opts().collation_retry_max_attempts
            );

            // Invoke collation with retry count passed via closure
            processor.invoke_collation_retry(slot, next_retry_count);
        });
    }

    /// Invoke collation for retry (tracks retry count via closure callback)
    ///
    /// Similar to invoke_collation but passes retry_count through the callback chain.
    fn invoke_collation_retry(&mut self, slot: SlotIndex, retry_count: u32) {
        instrument!();
        let Some(self_idx) =
            self.try_begin_collation_slot(slot, CollationAttempt::Retry { retry_count })
        else {
            return;
        };

        // Capture parent at collation start (same as invoke_collation)
        let parent = self.simplex_state.get_available_parent(&self.description, slot);
        self.execute_collation_attempt(
            slot,
            parent,
            self_idx,
            CollationAttempt::Retry { retry_count },
            false,
            false,
        );
    }

    /// Persist candidate info for a locally generated candidate.
    ///
    /// This starts the DB write early and registers it in `candidate_info_store_results`,
    /// so `broadcast_vote()` can later block in `WaitCandidateInfoStored` before sending
    /// `NotarizeVote` for this candidate.
    ///
    /// Reference: C++ `validator/consensus/simplex/candidate-resolver.cpp`
    /// (`CandidateReceived` handler calls `store_to_db(...).start().detach()`).
    /// Reference: C++ `validator/consensus/simplex/consensus.cpp`
    /// (`try_notarize()` awaits `WaitCandidateInfoStored(..., true, false)` before broadcasting `NotarizeVote`).
    fn persist_generated_candidate_info_to_db(
        &mut self,
        slot: SlotIndex,
        prepared: &GeneratedBlockDesc,
        parent: &Option<crate::block::CandidateParentInfo>,
        is_empty: bool,
    ) {
        let self_idx = self.description.get_self_idx();
        let parent_info = parent.as_ref().map(|p| (p.slot, &p.hash));
        let candidate_hash_data_bytes_for_db = if is_empty {
            let Some(p) = parent.as_ref() else {
                log::error!(
                    "Session {} persist_generated_candidate_info_to_db: empty block must have \
                    parent",
                    &self.description.get_session_id().to_hex_string()[..8],
                );
                return;
            };
            crate::utils::build_candidate_hash_data_bytes_empty(
                &prepared.block_id_ext,
                (p.slot, &p.hash),
            )
        } else {
            let Some(block_candidate) = prepared.block_candidate.as_ref() else {
                log::error!(
                    "Session {} persist_generated_candidate_info_to_db: normal block must have \
                    block_candidate",
                    &self.description.get_session_id().to_hex_string()[..8],
                );
                return;
            };
            let collated_file_hash = block_candidate.collated_file_hash.clone();
            crate::utils::build_candidate_hash_data_bytes(
                Some(&prepared.block_id_ext),
                Some(&collated_file_hash),
                parent_info,
            )
        };

        self.save_candidate_info_to_db(
            slot,
            &prepared.candidate_hash,
            self_idx,
            &candidate_hash_data_bytes_for_db,
            prepared.signature.clone(),
        );
    }

    /// Process successfully generated block
    ///
    /// Called when collation is complete (either direct or precollated).
    /// Handles both normal blocks and empty blocks for finalization recovery.
    /// Signs the block, broadcasts it, and submits to FSM.
    ///
    /// Reference: C++ block-producer.cpp generate_candidates() loop
    fn generated_block(&mut self, slot: SlotIndex, result: CollationResult) {
        instrument!();
        check_execution_time!(100_000);

        // Get parent from precollated block BEFORE removing it.
        // Parent was locked at collation start to avoid races with consensus events.
        let parent = self.precollated_blocks.get(&slot).and_then(|pb| pb.parent.clone());

        // Remove from precollated blocks
        self.remove_precollated_block(slot);

        // Stale window guard (C++ parity: block-producer.cpp generation loop,
        // consensus.cpp start_generation). Discard candidates whose leader window
        // has already been superseded — the collation callback arrived too late.
        let slot_window = self.description.get_window_idx(slot);
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if slot_window != current_window {
            log::warn!(
                "Session {} generated_block: discarding stale candidate for slot {} \
                (window {} != current {})",
                &self.session_id().to_hex_string()[..8],
                slot,
                slot_window,
                current_window
            );
            self.note_generated_candidate_validation_missed_for_slot(
                slot,
                format!("generated_block_stale_window window={slot_window} current_window={current_window}"),
            );
            self.invalidate_local_chain_head();
            return;
        }

        // C++ parity: only the leader-window freshness gate applies here.
        // If the callback is late but still belongs to the current leader
        // window, C++ still publishes CandidateGenerated/CandidateReceived.
        let fsm_first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        if slot < fsm_first_non_progressed_slot {
            log::trace!(
                "Session {} generated_block: publishing late same-window slot {} \
                (first_non_progressed_slot={})",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot
            );
        } else if slot > fsm_first_non_progressed_slot {
            log::trace!(
                "Session {} generated_block: publishing future in-window slot {} \
                (first_non_progressed_slot={})",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_progressed_slot
            );
        }

        log::trace!(
            "Session {} generated_block: using locked parent for slot {}: {:?}",
            self.session_id().to_hex_string(),
            slot,
            parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
        );

        // Determine if this is an empty block
        let is_empty = matches!(result, CollationResult::Empty { .. });

        // INVARIANT: Empty block must have a parent (first block in epoch cannot be empty)
        if is_empty && parent.is_none() {
            log::error!(
                "Session {} generated_block: empty block for slot {} has no parent \
                (first block in epoch cannot be empty)",
                self.session_id().to_hex_string(),
                slot
            );
            self.increment_error();
            return;
        }

        // Process block based on type: validate, sign, broadcast, submit to FSM
        let prepared = match &result {
            CollationResult::Block(candidate) => {
                self.create_normal_block_desc(slot, candidate, &parent)
            }
            CollationResult::Empty { parent_block_id } => {
                self.create_empty_block_desc(slot, parent_block_id, &parent)
            }
        };

        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                log::error!(
                    "Session {} generated_block: failed to generate block for slot {}: {}",
                    self.session_id().to_hex_string(),
                    slot,
                    e
                );
                self.increment_error();
                return;
            }
        };

        self.persist_generated_candidate_info_to_db(slot, &prepared, &parent, is_empty);

        // --- C++ candidate-chaining parity (block-producer.cpp `parent = id`) ---
        // Synchronously seed the generated-parent cache and local chain head BEFORE
        // the async on_candidate_received self-loop, so that precollate_block() for
        // the next slot in the same window can resolve the parent immediately.
        let candidate_parent_info =
            crate::block::CandidateParentInfo { slot, hash: prepared.candidate_hash.clone() };
        let raw_id = RawCandidateId { slot, hash: prepared.candidate_hash.clone() };
        self.link_self_collation_candidate(slot, &raw_id);
        self.generated_parent_cache.insert(raw_id.clone(), prepared.block_id_ext.clone());
        if let Some(gen_utime_ms) = prepared.gen_utime_ms {
            self.generated_parent_gen_utime_ms_cache.insert(
                RawCandidateId { slot, hash: prepared.candidate_hash.clone() },
                gen_utime_ms,
            );
        }
        self.track_generated_candidate_for_validation(raw_id.clone());

        let slot_window = self.description.get_window_idx(slot);
        self.local_chain_head = Some(LocalChainHead {
            window: slot_window,
            slot,
            parent_info: candidate_parent_info,
            block_id: prepared.block_id_ext.clone(),
            gen_utime_ms: prepared.gen_utime_ms,
        });

        log::trace!(
            "Session {} generated_block: updated local_chain_head: window={}, slot={}, hash={}",
            &self.session_id().to_hex_string()[..8],
            slot_window,
            slot,
            &prepared.candidate_hash.to_hex_string()[..8],
        );

        // Clone TL candidate data before broadcasting (needed for on_candidate_received)
        let tl_candidate_data_for_self = prepared.tl_candidate_data.clone();

        // Broadcast to network
        self.receiver.send_block_broadcast(
            slot.value(),
            prepared.candidate_hash.clone(),
            prepared.tl_candidate_data,
        );

        // DEBUG: Short pattern for quick grep (COLLATION = block generation flow)
        log::debug!(
            "Session {} COLLATION success: slot={}, hash={}, empty={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &prepared.candidate_hash.to_hex_string()[..8],
            is_empty
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} generated_block: broadcast complete for slot={slot}, hash={}, \
            empty={is_empty}, block_id={:?}",
            self.session_id().to_hex_string(),
            prepared.candidate_hash.to_hex_string(),
            prepared.block_id_ext,
        );

        // Simulate receiving our own block via on_candidate_received
        // This ensures the block goes through the same path as network-received blocks
        // and gets added to received_candidates uniformly
        let self_idx = self.description.get_self_idx().value();
        crate::task_queue::post_closure(
            &self.task_queue,
            move |processor: &mut SessionProcessor| {
                processor.on_candidate_received(self_idx, tl_candidate_data_for_self, None);
            },
        );

        log::trace!(
            "Session {} generated_block: posted on_candidate_received for own block slot {}",
            &self.session_id().to_hex_string()[..8],
            slot
        );

        // Update per-slot state
        self.slot_set_pending_generate(slot, false);
        self.slot_set_generated(slot, true);
        self.slot_set_sent_generated(slot, true);
    }

    /// Create normal (non-empty) block descriptor for broadcast and FSM submission
    ///
    /// Validates block size and seqno, computes hashes, signs, and builds TL structure.
    fn create_normal_block_desc(
        &self,
        slot: SlotIndex,
        candidate: &crate::ValidatorBlockCandidate,
        parent: &Option<crate::block::CandidateParentInfo>,
    ) -> Result<GeneratedBlockDesc> {
        let root_hash = &candidate.id.root_hash;
        let data = &candidate.data;
        let collated_data = &candidate.collated_data;
        // Compute hashes from canonical BOC representation to match C++ simplex behavior.
        // C++ leader hashes the original serialized bytes; C++ receiver hashes decompressed
        // bytes — they match because BOC serialization is deterministic given the same mode
        // flags (mode 31 for block data, mode 2 for collated data).
        // We explicitly canonicalize (deserialize → re-serialize with target flags) to
        // guarantee matching hashes even if the input BOC was serialized with different flags.
        //
        // Falls back to raw bytes if canonicalization fails (e.g., in unit tests with
        // mock data that's not valid BOC). In production, all data is valid BOC.
        let file_hash =
            match consensus_common::compression::canonicalize_boc(data.data(), BocFlags::all()) {
                Ok(canonical) => UInt256::from_slice(&sha256_digest(&canonical)),
                Err(_) => UInt256::from_slice(&sha256_digest(data.data())),
            };

        let collated_file_hash = match consensus_common::compression::canonicalize_boc(
            collated_data.data(),
            BocFlags::Crc32,
        ) {
            Ok(canonical) => UInt256::from_slice(&sha256_digest(&canonical)),
            Err(_) => UInt256::from_slice(&sha256_digest(collated_data.data())),
        };
        log::trace!(
            "Session {} create_normal_block_desc: slot={}, root_hash={:x}",
            self.session_id().to_hex_string(),
            slot,
            root_hash
        );

        // Validate sizes
        let max_block_size = self.description.opts().max_block_size;
        let max_collated_size = self.description.opts().max_collated_data_size;

        if data.data().len() > max_block_size || collated_data.data().len() > max_collated_size {
            fail!(
                "block too large ({}+{} > {max_block_size}+{max_collated_size})",
                data.data().len(),
                collated_data.data().len()
            );
        }

        // Derive expected seqno from locked parent (C++ behavior).
        // Seqno = parent_seqno + 1, or initial_block_seqno for genesis.
        //
        // Reference: C++ block-producer.cpp derive_seqno() uses parent block's seqno.
        let expected_seqno = match parent {
            None => {
                // Genesis block: use initial_block_seqno from session initialization.
                // This is the seqno of the first block in the epoch.
                let initial_seqno = self.description.get_initial_block_seqno();
                log::trace!(
                    "Session {} create_normal_block_desc: genesis block, seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    initial_seqno
                );
                initial_seqno
            }
            Some(parent_info) => {
                // Non-genesis: derive seqno from parent's BlockIdExt (parent_seqno + 1).
                // Look up parent's BlockIdExt from received_candidates.
                let parent_block_id =
                    self.resolve_parent_block_id(parent_info).ok_or_else(|| {
                        // Parent BlockIdExt not resolved - should not happen (checked in check_collation)
                        error!(
                            "parent BlockIdExt not resolved \
                            for parent {parent_info} at slot {slot}"
                        )
                    })?;
                parent_block_id.seq_no + 1
            }
        };

        // Validate seqno matches expected
        let candidate_seqno = candidate.id.seq_no;
        if candidate_seqno != expected_seqno {
            fail!(
                "seqno mismatch: candidate has seqno={candidate_seqno}, \
                expected={expected_seqno} (derived from parent={:?})",
                parent.as_ref().map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
            );
        }

        // Construct BlockIdExt for hash computation
        let block_id = BlockIdExt {
            shard_id: self.description.get_shard().clone(),
            seq_no: expected_seqno,
            root_hash: root_hash.clone(),
            file_hash: file_hash.clone(),
        };

        // Compute parent info for hash
        let parent_info: Option<(SlotIndex, &UInt256)> = parent.as_ref().map(|p| (p.slot, &p.hash));

        // Compute candidate hash
        let candidate_hash = crate::utils::compute_candidate_id_hash(
            slot,
            Some(&block_id),
            Some(&collated_file_hash),
            parent_info,
        );

        // Sign candidate
        let signature = crate::utils::sign_candidate(
            &self.session_id(),
            slot,
            &candidate_hash,
            self.local_key(),
        )
        .map_err(|e| error!("failed to sign candidate: {e}"))?;

        // Build TL candidate for broadcast
        // C++ simplex always uses compressed candidates (compression_enabled=true hardcoded).
        // Serialize as validatorSession.compressedCandidate (LZ4+BOC merged roots).
        let (compressed, decompressed_size) =
            consensus_common::compression::compress_candidate_data(
                data.data(),
                collated_data.data(),
            )?;
        let tl_block_candidate = CompressedCandidate {
            src: UInt256::default(),
            round: candidate_seqno as i32,
            root_hash: root_hash.clone(),
            data: compressed,
            decompressed_size: decompressed_size as i32,
        };
        let candidate_bytes =
            consensus_common::serialize_tl_boxed_object!(&tl_block_candidate.into_boxed());

        // Parent info for TL - use CandidateParent wrapper
        let tl_parent = match parent {
            Some(p) => CandidateParent {
                id: CandidateId { slot: p.slot.value() as i32, hash: p.hash.clone() }.into_boxed(),
            }
            .into_boxed(),
            None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
        };

        let tl_candidate_data = CandidateData::Consensus_Block(CandidateDataBlock {
            slot: slot.value() as i32,
            candidate: candidate_bytes,
            parent: tl_parent,
            signature: signature.clone(),
        });

        // Compute actual file hashes for FSM
        let computed_file_hash = consensus_common::utils::get_hash_from_block_payload(data);
        let computed_collated_file_hash =
            consensus_common::utils::get_hash_from_block_payload(collated_data);
        let gen_utime_ms = extract_consensus_gen_utime_ms(collated_data.data());

        let block_id_ext = BlockIdExt {
            shard_id: self.description.get_shard().clone(),
            seq_no: expected_seqno,
            root_hash: root_hash.clone(),
            file_hash: computed_file_hash,
        };

        // Create block candidate for FSM
        let block_candidate = crate::block::BlockCandidate {
            id: block_id_ext.clone(),
            collated_file_hash: computed_collated_file_hash,
            data: data.data().to_vec(),
            collated_data: collated_data.data().to_vec(),
            creator: self
                .description
                .get_source_public_key(self.description.get_self_idx())
                .clone(),
        };

        Ok(GeneratedBlockDesc {
            block_id_ext,
            block_candidate: Some(block_candidate),
            candidate_hash,
            tl_candidate_data,
            signature,
            gen_utime_ms,
        })
    }

    /// Create empty block descriptor for broadcast and FSM submission
    ///
    /// Empty blocks re-sign the previous block's BlockIdExt for finalization recovery.
    /// Reference: C++ block-producer.cpp generate_candidates() empty block branch
    fn create_empty_block_desc(
        &self,
        slot: SlotIndex,
        parent_block_id: &BlockIdExt,
        parent: &Option<crate::block::CandidateParentInfo>,
    ) -> Result<GeneratedBlockDesc> {
        log::debug!(
            "Session {} create_empty_block_desc: slot={}, parent_block_id={:?}",
            &self.session_id().to_hex_string()[..8],
            slot,
            parent_block_id
        );

        // INVARIANT: Empty blocks require parent (checked in generated_block)
        let p = parent
            .as_ref()
            .ok_or_else(|| error!("empty block must have parent for hash computation"))?;

        // For empty blocks, use candidateHashDataEmpty TL type (different from candidateHashDataOrdinary)
        // Reference: C++ CandidateId::create_hash_data() uses consensus_candidateHashDataEmpty
        let candidate_hash =
            crate::utils::compute_candidate_id_hash_empty(parent_block_id, (p.slot, &p.hash));

        // Sign candidate
        let signature = crate::utils::sign_candidate(
            &self.session_id(),
            slot,
            &candidate_hash,
            self.local_key(),
        )
        .map_err(|e| error!("failed to sign candidate: {e}"))?;

        // Build TL candidate for broadcast
        // consensus.empty uses CandidateId directly (not CandidateParent wrapper)
        let parent = CandidateId { slot: p.slot.value() as i32, hash: p.hash.clone() }.into_boxed();

        let tl_candidate_data = CandidateData::Consensus_Empty(CandidateDataEmpty {
            slot: slot.value() as i32,
            parent,
            block: parent_block_id.clone(),
            signature: signature.clone(),
        });

        Ok(GeneratedBlockDesc {
            block_id_ext: parent_block_id.clone(),
            block_candidate: None,
            candidate_hash,
            tl_candidate_data,
            signature,
            gen_utime_ms: None,
        })
    }

    /*
        Precollation Pipeline — C++ Candidate Chaining Parity
        Reference: C++ block-producer.cpp generate_candidates() loop

        The precollation pipeline chains candidates across slots within a single leader
        window. After `generated_block()` completes slot N, it updates `local_chain_head`
        and calls `precollate_block(N+1)`, which uses the local chain head as the explicit
        parent instead of waiting for FSM `available_base` propagation via notarization.

        Pipeline reset (`reset_precollations()`) is triggered by:
        - Session stop
        - Leader window changes (via `invalidate_local_chain_head()`)
        - Progress cursor jumping past queued slots

        ┌─────────────────────────────────────────────────────────────────────────────────┐
        │ Precollation Pipeline                                                           │
        │                                                                                 │
        │  1. check_collation() — first slot in window:                                   │
        │     ├── Use FSM available_base as parent                                        │
        │     └── invoke_collation(slot, parent)                                          │
        │                                                                                 │
        │  2. generated_block() — after broadcast:                                        │
        │     ├── Update local_chain_head + generated_parent_cache                        │
        │     └── precollate_block(slot + 1) — chain next slot                            │
        │                                                                                 │
        │  3. precollate_block(slot):                                                     │
        │     ├── Prefer local_chain_head for parent (same window)                        │
        │     ├── Fall back to FSM available_base                                         │
        │     └── invoke_collation(slot, parent)                                          │
        │                                                                                 │
        │  4. check_collation() finds precollated block:                                  │
        │     ├── Use precollated candidate directly                                      │
        │     └── Remove from pipeline, start next precollation                           │
        │                                                                                 │
        │  5. Window change / progress jump:                                              │
        │     └── invalidate_local_chain_head() + reset_precollations()                   │
        └─────────────────────────────────────────────────────────────────────────────────┘
    */

    /// Precollate block for future slot
    ///
    /// Keeps collation pipeline full to minimize latency.
    /// Reference: validator-session/src/session_processor.rs precollate_block
    fn precollate_block(&mut self, slot: SlotIndex) {
        // Check max precollated blocks limit
        let max_precollated =
            self.description.opts().slots_per_leader_window.saturating_sub(1) as usize;
        if self.precollated_blocks.len() >= max_precollated {
            log::trace!(
                "Session {} precollate_block: max precollated blocks limit {} reached",
                self.session_id().to_hex_string(),
                max_precollated
            );
            return;
        }

        // If slot already in pipeline, try next slot
        let mut target_slot = slot;
        if self.precollated_blocks.contains_key(&target_slot) {
            if let Some(max_slot) = self.precollated_blocks_max_slot {
                if let Some(precollated) = self.precollated_blocks.get(&max_slot) {
                    if precollated.result.is_some() {
                        target_slot = max_slot + 1;
                    }
                }
            }
        }

        // Must still be in the same leader window as the current window
        let target_window = self.description.get_window_idx(target_slot);
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if target_window != current_window {
            log::trace!(
                "Session {} precollate_block: slot {} is in window {} (current={}), skipping",
                self.session_id().to_hex_string(),
                target_slot,
                target_window,
                current_window
            );
            return;
        }

        // Must be leader for the target slot
        let self_idx = self.description.get_self_idx();
        let leader = self.description.get_leader(target_slot);
        if leader != self_idx {
            log::trace!(
                "Session {} precollate_block: not leader for slot {} (leader={})",
                self.session_id().to_hex_string(),
                target_slot,
                leader
            );
            return;
        }

        // C++ parity: prefer local chain head for parent when the previous slot in
        // the same window was just generated locally (block-producer.cpp `parent = id`).
        // Fall back to FSM available_base for the first slot in a window or if the
        // local chain head is stale.
        let parent = if let Some(ref head) = self.local_chain_head {
            if head.window == target_window && head.slot + 1 == target_slot {
                log::trace!(
                    "Session {} precollate_block: using local_chain_head for slot {} \
                    (parent=s{}:{})",
                    &self.session_id().to_hex_string()[..8],
                    target_slot,
                    head.parent_info.slot,
                    &head.parent_info.hash.to_hex_string()[..8],
                );
                Some(head.parent_info.clone())
            } else {
                None
            }
        } else {
            None
        };

        let parent = if parent.is_some() {
            parent
        } else {
            // Fall back to FSM available_base
            if !self.simplex_state.has_available_parent(&self.description, target_slot) {
                log::trace!(
                    "Session {} precollate_block: parent is not available for slot {}",
                    self.session_id().to_hex_string(),
                    target_slot
                );
                return;
            }
            self.simplex_state.get_available_parent(&self.description, target_slot)
        };

        if let Some(ref parent_info) = parent {
            if self.resolve_parent_block_id(parent_info).is_none() {
                log::trace!(
                    "Session {} precollate_block: parent BlockIdExt is not resolved yet for slot \
                    {target_slot} (parent={parent_info})",
                    self.session_id().to_hex_string(),
                );
                return;
            }
        }

        self.invoke_collation(target_slot, parent);
    }

    /// Remove precollated block entry.
    ///
    /// Does NOT touch `self_collation_starts_by_slot` — self-collation tracking
    /// has multiple terminal semantics (success, final failure, ignore) and is
    /// managed explicitly by callers. `generated_block()` in particular calls
    /// this BEFORE `link_self_collation_candidate()` and must NOT lose the
    /// in-flight tracking entry.
    fn remove_precollated_block(&mut self, slot: SlotIndex) {
        if self.precollated_blocks.remove(&slot).is_some() {
            log::trace!(
                "Session {} remove_precollated_block: removed slot {}",
                self.session_id().to_hex_string(),
                slot
            );
            self.precollation_results_counter.increment(1);
        }
    }

    /// Reset all precollations and invalidate the local chain head.
    ///
    /// Called when the precollation pipeline must be flushed:
    /// - Session stop
    /// - Leader window change (progress cursor moved to a new window)
    /// - Progress cursor jumped past queued precollation slots
    fn reset_precollations(&mut self) {
        log::debug!(
            "Session {} reset_precollations: cancelling {} pending precollations, \
            clearing local_chain_head",
            self.session_id().to_hex_string(),
            self.precollated_blocks.len()
        );

        // Cancel all pending requests
        let cancelled_slots: Vec<SlotIndex> = self.precollated_blocks.keys().copied().collect();
        for (_slot, precollated_block) in self.precollated_blocks.iter() {
            precollated_block.request.cancel();
        }
        for slot in cancelled_slots {
            self.forget_self_collation_tracking(slot, "precollation_pipeline_reset");
        }

        self.precollated_blocks.clear();
        self.precollated_blocks_max_slot = None;
        self.invalidate_local_chain_head();
    }

    /// Invalidate the window-local chain head.
    ///
    /// Called when the leader window changes, the progress cursor jumps, or a
    /// consensus event (skip/notarize) invalidates the locally generated parent
    /// chain. After invalidation, the next collation will start fresh from the
    /// FSM `available_base`.
    fn invalidate_local_chain_head(&mut self) {
        if let Some(ref head) = self.local_chain_head {
            log::trace!(
                "Session {} invalidate_local_chain_head: clearing (was window={}, slot={})",
                &self.session_id().to_hex_string()[..8],
                head.window,
                head.slot,
            );
        }
        self.local_chain_head = None;
        self.generated_parent_cache.clear();
        self.generated_parent_gen_utime_ms_cache.clear();
    }

    /*
        Receiver callbacks (called from main loop via ReceiverListener)
    */

    /// Handle incoming vote from the network
    ///
    /// Called by ReceiverListenerImpl when a vote is received.
    /// Verifies signature, converts TL to FSM vote, and passes to SimplexState.
    /// Signature is stored for certificate creation.
    /// Raw vote bytes are passed through for misbehavior proof storage.
    pub fn on_vote(&mut self, source_idx: u32, tl_vote: TlVoteBoxed, raw_vote: RawVoteData) {
        //check_execution_time!(30_000); //TODO: LK: restore during performance testing

        let source_idx = ValidatorIndex::new(source_idx);

        // Validate source index
        if !self.is_valid_source(source_idx) {
            log::warn!(
                "Session {} on_vote: invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.description.get_total_nodes()
            );
            return;
        }

        // Fast-path: drop votes that reference already-finalized slots BEFORE signature verification.
        // This avoids wasted crypto verification for late / duplicated votes.
        //
        // C++ parity: `state.slot_at(slot)` returns nullopt for `slot < first_non_finalized_slot_`.
        let (tl_kind, tl_slot, tl_hash_opt) = match tl_vote.vote() {
            UnsignedVote::Consensus_Simplex_NotarizeVote(u) => {
                if *u.id.slot() < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in NotarizeVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.id.slot()
                    );
                    return;
                }
                let slot = SlotIndex::new(*u.id.slot() as u32);
                let hash = UInt256::from_slice(u.id.hash().as_slice());
                ("notarize", slot, Some(hash))
            }
            UnsignedVote::Consensus_Simplex_FinalizeVote(u) => {
                if *u.id.slot() < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in FinalizeVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.id.slot()
                    );
                    return;
                }
                let slot = SlotIndex::new(*u.id.slot() as u32);
                let hash = UInt256::from_slice(u.id.hash().as_slice());
                ("finalize", slot, Some(hash))
            }
            UnsignedVote::Consensus_Simplex_SkipVote(u) => {
                if u.slot < 0 {
                    log::warn!(
                        "Session {} on_vote: REJECTED - \
                        negative slot {} in SkipVote from source_idx={source_idx}",
                        self.session_id().to_hex_string(),
                        u.slot
                    );
                    return;
                }
                ("skip", SlotIndex::new(u.slot as u32), None)
            }
        };

        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        if tl_slot < fsm_first_non_finalized_slot {
            log::trace!(
                "Session {} on_vote: dropping old vote slot={tl_slot} (< \
                first_non_finalized={fsm_first_non_finalized_slot}) kind={tl_kind} from \
                source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
            );
            return;
        }

        // Mirror C++ vote ingress: reject slots at or beyond `first_too_new_slot`.
        if self.simplex_state.is_vote_slot_too_far_ahead(tl_slot) {
            log::warn!(
                "Session {} on_vote: REJECTED - slot {tl_slot} too far ahead (first_too_new={}) \
                kind={tl_kind} from source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
                self.simplex_state.first_too_new_vote_slot(),
            );
            return;
        }

        let tl_hash_prefix = tl_hash_opt
            .as_ref()
            .map(|h| hex::encode(&h.as_slice()[..4]))
            .unwrap_or_else(|| "-".to_string());
        log::trace!(
            "Session {} on_vote: source_idx={} kind={} slot={} hash={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            tl_slot,
            tl_hash_prefix.as_str(),
        );

        // Get source's public key for signature verification
        let source_public_key = self.description.get_source_public_key(source_idx);

        // Verify signature
        if !verify_vote_signature(&tl_vote, &self.session_id(), source_public_key) {
            log::warn!(
                "Session {} on_vote: invalid signature from source_idx={}",
                self.session_id().to_hex_string(),
                source_idx
            );
            return;
        }

        // Extract FSM vote AND signature from TL (signature stored for certificate creation)
        let (vote, signature) = match extract_vote_and_signature(&tl_vote) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "Session {} on_vote: failed to extract vote from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    e
                );
                return;
            }
        };

        // Extract slot for logging before vote is moved
        let vote_slot = match &vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
        };

        log::trace!(
            "Session {} on_vote: verified vote from source_idx={} kind={} slot={} hash={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            vote_slot,
            tl_hash_prefix.as_str(),
        );

        match tl_kind {
            "notarize" => {
                self.votes_in_total_counter.increment(1);
                self.votes_in_notarize_counter.increment(1);
                self.votes_in_notarize_total = self.votes_in_notarize_total.saturating_add(1);
            }
            "finalize" => {
                self.votes_in_total_counter.increment(1);
                self.votes_in_finalize_counter.increment(1);
                self.votes_in_finalize_total = self.votes_in_finalize_total.saturating_add(1);
            }
            "skip" => {
                self.votes_in_total_counter.increment(1);
                self.votes_in_skip_counter.increment(1);
                self.votes_in_skip_total = self.votes_in_skip_total.saturating_add(1);
            }
            _ => {}
        }

        // Preserve raw bytes for DB persistence (simplex_state.on_vote consumes raw_vote)
        let raw_vote_for_db = raw_vote.clone();

        // Pass to FSM with signature and raw bytes (for certificate creation and misbehavior proofs)
        let result =
            self.simplex_state.on_vote(&self.description, source_idx, vote, signature, raw_vote);

        match result {
            VoteResult::Applied => {
                // Vote applied successfully.
                //
                // Persist vote to DB (fire-and-forget), matching C++:
                // `if (handle_vote(...)) store_vote_to_db(message->message.data.clone(), source).detach();`
                let vote_hash = UInt256::from_slice(&sha256_digest(raw_vote_for_db.as_bytes()));
                let record = VoteRecord {
                    vote_hash,
                    data: raw_vote_for_db.to_raw_buffer(),
                    node_idx: source_idx,
                    seqno: 0, // assigned by save_vote_async
                };
                if let Err(e) = self.db.save_vote_async(&record) {
                    log::error!(
                        "Session {} on_vote: failed to create vote save: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.increment_error();
                }

                // Proactively request missing candidate when receiving a NotarizeVote
                // for a block we don't have. This handles the case where the candidate
                // broadcast was lost (e.g., due to QUIC congestion stall with C++ ngtcp2).
                // Without this, the node can't vote and NotarizationReached is never triggered,
                // which is the normal trigger for candidate requests.
                if let Some(ref hash) = tl_hash_opt {
                    let candidate_id = RawCandidateId { slot: tl_slot, hash: hash.clone() };
                    if !self.has_real_candidate_body(&candidate_id) {
                        log::debug!(
                            "Session {} on_vote: NotarizeVote for missing candidate \
                            slot={tl_slot} hash={} from source_idx={source_idx}, requesting",
                            &self.session_id().to_hex_string()[..8],
                            &hash.to_hex_string()[..8]
                        );
                        self.request_candidate(tl_slot, hash.clone(), None);
                    }
                }
            }
            VoteResult::Duplicate => {
                // Duplicate vote, silently ignore
                log::trace!(
                    "Session {} on_vote: duplicate vote from source_idx={} slot={}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    vote_slot
                );
                return;
            }
            VoteResult::SlotAlreadyFinalized => {
                // Late vote for already-finalized slot - completely normal in distributed systems
                log::trace!(
                    "Session {} on_vote: late vote from source_idx={source_idx} slot={vote_slot} \
                    (slot already finalized)",
                    self.session_id().to_hex_string(),
                );
                return;
            }
            VoteResult::Misbehavior(proof) => {
                log::warn!(
                    "Session {} on_vote: MISBEHAVIOR from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    proof
                );

                // Collect misbehavior report for potential downstream processing
                let slot = proof.slot();
                let report = MisbehaviorReport { validator_idx: source_idx, slot, proof };
                self.misbehavior_reports.push(report);
                self.misbehavior_counter.increment(1);

                // TODO: Callback to ValidatorGroup for slashing/reporting
                return;
            }
            VoteResult::Rejected(reason) => {
                log::warn!(
                    "Session {} on_vote: FSM rejected vote from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    reason
                );
                return;
            }
        }

        // Immediately process the vote (don't wait for next awake)
        self.check_all();
    }

    /// Handle incoming certificate from network
    ///
    /// Called by ReceiverListenerImpl when a certificate is received.
    /// C++ nodes broadcast certificates when thresholds are reached.
    ///
    /// **Validation Policy**:
    /// - Source must be a valid validator in the session
    /// - Each signature is verified against the signer's public key
    /// - Duplicate entries for same validator are tolerated (weight counted once)
    /// - Signatures from unknown validators are ignored with warning
    /// - Total weight must meet 2/3 threshold after excluding invalids
    /// - Invalid signatures cause immediate rejection of entire certificate
    ///
    /// Reference: C++ pool.cpp `handle(IncomingProtocolMessage)` parses `tl::certificate`
    /// and calls `handle_foreign_certificate(cert)` which:
    /// 1. Looks up the slot state
    /// 2. Stores the certificate (notar/skip/final)
    /// 3. Updates per-validator vote accounting from certificate signatures
    /// 4. Calls handle_certificate() to propagate state changes
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `certificate` - Deserialized TL certificate object
    pub fn on_certificate(&mut self, source_idx: u32, tl_certificate: Certificate) {
        let source_idx = ValidatorIndex::new(source_idx);

        // Avoid logging the full TL certificate (includes signature bytes) on the hot path.
        // It is extremely verbose and materially slows down trace-enabled test runs.
        let (tl_slot, tl_kind, tl_hash_opt, tl_sig_count) = match &tl_certificate {
            Certificate::Consensus_Simplex_Certificate(c) => {
                let sig_count = c.signatures.votes().len();
                match &c.vote {
                    UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
                        if *v.id.slot() < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in NotarizeVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.id.slot()
                            );
                            return;
                        }
                        let slot = SlotIndex::new(*v.id.slot() as u32);
                        let hash = UInt256::from_slice(v.id.hash().as_slice());
                        (slot, "notarize", Some(hash), sig_count)
                    }
                    UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
                        if *v.id.slot() < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in FinalizeVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.id.slot()
                            );
                            return;
                        }
                        let slot = SlotIndex::new(*v.id.slot() as u32);
                        let hash = UInt256::from_slice(v.id.hash().as_slice());
                        (slot, "finalize", Some(hash), sig_count)
                    }
                    UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                        if v.slot < 0 {
                            log::warn!(
                                "Session {} on_certificate: REJECTED - \
                                negative slot {} in SkipVote from source_idx={source_idx}",
                                self.session_id().to_hex_string(),
                                v.slot
                            );
                            return;
                        }
                        (SlotIndex::new(v.slot as u32), "skip", None, sig_count)
                    }
                }
            }
        };

        let tl_hash_prefix = tl_hash_opt
            .as_ref()
            .map(|h| hex::encode(&h.as_slice()[..4]))
            .unwrap_or_else(|| "-".to_string());
        log::debug!(
            "Session {} on_certificate: source_idx={} kind={} slot={} hash={} sigs={}",
            self.session_id().to_hex_string(),
            source_idx,
            tl_kind,
            tl_slot,
            tl_hash_prefix.as_str(),
            tl_sig_count
        );

        // Validate source index - must be a known validator in the session
        if !self.is_valid_source(source_idx) {
            log::warn!(
                "Session {} on_certificate: REJECTED - invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.description.get_total_nodes()
            );
            return;
        }

        // C++ parity: drop certificates that reference finalized slots BEFORE signature verification.
        // This avoids wasted crypto verification for late / duplicated certificates.
        //
        // Reference: C++ `state.slot_at(slot)` returns nullopt for `slot < first_non_finalized_slot_`,
        // so `handle_foreign_certificate` ignores them (prevents state resurrection / last_final regressions).
        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        if tl_slot < fsm_first_non_finalized_slot {
            log::trace!(
                "Session {} on_certificate: dropping old certificate slot={tl_slot} \
                (< first_non_finalized={fsm_first_non_finalized_slot}) kind={tl_kind} \
                from source_idx={source_idx}",
                &self.session_id().to_hex_string()[..8],
            );
            return;
        }

        // Parse and verify the certificate (C++ strict policy)
        // Certificate::from_tl performs comprehensive validation:
        // - Rejects invalid validator indices
        // - Rejects duplicate validator indices
        // - Rejects if any signature is invalid
        // - Rejects if total weight < 2/3 threshold
        let cert = match crate::certificate::Certificate::<Vote>::from_tl(
            &tl_certificate,
            &self.description,
            &self.session_id(),
        ) {
            Ok(c) => c,
            Err(e) => {
                self.cert_verify_fail_counter.increment(1);
                self.cert_verify_fails_total += 1;
                log::warn!(
                    "Session {} on_certificate: REJECTED from source_idx={}: {}",
                    self.session_id().to_hex_string(),
                    source_idx,
                    e
                );
                // C++ parity (`pool.cpp`): bad-signature certificates trigger a
                // temporary peer ban so repeated forged traffic cannot starve
                // the receiver/processor pipeline.
                self.receiver.ban_source_for_bad_signature(source_idx.value());
                return;
            }
        };

        self.certs_in_counter.increment(1);

        log::debug!(
            "Session {} on_certificate: verified certificate with {} valid signatures",
            self.session_id().to_hex_string(),
            cert.signatures.len()
        );

        // Proactively request missing candidate when receiving a certificate
        // for a block we don't have. This handles the case where the candidate
        // broadcast was lost (e.g., due to QUIC congestion stall with C++ ngtcp2).
        if let Some(ref hash) = tl_hash_opt {
            let candidate_id = RawCandidateId { slot: tl_slot, hash: hash.clone() };
            if !self.has_real_candidate_body(&candidate_id) {
                log::debug!(
                    "Session {} on_certificate: {tl_kind} cert for missing candidate \
                    slot={tl_slot} hash={} from source_idx={source_idx}, requesting",
                    &self.session_id().to_hex_string()[..8],
                    &hash.to_hex_string()[..8]
                );
                self.request_candidate(tl_slot, hash.clone(), None);
            }
        }

        // Dispatch based on vote type in certificate
        // If stored (new certificate), relay to other validators and cache for standstill
        match &cert.vote {
            Vote::Notarize(notar_vote) => {
                log::debug!(
                    "Session {} on_certificate: NotarCert slot={} block={} sigs={}",
                    self.session_id().to_hex_string(),
                    notar_vote.slot,
                    &notar_vote.block_hash.to_hex_string()[..8],
                    cert.signatures.len()
                );
                let notar_cert = Arc::new(crate::certificate::Certificate {
                    vote: notar_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match self.simplex_state.set_notarize_certificate(
                    &self.description,
                    notar_vote.slot,
                    &notar_vote.block_hash,
                    notar_cert.clone(),
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored NotarCert slot={} block={} ({} \
                            sigs)",
                            self.session_id().to_hex_string(),
                            notar_vote.slot,
                            &notar_vote.block_hash.to_hex_string()[..8],
                            cert.signatures.len(),
                        );
                        self.receiver.notify_certificate_accepted(
                            notar_vote.slot.value(),
                            StandstillCertificateType::Notar,
                        );
                    }
                    Ok(false) => {
                        // Already stored for same block - idempotent
                    }
                    Err(e) => {
                        self.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: NotarCert conflict slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            notar_vote.slot,
                            e
                        );
                    }
                }
            }
            Vote::Finalize(final_vote) => {
                log::debug!(
                    "Session {} on_certificate: FinalCert slot={} block={} sigs={}",
                    self.session_id().to_hex_string(),
                    final_vote.slot,
                    &final_vote.block_hash.to_hex_string()[..8],
                    cert.signatures.len()
                );
                let final_cert = Arc::new(crate::certificate::Certificate {
                    vote: final_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match self.simplex_state.set_finalize_certificate(
                    &self.description,
                    final_vote.slot,
                    &final_vote.block_hash,
                    final_cert,
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored FinalCert slot={} block={} ({} \
                            sigs)",
                            self.session_id().to_hex_string(),
                            final_vote.slot,
                            &final_vote.block_hash.to_hex_string()[..8],
                            cert.signatures.len(),
                        );
                        self.receiver.notify_certificate_accepted(
                            final_vote.slot.value(),
                            StandstillCertificateType::Final,
                        );
                    }
                    Ok(false) => {
                        // Already stored for same block - idempotent
                    }
                    Err(e) => {
                        self.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: FinalCert conflict slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            final_vote.slot,
                            e
                        );
                    }
                }
            }
            Vote::Skip(skip_vote) => {
                log::debug!(
                    "Session {} on_certificate: SkipCert slot={} sigs={}",
                    self.session_id().to_hex_string(),
                    skip_vote.slot,
                    cert.signatures.len()
                );
                let skip_cert = Arc::new(crate::certificate::Certificate {
                    vote: skip_vote.clone(),
                    signatures: cert.signatures.clone(),
                });
                match self.simplex_state.set_skip_certificate(
                    &self.description,
                    skip_vote.slot,
                    skip_cert,
                ) {
                    Ok(true) => {
                        log::debug!(
                            "Session {} on_certificate: stored SkipCert slot={} ({} sigs)",
                            self.session_id().to_hex_string(),
                            skip_vote.slot,
                            cert.signatures.len()
                        );
                        self.receiver.notify_certificate_accepted(
                            skip_vote.slot.value(),
                            StandstillCertificateType::Skip,
                        );
                    }
                    Ok(false) => {
                        // Already stored - idempotent
                    }
                    Err(e) => {
                        self.cert_conflict_counter.increment(1);
                        log::warn!(
                            "Session {} on_certificate: SkipCert error slot={} - {}",
                            &self.session_id().to_hex_string()[..8],
                            skip_vote.slot,
                            e
                        );
                    }
                }
            }
        }

        // Immediately process any state changes
        self.check_all();
    }

    /*
        Validation Flow
        Reference: validator-session/src/session_processor.rs process_broadcast

        ┌─────────────────────────────────────────────────────────────────────────────────┐
        │ Validation Flow                                                                 │
        │                                                                                 │
        │  1. Receiver callback: on_candidate_received(source_idx, tl_candidate)          │
        │     ├── Deserialize TL to RawCandidate                                          │
        │     ├── Verify signature: utils::check_candidate_signature()                    │
        │     ├── Basic validation: slot in range, leader matches source                  │
        │     └── Store in pending_validations with status=Pending                        │
        │                                                                                 │
        │  2. check_validation() (called from check_all):                                 │
        │     ├── For each pending validation:                                            │
        │     │   ├── Extract block info: utils::extract_block_info_from_candidate()      │
        │     │   └── notify_candidate(source_info, root_hash, data, collated_data, cb)   │
        │     └── Set validation status to InProgress                                     │
        │                                                                                 │
        │  3. Validation callback from higher layer:                                      │
        │     └── candidate_decision(root_hash, decision)                                 │
        │         ├── If Valid: resolve RawCandidate → Candidate, push to validated_queue │
        │         └── If Invalid: log warning, remove from pending                        │
        │                                                                                 │
        │  4. Process validated candidates:                                               │
        │     └── For each in validated_candidates: simplex_state.on_candidate(&desc, c)  │
        └─────────────────────────────────────────────────────────────────────────────────┘
    */

    /// Handle incoming block candidate (from broadcast or query response)
    ///
    /// Called by ReceiverListenerImpl when a block candidate is received,
    /// either via broadcast or from a requestCandidate query response.
    /// See Validation Flow diagram above for full flow.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `candidate` - Deserialized candidate data
    /// * `notar_cert` - Serialized notarization certificate signature-set bytes (None for broadcasts)
    ///
    /// Reference: validator-session/src/session_processor.rs process_broadcast()
    /// Reference: C++ block-validator.cpp handle(ValidationRequest)
    pub fn on_candidate_received(
        &mut self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
    ) {
        check_execution_time!(20_000);

        // Extract slot and parent info from CandidateData variant.
        // TL uses i32 for slots; reject negative values at the boundary.
        let (slot, tl_parent_str) = match &candidate {
            CandidateData::Consensus_Block(block) => {
                if block.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Block",
                        self.session_id().to_hex_string(),
                        block.slot
                    );
                    return;
                }
                let parent_str = match block.parent.id() {
                    None => "genesis".to_string(),
                    Some(id) => {
                        if *id.slot() < 0 {
                            log::warn!(
                                "Session {} on_candidate_received: REJECTED - \
                                negative parent slot {} in Block",
                                self.session_id().to_hex_string(),
                                id.slot()
                            );
                            return;
                        }
                        format!("s{}:{}", id.slot(), &hex::encode(id.hash().as_slice())[..8])
                    }
                };
                (block.slot as u32, parent_str)
            }
            CandidateData::Consensus_Empty(empty) => {
                if empty.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.slot
                    );
                    return;
                }
                if *empty.parent.slot() < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - \
                        negative parent slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.parent.slot()
                    );
                    return;
                }
                let id_slot = *empty.parent.slot();
                let id_hash = empty.parent.hash();
                let parent_str = format!("s{}:{}", id_slot, &hex::encode(id_hash.as_slice())[..8]);
                (empty.slot as u32, parent_str)
            }
        };

        let sender_idx = ValidatorIndex::new(source_idx);
        let slot = SlotIndex::new(slot);
        let is_broadcast_candidate = notar_cert.is_none();
        let is_local_self_candidate =
            is_broadcast_candidate && sender_idx == self.description.get_self_idx();
        self.record_candidate_ingress(sender_idx, is_broadcast_candidate);

        // Reject far-future slots (DoS protection) — before any signature verification
        if self.simplex_state.is_slot_too_far_ahead(slot) {
            if is_broadcast_candidate {
                self.candidate_precheck_future_slot_drop_counter.increment(1);
            }
            if is_local_self_candidate {
                self.note_generated_candidate_validation_missed_for_slot(
                    slot,
                    format!(
                        "candidate_precheck_too_far_ahead max_acceptable_slot={}",
                        self.simplex_state.max_acceptable_slot()
                    ),
                );
            }
            log::warn!(
                "Session {} on_candidate_received: REJECTED precheck_drop_reason=too_far_ahead \
                slot={} max={} origin={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                self.simplex_state.max_acceptable_slot(),
                if is_broadcast_candidate { "broadcast" } else { "query" },
            );
            return;
        }

        // Candidate signatures are always created by the slot leader (not by the relay / query responder).
        // For requestCandidate responses, `sender_idx` is the responder, which can differ from the leader.
        let leader_idx = self.description.get_leader(slot);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "Session {} on_candidate_received: \
                sender_idx={sender_idx}, leader_idx={leader_idx}, \
                slot={slot}, tl_parent={tl_parent_str}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 1. Check sender_idx is valid
        if !self.is_valid_source(sender_idx) {
            log::warn!(
                "Session {} on_candidate_received: unknown sender_idx={} (max={})",
                self.session_id().to_hex_string(),
                sender_idx,
                self.description.get_total_nodes()
            );
            return;
        }

        if is_broadcast_candidate && sender_idx != leader_idx {
            self.candidate_precheck_unexpected_sender_drop_counter.increment(1);
            log::warn!(
                "Session {} on_candidate_received: REJECTED \
                precheck_drop_reason=unexpected_sender \
                slot={} leader={} sender={} origin=broadcast",
                &self.session_id().to_hex_string()[..8],
                slot,
                leader_idx,
                sender_idx
            );
            return;
        }

        // Broadcast path must reject stale slots eagerly to avoid stale-body/db churn.
        let fsm_first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();
        if slot < fsm_first_non_finalized_slot {
            if is_broadcast_candidate {
                self.candidate_precheck_old_slot_drop_counter.increment(1);
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!(
                            "candidate_precheck_old_slot first_non_finalized_slot={fsm_first_non_finalized_slot}"
                        ),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: REJECTED precheck_drop_reason=old_slot \
                    slot={} first_non_finalized={} origin=broadcast",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    fsm_first_non_finalized_slot
                );
                return;
            }

            log::trace!(
                "Session {} on_candidate_received: old slot received {} (current={}) origin=query",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_finalized_slot,
            );
        }

        // Get leader public key for signature verification
        let leader_key = self.description.get_source_public_key(leader_idx).clone();

        // 2. Create RawCandidate directly from TL (no serialization needed)
        // Note: max_size check is done in receiver
        let max_size =
            self.description.opts().max_block_size + self.description.opts().max_collated_data_size;

        let raw_candidate = match RawCandidate::from_tl(
            &candidate,
            &self.session_id(),
            &leader_key,
            leader_idx,
            self.description.get_shard(),
            max_size,
            self.description.opts().proto_version,
        ) {
            Ok(c) => c,
            Err(e) => {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!("candidate_deserialization_failed error={e}"),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: failed to deserialize candidate from \
                    sender={}, leader={}, slot={}: {}",
                    self.session_id().to_hex_string(),
                    sender_idx,
                    leader_idx,
                    slot,
                    e
                );
                return;
            }
        };

        // Trace log incoming candidate details for debugging
        log::trace!(
            "Session {} on_candidate_received: parsed candidate {}:{} parent={} leader=v{:03}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &raw_candidate.id.hash.to_hex_string()[..8],
            raw_candidate
                .parent_id
                .as_ref()
                .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
                .unwrap_or_else(|| "genesis".to_string()),
            leader_idx
        );

        // 5. Candidates can be received via relay or requestCandidate (query response),
        // so the sender can differ from the slot leader. The signature is verified against
        // the leader's key above, so a mismatch here is not an error.
        if sender_idx != leader_idx && !is_broadcast_candidate {
            log::trace!(
                "Session {} on_candidate_received: received leader candidate via relay/query: \
                slot={slot} leader={leader_idx} sender={sender_idx}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 6. Check if we already have this candidate
        let candidate_id = raw_candidate.id.clone();
        let id_hash = candidate_id.hash.clone();
        debug_assert!(
            candidate_id.slot == slot,
            "RawCandidateId slot mismatch: tl_slot={} raw_candidate.id.slot={}",
            slot,
            candidate_id.slot
        );

        if is_broadcast_candidate {
            match self.seen_broadcast_candidates.get(&slot).cloned() {
                Some(existing) if existing != candidate_id => {
                    self.candidate_precheck_conflicting_slot_drop_counter.increment(1);
                    if is_local_self_candidate {
                        self.note_generated_candidate_validation_missed(
                            &candidate_id,
                            format!(
                                "candidate_precheck_conflicting_slot first_seen_slot_hash={}:{}",
                                existing.slot,
                                &existing.hash.to_hex_string()[..8]
                            ),
                        );
                    }
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED \
                        precheck_drop_reason=conflicting_slot_candidate \
                        slot={} first_seen={:?} new_candidate={:?} origin=broadcast",
                        &self.session_id().to_hex_string()[..8],
                        slot,
                        existing,
                        candidate_id
                    );
                    return;
                }
                Some(_) => {}
                None => {
                    self.seen_broadcast_candidates.insert(slot, candidate_id.clone());
                }
            }
        }

        // Check if candidate already known.
        // A finalized-boundary stub (seeded by handle_block_finalized with empty data) is NOT
        // "already known" for this purpose -- we want the real body to overwrite it.
        let is_finalized_stub = self
            .received_candidates
            .get(&candidate_id)
            .map(|r| r.candidate_hash_data_bytes.is_empty())
            .unwrap_or(false);
        if !is_finalized_stub
            && (self.pending_validations.contains_key(&candidate_id)
                || self.pending_approve.contains(&candidate_id)
                || self.approved.contains_key(&candidate_id)
                || self.rejected.contains(&candidate_id)
                || self.received_candidates.contains_key(&candidate_id))
        {
            log::trace!(
                "Session {} on_candidate_received: candidate already known: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );

            // CandidateResolver parity: query responses can carry NotarCert bytes even when we
            // already have the candidate body (e.g., we missed the certificate broadcast).
            // Do NOT drop notar_cert in this case, otherwise the node can get permanently stuck
            // waiting for NotarCert while repeatedly receiving bodies.
            if let Some(ref cert_bytes) = notar_cert {
                self.process_received_notar_cert(slot, &id_hash, cert_bytes);
            }
            if notar_cert.is_some() {
                self.check_all();
            }
            return;
        }

        // 7. Store candidate in received_candidates for finalization (even if not validated)
        // This allows us to accept blocks that are finalized before validation completes
        // Reference: validator-session/src/session_processor.rs set_block_candidate
        let receive_time = self.now();
        let block_id = raw_candidate.block.block_id();
        let root_hash = block_id.root_hash.clone();
        let file_hash = block_id.file_hash.clone();

        // Determine if this is an empty block from the TL variant
        let is_empty = matches!(candidate, CandidateData::Consensus_Empty(_));

        // Cache serialized CandidateData for RequestCandidate query fallback (C++ parity).
        // This provides a secondary in-memory store that persists independently of
        // the receiver's resolver_cache, enabling peers to retrieve candidates even
        // after the resolver_cache is cleaned up.
        match serialize_boxed(&candidate) {
            Ok(bytes) => {
                self.candidate_data_cache.insert(candidate_id.clone(), bytes.clone());
                // Persist to DB for restart serving (C++ CandidateResolver::store_candidate parity)
                if let Err(e) = self.db.save_candidate_payload_async(&candidate_id, &bytes) {
                    log::error!(
                        "Session {} on_candidate_received: failed to persist candidate payload: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.increment_error();
                }
            }
            Err(e) => {
                log::warn!(
                    "Session {} on_candidate_received: failed to serialize CandidateData for cache: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
            }
        }

        // Seqno validation for on_candidate_received
        // Validate seqno is consistent with parent (if parent is already received)
        let received_seqno = block_id.seq_no;
        if let Some(ref parent) = raw_candidate.parent_id {
            if let Some(parent_received) = self.received_candidates.get(parent) {
                let parent_seqno = parent_received.block_id.seq_no;
                let expected_seqno = if is_empty { parent_seqno } else { parent_seqno + 1 };

                if received_seqno != expected_seqno {
                    // NOTE: We no longer reject candidates for seqno mismatch at receive time.
                    // The seqno in a candidate is based on the collator's prev_blocks_ids (their chain view),
                    // while the parent slot is from the Simplex FSM. These can legitimately diverge when:
                    // 1. The FSM parent is an older notarized block
                    // 2. The collator's chain has more finalized blocks
                    // Seqno validation is deferred until finalized state is materialized.
                    log::debug!(
                        "Session {} on_candidate_received: seqno differs from parent-based \
                        expectation for slot={slot}, received seqno={received_seqno}, \
                        expected={expected_seqno} (parent_seqno={parent_seqno}, \
                        is_empty={is_empty}). Allowing through - finalized path will resolve it.",
                        &self.session_id().to_hex_string()[..8],
                    );
                }
            }
            // If parent not yet received, we can't validate seqno - allow it through
            // Validation will happen when finalized state is applied.
        } else {
            // No parent (first block in epoch) - seqno is based on the session's initial_block_seqno
            // which may be > 1 if this is not the first session (e.g., after zerostate, seqno=1, but
            // subsequent sessions continue from their start seqno).
            // We don't validate first block seqno at receive time - defer to finalized application.

            // INVARIANT: First block (no parent) cannot be empty
            // Empty blocks inherit parent's BlockIdExt, so they require a parent
            if is_empty {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "first_block_cannot_be_empty",
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: INVARIANT VIOLATION - first block (slot={}) \
                    cannot be empty (empty blocks require parent). Rejecting.",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
                return;
            }

            // Genesis-parent candidates at slot > 0 are normal in Simplex: when early
            // slots are skipped, subsequent leaders produce blocks with parent_id=None.
            if slot.value() != 0 {
                log::trace!(
                    "Session {} on_candidate_received: genesis-parent block at slot={} \
                    (early slots were skipped)",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
            }

            log::debug!(
                "Session {} on_candidate_received: first block (slot={}) has seqno={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                received_seqno
            );
        }

        // Extract actual block data from RawCandidate (not the TL wrapper)
        // This is what validation/finalization callbacks consume.
        let gen_utime_ms = raw_candidate
            .block
            .as_block()
            .and_then(|block| extract_consensus_gen_utime_ms(&block.collated_data));
        let (block_data, collated_data) = match raw_candidate.block.as_block() {
            Some(block) => (
                consensus_common::ConsensusCommonFactory::create_block_payload(block.data.clone()),
                consensus_common::ConsensusCommonFactory::create_block_payload(
                    block.collated_data.clone(),
                ),
            ),
            None => (
                // Empty block - no data
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
            ),
        };
        let observed_data = block_data.clone();
        let observed_collated_data = collated_data.clone();

        let parent_id = raw_candidate.parent_id.clone();

        // Build CandidateHashData TL bytes for signature verification
        // This is the data that was hashed to produce candidate_id_hash
        let candidate_hash_data_bytes = if is_empty {
            // Empty blocks use candidateHashDataEmpty with CandidateId parent
            let Some(parent) = parent_id.as_ref() else {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "empty_candidate_missing_parent",
                    );
                }
                log::error!(
                    "Session {} on_candidate_received: empty block must have parent",
                    &self.session_id().to_hex_string()[..8]
                );
                return;
            };
            crate::utils::build_candidate_hash_data_bytes_empty(
                &block_id,
                (parent.slot, &parent.hash),
            )
        } else {
            // Non-empty blocks use candidateHashDataOrdinary
            let collated_file_hash = match raw_candidate.block.as_block() {
                Some(block) => block.collated_file_hash.clone(),
                None => UInt256::default(),
            };
            let parent_info = parent_id.as_ref().map(|p| (p.slot, &p.hash));
            crate::utils::build_candidate_hash_data_bytes(
                Some(&block_id),
                Some(&collated_file_hash),
                parent_info,
            )
        };

        let parent_metadata_present =
            parent_id.as_ref().is_none_or(|parent| self.received_candidates.contains_key(parent));
        log::trace!(
            "Session {} on_candidate_received: slot={} parent={:?} parent_metadata_present={}",
            self.session_id().to_hex_string(),
            slot,
            parent_id.as_ref().map(|p| p.slot),
            parent_metadata_present,
        );

        // Clone data needed for DB save before moving into ReceivedCandidate
        let candidate_hash_data_bytes_for_db = candidate_hash_data_bytes.clone();
        let signature_for_db = raw_candidate.signature.clone();

        self.received_candidates.insert(
            candidate_id.clone(),
            ReceivedCandidate {
                slot,
                source_idx: leader_idx,
                candidate_id_hash: id_hash.clone(),
                candidate_hash_data_bytes,
                block_id: block_id.clone(),
                root_hash,
                file_hash,
                data: block_data,
                collated_data,
                gen_utime_ms,
                receive_time,
                is_empty,
                parent_id: parent_id.clone(),
            },
        );

        // Save candidate info to DB (fire-and-forget, matching C++ `.start().detach()` pattern)
        self.save_candidate_info_to_db(
            slot,
            &id_hash,
            leader_idx,
            &candidate_hash_data_bytes_for_db,
            signature_for_db,
        );

        // Remove from requested_candidates if we were waiting for this
        self.requested_candidates.remove(&candidate_id);

        if !is_empty {
            match crate::utils::extract_before_split_flag(observed_data.data()) {
                Ok(before_split) => {
                    self.before_split_by_block_id.insert(block_id.clone(), before_split);
                }
                Err(e) => {
                    log::trace!(
                        "Session {} on_candidate_received: failed to extract before_split flag \
                        for block_id={}: {}",
                        self.session_id().to_hex_string(),
                        block_id,
                        e
                    );
                }
            }

            let observed_flags = CandidateObservedFlags {
                body_present: true,
                parent_ready: self.simplex_state.get_notarize_certificate(slot, &id_hash).is_some(),
                local_collated: is_local_self_candidate,
            };
            self.notify_candidate_observed(
                block_id.clone(),
                observed_data,
                observed_collated_data,
                observed_flags,
            );
        }

        // Candidate arrival can unblock deferred recursive finalization chains.
        self.retry_pending_recursive_finalization();

        // DEBUG: Short pattern for quick grep (RECV = candidate received)
        log::debug!(
            "Session {} RECV candidate: slot={slot}, hash={}, seqno={received_seqno}, \
            from=v{:03}, empty={is_empty}, parent_metadata_present={parent_metadata_present}",
            &self.session_id().to_hex_string()[..8],
            &id_hash.to_hex_string()[..8],
            leader_idx,
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} on_candidate_received: slot={slot}, hash={}, seqno={received_seqno}, \
            source={leader_idx}, empty={is_empty}, parent={:?}, parent_metadata_present={parent_metadata_present}",
            self.session_id().to_hex_string(),
            id_hash.to_hex_string(),
            parent_id.as_ref().map(|p| format!("{}:{}", p.slot, p.hash.to_hex_string())),
        );

        // 8. Process notarization/finalization signature-sets if provided (from query response)
        // This can be done immediately, regardless of parent-metadata availability.
        // Clone id_hash before use for certificates
        let id_hash_for_cert = id_hash.clone();
        if let Some(ref cert_bytes) = notar_cert {
            self.process_received_notar_cert(slot, &id_hash_for_cert, cert_bytes);
        }

        // 9. Admit the candidate immediately; check_validation() owns the remaining
        // WaitForParent gate and, for empties, waits until the expected normal tip can be
        // reconstructed from locally known parent metadata.
        if !parent_metadata_present {
            log::debug!(
                "Session {} on_candidate_received: slot={} hash={} is missing parent metadata, \
                but ingress no longer parks candidates behind a simplex-local resolution queue",
                &self.session_id().to_hex_string()[..8],
                slot,
                &id_hash.to_hex_string()[..8],
            );
        }
        self.register_candidate_for_validation(raw_candidate, slot, leader_idx, receive_time);

        // Immediately process the new candidate (don't wait for next awake)
        self.check_all();
    }

    /// Handle notar-only progress from requestCandidate repair path.
    ///
    /// This callback is used when receiver-side merge logic obtains notarization
    /// signatures before candidate body completeness. It allows the processor/FSM to
    /// ingest the certificate immediately and unblock parent-gated validations.
    pub fn on_candidate_notar_received(
        &mut self,
        source_idx: u32,
        slot: SlotIndex,
        block_hash: UInt256,
        notar_cert: Vec<u8>,
    ) {
        if !self.is_valid_source(ValidatorIndex::new(source_idx)) {
            log::warn!(
                "Session {} on_candidate_notar_received: invalid source_idx={} (max={})",
                self.session_id().to_hex_string(),
                source_idx,
                self.description.get_total_nodes(),
            );
            return;
        }

        self.process_received_notar_cert(slot, &block_hash, &notar_cert);
        self.check_all();
    }

    /// Process notarization certificate received from query response
    ///
    /// Deserializes, verifies, and stores the certificate in SimplexState.
    ///
    /// Parse VoteSignatureSet bytes (not full Certificate) to match C++ wire format.
    /// C++ `candidateAndCert.notar` contains serialized `voteSignatureSet`, not `certificate`.
    /// Reference: C++ candidate-resolver.cpp from_tl():
    ///   TRY_RESULT(signatures, fetch_tl_object<tl::voteSignatureSet>(entry.notar_, true));
    ///   TRY_RESULT_ASSIGN(result.notar_cert, NotarCert::from_tl(std::move(*signatures), vote, bus));
    fn process_received_notar_cert(
        &mut self,
        slot: SlotIndex,
        block_hash: &UInt256,
        notar_cert_bytes: &[u8],
    ) {
        log::trace!(
            "Session {} process_received_notar_cert: slot={} hash={} bytes={}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &block_hash.to_hex_string()[..8],
            notar_cert_bytes.len()
        );

        // Deserialize VoteSignatureSet (C++ wire format for candidateAndCert.notar)
        let tl_sigs = match deserialize_boxed(notar_cert_bytes) {
            Ok(msg) => match msg.downcast::<VoteSignatureSetBoxed>() {
                Ok(sigs) => sigs,
                Err(_) => {
                    log::warn!(
                        "Session {} process_received_notar_cert: unexpected type, expected \
                        VoteSignatureSet for slot={slot} hash={}",
                        &self.session_id().to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }
            },
            Err(e) => {
                log::warn!(
                    "Session {} process_received_notar_cert: failed to deserialize \
                    VoteSignatureSet for slot={slot} hash={}: {e}",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                );
                return;
            }
        };

        // Verify certificate using from_tl_signatures (matches C++ NotarCert::from_tl)
        match self.verify_notar_cert_from_vote_signature_set(slot, block_hash, &tl_sigs) {
            Ok(notar_cert_ptr) => {
                log::trace!(
                    "Session {} process_received_notar_cert: verified notar cert for slot={slot} \
                    hash={} with {} sigs",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                    notar_cert_ptr.signatures.len(),
                );

                // Ensure cert is persisted before updating FSM state.
                let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
                if !self.notar_cert_store_results.contains_key(&candidate_id) {
                    match self.db.save_notar_cert_async(&candidate_id, notar_cert_ptr.as_ref()) {
                        Ok(result) => {
                            self.notar_cert_store_results.insert(candidate_id.clone(), result);
                        }
                        Err(e) => {
                            log::error!(
                                "Session {} process_received_notar_cert: failed to create \
                                notar_cert save slot={slot}: {e}",
                                &self.session_id().to_hex_string()[..8],
                            );
                            self.increment_error();
                            return;
                        }
                    }
                }

                if !self.wait_candidate_info_stored(&candidate_id, false, true) {
                    log::warn!(
                        "Session {} process_received_notar_cert: skipping FSM feed because notar \
                        cert is not durable yet for slot={} hash={}",
                        &self.session_id().to_hex_string()[..8],
                        slot,
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }

                // Store in SimplexState after durability confirmation.
                let store_result = self.simplex_state.set_notarize_certificate(
                    &self.description,
                    slot,
                    block_hash,
                    notar_cert_ptr.clone(),
                );
                match store_result {
                    Ok(true) => {
                        // Cert accepted into FSM; DB persistence is already guaranteed above.
                    }
                    Ok(false) => {
                        // Already stored for the same block - idempotent
                    }
                    Err(e) => {
                        log::warn!(
                            "Session {} process_received_notar_cert: notar cert conflict \
                            slot={slot} hash={}: {e}",
                            &self.session_id().to_hex_string()[..8],
                            &block_hash.to_hex_string()[..8],
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "Session {} process_received_notar_cert: invalid notar cert for slot={slot} \
                    hash={}: {e}",
                    &self.session_id().to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                );
            }
        }

        // Newly ingested NotarCert can unblock deferred recursive-finalization chains
        // where ancestors require notar-signature callback mode.
        self.retry_pending_recursive_finalization();
    }

    /// Verify notarization certificate from VoteSignatureSet (C++ wire format)
    ///
    /// Parse VoteSignatureSet and verify signatures.
    /// Reference: C++ NotarCert::from_tl(voteSignatureSet&&, vote, bus)
    fn verify_notar_cert_from_vote_signature_set(
        &self,
        slot: SlotIndex,
        block_hash: &UInt256,
        tl_sigs: &VoteSignatureSetBoxed,
    ) -> Result<crate::certificate::NotarCertPtr> {
        // Build the vote being certified
        let vote = crate::simplex_state::NotarizeVote { slot, block_hash: block_hash.clone() };

        // Verify signatures and build certificate
        let cert = crate::certificate::NotarCert::from_tl_signatures(
            tl_sigs,
            vote,
            &self.description,
            &self.session_id(),
        )?;

        Ok(Arc::new(cert))
    }

    /// Handle activity update from the receiver
    ///
    /// Called periodically by ReceiverListenerImpl with active weight and per-validator activity times.
    pub fn on_activity(
        &mut self,
        active_weight: ValidatorWeight,
        last_activity: Vec<Option<SystemTime>>,
        snapshot: crate::receiver::ReceiverActivitySnapshot,
    ) {
        if self.active_weight != active_weight {
            log::debug!(
                "Session {} on_activity: active_weight {} -> {}",
                self.session_id().to_hex_string(),
                self.active_weight,
                active_weight
            );
            self.active_weight = active_weight;

            // Update metrics gauge
            self.active_weight_gauge.set(active_weight as f64);
        }
        self.last_activity = last_activity;
        self.last_receiver_snapshot = Some(snapshot);
    }

    pub fn on_standstill_trigger(&mut self, notification: StandstillTriggerNotification) {
        log::warn!("{}", self.build_standstill_trigger_log(&notification));
    }

    fn build_standstill_trigger_log(&self, notification: &StandstillTriggerNotification) -> String {
        let mut result = format!(
            "Session {}: Standstill detected, re-broadcasting \
            {} certs + {} votes (range [{}, {})). Current pool state:\n",
            &self.session_id().to_hex_string()[..8],
            notification.cert_count,
            notification.vote_count,
            notification.begin,
            notification.end,
        );
        result.push_str(&self.simplex_state.standstill_diagnostic_dump(&self.description));
        result
    }

    fn sync_standstill_slots_from_state(&self) {
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        // Keep receiver ingress progress within tracked interval bounds.
        // This mirrors C++ behavior where `now_` never stays behind finalized frontier.
        let progress = self.simplex_state.get_first_non_progressed_slot().value().max(begin);
        self.receiver.set_ingress_slot_begin(begin);
        self.receiver.set_ingress_progress_slot(progress);
        self.receiver.set_standstill_slots(begin, end);
    }

    /*
        ========================================================================
        Empty Parent Tip Metadata

        Reference: C++ consensus.cpp get_resolved_candidate, get_resolved_candidate_inner
        Reference: C++ candidate-resolver.cpp resolve_candidate_inner
        Reference: C++ pool.cpp maybe_resolve_request

        Non-empty candidates enter `pending_validations` immediately after ingress
        and rely on strict `WaitForParent` plus validator-side state resolution.
        Simplex only keeps the minimal metadata walk needed to resolve the
        expected normal tip for empty candidates when parent metadata is not yet
        present locally.
        ========================================================================
    */

    /// Find the first missing metadata record needed to resolve an empty
    /// candidate's expected normal tip.
    ///
    /// We only walk through empty ancestors; a non-empty parent already defines
    /// the normal tip we need for the C++ `event->state->as_normal()` check.
    fn find_first_missing_parent_metadata(
        &self,
        candidate: &RawCandidate,
    ) -> Option<crate::block::RawCandidateId> {
        let mut current_parent = candidate.parent_id.clone();
        let mut depth = 0u32;
        let mut depth_warned = false;

        while let Some(parent_id) = current_parent {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    "Session {} find_first_missing_parent_metadata: deep empty-parent chain \
                    depth={} (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) for candidate slot={}; \
                    continuing until hard limit={MAX_CHAIN_DEPTH}",
                    self.session_id().to_hex_string(),
                    depth,
                    candidate.id.slot,
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    "Session {} find_first_missing_parent_metadata: exceeded \
                    hard MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} for candidate slot={}",
                    self.session_id().to_hex_string(),
                    candidate.id.slot,
                );
                self.increment_error();
                return None;
            }

            let Some(parent_received) = self.received_candidates.get(&parent_id) else {
                log::trace!(
                    "Session {} find_first_missing_parent_metadata: missing parent slot={} hash={} \
                    for candidate slot={}",
                    self.session_id().to_hex_string(),
                    parent_id.slot,
                    &parent_id.hash.to_hex_string()[..8],
                    candidate.id.slot,
                );
                return Some(parent_id);
            };

            if !parent_received.is_empty {
                return None;
            }

            current_parent = parent_received.parent_id.clone();
        }

        None
    }

    /// Ensure an empty candidate can resolve the expected normal tip before approval.
    ///
    /// Unlike the old simplex-local waiting queue, this is an on-demand check in the
    /// validation path: if metadata is missing, we request the next missing parent and
    /// keep the candidate in `pending_validations`.
    fn ensure_empty_parent_tip_ready(
        &mut self,
        raw_candidate: &RawCandidate,
        slot: SlotIndex,
    ) -> bool {
        if self.resolve_parent_normal_tip(raw_candidate).is_some() {
            return true;
        }

        if let Some(missing_parent) = self.find_first_missing_parent_metadata(raw_candidate) {
            log::trace!(
                "Session {} ensure_empty_parent_tip_ready: requesting missing parent metadata \
                slot={} hash={} for empty candidate slot={}",
                self.session_id().to_hex_string(),
                missing_parent.slot,
                &missing_parent.hash.to_hex_string()[..8],
                slot,
            );
            self.request_candidate(missing_parent.slot, missing_parent.hash, Some(Duration::ZERO));
        } else {
            log::trace!(
                "Session {} ensure_empty_parent_tip_ready: empty candidate slot={} is still \
                waiting for the accepted normal head or restart-seeded parent metadata",
                self.session_id().to_hex_string(),
                slot,
            );
        }

        false
    }

    /// Register a candidate for validation once it has been accepted at ingress.
    fn register_candidate_for_validation(
        &mut self,
        raw_candidate: RawCandidate,
        slot: SlotIndex,
        source_idx: ValidatorIndex,
        receive_time: SystemTime,
    ) {
        let candidate_id = raw_candidate.id.clone();

        // Check if already processed
        if self.pending_validations.contains_key(&candidate_id)
            || self.pending_approve.contains(&candidate_id)
            || self.approved.contains_key(&candidate_id)
            || self.rejected.contains(&candidate_id)
        {
            log::trace!(
                "Session {} register_candidate_for_validation: candidate already known: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            return;
        }

        log::trace!(
            "Session {} register_candidate_for_validation: registering candidate slot={} hash={}",
            self.session_id().to_hex_string(),
            slot,
            &candidate_id.hash.to_hex_string()[..8],
        );

        // Add to pending_validations
        self.pending_validations.insert(
            candidate_id,
            PendingValidation { raw_candidate, slot, receive_time, source_idx },
        );

        // Track first candidate received in this slot (for latency metrics).
        let first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        if !self.slot_first_candidate_received(slot) && slot == first_non_progressed_slot {
            self.slot_set_first_candidate_received(slot, true);

            // Track latency from slot start
            if let Ok(elapsed) = self.now().duration_since(self.slot_started_at(slot)) {
                self.first_candidate_received_latency_histogram.record(elapsed.as_millis() as f64);
            }
        }
    }

    /*
        Validation processing
        Reference: validator-session/src/session_processor.rs try_approve_block, candidate_decision_*
    */

    /// C++ `WaitForParent`-equivalent parent gating decision.
    ///
    /// Mirrors `pool.cpp::maybe_resolve_request` semantics:
    /// - `Ready`: candidate can proceed to higher-layer validation now
    /// - `Wait`: keep candidate pending until more cert state arrives
    /// - `Reject`: parent chain conflicts with already established consensus state
    fn evaluate_wait_for_parent(&self, pending: &PendingValidation) -> WaitForParentDecision {
        let slot = pending.slot;
        let candidate_id = &pending.raw_candidate.id;
        let first_non_finalized = self.simplex_state.get_first_non_finalized_slot();
        let parent_id = pending.raw_candidate.parent_id.as_ref();

        // C++ parity (pool.cpp maybe_resolve_request):
        // next_slot_after_parent = parent.has_value() ? parent->slot + 1 : 0
        let next_slot_after_parent = match parent_id {
            Some(pid) => {
                if pid.slot >= slot {
                    return WaitForParentDecision::Reject(format!(
                        "invalid parent slot {} for candidate slot {}",
                        pid.slot, slot
                    ));
                }
                pid.slot + 1
            }
            None => SlotIndex::new(0),
        };

        if slot < first_non_finalized {
            return WaitForParentDecision::Reject(format!(
                "candidate slot {} is already finalized (first_non_finalized={})",
                slot, first_non_finalized
            ));
        }
        if next_slot_after_parent < first_non_finalized {
            return WaitForParentDecision::Reject(format!(
                "candidate parent frontier {} is below first_non_finalized={}",
                next_slot_after_parent, first_non_finalized
            ));
        }

        // C++ parity (pool.cpp maybe_resolve_request):
        // reject candidates conflicting with already notarized/finalized candidate in this slot.
        if let Some(notarized_hash) =
            self.simplex_state.get_notarized_block_hash(&self.description, slot)
        {
            if notarized_hash != candidate_id.hash {
                return WaitForParentDecision::Reject(format!(
                    "slot {} already notarized/finalized with different hash {} (candidate={})",
                    slot,
                    notarized_hash.to_hex_string(),
                    candidate_id.hash.to_hex_string()
                ));
            }
        }

        // C++ parity (pool.cpp maybe_resolve_request):
        // - if parent is at finalized boundary, it must match last finalized block;
        // - otherwise parent slot must be notarized with the same candidate hash.
        if next_slot_after_parent == first_non_finalized {
            match parent_id {
                None => {
                    // C++ parity: only genesis frontier (`first_non_finalized == 0`) can have
                    // `parent_id=None` at boundary check.
                    if first_non_finalized.value() != 0 {
                        return WaitForParentDecision::Reject(format!(
                            "expected finalized-boundary parent at slot {}, got genesis parent",
                            first_non_finalized.value().saturating_sub(1)
                        ));
                    }
                }
                Some(pid) => {
                    let Some((last_finalized_slot, final_cert)) =
                        self.simplex_state.get_last_finalize_certificate()
                    else {
                        return WaitForParentDecision::Reject(format!(
                            "finalized-boundary parent mismatch: no finalized certificate for parent {}:{}",
                            pid.slot,
                            pid.hash.to_hex_string()
                        ));
                    };
                    if last_finalized_slot != pid.slot || final_cert.vote.block_hash != pid.hash {
                        return WaitForParentDecision::Reject(format!(
                            "finalized-boundary parent mismatch: expected {}:{} got {}:{}",
                            last_finalized_slot,
                            final_cert.vote.block_hash.to_hex_string(),
                            pid.slot,
                            pid.hash.to_hex_string()
                        ));
                    }
                }
            }
        } else {
            // next_slot_after_parent > first_non_finalized, so parent must exist and be notarized.
            // Genesis parent has next_slot_after_parent=0 which can't exceed first_non_finalized.
            let Some(pid) = parent_id else {
                return WaitForParentDecision::Reject(format!(
                    "missing parent id for non-boundary candidate at slot {}",
                    slot
                ));
            };
            match self.simplex_state.get_notarized_block_hash(&self.description, pid.slot) {
                Some(notarized_hash) => {
                    if notarized_hash != pid.hash {
                        return WaitForParentDecision::Reject(format!(
                            "parent notarized hash mismatch at slot {}: expected {} got {}",
                            pid.slot,
                            pid.hash.to_hex_string(),
                            notarized_hash.to_hex_string()
                        ));
                    }
                }
                None => return WaitForParentDecision::Wait,
            }
        }

        if next_slot_after_parent == slot {
            return WaitForParentDecision::Ready;
        }

        // All intermediate slots must have Skip certificates.
        let mut gap_slot = next_slot_after_parent;
        while gap_slot < slot {
            if !self.simplex_state.has_skip_certificate_for_slot(&self.description, gap_slot) {
                return WaitForParentDecision::Wait;
            }
            gap_slot += 1;
        }

        WaitForParentDecision::Ready
    }

    /// Check pending validations and send to higher layer for validation
    ///
    /// Called from check_all(). Iterates all pending_validations and forwards
    /// each eligible candidate to the SessionListener via on_candidate().
    ///
    /// Validates pending candidates whose parent chain is `WaitForParent`-ready in the FSM:
    /// parent readiness + gap skip coverage (C++ parity).
    fn check_validation(&mut self) {
        check_execution_time!(10_000);
        instrument!();
        let now = self.now();

        // Collect candidates to validate.
        // A candidate is eligible when:
        // 1. It has been admitted into pending_validations.
        // 2. Parent chain is C++ WaitForParent-ready (notar/final parent + gap skip coverage).
        // 3. Empty candidates can resolve the expected `event->state->as_normal()` tip from
        //    locally known metadata (requesting the next missing parent on demand if needed).
        // 4. MC stale-parent protection is handled in validator-side candidate-native validation.
        // 5. It is not already being validated, approved, or rejected.
        let mut to_validate: Vec<(RawCandidateId, SlotIndex, ValidatorIndex, SystemTime)> =
            Vec::new();

        let candidate_ids: Vec<RawCandidateId> = self.pending_validations.keys().cloned().collect();
        for candidate_id in candidate_ids {
            let (slot, source_idx, receive_time, raw_candidate, wait_for_parent_decision) =
                match self.pending_validations.get(&candidate_id) {
                    Some(p) => (
                        p.slot,
                        p.source_idx,
                        p.receive_time,
                        p.raw_candidate.clone(),
                        self.evaluate_wait_for_parent(p),
                    ),
                    None => continue,
                };

            // Skip if already being validated or decided
            if self.pending_approve.contains(&candidate_id) {
                continue;
            }
            if self.rejected.contains(&candidate_id) {
                continue;
            }
            if self.approved.contains_key(&candidate_id) {
                continue;
            }

            match wait_for_parent_decision {
                WaitForParentDecision::Ready => {}
                WaitForParentDecision::Wait => continue,
                WaitForParentDecision::Reject(reason) => {
                    log::warn!(
                        "Session {} check_validation: rejecting candidate {:?} by WaitForParent \
                         parity gate: {}",
                        self.session_id().to_hex_string(),
                        candidate_id,
                        reason
                    );
                    self.validation_attempt_map.insert(
                        candidate_id.clone(),
                        self.description.opts().validation_retry_attempts,
                    );
                    self.candidate_decision_fail(slot, candidate_id, error!("{reason}"));
                    continue;
                }
            }

            // Check validation attempt count
            if let Some(attempt_idx) = self.validation_attempt_map.get(&candidate_id).copied() {
                if attempt_idx >= self.description.opts().validation_retry_attempts {
                    log::trace!(
                        "Session {} check_validation: max attempts reached for {:?}",
                        self.session_id().to_hex_string(),
                        candidate_id,
                    );
                    continue;
                }
            }

            if !raw_candidate.block.is_empty() {
                if let Some(parent_id) = raw_candidate.parent_id.as_ref() {
                    let parent_info = crate::block::CandidateParentInfo {
                        slot: parent_id.slot,
                        hash: parent_id.hash.clone(),
                    };
                    if let Some(parent_gen_utime_ms) =
                        self.resolve_parent_gen_utime_ms(&parent_info)
                    {
                        let earliest_validation_time = UNIX_EPOCH
                            .checked_add(Duration::from_millis(parent_gen_utime_ms))
                            .and_then(|parent_gen_time| {
                                parent_gen_time
                                    .checked_add(self.description.opts().min_block_interval)
                            });
                        let Some(earliest_validation_time) = earliest_validation_time else {
                            log::warn!(
                                "Session {} check_validation: invalid parent_gen_utime_ms {} for \
                                parent slot {}",
                                self.session_id().to_hex_string(),
                                parent_gen_utime_ms,
                                parent_info.slot,
                            );
                            continue;
                        };
                        if now < earliest_validation_time {
                            self.set_next_awake_time(earliest_validation_time);
                            continue;
                        }
                    }
                }
            }

            if raw_candidate.block.is_empty() {
                if !self.ensure_empty_parent_tip_ready(&raw_candidate, slot) {
                    continue;
                }

                to_validate.push((candidate_id.clone(), slot, source_idx, receive_time));
                continue;
            }

            to_validate.push((candidate_id.clone(), slot, source_idx, receive_time));
        }
        // Process each candidate
        for (candidate_id, slot, source_idx, receive_time) in to_validate {
            self.try_approve_block(&candidate_id, slot, source_idx, receive_time);
        }
    }

    /// Resolve the parent-chain normal tip (`event->state->as_normal()` in C++).
    ///
    /// Walks the parent chain through `received_candidates` until a non-empty
    /// ancestor is found. For candidates without an explicit parent, falls back to the
    /// exact accepted head when it is already known.
    ///
    /// Returns `None` if the parent chain is broken, missing, or contains
    /// only empty ancestors and no exact accepted head is available.
    fn resolve_parent_normal_tip(&self, raw_candidate: &RawCandidate) -> Option<BlockIdExt> {
        let parent_id = match raw_candidate.parent_id.as_ref() {
            Some(parent_id) => parent_id,
            None => return self.accepted_normal_head_block_id.clone(),
        };
        let parent = self.received_candidates.get(parent_id)?;
        if !parent.is_empty {
            return Some(parent.block_id.clone());
        }
        let mut current_parent = parent.parent_id.clone();
        let mut depth = 0u32;
        let mut depth_warned = false;
        while let Some(pid) = current_parent {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    "Session {} resolve_parent_normal_tip: deep empty-parent chain \
                    depth={} (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}); \
                    continuing until hard limit={MAX_CHAIN_DEPTH}",
                    self.session_id().to_hex_string(),
                    depth,
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    "Session {} resolve_parent_normal_tip: exceeded hard \
                    MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} while resolving empty-parent chain",
                    self.session_id().to_hex_string(),
                );
                self.increment_error();
                return None;
            }
            let ancestor = self.received_candidates.get(&pid)?;
            if !ancestor.is_empty {
                return Some(ancestor.block_id.clone());
            }
            current_parent = ancestor.parent_id.clone();
        }
        None
    }

    /// Try to approve a block candidate by sending to higher layer
    ///
    /// Reference: validator-session/src/session_processor.rs try_approve_block()
    fn try_approve_block(
        &mut self,
        candidate_id: &RawCandidateId,
        slot: SlotIndex,
        source_idx: ValidatorIndex,
        receive_time: SystemTime,
    ) {
        check_execution_time!(10_000);
        instrument!();

        // Check if pending validation exists (session-level)
        if !self.pending_validations.contains_key(candidate_id) {
            return;
        }

        // Mark as pending approval
        self.pending_approve.insert(candidate_id.clone());
        self.validation_attempt_map
            .entry(candidate_id.clone())
            .and_modify(|c| *c += 1)
            .or_insert(0);
        self.mark_generated_candidate_validation_started(candidate_id);

        // Get pending validation (now safe to borrow after mutable operations)
        let Some(pending) = self.pending_validations.get(candidate_id) else {
            log::error!(
                "Session {} try_approve_block: candidate not in pending_validations: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            return;
        };

        // Handle empty blocks: C++ block-validator.cpp rejects unless the referenced
        // block equals event->state->as_normal(). We resolve the expected block from
        // the parent chain and compare before approving; if metadata is still missing,
        // the candidate stays pending and waits for the next repair round.
        if pending.raw_candidate.block.is_empty() {
            let referenced_block = pending.raw_candidate.block.block_id().clone();
            let expected = self.resolve_parent_normal_tip(&pending.raw_candidate);
            let cid = candidate_id.clone();

            match expected {
                Some(expected_block) if referenced_block == expected_block => {
                    log::trace!(
                        "Session {} try_approve_block: empty block matches parent normal tip, \
                        approving {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                    self.candidate_decision_ok_internal(cid, slot, receive_time);
                }
                Some(expected_block) => {
                    log::warn!(
                        "Session {} try_approve_block: empty block REJECTED — wrong referenced \
                        block (got seqno={}, expected seqno={}) for {:?}",
                        self.session_id().to_hex_string(),
                        referenced_block.seq_no,
                        expected_block.seq_no,
                        cid,
                    );
                    self.candidate_decision_fail(
                        slot,
                        cid,
                        error!("Wrong referenced block in empty candidate"),
                    );
                }
                None => {
                    if let Some(missing_parent) =
                        self.find_first_missing_parent_metadata(&pending.raw_candidate)
                    {
                        self.request_candidate(
                            missing_parent.slot,
                            missing_parent.hash,
                            Some(Duration::ZERO),
                        );
                    }
                    self.pending_approve.remove(candidate_id);
                    log::trace!(
                        "Session {} try_approve_block: empty block still waiting for parent \
                        normal tip for {:?}",
                        self.session_id().to_hex_string(),
                        cid,
                    );
                }
            }
            return;
        }

        // Get block data for validation
        let Some(block) = pending.raw_candidate.block.as_block() else {
            log::error!(
                "Session {} try_approve_block: non-empty block has no block data: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            return;
        };

        let root_hash = block.id.root_hash.clone();
        let data =
            consensus_common::ConsensusCommonFactory::create_block_payload(block.data.clone());
        let collated_data = consensus_common::ConsensusCommonFactory::create_block_payload(
            block.collated_data.clone(),
        );

        // Create source info for callback
        // Note: source_idx was already validated in on_candidate_received
        // SIMPLEX_ROUNDLESS: bypass ValidatorGroup round invariants
        let source_public_key = self.description.get_source_public_key(source_idx).clone();

        let source_info = crate::BlockSourceInfo {
            source: source_public_key,
            priority: BlockCandidatePriority {
                round: SIMPLEX_ROUNDLESS,             // Simplex roundless mode
                first_block_round: SIMPLEX_ROUNDLESS, // Must match round for consistency
                priority: 0,                          // Leader priority
            },
        };

        // DEBUG: Short pattern for quick grep (VALIDATION = block validation flow)
        log::debug!(
            "Session {} VALIDATION request: slot={}, hash={}, from=v{:03}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &candidate_id.hash.to_hex_string()[..8],
            source_idx
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} try_approve_block: requesting validation for slot={}, hash={}, source={}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id.hash.to_hex_string(),
            source_idx
        );

        // Create callback for validation result
        let task_queue = self.task_queue.clone();
        let candidate_id_copy = candidate_id.clone();

        let callback: crate::ValidatorBlockCandidateDecisionCallback =
            Box::new(move |decision: Result<SystemTime>| {
                let candidate_id = candidate_id_copy.clone();
                let slot_copy = slot;
                let receive_time_copy = receive_time;

                let task: TaskPtr =
                    Box::new(move |processor: &mut SessionProcessor| match decision {
                        Ok(validity_start_time) => {
                            processor.candidate_decision_ok(
                                slot_copy,
                                candidate_id,
                                validity_start_time,
                                receive_time_copy,
                            );
                        }
                        Err(err) => {
                            processor.candidate_decision_fail(slot_copy, candidate_id, err);
                        }
                    });

                task_queue.post_closure(task);
            });

        // Invoke listener via the existing notify_candidate method
        self.notify_candidate(source_info, root_hash, data, collated_data, callback);
    }

    /// Handle successful validation callback
    ///
    /// Reference: validator-session/src/session_processor.rs candidate_decision_ok()
    fn candidate_decision_ok(
        &mut self,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        validity_start_time: SystemTime,
        receive_time: SystemTime,
    ) {
        check_execution_time!(10_000);
        instrument!();

        self.validates_counter.success();

        // Record validation latency (time spent in validator callback)
        if let Ok(latency) = self.now().duration_since(receive_time) {
            self.validation_latency_histogram.record(latency.as_millis() as f64);
        }

        // Record broadcast-to-validation-complete latency (full round-trip from network receive)
        if let Ok(broadcast_latency) = self.now().duration_since(receive_time) {
            self.broadcast_validation_latency_histogram
                .record(broadcast_latency.as_millis() as f64);
        }

        // DEBUG: Short pattern for quick grep (VALIDATION = block validation flow)
        let latency_ms =
            self.now().duration_since(receive_time).map(|d| d.as_millis()).unwrap_or(0);
        log::debug!(
            "Session {} VALIDATION success: slot={}, hash={}, latency={}ms",
            &self.session_id().to_hex_string()[..8],
            slot,
            &candidate_id.hash.to_hex_string()[..8],
            latency_ms
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} candidate_decision_ok: slot={}, hash={}, latency={}ms, validity_start={:?}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id.hash.to_hex_string(),
            latency_ms,
            validity_start_time
        );

        // Ignore late validation callbacks for already processed candidates (validator-session
        // has round gating; in roundless Simplex we gate by "still pending").
        if !self.pending_validations.contains_key(&candidate_id) {
            self.validation_late_callback_counter.increment(1);
            self.note_generated_candidate_validation_missed(
                &candidate_id,
                "validation_late_callback_without_pending_entry",
            );
            self.pending_approve.remove(&candidate_id);
            self.validation_attempt_map.remove(&candidate_id);
            return;
        }

        // If the block is already finalized by the time validation completes, drop the result.
        if let (Some(finalized_seqno), Some(cand_seqno)) = (
            self.finalized_head_seqno,
            self.pending_validations
                .get(&candidate_id)
                .and_then(|p| p.raw_candidate.block.as_block().map(|b| b.id.seq_no)),
        ) {
            if cand_seqno <= finalized_seqno {
                self.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!(
                        "validation_succeeded_after_finalization finalized_seqno={finalized_seqno} cand_seqno={cand_seqno}"
                    ),
                );
                log::warn!(
                    "Session {} candidate_decision_ok: slot={slot}, hash={:?}, \
                    finalized_seqno={finalized_seqno}, cand_seqno={cand_seqno} (drop because \
                    block is already finalized)",
                    self.session_id().to_hex_string(),
                    candidate_id,
                );
                self.pending_approve.remove(&candidate_id);
                self.pending_validations.remove(&candidate_id);
                self.validation_attempt_map.remove(&candidate_id);
                return;
            }
        }

        self.candidate_decision_ok_internal(candidate_id, slot, receive_time);

        // Wake immediately so check_all() runs in the very next main-loop iteration
        self.wake_now();
    }

    /// Internal helper for successful validation (used by both normal and empty block paths)
    fn candidate_decision_ok_internal(
        &mut self,
        candidate_id: RawCandidateId,
        slot: SlotIndex,
        _receive_time: SystemTime,
    ) {
        // Remove from pending_approve
        self.pending_approve.remove(&candidate_id);

        // Get and remove from pending_validations (per-slot state)
        let pending = match self.pending_validations.remove(&candidate_id) {
            Some(p) => p,
            None => {
                self.note_generated_candidate_validation_missed(
                    &candidate_id,
                    "validation_success_missing_pending_entry",
                );
                log::warn!(
                    "Session {} candidate_decision_ok_internal: no pending validation for {:?}",
                    self.session_id().to_hex_string(),
                    candidate_id,
                );
                return;
            }
        };

        // Store candidate for finalization callback
        if let Some(block) = pending.raw_candidate.block.as_block() {
            let stored = ValidatedCandidate {
                source_idx: pending.source_idx,
                root_hash: block.id.root_hash.clone(),
                file_hash: block.id.file_hash.clone(),
                data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    block.data.clone(),
                ),
            };
            self.slot_set_validated_candidate_data(slot, stored);
            log::trace!(
                "Session {} stored validated candidate for slot={}, root_hash={:?}",
                self.session_id().to_hex_string(),
                slot,
                &candidate_id.hash,
            );
        }

        // Resolve RawCandidate to Candidate
        // For empty blocks: inherit parent's BlockIdExt from parent (requires lookup)
        // For normal blocks: resolve() uses the block's BlockIdExt and self.parent_id
        // Note: For non-empty blocks, resolve(None) now correctly uses RawCandidate.parent_id
        let candidate = match pending.raw_candidate.resolve(None) {
            Ok(c) => c,
            Err(e) => {
                self.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!("validation_success_resolve_failed error={e}"),
                );
                log::warn!(
                    "Session {} candidate_decision_ok: failed to resolve candidate: {}",
                    self.session_id().to_hex_string(),
                    e
                );
                return;
            }
        };

        self.mark_generated_candidate_validation_succeeded(&candidate_id);

        // Mark as approved
        self.approved.insert(
            candidate_id,
            (self.now(), consensus_common::ConsensusCommonFactory::create_empty_block_payload()),
        );

        // Push to validated queue for FSM processing
        self.validated_candidates.push_back(candidate);
    }

    /// Handle failed validation callback
    ///
    /// Reference: validator-session/src/session_processor.rs candidate_decision_fail()
    fn candidate_decision_fail(
        &mut self,
        slot: SlotIndex,
        candidate_id: RawCandidateId,
        err: Error,
    ) {
        check_execution_time!(10_000);
        instrument!();

        self.validates_counter.failure();
        self.validation_reject_counter.increment(1);

        let mut reason = format!("{}", err);

        // Ignore late validation callbacks for already processed candidates (validator-session
        // has round gating; in roundless Simplex we gate by "still pending").
        if !self.pending_validations.contains_key(&candidate_id) {
            self.validation_late_callback_counter.increment(1);
            self.note_generated_candidate_validation_missed(
                &candidate_id,
                "validation_fail_late_callback_without_pending_entry",
            );
            self.pending_approve.remove(&candidate_id);
            self.validation_attempt_map.remove(&candidate_id);
            return;
        }

        // If the block is already finalized by the time validation fails, drop it without retries.
        if let (Some(finalized_seqno), Some(cand_seqno)) = (
            self.finalized_head_seqno,
            self.pending_validations
                .get(&candidate_id)
                .and_then(|p| p.raw_candidate.block.as_block().map(|b| b.id.seq_no)),
        ) {
            log::warn!(
                "Session {} candidate_decision_fail: slot={slot}, hash={:?}, \
                finalized_seqno={finalized_seqno}, cand_seqno={cand_seqno} (drop)",
                self.session_id().to_hex_string(),
                candidate_id,
            );
            if cand_seqno <= finalized_seqno {
                self.note_generated_candidate_validation_missed(
                    &candidate_id,
                    format!(
                        "validation_failed_after_finalization finalized_seqno={finalized_seqno} cand_seqno={cand_seqno}"
                    ),
                );
                self.pending_approve.remove(&candidate_id);
                self.pending_validations.remove(&candidate_id);
                self.validation_attempt_map.remove(&candidate_id);
                return;
            }
        }

        // Check if we should retry
        if let Some(attempt_idx) = self.validation_attempt_map.get(&candidate_id).copied() {
            if attempt_idx < self.description.opts().validation_retry_attempts {
                let retry_timeout = self.description.opts().validation_retry_timeout;
                let expiration_time = self.now() + retry_timeout;

                log::warn!(
                    "Session {} candidate_decision_fail: slot={}, hash={:?}, attempt={}/{}, \
                    reason={}. Will retry in {}ms.",
                    self.session_id().to_hex_string(),
                    slot,
                    candidate_id,
                    attempt_idx,
                    self.description.opts().validation_retry_attempts,
                    reason,
                    retry_timeout.as_millis(),
                );

                let candidate_id_copy = candidate_id.clone();
                self.post_delayed_action(
                    expiration_time,
                    move |processor: &mut SessionProcessor| {
                        log::trace!(
                            "Session {} allowing validation retry for {:?}",
                            processor.session_id().to_hex_string(),
                            candidate_id_copy,
                        );
                        // Remove from pending_approve to allow re-validation
                        processor.pending_approve.remove(&candidate_id_copy);
                        processor.wake_now();
                    },
                );

                return;
            }
        }

        log::warn!(
            "Session {} candidate_decision_fail: slot={}, hash={:?}, no attempts left, reason={}",
            self.session_id().to_hex_string(),
            slot,
            candidate_id,
            reason,
        );
        self.note_generated_candidate_validation_missed(
            &candidate_id,
            format!("validation_failed_final reason={reason}"),
        );

        // Remove from pending
        self.pending_approve.remove(&candidate_id);
        self.pending_validations.remove(&candidate_id);

        // Truncate reason if too long
        const MAX_REJECT_REASON_SIZE: usize = 1024;
        if reason.len() > MAX_REJECT_REASON_SIZE {
            reason = reason[..MAX_REJECT_REASON_SIZE].to_string();
        }

        // Mark as rejected
        self.pending_reject.insert(
            candidate_id.clone(),
            consensus_common::ConsensusCommonFactory::create_block_payload(
                reason.as_bytes().to_vec(),
            ),
        );
        self.rejected.insert(candidate_id);
    }

    /// Process validated candidates and feed to FSM
    ///
    /// Called from check_all() after check_validation().
    fn process_validated_candidates(&mut self) {
        check_execution_time!(10_000);

        // Process validated candidates (slot tracking available for future use)
        let _current_slot = self.simplex_state.get_first_non_progressed_slot();

        while let Some(candidate) = self.validated_candidates.pop_front() {
            let candidate_id =
                RawCandidateId { slot: candidate.id.slot, hash: candidate.id.hash.clone() };

            if !self.wait_candidate_info_stored(&candidate_id, true, false) {
                log::warn!(
                    "Session {} process_validated_candidates: skipping candidate due to \
                    missing candidateInfo durability slot={} hash={}",
                    self.session_id().to_hex_string(),
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                );
                continue;
            }

            log::trace!(
                "Session {} process_validated_candidates: feeding candidate to FSM, slot={}",
                self.session_id().to_hex_string(),
                candidate.id.slot,
            );

            if let Err(e) = self.simplex_state.on_candidate(&self.description, candidate) {
                log::warn!(
                    "Session {} process_validated_candidates: FSM rejected candidate: {}",
                    self.session_id().to_hex_string(),
                    e
                );
            }
        }
    }

    /*
        SimplexState event processing
    */

    /// Process all pending events from SimplexState FSM
    ///
    /// Pulls events from the FSM queue and dispatches to appropriate handlers.
    /// Called from check_all() after FSM processing.
    ///
    /// # Late Candidate Handling
    ///
    /// If a BlockFinalized event arrives but we haven't received the block body
    /// via broadcast yet, we push the event back to the front of the queue and
    /// stop processing. The event will be retried on the next check_all() cycle,
    /// giving time for the block broadcast to arrive.
    ///
    /// P2P block candidate download for lost broadcasts is handled by
    /// `request_candidate` which uses `receiver.request_candidate()`.
    fn process_simplex_events(&mut self) {
        let mut event_count = 0u64;

        while let Some(event) = self.simplex_state.pull_event() {
            log::trace!("SimplexState::event: {:?}", event);

            match event {
                SimplexEvent::BroadcastVote(vote) => {
                    // Send vote to receiver which will:
                    // 1. Sign it with session-scoped signature
                    // 2. Broadcast to all validators
                    // 3. Process loopback (our own vote submitted via listener for FSM accounting)
                    self.broadcast_vote(vote);
                }
                SimplexEvent::BlockFinalized(e) => {
                    self.handle_block_finalized(e);
                }
                SimplexEvent::SlotSkipped(event) => {
                    self.handle_slot_skipped_event(event);
                }
                SimplexEvent::NotarizationReached(event) => {
                    self.handle_notarization_reached(event);
                }
                SimplexEvent::SkipCertificateReached(event) => {
                    self.handle_skip_certificate_reached(event);
                }
                SimplexEvent::FinalizationReached(event) => {
                    self.handle_finalization_reached(event);
                }
            }

            event_count += 1;
        }

        if event_count > 0 {
            self.process_events_counter.increment(event_count);
        }
    }

    /// Broadcast a vote to all validators.
    ///
    /// Signs the vote with session-scoped signature and sends via receiver.
    ///
    /// Broadcast vote to all validators
    ///
    /// Signs the vote with session-scoped signature and delegates to receiver for:
    /// - Broadcast to all validators
    /// - Loopback processing (our own vote submitted via listener for FSM accounting)
    fn broadcast_vote(&mut self, vote: Vote) {
        log::trace!("Session {} broadcast_vote: {:?}", self.session_id().to_hex_string(), vote);

        match &vote {
            Vote::Notarize(_) => {
                self.votes_out_total_counter.increment(1);
                self.votes_out_notarize_counter.increment(1);
            }
            Vote::Finalize(_) => {
                self.votes_out_total_counter.increment(1);
                self.votes_out_finalize_counter.increment(1);
            }
            Vote::Skip(_) => {
                self.votes_out_total_counter.increment(1);
                self.votes_out_skip_counter.increment(1);
            }
        }

        // WaitCandidateInfoStored parity (C++ consensus.cpp):
        // - before NotarizeVote: wait candidateInfo stored
        // - before FinalizeVote: wait notarCert stored
        let wait_persist_ok = match &vote {
            Vote::Notarize(v) => {
                let id = RawCandidateId { slot: v.slot, hash: v.block_hash.clone() };
                self.wait_candidate_info_stored(&id, true, false)
            }
            Vote::Finalize(v) => {
                let id = RawCandidateId { slot: v.slot, hash: v.block_hash.clone() };
                self.wait_candidate_info_stored(&id, false, true)
            }
            _ => true,
        };

        if !wait_persist_ok {
            log::error!(
                "Session {} broadcast_vote: aborting vote send due to durability wait failure: {:?}",
                self.session_id().to_hex_string(),
                vote
            );
            self.votes_out_persist_fail_counter.increment(1);
            self.increment_error();
            return;
        }

        // Track first notarize vote in this slot (stage 2 latency)
        if let Vote::Notarize(v) = &vote {
            let vote_slot = v.slot;
            if !self.slot_first_candidate_notarized(vote_slot) {
                self.slot_set_first_candidate_notarized(vote_slot, true);
                if let Ok(latency) = self.now().duration_since(self.slot_started_at(vote_slot)) {
                    self.first_candidate_notarized_latency_histogram
                        .record(latency.as_millis() as f64);
                    log::trace!(
                        "Session {}: first notarize vote in {:.3}ms",
                        &self.session_id().to_hex_string()[..8],
                        latency.as_secs_f64() * 1000.0
                    );
                }
            }
        }

        // Sign the vote with session-scoped signature
        let signed_vote = match sign_vote(&vote, &self.session_id(), self.local_key()) {
            Ok(v) => v.only(), // Extract inner Vote from Vote_
            Err(e) => {
                log::error!(
                    "Session {} broadcast_vote: failed to sign vote: {}",
                    self.session_id().to_hex_string(),
                    e
                );
                self.increment_error();
                return;
            }
        };

        self.persist_our_vote_before_broadcast(&signed_vote);

        log::trace!(
            "Session {} broadcast_vote: sending signed vote",
            self.session_id().to_hex_string()
        );

        // Send via receiver to all validators
        // Note: send_vote serializes the TL object and broadcasts it
        self.receiver.send_vote(signed_vote);
    }

    /// Persist a locally produced signed vote to DB before broadcasting it.
    ///
    /// This matches C++ ordering where the node ensures its own vote is durably stored
    /// before publishing it to the network (restart/standstill reconstruction depends on it).
    ///
    /// Reference: C++ `validator/consensus/simplex/pool.cpp`:
    /// `handle_our_vote()` awaits `store_vote_to_db(...)` before publishing the outgoing message.
    fn persist_our_vote_before_broadcast(&mut self, tl_vote: &TlVote) {
        let serialized =
            consensus_common::serialize_tl_boxed_object!(&tl_vote.clone().into_boxed());
        let vote_hash = UInt256::from_slice(&sha256_digest(&serialized));
        let record = VoteRecord {
            vote_hash,
            data: serialized.into(),
            node_idx: self.description.get_self_idx(),
            seqno: 0, // assigned by save_vote_async
        };

        let result = match self.db.save_vote_async(&record) {
            Ok(r) => r,
            Err(e) => {
                log::error!(
                    "Session {} broadcast_vote: failed to create vote save: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
                return;
            }
        };
        let wait_started_at = self.now();
        log::trace!(
            "Session {} broadcast_vote: waiting vote db.set before send (hash={}, node_idx={})",
            &self.session_id().to_hex_string()[..8],
            &record.vote_hash.to_hex_string()[..8],
            record.node_idx
        );
        if let Err(e) = result.wait() {
            log::error!(
                "Session {} broadcast_vote: failed to store vote before send after {}ms: {}",
                &self.session_id().to_hex_string()[..8],
                self.now().duration_since(wait_started_at).map(|d| d.as_millis()).unwrap_or(0),
                e
            );
            self.increment_error();
        } else {
            log::trace!(
                "Session {} broadcast_vote: stored vote before send in {}ms (hash={})",
                &self.session_id().to_hex_string()[..8],
                self.now().duration_since(wait_started_at).map(|d| d.as_millis()).unwrap_or(0),
                &record.vote_hash.to_hex_string()[..8],
            );
        }
    }

    /*
        Finalization Flow
        Reference: Simplex finalized-driven delivery path

        ┌─────────────────────────────────────────────────────────────────────────────────┐
        │ Finalization Flow                                                               │
        │                                                                                 │
        │  SimplexEvent::BlockFinalized(slot, block)                                      │
        │      │                                                                          │
        │      ▼                                                                          │
        │  handle_block_finalized():                                                      │
        │      ├── Record/update pending trigger in finalized_pending_body                │
        │      ├── retry_pending_recursive_finalization_for(trigger)                      │
        │      ├── Walk parent chain and evaluate each block independently                 │
        │      └── Emit/apply immediately when ready; request/defer only missing blocks   │
        │                                                                                 │
        │  SimplexEvent::SlotSkipped(slot)                                                │
        │      │                                                                          │
        │      ▼                                                                          │
        │  handle_slot_skipped_event():                                                   │
        │      └── (skip tracked internally, no callback to listener)              │
        └─────────────────────────────────────────────────────────────────────────────────┘
    */

    /// Check whether we have a **real** candidate body (not a finalized-boundary stub).
    ///
    /// Finalized-boundary stubs are inserted by `handle_block_finalized` with empty
    /// `candidate_hash_data_bytes` to serve as parent-resolution boundaries. They must
    /// NOT suppress `requestCandidate` retries -- a stub is not a real body.
    fn has_real_candidate_body(&self, id: &RawCandidateId) -> bool {
        self.received_candidates
            .get(id)
            .map(|r| !r.candidate_hash_data_bytes.is_empty())
            .unwrap_or(false)
    }

    /// Schedule a candidate request with delay if not already requested
    ///
    /// Called when we need to repair missing candidate data after learning about a
    /// finalized or otherwise required block before all body/notar data is present.
    /// Adds the (slot, hash) to `requested_candidates` and schedules a delayed action.
    /// After the delay, if the candidate is still not in `received_candidates`, requests
    /// it from peers (with want_notar=true to get NotarCert).
    ///
    /// The delay allows time for the broadcast to arrive naturally before triggering
    /// a P2P query, reducing unnecessary network traffic.
    ///
    /// Request a candidate with optional initial delay.
    ///
    /// # Parameters
    /// - `initial_delay`: Optional delay before sending the request.
    ///   - `None`: Use default `CANDIDATE_REQUEST_DELAY` (allows broadcast to arrive first)
    ///   - `Some(Duration::ZERO)`: Request immediately (for repair-critical paths)
    ///   - `Some(dur)`: Custom delay
    fn request_candidate(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        initial_delay: Option<Duration>,
    ) {
        let delay = initial_delay.unwrap_or(CANDIDATE_REQUEST_DELAY);

        let key = RawCandidateId { slot, hash: block_hash.clone() };

        if self.simplex_state.has_skip_certificate_for_slot(&self.description, slot) {
            log::trace!(
                "Session {} request_candidate: slot={} hash={} - skipped already, not requesting",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
            );
            self.requested_candidates.remove(&key);
            return;
        }

        // Throttle repeated requests for the same (slot,hash) to survive transient partitions.
        let now = self.now();
        if let Some(next_allowed_at) = self.requested_candidates.get(&key) {
            if *next_allowed_at > now {
                log::trace!(
                    "Session {} request_candidate: slot={} hash={} - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    &block_hash.to_hex_string()[..8],
                    next_allowed_at
                );
                return;
            }
        }

        // Check if we already have what we need (stubs don't count as real bodies)
        let have_body = self.has_real_candidate_body(&key);
        let have_notar = self.simplex_state.get_notarize_certificate(slot, &block_hash).is_some();

        if have_body && have_notar {
            return;
        }

        if delay.is_zero() {
            self.requested_candidates.insert(key.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::debug!(
                "Session {} request_candidate: requesting slot={slot} hash={} immediately \
                (body={}, notar={})",
                &self.session_id().to_hex_string()[..8],
                &block_hash.to_hex_string()[..8],
                !have_body,
                !have_notar,
            );

            self.receiver.request_candidate(slot.value(), block_hash);
        } else {
            self.requested_candidates
                .insert(key.clone(), now + delay + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::trace!(
                "Session {} request_candidate: scheduling request for slot={} hash={} in {:?}",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
                delay,
            );

            let session_id = self.session_id().clone();
            let expiration_time = now + delay;

            self.post_delayed_action(expiration_time, move |processor: &mut SessionProcessor| {
                let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
                if !processor.requested_candidates.contains_key(&candidate_id) {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - cancelled before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }
                if processor
                    .simplex_state
                    .has_skip_certificate_for_slot(&processor.description, slot)
                {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - skipped before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    processor.requested_candidates.remove(&candidate_id);
                    return;
                }
                let have_body = processor.has_real_candidate_body(&candidate_id);
                let have_notar =
                    processor.simplex_state.get_notarize_certificate(slot, &block_hash).is_some();

                if have_body && have_notar {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} - already have \
                        what we need",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }

                log::debug!(
                    "Session {} delayed_request_candidate: requesting slot={slot} hash={} from \
                    peers (body={}, notar={})",
                    &session_id.to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                    !have_body,
                    !have_notar,
                );

                processor.receiver.request_candidate(slot.value(), block_hash);
                processor
                    .requested_candidates
                    .insert(candidate_id, processor.now() + CANDIDATE_REQUEST_RETRY_INTERVAL);
            });
        }
    }

    /// Handle a reverse-bridge request from the validator layer to ensure a
    /// candidate body (and optionally its parent chain) is available.
    ///
    /// The validator calls this when collation/validation needs a parent
    /// state that hasn't been applied by the engine yet. Simplex resolves
    /// `BlockIdExt` to the internal `RawCandidateId` and triggers repair.
    ///
    /// Important: a slot may be skipped but still have a notarized candidate.
    /// The repair must handle that case — the candidate body might not have
    /// been received via the normal broadcast path.
    ///
    /// C++ equivalent: demand-driven path in `BlockProducerImpl::produce()`
    /// that triggers `StateResolverImpl::resolve()`.
    pub(crate) fn ensure_candidate_available(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        self.ensure_candidate_available_impl(block_id, opts, 0);
    }

    fn ensure_candidate_available_impl(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
        attempt: u32,
    ) {
        log::info!(
            target: "simplex_resolver",
            "SessionProcessor::ensure_candidate_available session_id={} block_id={} \
            purpose={:?} include_parent_chain={} attempt={}/{}",
            self.session_id().to_hex_string(),
            block_id,
            opts.purpose,
            opts.include_parent_chain,
            attempt,
            RESOLVER_AVAILABILITY_MAX_RETRIES,
        );

        let Some(candidate_id) = self.resolve_candidate_id_by_block_id(&block_id) else {
            if attempt < RESOLVER_AVAILABILITY_MAX_RETRIES {
                let next_attempt = attempt + 1;
                let expiration_time = self.now() + RESOLVER_AVAILABILITY_RETRY_DELAY;
                log::info!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; scheduling deferred retry {}/{} in {}ms",
                    block_id,
                    opts.purpose,
                    next_attempt,
                    RESOLVER_AVAILABILITY_MAX_RETRIES,
                    RESOLVER_AVAILABILITY_RETRY_DELAY.as_millis(),
                );
                self.post_delayed_action(expiration_time, move |processor| {
                    processor.ensure_candidate_available_impl(block_id, opts, next_attempt);
                });
            } else {
                log::warn!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; exhausted {RESOLVER_AVAILABILITY_MAX_RETRIES} retries, giving up",
                    block_id,
                    opts.purpose,
                );
            }
            return;
        };

        self.request_candidate_body_for_resolver(candidate_id.clone());

        if !opts.include_parent_chain {
            return;
        }

        let mut current = candidate_id;
        let mut depth = 0u32;
        let mut depth_warned = false;
        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: deep parent chain depth={} \
                    (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) for block_id={}; \
                    continuing until hard limit={MAX_CHAIN_DEPTH}",
                    depth,
                    block_id,
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: exceeded \
                    hard MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} while resolving parents for block_id={}",
                    block_id,
                );
                self.increment_error();
                break;
            }

            let parent_id = match self
                .received_candidates
                .get(&current)
                .and_then(|received| received.parent_id.clone())
            {
                Some(parent_id) => parent_id,
                None => break,
            };

            self.request_candidate_body_for_resolver(parent_id.clone());

            if !self.received_candidates.contains_key(&parent_id) {
                log::trace!(
                    target: "simplex_resolver",
                    "SessionProcessor::ensure_candidate_available: parent metadata missing at \
                    slot={} hash={} while resolving block_id={}; stopping chain traversal",
                    parent_id.slot,
                    &parent_id.hash.to_hex_string()[..8],
                    block_id,
                );
                break;
            }

            current = parent_id;
        }
    }

    /// Resolver-driven candidate body request.
    ///
    /// Unlike `request_candidate`, this path is used by validator-side state resolution and
    /// must still request a candidate even when the slot already has a skip certificate.
    /// (A slot can be skipped and still have a notarized block body needed for parent state.)
    fn request_candidate_body_for_resolver(&mut self, candidate_id: RawCandidateId) {
        let now = self.now();

        if self.has_real_candidate_body(&candidate_id) {
            return;
        }

        if let Some(next_allowed_at) = self.requested_candidates.get(&candidate_id) {
            if *next_allowed_at > now {
                log::trace!(
                    target: "simplex_resolver",
                    "Session {} request_candidate_body_for_resolver: slot={} hash={} \
                    - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                    next_allowed_at,
                );
                return;
            }
        }

        let skipped =
            self.simplex_state.has_skip_certificate_for_slot(&self.description, candidate_id.slot);

        log::debug!(
            target: "simplex_resolver",
            "Session {} request_candidate_body_for_resolver: requesting slot={} hash={} \
            immediately (skipped_slot={})",
            &self.session_id().to_hex_string()[..8],
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            skipped,
        );

        self.requested_candidates
            .insert(candidate_id.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);
        self.receiver.request_candidate(candidate_id.slot.value(), candidate_id.hash.clone());
    }

    fn cancel_candidate_repairs_for_slot(&mut self, slot: SlotIndex) {
        let before = self.requested_candidates.len();
        self.requested_candidates.retain(|candidate_id, _| candidate_id.slot != slot);
        let removed_requests = before.saturating_sub(self.requested_candidates.len());
        let removed_missing_body = self.missing_body_logged.remove(&slot.value());

        self.receiver.cancel_candidate_requests_for_slot(slot.value());

        if removed_requests > 0 || removed_missing_body {
            log::trace!(
                "Session {} cancel_candidate_repairs_for_slot: slot={slot} \
                removed_requests={removed_requests} removed_missing_body={removed_missing_body}",
                &self.session_id().to_hex_string()[..8]
            );
        }
    }

    /// Handle block finalized event
    ///
    /// Called when FSM determines a block has finalization certificate.
    /// Records finalization and applies finalized-driven delivery/state updates.
    ///
    /// This function ALWAYS processes the finalization (never blocks FSM event processing).
    /// If bodies are missing, the finalization is recorded in the journal and local application
    /// is deferred until bodies arrive (triggered by on_candidate_received).
    ///
    /// See Finalization Flow diagram above for full flow.
    fn handle_block_finalized(&mut self, event: BlockFinalizedEvent) {
        check_execution_time!(50_000);
        instrument!();

        let slot = event.slot;
        let block_hash = &event.block_hash;
        let finalized_id = RawCandidateId { slot, hash: block_hash.clone() };

        // INVARIANT CHECK - Certificate must have sufficient weight (>=2/3+1)
        // Reference: C++ pool.cpp - certificate is only created when threshold is reached
        let certificate = &event.certificate;
        let cert_weight = certificate.total_weight(&self.description);
        let total_weight = self.description.get_total_weight();
        let threshold = threshold_66(total_weight);
        debug_assert!(
            cert_weight >= threshold,
            "SessionProcessor INVARIANT VIOLATION: finalization certificate weight {} \
            is below threshold {} (total={}). This should never happen - FSM only emits \
            BlockFinalized when threshold is reached.",
            cert_weight,
            threshold,
            total_weight
        );
        if cert_weight < threshold {
            log::error!(
                "Session {} handle_block_finalized: INVARIANT VIOLATION: certificate weight {} \
                below threshold {} (total={})",
                &self.session_id().to_hex_string()[..8],
                cert_weight,
                threshold,
                total_weight
            );
            self.increment_error();
        }

        // Always keep a pending entry until recursive parent-chain materialization
        // succeeds end-to-end. This provides deterministic retries on late body/cert
        // arrivals and ensures ancestor parity is eventually reached.
        let entry = FinalizedEntry { event: event.clone(), finalized_at: self.now() };
        self.finalized_pending_body.insert(finalized_id.clone(), entry);
        self.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);

        log::debug!(
            "Session {} FINALIZED: slot={}, hash={} - recorded in journal, weight={}/{} ({:.0}%)",
            &self.session_id().to_hex_string()[..8],
            slot,
            &block_hash.to_hex_string()[..8],
            cert_weight,
            total_weight,
            100.0 * cert_weight as f64 / total_weight as f64
        );

        // Note: Certificate caching for standstill is handled in handle_finalization_reached()
        // which is triggered by SimplexEvent::FinalizationReached (emitted after BlockFinalized).

        // Attempt recursive parent-chain finalization immediately.
        self.retry_pending_recursive_finalization_for(std::slice::from_ref(&finalized_id));

        // Continue FSM event processing (do NOT push event back to queue)
    }

    /// Clean up old slot data (receiver cache + validated candidates)
    ///
    /// Removes:
    /// - Receiver: votes, dedup entries, resolver cache for slots < up_to_slot
    /// - SessionProcessor: validated candidates, received candidates
    ///
    /// Reference: validator-session/src/session_processor.rs new_round()
    ///
    /// # Arguments
    /// * `finalized_slot` - The slot that was just finalized/skipped
    fn cleanup_old_slots(&mut self, finalized_slot: SlotIndex) {
        // Calculate up_to_slot for cleanup (finalized_slot - MAX_HISTORY_SLOTS)
        let up_to_slot = if finalized_slot.value() >= MAX_HISTORY_SLOTS {
            SlotIndex::new(finalized_slot.value() - MAX_HISTORY_SLOTS + 1)
        } else {
            SlotIndex::new(0) // Don't clean up if we haven't reached MAX_HISTORY_SLOTS yet
        };

        if up_to_slot.value() == 0 {
            log::trace!(
                "Session {} cleanup_old_slots: finalized_slot={finalized_slot}, skipping cleanup \
                (not enough history yet)",
                self.session_id().to_hex_string(),
            );
            return;
        }

        log::trace!(
            "Session {} cleanup_old_slots: finalized_slot={}, cleaning up slots < {}",
            self.session_id().to_hex_string(),
            finalized_slot,
            up_to_slot
        );

        // Clean up SimplexState FSM (old windows and vote accounting)
        self.simplex_state.cleanup_slots(up_to_slot);

        // Notify receiver to cleanup old data (votes, dedup, resolver cache)
        self.receiver.cleanup(up_to_slot.value());

        // Clean up session processor's validated candidates
        self.cleanup_old_candidates(up_to_slot);
    }

    /// Clean up candidates for slots that are now old
    ///
    /// Removes both validated candidates and received candidates for old slots.
    /// Also cleans up validation state collections (keyed by RawCandidateId which contains slot).
    /// Reference: validator-session/src/session_processor.rs blocks.retain
    fn cleanup_old_candidates(&mut self, up_to_slot: SlotIndex) {
        let first_non_progressed_slot = self.simplex_state.get_first_non_progressed_slot();
        let first_non_finalized_slot = self.simplex_state.get_first_non_finalized_slot();

        let stale_generated_candidates: Vec<RawCandidateId> = self
            .generated_candidates_waiting_validation
            .keys()
            .filter(|candidate_id| candidate_id.slot < up_to_slot)
            .cloned()
            .collect();
        for candidate_id in stale_generated_candidates {
            self.note_generated_candidate_validation_missed(
                &candidate_id,
                format!("cleanup_old_candidates up_to_slot={up_to_slot}"),
            );
        }

        // Clean up validation state collections (session-level, keyed by RawCandidateId)
        self.pending_validations.retain(|id, _| id.slot >= up_to_slot);
        self.pending_approve.retain(|id| id.slot >= up_to_slot);
        self.pending_reject.retain(|id, _| id.slot >= up_to_slot);
        self.rejected.retain(|id| id.slot >= up_to_slot);
        self.approved.retain(|id, _| id.slot >= up_to_slot);
        self.self_collation_starts_by_slot.retain(|slot, _| *slot >= up_to_slot);
        self.self_collation_pending_acceptance.retain(|id, _| id.slot >= up_to_slot);
        self.validation_attempt_map.retain(|id, _| id.slot >= up_to_slot);
        self.candidate_info_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.notar_cert_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.final_cert_store_results.retain(|id, _| id.slot >= up_to_slot);
        self.skip_cert_store_results.retain(|slot, _| *slot >= up_to_slot);
        // validated_candidates is a VecDeque, retain elements for slots >= up_to_slot
        self.validated_candidates.retain(|c| c.id.slot >= up_to_slot);

        // Remove received candidates for slots < up_to_slot
        //TODO: implement cleanup of blocks for old candidates
        //self.received_candidates.retain(|_hash, c| c.slot >= up_to_slot);

        // Clean up candidate_data_cache in sync with received_candidates
        self.candidate_data_cache.retain(|id, _| id.slot >= up_to_slot);
        self.seen_broadcast_candidates.retain(|slot, _| *slot >= up_to_slot);

        // Remove stale finalized-journal entries for old slots.
        // Prune entries for slots older than the FSM cursor.
        // This map is a transient "finalized but body not yet received" buffer.
        self.finalized_pending_body.retain(|id, _| id.slot >= up_to_slot);
        self.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);

        // Prune log-throttle set to prevent unbounded growth over long sessions
        self.missing_body_logged.retain(|&slot| slot >= up_to_slot.value());
        self.finalized_delivery_sent.retain(|id| id.slot >= up_to_slot);

        // Remove pending candidate requests for slots < up_to_slot
        self.requested_candidates.retain(|id, _| id.slot >= up_to_slot);

        // Clear SlotRuntime for old slots (keep SlotEntry for outcome emission)
        // TODO: LK: optimize this
        for slot_idx in 0..up_to_slot.value() {
            let slot = SlotIndex::new(slot_idx);
            if let Some(entry) = self.slots.get_mut(&slot) {
                entry.runtime = None;
            }
        }

        log::trace!(
            "Session {} cleanup_old_candidates: cleaned up slots < {up_to_slot}, \
            first_non_progressed={first_non_progressed_slot}, \
            first_non_finalized={first_non_finalized_slot}",
            self.session_id().to_hex_string(),
        );
    }

    /// Reset per-slot state after finalization or skip
    ///
    /// Called when a slot is finalized or skipped to clean up state.
    /// Reference: validator-session/src/session_processor.rs new_round()
    ///
    /// # Arguments
    /// * `slot` - The slot that was just finalized/skipped
    fn reset_slot_state(&mut self, slot: SlotIndex) {
        check_execution_time!(10_000);

        // Validate the slot is actually progressed (finalized OR notarized OR skipped).
        //
        // Note: We check the individual slot state rather than the progress cursor position,
        // because under network failures slots can be skipped out of order.
        let is_progressed = self.simplex_state.is_slot_progressed(&self.description, slot);
        debug_assert!(
            is_progressed,
            "SessionProcessor: reset_slot_state called for non-progressed slot {} (first_non_progressed={})",
            slot,
            self.simplex_state.get_first_non_progressed_slot()
        );

        log::trace!(
            "Session {} reset_slot_state: slot={slot}, is_progressed={is_progressed}, \
            fsm_first_non_progressed_slot={}, fsm_first_non_finalized_slot={}",
            self.session_id().to_hex_string(),
            self.simplex_state.get_first_non_progressed_slot(),
            self.simplex_state.get_first_non_finalized_slot(),
        );

        // Cleanup old slot data (receiver cache + validated candidates)
        self.cleanup_old_slots(slot);

        // Self-collation safety net: if the flow never reached success or final
        // failure (e.g. crash, race, missing callback), drop the tracking entry
        // so it auto-counts as ignore (auto-derived as `total - success - failure`).
        self.forget_self_collation_tracking(slot, "reset_slot_state");

        //TODO: LK: check if this is really needed here
        self.remove_precollated_block(slot);
    }

    /// Handle SlotSkipped event from FSM
    ///
    /// Called when FSM determines finalization is no longer possible for a slot.
    fn handle_slot_skipped_event(&mut self, event: SlotSkippedEvent) {
        self.handle_slot_skipped(event.slot);
    }

    /// Handle notarization reached event
    ///
    /// Called when FSM determines notarization threshold reached for a block.
    /// Serializes and caches the notarization certificate in the receiver
    /// for responding to requestCandidate queries from other nodes.
    ///
    /// Reference: C++ CandidateResolver subscribes to NotarizationObserved
    /// and caches the NotarCertRef.
    ///
    /// Cache VoteSignatureSet bytes (not full Certificate) to match C++ wire format.
    /// C++ `candidateAndCert.notar` contains serialized `voteSignatureSet`, not `certificate`.
    fn handle_notarization_reached(&mut self, event: NotarizationReachedEvent) {
        check_execution_time!(1_000);

        let now = self.now();
        self.observability.last_notarization_at = Some(now);
        self.observability.last_notarization_slot = Some(event.slot);
        self.observability.last_notar_cert_at = Some(now);
        self.observability.last_notar_cert_slot = Some(event.slot);

        log::trace!(
            "Session {} notarization reached: slot={} block={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            &event.block_hash.to_hex_string()[..8],
            event.certificate.signatures.len()
        );

        // Save notarization certificate to DB (store async result for write ordering)
        // Reference: C++ candidate-resolver.cpp NotarizationObserved handler:
        //   store_to_db(event->id, state).start().detach()
        let candidate_id = RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };

        // If we learned notarization via foreign votes/cert but the candidate body is missing,
        // proactively request it. Otherwise, the next leader may be unable to collate due to
        // unresolved parent chain, causing timeouts and skip cascades in single-host tests.
        //
        // C++ parity intent: CandidateResolver/Pool logic requests missing candidate data
        // based on observed certificates. Finalized-boundary stubs don't count as real bodies.
        if !self.has_real_candidate_body(&candidate_id) {
            self.request_candidate(event.slot, event.block_hash.clone(), None);
        }

        if let Some(received) = self.received_candidates.get(&candidate_id) {
            if !received.is_empty {
                let observed_flags = CandidateObservedFlags {
                    body_present: true,
                    parent_ready: true,
                    local_collated: received.source_idx == self.description.get_self_idx(),
                };
                self.notify_candidate_observed(
                    received.block_id.clone(),
                    received.data.clone(),
                    received.collated_data.clone(),
                    observed_flags,
                );
            }
        }

        let notar_store_result =
            if let Some(existing) = self.notar_cert_store_results.get(&candidate_id) {
                existing.clone()
            } else {
                match self.db.save_notar_cert_async(&candidate_id, &event.certificate) {
                    Ok(result) => {
                        self.notar_cert_store_results.insert(candidate_id.clone(), result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                        "Session {} handle_notarization_reached: failed to create notar_cert save \
                        slot={}: {e}",
                        &self.session_id().to_hex_string()[..8],
                        event.slot,
                    );
                        self.increment_error();
                        return;
                    }
                }
            };

        // TN-968 wait-for-store guarantee: certificate must be durable before broadcast/cache.
        if let Err(e) = notar_store_result.wait() {
            log::error!(
                "Session {} handle_notarization_reached: failed to store notar cert slot={}: {e}",
                &self.session_id().to_hex_string()[..8],
                event.slot,
            );
            self.increment_error();
            return;
        }

        // Serialize and cache the notarization certificate for query responses
        // Use VoteSignatureSet (not full Certificate) to match C++ wire format.
        // Reference: C++ candidate-resolver.cpp to_tl():
        //   serialized_notar = serialize_tl_object((*notar_cert)->to_tl_vote_signature_set(), true);
        let tl_sigs = event.certificate.to_tl_vote_signature_set();
        match serialize_boxed(&tl_sigs) {
            Ok(notar_cert_bytes) => {
                log::trace!(
                    "Session {} handle_notarization_reached: caching VoteSignatureSet for slot={} \
                    hash={} ({}B)",
                    &self.session_id().to_hex_string()[..8],
                    event.slot,
                    &event.block_hash.to_hex_string()[..8],
                    notar_cert_bytes.len(),
                );
                self.receiver.cache_notarization_cert(
                    event.slot.value(),
                    event.block_hash.clone(),
                    notar_cert_bytes,
                );
            }
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to serialize \
                    VoteSignatureSet: {e}",
                    &self.session_id().to_hex_string()[..8],
                );
                self.increment_error();
            }
        }

        // Broadcast full notarization certificate to all validators
        // Reference: C++ pool.cpp broadcasts certificate on NotarizationObserved
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to convert to TL: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
                return;
            }
        };

        // Serialize for standstill cache
        match serialize_boxed(&tl_cert) {
            Ok(cert_bytes) => {
                log::trace!(
                    "Session {} handle_notarization_reached: broadcasting notar cert for slot={} \
                    ({}B)",
                    &self.session_id().to_hex_string()[..8],
                    event.slot,
                    cert_bytes.len(),
                );

                // C++ parity (pool.cpp handle_saved_certificate): relay every newly
                // accepted certificate to all validators. Dedup is in SimplexState.
                self.certs_relayed_counter.increment(1);
                self.receiver.send_certificate(tl_cert);

                // Cache for standstill re-broadcast
                self.receiver.cache_standstill_certificate(
                    event.slot.value(),
                    StandstillCertificateType::Notar,
                    cert_bytes,
                );
            }
            Err(e) => {
                log::error!(
                    "Session {} handle_notarization_reached: failed to serialize certificate: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
            }
        }
    }

    /// Handle skip certificate reached event
    ///
    /// Called when FSM determines skip threshold reached for a slot.
    /// Serializes and broadcasts the skip certificate to all validators.
    ///
    /// Reference: C++ pool.cpp creates skip certificate and broadcasts it
    fn handle_skip_certificate_reached(&mut self, event: SkipCertificateReachedEvent) {
        check_execution_time!(1_000);

        log::trace!(
            "Session {} skip certificate reached: slot={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            event.certificate.signatures.len()
        );

        let skip_store_result =
            if let Some(existing) = self.skip_cert_store_results.get(&event.slot) {
                existing.clone()
            } else {
                match self.db.save_skip_cert_async(event.slot, &event.certificate) {
                    Ok(result) => {
                        self.skip_cert_store_results.insert(event.slot, result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                            "Session {} handle_skip_certificate_reached: failed to create skip \
                            cert save slot={}: {e}",
                            &self.session_id().to_hex_string()[..8],
                            event.slot,
                        );
                        self.increment_error();
                        return;
                    }
                }
            };

        // TN-968 wait-for-store guarantee: certificate must be durable before broadcast/cache.
        if let Err(e) = skip_store_result.wait() {
            log::error!(
                "Session {} handle_skip_certificate_reached: failed to store skip cert slot={}: \
                {e}",
                &self.session_id().to_hex_string()[..8],
                event.slot,
            );
            self.increment_error();
            return;
        }

        // Convert to TL format
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_skip_certificate_reached: failed to convert to TL: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
                return;
            }
        };

        // Serialize for caching
        match serialize_boxed(&tl_cert) {
            Ok(cert_bytes) => {
                log::trace!(
                    "Session {} handle_skip_certificate_reached: broadcasting skip cert for \
                    slot={} ({}B)",
                    &self.session_id().to_hex_string()[..8],
                    event.slot,
                    cert_bytes.len(),
                );

                // Send certificate to all validators
                self.certs_relayed_counter.increment(1);
                self.receiver.send_certificate(tl_cert);

                // Cache for standstill re-broadcast
                self.receiver.cache_standstill_certificate(
                    event.slot.value(),
                    StandstillCertificateType::Skip,
                    cert_bytes,
                );
            }
            Err(e) => {
                log::error!(
                    "Session {} handle_skip_certificate_reached: failed to serialize certificate: \
                    {e}",
                    &self.session_id().to_hex_string()[..8],
                );
                self.increment_error();
            }
        }
    }

    /// Handle finalization reached event
    ///
    /// Called when FSM determines finalization threshold reached for a block.
    /// Always caches the finalization certificate for standstill replay.
    /// Relays certificate to all validators (C++ parity: handle_saved_certificate).
    fn handle_finalization_reached(&mut self, event: FinalizationReachedEvent) {
        check_execution_time!(1_000);

        let now = self.now();
        self.observability.last_final_cert_at = Some(now);
        self.observability.last_final_cert_slot = Some(event.slot);

        log::trace!(
            "Session {} finalization reached: slot={} block={} sigs={}",
            self.session_id().to_hex_string(),
            event.slot,
            &event.block_hash.to_hex_string()[..8],
            event.certificate.signatures.len()
        );

        let candidate_id = RawCandidateId { slot: event.slot, hash: event.block_hash.clone() };
        let final_store_result =
            if let Some(existing) = self.final_cert_store_results.get(&candidate_id) {
                existing.clone()
            } else {
                match self.db.save_final_cert_async(&candidate_id, &event.certificate) {
                    Ok(result) => {
                        self.final_cert_store_results.insert(candidate_id, result.clone());
                        result
                    }
                    Err(e) => {
                        log::error!(
                            "Session {} handle_finalization_reached: failed to create final cert \
                            save slot={}: {e}",
                            &self.session_id().to_hex_string()[..8],
                            event.slot,
                        );
                        self.increment_error();
                        return;
                    }
                }
            };

        // TN-968 wait-for-store guarantee: certificate must be durable before broadcast/cache.
        if let Err(e) = final_store_result.wait() {
            log::error!(
                "Session {} handle_finalization_reached: failed to store final cert slot={}: {e}",
                &self.session_id().to_hex_string()[..8],
                event.slot,
            );
            self.increment_error();
            return;
        }

        // Convert to TL format
        let tl_cert = match event.certificate.to_tl() {
            Ok(cert) => cert,
            Err(e) => {
                log::error!(
                    "Session {} handle_finalization_reached: failed to convert to TL: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
                return;
            }
        };

        // Serialize for broadcast + caching
        match serialize_boxed(&tl_cert) {
            Ok(cert_bytes) => {
                // C++ parity (pool.cpp handle_saved_certificate): relay every newly
                // accepted certificate to all validators. Dedup is in SimplexState.
                log::trace!(
                    "Session {} handle_finalization_reached: \
                    broadcasting final cert for slot={} ({}B)",
                    &self.session_id().to_hex_string()[..8],
                    event.slot,
                    cert_bytes.len(),
                );
                self.certs_relayed_counter.increment(1);
                self.receiver.send_certificate(tl_cert);

                // Cache per-slot final certificate (for bundle replay)
                self.receiver.cache_standstill_certificate(
                    event.slot.value(),
                    StandstillCertificateType::Final,
                    cert_bytes.clone(),
                );

                // Cache last final certificate (always replayed first on standstill)
                self.receiver.cache_last_final_certificate(event.slot.value(), cert_bytes);

                // Update standstill state (timer + tracked slots range)
                self.update_standstill_after_final_cert(event.slot);
            }
            Err(e) => {
                log::error!(
                    "Session {} handle_finalization_reached: failed to serialize certificate: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
                self.increment_error();
            }
        }
    }

    /// Update standstill state after storing a finalization certificate
    ///
    /// Called when a final certificate is stored (local or foreign).
    /// Reschedules standstill timer and updates tracked slots range.
    ///
    /// Reference: C++ handle_certificate(FinalCertRef) calls reschedule_standstill_resolution()
    /// and updates first_nonfinalized_slot_ which affects tracked_slots_interval()
    fn update_standstill_after_final_cert(&self, slot: SlotIndex) {
        // Reschedule standstill timer
        self.receiver.reschedule_standstill();

        // Update standstill tracked slots range
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        self.sync_standstill_slots_from_state();

        log::trace!(
            "Session {} update_standstill_after_final_cert: slot={} tracked_slots=[{}, {})",
            &self.session_id().to_hex_string()[..8],
            slot,
            begin,
            end
        );
    }

    fn handle_slot_skipped(&mut self, slot: SlotIndex) {
        check_execution_time!(10_000);
        instrument!();

        self.skip_total_counter.increment(1);

        log::debug!(
            "Session {} SKIP: slot={} (no ValidatorGroup callback in roundless mode)",
            &self.session_id().to_hex_string()[..8],
            slot
        );

        // Record slot duration metric
        if let Ok(duration) = self.now().duration_since(self.slot_started_at(slot)) {
            self.slot_duration_histogram.record(duration.as_millis() as f64);
        }

        // FSM already updated first_non_finalized_slot and cleaned up internally
        // Reset per-slot state for this slot
        self.reset_slot_state(slot);
        self.cancel_candidate_repairs_for_slot(slot);

        // Update standstill tracked slots range (but DO NOT reschedule standstill on skip)
        // Reference: C++ pool.cpp on_skip() does NOT call reschedule_standstill_resolution()
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        self.sync_standstill_slots_from_state();

        // C++ parity: do NOT remove precollated blocks for same-window slots.
        // C++ block-producer.cpp only cancels collations at window transitions
        // (via CancellationTokenSource replacement), not on per-slot skip events.
        // Removing the entry here destroys the locked parent context that a
        // late same-window callback needs to publish the candidate.
        let slot_window = self.description.get_window_idx(slot);
        let current_window = self.simplex_state.get_current_leader_window_idx();
        if slot_window != current_window {
            self.forget_self_collation_tracking(slot, "skipped_window_mismatch");
            self.remove_precollated_block(slot);
        }

        log::trace!(
            "Session {} handle_slot_skipped: completed slot={} tracked_slots=[{}, {})",
            &self.session_id().to_hex_string()[..8],
            slot,
            begin,
            end
        );
    }

    /*
        Callback invocation
    */

    /// Invoke callback closure - checks use_callback_thread flag
    /// and either posts to callback queue or executes immediately.
    ///
    /// Suppresses all callbacks when `stop_flag` is set (session shutdown).
    /// This prevents notifying ValidatorGroup about events from old sessions
    /// after masterchain configuration has rotated to a new validator set.
    fn invoke_session_callback<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // Suppress callbacks during session shutdown (validator-group compatibility).
        // When stop_flag is set, the ValidatorGroup may have already rotated to a new
        // catchain_seqno, so notifying about old-session events would cause errors.
        if self.stop_flag.load(Ordering::Relaxed) {
            log::trace!(
                "Session {} invoke_session_callback: suppressed during shutdown",
                self.session_id().to_hex_string()
            );
            return;
        }

        if self.use_callback_thread {
            // Use callback thread - post to callback task queue
            post_callback_closure(&self.callbacks_task_queue, callback);
        } else {
            // Execute callback immediately in current thread
            callback();
        }
    }

    /*
        Listener notification methods

        Note: These methods use validator-session's SessionListener trait.
        The exact signatures will be adjusted when Simplex-specific listener is defined.
        For now, we use compatible wrapper calls.
    */

    /// Notify listener about a block candidate for validation
    ///
    /// Called when a block broadcast is received and needs validation.
    fn notify_candidate(
        &self,
        source_info: crate::BlockSourceInfo,
        root_hash: crate::BlockHash,
        data: crate::BlockPayloadPtr,
        collated_data: crate::BlockPayloadPtr,
        callback: crate::ValidatorBlockCandidateDecisionCallback,
    ) {
        check_execution_time!(20_000);

        log::trace!(
            "Session {} notify_candidate: posting on_candidate event for root_hash={:x}",
            self.session_id().to_hex_string(),
            root_hash
        );

        let listener = self.listener.clone();

        self.invoke_session_callback(move || {
            check_execution_time!(20_000);

            if let Some(listener) = listener.upgrade() {
                log::trace!("SessionProcessor::notify_candidate: on_candidate start");

                listener.on_candidate(source_info, root_hash, data, collated_data, callback);

                log::trace!("SessionProcessor::notify_candidate: on_candidate finish");
            }
        });
    }

    fn notify_candidate_observed(
        &self,
        block_id: BlockIdExt,
        data: crate::BlockPayloadPtr,
        collated_data: crate::BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        check_execution_time!(20_000);

        log::trace!(
            "Session {} notify_candidate_observed: block_id={} parent_ready={} local_collated={} body_present={}",
            self.session_id().to_hex_string(),
            block_id,
            flags.parent_ready,
            flags.local_collated,
            flags.body_present,
        );
        log::info!(
            target: "simplex_resolver",
            "SessionProcessor::notify_candidate_observed session_id={} block_id={} parent_ready={} local_collated={} body_present={}",
            self.session_id().to_hex_string(),
            block_id,
            flags.parent_ready,
            flags.local_collated,
            flags.body_present,
        );

        let listener = self.listener.clone();
        self.invoke_session_callback(move || {
            check_execution_time!(20_000);

            if let Some(listener) = listener.upgrade() {
                listener.on_candidate_observed(block_id, data, collated_data, flags);
            }
        });
    }

    /// Notify listener about a generation slot
    ///
    /// Called when this validator should generate a block.
    fn notify_generate_slot(
        &self,
        slot: SlotIndex,
        source_info: crate::BlockSourceInfo,
        request: crate::AsyncRequestPtr,
        parent: Option<crate::block::CandidateParentInfo>,
        prev_block_ids: Vec<BlockIdExt>,
        callback: crate::ValidatorBlockCandidateCallback,
    ) {
        check_execution_time!(20_000);

        log::trace!(
            "Session {} notify_generate_slot: posting on_generate_slot event",
            self.session_id().to_hex_string()
        );

        assert!(
            !prev_block_ids.is_empty() && prev_block_ids.len() <= 2,
            "SessionProcessor INVARIANT VIOLATION: notify_generate_slot requires one or two explicit prev blocks for slot {}",
            slot
        );
        if let Some(parent_info) = parent.as_ref() {
            let parent_block_id = self.resolve_parent_block_id(parent_info).unwrap_or_else(|| {
                log::error!(
                    "Session {} notify_generate_slot: parent BlockIdExt is not resolved \
                    for slot {slot} (parent={parent_info})",
                    self.session_id().to_hex_string(),
                );
                panic!(
                    "SessionProcessor INVARIANT VIOLATION: unresolved parent BlockIdExt for slot {} (parent={})",
                    slot,
                    parent_info
                );
            });
            assert_eq!(
                prev_block_ids.len(),
                1,
                "SessionProcessor INVARIANT VIOLATION: non-bootstrap Simplex collation must have exactly one parent"
            );
            assert_eq!(
                prev_block_ids[0], parent_block_id,
                "SessionProcessor INVARIANT VIOLATION: explicit prev block does not match resolved parent for slot {}",
                slot
            );
        }
        log::trace!(
            "Session {} notify_generate_slot: explicit prevs for slot {}: {}",
            self.session_id().to_hex_string(),
            slot,
            prev_block_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
        );
        let expected_block_id = self
            .self_collation_starts_by_slot
            .get(&slot)
            .map(|(_, exp)| self.format_expected_block_id(*exp))
            .unwrap_or_else(|| "unknown".to_string());
        log::info!(
            "Session {} COLLATION_FLOW handoff: expected_block_id={} slot={} prev_count={} \
            target=SessionListener::on_generate_slot",
            &self.session_id().to_hex_string()[..8],
            expected_block_id,
            slot,
            prev_block_ids.len(),
        );
        let parent_hint = consensus_common::CollationParentHint::Explicit(prev_block_ids);

        let listener = self.listener.clone();

        self.invoke_session_callback(move || {
            check_execution_time!(20_000);

            if let Some(listener) = listener.upgrade() {
                log::trace!("SessionProcessor::notify_generate_slot: on_generate_slot start");

                listener.on_generate_slot(source_info, request, parent_hint, callback);

                log::trace!("SessionProcessor::notify_generate_slot: on_generate_slot finish");
            }
        });
    }

    /// Retry all triggers currently stored in `finalized_pending_body`.
    ///
    /// This is the broad "scan everything pending" entrypoint used from
    /// periodic/auxiliary paths (candidate arrivals, notar cert arrivals, etc).
    /// We sort by `(slot, hash)` to keep retries deterministic across runs.
    fn retry_pending_recursive_finalization(&mut self) {
        // Snapshot current trigger ids from the pending journal.
        let mut pending_ids: Vec<RawCandidateId> =
            self.finalized_pending_body.keys().cloned().collect();
        if pending_ids.is_empty() {
            return;
        }

        // Stable order helps reproducible diagnostics/callback behavior.
        pending_ids.sort_by(|left, right| {
            left.slot
                .value()
                .cmp(&right.slot.value())
                .then_with(|| left.hash.as_slice().cmp(right.hash.as_slice()))
        });

        self.retry_pending_recursive_finalization_for(&pending_ids);
    }

    /// Retry recursive finalization for an explicit set of trigger ids.
    ///
    /// Completed triggers are removed from the pending journal; unresolved ones
    /// remain queued and will be retried when missing data/certs arrive.
    fn retry_pending_recursive_finalization_for(&mut self, pending_ids: &[RawCandidateId]) {
        if pending_ids.is_empty() {
            return;
        }

        // Collect completed triggers first; remove after the pass.
        let mut completed = Vec::new();
        for trigger_id in pending_ids {
            // Clone event payload to avoid holding a map borrow across processing.
            let Some(event) =
                self.finalized_pending_body.get(trigger_id).map(|entry| entry.event.clone())
            else {
                continue;
            };

            if self.try_finalize_recursive_chain(trigger_id, &event) {
                completed.push(trigger_id.clone());
            }
        }

        if completed.is_empty() {
            return;
        }

        for trigger_id in completed {
            self.finalized_pending_body.remove(&trigger_id);
        }
        self.finalized_pending_body_gauge.set(self.finalized_pending_body.len() as f64);
    }

    /// Incrementally resolve finalized trigger + parent chain.
    ///
    /// Processing model:
    /// - The trigger is evaluated immediately (out-of-order friendly).
    /// - Each ancestor is handled independently in the same walk.
    /// - Missing data/certs for one block do not block already-ready blocks.
    ///
    /// Return value:
    /// - `true`  => all currently relevant blocks are resolved/emitted/applied.
    /// - `false` => at least one block still waits for requested data/certs.
    ///
    /// C++ parity (`consensus.cpp::finalize_blocks_inner`):
    /// The `FinalCert` context (`maybe_final_cert`) flows through empty candidates
    /// unchanged and is consumed by the first non-empty candidate. On masterchain,
    /// once the cert is consumed the walk stops (null cert + MC → early return).
    fn try_finalize_recursive_chain(
        &mut self,
        trigger_id: &RawCandidateId,
        event: &BlockFinalizedEvent,
    ) -> bool {
        let applied_floor = self.last_mc_finalized_seqno.unwrap_or(0);
        let mut complete = true;
        let mut visited = HashSet::new();
        let mut depth = 0u32;
        let mut depth_warned = false;
        let mut current_id = trigger_id.clone();

        // C++ parity: `maybe_final_cert` flows through empty candidates and is
        // consumed (dropped to null) by the first non-empty candidate.
        let mut has_final_cert = true;

        // Capture trigger's slot and candidate hash data for FinalCert signature
        // verification context (C++ `maybe_final_candidate->id.slot` /
        // `maybe_final_candidate->hash_data()`).
        let trigger_slot = trigger_id.slot;
        let trigger_candidate_hash_data: Vec<u8> = self
            .received_candidates
            .get(trigger_id)
            .map(|r| r.candidate_hash_data_bytes.clone())
            .unwrap_or_default();

        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                depth_warned = true;
                log::warn!(
                    "Session {} recursive finalization: deep chain depth={} \
                     (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    depth,
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    "Session {} recursive finalization: exceeded MAX_CHAIN_DEPTH={} \
                     while resolving trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    MAX_CHAIN_DEPTH,
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.increment_error();
                return true;
            }
            if !visited.insert(current_id.clone()) {
                log::error!(
                    "Session {} recursive finalization: parent-cycle detected at s{}:{} \
                     while resolving trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.increment_error();
                return true;
            }

            // Step 1: resolve candidate metadata/body holder.
            let Some(received) = self.received_candidates.get(&current_id).cloned() else {
                log::debug!(
                    "Session {} recursive finalization: missing candidate metadata for \
                     s{}:{}, requesting body+cert and deferring trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.request_candidate(
                    current_id.slot,
                    current_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                complete = false;
                break;
            };

            // C++ parity (`consensus.cpp`):
            // if (maybe_final_cert.is_null() && shard.is_masterchain()) { co_return; }
            // On MC, once the final cert is consumed, stop the walk.
            if self.description.get_shard().is_masterchain() && !has_final_cert {
                if !received.is_empty && !self.finalized_delivery_sent.contains(&current_id) {
                    self.finalized_delivery_sent.insert(current_id.clone());
                }
                break;
            }

            // Step 2: evaluate local readiness + floor constraints for this block only.
            // C++ parity: cert-carrying walks bypass the floor check because the
            // recursive finalization in C++ has no applied-floor guard.
            let block_below_applied_floor =
                !has_final_cert && received.block_id.seq_no() < applied_floor;
            let body_available = !received.candidate_hash_data_bytes.is_empty();

            if body_available {
                // Step 3: persist/apply local finalized state before listener callback emission.
                if !self.maybe_apply_finalized_state(&current_id, has_final_cert) {
                    complete = false;
                    break;
                }

                // Step 4: callback emission policy + signature mode is handled per block.
                self.try_emit_recursive_finalized_callback(
                    &current_id,
                    &received,
                    event,
                    has_final_cert,
                    trigger_slot,
                    &trigger_candidate_hash_data,
                    &mut complete,
                );
            } else if !block_below_applied_floor {
                log::debug!(
                    "Session {} recursive finalization: candidate body missing for s{}:{}, \
                     requesting and deferring trigger=s{}:{}",
                    &self.session_id().to_hex_string()[..8],
                    current_id.slot,
                    &current_id.hash.to_hex_string()[..8],
                    trigger_id.slot,
                    &trigger_id.hash.to_hex_string()[..8],
                );
                self.request_candidate(
                    current_id.slot,
                    current_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                complete = false;
            }

            // Step 5: stop climbing once we reached already-applied floor.
            if block_below_applied_floor {
                if !received.is_empty && !self.finalized_delivery_sent.contains(&current_id) {
                    log::debug!(
                        "Session {} recursive finalization: stopping parent walk below applied-top \
                         floor for block_id={} seqno={} floor={}",
                        &self.session_id().to_hex_string()[..8],
                        received.block_id,
                        received.block_id.seq_no(),
                        applied_floor,
                    );
                    self.finalized_delivery_sent.insert(current_id.clone());
                }
                break;
            }

            // C++ parity: final cert context flows through empty candidates,
            // is consumed by the first non-empty candidate.
            // `if (is_empty) { recurse(parent, maybe_final_cert); }`
            // `else { recurse(parent, {}); }`
            if !received.is_empty {
                has_final_cert = false;
            }

            let Some(parent_id) = received.parent_id else {
                break;
            };

            current_id = parent_id;
        }

        complete
    }

    /// Attempt to emit `on_block_finalized()` for one block in recursive walk.
    ///
    /// C++ parity (`consensus.cpp::finalize_blocks_inner`):
    /// - Empty candidates never get `BlockFinalized` / `do_finalize_block`.
    /// - Non-empty candidates with `maybe_final_cert` use FinalCert signatures
    ///   and the **trigger** candidate's slot/hash_data for the signature set
    ///   (C++ `maybe_final_candidate->id.slot`, `maybe_final_candidate->hash_data()`).
    /// - Non-empty candidates without cert use NotarCert signatures and their own
    ///   slot/hash_data.
    ///
    /// `complete` is set to `false` when dependencies are missing and were requested.
    fn try_emit_recursive_finalized_callback(
        &mut self,
        candidate_id: &RawCandidateId,
        received: &ReceivedCandidate,
        event: &BlockFinalizedEvent,
        has_final_cert: bool,
        trigger_slot: SlotIndex,
        trigger_candidate_hash_data: &[u8],
        complete: &mut bool,
    ) {
        // C++ parity: empty candidates never emit BlockFinalized / do_finalize_block.
        if received.is_empty || self.finalized_delivery_sent.contains(candidate_id) {
            return;
        }

        // Protocol invariant: exactly one finalized callback per seqno.
        let seqno = received.block_id.seq_no();
        if let Some(existing_block_id) = self.finalized_delivery_sent_seqno.get(&seqno) {
            assert!(
                false,
                "Session {} protocol breach: multiple finalized callbacks for seqno={} \
                 existing_block_id={} new_block_id={}",
                &self.session_id().to_hex_string()[..8],
                seqno,
                existing_block_id,
                received.block_id,
            );
        }

        // Apply shard/MC callback emission policy for this block.
        // C++ parity: on MC, emit only when final cert context is present
        // (matches `maybe_final_cert.not_null()` gate before `BlockFinalized` publish).
        let should_emit =
            self.should_emit_on_block_finalized_for_block(&received.block_id, has_final_cert);

        if !should_emit {
            if self.description.get_shard().is_masterchain() {
                assert!(
                    !has_final_cert,
                    "Session {} invariant breach: masterchain FinalCert callback unexpectedly suppressed \
                     for block_id={} seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                );
                log::debug!(
                    "Session {} recursive finalization: skip masterchain ancestor callback \
                     without FinalCert context for block_id={} seqno={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                );
            } else {
                let applied_floor = self.last_mc_finalized_seqno.unwrap_or(0);
                log::warn!(
                    "Session {} recursive finalization: skip callback below applied-top \
                     floor for block_id={} seqno={} floor={}",
                    &self.session_id().to_hex_string()[..8],
                    received.block_id,
                    seqno,
                    applied_floor,
                );
            }
            self.finalized_delivery_sent.insert(candidate_id.clone());
            return;
        }

        // C++ parity: FinalCert context → use FinalCert signatures from the trigger
        // event; otherwise use NotarCert from this candidate's slot.
        let signatures = if has_final_cert {
            self.build_finalization_raw_signatures(&event.certificate.signatures)
        } else {
            let Some(notar_cert) =
                self.simplex_state.get_notarize_certificate(candidate_id.slot, &candidate_id.hash)
            else {
                log::debug!(
                    "Session {} recursive finalization: missing ancestor NotarCert for \
                     s{}:{}, requesting and deferring callback",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                );
                self.request_candidate(
                    candidate_id.slot,
                    candidate_id.hash.clone(),
                    Some(Duration::ZERO),
                );
                *complete = false;
                return;
            };
            self.build_finalization_raw_signatures(&notar_cert.signatures)
        };

        // C++ parity: when using FinalCert, the signature set references the
        // *trigger* candidate's slot/hash_data (C++ `maybe_final_candidate`),
        // not the current ancestor's. When using NotarCert, use this block's own.
        let (sig_slot, sig_hash_data) = if has_final_cert {
            (trigger_slot, trigger_candidate_hash_data.to_vec())
        } else {
            (received.slot, received.candidate_hash_data_bytes.clone())
        };

        let source_info = self.build_source_info_for_received(received);
        let delivered = self.notify_block_finalized(
            received.block_id.clone(),
            source_info,
            received.root_hash.clone(),
            received.file_hash.clone(),
            received.data.clone(),
            signatures,
            Vec::new(),
            sig_slot,
            sig_hash_data,
            has_final_cert,
        );
        if self.description.get_shard().is_masterchain() && has_final_cert {
            assert!(
                delivered,
                "Session {} protocol breach: failed to emit masterchain FinalCert callback \
                 for block_id={} seqno={}",
                &self.session_id().to_hex_string()[..8],
                received.block_id,
                seqno,
            );
        }
        if delivered {
            self.record_self_collation_acceptance(candidate_id, &received.block_id, has_final_cert);
            self.finalized_delivery_sent.insert(candidate_id.clone());
            let previous =
                self.finalized_delivery_sent_seqno.insert(seqno, received.block_id.clone());
            assert!(
                previous.is_none(),
                "Session {} protocol breach: duplicate finalized callback seqno={} \
                 previous_block_id={} new_block_id={}",
                &self.session_id().to_hex_string()[..8],
                seqno,
                previous.unwrap_or_else(|| received.block_id.clone()),
                received.block_id,
            );
        } else {
            *complete = false;
        }
    }

    fn should_emit_on_block_finalized_for_block(
        &self,
        block_id: &BlockIdExt,
        has_final_cert: bool,
    ) -> bool {
        if self.description.get_shard().is_masterchain() {
            return has_final_cert;
        }
        let applied_floor = self.last_mc_finalized_seqno.unwrap_or(0);
        block_id.seq_no() >= applied_floor
    }

    fn build_finalization_raw_signatures(
        &self,
        signatures: &[crate::certificate::VoteSignature],
    ) -> Vec<(crate::PublicKeyHash, crate::BlockPayloadPtr)> {
        signatures
            .iter()
            .map(|s| {
                (
                    self.description.get_source_public_key_hash(s.validator_idx).clone(),
                    consensus_common::ConsensusCommonFactory::create_block_payload(
                        s.signature.clone(),
                    ),
                )
            })
            .collect()
    }

    fn build_source_info_for_received(
        &self,
        received: &ReceivedCandidate,
    ) -> crate::BlockSourceInfo {
        let source_public_key = self.description.get_source_public_key(received.source_idx).clone();
        crate::BlockSourceInfo {
            source: source_public_key,
            priority: crate::BlockCandidatePriority {
                round: SIMPLEX_ROUNDLESS,
                priority: 0,
                first_block_round: SIMPLEX_ROUNDLESS,
            },
        }
    }

    /// Apply finalized-driven local state once the candidate body is available.
    ///
    /// This replaces the old sequential commit path for Simplex sessions:
    /// - updates local finalized/head cursors
    /// - persists finalized records for restart recovery
    /// - clears per-slot runtime once finalization is materially applied
    ///
    /// Returns `false` when persistence fails and this candidate must be retried.
    fn maybe_apply_finalized_state(
        &mut self,
        finalized_id: &RawCandidateId,
        is_final: bool,
    ) -> bool {
        if self.finalized_blocks.contains(finalized_id) {
            return true;
        }

        let Some(received) = self.received_candidates.get(finalized_id).cloned() else {
            return true;
        };

        if received.candidate_hash_data_bytes.is_empty() {
            return true;
        }

        let slot = received.slot;
        let seqno = received.block_id.seq_no();

        let slot_started_at = self.slot_started_at(slot);
        if let Ok(duration) = self.now().duration_since(slot_started_at) {
            self.slot_duration_histogram.record(duration.as_millis() as f64);
        }

        if !self.slot_first_candidate_finalized(slot) {
            self.slot_set_first_candidate_finalized(slot, true);
            if let Ok(latency) = self.now().duration_since(slot_started_at) {
                self.first_candidate_finalized_latency_histogram.record(latency.as_millis() as f64);
            }
        }

        let record = FinalizedBlockRecord {
            candidate_id: finalized_id.clone(),
            block_id: received.block_id.clone(),
            parent: received.parent_id.clone(),
            is_final,
        };

        if self.description.get_shard().is_masterchain() {
            match self.db.save_finalized_block_async(&record) {
                Ok(result) => {
                    if let Err(e) = result.wait() {
                        log::error!(
                            "Session {} maybe_apply_finalized_state: failed to store finalized block for \
                            slot={}: {e}",
                            &self.session_id().to_hex_string()[..8],
                            slot.value(),
                        );
                        self.increment_error();
                        return false;
                    }
                }
                Err(e) => {
                    log::error!(
                        "Session {} maybe_apply_finalized_state: failed to create finalized block save for \
                        slot={}: {e}",
                        &self.session_id().to_hex_string()[..8],
                        slot.value(),
                    );
                    self.increment_error();
                    return false;
                }
            }
        } else if let Err(e) = self.db.save_finalized_block(&record) {
            log::error!(
                "Session {} maybe_apply_finalized_state: failed to store finalized block for slot={}: {e}",
                &self.session_id().to_hex_string()[..8],
                slot.value(),
            );
            self.increment_error();
            return false;
        }

        let previous_head_seqno = self.finalized_head_seqno.unwrap_or(0);
        let should_replace_head = match self.finalized_head_slot {
            Some(current_slot) => {
                seqno > previous_head_seqno
                    || (seqno == previous_head_seqno && slot >= current_slot)
            }
            None => true,
        };

        if !received.is_empty {
            if seqno > previous_head_seqno {
                self.finalized_head_seqno = Some(seqno);
            }

            self.advance_accepted_normal_head_block(received.block_id.clone());

            if should_replace_head {
                if let Ok(before_split) =
                    crate::utils::extract_before_split_flag(received.data.data())
                {
                    self.finalized_head_before_split = before_split;
                    self.before_split_by_block_id.insert(received.block_id.clone(), before_split);
                    if before_split {
                        log::info!(
                            "Session {} maybe_apply_finalized_state: block at slot={slot} seqno={seqno} has \
                            before_split=true (next block MUST be empty for split/merge)",
                            &self.session_id().to_hex_string()[..8],
                        );
                    }
                } else {
                    log::trace!(
                        "Session {} maybe_apply_finalized_state: failed to extract before_split flag for \
                        slot={slot}, assuming false",
                        &self.session_id().to_hex_string()[..8],
                    );
                    self.finalized_head_before_split = false;
                }
            }
        }

        if should_replace_head {
            self.finalized_head_slot = Some(slot);
            self.finalized_head_block_id = Some(received.block_id.clone());
        }

        if seqno > self.last_consensus_finalized_seqno.unwrap_or(0) {
            self.last_consensus_finalized_seqno = Some(seqno);
        }

        self.finalized_blocks.insert(finalized_id.clone());

        self.last_finalized_slot_gauge.set(slot.0 as f64);
        let now = self.now();
        self.round_debug_at = now + ROUND_DEBUG_PERIOD;
        self.last_finalization_time = now;

        if self.simplex_state.is_slot_progressed(&self.description, slot) {
            self.reset_slot_state(slot);
        } else {
            log::trace!(
                "Session {} maybe_apply_finalized_state: skipping reset_slot_state for \
                non-progressed slot={slot}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        true
    }

    /// Emit `on_block_finalized` to the listener for finalized-driven acceptance.
    fn notify_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: crate::BlockSourceInfo,
        root_hash: crate::BlockHash,
        file_hash: crate::BlockHash,
        data: crate::BlockPayloadPtr,
        signatures: Vec<(crate::PublicKeyHash, crate::BlockPayloadPtr)>,
        approve_signatures: Vec<(crate::PublicKeyHash, crate::BlockPayloadPtr)>,
        slot: SlotIndex,
        candidate_hash_data_bytes: Vec<u8>,
        is_final: bool,
    ) -> bool {
        check_execution_time!(20_000);

        log::trace!(
            "Session {} notify_block_finalized: posting for block_id={} slot={}",
            self.session_id().to_hex_string(),
            block_id,
            slot,
        );

        let listener = self.listener.clone();

        let signatures_variant = match self.build_simplex_signatures_variant(
            &signatures,
            slot,
            candidate_hash_data_bytes,
            is_final,
        ) {
            Ok(v) => v,
            Err(e) => {
                log::error!(
                    "Session {} notify_block_finalized: failed to build signatures variant: {}",
                    self.session_id().to_hex_string(),
                    e
                );
                self.increment_error();
                return false;
            }
        };

        self.invoke_session_callback(move || {
            check_execution_time!(20_000);

            if let Some(listener) = listener.upgrade() {
                listener.on_block_finalized(
                    block_id,
                    source_info,
                    root_hash,
                    file_hash,
                    data,
                    signatures_variant,
                    approve_signatures,
                );
            }
        });
        true
    }

    /// Build BlockSignaturesVariant::Simplex from raw signature pairs with context
    ///
    /// Builds Simplex variant with session_id, slot, candidate_data, and is_final
    /// for proper signature verification in accept_block.
    ///
    /// # Invariants (checked with assert)
    /// - raw_signatures must not be empty
    /// - candidate_hash_data_bytes must not be empty
    /// - All signatures must have valid format (64 bytes for Ed25519)
    /// - Total weight must meet threshold_66 for finalized blocks
    fn build_simplex_signatures_variant(
        &self,
        raw_signatures: &[(crate::PublicKeyHash, crate::BlockPayloadPtr)],
        slot: SlotIndex,
        candidate_hash_data_bytes: Vec<u8>,
        is_final: bool,
    ) -> Result<BlockSignaturesVariant> {
        // INVARIANT: Must have at least one signature
        assert!(
            !raw_signatures.is_empty(),
            "build_simplex_signatures_variant: raw_signatures must not be empty for slot={}",
            slot
        );

        // INVARIANT: candidate_hash_data_bytes must not be empty (needed for signature verification)
        assert!(
            !candidate_hash_data_bytes.is_empty(),
            "build_simplex_signatures_variant: candidate_hash_data_bytes must not be empty for slot={}",
            slot
        );

        let mut pure_signatures = BlockSignaturesPure::new();
        let mut valid_sig_count = 0u32;
        let mut invalid_sig_count = 0u32;

        // Calculate total weight and add signature pairs
        let mut total_weight: u64 = 0;
        for (node_id, sig_payload) in raw_signatures {
            // Get validator weight by looking up the source index
            if let Ok(src_idx) = self.description.get_source_index(node_id) {
                total_weight += self.description.get_node_weight(src_idx);
            }

            // Convert raw signature bytes to CryptoSignaturePair
            let sig_bytes = sig_payload.data().to_vec();
            if sig_bytes.len() >= 64 {
                let mut r = [0u8; 32];
                let mut s = [0u8; 32];
                r.copy_from_slice(&sig_bytes[0..32]);
                s.copy_from_slice(&sig_bytes[32..64]);

                pure_signatures.add_sigpair(CryptoSignaturePair {
                    node_id_short: (*node_id.data()).into(),
                    sign: CryptoSignature::with_r_s(&r, &s),
                });
                valid_sig_count += 1;
            } else {
                invalid_sig_count += 1;
                log::warn!(
                    "build_simplex_signatures_variant: invalid signature length {} for node \
                    {node_id} at slot={slot}",
                    sig_bytes.len(),
                );
            }
        }

        // INVARIANT: All signatures must be valid (no invalid signatures)
        assert!(
            invalid_sig_count == 0,
            "build_simplex_signatures_variant: {} invalid signatures found for slot={} (valid={})",
            invalid_sig_count,
            slot,
            valid_sig_count
        );

        // INVARIANT: Must have at least one valid signature added
        assert!(
            valid_sig_count > 0,
            "build_simplex_signatures_variant: no valid signatures added for slot={}",
            slot
        );

        // INVARIANT: For finalized blocks, total weight must meet threshold_66
        let threshold = threshold_66(self.description.get_total_weight());
        assert!(
            total_weight >= threshold,
            "build_simplex_signatures_variant: total_weight {} < threshold {} for slot={} (is_final={})",
            total_weight,
            threshold,
            slot,
            is_final
        );

        pure_signatures.set_weight(total_weight);

        log::trace!(
            "build_simplex_signatures_variant: slot={} sigs={} weight={}/{} ({:.1}%)",
            slot,
            valid_sig_count,
            total_weight,
            self.description.get_total_weight(),
            100.0 * total_weight as f64 / self.description.get_total_weight() as f64
        );

        // Build BlockSignaturesSimplex with full context for signature verification
        let candidate_data =
            BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_hash_data_bytes)?;
        let simplex_signatures = BlockSignaturesSimplex::with_params(
            ValidatorBaseInfo::with_params(0, 0), // Placeholder - will be replaced in accept_block
            pure_signatures,
            self.session_id().clone(),
            slot.value() as u32,
            candidate_data,
            is_final,
        );

        Ok(BlockSignaturesVariant::Simplex(simplex_signatures))
    }

    /// Handle RequestCandidate query fallback when receiver's resolver_cache misses.
    ///
    /// Called from SXRCV thread via ReceiverListener when a peer's RequestCandidate query
    /// cannot be fully answered from the in-memory resolver_cache. Attempts to reconstruct
    /// requested candidate body and/or notar parts from:
    ///   1. `candidate_data_cache` (in-memory, fast path)
    ///   2. SimplexDB `CandidateInfoRecord` (empty blocks only -- reconstructed from metadata)
    ///
    /// Non-empty blocks not in the in-memory cache return an empty response; the
    /// querying peer will retry with other validators. This matches C++ behavior
    /// where `CandidateResolver` only loads from its own consensus DB, never from
    /// the validator manager.
    ///
    /// Reference: C++ `CandidateResolver::try_load_candidate_data_from_db()`
    /// TODO: LK: move DB operations to background thread
    pub fn handle_candidate_query_fallback(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        want_candidate: bool,
        want_notar: bool,
        response_callback: crate::QueryResponseCallback,
    ) {
        check_execution_time!(50_000);

        let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
        let session_hex = &self.session_id().to_hex_string()[..8];

        // Candidate and notar can be requested independently. Build each part
        // from the best available source and return partials when only one part exists.
        let mut candidate_bytes = Vec::new();

        if want_candidate {
            // 1. Fast path: in-memory candidate_data_cache
            if let Some(bytes) = self.candidate_data_cache.get(&candidate_id) {
                log::debug!(
                    "Session {session_hex} candidate_query_fallback: \
                    candidate cache HIT for slot={slot} hash={} ({}B)",
                    &block_hash.to_hex_string()[..8],
                    bytes.len()
                );
                candidate_bytes.clone_from(bytes);
            } else {
                // 2. DB path: candidate metadata
                let candidate_info = self.load_candidate_info_from_db(&candidate_id);

                // 3. Persisted payload (works for both empty and non-empty blocks)
                const DB_TIMEOUT: Duration = Duration::from_secs(2);
                match self.db.load_candidate_payload_by_id(&candidate_id, DB_TIMEOUT) {
                    Ok(Some(payload_bytes)) => {
                        log::debug!(
                            "Session {session_hex} candidate_query_fallback: \
                            loaded payload from DB for slot={slot} ({}B)",
                            payload_bytes.len()
                        );
                        candidate_bytes = payload_bytes;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!(
                            "Session {session_hex} candidate_query_fallback: \
                            DB payload load error for slot={slot}: {e}"
                        );
                    }
                }

                // 4. Metadata reconstruction for empty blocks when payload missing.
                if candidate_bytes.is_empty() {
                    if let Some(info) = candidate_info.as_ref() {
                        let is_empty = matches!(
                            info.candidate_hash_data,
                            CandidateHashData::Consensus_CandidateHashDataEmpty(_)
                        );
                        if is_empty {
                            match self
                                .reconstruct_empty_candidate_data_from_info(&candidate_id, info)
                            {
                                Ok(bytes) => {
                                    log::debug!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        reconstructed empty block for slot={slot} ({}B)",
                                        bytes.len()
                                    );
                                    candidate_bytes = bytes;
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        failed to reconstruct empty block for slot={slot}: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let notar_bytes =
            if want_notar { self.load_notar_cert_bytes_from_db(&candidate_id) } else { Vec::new() };

        if candidate_bytes.is_empty() && notar_bytes.is_empty() {
            log::debug!(
                "Session {} candidate_query_fallback: NOT FOUND for slot={} hash={} \
                (want_candidate={}, want_notar={})",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                want_candidate,
                want_notar,
            );
        } else {
            log::debug!(
                "Session {} candidate_query_fallback: responding slot={} hash={} \
                candidate_bytes={} notar_bytes={}",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                candidate_bytes.len(),
                notar_bytes.len()
            );
        }

        Self::send_candidate_and_cert_response(candidate_bytes, notar_bytes, response_callback);
    }

    /// Load CandidateInfoRecord from DB (blocking, used for rare query fallback).
    fn load_candidate_info_from_db(
        &self,
        candidate_id: &RawCandidateId,
    ) -> Option<crate::database::CandidateInfoRecord> {
        const DB_TIMEOUT: Duration = Duration::from_secs(2);

        match self.db.load_candidate_info_by_id(candidate_id, DB_TIMEOUT) {
            Ok(record) => record,
            Err(e) => {
                log::warn!(
                    "Session {} load_candidate_info_from_db: failed for slot={}: {}",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    e
                );
                None
            }
        }
    }

    /// Load notar cert bytes from DB (blocking, used for rare query fallback).
    fn load_notar_cert_bytes_from_db(&self, candidate_id: &RawCandidateId) -> Vec<u8> {
        const DB_TIMEOUT: Duration = Duration::from_secs(2);

        match self.db.load_notar_cert_by_id(candidate_id, DB_TIMEOUT) {
            Ok(Some(record)) => record.notar_cert_bytes,
            Ok(None) => Vec::new(),
            Err(e) => {
                log::debug!(
                    "Session {} load_notar_cert_bytes_from_db: failed for slot={}: {}",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    e
                );
                Vec::new()
            }
        }
    }

    /// Build and send CandidateAndCert response.
    fn send_candidate_and_cert_response(
        candidate_bytes: Vec<u8>,
        notar_bytes: Vec<u8>,
        response_callback: crate::QueryResponseCallback,
    ) {
        use consensus_common::ConsensusCommonFactory;

        let response =
            CandidateAndCert { candidate: candidate_bytes.into(), notar: notar_bytes.into() };

        let result = match serialize_boxed(&response.into_boxed()) {
            Ok(bytes) => Ok(ConsensusCommonFactory::create_block_payload(bytes)),
            Err(e) => Err(error!("Failed to serialize fallback response: {}", e)),
        };
        response_callback(result);
    }

    /// Reconstruct CandidateData::Consensus_Empty bytes from CandidateInfoRecord.
    fn reconstruct_empty_candidate_data_from_info(
        &self,
        candidate_id: &RawCandidateId,
        candidate_info: &crate::database::CandidateInfoRecord,
    ) -> Result<Vec<u8>> {
        let parent_id = match &candidate_info.candidate_hash_data {
            CandidateHashData::Consensus_CandidateHashDataEmpty(empty) => {
                let slot = SlotIndex(empty.parent.slot as u32);
                let hash = empty.parent.hash.clone();
                (slot, hash)
            }
            _ => return Err(error!("Expected empty hash data")),
        };

        let block_id = if let Some(rc) = self.received_candidates.get(candidate_id) {
            rc.block_id.clone()
        } else {
            return Err(error!(
                "Cannot reconstruct empty block: no block_id available for slot={}",
                candidate_id.slot
            ));
        };

        let parent =
            CandidateId { slot: parent_id.0.value() as i32, hash: parent_id.1 }.into_boxed();

        let tl_empty = CandidateDataEmpty {
            slot: candidate_id.slot.value() as i32,
            parent,
            block: block_id,
            signature: candidate_info.signature.clone(),
        };

        let candidate_data = CandidateData::Consensus_Empty(tl_empty);
        serialize_boxed(&candidate_data)
            .map_err(|e| error!("Failed to serialize empty CandidateData: {}", e))
    }
}

impl Drop for SessionProcessor {
    fn drop(&mut self) {
        log::info!("Dropping SessionProcessor for session {}", self.session_id().to_hex_string());
    }
}

/*
    SessionStartupRecoveryListener implementation

    Implements the startup recovery trait to allow SessionStartupRecoveryProcessor
    to drive the bootstrap process. Each method delegates to internal SimplexState
    operations or receiver cache updates.
*/

impl SessionStartupRecoveryListener for SessionProcessor {
    fn recovery_set_first_non_finalized_slot(&mut self, slot: SlotIndex) {
        log::trace!(
            "Session {}: recovery_set_first_non_finalized_slot({})",
            self.session_id().to_hex_string(),
            slot.value()
        );
        self.simplex_state.set_first_non_finalized_slot(slot);
        let (begin, _) = self.simplex_state.get_tracked_slots_interval();
        let progress = self.simplex_state.get_first_non_progressed_slot().value().max(begin);
        self.receiver.set_ingress_slot_begin(begin);
        self.receiver.set_ingress_progress_slot(progress);
    }

    fn recovery_on_vote(
        &mut self,
        node_idx: ValidatorIndex,
        vote: Vote,
        signature: SignatureBytes,
        raw_vote: RawVoteData,
    ) -> VoteResult {
        log::trace!(
            "Session {}: recovery_on_vote(node={}, vote={:?})",
            self.session_id().to_hex_string(),
            node_idx.value(),
            discriminant(&vote)
        );
        self.simplex_state.on_vote(&self.description, node_idx, vote, signature, raw_vote)
    }

    fn recovery_mark_slot_voted_on_restart(&mut self, vote: &Vote) {
        let slot = match vote {
            Vote::Notarize(v) => v.slot,
            Vote::Finalize(v) => v.slot,
            Vote::Skip(v) => v.slot,
        };
        log::trace!(
            "Session {}: recovery_mark_slot_voted_on_restart(slot={})",
            self.session_id().to_hex_string(),
            slot.value()
        );
        self.simplex_state.mark_slot_voted_on_restart(&self.description, vote);
    }

    fn recovery_set_first_nonannounced_window(&mut self, window: WindowIndex) {
        log::trace!(
            "Session {}: recovery_set_first_nonannounced_window({})",
            self.session_id().to_hex_string(),
            window.value()
        );
        self.first_nonannounced_window = window;
    }

    fn recovery_generate_restart_skip_votes(&mut self) -> usize {
        log::trace!(
            "Session {}: recovery_generate_restart_skip_votes(window={})",
            self.session_id().to_hex_string(),
            self.first_nonannounced_window.value()
        );
        let slots_per_window = self.description.opts().slots_per_leader_window;
        self.simplex_state
            .generate_restart_skip_votes(self.first_nonannounced_window, slots_per_window)
            as usize
    }

    fn recovery_drain_startup_events(&mut self) -> Vec<Vote> {
        log::trace!("Session {}: recovery_drain_startup_events", self.session_id().to_hex_string());

        // Drain all events, keeping only BroadcastVote
        let mut kept_votes = Vec::new();
        let mut dropped_finalized = 0u32;
        let mut dropped_skipped = 0u32;
        let mut dropped_notarization = 0u32;
        let mut dropped_skip_cert_reached = 0u32;
        let mut dropped_finalization_reached = 0u32;

        while let Some(event) = self.simplex_state.pull_event() {
            match event {
                SimplexEvent::BroadcastVote(vote) => {
                    kept_votes.push(vote);
                }
                SimplexEvent::BlockFinalized(_) => {
                    dropped_finalized += 1;
                }
                SimplexEvent::SlotSkipped(_) => {
                    dropped_skipped += 1;
                }
                SimplexEvent::NotarizationReached(_) => {
                    dropped_notarization += 1;
                }
                SimplexEvent::SkipCertificateReached(_) => {
                    dropped_skip_cert_reached += 1;
                }
                SimplexEvent::FinalizationReached(_) => {
                    dropped_finalization_reached += 1;
                }
            }
        }

        log::info!(
            "Session {}: drained startup events: kept {} votes, dropped {dropped_finalized} \
            finalized, {dropped_skipped} skipped, {dropped_notarization} notarization, \
            {dropped_skip_cert_reached} skip_cert_reached, \
            {dropped_finalization_reached} finalization_reached",
            self.session_id().to_hex_string(),
            kept_votes.len(),
        );

        kept_votes
    }

    fn recovery_restore_startup_votes(&mut self, votes: Vec<Vote>) {
        log::trace!(
            "Session {}: recovery_restore_startup_votes(count={})",
            self.session_id().to_hex_string(),
            votes.len()
        );

        // Push votes back to the front of the queue in reverse order
        // so they come out in the original order when pulled
        for vote in votes.into_iter().rev() {
            self.simplex_state.push_event_front(SimplexEvent::BroadcastVote(vote));
        }
    }

    fn recovery_seed_current_round(&mut self, round: u32) {
        // NOTE(Option B): current_round removed - round is now derived from slot at emit time.
        // This function is now a no-op but kept for trait compatibility.
        log::debug!(
            target: "startup_recovery",
            "Session {}: recovery_seed_current_round({}) - no-op (round=slot model)",
            self.session_id().to_hex_string(),
            round
        );
    }

    fn recovery_seed_finalized_block(
        &mut self,
        slot: crate::block::SlotIndex,
        block_hash: UInt256,
    ) {
        log::trace!(
            target: "startup_recovery",
            "Session {}: seeding finalized block slot={}, hash={}",
            self.session_id().to_hex_string(),
            slot.value(),
            block_hash.to_hex_string()
        );

        self.finalized_blocks.insert(RawCandidateId { slot, hash: block_hash });
    }

    fn recovery_seed_received_candidates(&mut self, finalized_blocks: &[FinalizedBlockRecord]) {
        log::info!(
            target: "startup_recovery",
            "Session {}: seeding {} finalized blocks into received_candidates for parent \
            resolution",
            self.session_id().to_hex_string(),
            finalized_blocks.len(),
        );

        for block in finalized_blocks {
            let slot = block.candidate_id.slot;
            let block_hash = block.candidate_id.hash.clone();
            let block_id = block.block_id.clone();
            let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
            let is_empty = block
                .parent
                .as_ref()
                .and_then(|parent_id| self.received_candidates.get(parent_id))
                .is_some_and(|parent| parent.block_id == block_id);

            // Skip if already present (shouldn't happen, but be safe)
            if self.received_candidates.contains_key(&candidate_id) {
                if !is_empty {
                    self.finalized_delivery_sent.insert(candidate_id.clone());
                    let seqno = block_id.seq_no();
                    if let Some(existing_block_id) = self.finalized_delivery_sent_seqno.get(&seqno)
                    {
                        assert!(
                            false,
                            "Session {} protocol breach: duplicate finalized seqno={} while seeding \
                             existing_block_id={} new_block_id={}",
                            &self.session_id().to_hex_string()[..8],
                            seqno,
                            existing_block_id,
                            block_id,
                        );
                    }
                    self.finalized_delivery_sent_seqno.insert(seqno, block_id);
                }
                continue;
            }

            // Seed a minimal received candidate record for restart-side parent/tip lookups.
            self.received_candidates.insert(
                candidate_id.clone(),
                ReceivedCandidate {
                    slot,
                    source_idx: self.description.get_self_idx(),
                    candidate_id_hash: block_hash.clone(),
                    candidate_hash_data_bytes: Vec::new(),
                    block_id: block_id.clone(),
                    root_hash: block_id.root_hash.clone(),
                    file_hash: block_id.file_hash.clone(),
                    data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
                    collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
                    gen_utime_ms: None,
                    receive_time: self.now(),
                    is_empty,
                    parent_id: block.parent.clone(),
                },
            );

            if !is_empty {
                self.finalized_delivery_sent.insert(candidate_id);
                let seqno = block_id.seq_no();
                if let Some(existing_block_id) = self.finalized_delivery_sent_seqno.get(&seqno) {
                    assert!(
                        false,
                        "Session {} protocol breach: duplicate finalized seqno={} while seeding \
                         existing_block_id={} new_block_id={}",
                        &self.session_id().to_hex_string()[..8],
                        seqno,
                        existing_block_id,
                        block_id,
                    );
                }
                self.finalized_delivery_sent_seqno.insert(seqno, block_id);
            }
        }

        log::debug!(
            target: "startup_recovery",
            "Session {}: seeded {} received candidates",
            self.session_id().to_hex_string(),
            finalized_blocks.len()
        );
    }

    fn recovery_seed_candidate_for_parent_resolution(
        &mut self,
        candidate_id: RawCandidateId,
        leader_idx: ValidatorIndex,
        block_id: BlockIdExt,
        parent: Option<RawCandidateId>,
        is_empty: bool,
        candidate_hash_data_bytes: Vec<u8>,
    ) {
        log::trace!(
            target: "startup_recovery",
            "Session {}: recovery_seed_candidate_for_parent_resolution(slot=s{}, hash={}, \
            leader=v{:03}, parent={:?}, is_empty={is_empty})",
            &self.session_id().to_hex_string()[..8],
            candidate_id.slot.value(),
            &candidate_id.hash.to_hex_string()[..8],
            leader_idx.value(),
            parent.as_ref()
                .map(|p| format!("s{}:{}", p.slot.value(), &p.hash.to_hex_string()[..8])),
        );

        if let Some(existing) = self.received_candidates.get_mut(&candidate_id) {
            existing.source_idx = leader_idx;
            existing.candidate_hash_data_bytes = candidate_hash_data_bytes;
            existing.block_id.clone_from(&block_id);
            existing.root_hash.clone_from(&block_id.root_hash);
            existing.file_hash.clone_from(&block_id.file_hash);
            existing.gen_utime_ms = None;
            existing.is_empty = is_empty;
            existing.parent_id = parent;
            return;
        }

        self.received_candidates.insert(
            candidate_id.clone(),
            ReceivedCandidate {
                slot: candidate_id.slot,
                source_idx: leader_idx,
                candidate_id_hash: candidate_id.hash.clone(),
                candidate_hash_data_bytes,
                block_id: block_id.clone(),
                root_hash: block_id.root_hash.clone(),
                file_hash: block_id.file_hash.clone(),
                data: consensus_common::ConsensusCommonFactory::create_block_payload(Vec::new()),
                collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                    Vec::new(),
                ),
                gen_utime_ms: None,
                receive_time: self.now(),
                is_empty,
                parent_id: parent,
            },
        );
    }

    fn recovery_notify_last_finalized(
        &mut self,
        slot: crate::block::SlotIndex,
        block_hash: UInt256,
        seqno: u32,
    ) {
        log::info!(
            target: "startup_recovery",
            "Session {}: last finalized notification on restart: slot={}, seqno={}, hash={}",
            self.session_id().to_hex_string(),
            slot.value(),
            seqno,
            block_hash.to_hex_string()
        );

        // Look up the BlockIdExt from received_candidates (already seeded by recovery_seed_received_candidates)
        let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
        let block_id =
            self.received_candidates.get(&candidate_id).map(|r| r.block_id.clone()).unwrap_or_else(
                || {
                    log::warn!(
                        target: "startup_recovery",
                        "Session {}: recovery_notify_last_finalized: block not found in \
                        received_candidates (slot={}, hash={})",
                        self.session_id().to_hex_string(),
                        slot.value(),
                        block_hash.to_hex_string(),
                    );
                    // Fallback: construct minimal BlockIdExt
                    BlockIdExt {
                        shard_id: self.description.get_shard().clone(),
                        seq_no: seqno,
                        root_hash: block_hash.clone(),
                        file_hash: block_hash.clone(),
                    }
                },
            );

        // Update last_committed tracking to reflect the restart state
        self.finalized_head_seqno = Some(seqno);
        self.finalized_head_slot = Some(slot);
        self.finalized_head_block_id = Some(block_id.clone());
        self.last_mc_finalized_seqno = Some(self.last_mc_finalized_seqno.unwrap_or(0).max(seqno));
        self.last_consensus_finalized_seqno = Some(seqno);
        self.advance_accepted_normal_head_block(block_id.clone());

        // Note: We do NOT set available_base here anymore. This is now done in
        // recovery_finalize_parent_chain() after all kept votes are restored,
        // because the kept votes may finalize additional slots.

        // Note: We do NOT notify ValidatorGroup here because:
        // 1. C++ only republishes finalized state, not a fresh accept callback
        // 2. The block was already accepted before restart
        // 3. Restart recovery now restores state only; no historical replay callbacks remain
    }

    fn recovery_finalize_parent_chain(&mut self) {
        // After all recovery steps complete (including kept votes restoration),
        // set up the parent chain for the first non-finalized slot.
        //
        // The kept votes may have finalized additional slots beyond what was in the DB,
        // so we must use the CURRENT first_non_finalized_slot, not the one from boot.
        let first_non_finalized = self.simplex_state.get_first_non_finalized_slot();

        // Find the parent for this slot (the last finalized block)
        let parent_slot = if first_non_finalized.value() > 0 {
            SlotIndex::new(first_non_finalized.value() - 1)
        } else {
            // Genesis case - no parent
            log::debug!(
                target: "startup_recovery",
                "Session {}: recovery_finalize_parent_chain: first_non_finalized=s0, using \
                genesis base",
                &self.session_id().to_hex_string()[..8],
            );
            return;
        };

        // Determine the parent/base candidate for `first_non_finalized`.
        //
        // On masterchain, empty candidates are not persisted as finalizedBlock records, so the
        // immediately preceding slot may be missing from `received_candidates` after bootstrap.
        // Fall back to the latest notarized candidate <= parent_slot from simplex_state.
        let parent_info = self
            .received_candidates
            .iter()
            .find(|(id, _)| id.slot == parent_slot)
            .map(|(id, _)| crate::block::CandidateParentInfo {
                slot: id.slot,
                hash: id.hash.clone(),
            })
            .or_else(|| self.simplex_state.get_latest_notarized_candidate_up_to(parent_slot));

        match parent_info {
            Some(parent_info) => {
                self.simplex_state
                    .set_available_base_after_restart(&self.description, parent_info.clone());
                log::info!(
                    target: "startup_recovery",
                    "Session {}: recovery_finalize_parent_chain: set available_base for slot {} \
                    (parent=s{}:{})",
                    &self.session_id().to_hex_string()[..8],
                    first_non_finalized.value(),
                    parent_info.slot.value(),
                    &parent_info.hash.to_hex_string()[..8],
                );
            }
            None => {
                log::warn!(
                    target: "startup_recovery",
                    "Session {}: recovery_finalize_parent_chain: no parent found for slot {} \
                    (parent_slot=s{})",
                    &self.session_id().to_hex_string()[..8],
                    first_non_finalized.value(),
                    parent_slot.value(),
                );
            }
        }
    }

    fn recovery_cache_notarization_cert(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        notar_cert_bytes: Vec<u8>,
    ) {
        log::trace!(
            "Session {}: recovery_cache_notarization_cert(slot={}, hash={})",
            self.session_id().to_hex_string(),
            slot.value(),
            candidate_hash.to_hex_string()
        );
        self.receiver.cache_notarization_cert(slot.value(), candidate_hash, notar_cert_bytes);
    }

    fn recovery_seed_notarize_certificate(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        certificate: crate::certificate::NotarCertPtr,
    ) {
        log::trace!(
            "Session {}: recovery_seed_notarize_certificate(slot={}, hash={}, sigs={})",
            self.session_id().to_hex_string(),
            slot.value(),
            &candidate_hash.to_hex_string()[..8],
            certificate.signatures.len()
        );
        if let Err(e) = self.simplex_state.set_notarize_certificate(
            &self.description,
            slot,
            &candidate_hash,
            certificate,
        ) {
            log::error!(
                "Session {}: recovery_seed_notarize_certificate conflict slot={} hash={}: {}",
                &self.session_id().to_hex_string()[..8],
                slot.value(),
                &candidate_hash.to_hex_string()[..8],
                e
            );
            self.increment_error();
        }
    }

    fn recovery_cache_candidate_bytes(
        &mut self,
        slot: SlotIndex,
        candidate_hash: CandidateHash,
        candidate_data_bytes: Vec<u8>,
    ) {
        log::trace!(
            "Session {}: recovery_cache_candidate_bytes(slot={}, hash={})",
            self.session_id().to_hex_string(),
            slot.value(),
            candidate_hash.to_hex_string()
        );
        self.receiver.cache_candidate_bytes(slot.value(), candidate_hash, candidate_data_bytes);
    }

    fn recovery_restore_receiver_standstill_cache(&mut self, votes: &[VoteRecord]) {
        log::trace!(
            target: "startup_recovery",
            "Session {}: recovery_restore_receiver_standstill_cache(votes={})",
            self.session_id().to_hex_string(),
            votes.len()
        );

        // 1) Cache per-slot certificates for standstill (tracked range only)
        let (begin, end) = self.simplex_state.get_tracked_slots_interval();
        let bundles = self.simplex_state.collect_cached_certificates_in_range(begin, end);
        let mut cached_certs = 0u32;

        for (slot, notar, skip, final_) in bundles {
            let slot_u32 = slot.value();

            if let Some(cert) = notar {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        self.receiver.cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Notar,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: "startup_recovery",
                            "Session {}: failed to serialize restart notar cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id().to_hex_string()[..8],
                        );
                        self.increment_error();
                    }
                }
            }

            if let Some(cert) = skip {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        self.receiver.cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Skip,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: "startup_recovery",
                            "Session {}: failed to serialize restart skip cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id().to_hex_string()[..8],
                        );
                        self.increment_error();
                    }
                }
            }

            if let Some(cert) = final_ {
                match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                    Ok(bytes) => {
                        self.receiver.cache_standstill_certificate(
                            slot_u32,
                            StandstillCertificateType::Final,
                            bytes,
                        );
                        cached_certs += 1;
                    }
                    Err(e) => {
                        log::error!(
                            target: "startup_recovery",
                            "Session {}: failed to serialize restart final cert for standstill \
                            slot={slot_u32}: {e}",
                            &self.session_id().to_hex_string()[..8],
                        );
                        self.increment_error();
                    }
                }
            }
        }

        // 2) Cache last final certificate (C++ pool.cpp last_final_cert_)
        if let Some((slot, cert)) = self.simplex_state.get_last_finalize_certificate() {
            let slot_u32 = slot.value();
            match cert.to_tl().and_then(|tl| serialize_boxed(&tl).map_err(Into::into)) {
                Ok(bytes) => {
                    // Keep per-slot bundle for completeness (even if slot is outside tracked range)
                    self.receiver.cache_standstill_certificate(
                        slot_u32,
                        StandstillCertificateType::Final,
                        bytes.clone(),
                    );
                    self.receiver.cache_last_final_certificate(slot_u32, bytes);
                }
                Err(e) => {
                    log::error!(
                        target: "startup_recovery",
                        "Session {}: failed to serialize restart last_final_cert slot={}: {}",
                        &self.session_id().to_hex_string()[..8],
                        slot_u32,
                        e
                    );
                    self.increment_error();
                }
            }
        }

        // 3) Cache our historical votes for standstill replay
        let self_idx = self.description.get_self_idx();
        let mut cached_votes = 0u32;
        let mut vote_parse_errors = 0u32;

        for record in votes {
            if record.node_idx != self_idx {
                continue;
            }

            let msg = match deserialize_boxed(record.data.as_slice()) {
                Ok(m) => m,
                Err(e) => {
                    log::error!(
                        target: "startup_recovery",
                        "Session {}: failed to deserialize restart vote for standstill: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.increment_error();
                    vote_parse_errors += 1;
                    continue;
                }
            };

            let tl_vote = match msg.downcast::<TlVoteBoxed>() {
                Ok(v) => v,
                Err(_) => {
                    vote_parse_errors += 1;
                    continue;
                }
            };

            let signed = tl_vote.only();
            self.receiver.cache_our_vote_for_standstill(signed);
            cached_votes += 1;
        }

        // 4) Update receiver standstill tracked range and timer
        // This also prunes cached votes outside [begin, end).
        self.sync_standstill_slots_from_state();
        self.receiver.reschedule_standstill();

        log::info!(
            target: "startup_recovery",
            "Session {}: restored receiver standstill cache: certs_cached={cached_certs} \
            our_votes_cached={cached_votes} vote_parse_errors={vote_parse_errors} \
            tracked_slots=[{begin}, {end})",
            self.session_id().to_hex_string(),
        );
    }
}

/*
    ============================================================================
    Tests
    ============================================================================

    Tests are in a separate file but included directly to access private internals.
*/

#[cfg(test)]
#[path = "tests/test_session_processor.rs"]
mod tests;
