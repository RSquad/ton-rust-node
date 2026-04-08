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
    engine_traits::EngineOperations,
    shard_state::ShardStateStuff,
    validating_utils::fmt_next_block_descr,
    validator::{
        collator::{CollateResult, Collator},
        validate_query::ValidateQuery,
        validator_group::PipelineContext,
        validator_utils::PrevBlockHistory,
        BlockCandidate, CollatorSettings,
    },
};
use std::{sync::Arc, time::SystemTime};
use ton_block::{
    Block, BlockIdExt, BlockSignaturesVariant, Cell, Deserializable, Result, ShardIdent, UInt256,
    ValidatorSet,
};

pub async fn run_validate_query_any_candidate(
    block_candidate: BlockCandidate,
    engine: Arc<dyn EngineOperations>,
    pipeline_context: PipelineContext,
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

    let query = ValidateQuery::new(
        info.shard().clone(),
        min_mc_seqno,
        prev_blocks_ids,
        pipeline_context,
        block_candidate,
        validator_set,
        engine.clone(),
        false,
        true,
        is_simplex,
    );
    let validator_result = query.try_validate().await;

    metrics::gauge!("ton_node_validator_active", &labels).decrement(1.0);

    match validator_result {
        Ok(_next_state) => {
            metrics::counter!("ton_node_validator_successes_total", &labels).increment(1);

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
    let validator_result = if !test_bundles_config.is_enable() {
        ValidateQuery::new(
            shard.clone(),
            min_masterchain_block_id.seq_no(),
            prev.get_prevs().to_vec(),
            Default::default(),
            block,
            set,
            engine.clone(),
            false,
            true,
            is_simplex,
        )
        .try_validate()
        .await
    } else {
        let query = ValidateQuery::new(
            shard.clone(),
            min_masterchain_block_id.seq_no(),
            prev.get_prevs().to_vec(),
            Default::default(),
            block.clone(),
            set,
            engine.clone(),
            false,
            true,
            is_simplex,
        );
        let validator_result = query.try_validate().await;
        if let Err(err) = &validator_result {
            let err_str = err.to_string();
            if test_bundles_config.need_to_build_for(&err_str) {
                let id = block.block_id.clone();
                if !CollatorTestBundle::exists(test_bundles_config.path(), &id) {
                    let path = test_bundles_config.path().to_string();
                    let engine = engine.clone();
                    let prev = prev.clone();
                    tokio::spawn(async move {
                        match CollatorTestBundle::build_for_validating_block(&engine, &prev, block)
                            .await
                        {
                            Err(e) => log::error!(
                                "({}): Error while test bundle for {} building: {}",
                                next_block_descr,
                                id,
                                e
                            ),
                            Ok(mut b) => {
                                b.set_notes(err_str);
                                if let Err(e) = b.save(&path) {
                                    log::error!(
                                        "({}): Error while test bundle for {} saving: {}",
                                        next_block_descr,
                                        id,
                                        e
                                    )
                                } else {
                                    log::info!(
                                        "({}): Built test bundle for {}",
                                        next_block_descr,
                                        id
                                    )
                                }
                            }
                        }
                    });
                }
            }
        };
        validator_result
    };

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
    _min_ts: SystemTime,
    min_mc_seqno: u32,
    prev: &PrevBlockHistory,
    pipeline_context: PipelineContext,
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
        set,
        UInt256::from(collator_id.pub_key()?),
        engine.clone(),
        None,
        CollatorSettings { is_simplex, ..Default::default() },
    )?;
    let collate_result = collator.collate().await;

    let labels = [("shard", shard.to_string())];
    metrics::gauge!("ton_node_collator_active", &labels).decrement(1.0);
    let mut usage_tree_opt = None;

    let err = match collate_result {
        Ok(CollateResult::Ok { candidate, new_state, new_block, block_root, .. }) => {
            let new_state = ShardStateStuff::from_state(
                candidate.block_id.clone(),
                new_state,
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry(),
                engine.engine_allocated(),
            )?;
            metrics::counter!("ton_node_collator_successes_total", &labels).increment(1);
            return Ok((
                validator_query_candidate_to_validator_block_candidate(collator_id, candidate),
                new_state,
                new_block,
                block_root,
            ));
        }
        Ok(CollateResult::Err { usage_tree, err }) => {
            usage_tree_opt = Some(usage_tree);
            err
        }
        Err(err) => err,
    };

    let labels = [("shard", shard.to_string())];
    metrics::counter!("ton_node_collator_failures_total", &labels).increment(1);
    let test_bundles_config = &engine.test_bundles_config().collator;
    let err_str = if test_bundles_config.is_enable() { err.to_string() } else { String::default() };

    #[cfg(feature = "telemetry")]
    engine.collator_telemetry().failed_attempt(&shard, &err_str);

    if test_bundles_config.is_enable() && test_bundles_config.need_to_build_for(&err_str) {
        let id = prev.get_next_block_id(&UInt256::default(), &UInt256::default());
        let prev_vec = prev.get_prevs().to_vec();

        if !CollatorTestBundle::exists(test_bundles_config.path(), &id) {
            let path = test_bundles_config.path().to_string();
            let engine = engine.clone();
            tokio::spawn(async move {
                match CollatorTestBundle::build_for_collating_block(
                    &engine,
                    prev_vec.to_vec(),
                    usage_tree_opt,
                )
                .await
                {
                    Err(e) => log::error!(
                        "({}): Error while test bundle for {} building: {}",
                        next_block_descr,
                        id,
                        e
                    ),
                    Ok(mut b) => {
                        b.set_notes(err_str.to_string());
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
    }
    Err(err)
}
