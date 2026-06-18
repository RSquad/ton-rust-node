/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    config::JsonRpcServerConfig,
    confirmed_blocks::ConfirmedBlockEvent,
    engine_traits::{EngineOperations, Stoppable},
};
use std::{
    collections::HashMap,
    convert::Infallible,
    error::Error,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use ton_block::{base64_encode, error, Result};
use warp::{Filter, Reply};

mod handlers;
mod serializers;
mod token;
mod wallets;

/// Maximum size of an incoming JSON-RPC / REST request body, in bytes.
/// This bounds the size of any BOC accepted via the public API.
const MAX_BODY_SIZE: u64 = 16 << 20;
const SSE_PENDING_EVENT_CAPACITY: usize = 16;

pub struct RpcServer {
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

impl RpcServer {
    pub async fn start(
        conf: JsonRpcServerConfig,
        engine: Arc<dyn EngineOperations>,
    ) -> Result<Self> {
        log::info!("start rpc server!");
        let listener = tokio::net::TcpListener::bind(conf.address).await?;
        Self::start_with_listener(listener, engine).await
    }

    async fn start_with_listener(
        listener: tokio::net::TcpListener,
        engine: Arc<dyn EngineOperations>,
    ) -> Result<Self> {
        let wallet_library = Arc::new(wallets::WalletLibrary::new()?);
        let ctx = Ctx { engine, wallet_library };
        let routes = build_routes(ctx);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            warp::serve(routes)
                .incoming(listener)
                .graceful(async move {
                    rx.await.ok();
                })
                .run()
                .await;
        });
        Ok(Self { shutdown: tx, join })
    }
}

#[async_trait::async_trait]
impl Stoppable for RpcServer {
    fn name(&self) -> &'static str {
        "rpcserver"
    }
    async fn shutdown(self: Box<Self>) {
        self.shutdown.send(()).ok();
        self.join.await.ok();
    }
}

#[derive(Clone)]
struct Ctx {
    engine: Arc<dyn EngineOperations>,
    wallet_library: Arc<wallets::WalletLibrary>,
}

impl Ctx {
    async fn is_testnet(&self) -> bool {
        if let Ok(state) = self.engine.load_last_applied_mc_state().await {
            if let Ok(st) = state.state() {
                return st.global_id() < 0;
            }
        }
        true
    }
}

type JsonResult = Result<serde_json::Value>;
type RestFilter = warp::filters::BoxedFilter<(warp::reply::Response,)>;

#[async_trait::async_trait]
trait JsonRpcHandler: Send + Sync {
    async fn handle(&self, params: serde_json::Value, ctx: Ctx) -> JsonResult;
}

#[async_trait::async_trait]
impl<P> JsonRpcHandler for Arc<dyn RpcHandler<P>>
where
    P: serde::de::DeserializeOwned + Send + Sync,
{
    async fn handle(&self, params: serde_json::Value, ctx: Ctx) -> JsonResult {
        let params: P = serde_json::from_value(params)
            .map_err(|e| ApiError::bad_request(format!("Invalid params: {e}")))?;
        self.invoke(params, ctx).await
    }
}

#[async_trait::async_trait]
trait RpcHandler<P>: Send + Sync {
    async fn invoke(&self, params: P, ctx: Ctx) -> JsonResult;
}

#[async_trait::async_trait]
impl<F, P, X> RpcHandler<P> for F
where
    F: Fn(P, Ctx) -> X + Send + Sync,
    P: Send + 'static,
    X: Future<Output = JsonResult> + Send,
{
    async fn invoke(&self, params: P, ctx: Ctx) -> JsonResult {
        (self)(params, ctx).await
    }
}

struct RpcRegistryBuilder {
    json: HashMap<String, Box<dyn JsonRpcHandler>>,
    rest: Vec<RestFilter>,
    ctx: Ctx,
}

impl RpcRegistryBuilder {
    pub(crate) fn add_jsonrpc<T>(
        &mut self,
        name: &'static str,
        handler: impl RpcHandler<T> + 'static,
        get_or_post: bool,
    ) where
        T: serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        let handler: Arc<dyn RpcHandler<T>> = Arc::new(handler);
        self.json.insert(name.to_string(), Box::new(handler.clone()));
        let handler = move |params: T, ctx: Ctx| {
            let handler = handler.clone();
            async move {
                match handler.invoke(params, ctx).await {
                    Ok(val) => Ok::<_, warp::Rejection>(rest_ok(val)),
                    Err(e) => Ok::<_, warp::Rejection>(rest_err(e)),
                }
            }
        };
        let ctx = self.ctx.clone();
        let rest = if get_or_post {
            warp::path(name)
                .and(warp::get())
                .and(warp::query::<T>())
                .and(warp::any().map(move || ctx.clone()))
                .and_then(handler)
                .boxed()
        } else {
            warp::path(name)
                .and(warp::post())
                .and(warp::body::content_length_limit(MAX_BODY_SIZE))
                .and(warp::body::json())
                .and(warp::any().map(move || ctx.clone()))
                .and_then(handler)
                .boxed()
        };
        self.rest.push(rest);
    }
}

#[derive(Clone)]
struct RpcRegistry {
    json: Arc<HashMap<String, Box<dyn JsonRpcHandler>>>,
    ctx: Ctx,
}

impl RpcRegistry {
    fn with_context(ctx: Ctx) -> (Self, RestFilter) {
        let mut builder = RpcRegistryBuilder { json: HashMap::new(), rest: Vec::new(), ctx };
        handlers::register(&mut builder);
        let registry = Self { json: Arc::new(builder.json), ctx: builder.ctx };
        let mut it = builder.rest.into_iter();
        let rest = if let Some(first) = it.next() {
            it.fold(first, |acc, f| acc.or(f).unify().boxed())
        } else {
            warp::any().and_then(|| async { Err(warp::reject::not_found()) }).boxed()
        };
        (registry, rest)
    }
}

// Convert handler result to REST JSON body: {"ok":true, "result":..., "@extra":...}
fn rest_ok(result: serde_json::Value) -> warp::reply::Response {
    warp::reply::json(&serde_json::json!({
        "ok": true,
        "result": result,
        "@extra": handlers::extra(0),
    }))
    .into_response()
}

fn rest_err(err: ton_block::Error) -> warp::reply::Response {
    let err = err.downcast::<ApiError>().unwrap_or_default();
    let body = serde_json::json!({
        "ok": false,
        "error": err.to_string(),
        "code": err.http_status().as_u16(),
        "@extra": handlers::extra(0),
    });
    let mut resp = warp::reply::json(&body).into_response();
    *resp.status_mut() = err.http_status();
    resp
}

async fn root_handler() -> std::result::Result<warp::reply::Response, warp::Rejection> {
    let body = std::str::from_utf8(include_bytes!("static/index.html"))
        .map_err(|_| warp::reject::not_found())?;
    Ok(warp::reply::with_header(
        warp::reply::html(body),
        warp::http::header::CONTENT_TYPE,
        "text/html; charset=utf-8",
    )
    .into_response())
}

async fn openapi_json_handler() -> std::result::Result<warp::reply::Response, warp::Rejection> {
    let body = std::str::from_utf8(include_bytes!("static/openapi.json"))
        .map_err(|_| warp::reject::not_found())?;
    Ok(warp::reply::with_header(
        warp::reply::html(body),
        warp::http::header::CONTENT_TYPE,
        "application/json",
    )
    .into_response())
}

async fn health_handler() -> std::result::Result<warp::reply::Response, warp::Rejection> {
    Ok(warp::reply::with_status("OK", warp::http::StatusCode::OK).into_response())
}

async fn confirmed_block_events_handler(
    query: ConfirmedBlockEventsQuery,
    ctx: Ctx,
) -> std::result::Result<warp::reply::Response, warp::Rejection> {
    let Some(events) = ctx.engine.confirmed_block_events() else {
        return Ok(json_status(
            warp::http::StatusCode::SERVICE_UNAVAILABLE,
            "confirmed block stream is not available",
        ));
    };
    if query.limit == Some(0) {
        return Ok(json_status(
            warp::http::StatusCode::BAD_REQUEST,
            "limit must be greater than 0",
        ));
    }

    let rx = events.subscribe();
    let (sender, stream) = SseEventStream::new();
    tokio::spawn(forward_confirmed_block_sse_events(
        rx,
        sender,
        query.include_data.unwrap_or(true),
        query.limit,
    ));

    let reply = warp::sse::reply(warp::sse::keep_alive().stream(stream));
    let reply = warp::reply::with_header(reply, warp::http::header::CACHE_CONTROL, "no-cache");
    let reply = warp::reply::with_header(reply, "X-Accel-Buffering", "no");
    Ok(reply.into_response())
}

fn json_status(
    status: warp::http::StatusCode,
    message: impl Into<String>,
) -> warp::reply::Response {
    let body = warp::reply::json(&serde_json::json!({
        "ok": false,
        "error": message.into(),
        "code": status.as_u16(),
    }));
    warp::reply::with_status(body, status).into_response()
}

#[derive(serde::Deserialize)]
struct ConfirmedBlockEventsQuery {
    include_data: Option<bool>,
    limit: Option<usize>,
}

async fn forward_confirmed_block_sse_events(
    mut rx: tokio::sync::broadcast::Receiver<ConfirmedBlockEvent>,
    sender: SseEventSender,
    include_data: bool,
    limit: Option<usize>,
) {
    let mut sent = 0usize;
    loop {
        tokio::select! {
            _ = sender.closed() => return,
            result = rx.recv() => {
                match result {
                    Ok(block_event) => {
                        if let Some(event) = confirmed_block_sse_event(block_event, include_data) {
                            if !sender.send(event) {
                                return;
                            }
                            sent += 1;
                            if limit.map_or(false, |limit| sent >= limit) {
                                return;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("confirmed block SSE receiver lagged by {skipped} block id(s)");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct SseEventSender {
    tx: tokio::sync::mpsc::Sender<warp::sse::Event>,
}

struct SseEventStream {
    rx: tokio::sync::mpsc::Receiver<warp::sse::Event>,
}

impl SseEventStream {
    fn new() -> (SseEventSender, Self) {
        let (tx, rx) = tokio::sync::mpsc::channel(SSE_PENDING_EVENT_CAPACITY);
        (SseEventSender { tx }, Self { rx })
    }
}

impl SseEventSender {
    fn send(&self, event: warp::sse::Event) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                log::warn!(
                    "confirmed block SSE client is too slow; closing stream with {SSE_PENDING_EVENT_CAPACITY} pending event(s)"
                );
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    async fn closed(&self) {
        self.tx.closed().await;
    }
}

impl futures::Stream for SseEventStream {
    type Item = std::result::Result<warp::sse::Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|event| event.map(Ok))
    }
}

fn confirmed_block_sse_event(
    event: ConfirmedBlockEvent,
    include_data: bool,
) -> Option<warp::sse::Event> {
    let block_id = event.id;
    if block_id.shard().is_masterchain() {
        return None;
    }
    let event_id = block_id.to_string();
    let mut block = serde_json::json!({
        "@type": "liteServer.blockData",
        "id": serializers::serialize_block_id(&block_id),
    });
    if include_data {
        block["data"] = serde_json::json!(base64_encode(event.data.as_slice()));
    }
    let payload = serde_json::json!({
        "status": "confirmed",
        "block": block,
    });
    let data = serde_json::to_string(&payload)
        .unwrap_or_else(|_| r#"{"status":"confirmed","serialization_error":true}"#.to_string());
    Some(warp::sse::Event::default().event("confirmed_block").id(event_id).data(data))
}

async fn handle_rejection(
    err: warp::Rejection,
) -> std::result::Result<warp::reply::Response, std::convert::Infallible> {
    let (code, message) = if err.is_not_found() {
        (warp::http::StatusCode::NOT_FOUND, "Not Found".to_string())
    } else if let Some(_) = err.find::<warp::reject::UnsupportedMediaType>() {
        let message = "The request's content-type is not supported".to_string();
        (warp::http::StatusCode::BAD_REQUEST, message)
    } else if let Some(_) = err.find::<warp::reject::InvalidQuery>() {
        (warp::http::StatusCode::UNPROCESSABLE_ENTITY, "Invalid query string".to_string())
    } else if let Some(_) = err.find::<warp::reject::MethodNotAllowed>() {
        (warp::http::StatusCode::METHOD_NOT_ALLOWED, "HTTP method not allowed".to_string())
    } else if let Some(_) = err.find::<warp::reject::PayloadTooLarge>() {
        (
            warp::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!("Request body exceeds {MAX_BODY_SIZE}-byte limit"),
        )
    } else if let Some(e) = err.find::<warp::filters::body::BodyDeserializeError>() {
        let message = match e.source() {
            Some(cause) => format!("Invalid JSON body: {cause}"),
            None => "Invalid JSON body".to_string(),
        };
        (warp::http::StatusCode::UNPROCESSABLE_ENTITY, message)
    } else {
        // We should have expected this... Just log and say its a 500
        log::info!("unhandled http error: {:?}", err);
        (warp::http::StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}"))
    };

    let body = warp::reply::json(&serde_json::json!({
        "error": message,
        "ok":false,
        "code":code.as_u16(),
    }));

    Ok(warp::reply::with_status(body, code).into_response())
}

fn build_routes(ctx: Ctx) -> RestFilter {
    // Box all routes to a single extract type to avoid nested `Either` explosions.
    let (registry, rest) = RpcRegistry::with_context(ctx);
    let root = warp::path::end().and_then(root_handler).boxed();
    let openapi_json =
        warp::path("openapi.json".to_string()).and_then(openapi_json_handler).boxed();
    let health_route = warp::path("health".to_string()).and_then(health_handler).boxed();
    let events_ctx = registry.ctx.clone();
    let confirmed_block_events_route = warp::path("jsonRPC".to_string())
        .and(warp::path("events".to_string()))
        .and(warp::path("confirmed-blocks".to_string()))
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::query::<ConfirmedBlockEventsQuery>())
        .and(warp::any().map(move || events_ctx.clone()))
        .and_then(confirmed_block_events_handler)
        .boxed();

    let jsonrpc_route = warp::path("jsonRPC".to_string())
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::content_length_limit(MAX_BODY_SIZE))
        .and(warp::body::json())
        .and(warp::any().map(move || registry.clone()))
        .and_then(jsonrpc_handler)
        .boxed();
    root.or(openapi_json)
        .unify()
        .or(confirmed_block_events_route)
        .unify()
        .or(jsonrpc_route)
        .unify()
        .or(health_route)
        .unify()
        .or(rest)
        .unify()
        .recover(handle_rejection)
        .unify()
        .boxed()
}

fn rpc_error(
    _id: serde_json::Value,
    code: i64,
    message: &str,
    jsonrpc_http_status: warp::http::StatusCode,
) -> warp::reply::Response {
    #[derive(serde::Serialize)]
    struct JsonRpcErrorResp {
        ok: serde_json::Value,
        error: String,
        code: i64,
        #[serde(rename = "@extra")]
        extra: String,
    }
    let body = JsonRpcErrorResp {
        ok: serde_json::Value::Bool(false),
        error: message.to_string(),
        code,
        extra: handlers::extra(0),
    };
    let mut resp = warp::reply::json(&body).into_response();
    *resp.status_mut() = jsonrpc_http_status;
    resp
}

fn rpc_ok(_id: serde_json::Value, result: serde_json::Value) -> warp::reply::Response {
    #[derive(serde::Serialize)]
    struct JsonRpcSuccessResp {
        ok: serde_json::Value,
        result: serde_json::Value,
        #[serde(rename = "@extra")]
        extra: String,
    }
    let body =
        JsonRpcSuccessResp { ok: serde_json::Value::Bool(true), result, extra: handlers::extra(0) };
    warp::reply::json(&body).into_response()
}

#[derive(serde::Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>, // array for our add/sub
    #[serde(default)]
    id: Option<serde_json::Value>, // may be number/string/null
}

async fn jsonrpc_handler(
    req: JsonRpcRequest,
    registry: RpcRegistry,
) -> std::result::Result<warp::reply::Response, warp::Rejection> {
    let id = req.id.unwrap_or(serde_json::Value::Null);
    if req.jsonrpc != "2.0" {
        return Ok(rpc_error(
            id,
            -32600,
            "Invalid Request: jsonrpc must be \"2.0\"",
            http::StatusCode::BAD_REQUEST,
        ));
    }

    let params = req.params.unwrap_or(serde_json::Value::Object(serde_json::map::Map::new()));
    let Some(handler) = registry.json.get(&req.method) else {
        return Ok(rpc_error(id, 404, "Method not found", http::StatusCode::NOT_FOUND));
    };

    match handler.handle(params, registry.ctx).await {
        Ok(result) => Ok(rpc_ok(id, result)),
        Err(err) => {
            let message = err.to_string();
            let err = err.downcast::<ApiError>().unwrap_or_default();
            Ok(rpc_error(id, err.jsonrpc_code(), &message, err.jsonrpc_http_status()))
        }
    }
}

#[derive(Default, Debug, thiserror::Error)]
pub(crate) enum ApiError {
    #[error("Internal Error")]
    #[default]
    InternalError,
    #[error("Bad Request: {0} (code {1})")]
    BadRequest(String, i64),
    #[error("Not Found: {0}")]
    #[allow(dead_code)]
    NotFound(String),
    #[error("{0}")]
    NotSatisfiable(String),
    #[error("Mimic: {0}")]
    Mimic(String),
    #[error("Missing required parameter {0}")]
    MissingParamTC(String), //This error returns 422/503 as TonCenter does
    #[error("Missing required parameter {0}")]
    MissingParam(String), //this error returns 422
    #[error("Invalid parameter {0}")]
    InvalidParam(String),
    #[error("{0}")]
    Conflict(String),
}

impl ApiError {
    pub(crate) fn bad_request(msg: impl ToString) -> Self {
        ApiError::BadRequest(msg.to_string(), -32400)
    }
    // pub fn not_found(msg: impl ToString) -> Self {
    //     ApiError::NotFound(msg.to_string())
    // }
    pub(crate) fn unprocessable_entry(msg: impl ToString, code: i64) -> Self {
        ApiError::BadRequest(msg.to_string(), code)
    }
    pub(crate) fn jsonrpc_code(&self) -> i64 {
        match self {
            ApiError::BadRequest(_, code) => *code,
            ApiError::NotFound(_) => 404,
            ApiError::NotSatisfiable(_) => 416,
            ApiError::Mimic(_) => 422,
            ApiError::MissingParamTC(_) => 422,
            ApiError::MissingParam(_) => 422,
            ApiError::InvalidParam(_) => 503,
            ApiError::InternalError => 500,
            ApiError::Conflict(_) => 409,
        }
    }
    pub(crate) fn http_status(&self) -> warp::http::StatusCode {
        match self {
            ApiError::BadRequest(_, _) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NotFound(_) => warp::http::StatusCode::NOT_FOUND,
            ApiError::NotSatisfiable(_) => warp::http::StatusCode::RANGE_NOT_SATISFIABLE,
            ApiError::Mimic(_) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::InternalError => warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::MissingParamTC(_) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::MissingParam(_) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::InvalidParam(_) => warp::http::StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Conflict(_) => warp::http::StatusCode::CONFLICT,
        }
    }
    pub(crate) fn jsonrpc_http_status(&self) -> warp::http::StatusCode {
        match self {
            ApiError::BadRequest(_, _) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NotFound(_) => warp::http::StatusCode::NOT_FOUND,
            ApiError::NotSatisfiable(_) => warp::http::StatusCode::RANGE_NOT_SATISFIABLE,
            ApiError::Mimic(_) => warp::http::StatusCode::OK,
            ApiError::InternalError => warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::MissingParamTC(_) => warp::http::StatusCode::SERVICE_UNAVAILABLE,
            ApiError::MissingParam(_) => warp::http::StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::InvalidParam(_) => warp::http::StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Conflict(_) => warp::http::StatusCode::CONFLICT,
        }
    }
}
