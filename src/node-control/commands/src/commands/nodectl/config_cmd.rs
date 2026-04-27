/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    config_bind_cmd::BindCmd, config_contracts_automation_cmd::ContractsAutomationCfgCmd,
    config_elections_cmd::ElectionsCfgCmd, config_log_cmd::LogCmd, config_node_cmd::NodeCmd,
    config_pool_cmd::PoolCmd, config_ton_http_api_cmd::TonHttpApiCmd, config_wallet_cmd::WalletCmd,
    master_wallet_cmd::MasterWalletCmd, utils::save_config,
};
use common::{
    TonWalletVersion,
    app_config::{
        AppConfig, ElectionsConfig, HttpConfig, KeyConfig, LogConfig, TonHttpApiConfig,
        WalletConfig,
    },
    task_cancellation::CancellationCtx,
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
    /// Manage contracts automation (auto-deploy / auto-topup) via the API
    #[command(name = "contracts-automation")]
    ContractsAutomation(ContractsAutomationCfgCmd),
    /// Manage log configuration
    Log(LogCmd),
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

impl ConfigCmd {
    pub async fn run(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let url = self.url.as_deref();
        let token = self.token.as_deref();
        let config_path = self.config.as_deref();

        match &self.action {
            ConfigAction::Generate(cmd) => cmd.run().await,
            ConfigAction::Node(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::Wallet(cmd) => cmd.run(config_path, cancellation_ctx, url, token).await,
            ConfigAction::Pool(cmd) => cmd.run(config_path, cancellation_ctx, url, token).await,
            ConfigAction::Bind(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::TonHttpApi(cmd) => cmd.run(url, token, config_path).await,
            ConfigAction::MasterWallet(cmd) => cmd.run(url, token, config_path).await,
            ConfigAction::Elections(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::ContractsAutomation(cmd) => cmd.run(config_path, url, token).await,
            ConfigAction::Log(cmd) => cmd.run(config_path, url, token).await,
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
            contracts_automation: Default::default(),
            log: Some(LogConfig::default()),
        };

        save_config(&config, path)?;

        Ok(())
    }
}
