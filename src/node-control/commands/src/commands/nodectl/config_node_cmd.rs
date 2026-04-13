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
    utils::{api_delete, api_get, api_post, resolve_service_url, warn_missing_secret},
};
use colored::Colorize;
use secrets_vault::vault_builder::SecretVaultBuilder;

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
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            NodeAction::Add(cmd) => cmd.run(url, token, config_path).await,
            NodeAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            NodeAction::Rm(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

#[derive(serde::Serialize)]
struct NodeAddBody<'a> {
    name: &'a str,
    control_server_endpoint: &'a str,
    control_server_pubkey: &'a str,
    control_client_secret: &'a str,
}

impl NodeAddCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = NodeAddBody {
            name: &self.name,
            control_server_endpoint: &self.control_server_endpoint,
            control_server_pubkey: &self.control_server_pubkey,
            control_client_secret: &self.control_client_secret_name,
        };
        api_post(&base_url, "/v1/nodes", token, &body).await?;

        if vault_secret_missing(&self.control_client_secret_name).await {
            warn_missing_secret(&self.control_client_secret_name);
            println!();
        }
        println!("\n{} Node '{}' added\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

/// Best-effort check that returns `true` only if we could reach the local vault
/// AND the secret is definitely absent. Any other outcome (no vault, lookup
/// error) is treated as "unknown" and produces no warning.
async fn vault_secret_missing(secret_name: &str) -> bool {
    match SecretVaultBuilder::from_env().await {
        Ok(vault) => vault.exists(&secret_name.into()).await.ok() == Some(false),
        Err(_) => false,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NodeView {
    name: String,
    control_server_endpoint: String,
    control_server_pubkey: String,
    control_client_secret: String,
    status: String,
}

impl NodeLsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/nodes", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let views: Vec<NodeView> = serde_json::from_value(resp["result"].clone())?;

        if views.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No nodes configured".yellow()),
            }
            return Ok(());
        }

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

impl NodeRmCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_delete(&base_url, &format!("/v1/nodes/{}", self.name), token).await?;
        println!("\n{} Node '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}
