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
    app_config::{AppConfig, WalletConfig},
    task_cancellation::CancellationCtx,
    ton_utils::extract_max_factor,
    vault_signer::VaultSigner,
};
use contracts::{WalletContract, contract_provider};
use secrets_vault::{
    errors::error::VaultError, types::secret::Secret, vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::{collections::HashMap, fs, path::Path, sync::Arc};
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

#[allow(dead_code)]
pub fn warn_ton_api_unavailable(error: &anyhow::Error, note: &str) {
    println!("\n{} {}", "[WARNING]".yellow().bold(), "Failed to connect to TON API".yellow(),);
    println!("  {} {}", "Reason:".yellow().bold(), error.root_cause().to_string());
    println!("  {} {}", "Note:".yellow().bold(), note.yellow().italic());
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

// Remove after all config commands switch to use the service API
#[allow(dead_code)]
pub async fn check_ton_api_connection(rpc_client: &ClientJsonRpc) -> anyhow::Result<()> {
    rpc_client.get_config_param(1).await.map(|_| ())
}

// Remove after all config commands switch to use the service API
#[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Service API client helpers
// ---------------------------------------------------------------------------

/// Returns the config path or an error if not provided.
pub fn require_config(config_path: Option<&str>) -> anyhow::Result<&Path> {
    config_path.map(Path::new).ok_or_else(|| anyhow::anyhow!("config is required for this command"))
}

/// Resolves the base URL for the nodectl service API.
/// Priority: `--url` flag, then `http.bind` from config file, then error.
pub fn resolve_service_url(url: Option<&str>, config_path: Option<&str>) -> anyhow::Result<String> {
    if let Some(u) = url {
        return Ok(normalize_base_url(u));
    }
    if let Some(path) = config_path {
        let path = Path::new(path);
        if path.exists() {
            let app_cfg = AppConfig::load(Path::new(path))?;
            return Ok(normalize_base_url(&app_cfg.http.bind));
        }
    }
    // Fallback to localhost:8080 if no URL or config path is provided.
    Ok("http://127.0.0.1:8080".to_string())
}

pub(crate) fn normalize_base_url(url: &str) -> String {
    let mut base = url.to_string();
    let trimmed = base.trim_start_matches("http://").trim_start_matches("https://");
    if trimmed.starts_with("0.0.0.0") {
        base = base.replacen("0.0.0.0", "127.0.0.1", 1);
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        base = format!("http://{}", base);
    }
    base.trim_end_matches('/').to_string()
}

/// Sends a GET request to the service API and returns the response body.
pub async fn api_get(base_url: &str, path: &str, token: Option<&str>) -> anyhow::Result<String> {
    send_request(reqwest::Method::GET, base_url, path, token, None::<&()>).await
}

/// Sends a POST request with a JSON body and returns the response body.
pub async fn api_post<B>(
    base_url: &str,
    path: &str,
    token: Option<&str>,
    body: &B,
) -> anyhow::Result<String>
where
    B: serde::Serialize,
{
    send_request(reqwest::Method::POST, base_url, path, token, Some(body)).await
}

/// Sends a DELETE request and returns the response body.
pub async fn api_delete(base_url: &str, path: &str, token: Option<&str>) -> anyhow::Result<String> {
    send_request(reqwest::Method::DELETE, base_url, path, token, None::<&()>).await
}

async fn send_request<B>(
    method: reqwest::Method,
    base_url: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&B>,
) -> anyhow::Result<String>
where
    B: serde::Serialize,
{
    let url = format!("{}/{}", base_url.trim_end_matches('/'), path.trim_start_matches('/'));
    let client = reqwest::Client::new();
    let mut req = client.request(method, &url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(b) = body {
        req = req.json(b);
    }
    let response = req.send().await.context(format!("failed to connect to {}", url))?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        // Try to extract `error.message` from the standard ApiErrorResponse.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(msg) =
                v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str())
        {
            anyhow::bail!("{msg}");
        }
        anyhow::bail!("request failed: status={}, body={}", status, body);
    }
    Ok(body)
}

/// Best-effort check that returns `true` only if we could reach the local vault
/// AND the secret is definitely absent. Any other outcome (no vault, lookup
/// error) is treated as "unknown" and produces no warning.
pub async fn vault_secret_missing(secret_name: &str) -> bool {
    match SecretVaultBuilder::from_env().await {
        Ok(vault) => vault.exists(&secret_name.into()).await.ok() == Some(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_base_url_adds_http_scheme_when_missing() {
        assert_eq!(normalize_base_url("example.com:8080"), "http://example.com:8080");
        assert_eq!(normalize_base_url("127.0.0.1:9000"), "http://127.0.0.1:9000");
    }

    #[test]
    fn test_normalize_base_url_preserves_existing_http_scheme() {
        assert_eq!(normalize_base_url("http://example.com:8080"), "http://example.com:8080");
    }

    #[test]
    fn test_normalize_base_url_preserves_existing_https_scheme() {
        assert_eq!(normalize_base_url("https://example.com:8080"), "https://example.com:8080");
    }

    #[test]
    fn test_normalize_base_url_replaces_bare_0_0_0_0_with_loopback() {
        assert_eq!(normalize_base_url("0.0.0.0:8080"), "http://127.0.0.1:8080");
    }

    #[test]
    fn test_normalize_base_url_replaces_0_0_0_0_with_http_scheme() {
        assert_eq!(normalize_base_url("http://0.0.0.0:8080"), "http://127.0.0.1:8080");
    }

    #[test]
    fn test_normalize_base_url_replaces_0_0_0_0_with_https_scheme() {
        assert_eq!(normalize_base_url("https://0.0.0.0:8080"), "https://127.0.0.1:8080");
    }

    #[test]
    fn test_normalize_base_url_replaces_only_first_0_0_0_0_occurrence() {
        assert_eq!(
            normalize_base_url("http://0.0.0.0/redirect/0.0.0.0"),
            "http://127.0.0.1/redirect/0.0.0.0"
        );
    }

    #[test]
    fn test_normalize_base_url_leaves_non_0_0_0_0_host_unchanged() {
        assert_eq!(normalize_base_url("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(normalize_base_url("http://10.0.0.0:8080"), "http://10.0.0.0:8080");
    }

    #[test]
    fn test_normalize_base_url_preserves_path() {
        assert_eq!(
            normalize_base_url("http://example.com:8080/api/v1"),
            "http://example.com:8080/api/v1"
        );
        assert_eq!(normalize_base_url("example.com/path"), "http://example.com/path");
    }

    #[test]
    fn test_normalize_base_url_trims_trailing_slash() {
        assert_eq!(normalize_base_url("http://example.com:8080/"), "http://example.com:8080");
        assert_eq!(normalize_base_url("127.0.0.1/"), "http://127.0.0.1");
    }
}
