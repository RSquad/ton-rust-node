/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::telemetry::{Metric, MetricBuilder, TelemetryItem, TelemetryPrinter};
use crate::{
    common::{
        AdnlHandshake, AdnlPeers, AdnlPingSubscriber, AdnlStream, AdnlStreamCrypto, Query, QueryId,
        Subscriber, TaggedAdnlMessage, TimedAnswer, Timeouts, TARGET, TARGET_QUERY,
    },
    dump,
};
use futures::prelude::*;
use rand::RngCore;
#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    convert::TryInto,
    io::ErrorKind,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use stream_cancel::StreamExt;
use ton_api::{
    deserialize_boxed, serialize_boxed_inplace,
    ton::{
        adnl::{message::message::Query as AdnlQueryMessage, Message as AdnlMessage},
        rpc::tcp::Ping as TcpPing,
        tcp::{message::AuthentificationNonce, pong::Pong as TcpPong, Message as TcpMessage},
        PublicKey,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{
    base64_encode, error, fail, Ed25519KeyOption, KeyId, KeyOption, KeyOptionJson, Result,
};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum AdnlServerClients {
    Any,
    List(Vec<KeyOptionJson>),
}

impl Default for AdnlServerClients {
    fn default() -> Self {
        AdnlServerClients::Any
    }
}

fn is_default(x: &AdnlServerClients) -> bool {
    if let AdnlServerClients::Any = x {
        true
    } else {
        false
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AdnlServerConfigJson {
    address: String,
    #[serde(default, skip_serializing_if = "is_default")]
    clients: AdnlServerClients,
    server_key: KeyOptionJson,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeouts: Option<Timeouts>,
}

impl AdnlServerConfigJson {
    pub fn with_params(
        address: String,
        server_key: KeyOptionJson,
        client_keys: Option<Vec<KeyOptionJson>>,
        timeouts: Option<Timeouts>,
    ) -> Self {
        let clients = if let Some(client_keys) = client_keys {
            AdnlServerClients::List(client_keys)
        } else {
            AdnlServerClients::Any
        };
        Self { address, clients, server_key, timeouts }
    }
}

/// ADNL server configuration
pub struct AdnlServerConfig {
    address: SocketAddr,
    clients: Arc<Option<lockfree::map::Map<[u8; 32], u8>>>,
    server_key: Arc<lockfree::map::Map<Arc<KeyId>, Arc<dyn KeyOption>>>,
    server_id: Arc<KeyId>,
    timeouts: Timeouts,
}

impl AdnlServerConfig {
    /// Costructs from JSON data
    pub fn from_json(json: &str) -> Result<Self> {
        let json_config: AdnlServerConfigJson = serde_json::from_str(json)?;
        Self::from_json_config(&json_config)
    }

    /// Construct from JSON config structure
    pub fn from_json_config(json_config: &AdnlServerConfigJson) -> Result<Self> {
        let key = Ed25519KeyOption::from_private_key_json(&json_config.server_key)?;
        let server_key = lockfree::map::Map::new();
        let server_id = key.id().clone();
        server_key.insert(key.id().clone(), key);
        let clients = match &json_config.clients {
            AdnlServerClients::Any => None,
            AdnlServerClients::List(list) => {
                let clients = lockfree::map::Map::new();
                for key in list {
                    let key = Ed25519KeyOption::from_public_key_json(key)?;
                    let key = key.pub_key()?;
                    if clients.insert(key.try_into()?, 0).is_some() {
                        fail!("Duplicated client key {} in server config", base64_encode(key))
                    }
                }
                Some(clients)
            }
        };
        let ret = AdnlServerConfig {
            address: json_config.address.parse()?,
            clients: Arc::new(clients),
            server_key: Arc::new(server_key),
            server_id,
            timeouts: if let Some(timeouts) = &json_config.timeouts {
                timeouts.clone()
            } else {
                Timeouts::default()
            },
        };
        Ok(ret)
    }

    /// Check clients list
    pub fn is_any_client(&self) -> bool {
        self.clients.is_none()
    }

    /// Get timeouts
    pub fn timeouts(&self) -> &Timeouts {
        &self.timeouts
    }

    /// Get server ID
    pub fn server_id(&self) -> &[u8; 32] {
        self.server_id.data()
    }
}

/// TCP authentication context
struct TcpAuthState {
    nonce: Option<Vec<u8>>,
    remote_id: Option<[u8; 32]>,
}

struct AdnlServerThreadMetrics {
    messages: u32,
}

/// ADNL server thread (one connection)
struct AdnlServerThread {
    auth: TcpAuthState,
    crypto: AdnlStreamCrypto,
    peers: AdnlPeers,
    recv_buf: Vec<u8>,
    send_buf: Vec<u8>,
    stream: AdnlStream,
    subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    #[cfg(feature = "telemetry")]
    start: Instant,
}

impl AdnlServerThread {
    fn get_query_print_id(query_id: &QueryId) -> String {
        format!(
            "query {:02x}{:02x}{:02x}{:02x}",
            query_id[0], query_id[1], query_id[2], query_id[3],
        )
    }

    fn parse_init_packet(
        key: &lockfree::map::Map<Arc<KeyId>, Arc<dyn KeyOption>>,
        buf: &mut Vec<u8>,
    ) -> Result<(AdnlStreamCrypto, AdnlPeers)> {
        let other_key = buf[32..64].try_into()?;
        let (local_key, version) = AdnlHandshake::parse_packet(key, buf, Some(160), false)?;
        let local_key =
            local_key.ok_or_else(|| error!("Unknown ADNL server key, cannot decrypt"))?;
        if let Some(version) = version {
            fail!("Unsupported ADNL version {version} in TCP connection")
        }
        let other_key = Ed25519KeyOption::from_public_key(&other_key).id().clone();
        dump!(trace, TARGET, "Nonce", &buf[..160]);
        let nonce: &mut [u8; 160] = buf.as_mut_slice().try_into()?;
        let ret = AdnlStreamCrypto::with_nonce_as_server(nonce);
        buf.drain(0..160);
        Ok((ret, AdnlPeers::with_keys(local_key, other_key)))
    }

    async fn recv(&mut self) -> Result<()> {
        self.crypto.receive(&mut self.recv_buf, &mut self.stream).await
    }

    async fn reply(&mut self, query: AdnlQueryMessage, reply: TaggedAdnlMessage) -> Result<()> {
        log::info!(
            target: TARGET_QUERY,
            "ADNL server reply to {}",
            Self::get_query_print_id(query.query_id.as_slice())
        );
        serialize_boxed_inplace(&mut self.send_buf, &reply.object)?;
        self.send().await
    }

    async fn respond(&mut self, msg: &str, response: &TLObject) -> Result<()> {
        log::info!(target: TARGET_QUERY, "ADNL server response to {msg}");
        serialize_boxed_inplace(&mut self.send_buf, response)?;
        self.send().await
    }

    async fn run(
        mut stream: AdnlStream,
        key: Arc<lockfree::map::Map<Arc<KeyId>, Arc<dyn KeyOption>>>,
        clients: Arc<Option<lockfree::map::Map<[u8; 32], u8>>>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        metrics: &mut AdnlServerThreadMetrics,
        #[cfg(feature = "telemetry")] telemetry: Arc<AdnlServerTelemetry>,
    ) -> Result<()> {
        struct AnswerContext {
            answer: Result<TimedAnswer<TaggedAdnlMessage>>,
            query: AdnlQueryMessage,
            #[cfg(feature = "telemetry")]
            start_at_micros: u64,
        }

        async fn process_delayed_answer(
            answer_context: Option<AnswerContext>,
            thread_context: &mut AdnlServerThread,
            #[cfg(feature = "telemetry")] telemetry: &Arc<AdnlServerTelemetry>,
            #[cfg(feature = "telemetry")] processed_ok: &mut bool,
        ) -> Result<()> {
            let Some(answer_context) = answer_context else {
                fail!("Unexpectedly closed delayed answers queue")
            };
            let timed_answer = answer_context.answer?;
            let Some(reply) = timed_answer.answer else {
                fail!("Unexpected/unsupported ADNL query {:?}", answer_context.query)
            };
            #[cfg(feature = "telemetry")]
            {
                let query_time = if let Some(start) = timed_answer.actual_start_at {
                    start.elapsed().as_micros() as u64
                } else {
                    thread_context.start.elapsed().as_micros() as u64
                        - answer_context.start_at_micros
                };
                telemetry.query_time.update(query_time);
                log::info!(
                    target: TARGET,
                    "ADNL server query {:X} done in {query_time}micros",
                    reply.tag
                );
                telemetry.queries_normal.update(1);
                *processed_ok = true;
            }
            thread_context.reply(answer_context.query, reply).await?;
            #[cfg(feature = "telemetry")]
            telemetry.queries_send.update(1);
            Ok(())
        }

        async fn process_query(
            query: AdnlQueryMessage,
            thread: &mut AdnlServerThread,
            answer_sender: tokio::sync::mpsc::UnboundedSender<AnswerContext>,
            #[cfg(feature = "telemetry")] telemetry: &Arc<AdnlServerTelemetry>,
            #[cfg(feature = "telemetry")] processed_ok: &mut bool,
        ) -> Result<()> {
            #[cfg(feature = "telemetry")]
            let start_at_micros = {
                telemetry.queries_recv.update(1);
                telemetry.queries.fetch_add(1, Ordering::Relaxed);
                thread.start.elapsed().as_micros() as u64
            };
            let answer = Query::process_adnl(&thread.subscribers, &query, &thread.peers).await?;
            let Some(answer) = answer else {
                fail!("Unexpected ADNL query {query:?}");
            };
            match answer.try_finalize()? {
                (Some(answer), _) => {
                    #[cfg(feature = "telemetry")]
                    let telemetry = telemetry.clone();
                    tokio::spawn(async move {
                        let query_id =
                            AdnlServerThread::get_query_print_id(query.query_id.as_slice());
                        let answer = AnswerContext {
                            answer: answer.try_wait().await,
                            query,
                            #[cfg(feature = "telemetry")]
                            start_at_micros,
                        };
                        #[cfg(feature = "telemetry")]
                        telemetry.queries.fetch_sub(1, Ordering::Relaxed);
                        if let Err(e) = answer_sender.send(answer) {
                            #[cfg(feature = "telemetry")]
                            telemetry.queries_failed.update(1);
                            log::error!(
                                target: TARGET,
                                "Cannot deliver ADNL server reply on {query_id}: {e}"
                            )
                        }
                    });
                }
                (None, Some(reply)) => {
                    #[cfg(feature = "telemetry")]
                    {
                        let query_time =
                            thread.start.elapsed().as_micros() as u64 - start_at_micros;
                        telemetry.query_time.update(query_time);
                        log::info!(
                            target: TARGET,
                            "ADNL server query {:X} done in {query_time}micros",
                            reply.tag
                        );
                        telemetry.queries_normal.update(1);
                        telemetry.queries.fetch_sub(1, Ordering::Relaxed);
                        *processed_ok = true;
                    }
                    thread.reply(query, reply).await?;
                    #[cfg(feature = "telemetry")]
                    telemetry.queries_send.update(1);
                }
                (None, None) => fail!("Unexpected ADNL query {query:?}"),
            }
            Ok(())
        }

        let mut recv_buf = Vec::with_capacity(256);
        stream.read_exact(&mut recv_buf, 256).await?;
        if let Some(clients) = clients.as_ref() {
            // Check known client if any
            if recv_buf.len() < 64 {
                fail!("ADNL init message is too short ({})", recv_buf.len())
            }
            if !clients.iter().any(|client| &recv_buf[32..64] == client.key()) {
                fail!("Message from unknown client {}", base64_encode(&recv_buf[32..64]))
            }
        }
        let (mut crypto, peers) = Self::parse_init_packet(&key, &mut recv_buf)?;
        let auth = TcpAuthState { nonce: None, remote_id: None };
        recv_buf.truncate(0);
        let mut send_buf = Vec::new();
        crypto.send(&mut stream, &mut send_buf).await?;
        let mut thread = AdnlServerThread {
            auth,
            crypto,
            peers,
            recv_buf,
            send_buf,
            stream,
            subscribers,
            #[cfg(feature = "telemetry")]
            start: Instant::now(),
        };

        let (answer_sender, mut answer_reader) =
            tokio::sync::mpsc::unbounded_channel::<AnswerContext>();
        loop {
            tokio::select! {
                result = answer_reader.recv() => {
                    #[cfg(feature = "telemetry")]
                    let mut processed_ok = false;
                    let result = process_delayed_answer(
                        result,
                        &mut thread,
                        #[cfg(feature = "telemetry")]
                        &telemetry,
                        #[cfg(feature = "telemetry")]
                        &mut processed_ok,
                    )
                    .await;
                    #[cfg(feature = "telemetry")]
                    if result.is_err() && !processed_ok {
                        telemetry.queries_failed.update(1);
                    }
                    result?;
                    continue;
                }
                result = thread.recv() => {
                    result?;
                }
            }
            let msg = deserialize_boxed(&thread.recv_buf[..])?;
            let msg = match msg.downcast::<AdnlMessage>() {
                Ok(msg) => {
                    metrics.messages += 1;
                    let AdnlMessage::Adnl_Message_Query(query) = msg else {
                        fail!("Unexpected ADNL message {msg:?}");
                    };
                    log::info!(
                        target: TARGET_QUERY,
                        "ADNL server recv {}",
                        Self::get_query_print_id(query.query_id.as_slice())
                    );
                    #[cfg(feature = "telemetry")]
                    let mut processed_ok = false;
                    let result = process_query(
                        query,
                        &mut thread,
                        answer_sender.clone(),
                        #[cfg(feature = "telemetry")]
                        &telemetry,
                        #[cfg(feature = "telemetry")]
                        &mut processed_ok,
                    )
                    .await;
                    #[cfg(feature = "telemetry")]
                    if result.is_err() && !processed_ok {
                        telemetry.queries.fetch_sub(1, Ordering::Relaxed);
                        telemetry.queries_failed.update(1);
                    }
                    result?;
                    continue;
                }
                Err(msg) => msg,
            };
            let msg = match msg.downcast::<TcpPing>() {
                Ok(ping) => {
                    let random_id = ping.random_id;
                    let pong = TcpPong { random_id }.into_boxed();
                    thread.respond("tcp.ping", &pong.into_tl_object()).await?;
                    continue;
                }
                Err(msg) => msg,
            };
            match msg.downcast::<TcpMessage>() {
                Ok(TcpMessage::Tcp_Authentificate(auth)) => {
                    const SERVER_NONCE_LEN: usize = 256;
                    if thread.auth.nonce.is_some() {
                        fail!("duplicated TCP authentication attempt: nonce is set already");
                    }
                    if thread.auth.remote_id.is_some() {
                        fail!("duplicated TCP authentication attempt: remote id is set already");
                    }
                    let client_nonce = auth.nonce.as_slice();
                    let mut full = Vec::with_capacity(client_nonce.len() + SERVER_NONCE_LEN);
                    full.extend_from_slice(client_nonce);
                    let mut server_part = vec![0u8; SERVER_NONCE_LEN];
                    rand::thread_rng().fill_bytes(&mut server_part);
                    full.extend_from_slice(&server_part);
                    thread.auth.nonce = Some(full);
                    let reply = AuthentificationNonce { nonce: server_part.into() }.into_boxed();
                    thread.respond("tcp.auth", &reply.into_tl_object()).await?;
                    continue;
                }
                Ok(TcpMessage::Tcp_AuthentificationComplete(complete)) => {
                    let Some(nonce) = thread.auth.nonce.as_ref() else {
                        fail!("cannot complete TCP authentication without nonce");
                    };
                    if thread.auth.remote_id.is_some() {
                        fail!("duplicated TCP authentication attempt");
                    }
                    let pub_key = match complete.key {
                        PublicKey::Pub_Ed25519(key) => key.key.inner(),
                        x => fail!("Unsupported PublicKey {x:?} in TCP auth"),
                    };
                    let pub_key = Ed25519KeyOption::from_public_key(&pub_key);
                    pub_key.verify(nonce, complete.signature.as_slice())?;
                    thread.auth.remote_id = Some(pub_key.id().data().clone());
                    thread.auth.nonce = None;
                    continue;
                }
                Ok(msg) => fail!("Unexpected ADNL server message {msg:?}"),
                Err(msg) => fail!("Unexpected ADNL server message {msg:?}"),
            }
        }
    }

    async fn send(&mut self) -> Result<()> {
        self.crypto.send(&mut self.stream, &mut self.send_buf).await
    }

    fn spawn(
        stream: tokio::net::TcpStream,
        config: &AdnlServerConfig,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        #[cfg(feature = "telemetry")] telemetry: Arc<AdnlServerTelemetry>,
    ) {
        let stream = AdnlStream::from_stream_with_timeouts(stream, config.timeouts());
        let clients = config.clients.clone();
        let key = config.server_key.clone();
        tokio::spawn(async move {
            #[cfg(feature = "telemetry")]
            telemetry.connections.fetch_add(1, Ordering::Relaxed);
            let mut metrics = AdnlServerThreadMetrics { messages: 0 };
            let start = Instant::now();
            if let Err(e) = AdnlServerThread::run(
                stream,
                key,
                clients,
                subscribers,
                &mut metrics,
                #[cfg(feature = "telemetry")]
                telemetry.clone(),
            )
            .await
            {
                log::warn!(
                    target: TARGET,
                    "ADNL server ERROR --> {e}, {} messages processed, {}ms total",
                    metrics.messages,
                    start.elapsed().as_millis()
                );
            }
            #[cfg(feature = "telemetry")]
            telemetry.connections.fetch_sub(1, Ordering::Relaxed);
            #[cfg(feature = "telemetry")]
            telemetry.connections_closed.update(1);
        });
    }
}

#[cfg(feature = "telemetry")]
struct AdnlServerTelemetry {
    connections: AtomicU64,
    connections_active: Arc<Metric>,
    connections_closed: Arc<MetricBuilder>,
    connections_failed: Arc<MetricBuilder>,
    connections_opened: Arc<MetricBuilder>,
    queries: AtomicU64,
    queries_failed: Arc<MetricBuilder>,
    queries_inproc: Arc<Metric>,
    queries_normal: Arc<MetricBuilder>,
    queries_recv: Arc<MetricBuilder>,
    queries_send: Arc<MetricBuilder>,
    query_time: Arc<Metric>,
    printer: TelemetryPrinter,
    stop: Arc<AtomicBool>,
}

/// ADNL server
pub struct AdnlServer {
    stop: stream_cancel::Trigger,
}

impl AdnlServer {
    #[cfg(feature = "telemetry")]
    const PERIOD_AVERAGE_SEC: u64 = 5;
    #[cfg(feature = "telemetry")]
    const PERIOD_MEASURE_NANO: u64 = 1000000000;
    const TIMEOUT_SHUTDOWN_MS: u64 = 100;

    /// Listen to connections
    pub async fn listen(
        config: AdnlServerConfig,
        mut subscribers: Vec<Arc<dyn Subscriber>>,
        #[cfg(feature = "telemetry")] name: &'static str,
    ) -> Result<Self> {
        #[cfg(feature = "telemetry")]
        let telemetry = {
            let connections_active =
                Metric::without_totals("active connections", Self::PERIOD_AVERAGE_SEC);
            let connections_closed = Self::create_metric("closed connections/sec");
            let connections_failed = Self::create_metric("failed connections/sec");
            let connections_opened = Self::create_metric("opened connections/sec");
            let queries_failed = Self::create_metric("failed queries/sec");
            let queries_inproc =
                Metric::without_totals("in-process queries/sec", Self::PERIOD_AVERAGE_SEC);
            let queries_normal = Self::create_metric("normal queries/sec");
            let queries_send = Self::create_metric("outgoing, answers/sec");
            let queries_recv = Self::create_metric("incoming, queries/sec");
            let query_time = Metric::without_totals("query time, micros", Self::PERIOD_AVERAGE_SEC);
            let printer = TelemetryPrinter::with_params(
                name,
                Self::PERIOD_AVERAGE_SEC,
                vec![
                    TelemetryItem::Metric(connections_active.clone()),
                    TelemetryItem::MetricBuilder(connections_closed.clone()),
                    TelemetryItem::MetricBuilder(connections_failed.clone()),
                    TelemetryItem::MetricBuilder(connections_opened.clone()),
                    TelemetryItem::MetricBuilder(queries_recv.clone()),
                    TelemetryItem::Metric(queries_inproc.clone()),
                    TelemetryItem::MetricBuilder(queries_failed.clone()),
                    TelemetryItem::MetricBuilder(queries_normal.clone()),
                    TelemetryItem::MetricBuilder(queries_send.clone()),
                    TelemetryItem::Metric(query_time.clone()),
                ],
            );
            Arc::new(AdnlServerTelemetry {
                connections: AtomicU64::new(0),
                connections_active,
                connections_closed,
                connections_failed,
                connections_opened,
                queries: AtomicU64::new(0),
                queries_failed,
                queries_inproc,
                queries_normal,
                queries_recv,
                queries_send,
                query_time,
                printer,
                stop: Arc::new(AtomicBool::new(false)),
            })
        };
        #[cfg(feature = "telemetry")]
        tokio::spawn({
            let telemetry = telemetry.clone();
            async move {
                loop {
                    if telemetry.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(Self::PERIOD_AVERAGE_SEC / 2)).await;
                    telemetry
                        .connections_active
                        .update(telemetry.connections.load(Ordering::Relaxed));
                    telemetry.queries_inproc.update(telemetry.queries.load(Ordering::Relaxed));
                    telemetry.printer.try_print();
                }
                log::info!(target: TARGET, "ADNL server telemetry task stopped");
            }
        });
        let (trigger, tripwire) = stream_cancel::Tripwire::new();
        subscribers.push(Arc::new(AdnlPingSubscriber));
        let subscribers = Arc::new(subscribers);
        if false {
            Self::listen_by_socket(
                config,
                subscribers,
                tripwire,
                #[cfg(feature = "telemetry")]
                telemetry,
            )?;
        } else {
            Self::listen_by_tokio(
                config,
                subscribers,
                tripwire,
                #[cfg(feature = "telemetry")]
                telemetry,
            )
            .await?;
        }
        Ok(Self { stop: trigger })
    }

    /// Shutdown server
    pub async fn shutdown(self) {
        drop(self.stop);
        tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_SHUTDOWN_MS)).await;
    }

    #[cfg(feature = "telemetry")]
    fn create_metric(name: &str) -> Arc<MetricBuilder> {
        MetricBuilder::with_metric_and_period(
            Metric::without_totals(name, Self::PERIOD_AVERAGE_SEC),
            Self::PERIOD_MEASURE_NANO,
        )
    }

    fn listen_by_socket(
        config: AdnlServerConfig,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        tripwire: stream_cancel::Tripwire,
        #[cfg(feature = "telemetry")] telemetry: Arc<AdnlServerTelemetry>,
    ) -> Result<()> {
        const SOCKET_TCP_BACKLOG: i32 = 20;
        const TIMEOUT_TCP_SPIN_MS: Duration = Duration::from_millis(10);
        let listener = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?;
        listener.set_nonblocking(true)?;
        listener.set_reuse_address(true)?;
        listener.bind(&config.address.clone().into())?;
        listener.listen(SOCKET_TCP_BACKLOG)?;
        tokio::spawn(async move {
            loop {
                let socket = tokio::select! {
                    _ = tripwire.clone() => {
                        break
                    }
                    _ = tokio::time::sleep(TIMEOUT_TCP_SPIN_MS) => {
                        match listener.accept() {
                            Ok((socket, _)) => socket,
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                #[cfg(feature = "telemetry")]
                                telemetry.connections_failed.update(1);
                                #[cfg(feature = "telemetry")]
                                telemetry.connections_opened.update(1);
                                log::warn!(target: TARGET, "Error in ADNL server listener: {e}");
                                continue;
                            }
                        }
                    }
                };
                if let Err(e) = socket.set_tcp_nodelay(true) {
                    log::warn!(target: TARGET, "Cannot set TCP_NODELAY to socket: {e}");
                }
                let stream = match tokio::net::TcpStream::from_std(socket.into()) {
                    Err(e) => {
                        #[cfg(feature = "telemetry")]
                        telemetry.connections_failed.update(1);
                        #[cfg(feature = "telemetry")]
                        telemetry.connections_opened.update(1);
                        log::warn!(target: TARGET, "Cannot create input TCP stream: {e}");
                        continue;
                    }
                    Ok(stream) => stream,
                };
                #[cfg(feature = "telemetry")]
                telemetry.connections_opened.update(1);
                AdnlServerThread::spawn(
                    stream,
                    &config,
                    subscribers.clone(),
                    #[cfg(feature = "telemetry")]
                    telemetry.clone(),
                );
            }
            #[cfg(feature = "telemetry")]
            telemetry.stop.store(true, Ordering::Relaxed);
        });
        Ok(())
    }

    async fn listen_by_tokio(
        config: AdnlServerConfig,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        tripwire: stream_cancel::Tripwire,
        #[cfg(feature = "telemetry")] telemetry: Arc<AdnlServerTelemetry>,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(config.address).await?;
        tokio::spawn(async move {
            let mut incoming =
                tokio_stream::wrappers::TcpListenerStream::new(listener).take_until_if(tripwire);
            loop {
                match incoming.next().await {
                    Some(Err(e)) => {
                        #[cfg(feature = "telemetry")]
                        telemetry.connections_failed.update(1);
                        #[cfg(feature = "telemetry")]
                        telemetry.connections_opened.update(1);
                        log::warn!(target: TARGET, "Error in listener {e}")
                    }
                    Some(Ok(stream)) => {
                        if let Err(e) = stream.set_nodelay(true) {
                            log::warn!(target: TARGET, "Cannot set TCP_NODELAY to socket: {e}")
                        }
                        #[cfg(feature = "telemetry")]
                        telemetry.connections_opened.update(1);
                        AdnlServerThread::spawn(
                            stream,
                            &config,
                            subscribers.clone(),
                            #[cfg(feature = "telemetry")]
                            telemetry.clone(),
                        );
                        continue;
                    }
                    _ => (),
                }
                break;
            }
            #[cfg(feature = "telemetry")]
            telemetry.stop.store(true, Ordering::Relaxed);
        });
        Ok(())
    }
}

#[cfg(all(test, feature = "client", feature = "server"))]
#[path = "../tests/test_server.rs"]
mod test;
