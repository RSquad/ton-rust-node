/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::utils::{open_vault, print_secret, print_secret_header};
use colored::Colorize;
use secrets_vault::types::{algorithm::Algorithm, secret_spec::SecretSpec};

pub async fn execute(
    secret_id: &str,
    algorithm: Algorithm,
    extractable: bool,
) -> anyhow::Result<()> {
    let vault = open_vault().await?;
    let secret_id = secret_id.into();
    let spec = SecretSpec::new(algorithm).extractable(extractable);
    vault.generate_secret(&spec, &secret_id).await?;
    vault.flush().await?;
    let secret = vault.load(&secret_id).await?;

    println!("\n{} {}\n", "✓".green().bold(), "The secret was successfully generated".green());

    print_secret_header();
    print_secret(&secret)?;

    Ok(())
}
