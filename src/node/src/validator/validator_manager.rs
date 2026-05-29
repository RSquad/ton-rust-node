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
    RawBuffer, SimplexSessionOptions,
};
use crate::{
    config::ValidatorManagerConfig,
    engine::Engine,
    engine_traits::{EngineOperations, ValidatorListOutcome},
    shard_state::ShardStateStuff,
    validator::{
        out_msg_queue::OutMsgQueueInfoStuff,
        validator_group::{SessionSnapshot, ValidatorGroup, ValidatorGroupStatus},
        validator_utils::{
            compute_validator_list_id, get_group_members_by_validator_descrs,
            get_masterchain_seqno, try_calc_subset_for_workchain,
            try_calc_subset_for_workchain_standard, validatordescr_to_consensus_node,
            validatorset_to_string, GeneralSessionInfo, PrevBlockHistory, ValidatorListHash,
            ValidatorSubsetInfo,
        },
    },
};
use std::{
    cmp::max,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    ops::RangeInclusive,
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime},
};
use tokio::time::timeout;
use ton_api::IntoBoxed;
use ton_block::{
    base64_encode, error, fail, AcceleratedConsensusConfig, BlockIdExt, CatchainConfig,
    ConfigParamEnum, ConsensusConfig, FutureSplitMerge, McStateExtra, Result, ShardDescr,
    ShardIdent, SimplexConfig, UInt256, UnixTime, ValidatorDescr, ValidatorSet,
};

#[cfg(feature = "xp25")]
const MC_ACCELERATED_CONSENSUS_ENABLED: bool = true;
#[cfg(not(feature = "xp25"))]
const MC_ACCELERATED_CONSENSUS_ENABLED: bool = false;

fn format_shard_short(shard: &ShardIdent) -> String {
    if shard.is_masterchain() {
        "MC".to_string()
    } else {
        format!("{}:{:04X}..", shard.workchain_id(), shard.shard_prefix_with_tag() >> 48)
    }
}

fn format_time_ago(now_unix: u64, ts: u64) -> String {
    if ts == 0 {
        "-".to_string()
    } else if now_unix >= ts {
        format_duration_short(Duration::from_secs(now_unix - ts))
    } else {
        "0s".to_string()
    }
}

fn format_duration_short(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

const SHARD_EMPTY_BLOCK_MC_LAG_THRESHOLD: u32 = 8;

fn simplex_empty_block_lag_threshold(shard: &ShardIdent) -> Option<u32> {
    if shard.is_masterchain() {
        None
    } else {
        Some(SHARD_EMPTY_BLOCK_MC_LAG_THRESHOLD)
    }
}

fn applied_top_for_session_shard(
    mc_state: &ShardStateStuff,
    mc_state_extra: &McStateExtra,
    shard: &ShardIdent,
) -> Result<Option<BlockIdExt>> {
    if shard.is_masterchain() {
        Ok(Some(mc_state.block_id().clone()))
    } else {
        mc_registered_top_for_shard(mc_state_extra, shard)
    }
}

fn mc_registered_top_for_shard(
    mc_state_extra: &McStateExtra,
    shard: &ShardIdent,
) -> Result<Option<BlockIdExt>> {
    mc_state_extra.shards().find_shard(shard).map(|record| record.map(|r| r.block_id().clone()))
}

fn build_runtime_simplex_session_options(
    shard: &ShardIdent,
    cfg: &SimplexConfig,
    catchain_options: &CatchainSessionOptions,
) -> SimplexSessionOptions {
    let np = &cfg.noncritical_params;
    SimplexSessionOptions {
        proto_version: catchain_options.proto_version as u32,
        slots_per_leader_window: cfg.slots_per_leader_window,
        target_rate: Duration::from_millis(np.target_rate_ms as u64),
        min_block_interval: Duration::from_millis(np.min_block_interval_ms as u64),
        first_block_timeout: Duration::from_millis(np.first_block_timeout_ms as u64),
        first_block_timeout_multiplier: f32::from_bits(np.first_block_timeout_multiplier_bits)
            as f64,
        first_block_timeout_cap: Duration::from_millis(np.first_block_timeout_cap_ms as u64),
        candidate_resolve_timeout: Duration::from_millis(np.candidate_resolve_timeout_ms as u64),
        candidate_resolve_timeout_multiplier: f32::from_bits(
            np.candidate_resolve_timeout_multiplier_bits,
        ) as f64,
        candidate_resolve_timeout_cap: Duration::from_millis(
            np.candidate_resolve_timeout_cap_ms as u64,
        ),
        candidate_resolve_cooldown: Duration::from_millis(np.candidate_resolve_cooldown_ms as u64),
        standstill_timeout: Duration::from_millis(np.standstill_timeout_ms as u64),
        standstill_max_egress_bytes_per_s: np.standstill_max_egress_bytes_per_s,
        max_leader_window_desync: np.max_leader_window_desync,
        bad_signature_ban_duration: Duration::from_millis(np.bad_signature_ban_duration_ms as u64),
        candidate_resolve_rate_limit: np.candidate_resolve_rate_limit,
        max_block_size: catchain_options.max_block_size as usize,
        max_collated_data_size: catchain_options.max_collated_data_size as usize,
        use_callback_thread: false,
        use_quic: cfg.use_quic,
        no_empty_blocks_on_error_timeout: Duration::from_millis(
            np.no_empty_blocks_on_error_timeout_ms as u64,
        ),
        // C++ parity: shard sessions use lag threshold 8 for empty-block recovery.
        // MC sessions use internal consensus-finalized tracking and keep this unset.
        empty_block_mc_lag_threshold: simplex_empty_block_lag_threshold(shard),
        ..Default::default()
    }
}

fn validation_state_phase_label(status: ValidatorGroupStatus) -> &'static str {
    match status {
        ValidatorGroupStatus::Created | ValidatorGroupStatus::EngineCreated => "pre-start",
        ValidatorGroupStatus::Sync => "pre-commit",
        ValidatorGroupStatus::Active => "post-commit",
        ValidatorGroupStatus::Stopping => "stopping",
        ValidatorGroupStatus::Stopped => "stopped",
    }
}

/// Magic suffix appended to session-ID serialization when accelerated consensus is enabled.
///
/// **Rust-specific extension**: the C++ reference (`get_validator_set_id()` in `manager.cpp`)
/// does not include this tag because C++ does not yet support accelerated consensus in the
/// validator manager (`bridge.cpp` has only a TODO). When accelerated consensus is
/// disabled (the default for C++ interop), session IDs are byte-identical to C++.
const ACCELERATED_CONSENSUS_MAGIC_TAG: u32 = 0xACCE1E8A;

#[derive(Clone)]
pub struct SessionsOptions {
    pub mc_options: CatchainSessionOptions,
    pub shard_options: CatchainSessionOptions,
    /// Session-options hash used inside `validator.groupNew` for masterchain sessions.
    ///
    /// Without `xp25`, both hashes intentionally collapse to one C++-compatible
    /// `ValidatorSessionOptions` hash. With `xp25`, masterchain and shard hashes may
    /// diverge if their runtime session options differ in hash-relevant fields.
    pub mc_session_id_hash: UInt256,
    /// Session-options hash used inside `validator.groupNew` for shard sessions.
    pub shard_session_id_hash: UInt256,
}

impl SessionsOptions {
    pub fn get_session_options(&self, shard_id: &ShardIdent) -> &CatchainSessionOptions {
        if shard_id.is_masterchain() {
            &self.mc_options
        } else {
            &self.shard_options
        }
    }

    pub fn get_session_id_hash(&self, shard_id: &ShardIdent) -> &UInt256 {
        if shard_id.is_masterchain() {
            &self.mc_session_id_hash
        } else {
            &self.shard_session_id_hash
        }
    }
}

/// Serialize the `validator.groupNew` TL object for session-ID hashing.
///
/// Mirrors `get_validator_set_id()` in C++ (`manager.cpp`) for the
/// `new_catchain_ids == true` branch. Old catchain ID formats are intentionally
/// unsupported (see assertion).
fn get_session_id_serialize(
    session_info: Arc<GeneralSessionInfo>,
    vals: &[ValidatorDescr],
    new_catchain_ids: bool,
) -> RawBuffer {
    let mut members = Vec::new();
    get_group_members_by_validator_descrs(vals, &mut members);

    assert!(
        new_catchain_ids,
        "Old catchain IDs format (new_catchain_ids=false) is not supported by the Rust implementation"
    );
    {
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

/// Compute session ID by hashing the serialized `validator.groupNew` TL object.
///
/// When `accelerated_consensus_enabled` is true, appends [`ACCELERATED_CONSENSUS_MAGIC_TAG`]
/// before hashing to differentiate accelerated sessions from standard ones. Without the
/// tag, the resulting hash is byte-identical to C++ `get_validator_set_id()`.
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

/// C++ parity: during unsafe rotation (`force_recover`), skip all non-masterchain shards.
///
/// Mirrors the `force_recover` early-continue in C++ `update_shards()`:
/// ```cpp
/// if (force_recover && !desc.first.is_masterchain()) { continue; }
/// ```
fn should_skip_session_for_unsafe_rotation(
    do_unsafe_catchain_rotate: bool,
    shard: &ShardIdent,
) -> bool {
    do_unsafe_catchain_rotate && !shard.is_masterchain()
}

fn unsafe_rotation_block_seqno(
    shard: &ShardIdent,
    last_masterchain_block: &BlockIdExt,
) -> Option<u32> {
    shard.is_masterchain().then_some(last_masterchain_block.seq_no)
}

/// Check whether any local key belongs to the given validator subset.
///
/// Mirrors the inner loop of C++ `get_validator()` (`manager.cpp`) which
/// iterates `temp_keys_` and calls `val_set->is_validator(key)`. Returns the first local
/// key, in local-key order, that matches any validator descriptor's short ID.
fn find_local_validator_key(
    validators: &[ValidatorDescr],
    local_keys: Option<&[PublicKey]>,
) -> Option<PublicKey> {
    for local_key in local_keys? {
        let local_keyhash = local_key.id().data();
        for val in validators {
            let pkhash = val.compute_node_id_short();
            if pkhash.as_slice() == local_keyhash {
                return Some(local_key.clone());
            }
        }
    }
    None
}

enum ExistingSessionSource<T> {
    Current(T),
    Future(T),
}

fn select_existing_session_for_current_map<T: Clone>(
    session_id: &UInt256,
    current_sessions: &HashMap<UInt256, T>,
    future_sessions: &mut HashMap<UInt256, T>,
) -> Option<ExistingSessionSource<T>> {
    if let Some(existing) = current_sessions.get(session_id) {
        return Some(ExistingSessionSource::Current(existing.clone()));
    }

    future_sessions.remove(session_id).map(ExistingSessionSource::Future)
}

/// Computes session_id and if unsafe rotation is taking place,
/// replaces session_id with unsafe rotation session id.
/// The `do_unsafe_catchain_rotate` flag mirrors C++ `force_recover`:
/// the per-shard rotation check only runs when the global gate is set.
fn get_session_unsafe_id(
    session_info: Arc<GeneralSessionInfo>,
    val_set: &[ValidatorDescr],
    new_catchain_ids: bool,
    do_unsafe_catchain_rotate: bool,
    rotation_block_seqno_opt: Option<u32>,
    vm_config: &ValidatorManagerConfig,
    accelerated_consensus_enabled: bool,
) -> UInt256 {
    let session_id = get_session_id(
        session_info.clone(),
        val_set,
        new_catchain_ids,
        accelerated_consensus_enabled,
    );

    if do_unsafe_catchain_rotate && session_info.shard.is_masterchain() {
        if let Some(rotate_id) = vm_config
            .check_unsafe_catchain_rotation(rotation_block_seqno_opt, session_info.catchain_seqno)
        {
            let unsafe_serialized = compute_session_unsafe_serialized(&session_id, rotate_id);
            let unsafe_id = UInt256::calc_file_hash(unsafe_serialized.as_slice());

            log::warn!(
                target: "validator_manager",
                "Unsafe master session rotation: session {} at block={:?}, cc={} -> rotate_id={}, new session {}",
                session_id.to_hex_string(),
                rotation_block_seqno_opt,
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

#[cfg_attr(feature = "xp25", allow(dead_code))]
fn get_cxx_interop_session_options_hash(
    opts: &CatchainSessionOptions,
    last_masterchain_block_seqno: u32,
) -> (UInt256, RawBuffer) {
    let mut interop_opts = opts.clone();
    let defaults = CatchainSessionOptions::default();

    interop_opts.accelerated_consensus_enabled = false;
    interop_opts.accelerated_consensus_collation_retry_timeout =
        defaults.accelerated_consensus_collation_retry_timeout;
    interop_opts.accelerated_consensus_skip_rounds_count_for_collator_rotation =
        defaults.accelerated_consensus_skip_rounds_count_for_collator_rotation;
    interop_opts.accelerated_consensus_max_precollated_blocks =
        defaults.accelerated_consensus_max_precollated_blocks;

    get_validator_session_options_hash(interop_opts, last_masterchain_block_seqno)
}

/// Build the session-options view used by C++ `ValidatorSessionOptions opts{config}`.
///
/// This is intentionally derived only from config 29 / catchain config and never from
/// Rust's runtime accelerated-consensus toggles. It is used only in non-`xp25` builds
/// where Rust must preserve the C++ single-`opts_hash` behavior.
#[cfg_attr(feature = "xp25", allow(dead_code))]
fn get_cxx_interop_session_options(
    opts: &ConsensusConfig,
    catchain_config: &CatchainConfig,
) -> CatchainSessionOptions {
    let no_accelerated_consensus_config = None;
    get_session_options(opts, catchain_config, false, &no_accelerated_consensus_config)
}

#[cfg(feature = "xp25")]
fn get_session_id_hashes(
    _consensus_config: &ConsensusConfig,
    _catchain_config: &CatchainConfig,
    mc_options: &CatchainSessionOptions,
    shard_options: &CatchainSessionOptions,
    last_masterchain_block_seqno: u32,
) -> ((UInt256, RawBuffer), (UInt256, RawBuffer)) {
    (
        get_validator_session_options_hash(mc_options.clone(), last_masterchain_block_seqno),
        get_validator_session_options_hash(shard_options.clone(), last_masterchain_block_seqno),
    )
}

#[cfg(not(feature = "xp25"))]
fn get_session_id_hashes(
    consensus_config: &ConsensusConfig,
    catchain_config: &CatchainConfig,
    _mc_options: &CatchainSessionOptions,
    _shard_options: &CatchainSessionOptions,
    last_masterchain_block_seqno: u32,
) -> ((UInt256, RawBuffer), (UInt256, RawBuffer)) {
    let session_id_options = get_cxx_interop_session_options(consensus_config, catchain_config);
    let (session_id_hash, session_id_options_serialized) =
        get_cxx_interop_session_options_hash(&session_id_options, last_masterchain_block_seqno);
    (
        (session_id_hash.clone(), session_id_options_serialized.clone()),
        (session_id_hash, session_id_options_serialized),
    )
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

#[repr(u8)]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum ValidationStatus {
    Disabled = 0,
    Waiting = 1,
    Active = 2,
}

impl ValidationStatus {
    fn allows_validate(&self) -> bool {
        match self {
            Self::Disabled | Self::Waiting => false,
            Self::Active => true,
        }
    }
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ValidationStatus::Waiting,
            2 => ValidationStatus::Active,
            _ => ValidationStatus::Disabled,
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum NoCurrentSessionHealthReason {
    MissingOwnedCurrentSubsetSession,
    FutureSubsetOnly,
    NoCurrentSubsetOwned,
}

impl NoCurrentSessionHealthReason {
    fn label(&self) -> &'static str {
        match self {
            Self::MissingOwnedCurrentSubsetSession => "owned_current_subset_missing_session",
            Self::FutureSubsetOnly => "future_subset_only",
            Self::NoCurrentSubsetOwned => "no_current_subset_owned",
        }
    }

    fn should_warn(&self) -> bool {
        matches!(self, Self::MissingOwnedCurrentSubsetSession)
    }
}

fn classify_no_current_session_health(
    in_current_set: bool,
    validation_status: ValidationStatus,
    current_sessions_len: usize,
    owned_current_shards: usize,
    owned_future_shards: usize,
) -> Option<NoCurrentSessionHealthReason> {
    if !in_current_set || !validation_status.allows_validate() || current_sessions_len > 0 {
        return None;
    }

    if owned_current_shards > 0 {
        Some(NoCurrentSessionHealthReason::MissingOwnedCurrentSubsetSession)
    } else if owned_future_shards > 0 {
        Some(NoCurrentSessionHealthReason::FutureSubsetOnly)
    } else {
        Some(NoCurrentSessionHealthReason::NoCurrentSubsetOwned)
    }
}

/// Local node's participation record for a single validator list.
///
/// Stores local validator keys in the same local-key order that C++ uses for `temp_keys_`,
/// so per-shard selection can still pick the first matching local key within a subset.
struct LocalValidatorListEntry {
    keys: Vec<PublicKey>,
}

/// Tracks which validator lists the local node belongs to.
///
/// Maintains the current and next validator list IDs (mirroring the masterchain state's
/// current and next validator sets) along with the local node's keys for each.
///
/// C++ does not have a direct equivalent structure; it relies on `temp_keys_` for
/// membership checks and `allow_validate_` for the enable/disable gate.
#[derive(Default)]
struct ValidatorListStatus {
    known_lists: HashMap<ValidatorListHash, LocalValidatorListEntry>,
    curr: Option<ValidatorListHash>,
    next: Option<ValidatorListHash>,
    curr_utime_since: Option<u32>,
    next_utime_since: Option<u32>,
}

impl ValidatorListStatus {
    fn add_list(&mut self, list_id: ValidatorListHash, keys: Vec<PublicKey>) {
        self.known_lists.insert(list_id, LocalValidatorListEntry { keys });
    }

    fn contains_list(&self, list_id: &ValidatorListHash) -> bool {
        self.known_lists.contains_key(list_id)
    }

    fn remove_list(&mut self, list_id: &ValidatorListHash) {
        self.known_lists.remove(list_id);
    }

    fn get_list(&self, list_id: &ValidatorListHash) -> Option<&LocalValidatorListEntry> {
        self.known_lists.get(list_id)
    }

    fn get_local_keys_for_list(&self, list_id: &ValidatorListHash) -> Option<&[PublicKey]> {
        self.get_list(list_id).map(|entry| entry.keys.as_slice())
    }

    fn get_local_keys(&self) -> Option<&[PublicKey]> {
        self.curr.as_ref().and_then(|current_list| self.get_local_keys_for_list(current_list))
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

/// Core validator manager state.
///
/// Mirrors `ValidatorManagerImpl` in C++ (`manager.cpp`). Tracks active and future
/// validator sessions, the local node's membership in validator lists, and a blacklist
/// of destroyed session IDs to prevent recreation during the same masterchain cycle.
struct ValidatorManagerImpl {
    engine: Arc<dyn EngineOperations>,
    rt: tokio::runtime::Handle,
    /// Sessions for the current validator set (started or starting).
    current_sessions: HashMap<UInt256, Arc<ValidatorGroup>>,
    /// Sessions for the next (future) validator set with pre-created engines.
    future_sessions: HashMap<UInt256, Arc<ValidatorGroup>>,
    /// Number of current subsets selected for the local node on the last successful
    /// `update_shards()` pass.
    owned_current_shards: usize,
    /// Number of future subsets selected for the local node on the last successful
    /// `update_shards()` pass.
    owned_future_shards: usize,
    validator_list_status: ValidatorListStatus,
    config: ValidatorManagerConfig,
    /// Session IDs that have been destroyed and must not be recreated until the next
    /// full shard rotation. Mirrors C++ `destroyed_validator_sessions_` in `manager.cpp`.
    ///
    /// Persisted in the validator-state DB and cleared when `rotate_all_shards()` returns
    /// true, matching the C++ lifecycle around init-block updates.
    destroyed_sessions: HashSet<UInt256>,
    /// Set to `true` once `check_sync()` succeeds, enabling `Waiting -> Active`.
    sync_complete: bool,
    /// Wall-clock timestamp of the last full metrics dump.
    last_metrics_dump: tokio::time::Instant,
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
            current_sessions: HashMap::default(),
            future_sessions: HashMap::default(),
            owned_current_shards: 0,
            owned_future_shards: 0,
            validator_list_status: ValidatorListStatus::default(),
            config,
            destroyed_sessions: HashSet::new(),
            sync_complete: false,
            last_metrics_dump: tokio::time::Instant::now(),
        }
    }

    fn load_destroyed_sessions(&mut self) -> Result<()> {
        let persisted = self.engine.load_destroyed_session_ids()?;
        self.destroyed_sessions = persisted.into_iter().collect();
        if !self.destroyed_sessions.is_empty() {
            log::info!(
                target: "validator_manager",
                "Loaded {} destroyed session IDs from persistent storage",
                self.destroyed_sessions.len()
            );
        }
        Ok(())
    }

    fn persist_destroyed_sessions(&self) -> Result<()> {
        self.engine.save_destroyed_session_ids(&self.destroyed_sessions)
    }

    fn clear_destroyed_sessions(&mut self) -> Result<()> {
        if !self.destroyed_sessions.is_empty() {
            log::debug!(
                target: "validator_manager",
                "Clearing {} destroyed session IDs",
                self.destroyed_sessions.len()
            );
            self.destroyed_sessions.clear();
        }
        self.engine.clear_destroyed_session_ids()
    }

    /// Find the first matching local key for a subset using a specific validator list.
    ///
    /// Used for future-session creation where the validator list may differ from `curr`.
    fn find_us_for_list(
        &self,
        validators: &[ValidatorDescr],
        list_id: &ValidatorListHash,
    ) -> Option<PublicKey> {
        find_local_validator_key(
            validators,
            self.validator_list_status.get_local_keys_for_list(list_id),
        )
    }

    /// Find the first matching local key for a subset using the current validator list.
    ///
    /// Used for active-session creation in `start_sessions`.
    fn find_us(&self, validators: &[ValidatorDescr]) -> Option<PublicKey> {
        find_local_validator_key(validators, self.validator_list_status.get_local_keys())
    }

    /// Register the local node in a validator list and return its hash if matched.
    ///
    /// Calls [`EngineOperations::set_validator_list`] which checks local keys against the
    /// validator set and attempts to set up ADNL/overlay infrastructure.
    /// Membership is cached in [`ValidatorListStatus`] independently from transport readiness.
    ///
    /// Returns `Ok(None)` only when the local node is genuinely not in the validator set.
    async fn update_single_validator_list(
        &mut self,
        validator_list: &[ValidatorDescr],
        name: &str,
    ) -> Result<Option<ValidatorListHash>> {
        let list_id = match compute_validator_list_id(validator_list, None)? {
            None => return Ok(None),
            Some(l) => l,
        };
        if self.validator_list_status.contains_list(&list_id)
            && self.engine.validator_network().has_validator_list_context(&list_id)
        {
            return Ok(Some(list_id));
        }

        let nodes_res: Vec<ConsensusNode> = validator_list
            .iter()
            .map(validatordescr_to_consensus_node)
            .collect::<Vec<ConsensusNode>>();

        log::info!(target: "validator_manager", "Updating {} validator list (id {:x}):", name, list_id);
        for x in &nodes_res {
            log::debug!(target: "validator_manager", "pk: {}, pk_id: {}, adnl_id: {}",
                hex::encode(x.public_key.pub_key().unwrap()),
                hex::encode(x.public_key.id().data()),
                hex::encode(x.adnl_id.data())
            );
        }

        match self.engine.set_validator_list(list_id.clone(), &nodes_res).await? {
            ValidatorListOutcome::Selected { key, matching_keys } => {
                self.validator_list_status.add_list(list_id.clone(), matching_keys);
                let context_ready =
                    self.engine.validator_network().has_validator_list_context(&list_id);
                if context_ready {
                    log::info!(target: "validator_manager", "Local node: pk_id: {} id: {}",
                        hex::encode(key.pub_key().unwrap()),
                        hex::encode(key.id().data())
                    );
                } else {
                    log::warn!(
                        target: "validator_manager",
                        "Local node is a {} validator by pubkey (id {:x}, key {}), \
                         but ADNL/network context is still pending; continuing with membership only",
                        name,
                        list_id,
                        hex::encode(key.id().data())
                    );
                }
                Ok(Some(list_id))
            }
            ValidatorListOutcome::NotValidator => {
                log::info!(target: "validator_manager", "Local node is not a {} validator", name);
                Ok(None)
            }
        }
    }

    /// Refresh the current and next validator lists from the masterchain state.
    ///
    /// Returns `true` if the local node belongs to at least one validator set (current or
    /// next), `false` if it is not a validator at all. The caller uses `false` to disable
    /// validation entirely.
    ///
    /// Mirrors the implicit list-management in C++ `update_shards()` where `get_validator()`
    /// is called per-shard. In Rust, we pre-resolve membership once per update round.
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
            if !self.engine.validator_network().has_validator_list_context(id) {
                log::warn!(
                    target: "validator_manager",
                    "Current validator list {:x} is selected by pubkey but transport context is \
                     still pending; session ownership remains active and startup will retry",
                    id
                );
            }
        }
        self.validator_list_status.next =
            self.update_single_validator_list(next_validator_set.list(), "next").await?;
        self.validator_list_status.next_utime_since = Some(next_validator_set.utime_since());

        metrics::gauge!("ton_node_validator_in_current_set")
            .set(self.validator_list_status.curr.is_some() as u8 as f64);
        metrics::gauge!("ton_node_validator_in_next_set")
            .set(self.validator_list_status.next.is_some() as u8 as f64);
        Ok(self.validator_list_status.curr.is_some() || self.validator_list_status.next.is_some())
    }

    async fn is_active_shard(&self, shard: &ShardIdent) -> bool {
        for group in self.current_sessions.values().chain(self.future_sessions.values()) {
            if group.shard() == shard {
                match group.get_status().await {
                    ValidatorGroupStatus::Sync | ValidatorGroupStatus::Active => return true,
                    _ => (),
                }
            }
        }
        false
    }

    async fn garbage_collect_lists(&mut self) -> Result<()> {
        log::trace!(target: "validator_manager", "Garbage collect lists");
        let mut lists_gc = self.validator_list_status.known_hashes();

        for id in self.current_sessions.values().chain(self.future_sessions.values()) {
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

    /// Notify simplex sessions with the currently applied top for their session shard.
    ///
    /// This should be called after each masterchain state update:
    /// - masterchain simplex sessions receive the applied masterchain block id
    /// - shard simplex sessions receive the shard top currently registered in masterchain
    ///
    /// This is an applied-top signal, not a session-local finalization signal.
    /// Current MC sessions use it to track accepted/applied head ordering, while
    /// local finalized progression remains owned by the Simplex session itself.
    ///
    /// Delivery is enqueued into each validator group's ordered action queue without
    /// blocking the manager's hot path.
    ///
    /// # Arguments
    /// * `mc_state` - Current applied masterchain state
    /// * `mc_state_extra` - Current masterchain extra containing shard descriptors
    fn notify_simplex_sessions_applied_tops(
        &self,
        mc_state: &ShardStateStuff,
        mc_state_extra: &McStateExtra,
    ) {
        consensus_common::check_execution_time!(5000); // 5ms max

        for (session_id, group) in self.current_sessions.iter() {
            if !group.is_simplex() {
                continue;
            }
            let applied_top =
                match applied_top_for_session_shard(mc_state, mc_state_extra, group.shard()) {
                    Ok(Some(block_id)) => block_id,
                    Ok(None) => {
                        log::trace!(
                            target: "validator_manager",
                            "Skipping applied-top notify for session {:x} (shard {}): \
                            no matching top in current MC state",
                            session_id,
                            group.shard(),
                        );
                        continue;
                    }
                    Err(e) => {
                        log::warn!(
                            target: "validator_manager",
                            "Failed to lookup shard {} in MC state for session {:x}: {}",
                            group.shard(),
                            session_id,
                            e
                        );
                        continue;
                    }
                };
            log::trace!(
                target: "validator_manager",
                "Notifying session {:x} (shard {}) about applied top: {}",
                session_id,
                group.shard(),
                applied_top
            );
            group.notify_mc_finalized(applied_top);
            log::trace!(
                target: "validator_manager",
                "Applied-top notification queued for session {:x}",
                session_id
            );
        }
    }

    /// Stop sessions that are no longer active or pending, and record their IDs in the
    /// destroyed-session blacklist to prevent recreation within the same masterchain cycle.
    /// Mirrors C++ group destruction logic in `update_shards()` (`manager.cpp`).
    async fn stop_and_remove_sessions(
        &mut self,
        sessions_to_remove: &HashSet<UInt256>,
        destroy_database: bool,
    ) {
        if !sessions_to_remove.is_empty() {
            log::debug!(target: "validator_manager",
                "stop_and_remove_sessions: removing {} sessions, destroy_db={}",
                sessions_to_remove.len(), destroy_database);
        }
        for id in sessions_to_remove.iter() {
            self.destroyed_sessions.insert(id.clone());

            let future_group = self.future_sessions.remove(id);

            match self.current_sessions.get(id) {
                None => {
                    if let Some(fg) = future_group {
                        // C++ parity: tentative groups are destroyed via
                        // IValidatorGroup::destroy in manager.cpp.
                        log::info!(target: "validator_manager",
                            "SESSION_LIFECYCLE: gc_future shard={} cc_seqno={} session_id={:x} \
                             destroy_db={} (no longer needed)",
                            fg.shard(), fg.cc_seqno(), id, destroy_database);
                        let cl = if fg.is_simplex() { "simplex" } else { "catchain" };
                        metrics::counter!(
                            "ton_node_validator_session_destroyed_total",
                            "consensus" => cl
                        )
                        .increment(1);
                        if let Err(e) = fg.stop(self.rt.clone(), destroy_database).await {
                            log::error!(target: "validator_manager",
                                "SESSION_LIFECYCLE: gc_future_stop_failed session_id={:x}: {}",
                                id, e);
                        }
                    } else {
                        log::trace!(target: "validator_manager",
                            "Session {:x} not in current or future maps", id);
                    }
                }
                Some(session) => {
                    let status = session.get_status().await;
                    let shard = session.shard().clone();
                    match status {
                        ValidatorGroupStatus::Stopping => {
                            log::debug!(target: "validator_manager",
                                "SESSION_LIFECYCLE: already stopping shard={} session_id={:x}",
                                shard, id);
                        }
                        ValidatorGroupStatus::Stopped => {
                            if let Some(group) = self.current_sessions.remove(id) {
                                log::info!(target: "validator_manager",
                                    "SESSION_LIFECYCLE: gc_stopped shard={} session_id={:x}",
                                    shard, id);
                                let cl = if group.is_simplex() { "simplex" } else { "catchain" };
                                metrics::counter!(
                                    "ton_node_validator_session_destroyed_total",
                                    "consensus" => cl
                                )
                                .increment(1);
                                if !self.is_active_shard(group.shard()).await {
                                    self.engine.remove_last_validation_time(group.shard());
                                    self.engine.remove_last_collation_time(group.shard());
                                }
                            }
                        }
                        _ => {
                            log::info!(target: "validator_manager",
                                "SESSION_LIFECYCLE: gc_stop shard={shard} session_id={id:x} \
                                status={status} destroy_db={}",
                                destroy_database
                            );
                            let cl = if session.is_simplex() { "simplex" } else { "catchain" };
                            metrics::counter!(
                                "ton_node_validator_session_destroyed_total",
                                "consensus" => cl
                            )
                            .increment(1);
                            if let Err(e) =
                                session.clone().stop(self.rt.clone(), destroy_database).await
                            {
                                log::error!(target: "validator_manager",
                                    "SESSION_LIFECYCLE: gc_stop_failed shard={} session_id={:x}: {}",
                                    shard, id, e);
                                self.current_sessions.remove(id);
                            }
                        }
                    }
                }
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
        let last_key_block_seqno =
            mc_state_extra.last_key_block.as_ref().map(|x| x.seq_no).unwrap_or(0);

        // Compute session options for masterchain (accelerated consensus controlled by constant)
        let mc_accelerated_consensus_enabled = MC_ACCELERATED_CONSENSUS_ENABLED
            && accelerated_consensus_config.as_ref().map(|x| x.enabled).unwrap_or(false);
        let mc_options = get_session_options(
            &consensus_config,
            catchain_config,
            mc_accelerated_consensus_enabled,
            &accelerated_consensus_config,
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
        let (
            (mc_session_id_hash, mc_session_id_options_serialized),
            (shard_session_id_hash, shard_session_id_options_serialized),
        ) = get_session_id_hashes(
            &consensus_config,
            catchain_config,
            &mc_options,
            &shard_options,
            last_key_block_seqno,
        );

        log::trace!(
            target: "validator_manager",
            "MC SessionOptions from config.29: {mc_options:?}"
        );
        log::trace!(
            target: "validator_manager",
            "MC Session-ID SessionOptions serialized: {} hash: {:x}",
            hex::encode(mc_session_id_options_serialized),
            mc_session_id_hash
        );
        log::trace!(
            target: "validator_manager",
            "Shard Session-ID SessionOptions serialized: {} hash: {:x}",
            hex::encode(shard_session_id_options_serialized),
            shard_session_id_hash
        );
        log::trace!(
            target: "validator_manager",
            "Shard SessionOptions from config.29: {shard_options:?}"
        );

        Ok(SessionsOptions { mc_options, shard_options, mc_session_id_hash, shard_session_id_hash })
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
    /// Returns `ConsensusOptions::Simplex` when ConfigParam 30 contains a valid
    /// SimplexConfig (v1 or v2) for the given shard.  Otherwise, returns
    /// `ConsensusOptions::Catchain` — the node must remain fully catchain-compatible
    /// as long as the on-chain config can switch between the two at any time.
    ///
    /// C++ reference: `validator/manager.cpp` — `ValidatorManagerImpl::create_validator_group`
    ///   - calls `last_masterchain_state_->get_new_consensus_config(shard.workchain)`
    ///   - if present → `IValidatorGroup::create_bridge` (simplex)
    ///   - if absent  → `IValidatorGroup::create_catchain`
    ///
    /// Noncritical params override (not yet implemented):
    ///   C++ ref: `validator/validator-options.hpp` —
    ///   `ValidatorManagerOptionsImpl::get_noncritical_params`
    fn select_consensus_options(
        &self,
        shard: &ShardIdent,
        mc_state: &ShardStateStuff,
        catchain_options: &CatchainSessionOptions,
        _cc_seqno: u32,
    ) -> ConsensusOptions {
        // C++ ref: mc-config.cpp — Config::get_new_consensus_config reads ConfigParam 30
        // directly without checking global_version.  Absence of the param
        // (or a parse error) falls through to the catchain path below.
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

        // C++ ref: get_new_consensus_config(wc) selects mc or shard inner config,
        // then tries simplex_config#21 / simplex_config_v2#22.
        // Absence → catchain fallback (the node must stay catchain-compatible).
        let simplex_cfg: Option<SimplexConfig> = if shard.is_masterchain() {
            config_params.get_mc_simplex_config().ok().flatten()
        } else {
            config_params.get_shard_simplex_config().ok().flatten()
        };

        if let Some(cfg) = simplex_cfg {
            log::trace!(
                target: "validator_manager",
                "Simplex config found for {}: target_rate={}ms, slots_per_window={}, first_block_timeout={}ms",
                shard,
                cfg.noncritical_params.target_rate_ms,
                cfg.slots_per_leader_window,
                cfg.noncritical_params.first_block_timeout_ms
            );

            // C++ ref: mc-config.cpp maps noncritical params to
            // NewConsensusConfig::NoncriticalParams fields via ENUMERATE_NONCRITICAL_PARAMS.
            // Doubles are stored as f32 bits in the u32 values.
            //
            // TODO: C++ also applies per-shard/cc_seqno overrides here via
            // get_noncritical_params() in validator-options.hpp.
            let opts = build_runtime_simplex_session_options(shard, &cfg, catchain_options);
            return ConsensusOptions::Simplex(opts);
        }

        // No simplex config → catchain fallback.
        // This is the expected path when testnet has empty ConfigParam 30 or when
        // ConfigParam 30 contains null_consensus_config#20.
        log::trace!(
            target: "validator_manager",
            "No simplex config for {}, using catchain",
            shard
        );
        ConsensusOptions::Catchain(catchain_options.clone())
    }

    async fn update_validation_status(
        &mut self,
        mc_state: &ShardStateStuff,
        mc_state_extra: &McStateExtra,
    ) -> Result<()> {
        let prev_status = self.engine.validation_status();
        match prev_status {
            ValidationStatus::Waiting => {
                let last_masterchain_block = mc_state.block_id();
                let later_than_hardfork =
                    self.engine.get_last_fork_masterchain_seqno() <= last_masterchain_block.seq_no;

                let synced = self.engine.check_sync().await?;
                log::trace!(target: "validator_manager",
                    "update_validation_status: Waiting check: synced={} later_than_hardfork={} \
                     mc_seqno={}",
                    synced, later_than_hardfork, last_masterchain_block.seq_no);

                // Phase 1: mark sync complete and backfill future engines.
                // C++ parity: sync_complete() sets started_=true and sweeps both
                // validator_groups_ and next_validator_groups_ calling
                // create_session() on all groups (manager.cpp sync_complete).
                if synced && later_than_hardfork && !self.sync_complete {
                    self.sync_complete = true;
                    if !self.future_sessions.is_empty() {
                        log::info!(target: "validator_manager",
                            "SESSION_LIFECYCLE: backfill_pre_create_engine for {} future sessions \
                             after sync_complete",
                            self.future_sessions.len());
                        for (id, group) in &self.future_sessions {
                            let g = group.clone();
                            let session_id = id.clone();
                            tokio::spawn(async move {
                                if let Err(e) = g.pre_create_engine().await {
                                    log::error!(target: "validator_manager",
                                        "SESSION_LIFECYCLE: backfill_pre_create_failed \
                                         session_id={:x}: {}",
                                        session_id, e);
                                }
                            });
                        }
                    }
                }

                // Phase 2: transition Waiting -> Active.
                // C++ parity: allow_validate_ is set purely from
                // rotated_all_shards() || seqno==0 plus the fork check
                // (manager.cpp update_shards).  sync_complete (started_)
                // only gates create_session() on existing groups (Phase 1
                // above), NOT the allow_validate_ / status transition.
                let rotated =
                    rotate_all_shards(mc_state_extra) || last_masterchain_block.seq_no == 0;
                if rotated && later_than_hardfork {
                    let bootstrap = last_masterchain_block.seq_no == 0;
                    log::info!(target: "validator_manager",
                        "VALIDATION_STATUS: Waiting -> Active \
                         (rotated_all_shards={}, mc_seqno={}, sync_complete={}, bootstrap={}, \
                         no_countdown_for_zerostate={})",
                        rotate_all_shards(mc_state_extra),
                        last_masterchain_block.seq_no,
                        self.sync_complete,
                        bootstrap,
                        self.config.no_countdown_for_zerostate);
                    self.engine.set_validation_status(ValidationStatus::Active);
                } else if !rotated {
                    log::trace!(target: "validator_manager",
                        "update_validation_status: rotated_all_shards=false, \
                         deferring Waiting -> Active (mc_seqno={})",
                        last_masterchain_block.seq_no);
                }
            }
            ValidationStatus::Disabled | ValidationStatus::Active => {}
        }
        Ok(())
    }

    async fn disable_validation(&mut self, clear_rotation: bool) -> Result<()> {
        let prev_status = self.engine.validation_status();
        let n_current = self.current_sessions.len();
        let n_future = self.future_sessions.len();
        log::info!(target: "validator_manager",
            "VALIDATION_STATUS: {prev_status:?} -> Disabled (clear_rotation={clear_rotation}, \
            current_sessions={n_current}, future_sessions={n_future})"
        );

        self.engine.set_validation_status(ValidationStatus::Disabled);
        self.sync_complete = false;
        self.owned_current_shards = 0;
        self.owned_future_shards = 0;

        let existing_validator_sessions: HashSet<UInt256> =
            self.current_sessions.keys().chain(self.future_sessions.keys()).cloned().collect();
        self.stop_and_remove_sessions(&existing_validator_sessions, clear_rotation).await;
        self.garbage_collect().await;
        self.engine.set_will_validate(false);
        if clear_rotation {
            self.clear_destroyed_sessions()?;
            self.engine.clear_last_rotation_block_id()?;
        } else {
            self.persist_destroyed_sessions()?;
        }
        log::info!(target: "validator_manager",
            "VALIDATION_STATUS: Disabled complete (stopped {} current + {} future sessions)",
            n_current, n_future);
        Ok(())
    }

    async fn stop_validation(&mut self) {
        if let Err(e) = self.disable_validation(false).await {
            log::error!(target: "validator_manager", "Cannot disable validation: {}", e);
        };
    }

    fn enable_validation(&mut self) {
        self.engine.set_will_validate(true);
        let current = self.engine.validation_status();
        // C++ parity: enable_validation() only ensures we are at least
        // Waiting. The Waiting -> Active promotion is handled by
        // update_validation_status() based on rotated_all_shards(),
        // matching C++'s allow_validate_ which is independent of sync.
        let target = max(current, ValidationStatus::Waiting);
        if target != current {
            log::info!(target: "validator_manager",
                "VALIDATION_STATUS: {:?} -> {:?}",
                current, target);
        }
        self.engine.set_validation_status(target);
    }

    /// Create and start validator sessions for all currently active shards.
    ///
    /// Mirrors the `new_shards` loop in C++ `update_shards()` (`manager.cpp`):
    /// - Skips non-masterchain shards during unsafe rotation (`force_recover`)
    /// - Computes validator subset and session ID (with optional unsafe-rotation patch)
    /// - Skips sessions in the [`destroyed_sessions`] blacklist
    /// - Finds the local validator key in the subset
    /// - Creates or reuses the `ValidatorGroup` and starts it if newly created
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
        let validator_list_id = match self.validator_list_status.curr.as_ref() {
            Some(list_id) => list_id.clone(),
            None => {
                log::trace!(
                    target: "validator_manager",
                    "Skipping current-session start: local node is not in current validator list"
                );
                return Ok(());
            }
        };
        let full_validator_set = mc_state_extra.config.validator_set()?;

        let catchain_config = self.read_catchain_config(mc_state)?;

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
        // C++ parity: rebuild current-session ownership in a fresh map each update pass.
        // We retain only sessions selected by the current shard iteration and swap atomically
        // at the end. Old current entries that are not selected are stopped right after swap.
        let mut new_current_sessions: HashMap<UInt256, Arc<ValidatorGroup>> = HashMap::new();

        for (ident, prev_blocks) in new_shards {
            let cc_seqno_from_state = if ident.is_masterchain() {
                *master_cc_range.end()
            } else {
                mc_state_extra.shards().calc_shard_cc_seqno(&ident)?
            };

            let cc_seqno = cc_seqno_from_state;

            // C++ parity: during unsafe rotation, skip all non-masterchain shards
            // before any expensive subset/session-id computation.
            if should_skip_session_for_unsafe_rotation(do_unsafe_catchain_rotate, &ident) {
                log::trace!(
                    target: "validator_manager",
                    "Shard {}, cc_seqno {}: unsafe rotation skipping",
                    ident, cc_seqno
                );
                continue;
            }

            log::trace!(
                target: "validator_manager",
                "Trying to start/update session for shard {ident}, cc_seqno {cc_seqno_from_state}"
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

            let general_session_info = Arc::new(GeneralSessionInfo {
                shard: ident.clone(),
                opts_hash: sessions_options.get_session_id_hash(&ident).clone(),
                catchain_seqno: cc_seqno,
                key_seqno: keyblock_seqno,
                max_vertical_seqno: max_vertical_seqno,
            });

            let rotation_block_seqno_opt =
                unsafe_rotation_block_seqno(&ident, last_masterchain_block);
            let accelerated_consensus_enabled =
                self.is_accelerated_consensus_enabled_for_shard(mc_state_extra, &ident);
            let session_id = get_session_unsafe_id(
                general_session_info.clone(),
                vsubset.list(),
                true,
                do_unsafe_catchain_rotate,
                rotation_block_seqno_opt,
                &self.config,
                accelerated_consensus_enabled,
            );

            let local_id_option = self.find_us(&subset.validators);

            if local_id_option.is_some() {
                // Track owned current subsets independently of whether this specific session
                // is temporarily suppressed by the destroyed-session blacklist.
                // Otherwise health classification can mislabel a missing owned current session
                // as benign `NoCurrentSubsetOwned`.
                our_current_shards.insert(ident.clone(), vsubset.clone());
            }

            // C++ parity: skip sessions in the destroyed set
            if self.destroyed_sessions.contains(&session_id) {
                log::trace!(
                    target: "validator_manager",
                    "Skipping destroyed session {:x} for shard {}", session_id, ident
                );
                continue;
            }

            if let Some(local_id) = &local_id_option {
                log::debug!(
                    target: "validator_manager",
                    "subset for session: shard {}, cc_seqno {}, keyblock_seqno {}, \
                    validator_set {}, session_id {:x}",
                    ident, cc_seqno, keyblock_seqno,
                    validatorset_to_string(&vsubset), session_id
                );

                gc_validator_sessions.remove(&session_id);

                let engine = self.engine.clone();
                let allow_unsafe_self_blocks_resync =
                    self.config.unsafe_resync_catchains.contains(&cc_seqno);
                let current_session_options = sessions_options.get_session_options(&ident);

                // Select consensus type based on ConfigParam 30
                let consensus_options = self.select_consensus_options(
                    &ident,
                    mc_state,
                    current_session_options,
                    cc_seqno,
                );

                let session = match select_existing_session_for_current_map(
                    &session_id,
                    &self.current_sessions,
                    &mut self.future_sessions,
                ) {
                    Some(ExistingSessionSource::Current(existing)) => {
                        log::trace!(
                            target: "validator_manager",
                            "SESSION_LIFECYCLE: keep_current shard={} cc_seqno={} session_id={:x}",
                            ident,
                            cc_seqno,
                            session_id
                        );
                        existing
                    }
                    Some(ExistingSessionSource::Future(promoted)) => {
                        log::info!(
                            target: "validator_manager",
                            "SESSION_LIFECYCLE: promote shard={} cc_seqno={} session_id={:x} \
                             future -> current",
                            ident,
                            cc_seqno,
                            session_id
                        );
                        promoted
                    }
                    None => {
                        let consensus_name = match &consensus_options {
                            ConsensusOptions::Simplex(_) => "simplex",
                            ConsensusOptions::Catchain(_) => "catchain",
                        };
                        log::info!(target: "validator_manager",
                            "SESSION_LIFECYCLE: create_current shard={} cc_seqno={} \
                             session_id={:x} consensus={} local_key={}",
                            ident, cc_seqno, session_id, consensus_name,
                            hex::encode(local_id.id().data()));
                        metrics::counter!(
                            "ton_node_validator_session_created_total",
                            "consensus" => consensus_name
                        )
                        .increment(1);
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
                    }
                };
                new_current_sessions.insert(session_id.clone(), session.clone());

                let session_status = session.get_status().await;
                if session.try_prepare_start().await? {
                    log::trace!(
                        target: "validator_manager",
                        "Current shard {ident}, session {session_id:x}: starting"
                    );

                    session
                        .start_session(
                            prev.get_prevs().to_vec(),
                            last_masterchain_block.clone(),
                            SystemTime::UNIX_EPOCH + Duration::from_secs(mc_now as u64),
                            self.rt.clone(),
                        )
                        .await?;
                } else if session.is_start_pending().await {
                    log::trace!(
                        target: "validator_manager",
                        "Current shard {}, session {:x}: start pending",
                        ident,
                        session_id
                    );
                } else if session_status >= ValidatorGroupStatus::Stopping {
                    log::error!(
                        target: "validator_manager",
                        "Cannot start stopped session {}",
                        session.info().await
                    );
                } else {
                    log::trace!(
                        target: "validator_manager",
                        "Current shard {ident}, session {session_id:x}: working"
                    );
                }
            } else {
                log::trace!(target: "validator_manager", "We are not in subset for {}", ident);
            }
            log::trace!(target: "validator_manager", "Session {} started (if necessary)", ident);
        }
        let stale_old_current_sessions: Vec<(UInt256, Arc<ValidatorGroup>)> = self
            .current_sessions
            .iter()
            .filter(|(id, _)| !new_current_sessions.contains_key(*id))
            .map(|(id, group)| (id.clone(), group.clone()))
            .collect();

        let old_current_count = self.current_sessions.len();
        self.current_sessions = new_current_sessions;

        for (session_id, stale_group) in stale_old_current_sessions {
            self.destroyed_sessions.insert(session_id.clone());
            let stale_shard = stale_group.shard().clone();
            let status = stale_group.get_status().await;
            log::info!(
                target: "validator_manager",
                "SESSION_LIFECYCLE: gc_stop shard={} session_id={:x} status={} destroy_db=true \
                 (obsolete after current-map swap)",
                stale_group.shard(),
                session_id,
                status
            );
            let consensus_label = if stale_group.is_simplex() { "simplex" } else { "catchain" };
            metrics::counter!(
                "ton_node_validator_session_destroyed_total",
                "consensus" => consensus_label
            )
            .increment(1);
            if let Err(e) = stale_group.clone().stop(self.rt.clone(), true).await {
                log::error!(
                    target: "validator_manager",
                    "SESSION_LIFECYCLE: gc_stop_failed shard={} session_id={:x}: {}",
                    stale_group.shard(),
                    session_id,
                    e
                );
            }
            if !self.is_active_shard(&stale_shard).await {
                self.engine.remove_last_validation_time(&stale_shard);
                self.engine.remove_last_collation_time(&stale_shard);
            }
        }

        log::trace!(
            target: "validator_manager",
            "Starting/updating sessions, end of list (current map swapped: old={} new={})",
            old_current_count,
            self.current_sessions.len()
        );
        Ok(())
    }

    /// Main per-masterchain-block update loop.
    ///
    /// Mirrors `ValidatorManagerImpl::update_shards()` in C++ (`manager.cpp`).
    /// Responsibilities:
    /// 1. Refresh validator list membership (`update_validator_lists`)
    /// 2. Collect current and future shards from the masterchain state
    /// 3. Create/start sessions for current shards (`start_sessions`)
    /// 4. Pre-create sessions for upcoming shards (future-sessions loop)
    /// 5. GC sessions that are no longer needed (`stop_and_remove_sessions`)
    /// 6. Clear the destroyed-sessions blacklist on full shard rotation
    async fn update_shards(&mut self, mc_state: Arc<ShardStateStuff>) -> Result<()> {
        let mc_state_extra = mc_state.shard_state_extra()?;
        let master_cc_seqno = get_masterchain_seqno(self.engine.clone(), &mc_state).await?;
        let catchain_config = self.read_catchain_config(&mc_state)?;
        let sessions_options =
            self.compute_session_options(mc_state_extra, &catchain_config).await?;

        log::trace!(target: "validator_manager",
            "update_shards: mc_seqno={} mc_cc_seqno={} current_sessions={} future_sessions={}",
            mc_state.block_id().seq_no, master_cc_seqno,
            self.current_sessions.len(), self.future_sessions.len());

        if !self.update_validator_lists(&mc_state).await? {
            log::info!(target: "validator_manager",
                "VALIDATION_STATUS: not a validator (not in current or next set), disabling");
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
            self.current_sessions.keys().chain(self.future_sessions.keys()).cloned().collect();

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
                descr.file_hash,
            );

            if descr.before_split {
                let lr_shards = ident.split();
                match lr_shards {
                    Err(e) => log::error!(target: "validator_manager", "Cannot split shard: `{e}`"),
                    Ok((l, r)) => {
                        new_shards.insert(l, vec![top_block.clone()]);
                        new_shards.insert(r, vec![top_block.clone()]);
                        blocks_before_split.insert(top_block);
                    }
                }
            } else if descr.before_merge {
                let parent_shard = ident.merge();
                match parent_shard {
                    Err(e) => log::error!(target: "validator_manager", "Cannot merge shard: `{e}`"),
                    Ok(p) => {
                        let mut prev_blocks = match new_shards.get(&p) {
                            Some(pb) => pb.clone(),
                            None => vec![BlockIdExt::default(), BlockIdExt::default()],
                        };

                        // Add previous block for the shard: there are two parents for merge, so two prevs
                        let (_l, r) = p.split()?;
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
                FutureSplitMerge::Split { split_utime: time, interval: _interval } => {
                    if (time as u64) < cur_time + 60 {
                        match ident.split() {
                            Ok((l, r)) => {
                                future_shards.insert(l);
                                future_shards.insert(r);
                            }
                            Err(e) => log::error!(
                                target: "validator_manager",
                                "Cannot split shard {ident}: `{e}`"
                            ),
                        }
                    } else {
                        future_shards.insert(ident);
                    }
                }
                FutureSplitMerge::Merge { merge_utime: time, interval: _interval } => {
                    if (time as u64) < cur_time + 60 {
                        match ident.merge() {
                            Ok(p) => {
                                future_shards.insert(p);
                            }
                            Err(e) => log::error!(
                                target: "validator_manager",
                                "Cannot merge shard {ident}: `{e}`"
                            ),
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
        let mut owned_future_shards = 0usize;
        for (ident, (wc, next_cc_seqno, next_val_list_id)) in our_future_shards.iter() {
            if ident.is_masterchain() {
                mc_validators.append(&mut wc.validators.clone());
            }

            if let Some(local_id) = self.find_us_for_list(&wc.validators, next_val_list_id) {
                owned_future_shards += 1;
                let max_vertical_seqno = self.engine.hardforks().len() as u32;
                let new_session_info = Arc::new(GeneralSessionInfo {
                    shard: ident.clone(),
                    opts_hash: sessions_options.get_session_id_hash(&ident).clone(),
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

                // C++ parity: skip sessions in the destroyed set
                if self.destroyed_sessions.contains(&session_id) {
                    log::trace!(
                        target: "validator_manager",
                        "Skipping destroyed future session {:x} for shard {}", session_id, ident
                    );
                    continue;
                }

                let vsubset = wc.compute_validator_set(*next_cc_seqno)?;
                let current_session_options = sessions_options.get_session_options(&ident);
                gc_validator_sessions.remove(&session_id);

                // Select consensus type based on ConfigParam 30
                let consensus_options = self.select_consensus_options(
                    &ident,
                    mc_state.as_ref(),
                    current_session_options,
                    *next_cc_seqno,
                );

                if self.current_sessions.contains_key(&session_id) {
                    log::trace!(
                        target: "validator_manager",
                        "Future session {:x} for shard {} already in current_sessions, skipping",
                        session_id, ident
                    );
                    continue;
                }

                let is_new = !self.future_sessions.contains_key(&session_id);
                let group = self
                    .future_sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| {
                        let consensus_name = match &consensus_options {
                            ConsensusOptions::Simplex(_) => "simplex",
                            ConsensusOptions::Catchain(_) => "catchain",
                        };
                        log::info!(target: "validator_manager",
                            "SESSION_LIFECYCLE: create_future shard={} cc_seqno={} \
                            session_id={:x} consensus={}",
                            ident, next_cc_seqno, session_id, consensus_name);
                        metrics::counter!(
                            "ton_node_validator_session_created_total",
                            "consensus" => consensus_name
                        )
                        .increment(1);
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
                    })
                    .clone();

                if is_new && self.sync_complete {
                    log::debug!(target: "validator_manager",
                        "Pre-creating engine for future session shard={} cc_seqno={} session_id={:x}",
                        ident, next_cc_seqno, session_id);
                    let g = group.clone();
                    let sid = session_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = g.pre_create_engine().await {
                            log::error!(target: "validator_manager",
                                "SESSION_LIFECYCLE: pre_create_engine_failed session_id={:x} error={}", sid, e);
                        }
                    });
                }
            }
        }

        self.owned_current_shards = our_current_shards.len();
        self.owned_future_shards = owned_future_shards;

        // Stale-future culling: remove future entries whose shard already has a current
        // group with equal or higher cc_seqno, or whose shard is an ancestor/descendant
        // of a current shard with strictly higher cc_seqno.
        // C++ parity: equal + related conditions from manager.cpp update_shards().
        {
            let stale_ids: Vec<UInt256> = self
                .future_sessions
                .iter()
                .filter(|(_, fg)| {
                    self.current_sessions.values().any(|cg| {
                        let shards_equal = cg.shard() == fg.shard();
                        let shards_related = cg.shard().is_ancestor_for(fg.shard())
                            || fg.shard().is_ancestor_for(cg.shard());
                        let equal_condition = shards_equal && cg.cc_seqno() >= fg.cc_seqno();
                        let related_condition = shards_related && cg.cc_seqno() > fg.cc_seqno();
                        equal_condition || related_condition
                    })
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale_ids {
                if let Some(fg) = self.future_sessions.remove(&id) {
                    // C++ parity: destroyed_validator_sessions_.insert(id)
                    self.destroyed_sessions.insert(id.clone());
                    log::info!(target: "validator_manager",
                        "SESSION_LIFECYCLE: cull_stale_future shard={} cc_seqno={} session_id={:x} \
                        (superseded by active current session)",
                        fg.shard(), fg.cc_seqno(), id);
                    let cl = if fg.is_simplex() { "simplex" } else { "catchain" };
                    metrics::counter!(
                        "ton_node_validator_session_destroyed_total",
                        "consensus" => cl
                    )
                    .increment(1);
                    // C++ parity: IValidatorGroup::destroy
                    if let Err(e) = fg.stop(self.rt.clone(), true).await {
                        log::error!(target: "validator_manager",
                            "SESSION_LIFECYCLE: cull_stale_future_stop_failed session_id={:x}: {}",
                            id, e);
                    }
                }
            }
        }

        let mut precalc_split_queues_for: HashSet<BlockIdExt> = HashSet::new();
        for session in self.current_sessions.values().chain(self.future_sessions.values()) {
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

        // Notify simplex sessions with the currently applied top for their shard.
        // This drives C++-parity empty-block recovery logic and MC validation ordering.
        self.notify_simplex_sessions_applied_tops(mc_state.as_ref(), mc_state_extra);

        log::trace!(target: "validator_manager", "starting stop&remove");
        self.stop_and_remove_sessions(&gc_validator_sessions, true).await;

        if rotate_all_shards(mc_state_extra) {
            log::info!(target: "validator_manager", "New last rotation block: {}", last_masterchain_block);
            self.engine.save_last_rotation_block_id(last_masterchain_block)?;
            self.clear_destroyed_sessions()?;
        } else {
            self.persist_destroyed_sessions()?;
        }

        log::trace!(target: "validator_manager", "starting garbage collect");
        self.garbage_collect().await;
        log::trace!(target: "validator_manager", "exiting");
        Ok(())
    }

    /// Light per-iteration update: Prometheus gauges, engine timing, health warnings.
    /// Called on every wait-loop iteration (every few seconds).
    async fn stats(&mut self) {
        let validation_status = self.engine.validation_status();
        let in_current_set = self.validator_list_status.curr.is_some();

        let mut state_counts: HashMap<&'static str, u64> = HashMap::new();
        let mut stalled_count: u64 = 0;
        let mut simplex_count: u64 = 0;
        let mut catchain_count: u64 = 0;

        for group in self.current_sessions.values() {
            let status = group.get_status().await;
            let is_stalled = group.stalled.load(Ordering::Relaxed);
            *state_counts.entry(status.metric_label()).or_default() += 1;
            if is_stalled {
                stalled_count += 1;
            }
            if group.is_simplex() {
                simplex_count += 1;
            } else {
                catchain_count += 1;
            }
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
        for group in self.future_sessions.values() {
            let status = group.get_status().await;
            *state_counts.entry(status.metric_label()).or_default() += 1;
            if group.is_simplex() {
                simplex_count += 1;
            } else {
                catchain_count += 1;
            }
        }

        // Health warnings for operator attention
        if stalled_count > 0 {
            log::warn!(target: "validator_manager",
                "HEALTH_CHECK: {} session(s) stalled (validation queue inactive)", stalled_count);
        }
        if let Some(reason) = classify_no_current_session_health(
            in_current_set,
            validation_status,
            self.current_sessions.len(),
            self.owned_current_shards,
            self.owned_future_shards,
        ) {
            if reason.should_warn() {
                log::warn!(target: "validator_manager",
                    "HEALTH_CHECK: node is in current validator set but has no current sessions \
                     (reason={}, owned_current_shards={}, owned_future_shards={}, \
                     future_sessions={}, validation_status={:?})",
                    reason.label(),
                    self.owned_current_shards,
                    self.owned_future_shards,
                    self.future_sessions.len(),
                    validation_status);
            }
        }
        if validation_status.allows_validate() {
            let sync_count = state_counts.get("sync").copied().unwrap_or(0);
            let active_count = state_counts.get("active").copied().unwrap_or(0);
            if sync_count == 0 && active_count == 0 && !self.current_sessions.is_empty() {
                log::warn!(target: "validator_manager",
                    "HEALTH_CHECK: validation enabled but no current session reached sync yet \
                     (possible startup/session-start regression, validation_status={:?})",
                    validation_status);
            }
        }

        // Prometheus metrics
        metrics::gauge!("ton_node_validator_sessions_total", "role" => "current")
            .set(self.current_sessions.len() as f64);
        metrics::gauge!("ton_node_validator_sessions_total", "role" => "future")
            .set(self.future_sessions.len() as f64);
        metrics::gauge!("ton_node_validator_sessions_by_consensus", "type" => "simplex")
            .set(simplex_count as f64);
        metrics::gauge!("ton_node_validator_sessions_by_consensus", "type" => "catchain")
            .set(catchain_count as f64);
        for (state, count) in &state_counts {
            metrics::gauge!("ton_node_validator_sessions_by_state", "state" => *state)
                .set(*count as f64);
        }
        metrics::gauge!("ton_node_validator_group_stalled").set(stalled_count as f64);
        metrics::gauge!("ton_node_validator_sync_complete").set(self.sync_complete as u8 as f64);

        // Full metrics dump once per minute
        if self.last_metrics_dump.elapsed() >= Duration::from_secs(60) {
            self.last_metrics_dump = tokio::time::Instant::now();
            self.dump_metrics().await;
        }
    }

    /// Comprehensive metrics dump emitted once per minute.
    /// Structured for easy grep and readable operator dashboards.
    async fn dump_metrics(&self) {
        let now = SystemTime::now();
        let now_unix = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        let validation_status = self.engine.validation_status();
        let in_current_set = self.validator_list_status.curr.is_some();
        let in_next_set = self.validator_list_status.next.is_some();
        // ── Header: overall manager state ──
        let mut simplex_count = 0u32;
        let mut catchain_count = 0u32;
        let mut stalled_count = 0u32;
        let mut state_counts: HashMap<&'static str, u32> = HashMap::new();

        // Collect current session snapshots
        let mut current_snapshots: Vec<(SessionSnapshot, bool, bool, u64, u64)> = Vec::new();
        for group in self.current_sessions.values() {
            let snap = group.snapshot().await;
            let is_stalled = group.stalled.load(Ordering::Relaxed);
            let is_collating = group.is_collating();
            let last_val = group.last_validation_time();
            let last_col = group.last_collation_time();
            *state_counts.entry(snap.status.metric_label()).or_default() += 1;
            if is_stalled {
                stalled_count += 1;
            }
            if snap.consensus_type == super::consensus::ConsensusType::Simplex {
                simplex_count += 1;
            } else {
                catchain_count += 1;
            }
            current_snapshots.push((snap, is_stalled, is_collating, last_val, last_col));
        }

        // Collect future session snapshots
        let mut future_snapshots: Vec<SessionSnapshot> = Vec::new();
        for group in self.future_sessions.values() {
            let snap = group.snapshot().await;
            *state_counts.entry(snap.status.metric_label()).or_default() += 1;
            if snap.consensus_type == super::consensus::ConsensusType::Simplex {
                simplex_count += 1;
            } else {
                catchain_count += 1;
            }
            future_snapshots.push(snap);
        }

        let state_str: String =
            state_counts.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(" ");

        let mut lines = Vec::<String>::new();
        lines.push(format!(
            "=== VALIDATOR MANAGER METRICS (once/min) ===\n\
             \x20 validation_status={:?}  sync_complete={}\n\
             \x20 in_current_set={}  in_next_set={}\n\
             \x20 owned_subsets: current={} future={}\n\
             \x20 sessions: current={} future={} total={} (simplex={} catchain={})\n\
             \x20 by_state: [{}]  stalled={}",
            validation_status,
            self.sync_complete,
            in_current_set,
            in_next_set,
            self.owned_current_shards,
            self.owned_future_shards,
            self.current_sessions.len(),
            self.future_sessions.len(),
            self.current_sessions.len() + self.future_sessions.len(),
            simplex_count,
            catchain_count,
            state_str,
            stalled_count,
        ));

        if let Some(reason) = classify_no_current_session_health(
            in_current_set,
            validation_status,
            self.current_sessions.len(),
            self.owned_current_shards,
            self.owned_future_shards,
        ) {
            lines.push(format!(
                "  NO CURRENT SESSIONS: reason={} owned_current_shards={} \
                 owned_future_shards={} current_sessions={} future_sessions={}",
                reason.label(),
                self.owned_current_shards,
                self.owned_future_shards,
                self.current_sessions.len(),
                self.future_sessions.len(),
            ));
        }

        // ── Validator keys ──
        lines.push(String::from("  VALIDATOR KEYS:"));
        for (role, list_id_opt, utime_opt) in [
            (
                "current",
                &self.validator_list_status.curr,
                self.validator_list_status.curr_utime_since,
            ),
            ("next", &self.validator_list_status.next, self.validator_list_status.next_utime_since),
        ] {
            if let Some(list_id) = list_id_opt {
                let entry = self.validator_list_status.get_list(list_id);
                let context_ready =
                    self.engine.validator_network().has_validator_list_context(list_id);
                let key_strs: Vec<String> = entry
                    .map(|e| e.keys.iter().map(|k| base64_encode(k.id().data())).collect())
                    .unwrap_or_default();
                lines.push(format!(
                    "    [{}] list_id={:x} election_utime={} context_ready={} keys=[{}]",
                    role,
                    list_id,
                    utime_opt.map_or("-".to_string(), |u| u.to_string()),
                    context_ready,
                    key_strs.join(", "),
                ));
            } else {
                lines.push(format!("    [{}] not in set", role));
            }
        }

        // ── Config-level key bindings (election_id → validator_key, adnl_key) ──
        match self.engine.get_validator_key_bindings() {
            Ok(bindings) => {
                lines.push(format!("  KEY BINDINGS ({}):", bindings.len()));
                let mut seen_elections: HashMap<i32, usize> = HashMap::new();
                for (idx, b) in bindings.iter().enumerate() {
                    let adnl_str = b.validator_adnl_key_id.as_deref().unwrap_or("(none)");
                    lines.push(format!(
                        "    election_id={:<12} key={} adnl={} expire_at={}",
                        b.election_id, b.validator_key_id, adnl_str, b.expire_at,
                    ));
                    if let Some(prev_idx) = seen_elections.insert(b.election_id, idx) {
                        log::error!(
                            target: "validator_manager",
                            "KEY BINDING INVARIANT VIOLATION: duplicate election_id={}: \
                             binding[{}] and binding[{}] share the same election_id",
                            b.election_id, prev_idx, idx,
                        );
                    }
                    if b.validator_adnl_key_id.is_none() {
                        log::warn!(
                            target: "validator_manager",
                            "KEY BINDING: election_id={} has validator_key={} but no ADNL key bound",
                            b.election_id, b.validator_key_id,
                        );
                    }
                }
            }
            Err(e) => {
                lines.push(format!("  KEY BINDINGS: error retrieving: {e}"));
            }
        }

        // ── Current sessions detail ──
        if current_snapshots.is_empty() {
            lines.push(String::from("  CURRENT SESSIONS: (none)"));
        } else {
            lines.push(format!("  CURRENT SESSIONS ({}):", current_snapshots.len()));
            for (snap, is_stalled, is_collating, last_val, last_col) in &current_snapshots {
                let shard_str = format_shard_short(&snap.shard);
                let consensus_str =
                    if snap.consensus_type == super::consensus::ConsensusType::Simplex {
                        "splx"
                    } else {
                        "cch"
                    };
                let age = now.duration_since(snap.created_at).unwrap_or_default();
                let val_ago = format_time_ago(now_unix, *last_val);
                let col_ago = format_time_ago(now_unix, *last_col);
                let status_str = format!("{}", snap.status);
                let last_mc =
                    snap.last_accepted_mc_seqno.map_or("-".to_string(), |s| s.to_string());
                let phase_str = validation_state_phase_label(snap.status);
                lines.push(format!(
                    "    {:<8} cc={:<4} {:<4} {:<14} phase={:<13} rnd={:<4} collator={:<3} \
                     collating={:<3} stall={:<3} val_ago={:<6} col_ago={:<6} \
                     mc_init={:<6} mc_last={:<6} age={} id={:x}",
                    shard_str,
                    snap.cc_seqno,
                    consensus_str,
                    status_str,
                    phase_str,
                    snap.round,
                    if snap.is_collator { "yes" } else { "no" },
                    if *is_collating { "yes" } else { "no" },
                    if *is_stalled { "yes" } else { "no" },
                    val_ago,
                    col_ago,
                    snap.mc_initial_seqno,
                    last_mc,
                    format_duration_short(age),
                    snap.session_id,
                ));
            }
        }

        // ── Future sessions detail ──
        if future_snapshots.is_empty() {
            lines.push(String::from("  FUTURE SESSIONS: (none)"));
        } else {
            lines.push(format!("  FUTURE SESSIONS ({}):", future_snapshots.len()));
            for snap in &future_snapshots {
                let shard_str = format_shard_short(&snap.shard);
                let consensus_str =
                    if snap.consensus_type == super::consensus::ConsensusType::Simplex {
                        "splx"
                    } else {
                        "cch"
                    };
                let age = now.duration_since(snap.created_at).unwrap_or_default();
                lines.push(format!(
                    "    {:<8} cc={:<4} {:<4} {:<14} engine={:<3} key_seq={} age={} id={:x}",
                    shard_str,
                    snap.cc_seqno,
                    consensus_str,
                    format!("{}", snap.status),
                    if snap.has_engine { "yes" } else { "no" },
                    snap.key_seqno,
                    format_duration_short(age),
                    snap.session_id,
                ));
            }
        }

        lines.push(String::from("=== END VALIDATOR MANAGER METRICS ==="));
        log::info!(target: "validator_manager", "{}", lines.join("\n"));
    }

    fn read_catchain_config(&self, state: &ShardStateStuff) -> Result<CatchainConfig> {
        let state_extra = state.shard_state_extra()?;
        state_extra.config.catchain_config()
    }

    /// infinite loop with possible error cancellation
    async fn invoke(&mut self) -> Result<()> {
        self.load_destroyed_sessions()?;
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
            log::info!(
                target: "validator_manager",
                "Validator manager initialization: no last rotation block, \
                using last applied block: {last_applied_block_id}"
            );
            last_applied_block_handle.clone()
        };

        //let block_observer = self.initialize_block_observer(&last_applied_block_handle).await?;

        while !self.engine.check_stop() {
            log::trace!(
                target: "validator_manager",
                "Trying to load state for masterblock {}",
                mc_handle.id().seq_no
            );

            match self.engine.load_state(mc_handle.id()).await {
                Ok(mc_state) => {
                    let seqno = mc_handle.id().seq_no;
                    log::info!(target: "validator_manager", "Processing masterblock {seqno}");
                    log::trace!(
                        target: "validator_manager",
                        "Processing messages from masterblock {seqno}"
                    );
                    log::trace!(
                        target: "validator_manager",
                        "Updating shards according to masterblock {seqno}"
                    );
                    self.update_shards(mc_state).await?;
                    log::trace!(
                        target: "validator_manager",
                        "Shards for masterblock {seqno} updated"
                    );
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
                    log::info!(
                        target: "validator_manager",
                        "Processing masterblock {}: state not available, going forward",
                        mc_handle.id().seq_no
                    );
                }
            }

            mc_handle = loop {
                log::trace!(target: "validator_manager", "Checking stop engine");
                if self.engine.check_stop() {
                    log::trace!(
                        target: "validator_manager",
                        "Engine is stopped. Exiting from invocation loop (while loading block)"
                    );
                    return Ok(());
                }
                log::trace!(target: "validator_manager", "Checked stop engine: going on");
                self.stats().await;
                log::trace!(
                    target: "validator_manager",
                    "Waiting next applied masterblock after {}",
                    mc_handle.id().seq_no
                );
                match timeout(
                    self.config.update_interval,
                    self.engine.wait_next_applied_mc_block(&mc_handle, None),
                )
                .await
                {
                    Ok(r_res) => {
                        log::trace!(
                            target: "validator_manager",
                            "Got next applied master block (result): {}",
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

        log::info!(
            target: "validator_manager",
            "Engine is stopped. Exiting from invocation loop (while applying state)"
        );
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
        log::info!(
            target: "validator_manager",
            "checking if current node is a validator during {CHECK_VALIDATOR_TIMEOUT} secs"
        );
        engine.acquire_stop(Engine::MASK_SERVICE_VALIDATOR_MANAGER);
        while !engine.get_validator_status() {
            log::trace!(target: "validator_manager", "Not a validator, waiting...");
            let _ = engine.clear_last_rotation_block_id();
            let _ = engine.clear_destroyed_session_ids();
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

        loop {
            match manager.invoke().await {
                Ok(()) => break, // shutdown requested
                Err(e) => {
                    log::error!(
                        target: "validator_manager",
                        "Validator manager error, restarting: {e}"
                    );
                    if engine.check_stop() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }

        manager.stop_validation().await;
        log::info!(target: "validator_manager", "Exiting, validator manager is stopped");
        engine.release_stop(Engine::MASK_SERVICE_VALIDATOR_MANAGER);
    });
}

#[cfg(test)]
#[path = "tests/test_session_id.rs"]
mod tests;
