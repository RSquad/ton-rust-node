/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for `v1_pools_handler`, particularly the new TONCore
//! per-slot output. RPC calls in the test environment fail (no live
//! ton-http-api endpoint), so tests assert deterministic outcomes for the
//! "not deployed" path (no address) and the "error" path (RPC unreachable),
//! plus the SNP shape and slot-ordering invariants.
use crate::{
    auth::{jwt::JwtAuth, user_store::UserStore},
    http::http_server_task::*,
    runtime_config::{RuntimeConfig, RuntimeConfigStore},
    task::task_manager::{ServiceTask, TaskController},
};
use axum::body::Body;
use common::{
    app_config::{AppConfig, HttpConfig, PoolConfig, TonCoreInitParams, TonCorePoolConfig},
    snapshot::SnapshotStore,
    task_cancellation::CancellationCtx,
};
use http_body_util::BodyExt;
use std::{collections::HashMap, sync::Arc};
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

fn empty_app_cfg() -> Arc<AppConfig> {
    Arc::new(AppConfig {
        nodes: HashMap::new(),
        wallets: HashMap::new(),
        pools: HashMap::new(),
        bindings: HashMap::new(),
        ton_http_api: Default::default(),
        // Auth disabled — tests target the handler logic directly, not the
        // auth middleware.
        http: HttpConfig { auth: None, ..Default::default() },
        elections: Some(Default::default()),
        voting: None,
        master_wallet: None,
        tick_interval: 30,
        automation: Default::default(),
        log: Some(Default::default()),
    })
}

async fn state_with_pools(pools: HashMap<String, PoolConfig>) -> AppState {
    let mut cfg = (*empty_app_cfg()).clone();
    cfg.pools = pools;
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

#[tokio::test]
async fn pools_empty() {
    let st = state_with_pools(HashMap::new()).await;
    let resp = routes(false, st).oneshot(get("/v1/pools")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"], serde_json::json!([]));
}

#[tokio::test]
async fn pools_snp_shape_preserved() {
    let mut pools = HashMap::new();
    pools.insert(
        "snp1".to_string(),
        PoolConfig::SNP {
            address: Some(
                "-1:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea".into(),
            ),
            owner: Some(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb".into(),
            ),
        },
    );

    let st = state_with_pools(pools).await;
    let resp = routes(false, st).oneshot(get("/v1/pools")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    let result = &v["result"][0];
    assert_eq!(result["name"], "snp1");
    assert_eq!(result["kind"], "SNP");
    assert_eq!(
        result["owner"],
        "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb"
    );
    // SNP must not produce a slots field.
    assert!(result.get("slots").is_none());
}

#[tokio::test]
async fn pools_toncore_slot_with_no_address_is_not_deployed() {
    // Slot configured with params but no address and no binding → pool isn't
    // on-chain yet. Handler should emit a "not deployed" slot entry rather
    // than failing or guessing an address.
    let mut pools = HashMap::new();
    pools.insert(
        "core1".to_string(),
        PoolConfig::TONCore {
            pools: [
                Some(TonCorePoolConfig {
                    address: None,
                    params: Some(TonCoreInitParams {
                        validator_share: 4000,
                        max_nominators: 40,
                        min_validator_stake: 10_000_000_000_000,
                        min_nominator_stake: 10_000_000_000_000,
                    }),
                }),
                None,
            ],
        },
    );

    let st = state_with_pools(pools).await;
    let resp = routes(false, st).oneshot(get("/v1/pools")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    let result = &v["result"][0];
    assert_eq!(result["kind"], "Core");
    let slots = result["slots"].as_array().unwrap();
    // Only the configured (even) slot is reported; the unconfigured odd slot
    // is omitted entirely.
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["slot"], "even");
    assert_eq!(slots[0]["state"], "not deployed");
    assert!(slots[0].get("address").is_none());
    assert!(slots[0].get("balance").is_none());
    assert!(slots[0].get("validator_share").is_none());
}

#[tokio::test]
async fn pools_toncore_both_slots_with_addresses_rpc_unreachable() {
    // Both slots have addresses but no live RPC — handler must gracefully
    // produce two slot entries with state="error" instead of returning 500.
    let mut pools = HashMap::new();
    pools.insert(
        "core2".to_string(),
        PoolConfig::TONCore {
            pools: [
                Some(TonCorePoolConfig {
                    address: Some(
                        "-1:0000000000000000000000000000000000000000000000000000000000000001"
                            .into(),
                    ),
                    params: None,
                }),
                Some(TonCorePoolConfig {
                    address: Some(
                        "-1:0000000000000000000000000000000000000000000000000000000000000002"
                            .into(),
                    ),
                    params: None,
                }),
            ],
        },
    );

    let st = state_with_pools(pools).await;
    let resp = routes(false, st).oneshot(get("/v1/pools")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    let result = &v["result"][0];
    let slots = result["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0]["slot"], "even");
    assert_eq!(slots[1]["slot"], "odd");
    for slot in slots {
        // RPC failed → state encoded into DTO, not bubbled up.
        assert_eq!(slot["state"], "error");
        assert!(slot.get("balance").is_none());
        assert!(slot["address"].is_string());
    }
}

// ---------------------------------------------------------------------------
// POST /v1/pools/core
// ---------------------------------------------------------------------------

fn core_add_body(name: &str, slot: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "slot": slot,
        "validator_share": 4000u16,
    })
}

#[tokio::test]
async fn pools_add_core_creates_new_pool_with_one_slot() {
    let st = state_with_pools(HashMap::new()).await;
    let resp = routes(false, st.clone())
        .oneshot(post_json("/v1/pools/core", &core_add_body("core1", "even")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v = json(resp).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["name"], "core1");

    // Live config now has a TONCore pool with only the even slot populated.
    let cfg = st.runtime_cfg.get();
    let pool = cfg.pools.get("core1").expect("pool inserted");
    match pool {
        PoolConfig::TONCore { pools } => {
            assert!(pools[0].is_some(), "even slot must be present");
            assert!(pools[1].is_none(), "odd slot must remain empty");
            let params = pools[0].as_ref().unwrap().params.as_ref().unwrap();
            assert_eq!(params.validator_share, 4000);
        }
        _ => panic!("expected TONCore pool"),
    }
}

#[tokio::test]
async fn pools_add_core_adds_second_slot_to_existing_pool() {
    let mut pools = HashMap::new();
    pools.insert(
        "core1".to_string(),
        PoolConfig::TONCore {
            pools: [
                Some(TonCorePoolConfig {
                    address: None,
                    params: Some(TonCoreInitParams {
                        validator_share: 4000,
                        max_nominators: 40,
                        min_validator_stake: 10_000_000_000_000,
                        min_nominator_stake: 10_000_000_000_000,
                    }),
                }),
                None,
            ],
        },
    );
    let st = state_with_pools(pools).await;

    let resp = routes(false, st.clone())
        .oneshot(post_json("/v1/pools/core", &core_add_body("core1", "odd")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let cfg = st.runtime_cfg.get();
    match cfg.pools.get("core1").unwrap() {
        PoolConfig::TONCore { pools } => {
            assert!(pools[0].is_some(), "even slot preserved");
            assert!(pools[1].is_some(), "odd slot added");
        }
        _ => panic!("expected TONCore pool"),
    }
}

#[tokio::test]
async fn pools_add_core_rejects_existing_snp_pool() {
    let mut pools = HashMap::new();
    pools.insert("name1".to_string(), PoolConfig::SNP { address: None, owner: None });
    let st = state_with_pools(pools).await;

    let resp = routes(false, st)
        .oneshot(post_json("/v1/pools/core", &core_add_body("name1", "even")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("SNP"));
}

#[tokio::test]
async fn pools_add_core_rejects_already_configured_slot() {
    let mut pools = HashMap::new();
    pools.insert(
        "core1".to_string(),
        PoolConfig::TONCore {
            pools: [Some(TonCorePoolConfig { address: None, params: None }), None],
        },
    );
    let st = state_with_pools(pools).await;

    let resp = routes(false, st)
        .oneshot(post_json("/v1/pools/core", &core_add_body("core1", "even")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("already configured"));
}

#[tokio::test]
async fn pools_add_core_rejects_missing_address_and_share() {
    let st = state_with_pools(HashMap::new()).await;
    // Body has neither `address` nor `validator_share` — must 400.
    let body = serde_json::json!({ "name": "core1", "slot": "even" });
    let resp = routes(false, st).oneshot(post_json("/v1/pools/core", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn pools_add_core_rejects_invalid_slot() {
    let st = state_with_pools(HashMap::new()).await;
    let resp = routes(false, st)
        .oneshot(post_json("/v1/pools/core", &core_add_body("core1", "middle")))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("slot"));
}

#[tokio::test]
async fn pools_add_core_rejects_validator_share_above_100_pct() {
    let st = state_with_pools(HashMap::new()).await;
    // 10001 bp = 100.01% — must be rejected.
    let body = serde_json::json!({
        "name": "core1",
        "slot": "even",
        "validator_share": 10_001u16,
    });
    let resp = routes(false, st).oneshot(post_json("/v1/pools/core", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn pools_add_core_accepts_validator_share_at_100_pct() {
    // Boundary check: 10000 bp = exactly 100% — must be accepted.
    let st = state_with_pools(HashMap::new()).await;
    let body = serde_json::json!({
        "name": "core1",
        "slot": "even",
        "validator_share": 10_000u16,
    });
    let resp = routes(false, st).oneshot(post_json("/v1/pools/core", &body)).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn pools_add_core_rejects_sibling_params_without_validator_share() {
    // Supplying max_nominators / min_*_stake without validator_share would
    // silently discard them (no TonCoreInitParams gets built). Must 400 so
    // the user knows their input was ignored.
    let st = state_with_pools(HashMap::new()).await;
    let body = serde_json::json!({
        "name": "core1",
        "slot": "even",
        "address": "-1:0000000000000000000000000000000000000000000000000000000000000001",
        "max_nominators": 50u16,
    });
    let resp = routes(false, st).oneshot(post_json("/v1/pools/core", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("validator_share"));
}

#[tokio::test]
async fn pools_add_core_rejects_max_nominators_zero() {
    let st = state_with_pools(HashMap::new()).await;
    let body = serde_json::json!({
        "name": "core1",
        "slot": "even",
        "validator_share": 4000u16,
        "max_nominators": 0u16,
    });
    let resp = routes(false, st).oneshot(post_json("/v1/pools/core", &body)).await.unwrap();
    assert_eq!(resp.status(), 400);
    let v = json(resp).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("max_nominators"));
}
