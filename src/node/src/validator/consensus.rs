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
use ton_block::ShardIdent;

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
    BlockSignature, BlockSourceInfo, CollationParentHint, CommittedBlockProof,
    CommittedBlockProofCallback, ConsensusCommonFactory, ConsensusNode, ConsensusOverlay,
    ConsensusOverlayListener, ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListener,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager, ConsensusOverlayManagerPtr,
    ConsensusOverlayPtr, ConsensusReplayListener, ConsensusReplayListenerPtr, LogPlayer,
    LogPlayerPtr, LogReplayOptions, OverlayTransportType, PrivateKey, PublicKey, PublicKeyHash,
    RawBuffer, Result, Session, SessionId, SessionListener, SessionListenerPtr, SessionNode,
    SessionPtr, SessionStats, ValidatorBlockCandidate, ValidatorBlockCandidateCallback,
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
}

impl ConsensusOptions {
    /// Get the consensus type from the options variant
    pub fn consensus_type(&self) -> ConsensusType {
        match self {
            ConsensusOptions::Catchain(_) => ConsensusType::Catchain,
            ConsensusOptions::Simplex(_) => ConsensusType::Simplex,
        }
    }

    /// Get catchain options if this is a catchain configuration
    pub fn as_catchain(&self) -> Option<&CatchainSessionOptions> {
        match self {
            ConsensusOptions::Catchain(opts) => Some(opts),
            ConsensusOptions::Simplex(_) => None,
        }
    }

    /// Get simplex options if this is a simplex configuration
    pub fn as_simplex(&self) -> Option<&SimplexSessionOptions> {
        match self {
            ConsensusOptions::Catchain(_) => None,
            ConsensusOptions::Simplex(opts) => Some(opts),
        }
    }

    /// Check if accelerated consensus is enabled (catchain-specific)
    pub fn is_accelerated_consensus_enabled(&self) -> bool {
        match self {
            ConsensusOptions::Catchain(opts) => opts.accelerated_consensus_enabled,
            ConsensusOptions::Simplex(_) => false,
        }
    }

    /// Check if pipeline context updates are enabled.
    ///
    /// Pipeline context keeps recently collated block states so that subsequent
    /// collations can chain on top of them (precollation).  Simplex always needs
    /// this; for Catchain it mirrors the accelerated-consensus flag.
    pub fn is_pipeline_context_enabled(&self) -> bool {
        match self {
            ConsensusOptions::Catchain(opts) => opts.accelerated_consensus_enabled,
            ConsensusOptions::Simplex(_) => true,
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
}

impl SessionInner {
    /// Get session as common Session trait reference
    fn as_common_session(&self) -> &dyn consensus_common::Session {
        match self {
            SessionInner::Catchain(s) => s.as_ref() as &dyn consensus_common::Session,
            SessionInner::Simplex(s) => s.as_ref() as &dyn consensus_common::Session,
        }
    }

    /// Get the consensus type
    fn get_consensus_type(&self) -> ConsensusType {
        match self {
            SessionInner::Catchain(_) => ConsensusType::Catchain,
            SessionInner::Simplex(_) => ConsensusType::Simplex,
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
        }
    }

    /// Get simplex session pointer for simplex-specific operations
    /// Returns None if this is not a simplex session
    pub fn get_simplex_session(&self) -> Option<SimplexSessionPtr> {
        match &self.inner {
            SessionInner::Catchain(_) => None,
            SessionInner::Simplex(s) => Some(s.clone()),
        }
    }

    /// Notify session about masterchain finalization
    ///
    /// For simplex shard sessions, this updates `last_mc_finalized_seqno` which is
    /// used to decide if an empty block should be generated (finalization recovery).
    ///
    /// For catchain sessions, this is a no-op as they don't need MC finalization tracking.
    ///
    /// # Arguments
    /// * `mc_block_seqno` - The seqno of the finalized masterchain block
    pub fn notify_mc_finalized(&self, mc_block_seqno: u32) {
        if let SessionInner::Simplex(s) = &self.inner {
            s.notify_mc_finalized(mc_block_seqno);
        } else {
            // Catchain sessions don't need MC finalization notification
            let _ = mc_block_seqno; // Suppress unused warning
        }
    }
}

// Implement consensus_common::Session for SessionHolder
// Delegates to the common Session interface of the inner session
impl consensus_common::Session for SessionHolder {
    fn start(&self, initial_block_seqno: u32) {
        self.inner.as_common_session().start(initial_block_seqno);
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
    /// New Alpenglow-based simplex consensus
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
