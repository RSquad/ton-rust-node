/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use rand::Rng;
use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{Read, Seek},
    path::Path,
    process,
    sync::Arc,
    time::Instant,
};
#[cfg(feature = "telemetry")]
use storage::StorageTelemetry;
use storage::{
    db::rocksdb::{AccessType, RocksDb},
    dynamic_boc_rc_db::DynamicBocDb,
    shardstate_db_async::{CellsDbConfig, ShardStateDb},
    StorageAlloc,
};
use ton_block::{BocFlags, BocReader, BocWriter, BuilderData, Cell, Result, UInt256};

fn generate_boc(path: &str, need_cells: usize) -> Result<()> {
    println!("Generating BOC at '{path}' with {need_cells} cells...");

    let rng = &mut rand::thread_rng();
    let bottom_level_cells = (need_cells as f32 * 0.08) as usize;
    let now = Instant::now();
    // starting from the bottom level
    let mut cells: Vec<Cell> = vec![];
    let mut bottom_level = true;
    // let mut level = 0;
    let mut total_cells = 0;
    let mut hashes = ahash::HashSet::default();
    loop {
        let mut next_level_cells = vec![];
        // level += 1;
        // println!("Next level #{level} started; prev level cells: {cells}", cells = cells.len());

        while !cells.is_empty() || bottom_level && next_level_cells.len() < bottom_level_cells {
            if rng.gen_range(1..4) == 1 && !cells.is_empty() {
                next_level_cells.push(cells.pop().unwrap());
                continue;
            }
            let mut builder = BuilderData::new();
            let bits = rng.gen_range(0..=1023usize);
            let data = vec![0_u8; bits.div_ceil(8)];
            // rng.fill(&mut data[..]); // use zeroed data to get more identical cells
            builder.append_raw(&data, bits).unwrap();

            let rc = match rng.gen_range(0..100) {
                0..=40 => 2,
                41..=80 => 3,
                81..=95 => 1,
                _ => 0,
            };

            for _ in 0..rc {
                if cells.is_empty() {
                    break;
                }
                let child = if rng.gen_range(1..3) == 1 {
                    cells[rng.gen_range(0..cells.len())].clone()
                } else {
                    cells.pop().unwrap()
                };
                builder.checked_append_reference(child).unwrap();
            }

            total_cells += 1;
            let cell = builder.into_cell().unwrap();
            hashes.insert(cell.repr_hash().clone());
            next_level_cells.push(cell);
        }
        bottom_level = false;
        cells = next_level_cells;

        if cells.len() == 1 {
            let elapsed = now.elapsed();
            let root = cells.pop().unwrap();
            println!(
                "Tree generating done in {elapsed:?}; needed cells {need_cells}  uniq cells {} \
                total cells: {total_cells}",
                hashes.len()
            );
            println!("Root hash: {:x}", root.repr_hash());
            println!("Writing BOC to file: {path}");
            let now = Instant::now();
            BocWriter::with_flags([root], BocFlags::all())?.write_to_file(path)?;
            println!("BOC successfully written in {:?}", now.elapsed());
            return Ok(());
        }
    }
}

async fn apply_boc(boc_path: &str, db_path: &str, cleanup: bool) -> Result<()> {
    if cleanup {
        println!("Cleaning up DB at '{db_path}'");
        let _ = std::fs::remove_dir_all(db_path);
    }
    println!("Applying BOC from '{boc_path}' to DB '{db_path}'");
    let mut cfs_opts = HashMap::new();
    cfs_opts.insert(
        "cells".to_string(),
        DynamicBocDb::build_cells_cf_options(&CellsDbConfig::default()),
    );
    cfs_opts.insert(
        "counters".to_string(),
        DynamicBocDb::build_counters_cf_options(&CellsDbConfig::default()),
    );
    let db = RocksDb::new(db_path, "db", cfs_opts, AccessType::ReadWrite)?;
    let ss_db = ShardStateDb::new(
        db.clone(),
        "shardstate_db",
        "cells",
        "counters",
        CellsDbConfig::default(),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )?;

    let mut boc_file = File::open(boc_path)?;
    let (boc_header, _) = BocReader::new().read_header(&mut boc_file)?;
    let cells_index = vec![(UInt256::default(), 0); boc_header.cells_count];
    let mut cs = ss_db.clone().create_fast_cell_storage(cells_index)?;
    let file_size = boc_file.metadata()?.len();
    let mut data = Vec::with_capacity(file_size as usize);
    boc_file.seek(std::io::SeekFrom::Start(0))?;
    boc_file.read_to_end(&mut data)?;
    let now = Instant::now();
    tokio::task::spawn_blocking(move || -> Result<_> {
        let result = BocReader::new().read_to_storage(data.as_slice(), &mut cs)?;

        let elapsed = now.elapsed();
        let cells = result.header.cells_count;
        println!(
            "BOC applied successfully, {cells} cells processed in {elapsed:?} ({:.0} cells/sec)",
            cells as f64 / elapsed.as_secs_f64()
        );
        println!("Root hash: {:x}", result.roots[0].repr_hash());
        Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::main]
async fn main() {
    let usage = "Usage: benchmark <command> [options]\n\
              Commands:\n\
              - generate-boc <path> <cells_count>\n\
              - apply-boc <boc-path> <db-path> [--cleanup]";

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("{usage}");
        process::exit(1);
    }

    match args[1].as_str() {
        "generate-boc" => {
            if args.len() != 4 {
                println!("{usage}");
                process::exit(1);
            }
            let path = &args[2];
            let cells_count = args[3].parse::<usize>().unwrap_or_else(|_| {
                eprintln!("cells_count must be a positive integer");
                process::exit(1);
            });
            if let Err(e) = generate_boc(path, cells_count) {
                eprintln!("Error generating BOC: {e}");
                process::exit(1);
            }
        }
        "apply-boc" => {
            if args.len() < 4 || args.len() > 5 {
                println!("{usage}");
                process::exit(1);
            }
            let boc_path = &args[2];
            let db_path = &args[3];
            let cleanup = args.get(4).map(|s| s == "--cleanup").unwrap_or(false);

            if !Path::exists(boc_path.as_ref()) {
                eprintln!("Invalid boc-path: {boc_path}");
                process::exit(1);
            }
            if let Err(e) = apply_boc(boc_path, db_path, cleanup).await {
                eprintln!("Error applying BOC: {e}");
                process::exit(1);
            }
        }
        _ => {
            println!("{usage}");
            process::exit(1);
        }
    }
}
