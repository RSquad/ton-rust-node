/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::collator_test_bundle::create_engine_telemetry;
use crate::{
    archive_import::{run_import, ImportConfig},
    block::{BlockIdExtExtention, BlockStuff},
    collator_test_bundle::create_engine_allocated,
    internal_db::{
        InternalDb, InternalDbConfig, ARCHIVES_GC_BLOCK, LAST_APPLIED_MC_BLOCK,
        PSS_KEEPER_MC_BLOCK, SHARD_CLIENT_MC_BLOCK,
    },
    test_helper::init_test_log,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicU8, Arc},
};
use storage::{archives::epoch::ArchivalModeConfig, db::rocksdb::RocksDb};
use ton_block::{
    read_single_root_boc, write_boc, AccountIdPrefixFull, BlockIdExt, Result, SHARD_FULL,
};

async fn wait_for_db_release(db: Arc<RocksDb>) {
    while Arc::strong_count(&db) > 1 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    drop(db);
}

const ARCHIVES_PATH: &str = "src/tests/static/archives";
const MC_ZEROSTATE_PATH: &str =
    "src/tests/static/5E994FCF4D425C0A6CE6A792594B7173205F740A39CD56F537DEFD28B48A0F6E.boc";
const WC_ZEROSTATE_PATH: &str =
    "src/tests/static/EE0BEDFE4B32761FB35E9E1D8818EA720CAD1A0E7B4D2ED673C488E72E910342.boc";
const GLOBAL_CONFIG_PATH: &str = "src/tests/config/mainnet.json";

/// Copy archive files to a temporary directory, restoring colons in filenames
/// (files are stored with underscores to avoid issues on Windows).
fn prepare_archives(dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(ARCHIVES_PATH)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let restored_name = name.replace("_", ":");
        std::fs::copy(entry.path(), dest.join(restored_name))?;
    }
    Ok(())
}

fn import_config(dir: &Path, archives_path: PathBuf) -> ImportConfig {
    ImportConfig {
        archives_path,
        epochs_path: dir.join("epochs"),
        epoch_size: 20_000,
        node_db_path: dir.join("node_db"),
        mc_zerostate_path: PathBuf::from(MC_ZEROSTATE_PATH),
        wc_zerostate_paths: vec![PathBuf::from(WC_ZEROSTATE_PATH)],
        global_config_path: PathBuf::from(GLOBAL_CONFIG_PATH),
        skip_validation: false,
        move_files: false,
    }
}

async fn open_db(dir: &Path) -> Result<InternalDb> {
    let db_dir = dir.join("node_db");
    let epochs_path = dir.join("epochs");
    InternalDb::with_update(
        InternalDbConfig {
            db_directory: db_dir.to_string_lossy().to_string(),
            archival_mode: Some(ArchivalModeConfig {
                epoch_size: 20_000,
                new_epochs_path: epochs_path,
                existing_epochs: vec![],
            }),
            ..Default::default()
        },
        false,
        false,
        false,
        None,
        &|| Ok(()),
        None,
        Arc::new(AtomicU8::new(0)),
        None,
        #[cfg(feature = "telemetry")]
        create_engine_telemetry(),
        create_engine_allocated(),
    )
    .await
}

async fn check_imported_block(
    db: &InternalDb,
    block_id: &BlockIdExt,
) -> Result<Option<BlockStuff>> {
    let handle =
        db.load_block_handle(block_id)?.expect("Block handle must exist for imported block");
    assert!(handle.has_state(), "Imported block must have state");
    assert!(handle.has_saved_state(), "Imported block must have saved state");
    assert!(handle.is_applied(), "Imported block must be applied");

    let mut block_stuff = None;
    if block_id.seq_no() > 0 {
        assert!(handle.has_data(), "Imported block must have data");
        assert!(handle.has_prev1(), "Imported block must have prev1");
        if block_id.is_masterchain() {
            assert!(handle.has_proof(), "Imported MC block must have proof");
        } else {
            assert!(handle.has_proof_link(), "Imported shard block must have proof link");
        }

        let prev1 = db.load_block_prev1(&block_id)?;
        assert_eq!(prev1.seq_no(), block_id.seq_no() - 1);
        let prev_handle = db.load_block_handle(&prev1)?.expect("Prev block handle must exist");
        assert!(prev_handle.has_next1(), "Imported block must have next1");
        let next1 = db.load_block_next1(prev_handle.id())?;
        assert_eq!(&next1, block_id);

        block_stuff = Some(db.load_block_data(&handle).await?);
        let _ = db.load_block_proof(&handle, !block_id.is_masterchain()).await?;
    }

    let loaded_state = db.load_shard_state_dynamic(block_id)?;
    let boc = write_boc(loaded_state.root_cell())?;
    let deserialized_state = read_single_root_boc(&boc)?;
    assert_eq!(loaded_state.root_cell().repr_hash(), deserialized_state.repr_hash());
    if block_id.seq_no() > 0 {
        assert_eq!(
            *deserialized_state.repr_hash(),
            block_stuff.as_ref().unwrap().block()?.read_state_update()?.new_hash
        );
    } else {
        assert_eq!(deserialized_state.repr_hash(), block_id.root_hash());
    }

    Ok(block_stuff)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_import_and_verify() -> Result<()> {
    init_test_log();
    let dir = tempfile::tempdir().unwrap();
    let archives = dir.path().join("archives");
    prepare_archives(&archives).unwrap();
    let config = import_config(dir.path(), archives);

    run_import(config).await?;

    let db = open_db(dir.path()).await?;

    let last_mc =
        db.load_full_node_state(LAST_APPLIED_MC_BLOCK)?.expect("LAST_APPLIED_MC_BLOCK must be set");
    assert_eq!(last_mc.seq_no(), 199);
    assert!(last_mc.shard().is_masterchain());

    let gc_block = db.load_full_node_state(ARCHIVES_GC_BLOCK)?;
    assert_eq!(last_mc, gc_block.unwrap());

    let pss_block = db.load_full_node_state(PSS_KEEPER_MC_BLOCK)?;
    assert_eq!(last_mc, pss_block.unwrap());

    let shard_client = db.load_full_node_state(SHARD_CLIENT_MC_BLOCK)?;
    assert_eq!(last_mc, shard_client.unwrap());

    let last_mc_block = check_imported_block(&db, &last_mc).await?.unwrap();

    for shard_block in last_mc_block.top_blocks_all()? {
        check_imported_block(&db, &shard_block).await?;
    }

    let first_mc =
        db.lookup_block_by_seqno(&AccountIdPrefixFull::any_masterchain(), 1).await?.unwrap();
    let first_mc_block = check_imported_block(&db, &first_mc.0).await?.unwrap();
    // MC zerostate
    check_imported_block(&db, &first_mc_block.construct_prev_id()?.0).await?;

    let first_wc =
        db.lookup_block_by_seqno(&AccountIdPrefixFull::workchain(0, SHARD_FULL), 1).await?.unwrap();
    let first_wc_block = check_imported_block(&db, &first_wc.0).await?.unwrap();
    // WC zerostate
    check_imported_block(&db, &first_wc_block.construct_prev_id()?.0).await?;

    db.stop_states_db().await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_import_resume() -> Result<()> {
    init_test_log();
    let dir = tempfile::tempdir().unwrap();
    let all_archives = dir.path().join("archives");
    prepare_archives(&all_archives).unwrap();

    let partial_archives = dir.path().join("partial");
    std::fs::create_dir_all(&partial_archives)?;

    // Copy only the first group (archive.00000.*)
    for entry in std::fs::read_dir(&all_archives)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("archive.00000.") {
            std::fs::copy(entry.path(), partial_archives.join(&name))?;
        }
    }

    // First import — only first group
    let config1 = ImportConfig {
        archives_path: partial_archives.clone(),
        epochs_path: dir.path().join("epochs"),
        epoch_size: 20_000,
        node_db_path: dir.path().join("node_db"),
        mc_zerostate_path: PathBuf::from(MC_ZEROSTATE_PATH),
        wc_zerostate_paths: vec![PathBuf::from(WC_ZEROSTATE_PATH)],
        global_config_path: PathBuf::from(GLOBAL_CONFIG_PATH),
        skip_validation: false,
        move_files: true,
    };
    let node_db = run_import(config1).await?;
    wait_for_db_release(node_db).await;

    let db1 = open_db(dir.path()).await?;
    let last_mc_1 = db1
        .load_full_node_state(LAST_APPLIED_MC_BLOCK)?
        .expect("After first import, LAST_APPLIED_MC_BLOCK must be set");
    assert_eq!(last_mc_1.seq_no(), 99);
    drop(db1);

    // Copy remaining files for second import
    for entry in std::fs::read_dir(&all_archives)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("archive.00000.") {
            std::fs::copy(entry.path(), partial_archives.join(&name))?;
        }
    }

    // Second import — should resume and process remaining groups
    let config2 = ImportConfig {
        archives_path: partial_archives,
        epochs_path: dir.path().join("epochs"),
        epoch_size: 20_000,
        node_db_path: dir.path().join("node_db"),
        mc_zerostate_path: PathBuf::from(MC_ZEROSTATE_PATH),
        wc_zerostate_paths: vec![PathBuf::from(WC_ZEROSTATE_PATH)],
        global_config_path: PathBuf::from(GLOBAL_CONFIG_PATH),
        skip_validation: false,
        move_files: false,
    };
    run_import(config2).await?;

    let db2 = open_db(dir.path()).await?;
    let last_mc_2 = db2
        .load_full_node_state(LAST_APPLIED_MC_BLOCK)?
        .expect("After second import, LAST_APPLIED_MC_BLOCK must be set");
    assert_eq!(last_mc_2.seq_no(), 199);
    db2.stop_states_db().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_import_skip_validation() -> Result<()> {
    init_test_log();
    let dir = tempfile::tempdir().unwrap();
    let archives = dir.path().join("archives");
    prepare_archives(&archives).unwrap();
    let mut config = import_config(dir.path(), archives);
    config.skip_validation = true;

    run_import(config).await?;

    let db = open_db(dir.path()).await?;
    let last_mc = db.load_full_node_state(LAST_APPLIED_MC_BLOCK)?;
    assert!(last_mc.is_some(), "Even with skip_validation, last MC must be set");

    let last_mc = last_mc.unwrap();
    let handle = db
        .load_block_handle(&last_mc)?
        .expect("Block handle must exist after skip_validation import");
    assert!(handle.has_data());

    db.stop_states_db().await;
    Ok(())
}
