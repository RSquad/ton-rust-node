/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    config_handlers::{
        BindingDto, BindingElectionStatusDto, BindingsResponse, ElectionsSettingsDto,
        ElectionsSettingsResponse, LogDto, LogResponse, MasterWalletDto, MasterWalletResponse,
        NodeDto, NodesResponse, PoolDto, PoolsResponse, StaticAdnlDto, StaticAdnlResponse,
        TonCorePoolSlotDataSource, TonCorePoolSlotDto, VotingConfigDto, VotingConfigResponse,
        VotingProposalAddRequest, VotingProposalDetailDto, VotingProposalDetailResponse,
        VotingProposalRowDto, VotingProposalsListResponse, WalletDto, WalletsResponse,
        v1_bindings_handler, v1_contracts_automation_settings_handler,
        v1_elections_settings_handler, v1_log_handler, v1_master_wallet_handler, v1_nodes_handler,
        v1_pools_handler, v1_voting_config_handler, v1_voting_proposals_add_handler,
        v1_voting_proposals_inspect_handler, v1_voting_proposals_list_handler,
        v1_voting_proposals_rm_handler, v1_wallets_handler,
    },
    login_rate_limiter::{LoginRateLimiter, login_limiter_key},
};
use crate::{
    audit::{AuditActorBuilder, AuditEventBuffer, log::AuditLog},
    auth::{
        Claims,
        jwt::JwtAuth,
        middleware,
        user_store::{UserStore, validate_username},
    },
    runtime_config::{RuntimeConfig, RuntimeConfigStore},
    task::task_manager::{TaskController, TaskStatus},
};
use common::{
    snapshot::{
        ElectionsSnapshot, ElectionsStatus, OurElectionParticipant, SnapshotStore, TimeRange,
        ValidatorsSnapshot,
    },
    task_cancellation::CancellationCtx,
    time_format,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<SnapshotStore>,
    pub runtime_cfg: Arc<RuntimeConfigStore>,
    pub elections_task: Arc<TaskController>,
    pub jwt_auth: Arc<JwtAuth>,
    pub user_store: Arc<UserStore>,
    pub(crate) login_rate_limiter: Arc<tokio::sync::Mutex<LoginRateLimiter>>,
    /// Signalled by mutation handlers after structural config changes
    /// (entity CRUD, ton-http-api) so the service loop can rebuild caches.
    pub config_changed: Arc<tokio::sync::Notify>,
    pub audit: Arc<dyn AuditLog>,
    pub actor_builder: Arc<AuditActorBuilder>,
    /// In-memory ring buffer for the REST read-path (e.g. GET /v1/elections).
    /// Never read from disk on the hot path.
    pub audit_ring: Arc<AuditEventBuffer>,
}

pub async fn run(
    cancellation_ctx: CancellationCtx,
    store: Arc<SnapshotStore>,
    runtime_cfg: Arc<RuntimeConfigStore>,
    tasks: HashMap<&'static str, Arc<TaskController>>,
    config_changed: Arc<tokio::sync::Notify>,
    audit: Arc<dyn AuditLog>,
    audit_ring: Arc<AuditEventBuffer>,
) {
    tracing::info!("http-server task started");

    let cfg = runtime_cfg.get();
    let bind = cfg.http.bind.clone();
    let enable_swagger = cfg.http.enable_swagger;
    let user_store = Arc::new(UserStore::new(runtime_cfg.clone() as Arc<dyn RuntimeConfig>));

    // Always create JwtAuth so that auth can be enabled at runtime via config
    // reload.
    // The middleware decides at request time whether to enforce authentication
    // by checking the live config.
    let vault = runtime_cfg.vault();
    let jwt_secret = cfg.http.auth.as_ref().and_then(|a| a.jwt_secret.clone());
    let jwt_auth = match JwtAuth::new(vault, jwt_secret.as_deref()).await {
        Ok(m) => {
            tracing::info!(
                target: "auth",
                event = "auth_jwt_key_ready",
                auth_configured = cfg.http.auth.is_some(),
                "JWT signing key loaded",
            );
            Arc::new(m)
        }
        Err(e) => {
            tracing::error!(
                target: "auth",
                event = "auth_setup_failed",
                error = ?e,
                "authentication setup failed",
            );
            return;
        }
    };
    drop(cfg);

    let bind_addr: SocketAddr = match bind.parse() {
        Ok(a) => a,
        Err(e) => {
            // Intentionally fall back to localhost (not 0.0.0.0) to avoid
            // accidentally exposing the API when the configured address is invalid.
            tracing::error!("invalid http.bind '{}': {} (fallback to 127.0.0.1:8080)", &bind, e);
            "127.0.0.1:8080".parse().expect("static bind must parse")
        }
    };

    let elections_task = tasks.get("elections").cloned().expect("elections task is not registered");

    let login_rate_limiter = Arc::new(tokio::sync::Mutex::new(LoginRateLimiter::default()));
    let actor_builder = Arc::new(AuditActorBuilder::new(runtime_cfg.clone()));
    let state = AppState {
        store,
        runtime_cfg,
        elections_task,
        jwt_auth,
        user_store,
        login_rate_limiter,
        config_changed,
        audit,
        actor_builder,
        audit_ring,
    };
    let app = routes(enable_swagger, state);

    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind to {}: {}", bind_addr, e);
            return;
        }
    };

    tracing::info!("http server listening on {}", bind_addr);

    let mut cancellation_rx = cancellation_ctx.subscribe();
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = cancellation_rx.changed().await;
        })
        .await
    {
        tracing::error!("http server error: {}", e);
    }

    tracing::info!("http-server task stopped");
}

pub(crate) fn routes(enable_swagger: bool, state: AppState) -> axum::Router {
    let mut public = axum::Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/openapi.json", axum::routing::get(openapi_handler))
        .route("/auth/login", axum::routing::post(login_handler));

    if enable_swagger {
        public = public
            .route("/swagger", axum::routing::get(swagger_ui_handler))
            .route("/swagger-ui", axum::routing::get(swagger_ui_handler));
    }

    // Auth middleware is always applied; it checks the live config on every
    // request and passes through when `http.auth` is not configured.
    let authenticated = axum::Router::new()
        .route("/v1/elections", axum::routing::get(v1_elections_handler))
        .route("/v1/elections/settings", axum::routing::get(v1_elections_settings_handler))
        .route("/v1/validators", axum::routing::get(v1_validators_handler))
        .route("/v1/nodes", axum::routing::get(v1_nodes_handler))
        .route("/v1/wallets", axum::routing::get(v1_wallets_handler))
        .route("/v1/pools", axum::routing::get(v1_pools_handler))
        .route("/v1/bindings", axum::routing::get(v1_bindings_handler))
        .route(
            "/v1/automation/settings",
            axum::routing::get(v1_contracts_automation_settings_handler),
        )
        .route("/v1/log", axum::routing::get(v1_log_handler))
        .route("/v1/voting/config", axum::routing::get(v1_voting_config_handler))
        .route("/v1/voting/proposals", axum::routing::get(v1_voting_proposals_list_handler))
        .route(
            "/v1/voting/proposals/{hash}",
            axum::routing::get(v1_voting_proposals_inspect_handler),
        )
        .route("/v1/master-wallet", axum::routing::get(v1_master_wallet_handler))
        .route("/auth/me", axum::routing::get(me_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_nominator,
        ));

    let operator_only = axum::Router::new()
        .route("/v1/elections/exclude", axum::routing::post(v1_elections_exclude_handler))
        .route("/v1/elections/include", axum::routing::post(v1_elections_include_handler))
        .route(
            "/v1/elections/settings",
            axum::routing::post(super::config_handlers::v1_elections_settings_update_handler),
        )
        .route(
            "/v1/automation/settings",
            axum::routing::post(
                super::config_handlers::v1_contracts_automation_settings_update_handler,
            ),
        )
        .route(
            "/v1/elections/static-adnl",
            axum::routing::post(super::config_handlers::v1_elections_static_adnl_handler),
        )
        .route(
            "/v1/elections/static-adnl/{node}",
            axum::routing::delete(super::config_handlers::v1_elections_static_adnl_disable_handler),
        )
        .route("/v1/voting/proposals", axum::routing::post(v1_voting_proposals_add_handler))
        .route("/v1/voting/proposals/{hash}", axum::routing::delete(v1_voting_proposals_rm_handler))
        .route("/v1/task/elections", axum::routing::post(v1_task_elections_handler))
        .route("/v1/nodes", axum::routing::post(super::config_handlers::v1_nodes_add_handler))
        .route(
            "/v1/nodes/{name}",
            axum::routing::delete(super::config_handlers::v1_nodes_rm_handler),
        )
        .route("/v1/wallets", axum::routing::post(super::config_handlers::v1_wallets_add_handler))
        .route(
            "/v1/wallets/{name}",
            axum::routing::delete(super::config_handlers::v1_wallets_rm_handler),
        )
        .route("/v1/pools", axum::routing::post(super::config_handlers::v1_pools_add_handler))
        .route(
            "/v1/pools/core",
            axum::routing::post(super::config_handlers::v1_pools_add_core_handler),
        )
        .route(
            "/v1/pools/{name}",
            axum::routing::delete(super::config_handlers::v1_pools_rm_handler),
        )
        .route("/v1/bindings", axum::routing::post(super::config_handlers::v1_bindings_add_handler))
        .route(
            "/v1/bindings/{node}",
            axum::routing::delete(super::config_handlers::v1_bindings_rm_handler),
        )
        .route(
            "/v1/ton-http-api",
            axum::routing::post(super::config_handlers::v1_ton_http_api_handler),
        )
        .route("/v1/log", axum::routing::post(super::config_handlers::v1_log_set_handler))
        .route("/auth/users", axum::routing::get(list_users_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_operator,
        ));

    axum::Router::new()
        .merge(public)
        .merge(authenticated)
        .merge(operator_only)
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}

// --- Error handling ---

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    pub code: i32,
    pub message: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ApiErrorResponse {
    pub ok: bool,
    pub error: ApiErrorBody,
}

#[derive(Debug)]
pub struct AppError {
    status: axum::http::StatusCode,
    body: ApiErrorBody,
}

impl AppError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_REQUEST,
            body: ApiErrorBody { code: 400, message: message.into() },
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::UNAUTHORIZED,
            body: ApiErrorBody { code: 401, message: message.into() },
        }
    }

    fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            body: ApiErrorBody { code: 429, message: message.into() },
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::NOT_FOUND,
            body: ApiErrorBody { code: 404, message: message.into() },
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiErrorBody { code: 500, message: message.into() },
        }
    }

    /// `503 Service Unavailable` — for upstream dependencies that are down
    /// (e.g. ton-http-api unreachable).
    pub(crate) fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            body: ApiErrorBody { code: 503, message: message.into() },
        }
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = ApiErrorResponse { ok: false, error: self.body };
        (self.status, axum::Json(body)).into_response()
    }
}

// --- DTO types ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub ok: bool,
    pub result: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsResponse {
    pub ok: bool,
    pub status: ElectionsStatus,
    pub result: Option<ElectionsSnapshot>,
    pub next_elections: Option<TimeRange>,
    pub our_participants: Vec<OurElectionParticipant>,
    /// Most recent elections audit events, newest first. Populated from the
    /// in-memory ring buffer; empty when audit is disabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Object>)]
    pub recent_events: Vec<serde_json::Value>,
}

#[derive(Clone, Default, serde::Deserialize)]
pub struct ElectionsQuery {
    /// Include full elections participants list in response.
    pub include_participants: Option<bool>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ValidatorsResponse {
    pub ok: bool,
    pub result: ValidatorsSnapshot,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ElectionsTaskAction {
    Enable,
    Disable,
    Restart,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsTaskControlRequest {
    pub action: ElectionsTaskAction,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatusDto {
    Running,
    Stopped,
}

impl From<TaskStatus> for TaskStatusDto {
    fn from(v: TaskStatus) -> Self {
        match v {
            TaskStatus::Running => TaskStatusDto::Running,
            TaskStatus::Stopped => TaskStatusDto::Stopped,
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsTaskControlResult {
    pub enabled: bool,
    pub status: TaskStatusDto,
    pub updated_at: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsTaskControlResponse {
    pub ok: bool,
    pub result: ElectionsTaskControlResult,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct NodeListRequest {
    pub nodes: Vec<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsExcludeResult {
    pub excluded: Vec<String>,
    pub updated_at: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ElectionsExcludeResponse {
    pub ok: bool,
    pub result: ElectionsExcludeResult,
}

// --- Auth DTO types ---

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LoginResponse {
    pub ok: bool,
    pub token: String,
    pub expires_in: u64,
    pub role: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub ok: bool,
    pub username: String,
    pub role: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UserListResponse {
    pub ok: bool,
    pub users: Vec<UserInfoDto>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UserInfoDto {
    pub username: String,
    pub role: String,
}

// --- Handlers ---

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse, example = json!({"ok": true, "result": "OK"})),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(())
)]
pub async fn health_handler() -> axum::Json<HealthResponse> {
    axum::Json(HealthResponse { ok: true, result: "OK".to_owned() })
}

#[utoipa::path(
    get,
    path = "/v1/elections",
    params(
        ("include_participants" = Option<bool>, Query, description = "Include full elections participants list")
    ),
    responses(
        (status = 200, description = "Current elections snapshot (may be null if not available yet)", body = ElectionsResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_elections_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ElectionsQuery>,
) -> axum::Json<ElectionsResponse> {
    use crate::audit::AuditSource;

    let include_participants = query.include_participants.unwrap_or(false);
    let view = state.store.get_elections_view(include_participants);

    let recent_events: Vec<serde_json::Value> = state
        .audit_ring
        .filter_collect(|e| e.payload.source() == AuditSource::Elections)
        .into_iter()
        .rev()
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect();

    axum::Json(ElectionsResponse {
        ok: true,
        result: view.elections,
        status: view.status,
        next_elections: view.next_elections,
        our_participants: view.our_participants,
        recent_events,
    })
}

#[utoipa::path(
    post,
    path = "/v1/elections/exclude",
    request_body = NodeListRequest,
    responses(
        (status = 200, description = "List of nodes excluded from elections", body = ElectionsExcludeResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_elections_exclude_handler(
    state: axum::extract::State<AppState>,
    claims: axum::Extension<Claims>,
    headers: axum::http::HeaderMap,
    req: axum::Json<NodeListRequest>,
) -> Result<axum::Json<ElectionsExcludeResponse>, AppError> {
    if state.runtime_cfg.get().elections.is_none() {
        return Err(AppError::bad_request("elections are not configured"));
    }

    let to_exclude = req.nodes.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            for node_id in &to_exclude {
                if let Some(binding) = cfg.bindings.get_mut(node_id) {
                    binding.enable = false;
                }
            }
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    let task = state.elections_task.clone();
    tokio::spawn(async move {
        let _ = task.restart().await;
    });

    let excluded: Vec<String> = state
        .runtime_cfg
        .get()
        .bindings
        .iter()
        .filter(|(_, b)| !b.enable)
        .map(|(name, _)| name.clone())
        .collect();
    tracing::info!("elections excluded: {}", excluded.join(", "));

    let changes: Vec<_> = to_exclude
        .iter()
        .map(|node| {
            super::rest_audit::config_field(
                format!("bindings.{node}.enable"),
                serde_json::json!(true),
                serde_json::json!(false),
            )
        })
        .collect();
    super::rest_audit::record_config_updated(
        &state,
        &claims,
        &headers,
        "elections",
        "elections.exclude",
        changes,
    )
    .await;

    let applied = ElectionsExcludeResult { excluded, updated_at: state.runtime_cfg.updated_at() };
    Ok(axum::Json(ElectionsExcludeResponse { ok: true, result: applied }))
}

#[utoipa::path(
    post,
    path = "/v1/elections/include",
    request_body = NodeListRequest,
    responses(
        (status = 200, description = "List of nodes excluded from elections", body = ElectionsExcludeResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_elections_include_handler(
    state: axum::extract::State<AppState>,
    claims: axum::Extension<Claims>,
    headers: axum::http::HeaderMap,
    req: axum::Json<NodeListRequest>,
) -> Result<axum::Json<ElectionsExcludeResponse>, AppError> {
    if state.runtime_cfg.get().elections.is_none() {
        return Err(AppError::bad_request("elections are not configured"));
    }

    let to_include = req.nodes.clone();
    state
        .runtime_cfg
        .update_and_save(|cfg| {
            for node_id in &to_include {
                if let Some(binding) = cfg.bindings.get_mut(node_id) {
                    binding.enable = true;
                }
            }
        })
        .map_err(|e| AppError::internal(e.to_string()))?;

    let task = state.elections_task.clone();
    tokio::spawn(async move {
        let _ = task.restart().await;
    });

    let excluded: Vec<String> = state
        .runtime_cfg
        .get()
        .bindings
        .iter()
        .filter(|(_, b)| !b.enable)
        .map(|(name, _)| name.clone())
        .collect();
    tracing::info!("elections excluded: {}", excluded.join(", "));

    let changes: Vec<_> = to_include
        .iter()
        .map(|node| {
            super::rest_audit::config_field(
                format!("bindings.{node}.enable"),
                serde_json::json!(false),
                serde_json::json!(true),
            )
        })
        .collect();
    super::rest_audit::record_config_updated(
        &state,
        &claims,
        &headers,
        "elections",
        "elections.include",
        changes,
    )
    .await;

    let applied = ElectionsExcludeResult { excluded, updated_at: state.runtime_cfg.updated_at() };
    Ok(axum::Json(ElectionsExcludeResponse { ok: true, result: applied }))
}

#[utoipa::path(
    get,
    path = "/v1/validators",
    responses(
        (status = 200, description = "Current validators snapshot", body = ValidatorsResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_validators_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::Json<ValidatorsResponse> {
    let snapshot = state.store.get();
    axum::Json(ValidatorsResponse { ok: true, result: snapshot.validators })
}

#[utoipa::path(
    post,
    path = "/v1/task/elections",
    request_body = ElectionsTaskControlRequest,
    responses(
        (status = 200, description = "Updated elections task state", body = ElectionsTaskControlResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn v1_task_elections_handler(
    state: axum::extract::State<AppState>,
    req: axum::Json<ElectionsTaskControlRequest>,
) -> axum::Json<ElectionsTaskControlResponse> {
    let st = match req.action {
        ElectionsTaskAction::Enable => state.elections_task.enable().await,
        ElectionsTaskAction::Disable => state.elections_task.disable().await,
        ElectionsTaskAction::Restart => state.elections_task.restart().await,
    };

    axum::Json(ElectionsTaskControlResponse {
        ok: true,
        result: ElectionsTaskControlResult {
            enabled: st.enabled,
            status: st.status.into(),
            updated_at: st.updated_at,
        },
    })
}

async fn openapi_handler() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(<ApiDoc as utoipa::OpenApi>::openapi())
}

async fn swagger_ui_handler() -> axum::response::Html<String> {
    axum::response::Html(
        r##"<!doctype html>
<html>
  <head>
    <meta charset="utf-8"/>
    <title>nodectld API</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css"/>
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script>
      window.onload = function() {
        SwaggerUIBundle({
          url: "/openapi.json",
          dom_id: "#swagger-ui",
          deepLinking: true,
          presets: [
            SwaggerUIBundle.presets.apis
          ],
          layout: "BaseLayout"
        });
      };
    </script>
  </body>
</html>"##
            .to_owned(),
    )
}

// --- Auth Handlers ---

#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ApiErrorResponse),
        (status = 429, description = "Too many login attempts", body = ApiErrorResponse),
        (status = 500, description = "Internal error", body = ApiErrorResponse)
    ),
    security(())
)]
pub async fn login_handler(
    state: axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    req: axum::Json<LoginRequest>,
) -> Result<axum::Json<LoginResponse>, AppError> {
    let (operator_ttl, nominator_ttl) = {
        let cfg_snapshot = state.runtime_cfg.get();
        let auth_cfg = cfg_snapshot
            .http
            .auth
            .as_ref()
            .ok_or_else(|| AppError::bad_request("authentication is not configured"))?;
        (auth_cfg.operator_token_ttl, auth_cfg.nominator_token_ttl)
    };

    validate_username(&req.username).map_err(|e| AppError::bad_request(&e.to_string()))?;

    let jwt_auth = &state.jwt_auth;
    let user_store = state.user_store.as_ref();
    let now = time_format::now();
    let limiter_key = login_limiter_key(&headers, &req.username);

    {
        let mut limiter = state.login_rate_limiter.lock().await;
        if limiter.is_blocked(&limiter_key, now) {
            tracing::warn!(
                target: "auth",
                event = "auth_login_rejected",
                status = 429,
                reason = "rate_limited",
                user = %req.username,
                rate_limit_key = %limiter_key,
                "login rejected"
            );
            super::rest_audit::record_login_rejected(
                &state,
                &req.username,
                "rate_limited",
                &headers,
            )
            .await;
            return Err(AppError::too_many_requests("too many login attempts, try again later"));
        }
    }

    let role = user_store.login(&req.username, &req.password).await.map_err(|e| {
        tracing::error!(
            target: "auth",
            event = "auth_login_backend_error",
            status = 500,
            user = %req.username,
            error = ?e,
            "login backend error"
        );
        AppError::internal("authentication backend error")
    })?;

    let role = match role {
        Some(role) => {
            let mut limiter = state.login_rate_limiter.lock().await;
            limiter.record_success(&limiter_key);
            role
        }
        None => {
            let mut limiter = state.login_rate_limiter.lock().await;
            if limiter.record_failure(&limiter_key, now).is_err() {
                return Err(AppError::too_many_requests("too many login attempts"));
            }
            if limiter.is_blocked(&limiter_key, now) {
                tracing::warn!(
                    target: "auth",
                    event = "auth_login_rejected",
                    status = 429,
                    reason = "rate_limit_threshold_reached",
                    user = %req.username,
                    rate_limit_key = %limiter_key,
                    "login rejected"
                );
                super::rest_audit::record_login_rejected(
                    &state,
                    &req.username,
                    "rate_limited",
                    &headers,
                )
                .await;
                return Err(AppError::too_many_requests(
                    "too many login attempts, try again later",
                ));
            }
            tracing::warn!(
                target: "auth",
                event = "auth_login_rejected",
                status = 401,
                reason = "invalid_credentials",
                user = %req.username,
                rate_limit_key = %limiter_key,
                "login rejected"
            );
            super::rest_audit::record_login_rejected(
                &state,
                &req.username,
                "invalid_credentials",
                &headers,
            )
            .await;
            return Err(AppError::unauthorized("invalid username or password"));
        }
    };

    let ttl = match role {
        crate::auth::Role::Operator => operator_ttl,
        crate::auth::Role::Nominator => nominator_ttl,
    };

    let (token, expires_in) = jwt_auth.generate(&req.username, role, ttl).map_err(|e| {
        tracing::error!(
            target: "auth",
            event = "auth_token_generation_error",
            status = 500,
            user = %req.username,
            error = ?e,
            "token generation error"
        );
        AppError::internal("token generation failed")
    })?;

    super::rest_audit::record_login_success(&state, &req.username, &role.to_string(), &headers)
        .await;

    Ok(axum::Json(LoginResponse { ok: true, token, expires_in, role: role.to_string() }))
}

#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, description = "Current user identity", body = MeResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn me_handler(
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::Json<MeResponse>, AppError> {
    let claims = req
        .extensions()
        .get::<Claims>()
        .ok_or_else(|| AppError::unauthorized("not authenticated"))?;

    Ok(axum::Json(MeResponse {
        ok: true,
        username: claims.sub.clone(),
        role: claims.role.to_string(),
    }))
}

#[utoipa::path(
    get,
    path = "/auth/users",
    responses(
        (status = 200, description = "List of registered users", body = UserListResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorResponse),
        (status = 403, description = "Insufficient permissions", body = ApiErrorResponse)
    ),
    security(("bearerAuth" = []))
)]
pub async fn list_users_handler(
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<UserListResponse>, AppError> {
    let users = state.user_store.list_users();

    Ok(axum::Json(UserListResponse {
        ok: true,
        users: users
            .into_iter()
            .map(|u| UserInfoDto { username: u.username, role: u.role.to_string() })
            .collect(),
    }))
}

struct BearerAuthAddon;

impl utoipa::Modify for BearerAuthAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::HttpBuilder::new()
                    .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("Paste a JWT token obtained from POST /auth/login"))
                    .build(),
            ),
        );
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    modifiers(&BearerAuthAddon),
    paths(
        health_handler,
        v1_elections_handler,
        v1_elections_exclude_handler,
        v1_elections_include_handler,
        v1_validators_handler,
        v1_task_elections_handler,
        super::config_handlers::v1_elections_settings_update_handler,
        super::config_handlers::v1_contracts_automation_settings_handler,
        super::config_handlers::v1_contracts_automation_settings_update_handler,
        super::config_handlers::v1_elections_static_adnl_handler,
        super::config_handlers::v1_elections_static_adnl_disable_handler,
        // It won't compile without full names
        super::config_handlers::v1_nodes_handler,
        super::config_handlers::v1_nodes_add_handler,
        super::config_handlers::v1_nodes_rm_handler,
        super::config_handlers::v1_wallets_handler,
        super::config_handlers::v1_wallets_add_handler,
        super::config_handlers::v1_wallets_rm_handler,
        super::config_handlers::v1_pools_handler,
        super::config_handlers::v1_pools_add_handler,
        super::config_handlers::v1_pools_add_core_handler,
        super::config_handlers::v1_pools_rm_handler,
        super::config_handlers::v1_bindings_handler,
        super::config_handlers::v1_bindings_add_handler,
        super::config_handlers::v1_bindings_rm_handler,
        super::config_handlers::v1_ton_http_api_handler,
        super::config_handlers::v1_log_set_handler,
        super::config_handlers::v1_elections_settings_handler,
        super::config_handlers::v1_log_handler,
        super::config_handlers::v1_voting_config_handler,
        super::config_handlers::v1_voting_proposals_list_handler,
        super::config_handlers::v1_voting_proposals_inspect_handler,
        super::config_handlers::v1_voting_proposals_add_handler,
        super::config_handlers::v1_voting_proposals_rm_handler,
        super::config_handlers::v1_master_wallet_handler,
        login_handler,
        me_handler,
        list_users_handler
    ),
    components(schemas(
        ApiErrorBody,
        ApiErrorResponse,
        HealthResponse,
        ElectionsResponse,
        NodeListRequest,
        ValidatorsResponse,
        common::app_config::StakePolicy,
        common::app_config::BindingStatus,
        common::app_config::LogRotation,
        common::app_config::LogOutput,
        common::app_config::TonCoreDeployMode,
        super::config_handlers::ContractsAutomationSettingsResponse,
        super::config_handlers::ContractsAutomationSettingsUpdateRequest,
        super::config_handlers::ElectionsSettingsUpdateRequest,
        super::config_handlers::WalletAmountsPatch,
        super::config_handlers::PoolAmountsPatch,
        common::app_config::ContractsAutomationConfig,
        common::app_config::WalletAmounts,
        common::app_config::PoolAmounts,
        ElectionsTaskAction,
        ElectionsTaskControlRequest,
        TaskStatusDto,
        ElectionsTaskControlResult,
        ElectionsTaskControlResponse,
        ElectionsExcludeResult,
        ElectionsExcludeResponse,
        NodeDto,
        NodesResponse,
        WalletDto,
        WalletsResponse,
        PoolDto,
        PoolsResponse,
        TonCorePoolSlotDataSource,
        TonCorePoolSlotDto,
        BindingDto,
        BindingsResponse,
        super::config_handlers::NodeAddRequest,
        super::config_handlers::WalletAddRequest,
        super::config_handlers::PoolAddRequest,
        super::config_handlers::PoolAddCoreRequest,
        super::config_handlers::BindingAddRequest,
        super::config_handlers::EntityRefDto,
        super::config_handlers::EntityRefResponse,
        super::config_handlers::OkResponse,
        super::config_handlers::StaticAdnlRequest,
        StaticAdnlDto,
        StaticAdnlResponse,
        super::config_handlers::TonHttpApiRequest,
        super::config_handlers::TonHttpApiResult,
        super::config_handlers::TonHttpApiResponse,
        super::config_handlers::LogSetRequest,
        BindingElectionStatusDto,
        ElectionsSettingsDto,
        ElectionsSettingsResponse,
        LogDto,
        LogResponse,
        VotingConfigDto,
        VotingConfigResponse,
        VotingProposalAddRequest,
        VotingProposalRowDto,
        VotingProposalsListResponse,
        VotingProposalDetailDto,
        VotingProposalDetailResponse,
        MasterWalletDto,
        MasterWalletResponse,
        LoginRequest,
        LoginResponse,
        MeResponse,
        UserListResponse,
        UserInfoDto,
        common::snapshot::Snapshot,
        common::snapshot::ElectionsStatus,
        common::snapshot::ElectionsSnapshot,
        common::snapshot::ElectionsParticipantSnapshot,
        common::snapshot::OurElectionParticipant,
        common::snapshot::ParticipationStatus,
        common::snapshot::StakeSubmission,
        common::snapshot::ValidatorsSnapshot,
        common::snapshot::ValidatorNodeSnapshot,
        common::snapshot::TimeRange
    )),
    info(
        title = "nodectld API",
        version = "0.1.0",
        description = "Node-control service API. Management, monitoring, and network tooling."
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        audit::{InMemoryAuditLog, NoopAuditLog, log::AuditLog},
        http::test_support,
        runtime_config::RuntimeConfigStore,
        task::task_manager::ServiceTask,
    };
    use axum::body::Body;
    use common::{
        app_config::{
            AppConfig, ElectionsConfig, HttpConfig, LogConfig, NodeBinding, StakePolicy,
            TonHttpApiConfig,
        },
        snapshot::{
            ElectionsParticipantSnapshot, ElectionsSnapshot, ElectionsStatus,
            OurElectionParticipant, StakeSubmission, TimeRange, ValidatorNodeSnapshot,
        },
        task_cancellation::CancellationCtx,
    };
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use tower::ServiceExt;

    struct NoopTask;

    #[async_trait::async_trait]
    impl ServiceTask for NoopTask {
        async fn run(
            &self,
            cancellation_ctx: CancellationCtx,
            _app_config: Arc<AppConfig>,
        ) -> anyhow::Result<()> {
            let mut cancel = cancellation_ctx.subscribe();
            let _ = cancel.changed().await;
            Ok(())
        }
    }

    fn test_elections_task() -> Arc<TaskController> {
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        Arc::new(TaskController::new("elections", NoopTask, runtime_cfg))
    }

    async fn test_state(
        store: Arc<SnapshotStore>,
        runtime_cfg: Arc<RuntimeConfigStore>,
        elections_task: Arc<TaskController>,
    ) -> AppState {
        test_state_with_audit(store, runtime_cfg, elections_task, Arc::new(NoopAuditLog)).await
    }

    async fn test_state_with_audit(
        store: Arc<SnapshotStore>,
        runtime_cfg: Arc<RuntimeConfigStore>,
        elections_task: Arc<TaskController>,
        audit: Arc<dyn AuditLog>,
    ) -> AppState {
        test_support::build_app_state_with(runtime_cfg, audit, store, Some(elections_task)).await
    }

    fn test_app_config(policy: StakePolicy) -> Arc<AppConfig> {
        test_app_config_with_bindings(policy, HashMap::new())
    }

    fn test_app_config_with_bindings(
        policy: StakePolicy,
        bindings: HashMap<String, NodeBinding>,
    ) -> Arc<AppConfig> {
        Arc::new(AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings,
            ton_http_api: TonHttpApiConfig::default(),
            http: HttpConfig { auth: None, ..Default::default() },
            elections: Some(ElectionsConfig { policy, ..Default::default() }),
            voting: None,
            master_wallet: None,
            tick_interval: 30,
            automation: Default::default(),
            log: Some(LogConfig::default()),
            audit_log: Default::default(),
        })
    }

    fn test_app_config_no_elections() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: TonHttpApiConfig::default(),
            http: HttpConfig { auth: None, ..Default::default() },
            elections: None,
            voting: None,
            master_wallet: None,
            tick_interval: 30,
            automation: Default::default(),
            log: Some(LogConfig::default()),
            audit_log: Default::default(),
        })
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn get_request(uri: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_json(uri: &str, body: &impl serde::Serialize) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(body).unwrap()))
            .unwrap()
    }

    fn collect_component_schema_refs(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(reference) = map.get("$ref").and_then(serde_json::Value::as_str) {
                    if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                        out.push(name.to_string());
                    }
                }
                for child in map.values() {
                    collect_component_schema_refs(child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    collect_component_schema_refs(child, out);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn stake_policy_invalid_fixed_zero_returns_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/settings",
                &serde_json::json!({ "policy": { "fixed": 0 } }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], 400);
    }

    #[tokio::test]
    async fn stake_policy_valid_fixed_returns_200() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/settings",
                &serde_json::json!({ "policy": { "fixed": 123 } }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["stake_policy"]["fixed"], 123);
    }

    #[tokio::test]
    async fn stake_policy_per_node_override_returns_200() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let state = test_state(store, runtime_cfg.clone(), elections_task).await;
        let app = routes(false, state);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/settings",
                &serde_json::json!({ "policy": { "fixed": 500 }, "node": "node1" }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);

        let cfg = runtime_cfg.get();
        let elections = cfg.elections.as_ref().unwrap();
        assert!(matches!(elections.policy, StakePolicy::Minimum));
        assert!(matches!(elections.policy_overrides.get("node1"), Some(StakePolicy::Fixed(500))));
    }

    #[tokio::test]
    async fn elections_settings_adaptive_timing_invalid_returns_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/settings",
                &serde_json::json!({ "sleep_period_pct": 0.9, "waiting_period_pct": 0.2 }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
    }

    #[tokio::test]
    async fn elections_settings_adaptive_timing_update_returns_200() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let state = test_state(store, runtime_cfg.clone(), elections_task).await;
        let app = routes(false, state);

        let resp = app
            .clone()
            .oneshot(post_json(
                "/v1/elections/settings",
                &serde_json::json!({ "sleep_period_pct": 0.25, "waiting_period_pct": 0.75 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["sleep_period_pct"], 0.25);
        assert_eq!(v["result"]["waiting_period_pct"], 0.75);

        let cfg = runtime_cfg.get();
        let elections = cfg.elections.as_ref().unwrap();
        assert_eq!(elections.sleep_period_pct, 0.25);
        assert_eq!(elections.waiting_period_pct, 0.75);

        let resp = app.oneshot(get_request("/v1/elections/settings")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["result"]["sleep_period_pct"], 0.25);
        assert_eq!(v["result"]["waiting_period_pct"], 0.75);
    }

    #[tokio::test]
    async fn contracts_automation_settings_get_returns_defaults() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/automation/settings")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["tick_interval_sec"], 40);
        assert_eq!(v["result"]["auto_deploy"], true);
        assert_eq!(v["result"]["auto_topup"], true);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_updates_tick() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/automation/settings",
                &serde_json::json!({ "tick_interval_sec": 60 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["tick_interval_sec"], 60);
        assert_eq!(runtime_cfg.get().automation.tick_interval_sec, 60);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_empty_body_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json("/v1/automation/settings", &serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_invalid_tick_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/automation/settings",
                &serde_json::json!({ "tick_interval_sec": 0 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_merges_wallet_deploy_and_toggles() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/automation/settings",
                &serde_json::json!({
                    "wallet": { "deploy": 2_000_000_000u64 },
                    "auto_deploy": false,
                    "auto_topup": false,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["wallet"]["deploy"].as_u64().unwrap(), 2_000_000_000);
        assert_eq!(v["result"]["auto_deploy"], false);
        assert_eq!(v["result"]["auto_topup"], false);

        let cfg = runtime_cfg.get();
        assert_eq!(cfg.automation.wallet.deploy, 2_000_000_000);
        assert!(!cfg.automation.auto_deploy);
        assert!(!cfg.automation.auto_topup);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_partial_pool_deploy_preserves_snp() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let snp_before = runtime_cfg.get().automation.pool.snp;

        let resp = app
            .oneshot(post_json(
                "/v1/automation/settings",
                &serde_json::json!({
                    "pool": { "ton_core": 3_000_000_000u64 },
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["pool"]["snp"].as_u64().unwrap(), snp_before);
        assert_eq!(v["result"]["pool"]["ton_core"].as_u64().unwrap(), 3_000_000_000u64);

        let cfg = runtime_cfg.get();
        assert_eq!(cfg.automation.pool.snp, snp_before);
        assert_eq!(cfg.automation.pool.ton_core, 3_000_000_000);
    }

    #[tokio::test]
    async fn contracts_automation_settings_post_updates_wallet_topup() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/automation/settings",
                &serde_json::json!({ "wallet": { "topup": 9_500_000_000u64 } }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["wallet"]["topup"].as_u64().unwrap(), 9_500_000_000u64);

        assert_eq!(runtime_cfg.get().automation.wallet.topup, 9_500_000_000);
    }

    #[tokio::test]
    async fn elections_task_disable_enable_restart_toggles_status() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let state = test_state(store, runtime_cfg, elections_task).await;

        // Disable
        let app = routes(false, state.clone());
        let resp = app
            .oneshot(post_json(
                "/v1/task/elections",
                &ElectionsTaskControlRequest { action: ElectionsTaskAction::Disable },
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["enabled"], false);
        assert_eq!(v["result"]["status"], "stopped");

        // Enable
        let app = routes(false, state.clone());
        let resp = app
            .oneshot(post_json(
                "/v1/task/elections",
                &ElectionsTaskControlRequest { action: ElectionsTaskAction::Enable },
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["result"]["enabled"], true);
        assert_eq!(v["result"]["status"], "running");

        // Restart
        let app = routes(false, state.clone());
        let resp = app
            .oneshot(post_json(
                "/v1/task/elections",
                &ElectionsTaskControlRequest { action: ElectionsTaskAction::Restart },
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["result"]["enabled"], true);
        assert_eq!(v["result"]["status"], "running");
    }

    #[tokio::test]
    async fn health_returns_200() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/health")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"], "OK");
    }

    #[tokio::test]
    async fn elections_returns_empty_snapshot() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/elections")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "closed");
        assert!(v["result"].is_null());
        assert!(v["our_participants"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn elections_returns_active_snapshot() {
        let store = Arc::new(SnapshotStore::new());
        store.update_with(|s| {
            s.elections_status = ElectionsStatus::Active;
            s.elections = Some(ElectionsSnapshot {
                election_id: 100,
                participants_count: 5,
                min_stake: "100".to_string(),
                participant_min_stake: Some("200".to_string()),
                participant_max_stake: Some("900".to_string()),
                participants: vec![ElectionsParticipantSnapshot {
                    pubkey: "aa".to_string(),
                    adnl: "bb".to_string(),
                    sender_addr: "cc".to_string(),
                    is_controlled: false,
                    stake: "300".to_string(),
                    max_factor: 3.0,
                    election_id: 100,
                }],
                ..Default::default()
            });
            s.next_elections_range =
                Some(TimeRange { start: 1000, end: 2000, ..Default::default() });
        });

        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/elections")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "active");
        assert_eq!(v["result"]["election_id"], 100);
        assert_eq!(v["result"]["participants_count"], 5);
        assert_eq!(v["result"]["min_stake"], "100");
        assert_eq!(v["result"]["participant_min_stake"], "200");
        assert_eq!(v["result"]["participant_max_stake"], "900");
        assert!(v["result"]["participants"].as_array().unwrap().is_empty());
        assert_eq!(v["next_elections"]["start"], 1000);
        assert_eq!(v["next_elections"]["end"], 2000);
        assert!(v["our_participants"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn elections_include_participants_query_returns_full_list() {
        let store = Arc::new(SnapshotStore::new());
        store.update_with(|s| {
            s.elections_status = ElectionsStatus::Active;
            s.elections = Some(ElectionsSnapshot {
                election_id: 100,
                participants_count: 1,
                participants: vec![ElectionsParticipantSnapshot {
                    pubkey: "aa".to_string(),
                    adnl: "bb".to_string(),
                    sender_addr: "cc".to_string(),
                    is_controlled: true,
                    stake: "300".to_string(),
                    max_factor: 3.0,
                    election_id: 100,
                }],
                ..Default::default()
            });
        });

        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp =
            app.oneshot(get_request("/v1/elections?include_participants=true")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["participants_count"], 1);
        assert_eq!(v["result"]["participants"].as_array().unwrap().len(), 1);
        assert_eq!(v["result"]["participants"][0]["pubkey"], "aa");
    }

    #[tokio::test]
    async fn elections_returns_our_participants() {
        let store = Arc::new(SnapshotStore::new());
        store.update_with(|s| {
            s.our_participants.push(OurElectionParticipant {
                node_id: "node-1".to_string(),
                stake_accepted: true,
                stake_submissions: vec![
                    StakeSubmission {
                        stake: "100".to_string(),
                        max_factor: 3.0,
                        submission_time: 12345,
                        submission_time_utc: "2024-01-01T00:00:00Z".to_string(),
                    },
                    StakeSubmission {
                        stake: "50".to_string(),
                        max_factor: 3.0,
                        submission_time: 12400,
                        submission_time_utc: "2024-01-01T00:01:00Z".to_string(),
                    },
                ],
                accepted_stake: Some("150".to_string()),
                elected: true,
                position: Some(5),
                ..Default::default()
            });
        });
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/elections")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        let participants = v["our_participants"].as_array().unwrap();
        assert_eq!(participants.len(), 1);
        assert_eq!(participants[0]["node_id"], "node-1");
        assert_eq!(participants[0]["stake_accepted"], true);
        let submissions = participants[0]["stake_submissions"].as_array().unwrap();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[0]["stake"], "100");
        assert_eq!(submissions[0]["max_factor"], 3.0);
        assert_eq!(submissions[1]["stake"], "50");
        assert_eq!(participants[0]["accepted_stake"], "150");
        assert_eq!(participants[0]["elected"], true);
        assert_eq!(participants[0]["position"], 5);
    }

    #[tokio::test]
    async fn validators_returns_empty_snapshot() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/validators")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert!(v["result"]["controlled_nodes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validators_returns_populated_snapshot() {
        let store = Arc::new(SnapshotStore::new());
        store.update_with(|s| {
            s.validators.controlled_nodes.push(ValidatorNodeSnapshot {
                node_id: "node-1".to_string(),
                is_validator: true,
                validator_index: Some(42),
                key_id: Some("a2V5X2lk".to_string()),
                adnl: Some("YWRubA==".to_string()),
                ..Default::default()
            });
            s.validators.default_stake_policy = StakePolicy::Minimum;
        });

        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/v1/validators")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        let nodes = v["result"]["controlled_nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["node_id"], "node-1");
        assert_eq!(nodes[0]["is_validator"], true);
        assert_eq!(nodes[0]["validator_index"], 42);
        assert_eq!(nodes[0]["key_id"], "a2V5X2lk");
        assert_eq!(nodes[0]["adnl"], "YWRubA==");
    }

    #[tokio::test]
    async fn openapi_returns_valid_schema() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app.oneshot(get_request("/openapi.json")).await.unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["info"]["title"], "nodectld API");
        assert!(v["paths"].as_object().unwrap().contains_key("/health"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/elections"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/validators"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/automation/settings"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/voting/config"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/voting/proposals"));
        assert!(v["paths"].as_object().unwrap().contains_key("/v1/voting/proposals/{hash}"));
        let schemas = v["components"]["schemas"].as_object().unwrap();
        assert!(schemas.contains_key("ElectionsStatus"));
        assert!(schemas.contains_key("NodeListRequest"));

        let mut refs = Vec::new();
        collect_component_schema_refs(&v, &mut refs);
        refs.sort();
        refs.dedup();
        let missing_refs: Vec<String> =
            refs.into_iter().filter(|name| !schemas.contains_key(name)).collect();
        assert!(
            missing_refs.is_empty(),
            "openapi has unresolved component schema refs: {:?}",
            missing_refs
        );
    }

    #[tokio::test]
    async fn elections_exclude_disables_bindings() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "node-a".to_string(),
            NodeBinding {
                wallet: "w1".to_string(),
                pool: None,
                enable: true,
                status: Default::default(),
            },
        );
        bindings.insert(
            "node-b".to_string(),
            NodeBinding {
                wallet: "w2".to_string(),
                pool: None,
                enable: true,
                status: Default::default(),
            },
        );

        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg = Arc::new(RuntimeConfigStore::from_app_config(
            test_app_config_with_bindings(StakePolicy::Minimum, bindings),
        ));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/exclude",
                &NodeListRequest { nodes: vec!["node-a".to_string()] },
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert!(v["result"]["excluded"].as_array().unwrap().contains(&serde_json::json!("node-a")));
        assert!(
            !v["result"]["excluded"].as_array().unwrap().contains(&serde_json::json!("node-b"))
        );

        let cfg = runtime_cfg.get();
        assert!(!cfg.bindings["node-a"].enable);
        assert!(cfg.bindings["node-b"].enable);
    }

    #[tokio::test]
    async fn config_noop_update_does_not_emit_audit_event() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config(StakePolicy::Minimum)));
        let elections_task = test_elections_task();
        let audit_mem = Arc::new(InMemoryAuditLog::new());
        let state =
            test_state_with_audit(store, runtime_cfg, elections_task, audit_mem.clone()).await;

        let app = routes(false, state);
        let resp = app
            .oneshot(post_json("/v1/elections/exclude", &NodeListRequest { nodes: vec![] }))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert!(audit_mem.drain().is_empty(), "empty diff must not emit rest_api.config_updated");
    }

    #[tokio::test]
    async fn elections_exclude_without_elections_config_returns_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config_no_elections()));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/exclude",
                &NodeListRequest { nodes: vec!["node-a".to_string()] },
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
    }

    #[tokio::test]
    async fn elections_include_enables_bindings() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "node-a".to_string(),
            NodeBinding {
                wallet: "w1".to_string(),
                pool: None,
                enable: false,
                status: Default::default(),
            },
        );
        bindings.insert(
            "node-b".to_string(),
            NodeBinding {
                wallet: "w2".to_string(),
                pool: None,
                enable: false,
                status: Default::default(),
            },
        );

        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg = Arc::new(RuntimeConfigStore::from_app_config(
            test_app_config_with_bindings(StakePolicy::Minimum, bindings),
        ));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg.clone(), elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/include",
                &NodeListRequest { nodes: vec!["node-a".to_string()] },
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        // node-b is still disabled, so it should be in the excluded list
        assert!(v["result"]["excluded"].as_array().unwrap().contains(&serde_json::json!("node-b")));
        assert!(
            !v["result"]["excluded"].as_array().unwrap().contains(&serde_json::json!("node-a"))
        );

        let cfg = runtime_cfg.get();
        assert!(cfg.bindings["node-a"].enable);
        assert!(!cfg.bindings["node-b"].enable);
    }

    #[tokio::test]
    async fn elections_include_without_elections_config_returns_400() {
        let store = Arc::new(SnapshotStore::new());
        let runtime_cfg =
            Arc::new(RuntimeConfigStore::from_app_config(test_app_config_no_elections()));
        let elections_task = test_elections_task();

        let app = routes(false, test_state(store, runtime_cfg, elections_task).await);

        let resp = app
            .oneshot(post_json(
                "/v1/elections/include",
                &NodeListRequest { nodes: vec!["node-a".to_string()] },
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn openapi_spec_contains_bearer_auth_scheme() {
        let spec = <ApiDoc as utoipa::OpenApi>::openapi();
        let json = serde_json::to_value(&spec).unwrap();

        // Security scheme is defined in components
        let scheme = &json["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], "http");
        assert_eq!(scheme["scheme"], "bearer");
        assert_eq!(scheme["bearerFormat"], "JWT");

        // Protected endpoint references the scheme
        let elections_security = &json["paths"]["/v1/elections"]["get"]["security"];
        assert!(elections_security.is_array(), "elections endpoint should have security");
        assert_eq!(elections_security[0]["bearerAuth"], serde_json::json!([]));

        // Public endpoints opt out of security
        let health_security = &json["paths"]["/health"]["get"]["security"];
        let login_security = &json["paths"]["/auth/login"]["post"]["security"];
        for (name, sec) in [("health", health_security), ("login", login_security)] {
            assert!(sec.is_array(), "{name} endpoint should have a security array");
            let arr = sec.as_array().unwrap();
            assert!(
                !arr.iter().any(|v| v.get("bearerAuth").is_some()),
                "{name} endpoint should not require bearerAuth"
            );
        }
    }
}
