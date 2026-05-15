/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use clap::Parser;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use ton_block::{
    base64_encode, error, write_boc, Augmentation, ConfigParam12, ConfigParamEnum,
    CurrencyCollection, HashmapAugType, Result, Serializable, ShardAccount, ShardIdent,
    ShardStateUnsplit, UInt256, MASTERCHAIN_ID, SHARD_FULL,
};
use ton_block_json::parse_state;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[arg(value_name = "INPUT_JSON", help = "input JSON file with masterchain zerostate")]
    input: PathBuf,
    #[arg(
        short = 'o',
        long = "output-dir",
        value_name = "DIR",
        default_value = ".",
        help = "output directory for generated files"
    )]
    out_dir: PathBuf,
}

#[derive(Debug)]
struct WcOutput {
    wc_id: i32,
    root_hash: UInt256,
    file_hash: UInt256,
    boc: Vec<u8>,
}

fn log_stage(stage: &str, message: &str) {
    eprintln!("[{stage}] {message}");
}

fn parse_balance_overrides(
    map: &serde_json::Map<String, serde_json::Value>,
) -> HashMap<[u8; 32], u128> {
    let mut out = HashMap::new();
    let Some(serde_json::Value::Array(accounts)) = map.get("accounts") else { return out };
    for entry in accounts {
        let Some(hex) = entry.get("id").and_then(|v| v.as_str()).and_then(|s| s.split(':').nth(1))
        else {
            continue;
        };
        let Some(bal) =
            entry.get("balance").and_then(|v| v.as_str()).and_then(|s| s.parse::<u128>().ok())
        else {
            continue;
        };
        let Ok(id) = hex::decode(hex) else { continue };
        if id.len() == 32 {
            out.insert(<[u8; 32]>::try_from(id.as_slice()).unwrap(), bal);
        }
    }
    out
}

fn load_mc_state(input: &Path) -> Result<(ShardStateUnsplit, HashMap<[u8; 32], u128>)> {
    let json = std::fs::read_to_string(input)
        .map_err(|e| error!("failed to read input {}: {e}", input.display()))?;
    let map = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
        .map_err(|e| error!("invalid zerostate JSON: {e}"))?;
    let overrides = parse_balance_overrides(&map);
    let state = parse_state(&map)
        .map_err(|e| error!("failed to parse masterchain state from JSON: {e}"))?;
    Ok((state, overrides))
}

fn transform_mc_state(
    mc_state: &mut ShardStateUnsplit,
    balance_overrides: &HashMap<[u8; 32], u128>,
) -> Result<Vec<WcOutput>> {
    let gen_time = mc_state.gen_time();
    let mut mc_extra = mc_state
        .read_custom()?
        .ok_or_else(|| error!("masterchain state must contain McStateExtra"))?;
    let mut wc_map = mc_extra.config.workchains()?;
    let mut wc_outputs = Vec::new();

    wc_map.clone().iterate_with_keys(|wc_id, mut wc_descr| {
        let shard = ShardIdent::with_tagged_prefix(wc_id, SHARD_FULL)?;
        let mut wc_state = ShardStateUnsplit::with_ident(shard);
        wc_state.set_gen_time(gen_time);
        wc_state.set_global_id(mc_state.global_id());
        wc_state.set_min_ref_mc_seqno(u32::MAX);

        let wc_cell = wc_state.serialize()?;
        wc_descr.zerostate_root_hash = wc_cell.repr_hash().clone();
        let wc_boc = write_boc(&wc_cell)?;
        wc_descr.zerostate_file_hash = UInt256::calc_file_hash(&wc_boc);
        wc_map.set(&wc_id, &wc_descr)?;
        wc_outputs.push(WcOutput {
            wc_id,
            root_hash: wc_descr.zerostate_root_hash,
            file_hash: wc_descr.zerostate_file_hash,
            boc: wc_boc,
        });
        Ok(true)
    })?;

    mc_extra
        .config
        .set_config(ConfigParamEnum::ConfigParam12(ConfigParam12 { workchains: wc_map }))?;
    let catchain_cfg = mc_extra.config.catchain_config()?;
    let validators = mc_extra.config.validator_set()?;
    let (_subset, hash_short) =
        validators.calc_subset(&catchain_cfg, SHARD_FULL, MASTERCHAIN_ID, 0)?;
    mc_extra.validator_info.validator_list_hash_short = hash_short;
    mc_extra.validator_info.nx_cc_updated = true;
    mc_extra.validator_info.catchain_seqno = 0;
    mc_state.write_custom(Some(&mc_extra))?;
    // Keep config contract data in sync with `master.config` params from JSON.
    // Without this, config account may carry an unrelated config dict.
    mc_state.update_config_smc()?;
    // Reset last_trans_lt and apply balance overrides.
    let mut accounts = mc_state.read_accounts()?;
    let mut patches = Vec::new();
    accounts.iterate_with_keys(|id, sa| {
        let mut acc = sa.read_account()?;
        acc.set_last_tr_time(0);
        if let Ok(id_bytes) = <[u8; 32]>::try_from(id.get_bytestring(0).as_slice()) {
            if let Some(&bal) = balance_overrides.get(&id_bytes) {
                log_stage("balance", &format!("{} → {bal} nanotons", hex::encode(id_bytes)));
                let coins: ton_block::Coins = bal.to_string().parse().expect("balance overflow");
                acc.set_balance(CurrencyCollection::from_coins(coins));
            }
        }
        patches.push((id, ShardAccount::with_params(&acc, UInt256::ZERO, 0)?, acc.aug()?));
        Ok(true)
    })?;
    for (id, sa, aug) in patches {
        accounts.set(&id, &sa, &aug)?;
    }
    mc_state.write_accounts(&accounts)?;
    let total_bal = mc_state.read_accounts()?.root_extra().balance().clone();
    mc_state.set_total_balance(total_bal);
    Ok(wc_outputs)
}

fn emit_outputs(
    out_dir: &Path,
    mc_state: &ShardStateUnsplit,
    wc_outputs: &[WcOutput],
) -> Result<()> {
    let base = wc_outputs.iter().find(|wc| wc.wc_id == 0).ok_or_else(|| {
        error!("workchain 0 is missing in config.p12, cannot emit basestate0.boc")
    })?;
    let base_path = out_dir.join("basestate0.boc");
    std::fs::write(&base_path, &base.boc)
        .map_err(|e| error!("failed to write basestate0 BOC {}: {e}", base_path.display()))?;
    log_stage("emit", &format!("basestate0: {}", base_path.display()));

    let mc_cell = mc_state.serialize()?;
    let mc_boc = write_boc(&mc_cell)?;
    let mc_root_hash = mc_cell.repr_hash();
    let mc_file_hash = UInt256::calc_file_hash(&mc_boc);

    let zero_path = out_dir.join("zerostate.boc");
    std::fs::write(&zero_path, &mc_boc)
        .map_err(|e| error!("failed to write zerostate BOC {}: {e}", zero_path.display()))?;
    log_stage("emit", &format!("zerostate: {}", zero_path.display()));

    let cfg_json = serde_json::json!({
        "zero_state": {
            "workchain": -1,
            "shard": -9223372036854775808i64,
            "seqno": 0,
            "root_hash": base64_encode(mc_root_hash.as_slice()),
            "file_hash": base64_encode(mc_file_hash.as_slice()),
        },
        "base_state": {
            "workchain": 0,
            "shard": -9223372036854775808i64,
            "seqno": 0,
            "root_hash": base64_encode(base.root_hash.as_slice()),
            "file_hash": base64_encode(base.file_hash.as_slice()),
        }
    });
    let cfg_str = serde_json::to_string_pretty(&cfg_json)?;
    let cfg_path = out_dir.join("config.json");
    std::fs::write(&cfg_path, &cfg_str)
        .map_err(|e| error!("failed to write config JSON {}: {e}", cfg_path.display()))?;
    log_stage("emit", &format!("config: {}", cfg_path.display()));
    println!("{cfg_str}");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    std::fs::create_dir_all(&cli.out_dir)
        .map_err(|e| error!("failed to create output dir {}: {e}", cli.out_dir.display()))?;

    let (mut mc_state, balance_overrides) =
        load_mc_state(&cli.input).map_err(|e| error!("load stage failed: {e}"))?;

    let wc_outputs = transform_mc_state(&mut mc_state, &balance_overrides)
        .map_err(|e| error!("transform stage failed: {e}"))?;

    emit_outputs(&cli.out_dir, &mc_state, &wc_outputs)
        .map_err(|e| error!("emit stage failed: {e}"))?;
    Ok(())
}
