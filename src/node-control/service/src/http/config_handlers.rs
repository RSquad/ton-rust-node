/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::http_server_task::{AppError, AppState};
use crate::runtime_config::{RuntimeConfig, open_wallet};
use adnl::common::Timeouts;
use common::{
    TonWalletVersion,
    app_config::{
        AdnlConfig, BindingStatus, ElectionsConfig, EndpointEntry, KeyConfig, LogConfig, LogOutput,
        LogRotation, NodeBinding, PoolConfig, StakePolicy, TimeoutVariant, WalletConfig,
    },
    ton_utils::{extract_max_factor, normalize_ton_address},
};
use control_client::client_adnl::ControlClientAdnl;
use std::{collections::HashMap, path::PathBuf, str::FromStr};
use ton_block::MsgAddressInt;

/// `type_id` for ADNL public keys (Ed25519).
const ADNL_PUBKEY_TYPE_ID: i32 = 1209251014;

/// Logical name reserved for the master wallet entry; cannot be used as a regular wallet name.
const MASTER_WALLET_RESERVED_NAME: &str = "master_wallet";

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

// --- Nodes ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct NodeDto {
    pub name: String,
    pub control_server_endpoint: String,
    pub control_server_pubkey: String,
    pub control_client_secret: String,
    pub status: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct NodesResponse {
    pub ok: bool,
    pub result: Vec<NodeDto>,
}

// --- Wallets ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct WalletDto {
    pub name: String,
    pub secret: String,
    pub version: String,
    pub state: Option<String>,
    pub balance: Option<u64>,
    pub address: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct WalletsResponse {
    pub ok: bool,
    pub result: Vec<WalletDto>,
}

// --- Pools ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PoolDto {
    pub name: String,
    pub kind: String,
    pub balance: Option<u64>,
    pub address: Option<String>,
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validator_share: Option<u64>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PoolsResponse {
    pub ok: bool,
    pub result: Vec<PoolDto>,
}

// --- Bindings ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct BindingDto {
    pub node: String,
    pub wallet: String,
    pub pool: Option<String>,
    pub enable: bool,
    pub status: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct BindingsResponse {
    pub ok: bool,
    pub result: Vec<BindingDto>,
}

// --- Elections settings ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct BindingElectionStatusDto {
    pub name: String,
    pub enable: bool,
    pub status: BindingStatus,
    pub stake_policy: StakePolicy,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsSettingsDto {
    pub stake_policy: StakePolicy,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub policy_overrides: HashMap<String, StakePolicy>,
    pub max_factor: f32,
    pub tick_interval: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<BindingElectionStatusDto>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsSettingsResponse {
    pub ok: bool,
    pub result: ElectionsSettingsDto,
}

// --- Log ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LogDto {
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub rotation: LogRotation,
    pub output: LogOutput,
    pub max_size_mb: u64,
    pub max_files: usize,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LogResponse {
    pub ok: bool,
    pub result: LogDto,
}

// --- Master wallet ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct MasterWalletDto {
    pub address: Option<String>,
    pub balance: Option<u64>,
    pub state: Option<String>,
    pub version: String,
    pub subwallet_id: u32,
    pub secret: String,
    pub public_key: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct MasterWalletResponse {
    pub ok: bool,
    pub result: MasterWalletDto,
}

// --- Mutation requests (CRUD) ---

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct NodeAddRequest {
    pub name: String,
    pub control_server_endpoint: String,
    /// Base64-encoded Ed25519 public key of the node's control server.
    pub control_server_pubkey: String,
    /// Vault secret name holding the ADNL client private key.
    pub control_client_secret: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct WalletAddRequest {
    pub name: String,
    /// Vault secret name holding the wallet keypair.
    pub secret: String,
    /// Wallet version: V1R3, V3R2, V4R2 or V5R1 (case-insensitive).
    pub version: String,
    pub subwallet_id: u32,
    pub workchain: i32,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PoolAddRequest {
    pub name: String,
    /// Pool contract address (raw or base64url). At least one of `address`/`owner` is required.
    pub address: Option<String>,
    /// Owner address (raw or base64url). At least one of `address`/`owner` is required.
    pub owner: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct BindingAddRequest {
    pub node: String,
    pub wallet: String,
    pub pool: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct EntityRefDto {
    pub name: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct EntityRefResponse {
    pub ok: bool,
    pub result: EntityRefDto,
}

// --- Generic OK response (no payload) ---

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct OkResponse {
    pub ok: bool,
}

// --- Elections settings mutation ---

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsSettingsUpdateRequest {
    /// Stake policy to set. Ignored when `reset` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<StakePolicy>,
    /// Target node for per-node policy override. Omit to set the default policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Remove the per-node override (requires `node`). Other fields are still applied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    /// Elections tick interval in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick_interval: Option<u64>,
    /// Max stake factor (validated against network param 17).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_factor: Option<f32>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TonHttpApiRequest {
    pub urls: Vec<String>,
    pub api_key: Option<String>,
    /// When true, appends to existing endpoints (skipping duplicates).
    /// When false (default), replaces all endpoints.
    #[serde(default)]
    pub append: bool,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TonHttpApiResult {
    pub endpoints: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TonHttpApiResponse {
    pub ok: bool,
    pub result: TonHttpApiResult,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LogSetRequest {
    pub level: Option<String>,
    pub path: Option<String>,
    pub rotation: Option<LogRotation>,
    pub output: Option<LogOutput>,
    pub max_size_mb: Option<u64>,
    pub max_files: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

const NODE_STATUS_CHECK_TIMEOUT_SECS: u64 = 5;

#[utoipa::path(
    get,
    path = "/v1/nodes",
    responses(
        (status = 200, description = "List of configured nodes", body = NodesResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_nodes_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<NodesResponse>, AppError> {
    let config = state.runtime_cfg.get();

    if config.nodes.is_empty() {
        return Ok(axum::Json(NodesResponse { ok: true, result: vec![] }));
    }

    let adnl_configs = state.runtime_cfg.node_adnl_configs().await;

    // Check connectivity for all nodes concurrently.
    // AdnlClientConfig is not Clone, so we move configs out of the map.
    let mut set = tokio::task::JoinSet::new();
    let node_names: Vec<String> = config.nodes.keys().cloned().collect();
    let mut adnl_configs = adnl_configs;
    for name in &node_names {
        let adnl_config = adnl_configs.remove(name);
        let name = name.clone();
        set.spawn(async move {
            let status = match adnl_config {
                Some(cfg) => check_node_status(cfg).await,
                None => Err("adnl config error".to_string()),
            };
            (name, status)
        });
    }

    let mut statuses: HashMap<String, Result<(), String>> = HashMap::new();
    while let Some(result) = set.join_next().await {
        if let Ok((name, status)) = result {
            statuses.insert(name, status);
        }
    }

    let mut views: Vec<NodeDto> = config
        .nodes
        .iter()
        .map(|(name, adnl)| NodeDto {
            control_server_pubkey: match &adnl.server_key {
                KeyConfig::PublicKey { pub_key, .. } => {
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pub_key)
                }
                _ => "-".to_string(),
            },
            control_client_secret: match &adnl.client_key {
                KeyConfig::VaultKey { name } => name.clone(),
                _ => "-".to_string(),
            },
            status: match statuses.get(name) {
                Some(Ok(())) => "ok".to_string(),
                Some(Err(msg)) => msg.clone(),
                None => "unknown".to_string(),
            },
            control_server_endpoint: adnl.server_address.clone(),
            name: name.clone(),
        })
        .collect();
    views.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(axum::Json(NodesResponse { ok: true, result: views }))
}

async fn check_node_status(adnl_config: adnl::client::AdnlClientConfig) -> Result<(), String> {
    let timeout = tokio::time::Duration::from_secs(NODE_STATUS_CHECK_TIMEOUT_SECS);
    let result = tokio::time::timeout(timeout, async {
        let mut client = ControlClientAdnl::new(adnl_config, 3);
        client.connect().await.map_err(|e| e.root_cause().to_string())?;
        client.ping().await.map_err(|e| e.root_cause().to_string())?;
        let _ = client.shutdown().await;
        Ok(())
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err("timeout".to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/v1/wallets",
    responses(
        (status = 200, description = "List of configured wallets", body = WalletsResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_wallets_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<WalletsResponse>, AppError> {
    let config = state.runtime_cfg.get();
    let cached_wallets = state.runtime_cfg.wallets();
    let rpc_client = state.runtime_cfg.rpc_client();

    let mut all_wallets: Vec<(&str, &WalletConfig)> =
        config.wallets.iter().map(|(k, v)| (k.as_str(), v)).collect();
    if let Some(mw) = config.master_wallet.as_ref() {
        all_wallets.push(("master_wallet", mw));
    }

    let mut views = Vec::new();
    for (name, wallet_cfg) in all_wallets {
        let secret = match &wallet_cfg.key {
            KeyConfig::VaultKey { name } => name.clone(),
            _ => "-".to_string(),
        };

        let wallet: Option<std::sync::Arc<dyn contracts::TonWallet>> =
            if let Some(w) = cached_wallets.get(name) {
                Some(w.clone())
            } else {
                let vault = state.runtime_cfg.vault();
                open_wallet(wallet_cfg, rpc_client.clone(), vault, false).await.ok()
            };

        let (address, account_state, balance) = if let Some(wallet) = wallet {
            let addr = wallet.address();
            let addr_str = addr.to_string();
            match rpc_client.get_wallet_information(&addr).await {
                Ok(info) => {
                    (Some(addr_str), Some(info.account_state.to_string()), Some(info.balance))
                }
                Err(_) => (Some(addr_str), None, None),
            }
        } else {
            (None, None, None)
        };

        views.push(WalletDto {
            name: name.to_string(),
            secret,
            version: wallet_cfg.version.to_string(),
            state: account_state,
            balance,
            address,
        });
    }
    views.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(axum::Json(WalletsResponse { ok: true, result: views }))
}

#[utoipa::path(
    get,
    path = "/v1/pools",
    responses(
        (status = 200, description = "List of configured pools", body = PoolsResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_pools_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<PoolsResponse>, AppError> {
    let config = state.runtime_cfg.get();
    let cached_pools = state.runtime_cfg.pools();
    let rpc_client = state.runtime_cfg.rpc_client();

    let mut views = Vec::new();
    for (name, pool_cfg) in &config.pools {
        let (kind, address, balance, owner, addresses, validator_share) = match pool_cfg {
            PoolConfig::SNP { address, owner } => {
                // If Pool is bound to a node — use pre-loaded pool instance.
                //First, get the name of the node that is bound to the pool.
                let bound_node = config
                    .bindings
                    .iter()
                    .find(|(_, b)| b.pool == Some(name.clone()))
                    .map(|(node, _)| node.clone());
                let (addr, bal) = if let Some(n) = bound_node {
                    // Pool is bound to a node - get the pool instance from the cached pools.
                    if let Some(pool) = cached_pools.get(&n) {
                        let addr = pool.address().to_string();
                        let bal = pool.balance().await.ok();
                        (Some(addr), bal)
                    } else {
                        // For some reason, the pool is not in the cached pools - return None.
                        (None, None)
                    }
                // Pool has an explicit address in config — try to fetch balance directly
                } else if let Some(a) = address {
                    match MsgAddressInt::from_str(a) {
                        Ok(parsed) => {
                            let bal = rpc_client
                                .get_address_information(&parsed)
                                .await
                                .ok()
                                .map(|info| info.balance);
                            (Some(a.clone()), bal)
                        }
                        Err(_) => (Some(a.clone()), None),
                    }
                // Pool has neither cached instance nor address (e.g. only owner, no binding)
                } else {
                    (None, None)
                };
                ("SNP".to_string(), addr, bal, owner.clone(), None, None)
            }
            PoolConfig::TONCore { addresses, validator_share } => (
                "Core".to_string(),
                None,
                None,
                None,
                Some(addresses.to_vec()),
                Some(*validator_share),
            ),
        };
        views.push(PoolDto {
            name: name.clone(),
            kind,
            balance,
            address,
            owner,
            addresses,
            validator_share,
        });
    }
    views.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(axum::Json(PoolsResponse { ok: true, result: views }))
}

#[utoipa::path(
    get,
    path = "/v1/bindings",
    responses(
        (status = 200, description = "List of configured bindings", body = BindingsResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_bindings_handler(
    state: axum::extract::State<AppState>,
) -> axum::Json<BindingsResponse> {
    let config = state.runtime_cfg.get();

    let mut views: Vec<BindingDto> = config
        .bindings
        .iter()
        .map(|(node, b)| BindingDto {
            node: node.clone(),
            wallet: b.wallet.clone(),
            pool: b.pool.clone(),
            enable: b.enable,
            status: b.status.to_string(),
        })
        .collect();
    views.sort_by(|a, b| a.node.cmp(&b.node));

    axum::Json(BindingsResponse { ok: true, result: views })
}

#[utoipa::path(
    get,
    path = "/v1/elections/settings",
    responses(
        (status = 200, description = "Elections configuration", body = ElectionsSettingsResponse),
        (status = 400, description = "Elections not configured", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_elections_settings_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<ElectionsSettingsResponse>, AppError> {
    let config = state.runtime_cfg.get();
    let elections = config
        .elections
        .as_ref()
        .ok_or_else(|| AppError::bad_request("elections are not configured"))?;

    let bindings = build_binding_election_status(&config.bindings, elections);

    let dto = ElectionsSettingsDto {
        stake_policy: elections.policy.clone(),
        policy_overrides: elections.policy_overrides.clone(),
        max_factor: elections.max_factor,
        tick_interval: elections.tick_interval,
        bindings,
    };

    Ok(axum::Json(ElectionsSettingsResponse { ok: true, result: dto }))
}

fn build_binding_election_status(
    bindings: &HashMap<String, common::app_config::NodeBinding>,
    elections: &ElectionsConfig,
) -> Vec<BindingElectionStatusDto> {
    let mut result: Vec<_> = bindings
        .iter()
        .map(|(name, b)| BindingElectionStatusDto {
            name: name.clone(),
            enable: b.enable,
            status: b.status,
            stake_policy: elections.stake_policy(name).clone(),
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[utoipa::path(
    get,
    path = "/v1/log",
    responses(
        (status = 200, description = "Log configuration", body = LogResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_log_handler(state: axum::extract::State<AppState>) -> axum::Json<LogResponse> {
    let config = state.runtime_cfg.get();
    let log = config.log.as_ref().cloned().unwrap_or_default();

    let dto = log_config_to_dto(&log);
    axum::Json(LogResponse { ok: true, result: dto })
}

fn log_config_to_dto(log: &LogConfig) -> LogDto {
    LogDto {
        level: log.level.to_string(),
        path: log.path.as_ref().map(|p| p.display().to_string()),
        rotation: log.rotation.clone(),
        output: log.output.clone(),
        max_size_mb: log.max_size_mb,
        max_files: log.max_files,
    }
}

#[utoipa::path(
    get,
    path = "/v1/master-wallet",
    responses(
        (status = 200, description = "Master wallet info", body = MasterWalletResponse),
        (status = 400, description = "Master wallet not configured", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_master_wallet_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<MasterWalletResponse>, AppError> {
    let config = state.runtime_cfg.get();
    let master_wallet_cfg = config
        .master_wallet
        .as_ref()
        .ok_or_else(|| AppError::bad_request("master_wallet is not configured"))?;

    let secret_name = match &master_wallet_cfg.key {
        KeyConfig::VaultKey { name } => name.clone(),
        _ => "-".to_string(),
    };

    let master_wallet = state.runtime_cfg.master_wallet();
    let rpc_client = state.runtime_cfg.rpc_client();
    let addr = master_wallet.address();
    let addr_str = addr.to_string();

    let (address, account_state, balance, public_key) =
        match rpc_client.get_wallet_information(&addr).await {
            Ok(info) => {
                let pk = extract_public_key(&state).await;
                (Some(addr_str), Some(info.account_state.to_string()), Some(info.balance), pk)
            }
            Err(_) => (Some(addr_str), None, None, None),
        };

    let dto = MasterWalletDto {
        address,
        balance,
        state: account_state,
        version: master_wallet_cfg.version.to_string(),
        subwallet_id: master_wallet_cfg.subwallet_id,
        secret: secret_name,
        public_key,
    };

    Ok(axum::Json(MasterWalletResponse { ok: true, result: dto }))
}

async fn extract_public_key(state: &AppState) -> Option<String> {
    let config = state.runtime_cfg.get();
    let master_wallet_cfg = config.master_wallet.as_ref()?;
    let vault = state.runtime_cfg.vault()?;
    let secret = master_wallet_cfg.key.read_secret(Some(vault)).await.ok()?;
    if let secrets_vault::types::secret::Secret::KeyPair { keypair } = secret {
        let pk = keypair.public_key().await.ok()??;
        Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pk.as_slice()))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Mutation handlers (CRUD)
//
// Each handler validates input against the live config, then applies the
// change via `RuntimeConfigStore::update_and_save`, which persists to
// disk before swapping the in-memory snapshot (so a write failure leaves the
// live state untouched). Validation errors map to 400, missing entities to
// 404, I/O failures to 500. All routes are mounted behind `require_operator`.
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/v1/nodes",
    request_body = NodeAddRequest,
    responses(
        (status = 200, description = "Node added", body = EntityRefResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_nodes_add_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<NodeAddRequest>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let req = req.0;

    if state.runtime_cfg.get().nodes.contains_key(&req.name) {
        return Err(AppError::bad_request(format!("node '{}' already exists", req.name)));
    }

    let pub_key = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &req.control_server_pubkey,
    )
    .map_err(|_| AppError::bad_request("control_server_pubkey: invalid base64"))?;

    let adnl_config = AdnlConfig {
        server_address: req.control_server_endpoint,
        server_key: KeyConfig::PublicKey { type_id: ADNL_PUBKEY_TYPE_ID, pub_key },
        client_key: KeyConfig::VaultKey { name: req.control_client_secret },
        timeouts: TimeoutVariant::Single(Timeouts::DEFAULT_TIMEOUT.as_secs()),
    };

    let name = req.name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.nodes.insert(name, adnl_config);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name: req.name } }))
}

#[utoipa::path(
    delete,
    path = "/v1/nodes/{name}",
    params(("name" = String, Path, description = "Node name")),
    responses(
        (status = 200, description = "Node removed", body = EntityRefResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 404, description = "Node not found", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_nodes_rm_handler(
    state: axum::extract::State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let cfg = state.runtime_cfg.get();
    if !cfg.nodes.contains_key(&name) {
        return Err(AppError::not_found(format!("node '{name}' not found")));
    }
    if cfg.bindings.contains_key(&name) {
        return Err(AppError::bad_request(format!(
            "cannot remove node '{name}': referenced by a binding"
        )));
    }
    drop(cfg);

    let target = name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.nodes.remove(&target);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name } }))
}

#[utoipa::path(
    post,
    path = "/v1/wallets",
    request_body = WalletAddRequest,
    responses(
        (status = 200, description = "Wallet added", body = EntityRefResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_wallets_add_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<WalletAddRequest>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let req = req.0;

    if req.name == MASTER_WALLET_RESERVED_NAME {
        return Err(AppError::bad_request(format!(
            "'{MASTER_WALLET_RESERVED_NAME}' is a reserved name"
        )));
    }
    if state.runtime_cfg.get().wallets.contains_key(&req.name) {
        return Err(AppError::bad_request(format!("wallet '{}' already exists", req.name)));
    }

    let version = TonWalletVersion::from_str(&req.version)
        .map_err(|_| AppError::bad_request(format!("invalid wallet version: '{}'", req.version)))?;

    let wallet_config = WalletConfig {
        key: KeyConfig::VaultKey { name: req.secret },
        version,
        subwallet_id: req.subwallet_id,
        workchain: req.workchain,
    };

    let name = req.name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.wallets.insert(name, wallet_config);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name: req.name } }))
}

#[utoipa::path(
    delete,
    path = "/v1/wallets/{name}",
    params(("name" = String, Path, description = "Wallet name")),
    responses(
        (status = 200, description = "Wallet removed", body = EntityRefResponse),
        (status = 400, description = "Wallet is referenced or reserved", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 404, description = "Wallet not found", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_wallets_rm_handler(
    state: axum::extract::State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    if name == MASTER_WALLET_RESERVED_NAME {
        return Err(AppError::bad_request("the master wallet cannot be removed"));
    }

    let cfg = state.runtime_cfg.get();
    if !cfg.wallets.contains_key(&name) {
        return Err(AppError::not_found(format!("wallet '{name}' not found")));
    }
    if let Some((node, _)) = cfg.bindings.iter().find(|(_, b)| b.wallet == name) {
        return Err(AppError::bad_request(format!(
            "cannot remove wallet '{name}': referenced by binding for node '{node}'"
        )));
    }
    drop(cfg);

    let target = name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.wallets.remove(&target);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name } }))
}

#[utoipa::path(
    post,
    path = "/v1/pools",
    request_body = PoolAddRequest,
    responses(
        (status = 200, description = "Pool added", body = EntityRefResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_pools_add_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<PoolAddRequest>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let req = req.0;

    if req.address.is_none() && req.owner.is_none() {
        return Err(AppError::bad_request("at least one of 'address' or 'owner' is required"));
    }
    if state.runtime_cfg.get().pools.contains_key(&req.name) {
        return Err(AppError::bad_request(format!("pool '{}' already exists", req.name)));
    }

    let address = req
        .address
        .as_deref()
        .map(|a| normalize_ton_address(a, "address"))
        .transpose()
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    let owner = req
        .owner
        .as_deref()
        .map(|o| normalize_ton_address(o, "owner"))
        .transpose()
        .map_err(|e| AppError::bad_request(e.to_string()))?;

    let pool_config = PoolConfig::SNP { address, owner };

    let name = req.name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.pools.insert(name, pool_config);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name: req.name } }))
}

#[utoipa::path(
    delete,
    path = "/v1/pools/{name}",
    params(("name" = String, Path, description = "Pool name")),
    responses(
        (status = 200, description = "Pool removed", body = EntityRefResponse),
        (status = 400, description = "Pool is referenced by a binding", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 404, description = "Pool not found", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_pools_rm_handler(
    state: axum::extract::State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let cfg = state.runtime_cfg.get();
    if !cfg.pools.contains_key(&name) {
        return Err(AppError::not_found(format!("pool '{name}' not found")));
    }
    if let Some((node, _)) =
        cfg.bindings.iter().find(|(_, b)| b.pool.as_deref() == Some(name.as_str()))
    {
        return Err(AppError::bad_request(format!(
            "cannot remove pool '{name}': referenced by binding for node '{node}'"
        )));
    }
    drop(cfg);

    let target = name.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.pools.remove(&target);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name } }))
}

#[utoipa::path(
    post,
    path = "/v1/bindings",
    request_body = BindingAddRequest,
    responses(
        (status = 200, description = "Binding added", body = EntityRefResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_bindings_add_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<BindingAddRequest>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let req = req.0;

    let cfg = state.runtime_cfg.get();
    if cfg.bindings.contains_key(&req.node) {
        return Err(AppError::bad_request(format!(
            "binding for node '{}' already exists",
            req.node
        )));
    }
    if !cfg.nodes.contains_key(&req.node) {
        return Err(AppError::bad_request(format!("node '{}' not found", req.node)));
    }
    if !cfg.wallets.contains_key(&req.wallet) {
        return Err(AppError::bad_request(format!("wallet '{}' not found", req.wallet)));
    }
    if let Some(pool_name) = &req.pool {
        if !cfg.pools.contains_key(pool_name) {
            return Err(AppError::bad_request(format!("pool '{pool_name}' not found")));
        }
        // A pool may be bound to at most one node.
        if let Some((other_node, _)) =
            cfg.bindings.iter().find(|(_, b)| b.pool.as_deref() == Some(pool_name))
        {
            return Err(AppError::bad_request(format!(
                "pool '{pool_name}' is already bound to node '{other_node}'"
            )));
        }
    }
    drop(cfg);

    let binding = NodeBinding {
        wallet: req.wallet,
        pool: req.pool,
        enable: false,
        status: BindingStatus::default(),
    };

    let node = req.node.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.bindings.insert(node, binding);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name: req.node } }))
}

#[utoipa::path(
    delete,
    path = "/v1/bindings/{node}",
    params(("node" = String, Path, description = "Bound node name")),
    responses(
        (status = 200, description = "Binding removed", body = EntityRefResponse),
        (status = 400, description = "Binding is not idle", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse),
        (status = 404, description = "Binding not found", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_bindings_rm_handler(
    state: axum::extract::State<AppState>,
    axum::extract::Path(node): axum::extract::Path<String>,
) -> Result<axum::Json<EntityRefResponse>, AppError> {
    let cfg = state.runtime_cfg.get();
    let binding = cfg
        .bindings
        .get(&node)
        .ok_or_else(|| AppError::not_found(format!("binding for node '{node}' not found")))?;
    if binding.status != BindingStatus::Idle {
        return Err(AppError::bad_request(format!(
            "cannot remove binding for node '{node}': status is '{}', must be 'idle'. \
             Disable elections first and wait for stake recovery to complete.",
            binding.status
        )));
    }
    drop(cfg);

    let target = node.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            cfg.bindings.remove(&target);
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(axum::Json(EntityRefResponse { ok: true, result: EntityRefDto { name: node } }))
}

// ---------------------------------------------------------------------------
// Settings mutation handlers
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/v1/elections/settings",
    request_body = ElectionsSettingsUpdateRequest,
    responses(
        (status = 200, description = "Elections settings updated", body = ElectionsSettingsResponse),
        (status = 400, description = "Invalid request or elections not configured", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_elections_settings_update_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<ElectionsSettingsUpdateRequest>,
) -> Result<axum::Json<ElectionsSettingsResponse>, AppError> {
    let req = req.0;

    if req.policy.is_none() && !req.reset && req.tick_interval.is_none() && req.max_factor.is_none()
    {
        return Err(AppError::bad_request("at least one setting is required"));
    }

    let cfg = state.runtime_cfg.get();
    let elections = cfg
        .elections
        .as_ref()
        .ok_or_else(|| AppError::bad_request("elections are not configured"))?;

    // --- Validate max_factor against network param 17 (best-effort) ---
    if let Some(value) = req.max_factor {
        let network_limit = state
            .runtime_cfg
            .rpc_client()
            .get_config_param(17)
            .await
            .ok()
            .and_then(|p| extract_max_factor(p).ok());

        let probe = ElectionsConfig { max_factor: value, ..elections.clone() };
        probe.validate(network_limit).map_err(|e| AppError::bad_request(e.to_string()))?;
    }

    // --- Validate policy ---
    if let Some(ref policy) = req.policy {
        if !req.reset && matches!(policy, StakePolicy::Fixed(0)) {
            return Err(AppError::bad_request("fixed stake must be > 0"));
        }
    }
    if req.reset && req.node.is_none() {
        return Err(AppError::bad_request("reset requires 'node' to be set"));
    }
    drop(cfg);

    // --- Apply all changes in a single update ---
    let policy = req.policy;
    let node = req.node;
    let reset = req.reset;
    let tick_interval = req.tick_interval;
    let max_factor = req.max_factor;

    state
        .runtime_cfg
        .update_with(|cfg| {
            if let Some(elections) = &mut cfg.elections {
                // Stake policy
                if reset {
                    if let Some(ref node_id) = node {
                        elections.policy_overrides.remove(node_id);
                    }
                } else if let Some(policy) = policy {
                    if let Some(ref node_id) = node {
                        elections.policy_overrides.insert(node_id.clone(), policy);
                    } else {
                        elections.policy = policy;
                    }
                }

                if let Some(seconds) = tick_interval {
                    elections.tick_interval = seconds;
                }
                if let Some(value) = max_factor {
                    elections.max_factor = value;
                }
            }
        })
        .map_err(|e| AppError::internal(e.to_string()))?;
    state.runtime_cfg.save_to_file().map_err(|e| AppError::internal(e.to_string()))?;

    // Restart elections task so changes take effect immediately.
    let task = state.elections_task.clone();
    tokio::spawn(async move {
        let _ = task.restart().await;
    });

    // Return updated settings (without bindings — use GET for the full view).
    let config = state.runtime_cfg.get();
    let elections = config.elections.as_ref().expect("validated above");
    let dto = ElectionsSettingsDto {
        stake_policy: elections.policy.clone(),
        policy_overrides: elections.policy_overrides.clone(),
        max_factor: elections.max_factor,
        tick_interval: elections.tick_interval,
        bindings: vec![],
    };

    Ok(axum::Json(ElectionsSettingsResponse { ok: true, result: dto }))
}

#[utoipa::path(
    post,
    path = "/v1/ton-http-api",
    request_body = TonHttpApiRequest,
    responses(
        (status = 200, description = "TON HTTP API config updated", body = TonHttpApiResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_ton_http_api_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<TonHttpApiRequest>,
) -> Result<axum::Json<TonHttpApiResponse>, AppError> {
    let req = req.0;
    let urls: Vec<String> =
        req.urls.into_iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
    if urls.is_empty() {
        return Err(AppError::bad_request("at least one non-empty url is required"));
    }

    let api_key = req.api_key;
    let append = req.append;
    state
        .runtime_cfg
        .update_with(|cfg| {
            if append {
                let existing = cfg.ton_http_api.endpoints();
                for url in &urls {
                    if !existing.iter().any(|e| e == url) {
                        let entry = match &api_key {
                            Some(key) => {
                                EndpointEntry::WithKey { url: url.clone(), api_key: key.clone() }
                            }
                            None => EndpointEntry::Url(url.clone()),
                        };
                        cfg.ton_http_api.urls.push(entry);
                    }
                }
            } else {
                cfg.ton_http_api.urls = urls.into_iter().map(|u| EndpointEntry::Url(u)).collect();
                cfg.ton_http_api.api_key = api_key;
            }
        })
        .map_err(|e| AppError::internal(e.to_string()))?;
    state.runtime_cfg.save_to_file().map_err(|e| AppError::internal(e.to_string()))?;

    let endpoints = state.runtime_cfg.get().ton_http_api.endpoints();
    Ok(axum::Json(TonHttpApiResponse { ok: true, result: TonHttpApiResult { endpoints } }))
}

#[utoipa::path(
    post,
    path = "/v1/log",
    request_body = LogSetRequest,
    responses(
        (status = 200, description = "Log settings updated", body = LogResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_log_set_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<LogSetRequest>,
) -> Result<axum::Json<LogResponse>, AppError> {
    let req = req.0;
    if req.level.is_none()
        && req.path.is_none()
        && req.rotation.is_none()
        && req.output.is_none()
        && req.max_size_mb.is_none()
        && req.max_files.is_none()
    {
        return Err(AppError::bad_request("at least one setting is required"));
    }

    let level = req
        .level
        .as_deref()
        .map(|l| {
            tracing::Level::from_str(l)
                .map_err(|_| AppError::bad_request(format!("invalid log level: '{l}'")))
        })
        .transpose()?;

    // Pre-validate: file/all output requires a path (check against current + incoming)
    if let Some(ref output) = req.output {
        if matches!(output, LogOutput::File | LogOutput::All) {
            let has_path = req.path.is_some()
                || state.runtime_cfg.get().log.as_ref().and_then(|l| l.path.as_ref()).is_some();
            if !has_path {
                let mode = match output {
                    LogOutput::File => "file",
                    LogOutput::All => "all",
                    _ => unreachable!(),
                };
                return Err(AppError::bad_request(format!(
                    "output mode '{mode}' requires a log file path"
                )));
            }
        }
    }

    state
        .runtime_cfg
        .update_with(|cfg| {
            let log = cfg.log.get_or_insert_with(LogConfig::default);
            if let Some(l) = level {
                log.level = l;
            }
            if let Some(p) = &req.path {
                log.path = Some(PathBuf::from(p));
            }
            if let Some(r) = req.rotation {
                log.rotation = r;
            }
            if let Some(o) = req.output {
                log.output = o;
            }
            if let Some(s) = req.max_size_mb {
                log.max_size_mb = s;
            }
            if let Some(f) = req.max_files {
                log.max_files = f;
            }
        })
        .map_err(|e| AppError::internal(e.to_string()))?;
    state.runtime_cfg.save_to_file().map_err(|e| AppError::internal(e.to_string()))?;

    let config = state.runtime_cfg.get();
    let log = config.log.as_ref().cloned().unwrap_or_default();
    let dto = log_config_to_dto(&log);
    Ok(axum::Json(LogResponse { ok: true, result: dto }))
}
