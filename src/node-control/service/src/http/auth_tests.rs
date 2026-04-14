/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    auth::{Role, jwt::JwtAuth, user_store::UserStore},
    http::http_server_task::*,
    runtime_config::{RuntimeConfig, RuntimeConfigStore},
    task::task_manager::{ServiceTask, TaskController},
};
use argon2::PasswordHasher;
use axum::body::Body;
use base64::Engine;
use common::{
    app_config::{AuthConfig, UserEntry},
    snapshot::SnapshotStore,
    task_cancellation::CancellationCtx,
};
use http_body_util::BodyExt;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

struct Noop;

#[async_trait::async_trait]
impl ServiceTask for Noop {
    async fn run(
        &self,
        ctx: CancellationCtx,
        _: Arc<common::app_config::AppConfig>,
    ) -> anyhow::Result<()> {
        let mut c = ctx.subscribe();
        let _ = c.changed().await;
        Ok(())
    }
}

fn hash_test_password(password: &[u8]) -> String {
    let salt =
        argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    argon2::Argon2::default().hash_password(password, &salt).unwrap().to_string()
}

fn app_cfg_with_auth(auth: AuthConfig) -> Arc<common::app_config::AppConfig> {
    Arc::new(common::app_config::AppConfig {
        nodes: HashMap::new(),
        wallets: HashMap::new(),
        pools: HashMap::new(),
        bindings: HashMap::new(),
        ton_http_api: Default::default(),
        http: common::app_config::HttpConfig { auth: Some(auth), ..Default::default() },
        elections: Some(Default::default()),
        voting: None,
        master_wallet: None,
        tick_interval: 30,
        log: Some(Default::default()),
    })
}

fn app_cfg_no_auth() -> Arc<common::app_config::AppConfig> {
    Arc::new(common::app_config::AppConfig {
        nodes: HashMap::new(),
        wallets: HashMap::new(),
        pools: HashMap::new(),
        bindings: HashMap::new(),
        ton_http_api: Default::default(),
        http: common::app_config::HttpConfig { auth: None, ..Default::default() },
        elections: Some(Default::default()),
        voting: None,
        master_wallet: None,
        tick_interval: 30,
        log: Some(Default::default()),
    })
}

fn auth_config() -> AuthConfig {
    let hash = hash_test_password(b"pass1");
    AuthConfig {
        operator_token_ttl: 3600,
        nominator_token_ttl: 7200,
        min_password_length: 8,
        jwt_secret: Some(base64::engine::general_purpose::STANDARD.encode([42u8; 32])),
        users: vec![
            UserEntry {
                username: "op".into(),
                role: Role::Operator,
                password_name: None,
                password_hash: Some(hash.clone()),
                revoked_after: None,
            },
            UserEntry {
                username: "nom".into(),
                role: Role::Nominator,
                password_name: None,
                password_hash: Some(hash),
                revoked_after: None,
            },
        ],
    }
}

fn elections_task(rt: Arc<RuntimeConfigStore>) -> Arc<TaskController> {
    Arc::new(TaskController::new("elections", Noop, rt))
}

const TEST_JWT_SECRET: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="; // [42u8; 32]

async fn test_jwt_auth() -> Arc<JwtAuth> {
    Arc::new(JwtAuth::new(None, Some(TEST_JWT_SECRET)).await.unwrap())
}

async fn state_with_auth() -> AppState {
    let cfg = auth_config();
    let rt = Arc::new(RuntimeConfigStore::from_app_config(app_cfg_with_auth(cfg.clone())));
    AppState {
        store: Arc::new(SnapshotStore::new()),
        runtime_cfg: rt.clone(),
        elections_task: elections_task(rt.clone()),
        jwt_auth: test_jwt_auth().await,
        user_store: Arc::new(UserStore::new(rt as Arc<dyn RuntimeConfig>)),
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(Default::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
    }
}

async fn state_no_auth() -> AppState {
    let rt = Arc::new(RuntimeConfigStore::from_app_config(app_cfg_no_auth()));
    AppState {
        store: Arc::new(SnapshotStore::new()),
        runtime_cfg: rt.clone(),
        elections_task: elections_task(rt.clone()),
        jwt_auth: test_jwt_auth().await,
        user_store: Arc::new(UserStore::new(rt.clone() as Arc<dyn RuntimeConfig>)),
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(Default::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
    }
}

fn app(st: AppState) -> axum::Router {
    routes(false, st)
}

async fn json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn get_bearer(uri: &str, token: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body: &impl serde::Serialize) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap()
}

fn post_bearer(uri: &str, body: &impl serde::Serialize, token: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap()
}

// --- Login flow ---

#[tokio::test]
async fn login_valid_operator() {
    let st = state_with_auth().await;
    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "op".into(), password: "pass1".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["ok"], true);
    assert!(v["token"].as_str().unwrap().len() > 20);
    assert_eq!(v["role"], "operator");
    assert_eq!(v["expires_in"], 3600);
}

#[tokio::test]
async fn login_valid_nominator() {
    let st = state_with_auth().await;
    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "nom".into(), password: "pass1".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["role"], "nominator");
    assert_eq!(v["expires_in"], 7200);
}

#[tokio::test]
async fn login_wrong_password() {
    let st = state_with_auth().await;
    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "op".into(), password: "wrong".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn login_unknown_user() {
    let st = state_with_auth().await;
    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "ghost".into(), password: "pass1".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn login_rate_limit_after_repeated_failures() {
    let st = state_with_auth().await;
    let app = app(st);

    for _ in 0..4 {
        let resp = app
            .clone()
            .oneshot(post_json(
                "/auth/login",
                &LoginRequest { username: "op".into(), password: "wrong".into() },
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    let resp = app
        .clone()
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "op".into(), password: "wrong".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    let v = json(resp).await;
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], 429);
    assert_eq!(v["error"]["message"], "too many login attempts, try again later");
}

#[tokio::test]
async fn login_backend_error_message_is_sanitized() {
    let st = state_with_auth().await;
    st.runtime_cfg
        .update_with(|cfg| {
            let auth = cfg.http.auth.as_mut().expect("auth should be configured in test");
            let op = auth
                .users
                .iter_mut()
                .find(|u| u.username == "op")
                .expect("operator user should exist");
            op.password_hash = Some("not-a-valid-argon2-hash".to_owned());
        })
        .unwrap();

    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "op".into(), password: "pass1".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    let v = json(resp).await;
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], 500);
    assert_eq!(v["error"]["message"], "authentication backend error");
}

// --- JWT middleware ---

#[tokio::test]
async fn protected_route_no_token_401() {
    let st = state_with_auth().await;
    let resp = app(st).oneshot(get("/v1/elections")).await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn protected_route_invalid_token_401() {
    let st = state_with_auth().await;
    let resp = app(st).oneshot(get_bearer("/v1/elections", "not.a.jwt")).await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn protected_route_valid_operator_token_200() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;
    let resp = app(st).oneshot(get_bearer("/v1/elections", &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn protected_route_valid_nominator_token_200() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("nom", Role::Nominator, 3600).unwrap().0;
    let resp = app(st).oneshot(get_bearer("/v1/elections", &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

// --- Role-based access ---

#[tokio::test]
async fn nominator_forbidden_on_operator_route() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("nom", Role::Nominator, 3600).unwrap().0;
    let body = serde_json::json!({ "policy": "minimum" });
    let resp = app(st).oneshot(post_bearer("/v1/elections/settings", &body, &tok)).await.unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn operator_allowed_on_operator_route() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;
    let body = serde_json::json!({ "policy": { "fixed": 100 } });
    let resp = app(st).oneshot(post_bearer("/v1/elections/settings", &body, &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn operator_can_access_nominator_routes() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;
    let resp = app(st).oneshot(get_bearer("/v1/validators", &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

// --- Public routes ---

#[tokio::test]
async fn health_always_public() {
    let st = state_with_auth().await;
    let resp = app(st).oneshot(get("/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn login_endpoint_always_public() {
    let st = state_with_auth().await;
    let resp = app(st)
        .oneshot(post_json(
            "/auth/login",
            &LoginRequest { username: "op".into(), password: "pass1".into() },
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// --- Auth disabled (backward compat) ---

#[tokio::test]
async fn auth_disabled_all_routes_open() {
    let st = state_no_auth().await;
    let resp = app(st).oneshot(get("/v1/elections")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn auth_disabled_operator_routes_open() {
    let st = state_no_auth().await;
    let body = serde_json::json!({ "policy": { "fixed": 100 } });
    let resp = app(st).oneshot(post_json("/v1/elections/settings", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

// --- /auth/me ---

#[tokio::test]
async fn me_returns_operator_claims() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;
    let resp = app(st).oneshot(get_bearer("/auth/me", &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["username"], "op");
    assert_eq!(v["role"], "operator");
}

#[tokio::test]
async fn me_returns_nominator_claims() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("nom", Role::Nominator, 3600).unwrap().0;
    let resp = app(st).oneshot(get_bearer("/auth/me", &tok)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["username"], "nom");
    assert_eq!(v["role"], "nominator");
}

#[tokio::test]
async fn protected_route_token_revoked_by_cutoff_401() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;

    st.runtime_cfg
        .update_with(|cfg| {
            let auth = cfg.http.auth.as_mut().expect("auth should be configured in test");
            let op = auth
                .users
                .iter_mut()
                .find(|u| u.username == "op")
                .expect("operator user should exist");
            op.revoked_after = Some(u64::MAX);
        })
        .unwrap();

    let resp = app(st).oneshot(get_bearer("/v1/elections", &tok)).await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn protected_route_token_revoked_on_equal_cutoff_401() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;
    let claims = st.jwt_auth.verify(&tok).unwrap();

    st.runtime_cfg
        .update_with(|cfg| {
            let auth = cfg.http.auth.as_mut().expect("auth should be configured in test");
            let op = auth
                .users
                .iter_mut()
                .find(|u| u.username == "op")
                .expect("operator user should exist");
            op.revoked_after = Some(claims.iat);
        })
        .unwrap();

    let resp = app(st).oneshot(get_bearer("/v1/elections", &tok)).await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn protected_route_token_rejected_after_role_change_401() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;

    st.runtime_cfg
        .update_with(|cfg| {
            let auth = cfg.http.auth.as_mut().expect("auth should be configured in test");
            let op = auth
                .users
                .iter_mut()
                .find(|u| u.username == "op")
                .expect("operator user should exist");
            op.role = Role::Nominator;
        })
        .unwrap();

    let resp = app(st).oneshot(get_bearer("/v1/elections", &tok)).await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn create_user_via_rest_is_not_allowed() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;

    let resp = app(st)
        .oneshot(post_bearer(
            "/auth/users",
            &serde_json::json!({
                "username": "new-op",
                "password": "pass1",
                "role": "operator"
            }),
            &tok,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn delete_user_via_rest_returns_404() {
    let st = state_with_auth().await;
    let tok = st.jwt_auth.generate("op", Role::Operator, 3600).unwrap().0;

    let req = axum::http::Request::builder()
        .method("DELETE")
        .uri("/auth/users/op")
        .header("Authorization", format!("Bearer {tok}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(st).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
}
