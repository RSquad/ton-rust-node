/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{collections::HashMap, fs::OpenOptions, path::Path, sync::Arc};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{
    db::rocksdb::RocksDb,
    dynamic_boc_rc_db::DynamicBocDb,
    shardstate_db_async::{CellsDbConfig, ShardStateDb},
    StorageAlloc,
};
use ton_block::{BlockIdExt, BocWriterStack, Result, MAX_SAFE_DEPTH};

// include!("../../../common/src/log.rs");

const DB_PATH: &str = "../target/test";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // init_log("../../common/config/log_cfg_debug.yml");

    const DB_NAME: &str = "bench_shardstate_db";

    let open_db = || -> Result<(Arc<RocksDb>, Arc<ShardStateDb>)> {
        let mut cfs_opts = HashMap::new();
        cfs_opts.insert(
            "cells".to_string(),
            DynamicBocDb::build_cells_cf_options(&CellsDbConfig::default()),
        );
        cfs_opts.insert(
            "counters".to_string(),
            DynamicBocDb::build_counters_cf_options(&CellsDbConfig::default()),
        );
        let db = RocksDb::new(DB_PATH, DB_NAME, cfs_opts, None)?;
        let ss_db = ShardStateDb::new(
            db.clone(),
            "shardstate_db",
            "cells",
            "counters",
            DB_PATH,
            CellsDbConfig::default(),
            #[cfg(feature = "telemetry")]
            Arc::new(StorageTelemetry::default()),
            Arc::new(StorageAlloc::default()),
        )?;
        Ok((db, ss_db))
    };

    let (_db, ss_db) = open_db()?;

    let mut mc_state_id = BlockIdExt::default();
    ss_db.enumerate_ids(&mut |id| {
        if id.shard().is_masterchain() {
            mc_state_id = id.clone();
            Ok(false)
        } else {
            Ok(true)
        }
    })?;

    println!("Masterchain state id: {}", mc_state_id);

    let root_cell = ss_db.get(&mc_state_id)?;
    let now = std::time::Instant::now();
    let mut dest = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/pss.boc", DB_PATH))?;
    let temp_dir = Path::new(DB_PATH);
    let cells_storage = ss_db.create_hashed_cell_storage(None, 0)?;

    BocWriterStack::write(&mut dest, &temp_dir, root_cell, MAX_SAFE_DEPTH, cells_storage, &|| {
        false
    })?;

    log::info!("TIME {:#?}", now.elapsed());
    Ok(())
}
