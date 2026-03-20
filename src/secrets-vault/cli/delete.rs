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

pub async fn execute(secret_ids: &Vec<String>) -> anyhow::Result<()> {
    let vault = open_vault().await?;

    let mut success_count = 0;
    let mut failed_count = 0;

    println!();
    println!("{}", "Deletion Results:".cyan().bold());
    println!("{}", "─".repeat(50).dimmed());

    for secret_id in secret_ids {
        let secret_id = secret_id.into();
        match vault.delete(&secret_id).await {
            Ok(()) => {
                println!("  {} '{}'", "✓".green(), secret_id.as_str());
                success_count += 1;
            }
            Err(e) => {
                println!("  {} '{}' - {}", "✗".red(), secret_id.as_str().red(), e);
                failed_count += 1;
            }
        }
    }

    println!("{}", "─".repeat(50).dimmed());
    println!(
        "  Success: {}  Failed: {}",
        success_count.to_string().green(),
        failed_count.to_string().red()
    );
    println!();

    if success_count == 0 {
        println!("{} {}", "⚠".yellow().bold(), "No changes to save".yellow());
        return Ok(());
    }

    Ok(())
}
