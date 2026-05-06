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
#![allow(clippy::too_many_arguments)]

use super::{
    consensus::{BlockPayloadPtr, PublicKey, PublicKeyHash, ValidatorBlockCandidate},
    validator_utils::{
        pairvec_to_cryptopair_vec, validator_query_candidate_to_validator_block_candidate,
    },
};
use crate::{
    block::BlockStuff,
    collator_test_bundle::CollatorTestBundle,
    config::CollatorTestBundlesConfig,
    engine_traits::EngineOperations,
    shard_state::ShardStateStuff,
    validating_utils::fmt_next_block_descr,
    validator::{
        collator::{CollateResult, Collator},
        state_resolver_cache::StateResolverCache,
        validate_query::ValidateQuery,
        validator_group::PipelineContext,
        validator_utils::PrevBlockHistory,
        BlockCandidate, CollatorSettings,
    },
};
use std::{sync::Arc, time::SystemTime};
use ton_block::{
    Block, BlockIdExt, BlockSignaturesVariant, Cell, Deserializable, Message, Result, ShardIdent,
    UInt256, UsageTree, ValidatorSet,
};

pub async fn run_validate_query_any_candidate(
    block_candidate: BlockCandidate,
    engine: Arc<dyn EngineOperations>,
    pipeline_context: PipelineContext,
    state_resolver_cache: Arc<tokio::sync::Mutex<StateResolverCache>>,
    is_simplex: bool,
) -> Result<SystemTime> {
    let block_id = block_candidate.block_id.clone();
    let block_data = block_candidate.data.clone();
    let real_block = Block::construct_from_bytes(&block_data)?;
    let info = real_block.read_info()?;
    let prev_blocks_ids = info.read_prev_ids()?;
    let (_, master_ref) = info.read_master_id()?.master_block_id();
    let mc_state = engine.load_state(&master_ref).await?;
    let mc_state_extra = mc_state.shard_state_extra()?;
    let mut cc_seqno_with_delta = 0;
    let cc_seqno_from_state = if info.shard().is_masterchain() {
        mc_state_extra.validator_info.catchain_seqno
    } else {
        mc_state_extra.shards.calc_shard_cc_seqno(info.shard())?
    };
    let nodes = crate::validator::validator_utils::compute_validator_set_cc(
        &mc_state,
        info.shard(),
        engine.now(),
        cc_seqno_from_state,
        &mut cc_seqno_with_delta,
    )?;
    let validator_set = ValidatorSet::with_cc_seqno(0, 0, 0, cc_seqno_with_delta, nodes)?;

    let labels = [("shard", info.shard().to_string())];
    metrics::gauge!("ton_node_validator_active", &labels).increment(1.0);

    // For MC blocks, min_ref_mc_seqno from the header includes the block's own seqno,
    // which is newer than its parent. Use the loaded MC state seqno (= prev MC block)
    // to match the legacy run_validate_query semantics.
    let min_mc_seqno =
        if info.shard().is_masterchain() { master_ref.seq_no() } else { info.min_ref_mc_seqno() };

    let test_bundles_config = &engine.test_bundles_config().validator;
    let bundle_block = test_bundles_config.is_enable().then(|| block_candidate.clone());
    let bundle_prevs = bundle_block.as_ref().map(|_| prev_blocks_ids.clone());

    let query = ValidateQuery::new(
        info.shard().clone(),
        min_mc_seqno,
        prev_blocks_ids,
        pipeline_context,
        if is_simplex { Some(state_resolver_cache.clone()) } else { None },
        block_candidate,
        validator_set,
        engine.clone(),
        false,
        true,
        is_simplex,
    );
    let validator_result = query.try_validate().await;

    metrics::gauge!("ton_node_validator_active", &labels).decrement(1.0);

    if let (Err(err), Some(block), Some(prevs)) = (&validator_result, bundle_block, bundle_prevs) {
        let err_str = err.to_string();
        if test_bundles_config.need_to_build_for(&err_str) {
            let id = block.block_id.clone();
            spawn_build_test_bundle(
                Some(block),
                test_bundles_config,
                &id,
                prevs,
                None,
                Vec::new(),
                Some(err_str),
                fmt_next_block_descr(&block_id),
                engine.clone(),
            );
        }
    }

    match validator_result {
        Ok(next_state_opt) => {
            metrics::counter!("ton_node_validator_successes_total", &labels).increment(1);

            if is_simplex {
                if let Some(next_state) = next_state_opt {
                    state_resolver_cache.lock().await.store_validated_state(&block_id, next_state);
                }
            }

            // Store block data so accept_block_routine can find it without a network download.
            // Note: download_and_apply_block_worker also requires a proof/link on the handle
            // to skip the download path; storing data alone is not sufficient for that path.
            if let Err(e) = store_validated_block(&engine, &block_id, &block_data).await {
                log::warn!(
                    "run_validate_query_any_candidate: failed to store validated block \
                     for {} (non-fatal): {}",
                    block_id,
                    e
                );
            }

            Ok(SystemTime::now())
        }
        Err(e) => {
            metrics::counter!("ton_node_validator_failures_total", &labels).increment(1);

            #[cfg(feature = "telemetry")]
            engine.validator_telemetry().failed_attempt(info.shard(), &e.to_string());

            Err(e)
        }
    }
}

/// Store block data after successful validation so that `accept_block_routine`
/// can find it without a network download.
///
/// Note: `download_and_apply_block_worker` requires both data AND a proof/link
/// on the handle to skip the download path; storing data alone is not sufficient
/// for that path.
///
/// We intentionally store only block data, NOT the computed state. Storing the
/// state would set `handle.has_state() == true`, causing `accept_block_routine`
/// to return early and skip MC block application / shard block broadcasting.
/// The state will be computed during normal block application instead.
async fn store_validated_block(
    engine: &Arc<dyn EngineOperations>,
    block_id: &BlockIdExt,
    block_data: &[u8],
) -> Result<()> {
    let block_stuff =
        BlockStuff::deserialize_block(block_id.clone(), Arc::new(block_data.to_vec()))?;
    engine.store_block(&block_stuff).await?;
    log::debug!(
        "store_validated_block: stored block data for optimistically validated block {}",
        block_id
    );
    Ok(())
}

pub async fn run_validate_query(
    shard: ShardIdent,
    _min_ts: SystemTime,
    min_masterchain_block_id: BlockIdExt,
    prev: &PrevBlockHistory,
    block: BlockCandidate,
    set: ValidatorSet,
    engine: Arc<dyn EngineOperations>,
    is_simplex: bool,
) -> Result<SystemTime> {
    let next_block_descr = fmt_next_block_descr(&block.block_id);

    log::info!(
        target: "validator",
        "({}): before validator query shard: {}, min: {}",
        next_block_descr,
        shard,
        min_masterchain_block_id,
    );

    let labels = [("shard", shard.to_string())];
    metrics::gauge!("ton_node_validator_active", &labels).increment(1.0);

    let test_bundles_config = &engine.test_bundles_config().validator;
    let bundle_block = test_bundles_config.is_enable().then(|| block.clone());

    let validator_result = ValidateQuery::new(
        shard.clone(),
        min_masterchain_block_id.seq_no(),
        prev.get_prevs().to_vec(),
        Default::default(),
        None,
        block,
        set,
        engine.clone(),
        false,
        true,
        is_simplex,
    )
    .try_validate()
    .await;

    if let (Err(err), Some(block)) = (&validator_result, bundle_block) {
        let err_str = err.to_string();
        if test_bundles_config.need_to_build_for(&err_str) {
            let id = block.block_id.clone();
            spawn_build_test_bundle(
                Some(block),
                test_bundles_config,
                &id,
                prev.get_prevs().to_vec(),
                None,
                Vec::new(),
                Some(err_str),
                next_block_descr.clone(),
                engine.clone(),
            );
        }
    }

    metrics::gauge!("ton_node_validator_active", &labels).decrement(1.0);

    match validator_result {
        Ok(_) => {
            metrics::counter!("ton_node_validator_successes_total", &labels).increment(1);
            Ok(SystemTime::now())
        }
        Err(e) => {
            metrics::counter!("ton_node_validator_failures_total", &labels).increment(1);

            #[cfg(feature = "telemetry")]
            engine.validator_telemetry().failed_attempt(&shard, &e.to_string());

            Err(e)
        }
    }
}

pub async fn run_accept_block_query(
    id: BlockIdExt,
    data: Option<Vec<u8>>,
    prev: Vec<BlockIdExt>,
    set: ValidatorSet,
    signatures: BlockSignaturesVariant,
    approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    send_broadcast: bool,
    engine: Arc<dyn EngineOperations>,
) -> Result<()> {
    let approve_sigs = pairvec_to_cryptopair_vec(approve_signatures)?;
    super::accept_block::accept_block(
        id,
        data,
        prev,
        set,
        signatures,
        approve_sigs,
        send_broadcast,
        engine,
    )
    .await
}

pub async fn run_collate_query(
    shard: ShardIdent,
    min_ts: SystemTime,
    min_mc_seqno: u32,
    prev: &PrevBlockHistory,
    pipeline_context: PipelineContext,
    state_resolver_cache: Arc<tokio::sync::Mutex<StateResolverCache>>,
    collator_id: PublicKey,
    set: ValidatorSet,
    engine: Arc<dyn EngineOperations>,
    is_simplex: bool,
) -> Result<(Arc<ValidatorBlockCandidate>, Arc<ShardStateStuff>, Block, Cell)> {
    let labels = [("shard", shard.to_string())];
    metrics::gauge!("ton_node_collator_active", &labels).increment(1.0);

    let next_block_descr = prev.get_next_block_descr(None); //fmt_next_block_descr_from_next_seqno(&shard, get_first_block_seqno_after_prevs(&prev));

    let collator = Collator::new(
        shard.clone(),
        min_mc_seqno,
        prev,
        pipeline_context,
        state_resolver_cache,
        set,
        UInt256::from(collator_id.pub_key()?),
        engine.clone(),
        None,
        CollatorSettings {
            is_simplex,
            min_gen_utime_ms: Some(
                min_ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis()
                    as u64,
            ),
            ..Default::default()
        },
    )?;
    let collate_result = collator.collate().await;

    let labels = [("shard", shard.to_string())];
    metrics::gauge!("ton_node_collator_active", &labels).decrement(1.0);
    let mut usage_tree_opt = None;
    let mut accepted_external_messages = Vec::new();
    let test_bundles_config = &engine.test_bundles_config().collator;

    let err = match collate_result {
        Ok(CollateResult::Ok { candidate, new_state, new_block, block_root, usage_tree }) => {
            let new_state = ShardStateStuff::from_state(
                candidate.block_id.clone(),
                new_state,
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry(),
                engine.engine_allocated(),
            )?;
            metrics::counter!("ton_node_collator_successes_total", &labels).increment(1);

            if test_bundles_config.build_all() {
                spawn_build_test_bundle(
                    Some(candidate.clone()),
                    test_bundles_config,
                    &candidate.block_id,
                    prev.get_prevs().to_vec(),
                    Some(usage_tree),
                    Vec::new(),
                    None,
                    next_block_descr.clone(),
                    engine.clone(),
                );
            }

            return Ok((
                validator_query_candidate_to_validator_block_candidate(collator_id, candidate),
                new_state,
                new_block,
                block_root,
            ));
        }
        Ok(CollateResult::Err { usage_tree, external_messages, err }) => {
            usage_tree_opt = Some(usage_tree);
            accepted_external_messages = external_messages;
            err
        }
        Err(err) => err,
    };

    let labels = [("shard", shard.to_string())];
    metrics::counter!("ton_node_collator_failures_total", &labels).increment(1);
    let err_str = if test_bundles_config.is_enable() { err.to_string() } else { String::default() };

    #[cfg(feature = "telemetry")]
    engine.collator_telemetry().failed_attempt(&shard, &err_str);

    if test_bundles_config.is_enable() && test_bundles_config.need_to_build_for(&err_str) {
        let id = prev.get_next_block_id(&UInt256::default(), &UInt256::default());
        spawn_build_test_bundle(
            None,
            test_bundles_config,
            &id,
            prev.get_prevs().to_vec(),
            usage_tree_opt,
            accepted_external_messages,
            Some(err_str),
            next_block_descr,
            engine.clone(),
        );
    }
    Err(err)
}

fn spawn_build_test_bundle(
    candidate: Option<BlockCandidate>,
    config: &CollatorTestBundlesConfig,
    id: &BlockIdExt,
    prev_blocks_ids: Vec<BlockIdExt>,
    usage_tree: Option<UsageTree>,
    external_messages: Vec<(Arc<Message>, UInt256)>,
    notes: Option<String>,
    next_block_descr: String,
    engine: Arc<dyn EngineOperations>,
) {
    if CollatorTestBundle::exists(config.path(), id) {
        return;
    }
    let path = config.path().to_string();
    let id = id.clone();

    tokio::spawn(async move {
        let result = if let Some(candidate) = candidate {
            CollatorTestBundle::build_for_validating_block(
                &engine,
                &PrevBlockHistory::with_prevs(id.shard(), prev_blocks_ids),
                candidate,
            )
            .await
        } else {
            CollatorTestBundle::build_for_collating_block(
                &engine,
                prev_blocks_ids,
                usage_tree,
                external_messages,
            )
            .await
        };

        match result {
            Err(e) => log::error!(
                "({}): Error while test bundle for {} building: {}",
                next_block_descr,
                id,
                e
            ),
            Ok(mut b) => {
                if let Some(notes) = notes {
                    b.set_notes(notes);
                }
                if let Err(e) = b.save(&path) {
                    log::error!(
                        "({}): Error while test bundle for {} saving: {}",
                        next_block_descr,
                        id,
                        e
                    );
                } else {
                    log::info!("({}): Built test bundle for {}", next_block_descr, id);
                }
            }
        }
    });
}
