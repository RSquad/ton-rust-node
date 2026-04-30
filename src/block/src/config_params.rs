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
use crate::{
    define_HashmapE,
    dictionary::hashmapaug::HashmapAugType,
    error, fail,
    shard::ShardIdent,
    shard_accounts::ShardAccounts,
    signature::{CryptoSignature, SigPubKey},
    types::{
        ChildCell, Coins, ExtraCurrencyCollection, Number12, Number13, Number16, Number32, Number8,
    },
    validators::{ValidatorDescr, ValidatorSet},
    AccountId, BlockError, BuilderData, Cell, Deserializable, EmptyValue, HashmapE,
    HashmapIterator, HashmapType, IBitstring, MsgAddressInt, Result, Serializable, SliceData,
    UInt256, BASE_WORKCHAIN_ID, MAX_SPLIT_DEPTH,
};
use num::BigInt;
use std::collections::BTreeMap;

#[cfg(test)]
#[path = "tests/test_config_params.rs"]
mod tests;

/*
1.6.3. Quick access through the header of masterchain blocks
_ config_addr:uint256
config:^(Hashmap 32 ^Cell) = ConfigParams;
*/
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigParams {
    pub config_addr: AccountId,
    pub config_params: HashmapE, // <u32, SliceData>
}

impl Default for ConfigParams {
    fn default() -> ConfigParams {
        Self { config_addr: AccountId::ZERO_ID, config_params: HashmapE::with_bit_len(32) }
    }
}

impl ConfigParams {
    pub fn with_root(data: Cell) -> Result<Self> {
        let config_params = HashmapE::with_hashmap(32, Some(data));
        let cell = config_params
            .get(0u32.write_to_bitstring()?)?
            .ok_or_else(|| error!("config param 0 is missing"))?
            .reference(0)?;
        let result = ConfigParamEnum::construct_from_cell_and_number(cell, 0)?;
        let ConfigParamEnum::ConfigParam0(ConfigParam0 { config_addr }) = result else {
            fail!("config param 0 has invalid format");
        };
        Ok(Self { config_addr, config_params })
    }

    pub const fn with_address_and_params(config_addr: AccountId, data: Option<Cell>) -> Self {
        Self { config_addr, config_params: HashmapE::with_hashmap(32, data) }
    }

    pub fn root(&self) -> Option<&Cell> {
        self.config_params.data()
    }

    pub fn update_param<F>(&mut self, index: u32, update: F) -> Result<()>
    where
        F: FnOnce(&mut Option<ConfigParamEnum>),
    {
        let key = index.write_to_bitstring()?;
        let mut param = None;
        if let Some(slice) = self.config_params.get(key.clone())? {
            if let Some(cell) = slice.reference_opt(0) {
                param = ConfigParamEnum::construct_from_cell_and_number(cell, index).ok();
            }
        }
        update(&mut param);
        if let Some(param) = param {
            let mut value = BuilderData::new();
            param.write_to_cell(&mut value)?;
            self.config_params.set_builder(key, &value)?;
        } else {
            self.config_params.remove(key)?;
        }
        Ok(())
    }

    /// get config by index
    pub fn config(&self, index: u32) -> Result<Option<ConfigParamEnum>> {
        let key = index.write_to_bitstring()?;
        if let Some(slice) = self.config_params.get(key)? {
            if let Some(cell) = slice.reference_opt(0) {
                return Ok(Some(ConfigParamEnum::construct_from_cell_and_number(cell, index)?));
            }
        }
        Ok(None)
    }

    /// get config by index
    pub fn config_present(&self, index: u32) -> Result<bool> {
        let key = index.write_to_bitstring()?;
        if let Some(slice) = self.config_params.get(key)? {
            if slice.remaining_references() != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// load config cell by index
    pub fn config_cell_slice(&self, index: u32) -> Result<SliceData> {
        let key = index.write_to_bitstring()?;
        if let Some(slice) = self.config_params.get(key)? {
            if let Some(cell) = slice.reference_opt(0) {
                return SliceData::load_cell(cell);
            }
        }
        fail!("Cannot load cell for config param {}", index)
    }

    /// set config
    pub fn set_config(&mut self, config: ConfigParamEnum) -> Result<()> {
        let mut value = BuilderData::new();
        let index = config.write_to_cell(&mut value)?;
        let key = index.write_to_bitstring()?;
        self.config_params.set_builder(key, &value)?;
        Ok(())
    }

    pub fn get_smc_tick_tock(
        &self,
        smc_addr: &AccountId,
        accounts: &ShardAccounts,
    ) -> Result<usize> {
        let account = match accounts.get(smc_addr)? {
            Some(shard_account) => shard_account.read_account()?,
            None => fail!("Tick-tock smartcontract not found"),
        };
        Ok(account.get_tick_tock().map(|tick_tock| tick_tock.as_usize()).unwrap_or_default())
    }

    pub fn special_ticktock_smartcontracts(
        &self,
        tick_tock: usize,
        accounts: &ShardAccounts,
    ) -> Result<Vec<(AccountId, usize)>> {
        let mut vec = Vec::new();
        self.fundamental_smc_addr()?.iterate_keys(|key: AccountId| {
            let tt = self.get_smc_tick_tock(&key, accounts)?;
            if (tick_tock & tt) != 0 {
                vec.push((key, tt))
            }
            Ok(true)
        })?;
        Ok(vec)
    }

    //
    // Wrappers
    //
    pub fn config_address(&self) -> Result<AccountId> {
        match self.config(0)? {
            Some(ConfigParamEnum::ConfigParam0(param)) => Ok(param.config_addr),
            _ => fail!("no config smc address in config"),
        }
    }
    pub fn elector_address(&self) -> Result<AccountId> {
        match self.config(1)? {
            Some(ConfigParamEnum::ConfigParam1(param)) => Ok(param.elector_addr),
            _ => fail!("no elector address in config"),
        }
    }
    pub fn minter_address(&self) -> Result<AccountId> {
        let addr = match self.config(2)? {
            Some(ConfigParamEnum::ConfigParam2(param)) => param.minter_addr,
            _ => match self.config(0)? {
                Some(ConfigParamEnum::ConfigParam0(param)) => param.config_addr,
                _ => fail!("no minter address in config"),
            },
        };
        Ok(addr)
    }
    pub fn fee_collector_address(&self) -> Result<AccountId> {
        let addr = match self.config(3)? {
            Some(ConfigParamEnum::ConfigParam3(param)) => param.fee_collector_addr,
            _ => match self.config(1)? {
                Some(ConfigParamEnum::ConfigParam1(param)) => param.elector_addr,
                _ => fail!("no fee collector address in config"),
            },
        };
        Ok(addr)
    }
    // TODO 4 dns_root_addr
    pub fn mint_prices(&self) -> Result<ConfigParam6> {
        match self.config(6)? {
            Some(ConfigParamEnum::ConfigParam6(cp)) => Ok(cp),
            _ => fail!("no config 6 (mint prices)"),
        }
    }
    pub fn to_mint(&self) -> Result<ExtraCurrencyCollection> {
        match self.config(7)? {
            Some(ConfigParamEnum::ConfigParam7(cp)) => Ok(cp.to_mint),
            _ => fail!("no config 7 (to mint)"),
        }
    }
    pub fn get_global_version(&self) -> Result<GlobalVersion> {
        match self.config(8)? {
            Some(ConfigParamEnum::ConfigParam8(gb)) => Ok(gb.global_version),
            _ => fail!("no global version in config"),
        }
    }
    pub fn mandatory_params(&self) -> Result<MandatoryParams> {
        match self.config(9)? {
            Some(ConfigParamEnum::ConfigParam9(mp)) => Ok(mp.mandatory_params),
            _ => fail!("no mandatory params in config"),
        }
    }
    // TODO 11 ConfigVotingSetup
    pub fn workchains(&self) -> Result<Workchains> {
        match self.config(12)? {
            Some(ConfigParamEnum::ConfigParam12(param)) => Ok(param.workchains),
            _ => fail!("Workchains not found in config"),
        }
    }
    pub fn base_workchain(&self) -> Result<WorkchainDescr> {
        self.workchains()?
            .get(&BASE_WORKCHAIN_ID)?
            .ok_or_else(|| error!("No config for base workchain"))
    }
    // TODO 13 compliant pricing
    pub fn block_create_fees(&self, masterchain: bool) -> Result<Coins> {
        match self.config(14)? {
            Some(ConfigParamEnum::ConfigParam14(param)) => {
                if masterchain {
                    Ok(param.block_create_fees.masterchain_block_fee)
                } else {
                    Ok(param.block_create_fees.basechain_block_fee)
                }
            }
            _ => fail!("no block create fee parameter"),
        }
    }
    pub fn elector_params(&self) -> Result<ConfigParam15> {
        match self.config(15)? {
            Some(ConfigParamEnum::ConfigParam15(param)) => Ok(param),
            _ => fail!("no elector params in config"),
        }
    }
    pub fn validators_count(&self) -> Result<ConfigParam16> {
        match self.config(16)? {
            Some(ConfigParamEnum::ConfigParam16(param)) => Ok(param),
            _ => fail!("no elector params in config"),
        }
    }
    pub fn stakes_config(&self) -> Result<ConfigParam17> {
        match self.config(17)? {
            Some(ConfigParamEnum::ConfigParam17(param)) => Ok(param),
            _ => fail!("no stakes params in config"),
        }
    }
    // TODO 16 validators count
    // TODO 17 stakes config
    pub fn storage_prices(&self) -> Result<ConfigParam18> {
        match self.config(18)? {
            Some(ConfigParamEnum::ConfigParam18(param)) => Ok(param),
            _ => fail!("Storage prices not found"),
        }
    }
    pub fn gas_prices(&self, is_masterchain: bool) -> Result<GasLimitsPrices> {
        if is_masterchain {
            if let Some(ConfigParamEnum::ConfigParam20(param)) = self.config(20)? {
                return Ok(param);
            }
        } else if let Some(ConfigParamEnum::ConfigParam21(param)) = self.config(21)? {
            return Ok(param);
        }
        fail!("Gas prices not found")
    }
    pub fn block_limits(&self, masterchain: bool) -> Result<BlockLimits> {
        if masterchain {
            if let Some(ConfigParamEnum::ConfigParam22(param)) = self.config(22)? {
                return Ok(param);
            }
        } else if let Some(ConfigParamEnum::ConfigParam23(param)) = self.config(23)? {
            return Ok(param);
        }
        fail!("BlockLimits not found")
    }
    pub fn fwd_prices(&self, is_masterchain: bool) -> Result<MsgForwardPrices> {
        if is_masterchain {
            if let Some(ConfigParamEnum::ConfigParam24(param)) = self.config(24)? {
                return Ok(param);
            }
        } else if let Some(ConfigParamEnum::ConfigParam25(param)) = self.config(25)? {
            return Ok(param);
        }
        fail!("Forward prices not found")
    }
    pub fn catchain_config(&self) -> Result<CatchainConfig> {
        match self.config(28)? {
            Some(ConfigParamEnum::ConfigParam28(ccc)) => Ok(ccc),
            _ => fail!("no CatchainConfig in config_params"),
        }
    }
    pub fn consensus_config(&self) -> Result<ConsensusConfig> {
        match self.config(29)? {
            Some(ConfigParamEnum::ConfigParam29(consensus_config)) => Ok(consensus_config),
            _ => fail!("no ConsensusConfig in config_params"),
        }
    }
    // TODO 29 consensus config
    pub fn fundamental_smc_addr(&self) -> Result<FundamentalSmcAddresses> {
        match self.config(31)? {
            Some(ConfigParamEnum::ConfigParam31(param)) => Ok(param.fundamental_smc_addr),
            _ => fail!("fundamental_smc_addr not found in config"),
        }
    }
    pub fn prev_validator_set(&self) -> Result<ValidatorSet> {
        #[cfg(feature = "mirrornet")]
        if let Some(ConfigParamEnum::ConfigParam33(param)) = self.config(33)? {
            return Ok(param.prev_temp_validators);
        }
        Ok(match self.config(32)? {
            Some(ConfigParamEnum::ConfigParam32(param)) => param.prev_validators,
            _ => ValidatorSet::default(),
        })
    }
    pub fn prev_validator_set_present(&self) -> Result<bool> {
        Ok(self.config_present(33)? || self.config_present(32)?)
    }
    pub fn validator_set(&self) -> Result<ValidatorSet> {
        #[cfg(feature = "mirrornet")]
        if let Some(ConfigParamEnum::ConfigParam35(param)) = self.config(35)? {
            return Ok(param.cur_temp_validators);
        }
        Ok(match self.config(34)? {
            Some(ConfigParamEnum::ConfigParam34(param)) => param.cur_validators,
            _ => ValidatorSet::default(),
        })
    }
    pub fn next_validator_set(&self) -> Result<ValidatorSet> {
        #[cfg(feature = "mirrornet")]
        if let Some(ConfigParamEnum::ConfigParam37(param)) = self.config(37)? {
            return Ok(param.next_temp_validators);
        }
        Ok(match self.config(36)? {
            Some(ConfigParamEnum::ConfigParam36(param)) => param.next_validators,
            _ => ValidatorSet::default(),
        })
    }
    pub fn next_validator_set_present(&self) -> Result<bool> {
        Ok(self.config_present(37)? || self.config_present(36)?)
    }
    pub fn read_cur_validator_set_and_cc_conf(&self) -> Result<(ValidatorSet, CatchainConfig)> {
        Ok((self.validator_set()?, self.catchain_config()?))
    }
    pub fn size_limits_config(&self) -> Result<SizeLimitsConfig> {
        match self.config(43)? {
            Some(ConfigParamEnum::ConfigParam43(s)) => Ok(s),
            _ => Ok(Default::default()),
        }
    }
    pub fn suspended_address_list(&self) -> Result<Option<SuspendedAddressList>> {
        match self.config(44)? {
            Some(ConfigParamEnum::ConfigParam44(sa)) => Ok(Some(sa)),
            _ => Ok(None),
        }
    }
    pub fn precompiled_contracts_list(&self) -> Result<Option<PrecompiledContractsList>> {
        match self.config(45)? {
            Some(ConfigParamEnum::ConfigParam45(pl)) => Ok(Some(pl)),
            _ => Ok(None),
        }
    }
    pub fn accelerated_consensus_params(&self) -> Result<AcceleratedConsensusConfig> {
        match self.config(63)? {
            Some(ConfigParamEnum::ConfigParam63(params)) => Ok(params),
            _ => fail!("no AcceleratedConsensusConfig in config_params"),
        }
    }

    /// Get simplex config for masterchain from ConfigParam 30.
    /// Returns None if config is missing or is null_consensus_config (use catchain).
    pub fn get_mc_simplex_config(&self) -> Result<Option<SimplexConfig>> {
        match self.config(30)? {
            Some(ConfigParamEnum::ConfigParam30(cfg)) => Ok(cfg.mc),
            _ => Ok(None),
        }
    }

    /// Get simplex config for shards (basechain) from ConfigParam 30.
    /// Returns None if config is missing or is null_consensus_config (use catchain).
    pub fn get_shard_simplex_config(&self) -> Result<Option<SimplexConfig>> {
        match self.config(30)? {
            Some(ConfigParamEnum::ConfigParam30(cfg)) => Ok(cfg.shard),
            _ => Ok(None),
        }
    }

    pub fn serialize_single_param(&self) -> Result<(Cell, u32)> {
        let Some((key, slice)) = self.config_params.is_single()? else {
            fail!("ConfigParams contain more than a single parameter");
        };
        let index = SliceData::load_bitstring(key)?.get_int(32)? as u32;
        let cell = slice.reference(0)?;
        Ok((cell, index))
    }
    // TODO 39 validator signed temp keys
}

#[rustfmt::skip]
#[derive(Clone, Copy, Debug)]
#[repr(u64)]
pub enum GlobalCapabilities {
    CapNone                   = 0,
    CapIhrEnabled             = 0x0000_0000_0001,
    CapCreateStatsEnabled     = 0x0000_0000_0002,
    CapBounceMsgBody          = 0x0000_0000_0004,
    CapReportVersion          = 0x0000_0000_0008,
    CapSplitMergeTransactions = 0x0000_0000_0010,
    CapShortDequeue           = 0x0000_0000_0020,
    CapStoreOutMsgQueueSize   = 0x0000_0000_0040,
    CapMsgMetadata            = 0x0000_0000_0080,
    CapDeferMessages          = 0x0000_0000_0100,
    CapFullCollatedData       = 0x0000_0000_0200,
    CapResolveMerkleCell      = 0x0000_0200_0000,
}

pub const SUPPORTED_VERSION: u32 = 13;
pub const LT_ALIGN: u64 = 1_000_000;

impl ConfigParams {
    pub fn get_lt_align(&self) -> u64 {
        LT_ALIGN
    }
    #[cfg(not(feature = "xp25"))]
    pub fn get_max_lt_growth(&self) -> u64 {
        10 * self.get_lt_align() - 1
    }
    #[cfg(feature = "xp25")]
    pub fn get_max_lt_growth(&self) -> u64 {
        1000 * self.get_lt_align() - 1
    }
    pub fn get_next_block_lt(&self, prev_block_lt: u64) -> u64 {
        (prev_block_lt / self.get_lt_align() + 1) * self.get_lt_align()
    }
    pub fn has_capabilities(&self) -> bool {
        self.get_global_version().is_ok_and(|gb| gb.capabilities != 0)
    }
    pub fn has_capability(&self, capability: GlobalCapabilities) -> bool {
        self.get_global_version().is_ok_and(|gb| gb.has_capability(capability))
    }
    pub fn capabilities(&self) -> u64 {
        self.get_global_version().map_or(0, |gb| gb.capabilities)
    }
    pub fn global_version(&self) -> u32 {
        self.get_global_version().map_or(0, |gb| gb.version)
    }
}

impl ConfigParams {
    pub fn compute_validator_set_cc(
        &self,
        shard: &ShardIdent,
        cc_seqno: u32,
        cc_seqno_delta: &mut u32,
    ) -> Result<Vec<ValidatorDescr>> {
        let (vset, ccc) = self.read_cur_validator_set_and_cc_conf()?;
        if (*cc_seqno_delta & 0xfffffffe) != 0 {
            fail!("seqno_delta>1 is not implemented yet");
        }
        *cc_seqno_delta += cc_seqno;
        vset.calc_subset(&ccc, shard.shard_prefix_with_tag(), shard.workchain_id(), *cc_seqno_delta)
            .map(|(set, _hash)| set)
    }
    pub fn compute_validator_set(
        &self,
        shard: &ShardIdent,
        _at: u32,
        cc_seqno: u32,
    ) -> Result<Vec<ValidatorDescr>> {
        let (vset, ccc) = self.read_cur_validator_set_and_cc_conf()?;
        vset.calc_subset(&ccc, shard.shard_prefix_with_tag(), shard.workchain_id(), cc_seqno)
            .map(|(set, _seq_no)| set)
    }
}

const MANDATORY_CONFIG_PARAMS: [u32; 9] = [18, 20, 21, 22, 23, 24, 25, 28, 34];

impl ConfigParams {
    pub fn valid_config_data(
        &self,
        relax_par0: bool,
        mparams: Option<MandatoryParams>,
    ) -> Result<bool> {
        if !relax_par0 {
            match self.config(0) {
                Ok(Some(ConfigParamEnum::ConfigParam0(param))) => {
                    if param.config_addr != self.config_addr {
                        log::warn!(
                            target: "block",
                            "config address is not set in config parameters root {}",
                            param.config_addr
                        );
                        return Ok(false);
                    }
                }
                _ => return Ok(false),
            }
        }
        // porting from Durov's code
        // previously was not 9 parameter in config params
        for index in &MANDATORY_CONFIG_PARAMS {
            if self.config(*index)?.is_none() {
                log::error!(target: "block", "configuration parameter #{} \
                    (hardcoded as mandatory) is missing)", index);
                return Ok(false);
            }
        }
        let result = match self.config(9) {
            Ok(Some(ConfigParamEnum::ConfigParam9(param))) => {
                self.config_params_present(Some(param.mandatory_params))?
            }
            _ => {
                log::error!(target: "block", "invalid mandatory parameters dictionary while checking \
                    existence of all mandatory configuration parameters");
                false
            }
        };
        Ok(result && self.config_params_present(mparams)?)
    }
    fn config_params_present(&self, params: Option<MandatoryParams>) -> Result<bool> {
        match params {
            Some(params) => params.iterate_keys(|index: u32| match self.config(index) {
                Ok(Some(_)) => Ok(true),
                _ => {
                    log::error!(target: "block", "configuration parameter #{} \
                        (declared as mandatory in configuration parameter #9) is missing)", index);
                    Ok(false)
                }
            }),
            None => Ok(true),
        }
    }
    // when these parameters change, the block must be marked as a key block
    pub fn important_config_parameters_changed(
        &self,
        other: &ConfigParams,
        coarse: bool,
    ) -> Result<bool> {
        if self.config_params == other.config_params {
            return Ok(false);
        }
        if coarse {
            return Ok(true);
        }
        // for now, all parameters are "important"
        // at least the parameters affecting the computations of validator sets must be considered important
        // ...
        Ok(true)
    }
}

impl Deserializable for ConfigParams {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let config_addr = slice.get_next_slice(256)?;
        let data = slice.checked_drain_reference()?;
        let config_params = HashmapE::with_hashmap(32, Some(data));
        Ok(Self { config_addr, config_params })
    }
}

impl Serializable for ConfigParams {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.checked_append_reference(self.config_params.data().cloned().unwrap_or_default())?;
        self.config_addr.write_to(cell)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigParamEnum {
    ConfigParam0(ConfigParam0),
    ConfigParam1(ConfigParam1),
    ConfigParam2(ConfigParam2),
    ConfigParam3(ConfigParam3),
    ConfigParam4(ConfigParam4),
    ConfigParam5(BurningConfig),
    ConfigParam6(ConfigParam6),
    ConfigParam7(ConfigParam7),
    ConfigParam8(ConfigParam8),
    ConfigParam9(ConfigParam9),
    ConfigParam10(ConfigParam10),
    ConfigParam11(ConfigParam11),
    ConfigParam12(ConfigParam12),
    ConfigParam13(ConfigParam13),
    ConfigParam14(ConfigParam14),
    ConfigParam15(ConfigParam15),
    ConfigParam16(ConfigParam16),
    ConfigParam17(ConfigParam17),
    ConfigParam18(ConfigParam18),
    ConfigParam19(GlobalId),
    ConfigParam20(GasLimitsPrices),
    ConfigParam21(GasLimitsPrices),
    ConfigParam22(ConfigParam22),
    ConfigParam23(ConfigParam23),
    ConfigParam24(MsgForwardPrices),
    ConfigParam25(MsgForwardPrices),
    ConfigParam28(CatchainConfig),
    ConfigParam29(ConsensusConfig),
    ConfigParam30(NewConsensusConfigAll),
    ConfigParam31(ConfigParam31),
    ConfigParam32(ConfigParam32),
    ConfigParam33(ConfigParam33),
    ConfigParam34(ConfigParam34),
    ConfigParam35(ConfigParam35),
    ConfigParam36(ConfigParam36),
    ConfigParam37(ConfigParam37),
    ConfigParam39(ConfigParam39),
    ConfigParam40(MisbehaviourPunishmentConfig),
    ConfigParam43(SizeLimitsConfig),
    ConfigParam44(SuspendedAddressList),
    ConfigParam45(PrecompiledContractsList),
    ConfigParam63(AcceleratedConsensusConfig),
    ConfigParam71(OracleBridgeParams), // Ethereum bridge
    ConfigParam72(OracleBridgeParams), // Binance Smart Chain bridge
    ConfigParam73(OracleBridgeParams), // Polygon bridge
    ConfigParam79(JettonBridgeParams), // ETH->TON token bridge
    ConfigParam81(JettonBridgeParams), // BNB->TON token bridge
    ConfigParam82(JettonBridgeParams), // Polygon->TON token bridge
    ConfigParamAny(u32, Cell),
}

macro_rules! read_config {
    ( $cpname:ident, $cname:ident, $cell:expr ) => {{
        let c = $cname::construct_from_cell($cell)?;
        Ok(ConfigParamEnum::$cpname(c))
    }};
}

impl ConfigParamEnum {
    /// read config from cell
    #[rustfmt::skip]
    pub fn construct_from_cell_and_number(
        cell: Cell,
        index: u32
    ) -> Result<ConfigParamEnum> {
        match index {
            0  => read_config!(ConfigParam0,  ConfigParam0,                 cell),
            1  => read_config!(ConfigParam1,  ConfigParam1,                 cell),
            2  => read_config!(ConfigParam2,  ConfigParam2,                 cell),
            3  => read_config!(ConfigParam3,  ConfigParam3,                 cell),
            4  => read_config!(ConfigParam4,  ConfigParam4,                 cell),
            5  => read_config!(ConfigParam5,  BurningConfig,                cell),
            6  => read_config!(ConfigParam6,  ConfigParam6,                 cell),
            7  => read_config!(ConfigParam7,  ConfigParam7,                 cell),
            8  => read_config!(ConfigParam8,  ConfigParam8,                 cell),
            9  => read_config!(ConfigParam9,  ConfigParam9,                 cell),
            10 => read_config!(ConfigParam10, ConfigParam10,                cell),
            11 => read_config!(ConfigParam11, ConfigParam11,                cell),
            12 => read_config!(ConfigParam12, ConfigParam12,                cell),
            13 => read_config!(ConfigParam13, ConfigParam13,                cell),
            14 => read_config!(ConfigParam14, ConfigParam14,                cell),
            15 => read_config!(ConfigParam15, ConfigParam15,                cell),
            16 => read_config!(ConfigParam16, ConfigParam16,                cell),
            17 => read_config!(ConfigParam17, ConfigParam17,                cell),
            18 => read_config!(ConfigParam18, ConfigParam18,                cell),
            19 => read_config!(ConfigParam19, GlobalId,                     cell),
            20 => read_config!(ConfigParam20, GasLimitsPrices,              cell),
            21 => read_config!(ConfigParam21, GasLimitsPrices,              cell),
            22 => read_config!(ConfigParam22, ConfigParam22,                cell),
            23 => read_config!(ConfigParam23, ConfigParam23,                cell),
            24 => read_config!(ConfigParam24, MsgForwardPrices,             cell),
            25 => read_config!(ConfigParam25, MsgForwardPrices,             cell),
            28 => read_config!(ConfigParam28, CatchainConfig,               cell),
            29 => read_config!(ConfigParam29, ConsensusConfig,              cell),
            30 => read_config!(ConfigParam30, NewConsensusConfigAll,        cell),
            31 => read_config!(ConfigParam31, ConfigParam31,                cell),
            32 => read_config!(ConfigParam32, ConfigParam32,                cell),
            33 => read_config!(ConfigParam33, ConfigParam33,                cell),
            34 => read_config!(ConfigParam34, ConfigParam34,                cell),
            35 => read_config!(ConfigParam35, ConfigParam35,                cell),
            36 => read_config!(ConfigParam36, ConfigParam36,                cell),
            37 => read_config!(ConfigParam37, ConfigParam37,                cell),
            39 => read_config!(ConfigParam39, ConfigParam39,                cell),
            40 => read_config!(ConfigParam40, MisbehaviourPunishmentConfig, cell),
            43 => read_config!(ConfigParam43, SizeLimitsConfig,             cell),
            44 => read_config!(ConfigParam44, SuspendedAddressList,         cell),
            45 => read_config!(ConfigParam45, PrecompiledContractsList,     cell),
            63 => read_config!(ConfigParam63, AcceleratedConsensusConfig,   cell),
            71 => read_config!(ConfigParam71, OracleBridgeParams,           cell),
            72 => read_config!(ConfigParam72, OracleBridgeParams,           cell),
            73 => read_config!(ConfigParam73, OracleBridgeParams,           cell),
            79 => read_config!(ConfigParam79, JettonBridgeParams,           cell),
            81 => read_config!(ConfigParam81, JettonBridgeParams,           cell),
            82 => read_config!(ConfigParam82, JettonBridgeParams,           cell),
            index => Ok(ConfigParamEnum::ConfigParamAny(index, cell)),
        }
    }

    /// Save config to cell
    #[rustfmt::skip]
    pub fn write_to_cell(&self, builder: &mut BuilderData) -> Result<u32> {
        #[inline]
        fn serialize<T: Serializable>(cell: &mut BuilderData, c: &T, ret: u32) -> Result<u32> {
            cell.checked_append_reference(c.serialize()?)?;
            Ok(ret)
        }
        match self {
            ConfigParamEnum::ConfigParam0 (ref c) => serialize(builder, c,  0),
            ConfigParamEnum::ConfigParam1 (ref c) => serialize(builder, c,  1),
            ConfigParamEnum::ConfigParam2 (ref c) => serialize(builder, c,  2),
            ConfigParamEnum::ConfigParam3 (ref c) => serialize(builder, c,  3),
            ConfigParamEnum::ConfigParam4 (ref c) => serialize(builder, c,  4),
            ConfigParamEnum::ConfigParam5 (ref c) => serialize(builder, c,  5),
            ConfigParamEnum::ConfigParam6 (ref c) => serialize(builder, c,  6),
            ConfigParamEnum::ConfigParam7 (ref c) => serialize(builder, c,  7),
            ConfigParamEnum::ConfigParam8 (ref c) => serialize(builder, c,  8),
            ConfigParamEnum::ConfigParam9 (ref c) => serialize(builder, c,  9),
            ConfigParamEnum::ConfigParam10(ref c) => serialize(builder, c, 10),
            ConfigParamEnum::ConfigParam11(ref c) => serialize(builder, c, 11),
            ConfigParamEnum::ConfigParam12(ref c) => serialize(builder, c, 12),
            ConfigParamEnum::ConfigParam13(ref c) => serialize(builder, c, 13),
            ConfigParamEnum::ConfigParam14(ref c) => serialize(builder, c, 14),
            ConfigParamEnum::ConfigParam15(ref c) => serialize(builder, c, 15),
            ConfigParamEnum::ConfigParam16(ref c) => serialize(builder, c, 16),
            ConfigParamEnum::ConfigParam17(ref c) => serialize(builder, c, 17),
            ConfigParamEnum::ConfigParam18(ref c) => serialize(builder, c, 18),
            ConfigParamEnum::ConfigParam19(ref c) => serialize(builder, c, 19),
            ConfigParamEnum::ConfigParam20(ref c) => serialize(builder, c, 20),
            ConfigParamEnum::ConfigParam21(ref c) => serialize(builder, c, 21),
            ConfigParamEnum::ConfigParam22(ref c) => serialize(builder, c, 22),
            ConfigParamEnum::ConfigParam23(ref c) => serialize(builder, c, 23),
            ConfigParamEnum::ConfigParam24(ref c) => serialize(builder, c, 24),
            ConfigParamEnum::ConfigParam25(ref c) => serialize(builder, c, 25),
            ConfigParamEnum::ConfigParam28(ref c) => serialize(builder, c, 28),
            ConfigParamEnum::ConfigParam29(ref c) => serialize(builder, c, 29),
            ConfigParamEnum::ConfigParam30(ref c) => serialize(builder, c, 30),
            ConfigParamEnum::ConfigParam31(ref c) => serialize(builder, c, 31),
            ConfigParamEnum::ConfigParam32(ref c) => serialize(builder, c, 32),
            ConfigParamEnum::ConfigParam33(ref c) => serialize(builder, c, 33),
            ConfigParamEnum::ConfigParam34(ref c) => serialize(builder, c, 34),
            ConfigParamEnum::ConfigParam35(ref c) => serialize(builder, c, 35),
            ConfigParamEnum::ConfigParam36(ref c) => serialize(builder, c, 36),
            ConfigParamEnum::ConfigParam37(ref c) => serialize(builder, c, 37),
            ConfigParamEnum::ConfigParam39(ref c) => serialize(builder, c, 39),
            ConfigParamEnum::ConfigParam40(ref c) => serialize(builder, c, 40),
            ConfigParamEnum::ConfigParam43(ref c) => serialize(builder, c, 43),
            ConfigParamEnum::ConfigParam44(ref c) => serialize(builder, c, 44),
            ConfigParamEnum::ConfigParam45(ref c) => serialize(builder, c, 45),
            ConfigParamEnum::ConfigParam63(ref c) => serialize(builder, c, 63),
            ConfigParamEnum::ConfigParam71(ref c) => serialize(builder, c, 71),
            ConfigParamEnum::ConfigParam72(ref c) => serialize(builder, c, 72),
            ConfigParamEnum::ConfigParam73(ref c) => serialize(builder, c, 73),
            ConfigParamEnum::ConfigParam79(ref c) => serialize(builder, c, 79),
            ConfigParamEnum::ConfigParam81(ref c) => serialize(builder, c, 81),
            ConfigParamEnum::ConfigParam82(ref c) => serialize(builder, c, 82),
            ConfigParamEnum::ConfigParamAny(index, cell) => {
                builder.checked_append_reference(cell.clone())?;
                Ok(*index)
            }
        }
    }
}

/*
_ config_addr:bits256 = ConfigParam 0;
*/

///
/// Config Param 0 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam0 {
    pub config_addr: AccountId,
}

impl ConfigParam0 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam0 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.config_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam0 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.config_addr.write_to(cell)?;
        Ok(())
    }
}

/*
_ elector_addr:bits256 = ConfigParam 1;
*/

///
/// Config Param 1 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam1 {
    pub elector_addr: AccountId,
}

impl ConfigParam1 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam1 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.elector_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam1 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.elector_addr.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 2 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam2 {
    pub minter_addr: AccountId,
}

impl ConfigParam2 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam2 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.minter_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam2 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.minter_addr.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 3 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam3 {
    pub fee_collector_addr: AccountId,
}

impl ConfigParam3 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam3 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.fee_collector_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam3 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.fee_collector_addr.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 4 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam4 {
    pub dns_root_addr: AccountId,
}

impl ConfigParam4 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam4 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.dns_root_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam4 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.dns_root_addr.write_to(cell)?;
        Ok(())
    }
}

// burning_config#01
//   blackhole_addr:(Maybe bits256)
//   fee_burn_num:# fee_burn_denom:# { fee_burn_num <= fee_burn_denom } { fee_burn_denom >= 1 } = BurningConfig;
// _ BurningConfig = ConfigParam 5;
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct BurningConfig {
    pub blackhole_addr: Option<AccountId>,
    pub fee_burn_num: u32,
    pub fee_burn_denom: u32,
}

impl BurningConfig {
    pub fn check_validity(&self) -> Result<()> {
        if self.fee_burn_denom == 0 {
            fail!("fee_burn_denom cannot be zero");
        }
        if self.fee_burn_num > self.fee_burn_denom {
            fail!("fee_burn_num cannot be greater than fee_burn_denom");
        }
        Ok(())
    }

    pub fn calculate_burned_fees(&self, value: u128) -> Result<Coins> {
        if self.fee_burn_num == 0 || value == 0 {
            return Ok(Coins::default());
        }
        (value * self.fee_burn_num as u128 / self.fee_burn_denom as u128).try_into()
    }
}

impl Deserializable for BurningConfig {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != 0x01 {
            fail!(Self::invalid_tag(tag as u32));
        }
        let blackhole_addr = Deserializable::construct_from(slice)?;
        let fee_burn_num = Deserializable::construct_from(slice)?;
        let fee_burn_denom = Deserializable::construct_from(slice)?;
        let p5 = BurningConfig { blackhole_addr, fee_burn_num, fee_burn_denom };
        p5.check_validity()?;
        Ok(p5)
    }
}

impl Serializable for BurningConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        0x01u8.write_to(cell)?;
        self.blackhole_addr.write_to(cell)?;
        self.fee_burn_num.write_to(cell)?;
        self.fee_burn_denom.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 6 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam6 {
    pub mint_new_price: Coins,
    pub mint_add_price: Coins,
}

impl ConfigParam6 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam6 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.mint_new_price.read_from(cell)?;
        self.mint_add_price.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam6 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.mint_new_price.write_to(cell)?;
        self.mint_add_price.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 7 structure
///
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct ConfigParam7 {
    pub to_mint: ExtraCurrencyCollection,
}

impl ConfigParam7 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam7 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.to_mint.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam7 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.to_mint.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 8 structure
///
// capabilities#c4 version:uint32 capabilities:uint64 = GlobalVersion;
// _ GlobalVersion = ConfigParam 8;  // all zero if absent

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct GlobalVersion {
    pub version: u32,
    pub capabilities: u64,
}

impl GlobalVersion {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn has_capability(&self, capability: GlobalCapabilities) -> bool {
        (self.capabilities & (capability as u64)) != 0
    }
}

const GLOBAL_VERSION_TAG: u8 = 0xC4;

impl Deserializable for GlobalVersion {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != GLOBAL_VERSION_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.version.read_from(cell)?;
        self.capabilities.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for GlobalVersion {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(GLOBAL_VERSION_TAG)?;
        self.version.write_to(cell)?;
        self.capabilities.write_to(cell)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam8 {
    pub global_version: GlobalVersion,
}

impl ConfigParam8 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam8 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.global_version.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam8 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.global_version.write_to(cell)?;
        Ok(())
    }
}

// _ mandatory_params:(Hashmap 32 True) = ConfigParam 9;

define_HashmapE! {MandatoryParams, 32, EmptyValue}

///
/// Config Param 9 structure
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam9 {
    pub mandatory_params: MandatoryParams,
}

impl ConfigParam9 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam9 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.mandatory_params.read_hashmap_root(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam9 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.mandatory_params.write_hashmap_root(cell)?;
        Ok(())
    }
}

///
/// Config Param 10 structure
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam10 {
    pub critical_params: MandatoryParams,
}

impl ConfigParam10 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam10 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.critical_params.read_hashmap_root(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam10 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.critical_params.write_hashmap_root(cell)?;
        Ok(())
    }
}

///
/// Config Param 14 structure
///
// block_coins_created#6b masterchain_block_fee:Coins basechain_block_fee:Coins
//   = BlockCreateFees;
// _ BlockCreateFees = ConfigParam 14;

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct BlockCreateFees {
    pub masterchain_block_fee: Coins,
    pub basechain_block_fee: Coins,
}

impl BlockCreateFees {
    pub fn new() -> Self {
        Self::default()
    }
}

const BLOCK_CREATE_FEES: u8 = 0x6b;

impl Deserializable for BlockCreateFees {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != BLOCK_CREATE_FEES {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.masterchain_block_fee.read_from(cell)?;
        self.basechain_block_fee.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for BlockCreateFees {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(BLOCK_CREATE_FEES)?;
        self.masterchain_block_fee.write_to(cell)?;
        self.basechain_block_fee.write_to(cell)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam14 {
    pub block_create_fees: BlockCreateFees,
}

impl ConfigParam14 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam14 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.block_create_fees.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam14 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.block_create_fees.write_to(cell)?;
        Ok(())
    }
}

///
/// Config Param 15 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam15 {
    pub validators_elected_for: u32,
    pub elections_start_before: u32,
    pub elections_end_before: u32,
    pub stake_held_for: u32,
}

impl ConfigParam15 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam15 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.validators_elected_for.read_from(cell)?;
        self.elections_start_before.read_from(cell)?;
        self.elections_end_before.read_from(cell)?;
        self.stake_held_for.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam15 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.validators_elected_for.write_to(cell)?;
        self.elections_start_before.write_to(cell)?;
        self.elections_end_before.write_to(cell)?;
        self.stake_held_for.write_to(cell)?;
        Ok(())
    }
}

/*
_ max_validators:(## 16) max_main_validators:(## 16) min_validators:(## 16)
  { max_validators >= max_main_validators }
  { max_main_validators >= min_validators }
  { min_validators >= 1 }
  = ConfigParam 16;
*/

///
/// Config Param 16 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam16 {
    pub max_validators: Number16,
    pub max_main_validators: Number16,
    pub min_validators: Number16,
}

impl ConfigParam16 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam16 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.max_validators.read_from(cell)?;
        self.max_main_validators.read_from(cell)?;
        self.min_validators.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam16 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.max_validators.write_to(cell)?;
        self.max_main_validators.write_to(cell)?;
        self.min_validators.write_to(cell)?;
        Ok(())
    }
}

/*
_
    min_stake: Coins
    max_stake: Coins
    min_total_stake: Coins
    max_stake_factor: uint32
= ConfigParam 17;
*/

///
/// Config Param 17 structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConfigParam17 {
    pub min_stake: Coins,
    pub max_stake: Coins,
    pub min_total_stake: Coins,
    pub max_stake_factor: u32,
}

impl ConfigParam17 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deserializable for ConfigParam17 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.min_stake.read_from(cell)?;
        self.max_stake.read_from(cell)?;
        self.min_total_stake.read_from(cell)?;
        self.max_stake_factor.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam17 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.min_stake.write_to(cell)?;
        self.max_stake.write_to(cell)?;
        self.min_total_stake.write_to(cell)?;
        self.max_stake_factor.write_to(cell)?;
        Ok(())
    }
}

/*
_#cc
    utime_since:uint32
    bit_price_ps:uint64
    cell_price_ps:uint64
    mc_bit_price_ps:uint64
    mc_cell_price_ps:uint64
= StoragePrices;

_ (Hashmap 32 StoragePrices) = ConfigParam 18;
*/

///
/// StoragePrices structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct StoragePrices {
    pub utime_since: u32,
    pub bit_price_ps: u64,
    pub cell_price_ps: u64,
    pub mc_bit_price_ps: u64,
    pub mc_cell_price_ps: u64,
}

impl StoragePrices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calc_storage_fee(
        &self,
        cells: u64,
        bits: u64,
        delta: u64,
        is_masterchain: bool,
    ) -> BigInt {
        let bit_price = if is_masterchain { self.mc_bit_price_ps } else { self.bit_price_ps };
        let cell_price = if is_masterchain { self.mc_cell_price_ps } else { self.cell_price_ps };
        (BigInt::from(delta)
            * (bits as u128 * bit_price as u128 + cells as u128 * cell_price as u128)
            + 0xffff)
            >> 16u8
    }
}

const STORAGE_PRICES_TAG: u8 = 0xCC;

impl Deserializable for StoragePrices {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != STORAGE_PRICES_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.utime_since.read_from(cell)?;
        self.bit_price_ps.read_from(cell)?;
        self.cell_price_ps.read_from(cell)?;
        self.mc_bit_price_ps.read_from(cell)?;
        self.mc_cell_price_ps.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for StoragePrices {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(STORAGE_PRICES_TAG)?;
        self.utime_since.write_to(cell)?;
        self.bit_price_ps.write_to(cell)?;
        self.cell_price_ps.write_to(cell)?;
        self.mc_bit_price_ps.write_to(cell)?;
        self.mc_cell_price_ps.write_to(cell)?;
        Ok(())
    }
}

define_HashmapE!(ConfigParam18Map, 32, StoragePrices);

///
/// ConfigParam 18 struct
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam18 {
    pub map: ConfigParam18Map,
}

impl ConfigParam18 {
    /// determine is empty
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// get value by index
    pub fn get(&self, index: u32) -> Result<StoragePrices> {
        self.map.get(&index)?.ok_or_else(|| error!(BlockError::InvalidIndex(index as usize)))
    }

    /// get all value as vector
    pub fn prices(&self) -> Result<Vec<StoragePrices>> {
        self.map.export_vector()
    }

    /// insert value
    pub fn insert(&mut self, sp: &StoragePrices) -> Result<()> {
        let index = match self.map.0.get_max(false, &mut 0)? {
            Some((key, _value)) => u32::construct_from_bitstring(key)? + 1,
            None => 0,
        };
        self.map.set(&index, sp)
    }
}

impl Deserializable for ConfigParam18 {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.map.read_hashmap_root(slice)?;
        Ok(())
    }
}

impl Serializable for ConfigParam18 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.map.is_empty() {
            fail!(BlockError::InvalidOperation("self.map is empty".to_string()))
        }
        self.map.write_hashmap_root(cell)?;
        Ok(())
    }
}

pub type GlobalId = u32;

/*
gas_prices#dd
    gas_price:uint64
    gas_limit:uint64
    gas_credit:uint64
    block_gas_limit:uint64
    freeze_due_limit:uint64
    delete_due_limit:uint64
= GasLimitsPrices;

gas_prices_ext#de
  gas_price:uint64
  gas_limit:uint64
  special_gas_limit:uint64
  gas_credit:uint64
  block_gas_limit:uint64
  freeze_due_limit:uint64
  delete_due_limit:uint64
  = GasLimitsPrices;

gas_flat_pfx#d1
  flat_gas_limit:uint64
  flat_gas_price:uint64
  other:GasLimitsPrices
= GasLimitsPrices;
*/

///
/// GasLimitsPrices
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GasLimitsPrices {
    pub gas_price: u64,
    pub gas_limit: u64,
    pub special_gas_limit: u64,
    pub gas_credit: u64,
    pub block_gas_limit: u64,
    pub freeze_due_limit: u64,
    pub delete_due_limit: u64,
    pub flat_gas_limit: u64,
    pub flat_gas_price: u64,
    pub max_gas_threshold: u128,
}

impl GasLimitsPrices {
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate gas fee by gas used value
    pub fn calc_gas_fee(&self, gas_used: u64) -> u128 {
        // There is a flat_gas_limit value which is the minimum gas value possible and has fixed price.
        // If actual gas value is less then flat_gas_limit then flat_gas_price paid.
        // If actual gas value is bigger then flat_gas_limit then flat_gas_price paid for first
        // flat_gas_limit gas and remaining value costs gas_price
        if gas_used <= self.flat_gas_limit {
            self.flat_gas_price as u128
        } else {
            // gas_price is pseudo value (shifted by 16 as forward and storage price)
            // after calculation divide by 0xffff with ceil rounding
            self.flat_gas_price as u128
                + (((gas_used - self.flat_gas_limit) as u128 * self.gas_price as u128 + 0xffff)
                    >> 16)
        }
    }

    /// Calculate gas fee by gas used value without flat gas price
    pub fn calc_gas_fee_simple(&self, gas_used: u64) -> u128 {
        (gas_used as u128 * self.gas_price as u128 + 0xffff) >> 16
    }

    /// Get gas price in nanocoins
    pub fn get_real_gas_price(&self) -> u64 {
        self.gas_price >> 16
    }

    /// Calculate gas by coins balance
    pub fn calc_gas(&self, value: u128, gas_limit: u64, max_gas_threshold: u128) -> u64 {
        if value >= max_gas_threshold {
            return gas_limit;
        }
        if value < self.flat_gas_price as u128 {
            return 0;
        }
        let res = ((value - self.flat_gas_price as u128) << 16) / (self.gas_price as u128);
        (self.flat_gas_limit + res as u64).min(crate::VarUInteger7::MAX.as_u64())
    }

    /// Calculate max gas threshold
    pub fn calc_max_gas_threshold(&self, gas_limit: u64) -> u128 {
        let mut result = self.flat_gas_price as u128;
        if gas_limit > self.flat_gas_limit {
            result +=
                ((self.gas_price as u128) * ((gas_limit - self.flat_gas_limit) as u128)) >> 16;
        }
        result
    }
}

const GAS_PRICES_TAG: u8 = 0xDD;
const GAS_PRICES_EXT_TAG: u8 = 0xDE;
const GAS_FLAT_PFX_TAG: u8 = 0xD1;

impl Deserializable for GasLimitsPrices {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.flat_gas_limit = 0;
        self.flat_gas_price = 0;
        self.special_gas_limit = 0;
        loop {
            match cell.get_next_byte()? {
                GAS_PRICES_TAG => {
                    self.gas_price.read_from(cell)?;
                    self.gas_limit.read_from(cell)?;
                    self.gas_credit.read_from(cell)?;
                    self.block_gas_limit.read_from(cell)?;
                    self.freeze_due_limit.read_from(cell)?;
                    self.delete_due_limit.read_from(cell)?;
                    break;
                }
                GAS_PRICES_EXT_TAG => {
                    self.gas_price.read_from(cell)?;
                    self.gas_limit.read_from(cell)?;
                    self.special_gas_limit.read_from(cell)?;
                    self.gas_credit.read_from(cell)?;
                    self.block_gas_limit.read_from(cell)?;
                    self.freeze_due_limit.read_from(cell)?;
                    self.delete_due_limit.read_from(cell)?;
                    break;
                }
                GAS_FLAT_PFX_TAG => {
                    self.flat_gas_limit.read_from(cell)?;
                    self.flat_gas_price.read_from(cell)?;
                }
                tag => {
                    fail!(Self::invalid_tag(tag as u32))
                }
            }
        }
        self.max_gas_threshold = self.calc_max_gas_threshold(self.gas_limit);
        Ok(())
    }
}

impl Serializable for GasLimitsPrices {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(GAS_FLAT_PFX_TAG)?;
        self.flat_gas_limit.write_to(cell)?;
        self.flat_gas_price.write_to(cell)?;
        cell.append_u8(GAS_PRICES_EXT_TAG)?;
        self.gas_price.write_to(cell)?;
        self.gas_limit.write_to(cell)?;
        self.special_gas_limit.write_to(cell)?;
        self.gas_credit.write_to(cell)?;
        self.block_gas_limit.write_to(cell)?;
        self.freeze_due_limit.write_to(cell)?;
        self.delete_due_limit.write_to(cell)?;
        Ok(())
    }
}

/*
config_mc_gas_prices#_ GasLimitsPrices = ConfigParam 20;
*/
/*
config_gas_prices#_ GasLimitsPrices = ConfigParam 21;
*/

/*

// msg_fwd_fees = (lump_price + ceil((bit_price * msg.bits + cell_price * msg.cells)/2^16)) nanocoins
// ihr_fwd_fees = ceil((msg_fwd_fees * ihr_price_factor)/2^16) nanocoins
// bits in the root cell of a message are not included in msg.bits (lump_price pays for them)
msg_forward_prices#ea
    lump_price:uint64
    bit_price:uint64
    cell_price:uint64
    ihr_price_factor:uint32
    first_frac:uint16
    next_frac:uint16
= MsgForwardPrices;

*/

///
/// MsgForwardPrices
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct MsgForwardPrices {
    pub lump_price: u64,
    pub bit_price: u64,
    pub cell_price: u64,
    pub ihr_price_factor: u32,
    pub first_frac: u16,
    pub next_frac: u16,
}

impl MsgForwardPrices {
    // All prices except `lump_price` are presented in `0xffff * price` form.
    // It is needed because `ihr_factor`, `first_frac` and `next_frac` are not integer values
    // but calculations are performed in integers, so prices are multiplied to some big
    // number (0xffff) and fee calculation uses such values. At the end result is divided by
    // 0xffff with ceil rounding to obtain nanocoins (add 0xffff and then `>> 16`)
    pub fn calc_fwd_fee(&self, bits: u64, cells: u64) -> u128 {
        let res = (bits as u128 * self.bit_price as u128
            + cells as u128 * self.cell_price as u128
            + 0xffff)
            >> 16;
        self.lump_price as u128 + res
    }
    pub fn calc_fwd_fee_simple(&self, bits: u64, cells: u64) -> u128 {
        (bits as u128 * self.bit_price as u128 + cells as u128 * self.cell_price as u128 + 0xffff)
            >> 16
    }

    /// Calculate message IHR fee
    /// IHR fee is calculated as `(msg_forward_fee * ihr_factor) >> 16`
    pub fn ihr_fee_checked(&self, fwd_fee: &Coins) -> Result<Coins> {
        Coins::try_from((fwd_fee.as_u128() * self.ihr_price_factor as u128) >> 16)
    }

    /// Calculate mine part of forward fee
    /// Forward fee for internal message is splited to `int_msg_mine_fee` and `int_msg_remain_fee`:
    /// `msg_forward_fee = int_msg_mine_fee + int_msg_remain_fee`
    /// `int_msg_mine_fee` is a part of transaction `total_fees` and will go validators of account's shard
    /// `int_msg_remain_fee` is placed in header of internal message and will go to validators
    /// of shard to which message destination address is belong.
    pub fn mine_fee_checked(&self, fwd_fee: &Coins) -> Result<Coins> {
        Coins::try_from((fwd_fee.as_u128() * self.first_frac as u128) >> 16)
    }

    pub fn next_fee_checked(&self, fwd_fee: &Coins) -> Result<Coins> {
        Coins::try_from((fwd_fee.as_u128() * self.next_frac as u128) >> 16)
    }
}

const MSG_FWD_PRICES_TAG: u8 = 0xEA;

impl Deserializable for MsgForwardPrices {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != MSG_FWD_PRICES_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.lump_price.read_from(cell)?;
        self.bit_price.read_from(cell)?;
        self.cell_price.read_from(cell)?;
        self.ihr_price_factor.read_from(cell)?;
        self.first_frac.read_from(cell)?;
        self.next_frac.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for MsgForwardPrices {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(MSG_FWD_PRICES_TAG)?;
        self.lump_price.write_to(cell)?;
        self.bit_price.write_to(cell)?;
        self.cell_price.write_to(cell)?;
        self.ihr_price_factor.write_to(cell)?;
        self.first_frac.write_to(cell)?;
        self.next_frac.write_to(cell)?;
        Ok(())
    }
}

/*
// used for messages to/from masterchain
config_mc_fwd_prices#_ MsgForwardPrices = ConfigParam 24;
// used for all other messages
config_fwd_prices#_ MsgForwardPrices = ConfigParam 25;

*/

/*
catchain_config#c1
    mc_catchain_lifetime:uint32
    shard_catchain_lifetime:uint32
    shard_validators_lifetime:uint32
    shard_validators_num:uint32
= CatchainConfig;

catchain_config_new#c2
    flags: (## 7)
    { flags = 0 }
    shuffle_mc_validators: Bool
    mc_catchain_lifetime: uint3
    shard_catchain_lifetime: uint32
    shard_validators_lifetime: uint32
    shard_validators_num: uint32
= CatchainConfig;
*/

///
/// MsgForwardPrices
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CatchainConfig {
    pub isolate_mc_validators: bool,
    pub shuffle_mc_validators: bool,
    pub mc_catchain_lifetime: u32,
    pub shard_catchain_lifetime: u32,
    pub shard_validators_lifetime: u32,
    pub shard_validators_num: u32,
}

impl CatchainConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

const CATCHAIN_CONFIG_TAG_1: u8 = 0xC1;
const CATCHAIN_CONFIG_TAG_2: u8 = 0xC2;

impl Deserializable for CatchainConfig {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if (tag != CATCHAIN_CONFIG_TAG_1) && (tag != CATCHAIN_CONFIG_TAG_2) {
            fail!(Self::invalid_tag(tag as u32))
        }
        if tag == CATCHAIN_CONFIG_TAG_2 {
            let flags = u8::construct_from(cell)?;
            self.isolate_mc_validators = flags & 0b10 != 0;
            self.shuffle_mc_validators = flags & 0b01 != 0;
            if flags >> 2 != 0 {
                fail!(BlockError::InvalidArg("`flags` should be zero".to_string()))
            }
        }
        self.mc_catchain_lifetime.read_from(cell)?;
        self.shard_catchain_lifetime.read_from(cell)?;
        self.shard_validators_lifetime.read_from(cell)?;
        self.shard_validators_num.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for CatchainConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(CATCHAIN_CONFIG_TAG_2)?;
        cell.append_bits(0, 6)?;
        cell.append_bit_bool(self.isolate_mc_validators)?;
        cell.append_bit_bool(self.shuffle_mc_validators)?;
        self.mc_catchain_lifetime.write_to(cell)?;
        self.shard_catchain_lifetime.write_to(cell)?;
        self.shard_validators_lifetime.write_to(cell)?;
        self.shard_validators_num.write_to(cell)?;
        Ok(())
    }
}

/*
_ CatchainConfig = ConfigParam 28;
*/

/*
_ fundamental_smc_addr:(HashmapE 256 True) = ConfigParam 31;

consensus_config#d6
    round_candidates:# { round_candidates >= 1 }
    next_candidate_delay_ms:uint32
    consensus_timeout_ms:uint32
    fast_attempts:uint32
    attempt_duration:uint32
    catchain_max_deps:uint32
    max_block_bytes:uint32
    max_collated_bytes:uint32
= ConsensusConfig;

consensus_config_new#d7
    flags: (## 7)
    { flags = 0 }
    new_catchain_ids: Bool
    round_candidates: (## 8) { round_candidates >= 1 }
    next_candidate_delay_ms: uint32
    consensus_timeout_ms: uint32
    fast_attempts: uint32
    attempt_duration: uint32
    catchain_max_deps: uint32
    max_block_bytes: uint32
    max_collated_bytes: uint32
= ConsensusConfig;

consensus_config_new#d8
    flags: (## 7)
    { flags = 0 }
    new_catchain_ids: Bool
    round_candidates: (## 8) { round_candidates >= 1 }
    next_candidate_delay_ms: uint32
    consensus_timeout_ms: uint32
    fast_attempts: uint32
    attempt_duration: uint32
    catchain_max_deps: uint32
    max_block_bytes: uint32
    max_collated_bytes: uint32
    proto_version: uint16
= ConsensusConfig;

consensus_config_new#d9
    flags: (## 7)
    { flags = 0 }
    new_catchain_ids: Bool
    round_candidates: (## 8) { round_candidates >= 1 }
    next_candidate_delay_ms: uint32
    consensus_timeout_ms: uint32
    fast_attempts: uint32
    attempt_duration: uint32
    catchain_max_deps: uint32
    max_block_bytes: uint32
    max_collated_bytes: uint32
    proto_version: uint16
    catchain_max_blocks_coeff: uint32
= ConsensusConfig;
*/

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ConsensusConfig {
    pub new_catchain_ids: bool,
    pub round_candidates: u32,
    pub next_candidate_delay_ms: u32,
    pub consensus_timeout_ms: u32,
    pub fast_attempts: u32,
    pub attempt_duration: u32,
    pub catchain_max_deps: u32,
    pub max_block_bytes: u32,
    pub max_collated_bytes: u32,
    pub proto_version: u16,
    pub catchain_max_blocks_coeff: u32,
}

impl ConsensusConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

const CONSENSUS_CONFIG_TAG_1: u8 = 0xD6;
const CONSENSUS_CONFIG_TAG_2: u8 = 0xD7;
const CONSENSUS_CONFIG_TAG_3: u8 = 0xD8;
const CONSENSUS_CONFIG_TAG_4: u8 = 0xD9;

impl Deserializable for ConsensusConfig {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if (tag != CONSENSUS_CONFIG_TAG_1)
            && (tag != CONSENSUS_CONFIG_TAG_2)
            && (tag != CONSENSUS_CONFIG_TAG_3)
            && (tag != CONSENSUS_CONFIG_TAG_4)
        {
            fail!(Self::invalid_tag(tag as u32))
        }
        if tag == CONSENSUS_CONFIG_TAG_1 {
            self.round_candidates.read_from(cell)?;
        } else {
            let flags = u8::construct_from(cell)?;
            self.new_catchain_ids = flags == 1;
            if flags >> 1 != 0 {
                fail!(BlockError::InvalidArg("`flags` should be zero".to_string()))
            }
            self.round_candidates = u8::construct_from(cell)? as u32;
            if self.round_candidates == 0 {
                fail!(BlockError::InvalidArg("`round_candidates` should be positive".to_string()))
            }
        }
        self.next_candidate_delay_ms.read_from(cell)?;
        self.consensus_timeout_ms.read_from(cell)?;
        self.fast_attempts.read_from(cell)?;
        self.attempt_duration.read_from(cell)?;
        self.catchain_max_deps.read_from(cell)?;
        self.max_block_bytes.read_from(cell)?;
        self.max_collated_bytes.read_from(cell)?;

        if tag >= CONSENSUS_CONFIG_TAG_3 {
            self.proto_version.read_from(cell)?;
        }

        if tag >= CONSENSUS_CONFIG_TAG_4 {
            self.catchain_max_blocks_coeff.read_from(cell)?;
        }

        Ok(())
    }
}

impl Serializable for ConsensusConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.round_candidates == 0 {
            fail!(BlockError::InvalidArg("`round_candidates` should be positive".to_string()))
        }
        cell.append_u8(CONSENSUS_CONFIG_TAG_4)?;
        cell.append_u8(self.new_catchain_ids as u8)?;
        (self.round_candidates as u8).write_to(cell)?;
        self.next_candidate_delay_ms.write_to(cell)?;
        self.consensus_timeout_ms.write_to(cell)?;
        self.fast_attempts.write_to(cell)?;
        self.attempt_duration.write_to(cell)?;
        self.catchain_max_deps.write_to(cell)?;
        self.max_block_bytes.write_to(cell)?;
        self.max_collated_bytes.write_to(cell)?;
        self.proto_version.write_to(cell)?;
        self.catchain_max_blocks_coeff.write_to(cell)?;
        Ok(())
    }
}

/*
_ fundamental_smc_addr:(HashmapE 256 True) = ConfigParam 31;
*/

define_HashmapE! {FundamentalSmcAddresses, 256, EmptyValue}

impl IntoIterator for &FundamentalSmcAddresses {
    type Item = <HashmapIterator<HashmapE> as std::iter::Iterator>::Item;
    type IntoIter = HashmapIterator<HashmapE>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

///
/// ConfigParam 31;
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam31 {
    pub fundamental_smc_addr: FundamentalSmcAddresses,
}

impl ConfigParam31 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_address(&mut self, address: AccountId) {
        self.fundamental_smc_addr.set(&address, &EmptyValue).unwrap();
    }
}

impl Deserializable for ConfigParam31 {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.fundamental_smc_addr.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ConfigParam31 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.fundamental_smc_addr.write_to(cell)?;
        Ok(())
    }
}

macro_rules! define_configparams {
    ( $cpname:ident, $pname:ident ) => {
        ///
        /// $cpname structure
        ///
        #[derive(Clone, Debug, Eq, PartialEq, Default)]
        pub struct $cpname {
            pub $pname: ValidatorSet,
        }

        impl $cpname {
            /// create new instance of $cpname
            pub fn new() -> Self {
                Self::default()
            }

            pub fn with_validator_set($pname: ValidatorSet) -> Self {
                Self { $pname }
            }
        }

        impl Deserializable for $cpname {
            fn construct_from(slice: &mut SliceData) -> Result<Self> {
                let $pname = ValidatorSet::construct_from(slice)?;
                Ok(Self { $pname })
            }
        }

        impl Serializable for $cpname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                self.$pname.write_to(cell)?;
                Ok(())
            }
        }
    };
}

// _ prev_validators:ValidatorSet = ConfigParam 32;
define_configparams!(ConfigParam32, prev_validators);

// _ prev_temp_validators: ValidatorSet = ConfigParam 33;
define_configparams!(ConfigParam33, prev_temp_validators);

// _ cur_validators:ValidatorSet = ConfigParam 34;
define_configparams!(ConfigParam34, cur_validators);

// _ cur_temp_validators: ValidatorSet = ConfigParam 35;
define_configparams!(ConfigParam35, cur_temp_validators);

//_ next_validators:ValidatorSet = ConfigParam 36;
define_configparams!(ConfigParam36, next_validators);

// _ next_temp_validators: ValidatorSet = ConfigParam 37;
define_configparams!(ConfigParam37, next_temp_validators);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkchainFormat {
    Basic(WorkchainFormat1),
    Extended(WorkchainFormat0),
}

impl Default for WorkchainFormat {
    fn default() -> Self {
        WorkchainFormat::Basic(WorkchainFormat1::default())
    }
}

impl Deserializable for WorkchainFormat {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_bits(3)?;
        match slice.get_next_bit()? {
            true => Ok(WorkchainFormat::Basic(WorkchainFormat1::construct_from(slice)?)),
            false => Ok(WorkchainFormat::Extended(WorkchainFormat0::construct_from(slice)?)),
        }
    }
}

impl Serializable for WorkchainFormat {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(0, 3)?;
        match self {
            WorkchainFormat::Basic(ref val) => {
                cell.append_bit_one()?;
                val.write_to(cell)?;
            }
            WorkchainFormat::Extended(val) => {
                cell.append_bit_zero()?;
                val.write_to(cell)?;
            }
        }
        Ok(())
    }
}

/*
wfmt_basic#1
    vm_version:int32
    vm_mode:uint64
= WorkchainFormat 1;
*/

///
/// Workchain format basic
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct WorkchainFormat1 {
    pub vm_version: i32,
    pub vm_mode: u64,
}

impl WorkchainFormat1 {
    ///
    /// Create empty intance of WorkchainFormat1
    ///
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// Create new instance of WorkchainFormat1
    ///
    pub fn with_params(vm_version: i32, vm_mode: u64) -> Self {
        WorkchainFormat1 { vm_version, vm_mode }
    }
}

impl Deserializable for WorkchainFormat1 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let vm_version = Deserializable::construct_from(slice)?;
        let vm_mode = Deserializable::construct_from(slice)?;
        Ok(Self { vm_version, vm_mode })
    }
}

impl Serializable for WorkchainFormat1 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.vm_version.write_to(cell)?;
        self.vm_mode.write_to(cell)?;
        Ok(())
    }
}

/*
wfmt_ext#0
    min_addr_len:(## 12)
    max_addr_len:(## 12)
    addr_len_step:(## 12)
  { min_addr_len >= 64 } { min_addr_len <= max_addr_len }
  { max_addr_len <= 1023 } { addr_len_step <= 1023 }
  workchain_type_id:(## 32) { workchain_type_id >= 1 }
= WorkchainFormat 0;
*/

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkchainFormat0 {
    min_addr_len: Number12,
    max_addr_len: Number12,
    addr_len_step: Number12,
    workchain_type_id: Number32,
}

impl Default for WorkchainFormat0 {
    fn default() -> Self {
        Self {
            min_addr_len: Number12::from(64),
            max_addr_len: Number12::from(64),
            addr_len_step: Number12::from(0),
            workchain_type_id: Number32::from(1),
        }
    }
}

impl WorkchainFormat0 {
    ///
    /// Create empty new instance of WorkchainFormat0
    ///
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// Create new instance of WorkchainFormat0
    ///
    pub fn with_params(
        min_addr_len: u16,
        max_addr_len: u16,
        addr_len_step: u16,
        workchain_type_id: u32,
    ) -> Result<WorkchainFormat0> {
        if min_addr_len >= 64
            && min_addr_len <= max_addr_len
            && max_addr_len <= 1023
            && addr_len_step <= 1023
            && workchain_type_id >= 1
        {
            Ok(WorkchainFormat0 {
                min_addr_len: Number12::new(min_addr_len as u32)?,
                max_addr_len: Number12::new(max_addr_len as u32)?,
                addr_len_step: Number12::new(addr_len_step as u32)?,
                workchain_type_id: Number32::new(workchain_type_id)?,
            })
        } else {
            fail!(BlockError::InvalidData(
                "min_addr_len >= 64 && min_addr_len <= max_addr_len \
                     && max_addr_len <= 1023 && addr_len_step <= 1023 \
                     && workchain_type_id >= 1"
                    .to_string()
            ))
        }
    }

    ///
    /// Getter for min_addr_len
    ///
    pub fn min_addr_len(&self) -> u16 {
        self.min_addr_len.as_u16()
    }

    ///
    /// Setter for min_addr_len
    ///
    pub fn set_min_addr_len(&mut self, min_addr_len: u16) -> Result<()> {
        if (64..=1023).contains(&min_addr_len) {
            self.min_addr_len = Number12::new(min_addr_len as u32)?;
            Ok(())
        } else {
            fail!(BlockError::InvalidData(
                "should: min_addr_len >= 64 && min_addr_len <= 1023".to_string()
            ))
        }
    }

    ///
    /// Getter for min_addr_len
    ///
    pub fn max_addr_len(&self) -> u16 {
        self.max_addr_len.as_u16()
    }

    ///
    /// Setter for max_addr_len
    ///
    pub fn set_max_addr_len(&mut self, max_addr_len: u16) -> Result<()> {
        if (64..=1024).contains(&max_addr_len) && self.min_addr_len <= max_addr_len as u32 {
            self.max_addr_len = Number12::new(max_addr_len as u32)?;
            Ok(())
        } else {
            fail!(BlockError::InvalidData(
                "should: max_addr_len >= 64 && max_addr_len <= 1024 \
                     && self.min_addr_len <= max_addr_len"
                    .to_string()
            ))
        }
    }

    ///
    /// Getter for addr_len_step
    ///
    pub fn addr_len_step(&self) -> u16 {
        self.addr_len_step.as_u16()
    }

    ///
    /// Setter for min_addr_len
    ///
    pub fn set_addr_len_step(&mut self, addr_len_step: u16) -> Result<()> {
        if addr_len_step <= 1024 {
            self.addr_len_step = Number12::new(addr_len_step as u32)?;
            Ok(())
        } else {
            fail!(BlockError::InvalidData("should: addr_len_step <= 1024".to_string()))
        }
    }

    ///
    /// Getter for workchain_type_id
    ///
    pub fn workchain_type_id(&self) -> u32 {
        self.workchain_type_id.as_u32()
    }

    ///
    /// Setter for min_addr_len
    ///
    pub fn set_workchain_type_id(&mut self, workchain_type_id: u32) -> Result<()> {
        if workchain_type_id >= 1 {
            self.workchain_type_id = Number32::new(workchain_type_id)?;
            Ok(())
        } else {
            fail!(BlockError::InvalidData("should: workchain_type_id >= 1".to_string()))
        }
    }
}

impl Deserializable for WorkchainFormat0 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let min_addr_len = Number12::construct_from(slice)?;
        let max_addr_len = Number12::construct_from(slice)?;
        let addr_len_step = Number12::construct_from(slice)?;
        let workchain_type_id = Number32::construct_from(slice)?;
        if min_addr_len >= 64
            && min_addr_len <= max_addr_len
            && max_addr_len <= 1023
            && addr_len_step <= 1023
            && workchain_type_id >= 1
        {
            Ok(Self { min_addr_len, max_addr_len, addr_len_step, workchain_type_id })
        } else {
            fail!(BlockError::InvalidData(
                "should: min_addr_len >= 64 && min_addr_len <= max_addr_len \
                     && max_addr_len <= 1023 && addr_len_step <= 1023"
                    .to_string()
            ))
        }
    }
}

impl Serializable for WorkchainFormat0 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.min_addr_len >= 64
            && self.min_addr_len <= self.max_addr_len
            && self.max_addr_len <= 1023
            && self.addr_len_step <= 1023
            && self.workchain_type_id >= 1
        {
            self.min_addr_len.write_to(cell)?;
            self.max_addr_len.write_to(cell)?;
            self.addr_len_step.write_to(cell)?;
            self.workchain_type_id.write_to(cell)?;
            Ok(())
        } else {
            fail!(BlockError::InvalidData(
                "should: min_addr_len >= 64 && min_addr_len <= max_addr_len \
                     && max_addr_len <= 1023 && addr_len_step <= 1023"
                    .to_string()
            ))
        }
    }
}

/*
wc_split_merge_timings#0
  split_merge_delay:uint32 split_merge_interval:uint32
  min_split_merge_interval:uint32 max_split_merge_delay:uint32
  = WcSplitMergeTimings;
*/
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WcSplitMergeTimings {
    pub split_merge_delay: u32,
    pub split_merge_interval: u32,
    pub min_split_merge_interval: u32,
    pub max_split_merge_delay: u32,
}
impl Default for WcSplitMergeTimings {
    fn default() -> Self {
        Self {
            split_merge_delay: 100,
            split_merge_interval: 100,
            min_split_merge_interval: 30,
            max_split_merge_delay: 1000,
        }
    }
}
const WC_SPLIT_MERGE_TIMINGS_TAG: u8 = 0x0;
impl Deserializable for WcSplitMergeTimings {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_bits(4)?[0];
        if tag != WC_SPLIT_MERGE_TIMINGS_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.split_merge_delay.read_from(cell)?;
        self.split_merge_interval.read_from(cell)?;
        self.min_split_merge_interval.read_from(cell)?;
        self.max_split_merge_delay.read_from(cell)?;
        Ok(())
    }
}
impl Serializable for WcSplitMergeTimings {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(WC_SPLIT_MERGE_TIMINGS_TAG as usize, 4)?;
        self.split_merge_delay.write_to(cell)?;
        self.split_merge_interval.write_to(cell)?;
        self.min_split_merge_interval.write_to(cell)?;
        self.max_split_merge_delay.write_to(cell)?;
        Ok(())
    }
}

/*
workchain#a6 enabled_since:uint32 monitor_min_split:(## 8)
  min_split:(## 8) max_split:(## 8) { monitor_min_split <= min_split }
  basic:(## 1) active:Bool accept_msgs:Bool flags:(## 13) { flags = 0 }
  zerostate_root_hash:bits256 zerostate_file_hash:bits256
  version:uint32 format:(WorkchainFormat basic)
  = WorkchainDescr;

workchain_v2#a7 enabled_since:uint32 monitor_min_split:(## 8)
  min_split:(## 8) max_split:(## 8) { monitor_min_split <= min_split }
  basic:(## 1) active:Bool accept_msgs:Bool flags:(## 13) { flags = 0 }
  zerostate_root_hash:bits256 zerostate_file_hash:bits256
  version:uint32 format:(WorkchainFormat basic)
  split_merge_timings:WcSplitMergeTimings
  persistent_state_split_depth:(## 8) { persistent_state_split_depth <= 63 }
  = WorkchainDescr;
*/
///
/// WorkchainDescr structure
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkchainDescr {
    pub enabled_since: u32,
    pub monitor_min_split: u8,
    pub min_split: u8,
    pub max_split: u8,
    pub active: bool,
    pub accept_msgs: bool,
    pub flags: u16, // 13 bit
    pub zerostate_root_hash: UInt256,
    pub zerostate_file_hash: UInt256,
    pub version: u32,
    pub format: WorkchainFormat,
    pub split_merge_timings: Option<WcSplitMergeTimings>,
    pub persistent_state_split_depth: u8, // 0..=63
}

impl WorkchainDescr {
    ///
    /// Create empty instance of WorkchainDescr
    ///
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// Getter for min_split
    ///
    pub fn min_split(&self) -> u8 {
        self.min_split
    }

    ///
    /// Setter for min_split
    ///
    pub fn set_min_split(&mut self, min_split: u8) -> Result<()> {
        if min_split <= 60 {
            self.min_split = min_split;
            Ok(())
        } else {
            fail!(BlockError::InvalidData(
                "should: min_split <= max_split && max_split <= 60".to_string()
            ))
        }
    }

    ///
    /// Getter for monitor_min_split
    ///
    pub fn monitor_min_split(&self) -> u8 {
        self.monitor_min_split
    }

    ///
    /// Getter for max_split
    ///
    pub fn max_split(&self) -> u8 {
        self.max_split
    }

    ///
    /// Setter for max_split
    ///
    pub fn set_max_split(&mut self, max_split: u8) -> Result<()> {
        if self.min_split <= max_split && max_split <= 60 {
            self.max_split = max_split;
            Ok(())
        } else {
            fail!(BlockError::InvalidData(
                "should: min_split <= max_split && max_split <= 60".to_string()
            ))
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn basic(&self) -> bool {
        matches!(self.format, WorkchainFormat::Basic(_))
    }
}

const WORKCHAIN_DESCRIPTOR_TAG: u8 = 0xA6;
const WORKCHAIN_DESCRIPTOR_TAG_2: u8 = 0xA7;

impl Deserializable for WorkchainDescr {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != WORKCHAIN_DESCRIPTOR_TAG && tag != WORKCHAIN_DESCRIPTOR_TAG_2 {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.enabled_since.read_from(cell)?;
        let mut min = Number8::default();
        min.read_from(cell)?;
        self.monitor_min_split = min.as_u8();
        let mut min = Number8::default();
        min.read_from(cell)?;
        self.min_split = min.as_u8();
        let mut max = Number8::default();
        max.read_from(cell)?;
        self.max_split = max.as_u8();
        cell.get_next_bit()?; // basic
        self.active = cell.get_next_bit()?;
        self.accept_msgs = cell.get_next_bit()?;
        let mut flags = Number13::default();
        flags.read_from(cell)?;
        self.flags = flags.as_u16();
        self.zerostate_root_hash.read_from(cell)?;
        self.zerostate_file_hash.read_from(cell)?;
        self.version.read_from(cell)?;
        self.format.read_from(cell)?;
        if tag == WORKCHAIN_DESCRIPTOR_TAG_2 {
            self.split_merge_timings = Some(WcSplitMergeTimings::construct_from(cell)?);
            let mut persistent_state_split_depth = Number8::default();
            persistent_state_split_depth.read_from(cell)?;
            if persistent_state_split_depth.as_u8() <= 63 {
                self.persistent_state_split_depth = persistent_state_split_depth.as_u8();
            } else {
                fail!(BlockError::InvalidData(
                    "persistent_state_split_depth should be <= 63".to_string()
                ));
            }
        } else {
            self.split_merge_timings = None;
            self.persistent_state_split_depth = 0;
        }

        Ok(())
    }
}

impl Serializable for WorkchainDescr {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.max_split > MAX_SPLIT_DEPTH {
            fail!("max_split must be <= {}", MAX_SPLIT_DEPTH);
        }
        if self.min_split > self.max_split {
            fail!("min_split must be <= max_split");
        }
        if self.monitor_min_split > self.min_split {
            fail!("monitor_min_split must be <= min_split");
        }
        if self.split_merge_timings.is_none() && self.persistent_state_split_depth != 0 {
            fail!("persistent_state_split_depth must be 0 when split_merge_timings is None");
        }

        let tag = if self.split_merge_timings.is_some() {
            WORKCHAIN_DESCRIPTOR_TAG_2
        } else {
            WORKCHAIN_DESCRIPTOR_TAG
        };
        cell.append_u8(tag)?;

        self.enabled_since.write_to(cell)?;

        let min = Number8::new(self.monitor_min_split as u32)?;
        min.write_to(cell)?;

        let min = Number8::new(self.min_split as u32)?;
        min.write_to(cell)?;

        let max = Number8::new(self.max_split as u32)?;
        max.write_to(cell)?;

        if let WorkchainFormat::Basic(_) = self.format {
            cell.append_bit_one()?;
        } else {
            cell.append_bit_zero()?;
        }

        if self.active {
            cell.append_bit_one()?;
        } else {
            cell.append_bit_zero()?;
        }

        if self.accept_msgs {
            cell.append_bit_one()?;
        } else {
            cell.append_bit_zero()?;
        }

        let flags = Number13::new(self.flags as u32)?;
        flags.write_to(cell)?;
        self.zerostate_root_hash.write_to(cell)?;
        self.zerostate_file_hash.write_to(cell)?;
        self.version.write_to(cell)?;
        self.format.write_to(cell)?;
        if let Some(ref timings) = self.split_merge_timings {
            timings.write_to(cell)?;
            let persistent_state_split_depth =
                Number8::new(self.persistent_state_split_depth as u32)?;
            persistent_state_split_depth.write_to(cell)?;
        }

        Ok(())
    }
}

/*
cfg_vote_cfg#36
    min_tot_rounds: uint8
    max_tot_rounds: uint8
    min_wins: uint8
    max_losses: uint8
    min_store_sec: uint32
    max_store_sec: uint32
    bit_price: uint32
    cell_price: uint32
= ConfigProposalSetup;

cfg_vote_setup#91
    normal_params: ^ConfigProposalSetup
    critical_params: ^ConfigProposalSetup
= ConfigVotingSetup;

_ ConfigVotingSetup = ConfigParam 11;
*/

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigProposalSetup {
    pub min_tot_rounds: u8,
    pub max_tot_rounds: u8,
    pub min_wins: u8,
    pub max_losses: u8,
    pub min_store_sec: u32,
    pub max_store_sec: u32,
    pub bit_price: u32,
    pub cell_price: u32,
}

const CONFIG_PROPOSAL_SETUP_TAG: u8 = 0x36;

impl Deserializable for ConfigProposalSetup {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != CONFIG_PROPOSAL_SETUP_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.min_tot_rounds.read_from(slice)?;
        self.max_tot_rounds.read_from(slice)?;
        self.min_wins.read_from(slice)?;
        self.max_losses.read_from(slice)?;
        self.min_store_sec.read_from(slice)?;
        self.max_store_sec.read_from(slice)?;
        self.bit_price.read_from(slice)?;
        self.cell_price.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for ConfigProposalSetup {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(CONFIG_PROPOSAL_SETUP_TAG)?;
        self.min_tot_rounds.write_to(cell)?;
        self.max_tot_rounds.write_to(cell)?;
        self.min_wins.write_to(cell)?;
        self.max_losses.write_to(cell)?;
        self.min_store_sec.write_to(cell)?;
        self.max_store_sec.write_to(cell)?;
        self.bit_price.write_to(cell)?;
        self.cell_price.write_to(cell)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigVotingSetup {
    normal_params: ChildCell<ConfigProposalSetup>,
    critical_params: ChildCell<ConfigProposalSetup>,
}

impl ConfigVotingSetup {
    pub fn new(
        normal_params: &ConfigProposalSetup,
        critical_params: &ConfigProposalSetup,
    ) -> Result<Self> {
        Ok(ConfigVotingSetup {
            normal_params: ChildCell::with_struct(normal_params)?,
            critical_params: ChildCell::with_struct(critical_params)?,
        })
    }

    pub fn read_normal_params(&self) -> Result<ConfigProposalSetup> {
        self.normal_params.read_struct()
    }

    pub fn write_normal_params(&mut self, value: &ConfigProposalSetup) -> Result<()> {
        self.normal_params.write_struct(value)
    }

    pub fn normal_params_cell(&self) -> Cell {
        self.normal_params.cell()
    }

    pub fn read_critical_params(&self) -> Result<ConfigProposalSetup> {
        self.critical_params.read_struct()
    }

    pub fn write_critical_params(&mut self, value: &ConfigProposalSetup) -> Result<()> {
        self.critical_params.write_struct(value)
    }

    pub fn critical_params_cell(&self) -> Cell {
        self.critical_params.cell()
    }
}

const CONFIG_VOTING_SETUP_TAG: u8 = 0x91;

impl Deserializable for ConfigVotingSetup {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != CONFIG_VOTING_SETUP_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.normal_params.read_from(slice)?;
        self.critical_params.read_from(slice)?;

        Ok(())
    }
}

impl Serializable for ConfigVotingSetup {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(CONFIG_VOTING_SETUP_TAG)?;
        self.normal_params.write_to(cell)?;
        self.critical_params.write_to(cell)?;
        Ok(())
    }
}

pub type ConfigParam11 = ConfigVotingSetup;

/*
_ workchains:(HashmapE 32 WorkchainDescr) = ConfigParam 12;
*/
define_HashmapE! {Workchains, 32, WorkchainDescr}

///
/// ConfigParam 12 struct
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam12 {
    pub workchains: Workchains,
}

impl ConfigParam12 {
    /// new instance of ConfigParam12
    pub fn new() -> Self {
        Self::default()
    }

    /// determine is empty
    pub fn is_empty(&self) -> bool {
        self.workchains.is_empty()
    }

    /// get value by index
    pub fn get(&self, workchain_id: i32) -> Result<Option<WorkchainDescr>> {
        self.workchains.get(&workchain_id)
    }

    /// insert value
    pub fn insert(&mut self, workchain_id: i32, sp: &WorkchainDescr) -> Result<()> {
        self.workchains.set(&workchain_id, sp)
    }
}

impl Deserializable for ConfigParam12 {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.workchains.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for ConfigParam12 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.workchains.write_to(cell)?;
        Ok(())
    }
}

///
/// ConfigParam 13 struct
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam13 {
    pub cell: Cell,
}

impl Deserializable for ConfigParam13 {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.cell = slice.clone().into_cell()?;
        Ok(())
    }
}

impl Serializable for ConfigParam13 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.checked_append_references_and_data(&SliceData::load_cell_ref(&self.cell)?)?;
        Ok(())
    }
}

// validator_temp_key#3
//     adnl_addr:bits256
//     temp_public_key:SigPubKey
//     seqno:#
//     valid_until:uint32
// = ValidatorTempKey;

const VALIDATOR_TEMP_KEY_TAG: u8 = 0x3;

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct ValidatorTempKey {
    adnl_addr: UInt256,
    temp_public_key: SigPubKey,
    seqno: u32,
    valid_until: u32,
}

impl ValidatorTempKey {
    /// new instance of ValidatorTempKey
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_params(
        adnl_addr: UInt256,
        temp_public_key: SigPubKey,
        seqno: u32,
        valid_until: u32,
    ) -> Self {
        Self { adnl_addr, temp_public_key, seqno, valid_until }
    }

    pub fn set_adnl_addr(&mut self, adnl_addr: UInt256) {
        self.adnl_addr = adnl_addr
    }

    pub fn adnl_addr(&self) -> &UInt256 {
        &self.adnl_addr
    }

    pub fn set_key(&mut self, temp_public_key: SigPubKey) {
        self.temp_public_key = temp_public_key
    }

    pub fn temp_public_key(&self) -> &SigPubKey {
        &self.temp_public_key
    }

    pub fn set_seqno(&mut self, seqno: u32) {
        self.seqno = seqno
    }

    pub fn seqno(&self) -> u32 {
        self.seqno
    }

    pub fn set_valid_until(&mut self, valid_until: u32) {
        self.valid_until = valid_until
    }

    pub fn valid_until(&self) -> u32 {
        self.valid_until
    }
}

impl Deserializable for ValidatorTempKey {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?; // TODO what is tag length in bits???
        if tag != VALIDATOR_TEMP_KEY_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.adnl_addr.read_from(slice)?;
        self.temp_public_key.read_from(slice)?;
        self.seqno.read_from(slice)?;
        self.valid_until.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for ValidatorTempKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(VALIDATOR_TEMP_KEY_TAG)?; // TODO what is tag length in bits???
        self.adnl_addr.write_to(cell)?;
        self.temp_public_key.write_to(cell)?;
        self.seqno.write_to(cell)?;
        self.valid_until.write_to(cell)?;
        Ok(())
    }
}

// signed_temp_key#4
//     key:^ValidatorTempKey
//     signature:CryptoSignature
// = ValidatorSignedTempKey;

const VALIDATOR_SIGNED_TEMP_KEY_TAG: u8 = 0x4;

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct ValidatorSignedTempKey {
    key: ValidatorTempKey,
    signature: CryptoSignature,
}

impl ValidatorSignedTempKey {
    /// new instance of
    pub fn with_key_and_signature(key: ValidatorTempKey, signature: CryptoSignature) -> Self {
        Self { key, signature }
    }

    pub fn key(&self) -> &ValidatorTempKey {
        &self.key
    }

    pub fn signature(&self) -> &CryptoSignature {
        &self.signature
    }
}

impl Deserializable for ValidatorSignedTempKey {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?; // TODO what is tag length in bits???
        if tag != VALIDATOR_SIGNED_TEMP_KEY_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.signature.read_from(slice)?;
        self.key.read_from_cell(slice.checked_drain_reference()?)?;
        Ok(())
    }
}

impl Serializable for ValidatorSignedTempKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(VALIDATOR_SIGNED_TEMP_KEY_TAG)?; // TODO what is tag length in bits???
        self.signature.write_to(cell)?;
        cell.checked_append_reference(self.key.serialize()?)?;
        Ok(())
    }
}

///
/// ConfigParam 39 struct
///
// _ (HashmapE 256 ValidatorSignedTempKey) = ConfigParam 39;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigParam39 {
    pub validator_keys: ValidatorKeys,
}

define_HashmapE!(ValidatorKeys, 256, ValidatorSignedTempKey);

impl ConfigParam39 {
    pub fn new() -> Self {
        Self::default()
    }

    /// determine is empty
    pub fn is_empty(&self) -> bool {
        self.validator_keys.is_empty()
    }

    /// get value by key
    pub fn get(&self, key: &UInt256) -> Result<ValidatorSignedTempKey> {
        self.validator_keys
            .get(key)?
            .ok_or_else(|| error!(BlockError::InvalidArg(format!("{:x}", key))))
    }

    /// insert value
    pub fn insert(&mut self, key: &UInt256, validator_key: &ValidatorSignedTempKey) -> Result<()> {
        self.validator_keys.set(key, validator_key)
    }
}

impl Deserializable for ConfigParam39 {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.validator_keys.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for ConfigParam39 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.validator_keys.write_to(cell)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum ParamLimitIndex {
    Underload = 0,
    Normal,
    Soft,
    Medium,
    Hard,
}

const LIMIT_COUNT: usize = 4;

// param_limits#c3
//     underload:#
//     soft_limit:#
//     { underload <= soft_limit }
//     hard_limit:#
//     { soft_limit <= hard_limit }
// = ParamLimits;

const PARAM_LIMITS_TAG: u8 = 0xc3;

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct ParamLimits {
    // [ unerload , soft, (soft+hard)/2, hard ]
    limits: [u32; LIMIT_COUNT],
}

impl ParamLimits {
    pub fn with_limits(underload: u32, soft: u32, hard: u32) -> Result<Self> {
        let mut limits = [0u32; LIMIT_COUNT];
        Self::set_limits(&mut limits, underload, soft, hard)?;
        Ok(Self { limits })
    }

    pub fn classify(&self, value: u32) -> ParamLimitIndex {
        if value >= self.medium() {
            if value >= self.hard_limit() {
                ParamLimitIndex::Hard
            } else {
                ParamLimitIndex::Medium
            }
        } else if value >= self.underload() {
            if value >= self.soft_limit() {
                ParamLimitIndex::Soft
            } else {
                ParamLimitIndex::Normal
            }
        } else {
            ParamLimitIndex::Underload
        }
    }

    pub fn fits(&self, level: ParamLimitIndex, value: u32) -> bool {
        // *level*         *checks*
        // Underload       value < unerload
        // Normal          value < soft
        // Soft            value < medium
        // Medium          value < hard
        // Hard            always true
        level == ParamLimitIndex::Hard || value < self.limits[level as usize]
    }

    pub fn fits_normal(&self, value: u32, percent: u32) -> bool {
        value * 100 < self.soft_limit() * percent
    }

    pub fn underload(&self) -> u32 {
        self.limits[ParamLimitIndex::Underload as usize]
    }

    pub fn soft_limit(&self) -> u32 {
        self.limits[ParamLimitIndex::Soft as usize - 1]
    }

    pub fn medium(&self) -> u32 {
        self.limits[ParamLimitIndex::Medium as usize - 1]
    }

    pub fn hard_limit(&self) -> u32 {
        self.limits[ParamLimitIndex::Hard as usize - 1]
    }

    fn compute_medium_limit(soft: u32, hard: u32) -> u32 {
        soft + ((hard - soft) >> 1)
    }

    fn set_limits(
        limits: &mut [u32; LIMIT_COUNT],
        underload: u32,
        soft: u32,
        hard: u32,
    ) -> Result<()> {
        if underload > soft {
            fail!(BlockError::InvalidArg(
                "underload have to be less or equal to soft limit".to_string()
            ))
        }
        if soft > hard {
            fail!(BlockError::InvalidArg(
                "soft limit have to be less or equal to hard one".to_string()
            ))
        }
        limits[ParamLimitIndex::Underload as usize] = underload;
        limits[ParamLimitIndex::Soft as usize - 1] = soft;
        limits[ParamLimitIndex::Medium as usize - 1] = Self::compute_medium_limit(soft, hard);
        limits[ParamLimitIndex::Hard as usize - 1] = hard;
        Ok(())
    }
}

impl Deserializable for ParamLimits {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != PARAM_LIMITS_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        let underload = u32::construct_from(slice)?;
        let soft = u32::construct_from(slice)?;
        let hard = u32::construct_from(slice)?;
        Self::set_limits(&mut self.limits, underload, soft, hard)
    }
}

impl Serializable for ParamLimits {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(PARAM_LIMITS_TAG)?;
        self.underload().write_to(cell)?;
        self.soft_limit().write_to(cell)?;
        self.hard_limit().write_to(cell)?;
        Ok(())
    }
}

// imported_msg_queue_limits#d3 max_bytes:# max_msgs:# = ImportedMsgQueueLimits;

const IMPORTED_MSG_QUEUE_LIMITS_TAG: u8 = 0xd3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedMsgQueueLimits {
    pub max_bytes: u32,
    pub max_msgs: u32,
}

impl Default for ImportedMsgQueueLimits {
    fn default() -> Self {
        Self {
            max_bytes: 1 << 16, // 64K
            max_msgs: 30,
        }
    }
}

impl ImportedMsgQueueLimits {
    pub fn new(max_bytes: u32, max_msgs: u32) -> Self {
        Self { max_bytes, max_msgs }
    }
}

impl Deserializable for ImportedMsgQueueLimits {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != IMPORTED_MSG_QUEUE_LIMITS_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.max_bytes.read_from(slice)?;
        self.max_msgs.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for ImportedMsgQueueLimits {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(IMPORTED_MSG_QUEUE_LIMITS_TAG)?;
        self.max_bytes.write_to(cell)?;
        self.max_msgs.write_to(cell)?;
        Ok(())
    }
}

// block_limits#5d bytes:ParamLimits gas:ParamLimits lt_delta:ParamLimits
//   = BlockLimits;
// block_limits_v2#5e bytes:ParamLimits gas:ParamLimits lt_delta:ParamLimits
//   collated_data:ParamLimits imported_msg_queue:ImportedMsgQueueLimits
//   = BlockLimits;

const BLOCK_LIMITS_TAG: u8 = 0x5d;
const BLOCK_LIMITS_TAG_2: u8 = 0x5e;

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct BlockLimits {
    bytes: ParamLimits,
    gas: ParamLimits,
    lt_delta: ParamLimits,
    collated_data: Option<ParamLimits>,
    imported_msg_queue: Option<ImportedMsgQueueLimits>,
}

impl BlockLimits {
    pub fn with_limits(
        bytes: ParamLimits,
        gas: ParamLimits,
        lt_delta: ParamLimits,
        collated_data: Option<ParamLimits>,
        imported_msg_queue: Option<ImportedMsgQueueLimits>,
    ) -> Self {
        Self { bytes, gas, lt_delta, collated_data, imported_msg_queue }
    }

    pub fn bytes(&self) -> &ParamLimits {
        &self.bytes
    }

    pub fn gas(&self) -> &ParamLimits {
        &self.gas
    }

    pub fn lt_delta(&self) -> &ParamLimits {
        &self.lt_delta
    }

    pub fn collated_data(&self) -> Option<&ParamLimits> {
        self.collated_data.as_ref()
    }

    pub fn imported_msg_queue(&self) -> Option<&ImportedMsgQueueLimits> {
        self.imported_msg_queue.as_ref()
    }

    pub fn fits(&self, level: ParamLimitIndex, bytes: u32, gas: u32, lt_delta: u32) -> bool {
        self.gas.fits(level, gas)
            && self.bytes.fits(level, bytes)
            && self.lt_delta.fits(level, lt_delta)
    }

    pub fn fits_normal(&self, bytes: u32, gas: u32, lt_delta: u32, percent: u32) -> bool {
        self.gas.fits_normal(gas, percent)
            && self.bytes.fits_normal(bytes, percent)
            && self.lt_delta.fits_normal(lt_delta, percent)
    }
}

impl Deserializable for BlockLimits {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != BLOCK_LIMITS_TAG && tag != BLOCK_LIMITS_TAG_2 {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.bytes.read_from(slice)?;
        self.gas.read_from(slice)?;
        self.lt_delta.read_from(slice)?;
        if tag == BLOCK_LIMITS_TAG_2 {
            self.collated_data = Some(ParamLimits::construct_from(slice)?);
            self.imported_msg_queue = Some(ImportedMsgQueueLimits::construct_from(slice)?);
        } else {
            self.collated_data = None;
            self.imported_msg_queue = None;
        }
        Ok(())
    }
}

impl Serializable for BlockLimits {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.collated_data.is_some() ^ self.imported_msg_queue.is_some() {
            fail!("collated_data and imported_msg_queue limits should be both Some or both None");
        }
        let tag = if self.collated_data.is_some() { BLOCK_LIMITS_TAG_2 } else { BLOCK_LIMITS_TAG };
        cell.append_u8(tag)?;
        self.bytes.write_to(cell)?;
        self.gas.write_to(cell)?;
        self.lt_delta.write_to(cell)?;
        if let Some(collated_data) = self.collated_data.as_ref() {
            collated_data.write_to(cell)?;
        }
        if let Some(imported_msg_queue) = self.imported_msg_queue.as_ref() {
            imported_msg_queue.write_to(cell)?;
        }
        Ok(())
    }
}

type ConfigParam22 = BlockLimits;
type ConfigParam23 = BlockLimits;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MisbehaviourPunishmentConfig {
    pub default_flat_fine: Coins,
    pub default_proportional_fine: u32,
    pub severity_flat_mult: u16,
    pub severity_proportional_mult: u16,
    pub unpunishable_interval: u16,
    pub long_interval: u16,
    pub long_flat_mult: u16,
    pub long_proportional_mult: u16,
    pub medium_interval: u16,
    pub medium_flat_mult: u16,
    pub medium_proportional_mult: u16,
}

const MISBEHAVIOUR_PUNISHMENT_CONFIG_TAG: u8 = 0x01;

impl Serializable for MisbehaviourPunishmentConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(MISBEHAVIOUR_PUNISHMENT_CONFIG_TAG)?;
        self.default_flat_fine.write_to(cell)?;
        self.default_proportional_fine.write_to(cell)?;
        self.severity_flat_mult.write_to(cell)?;
        self.severity_proportional_mult.write_to(cell)?;
        self.unpunishable_interval.write_to(cell)?;
        self.long_interval.write_to(cell)?;
        self.long_flat_mult.write_to(cell)?;
        self.long_proportional_mult.write_to(cell)?;
        self.medium_interval.write_to(cell)?;
        self.medium_flat_mult.write_to(cell)?;
        self.medium_proportional_mult.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for MisbehaviourPunishmentConfig {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != MISBEHAVIOUR_PUNISHMENT_CONFIG_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.default_flat_fine.read_from(slice)?;
        self.default_proportional_fine.read_from(slice)?;
        self.severity_flat_mult.read_from(slice)?;
        self.severity_proportional_mult.read_from(slice)?;
        self.unpunishable_interval.read_from(slice)?;
        self.long_interval.read_from(slice)?;
        self.long_flat_mult.read_from(slice)?;
        self.long_proportional_mult.read_from(slice)?;
        self.medium_interval.read_from(slice)?;
        self.medium_flat_mult.read_from(slice)?;
        self.medium_proportional_mult.read_from(slice)?;
        Ok(())
    }
}

pub const MAX_MSG_BITS: u32 = 1 << 21;
pub const MAX_MSG_CELLS: u32 = 1 << 13;
pub const MAX_MSG_MERKLE_DEPTH: u32 = 2;
pub const MAX_MERKLE_DEPTH: u8 = 2;
pub const DICT_HASH_MIN_CELLS: u32 = 26;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SizeLimitsConfig {
    pub max_msg_bits: u32,
    pub max_msg_cells: u32,
    pub max_library_cells: u32,
    pub max_vm_data_depth: u16,
    pub max_ext_msg_size: u32,
    pub max_ext_msg_depth: u16,
    pub max_acc_state_cells: u32,
    pub max_mc_acc_state_cells: u32,
    pub max_acc_public_libraries: u32,
    pub defer_out_queue_size_limit: u32,
    pub max_msg_extra_currencies: u32,
    pub max_acc_fixed_prefix_length: u8,
    pub acc_state_cells_for_storage_dict: u32,
}

impl Default for SizeLimitsConfig {
    fn default() -> Self {
        Self {
            max_msg_bits: MAX_MSG_BITS,
            max_msg_cells: MAX_MSG_CELLS,
            max_library_cells: 1000,
            max_vm_data_depth: 512,
            max_ext_msg_size: 65535,
            max_ext_msg_depth: 512,
            max_acc_state_cells: 1 << 16,
            max_mc_acc_state_cells: 1 << 11,
            max_acc_public_libraries: 256,
            defer_out_queue_size_limit: 256,
            max_msg_extra_currencies: 2,
            max_acc_fixed_prefix_length: 8,
            acc_state_cells_for_storage_dict: DICT_HASH_MIN_CELLS,
        }
    }
}

impl Serializable for SizeLimitsConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        2u8.write_to(cell)?;
        self.max_msg_bits.write_to(cell)?;
        self.max_msg_cells.write_to(cell)?;
        self.max_library_cells.write_to(cell)?;
        self.max_vm_data_depth.write_to(cell)?;
        self.max_ext_msg_size.write_to(cell)?;
        self.max_ext_msg_depth.write_to(cell)?;
        self.max_acc_state_cells.write_to(cell)?;
        self.max_mc_acc_state_cells.write_to(cell)?;
        self.max_acc_public_libraries.write_to(cell)?;
        self.defer_out_queue_size_limit.write_to(cell)?;
        self.max_msg_extra_currencies.write_to(cell)?;
        self.max_acc_fixed_prefix_length.write_to(cell)?;
        self.acc_state_cells_for_storage_dict.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for SizeLimitsConfig {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_byte()?;
        if tag != 1 && tag != 2 {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.max_msg_bits.read_from(slice)?;
        self.max_msg_cells.read_from(slice)?;
        self.max_library_cells.read_from(slice)?;
        self.max_vm_data_depth.read_from(slice)?;
        self.max_ext_msg_size.read_from(slice)?;
        self.max_ext_msg_depth.read_from(slice)?;
        if tag == 2 {
            self.max_acc_state_cells.read_from(slice)?;
            self.max_mc_acc_state_cells.read_from(slice)?;
            self.max_acc_public_libraries.read_from(slice)?;
            self.defer_out_queue_size_limit.read_from(slice)?;
            self.max_msg_extra_currencies.read_from(slice)?;
            self.max_acc_fixed_prefix_length.read_from(slice)?;
            self.acc_state_cells_for_storage_dict.read_from(slice)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct SuspendedAddressesKey {
    pub workchain_id: i32,
    pub address: SliceData,
}
impl SuspendedAddressesKey {
    pub fn new(workchain_id: i32, address: SliceData) -> Self {
        Self { workchain_id, address }
    }
}
impl Serializable for SuspendedAddressesKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_i32(self.workchain_id)?;
        cell.append_bytestring(&self.address)?;
        Ok(())
    }
}
impl Deserializable for SuspendedAddressesKey {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.workchain_id = slice.get_next_i32()?;
        self.address = slice.get_next_slice(256)?;
        Ok(())
    }
}

define_HashmapE! {SuspendedAddresses, 288, EmptyValue}

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct SuspendedAddressList {
    addresses: SuspendedAddresses,
    suspended_until: u32,
}

impl Serializable for SuspendedAddressList {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        0u8.write_to(cell)?;
        self.addresses.write_to(cell)?;
        self.suspended_until.write_to(cell)?;
        Ok(())
    }
}
impl Deserializable for SuspendedAddressList {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != 0x00 {
            fail!(Self::invalid_tag(tag as u32))
        }
        let addresses = Deserializable::construct_from(slice)?;
        let suspended_until = slice.get_next_u32()?;
        Ok(Self { addresses, suspended_until })
    }
}

impl SuspendedAddressList {
    pub fn set_suspended_until(&mut self, suspended_until: u32) {
        self.suspended_until = suspended_until
    }
    pub fn suspended_until(&self) -> u32 {
        self.suspended_until
    }
    pub fn is_address_suspended(&self, addr: &MsgAddressInt, now: u32) -> Result<bool> {
        if self.suspended_until <= now {
            return Ok(false);
        }
        let key = SuspendedAddressesKey::new(addr.workchain_id(), addr.address().clone());
        self.addresses.check_key(&key)
    }
    pub fn add_suspended_address(&mut self, wc: i32, addr: SliceData) -> Result<()> {
        let key = SuspendedAddressesKey::new(wc, addr);
        self.addresses.set(&key, &EmptyValue)
    }
    pub fn iterate_addresses(
        &self,
        p: impl FnMut(SuspendedAddressesKey) -> Result<bool>,
    ) -> Result<bool> {
        self.addresses.iterate_keys(p)
    }

    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrecompiledSmc {
    pub gas_usage: u64,
}

const PRECOMPILED_CONTRACT_TAG: u8 = 0xb0;

impl Serializable for PrecompiledSmc {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        PRECOMPILED_CONTRACT_TAG.write_to(cell)?;
        self.gas_usage.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for PrecompiledSmc {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != PRECOMPILED_CONTRACT_TAG {
            fail!(Self::invalid_tag(tag as u32));
        }
        let gas_usage = slice.get_next_u64()?;
        Ok(Self { gas_usage })
    }
}

define_HashmapE! {PrecompiledContracts, 256, PrecompiledSmc}

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct PrecompiledContractsList {
    contracts: PrecompiledContracts,
}

const PRECOMPILED_CONTRACTS_LIST_TAG: u8 = 0xc0;

impl Serializable for PrecompiledContractsList {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        PRECOMPILED_CONTRACTS_LIST_TAG.write_to(cell)?;
        self.contracts.write_to(cell)?;
        Ok(())
    }
}
impl Deserializable for PrecompiledContractsList {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != PRECOMPILED_CONTRACTS_LIST_TAG {
            fail!(Self::invalid_tag(tag as u32));
        }
        let contracts = Deserializable::construct_from(slice)?;
        Ok(Self { contracts })
    }
}

impl PrecompiledContractsList {
    pub fn add(&mut self, code_hash: &UInt256, gas_usage: u64) -> Result<()> {
        let precompiled = PrecompiledSmc { gas_usage };
        self.contracts.set(code_hash, &precompiled)
    }
    pub fn get(&self, code_hash: &UInt256) -> Result<Option<PrecompiledSmc>> {
        self.contracts.get(code_hash)
    }
    pub fn iterate(&self, mut p: impl FnMut(SliceData, u64) -> Result<bool>) -> Result<bool> {
        self.contracts.iterate_with_keys(|address, precompiled| p(address, precompiled.gas_usage))
    }
}

define_HashmapE!(Oracles, 256, AccountId);

// oracle_bridge_params#_ bridge_address:bits256 oracle_mutlisig_address:bits256 oracles:(HashmapE 256 uint256) external_chain_address:bits256 = OracleBridgeParams;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleBridgeParams {
    pub bridge_address: AccountId,
    pub oracle_mutlisig_address: AccountId,
    pub oracles: Oracles,
    pub external_chain_address: AccountId,
}

impl Default for OracleBridgeParams {
    fn default() -> Self {
        Self {
            oracles: Oracles::default(),
            bridge_address: [0; 32].into(),
            external_chain_address: [0; 32].into(),
            oracle_mutlisig_address: [0; 32].into(),
        }
    }
}

impl Serializable for OracleBridgeParams {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.bridge_address.write_to(cell)?;
        self.oracle_mutlisig_address.write_to(cell)?;
        self.oracles.write_to(cell)?;
        self.external_chain_address.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OracleBridgeParams {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let bridge_address = AccountId::construct_from(slice)?;
        let oracle_mutlisig_address = AccountId::construct_from(slice)?;
        let oracles = Oracles::construct_from(slice)?;
        let external_chain_address = AccountId::construct_from(slice)?;
        Ok(Self { bridge_address, oracle_mutlisig_address, oracles, external_chain_address })
    }
}

// jetton_bridge_prices#_ bridge_burn_fee:Coins bridge_mint_fee:Coins
//                        wallet_min_tons_for_storage:Coins
//                        wallet_gas_consumption:Coins
//                        minter_min_tons_for_storage:Coins
//                        discover_gas_consumption:Coins = JettonBridgePrices;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JettonBridgePrices {
    pub bridge_burn_fee: Coins,
    pub bridge_mint_fee: Coins,
    pub wallet_min_tons_for_storage: Coins,
    pub wallet_gas_consumption: Coins,
    pub minter_min_tons_for_storage: Coins,
    pub discover_gas_consumption: Coins,
}

impl Serializable for JettonBridgePrices {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.bridge_burn_fee.write_to(cell)?;
        self.bridge_mint_fee.write_to(cell)?;
        self.wallet_min_tons_for_storage.write_to(cell)?;
        self.wallet_gas_consumption.write_to(cell)?;
        self.minter_min_tons_for_storage.write_to(cell)?;
        self.discover_gas_consumption.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for JettonBridgePrices {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let bridge_burn_fee = Coins::construct_from(slice)?;
        let bridge_mint_fee = Coins::construct_from(slice)?;
        let wallet_min_tons_for_storage = Coins::construct_from(slice)?;
        let wallet_gas_consumption = Coins::construct_from(slice)?;
        let minter_min_tons_for_storage = Coins::construct_from(slice)?;
        let discover_gas_consumption = Coins::construct_from(slice)?;
        Ok(Self {
            bridge_burn_fee,
            bridge_mint_fee,
            wallet_min_tons_for_storage,
            wallet_gas_consumption,
            minter_min_tons_for_storage,
            discover_gas_consumption,
        })
    }
}

// jetton_bridge_params_v0#00 bridge_address:bits256 oracles_address:bits256 oracles:(HashmapE 256 uint256) state_flags:uint8 burn_bridge_fee:Coins = JettonBridgeParams;
// jetton_bridge_params_v1#01 bridge_address:bits256 oracles_address:bits256 oracles:(HashmapE 256 uint256) state_flags:uint8 prices:^JettonBridgePrices external_chain_address:bits256 = JettonBridgeParams;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JettonBridgeParams {
    pub bridge_address: AccountId,
    pub oracles_address: AccountId,
    pub oracles: Oracles,
    pub state_flags: u8,
    pub prices: JettonBridgePrices,
    pub external_chain_address: AccountId,
}

// accelerated_consensus_config#_ enabled:Bool failed_collation_retry_timeout_ms:uint32 skip_rounds_count_for_collator_rotation:uint32 = AcceleratedConsensusConfig;

const ACCELERATED_CONSENSUS_CONFIG_TAG: u8 = 0x03;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceleratedConsensusConfig {
    pub enabled: bool,
    pub failed_collation_retry_timeout_ms: u32,
    pub skip_rounds_count_for_collator_rotation: u32,
    pub max_precollated_blocks: u32,
}

impl AcceleratedConsensusConfig {
    pub fn new() -> Self {
        Self {
            enabled: true,
            failed_collation_retry_timeout_ms: 1000,
            skip_rounds_count_for_collator_rotation: 5,
            max_precollated_blocks: 10,
        }
    }
}

impl Default for AcceleratedConsensusConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl Serializable for AcceleratedConsensusConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(ACCELERATED_CONSENSUS_CONFIG_TAG)?;
        cell.append_bit_bool(self.enabled)?;
        self.failed_collation_retry_timeout_ms.write_to(cell)?;
        self.skip_rounds_count_for_collator_rotation.write_to(cell)?;
        self.max_precollated_blocks.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for AcceleratedConsensusConfig {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != ACCELERATED_CONSENSUS_CONFIG_TAG {
            fail!(Self::invalid_tag(tag as u32));
        }
        let enabled = slice.get_next_bit()?;
        let failed_collation_retry_timeout_ms = u32::construct_from(slice)?;
        let skip_rounds_count_for_collator_rotation = u32::construct_from(slice)?;
        let max_precollated_blocks = u32::construct_from(slice)?;
        Ok(Self {
            enabled,
            failed_collation_retry_timeout_ms,
            skip_rounds_count_for_collator_rotation,
            max_precollated_blocks,
        })
    }
}

// ============ ConfigParam 30: New Consensus Config (Simplex) ============

/// TL-B tags for NewConsensusConfig variants
const NEW_CONSENSUS_CONFIG_ALL_TAG: u8 = 0x10;
#[allow(dead_code)] // Used in deserialization logic - null consensus means fallback to catchain
const NULL_CONSENSUS_CONFIG_TAG: u8 = 0x20;
const SIMPLEX_CONFIG_TAG: u8 = 0x21;
const SIMPLEX_CONFIG_V2_TAG: u8 = 0x22;

/// Named noncritical consensus parameters, mirroring the C++ `NoncriticalParams` struct
/// defined via `ENUMERATE_NONCRITICAL_PARAMS` in `ton-types.h`.
///
/// All fields have concrete default values matching C++. For v1 configs the three
/// on-chain fields (`target_rate_ms`, `first_block_timeout_ms`, `max_leader_window_desync`)
/// are populated from the TL-B, the rest keep their defaults. For v2, the on-chain hashmap
/// overrides any subset of the 13 parameters.
///
/// "double" parameters (multipliers) are stored as raw `f32` bit patterns in a `u32`,
/// matching the C++ `store_double` / `read_double` convention.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoncriticalParams {
    pub target_rate_ms: u32,                            // idx 0, duration
    pub first_block_timeout_ms: u32,                    // idx 1, duration
    pub first_block_timeout_multiplier_bits: u32,       // idx 2, double (f32 bits)
    pub first_block_timeout_cap_ms: u32,                // idx 3, duration
    pub candidate_resolve_timeout_ms: u32,              // idx 4, duration
    pub candidate_resolve_timeout_multiplier_bits: u32, // idx 5, double (f32 bits)
    pub candidate_resolve_timeout_cap_ms: u32,          // idx 6, duration
    pub candidate_resolve_cooldown_ms: u32,             // idx 7, duration
    pub standstill_timeout_ms: u32,                     // idx 8, duration
    pub standstill_max_egress_bytes_per_s: u32,         // idx 9, uint32
    pub max_leader_window_desync: u32,                  // idx 10, uint32
    pub bad_signature_ban_duration_ms: u32,             // idx 11, duration
    pub candidate_resolve_rate_limit: u32,              // idx 12, uint32
    pub min_block_interval_ms: u32,                     // idx 13, duration
    pub no_empty_blocks_on_error_timeout_ms: u32,       // idx 14, duration
}

impl Default for NoncriticalParams {
    fn default() -> Self {
        Self {
            target_rate_ms: 2400,
            first_block_timeout_ms: 1000,
            first_block_timeout_multiplier_bits: (1.2f32).to_bits(),
            first_block_timeout_cap_ms: 100_000,
            candidate_resolve_timeout_ms: 1000,
            candidate_resolve_timeout_multiplier_bits: (1.2f32).to_bits(),
            candidate_resolve_timeout_cap_ms: 10_000,
            candidate_resolve_cooldown_ms: 10,
            standstill_timeout_ms: 10_000,
            standstill_max_egress_bytes_per_s: 50 << 17,
            max_leader_window_desync: 250,
            bad_signature_ban_duration_ms: 5_000,
            candidate_resolve_rate_limit: 10,
            min_block_interval_ms: 0,
            no_empty_blocks_on_error_timeout_ms: 15_000,
        }
    }
}

impl NoncriticalParams {
    /// Set a parameter by its on-chain hashmap index.
    pub fn set(&mut self, idx: u8, value: u32) {
        match idx {
            0 => self.target_rate_ms = value,
            1 => self.first_block_timeout_ms = value,
            2 => self.first_block_timeout_multiplier_bits = value,
            3 => self.first_block_timeout_cap_ms = value,
            4 => self.candidate_resolve_timeout_ms = value,
            5 => self.candidate_resolve_timeout_multiplier_bits = value,
            6 => self.candidate_resolve_timeout_cap_ms = value,
            7 => self.candidate_resolve_cooldown_ms = value,
            8 => self.standstill_timeout_ms = value,
            9 => self.standstill_max_egress_bytes_per_s = value,
            10 => self.max_leader_window_desync = value,
            11 => self.bad_signature_ban_duration_ms = value,
            12 => self.candidate_resolve_rate_limit = value,
            13 => self.min_block_interval_ms = value,
            14 => self.no_empty_blocks_on_error_timeout_ms = value,
            _ => {}
        }
    }

    /// Construct from a raw hashmap (as stored on-chain in simplex_config_v2).
    pub fn from_raw_map(map: &BTreeMap<u8, u32>) -> Self {
        let mut p = Self::default();
        for (&k, &v) in map {
            p.set(k, v);
        }
        p
    }

    /// Convert all fields to a raw hashmap for on-chain v2 serialization.
    pub fn to_raw_map(&self) -> BTreeMap<u8, u32> {
        BTreeMap::from([
            (0, self.target_rate_ms),
            (1, self.first_block_timeout_ms),
            (2, self.first_block_timeout_multiplier_bits),
            (3, self.first_block_timeout_cap_ms),
            (4, self.candidate_resolve_timeout_ms),
            (5, self.candidate_resolve_timeout_multiplier_bits),
            (6, self.candidate_resolve_timeout_cap_ms),
            (7, self.candidate_resolve_cooldown_ms),
            (8, self.standstill_timeout_ms),
            (9, self.standstill_max_egress_bytes_per_s),
            (10, self.max_leader_window_desync),
            (11, self.bad_signature_ban_duration_ms),
            (12, self.candidate_resolve_rate_limit),
            (13, self.min_block_interval_ms),
            (14, self.no_empty_blocks_on_error_timeout_ms),
        ])
    }
}

/// Unified Simplex consensus config — the single output type
/// produced by deserializing either `simplex_config#21` (v1) or
/// `simplex_config_v2#22` (v2) from ConfigParam 30.
///
/// Mirrors C++ `NewConsensusConfig` in `ton-types.h`: critical fields
/// (`use_quic`, `slots_per_leader_window`) live at top level; all tunable
/// timing/rate parameters live inside `noncritical_params`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimplexConfig {
    pub use_quic: bool,
    pub slots_per_leader_window: u32,
    pub noncritical_params: NoncriticalParams,
}

impl Default for SimplexConfig {
    fn default() -> Self {
        Self {
            use_quic: false,
            slots_per_leader_window: 4,
            noncritical_params: NoncriticalParams::default(),
        }
    }
}

/// Maximum noncritical param key defined in the C++ reference.
const NONCRITICAL_PARAMS_MAX_KEY: u8 = 14;

/// Always serializes as simplex_config_v2#22 (the current on-chain format).
impl Serializable for SimplexConfig {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(SIMPLEX_CONFIG_V2_TAG)?;
        let flags_byte = if self.use_quic { 1u8 } else { 0u8 };
        cell.append_u8(flags_byte)?;
        self.slots_per_leader_window.write_to(cell)?;
        let raw_map = self.noncritical_params.to_raw_map();
        let mut params_hashmap = HashmapE::with_bit_len(8);
        for (&key, &value) in &raw_map {
            let key_slice = SliceData::from_raw(vec![key], 8);
            let mut vb = BuilderData::new();
            value.write_to(&mut vb)?;
            params_hashmap.set(key_slice, &SliceData::load_builder(vb)?)?;
        }
        params_hashmap.write_hashmap_data(cell)?;
        Ok(())
    }
}

/// Deserializes both simplex_config#21 (v1) and simplex_config_v2#22 (v2).
impl Deserializable for SimplexConfig {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        match tag {
            SIMPLEX_CONFIG_TAG => {
                let flags_byte = slice.get_next_byte()?;
                let use_quic = (flags_byte & 1) != 0;
                let target_rate_ms = u32::construct_from(slice)?;
                let slots_per_leader_window = u32::construct_from(slice)?;
                let first_block_timeout_ms = u32::construct_from(slice)?;
                let max_leader_window_desync = u32::construct_from(slice)?;
                Ok(Self {
                    use_quic,
                    slots_per_leader_window,
                    noncritical_params: NoncriticalParams {
                        target_rate_ms,
                        first_block_timeout_ms,
                        max_leader_window_desync,
                        ..Default::default()
                    },
                })
            }
            SIMPLEX_CONFIG_V2_TAG => {
                let flags_byte = slice.get_next_byte()?;
                let use_quic = (flags_byte & 1) != 0;
                let slots_per_leader_window = u32::construct_from(slice)?;
                let has_params = slice.get_next_bit()?;
                let params_cell =
                    if has_params { Some(slice.checked_drain_reference()?) } else { None };
                let params_map = HashmapE::with_hashmap(8, params_cell);
                let mut raw = BTreeMap::new();
                for key_idx in 0..=NONCRITICAL_PARAMS_MAX_KEY {
                    let key = SliceData::from_raw(vec![key_idx], 8);
                    if let Some(mut vs) = params_map.get(key)? {
                        raw.insert(key_idx, u32::construct_from(&mut vs)?);
                    }
                }
                Ok(Self {
                    use_quic,
                    slots_per_leader_window,
                    noncritical_params: NoncriticalParams::from_raw_map(&raw),
                })
            }
            _ => fail!(Self::invalid_tag(tag as u32)),
        }
    }
}

/// NewConsensusConfigAll - ConfigParam 30 wrapper
/// Contains optional configs for masterchain and shards.
///
/// TL-B: new_consensus_config_all#10 mc:(Maybe ^NewConsensusConfig)
///       shard:(Maybe ^NewConsensusConfig) = NewConsensusConfigAll;
///       _ NewConsensusConfigAll = ConfigParam 30;
///
/// Note: If the inner config is null_consensus_config#20, we store None
/// (Rust only supports SimplexConfig; null consensus falls back to catchain)
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NewConsensusConfigAll {
    pub mc: Option<SimplexConfig>,
    pub shard: Option<SimplexConfig>,
}

impl Serializable for NewConsensusConfigAll {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(NEW_CONSENSUS_CONFIG_ALL_TAG)?;
        // mc:(Maybe ^NewConsensusConfig)
        if let Some(ref cfg) = self.mc {
            cell.append_bit_one()?;
            cell.checked_append_reference(cfg.serialize()?)?;
        } else {
            cell.append_bit_zero()?;
        }
        // shard:(Maybe ^NewConsensusConfig)
        if let Some(ref cfg) = self.shard {
            cell.append_bit_one()?;
            cell.checked_append_reference(cfg.serialize()?)?;
        } else {
            cell.append_bit_zero()?;
        }
        Ok(())
    }
}

impl Deserializable for NewConsensusConfigAll {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != NEW_CONSENSUS_CONFIG_ALL_TAG {
            fail!(Self::invalid_tag(tag as u32));
        }
        let mut result = Self::default();
        // mc:(Maybe ^NewConsensusConfig)
        if slice.get_next_bit()? {
            let cell = slice.checked_drain_reference()?;
            let mut inner = SliceData::load_cell(cell)?;
            let inner_tag = inner.clone().get_next_byte()?;
            if inner_tag == SIMPLEX_CONFIG_TAG || inner_tag == SIMPLEX_CONFIG_V2_TAG {
                result.mc = Some(SimplexConfig::construct_from(&mut inner)?);
            }
            // else null_consensus_config#20 or unknown → None (catchain fallback)
        }
        // shard:(Maybe ^NewConsensusConfig)
        if slice.get_next_bit()? {
            let cell = slice.checked_drain_reference()?;
            let mut inner = SliceData::load_cell(cell)?;
            let inner_tag = inner.clone().get_next_byte()?;
            if inner_tag == SIMPLEX_CONFIG_TAG || inner_tag == SIMPLEX_CONFIG_V2_TAG {
                result.shard = Some(SimplexConfig::construct_from(&mut inner)?);
            }
        }
        Ok(result)
    }
}

impl Default for JettonBridgeParams {
    fn default() -> Self {
        Self {
            prices: JettonBridgePrices::default(),
            oracles: Oracles::default(),
            state_flags: 0,
            bridge_address: [0; 32].into(),
            oracles_address: [0; 32].into(),
            external_chain_address: [0; 32].into(),
        }
    }
}

const JETTON_BRIDGE_PARAMS_TAG_V0: u8 = 0x00;
const JETTON_BRIDGE_PARAMS_TAG_V1: u8 = 0x01;

impl Deserializable for JettonBridgeParams {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_byte()?;
        if tag != JETTON_BRIDGE_PARAMS_TAG_V0 && tag != JETTON_BRIDGE_PARAMS_TAG_V1 {
            fail!(Self::invalid_tag(tag as u32))
        }
        let bridge_address = AccountId::construct_from(slice)?;
        let oracles_address = AccountId::construct_from(slice)?;
        let oracles = Oracles::construct_from(slice)?;
        let state_flags = slice.get_next_byte()?;
        if tag == JETTON_BRIDGE_PARAMS_TAG_V0 {
            let bridge_burn_fee = Coins::construct_from(slice)?;
            return Ok(Self {
                bridge_address,
                oracles_address,
                oracles,
                state_flags,
                prices: JettonBridgePrices { bridge_burn_fee, ..Default::default() },
                external_chain_address: AccountId::default(),
            });
        }
        let prices = JettonBridgePrices::construct_from_reference(slice)?;
        let external_chain_address = AccountId::construct_from(slice)?;
        Ok(Self {
            bridge_address,
            oracles_address,
            oracles,
            state_flags,
            prices,
            external_chain_address,
        })
    }
}

impl Serializable for JettonBridgeParams {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(JETTON_BRIDGE_PARAMS_TAG_V1)?;
        self.bridge_address.write_to(cell)?;
        self.oracles_address.write_to(cell)?;
        self.oracles.write_to(cell)?;
        self.state_flags.write_to(cell)?;
        let cell1 = self.prices.serialize()?;
        cell.checked_append_reference(cell1)?;
        self.external_chain_address.write_to(cell)?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn dump_config(params: &HashmapE) {
    params
        .iterate_slices(|ref mut key, ref mut slice| -> Result<bool> {
            let key = key.get_next_u32()?;
            match ConfigParamEnum::construct_from_cell_and_number(slice.reference(0)?, key)? {
                ConfigParamEnum::ConfigParam31(ref mut cfg) => {
                    println!("\tConfigParam31.fundamental_smc_addr");
                    cfg.fundamental_smc_addr.iterate_keys(|addr: AccountId| -> Result<bool> {
                        println!("\t\t{:x}", addr);
                        Ok(true)
                    })?;
                }
                ConfigParamEnum::ConfigParam34(ref mut cfg) => {
                    println!("\tConfigParam34.cur_validators");
                    for validator in cfg.cur_validators.list() {
                        println!("\t\t{:?}", validator);
                    }
                }
                x => println!("\t{:?}", x),
            }
            Ok(true)
        })
        .unwrap();
}
