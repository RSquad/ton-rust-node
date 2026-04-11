/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! TONCore nominator setup with two on-chain pools (even/odd validation rounds),
//! exposed as one [`NominatorWrapper`] handle. Use [`crate::nominator::nominator_constituents`]
//! to iterate both contracts for deploy and RPC.

use super::{NominatorRoles, NominatorWrapper, PoolData};
use crate::SmartContract;
use std::sync::Arc;
use ton_block::{MsgAddressInt, StateInit};

/// TONCore nominator binding: two pool contracts (even/odd validation rounds).
///
/// Implements [`NominatorWrapper`] so call sites keep a single `Arc<dyn NominatorWrapper>` per
/// node; use [`nominator_constituents`](crate::nominator::nominator_constituents) to iterate both contracts (deploy, RPC).
pub struct TonCoreNominatorPair {
    pools: [Arc<dyn NominatorWrapper>; 2],
}

impl TonCoreNominatorPair {
    #[must_use]
    pub fn new(pools: [Arc<dyn NominatorWrapper>; 2]) -> Self {
        Self { pools }
    }
}

#[async_trait::async_trait]
impl SmartContract for TonCoreNominatorPair {
    async fn balance(&self) -> anyhow::Result<u64> {
        let a = self.pools[0].balance().await?;
        let b = self.pools[1].balance().await?;
        Ok(a.saturating_add(b))
    }

    fn address(&self) -> MsgAddressInt {
        self.pools[0].address()
    }
}

#[async_trait::async_trait]
impl NominatorWrapper for TonCoreNominatorPair {
    fn is_toncore_pool(&self) -> bool {
        true
    }

    fn state_init(&self) -> Option<StateInit> {
        None
    }

    async fn get_roles(&self) -> anyhow::Result<NominatorRoles> {
        self.pools[0].get_roles().await
    }

    /// Returns pool data for `pools[0]` only. For per-pool data use
    /// [`nominator_constituents`](crate::nominator::nominator_constituents) and query each pool individually.
    async fn get_pool_data(&self) -> anyhow::Result<PoolData> {
        self.pools[0].get_pool_data().await
    }

    fn as_toncore_nominator_slots(
        &self,
    ) -> Option<(Arc<dyn NominatorWrapper>, Arc<dyn NominatorWrapper>)> {
        Some((self.pools[0].clone(), self.pools[1].clone()))
    }

    async fn resolve_staking_address(&self) -> anyhow::Result<MsgAddressInt> {
        for pool in &self.pools {
            let data = pool.get_pool_data().await?;
            if data.state == 0 {
                return Ok(pool.address());
            }
        }
        anyhow::bail!("all TONCore nominator pools are busy (state != 0)")
    }

    fn is_toncore_nominator_pair(&self) -> bool {
        true
    }
}
