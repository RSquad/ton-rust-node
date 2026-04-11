/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    config_bind_cmd::BindCmd, config_elections_cmd::ElectionsCfgCmd, config_log_cmd::LogCmd,
    config_node_cmd::NodeCmd, config_pool_cmd::PoolCmd, config_ton_http_api_cmd::TonHttpApiCmd,
    config_wallet_cmd::WalletCmd, master_wallet_cmd::MasterWalletCmd,
    utils::{require_config, save_config},
};
use anyhow::Context;
use common::{
    TonWalletVersion,
    app_config::{
        AppConfig, ElectionsConfig, HttpConfig, KeyConfig, LogConfig, StakePolicy,
        TonHttpApiConfig, WalletConfig,
    },
    task_cancellation::CancellationCtx,
    ton_utils::tons_f64_to_nanotons,
};
use std::{collections::HashMap, path::Path};

/// Configuration management commands
#[derive(clap::Args, Clone)]
#[command(about = "Manage nodectl configuration")]
pub struct ConfigCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        env = "CONFIG_PATH",
        global = true
    )]
    config: Option<String>,

    #[arg(
        short = 'u',
        long = "url",
        value_hint = clap::ValueHint::Url,
        help = "URL to the node control service API (takes precedence over --config for reads)",
        global = true
    )]
    url: Option<String>,

    #[arg(
        long = "token",
        env = "NODECTL_API_TOKEN",
        value_name = "TOKEN",
        help = "JWT token to authenticate with the service API (or NODECTL_API_TOKEN)",
        global = true
    )]
    token: Option<String>,

    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum ConfigAction {
    /// Generate a new default configuration file
    Generate(GenerateCmd),
    /// Manage nodes in the configuration
    Node(NodeCmd),
    /// Manage wallets in the configuration
    Wallet(WalletCmd),
    /// Manage pools in the configuration
    Pool(PoolCmd),
    /// Manage node-to-wallet/pool bindings
    Bind(BindCmd),
    /// Manage ton-http-api configuration
    TonHttpApi(TonHttpApiCmd),
    /// Master wallet info
    MasterWallet(MasterWalletCmd),
    /// Manage elections configuration
    Elections(ElectionsCfgCmd),
    /// Manage log configuration
    Log(LogCmd),
    /// Set the stake policy (shortcut for `elections stake-policy`)
    StakePolicy(StakePolicyCmd),
}

#[derive(clap::Args, Clone)]
pub struct GenerateCmd {
    /// Output path for the configuration file
    #[arg(short = 'o', long = "output", default_value = "nodectl-config.json")]
    output: String,

    /// Overwrite existing file
    #[arg(short = 'f', long = "force", default_value = "false")]
    force: bool,
}

#[derive(clap::Args, Clone)]
pub struct StakePolicyCmd {
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
        help = "Apply policy only to this node (override). Omit to set the default policy for all nodes."
    )]
    node: Option<String>,
    #[arg(long = "reset", help = "Remove a per-node policy override (requires --node)")]
    reset: bool,
}

impl ConfigCmd {
    pub async fn run(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let url = self.url.as_deref();
        let token = self.token.as_deref();
        let config_path = self.config.as_deref();

        match &self.action {
            ConfigAction::Generate(cmd) => cmd.run().await,
            ConfigAction::Node(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::Wallet(cmd) => {
                cmd.run(config_path, cancellation_ctx, url, token).await
            }
            ConfigAction::Pool(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::Bind(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::TonHttpApi(cmd) => cmd.run(require_config(config_path)?).await,
            ConfigAction::MasterWallet(cmd) => cmd.run(url, token, config_path).await,
            ConfigAction::Elections(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::Log(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::StakePolicy(cmd) => cmd.run(require_config(config_path)?).await,
        }
    }
}

impl GenerateCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let path = Path::new(&self.output);

        if path.exists() && !self.force {
            anyhow::bail!("File '{}' already exists. Use --force to overwrite.", self.output);
        }

        // Create default config
        let config = AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: TonHttpApiConfig::default(),
            elections: Some(ElectionsConfig::default()),
            voting: None,
            http: HttpConfig::default(),
            master_wallet: Some(WalletConfig {
                key: KeyConfig::VaultKey { name: "master-wallet-secret".to_string() },
                version: TonWalletVersion::V3R2,
                subwallet_id: 42,
                workchain: 0,
            }),
            tick_interval: 40,
            log: Some(LogConfig::default()),
        };

        save_config(&config, path)?;

        Ok(())
    }
}

impl StakePolicyCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;

        // Handle clearing a per-node override
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
            save_config(&config, path).context("failed to write config file")?;
            let result = StakePolicyClearResult {
                ok: true,
                node: node_id.clone(),
                policy: config.elections.as_ref().map(|e| e.policy.clone()).unwrap_or_default(),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
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

        // Update elections config
        if let Some(elections) = &mut config.elections {
            if let Some(node_id) = &self.node {
                elections.policy_overrides.insert(node_id.clone(), policy.clone());
            } else {
                // Default policy for all nodes
                elections.policy = policy.clone();
            }
        } else {
            if self.node.is_some() {
                anyhow::bail!(
                    "Elections are not configured. Set a default policy first before adding per-node overrides."
                );
            }
            config.elections =
                Some(ElectionsConfig { policy: policy.clone(), ..Default::default() });
        }

        save_config(&config, path)?;

        let result = StakePolicyResult {
            ok: true,
            config: path.display().to_string(),
            node: self.node.clone(),
            policy,
        };

        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct StakePolicyResult {
    ok: bool,
    config: String,
    /// If set, the policy was applied as a per-node override.
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    policy: StakePolicy,
}

#[derive(serde::Serialize)]
struct StakePolicyClearResult {
    ok: bool,
    node: String,
    policy: StakePolicy,
}
