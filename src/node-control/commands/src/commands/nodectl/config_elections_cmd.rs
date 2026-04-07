/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::{output_format::OutputFormat, utils::save_config};
use colored::Colorize;
use common::{
    app_config::{AppConfig, BindingStatus, ElectionsConfig, StakePolicy},
    ton_utils::tons_f64_to_nanotons,
};
use std::{collections::HashMap, path::Path};

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
    /// Enable elections for binding(s)
    Enable(EnableCmd),
    /// Disable elections for binding(s)
    Disable(DisableCmd),
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
    #[arg(help = "Max factor (1.0..3.0)")]
    value: f32,
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

impl ElectionsCfgCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            ElectionsAction::Show(cmd) => cmd.run(path).await,
            ElectionsAction::StakePolicy(cmd) => cmd.run(path).await,
            ElectionsAction::TickInterval(cmd) => cmd.run(path).await,
            ElectionsAction::MaxFactor(cmd) => cmd.run(path).await,
            ElectionsAction::Enable(cmd) => cmd.run(path).await,
            ElectionsAction::Disable(cmd) => cmd.run(path).await,
        }
    }
}

#[derive(serde::Serialize)]
struct ElectionsView {
    stake_policy: StakePolicy,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    policy_overrides: HashMap<String, StakePolicy>,
    max_factor: f32,
    tick_interval: u64,
    bindings: Vec<BindingElectionStatus>,
}

#[derive(serde::Serialize)]
struct BindingElectionStatus {
    name: String,
    enable: bool,
    status: BindingStatus,
    stake_policy: StakePolicy,
}

impl ShowCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let config = AppConfig::load(path)?;
        let elections = config
            .elections
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Elections are not configured"))?;

        let bindings = build_binding_status(&config, elections);

        match self.format {
            OutputFormat::Json => {
                let view = build_view(elections, bindings);
                println!("{}", serde_json::to_string_pretty(&view)?);
            }
            OutputFormat::Table => {
                print_elections_table(elections, &bindings);
            }
        }
        Ok(())
    }
}

impl StakePolicySetCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        if self.reset {
            let node_id = self
                .node
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("--reset requires --node <NODE>"))?;
            config
                .elections
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Elections are not configured"))?
                .policy_overrides
                .remove(node_id);
            save_config(&config, path)?;
            println!(
                "{} Per-node override for '{}' removed. Default policy: {}",
                "OK".green().bold(),
                node_id,
                config.elections.as_ref().map(|e| e.policy.to_string()).unwrap_or_default()
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

        if let Some(elections) = &mut config.elections {
            if let Some(node_id) = &self.node {
                elections.policy_overrides.insert(node_id.clone(), policy.clone());
            } else {
                elections.policy = policy.clone();
            }
        } else if self.node.is_some() {
            anyhow::bail!("Elections are not configured. Set a default policy first.");
        } else {
            config.elections =
                Some(ElectionsConfig { policy: policy.clone(), ..Default::default() });
        }

        save_config(&config, path)?;

        let scope = match &self.node {
            Some(n) => format!("node '{}'", n),
            None => "default".to_string(),
        };
        println!("{} Stake policy for {} set to: {}", "OK".green().bold(), scope, policy);
        Ok(())
    }
}

impl TickIntervalCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;
        config
            .elections
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Elections are not configured"))?
            .tick_interval = self.seconds;
        save_config(&config, path)?;
        println!("{} Tick interval set to {} seconds", "OK".green().bold(), self.seconds);
        Ok(())
    }
}

impl MaxFactorCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if !(1.0..=3.0).contains(&self.value) {
            anyhow::bail!("max-factor must be in range [1.0..3.0]");
        }
        let mut config = AppConfig::load(path)?;
        config
            .elections
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Elections are not configured"))?
            .max_factor = self.value;
        save_config(&config, path)?;
        println!("{} Max factor set to {}", "OK".green().bold(), self.value);
        Ok(())
    }
}

impl EnableCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        for node_id in &self.nodes {
            let binding = config
                .bindings
                .get_mut(node_id)
                .ok_or_else(|| anyhow::anyhow!("Binding for node '{}' not found", node_id))?;
            binding.enable = true;
        }
        save_config(&config, path)?;
        println!("{} Elections enabled for: {}", "OK".green().bold(), self.nodes.join(", "));
        Ok(())
    }
}

impl DisableCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        for node_id in &self.nodes {
            let binding = config
                .bindings
                .get_mut(node_id)
                .ok_or_else(|| anyhow::anyhow!("Binding for node '{}' not found", node_id))?;
            binding.enable = false;
        }
        save_config(&config, path)?;
        println!("{} Elections disabled for: {}", "OK".green().bold(), self.nodes.join(", "));
        Ok(())
    }
}

fn build_binding_status(
    config: &AppConfig,
    elections: &ElectionsConfig,
) -> Vec<BindingElectionStatus> {
    let mut names: Vec<String> = config.bindings.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let binding = config.bindings.get(&name).expect("binding exists");
            let stake_policy = elections.stake_policy(&name).clone();
            BindingElectionStatus {
                enable: binding.enable,
                status: binding.status,
                name,
                stake_policy,
            }
        })
        .collect()
}

fn build_view(elections: &ElectionsConfig, bindings: Vec<BindingElectionStatus>) -> ElectionsView {
    ElectionsView {
        stake_policy: elections.policy.clone(),
        policy_overrides: elections.policy_overrides.clone(),
        max_factor: elections.max_factor,
        tick_interval: elections.tick_interval,
        bindings,
    }
}

fn print_elections_table(elections: &ElectionsConfig, bindings: &[BindingElectionStatus]) {
    println!("\n  {}", "Elections Configuration".cyan().bold());
    println!("  {}", "─".repeat(60).dimmed());
    println!("  {:<24} {}", "Stake Policy:".bold(), elections.policy);
    println!("  {:<24} {}", "Max Factor:".bold(), elections.max_factor);
    println!("  {:<24} {}s", "Tick Interval:".bold(), elections.tick_interval);

    if !elections.policy_overrides.is_empty() {
        println!();
        println!("  {}", "Policy Overrides".cyan().bold());
        println!("  {}", "─".repeat(60).dimmed());
        println!("  {:<24} {}", "Node".cyan().bold(), "Policy".cyan().bold(),);
        let mut overrides: Vec<_> = elections.policy_overrides.iter().collect();
        overrides.sort_by_key(|(k, _)| (*k).clone());
        for (node, policy) in overrides {
            println!("  {:<24} {}", node, policy);
        }
    }

    if !bindings.is_empty() {
        println!();
        println!("  {}", "Bindings".cyan().bold());
        println!("  {}", "─".repeat(60).dimmed());
        println!(
            "  {:<24} {:<12} {:<16} {}",
            "Node".cyan().bold(),
            "Enable".cyan().bold(),
            "Status".cyan().bold(),
            "Stake Policy".cyan().bold(),
        );
        for b in bindings {
            let enable_str =
                if b.enable { "yes".green().to_string() } else { "no".red().to_string() };
            println!("  {:<24} {:<21} {:<16} {}", b.name, enable_str, b.status, b.stake_policy);
        }
    }
    println!();
}
