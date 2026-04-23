/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
mod rate_limiter;
mod stat;

use crate::{
    common::{
        add_unbound_object_to_map, add_unbound_object_to_map_with_update, spawn_cancelable,
        AdnlPeers, Answer, Query, QueryAnswer, Subscriber,
    },
    node::AdnlNode,
    transport::{Connections, SendQueue},
};
pub use rate_limiter::QuicRateLimitConfig;
use rate_limiter::{ConnectionRateLimiters, RateLimiter};
use stat::{extract_inner_tag, tl_tag_name, ConnSnapshot, MsgKind, MsgStats, TransportErrors};
use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Formatter, Write},
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Once, Weak,
    },
    time::{Duration, Instant},
};
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::quic::{
        answer::Answer as QuicAnswer,
        request::{Message as QuicMessage, Query as QuicQuery},
        Request, Response as QuicResponse,
    },
    IntoBoxed,
};
use ton_block::{
    ed25519_encode_private_key_to_pkcs8, error, fail, Ed25519KeyOption, KeyId, Result,
};

const TARGET: &str = "quic";

/// Distinguishes connection-level failures from per-message send errors.
enum SendError {
    /// Could not establish a connection (peer unreachable, handshake timeout).
    /// The sender task should flush the queue — retrying individual messages
    /// will hit the same handshake timeout each time.
    Fatal(anyhow::Error),
    /// Connection existed but the send failed (stream reset, dead connection).
    /// The sender task should continue to the next message.
    Temporary(anyhow::Error),
}

/// Key for the QUIC inbound connection map: (local_key_id, peer_key_id).
/// Matches the C++ `AdnlPath{local_id, peer_id}` semantics so that two
/// connections from the same peer address but different key pairs (e.g.
/// current + next validator keys) coexist instead of evicting each other.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct QuicInboundKey(Arc<KeyId>, Arc<KeyId>, usize);

type QuicInboundMap = lockfree::map::Map<QuicInboundKey, quinn::Connection>;
type QuicSendQueue = SendQueue<Vec<u8>>;

/// Extract a `KeyId` from an Ed25519 SubjectPublicKeyInfo (SPKI) DER blob.
/// Ed25519 SPKI = 12-byte OID header || 32-byte raw public key (total 44 bytes).
fn key_id_from_spki(spki: &[u8]) -> Result<Arc<KeyId>> {
    const ED25519_SPKI_LEN: usize = 44;
    const ED25519_KEY_OFFSET: usize = 12;
    if spki.len() != ED25519_SPKI_LEN {
        fail!("Unexpected Ed25519 SPKI length: {} (expected {ED25519_SPKI_LEN})", spki.len());
    }
    let pub_key: &[u8; 32] = spki[ED25519_KEY_OFFSET..]
        .try_into()
        .map_err(|_| error!("Cannot slice Ed25519 public key from SPKI"))?;
    let data =
        ton_block::sha256_digest_slices(&[&Ed25519KeyOption::KEY_TYPE.to_le_bytes(), pub_key]);
    Ok(KeyId::from_data(data))
}

struct QuicOutboundConnection {
    conn: Option<quinn::Connection>,
    send_queue: Arc<QuicSendQueue>,
    sender_state: Arc<SenderState>,
}

/// Per-peer sender lifecycle guard. Uses an atomic flag to ensure exactly
/// one sender task runs per outbound peer. Also tracks connect attempts
/// and timestamps for diagnostics.
struct SenderState {
    active: AtomicBool,
    /// Number of consecutive connect attempts (reset on success).
    connect_attempts: AtomicU64,
    /// Timestamp of the last successful connect (`None` if never connected).
    last_connect: Mutex<Option<Instant>>,
    /// Timestamp of the last time the connection was seen alive by the
    /// periodic checker (~every 5s). Updated by `spawn_connection_checker`.
    last_alive: Mutex<Option<Instant>>,
}

impl SenderState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            connect_attempts: AtomicU64::new(0),
            last_connect: Mutex::new(None),
            last_alive: Mutex::new(None),
        })
    }

    fn record_connect_success(&self) {
        self.connect_attempts.store(0, Ordering::Relaxed);
        let now = Instant::now();
        if let Ok(mut ts) = self.last_connect.lock() {
            *ts = Some(now);
        }
        if let Ok(mut ts) = self.last_alive.lock() {
            *ts = Some(now);
        }
    }

    /// Called by the connection checker when the connection is alive.
    fn touch_alive(&self) {
        if let Ok(mut ts) = self.last_alive.lock() {
            *ts = Some(Instant::now());
        }
    }

    fn next_attempt(&self) -> u64 {
        self.connect_attempts.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn last_alive_ago(&self) -> String {
        match self.last_alive.lock().ok().and_then(|ts| *ts) {
            Some(ts) => format!("{:.1}s ago", ts.elapsed().as_secs_f64()),
            None => "never".to_string(),
        }
    }
}

/// Presents a single fixed Ed25519 SPKI (RPK) as the client certificate.
/// One instance is created per local identity so `resolve()` always returns the right key.
#[derive(Debug)]
struct QuicCertResolver(Arc<rustls::sign::CertifiedKey>);

impl rustls::client::ResolvesClientCert for QuicCertResolver {
    fn resolve(
        &self,
        _: &[&[u8]],
        _: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }

    fn has_certs(&self) -> bool {
        true
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

/// Fixed single-key server cert resolver for the per-key fallback configs.
struct QuicSingleKeyServerResolver(Arc<rustls::sign::CertifiedKey>);

impl Debug for QuicSingleKeyServerResolver {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicSingleKeyServerResolver").finish()
    }
}

impl rustls::server::ResolvesServerCert for QuicSingleKeyServerResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

/// Per-identity outbound state: a dedicated client config (with this identity's cert) and
/// its outbound connection pool keyed by remote SocketAddr.
struct LocalKeyState {
    client_config: quinn::ClientConfig,
    outbound: Arc<Connections<QuicOutboundConnection>>,
    /// The port this identity's endpoint is bound to (for outbound connect).
    bound_port: u16,
}

/// Used by the client to verify the server's TLS credential (RFC 7250 RPK mode).
/// `requires_raw_public_keys()` tells the server to send its Ed25519 SPKI instead of X.509,
/// matching the C++ `SSL_CTX_set1_server_cert_type(RPK)`. Chain validation is skipped like
/// the C++ no-op verify callback; the TLS 1.3 handshake signature is still verified.
#[derive(Debug)]
struct QuicServerCertVerifier;

impl rustls::client::danger::ServerCertVerifier for QuicServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let spki = cert.as_ref();
        const KEY_OFFSET: usize = 12;
        const KEY_END: usize = KEY_OFFSET + 32;
        if spki.len() < KEY_END {
            return Err(rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding));
        }
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            &spki[KEY_OFFSET..KEY_END],
        )
        .verify(message, dss.signature())
        .map(|_| rustls::client::danger::HandshakeSignatureValid::assertion())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature))
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// Resolves the server's TLS credential per SNI. Returns a raw Ed25519 SPKI (RFC 7250 RPK)
/// instead of an X.509 certificate to match the C++ ADNL/QUIC implementation.
struct QuicServerCertResolver {
    keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>>,
    /// Most recently registered identity name. Used as SNI fallback when the client
    /// (e.g. C++ ngtcp2) doesn't send SNI, matching C++ SO_REUSEADDR behavior where
    /// the last-bound socket receives packets.
    last_added_name: Arc<Mutex<Option<String>>>,
}

impl QuicServerCertResolver {
    fn new(
        keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>>,
        last_added_name: Arc<Mutex<Option<String>>>,
    ) -> Arc<Self> {
        Arc::new(Self { keys, last_added_name })
    }
}

impl Debug for QuicServerCertResolver {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicServerCertResolver").finish()
    }
}

impl rustls::server::ResolvesServerCert for QuicServerCertResolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let sni_desc = client_hello.server_name().unwrap_or("<none>");
        log::trace!(target: TARGET, "QuicServerCertResolver::resolve SNI='{sni_desc}'");
        if let Some(sni) = client_hello.server_name() {
            if let Some(entry) = self.keys.get(sni) {
                log::trace!(target: TARGET, "QuicServerCertResolver: exact SNI match for '{sni}'");
                return Some(entry.val().clone());
            }
        }
        let fallback_name = self.last_added_name.lock().ok().and_then(|g| g.clone());
        let result = fallback_name
            .as_ref()
            .and_then(|name| self.keys.get(name).map(|e| (name.clone(), e.val().clone())))
            .or_else(|| self.keys.iter().next().map(|e| (e.key().clone(), e.val().clone())))
            .map(|(name, key)| {
                log::debug!(
                    target: TARGET,
                    "QuicServerCertResolver: SNI '{}' not found, falling back to '{}'{}",
                    sni_desc, name,
                    if fallback_name.is_some() { " (last added)" } else { " (arbitrary)" }
                );
                key
            });
        if result.is_none() {
            log::warn!(target: TARGET, "QuicServerCertResolver: NO keys registered, returning None");
        }
        result
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

struct QuicClientCertVerifier;

impl QuicClientCertVerifier {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Debug for QuicClientCertVerifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicClientCertVerifier").finish()
    }
}

impl rustls::server::danger::ClientCertVerifier for QuicClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    // Tell rustls to request RPK (raw public key) instead of X.509.
    fn requires_raw_public_keys(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        log::trace!(target: TARGET, "verify_client_cert: SPKI len={}", end_entity.as_ref().len());
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        log::trace!(target: TARGET, "verify_tls12_signature");
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    /// QUIC uses TLS 1.3. With RPK the `cert` parameter is SPKI DER, not X.509,
    /// so we verify directly with ring instead of going through webpki.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        log::trace!(target: TARGET, "verify_tls13_signature");
        let spki = cert.as_ref();
        const KEY_OFFSET: usize = 12;
        const KEY_END: usize = KEY_OFFSET + 32;
        if spki.len() < KEY_END {
            return Err(rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding));
        }
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            &spki[KEY_OFFSET..KEY_END],
        )
        .verify(message, dss.signature())
        .map(|_| rustls::client::danger::HandshakeSignatureValid::assertion())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature))
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Try to extract peer KeyId from quinn's `Connection::peer_identity()`.
/// Returns `Some(KeyId)` if the connection exposed RPK certs via rustls.
fn peer_key_id_from_connection(conn: &quinn::Connection) -> Option<Arc<KeyId>> {
    let identity = conn.peer_identity()?;
    let certs = identity.downcast::<Vec<rustls::pki_types::CertificateDer>>().ok()?;
    let first = certs.first()?;
    key_id_from_spki(first.as_ref()).ok()
}

/// Tracks rapid reconnection attempts from a remote address.
/// Used to detect C++ clients stuck in a connect-and-abandon loop due to
/// key mismatch after validator key rotation.
struct ReconnectTracker {
    /// Number of connections that closed without opening any streams.
    count: u32,
    /// When the first failed attempt in this window was recorded.
    window_start: Instant,
}

impl ReconnectTracker {
    fn record(entry: &mut ReconnectTracker, window: Duration) {
        let now = Instant::now();
        if now.duration_since(entry.window_start) > window {
            entry.count = 1;
            entry.window_start = now;
        } else {
            entry.count += 1;
        }
    }

    fn should_fallback(entry: &ReconnectTracker, window: Duration, threshold: u32) -> bool {
        let now = Instant::now();
        if now.duration_since(entry.window_start) > window || entry.count < threshold {
            return false;
        }
        // After threshold: alternate OLD/NEW on each attempt.
        // offset 0 → OLD, offset 1 → NEW, offset 2 → OLD, ...
        (entry.count - threshold) % 2 == 0
    }
}

/// Per-port endpoint state: the quinn endpoint, its accept loop handle,
/// and the TLS cert/key maps for identities registered on this port.
struct EndpointState {
    endpoint: quinn::Endpoint,
    server_cert_keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>>,
    local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>>,
    /// Tracks the most recently added identity name for SNI fallback.
    last_added_name: Arc<Mutex<Option<String>>>,
    /// Per-key ServerConfig for fallback: when a peer keeps failing with the
    /// default (newest) key, we cycle through older keys.
    per_key_configs: Arc<Mutex<HashMap<String, Arc<quinn::ServerConfig>>>>,
    /// Tracks rapid reconnection failures per remote IP (port-independent,
    /// since QUIC clients use a new source port for each connection).
    /// Shared with the accept loop; also stored here for potential future stats access.
    #[allow(dead_code)]
    reconnect_tracker: Arc<Mutex<HashMap<IpAddr, ReconnectTracker>>>,
}

/// Command sent to the background Tokio task that manages QUIC key operations.
/// Quinn endpoint creation requires a Tokio runtime context, but callers may run
/// on bare OS threads (e.g. Simplex SXMAIN). The channel decouples the two.
enum KeyCommand {
    AddKey {
        key: [u8; Ed25519KeyOption::PVT_KEY_SIZE],
        key_id: Arc<KeyId>,
        bind_addr: SocketAddr,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    RemoveKey {
        key_id: Arc<KeyId>,
        bind_addr: SocketAddr,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    ActivateKey {
        key_id: Arc<KeyId>,
    },
}

pub struct QuicNode {
    cancellation_token: tokio_util::sync::CancellationToken,
    /// One entry per local identity; each carries its own client config and outbound pool.
    local_keys: lockfree::map::Map<Arc<KeyId>, Arc<LocalKeyState>>,
    /// One endpoint per unique bind port. Endpoints are created lazily by `add_key()`.
    endpoints: Mutex<HashMap<u16, Arc<EndpointState>>>,
    /// Shared subscriber list for all accept loops.
    subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    peer_keys: lockfree::map::Map<Arc<KeyId>, SocketAddr>,
    /// Inbound connection maps, one per endpoint/accept-loop. Used by the stats dumper.
    inbound_pools: Mutex<Vec<Arc<QuicInboundMap>>>,
    /// Per-TL-tag message counters for the stats dumper.
    msg_stats: Arc<MsgStats>,
    /// Channel for dispatching key operations to a Tokio-hosted background task.
    key_cmd_tx: tokio::sync::mpsc::UnboundedSender<KeyCommand>,
    /// Aggregate error counters (reset each stats dump interval).
    transport_errors: Arc<TransportErrors>,
    /// Rate limiting configuration for inbound QUIC connections.
    rate_limit_config: QuicRateLimitConfig,
}

impl QuicNode {
    pub const OFFSET_PORT: u16 = 1000;

    /// How often the background checker scans outbound connections for dead ones.
    const CONNECTION_CHECK_INTERVAL: Duration = Duration::from_secs(5);
    /// How often the stats dumper logs connection statistics.
    const STATS_DUMP_INTERVAL: Duration = Duration::from_secs(60);
    const DEFAULT_QUERY_TIMEOUT_MS: u64 = 5000;
    /// Maximum number of messages buffered per outbound peer
    const SEND_QUEUE_CAPACITY: usize = 1024;
    /// Timeout for QUIC handshake when connecting to a peer.
    /// C++ ngtcp2 abandons after ~3-5s, so 5s is a reasonable upper bound.

    // --- Key fallback on rapid reconnection ---
    // When a C++ client keeps connecting and immediately disconnecting (key
    // mismatch after validator key rotation), the server detects the pattern
    // and presents an older key as a fallback.
    //
    /// Enable the key fallback mechanism. When `false`, the server always
    /// presents the most recently registered key (last_added_name).
    const KEY_FALLBACK_ENABLED: bool = false;
    /// Time window for counting rapid reconnection attempts from the same IP.
    const KEY_FALLBACK_WINDOW: Duration = Duration::from_secs(60);
    /// Number of connection attempts within the window before triggering fallback.
    const KEY_FALLBACK_THRESHOLD: u32 = 3;
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

    /// Backoff schedule: first 2 attempts (cycle 1) have no delay, then groups
    /// of 10 attempts (5 cycles) increase by 5s each, capped at 30s.
    ///   attempts 1-2   → 0s
    ///   attempts 3-12  → 5s
    ///   attempts 13-22 → 10s
    ///   attempts 23-32 → 15s
    ///   attempts 33-42 → 20s
    ///   attempts 43-52 → 25s
    ///   attempts 53+   → 30s (capped)
    fn connect_backoff(prev_attempts: u64) -> Duration {
        if prev_attempts < 2 {
            return Duration::ZERO;
        }
        let group = (prev_attempts - 2) / 10;
        let secs = ((group + 1) * 5).min(30);
        Duration::from_secs(secs)
    }

    /// Create a new QuicNode. No endpoints are bound — they are created lazily
    /// by `add_key()` when the first identity for a given port is registered.
    ///
    /// `runtime_handle` is used to spawn a background task that processes key
    /// operations requiring Tokio context (Quinn endpoint creation).
    pub fn new(
        subscribers: Vec<Arc<dyn Subscriber>>,
        cancellation_token: tokio_util::sync::CancellationToken,
        runtime_handle: tokio::runtime::Handle,
        rate_limit_config: Option<QuicRateLimitConfig>,
    ) -> Arc<Self> {
        static CRYPTO_INIT: Once = Once::new();
        CRYPTO_INIT.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install default Rustls CryptoProvider");
        });
        let (key_cmd_tx, mut key_cmd_rx) = tokio::sync::mpsc::unbounded_channel::<KeyCommand>();
        let transport = Arc::new(Self {
            cancellation_token: cancellation_token.clone(),
            local_keys: lockfree::map::Map::new(),
            endpoints: Mutex::new(HashMap::new()),
            subscribers: Arc::new(subscribers),
            peer_keys: lockfree::map::Map::new(),
            inbound_pools: Mutex::new(Vec::new()),
            msg_stats: MsgStats::new(),
            key_cmd_tx,
            transport_errors: TransportErrors::new(),
            rate_limit_config: rate_limit_config.unwrap_or_default(),
        });
        // Spawn background task that processes key commands inside the Tokio runtime.
        let weak = Arc::downgrade(&transport);
        let token = cancellation_token.clone();
        runtime_handle.spawn(async move {
            loop {
                tokio::select! {
                    cmd = key_cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break };
                        match cmd {
                            KeyCommand::AddKey { key, key_id, bind_addr, reply } => {
                                let result = if let Some(this) = weak.upgrade() {
                                    this.add_key_inner(&key, &key_id, bind_addr)
                                } else {
                                    Err(error!("QuicNode dropped"))
                                };
                                let _ = reply.send(result);
                            }
                            KeyCommand::RemoveKey { key_id, bind_addr, reply } => {
                                let result = if let Some(this) = weak.upgrade() {
                                    this.remove_key_inner(&key_id, bind_addr)
                                } else {
                                    Err(error!("QuicNode dropped"))
                                };
                                let _ = reply.send(result);
                            }
                            KeyCommand::ActivateKey { key_id } => {
                                if let Some(this) = weak.upgrade() {
                                    this.activate_key_inner(&key_id);
                                }
                            }
                        }
                    }
                    _ = token.cancelled() => break,
                }
            }
        });
        Self::spawn_connection_checker(Arc::downgrade(&transport), cancellation_token.clone());
        Self::spawn_stats_dumper(Arc::downgrade(&transport), cancellation_token);
        transport
    }

    /// Register a local identity on a specific bind address.
    /// Creates a new endpoint if one doesn't exist for this port yet.
    ///
    /// Safe to call from any thread — the actual work is dispatched to a
    /// Tokio-hosted background task via an internal channel.
    pub fn add_key(
        &self,
        key: &[u8; Ed25519KeyOption::PVT_KEY_SIZE],
        key_id: &Arc<KeyId>,
        bind_addr: SocketAddr,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.key_cmd_tx
            .send(KeyCommand::AddKey {
                key: *key,
                key_id: key_id.clone(),
                bind_addr,
                reply: reply_tx,
            })
            .map_err(|_| error!("QuicNode key command channel closed"))?;
        // Use blocking_recv on bare OS threads, tokio::block_in_place on Tokio workers.
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| {
                reply_rx
                    .blocking_recv()
                    .map_err(|_| error!("QuicNode key command reply channel dropped"))?
            }),
            Err(_) => reply_rx
                .blocking_recv()
                .map_err(|_| error!("QuicNode key command reply channel dropped"))?,
        }
    }

    /// Internal implementation of add_key — always runs inside the Tokio runtime
    /// (called by the background key-command task).
    fn add_key_inner(
        &self,
        key: &[u8; Ed25519KeyOption::PVT_KEY_SIZE],
        key_id: &Arc<KeyId>,
        bind_addr: SocketAddr,
    ) -> Result<()> {
        let key_bytes = ed25519_encode_private_key_to_pkcs8(key)?;
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_bytes)
            .map_err(|e| error!("Cannot convert private key to DER: {e}"))?;
        let key_pair = rcgen::KeyPair::from_der_and_sign_algo(&key_der, &rcgen::PKCS_ED25519)?;
        let pub_key_bytes = key_pair.public_key_der();

        // Client cert: SPKI DER (RPK) — presented when connecting outbound.
        let client_spki = rustls::pki_types::CertificateDer::from(pub_key_bytes.clone());
        let client_signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| error!("Cannot create client signing key: {e}"))?;
        let client_certified_key =
            Arc::new(rustls::sign::CertifiedKey::new(vec![client_spki], client_signing_key));
        let mut client_tls = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(QuicServerCertVerifier))
            .with_client_cert_resolver(Arc::new(QuicCertResolver(client_certified_key)));
        client_tls.alpn_protocols = vec![b"ton".to_vec()];

        let mut quinn_client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_tls)
                .map_err(|e| error!("Cannot create QUIC client config: {e}"))?,
        ));
        // Match server-side timeouts so both ends agree on connection liveness.
        let mut client_transport = quinn::TransportConfig::default();
        client_transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(Duration::from_secs(15)).expect("15s fits in IdleTimeout"),
        ));
        client_transport.keep_alive_interval(Some(Duration::from_secs(5)));
        client_transport
            .congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        quinn_client_config.transport_config(Arc::new(client_transport));

        let local_key_state = Arc::new(LocalKeyState {
            client_config: quinn_client_config,
            outbound: Connections::new(),
            bound_port: bind_addr.port(),
        });
        add_unbound_object_to_map(
            &self.local_keys,
            key_id.clone(),
            || Ok(local_key_state.clone()),
        )?;

        // Get or create endpoint for this port
        let endpoint_state = self.get_or_create_endpoint(bind_addr)?;

        // Server cert: SPKI DER (RPK) — presented when accepting inbound connections.
        let name = Self::key_id_to_server_name(key_id);
        let server_spki = rustls::pki_types::CertificateDer::from(pub_key_bytes);
        let server_signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| error!("Cannot create server signing key: {e}"))?;
        add_unbound_object_to_map(&*endpoint_state.server_cert_keys, name.clone(), || {
            Ok(Arc::new(rustls::sign::CertifiedKey::new(
                vec![server_spki.clone()],
                server_signing_key.clone(),
            )))
        })?;

        // Register the key name → key id mapping for SNI resolution on this endpoint
        add_unbound_object_to_map(&*endpoint_state.local_key_names, name.clone(), || {
            Ok(key_id.clone())
        })?;

        // Build a dedicated ServerConfig for this key (used by the fallback mechanism
        // when a peer keeps reconnecting and failing with the default/newest key).
        if let Some(cert_entry) = endpoint_state.server_cert_keys.get(&name) {
            match Self::build_single_key_server_config(cert_entry.val().clone()) {
                Ok(cfg) => {
                    if let Ok(mut configs) = endpoint_state.per_key_configs.lock() {
                        configs.insert(name.clone(), Arc::new(cfg));
                    }
                }
                Err(e) => log::warn!(
                    target: TARGET,
                    "Cannot build per-key ServerConfig for {key_id}: {e}"
                ),
            }
        }

        // Auto-activate the first key so the server always has an active identity
        if let Ok(mut last) = endpoint_state.last_added_name.lock() {
            if last.is_none() {
                *last = Some(name);
                log::info!(
                    target: TARGET,
                    "Registered and auto-activated QUIC identity {} on port {}",
                    key_id, bind_addr.port()
                );
                return Ok(());
            }
        }

        log::info!(
            target: TARGET,
            "Registered QUIC identity {} on port {}",
            key_id, bind_addr.port()
        );

        Ok(())
    }

    /// Unregister a local identity from a specific bind address.
    /// Removes the key from the server cert resolver, local key names, and local
    /// key state. After this call the QUIC server will no longer present this
    /// key's RPK certificate to connecting peers.
    pub fn remove_key(&self, key_id: &Arc<KeyId>, bind_addr: SocketAddr) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.key_cmd_tx
            .send(KeyCommand::RemoveKey { key_id: key_id.clone(), bind_addr, reply: reply_tx })
            .map_err(|_| error!("QuicNode key command channel closed"))?;
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| {
                reply_rx
                    .blocking_recv()
                    .map_err(|_| error!("QuicNode key command reply channel dropped"))?
            }),
            Err(_) => reply_rx
                .blocking_recv()
                .map_err(|_| error!("QuicNode key command reply channel dropped"))?,
        }
    }

    /// Internal implementation of remove_key — always runs inside the Tokio runtime.
    fn remove_key_inner(&self, key_id: &Arc<KeyId>, bind_addr: SocketAddr) -> Result<()> {
        let port = bind_addr.port();
        let endpoints = self.endpoints.lock().map_err(|e| error!("Endpoints lock: {e}"))?;
        let Some(endpoint_state) = endpoints.get(&port) else {
            fail!("No QUIC endpoint on port {port} for key {key_id}");
        };

        let name = Self::key_id_to_server_name(key_id);

        // Remove from server cert resolver map
        endpoint_state.server_cert_keys.remove(&name);

        // Remove from local key name → key id mapping
        endpoint_state.local_key_names.remove(&name);

        // Update last_added_name: if it was pointing to the removed key,
        // switch to another remaining key (or None).
        if let Ok(mut last) = endpoint_state.last_added_name.lock() {
            if last.as_deref() == Some(&name) {
                *last = endpoint_state.local_key_names.iter().next().map(|e| e.key().clone());
            }
        }

        // Remove per-key ServerConfig for fallback
        if let Ok(mut configs) = endpoint_state.per_key_configs.lock() {
            configs.remove(&name);
        }

        // Remove from local key state (outbound connections for this identity)
        self.local_keys.remove(key_id);

        log::info!(
            target: TARGET,
            "Unregistered QUIC identity {} from port {}",
            key_id, bind_addr.port()
        );

        Ok(())
    }

    /// Activate a previously added key as the current SNI fallback identity.
    /// Called when the validator set containing this key becomes active.
    pub fn activate_key(&self, key_id: &Arc<KeyId>) {
        let _ = self.key_cmd_tx.send(KeyCommand::ActivateKey { key_id: key_id.clone() });
    }

    fn activate_key_inner(&self, key_id: &Arc<KeyId>) {
        let Some(local_key) = self.local_keys.get(key_id) else {
            log::warn!(target: TARGET, "activate_key: unknown key {key_id}");
            return;
        };
        let port = local_key.val().bound_port;
        let Ok(endpoints) = self.endpoints.lock() else {
            log::error!(target: TARGET, "activate_key: endpoints lock poisoned");
            return;
        };
        let Some(endpoint_state) = endpoints.get(&port) else {
            log::warn!(target: TARGET, "activate_key: no endpoint on port {port} for key {key_id}");
            return;
        };
        let name = Self::key_id_to_server_name(key_id);
        // Update last-added name for SNI fallback (C++ ngtcp2 doesn't send SNI)
        let changed = if let Ok(mut last) = endpoint_state.last_added_name.lock() {
            if last.as_deref() == Some(name.as_str()) {
                false
            } else {
                *last = Some(name);
                true
            }
        } else {
            false
        };
        if changed {
            log::info!(target: TARGET, "Activated QUIC identity {} on port {}", key_id, port);
        }
    }

    pub fn add_peer_key(&self, key_id: Arc<KeyId>, addr: SocketAddr) -> Result<()> {
        add_unbound_object_to_map_with_update(&self.peer_keys, key_id, |_| Ok(Some(addr)))?;
        Ok(())
    }

    pub fn has_peer_key(&self, key_id: &Arc<KeyId>) -> bool {
        self.peer_keys.get(key_id).is_some()
    }

    pub async fn message(
        self: &Arc<Self>,
        data: Vec<u8>,
        adnl: Option<&AdnlNode>,
        peers: &AdnlPeers,
    ) -> Result<Option<usize>> {
        let t0 = Instant::now();
        self.ensure_peer_registered(adnl, peers)?;
        let tag = extract_inner_tag(&data);
        let size = data.len();
        let data = serialize_boxed(&QuicMessage { data: data.into() }.into_boxed())?;
        let addr = self.addr_by_key(peers.other())?;
        let state = self.local_key_state(peers.local())?;
        let outbound = Self::get_or_create_outbound_connection(&state.outbound, addr)?;
        let t_prep = t0.elapsed();

        // Fast path: if connection is alive, send directly without queue overhead
        if let Some(ref conn) = outbound.conn {
            match Self::send_via_stream_nowait(conn, &data).await {
                Ok(()) => {
                    self.msg_stats.record(tag, size, addr, true, MsgKind::Message);
                    let t_total = t0.elapsed();
                    if t_total > Duration::from_millis(10) {
                        log::warn!(
                            target: TARGET,
                            "QUIC message() SLOW to {addr}: \
                            prep={:.1}ms send={:.1}ms total={:.1}ms tag={tag:08x} size={size}",
                            t_prep.as_secs_f64() * 1000.0,
                            (t_total - t_prep).as_secs_f64() * 1000.0,
                            t_total.as_secs_f64() * 1000.0,
                        );
                    } else {
                        log::trace!(
                            target: TARGET,
                            "QUIC message() to {addr}: \
                            prep={:.1}ms send={:.1}ms total={:.1}ms tag={tag:08x} size={size}",
                            t_prep.as_secs_f64() * 1000.0,
                            (t_total - t_prep).as_secs_f64() * 1000.0,
                            t_total.as_secs_f64() * 1000.0,
                        );
                    }
                    return Ok(Some(data.len()));
                }
                Err(e) => {
                    self.transport_errors.send_failed.fetch_add(1, Ordering::Relaxed);
                    log::warn!(
                        target: TARGET,
                        "QUIC direct send to {} failed: {e}, removing dead connection, \
                        falling back to queue",
                        peers.other()
                    );
                    Self::remove_dead_connection(&state.outbound, addr, conn);
                    self.transport_errors.dead_conn_removed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Slow path: no connection (or it just died) — enqueue for the sender task
        // which will establish the connection and deliver
        if !outbound.send_queue.try_push(data) {
            self.transport_errors.queue_full.fetch_add(1, Ordering::Relaxed);
            fail!("QUIC send queue full for peer {}", peers.other());
        }
        self.msg_stats.record(tag, size, addr, true, MsgKind::Message);

        // Spawn sender task if not already running (CAS guarantees at most one per peer)
        if outbound
            .sender_state
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let quic = self.clone();
            let send_queue = outbound.send_queue.clone();
            let sender_state = outbound.sender_state.clone();
            let outbound_conns = state.outbound.clone();
            let server_name = Self::key_id_to_server_name(peers.other());

            spawn_cancelable(
                self.cancellation_token.clone(),
                Self::run_sender_task(
                    quic,
                    peers.clone(),
                    addr,
                    server_name,
                    send_queue,
                    sender_state,
                    outbound_conns,
                ),
            );
        }

        Ok(None)
    }

    pub async fn query(
        self: &Arc<Self>,
        data: Vec<u8>,
        adnl: Option<&AdnlNode>,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
    ) -> Result<Option<Vec<u8>>> {
        self.ensure_peer_registered(adnl, peers)?;
        let addr = self.addr_by_key(peers.other())?;
        let tag = extract_inner_tag(&data);
        let size = data.len();
        let timeout_ms = timeout_ms.unwrap_or(Self::DEFAULT_QUERY_TIMEOUT_MS);
        let wire = serialize_boxed(&QuicQuery { data: data.into() }.into_boxed())?;
        self.msg_stats.record(tag, size, addr, true, MsgKind::Query);
        let response = match self.send_query_raw(wire, peers, timeout_ms).await {
            Ok(r) => r,
            Err(e) => {
                self.msg_stats.record(tag, 0, addr, false, MsgKind::NoAnswer);
                return Err(e);
            }
        };
        if response.is_empty() {
            self.msg_stats.record(tag, 0, addr, false, MsgKind::NoAnswer);
            return Ok(None);
        }
        let obj = deserialize_boxed(&response)
            .map_err(|e| error!("Cannot deserialise QUIC answer: {e}"))?;
        match obj.downcast::<QuicResponse>() {
            Ok(QuicResponse::Quic_Answer(answer)) => {
                self.msg_stats.record(tag, answer.data.len(), addr, false, MsgKind::Answer);
                Ok(Some(answer.data.to_vec()))
            }
            Err(x) => fail!("Unexpected QUIC response type {x:?}"),
        }
    }

    /// Shut down all QUIC endpoints, cancel background tasks, and release resources.
    pub fn shutdown(&self) {
        // Cancel all background tasks (accept loops, connection checkers, drain tasks)
        self.cancellation_token.cancel();

        // Close all outbound connections in every local key state
        for entry in self.local_keys.iter() {
            let outbound = &entry.val().outbound;
            for conn_entry in outbound.map().iter() {
                if let Some(ref conn) = conn_entry.val().conn {
                    conn.close(0u32.into(), b"shutdown");
                }
            }
        }

        // Close all endpoints
        if let Ok(endpoints) = self.endpoints.lock() {
            for (port, state) in endpoints.iter() {
                log::info!(target: TARGET, "Shutting down QUIC endpoint on port {port}");
                state.endpoint.close(0u32.into(), b"shutdown");
            }
        }
    }

    fn addr_by_key(&self, key_id: &Arc<KeyId>) -> Result<SocketAddr> {
        match self.peer_keys.get(key_id) {
            Some(entry) => Ok(*entry.val()),
            None => fail!("No address registered for peer key {key_id}"),
        }
    }

    async fn connect(&self, peers: &AdnlPeers, addr: SocketAddr, server_name: &str) -> Result<()> {
        let dst = peers.other();
        let state = self.local_key_state(peers.local())?;

        // Check if a live connection already exists — avoid creating a duplicate
        // that would be immediately closed and disrupt the peer's accept loop.
        if let Some(entry) = state.outbound.map().get(&addr) {
            if let Some(ref conn) = entry.val().conn {
                if conn.close_reason().is_none() {
                    log::trace!(
                        target: TARGET,
                        "QUIC connect to {addr}: reusing existing live connection"
                    );
                    return Ok(());
                }
            }
        }

        let endpoint = {
            let endpoints = self.endpoints.lock().map_err(|e| error!("Endpoints lock: {e}"))?;
            endpoints
                .get(&state.bound_port)
                .map(|s| s.endpoint.clone())
                .ok_or_else(|| error!("No QUIC endpoint for port {}", state.bound_port))?
        };
        let conn = endpoint
            .connect_with(state.client_config.clone(), addr, server_name)
            .map_err(|e| error!("QUIC connect to {addr} (SNI={server_name}): {e}"))?
            .await
            .map_err(|e| error!("QUIC handshake to {addr}: {e}"))?;

        let peer_id = peer_key_id_from_connection(&conn).ok_or_else(|| {
            conn.close(0u32.into(), b"No peer RPK");
            error!("QUIC connect to {addr}: no peer RPK identity")
        })?;
        if peer_id.as_ref() != dst.as_ref() {
            conn.close(0u32.into(), b"RPK identity mismatch");
            fail!("QUIC RPK mismatch connecting to {addr}: expected {dst}, got {peer_id}");
        }

        let stored = state.outbound.set_connection_state(addr, |found| {
            if found.conn.is_none() {
                Ok(Some(QuicOutboundConnection {
                    conn: Some(conn.clone()),
                    send_queue: found.send_queue.clone(),
                    sender_state: found.sender_state.clone(),
                }))
            } else {
                Ok(None)
            }
        })?;
        if !stored {
            // Another thread won the race. Don't close our connection — the peer
            // may already be using it for inbound stream processing. Park it so
            // quinn's idle timeout cleans it up gracefully.
            log::debug!(
                target: TARGET,
                "QUIC connect to {addr}: another connection stored first, \
                parking ours for idle-timeout cleanup"
            );
            self.park_superseded_connection(conn);
        }
        Ok(())
    }

    /// Keep a superseded connection alive until quinn's idle timeout expires,
    /// so the peer's accept loop isn't disrupted by an abrupt close.
    fn park_superseded_connection(&self, conn: quinn::Connection) {
        let token = self.cancellation_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = conn.closed() => {}
                _ = token.cancelled() => {
                    conn.close(0u32.into(), b"shutdown");
                }
            }
        });
    }

    /// Obtain (or create) an outbound connection and connect in the foreground.
    /// Used by the query path where a live connection is required synchronously.
    async fn ensure_outbound_connection(
        self: &Arc<Self>,
        peers: &AdnlPeers,
    ) -> Result<QuicOutboundConnection> {
        let addr = self.addr_by_key(peers.other())?;
        let server_name = Self::key_id_to_server_name(peers.other());
        let state = self.local_key_state(peers.local())?;
        loop {
            let conn = Self::get_or_create_outbound_connection(&state.outbound, addr)?;
            if conn.conn.is_some() {
                break Ok(conn);
            }
            log::info!(target: TARGET, "Try new QUIC connection to {addr} in foreground");
            if let Err(e) = self.connect(peers, addr, &server_name).await {
                self.transport_errors.connect_failed.fetch_add(1, Ordering::Relaxed);
                return Err(e);
            }
            log::info!(target: TARGET, "QUIC connected to {addr} in foreground");
        }
    }

    fn ensure_peer_registered(&self, adnl: Option<&AdnlNode>, peers: &AdnlPeers) -> Result<()> {
        let dst = peers.other();
        if self.has_peer_key(dst) {
            return Ok(());
        }
        let Some(adnl) = adnl else {
            fail!("QUIC peer {dst} is not registered and no ADNL node provided");
        };
        // Get peer's ADNL and optional QUIC addresses
        let (adnl_addr, quic_addr) = adnl
            .peer_ip_address(peers.local(), dst)?
            .ok_or_else(|| error!("QUIC peer {dst} IP is not known in ADNL"))?;
        // Prefer explicit QUIC address from peer's address list (adnl.address.quic)
        if let Some(quic_addr) = quic_addr {
            return self.add_peer_key(dst.clone(), quic_addr);
        }
        // Fallback: derive QUIC port from ADNL port + offset
        let mut addr = adnl_addr;
        let quic_port = addr.port().checked_add(Self::OFFSET_PORT).ok_or_else(|| {
            error!("QUIC port overflow for peer {dst}: ADNL port {}", addr.port())
        })?;
        addr.set_port(quic_port);
        self.add_peer_key(dst.clone(), addr)
    }

    /// Get or create a quinn endpoint for the given bind address.
    fn get_or_create_endpoint(&self, bind_addr: SocketAddr) -> Result<Arc<EndpointState>> {
        let port = bind_addr.port();
        let mut endpoints = self.endpoints.lock().map_err(|e| error!("Endpoints lock: {e}"))?;

        if let Some(state) = endpoints.get(&port) {
            return Ok(state.clone());
        }

        // Create per-endpoint TLS state
        let server_cert_keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>> =
            Arc::new(lockfree::map::Map::new());
        let last_added_name: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let verifier = QuicClientCertVerifier::new();
        let server_cert_resolver =
            QuicServerCertResolver::new(server_cert_keys.clone(), last_added_name.clone());
        let mut tls_config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier.clone())
            .with_cert_resolver(server_cert_resolver.clone());
        tls_config.alpn_protocols = vec![b"ton".to_vec()];

        let mut quinn_server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
                .map_err(|e| error!("Cannot create QUIC server config: {e}"))?,
        ));

        // Increase max concurrent bidi streams above the default (~100): each overlay
        // message from C++ opens a new stream. Capped at 1000 (not higher) to limit
        // memory exposure from a single malicious peer.
        let mut transport_config = quinn::TransportConfig::default();
        transport_config.max_concurrent_bidi_streams(1_000u32.into());
        // Prevent stale half-open connections from accumulating inside the endpoint.
        // C++ ngtcp2 clients abandon after ~3-5s; without this, quinn keeps dead
        // connections for the default 30s, overloading the internal event loop and
        // making endpoint.accept() slow (the "HoL blocking" symptom).
        transport_config.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(Duration::from_secs(15)).expect("15s fits in IdleTimeout"),
        ));
        // Keep established connections alive so the idle timeout only fires on
        // truly dead peers, not on connections that are just quiet between rounds.
        transport_config.keep_alive_interval(Some(Duration::from_secs(5)));
        transport_config
            .congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        quinn_server_config.transport_config(Arc::new(transport_config));

        // Create UDP socket with SO_REUSEADDR so the port can be reused immediately
        // after shutdown (important for quick restarts and peer recovery).
        let udp_socket = {
            let sock = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(|e| error!("Cannot create UDP socket: {e}"))?;
            sock.set_reuse_address(true).map_err(|e| error!("Cannot set SO_REUSEADDR: {e}"))?;
            sock.bind(&bind_addr.into())
                .map_err(|e| error!("Cannot bind UDP socket to {bind_addr}: {e}"))?;
            sock.set_nonblocking(true).map_err(|e| error!("Cannot set non-blocking: {e}"))?;
            UdpSocket::from(sock)
        };
        // Probe the UDP socket's hardware/OS offload capabilities before moving
        // it into the quinn endpoint. GSO/GRO status is useful for diagnosing
        // throughput differences across hosts.
        match quinn::udp::UdpSocketState::new((&udp_socket).into()) {
            Ok(state) => log::info!(
                target: TARGET,
                "QUIC UDP caps on {bind_addr}: max_gso_segments={}, gro_segments={}, may_fragment={}",
                state.max_gso_segments(),
                state.gro_segments(),
                state.may_fragment(),
            ),
            Err(e) => log::warn!(
                target: TARGET,
                "QUIC UDP caps probe failed on {bind_addr}: {e}"
            ),
        }
        let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
        let endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(quinn_server_config),
            udp_socket,
            runtime,
        )
        .map_err(|e| error!("Cannot create QUIC endpoint on {bind_addr}: {e}"))?;

        let local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>> =
            Arc::new(lockfree::map::Map::new());

        let inbound: Arc<QuicInboundMap> = Arc::new(lockfree::map::Map::new());
        match self.inbound_pools.lock() {
            Ok(mut pools) => pools.push(inbound.clone()),
            Err(e) => log::warn!(
                target: TARGET,
                "inbound_pools lock poisoned, inbound stats will be incomplete: {e}"
            ),
        }

        let per_key_configs = Arc::new(Mutex::new(HashMap::new()));
        let reconnect_tracker = Arc::new(Mutex::new(HashMap::new()));

        let rl_config = self.rate_limit_config.clone();
        let conn_rate_limiters =
            ConnectionRateLimiters::new(rl_config.per_ip_capacity, rl_config.per_ip_period);
        let global_rate_limiter = if rl_config.global_capacity > 0 {
            Some(RateLimiter::new(rl_config.global_capacity, rl_config.global_period))
        } else {
            None
        };

        Self::spawn_accept_loop(
            endpoint.clone(),
            local_key_names.clone(),
            server_cert_resolver,
            self.subscribers.clone(),
            bind_addr,
            self.cancellation_token.clone(),
            inbound,
            self.msg_stats.clone(),
            per_key_configs.clone(),
            reconnect_tracker.clone(),
            rl_config,
            conn_rate_limiters,
            global_rate_limiter,
            self.transport_errors.clone(),
        );

        let state = Arc::new(EndpointState {
            endpoint,
            server_cert_keys,
            local_key_names,
            last_added_name,
            per_key_configs,
            reconnect_tracker,
        });
        endpoints.insert(port, state.clone());

        log::info!(target: TARGET, "Created QUIC endpoint on {bind_addr}");
        Ok(state)
    }

    fn get_or_create_outbound_connection(
        outbound: &Connections<QuicOutboundConnection>,
        addr: SocketAddr,
    ) -> Result<QuicOutboundConnection> {
        loop {
            match outbound.map().get(&addr) {
                Some(entry) => {
                    let found = entry.val();
                    if let Some(ref c) = found.conn {
                        if c.close_reason().is_some() {
                            log::info!(
                                target: TARGET,
                                "Proactive removal: dead QUIC outbound to {addr} \
                                 (close_reason={:?})",
                                c.close_reason()
                            );
                            Self::remove_dead_connection(outbound, addr, c);
                            continue;
                        }
                    }
                    break Ok(QuicOutboundConnection {
                        conn: found.conn.clone(),
                        send_queue: found.send_queue.clone(),
                        sender_state: found.sender_state.clone(),
                    });
                }
                None => {
                    let queue = QuicSendQueue::with_capacity(Self::SEND_QUEUE_CAPACITY);
                    let sender_state = SenderState::new();
                    add_unbound_object_to_map(outbound.map(), addr, || {
                        Ok(QuicOutboundConnection {
                            conn: None,
                            send_queue: queue.clone(),
                            sender_state: sender_state.clone(),
                        })
                    })?;
                }
            }
        }
    }

    async fn handle_connection(
        incoming: quinn::Incoming,
        local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>>,
        server_cert_resolver: Arc<QuicServerCertResolver>,
        inbound: Arc<QuicInboundMap>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        bind_addr: SocketAddr,
        msg_stats: Arc<MsgStats>,
        fallback_config: Option<Arc<quinn::ServerConfig>>,
        reconnect_tracker: Arc<Mutex<HashMap<IpAddr, ReconnectTracker>>>,
    ) {
        let addr = incoming.remote_address();
        // Use fallback ServerConfig if provided (older key for rapid-reconnect peers).
        let connecting = if let Some(config) = fallback_config {
            match incoming.accept_with(config) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(target: TARGET, "QUIC accept_with (fallback) from {addr}: {e}");
                    return;
                }
            }
        } else {
            match incoming.accept() {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(target: TARGET, "QUIC accept from {addr}: {e}");
                    return;
                }
            }
        };
        // Bound handshake time: C++ ngtcp2 clients abandon after ~3-5s and retry,
        // so a handshake still in progress after 5s is almost certainly stale.
        // Without this, stale Connecting futures accumulate inside quinn's endpoint,
        // slowing its internal event loop and delaying endpoint.accept() for new peers.
        let conn = match tokio::time::timeout(Duration::from_secs(5), connecting).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                log::warn!(target: TARGET, "QUIC handshake from {addr} on {bind_addr} failed: {e}");
                return;
            }
            Err(_) => {
                log::warn!(target: TARGET, "QUIC handshake from {addr} on {bind_addr} timed out (5s)");
                return;
            }
        };

        let peer_key_id = match peer_key_id_from_connection(&conn) {
            Some(key_id) => key_id,
            None => {
                log::warn!(
                    target: TARGET,
                    "No RPK cert from {addr}, closing connection"
                );
                conn.close(0u32.into(), b"No client RPK");
                return;
            }
        };

        let local_key_id = {
            let resolved = server_cert_resolver
                .last_added_name
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .and_then(|name| local_key_names.get(&name).map(|e| e.val().clone()));
            match resolved {
                Some(key_id) => key_id,
                None => match local_key_names.iter().next() {
                    Some(entry) => entry.val().clone(),
                    None => {
                        log::warn!(
                            target: TARGET,
                            "No local keys registered on {bind_addr}, closing {addr}"
                        );
                        conn.close(0u32.into(), b"No local keys");
                        return;
                    }
                },
            }
        };

        log::info!(
            target: TARGET,
            "Accepted QUIC connection from {addr} on {bind_addr} \
            local {local_key_id} peer {peer_key_id}"
        );

        // Keep all inbound connections (no dedup) so old ones stay until the peer
        // closes them.  Each connection gets a unique map slot via stable_id().
        let inbound_key =
            QuicInboundKey(local_key_id.clone(), peer_key_id.clone(), conn.stable_id());
        let _ = add_unbound_object_to_map(&inbound, inbound_key.clone(), || Ok(conn.clone()));

        let peers = AdnlPeers::with_keys(local_key_id, peer_key_id);
        let conn_id = conn.stable_id();

        // Accept both bi-directional streams (queries + legacy messages) and
        // uni-directional streams (fire-and-forget messages from the new sender).
        // Concurrency is bounded at the QUIC layer via
        // `TransportConfig::max_concurrent_bidi_streams` — no additional
        // user-level semaphore is needed.
        let streams_accepted = Arc::new(AtomicU64::new(0));
        let conn_bi = conn.clone();
        let conn_uni = conn.clone();
        let subs_bi = subscribers.clone();
        let subs_uni = subscribers;
        let peers_bi = peers.clone();
        let peers_uni = peers;
        let stats_bi = msg_stats.clone();
        let stats_uni = msg_stats;
        let streams_bi = streams_accepted.clone();
        let streams_uni = streams_accepted.clone();

        let bi_loop = async {
            loop {
                let (send, recv) = match conn_bi.accept_bi().await {
                    Ok(streams) => streams,
                    Err(e) => {
                        log::warn!(target: TARGET, "QUIC accept bi-stream from {addr}: {e}");
                        break;
                    }
                };
                streams_bi.fetch_add(1, Ordering::Relaxed);
                let subscribers = subs_bi.clone();
                let peers = peers_bi.clone();
                let stats = stats_bi.clone();
                tokio::spawn(async move {
                    if let Err(e) = Self::process_incoming_stream(
                        recv,
                        send,
                        &subscribers,
                        &peers,
                        addr,
                        &stats,
                    )
                    .await
                    {
                        log::warn!(target: TARGET, "QUIC process bi-stream from {addr}: {e}");
                    }
                });
            }
        };

        let uni_loop = async {
            loop {
                let recv = match conn_uni.accept_uni().await {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::warn!(target: TARGET, "QUIC accept uni-stream from {addr}: {e}");
                        break;
                    }
                };
                streams_uni.fetch_add(1, Ordering::Relaxed);
                let subscribers = subs_uni.clone();
                let peers = peers_uni.clone();
                let stats = stats_uni.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        Self::process_incoming_uni_stream(recv, &subscribers, &peers, addr, &stats)
                            .await
                    {
                        log::warn!(target: TARGET, "QUIC process uni-stream from {addr}: {e}");
                    }
                });
            }
        };

        // Run both accept loops; when either exits (connection closed), both stop.
        // Also monitor conn.closed() directly — if the remote peer disconnects
        // without ever opening streams (e.g., C++ key-mismatch abandon), we detect
        // it immediately instead of waiting for the 15s idle timeout.
        tokio::select! {
            () = bi_loop => {}
            () = uni_loop => {}
            reason = conn.closed() => {
                log::debug!(
                    target: TARGET,
                    "QUIC connection from {addr} closed early: {reason}"
                );
            }
        }
        let total_streams = streams_accepted.load(Ordering::Relaxed);
        inbound.remove(&inbound_key);

        // If the connection was productive (streams were opened), clear the
        // reconnect tracker for this IP. This resets the fallback state so
        // future connections from this peer use the default (newest) key again.
        if total_streams > 0 {
            if let Ok(mut tracker) = reconnect_tracker.lock() {
                tracker.remove(&addr.ip());
            }
        }

        log::info!(
            target: TARGET,
            "Exit QUIC inbound receiver for {addr} \
            (conn_id={conn_id}, streams={total_streams})"
        );
    }

    fn key_id_to_server_name(key_id: &KeyId) -> String {
        // DNS labels are limited to 63 chars; 64 hex chars → split into two 32-char labels
        let hex = hex::encode(key_id.data());
        log::trace!(target: TARGET, "key_id_to_server_name {} -> {}.{}", hex, &hex[..32], &hex[32..]);
        format!("{}.{}", &hex[..32], &hex[32..])
    }

    /// Build a ServerConfig that always presents a single fixed RPK certificate.
    /// Used by the fallback mechanism to present an older key to peers that
    /// keep failing with the default (newest) key.
    fn build_single_key_server_config(
        certified_key: Arc<rustls::sign::CertifiedKey>,
    ) -> Result<quinn::ServerConfig> {
        let resolver = Arc::new(QuicSingleKeyServerResolver(certified_key));
        let verifier = QuicClientCertVerifier::new();
        let mut tls_config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(resolver);
        tls_config.alpn_protocols = vec![b"ton".to_vec()];

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
                .map_err(|e| error!("Cannot create per-key QUIC server config: {e}"))?,
        ));
        let mut transport_config = quinn::TransportConfig::default();
        transport_config.max_concurrent_bidi_streams(1_000u32.into());
        transport_config.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(Duration::from_secs(15)).expect("15s fits in IdleTimeout"),
        ));
        transport_config.keep_alive_interval(Some(Duration::from_secs(5)));
        transport_config
            .congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        server_config.transport_config(Arc::new(transport_config));
        Ok(server_config)
    }

    fn local_key_state(&self, src: &Arc<KeyId>) -> Result<Arc<LocalKeyState>> {
        self.local_keys
            .get(src)
            .map(|e| e.val().clone())
            .ok_or_else(|| error!("No local key state for {src}"))
    }

    async fn process_incoming_stream(
        mut recv: quinn::RecvStream,
        mut send: quinn::SendStream,
        subscribers: &[Arc<dyn Subscriber>],
        peers: &AdnlPeers,
        addr: SocketAddr,
        msg_stats: &MsgStats,
    ) -> Result<()> {
        log::debug!(target: TARGET, "process_incoming_stream from {addr}: reading data...");
        let buf = match tokio::time::timeout(
            Duration::from_secs(5),
            recv.read_to_end(16 * 1024 * 1024), // 16MB limit
        )
        .await
        {
            Ok(result) => result.map_err(|e| error!("QUIC read from {addr}: {e}"))?,
            Err(_) => {
                log::warn!(
                    target: TARGET,
                    "process_incoming_stream from {addr}: read_to_end timed out after 5s \
                     (C++ ngtcp2 ACK misrouting may be stalling the connection). Dropping stream."
                );
                return Ok(());
            }
        };
        log::debug!(
            target: TARGET,
            "process_incoming_stream from {addr}: read {} bytes",
            buf.len()
        );
        if buf.is_empty() {
            return Ok(());
        }
        let obj = deserialize_boxed(&buf)
            .map_err(|e| error!("Cannot deserialize QUIC message from {addr}: {e}"))?;
        log::debug!(
            target: TARGET,
            "process_incoming_stream from {addr}: deserialized TL, about to downcast"
        );
        match obj.downcast::<Request>() {
            Ok(Request::Quic_Message(msg)) => {
                msg_stats.record(
                    extract_inner_tag(&msg.data),
                    msg.data.len(),
                    addr,
                    false,
                    MsgKind::Message,
                );
                // Ack immediately before processing — don't block the sender
                // while we dispatch to subscribers
                let _ = send.finish();
                log::debug!(
                    target: TARGET,
                    "process_incoming_stream from {addr}: QUIC MESSAGE, \
                    dispatching to {} subscribers",
                    subscribers.len()
                );
                for subscriber in subscribers {
                    if subscriber.try_consume_custom(&msg.data, &peers).await? {
                        log::debug!(
                            target: TARGET,
                            "process_incoming_stream from {addr}: consumed by subscriber"
                        );
                        break;
                    }
                }
            }
            Ok(Request::Quic_Query(query)) => {
                let query_tag = extract_inner_tag(&query.data);
                msg_stats.record(query_tag, query.data.len(), addr, false, MsgKind::Query);
                log::debug!(target: TARGET, "process_incoming_stream from {addr}: QUIC QUERY");
                let mut answered = false;
                let answer = Query::process(subscribers, &query.data, &peers).await?;
                if let Some(answer) = answer {
                    let answer = match answer {
                        QueryAnswer::Pending(handle) => handle.await??.answer,
                        QueryAnswer::Ready(a) => a,
                    };
                    if let Some(answer) = answer {
                        let data = match answer {
                            Answer::Object(tagged) => serialize_boxed(&tagged.object)?,
                            Answer::Raw(tagged) => tagged.object,
                        };
                        msg_stats.record(query_tag, data.len(), addr, true, MsgKind::Answer);
                        answered = true;
                        let response = QuicAnswer { data: data.into() }.into_boxed();
                        send.write_all(&serialize_boxed(&response)?)
                            .await
                            .map_err(|e| error!("QUIC write answer to {addr}: {e}"))?;
                    }
                }
                if !answered {
                    msg_stats.record(query_tag, 0, addr, true, MsgKind::NoAnswer);
                }
                let _ = send.finish();
            }
            Err(_obj) => {
                log::warn!(
                    target: TARGET,
                    "Unknown QUIC TL message from {addr}: failed to downcast to Request"
                );
            }
        }
        Ok(())
    }

    /// Process a fire-and-forget message received on a uni-directional QUIC stream.
    /// Only `QuicMessage` is expected; queries arriving on uni streams are rejected
    /// because there is no send side to write a response to.
    async fn process_incoming_uni_stream(
        mut recv: quinn::RecvStream,
        subscribers: &[Arc<dyn Subscriber>],
        peers: &AdnlPeers,
        addr: SocketAddr,
        msg_stats: &MsgStats,
    ) -> Result<()> {
        let buf =
            match tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(16 * 1024 * 1024))
                .await
            {
                Ok(result) => result.map_err(|e| error!("QUIC uni read from {addr}: {e}"))?,
                Err(_) => {
                    log::warn!(
                        target: TARGET,
                        "process_incoming_uni_stream from {addr}: read timed out after 5s"
                    );
                    return Ok(());
                }
            };
        if buf.is_empty() {
            return Ok(());
        }
        let obj = deserialize_boxed(&buf)
            .map_err(|e| error!("Cannot deserialize QUIC uni-stream from {addr}: {e}"))?;
        match obj.downcast::<Request>() {
            Ok(Request::Quic_Message(msg)) => {
                msg_stats.record(
                    extract_inner_tag(&msg.data),
                    msg.data.len(),
                    addr,
                    false,
                    MsgKind::Message,
                );
                for subscriber in subscribers {
                    if subscriber.try_consume_custom(&msg.data, peers).await? {
                        break;
                    }
                }
            }
            Ok(Request::Quic_Query(_)) => {
                log::warn!(
                    target: TARGET,
                    "Received QUIC query on uni-directional stream from {addr} — no response possible"
                );
            }
            Err(_) => {
                log::warn!(
                    target: TARGET,
                    "Unknown QUIC TL message on uni-stream from {addr}"
                );
            }
        }
        Ok(())
    }

    /// Atomically remove a dead outbound connection, resetting `conn` to `None`
    /// while preserving the `send_queue`. Uses `stable_id()` comparison to prevent
    /// ABA races: only removes if the stored connection is the exact same one detected
    /// as dead. Returns `true` if the dead connection was removed.
    fn remove_dead_connection(
        outbound: &Connections<QuicOutboundConnection>,
        addr: SocketAddr,
        dead_conn: &quinn::Connection,
    ) -> bool {
        let dead_id = dead_conn.stable_id();
        // Explicitly close the quinn connection so its internal ConnectionDriver
        // task stops immediately. Without this, the driver continues processing
        // keep-alive and retransmit timers until idle timeout (15s), causing the
        // EndpointDriver to busy-loop on timer events and burn 100% CPU.
        if dead_conn.close_reason().is_none() {
            dead_conn.close(0u32.into(), b"dead connection cleanup");
        }
        match outbound.set_connection_state(addr, |found| {
            if let Some(ref conn) = found.conn {
                if conn.stable_id() == dead_id {
                    log::info!(
                        target: TARGET,
                        "Removing dead QUIC outbound connection to {addr} (stable_id={dead_id})"
                    );
                    return Ok(Some(QuicOutboundConnection {
                        conn: None,
                        send_queue: found.send_queue.clone(),
                        sender_state: found.sender_state.clone(),
                    }));
                }
            }
            Ok(None)
        }) {
            Ok(removed) => removed,
            Err(e) => {
                log::warn!(
                    target: TARGET,
                    "remove_dead_connection to {addr}: {e}"
                );
                false
            }
        }
    }

    /// Drain the send queue and exit. Spawned when `message()` has no live
    /// connection and must enqueue data for later delivery. The task establishes
    /// the connection, sends all queued messages, and terminates.
    ///
    /// On connect failure the task waits 1s and retries once. If the retry also
    /// fails, all remaining queued messages are flushed (the peer is unreachable
    /// and retrying each message individually would stall for the full handshake
    /// timeout every time).
    ///
    /// When previous connect attempts have failed (counter persists in
    /// `SenderState`), the task applies a stepped backoff before the first
    /// attempt. Messages keep queuing during the backoff and are either
    /// delivered on success or flushed on failure.
    async fn run_sender_task(
        quic: Arc<Self>,
        peers: AdnlPeers,
        addr: SocketAddr,
        server_name: String,
        send_queue: Arc<QuicSendQueue>,
        sender_state: Arc<SenderState>,
        outbound: Arc<Connections<QuicOutboundConnection>>,
    ) {
        log::trace!(target: TARGET, "QUIC sender task started for {addr}");

        // Stepped backoff based on previous failed attempts (2 attempts per cycle).
        let prev_attempts = sender_state.connect_attempts.load(Ordering::Relaxed);
        let backoff = Self::connect_backoff(prev_attempts);
        if !backoff.is_zero() {
            log::info!(
                target: TARGET,
                "QUIC sender to {addr}: backoff {backoff:?} before connect \
                (previous attempts: {prev_attempts})"
            );
            tokio::time::sleep(backoff).await;
        }

        'outer: loop {
            // Drain the queue
            while let Some(data) = send_queue.pop() {
                match quic
                    .send_message(&peers, addr, &server_name, &outbound, &sender_state, &data)
                    .await
                {
                    Ok(()) => {}
                    Err(SendError::Temporary(e)) => {
                        log::warn!(target: TARGET, "QUIC sender to {addr} send error: {e}");
                    }
                    Err(SendError::Fatal(e)) => {
                        log::warn!(target: TARGET, "QUIC sender to {addr} connect error: {e}");
                        if send_queue.is_empty() {
                            break 'outer;
                        }
                        // Retry once after 1s
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        match quic
                            .send_message(
                                &peers,
                                addr,
                                &server_name,
                                &outbound,
                                &sender_state,
                                &data,
                            )
                            .await
                        {
                            Ok(()) => {}
                            Err(SendError::Temporary(e)) => {
                                log::warn!(
                                    target: TARGET,
                                    "QUIC sender to {addr} send error on retry: {e}"
                                );
                            }
                            Err(SendError::Fatal(e)) => {
                                let mut flushed = 0usize;
                                while send_queue.pop().is_some() {
                                    flushed += 1;
                                }
                                log::warn!(
                                    target: TARGET,
                                    "QUIC sender to {addr} connect retry failed: {e}, \
                                    flushed {flushed} queued messages"
                                );
                                break 'outer;
                            }
                        }
                    }
                }
            }

            // Mark inactive, then re-check: a new message may have been enqueued
            // between the last pop() returning None and the store below.
            sender_state.active.store(false, Ordering::Release);
            if send_queue.is_empty() {
                break;
            }
            // Lost race — reactivate if no other task took over
            if sender_state
                .active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                break; // another task took over
            }
        }

        log::trace!(target: TARGET, "QUIC sender task for {addr} exited");
    }

    /// Send a single message to the peer, establishing the connection first if needed.
    /// Returns `SendError::Fatal` when the connection cannot be established
    /// (peer unreachable) and `SendError::Temporary` when the send itself fails
    /// on an existing connection.
    async fn send_message(
        &self,
        peers: &AdnlPeers,
        addr: SocketAddr,
        server_name: &str,
        outbound: &Connections<QuicOutboundConnection>,
        sender_state: &SenderState,
        data: &[u8],
    ) -> std::result::Result<(), SendError> {
        let entry = Self::get_or_create_outbound_connection(outbound, addr)
            .map_err(SendError::Temporary)?;
        match entry.conn {
            Some(ref conn) => {
                if let Err(e) = Self::send_via_stream_nowait(conn, data).await {
                    self.transport_errors.send_failed.fetch_add(1, Ordering::Relaxed);
                    log::warn!(
                        target: TARGET,
                        "QUIC send to {addr} failed: {e}, removing dead connection"
                    );
                    Self::remove_dead_connection(outbound, addr, conn);
                    self.transport_errors.dead_conn_removed.fetch_add(1, Ordering::Relaxed);
                    return Err(SendError::Temporary(e));
                }
            }
            None => {
                let attempt = sender_state.next_attempt();
                log::info!(
                    target: TARGET,
                    "QUIC sender: connecting to {addr} (attempt {attempt})"
                );
                match tokio::time::timeout(
                    Self::CONNECT_TIMEOUT,
                    self.connect(peers, addr, server_name),
                )
                .await
                {
                    Ok(Ok(())) => {
                        sender_state.record_connect_success();
                    }
                    Ok(Err(e)) => {
                        self.transport_errors.connect_failed.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            target: TARGET,
                            "QUIC connect to {addr} failed: {e} \
                            (attempt {attempt}, last alive: {})",
                            sender_state.last_alive_ago()
                        );
                        return Err(SendError::Fatal(e));
                    }
                    Err(_) => {
                        self.transport_errors.connect_failed.fetch_add(1, Ordering::Relaxed);
                        let msg = format!(
                            "QUIC connect to {addr} timed out ({}s, \
                            attempt {attempt}, last alive: {})",
                            Self::CONNECT_TIMEOUT.as_secs(),
                            sender_state.last_alive_ago()
                        );
                        log::warn!(target: TARGET, "{msg}");
                        return Err(SendError::Fatal(error!("{msg}").into()));
                    }
                }
                log::info!(target: TARGET, "QUIC sender: connected to {addr}");
                let entry = Self::get_or_create_outbound_connection(outbound, addr)
                    .map_err(SendError::Temporary)?;
                if let Some(ref conn) = entry.conn {
                    Self::send_via_stream_nowait(conn, data).await.map_err(SendError::Temporary)?;
                } else {
                    return Err(SendError::Temporary(
                        error!("QUIC connection to {addr} lost after connect").into(),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn send_query_raw(
        self: &Arc<Self>,
        data: Vec<u8>,
        peers: &AdnlPeers,
        timeout_ms: u64,
    ) -> Result<Vec<u8>> {
        let addr = self.addr_by_key(peers.other())?;
        let state = self.local_key_state(peers.local())?;
        let timeout = Duration::from_millis(timeout_ms);

        // First attempt
        match self.ensure_outbound_connection(peers).await? {
            QuicOutboundConnection { conn: Some(ref conn), .. } => {
                let result =
                    tokio::time::timeout(timeout, Self::send_via_stream(conn, &data)).await;
                match result {
                    Ok(Ok(response)) => return Ok(response),
                    Ok(Err(e)) => {
                        self.transport_errors.send_failed.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} failed: {e}, removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                        self.transport_errors.dead_conn_removed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        self.transport_errors.query_timeout.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} timed out ({timeout_ms}ms), \
                            removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                        self.transport_errors.dead_conn_removed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            _ => fail!("Cannot create QUIC connection to {} in foreground", peers.other()),
        }

        // Retry once with a fresh connection
        match self.ensure_outbound_connection(peers).await? {
            QuicOutboundConnection { conn: Some(ref conn), .. } => {
                Self::send_via_stream(conn, &data).await
            }
            _ => fail!("Cannot create QUIC connection to {} in foreground (retry)", peers.other()),
        }
    }

    /// Fire-and-forget message send: opens a bidirectional stream, writes data,
    /// and returns immediately without waiting for a response.
    /// Uses bidi (not uni) streams because C++ ngtcp2 servers set
    /// `initial_max_streams_uni = 0` by default, rejecting uni-streams.
    /// Used by `message()` and `send_message()` where the response is not needed.
    async fn send_via_stream_nowait(conn: &quinn::Connection, data: &[u8]) -> Result<()> {
        let t0 = Instant::now();
        let addr = conn.remote_address();
        let (mut send, _recv) =
            conn.open_bi().await.map_err(|e| error!("Cannot open QUIC bi-stream: {e}"))?;
        let t_open = t0.elapsed();
        send.write_all(data).await.map_err(|e| error!("QUIC stream write: {e}"))?;
        let t_write = t0.elapsed();
        send.finish().map_err(|e| error!("QUIC stream finish: {e}"))?;
        let t_finish = t0.elapsed();
        // Drop _recv without reading — fire-and-forget, matching C++ behavior
        if t_finish > Duration::from_millis(10) {
            log::warn!(
                target: TARGET,
                "send_via_stream_nowait SLOW to {addr}: \
                open={:.1}ms write={:.1}ms finish={:.1}ms total={:.1}ms data_len={}",
                t_open.as_secs_f64() * 1000.0,
                (t_write - t_open).as_secs_f64() * 1000.0,
                (t_finish - t_write).as_secs_f64() * 1000.0,
                t_finish.as_secs_f64() * 1000.0,
                data.len()
            );
        } else {
            log::trace!(
                target: TARGET,
                "send_via_stream_nowait to {addr}: \
                open={:.1}ms write={:.1}ms finish={:.1}ms total={:.1}ms data_len={}",
                t_open.as_secs_f64() * 1000.0,
                (t_write - t_open).as_secs_f64() * 1000.0,
                (t_finish - t_write).as_secs_f64() * 1000.0,
                t_finish.as_secs_f64() * 1000.0,
                data.len()
            );
        }
        Ok(())
    }

    /// Request-response send: opens a bidirectional stream, writes data,
    /// and waits for the peer's response. Used by `send_query_raw()`.
    async fn send_via_stream(conn: &quinn::Connection, data: &[u8]) -> Result<Vec<u8>> {
        log::debug!(
            target: TARGET,
            "send_via_stream: opening bi-stream to {:?}, data_len={}",
            conn.remote_address(), data.len()
        );
        let (mut send, mut recv) =
            conn.open_bi().await.map_err(|e| error!("Cannot open QUIC bi-stream: {e}"))?;
        send.write_all(data).await.map_err(|e| error!("QUIC stream write: {e}"))?;
        send.finish().map_err(|e| error!("QUIC stream finish: {e}"))?;
        let response = match tokio::time::timeout(
            Duration::from_secs(30),
            recv.read_to_end(16 * 1024 * 1024), // 16MB limit
        )
        .await
        {
            Ok(result) => result.map_err(|e| error!("QUIC stream read response: {e}"))?,
            Err(_) => fail!(
                "QUIC stream read response from {:?} timed out after 30s",
                conn.remote_address()
            ),
        };
        log::debug!(
            target: TARGET,
            "send_via_stream: completed to {:?}, response_len={}",
            conn.remote_address(), response.len()
        );
        Ok(response)
    }

    /// Spawn the accept loop for a single endpoint.
    ///
    /// Each incoming connection handshake is spawned in a separate task so that
    /// slow or stale handshakes don't block new ones (head-of-line blocking fix).
    fn spawn_accept_loop(
        endpoint: quinn::Endpoint,
        local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>>,
        server_cert_resolver: Arc<QuicServerCertResolver>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        bind_addr: SocketAddr,
        cancellation_token: tokio_util::sync::CancellationToken,
        inbound: Arc<QuicInboundMap>,
        msg_stats: Arc<MsgStats>,
        per_key_configs: Arc<Mutex<HashMap<String, Arc<quinn::ServerConfig>>>>,
        reconnect_tracker: Arc<Mutex<HashMap<IpAddr, ReconnectTracker>>>,
        rl_config: QuicRateLimitConfig,
        mut conn_rate_limiters: ConnectionRateLimiters,
        mut global_rate_limiter: Option<RateLimiter>,
        transport_errors: Arc<TransportErrors>,
    ) {
        log::info!(
            target: TARGET,
            "QUIC accept loop on {bind_addr}: Retry={} per_ip_capacity={} global_capacity={}",
            if rl_config.stateless_retry { "on" } else { "off" },
            rl_config.per_ip_capacity,
            rl_config.global_capacity,
        );
        tokio::spawn(async move {
            loop {
                log::trace!(target: TARGET, "Loop QUIC server on {bind_addr}");
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        log::info!(target: TARGET, "QUIC accept loop on {bind_addr} cancelled");
                        break;
                    }
                    incoming = endpoint.accept() => {
                        let Some(incoming) = incoming else {
                            log::info!(target: TARGET, "QUIC endpoint on {bind_addr} closed");
                            break;
                        };
                        let Some(incoming) = rate_limit(
                            incoming,
                            &rl_config,
                            &mut conn_rate_limiters,
                            &mut global_rate_limiter,
                            &transport_errors,
                            bind_addr,
                        ) else {
                            continue;
                        };
                        let addr = incoming.remote_address();
                        log::debug!(target: TARGET, "Accept in QUIC server on {bind_addr} from {addr}");

                        // Check if this peer has been rapidly reconnecting BEFORE
                        // recording this attempt, so the fallback triggers on the
                        // connection AFTER the threshold, not the one that reaches it.
                        let fallback_config = Self::pick_fallback_config(
                            addr,
                            &reconnect_tracker,
                            &per_key_configs,
                            &server_cert_resolver,
                        );

                        // Record this attempt AFTER the fallback check, so the
                        // threshold triggers on the next connection, not this one.
                        if Self::KEY_FALLBACK_ENABLED {
                            if let Ok(mut tracker) = reconnect_tracker.lock() {
                                let entry = tracker.entry(addr.ip()).or_insert_with(|| ReconnectTracker {
                                    count: 0,
                                    window_start: Instant::now(),
                                });
                                ReconnectTracker::record(entry, Self::KEY_FALLBACK_WINDOW);
                            }
                        }

                        let token = cancellation_token.clone();
                        let lkn = local_key_names.clone();
                        let scr = server_cert_resolver.clone();
                        let ib = inbound.clone();
                        let subs = subscribers.clone();
                        let stats = msg_stats.clone();
                        let tracker = reconnect_tracker.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {
                                    log::debug!(target: TARGET, "QUIC connection handler for {addr} cancelled");
                                }
                                _ = Self::handle_connection(
                                    incoming, lkn, scr, ib, subs, bind_addr,
                                    stats, fallback_config, tracker,
                                ) => {}
                            }
                        });
                    }
                }
            }
        });
    }

    /// Pick an alternative ServerConfig if a peer has been rapidly reconnecting.
    /// Returns None (use default) or Some(config with an older key).
    fn pick_fallback_config(
        addr: SocketAddr,
        reconnect_tracker: &Mutex<HashMap<IpAddr, ReconnectTracker>>,
        per_key_configs: &Mutex<HashMap<String, Arc<quinn::ServerConfig>>>,
        server_cert_resolver: &QuicServerCertResolver,
    ) -> Option<Arc<quinn::ServerConfig>> {
        if !Self::KEY_FALLBACK_ENABLED {
            return None;
        }

        let should_fallback = reconnect_tracker
            .lock()
            .ok()
            .and_then(|tracker| {
                tracker.get(&addr.ip()).map(|entry| {
                    ReconnectTracker::should_fallback(
                        entry,
                        Self::KEY_FALLBACK_WINDOW,
                        Self::KEY_FALLBACK_THRESHOLD,
                    )
                })
            })
            .unwrap_or(false);

        if !should_fallback {
            return None;
        }

        // Find a key that is NOT the current default (last_added_name).
        let last_name = server_cert_resolver.last_added_name.lock().ok().and_then(|g| g.clone());

        let configs = per_key_configs.lock().ok()?;
        for (name, config) in configs.iter() {
            if Some(name) != last_name.as_ref() {
                log::info!(
                    target: TARGET,
                    "Fallback: presenting older key to {addr} (rapid reconnection detected)"
                );
                return Some(config.clone());
            }
        }
        None
    }

    /// Background task that periodically scans all outbound connection pools and
    /// removes dead connections. This detects peer crashes, network changes, and
    /// idle timeouts before the next send attempt, avoiding the 10-15s hang on
    /// first use of a dead connection.
    fn spawn_connection_checker(
        weak: Weak<QuicNode>,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        spawn_cancelable(cancellation_token, async move {
            loop {
                tokio::time::sleep(Self::CONNECTION_CHECK_INTERVAL).await;
                let Some(transport) = weak.upgrade() else {
                    log::trace!(target: TARGET, "Connection checker: transport dropped, exiting");
                    break;
                };
                let mut checked = 0u32;
                let mut removed = 0u32;
                for entry in transport.local_keys.iter() {
                    let outbound = &entry.val().outbound;
                    for conn_entry in outbound.map().iter() {
                        let addr = *conn_entry.key();
                        let state = conn_entry.val();
                        if let Some(ref conn) = state.conn {
                            checked += 1;
                            if conn.close_reason().is_some() {
                                log::info!(
                                    target: TARGET,
                                    "Connection checker: removing dead outbound to {addr} \
                                     (close_reason={:?})",
                                    conn.close_reason()
                                );
                                Self::remove_dead_connection(outbound, addr, conn);
                                transport
                                    .transport_errors
                                    .dead_conn_removed
                                    .fetch_add(1, Ordering::Relaxed);
                                removed += 1;
                            } else {
                                state.sender_state.touch_alive();
                            }
                        }
                        // Fully remove entry only when connection is cleared, no sender
                        // task is running, and the queue is empty. Re-fetch from map
                        // because remove_dead_connection may have updated the entry.
                        if let Some(fresh) = outbound.map().get(&addr) {
                            let s = fresh.val();
                            if s.conn.is_none()
                                && !s.sender_state.active.load(Ordering::Acquire)
                                && s.send_queue.is_empty()
                            {
                                outbound.map().remove(&addr);
                            }
                        }
                    }
                }
                if removed > 0 {
                    log::info!(
                        target: TARGET,
                        "Connection checker: scanned {checked} connections, removed {removed} dead"
                    );
                } else {
                    log::trace!(
                        target: TARGET,
                        "Connection checker: scanned {checked} connections, all alive"
                    );
                }
            }
        });
    }

    /// Background task that periodically logs statistics for all active QUIC connections.
    /// Shows deltas (bytes/dgrams/lost since last dump) plus instantaneous path metrics.
    fn spawn_stats_dumper(
        weak: Weak<QuicNode>,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        spawn_cancelable(cancellation_token, async move {
            // Key: (stable_id, is_outbound) → previous snapshot of cumulative counters
            let mut prev: HashMap<(usize, bool), ConnSnapshot> = HashMap::new();

            loop {
                tokio::time::sleep(Self::STATS_DUMP_INTERVAL).await;
                let Some(transport) = weak.upgrade() else {
                    log::trace!(target: TARGET, "Stats dumper: transport dropped, exiting");
                    break;
                };

                let mut seen = HashSet::new();
                let mut total = 0u32;
                let mut dump = String::from("QUIC STATS dump:\n");

                // Outbound connections
                for key_entry in transport.local_keys.iter() {
                    let key_id = key_entry.key();
                    for conn_entry in key_entry.val().outbound.map().iter() {
                        let addr = *conn_entry.key();
                        if let Some(ref conn) = conn_entry.val().conn {
                            let s = conn.stats();
                            let id = (conn.stable_id(), true);
                            seen.insert(id);
                            total += 1;
                            let since = prev
                                .get(&id)
                                .map(|p| p.connected_since)
                                .unwrap_or_else(Instant::now);
                            let snap = ConnSnapshot::new(&s, since);
                            let delta = prev.get(&id).map(|p| snap.delta(p)).unwrap_or(snap);
                            prev.insert(id, snap);
                            Write::write_fmt(
                                &mut dump,
                                format_args!(
                                    "  outbound peer={addr} \
                                    up={} \
                                    dtx={} bytes/{} dgrams drx={} bytes/{} dgrams \
                                    dlost={} pkts rtt={:?} cwnd={} mtu={} \
                                    local={} remote={}\n",
                                    snap.uptime_str(),
                                    delta.tx_bytes,
                                    delta.tx_dgrams,
                                    delta.rx_bytes,
                                    delta.rx_dgrams,
                                    delta.lost_pkts,
                                    s.path.rtt,
                                    s.path.cwnd,
                                    s.path.current_mtu,
                                    key_id,
                                    peer_key_id_from_connection(&conn)
                                        .map(|k| k.to_string())
                                        .unwrap_or_else(|| "?".to_string()),
                                ),
                            )
                            .ok();
                        }
                    }
                }

                // Inbound connections
                let pools = match transport.inbound_pools.lock() {
                    Ok(g) => g.clone(),
                    Err(e) => {
                        log::warn!(
                            target: TARGET,
                            "inbound_pools lock poisoned, skipping inbound stats: {e}"
                        );
                        Vec::new()
                    }
                };
                for pool in &pools {
                    for conn_entry in pool.iter() {
                        let QuicInboundKey(ref local_id, ref peer_id, _) = *conn_entry.key();
                        let addr = conn_entry.val().remote_address();
                        let conn = conn_entry.val();
                        let s = conn.stats();
                        let id = (conn.stable_id(), false);
                        seen.insert(id);
                        total += 1;
                        let since =
                            prev.get(&id).map(|p| p.connected_since).unwrap_or_else(Instant::now);
                        let snap = ConnSnapshot::new(&s, since);
                        let delta = prev.get(&id).map(|p| snap.delta(p)).unwrap_or(snap);
                        prev.insert(id, snap);
                        Write::write_fmt(
                            &mut dump,
                            format_args!(
                                "  inbound peer={addr} \
                                up={} \
                                dtx={} bytes/{} dgrams drx={} bytes/{} dgrams \
                                dlost={} pkts rtt={:?} cwnd={} mtu={} \
                                local={} remote={}\n",
                                snap.uptime_str(),
                                delta.tx_bytes,
                                delta.tx_dgrams,
                                delta.rx_bytes,
                                delta.rx_dgrams,
                                delta.lost_pkts,
                                s.path.rtt,
                                s.path.cwnd,
                                s.path.current_mtu,
                                local_id,
                                peer_id,
                            ),
                        )
                        .ok();
                    }
                }

                // Evict snapshots for connections that no longer exist
                prev.retain(|id, _| seen.contains(id));

                // Per-peer, per-message-kind stats (deltas since last dump)
                let msg_entries = transport.msg_stats.drain();
                let mut current_peer = None;
                for (key, count, bytes) in &msg_entries {
                    if current_peer != Some(key.addr) {
                        current_peer = Some(key.addr);
                        Write::write_fmt(&mut dump, format_args!("  peer {}:\n", key.addr,)).ok();
                    }
                    let dir = if key.is_outbound { "out" } else { " in" };
                    let kind = key.kind.label();
                    Write::write_fmt(
                        &mut dump,
                        format_args!(
                            "    {dir}/{kind} {:#010x}({}) count={count} bytes={bytes}\n",
                            key.tag,
                            tl_tag_name(key.tag),
                        ),
                    )
                    .ok();
                }

                let (sf, qt, cf, qf, dr, rl_ip, rl_gl, retry) = transport.transport_errors.take();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  errors: send_failed={sf} query_timeout={qt} \
                    connect_failed={cf} queue_full={qf} dead_conn_removed={dr}\n",
                    ),
                )
                .ok();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  rate_limit: per_ip_rejected={rl_ip} global_rejected={rl_gl} \
                    retry_sent={retry}\n",
                    ),
                )
                .ok();
                Write::write_fmt(&mut dump, format_args!(
                    "  total: {total} connections, {} msg entries",
                    msg_entries.len(),
                )).ok();

                log::info!(target: TARGET, "{dump}");
            }
        });
    }
}

/// Apply rate-limiting checks to an incoming QUIC connection.
///
/// Returns `Some(incoming)` if the connection is allowed to proceed,
/// or `None` if it was rejected (retry sent, refused, or ignored).
fn rate_limit(
    incoming: quinn::Incoming,
    config: &QuicRateLimitConfig,
    conn_rate_limiters: &mut ConnectionRateLimiters,
    global_rate_limiter: &mut Option<RateLimiter>,
    transport_errors: &TransportErrors,
    bind_addr: SocketAddr,
) -> Option<quinn::Incoming> {
    let addr = incoming.remote_address();

    // Layer 1: Stateless Retry — force address validation
    if config.stateless_retry && !incoming.remote_address_validated() && incoming.may_retry() {
        log::trace!(target: TARGET, "Sending QUIC Retry to unvalidated {addr} on {bind_addr}");
        transport_errors.retry_sent.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = incoming.retry() {
            log::warn!(target: TARGET, "QUIC retry failed for {addr}: {e}");
        }
        return None;
    }

    // Layer 2: Per-IP rate limit
    if !conn_rate_limiters.take_new_connection(addr.ip()) {
        log::debug!(target: TARGET, "Per-IP rate limit for {} on {bind_addr}", addr.ip());
        transport_errors.rate_limited_per_ip.fetch_add(1, Ordering::Relaxed);
        incoming.refuse();
        return None;
    }

    // Periodic cleanup of stale per-IP entries
    conn_rate_limiters.cleanup();

    // Layer 3: Global rate limit
    if let Some(ref mut gl) = global_rate_limiter {
        if !gl.take() {
            log::debug!(target: TARGET, "Global rate limit on {bind_addr}, refusing {addr}");
            transport_errors.rate_limited_global.fetch_add(1, Ordering::Relaxed);
            incoming.refuse();
            return None;
        }
    }

    Some(incoming)
}
