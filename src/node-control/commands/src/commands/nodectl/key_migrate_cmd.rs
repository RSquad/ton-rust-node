/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use colored::Colorize;
use secrets_vault::{
    crypto::factory::CryptoFactory,
    errors::error::VaultError,
    storage::{file_json::FileJsonStorage, hashicorp::HashicorpStorage, storage_trait::ListMode},
    types::{algorithm::Algorithm, metadata::Metadata, secret_id::SecretId, store_mode::StoreMode},
    vault::SecretVault,
    vault_block::BlockCryptoFactory,
    vault_builder::SecretVaultBuilder,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum OnConflict {
    Fail,
    Skip,
    Overwrite,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum ListModeArg {
    OnlyNeeded,
    All,
}

impl From<ListModeArg> for ListMode {
    fn from(v: ListModeArg) -> Self {
        match v {
            ListModeArg::OnlyNeeded => ListMode::OnlyNeeded,
            ListModeArg::All => ListMode::All,
        }
    }
}

#[derive(clap::Args, Clone, Debug)]
#[command(about = "Copy all secrets from FROM_VAULT_URL to VAULT_URL")]
pub struct KeyMigrateCmd {
    /// Conflict policy when destination already has a secret with the same id
    #[arg(long = "on-conflict", value_enum, default_value = "fail")]
    on_conflict: OnConflict,

    /// Source list mode
    #[arg(long = "list-mode", value_enum, default_value = "all")]
    list_mode: ListModeArg,

    /// Print plan without writing to destination
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Continue on per-secret errors instead of aborting
    #[arg(long = "continue-on-error")]
    continue_on_error: bool,

    /// Allow exporting private bytes of non-extractable Ed25519 keys from a
    /// file-storage source into HashiCorp Vault. The imported key remains
    /// non-extractable at the destination. Has no effect for any other
    /// source/destination combination.
    #[arg(long = "allow-nonexportable")]
    allow_nonexportable: bool,
}

impl KeyMigrateCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let from_url = std::env::var("FROM_VAULT_URL")
            .map_err(|_| anyhow::anyhow!("FROM_VAULT_URL environment variable is not set"))?;
        let to_url = std::env::var("VAULT_URL")
            .map_err(|_| anyhow::anyhow!("VAULT_URL environment variable is not set"))?;

        if from_url.trim() == to_url.trim() {
            anyhow::bail!("FROM_VAULT_URL and VAULT_URL refer to the same vault");
        }

        let crypto = BlockCryptoFactory {}.new_crypto()?;

        println!();
        println!("{}", "Migrate:".cyan().bold());
        println!("  {} {}", "from:".dimmed(), redact_url(&from_url));
        println!("  {} {}", "to:  ".dimmed(), redact_url(&to_url));
        if self.dry_run {
            println!("  {} {}", "mode:".dimmed(), "DRY RUN (no writes)".yellow().bold());
        }
        println!();

        let src: Arc<SecretVault> = SecretVaultBuilder::from_url(&from_url, crypto.clone())
            .await
            .context("open source vault")?;
        let dst: Arc<SecretVault> = SecretVaultBuilder::from_url(&to_url, crypto)
            .await
            .context("open destination vault")?;

        // Typed downcasts for the file→hashicorp raw migration path used for
        // non-extractable Ed25519 secrets (gated by --allow-nonexportable).
        let src_file: Option<&FileJsonStorage> =
            src.storage().as_ref().downcast_ref::<FileJsonStorage>();
        let dst_hc: Option<&HashicorpStorage> =
            dst.storage().as_ref().downcast_ref::<HashicorpStorage>();

        let records =
            src.list_metadata(self.list_mode.into()).await.context("list source vault records")?;
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
                Some(id) => id,
                None => {
                    eprintln!(
                        "{}",
                        format!("[{n}/{total}] ✗ metadata without secret_id, skipping").red()
                    );
                    failed += 1;
                    if !self.continue_on_error {
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

            let use_raw_path = self.allow_nonexportable
                && !meta.extractable
                && meta.algorithm == Algorithm::Ed25519
                && src_file.is_some()
                && dst_hc.is_some();

            let secret = if use_raw_path {
                None
            } else {
                match src.load(secret_id).await {
                    Ok(s) => Some(s),
                    Err(e) => {
                        println!(
                            "         {} cannot read from source: {}",
                            "SKIP".yellow().bold(),
                            e
                        );
                        skipped += 1;
                        if !self.continue_on_error && !is_skippable_load_error(&e) {
                            return Err(e);
                        }
                        continue;
                    }
                }
            };

            let exists = dst.exists(secret_id).await.unwrap_or(false);
            let mode = match (exists, self.on_conflict) {
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
                    if self.continue_on_error {
                        continue;
                    }
                    anyhow::bail!(msg);
                }
            };

            if use_raw_path {
                println!(
                    "         {} {}  mode={}  (non-extractable, file→hashicorp)",
                    "WRITE-RAW".magenta().bold(),
                    secret_id.as_str(),
                    mode_label(mode)
                );
            } else {
                println!(
                    "         {} {}  mode={}",
                    "WRITE".green(),
                    secret_id.as_str(),
                    mode_label(mode)
                );
            }

            if self.dry_run {
                println!("         {} (dry-run, no write performed)", "DRY".yellow().bold());
                copied += 1;
                continue;
            }

            let write_started = Instant::now();
            let write_result = match (use_raw_path, src_file, dst_hc, secret.as_ref()) {
                (true, Some(sf), Some(dh), _) => {
                    raw_migrate_ed25519(sf, dh, secret_id, meta, mode).await
                }
                (false, _, _, Some(s)) => dst.store(s, mode).await,
                _ => Err(anyhow::anyhow!(
                    "internal: use_raw_path/storage downcasts/loaded secret out of sync"
                )),
            };

            match write_result {
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
                    if !self.continue_on_error {
                        return Err(e);
                    }
                }
            }
        }

        if !self.dry_run && copied > 0 {
            dst.flush().await.context("flush destination vault")?;
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

        println!("{} {}\n", "✓".green().bold(), "Migration completed".green());
        Ok(())
    }
}

/// Migrate one Ed25519 secret from file-storage to HashiCorp Vault via the raw
/// bytes path. The destination key is created non-extractable; the original
/// `extractable=false` policy is preserved in the recorded metadata.
async fn raw_migrate_ed25519(
    src_file: &FileJsonStorage,
    dst_hc: &HashicorpStorage,
    secret_id: &SecretId,
    meta: &Metadata,
    mode: StoreMode,
) -> anyhow::Result<()> {
    let (_, pvt_key) = src_file
        .export_for_migration(secret_id)
        .await
        .context("export secret bytes from file storage")?;
    dst_hc
        .client()
        .import_ed25519_raw(secret_id, &pvt_key, false, mode)
        .await
        .context("import wrapped key into hashicorp")?;
    dst_hc
        .client()
        .set_metadata(secret_id.as_str(), meta, None)
        .await
        .context("write metadata into hashicorp")?;
    Ok(())
}

fn is_skippable_load_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<VaultError>().is_some_and(|ve| ve.code() == VaultError::NOT_EXTRACTABLE)
}

fn mode_label(mode: StoreMode) -> &'static str {
    match mode {
        StoreMode::NewOnly => "NewOnly",
        StoreMode::ReplaceExists => "ReplaceExists",
        StoreMode::CreateOrReplace => "CreateOrReplace",
    }
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 { format!("{ms}ms") } else { format!("{:.2}s", d.as_secs_f64()) }
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
    let Some(query) = query else {
        return url.to_string();
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser, Debug)]
    struct TestCli {
        #[command(flatten)]
        cmd: KeyMigrateCmd,
    }

    #[test]
    fn migrate_cli_uses_safe_defaults() {
        let cli = TestCli::try_parse_from(["nodectl"]).expect("parse defaults");
        assert!(matches!(cli.cmd.on_conflict, OnConflict::Fail));
        assert!(matches!(cli.cmd.list_mode, ListModeArg::All));
        assert!(!cli.cmd.dry_run);
        assert!(!cli.cmd.continue_on_error);
        assert!(!cli.cmd.allow_nonexportable);
    }

    #[test]
    fn migrate_cli_accepts_full_flag_set() {
        let cli = TestCli::try_parse_from([
            "nodectl",
            "--on-conflict",
            "overwrite",
            "--list-mode",
            "all",
            "--dry-run",
            "--continue-on-error",
            "--allow-nonexportable",
        ])
        .expect("parse full set");
        assert!(matches!(cli.cmd.on_conflict, OnConflict::Overwrite));
        assert!(matches!(cli.cmd.list_mode, ListModeArg::All));
        assert!(cli.cmd.dry_run);
        assert!(cli.cmd.continue_on_error);
        assert!(cli.cmd.allow_nonexportable);
    }

    #[test]
    fn migrate_cli_rejects_unknown_on_conflict() {
        let err = TestCli::try_parse_from(["nodectl", "--on-conflict", "bogus"]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bogus"), "error should mention bad value: {msg}");
    }

    #[test]
    fn migrate_cli_rejects_unknown_list_mode() {
        let err = TestCli::try_parse_from(["nodectl", "--list-mode", "partial"]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("partial"), "error should mention bad value: {msg}");
    }

    #[test]
    fn redact_url_masks_sensitive_query_params() {
        let red = redact_url("file:///vault.json?master_key=abcd&auto_migrate=true");
        assert!(red.contains("master_key=***"));
        assert!(red.contains("auto_migrate=true"));

        let red = redact_url("hashicorp://vault:8200?api_key=hvs.secret&namespace=ns");
        assert!(red.contains("api_key=***"));
        assert!(red.contains("namespace=ns"));

        let red = redact_url("hashicorp://vault:8200?token=t1&role=r1");
        assert!(red.contains("token=***"));
        assert!(red.contains("role=r1"));
    }

    #[test]
    fn redact_url_passes_through_when_no_query() {
        let url = "file:///vault.json";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn fmt_duration_picks_ms_or_seconds() {
        assert!(fmt_duration(Duration::from_millis(42)).ends_with("ms"));
        assert!(fmt_duration(Duration::from_millis(2500)).ends_with("s"));
    }

    #[test]
    fn mode_label_returns_expected_strings() {
        assert_eq!(mode_label(StoreMode::NewOnly), "NewOnly");
        assert_eq!(mode_label(StoreMode::ReplaceExists), "ReplaceExists");
        assert_eq!(mode_label(StoreMode::CreateOrReplace), "CreateOrReplace");
    }
}
