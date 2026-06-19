/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # Simplex Consensus Protocol
//!
//! This crate implements the Simplex consensus protocol for TON blockchain.
//!
//! ## Key Protocol Properties
//!
//! - **Conservative path only** (no fast finality/optimistic path)
//! - **Fault tolerance**: <1/3 Byzantine nodes (not 20% as in original)
//! - **Certificate threshold**: 2/3 stake weight
//! - **No erasure coding**: Simple broadcast instead of Rotor shreds
//! - **Spec mapping**: protocol rules tracked in [ton-blockchain/simplex-docs](https://github.com/ton-blockchain/simplex-docs)
//! - **Finalized-driven semantics**: `on_block_finalized()` is the delivery path; `on_block_committed()` stays legacy-only
//!
//! ## Quick Start
//!
//! ```ignore
//! use simplex::{SessionFactory, SessionOptions, SessionListenerPtr};
//! use std::sync::{Arc, Weak};
//!
//! // 1. Create overlay manager (for production use ADNL, for tests use in-process)
//! let overlay_manager = SessionFactory::create_in_process_overlay_manager(4);
//!
//! // 2. Create session with options (overlay starts warming up immediately)
//! let options = SessionOptions::default();
//! let shard = ton_block::ShardIdent::masterchain();
//! let session = SessionFactory::create_session(
//!     &options,
//!     &session_id,
//!     &shard,
//!     validator_nodes,
//!     &local_private_key,
//!     db_path,
//!     overlay_manager,
//!     session_listener,  // Weak<dyn SessionListener>
//! )?;
//!
//! // 3. Start consensus processing. Pass the previous block(s) the new
//! //    session will build on (one entry for normal flow, two for a shard
//! //    merge) and the masterchain block ID that gates external
//! //    `min_masterchain_block_id` (the same MC block for an MC session).
//! let prev_blocks = vec![previous_block_id];          // BlockIdExt
//! let min_masterchain_block_id = masterchain_block;   // BlockIdExt
//! session.start(prev_blocks, min_masterchain_block_id);
//!
//! // 4. Session runs in background threads, callbacks via SessionListener
//! // 5. Stop when done
//! session.stop();
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         Higher Level                                │
//! │              (Validator Manager, implements SessionListener)        │
//! └─────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    │ SessionListener callbacks
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ simplex crate                                                       │
//! │                                                                     │
//! │  SessionFactory ──creates──► Session (multi-threaded wrapper)       │
//! │                                  │                                  │
//! │                                  ├── SXMAIN thread (consensus)      │
//! │                                  ├── SXCB thread (callbacks)        │
//! │                                  └── Receiver ─── SXRCV thread      │
//! └─────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    │ ConsensusOverlayManager
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         Lower Level                                 │
//! │  (ConsensusOverlayManager from consensus-common over ADNL / QUIC)   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Integration Points
//!
//! ### Lower Level (Network)
//!
//! Uses [`ConsensusOverlayManager`] from consensus-common for network communication:
//! - [`SessionFactory::create_adnl_overlay_manager`] - Production ADNL overlay
//! - [`SessionFactory::create_in_process_overlay_manager`] - Testing overlay
//!
//! ### Higher Level (Validator)
//!
//! Implements callbacks via [`SessionListener`] trait (from `consensus-common`):
//! - `on_candidate` - Validate incoming block candidate
//! - `on_generate_slot` - Generate new block when leader
//! - `on_block_finalized` - Finalized block delivered to validator side
//! - `on_block_committed` - legacy sequential callback; not used by Simplex
//! - `on_block_skipped` - Slot was skipped
//! - `on_candidate_observed` - every observed candidate (body, parent_ready,
//!   local_collated flags); validator forwards into `StateResolverCache`
//!   so collation/validation can race the cache against `engine.wait_state()`
//!
//! ### State-resolver bridge
//!
//! - [`SimplexSession::ensure_candidate_available`] is the resolver-driven
//!   repair entry point. The validator-side `StateResolverCache` calls it
//!   when it needs a body / parent chain that is not yet observed; the
//!   request is posted onto the main task queue so the validator side
//!   never blocks on Simplex.
//!
//! ## Type Relationships
//!
//! ```text
//! SessionFactory
//!     ├── create_session() ──► SessionPtr (Arc<dyn Session>)
//!     └── create_*_overlay_manager() ──► ConsensusOverlayManagerPtr
//!
//! SessionOptions ──configures──► Session
//!     ├── slots_per_leader_window
//!     ├── first_block_timeout_multiplier, first_block_timeout_cap
//!     ├── target_rate_ms, first_block_timeout
//!     └── use_callback_thread
//!
//! SessionListener (trait, implemented by caller)
//!     ├── on_candidate()
//!     ├── on_generate_slot()
//!     ├── on_block_finalized()
//!     └── on_block_skipped()
//!
//! Receiver (trait) ──sends──► Votes, BlockBroadcasts
//! ReceiverListener (trait) ──receives──► Votes, BlockBroadcasts
//! ```
//!
//! ## Re-exports
//!
//! This crate re-exports commonly used types from `consensus-common` for
//! convenience (validator-side bridges, listener trait, payload pointers,
//! `CandidateObservedFlags`, `EnsureCandidateAvailabilityOptions`, etc.).
//! See the individual type documentation for details.

#![allow(clippy::too_many_arguments)]

/// Modules
mod block;
mod certificate;
mod database;
mod misbehavior;
mod prometheus_publisher;
mod receiver;
mod session;
mod session_description;
mod session_processor;
mod simplex_state;
mod startup_recovery;
mod task_queue;
pub mod utils;

/// Internal tests (private unit tests with crate access)
#[cfg(test)]
mod tests;

/*
    Imported consensus dependencies from consensus-common
*/
/// Metrics handle for profiling
pub use consensus_common::utils::MetricsHandle;
/// Activity node for liveness tracking
pub use consensus_common::ActivityNode;
/// Activity node pointer
pub use consensus_common::ActivityNodePtr;
/// Async request interface
pub use consensus_common::AsyncRequest;
/// Async request pointer
pub use consensus_common::AsyncRequestPtr;
/// Block candidate priority
pub use consensus_common::BlockCandidatePriority;
/// Block hash
pub use consensus_common::BlockHash;
/// Block payload pointer
pub use consensus_common::BlockPayloadPtr;
/// Block signature
pub use consensus_common::BlockSignature;
/// Block source information
pub use consensus_common::BlockSourceInfo;
/// Candidate observation flags for resolver
pub use consensus_common::CandidateObservedFlags;
/// Overlay for consensus communication
pub use consensus_common::ConsensusOverlay;
/// Overlay listener
pub use consensus_common::ConsensusOverlayListener;
/// Overlay listener pointer
pub use consensus_common::ConsensusOverlayListenerPtr;
/// Overlay log replay listener
pub use consensus_common::ConsensusOverlayLogReplayListener;
/// Overlay log replay listener pointer
pub use consensus_common::ConsensusOverlayLogReplayListenerPtr;
/// Overlay manager for consensus
pub use consensus_common::ConsensusOverlayManager;
/// Overlay manager pointer
pub use consensus_common::ConsensusOverlayManagerPtr;
/// Consensus replay listener
pub use consensus_common::ConsensusReplayListener;
/// Options for ensure_candidate_available
pub use consensus_common::EnsureCandidateAvailabilityOptions;
/// Log player for replay
pub use consensus_common::LogPlayer;
/// Log replay options
pub use consensus_common::LogReplayOptions;
/// Private key type
pub use consensus_common::PrivateKey;
/// Public key type
pub use consensus_common::PublicKey;
/// Public key hash type
pub use consensus_common::PublicKeyHash;
/// Query response callback
pub use consensus_common::QueryResponseCallback;
/// Raw data buffer
pub use consensus_common::RawBuffer;
/// Session trait (multi-threaded wrapper)
pub use consensus_common::Session as ConsensusSession;
/// Session identifier
pub use consensus_common::SessionId;
/// Session listener trait
pub use consensus_common::SessionListener;
/// Session node description
pub use consensus_common::SessionNode;
/// Validator block candidate
pub use consensus_common::ValidatorBlockCandidate;
/// Validator block candidate callback
pub use consensus_common::ValidatorBlockCandidateCallback;
/// Validator block candidate decision callback
pub use consensus_common::ValidatorBlockCandidateDecisionCallback;
/// Validator block candidate pointer
pub use consensus_common::ValidatorBlockCandidatePtr;
/// Validator block identifier
pub use consensus_common::ValidatorBlockId;
/// Validator weight type
pub use consensus_common::ValidatorWeight;
use std::{
    sync::{Arc, Weak},
    time::Duration,
};
use ton_block::{fail, BlockIdExt, Result, ShardIdent};

/*
    Shared Raw Vote Data (memory-efficient storage)
*/

/// Shared raw vote data for memory-efficient storage.
///
/// Wraps `Arc<RawBuffer>` to allow sharing serialized vote bytes across
/// multiple data structures (e.g., `ValidatorVotes` storage and `MisbehaviorProof`).
///
/// The underlying bytes are TL-serialized `consensus.simplex.vote` objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawVoteData(Arc<RawBuffer>);

impl RawVoteData {
    /// Create new RawVoteData from raw bytes
    pub fn new(data: RawBuffer) -> Self {
        Self(Arc::new(data))
    }

    /// Create from Vec<u8>
    pub fn from_vec(data: Vec<u8>) -> Self {
        Self(Arc::new(data.into()))
    }

    /// Get reference to underlying bytes
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }

    /// Get reference to underlying RawBuffer
    pub fn as_raw_buffer(&self) -> &RawBuffer {
        &self.0
    }

    /// Get length of underlying data
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Clone the underlying RawBuffer (for when ownership is needed)
    pub fn to_raw_buffer(&self) -> RawBuffer {
        (*self.0).clone()
    }

    /// Get Arc reference count (useful for debugging)
    pub fn ref_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}

impl Default for RawVoteData {
    fn default() -> Self {
        Self(Arc::new(RawBuffer::default()))
    }
}

impl From<RawBuffer> for RawVoteData {
    fn from(data: RawBuffer) -> Self {
        Self::new(data)
    }
}

// Note: From<Vec<u8>> is not implemented separately because RawBuffer is Vec<u8>
// (ton_api::ton::bytes). Use From<RawBuffer> or from_vec() method instead.

impl AsRef<[u8]> for RawVoteData {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/*
    TL types for simplex consensus
*/

/// Module with TL types for simplex consensus
pub mod ton {
    pub use ton_api::ton::consensus::{simplex::*, *};
}

/*
    Simplex Roundless Mode Constants
*/

/// Sentinel value indicating Simplex roundless mode.
///
/// When Simplex uses this value for the `round` field in callbacks (`on_candidate`,
/// `on_generate_slot`, `on_block_finalized`), it signals to `ValidatorGroup` that
/// round-based invariants should be bypassed.
///
/// # Rationale
///
/// Simplex is slot-native and doesn't use validator-session's sequential round model.
/// Instead of trying to map slots to rounds (which was fragile and required gap-fill
/// logic), Simplex now passes this sentinel to indicate "roundless" semantics.
///
/// # ValidatorGroup Behavior
///
/// When `ValidatorGroup` receives callbacks with `round == SIMPLEX_ROUNDLESS`:
/// - `finish_round()`: skips `expected_current_round` validation and advancement
/// - `on_generate_slot()`: skips collation round check (`expected_collation_round`)
/// - `on_candidate()`: skips validation round check (`expected_current_round`)
///
/// # Usage
///
/// Simplex sets `source_info.priority.round = SIMPLEX_ROUNDLESS` in all callbacks.
/// `on_block_skipped()` should never be called with `SIMPLEX_ROUNDLESS` — use
/// `unreachable!()` to assert this.
pub const SIMPLEX_ROUNDLESS: u32 = u32::MAX;

/*
    Simplex-specific types
*/

/// Pointer to Simplex Session
pub type SessionPtr = Arc<dyn SimplexSession + Send + Sync>;

/// Pointer to SessionListener
pub type SessionListenerPtr = Weak<dyn SessionListener + Send + Sync>;

/// Log replay listener pointer
pub type SessionReplayListenerPtr = consensus_common::ConsensusReplayListenerPtr;

/*
    SessionOptions for Simplex consensus
*/

/// Simplex session options
#[derive(Clone, Copy, Debug)]
pub struct SessionOptions {
    /// Protocol version
    pub proto_version: u32,

    /// Number of consecutive slots per leader window
    /// Default: 1 (must be >= 1)
    pub slots_per_leader_window: u32,

    /// Target time between blocks
    /// Default: 1 second
    pub target_rate: Duration,

    /// Minimum interval between a parent block's exact generation time and
    /// validation / generation of the next non-empty block.
    pub min_block_interval: Duration,

    /// Timeout for first block in window
    /// Default: 3 seconds
    pub first_block_timeout: Duration,

    /// Maximum block size
    pub max_block_size: usize,

    /// Maximum collated data size
    pub max_collated_data_size: usize,

    /// Use callback thread for session callbacks
    pub use_callback_thread: bool,

    /// Validation retry attempts (0 = no retries)
    pub validation_retry_attempts: u32,

    /// Timeout between validation retry attempts
    pub validation_retry_timeout: Duration,

    /// Collation retry timeout
    pub collation_retry_timeout: Duration,

    /// Collation retry max attempts
    pub collation_retry_max_attempts: u32,

    /// Standstill timeout - if no finalization occurs within this period,
    /// re-broadcast all our votes for tracked slots
    /// Default: 10 seconds (matches C++ standstill_timeout_s)
    pub standstill_timeout: Duration,

    /// Empty block MC lag threshold for shard sessions
    ///
    /// For shard chains: generate empty block when `mc_finalized_seqno + threshold < new_seqno`.
    /// Default: None - do not generate empty blocks
    ///
    /// MUST be None for masterchain sessions (MC uses internal finalization tracking).
    /// Validation will fail if this is Some(_) for masterchain sessions.
    pub empty_block_mc_lag_threshold: Option<u32>,

    /// Wait for full database initialization before returning from create_session()
    ///
    /// If true: create_session() blocks until DB is opened, bootstrap is loaded,
    /// receiver and processor are created (full initialization).
    ///
    /// If false: create_session() returns immediately after the main thread starts,
    /// DB initialization happens asynchronously (non-blocking).
    ///
    /// Default: false (non-blocking)
    pub wait_for_db_init: bool,

    /// Use QUIC overlay transport instead of ADNL UDP for this session.
    /// When true, overlay messages/queries are sent via QUIC streams.
    /// Default: false
    pub use_quic: bool,

    /// `SimplexConfig.enable_observers` (ConfigParam 30).
    /// Default: false (use consensus private overlay for candidate propagation).
    pub enable_observers: bool,

    /// Cooldown between repeated health alerts of the same anomaly type.
    /// Default: 30 seconds
    pub health_alert_cooldown: Duration,

    /// Finalization stall warning threshold (seconds without progress).
    /// Default: 15s (warn), 60s (error)
    pub health_stall_warning_secs: u64,
    pub health_stall_error_secs: u64,

    // -- Noncritical params (from simplex_config_v2 HashmapE) --
    //
    // Most of these fields are deserialized from on-chain config and passed through.
    // `min_block_interval` is consumed directly by the Rust session timing logic.

    // Wired into SimplexState adaptive timeout backoff.
    // C++ consensus.cpp parity: multiplier+cap for first_block_timeout_ only.
    pub first_block_timeout_multiplier: f64,
    pub first_block_timeout_cap: Duration,

    // Wired into Receiver candidate resolver (requestCandidate):
    // C++ candidate-resolver.cpp parity for per-request timeout/backoff/cooldown.
    pub candidate_resolve_timeout: Duration,
    pub candidate_resolve_timeout_multiplier: f64,
    pub candidate_resolve_timeout_cap: Duration,
    pub candidate_resolve_cooldown: Duration,

    // Wired into Receiver standstill replay token-bucket shaping.
    // C++ parity: pool.cpp standstill_resolution_task byte budget.
    pub standstill_max_egress_bytes_per_s: u32,

    // Wired into slot/vote acceptance bounds in SimplexState + Receiver.
    // C++ parity: consensus.cpp/pool.cpp future-window rejection.
    pub max_leader_window_desync: u32,

    // Wired into Receiver temporary peer-ban logic. C++ pool.cpp bans peers with
    // bad vote/cert signatures for this duration via `bad_signature_bans_`.
    pub bad_signature_ban_duration: Duration,

    // Wired into Receiver inbound requestCandidate rate limiting:
    // C++ candidate-resolver.cpp parity (1-second sliding window per peer).
    pub candidate_resolve_rate_limit: u32,

    // TODO: wire into empty-block error backoff. C++ block-producer.cpp suppresses
    // empty blocks for this period after a failed normal collation.
    pub no_empty_blocks_on_error_timeout: Duration,

    /// Label set attached to per-session metrics that are republished to the
    /// global Prometheus recorder. Multiple parallel Simplex sessions
    /// disambiguate themselves via labels; the cardinality vs. correlation
    /// trade-off is selected here.
    pub prometheus_labels: PrometheusLabels,
}

/// Strategy for labelling per-session Simplex metrics published to Prometheus.
///
/// Many Simplex sessions can run in parallel (one per shard, rotating per
/// validator-set epoch). The metrics layer republishes each session's local
/// dump to the global Prometheus recorder; this enum decides which labels
/// disambiguate the resulting time series.
///
/// Cardinality grows with the number of distinct label tuples observed by
/// Prometheus, so prefer the lowest-cardinality option that still answers the
/// monitoring question at hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrometheusLabels {
    /// Only the shard identifier (e.g. `0:8000000000000000`).
    ///
    /// Lowest cardinality; cannot tell two consecutive sessions of the same
    /// shard apart. Default.
    #[default]
    ShardOnly,
    /// Shard identifier plus first 8 hex chars of the session id
    /// (matches the `sid8` prefix used in log dumps and monitoring reports).
    ///
    /// Per-session breakdown. Cardinality grows by one new series per shard on
    /// every validator-set rotation.
    ShardAndSessionId,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            proto_version: 0,
            slots_per_leader_window: 1,
            target_rate: Duration::from_secs(1),
            min_block_interval: Duration::from_secs(0),
            first_block_timeout: Duration::from_secs(3),
            max_block_size: 4 << 20,         // 4 MB
            max_collated_data_size: 4 << 20, // 4 MB
            use_callback_thread: true,
            validation_retry_attempts: 0,
            validation_retry_timeout: Duration::from_secs(1),
            collation_retry_timeout: Duration::from_millis(500),
            collation_retry_max_attempts: 3,
            standstill_timeout: Duration::from_secs(10),
            empty_block_mc_lag_threshold: None,
            wait_for_db_init: false,
            use_quic: false,
            enable_observers: false,
            health_alert_cooldown: Duration::from_secs(30),
            health_stall_warning_secs: 15,
            health_stall_error_secs: 60,
            first_block_timeout_multiplier: 1.2,
            first_block_timeout_cap: Duration::from_secs(100),
            candidate_resolve_timeout: Duration::from_secs(1),
            candidate_resolve_timeout_multiplier: 1.2,
            candidate_resolve_timeout_cap: Duration::from_secs(10),
            candidate_resolve_cooldown: Duration::from_millis(10),
            standstill_max_egress_bytes_per_s: 50 << 17,
            max_leader_window_desync: 250,
            bad_signature_ban_duration: Duration::from_secs(5),
            candidate_resolve_rate_limit: 10,
            no_empty_blocks_on_error_timeout: Duration::from_secs(15),
            prometheus_labels: PrometheusLabels::default(),
        }
    }
}

impl SessionOptions {
    /// Validate options and return error if invalid
    pub fn validate(&self) -> Result<()> {
        if self.slots_per_leader_window == 0 {
            fail!("slots_per_leader_window must be >= 1")
        }

        if self.target_rate.is_zero() {
            fail!("target_rate must be > 0")
        }

        if self.max_block_size == 0 {
            fail!("max_block_size must be > 0")
        }

        if self.max_collated_data_size == 0 {
            fail!("max_collated_data_size must be > 0")
        }

        if self.first_block_timeout.is_zero() {
            fail!("first_block_timeout must be > 0")
        }

        // Collation flow parameters
        if self.collation_retry_timeout.is_zero() {
            fail!("collation_retry_timeout must be > 0")
        }

        // collation_retry_max_attempts = 0 is valid (no retries)

        if self.health_alert_cooldown.is_zero() {
            fail!("health_alert_cooldown must be > 0")
        }

        if self.health_stall_warning_secs == 0 {
            fail!("health_stall_warning_secs must be > 0")
        }

        if self.health_stall_error_secs < self.health_stall_warning_secs {
            fail!("health_stall_error_secs must be >= health_stall_warning_secs")
        }

        // Noncritical params from on-chain config
        if !self.first_block_timeout_multiplier.is_finite()
            || self.first_block_timeout_multiplier < 1.0
        {
            fail!("first_block_timeout_multiplier must be finite and >= 1.0")
        }

        if self.first_block_timeout_cap.is_zero() {
            fail!("first_block_timeout_cap must be > 0")
        }

        if self.candidate_resolve_timeout.is_zero() {
            fail!("candidate_resolve_timeout must be > 0")
        }

        if !self.candidate_resolve_timeout_multiplier.is_finite()
            || self.candidate_resolve_timeout_multiplier < 1.0
        {
            fail!("candidate_resolve_timeout_multiplier must be finite and >= 1.0")
        }

        if self.candidate_resolve_timeout_cap.is_zero() {
            fail!("candidate_resolve_timeout_cap must be > 0")
        }

        if self.candidate_resolve_cooldown.is_zero() {
            fail!("candidate_resolve_cooldown must be > 0")
        }

        if self.bad_signature_ban_duration.is_zero() {
            fail!("bad_signature_ban_duration must be > 0")
        }

        Ok(())
    }

    /// Validate options for a specific shard
    ///
    /// Additional validation that requires knowledge of the shard type.
    /// Called during session creation.
    pub fn validate_for_shard(&self, shard: &ShardIdent) -> Result<()> {
        // First run basic validation
        self.validate()?;

        // For masterchain sessions, empty_block_mc_lag_threshold must be None
        // (MC uses internal finalization tracking, not MC lag)
        if shard.is_masterchain() && self.empty_block_mc_lag_threshold.is_some() {
            fail!(
                "empty_block_mc_lag_threshold must be None for masterchain sessions (got {:?})",
                self.empty_block_mc_lag_threshold
            )
        }

        Ok(())
    }
}

/*
    SimplexSession trait (Simplex-specific operations)
*/

/// Simplex-specific session operations
///
/// This trait extends `Session` with simplex-specific functionality.
/// It is kept separate from the base `Session` trait for compatibility
/// with the existing validator-session crate.
///
/// # MC Finalization Notification
///
/// For **shard chains**, empty block generation depends on the masterchain-applied
/// top for that shard. When `last_mc_finalized_seqno + 8 < new_seqno`, the session
/// generates empty blocks instead of normal blocks.
///
/// The higher layer (ValidatorManager) should call `notify_mc_finalized()`
/// when masterchain state is updated, passing the current applied top for
/// each simplex session shard.
///
/// # Example
///
/// ```ignore
/// // When MC state is updated, notify simplex sessions with their applied top.
/// simplex_session.notify_mc_finalized(applied_top_for_session_shard);
/// ```
pub trait SimplexSession: ConsensusSession {
    /// Notify session about the current applied top for its shard
    ///
    /// # Purpose
    ///
    /// This updates session-local applied-top tracking:
    /// - shard sessions use it for empty-block recovery against MC-registered tops
    /// - masterchain sessions use it to mirror the applied MC head for validation ordering
    ///   and C++-parity producer-side finalization tracking
    ///
    /// In C++ parity mode this mirrors `BlockFinalizedInMasterchain` handling: the
    /// session updates the applied/accepted head and the producer-side finalized seqno
    /// cursor used by empty-block generation.
    ///
    /// # When to Call
    ///
    /// When masterchain state is updated, ValidatorManager should call this for all
    /// simplex validator sessions with the current applied top for that session shard.
    ///
    /// # Arguments
    ///
    /// * `applied_top` - Current applied top for this session shard
    fn notify_mc_finalized(&self, applied_top: BlockIdExt);

    /// Request that the candidate body (and optionally its parent chain) be
    /// made available for a given `BlockIdExt`.
    ///
    /// Called by the validator layer when collation or validation needs a
    /// state whose parent is notarized but not yet applied by the engine.
    /// Simplex resolves `BlockIdExt` to its internal `RawCandidateId` and
    /// initiates `requestCandidate` repair if needed.
    ///
    /// C++ counterpart: state-resolver demand path in `BlockProducerImpl`.
    fn ensure_candidate_available(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    );

    /// Check if the session has fully stopped (all threads have terminated).
    ///
    /// # Usage
    ///
    /// After calling `stop_async()`, poll this method to determine when it's safe
    /// to recreate the session with the same DB path (for restart-gremlin testing).
    ///
    /// # Returns
    ///
    /// * `true` if all internal threads have stopped
    /// * `false` if the session is still running or in the process of stopping
    fn is_stopped(&self) -> bool;

    /// Check if any internal Simplex thread panicked.
    ///
    /// This is set to true if at least one of the following threads panicked:
    /// - `SXMAIN` (session main loop)
    /// - `SXCB` (callbacks loop, if enabled)
    /// - `SXRCV` (receiver thread)
    ///
    fn is_panicked(&self) -> bool;
}

/*
    SessionFactory
*/

/// Factory for creating Simplex sessions and related objects
pub struct SessionFactory;

impl SessionFactory {
    /// Create ADNL overlay manager for consensus
    pub fn create_adnl_overlay_manager(
        runtime_handle: tokio::runtime::Handle,
        stack: Arc<adnl::NetworkStack>,
        broadcast_hops: Option<u8>,
        track_private_peers: bool,
    ) -> Result<ConsensusOverlayManagerPtr> {
        consensus_common::ConsensusCommonFactory::create_adnl_overlay_manager(
            runtime_handle,
            stack,
            broadcast_hops,
            track_private_peers,
        )
    }

    /// Create in-process overlay manager for testing
    pub fn create_in_process_overlay_manager(num_threads: usize) -> ConsensusOverlayManagerPtr {
        consensus_common::ConsensusCommonFactory::create_in_process_overlay_manager(num_threads)
    }

    /// Create simplex session
    ///
    /// Returns a `SessionPtr` which provides access to both base `Session` operations
    /// and simplex-specific methods like `notify_mc_finalized()`.
    ///
    /// # Arguments
    /// * `options` - Session configuration options
    /// * `session_id` - Unique session identifier
    /// * `shard` - Shard identifier for this session
    /// * `ids` - List of validator nodes
    /// * `local_key` - Private key for signing
    /// * `db_path` - Full database path
    /// * `overlay_manager` - Network overlay manager
    /// * `listener` - Session event listener
    ///
    /// After creation, call
    /// `Session::start(prev_blocks, min_masterchain_block_id)` to begin
    /// consensus processing. `prev_blocks` is the parent chain the new
    /// session will build on (one entry for normal flow, two for a shard
    /// merge); the session derives `initial_block_seqno` as
    /// `max(prev_blocks[].seq_no) + 1` (the merge case picks the higher
    /// of the two parents). `min_masterchain_block_id` is the masterchain
    /// block ID that anchors external-block bounds (same MC block for an
    /// MC session).
    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        options: &SessionOptions,
        session_id: &SessionId,
        shard: &ShardIdent,
        ids: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_path: String,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
    ) -> Result<SessionPtr> {
        session::SessionImpl::create(
            options,
            session_id,
            shard,
            ids,
            local_key,
            db_path,
            overlay_manager,
            listener,
        )
    }

    /// Create session with log replay
    pub fn create_session_replay(
        options: &SessionOptions,
        log_replay_options: &LogReplayOptions,
        session_listener: SessionListenerPtr,
        replay_listener: SessionReplayListenerPtr,
    ) -> Result<SessionPtr> {
        session::SessionImpl::create_replay(
            options,
            log_replay_options,
            session_listener,
            replay_listener,
        )
    }
}
