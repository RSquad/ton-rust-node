/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use colored::Colorize;
use secrets_vault::{
    crypto::key_material::KeyMaterial, errors::error::VaultError, vault_builder::VaultUrl,
};

pub async fn execute() -> anyhow::Result<()> {
    let url = resolve_url()?;
    let parsed = VaultUrl::parse(&url)?;

    if parsed.storage_name == "file" {
        #[cfg(feature = "file-storage-json")]
        {
            use secrets_vault::{
                crypto::factory::{AutoCryptoFactory, CryptoFactory},
                storage::file_json::FileJsonStorage,
                utils::hex::hex_val_to_pm,
            };
            use std::path::Path;

            if parsed.path.is_empty() {
                anyhow::bail!(VaultError::invalid_config_url("Missing path part"));
            }

            let file_path = if Path::new(parsed.path).is_absolute() {
                Path::new(parsed.path).to_path_buf()
            } else {
                std::env::current_dir()?.join(parsed.path)
            };

            if !tokio::fs::try_exists(&file_path).await? {
                anyhow::bail!("Vault file not found: {}", file_path.display());
            }

            let master_key_hex = parsed
                .query_param("master_key")
                .ok_or_else(|| anyhow::anyhow!("Missing master_key parameter in URL"))?;
            let master_key_pm = hex_val_to_pm("master_key", master_key_hex).await?;
            let master_key = KeyMaterial::new_symmetric_key(master_key_pm).await?;
            let crypto = AutoCryptoFactory {}.new_crypto()?;

            FileJsonStorage::migrate(&file_path, &master_key, crypto.as_ref()).await?;
        }

        #[cfg(not(feature = "file-storage-json"))]
        {
            anyhow::bail!("file-storage-json feature is not enabled");
        }
    } else if parsed.storage_name == "hashicorp" {
        #[cfg(feature = "hashicorp-storage")]
        {
            secrets_vault::storage::hashicorp::HashicorpStorage::migrate().await?;
        }

        #[cfg(not(feature = "hashicorp-storage"))]
        {
            anyhow::bail!("hashicorp-storage feature is not enabled");
        }
    } else {
        anyhow::bail!(VaultError::invalid_config_url(format!(
            "unsupported storage '{}'",
            parsed.storage_name
        )));
    }

    println!("\n{} {}\n", "✓".green().bold(), "Migration completed successfully".green());

    Ok(())
}

fn resolve_url() -> anyhow::Result<String> {
    match std::env::var("VAULT_URL") {
        Ok(val) => Ok(val),
        Err(_) => anyhow::bail!("No URL provided. Set VAULT_URL environment variable"),
    }
}
