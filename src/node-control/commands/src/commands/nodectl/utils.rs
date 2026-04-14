/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use colored::Colorize;
use common::{
    app_config::{AppConfig, PoolConfig, WalletConfig},
    task_cancellation::CancellationCtx,
    ton_utils::extract_max_factor,
    vault_signer::VaultSigner,
};
use contracts::{NominatorWrapperImpl, WalletContract, contract_provider, resolve_toncore_pool};
use secrets_vault::{
    errors::error::VaultError, types::secret::Secret, vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::{collections::HashMap, fs, io::Write, path::Path, sync::Arc};
use ton_block::MsgAddressInt;
use ton_http_api_client::v2::{
    client_json_rpc::ClientJsonRpc,
    data_models::{AccountState, GetWalletInformationRes},
};

const POLL_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(2);
pub const SEND_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(15);
pub const DEPLOY_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(60);

/// Logical name for the master wallet in CLI, `get_wallet_config`, and `config wallet ls`.
pub const MASTER_WALLET_RESERVED_NAME: &str = "master_wallet";

/// `max_stake_factor` from masterchain config param 17 as a float multiplier (e.g. `3.0`).
pub async fn fetch_network_max_factor(rpc_client: &ClientJsonRpc) -> anyhow::Result<f32> {
    extract_max_factor(rpc_client.get_config_param(17).await?)
}

/// TONCore nominator (two pools): slot index from `--pool-even` / `--pool-odd` (`0` = even round, `1` = odd).
#[inline]
pub fn toncore_pool_slot_from_cli_flags(pool_even: bool, pool_odd: bool) -> usize {
    match (pool_even, pool_odd) {
        (_, true) => 1,
        (true, false) | (false, false) => 0,
    }
}

/// Resolve the on-chain pool address from config, validator wallet address, and pool slot index.
///
/// Handles SNP (address or derived from owner), single TONCore, and dual-pool TONCore.
/// `pool_slot` is `0` for even rounds (or SNP) and `1` for odd rounds (`--pool-odd`).
pub fn resolve_pool_address_from_config(
    pool_cfg: &PoolConfig,
    validator_addr: &MsgAddressInt,
    pool_slot: usize,
) -> anyhow::Result<MsgAddressInt> {
    match pool_cfg {
        PoolConfig::SNP { .. } if pool_slot != 0 => {
            anyhow::bail!("--pool-odd is not applicable for SNP pools");
        }
        PoolConfig::SNP { address, owner } => match (address, owner) {
            (Some(addr), _) => addr.parse::<MsgAddressInt>().context("invalid pool address"),
            (None, Some(owner)) => {
                let owner_addr =
                    owner.parse::<MsgAddressInt>().context("invalid pool owner address")?;
                NominatorWrapperImpl::calculate_address(-1, &owner_addr, validator_addr)
            }
            (None, None) => anyhow::bail!("Pool has neither address nor owner configured"),
        },
        PoolConfig::TONCore { pools } => {
            if pool_slot > 1 {
                anyhow::bail!("TONCore pool slot must be 0 (even) or 1 (odd)");
            }
            if pool_slot == 1 && pools[1].is_none() {
                anyhow::bail!(
                    "--pool-odd is only valid for TONCore nominator with two pool slots configured"
                );
            }
            let slot = pools[pool_slot].as_ref().ok_or_else(|| {
                anyhow::anyhow!("TONCore pool slot {} is not configured", pool_slot)
            })?;
            match (&slot.address, &slot.params) {
                (Some(addr), None) => addr.parse::<MsgAddressInt>().context("invalid pool address"),
                (addr, Some(params)) => {
                    let resolved =
                        resolve_toncore_pool(validator_addr, addr.as_deref(), params.clone())?;
                    Ok(resolved.address)
                }
                (None, None) => {
                    anyhow::bail!("TONCore pool slot {} has neither address nor params", pool_slot)
                }
            }
        }
    }
}

pub fn warn_missing_secret(secret_name: &str) {
    println!("\n{} {}", "[WARNING]".yellow().bold(), "Vault secret is missing".yellow(),);
    println!(
        "  {} Secret '{}' does not exist in vault",
        "Reason:".yellow().bold(),
        secret_name.yellow()
    );
    println!(
        "  {} {}",
        "Note:".yellow().bold(),
        format!("Create it with `nodectl key add --name {secret_name}`").yellow().italic()
    );
}

pub fn warn_ton_api_unavailable(error: &anyhow::Error, note: &str) {
    println!("\n{} {}", "[WARNING]".yellow().bold(), "Failed to connect to TON API".yellow(),);
    println!("  {} {}", "Reason:".yellow().bold(), error.root_cause().to_string());
    println!("  {} {}", "Note:".yellow().bold(), note.yellow().italic());
}

pub fn confirm(prompt: &str) -> anyhow::Result<bool> {
    print!("{prompt} [y/N]: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

pub fn save_config(config: &AppConfig, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    fs::write(path, json)?;
    Ok(())
}

pub async fn load_config_vault(
    config_path: &Path,
) -> anyhow::Result<(AppConfig, Arc<SecretVault>)> {
    let config = AppConfig::load(config_path)?;
    let vault = SecretVaultBuilder::from_env().await?;

    Ok((config, vault))
}

pub async fn check_ton_api_connection(rpc_client: &ClientJsonRpc) -> anyhow::Result<()> {
    rpc_client.get_config_param(1).await.map(|_| ())
}

pub async fn try_create_rpc_client(config: &AppConfig) -> anyhow::Result<Arc<ClientJsonRpc>> {
    let client = ClientJsonRpc::connect_many(
        config.ton_http_api.resolved_endpoints(),
        config.ton_http_api.api_key.clone(),
    )?;
    check_ton_api_connection(&client).await.map(|_| Arc::new(client))
}

pub async fn load_config_vault_rpc_client(
    config_path: &Path,
) -> anyhow::Result<(AppConfig, Arc<SecretVault>, Arc<ClientJsonRpc>)> {
    let (config, vault) = load_config_vault(config_path).await?;
    let rpc_client = Arc::new(
        ClientJsonRpc::connect_many(
            config.ton_http_api.resolved_endpoints(),
            config.ton_http_api.api_key.clone(),
        )
        .context("ClientJsonRpc")?,
    );

    Ok((config, vault, rpc_client))
}

pub async fn wallet_address(
    wallet_cfg: &WalletConfig,
    vault: Arc<SecretVault>,
) -> anyhow::Result<(MsgAddressInt, Secret)> {
    let secret = wallet_cfg.key.read_secret(Some(vault)).await?;
    let keypair = secret.as_keypair()?;

    let pub_key = keypair
        .public_key()
        .await?
        .ok_or_else(|| anyhow::anyhow!(VaultError::empty_public_key("Empty public key")))?;

    let address = calculate_wallet_address(wallet_cfg, &pub_key).context("calculate_address")?;

    Ok((address, secret))
}

pub async fn wallet_info(
    rpc_client: Arc<ClientJsonRpc>,
    wallet_cfg: &WalletConfig,
    vault: Arc<SecretVault>,
) -> anyhow::Result<(MsgAddressInt, GetWalletInformationRes, Secret)> {
    let (wallet_address, secret) = wallet_address(wallet_cfg, vault).await?;
    let wallet_info = rpc_client.get_wallet_information(&wallet_address).await?;

    Ok((wallet_address, wallet_info, secret))
}

pub fn calculate_wallet_address(
    wallet_cfg: &WalletConfig,
    pub_key: &[u8],
) -> anyhow::Result<MsgAddressInt> {
    WalletContract::calculate_address(
        wallet_cfg.version,
        wallet_cfg.workchain,
        wallet_cfg.subwallet_id,
        pub_key,
    )
}

pub fn get_wallet_config<'a>(
    name: &str,
    wallets: &'a HashMap<String, WalletConfig>,
    master_wallet: Option<&'a WalletConfig>,
) -> anyhow::Result<&'a WalletConfig> {
    let config =
        if name == MASTER_WALLET_RESERVED_NAME { master_wallet } else { wallets.get(name) };
    config.ok_or_else(|| anyhow::anyhow!("Wallet not found '{}'", name))
}

pub async fn make_wallet(
    rpc_client: Arc<ClientJsonRpc>,
    wallet_cfg: &WalletConfig,
    secret: Secret,
    label: &str,
) -> anyhow::Result<WalletContract> {
    let wallet_signer = VaultSigner::new(secret)
        .await
        .with_context(|| format!("[{label}] create wallet signer"))?;

    let wallet = WalletContract::new(
        Box::new(wallet_signer),
        wallet_cfg.version,
        wallet_cfg.subwallet_id,
        wallet_cfg.workchain,
        contract_provider!(rpc_client.clone()),
    )
    .await
    .with_context(|| format!("[{label}] create wallet"))?;

    Ok(wallet)
}

async fn poll_until(
    cancellation_ctx: &CancellationCtx,
    max_wait: tokio::time::Duration,
    timeout_msg: &str,
    mut check: impl AsyncFnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let poll = async {
        loop {
            if cancellation_ctx.is_cancelled() {
                anyhow::bail!("Task cancelled");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if check().await? {
                return Ok(());
            }
        }
    };

    tokio::time::timeout(max_wait, poll).await.map_err(|_| anyhow::anyhow!("{timeout_msg}"))?
}

pub async fn wait_for_deploy(
    rpc_client: Arc<ClientJsonRpc>,
    address: &MsgAddressInt,
    cancellation_ctx: &CancellationCtx,
    verbose: bool,
    max_wait: tokio::time::Duration,
) -> anyhow::Result<()> {
    poll_until(cancellation_ctx, max_wait, "Timeout waiting for contract deployment", async || {
        if verbose {
            println!("\n{}: {}...", "Wait for deploy".bold(), address);
        }
        let info = rpc_client.get_address_information(address).await?;
        let deployed = info.state == AccountState::Active;
        if deployed && verbose {
            println!("\n{}: {}", "Deployed".bold(), address);
        }
        Ok(deployed)
    })
    .await
}

pub async fn wait_for_seqno_change(
    rpc_client: Arc<ClientJsonRpc>,
    address: &MsgAddressInt,
    initial_seqno: Option<u32>,
    cancellation_ctx: &CancellationCtx,
    max_wait: tokio::time::Duration,
) -> anyhow::Result<()> {
    poll_until(cancellation_ctx, max_wait, "Transaction timeout expired", async || {
        let info = rpc_client.get_wallet_information(address).await?;
        Ok(info.seqno != initial_seqno)
    })
    .await
}
