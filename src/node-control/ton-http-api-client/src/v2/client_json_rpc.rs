/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::v2::data_models::{
    BlockIdExt, GetAddressInformationRes, GetBlockHeaderRes, GetExtendedAddressInformationRes,
    GetMasterchainInfoRes, GetWalletInformationRes, RunGetMethodParams, RunGetMethodRes,
};
use anyhow::Context;
use base64::Engine;
use common::{
    app_config::{EndpointTimeouts, FreshnessConfig},
    clock::{Clock, SystemClock},
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};
use ton_block::{ConfigParamEnum, MsgAddressInt, read_boc};
use toncenter_rs::client::{
    ApiClientV2 as TonApiClientV2, ApiKey as TonApiKey, Network as TonNetwork,
};

/// Marker embedded in the aggregated "all endpoints failed" error. Call sites
/// should use [`is_endpoints_unreachable`] rather than matching on it directly.
pub const ENDPOINTS_UNREACHABLE_TAG: &str = "ton-http-api unreachable";

/// Marker embedded in the "all endpoints serving stale chain data" error.
/// Use [`is_endpoints_stale`] rather than matching on it directly.
pub const ENDPOINTS_STALE_TAG: &str = "ton-http-api stale";

/// `true` if `err` (or any source in its chain) is the aggregated
/// "all endpoints failed" error from [`ClientJsonRpc::json_rpc`].
pub fn is_endpoints_unreachable(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.to_string().contains(ENDPOINTS_UNREACHABLE_TAG))
}

/// `true` if `err` is the "all endpoints stale" error from [`ClientJsonRpc::json_rpc`].
pub fn is_endpoints_stale(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.to_string().contains(ENDPOINTS_STALE_TAG))
}

/// Cached freshness signal for an endpoint, populated by lazy probes.
/// `observed_at == 0` means never probed successfully.
#[derive(Default)]
struct EndpointFreshness {
    gen_utime: AtomicU32,
    observed_at: AtomicU64,
}

struct EndpointClient {
    url: String,
    client: TonApiClientV2,
    freshness: EndpointFreshness,
}

pub struct ClientJsonRpc {
    api_key: Option<String>,
    endpoints: Vec<EndpointClient>,
    timeouts: EndpointTimeouts,
    freshness_cfg: FreshnessConfig,
    clock: Arc<dyn Clock>,
}

impl ClientJsonRpc {
    pub fn connect(url: String, api_key: Option<String>) -> anyhow::Result<Self> {
        Self::connect_many(
            vec![(url, None)],
            api_key,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
    }

    /// Builds a failover client from one or more endpoint entries.
    ///
    /// Each entry is a `(url, per_endpoint_api_key)` pair. When the
    /// per-endpoint key is `None`, the `default_api_key` is used instead.
    ///
    /// `timeouts` bounds the wait on every individual endpoint so that an
    /// unreachable ton-http-api cannot stall the daemon: each call is
    /// wrapped in [`tokio::time::timeout`] with budget [`EndpointTimeouts::total`].
    ///
    /// `freshness` governs the lazy `getMasterchainInfo` + `getBlockHeader`
    /// probe that detects endpoints serving stale chain data.
    ///
    /// This constructor is defensive: it trims inputs, drops empty values and
    /// deduplicates URLs while preserving order. Callers should normally pass
    /// pre-normalized values from `TonHttpApiConfig::resolved_endpoints()`,
    /// but this method still tolerates duplicates for safety.
    pub fn connect_many(
        entries: Vec<(String, Option<String>)>,
        default_api_key: Option<String>,
        timeouts: EndpointTimeouts,
        freshness_cfg: FreshnessConfig,
    ) -> anyhow::Result<Self> {
        let mut seen = HashSet::with_capacity(entries.len());
        let mut unique: Vec<(String, Option<String>)> = Vec::with_capacity(entries.len());
        for (url, key) in entries {
            let url_trimmed = url.trim().to_string();
            if url_trimmed.is_empty() {
                continue;
            }
            if !seen.insert(url_trimmed.clone()) {
                continue;
            }
            unique.push((url_trimmed, key));
        }

        if unique.is_empty() {
            anyhow::bail!("No ton-http-api endpoints configured");
        }

        let endpoints = unique
            .into_iter()
            .map(|(url, per_key)| {
                let effective_key = per_key.as_ref().or(default_api_key.as_ref());
                EndpointClient {
                    client: TonApiClientV2::new(
                        TonNetwork::Custom(url.clone()),
                        effective_key.map(|v| TonApiKey::Header(v.to_string())),
                    ),
                    url,
                    freshness: EndpointFreshness::default(),
                }
            })
            .collect::<Vec<_>>();

        Ok(ClientJsonRpc {
            api_key: default_api_key,
            endpoints,
            timeouts,
            freshness_cfg,
            clock: Arc::new(SystemClock),
        })
    }

    #[cfg(test)]
    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    pub fn api_key(&self) -> Option<String> {
        self.api_key.clone()
    }

    pub fn url(&self) -> &str {
        &self.endpoints[0].url
    }

    pub fn urls(&self) -> Vec<String> {
        self.endpoints.iter().map(|e| e.url.clone()).collect()
    }

    /// Executes a JSON-RPC call with priority-based failover across all endpoints.
    ///
    /// Algorithm:
    /// 1. Endpoints are tried in declared config order. The first entry is the primary
    ///    and carries all traffic while healthy; subsequent entries are fallbacks used
    ///    only on error or detected staleness.
    /// 2. Before each attempt the endpoint's freshness is refreshed lazily
    ///    (probe = `getMasterchainInfo` + `getBlockHeader`, rate-limited by
    ///    `freshness_cfg.probe_interval_secs`). Endpoints whose masterchain block view
    ///    is older than `freshness_cfg.max_lag_secs` are skipped. Each individual RPC
    ///    is bounded by [`EndpointTimeouts::total`]; when a probe runs, an attempt may
    ///    therefore consume up to ~3× that budget (2 probe RPCs + 1 business RPC). Once
    ///    an endpoint is known fresh (probe within TTL), only the business RPC runs and
    ///    the attempt is bounded by 1× budget.
    /// 3. On success the response is returned immediately. On total failure the error
    ///    is `ENDPOINTS_STALE_TAG` when every endpoint was skipped because of staleness,
    ///    otherwise `ENDPOINTS_UNREACHABLE_TAG`.
    ///
    /// Operator guidance: list endpoints in trust/freshness order — the most up-to-date
    /// or most-controlled endpoint goes first; lagging or less-trusted endpoints belong
    /// later as fallbacks.
    async fn json_rpc(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let total = self.endpoints.len();
        let request_id = serde_json::json!(uuid::Uuid::new_v4().to_string());
        let per_endpoint_budget = self.timeouts.total();
        let mut failures: Vec<(String, String)> = Vec::with_capacity(total);
        let mut stale_count: usize = 0;

        for idx in 0..total {
            let endpoint = &self.endpoints[idx];

            if self.needs_freshness_refresh(idx)
                && let Err(e) = self.refresh_endpoint_freshness(idx).await
            {
                tracing::debug!(
                    method,
                    endpoint = %endpoint.url,
                    attempt = idx + 1,
                    error = %e,
                    "ton-http-api freshness probe failed"
                );
                failures.push((endpoint.url.clone(), format!("freshness probe failed: {e}")));
                continue;
            }
            if self.is_endpoint_stale(idx) {
                stale_count += 1;
                tracing::debug!(
                    method,
                    endpoint = %endpoint.url,
                    attempt = idx + 1,
                    "ton-http-api endpoint is stale, skipping"
                );
                failures.push((endpoint.url.clone(), "stale chain view".to_string()));
                continue;
            }

            let call = endpoint.client.json_rpc(method, params.clone(), request_id.clone());
            match tokio::time::timeout(per_endpoint_budget, call).await {
                Ok(Ok(response)) => {
                    if idx > 0 {
                        tracing::debug!(
                            method,
                            used_endpoint = %endpoint.url,
                            attempt = idx + 1,
                            "ton-http-api priority failover succeeded"
                        );
                    }
                    return Ok(response);
                }
                Ok(Err(err)) => {
                    let reason = err.to_string();
                    tracing::debug!(
                        method,
                        endpoint = %endpoint.url,
                        attempt = idx + 1,
                        total_attempts = total,
                        error = %reason,
                        "ton-http-api request failed"
                    );
                    failures.push((endpoint.url.clone(), reason));
                }
                Err(_elapsed) => {
                    let reason = format!("timed out after {}s", per_endpoint_budget.as_secs());
                    tracing::debug!(
                        method,
                        endpoint = %endpoint.url,
                        attempt = idx + 1,
                        total_attempts = total,
                        timeout_secs = per_endpoint_budget.as_secs(),
                        "ton-http-api request timed out"
                    );
                    failures.push((endpoint.url.clone(), reason));
                }
            }
        }

        let detail = failures
            .iter()
            .map(|(url, reason)| format!("{}: {}", url, reason))
            .collect::<Vec<_>>()
            .join("; ");

        if stale_count == total {
            tracing::warn!(
                method,
                total_attempts = total,
                failures = %detail,
                "ton-http-api all endpoints serving stale chain data"
            );
            anyhow::bail!("{}: all {} endpoint(s) stale: [{}]", ENDPOINTS_STALE_TAG, total, detail);
        }

        tracing::warn!(
            method,
            total_attempts = total,
            failures = %detail,
            "ton-http-api unreachable on all endpoints"
        );
        anyhow::bail!("{}: tried {} endpoint(s): [{}]", ENDPOINTS_UNREACHABLE_TAG, total, detail)
    }

    /// `true` when the cached freshness is older than the configured probe interval.
    fn needs_freshness_refresh(&self, idx: usize) -> bool {
        let observed_at = self.endpoints[idx].freshness.observed_at.load(Ordering::Relaxed);
        self.clock.now().saturating_sub(observed_at) >= self.freshness_cfg.probe_interval_secs
    }

    /// `observed_at == 0` (never probed) is intentionally treated as fresh: probe failures
    /// are handled by the caller (`json_rpc` continues to the next endpoint), so this branch
    /// only fires when probing is disabled via [`FreshnessConfig::disabled`].
    ///
    /// The `Acquire` load of `observed_at` pairs with the `Release` store in
    /// [`Self::refresh_endpoint_freshness`] so the matching `gen_utime` is guaranteed to be
    /// the one published alongside this `observed_at` value (no torn snapshot).
    fn is_endpoint_stale(&self, idx: usize) -> bool {
        let observed_at = self.endpoints[idx].freshness.observed_at.load(Ordering::Acquire);
        if observed_at == 0 {
            return false;
        }
        let gen_utime = self.endpoints[idx].freshness.gen_utime.load(Ordering::Relaxed);
        self.clock.now().saturating_sub(u64::from(gen_utime)) > self.freshness_cfg.max_lag_secs
    }

    /// Invoke a JSON-RPC method on a single endpoint (no failover), parse the result into `T`.
    async fn call_single_endpoint<T: serde::de::DeserializeOwned>(
        &self,
        idx: usize,
        method: &'static str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        let endpoint = &self.endpoints[idx];
        let budget = self.timeouts.total();
        let request_id = serde_json::json!(uuid::Uuid::new_v4().to_string());
        let call = endpoint.client.json_rpc(method, params, request_id);
        let value = match tokio::time::timeout(budget, call).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => anyhow::bail!("{}: {}", method, e),
            Err(_) => anyhow::bail!("{}: timed out after {}s", method, budget.as_secs()),
        };
        serde_json::from_value::<T>(value).with_context(|| format!("deserialize {method} response"))
    }

    /// Two-step probe (`getMasterchainInfo` → `getBlockHeader`) that yields the tip's `gen_utime`
    /// and stores the snapshot. Both calls must succeed; partial success leaves the cache
    /// untouched so it never reflects half-fresh state.
    async fn refresh_endpoint_freshness(&self, idx: usize) -> anyhow::Result<()> {
        let info: GetMasterchainInfoRes =
            self.call_single_endpoint(idx, "getMasterchainInfo", serde_json::json!({})).await?;
        let block_id_params =
            serde_json::to_value(&info.last).context("serialize BlockIdExt for getBlockHeader")?;
        let header: GetBlockHeaderRes =
            self.call_single_endpoint(idx, "getBlockHeader", block_id_params).await?;

        let now = self.clock.now();
        let endpoint = &self.endpoints[idx];
        // Publish gen_utime first, then observed_at with Release so an Acquire reader of
        // observed_at sees the matching gen_utime (no torn snapshot).
        endpoint.freshness.gen_utime.store(header.gen_utime, Ordering::Relaxed);
        endpoint.freshness.observed_at.store(now, Ordering::Release);
        Ok(())
    }

    pub async fn get_config_param(&self, param_id: u32) -> anyhow::Result<ConfigParamEnum> {
        let json_params: serde_json::Value = serde_json::json!({
            "config_id": param_id,
        });

        let config_info = self
            .json_rpc("getConfigParam", json_params)
            .await
            .with_context(|| format!("getConfigParam({})", param_id))?;

        let b64 = config_info
            .get("config")
            .and_then(|c| c.get("bytes"))
            .or_else(|| {
                config_info.get("result").and_then(|r| r.get("config")).and_then(|c| c.get("bytes"))
            })
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!(r#"missing "config.bytes" string"#))?;

        let boc = base64::engine::general_purpose::STANDARD.decode(b64)?;
        let cell = read_boc(boc)?.withdraw_single_root()?;

        let config_param = ConfigParamEnum::construct_from_cell_and_number(cell, param_id)?;
        Ok(config_param)
    }

    pub async fn run_get_method(
        &self,
        args: &RunGetMethodParams,
    ) -> anyhow::Result<RunGetMethodRes> {
        let json_params = serde_json::json!(args);
        let json_params_str = json_params.to_string();
        let res = self.json_rpc("runGetMethod", json_params).await.map_err(|e| {
            anyhow::anyhow!("Request `runGetMethod({})` return error: {}", json_params_str, e)
        })?;

        let run_get_method_res = serde_json::from_value::<RunGetMethodRes>(res)?;

        Ok(run_get_method_res)
    }

    pub async fn send_boc(&self, boc: &Vec<u8>) -> anyhow::Result<()> {
        let json_params = serde_json::json!({
            "boc": base64::engine::general_purpose::STANDARD.encode(boc)
        });
        let json_params_str = json_params.to_string();
        let _ = self.json_rpc("sendBoc", json_params).await.map_err(|e| {
            anyhow::anyhow!("Request `sendBoc({})` return error: {}", json_params_str, e)
        })?;

        Ok(())
    }

    pub async fn get_extended_address_information(
        &self,
        address: &MsgAddressInt,
    ) -> anyhow::Result<GetExtendedAddressInformationRes> {
        let json_params = serde_json::json!({
            "address": address.to_string(),
        });
        let json_params_str = json_params.to_string();
        let res =
            self.json_rpc("getExtendedAddressInformation", json_params).await.map_err(|e| {
                anyhow::anyhow!(
                    "Request `getExtendedAddressInformation({})` return error: {}",
                    json_params_str,
                    e
                )
            })?;

        let get_extended_address_information_res =
            serde_json::from_value::<GetExtendedAddressInformationRes>(res)?;

        Ok(get_extended_address_information_res)
    }

    pub async fn get_address_information(
        &self,
        address: &MsgAddressInt,
    ) -> anyhow::Result<GetAddressInformationRes> {
        let json_params = serde_json::json!({
            "address": address.to_string(),
        });
        let json_params_str = json_params.to_string();
        let res = self.json_rpc("getAddressInformation", json_params).await.map_err(|e| {
            anyhow::anyhow!(
                "Request `getAddressInformation({})` return error: {}",
                json_params_str,
                e
            )
        })?;
        let address_info = serde_json::from_value::<GetAddressInformationRes>(res)?;
        Ok(address_info)
    }

    pub async fn get_wallet_information(
        &self,
        address: &MsgAddressInt,
    ) -> anyhow::Result<GetWalletInformationRes> {
        let json_params = serde_json::json!({
            "address": address.to_string(),
        });
        let json_params_str = json_params.to_string();
        let res = self.json_rpc("getWalletInformation", json_params).await.map_err(|e| {
            anyhow::anyhow!(
                "Request `getWalletInformation({})` return error: {}",
                json_params_str,
                e
            )
        })?;
        let wallet_info = serde_json::from_value::<GetWalletInformationRes>(res)?;
        Ok(wallet_info)
    }

    pub async fn get_masterchain_info(&self) -> anyhow::Result<GetMasterchainInfoRes> {
        let res = self
            .json_rpc("getMasterchainInfo", serde_json::json!({}))
            .await
            .context("getMasterchainInfo")?;
        Ok(serde_json::from_value::<GetMasterchainInfoRes>(res)?)
    }

    pub async fn get_block_header(
        &self,
        block_id: &BlockIdExt,
    ) -> anyhow::Result<GetBlockHeaderRes> {
        let params = serde_json::to_value(block_id).context("serialize BlockIdExt")?;
        let res = self.json_rpc("getBlockHeader", params).await.context("getBlockHeader")?;
        Ok(serde_json::from_value::<GetBlockHeaderRes>(res)?)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientJsonRpc, is_endpoints_stale, is_endpoints_unreachable};
    use base64::Engine;
    use common::{
        app_config::{EndpointTimeouts, FreshnessConfig},
        clock::MockClock,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn spawn_jsonrpc_ok_server(
        result: serde_json::Value,
        request_count: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind async listener");
        let addr = listener.local_addr().expect("listener local addr");
        let response_body = serde_json::json!({
            "ok": true,
            "jsonrpc": "2.0",
            "result": result,
            "id": "1"
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            request_count.fetch_add(1, Ordering::SeqCst);

            let mut buf = [0_u8; 4096];
            let mut acc = Vec::new();
            loop {
                let n = socket.read(&mut buf).await.expect("read request");
                if n == 0 {
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
                if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            socket.write_all(response.as_bytes()).await.expect("write response");
            socket.shutdown().await.expect("shutdown socket");
        });

        (format!("http://{}", addr), handle)
    }

    async fn spawn_http_500_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind async listener");
        let addr = listener.local_addr().expect("listener local addr");
        let response_body = r#"{"ok":false,"error":"internal error"}"#;
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept connection");

            let mut buf = [0_u8; 4096];
            let mut acc = Vec::new();
            loop {
                let n = socket.read(&mut buf).await.expect("read request");
                if n == 0 {
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
                if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            socket.write_all(response.as_bytes()).await.expect("write response");
            socket.shutdown().await.expect("shutdown socket");
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn json_rpc_failover_uses_second_url_when_first_is_broken() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let (bad_url, bad_server_handle) = spawn_http_500_server().await;
        let (good_url, server_handle) =
            spawn_jsonrpc_ok_server(serde_json::json!({"from":"fallback"}), request_count.clone())
                .await;

        let client = ClientJsonRpc::connect_many(
            vec![(bad_url, None), (good_url, None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::disabled(),
        )
        .expect("client");

        let response = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("json_rpc should fail over to healthy endpoint");

        assert_eq!(response["from"], "fallback");
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            1,
            "healthy endpoint should receive one request"
        );
        bad_server_handle.await.expect("bad server task");
        server_handle.await.expect("server task");
    }

    #[tokio::test]
    async fn json_rpc_priority_uses_first_endpoint_when_healthy() {
        // With priority routing, every call hits the first endpoint when it is healthy;
        // secondaries stay cold. We use the freshness server (multi-connection capable)
        // with probing disabled so only business calls run.
        let s1 = Arc::new(ProbeStats::default());
        let s2 = Arc::new(ProbeStats::default());
        let (first_url, first_handle) = spawn_freshness_server(0, s1.clone()).await;
        let (second_url, second_handle) = spawn_freshness_server(0, s2.clone()).await;

        let client = ClientJsonRpc::connect_many(
            vec![(first_url, None), (second_url, None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::disabled(),
        )
        .expect("client");

        for _ in 0..3 {
            client
                .json_rpc("getAddressInformation", serde_json::json!({"address": "x"}))
                .await
                .expect("request should succeed against primary");
        }

        assert_eq!(s1.other.load(Ordering::Relaxed), 3, "all calls must hit the primary");
        assert_eq!(s2.other.load(Ordering::Relaxed), 0, "secondary stays cold");

        first_handle.abort();
        second_handle.abort();
    }

    #[tokio::test]
    async fn json_rpc_all_endpoints_failed_returns_aggregated_error() {
        let (bad_1, bad_1_handle) = spawn_http_500_server().await;
        let (bad_2, bad_2_handle) = spawn_http_500_server().await;

        let client = ClientJsonRpc::connect_many(
            vec![(bad_1.clone(), None), (bad_2.clone(), None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::disabled(),
        )
        .expect("client");

        let err = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect_err("json_rpc should fail when all endpoints are down");
        let err_text = err.to_string();

        assert!(
            err_text.contains("ton-http-api unreachable"),
            "error should identify ton-http-api as the source: {err_text}"
        );
        assert!(
            err_text.contains(&bad_1) && err_text.contains(&bad_2),
            "error should list every attempted endpoint: {err_text}"
        );
        assert!(
            !err_text.contains("Request `getAddressInformation`"),
            "error should not include method wrapper text: {err_text}"
        );
        assert!(
            is_endpoints_unreachable(&err),
            "helper should recognize aggregated unreachable error: {err_text}"
        );

        bad_1_handle.await.expect("bad_1 server task");
        bad_2_handle.await.expect("bad_2 server task");
    }

    /// Spawns a TCP listener that accepts connections but never replies.
    /// Used to simulate an unreachable / blackholed ton-http-api endpoint.
    async fn spawn_blackhole_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind listener");
        let addr = listener.local_addr().expect("listener local addr");
        let handle = tokio::spawn(async move {
            // Accept and hold connections open without responding.
            loop {
                if let Ok((socket, _)) = listener.accept().await {
                    // Park the socket so the request never completes; drop on task abort.
                    tokio::spawn(async move {
                        let _socket = socket;
                        std::future::pending::<()>().await;
                    });
                } else {
                    break;
                }
            }
        });
        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn json_rpc_times_out_unreachable_endpoint() {
        let (dead_url, dead_handle) = spawn_blackhole_server().await;
        let timeouts = EndpointTimeouts {
            connect: Duration::from_millis(100),
            request: Duration::from_millis(200),
        };

        let client = ClientJsonRpc::connect_many(
            vec![(dead_url.clone(), None)],
            None,
            timeouts,
            FreshnessConfig::disabled(),
        )
        .expect("client");

        let start = std::time::Instant::now();
        let err = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect_err("json_rpc should fail when endpoint never replies");
        let elapsed = start.elapsed();

        let err_text = err.to_string();
        assert!(
            err_text.contains(&dead_url) && err_text.contains("timed out"),
            "error should report timeout for dead endpoint: {err_text}"
        );
        // Per-endpoint budget is 300ms; allow generous slack for slow CI.
        assert!(
            elapsed < Duration::from_secs(2),
            "single dead endpoint must not stall: elapsed={elapsed:?}"
        );

        dead_handle.abort();
    }

    #[tokio::test]
    async fn json_rpc_total_time_bounded_with_n_dead_endpoints() {
        let (a_url, a_handle) = spawn_blackhole_server().await;
        let (b_url, b_handle) = spawn_blackhole_server().await;
        let (c_url, c_handle) = spawn_blackhole_server().await;
        let timeouts = EndpointTimeouts {
            connect: Duration::from_millis(50),
            request: Duration::from_millis(150),
        };

        let client = ClientJsonRpc::connect_many(
            vec![(a_url.clone(), None), (b_url.clone(), None), (c_url.clone(), None)],
            None,
            timeouts,
            FreshnessConfig::disabled(),
        )
        .expect("client");

        let start = std::time::Instant::now();
        let err = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect_err("all endpoints dead should produce aggregated error");
        let elapsed = start.elapsed();

        let err_text = err.to_string();
        assert!(err_text.contains(&a_url), "expected error to list endpoint A: {err_text}");
        assert!(err_text.contains(&b_url), "expected error to list endpoint B: {err_text}");
        assert!(err_text.contains(&c_url), "expected error to list endpoint C: {err_text}");
        // Budget is 3 × 200ms = 600ms; allow generous slack for CI.
        assert!(
            elapsed < Duration::from_secs(3),
            "total wait must stay bounded across N dead endpoints: elapsed={elapsed:?}"
        );

        a_handle.abort();
        b_handle.abort();
        c_handle.abort();
    }

    // ---- Freshness probe tests ----

    #[derive(Default)]
    struct ProbeStats {
        masterchain_info: AtomicUsize,
        block_header: AtomicUsize,
        other: AtomicUsize,
    }

    /// Spawn a mock server that accepts multiple connections and dispatches responses by the
    /// `"method":...` substring in the request body. Returns the URL and an abort handle.
    ///
    /// - `getMasterchainInfo` → a fixed `last` BlockIdExt.
    /// - `getBlockHeader` → header with the provided `gen_utime`.
    /// - everything else → `{"from":"business"}`.
    ///
    /// Tests must avoid placing the probe method names into business `params` — the
    /// dispatcher is substring-based and would misclassify (no business call uses
    /// these method names today).
    async fn spawn_freshness_server(
        gen_utime: u32,
        stats: Arc<ProbeStats>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let stats = stats.clone();
                tokio::spawn(async move {
                    let mut acc = Vec::with_capacity(4096);
                    let mut buf = [0_u8; 4096];
                    loop {
                        let n = match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        acc.extend_from_slice(&buf[..n]);
                        let s = std::str::from_utf8(&acc).unwrap_or("");
                        if s.contains("\"method\"") {
                            break;
                        }
                        if acc.len() > 8192 {
                            break;
                        }
                    }
                    let body_str = std::str::from_utf8(&acc).unwrap_or("");
                    let zero_hash = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
                    let result = if body_str.contains("\"getMasterchainInfo\"") {
                        stats.masterchain_info.fetch_add(1, Ordering::Relaxed);
                        serde_json::json!({
                            "@type": "blocks.masterchainInfo",
                            "last": {
                                "@type": "ton.blockIdExt",
                                "workchain": -1_i32,
                                "shard": "-9223372036854775808",
                                "seqno": 100_u64,
                                "root_hash": zero_hash,
                                "file_hash": zero_hash,
                            }
                        })
                    } else if body_str.contains("\"getBlockHeader\"") {
                        stats.block_header.fetch_add(1, Ordering::Relaxed);
                        serde_json::json!({
                            "@type": "blocks.header",
                            "id": { "workchain": -1, "shard": "-9223372036854775808", "seqno": 100 },
                            "gen_utime": gen_utime,
                        })
                    } else {
                        stats.other.fetch_add(1, Ordering::Relaxed);
                        serde_json::json!({"from": "business"})
                    };

                    let body = serde_json::json!({
                        "ok": true,
                        "jsonrpc": "2.0",
                        "result": result,
                        "id": "1"
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{}", addr), handle)
    }

    /// Both probe calls (`getMasterchainInfo`, `getBlockHeader`) return HTTP 500;
    /// any other method gets the canned business response. Useful for testing how
    /// `json_rpc` reacts when a probe cannot succeed against an endpoint.
    async fn spawn_probe_failing_server(
        stats: Arc<ProbeStats>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let stats = stats.clone();
                tokio::spawn(async move {
                    let mut acc = Vec::with_capacity(4096);
                    let mut buf = [0_u8; 4096];
                    loop {
                        let n = match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        acc.extend_from_slice(&buf[..n]);
                        if std::str::from_utf8(&acc).unwrap_or("").contains("\"method\"") {
                            break;
                        }
                        if acc.len() > 8192 {
                            break;
                        }
                    }
                    let body_str = std::str::from_utf8(&acc).unwrap_or("");
                    let is_probe = body_str.contains("\"getMasterchainInfo\"")
                        || body_str.contains("\"getBlockHeader\"");
                    if is_probe {
                        if body_str.contains("\"getMasterchainInfo\"") {
                            stats.masterchain_info.fetch_add(1, Ordering::Relaxed);
                        } else {
                            stats.block_header.fetch_add(1, Ordering::Relaxed);
                        }
                        let body = r#"{"ok":false,"error":"down"}"#;
                        let response = format!(
                            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    } else {
                        stats.other.fetch_add(1, Ordering::Relaxed);
                        let result = serde_json::json!({"from": "business"});
                        let body = serde_json::json!({
                            "ok": true,
                            "jsonrpc": "2.0",
                            "result": result,
                            "id": "1"
                        })
                        .to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{}", addr), handle)
    }

    /// `MockClock` returning a wall-clock time consistent with the block's `gen_utime`
    /// so freshness math is deterministic. `chain_now` is the simulated chain head time.
    fn mock_clock_at(chain_now: u32) -> MockClock {
        MockClock::new(u64::from(chain_now))
    }

    #[tokio::test]
    async fn probe_updates_freshness_and_endpoint_serves_business_request() {
        let stats = Arc::new(ProbeStats::default());
        // gen_utime equal to the clock → 0s of lag → fresh.
        let chain_now: u32 = 1_700_000_000;
        let (url, handle) = spawn_freshness_server(chain_now, stats.clone()).await;

        let mut client = ClientJsonRpc::connect_many(
            vec![(url, None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(mock_clock_at(chain_now)));

        let resp = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("business call should succeed");

        assert_eq!(resp["from"], "business");
        assert_eq!(stats.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(stats.block_header.load(Ordering::Relaxed), 1);
        assert_eq!(stats.other.load(Ordering::Relaxed), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn all_endpoints_stale_returns_dedicated_error() {
        let s1 = Arc::new(ProbeStats::default());
        let s2 = Arc::new(ProbeStats::default());
        let chain_now: u32 = 1_700_000_000;
        // gen_utime 5 minutes behind the clock → stale (default max_lag_secs=60).
        let (u1, h1) = spawn_freshness_server(chain_now - 300, s1.clone()).await;
        let (u2, h2) = spawn_freshness_server(chain_now - 300, s2.clone()).await;

        let mut client = ClientJsonRpc::connect_many(
            vec![(u1.clone(), None), (u2.clone(), None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(mock_clock_at(chain_now)));

        let err = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect_err("should fail with ENDPOINTS_STALE_TAG");

        assert!(is_endpoints_stale(&err), "expected stale error: {err}");
        assert!(!is_endpoints_unreachable(&err), "should NOT be unreachable: {err}");
        let s = err.to_string();
        assert!(s.contains("stale chain view"), "error should mention stale: {s}");

        // Both endpoints were fully probed (both probe RPCs); no business calls reached either.
        assert_eq!(s1.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(s1.block_header.load(Ordering::Relaxed), 1);
        assert_eq!(s2.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(s2.block_header.load(Ordering::Relaxed), 1);
        assert_eq!(s1.other.load(Ordering::Relaxed) + s2.other.load(Ordering::Relaxed), 0);

        h1.abort();
        h2.abort();
    }

    #[tokio::test]
    async fn stale_endpoint_skipped_and_fresh_endpoint_used() {
        let s_stale = Arc::new(ProbeStats::default());
        let s_fresh = Arc::new(ProbeStats::default());
        let chain_now: u32 = 1_700_000_000;
        let (u_stale, h_stale) = spawn_freshness_server(chain_now - 300, s_stale.clone()).await;
        let (u_fresh, h_fresh) = spawn_freshness_server(chain_now, s_fresh.clone()).await;

        let mut client = ClientJsonRpc::connect_many(
            vec![(u_stale.clone(), None), (u_fresh.clone(), None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(mock_clock_at(chain_now)));

        let resp = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("fresh endpoint should serve the request");

        assert_eq!(resp["from"], "business");
        // Stale endpoint received a probe but no business call.
        assert_eq!(s_stale.other.load(Ordering::Relaxed), 0);
        // Fresh endpoint received a probe + the business call.
        assert_eq!(s_fresh.other.load(Ordering::Relaxed), 1);

        h_stale.abort();
        h_fresh.abort();
    }

    #[tokio::test]
    async fn lazy_probe_within_ttl_skips_reprobe() {
        let stats = Arc::new(ProbeStats::default());
        let chain_now: u32 = 1_700_000_000;
        let (url, handle) = spawn_freshness_server(chain_now, stats.clone()).await;

        let clock = mock_clock_at(chain_now);
        let mut client = ClientJsonRpc::connect_many(
            vec![(url, None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(clock.clone()));

        client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("first business call");
        // Within TTL (default 30s).
        clock.advance(10);
        client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("second business call");

        // Probe ran once, business calls ran twice.
        assert_eq!(stats.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(stats.block_header.load(Ordering::Relaxed), 1);
        assert_eq!(stats.other.load(Ordering::Relaxed), 2);

        handle.abort();
    }

    #[tokio::test]
    async fn lazy_probe_after_ttl_reprobes() {
        let stats = Arc::new(ProbeStats::default());
        let chain_now: u32 = 1_700_000_000;
        let (url, handle) = spawn_freshness_server(chain_now, stats.clone()).await;

        let clock = mock_clock_at(chain_now);
        let mut client = ClientJsonRpc::connect_many(
            vec![(url, None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(clock.clone()));

        client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("first business call");
        // Beyond TTL (default 30s).
        clock.advance(31);
        client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("second business call");

        assert_eq!(stats.masterchain_info.load(Ordering::Relaxed), 2);
        assert_eq!(stats.block_header.load(Ordering::Relaxed), 2);
        assert_eq!(stats.other.load(Ordering::Relaxed), 2);

        handle.abort();
    }

    #[tokio::test]
    async fn probe_failure_skips_endpoint_and_fails_over() {
        let s_bad = Arc::new(ProbeStats::default());
        let s_good = Arc::new(ProbeStats::default());
        let chain_now: u32 = 1_700_000_000;
        let (u_bad, h_bad) = spawn_probe_failing_server(s_bad.clone()).await;
        let (u_good, h_good) = spawn_freshness_server(chain_now, s_good.clone()).await;

        let mut client = ClientJsonRpc::connect_many(
            vec![(u_bad.clone(), None), (u_good.clone(), None)],
            None,
            EndpointTimeouts::default(),
            FreshnessConfig::default(),
        )
        .expect("client");
        client.set_clock(Arc::new(mock_clock_at(chain_now)));

        let resp = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("fail over to good endpoint");

        assert_eq!(resp["from"], "business");
        // Bad endpoint: probe attempted (500), no business call.
        assert_eq!(s_bad.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(s_bad.other.load(Ordering::Relaxed), 0);
        // Good endpoint served probe + business call.
        assert_eq!(s_good.masterchain_info.load(Ordering::Relaxed), 1);
        assert_eq!(s_good.block_header.load(Ordering::Relaxed), 1);
        assert_eq!(s_good.other.load(Ordering::Relaxed), 1);

        h_bad.abort();
        h_good.abort();
    }
}
