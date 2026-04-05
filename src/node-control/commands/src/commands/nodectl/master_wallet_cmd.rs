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
    utils::{load_config_vault_rpc_client, wallet_info},
};
use colored::Colorize;
use common::{
    app_config::{KeyConfig, WalletConfig},
    ton_utils::display_tons,
};
use secrets_vault::{types::secret::Secret, vault::SecretVault};
use std::{path::Path, sync::Arc};
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

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
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            MasterWalletAction::Info(cmd) => cmd.run(path).await,
        }
    }
}

#[derive(serde::Serialize)]
struct MasterWalletView {
    address: Option<String>,
    balance: Option<String>,
    state: Option<String>,
    version: String,
    subwallet_id: u32,
    secret: String,
    public_key: Option<String>,
}

impl MasterWalletInfoCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

        let master_wallet = config
            .master_wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("master_wallet is not configured"))?;

        let secret_name = match &master_wallet.key {
            KeyConfig::VaultKey { name } => name.clone(),
            _ => "-".to_string(),
        };

        match self.format {
            OutputFormat::Json => {
                print_master_wallet_json(master_wallet, &secret_name, rpc_client, vault).await?
            }
            OutputFormat::Table => {
                print_master_wallet_table(master_wallet, &secret_name, rpc_client, vault).await?
            }
        }

        Ok(())
    }
}

async fn print_master_wallet_json(
    master_wallet: &WalletConfig,
    secret_name: &str,
    rpc_client: Arc<ClientJsonRpc>,
    vault: Arc<SecretVault>,
) -> anyhow::Result<()> {
    let (address, state, balance, public_key) =
        match wallet_info(rpc_client, master_wallet, vault).await {
            Ok((address, info, secret)) => {
                let pk = if let Secret::KeyPair { keypair } = secret {
                    keypair.public_key().await.ok().flatten().map(|pk| {
                        base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            pk.as_slice(),
                        )
                    })
                } else {
                    None
                };
                (
                    Some(address.to_string()),
                    Some(info.account_state.to_string()),
                    Some(display_tons(info.balance)),
                    pk,
                )
            }
            Err(_) => (None, None, None, None),
        };

    let view = MasterWalletView {
        address,
        balance,
        state,
        version: master_wallet.version.to_string(),
        subwallet_id: master_wallet.subwallet_id,
        secret: secret_name.to_string(),
        public_key,
    };
    println!("{}", serde_json::to_string_pretty(&view)?);
    Ok(())
}

async fn print_master_wallet_table(
    master_wallet: &WalletConfig,
    secret_name: &str,
    rpc_client: Arc<ClientJsonRpc>,
    vault: Arc<SecretVault>,
) -> anyhow::Result<()> {
    let (address, account_state, balance, public_key) =
        match wallet_info(rpc_client, master_wallet, vault).await {
            Ok((address, info, secret)) => (
                address.to_string().white(),
                info.account_state.to_string().white(),
                display_tons(info.balance).white(),
                if let Secret::KeyPair { keypair } = secret {
                    let public_key = keypair
                        .public_key()
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no public key"))?;
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        public_key.as_slice(),
                    )
                    .white()
                } else {
                    "unknown".red()
                },
            ),
            Err(_) => ("unknown".red(), "unknown".red(), "unknown".red(), "unknown".red()),
        };

    println!("\n{} {}\n", "OK".green().bold(), "Master Wallet".green());
    println!("  {:<16} {}", "Address:".cyan().bold(), address);
    println!("  {:<16} {}", "Balance:".cyan().bold(), balance);
    println!("  {:<16} {}", "State:".cyan().bold(), account_state);
    println!("  {:<16} {}", "Version:".cyan().bold(), master_wallet.version);
    println!("  {:<16} {}", "Subwallet ID:".cyan().bold(), master_wallet.subwallet_id);
    println!("  {:<16} {}", "Secret:".cyan().bold(), secret_name);
    println!("  {:<16} {}", "Public Key:".cyan().bold(), public_key);
    println!();
    Ok(())
}
