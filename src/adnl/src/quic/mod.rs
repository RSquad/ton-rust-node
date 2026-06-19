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
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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
    ed25519_encode_private_key_to_pkcs8, error, fail, sha256_digest_slices, KeyId, Result,
    ED25519_KEY_TYPE, ED25519_SECRET_KEY_LENGTH,
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
type QuicIpConnCount = lockfree::map::Map<IpAddr, AtomicUsize>;
type QuicDelayedAccepts = lockfree::map::Map<IpAddr, ()>;
type QuicSendQueue = SendQueue<Vec<u8>>;

/// Reason why a delayed-accept reservation was refused.
enum DelayedAcceptRefusal {
    /// This IP already has a delayed accept in progress.
    IpAlreadyDelayed,
    /// The global delayed-accept limit (MAX_DELAYED_ACCEPTS) was reached.
    GlobalLimitReached,
}

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
    let data = sha256_digest_slices(&[&ED25519_KEY_TYPE.to_le_bytes(), pub_key]);
    Ok(KeyId::from_data(data))
}

/// SNI name used to route an inbound QUIC handshake to a specific ADNL identity
/// when several identities share one UDP port. Matches the C++ node's
/// `ServerIdentity::sni` the 32-byte ADNL short id is rendered as lowercase hex
/// and split at the midpoint into two 32-char labels, joined by dots, with a
/// trailing ".adnl" — "<hex[..32]>.<hex[32..]>.adnl".
/// Splitting the hex keeps each label within the RFC 1035 63-octet DNS label
/// limit so rustls accepts the name natively, with no patched dependency
fn compute_sni_name(key_id: &KeyId) -> String {
    let hex = hex::encode(key_id.data());
    format!("{}.{}.adnl", &hex[..32], &hex[32..])
}

/// Inverse of `compute_sni_name`. Returns `None` if the SNI does not match
/// the "<32-hex>.<32-hex>.adnl" shape
fn key_id_from_sni(server_name: &str) -> Option<Arc<KeyId>> {
    const SUFFIX: &str = ".adnl";
    let prefix_len = server_name.len().checked_sub(SUFFIX.len())?;
    let (prefix, suffix) = server_name.split_at(prefix_len);
    if !suffix.eq_ignore_ascii_case(SUFFIX) {
        return None;
    }
    let (h1, h2) = prefix.split_once('.')?;
    if h1.len() != 32 || h2.len() != 32 {
        return None;
    }
    let mut data = [0u8; 32];
    hex::decode_to_slice(h1, &mut data[..16]).ok()?;
    hex::decode_to_slice(h2, &mut data[16..]).ok()?;
    Some(KeyId::from_data(data))
}

/// Look up a registered local identity by its SNI. Returns `None` if SNI is
/// absent or does not parse to a known identity; the caller decides whether
/// that means fall back to the active identity or reject the handshake
fn match_identity_by_sni(
    server_name: Option<&str>,
    registered_keys: &Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>,
) -> Option<(Arc<KeyId>, Arc<rustls::sign::CertifiedKey>)> {
    let key_id = key_id_from_sni(server_name?)?;
    let keys = registered_keys.lock().ok()?;
    keys.get_key_value(&*key_id).map(|(k, v)| (k.clone(), v.clone()))
}

/// Read the SNI the client sent during the QUIC/TLS handshake (server side)
fn negotiated_sni(conn: &quinn::Connection) -> Option<String> {
    let data = conn.handshake_data()?;
    let data = data.downcast::<quinn::crypto::rustls::HandshakeData>().ok()?;
    data.server_name
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

/// Presents an RPK (Ed25519 SPKI, RFC 7250) to connecting peers.
/// If the client sends an SNI matching a registered identity, that identity's
/// cert is presented; otherwise the active identity is used as the default.
/// This mirrors the C++ node's SNI-based identity dispatch and lets several
/// validator identities share one UDP port.
struct QuicServerCertResolver {
    active_identity: Arc<Mutex<Option<ActiveIdentity>>>,
    registered_keys: Arc<Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>>,
}

impl QuicServerCertResolver {
    fn new(
        active_identity: Arc<Mutex<Option<ActiveIdentity>>>,
        registered_keys: Arc<Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>>,
    ) -> Arc<Self> {
        Arc::new(Self { active_identity, registered_keys })
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
        // 1. SNI names one of our registered identities -> present that cert.
        if let Some((_, cert)) =
            match_identity_by_sni(client_hello.server_name(), &self.registered_keys)
        {
            return Some(cert);
        }
        // 2. SNI present but did not match. "ton" is the legacy dummy older Rust
        //    clients sent unconditionally; treat it as no SNI (silent fallback
        //    to the active identity). Anything else is rejected: returning None
        //    here makes rustls fail the handshake
        if let Some(name) = client_hello.server_name() {
            if !name.eq_ignore_ascii_case("ton") {
                // handle_connection logs the resulting handshake failure with
                // peer address; this only adds the SNI for diagnostics
                log::debug!(
                    target: TARGET,
                    "QUIC inbound: rejecting unknown SNI {name:?}"
                );
                return None;
            }
        }
        // 3. No SNI or legacy "ton" -> present the active identity
        self.active_identity
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|identity| identity.cert.clone()))
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

/// Per-port endpoint state: the quinn endpoint, its accept loop handle,
/// and the TLS cert/key maps for identities registered on this port.
#[derive(Clone)]
struct ActiveIdentity {
    key_id: Arc<KeyId>,
    cert: Arc<rustls::sign::CertifiedKey>,
}

struct EndpointState {
    endpoint: quinn::Endpoint,
    /// The currently active identity for this endpoint.
    active_identity: Arc<Mutex<Option<ActiveIdentity>>>,
    /// All registered keys on this endpoint (for key rotation / removal).
    registered_keys: Arc<Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>>,
}

/// Command sent to the background Tokio task that manages QUIC key operations.
/// Quinn endpoint creation requires a Tokio runtime context, but callers may run
/// on bare OS threads (e.g. Simplex SXMAIN). The channel decouples the two.
enum KeyCommand {
    AddKey {
        key: [u8; ED25519_SECRET_KEY_LENGTH],
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

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

    /// Experiment A (fix_quic_problem_0417.md): gate for the per-IP delayed-
    /// accept path. When `false`, every handshake proceeds straight to
    /// `handle_connection`, bounded only by `rate_limit()` (stateless retry +
    /// conn_rate_limiters + global_rate_limiter) and `CONNECT_TIMEOUT`.
    ///
    /// Rationale: `incoming.refuse()` from the delayed-accept path triggers the
    /// peer's ngtcp2 on_closed -> outbound_.erase(path) -> ADNL retry -> new
    /// handshake, a self-reinforcing churn loop. The 2 s delay additionally
    /// burns the peer's 5 s handshake budget on high-RTT/lossy paths. Set back
    /// to `true` to re-enable the throttle.
    const DELAYED_ACCEPT_ENABLED: bool = false;

    /// Per-IP live inbound connection limit.
    /// When an IP already has PER_IP_INBOUND_FAST_THRESHOLD inbound connections,
    /// new incoming handshakes may enter a bounded delayed-accept path.
    const PER_IP_INBOUND_FAST_THRESHOLD: usize = 5;
    /// Maximum number of delayed accepts permitted globally.
    const MAX_DELAYED_ACCEPTS: usize = 64;
    /// How long to hold a bounded delayed accept before starting the handshake.
    const PER_IP_INBOUND_DELAY: Duration = Duration::from_secs(2);

    const ZOMBIE_STREAM_GRACE: Duration = Duration::from_secs(60);

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
        // Pre-register Prometheus counters so they appear in the exporter
        // output before the first event
        transport.transport_errors.register_metrics();
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
        key: &[u8; ED25519_SECRET_KEY_LENGTH],
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
        key: &[u8; ED25519_SECRET_KEY_LENGTH],
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
        // BBR instead of the default CUBIC
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
        let server_spki = rustls::pki_types::CertificateDer::from(pub_key_bytes);
        let server_signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| error!("Cannot create server signing key: {e}"))?;
        let certified_key = Arc::new(rustls::sign::CertifiedKey::new(
            vec![server_spki.clone()],
            server_signing_key.clone(),
        ));

        // Register in the endpoint's key store
        if let Ok(mut keys) = endpoint_state.registered_keys.lock() {
            keys.insert(key_id.clone(), certified_key.clone());
        }

        // Auto-activate the first key so the server always has an active identity
        if let Ok(active) = endpoint_state.active_identity.lock() {
            if active.is_none() {
                drop(active);
                if self.set_active_key(&endpoint_state, key_id, &certified_key) {
                    log::info!(
                        target: TARGET,
                        "Registered and auto-activated QUIC identity {} on port {}",
                        key_id, bind_addr.port()
                    );
                    return Ok(());
                }
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

        // Remove from registered keys
        if let Ok(mut keys) = endpoint_state.registered_keys.lock() {
            keys.remove(key_id);
        }

        // If the removed key was the active one, switch to another or None.
        // Determine the replacement under registered_keys first, then lock
        // active_identity — this keeps the same order as add_key (registered_keys
        // before active_identity) and avoids a potential deadlock.
        let replacement = endpoint_state.registered_keys.lock().ok().and_then(|keys| {
            keys.iter()
                .next()
                .map(|(id, cert)| ActiveIdentity { key_id: id.clone(), cert: cert.clone() })
        });
        if let Ok(mut active) = endpoint_state.active_identity.lock() {
            if active.as_ref().map(|identity| identity.key_id.as_ref()) == Some(key_id.as_ref()) {
                *active = replacement;
            }
        }

        // Close all inbound connections that were established with this key.
        // The handle_connection tasks will detect the closure and clean up.
        if let Ok(pools) = self.inbound_pools.lock() {
            let mut closed = 0u32;
            for pool in pools.iter() {
                for entry in pool.iter() {
                    let QuicInboundKey(ref local_id, _, _) = *entry.key();
                    if local_id == key_id {
                        entry.val().close(0u32.into(), b"Key removed");
                        closed += 1;
                    }
                }
            }
            if closed > 0 {
                log::info!(
                    target: TARGET,
                    "Closed {closed} inbound connection(s) bound to removed key {key_id}"
                );
            }
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

    /// Activate a previously added key as the current identity for inbound connections.
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
        let cert =
            endpoint_state.registered_keys.lock().ok().and_then(|keys| keys.get(key_id).cloned());
        let Some(cert) = cert else {
            log::warn!(target: TARGET, "activate_key: no cert for key {key_id}");
            return;
        };
        if self.set_active_key(endpoint_state, key_id, &cert) {
            log::info!(target: TARGET, "Activated QUIC identity {} on port {}", key_id, port);
        }
    }

    fn set_active_key(
        &self,
        endpoint_state: &EndpointState,
        key_id: &Arc<KeyId>,
        cert: &Arc<rustls::sign::CertifiedKey>,
    ) -> bool {
        if let Ok(mut active) = endpoint_state.active_identity.lock() {
            if active.as_ref().map(|identity| identity.key_id.as_ref()) == Some(key_id.as_ref()) {
                return false;
            }
            *active = Some(ActiveIdentity { key_id: key_id.clone(), cert: cert.clone() });
            return true;
        }
        false
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
                    self.transport_errors.send_failed.inc();
                    log::warn!(
                        target: TARGET,
                        "QUIC direct send to {} failed: {e}, removing dead connection, \
                        falling back to queue",
                        peers.other()
                    );
                    Self::remove_dead_connection(&state.outbound, addr, conn);
                    self.transport_errors.dead_conn_removed.inc();
                }
            }
        }

        // Slow path: no connection (or it just died) — enqueue for the sender task
        // which will establish the connection and deliver
        if !outbound.send_queue.try_push(data) {
            self.transport_errors.queue_full.inc();
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
            spawn_cancelable(
                self.cancellation_token.clone(),
                Self::run_sender_task(
                    quic,
                    peers.clone(),
                    addr,
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

    async fn connect(&self, peers: &AdnlPeers, addr: SocketAddr) -> Result<()> {
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
        // Send the peer's SNI so a node hosting several identities on this UDP
        // port routes the handshake to the identity we actually want to reach
        // (matches the C++ node). Peers hosting a single identity ignore it.
        let server_name = compute_sni_name(dst);
        let conn = endpoint
            .connect_with(state.client_config.clone(), addr, &server_name)
            .map_err(|e| error!("QUIC connect to {addr}: {e}"))?
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
        let state = self.local_key_state(peers.local())?;
        loop {
            let conn = Self::get_or_create_outbound_connection(&state.outbound, addr)?;
            if conn.conn.is_some() {
                break Ok(conn);
            }
            log::info!(target: TARGET, "Try new QUIC connection to {addr} in foreground");
            if let Err(e) = self.connect(peers, addr).await {
                self.transport_errors.connect_failed.inc();
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
        let active_identity: Arc<Mutex<Option<ActiveIdentity>>> = Arc::new(Mutex::new(None));
        let registered_keys = Arc::new(Mutex::new(HashMap::new()));
        let verifier = QuicClientCertVerifier::new();
        let server_cert_resolver =
            QuicServerCertResolver::new(active_identity.clone(), registered_keys.clone());
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
        // BBR instead of the default CUBIC
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

        let inbound: Arc<QuicInboundMap> = Arc::new(lockfree::map::Map::new());
        let ip_conn_count: Arc<QuicIpConnCount> = Arc::new(lockfree::map::Map::new());
        let delayed_accepts: Arc<QuicDelayedAccepts> = Arc::new(lockfree::map::Map::new());
        let delayed_accept_count = Arc::new(AtomicUsize::new(0));
        match self.inbound_pools.lock() {
            Ok(mut pools) => pools.push(inbound.clone()),
            Err(e) => log::warn!(
                target: TARGET,
                "inbound_pools lock poisoned, inbound stats will be incomplete: {e}"
            ),
        }

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
            active_identity.clone(),
            registered_keys.clone(),
            self.subscribers.clone(),
            bind_addr,
            self.cancellation_token.clone(),
            inbound,
            ip_conn_count,
            delayed_accepts,
            delayed_accept_count,
            self.msg_stats.clone(),
            rl_config,
            conn_rate_limiters,
            global_rate_limiter,
            self.transport_errors.clone(),
        );

        let state = Arc::new(EndpointState { endpoint, active_identity, registered_keys });
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

    /// Returns `true` if this IP has fewer than PER_IP_INBOUND_FAST_THRESHOLD
    /// live inbound connections and is allowed to start another handshake.
    fn ip_allow_fast(ip_conn_count: &QuicIpConnCount, ip: IpAddr) -> bool {
        match ip_conn_count.get(&ip) {
            Some(entry) => {
                entry.val().load(Ordering::Relaxed) < Self::PER_IP_INBOUND_FAST_THRESHOLD
            }
            None => true,
        }
    }

    fn ip_conn_inc(ip_conn_count: &QuicIpConnCount, ip: IpAddr) {
        if let Some(entry) = ip_conn_count.get(&ip) {
            entry.val().fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _ = add_unbound_object_to_map_with_update(ip_conn_count, ip, |found| match found {
            Some(count) => {
                count.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }
            None => Ok(Some(AtomicUsize::new(1))),
        });
    }

    fn ip_conn_dec(ip_conn_count: &QuicIpConnCount, ip: IpAddr) {
        if let Some(entry) = ip_conn_count.get(&ip) {
            let prev = entry.val().fetch_sub(1, Ordering::Relaxed);
            if prev <= 1 {
                // Remove the zero-valued entry so the map only holds IPs with
                // active connections.  A narrow race exists: a concurrent
                // ip_conn_inc for the same IP may bump the counter between
                // fetch_sub and remove.  This is benign — ip_conn_inc's
                // fallback recreates the entry, so at worst one connection
                // skips the delayed-accept path.
                ip_conn_count.remove(&ip);
            }
        }
    }

    /// Try to reserve a delayed-accept slot for `ip`.
    /// Returns `Ok(())` on success, `Err(reason)` on refusal.
    fn try_acquire_delayed_accept(
        delayed_accepts: &QuicDelayedAccepts,
        delayed_accept_count: &AtomicUsize,
        ip: IpAddr,
    ) -> std::result::Result<(), DelayedAcceptRefusal> {
        let inserted = match add_unbound_object_to_map(delayed_accepts, ip, || Ok(())) {
            Ok(inserted) => inserted,
            Err(e) => {
                log::warn!(target: TARGET, "Cannot reserve delayed accept for {ip}: {e}");
                return Err(DelayedAcceptRefusal::IpAlreadyDelayed);
            }
        };
        if !inserted {
            return Err(DelayedAcceptRefusal::IpAlreadyDelayed);
        }

        let reserved = delayed_accept_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < Self::MAX_DELAYED_ACCEPTS).then_some(count + 1)
            })
            .is_ok();
        if !reserved {
            delayed_accepts.remove(&ip);
            return Err(DelayedAcceptRefusal::GlobalLimitReached);
        }
        Ok(())
    }

    fn release_delayed_accept(
        delayed_accepts: &QuicDelayedAccepts,
        delayed_accept_count: &AtomicUsize,
        ip: IpAddr,
    ) {
        delayed_accepts.remove(&ip);
        delayed_accept_count.fetch_sub(1, Ordering::AcqRel);
    }

    async fn handle_connection(
        incoming: quinn::Incoming,
        active_identity: Arc<Mutex<Option<ActiveIdentity>>>,
        registered_keys: Arc<Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>>,
        inbound: Arc<QuicInboundMap>,
        ip_conn_count: Arc<QuicIpConnCount>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        bind_addr: SocketAddr,
        msg_stats: Arc<MsgStats>,
    ) {
        let addr = incoming.remote_address();

        let connecting = match incoming.accept() {
            Ok(c) => c,
            Err(e) => {
                log::warn!(target: TARGET, "QUIC accept from {addr}: {e}");
                return;
            }
        };
        // Bound handshake time: C++ ngtcp2 clients abandon after ~3-5s and retry,
        // so a handshake still in progress after 5s is almost certainly stale.
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

        // Determine which local identity served this handshake. If the client
        // sent an SNI matching a registered identity, the cert resolver presented
        // that identity's cert, so bind the connection to it. Otherwise fall back
        // to the active identity (the C++ "default" identity behavior).
        let sni = negotiated_sni(&conn);
        let local_key_id = match match_identity_by_sni(sni.as_deref(), &registered_keys) {
            Some((key_id, _)) => key_id,
            None => match active_identity
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|identity| identity.key_id.clone()))
            {
                Some(key_id) => key_id,
                None => {
                    log::warn!(
                        target: TARGET,
                        "No active key on {bind_addr}, closing {addr}"
                    );
                    conn.close(0u32.into(), b"No active key");
                    return;
                }
            },
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
        Self::ip_conn_inc(&ip_conn_count, addr.ip());

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
        let zombie_streams = streams_accepted.clone();
        let zombie_conn = conn.clone();
        tokio::select! {
            () = bi_loop => {}
            () = uni_loop => {}
            reason = conn.closed() => {
                log::debug!(
                    target: TARGET,
                    "QUIC connection from {addr} closed early: {reason}"
                );
            }
            () = async move {
                if Self::ZOMBIE_STREAM_GRACE.is_zero() {
                    std::future::pending::<()>().await;
                }
                tokio::time::sleep(Self::ZOMBIE_STREAM_GRACE).await;
                if zombie_streams.load(Ordering::Relaxed) == 0 {
                    log::info!(
                        target: TARGET,
                        "Evicting zombie QUIC inbound from {addr}: no streams after {:?}",
                        Self::ZOMBIE_STREAM_GRACE
                    );
                    zombie_conn.close(0u32.into(), b"zombie - no streams");
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
        let total_streams = streams_accepted.load(Ordering::Relaxed);
        inbound.remove(&inbound_key);
        Self::ip_conn_dec(&ip_conn_count, addr.ip());

        log::info!(
            target: TARGET,
            "Exit QUIC inbound receiver for {addr} \
            (conn_id={conn_id}, streams={total_streams})"
        );
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
    /// On connect failure the whole queue is flushed (the peer is unreachable
    /// and retrying each message individually would stall for the full
    /// handshake timeout every time). The flush is a best-effort drain of a
    /// lock-free queue: messages pushed concurrently with the flush may be
    /// observed and dropped. Anything pushed after the flush completes is
    /// picked up by the next connect attempt.
    ///
    /// When previous connect attempts have failed (counter persists in
    /// `SenderState`), the task applies a stepped backoff before the first
    /// attempt of each drain cycle. Messages keep queuing during the backoff
    /// and are either delivered on success or flushed on failure.
    ///
    /// Every exit goes through the deactivation epilogue below: `active` is
    /// set back to `false` before the task terminates, so a later `message()`
    /// can spawn a fresh sender task and retry the connect with backoff. Any
    /// messages that survive a flush race show up in the epilogue re-check
    /// and are handled in the next drain cycle instead of being stranded
    async fn run_sender_task(
        quic: Arc<Self>,
        peers: AdnlPeers,
        addr: SocketAddr,
        send_queue: Arc<QuicSendQueue>,
        sender_state: Arc<SenderState>,
        outbound: Arc<Connections<QuicOutboundConnection>>,
    ) {
        log::trace!(target: TARGET, "QUIC sender task started for {addr}");

        loop {
            // Stepped backoff based on previous failed attempts (1 attempt per cycle).
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

            // Drain the queue. A fatal connect failure breaks out of the drain
            // only, never out of the task: the deactivation epilogue below must
            // run on every exit path so that `active` never stays `true` after
            // the task terminates
            'drain: while let Some(data) = send_queue.pop() {
                match quic.send_message(&peers, addr, &outbound, &sender_state, &data).await {
                    Ok(()) => {}
                    Err(SendError::Temporary(e)) => {
                        log::warn!(target: TARGET, "QUIC sender to {addr} send error: {e}");
                    }
                    Err(SendError::Fatal(e)) => {
                        // Connect failed: drop the in-flight message and flush
                        // the whole queue, then exit the drain. New messages
                        // enqueued from now on are collected for the next
                        // connect attempt (with stepped backoff)
                        let mut queue_len = 1usize;
                        while send_queue.pop().is_some() {
                            queue_len += 1;
                        }
                        let peer = peers.other();
                        log::warn!(
                            target: TARGET,
                            "QUIC sender to {addr}: connect to peer {peer} failed: {e}, \
                            {queue_len} messages dropped"
                        );
                        break 'drain;
                    }
                }
            }

            // Mark inactive, then re-check: a new message may have been enqueued
            // between the last pop() (or the flush above) and the store below.
            sender_state.active.store(false, Ordering::Release);
            if send_queue.is_empty() {
                break;
            }
            // Lost race - reactivate if no other task took over
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
        outbound: &Connections<QuicOutboundConnection>,
        sender_state: &SenderState,
        data: &[u8],
    ) -> std::result::Result<(), SendError> {
        let entry = Self::get_or_create_outbound_connection(outbound, addr)
            .map_err(SendError::Temporary)?;
        match entry.conn {
            Some(ref conn) => {
                if let Err(e) = Self::send_via_stream_nowait(conn, data).await {
                    self.transport_errors.send_failed.inc();
                    log::warn!(
                        target: TARGET,
                        "QUIC send to {addr} failed: {e}, removing dead connection"
                    );
                    Self::remove_dead_connection(outbound, addr, conn);
                    self.transport_errors.dead_conn_removed.inc();
                    return Err(SendError::Temporary(e));
                }
            }
            None => {
                let attempt = sender_state.next_attempt();
                log::info!(
                    target: TARGET,
                    "QUIC sender: connecting to {addr} (attempt {attempt})"
                );
                match tokio::time::timeout(Self::CONNECT_TIMEOUT, self.connect(peers, addr)).await {
                    Ok(Ok(())) => {
                        sender_state.record_connect_success();
                    }
                    Ok(Err(e)) => {
                        self.transport_errors.connect_failed.inc();
                        log::warn!(
                            target: TARGET,
                            "QUIC connect to {addr} failed: {e} \
                            (attempt {attempt}, last alive: {})",
                            sender_state.last_alive_ago()
                        );
                        return Err(SendError::Fatal(e));
                    }
                    Err(_) => {
                        self.transport_errors.connect_failed.inc();
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
                        self.transport_errors.send_failed.inc();
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} failed: {e}, removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                        self.transport_errors.dead_conn_removed.inc();
                    }
                    Err(_) => {
                        self.transport_errors.query_timeout.inc();
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} timed out ({timeout_ms}ms), \
                            removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                        self.transport_errors.dead_conn_removed.inc();
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
        active_identity: Arc<Mutex<Option<ActiveIdentity>>>,
        registered_keys: Arc<Mutex<HashMap<Arc<KeyId>, Arc<rustls::sign::CertifiedKey>>>>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        bind_addr: SocketAddr,
        cancellation_token: tokio_util::sync::CancellationToken,
        inbound: Arc<QuicInboundMap>,
        ip_conn_count: Arc<QuicIpConnCount>,
        delayed_accepts: Arc<QuicDelayedAccepts>,
        delayed_accept_count: Arc<AtomicUsize>,
        msg_stats: Arc<MsgStats>,
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
                        let delayed = if Self::DELAYED_ACCEPT_ENABLED
                            && !Self::ip_allow_fast(&ip_conn_count, addr.ip())
                        {
                            match Self::try_acquire_delayed_accept(
                                &delayed_accepts,
                                &delayed_accept_count,
                                addr.ip(),
                            ) {
                                Ok(()) => {
                                    log::debug!(
                                        target: TARGET,
                                        "Delaying QUIC accept from {addr} for {:?} \
                                        (live inbound >= {}, delayed global={}/{})",
                                        Self::PER_IP_INBOUND_DELAY,
                                        Self::PER_IP_INBOUND_FAST_THRESHOLD,
                                        delayed_accept_count.load(Ordering::Relaxed),
                                        Self::MAX_DELAYED_ACCEPTS,
                                    );
                                    transport_errors.delayed.inc();
                                    true
                                }
                                Err(reason) => {
                                    let reason_str = match reason {
                                        DelayedAcceptRefusal::IpAlreadyDelayed =>
                                            "IP already has a delayed accept in progress",
                                        DelayedAcceptRefusal::GlobalLimitReached =>
                                            "global delayed accept limit reached",
                                    };
                                    log::debug!(
                                        target: TARGET,
                                        "Refusing QUIC accept from {addr}: {reason_str}"
                                    );
                                    transport_errors.delayed_refused.inc();
                                    incoming.refuse();
                                    continue;
                                }
                            }
                        } else {
                            false
                        };

                        transport_errors.accepted.inc();
                        let token = cancellation_token.clone();
                        let ai = active_identity.clone();
                        let rk = registered_keys.clone();
                        let ib = inbound.clone();
                        let ipc = ip_conn_count.clone();
                        let subs = subscribers.clone();
                        let stats = msg_stats.clone();
                        if delayed {
                            let da = delayed_accepts.clone();
                            let dac = delayed_accept_count.clone();
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = token.cancelled() => {
                                        log::debug!(target: TARGET, "QUIC delayed accept for {addr} cancelled");
                                        Self::release_delayed_accept(&da, &dac, addr.ip());
                                        return;
                                    }
                                    _ = tokio::time::sleep(Self::PER_IP_INBOUND_DELAY) => {
                                        Self::release_delayed_accept(&da, &dac, addr.ip());
                                    }
                                }
                                tokio::select! {
                                    _ = token.cancelled() => {
                                        log::debug!(target: TARGET, "QUIC connection handler for {addr} cancelled");
                                    }
                                    _ = Self::handle_connection(
                                        incoming, ai, rk, ib, ipc, subs, bind_addr, stats,
                                    ) => {}
                                }
                            });
                        } else {
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = token.cancelled() => {
                                        log::debug!(target: TARGET, "QUIC connection handler for {addr} cancelled");
                                    }
                                    _ = Self::handle_connection(
                                        incoming, ai, rk, ib, ipc, subs, bind_addr, stats,
                                    ) => {}
                                }
                            });
                        }
                    }
                }
            }
        });
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
                                transport.transport_errors.dead_conn_removed.inc();
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
                let mut outbound_total = 0u32;
                let mut inbound_total = 0u32;
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
                            outbound_total += 1;
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
                        inbound_total += 1;
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

                let e = transport.transport_errors.take();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  errors: send_failed={} query_timeout={} \
                    connect_failed={} queue_full={} dead_conn_removed={}\n",
                        e.send_failed,
                        e.query_timeout,
                        e.connect_failed,
                        e.queue_full,
                        e.dead_conn_removed,
                    ),
                )
                .ok();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  rate_limit: per_ip_rejected={} global_rejected={} \
                    retry_sent={}\n",
                        e.rate_limited_per_ip, e.rate_limited_global, e.retry_sent,
                    ),
                )
                .ok();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  accept: accepted={} delayed={} \
                    delayed_refused={}\n",
                        e.accepted, e.delayed, e.delayed_refused,
                    ),
                )
                .ok();
                Write::write_fmt(
                    &mut dump,
                    format_args!(
                        "  total: {} connections, in: {inbound_total}, out: {outbound_total}, {} msg entries",
                        outbound_total + inbound_total,
                        msg_entries.len(),
                    ),
                )
                .ok();

                // Export connection counts as Prometheus gauges. The error
                // counters are exported instantly at their event sites via
                // EventCounter::inc(), not from this dump cycle
                metrics::gauge!("ton_node_quic_outbound_connections").set(outbound_total as f64);
                metrics::gauge!("ton_node_quic_inbound_connections").set(inbound_total as f64);

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
        transport_errors.retry_sent.inc();
        if let Err(e) = incoming.retry() {
            log::warn!(target: TARGET, "QUIC retry failed for {addr}: {e}");
        }
        return None;
    }

    // Layer 2: Per-IP rate limit
    if !conn_rate_limiters.take_new_connection(addr.ip()) {
        log::debug!(target: TARGET, "Per-IP rate limit for {} on {bind_addr}", addr.ip());
        transport_errors.rate_limited_per_ip.inc();
        incoming.refuse();
        return None;
    }

    // Periodic cleanup of stale per-IP entries
    conn_rate_limiters.cleanup();

    // Layer 3: Global rate limit
    if let Some(ref mut gl) = global_rate_limiter {
        if !gl.take() {
            log::debug!(target: TARGET, "Global rate limit on {bind_addr}, refusing {addr}");
            transport_errors.rate_limited_global.inc();
            incoming.refuse();
            return None;
        }
    }

    Some(incoming)
}
