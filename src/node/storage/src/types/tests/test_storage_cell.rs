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
    cell_db::CellDb,
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    shardstate_db_async::CellsDbConfig,
    StorageAlloc,
};
use ton_block::{create_cell, BuilderData, IBitstring, MAX_LEVEL};

const DB_PATH: &str = "../../target/test";

async fn init_cell_db(db_name: &str) -> Result<Arc<CellDb>> {
    destroy_rocks_db(DB_PATH, db_name).await?;
    let db = RocksDb::new(DB_PATH, db_name, None, AccessType::ReadWrite)?;
    Ok(Arc::new(CellDb::with_db(
        db.clone(),
        "cells",
        &CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?))
}

/// Verify that serialize -> deserialize roundtrip preserves cell data
fn assert_roundtrip(cell_db: &Arc<CellDb>, original: &Cell) -> Result<()> {
    let serialized = serialize_stored_cell(original)?;
    let loader = cell_db.stored_loader();
    let deserialized = deserialize_stored_cell(original.repr_hash(), &serialized, loader)?;

    // Compare hashes, depths, data, references
    assert_eq!(original.repr_hash(), deserialized.repr_hash());
    assert_eq!(original.repr_depth(), deserialized.repr_depth());
    assert_eq!(original.cell_type(), deserialized.cell_type());
    assert_eq!(original.level(), deserialized.level());
    assert_eq!(original.bit_length(), deserialized.bit_length());
    assert_eq!(original.data(), deserialized.data());
    assert_eq!(original.references_count(), deserialized.references_count());

    for i in 0..=MAX_LEVEL {
        if original.level_mask().is_significant_index(i) {
            assert_eq!(original.hash(i), deserialized.hash(i));
            assert_eq!(original.depth(i), deserialized.depth(i));
        }
    }

    for i in 0..original.references_count() {
        assert_eq!(original.reference_repr_hash(i)?, deserialized.reference_repr_hash(i)?);
        assert_eq!(original.reference_repr_depth(i)?, deserialized.reference_repr_depth(i)?);
    }

    Ok(())
}

#[tokio::test]
async fn test_storage_cell_serde() -> Result<()> {
    let cell_db = init_cell_db("test_storage_cell_serde").await?;

    let c1 = create_cell(&[], &[1, 2, 45, 76, 200])?;
    let c2 = create_cell(&[], &[10, 200, 45, 7, 20])?;
    let c3 = create_cell(&[c1.clone(), c2.clone()], &[1, 2, 45, 76, 200])?;

    let mut b = BuilderData::new();
    b.set_type(CellType::PrunedBranch);
    b.append_u8(u8::from(CellType::PrunedBranch))?;
    b.append_u8(1)?;
    b.append_raw(UInt256::rand().as_slice(), 256)?;
    b.append_u16(47)?;
    let c4 = b.into_cell()?;

    assert_roundtrip(&cell_db, &c1)?;
    assert_roundtrip(&cell_db, &c2)?;
    assert_roundtrip(&cell_db, &c3)?;
    assert_roundtrip(&cell_db, &c4)?;

    Ok(())
}
