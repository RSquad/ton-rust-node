/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod wallet_contract;
use crate::SmartContract;
use ton_block::{Cell, MsgAddressInt, StateInit};
pub use wallet_contract::WalletContract;

#[async_trait::async_trait]
pub trait TonWallet: SmartContract + Send + Sync {
    async fn message(&self, dest: MsgAddressInt, value: u64, payload: Cell)
    -> anyhow::Result<Cell>;

    async fn deploy_message(&self, value: u64, payload: Cell) -> anyhow::Result<Cell>;

    async fn build_message(
        &self,
        dest: MsgAddressInt,
        value: u64,
        payload: Cell,
        bounce: bool,
        seqno: Option<u32>,
        state_init_external: Option<StateInit>,
        state_init_internal: Option<StateInit>,
    ) -> anyhow::Result<Cell>;

    /// Build the StateInit used to deploy this wallet on-chain.
    async fn state_init(&self) -> anyhow::Result<StateInit> {
        anyhow::bail!("state_init not supported for this wallet")
    }
}
