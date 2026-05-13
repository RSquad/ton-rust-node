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
use common::{
    app_config::{BindingStatus, StakePolicy},
    ton_utils::tons_f64_to_nanotons,
};
use std::collections::HashMap;

#[derive(clap::Args, Clone)]
#[command(about = "Manage elections configuration")]
pub struct ElectionsCfgCmd {
    #[command(subcommand)]
    action: ElectionsAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum ElectionsAction {
    /// View the current elections configuration
    Show(ShowCmd),
    /// Set the default or per-node stake policy
    StakePolicy(StakePolicySetCmd),
    /// Set the elections tick interval (seconds)
    TickInterval(TickIntervalCmd),
    /// Set the max-factor
    MaxFactor(MaxFactorCmd),
    /// Set the AdaptiveSplit50 staking window — when to start staking and how long to wait for peers.
    ///
    /// Only used when the stake policy is `adaptive_split50`; ignored for `minimum`, `split50`,
    /// and `fixed`.
    #[command(name = "wait-pct")]
    Wait(WaitCmd),
    /// Enable elections for binding(s)
    Enable(EnableCmd),
    /// Disable elections for binding(s)
    Disable(DisableCmd),
    /// Generate and assign a persistent ADNL address for a node
    StaticAdnl(StaticAdnlCmd),
}

#[derive(clap::Args, Clone)]
pub struct ShowCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::Args, Clone)]
pub struct StakePolicySetCmd {
    #[arg(long = "fixed", conflicts_with_all = ["split50", "minimum", "adaptive_split50"], help = "Fixed stake amount in TON")]
    fixed: Option<f64>,
    #[arg(long = "split50", conflicts_with_all = ["fixed", "minimum", "adaptive_split50"], help = "Use 50% of available balance")]
    split50: bool,
    #[arg(long = "minimum", conflicts_with_all = ["fixed", "split50", "adaptive_split50"], help = "Use minimum required stake")]
    minimum: bool,
    #[arg(long = "adaptive-split50", conflicts_with_all = ["fixed", "split50", "minimum"], help = "Adaptive split: splits when half exceeds effective minimum, otherwise stakes all")]
    adaptive_split50: bool,
    #[arg(
        short = 'n',
        long = "node",
        help = "Apply policy only to this node (override). Omit to set the default policy."
    )]
    node: Option<String>,
    #[arg(long = "reset", help = "Remove a per-node policy override (requires --node)")]
    reset: bool,
}

#[derive(clap::Args, Clone)]
pub struct TickIntervalCmd {
    #[arg(help = "Tick interval in seconds")]
    seconds: u64,
}

#[derive(clap::Args, Clone)]
pub struct MaxFactorCmd {
    #[arg(
        help = "Max factor: from 1.0 up to the network limit (config param 17 max_stake_factor)"
    )]
    value: f32,
}

#[derive(clap::Args, Clone)]
pub struct WaitCmd {
    /// Earliest stake submission, as fraction of election duration
    #[arg(
        long,
        help = "Earliest stake submission, as fraction of election duration",
        long_help = "Defer staking until this fraction of the election window has elapsed, \
                     even when there are already enough peers. Range [0.0, 1.0]; default 0.2. \
                     Only applied under the adaptive_split50 stake policy."
    )]
    min: Option<f64>,
    /// Latest peer-wait deadline, as fraction of election duration
    #[arg(
        long,
        help = "Latest peer-wait deadline, as fraction of election duration",
        long_help = "Keep waiting for enough peers until this fraction of the election window. \
                     After this point, stake regardless of peer count. Range [0.0, 1.0]; \
                     must be \u{2265} --min; default 0.4. Only applied under the adaptive_split50 \
                     stake policy."
    )]
    max: Option<f64>,
}

#[derive(clap::Args, Clone)]
pub struct EnableCmd {
    #[arg(required = true, help = "Binding name(s) to enable for elections")]
    nodes: Vec<String>,
}

#[derive(clap::Args, Clone)]
pub struct DisableCmd {
    #[arg(required = true, help = "Binding name(s) to disable from elections")]
    nodes: Vec<String>,
}

#[derive(clap::Args, Clone)]
pub struct StaticAdnlCmd {
    #[arg(short = 'n', long = "node", required = true, help = "Node name")]
    node: String,
}

impl ElectionsCfgCmd {
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            ElectionsAction::Show(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::StakePolicy(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::TickInterval(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::MaxFactor(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::Wait(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::Enable(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::Disable(cmd) => cmd.run(url, token, config_path).await,
            ElectionsAction::StaticAdnl(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

impl ShowCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/elections/settings", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let result = &resp["result"];

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
            OutputFormat::Table => {
                let dto: ElectionsSettingsView = serde_json::from_value(result.clone())
                    .map_err(|e| anyhow::anyhow!("failed to parse elections settings: {}", e))?;
                print_elections_settings_table(&dto);
            }
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct ElectionsSettingsView {
    stake_policy: StakePolicy,
    #[serde(default)]
    policy_overrides: HashMap<String, StakePolicy>,
    max_factor: f32,
    tick_interval: u64,
    sleep_period_pct: f64,
    waiting_period_pct: f64,
    #[serde(default)]
    bindings: Vec<BindingElectionView>,
}

#[derive(serde::Deserialize)]
struct BindingElectionView {
    name: String,
    enable: bool,
    status: BindingStatus,
    stake_policy: StakePolicy,
    #[serde(default)]
    static_adnl: Option<String>,
}

fn print_elections_settings_table(view: &ElectionsSettingsView) {
    println!("\n{} {}\n", "OK".green().bold(), "Elections Configuration".green());
    println!("  {:<28} {}", "Stake Policy:".cyan().bold(), view.stake_policy);
    println!("  {:<28} {}", "Max Factor:".cyan().bold(), view.max_factor);
    println!("  {:<28} {}s", "Tick Interval:".cyan().bold(), view.tick_interval);
    println!("  {:<28} {}", "Adaptive sleep fraction:".cyan().bold(), view.sleep_period_pct);
    println!("  {:<28} {}", "Adaptive wait fraction:".cyan().bold(), view.waiting_period_pct);

    if !view.policy_overrides.is_empty() {
        println!("\n  {}", "Policy Overrides:".cyan().bold());
        for (node, policy) in &view.policy_overrides {
            println!("    {:<18} {}", node, policy);
        }
    }

    if !view.bindings.is_empty() {
        let has_static_adnl = view.bindings.iter().any(|b| b.static_adnl.is_some());
        // Column widths: stake policy strings can be long (e.g. adaptive_split50 (50% or 100%)).
        // Headers/data must pad plain text before coloring — ANSI must not count toward {:<N}.
        const W_NODE: usize = 20;
        const W_ENABLE: usize = 12;
        const W_STATUS: usize = 18;
        const W_STAKE: usize = 38;
        const W_ADNL: usize = 24;

        println!("\n  {}", "Bindings:".cyan().bold());
        let rule_len =
            W_NODE + W_ENABLE + W_STATUS + W_STAKE + if has_static_adnl { W_ADNL } else { 0 };
        if has_static_adnl {
            println!(
                "    {}{}{}{}{}",
                format!("{:<w$}", "Node", w = W_NODE).cyan(),
                format!("{:<w$}", "Enable", w = W_ENABLE).cyan(),
                format!("{:<w$}", "Status", w = W_STATUS).cyan(),
                format!("{:<w$}", "Stake Policy", w = W_STAKE).cyan(),
                format!("{:<w$}", "Static ADNL", w = W_ADNL).cyan(),
            );
        } else {
            println!(
                "    {}{}{}{}",
                format!("{:<w$}", "Node", w = W_NODE).cyan(),
                format!("{:<w$}", "Enable", w = W_ENABLE).cyan(),
                format!("{:<w$}", "Status", w = W_STATUS).cyan(),
                format!("{:<w$}", "Stake Policy", w = W_STAKE).cyan(),
            );
        }
        println!("    {}", "─".repeat(rule_len).dimmed());
        for b in &view.bindings {
            let enable_cell = if b.enable {
                format!("{:<w$}", "yes", w = W_ENABLE).green()
            } else {
                format!("{:<w$}", "no", w = W_ENABLE).red()
            };

            let status_cell = format!("{:<w_st$}", b.status.to_string(), w_st = W_STATUS);
            let stake_cell = format!("{:<w_sk$}", b.stake_policy.to_string(), w_sk = W_STAKE);
            if has_static_adnl {
                let adnl = b.static_adnl.as_deref().unwrap_or("—");
                let adnl_cell = format!("{:<w_ad$}", adnl, w_ad = W_ADNL);
                println!(
                    "    {:<w$}{}{}{}{}",
                    b.name,
                    enable_cell,
                    status_cell,
                    stake_cell,
                    adnl_cell,
                    w = W_NODE,
                );
            } else {
                println!(
                    "    {:<w$}{}{}{}",
                    b.name,
                    enable_cell,
                    status_cell,
                    stake_cell,
                    w = W_NODE,
                );
            }
        }
    }
    println!();
}

/// Shared body for `POST /v1/elections/settings` — all fields optional.
#[derive(Default, serde::Serialize)]
struct ElectionsSettingsBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<StakePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reset: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tick_interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_factor: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sleep_period_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_period_pct: Option<f64>,
}

const ELECTIONS_SETTINGS_PATH: &str = "/v1/elections/settings";

impl StakePolicySetCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;

        if self.reset {
            let body = ElectionsSettingsBody {
                node: self.node.clone(),
                reset: true,
                ..Default::default()
            };
            api_post(&base_url, ELECTIONS_SETTINGS_PATH, token, &body).await?;
            println!(
                "{} Per-node override for '{}' removed",
                "OK".green().bold(),
                self.node.as_deref().unwrap_or("?"),
            );
            return Ok(());
        }

        let policy = if let Some(tons) = self.fixed {
            StakePolicy::Fixed(tons_f64_to_nanotons(tons))
        } else if self.split50 {
            StakePolicy::Split50
        } else if self.minimum {
            StakePolicy::Minimum
        } else if self.adaptive_split50 {
            StakePolicy::AdaptiveSplit50
        } else {
            anyhow::bail!(
                "No policy specified. Use --fixed, --split50, --minimum, or --adaptive-split50"
            );
        };

        let body = ElectionsSettingsBody {
            policy: Some(policy.clone()),
            node: self.node.clone(),
            ..Default::default()
        };
        api_post(&base_url, ELECTIONS_SETTINGS_PATH, token, &body).await?;

        let scope = match &self.node {
            Some(n) => format!("node '{}'", n),
            None => "default".to_string(),
        };
        println!("{} Stake policy for {} set to: {}", "OK".green().bold(), scope, policy);
        Ok(())
    }
}

impl TickIntervalCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_post(
            &base_url,
            ELECTIONS_SETTINGS_PATH,
            token,
            &ElectionsSettingsBody { tick_interval: Some(self.seconds), ..Default::default() },
        )
        .await?;
        println!("{} Tick interval set to {} seconds", "OK".green().bold(), self.seconds);
        Ok(())
    }
}

impl MaxFactorCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let resp = api_post(
            &base_url,
            ELECTIONS_SETTINGS_PATH,
            token,
            &ElectionsSettingsBody { max_factor: Some(self.value), ..Default::default() },
        )
        .await?;
        let parsed: serde_json::Value = serde_json::from_str(&resp)?;
        let max_factor = parsed["result"]["max_factor"].as_f64();
        match max_factor {
            Some(v) => println!("{} Max factor set to {v}", "OK".green().bold()),
            None => println!("{} Max factor set to {}", "OK".green().bold(), self.value),
        }
        Ok(())
    }
}

impl WaitCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.min.is_none() && self.max.is_none() {
            anyhow::bail!("specify at least one of --min or --max");
        }
        let base_url = resolve_service_url(url, config_path)?;
        api_post(
            &base_url,
            ELECTIONS_SETTINGS_PATH,
            token,
            &ElectionsSettingsBody {
                sleep_period_pct: self.min,
                waiting_period_pct: self.max,
                ..Default::default()
            },
        )
        .await?;
        match (self.min, self.max) {
            (Some(mn), Some(mx)) => println!(
                "{} sleep_period_pct (--min)={}, waiting_period_pct (--max)={}",
                "OK".green().bold(),
                mn,
                mx
            ),
            (Some(mn), None) => {
                println!("{} sleep_period_pct (--min) set to {}", "OK".green().bold(), mn)
            }
            (None, Some(mx)) => {
                println!("{} waiting_period_pct (--max) set to {}", "OK".green().bold(), mx)
            }
            (None, None) => unreachable!("validated above"),
        }
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct NodeListBody<'a> {
    nodes: &'a [String],
}

impl EnableCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_post(&base_url, "/v1/elections/include", token, &NodeListBody { nodes: &self.nodes })
            .await?;
        println!("{} Elections enabled for: {}", "OK".green().bold(), self.nodes.join(", "));
        Ok(())
    }
}

impl DisableCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        api_post(&base_url, "/v1/elections/exclude", token, &NodeListBody { nodes: &self.nodes })
            .await?;
        println!("{} Elections disabled for: {}", "OK".green().bold(), self.nodes.join(", "));
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct StaticAdnlBody<'a> {
    node: &'a str,
}

impl StaticAdnlCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let resp = api_post(
            &base_url,
            "/v1/elections/static-adnl",
            token,
            &StaticAdnlBody { node: &self.node },
        )
        .await?;
        let parsed: serde_json::Value = serde_json::from_str(&resp)?;
        let adnl_addr = parsed["result"]["adnl_addr"].as_str().unwrap_or("unknown");
        println!("{} Static ADNL address for '{}': {}", "OK".green().bold(), self.node, adnl_addr);
        Ok(())
    }
}
