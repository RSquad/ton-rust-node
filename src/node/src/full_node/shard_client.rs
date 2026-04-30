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
use crate::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    engine::Engine,
    engine_traits::EngineOperations,
    error::NodeError,
    network::BlockBroadcastV2,
    shard_state::ShardStateStuff,
    validating_utils::{build_checked_data, fmt_block_id_short},
    validator::validator_utils::{calc_subset_for_masterchain, calc_subset_for_workchain_standard},
};
use std::{mem::drop, sync::Arc, time::Duration};
use ton_api::ton::ton_node::{blocksignature::BlockSignature, broadcast::BlockBroadcast};
use ton_block::{
    error, fail, BlockIdExt, BlockSignaturesPure, BlockSignaturesVariant, CryptoSignature,
    CryptoSignaturePair, Result, BASE_WORKCHAIN_ID,
};

pub fn start_masterchain_client(
    engine: Arc<dyn EngineOperations>,
    last_got_block_id: BlockIdExt,
) -> Result<tokio::task::JoinHandle<()>> {
    let join_handle = tokio::spawn(async move {
        engine.acquire_stop(Engine::MASK_SERVICE_MASTERCHAIN_CLIENT);
        while let Err(e) = load_master_blocks_cycle(engine.clone(), last_got_block_id.clone()).await
        {
            log::error!("Unexpected error in master blocks loading cycle: {:?}", e);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        engine.release_stop(Engine::MASK_SERVICE_MASTERCHAIN_CLIENT);
    });
    Ok(join_handle)
}

pub fn start_shards_client(
    engine: Arc<dyn EngineOperations>,
    shards_mc_block_id: BlockIdExt,
) -> Result<tokio::task::JoinHandle<()>> {
    let join_handle = tokio::spawn(async move {
        engine.acquire_stop(Engine::MASK_SERVICE_SHARDCHAIN_CLIENT);
        while let Err(e) = load_shard_blocks_cycle(engine.clone(), &shards_mc_block_id).await {
            log::error!("Unexpected error in shards client: {:?}", e);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        engine.release_stop(Engine::MASK_SERVICE_SHARDCHAIN_CLIENT);
    });
    Ok(join_handle)
}

// Remember about ShardStatesKeeper::MAX_CATCH_UP_DEPTH and ShardStateDb::MAX_QUEUE_LEN
pub const MC_MAX_SUPERIORITY: u32 = 500;

async fn load_master_blocks_cycle(
    engine: Arc<dyn EngineOperations>,
    mut last_got_block_id: BlockIdExt,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        if engine.check_stop() {
            break Ok(());
        }
        if let Some(shard_client) = engine.load_shard_client_mc_block_id()? {
            if shard_client.seq_no() < last_got_block_id.seq_no()
                && last_got_block_id.seq_no() - shard_client.seq_no() > MC_MAX_SUPERIORITY
            {
                log::info!(
                    "load_next_master_block (block {last_got_block_id}): waiting for shard client ({shard_client})");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        }
        last_got_block_id = match load_next_master_block(&engine, &last_got_block_id).await {
            Ok(id) => {
                attempt = 0;
                id
            }
            Err(e) => {
                log::error!(
                    "Error while load and apply next master block, prev: {}: attempt: {}, err: {:?}",
                    last_got_block_id,
                    attempt,
                    e
                );
                attempt += 1;
                // TODO make method to ban bad peer who gave bad block
                continue;
            }
        };
    }
}

async fn load_next_master_block(
    engine: &Arc<dyn EngineOperations>,
    prev_id: &BlockIdExt,
) -> Result<BlockIdExt> {
    log::debug!("load_next_master_block: prev block: {}", prev_id);
    if let Some(prev_handle) = engine.load_block_handle(prev_id)? {
        if prev_handle.has_next1() {
            let next_id = engine.load_block_next1(prev_id)?;
            log::debug!(
                "load_next_master_block: has_next1, will download_and_apply_block {}, prev: {}",
                next_id,
                prev_id
            );
            engine.clone().download_and_apply_block(&next_id, next_id.seq_no(), false).await?;
            return Ok(next_id);
        }
    } else {
        fail!("Cannot load handle for prev block {}", prev_id)
    };

    log::debug!("load_next_master_block: downloading next block... prev: {}", prev_id);
    let (block, proof) = engine.download_next_block(prev_id).await?;
    log::debug!("load_next_master_block: downloaded next block {}, prev: {}", block.id(), prev_id);
    if block.id().seq_no != prev_id.seq_no + 1 {
        fail!("Invalid next master block got: {}, prev: {}", block.id(), prev_id);
    }

    log::debug!("load_next_master_block: waiting for prev state {}", prev_id);
    let prev_state = engine.clone().wait_state(prev_id, None, true).await?;
    log::debug!("load_next_master_block: got prev state, checking proof for {}", block.id());
    proof.check_with_master_state(&prev_state)?;
    let mut next_handle = loop {
        if let Some(next_handle) = engine.load_block_handle(block.id())? {
            if !next_handle.has_data() {
                log::warn!(
                    "load_next_master_block: unitialized handle detected for block {}",
                    block.id()
                )
            } else {
                break next_handle;
            }
        }
        if let Some(next_handle) = engine.store_block(&block).await?.to_non_created() {
            break next_handle;
        } else {
            continue;
        }
    };
    if !next_handle.has_proof() {
        next_handle = engine
            .store_block_proof(block.id(), Some(next_handle), &proof)
            .await?
            .to_non_created()
            .ok_or_else(|| {
                error!(
                    "INTERNAL ERROR: load_next_master_block: bad result for store block {} proof",
                    block.id()
                )
            })?;
    }
    log::debug!("load_next_master_block: applying block {}", block.id());
    engine.clone().apply_block(&next_handle, &block, next_handle.id().seq_no(), false).await?;
    Ok(block.id().clone())
}

// TODO: We limited this window to 1 thread instead of 2 because of the issue with archives.
//       If we still need to process 2 parallel MC blocks or more, we should develop an algorithm
//       to mark correctly shard blocks with appropriate mc_seq_no despite of application order.
const SHARD_CLIENT_WINDOW: usize = 1;

async fn load_shard_blocks_cycle(
    engine: Arc<dyn EngineOperations>,
    shards_mc_block_id: &BlockIdExt,
) -> Result<()> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(SHARD_CLIENT_WINDOW));
    let mut mc_handle = engine.load_block_handle(shards_mc_block_id)?.ok_or_else(|| {
        error!("Cannot load handle for shard master block {}", shards_mc_block_id)
    })?;
    loop {
        if engine.check_stop() {
            break Ok(());
        }
        log::trace!("load_shard_blocks_cycle: mc block: {}", mc_handle.id());
        let r = match engine.wait_next_applied_mc_block(&mc_handle, Some(5_000)).await {
            Err(e) => {
                log::debug!("load_shard_blocks_cycle: no next mc block: {}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Ok(r) => r,
        };
        mc_handle = r.0;
        let mc_block = r.1;

        log::trace!("load_shard_blocks_cycle: waiting semaphore: {}", mc_block.id());
        let semaphore_permit = Arc::clone(&semaphore).acquire_owned().await?;

        log::trace!("load_shard_blocks_cycle: process next mc block: {}", mc_block.id());

        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            if let Err(e) = load_shard_blocks(engine.clone(), semaphore_permit, &mc_block).await {
                log::error!(
                    "FATAL!!! Unexpected error in shard blocks processing for mc block {}: {:?}",
                    mc_block.id(),
                    e
                );
            }
        });
    }
}

pub async fn load_shard_blocks(
    engine: Arc<dyn EngineOperations>,
    semaphore_permit: tokio::sync::OwnedSemaphorePermit,
    mc_block: &BlockStuff,
) -> Result<()> {
    fn start_apply_block_task(
        engine: Arc<dyn EngineOperations>,
        shard_block_id: BlockIdExt,
        mc_seq_no: u32,
        msg: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut attempt = 0;
            log::trace!("load_shard_blocks_cycle: {}, applying block...", msg);
            loop {
                if let Err(e) = Arc::clone(&engine)
                    .download_and_apply_block(&shard_block_id, mc_seq_no, false)
                    .await
                {
                    log::error!(
                        "Error while applying shard block (attempt {}) {}: {:?}",
                        attempt,
                        shard_block_id,
                        e
                    );
                    attempt += 1;
                    // TODO make method to ban bad peer who gave bad block
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if engine.check_stop() {
                        break;
                    }
                } else {
                    log::trace!("load_shard_blocks_cycle: {}, applied block", msg);
                    break;
                }
            }
        })
    }

    let mut apply_tasks = Vec::new();
    let mc_seq_no = mc_block.id().seq_no();

    // Apply full shard blocks (classic single wc config)
    for shard_block_id in mc_block.top_blocks(BASE_WORKCHAIN_ID)? {
        let msg = format!(
            "load_shard_blocks: mc block {}, shard block {}",
            mc_block.id(),
            shard_block_id
        );
        if let Some(shard_block_handle) = engine.load_block_handle(&shard_block_id)? {
            if shard_block_handle.is_applied() {
                continue;
            }
        }
        apply_tasks.push(start_apply_block_task(
            engine.clone(),
            shard_block_id.clone(),
            mc_seq_no,
            msg,
        ));
    }

    futures::future::join_all(apply_tasks)
        .await
        .into_iter()
        .find(|r| r.is_err())
        .unwrap_or(Ok(()))?;

    if engine.check_stop() {
        return Ok(());
    }

    log::trace!("load_shard_blocks_cycle: processed mc block: {}", mc_block.id());
    engine.save_shard_client_mc_block_id(mc_block.id())?;
    drop(semaphore_permit);
    Ok(())
}

pub const SHARD_BROADCAST_WINDOW: u32 = 8;

pub async fn process_block_broadcast(
    engine: &Arc<dyn EngineOperations>,
    broadcast: BlockBroadcast,
) -> Result<Option<BlockStuff>> {
    let block_descr = fmt_block_id_short(&broadcast.id);
    log::trace!("({}): process_block_broadcast: {}", block_descr, broadcast.id);
    if let Some(handle) = engine.load_block_handle(&broadcast.id)? {
        if handle.has_data() {
            #[cfg(feature = "telemetry")]
            {
                let duplicate = handle.got_by_broadcast();
                let unneeded = !duplicate;
                engine.full_node_telemetry().new_block_broadcast(
                    &broadcast.id,
                    duplicate,
                    unneeded,
                );
            }
            return Ok(None);
        }
    }
    #[cfg(feature = "telemetry")]
    engine.full_node_telemetry().new_block_broadcast(&broadcast.id, false, false);

    let is_master = broadcast.id.shard().is_masterchain();

    let last_applied_mc_state = engine
        .load_last_applied_mc_state()
        .await
        .map_err(|e| error!("INTERNAL ERROR: can't load last mc state: {}", e))?;

    let proof = BlockProofStuff::deserialize(&broadcast.id, broadcast.proof, !is_master)?;
    let (virt_block, _) = proof.virtualize_block()?;
    let block_info = virt_block.read_info()?;
    let prev_key_block_seqno = block_info.prev_key_block_seqno();
    if prev_key_block_seqno > last_applied_mc_state.block_id().seq_no() {
        log::debug!(
            "({}): Skipped block broadcast {} because it refers too new key block: {}, \
            but last processed mc block is {})",
            block_descr,
            broadcast.id,
            prev_key_block_seqno,
            last_applied_mc_state.block_id().seq_no()
        );
        return Ok(None);
    }

    validate_brodcast(
        broadcast.catchain_seqno as u32,
        broadcast.validator_set_hash as u32,
        &broadcast.signatures,
        &last_applied_mc_state,
        &broadcast.id,
    )?;

    log::trace!(
        "({}): validated that broadcast {} is from right validators set, last_applied_mc_state {}",
        block_descr,
        broadcast.id,
        last_applied_mc_state.block_id(),
    );

    // Build and save block and proof
    if is_master {
        proof.check_with_master_state(last_applied_mc_state.as_ref())?;
    } else {
        proof.check_proof_link()?;
    }

    let block =
        BlockStuff::deserialize_block_checked(broadcast.id.clone(), Arc::new(broadcast.data))?;

    let mut handle = if let Some(handle) = engine.store_block(&block).await?.to_updated() {
        handle
    } else {
        log::debug!(
            "({}): Skipped apply for block {} broadcast because block is already in processing",
            block_descr,
            block.id()
        );
        return Ok(None);
    };

    #[cfg(feature = "telemetry")]
    handle.set_got_by_broadcast(true);

    if !handle.has_proof() {
        let result = engine.store_block_proof(block.id(), Some(handle), &proof).await?;
        handle = if let Some(handle) = result.to_updated() {
            handle
        } else {
            log::debug!(
                "({}): Skipped apply for block {} broadcast because block is already in processing",
                block_descr,
                block.id()
            );
            return Ok(None);
        }
    }

    // Apply (only blocks that is not too new for us)
    if is_master {
        if block.id().seq_no() == last_applied_mc_state.block_id().seq_no() + 1 {
            engine.clone().apply_block(&handle, &block, block.id().seq_no(), false).await?;
        } else {
            log::debug!(
                "({}): Skipped apply for block broadcast {} because it is too new (last master block: {})",
                block_descr,
                block.id(), last_applied_mc_state.block_id().seq_no()
            )
        }
    } else {
        let master_ref = block.block()?.read_info()?.read_master_ref()?.ok_or_else(|| {
            NodeError::InvalidData(format!(
                "Block {} doesn't contain masterchain block extra",
                block.id(),
            ))
        })?;
        let shard_client_mc_block_id = engine
            .load_shard_client_mc_block_id()?
            .ok_or_else(|| error!("INTERNAL ERROR: No shard client MC block after sync"))?;
        if shard_client_mc_block_id.seq_no() + SHARD_BROADCAST_WINDOW >= master_ref.master.seq_no {
            engine
                .clone()
                .apply_block(&handle, &block, shard_client_mc_block_id.seq_no(), true)
                .await?;
        } else {
            log::debug!(
                "({}): Skipped pre-apply for block broadcast {} because it refers to master block {}, but shard client is on {}",
                block_descr,
                block.id(), master_ref.master.seq_no, shard_client_mc_block_id.seq_no()
            )
        }
    }
    Ok(Some(block))
}

fn validate_brodcast(
    cc_seqno: u32,
    validator_set_hash: u32,
    signatures: &[BlockSignature],
    mc_state: &ShardStateStuff,
    block_id: &BlockIdExt,
) -> Result<()> {
    let config = mc_state.config_params()?;
    let val_set = config.validator_set()?;

    // build validator set
    let subset = if block_id.shard().is_masterchain() {
        calc_subset_for_masterchain(&val_set, config, cc_seqno)?
    } else {
        let subset =
            calc_subset_for_workchain_standard(&val_set, config, block_id.shard(), cc_seqno)?;
        subset
    };

    if subset.short_hash != validator_set_hash as u32 {
        fail!(NodeError::InvalidData(format!(
            "Bad validator set hash in broadcast with block {}, calculated: {}, found: {}",
            block_id, subset.short_hash, validator_set_hash
        )));
    }

    // extract signatures - build ton_block::BlockSignaturesPure

    let mut blk_pure_signatures = BlockSignaturesPure::default();
    for api_sig in signatures {
        blk_pure_signatures.add_sigpair(CryptoSignaturePair {
            node_id_short: api_sig.who.clone(),
            sign: CryptoSignature::from_bytes(&api_sig.signature)?,
        });
    }

    // Check signatures
    let checked_data =
        ton_block::Block::build_data_for_sign(&block_id.root_hash, &block_id.file_hash);
    let total_weight: u64 = subset.validators.iter().map(|v| v.weight).sum();
    let weight =
        blk_pure_signatures.check_signatures(&subset.validators, &checked_data).map_err(|err| {
            NodeError::InvalidData(format!(
                "Bad signatures in broadcast with block {}: {}",
                block_id, err
            ))
        })?;

    if weight * 3 <= total_weight * 2 {
        fail!(NodeError::InvalidData(format!(
            "Too small signatures weight in broadcast with block {}",
            block_id,
        )));
    }

    Ok(())
}

/// Validate V2 block broadcast with variant signature support.
///
/// For Ordinary signatures: Uses standard block data signing
/// For Simplex signatures: Uses session-aware signing with CandidateHashData
fn validate_brodcast_v2(
    signatures: &BlockSignaturesVariant,
    mc_state: &ShardStateStuff,
    block_id: &BlockIdExt,
) -> Result<()> {
    let cc_seqno = signatures.validator_info().catchain_seqno;
    let validator_set_hash = signatures.validator_info().validator_list_hash_short;

    let config = mc_state.config_params()?;
    let val_set = config.validator_set()?;

    // Build validator set
    let subset = if block_id.shard().is_masterchain() {
        calc_subset_for_masterchain(&val_set, config, cc_seqno)?
    } else {
        calc_subset_for_workchain_standard(&val_set, config, block_id.shard(), cc_seqno)?
    };

    if subset.short_hash != validator_set_hash as u32 {
        fail!(NodeError::InvalidData(format!(
            "Bad validator set hash in V2 broadcast with block {}, calculated: {}, found: {}",
            block_id, subset.short_hash, validator_set_hash
        )));
    }

    // Use variant-aware signature checking
    let data_to_verify = build_checked_data(signatures, block_id)?;

    let total_weight: u64 = subset.validators.iter().map(|v| v.weight).sum();
    let weight = signatures
        .pure_signatures()
        .check_signatures(&subset.validators, &data_to_verify)
        .map_err(|err| {
            NodeError::InvalidData(format!(
                "Bad signatures in V2 broadcast with block {}: {}",
                block_id, err
            ))
        })?;

    if weight * 3 <= total_weight * 2 {
        fail!(NodeError::InvalidData(format!(
            "Too small signatures weight in V2 broadcast with block {}",
            block_id,
        )));
    }

    Ok(())
}

/// Process V2 block broadcast with variant signature support.
///
/// Similar to `process_block_broadcast` but handles `BlockSignaturesVariant`
/// which supports both Ordinary (catchain) and Simplex signature schemes.
pub async fn process_block_broadcast_v2(
    engine: &Arc<dyn EngineOperations>,
    broadcast: BlockBroadcastV2,
) -> Result<Option<BlockStuff>> {
    let block_descr = fmt_block_id_short(&broadcast.id);
    log::trace!("({}): process_block_broadcast_v2: {}", block_descr, broadcast.id);

    // Check if block already exists
    if let Some(handle) = engine.load_block_handle(&broadcast.id)? {
        if handle.has_data() {
            #[cfg(feature = "telemetry")]
            {
                let duplicate = handle.got_by_broadcast();
                let unneeded = !duplicate;
                engine.full_node_telemetry().new_block_broadcast(
                    &broadcast.id,
                    duplicate,
                    unneeded,
                );
            }
            return Ok(None);
        }
    }
    #[cfg(feature = "telemetry")]
    engine.full_node_telemetry().new_block_broadcast(&broadcast.id, false, false);

    let is_master = broadcast.id.shard().is_masterchain();

    let last_applied_mc_state = engine
        .load_last_applied_mc_state()
        .await
        .map_err(|e| error!("INTERNAL ERROR: can't load last mc state: {}", e))?;

    let proof = BlockProofStuff::deserialize(&broadcast.id, broadcast.proof, !is_master)?;
    let (virt_block, _) = proof.virtualize_block()?;
    let block_info = virt_block.read_info()?;
    let prev_key_block_seqno = block_info.prev_key_block_seqno();

    if prev_key_block_seqno > last_applied_mc_state.block_id().seq_no() {
        log::debug!(
            "({}): Skipped block broadcast V2 {} because it refers too new key block: {}, \
            but last processed mc block is {})",
            block_descr,
            broadcast.id,
            prev_key_block_seqno,
            last_applied_mc_state.block_id().seq_no()
        );
        return Ok(None);
    }

    // Validate with V2 signature support
    validate_brodcast_v2(&broadcast.signatures, &last_applied_mc_state, &broadcast.id)?;

    log::trace!(
        "({}): validated that V2 broadcast {} is from right validators set, last_applied_mc_state {}",
        block_descr,
        broadcast.id,
        last_applied_mc_state.block_id(),
    );

    // Build and save block and proof
    if is_master {
        proof.check_with_master_state(last_applied_mc_state.as_ref())?;
    } else {
        proof.check_proof_link()?;
    }

    let block =
        BlockStuff::deserialize_block_checked(broadcast.id.clone(), Arc::new(broadcast.data))?;

    let mut handle = if let Some(handle) = engine.store_block(&block).await?.to_updated() {
        handle
    } else {
        log::debug!(
            "({}): Skipped apply for block {} V2 broadcast because block is already in processing",
            block_descr,
            block.id()
        );
        return Ok(None);
    };

    #[cfg(feature = "telemetry")]
    handle.set_got_by_broadcast(true);

    if !handle.has_proof() {
        let result = engine.store_block_proof(block.id(), Some(handle), &proof).await?;
        handle = if let Some(handle) = result.to_updated() {
            handle
        } else {
            log::debug!(
                "({}): Skipped apply for block {} V2 broadcast because block is already in processing",
                block_descr,
                block.id()
            );
            return Ok(None);
        }
    }

    // Apply (only blocks that are not too new for us) - same as V1
    if is_master {
        if block.id().seq_no() == last_applied_mc_state.block_id().seq_no() + 1 {
            engine.clone().apply_block(&handle, &block, block.id().seq_no(), false).await?;
        } else {
            log::debug!(
                "({}): Skipped apply for V2 broadcast {} because it is too new (last master block: {})",
                block_descr,
                block.id(),
                last_applied_mc_state.block_id().seq_no()
            )
        }
    } else {
        let master_ref = block.block()?.read_info()?.read_master_ref()?.ok_or_else(|| {
            NodeError::InvalidData(format!(
                "Block {} doesn't contain masterchain block extra",
                block.id(),
            ))
        })?;
        let shard_client_mc_block_id = engine
            .load_shard_client_mc_block_id()?
            .ok_or_else(|| error!("INTERNAL ERROR: No shard client MC block after sync"))?;
        if shard_client_mc_block_id.seq_no() + SHARD_BROADCAST_WINDOW >= master_ref.master.seq_no {
            engine
                .clone()
                .apply_block(&handle, &block, shard_client_mc_block_id.seq_no(), true)
                .await?;
        } else {
            log::debug!(
                "({}): Skipped pre-apply for V2 broadcast {} because it refers to master block {}, but shard client is on {}",
                block_descr,
                block.id(),
                master_ref.master.seq_no,
                shard_client_mc_block_id.seq_no()
            )
        }
    }
    Ok(Some(block))
}
