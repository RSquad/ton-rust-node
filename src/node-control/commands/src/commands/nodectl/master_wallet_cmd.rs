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
    utils::{api_get, resolve_service_url},
};
use colored::Colorize;
use common::ton_utils::display_tons;

#[derive(clap::Args, Clone)]
#[command(about = "Master wallet info")]
pub struct MasterWalletCmd {
    #[command(subcommand)]
    action: MasterWalletAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum MasterWalletAction {
    /// Show master wallet info (address, version, etc.)
    Info(MasterWalletInfoCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Show master wallet info")]
pub struct MasterWalletInfoCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

impl MasterWalletCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            MasterWalletAction::Info(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MasterWalletView {
    address: Option<String>,
    balance: Option<u64>,
    state: Option<String>,
    version: String,
    subwallet_id: u32,
    secret: String,
    public_key: Option<String>,
}

impl MasterWalletInfoCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/master-wallet", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let view: MasterWalletView = serde_json::from_value(resp["result"].clone())?;

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&view)?);
            }
            OutputFormat::Table => {
                print_master_wallet_table_from_view(&view);
            }
        }

        Ok(())
    }
}

fn print_master_wallet_table_from_view(view: &MasterWalletView) {
    println!("\n{} {}\n", "OK".green().bold(), "Master Wallet".green());
    println!("  {:<16} {}", "Address:".cyan().bold(), view.address.as_deref().unwrap_or("unknown"));
    println!(
        "  {:<16} {}",
        "Balance:".cyan().bold(),
        view.balance.map(|b| display_tons(b)).unwrap_or("unknown".to_string())
    );
    println!("  {:<16} {}", "State:".cyan().bold(), view.state.as_deref().unwrap_or("unknown"));
    println!("  {:<16} {}", "Version:".cyan().bold(), view.version);
    println!("  {:<16} {}", "Subwallet ID:".cyan().bold(), view.subwallet_id);
    println!("  {:<16} {}", "Secret:".cyan().bold(), view.secret);
    println!(
        "  {:<16} {}",
        "Public Key:".cyan().bold(),
        view.public_key.as_deref().unwrap_or("unknown")
    );
    println!();
}
