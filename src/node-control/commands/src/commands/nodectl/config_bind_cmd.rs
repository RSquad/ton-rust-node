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
use colored::Colorize;

#[derive(clap::Args, Clone)]
#[command(about = "Manage node bindings in the configuration")]
pub struct BindCmd {
    #[command(subcommand)]
    action: BindAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum BindAction {
    /// Bind a wallet (and optionally a pool) to a node
    Add(BindAddCmd),
    /// Remove a node binding (only allowed when binding is in idle state)
    Rm(BindRmCmd),
    /// List all node bindings
    Ls(BindLsCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Bind a wallet and optional pool to a node")]
pub struct BindAddCmd {
    #[arg(short = 'n', long = "node", help = "Node name (must exist in nodes)")]
    node: String,
    #[arg(short = 'w', long = "wallet", help = "Wallet name (must exist in wallets)")]
    wallet: String,
    #[arg(short = 'p', long = "pool", help = "Pool name (optional, must exist in pools)")]
    pool: Option<String>,
}

#[derive(clap::Args, Clone)]
#[command(about = "Remove a node binding")]
pub struct BindRmCmd {
    #[arg(short = 'n', long = "node", help = "Node name to remove from bindings")]
    node: String,
}

#[derive(clap::Args, Clone)]
#[command(about = "List all node bindings")]
pub struct BindLsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

impl BindCmd {
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            BindAction::Add(cmd) => cmd.run(url, token, config_path).await,
            BindAction::Rm(cmd) => cmd.run(url, token, config_path).await,
            BindAction::Ls(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

#[derive(serde::Serialize)]
struct BindingAddBody<'a> {
    node: &'a str,
    wallet: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<&'a str>,
}

impl BindAddCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body =
            BindingAddBody { node: &self.node, wallet: &self.wallet, pool: self.pool.as_deref() };
        api_post(&base_url, "/v1/bindings", token, &body).await?;

        let pool_info = self.pool.as_deref().map(|p| format!(", pool='{}'", p)).unwrap_or_default();
        println!(
            "\n{} Binding added: node='{}', wallet='{}'{}\n",
            "OK".green().bold(),
            self.node,
            self.wallet,
            pool_info
        );
        Ok(())
    }
}

impl BindRmCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_delete(&base_url, &format!("/v1/bindings/{}", self.node), token).await?;
        println!("\n{} Binding {} removed\n", "OK".green().bold(), self.node);
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BindingView {
    node: String,
    wallet: String,
    pool: Option<String>,
    enable: bool,
    status: String,
}

impl BindLsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/bindings", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let views: Vec<BindingView> = serde_json::from_value(resp["result"].clone())?;

        if views.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No bindings configured".yellow()),
            }
            return Ok(());
        }

        match self.format {
            OutputFormat::Json => print_bindings_json(&views)?,
            OutputFormat::Table => print_bindings_table(&views),
        }
        Ok(())
    }
}

fn print_bindings_json(views: &[BindingView]) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(views)?);
    Ok(())
}

fn print_bindings_table(views: &[BindingView]) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "Bindings:".green(), views.len());
    println!(
        "  {:<20} {:<20} {:<20} {:<12} {}",
        "Node".cyan().bold(),
        "Wallet".cyan().bold(),
        "Pool".cyan().bold(),
        "Enable".cyan().bold(),
        "Status".cyan().bold(),
    );
    println!("  {}", "─".repeat(90).dimmed());

    for v in views {
        let enable_str = if v.enable { "yes".green().to_string() } else { "no".red().to_string() };
        println!(
            "  {:<20} {:<20} {:<20} {:<21} {}",
            v.node,
            v.wallet,
            v.pool.as_deref().unwrap_or("-"),
            enable_str,
            v.status,
        );
    }
    println!();
}
