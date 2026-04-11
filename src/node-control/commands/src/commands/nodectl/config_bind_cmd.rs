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
use common::app_config::{AppConfig, BindingStatus, NodeBinding};
use std::path::Path;

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
            BindAction::Add(cmd) => cmd.run(require_config(config_path)?).await,
            BindAction::Rm(cmd) => cmd.run(require_config(config_path)?).await,
            BindAction::Ls(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

impl BindAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if !config.nodes.contains_key(&self.node) {
            anyhow::bail!("Node '{}' not found in configuration", self.node);
        }

        if !config.wallets.contains_key(&self.wallet) {
            anyhow::bail!("Wallet '{}' not found in configuration", self.wallet);
        }

        if let Some(pool_name) = &self.pool {
            if !config.pools.contains_key(pool_name) {
                anyhow::bail!("Pool '{}' not found in configuration", pool_name);
            }
            for (node_name, binding) in &config.bindings {
                if binding.pool.as_deref() == Some(pool_name) && *node_name != self.node {
                    anyhow::bail!(
                        "Pool '{}' is already bound to node '{}'. A pool can only be bound to one node.",
                        pool_name,
                        node_name
                    );
                }
            }
        }

        let binding = NodeBinding {
            wallet: self.wallet.clone(),
            pool: self.pool.clone(),
            enable: false,
            status: Default::default(),
        };
        config.bindings.insert(self.node.clone(), binding);
        save_config(&config, path)?;

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
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        let binding = config
            .bindings
            .get(&self.node)
            .ok_or_else(|| anyhow::anyhow!("Binding for node '{}' not found", self.node))?;

        if binding.status != BindingStatus::Idle {
            anyhow::bail!(
                "Cannot remove binding for node '{}': status is '{}', must be 'idle'. \
                 Disable elections first and wait for stake recovery to complete.",
                self.node,
                binding.status
            );
        }

        config.bindings.remove(&self.node);
        save_config(&config, path)?;

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
