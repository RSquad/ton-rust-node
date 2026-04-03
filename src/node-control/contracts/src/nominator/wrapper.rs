/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::SmartContract;
use std::sync::Arc;
use ton_block::{MsgAddressInt, StateInit};

/// Trait for interacting with single-nominator smart contract
///
/// Based on https://github.com/ton-blockchain/single-nominator
///
/// The single-nominator contract provides secure validation for TON blockchain
/// by separating the owner role (cold wallet) from the validator role (hot wallet).
#[async_trait::async_trait]
pub trait NominatorWrapper: SmartContract + Send + Sync {
    /// Get the owner and validator addresses stored in the contract
    async fn get_roles(&self) -> anyhow::Result<NominatorRoles>;
    /// Get pool data (parsed persistent storage of nominator)
    async fn get_pool_data(&self) -> anyhow::Result<PoolData>;
    /// Return the state_init used for deploying this contract (if available).
    fn state_init(&self) -> Option<StateInit> {
        None
    }
}

/// Roles stored in the single nominator contract
#[derive(Debug, Clone)]
pub struct NominatorRoles {
    /// Owner address (can add or withdraw funds to/from nominator)
    pub owner_address: MsgAddressInt,
    /// Validator address (can stake or recover funds to/from elector)
    pub validator_address: MsgAddressInt,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolConfig {
    pub validator_addr: [u8; 32],
    pub validator_reward_share: u16,
    pub max_nominators_count: u16,
    pub min_validator_stake: u64,
    /// SNP: max nominator stake; TONCore: min nominator stake.
    pub nominator_stake_threshold: u64,
}
/// Pool data returned by get_pool_data()
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolData {
    /// Pool state (2 = funds staked at elector)
    pub state: i32,
    /// Number of nominators (always 1 for single nominator)
    pub nominators_count: u32,
    /// Stake amount sent (always 0 for single nominator)
    pub stake_amount_sent: u64,
    /// Validator amount (always 0 for single nominator)
    pub validator_amount: u64,
    /// Pool config
    pub pool_config: PoolConfig,
    /// Elections Id
    pub stake_at: u32,
    /// Saved validator set hash from config 34
    pub saved_validator_set_hash: [u8; 32],
    /// Validator set changes count (2 = funds staked at elector)
    pub validator_set_changes_count: i32,
    /// Validator set change time
    pub validator_set_change_time: u64,
    /// Stake held for duration
    pub stake_held_for: u64,
}

/// Pool binding for a single node: either one pool or two with routing.
#[derive(Clone)]
pub enum NodePools {
    /// SNP or TONCore — a single nominator pool.
    Single(Arc<dyn NominatorWrapper>),
    /// TONCoreRouter — two pools; the runner picks the free one via `get_pool_data().state`.
    Router([Arc<dyn NominatorWrapper>; 2]),
}

impl NodePools {
    /// Primary pool (pool[0]). Used for address display and as the default staking address.
    pub fn primary(&self) -> &Arc<dyn NominatorWrapper> {
        match self {
            NodePools::Single(p) => p,
            NodePools::Router([p, _]) => p,
        }
    }

    /// All pools (1 for Single, 2 for Router).
    pub fn all(&self) -> Vec<&Arc<dyn NominatorWrapper>> {
        match self {
            NodePools::Single(p) => vec![p],
            NodePools::Router([a, b]) => vec![a, b],
        }
    }

    /// Select the pool that is ready for validation (`state == 0`).
    /// For `Single` — always returns the only pool.
    /// For `Router` — queries `get_pool_data()` on each pool, returns the first with `state == 0`.
    pub async fn select_free(&self) -> anyhow::Result<&Arc<dyn NominatorWrapper>> {
        match self {
            NodePools::Single(p) => Ok(p),
            NodePools::Router(pools) => {
                for pool in pools {
                    let data = pool.get_pool_data().await?;
                    if data.state == 0 {
                        return Ok(pool);
                    }
                }
                anyhow::bail!("all router pools are busy (state != 0)")
            }
        }
    }
}
