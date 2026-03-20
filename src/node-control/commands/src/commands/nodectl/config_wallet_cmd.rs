/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::utils::{
    SEND_TIMEOUT, get_wallet_config, load_config_vault, load_config_vault_rpc_client, make_wallet,
    save_config, wait_for_seqno_change, wallet_info, warn_missing_secret,
};
use anyhow::Context;
use colored::Colorize;
use common::{
    TonWalletVersion,
    app_config::{AppConfig, KeyConfig, WalletConfig},
    task_cancellation::CancellationCtx,
    ton_utils::{nanotons_to_tons_f64, tons_f64_to_nanotons},
};
use contracts::TonWallet;
use secrets_vault::errors::error::VaultError;
use std::{io::Write, path::Path};
use ton_block::{Cell, MsgAddressInt, write_boc};
use ton_http_api_client::v2::data_models::AccountState;

const WALLET_SEND_GAS: u64 = 1_000_000; // 0.001 TON

#[derive(clap::Args, Clone)]
#[command(about = "Manage wallets in the configuration")]
pub struct WalletCmd {
    #[command(subcommand)]
    action: WalletAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum WalletAction {
    /// Add a wallet to the configuration
    Add(WalletAddCmd),
    /// List all configured wallets
    Ls(WalletLsCmd),
    /// Remove a wallet from the configuration
    Rm(WalletRmCmd),
    /// Send TONs
    Send(WalletSendCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add a wallet to the configuration")]
pub struct WalletAddCmd {
    #[arg(short = 'n', long = "name", help = "Wallet name (unique identifier)")]
    name: String,
    #[arg(short = 's', long = "secret-name", help = "Vault secret name for wallet key")]
    secret_name: String,
    #[arg(
        short = 'v',
        long = "version",
        default_value = "V3R2",
        help = "Wallet version (case insensitive) [possible values: V1R3, V3R2, V4R2, V5R1]"
    )]
    version: TonWalletVersion,
    #[arg(short = 'i', long = "subwallet-id", default_value = "42", help = "Subwallet ID")]
    subwallet_id: u32,
    #[arg(short = 'w', long = "workchain", default_value = "-1", help = "Workchain ID")]
    workchain: i32,
}

#[derive(clap::Args, Clone)]
#[command(about = "List all configured wallets")]
pub struct WalletLsCmd {}

#[derive(clap::Args, Clone)]
#[command(about = "Remove a wallet from the configuration")]
pub struct WalletRmCmd {
    #[arg(short = 'n', long = "name", help = "Wallet name")]
    name: String,
}

#[derive(clap::Args, Clone)]
#[command(about = "Send TON")]
pub struct WalletSendCmd {
    #[arg(short = 'f', long = "from", help = "Wallet name")]
    from: String,
    #[arg(short = 't', long = "to", help = "Destination address")]
    to: String,
    #[arg(short = 'a', long = "amount", help = "Amount in TONs")]
    amount: f64,
    #[arg(
        short = 'b',
        long = "bounce",
        help = "Bounce transfer to the sender if recipient fails to process it"
    )]
    bounce: bool,
}

impl WalletCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        match &self.action {
            WalletAction::Add(cmd) => cmd.run(path).await,
            WalletAction::Ls(cmd) => cmd.run(path).await,
            WalletAction::Rm(cmd) => cmd.run(path).await,
            WalletAction::Send(cmd) => cmd.run(path, cancellation_ctx).await,
        }
    }
}

impl WalletAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.name == "master_wallet" {
            anyhow::bail!("'master_wallet' is a reserved name");
        }

        let (config, vault) = load_config_vault(path).await?;

        if config.wallets.contains_key(&self.name) {
            anyhow::bail!(
                "Wallet '{}' already exists. Remove it first or use a different name.",
                self.name
            );
        }

        let wallet_config = WalletConfig {
            key: KeyConfig::VaultKey { name: self.secret_name.clone() },
            version: self.version,
            subwallet_id: self.subwallet_id,
            workchain: self.workchain,
        };

        let secret_id = self.secret_name.as_str().into();

        if !vault.exists(&secret_id).await? {
            warn_missing_secret(&self.secret_name);
        }

        let mut config = config.clone();
        config.wallets.insert(self.name.clone(), wallet_config);
        save_config(&config, path)?;

        println!("\n{} Wallet '{}' added\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

impl WalletLsCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

        let mut all_wallets: Vec<(&str, &WalletConfig)> =
            config.wallets.iter().map(|(k, v)| (k.as_str(), v)).collect();
        if let Some(mw) = config.master_wallet.as_ref() {
            all_wallets.push(("master_wallet", mw));
        }

        if all_wallets.is_empty() {
            println!("\n{}\n", "No wallets configured".yellow());
            return Ok(());
        }

        println!("\n{} {} ({})\n", "OK".green().bold(), "Wallets:".green(), all_wallets.len());
        println!(
            "  {:<20} {:<22} {:<8} {:<12} {:<12} {}",
            "Name".cyan().bold(),
            "Secret".cyan().bold(),
            "Version".cyan().bold(),
            "State".cyan().bold(),
            "Balance".cyan().bold(),
            "Address".cyan().bold(),
        );
        println!("  {}", "─".repeat(120).dimmed());

        for (name, wallet_cfg) in &all_wallets {
            let (address, account_state, balance) =
                match wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await {
                    Ok((address, wallet_info, _)) => (
                        address.to_string(),
                        wallet_info.account_state.to_string(),
                        format!("{:.4}", nanotons_to_tons_f64(wallet_info.balance)),
                    ),
                    Err(e) => {
                        if e.downcast_ref::<VaultError>()
                            .is_some_and(|e| e.code() == VaultError::NOT_FOUND)
                        {
                            ("not found in the vault".to_string(), "".to_string(), "".to_string())
                        } else {
                            (e.to_string(), "".to_string(), "".to_string())
                        }
                    }
                };

            let secret_name = match &wallet_cfg.key {
                KeyConfig::VaultKey { name } => name.clone(),
                _ => "-".to_string(),
            };
            println!(
                "  {:<20} {:<22} {:<8} {:<12} {:<12} {}",
                name, secret_name, wallet_cfg.version, account_state, balance, address,
            );
        }
        println!();
        Ok(())
    }
}

impl WalletRmCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if !config.wallets.contains_key(&self.name) {
            anyhow::bail!("Wallet '{}' not found in configuration", self.name);
        }

        for (node_name, binding) in &config.bindings {
            if binding.wallet == self.name {
                anyhow::bail!(
                    "Cannot remove wallet '{}': referenced by binding for node '{}'",
                    self.name,
                    node_name
                );
            }
        }

        config.wallets.remove(&self.name);
        save_config(&config, path)?;

        println!("\n{} Wallet '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

impl WalletSendCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

        let from_wallet_cfg =
            get_wallet_config(&self.from, &config.wallets, config.master_wallet.as_ref())?;

        let (from_wallet_address, from_wallet_info, from_secret) =
            wallet_info(rpc_client.clone(), from_wallet_cfg, vault.clone()).await?;

        if !(1..=from_wallet_info.balance.saturating_sub(WALLET_SEND_GAS))
            .contains(&tons_f64_to_nanotons(self.amount))
        {
            anyhow::bail!(
                "Wrong amount value {}TON. Wallet balance is {}TON",
                self.amount,
                nanotons_to_tons_f64(from_wallet_info.balance)
            )
        }

        let to_wallet_address =
            self.to.parse::<MsgAddressInt>().context("Invalid destination address")?;

        let from_wallet =
            make_wallet(rpc_client.clone(), from_wallet_cfg, from_secret, &self.from).await?;

        if from_wallet_info.account_state == AccountState::Frozen {
            anyhow::bail!("wallet '{}' is frozen", self.from);
        }

        if from_wallet_info.account_state == AccountState::Uninitialized {
            anyhow::bail!("wallet '{}' is uninitialized", self.from);
        }

        println!(
            "\n{}\n  From:   {} ({})\n  To:     {}\n  Amount: {:.9} TON\n  Bounce: {}\n",
            "Transfer summary:".cyan().bold(),
            self.from,
            from_wallet_address,
            to_wallet_address,
            self.amount,
            self.bounce,
        );

        print!("Confirm transfer? [y/N]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            println!("{}", "Transfer cancelled".yellow());
            return Ok(());
        }

        let msg = from_wallet
            .build_message(
                to_wallet_address,
                tons_f64_to_nanotons(self.amount),
                Cell::default(),
                self.bounce,
                None,
                None,
                None,
            )
            .await?;

        let msg_boc = write_boc(&msg)?;
        rpc_client.send_boc(&msg_boc).await?;

        wait_for_seqno_change(
            rpc_client.clone(),
            &from_wallet_address,
            from_wallet_info.seqno,
            &cancellation_ctx,
            SEND_TIMEOUT,
        )
        .await?;

        println!("{} Transfer complete", "OK".green().bold());
        Ok(())
    }
}
