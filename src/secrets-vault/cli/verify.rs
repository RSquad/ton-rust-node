/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::utils::open_vault;
use colored::Colorize;
use secrets_vault::{
    errors::error::VaultError,
    types::{secret::Secret, secret_id::SecretId},
};

pub async fn execute(secret_id: &str, data: &[u8], signature: &[u8]) -> anyhow::Result<()> {
    let vault = open_vault().await?;
    let secret_id: SecretId = secret_id.into();
    let secret = vault.load(&secret_id).await?;

    let key_pair = match secret {
        Secret::Blob { .. } | Secret::SymmetricKey { .. } => {
            anyhow::bail!(VaultError::unsupported_algorithm(secret.metadata().algorithm));
        }
        Secret::KeyPair { keypair } => keypair,
    };

    let res = key_pair.verify(data, signature).await;

    match res {
        Ok(..) => {
            println!("\n{} {}\n", "✓".green().bold(), "Valid signature".green());
            Ok(())
        }
        Err(e) => Err(e),
    }
}
