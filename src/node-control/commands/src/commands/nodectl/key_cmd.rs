/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::key_migrate_cmd::KeyMigrateCmd;
use anyhow::Context;
use colored::Colorize;
use secrets_vault::{
    crypto::factory::CryptoFactory,
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        secret::{Secret, SecretDataType, SecretInMemoryFactory},
        secret_spec::SecretSpec,
        store_mode::StoreMode,
    },
    vault::SecretVault,
    vault_block::BlockCryptoFactory,
    vault_builder::SecretVaultBuilder,
};

#[derive(clap::Args, Clone)]
#[command(about = "Manage vault keys")]
pub struct KeyCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[command(subcommand)]
    action: KeyAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum KeyAction {
    /// Generate a new key in the vault
    Add(KeyAddCmd),
    /// Import a key from a private key into the vault
    Import(KeyImportCmd),
    /// List all keys in the vault
    Ls(KeyLsCmd),
    /// Remove a key from the vault
    Rm(KeyRmCmd),
    /// Copy all secrets from FROM_VAULT_URL to VAULT_URL
    Migrate(KeyMigrateCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Generate a new key in the vault")]
pub struct KeyAddCmd {
    #[arg(short = 'n', long = "name")]
    name: String,
    #[arg(short = 'a', long = "algorithm", default_value = "ed25519")]
    algorithm: String,
    #[arg(short = 'e', long = "extractable")]
    extractable: bool,
}

#[derive(clap::Args, Clone)]
#[command(about = "Import a key from a private key into the vault")]
pub struct KeyImportCmd {
    #[arg(short = 'n', long = "name")]
    name: String,
    #[arg(short = 'k', long = "private-key")]
    private_key: String,
    #[arg(short = 'a', long = "algorithm", default_value = "ed25519")]
    algorithm: String,
    #[arg(short = 'e', long = "extractable")]
    extractable: bool,
}

#[derive(clap::Args, Clone)]
#[command(about = "List all keys in the vault")]
pub struct KeyLsCmd {}

#[derive(clap::Args, Clone)]
#[command(about = "Remove a key from the vault")]
pub struct KeyRmCmd {
    #[arg(short = 'n', long = "name")]
    name: String,
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len { s.to_string() } else { format!("{}...", &s[..max_len - 3]) }
}

fn print_header() {
    println!(
        "  {:<30} {:<12} {:<12} {:<20} {}",
        "Name".cyan().bold(),
        "Algorithm".cyan().bold(),
        "Extractable".cyan().bold(),
        "Created".cyan().bold(),
        "Public Key".cyan().bold(),
    );
    println!("  {}", "─".repeat(120).dimmed());
}

async fn print_secret(secret: &Secret) -> anyhow::Result<()> {
    let metadata = secret.metadata();
    let secret_id =
        metadata.secret_id.as_ref().ok_or_else(|| anyhow::anyhow!("Secret has no ID"))?;
    let public_key =
        if let Secret::KeyPair { keypair } = secret { keypair.public_key() } else { None };

    println!(
        "  {:<30} {:<12} {:<12} {:<20} {}",
        truncate(&secret_id.to_string(), 30),
        metadata.algorithm.to_string(),
        if metadata.extractable { "Yes".green() } else { "No".red() },
        metadata.created_at.format("%Y-%m-%d %H:%M"),
        if let Some(pk) = public_key {
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pk)
        } else {
            "".to_string()
        }
    );

    Ok(())
}

impl KeyCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        // `migrate` opens both source and destination vaults itself from
        // FROM_VAULT_URL and VAULT_URL, so skip the shared single-vault open.
        if let KeyAction::Migrate(cmd) = &self.action {
            return cmd.run().await;
        }

        let vault = SecretVaultBuilder::from_env(BlockCryptoFactory {}.new_crypto()?).await?;
        match &self.action {
            KeyAction::Add(cmd) => cmd.run(&vault).await,
            KeyAction::Import(cmd) => cmd.run(&vault).await,
            KeyAction::Ls(cmd) => cmd.run(&vault).await,
            KeyAction::Rm(cmd) => cmd.run(&vault).await,
            KeyAction::Migrate(_) => unreachable!("handled above"),
        }
    }
}

impl KeyAddCmd {
    pub async fn run(&self, vault: &SecretVault) -> anyhow::Result<()> {
        let algo: Algorithm = self.algorithm.parse().context("Invalid algorithm")?;
        let secret_id = self.name.as_str().into();
        let spec = SecretSpec::new(algo).extractable(self.extractable);
        vault.generate_secret(&spec, &secret_id).await?;
        vault.flush().await?;
        let secret = vault.load(&secret_id).await?;

        println!("\n{} {}\n", "OK".green().bold(), "Key generated successfully".green());

        print_header();
        print_secret(&secret).await?;
        println!();
        Ok(())
    }
}

impl KeyImportCmd {
    pub async fn run(&self, vault: &SecretVault) -> anyhow::Result<()> {
        let algo: Algorithm = self.algorithm.parse().context("Invalid algorithm")?;
        let private_key_bytes =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &self.private_key)
                .context("Invalid base64 private key")?;
        let secret_id = self.name.as_str().into();
        let metadata = Metadata::new(Some(&secret_id), algo, self.extractable);
        let secret = SecretInMemoryFactory::from_raw_data(
            &private_key_bytes,
            SecretDataType::from_algo(algo),
            metadata,
            BlockCryptoFactory {}.new_crypto()?,
        )?;
        vault.store(&secret, StoreMode::CreateOrReplace).await?;
        vault.flush().await?;
        let secret = vault.load(&secret_id).await?;

        println!("\n{} {}\n", "OK".green().bold(), "Key imported successfully".green());

        print_header();
        print_secret(&secret).await?;
        println!();
        Ok(())
    }
}

impl KeyLsCmd {
    pub async fn run(&self, vault: &SecretVault) -> anyhow::Result<()> {
        let records = vault.list_metadata().await?;

        if records.is_empty() {
            println!("\n{}\n", "No keys found".yellow());
            return Ok(());
        }

        println!("\n{} {} ({})\n", "OK".green().bold(), "Keys:".green(), records.len());

        print_header();

        for meta in &records {
            let secret_id =
                meta.secret_id.as_ref().ok_or_else(|| anyhow::anyhow!("Secret has no ID"))?;
            let secret = vault.load(secret_id).await?;
            print_secret(&secret).await?;
        }

        println!();
        Ok(())
    }
}

impl KeyRmCmd {
    pub async fn run(&self, vault: &SecretVault) -> anyhow::Result<()> {
        let secret_id = self.name.as_str().into();
        vault.delete(&secret_id).await?;
        vault.flush().await?;

        println!("\n{} Key '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}
