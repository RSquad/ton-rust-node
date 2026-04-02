/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{TonWalletVersion, serde_utils, socket_utils::resolve_ip};
use adnl::{client::AdnlClientConfig, common::Timeouts};
use anyhow::Context;
use secrets_vault::{
    crypto::factory::{AutoCryptoFactory, CryptoFactory},
    types::{algorithm::Algorithm, metadata::Metadata, secret::Secret},
    vault::SecretVault,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use ton_block::Ed25519KeyOption;

fn default_ton_http_api_url() -> String {
    "http://127.0.0.1:3301/".to_owned()
}

/// A single ton-http-api endpoint entry.
///
/// Plain strings use the global `api_key`; objects can override it per-endpoint.
/// ```json
/// "urls": ["http://a/", { "url": "http://b/", "api_key": "key-for-b" }]
/// ```
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum EndpointEntry {
    Url(String),
    WithKey { url: String, api_key: String },
}

impl EndpointEntry {
    pub fn url(&self) -> &str {
        match self {
            EndpointEntry::Url(u) => u,
            EndpointEntry::WithKey { url, .. } => url,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match self {
            EndpointEntry::Url(_) => None,
            EndpointEntry::WithKey { api_key, .. } => Some(api_key),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct TonHttpApiConfig {
    /// Endpoint entries for the ton-http-api service.
    /// The first entry is the primary endpoint; the rest are used for failover.
    /// Each entry is either a plain URL string (uses global `api_key`)
    /// or an object `{ "url": "...", "api_key": "..." }` with its own key.
    #[serde(default)]
    pub urls: Vec<EndpointEntry>,
    /// Legacy single-endpoint field. Merged into the head of `urls` by [`Self::endpoints`].
    /// Skipped on serialization so that re-saved configs migrate to `urls` automatically.
    #[serde(default, skip_serializing)]
    url: Option<String>,
    /// Global API key used for endpoints that don't specify their own.
    pub api_key: Option<String>,
}

impl Default for TonHttpApiConfig {
    fn default() -> Self {
        Self {
            urls: vec![EndpointEntry::Url(default_ton_http_api_url())],
            url: None,
            api_key: None,
        }
    }
}

impl TonHttpApiConfig {
    /// Migrates the legacy `url` field into `urls` so that re-saving the
    /// config is non-destructive. Should be called once after deserialization
    /// (e.g. in [`AppConfig::load`]).
    fn normalize(&mut self) {
        if let Some(legacy) = self.url.take() {
            let trimmed = legacy.trim().to_string();
            if !trimmed.is_empty() && !self.urls.iter().any(|e| e.url() == trimmed) {
                self.urls.insert(0, EndpointEntry::Url(trimmed));
            }
        }
    }

    /// Returns deduplicated URL strings only (for display / logging).
    ///
    /// The legacy `url` field (if present) is prepended before `urls`.
    /// Falls back to the default endpoint when everything is empty.
    pub fn endpoints(&self) -> Vec<String> {
        self.resolved_endpoints().into_iter().map(|(url, _)| url).collect()
    }

    /// Returns deduplicated `(url, per_endpoint_api_key)` pairs.
    ///
    /// Per-endpoint key is `Some` only when the entry explicitly overrides
    /// the global `api_key`. Callers should fall back to the global key
    /// when the per-endpoint value is `None`.
    pub fn resolved_endpoints(&self) -> Vec<(String, Option<String>)> {
        let legacy = self.url.iter().map(|u| (u.as_str(), None));
        let entries = self.urls.iter().map(|e| (e.url(), e.api_key()));

        let mut seen = HashSet::with_capacity(self.urls.len() + 1);
        let mut result: Vec<(String, Option<String>)> = Vec::with_capacity(self.urls.len() + 1);
        for (url, key) in legacy.chain(entries) {
            let url = url.trim();
            if url.is_empty() {
                continue;
            }
            if seen.insert(url.to_string()) {
                result.push((url.to_string(), key.map(str::to_string)));
            }
        }
        if result.is_empty() {
            result.push((default_ton_http_api_url(), None));
        }
        result
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(untagged)]
pub enum KeyConfig {
    PrivateKey {
        type_id: i32,
        #[serde(with = "serde_utils::base64_key")]
        pvt_key: Vec<u8>,
    },
    PublicKey {
        type_id: i32,
        #[serde(with = "serde_utils::base64_key")]
        pub_key: Vec<u8>,
    },
    VaultKey {
        name: String,
    },
    #[serde(with = "serde_utils::hex_string")]
    KeyPair(Vec<u8>),
}

impl KeyConfig {
    pub async fn read_secret(&self, vault: Option<Arc<SecretVault>>) -> anyhow::Result<Secret> {
        match self {
            KeyConfig::PrivateKey { type_id: _, pvt_key } => {
                let metadata = Metadata::new(None, Algorithm::Ed25519, true);
                Secret::from_raw_data(&pvt_key, metadata, AutoCryptoFactory {}.new_crypto()?).await
            }
            KeyConfig::PublicKey { type_id: _, pub_key } => {
                let metadata = Metadata::new(None, Algorithm::None, true);
                Secret::from_raw_data(&pub_key, metadata, AutoCryptoFactory {}.new_crypto()?).await
            }
            KeyConfig::VaultKey { name } => {
                let vault =
                    vault.ok_or(anyhow::anyhow!("The secret vault is not set in the config"))?;
                let secret = vault.get(&name.into()).await?;
                let algo = secret.metadata().algorithm;

                if algo != Algorithm::Ed25519 {
                    anyhow::bail!(
                        "Invalid secret algorithm: expected Ed25519, got {}",
                        algo.as_str()
                    );
                }

                Ok(secret)
            }
            KeyConfig::KeyPair(data) => {
                let metadata = Metadata::new(None, Algorithm::Ed25519, true);

                Secret::from_raw_data(&data, metadata, AutoCryptoFactory {}.new_crypto()?).await
            }
        }
    }
}

fn default_http_bind() -> String {
    "0.0.0.0:8080".to_owned()
}

fn default_http_enable_swagger() -> bool {
    true
}

fn default_operator_ttl() -> u64 {
    86400 * 30 // 30 days
}

fn default_nominator_ttl() -> u64 {
    86400 // 1 day
}

fn default_min_password_length() -> usize {
    8
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Nominator = 0,
    Operator = 1,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Nominator => "nominator",
            Role::Operator => "operator",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "operator" => Ok(Role::Operator),
            "nominator" => Ok(Role::Nominator),
            other => anyhow::bail!("unknown role: {other}"),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct UserEntry {
    pub username: String,
    pub role: Role,
    /// Vault secret name where the password hash is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_name: Option<String>,
    /// Inline Argon2 password hash (testing fallback when vault is unavailable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// If set, JWTs with `iat` earlier than this unix timestamp are rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_after: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct AuthConfig {
    /// Operator token TTL in seconds.
    pub operator_token_ttl: u64,
    /// Nominator token TTL in seconds.
    pub nominator_token_ttl: u64,
    /// Minimum password length for new users. Defaults to 8.
    #[serde(default = "default_min_password_length")]
    pub min_password_length: usize,
    /// Base64-encoded 32-byte JWT signing key. Fallback when vault is not available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt_secret: Option<String>,
    /// Registered users. Each entry points to a vault secret holding the password hash,
    /// or contains an inline `password_hash` for testing without vault.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<UserEntry>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            operator_token_ttl: default_operator_ttl(),
            nominator_token_ttl: default_nominator_ttl(),
            min_password_length: default_min_password_length(),
            jwt_secret: None,
            users: Vec::new(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct HttpConfig {
    /// HTTP bind address, e.g. "127.0.0.1:8080" or "0.0.0.0:8080".
    #[serde(default = "default_http_bind")]
    pub bind: String,

    /// Expose Swagger UI endpoints.
    #[serde(default = "default_http_enable_swagger")]
    pub enable_swagger: bool,

    /// Authentication and authorization configuration.
    /// When `Some`, all protected routes require a valid JWT token.
    /// When `None`, all routes are open (auth explicitly disabled).
    /// Default: enabled with no users — all protected endpoints return 401
    /// until at least one user is created via `nodectl auth add`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind: default_http_bind(),
            enable_swagger: default_http_enable_swagger(),
            auth: Some(AuthConfig::default()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct WalletConfig {
    pub key: KeyConfig,
    #[serde(with = "crate::wallet_version::version_serde")]
    pub version: TonWalletVersion,
    pub subwallet_id: u32,
    #[serde(default = "default_workchain")]
    pub workchain: i32,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind")]
pub enum PoolConfig {
    #[serde(rename = "snp")]
    SNP {
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
    },
    #[serde(rename = "core")]
    TONCore {
        validator_share: u16,
        /// Even-round pool address. `None` = not deployed yet (will be derived from validator wallet).
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        even_pool_address: Option<String>,
        /// Odd-round pool address (same params but `min_validator_stake + 1`). `None` = not deployed.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        odd_pool_address: Option<String>,
        /// Deploy-time pool parameters; if omitted, defaults are applied in `contracts` (`resolve_deploy_pool_params`).
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        max_nominators: Option<u16>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        min_validator_stake: Option<u64>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        min_nominator_stake: Option<u64>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(untagged)]
pub enum TimeoutVariant {
    Single(u64),
    Detailed(Timeouts),
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct AdnlConfig {
    pub server_address: String,
    pub server_key: KeyConfig,
    pub client_key: KeyConfig,
    pub timeouts: TimeoutVariant,
}

impl AdnlConfig {
    pub async fn to_node_adnl_config(
        &self,
        vault: Option<Arc<SecretVault>>,
    ) -> anyhow::Result<AdnlClientConfig> {
        let server_key = self.server_key.read_secret(vault.clone()).await?;
        let client_key = self.client_key.read_secret(vault.clone()).await?;
        let timeouts = match self.timeouts.clone() {
            TimeoutVariant::Single(secs) => Timeouts::with_duration(Duration::from_secs(secs)),
            TimeoutVariant::Detailed(timeouts) => timeouts,
        };

        let blob = match server_key {
            Secret::Blob { blob } => blob,
            _ => anyhow::bail!("Unsupported server key type"),
        };

        let client_keypair = match client_key {
            Secret::KeyPair { keypair } => keypair,
            _ => anyhow::bail!("Unsupported secret type"),
        };

        let client_pvt_key = client_keypair.private_key().await?;
        let pvt_key = client_pvt_key.lock().await?;
        if pvt_key.len() < 32 {
            anyhow::bail!("invalid client private key length");
        }
        let client_key_opt = Ed25519KeyOption::from_private_key(&pvt_key[..32].try_into()?)?;
        let server_pub_key = blob.data().await?;
        let server_key = Ed25519KeyOption::from_public_key(
            server_pub_key
                .lock()
                .await?
                .deref()
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid public key length"))?,
        );

        Ok(AdnlClientConfig::new(
            Some(client_key_opt),
            resolve_ip(&self.server_address)?,
            server_key,
            timeouts,
        ))
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum StakePolicy {
    #[serde(rename = "fixed")]
    Fixed(u64),
    #[default]
    #[serde(rename = "split50")]
    Split50,
    #[serde(rename = "minimum")]
    Minimum,
}

impl std::fmt::Display for StakePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StakePolicy::Fixed(nanotons) => {
                let tons = crate::ton_utils::nanotons_to_tons_f64(*nanotons);
                if tons.fract() == 0.0 {
                    write!(f, "fixed({} TON)", tons as u64)
                } else {
                    write!(f, "fixed({} TON)", tons)
                }
            }
            StakePolicy::Split50 => write!(f, "split50"),
            StakePolicy::Minimum => write!(f, "minimum"),
        }
    }
}

impl StakePolicy {
    pub fn calculate_stake(&self, min_stake: u64, available_stake: u64) -> anyhow::Result<u64> {
        if available_stake < min_stake {
            anyhow::bail!(
                "not enough balance: available={}, min_stake={}",
                available_stake,
                min_stake
            );
        }
        let stake = match self {
            StakePolicy::Fixed(v) => v.to_owned().max(min_stake).min(available_stake),
            StakePolicy::Minimum => min_stake,
            StakePolicy::Split50 => (available_stake / 2).max(min_stake),
        };
        Ok(stake)
    }
}

fn default_workchain() -> i32 {
    -1
}

fn default_max_factor() -> f32 {
    3.0
}

fn default_tick_interval() -> u64 {
    40
}
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ElectionsConfig {
    #[serde(default)]
    pub policy: StakePolicy,
    /// Per-node stake policy overrides.
    /// Key is the node name.
    /// When a node has an entry here, it takes precedence over the default `policy`.
    #[serde(default)]
    pub policy_overrides: HashMap<String, StakePolicy>,
    #[serde(default = "default_max_factor")]
    pub max_factor: f32,
    /// Interval for elections runner in seconds
    #[serde(default = "default_tick_interval")]
    pub tick_interval: u64,
}

impl ElectionsConfig {
    /// Returns the stake policy for a given node.
    /// If the node has an override, that is returned; otherwise the default policy.
    pub fn stake_policy(&self, node_id: &str) -> &StakePolicy {
        self.policy_overrides.get(node_id).unwrap_or(&self.policy)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if !(1.0..=3.0).contains(&self.max_factor) {
            anyhow::bail!("max_factor must be in range [1.0..3.0]");
        }
        Ok(())
    }
}

impl Default for ElectionsConfig {
    fn default() -> Self {
        Self {
            policy: StakePolicy::default(),
            policy_overrides: HashMap::new(),
            max_factor: default_max_factor(),
            tick_interval: default_tick_interval(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct VotingConfig {
    #[serde(default)]
    pub proposals: Vec<String>,
    #[serde(default = "default_tick_interval")]
    pub tick_interval: u64,
}

/// Lifecycle status of a node binding.
///
/// Transitions:
/// - `idle` → `participating`: elections enabled and elections are open
/// - `participating` → `validating`: node appears in current validator set
/// - `validating` → `draining`: elections disabled or node left validator set with pending recovery
/// - `draining` → `idle`: recover stake reaches zero
/// - `validating` → `idle`: node left validator set and recover stake is zero
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BindingStatus {
    #[default]
    Idle,
    Participating,
    Draining,
    Validating,
}

impl std::fmt::Display for BindingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingStatus::Idle => write!(f, "idle"),
            BindingStatus::Participating => write!(f, "participating"),
            BindingStatus::Draining => write!(f, "draining"),
            BindingStatus::Validating => write!(f, "validating"),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct NodeBinding {
    pub wallet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// Whether this binding participates in elections. Defaults to `false`.
    #[serde(default)]
    pub enable: bool,
    /// Current lifecycle status. Managed by the service at runtime.
    #[serde(default)]
    pub status: BindingStatus,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LogRotation {
    Daily,
    Hourly,
    Never,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LogOutput {
    Console,
    File,
    All,
}

fn default_max_size_mb() -> u64 {
    50
}

fn default_max_files() -> usize {
    10
}

fn default_rotation() -> LogRotation {
    LogRotation::Daily
}

fn default_output() -> LogOutput {
    LogOutput::Console
}

fn default_level() -> tracing::Level {
    tracing::Level::INFO
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LogConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default = "default_max_size_mb")]
    pub max_size_mb: u64,
    #[serde(default = "default_max_files")]
    pub max_files: usize,
    #[serde(default = "default_rotation")]
    pub rotation: LogRotation,
    #[serde(with = "serde_utils::serde_level", default = "default_level")]
    pub level: tracing::Level,
    #[serde(default = "default_output")]
    pub output: LogOutput,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            path: None,
            max_size_mb: default_max_size_mb(),
            max_files: default_max_files(),
            rotation: default_rotation(),
            level: default_level(),
            output: default_output(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct AppConfig {
    pub nodes: HashMap<String, AdnlConfig>,
    #[serde(default)]
    pub wallets: HashMap<String, WalletConfig>,
    #[serde(default)]
    pub pools: HashMap<String, PoolConfig>,
    #[serde(default)]
    pub bindings: HashMap<String, NodeBinding>,
    pub ton_http_api: TonHttpApiConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elections: Option<ElectionsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voting: Option<VotingConfig>,
    pub http: HttpConfig,
    pub master_wallet: Option<WalletConfig>,
    /// Default interval for all tasks in seconds
    #[serde(default = "default_tick_interval")]
    pub tick_interval: u64,
    pub log: Option<LogConfig>,
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            anyhow::bail!(
                "Configuration file '{:?}' not found. Generate it first with: nodectl config generate --output={:?}",
                path,
                path
            );
        }

        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let file_ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_ascii_lowercase();

        let mut config = match file_ext.as_str() {
            "yaml" | "yml" => serde_yaml2::from_str::<Self>(&data).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to parse YAML config file '{}'. Error: {}",
                    path.display(),
                    e
                )
            })?,
            "json" => serde_json::from_str::<Self>(&data).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to parse JSON config file '{}'. Error: {}",
                    path.display(),
                    e
                )
            })?,
            other => anyhow::bail!("Unsupported config extension: {other}"),
        };

        config.ton_http_api.normalize();
        config.validate()?;

        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        self.elections.as_ref().map(|e| e.validate()).transpose()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &'static str =
        "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea";
    const OWNER: &'static str =
        "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb";

    #[test]
    fn test_calculate_stake_insufficient_balance() {
        let policy = StakePolicy::Minimum;
        let result = policy.calculate_stake(10, 9);
        assert!(result.is_err());
    }

    #[test]
    fn test_calculate_stake_fixed_clamped_to_available() {
        let policy = StakePolicy::Fixed(100);
        let stake = policy.calculate_stake(50, 80).unwrap();
        assert_eq!(stake, 80);
    }

    #[test]
    fn test_calculate_stake_split50_respects_minimum() {
        let policy = StakePolicy::Split50;
        let stake = policy.calculate_stake(70, 100).unwrap();
        assert_eq!(stake, 70);
    }

    #[test]
    fn test_calculate_stake_minimum_ok() {
        let policy = StakePolicy::Minimum;
        let stake = policy.calculate_stake(10, 100).unwrap();
        assert_eq!(stake, 10);
    }

    #[test]
    fn test_calculate_stake_split50_ok() {
        let policy = StakePolicy::Split50;
        let stake = policy.calculate_stake(10, 100).unwrap();
        assert_eq!(stake, 50);
    }

    #[test]
    fn test_calculate_stake_fixed_within_range() {
        let policy = StakePolicy::Fixed(60);
        let stake = policy.calculate_stake(10, 100).unwrap();
        assert_eq!(stake, 60);
    }

    #[test]
    fn test_policy_for_node_returns_default_when_no_override() {
        let config = ElectionsConfig {
            policy: StakePolicy::Minimum,
            policy_overrides: HashMap::new(),
            ..Default::default()
        };
        assert!(matches!(config.stake_policy("node1"), StakePolicy::Minimum));
    }

    #[test]
    fn test_policy_for_node_returns_override_when_present() {
        let mut overrides = HashMap::new();
        overrides.insert("node1".to_string(), StakePolicy::Fixed(500));
        let config = ElectionsConfig {
            policy: StakePolicy::Minimum,
            policy_overrides: overrides,
            ..Default::default()
        };
        assert!(matches!(config.stake_policy("node1"), StakePolicy::Fixed(500)));
        // Other nodes still get the default
        assert!(matches!(config.stake_policy("node2"), StakePolicy::Minimum));
    }

    #[test]
    fn test_policy_for_node_override_split50() {
        let mut overrides = HashMap::new();
        overrides.insert("nodeA".to_string(), StakePolicy::Split50);
        let config = ElectionsConfig {
            policy: StakePolicy::Fixed(1000),
            policy_overrides: overrides,
            ..Default::default()
        };
        assert!(matches!(config.stake_policy("nodeA"), StakePolicy::Split50));
        assert!(matches!(config.stake_policy("nodeB"), StakePolicy::Fixed(1000)));
    }

    #[test]
    fn test_pool_config_serde_snp_with_owner() {
        let addr = ADDR;
        let owner = OWNER;
        let value = serde_json::json!({
            "kind": "snp",
            "address": addr,
            "owner": owner,
        });
        let cfg: PoolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            cfg,
            PoolConfig::SNP { address: Some(addr.to_string()), owner: Some(owner.to_string()) }
        );

        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["kind"], "snp");
        assert_eq!(json["address"], addr);
        assert_eq!(json["owner"], owner);
    }

    #[test]
    fn test_pool_config_serde_snp_without_owner() {
        let addr = ADDR;
        let value = serde_json::json!({
            "kind": "snp",
            "address": addr,
        });
        let cfg: PoolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(cfg, PoolConfig::SNP { address: Some(addr.to_string()), owner: None });

        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["kind"], "snp");
        assert_eq!(json["address"], addr.to_string());
        assert!(json.get("owner").is_none());
    }

    #[test]
    fn test_pool_config_serde_core_no_addresses() {
        let value = serde_json::json!({
            "kind": "core",
            "validator_share": 50,
        });
        let cfg: PoolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            cfg,
            PoolConfig::TONCore {
                validator_share: 50,
                even_pool_address: None,
                odd_pool_address: None,
                max_nominators: None,
                min_validator_stake: None,
                min_nominator_stake: None,
            }
        );

        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["kind"], "core");
        assert_eq!(json["validator_share"], 50);
        assert!(json.get("even_pool_address").is_none());
        assert!(json.get("odd_pool_address").is_none());
    }

    #[test]
    fn test_pool_config_serde_core_with_addresses() {
        let addr1 = ADDR;
        let addr2 = OWNER;
        let value = serde_json::json!({
            "kind": "core",
            "validator_share": 100,
            "even_pool_address": addr1.to_string(),
            "odd_pool_address": addr2.to_string(),
            "max_nominators": 10,
            "min_validator_stake": 5_000_000_000_000u64,
            "min_nominator_stake": 1_000_000_000_000u64,
        });
        let cfg: PoolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            cfg,
            PoolConfig::TONCore {
                validator_share: 100,
                even_pool_address: Some(addr1.to_string()),
                odd_pool_address: Some(addr2.to_string()),
                max_nominators: Some(10),
                min_validator_stake: Some(5_000_000_000_000),
                min_nominator_stake: Some(1_000_000_000_000),
            }
        );
    }

    #[test]
    fn test_binding_status_serde_roundtrip() {
        for status in [
            BindingStatus::Idle,
            BindingStatus::Participating,
            BindingStatus::Draining,
            BindingStatus::Validating,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: BindingStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_binding_status_display() {
        assert_eq!(BindingStatus::Idle.to_string(), "idle");
        assert_eq!(BindingStatus::Participating.to_string(), "participating");
        assert_eq!(BindingStatus::Draining.to_string(), "draining");
        assert_eq!(BindingStatus::Validating.to_string(), "validating");
    }

    #[test]
    fn test_binding_status_default_is_idle() {
        let status: BindingStatus = Default::default();
        assert_eq!(status, BindingStatus::Idle);
    }

    #[test]
    fn test_node_binding_serde_with_status() {
        let binding = NodeBinding {
            wallet: "w1".to_string(),
            pool: Some("p1".to_string()),
            enable: true,
            status: BindingStatus::Validating,
        };
        let json = serde_json::to_value(&binding).unwrap();
        assert_eq!(json["enable"], true);
        assert_eq!(json["status"], "validating");
        assert_eq!(json["wallet"], "w1");
        assert_eq!(json["pool"], "p1");

        let parsed: NodeBinding = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.status, BindingStatus::Validating);
        assert!(parsed.enable);
    }

    #[test]
    fn test_node_binding_serde_defaults() {
        let json = serde_json::json!({"wallet": "w1"});
        let binding: NodeBinding = serde_json::from_value(json).unwrap();
        assert!(!binding.enable);
        assert_eq!(binding.status, BindingStatus::Idle);
        assert!(binding.pool.is_none());
    }

    #[test]
    fn test_node_binding_serde_enable_and_status() {
        let json = serde_json::json!({"wallet": "w1", "enable": true, "status": "draining"});
        let binding: NodeBinding = serde_json::from_value(json).unwrap();
        assert!(binding.enable);
        assert_eq!(binding.status, BindingStatus::Draining);
    }

    #[test]
    fn test_ton_http_api_endpoints_dedup_and_order() {
        let cfg = TonHttpApiConfig {
            urls: vec![
                EndpointEntry::Url("http://a/".into()),
                EndpointEntry::Url("http://b/".into()),
                EndpointEntry::Url("http://a/".into()),
            ],
            ..Default::default()
        };
        assert_eq!(cfg.endpoints(), vec!["http://a/", "http://b/"]);
    }

    #[test]
    fn test_ton_http_api_endpoints_single() {
        let cfg = TonHttpApiConfig {
            urls: vec![EndpointEntry::Url("http://single/".into())],
            ..Default::default()
        };
        assert_eq!(cfg.endpoints(), vec!["http://single/"]);
    }

    #[test]
    fn test_ton_http_api_endpoints_empty_falls_back_to_default() {
        let cfg = TonHttpApiConfig { urls: vec![], ..Default::default() };
        assert_eq!(cfg.endpoints(), vec!["http://127.0.0.1:3301/"]);
    }

    #[test]
    fn test_ton_http_api_compat_old_url_field() {
        // Old config format had "url" (singular string) instead of "urls"
        let old_json = r#"{"url": "http://custom:9999/", "api_key": "secret"}"#;
        let cfg: TonHttpApiConfig = serde_json::from_str(old_json).unwrap();
        assert_eq!(cfg.endpoints(), vec!["http://custom:9999/"]);
    }

    #[test]
    fn test_ton_http_api_compat_old_url_merged_with_urls() {
        let json = r#"{"url": "http://primary/", "urls": ["http://fallback/"], "api_key": null}"#;
        let cfg: TonHttpApiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.endpoints(), vec!["http://primary/", "http://fallback/"]);
    }

    #[test]
    fn test_ton_http_api_compat_old_url_deduped_with_urls() {
        let json = r#"{"url": "http://same/", "urls": ["http://same/", "http://other/"], "api_key": null}"#;
        let cfg: TonHttpApiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.endpoints(), vec!["http://same/", "http://other/"]);
    }

    #[test]
    fn test_ton_http_api_legacy_url_migrated_on_normalize() {
        let json = r#"{"url": "http://legacy/", "api_key": null}"#;
        let mut cfg: TonHttpApiConfig = serde_json::from_str(json).unwrap();
        cfg.normalize();
        let reserialized = serde_json::to_value(&cfg).unwrap();
        assert!(reserialized.get("url").is_none(), "legacy 'url' must not be serialized");
        assert_eq!(
            reserialized["urls"],
            serde_json::json!(["http://legacy/"]),
            "legacy url must be migrated into urls"
        );
    }

    #[test]
    fn test_ton_http_api_per_endpoint_api_key() {
        let json = r#"{
            "urls": ["http://a/", {"url": "http://b/", "api_key": "key-b"}],
            "api_key": "global"
        }"#;
        let cfg: TonHttpApiConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.endpoints(), vec!["http://a/", "http://b/"]);

        let resolved = cfg.resolved_endpoints();
        assert_eq!(resolved[0], ("http://a/".to_string(), None));
        assert_eq!(resolved[1], ("http://b/".to_string(), Some("key-b".to_string())));
    }

    #[test]
    fn test_ton_http_api_mixed_entries_serde_roundtrip() {
        let cfg = TonHttpApiConfig {
            urls: vec![
                EndpointEntry::Url("http://a/".into()),
                EndpointEntry::WithKey { url: "http://b/".into(), api_key: "secret".into() },
            ],
            ..Default::default()
        };
        let json = serde_json::to_value(&cfg).unwrap();
        let parsed: TonHttpApiConfig = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.endpoints(), vec!["http://a/", "http://b/"]);
        assert_eq!(parsed.resolved_endpoints()[1].1, Some("secret".to_string()));
    }
}
