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
    utils::{api_get, api_post, resolve_service_url},
};
use colored::Colorize;
use common::{app_config::ContractsAutomationConfig, ton_utils::tons_f64_to_nanotons};

const SETTINGS_PATH: &str = "/v1/automation/settings";

/// Contracts automation (auto-deploy / auto-topup): REST client for `/v1/automation/settings`.
#[derive(clap::Args, Clone)]
#[command(
    about = "Manage contracts task automation (auto-deploy / auto-topup) via the service API"
)]
pub struct AutomationCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: Option<String>,

    #[arg(
        short = 'u',
        long = "url",
        value_hint = clap::ValueHint::Url,
        help = "URL to the node control service API (takes precedence over --config; defaults to http://127.0.0.1:8080 if not --url, --config, or NODECTL_URL environment variable are provided)",
        env = "NODECTL_URL",
        global = true,
    )]
    url: Option<String>,

    #[arg(
        long = "token",
        env = "NODECTL_API_TOKEN",
        hide_env = true,
        value_name = "TOKEN",
        help = "JWT token to authenticate with the service API",
        global = true
    )]
    token: Option<String>,

    #[command(subcommand)]
    action: AutomationAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum AutomationAction {
    /// Show current automation settings
    Ls(LsCmd),
    /// Set the contracts task tick interval (seconds)
    Tick(TickCmd),
    /// Update wallet deploy / top-up / threshold (amounts in TON)
    Wallet(WalletCmd),
    /// Update pool deploy amounts (TON); see `--help` for `--deploy` vs per-kind flags
    Pool(PoolCmd),
    /// Turn auto-deploy or auto-topup on (`deploy` / `topup` subcommands)
    Enable(EnableCmd),
    /// Turn auto-deploy or auto-topup off (`deploy` / `topup` subcommands)
    Disable(DisableCmd),
}

#[derive(clap::Args, Clone)]
pub struct LsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::Args, Clone)]
pub struct TickCmd {
    #[arg(help = "Tick interval in seconds")]
    seconds: u64,
}

#[derive(clap::Args, Clone)]
pub struct WalletCmd {
    #[arg(long, help = "Amount in TON sent when deploying a validator wallet from the master")]
    deploy: Option<f64>,

    #[arg(long, help = "Amount in TON to send when topping up a wallet below the threshold")]
    topup: Option<f64>,

    #[arg(long, help = "Minimum wallet balance in TON; below this, auto-topup sends `--topup`")]
    threshold: Option<f64>,
}

#[derive(clap::Args, Clone)]
pub struct PoolCmd {
    #[arg(
        long,
        help = "Default deploy amount in TON for both pool kinds (overridden by --snp / --ton-core when set)"
    )]
    deploy: Option<f64>,

    #[arg(long, help = "Deploy amount in TON for Single Nominator pools")]
    snp: Option<f64>,

    #[arg(long, help = "Deploy amount in TON for TONCore pools")]
    ton_core: Option<f64>,
}

#[derive(clap::Args, Clone)]
pub struct EnableCmd {
    #[command(subcommand)]
    target: EnableTarget,
}

#[derive(clap::Subcommand, Clone, Copy)]
pub enum EnableTarget {
    /// Turn on automatic deployment of validator wallets and pools
    Deploy,
    /// Turn on automatic validator wallet top-ups from the master
    Topup,
}

#[derive(clap::Args, Clone)]
pub struct DisableCmd {
    #[command(subcommand)]
    target: DisableTarget,
}

#[derive(clap::Subcommand, Clone, Copy)]
pub enum DisableTarget {
    /// Turn off automatic deployment of validator wallets and pools
    Deploy,
    /// Turn off automatic validator wallet top-ups
    Topup,
}

impl AutomationCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let url = self.url.as_deref();
        let token = self.token.as_deref();
        let config_path = self.config.as_deref();

        match &self.action {
            AutomationAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            AutomationAction::Tick(cmd) => cmd.run(url, token, config_path).await,
            AutomationAction::Wallet(cmd) => cmd.run(url, token, config_path).await,
            AutomationAction::Pool(cmd) => cmd.run(url, token, config_path).await,
            AutomationAction::Enable(cmd) => cmd.run(url, token, config_path).await,
            AutomationAction::Disable(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

impl LsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, SETTINGS_PATH, token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let result = &resp["result"];

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
            OutputFormat::Table => {
                let cfg: ContractsAutomationConfig = serde_json::from_value(result.clone())
                    .map_err(|e| anyhow::anyhow!("failed to parse automation settings: {e}"))?;
                print_table(&cfg);
            }
        }
        Ok(())
    }
}

impl TickCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = ContractsAutomationPatchBody {
            tick_interval_sec: Some(self.seconds),
            ..Default::default()
        };
        patch(url, token, config_path, &body).await
    }
}

impl WalletCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.deploy.is_none() && self.topup.is_none() && self.threshold.is_none() {
            anyhow::bail!("Specify at least one of: --deploy, --topup, --threshold");
        }
        let body = ContractsAutomationPatchBody {
            wallet: Some(WalletPatchBody {
                deploy: self.deploy.map(tons_f64_to_nanotons),
                topup: self.topup.map(tons_f64_to_nanotons),
                threshold: self.threshold.map(tons_f64_to_nanotons),
            }),
            ..Default::default()
        };
        patch(url, token, config_path, &body).await
    }
}

impl PoolCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.deploy.is_none() && self.snp.is_none() && self.ton_core.is_none() {
            anyhow::bail!("Specify at least one of: --deploy, --snp, --ton-core");
        }
        let snp = self.snp.or(self.deploy).map(tons_f64_to_nanotons);
        let ton_core = self.ton_core.or(self.deploy).map(tons_f64_to_nanotons);
        let body = ContractsAutomationPatchBody {
            pool: Some(PoolPatchBody { snp, ton_core }),
            ..Default::default()
        };
        patch(url, token, config_path, &body).await
    }
}

impl EnableCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = match self.target {
            EnableTarget::Deploy => {
                ContractsAutomationPatchBody { auto_deploy: Some(true), ..Default::default() }
            }
            EnableTarget::Topup => {
                ContractsAutomationPatchBody { auto_topup: Some(true), ..Default::default() }
            }
        };
        patch(url, token, config_path, &body).await
    }
}

impl DisableCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = match self.target {
            DisableTarget::Deploy => {
                ContractsAutomationPatchBody { auto_deploy: Some(false), ..Default::default() }
            }
            DisableTarget::Topup => {
                ContractsAutomationPatchBody { auto_topup: Some(false), ..Default::default() }
            }
        };
        patch(url, token, config_path, &body).await
    }
}

async fn patch(
    url: Option<&str>,
    token: Option<&str>,
    config_path: Option<&str>,
    body: &ContractsAutomationPatchBody,
) -> anyhow::Result<()> {
    let base_url = resolve_service_url(url, config_path)?;
    api_post(&base_url, SETTINGS_PATH, token, body).await?;
    println!("{} Automation settings updated", "OK".green().bold());
    Ok(())
}

fn nanotons_to_ton_display(n: u64) -> String {
    format!("{:.4}", n as f64 / 1e9)
}

fn print_table(cfg: &ContractsAutomationConfig) {
    println!("\n{} {}\n", "OK".green().bold(), "Automation".green());
    println!("  {:<28} {}s", "Tick interval:".cyan().bold(), cfg.tick_interval_sec);
    println!(
        "  {:<28} {}",
        "Auto-deploy:".cyan().bold(),
        if cfg.auto_deploy { "yes".green() } else { "no".red() }
    );
    println!(
        "  {:<28} {}",
        "Auto-topup:".cyan().bold(),
        if cfg.auto_topup { "yes".green() } else { "no".red() }
    );
    println!(
        "  {:<28} {} TON",
        "Wallet deploy:".cyan().bold(),
        nanotons_to_ton_display(cfg.wallet.deploy)
    );
    println!(
        "  {:<28} SNP {} TON, TONCore {} TON",
        "Pool deploy:".cyan().bold(),
        nanotons_to_ton_display(cfg.pool.snp),
        nanotons_to_ton_display(cfg.pool.ton_core),
    );
    println!(
        "  {:<28} {} TON",
        "Wallet top-up:".cyan().bold(),
        nanotons_to_ton_display(cfg.wallet.topup)
    );
    println!(
        "  {:<28} {} TON",
        "Wallet balance threshold:".cyan().bold(),
        nanotons_to_ton_display(cfg.wallet.threshold)
    );
    println!();
}

#[derive(Default, serde::Serialize)]
struct WalletPatchBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    deploy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topup: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    threshold: Option<u64>,
}

#[derive(Default, serde::Serialize)]
struct PoolPatchBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    snp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ton_core: Option<u64>,
}

#[derive(Default, serde::Serialize)]
struct ContractsAutomationPatchBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    tick_interval_sec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_deploy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_topup: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wallet: Option<WalletPatchBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<PoolPatchBody>,
}
