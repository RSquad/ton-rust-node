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
use contracts::{SingleNominatorWrapper, WalletContract, contract_provider, resolve_toncore_pool};
use secrets_vault::{
    crypto::factory::CryptoFactory, errors::error::VaultError, types::secret::Secret,
    vault::SecretVault, vault_block::BlockCryptoFactory, vault_builder::SecretVaultBuilder,
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

/// Default timeout for establishing a TCP connection to the nodectl service.
pub(crate) const API_CONNECT_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(5);
/// Default overall request timeout for nodectl service REST calls.
pub(crate) const API_REQUEST_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(10);

const API_CONNECT_TIMEOUT_ENV: &str = "NODECTL_API_CONNECT_TIMEOUT_SECS";
const API_REQUEST_TIMEOUT_ENV: &str = "NODECTL_API_REQUEST_TIMEOUT_SECS";

/// Build the HTTP client used for all `nodectl` → service REST calls.
///
/// Applies a connect timeout and an overall request timeout so that
/// CLI commands fail fast when the service port is unreachable instead of
/// hanging indefinitely. Both timeouts can be overridden at runtime via
/// `NODECTL_API_CONNECT_TIMEOUT_SECS` and `NODECTL_API_REQUEST_TIMEOUT_SECS`.
#[must_use = "the client must be used to perform requests"]
pub(crate) fn build_api_client() -> anyhow::Result<reqwest::Client> {
    let connect = read_timeout_env(API_CONNECT_TIMEOUT_ENV, API_CONNECT_TIMEOUT);
    let request = read_timeout_env(API_REQUEST_TIMEOUT_ENV, API_REQUEST_TIMEOUT);
    build_api_client_with_timeouts(connect, request)
}

/// Build an HTTP client with explicit connect and overall request timeouts.
/// Used internally by [`build_api_client`] and directly from tests.
pub(crate) fn build_api_client_with_timeouts(
    connect: tokio::time::Duration,
    request: tokio::time::Duration,
) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(request)
        .build()
        .context("failed to build HTTP client")
}

fn read_timeout_env(var: &str, default: tokio::time::Duration) -> tokio::time::Duration {
    match std::env::var(var) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) => tokio::time::Duration::from_secs(secs),
            Err(_) => {
                tracing::warn!(
                    "invalid {}={:?} (expected integer seconds), falling back to default {:?}",
                    var,
                    raw,
                    default,
                );
                default
            }
        },
        Err(_) => default,
    }
}

/// Map a `reqwest::Error` into an actionable user-facing error for service
/// REST calls. Connect failures and timeouts get a dedicated message that
/// includes the attempted URL and a hint about overriding timeouts.
pub(crate) fn map_send_error(err: reqwest::Error, url: &str) -> anyhow::Error {
    if err.is_timeout() {
        anyhow::anyhow!(
            "request to {url} timed out: check that the nodectl service is running \
             and reachable; override with {API_CONNECT_TIMEOUT_ENV} / {API_REQUEST_TIMEOUT_ENV}"
        )
    } else if err.is_connect() {
        anyhow::anyhow!(
            "cannot reach nodectl service at {url}: {err}; check that the service \
             is running and the URL is correct (use --url or NODECTL_URL)"
        )
    } else {
        anyhow::Error::new(err).context(format!("request to {url} failed"))
    }
}

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
                SingleNominatorWrapper::calculate_address(-1, &owner_addr, validator_addr)
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
                    let resolved = resolve_toncore_pool(
                        validator_addr,
                        addr.as_deref(),
                        *params,
                        slot.deploy_mode,
                    )?;
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
    let vault = SecretVaultBuilder::from_env(BlockCryptoFactory {}.new_crypto()?).await?;

    Ok((config, vault))
}

pub async fn load_config_vault_rpc_client(
    config_path: &Path,
) -> anyhow::Result<(AppConfig, Arc<SecretVault>, Arc<ClientJsonRpc>)> {
    let (config, vault) = load_config_vault(config_path).await?;
    let rpc_client = Arc::new(
        ClientJsonRpc::connect_many(
            config.ton_http_api.resolved_endpoints(),
            config.ton_http_api.api_key.clone(),
            config.ton_http_api.resolved_timeouts(),
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
    let client = build_api_client()?;
    let mut req = client.request(method, &url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(b) = body {
        req = req.json(b);
    }
    let response = req.send().await.map_err(|e| map_send_error(e, &url))?;
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
    let crypto = BlockCryptoFactory {}.new_crypto();
    let crypto = match crypto {
        Ok(c) => c,
        Err(_) => return false,
    };

    match SecretVaultBuilder::from_env(crypto).await {
        Ok(vault) => {
            let key = secret_name.into();
            vault.exists(&key).await.ok() == Some(false)
        }
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

    /// Service is bound but never accepts: HTTP request hangs until the
    /// overall request timeout fires. Verifies that the CLI HTTP client
    /// honours the configured timeout and produces an actionable error.
    #[tokio::test]
    async fn build_api_client_times_out_when_service_does_not_respond() {
        // Bind a listener and intentionally never accept connections.
        // Connections sit in the OS accept backlog (ESTABLISHED from
        // client view) and the request timeout has to kick in.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/health");

        let client = build_api_client_with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(500),
        )
        .unwrap();

        let start = std::time::Instant::now();
        let err = client.get(&url).send().await.expect_err("request must fail");
        let elapsed = start.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "expected fast fail, took {elapsed:?}"
        );
        assert!(err.is_timeout(), "expected timeout error, got: {err}");

        let mapped = map_send_error(err, &url);
        let msg = format!("{mapped:#}");
        assert!(msg.contains("timed out"), "message missing timeout hint: {msg}");
        assert!(msg.contains(&url), "message missing URL: {msg}");
    }

    /// No listener bound: connect attempt is refused (RST) almost
    /// instantly. Verifies the actionable connect-error path.
    #[tokio::test]
    async fn build_api_client_reports_connection_refused() {
        // Bind to a port, capture it, drop the listener — port is now free
        // and the OS will RST any further connects.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/health");

        let client = build_api_client_with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(5),
        )
        .unwrap();

        let err = client.get(&url).send().await.expect_err("request must fail");
        assert!(err.is_connect(), "expected connect error, got: {err}");

        let mapped = map_send_error(err, &url);
        let msg = format!("{mapped:#}");
        assert!(msg.contains("cannot reach nodectl service"), "unexpected message: {msg}");
        assert!(msg.contains(&url), "message missing URL: {msg}");
    }
}
