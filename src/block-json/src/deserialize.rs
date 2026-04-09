/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde_json::{Map, Value};
use std::str::FromStr;
use ton_block::*;

#[allow(dead_code)]
trait ParseJson {
    fn as_uint256(&self) -> Result<UInt256>;
    fn as_base64(&self) -> Result<Vec<u8>>;
    fn as_int(&self) -> Result<i32>;
    fn as_uint(&self) -> Result<u32>;
    fn as_long(&self) -> Result<i64>;
    fn as_ulong(&self) -> Result<u64>;
}

impl ParseJson for Value {
    fn as_uint256(&self) -> Result<UInt256> {
        self.as_str().ok_or_else(|| error!("field is not str"))?.parse()
    }
    fn as_base64(&self) -> Result<Vec<u8>> {
        base64_decode(self.as_str().ok_or_else(|| error!("field is not str"))?)
    }
    fn as_int(&self) -> Result<i32> {
        match self.as_i64() {
            Some(v) => Ok(v as i32),
            None => match self.as_str() {
                Some(s) => Ok(s.parse()?),
                None => Ok(i32::default()),
            },
        }
    }
    fn as_uint(&self) -> Result<u32> {
        match self.as_u64() {
            Some(v) => Ok(v as u32),
            None => match self.as_str() {
                Some(s) => Ok(s.parse()?),
                None => Ok(u32::default()),
            },
        }
    }
    fn as_long(&self) -> Result<i64> {
        match self.as_i64() {
            Some(v) => Ok(v),
            None => match self.as_str() {
                Some(s) => Ok(i64::from_str(s)?),
                None => Ok(i64::default()),
            },
        }
    }
    fn as_ulong(&self) -> Result<u64> {
        match self.as_u64() {
            Some(v) => Ok(v),
            None => match self.as_str() {
                Some(s) => Ok(s.parse()?),
                None => Ok(u64::default()),
            },
        }
    }
}

#[derive(Debug)]
pub struct PathMap<'m, 'a> {
    map: &'m Map<String, Value>,
    path: Vec<&'a str>,
}

impl<'m, 'a> PathMap<'m, 'a> {
    pub fn new(map: &'m Map<String, Value>) -> Self {
        Self { map, path: vec!["root"] }
    }
    pub fn cont(prev: &Self, name: &'a str, value: &'m Value) -> Result<Self> {
        let map = value.as_object().ok_or_else(|| {
            error!("{}/{} must be the vector of objects", prev.path.join("/"), name)
        })?;
        let mut path = prev.path.clone();
        path.push(name);
        Ok(Self { map, path })
    }
    pub fn iter(&self) -> serde_json::map::Iter<'m> {
        self.map.iter()
    }
    pub fn get_item(&self, name: &'a str) -> Result<&'m Value> {
        let item = self
            .map
            .get(name)
            .ok_or_else(|| error!("{} must have the field `{}`", self.path.join("/"), name))?;
        Ok(item)
    }
    pub fn get_obj(&self, name: &'a str) -> Result<Self> {
        let map = self
            .get_item(name)?
            .as_object()
            .ok_or_else(|| error!("{}/{} must be the object", self.path.join("/"), name))?;
        let mut path = self.path.clone();
        path.push(name);
        Ok(Self { map, path })
    }
    pub fn get_obj_vec(&self, name: &'a str) -> Result<Vec<Self>> {
        let vec = self.get_vec(name)?;
        let mut result = Vec::with_capacity(vec.len());
        for val in vec {
            let map = val.as_object().ok_or_else(|| {
                error!("{}/{} must be the vector of objects", self.path.join("/"), name)
            })?;
            let mut path = self.path.clone();
            path.push(name);
            result.push(Self { map, path });
        }
        Ok(result)
    }
    pub fn get_vec(&self, name: &'a str) -> Result<&'m Vec<Value>> {
        self.get_item(name)?
            .as_array()
            .ok_or_else(|| error!("{}/{} must be the vector", self.path.join("/"), name))
    }
    pub fn get_str(&self, name: &'a str) -> Result<&'m str> {
        self.get_item(name)?
            .as_str()
            .ok_or_else(|| error!("{}/{} must be the string", self.path.join("/"), name))
    }
    pub fn get_uint256(&self, name: &'a str) -> Result<UInt256> {
        self.get_str(name)?.parse().map_err(|err| {
            error!("{}/{} must be the uint256 in hex format : {}", self.path.join("/"), name, err)
        })
    }
    pub fn get_base64(&self, name: &'a str) -> Result<Vec<u8>> {
        base64_decode(self.get_str(name)?)
            .map_err(|err| error!("{}/{} must be the base64 : {}", self.path.join("/"), name, err))
    }

    pub fn get_num(&self, name: &'a str) -> Result<i64> {
        if let Ok(value) = self.get_item(name) {
            if let Some(v) = value.as_i64() {
                return Ok(v);
            }
        }
        if let Ok(value) = self.get_item(&(name.to_string() + "_dec")) {
            if let Some(v) = value.as_str() {
                return i64::from_str(v).map_err(|err| {
                    error!(
                        "{}/{} must be the integer or a string with the integer {}: {}",
                        self.path.join("/"),
                        name,
                        v,
                        err
                    )
                });
            }
        }
        if let Ok(value) = self.get_item(name) {
            if let Some(v) = value.as_str() {
                if let Some(v) = v.strip_prefix("0x") {
                    return i64::from_str_radix(v, 16).map_err(|err| {
                        error!(
                            "{}/{} must be the integer or a string with the integer {}: {}",
                            self.path.join("/"),
                            name,
                            v,
                            err
                        )
                    });
                } else {
                    return i64::from_str(v).map_err(|err| {
                        error!(
                            "{}/{} must be the integer or a string with the integer {}: {}",
                            self.path.join("/"),
                            name,
                            v,
                            err
                        )
                    });
                }
            }
        }
        fail!("{}/{} must be the integer or a string with the integer", self.path.join("/"), name)
    }

    pub fn get_coins(&self, name: &'a str) -> Result<Coins> {
        if let Ok(value) = self.get_item(name) {
            if let Some(v) = value.as_u64() {
                return Ok(v.into());
            }
        }
        if let Ok(value) = self.get_item(&(name.to_string() + "_dec")) {
            if let Some(v) = value.as_str() {
                return Coins::from_str(v).map_err(|err| {
                    error!(
                        "{}/{} must be the integer or a string with the integer {}: {}",
                        self.path.join("/"),
                        name,
                        v,
                        err
                    )
                });
            }
        }
        if let Ok(value) = self.get_item(name) {
            if let Some(v) = value.as_str() {
                return Coins::from_str(v).map_err(|err| {
                    error!(
                        "{}/{} must be the integer or a string with the integer {}: {}",
                        self.path.join("/"),
                        name,
                        v,
                        err
                    )
                });
            }
        }
        fail!("{}/{} must be the integer or a string with the integer", self.path.join("/"), name)
    }

    pub fn get_u32(&self, name: &'a str, value: &mut u32) {
        if let Ok(new_value) = self.get_num32(name) {
            *value = new_value;
        }
    }
    pub fn get_u16(&self, name: &'a str, value: &mut u16) {
        if let Ok(new_value) = self.get_num16(name) {
            *value = new_value;
        }
    }
    pub fn get_u8(&self, name: &'a str, value: &mut u8) {
        if let Ok(new_value) = self.get_num8(name) {
            *value = new_value;
        }
    }
    pub fn get_num8(&self, name: &'a str) -> Result<u8> {
        Ok(self.get_num(name)? as u8)
    }
    pub fn get_num16(&self, name: &'a str) -> Result<u16> {
        Ok(self.get_num(name)? as u16)
    }
    pub fn get_num32(&self, name: &'a str) -> Result<u32> {
        Ok(self.get_num(name)? as u32)
    }
    pub fn get_num64(&self, name: &'a str) -> Result<u64> {
        Ok(self.get_num(name)? as u64)
    }
    pub fn get_bool(&self, name: &'a str) -> Result<bool> {
        self.get_item(name)?
            .as_bool()
            .ok_or_else(|| error!("{}/{} must be boolean", self.path.join("/"), name))
    }
}

struct StateParser {
    state: ShardStateUnsplit,
    extra: McStateExtra,
    mandatory_params: u64,
}

impl StateParser {
    fn new() -> Self {
        Self {
            state: ShardStateUnsplit::with_ident(ShardIdent::masterchain()),
            extra: McStateExtra::default(),
            mandatory_params: 0,
        }
    }

    fn for_zero_state() -> Self {
        // let mandatory_params = [0, 1, 2, 7, 8, 9, 10, 11, 12, 14, 15, 16, 17, 18,
        //     20, 21, 22, 23, 24, 25, 28, 29, 31, 34];
        // let mandatory_params = mandatory_params.iter().fold(0, |s, p| a |= 1 << p);
        // println!("0x{:X}", mandatory_params);
        Self {
            state: ShardStateUnsplit::with_ident(ShardIdent::masterchain()),
            extra: McStateExtra::default(),
            mandatory_params: 0x0000_0004_B3F7_CF87,
        }
    }

    fn is_need(&self, num: i32) -> bool {
        (num < 64) && ((self.mandatory_params >> num) & 1) != 0
    }

    fn parse_parameter(
        &mut self,
        config: &PathMap,
        num: i32,
        f: impl FnOnce(&PathMap) -> Result<ConfigParamEnum>,
    ) -> Result<()> {
        let p = format!("p{}", num);
        match config.get_obj(&p) {
            Ok(p) => self
                .extra
                .config
                .set_config(f(&p)?)
                .map_err(|err| error!("Can't set config for {} : {}", p.path.join("/"), err)),
            Err(err) if self.is_need(num) => {
                fail!("parameter p{} not found: {}", num, err)
            }
            _ => Ok(()),
        }
    }

    fn parse_array(
        &mut self,
        config: &PathMap,
        num: i32,
        f: impl FnOnce(&Vec<Value>) -> Result<ConfigParamEnum>,
    ) -> Result<()> {
        let p = format!("p{}", num);
        match config.get_vec(&p) {
            Ok(v) => {
                self.extra.config.set_config(f(v)?).map_err(|err| {
                    error!("Can't set config for {} : {}", config.path.join("/"), err)
                })
            }
            Err(err) if self.is_need(num) => {
                fail!("parameter p{} not found: {}", num, err)
            }
            _ => Ok(()),
        }
    }

    fn parse_uint256(
        &mut self,
        config: &PathMap,
        num: i32,
        f: impl FnOnce(AccountId) -> Result<ConfigParamEnum>,
    ) -> Result<()> {
        let p = format!("p{}", num);
        match config.get_uint256(&p) {
            Ok(p) => {
                self.extra.config.set_config(f(p.into())?).map_err(|err| {
                    error!("Can't set config for {} : {}", config.path.join("/"), err)
                })
            }
            Err(err) if self.is_need(num) => {
                fail!("parameter p{} not found: {}", num, err)
            }
            _ => Ok(()),
        }
    }

    fn parse_param_set_params(
        &mut self,
        config: &PathMap,
        num: i32,
    ) -> Result<Option<MandatoryParams>> {
        let p = format!("p{}", num);
        match config.get_vec(&p) {
            Ok(vec) => {
                let mut params = MandatoryParams::default();
                vec.iter().try_for_each(|n| params.add_key(&n.as_uint()?))?;
                Ok(Some(params))
            }
            Err(err) => {
                if self.is_need(num) {
                    Err(err)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn parse_param_limits(param: &PathMap) -> Result<ParamLimits> {
        ParamLimits::with_limits(
            param.get_num32("underload")?,
            param.get_num32("soft_limit")?,
            param.get_num32("hard_limit")?,
        )
    }

    fn parse_msg_queue_limits(param: &PathMap) -> Result<ImportedMsgQueueLimits> {
        Ok(ImportedMsgQueueLimits::new(param.get_num32("max_bytes")?, param.get_num32("max_msgs")?))
    }

    fn parse_block_limits_struct(param: &PathMap) -> Result<BlockLimits> {
        Ok(BlockLimits::with_limits(
            Self::parse_param_limits(&param.get_obj("bytes")?)?,
            Self::parse_param_limits(&param.get_obj("gas")?)?,
            Self::parse_param_limits(&param.get_obj("lt_delta")?)?,
            if let Ok(obj) = param.get_obj("collated_data") {
                Some(Self::parse_param_limits(&obj)?)
            } else {
                None
            },
            if let Ok(obj) = param.get_obj("imported_msg_queue") {
                Some(Self::parse_msg_queue_limits(&obj)?)
            } else {
                None
            },
        ))
    }

    fn parse_block_limits(&mut self, config: &PathMap) -> Result<()> {
        self.parse_parameter(config, 22, |p| {
            Ok(ConfigParamEnum::ConfigParam22(Self::parse_block_limits_struct(p)?))
        })?;
        self.parse_parameter(config, 23, |p| {
            Ok(ConfigParamEnum::ConfigParam23(Self::parse_block_limits_struct(p)?))
        })
    }

    fn parse_msg_forward_prices_struct(param: &PathMap) -> Result<MsgForwardPrices> {
        Ok(MsgForwardPrices {
            lump_price: param.get_num64("lump_price")?,
            bit_price: param.get_num64("bit_price")?,
            cell_price: param.get_num64("cell_price")?,
            ihr_price_factor: param.get_num32("ihr_price_factor")?,
            first_frac: param.get_num16("first_frac")?,
            next_frac: param.get_num16("next_frac")?,
        })
    }

    fn parse_msg_forward_prices(&mut self, config: &PathMap) -> Result<()> {
        self.parse_parameter(config, 24, |p| {
            Ok(ConfigParamEnum::ConfigParam24(Self::parse_msg_forward_prices_struct(p)?))
        })?;
        self.parse_parameter(config, 25, |p| {
            Ok(ConfigParamEnum::ConfigParam25(Self::parse_msg_forward_prices_struct(p)?))
        })
    }

    fn parse_gas_limits_struct(param: &PathMap) -> Result<GasLimitsPrices> {
        Ok(GasLimitsPrices {
            gas_price: param.get_num64("gas_price")?,
            gas_limit: param.get_num64("gas_limit")?,
            special_gas_limit: param.get_num64("special_gas_limit")?,
            gas_credit: param.get_num64("gas_credit")?,
            block_gas_limit: param.get_num64("block_gas_limit")?,
            freeze_due_limit: param.get_num64("freeze_due_limit")?,
            delete_due_limit: param.get_num64("delete_due_limit")?,
            flat_gas_limit: param.get_num64("flat_gas_limit")?,
            flat_gas_price: param.get_num64("flat_gas_price")?,
            max_gas_threshold: 0,
        })
    }

    fn parse_gas_limits(&mut self, config: &PathMap) -> Result<()> {
        self.parse_parameter(config, 20, |p| {
            Ok(ConfigParamEnum::ConfigParam20(Self::parse_gas_limits_struct(p)?))
        })?;
        self.parse_parameter(config, 21, |p| {
            Ok(ConfigParamEnum::ConfigParam21(Self::parse_gas_limits_struct(p)?))
        })
    }

    fn parse_storage_prices(&mut self, config: &PathMap) -> Result<()> {
        self.parse_array(config, 18, |p18| {
            let mut map = ConfigParam18Map::default();
            let mut index = 0u32;
            p18.iter().try_for_each::<_, Result<_>>(|value| {
                let p = PathMap::cont(config, "p18", value)?;
                let p = StoragePrices {
                    utime_since: p.get_num32("utime_since")?,
                    bit_price_ps: p.get_num64("bit_price_ps")?,
                    cell_price_ps: p.get_num64("cell_price_ps")?,
                    mc_bit_price_ps: p.get_num64("mc_bit_price_ps")?,
                    mc_cell_price_ps: p.get_num64("mc_cell_price_ps")?,
                };
                map.set(&index, &p)?;
                index += 1;
                Ok(())
            })?;
            Ok(ConfigParamEnum::ConfigParam18(ConfigParam18 { map }))
        })
    }

    fn parse_param_set(&mut self, config: &PathMap) -> Result<()> {
        if let Some(mandatory_params) = self.parse_param_set_params(config, 9)? {
            self.extra
                .config
                .set_config(ConfigParamEnum::ConfigParam9(ConfigParam9 { mandatory_params }))?;
        }
        if let Some(critical_params) = self.parse_param_set_params(config, 10)? {
            self.extra
                .config
                .set_config(ConfigParamEnum::ConfigParam10(ConfigParam10 { critical_params }))?;
        }
        Ok(())
    }

    fn parse_critical_params(params: &PathMap) -> Result<ConfigProposalSetup> {
        Ok(ConfigProposalSetup {
            min_tot_rounds: params.get_num8("min_tot_rounds")?,
            max_tot_rounds: params.get_num8("max_tot_rounds")?,
            min_wins: params.get_num8("min_wins")?,
            max_losses: params.get_num8("max_losses")?,
            min_store_sec: params.get_num32("min_store_sec")?,
            max_store_sec: params.get_num32("max_store_sec")?,
            bit_price: params.get_num32("bit_price")?,
            cell_price: params.get_num32("cell_price")?,
        })
    }

    fn parse_p11(&mut self, config: &PathMap) -> Result<()> {
        self.parse_parameter(config, 11, |p11| {
            let normal_params = Self::parse_critical_params(&p11.get_obj("normal_params")?)?;
            let critical_params = Self::parse_critical_params(&p11.get_obj("critical_params")?)?;
            let p11 = ConfigParam11::new(&normal_params, &critical_params)?;
            Ok(ConfigParamEnum::ConfigParam11(p11))
        })
    }

    fn parse_p12(&mut self, config: &PathMap) -> Result<()> {
        self.parse_array(config, 12, |p12| {
            let mut workchains = Workchains::default();
            p12.iter().try_for_each(|wc_info| {
                let wc_info = PathMap::cont(config, "p12", wc_info)?;
                let mut descr = WorkchainDescr::default();
                let workchain_id = wc_info.get_num32("workchain_id")?;
                descr.enabled_since = wc_info.get_num32("enabled_since")?;
                descr.set_min_split(wc_info.get_num8("min_split")?)?;
                descr.set_max_split(wc_info.get_num8("max_split")?)?;
                descr.flags = wc_info.get_num16("flags")?;
                descr.active = wc_info.get_bool("active")?;
                descr.accept_msgs = wc_info.get_bool("accept_msgs")?;
                descr.zerostate_root_hash = wc_info.get_uint256("zerostate_root_hash")?;
                descr.zerostate_file_hash = wc_info.get_uint256("zerostate_file_hash")?;
                descr.version = wc_info.get_num32("version")?;
                // TODO: check here
                descr.format = match wc_info.get_bool("basic")? {
                    true => {
                        let vm_version = wc_info.get_num("vm_version")? as i32;
                        let vm_mode = wc_info.get_num64("vm_mode")?;
                        WorkchainFormat::Basic(WorkchainFormat1::with_params(vm_version, vm_mode))
                    }
                    false => {
                        let min_addr_len = wc_info.get_num16("min_addr_len")?;
                        let max_addr_len = wc_info.get_num16("max_addr_len")?;
                        let addr_len_step = wc_info.get_num16("addr_len_step")?;
                        let workchain_type_id = wc_info.get_num32("workchain_type_id")?;
                        WorkchainFormat::Extended(WorkchainFormat0::with_params(
                            min_addr_len,
                            max_addr_len,
                            addr_len_step,
                            workchain_type_id,
                        )?)
                    }
                };
                workchains.set(&workchain_id, &descr)
            })?;
            Ok(ConfigParamEnum::ConfigParam12(ConfigParam12 { workchains }))
        })
    }

    fn parse_catchain_config(p28: &PathMap) -> Result<ConfigParamEnum> {
        Ok(ConfigParamEnum::ConfigParam28(CatchainConfig {
            shuffle_mc_validators: p28.get_bool("shuffle_mc_validators")?,
            isolate_mc_validators: p28.get_bool("isolate_mc_validators").unwrap_or_default(),
            mc_catchain_lifetime: p28.get_num32("mc_catchain_lifetime")?,
            shard_catchain_lifetime: p28.get_num32("shard_catchain_lifetime")?,
            shard_validators_lifetime: p28.get_num32("shard_validators_lifetime")?,
            shard_validators_num: p28.get_num32("shard_validators_num")?,
        }))
    }

    fn parse_consensus_config(p29: &PathMap) -> Result<ConfigParamEnum> {
        Ok(ConfigParamEnum::ConfigParam29(ConsensusConfig {
            new_catchain_ids: p29.get_bool("new_catchain_ids")?,
            round_candidates: p29.get_num32("round_candidates")?,
            next_candidate_delay_ms: p29.get_num32("next_candidate_delay_ms")?,
            consensus_timeout_ms: p29.get_num32("consensus_timeout_ms")?,
            fast_attempts: p29.get_num32("fast_attempts")?,
            attempt_duration: p29.get_num32("attempt_duration")?,
            catchain_max_deps: p29.get_num32("catchain_max_deps")?,
            max_block_bytes: p29.get_num32("max_block_bytes")?,
            max_collated_bytes: p29.get_num32("max_collated_bytes")?,
            catchain_max_blocks_coeff: p29
                .get_num32("catchain_max_blocks_coeff")
                .unwrap_or_default(),
            proto_version: p29.get_num16("proto_version").unwrap_or_default(),
        }))
    }

    fn parse_validator_set(config: &PathMap) -> Result<ValidatorSet> {
        let utime_since = config.get_num32("utime_since")?;
        let utime_until = config.get_num32("utime_until")?;
        //let total = config.get_num16("total")?;
        let main = config.get_num16("main")?;
        //let total_weight = config.get_num64("total_weight")?;

        let mut list = Vec::default();
        config.get_vec("list").and_then(|p| {
            p.iter().try_for_each::<_, Result<_>>(|p| {
                let p = PathMap::cont(config, "p", p)?;
                let public_key = hex::decode(p.get_str("public_key")?)?;
                let weight = p.get_num64("weight")?;
                let adnl_addr = p.get_uint256("adnl_addr").ok();

                let descr = ValidatorDescr::with_params(
                    SigPubKey::from_bytes(&public_key)?,
                    weight,
                    adnl_addr,
                );
                list.push(descr);

                Ok(())
            })?;
            Ok(())
        })?;

        let validator_set = ValidatorSet::new(utime_since, utime_until, main, list)?;
        Ok(validator_set)
    }

    pub fn parse_oracles(p: &PathMap) -> Result<Oracles> {
        let p = p.get_obj("oracles")?;
        let mut result = Oracles::default();
        for (key, value) in p.iter() {
            let key: UInt256 = key
                .parse()
                .map_err(|err| error!("{key} must be the uint256 in hex format : {err}"))?;
            let value = value.as_uint256().map_err(|err| {
                error!("{}/{key} must be the uint256 in hex format : {err}", p.path.join("/"))
            })?;
            result.set(&key, &value.into())?;
        }
        Ok(result)
    }

    pub fn parse_oracle_bridge_params(p: &PathMap) -> Result<OracleBridgeParams> {
        let oracles = Self::parse_oracles(p)?;
        Ok(OracleBridgeParams {
            oracles,
            bridge_address: p.get_uint256("bridge_address")?.into(),
            oracle_mutlisig_address: p.get_uint256("oracle_mutlisig_address")?.into(),
            external_chain_address: p.get_uint256("external_chain_address")?.into(),
        })
    }

    pub fn parse_jetton_bridge_params(p: &PathMap) -> Result<JettonBridgeParams> {
        let oracles = Self::parse_oracles(p)?;
        let prices = JettonBridgePrices {
            bridge_burn_fee: p.get_coins("bridge_burn_fee")?,
            bridge_mint_fee: p.get_coins("bridge_mint_fee")?,
            wallet_min_tons_for_storage: p.get_coins("wallet_min_tons_for_storage")?,
            wallet_gas_consumption: p.get_coins("wallet_gas_consumption")?,
            minter_min_tons_for_storage: p.get_coins("minter_min_tons_for_storage")?,
            discover_gas_consumption: p.get_coins("discover_gas_consumption")?,
        };
        Ok(JettonBridgeParams {
            prices,
            oracles,
            state_flags: p.get_num8("state_flags")?,
            bridge_address: p.get_uint256("bridge_address")?.into(),
            oracles_address: p.get_uint256("oracles_address")?.into(),
            external_chain_address: p.get_uint256("external_chain_address")?.into(),
        })
    }

    pub fn parse_accelerated_consensus_config(p63: &PathMap) -> Result<ConfigParamEnum> {
        Ok(ConfigParamEnum::ConfigParam63(AcceleratedConsensusConfig {
            enabled: p63.get_bool("enabled")?,
            failed_collation_retry_timeout_ms: p63
                .get_num32("failed_collation_retry_timeout_ms")?,
            skip_rounds_count_for_collator_rotation: p63
                .get_num32("skip_rounds_count_for_collator_rotation")?,
            max_precollated_blocks: p63.get_num32("max_precollated_blocks")?,
        }))
    }

    fn parse_simplex_config(p: &PathMap) -> Result<SimplexConfig> {
        let d = NoncriticalParams::default();
        Ok(SimplexConfig {
            use_quic: p.get_num32("use_quic").unwrap_or(0) != 0,
            slots_per_leader_window: p.get_num32("slots_per_leader_window")?,
            noncritical_params: NoncriticalParams {
                target_rate_ms: p.get_num32("target_rate_ms")?,
                first_block_timeout_ms: p.get_num32("first_block_timeout_ms")?,
                first_block_timeout_multiplier_bits: p
                    .get_num32("first_block_timeout_multiplier_bits")
                    .unwrap_or(d.first_block_timeout_multiplier_bits),
                first_block_timeout_cap_ms: p
                    .get_num32("first_block_timeout_cap_ms")
                    .unwrap_or(d.first_block_timeout_cap_ms),
                candidate_resolve_timeout_ms: p
                    .get_num32("candidate_resolve_timeout_ms")
                    .unwrap_or(d.candidate_resolve_timeout_ms),
                candidate_resolve_timeout_multiplier_bits: p
                    .get_num32("candidate_resolve_timeout_multiplier_bits")
                    .unwrap_or(d.candidate_resolve_timeout_multiplier_bits),
                candidate_resolve_timeout_cap_ms: p
                    .get_num32("candidate_resolve_timeout_cap_ms")
                    .unwrap_or(d.candidate_resolve_timeout_cap_ms),
                candidate_resolve_cooldown_ms: p
                    .get_num32("candidate_resolve_cooldown_ms")
                    .unwrap_or(d.candidate_resolve_cooldown_ms),
                standstill_timeout_ms: p
                    .get_num32("standstill_timeout_ms")
                    .unwrap_or(d.standstill_timeout_ms),
                standstill_max_egress_bytes_per_s: p
                    .get_num32("standstill_max_egress_bytes_per_s")
                    .unwrap_or(d.standstill_max_egress_bytes_per_s),
                max_leader_window_desync: p.get_num32("max_leader_window_desync")?,
                bad_signature_ban_duration_ms: p
                    .get_num32("bad_signature_ban_duration_ms")
                    .unwrap_or(d.bad_signature_ban_duration_ms),
                candidate_resolve_rate_limit: p
                    .get_num32("candidate_resolve_rate_limit")
                    .unwrap_or(d.candidate_resolve_rate_limit),
                min_block_interval_ms: p
                    .get_num32("min_block_interval_ms")
                    .unwrap_or(d.min_block_interval_ms),
                no_empty_blocks_on_error_timeout_ms: p
                    .get_num32("no_empty_blocks_on_error_timeout_ms")
                    .unwrap_or(d.no_empty_blocks_on_error_timeout_ms),
            },
        })
    }

    pub fn parse_new_consensus_config_all(p30: &PathMap) -> Result<ConfigParamEnum> {
        let mc = p30.get_obj("mc").ok().map(|p| Self::parse_simplex_config(&p)).transpose()?;
        let shard =
            p30.get_obj("shard").ok().map(|p| Self::parse_simplex_config(&p)).transpose()?;
        Ok(ConfigParamEnum::ConfigParam30(NewConsensusConfigAll { mc, shard }))
    }

    pub fn parse_config(&mut self, config: &PathMap) -> Result<()> {
        self.parse_uint256(config, 0, |config_addr| {
            Ok(ConfigParamEnum::ConfigParam0(ConfigParam0 { config_addr }))
        })?;
        self.parse_uint256(config, 1, |elector_addr| {
            Ok(ConfigParamEnum::ConfigParam1(ConfigParam1 { elector_addr }))
        })?;
        self.parse_uint256(config, 2, |minter_addr| {
            Ok(ConfigParamEnum::ConfigParam2(ConfigParam2 { minter_addr }))
        })?;
        self.parse_uint256(config, 3, |fee_collector_addr| {
            Ok(ConfigParamEnum::ConfigParam3(ConfigParam3 { fee_collector_addr }))
        })?;
        self.parse_uint256(config, 4, |dns_root_addr| {
            Ok(ConfigParamEnum::ConfigParam4(ConfigParam4 { dns_root_addr }))
        })?;

        self.parse_parameter(config, 5, |value| {
            let p5 = BurningConfig {
                blackhole_addr: value.get_uint256("blackhole_addr").ok().map(Into::into),
                fee_burn_num: value.get_num32("fee_burn_num")?,
                fee_burn_denom: value.get_num32("fee_burn_denom")?,
            };
            p5.check_validity()?;
            Ok(ConfigParamEnum::ConfigParam5(p5))
        })?;

        self.parse_parameter(config, 6, |value| {
            Ok(ConfigParamEnum::ConfigParam6(ConfigParam6 {
                mint_new_price: value.get_coins("mint_new_price")?,
                mint_add_price: value.get_coins("mint_add_price")?,
            }))
        })?;

        self.parse_array(config, 7, |p7| {
            let mut to_mint = ExtraCurrencyCollection::default();
            p7.iter().try_for_each(|currency| {
                let currency = PathMap::cont(config, "p7", currency)?;
                let value = if let Ok(value) = currency.get_str("value_dec") {
                    value.parse()?
                } else {
                    currency.get_str("value")?.parse()?
                };
                to_mint.set(&(currency.get_num32("currency")?), &value)
            })?;
            Ok(ConfigParamEnum::ConfigParam7(ConfigParam7 { to_mint }))
        })?;

        self.parse_parameter(config, 8, |p8| {
            Ok(ConfigParamEnum::ConfigParam8(ConfigParam8 {
                global_version: GlobalVersion {
                    version: p8.get_num32("version")?,
                    capabilities: p8.get_num64("capabilities")?,
                },
            }))
        })?;

        self.parse_param_set(config)?; // p9 p10
        self.parse_p11(config)?;
        self.parse_p12(config)?;

        self.parse_parameter(config, 13, |p13| {
            let cell = read_single_root_boc(p13.get_base64("boc")?)?;
            Ok(ConfigParamEnum::ConfigParam13(ConfigParam13 { cell }))
        })?;
        self.parse_parameter(config, 14, |p14| {
            Ok(ConfigParamEnum::ConfigParam14(ConfigParam14 {
                block_create_fees: BlockCreateFees {
                    masterchain_block_fee: p14.get_coins("masterchain_block_fee")?,
                    basechain_block_fee: p14.get_coins("basechain_block_fee")?,
                },
            }))
        })?;

        self.parse_parameter(config, 15, |p15| {
            Ok(ConfigParamEnum::ConfigParam15(ConfigParam15 {
                validators_elected_for: p15.get_num32("validators_elected_for")?,
                elections_start_before: p15.get_num32("elections_start_before")?,
                elections_end_before: p15.get_num32("elections_end_before")?,
                stake_held_for: p15.get_num32("stake_held_for")?,
            }))
        })?;

        self.parse_parameter(config, 16, |p16| {
            Ok(ConfigParamEnum::ConfigParam16(ConfigParam16 {
                min_validators: p16.get_num16("min_validators")?.into(),
                max_validators: p16.get_num16("max_validators")?.into(),
                max_main_validators: p16.get_num16("max_main_validators")?.into(),
            }))
        })?;

        self.parse_parameter(config, 17, |p17| {
            Ok(ConfigParamEnum::ConfigParam17(ConfigParam17 {
                min_stake: p17.get_coins("min_stake")?,
                max_stake: p17.get_coins("max_stake")?,
                min_total_stake: p17.get_coins("min_total_stake")?,
                max_stake_factor: p17.get_num32("max_stake_factor")?,
            }))
        })?;

        self.parse_storage_prices(config)?; // p18
        self.parse_parameter(config, 19, |p19| {
            let global_id = p19.get_num32("global_id")?;
            Ok(ConfigParamEnum::ConfigParam19(global_id))
        })?;
        self.parse_gas_limits(config)?; // p20 p21
        self.parse_block_limits(config)?; // p22 p23
        self.parse_msg_forward_prices(config)?; // p24 p25
        self.parse_parameter(config, 28, Self::parse_catchain_config)?;
        self.parse_parameter(config, 29, Self::parse_consensus_config)?;
        self.parse_parameter(config, 30, Self::parse_new_consensus_config_all)?;

        self.parse_array(config, 31, |p31| {
            let mut fundamental_smc_addr = FundamentalSmcAddresses::default();
            p31.iter().try_for_each(|n| fundamental_smc_addr.add_key(&n.as_uint256()?))?;
            Ok(ConfigParamEnum::ConfigParam31(ConfigParam31 { fundamental_smc_addr }))
        })?;

        self.parse_parameter(config, 32, |p| {
            Ok(ConfigParamEnum::ConfigParam32(ConfigParam32 {
                prev_validators: Self::parse_validator_set(p)?,
            }))
        })?;
        self.parse_parameter(config, 33, |p| {
            Ok(ConfigParamEnum::ConfigParam33(ConfigParam33 {
                prev_temp_validators: Self::parse_validator_set(p)?,
            }))
        })?;

        self.parse_parameter(config, 34, |p34| {
            let mut list = vec![];
            p34.get_vec("list").and_then(|p| {
                p.iter().try_for_each::<_, Result<()>>(|p| {
                    let p = PathMap::cont(config, "p34", p)?;

                    list.push(ValidatorDescr::with_params(
                        p.get_str("public_key")?.parse()?,
                        p.get_num64("weight")?,
                        None,
                    ));
                    Ok(())
                })
            })?;
            let cur_validators = ValidatorSet::new(
                p34.get_num32("utime_since")?,
                p34.get_num32("utime_until")?,
                p34.get_num16("main")?,
                list,
            )?;
            Ok(ConfigParamEnum::ConfigParam34(ConfigParam34 { cur_validators }))
        })?;

        self.parse_parameter(config, 35, |p| {
            Ok(ConfigParamEnum::ConfigParam35(ConfigParam35 {
                cur_temp_validators: Self::parse_validator_set(p)?,
            }))
        })?;
        self.parse_parameter(config, 36, |p| {
            Ok(ConfigParamEnum::ConfigParam36(ConfigParam36 {
                next_validators: Self::parse_validator_set(p)?,
            }))
        })?;
        self.parse_parameter(config, 37, |p| {
            Ok(ConfigParamEnum::ConfigParam37(ConfigParam37 {
                next_temp_validators: Self::parse_validator_set(p)?,
            }))
        })?;

        self.parse_array(config, 39, |p39| {
            let mut validator_keys = ValidatorKeys::default();

            p39.iter().try_for_each::<_, Result<()>>(|p| {
                let p = PathMap::cont(config, "p39", p)?;

                let key = p.get_uint256("map_key")?;
                let adnl_addr = p.get_uint256("adnl_addr")?;
                let temp_public_key = hex::decode(p.get_str("temp_public_key")?)?;
                let seqno = p.get_num32("seqno")?;
                let valid_until = p.get_num32("valid_until")?;
                let signature_r = p.get_str("signature_r")?;
                let signature_s = p.get_str("signature_s")?;

                let pk = ValidatorTempKey::with_params(
                    adnl_addr,
                    SigPubKey::from_bytes(&temp_public_key)?,
                    seqno,
                    valid_until,
                );
                let sk = CryptoSignature::from_r_s_str(signature_r, signature_s)?;
                validator_keys
                    .set(&key, &ValidatorSignedTempKey::with_key_and_signature(pk, sk))?;
                Ok(())
            })?;

            Ok(ConfigParamEnum::ConfigParam39(ConfigParam39 { validator_keys }))
        })?;

        self.parse_parameter(config, 40, |p40| {
            Ok(ConfigParamEnum::ConfigParam40(MisbehaviourPunishmentConfig {
                default_flat_fine: p40.get_coins("default_flat_fine")?,
                default_proportional_fine: p40.get_num32("default_proportional_fine")?,
                severity_flat_mult: p40.get_num16("severity_flat_mult")?,
                severity_proportional_mult: p40.get_num16("severity_proportional_mult")?,
                unpunishable_interval: p40.get_num16("unpunishable_interval")?,
                long_interval: p40.get_num16("long_interval")?,
                long_flat_mult: p40.get_num16("long_flat_mult")?,
                long_proportional_mult: p40.get_num16("long_proportional_mult")?,
                medium_interval: p40.get_num16("medium_interval")?,
                medium_flat_mult: p40.get_num16("medium_flat_mult")?,
                medium_proportional_mult: p40.get_num16("medium_proportional_mult")?,
            }))
        })?;

        self.parse_parameter(config, 43, |p43| {
            Ok(ConfigParamEnum::ConfigParam43(SizeLimitsConfig {
                max_msg_bits: p43.get_num32("max_msg_bits")?,
                max_msg_cells: p43.get_num32("max_msg_cells")?,
                max_library_cells: p43.get_num32("max_library_cells")?,
                max_vm_data_depth: p43.get_num16("max_vm_data_depth")?,
                max_ext_msg_size: p43.get_num32("max_ext_msg_size")?,
                max_ext_msg_depth: p43.get_num16("max_ext_msg_depth")?,
                max_acc_state_cells: p43.get_num32("max_acc_state_cells")?,
                max_mc_acc_state_cells: p43.get_num32("max_mc_acc_state_cells")?,
                max_acc_public_libraries: p43.get_num32("max_acc_public_libraries")?,
                defer_out_queue_size_limit: p43.get_num32("defer_out_queue_size_limit")?,
                max_msg_extra_currencies: p43.get_num32("max_msg_extra_currencies")?,
                max_acc_fixed_prefix_length: p43.get_num8("max_acc_fixed_prefix_length")?,
                acc_state_cells_for_storage_dict: p43
                    .get_num32("acc_state_cells_for_storage_dict")?,
            }))
        })?;

        self.parse_parameter(config, 44, |p44| {
            let mut sa = SuspendedAddressList::default();
            let suspended_until = p44.get_num32("suspended_until")?;
            sa.set_suspended_until(suspended_until);
            p44.get_vec("addresses").and_then(|p| {
                for address in p {
                    let address =
                        address.as_str().ok_or_else(|| error!("address must be string"))?;
                    let address = MsgAddressInt::from_str(address)?;
                    sa.add_suspended_address(address.workchain_id(), address.address().clone())?;
                }
                Ok(())
            })?;
            Ok(ConfigParamEnum::ConfigParam44(sa))
        })?;

        self.parse_parameter(config, 45, |p45| {
            let mut pl = PrecompiledContractsList::default();
            let list = p45.get_obj("precompiled_contracts_list")?;
            for (address, gas) in list.iter() {
                let address = address.parse()?;
                let gas = gas.as_ulong()?;
                pl.add(&address, gas)?;
            }
            Ok(ConfigParamEnum::ConfigParam45(pl))
        })?;

        self.parse_parameter(config, 63, Self::parse_accelerated_consensus_config)?;

        self.parse_parameter(config, 71, |p| {
            let p = Self::parse_oracle_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam71(p))
        })?;
        self.parse_parameter(config, 72, |p| {
            let p = Self::parse_oracle_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam72(p))
        })?;
        self.parse_parameter(config, 73, |p| {
            let p = Self::parse_oracle_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam73(p))
        })?;
        self.parse_parameter(config, 79, |p| {
            let p = Self::parse_jetton_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam79(p))
        })?;
        self.parse_parameter(config, 81, |p| {
            let p = Self::parse_jetton_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam81(p))
        })?;
        self.parse_parameter(config, 82, |p| {
            let p = Self::parse_jetton_bridge_params(p)?;
            Ok(ConfigParamEnum::ConfigParam82(p))
        })?;
        Ok(())
    }

    fn parse_state_unchecked(mut self, map: &Map<String, Value>) -> Result<ShardStateUnsplit> {
        let map_path = PathMap::new(map);

        self.state.set_min_ref_mc_seqno(u32::MAX);

        match map_path.get_num("global_id") {
            Ok(global_id) => self.state.set_global_id(global_id as i32),
            Err(err) => {
                if self.mandatory_params != 0 {
                    return Err(err);
                }
            }
        }
        match map_path.get_num32("gen_utime") {
            Ok(gen_utime) => self.state.set_gen_time(gen_utime),
            Err(err) => {
                if self.mandatory_params != 0 {
                    return Err(err);
                }
            }
        }

        match map_path.get_coins("total_balance") {
            Ok(balance) => self.state.set_total_balance(CurrencyCollection::from_coins(balance)),
            Err(err) => {
                if self.mandatory_params != 0 {
                    return Err(err);
                }
            }
        }

        match map_path.get_obj("master") {
            Ok(master) => {
                let config = master.get_obj("config")?;
                self.parse_config(&config)?;
                match master.get_uint256("config_addr") {
                    Ok(addr) => self.extra.config.config_addr = addr.into(),
                    Err(err) => {
                        if self.mandatory_params != 0 {
                            return Err(err);
                        }
                    }
                }
                match master.get_num32("validator_list_hash_short") {
                    Ok(v) => self.extra.validator_info.validator_list_hash_short = v,
                    Err(err) => {
                        if self.mandatory_params != 0 {
                            return Err(err);
                        }
                    }
                }
                match master.get_num32("catchain_seqno") {
                    Ok(v) => self.extra.validator_info.catchain_seqno = v,
                    Err(err) => {
                        if self.mandatory_params != 0 {
                            return Err(err);
                        }
                    }
                }
                match master.get_bool("nx_cc_updated") {
                    Ok(v) => self.extra.validator_info.nx_cc_updated = v,
                    Err(err) => {
                        if self.mandatory_params != 0 {
                            return Err(err);
                        }
                    }
                }
                match master.get_coins("global_balance") {
                    Ok(balance) => self.extra.global_balance.coins = balance,
                    Err(err) => {
                        if self.mandatory_params != 0 {
                            return Err(err);
                        }
                    }
                }
                self.extra.after_key_block = true;
                self.state.write_custom(Some(&self.extra))?;
            }
            Err(err) => {
                if self.mandatory_params != 0 {
                    return Err(err);
                }
            }
        }

        if let Ok(accounts) = map_path.get_vec("accounts") {
            let mut shard_accounts = self.state.read_accounts()?;
            accounts.iter().try_for_each::<_, Result<()>>(|account| {
                let account = PathMap::cont(&map_path, "accounts", account)?;
                let mut account = Account::construct_from_bytes(&account.get_base64("boc")?)?;
                account.update_storage_stat(
                    self.extra.config.size_limits_config()?.acc_state_cells_for_storage_dict,
                )?;
                if let Some(account_id) = account.get_id() {
                    let aug = account.aug()?;
                    let account = ShardAccount::with_params(&account, UInt256::ZERO, 0)?;
                    shard_accounts.set(account_id, &account, &aug)?;
                }
                Ok(())
            })?;
            self.state.write_accounts(&shard_accounts)?;
        }

        if let Ok(libraries) = map_path.get_vec("libraries") {
            libraries.iter().try_for_each::<_, Result<()>>(|library| {
                let library = PathMap::cont(&map_path, "libraries", library)?;
                let id = library.get_uint256("hash")?;
                let lib = library.get_base64("lib")?;
                let lib = read_single_root_boc(lib)?;
                let mut lib = LibDescr::new(lib);
                let publishers = library.get_vec("publishers")?;
                publishers.iter().try_for_each::<_, Result<()>>(|publisher| {
                    lib.publishers_mut().add_key(&publisher.as_uint256()?)
                })?;
                self.state.libraries_mut().set(&id, &lib)?;
                Ok(())
            })?;
        }

        Ok(self.state)
    }
}

pub fn parse_config_with_mandatory_params(
    config: &Map<String, Value>,
    mandatories: &[u32],
) -> Result<ConfigParams> {
    let config = PathMap::new(config);
    let mut parser = StateParser::new();
    if !mandatories.is_empty() {
        parser.mandatory_params = 0;
        for mandatory in mandatories {
            parser.mandatory_params |= 1u64 << mandatory;
        }
    }
    parser.parse_config(&config)?;
    Ok(parser.extra.config)
}

pub fn parse_config(config: &Map<String, Value>) -> Result<ConfigParams> {
    parse_config_with_mandatory_params(config, &[])
}

pub fn parse_state(map: &Map<String, Value>) -> Result<ShardStateUnsplit> {
    StateParser::for_zero_state().parse_state_unchecked(map)
}

pub fn parse_state_unchecked(map: &Map<String, Value>) -> Result<ShardStateUnsplit> {
    StateParser::new().parse_state_unchecked(map)
}

pub fn parse_block_proof(map: &Map<String, Value>, block_file_hash: UInt256) -> Result<BlockProof> {
    let map_path = PathMap::new(map);

    let root = read_single_root_boc(base64_decode(map_path.get_str("proof")?)?)?;

    let merkle_proof = MerkleProof::construct_from_cell(root.clone())?;
    let block_virt_root = merkle_proof.proof.virtualize(1);
    let virt_block = Block::construct_from_cell(block_virt_root.clone())?;
    let block_info = virt_block.read_info()?;

    let proof_for = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(
            block_info.shard().workchain_id(),
            block_info.shard().shard_prefix_with_tag(),
        )?,
        block_info.seq_no(),
        block_virt_root.repr_hash(),
        block_file_hash,
    );

    let signatures = if let Ok(signatures) = map_path.get_vec("signatures") {
        // Parse common signature fields
        let mut pure_signatures = BlockSignaturesPure::new();
        pure_signatures.set_weight(map_path.get_num64("sig_weight")?);
        for signature in signatures {
            let signature = PathMap::cont(&map_path, "signatures", signature)?;
            pure_signatures.add_sigpair(CryptoSignaturePair {
                node_id_short: signature.get_uint256("node_id")?,
                sign: CryptoSignature::from_r_s_str(
                    signature.get_str("r")?,
                    signature.get_str("s")?,
                )?,
            });
        }

        let validator_info = ValidatorBaseInfo::with_params(
            map_path.get_num32("validator_list_hash_short")?,
            map_path.get_num32("catchain_seqno")?,
        );

        // Check signature type - defaults to "ordinary" for backward compatibility
        let signature_type = map_path.get_str("signature_type").unwrap_or("ordinary");
        let variant = if signature_type == "simplex" {
            // Parse Simplex-specific fields
            let session_id = map_path.get_uint256("session_id")?;
            let slot = map_path.get_num32("slot")?;
            let candidate_data = read_single_root_boc(map_path.get_base64("candidate_data")?)?;
            BlockSignaturesVariant::Simplex(BlockSignaturesSimplex::new_finalize(
                validator_info,
                pure_signatures,
                session_id,
                slot,
                candidate_data,
            ))
        } else {
            BlockSignaturesVariant::Ordinary(ton_block::BlockSignatures::with_params(
                validator_info,
                pure_signatures,
            ))
        };
        Some(variant)
    } else {
        None
    };

    Ok(BlockProof::with_params(proof_for, root, signatures))
}

#[cfg(test)]
#[path = "tests/test_deserialize.rs"]
mod tests;
