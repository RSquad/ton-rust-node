/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Centralized consensus type imports for the validator module.
//!
//! All files in `node/src/validator/` should import consensus-related types
//! from this module rather than directly from `catchain`, `validator_session`,
//! `consensus_common`, or `simplex` crates.
//!
//! This provides:
//! - Single point of change when switching consensus implementations
//! - Clear dependency boundary between validator and consensus layers
//! - Easier future integration of simplex consensus
//!
//! ## Design Principle
//!
//! Types are imported from `consensus_common` whenever possible.
//! Implementation-specific types are prefixed with `Catchain` or `Simplex`.

// Allow unused imports - this is a re-export module and not all types are used yet
#![allow(unused_imports)]
#![allow(dead_code)]

use std::{
    any::Any,
    fmt::{Debug, Display, Formatter},
    sync::Arc,
    time::Duration,
};
use ton_block::{BlockIdExt, ShardIdent};

// =============================================================================
// Consensus Timing Constants (for accelerated consensus mode only)
// =============================================================================

// Catchain session timing - ONLY for accelerated consensus mode.
// These values are set via set_catchain_max_block_delay only when
// accelerated_consensus_enabled=true. In default mode, catchain uses
// its internal default timing values.
pub(super) const ACCELERATED_CONSENSUS_MC_MAX_BLOCK_DELAY_MS: u64 = 5;
pub(super) const ACCELERATED_CONSENSUS_SHARD_MAX_BLOCK_DELAY_MS: u64 = 5;
pub(super) const ACCELERATED_CONSENSUS_MAX_BLOCK_DELAY_SLOW_MS: u64 = 1000;

// Additional accelerated consensus specific settings
pub(super) const ACCELERATED_CONSENSUS_CATCHAIN_IDLE_TIMEOUT_MS: u64 = 100;
pub(super) const ACCELERATED_CONSENSUS_NEIGHBOURS_SYNC_MIN_PERIOD_MS: u64 = 1000;
pub(super) const ACCELERATED_CONSENSUS_NEIGHBOURS_SYNC_MAX_PERIOD_MS: u64 = 2000;
pub(super) const ACCELERATED_CONSENSUS_MC_SKIP_CANDIDATE_DELAY_MS: u64 = 12000;
pub(super) const ACCELERATED_CONSENSUS_SHARD_SKIP_CANDIDATE_DELAY_MS: u64 = 6000;
pub(super) const ACCELERATED_CONSENSUS_MC_SKIP_ROUNDS_COUNT_FOR_COLLATOR_ROTATION: u32 = 5;
pub(super) const ACCELERATED_CONSENSUS_SHARD_SKIP_ROUNDS_COUNT_FOR_COLLATOR_ROTATION: u32 = 5;
pub(super) const ACCELERATED_CONSENSUS_VALIDATION_RETRY_ATTEMPTS: u32 = 8;
pub(super) const ACCELERATED_CONSENSUS_VALIDATION_RETRY_TIMEOUT_MS: u64 = 500;
pub(super) const ACCELERATED_CONSENSUS_BLOCK_CANDIDATE_SENDING_RETRY_TIMEOUT_MS: u64 = 2000;
pub(super) const ACCELERATED_CONSENSUS_BLOCK_CANDIDATE_SENDING_RETRY_ATTEMPTS: u32 = 3;

// =============================================================================
// Common Types from consensus-common (preferred source)
// =============================================================================

pub use consensus_common::{
    serialize_tl_bare_object, serialize_tl_boxed_object,
    utils::{get_elapsed_time, get_hash, get_hash_from_block_payload},
    AsyncRequest, AsyncRequestPtr, BlockCandidatePriority, BlockHash, BlockPayloadPtr,
    BlockSignature, BlockSourceInfo, CandidateObservedFlags, CollationParentHint,
    ConsensusCommonFactory, ConsensusNode, ConsensusOverlay, ConsensusOverlayListener,
    ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListener,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager, ConsensusOverlayManagerPtr,
    ConsensusOverlayPtr, ConsensusReplayListener, ConsensusReplayListenerPtr,
    EmulatorLeaderRotation, EmulatorOptions, EmulatorPtr, EmulatorSignerSubset,
    EnsureCandidateAvailabilityOptions, LogPlayer, LogPlayerPtr, LogReplayOptions,
    OverlayTransportType, PrivateKey, PublicKey, PublicKeyHash, RawBuffer, ResolverPurpose, Result,
    Session, SessionId, SessionListener, SessionListenerPtr, SessionNode, SessionPtr, SessionStats,
    ValidatorBlockCandidate, ValidatorBlockCandidateCallback,
    ValidatorBlockCandidateDecisionCallback, ValidatorBlockCandidatePtr, ValidatorWeight,
};

// =============================================================================
// Catchain-specific types (catchain-based consensus implementation)
// =============================================================================

/// Catchain session pointer type
pub type CatchainSessionPtr = validator_session::SessionPtr;

/// Catchain session options
pub type CatchainSessionOptions = validator_session::SessionOptions;

/// Catchain session statistics
pub type CatchainSessionStats = validator_session::ValidatorSessionStats;

// =============================================================================
// Simplex-specific types (simplex consensus implementation) - Phase 2
// =============================================================================
pub use simplex::SimplexSession;
/// Catchain-specific session trait (extends consensus_common::Session)
pub use validator_session::Session as CatchainSession;

/// Simplex session pointer type
pub type SimplexSessionPtr = simplex::SessionPtr;

/// Simplex session options
pub type SimplexSessionOptions = simplex::SessionOptions;

/// Test-only options for replacing a real Simplex session with the in-process
/// consensus emulator while keeping ValidatorGroup in Simplex mode.
#[cfg(test)]
#[derive(Clone)]
pub struct EmulatorConsensusOptions {
    /// Simplex options consumed by ValidatorGroup for Simplex-specific
    /// behaviour such as in-window collation context limits.
    pub simplex_options: SimplexSessionOptions,
    /// Emulator timing/leader/signing configuration. The session id and shard
    /// are overwritten from the ValidatorGroup at creation time.
    pub emulator_options: EmulatorOptions,
    /// Private-key-bearing validator keys owned by the single test process.
    pub validator_keys: Vec<PrivateKey>,
}

#[cfg(test)]
impl EmulatorConsensusOptions {
    pub fn fixed_local(validator_keys: Vec<PrivateKey>) -> Self {
        let mut emulator_options = EmulatorOptions::default();
        emulator_options.leader_rotation = EmulatorLeaderRotation::FixedLocal;
        emulator_options.signer_subset = EmulatorSignerSubset::All;
        Self { simplex_options: SimplexSessionOptions::default(), emulator_options, validator_keys }
    }
}

#[cfg(test)]
struct EmulatorSimplexSessionAdapter {
    emulator: EmulatorPtr,
}

#[cfg(test)]
impl Display for EmulatorSimplexSessionAdapter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.emulator.as_ref(), f)
    }
}

#[cfg(test)]
impl Session for EmulatorSimplexSessionAdapter {
    fn start(&self, prev_blocks: Vec<BlockIdExt>, min_masterchain_block_id: BlockIdExt) {
        self.emulator.start(prev_blocks, min_masterchain_block_id);
    }

    fn stop(&self) {
        self.emulator.stop();
    }

    fn stop_async(&self) {
        self.emulator.stop_async();
    }

    fn destroy(&self) {
        self.emulator.destroy();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
impl SimplexSession for EmulatorSimplexSessionAdapter {
    fn notify_mc_finalized(&self, applied_top: BlockIdExt) {
        self.emulator.notify_mc_finalized(applied_top);
    }

    fn ensure_candidate_available(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        self.emulator.ensure_candidate_available(block_id, opts);
    }

    fn is_stopped(&self) -> bool {
        self.emulator.is_stopped()
    }

    fn is_panicked(&self) -> bool {
        self.emulator.is_panicked()
    }
}

// =============================================================================
// Consensus Options - Unified options enum for session creation
// =============================================================================

/// Unified consensus options enum.
///
/// This enum determines both the consensus type AND provides the
/// appropriate options for that consensus implementation.
/// Pass this to `ValidatorGroup::new()` instead of separate options + type.
#[derive(Clone)]
pub enum ConsensusOptions {
    /// Catchain-based validator session options
    Catchain(CatchainSessionOptions),
    /// Simplex-based consensus options
    Simplex(SimplexSessionOptions),
    /// Test-only Simplex-compatible session backed by ConsensusEmulator.
    #[cfg(test)]
    Emulator(EmulatorConsensusOptions),
}

impl ConsensusOptions {
    /// Get the consensus type from the options variant
    pub fn consensus_type(&self) -> ConsensusType {
        match self {
            ConsensusOptions::Catchain(_) => ConsensusType::Catchain,
            ConsensusOptions::Simplex(_) => ConsensusType::Simplex,
            #[cfg(test)]
            ConsensusOptions::Emulator(_) => ConsensusType::Simplex,
        }
    }

    /// Get catchain options if this is a catchain configuration
    pub fn as_catchain(&self) -> Option<&CatchainSessionOptions> {
        match self {
            ConsensusOptions::Catchain(opts) => Some(opts),
            ConsensusOptions::Simplex(_) => None,
            #[cfg(test)]
            ConsensusOptions::Emulator(_) => None,
        }
    }

    /// Get simplex options if this is a simplex configuration
    pub fn as_simplex(&self) -> Option<&SimplexSessionOptions> {
        match self {
            ConsensusOptions::Catchain(_) => None,
            ConsensusOptions::Simplex(opts) => Some(opts),
            #[cfg(test)]
            ConsensusOptions::Emulator(opts) => Some(&opts.simplex_options),
        }
    }

    /// Check if accelerated consensus is enabled (catchain-specific)
    pub fn is_accelerated_consensus_enabled(&self) -> bool {
        match self {
            ConsensusOptions::Catchain(opts) => opts.accelerated_consensus_enabled,
            ConsensusOptions::Simplex(_) => false,
            #[cfg(test)]
            ConsensusOptions::Emulator(_) => false,
        }
    }

    /// Check if pipeline context updates are enabled.
    ///
    /// Pipeline context keeps recently collated block states so that subsequent
    /// collations can chain on top of them (precollation). It is used only
    /// for accelerated Catchain consensus.
    pub fn is_pipeline_context_enabled(&self) -> bool {
        match self {
            ConsensusOptions::Catchain(opts) => opts.accelerated_consensus_enabled,
            ConsensusOptions::Simplex(_) => false,
            #[cfg(test)]
            ConsensusOptions::Emulator(_) => false,
        }
    }
}

impl Default for ConsensusOptions {
    fn default() -> Self {
        ConsensusOptions::Catchain(CatchainSessionOptions::default())
    }
}

impl Debug for ConsensusOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsensusOptions::Catchain(opts) => {
                write!(f, "ConsensusOptions::Catchain({:?})", opts)
            }
            ConsensusOptions::Simplex(_) => write!(f, "ConsensusOptions::Simplex(...)"),
            #[cfg(test)]
            ConsensusOptions::Emulator(_) => write!(f, "ConsensusOptions::Emulator(...)"),
        }
    }
}

// =============================================================================
// Session Holder - Unified session storage implementing consensus_common::Session
// =============================================================================

/// Inner enum holding the specific session type
///
/// This enum distinguishes between different consensus implementations
/// while providing access to implementation-specific session pointers.
pub enum SessionInner {
    /// Catchain-based validator session
    Catchain(CatchainSessionPtr),
    /// Simplex-based consensus session
    Simplex(SimplexSessionPtr),
    /// Test-only Simplex-compatible emulator session
    #[cfg(test)]
    Emulator(EmulatorPtr),
}

impl SessionInner {
    /// Get session as common Session trait reference
    fn as_common_session(&self) -> &dyn consensus_common::Session {
        match self {
            SessionInner::Catchain(s) => s.as_ref() as &dyn consensus_common::Session,
            SessionInner::Simplex(s) => s.as_ref() as &dyn consensus_common::Session,
            #[cfg(test)]
            SessionInner::Emulator(s) => s.as_ref() as &dyn consensus_common::Session,
        }
    }

    /// Get the consensus type
    fn get_consensus_type(&self) -> ConsensusType {
        match self {
            SessionInner::Catchain(_) => ConsensusType::Catchain,
            SessionInner::Simplex(_) => ConsensusType::Simplex,
            #[cfg(test)]
            SessionInner::Emulator(_) => ConsensusType::Simplex,
        }
    }
}

/// Holds a consensus session of any type, implementing the common Session interface.
///
/// This allows storing any consensus session type as a single `SessionHolderPtr` while
/// providing direct access to `SessionHolder` methods without downcasting.
pub struct SessionHolder {
    /// The underlying session (enum for type-specific access)
    inner: SessionInner,
}

impl SessionHolder {
    /// Create a new catchain session holder
    pub fn catchain(session: CatchainSessionPtr) -> Self {
        SessionHolder { inner: SessionInner::Catchain(session) }
    }

    /// Create a new simplex session holder
    pub fn simplex(session: SimplexSessionPtr) -> Self {
        SessionHolder { inner: SessionInner::Simplex(session) }
    }

    /// Create a new test-only emulator session holder.
    #[cfg(test)]
    pub fn emulator(session: EmulatorPtr) -> Self {
        SessionHolder { inner: SessionInner::Emulator(session) }
    }

    /// Get the consensus type
    pub fn get_consensus_type(&self) -> ConsensusType {
        self.inner.get_consensus_type()
    }

    /// Get catchain session pointer for catchain-specific operations
    /// Returns None if this is not a catchain session
    pub fn get_catchain_session(&self) -> Option<CatchainSessionPtr> {
        match &self.inner {
            SessionInner::Catchain(s) => Some(s.clone()),
            SessionInner::Simplex(_) => None,
            #[cfg(test)]
            SessionInner::Emulator(_) => None,
        }
    }

    /// Get simplex session pointer for simplex-specific operations
    /// Returns None if this is not a simplex session
    pub fn get_simplex_session(&self) -> Option<SimplexSessionPtr> {
        match &self.inner {
            SessionInner::Catchain(_) => None,
            SessionInner::Simplex(s) => Some(s.clone()),
            #[cfg(test)]
            SessionInner::Emulator(s) => {
                Some(Arc::new(EmulatorSimplexSessionAdapter { emulator: s.clone() }))
            }
        }
    }

    /// Get emulator session pointer for test-only controls.
    #[cfg(test)]
    pub fn get_emulator_session(&self) -> Option<EmulatorPtr> {
        match &self.inner {
            SessionInner::Emulator(s) => Some(s.clone()),
            SessionInner::Catchain(_) | SessionInner::Simplex(_) => None,
        }
    }

    /// Notify session about the current applied top for its shard.
    ///
    /// For simplex sessions, this updates the session-local applied-top tracking used for
    /// empty-block recovery and MC validation ordering.
    ///
    /// For catchain sessions, this is a no-op as they don't need MC finalization tracking.
    ///
    /// # Arguments
    /// * `applied_top` - Current applied top for this session shard
    pub fn notify_mc_finalized(&self, applied_top: BlockIdExt) {
        match &self.inner {
            SessionInner::Simplex(s) => s.notify_mc_finalized(applied_top),
            #[cfg(test)]
            SessionInner::Emulator(s) => s.notify_mc_finalized(applied_top),
            SessionInner::Catchain(_) => {}
        }
    }
}

// Implement consensus_common::Session for SessionHolder
// Delegates to the common Session interface of the inner session
impl consensus_common::Session for SessionHolder {
    fn start(&self, prev_blocks: Vec<BlockIdExt>, min_masterchain_block_id: BlockIdExt) {
        self.inner.as_common_session().start(prev_blocks, min_masterchain_block_id);
    }

    fn stop(&self) {
        self.inner.as_common_session().stop();
    }

    fn stop_async(&self) {
        self.inner.as_common_session().stop_async();
    }

    fn destroy(&self) {
        self.inner.as_common_session().destroy();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Display for SessionHolder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            SessionInner::Catchain(s) => write!(f, "{}", s),
            SessionInner::Simplex(s) => write!(f, "{}", s),
            #[cfg(test)]
            SessionInner::Emulator(s) => write!(f, "{}", s),
        }
    }
}

/// Pointer to a SessionHolder
///
/// Use this type instead of `SessionPtr` when you need direct access to
/// `SessionHolder` methods like `notify_mc_finalized()` without downcasting.
pub type SessionHolderPtr = Arc<SessionHolder>;

// =============================================================================
// Consensus Factory
// =============================================================================

/// Unified factory for creating consensus sessions.
///
/// This factory delegates to the appropriate implementation-specific factory
/// (CatchainFactory, validator_session::SessionFactory, or simplex::SessionFactory)
/// based on the consensus type.
pub struct ConsensusFactory;

impl ConsensusFactory {
    // -------------------------------------------------------------------------
    // Catchain-based session creation
    // -------------------------------------------------------------------------

    /// Create a catchain-based consensus session
    ///
    /// This is the primary method for creating catchain sessions. It handles all
    /// catchain-specific configuration including accelerated consensus settings.
    ///
    /// Returns a `SessionHolderPtr` for direct access to `SessionHolder` methods.
    #[allow(clippy::too_many_arguments)]
    pub fn create_catchain_based_session(
        base_options: &CatchainSessionOptions,
        session_id: &SessionId,
        nodes: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_root: String,
        catchain_seqno: u32,
        allow_unsafe_self_blocks_resync: bool,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
        is_masterchain: bool,
    ) -> consensus_common::Result<SessionHolderPtr> {
        let mut options =
            Self::configure_catchain_options(base_options.clone(), nodes.len(), is_masterchain);
        // Disable callback thread - ValidatorSessionListener has its own
        options.use_callback_thread = false;

        let db_suffix = format!("_{}", catchain_seqno);

        let catchain_session = Self::create_catchain_session(
            &options,
            session_id,
            nodes,
            local_key,
            db_root,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            overlay_manager,
            listener,
        )?;

        // Configure catchain session timing ONLY for accelerated consensus mode.
        // In default mode, catchain uses its internal default timing.
        if options.accelerated_consensus_enabled {
            let max_block_delay = if is_masterchain {
                Duration::from_millis(ACCELERATED_CONSENSUS_MC_MAX_BLOCK_DELAY_MS)
            } else {
                Duration::from_millis(ACCELERATED_CONSENSUS_SHARD_MAX_BLOCK_DELAY_MS)
            };
            let max_block_delay_slow =
                Duration::from_millis(ACCELERATED_CONSENSUS_MAX_BLOCK_DELAY_SLOW_MS);
            catchain_session.set_catchain_max_block_delay(max_block_delay, max_block_delay_slow);
        }

        // Wrap in SessionHolder and return as SessionHolderPtr
        Ok(Arc::new(SessionHolder::catchain(catchain_session)))
    }

    // -------------------------------------------------------------------------
    // Simplex-based session creation
    // -------------------------------------------------------------------------

    /// Construct database path for simplex session.
    ///
    /// Path format matches C++ `bridge.cpp`:
    /// `{db_root}/consensus/consensus.{workchain}.{shard_hex}.{cc_seqno}.{session_id_hex}/`
    ///
    /// # Arguments
    ///
    /// * `db_root` - Base database root directory
    /// * `shard` - Shard identifier
    /// * `catchain_seqno` - Catchain sequence number
    /// * `session_id` - Session identifier
    pub fn make_simplex_db_path(
        db_root: &str,
        shard: &ShardIdent,
        catchain_seqno: u32,
        session_id: &SessionId,
    ) -> String {
        let db_dir_name = format!(
            "consensus.{}.{:016x}.{}.{}",
            shard.workchain_id(),
            shard.shard_prefix_with_tag(),
            catchain_seqno,
            session_id.to_hex_string()
        );
        format!("{}/consensus/{}", db_root, db_dir_name)
    }

    /// Create a simplex-based consensus session
    ///
    /// This is the primary method for creating simplex sessions.
    ///
    /// Returns a `SessionHolderPtr` for direct access to `SessionHolder` methods.
    #[allow(clippy::too_many_arguments)]
    pub fn create_simplex_based_session(
        options: &SimplexSessionOptions,
        session_id: &SessionId,
        shard: &ShardIdent,
        nodes: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_root: String,
        catchain_seqno: u32,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
    ) -> consensus_common::Result<SessionHolderPtr> {
        let mut options = options.clone();
        options.use_callback_thread = false;

        let db_path = Self::make_simplex_db_path(&db_root, shard, catchain_seqno, session_id);

        let simplex_session = Self::create_simplex_session(
            &options,
            session_id,
            shard,
            nodes,
            local_key,
            db_path,
            overlay_manager,
            listener,
        )?;

        Ok(Arc::new(SessionHolder::simplex(simplex_session)))
    }

    /// Create a test-only Simplex-compatible session backed by ConsensusEmulator.
    #[cfg(test)]
    pub fn create_emulator_based_session(
        options: &EmulatorConsensusOptions,
        session_id: &SessionId,
        shard: &ShardIdent,
        nodes: Vec<SessionNode>,
        local_key: &PrivateKey,
        listener: SessionListenerPtr,
    ) -> consensus_common::Result<SessionHolderPtr> {
        let local_key_id = local_key.id();
        let local_idx = nodes
            .iter()
            .position(|node| node.public_key.id() == local_key_id)
            .ok_or_else(|| {
                ton_block::error!(
                    "Consensus emulator session: local key {} is absent from validator set",
                    hex::encode(local_key_id.data())
                )
            })?;

        let mut signing_nodes = Vec::with_capacity(nodes.len());
        for node in nodes {
            let node_key_id = node.public_key.id();
            let signing_key = options
                .validator_keys
                .iter()
                .find(|key| key.id() == node_key_id)
                .cloned()
                .ok_or_else(|| {
                    ton_block::error!(
                        "Consensus emulator session: missing signing key for validator {}",
                        hex::encode(node_key_id.data())
                    )
                })?;
            signing_nodes.push(SessionNode {
                adnl_id: node.adnl_id,
                public_key: signing_key,
                weight: node.weight,
            });
        }

        let mut emulator_options = options.emulator_options.clone();
        emulator_options.session_id = session_id.clone();
        emulator_options.shard = shard.clone();
        emulator_options.leader_rotation = EmulatorLeaderRotation::FixedLocal;

        let emulator = ConsensusCommonFactory::create_consensus_emulator(
            emulator_options,
            signing_nodes,
            local_idx,
            listener,
        )?;
        Ok(Arc::new(SessionHolder::emulator(emulator)))
    }

    /// Configure catchain-specific options for accelerated consensus
    pub fn configure_catchain_options(
        mut options: CatchainSessionOptions,
        node_count: usize,
        is_masterchain: bool,
    ) -> CatchainSessionOptions {
        use super::*;

        if !options.accelerated_consensus_enabled {
            return options;
        }

        options.catchain_idle_timeout =
            Duration::from_millis(ACCELERATED_CONSENSUS_CATCHAIN_IDLE_TIMEOUT_MS);
        options.catchain_receiver_neighbours_sync_min_period =
            Duration::from_millis(ACCELERATED_CONSENSUS_NEIGHBOURS_SYNC_MIN_PERIOD_MS);
        options.catchain_receiver_neighbours_sync_max_period =
            Duration::from_millis(ACCELERATED_CONSENSUS_NEIGHBOURS_SYNC_MAX_PERIOD_MS);
        options.catchain_receiver_max_neighbours_count = node_count;
        options.catchain_max_deps = node_count as u32;
        options.round_candidates = 1;
        options.block_candidate_sending_retry_timeout =
            Duration::from_millis(ACCELERATED_CONSENSUS_BLOCK_CANDIDATE_SENDING_RETRY_TIMEOUT_MS);
        options.block_candidate_sending_retry_attempts =
            ACCELERATED_CONSENSUS_BLOCK_CANDIDATE_SENDING_RETRY_ATTEMPTS;
        options.next_candidate_delay = if is_masterchain {
            Duration::from_millis(ACCELERATED_CONSENSUS_MC_SKIP_CANDIDATE_DELAY_MS)
        } else {
            Duration::from_millis(ACCELERATED_CONSENSUS_SHARD_SKIP_CANDIDATE_DELAY_MS)
        };
        options.accelerated_consensus_skip_rounds_count_for_collator_rotation = if is_masterchain {
            ACCELERATED_CONSENSUS_MC_SKIP_ROUNDS_COUNT_FOR_COLLATOR_ROTATION
        } else {
            ACCELERATED_CONSENSUS_SHARD_SKIP_ROUNDS_COUNT_FOR_COLLATOR_ROTATION
        };
        options.validation_retry_attempts = ACCELERATED_CONSENSUS_VALIDATION_RETRY_ATTEMPTS;
        options.validation_retry_timeout =
            Duration::from_millis(ACCELERATED_CONSENSUS_VALIDATION_RETRY_TIMEOUT_MS);

        #[cfg(feature = "xp25")]
        if is_masterchain {
            log::warn!(target: "validator", "Accelerated consensus mode is enabled for masterchain but precollation pipeline is manually disabled!");
            options.accelerated_consensus_max_precollated_blocks = 0;
        }

        options
    }

    // -------------------------------------------------------------------------
    // Catchain-based session creation (delegated to validator_session)
    // -------------------------------------------------------------------------

    /// Create a catchain-based consensus session
    #[allow(clippy::too_many_arguments)]
    pub fn create_catchain_session(
        options: &CatchainSessionOptions,
        session_id: &SessionId,
        nodes: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_path: String,
        db_suffix: String,
        allow_unsafe_self_blocks_resync: bool,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
    ) -> consensus_common::Result<CatchainSessionPtr> {
        validator_session::SessionFactory::create_session(
            options,
            session_id,
            nodes,
            local_key,
            db_path,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            overlay_manager,
            listener,
        )
    }

    /// Create a single-node catchain session (for testing/development)
    pub fn create_single_node_catchain_session(
        options: &CatchainSessionOptions,
        session_id: &SessionId,
        local_key: &PrivateKey,
        db_path: String,
        db_suffix: String,
        listener: SessionListenerPtr,
    ) -> consensus_common::Result<CatchainSessionPtr> {
        validator_session::SessionFactory::create_single_node_session(
            options, session_id, local_key, db_path, db_suffix, listener,
        )
    }

    // -------------------------------------------------------------------------
    // Common object creation (delegated to CatchainFactory)
    // -------------------------------------------------------------------------

    /// Create block payload from raw data
    pub fn create_block_payload(data: RawBuffer) -> BlockPayloadPtr {
        catchain::CatchainFactory::create_block_payload(data)
    }

    /// Create empty block payload
    pub fn create_empty_block_payload() -> BlockPayloadPtr {
        catchain::CatchainFactory::create_empty_block_payload()
    }

    // -------------------------------------------------------------------------
    // Simplex session creation - Phase 2
    // -------------------------------------------------------------------------

    /// Create a simplex consensus session
    #[allow(clippy::too_many_arguments)]
    pub fn create_simplex_session(
        options: &SimplexSessionOptions,
        session_id: &SessionId,
        shard: &ShardIdent,
        nodes: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_path: String,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
    ) -> consensus_common::Result<SimplexSessionPtr> {
        simplex::SessionFactory::create_session(
            options,
            session_id,
            shard,
            nodes,
            local_key,
            db_path,
            overlay_manager,
            listener,
        )
    }
}

// =============================================================================
// Consensus type selector
// =============================================================================

/// Consensus implementation selector.
///
/// Used by ValidatorGroup to determine which consensus implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsensusType {
    /// Old catchain-based validator-session (default)
    #[default]
    Catchain,
    /// Simplex consensus
    Simplex,
}

impl Display for ConsensusType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsensusType::Catchain => write!(f, "catchain"),
            ConsensusType::Simplex => write!(f, "simplex"),
        }
    }
}
