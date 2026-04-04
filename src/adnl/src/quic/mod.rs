/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    common::{
        add_unbound_object_to_map, add_unbound_object_to_map_with_update, spawn_cancelable,
        AdnlPeers, Answer, Query, QueryAnswer, Subscriber,
    },
    node::AdnlNode,
    transport::{Connections, SendQueue},
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Once, Weak,
    },
    time::Duration,
};
use ton_api::{
    deserialize_boxed, deserialize_boxed_with_suffix, serialize_boxed,
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

/// Key for the QUIC inbound connection map: (local_key_id, peer_key_id).
/// Matches the C++ `AdnlPath{local_id, peer_id}` semantics so that two
/// connections from the same peer address but different key pairs (e.g.
/// current + next validator keys) coexist instead of evicting each other.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct QuicInboundKey(Arc<KeyId>, Arc<KeyId>);

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
/// one sender task runs per outbound peer.
struct SenderState {
    active: AtomicBool,
}

impl SenderState {
    fn new() -> Arc<Self> {
        Arc::new(Self { active: AtomicBool::new(false) })
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

impl fmt::Debug for QuicServerCertResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

impl fmt::Debug for QuicClientCertVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
struct EndpointState {
    endpoint: quinn::Endpoint,
    server_cert_keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>>,
    local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>>,
    /// Tracks the most recently added identity name for SNI fallback.
    last_added_name: Arc<Mutex<Option<String>>>,
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
    /// Max concurrent in-flight streams per inbound connection.
    max_streams_per_connection: usize,
    /// Inbound connection maps, one per endpoint/accept-loop. Used by the stats dumper.
    inbound_pools: Mutex<Vec<Arc<QuicInboundMap>>>,
    /// Per-TL-tag message counters for the stats dumper.
    msg_stats: Arc<MsgStats>,
}

impl QuicNode {
    pub const OFFSET_PORT: u16 = 1000;

    /// How often the background checker scans outbound connections for dead ones.
    const CONNECTION_CHECK_INTERVAL: Duration = Duration::from_secs(5);
    /// How often the stats dumper logs connection statistics.
    const STATS_DUMP_INTERVAL: Duration = Duration::from_secs(60);
    const DEFAULT_MAX_STREAMS_PER_CONNECTION: usize = 256;
    const DEFAULT_QUERY_TIMEOUT_MS: u64 = 5000;
    /// Maximum number of messages buffered per outbound peer
    const SEND_QUEUE_CAPACITY: usize = 1024;

    /// Create a new QuicNode. No endpoints are bound — they are created lazily
    /// by `add_key()` when the first identity for a given port is registered.
    pub fn new(
        subscribers: Vec<Arc<dyn Subscriber>>,
        cancellation_token: tokio_util::sync::CancellationToken,
        max_streams_per_connection: Option<usize>,
    ) -> Arc<Self> {
        let max_streams_per_connection =
            max_streams_per_connection.unwrap_or(Self::DEFAULT_MAX_STREAMS_PER_CONNECTION);
        static CRYPTO_INIT: Once = Once::new();
        CRYPTO_INIT.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install default Rustls CryptoProvider");
        });
        let transport = Arc::new(Self {
            cancellation_token: cancellation_token.clone(),
            local_keys: lockfree::map::Map::new(),
            endpoints: Mutex::new(HashMap::new()),
            subscribers: Arc::new(subscribers),
            peer_keys: lockfree::map::Map::new(),
            max_streams_per_connection,
            inbound_pools: Mutex::new(Vec::new()),
            msg_stats: MsgStats::new(),
        });
        Self::spawn_connection_checker(Arc::downgrade(&transport), cancellation_token.clone());
        Self::spawn_stats_dumper(Arc::downgrade(&transport), cancellation_token);
        transport
    }

    /// Register a local identity on a specific bind address.
    /// Creates a new endpoint if one doesn't exist for this port yet.
    pub fn add_key(
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

        // Update last-added name for SNI fallback (C++ ngtcp2 doesn't send SNI)
        if let Ok(mut last) = endpoint_state.last_added_name.lock() {
            *last = Some(name);
        }

        log::info!(
            target: TARGET,
            "Registered QUIC identity {} on port {}",
            key_id, bind_addr.port()
        );

        Ok(())
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
        self.ensure_peer_registered(adnl, peers)?;
        let tag = extract_inner_tag(&data);
        let size = data.len();
        let data = serialize_boxed(&QuicMessage { data: data.into() }.into_boxed())?;
        let addr = self.addr_by_key(peers.other())?;
        let state = self.local_key_state(peers.local())?;
        let outbound = Self::get_or_create_outbound_connection(&state.outbound, addr)?;

        // Fast path: if connection is alive, send directly without queue overhead
        if let Some(ref conn) = outbound.conn {
            match Self::send_via_stream(conn, &data).await {
                Ok(_) => {
                    self.msg_stats.record(tag, size, addr, true, false);
                    return Ok(Some(data.len()));
                }
                Err(e) => {
                    log::warn!(
                        target: TARGET,
                        "QUIC direct send to {} failed: {e}, removing dead connection, \
                        falling back to queue",
                        peers.other()
                    );
                    Self::remove_dead_connection(&state.outbound, addr, conn);
                }
            }
        }

        // Slow path: no connection (or it just died) — enqueue for the sender task
        // which will establish the connection and deliver
        if !outbound.send_queue.try_push(data) {
            fail!("QUIC send queue full for peer {}", peers.other());
        }
        self.msg_stats.record(tag, size, addr, true, false);

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
        let response = self.send_query_raw(wire, peers, timeout_ms).await?;
        self.msg_stats.record(tag, size, addr, true, true);
        if response.is_empty() {
            return Ok(None);
        }
        let obj = deserialize_boxed(&response)
            .map_err(|e| error!("Cannot deserialise QUIC answer: {e}"))?;
        match obj.downcast::<QuicResponse>() {
            Ok(QuicResponse::Quic_Answer(answer)) => Ok(Some(answer.data.to_vec())),
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

        if !state.outbound.set_connection_state(addr, |found| {
            if found.conn.is_none() {
                Ok(Some(QuicOutboundConnection {
                    conn: Some(conn.clone()),
                    send_queue: found.send_queue.clone(),
                    sender_state: found.sender_state.clone(),
                }))
            } else {
                Ok(None)
            }
        })? {
            conn.close(0u32.into(), b"Duplicate QUIC connection");
        }
        Ok(())
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
            self.connect(peers, addr, &server_name).await?;
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
        let mut addr = adnl
            .peer_ip_address(peers.local(), dst)?
            .ok_or_else(|| error!("QUIC peer {dst} IP is not known in ADNL"))?;
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
            std::net::UdpSocket::from(sock)
        };
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

        Self::spawn_accept_loop(
            endpoint.clone(),
            local_key_names.clone(),
            server_cert_resolver,
            self.subscribers.clone(),
            bind_addr,
            self.max_streams_per_connection,
            self.cancellation_token.clone(),
            inbound,
            self.msg_stats.clone(),
        );

        let state = Arc::new(EndpointState {
            endpoint,
            server_cert_keys,
            local_key_names,
            last_added_name,
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
        max_streams_per_connection: usize,
        msg_stats: Arc<MsgStats>,
    ) {
        let addr = incoming.remote_address();
        // Bound handshake time: C++ ngtcp2 clients abandon after ~3-5s and retry,
        // so a handshake still in progress after 5s is almost certainly stale.
        // Without this, stale Connecting futures accumulate inside quinn's endpoint,
        // slowing its internal event loop and delaying endpoint.accept() for new peers.
        let conn = match tokio::time::timeout(Duration::from_secs(5), incoming).await {
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

        log::info!(target: TARGET, "Accepted QUIC connection from {addr} on {bind_addr}");

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

        let inbound_key = QuicInboundKey(local_key_id.clone(), peer_key_id.clone());
        let had_existing = {
            let mut found_existing = false;
            let result =
                add_unbound_object_to_map_with_update(&inbound, inbound_key.clone(), |existing| {
                    if existing.is_some() {
                        found_existing = true;
                        // Keep existing entry; resolver task will handle replacement
                        Ok(None)
                    } else {
                        Ok(Some(conn.clone()))
                    }
                });
            if let Err(e) = result {
                log::warn!(target: TARGET, "Store QUIC inbound for {addr}: {e}");
                return;
            }
            found_existing
        };
        if had_existing {
            tokio::spawn(Self::resolve_duplicate_connection(
                inbound.clone(),
                conn.clone(),
                inbound_key.clone(),
                addr,
            ));
        }

        let peers = AdnlPeers::with_keys(local_key_id, peer_key_id);
        let conn_id = conn.stable_id();
        // Limit concurrent in-flight streams per connection to bound memory usage.
        // When the semaphore is full, accept stalls, applying QUIC-level backpressure.
        let stream_semaphore = Arc::new(tokio::sync::Semaphore::new(max_streams_per_connection));

        // Accept both bi-directional streams (queries + legacy messages) and
        // uni-directional streams (fire-and-forget messages from the new sender).
        let conn_bi = conn.clone();
        let conn_uni = conn.clone();
        let sem_bi = stream_semaphore.clone();
        let sem_uni = stream_semaphore;
        let subs_bi = subscribers.clone();
        let subs_uni = subscribers;
        let peers_bi = peers.clone();
        let peers_uni = peers;
        let stats_bi = msg_stats.clone();
        let stats_uni = msg_stats;

        let bi_loop = async {
            loop {
                let (send, recv) = match conn_bi.accept_bi().await {
                    Ok(streams) => streams,
                    Err(e) => {
                        log::warn!(target: TARGET, "QUIC accept bi-stream from {addr}: {e}");
                        break;
                    }
                };
                let permit = match sem_bi.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let subscribers = subs_bi.clone();
                let peers = peers_bi.clone();
                let stats = stats_bi.clone();
                tokio::spawn(async move {
                    let _permit = permit;
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
                let permit = match sem_uni.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let subscribers = subs_uni.clone();
                let peers = peers_uni.clone();
                let stats = stats_uni.clone();
                tokio::spawn(async move {
                    let _permit = permit;
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
        tokio::select! {
            () = bi_loop => {}
            () = uni_loop => {}
        }
        let is_current =
            inbound.get(&inbound_key).map(|e| e.val().stable_id() == conn_id).unwrap_or(false);
        if is_current {
            inbound.remove(&inbound_key);
        }
        log::info!(
            target: TARGET,
            "Exit QUIC inbound receiver for {addr} (conn_id={conn_id}, removed={is_current})"
        );
    }

    fn key_id_to_server_name(key_id: &KeyId) -> String {
        // DNS labels are limited to 63 chars; 64 hex chars → split into two 32-char labels
        let hex = hex::encode(key_id.data());
        log::trace!(target: TARGET, "key_id_to_server_name {} -> {}.{}", hex, &hex[..32], &hex[32..]);
        format!("{}.{}", &hex[..32], &hex[32..])
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
                msg_stats.record(extract_inner_tag(&msg.data), msg.data.len(), addr, false, false);
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
                let _ = send.finish();
                log::debug!(
                    target: TARGET,
                    "process_incoming_stream from {addr}: finished send side"
                );
            }
            Ok(Request::Quic_Query(query)) => {
                msg_stats.record(
                    extract_inner_tag(&query.data),
                    query.data.len(),
                    addr,
                    false,
                    true,
                );
                log::debug!(target: TARGET, "process_incoming_stream from {addr}: QUIC QUERY");
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
                        let response = QuicAnswer { data: data.into() }.into_boxed();
                        send.write_all(&serialize_boxed(&response)?)
                            .await
                            .map_err(|e| error!("QUIC write answer to {addr}: {e}"))?;
                    }
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
                msg_stats.record(extract_inner_tag(&msg.data), msg.data.len(), addr, false, false);
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

    async fn resolve_duplicate_connection(
        inbound: Arc<QuicInboundMap>,
        new_conn: quinn::Connection,
        key: QuicInboundKey,
        addr: SocketAddr,
    ) {
        use rand::Rng;
        let delay_ms = rand::thread_rng().gen_range(500..=2500);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let old_alive =
            inbound.get(&key).map(|e| e.val().close_reason().is_none()).unwrap_or(false);
        let new_alive = new_conn.close_reason().is_none();

        if old_alive && new_alive {
            if let Some(old) = inbound.remove(&key) {
                log::info!(
                    target: TARGET,
                    "Closing old duplicate inbound from {addr} (both alive after {delay_ms}ms)"
                );
                old.val().close(0u32.into(), b"Replaced by new inbound");
            }
            let nc = new_conn.clone();
            let _ = add_unbound_object_to_map_with_update(&inbound, key, |_| Ok(Some(nc.clone())));
        } else if new_alive {
            inbound.remove(&key);
            let nc = new_conn.clone();
            let _ = add_unbound_object_to_map_with_update(&inbound, key, |_| Ok(Some(nc.clone())));
            log::debug!(
                target: TARGET,
                "Old inbound from {addr} already closed, keeping new"
            );
        } else {
            log::debug!(
                target: TARGET,
                "New inbound from {addr} already closed, keeping old"
            );
        }
    }

    /// Drain the send queue and exit. Spawned when `message()` has no live
    /// connection and must enqueue data for later delivery. The task establishes
    /// the connection, sends all queued messages, and terminates.
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

        loop {
            // Drain the queue
            while let Some(data) = send_queue.pop() {
                if let Err(e) =
                    quic.send_message(&peers, addr, &server_name, &outbound, &data).await
                {
                    log::warn!(target: TARGET, "QUIC sender to {addr} error: {e}");
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
    async fn send_message(
        &self,
        peers: &AdnlPeers,
        addr: SocketAddr,
        server_name: &str,
        outbound: &Connections<QuicOutboundConnection>,
        data: &[u8],
    ) -> Result<()> {
        let entry = Self::get_or_create_outbound_connection(outbound, addr)?;
        match entry.conn {
            Some(ref conn) => {
                if let Err(e) = Self::send_via_stream(conn, data).await {
                    log::warn!(
                        target: TARGET,
                        "QUIC send to {addr} failed: {e}, removing dead connection"
                    );
                    Self::remove_dead_connection(outbound, addr, conn);
                    return Err(e);
                }
            }
            None => {
                log::info!(target: TARGET, "QUIC sender: connecting to {addr}");
                self.connect(peers, addr, server_name).await?;
                log::info!(target: TARGET, "QUIC sender: connected to {addr}");
                let entry = Self::get_or_create_outbound_connection(outbound, addr)?;
                if let Some(ref conn) = entry.conn {
                    Self::send_via_stream(conn, data).await?;
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
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} failed: {e}, removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                    }
                    Err(_) => {
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {} timed out ({timeout_ms}ms), \
                            removing dead connection and retrying",
                            peers.other()
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
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
        max_streams_per_connection: usize,
        cancellation_token: tokio_util::sync::CancellationToken,
        inbound: Arc<QuicInboundMap>,
        msg_stats: Arc<MsgStats>,
    ) {
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
                        let addr = incoming.remote_address();
                        log::debug!(target: TARGET, "Accept in QUIC server on {bind_addr} from {addr}");

                        let token = cancellation_token.clone();
                        let lkn = local_key_names.clone();
                        let scr = server_cert_resolver.clone();
                        let ib = inbound.clone();
                        let subs = subscribers.clone();
                        let stats = msg_stats.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {
                                    log::debug!(target: TARGET, "QUIC connection handler for {addr} cancelled");
                                }
                                _ = Self::handle_connection(
                                    incoming, lkn, scr, ib, subs, bind_addr,
                                    max_streams_per_connection, stats,
                                ) => {}
                            }
                        });
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
                                removed += 1;
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
                            let snap = ConnSnapshot::from_stats(&s);
                            let delta = prev.get(&id).map(|p| snap.delta(p)).unwrap_or(snap);
                            prev.insert(id, snap);
                            fmt::Write::write_fmt(
                                &mut dump,
                                format_args!(
                                    "  outbound peer={addr} \
                                    dtx={} bytes/{} dgrams drx={} bytes/{} dgrams \
                                    dlost={} pkts rtt={:?} cwnd={} mtu={} key={key_id:.8}\n",
                                    delta.tx_bytes,
                                    delta.tx_dgrams,
                                    delta.rx_bytes,
                                    delta.rx_dgrams,
                                    delta.lost_pkts,
                                    s.path.rtt,
                                    s.path.cwnd,
                                    s.path.current_mtu,
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
                        let QuicInboundKey(ref local_id, ref peer_id) = *conn_entry.key();
                        let addr = conn_entry.val().remote_address();
                        let conn = conn_entry.val();
                        let s = conn.stats();
                        let id = (conn.stable_id(), false);
                        seen.insert(id);
                        total += 1;
                        let snap = ConnSnapshot::from_stats(&s);
                        let delta = prev.get(&id).map(|p| snap.delta(p)).unwrap_or(snap);
                        prev.insert(id, snap);
                        fmt::Write::write_fmt(
                            &mut dump,
                            format_args!(
                                "  inbound peer={addr} local={local_id} remote={peer_id} \
                                dtx={} bytes/{} dgrams drx={} bytes/{} dgrams \
                                dlost={} pkts rtt={:?} cwnd={} mtu={}\n",
                                delta.tx_bytes,
                                delta.tx_dgrams,
                                delta.rx_bytes,
                                delta.rx_dgrams,
                                delta.lost_pkts,
                                s.path.rtt,
                                s.path.cwnd,
                                s.path.current_mtu,
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
                        fmt::Write::write_fmt(&mut dump, format_args!("  peer {}:\n", key.addr,))
                            .ok();
                    }
                    let dir = if key.is_outbound { "out" } else { " in" };
                    let kind = if key.is_query { "query" } else { "msg  " };
                    fmt::Write::write_fmt(
                        &mut dump,
                        format_args!(
                            "    {dir}/{kind} {:#010x}({}) count={count} bytes={bytes}\n",
                            key.tag,
                            tl_tag_name(key.tag),
                        ),
                    )
                    .ok();
                }

                fmt::Write::write_fmt(&mut dump, format_args!(
                    "  total: {total} connections, {} msg entries",
                    msg_entries.len(),
                )).ok();

                log::info!(target: TARGET, "{dump}");
            }
        });
    }
}

/// Extract the "inner" TL constructor tag from message data.
///
/// QUIC message payloads are typically wrapped in an overlay prefix
/// (`overlay.message` or `overlay.query`). The outer tag is not useful
/// for diagnostics. This function skips past the overlay wrapper and
/// returns the constructor tag of the actual inner payload.
///
/// `overlay.message` and `overlay.query` have a fixed layout:
///   constructor(4 bytes) + int256(32 bytes) = 36 bytes prefix.
/// `WithExtra` variants have a variable-length extra field, so we
/// fall back to `deserialize_boxed_with_suffix` for those.
fn extract_inner_tag(data: &[u8]) -> u32 {
    if data.len() < 4 {
        return 0;
    }
    let outer = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    // overlay.message / overlay.query: fixed 36-byte prefix (constructor + int256)
    const FIXED_PREFIX: usize = 4 + 32;
    match outer {
        0x75252420 | 0xccfd8443 => {
            // overlay.message, overlay.query
            if data.len() >= FIXED_PREFIX + 4 {
                let s = &data[FIXED_PREFIX..];
                return u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            }
            outer
        }
        0xa232233d | 0x94ffc3e9 => {
            // overlay.messageWithExtra, overlay.queryWithExtra
            if let Ok((_obj, suffix_offset)) = deserialize_boxed_with_suffix(data) {
                if suffix_offset + 4 <= data.len() {
                    let s = &data[suffix_offset..];
                    return u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                }
            }
            outer
        }
        _ => outer,
    }
}

/// Map well-known TL constructor tags to short human-readable names for log output.
fn tl_tag_name(tag: u32) -> &'static str {
    match tag {
        0x75252420 => "overlay.message",
        0xa232233d => "overlay.messageWithExtra",
        0xccfd8443 => "overlay.query",
        0x94ffc3e9 => "overlay.queryWithExtra",
        0xb15a2b6b => "overlay.broadcast",
        0xbad7c36a => "overlay.broadcastFec",
        0xf1881342 => "overlay.broadcastFecShort",
        0x46efae62 => "overlay.broadcastStream",
        0xf99fd63d => "overlay.broadcastTwostepFec",
        0x80b859b0 => "overlay.broadcastTwostepSimple",
        0x33534e24 => "overlay.unicast",
        0xd55c14ec => "overlay.fec.received",
        0x09d76914 => "overlay.fec.completed",
        0x48ee64ab => "overlay.getRandomPeers",
        0xa58e7ecc => "overlay.getRandomPeersV2",
        0x690cb481 => "overlay.ping",
        0x236758c4 => "catchain.blockUpdate",
        0x9283ce37 => "validatorSession.blockUpdate",
        0xbe7b573a => "consensus.simplex.certificate",
        0xc37ef4f3 => "consensus.simplex.vote",
        _ => "unknown",
    }
}

/// Snapshot of cumulative counters from a single connection, used to compute deltas.
#[derive(Clone, Copy)]
struct ConnSnapshot {
    tx_bytes: u64,
    tx_dgrams: u64,
    rx_bytes: u64,
    rx_dgrams: u64,
    lost_pkts: u64,
}

impl ConnSnapshot {
    fn from_stats(s: &quinn::ConnectionStats) -> Self {
        Self {
            tx_bytes: s.udp_tx.bytes,
            tx_dgrams: s.udp_tx.datagrams,
            rx_bytes: s.udp_rx.bytes,
            rx_dgrams: s.udp_rx.datagrams,
            lost_pkts: s.path.lost_packets,
        }
    }

    fn delta(&self, prev: &Self) -> Self {
        Self {
            tx_bytes: self.tx_bytes.saturating_sub(prev.tx_bytes),
            tx_dgrams: self.tx_dgrams.saturating_sub(prev.tx_dgrams),
            rx_bytes: self.rx_bytes.saturating_sub(prev.rx_bytes),
            rx_dgrams: self.rx_dgrams.saturating_sub(prev.rx_dgrams),
            lost_pkts: self.lost_pkts.saturating_sub(prev.lost_pkts),
        }
    }
}

/// Per-TL-tag message counters (lock-free atomics, collected per dump interval).
struct MsgTagCounters {
    count: AtomicU64,
    bytes: AtomicU64,
}

impl MsgTagCounters {
    fn new() -> Self {
        Self { count: AtomicU64::new(0), bytes: AtomicU64::new(0) }
    }

    fn record(&self, size: usize) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(size as u64, Ordering::Relaxed);
    }

    /// Take current values and reset to zero.
    fn take(&self) -> (u64, u64) {
        (self.count.swap(0, Ordering::Relaxed), self.bytes.swap(0, Ordering::Relaxed))
    }
}

/// Per-peer, per-TL-tag message statistics key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct MsgStatsKey {
    addr: SocketAddr,
    tag: u32,
    is_outbound: bool,
    is_query: bool,
}

/// Tracks per-peer, per-message-kind statistics for QUIC traffic.
struct MsgStats {
    counters: lockfree::map::Map<MsgStatsKey, MsgTagCounters>,
}

impl MsgStats {
    fn new() -> Arc<Self> {
        Arc::new(Self { counters: lockfree::map::Map::new() })
    }

    fn record(&self, tag: u32, size: usize, addr: SocketAddr, is_outbound: bool, is_query: bool) {
        let key = MsgStatsKey { addr, tag, is_outbound, is_query };
        if let Some(entry) = self.counters.get(&key) {
            entry.val().record(size);
            return;
        }
        let _ = add_unbound_object_to_map(&self.counters, key, || Ok(MsgTagCounters::new()));
        if let Some(entry) = self.counters.get(&key) {
            entry.val().record(size);
        }
    }

    /// Drain all counters and return entries sorted by peer then bytes desc.
    /// Entries with zero activity since the last drain are removed
    fn drain(&self) -> Vec<(MsgStatsKey, u64, u64)> {
        let mut result = Vec::new();
        let mut stale = Vec::new();
        for entry in self.counters.iter() {
            let (count, bytes) = entry.val().take();
            if count > 0 {
                result.push((*entry.key(), count, bytes));
            } else {
                stale.push(*entry.key());
            }
        }
        for key in stale {
            self.counters.remove(&key);
        }
        result.sort_by(|a, b| a.0.addr.cmp(&b.0.addr).then(b.2.cmp(&a.2)));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_inner_tag ---

    /// Helper: build an overlay.message (0x75252420) wrapping the given inner tag.
    fn make_overlay_message(inner_tag: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x75252420u32.to_le_bytes()); // outer tag
        buf.extend_from_slice(&[0u8; 32]); // overlay int256
        buf.extend_from_slice(&inner_tag.to_le_bytes()); // inner payload tag
        buf
    }

    /// Helper: build an overlay.query (0xccfd8443) wrapping the given inner tag.
    fn make_overlay_query(inner_tag: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0xccfd8443u32.to_le_bytes());
        buf.extend_from_slice(&[0u8; 32]);
        buf.extend_from_slice(&inner_tag.to_le_bytes());
        buf
    }

    #[test]
    fn test_extract_inner_tag_empty() {
        assert_eq!(extract_inner_tag(&[]), 0);
        assert_eq!(extract_inner_tag(&[1, 2, 3]), 0);
    }

    #[test]
    fn test_extract_inner_tag_unknown_outer() {
        let data = 0xDEADBEEFu32.to_le_bytes();
        assert_eq!(extract_inner_tag(&data), 0xDEADBEEF);
    }

    #[test]
    fn test_extract_inner_tag_overlay_message() {
        let data = make_overlay_message(0x236758c4); // catchain.blockUpdate
        assert_eq!(extract_inner_tag(&data), 0x236758c4);
    }

    #[test]
    fn test_extract_inner_tag_overlay_query() {
        let data = make_overlay_query(0x48ee64ab); // overlay.getRandomPeers
        assert_eq!(extract_inner_tag(&data), 0x48ee64ab);
    }

    #[test]
    fn test_extract_inner_tag_overlay_message_too_short() {
        // outer tag + partial overlay id (not enough for inner tag)
        let mut data = Vec::new();
        data.extend_from_slice(&0x75252420u32.to_le_bytes());
        data.extend_from_slice(&[0u8; 30]); // only 30 bytes, need 32 + 4
        assert_eq!(extract_inner_tag(&data), 0x75252420); // falls back to outer
    }

    // --- MsgStats ---

    fn test_addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn test_msg_stats_record_and_drain() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, false);
        stats.record(0xAA, 200, addr, true, false);
        stats.record(0xBB, 50, addr, true, true);

        let entries = stats.drain();
        assert_eq!(entries.len(), 2);

        // Sorted by addr (same), then bytes desc: AA(300) before BB(50)
        assert_eq!(entries[0].0.tag, 0xAA);
        assert_eq!(entries[0].1, 2); // count
        assert_eq!(entries[0].2, 300); // bytes

        assert_eq!(entries[1].0.tag, 0xBB);
        assert_eq!(entries[1].1, 1);
        assert_eq!(entries[1].2, 50);
    }

    #[test]
    fn test_msg_stats_drain_sorts_by_addr_then_bytes() {
        let stats = MsgStats::new();
        let addr_a = test_addr(1000);
        let addr_b = test_addr(2000);

        stats.record(0xAA, 10, addr_b, true, false);
        stats.record(0xBB, 500, addr_a, true, false);
        stats.record(0xCC, 100, addr_a, true, false);

        let entries = stats.drain();
        assert_eq!(entries.len(), 3);

        // addr_a (port 1000) first, sorted by bytes desc
        assert_eq!(entries[0].0.addr, addr_a);
        assert_eq!(entries[0].0.tag, 0xBB); // 500 bytes
        assert_eq!(entries[1].0.addr, addr_a);
        assert_eq!(entries[1].0.tag, 0xCC); // 100 bytes

        // addr_b (port 2000) last
        assert_eq!(entries[2].0.addr, addr_b);
        assert_eq!(entries[2].0.tag, 0xAA);
    }

    #[test]
    fn test_msg_stats_drain_resets_counters() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, false);
        let entries = stats.drain();
        assert_eq!(entries.len(), 1);

        // Second drain: no new activity, should return empty
        let entries = stats.drain();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_msg_stats_drain_evicts_stale_keys() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, false);
        stats.record(0xBB, 50, addr, false, false);

        // First drain: both active, counters reset
        let _ = stats.drain();

        // Only record on 0xAA
        stats.record(0xAA, 200, addr, true, false);

        // Second drain: 0xBB was idle → evicted
        let _ = stats.drain();

        // Record on 0xBB again — must re-insert (was evicted)
        stats.record(0xBB, 30, addr, false, false);
        let entries = stats.drain();

        // 0xAA was idle since last drain (evicted), only 0xBB with activity is returned
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.tag, 0xBB);
        assert_eq!(entries[0].2, 30);
    }

    #[test]
    fn test_msg_stats_distinguishes_direction_and_kind() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, false); // outbound msg
        stats.record(0xAA, 200, addr, false, false); // inbound msg
        stats.record(0xAA, 300, addr, true, true); // outbound query

        let entries = stats.drain();
        assert_eq!(entries.len(), 3);
    }
}
