/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! TONCore nominator setup with two on-chain pools (even/odd validation rounds),
//! exposed as one [`NominatorWrapper`] handle. Use [`NominatorWrapper::inner_pools`]
//! to iterate both contracts for deploy and RPC.

use super::{NominatorRoles, NominatorWrapper, PoolData, TONCORE_STORAGE_RESERVE};
use crate::SmartContract;
use std::sync::Arc;
use ton_block::{StateInit, MsgAddressInt};
use crate::ton_core_nominator::{NominatorPoolWrapperImpl, toncore_pool_address_and_state};
use crate::ContractProvider;
use common::app_config::TonCoreInitParams;

/// TONCore nominator binding: two pool contracts (even/odd validation rounds).
///
/// Implements [`NominatorWrapper`] so call sites keep a single `Arc<dyn NominatorWrapper>` per
/// node; use [`NominatorWrapper::inner_pools`] to iterate both contracts (deploy, RPC).
pub struct TonCoreNominatorRouter {
    pools: [Option<Arc<dyn NominatorWrapper>>; 2],
}

impl TonCoreNominatorRouter {
    pub fn new(provider: Arc<dyn ContractProvider>, pools: [Option<MsgAddressInt>; 2]) -> Self {
        let [a0, a1] = pools;
        Self {
            pools: [
                a0.map(|addr| {
                    Arc::new(NominatorPoolWrapperImpl::new(provider.clone(), addr))
                        as Arc<dyn NominatorWrapper>
                }),
                a1.map(|addr| {
                    Arc::new(NominatorPoolWrapperImpl::new(provider, addr))
                        as Arc<dyn NominatorWrapper>
                }),
            ],
        }
    }
    pub fn from_state_init(
        provider: Arc<dyn ContractProvider>,
        pools: [Option<(TonCoreInitParams, MsgAddressInt)>; 2],
        validator_address: &MsgAddressInt,
    ) -> anyhow::Result<Self> {
        let [p0, p1] = pools;
        let w0 = match p0 {
            Some((init_params, _)) => {
                let (addr, si) = toncore_pool_address_and_state(init_params, validator_address)?;
                Some(Arc::new(NominatorPoolWrapperImpl::new_with_state_init(
                    provider.clone(),
                    addr,
                    si,
                )) as Arc<dyn NominatorWrapper>)
            }
            None => None,
        };
        let w1 = match p1 {
            Some((init_params, _)) => {
                let (addr, si) = toncore_pool_address_and_state(init_params, validator_address)?;
                Some(Arc::new(NominatorPoolWrapperImpl::new_with_state_init(provider, addr, si))
                    as Arc<dyn NominatorWrapper>)
            }
            None => None,
        };
        Ok(Self { pools: [w0, w1] })
    }

    /// Build a router from optional per-slot pool wrappers (e.g. from config).
    pub fn from_wrappers(pools: [Option<Arc<dyn NominatorWrapper>>; 2]) -> Self {
        Self { pools }
    }
}

#[async_trait::async_trait]
impl SmartContract for TonCoreNominatorRouter {
    async fn balance(&self) -> anyhow::Result<u64> {
        let a = if let Some(p) = &self.pools[0] { p.balance().await? } else { 0 };
        let b = if let Some(p) = &self.pools[1] { p.balance().await? } else { 0 };
        Ok(a.saturating_add(b))
    }

    async fn address(&self) -> MsgAddressInt {
        for pool in &self.pools {
            if let Some(p) = pool {
                return p.address().await;
            }
        }
        panic!("TonCoreNominatorRouter: no pools configured")
    }
}

#[async_trait::async_trait]
impl NominatorWrapper for TonCoreNominatorRouter {
    fn storage_reserve(&self) -> u64 {
        TONCORE_STORAGE_RESERVE
    }

    fn state_init(&self) -> Option<StateInit> {
        None
    }

    async fn get_roles(&self) -> anyhow::Result<NominatorRoles> {
        for pool in self.pools.iter().flatten() {
            return pool.get_roles().await;
        }
        anyhow::bail!("TonCoreNominatorRouter: no pools configured")
    }

    /// Returns pool data for the first configured pool. For per-pool data use
    /// [`inner_pools`](NominatorWrapper::inner_pools) and query each pool individually.
    async fn get_pool_data(&self) -> anyhow::Result<PoolData> {
        for pool in self.pools.iter().flatten() {
            return pool.get_pool_data().await;
        }
        anyhow::bail!("TonCoreNominatorRouter: no pools configured")
    }

    fn inner_pools(&self) -> Vec<Arc<dyn NominatorWrapper>> {
        self.pools.iter().flatten().cloned().collect()
    }
}
