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
use super::consensus::{
    serialize_tl_boxed_object, CatchainSessionOptions, ConsensusNode, ConsensusOptions, PublicKey,
    RawBuffer,
};
use crate::{
    config::ValidatorManagerConfig,
    engine::Engine,
    engine_traits::EngineOperations,
    shard_state::ShardStateStuff,
    validator::{
        out_msg_queue::OutMsgQueueInfoStuff,
        validator_group::{ValidatorGroup, ValidatorGroupStatus},
        validator_utils::{
            compute_validator_list_id, get_group_members_by_validator_descrs,
            get_masterchain_seqno, try_calc_subset_for_workchain,
            try_calc_subset_for_workchain_standard, validatordescr_to_consensus_node,
            validatorset_to_string, GeneralSessionInfo, PrevBlockHistory, ValidatorListHash,
            ValidatorSubsetInfo,
        },
    },
};
#[cfg(feature = "simplex")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    cmp::{max, min},
    collections::{HashMap, HashSet},
    convert::TryFrom,
    fs,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::timeout;
use ton_api::IntoBoxed;
#[cfg(feature = "simplex")]
use ton_block::SimplexConfig;
use ton_block::{
    error, fail, AcceleratedConsensusConfig, BlockIdExt, CatchainConfig, ConfigParamEnum,
    ConsensusConfig, FutureSplitMerge, McStateExtra, Result, ShardDescr, ShardIdent, UInt256,
    UnixTime, ValidatorDescr, ValidatorSet,
};

#[cfg(feature = "xp25")]
const MC_ACCELERATED_CONSENSUS_ENABLED: bool = true;
#[cfg(not(feature = "xp25"))]
const MC_ACCELERATED_CONSENSUS_ENABLED: bool = false;

// When true, use hardcoded testing constants for simplex instead of ConfigParam 30.
// Set to true during testing period for consistent behavior across all nodes.
#[cfg(feature = "simplex")]
const SIMPLEX_USE_TESTING_CONSTANTS: bool = true;

// Magic tag for accelerated consensus session ID differentiation
const ACCELERATED_CONSENSUS_MAGIC_TAG: u32 = 0xACCE1E8A;

#[derive(Clone)]
pub struct SessionsOptions {
    pub mc_options: CatchainSessionOptions,
    pub mc_hash: UInt256,
    pub shard_options: CatchainSessionOptions,
    pub shard_hash: UInt256,
}

impl SessionsOptions {
    pub fn get_session_options(&self, shard_id: &ShardIdent) -> &CatchainSessionOptions {
        if shard_id.is_masterchain() {
            &self.mc_options
        } else {
            &self.shard_options
        }
    }
}

fn get_session_id_serialize(
    session_info: Arc<GeneralSessionInfo>,
    vals: &[ValidatorDescr],
    new_catchain_ids: bool,
) -> RawBuffer {
    let mut members = Vec::new();
    get_group_members_by_validator_descrs(vals, &mut members);

    if !new_catchain_ids {
        unimplemented!("Old catchain ids format is not supported")
    } else {
        serialize_tl_boxed_object!(&ton_api::ton::validator::group::GroupNew {
            workchain: session_info.shard.workchain_id(),
            shard: session_info.shard.shard_prefix_with_tag() as i64,
            vertical_seqno: session_info.max_vertical_seqno as i32,
            last_key_block_seqno: session_info.key_seqno as i32,
            catchain_seqno: session_info.catchain_seqno as i32,
            config_hash: session_info.opts_hash.clone(),
            members
        }
        .into_boxed())
    }
}

/// serialize data and calc sha256
fn get_session_id(
    session_info: Arc<GeneralSessionInfo>,
    val_set: &[ValidatorDescr],
    new_catchain_ids: bool,
    accelerated_consensus_enabled: bool,
) -> UInt256 {
    let mut serialized = get_session_id_serialize(session_info, val_set, new_catchain_ids);
    if accelerated_consensus_enabled {
        serialized.extend_from_slice(&ACCELERATED_CONSENSUS_MAGIC_TAG.to_le_bytes());
    }
    UInt256::calc_file_hash(&serialized)
}

fn compute_session_unsafe_serialized(session_id: &UInt256, rotate_id: u32) -> Vec<u8> {
    let mut unsafe_id_serialized: Vec<u8> = session_id.as_slice().to_vec();
    let mut rotate_id_serialized: Vec<u8> = rotate_id.to_le_bytes().to_vec();
    unsafe_id_serialized.append(&mut rotate_id_serialized);
    unsafe_id_serialized
}

/// Computes session_id and if unsafe rotation is taking place,
/// replaces session_id with unsafe rotation session id.
fn get_session_unsafe_id(
    session_info: Arc<GeneralSessionInfo>,
    val_set: &[ValidatorDescr],
    new_catchain_ids: bool,
    prev_block_opt: Option<u32>,
    vm_config: &ValidatorManagerConfig,
    accelerated_consensus_enabled: bool,
) -> UInt256 {
    let session_id = get_session_id(
        session_info.clone(),
        val_set,
        new_catchain_ids,
        accelerated_consensus_enabled,
    );

    if session_info.shard.is_masterchain() {
        if let Some(rotate_id) =
            vm_config.check_unsafe_catchain_rotation(prev_block_opt, session_info.catchain_seqno)
        {
            let unsafe_serialized = compute_session_unsafe_serialized(&session_id, rotate_id);
            let unsafe_id = UInt256::calc_file_hash(unsafe_serialized.as_slice());

            log::warn!(
                target: "validator",
                "Unsafe master session rotation: session {} at block={:?}, cc={} -> rotate_id={}, new session {}",
                session_id.to_hex_string(),
                prev_block_opt,
                session_info.catchain_seqno,
                rotate_id,
                unsafe_id.to_hex_string()
            );
            return unsafe_id;
        }
    }
    session_id
}

fn validator_session_options_serialize(opts: &CatchainSessionOptions) -> RawBuffer {
    match opts.proto_version {
        0 => {
            if !opts.new_catchain_ids {
                serialize_tl_boxed_object!(&ton_api::ton::validator_session::config::Config {
                    catchain_idle_timeout: opts.catchain_idle_timeout.as_secs_f64().into(),
                    catchain_max_deps: i32::try_from(opts.catchain_max_deps).unwrap(),
                    round_candidates: i32::try_from(opts.round_candidates).unwrap(),
                    next_candidate_delay: opts.next_candidate_delay.as_secs_f64().into(),
                    round_attempt_duration: i32::try_from(opts.round_attempt_duration.as_secs())
                        .unwrap(),
                    max_round_attempts: i32::try_from(opts.max_round_attempts).unwrap(),
                    max_block_size: i32::try_from(opts.max_block_size).unwrap(),
                    max_collated_data_size: i32::try_from(opts.max_collated_data_size).unwrap()
                }
                .into_boxed())
            } else {
                serialize_tl_boxed_object!(&ton_api::ton::validator_session::config::ConfigNew {
                    catchain_idle_timeout: opts.catchain_idle_timeout.as_secs_f64().into(),
                    catchain_max_deps: i32::try_from(opts.catchain_max_deps).unwrap(),
                    round_candidates: i32::try_from(opts.round_candidates).unwrap(),
                    next_candidate_delay: opts.next_candidate_delay.as_secs_f64().into(),
                    round_attempt_duration: i32::try_from(opts.round_attempt_duration.as_secs())
                        .unwrap(),
                    max_round_attempts: i32::try_from(opts.max_round_attempts).unwrap(),
                    max_block_size: i32::try_from(opts.max_block_size).unwrap(),
                    max_collated_data_size: i32::try_from(opts.max_collated_data_size).unwrap(),
                    new_catchain_ids: ton_api::ton::Bool::from(opts.new_catchain_ids)
                }
                .into_boxed())
            }
        }
        1 => {
            serialize_tl_boxed_object!(&ton_api::ton::validator_session::config::ConfigVersioned {
                catchain_idle_timeout: opts.catchain_idle_timeout.as_secs_f64().into(),
                catchain_max_deps: i32::try_from(opts.catchain_max_deps).unwrap(),
                round_candidates: i32::try_from(opts.round_candidates).unwrap(),
                next_candidate_delay: opts.next_candidate_delay.as_secs_f64().into(),
                round_attempt_duration: i32::try_from(opts.round_attempt_duration.as_secs())
                    .unwrap(),
                max_round_attempts: i32::try_from(opts.max_round_attempts).unwrap(),
                max_block_size: i32::try_from(opts.max_block_size).unwrap(),
                max_collated_data_size: i32::try_from(opts.max_collated_data_size).unwrap(),
                version: opts.proto_version as i32
            }
            .into_boxed())
        }
        _ => {
            serialize_tl_boxed_object!(
                &ton_api::ton::validator_session::config::ConfigVersionedV2 {
                    catchain_opts:
                        ton_api::ton::validator_session::catchainoptions::CatchainOptions {
                            idle_timeout: opts.catchain_idle_timeout.as_secs_f64().into(),
                            max_deps: i32::try_from(opts.catchain_max_deps).unwrap(),
                            max_block_size: i32::try_from(opts.catchain_max_serialized_block_size)
                                .unwrap(),
                            block_hash_covers_data: ton_api::ton::Bool::from(
                                opts.catchain_block_hash_covers_data
                            ),
                            max_block_height_ceoff: i32::try_from(
                                opts.catchain_max_block_height_coeff
                            )
                            .unwrap(),
                            debug_disable_db: ton_api::ton::Bool::from(opts.catchain_disable_db)
                        }
                        .into_boxed(),
                    round_candidates: i32::try_from(opts.round_candidates).unwrap(),
                    next_candidate_delay: opts.next_candidate_delay.as_secs_f64().into(),
                    round_attempt_duration: i32::try_from(opts.round_attempt_duration.as_secs())
                        .unwrap(),
                    max_round_attempts: i32::try_from(opts.max_round_attempts).unwrap(),
                    max_block_size: i32::try_from(opts.max_block_size).unwrap(),
                    max_collated_data_size: i32::try_from(opts.max_collated_data_size).unwrap(),
                    version: opts.proto_version as i32
                }
                .into_boxed()
            )
        }
    }
}

fn get_validator_session_options_hash(
    mut opts: CatchainSessionOptions,
    last_masterchain_block_seqno: u32,
) -> (UInt256, RawBuffer) {
    const THRESHOLD: u32 = 9407194;
    if last_masterchain_block_seqno >= THRESHOLD {
        opts.proto_version = max(1, opts.proto_version);
    }
    let serialized = validator_session_options_serialize(&opts);
    (UInt256::calc_file_hash(&serialized), serialized)
}

fn get_session_options(
    opts: &ConsensusConfig,
    catchain_config: &CatchainConfig,
    accelerated_consensus_enabled: bool,
    accelerated_consensus_config: &Option<AcceleratedConsensusConfig>,
) -> CatchainSessionOptions {
    let default_opts = CatchainSessionOptions::default();

    let mut result = CatchainSessionOptions {
        catchain_idle_timeout: Duration::from_millis(opts.consensus_timeout_ms.into()),
        catchain_max_deps: opts.catchain_max_deps,
        catchain_max_serialized_block_size: default_opts.catchain_max_serialized_block_size, // always set to default
        catchain_max_block_height_coeff: {
            let catchain_lifetime =
                max(catchain_config.mc_catchain_lifetime, catchain_config.shard_catchain_lifetime);
            (opts.catchain_max_blocks_coeff * catchain_lifetime) as u64
        },
        proto_version: opts.proto_version as u32,
        catchain_block_hash_covers_data: {
            const BLOCK_HASH_COVERS_DATA_FROM_VERSION: u32 = 2;
            if opts.proto_version as u32 >= BLOCK_HASH_COVERS_DATA_FROM_VERSION {
                true
            } else {
                default_opts.catchain_block_hash_covers_data
            }
        },
        catchain_disable_db: default_opts.catchain_disable_db,
        catchain_skip_processed_blocks: false, // Debugging option, not found in consensus config
        round_candidates: opts.round_candidates,
        next_candidate_delay: Duration::from_millis(opts.next_candidate_delay_ms.into()),
        round_attempt_duration: Duration::from_secs(opts.attempt_duration.into()),
        max_round_attempts: opts.fast_attempts,
        max_block_size: opts.max_block_bytes,
        max_collated_data_size: opts.max_collated_bytes,
        new_catchain_ids: opts.new_catchain_ids,
        skip_single_node_session_validations: false, // This should be set to true for single-node sessions
        catchain_receiver_max_neighbours_count: default_opts.catchain_receiver_max_neighbours_count,
        catchain_receiver_neighbours_sync_min_period: default_opts
            .catchain_receiver_neighbours_sync_min_period,
        catchain_receiver_neighbours_sync_max_period: default_opts
            .catchain_receiver_neighbours_sync_max_period,
        catchain_receiver_max_sources_sync_attempts: default_opts
            .catchain_receiver_max_sources_sync_attempts,
        catchain_receiver_neighbours_rotate_min_period: default_opts
            .catchain_receiver_neighbours_rotate_min_period,
        catchain_receiver_neighbours_rotate_max_period: default_opts
            .catchain_receiver_neighbours_rotate_max_period,
        accelerated_consensus_enabled,
        accelerated_consensus_collation_retry_timeout: default_opts
            .accelerated_consensus_collation_retry_timeout,
        accelerated_consensus_skip_rounds_count_for_collator_rotation: default_opts
            .accelerated_consensus_skip_rounds_count_for_collator_rotation,
        accelerated_consensus_max_precollated_blocks: default_opts
            .accelerated_consensus_max_precollated_blocks,
        validation_retry_attempts: default_opts.validation_retry_attempts,
        validation_retry_timeout: default_opts.validation_retry_timeout,
        block_candidate_sending_retry_timeout: default_opts.block_candidate_sending_retry_timeout,
        block_candidate_sending_retry_attempts: default_opts.block_candidate_sending_retry_attempts,
        use_callback_thread: default_opts.use_callback_thread,
    };

    if accelerated_consensus_enabled {
        if let Some(accelerated_consensus_config) = accelerated_consensus_config {
            result.accelerated_consensus_collation_retry_timeout = Duration::from_millis(
                accelerated_consensus_config.failed_collation_retry_timeout_ms.into(),
            );
            result.accelerated_consensus_skip_rounds_count_for_collator_rotation =
                accelerated_consensus_config.skip_rounds_count_for_collator_rotation;
            result.accelerated_consensus_max_precollated_blocks =
                accelerated_consensus_config.max_precollated_blocks;
        }
    }

    result
}

async fn clear_catchains_cache(path_str: String) -> Result<()> {
    log::info!(target: "validator_manager", "Clearing catchains cache...");
    let removed = tokio::task::spawn_blocking(move || -> Result<u32> {
        let entries = fs::read_dir(path_str)?;
        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                if let Err(err) = fs::remove_dir_all(path.clone()) {
                    log::warn!("Error clearing catchains cache {}: {}", path.display(), err);
                } else {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    })
    .await??;
    log::info!(target: "validator_manager", "Cleared catchains cache, removed {} entries", removed);
    Ok(())
}

#[repr(u8)]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum ValidationStatus {
    Disabled = 0,
    Waiting = 1,
    Countdown = 2,
    Active = 3,
}

impl ValidationStatus {
    fn allows_validate(&self) -> bool {
        match self {
            Self::Disabled | Self::Waiting => false,
            Self::Countdown | Self::Active => true,
        }
    }
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ValidationStatus::Waiting,
            2 => ValidationStatus::Countdown,
            3 => ValidationStatus::Active,
            _ => ValidationStatus::Disabled,
        }
    }
}

#[derive(Default)]
struct ValidatorListStatus {
    known_lists: HashMap<ValidatorListHash, PublicKey>,
    curr: Option<ValidatorListHash>,
    next: Option<ValidatorListHash>,
    curr_utime_since: Option<u32>,
    next_utime_since: Option<u32>,
}

impl ValidatorListStatus {
    fn add_list(&mut self, list_id: ValidatorListHash, key: PublicKey) {
        self.known_lists.insert(list_id, key);
    }

    fn contains_list(&self, list_id: &ValidatorListHash) -> bool {
        self.known_lists.contains_key(list_id)
    }

    fn remove_list(&mut self, list_id: &ValidatorListHash) {
        self.known_lists.remove(list_id);
    }

    fn get_list(&self, list_id: &ValidatorListHash) -> Option<PublicKey> {
        return match self.known_lists.get(list_id) {
            None => None,
            Some(ch) => Some(ch.clone()),
        };
    }

    fn get_local_key(&self) -> Option<PublicKey> {
        match &self.curr {
            None => None,
            Some(ch) => self.get_list(ch),
        }
    }

    fn actual_or_coming(&self, list_id: &ValidatorListHash) -> bool {
        match &self.curr {
            Some(curr_id) if list_id == curr_id => return true,
            _ => (),
        };

        match &self.next {
            Some(next_id) => list_id == next_id,
            _ => false,
        }
    }

    fn known_hashes(&self) -> HashSet<ValidatorListHash> {
        return self.known_lists.keys().cloned().collect();
    }
}

fn rotate_all_shards(mc_state_extra: &McStateExtra) -> bool {
    mc_state_extra.validator_info.nx_cc_updated
}

struct ValidatorManagerImpl {
    engine: Arc<dyn EngineOperations>,
    rt: tokio::runtime::Handle,
    validator_sessions: HashMap<UInt256, Arc<ValidatorGroup>>, // Sessions: both actual (started) and future
    validator_list_status: ValidatorListStatus,
    config: ValidatorManagerConfig,
}

impl ValidatorManagerImpl {
    fn create(
        engine: Arc<dyn EngineOperations>,
        rt: tokio::runtime::Handle,
        config: ValidatorManagerConfig,
    ) -> Self {
        engine.set_validation_status(ValidationStatus::Disabled);
        ValidatorManagerImpl {
            engine: engine.clone(),
            rt: rt.clone(),
            validator_sessions: HashMap::default(),
            validator_list_status: ValidatorListStatus::default(),
            config,
        }
    }

    /// find own key in validator subset
    fn find_us(&self, validators: &[ValidatorDescr]) -> Option<PublicKey> {
        if let Some(lk) = self.validator_list_status.get_local_key() {
            let local_keyhash = lk.id().data();
            for val in validators {
                let pkhash = val.compute_node_id_short();
                if pkhash.as_slice() == local_keyhash {
                    //log::info!(target: "validator_manager", "Comparing {} with {}", pkhash, local_keyhash);
                    //log::info!(target: "validator_manager", "({:?})", pk.pub_key().unwrap());
                    //compute public key hash
                    return Some(lk);
                }
            }
        }
        None
    }

    async fn update_single_validator_list(
        &mut self,
        validator_list: &[ValidatorDescr],
        name: &str,
    ) -> Result<Option<ValidatorListHash>> {
        let list_id = match compute_validator_list_id(validator_list, None)? {
            None => return Ok(None),
            Some(l) if self.validator_list_status.contains_list(&l) => return Ok(Some(l)),
            Some(l) => l,
        };

        let nodes_res: Vec<ConsensusNode> = validator_list
            .iter()
            .map(validatordescr_to_consensus_node)
            .collect::<Vec<ConsensusNode>>();

        log::info!(target: "validator_manager", "Updating {} validator list (id {:x}):", name, list_id);
        for x in &nodes_res {
            log::debug!(target: "validator_manager", "pk: {}, pk_id: {}, andl_id: {}",
                hex::encode(x.public_key.pub_key().unwrap()),
                hex::encode(x.public_key.id().data()),
                hex::encode(x.adnl_id.data())
            );
        }

        match self.engine.set_validator_list(list_id.clone(), &nodes_res).await? {
            Some(key) => {
                self.validator_list_status.add_list(list_id.clone(), key.clone());
                log::info!(target: "validator_manager", "Local node: pk_id: {} id: {}",
                    hex::encode(key.pub_key().unwrap()),
                    hex::encode(key.id().data())
                );
                Ok(Some(list_id))
            }
            None => {
                log::info!(target: "validator_manager", "Local node is not a {} validator", name);
                Ok(None)
            }
        }
    }

    async fn update_validator_lists(&mut self, mc_state: &ShardStateStuff) -> Result<bool> {
        let (validator_set, next_validator_set) = match mc_state.state()?.read_custom()? {
            None => return Ok(false),
            Some(state) => (state.config.validator_set()?, state.config.next_validator_set()?),
        };

        self.validator_list_status.curr =
            self.update_single_validator_list(validator_set.list(), "current").await?;
        self.validator_list_status.curr_utime_since = Some(validator_set.utime_since());
        if let Some(id) = self.validator_list_status.curr.as_ref() {
            self.engine.activate_validator_list(id.clone())?;
        }
        self.validator_list_status.next =
            self.update_single_validator_list(next_validator_set.list(), "next").await?;
        self.validator_list_status.next_utime_since = Some(next_validator_set.utime_since());

        metrics::gauge!("ton_node_validator_in_current_set").set(if self
            .validator_list_status
            .curr
            .is_some()
        {
            1
        } else {
            0
        } as f64);
        metrics::gauge!("ton_node_validator_in_next_set").set(if self
            .validator_list_status
            .next
            .is_some()
        {
            1
        } else {
            0
        } as f64);
        Ok(self.validator_list_status.curr.is_some() || self.validator_list_status.next.is_some())
    }

    async fn is_active_shard(&self, shard: &ShardIdent) -> bool {
        for group in self.validator_sessions.values() {
            if group.shard() == shard {
                match group.get_status().await {
                    ValidatorGroupStatus::Sync
                    | ValidatorGroupStatus::Active
                    | ValidatorGroupStatus::Countdown { .. } => return true,
                    _ => (),
                }
            }
        }
        false
    }

    async fn garbage_collect_lists(&mut self) -> Result<()> {
        log::trace!(target: "validator_manager", "Garbage collect lists");
        let mut lists_gc = self.validator_list_status.known_hashes();

        for id in self.validator_sessions.values() {
            lists_gc.remove(&id.get_validator_list_id());
        }

        for id in lists_gc {
            if !self.validator_list_status.actual_or_coming(&id) {
                log::trace!(target: "validator_manager", "Removing validator list: {:x}", id);
                self.validator_list_status.remove_list(&id);
                self.engine.remove_validator_list(id.clone())?;
                log::trace!(target: "validator_manager", "Validator list removed: {:x}", id);
            } else {
                log::trace!(target: "validator_manager", "Validator list is still actual: {:x}", id);
            }
        }
        log::trace!(target: "validator_manager", "Garbage collect lists -- ok");

        Ok(())
    }

    async fn garbage_collect(&mut self) {
        if let Err(e) = self.garbage_collect_lists().await {
            log::error!(target: "validator_manager", "Error while garbage collecting validator lists: `{}`", e);
        }
    }

    /// Notify all shard simplex sessions about masterchain finalization.
    ///
    /// This should be called when a masterchain block is finalized (committed).
    /// For shard simplex sessions, this updates MC finalization tracking which is
    /// used for empty block generation (finalization recovery).
    ///
    /// For catchain sessions and MC sessions, this is a no-op.
    ///
    /// Notifications are spawned in parallel without waiting for completion.
    ///
    /// # Arguments
    /// * `mc_block_seqno` - The seqno of the finalized masterchain block
    fn notify_shard_sessions_mc_finalized(&self, mc_block_seqno: u32) {
        consensus_common::check_execution_time!(5000); // 5ms max

        for (session_id, group) in self.validator_sessions.iter() {
            if group.shard().is_masterchain() || !group.is_simplex() {
                continue;
            }
            log::trace!(
                target: "validator_manager",
                "Notifying session {:x} (shard {}) about MC finalization: seqno={}",
                session_id,
                group.shard(),
                mc_block_seqno
            );
            let group = group.clone();
            let sid = session_id.clone();
            tokio::spawn(async move {
                group.notify_mc_finalized(mc_block_seqno).await;
                log::trace!(
                    target: "validator_manager",
                    "MC finalization notification delivered for session {:x}, seqno={}",
                    sid, mc_block_seqno
                );
            });
        }
    }

    async fn stop_and_remove_sessions(
        &mut self,
        sessions_to_remove: &HashSet<UInt256>,
        destroy_database: bool,
    ) {
        for id in sessions_to_remove.iter() {
            log::trace!(target: "validator_manager", "stop&remove: removing {:x}", id);
            match self.validator_sessions.get(id) {
                None => {
                    log::error!(target: "validator_manager",
                        "Session stopping error: {:x} already removed from hash", id
                    )
                }
                Some(session) => match session.get_status().await {
                    ValidatorGroupStatus::Stopping => {}
                    ValidatorGroupStatus::Stopped => {
                        if let Some(group) = self.validator_sessions.remove(id) {
                            if !self.is_active_shard(group.shard()).await {
                                self.engine.remove_last_validation_time(group.shard());
                                self.engine.remove_last_collation_time(group.shard());
                            }
                        }
                    }
                    _ => {
                        if let Err(e) =
                            session.clone().stop(self.rt.clone(), destroy_database).await
                        {
                            log::error!(target: "validator_manager",
                                    "Could not stop session {:x}: `{}`", id, e);
                            self.validator_sessions.remove(id);
                        }
                    }
                },
            }
        }
    }

    async fn compute_session_options(
        &mut self,
        mc_state_extra: &McStateExtra,
        catchain_config: &CatchainConfig,
    ) -> Result<SessionsOptions> {
        let consensus_config = match mc_state_extra.config.config(29)? {
            Some(ConfigParamEnum::ConfigParam29(consensus_config)) => consensus_config,
            _ => fail!("no CatchainConfig in config_params"),
        };
        let accelerated_consensus_config = self.get_accelerated_consensus_config(mc_state_extra);

        // Compute session options for masterchain (accelerated consensus controlled by constant)
        let mc_accelerated_consensus_enabled = MC_ACCELERATED_CONSENSUS_ENABLED
            && accelerated_consensus_config.as_ref().map(|x| x.enabled).unwrap_or(false);
        let mc_options = get_session_options(
            &consensus_config,
            catchain_config,
            mc_accelerated_consensus_enabled,
            &accelerated_consensus_config,
        );
        let (mc_hash, mc_session_options_serialized) = get_validator_session_options_hash(
            mc_options.clone(),
            mc_state_extra.last_key_block.as_ref().map(|x| x.seq_no).unwrap_or(0),
        );

        // Compute session options for shards (accelerated consensus may be enabled)
        let shard_accelerated_consensus_enabled =
            accelerated_consensus_config.as_ref().map(|x| x.enabled).unwrap_or(false);
        let shard_options = get_session_options(
            &consensus_config,
            catchain_config,
            shard_accelerated_consensus_enabled,
            &accelerated_consensus_config,
        );
        let (shard_hash, shard_session_options_serialized) = get_validator_session_options_hash(
            shard_options.clone(),
            mc_state_extra.last_key_block.as_ref().map(|x| x.seq_no).unwrap_or(0),
        );

        log::trace!(target: "validator_manager", "MC SessionOptions from config.29: {:?}", mc_options);
        log::trace!(
            target: "validator_manager",
            "MC SessionOptions from config.29 serialized: {} hash: {:x}",
            hex::encode(mc_session_options_serialized),
            mc_hash
        );
        log::trace!(target: "validator_manager", "Shard SessionOptions from config.29: {:?}", shard_options);
        log::trace!(
            target: "validator_manager",
            "Shard SessionOptions from config.29 serialized: {} hash: {:x}",
            hex::encode(shard_session_options_serialized),
            shard_hash
        );

        Ok(SessionsOptions { mc_options, mc_hash, shard_options, shard_hash })
    }

    fn get_accelerated_consensus_config(
        &self,
        mc_state_extra: &McStateExtra,
    ) -> Option<AcceleratedConsensusConfig> {
        match mc_state_extra.config.accelerated_consensus_params() {
            Ok(accelerated_config) => Some(accelerated_config),
            _ => None,
        }
    }

    fn is_accelerated_consensus_enabled(&self, mc_state_extra: &McStateExtra) -> bool {
        // Check if disabled in config first
        if self.config.accelerated_consensus_disabled {
            return false;
        }

        if let Some(accelerated_config) = self.get_accelerated_consensus_config(mc_state_extra) {
            return accelerated_config.enabled;
        }

        false
    }

    fn is_accelerated_consensus_enabled_for_shard(
        &self,
        mc_state_extra: &McStateExtra,
        shard_id: &ShardIdent,
    ) -> bool {
        // For masterchain, use the constant to control accelerated consensus
        if shard_id.is_masterchain() {
            if !MC_ACCELERATED_CONSENSUS_ENABLED {
                return false;
            }
        }

        // Use the general accelerated consensus check for shards
        self.is_accelerated_consensus_enabled(mc_state_extra)
    }

    /// Select consensus options based on ConfigParam 30 (NewConsensusConfigAll).
    ///
    /// If simplex feature is enabled and ConfigParam 30 contains a SimplexConfig for
    /// the given shard (mc or shard), returns `ConsensusOptions::Simplex`.
    /// Otherwise, returns `ConsensusOptions::Catchain` with the provided catchain options.
    ///
    /// This follows the C++ pattern in `ValidatorManagerImpl::create_validator_group`:
    /// - Get `new_consensus_config` from masterchain state
    /// - If present, create bridge (simplex/null consensus)
    /// - If absent, create catchain
    #[cfg(feature = "simplex")]
    fn select_consensus_options(
        &self,
        shard: &ShardIdent,
        mc_state: &ShardStateStuff,
        catchain_options: &CatchainSessionOptions,
    ) -> ConsensusOptions {
        use super::consensus::{ConsensusFactory, SimplexSessionOptions};

        // During testing period, use hardcoded constants instead of ConfigParam 30
        if SIMPLEX_USE_TESTING_CONSTANTS {
            let options = ConsensusFactory::create_simplex_options(
                catchain_options.max_block_size as usize,
                catchain_options.max_collated_data_size as usize,
            );
            static LAST_WARN: AtomicU64 = AtomicU64::new(0);
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let last = LAST_WARN.load(Ordering::Relaxed);
            if now >= last + 30
                && LAST_WARN
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                log::warn!(
                    target: "validator_manager",
                    "Simplex TESTING MODE for {}: target_rate={}ms, slots_per_window={}, first_block_timeout={}ms",
                    shard,
                    options.target_rate.as_millis(),
                    options.slots_per_leader_window,
                    options.first_block_timeout.as_millis()
                );
            }
            return ConsensusOptions::Simplex(options);
        }

        // Try to get ConfigParam 30 from masterchain state
        let config_params = match mc_state.config_params() {
            Ok(cfg) => cfg,
            Err(e) => {
                log::trace!(
                    target: "validator_manager",
                    "Could not get config_params from mc_state: {}, using catchain",
                    e
                );
                return ConsensusOptions::Catchain(catchain_options.clone());
            }
        };

        // Get simplex config for mc or shard based on workchain
        let simplex_cfg: Option<SimplexConfig> = if shard.is_masterchain() {
            config_params.get_mc_simplex_config().ok().flatten()
        } else {
            config_params.get_shard_simplex_config().ok().flatten()
        };

        if let Some(cfg) = simplex_cfg {
            log::info!(
                target: "validator_manager",
                "Simplex config found for {}: target_rate={}ms, slots_per_window={}, first_block_timeout={}ms",
                shard,
                cfg.target_rate_ms,
                cfg.slots_per_leader_window,
                cfg.first_block_timeout_ms
            );
            return ConsensusOptions::Simplex(SimplexSessionOptions {
                slots_per_leader_window: cfg.slots_per_leader_window,
                first_block_timeout: Duration::from_millis(cfg.first_block_timeout_ms as u64),
                target_rate: Duration::from_millis(cfg.target_rate_ms as u64),
                // max_block_size and max_collated_data_size come from ConfigParam 29 (via catchain_options)
                max_block_size: catchain_options.max_block_size as usize,
                max_collated_data_size: catchain_options.max_collated_data_size as usize,
                ..Default::default()
            });
        }

        log::trace!(
            target: "validator_manager",
            "No simplex config for {}, using catchain",
            shard
        );
        ConsensusOptions::Catchain(catchain_options.clone())
    }

    #[cfg(not(feature = "simplex"))]
    fn select_consensus_options(
        &self,
        _shard: &ShardIdent,
        _mc_state: &ShardStateStuff,
        catchain_options: &CatchainSessionOptions,
    ) -> ConsensusOptions {
        ConsensusOptions::Catchain(catchain_options.clone())
    }

    async fn update_validation_status(
        &mut self,
        mc_state: &ShardStateStuff,
        mc_state_extra: &McStateExtra,
    ) -> Result<()> {
        match self.engine.validation_status() {
            ValidationStatus::Waiting => {
                let last_masterchain_block = mc_state.block_id();
                if last_masterchain_block.seq_no == 0 || rotate_all_shards(mc_state_extra) {
                    let later_than_hardfork = self.engine.get_last_fork_masterchain_seqno()
                        <= last_masterchain_block.seq_no;

                    if self.engine.check_sync().await? && later_than_hardfork {
                        if last_masterchain_block.seq_no == 0
                            && self.config.no_countdown_for_zerostate
                        {
                            self.engine.set_validation_status(ValidationStatus::Active);
                        } else {
                            self.engine.set_validation_status(ValidationStatus::Countdown);
                        }
                    }
                }
            }
            ValidationStatus::Countdown => {
                for (_, group) in self.validator_sessions.iter() {
                    let status = group.get_status().await;
                    if status == ValidatorGroupStatus::Sync
                        || status == ValidatorGroupStatus::Active
                    {
                        let path_str: String = self.engine.db_root_dir()?.to_owned() + "/catchains";
                        tokio::spawn(async move {
                            if let Err(err) = clear_catchains_cache(path_str).await {
                                log::warn!("Error clearing catchains cache: {}", err);
                            }
                        });
                        self.engine.set_validation_status(ValidationStatus::Active);
                        break;
                    }
                }
            }
            ValidationStatus::Disabled | ValidationStatus::Active => {}
        }
        Ok(())
    }

    async fn disable_validation(&mut self, clear_rotation: bool) -> Result<()> {
        self.engine.set_validation_status(ValidationStatus::Disabled);

        let existing_validator_sessions: HashSet<UInt256> =
            self.validator_sessions.keys().cloned().collect();
        self.stop_and_remove_sessions(&existing_validator_sessions, clear_rotation).await;
        self.garbage_collect().await;
        self.engine.set_will_validate(false);
        if clear_rotation {
            self.engine.clear_last_rotation_block_id()?;
        }
        log::info!(target: "validator_manager", "All sessions were removed, validation disabled");
        Ok(())
    }

    async fn stop_validation(&mut self) {
        if let Err(e) = self.disable_validation(false).await {
            log::error!(target: "validator_manager", "Cannot disable validation: {}", e);
        };
    }

    fn enable_validation(&mut self) {
        self.engine.set_will_validate(true);
        let validation_status = max(self.engine.validation_status(), ValidationStatus::Waiting);
        self.engine.set_validation_status(validation_status);
        log::debug!(target: "validator_manager", "Validation enabled: status {:?}", validation_status);
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_sessions(
        &mut self,
        new_shards: HashMap<ShardIdent, Vec<BlockIdExt>>,
        our_current_shards: &mut HashMap<ShardIdent, ValidatorSet>,
        keyblock_seqno: u32,
        sessions_options: &SessionsOptions,
        gc_validator_sessions: &mut HashSet<UInt256>,
        mc_now: u32,
        mc_state: &ShardStateStuff,
        mc_state_extra: &McStateExtra,
        master_cc_range: &RangeInclusive<u32>,
        last_masterchain_block: &BlockIdExt,
    ) -> Result<()> {
        let validator_list_id = match &self.validator_list_status.curr {
            Some(list_id) => list_id,
            None => return Ok(()),
        };
        let full_validator_set = mc_state_extra.config.validator_set()?;

        let validation_status = self.engine.validation_status();
        let catchain_config = self.read_catchain_config(mc_state)?;
        let group_start_status = if validation_status == ValidationStatus::Countdown {
            let session_lifetime =
                min(catchain_config.mc_catchain_lifetime, catchain_config.shard_catchain_lifetime);
            let start_at =
                tokio::time::Instant::now() + Duration::from_secs((session_lifetime / 2).into());
            ValidatorGroupStatus::Countdown { start_at }
        } else {
            ValidatorGroupStatus::Sync
        };

        let do_unsafe_catchain_rotate = self
            .config
            .check_unsafe_catchain_rotation(
                Some(last_masterchain_block.seq_no),
                mc_state_extra.validator_info.catchain_seqno,
            )
            .is_some();

        log::trace!(target: "validator_manager", "Starting/updating sessions {}",
            if do_unsafe_catchain_rotate {"(unsafe rotate)"} else {""}
        );

        for (ident, prev_blocks) in new_shards {
            let cc_seqno_from_state = if ident.is_masterchain() {
                *master_cc_range.end()
            } else {
                mc_state_extra.shards().calc_shard_cc_seqno(&ident)?
            };

            let cc_seqno = cc_seqno_from_state;

            log::trace!(target: "validator_manager", "Trying to start/update session for shard {}, cc_seqno {}",
                ident, cc_seqno_from_state
            );

            let prev = PrevBlockHistory::with_prevs(&ident, prev_blocks);
            let subset = match try_calc_subset_for_workchain(
                &full_validator_set,
                mc_state,
                &ident,
                cc_seqno,
                prev.get_next_seqno().unwrap_or_default(),
            )? {
                Some(x) => x,
                None => {
                    log::debug!(
                        target: "validator_manager",
                        "Cannot compute validator set for workchain {}: less than {} of {}",
                        ident,
                        catchain_config.shard_validators_num,
                        full_validator_set.list().len()
                    );
                    continue;
                }
            };

            let vsubset =
                ValidatorSet::with_cc_seqno(0, 0, 0, cc_seqno, subset.validators.clone())?;
            let max_vertical_seqno = self.engine.hardforks().len() as u32;

            // Select appropriate options hash based on shard type
            let current_opts_hash = if ident.is_masterchain() {
                &sessions_options.mc_hash
            } else {
                &sessions_options.shard_hash
            };

            let general_session_info = Arc::new(GeneralSessionInfo {
                shard: ident.clone(),
                opts_hash: current_opts_hash.clone(),
                catchain_seqno: cc_seqno,
                key_seqno: keyblock_seqno,
                max_vertical_seqno: max_vertical_seqno,
            });

            let prev_block_seqno_opt = prev.get_prevs().first().map(|x| x.seq_no);
            let accelerated_consensus_enabled =
                self.is_accelerated_consensus_enabled_for_shard(mc_state_extra, &ident);
            let session_id = get_session_unsafe_id(
                general_session_info.clone(),
                vsubset.list(),
                true,
                prev_block_seqno_opt,
                &self.config,
                accelerated_consensus_enabled,
            );

            let local_id_option = self.find_us(&subset.validators);

            if let Some(local_id) = &local_id_option {
                our_current_shards.insert(ident.clone(), vsubset.clone());

                log::debug!(
                    target: "validator_manager",
                    "subset for session: shard {}, cc_seqno {}, keyblock_seqno {}, \
                    validator_set {}, session_id {:x}",
                    ident, cc_seqno, keyblock_seqno,
                    validatorset_to_string(&vsubset), session_id
                );

                gc_validator_sessions.remove(&session_id);

                // If blockchain works under unsafe_catchain_rotation, then do not change its status:
                // 1. Do not start new sessions
                // 2. Do not remove functioning old sessions
                if do_unsafe_catchain_rotate && !ident.is_masterchain() && local_id_option.is_none()
                {
                    log::trace!(
                        target: "validator",
                        "Current shard {}, session {:x}: unsafe rotation skipping",
                        ident, session_id
                    );
                    continue;
                }

                let engine = self.engine.clone();
                let allow_unsafe_self_blocks_resync =
                    self.config.unsafe_resync_catchains.contains(&cc_seqno);
                let current_session_options = sessions_options.get_session_options(&ident);

                // Select consensus type based on ConfigParam 30
                let consensus_options =
                    self.select_consensus_options(&ident, mc_state, current_session_options);

                let session = self
                    .validator_sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| {
                        Arc::new(ValidatorGroup::new(
                            general_session_info.clone(),
                            local_id.clone(),
                            session_id.clone(),
                            validator_list_id.clone(),
                            vsubset.clone(),
                            consensus_options.clone(),
                            engine,
                            allow_unsafe_self_blocks_resync,
                        ))
                    })
                    .clone();

                let session_status = session.get_status().await;
                if session_status == ValidatorGroupStatus::Created {
                    log::trace!(target: "validator_manager", "Current shard {}, session {:x}: starting", ident, session_id);

                    session
                        .start_with_status(
                            group_start_status,
                            prev.get_prevs().to_vec(),
                            last_masterchain_block.clone(),
                            SystemTime::UNIX_EPOCH + Duration::from_secs(mc_now as u64),
                            self.rt.clone(),
                        )
                        .await?;
                } else if session_status >= ValidatorGroupStatus::Stopping {
                    log::error!(target: "validator_manager", "Cannot start stopped session {}", session.info().await);
                } else {
                    log::trace!(target: "validator_manager", "Current shard {}, session {:x}: working", ident, session_id);
                }
            } else {
                log::trace!(target: "validator_manager", "We are not in subset for {}", ident);
            }
            log::trace!(target: "validator_manager", "Session {} started (if necessary)", ident);
        }
        log::trace!(target: "validator_manager", "Starting/updating sessions, end of list");
        Ok(())
    }

    async fn update_shards(&mut self, mc_state: Arc<ShardStateStuff>) -> Result<()> {
        let mc_state_extra = mc_state.shard_state_extra()?;
        let master_cc_seqno = get_masterchain_seqno(self.engine.clone(), &mc_state).await?;
        let catchain_config = self.read_catchain_config(&mc_state)?;
        let sessions_options =
            self.compute_session_options(mc_state_extra, &catchain_config).await?;

        if !self.update_validator_lists(&mc_state).await? {
            log::info!(target: "validator_manager", "Current validator list is empty, validation is disabled.");
            self.disable_validation(true).await?;
            return Ok(());
        }

        let last_masterchain_block = mc_state.block_id();
        let keyblock_seqno = if mc_state_extra.after_key_block {
            mc_state.block_id().seq_no
        } else {
            mc_state_extra
                .last_key_block
                .as_ref()
                .map(|id| id.seq_no)
                .expect("masterchain state must contain info about previous key block")
        };
        let mc_now = mc_state.state()?.gen_time();

        self.enable_validation();

        self.update_validation_status(&mc_state, mc_state_extra).await?;

        let master_cc_range = master_cc_seqno..=master_cc_seqno; // Todo: compute always

        // Collect info about shards
        let mut gc_validator_sessions: HashSet<UInt256> =
            self.validator_sessions.keys().cloned().collect();

        // Shards that are working or about to start (continue) in this masterstate: shard_ident -> prevs
        let mut new_shards = HashMap::new();
        // Validator sets for shards that are working or about to start
        let mut our_current_shards: HashMap<ShardIdent, ValidatorSet> = HashMap::new();

        // Shards that will eventually be started (in later masterstates): need to prepare
        let mut future_shards: HashSet<ShardIdent> = HashSet::new();
        // Validator sets for shards that will eventually be started
        let mut our_future_shards: HashMap<
            ShardIdent,
            (ValidatorSubsetInfo, u32, ValidatorListHash),
        > = HashMap::new();
        let mut blocks_before_split: HashSet<BlockIdExt> = HashSet::new();

        new_shards.insert(ShardIdent::masterchain(), vec![last_masterchain_block.clone()]);
        future_shards.insert(ShardIdent::masterchain());
        mc_state_extra.shards().iterate_shards(|ident: ShardIdent, descr: ShardDescr| {
            // Add all shards that are effective from now
            // ValidatorGroups will be created and appropriate sessions started for these shards
            let top_block = BlockIdExt::with_params(
                ident.clone(),
                descr.seq_no,
                descr.root_hash,
                descr.file_hash
            );

            if descr.before_split {
                let lr_shards = ident.split();
                match lr_shards {
                    Err(e) => log::error!(target: "validator_manager", "Cannot split shard: `{}`", e),
                    Ok((l,r)) => {
                        new_shards.insert(l, vec![top_block.clone()]);
                        new_shards.insert(r, vec![top_block.clone()]);
                        blocks_before_split.insert(top_block);
                    }
                }
            } else if descr.before_merge {
                let parent_shard = ident.merge();
                match parent_shard {
                    Err(e) => log::error!(target: "validator_manager", "Cannot merge shard: `{}`", e),
                    Ok(p) => {
                        let mut prev_blocks = match new_shards.get(&p) {
                            Some(pb) => pb.clone(),
                            None => vec![BlockIdExt::default(), BlockIdExt::default()]
                        };

                        // Add previous block for the shard: there are two parents for merge, so two prevs
                        let (_l,r) = p.split()?;
                        prev_blocks[(r == ident) as usize] = top_block;
                        new_shards.insert(p, prev_blocks);
                    }
                }
            } else {
                new_shards.insert(ident.clone(), vec![top_block]);
            }

            // Create list of shards which will be effective soon
            // ValidatorGroups will be created for these shards, but not started.
            let cur_time = UnixTime::now();
            match descr.split_merge_at {
                FutureSplitMerge::None => {
                    future_shards.insert(ident);
                }
                FutureSplitMerge::Split{split_utime: time, interval: _interval} => {
                    if (time as u64) < cur_time + 60 {
                        match ident.split() {
                            Ok((l,r)) => {
                                future_shards.insert(l);
                                future_shards.insert(r);
                            }
                            Err(e) => log::error!(target: "validator_manager", "Cannot split shard {}: `{}`", ident, e)
                        }
                    } else {
                        future_shards.insert(ident);
                    }
                }
                FutureSplitMerge::Merge{merge_utime: time, interval: _interval} => {
                    if (time as u64) < cur_time + 60 {
                        match ident.merge() {
                            Ok(p) => {
                                future_shards.insert(p);
                            }
                            Err(e) => log::error!(target: "validator_manager", "Cannot merge shard {}: `{}`", ident, e)
                        }
                    } else {
                        future_shards.insert(ident);
                    }
                }
            };

            Ok(true)
        })?;

        // Initializing future shards
        log::debug!(target: "validator_manager", "Future shards initialization:");
        let next_validator_set = mc_state_extra.config.next_validator_set()?;
        let full_validator_set = mc_state_extra.config.validator_set()?;
        let possible_validator_change = next_validator_set.total() > 0;
        let mut mc_validators = Vec::with_capacity(full_validator_set.total() as usize);

        for ident in future_shards.iter() {
            log::trace!(target: "validator_manager", "Future shard {}", ident);
            let (cc_seqno_from_state, cc_lifetime) = if ident.is_masterchain() {
                (master_cc_seqno, catchain_config.mc_catchain_lifetime)
            } else {
                (
                    mc_state_extra.shards().calc_shard_cc_seqno(ident)?,
                    catchain_config.shard_catchain_lifetime,
                )
            };

            let near_validator_change = possible_validator_change
                && next_validator_set.utime_since() <= (mc_now / cc_lifetime + 1) * cc_lifetime;
            let future_validator_set = if near_validator_change {
                log::info!(
                    target: "validator_manager",
                    "Validator change will happen during catchain session lifetime \
                    for shard {}: cc_lifetime {}, now {}, next set since {}",
                    ident, cc_lifetime, mc_now, next_validator_set.utime_since()
                );
                &next_validator_set
            } else {
                &full_validator_set
            };

            let vnext_list_id = match compute_validator_list_id(future_validator_set.list(), None)?
            {
                None => continue,
                Some(l) => l,
            };

            let next_cc_seqno = cc_seqno_from_state + 1;

            let next_subset_opt = try_calc_subset_for_workchain_standard(
                future_validator_set,
                mc_state.config_params()?,
                ident,
                next_cc_seqno,
            )?;

            let next_subset = match next_subset_opt {
                Some(x) => x,
                None => {
                    log::error!(
                        target: "validator_manager",
                        "Cannot compute validator set for workchain {}: less than {} of {}",
                        ident,
                        catchain_config.shard_validators_num,
                        future_validator_set.list().len()
                    );
                    continue;
                }
            };

            our_future_shards.insert(ident.clone(), (next_subset, next_cc_seqno, vnext_list_id));
            log::trace!(
                target: "validator_manager",
                "Future shard {}: computing next subset with cc_seqno {} -- done",
                ident, next_cc_seqno
            );
        }

        // Iterate over shards and start all missing sessions
        log::trace!(target: "validator_manager", "Starting missing sessions");
        let validation_status = self.engine.validation_status();
        if validation_status.allows_validate() {
            self.start_sessions(
                new_shards,
                &mut our_current_shards,
                keyblock_seqno,
                &sessions_options,
                &mut gc_validator_sessions,
                mc_now,
                &mc_state,
                mc_state_extra,
                &master_cc_range,
                last_masterchain_block,
            )
            .await?;
        }
        log::trace!(target: "validator_manager", "Missing sessions started. Current shards:");

        // Iterate over future shards and create all future sessions
        for (ident, (wc, next_cc_seqno, next_val_list_id)) in our_future_shards.iter() {
            if ident.is_masterchain() {
                mc_validators.append(&mut wc.validators.clone());
            }

            if let Some(local_id) = self.find_us(&wc.validators) {
                let max_vertical_seqno = self.engine.hardforks().len() as u32;
                // Select appropriate options and hash based on shard type
                let current_opts_hash = if ident.is_masterchain() {
                    &sessions_options.mc_hash
                } else {
                    &sessions_options.shard_hash
                };

                let new_session_info = Arc::new(GeneralSessionInfo {
                    shard: ident.clone(),
                    opts_hash: current_opts_hash.clone(),
                    catchain_seqno: *next_cc_seqno,
                    key_seqno: keyblock_seqno,
                    max_vertical_seqno: max_vertical_seqno,
                });

                let accelerated_consensus_enabled =
                    self.is_accelerated_consensus_enabled_for_shard(mc_state_extra, &ident);
                let session_id = get_session_id(
                    new_session_info.clone(),
                    &wc.validators,
                    true,
                    accelerated_consensus_enabled,
                );
                let vsubset = wc.compute_validator_set(*next_cc_seqno)?;
                let current_session_options = sessions_options.get_session_options(&ident);
                gc_validator_sessions.remove(&session_id);

                // Select consensus type based on ConfigParam 30
                let consensus_options = self.select_consensus_options(
                    &ident,
                    mc_state.as_ref(),
                    current_session_options,
                );

                self.validator_sessions.entry(session_id.clone()).or_insert_with(|| {
                    Arc::new(ValidatorGroup::new(
                        new_session_info,
                        local_id,
                        session_id.clone(),
                        next_val_list_id.clone(),
                        vsubset.clone(),
                        consensus_options.clone(),
                        self.engine.clone(),
                        self.config.unsafe_resync_catchains.contains(next_cc_seqno),
                    ))
                });
            }
        }
        let mut precalc_split_queues_for: HashSet<BlockIdExt> = HashSet::new();
        for session in self.validator_sessions.values() {
            for id in &blocks_before_split {
                if id.shard().is_parent_for(session.shard()) {
                    log::trace!(
                        target: "validator_manager", "precalc_split_queues_for {}", id
                    );
                    precalc_split_queues_for.insert(id.clone());
                }
            }
        }

        // start background tasks which will precalculate split out messages queues
        for id in precalc_split_queues_for {
            let engine = self.engine.clone();
            tokio::spawn(async move {
                log::trace!(
                    target: "validator_manager", "Split queues precalculating for {}", id
                );
                match OutMsgQueueInfoStuff::precalc_split_queues(&engine, &id).await {
                    Ok(_) => log::trace!(
                        target: "validator_manager", "Split queues precalculated for {}", id
                    ),
                    Err(e) => log::error!(
                        target: "validator_manager",
                        "Can't precalculate split queues for {}: {}", id, e
                    ),
                }
            });
        }

        if rotate_all_shards(mc_state_extra) {
            log::info!(target: "validator_manager", "New last rotation block: {}", last_masterchain_block);
            self.engine.save_last_rotation_block_id(last_masterchain_block)?;
        }

        // Notify shard simplex sessions about MC finalization
        // This is needed for empty block generation (finalization recovery)
        self.notify_shard_sessions_mc_finalized(last_masterchain_block.seq_no);

        log::trace!(target: "validator_manager", "starting stop&remove");
        self.stop_and_remove_sessions(&gc_validator_sessions, true).await;

        log::trace!(target: "validator_manager", "starting garbage collect");
        self.garbage_collect().await;
        log::trace!(target: "validator_manager", "exiting");
        Ok(())
    }

    async fn stats(&mut self) {
        log::info!(target: "validator_manager", "{:32} {}", "session id", "st round shard");
        log::info!(target: "validator_manager", "{:-64}", "");

        // Validation shards statistics
        for (_, group) in self.validator_sessions.iter() {
            log::info!(target: "validator_manager", "{}", group.info().await);
            let status = group.get_status().await;
            if status == ValidatorGroupStatus::Sync
                || status == ValidatorGroupStatus::Active
                || status == ValidatorGroupStatus::Stopping
            {
                self.engine
                    .set_last_validation_time(group.shard().clone(), group.last_validation_time());
                self.engine
                    .set_last_collation_time(group.shard().clone(), group.last_collation_time());
            }
        }
        log::trace!(target: "validator_manager", "======= sessions stats over =======");
    }

    fn read_catchain_config(&self, state: &ShardStateStuff) -> Result<CatchainConfig> {
        let state_extra = state.shard_state_extra()?;
        state_extra.config.catchain_config()
    }

    /// infinite loop with possible error cancellation
    async fn invoke(&mut self) -> Result<()> {
        let last_applied_block_id =
            self.engine.load_last_applied_mc_block_id()?.ok_or_else(|| {
                error!("Cannot run validator_manager if no last applied block is present")
            })?;
        let last_applied_block_handle =
            self.engine.load_block_handle(&last_applied_block_id)?.ok_or_else(|| {
                error!("Cannot load handle for last applied master block {}", last_applied_block_id)
            })?;
        let mut mc_handle = if let Some(id) = self.engine.load_last_rotation_block_id()? {
            log::info!(
                target: "validator_manager",
                "Validator manager initialization: last rotation block: {}",
                id
            );
            self.engine
                .load_block_handle(&id)?
                .ok_or_else(|| error!("Cannot load handle for master block {}", id))?
        } else {
            log::info!(target: "validator_manager",
                "Validator manager initialization: no last rotation block, using last applied block: {}", last_applied_block_id
            );
            last_applied_block_handle.clone()
        };

        //let block_observer = self.initialize_block_observer(&last_applied_block_handle).await?;

        while !self.engine.check_stop() {
            log::trace!(target: "validator_manager", "Trying to load state for masterblock {}", mc_handle.id().seq_no);

            match self.engine.load_state(mc_handle.id()).await {
                Ok(mc_state) => {
                    log::info!(target: "validator_manager", "Processing masterblock {}", mc_handle.id().seq_no);
                    log::trace!(target: "validator_manager", "Processing messages from masterblock {}", mc_handle.id().seq_no);
                    log::trace!(target: "validator_manager", "Updating shards according to masterblock {}", mc_handle.id().seq_no);
                    self.update_shards(mc_state).await?;
                    log::trace!(target: "validator_manager", "Shards for masterblock {} updated", mc_handle.id().seq_no);
                }
                Err(e) => {
                    if self.engine.validation_status().allows_validate() {
                        fail!(
                            "State for {} lost while validating (status {:?}): error '{}'",
                            mc_handle.id(),
                            self.engine.validation_status(),
                            e
                        )
                    }
                    log::info!(target: "validator_manager", "Processing masterblock {}: state not available, going forward", mc_handle.id().seq_no);
                }
            }

            mc_handle = loop {
                log::trace!(target: "validator_manager", "Checking stop engine");
                if self.engine.check_stop() {
                    log::trace!(target: "validator_manager", "Engine is stoped. Exiting from invocation loop (while loading block)");
                    return Ok(());
                }
                log::trace!(target: "validator_manager", "Checked stop engine: going on");
                self.stats().await;
                log::trace!(target: "validator_manager", "Waiting next applied masterblock after {}", mc_handle.id().seq_no);
                match timeout(
                    self.config.update_interval,
                    self.engine.wait_next_applied_mc_block(&mc_handle, None),
                )
                .await
                {
                    Ok(r_res) => {
                        log::trace!(target: "validator_manager", "Got next applied master block (result): {}",
                            match &r_res {
                                Err(e) => format!("Err({})", e),
                                Ok((h, _bs)) => format!("Ok({})", h.id())
                            }
                        );
                        break r_res?.0;
                    }
                    Err(tokio::time::error::Elapsed { .. }) => {
                        log::warn!(
                            target: "validator_manager",
                            "Validator manager didn't receive next applied master block after {}",
                            mc_handle.id()
                        );
                    }
                }
            }
        }

        log::info!(target: "validator_manager", "Engine is stopped. Exiting from invocation loop (while applying state)");
        Ok(())
    }
}

/// main entry point to validation process
pub fn start_validator_manager(
    engine: Arc<dyn EngineOperations>,
    runtime: tokio::runtime::Handle,
    config: ValidatorManagerConfig,
) {
    const CHECK_VALIDATOR_TIMEOUT: u64 = 60; //secs
    runtime.clone().spawn(async move {
        log::info!(target: "validator_manager", "checking if current node is a validator during {CHECK_VALIDATOR_TIMEOUT} secs");
        engine.acquire_stop(Engine::MASK_SERVICE_VALIDATOR_MANAGER);
        while !engine.get_validator_status() {
            log::trace!(target: "validator_manager", "Not a validator, waiting...");
            let _ = engine.clear_last_rotation_block_id();
            for _ in 0..CHECK_VALIDATOR_TIMEOUT {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if engine.check_stop() {
                    log::error!(target: "validator_manager", "Engine is stopped. exiting");
                    engine.release_stop(Engine::MASK_SERVICE_VALIDATOR_MANAGER);
                    return;
                }
            }
        }

        log::trace!(target: "validator_manager", "Starting validator manager");
        let mut manager = ValidatorManagerImpl::create(engine.clone(), runtime.clone(), config);

        if let Err(e) = manager.invoke().await {
            log::error!(target: "validator_manager", "FATAL!!! Unexpected error in validator manager: {}", e);
        }

        manager.stop_validation().await;
        log::info!(target: "validator_manager", "Exiting, validator manager is stopped");
        engine.release_stop(Engine::MASK_SERVICE_VALIDATOR_MANAGER);
    });
}

#[cfg(test)]
#[path = "tests/test_session_id.rs"]
mod tests;
