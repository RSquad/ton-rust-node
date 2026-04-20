/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{collections::HashMap, io::Cursor, ops::Deref, sync::Arc};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    dynamic_boc_rc_db::DynamicBocDb,
    shardstate_db_async::{CellsDbConfig, ShardStateDb},
    StorageAlloc,
};
use tokio::io::AsyncReadExt;
use ton_block::{BocReader, Result, UInt256};

include!("../../../common/src/log.rs");

const DB_PATH: &str = "../target/test";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // init_log("../../common/config/log_cfg_debug.yml");

    const DB_NAME: &str = "bench_shardstate_db";

    destroy_rocks_db(DB_PATH, DB_NAME).await?;

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
        let db = RocksDb::new(DB_PATH, DB_NAME, cfs_opts, AccessType::ReadOnly)?;
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

    let files = [
        "/root/kirill/ton-node/target/node_db/shard_state_persistent_db/2670/8ae1a60a88e184a1252812ca1c84ea22ed0a46aed67988413f65a2ea59cc", // master
        "/root/kirill/ton-node/target/node_db/shard_state_persistent_db/3f20/02235495b9ea963514f5f6e1a5ec8a6003a8bd324b84ba863f3042bd0334",
        "/root/kirill/ton-node/target/node_db/shard_state_persistent_db/2c61/61457ab9aa936fbe7858ebc8f1d0404b3e4334a8c0893a242e6ad29de331",
        "/root/kirill/ton-node/target/node_db/shard_state_persistent_db/b739/a754580d4ba752768d355eed15c9375704ca160a2055cfedc681232a38ac",
        "/root/kirill/ton-node/target/node_db/shard_state_persistent_db/c069/9e313f78f0bea170c8c8b5c8a29b67e4960eb8ee8aa19ccab83599a07479",
    ];

    let mut max_size = 0;
    let mut max_cells = 0;
    for name in files {
        let size = std::fs::metadata(name)?.len();
        if size > max_size {
            max_size = size;
        }
        let mut f = std::fs::File::open(name)?;
        let (boc_header, _) = BocReader::new().read_header(&mut f)?;
        if boc_header.cells_count > max_cells {
            max_cells = boc_header.cells_count;
        }
    }

    println!("max_size {}, max_cells {}", max_size, max_cells);

    let mut data = Arc::new(Vec::with_capacity(max_size as usize));
    let mut cells_index = Vec::with_capacity(max_cells as usize);

    let (_db, ss_db) = open_db()?;

    for name in files {
        // println!("{} started at {}", name, chrono::Local::now().format("%Y-%m-%d %H:%M:%S")); // chrono = "0.4.40"
        let mut file = tokio::fs::File::open(name).await?;
        let d = 'a: loop {
            match Arc::get_mut(&mut data) {
                Some(d) => break 'a d,
                None => {
                    println!("INTERNAL ERROR: can't get mut ref for states buffer");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        };
        d.truncate(0);
        file.read_to_end(d).await?;
        let _ = d;

        let (boc_header, _) = BocReader::new().read_header(&mut Cursor::new(data.deref()))?;
        let cells_count = boc_header.cells_count;
        cells_index.resize(cells_count, (UInt256::default(), 0));
        cells_index.fill((UInt256::default(), 0));

        let start = std::time::Instant::now();

        let ss_db_ = ss_db.clone();
        let data_ = data.clone();
        let (_, cs) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut reader = BocReader::new();
            cells_index.resize(boc_header.cells_count, (UInt256::default(), 0));
            let mut cs = ss_db_.clone().create_fast_cell_storage(cells_index)?;
            let root = reader.read_inmem_to_storage(data_, &mut cs)?.withdraw_single_root()?;

            Ok((root, cs))
        })
        .await??;

        cells_index = cs.finish().await?;

        println!("Cells: {}", cells_count);
        println!("Time (read_inmem_to_storage): {} seconds", start.elapsed().as_secs());
    }

    Ok(())
}
