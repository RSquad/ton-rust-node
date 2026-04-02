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
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    cell_db::CellByHashStorageAdapter,
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    dynamic_boc_rc_db::DynamicBocDb,
    shardstate_db_async::CellsDbConfig,
    tests::utils::{
        count_tree_unique_cells, get_another_test_tree_of_cells, get_test_tree_of_cells,
        init_test_log,
    },
    StorageAlloc,
};
use std::sync::Arc;
use ton_block::{
    read_single_root_boc, BigBocWriter, BocFlags, BuilderData, Cell, IBitstring, Result,
    MAX_SAFE_DEPTH,
};

const DB_PATH: &str = "../../target/test";

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_boc_rc_db() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_dynamic_boc_rc_db";

    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocDb::with_db(
        db.clone(),
        "cells",
        "counters",
        "",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let root_cell = get_test_tree_of_cells();

    assert!(boc_db.count() == 0);
    let initial_cell_count = count_tree_unique_cells(root_cell.clone());
    boc_db.save_boc(root_cell.clone(), &|| Ok(()))?;
    assert_eq!(boc_db.count(), initial_cell_count);

    let loaded_boc = boc_db.load_cell(&root_cell.repr_hash(), true)?;
    let fetched_count = count_tree_unique_cells(loaded_boc.clone());
    assert_eq!(fetched_count, initial_cell_count);

    let root_cell_2 = get_another_test_tree_of_cells();
    boc_db.save_boc(root_cell_2.clone(), &|| Ok(()))?;

    boc_db.delete_boc(&root_cell.repr_hash(), &|| Ok(()))?;
    assert!(boc_db.count() > 0);

    boc_db.delete_boc(&root_cell_2.repr_hash(), &|| Ok(()))?;
    assert_eq!(boc_db.count(), 0);

    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_boc_rc_db_2() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_dynamic_boc_rc_db_2";

    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocDb::with_db(
        db.clone(),
        "cells",
        "counters",
        "",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let cells_factory = boc_db.cells_factory();
    let create_ss = |cells_chain: Vec<&str>| -> Cell {
        let mut child = None;
        let mut cell = Cell::default();
        for data in cells_chain.iter().rev() {
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
    let check_stop = || Ok(());

    let r1 = create_ss(vec!["r1", "c1", "A", "B"]);
    let r1_id = r1.repr_hash();
    boc_db.save_boc(r1, &check_stop).unwrap();

    let r2 = create_ss(vec!["r2", "c2", "A", "B"]);
    let r2_id = r2.repr_hash();
    boc_db.save_boc(r2, &check_stop).unwrap();

    boc_db.delete_boc(&r1_id, &check_stop).unwrap();
    boc_db.delete_boc(&r2_id, &check_stop).unwrap();

    let r3 = create_ss(vec!["r3", "c3", "B"]);
    let r3_id = r3.repr_hash();
    boc_db.save_boc(r3, &check_stop).unwrap();

    boc_db.delete_boc(&r3_id, &check_stop).unwrap();

    let r4 = create_ss(vec!["r4", "c4", "A", "B"]);
    boc_db.save_boc(r4, &check_stop).unwrap();

    drop(cells_factory);
    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

fn repack_without_hashes(cell: Cell) -> Result<Cell> {
    let mut builder = BuilderData::with_raw(cell.data(), cell.bit_length())?;
    builder.set_type(cell.cell_type());
    for r in cell.clone_references() {
        let repacked_ref = repack_without_hashes(r.clone())?;
        builder.checked_append_reference(repacked_ref)?;
    }
    builder.finalize(MAX_SAFE_DEPTH)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cell_by_hash_storage() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_cell_by_hash_storage";

    destroy_rocks_db(DB_PATH, DB_NAME).await?;

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let boc_db = Arc::new(DynamicBocDb::with_db(
        db.clone(),
        "cells",
        "counters",
        "",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?);

    let data = std::fs::read("../../block/src/tests/data/6A3BD5B96ABEA186BFEE202B70D510C29F85E126A522B08C1DCAD39F92CF5C51.boc")?;
    let root_cell = read_single_root_boc(&data)?;
    let root_cell = repack_without_hashes(root_cell)?;
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

    let root_cell_2 = read_single_root_boc(&boc)?;

    assert_eq!(root_cell, root_cell_2);

    drop(boc_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}
