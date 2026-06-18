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
    crypto::factory::{CryptoFactory, DefaultCryptoFactory},
    errors::error::VaultError,
    types::secret::Secret,
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::sync::Arc;

pub async fn open_vault() -> anyhow::Result<Arc<SecretVault>> {
    let vault = SecretVaultBuilder::from_env(DefaultCryptoFactory {}.new_crypto()?).await?;
    Ok(vault)
}

#[derive(Clone)]
pub struct HexBytes(pub Vec<u8>);

pub fn parse_hex_bytes(s: &str) -> Result<HexBytes, String> {
    hex::decode(s).map(HexBytes).map_err(|e| e.to_string())
}

pub fn print_secret_header() {
    println!(
        "  {:<30} {:<12} {:<10} {:<12} {}",
        "ID".cyan().bold(),
        "Algorithm".cyan().bold(),
        "Payload".cyan().bold(),
        "Extractable".cyan().bold(),
        "Created".cyan().bold(),
    );
    println!("  {}", "─".repeat(132).dimmed());
}

pub fn print_secret(secret: &Secret) -> anyhow::Result<()> {
    let metadata = secret.metadata();
    let secret_id = metadata
        .secret_id
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_id("Failed to print secret"))?;
    let public_key: Option<&[u8]> =
        if let Secret::KeyPair { keypair } = secret { keypair.public_key() } else { None };

    println!(
        "  {:<30} {:<12} {:<10} {:<12} {:<20} {}",
        truncate(&secret_id.to_string(), 30),
        metadata.algorithm.to_string(),
        metadata.algorithm.payload_type().to_string(),
        if metadata.extractable { "Yes".green() } else { "No".red() },
        metadata.created_at.format("%Y-%m-%d %H:%M"),
        if let Some(pub_key) = public_key { base64::encode(pub_key) } else { "".to_string() }
    );

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

pub fn print_secret_full(secret: &Secret, show_private: bool) -> anyhow::Result<()> {
    let metadata = secret.metadata();
    let secret_id = metadata
        .secret_id
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_id("Failed to print secret"))?;

    println!("  {} {}", "ID:".cyan().bold(), secret_id);
    println!("  {} {}", "Variant:".cyan().bold(), secret.variant_name());
    println!("  {} {}", "Algorithm:".cyan().bold(), metadata.algorithm);
    println!("  {} {}", "Payload:".cyan().bold(), metadata.algorithm.payload_type());
    println!(
        "  {} {}",
        "Extractable:".cyan().bold(),
        if metadata.extractable { "Yes".green() } else { "No".red() }
    );
    println!(
        "  {} {}",
        "Created At:".cyan().bold(),
        metadata.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!(
        "  {} {}",
        "Expires At:".cyan().bold(),
        match metadata.expires_at {
            Some(ts) => ts.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => "never".dimmed().to_string(),
        }
    );

    if let Secret::KeyPair { keypair } = secret {
        if let Some(pub_key) = keypair.public_key() {
            println!("  {} {}", "Public Key:".cyan().bold(), base64::encode(pub_key));
        }

        if keypair.extractable() {
            if show_private {
                let pvt_key = keypair.private_key()?.lock()?;
                println!("  {} {}", "Private Key:".cyan().bold(), base64::encode(pvt_key));
            } else {
                println!(
                    "  {} {}",
                    "Private Key:".cyan().bold(),
                    "(hidden, pass --show-private to reveal)".dimmed()
                );
            }
        }
    }

    if metadata.tags.is_empty() {
        println!("  {} {}", "Tags:".cyan().bold(), "(none)".dimmed());
    } else {
        println!("  {}", "Tags:".cyan().bold());
        let mut tags: Vec<_> = metadata.tags.iter().collect();
        tags.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in tags {
            println!("    {} = {}", k, v);
        }
    }

    println!("  {}", "─".repeat(132).dimmed());

    Ok(())
}
