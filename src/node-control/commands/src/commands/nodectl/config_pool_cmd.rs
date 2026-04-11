/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::{
    output_format::OutputFormat,
    utils::{api_get, require_config, resolve_service_url, save_config},
};
use colored::Colorize;
use common::app_config::{AppConfig, PoolConfig};
use std::{path::Path, str::FromStr};
use ton_block::MsgAddressInt;

#[derive(clap::Args, Clone)]
#[command(about = "Manage pools in the configuration")]
pub struct PoolCmd {
    #[command(subcommand)]
    action: PoolAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum PoolAction {
    /// Add a pool to the configuration
    Add(PoolAddCmd),
    /// List all configured pools
    Ls(PoolLsCmd),
    /// Remove a pool from the configuration
    Rm(PoolRmCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add a pool to the configuration")]
pub struct PoolAddCmd {
    #[arg(short = 'n', long = "name", help = "Pool name (unique identifier)")]
    name: String,
    #[arg(
        short = 'a',
        long = "address",
        help = "Pool contract address, raw or base64url (if already deployed)"
    )]
    address: Option<String>,
    #[arg(
        short = 'o',
        long = "owner",
        help = "Owner address, raw or base64url (for deployment/verification)"
    )]
    owner: Option<String>,
}

#[derive(clap::Args, Clone)]
#[command(about = "List all configured pools")]
pub struct PoolLsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::Args, Clone)]
#[command(about = "Remove a pool from the configuration")]
pub struct PoolRmCmd {
    #[arg(short = 'n', long = "name", help = "Pool name")]
    name: String,
}

impl PoolCmd {
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            PoolAction::Add(cmd) => cmd.run(require_config(config_path)?).await,
            PoolAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            PoolAction::Rm(cmd) => cmd.run(require_config(config_path)?).await,
        }
    }
}

impl PoolAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.address.is_none() && self.owner.is_none() {
            anyhow::bail!("At least one of --address or --owner must be specified");
        }

        let normalized_address = self
            .address
            .as_deref()
            .map(|addr| normalize_ton_address(addr, "address"))
            .transpose()?;
        let normalized_owner =
            self.owner.as_deref().map(|owner| normalize_ton_address(owner, "owner")).transpose()?;

        let mut config = AppConfig::load(path)?;

        if config.pools.contains_key(&self.name) {
            anyhow::bail!(
                "Pool '{}' already exists. Remove it first or use a different name.",
                self.name
            );
        }

        let pool_config = PoolConfig::SNP {
            address: normalized_address.clone(),
            owner: normalized_owner.clone(),
        };
        config.pools.insert(self.name.clone(), pool_config);
        save_config(&config, path)?;

        let info = match (&normalized_address, &normalized_owner) {
            (Some(a), Some(o)) => format!("address='{}', owner='{}'", a, o),
            (Some(a), None) => format!("address='{}'", a),
            (None, Some(o)) => format!("owner='{}' (address will be calculated on bind)", o),
            _ => unreachable!(),
        };
        println!("\n{} Pool '{}' added ({})\n", "OK".green().bold(), self.name, info);
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PoolView {
    name: String,
    kind: String,
    balance: Option<String>,
    address: Option<String>,
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    addresses: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validator_share: Option<u64>,
}

impl PoolLsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/pools", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let views: Vec<PoolView> = serde_json::from_value(resp["result"].clone())?;

        if views.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No pools configured".yellow()),
            }
            return Ok(());
        }

        match self.format {
            OutputFormat::Json => print_pools_json(&views)?,
            OutputFormat::Table => print_pools_table(&views),
        }
        Ok(())
    }
}

fn print_pools_json(views: &[PoolView]) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(views)?);
    Ok(())
}

fn print_pools_table(views: &[PoolView]) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "Pools:".green(), views.len());
    println!(
        "  {:<15} {:<6} {:<14} {:<50} {}",
        "Name".cyan().bold(),
        "Kind".cyan().bold(),
        "Balance".cyan().bold(),
        "Address".cyan().bold(),
        "Owner".cyan().bold(),
    );
    println!("  {}", "─".repeat(137).dimmed());

    for v in views {
        match v.kind.as_str() {
            "SNP" => {
                let display_addr =
                    v.address.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                let display_owner =
                    v.owner.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                let display_balance =
                    v.balance.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());

                println!(
                    "  {:<15} {:<6} {:<14} {:<50} {}",
                    v.name, "SNP", display_balance, display_addr, display_owner,
                );
            }
            "Core" => {
                let addrs = v.addresses.as_deref().map(|a| a.join(", ")).unwrap_or_default();
                let share = v.validator_share.map(|s| s.to_string()).unwrap_or_default();
                println!(
                    "  {:<15} {:<6} {:<14} {:<50} share={}",
                    v.name, "Core", "-", addrs, share,
                );
            }
            _ => {}
        }
    }
    println!();
}

fn normalize_ton_address(addr: &str, flag_name: &str) -> anyhow::Result<String> {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--{flag_name} must not be empty");
    }
    MsgAddressInt::from_str(trimmed).map_err(|_| {
        anyhow::anyhow!(
            "invalid TON address for --{flag_name}: '{trimmed}'. Expected format: raw address or base64url"
        )
    })?;
    Ok(trimmed.to_string())
}

#[cfg(test)]
fn validate_ton_address(addr: &str, flag_name: &str) -> anyhow::Result<()> {
    normalize_ton_address(addr, flag_name).map(|_| ())
}

impl PoolRmCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if !config.pools.contains_key(&self.name) {
            anyhow::bail!("Pool '{}' not found in configuration", self.name);
        }

        for (node_name, binding) in &config.bindings {
            if binding.pool.as_deref() == Some(&self.name) {
                anyhow::bail!(
                    "Cannot remove pool '{}': referenced by binding for node '{}'",
                    self.name,
                    node_name
                );
            }
        }

        config.pools.remove(&self.name);
        save_config(&config, path)?;

        println!("\n{} Pool '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE};

    #[test]
    fn test_validate_ton_address_valid_raw() {
        assert!(
            validate_ton_address(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb",
                "owner",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_ton_address_valid_masterchain() {
        assert!(
            validate_ton_address(
                "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea",
                "address",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_ton_address_valid_base64url() {
        // Round-trip: raw -> MsgAddressInt -> base64url -> validate
        let raw = "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb";
        let addr = MsgAddressInt::from_str(raw).unwrap();
        let base64url = addr.to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE).unwrap();
        assert!(validate_ton_address(&base64url, "owner").is_ok());
    }

    #[test]
    fn test_validate_ton_address_empty() {
        let err = validate_ton_address("", "owner").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_validate_ton_address_whitespace() {
        let err = validate_ton_address("   ", "owner").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_validate_ton_address_invalid() {
        let err = validate_ton_address("not-an-address", "owner").unwrap_err();
        assert!(err.to_string().contains("invalid TON address"));
    }

    #[test]
    fn test_validate_ton_address_valid_with_surrounding_spaces() {
        assert!(
            validate_ton_address(
                "  0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb  ",
                "owner",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_normalize_ton_address_trims_surrounding_spaces() {
        let normalized = normalize_ton_address(
            "  0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb  ",
            "owner",
        )
        .unwrap();
        assert_eq!(
            normalized,
            "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb"
        );
    }
}
