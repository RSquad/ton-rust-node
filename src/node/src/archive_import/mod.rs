/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod ingester;
pub mod scanner;
pub mod validator;

use crate::{
    block_proof::BlockProofStuff,
    collator_test_bundle::create_engine_allocated,
    config::TonNodeGlobalConfig,
    engine_traits::EngineAlloc,
    internal_db::{
        ARCHIVE_CELLS_CF_NAME, ARCHIVE_SHARDSTATE_CF_NAME, CURRENT_DB_VERSION, DB_VERSION,
        SHARD_CLIENT_MC_BLOCK,
    },
    shard_state::ShardStateStuff,
};
#[cfg(feature = "telemetry")]
use crate::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
use ingester::{Ingester, LastGroupState};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicU8, Arc},
};
use storage::{
    archive_shardstate_db::ArchiveShardStateDb,
    archives::{
        archive_manager::ArchiveManager,
        db_provider::EpochDbProvider,
        epoch::{ArchivalModeConfig, EpochRouter},
        ARCHIVE_PACKAGE_SIZE,
    },
    block_handle_db::{
        BlockHandleDb, BlockHandleStorage, NodeStateDb, BLOCK_HANDLE_DB_NAME,
        VALIDATOR_STATE_DB_NAME,
    },
    block_info_db::{
        BlockInfoDb, NEXT1_BLOCK_DB_NAME, NEXT2_BLOCK_DB_NAME, PREV1_BLOCK_DB_NAME,
        PREV2_BLOCK_DB_NAME,
    },
    db::rocksdb::{AccessType, RocksDb, NODE_DB_NAME},
    shardstate_db_async::CellsDbConfig,
    traits::Serializable,
    types::BlockMeta,
};
use ton_block::{
    error, AccountIdPrefixFull, Block, BlockIdExt, Deserializable, Result, ShardIdent, UInt256,
    WorkchainDescr, MASTERCHAIN_ID, SHARD_FULL,
};
use validator::ValidatorState;

const TARGET: &str = "archive_import";

pub struct ImportConfig {
    pub archives_path: PathBuf,
    pub epochs_path: PathBuf,
    pub epoch_size: u32,
    pub node_db_path: PathBuf,
    pub mc_zerostate_path: PathBuf,
    pub wc_zerostate_paths: Vec<PathBuf>,
    pub global_config_path: PathBuf,
    pub skip_validation: bool,
    pub move_files: bool,
}

fn read_wc_zerostates_from_config(mc_zerostate: &ShardStateStuff) -> Result<Vec<BlockIdExt>> {
    // shard_hashes is empty at genesis; workchain zerostates are in ConfigParams::workchains()
    let mut shards = Vec::new();
    mc_zerostate.config_params()?.workchains()?.iterate_with_keys(
        |wc_id: i32, descr: WorkchainDescr| {
            let shard = ShardIdent::with_tagged_prefix(wc_id, SHARD_FULL)?;
            shards.push(BlockIdExt::with_params(
                shard,
                0,
                descr.zerostate_root_hash,
                descr.zerostate_file_hash,
            ));
            Ok(true)
        },
    )?;
    Ok(shards)
}

async fn build_initial_group_state(
    zerostate: &ShardStateStuff,
    archive_manager: &ArchiveManager,
    last_imported: u32,
) -> Result<LastGroupState> {
    if last_imported == 0 {
        let shard_tops = read_wc_zerostates_from_config(zerostate)?;
        log::info!(
            target: TARGET,
            "Initial state from zerostate {}, {} workchain shard tops",
            zerostate.block_id(),
            shard_tops.len(),
        );
        return Ok(LastGroupState { mc_block_id: zerostate.block_id().clone(), shard_tops });
    }

    let mc_prefix = AccountIdPrefixFull { workchain_id: MASTERCHAIN_ID, prefix: 0 };
    let (block_id, block_data) = archive_manager
        .lookup_block_by_seqno(&mc_prefix, last_imported)
        .await?
        .ok_or_else(|| error!("Cannot find MC block at seqno {}", last_imported))?;
    let block = Block::construct_from_bytes(&block_data)?;
    let extra = block
        .read_extra()?
        .read_custom()?
        .ok_or_else(|| error!("No McExtra in MC block {}", block_id))?;
    let shard_tops =
        crate::shard_state::ShardHashesStuff::from(extra.shards().clone()).top_blocks_all()?;
    log::info!(
        target: TARGET,
        "Resuming from MC block {} (seqno {}), {} shard tops",
        block_id,
        last_imported,
        shard_tops.len(),
    );
    Ok(LastGroupState { mc_block_id: block_id, shard_tops })
}

fn process_zerostates(
    config: &ImportConfig,
    global_config: &TonNodeGlobalConfig,
    archive_state_db: &ArchiveShardStateDb,
    block_handle_storage: &BlockHandleStorage,
    #[cfg(feature = "telemetry")] engine_telemetry: Arc<EngineTelemetry>,
    engine_allocated: Arc<EngineAlloc>,
) -> Result<Arc<ShardStateStuff>> {
    log::info!(target: TARGET, "Loading MC zerostate from {}", config.mc_zerostate_path.display());
    let zerostate_bytes = std::fs::read(&config.mc_zerostate_path).map_err(|e| {
        error!("Cannot read MC zerostate file {}: {}", config.mc_zerostate_path.display(), e)
    })?;
    let expected_mc_zerostate_id = global_config.zero_state()?;
    let mc_zerostate = ShardStateStuff::deserialize_zerostate(
        expected_mc_zerostate_id.clone(),
        &zerostate_bytes,
        #[cfg(feature = "telemetry")]
        &engine_telemetry,
        &engine_allocated,
    )?;
    log::info!(target: TARGET, "MC zerostate loaded successfully");

    // Load and validate workchain zerostates
    let mut expected_wc_zerostates: HashMap<UInt256, BlockIdExt> = HashMap::from_iter(
        read_wc_zerostates_from_config(&mc_zerostate)?
            .into_iter()
            .map(|id| (id.file_hash.clone(), id)),
    );

    let mut wc_zerostates = Vec::new();
    for path in &config.wc_zerostate_paths {
        log::info!(target: TARGET, "Loading workchain zerostate from {}", path.display());
        let zerostate_bytes = std::fs::read(path)
            .map_err(|e| error!("Cannot read WC zerostate file {}: {}", path.display(), e))?;
        let id = expected_wc_zerostates.remove(&UInt256::calc_file_hash(&zerostate_bytes)).ok_or_else(|| {
            error!(
                "Workchain zerostate file {} does not match any expected file hash from MC zerostate",
                path.display(),
            )
        })?;
        let state = ShardStateStuff::deserialize_zerostate(
            id.clone(),
            &zerostate_bytes,
            #[cfg(feature = "telemetry")]
            &engine_telemetry,
            &engine_allocated,
        )?;
        wc_zerostates.push((id, state.root_cell().clone()));
    }

    if !expected_wc_zerostates.is_empty() {
        let missing: Vec<_> = expected_wc_zerostates.into_iter().collect();
        return Err(error!("Missing workchain zerostates: {:?}", missing,));
    }

    let save_handle = |id: &BlockIdExt| -> Result<()> {
        let handle = if let Some(handle) =
            block_handle_storage.create_handle(id.clone(), BlockMeta::default(), None)?
        {
            handle
        } else {
            block_handle_storage
                .load_handle_by_id(&id)?
                .ok_or_else(|| error!("Failed to create or load block handle for MC zerostate"))?
        };
        if handle.set_state() | handle.set_state_saved() | handle.set_block_applied() {
            block_handle_storage.save_handle(&handle, None)?;
        }
        Ok(())
    };

    archive_state_db.put(&expected_mc_zerostate_id, mc_zerostate.root_cell().clone())?;
    save_handle(&expected_mc_zerostate_id)?;
    log::info!(target: TARGET, "MC zerostate saved to archive state DB");

    for (wc_id, wc_root) in wc_zerostates {
        archive_state_db.put(&wc_id, wc_root)?;
        save_handle(&wc_id)?;
        log::info!(target: TARGET, "Workchain zerostate {} saved to archive state DB", wc_id);
    }

    Ok(mc_zerostate)
}

/// Returns the node_db Arc so the caller can wait for all background tasks to release it.
pub async fn run_import(config: ImportConfig) -> Result<Arc<RocksDb>> {
    log::info!(
        target: TARGET,
        "Loading global config from {}",
        config.global_config_path.display()
    );
    let global_config = TonNodeGlobalConfig::from_json_file(&config.global_config_path)
        .map_err(|e| error!("Cannot load global config: {}", e))?;
    let expected_zerostate_id = global_config.zero_state()?;
    let mut hardforks = global_config.hardforks()?;
    hardforks.sort_by_key(|hf| hf.seq_no());
    log::info!(
        target: TARGET,
        "Global config: zerostate={}, {} hard fork(s)",
        expected_zerostate_id,
        hardforks.len(),
    );

    #[cfg(feature = "telemetry")]
    let engine_telemetry = create_engine_telemetry();
    let engine_allocated = create_engine_allocated();

    let epoch_config = ArchivalModeConfig {
        epoch_size: config.epoch_size,
        new_epochs_path: config.epochs_path.clone(),
        existing_epochs: vec![],
    };
    let router = Arc::new(EpochRouter::new(&epoch_config).await?);
    let db_provider = Arc::new(EpochDbProvider::new(router));

    std::fs::create_dir_all(&config.node_db_path).map_err(|e| {
        error!("Cannot create node_db_path {}: {}", config.node_db_path.display(), e)
    })?;
    let node_db = RocksDb::new(&config.node_db_path, NODE_DB_NAME, None, AccessType::ReadWrite)?;

    let handle_db = Arc::new(BlockHandleDb::with_db(node_db.clone(), BLOCK_HANDLE_DB_NAME, true)?);
    let full_node_state_db = Arc::new(NodeStateDb::with_db(
        node_db.clone(),
        storage::db::rocksdb::NODE_STATE_DB_NAME,
        true,
    )?);
    full_node_state_db.put(&DB_VERSION, &CURRENT_DB_VERSION.serialize())?;
    let validator_state_db =
        Arc::new(NodeStateDb::with_db(node_db.clone(), VALIDATOR_STATE_DB_NAME, true)?);

    let prev1_block_db = BlockInfoDb::with_db(node_db.clone(), PREV1_BLOCK_DB_NAME, true)?;
    let prev2_block_db = BlockInfoDb::with_db(node_db.clone(), PREV2_BLOCK_DB_NAME, true)?;
    let next1_block_db = BlockInfoDb::with_db(node_db.clone(), NEXT1_BLOCK_DB_NAME, true)?;
    let next2_block_db = BlockInfoDb::with_db(node_db.clone(), NEXT2_BLOCK_DB_NAME, true)?;

    #[cfg(feature = "telemetry")]
    let storage_telemetry = engine_telemetry.storage.clone();
    let storage_alloc = engine_allocated.storage.clone();

    let mut block_handle_storage = BlockHandleStorage::with_dbs(
        handle_db,
        full_node_state_db,
        validator_state_db,
        #[cfg(feature = "telemetry")]
        storage_telemetry.clone(),
        storage_alloc.clone(),
    );
    block_handle_storage.set_no_cache();
    let block_handle_storage = Arc::new(block_handle_storage);

    let db_root_path = Arc::new(config.node_db_path.clone());
    let shard_split_depth = Arc::new(AtomicU8::new(0));

    let archive_manager = Arc::new(
        ArchiveManager::with_data(
            node_db.clone(),
            db_root_path,
            db_provider,
            0, // last_unneeded_key_block
            shard_split_depth,
            #[cfg(feature = "telemetry")]
            storage_telemetry,
            storage_alloc,
        )
        .await?,
    );

    let cells_db_config = CellsDbConfig::default();
    let (archive_cells_opts, archive_cells_cache) =
        storage::cell_db::CellDb::build_cf_options(cells_db_config.cells_cache_size_bytes);
    let archive_states_db = RocksDb::new(
        &config.node_db_path,
        crate::internal_db::ARCHIVE_STATES_DB_NAME,
        std::collections::HashMap::from([(ARCHIVE_CELLS_CF_NAME.to_string(), archive_cells_opts)]),
        AccessType::ReadWrite,
    )?;
    archive_states_db.register_cache(archive_cells_cache);
    let archive_state_db = Arc::new(ArchiveShardStateDb::new(
        archive_states_db,
        ARCHIVE_SHARDSTATE_CF_NAME,
        ARCHIVE_CELLS_CF_NAME,
        &cells_db_config,
        #[cfg(feature = "telemetry")]
        engine_telemetry.storage.clone(),
        engine_allocated.storage.clone(),
    )?);

    let mc_zerostate = process_zerostates(
        &config,
        &global_config,
        &archive_state_db,
        &block_handle_storage,
        #[cfg(feature = "telemetry")]
        engine_telemetry.clone(),
        engine_allocated.clone(),
    )?;

    log::info!(target: TARGET, "Scanning packages in {}", config.archives_path.display());
    let packages = scanner::scan_packages(&config.archives_path)?;
    log::info!(target: TARGET, "Found {} package files", packages.len());

    if packages.is_empty() {
        log::warn!(target: TARGET, "No packages found, nothing to import");
        return Ok(node_db);
    }

    let groups = scanner::group_by_archive_id(packages)?;
    log::info!(target: TARGET, "Grouped into {} archive groups", groups.len());

    let mut validator_state = ValidatorState::new(mc_zerostate.clone(), hardforks);
    let mut skip_count = 0;

    let last_imported = if let Some(max_mc) = archive_manager.get_max_mc_seqno().await {
        // Clamp resume to the shard client: the node may have
        // applied MC blocks ahead of the shard client, whose shard tops have no handles
        // yet — re-import those groups instead of skipping them.
        let shard_client_seqno = block_handle_storage
            .load_full_node_state(SHARD_CLIENT_MC_BLOCK)?
            .map(|id| id.seq_no())
            .unwrap_or(max_mc);
        let max_seqno = max_mc.min(shard_client_seqno);
        if max_seqno > groups.last().unwrap().archive_id + ARCHIVE_PACKAGE_SIZE as u32 {
            log::warn!(target: TARGET,
                "Existing import detected with max MC seqno {}, which is beyond the last archive group ({}), skipping all groups",
                max_seqno, groups.last().unwrap().archive_id);
            return Ok(node_db);
        }
        skip_count =
            groups.iter().take_while(|g| g.archive_id < max_seqno).count().saturating_sub(1);

        log::info!(
            target: TARGET,
            "Detected existing import (max MC seqno = {}), skipping {} groups",
            max_seqno,
            skip_count,
        );

        // Restore key block proof regardless of skip_count: files may have been moved
        // and the scanned list may start mid-chain.
        if !config.skip_validation {
            if let Some(key_seqno) = archive_manager.get_max_key_block_seqno().await {
                let mc_prefix = AccountIdPrefixFull { workchain_id: MASTERCHAIN_ID, prefix: 0 };
                let (block_id, proof_data) = archive_manager
                    .lookup_proof_by_seqno(&mc_prefix, key_seqno)
                    .await?
                    .ok_or_else(|| {
                        error!(
                            "Key block seqno {} found in index but proof not readable",
                            key_seqno,
                        )
                    })?;
                let proof = BlockProofStuff::deserialize(&block_id, proof_data, false)?;
                log::info!(
                    target: TARGET,
                    "Restored key block proof: {}",
                    block_id,
                );
                validator_state.set_key_block_proof(proof);
            }
        }
        groups[skip_count].archive_id.saturating_sub(1)
    } else {
        0
    };

    let initial_group_state =
        build_initial_group_state(&mc_zerostate, &archive_manager, last_imported).await?;

    let ingester = Ingester::new(
        archive_manager,
        block_handle_storage,
        archive_state_db,
        prev1_block_db,
        prev2_block_db,
        next1_block_db,
        next2_block_db,
        config.move_files,
        config.skip_validation,
    );

    let total = groups.len();
    ingester
        .run_groups(&groups[skip_count..], skip_count, total, validator_state, initial_group_state)
        .await?;

    log::info!(target: TARGET, "Import complete! Processed {} archive groups", total);
    Ok(node_db)
}

#[cfg(not(target_os = "windows"))]
#[cfg(test)]
#[path = "../tests/test_archive_import.rs"]
mod tests;
