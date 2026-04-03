/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::utils::{
    DEPLOY_TIMEOUT, load_config_vault_rpc_client, make_wallet, wait_for_deploy, wallet_info,
};
use colored::Colorize;
use common::{
    TonWalletVersion,
    app_config::PoolConfig,
    task_cancellation::CancellationCtx,
    ton_utils::{nanotons_to_tons_f64, tons_f64_to_nanotons},
};
use contracts::{NominatorWrapperImpl, TonWallet, resolve_toncore_pool};
use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc, sync::Arc};
use ton_block::{Cell, MsgAddressInt, write_boc};
use ton_http_api_client::v2::data_models::AccountState;

#[derive(clap::Args, Clone)]
#[command(about = "Deploy contracts (wallet, nominator pool)")]
pub struct DeployCmd {
    #[command(subcommand)]
    action: DeployAction,
}

#[derive(clap::Subcommand, Clone)]
enum DeployAction {
    Wallet(DeployWalletsCmd),
    Pool(DeployPoolCmd),
}

#[derive(clap::Args, Clone)]
#[command(group(clap::ArgGroup::new("target").required(true).args(&["node", "all"])))]
struct DeployWalletsCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[arg(long = "verbose", help = "Print progress", required = false)]
    verbose: bool,

    #[arg(long = "node", help = "Node name to get the wallet to deploy")]
    node: Option<String>,

    #[arg(long = "all", help = "Deploy all wallets")]
    all: bool,
}

#[derive(clap::Args, Clone)]
struct DeployPoolCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[arg(long = "verbose", help = "Print progress", required = false)]
    verbose: bool,
    #[arg(
        long = "owner",
        help = "SNP: pool owner address (required unless deploying a `kind: core` pool)"
    )]
    owner: Option<MsgAddressInt>,
    #[arg(
        long = "pool",
        help = "Pool name in config (defaults to the `pool` field of this node's binding)"
    )]
    pool: Option<String>,
    #[arg(long = "amount", help = "Amount of TONs to transfer")]
    amount: f64,
    #[arg(long = "node", help = "Node ID")]
    node: String,
}

impl DeployCmd {
    pub async fn run(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        match &self.action {
            DeployAction::Wallet(cmd) => cmd.run(cancellation_ctx).await,
            DeployAction::Pool(cmd) => cmd.run(cancellation_ctx).await,
        }
    }
}

#[derive(serde::Serialize)]
struct DeployWalletResult {
    pub config: String,
    pub details: HashMap<String, DeployWalletResultDetails>,
}

#[derive(serde::Serialize)]
struct DeployWalletResultDetails {
    pub version: TonWalletVersion,
    pub subwallet_id: u32,
    pub workchain: i32,
    pub account_state: AccountState,
    pub address: String,
    pub balance: u64,
    pub deployed: bool,
    pub error: Option<String>,
}

impl DeployWalletsCmd {
    const MIN_BALANCE: u64 = 100_000_000;

    pub async fn run(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let mut uninitialized_wallets: Vec<(Arc<dyn TonWallet>, u64, String)> = Vec::new();
        let (config, vault, rpc_client) =
            load_config_vault_rpc_client(Path::new(&self.config)).await?;
        let mut res_details = HashMap::new();

        if self.verbose {
            println!(
                "  {:<10} {:<7} {:<12} {:<9} {:<14} {}",
                "Node".bold(),
                "Version".bold(),
                "Subwallet ID".bold(),
                "Workchain".bold(),
                "State".bold(),
                "Address:".bold(),
            );

            println!("  {}", "─".repeat(124).dimmed());
        }

        // Get wallets account state
        for (node_id, wallet_cfg) in &config.wallets {
            if cancellation_ctx.is_cancelled() {
                anyhow::bail!("Task cancelled");
            }

            if let Some(node_filter) = &self.node
                && node_filter != node_id
            {
                continue;
            }

            let (wallet_address, wallet_info, secret) =
                wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await?;

            if self.verbose {
                println!(
                    "  {:<10} {:<7} {:<12} {:<9} {:<14} {}",
                    node_id,
                    format!("{}", wallet_cfg.version),
                    wallet_cfg.subwallet_id,
                    wallet_cfg.workchain,
                    format!("{}", &wallet_info.account_state),
                    wallet_address
                );
            }

            let mut deployed = false;
            if wallet_info.account_state == AccountState::Uninitialized {
                let wallet = make_wallet(rpc_client.clone(), wallet_cfg, secret, node_id).await?;

                uninitialized_wallets.push((
                    Arc::new(wallet),
                    wallet_info.balance,
                    node_id.clone(),
                ));
            } else if wallet_info.account_state == AccountState::Active {
                deployed = true;
            }

            res_details.insert(
                node_id.clone(),
                DeployWalletResultDetails {
                    version: wallet_cfg.version,
                    subwallet_id: wallet_cfg.subwallet_id,
                    workchain: wallet_cfg.workchain,
                    account_state: wallet_info.account_state,
                    address: wallet_address.to_string(),
                    balance: wallet_info.balance,
                    deployed,
                    error: None,
                },
            );
        }

        if self.verbose && !uninitialized_wallets.is_empty() {
            println!(
                "\n{}: {}",
                "Trying to deploy uninitialized wallets".bold(),
                uninitialized_wallets.len()
            );
        }

        // Deploy
        let mut wallets_to_wait: Vec<(Arc<dyn TonWallet>, String)> = Vec::new();
        for (wallet, balance, node_id) in &uninitialized_wallets {
            if cancellation_ctx.is_cancelled() {
                anyhow::bail!("Task cancelled");
            }

            let res = res_details.get_mut(node_id).unwrap();
            if *balance < Self::MIN_BALANCE {
                if self.verbose {
                    println!("Failed to deploy wallet {}, balance is too low", wallet.address());
                }

                res.deployed = false;
                res.error = Some("Failed to deploy wallet, balance is too low".to_string());
                continue;
            }

            if self.verbose {
                println!("Deploy wallet {}...", wallet.address());
            }

            let msg_boc =
                write_boc(&wallet.deploy_message(Self::MIN_BALANCE / 10, Cell::default()).await?)?;

            rpc_client.send_boc(&msg_boc).await?;

            wallets_to_wait.push((wallet.clone(), node_id.clone()));
        }

        // Wait for wallets deploy
        if self.verbose && !wallets_to_wait.is_empty() {
            println!("\n{}: {}...", "Wait for wallets deploy".bold(), wallets_to_wait.len());
        }

        for (wallet, node_id) in &wallets_to_wait {
            wait_for_deploy(
                rpc_client.clone(),
                &wallet.address(),
                &cancellation_ctx,
                self.verbose,
                DEPLOY_TIMEOUT,
            )
            .await?;

            let res = res_details.get_mut(node_id).unwrap();
            res.deployed = true;
            res.account_state = AccountState::Active;
        }

        let result = DeployWalletResult { config: self.config.clone(), details: res_details };
        match serde_json::to_string_pretty(&result) {
            Ok(s) => println!("{}", s),
            Err(_) => {
                tracing::error!("Failed to serialize result")
            }
        };

        Ok(())
    }
}

#[derive(serde::Serialize, Default)]
struct DeployPoolResult {
    pub config: String,
    pub account_state: AccountState,
    pub address: String,
    pub deployed: bool,
    pub error: Option<String>,
}

impl DeployPoolCmd {
    pub async fn run(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        // Suppress the returned error because the error is already printed to the stdout in json format
        let _ = self.run_with_result(cancellation_ctx).await;
        Ok(())
    }
    pub async fn run_with_result(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let res = Rc::new(RefCell::new(DeployPoolResult {
            config: self.config.clone(),
            ..Default::default()
        }));
        let set_err = |e: anyhow::Error| {
            res.borrow_mut().error = Some(format!("{:#}", e));
            e
        };

        scopeguard::defer! {
            let result = res.borrow();
            match serde_json::to_string_pretty(&*result) {
                Ok(s) => println!("{}", s),
                Err(_) => {
                    tracing::error!("Failed to serialize result")
                }
            };
        }

        let (config, vault, rpc_client) =
            load_config_vault_rpc_client(Path::new(&self.config)).await.map_err(set_err)?;
        let wallet_cfg = config
            .wallets
            .get(&self.node)
            .ok_or_else(|| anyhow::anyhow!("Wallet '{}' not found", &self.node))
            .map_err(set_err)?;

        if self.verbose {
            println!("Update wallet info ...");
        }

        let (wallet_address, wallet_info, secret) =
            wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await.map_err(set_err)?;
        if cancellation_ctx.is_cancelled() {
            anyhow::bail!("Task cancelled");
        }

        let pool_cfg_opt = self
            .pool
            .as_ref()
            .or_else(|| config.bindings.get(&self.node).and_then(|b| b.pool.as_ref()))
            .and_then(|name| config.pools.get(name));

        let (pool_address, state_init) = match pool_cfg_opt {
            Some(PoolConfig::TONCore {
                validator_share,
                address,
                max_nominators,
                min_validator_stake,
                min_nominator_stake,
            }) => {
                let resolved = resolve_toncore_pool(
                    &wallet_address,
                    *validator_share,
                    address.as_deref(),
                    max_nominators.as_ref().copied(),
                    min_validator_stake.as_ref().copied(),
                    min_nominator_stake.as_ref().copied(),
                )
                .map_err(set_err)?;
                (resolved.address, resolved.state_init)
            }
            Some(PoolConfig::SNP { .. }) | None => {
                let owner = self.owner.as_ref().ok_or_else(|| {
                    set_err(anyhow::anyhow!(
                        "SNP deploy requires --owner (set `pool` to a `kind: core` entry for TON Nominator Pool deploy)"
                    ))
                })?;
                let (pool_address, state_init) =
                    NominatorWrapperImpl::calculate_address_with_state_init(-1, owner, &wallet_address)
                        .map_err(set_err)?;
                (pool_address, state_init)
            }
        };

        res.borrow_mut().address = pool_address.to_string();

        if self.verbose {
            println!("Update pool info ...");
        }

        let pool_info = rpc_client.get_address_information(&pool_address).await.map_err(set_err)?;
        res.borrow_mut().account_state = pool_info.state.clone();

        if pool_info.state == AccountState::Active {
            if self.verbose {
                println!("The pool '{}' is already deployed", &pool_address);
            }

            return Ok(());
        } else if pool_info.state == AccountState::Frozen {
            return Err(set_err(anyhow::anyhow!("The pool '{}' is frozen", &pool_address)));
        }

        if cancellation_ctx.is_cancelled() {
            return Err(set_err(anyhow::anyhow!("Task cancelled")));
        }

        if self.verbose {
            match pool_cfg_opt {
                Some(PoolConfig::TONCore { validator_share, .. }) => println!(
                    "Deploy TON Nominator Pool (core): validator={}, validator_share={}, pool={} ...",
                    wallet_address, validator_share, pool_address
                ),
                _ => {
                    if let Some(owner) = self.owner.as_ref() {
                        println!(
                            "Deploy Single Nominator Pool: owner={}, wallet={} ...",
                            owner, wallet_address
                        );
                    }
                }
            }
        }

        if wallet_info.account_state != AccountState::Active {
            res.borrow_mut().error =
                Some(format!("Wallet '{}' state {}", wallet_address, wallet_info.account_state));
            return Ok(());
        }

        let amount_to_send_nano = tons_f64_to_nanotons(self.amount);
        if wallet_info.balance < amount_to_send_nano {
            return Err(set_err(anyhow::anyhow!(
                "Wallet '{}' balance {:.4}_TON is too low",
                wallet_address,
                nanotons_to_tons_f64(wallet_info.balance)
            )));
        }

        // Deploy
        let wallet = make_wallet(rpc_client.clone(), wallet_cfg, secret, &self.node)
            .await
            .map_err(set_err)?;
        let msg_boc = write_boc(
            &wallet
                .build_message(
                    pool_address.clone(),
                    amount_to_send_nano,
                    Cell::default(),
                    false,
                    wallet_info.seqno,
                    None,
                    Some(state_init),
                )
                .await
                .map_err(set_err)?,
        )
        .map_err(set_err)?;

        rpc_client.send_boc(&msg_boc).await.map_err(set_err)?;
        wait_for_deploy(
            rpc_client.clone(),
            &pool_address,
            &cancellation_ctx,
            self.verbose,
            DEPLOY_TIMEOUT,
        )
        .await
        .map_err(set_err)?;

        res.borrow_mut().deployed = true;
        res.borrow_mut().account_state = AccountState::Active;

        Ok(())
    }
}
