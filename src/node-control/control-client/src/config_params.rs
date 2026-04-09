/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use std::str::FromStr;
use ton_block::{
    Coins, ConfigParam15, ConfigParam16, ConfigParam17, SigPubKey, UInt256, ValidatorDescr,
    ValidatorSet,
};

pub fn parse_config_param_15(bytes: &[u8]) -> anyhow::Result<ConfigParam15> {
    let param: serde_json::Value =
        serde_json::from_slice(bytes).context("config param 15 is not valid JSON")?;
    let p15 = param
        .get("p15")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("p15 not found in config param JSON"))?;

    Ok(ConfigParam15 {
        validators_elected_for: p15
            .get("validators_elected_for")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        elections_start_before: p15
            .get("elections_start_before")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        elections_end_before: p15
            .get("elections_end_before")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        stake_held_for: p15.get("stake_held_for").and_then(serde_json::Value::as_u64).unwrap_or(0)
            as u32,
    })
}

pub fn parse_config_param_34(bytes: &[u8]) -> anyhow::Result<ValidatorSet> {
    parse_validator_set(bytes, "p34")
}

pub fn parse_config_param_36(bytes: &[u8]) -> anyhow::Result<ValidatorSet> {
    parse_validator_set(bytes, "p36")
}

fn parse_validator_set(bytes: &[u8], key: &str) -> anyhow::Result<ValidatorSet> {
    let param: serde_json::Value = serde_json::from_slice(bytes)?;
    let map = param
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid config param"))?
        .get(key)
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("{} entry not found", key))?;
    let utime_since = map
        .get("utime_since")
        .and_then(|value| value.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| anyhow::anyhow!("utime_since"))?;
    let utime_until = map
        .get("utime_until")
        .and_then(|value| value.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| anyhow::anyhow!("utime_until"))?;
    let _ = map
        .get("total")
        .and_then(|value| value.as_u64())
        .map(|v| v as u16)
        .ok_or_else(|| anyhow::anyhow!("total"))?;
    let main = map
        .get("main")
        .and_then(|value| value.as_u64())
        .map(|v| v as u16)
        .ok_or_else(|| anyhow::anyhow!("main"))?;
    let json_list = map
        .get("list")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("list"))?;
    let mut list = vec![];
    for entry in json_list {
        let map = entry.as_object().ok_or_else(|| anyhow::anyhow!("invalid list entry"))?;
        let pubkey = map
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(hex::decode)
            .transpose()?
            .ok_or(anyhow::anyhow!("public_key"))?;
        let weight = map
            .get("weight_dec")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or(anyhow::anyhow!("weight"))?;
        let adnl_addr =
            map.get("adnl_addr").and_then(|v| v.as_str()).map(UInt256::from_str).transpose()?;
        let descr = ValidatorDescr {
            public_key: SigPubKey::from_bytes(&pubkey)
                .map_err(|_| anyhow::anyhow!("public key is invalid"))?,
            weight,
            adnl_addr,
            mc_seq_no_since: 0,
            prev_weight_sum: 0,
        };
        list.push(descr);
    }
    ValidatorSet::new(utime_since, utime_until, main, list)
}

pub fn parse_config_param_16(bytes: &[u8]) -> anyhow::Result<ConfigParam16> {
    let param: serde_json::Value =
        serde_json::from_slice(bytes).context("config param 16 is not valid JSON")?;
    let p16 = param
        .get("p16")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("p16 not found in JSON"))?;

    let max_validators =
        p16.get("max_validators")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("max_validators not found"))? as u16;
    let max_main_validators =
        p16.get("max_main_validators")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("max_main_validators not found"))? as u16;
    let min_validators =
        p16.get("min_validators")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("min_validators not found"))? as u16;

    Ok(ConfigParam16 {
        max_validators: max_validators.into(),
        max_main_validators: max_main_validators.into(),
        min_validators: min_validators.into(),
    })
}

pub fn parse_config_param_17(bytes: &[u8]) -> anyhow::Result<ConfigParam17> {
    let param: serde_json::Value =
        serde_json::from_slice(bytes).context("config param 17 is not valid JSON")?;
    let p17 = param
        .get("p17")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("p17 not found in JSON"))?;

    let parse_coins = |key: &str| -> anyhow::Result<Coins> {
        let val = p17.get(key).ok_or_else(|| anyhow::anyhow!("{} not found", key))?;
        // It can be a string (decimal) or a number
        if let Some(s) = val.as_str() {
            Ok(Coins::from(u64::from_str_radix(s, 10).context(format!("parse {} as u64", key))?))
        } else if let Some(n) = val.as_u64() {
            Ok(Coins::from(n))
        } else {
            anyhow::bail!("{} is not a valid coins value", key)
        }
    };

    let min_stake = parse_coins("min_stake_dec")?;
    let max_stake = parse_coins("max_stake_dec")?;
    let min_total_stake = parse_coins("min_total_stake_dec")?;
    let max_stake_factor =
        p17.get("max_stake_factor")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("max_stake_factor not found"))? as u32;

    Ok(ConfigParam17 { min_stake, max_stake, min_total_stake, max_stake_factor })
}
