/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    archive_shardstate_db::ArchiveShardStateDb,
    cell_db::CellByHashStorageAdapter,
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    dynamic_boc_archive_db::DynamicBocArchiveDb,
    shardstate_db_async::CellsDbConfig,
    tests::utils::{count_tree_unique_cells, get_test_tree_of_cells, init_test_log},
    StorageAlloc,
};
use std::sync::Arc;
use ton_block::{
    read_single_root_boc, BigBocWriter, BlockIdExt, BocFlags, BuilderData, CellsFactory,
    IBitstring, Result, ShardIdent, UInt256, MAX_SAFE_DEPTH, SHARD_FULL,
};

const DB_PATH: &str = "../../target/test";

fn make_block_id(seq_no: u32) -> BlockIdExt {
    BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(-1, SHARD_FULL).unwrap(),
        seq_no,
        UInt256::from([seq_no as u8; 32]),
        UInt256::from([(seq_no + 100) as u8; 32]),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_boc_archive_db() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_dynamic_boc_archive_db";
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocArchiveDb::with_db(
        db.clone(),
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let root_cell = get_test_tree_of_cells();
    let initial_count = count_tree_unique_cells(root_cell.clone());

    // Save and verify
    boc_db.save_boc(root_cell.clone(), &|| Ok(()))?;
    assert_eq!(boc_db.cell_db().count(), initial_count);

    // Load and verify
    let loaded = boc_db.cell_db().load_cell(&root_cell.repr_hash())?;
    assert_eq!(count_tree_unique_cells(loaded), initial_count);

    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_archive_save_idempotent() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_archive_save_idempotent";
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocArchiveDb::with_db(
        db.clone(),
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let root_cell = get_test_tree_of_cells();
    let initial_count = count_tree_unique_cells(root_cell.clone());

    // Save twice
    boc_db.save_boc(root_cell.clone(), &|| Ok(()))?;
    boc_db.save_boc(root_cell.clone(), &|| Ok(()))?;

    // Count should not change
    assert_eq!(boc_db.cell_db().count(), initial_count);

    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_archive_shared_cells() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_archive_shared_cells";
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocArchiveDb::with_db(
        db.clone(),
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    // Create shared cells via CellsFactory
    let cells_factory = boc_db.cell_db().clone() as Arc<dyn CellsFactory>;
    let create_chain = |data_values: Vec<&str>| -> ton_block::Cell {
        let mut child = None;
        let mut cell = ton_block::Cell::default();
        for data in data_values.iter().rev() {
            let mut builder = BuilderData::new();
            let mut data = data.as_bytes().to_vec();
            data.push(0x80);
            builder.append_bitstring(&data).unwrap();
            if let Some(child) = child {
                builder.checked_append_reference(child).unwrap();
            }
            cell = cells_factory.clone().create_cell(builder).unwrap();
            child = Some(cell.clone());
        }
        cell
    };

    let r1 = create_chain(vec!["r1", "shared", "leaf"]);
    boc_db.save_boc(r1.clone(), &|| Ok(()))?;
    let count_after_r1 = boc_db.cell_db().count();

    let r2 = create_chain(vec!["r2", "shared", "leaf"]);
    boc_db.save_boc(r2.clone(), &|| Ok(()))?;
    let count_after_r2 = boc_db.cell_db().count();

    // r2 shares "shared" and "leaf" with r1, so only 1 new cell ("r2") should be added
    assert_eq!(count_after_r2, count_after_r1 + 1);

    // Both roots should be loadable
    let _ = boc_db.cell_db().load_cell(&r1.repr_hash())?;
    let _ = boc_db.cell_db().load_cell(&r2.repr_hash())?;

    drop(cells_factory);
    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_archive_shardstate_db() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_archive_shardstate_db";
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let ss_db = ArchiveShardStateDb::new(
        db.clone(),
        "shardstate_idx",
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?;

    let root_cell = get_test_tree_of_cells();
    let block_id = make_block_id(1);

    // Put
    assert!(!ss_db.contains(&block_id)?);
    ss_db.put(&block_id, root_cell.clone())?;
    assert!(ss_db.contains(&block_id)?);

    // Get
    let loaded = ss_db.get(&block_id)?;
    assert_eq!(count_tree_unique_cells(loaded), count_tree_unique_cells(root_cell));

    // Put idempotent
    ss_db.put(&block_id, ton_block::Cell::default())?; // should return existing, not overwrite
    let loaded2 = ss_db.get(&block_id)?;
    assert_eq!(loaded2.repr_hash(), ss_db.get(&block_id)?.repr_hash());

    drop(ss_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_archive_cell_by_hash_storage() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_archive_cell_by_hash_storage";
    destroy_rocks_db(DB_PATH, DB_NAME).await?;

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocArchiveDb::with_db(
        db.clone(),
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let data = std::fs::read(
        "../../block/src/tests/data/6A3BD5B96ABEA186BFEE202B70D510C29F85E126A522B08C1DCAD39F92CF5C51.boc",
    )?;
    let root_cell = read_single_root_boc(&data)?;

    // Repack without hashes (same as test_cell_by_hash_storage in test_dynamic_boc_rc_db.rs)
    fn repack(cell: ton_block::Cell) -> Result<ton_block::Cell> {
        let mut builder = BuilderData::with_raw(cell.data(), cell.bit_length())?;
        builder.set_type(cell.cell_type());
        for r in cell.clone_references()? {
            builder.checked_append_reference(repack(r)?)?;
        }
        builder.finalize(MAX_SAFE_DEPTH)
    }
    let root_cell = repack(root_cell)?;

    boc_db.save_boc(root_cell.clone(), &|| Ok(()))?;

    let writer = BigBocWriter::with_params(
        [root_cell.clone()],
        MAX_SAFE_DEPTH,
        BocFlags::all(),
        &|| false,
        Arc::new(CellByHashStorageAdapter::new(boc_db.cell_db().clone(), None, 0)?),
    )?;

    let mut boc = Vec::new();
    writer.write(&mut boc)?;

    assert_eq!(boc.len(), data.len());
    assert_eq!(boc, data);

    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}
