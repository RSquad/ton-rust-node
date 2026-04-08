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
        get_hash, BlockHash, BlockPayloadPtr, CollationParentHint, CommittedBlockProof,
        CommittedBlockProofCallback, ConsensusOptions, ConsensusOverlayManagerPtr, ConsensusType,
        PrivateKey, PublicKey, PublicKeyHash, Session, SessionHolderPtr, SessionId,
        SessionListener, SessionListenerPtr, SessionNode, ValidatorBlockCandidate,
        ValidatorBlockCandidateCallback, ValidatorBlockCandidateDecisionCallback,
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
        validator_utils::{
            validator_query_candidate_to_validator_block_candidate, validatordescr_to_session_node,
            ValidatorListHash,
        },
    },
};
use std::{
    cmp::max,
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

/// When true, non-accelerated consensus (Catchain / Simplex) will block the
/// validator-group message loop while a validation task runs.
/// Set to false (default) to let those tasks run in the background, keeping
/// the group responsive to incoming accept / collation messages.
const WAIT_FOR_VALIDATION: bool = false;

/// When true, non-accelerated consensus (Catchain / Simplex) will block the
/// validator-group message loop while a collation task runs.
const WAIT_FOR_COLLATION: bool = false;

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
    last_accepted_seqno: Option<u32>,
    candidate_parent_seqno: u32,
) -> bool {
    match last_accepted_seqno {
        Some(accepted) => candidate_parent_seqno < accepted,
        None => false,
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum ValidatorGroupStatus {
    Created,
    Countdown { start_at: tokio::time::Instant },
    Sync,
    Active,
    Stopping,
    Stopped,
}

impl Display for ValidatorGroupStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidatorGroupStatus::Created => write!(f, "created"),
            ValidatorGroupStatus::Countdown { start_at: at } => {
                let now = tokio::time::Instant::now();
                write!(f, "cntdwn {}", at.saturating_duration_since(now).as_secs())
            }
            ValidatorGroupStatus::Sync => write!(f, "sync"),
            ValidatorGroupStatus::Active => write!(f, "active"),
            ValidatorGroupStatus::Stopping => write!(f, "stopping"),
            ValidatorGroupStatus::Stopped => write!(f, "stopped"),
        }
    }
}

impl ValidatorGroupStatus {
    pub fn before(&self, of: &ValidatorGroupStatus) -> bool {
        match (&self, of) {
            (ValidatorGroupStatus::Countdown { .. }, ValidatorGroupStatus::Countdown { .. }) => {
                false
            }
            _ => self <= of,
        }
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
    pub fn states(&self) -> &VecDeque<Arc<ShardStateStuff>> {
        &self.states
    }
    pub fn states_with_blocks(&self) -> impl Iterator<Item = (&Arc<ShardStateStuff>, &Block)> {
        self.states.iter().zip(self.blocks.iter())
    }
    pub fn try_get_state(&self, id: &BlockIdExt) -> Option<Arc<ShardStateStuff>> {
        for s in &self.states {
            if s.block_id() == id {
                return Some(s.clone());
            }
        }
        None
    }
    #[cfg(feature = "xp25")]
    pub fn get_prev_for(&self, id: &BlockIdExt) -> Option<Vec<BlockIdExt>> {
        for i in 0..self.states.len() {
            if self.states[i].block_id() == id {
                if let Ok(info) = self.blocks[i].read_info() {
                    return info.read_prev_ids().ok();
                }
            }
        }
        None
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

    /// Highest MC block seqno accepted (committed) in this session.
    /// Used for MC fork prevention: reject candidates building on stale heads.
    last_accepted_mc_seqno: Option<u32>,
}

impl Drop for ValidatorGroupImpl {
    fn drop(&mut self) {
        // Important: does not stop the session -- to avoid database deletion,
        // which otherwise would happen each time the validator-manager crashes.
        log::info!(target: "validator", "ValidatorGroupImpl: dropping session {}", self.info());
    }
}

impl ValidatorGroupImpl {
    // Creates and starts session
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

        self.status = ValidatorGroupStatus::Sync;

        log::info!(target: "validator", "Starting session {}", self.info());

        self.prev_block_ids.update_prev(prev);
        self.min_masterchain_block_id = Some(min_masterchain_block_id.clone());
        self.min_ts = min_ts;
        if self.shard.is_masterchain() {
            // Seed stale-head guard baseline at session start so fork prevention
            // is active before the first local on_block_committed callback.
            self.last_accepted_mc_seqno = Some(min_masterchain_block_id.seq_no);
        }

        // Create session using unified factory
        let session = self.create_consensus_session(g.clone(), session_listener)?;

        let g_clone = g.clone();
        let receiver = g.receiver.lock().unwrap().take().ok_or_else(|| {
            error!("Receiver already taken - session cannot be started multiple times")
        })?;
        rt.clone().spawn(async move {
            process_validation_queue(receiver, g_clone.clone(), rt).await;
        });

        log::trace!(target: "validator", "Started session {}, options {:?}, ref.cnt = {}",
            self.info(), g.consensus_options, Arc::strong_count(&session)
        );

        self.session = Some(session);
        Ok(())
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
    /// Returns None if this is not a simplex session or simplex feature is not enabled
    #[cfg(feature = "simplex")]
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

        let overlay_manager: ConsensusOverlayManagerPtr =
            Arc::new(ConsensusOverlayManagerImpl::new(
                g.engine.validator_network(),
                g.validator_list_id.clone(),
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
                //TODO: check initial seqno for simplex
                let initial_block_seqno = self.prev_block_ids.get_next_seqno().unwrap_or(1);

                ConsensusFactory::create_simplex_based_session(
                    simplex_options,
                    &g.session_id,
                    &g.shard,
                    initial_block_seqno,
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
        consensus_type: ConsensusType,
    ) -> ValidatorGroupImpl {
        log::info!(target: "validator", "Initializing session {:x}, shard {}, consensus_type {}", 
            session_id, shard, consensus_type);

        let prev_block_ids = PrevBlockHistory::with_shard(&shard);
        ValidatorGroupImpl {
            local_id: local_id.clone(),
            is_accelerated_consensus_enabled,
            min_masterchain_block_id: None,
            cc_seqno,
            min_ts: SystemTime::now(),
            status: ValidatorGroupStatus::Created,
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
    //catchain_seqno: u32,
    validator_list_id: ValidatorListHash,

    //shard: ShardIdent,
    engine: Arc<dyn EngineOperations>,
    validator_set: ValidatorSet,
    #[allow(dead_code)]
    allow_unsafe_self_blocks_resync: bool,

    group_impl: Arc<MutexWrapper<ValidatorGroupImpl>>,
    callback: Arc<dyn SessionListener + Send + Sync>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ValidationAction>>>>,

    last_validation_time: Arc<AtomicU64>,
    last_collation_time: Arc<AtomicU64>,
    is_collating: Arc<AtomicBool>,
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
    ) -> Self {
        let consensus_type = consensus_options.consensus_type();
        let is_accelerated = consensus_options.is_accelerated_consensus_enabled();

        let group_impl = ValidatorGroupImpl::new(
            local_key.id(),
            general_session_info.shard.clone(),
            general_session_info.catchain_seqno,
            session_id.clone(),
            is_accelerated,
            consensus_type,
        );
        let id = format!("Val. group {} {:x}", general_session_info.shard, session_id);
        let (listener, receiver) = ValidatorSessionListener::create(
            session_id.clone(),
            general_session_info.shard.clone(),
        );

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
            callback: Arc::new(listener),
            receiver: Arc::new(Mutex::new(Some(receiver))),
            last_validation_time: Arc::new(AtomicU64::new(0)),
            last_collation_time: Arc::new(AtomicU64::new(0)),
            is_collating: Arc::new(AtomicBool::new(false)),
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

    pub fn is_simplex(&self) -> bool {
        matches!(self.consensus_options, ConsensusOptions::Simplex(_))
    }

    /// Notify this session about masterchain finalization.
    ///
    /// For simplex shard sessions, this updates the MC finalization tracking which is
    /// used for empty block generation (finalization recovery).
    ///
    /// This should be called by ValidatorManager when a masterchain block is finalized,
    /// for all shard validator groups.
    ///
    /// # Arguments
    /// * `mc_block_seqno` - The seqno of the finalized masterchain block
    pub async fn notify_mc_finalized(&self, mc_block_seqno: u32) {
        // Only shard sessions need MC finalization notification
        if self.shard().is_masterchain() {
            return;
        }

        //TODO: lock optimization is required
        self.group_impl
            .execute_sync(|group_impl| {
                if let Some(ref session) = group_impl.session {
                    session.notify_mc_finalized(mc_block_seqno);
                }
            })
            .await;
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

    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_status(
        self: Arc<ValidatorGroup>,
        validation_start_status: ValidatorGroupStatus,
        prev: Vec<BlockIdExt>,
        min_masterchain_block_id: BlockIdExt,
        min_ts: SystemTime,
        rt: tokio::runtime::Handle,
    ) -> Result<()> {
        self.set_status(validation_start_status).await?;
        rt.clone().spawn (async move {
            if let ValidatorGroupStatus::Countdown { start_at } = validation_start_status {
                log::trace!(target: "validator", "Session delay started: {}", self.info().await);
                tokio::time::sleep_until(start_at).await;
            }

            let callback = self.make_validator_session_callback();
            self.group_impl.execute_sync(|group_impl|
            {
                if group_impl.status <= ValidatorGroupStatus::Active {
                    if let Err(e) = group_impl.start(
                        callback,
                        prev,
                        min_masterchain_block_id,
                        min_ts,
                        self.clone(),
                        rt
                    )
                    {
                        log::error!(target: "validator", "Cannot start group: {}", e);
                    }
                }
                else {
                    log::trace!(target: "validator", "Session deleted before countdown: {}", group_impl.info());
                }
            }).await;
        });
        Ok(())
    }

    pub async fn stop(
        self: Arc<ValidatorGroup>,
        rt: tokio::runtime::Handle,
        destroy_database: bool,
    ) -> Result<()> {
        self.set_status(ValidatorGroupStatus::Stopping).await?;
        log::info!(target: "validator", "Stopping group: {} (destroy database {})", self.info().await, destroy_database);
        let group_impl = self.group_impl.clone();
        let self_clone = self.clone();
        rt.spawn({
            async move {
                log::debug!(target: "validator", "Stopping group (spawn): {}", self_clone.info().await);
                let session_ptr = group_impl.execute_sync(
                    |group_impl| group_impl.session.clone()).await;
                if let Some(s_ptr) = session_ptr {
                    log::debug!(target: "validator", "Stopping catchain: {}", self_clone.info().await);
                    if destroy_database {
                        s_ptr.destroy(); // Blocking, destroys catchain DB
                    } else {
                        s_ptr.stop();    // Blocking, preserves catchain DB
                    }
                }
                log::debug!(target: "validator", "Group stopped: {}", self_clone.info().await);
                let _ = self_clone.set_status(ValidatorGroupStatus::Stopped).await;
                log::info!(target: "validator", "Status set: {}", self_clone.info().await);
                if destroy_database {
                    let _ = self_clone.destroy_db().await;
                    log::debug!(target: "validator", "Db destroyed: {}", self_clone.info().await);
                }
                else {
                    log::debug!(
                        target: "validator",
                        "Db destroy skipped (destroy_databse option set to false): {}",
                        self_clone.info().await
                    );
                }
            }
        });
        log::debug!(target: "validator", "Stopping group {}, stop spawned", self.info().await);
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

    pub async fn set_status(&self, status: ValidatorGroupStatus) -> Result<()> {
        self.group_impl
            .execute_sync(|group_impl| {
                if group_impl.status.before(&status) {
                    group_impl.status = status;
                    Ok(())
                } else {
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
            CollationParentHint::Explicit(parent_id) => {
                // Simplex explicit-parent collation: parent is locked by consensus layer.
                //
                // Invariant: explicit parent must not be "too old" vs local progress.
                // - last_committed_seqno: from `prev_block_ids` (finalized/committed head)
                // - last_collated_seqno: from `pipeline_context` (accelerated consensus), if any
                //
                // With notarized-parent collation (require_finalized_parent=false), the parent
                // can be *ahead* of last_committed_seqno (notarized but not yet finalized).
                // This is expected and allowed. The guard only rejects parents that are *behind*
                // our current collation head (going backward).
                let last_committed_seqno =
                    prev_block_ids.get_next_seqno().and_then(|n| n.checked_sub(1)).unwrap_or(0);

                let last_collated_seqno =
                    pipeline_context.last_id().map(|id| id.seq_no).unwrap_or(last_committed_seqno);

                let min_allowed_parent_seqno = max(last_committed_seqno, last_collated_seqno);

                if parent_id.shard() != &shard {
                    log::error!(
                        target: "validator",
                        "ValidatorGroup::on_generate_slot: explicit parent shard mismatch \
                        (round={}, request_id={}, expected_shard={}, parent={})",
                        round,
                        request_id,
                        shard,
                        parent_id
                    );
                    self.is_collating.store(false, Ordering::Release);
                    callback(Err(error!("Explicit parent shard mismatch")));
                    return;
                }

                if parent_id.seq_no < min_allowed_parent_seqno {
                    log::error!(
                        target: "validator",
                        "ValidatorGroup::on_generate_slot: explicit parent is too old \
                        (round={}, request_id={}, parent_seqno={}, min_allowed_seqno={}, committed_seqno={}, collated_seqno={}, parent={})",
                        round,
                        request_id,
                        parent_id.seq_no,
                        min_allowed_parent_seqno,
                        last_committed_seqno,
                        last_collated_seqno,
                        parent_id
                    );
                    self.is_collating.store(false, Ordering::Release);
                    callback(Err(error!("Explicit parent is too old")));
                    return;
                }

                log::trace!(
                    target: "validator",
                    "ValidatorGroup::on_generate_slot: using explicit parent \
                    (round={}, request_id={}, parent={})",
                    round,
                    request_id,
                    parent_id
                );

                PrevBlockHistory::with_id(parent_id.clone())
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
        let last_collation_time = self.last_collation_time.clone();
        let is_collating = self.is_collating.clone();
        let max_precollated_blocks = match &self.consensus_options {
            ConsensusOptions::Catchain(opts) => {
                opts.accelerated_consensus_max_precollated_blocks as usize
            }
            ConsensusOptions::Simplex(opts) => opts.max_precollated_blocks as usize,
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
            let (result, result_message) = if is_roundless || round == expected_collation_round {
                let (result, new_state_n_block, result_message) = match mm_block_id {
                    Some(mc) => {
                        match run_collate_query(
                            shard.clone(),
                            min_ts,
                            mc.seq_no,
                            &prev_block_ids,
                            pipeline_context,
                            local_key,
                            validator_set.clone(),
                            engine.clone(),
                        )
                        .await
                        {
                            Ok((candidate, new_state, new_block, block_root)) => {
                                let now = UnixTime::now();
                                last_collation_time.fetch_max(now, Ordering::Relaxed);

                                // Send block candidate broadcast if conditions are met
                                // Note: For SIMPLEX_ROUNDLESS, first_block_round check may not apply
                                if need_send_candidate_broadcast(&source_info, is_masterchain) {
                                    let validator_set_hash = ValidatorSet::calc_subset_hash_short(
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
                            // SIMPLEX_ROUNDLESS: don't advance expected_collation_round
                            // Simplex uses seqno-based tracking, not round-based
                            if !is_roundless {
                                group_impl.expected_collation_round = round + 1;
                            }

                            if group_impl.is_accelerated_consensus_enabled {
                                group_impl.pipeline_context.add(
                                    new_state,
                                    new_block,
                                    max_precollated_blocks,
                                );
                            }
                        })
                        .await;
                }

                (result, result_message)
            } else {
                let result_message = format!(
                    "round {} != expected_collation_round {}. Collation sequence violation",
                    round, expected_collation_round
                );

                (Err(anyhow::anyhow!(result_message.clone())), result_message)
            };

            log::info!(
                target: "validator",
                "({next_block_descr}): ValidatorGroup::on_generate_slot: {round_info}, {result_message}"
            );

            callback(result);

            // Reset the collating flag
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
        let general_session_info = self.general_session_info.clone();
        let session_id = self.session_id.clone();
        let last_validation_time = self.last_validation_time.clone();
        let cc_seqno = self.general_session_info.catchain_seqno;
        let is_masterchain = self.shard.is_masterchain();
        let (
            expected_current_round,
            prev_block_ids,
            mc_block_id_opt,
            min_ts,
            last_accepted_mc_seqno,
        ) = group_impl
            .execute_sync(|group_impl| {
                (
                    group_impl.expected_current_round,
                    group_impl.prev_block_ids.clone(),
                    group_impl.min_masterchain_block_id.clone(),
                    group_impl.min_ts,
                    group_impl.last_accepted_mc_seqno,
                )
            })
            .await;

        let validation_task = tokio::spawn(async move {
            let validation_result = async {
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

                    // MC fork prevention (C++ block-validator.cpp, commit 9aac62b8):
                    // Reject MC candidates whose parent is behind our last accepted MC block.
                    if is_masterchain {
                        let prev_ids = info.read_prev_ids()?;
                        let candidate_parent_seqno =
                            prev_ids.first().map(|id| id.seq_no).unwrap_or(0);
                        if should_reject_stale_mc_candidate(
                            last_accepted_mc_seqno,
                            candidate_parent_seqno,
                        ) {
                            metrics::counter!("simplex_mc_fork_prevention_rejected").increment(1);
                            fail!(
                                "MC fork prevention: candidate {} builds upon seqno {} \
                                 but we already accepted seqno {}",
                                root_hash.to_hex_string(),
                                candidate_parent_seqno,
                                last_accepted_mc_seqno.unwrap_or(0)
                            );
                        }
                    }

                    let candidate_block_id = BlockIdExt::with_params(
                        info.shard().clone(),
                        info.seq_no(),
                        root_hash.clone(),
                        get_hash(&candidate.data),
                    );

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

                    let validation_completion_time =
                        run_validate_query_any_candidate(candidate.clone(), engine.clone()).await?;

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
            }
            .await;

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

        let (full_result, new_prevs) = self
            .group_impl
            .execute_sync(|group_impl| {
                let full_result = match result {
                    Ok(()) => {
                        if !group_impl.prev_block_ids.same_prevs(&prev_block_history) {
                            Err(error!("Sync error: two requests at a time, prevs have changed!!!"))
                        } else {
                            Ok(())
                        }
                    }
                    err => err, // TODO: retry block commit
                };

                let committed_seqno = next_block_id.seq_no;
                group_impl.prev_block_ids.update_prev(vec![next_block_id]);

                if group_impl.shard.is_masterchain() {
                    let prev = group_impl.last_accepted_mc_seqno.unwrap_or(0);
                    group_impl.last_accepted_mc_seqno = Some(prev.max(committed_seqno));
                }

                (full_result, group_impl.prev_block_ids.display_prevs())
            })
            .await;

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

    /// Download committed block proof from full-node.
    ///
    /// Spawns an async task (non-blocking) that downloads the block proof via
    /// EngineOperations, extracts BlockSignaturesVariant, and invokes the callback.
    /// The spawned task does NOT hold up the ValidationAction queue.
    pub async fn on_get_committed_candidate(
        &self,
        block_id: BlockIdExt,
        callback: CommittedBlockProofCallback,
    ) {
        log::info!(
            target: "validator",
            "ValidatorGroup::on_get_committed_candidate: block_id={}, {}",
            block_id, self.info().await
        );

        let engine = self.engine.clone();
        let block_id_clone = block_id.clone();
        tokio::spawn(async move {
            let result = Self::fetch_committed_block_proof(engine.as_ref(), &block_id_clone).await;
            let result_txt = match &result {
                Ok(_) => "Ok".to_string(),
                Err(e) => format!("Err: {}", e),
            };
            log::info!(
                target: "validator",
                "ValidatorGroup::on_get_committed_candidate: result={} for {}",
                result_txt, block_id_clone,
            );
            callback(result);
        });
    }

    async fn fetch_committed_block_proof(
        engine: &dyn crate::engine_traits::EngineOperations,
        block_id: &BlockIdExt,
    ) -> Result<CommittedBlockProof> {
        let is_link = !block_id.shard().is_masterchain();
        let proof = engine.download_block_proof(block_id, is_link, false).await?;

        let signatures = proof.drain_signatures()?;

        if block_id.shard().is_masterchain() {
            match &signatures {
                BlockSignaturesVariant::Simplex(s) if s.is_final => { /* ok */ }
                _ => {
                    fail!("Expected Simplex(is_final=true) for MC block {}", block_id);
                }
            }
        }

        Ok(CommittedBlockProof { block_id: block_id.clone(), signatures })
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
mod tests {
    use super::*;

    #[test]
    fn test_mc_fork_prevention_none_allows() {
        assert!(!should_reject_stale_mc_candidate(None, 0));
        assert!(!should_reject_stale_mc_candidate(None, 100));
    }

    #[test]
    fn test_mc_fork_prevention_equal_allows() {
        assert!(!should_reject_stale_mc_candidate(Some(10), 10));
    }

    #[test]
    fn test_mc_fork_prevention_ahead_allows() {
        assert!(!should_reject_stale_mc_candidate(Some(10), 11));
        assert!(!should_reject_stale_mc_candidate(Some(10), 100));
    }

    #[test]
    fn test_mc_fork_prevention_stale_rejects() {
        assert!(should_reject_stale_mc_candidate(Some(10), 9));
        assert!(should_reject_stale_mc_candidate(Some(10), 0));
        assert!(should_reject_stale_mc_candidate(Some(100), 50));
    }
}
