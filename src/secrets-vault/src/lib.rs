/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

// SecretsVault - Cryptographic Key/Secrets Management Library

pub mod crypto;
pub mod errors;
pub mod events;
pub mod memory;
pub mod storage;
pub mod types;
pub mod utils;
pub mod vault;
pub mod vault_builder;

#[cfg(feature = "ton-block")]
pub mod vault_block;

#[cfg(test)]
mod tests;
