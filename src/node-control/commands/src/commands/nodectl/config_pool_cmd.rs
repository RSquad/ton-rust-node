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
        SEND_TIMEOUT, api_delete, api_get, api_post, confirm, get_wallet_config,
        load_config_vault_rpc_client, make_wallet, require_config,
        resolve_pool_address_from_config, resolve_service_url, toncore_pool_slot_from_cli_flags,
        wait_for_seqno_change, wallet_info,
    },
};
use colored::{ColoredString, Colorize};
use common::{
    app_config::{PoolConfig, TonCoreDeployLayout},
    task_cancellation::CancellationCtx,
    ton_utils::{display_tons, nanotons_to_tons_f64, tons_f64_to_nanotons},
};
use contracts::{TonWallet, nominator::ton_core_pool as pool_messages};
use std::{path::Path, str::FromStr};
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, MsgAddressInt, write_boc};

fn parse_cli_deploy_layout(s: &str) -> Result<TonCoreDeployLayout, String> {
    match s.trim() {
        "embedded_code" => Ok(TonCoreDeployLayout::EmbeddedCode),
        "activate_upgrade" => Ok(TonCoreDeployLayout::ActivateUpgrade),
        other => Err(format!(
            "invalid deploy-layout '{other}': expected 'embedded_code' or 'activate_upgrade'"
        )),
    }
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
    Add(PoolAddSubCmd),
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
        conflicts_with = "validator_share_percent",
        help = "Validator reward share in basis points (100 bp = 1%; must be < 10000 so nominators receive pool rewards, e.g. 5000 = 50%)"
    )]
    validator_share: Option<u16>,

    #[arg(
        long = "validator-share-percent",
        conflicts_with = "validator_share",
        help = "Validator reward share as percent of pool rewards [0.0, 100.0), below 100% so nominators earn rewards (e.g. 50.4 → 5040 bp). Mutually exclusive with --validator-share"
    )]
    validator_share_percent: Option<f64>,

    #[arg(long = "max-nominators", help = "Max nominators (default: 40)")]
    max_nominators: Option<u16>,

    #[arg(
        long = "min-validator-stake",
        help = "Minimum validator stake in TON (server default when omitted: 100 000)"
    )]
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
    #[arg(
        long = "deploy-layout",
        value_parser = parse_cli_deploy_layout,
        help = "StateInit.code layout: embedded_code (default) or activate_upgrade (bootstrap + SETCODE on run)",
    )]
    deploy_layout: Option<TonCoreDeployLayout>,
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
    #[arg(
        short = 'a',
        long = "amount",
        help = "Validator stake to credit (TON); message adds 1 TON pool processing fee on-chain"
    )]
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
    pub async fn run(
        &self,
        config_path: Option<&str>,
        cancellation_ctx: CancellationCtx,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            // Both SNP and Core add go through REST.
            PoolAction::Add(cmd) => match &cmd.action {
                Some(PoolAddAction::Snp(cmd)) => cmd.run(url, token, config_path).await,
                Some(PoolAddAction::Core(cmd)) => cmd.run(url, token, config_path).await,
                None => cmd.snp.run(url, token, config_path).await,
            },
            PoolAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            PoolAction::Rm(cmd) => cmd.run(url, token, config_path).await,
            // Validator deposits/withdrawals require local secrets + RPC; not yet in REST.
            PoolAction::DepositValidator(cmd) => {
                cmd.run(require_config(config_path)?, cancellation_ctx).await
            }
            PoolAction::WithdrawValidator(cmd) => {
                cmd.run(require_config(config_path)?, cancellation_ctx).await
            }
        }
    }
}

#[derive(serde::Serialize)]
struct PoolAddBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
}

impl PoolAddCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let name = self.name.as_ref().ok_or_else(|| anyhow::anyhow!("--name must be specified"))?;
        if self.address.is_none() && self.owner.is_none() {
            anyhow::bail!("At least one of --address or --owner must be specified");
        }

        let base_url = resolve_service_url(url, config_path)?;
        let body = PoolAddBody {
            name: name.as_str(),
            address: self.address.as_deref(),
            owner: self.owner.as_deref(),
        };
        api_post(&base_url, "/v1/pools", token, &body).await?;

        let info = match (&self.address, &self.owner) {
            (Some(a), Some(o)) => format!("address='{}', owner='{}'", a, o),
            (Some(a), None) => format!("address='{}'", a),
            (None, Some(o)) => format!("owner='{}' (address will be calculated on bind)", o),
            (None, None) => String::new(),
        };
        println!("\n{} Pool '{}' added ({})\n", "OK".green().bold(), name, info);
        Ok(())
    }
}

/// Wire body for `POST /v1/pools/core`. Mirrors `service::PoolAddCoreRequest`
/// — duplicated rather than shared to keep the CLI free of a service-crate
/// dependency.
#[derive(serde::Serialize)]
struct PoolAddCoreBody<'a> {
    name: &'a str,
    slot: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validator_share: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_nominators: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_validator_stake: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_nominator_stake: Option<u64>,
    deploy_layout: TonCoreDeployLayout,
}

fn share_pct_to_bp(pct: f64) -> anyhow::Result<u16> {
    anyhow::ensure!(
        pct.is_finite() && (0.0..100.0).contains(&pct),
        "validator-share-percent must be finite in [0.0, 100.0): 100% would leave no pool rewards for nominators (got {pct})"
    );
    let bp = (pct * 100.0).round() as u16;
    anyhow::ensure!(
        bp < 10_000,
        "validator-share-percent rounds to {bp} basis points (100% validator share); nominators would receive no rewards — use a lower percent"
    );
    Ok(bp)
}

/// Validate `--validator-share` raw basis-point input.
///
/// Mirrors the server-side check (`0..=10_000`) but tightens the upper bound
/// to **`< 10_000`**: a 100 % validator share would leave nominators with no
/// pool rewards, which is almost always operator error rather than intent.
fn validate_validator_share_bp(bp: u16) -> anyhow::Result<u16> {
    match bp {
        10_000 => anyhow::bail!(
            "--validator-share 10000 bp is 100%: nominators would receive no pool rewards — use a value below 10000 bp"
        ),
        bp if bp > 10_000 => {
            anyhow::bail!("validator_share must be in 0..10000 basis points (<100%; got {bp})")
        }
        bp => Ok(bp),
    }
}

/// Basis points → percentage for display (100 bp = 1%; inverse of [`share_pct_to_bp`]).
fn bp_to_pct(bp: u16) -> f64 {
    bp as f64 / 100.0
}

impl PoolAddCoreCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.address.is_none()
            && self.validator_share.is_none()
            && self.validator_share_percent.is_none()
        {
            anyhow::bail!(
                "At least one of --address, --validator-share, or --validator-share-percent must be specified"
            );
        }

        let resolved_validator_share: Option<u16> =
            match (self.validator_share, self.validator_share_percent) {
                (Some(bp), None) => Some(validate_validator_share_bp(bp)?),
                (None, Some(pct)) => Some(share_pct_to_bp(pct)?),
                (None, None) => None,
                (Some(_), Some(_)) => {
                    unreachable!("clap conflicts validator_share with validator_share_percent")
                }
            };

        if resolved_validator_share.is_none()
            && (self.max_nominators.is_some()
                || self.min_validator_stake.is_some()
                || self.min_nominator_stake.is_some())
        {
            anyhow::bail!(
                "max_nominators / min_validator_stake / min_nominator_stake require a validator share (--validator-share or --validator-share-percent)"
            );
        }

        let slot_name = if self.odd { "odd" } else { "even" };

        // Validate locally so the user gets the standard "invalid TON address"
        // message instead of a 400 from the server.
        let address =
            self.address.as_deref().map(|a| normalize_ton_address(a, "address")).transpose()?;

        let base_url = resolve_service_url(url, config_path)?;
        let deploy_layout = self.deploy_layout.unwrap_or_default();
        let body = PoolAddCoreBody {
            name: &self.name,
            slot: slot_name,
            address: address.as_deref(),
            validator_share: resolved_validator_share,
            max_nominators: self.max_nominators,
            // CLI flags accept TON (f64) — convert to nanotons for the API.
            min_validator_stake: self.min_validator_stake.map(tons_f64_to_nanotons),
            min_nominator_stake: self.min_nominator_stake.map(tons_f64_to_nanotons),
            deploy_layout,
        };
        api_post(&base_url, "/v1/pools/core", token, &body).await?;

        let mut info_parts = Vec::new();
        if let Some(vs) = resolved_validator_share {
            let pct = bp_to_pct(vs);
            info_parts.push(format!("validator_share={vs} bp (~{pct:.2}%)"));
        }
        if let Some(a) = &address {
            info_parts.push(format!("address='{a}'"));
        }
        if !deploy_layout.is_embedded_code() {
            info_parts.push("deploy_layout=activate_upgrade".to_string());
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

/// Account / deployment state for a TONCore pool slot in [`GET /v1/pools`].
///
/// Wire strings match `TonCorePoolSlotDto.state` in `config_handlers` (RPC
/// [`ton_http_api_client::v2::data_models::AccountState`] as lowercase, plus
/// synthetic `"not deployed"` and `"error"`).
///
/// Serde shape is `rename_all = "lowercase"` with [`Self::NotDeployed`] mapped
/// to `"not deployed"` (single multi-word token on the wire).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum TonCorePoolSlotWireState {
    #[serde(rename = "not deployed", alias = "")]
    #[default]
    NotDeployed,
    Active,
    Frozen,
    Error,
}

/// Source of deploy-style pool parameters (`validator_share`, stake thresholds) in
/// [`GET /v1/pools`] JSON — matches `TonCorePoolSlotDto::data_source` on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum TonCorePoolSlotDataSource {
    Chain,
    Config,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PoolView {
    name: String,
    kind: String,
    balance: Option<u64>,
    address: Option<String>,
    owner: Option<String>,
    /// Per-slot data for TONCore pools; absent for SNP pools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slots: Option<Vec<TonCorePoolSlotView>>,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct TonCorePoolSlotView {
    /// "even" (slot 0) or "odd" (slot 1)
    slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(default)]
    state: TonCorePoolSlotWireState,
    /// Whether deploy-style fields came from chain (`get_pool_data`) or config fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_source: Option<TonCorePoolSlotDataSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    balance: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validator_share: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_nominators: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_validator_stake: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_nominator_stake: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nominators_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stake_amount_sent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validator_amount: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pool_state: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_election_id: Option<u32>,
    #[serde(default)]
    deploy_layout: TonCoreDeployLayout,
}

impl PoolLsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/pools", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let views: Vec<PoolView> = serde_json::from_value(resp["result"].clone())?;

        if views.is_empty() {
            match self.format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Table => println!("\n{}\n", "No pools configured".yellow()),
            }
            return Ok(());
        }

        match self.format {
            OutputFormat::Json => print_pools_json(&views)?,
            OutputFormat::Table => print_pools_table(&views),
        }
        Ok(())
    }
}

fn print_pools_json(views: &[PoolView]) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(views)?);
    Ok(())
}

fn display_ton_address(addr: Option<&str>) -> ColoredString {
    addr.map(|s| {
        MsgAddressInt::from_str(s)
            .and_then(|a| a.to_string_custom(ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE))
            .unwrap_or_else(|_| s.to_string())
    })
    .map(|s| s.white())
    .unwrap_or_else(|| "-".red())
}

fn print_pools_table(views: &[PoolView]) {
    let snp: Vec<&PoolView> = views.iter().filter(|v| v.kind == "SNP").collect();
    let core: Vec<&PoolView> = views.iter().filter(|v| v.kind == "Core").collect();

    if !snp.is_empty() {
        print_snp_table(&snp);
    }
    if !core.is_empty() {
        print_toncore_table(&core);
    }
}

fn print_snp_table(views: &[&PoolView]) {
    println!("\n{} {} ({})\n", "OK".green().bold(), "SNP pools:".green(), views.len());
    println!(
        "  {:<15} {:<14} {:<50} {}",
        "Name".cyan().bold(),
        "Balance".cyan().bold(),
        "Address".cyan().bold(),
        "Owner".cyan().bold(),
    );
    println!("  {}", "─".repeat(130).dimmed());

    for v in views {
        let display_addr = display_ton_address(v.address.as_deref());
        let display_owner = display_ton_address(v.owner.as_deref());
        let display_balance =
            v.balance.map(|b| display_tons(b).white()).unwrap_or_else(|| "-".red());
        println!("  {:<15} {:<14} {:<50} {}", v.name, display_balance, display_addr, display_owner,);
    }
    println!();
}

fn print_toncore_table(views: &[&PoolView]) {
    let total_slots: usize = views.iter().filter_map(|v| v.slots.as_ref()).map(|s| s.len()).sum();
    println!(
        "\n{} {} ({} pools, {} slots)\n",
        "OK".green().bold(),
        "TONCore pools:".green(),
        views.len(),
        total_slots,
    );
    println!(
        "  {:<15} {:<5} {:<13} {:<7} {:<14} {:<10} {:<8} {:<14} {:<14} {}",
        "Name".cyan().bold(),
        "Slot".cyan().bold(),
        "State".cyan().bold(),
        "Src".cyan().bold(),
        "Balance".cyan().bold(),
        "Noms".cyan().bold(),
        "Share".cyan().bold(),
        "Validator".cyan().bold(),
        "Min nom.stake".cyan().bold(),
        "Address".cyan().bold(),
    );
    println!("  {}", "─".repeat(170).dimmed());

    for v in views {
        let Some(slots) = v.slots.as_ref() else { continue };
        if slots.is_empty() {
            // Pool exists in config but no slots configured at all — surface this.
            println!("  {:<15} {}", v.name, "(no slots configured)".dimmed());
            continue;
        }
        for s in slots {
            let display_addr = display_ton_address(s.address.as_deref());
            let display_state = display_toncore_slot_row(s);
            let display_src = display_toncore_data_source(s.data_source);
            let display_balance =
                s.balance.map(|b| display_tons(b).white()).unwrap_or_else(|| "-".red());
            let noms = match (s.nominators_count, s.max_nominators) {
                (Some(n), Some(m)) => format!("{n}/{m}"),
                (Some(n), None) => format!("{n}"),
                _ => "-".to_string(),
            };
            // `validator_share` is on-chain in basis points; use bp_to_pct for display.
            let share = s
                .validator_share
                .map(|sh| format!("{:.2}%", bp_to_pct(sh)))
                .unwrap_or_else(|| "-".to_string());
            let validator_amount = s
                .validator_amount
                .map(|b| display_tons(b).to_string())
                .unwrap_or_else(|| "-".to_string());
            let min_nom = s
                .min_nominator_stake
                .map(|b| display_tons(b).to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  {:<15} {:<5} {:<13} {:<7} {:<14} {:<10} {:<8} {:<14} {:<14} {}",
                v.name,
                s.slot,
                display_state,
                display_src,
                display_balance,
                noms,
                share,
                validator_amount,
                min_nom,
                display_addr,
            );
        }
    }
    println!();
}

/// How deploy-style TONCore slot fields were resolved (chain vs config.toml merge).
fn display_toncore_data_source(src: Option<TonCorePoolSlotDataSource>) -> ColoredString {
    match src {
        Some(TonCorePoolSlotDataSource::Chain) => "chain".green(),
        Some(TonCorePoolSlotDataSource::Config) => "config".yellow(),
        None => "-".normal(),
    }
}

/// Contract-internal pool state from `get_pool_data` (idle / staking / …).
fn display_toncore_contract_state(state: i32) -> ColoredString {
    match state {
        -1 => "error".red(),
        0 => "idle".cyan(),
        1 => "staking".green(),
        2 => "staking".green(),
        _ => "unknown".red(),
    }
}

/// Row "State" for TONCore table: account / deployment state wins for undeployed accounts; then
/// on-chain pool lifecycle from `pool_state` when present.
fn display_toncore_slot_row(s: &TonCorePoolSlotView) -> ColoredString {
    use TonCorePoolSlotWireState::*;
    if matches!(s.state, NotDeployed) {
        return "not deployed".yellow();
    }
    if let Some(ps) = s.pool_state {
        return display_toncore_contract_state(ps);
    }
    match s.state {
        Error => "error".red(),
        Active => "active".green(),
        Frozen => "frozen".yellow(),
        NotDeployed => "not deployed".yellow(),
    }
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
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_delete(&base_url, &format!("/v1/pools/{}", self.name), token).await?;
        println!("\n{} Pool '{}' removed\n", "OK".green().bold(), self.name);
        Ok(())
    }
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

        // Pool contract keeps DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS (1 TON) from the message value;
        // send stake + fee so the credited validator_amount matches `--amount`.
        let msg_value_nanotons =
            deposit_nanotons.saturating_add(pool_messages::DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS);

        let gas_reserve: u64 = 2_000_000_000;
        if wallet_info_data.balance < msg_value_nanotons.saturating_add(gas_reserve) {
            anyhow::bail!(
                "Insufficient wallet balance: {} TON (need {:.9} TON on-chain: {:.9} stake + {:.9} pool fee + {:.9} TON gas reserve)",
                nanotons_to_tons_f64(wallet_info_data.balance),
                nanotons_to_tons_f64(msg_value_nanotons.saturating_add(gas_reserve)),
                self.amount,
                nanotons_to_tons_f64(pool_messages::DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS),
                nanotons_to_tons_f64(gas_reserve),
            );
        }

        println!("\n{}", "Deposit validator summary:".cyan().bold());
        println!("  Binding: {}", self.binding);
        println!("  Wallet:  {} ({})", binding.wallet, wallet_address);
        println!("  Pool:    {}", pool_address);
        println!("  Credited to validator stake: {:.9} TON", self.amount);
        println!(
            "  Message value (stake + {:.9} TON pool fee): {:.9} TON\n",
            nanotons_to_tons_f64(pool_messages::DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS),
            nanotons_to_tons_f64(msg_value_nanotons),
        );

        if !self.yes && !confirm("Confirm deposit?")? {
            println!("{}", "Deposit cancelled".yellow());
            return Ok(());
        }

        let wallet =
            make_wallet(rpc_client.clone(), wallet_cfg, wallet_secret, &binding.wallet).await?;

        let pool_addr_display = pool_address.to_string();
        let body = pool_messages::deposit_validator(0)?;
        let msg = wallet
            .build_message(pool_address, msg_value_nanotons, body, true, None, None, None)
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
            "{} Credited {:.9} TON validator stake to pool {} (sent {:.9} TON including {:.9} TON pool fee)",
            "OK".green().bold(),
            self.amount,
            pool_addr_display,
            nanotons_to_tons_f64(msg_value_nanotons),
            nanotons_to_tons_f64(pool_messages::DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS),
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

        if !self.yes && !confirm("Confirm withdrawal?")? {
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

    // ---- validator share validation ----

    #[test]
    fn share_pct_to_bp_accepts_valid_values() {
        assert_eq!(share_pct_to_bp(0.0).unwrap(), 0);
        assert_eq!(share_pct_to_bp(50.0).unwrap(), 5000);
        assert_eq!(share_pct_to_bp(50.4).unwrap(), 5040);
        assert_eq!(share_pct_to_bp(99.99).unwrap(), 9999);
    }

    #[test]
    fn share_pct_to_bp_rejects_exact_100() {
        let err = share_pct_to_bp(100.0).unwrap_err().to_string();
        assert!(err.contains("[0.0, 100.0)"), "unexpected error: {err}");
    }

    #[test]
    fn share_pct_to_bp_rejects_above_100() {
        let err = share_pct_to_bp(150.0).unwrap_err().to_string();
        assert!(err.contains("[0.0, 100.0)"), "unexpected error: {err}");
    }

    #[test]
    fn share_pct_to_bp_rejects_negative() {
        let err = share_pct_to_bp(-0.5).unwrap_err().to_string();
        assert!(err.contains("[0.0, 100.0)"), "unexpected error: {err}");
    }

    #[test]
    fn share_pct_to_bp_rejects_non_finite() {
        assert!(share_pct_to_bp(f64::NAN).is_err());
        assert!(share_pct_to_bp(f64::INFINITY).is_err());
    }

    #[test]
    fn share_pct_to_bp_rejects_rounding_to_10000() {
        // 99.995 rounds up to 10000 bp — must be rejected so nominators always
        // keep at least 1 bp of pool rewards.
        let err = share_pct_to_bp(99.995).unwrap_err().to_string();
        assert!(err.contains("rounds to 10000 basis points"), "unexpected error: {err}");
    }

    #[test]
    fn validate_validator_share_bp_accepts_valid_values() {
        assert_eq!(validate_validator_share_bp(0).unwrap(), 0);
        assert_eq!(validate_validator_share_bp(5000).unwrap(), 5000);
        assert_eq!(validate_validator_share_bp(9999).unwrap(), 9999);
    }

    #[test]
    fn validate_validator_share_bp_rejects_exact_10000_with_nominator_hint() {
        let err = validate_validator_share_bp(10_000).unwrap_err().to_string();
        assert!(
            err.contains("nominators would receive no pool rewards"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_validator_share_bp_rejects_above_10000() {
        let err = validate_validator_share_bp(15_000).unwrap_err().to_string();
        assert!(err.contains("0..10000 basis points"), "unexpected error: {err}");
    }

    // ---- PoolView serde / table rendering ----

    fn snp_view() -> PoolView {
        PoolView {
            name: "snp1".into(),
            kind: "SNP".into(),
            balance: Some(123_000_000_000),
            address: Some(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb".into(),
            ),
            owner: Some(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb".into(),
            ),
            slots: None,
        }
    }

    fn core_view_two_slots() -> PoolView {
        PoolView {
            name: "core1".into(),
            kind: "Core".into(),
            balance: None,
            address: None,
            owner: None,
            slots: Some(vec![
                TonCorePoolSlotView {
                    slot: "even".into(),
                    address: Some(
                        "-1:0000000000000000000000000000000000000000000000000000000000000001"
                            .into(),
                    ),
                    state: TonCorePoolSlotWireState::Active,
                    data_source: Some(TonCorePoolSlotDataSource::Chain),
                    balance: Some(50_000_000_000),
                    validator_share: Some(4000),
                    max_nominators: Some(40),
                    min_validator_stake: Some(10_000_000_000_000),
                    min_nominator_stake: Some(10_000_000_000_000),
                    nominators_count: Some(3),
                    stake_amount_sent: Some(0),
                    validator_amount: Some(0),
                    pool_state: Some(0),
                    last_election_id: Some(1_700_000_000),
                    ..Default::default()
                },
                TonCorePoolSlotView {
                    slot: "odd".into(),
                    address: None,
                    state: TonCorePoolSlotWireState::NotDeployed,
                    ..Default::default()
                },
            ]),
        }
    }

    #[test]
    fn pool_view_snp_json_no_slots_field() {
        // Round-trip: PoolView → JSON → PoolView. The SNP view must serialize
        // without a `slots` field so the API contract for SNP stays stable.
        let view = snp_view();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["kind"], "SNP");
        assert!(json.get("slots").is_none(), "SNP must not emit slots field");
        let back: PoolView = serde_json::from_value(json).unwrap();
        assert_eq!(back.kind, "SNP");
        assert!(back.slots.is_none());
    }

    #[test]
    fn pool_view_toncore_json_round_trip() {
        let view = core_view_two_slots();
        let json = serde_json::to_value(&view).unwrap();
        let slots = json["slots"].as_array().expect("slots present");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0]["slot"], "even");
        assert_eq!(slots[0]["data_source"], "chain");
        assert_eq!(slots[0]["validator_share"], 4000);
        assert_eq!(slots[1]["slot"], "odd");
        assert_eq!(slots[1]["state"], "not deployed");
        assert!(slots[1].get("data_source").is_none());
        // Optional fields on the not-deployed slot must be omitted, not null.
        assert!(slots[1].get("balance").is_none());
        assert!(slots[1].get("validator_share").is_none());

        let back: PoolView = serde_json::from_value(json).unwrap();
        let back_slots = back.slots.as_ref().unwrap();
        assert_eq!(back_slots[0].nominators_count, Some(3));
        assert_eq!(back_slots[0].data_source, Some(TonCorePoolSlotDataSource::Chain));
        assert_eq!(back_slots[1].state, TonCorePoolSlotWireState::NotDeployed);
        assert!(back_slots[1].data_source.is_none());
    }

    #[test]
    fn print_pools_table_handles_mixed_kinds_without_panic() {
        // Smoke test: rendering must not panic on the new two-table layout
        // when both an SNP pool and a TONCore pool with mixed slots are
        // present. Validates the column-formatting code paths for every slot
        // state we care about.
        let views = vec![snp_view(), core_view_two_slots()];
        print_pools_table(&views);
    }

    #[test]
    fn print_pools_table_skips_empty_sections() {
        // Only TONCore configured → SNP renderer must be skipped (no panic,
        // no "SNP pools (0)" header).
        print_pools_table(&[core_view_two_slots()]);
        // Only SNP configured → TONCore renderer is skipped.
        print_pools_table(&[snp_view()]);
    }
}
