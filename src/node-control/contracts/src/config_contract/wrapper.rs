/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::SmartContract;
use ton_block::Cell;

pub type ProposalHash = [u8; 32];

#[async_trait::async_trait]
pub trait ConfigContractWrapper: SmartContract + Send + Sync {
    /// Get the current sequence number of the config contract
    async fn seqno(&self) -> anyhow::Result<u32>;

    /// Get details of a specific proposal by its hash
    async fn get_proposal(&self, phash: ProposalHash) -> anyhow::Result<Option<ConfigProposal>>;

    /// List all active voting proposals
    async fn list_proposals(&self) -> anyhow::Result<Vec<ConfigProposal>>;

    /// Calculate the storage price for a proposal
    async fn proposal_storage_price(
        &self,
        critical: bool,
        seconds: u32,
        bits: u32,
        refs: u32,
    ) -> anyhow::Result<i64>;
}

#[derive(Clone)]
pub struct ConfigProposal {
    pub hash: ProposalHash,
    pub expires: u32,
    pub is_critical: bool,
    pub param: ProposedParam,
    pub vset_id: [u8; 32],
    pub voters: Vec<u16>,
    pub weight_remaining: i64,
    pub rounds_remaining: u8,
    pub losses: u8,
    pub wins: u8,
}

#[derive(Clone)]
pub struct ProposedParam {
    pub id: i32,
    pub cell: Option<Cell>,
    pub hash: Option<[u8; 32]>,
}
