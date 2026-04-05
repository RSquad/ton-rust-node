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
    utils::{save_config, warn_missing_secret},
};
use adnl::common::Timeouts;
use anyhow::Context;
use colored::Colorize;
use common::app_config::{AdnlConfig, AppConfig, KeyConfig, TimeoutVariant};
use control_client::client_adnl::ControlClientAdnl;
use secrets_vault::{vault::SecretVault, vault_builder::SecretVaultBuilder};
use std::{collections::HashMap, path::Path, sync::Arc};

#[derive(clap::Args, Clone)]
#[command(about = "Manage nodes in the configuration")]
pub struct NodeCmd {
    #[command(subcommand)]
    action: NodeAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum NodeAction {
    /// Add a node to the configuration
    Add(NodeAddCmd),
    /// List all configured nodes
    Ls(NodeLsCmd),
    /// Remove a node from the configuration
    Rm(NodeRmCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add a node to the configuration")]
pub struct NodeAddCmd {
    #[arg(short = 'n', long = "name", help = "Node name (unique identifier)")]
    name: String,
    #[arg(
        short = 'e',
        long = "control-server-endpoint",
        help = "Control server endpoint (IP:PORT)"
    )]
    control_server_endpoint: String,
    #[arg(
        short = 'p',
        long = "control-server-pubkey",
        help = "Control server public key (base64)"
    )]
    control_server_pubkey: String,
    #[arg(
        short = 's',
        long = "control-client-secret-name",
        help = "Vault secret name for ADNL client private key"
    )]
    control_client_secret_name: String,
}

#[derive(clap::Args, Clone)]
#[command(about = "List all configured nodes")]
pub struct NodeLsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::Args, Clone)]
#[command(about = "Remove a node from the configuration")]
pub struct NodeRmCmd {
    #[arg(short = 'n', long = "name", help = "Node name")]
    name: String,
}

impl NodeCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            NodeAction::Add(cmd) => cmd.run(path).await,
            NodeAction::Ls(cmd) => cmd.run(path).await,
            NodeAction::Rm(cmd) => cmd.run(path).await,
        }
    }
}

impl NodeAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if config.nodes.contains_key(&self.name) {
            anyhow::bail!(
                "Node '{}' already exists. Remove it first or use a different name.",
                self.name
            );
        }

        let server_key = KeyConfig::PublicKey {
            type_id: 1209251014,
            pub_key: base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &self.control_server_pubkey,
            )
            .context("Failed to decode control server public key")?,
        };

        let client_key = KeyConfig::VaultKey { name: self.control_client_secret_name.clone() };
        let adnl_config = AdnlConfig {
            server_address: self.control_server_endpoint.clone(),
            server_key,
            client_key,
            timeouts: TimeoutVariant::Single(Timeouts::DEFAULT_TIMEOUT.as_secs()),
        };
        config.nodes.insert(self.name.clone(), adnl_config);
        save_config(&config, path)?;

        let secret_exists_in_vault = match SecretVaultBuilder::from_env().await {
            Ok(vault) => {
                let secret_id = self.control_client_secret_name.as_str().into();
                vault.exists(&secret_id).await.ok()
            }
            Err(_) => None,
        };

        if secret_exists_in_vault == Some(false) {
            warn_missing_secret(&self.control_client_secret_name);
            println!();
        }
        println!("\n{} Node '{}' added\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct NodeView {
    name: String,
    control_server_endpoint: String,
    control_server_pubkey: String,
    control_client_secret: String,
    status: String,
}

impl NodeLsCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let config = AppConfig::load(path)?;

        if config.nodes.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No nodes configured".yellow()),
            }
            return Ok(());
        }

        let vault: Option<Arc<SecretVault>> = SecretVaultBuilder::from_env().await.ok();

        // Check connectivity for all nodes concurrently
        let mut set = tokio::task::JoinSet::new();
        for (name, adnl) in config.nodes.clone() {
            let vault = vault.clone();
            set.spawn(async move { (name, check_node_status(adnl, vault).await) });
        }

        let mut statuses: HashMap<String, Result<(), String>> = HashMap::new();
        while let Some(result) = set.join_next().await {
            if let Ok((name, status)) = result {
                statuses.insert(name, status);
            }
        }

        let mut views: Vec<NodeView> = config
            .nodes
            .into_iter()
            .map(|(name, adnl)| NodeView {
                control_server_pubkey: match &adnl.server_key {
                    KeyConfig::PublicKey { pub_key, .. } => {
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pub_key)
                    }
                    _ => "-".to_string(),
                },
                control_client_secret: match &adnl.client_key {
                    KeyConfig::VaultKey { name } => name.clone(),
                    _ => "-".to_string(),
                },
                status: match statuses.get(&name) {
                    Some(Ok(())) => "ok".to_string(),
                    Some(Err(msg)) => msg.clone(),
                    None => "unknown".to_string(),
                },
                control_server_endpoint: adnl.server_address,
                name,
            })
            .collect();
        views.sort_by(|a, b| a.name.cmp(&b.name));

        match self.format {
            OutputFormat::Json => print_nodes_json(&views)?,
            OutputFormat::Table => print_nodes_table(&views),
        }
        Ok(())
    }
}

fn print_nodes_json(views: &[NodeView]) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(views)?);
    Ok(())
}

fn print_nodes_table(views: &[NodeView]) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "Nodes:".green(), views.len());
    println!(
        "  {:<20} {:<25} {:<48} {:<30} {}",
        "Name".cyan().bold(),
        "Control Server Endpoint".cyan().bold(),
        "Control Server Pubkey".cyan().bold(),
        "Control Client Secret".cyan().bold(),
        "Status".cyan().bold(),
    );
    println!("  {}", "─".repeat(150).dimmed());

    for v in views {
        let status_display = match v.status.as_str() {
            "ok" => "OK".green().to_string(),
            "unknown" => "unknown".dimmed().to_string(),
            msg => msg.red().to_string(),
        };
        println!(
            "  {:<20} {:<25} {:<48} {:<30} {}",
            v.name,
            v.control_server_endpoint,
            v.control_server_pubkey,
            v.control_client_secret,
            status_display,
        );
    }
    println!();
}

const STATUS_CHECK_TIMEOUT_SECS: u64 = 5;

async fn check_node_status(
    mut adnl: AdnlConfig,
    vault: Option<Arc<SecretVault>>,
) -> Result<(), String> {
    adnl.timeouts = TimeoutVariant::Single(STATUS_CHECK_TIMEOUT_SECS);

    let adnl_config =
        adnl.to_node_adnl_config(vault).await.map_err(|e| e.root_cause().to_string())?;

    let mut client = ControlClientAdnl::new(adnl_config, 1);
    client.connect().await.map_err(|e| e.root_cause().to_string())?;
    client.ping().await.map_err(|e| e.root_cause().to_string())?;
    let _ = client.shutdown().await;

    Ok(())
}

impl NodeRmCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if config.nodes.remove(&self.name).is_none() {
            anyhow::bail!("Node '{}' not found in configuration", self.name);
        }

        save_config(&config, path)?;

        println!("\n{} Node '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}
