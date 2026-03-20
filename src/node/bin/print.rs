/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use node::collator_test_bundle::create_engine_telemetry;
use node::{
    collator_test_bundle::create_engine_allocated,
    internal_db::{InternalDb, InternalDbConfig, LAST_APPLIED_MC_BLOCK},
};
use std::sync::{atomic::AtomicU8, Arc};
use storage::{
    archives::package::read_package_from_file,
    db::{
        rocksdb::{AccessType, RocksDb},
        U32Key,
    },
};
use ton_block::{
    error, fail, read_boc, Account, AccountIdPrefixFull, Block, BlockIdExt, ConfigParams,
    Deserializable, HashmapAugType, McShardRecord, Result, ShardStateUnsplit,
};
use ton_block_json::{debug_account, debug_block, debug_block_full, debug_state, debug_state_full};

fn print_block(block: &Block, brief: bool) -> Result<()> {
    if brief {
        println!("{}", debug_block(block.clone())?);
    } else {
        println!("{}", debug_block_full(block)?);
    }
    Ok(())
}

fn print_state(state: &ShardStateUnsplit, brief: bool) -> Result<()> {
    if brief {
        println!("{}", debug_state(state.clone())?);
    } else {
        println!("{}", debug_state_full(state.clone())?);
    }
    Ok(())
}

async fn print_db_block(db: &InternalDb, block_id: BlockIdExt, brief: bool) -> Result<()> {
    println!("loading block: {}", block_id);
    let handle =
        db.load_block_handle(&block_id)?.ok_or_else(|| error!("Cannot load block {}", block_id))?;
    let block = db.load_block_data(&handle).await?;
    print_block(block.block()?, brief)
}

async fn print_db_state(db: &InternalDb, block_id: BlockIdExt, brief: bool) -> Result<()> {
    println!("loading state: {}", block_id);
    let state = db.load_shard_state_dynamic(&block_id)?;
    print_state(state.state()?, brief)
}

async fn print_shards(db: &InternalDb, block_id: BlockIdExt) -> Result<()> {
    println!("loading state: {}", block_id);
    let state = db.load_shard_state_dynamic(&block_id)?;
    if let Ok(shards) = state.shards() {
        shards.iterate_shards(|shard, descr| {
            let descr = McShardRecord::from_shard_descr(shard, descr);
            println!("before_merge: {} {}", descr.descr.before_merge, descr.block_id());
            Ok(true)
        })?;
    }
    Ok(())
}

// full BlockIdExt or masterchain seq_no
async fn get_block_id(db: &InternalDb, id: &str) -> Result<BlockIdExt> {
    if let Ok(id) = id.parse() {
        Ok(id)
    } else {
        let mc_seqno = id.parse()?;
        let (id, _) = db
            .lookup_block_by_seqno(&AccountIdPrefixFull::any_masterchain(), mc_seqno)
            .await?
            .ok_or_else(|| error!("no block with mc seqno {}", mc_seqno))?;
        Ok(id)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = clap::Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            clap::Arg::new("PATH")
                .short('p')
                .long("path")
                .help("path to DB")
                .default_value("node_db")
                .num_args(1),
        )
        .arg(clap::Arg::new("BLOCK").short('b').long("block").help("print block").num_args(1))
        .arg(clap::Arg::new("STATE").short('s').long("state").help("print state").num_args(1))
        .arg(
            clap::Arg::new("SHARDS")
                .short('r')
                .long("shards")
                .help("shard ids from master with seqno")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("LAST_ACCOUNTS")
                .short('a')
                .long("accounts")
                .action(clap::ArgAction::SetTrue)
                .help(
                    "print all accounts from all shards of workchains and masterchain \
                    for last applied state",
                ),
        )
        .arg(
            clap::Arg::new("BOC")
                .short('c')
                .long("boc")
                .help("print containtment of bag of cells")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("PACKAGE")
                .short('k')
                .long("package")
                .help("print package info")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("BRIEF").short('i').long("brief").action(clap::ArgAction::SetTrue).help(
                "print brief info \
                    (block without messages and transactions, state without accounts)",
            ),
        )
        .arg(
            clap::Arg::new("TABLES")
                .long("tables")
                .help("print list of DB table names")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("TABLE_U32")
                .long("table_u32")
                .help("print info for table with u32 keys")
                .num_args(1),
        )
        .get_matches();

    let brief = args.get_flag("BRIEF");
    if let Some(path) = args.get_one::<String>("BOC") {
        let bytes = std::fs::read(path)?;
        let res = read_boc(&bytes)?;
        println!("{:?}", res.header);
        if res.roots.len() > 1 {
            for root in res.roots {
                println!("{0:#.1$}", root, res.header.cells_count);
            }
        } else if let Ok(block) = Block::construct_from_cell(res.roots[0].clone()) {
            print_block(&block, brief)?;
        } else if let Ok(state) = ShardStateUnsplit::construct_from_cell(res.roots[0].clone()) {
            print_state(&state, brief)?;
        } else if let Ok(account) = Account::construct_from_cell(res.roots[0].clone()) {
            if let Some(data) = account.data().and_then(|data| data.reference(0).ok()) {
                let config_params = ConfigParams::with_root(data);
                let mut json = Default::default();
                let mode = ton_block_json::SerializationMode::Debug;
                if ton_block_json::serialize_config(&mut json, &config_params, mode).is_ok() {
                    println!("config params: {}", serde_json::to_string_pretty(&json)?);
                }
            }
            println!("{}", debug_account(account)?);
        }
    } else if let Some(path) = args.get_one::<String>("PACKAGE") {
        let mut reader = read_package_from_file(path).await?;
        let mut count = 0;
        while let Some(entry) = reader.next().await? {
            println!("{}", entry.filename());
            count += 1;
        }
        println!("Total {count} entries");
    }
    if let Some(db_dir) = args.get_one::<String>("PATH") {
        if args.get_flag("TABLES") || args.get_one::<String>("TABLE_U32").is_some() {
            // Direct access to RocksDB
            let db = RocksDb::new(db_dir, "", None, AccessType::ReadOnly)?;
            if args.get_flag("TABLES") {
                if let Some(names) = db.cfs() {
                    for name in names.iter() {
                        println!("{name}");
                    }
                } else {
                    println!("No tables in DB {db_dir}");
                }
            }
            if let Some(cf) = args.get_one::<String>("TABLE_U32") {
                let Ok(cf) = db.clone().table::<U32Key>(cf, false) else {
                    fail!("No {cf} Column Family in DB {db_dir}");
                };
                cf.for_each(&mut |key, val| {
                    let key = hex::encode(key);
                    let val = match str::from_utf8(val) {
                        Ok(val) => val.to_string(),
                        _ => hex::encode(val),
                    };
                    println!("{key} -> {val}");
                    Ok(true)
                })?;
            }
            return Ok(());
        }
        let db_config = InternalDbConfig { db_directory: db_dir.to_string(), ..Default::default() };
        let db = InternalDb::with_update(
            db_config,
            false,
            false,
            false,
            &|| Ok(()),
            None,
            Arc::new(AtomicU8::new(0)),
            Some(AccessType::ReadOnly),
            #[cfg(feature = "telemetry")]
            create_engine_telemetry(),
            create_engine_allocated(),
        )
        .await?;
        if let Some(block_id) = args.get_one::<String>("BLOCK") {
            let block_id = get_block_id(&db, block_id).await?;
            print_db_block(&db, block_id, brief).await?;
        }
        if let Some(block_id) = args.get_one::<String>("STATE") {
            let block_id = get_block_id(&db, block_id).await?;
            print_db_state(&db, block_id, brief).await?;
        }
        if let Some(block_id) = args.get_one::<String>("SHARDS") {
            let block_id = get_block_id(&db, block_id).await?;
            print_shards(&db, block_id).await?;
        }
        if args.get_flag("LAST_ACCOUNTS") {
            let last_mc_id = db
                .load_full_node_state(LAST_APPLIED_MC_BLOCK)?
                .ok_or_else(|| error!("no info about last applied mc block"))?;
            println!("{{\"accounts\":[");
            let mut first = true;
            let last_mc_state = db.load_shard_state_dynamic(&last_mc_id)?;
            let mut top_blocks = last_mc_state.top_blocks_all()?;
            top_blocks.push((*last_mc_id).clone());
            for block_id in &top_blocks {
                let state = db.load_shard_state_dynamic(block_id)?;
                state.state()?.read_accounts()?.iterate_objects(|shard_account| {
                    let account = shard_account.read_account()?;
                    let addr = account.get_addr().unwrap();
                    let balance = account.balance().unwrap();
                    let mut acc = serde_json::json!({
                        "id": addr.to_string(),
                        "last_paid": account.storage_info().unwrap().last_paid(),
                        "last_trans_lt": account.last_tr_time().unwrap_or_default(),
                        "balance": balance.coins.as_u128(),
                    });
                    if !balance.other.is_empty() {
                        let mut other = serde_json::Map::new();
                        balance.other.iterate_with_keys(|k: u32, v| {
                            other.insert(k.to_string(), v.value().to_string().into());
                            Ok(true)
                        })?;
                        if let Some(map) = acc.as_object_mut() {
                            map.insert("balance_other".to_string(), other.into());
                        }
                    };
                    if !first {
                        println!(",");
                    } else {
                        first = false;
                    }
                    print!("{:#}", acc);
                    Ok(true)
                })?;
            }
            println!("]}}");
        }
    }
    Ok(())
}
