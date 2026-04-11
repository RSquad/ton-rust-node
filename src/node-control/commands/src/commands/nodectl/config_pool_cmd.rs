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
        AppConfig, DEFAULT_TONCORE_MAX_NOMINATORS, DEFAULT_TONCORE_MIN_NOMINATOR_STAKE_NANOTONS,
        DEFAULT_TONCORE_MIN_VALIDATOR_STAKE_NANOTONS, PoolConfig,
    },
    task_cancellation::CancellationCtx,
    ton_utils::{display_tons, nanotons_to_tons_f64, tons_f64_to_nanotons},
};
use contracts::{
    NOMINATOR_POOL_WORKCHAIN, NominatorWrapperImpl, TonWallet, resolve_toncore_nominator_pools,
    resolve_toncore_pool, ton_core_nominator::messages as pool_messages,
};
use secrets_vault::{vault::SecretVault, vault_builder::SecretVaultBuilder};
use std::{io::Write, path::Path, str::FromStr, sync::Arc};
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, MsgAddressInt, write_boc};
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum PoolAddKind {
    /// Single Nominator Pool (`kind: "snp"` in config)
    #[default]
    Snp,
    /// TONCore nominator: saved as `kind: "core"` with `dual_pools: true` (two pools; see README)
    Core,
}

#[derive(clap::Args, Clone)]
#[command(about = "Manage pools in the configuration")]
pub struct PoolCmd {
    #[command(subcommand)]
    action: PoolAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum PoolAction {
    /// Add a pool to the configuration
    Add(PoolAddCmd),
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
pub struct PoolAddCmd {
    #[arg(short = 'n', long = "name", help = "Pool name (unique identifier)")]
    name: String,
    #[arg(long = "kind", value_enum, default_value_t = PoolAddKind::Snp, help = "snp or core")]
    kind: PoolAddKind,
    #[arg(
        short = 'a',
        long = "address",
        help = "SNP: deployed pool contract address. For --kind core: saves one address for the even-round pool only (config `addresses[0]`); do not use together with --address-even or --address-odd"
    )]
    address: Option<String>,
    #[arg(
        long = "address-even",
        help = "Core: pool address for even validation rounds (optional; derived if omitted)"
    )]
    address_even: Option<String>,
    #[arg(
        long = "address-odd",
        help = "Core: pool address for odd validation rounds (optional; derived if omitted)"
    )]
    address_odd: Option<String>,
    #[arg(
        short = 'o',
        long = "owner",
        help = "SNP: owner address, raw or base64url (for deployment/verification)"
    )]
    owner: Option<String>,
    #[arg(
        long = "validator-share",
        help = "Core: validator reward share (basis points, 0–65535; e.g. 5000 ≈ 50%)"
    )]
    validator_share: Option<u16>,
    #[arg(long = "max-nominators", help = "Core: max nominators (default: 40)")]
    max_nominators: Option<u16>,
    #[arg(
        long = "min-validator-stake",
        help = "Core: min validator stake in TON (embedded at deploy; optional)"
    )]
    min_validator_stake: Option<f64>,
    #[arg(
        long = "min-nominator-stake",
        help = "Core: min nominator stake in TON (embedded at deploy; optional)"
    )]
    min_nominator_stake: Option<f64>,
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
            PoolAction::Add(cmd) => cmd.run(path).await,
            PoolAction::Ls(cmd) => cmd.run(path).await,
            PoolAction::Rm(cmd) => cmd.run(path).await,
            PoolAction::DepositValidator(cmd) => cmd.run(path, cancellation_ctx).await,
            PoolAction::WithdrawValidator(cmd) => cmd.run(path, cancellation_ctx).await,
        }
    }
}

impl PoolAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if config.pools.contains_key(&self.name) {
            anyhow::bail!(
                "Pool '{}' already exists. Remove it first or use a different name.",
                self.name
            );
        }

        let (pool_config, info) = match self.kind {
            PoolAddKind::Snp => {
                if self.address.is_none() && self.owner.is_none() {
                    anyhow::bail!(
                        "For SNP: at least one of --address or --owner must be specified"
                    );
                }

                let normalized_address = self
                    .address
                    .as_deref()
                    .map(|addr| normalize_ton_address(addr, "address"))
                    .transpose()?;
                let normalized_owner = self
                    .owner
                    .as_deref()
                    .map(|owner| normalize_ton_address(owner, "owner"))
                    .transpose()?;

                let info = match (&normalized_address, &normalized_owner) {
                    (Some(a), Some(o)) => format!("kind=snp address='{}', owner='{}'", a, o),
                    (Some(a), None) => format!("kind=snp address='{}'", a),
                    (None, Some(o)) => {
                        format!("kind=snp owner='{}' (address will be calculated on bind)", o)
                    }
                    _ => unreachable!(),
                };

                (
                    PoolConfig::SNP {
                        address: normalized_address.clone(),
                        owner: normalized_owner.clone(),
                    },
                    info,
                )
            }
            PoolAddKind::Core => {
                let share = self
                    .validator_share
                    .ok_or_else(|| anyhow::anyhow!("For core: --validator-share is required"))?;

                if self.address.is_some()
                    && (self.address_even.is_some() || self.address_odd.is_some())
                {
                    anyhow::bail!(
                        "For core: use either --address (slot 0) or --address-even / --address-odd, not both"
                    );
                }

                let normalized_address = self
                    .address
                    .as_deref()
                    .map(|a| normalize_ton_address(a, "address"))
                    .transpose()?;

                let addr_even = self
                    .address_even
                    .as_deref()
                    .map(|a| normalize_ton_address(a, "address-even"))
                    .transpose()?;
                let addr_odd = self
                    .address_odd
                    .as_deref()
                    .map(|a| normalize_ton_address(a, "address-odd"))
                    .transpose()?;

                let addresses = if normalized_address.is_some() {
                    Some(vec![normalized_address.clone()])
                } else if addr_even.is_some() || addr_odd.is_some() {
                    Some(vec![addr_even.clone(), addr_odd.clone()])
                } else {
                    None
                };

                let max_n = self.max_nominators.unwrap_or(DEFAULT_TONCORE_MAX_NOMINATORS);
                let min_validator_stake_nano = self
                    .min_validator_stake
                    .map(tons_f64_to_nanotons)
                    .unwrap_or(DEFAULT_TONCORE_MIN_VALIDATOR_STAKE_NANOTONS);
                let min_nominator_stake_nano = self
                    .min_nominator_stake
                    .map(tons_f64_to_nanotons)
                    .unwrap_or(DEFAULT_TONCORE_MIN_NOMINATOR_STAKE_NANOTONS);
                let (even_disp, odd_disp) = match &addresses {
                    Some(v) => {
                        let e = v.first().and_then(|x| x.as_deref()).unwrap_or("<will be derived>");
                        let o = v.get(1).and_then(|x| x.as_deref()).unwrap_or("<will be derived>");
                        (e, o)
                    }
                    None => ("<will be derived>", "<will be derived>"),
                };
                let info = format!(
                    "kind=core dual_pools (CLI --kind core) validator_share={}, even_round={}, odd_round={}, deploy_params: max_nominators={}, min_validator_stake={}, min_nominator_stake={}",
                    share,
                    even_disp,
                    odd_disp,
                    max_n,
                    min_validator_stake_nano,
                    min_nominator_stake_nano
                );

                (
                    PoolConfig::TONCore {
                        dual_pools: true,
                        validator_share: share,
                        address: None,
                        addresses,
                        max_nominators: max_n,
                        min_validator_stake: min_validator_stake_nano,
                        min_nominator_stake: min_nominator_stake_nano,
                    },
                    info,
                )
            }
        };

        config.pools.insert(self.name.clone(), pool_config);
        save_config(&config, path)?;
        println!("\n{} Pool '{}' added ({})\n", "OK".green().bold(), self.name, info);
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
            PoolConfig::TONCore { dual_pools: false, validator_share, address, .. } => {
                let resolved_addr = if address.is_some() {
                    address.clone()
                } else {
                    resolve_toncore_pool_address_via_binding(
                        name,
                        pool,
                        config,
                        &mut vault,
                        warn_on_error,
                    )
                    .await
                    .ok()
                };

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
                    validator_share: Some(*validator_share),
                });
            }
            PoolConfig::TONCore { dual_pools: true, validator_share, addresses, .. } => {
                let has_stored = addresses.as_ref().is_some_and(|a| a.iter().any(|o| o.is_some()));

                let resolved_addrs = if has_stored {
                    addresses.as_ref().map(|a| {
                        let mut labels: Vec<String> = a
                            .iter()
                            .map(|o| o.clone().unwrap_or_else(|| "<not deployed>".into()))
                            .collect();
                        while labels.len() < 2 {
                            labels.push("<not deployed>".into());
                        }
                        labels.truncate(2);
                        labels
                    })
                } else {
                    resolve_toncore_nominator_pools_addresses_via_binding(
                        name,
                        pool,
                        config,
                        &mut vault,
                        warn_on_error,
                    )
                    .await
                    .ok()
                };

                let pool_balances =
                    if let (Some(addrs), Some(client)) = (&resolved_addrs, rpc_client) {
                        let mut bals = Vec::new();
                        for addr_str in addrs {
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
                    addresses: resolved_addrs,
                    balances: pool_balances,
                    validator_share: Some(*validator_share),
                });
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
        "  {:<15} {:<6} {:<14} {:<50} {}",
        "Name".cyan().bold(),
        "Kind".cyan().bold(),
        "Balance".cyan().bold(),
        "Address".cyan().bold(),
        "Owner".cyan().bold(),
    );
    println!("  {}", "─".repeat(137).dimmed());

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
                    "  {:<15} {:<6} {:<14} {:<50} {}",
                    v.name, "SNP", display_balance, display_addr, display_owner,
                );
            }
            "Core" => {
                let display_addr = v.address.as_deref().unwrap_or("<not deployed>");
                let display_balance =
                    v.balance.as_deref().map(|s| s.white()).unwrap_or_else(|| "-".red());
                println!(
                    "  {:<15} {:<6} {:<14} {:<50} {}",
                    v.name, "Core", display_balance, display_addr, "-",
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
                println!("  {:<15} {:<8} {:<28} {}", v.name, "TONCore", display_balance, addrs,);
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

async fn resolve_validator_addr_via_binding(
    pool_name: &str,
    config: &AppConfig,
    vault: &mut Option<Option<Arc<SecretVault>>>,
    warn_on_error: bool,
) -> Result<MsgAddressInt, String> {
    if vault.is_none() {
        *vault = Some(match SecretVaultBuilder::from_env().await {
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
    let vault_arc = vault.as_ref().and_then(|v| v.clone()).ok_or("vault unavailable")?;

    let mut matching: Vec<_> =
        config.bindings.iter().filter(|(_, b)| b.pool.as_deref() == Some(pool_name)).collect();
    if matching.is_empty() {
        return Err("no binding found".to_string());
    }
    matching.sort_by_key(|(k, b)| (!b.enable, k.as_str()));
    let (_, binding) = matching[0];

    let wallet_cfg = config.wallets.get(&binding.wallet).ok_or("wallet not configured")?;
    let secret =
        wallet_cfg.key.read_secret(Some(vault_arc)).await.map_err(|_| "get wallet key error")?;
    let keypair = secret.as_keypair().map_err(|_| "wallet key is not a keypair")?;
    let pub_key = keypair
        .public_key()
        .await
        .map_err(|_| "get public key error")?
        .ok_or("empty public key")?;
    calculate_wallet_address(wallet_cfg, &pub_key)
        .map_err(|_| "address calculation error".to_string())
}

async fn resolve_toncore_pool_address_via_binding(
    pool_name: &str,
    pool: &PoolConfig,
    config: &AppConfig,
    vault: &mut Option<Option<Arc<SecretVault>>>,
    warn_on_error: bool,
) -> Result<String, String> {
    let PoolConfig::TONCore {
        dual_pools: false,
        validator_share,
        max_nominators,
        min_validator_stake,
        min_nominator_stake,
        ..
    } = pool
    else {
        return Err("not a single-pool TONCore".into());
    };
    let wallet_addr =
        resolve_validator_addr_via_binding(pool_name, config, vault, warn_on_error).await?;
    let resolved = resolve_toncore_pool(
        &wallet_addr,
        *validator_share,
        None,
        Some(*max_nominators),
        Some(*min_validator_stake),
        Some(*min_nominator_stake),
    )
    .map_err(|e| format!("resolve error: {e}"))?;
    resolved
        .address
        .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
        .map_err(|_| "address conversion error".into())
}

async fn resolve_toncore_nominator_pools_addresses_via_binding(
    pool_name: &str,
    pool: &PoolConfig,
    config: &AppConfig,
    vault: &mut Option<Option<Arc<SecretVault>>>,
    warn_on_error: bool,
) -> Result<Vec<String>, String> {
    let PoolConfig::TONCore {
        dual_pools: true,
        validator_share,
        max_nominators,
        min_validator_stake,
        min_nominator_stake,
        ..
    } = pool
    else {
        return Err("not a TONCore nominator (dual_pools) pool".into());
    };
    let wallet_addr =
        resolve_validator_addr_via_binding(pool_name, config, vault, warn_on_error).await?;
    let resolved = resolve_toncore_nominator_pools(
        &wallet_addr,
        *validator_share,
        None,
        Some(*max_nominators),
        Some(*min_validator_stake),
        Some(*min_nominator_stake),
    )
    .map_err(|e| format!("resolve error: {e}"))?;
    resolved
        .iter()
        .map(|r| {
            r.address
                .to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)
                .map_err(|_| "address conversion error".to_string())
        })
        .collect()
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
