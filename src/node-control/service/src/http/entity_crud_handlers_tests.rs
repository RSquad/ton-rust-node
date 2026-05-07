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
use base64::Engine;
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
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tower::ServiceExt;

struct Noop;

#[async_trait::async_trait]
impl ServiceTask for Noop {
    async fn run(&self, ctx: CancellationCtx, _: Arc<AppConfig>) -> anyhow::Result<()> {
        let mut c = ctx.subscribe();
        let _ = c.changed().await;
        Ok(())
    }
}

const TEST_JWT_SECRET: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="; // [42u8; 32]

/// ADNL server key type id (must match `config_handlers::ADNL_PUBKEY_TYPE_ID`).
const ADNL_PUBKEY_TYPE_ID: i32 = 1209251014;

fn empty_app_cfg() -> Arc<AppConfig> {
    Arc::new(AppConfig {
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
        log: Some(Default::default()),
    })
}

fn valid_control_server_pubkey_b64() -> String {
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

async fn app_state(cfg: Arc<AppConfig>, config_path: Option<PathBuf>) -> AppState {
    let rt: Arc<RuntimeConfigStore> = match config_path {
        Some(p) => Arc::new(RuntimeConfigStore::from_app_config_with_path(
            cfg,
            p.to_string_lossy().into_owned(),
        )),
        None => Arc::new(RuntimeConfigStore::from_app_config(cfg)),
    };
    let jwt_auth = Arc::new(JwtAuth::new(None, Some(TEST_JWT_SECRET)).await.unwrap());
    AppState {
        store: Arc::new(SnapshotStore::new()),
        runtime_cfg: rt.clone(),
        elections_task: Arc::new(TaskController::new("elections", Noop, rt.clone())),
        jwt_auth,
        user_store: Arc::new(UserStore::new(rt as Arc<dyn RuntimeConfig>)),
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(Default::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
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
async fn nodes_post_happy_path() {
    let st = app_state(empty_app_cfg(), None).await;
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
    let st = app_state(empty_app_cfg(), None).await;
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
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);
    let body = serde_json::json!({
        "name": "n_bad_pk",
        "control_server_endpoint": "127.0.0.1:1",
        "control_server_pubkey": "not-valid-base64!!!",
        "control_client_secret": "sec",
    });
    let resp = app.oneshot(post_json("/v1/nodes", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("base64"));
}

#[tokio::test]
async fn nodes_delete_happy_and_not_found() {
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);
    let body = node_add_json("to_rm");
    let resp = app.clone().oneshot(post_json("/v1/nodes", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);

    let resp = app.clone().oneshot(delete("/v1/nodes/to_rm")).await.unwrap();
    assert_eq!(resp.status(), 200);

    let resp = app.oneshot(delete("/v1/nodes/to_rm")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn nodes_delete_referenced_by_binding() {
    let mut cfg = (*empty_app_cfg()).clone();
    let (nname, ncfg) = sample_node_adnl("bound");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (wname, wcfg) = sample_wallet("bw");
    cfg.wallets.insert(wname.clone(), wcfg);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding { wallet: wname, pool: None, enable: false, status: BindingStatus::Idle },
    );
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete(&format!("/v1/nodes/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("binding"));
}

// --- POST/DELETE /v1/wallets ---

#[tokio::test]
async fn wallets_post_happy_path() {
    let st = app_state(empty_app_cfg(), None).await;
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
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);
    let body = wallet_add_json("master_wallet");
    let resp = app.oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("reserved"));
}

#[tokio::test]
async fn wallets_post_duplicate() {
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);
    let body = wallet_add_json("w_dup");
    let resp = app.clone().oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let resp = app.oneshot(post_json("/v1/wallets", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn wallets_delete_happy_and_referenced_by_binding() {
    let mut cfg = (*empty_app_cfg()).clone();
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
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);

    let resp = app.clone().oneshot(delete(&format!("/v1/wallets/{wname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("binding"));

    cfg = (*empty_app_cfg()).clone();
    let (wn, wc) = sample_wallet("w_free");
    cfg.wallets.insert(wn, wc);
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete("/v1/wallets/w_free")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn wallets_delete_not_found() {
    let st = app_state(empty_app_cfg(), None).await;
    let resp = routes(false, st).oneshot(delete("/v1/wallets/no_such_wallet")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- POST/DELETE /v1/pools (SNP) ---

#[tokio::test]
async fn pools_post_happy_and_missing_address_owner() {
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/v1/pools",
            &pool_add_json(
                "pool_ok",
                Some("-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea"),
                None,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body = serde_json::json!({ "name": "pool_bad", "address": null, "owner": null });
    let resp = app.oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("address") && msg.contains("owner"), "unexpected error message: {msg}");
}

#[tokio::test]
async fn pools_post_duplicate() {
    let st = app_state(empty_app_cfg(), None).await;
    let app = routes(false, st);
    let body = pool_add_json(
        "pool_dup",
        Some("-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea"),
        None,
    );
    let resp = app.clone().oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let resp = app.oneshot(post_json("/v1/pools", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn pools_delete_happy_and_referenced_by_binding() {
    let mut cfg = (*empty_app_cfg()).clone();
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
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);

    let resp = app.clone().oneshot(delete(&format!("/v1/pools/{pname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);

    let mut cfg = (*empty_app_cfg()).clone();
    let (pn, pc) = sample_snp_pool("p_free");
    cfg.pools.insert(pn, pc);
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete("/v1/pools/p_free")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn pools_delete_not_found() {
    let st = app_state(empty_app_cfg(), None).await;
    let resp = routes(false, st).oneshot(delete("/v1/pools/no_such_pool")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- POST/DELETE /v1/bindings ---

#[tokio::test]
async fn bindings_post_happy_path_and_duplicate() {
    let mut cfg = (*empty_app_cfg()).clone();
    let (nname, ncfg) = sample_node_adnl("bind");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (bw, bwc) = sample_wallet("bind_w");
    cfg.wallets.insert(bw.clone(), bwc);

    let st = app_state(Arc::new(cfg), None).await;
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
    let mut cfg = (*empty_app_cfg()).clone();
    let (nname, ncfg) = sample_node_adnl("mr");
    cfg.nodes.insert(nname.clone(), ncfg);
    let (mwn, mwc) = sample_wallet("mr_w");
    cfg.wallets.insert(mwn.clone(), mwc);
    let (pname, pcfg) = sample_snp_pool("mr_p");
    cfg.pools.insert(pname.clone(), pcfg);

    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);

    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json("no_such_node", "mr_w", None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("node"));

    let resp = app
        .clone()
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, "no_wallet", None)))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("wallet"));

    let resp = app
        .oneshot(post_json("/v1/bindings", &binding_add_json(&nname, &mwn, Some("no_pool"))))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("pool"));
}

#[tokio::test]
async fn bindings_delete_happy_and_non_idle() {
    let mut cfg = (*empty_app_cfg()).clone();
    let (nname, ncfg) = sample_node_adnl("idle_rm");
    cfg.nodes.insert(nname.clone(), ncfg.clone());
    let (idle_wn, idle_wc) = sample_wallet("idle_w");
    cfg.wallets.insert(idle_wn.clone(), idle_wc);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding { wallet: idle_wn, pool: None, enable: false, status: BindingStatus::Idle },
    );

    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete(&format!("/v1/bindings/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 200);

    let mut cfg = (*empty_app_cfg()).clone();
    cfg.nodes.insert(nname.clone(), ncfg);
    let (busy_wn, busy_wc) = sample_wallet("busy_w");
    cfg.wallets.insert(busy_wn.clone(), busy_wc);
    cfg.bindings.insert(
        nname.clone(),
        NodeBinding {
            wallet: busy_wn,
            pool: None,
            enable: false,
            status: BindingStatus::Validating,
        },
    );
    let st = app_state(Arc::new(cfg), None).await;
    let app = routes(false, st);
    let resp = app.oneshot(delete(&format!("/v1/bindings/{nname}"))).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("idle"));
}

#[tokio::test]
async fn bindings_delete_not_found() {
    let st = app_state(empty_app_cfg(), None).await;
    let resp = routes(false, st).oneshot(delete("/v1/bindings/no_such_node")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// --- Config persisted to disk ---

#[tokio::test]
async fn mutation_persists_config_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg.json");
    let st = app_state(empty_app_cfg(), Some(path.clone())).await;
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
