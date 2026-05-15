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
    crypto::factory::{CryptoFactory, DefaultCryptoFactory},
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        secret::{SecretDataType, SecretInMemoryFactory},
        store_mode::StoreMode,
    },
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
            let crypto = DefaultCryptoFactory {}.new_crypto()?;
            SecretInMemoryFactory::new_raw(data, metadata, crypto)?
        }
        Algorithm::Ed25519 => {
            let crypto = DefaultCryptoFactory {}.new_crypto()?;
            let data = from_private_key(data)?;
            SecretInMemoryFactory::from_protected_data(
                data,
                SecretDataType::Ed25519PvtKey,
                metadata,
                crypto,
            )?
        }
        Algorithm::Aes256Gcm => {
            todo!()
        }
        _ => anyhow::bail!(VaultError::unsupported_algorithm(algorithm)),
    };

    vault.store(&secret, store_mode).await?;
    vault.flush().await?;

    println!(
        "\n{} {}\n",
        "✓".green().bold(),
        "The secret was successfully imported into the vault".green()
    );

    Ok(())
}

fn from_private_key(key: &[u8]) -> anyhow::Result<ProtectedMemory> {
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
    let signing_key_p: ProtectedMemory =
        ProtectedMemoryInner::from_slice(signing_key.as_bytes())?.into();

    Ok(signing_key_p)
}
