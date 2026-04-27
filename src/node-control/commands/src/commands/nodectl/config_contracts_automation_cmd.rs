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

const SETTINGS_PATH: &str = "/v1/contracts-automation/settings";

#[derive(clap::Args, Clone)]
#[command(about = "Manage contracts automation (auto-deploy / auto-topup) via the service API")]
pub struct ContractsAutomationCfgCmd {
    #[command(subcommand)]
    action: ContractsAutomationAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum ContractsAutomationAction {
    /// Show current contracts automation settings
    Ls(LsCmd),
    /// Update settings (requires operator JWT when auth is enabled)
    Set(SetCmd),
}

#[derive(clap::Args, Clone)]
pub struct LsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::Args, Clone)]
pub struct SetCmd {
    #[arg(long, help = "Contracts monitor tick interval (seconds)")]
    tick_interval_sec: Option<u64>,

    #[arg(long, help = "Amount in TON sent when deploying a validator wallet from the master")]
    wallet_deploy: Option<f64>,

    #[arg(long, help = "Amount in TON sent when deploying a Single Nominator pool")]
    pool_deploy_snp: Option<f64>,

    #[arg(long, help = "Amount in TON sent when deploying a TONCore pool contract")]
    pool_deploy_ton_core: Option<f64>,

    #[arg(long, help = "Amount in TON to send when topping up a wallet below the threshold")]
    wallet_topup: Option<f64>,

    #[arg(long, help = "Minimum wallet balance in TON; below this, auto-topup sends wallet-topup")]
    wallet_threshold: Option<f64>,

    #[arg(long = "enable-auto-deploy", conflicts_with = "disable_auto_deploy")]
    enable_auto_deploy: bool,

    #[arg(long = "disable-auto-deploy", conflicts_with = "enable_auto_deploy")]
    disable_auto_deploy: bool,

    #[arg(long = "enable-auto-topup", conflicts_with = "disable_auto_topup")]
    enable_auto_topup: bool,

    #[arg(long = "disable-auto-topup", conflicts_with = "enable_auto_topup")]
    disable_auto_topup: bool,
}

impl ContractsAutomationCfgCmd {
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            ContractsAutomationAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            ContractsAutomationAction::Set(cmd) => cmd.run(url, token, config_path).await,
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
                    .map_err(|e| {
                        anyhow::anyhow!("failed to parse contracts automation settings: {e}")
                    })?;
                print_table(&cfg);
            }
        }
        Ok(())
    }
}

fn nanotons_to_ton_display(n: u64) -> String {
    format!("{:.4}", n as f64 / 1e9)
}

fn print_table(cfg: &ContractsAutomationConfig) {
    println!("\n{} {}\n", "OK".green().bold(), "Contracts automation".green());
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
        nanotons_to_ton_display(cfg.wallet_deploy_nanotons)
    );
    println!(
        "  {:<28} SNP {} TON, TONCore {} TON",
        "Pool deploy:".cyan().bold(),
        nanotons_to_ton_display(cfg.pool_deploy_nanotons.single_nominator),
        nanotons_to_ton_display(cfg.pool_deploy_nanotons.ton_core),
    );
    println!(
        "  {:<28} {} TON",
        "Wallet top-up:".cyan().bold(),
        nanotons_to_ton_display(cfg.wallet_topup_nanotons)
    );
    println!(
        "  {:<28} {} TON",
        "Wallet balance threshold:".cyan().bold(),
        nanotons_to_ton_display(cfg.wallet_balance_threshold_nanotons)
    );
    println!();
}

#[derive(Default, serde::Serialize)]
struct PoolDeployPatchBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    single_nominator: Option<u64>,
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
    wallet_deploy_nanotons: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_deploy_nanotons: Option<PoolDeployPatchBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wallet_topup_nanotons: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wallet_balance_threshold_nanotons: Option<u64>,
}

impl SetCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = ContractsAutomationPatchBody::default();

        if let Some(v) = self.tick_interval_sec {
            body.tick_interval_sec = Some(v);
        }
        if self.enable_auto_deploy {
            body.auto_deploy = Some(true);
        } else if self.disable_auto_deploy {
            body.auto_deploy = Some(false);
        }
        if self.enable_auto_topup {
            body.auto_topup = Some(true);
        } else if self.disable_auto_topup {
            body.auto_topup = Some(false);
        }
        if let Some(tons) = self.wallet_deploy {
            body.wallet_deploy_nanotons = Some(tons_f64_to_nanotons(tons));
        }
        if let Some(tons) = self.wallet_topup {
            body.wallet_topup_nanotons = Some(tons_f64_to_nanotons(tons));
        }
        if let Some(tons) = self.wallet_threshold {
            body.wallet_balance_threshold_nanotons = Some(tons_f64_to_nanotons(tons));
        }

        if self.pool_deploy_snp.is_some() || self.pool_deploy_ton_core.is_some() {
            body.pool_deploy_nanotons = Some(PoolDeployPatchBody {
                single_nominator: self.pool_deploy_snp.map(tons_f64_to_nanotons),
                ton_core: self.pool_deploy_ton_core.map(tons_f64_to_nanotons),
            });
        }

        let any = body.tick_interval_sec.is_some()
            || body.auto_deploy.is_some()
            || body.auto_topup.is_some()
            || body.wallet_deploy_nanotons.is_some()
            || body.wallet_topup_nanotons.is_some()
            || body.wallet_balance_threshold_nanotons.is_some()
            || body.pool_deploy_nanotons.is_some();

        if !any {
            anyhow::bail!(
                "No settings to update. Pass at least one of: --tick-interval-sec, \
                 --wallet-deploy, --pool-deploy-snp, --pool-deploy-ton-core, --wallet-topup, \
                 --wallet-threshold, --enable-auto-deploy / --disable-auto-deploy, \
                 --enable-auto-topup / --disable-auto-topup"
            );
        }

        let base_url = resolve_service_url(url, config_path)?;
        api_post(&base_url, SETTINGS_PATH, token, &body).await?;
        println!("{} Contracts automation settings updated", "OK".green().bold());
        Ok(())
    }
}
