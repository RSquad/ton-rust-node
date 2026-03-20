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
    errors::error::VaultError, types::secret::Secret, vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::sync::Arc;

pub async fn open_vault() -> anyhow::Result<Arc<SecretVault>> {
    let vault = SecretVaultBuilder::from_env().await?;
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

pub async fn print_secret(secret: &Secret) -> anyhow::Result<()> {
    let metadata = secret.metadata();
    let secret_id = metadata
        .secret_id
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_id("Failed to print secret"))?;
    let public_key =
        if let Secret::KeyPair { keypair } = secret { keypair.public_key().await? } else { None };

    println!(
        "  {:<30} {:<12} {:<10} {:<12} {:<20} {}",
        truncate(&secret_id.to_string(), 30),
        metadata.algorithm.to_string(),
        metadata.algorithm.payload_type().to_string(),
        if metadata.extractable { "Yes".green() } else { "No".red() },
        metadata.created_at.format("%Y-%m-%d %H:%M"),
        if let Some(pk) = public_key {
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pk.as_slice())
        } else {
            "".to_string()
        }
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
