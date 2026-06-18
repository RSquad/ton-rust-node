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
    block::{BlockIdExtExtention, BlockStuff},
    internal_db::{
        InternalDb, ARCHIVES_GC_BLOCK, LAST_APPLIED_MC_BLOCK, LAST_ROTATION_MC_BLOCK,
        PSS_KEEPER_MC_BLOCK, SHARD_CLIENT_MC_BLOCK,
    },
};
use std::{
    fs::{remove_file, write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use storage::{cell_db::BROKEN_CELL_BEACON_FILE, traits::Serializable};
use ton_block::{error, fail, BlockIdExt, Error, Result, MASTERCHAIN_ID};

const UNEXPECTED_TERMINATION_BEACON_FILE: &str = "ton_node.running";
const RESTORING_BEACON_FILE: &str = "ton_node.restoring";
const LAST_MC_BLOCKS: u32 = 100;
const SHARD_CLIENT_MC_BLOCK_CANDIDATES: u32 = 300;

pub async fn check_db(
    mut db: InternalDb,
    processed_wc: i32,
    restore_db_enabled: bool,
    force: bool,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
    is_broken: Option<&AtomicBool>,
) -> Result<InternalDb> {
    async fn force_db_reset(
        err: Error,
        check_stop: &(dyn Fn() -> Result<()> + Sync),
        is_broken: Option<&AtomicBool>,
    ) -> ! {
        if let Some(is_broken) = is_broken {
            is_broken.store(true, Ordering::Relaxed)
        }
        log::error!("Error while restoring database: {}. Need to clear db and re-sync node.", err);
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if check_stop().is_err() {
                std::process::exit(0xFF)
            }
        }
    }

    let unexpected_termination = check_unexpected_termination(&db.config.db_directory);
    let restoring = check_restoring(&db.config.db_directory);

    if unexpected_termination || restoring || force {
        if force {
            log::info!("Starting check & restore db process forcedly (with cells db refilling)");
        } else if restore_db_enabled {
            log::warn!(
                "Previous node run was unexpectedly terminated, \
                starting check & restore process..."
            );
        } else {
            if unexpected_termination {
                log::warn!(
                    "Previous node run was terminated unexpectedly, \
                    but 'restore_db' option in node config is 'false', \
                    so restore operation is skipped. Node may work incorrectly."
                );
            } else {
                log::warn!(
                    "Previous node run was terminated unexpectedly while DB's \
                    checking or restoring, but now 'restore_db' option in node \
                    config is 'false', so restore operation is skipped. Node may work incorrectly."
                );
            }
            return Ok(db);
        }

        set_restoring(&db.config.db_directory)?;
        match restore_last_applied_mc_block(&db, check_stop).await {
            Ok(Some(last_applied_mc_block)) => {
                let shard_client_mc_block = restore_shard_client_mc_block(
                    &db,
                    &last_applied_mc_block,
                    processed_wc,
                    check_stop,
                )
                .await?;
                db = match restore(db, &last_applied_mc_block, &shard_client_mc_block).await {
                    Ok(db) => db,
                    Err(err) => force_db_reset(err, check_stop, is_broken).await,
                };
            }
            Ok(None) => {
                log::info!(
                    "End of check & restore: looks like node hasn't \
                    ever booted in blockchain."
                );
            }
            Err(err) => force_db_reset(err, check_stop, is_broken).await,
        }
        reset_restoring(&db.config.db_directory)?;
    }
    set_unexpected_termination(&db.config.db_directory)?;
    Ok(db)
}

pub fn set_graceful_termination(db_dir: &str) {
    let path = Path::new(db_dir).join(UNEXPECTED_TERMINATION_BEACON_FILE);
    if let Err(e) = remove_file(&path) {
        log::error!(
            "set_graceful_termination: can't remove file {}, please do it manually, \
                otherwice check and restore DB operation will run next start (error: {})",
            UNEXPECTED_TERMINATION_BEACON_FILE,
            e
        );
    }
}

fn check_unexpected_termination(db_dir: &str) -> bool {
    Path::new(db_dir).join(UNEXPECTED_TERMINATION_BEACON_FILE).as_path().exists()
}

fn set_unexpected_termination(db_dir: &str) -> Result<()> {
    let path = Path::new(db_dir).join(UNEXPECTED_TERMINATION_BEACON_FILE);
    write(&path, "")?;
    Ok(())
}

fn reset_restoring(db_dir: &str) -> Result<()> {
    let path = Path::new(db_dir).join(RESTORING_BEACON_FILE);
    remove_file(&path)?;
    let path = Path::new(db_dir).join(BROKEN_CELL_BEACON_FILE);
    if path.exists() {
        remove_file(&path)?;
    }
    Ok(())
}

fn check_restoring(db_dir: &str) -> bool {
    Path::new(db_dir).join(RESTORING_BEACON_FILE).as_path().exists()
}

fn set_restoring(db_dir: &str) -> Result<()> {
    let path = Path::new(db_dir).join(RESTORING_BEACON_FILE);
    write(&path, "")?;
    Ok(())
}

async fn restore_last_applied_mc_block(
    db: &InternalDb,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
) -> Result<Option<BlockStuff>> {
    log::trace!("restore_last_applied_mc_block");
    match db.load_full_node_state(LAST_APPLIED_MC_BLOCK) {
        Ok(None) => return Ok(None),
        Ok(Some(id)) => match check_one_block(db, &id, false, true).await {
            Ok(block) => {
                log::info!("restore_last_applied_mc_block: {} looks good", id);
                return Ok(Some(block));
            }
            Err(e) => log::warn!("LAST_APPLIED_MC_BLOCK {} is broken: {}", id, e),
        },
        Err(e) => {
            log::warn!("Can't load LAST_APPLIED_MC_BLOCK: {}, ", e);
        }
    }

    let block = search_and_restore_last_applied_mc_block(db, check_stop)
        .await
        .map_err(|e| error!("search_and_restore_last_applied_mc_block: {}", e))?;
    Ok(Some(block))
}

async fn search_and_restore_last_applied_mc_block(
    db: &InternalDb,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
) -> Result<BlockStuff> {
    log::trace!("search_and_restore_last_applied_mc_block");

    let mut last_mc_blocks = search_last_mc_blocks(db, check_stop)?;
    while let Some(id) = last_mc_blocks.pop() {
        check_stop()?;
        log::trace!("search_and_restore_last_applied_mc_block: trying {}...", id);
        match check_one_block(db, &id, false, true).await {
            Ok(block) => return Ok(block),
            Err(e) => log::warn!("{} is broken: {}", id, e),
        }
    }
    fail!("All found last mc blocks were broken")
}

async fn restore_shard_client_mc_block(
    db: &InternalDb,
    last_applied_mc_block: &BlockStuff,
    processed_wc: i32,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
) -> Result<BlockStuff> {
    let mut block = match db.load_full_node_state(SHARD_CLIENT_MC_BLOCK) {
        Ok(None) => {
            log::warn!("SHARD_CLIENT_MC_BLOCK is None, use last applied mc block instead");
            last_applied_mc_block.clone()
        }
        Ok(Some(id)) => {
            log::trace!("SHARD_CLIENT_MC_BLOCK: {}", id);
            if *id == *last_applied_mc_block.id() {
                last_applied_mc_block.clone()
            } else {
                match check_one_block(db, &id, false, true).await {
                    Ok(block) => {
                        log::trace!("SHARD_CLIENT_MC_BLOCK {} looks good", id);
                        block
                    }
                    Err(e) => {
                        log::warn!(
                            "SHARD_CLIENT_MC_BLOCK {} is broken: {}; \
                                use last applied mc block instead",
                            id,
                            e
                        );
                        last_applied_mc_block.clone()
                    }
                }
            }
        }
        Err(e) => {
            log::warn!(
                "Can't load SHARD_CLIENT_MC_BLOCK: {}; \
                use last applied mc block instead",
                e
            );
            last_applied_mc_block.clone()
        }
    };

    let mut checked_blocks = 0_u32;
    loop {
        check_stop()?;
        match check_shard_client_mc_block(db, &block, processed_wc, false, check_stop).await {
            Ok(_) => {
                log::info!("restore_shard_client_mc_block: {} has all shard blocks", block.id());
                return Ok(block);
            }
            Err(e) => {
                log::warn!("{} doesn't have all shard blocks: {}, use prev", block.id(), e);
                let prev_id = block.construct_prev_id()?.0;
                block = check_one_block(db, &prev_id, true, true).await.map_err(|e| {
                    error!("restore_shard_client_mc_block: can't check prev block: {}", e)
                })?;
            }
        }
        checked_blocks += 1;
        if checked_blocks > SHARD_CLIENT_MC_BLOCK_CANDIDATES {
            fail!("Can't find good mc block for shard client");
        }
    }
}

async fn restore(
    db: InternalDb,
    last_applied_mc_block: &BlockStuff,
    shard_client_mc_block: &BlockStuff,
) -> Result<InternalDb> {
    let last_mc_block = if last_applied_mc_block.id().seq_no() > shard_client_mc_block.id().seq_no()
    {
        shard_client_mc_block
    } else {
        last_applied_mc_block
    };

    log::info!("restore, use as last block: {}", last_mc_block.id());

    // truncate DB
    log::debug!("try to truncate database after {}", last_mc_block.id());
    match db.load_block_next1(last_mc_block.id()) {
        Err(_) => {
            log::info!(
                "Can't load next mc block (prev {}), \
                db truncation will be skipped",
                last_mc_block.id()
            );
        }
        Ok(next_id) => match db.truncate_database(&next_id).await {
            Err(e) => log::warn!("Error while db truncation at block {}: {}", next_id, e),
            Ok(_) => log::info!("Database was truncated at {}", next_id),
        },
    }
    db.save_full_node_state(SHARD_CLIENT_MC_BLOCK, last_mc_block.id())?;
    db.save_full_node_state(LAST_APPLIED_MC_BLOCK, last_mc_block.id())?;

    if let Some(block_id) = db.load_validator_state(LAST_ROTATION_MC_BLOCK)? {
        if block_id.seq_no > last_mc_block.id().seq_no {
            db.save_validator_state(LAST_ROTATION_MC_BLOCK, last_mc_block.id())?;
        }
    }
    if let Some(block_id) = db.load_full_node_state(PSS_KEEPER_MC_BLOCK)? {
        if block_id.seq_no > last_mc_block.id().seq_no {
            db.save_full_node_state(PSS_KEEPER_MC_BLOCK, last_mc_block.id())?;
        }
    }
    if let Some(block_id) = db.load_full_node_state(ARCHIVES_GC_BLOCK)? {
        if block_id.seq_no > last_mc_block.id().seq_no {
            db.save_full_node_state(ARCHIVES_GC_BLOCK, last_mc_block.id())?;
        }
    }

    log::info!("Restore successfully finished");

    Ok(db)
}

async fn check_shard_client_mc_block(
    db: &InternalDb,
    block: &BlockStuff,
    processed_wc: i32,
    check_to_prev_mc_block: bool,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
) -> Result<Vec<BlockIdExt>> {
    log::trace!("check_shard_client_mc_block, mc block {}", block.id());

    let shard_block_from_prev_mc = if !check_to_prev_mc_block || block.id().seq_no() == 1 {
        vec![]
    } else {
        let id = block.construct_prev_id()?.0;
        let handle = db
            .load_block_handle(&id)?
            .ok_or_else(|| error!("there is no handle for block {}", id))?;
        let block = db.load_block_data(&handle).await?;
        block.shard_hashes()?.top_blocks(&[processed_wc])?
    };

    let top_shard_blocks_ids = block.shard_hashes()?.top_blocks(&[processed_wc])?;
    let mut shard_blocks_ids = Vec::with_capacity(top_shard_blocks_ids.len());
    for mut id in top_shard_blocks_ids {
        loop {
            check_stop()?;
            log::trace!("check_shard_client_mc_block: checking {}", id);
            if id.seq_no() == 0 {
                // zerostate doesn't have correspond block, but we return its id to check
                // the state later
                shard_blocks_ids.push(id);
                break;
            } else {
                let block = check_one_block(db, &id, false, check_to_prev_mc_block)
                    .await
                    .map_err(|e| error!("Shard block has a problem: {}", e))?;
                log::debug!("check_shard_client_mc_block: {} looks good", id);
                // splites or merges is possible only at commited (into masterchain) blocks,
                // so it is ok to use only one prev block here
                let prev_id = block.construct_prev_id()?.0;
                shard_blocks_ids.push(id.clone());

                if shard_block_from_prev_mc.is_empty() {
                    break;
                } else {
                    if shard_block_from_prev_mc.contains(&prev_id)
                        || shard_block_from_prev_mc.contains(&id)
                    {
                        break;
                    }
                    id = prev_id;
                }
            }
        }
    }
    Ok(shard_blocks_ids)
}

fn search_last_mc_blocks(
    db: &InternalDb,
    check_stop: &(dyn Fn() -> Result<()> + Sync),
) -> Result<Vec<BlockIdExt>> {
    let mut last_mc_block = BlockIdExt::default();
    log::trace!("search_last_mc_blocks: search last id");
    db.next1_block_db.for_each(&mut |_key, val| {
        check_stop()?;
        let id = BlockIdExt::deserialize(&val)?;
        if id.shard().workchain_id() == MASTERCHAIN_ID && id.seq_no() > last_mc_block.seq_no() {
            last_mc_block = id;
        }
        Ok(true)
    })?;
    if last_mc_block == BlockIdExt::default() {
        fail!("Can't find last mc block in next1_block_db");
    }

    log::trace!(
        "search_last_mc_blocks: last id is {}; \
        search last {} ids",
        last_mc_block,
        LAST_MC_BLOCKS
    );
    let mut last_mc_blocks = Vec::with_capacity(LAST_MC_BLOCKS as usize);
    db.next1_block_db.for_each(&mut |_key, val| {
        check_stop()?;
        let id = BlockIdExt::deserialize(&val)?;
        if id.shard().workchain_id() == MASTERCHAIN_ID
            && id.seq_no() + LAST_MC_BLOCKS >= last_mc_block.seq_no()
        {
            last_mc_blocks.push(id);
        }
        Ok(true)
    })?;
    last_mc_blocks.sort_unstable_by_key(|id| id.seq_no());
    log::trace!("search_last_mc_blocks: found {} id", last_mc_blocks.len());
    Ok(last_mc_blocks)
}

async fn check_one_block(
    db: &InternalDb,
    id: &BlockIdExt,
    should_has_next: bool,
    should_has_prev: bool,
) -> Result<BlockStuff> {
    log::trace!("check_one_block {id}");

    let handle =
        db.load_block_handle(id)?.ok_or_else(|| error!("there is no handle for block {id}"))?;
    let block_data = db.load_block_data_raw(&handle).await?;
    let block = BlockStuff::deserialize_block_checked(id.clone(), Arc::new(block_data))?;
    let _proof = db.load_block_proof(&handle, !id.is_masterchain()).await?;

    if !handle.has_data() {
        log::warn!("Block {id} has handle.has_data() false");
        handle.set_data();
        db.store_block_handle(&handle, None)?;
    }
    if id.shard().is_masterchain() {
        if !handle.has_proof() {
            log::warn!("Block {id} has handle.has_proof() false");
            handle.set_proof();
            db.store_block_handle(&handle, None)?;
        }
    } else if !handle.has_proof_link() {
        log::warn!("Block {id} has handle.has_proof_or_link() false");
        handle.set_proof_link();
        db.store_block_handle(&handle, None)?;
    }

    if !handle.is_applied() {
        fail!("Block {id} was not applied");
    }
    if !handle.is_archived() {
        if handle.masterchain_ref_seq_no() == 0 {
            fail!("Applied block {id} is not archived and doesn't have masterchain_ref_seq_no");
        } else {
            db.archive_block(id, None).await?;
        }
    }

    // prev 1
    if should_has_prev {
        let (prev1, prev2) = block.construct_prev_id()?;
        let mut restore = true;
        match db.load_block_prev1(id) {
            Ok(prev1_db) => {
                if prev1_db != prev1 {
                    log::warn!("Block {id} has real prev {prev1}, but in db {prev1_db}, restore")
                } else {
                    restore = false
                }
            }
            Err(e) => log::warn!("Block {id} prev1 read error: {e}, restore"),
        };
        if restore {
            db.prev1_block_db.put(id, &prev1.serialize())?;
            if handle.set_prev1() {
                db.store_block_handle(&handle, None)?;
            }
        }
        if !handle.has_prev1() {
            log::warn!("Applied block {id} has handle.has_prev1() false");
            handle.set_prev1();
            db.store_block_handle(&handle, None)?;
        }

        // prev 2 (after merge)
        if let Some(prev2) = prev2 {
            let mut restore = true;
            match db.load_block_prev2(id) {
                Ok(Some(prev2_db)) => {
                    if prev2_db != prev2 {
                        log::warn!(
                            "Block {id} has real prev {prev2}, but in db {prev2_db}, restore"
                        )
                    } else {
                        restore = false
                    }
                }
                Ok(None) => log::warn!("Block {id} has no prev2 in db, restore"),
                Err(e) => log::warn!("Block {id} prev2 read error: {e}, restore"),
            };
            if restore {
                db.prev2_block_db.put(id, &prev2.serialize())?;
                if handle.set_prev2() {
                    db.store_block_handle(&handle, None)?;
                }
            }
            if !handle.has_prev2() {
                log::warn!("Applied block {id} has handle.has_prev2() false");
                handle.set_prev2();
                db.store_block_handle(&handle, None)?;
            }
        }
    }

    if should_has_next {
        // next 1
        match db.load_block_next1(id) {
            Ok(_) => {
                if !handle.has_next1() {
                    log::warn!("Applied block {id} has handle.has_next1() false");
                    handle.set_next1();
                    db.store_block_handle(&handle, None)?;
                }
            }
            Err(e) => fail!("Applied block {id} next1 read error: {e}"),
        }

        // next 2 (before split)
        if block.block()?.read_info()?.before_split() {
            match db.load_block_next2(id) {
                Ok(_) => {
                    if !handle.has_next2() {
                        log::warn!(
                            "Applied block {id} is before split, but has handle.has_next2() false"
                        );
                        handle.set_next2();
                        db.store_block_handle(&handle, None)?;
                    }
                }
                Err(e) => {
                    fail!("Applied block {id} is before split, but has next2 read error: {e}")
                }
            }
        }
    }

    Ok(block)
}
