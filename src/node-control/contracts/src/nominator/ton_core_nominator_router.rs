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
    ton_core_nominator::TonCoreNominatorWrapper,
};
use crate::{ContractProvider, SmartContract, TonWallet};
use anyhow::Context;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use ton_block::{Cell, MsgAddressInt, StateInit};

/// TONCore nominator binding: two pool contracts (even/odd validation rounds).
///
/// Implements [`NominatorWrapper`] so call sites keep a single `Arc<dyn NominatorWrapper>` per
/// node; use [`NominatorWrapper::inner_pools`] to iterate both contracts (deploy, RPC).
pub struct TonCoreNominatorRouter {
    pools: [Option<Arc<dyn NominatorWrapper>>; 2],
    /// Current election id; pins active-slot selection to the cycle being serviced.
    /// `0` means "not set" — falls back to legacy state-based selection (used by non-election
    /// callers like the HTTP config handler that share the router via `Arc`). The election
    /// runner short-circuits at `election_id == 0`, so `0` is never a real cycle id and the
    /// sentinel is unambiguous.
    current_election_id: AtomicU64,
}

impl TonCoreNominatorRouter {
    /// Resolve the active slot for the current election cycle.
    ///
    /// Rules (applied in order):
    /// 1. `election_id` set and one of the pools has `stake_at == election_id` → that pool.
    ///    `pool.fc` writes `stake_at` immediately on `op::new_stake` (before the elector
    ///    reply), so this match remains valid across the in-cycle `state` transitions
    ///    (0→1→2): once the runner submits a bid, every subsequent call returns the same slot.
    /// 2. Otherwise return the first pool that is either idle (`state == 0`) or has finished
    ///    its previous cycle and is ready to be recovered (`state == 2 && vsc >= 2`). This
    ///    branch is also the fallback for non-election callers (where `election_id` is unset).
    /// 3. Neither rule matches → no pool is ready; error.
    async fn active_pool(&self) -> anyhow::Result<Arc<dyn NominatorWrapper>> {
        let election_id = self.current_election_id.load(Ordering::Relaxed);

        let mut entries: Vec<(Arc<dyn NominatorWrapper>, PoolData)> = Vec::with_capacity(2);
        for pool in self.pools.iter().flatten() {
            let data = pool.get_pool_data().await.context("get_pool_data failed")?;
            entries.push((pool.clone(), data));
        }
        if entries.is_empty() {
            anyhow::bail!("no pools configured");
        }

        if election_id != 0
            && let Some((pool, _)) = entries.iter().find(|(_, d)| d.stake_at as u64 == election_id)
        {
            return Ok(pool.clone());
        }
        if let Some((pool, _)) = entries
            .iter()
            .find(|(_, d)| d.state == 0 || (d.state == 2 && d.validator_set_changes_count >= 2))
        {
            return Ok(pool.clone());
        }
        anyhow::bail!(
            "no pool ready: none matches stake_at={election_id} and none is idle/recoverable",
        );
    }

    pub fn new(provider: Arc<dyn ContractProvider>, pools: [Option<MsgAddressInt>; 2]) -> Self {
        let pools: [Option<Arc<dyn NominatorWrapper>>; 2] = pools.map(|addr| {
            addr.map(|addr| {
                Arc::new(TonCoreNominatorWrapper::new(provider.clone(), addr))
                    as Arc<dyn NominatorWrapper>
            })
        });
        Self { pools, current_election_id: AtomicU64::new(0) }
    }

    /// Build a router from optional per-slot pool wrappers (e.g. from config).
    pub fn from_wrappers(pools: [Option<Arc<dyn NominatorWrapper>>; 2]) -> Self {
        Self { pools, current_election_id: AtomicU64::new(0) }
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
    fn set_election_id(&self, election_id: u64) {
        self.current_election_id.store(election_id, Ordering::Relaxed);
    }

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

    /// Reports the active slot's queue only — `send_process_withdraw_requests` would also route
    /// to that slot, so checking the inactive slot here would create a false positive that
    /// the runner could not act on.
    async fn has_withdraw_requests(&self) -> anyhow::Result<bool> {
        self.active_pool().await?.has_withdraw_requests().await
    }

    async fn send_process_withdraw_requests(
        &self,
        wallet: Arc<dyn TonWallet>,
        query_id: u64,
        limit: u8,
        gas_value: u64,
    ) -> anyhow::Result<Cell> {
        self.active_pool()
            .await?
            .send_process_withdraw_requests(wallet, query_id, limit, gas_value)
            .await
    }
}
