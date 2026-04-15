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
    utils::{api_delete, api_get, api_post, resolve_service_url},
};
use colored::{ColoredString, Colorize};
use common::ton_utils::display_tons;
use std::str::FromStr;
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, MsgAddressInt};

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
            PoolAction::Add(cmd) => cmd.run(url, token, config_path).await,
            PoolAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            PoolAction::Rm(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

#[derive(serde::Serialize)]
struct PoolAddBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
}

impl PoolAddCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = PoolAddBody {
            name: &self.name,
            address: self.address.as_deref(),
            owner: self.owner.as_deref(),
        };
        api_post(&base_url, "/v1/pools", token, &body).await?;

        let info = match (&self.address, &self.owner) {
            (Some(a), Some(o)) => format!("address='{}', owner='{}'", a, o),
            (Some(a), None) => format!("address='{}'", a),
            (None, Some(o)) => format!("owner='{}' (address will be calculated on bind)", o),
            (None, None) => String::new(),
        };
        println!("\n{} Pool '{}' added ({})\n", "OK".green().bold(), self.name, info);
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PoolView {
    name: String,
    kind: String,
    balance: Option<u64>,
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

fn display_ton_address(addr: Option<&str>) -> ColoredString {
    addr.map(|s| {
        MsgAddressInt::from_str(s)
            .and_then(|a| a.to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE))
            .unwrap_or_else(|_| s.to_string())
    })
    .map(|s| s.white())
    .unwrap_or_else(|| "-".red())
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
                let display_addr = display_ton_address(v.address.as_deref());
                let display_owner = display_ton_address(v.owner.as_deref());
                let display_balance =
                    v.balance.map(|b| display_tons(b).white()).unwrap_or_else(|| "-".red());

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

impl PoolRmCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_delete(&base_url, &format!("/v1/pools/{}", self.name), token).await?;
        println!("\n{} Pool '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}
