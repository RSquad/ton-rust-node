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
    fmt,
    net::SocketAddr,
    sync::{Arc, Once},
    time::Duration,
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
    last_added_name: Arc<std::sync::Mutex<Option<String>>>,
}

impl QuicServerCertResolver {
    fn new(
        keys: Arc<lockfree::map::Map<String, Arc<rustls::sign::CertifiedKey>>>,
        last_added_name: Arc<std::sync::Mutex<Option<String>>>,
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
    last_added_name: Arc<std::sync::Mutex<Option<String>>>,
}

pub struct QuicNode {
    cancellation_token: tokio_util::sync::CancellationToken,
    /// One entry per local identity; each carries its own client config and outbound pool.
    local_keys: lockfree::map::Map<Arc<KeyId>, Arc<LocalKeyState>>,
    /// One endpoint per unique bind port. Endpoints are created lazily by `add_key()`.
    endpoints: std::sync::Mutex<std::collections::HashMap<u16, Arc<EndpointState>>>,
    /// Shared subscriber list for all accept loops.
    subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    peer_keys: lockfree::map::Map<Arc<KeyId>, SocketAddr>,
    /// Max concurrent in-flight streams per inbound connection.
    max_streams_per_connection: usize,
}

impl QuicNode {
    pub const OFFSET_PORT: u16 = 1000;
    const DEFAULT_QUERY_TIMEOUT_MS: u64 = 5000;

    /// How often the background checker scans outbound connections for dead ones.
    const CONNECTION_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

    const DEFAULT_MAX_STREAMS_PER_CONNECTION: usize = 256;

    /// Maximum number of messages buffered per outbound peer
    const SEND_QUEUE_CAPACITY: usize = 1024;

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
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(15))
                .expect("15s fits in IdleTimeout"),
        ));
        client_transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
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
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
    ) -> Result<Option<usize>> {
        self.ensure_peer_registered(adnl, src, dst)?;
        let data = serialize_boxed(&QuicMessage { data: data.into() }.into_boxed())?;
        match self.get_outbound_connection(src, dst, true).await? {
            QuicOutboundConnection { conn: Some(ref conn), ref send_queue } => {
                if send_queue.check(true) {
                    if !send_queue.try_push(data) {
                        send_queue.check(false);
                        fail!("QUIC send queue full for peer {dst}");
                    }
                    while !send_queue.check(false) {
                        tokio::task::yield_now().await;
                    }
                } else {
                    let len = data.len();
                    if let Err(e) = Self::send_via_stream(conn, &data).await {
                        log::warn!(
                            target: TARGET,
                            "QUIC send_message to {dst} failed: {e}, removing dead connection"
                        );
                        let addr = self.addr_by_key(dst)?;
                        let state = self.local_key_state(src)?;
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                        return Err(e);
                    }
                    return Ok(Some(len));
                }
            }
            QuicOutboundConnection { conn: None, ref send_queue } => {
                if !send_queue.try_push(data) {
                    fail!("QUIC send queue full for peer {dst} (connecting)");
                }
            }
        }
        Ok(None)
    }

    /// Create a new QuicNode. No endpoints are bound — they are created lazily
    /// by `add_key()` when the first identity for a given port is registered.
    pub fn new(
        subscribers: Vec<Arc<dyn Subscriber>>,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> Arc<Self> {
        Self::with_stream_limit(
            subscribers,
            cancellation_token,
            Self::DEFAULT_MAX_STREAMS_PER_CONNECTION,
        )
    }

    pub async fn query(
        self: &Arc<Self>,
        data: Vec<u8>,
        adnl: Option<&AdnlNode>,
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
        timeout_ms: Option<u64>,
    ) -> Result<Option<Vec<u8>>> {
        self.ensure_peer_registered(adnl, src, dst)?;
        let timeout_ms = timeout_ms.unwrap_or(Self::DEFAULT_QUERY_TIMEOUT_MS);
        let wire = serialize_boxed(&QuicQuery { data: data.into() }.into_boxed())?;
        let response = self.send_query_raw(wire, src, dst, timeout_ms).await?;
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

    /// Like `new`, but with a custom per-connection stream concurrency limit.
    pub fn with_stream_limit(
        subscribers: Vec<Arc<dyn Subscriber>>,
        cancellation_token: tokio_util::sync::CancellationToken,
        max_streams_per_connection: usize,
    ) -> Arc<Self> {
        static CRYPTO_INIT: Once = Once::new();
        CRYPTO_INIT.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install default Rustls CryptoProvider");
        });
        let transport = Arc::new(Self {
            cancellation_token: cancellation_token.clone(),
            local_keys: lockfree::map::Map::new(),
            endpoints: std::sync::Mutex::new(std::collections::HashMap::new()),
            subscribers: Arc::new(subscribers),
            peer_keys: lockfree::map::Map::new(),
            max_streams_per_connection,
        });
        Self::spawn_connection_checker(Arc::downgrade(&transport), cancellation_token);
        transport
    }

    fn addr_by_key(&self, key_id: &Arc<KeyId>) -> Result<SocketAddr> {
        match self.peer_keys.get(key_id) {
            Some(entry) => Ok(*entry.val()),
            None => fail!("No address registered for peer key {key_id}"),
        }
    }

    async fn connect(
        &self,
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
        addr: SocketAddr,
        server_name: &str,
    ) -> Result<()> {
        let state = self.local_key_state(src)?;
        let endpoint = self.endpoint_for_port(state.bound_port)?;
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
                }))
            } else {
                Ok(None)
            }
        })? {
            conn.close(0u32.into(), b"Duplicate QUIC connection");
        }
        Ok(())
    }

    /// Get the endpoint for the given port.
    fn endpoint_for_port(&self, port: u16) -> Result<quinn::Endpoint> {
        let endpoints = self.endpoints.lock().map_err(|e| error!("Endpoints lock: {e}"))?;
        endpoints
            .get(&port)
            .map(|s| s.endpoint.clone())
            .ok_or_else(|| error!("No QUIC endpoint for port {port}"))
    }

    fn ensure_peer_registered(
        &self,
        adnl: Option<&AdnlNode>,
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
    ) -> Result<()> {
        if self.has_peer_key(dst) {
            return Ok(());
        }
        let Some(adnl) = adnl else {
            fail!("QUIC peer {dst} is not registered and no ADNL node provided");
        };
        let mut addr = adnl
            .peer_ip_address(src, dst)?
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
        let last_added_name: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
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
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(15))
                .expect("15s fits in IdleTimeout"),
        ));
        // Keep established connections alive so the idle timeout only fires on
        // truly dead peers, not on connections that are just quiet between rounds.
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
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

        Self::spawn_accept_loop(
            endpoint.clone(),
            local_key_names.clone(),
            server_cert_resolver,
            self.subscribers.clone(),
            bind_addr,
            self.max_streams_per_connection,
            self.cancellation_token.clone(),
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
                    // Proactive liveness check: if the connection is dead, remove it
                    // and loop again — the next iteration will see conn: None and
                    // trigger a reconnect.
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
                    });
                }
                None => {
                    let queue = QuicSendQueue::with_capacity(Self::SEND_QUEUE_CAPACITY);
                    add_unbound_object_to_map(outbound.map(), addr, || {
                        Ok(QuicOutboundConnection { conn: None, send_queue: queue.clone() })
                    })?;
                }
            }
        }
    }

    async fn get_outbound_connection(
        self: &Arc<Self>,
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
        create_async: bool,
    ) -> Result<QuicOutboundConnection> {
        let addr = self.addr_by_key(dst)?;
        let server_name = Self::key_id_to_server_name(dst);
        let state = self.local_key_state(src)?;
        loop {
            let conn = Self::get_or_create_outbound_connection(&state.outbound, addr)?;
            if let QuicOutboundConnection { conn: Some(_), .. } = &conn {
                break Ok(conn);
            }
            if create_async {
                let queue = conn.send_queue.clone();
                let quic = self.clone();
                let src = src.clone();
                let dst = dst.clone();
                let server_name = server_name.clone();
                spawn_cancelable(self.cancellation_token.clone(), async move {
                    while !queue.activate(true) {
                        tokio::task::yield_now().await;
                    }
                    loop {
                        let Some(data) = queue.pop() else {
                            if queue.activate(false) {
                                break;
                            }
                            tokio::task::yield_now().await;
                            continue;
                        };
                        loop {
                            let result = quic.local_key_state(&src).and_then(|s| {
                                Self::get_or_create_outbound_connection(&s.outbound, addr)
                            });
                            let result = match result {
                                Ok(QuicOutboundConnection { conn: Some(ref conn), .. }) => {
                                    Self::send_via_stream(conn, &data).await
                                }
                                Ok(_) => {
                                    log::info!(
                                        target: TARGET,
                                        "Try new QUIC connection to {addr} in background"
                                    );
                                    let result = quic.connect(&src, &dst, addr, &server_name).await;
                                    if let Err(e) = result {
                                        Err(error!(
                                            "QUIC background connection to {addr} error: {e}"
                                        ))
                                    } else {
                                        log::info!(
                                            target: TARGET,
                                            "QUIC connected to {addr} in background"
                                        );
                                        continue;
                                    }
                                }
                                Err(e) => Err(e),
                            };
                            if let Err(e) = result {
                                log::warn!(
                                    target: TARGET,
                                    "QUIC send to {addr} in background error: {e}"
                                );
                            }
                            break;
                        }
                    }
                });
                break Ok(conn);
            } else {
                log::info!(target: TARGET, "Try new QUIC connection to {addr} in foreground");
                self.connect(&src, dst, addr, &server_name).await?;
                log::info!(target: TARGET, "QUIC connected to {addr} in foreground");
            }
        }
    }

    async fn handle_connection(
        incoming: quinn::Incoming,
        local_key_names: Arc<lockfree::map::Map<String, Arc<KeyId>>>,
        server_cert_resolver: Arc<QuicServerCertResolver>,
        inbound: Arc<Connections<quinn::Connection>>,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        bind_addr: SocketAddr,
        max_streams_per_connection: usize,
    ) {
        let addr = incoming.remote_address();
        // Bound handshake time: C++ ngtcp2 clients abandon after ~3-5s and retry,
        // so a handshake still in progress after 5s is almost certainly stale.
        // Without this, stale Connecting futures accumulate inside quinn's endpoint,
        // slowing its internal event loop and delaying endpoint.accept() for new peers.
        let conn = match tokio::time::timeout(std::time::Duration::from_secs(5), incoming).await {
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

        let had_existing = {
            let mut found_existing = false;
            let result = add_unbound_object_to_map_with_update(inbound.map(), addr, |existing| {
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
            tokio::spawn(Self::resolve_duplicate_connection(inbound.clone(), conn.clone(), addr));
        }

        let peers = AdnlPeers::with_keys(local_key_id, peer_key_id);
        let conn_id = conn.stable_id();
        // Limit concurrent in-flight streams per connection to bound memory usage.
        // When the semaphore is full, accept_bi() stalls, applying QUIC-level backpressure.
        let stream_semaphore = Arc::new(tokio::sync::Semaphore::new(max_streams_per_connection));
        loop {
            let (send, recv) = match conn.accept_bi().await {
                Ok(streams) => streams,
                Err(e) => {
                    log::warn!(
                        target: TARGET,
                        "QUIC accept stream from {addr}: {e}"
                    );
                    break;
                }
            };
            let permit = match stream_semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let subscribers = subscribers.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) =
                    Self::process_incoming_stream(recv, send, &subscribers, &peers, addr).await
                {
                    log::warn!(
                        target: TARGET,
                        "QUIC process stream from {addr}: {e}"
                    );
                }
            });
        }
        let is_current =
            inbound.map().get(&addr).map(|e| e.val().stable_id() == conn_id).unwrap_or(false);
        if is_current {
            inbound.map().remove(&addr);
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
    ) -> Result<()> {
        log::debug!(target: TARGET, "process_incoming_stream from {addr}: reading data...");
        let buf = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
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
        log::debug!(target: TARGET, "process_incoming_stream from {addr}: read {} bytes", buf.len());
        if buf.is_empty() {
            return Ok(());
        }
        let obj = deserialize_boxed(&buf)
            .map_err(|e| error!("Cannot deserialize QUIC message from {addr}: {e}"))?;
        log::debug!(target: TARGET, "process_incoming_stream from {addr}: deserialized TL, about to downcast");
        match obj.downcast::<Request>() {
            Ok(Request::Quic_Message(msg)) => {
                log::debug!(target: TARGET, "process_incoming_stream from {addr}: QUIC MESSAGE, dispatching to {} subscribers", subscribers.len());
                for subscriber in subscribers {
                    if subscriber.try_consume_custom(&msg.data, &peers).await? {
                        log::debug!(target: TARGET, "process_incoming_stream from {addr}: consumed by subscriber");
                        break;
                    }
                }
                let _ = send.finish();
                log::debug!(target: TARGET, "process_incoming_stream from {addr}: finished send side");
            }
            Ok(Request::Quic_Query(query)) => {
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
                log::warn!(target: TARGET, "Unknown QUIC TL message from {addr}: failed to downcast to Request");
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
        inbound: Arc<Connections<quinn::Connection>>,
        new_conn: quinn::Connection,
        addr: SocketAddr,
    ) {
        use rand::Rng;
        let delay_ms = rand::thread_rng().gen_range(500..=2500);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

        let old_alive =
            inbound.map().get(&addr).map(|e| e.val().close_reason().is_none()).unwrap_or(false);
        let new_alive = new_conn.close_reason().is_none();

        if old_alive && new_alive {
            if let Some(old) = inbound.map().remove(&addr) {
                log::info!(
                    target: TARGET,
                    "Closing old duplicate inbound from {addr} (both alive after {delay_ms}ms)"
                );
                old.val().close(0u32.into(), b"Replaced by new inbound");
            }
            let nc = new_conn.clone();
            let _ = add_unbound_object_to_map_with_update(inbound.map(), addr, |_| {
                Ok(Some(nc.clone()))
            });
        } else if new_alive {
            inbound.map().remove(&addr);
            let nc = new_conn.clone();
            let _ = add_unbound_object_to_map_with_update(inbound.map(), addr, |_| {
                Ok(Some(nc.clone()))
            });
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

    async fn send_query_raw(
        self: &Arc<Self>,
        data: Vec<u8>,
        src: &Arc<KeyId>,
        dst: &Arc<KeyId>,
        timeout_ms: u64,
    ) -> Result<Vec<u8>> {
        let addr = self.addr_by_key(dst)?;
        let state = self.local_key_state(src)?;
        let timeout = Duration::from_millis(timeout_ms);

        // First attempt
        match self.get_outbound_connection(src, dst, false).await? {
            QuicOutboundConnection { conn: Some(ref conn), .. } => {
                let result =
                    tokio::time::timeout(timeout, Self::send_via_stream(conn, &data)).await;
                match result {
                    Ok(Ok(response)) => return Ok(response),
                    Ok(Err(e)) => {
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {dst} failed: {e}, removing dead connection and retrying"
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                    }
                    Err(_) => {
                        log::warn!(
                            target: TARGET,
                            "QUIC query to {dst} timed out ({timeout_ms}ms), removing dead connection and retrying"
                        );
                        Self::remove_dead_connection(&state.outbound, addr, conn);
                    }
                }
            }
            _ => fail!("Cannot create QUIC connection to {dst} in foreground"),
        }

        // Retry once with a fresh connection
        match self.get_outbound_connection(src, dst, false).await? {
            QuicOutboundConnection { conn: Some(ref conn), .. } => {
                Self::send_via_stream(conn, &data).await
            }
            _ => fail!("Cannot create QUIC connection to {dst} in foreground (retry)"),
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
            std::time::Duration::from_secs(30),
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
    ) {
        tokio::spawn(async move {
            let inbound: Arc<Connections<quinn::Connection>> = Connections::new();
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
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {
                                    log::debug!(target: TARGET, "QUIC connection handler for {addr} cancelled");
                                }
                                _ = Self::handle_connection(
                                    incoming, lkn, scr, ib, subs, bind_addr,
                                    max_streams_per_connection,
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
        weak: std::sync::Weak<QuicNode>,
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
                                // Fully remove entry if queue is drained
                                if let Some(entry) = outbound.map().get(&addr) {
                                    let s = entry.val();
                                    if s.conn.is_none() && s.send_queue.is_inactive() {
                                        outbound.map().remove(&addr);
                                    }
                                }
                                removed += 1;
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
}
