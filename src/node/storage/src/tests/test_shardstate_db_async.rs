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
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    shardstate_db_async::{AllowStateGcResolver, CellsDbConfig, ShardStateDb},
    tests::utils::init_test_log,
    StorageAlloc,
};
use std::{
    fs::read,
    path::Path,
    sync::{atomic::AtomicU32, Arc},
    time::Duration,
};
use ton_block::{read_single_root_boc, BlockIdExt, Cell, Result, ShardIdent, UInt256};

const DB_PATH: &str = "../../target/test";

fn ss_from_file(index: u32) -> Cell {
    let path = format!("src/tests/testdata/{}", index);
    let orig_bytes =
        read(Path::new(&path)).unwrap_or_else(|_| panic!("Error reading file {}", path));

    read_single_root_boc(orig_bytes).expect("Error deserializing shard-state")
}

struct MockedResolver;

impl AllowStateGcResolver for MockedResolver {
    fn allow_state_gc(
        &self,
        block_id: &BlockIdExt,
        _saved_at: u64,
        _gc_utime: u64,
    ) -> Result<bool> {
        Ok(block_id.seq_no() > 2_467_100)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_shardstate_db_async() -> Result<()> {
    init_test_log();

    const DB_NAME: &str = "test_shardstate_db_async";

    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let ss_db = ShardStateDb::new(
        db.clone(),
        "shardstate_db",
        "cells",
        "counters",
        DB_NAME,
        CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?;

    ss_db.clone().start_gc(Arc::new(MockedResolver), Arc::new(AtomicU32::new(1)));

    let range = 2_467_080..2_467_119;
    for i in range.clone() {
        let root_cell = ss_from_file(i);
        let block_id_ext = BlockIdExt::with_params(
            ShardIdent::with_tagged_prefix(-1, 0x8000_0000_0000_0000)?,
            i,
            root_cell.repr_hash(),
            UInt256::default(),
        );
        ss_db.put(&block_id_ext, root_cell.clone(), None).await?;
    }

    tokio::time::sleep(Duration::from_secs(5)).await;

    for i in range {
        let root_cell = ss_from_file(i);
        let block_id_ext = BlockIdExt::with_params(
            ShardIdent::with_tagged_prefix(-1, 0x8000_0000_0000_0000)?,
            i,
            root_cell.repr_hash(),
            UInt256::default(),
        );
        let res = ss_db.get(&block_id_ext);

        if block_id_ext.seq_no() > 2_467_100 {
            if res.is_ok() {
                panic!("Should be error");
            }
        } else {
            let loaded_root_cell = res.unwrap();
            assert_eq!(root_cell, loaded_root_cell);
        }
    }

    ss_db.stop().await;

    drop(ss_db);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}
