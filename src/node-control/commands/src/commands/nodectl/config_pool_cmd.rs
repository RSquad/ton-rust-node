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
        calculate_wallet_address, save_config, try_create_rpc_client, warn_ton_api_unavailable,
    },
};
use colored::Colorize;
use common::{
    app_config::{AppConfig, PoolConfig},
    ton_utils::display_tons,
};
use contracts::{NOMINATOR_POOL_WORKCHAIN, NominatorWrapperImpl};
use secrets_vault::{vault::SecretVault, vault_builder::SecretVaultBuilder};
use std::{path::Path, str::FromStr, sync::Arc};
use ton_block::{ADDR_FORMAT_BOUNCE, ADDR_FORMAT_URL_SAFE, MsgAddressInt};
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
    Add(PoolAddCmd),
    /// List all configured pools
    Ls(PoolLsCmd),
    /// Remove a pool from the configuration
    Rm(PoolRmCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Add a pool to the configuration")]
pub struct PoolAddCmd {
    #[arg(short = 'n', long = "name", help = "Pool name (unique identifier)")]
    name: String,
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

impl PoolCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            PoolAction::Add(cmd) => cmd.run(path).await,
            PoolAction::Ls(cmd) => cmd.run(path).await,
            PoolAction::Rm(cmd) => cmd.run(path).await,
        }
    }
}

impl PoolAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
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

        if config.pools.contains_key(&self.name) {
            anyhow::bail!(
                "Pool '{}' already exists. Remove it first or use a different name.",
                self.name
            );
        }

        let pool_config = PoolConfig::SNP {
            address: normalized_address.clone(),
            owner: normalized_owner.clone(),
        };
        config.pools.insert(self.name.clone(), pool_config);
        save_config(&config, path)?;

        let info = match (&normalized_address, &normalized_owner) {
            (Some(a), Some(o)) => format!("address='{}', owner='{}'", a, o),
            (Some(a), None) => format!("address='{}'", a),
            (None, Some(o)) => format!("owner='{}' (address will be calculated on bind)", o),
            _ => unreachable!(),
        };
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
    validator_share: Option<u64>,
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
                    validator_share: None,
                });
            }
            PoolConfig::TONCore { addresses, validator_share } => {
                views.push(PoolView {
                    name: name.clone(),
                    kind: "Core".to_string(),
                    balance: None,
                    address: None,
                    owner: None,
                    addresses: Some(addresses.to_vec()),
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
                let addrs = v.addresses.as_deref().map(|a| a.join(", ")).unwrap_or_default();
                let share = v.validator_share.map(|s| s.to_string()).unwrap_or_default();
                println!(
                    "  {:<15} {:<6} {:<14} {:<50} share={}",
                    v.name, "Core", "-", addrs, share,
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
