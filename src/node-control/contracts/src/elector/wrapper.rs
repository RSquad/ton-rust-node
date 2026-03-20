/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::SmartContract;
use std::collections::HashMap;

#[derive(Default, Debug, Clone)]
pub struct Participant {
    pub pub_key: Vec<u8>,
    pub adnl_addr: Vec<u8>,
    pub wallet_addr: Vec<u8>,
    pub stake: u64,
    pub max_factor: u32,
    pub election_id: u64,
    pub stake_message_boc: Option<Vec<u8>>,
}

impl Participant {
    pub fn with_wallet_addr(wallet_addr: Vec<u8>) -> Self {
        Self { wallet_addr, ..Default::default() }
    }
    pub fn wallet(&self) -> &[u8] {
        self.wallet_addr.as_slice()
    }
}

#[derive(Default, Debug)]
pub struct ElectionsInfo {
    pub election_id: u64,
    pub elect_close: u64,
    pub min_stake: u64,
    pub total_stake: u64,
    pub failed: bool,
    pub finished: bool,
    pub participants: Vec<Participant>,
}

pub struct FrozenParticipant {
    pub wallet_addr: [u8; 32],
    pub weight: u64,
    pub stake: u64,
    pub banned: bool,
}

pub struct PastElections {
    pub election_id: u64,
    pub unfreeze_at: u64,
    pub stake_held: u64,
    pub vset_hash: Vec<u8>,
    pub frozen_map: HashMap<[u8; 32], FrozenParticipant>,
    pub total_stake: u64,
    pub bonuses: u64,
}

#[async_trait::async_trait]
pub trait ElectorWrapper: SmartContract + Send + Sync {
    async fn get_active_election_id(&self) -> anyhow::Result<u64>;
    async fn participates_in(&self, pubkey: &[u8]) -> anyhow::Result<Option<Participant>>;
    async fn compute_returned_stake(&self, address: &[u8]) -> anyhow::Result<u64>;
    async fn elections_info(&self) -> anyhow::Result<ElectionsInfo>;
    async fn past_elections(&self) -> anyhow::Result<Vec<PastElections>>;
}
