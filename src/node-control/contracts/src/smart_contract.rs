/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::MsgAddressInt;

#[async_trait::async_trait]
pub trait SmartContract {
    async fn balance(&self) -> anyhow::Result<u64>;
    async fn address(&self) -> anyhow::Result<MsgAddressInt>;
}
