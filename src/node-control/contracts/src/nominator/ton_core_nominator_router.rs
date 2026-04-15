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
use anyhow::Context;
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
    async fn active_pool(&self) -> anyhow::Result<Arc<dyn NominatorWrapper>> {
        // TONCore: pick the first pool that is not currently staking or is ready to recover stake.
        for pool in self.pools.iter().flatten() {
            let data = pool.get_pool_data().await.context("get_pool_data failed")?;
            if data.state == 0 // pool is not staking
            // pool has sent stake (state=2) earlier and is ready to recover it (validator_set_changes_count >= 2)
                || (data.state == 2 && data.validator_set_changes_count >= 2)
            {
                return Ok(pool.clone());
            }
        }
        if self.pools.iter().any(|p| p.is_some()) {
            anyhow::bail!("no one pool is ready");
        }
        anyhow::bail!("no pools configured")
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
            let (addr, si) = toncore_pool_address_and_state(&init_params, validator_address)?;
            Ok(Some(Arc::new(NominatorPoolWrapperImpl::new_with_state_init(
                provider.clone(),
                addr,
                si,
            )) as Arc<dyn NominatorWrapper>))
        });
        let [p0, p1] = pools;
        Ok(Self { pools: [p0.context("slot 0")?, p1.context("slot 1")?] })
    }

    /// Build a router from optional per-slot pool wrappers (e.g. from config).
    pub fn from_wrappers(pools: [Option<Arc<dyn NominatorWrapper>>; 2]) -> Self {
        Self { pools }
    }
}

#[async_trait::async_trait]
impl SmartContract for TonCoreNominatorRouter {
    async fn balance(&self) -> anyhow::Result<u64> {
        self.active_pool().await?.balance().await
    }

    async fn address(&self) -> anyhow::Result<MsgAddressInt> {
        self.active_pool().await?.address().await
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
        self.active_pool().await?.get_roles().await
    }

    /// Returns pool data for the first configured pool. For per-pool data use
    /// [`inner_pools`](NominatorWrapper::inner_pools) and query each pool individually.
    async fn get_pool_data(&self) -> anyhow::Result<PoolData> {
        self.active_pool().await?.get_pool_data().await
    }

    fn inner_pools(&self) -> Vec<Arc<dyn NominatorWrapper>> {
        self.pools.iter().flatten().cloned().collect()
    }
}
