/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::utils::{open_vault, print_secret, print_secret_full, print_secret_header};
use colored::Colorize;
use secrets_vault::{errors::error::VaultError, storage::storage_trait::ListMode};

pub async fn execute(full: bool) -> anyhow::Result<()> {
    let vault = open_vault().await?;
    let records = vault.list_metadata(ListMode::OnlyNeeded).await?;

    if records.is_empty() {
        println!("\n{} {}\n", "⚠".yellow().bold(), "No records found".yellow());
        return Ok(());
    }

    println!("\n{} {} ({})\n", "✓".green().bold(), "Records:".green(), records.len());

    if !full {
        print_secret_header();
    }

    for meta in &records {
        let secret_id = meta
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to list secrets"))?;
        let secret = vault.load(secret_id).await?;
        if full {
            print_secret_full(&secret)?;
        } else {
            print_secret(&secret)?;
        }
    }

    println!();
    Ok(())
}
