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
        SEND_TIMEOUT, calculate_wallet_address, get_wallet_config, load_config_vault_rpc_client,
        make_wallet, resolve_pool_address_from_config, save_config,
        toncore_pool_slot_from_cli_flags, try_create_rpc_client, wait_for_seqno_change,
        wallet_info, warn_ton_api_unavailable,
    },
};
use colored::Colorize;
use common::{
    app_config::{
        AppConfig, DEFAULT_TONCORE_MAX_NOMINATORS, DEFAULT_TONCORE_MIN_NOMINATOR_STAKE,
        DEFAULT_TONCORE_MIN_VALIDATOR_STAKE, PoolConfig, TonCoreInitParams, TonCorePoolConfig,
    },
    task_cancellation::CancellationCtx,
    ton_utils::{display_tons, nanotons_to_tons_f64, tons_f64_to_nanotons},
};
use contracts::{
    NOMINATOR_POOL_WORKCHAIN, NominatorWrapperImpl, TonWallet,
    ton_core_nominator::messages as pool_messages,
};
use secrets_vault::{vault::SecretVault, vault_builder::SecretVaultBuilder};
use std::{io::Write, path::Path, str::FromStr, sync::Arc};
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, MsgAddressInt, write_boc};
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

#[derive(clap::Args, Clone)]
#[command(about = "Manage pools in the configuration")]
pub struct PoolCmd {
    #[command(subcommand)]
    action: PoolAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum PoolAction {
    /// Add a pool to the configuration
    Add(PoolAddSubCmd),
    /// Backward-compatible alias for `add core` (hidden)
    #[command(hide = true)]
    AddCore(PoolAddCoreCmd),
    /// List all configured pools
    Ls(PoolLsCmd),
    /// Remove a pool from the configuration
    Rm(PoolRmCmd),
    /// Deposit validator funds into a TONCore nominator pool
    DepositValidator(PoolDepositValidatorCmd),
    /// Withdraw validator funds from a TONCore nominator pool
    WithdrawValidator(PoolWithdrawValidatorCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add a pool to the configuration")]
pub struct PoolAddSubCmd {
    #[command(subcommand)]
    action: Option<PoolAddAction>,
    #[command(flatten)]
    snp: PoolAddCmd,
}

#[derive(clap::Subcommand, Clone)]
pub enum PoolAddAction {
    /// Add an SNP pool to the configuration
    Snp(PoolAddCmd),
    /// Add a TONCore nominator pool to the configuration
    Core(PoolAddCoreCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add an SNP pool to the configuration")]
pub struct PoolAddCmd {
    #[arg(short = 'n', long = "name", help = "Pool name (unique identifier)")]
    name: Option<String>,
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
#[command(about = "Add a TONCore nominator pool to the configuration")]
#[command(group(clap::ArgGroup::new("slot").required(true).args(&["even", "odd"])))]
pub struct PoolAddCoreCmd {
    #[arg(short = 'n', long = "name", help = "Pool name (unique identifier)")]
    name: String,

    #[arg(
        long = "validator-share",
        help = "Validator reward share in basis points (e.g. 1000 = 10%)"
    )]
    validator_share: Option<u16>,

    #[arg(long = "max-nominators", help = "Max nominators (default: 40)")]
    max_nominators: Option<u16>,

    #[arg(long = "min-validator-stake", help = "Minimum validator stake in TON (default: 10 000)")]
    min_validator_stake: Option<f64>,

    #[arg(long = "min-nominator-stake", help = "Minimum nominator stake in TON (default: 10 000)")]
    min_nominator_stake: Option<f64>,

    #[arg(long = "address", help = "Existing pool address for selected slot, raw or base64url")]
    address: Option<String>,

    #[arg(
        long = "even",
        conflicts_with = "odd",
        help = "Configure/deploy TONCore slot 0 (even rounds)"
    )]
    even: bool,
    #[arg(
        long = "odd",
        conflicts_with = "even",
        help = "Configure/deploy TONCore slot 1 (odd rounds)"
    )]
    odd: bool,
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

#[derive(clap::Args, Clone)]
#[command(about = "Deposit validator funds into a TONCore nominator pool")]
pub struct PoolDepositValidatorCmd {
    #[arg(short = 'b', long = "binding", help = "Binding name (resolves wallet and pool)")]
    binding: String,
    #[arg(short = 'a', long = "amount", help = "Amount in TON to deposit")]
    amount: f64,
    #[arg(
        long = "pool-even",
        conflicts_with = "pool_odd",
        help = "Core: use the pool for even validation rounds (default if neither flag is set)"
    )]
    pool_even: bool,
    #[arg(
        long = "pool-odd",
        conflicts_with = "pool_even",
        help = "Core: use the pool for odd validation rounds"
    )]
    pool_odd: bool,
    /// Skip the interactive confirmation prompt (for scripts and CI)
    #[arg(long = "yes", help = "Do not ask for confirmation")]
    yes: bool,
}

#[derive(clap::Args, Clone)]
#[command(about = "Withdraw validator funds from a TONCore nominator pool")]
pub struct PoolWithdrawValidatorCmd {
    #[arg(short = 'b', long = "binding", help = "Binding name (resolves wallet and pool)")]
    binding: String,
    #[arg(short = 'a', long = "amount", help = "Amount in TON to withdraw")]
    amount: f64,
    #[arg(
        long = "pool-even",
        conflicts_with = "pool_odd",
        help = "Core: use the pool for even validation rounds (default if neither flag is set)"
    )]
    pool_even: bool,
    #[arg(
        long = "pool-odd",
        conflicts_with = "pool_even",
        help = "Core: use the pool for odd validation rounds"
    )]
    pool_odd: bool,
    /// Skip the interactive confirmation prompt (for scripts and CI)
    #[arg(long = "yes", help = "Do not ask for confirmation")]
    yes: bool,
}

impl PoolCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        match &self.action {
            PoolAction::Add(cmd) => match &cmd.action {
                Some(PoolAddAction::Snp(cmd)) => cmd.run(path).await,
                Some(PoolAddAction::Core(cmd)) => cmd.run(path).await,
                None => cmd.snp.run(path).await,
            },
            PoolAction::AddCore(cmd) => cmd.run(path).await,
            PoolAction::Ls(cmd) => cmd.run(path).await,
            PoolAction::Rm(cmd) => cmd.run(path).await,
            PoolAction::DepositValidator(cmd) => cmd.run(path, cancellation_ctx).await,
            PoolAction::WithdrawValidator(cmd) => cmd.run(path, cancellation_ctx).await,
        }
    }
}

impl PoolAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let name = self.name.as_ref().ok_or_else(|| anyhow::anyhow!("--name must be specified"))?;
        if self.address.is_none() && self.owner.is_none() {
            anyhow::bail!("At least one of --address or --owner must be specified");
        }

        let normalized_address = self
            .address
            .as_deref()
            .map(|addr| normalize_ton_address(addr, "address"))
            .transpose()?;
        let normalized_owner =
            self.owner.as_deref().map(|owner| normalize_ton_address(owner, "owner")).transpose()?;

        let mut config = AppConfig::load(path)?;

        if config.pools.contains_key(name) {
            anyhow::bail!(
                "Pool '{}' already exists. Remove it first or use a different name.",
                name
            );
        }

        let pool_config = PoolConfig::SNP {
            address: normalized_address.clone(),
            owner: normalized_owner.clone(),
        };

        config.pools.insert(name.clone(), pool_config);
        save_config(&config, path)?;

        let info = match (&normalized_address, &normalized_owner) {
            (Some(a), Some(o)) => format!("address='{}', owner='{}'", a, o),
            (Some(a), None) => format!("address='{}'", a),
            (None, Some(o)) => format!("owner='{}' (address will be calculated on bind)", o),
            _ => unreachable!(),
        };

        println!("\n{} Pool '{}' added ({})\n", "OK".green().bold(), name, info);
        Ok(())
    }
}

impl PoolAddCoreCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.address.is_none() && self.validator_share.is_none() {
            anyhow::bail!("At least one of --address or --validator-share must be specified");
        }

        let slot = if self.odd { 1usize } else { 0usize };
        let slot_name = if slot == 0 { "even" } else { "odd" };

        let mut config = AppConfig::load(path)?;

        let address =
            self.address.as_deref().map(|a| normalize_ton_address(a, "address")).transpose()?;

        let params = self.validator_share.map(|vs| TonCoreInitParams {
            validator_share: vs,
            max_nominators: self.max_nominators.unwrap_or(DEFAULT_TONCORE_MAX_NOMINATORS),
            min_validator_stake: self
                .min_validator_stake
                .map(tons_f64_to_nanotons)
                .unwrap_or(DEFAULT_TONCORE_MIN_VALIDATOR_STAKE),
            min_nominator_stake: self
                .min_nominator_stake
                .map(tons_f64_to_nanotons)
                .unwrap_or(DEFAULT_TONCORE_MIN_NOMINATOR_STAKE),
        });

        match config.pools.get_mut(&self.name) {
            Some(PoolConfig::TONCore { pools }) => {
                if pools[slot].is_some() {
                    anyhow::bail!(
                        "TONCore pool '{}' slot '{}' is already configured. Remove pool first or use another name.",
                        self.name,
                        slot_name
                    );
                }
                pools[slot] =
                    Some(TonCorePoolConfig { address: address.clone(), params: params.clone() });
            }
            Some(PoolConfig::SNP { .. }) => {
                anyhow::bail!(
                    "Pool '{}' already exists and is SNP. Remove it first or use another name.",
                    self.name
                );
            }
            None => {
                let mut pools: [Option<TonCorePoolConfig>; 2] = [None, None];
                pools[slot] =
                    Some(TonCorePoolConfig { address: address.clone(), params: params.clone() });
                config.pools.insert(self.name.clone(), PoolConfig::TONCore { pools });
            }
        }

        save_config(&config, path)?;

        let mut info_parts = Vec::new();
        if let Some(vs) = self.validator_share {
            info_parts.push(format!("validator_share={vs} bp"));
        }
        if let Some(a) = &address {
            info_parts.push(format!("address='{a}'"));
        }

        println!(
            "\n{} TONCore pool '{}' slot '{}' configured ({})\n",
            "OK".green().bold(),
            self.name,
            slot_name,
            info_parts.join(", ")
        );
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct PoolView {
    name: String,
    kind: String,
    balance: Option<String>,
    address: Option<String>,
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    addresses: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    balances: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validator_share: Option<u16>,
}

impl PoolLsCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let config = AppConfig::load(path)?;

        if config.pools.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No pools configured".yellow()),
            }
            return Ok(());
        }

        let rpc_client = match try_create_rpc_client(&config).await {
            Ok(c) => Some(c),
            Err(e) => {
                if matches!(self.format, OutputFormat::Table) {
                    warn_ton_api_unavailable(&e, "Balances will not be available");
                }
                None
            }
        };

        let views =
            collect_pool_views(&config, &rpc_client, self.format == OutputFormat::Table).await;

        match self.format {
            OutputFormat::Json => print_pools_json(&views)?,
            OutputFormat::Table => print_pools_table(&views),
        }
        Ok(())
    }
}

async fn collect_pool_views(
    config: &AppConfig,
    rpc_client: &Option<Arc<ClientJsonRpc>>,
    warn_on_error: bool,
) -> Vec<PoolView> {
    let mut vault: Option<Option<Arc<SecretVault>>> = None;
    let mut views = Vec::new();

    for (name, pool) in &config.pools {
        match pool {
            PoolConfig::SNP { address, owner } => {
                let needs_vault = address.is_none() && owner.is_some();
                if needs_vault && vault.is_none() {
                    vault = Some(match SecretVaultBuilder::from_env().await {
                        Ok(v) => Some(v),
                        Err(e) => {
                            if warn_on_error {
                                println!(
                                    "{}: {}",
                                    "Warning: failed to initialize secret vault".yellow(),
                                    e.to_string().yellow()
                                );
                            }
                            None
                        }
                    });
                }
                let vault_ref = vault.as_ref().and_then(|v| v.clone());

                let addr_result = get_pool_display_result(
                    name,
                    address.as_ref(),
                    owner.as_ref(),
                    config,
                    vault_ref,
                )
                .await;

                let balance_result = resolve_pool_balance(&addr_result, rpc_client).await;

                let display_owner =
                    owner.as_ref().and_then(|o| MsgAddressInt::from_str(o).ok()).and_then(|o| {
                        o.to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE).ok()
                    });

                views.push(PoolView {
                    name: name.clone(),
                    kind: "SNP".to_string(),
                    balance: balance_result.ok(),
                    address: addr_result.ok(),
                    owner: display_owner,
                    addresses: None,
                    balances: None,
                    validator_share: None,
                });
            }
            PoolConfig::TONCore { pools } => {
                let is_dual = pools[1].is_some();
                let validator_share = pools
                    .iter()
                    .flatten()
                    .find_map(|s| s.params.as_ref().map(|p| p.validator_share));

                if is_dual {
                    // Two-pool (even + odd) view
                    let mut resolved_addrs: Vec<String> = Vec::new();
                    for slot in pools.iter().flatten() {
                        let addr_str = slot.address.clone().unwrap_or_else(|| {
                            resolve_toncore_pool_address_via_binding_for_slot(slot, name, config)
                                .unwrap_or_else(|| "<not deployed>".into())
                        });
                        resolved_addrs.push(addr_str);
                    }
                    while resolved_addrs.len() < 2 {
                        resolved_addrs.push("<not deployed>".into());
                    }

                    let pool_balances = if let Some(client) = rpc_client {
                        let mut bals = Vec::new();
                        for addr_str in &resolved_addrs {
                            if let Ok(addr) = MsgAddressInt::from_str(addr_str) {
                                match client.get_address_information(&addr).await {
                                    Ok(info) => bals.push(display_tons(info.balance)),
                                    Err(_) => bals.push("-".to_string()),
                                }
                            } else {
                                bals.push("-".to_string());
                            }
                        }
                        Some(bals)
                    } else {
                        None
                    };

                    views.push(PoolView {
                        name: name.clone(),
                        kind: "TONCore".to_string(),
                        balance: None,
                        address: None,
                        owner: None,
                        addresses: Some(resolved_addrs),
                        balances: pool_balances,
                        validator_share,
                    });
                } else {
                    // Single-pool view (slot 0 only)
                    let slot0 = pools[0].as_ref();
                    let resolved_addr = slot0.and_then(|s| s.address.clone()).or_else(|| {
                        slot0.and_then(|s| {
                            resolve_toncore_pool_address_via_binding_for_slot(s, name, config)
                        })
                    });

                    let balance_result = if let Some(ref addr_str) = resolved_addr {
                        resolve_pool_balance(&Ok(addr_str.clone()), rpc_client).await.ok()
                    } else {
                        None
                    };

                    views.push(PoolView {
                        name: name.clone(),
                        kind: "Core".to_string(),
                        balance: balance_result,
                        address: resolved_addr,
                        owner: None,
                        addresses: None,
                        balances: None,
                        validator_share,
                    });
                }
            }
        }
    }
    views
}

fn print_pools_json(views: &[PoolView]) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(views)?);
    Ok(())
}

fn print_pools_table(views: &[PoolView]) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "Pools:".green(), views.len());
    println!(
        "  {:<15} {:<8} {:<14} {:<50} {}",
        "Name".cyan().bold(),
        "Kind".cyan().bold(),
        "Balance".cyan().bold(),
        "Address".cyan().bold(),
        "Owner / share".cyan().bold(),
    );
    println!("  {}", "─".repeat(145).dimmed());

    fn format_validator_share_bp(share: Option<u16>) -> String {
        share.map(|s| format!("{s} bp")).unwrap_or_else(|| "-".to_string())
    }

    for v in views {
        match v.kind.as_str() {
            "SNP" => {
                let display_addr =
                    v.address.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                let display_owner =
                    v.owner.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                let display_balance =
                    v.balance.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());

                println!(
                    "  {:<15} {:<8} {:<14} {:<50} {}",
                    v.name, "SNP", display_balance, display_addr, display_owner,
                );
            }
            "Core" => {
                let display_addr = v.address.as_deref().unwrap_or("<not deployed>");
                let display_balance =
                    v.balance.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                let share = format_validator_share_bp(v.validator_share);
                println!(
                    "  {:<15} {:<8} {:<14} {:<50} {}",
                    v.name, "Core", display_balance, display_addr, share,
                );
            }
            "TONCore" if v.addresses.is_some() => {
                let addrs = v
                    .addresses
                    .as_deref()
                    .map(|a| a.join(", "))
                    .unwrap_or_else(|| "<not deployed>".into());
                let display_balance =
                    v.balances.as_deref().map(|b| b.join(" | ")).unwrap_or_else(|| "-".into());
                let share = format_validator_share_bp(v.validator_share);
                println!(
                    "  {:<15} {:<8} {:<14} {:<50} {}",
                    v.name, "TONCore", display_balance, addrs, share,
                );
            }
            _ => {}
        }
    }
    println!();
}

async fn get_pool_display_result(
    pool_name: &str,
    address: Option<&String>,
    owner: Option<&String>,
    config: &AppConfig,
    vault: Option<Arc<SecretVault>>,
) -> Result<String, String> {
    match (address, owner) {
        (Some(addr), _) => Ok(MsgAddressInt::from_str(addr)
            .map_err(|_| "invalid address")?
            .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
            .map_err(|_| "conversion failed")?),
        (None, Some(owner_str)) => resolve_pool_address(pool_name, owner_str, config, vault).await,
        (None, None) => Err("pool owner not configured".to_string()),
    }
}

async fn resolve_pool_balance(
    addr_result: &Result<String, String>,
    rpc_client: &Option<Arc<ClientJsonRpc>>,
) -> Result<String, String> {
    match (addr_result, rpc_client) {
        (Ok(addr_str), Some(client)) => {
            let addr = MsgAddressInt::from_str(addr_str).map_err(|_| "invalid address")?;
            match client.get_address_information(&addr).await {
                Ok(info) => Ok(display_tons(info.balance)),
                Err(e) => Err(format!("ton api failed: {e}")),
            }
        }
        (Err(_), _) => Err("-".to_string()),
        (_, None) => Err("-".to_string()),
    }
}

async fn resolve_pool_address(
    pool_name: &str,
    owner: &str,
    config: &AppConfig,
    vault: Option<Arc<SecretVault>>,
) -> Result<String, String> {
    // 1. Find binding referencing this pool; prefer enabled, then sort by name for determinism
    let mut matching: Vec<_> =
        config.bindings.iter().filter(|(_, b)| b.pool.as_deref() == Some(pool_name)).collect();
    if matching.is_empty() {
        return Err("no binding found".to_string());
    }
    matching.sort_by_key(|(k, b)| (!b.enable, k.as_str()));
    let (_, binding) = matching[0];

    // 2. Get wallet config
    let wallet_cfg = config.wallets.get(&binding.wallet).ok_or("wallet not configured")?;

    // 3. Read secret and get public key
    let vault_arc = vault.ok_or("vault unavailable")?;
    let secret =
        wallet_cfg.key.read_secret(Some(vault_arc)).await.map_err(|_| "get wallet key error")?;
    let keypair = secret.as_keypair().map_err(|_| "wallet key is not a keypair")?;
    let pub_key = keypair
        .public_key()
        .await
        .map_err(|_| "get public key error")?
        .ok_or("empty public key")?;

    // 4. Compute wallet address
    let wallet_addr =
        calculate_wallet_address(wallet_cfg, &pub_key).map_err(|_| "address calculation error")?;

    // 5. Parse owner and compute pool address
    let owner_addr = MsgAddressInt::from_str(owner).map_err(|_| "invalid owner address")?;
    let pool_addr = NominatorWrapperImpl::calculate_address(
        NOMINATOR_POOL_WORKCHAIN,
        &owner_addr,
        &wallet_addr,
    )
    .map_err(|_| "address calculation error")?;

    let addr_str = pool_addr
        .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
        .map_err(|_| "failed to convert address to string")?;
    Ok(addr_str)
}

/// Resolve a single TONCore pool slot address (synchronous, no vault needed —
/// only used when the address is already stored in config).
fn resolve_toncore_pool_address_via_binding_for_slot(
    slot: &TonCorePoolConfig,
    _pool_name: &str,
    _config: &AppConfig,
) -> Option<String> {
    slot.address.clone()
}

fn normalize_ton_address(addr: &str, flag_name: &str) -> anyhow::Result<String> {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--{flag_name} must not be empty");
    }
    MsgAddressInt::from_str(trimmed).map_err(|_| {
        anyhow::anyhow!(
            "invalid TON address for --{flag_name}: '{trimmed}'. Expected format: raw address or base64url"
        )
    })?;
    Ok(trimmed.to_string())
}

#[cfg(test)]
fn validate_ton_address(addr: &str, flag_name: &str) -> anyhow::Result<()> {
    normalize_ton_address(addr, flag_name).map(|_| ())
}

impl PoolRmCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if !config.pools.contains_key(&self.name) {
            anyhow::bail!("Pool '{}' not found in configuration", self.name);
        }

        for (node_name, binding) in &config.bindings {
            if binding.pool.as_deref() == Some(&self.name) {
                anyhow::bail!(
                    "Cannot remove pool '{}': referenced by binding for node '{}'",
                    self.name,
                    node_name
                );
            }
        }

        config.pools.remove(&self.name);
        save_config(&config, path)?;

        println!("\n{} Pool '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
}

fn confirm_action(prompt: &str) -> anyhow::Result<bool> {
    print!("{prompt} [y/N]: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

impl PoolDepositValidatorCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

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
        if !matches!(pool_cfg, PoolConfig::TONCore { .. }) {
            anyhow::bail!(
                "Binding '{}' points to non-TONCore pool '{}'; deposit-validator is only supported for TONCore pools",
                self.binding,
                pool_name
            );
        }

        let (wallet_address, wallet_info_data, wallet_secret) =
            wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await?;

        if wallet_info_data.account_state
            != ton_http_api_client::v2::data_models::AccountState::Active
        {
            anyhow::bail!("Wallet '{}' is {}", binding.wallet, wallet_info_data.account_state);
        }

        let pool_slot = toncore_pool_slot_from_cli_flags(self.pool_even, self.pool_odd);
        let pool_address = resolve_pool_address_from_config(pool_cfg, &wallet_address, pool_slot)?;

        let deposit_nanotons = tons_f64_to_nanotons(self.amount);
        if deposit_nanotons == 0 {
            anyhow::bail!("Amount must be greater than 0");
        }

        let gas_reserve: u64 = 2_000_000_000;
        if wallet_info_data.balance < deposit_nanotons.saturating_add(gas_reserve) {
            anyhow::bail!(
                "Insufficient wallet balance: {} TON (need {} TON + gas)",
                nanotons_to_tons_f64(wallet_info_data.balance),
                self.amount,
            );
        }

        println!(
            "\n{}\n  Binding: {}\n  Wallet:  {} ({})\n  Pool:    {}\n  Amount:  {:.9} TON\n",
            "Deposit validator summary:".cyan().bold(),
            self.binding,
            binding.wallet,
            wallet_address,
            pool_address,
            self.amount,
        );

        if !self.yes && !confirm_action("Confirm deposit?")? {
            println!("{}", "Deposit cancelled".yellow());
            return Ok(());
        }

        let wallet =
            make_wallet(rpc_client.clone(), wallet_cfg, wallet_secret, &binding.wallet).await?;

        let pool_addr_display = pool_address.to_string();
        let body = pool_messages::deposit_validator(0)?;
        let msg = wallet
            .build_message(pool_address, deposit_nanotons, body, true, None, None, None)
            .await?;

        let msg_boc = write_boc(&msg)?;
        rpc_client.send_boc(&msg_boc).await?;

        wait_for_seqno_change(
            rpc_client.clone(),
            &wallet_address,
            wallet_info_data.seqno,
            &cancellation_ctx,
            SEND_TIMEOUT,
        )
        .await?;

        println!(
            "{} Deposited {:.9} TON to pool {}",
            "OK".green().bold(),
            self.amount,
            pool_addr_display
        );
        Ok(())
    }
}

impl PoolWithdrawValidatorCmd {
    pub async fn run(&self, path: &Path, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let (config, vault, rpc_client) = load_config_vault_rpc_client(path).await?;

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
        if !matches!(pool_cfg, PoolConfig::TONCore { .. }) {
            anyhow::bail!(
                "Binding '{}' points to non-TONCore pool '{}'; withdraw-validator is only supported for TONCore pools",
                self.binding,
                pool_name
            );
        }

        let (wallet_address, wallet_info_data, wallet_secret) =
            wallet_info(rpc_client.clone(), wallet_cfg, vault.clone()).await?;

        if wallet_info_data.account_state
            != ton_http_api_client::v2::data_models::AccountState::Active
        {
            anyhow::bail!("Wallet '{}' is {}", binding.wallet, wallet_info_data.account_state);
        }

        let pool_slot = toncore_pool_slot_from_cli_flags(self.pool_even, self.pool_odd);
        let pool_address = resolve_pool_address_from_config(pool_cfg, &wallet_address, pool_slot)?;

        let withdraw_nanotons = tons_f64_to_nanotons(self.amount);
        if withdraw_nanotons == 0 {
            anyhow::bail!("Amount must be greater than 0");
        }

        println!(
            "\n{}\n  Binding: {}\n  Wallet:  {} ({})\n  Pool:    {}\n  Amount:  {:.9} TON\n",
            "Withdraw validator summary:".cyan().bold(),
            self.binding,
            binding.wallet,
            wallet_address,
            pool_address,
            self.amount,
        );

        if !self.yes && !confirm_action("Confirm withdrawal?")? {
            println!("{}", "Withdrawal cancelled".yellow());
            return Ok(());
        }

        let wallet =
            make_wallet(rpc_client.clone(), wallet_cfg, wallet_secret, &binding.wallet).await?;

        let pool_addr_display = pool_address.to_string();
        let gas_amount: u64 = 1_000_000_000;
        let body = pool_messages::withdraw_validator(0, withdraw_nanotons)?;
        let msg =
            wallet.build_message(pool_address, gas_amount, body, true, None, None, None).await?;

        let msg_boc = write_boc(&msg)?;
        rpc_client.send_boc(&msg_boc).await?;

        wait_for_seqno_change(
            rpc_client.clone(),
            &wallet_address,
            wallet_info_data.seqno,
            &cancellation_ctx,
            SEND_TIMEOUT,
        )
        .await?;

        println!(
            "{} Withdrawal of {:.9} TON requested from pool {}",
            "OK".green().bold(),
            self.amount,
            pool_addr_display
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        TonWalletVersion,
        app_config::{
            AppConfig, BindingStatus, HttpConfig, KeyConfig, NodeBinding, TonHttpApiConfig,
            WalletConfig,
        },
    };
    use std::collections::HashMap;

    fn minimal_config() -> AppConfig {
        AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: TonHttpApiConfig::default(),
            elections: None,
            voting: None,
            http: HttpConfig::default(),
            master_wallet: None,
            tick_interval: 40,
            log: None,
        }
    }

    const OWNER: &str = "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb";

    #[tokio::test]
    async fn test_display_result_with_address() {
        let config = minimal_config();
        let addr =
            "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea".to_string();
        let expected = MsgAddressInt::from_str(&addr)
            .unwrap()
            .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
            .unwrap();
        let result = get_pool_display_result("pool1", Some(&addr), None, &config, None).await;
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_display_result_no_address_no_owner() {
        let config = minimal_config();
        let result = get_pool_display_result("pool1", None, None, &config, None).await;
        assert_eq!(result, Err("pool owner not configured".to_string()));
    }

    #[tokio::test]
    async fn test_display_result_owner_no_binding() {
        let config = minimal_config();
        let owner = OWNER.to_string();
        let result = get_pool_display_result("pool1", None, Some(&owner), &config, None).await;
        assert_eq!(result, Err("no binding found".to_string()));
    }

    #[tokio::test]
    async fn test_display_result_owner_wallet_not_configured() {
        let mut config = minimal_config();
        config.bindings.insert(
            "node1".to_string(),
            NodeBinding {
                wallet: "missing_wallet".to_string(),
                pool: Some("pool1".to_string()),
                enable: false,
                status: BindingStatus::default(),
            },
        );
        let owner = OWNER.to_string();
        let result = get_pool_display_result("pool1", None, Some(&owner), &config, None).await;
        assert_eq!(result, Err("wallet not configured".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_balance_addr_error_returns_dash() {
        let addr_result: Result<String, String> = Err("some error".to_string());
        let result = resolve_pool_balance(&addr_result, &None).await;
        assert_eq!(result, Err("-".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_balance_rpc_unavailable() {
        let addr_result: Result<String, String> =
            Ok("Ef+9Ez08YSxbbVoxLlgq6L_s94BXLwPZKHE1Wa7yQCb06Ic5".to_string());
        let result = resolve_pool_balance(&addr_result, &None).await;
        assert_eq!(result, Err("-".to_string()));
    }

    #[test]
    fn test_validate_ton_address_valid_raw() {
        assert!(
            validate_ton_address(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb",
                "owner",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_ton_address_valid_masterchain() {
        assert!(
            validate_ton_address(
                "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea",
                "address",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_ton_address_valid_base64url() {
        // Round-trip: raw -> MsgAddressInt -> base64url -> validate
        let raw = "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb";
        let addr = MsgAddressInt::from_str(raw).unwrap();
        let base64url = addr.to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE).unwrap();
        assert!(validate_ton_address(&base64url, "owner").is_ok());
    }

    #[test]
    fn test_validate_ton_address_empty() {
        let err = validate_ton_address("", "owner").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_validate_ton_address_whitespace() {
        let err = validate_ton_address("   ", "owner").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_validate_ton_address_invalid() {
        let err = validate_ton_address("not-an-address", "owner").unwrap_err();
        assert!(err.to_string().contains("invalid TON address"));
    }

    #[test]
    fn test_validate_ton_address_valid_with_surrounding_spaces() {
        assert!(
            validate_ton_address(
                "  0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb  ",
                "owner",
            )
            .is_ok()
        );
    }

    #[test]
    fn test_normalize_ton_address_trims_surrounding_spaces() {
        let normalized = normalize_ton_address(
            "  0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb  ",
            "owner",
        )
        .unwrap();
        assert_eq!(
            normalized,
            "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb"
        );
    }

    #[tokio::test]
    async fn test_display_result_owner_vault_unavailable() {
        let mut config = minimal_config();
        config.bindings.insert(
            "node1".to_string(),
            NodeBinding {
                wallet: "wallet1".to_string(),
                pool: Some("pool1".to_string()),
                enable: false,
                status: BindingStatus::default(),
            },
        );
        config.wallets.insert(
            "wallet1".to_string(),
            WalletConfig {
                key: KeyConfig::VaultKey { name: "test-key".to_string() },
                version: TonWalletVersion::V3R2,
                subwallet_id: 698983191,
                workchain: -1,
            },
        );
        let owner = OWNER.to_string();
        let result = get_pool_display_result("pool1", None, Some(&owner), &config, None).await;
        assert_eq!(result, Err("vault unavailable".to_string()));
    }
}
