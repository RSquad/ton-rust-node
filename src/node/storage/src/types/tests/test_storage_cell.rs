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
use super::*;
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    shardstate_db_async::CellsDbConfig,
    StorageAlloc,
};
use std::ops::Deref;
use ton_block::{create_cell, BuilderData, IBitstring};

const DB_PATH: &str = "../../target/test";

async fn init_cell_db(db_name: &str) -> Result<Arc<CellDb>> {
    destroy_rocks_db(DB_PATH, db_name).await?;
    let db = RocksDb::new(DB_PATH, db_name, None, AccessType::ReadWrite)?;
    Ok(Arc::new(CellDb::with_db(
        db.clone(),
        "cells",
        DB_PATH,
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?))
}

#[tokio::test]
async fn test_storage_cell_serde() -> Result<()> {
    let cell_db = init_cell_db("test_storage_cell_serde").await?;

    let c1 = create_cell(vec![], &[1, 2, 45, 76, 200])?;
    let c2 = create_cell(vec![], &[10, 200, 45, 7, 20])?;
    let c3 = create_cell(vec![c1.clone(), c2.clone()], &[1, 2, 45, 76, 200])?;

    let mut b = BuilderData::new();
    b.set_type(CellType::PrunedBranch);
    b.append_u8(u8::from(CellType::PrunedBranch))?;
    b.append_u8(1)?;
    b.append_raw(UInt256::rand().as_slice(), 256)?;
    b.append_u16(47)?;
    let c4 = b.into_cell()?;

    let s1 = StoringCell::with_cell(c1.cell_impl().deref(), &cell_db)?;
    let s2 = StoringCell::with_cell(c2.cell_impl().deref(), &cell_db)?;
    let s3 = StoringCell::with_cell(c3.cell_impl().deref(), &cell_db)?;
    let s4 = StoringCell::with_cell(c4.cell_impl().deref(), &cell_db)?;

    let d1 = StoredCell::serialize(&s1)?;
    let d2 = StoredCell::serialize(&s2)?;
    let d3 = StoredCell::serialize(&s3)?;
    let d4 = StoredCell::serialize(&s4)?;

    assert!(s1.cell_data == StoredCell::deserialize(&cell_db, &c1.repr_hash(), &d1)?.cell_data);
    assert!(s2.cell_data == StoredCell::deserialize(&cell_db, &c2.repr_hash(), &d2)?.cell_data);
    assert!(s3.cell_data == StoredCell::deserialize(&cell_db, &c3.repr_hash(), &d3)?.cell_data);
    assert!(s4.cell_data == StoredCell::deserialize(&cell_db, &c4.repr_hash(), &d4)?.cell_data);

    Ok(())
}
