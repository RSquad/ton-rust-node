/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::v2::data_models::{
    GetAddressInformationRes, GetExtendedAddressInformationRes, GetWalletInformationRes,
    RunGetMethodParams, RunGetMethodRes,
};
use anyhow::Context;
use base64::Engine;
use std::{
    collections::HashSet,
    sync::atomic::{AtomicUsize, Ordering},
};
use ton_block::{ConfigParamEnum, MsgAddressInt, read_boc};
use toncenter_rs::client::{
    ApiClientV2 as TonApiClientV2, ApiKey as TonApiKey, Network as TonNetwork,
};

struct EndpointClient {
    url: String,
    client: TonApiClientV2,
}

pub struct ClientJsonRpc {
    api_key: Option<String>,
    endpoints: Vec<EndpointClient>,
    rr_cursor: AtomicUsize,
}

impl ClientJsonRpc {
    pub fn connect(url: String, api_key: Option<String>) -> anyhow::Result<Self> {
        Self::connect_many(vec![(url, None)], api_key)
    }

    /// Builds a failover client from one or more endpoint entries.
    ///
    /// Each entry is a `(url, per_endpoint_api_key)` pair. When the
    /// per-endpoint key is `None`, the `default_api_key` is used instead.
    ///
    /// This constructor is defensive: it trims inputs, drops empty values and
    /// deduplicates URLs while preserving order. Callers should normally pass
    /// pre-normalized values from `TonHttpApiConfig::resolved_endpoints()`,
    /// but this method still tolerates duplicates for safety.
    pub fn connect_many(
        entries: Vec<(String, Option<String>)>,
        default_api_key: Option<String>,
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
                }
            })
            .collect::<Vec<_>>();

        Ok(ClientJsonRpc { api_key: default_api_key, endpoints, rr_cursor: AtomicUsize::new(0) })
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

    /// Executes a JSON-RPC call with round-robin failover across all endpoints.
    ///
    /// Algorithm:
    /// 1. An atomic cursor picks a per-request start endpoint so that
    ///    successive calls are spread across endpoints in round-robin order.
    /// 2. Starting from that endpoint, each endpoint is tried once in
    ///    cyclic order until one succeeds or all have been exhausted.
    /// 3. On success the response is returned immediately; on total failure
    ///    the last error is propagated.
    async fn json_rpc(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let total = self.endpoints.len();
        let start = self.rr_cursor.fetch_add(1, Ordering::Relaxed) % total;
        let request_id = serde_json::json!(uuid::Uuid::new_v4().to_string());
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..total {
            let idx = (start + attempt) % total;
            let endpoint = &self.endpoints[idx];
            match endpoint.client.json_rpc(method, params.clone(), request_id.clone()).await {
                Ok(response) => {
                    if attempt > 0 {
                        tracing::debug!(
                            method,
                            used_endpoint = %endpoint.url,
                            attempt = attempt + 1,
                            "ton-http-api failover succeeded"
                        );
                    }
                    return Ok(response);
                }
                Err(err) => {
                    tracing::debug!(
                        method,
                        endpoint = %endpoint.url,
                        attempt = attempt + 1,
                        total_attempts = total,
                        error = %err,
                        "ton-http-api request failed"
                    );
                    last_error = Some(anyhow::Error::from(err));
                }
            }
        }

        if let Some(err) = last_error {
            Err(err.context(format!("all endpoints ({}) failed", total)))
        } else {
            anyhow::bail!("request failed")
        }
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

    /// Global `max_stake_factor` from config param 17 (raw fixed-point, multiplier ×65536).
    pub async fn network_max_stake_factor_raw(&self) -> anyhow::Result<u32> {
        match self.get_config_param(17).await? {
            ConfigParamEnum::ConfigParam17(c) => Ok(c.max_stake_factor),
            _ => anyhow::bail!("expected config param 17 (stakes config)"),
        }
    }

    /// Same as [`Self::network_max_stake_factor_raw`], as float multiplier (e.g. `3.0`).
    pub async fn network_max_stake_factor_multiplier(&self) -> anyhow::Result<f32> {
        let raw = self.network_max_stake_factor_raw().await?;
        Ok(common::ton_utils::max_stake_factor_raw_to_multiplier(raw))
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
}

#[cfg(test)]
mod tests {
    use super::ClientJsonRpc;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
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

        let client = ClientJsonRpc::connect_many(vec![(bad_url, None), (good_url, None)], None)
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
    async fn json_rpc_round_robin_starts_from_first_endpoint() {
        let first_count = Arc::new(AtomicUsize::new(0));
        let second_count = Arc::new(AtomicUsize::new(0));
        let (first_url, first_handle) =
            spawn_jsonrpc_ok_server(serde_json::json!({"from":"first"}), first_count.clone()).await;
        let (second_url, second_handle) =
            spawn_jsonrpc_ok_server(serde_json::json!({"from":"second"}), second_count.clone())
                .await;

        let client = ClientJsonRpc::connect_many(vec![(first_url, None), (second_url, None)], None)
            .expect("client");

        let first_response = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("first request should succeed");
        let second_response = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect("second request should succeed");

        assert_eq!(first_response["from"], "first");
        assert_eq!(second_response["from"], "second");
        assert_eq!(first_count.load(Ordering::SeqCst), 1, "first endpoint request count");
        assert_eq!(second_count.load(Ordering::SeqCst), 1, "second endpoint request count");

        first_handle.await.expect("first server task");
        second_handle.await.expect("second server task");
    }

    #[tokio::test]
    async fn json_rpc_all_endpoints_failed_returns_last_error_only() {
        let (bad_1, bad_1_handle) = spawn_http_500_server().await;
        let (bad_2, bad_2_handle) = spawn_http_500_server().await;

        let client =
            ClientJsonRpc::connect_many(vec![(bad_1, None), (bad_2, None)], None).expect("client");

        let err = client
            .json_rpc("getAddressInformation", serde_json::json!({"address":"x"}))
            .await
            .expect_err("json_rpc should fail when all endpoints are down");
        let err_text = err.to_string();

        assert!(
            !err_text.contains("failed on all ton-http-api endpoints"),
            "error should not contain aggregated wrapper message"
        );
        assert!(
            !err_text.contains("Request `getAddressInformation`"),
            "error should not include method wrapper text"
        );

        bad_1_handle.await.expect("bad_1 server task");
        bad_2_handle.await.expect("bad_2 server task");
    }
}
