/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

#![cfg(feature = "file-storage-json")]
#![cfg(feature = "hashicorp-storage")]

use colored::Colorize;
use secrets_vault::{
    crypto::factory::{CryptoFactory, DefaultCryptoFactory},
    errors::error::VaultError,
    storage::{file_json::FileJsonStorage, hashicorp::HashicorpStorage, storage_trait::Storage},
    types::store_mode::StoreMode,
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum OnConflict {
    Fail,
    Skip,
    Overwrite,
}

impl FromStr for OnConflict {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fail" => Ok(Self::Fail),
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(format!("invalid on-conflict '{other}' (expected: fail|skip|overwrite)")),
        }
    }
}

pub async fn execute(
    on_conflict: OnConflict,
    dry_run: bool,
    continue_on_error: bool,
) -> anyhow::Result<()> {
    let from_url = std::env::var("FROM_VAULT_URL")
        .map_err(|_| anyhow::anyhow!("FROM_VAULT_URL environment variable is not set"))?;
    let to_url = std::env::var("VAULT_URL")
        .map_err(|_| anyhow::anyhow!("VAULT_URL environment variable is not set"))?;

    if from_url.trim() == to_url.trim() {
        anyhow::bail!("FROM_VAULT_URL and VAULT_URL refer to the same vault");
    }

    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    println!();
    println!("{}", "Copy:".cyan().bold());
    println!("  {} {}", "from:".dimmed(), redact_url(&from_url));
    println!("  {} {}", "to:  ".dimmed(), redact_url(&to_url));
    if dry_run {
        println!("  {} {}", "mode:".dimmed(), "DRY RUN (no writes)".yellow().bold());
    }
    println!();

    let dst: Arc<SecretVault> = SecretVaultBuilder::from_url(&to_url, crypto.clone()).await?;
    let dst_storage = {
        dst.storage()
            .as_ref()
            .downcast_ref::<HashicorpStorage>()
            .ok_or_else(|| anyhow::anyhow!("VAULT_URL must refer to a Hashicorp vault"))?
    };

    let src: Arc<SecretVault> = SecretVaultBuilder::from_url(&from_url, crypto).await?;
    let src_storage = {
        src.storage()
            .as_ref()
            .downcast_ref::<FileJsonStorage>()
            .ok_or_else(|| anyhow::anyhow!("FROM_VAULT_URL must refer to a file vault"))?
    };

    let records = src_storage.list_metadata().await?;
    let total = records.len();

    if total == 0 {
        println!("{} {}\n", "⚠".yellow().bold(), "Source vault has no records".yellow());
        return Ok(());
    }

    let mut copied = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let started = Instant::now();

    for (idx, meta) in records.iter().enumerate() {
        let n = idx + 1;
        let secret_id = match meta.secret_id.as_ref() {
            Some(id) => id.clone(),
            None => {
                eprintln!(
                    "{}",
                    format!("[{n}/{total}] ✗ metadata without secret_id, skipping").red()
                );
                failed += 1;
                if !continue_on_error {
                    anyhow::bail!(VaultError::empty_secret_id(
                        "Source returned metadata without secret_id"
                    ));
                }
                continue;
            }
        };

        println!(
            "{} {}",
            format!("[{n}/{total}]").dimmed(),
            format!("READ  {}", secret_id.as_str()).cyan()
        );
        println!(
            "         algo={}  payload={}  extractable={}  expires={}  tags={}",
            meta.algorithm,
            meta.algorithm.payload_type(),
            if meta.extractable { "yes" } else { "no" },
            match meta.expires_at {
                Some(t) => t.format("%Y-%m-%d %H:%M:%SZ").to_string(),
                None => "never".to_string(),
            },
            meta.tags.len()
        );
        if !meta.tags.is_empty() {
            let mut tags: Vec<_> = meta.tags.iter().collect();
            tags.sort_by(|a, b| a.0.cmp(b.0));
            let preview =
                tags.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
            println!("         tags: {}", preview.dimmed());
        }

        let (secret, is_extractable) = src_storage.load_for_migrate(&secret_id).await?;
        let exists = dst.exists(&secret_id).await.unwrap_or(false);
        let mode = match (exists, on_conflict) {
            (false, _) => StoreMode::NewOnly,
            (true, OnConflict::Skip) => {
                println!(
                    "         {} destination already has this id (on-conflict=skip)",
                    "SKIP".yellow().bold()
                );
                skipped += 1;
                continue;
            }
            (true, OnConflict::Overwrite) => StoreMode::CreateOrReplace,
            (true, OnConflict::Fail) => {
                let msg = format!(
                    "destination already has secret '{}' (use --on-conflict=skip|overwrite)",
                    secret_id
                );
                println!("         {} {}", "FAIL".red().bold(), msg);
                failed += 1;
                if continue_on_error {
                    continue;
                }
                anyhow::bail!(msg);
            }
        };

        println!("         {} {}  mode={}", "WRITE".green(), secret_id.as_str(), mode_label(&mode));

        if dry_run {
            println!("         {} (dry-run, no write performed)", "DRY".yellow().bold());
            copied += 1;
            continue;
        }

        let write_started = Instant::now();
        match dst_storage.store(&secret, mode, Some(is_extractable)).await {
            Ok(()) => {
                println!(
                    "         {} {} {}",
                    "OK".green().bold(),
                    secret_id.as_str(),
                    format!("({})", fmt_duration(write_started.elapsed())).dimmed()
                );
                copied += 1;
            }
            Err(e) => {
                println!("         {} {}: {}", "ERR".red().bold(), secret_id.as_str(), e);
                failed += 1;
                if !continue_on_error {
                    return Err(e);
                }
            }
        }
    }

    if !dry_run && copied > 0 {
        dst.flush().await?;
    }

    let elapsed = started.elapsed();
    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "  total: {}   copied: {}   skipped: {}   failed: {}   elapsed: {}",
        total.to_string().bold(),
        copied.to_string().green(),
        skipped.to_string().yellow(),
        failed.to_string().red(),
        fmt_duration(elapsed)
    );
    println!();

    if failed > 0 {
        anyhow::bail!("{} secret(s) failed to copy", failed);
    }

    println!("{} {}\n", "✓".green().bold(), "Copy completed".green());
    Ok(())
}

fn mode_label(mode: &StoreMode) -> &'static str {
    match mode {
        StoreMode::NewOnly => "NewOnly",
        StoreMode::ReplaceExists => "ReplaceExists",
        StoreMode::CreateOrReplace => "CreateOrReplace",
    }
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

fn redact_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some(p) => p,
        None => return url.to_string(),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let Some(query) = query else { return url.to_string() };

    let redacted: Vec<String> = query
        .split('&')
        .map(|seg| {
            if let Some((k, _)) = seg.split_once('=') {
                let key = k.trim().to_ascii_lowercase();
                if matches!(key.as_str(), "api_key" | "master_key" | "token") {
                    return format!("{k}=***");
                }
            }
            seg.to_string()
        })
        .collect();

    format!("{scheme}://{path}?{}", redacted.join("&"))
}
