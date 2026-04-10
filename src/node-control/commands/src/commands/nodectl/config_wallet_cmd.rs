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
    utils::{
        MASTER_WALLET_RESERVED_NAME, SEND_TIMEOUT, check_ton_api_connection,
        fetch_network_max_factor, get_wallet_config, load_config_vault,
        load_config_vault_rpc_client, make_wallet, save_config, wait_for_seqno_change,
        wallet_address, wallet_info, warn_missing_secret, warn_ton_api_unavailable,
    },
};
use anyhow::Context;
use colored::Colorize;
use common::{
    TonWalletVersion,
    app_config::{AppConfig, ElectionsConfig, KeyConfig, PoolConfig, WalletConfig},
    task_cancellation::CancellationCtx,
    time_format,
    ton_utils::{display_tons, tons_f64_to_nanotons},
};
use contracts::{
    ElectorWrapper, ElectorWrapperImpl, NominatorWrapperImpl, TonWallet, contract_provider,
    nominator, resolve_toncore_pool, resolve_toncore_router,
};
use elections::providers::{DefaultElectionsProvider, ElectionsProvider};
use secrets_vault::{errors::error::VaultError, vault::SecretVault};
use std::{borrow::Cow, io::Write, path::Path, sync::Arc};
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, Cell, MsgAddressInt, write_boc};
use ton_http_api_client::v2::{client_json_rpc::ClientJsonRpc, data_models::AccountState};

const WALLET_SEND_GAS: u64 = 1_000_000; // 0.001 TON
/// Value in nanotons required by elector to execute stake operations.
const ELECTOR_STAKE_FEE: u64 = 1_000_000_000;
/// Gas fee consumed by nominator pool.
const NPOOL_COMPUTE_FEE: u64 = 200_000_000;
/// Gas fee consumed by wallet to send message to nominator pool.
const WALLET_COMPUTE_FEE: u64 = 100_000_000;

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
    /// Send election stake via nominator pool
    Stake(WalletStakeCmd),
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
pub struct WalletLsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

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

#[derive(clap::Args, Clone)]
#[command(about = "Send election stake via nominator pool")]
pub struct WalletStakeCmd {
    #[arg(short = 'b', long = "binding", help = "Binding name")]
    binding: String,
    #[arg(short = 'a', long = "amount", help = "Stake amount in TONs")]
    amount: f64,
    #[arg(
        short = 'm',
        long = "max-factor",
        default_value = "3.0",
        help = "Max factor from 1.0 up to the network limit (config param 17)"
    )]
    max_factor: f32,
    #[arg(long = "pool-index", default_value_t = 0, help = "Router: pool index (0 or 1)")]
    pool_index: usize,
}

impl WalletCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        match &self.action {
            WalletAction::Add(cmd) => cmd.run(path).await,
            WalletAction::Ls(cmd) => cmd.run(path).await,
            WalletAction::Rm(cmd) => cmd.run(path).await,
            WalletAction::Send(cmd) => cmd.run(path, cancellation_ctx).await,
            WalletAction::Stake(cmd) => cmd.run(path, cancellation_ctx).await,
        }
    }
}

impl WalletAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.name == MASTER_WALLET_RESERVED_NAME {
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

#[derive(serde::Serialize)]
struct WalletView {
    name: String,
    secret: String,
    version: String,
    state: Option<String>,
    balance: Option<String>,
    address: Option<String>,
}

impl WalletLsCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

        if let Err(e) = check_ton_api_connection(&rpc_client).await {
            if matches!(self.format, OutputFormat::Table) {
                warn_ton_api_unavailable(&e, "State and balances will not be available");
            }
        }

        let mut all_wallets: Vec<(&str, &WalletConfig)> =
            config.wallets.iter().map(|(k, v)| (k.as_str(), v)).collect();
        if let Some(mw) = config.master_wallet.as_ref() {
            all_wallets.push((MASTER_WALLET_RESERVED_NAME, mw));
        }

        if all_wallets.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No wallets configured".yellow()),
            }
            return Ok(());
        }

        match self.format {
            OutputFormat::Json => {
                print_wallets_json(all_wallets, vault, rpc_client).await?;
            }
            OutputFormat::Table => {
                print_wallets_table(all_wallets, vault, rpc_client).await;
            }
        }
        Ok(())
    }
}

async fn print_wallets_json(
    wallets: Vec<(&str, &WalletConfig)>,
    vault: Arc<SecretVault>,
    rpc_client: Arc<ClientJsonRpc>,
) -> anyhow::Result<()> {
    let mut views = Vec::new();
    for (name, wallet_cfg) in wallets {
        let secret = match &wallet_cfg.key {
            KeyConfig::VaultKey { name } => name.clone(),
            _ => "-".to_string(),
        };
        let (address, state, balance) = match wallet_address(wallet_cfg, vault.clone()).await {
            Ok((address, _)) => {
                let address_str = address
                    .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
                    .unwrap_or_else(|_| address.to_string());
                match rpc_client.get_wallet_information(&address).await {
                    Ok(info) => (
                        Some(address_str),
                        Some(info.account_state.to_string()),
                        Some(display_tons(info.balance)),
                    ),
                    Err(_) => (Some(address_str), None, None),
                }
            }
            Err(_) => (None, None, None),
        };
        views.push(WalletView {
            name: name.to_string(),
            secret,
            version: wallet_cfg.version.to_string(),
            state,
            balance,
            address,
        });
    }
    println!("{}", serde_json::to_string_pretty(&views)?);
    Ok(())
}

async fn print_wallets_table(
    wallets: Vec<(&str, &WalletConfig)>,
    vault: Arc<SecretVault>,
    rpc_client: Arc<ClientJsonRpc>,
) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "Wallets:".green(), wallets.len());
    println!(
        "  {:<20} {:<22} {:<8} {:<9} {:<14} {}",
        "Name".cyan().bold(),
        "Secret".cyan().bold(),
        "Version".cyan().bold(),
        "State".cyan().bold(),
        "Balance".cyan().bold(),
        "Address".cyan().bold(),
    );
    println!("  {}", "─".repeat(125).dimmed());

    let red_dash = Cow::Borrowed(&"-".red());
    for (name, wallet_cfg) in wallets {
        let (address, account_state, balance) =
            match wallet_address(wallet_cfg, vault.clone()).await {
                Ok((address, _)) => {
                    let address_str = address
                        .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
                        .unwrap_or_else(|_| address.to_string());

                    match rpc_client.get_wallet_information(&address).await {
                        Ok(info) => (
                            address_str.white(),
                            Cow::Owned(info.account_state.to_string().white()),
                            Cow::Owned(display_tons(info.balance).white()),
                        ),
                        Err(_) => (address_str.white(), red_dash.clone(), red_dash.clone()),
                    }
                }
                Err(e) => {
                    let error_message = if e
                        .downcast_ref::<VaultError>()
                        .is_some_and(|e| e.code() == VaultError::NOT_FOUND)
                    {
                        "not found in the vault".red()
                    } else {
                        e.root_cause().to_string().red()
                    };
                    (error_message, red_dash.clone(), red_dash.clone())
                }
            };

        let secret_name = match &wallet_cfg.key {
            KeyConfig::VaultKey { name } => Cow::Owned(name.white()),
            _ => red_dash.clone(),
        };
        println!(
            "  {:<20} {:<22} {:<8} {:<9} {:<14} {}",
            name,
            secret_name,
            wallet_cfg.version.to_string(),
            account_state,
            balance,
            address,
        );
    }
    println!();
}

impl WalletRmCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.name == MASTER_WALLET_RESERVED_NAME {
            anyhow::bail!("The master wallet cannot be removed");
        }

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
                "Wrong amount value {} TON. Wallet balance is {} TON",
                self.amount,
                display_tons(from_wallet_info.balance)
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

        if !confirm("Confirm transfer?")? {
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

impl WalletStakeCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;
        let network_max_factor = fetch_network_max_factor(&rpc_client).await?;
        ElectionsConfig { max_factor: self.max_factor, ..Default::default() }
            .validate(Some(network_max_factor))?;

        // Resolve binding → wallet, pool, node
        let binding = config
            .bindings
            .get(&self.binding)
            .ok_or_else(|| anyhow::anyhow!("Binding '{}' not found", self.binding))?;

        let wallet_cfg =
            get_wallet_config(&binding.wallet, &config.wallets, config.master_wallet.as_ref())?;

        let pool_name = binding
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Binding '{}' has no pool configured", self.binding))?;
        let pool_cfg = config
            .pools
            .get(pool_name)
            .ok_or_else(|| anyhow::anyhow!("Pool '{}' not found", pool_name))?;

        let adnl_cfg = config
            .nodes
            .get(&self.binding)
            .ok_or_else(|| anyhow::anyhow!("Node '{}' not found", self.binding))?;

        // Wallet and pool addresses
        let (wallet_address, wallet_info_res, wallet_secret) =
            wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await?;
        if wallet_info_res.account_state != AccountState::Active {
            anyhow::bail!("Wallet '{}' is {}", binding.wallet, wallet_info_res.account_state);
        }
        let pool_address = resolve_pool_address(pool_cfg, &wallet_address, self.pool_index)?;
        let pool_addr_bytes = pool_address.address().clone().storage().to_vec();

        // Connect to validator node via control protocol
        let adnl_client_cfg = adnl_cfg
            .to_node_adnl_config(Some(vault.clone()))
            .await
            .context("ADNL client config")?;
        let mut provider = DefaultElectionsProvider::new(adnl_client_cfg);

        // Get active election ID from elector via RPC
        let elector = ElectorWrapperImpl::new(contract_provider!(rpc_client.clone()));
        let election_id =
            elector.get_active_election_id().await.context("get_active_election_id")?;
        if election_id == 0 {
            anyhow::bail!("No active elections");
        }
        let elections_info = elector.elections_info().await.context("elections_info")?;
        if elections_info.finished {
            anyhow::bail!("Elections are already finished");
        }

        let stake_nanotons = tons_f64_to_nanotons(self.amount);
        if stake_nanotons < elections_info.min_stake {
            anyhow::bail!(
                "Stake {:.4} TON is below minimum {:.4} TON",
                self.amount,
                elections_info.min_stake as f64 / 1_000_000_000.0
            );
        }

        // Get election parameters for key expiration
        let cfg15 = provider.election_parameters().await.context("election_parameters")?;
        const KEY_EXPIRED_LAG: u64 = 300;
        let key_expired_at = election_id + cfg15.validators_elected_for as u64 + KEY_EXPIRED_LAG;

        // Find or generate validator key
        let validator_config = provider.validator_config().await.context("validator_config")?;
        let existing_key = validator_config.find(election_id);

        let (key_id, pub_key, adnl_addr) = match existing_key {
            Some(entry) => {
                let pub_key =
                    provider.export_public_key(&entry.key_id).await.context("export_public_key")?;
                let adnl_addr = entry
                    .adnl_addr()
                    .ok_or_else(|| anyhow::anyhow!("Validator key has no ADNL address"))?;
                println!(
                    "{} Reusing existing validator key for election {}: {}",
                    "Info:".cyan().bold(),
                    election_id,
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &pub_key)
                );
                (entry.key_id, pub_key, adnl_addr)
            }
            None => {
                println!(
                    "\n{} No validator key found for election {}",
                    "Warning:".yellow().bold(),
                    election_id
                );
                if !confirm("Generate new validator key?")? {
                    anyhow::bail!("Aborted: no validator key for this election");
                }
                let (key_id, pub_key) = provider
                    .new_validator_key(election_id, key_expired_at)
                    .await
                    .context("new_validator_key")?;
                let adnl_addr = provider
                    .new_adnl_addr(key_id.clone(), key_expired_at)
                    .await
                    .context("new_adnl_addr")?;
                println!(
                    "{} Generated validator key: {}\n{} Generated ADNL address: {}",
                    "Info:".cyan().bold(),
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &pub_key),
                    "Info:".cyan().bold(),
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &adnl_addr),
                );
                (key_id, pub_key, adnl_addr)
            }
        };

        // Check if already participating
        if let Some(p) = elections_info.participants.iter().find(|p| p.pub_key == pub_key) {
            println!(
                "\n{} Already participating with stake {:.4} TON",
                "Warning:".yellow().bold(),
                p.stake as f64 / 1_000_000_000.0
            );
        }

        // Build election bid: sign data with validator key
        let max_factor_raw = (self.max_factor * 65536.0) as u32;

        let mut sign_data = 0x654C5074u32.to_be_bytes().to_vec();
        sign_data.extend_from_slice(&(election_id as u32).to_be_bytes());
        sign_data.extend_from_slice(&max_factor_raw.to_be_bytes());
        sign_data.extend_from_slice(&pool_addr_bytes);
        sign_data.extend_from_slice(&adnl_addr);

        let signature = provider.sign(key_id, sign_data).await.context("sign election bid")?;

        // Build NEW_STAKE payload for nominator pool
        let payload = nominator::new_stake(&nominator::NewStakeParams {
            query_id: time_format::now(),
            stake_amount: stake_nanotons,
            validator_pubkey: &pub_key,
            stake_at: election_id as u32,
            max_factor: max_factor_raw,
            adnl_addr: &adnl_addr,
            signature: &signature,
        })?;

        // Build wallet message to nominator pool (wallet sends only gas, pool has the stake)
        let wallet =
            make_wallet(rpc_client.clone(), wallet_cfg, wallet_secret, &binding.wallet).await?;
        let fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
        if wallet_info_res.balance < fee + WALLET_COMPUTE_FEE {
            anyhow::bail!(
                "Insufficient wallet balance: required {:.4} TON, available {:.4} TON",
                (fee + WALLET_COMPUTE_FEE) as f64 / 1_000_000_000.0,
                display_tons(wallet_info_res.balance)
            );
        }
        let msg = wallet.message(pool_address.clone(), fee, payload).await?;
        let msg_boc = write_boc(&msg)?;

        // Confirmation
        println!(
            "\n{}\n  Binding:     {}\n  Wallet:      {} ({})\n  Pool:        {}\n  Election ID: {} ({})\n  Stake:       {:.9} TON\n  Max Factor:  {:.2}\n  Min Stake:   {:.4} TON\n",
            "Stake summary:".cyan().bold(),
            self.binding,
            binding.wallet,
            wallet_address,
            pool_address,
            election_id,
            time_format::format_ts(election_id),
            self.amount,
            self.max_factor,
            elections_info.min_stake as f64 / 1_000_000_000.0,
        );

        if !confirm("Confirm stake?")? {
            println!("{}", "Stake cancelled".yellow());
            return Ok(());
        }

        println!("{} Sending message to wallet...", "DOING".blue().bold());
        // Send via control protocol
        provider.send_boc(&msg_boc).await.context("send stake message")?;

        wait_for_seqno_change(
            rpc_client.clone(),
            &wallet_address,
            wallet_info_res.seqno,
            &cancellation_ctx,
            SEND_TIMEOUT,
        )
        .await?;

        println!(
            "{} Message delivered, waiting for stake to appear in elector...",
            "OK   ".green().bold()
        );

        let previous_stake = elections_info
            .participants
            .iter()
            .find(|p| p.pub_key == pub_key)
            .map(|p| p.stake)
            .unwrap_or(0);
        let expected_stake = previous_stake + stake_nanotons;

        let stake_timeout = tokio::time::Duration::from_secs(60);
        wait_for_stake_accepted(
            &elector,
            &pub_key,
            expected_stake,
            &cancellation_ctx,
            stake_timeout,
        )
        .await?;

        println!("{} Stake accepted by elector", "OK   ".green().bold());
        let _ = provider.shutdown().await;
        Ok(())
    }
}

const STAKE_POLL_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(3);

async fn wait_for_stake_accepted(
    elector: &ElectorWrapperImpl,
    pub_key: &[u8],
    expected_stake: u64,
    cancellation_ctx: &CancellationCtx,
    max_wait: tokio::time::Duration,
) -> anyhow::Result<()> {
    let poll = async {
        loop {
            if cancellation_ctx.is_cancelled() {
                anyhow::bail!("Task cancelled");
            }
            tokio::time::sleep(STAKE_POLL_INTERVAL).await;
            let info = elector.elections_info().await.context("elections_info")?;
            if let Some(p) = info.participants.iter().find(|p| p.pub_key == pub_key) {
                if p.stake >= expected_stake {
                    return Ok(());
                }
            }
        }
    };
    tokio::time::timeout(max_wait, poll)
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for stake to appear in elector"))?
}

fn confirm(prompt: &str) -> anyhow::Result<bool> {
    print!("{prompt} [y/N]: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn resolve_pool_address(
    pool_cfg: &PoolConfig,
    validator_addr: &MsgAddressInt,
    pool_index: usize,
) -> anyhow::Result<MsgAddressInt> {
    match pool_cfg {
        PoolConfig::SNP { .. } if pool_index != 0 => {
            anyhow::bail!("--pool-index is not applicable for SNP pools");
        }
        PoolConfig::SNP { address, owner } => match (address, owner) {
            (Some(addr), _) => addr.parse::<MsgAddressInt>().context("invalid pool address"),
            (None, Some(owner)) => {
                let owner_addr =
                    owner.parse::<MsgAddressInt>().context("invalid pool owner address")?;
                NominatorWrapperImpl::calculate_address(-1, &owner_addr, validator_addr)
            }
            (None, None) => anyhow::bail!("Pool has neither address nor owner configured"),
        },
        PoolConfig::TONCore { .. } if pool_index != 0 => {
            anyhow::bail!(
                "--pool-index is only valid for Router pools (TONCore has a single pool)"
            );
        }
        PoolConfig::TONCore {
            validator_share,
            address,
            max_nominators,
            min_validator_stake,
            min_nominator_stake,
        } => {
            let resolved = resolve_toncore_pool(
                validator_addr,
                *validator_share,
                address.as_deref(),
                max_nominators.as_ref().copied(),
                min_validator_stake.as_ref().copied(),
                min_nominator_stake.as_ref().copied(),
            )?;
            Ok(resolved.address)
        }
        PoolConfig::TONCoreRouter { .. } if pool_index > 1 => {
            anyhow::bail!("--pool-index must be 0 or 1 for Router pools");
        }
        PoolConfig::TONCoreRouter {
            validator_share,
            addresses,
            max_nominators,
            min_validator_stake,
            min_nominator_stake,
        } => {
            let resolved = resolve_toncore_router(
                validator_addr,
                *validator_share,
                addresses.as_ref(),
                max_nominators.as_ref().copied(),
                min_validator_stake.as_ref().copied(),
                min_nominator_stake.as_ref().copied(),
            )?;
            Ok(resolved[pool_index].address.clone())
        }
    }
}
