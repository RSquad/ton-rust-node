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

use super::{
    NominatorRoles, NominatorWrapper, PoolData, PoolKind, TONCORE_STORAGE_RESERVE,
    ton_core_nominator::{NominatorPoolWrapperImpl, toncore_pool_address_and_state},
};
use crate::{ContractProvider, SmartContract};
use common::app_config::TonCoreInitParams;
use std::sync::Arc;
use ton_block::{MsgAddressInt, StateInit};

/// TONCore nominator binding: two pool contracts (even/odd validation rounds).
///
/// Implements [`NominatorWrapper`] so call sites keep a single `Arc<dyn NominatorWrapper>` per
/// node; use [`NominatorWrapper::inner_pools`] to iterate both contracts (deploy, RPC).
pub struct TonCoreNominatorRouter {
    pools: [Option<Arc<dyn NominatorWrapper>>; 2],
}

impl TonCoreNominatorRouter {
    async fn active_pool(&self) -> Option<Arc<dyn NominatorWrapper>> {
        // TONCore: pick the first pool that is not currently staking.
        for pool in self.pools.iter().flatten() {
            match pool.get_pool_data().await {
                Ok(data) => {
                    let is_free = data.state == 0
                        || (data.state == 2 && data.validator_set_changes_count >= 2);
                    if is_free {
                        return Some(pool.clone());
                    }
                }
                Err(e) => {
                    eprintln!("TonCoreNominatorRouter: ERROR failed to resolve active pool: {e:#}");
                }
            }
        }

        if let Some(pool) = self.pools.iter().flatten().next().cloned() {
            eprintln!(
                "TonCoreNominatorRouter: no active pool found, fallback to first configured pool"
            );
            return Some(pool);
        }

        None
    }

    pub fn new(provider: Arc<dyn ContractProvider>, pools: [Option<MsgAddressInt>; 2]) -> Self {
        let pools: [Option<Arc<dyn NominatorWrapper>>; 2] = pools.map(|addr| {
            addr.map(|addr| {
                Arc::new(NominatorPoolWrapperImpl::new(provider.clone(), addr))
                    as Arc<dyn NominatorWrapper>
            })
        });
        Self { pools }
    }

    pub fn from_state_init(
        provider: Arc<dyn ContractProvider>,
        pools: [Option<TonCoreInitParams>; 2],
        validator_address: &MsgAddressInt,
    ) -> anyhow::Result<Self> {
        let pools = pools.map(|slot| -> anyhow::Result<Option<Arc<dyn NominatorWrapper>>> {
            let Some(init_params) = slot else {
                return Ok(None);
            };
            let (addr, si) = toncore_pool_address_and_state(init_params, validator_address)?;
            Ok(Some(Arc::new(NominatorPoolWrapperImpl::new_with_state_init(
                provider.clone(),
                addr,
                si,
            )) as Arc<dyn NominatorWrapper>))
        });
        let [p0, p1] = pools;
        Ok(Self { pools: [p0?, p1?] })
    }

    /// Build a router from optional per-slot pool wrappers (e.g. from config).
    pub fn from_wrappers(pools: [Option<Arc<dyn NominatorWrapper>>; 2]) -> Self {
        Self { pools }
    }
}

#[async_trait::async_trait]
impl SmartContract for TonCoreNominatorRouter {
    async fn balance(&self) -> anyhow::Result<u64> {
        if let Some(pool) = self.active_pool().await {
            return pool.balance().await;
        }
        anyhow::bail!("TonCoreNominatorRouter: no pools configured")
    }

    async fn address(&self) -> MsgAddressInt {
        if let Some(pool) = self.active_pool().await {
            return pool.address().await;
        }
        eprintln!("TonCoreNominatorRouter: ERROR no pools configured");
        MsgAddressInt::default()
    }
}

#[async_trait::async_trait]
impl NominatorWrapper for TonCoreNominatorRouter {
    fn pool_kind(&self) -> PoolKind {
        PoolKind::TONCore
    }

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
        if let Some(pool) = self.active_pool().await {
            return pool.get_pool_data().await;
        }
        anyhow::bail!("TonCoreNominatorRouter: no pools configured")
    }

    fn inner_pools(&self) -> Vec<Arc<dyn NominatorWrapper>> {
        self.pools.iter().flatten().cloned().collect()
    }
}
