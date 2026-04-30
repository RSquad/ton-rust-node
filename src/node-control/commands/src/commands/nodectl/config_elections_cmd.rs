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
    app_config::{BindingStatus, ElectionsConfig, StakePolicy},
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
    /// Set AdaptiveSplit50 minimum wait fraction (`sleep_period_pct` in config, 0.0–1.0, must be ≤ waiting period)
    AdaptiveSleepPeriodPct(AdaptivePeriodPctCmd),
    /// Set AdaptiveSplit50 maximum wait fraction (`waiting_period_pct` in config, 0.0–1.0, must be ≥ sleep period)
    AdaptiveWaitingPeriodPct(AdaptivePeriodPctCmd),
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
pub struct AdaptivePeriodPctCmd {
    #[arg(help = "Fraction of election duration in [0.0, 1.0]")]
    value: f64,
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
            ElectionsAction::AdaptiveSleepPeriodPct(cmd) => {
                cmd.run_adaptive_sleep(url, token, config_path).await
            }
            ElectionsAction::AdaptiveWaitingPeriodPct(cmd) => {
                cmd.run_adaptive_waiting(url, token, config_path).await
            }
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
    #[serde(default = "default_sleep_period_pct")]
    sleep_period_pct: f64,
    #[serde(default = "default_waiting_period_pct")]
    waiting_period_pct: f64,
    bindings: Vec<BindingElectionView>,
}

fn default_sleep_period_pct() -> f64 {
    0.2
}

fn default_waiting_period_pct() -> f64 {
    0.4
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
    println!("  {:<20} {}", "Stake Policy:".cyan().bold(), view.stake_policy);
    println!("  {:<20} {}", "Max Factor:".cyan().bold(), view.max_factor);
    println!("  {:<20} {}s", "Tick Interval:".cyan().bold(), view.tick_interval);
    println!(
        "  {:<28} {} (config: sleep_period_pct)",
        "adaptive_sleep_period_pct:".cyan().bold(),
        view.sleep_period_pct
    );
    println!(
        "  {:<28} {} (config: waiting_period_pct)",
        "adaptive_waiting_period_pct:".cyan().bold(),
        view.waiting_period_pct
    );

    if !view.policy_overrides.is_empty() {
        println!("\n  {}", "Policy Overrides:".cyan().bold());
        for (node, policy) in &view.policy_overrides {
            println!("    {:<18} {}", node, policy);
        }
    }

    if !view.bindings.is_empty() {
        let has_static_adnl = view.bindings.iter().any(|b| b.static_adnl.is_some());
        println!("\n  {}", "Bindings:".cyan().bold());
        if has_static_adnl {
            println!(
                "    {:<20} {:<12} {:<16} {:<20} {}",
                "Node".cyan(),
                "Enable".cyan(),
                "Status".cyan(),
                "Stake Policy".cyan(),
                "Static ADNL".cyan(),
            );
        } else {
            println!(
                "    {:<20} {:<12} {:<16} {}",
                "Node".cyan(),
                "Enable".cyan(),
                "Status".cyan(),
                "Stake Policy".cyan(),
            );
        }
        println!("    {}", "─".repeat(if has_static_adnl { 100 } else { 70 }).dimmed());
        for b in &view.bindings {
            let enable_str =
                if b.enable { "yes".green().to_string() } else { "no".red().to_string() };
            if has_static_adnl {
                let adnl = b.static_adnl.as_deref().unwrap_or("—");
                println!(
                    "    {:<20} {:<21} {:<16} {:<20} {}",
                    b.name, enable_str, b.status, b.stake_policy, adnl,
                );
            } else {
                println!(
                    "    {:<20} {:<21} {:<16} {}",
                    b.name, enable_str, b.status, b.stake_policy,
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

/// Current adaptive timing from `GET /v1/elections/settings` (same merge semantics as the service).
async fn fetch_adaptive_timing_percentages(
    base_url: &str,
    token: Option<&str>,
) -> anyhow::Result<(f64, f64)> {
    let body = api_get(base_url, "/v1/elections/settings", token).await?;
    let resp: serde_json::Value = serde_json::from_str(&body)?;
    let result = resp
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("elections settings response: missing 'result'"))?;
    let sleep = result
        .get("sleep_period_pct")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("elections settings: missing sleep_period_pct"))?;
    let waiting = result
        .get("waiting_period_pct")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("elections settings: missing waiting_period_pct"))?;
    Ok((sleep, waiting))
}

fn validate_sleep_waiting_pair(sleep: f64, waiting: f64) -> anyhow::Result<()> {
    let mut ec = ElectionsConfig::default();
    ec.sleep_period_pct = sleep;
    ec.waiting_period_pct = waiting;
    ec.validate_timing_fields()
}

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

impl AdaptivePeriodPctCmd {
    pub async fn run_adaptive_sleep(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        Self::validate_range(self.value)?;
        let base_url = resolve_service_url(url, config_path)?;
        let (_cur_sleep, cur_waiting) = fetch_adaptive_timing_percentages(&base_url, token).await?;
        validate_sleep_waiting_pair(self.value, cur_waiting)?;
        api_post(
            &base_url,
            ELECTIONS_SETTINGS_PATH,
            token,
            &ElectionsSettingsBody { sleep_period_pct: Some(self.value), ..Default::default() },
        )
        .await?;
        println!(
            "{} adaptive_sleep_period_pct (sleep_period_pct) set to {}",
            "OK".green().bold(),
            self.value
        );
        Ok(())
    }

    pub async fn run_adaptive_waiting(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        Self::validate_range(self.value)?;
        let base_url = resolve_service_url(url, config_path)?;
        let (cur_sleep, _cur_waiting) = fetch_adaptive_timing_percentages(&base_url, token).await?;
        validate_sleep_waiting_pair(cur_sleep, self.value)?;
        api_post(
            &base_url,
            ELECTIONS_SETTINGS_PATH,
            token,
            &ElectionsSettingsBody { waiting_period_pct: Some(self.value), ..Default::default() },
        )
        .await?;
        println!(
            "{} adaptive_waiting_period_pct (waiting_period_pct) set to {}",
            "OK".green().bold(),
            self.value
        );
        Ok(())
    }

    fn validate_range(v: f64) -> anyhow::Result<()> {
        if !(0.0..=1.0).contains(&v) {
            anyhow::bail!("value must be in range [0.0, 1.0]");
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
