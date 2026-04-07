/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::utils::save_config;
use anyhow::Context;
use colored::Colorize;
use common::{
    app_config::{AppConfig, Role, UserEntry},
    hash_password, time_format,
};
use secrets_vault::{types::secret_id::SecretId, vault_builder::SecretVaultBuilder};
use service::auth::user_store::{store_password_blob, user_secret_id, validate_username};
use std::{io::Read, path::Path};

#[derive(clap::Args, Clone)]
#[command(about = "Manage authentication users")]
pub struct AuthCmd {
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
    action: AuthAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum AuthAction {
    /// Add a new user
    Add(AddUserCmd),
    /// List all users
    Ls,
    /// Remove a user
    Rm(RemoveUserCmd),
    /// Revoke all existing tokens for a user
    Revoke(RevokeUserCmd),
    /// Update auth settings
    Set(SetCmd),
}

#[derive(clap::Args, Clone)]
pub struct SetCmd {
    #[command(subcommand)]
    action: SetAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum SetAction {
    /// Set token TTL (seconds)
    Ttl(SetTtlCmd),
}

#[derive(clap::Args, Clone)]
pub struct SetTtlCmd {
    #[arg(long = "operator", value_parser = parse_duration, help = "Operator token TTL (e.g. 3600, 30s, 60m, 8h)")]
    operator: Option<u64>,
    #[arg(long = "nominator", value_parser = parse_duration, help = "Nominator token TTL (e.g. 86400, 30s, 60m, 24h)")]
    nominator: Option<u64>,
}

fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("value cannot be empty".to_string());
    }
    if let Ok(v) = s.parse::<u64>() {
        return Ok(v);
    }
    let (num, suffix) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().map_err(|_| format!("invalid number: {num}"))?;
    match suffix {
        "s" => Ok(n),
        "m" => Ok(n * 60),
        "h" => Ok(n * 3600),
        _ => Err(format!("unknown suffix '{suffix}', use s, m, or h")),
    }
}

#[derive(clap::Args, Clone)]
pub struct AddUserCmd {
    #[arg(
        long = "username",
        short = 'u',
        required = true,
        help = "User username (alphanumeric, underscore, or hyphen, max 64 characters)"
    )]
    username: String,
    #[arg(
        long = "role",
        short = 'r',
        value_enum,
        required = true,
        help = "User role [possible values: operator, nominator]"
    )]
    role: Role,
    #[arg(
        long = "password-stdin",
        help = "Read password from stdin instead of interactive prompt (no confirmation)"
    )]
    password_stdin: bool,
}

#[derive(clap::Args, Clone)]
pub struct RemoveUserCmd {
    #[arg(required = true)]
    username: String,
}

#[derive(clap::Args, Clone)]
pub struct RevokeUserCmd {
    #[arg(required = true)]
    username: String,
    #[arg(long = "at", help = "Unix timestamp cutoff. Defaults to current time.")]
    at: Option<u64>,
}

impl AuthCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let path = Path::new(&self.config);
        match &self.action {
            AuthAction::Add(cmd) => cmd.run(path).await,
            AuthAction::Ls => list_users(path),
            AuthAction::Rm(cmd) => cmd.run(path).await,
            AuthAction::Revoke(cmd) => cmd.run(path),
            AuthAction::Set(cmd) => cmd.run(path),
        }
    }
}

impl AddUserCmd {
    async fn run(&self, config_path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(config_path)?;

        if config.http.auth.is_none() {
            println!(
                "{}: auth is not configured yet; enabling with default settings",
                "Warning".yellow().bold()
            );
        }

        validate_username(&self.username)?;

        let auth = config.http.auth.get_or_insert_with(Default::default);
        if auth.users.iter().any(|u| u.username == self.username) {
            anyhow::bail!("user '{}' already exists in config", self.username);
        }
        let min_len = auth.min_password_length;

        let password = if self.password_stdin {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .context("failed to read password from stdin")?;
            input.trim_end_matches(['\n', '\r']).to_owned()
        } else {
            let pw =
                rpassword::prompt_password("Enter password: ").context("failed to read password")?;
            let confirm = rpassword::prompt_password("Confirm password: ")
                .context("failed to read password confirmation")?;
            if pw != confirm {
                anyhow::bail!("passwords do not match");
            }
            pw
        };

        if password.is_empty() {
            anyhow::bail!("password cannot be empty");
        }
        if password.len() < min_len {
            anyhow::bail!("password must be at least {min_len} characters");
        }

        let hash = hash_password(&password).context("failed to hash password")?;
        let vault = SecretVaultBuilder::from_env().await.context("failed to open vault")?;
        let secret_id = user_secret_id(&self.username);
        let secret_name = secret_id.to_string();

        store_password_blob(&vault, &secret_id, &hash).await?;

        let auth = config.http.auth.get_or_insert_with(Default::default);
        auth.users.push(UserEntry {
            username: self.username.clone(),
            role: self.role.clone(),
            password_name: Some(secret_name.clone()),
            password_hash: None,
            revoked_after: None,
        });

        if let Err(e) = save_config(&config, config_path) {
            // Rollback: remove the vault secret so a retry with NewOnly won't fail.
            if let Err(err) = vault.delete(&secret_id).await {
                println!(
                    "{} rollback: failed to delete vault secret '{}': {err}",
                    "Warning".yellow().bold(),
                    secret_id
                );
            }
            return Err(e.context("save config failed"));
        }

        println!(
            "{} user '{}' with role '{}' (vault secret: '{}')",
            "Created".green().bold(),
            self.username,
            self.role,
            secret_name,
        );
        Ok(())
    }
}

impl RemoveUserCmd {
    async fn run(&self, config_path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(config_path)?;

        let auth = config
            .http
            .auth
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("auth is not configured".red()))?;

        let entry = auth
            .users
            .iter()
            .find(|u| u.username == self.username)
            .ok_or_else(|| anyhow::anyhow!("user '{}' not found in config", self.username))?
            .clone();

        if let Some(ref secret_name) = entry.password_name {
            match SecretVaultBuilder::from_env().await {
                Ok(vault) => {
                    let sid = SecretId::new(secret_name);
                    if vault.exists(&sid).await? {
                        vault.delete(&sid).await?;
                        println!("  {} vault secret '{secret_name}'", "Deleted".yellow());
                    }
                }
                Err(e) => {
                    println!(
                        "{}: could not open vault to clean up secret: {e}",
                        "Warning".yellow().bold()
                    );
                }
            }
        }

        auth.users.retain(|u| u.username != self.username);
        save_config(&config, config_path)?;

        println!("{} user '{}'", "Removed".red().bold(), self.username);
        Ok(())
    }
}

impl RevokeUserCmd {
    fn run(&self, config_path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(config_path)?;
        let auth = config
            .http
            .auth
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("auth is not configured".red()))?;

        let cutoff = self.at.unwrap_or_else(time_format::now);
        let user = auth
            .users
            .iter_mut()
            .find(|u| u.username == self.username)
            .ok_or_else(|| anyhow::anyhow!("user '{}' not found in config", self.username))?;

        user.revoked_after = Some(cutoff);
        save_config(&config, config_path)?;

        println!(
            "{} tokens for user '{}' revoked (revoked_after={})",
            "Updated".yellow().bold(),
            self.username,
            cutoff
        );
        Ok(())
    }
}

impl SetCmd {
    fn run(&self, config_path: &Path) -> anyhow::Result<()> {
        match &self.action {
            SetAction::Ttl(cmd) => cmd.run(config_path),
        }
    }
}

impl SetTtlCmd {
    fn run(&self, config_path: &Path) -> anyhow::Result<()> {
        if self.operator.is_none() && self.nominator.is_none() {
            anyhow::bail!("specify at least one of --operator or --nominator");
        }

        let mut config = AppConfig::load(config_path)?;
        let auth = config.http.auth.get_or_insert_with(Default::default);
        let mut changes = Vec::new();

        if let Some(ttl) = self.operator {
            auth.operator_token_ttl = ttl;
            changes.push(format!("operator={}s", ttl));
        }
        if let Some(ttl) = self.nominator {
            auth.nominator_token_ttl = ttl;
            changes.push(format!("nominator={}s", ttl));
        }

        save_config(&config, config_path)?;
        println!("{} Token TTL updated: {}", "OK".green().bold(), changes.join(", "));
        Ok(())
    }
}

fn list_users(config_path: &Path) -> anyhow::Result<()> {
    let config = AppConfig::load(config_path)?;

    if config.http.auth.is_none() {
        anyhow::bail!("{}", "auth is disabled (http.auth section removed from config). Use 'nodectl auth add' to enable.".red());
    }

    let users = config.http.auth.as_ref().map(|a| a.users.as_slice()).unwrap_or_default();

    println!("{:<20} {:<14} {}", "USERNAME".bold(), "ROLE".bold(), "SECRET".bold());
    println!("{}", "-".repeat(62));

    for entry in users {
        let secret = entry.password_name.as_deref().unwrap_or("-");
        println!("{:<20} {:<14} {}", entry.username, entry.role.as_str(), secret);
    }

    println!("\n{} user(s)", users.len());
    Ok(())
}
