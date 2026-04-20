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

/// Minimum TON to keep in an SNP pool (or validator wallet for direct staking) for storage.
/// Matches the `MIN_TONS_FOR_STORAGE` constant in the single-nominator contract (~1 TON).
pub const SNP_STORAGE_RESERVE: u64 = 1_000_000_000;
/// Minimum TON to keep in a TONCore nominator pool for storage.
/// Matches `MIN_TONS_FOR_STORAGE` in pool.fc (10 TON).
pub const TONCORE_STORAGE_RESERVE: u64 = 10_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolKind {
    SNP,
    TONCore,
}

/// Trait for interacting with single-nominator or TONCore nominator pool contracts.
///
/// Based on https://github.com/ton-blockchain/single-nominator
///
/// TONCore nominator with two pools uses [`crate::nominator::TonCoreNominatorRouter`], which
/// implements this trait. Use [`inner_pools`](NominatorWrapper::inner_pools) to iterate the
/// physical pool contracts (deploy, RPC). SNP returns `[self]`; single TONCore returns an empty
/// vec (the pool itself is the only physical contract).
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
    /// Physical sub-pool contracts for deploy and RPC.
    ///
    /// Returns the two on-chain [`NominatorWrapper`] contracts for a
    /// [`TonCoreNominatorRouter`](crate::nominator::TonCoreNominatorRouter), `[self]` for
    /// [`SingleNominatorWrapper`](crate::nominator::SingleNominatorWrapper) (preserves state_init
    /// for deploy), or an empty `Vec` for a single TONCore pool.
    fn inner_pools(&self) -> Vec<Arc<dyn NominatorWrapper>>;
    /// Minimum nanotons that must remain in the staking account after a stake withdrawal
    /// (contract storage reserve). SNP = [`SNP_STORAGE_RESERVE`]; TONCore = [`TONCORE_STORAGE_RESERVE`].
    fn storage_reserve(&self) -> u64;
    /// Pool type for routing/optimization decisions.
    fn pool_kind(&self) -> PoolKind;
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
