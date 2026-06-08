/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! REST mutation tests for `/v1/nodes`, `/v1/wallets`, `/v1/pools`, `/v1/bindings`.
use crate::{
    auth::{jwt::JwtAuth, user_store::UserStore},
    http::http_server_task::*,
    runtime_config::{RuntimeConfig, RuntimeConfigStore},
    task::task_manager::{ServiceTask, TaskController},
};
use adnl::common::Timeouts;
use axum::body::Body;
use common::{
    TonWalletVersion,
    app_config::{
        AdnlConfig, AppConfig, BindingStatus, HttpConfig, KeyConfig, NodeBinding, PoolConfig,
        TimeoutVariant, WalletConfig,
    },
    snapshot::SnapshotStore,
    task_cancellation::CancellationCtx,
};
use http_body_util::BodyExt;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

const TEST_JWT_SECRET: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="; // [42u8; 32]

struct Noop;

#[async_trait::async_trait]
impl ServiceTask for Noop {
    async fn run(&self, ctx: CancellationCtx, _: Arc<AppConfig>) -> anyhow::Result<()> {
        let mut c = ctx.subscribe();
        let _ = c.changed().await;
        Ok(())
    }
}

async fn json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: &str) -> axum::http::Request<Body> {
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

fn delete(uri: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder().method("DELETE").uri(uri).body(Body::empty()).unwrap()
}

/// Local copy of the ADNL Ed25519 type_id. Kept in sync with `config_handlers::ADNL_PUBKEY_TYPE_ID`
/// — the value is never round-tripped through the prod code path, so a duplicate constant in tests
/// is fine and avoids widening crate-internal visibility.
const ADNL_PUBKEY_TYPE_ID: i32 = 1209251014;

fn empty_app_cfg() -> AppConfig {
    AppConfig {
        nodes: HashMap::new(),
        wallets: HashMap::new(),
        pools: HashMap::new(),
        bindings: HashMap::new(),
        ton_http_api: Default::default(),
        http: HttpConfig { auth: None, ..Default::default() },
        elections: Some(Default::default()),
        voting: None,
        master_wallet: None,
        tick_interval: 30,
        automation: Default::default(),
        log: Some(Default::default()),
        audit_log: Default::default(),
    }
}

fn valid_control_server_pubkey_b64() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([11u8; 32])
}

fn sample_node_adnl(name_suffix: &str) -> (String, AdnlConfig) {
    let name = format!("pre_{name_suffix}");
    let cfg = AdnlConfig {
        server_address: "127.0.0.1:3031".into(),
        server_key: KeyConfig::PublicKey { type_id: ADNL_PUBKEY_TYPE_ID, pub_key: vec![5u8; 32] },
        client_key: KeyConfig::VaultKey { name: "client_key".into() },
        timeouts: TimeoutVariant::Single(Timeouts::DEFAULT_TIMEOUT.as_secs()),
    };
    (name, cfg)
}

fn sample_wallet(name: &str) -> (String, WalletConfig) {
    (
        name.into(),
        WalletConfig {
            key: KeyConfig::VaultKey { name: format!("{name}_sec") },
            version: TonWalletVersion::V4R2,
            subwallet_id: 0,
            workchain: -1,
        },
    )
}

fn sample_snp_pool(name: &str) -> (String, PoolConfig) {
    (
        name.into(),
        PoolConfig::SNP {
            address: Some(
                "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea".into(),
            ),
            owner: Some(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb".into(),
            ),
        },
    )
}

async fn app_state(cfg: AppConfig) -> AppState {
    let rt = Arc::new(RuntimeConfigStore::from_app_config(Arc::new(cfg)));
    let jwt_auth = Arc::new(JwtAuth::new(None, Some(TEST_JWT_SECRET)).await.unwrap());
    AppState {
        store: Arc::new(SnapshotStore::new()),
        runtime_cfg: rt.clone(),
        elections_task: Arc::new(TaskController::new("elections", Noop, rt.clone())),
        jwt_auth,
        user_store: Arc::new(UserStore::new(rt as Arc<dyn RuntimeConfig>)),
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(Default::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
        audit: Arc::new(crate::audit::log::NoopAuditLog),
        audit_ring: crate::audit::AuditEventBuffer::new(0),
    }
}

/// Test-only state where mutations are persisted to `path` (TempDir-managed by caller).
async fn app_state_with_path(cfg: AppConfig, path: std::path::PathBuf) -> AppState {
    let rt = Arc::new(
        RuntimeConfigStore::from_app_config(Arc::new(cfg))
            .with_path(path.to_string_lossy().into_owned()),
    );
    let jwt_auth = Arc::new(JwtAuth::new(None, Some(TEST_JWT_SECRET)).await.unwrap());
    AppState {
        store: Arc::new(SnapshotStore::new()),
        runtime_cfg: rt.clone(),
        elections_task: Arc::new(TaskController::new("elections", Noop, rt.clone())),
        jwt_auth,
        user_store: Arc::new(UserStore::new(rt as Arc<dyn RuntimeConfig>)),
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(Default::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
        audit: Arc::new(crate::audit::log::NoopAuditLog),
        audit_ring: crate::audit::AuditEventBuffer::new(0),
    }
}

const POOL_ADDR_A: &str = "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea";

fn node_add_json(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "control_server_endpoint": "127.0.0.1:3033",
        "control_server_pubkey": valid_control_server_pubkey_b64(),
        "control_client_secret": "node_client_sec",
    })
}

fn wallet_add_json(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "secret": "wallet_sec",
        "version": "V4R2",
        "subwallet_id": 0,
        "workchain": -1,
    })
}

fn pool_add_json(name: &str, address: Option<&str>, owner: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "address": address,
        "owner": owner,
    })
}

fn binding_add_json(node: &str, wallet: &str, pool: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "node": node,
        "wallet": wallet,
        "pool": pool,
    })
}

// --- POST /v1/nodes ---

#[tokio::test]
async fn nodes_post_succeeds() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let resp = app.clone().oneshot(post_json("/v1/nodes", &node_add_json("node_a"))).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["name"], "node_a");

    let resp = app.oneshot(get("/v1/nodes")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    let names: Vec<String> = v["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["name"].as_str().map(String::from))
        .collect();
    assert!(names.contains(&"node_a".into()));
}

#[tokio::test]
async fn nodes_post_duplicate() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let body = node_add_json("dup_node");
    let resp = app.clone().oneshot(post_json("/v1/nodes", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let resp = app.oneshot(post_json("/v1/nodes", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("already exists"));
}

#[tokio::test]
async fn nodes_post_invalid_pubkey_base64() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let mut body = node_add_json("n_bad_pk");
    body["control_server_pubkey"] = "not-valid-base64!!!".into();
    let resp = app.oneshot(post_json("/v1/nodes", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("base64"));
}

// --- DELETE /v1/nodes ---

#[tokio::test]
async fn nodes_delete_succeeds() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    app.clone().oneshot(post_json("/v1/nodes", &node_add_json("to_rm"))).await.unwrap();
    let resp = app.oneshot(delete("/v1/nodes/to_rm")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn nodes_delete_not_found() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st).oneshot(delete("/v1/nodes/missing")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn nodes_delete_rejected_when_referenced_by_binding() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("bound");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wname, wcfg) = sample_wallet("bw");
    cfg.wallets.insert(wname.clone(), wcfg);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding { wallet: wname, pool: None, enable: false, status: BindingStatus::Idle },
    );
    let st = app_state(cfg).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete(&format!("/v1/nodes/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("referenced by a binding"));
}

// --- POST /v1/wallets ---

#[tokio::test]
async fn wallets_post_succeeds() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let resp =
        app.clone().oneshot(post_json("/v1/wallets", &wallet_add_json("w_main"))).await.unwrap();
    assert_eq!(resp.status(), 200);

    let resp = app.oneshot(get("/v1/wallets")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    let names: Vec<String> = v["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["name"].as_str().map(String::from))
        .collect();
    assert!(names.contains(&"w_main".into()));
}

#[tokio::test]
async fn wallets_post_reserved_master_wallet_name() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let body = wallet_add_json("master_wallet");
    let resp = app.oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("reserved"));
}

#[tokio::test]
async fn wallets_post_duplicate() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let body = wallet_add_json("w_dup");
    let resp = app.clone().oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let resp = app.oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

// --- DELETE /v1/wallets ---

#[tokio::test]
async fn wallets_delete_succeeds_when_orphan() {
    let mut cfg = empty_app_cfg();
    let (wn, wc) = sample_wallet("w_free");
    cfg.wallets.insert(wn, wc);
    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete("/v1/wallets/w_free")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn wallets_delete_rejected_when_bound() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("w_del");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wname, wcfg) = sample_wallet("w_bound");
    cfg.wallets.insert(wname.clone(), wcfg);
    cfg.bindings.insert(
        nname,
        NodeBinding {
            wallet: wname.clone(),
            pool: None,
            enable: false,
            status: BindingStatus::Idle,
        },
    );
    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete(&format!("/v1/wallets/{wname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("referenced by binding"));
}

#[tokio::test]
async fn wallets_delete_master_wallet_rejected() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st).oneshot(delete("/v1/wallets/master_wallet")).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("master wallet"));
}

#[tokio::test]
async fn wallets_delete_not_found() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st).oneshot(delete("/v1/wallets/no_such_wallet")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- POST /v1/pools (SNP) ---

#[tokio::test]
async fn pools_post_succeeds() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st)
        .oneshot(post_json("/v1/pools", &pool_add_json("pool_ok", Some(POOL_ADDR_A), None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn pools_post_missing_address_and_owner_rejected() {
    let st = app_state(empty_app_cfg()).await;
    let body = serde_json::json!({ "name": "pool_bad", "address": null, "owner": null });
    let resp = routes(false, st).oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("address") && msg.contains("owner"), "unexpected error message: {msg}");
}

#[tokio::test]
async fn pools_post_invalid_address_rejected() {
    let st = app_state(empty_app_cfg()).await;
    let body = pool_add_json("p_bad_addr", Some("not-a-valid-address"), None);
    let resp = routes(false, st).oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("address"));
}

#[tokio::test]
async fn pools_post_invalid_owner_rejected() {
    let st = app_state(empty_app_cfg()).await;
    let body = pool_add_json("p_bad_owner", None, Some("definitely-not-an-address"));
    let resp = routes(false, st).oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("owner"));
}

#[tokio::test]
async fn pools_post_duplicate() {
    let st = app_state(empty_app_cfg()).await;
    let app = routes(false, st);
    let body = pool_add_json("pool_dup", Some(POOL_ADDR_A), None);
    let resp = app.clone().oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let resp = app.oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

// --- DELETE /v1/pools ---

#[tokio::test]
async fn pools_delete_succeeds_when_orphan() {
    let mut cfg = empty_app_cfg();
    let (pn, pc) = sample_snp_pool("p_free");
    cfg.pools.insert(pn, pc);
    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete("/v1/pools/p_free")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn pools_delete_rejected_when_bound() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("p_del");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wname, wcfg) = sample_wallet("pw");
    cfg.wallets.insert(wname.clone(), wcfg);
    let (pname, pcfg) = sample_snp_pool("p_bound");
    cfg.pools.insert(pname.clone(), pcfg);
    cfg.bindings.insert(
        nname,
        NodeBinding {
            wallet: wname,
            pool: Some(pname.clone()),
            enable: false,
            status: BindingStatus::Idle,
        },
    );
    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete(&format!("/v1/pools/{pname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn pools_delete_not_found() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st).oneshot(delete("/v1/pools/no_such_pool")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- POST /v1/bindings ---

#[tokio::test]
async fn bindings_post_succeeds() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("bind");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (bw, bwc) = sample_wallet("bind_w");
    cfg.wallets.insert(bw.clone(), bwc);

    let st = app_state(cfg).await;
    let app = routes(false, st);
    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, &bw, None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn bindings_post_succeeds_with_pool() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("bind_p");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wn, wc) = sample_wallet("bind_p_w");
    cfg.wallets.insert(wn.clone(), wc);
    let (pn, pc) = sample_snp_pool("bind_p_pool");
    cfg.pools.insert(pn.clone(), pc);

    let st = app_state(cfg).await;
    let app = routes(false, st);
    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, &wn, Some(&pn))))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify the pool reference round-trips via GET /v1/bindings.
    let resp = app.oneshot(get("/v1/bindings")).await.unwrap();
    let v = json(resp).await;
    let entry = v["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["node"].as_str() == Some(&nname))
        .expect("binding should be listed");
    assert_eq!(entry["pool"].as_str(), Some(pn.as_str()));
}

#[tokio::test]
async fn bindings_post_duplicate_rejected() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("dup");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (bw, bwc) = sample_wallet("dup_w");
    cfg.wallets.insert(bw.clone(), bwc);

    let st = app_state(cfg).await;
    let app = routes(false, st);
    let body = binding_add_json(&nname, &bw, None);
    let resp = app.clone().oneshot(post_json("/v1/bindings", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);

    let resp = app.oneshot(post_json("/v1/bindings", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("already exists"));
}

#[tokio::test]
async fn bindings_post_missing_refs() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("mr");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (mwn, mwc) = sample_wallet("mr_w");
    cfg.wallets.insert(mwn.clone(), mwc);
    let (pname, pcfg) = sample_snp_pool("mr_p");
    cfg.pools.insert(pname.clone(), pcfg);

    let st = app_state(cfg).await;
    let app = routes(false, st);

    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json("no_such_node", "mr_w", None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("node 'no_such_node' not found"));

    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, "no_wallet", None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("wallet 'no_wallet' not found"));

    let resp = app
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, &mwn, Some("no_pool"))))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("pool 'no_pool' not found"));
}

#[tokio::test]
async fn bindings_post_pool_already_bound() {
    let mut cfg = empty_app_cfg();
    let (n1, n1c) = sample_node_adnl("ab1");
    let (n2, n2c) = sample_node_adnl("ab2");
    cfg.nodes.insert(n1.clone(), n1c);
    cfg.nodes.insert(n2.clone(), n2c);
    let (w, wc) = sample_wallet("ab_w");
    cfg.wallets.insert(w.clone(), wc);
    let (p, pc) = sample_snp_pool("ab_p");
    cfg.pools.insert(p.clone(), pc);
    cfg.bindings.insert(
        n1.clone(),
        NodeBinding {
            wallet: w.clone(),
            pool: Some(p.clone()),
            enable: false,
            status: BindingStatus::Idle,
        },
    );

    let st = app_state(cfg).await;
    let resp = routes(false, st)
        .oneshot(post_json("/v1/bindings", &binding_add_json(&n2, &w, Some(&p))))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("already bound"), "unexpected error message: {msg}");
}

// --- DELETE /v1/bindings ---

#[tokio::test]
async fn bindings_delete_succeeds_when_idle() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("idle_rm");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wn, wc) = sample_wallet("idle_w");
    cfg.wallets.insert(wn.clone(), wc);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding { wallet: wn, pool: None, enable: false, status: BindingStatus::Idle },
    );

    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete(&format!("/v1/bindings/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn bindings_delete_rejected_when_non_idle() {
    let mut cfg = empty_app_cfg();
    let (nname, ncfg) = sample_node_adnl("busy_rm");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wn, wc) = sample_wallet("busy_w");
    cfg.wallets.insert(wn.clone(), wc);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding { wallet: wn, pool: None, enable: false, status: BindingStatus::Validating },
    );

    let st = app_state(cfg).await;
    let resp = routes(false, st).oneshot(delete(&format!("/v1/bindings/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("must be 'idle'"));
}

#[tokio::test]
async fn bindings_delete_not_found() {
    let st = app_state(empty_app_cfg()).await;
    let resp = routes(false, st).oneshot(delete("/v1/bindings/no_such_node")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- Config persisted to disk ---

#[tokio::test]
async fn mutation_persists_config_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg.json");
    let st = app_state_with_path(empty_app_cfg(), path.clone()).await;
    let app = routes(false, st);

    let resp = app
        .clone()
        .oneshot(post_json("/v1/nodes", &node_add_json("persisted_node")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let raw = std::fs::read_to_string(&path).expect("config file written by save_to_file");
    let cfg: AppConfig = serde_json::from_str(&raw).unwrap();
    assert!(cfg.nodes.contains_key("persisted_node"));

    let resp = app.oneshot(delete("/v1/nodes/persisted_node")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let raw = std::fs::read_to_string(&path).unwrap();
    let cfg: AppConfig = serde_json::from_str(&raw).unwrap();
    assert!(!cfg.nodes.contains_key("persisted_node"));
}
