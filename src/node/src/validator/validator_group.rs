/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//TODO: change async MutexWrapper to sync Mutex/SpinMutex to decrease lock latency

use super::{
    consensus::{
        get_hash, BlockHash, BlockPayloadPtr, CandidateObservedFlags, CollationParentHint,
        ConsensusOptions, ConsensusOverlayManagerPtr, ConsensusType,
        EnsureCandidateAvailabilityOptions, PrivateKey, PublicKey, PublicKeyHash, ResolverPurpose,
        Session, SessionHolderPtr, SessionId, SessionListener, SessionListenerPtr, SessionNode,
        ValidatorBlockCandidate, ValidatorBlockCandidateCallback,
        ValidatorBlockCandidateDecisionCallback,
    },
    fabric::*,
    validator_utils::{GeneralSessionInfo, PrevBlockHistory},
    *,
};
use crate::{
    engine_traits::EngineOperations,
    validator::{
        consensus_overlay::ConsensusOverlayManagerImpl,
        mutex_wrapper::MutexWrapper,
        state_resolver_cache::{ResolverBackend, StateResolverCache},
        validator_utils::{
            prevs_to_string, validator_query_candidate_to_validator_block_candidate,
            validatordescr_to_session_node, ValidatorListHash,
        },
    },
};
use std::{
    collections::VecDeque,
    fmt::{Display, Formatter},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        *,
    },
    time::*,
};
use ton_block::{
    error, fail, read_single_root_boc, Block, BlockIdExt, BlockSignaturesVariant, Deserializable,
    Result, ShardIdent, UInt256, UnixTime, ValidatorSet,
};
use validator_session_listener::{
    process_validation_queue, ValidationAction, ValidatorSessionListener,
};

/// Sentinel value for Simplex roundless mode.
///
/// When round == SIMPLEX_ROUNDLESS, ValidatorGroup bypasses round-based invariants:
/// - `finish_round()`: skips expected_current_round validation and advancement
/// - `on_generate_slot()`: skips collation round check
/// - `on_candidate()`: skips validation round check
///
/// This mirrors the constant in `simplex::SIMPLEX_ROUNDLESS` but is defined locally
/// to avoid feature-gating complexity in ValidatorGroup.
const SIMPLEX_ROUNDLESS: u32 = u32::MAX;

/// Check if round indicates Simplex roundless mode
#[inline]
fn is_simplex_roundless(round: u32) -> bool {
    round == SIMPLEX_ROUNDLESS
}

/// Snapshot of session state for monitoring dumps.
/// Captured atomically from the inner mutex to avoid multiple async calls.
pub struct SessionSnapshot {
    pub session_id: UInt256,
    pub shard: ShardIdent,
    pub cc_seqno: u32,
    pub status: ValidatorGroupStatus,
    pub consensus_type: ConsensusType,
    pub round: u32,
    pub mc_initial_seqno: u32,
    pub has_engine: bool,
    pub is_collator: bool,
    pub created_at: SystemTime,
    pub key_seqno: u32,
    pub last_accepted_mc_seqno: Option<u32>,
}

/// When true, non-accelerated consensus (Catchain / Simplex) will block the
/// validator-group message loop while a validation task runs.
/// Set to false (default) to let those tasks run in the background, keeping
/// the group responsive to incoming accept / collation messages.
const WAIT_FOR_VALIDATION: bool = false;

/// When true, non-accelerated consensus (Catchain / Simplex) will block the
/// validator-group message loop while a collation task runs.
const WAIT_FOR_COLLATION: bool = false;

/// C++ parity: simplex candidate-native validation deadline.
/// Matches `block-validator.cpp`: `validate_block_candidate(..., td::Timestamp::in(60.0))`.
const SIMPLEX_VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);

/// C++ parity: legacy (catchain) validation deadline.
/// Matches `validator-group.cpp`: `run_validate_query(..., td::Timestamp::in(15.0))`.
const LEGACY_VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);

/// C++ parity: collation request deadline.
/// Matches `validator-group.cpp` / `collation-manager.cpp`: `td::Timestamp::in(10.0)`.
const COLLATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Determines if block candidate should be broadcast publicly via FastSync overlay.
/// Mirrors C++ `need_send_candidate_broadcast` logic from validator-group.cpp.
///
/// Broadcast is sent when:
/// - It's the first block round (first_block_round == round)
/// - Priority is 0 (highest priority collator)
/// - It's NOT a masterchain block
fn need_send_candidate_broadcast(
    source_info: &validator_session::BlockSourceInfo,
    is_masterchain: bool,
) -> bool {
    source_info.priority.first_block_round == source_info.priority.round
        && source_info.priority.priority == 0
        && !is_masterchain
}

/// MC fork prevention: returns true if a masterchain candidate should be rejected
/// because it builds on a parent that is behind the last accepted MC block.
///
/// Mirrors C++ `block-validator.cpp` guard (commit 9aac62b8):
/// ```text
/// if (expected_seqno < last_accepted_block_) {
///     co_return td::Status::Error("Candidate builds upon older head");
/// }
/// ```
fn should_reject_stale_mc_candidate(
    last_accepted_block_id: Option<&BlockIdExt>,
    candidate_parent_block_id: &BlockIdExt,
) -> bool {
    matches!(
        last_accepted_block_id,
        Some(accepted_block_id) if candidate_parent_block_id < accepted_block_id
    )
}

fn should_wait_for_mc_validation_parent(
    last_accepted_block_id: Option<&BlockIdExt>,
    candidate_parent_block_id: &BlockIdExt,
) -> bool {
    matches!(
        last_accepted_block_id,
        Some(accepted_block_id) if accepted_block_id < candidate_parent_block_id
    ) || last_accepted_block_id.is_none()
}

fn initial_accepted_mc_head_from_start_inputs(
    shard: &ShardIdent,
    prev: &[BlockIdExt],
    min_masterchain_block_id: &BlockIdExt,
) -> Option<BlockIdExt> {
    if !shard.is_masterchain() {
        return None;
    }
    // C++ parity intent (block-validator Start uses state->as_normal()):
    // prefer exact session prev head when known, otherwise seed from the masterchain
    // start context so MC parent waits do not stall at accepted_head=<none>.
    prev.iter().max().cloned().or_else(|| Some(min_masterchain_block_id.clone()))
}

fn sync_last_accepted_mc_head_from_block(
    group_impl: &mut ValidatorGroupImpl,
    block_id: &BlockIdExt,
) {
    if group_impl.shard.is_masterchain() {
        let prev = group_impl.last_accepted_mc_seqno.unwrap_or(0);
        group_impl.last_accepted_mc_seqno = Some(prev.max(block_id.seq_no));
        match group_impl.last_accepted_mc_block_id.as_ref() {
            Some(current) if current >= block_id => {}
            _ => group_impl.last_accepted_mc_block_id = Some(block_id.clone()),
        }
    }
}

fn sync_last_notified_mc_finalized_seqno(
    group_impl: &mut ValidatorGroupImpl,
    applied_top: &BlockIdExt,
) {
    // C++ parity (`block-accepter.cpp`): keep external MC-finalized cursor monotonic.
    let prev = group_impl.last_notified_mc_finalized_seqno.unwrap_or(0);
    group_impl.last_notified_mc_finalized_seqno = Some(prev.max(applied_top.seq_no));
}

fn should_suppress_stale_finalized_rebroadcast(
    last_notified_mc_finalized_seqno: Option<u32>,
    block_seqno: u32,
) -> bool {
    // C++ parity (`block-accepter.cpp`):
    // if (last_mc_finalized_seqno_ >= 2 && block.id.seqno() < last_mc_finalized_seqno_ - 2) {
    //   broadcast_mode = 0;
    // }
    matches!(
        last_notified_mc_finalized_seqno,
        Some(last_seqno) if last_seqno >= 2 && block_seqno < last_seqno - 2
    )
}

async fn wait_for_mc_validation_parent(
    mut accepted_mc_block_rx: tokio::sync::watch::Receiver<Option<BlockIdExt>>,
    candidate_block_id: &BlockIdExt,
    candidate_parent_block_id: &BlockIdExt,
) -> Result<()> {
    //TODO: LK: add max timeout for parents waiting
    let mut logged_wait = false;
    loop {
        let accepted_block_id = accepted_mc_block_rx.borrow_and_update().clone();
        if should_reject_stale_mc_candidate(accepted_block_id.as_ref(), candidate_parent_block_id) {
            metrics::counter!("simplex_mc_fork_prevention_rejected").increment(1);
            fail!(
                "MC fork prevention: candidate {} builds upon {} \
                 but we already accepted {}",
                candidate_block_id,
                candidate_parent_block_id,
                accepted_block_id.as_ref().expect("stale branch must have accepted head")
            );
        }
        if should_wait_for_mc_validation_parent(
            accepted_block_id.as_ref(),
            candidate_parent_block_id,
        ) {
            if !logged_wait {
                logged_wait = true;
                log::debug!(
                    "MC validation wait started for candidate {} \
                     (parent={}, accepted_head={})",
                    candidate_block_id,
                    candidate_parent_block_id,
                    accepted_block_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<none>".to_string()),
                );
            }

            match accepted_mc_block_rx.changed().await {
                Ok(()) => continue,
                Err(_) => {
                    fail!(
                        "MC validation wait cancelled for candidate {} \
                         while waiting for accepted parent {}",
                        candidate_block_id,
                        candidate_parent_block_id
                    );
                }
            }
        }
        if logged_wait {
            log::debug!(
                "MC validation wait resolved for candidate {} \
                 (parent={}, accepted_head={})",
                candidate_block_id,
                candidate_parent_block_id,
                accepted_block_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<none>".to_string()),
            );
        }
        return Ok(());
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum ValidatorGroupStatus {
    Created,
    EngineCreated,
    Sync,
    Active,
    Stopping,
    Stopped,
}

impl Display for ValidatorGroupStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidatorGroupStatus::Created => write!(f, "created"),
            ValidatorGroupStatus::EngineCreated => write!(f, "engine_created"),
            ValidatorGroupStatus::Sync => write!(f, "sync"),
            ValidatorGroupStatus::Active => write!(f, "active"),
            ValidatorGroupStatus::Stopping => write!(f, "stopping"),
            ValidatorGroupStatus::Stopped => write!(f, "stopped"),
        }
    }
}

impl ValidatorGroupStatus {
    pub fn metric_label(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::EngineCreated => "engine_created",
            Self::Sync => "sync",
            Self::Active => "active",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }
}

impl ValidatorGroupStatus {
    pub fn before(&self, of: &ValidatorGroupStatus) -> bool {
        self <= of
    }
}

#[derive(Clone, Default)]
pub struct PipelineContext {
    blocks: VecDeque<Block>,
    states: VecDeque<Arc<ShardStateStuff>>,
}
impl Display for PipelineContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.states.is_empty() {
            write!(f, "empty")?;
        } else {
            write!(f, "all prevs: ")?;
            for s in &self.states {
                write!(f, "{} ", s.block_id())?;
            }
        }
        Ok(())
    }
}
impl PipelineContext {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, state: Arc<ShardStateStuff>, block: Block, max_size: usize) {
        if self.blocks.len() > max_size {
            self.states.pop_front();
            self.blocks.pop_front();
            log::warn!(
                target: "validator",
                "Accelerated consensus max precollated blocks limit {max_size} reached, dropping old states"
            );
        }
        self.states.push_back(state);
        self.blocks.push_back(block);
    }
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.states.clear();
    }
    pub fn last_id(&self) -> Option<&BlockIdExt> {
        self.states.iter().last().map(|b| b.block_id())
    }
}

pub struct ValidatorGroupImpl {
    local_id: PublicKeyHash,
    prev_block_ids: PrevBlockHistory, //Vec<BlockIdExt>,
    pipeline_context: PipelineContext,
    expected_current_round: u32,
    expected_collation_round: u32,
    is_collator: bool,
    is_accelerated_consensus_enabled: bool,
    is_pipeline_context_enabled: bool,

    shard: ShardIdent,
    session_id: SessionId,
    session: Option<SessionHolderPtr>,
    #[allow(dead_code)]
    consensus_type: ConsensusType,

    min_masterchain_block_id: Option<BlockIdExt>,
    cc_seqno: u32,
    min_ts: SystemTime,

    #[allow(dead_code)]
    replay_finished: bool,

    status: ValidatorGroupStatus,
    start_pending: bool,

    /// Highest MC block seqno accepted (committed) in this session.
    /// Used for MC fork prevention: reject candidates building on stale heads.
    last_accepted_mc_seqno: Option<u32>,
    /// Exact MC block identity accepted (or externally notified) as the current head.
    /// Used for C++-parity stale-branch rejection in MC validation.
    last_accepted_mc_block_id: Option<BlockIdExt>,
    /// Highest external MC-finalized notification seqno delivered via `notify_mc_finalized`.
    /// Used to suppress stale finalized block rebroadcasts (`block-accepter.cpp` parity).
    last_notified_mc_finalized_seqno: Option<u32>,
}

impl Drop for ValidatorGroupImpl {
    fn drop(&mut self) {
        // Does not stop the session to avoid database deletion on validator-manager crash.
        log::info!(target: "validator",
            "SESSION_LIFECYCLE: dropped shard={} cc_seqno={} session_id={:x} final_status={} \
             has_engine={}",
            self.shard, self.cc_seqno, self.session_id, self.status, self.session.is_some());
    }
}

impl ValidatorGroupImpl {
    /// Create the consensus engine (session) without starting the validation queue.
    ///
    /// Two-phase activation (C++ parity): `create_engine()` materializes the consensus
    /// session so future groups have a pre-initialized engine. `start()` then spawns
    /// the validation queue processor and calls `Session::start(initial_block_seqno)`
    /// to begin consensus. If `create_engine()` was not called (e.g. the group was
    /// promoted directly), `start()` creates the session inline as a fallback.
    fn create_engine(
        &mut self,
        g: Arc<ValidatorGroup>,
        session_listener: SessionListenerPtr,
    ) -> Result<()> {
        if self.session.is_some() {
            log::debug!(target: "validator",
                "create_engine: session already exists for shard={} cc_seqno={}, skipping",
                self.shard, self.cc_seqno);
            return Ok(());
        }
        if self.status >= ValidatorGroupStatus::Stopping {
            fail!("Inactive session cannot have engine created! {}", self.info())
        }

        log::info!(target: "validator",
            "SESSION_LIFECYCLE: create_engine shard={} cc_seqno={} session_id={:x} \
             consensus={} status={} -> engine_created",
            self.shard, self.cc_seqno, self.session_id,
            self.consensus_type, self.status);
        let session = self.create_consensus_session(g, session_listener)?;
        self.session = Some(session);
        self.status = ValidatorGroupStatus::EngineCreated;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        session_listener: SessionListenerPtr,
        prev: Vec<BlockIdExt>,
        min_masterchain_block_id: BlockIdExt,
        min_ts: SystemTime,
        g: Arc<ValidatorGroup>,
        rt: tokio::runtime::Handle,
    ) -> Result<()> {
        if self.status >= ValidatorGroupStatus::Stopping {
            fail!("Inactive session cannot be started! {}", self.info())
        }

        let initial_accepted_mc_block_id = initial_accepted_mc_head_from_start_inputs(
            &self.shard,
            &prev,
            &min_masterchain_block_id,
        );

        self.prev_block_ids.update_prev(prev);
        self.min_masterchain_block_id = Some(min_masterchain_block_id.clone());
        self.min_ts = min_ts;
        if self.shard.is_masterchain() {
            self.last_accepted_mc_seqno = Some(min_masterchain_block_id.seq_no);
            self.last_accepted_mc_block_id = initial_accepted_mc_block_id;
        }

        if self.session.is_none() {
            log::info!(target: "validator",
                "SESSION_LIFECYCLE: create_session_at_start shard={} cc_seqno={} consensus={}",
                self.shard, self.cc_seqno, self.consensus_type);
            let session = self.create_consensus_session(g.clone(), session_listener)?;
            self.session = Some(session);
        }

        if let Some(session) = &self.session {
            log::info!(target: "validator",
                "SESSION_LIFECYCLE: session.start(prevs={}, min_mc={}) shard={} cc_seqno={}",
                self.prev_block_ids.display_prevs(),
                min_masterchain_block_id,
                self.shard,
                self.cc_seqno
            );
            session
                .start(self.prev_block_ids.get_prevs().to_vec(), min_masterchain_block_id.clone());
        }

        log::info!(target: "validator",
            "SESSION_LIFECYCLE: start shard={} cc_seqno={} session_id={:x} consensus={} \
             mc_init_seqno={} status={} -> sync",
            self.shard, self.cc_seqno, self.session_id, self.consensus_type,
            self.min_masterchain_block_id.as_ref().map_or(0, |id| id.seq_no),
            self.status);

        let g_clone = g.clone();
        let receiver = g.receiver.lock().unwrap().take().ok_or_else(|| {
            error!("Receiver already taken - session cannot be started multiple times")
        })?;
        rt.clone().spawn(async move {
            process_validation_queue(receiver, g_clone.clone(), rt).await;
        });

        log::debug!(target: "validator",
            "Validation queue spawned for shard={} cc_seqno={}, options={:?}",
            self.shard, self.cc_seqno, g.consensus_options);

        self.status = ValidatorGroupStatus::Sync;
        Ok(())
    }

    fn prepare_start(&mut self) -> bool {
        if self.start_pending {
            return false;
        }
        match self.status {
            ValidatorGroupStatus::Created | ValidatorGroupStatus::EngineCreated => {}
            _ => return false,
        }
        self.start_pending = true;
        true
    }

    fn reset_after_start_failure(&mut self) {
        log::warn!(target: "validator",
            "SESSION_LIFECYCLE: reset_after_failure shard={} cc_seqno={} session_id={:x} \
             status={} -> created (session dropped, will retry)",
            self.shard, self.cc_seqno, self.session_id, self.status);
        self.session = None;
        self.start_pending = false;
        self.status = ValidatorGroupStatus::Created;
    }

    /// Get the consensus type for this validator group
    #[allow(dead_code)]
    pub fn get_consensus_type(&self) -> ConsensusType {
        self.consensus_type
    }

    /// Check if session is active
    #[allow(dead_code)]
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Get simplex session pointer for simplex-specific operations (e.g., MC finalization)
    /// Returns None if this is not a simplex session
    #[allow(dead_code)]
    pub fn get_simplex_session(&self) -> Option<super::consensus::SimplexSessionPtr> {
        self.session.as_ref().and_then(|s| s.get_simplex_session())
    }

    /// Create a consensus session based on the group's consensus type
    ///
    /// This method encapsulates all consensus-specific session creation logic,
    /// making it easy to switch between catchain and simplex implementations.
    /// Returns a `SessionHolderPtr` for direct access to `SessionHolder` methods.
    fn create_consensus_session(
        &self,
        g: Arc<ValidatorGroup>,
        listener: SessionListenerPtr,
    ) -> Result<SessionHolderPtr> {
        use super::consensus::ConsensusFactory;

        let nodes: Vec<SessionNode> = g
            .validator_set
            .list()
            .iter()
            .map(validatordescr_to_session_node)
            .collect::<Result<_>>()?;

        let block_sync_params_with_identity = g
            .block_sync_overlay_params
            .clone()
            .map(|p| p.with_identity(g.shard.clone(), g.session_id.clone()));
        let overlay_manager: ConsensusOverlayManagerPtr =
            Arc::new(ConsensusOverlayManagerImpl::new(
                g.engine.validator_network(),
                g.validator_list_id.clone(),
                block_sync_params_with_identity,
            ));

        let db_root = format!("{}/catchains", g.engine.db_root_dir()?);
        let is_masterchain = g.shard.is_masterchain();

        match &g.consensus_options {
            ConsensusOptions::Catchain(catchain_options) => {
                ConsensusFactory::create_catchain_based_session(
                    catchain_options,
                    &g.session_id,
                    nodes,
                    &g.local_key,
                    db_root,
                    g.general_session_info.catchain_seqno,
                    g.allow_unsafe_self_blocks_resync,
                    overlay_manager,
                    listener,
                    is_masterchain,
                )
            }
            ConsensusOptions::Simplex(simplex_options) => {
                ConsensusFactory::create_simplex_based_session(
                    simplex_options,
                    &g.session_id,
                    &g.shard,
                    nodes,
                    &g.local_key,
                    db_root,
                    g.general_session_info.catchain_seqno,
                    overlay_manager,
                    listener,
                )
            }
        }
    }

    pub fn info_round(&self, round: u32) -> String {
        let next_seqno = self
            .prev_block_ids
            .get_next_seqno()
            .map_or("".to_owned(), |seqno| format!(", {} next seqno", seqno));
        format!(
            "session_status: id {:x}, shard {}{}, {}, cc_seqno {}, mc_initial_seqno {}, round {}, prevs {}",
            self.session_id,
            self.shard,
            next_seqno,
            self.status,
            self.cc_seqno,
            self.min_masterchain_block_id.as_ref().map_or(0, |id| id.seq_no),
            round,
            self.prev_block_ids
        )
    }

    pub fn info(&self) -> String {
        self.info_round(self.expected_current_round)
    }

    // Initializes structure
    pub fn new(
        local_id: &PublicKeyHash,
        shard: ShardIdent,
        cc_seqno: u32,
        session_id: SessionId,
        is_accelerated_consensus_enabled: bool,
        is_pipeline_context_enabled: bool,
        consensus_type: ConsensusType,
    ) -> ValidatorGroupImpl {
        log::info!(target: "validator",
            "SESSION_LIFECYCLE: created shard={} cc_seqno={} session_id={:x} consensus={} local_id={}",
            shard, cc_seqno, session_id, consensus_type, local_id);

        let prev_block_ids = PrevBlockHistory::with_shard(&shard);
        ValidatorGroupImpl {
            local_id: local_id.clone(),
            is_accelerated_consensus_enabled,
            is_pipeline_context_enabled,
            min_masterchain_block_id: None,
            cc_seqno,
            min_ts: SystemTime::now(),
            status: ValidatorGroupStatus::Created,
            start_pending: false,
            expected_current_round: 0,
            expected_collation_round: 0,
            is_collator: false,

            shard,
            session_id,
            session: None,
            consensus_type,
            prev_block_ids,
            pipeline_context: PipelineContext::new(),

            replay_finished: false,
            last_accepted_mc_seqno: None,
            last_accepted_mc_block_id: None,
            last_notified_mc_finalized_seqno: None,
        }
    }

    /*
       pub fn update_next_validator_set(&mut self, catchain_seqno: u32, curr_set: &[ValidatorDescr], next_set: &[ValidatorDescr]) {
           self.reliable_queue.switch_queue(catchain_seqno, curr_set, next_set);
       }
    */

    fn finish_round(&mut self, round: u32, is_block_ours: bool, is_block_skipped: bool) {
        // SIMPLEX_ROUNDLESS: bypass round-based invariants for Simplex
        // When round == SIMPLEX_ROUNDLESS, skip expected_current_round tracking entirely.
        // Simplex uses seqno-based tracking instead of round-based tracking.
        if is_simplex_roundless(round) {
            // In Simplex roundless mode:
            // - Don't validate or advance expected_current_round
            // - Don't validate or advance expected_collation_round
            // - Still handle pipeline_context clearing for non-ours blocks
            // - Still track is_collator for logging purposes

            if !is_block_ours {
                if self.is_collator && !is_block_skipped {
                    if self.is_accelerated_consensus_enabled {
                        log::info!(
                            target: "validator",
                            "COLLATOR STATUS: Node {} lost collator status for session {} in shard ({}) [SIMPLEX_ROUNDLESS]",
                            self.local_id,
                            self.session_id.to_hex_string(),
                            self.shard
                        );
                    }
                    self.is_collator = false;
                }

                // Clear pipeline context when another validator's block is committed.
                // With notarized-parent collation, our precollated blocks may be built on
                // a parent that is now stale (another branch was committed). Conservative
                // but correct: the next collation will re-derive from the current FSM parent.
                if !self.pipeline_context.is_empty() {
                    log::trace!(
                        target: "validator",
                        "Flushing collation pipeline [SIMPLEX_ROUNDLESS]"
                    );
                    self.pipeline_context.clear();
                }
            }
            return;
        }

        // Standard round-based tracking (validator-session / catchain mode)
        if round != self.expected_current_round {
            log::error!(
                target: "validator",
                "round {round} != expected_current_round {}, expected_current_round sequence violation",
                self.expected_current_round
            );
        }

        self.expected_current_round = round + 1;

        if is_block_ours {
            if self.expected_current_round > self.expected_collation_round {
                if !self.pipeline_context.is_empty() {
                    log::error!(
                        target: "validator",
                        "INTERNAL ERROR: Commit vs collation sequence violation on round {round} \
                        (expected_current_round = {}, expected_collation_round = {}). Flushing pipeline",
                        self.expected_current_round,
                        self.expected_collation_round
                    );
                    self.pipeline_context.clear();
                }

                self.expected_collation_round = self.expected_current_round;
            }
        } else {
            if self.is_collator && !is_block_skipped && self.is_accelerated_consensus_enabled {
                log::info!(
                    target: "validator",
                    "COLLATOR STATUS: Node {} lost collator status for session {} in shard ({}) on round {round}",
                    self.local_id,
                    self.session_id.to_hex_string(),
                    self.shard
                );
                self.is_collator = false;
            }

            if !self.pipeline_context.is_empty() {
                log::info!(target: "validator", "Flushing collation pipeline on round {}", round);
                self.pipeline_context.clear();
            }

            self.expected_collation_round = self.expected_current_round;
        }
    }
}

pub struct ValidatorGroup {
    general_session_info: Arc<GeneralSessionInfo>,
    local_key: PrivateKey,
    consensus_options: ConsensusOptions,
    is_accelerated_consensus_enabled: bool,
    session_id: SessionId,
    shard: ShardIdent,
    validator_list_id: ValidatorListHash,

    engine: Arc<dyn EngineOperations>,
    validator_set: ValidatorSet,
    #[allow(dead_code)]
    allow_unsafe_self_blocks_resync: bool,

    group_impl: Arc<MutexWrapper<ValidatorGroupImpl>>,
    /// Validator-side cache/resolver for speculative shard states.
    ///
    /// Simplex delivers candidate observations through `SessionListener`.
    /// We persist those observations here and resolve parent states before
    /// falling back to `engine.wait_state()`.
    state_resolver_cache: Arc<tokio::sync::Mutex<StateResolverCache>>,
    action_queue: tokio::sync::mpsc::UnboundedSender<ValidationAction>,
    callback: Arc<dyn SessionListener + Send + Sync>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ValidationAction>>>>,

    last_validation_time: Arc<AtomicU64>,
    last_collation_time: Arc<AtomicU64>,
    is_collating: Arc<AtomicBool>,
    accepted_mc_seqno_tx: tokio::sync::watch::Sender<Option<u32>>,
    accepted_mc_block_tx: tokio::sync::watch::Sender<Option<BlockIdExt>>,
    /// Set by the validation queue on prolonged inactivity, cleared on any action.
    pub stalled: Arc<AtomicBool>,
    /// Block-sync overlay membership and authorization; `Some` only when this shard has
    /// `simplex_config_v2.enable_observers=true`
    block_sync_overlay_params: Option<consensus_common::BlockSyncOverlayParams>,
}

impl ResolverBackend for ValidatorGroup {
    fn request_candidate_availability(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        // Reverse bridge:
        // StateResolverCache -> ValidatorGroup (ResolverBackend) -> SimplexSession.
        // This keeps cache/resolver logic independent from simplex internals.
        let group_impl = self.group_impl.clone();
        let session_id = self.session_id.clone();
        let shard = self.shard.clone();
        tokio::spawn(async move {
            let simplex_session = group_impl.execute_sync(|gi| gi.get_simplex_session()).await;
            match simplex_session {
                Some(session) => {
                    log::info!(
                        target: "simplex_resolver",
                        "ResolverBackend::request_candidate_availability session_id={:x} shard={} block_id={} purpose={:?}",
                        session_id,
                        shard,
                        block_id,
                        opts.purpose,
                    );
                    session.ensure_candidate_available(block_id, opts);
                }
                None => {
                    log::warn!(
                        target: "simplex_resolver",
                        "ResolverBackend::request_candidate_availability: no simplex session for shard={} block_id={}",
                        shard,
                        block_id,
                    );
                }
            }
        });
    }
}

impl ValidatorGroup {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        general_session_info: Arc<GeneralSessionInfo>,
        local_key: PrivateKey,
        session_id: SessionId,
        validator_list_id: ValidatorListHash,
        validator_set: ValidatorSet,
        consensus_options: ConsensusOptions,
        engine: Arc<dyn EngineOperations>,
        allow_unsafe_self_blocks_resync: bool,
        block_sync_overlay_params: Option<consensus_common::BlockSyncOverlayParams>,
    ) -> Self {
        let consensus_type = consensus_options.consensus_type();
        let is_accelerated = consensus_options.is_accelerated_consensus_enabled();
        let is_pipeline_context_enabled = consensus_options.is_pipeline_context_enabled();

        let group_impl = ValidatorGroupImpl::new(
            local_key.id(),
            general_session_info.shard.clone(),
            general_session_info.catchain_seqno,
            session_id.clone(),
            is_accelerated,
            is_pipeline_context_enabled,
            consensus_type,
        );
        let id = format!("Val. group {} {:x}", general_session_info.shard, session_id);
        let (listener, receiver) = ValidatorSessionListener::create(
            session_id.clone(),
            general_session_info.shard.clone(),
        );
        let action_queue = listener.queue_sender();
        let (accepted_mc_seqno_tx, _accepted_mc_seqno_rx) = tokio::sync::watch::channel(None);
        let (accepted_mc_block_tx, _accepted_mc_block_rx) = tokio::sync::watch::channel(None);

        log::trace!(target: "validator", "Creating validator group: {}, consensus_type: {}", id, consensus_type);
        ValidatorGroup {
            shard: general_session_info.shard.clone(),
            general_session_info,
            local_key,
            validator_list_id,
            session_id,
            validator_set,
            consensus_options,
            is_accelerated_consensus_enabled: is_accelerated,
            engine,
            allow_unsafe_self_blocks_resync,
            group_impl: Arc::new(MutexWrapper::new(group_impl, id)),
            state_resolver_cache: Arc::new(tokio::sync::Mutex::new(StateResolverCache::new())),
            action_queue,
            callback: Arc::new(listener),
            receiver: Arc::new(Mutex::new(Some(receiver))),
            last_validation_time: Arc::new(AtomicU64::new(0)),
            last_collation_time: Arc::new(AtomicU64::new(0)),
            is_collating: Arc::new(AtomicBool::new(false)),
            accepted_mc_seqno_tx,
            accepted_mc_block_tx,
            stalled: Arc::new(AtomicBool::new(false)),
            block_sync_overlay_params,
        }
    }

    /// Mutex used inside. Needs to cache the result
    pub async fn get_next_block_descr(&self, root_hash: Option<&BlockHash>) -> String {
        self.group_impl
            .execute_sync(|group_impl| group_impl.prev_block_ids.get_next_block_descr(root_hash))
            .await
    }

    pub fn shard(&self) -> &ShardIdent {
        &self.general_session_info.shard
    }

    pub fn cc_seqno(&self) -> u32 {
        self.general_session_info.catchain_seqno
    }

    pub fn is_simplex(&self) -> bool {
        matches!(self.consensus_options, ConsensusOptions::Simplex(_))
    }

    pub async fn snapshot(&self) -> SessionSnapshot {
        let key_seqno = self.general_session_info.key_seqno;
        self.group_impl
            .execute_sync(|g| SessionSnapshot {
                session_id: g.session_id.clone(),
                shard: g.shard.clone(),
                cc_seqno: g.cc_seqno,
                status: g.status,
                consensus_type: g.consensus_type,
                round: g.expected_current_round,
                mc_initial_seqno: g.min_masterchain_block_id.as_ref().map_or(0, |id| id.seq_no),
                has_engine: g.session.is_some(),
                is_collator: g.is_collator,
                created_at: g.min_ts,
                key_seqno,
                last_accepted_mc_seqno: g.last_accepted_mc_seqno,
            })
            .await
    }

    /// Enqueue an applied-top update into this group's ordered action queue.
    ///
    /// For simplex sessions, the queued action eventually updates the session processor:
    /// - shard sessions use it for empty-block recovery against MC-registered tops
    /// - masterchain sessions use it to mirror the applied MC head
    ///
    /// The manager should not await this on the hot path; queue processing preserves
    /// per-group sequencing with other validator-group actions.
    ///
    /// # Arguments
    /// * `applied_top` - Current applied top for this group shard
    pub fn notify_mc_finalized(&self, applied_top: BlockIdExt) {
        if let Err(error) = self.action_queue.send(ValidationAction::OnAppliedTop { applied_top }) {
            log::warn!(
                target: "validator",
                "Failed to enqueue applied-top notification for {}: {}",
                self.shard,
                error
            );
        }
    }

    fn publish_accepted_mc_seqno(&self, seqno: Option<u32>) {
        if self.shard.is_masterchain() {
            self.accepted_mc_seqno_tx.send_replace(seqno);
        }
    }

    fn publish_accepted_mc_head(&self, block_id: Option<BlockIdExt>) {
        if self.shard.is_masterchain() {
            self.accepted_mc_block_tx.send_replace(block_id);
        }
    }

    pub async fn on_applied_top(&self, applied_top: BlockIdExt) {
        let (accepted_mc_seqno, accepted_mc_block_id) = self
            .group_impl
            .execute_sync(|group_impl| {
                sync_last_notified_mc_finalized_seqno(group_impl, &applied_top);
                // C++ parity (block-validator.cpp):
                // BlockFinalizedInMasterchain ignores seqno 0 for accepted-head progression.
                if !(group_impl.shard.is_masterchain() && applied_top.seq_no == 0) {
                    sync_last_accepted_mc_head_from_block(group_impl, &applied_top);
                }
                if let Some(ref session) = group_impl.session {
                    session.notify_mc_finalized(applied_top);
                }
                (group_impl.last_accepted_mc_seqno, group_impl.last_accepted_mc_block_id.clone())
            })
            .await;
        self.publish_accepted_mc_seqno(accepted_mc_seqno);
        self.publish_accepted_mc_head(accepted_mc_block_id);
    }

    pub fn is_collating(&self) -> bool {
        self.is_collating.load(Ordering::Relaxed)
    }

    pub fn last_validation_time(&self) -> u64 {
        self.last_validation_time.load(Ordering::Relaxed)
    }

    pub fn last_collation_time(&self) -> u64 {
        self.last_collation_time.load(Ordering::Relaxed)
    }

    pub fn make_validator_session_callback(&self) -> SessionListenerPtr {
        Arc::downgrade(&self.callback)
    }

    /// Pre-create the consensus engine without starting the validation queue.
    /// Called for future-validator groups to warm up the session ahead of time.
    pub async fn pre_create_engine(self: Arc<ValidatorGroup>) -> Result<()> {
        let callback = self.make_validator_session_callback();
        self.group_impl
            .execute_sync(|group_impl| {
                if let Err(e) = group_impl.create_engine(self.clone(), callback) {
                    log::error!(target: "validator",
                        "SESSION_LIFECYCLE: pre_create_engine failed shard={} cc_seqno={} \
                         session_id={:x}: {}",
                        group_impl.shard, group_impl.cc_seqno, group_impl.session_id, e);
                }
            })
            .await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_session(
        self: Arc<ValidatorGroup>,
        prev: Vec<BlockIdExt>,
        min_masterchain_block_id: BlockIdExt,
        min_ts: SystemTime,
        rt: tokio::runtime::Handle,
    ) -> Result<()> {
        rt.clone().spawn(async move {
            let callback = self.make_validator_session_callback();
            self.group_impl
                .execute_sync(|group_impl| {
                    if group_impl.status <= ValidatorGroupStatus::Active {
                        if let Err(e) = group_impl.start(
                            callback,
                            prev,
                            min_masterchain_block_id,
                            min_ts,
                            self.clone(),
                            rt,
                        ) {
                            group_impl.reset_after_start_failure();
                            log::error!(
                                target: "validator",
                                "Cannot start group: {}; resetting session to Created for retry",
                                e
                            );
                        } else {
                            group_impl.start_pending = false;
                        }
                    } else {
                        group_impl.start_pending = false;
                        log::trace!(target: "validator", "Session deleted before start: {}", group_impl.info());
                    }
                })
                .await;
            self.publish_accepted_mc_seqno(
                self.group_impl.execute_sync(|group_impl| group_impl.last_accepted_mc_seqno).await,
            );
            self.publish_accepted_mc_head(
                self.group_impl
                    .execute_sync(|group_impl| group_impl.last_accepted_mc_block_id.clone())
                    .await,
            );

            if matches!(self.consensus_options, ConsensusOptions::Simplex(_)) {
                // Bind cache backend only for simplex sessions.
                // Catchain mode must not request non-finalized parents via simplex.
                self.state_resolver_cache.lock().await.set_backend(
                    Arc::downgrade(&self) as Weak<dyn ResolverBackend>,
                );
            }
        });
        Ok(())
    }

    pub async fn stop(
        self: Arc<ValidatorGroup>,
        rt: tokio::runtime::Handle,
        destroy_database: bool,
    ) -> Result<()> {
        self.set_status(ValidatorGroupStatus::Stopping).await?;
        let consensus_label = if self.is_simplex() { "simplex" } else { "catchain" };
        metrics::counter!("ton_node_validator_session_stopped_total", "consensus" => consensus_label)
            .increment(1);
        let shard = self.shard.clone();
        let cc_seqno = self.cc_seqno();
        log::info!(target: "validator",
            "SESSION_LIFECYCLE: stop_initiated shard={} cc_seqno={} destroy_db={}",
            shard, cc_seqno, destroy_database);
        let group_impl = self.group_impl.clone();
        let self_clone = self.clone();
        rt.spawn({
            async move {
                let session_ptr =
                    group_impl.execute_sync(|group_impl| group_impl.session.clone()).await;
                if let Some(s_ptr) = session_ptr {
                    log::debug!(target: "validator",
                        "Stopping consensus engine for shard={} cc_seqno={}", shard, cc_seqno);
                    if destroy_database {
                        s_ptr.destroy();
                    } else {
                        s_ptr.stop();
                    }
                }
                let _ = self_clone.set_status(ValidatorGroupStatus::Stopped).await;
                if destroy_database {
                    let _ = self_clone.destroy_db().await;
                    log::debug!(target: "validator",
                        "DB destroyed for shard={} cc_seqno={}", shard, cc_seqno);
                }
                log::info!(target: "validator",
                    "SESSION_LIFECYCLE: stopped shard={} cc_seqno={} destroy_db={}",
                    shard, cc_seqno, destroy_database);
            }
        });
        Ok(())
    }

    async fn load_block_candidate(
        &self,
        root_hash: &UInt256,
    ) -> Result<Arc<ValidatorBlockCandidate>> {
        self.engine.load_block_candidate(&self.session_id, root_hash)
    }

    pub async fn destroy_db(&self) -> Result<()> {
        while !self.engine.destroy_block_candidates(&self.session_id)? {
            tokio::task::yield_now().await
        }
        Ok(())
    }

    pub async fn get_status(&self) -> ValidatorGroupStatus {
        self.group_impl.execute_sync(|group_impl| group_impl.status).await
    }

    pub async fn is_start_pending(&self) -> bool {
        self.group_impl.execute_sync(|group_impl| group_impl.start_pending).await
    }

    pub async fn try_prepare_start(&self) -> Result<bool> {
        self.group_impl.execute_sync(|group_impl| Ok(group_impl.prepare_start())).await
    }

    pub async fn set_status(&self, status: ValidatorGroupStatus) -> Result<()> {
        self.group_impl
            .execute_sync(|group_impl| {
                if group_impl.status.before(&status) {
                    let from = group_impl.status;
                    group_impl.status = status;
                    log::info!(target: "validator",
                        "SESSION_LIFECYCLE: transition shard={} cc_seqno={} session_id={:x} {} -> {}",
                        group_impl.shard, group_impl.cc_seqno, group_impl.session_id, from, status);
                    Ok(())
                } else {
                    log::error!(target: "validator",
                        "SESSION_LIFECYCLE: invalid transition shard={} cc_seqno={} session_id={:x} \
                         {} -> {} (monotonic violation)",
                        group_impl.shard, group_impl.cc_seqno, group_impl.session_id,
                        group_impl.status, status);
                    fail!("Status cannot retreat, from {} to {}", group_impl.status, status)
                }
            })
            .await
    }

    pub fn get_validator_list_id(&self) -> ValidatorListHash {
        self.validator_list_id.clone()
    }

    pub async fn info_round(&self, round: u32) -> String {
        self.group_impl.execute_sync(|group_impl| group_impl.info_round(round)).await
    }

    pub async fn info(&self) -> String {
        self.group_impl.execute_sync(|group_impl| group_impl.info()).await
    }

    pub async fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        log::info!(
            target: "simplex_resolver",
            "ValidatorGroup::on_candidate_observed session_id={:x} shard={} block_id={} parent_ready={} local_collated={} body_present={}",
            self.session_id,
            self.shard,
            block_id,
            flags.parent_ready,
            flags.local_collated,
            flags.body_present,
        );

        // Pre-BlockSync the handle was written implicitly by
        // `run_validate_query_any_candidate` -> `store_validated_block`, but only
        // when this node actually validated the candidate. With the block-sync
        // overlay carrying candidate bodies, simplex may receive a body it does
        // not validate (consensus quorum already reached)
        let block = if flags.body_present {
            let block_bytes = data.data().to_vec();
            match crate::block::BlockStuff::deserialize_block(
                block_id.clone(),
                std::sync::Arc::new(block_bytes),
            ) {
                Ok(block_stuff) => {
                    if let Err(e) = self.engine.store_block(&block_stuff).await {
                        log::debug!(
                            target: "simplex_resolver",
                            "on_candidate_observed: store_block failed (non-fatal) for {}: {}",
                            block_id, e
                        );
                    } else {
                        log::trace!(
                            target: "simplex_resolver",
                            "on_candidate_observed: stored block handle eagerly for {}",
                            block_id
                        );
                    }
                    block_stuff.block().ok().cloned()
                }
                Err(e) => {
                    log::warn!(
                        target: "simplex_resolver",
                        "on_candidate_observed: deserialize_block failed for {}: {}",
                        block_id, e
                    );
                    None
                }
            }
        } else {
            None
        };

        self.state_resolver_cache.lock().await.upsert_observed_candidate(
            block_id,
            data,
            collated_data,
            flags,
            block,
        );
    }

    pub async fn on_generate_slot(
        &self,
        source_info: validator_session::BlockSourceInfo,
        request: validator_session::AsyncRequestPtr,
        parent: CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        let round = source_info.priority.round;
        let request_id = request.get_request_id();

        // Check if request is already cancelled
        if request.is_cancelled() {
            log::debug!(
                target: "validator",
                "Collation request {} for round {} was cancelled before processing",
                request_id,
                round
            );
            return;
        }

        let (
            expected_collation_round,
            prev_block_ids,
            pipeline_context,
            mm_block_id,
            min_ts,
            shard,
            status,
            is_collator,
        ) = self
            .group_impl
            .execute_sync(|group_impl| {
                let was_collator = group_impl.is_collator;

                group_impl.is_collator = true;

                (
                    group_impl.expected_collation_round,
                    group_impl.prev_block_ids.clone(),
                    group_impl.pipeline_context.clone(), //TODO: optimize
                    group_impl.min_masterchain_block_id.clone(),
                    group_impl.min_ts,
                    group_impl.shard.clone(),
                    group_impl.status,
                    was_collator,
                )
            })
            .await;
        let min_ts = min_ts.max(request.get_creation_time());

        let is_simplex = matches!(self.consensus_options, ConsensusOptions::Simplex(_));
        if is_simplex {
            match &parent {
                CollationParentHint::Implicit => {
                    panic!(
                        "ValidatorGroup::on_generate_slot: Simplex must not use implicit collation parents"
                    );
                }
                CollationParentHint::Explicit(parent_block_ids) => {
                    assert!(
                        !parent_block_ids.is_empty() && parent_block_ids.len() <= 2,
                        "ValidatorGroup::on_generate_slot: Simplex explicit parents must contain one or two block ids"
                    );
                    for parent_block_id in parent_block_ids {
                        self.state_resolver_cache.lock().await.request_availability(
                            parent_block_id,
                            ResolverPurpose::SimplexCollationParent,
                        );
                        log::info!(
                            target: "simplex_resolver",
                            "ValidatorGroup::on_generate_slot session_id={:x} shard={} explicit_parent={} requested_availability=true",
                            self.session_id,
                            self.shard,
                            parent_block_id,
                        );
                    }
                }
            }
        }

        if !is_collator && self.is_accelerated_consensus_enabled {
            log::info!(
                target: "validator",
                "COLLATOR STATUS: Node {} became a collator for session {} in shard ({}) on round {round}",
                self.local_key.id(),
                self.session_id.to_hex_string(),
                self.shard
            );
        }

        // Atomically check and set is_collating flag
        if self
            .is_collating
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            log::warn!(target: "validator", "Collation pipeline is already running. Skipping collation request.");
            return;
        }

        let prev_block_ids = match &parent {
            CollationParentHint::Implicit => {
                assert!(
                    !is_simplex,
                    "ValidatorGroup::on_generate_slot: Simplex must not use implicit collation parents"
                );
                if let Some(prev_id) = pipeline_context.last_id() {
                    // Construct prev_block_ids for precollations (accelerated consensus).
                    PrevBlockHistory::with_id(prev_id.clone())
                } else {
                    log::info!(
                        target: "validator",
                        "Collation priority assignment was detected on round {round}. \
                        Enable precollation pipeline (collation request ID: {request_id})"
                    );
                    prev_block_ids
                }
            }
            CollationParentHint::Explicit(parent_ids) => {
                if parent_ids.is_empty() || parent_ids.len() > 2 {
                    self.is_collating.store(false, Ordering::Release);
                    callback(Err(error!("Explicit parents must contain one or two block ids")));
                    return;
                }
                log::trace!(
                    target: "validator",
                    "ValidatorGroup::on_generate_slot: using explicit parents \
                    (round={}, request_id={}, parents={})",
                    round,
                    request_id,
                    prevs_to_string(parent_ids)
                );

                if is_simplex {
                    PrevBlockHistory::with_prevs(&shard, parent_ids.clone())
                } else {
                    let last_committed_seqno =
                        prev_block_ids.get_next_seqno().and_then(|n| n.checked_sub(1)).unwrap_or(0);
                    let last_collated_seqno = pipeline_context
                        .last_id()
                        .map(|id| id.seq_no)
                        .unwrap_or(last_committed_seqno);
                    let min_allowed_parent_seqno =
                        std::cmp::max(last_committed_seqno, last_collated_seqno);
                    let explicit_parent_seqno =
                        parent_ids.iter().map(|id| id.seq_no).max().unwrap_or(last_committed_seqno);

                    if explicit_parent_seqno < min_allowed_parent_seqno {
                        log::error!(
                            target: "validator",
                            "ValidatorGroup::on_generate_slot: explicit parents are too old \
                            (round={}, request_id={}, parent_seqno={}, min_allowed_seqno={}, committed_seqno={}, collated_seqno={}, parents={})",
                            round,
                            request_id,
                            explicit_parent_seqno,
                            min_allowed_parent_seqno,
                            last_committed_seqno,
                            last_collated_seqno,
                            prevs_to_string(parent_ids)
                        );
                        self.is_collating.store(false, Ordering::Release);
                        callback(Err(error!("Explicit parents are too old")));
                        return;
                    }

                    PrevBlockHistory::with_prevs(&shard, parent_ids.clone())
                }
            }
        };

        let (next_block_descr, round_info) = {
            let root_hash = None;
            let next_block_descr = prev_block_ids.get_next_block_descr(root_hash);
            let next_seqno = prev_block_ids
                .get_next_seqno()
                .map_or("".to_owned(), |seqno| format!(", {} next seqno", seqno));
            let round_info = format!(
                "session_status: id {:x}, shard {}{}, {}, round {}, request_id {}, prevs {}",
                self.session_id, shard, next_seqno, status, round, request_id, prev_block_ids
            );

            (next_block_descr, round_info)
        };

        let shard = self.shard().clone();
        let engine = self.engine.clone();
        let local_key = self.local_key.clone();
        let validator_set = self.validator_set.clone(); //TODO: optimize
        let group_impl = self.group_impl.clone();
        let state_resolver_cache = self.state_resolver_cache.clone();
        let last_collation_time = self.last_collation_time.clone();
        let is_collating = self.is_collating.clone();
        let max_precollated_blocks = match &self.consensus_options {
            ConsensusOptions::Catchain(opts) => {
                opts.accelerated_consensus_max_precollated_blocks as usize
            }
            ConsensusOptions::Simplex(opts) => {
                opts.slots_per_leader_window.saturating_sub(1) as usize
            }
        };
        let request_clone = request.clone();
        let cc_seqno = self.general_session_info.catchain_seqno;
        let is_masterchain = self.shard.is_masterchain();

        let collation_task = tokio::spawn(async move {
            log::info!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_generate_slot: collator request, {round_info}"
            );

            // Check if request was cancelled before starting collation
            if request_clone.is_cancelled() {
                log::debug!(
                    target: "validator",
                    "({next_block_descr}): Collation request {} for round {} was cancelled before collation started",
                    request_id,
                    round
                );
                is_collating.store(false, Ordering::Release);
                return;
            }

            // SIMPLEX_ROUNDLESS: bypass collation round check for Simplex
            // When round == SIMPLEX_ROUNDLESS, skip the expected_collation_round validation
            let is_roundless = is_simplex_roundless(round);

            // C++ parity: bounded collation deadline (10s from validator-group.cpp /
            // collation-manager.cpp). On timeout, report failure and clear is_collating
            // so the next request can proceed.
            let collation_future = async {
                if is_roundless || round == expected_collation_round {
                    let (result, new_state_n_block, result_message) = match mm_block_id {
                        Some(mc) => {
                            match run_collate_query(
                                shard.clone(),
                                min_ts,
                                mc.seq_no,
                                &prev_block_ids,
                                state_resolver_cache.clone(),
                                local_key,
                                validator_set.clone(),
                                engine.clone(),
                                is_simplex,
                            )
                            .await
                            {
                                Ok((candidate, new_state, new_block, block_root)) => {
                                    let now = UnixTime::now();
                                    last_collation_time.fetch_max(now, Ordering::Relaxed);

                                    if need_send_candidate_broadcast(&source_info, is_masterchain) {
                                        let validator_set_hash =
                                            ValidatorSet::calc_subset_hash_short(
                                                validator_set.list(),
                                                cc_seqno,
                                            )
                                            .unwrap_or(0);

                                        if let Err(e) = engine
                                            .send_block_candidate_broadcast(
                                                &candidate.id,
                                                cc_seqno,
                                                validator_set_hash,
                                                &block_root,
                                            )
                                            .await
                                        {
                                            log::warn!(
                                                target: "validator",
                                                "({next_block_descr}): Failed to send block candidate broadcast after collation: {}",
                                                e
                                            );
                                        } else {
                                            log::debug!(
                                                target: "validator",
                                                "({next_block_descr}): Sent block candidate broadcast after collation"
                                            );
                                        }
                                    }

                                    let new_state_n_block = Some((new_state, new_block));

                                    (
                                        Ok(candidate),
                                        new_state_n_block,
                                        "Collation successful".to_string(),
                                    )
                                }
                                Err(err) => {
                                    let err_msg = format!("Collation failed: `{}`", err);
                                    (Err(err), None, err_msg)
                                }
                            }
                        }
                        None => (
                            Err(error!("Min masterchain block id missing")),
                            None,
                            "Collation failed: Min masterchain block id missing".to_string(),
                        ),
                    };

                    if let Some((new_state, new_block)) = new_state_n_block {
                        group_impl
                            .execute_sync(|group_impl| {
                                if !is_roundless {
                                    group_impl.expected_collation_round = round + 1;
                                }

                                if group_impl.is_pipeline_context_enabled {
                                    group_impl.pipeline_context.add(
                                        new_state.clone(),
                                        new_block.clone(),
                                        max_precollated_blocks,
                                    );
                                }
                            })
                            .await;

                        if is_simplex {
                            let candidate_for_cache =
                                result.as_ref().ok().cloned().expect(
                                    "Simplex successful collation must produce a candidate",
                                );
                            let mut cache = state_resolver_cache.lock().await;
                            cache.upsert_observed_candidate(
                                candidate_for_cache.id.clone(),
                                candidate_for_cache.data.clone(),
                                candidate_for_cache.collated_data.clone(),
                                CandidateObservedFlags {
                                    body_present: true,
                                    parent_ready: true,
                                    local_collated: true,
                                },
                                Some(new_block.clone()),
                            );
                            cache.store_validated_state(&candidate_for_cache.id, new_state);
                        }
                    }

                    (result, result_message)
                } else {
                    let result_message = format!(
                        "round {} != expected_collation_round {}. Collation sequence violation",
                        round, expected_collation_round
                    );

                    (Err(anyhow::anyhow!(result_message.clone())), result_message)
                }
            };

            let (result, result_message) = match tokio::time::timeout(
                COLLATION_TIMEOUT,
                collation_future,
            )
            .await
            {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    metrics::counter!("simplex_collation_timeout").increment(1);
                    let msg = format!("Collation timed out after {:?}", COLLATION_TIMEOUT);
                    log::warn!(
                        target: "validator",
                        "({next_block_descr}): ValidatorGroup::on_generate_slot: {round_info}, {msg}"
                    );
                    (Err(error!("{}", msg)), msg)
                }
            };

            log::info!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_generate_slot: {round_info}, {result_message}"
            );

            callback(result);

            is_collating.store(false, Ordering::Release);
        });

        if WAIT_FOR_COLLATION {
            let _ = collation_task.await;
        }
    }

    // Validate_query
    pub async fn on_candidate(
        &self,
        source_info: validator_session::BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        let round = source_info.priority.round;
        let source = source_info.source.clone();
        let next_block_descr = self.get_next_block_descr(Some(&root_hash)).await;

        let candidate_id = format!("source {}, rh {:x}", source.id(), root_hash);

        log::trace!(target: "validator", "({}): ValidatorGroup::on_candidate: {}, {}",
            next_block_descr,
            candidate_id, self.info_round(round).await);

        let use_candidate_native = is_simplex_roundless(round);

        let candidate_block_id = if use_candidate_native {
            // Simplex: derive block_id from the block header itself,
            // independent of committed prev_block_ids.
            BlockIdExt::with_params(
                self.shard().clone(),
                0, // placeholder seqno — overwritten below after parsing
                root_hash.clone(),
                get_hash(data.data()),
            )
        } else {
            self.group_impl
                .execute_sync(|group_impl| {
                    group_impl.prev_block_ids.get_next_block_id(&root_hash, &get_hash(data.data()))
                })
                .await
        };

        let candidate = super::BlockCandidate {
            block_id: candidate_block_id,
            data: data.data().clone(),
            collated_file_hash: catchain::utils::get_hash(collated_data.data()),
            collated_data: collated_data.data().to_vec(),
            created_by: UInt256::from(source.pub_key().expect("source must contain pub_key")),
        };

        log::trace!(
            target: "validator",
            "({next_block_descr}): ValidatorGroup::on_candidate: {candidate_id}, {}, spawning validation task",
            self.info_round(round).await
        );

        let group_impl = self.group_impl.clone();
        let engine = self.engine.clone();
        let validator_set = self.validator_set.clone();
        let shard = self.shard().clone();
        let state_resolver_cache = self.state_resolver_cache.clone();
        let general_session_info = self.general_session_info.clone();
        let session_id = self.session_id.clone();
        let last_validation_time = self.last_validation_time.clone();
        let cc_seqno = self.general_session_info.catchain_seqno;
        let is_masterchain = self.shard.is_masterchain();
        let is_simplex = matches!(self.consensus_options, ConsensusOptions::Simplex(_));
        let (expected_current_round, prev_block_ids, mc_block_id_opt, min_ts) = group_impl
            .execute_sync(|group_impl| {
                (
                    group_impl.expected_current_round,
                    group_impl.prev_block_ids.clone(),
                    group_impl.min_masterchain_block_id.clone(),
                    group_impl.min_ts,
                )
            })
            .await;
        let accepted_mc_block_rx = self.accepted_mc_block_tx.subscribe();

        let validation_task = tokio::spawn(async move {
            // C++ parity: bounded validation deadline.
            // Simplex: 60s (block-validator.cpp), legacy: 15s (validator-group.cpp).
            let deadline = if use_candidate_native {
                SIMPLEX_VALIDATION_TIMEOUT
            } else {
                LEGACY_VALIDATION_TIMEOUT
            };

            let validation_result = tokio::time::timeout(deadline, async {
                if use_candidate_native {
                    // ---- Simplex candidate-native validation path ----
                    //
                    // Derive prev_blocks_ids and MC ref from the candidate's own block header.
                    // This allows validating candidates whose parent is notarized but not yet
                    // committed, bypassing the committed prev_block_ids assumption.
                    let real_block = Block::construct_from_bytes(&candidate.data)?;
                    let info = real_block.read_info()?;

                    if info.shard() != &shard {
                        fail!(
                            "on_candidate [simplex-native]: shard mismatch: \
                            expected {shard}, got {}",
                            info.shard()
                        );
                    }

                    let candidate_block_id = BlockIdExt::with_params(
                        info.shard().clone(),
                        info.seq_no(),
                        root_hash.clone(),
                        get_hash(&candidate.data),
                    );

                    // MC fork prevention (C++ block-validator.cpp, commit 9aac62b8):
                    // Wait until the accepted MC head reaches the candidate parent, then
                    // reject stale branches if we have already moved past that parent.
                    if is_masterchain {
                        let prev_ids = info.read_prev_ids()?;
                        if let Some(candidate_parent_block_id) = prev_ids.first() {
                            wait_for_mc_validation_parent(
                                accepted_mc_block_rx.clone(),
                                &candidate_block_id,
                                candidate_parent_block_id,
                            )
                            .await?;
                        }
                    }

                    // Obsolete candidate guard (parity with legacy path)
                    let last_applied_block_opt = if general_session_info.shard.is_masterchain() {
                        let mc = engine.load_last_applied_mc_state().await?;
                        Some(mc.block_id().clone())
                    } else {
                        let mc = engine.load_last_applied_mc_state().await?;
                        mc.shard_state_extra()?
                            .shards()
                            .find_shard(&general_session_info.shard)?
                            .map(|x| x.block_id().clone())
                    };
                    if let Some(last_applied_block) = &last_applied_block_opt {
                        if last_applied_block.seq_no >= candidate_block_id.seq_no {
                            fail!(
                                "Attempt to validate obsolete candidate block \
                                {candidate_block_id}, actual last shard block {last_applied_block}"
                            );
                        }
                    }

                    let candidate = super::BlockCandidate {
                        block_id: candidate_block_id.clone(),
                        data: candidate.data.clone(),
                        collated_file_hash: candidate.collated_file_hash.clone(),
                        collated_data: candidate.collated_data.clone(),
                        created_by: candidate.created_by.clone(),
                    };

                    log::info!(
                        target: "validator",
                        "({next_block_descr}): on_candidate [simplex-native]: \
                         validating {} via candidate-native path",
                        candidate_block_id
                    );

                    let validation_completion_time = run_validate_query_any_candidate(
                        candidate.clone(),
                        engine.clone(),
                        state_resolver_cache.clone(),
                        is_simplex,
                    )
                    .await?;

                    // Post-validation: broadcast + save (shared with legacy path)
                    Self::post_validation_actions(
                        &next_block_descr,
                        &source_info,
                        is_masterchain,
                        &candidate,
                        &source,
                        cc_seqno,
                        &validator_set,
                        &engine,
                        &session_id,
                        &last_validation_time,
                        validation_completion_time,
                    )
                    .await
                } else {
                    // ---- Legacy catchain / validator-session path ----
                    if round != expected_current_round {
                        fail!(
                            "on_candidate: round {} != expected_current_round {}",
                            round,
                            expected_current_round
                        );
                    }

                    prev_block_ids.ensure_next_block_new(
                        &candidate.block_id.root_hash,
                        &candidate.block_id.file_hash,
                    )?;

                    if prev_block_ids.get_prevs().len() == 1 {
                        if let Some(prev_block_id) = prev_block_ids.get_prev(0) {
                            if prev_block_id.shard() == &shard
                                && prev_block_id.seq_no != candidate.block_id.seq_no - 1
                            {
                                fail!(
                                    "on_candidate: next seqno mismatch: {} != {}",
                                    prev_block_id.seq_no,
                                    candidate.block_id.seq_no - 1
                                );
                            }
                        }
                    }

                    let mc_block_id = mc_block_id_opt
                        .ok_or_else(|| error!("Min masterchain block id missing"))?;

                    let mc = engine.load_last_applied_mc_state().await?;

                    let last_applied_block_opt = if general_session_info.shard.is_masterchain() {
                        Some(mc.block_id().clone())
                    } else {
                        mc.shard_state_extra()?
                            .shards()
                            .find_shard(&general_session_info.shard)?
                            .map(|x| x.block_id().clone())
                    };

                    if let Some(last_applied_block) = last_applied_block_opt {
                        if last_applied_block.seq_no >= candidate.block_id.seq_no {
                            fail!(
                                "Attempting to validate obsolete candidate block {}, \
                                 actual last shard block {}",
                                candidate.block_id,
                                last_applied_block
                            );
                        }
                    }

                    let validation_completion_time = run_validate_query(
                        shard,
                        min_ts,
                        mc_block_id,
                        &prev_block_ids,
                        candidate.clone(),
                        validator_set.clone(),
                        engine.clone(),
                        is_simplex,
                    )
                    .await?;

                    Self::post_validation_actions(
                        &next_block_descr,
                        &source_info,
                        is_masterchain,
                        &candidate,
                        &source,
                        cc_seqno,
                        &validator_set,
                        &engine,
                        &session_id,
                        &last_validation_time,
                        validation_completion_time,
                    )
                    .await
                }
            })
            .await;

            // Convert timeout to a validation failure so Simplex retry machinery
            // clears pending_approve and reschedules.
            let validation_result: Result<SystemTime> = match validation_result {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    metrics::counter!("simplex_validation_timeout").increment(1);
                    log::warn!(
                        target: "validator",
                        "({next_block_descr}): ValidatorGroup::on_candidate: {candidate_id}, \
                         validation timed out after {deadline:?}"
                    );
                    Err(error!("validation timed out after {:?}", deadline))
                }
            };

            let validation_result_message = match &validation_result {
                Ok(completion_time) => {
                    format!("Validation successful: finished at {:?}", completion_time)
                }
                Err(e) => format!("Validation failed with verdict `{}`", e),
            };

            log::info!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_candidate: {candidate_id}, {validation_result_message}"
            );

            callback(validation_result);

            log::trace!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_candidate: {candidate_id}, callback called"
            );
        });

        if WAIT_FOR_VALIDATION {
            let _ = validation_task.await;
        }
    }

    /// Shared post-validation actions: broadcast, save candidate, update timing.
    #[allow(clippy::too_many_arguments)]
    async fn post_validation_actions(
        next_block_descr: &str,
        source_info: &validator_session::BlockSourceInfo,
        is_masterchain: bool,
        candidate: &super::BlockCandidate,
        source: &PublicKey,
        cc_seqno: u32,
        validator_set: &ValidatorSet,
        engine: &Arc<dyn EngineOperations>,
        session_id: &SessionId,
        last_validation_time: &AtomicU64,
        validation_completion_time: SystemTime,
    ) -> Result<SystemTime> {
        if need_send_candidate_broadcast(source_info, is_masterchain) {
            match read_single_root_boc(&candidate.data) {
                Ok(block_root) => {
                    let validator_set_hash =
                        ValidatorSet::calc_subset_hash_short(validator_set.list(), cc_seqno)
                            .unwrap_or(0);

                    if let Err(e) = engine
                        .send_block_candidate_broadcast(
                            &candidate.block_id,
                            cc_seqno,
                            validator_set_hash,
                            &block_root,
                        )
                        .await
                    {
                        log::warn!(
                            target: "validator",
                            "({next_block_descr}): Failed to send block candidate broadcast \
                             after validation: {}",
                            e
                        );
                    } else {
                        log::debug!(
                            target: "validator",
                            "({next_block_descr}): Sent block candidate broadcast after validation"
                        );
                    }
                }
                Err(e) => {
                    log::warn!(
                        target: "validator",
                        "({next_block_descr}): Failed to parse block root for broadcast: {}",
                        e
                    );
                }
            }
        }

        let vb_candidate = validator_query_candidate_to_validator_block_candidate(
            source.clone(),
            candidate.clone(),
        );

        engine.save_block_candidate(session_id, vb_candidate)?;

        last_validation_time.fetch_max(
            validation_completion_time
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );

        Ok(validation_completion_time)
    }

    // Accept_block
    //self.accept_block_candidate (round, source, root_hash, file_hash, data, signatures, approve_signatures);
    //signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    //approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>)
    #[allow(clippy::too_many_arguments)]
    // Accept_block
    //self.accept_block_candidate (round, source, root_hash, file_hash, data, signatures, approve_signatures);
    #[allow(clippy::too_many_arguments)]
    pub async fn on_block_committed(
        &self,
        round: u32,
        source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_sig_set: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        let is_simplex_group = self
            .group_impl
            .execute_sync(|group_impl| group_impl.consensus_type == ConsensusType::Simplex)
            .await;
        assert!(
            !is_simplex_group,
            "ValidatorGroup::on_block_committed must not be called for simplex sessions"
        );

        let next_block_descr = self.get_next_block_descr(Some(&root_hash)).await;

        let data_vec = data.data().to_vec();
        let we_generated = source.id() == self.local_key.id();

        log::info!(
            target: "validator",
            "({next_block_descr}): ValidatorGroup::on_block_committed: source {}, data size = {}, {}" ,
            source.id(),
            data_vec.len(),
            self.info_round(round).await
        );

        let prev_block_history = self
            .group_impl
            .execute_sync(|group_impl| {
                let is_block_ours = source.id() == &group_impl.local_id;
                const IS_BLOCK_SKIPPED: bool = false;
                group_impl.finish_round(round, is_block_ours, IS_BLOCK_SKIPPED);

                group_impl.prev_block_ids.clone()
            })
            .await;

        if let Err(e) = prev_block_history.ensure_next_block_new(&root_hash, &file_hash) {
            log::error!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_block_committed: source {}, `{e}`, {}",
                source.id(),
                self.info_round(round).await
            );
            return;
        }
        let next_block_id = prev_block_history.get_next_block_id(&root_hash, &file_hash);

        log::info!(
            target: "validator",
            "({next_block_descr}): ValidatorGroup::on_block_committed: source {}, id {}, data size = {}, {}",
            source.id(),
            next_block_id,
            data_vec.len(),
            self.info_round(round).await
        );

        let result = run_accept_block_query(
            next_block_id.clone(),
            if !data_vec.is_empty() { Some(data_vec) } else { None },
            prev_block_history.get_prevs().to_vec(),
            self.validator_set.clone(),
            signatures,
            approve_sig_set,
            we_generated,
            self.engine.clone(),
        )
        .await;

        let (full_result, new_prevs, accepted_mc_seqno, accepted_mc_block_id) = self
            .group_impl
            .execute_sync(|group_impl| {
                let full_result = match result {
                    Ok(()) => {
                        if !group_impl.prev_block_ids.same_prevs(&prev_block_history) {
                            Err(error!("Sync error: two requests at a time, prevs have changed!!!"))
                        } else {
                            // Block verified and accepted: transition Sync -> Active.
                            // Deferred to this point (after ensure_next_block_new +
                            // run_accept_block_query) so stale/duplicate commits don't
                            // produce false-positive activation.
                            if group_impl.status == ValidatorGroupStatus::Sync {
                                group_impl.status = ValidatorGroupStatus::Active;
                                let cl = if group_impl.consensus_type == ConsensusType::Simplex {
                                    "simplex"
                                } else {
                                    "catchain"
                                };
                                metrics::counter!(
                                    "ton_node_validator_session_activated_total",
                                    "consensus" => cl
                                )
                                .increment(1);
                                log::info!(target: "validator",
                                    "SESSION_LIFECYCLE: transition shard={} cc_seqno={} \
                                     session_id={:x} sync -> active \
                                     (first committed block accepted)",
                                    group_impl.shard,
                                    group_impl.cc_seqno,
                                    group_impl.session_id);
                            }
                            Ok(())
                        }
                    }
                    err => err, // TODO: retry block commit
                };

                if full_result.is_ok() {
                    sync_last_accepted_mc_head_from_block(group_impl, &next_block_id);
                    group_impl.prev_block_ids.update_prev(vec![next_block_id]);
                }

                (
                    full_result,
                    group_impl.prev_block_ids.display_prevs(),
                    group_impl.last_accepted_mc_seqno,
                    group_impl.last_accepted_mc_block_id.clone(),
                )
            })
            .await;
        self.publish_accepted_mc_seqno(accepted_mc_seqno);
        self.publish_accepted_mc_head(accepted_mc_block_id);

        match full_result {
            Ok(()) => log::info!(
                target: "validator",
                "({}): ValidatorGroup::on_block_committed: success!, source {}, {}, new prevs {}",
                next_block_descr,
                source.id(),
                self.info_round(round).await,
                new_prevs
            ),
            Err(err) => log::error!(
                target: "validator",
                "({}): ValidatorGroup::on_block_committed: error!, source {}, \
                error message: `{}`, {}, new prevs {}",
                next_block_descr,
                source.id(),
                err,
                self.info_round(round).await,
                new_prevs
            ),
        }
    }

    /// Out-of-order finalized block delivery.
    ///
    /// Called immediately when a finalization certificate is observed,
    /// regardless of whether predecessors have been committed.
    /// `block_id` carries the full identity (shard, seqno, root_hash, file_hash)
    /// so we don't rely on sequential `prev_block_ids` tracking.
    ///
    /// The engine's `apply_block` is dependency-driven and will recursively
    /// fetch predecessors, so out-of-order acceptance is safe at that layer.
    #[allow(clippy::too_many_arguments)]
    pub async fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        round: u32,
        source: PublicKey,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_sig_set: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        let is_simplex_group = self
            .group_impl
            .execute_sync(|group_impl| group_impl.consensus_type == ConsensusType::Simplex)
            .await;
        if !is_simplex_group {
            log::error!(
                target: "validator",
                "ValidatorGroup::on_block_finalized: unexpected callback for non-simplex session; \
                ignoring (block_id={block_id}, source={}, round={round})",
                source.id()
            );
            return;
        }

        // Important: do not block ValidationAction queue on out-of-order acceptance.
        // This path can involve downloads/apply and must run in detached task.
        let engine = self.engine.clone();
        let state_resolver_cache = self.state_resolver_cache.clone();
        let validator_set = self.validator_set.clone();
        let group_impl = self.group_impl.clone();
        let accepted_mc_seqno_tx = self.accepted_mc_seqno_tx.clone();
        let accepted_mc_block_tx = self.accepted_mc_block_tx.clone();
        let local_key = self.local_key.clone();
        let source_id = source.id().clone();
        let data_vec = data.data().to_vec();
        let data_opt = if data_vec.is_empty() { None } else { Some(data_vec) };
        let we_generated = source.id() == local_key.id();
        let block_seqno = block_id.seq_no;
        let (send_block_broadcast, last_notified_mc_finalized_seqno) = self
            .group_impl
            .execute_sync(|group_impl| {
                let suppress = should_suppress_stale_finalized_rebroadcast(
                    group_impl.last_notified_mc_finalized_seqno,
                    block_seqno,
                );
                (we_generated && !suppress, group_impl.last_notified_mc_finalized_seqno)
            })
            .await;
        if we_generated
            && !send_block_broadcast
            && should_suppress_stale_finalized_rebroadcast(
                last_notified_mc_finalized_seqno,
                block_seqno,
            )
        {
            log::debug!(
                target: "validator",
                "ValidatorGroup::on_block_finalized: suppressed stale rebroadcast for {} \
                 (block_seqno={}, last_notified_mc_finalized_seqno={})",
                block_id,
                block_seqno,
                last_notified_mc_finalized_seqno.unwrap_or(0),
            );
        }
        metrics::counter!("ton_node_validator_finalized_received_total", "consensus" => "simplex")
            .increment(1);

        // Activate session on first finalized-block receipt because simplex is
        // finalized-driven and does not use on_block_committed callbacks.
        self.group_impl
            .execute_sync(|group_impl| {
                if group_impl.status == ValidatorGroupStatus::Sync {
                    group_impl.status = ValidatorGroupStatus::Active;
                    let cl = if group_impl.consensus_type == ConsensusType::Simplex {
                        "simplex"
                    } else {
                        "catchain"
                    };
                    metrics::counter!(
                        "ton_node_validator_session_activated_total",
                        "consensus" => cl
                    )
                    .increment(1);
                    log::info!(
                        target: "validator",
                        "SESSION_LIFECYCLE: transition shard={} cc_seqno={} \
                         session_id={:x} sync -> active \
                         (first finalized block received)",
                        group_impl.shard,
                        group_impl.cc_seqno,
                        group_impl.session_id
                    );
                }
            })
            .await;

        log::info!(
            target: "validator",
            "ValidatorGroup::on_block_finalized: scheduling async accept for \
            block_id={block_id} source={source_id} round={round}"
        );

        tokio::spawn(async move {
            let (accept_data, prevs) =
                match Self::resolve_prev_for_finalized_block(engine.clone(), &block_id, data_opt)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!(
                            target: "validator",
                            "ValidatorGroup::on_block_finalized:
                            failed to resolve prev for {block_id}: {e}"
                        );
                        return;
                    }
                };

            let result = run_accept_block_query(
                block_id.clone(),
                accept_data,
                prevs,
                validator_set,
                signatures,
                approve_sig_set,
                send_block_broadcast,
                engine,
            )
            .await;

            match result {
                Ok(()) => {
                    log::info!(
                        target: "validator",
                        "ValidatorGroup::on_block_finalized: \
                        accepted block_id={block_id} source={source_id} round={round}"
                    );

                    // Prune only after apply, so the finalized block's state is in
                    // the DB and can serve as the engine anchor for subsequent blocks.
                    state_resolver_cache.lock().await.prune_finalized(&block_id);

                    let (accepted_mc_seqno, accepted_mc_block_id) = group_impl
                        .execute_sync(|group_impl| {
                            sync_last_accepted_mc_head_from_block(group_impl, &block_id);
                            (
                                group_impl.last_accepted_mc_seqno,
                                group_impl.last_accepted_mc_block_id.clone(),
                            )
                        })
                        .await;
                    accepted_mc_seqno_tx.send_replace(accepted_mc_seqno);
                    accepted_mc_block_tx.send_replace(accepted_mc_block_id);
                }
                Err(err) => {
                    log::error!(
                        target: "validator",
                        "ValidatorGroup::on_block_finalized: accept failed for \
                        block_id={block_id} source={source_id} round={round}: {err}"
                    );
                }
            }
        });
    }

    fn extract_prev_ids_from_block_data(
        block_id: &BlockIdExt,
        data: Vec<u8>,
    ) -> Result<Vec<BlockIdExt>> {
        let block = crate::block::BlockStuff::deserialize_block(block_id.clone(), Arc::new(data))?;
        Self::extract_prev_ids_from_block(&block)
    }

    fn extract_prev_ids_from_block(block: &crate::block::BlockStuff) -> Result<Vec<BlockIdExt>> {
        let (prev1, prev2) = block.construct_prev_id()?;
        let mut prev = Vec::with_capacity(if prev2.is_some() { 2 } else { 1 });
        prev.push(prev1);
        if let Some(prev2) = prev2 {
            prev.push(prev2);
        }
        Ok(prev)
    }

    async fn resolve_prev_for_finalized_block(
        engine: Arc<dyn EngineOperations>,
        block_id: &BlockIdExt,
        data_opt: Option<Vec<u8>>,
    ) -> Result<(Option<Vec<u8>>, Vec<BlockIdExt>)> {
        if let Some(data) = data_opt {
            match Self::extract_prev_ids_from_block_data(block_id, data.clone()) {
                Ok(prev) => return Ok((Some(data), prev)),
                Err(e) => {
                    log::warn!(
                        target: "validator",
                        "ValidatorGroup::resolve_prev_for_finalized_block: \
                        failed to parse payload for {block_id}: {e}. Falling back to download_block"
                    );
                }
            }
        }

        let (downloaded_block, _proof) = engine.download_block(block_id, Some(10)).await?;
        let prev = Self::extract_prev_ids_from_block(&downloaded_block)?;
        let downloaded_data = downloaded_block.data().to_vec();
        Ok((Some(downloaded_data), prev))
    }

    pub async fn on_block_skipped(&self, round: u32) {
        log::info!(
            target: "validator",
            "({}): ValidatorGroup::on_block_skipped, {}",
            self.get_next_block_descr(None).await,
            self.info_round(round).await
        );

        self.group_impl
            .execute_sync(|group_impl| {
                const IS_BLOCK_OURS: bool = false;
                const IS_BLOCK_SKIPPED: bool = true;
                group_impl.finish_round(round, IS_BLOCK_OURS, IS_BLOCK_SKIPPED);
            })
            .await;
    }

    pub async fn on_get_approved_candidate(
        &self,
        _source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        callback: ValidatorBlockCandidateCallback,
    ) {
        let next_block_descr = self.get_next_block_descr(Some(&root_hash)).await;

        log::info!(
            target: "validator",
            "({}): ValidatorGroup::on_get_approved_candidate rh {:x}, fh {:x}, {}",
            next_block_descr,
            root_hash, file_hash, self.info().await
        );

        let result = self.load_block_candidate(&root_hash).await;
        let result_txt = match &result {
            Ok(_) => "Ok".to_string(),
            Err(err) => format!("Candidate not found: {}", err),
        };
        log::info!(
            target: "validator",
            "({}): ValidatorGroup::on_get_approved_candidate {}, {}",
            next_block_descr,
            result_txt, self.info().await
        );
        callback(result);
    }
}

impl Drop for ValidatorGroup {
    fn drop(&mut self) {
        // Important: does not stop the session -- to avoid database deletion,
        // which otherwise would happen each time the validator-manager crashes.
        log::info!(target: "validator", "ValidatorGroup: dropping session {:x}", self.session_id);
    }
}

#[cfg(test)]
#[path = "tests/test_validator_group.rs"]
mod tests;
