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

pub async fn execute() -> anyhow::Result<()> {
    let vault = open_vault().await?;
    vault.flush().await?;

    println!("\n{} {}\n", "✓".green().bold(), "Vault initialized successfully".green());

    Ok(())
}
