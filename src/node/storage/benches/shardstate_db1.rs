/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use rand::SeedableRng;
use std::{collections::HashMap, sync::Arc};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    dynamic_boc_rc_db::DynamicBocDb,
    shardstate_db_async::{CellsDbConfig, ShardStateDb, SsNotificationCallback},
    StorageAlloc,
};
use ton_block::{BlockIdExt, Cell, Result, ShardIdent};

mod helper;
use helper::*;

include!("../../../common/src/log.rs");

const DB_PATH: &str = "../target/test";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let mut rng = rand::rngs::SmallRng::from_seed([123; 32]);
    let cells = 3_000_000;
    let number_of_bocs = 1;
    let update = 5_000;
    let updates = 500;

    init_log("../../../common/config/log_cfg_debug.yml");

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

    let (db, ss_db) = open_db()?;
    let mut cell = Cell::default();
    let mut first_put_time = vec![];
    let mut block_id = BlockIdExt::default();

    for _ in 0..number_of_bocs {
        cell = generate_boc(cells, &mut rng);
        // print the cell
        // println!("{:#.2}", cell);

        block_id = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: 0,
            root_hash: cell.repr_hash(),
            file_hash: Default::default(),
        };

        let cb = SsNotificationCallback::new();
        let first_put = std::time::Instant::now();

        ss_db.put(&block_id, cell.clone(), Some(cb.clone())).await?;
        cb.wait().await;

        first_put_time.push(first_put.elapsed().as_millis());
    }
    ss_db.stop().await;
    drop(ss_db);
    drop(db);

    let (_db, ss_db) = open_db()?;

    let before_updates = std::time::Instant::now();

    for i in 0..updates {
        let mut new_cells = 0;
        cell = update_boc(&cell, update, &mut new_cells, &mut rng)?;
        println!("updated {new_cells}");

        block_id = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: block_id.seq_no + 1,
            root_hash: cell.repr_hash(),
            file_hash: Default::default(),
        };

        if i != updates - 1 {
            ss_db.put(&block_id, cell.clone(), None).await?;
        } else {
            let cb = SsNotificationCallback::new();
            ss_db.put(&block_id, cell.clone(), Some(cb.clone())).await?;
            cb.wait().await;
        }
    }

    println!("Cells: {cells}");
    println!("Update cells: {update}");
    println!("Updates: {updates}");
    println!("Bocs: {number_of_bocs}");
    print!("first put time: ");
    for t in first_put_time {
        print!("{} ", t);
    }
    println!();
    let update_total = before_updates.elapsed().as_millis();
    println!("updates total time {}, per update {}", update_total, update_total / updates as u128);

    ss_db.stop().await;

    Ok(())
}
