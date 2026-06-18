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
use num::BigInt;
use ton_block::{
    fail, AccountId, BurningConfig, Coins, ConfigParam18, ConfigParamEnum, ConfigParams,
    FundamentalSmcAddresses, GasLimitsPrices, GlobalCapabilities, Mask, MsgAddressInt,
    MsgForwardPrices, Result, SizeLimitsConfig, StorageInfo, StoragePrices, UInt256,
    SUPPORTED_VERSION,
};

pub(crate) trait DefaultConfig {
    /// Get default value for masterchain
    fn default_mc() -> Self;
    /// Get default value for workchains
    fn default_wc() -> Self;
}

impl DefaultConfig for MsgForwardPrices {
    fn default_mc() -> Self {
        MsgForwardPrices {
            lump_price: 10000000,
            bit_price: 655360000,
            cell_price: 65536000000,
            ihr_price_factor: 98304,
            first_frac: 21845,
            next_frac: 21845,
        }
    }

    fn default_wc() -> Self {
        MsgForwardPrices {
            lump_price: 1000000,
            bit_price: 65536000,
            cell_price: 6553600000,
            ihr_price_factor: 98304,
            first_frac: 21845,
            next_frac: 21845,
        }
    }
}

#[derive(Clone)]
pub struct AccStoragePrices {
    prices: Vec<StoragePrices>,
}

impl Default for AccStoragePrices {
    fn default() -> Self {
        AccStoragePrices {
            prices: vec![StoragePrices {
                utime_since: 0,
                bit_price_ps: 1,
                cell_price_ps: 500,
                mc_bit_price_ps: 1000,
                mc_cell_price_ps: 500000,
            }],
        }
    }
}

impl AccStoragePrices {
    /// Calculate storage fee for provided data
    pub fn calc_storage_fees(
        &self,
        cells: u64,
        bits: u64,
        mut last_paid: u32,
        now: u32,
        is_masterchain: bool,
    ) -> Result<u128> {
        if now <= last_paid
            || last_paid == 0
            || self.prices.is_empty()
            || now <= self.prices[0].utime_since
        {
            return Ok(0);
        }
        let mut fee = BigInt::default();
        // storage prices config contains prices array for some time intervals
        // to calculate account storage fee we need to sum fees for all intervals since last
        // storage fee pay calculated by formula `(cells * cell_price + bits * bits_price) * interval`
        for i in 0..self.prices.len() {
            let prices = &self.prices[i];
            let end = if i < self.prices.len() - 1 { self.prices[i + 1].utime_since } else { now };

            if end >= last_paid {
                let delta = end - prices.utime_since.max(last_paid);
                fee += prices.calc_storage_fee_part(cells, bits, delta as u64, is_masterchain);
                last_paid = end;
            }
        }
        fee = (fee + 0xffffu32) >> 16;
        Ok(fee.try_into()?)
    }

    fn with_config(config: &ConfigParam18) -> Result<Self> {
        let prices = config.prices()?;
        Ok(AccStoragePrices { prices })
    }
}

impl DefaultConfig for GasLimitsPrices {
    fn default_mc() -> Self {
        GasLimitsPrices {
            gas_price: 655360000,
            flat_gas_limit: 100,
            flat_gas_price: 1000000,
            gas_limit: 1000000,
            special_gas_limit: 10000000,
            gas_credit: 10000,
            block_gas_limit: 10000000,
            freeze_due_limit: 100000000,
            delete_due_limit: 1000000000,
            max_gas_threshold: 10000000000,
        }
    }

    fn default_wc() -> Self {
        GasLimitsPrices {
            gas_price: 65536000,
            flat_gas_limit: 100,
            flat_gas_price: 100000,
            gas_limit: 1000000,
            special_gas_limit: 1000000,
            gas_credit: 10000,
            block_gas_limit: 10000000,
            freeze_due_limit: 100000000,
            delete_due_limit: 1000000000,
            max_gas_threshold: 1000000000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_storage_fees_rounds_after_summing_intervals() {
        let prices = AccStoragePrices {
            prices: vec![
                StoragePrices {
                    utime_since: 1,
                    bit_price_ps: 0,
                    cell_price_ps: 1,
                    mc_bit_price_ps: 0,
                    mc_cell_price_ps: 1,
                },
                StoragePrices {
                    utime_since: 2,
                    bit_price_ps: 0,
                    cell_price_ps: 1,
                    mc_bit_price_ps: 0,
                    mc_cell_price_ps: 1,
                },
            ],
        };

        let fee = prices.calc_storage_fees(1, 0, 1, 65_537, false).unwrap();

        assert_eq!(fee, 1);
    }
}

/// Blockchain configuration parameters
#[derive(Clone)]
pub struct BlockchainConfig {
    gas_prices_mc: GasLimitsPrices,
    gas_prices_wc: GasLimitsPrices,
    fwd_prices_mc: MsgForwardPrices,
    fwd_prices_wc: MsgForwardPrices,
    storage_prices: AccStoragePrices,
    burning_cfg: Option<BurningConfig>,
    special_contracts: FundamentalSmcAddresses,
    limits: SizeLimitsConfig,
    capabilities: u64,
    global_version: u32,
    raw_config: ConfigParams,
    pub deferring_enabled: bool,
}

impl Default for BlockchainConfig {
    fn default() -> Self {
        let capabilities = 0x1ee;
        BlockchainConfig {
            gas_prices_mc: GasLimitsPrices::default_mc(),
            gas_prices_wc: GasLimitsPrices::default_wc(),
            fwd_prices_mc: MsgForwardPrices::default_mc(),
            fwd_prices_wc: MsgForwardPrices::default_wc(),
            storage_prices: AccStoragePrices::default(),
            burning_cfg: None,
            special_contracts: Self::get_default_special_contracts(),
            limits: Default::default(),
            raw_config: Self::get_defult_raw_config(),
            global_version: SUPPORTED_VERSION,
            capabilities,
            deferring_enabled: capabilities.bit(GlobalCapabilities::CapDeferMessages as u64),
        }
    }
}

impl BlockchainConfig {
    fn get_default_special_contracts() -> FundamentalSmcAddresses {
        let mut map = FundamentalSmcAddresses::default();
        map.add_key(&UInt256::with_array([0x33u8; 32])).unwrap();
        map.add_key(&UInt256::with_array([0x66u8; 32])).unwrap();
        map.add_key(
            &"34517C7BDF5187C55AF4F8B61FDC321588C7AB768DEE24B006DF29106458D7CF"
                .parse::<UInt256>()
                .unwrap(),
        )
        .unwrap();
        map
    }

    fn get_defult_raw_config() -> ConfigParams {
        ConfigParams { config_addr: [0x55; 32].into(), ..ConfigParams::default() }
    }

    /// Create `BlockchainConfig` struct with `ConfigParams` taken from blockchain
    pub fn with_config(config: ConfigParams) -> Result<Self> {
        Self::with_params(config.capabilities(), config.global_version(), config)
    }

    /// Create `BlockchainConfig` struct with `ConfigParams` taken from blockchain
    pub fn with_params(
        capabilities: u64,
        global_version: u32,
        config: ConfigParams,
    ) -> Result<Self> {
        log::debug!(
            "Creating BlockchainConfig: capabilities={capabilities:#x}, block_version={global_version}"
        );
        let burning_cfg = match config.config(5)? {
            Some(ConfigParamEnum::ConfigParam5(burning_cfg)) => Some(burning_cfg),
            _ => None,
        };
        Ok(BlockchainConfig {
            gas_prices_mc: config.gas_prices(true)?,
            gas_prices_wc: config.gas_prices(false)?,
            fwd_prices_mc: config.fwd_prices(true)?,
            fwd_prices_wc: config.fwd_prices(false)?,
            storage_prices: AccStoragePrices::with_config(&config.storage_prices()?)?,
            burning_cfg,
            limits: config.size_limits_config()?,
            special_contracts: config.fundamental_smc_addr()?,
            capabilities,
            global_version,
            raw_config: config,
            deferring_enabled: capabilities.bit(GlobalCapabilities::CapDeferMessages as u64),
        })
    }

    pub fn global_version(&self) -> u32 {
        self.global_version
    }

    /// Get `MsgForwardPrices` for message forward fee calculation
    pub fn get_fwd_prices(&self, is_masterchain: bool) -> &MsgForwardPrices {
        if is_masterchain {
            &self.fwd_prices_mc
        } else {
            &self.fwd_prices_wc
        }
    }

    pub fn size_limits_config(&self) -> &SizeLimitsConfig {
        &self.limits
    }

    pub fn burning_config(&self) -> Option<&BurningConfig> {
        self.burning_cfg.as_ref()
    }

    /// Calculate gas fee for account
    pub fn calc_gas_fee(&self, gas_used: u64, address: &MsgAddressInt) -> u128 {
        self.get_gas_config(address.is_masterchain()).calc_gas_fee(gas_used)
    }

    /// Get `GasLimitsPrices` for account gas fee calculation
    pub fn get_gas_config(&self, is_masterchain: bool) -> &GasLimitsPrices {
        if is_masterchain {
            &self.gas_prices_mc
        } else {
            &self.gas_prices_wc
        }
    }

    /// Calculate account storage fee
    pub fn calc_storage_fees(
        &self,
        storage: &StorageInfo,
        is_masterchain: bool,
        now: u32,
    ) -> Result<Coins> {
        let storage_fee = self.storage_prices.calc_storage_fees(
            storage.used().cells(),
            storage.used().bits(),
            storage.last_paid(),
            now,
            is_masterchain,
        )?;
        Coins::try_from(storage_fee)
    }

    /// Check if account is special account
    pub fn is_special_account(&self, is_masterchain: bool, account_id: &AccountId) -> Result<bool> {
        if is_masterchain {
            // special account adresses are stored in hashmap
            // config account is special too
            Ok(&self.raw_config.config_addr == account_id
                || self.special_contracts.get(account_id)?.is_some())
        } else {
            Ok(false)
        }
    }

    pub fn block_version(&self) -> u32 {
        self.global_version
    }

    pub fn raw_config(&self) -> &ConfigParams {
        &self.raw_config
    }

    pub fn has_capability(&self, capability: GlobalCapabilities) -> bool {
        (self.capabilities & (capability as u64)) != 0
    }

    pub fn capabilites(&self) -> u64 {
        self.capabilities
    }

    pub(crate) fn check_fixed_prefix_length(&self, fixed_prefix_length: u32) -> Result<()> {
        if fixed_prefix_length > self.limits.max_acc_fixed_prefix_length as u32 {
            fail!(
                "fixed prefix length {fixed_prefix_length} more than {}",
                self.limits.max_acc_fixed_prefix_length
            )
        }
        Ok(())
    }
}
