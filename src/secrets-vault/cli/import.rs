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
    crypto::factory::{AutoCryptoFactory, CryptoFactory},
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    types::{algorithm::Algorithm, metadata::Metadata, secret::Secret, store_mode::StoreMode},
};

pub async fn execute(
    secret_id: &str,
    data: &[u8],
    algorithm: Algorithm,
    extractable: bool,
    overwrite: bool,
) -> anyhow::Result<()> {
    let vault = open_vault().await?;

    let store_mode = match overwrite {
        true => StoreMode::CreateOrReplace,
        false => StoreMode::NewOnly,
    };

    let secret_id = secret_id.into();
    let metadata = Metadata::new(Some(&secret_id), algorithm, extractable);

    let secret = match algorithm {
        Algorithm::None => {
            let crypto = AutoCryptoFactory {}.new_crypto()?;
            Secret::from_raw_data(data, metadata, crypto).await?
        }
        Algorithm::Ed25519 => {
            let crypto = AutoCryptoFactory {}.new_crypto()?;
            let data = from_private_key(data).await?;
            Secret::from_protected_data(data, metadata, crypto).await?
        }
        Algorithm::Aes256Gcm => {
            todo!()
        }
        _ => anyhow::bail!(VaultError::unsupported_algorithm(algorithm)),
    };

    vault.put(&secret, store_mode).await?;
    vault.flush().await?;

    println!(
        "\n{} {}\n",
        "✓".green().bold(),
        "The secret was successfully imported into the vault".green()
    );

    Ok(())
}

async fn from_private_key(key: &[u8]) -> anyhow::Result<ProtectedMemory> {
    // Verify key length
    if key.len() != ed25519_dalek::SECRET_KEY_LENGTH {
        anyhow::bail!(VaultError::invalid_private_key(format!(
            "Wrong key length {}, expected {}",
            key.len(),
            ed25519_dalek::SECRET_KEY_LENGTH
        )));
    }

    // Create key pair
    let secret_key: ed25519_dalek::SecretKey = key.try_into().map_err(|e| {
        VaultError::invalid_private_key("Failed to create a SigningKey from the bytes")
            .with_source(e)
    })?;

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_key);
    let signing_key_p = ProtectedMemory::from_slice(signing_key.as_bytes()).await?;

    Ok(signing_key_p)
}
