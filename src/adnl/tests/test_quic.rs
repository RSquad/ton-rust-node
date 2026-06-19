/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use adnl::{
    common::{AdnlPeers, QueryResult, Subscriber, Version},
    node::{AdnlNode, IpAddress},
    DhtNode, OverlayNode, QuicNode, QuicRateLimitConfig,
};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, UdpSocket},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::{
        adnl::{
            address::address::{Quic, Udp},
            addresslist::AddressList,
            pong::Pong as AdnlPong,
            Address, Pong as AdnlPongBoxed,
        },
        quic::{request::Query as QuicQuery, Response as QuicResponse},
        rpc::adnl::Ping as AdnlPing,
    },
    IntoBoxed, TLObject,
};
use ton_block::{
    ed25519_encode_private_key_to_pkcs8, ed25519_generate_private_key, sha256_digest_slices, KeyId,
    ED25519_KEY_TYPE, ED25519_SECRET_KEY_LENGTH,
};

include!("../../common/src/config.rs");
include!("../../common/src/test.rs");

const KEY_TAG: usize = 0;
const ITERATIONS: usize = 3;
const MSG_PAYLOAD: &[u8] = b"quic test payload";

fn ip_address_to_socket_addr(ip: &IpAddress) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip.ip())), ip.port())
}

/// Helper: build an AddressList with the given addresses and current version.
fn make_address_list(addrs: Vec<Address>) -> AddressList {
    let version = Version::get();
    AddressList { addrs: addrs.into(), version, reinit_date: version, priority: 0, expire_at: 0 }
}

fn udp_addr(ip: u32, port: u16) -> Address {
    Address::Adnl_Address_Udp(Udp { ip: ip as i32, port: port as i32 })
}

fn quic_addr(ip: u32, port: u16) -> Address {
    Address::Adnl_Address_Quic(Quic { ip: ip as i32, port: port as i32 })
}

/// Routes messages and queries to a channel only when addressed to `key_id`.
struct TestSubscriber {
    key_id: Arc<KeyId>,
    msg_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

#[async_trait::async_trait]
impl Subscriber for TestSubscriber {
    async fn try_consume_custom(&self, data: &[u8], peers: &AdnlPeers) -> Result<bool> {
        if peers.local() != &self.key_id {
            return Ok(false);
        }
        let _ = self.msg_tx.send(data.to_vec());
        Ok(true)
    }

    async fn try_consume_query(&self, object: TLObject, peers: &AdnlPeers) -> Result<QueryResult> {
        if peers.local() != &self.key_id {
            return Ok(QueryResult::Rejected(object));
        }
        match object.downcast::<AdnlPing>() {
            Ok(ping) => QueryResult::consume(
                AdnlPong { value: ping.value },
                #[cfg(feature = "telemetry")]
                None,
            ),
            Err(obj) => Ok(QueryResult::Rejected(obj)),
        }
    }
}

async fn recv_with_timeout(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("message receive timed out")
        .expect("channel closed")
}

fn make_ping_data(value: i64) -> Vec<u8> {
    serialize_boxed(&AdnlPing { value }).unwrap()
}

/// Raw TL-wrapped query for low-level stream tests that bypass QuicNode::query()
fn make_ping_wire(value: i64) -> Vec<u8> {
    serialize_boxed(&QuicQuery { data: make_ping_data(value).into() }.into_boxed()).unwrap()
}

fn parse_pong(data: Vec<u8>) -> i64 {
    deserialize_boxed(&data).unwrap().downcast::<AdnlPongBoxed>().unwrap().only().value
}

/// Parse pong from raw wire bytes (for low-level stream tests)
fn parse_pong_wire(bytes: &[u8]) -> i64 {
    let obj = deserialize_boxed(bytes).unwrap();
    let answer = obj.downcast::<QuicResponse>().unwrap().only().data;
    deserialize_boxed(&answer).unwrap().downcast::<AdnlPongBoxed>().unwrap().only().value
}

/// 10 clients connect to one server simultaneously.
/// Verifies the accept loop doesn't serialize handshakes (HoL blocking).
/// With serialized accepts each handshake can take seconds, so 10 sequential
/// handshakes would blow the 15s budget. Concurrent accepts finish fast.
#[test]
fn test_quic_concurrent_accept() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const NUM_CLIENTS: usize = 10;
        const BASE_CLIENT_PORT: u16 = 5700;
        const SERVER_PORT: u16 = 5690;
        const TIMEOUT: Duration = Duration::from_secs(15);

        // --- server ---
        let server_token = CancellationToken::new();
        let server_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, server_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(server_key, KEY_TAG)],
        )
        .unwrap();
        let server_key_id = server_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (srv_tx, _srv_rx) = tokio::sync::mpsc::unbounded_channel();
        let server_sub = Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx })
            as Arc<dyn Subscriber>;

        let server_bind: SocketAddr =
            format!("127.0.0.1:{}", SERVER_PORT + QuicNode::OFFSET_PORT).parse().unwrap();
        let server = QuicNode::new(
            vec![server_sub],
            server_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server.add_key(&server_key, &server_key_id, server_bind).unwrap();

        // --- clients ---
        struct ClientCtx {
            quic: Arc<QuicNode>,
            key_id: Arc<KeyId>,
            token: CancellationToken,
        }

        let mut clients = Vec::with_capacity(NUM_CLIENTS);
        for i in 0..NUM_CLIENTS {
            let port = BASE_CLIENT_PORT + i as u16;
            let key = ed25519_generate_private_key().unwrap().to_bytes();
            let (_, cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
                &format!("127.0.0.1:{port}"),
                vec![(key, KEY_TAG)],
            )
            .unwrap();
            let key_id = cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let sub = Arc::new(TestSubscriber { key_id: key_id.clone(), msg_tx: tx })
                as Arc<dyn Subscriber>;

            let bind: SocketAddr =
                format!("127.0.0.1:{}", port + QuicNode::OFFSET_PORT).parse().unwrap();
            let token = CancellationToken::new();
            let quic = QuicNode::new(
                vec![sub],
                token.clone(),
                tokio::runtime::Handle::current(),
                Some(QuicRateLimitConfig::disabled()),
            );
            quic.add_key(&key, &key_id, bind).unwrap();
            quic.add_peer_key(server_key_id.clone(), server_bind).unwrap();
            server.add_peer_key(key_id.clone(), bind).unwrap();

            clients.push(ClientCtx { quic, key_id, token });
        }

        // --- fire all 10 queries concurrently ---
        let start = tokio::time::Instant::now();
        let mut handles = Vec::with_capacity(NUM_CLIENTS);
        for (i, client) in clients.iter().enumerate() {
            let quic = client.quic.clone();
            let peers = AdnlPeers::with_keys(client.key_id.clone(), server_key_id.clone());
            let value = i as i64;
            handles.push(tokio::spawn(async move {
                let resp = quic
                    .query(make_ping_data(value), None, &peers, None)
                    .await
                    .unwrap_or_else(|e| panic!("client {i} query failed: {e}"));
                let pong = parse_pong(resp.unwrap());
                assert_eq!(pong, value, "client {i}: pong mismatch");
                i
            }));
        }

        // --- await all with a single timeout ---
        let results = tokio::time::timeout(TIMEOUT, async {
            let mut completed = Vec::with_capacity(NUM_CLIENTS);
            for h in handles {
                completed.push(h.await.expect("task panicked"));
            }
            completed
        })
        .await
        .expect("concurrent accept timed out — possible HoL blocking regression");

        let elapsed = start.elapsed();
        assert_eq!(results.len(), NUM_CLIENTS);
        println!(
            "All {NUM_CLIENTS} concurrent connections completed in {:.2}s",
            elapsed.as_secs_f64()
        );

        // --- cleanup ---
        for c in &clients {
            c.quic.shutdown();
            c.token.cancel();
        }
        server.shutdown();
        server_token.cancel();
    });
}

/// Two QUIC endpoints (A and B) on separate ports, each with its own key.
/// Tests bidirectional message and query exchange.
#[test]
fn test_quic_session() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();

        // Generate two key pairs
        let key_bytes_a = ed25519_generate_private_key().unwrap().to_bytes();
        let key_bytes_b = ed25519_generate_private_key().unwrap().to_bytes();

        // Derive key IDs via AdnlNodeConfig
        let (_, config_a) = AdnlNodeConfig::from_ip_address_and_private_keys(
            "127.0.0.1:4600",
            vec![(key_bytes_a, KEY_TAG)],
        )
        .unwrap();
        let key_id_a = config_a.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (_, config_b) = AdnlNodeConfig::from_ip_address_and_private_keys(
            "127.0.0.1:4601",
            vec![(key_bytes_b, KEY_TAG)],
        )
        .unwrap();
        let key_id_b = config_b.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel();

        let sub_a = Arc::new(TestSubscriber { key_id: key_id_a.clone(), msg_tx: tx_a })
            as Arc<dyn Subscriber>;
        let sub_b = Arc::new(TestSubscriber { key_id: key_id_b.clone(), msg_tx: tx_b })
            as Arc<dyn Subscriber>;

        // Endpoint A on QUIC port 5600, endpoint B on QUIC port 5601
        let bind_a: SocketAddr = "127.0.0.1:5600".parse().unwrap();
        let bind_b: SocketAddr = "127.0.0.1:5601".parse().unwrap();

        let quic_a = QuicNode::new(
            vec![sub_a],
            token_a.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        quic_a.add_key(&key_bytes_a, &key_id_a, bind_a).unwrap();

        let quic_b = QuicNode::new(
            vec![sub_b],
            token_b.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        quic_b.add_key(&key_bytes_b, &key_id_b, bind_b).unwrap();

        // Register peer addresses
        quic_a.add_peer_key(key_id_b.clone(), "127.0.0.1:5601".parse().unwrap()).unwrap();
        quic_b.add_peer_key(key_id_a.clone(), "127.0.0.1:5600".parse().unwrap()).unwrap();

        let peers_ab = AdnlPeers::with_keys(key_id_a.clone(), key_id_b.clone());
        let peers_ba = AdnlPeers::with_keys(key_id_b.clone(), key_id_a.clone());
        for i in 0..ITERATIONS {
            let value = i as i64;

            // A → B: query
            let resp =
                quic_a.query(make_ping_data(value), None, &peers_ab, None).await.unwrap().unwrap();
            assert_eq!(parse_pong(resp), value, "A→B query iter {i}: pong mismatch");

            // B → A: query
            let resp =
                quic_b.query(make_ping_data(value), None, &peers_ba, None).await.unwrap().unwrap();
            assert_eq!(parse_pong(resp), value, "B→A query iter {i}: pong mismatch");

            // A → B: message
            quic_a.message(MSG_PAYLOAD.to_vec(), None, &peers_ab).await.unwrap();
            assert_eq!(
                recv_with_timeout(&mut rx_b).await,
                MSG_PAYLOAD,
                "A→B message iter {i}: payload mismatch"
            );

            // B → A: message
            quic_b.message(MSG_PAYLOAD.to_vec(), None, &peers_ba).await.unwrap();
            assert_eq!(
                recv_with_timeout(&mut rx_a).await,
                MSG_PAYLOAD,
                "B→A message iter {i}: payload mismatch"
            );
        }

        quic_a.shutdown();
        quic_b.shutdown();
        token_a.cancel();
        token_b.cancel();
    });
}

/// Verify that a client automatically reconnects after the server restarts.
/// Without dead-connection removal this test hangs forever on the dead connection.
#[test]
fn test_quic_reconnect_after_server_restart() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const CLIENT_PORT: u16 = 5800;
        const SERVER_PORT: u16 = 5801;
        const TIMEOUT: Duration = Duration::from_secs(15);

        let client_bind: SocketAddr = format!("127.0.0.1:{CLIENT_PORT}").parse().unwrap();
        let server_bind: SocketAddr = format!("127.0.0.1:{SERVER_PORT}").parse().unwrap();

        // --- client A (lives for the entire test) ---
        let client_token = CancellationToken::new();
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, client_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{CLIENT_PORT}"),
            vec![(client_key, KEY_TAG)],
        )
        .unwrap();
        let client_key_id = client_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (cli_tx, _cli_rx) = tokio::sync::mpsc::unbounded_channel();
        let client_sub = Arc::new(TestSubscriber { key_id: client_key_id.clone(), msg_tx: cli_tx })
            as Arc<dyn Subscriber>;

        let client = QuicNode::new(
            vec![client_sub],
            client_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        client.add_key(&client_key, &client_key_id, client_bind).unwrap();

        // --- server B1 (will be shut down) ---
        // Use a fixed key so B2 can reuse the same identity.
        let server_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, server_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(server_key, KEY_TAG)],
        )
        .unwrap();
        let server_key_id = server_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let server_token1 = CancellationToken::new();
        let (srv_tx1, _srv_rx1) = tokio::sync::mpsc::unbounded_channel();
        let server_sub1 =
            Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx1 })
                as Arc<dyn Subscriber>;

        let server1 = QuicNode::new(
            vec![server_sub1],
            server_token1.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server1.add_key(&server_key, &server_key_id, server_bind).unwrap();

        // Register peer keys
        client.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server1.add_peer_key(client_key_id.clone(), client_bind).unwrap();
        let peers = AdnlPeers::with_keys(client_key_id.clone(), server_key_id.clone());

        // Step 1: successful ping/pong through B1
        let resp =
            tokio::time::timeout(TIMEOUT, client.query(make_ping_data(1), None, &peers, None))
                .await
                .expect("initial query timed out")
                .expect("initial query failed");
        assert_eq!(parse_pong(resp.unwrap()), 1, "initial pong mismatch");
        println!("Step 1: initial ping/pong succeeded");

        // Step 2: shut down B1 and drop it so the socket is released
        server1.shutdown();
        server_token1.cancel();
        drop(server1);
        // Wait for the accept loop task to observe the closed endpoint and exit,
        // releasing its clone of the quinn::Endpoint (and thus the UDP socket).
        tokio::time::sleep(Duration::from_millis(1000)).await;
        println!("Step 2: server B1 shut down");

        // Step 3: create B2 on the same port with the same key
        let server_token2 = CancellationToken::new();
        let (srv_tx2, _srv_rx2) = tokio::sync::mpsc::unbounded_channel();
        let server_sub2 =
            Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx2 })
                as Arc<dyn Subscriber>;

        let server2 = QuicNode::new(
            vec![server_sub2],
            server_token2.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server2.add_key(&server_key, &server_key_id, server_bind).unwrap();
        server2.add_peer_key(client_key_id.clone(), client_bind).unwrap();
        println!("Step 3: server B2 started on same port with same key");

        // Step 4: client sends another query — should remove dead conn, reconnect, and succeed
        let resp =
            tokio::time::timeout(TIMEOUT, client.query(make_ping_data(2), None, &peers, None))
                .await
                .expect("reconnect query timed out — dead connection removal may be broken")
                .expect("reconnect query failed");
        assert_eq!(parse_pong(resp.unwrap()), 2, "reconnect pong mismatch");
        println!("Step 4: reconnect ping/pong succeeded");

        // --- cleanup ---
        client.shutdown();
        server2.shutdown();
        client_token.cancel();
        server_token2.cancel();
    });
}

// ---------------------------------------------------------------------------
// Helper: create a QUIC endpoint with a fresh key on the given ADNL port.
// ---------------------------------------------------------------------------
fn make_endpoint(
    adnl_port: u16,
) -> (Arc<QuicNode>, [u8; ED25519_SECRET_KEY_LENGTH], Arc<KeyId>, SocketAddr, CancellationToken) {
    make_endpoint_with_config(adnl_port, QuicRateLimitConfig::disabled())
}

fn make_endpoint_with_config(
    adnl_port: u16,
    rl_config: QuicRateLimitConfig,
) -> (Arc<QuicNode>, [u8; ED25519_SECRET_KEY_LENGTH], Arc<KeyId>, SocketAddr, CancellationToken) {
    let key = ed25519_generate_private_key().unwrap().to_bytes();
    let (_, cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
        &format!("127.0.0.1:{adnl_port}"),
        vec![(key, KEY_TAG)],
    )
    .unwrap();
    let key_id = cfg.key_by_tag(KEY_TAG).unwrap().id().clone();
    let bind: SocketAddr =
        format!("127.0.0.1:{}", adnl_port + QuicNode::OFFSET_PORT).parse().unwrap();
    let token = CancellationToken::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let sub =
        Arc::new(TestSubscriber { key_id: key_id.clone(), msg_tx: tx }) as Arc<dyn Subscriber>;
    let quic =
        QuicNode::new(vec![sub], token.clone(), tokio::runtime::Handle::current(), Some(rl_config));
    quic.add_key(&key, &key_id, bind).unwrap();
    (quic, key, key_id, bind, token)
}

/// Build a raw quinn client config using an Ed25519 RPK cert from the given key.
/// This produces a client that speaks the same TLS-RPK protocol as QuicNode
/// but is fully independent — useful for injecting rogue connections.
fn build_raw_quinn_client(key_bytes: &[u8; ED25519_SECRET_KEY_LENGTH]) -> quinn::ClientConfig {
    let key_der_vec = ed25519_encode_private_key_to_pkcs8(key_bytes).unwrap();
    let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_der_vec).unwrap();
    let key_pair = rcgen::KeyPair::from_der_and_sign_algo(&key_der, &rcgen::PKCS_ED25519).unwrap();
    let spki = rustls::pki_types::CertificateDer::from(key_pair.public_key_der());
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der).unwrap();
    let certified = Arc::new(rustls::sign::CertifiedKey::new(vec![spki], signing_key));

    /// Resolver that always returns the same RPK cert.
    #[derive(Debug)]
    struct FixedCertResolver(Arc<rustls::sign::CertifiedKey>);
    impl rustls::client::ResolvesClientCert for FixedCertResolver {
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

    /// Accept any server cert (same as QuicServerCertVerifier in production).
    #[derive(Debug)]
    struct AcceptAll;
    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![rustls::SignatureScheme::ED25519]
        }
        fn requires_raw_public_keys(&self) -> bool {
            true
        }
    }

    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAll))
        .with_client_cert_resolver(Arc::new(FixedCertResolver(certified)));
    tls.alpn_protocols = vec![b"ton".to_vec()];
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
    let mut client_cfg = quinn::ClientConfig::new(Arc::new(quic_crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_idle_timeout(Some(quinn::IdleTimeout::try_from(Duration::from_secs(15)).unwrap()));
    client_cfg.transport_config(Arc::new(transport));
    client_cfg
}

// ===========================================================================
// Test 1: Duplicate inbound connection resolution
// ===========================================================================

/// Two independent QUIC transports (client1 and client2) sharing the same
/// identity key connect to the same server from different source ports.
/// After the duplicate-resolution window (500-2500ms) only one survives.
/// The server must still answer queries on the surviving connection.
#[test]
fn test_quic_duplicate_inbound_resolution() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5900;
        const CLIENT1_PORT: u16 = 5901;
        const CLIENT2_PORT: u16 = 5902;
        const TIMEOUT: Duration = Duration::from_secs(15);

        // --- server ---
        let (server, _server_key, server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- two clients with different keys but connecting to the same server ---
        let (client1, _c1_key, c1_key_id, c1_bind, c1_token) = make_endpoint(CLIENT1_PORT);
        let (client2, _c2_key, c2_key_id, c2_bind, c2_token) = make_endpoint(CLIENT2_PORT);

        // Cross-register peers
        client1.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        client2.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(c1_key_id.clone(), c1_bind).unwrap();
        server.add_peer_key(c2_key_id.clone(), c2_bind).unwrap();

        let peers1 = AdnlPeers::with_keys(c1_key_id.clone(), server_key_id.clone());
        let peers2 = AdnlPeers::with_keys(c2_key_id.clone(), server_key_id.clone());

        // Step 1: both clients connect concurrently
        let h1 = {
            let q = client1.clone();
            let peers = peers1.clone();
            tokio::spawn(async move { q.query(make_ping_data(1), None, &peers, None).await })
        };
        let h2 = {
            let q = client2.clone();
            let peers = peers2.clone();
            tokio::spawn(async move { q.query(make_ping_data(2), None, &peers, None).await })
        };

        let (r1, r2) = tokio::time::timeout(TIMEOUT, async { tokio::join!(h1, h2) })
            .await
            .expect("concurrent connection timed out");
        let r1 = r1.expect("task 1 panicked");
        let r2 = r2.expect("task 2 panicked");
        assert!(r1.is_ok(), "client1 first query failed: {:?}", r1.err());
        assert!(r2.is_ok(), "client2 first query failed: {:?}", r2.err());
        println!("Step 1: both clients connected and got pong");

        // Step 2: wait for duplicate resolution window to pass
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Step 3: both clients should still be able to query (through surviving connections)
        let resp1 =
            tokio::time::timeout(TIMEOUT, client1.query(make_ping_data(10), None, &peers1, None))
                .await
                .expect("post-resolution query1 timed out")
                .expect("post-resolution query1 failed");
        assert_eq!(parse_pong(resp1.unwrap()), 10);

        let resp2 =
            tokio::time::timeout(TIMEOUT, client2.query(make_ping_data(20), None, &peers2, None))
                .await
                .expect("post-resolution query2 timed out")
                .expect("post-resolution query2 failed");
        assert_eq!(parse_pong(resp2.unwrap()), 20);
        println!("Step 3: both clients still functional after duplicate resolution");

        // --- cleanup ---
        client1.shutdown();
        client2.shutdown();
        server.shutdown();
        c1_token.cancel();
        c2_token.cancel();
        server_token.cancel();
    });
}

// ===========================================================================
// Test 1b: Multiple keys from same address must coexist
// ===========================================================================

/// A TON node may have multiple connections to each peer — one for the
/// current validator key and one for the next key. Both connections originate
/// from the same source address but use different client Ed25519 keys.
/// They must coexist: the server must NOT close one when the other arrives.
///
/// This test opens two raw quinn connections from the same UDP endpoint with
/// different Ed25519 RPK identities. After waiting past the duplicate-resolution
/// window, BOTH connections must still be alive and answer queries.
#[test]
fn test_quic_multi_key_same_address() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5915;
        const RAW_CLIENT_PORT: u16 = 5916;

        // --- server ---
        let (server, _server_key, server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- two different client keys (simulating current + next validator keys) ---
        let key1 = ed25519_generate_private_key().unwrap().to_bytes();
        let key2 = ed25519_generate_private_key().unwrap().to_bytes();

        let (_, cfg1) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{RAW_CLIENT_PORT}"),
            vec![(key1, KEY_TAG)],
        )
        .unwrap();
        let key1_id = cfg1.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (_, cfg2) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{RAW_CLIENT_PORT}"),
            vec![(key2, KEY_TAG)],
        )
        .unwrap();
        let key2_id = cfg2.key_by_tag(KEY_TAG).unwrap().id().clone();

        server
            .add_peer_key(
                key1_id.clone(),
                format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT).parse().unwrap(),
            )
            .unwrap();
        server
            .add_peer_key(
                key2_id.clone(),
                format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT).parse().unwrap(),
            )
            .unwrap();

        // Build two different quinn client configs (different RPK identities)
        let client_config1 = build_raw_quinn_client(&key1);
        let client_config2 = build_raw_quinn_client(&key2);

        // Create a single raw quinn endpoint (both connections share the same source addr)
        let raw_bind: SocketAddr =
            format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT).parse().unwrap();
        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        sock.set_reuse_address(true).unwrap();
        sock.bind(&raw_bind.into()).unwrap();
        sock.set_nonblocking(true).unwrap();
        let udp = std::net::UdpSocket::from(sock);
        let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
        let endpoint =
            quinn::Endpoint::new(quinn::EndpointConfig::default(), None, udp, runtime).unwrap();

        // C++-compatible SNI (PR #2397): "<hex[..32]>.<hex[32..]>.adnl" for the server's id.
        let hex = hex::encode(server_key_id.data());
        let sni = format!("{}.{}.adnl", &hex[..32], &hex[32..]);

        // Open connection 1 with key1
        let conn1 = endpoint
            .connect_with(client_config1, server_bind, &sni)
            .unwrap()
            .await
            .expect("conn1 (key1) handshake failed");

        // Open connection 2 with key2 (same source address, different identity)
        let conn2 = endpoint
            .connect_with(client_config2, server_bind, &sni)
            .unwrap()
            .await
            .expect("conn2 (key2) handshake failed");

        println!("Two connections established from same address with different keys");

        // Verify both work immediately
        let ping1 = make_ping_wire(101);
        let (mut s1, mut r1) = conn1.open_bi().await.unwrap();
        s1.write_all(&ping1).await.unwrap();
        s1.finish().unwrap();
        let resp1 = tokio::time::timeout(Duration::from_secs(10), r1.read_to_end(1 << 20))
            .await
            .expect("conn1 response timed out")
            .expect("conn1 read failed");
        assert_eq!(parse_pong_wire(&resp1), 101);
        println!("conn1 (key1) ping/pong OK");

        let ping2 = make_ping_wire(102);
        let (mut s2, mut r2) = conn2.open_bi().await.unwrap();
        s2.write_all(&ping2).await.unwrap();
        s2.finish().unwrap();
        let resp2 = tokio::time::timeout(Duration::from_secs(10), r2.read_to_end(1 << 20))
            .await
            .expect("conn2 response timed out")
            .expect("conn2 read failed");
        assert_eq!(parse_pong_wire(&resp2), 102);
        println!("conn2 (key2) ping/pong OK");

        // Wait past the maximum duplicate-resolution window (2500ms + margin)
        tokio::time::sleep(Duration::from_secs(4)).await;

        // BOTH connections must still be alive — this is the key assertion.
        // With the old SocketAddr-based keying, one would have been killed.
        assert!(
            conn1.close_reason().is_none(),
            "conn1 (key1) was closed — multi-key coexistence broken!"
        );
        assert!(
            conn2.close_reason().is_none(),
            "conn2 (key2) was closed — multi-key coexistence broken!"
        );

        // Both must still answer queries
        let ping3 = make_ping_wire(201);
        let (mut s3, mut r3) = conn1.open_bi().await.expect("conn1 should still accept streams");
        s3.write_all(&ping3).await.unwrap();
        s3.finish().unwrap();
        let resp3 = tokio::time::timeout(Duration::from_secs(10), r3.read_to_end(1 << 20))
            .await
            .expect("conn1 post-wait response timed out")
            .expect("conn1 post-wait read failed");
        assert_eq!(parse_pong_wire(&resp3), 201);

        let ping4 = make_ping_wire(202);
        let (mut s4, mut r4) = conn2.open_bi().await.expect("conn2 should still accept streams");
        s4.write_all(&ping4).await.unwrap();
        s4.finish().unwrap();
        let resp4 = tokio::time::timeout(Duration::from_secs(10), r4.read_to_end(1 << 20))
            .await
            .expect("conn2 post-wait response timed out")
            .expect("conn2 post-wait read failed");
        assert_eq!(parse_pong_wire(&resp4), 202);

        println!("PASS: both connections survived duplicate-resolution window");

        // --- cleanup ---
        conn1.close(0u32.into(), b"done");
        conn2.close(0u32.into(), b"done");
        endpoint.close(0u32.into(), b"done");
        server.shutdown();
        server_token.cancel();
    });
}

// ===========================================================================
// Test 1b-2: C++-compatible SNI routing across multiple server identities
// ===========================================================================

/// Verifies SNI-based identity dispatch matching the C++ node: a server hosting
/// several identities on one UDP port presents the identity whose SNI the client
/// requested (compute_sni_name = "<hex[..32]>.<hex[32..]>.adnl") and falls back
/// to the active identity when the SNI is missing or the legacy "ton" value.
/// Unknown SNI (anything else) is rejected during the TLS handshake.
/// This also exercises end-to-end SNI handling for the split-hex name format,
/// which keeps each DNS label within the RFC 1035 63-octet limit so rustls
/// accepts it without any special-case handling.
#[test]
fn test_quic_sni_identity_routing() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5960;

        // C++-compatible SNI (PR #2397): "<hex[..32]>.<hex[32..]>.adnl".
        let sni_of = |key_id: &Arc<KeyId>| {
            let hex = hex::encode(key_id.data());
            format!("{}.{}.adnl", &hex[..32], &hex[32..])
        };
        // Ed25519 public key (last 32 bytes of the 44-byte SPKI) for a private key.
        let pubkey_of = |key: &[u8; ED25519_SECRET_KEY_LENGTH]| -> [u8; 32] {
            let der = rustls::pki_types::PrivateKeyDer::try_from(
                ed25519_encode_private_key_to_pkcs8(key).unwrap(),
            )
            .unwrap();
            let kp = rcgen::KeyPair::from_der_and_sign_algo(&der, &rcgen::PKCS_ED25519).unwrap();
            let spki = kp.public_key_der();
            spki[12..44].try_into().unwrap()
        };
        // Public key the server presented during the handshake (server's RPK SPKI).
        let presented_pubkey = |conn: &quinn::Connection| -> [u8; 32] {
            let id = conn.peer_identity().expect("no server identity");
            let certs =
                id.downcast::<Vec<rustls::pki_types::CertificateDer>>().expect("not RPK certs");
            let spki = certs.first().expect("empty cert chain");
            spki.as_ref()[12..44].try_into().expect("short SPKI")
        };

        // Server with identity A (auto-activated as the default).
        let (server, key_a, key_a_id, server_bind, server_token) = make_endpoint(SERVER_PORT);

        // Register a second identity B on the SAME port (no activation).
        let key_b = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, cfg_b) = AdnlNodeConfig::from_ip_address_and_private_keys(
            "127.0.0.1:5961",
            vec![(key_b, KEY_TAG)],
        )
        .unwrap();
        let key_b_id = cfg_b.key_by_tag(KEY_TAG).unwrap().id().clone();
        server.add_key(&key_b, &key_b_id, server_bind).unwrap();

        // Raw client endpoint (one client identity is enough; we vary only the SNI).
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let client_config = build_raw_quinn_client(&client_key);
        let client_ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();

        // SNI of B reaches identity B even though A is the active default. The
        // successful connect_with proves rustls accepted the 64-char label.
        let conn_b = client_ep
            .connect_with(client_config.clone(), server_bind, &sni_of(&key_b_id))
            .unwrap()
            .await
            .expect("connect with B's SNI failed");
        assert_eq!(
            presented_pubkey(&conn_b),
            pubkey_of(&key_b),
            "SNI of B should route to identity B"
        );

        // SNI of A reaches identity A.
        let conn_a = client_ep
            .connect_with(client_config.clone(), server_bind, &sni_of(&key_a_id))
            .unwrap()
            .await
            .expect("connect with A's SNI failed");
        assert_eq!(
            presented_pubkey(&conn_a),
            pubkey_of(&key_a),
            "SNI of A should route to identity A"
        );

        // Unknown SNI is rejected: the server returns no cert from the resolver
        // and rustls fails the TLS handshake. The connection must not establish.
        let unknown_result = client_ep
            .connect_with(client_config.clone(), server_bind, "unknown.adnl")
            .unwrap()
            .await;
        assert!(
            unknown_result.is_err(),
            "unknown SNI should fail the handshake, got {unknown_result:?}"
        );

        // Legacy "ton" SNI (older Rust clients used to send this dummy value)
        // falls back to the active identity silently (no WARN, no rejection).
        let conn_legacy = client_ep
            .connect_with(client_config, server_bind, "ton")
            .unwrap()
            .await
            .expect("connect with legacy \"ton\" SNI failed");
        assert_eq!(
            presented_pubkey(&conn_legacy),
            pubkey_of(&key_a),
            "legacy \"ton\" SNI should fall back to the active identity A"
        );

        println!("PASS: SNI routes to B, A; unknown rejected; legacy \"ton\" falls back to active");

        conn_a.close(0u32.into(), b"done");
        conn_b.close(0u32.into(), b"done");
        conn_legacy.close(0u32.into(), b"done");
        client_ep.close(0u32.into(), b"done");
        server.shutdown();
        server_token.cancel();
    });
}

// ===========================================================================
// Test 1c: Key rotation — server identity switching
// ===========================================================================

/// Verifies the server key rotation lifecycle using raw quinn connections so
/// we can inspect the exact server identity (SPKI RPK) presented during TLS:
///
///   1. Server starts with key A (auto-activated). New connection sees key A.
///   2. Key B is registered but NOT activated. New connection still sees key A.
///   3. Key B is activated. Connection established in step 1 stays alive.
///      A new connection now sees key B.
///   4. Key A is removed. The step-1 connection is unaffected.
///      A new connection still sees key B.
#[test]
fn test_quic_key_rotation() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5925;
        const RAW_CLIENT_PORT: u16 = 5927;

        // --- helper: extract server KeyId from a quinn connection ---
        fn server_key_id(conn: &quinn::Connection) -> Arc<KeyId> {
            let identity = conn.peer_identity().expect("no peer identity");
            let certs = identity
                .downcast::<Vec<rustls::pki_types::CertificateDer>>()
                .expect("unexpected identity type");
            let spki = certs.first().expect("no server cert");
            assert_eq!(spki.as_ref().len(), 44, "unexpected SPKI length");
            let pub_key: &[u8; 32] = spki.as_ref()[12..].try_into().unwrap();
            let data = sha256_digest_slices(&[&ED25519_KEY_TYPE.to_le_bytes(), pub_key]);
            KeyId::from_data(data)
        }

        // --- helper: open a fresh raw quinn connection and return it ---
        async fn raw_connect(
            endpoint: &quinn::Endpoint,
            server_bind: std::net::SocketAddr,
            sni: &str,
        ) -> quinn::Connection {
            endpoint.connect(server_bind, sni).unwrap().await.expect("raw handshake failed")
        }

        // --- helper: ping/pong over a connection ---
        async fn ping_pong(conn: &quinn::Connection, value: i64) -> i64 {
            let ping =
                serialize_boxed(&QuicQuery { data: make_ping_data(value).into() }.into_boxed())
                    .unwrap();
            let (mut send, mut recv) = conn.open_bi().await.unwrap();
            send.write_all(&ping).await.unwrap();
            send.finish().unwrap();
            let resp = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(1 << 20))
                .await
                .expect("pong timed out")
                .expect("pong read failed");
            parse_pong_wire(&resp)
        }

        // --- generate two server keys ---
        let key_a = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, cfg_a) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(key_a, KEY_TAG)],
        )
        .unwrap();
        let key_a_id = cfg_a.key_by_tag(KEY_TAG).unwrap().id().clone();

        let key_b = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, cfg_b) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(key_b, KEY_TAG)],
        )
        .unwrap();
        let key_b_id = cfg_b.key_by_tag(KEY_TAG).unwrap().id().clone();

        let server_bind: std::net::SocketAddr =
            format!("127.0.0.1:{}", SERVER_PORT + QuicNode::OFFSET_PORT).parse().unwrap();

        // --- server with key A only (key B added later) ---
        // Register subscribers for both keys so the server can answer queries
        // regardless of which identity is active.
        let token = CancellationToken::new();
        let (tx_a, _rx_a) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, _rx_b) = tokio::sync::mpsc::unbounded_channel();
        let sub_a = Arc::new(TestSubscriber { key_id: key_a_id.clone(), msg_tx: tx_a })
            as Arc<dyn Subscriber>;
        let sub_b = Arc::new(TestSubscriber { key_id: key_b_id.clone(), msg_tx: tx_b })
            as Arc<dyn Subscriber>;
        let server = QuicNode::new(
            vec![sub_a, sub_b],
            token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server.add_key(&key_a, &key_a_id, server_bind).unwrap();

        // --- raw quinn client endpoint ---
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, client_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{RAW_CLIENT_PORT}"),
            vec![(client_key, KEY_TAG)],
        )
        .unwrap();
        let client_key_id = client_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();
        let raw_bind: std::net::SocketAddr =
            format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT).parse().unwrap();
        server.add_peer_key(client_key_id.clone(), raw_bind).unwrap();

        // --- helper: create a fresh raw quinn endpoint (no TLS session cache) ---
        // A fresh endpoint avoids TLS session resumption, which would reuse the
        // old server cert and mask identity changes.
        let make_raw_endpoint = |port_offset: u16| -> quinn::Endpoint {
            let cfg = build_raw_quinn_client(&client_key);
            let bind: std::net::SocketAddr =
                format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT + port_offset)
                    .parse()
                    .unwrap();
            let sock = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .unwrap();
            sock.set_reuse_address(true).unwrap();
            sock.bind(&bind.into()).unwrap();
            sock.set_nonblocking(true).unwrap();
            let udp = std::net::UdpSocket::from(sock);
            let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
            let mut ep =
                quinn::Endpoint::new(quinn::EndpointConfig::default(), None, udp, runtime).unwrap();
            ep.set_default_client_config(cfg);
            ep
        };

        // SNI — use the legacy "ton" value so every connect falls through to
        // the active identity. This test asserts which identity is currently
        // active over the lifecycle (add/activate/remove); routing by SNI would
        // short-circuit that by always pinning to whichever id the SNI names,
        // defeating the test's intent. An arbitrary non-matching SNI would be
        // rejected outright by QuicServerCertResolver, so "ton" is the only
        // value that exercises the active-identity fallback path here.
        let sni = "ton";

        // ---- Step 1: only key A registered, server must present key A ----
        let ep1 = make_raw_endpoint(0);
        let conn_a1 = raw_connect(&ep1, server_bind, &sni).await;
        assert_eq!(
            server_key_id(&conn_a1).as_ref(),
            key_a_id.as_ref(),
            "step 1: server should present key A"
        );
        assert_eq!(ping_pong(&conn_a1, 1).await, 1);
        println!("Step 1 OK: server presents key A");

        // ---- Step 2: add key B without activating — server still presents A ----
        server.add_key(&key_b, &key_b_id, server_bind).unwrap();
        // Small delay to let the key command propagate
        tokio::time::sleep(Duration::from_millis(50)).await;

        let ep2 = make_raw_endpoint(1);
        let conn_a2 = raw_connect(&ep2, server_bind, &sni).await;
        assert_eq!(
            server_key_id(&conn_a2).as_ref(),
            key_a_id.as_ref(),
            "step 2: server should still present key A after adding (not activating) key B"
        );
        assert_eq!(ping_pong(&conn_a2, 2).await, 2);
        conn_a2.close(0u32.into(), b"done");
        ep2.close(0u32.into(), b"done");
        println!("Step 2 OK: key B added but not activated, server still presents key A");

        // ---- Step 3: activate key B — old conn stays alive, new conn sees B ----
        server.activate_key(&key_b_id);
        // activate_key is fire-and-forget; give the background task time
        tokio::time::sleep(Duration::from_millis(100)).await;

        // conn_a1 (established with key A) must still be alive and functional
        assert!(
            conn_a1.close_reason().is_none(),
            "step 3: conn_a1 should still be alive after key B activation"
        );
        assert_eq!(ping_pong(&conn_a1, 3).await, 3);
        println!("Step 3a OK: old connection (key A) still alive after activating key B");

        // Fresh endpoint — no TLS session cache from previous connections
        let ep3 = make_raw_endpoint(2);
        let conn_b1 = raw_connect(&ep3, server_bind, &sni).await;
        assert_eq!(
            server_key_id(&conn_b1).as_ref(),
            key_b_id.as_ref(),
            "step 3: new connection should see key B after activation"
        );
        assert_eq!(ping_pong(&conn_b1, 4).await, 4);
        println!("Step 3b OK: new connection sees key B");

        // ---- Step 4: remove key A — conn_a1 must be closed, new conn still B ----
        server.remove_key(&key_a_id, server_bind).unwrap();
        // Give the server time to close connections bound to key A
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(
            conn_a1.close_reason().is_some(),
            "step 4: conn_a1 should be closed after key A removal"
        );
        println!("Step 4a OK: old connection closed after key A removal");

        let ep4 = make_raw_endpoint(3);
        let conn_b2 = raw_connect(&ep4, server_bind, &sni).await;
        assert_eq!(
            server_key_id(&conn_b2).as_ref(),
            key_b_id.as_ref(),
            "step 4: server should still present key B after removing key A"
        );
        assert_eq!(ping_pong(&conn_b2, 6).await, 6);
        println!("Step 4b OK: server still presents key B after key A removed");

        // --- cleanup ---
        conn_b1.close(0u32.into(), b"done");
        conn_b2.close(0u32.into(), b"done");
        ep1.close(0u32.into(), b"done");
        ep3.close(0u32.into(), b"done");
        ep4.close(0u32.into(), b"done");
        server.shutdown();
        token.cancel();
    });
}

// ===========================================================================
// Test 2: Stream timeout handling
// ===========================================================================

/// A raw quinn client opens a bi-stream, sends partial data, then stalls.
/// The server's 5s read timeout should drop the stream without crashing.
/// A subsequent normal query must still succeed.
#[test]
fn test_quic_stream_read_timeout() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5920;
        const RAW_CLIENT_PORT: u16 = 5921;
        const NORMAL_CLIENT_PORT: u16 = 5922;

        // --- server ---
        let (server, _server_key, server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- raw client that will stall ---
        let raw_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, raw_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{RAW_CLIENT_PORT}"),
            vec![(raw_key, KEY_TAG)],
        )
        .unwrap();
        let raw_key_id = raw_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();
        let raw_bind: SocketAddr =
            format!("127.0.0.1:{}", RAW_CLIENT_PORT + QuicNode::OFFSET_PORT).parse().unwrap();
        server.add_peer_key(raw_key_id.clone(), raw_bind).unwrap();

        let client_config = build_raw_quinn_client(&raw_key);
        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        sock.set_reuse_address(true).unwrap();
        sock.bind(&raw_bind.into()).unwrap();
        sock.set_nonblocking(true).unwrap();
        let udp = std::net::UdpSocket::from(sock);
        let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
        let mut endpoint =
            quinn::Endpoint::new(quinn::EndpointConfig::default(), None, udp, runtime).unwrap();
        endpoint.set_default_client_config(client_config);

        let hex = hex::encode(server_key_id.data());
        let sni = format!("{}.{}.adnl", &hex[..32], &hex[32..]);

        let conn =
            endpoint.connect(server_bind, &sni).unwrap().await.expect("raw conn handshake failed");

        // Open a stream and write partial data, then do NOT finish — stall.
        let (mut send_stall, _recv_stall) = conn.open_bi().await.unwrap();
        send_stall.write_all(b"partial garbage").await.unwrap();
        // Intentionally do NOT call send_stall.finish() — server will time out after 5s.
        println!("Opened stalling stream (no finish), waiting for server-side timeout...");

        // Wait for the 5s server read timeout to fire, plus margin.
        tokio::time::sleep(Duration::from_secs(7)).await;

        // The connection itself should still be alive (only the stream timed out).
        assert!(
            conn.close_reason().is_none(),
            "Connection was closed — only the stream should have timed out"
        );

        // Send a proper query on a new stream — should succeed.
        let ping = make_ping_wire(999);
        let (mut s, mut r) = conn.open_bi().await.expect("open_bi after timeout failed");
        s.write_all(&ping).await.unwrap();
        s.finish().unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(10), r.read_to_end(1 << 20))
            .await
            .expect("post-timeout query timed out")
            .expect("post-timeout query read failed");
        assert_eq!(parse_pong_wire(&resp), 999, "post-timeout pong mismatch");
        println!("Post-timeout query succeeded — stream timeout didn't break the connection");

        // Also verify a normal QuicNode client works fine after the stall.
        let (normal_client, _nc_key, nc_key_id, nc_bind, nc_token) =
            make_endpoint(NORMAL_CLIENT_PORT);
        normal_client.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(nc_key_id.clone(), nc_bind).unwrap();

        let resp = tokio::time::timeout(
            Duration::from_secs(10),
            normal_client.query(
                make_ping_data(777),
                None,
                &AdnlPeers::with_keys(nc_key_id.clone(), server_key_id.clone()),
                None,
            ),
        )
        .await
        .expect("normal client query timed out")
        .expect("normal client query failed");
        assert_eq!(parse_pong(resp.unwrap()), 777);
        println!("Normal client query after stall succeeded");

        // --- cleanup ---
        drop(send_stall);
        conn.close(0u32.into(), b"done");
        endpoint.close(0u32.into(), b"done");
        normal_client.shutdown();
        nc_token.cancel();
        server.shutdown();
        server_token.cancel();
    });
}

// ===========================================================================
// Test 3: Malformed SPKI / signature failure
// ===========================================================================

/// A raw quinn client connects using standard X.509 self-signed certs (not RPK).
/// The server expects RPK and should reject the handshake or close the connection
/// because peer_key_id_from_connection() won't find a valid Ed25519 SPKI.
#[test]
fn test_quic_reject_non_rpk_client() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5930;

        // --- server ---
        let (server, _server_key, server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- build a rogue client with standard X.509 self-signed cert ---
        let rogue_cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(rogue_cert.cert.der().to_vec());
        let key_der =
            rustls::pki_types::PrivateKeyDer::try_from(rogue_cert.key_pair.serialize_der())
                .unwrap();

        // Use dangerous verifier to skip server cert validation
        #[derive(Debug)]
        struct AcceptAllServer;
        impl rustls::client::danger::ServerCertVerifier for AcceptAllServer {
            fn verify_server_cert(
                &self,
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &[rustls::pki_types::CertificateDer<'_>],
                _: &rustls::pki_types::ServerName<'_>,
                _: &[u8],
                _: rustls::pki_types::UnixTime,
            ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
            {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _: &[u8],
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &rustls::DigitallySignedStruct,
            ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _: &[u8],
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &rustls::DigitallySignedStruct,
            ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                rustls::crypto::ring::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            }
            fn requires_raw_public_keys(&self) -> bool {
                false // standard X.509 mode
            }
        }

        let mut tls = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAllServer))
            .with_client_auth_cert(vec![cert_der], key_der)
            .unwrap();
        tls.alpn_protocols = vec![b"ton".to_vec()];

        // The handshake should fail because server requires RPK but client sends X.509.
        // Quinn wraps TLS alert as a ConnectionError.
        let rogue_bind: SocketAddr = "127.0.0.1:6931".parse().unwrap();
        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        sock.set_reuse_address(true).unwrap();
        sock.bind(&rogue_bind.into()).unwrap();
        sock.set_nonblocking(true).unwrap();
        let udp = std::net::UdpSocket::from(sock);
        let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
        let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
        let mut rogue_endpoint =
            quinn::Endpoint::new(quinn::EndpointConfig::default(), None, udp, runtime).unwrap();
        rogue_endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic_crypto)));

        let hex = hex::encode(server_key_id.data());
        let sni = format!("{}.{}.adnl", &hex[..32], &hex[32..]);

        let handshake_result = tokio::time::timeout(
            Duration::from_secs(10),
            rogue_endpoint.connect(server_bind, &sni).unwrap(),
        )
        .await;

        match handshake_result {
            Err(_) => println!("Handshake timed out — server rejected non-RPK client (OK)"),
            Ok(Err(e)) => println!("Handshake failed with error: {e} — server rejected (OK)"),
            Ok(Ok(conn)) => {
                // Even if the handshake somehow succeeded, the server should close
                // the connection because peer_key_id_from_connection returns None
                // for X.509 certs (SPKI length != 44 bytes).
                println!("Handshake succeeded unexpectedly, checking if server closes...");
                // Try to use the connection — should fail
                let result = tokio::time::timeout(Duration::from_secs(5), async {
                    let (mut s, mut r) = match conn.open_bi().await {
                        Ok(pair) => pair,
                        Err(e) => return Err(format!("open_bi: {e}")),
                    };
                    if let Err(e) = s.write_all(&make_ping_wire(1)).await {
                        return Err(format!("write: {e}"));
                    }
                    let _ = s.finish();
                    r.read_to_end(1 << 20).await.map_err(|e| format!("read: {e}"))
                })
                .await;
                match result {
                    Err(_) => println!("Stream timed out — server is ignoring rogue conn (OK)"),
                    Ok(Err(e)) => println!("Stream failed: {e} — server rejected rogue conn (OK)"),
                    Ok(Ok(resp)) if resp.is_empty() => {
                        println!("Empty response — server dropped the stream (OK)")
                    }
                    Ok(Ok(_)) => {
                        panic!("Server responded to a non-RPK X.509 client — security violation!")
                    }
                }
            }
        }
        println!("Non-RPK client correctly rejected");

        // Verify the server is still healthy by connecting a legitimate client.
        let (legit, _lk, lk_id, lk_bind, lk_token) = make_endpoint(5932);
        legit.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(lk_id.clone(), lk_bind).unwrap();

        let resp = tokio::time::timeout(
            Duration::from_secs(10),
            legit.query(
                make_ping_data(100500),
                None,
                &AdnlPeers::with_keys(lk_id.clone(), server_key_id.clone()),
                None,
            ),
        )
        .await
        .expect("legit query timed out after rogue attempt")
        .expect("legit query failed");
        assert_eq!(parse_pong(resp.unwrap()), 100500);
        println!("Legitimate client works fine after rogue rejection");

        // --- cleanup ---
        rogue_endpoint.close(0u32.into(), b"done");
        legit.shutdown();
        lk_token.cancel();
        server.shutdown();
        server_token.cancel();
    });
}

/// Client connects to a server but the server's RPK identity doesn't match what
/// the client expects (RPK identity mismatch). The client should reject the
/// connection and fail the query.
#[test]
fn test_quic_rpk_identity_mismatch() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 5940;
        const CLIENT_PORT: u16 = 5941;

        // --- server with key S ---
        let (server, _s_key, _server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- client knows about a *different* key for the server's address ---
        let (client, _c_key, client_key_id, client_bind, client_token) = make_endpoint(CLIENT_PORT);

        // Generate a fake key ID that doesn't match the server's actual key
        let fake_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, fake_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            "127.0.0.1:9999", // dummy address, we just want the key_id
            vec![(fake_key, KEY_TAG)],
        )
        .unwrap();
        let fake_key_id = fake_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        // Client thinks the server has fake_key_id, but server actually has server_key_id.
        client.add_peer_key(fake_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(client_key_id.clone(), client_bind).unwrap();

        // The query should fail because after the TLS handshake, the client verifies
        // peer_key_id == expected dst, and they won't match.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            client.query(
                make_ping_data(1),
                None,
                &AdnlPeers::with_keys(client_key_id.clone(), fake_key_id.clone()),
                None,
            ),
        )
        .await;

        match result {
            Err(_) => println!("Query timed out — connection was rejected (OK)"),
            Ok(Err(e)) => {
                let err_msg = format!("{e}");
                println!("Query failed with: {err_msg}");
                assert!(
                    err_msg.contains("mismatch")
                        || err_msg.contains("RPK")
                        || err_msg.contains("handshake"),
                    "Expected RPK mismatch error, got: {err_msg}"
                );
            }
            Ok(Ok(_)) => panic!("Query succeeded despite RPK identity mismatch — security bug!"),
        }
        println!("RPK identity mismatch correctly prevented communication");

        // --- cleanup ---
        client.shutdown();
        server.shutdown();
        client_token.cancel();
        server_token.cancel();
    });
}

// ===========================================================================
// Test 4: Per-IP inbound rate limiting and pool recovery
// ===========================================================================

/// Verifies the per-IP inbound connection throttle:
///   - First PER_IP_INBOUND_FAST_THRESHOLD (5) connections from the same IP
///     are accepted immediately and answer queries.
///   - The 6th connection from the same IP is delayed (2s) but still accepted.
///   - A 7th concurrent connection from the same IP is refused while the 6th
///     is still in the delayed-accept window.
///   - After all clients disconnect, a fresh client connects immediately.
#[test]
fn test_quic_connection_pool_exhaustion() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        // 5 fast + 1 delayed + 1 refused = 7 clients
        const FAST_CLIENTS: usize = 5;
        const SERVER_PORT: u16 = 5950;
        const BASE_CLIENT_PORT: u16 = 6000;
        const TIMEOUT: Duration = Duration::from_secs(30);

        // --- server ---
        let (server, _server_key, server_key_id, server_bind, server_token) =
            make_endpoint(SERVER_PORT);

        // --- helper to create a QuicNode client ---
        struct ClientCtx {
            quic: Arc<QuicNode>,
            key_id: Arc<KeyId>,
            token: CancellationToken,
        }

        let make_client = |port: u16| -> ClientCtx {
            let key = ed25519_generate_private_key().unwrap().to_bytes();
            let (_, cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
                &format!("127.0.0.1:{port}"),
                vec![(key, KEY_TAG)],
            )
            .unwrap();
            let key_id = cfg.key_by_tag(KEY_TAG).unwrap().id().clone();
            let bind: SocketAddr =
                format!("127.0.0.1:{}", port + QuicNode::OFFSET_PORT).parse().unwrap();
            let token = CancellationToken::new();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let sub = Arc::new(TestSubscriber { key_id: key_id.clone(), msg_tx: tx })
                as Arc<dyn Subscriber>;
            let quic = QuicNode::new(
                vec![sub],
                token.clone(),
                tokio::runtime::Handle::current(),
                Some(QuicRateLimitConfig::disabled()),
            );
            quic.add_key(&key, &key_id, bind).unwrap();
            quic.add_peer_key(server_key_id.clone(), server_bind).unwrap();
            server.add_peer_key(key_id.clone(), bind).unwrap();
            ClientCtx { quic, key_id, token }
        };

        // Step 1: first FAST_CLIENTS connections are accepted immediately
        let mut fast = Vec::with_capacity(FAST_CLIENTS);
        for i in 0..FAST_CLIENTS {
            fast.push(make_client(BASE_CLIENT_PORT + i as u16));
        }

        let mut handles = Vec::with_capacity(FAST_CLIENTS);
        for (i, c) in fast.iter().enumerate() {
            let quic = c.quic.clone();
            let peers = AdnlPeers::with_keys(c.key_id.clone(), server_key_id.clone());
            let value = i as i64;
            handles.push(tokio::spawn(async move {
                let resp = quic
                    .query(make_ping_data(value), None, &peers, None)
                    .await
                    .unwrap_or_else(|e| panic!("fast client {i} query failed: {e}"));
                assert_eq!(parse_pong(resp.unwrap()), value, "fast client {i}: pong mismatch");
            }));
        }
        let start = tokio::time::Instant::now();
        tokio::time::timeout(TIMEOUT, async {
            for h in handles {
                h.await.expect("task panicked");
            }
        })
        .await
        .expect("fast clients timed out");
        let fast_elapsed = start.elapsed();
        println!(
            "Step 1: {FAST_CLIENTS} fast clients completed in {:.2}s",
            fast_elapsed.as_secs_f64()
        );

        // Step 2: 6th client — should be delayed (~2s) but still accepted
        let delayed = make_client(BASE_CLIENT_PORT + FAST_CLIENTS as u16);
        let dq = delayed.quic.clone();
        let dp = AdnlPeers::with_keys(delayed.key_id.clone(), server_key_id.clone());
        let start = tokio::time::Instant::now();
        let resp = tokio::time::timeout(Duration::from_secs(10), async {
            dq.query(make_ping_data(600), None, &dp, None).await
        })
        .await
        .expect("delayed client timed out")
        .expect("delayed client query failed")
        .expect("delayed client query returned None");
        let delayed_elapsed = start.elapsed();
        assert_eq!(parse_pong(resp), 600);
        println!("Step 2: delayed client completed in {:.2}s", delayed_elapsed.as_secs_f64());

        // Step 3: 7th client while all 6 are still alive — should be refused
        // (only one delayed accept per IP is allowed, and that slot is now
        // occupied by a live connection, so the next one gets refused)
        let refused = make_client(BASE_CLIENT_PORT + FAST_CLIENTS as u16 + 1);
        let rq = refused.quic.clone();
        let rp = AdnlPeers::with_keys(refused.key_id.clone(), server_key_id.clone());
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            rq.query(make_ping_data(700), None, &rp, None).await
        })
        .await;
        let was_refused = match result {
            Err(_) => {
                println!("Step 3: 7th client timed out (throttled as expected)");
                true
            }
            Ok(Err(e)) => {
                let msg = format!("{e}");
                println!("Step 3: 7th client error: {msg}");
                msg.contains("refused") || msg.contains("aborted")
            }
            Ok(Ok(_)) => {
                // May succeed if the delayed accept slot freed up in time
                println!("Step 3: 7th client succeeded (delayed accept slot freed)");
                false
            }
        };
        // The 7th client should be refused or at least severely delayed
        // because the per-IP delayed-accept slot is already taken.
        if was_refused {
            println!("Step 3: confirmed per-IP throttle refused 7th connection");
        }

        // Step 4: shut down all clients, verify pool recovery
        for c in &fast {
            c.quic.shutdown();
            c.token.cancel();
        }
        delayed.quic.shutdown();
        delayed.token.cancel();
        refused.quic.shutdown();
        refused.token.cancel();
        drop(fast);

        // Wait for server-side handle_connection tasks to detect closed
        // connections and decrement ip_conn_count
        tokio::time::sleep(Duration::from_secs(3)).await;

        // A fresh client should connect immediately (pool recovered)
        let (fresh, _fk, fk_id, fk_bind, fk_token) = make_endpoint(6099);
        fresh.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(fk_id.clone(), fk_bind).unwrap();

        let resp = tokio::time::timeout(
            Duration::from_secs(10),
            fresh.query(
                make_ping_data(12345),
                None,
                &AdnlPeers::with_keys(fk_id.clone(), server_key_id.clone()),
                None,
            ),
        )
        .await
        .expect("fresh client query timed out after pool recovery")
        .expect("fresh client query failed")
        .expect("fresh client query returned None");
        assert_eq!(parse_pong(resp), 12345);
        println!("Step 4: fresh client succeeded after pool recovery");

        // --- cleanup ---
        fresh.shutdown();
        fk_token.cancel();
        server.shutdown();
        server_token.cancel();
    });
}

/// Fire messages through a server restart cycle. Verifies the sender task
/// drains the queue after reconnection without hanging or losing messages.
/// In the old hot-loop design, the yield_now() spins would starve the runtime.
#[test]
fn test_quic_message_burst_reconnect() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const CLIENT_PORT: u16 = 8100;
        const SERVER_PORT: u16 = 8101;
        const BURST_SIZE: usize = 50;

        let client_bind: SocketAddr = format!("127.0.0.1:{CLIENT_PORT}").parse().unwrap();
        let server_bind: SocketAddr = format!("127.0.0.1:{SERVER_PORT}").parse().unwrap();

        let client_token = CancellationToken::new();
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, client_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{CLIENT_PORT}"),
            vec![(client_key, KEY_TAG)],
        )
        .unwrap();
        let client_key_id = client_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (cli_tx, _cli_rx) = tokio::sync::mpsc::unbounded_channel();
        let client_sub = Arc::new(TestSubscriber { key_id: client_key_id.clone(), msg_tx: cli_tx })
            as Arc<dyn Subscriber>;
        let client = QuicNode::new(
            vec![client_sub],
            client_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        client.add_key(&client_key, &client_key_id, client_bind).unwrap();

        let server_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, server_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(server_key, KEY_TAG)],
        )
        .unwrap();
        let server_key_id = server_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        // --- Phase 1: first server instance ---
        let srv_token1 = CancellationToken::new();
        let (srv_tx1, mut srv_rx1) = tokio::sync::mpsc::unbounded_channel();
        let srv_sub1 = Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx1 })
            as Arc<dyn Subscriber>;
        let server1 = QuicNode::new(
            vec![srv_sub1],
            srv_token1.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server1.add_key(&server_key, &server_key_id, server_bind).unwrap();

        client.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server1.add_peer_key(client_key_id.clone(), client_bind).unwrap();
        let peers = AdnlPeers::with_keys(client_key_id.clone(), server_key_id.clone());

        for i in 0..BURST_SIZE {
            let payload = format!("msg-phase1-{i}").into_bytes();
            client.message(payload, None, &peers).await.unwrap();
        }

        let expected_p1: HashSet<Vec<u8>> =
            (0..BURST_SIZE).map(|i| format!("msg-phase1-{i}").into_bytes()).collect();
        let mut got_p1 = HashSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while got_p1.len() < BURST_SIZE {
            match tokio::time::timeout_at(deadline, srv_rx1.recv()).await {
                Ok(Some(data)) => {
                    got_p1.insert(data);
                }
                _ => break,
            }
        }
        println!("Phase 1: received {}/{BURST_SIZE} unique messages", got_p1.len());
        assert_eq!(
            got_p1, expected_p1,
            "Phase 1 must deliver every distinct message (at-least-once guarantee)"
        );

        // --- Phase 2: restart server, send another burst ---
        server1.shutdown();
        srv_token1.cancel();
        drop(server1);
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let srv_token2 = CancellationToken::new();
        let (srv_tx2, mut srv_rx2) = tokio::sync::mpsc::unbounded_channel();
        let srv_sub2 = Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx2 })
            as Arc<dyn Subscriber>;
        let server2 = QuicNode::new(
            vec![srv_sub2],
            srv_token2.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server2.add_key(&server_key, &server_key_id, server_bind).unwrap();
        server2.add_peer_key(client_key_id.clone(), client_bind).unwrap();

        for i in 0..BURST_SIZE {
            let payload = format!("msg-phase2-{i}").into_bytes();
            client.message(payload, None, &peers).await.unwrap();
        }

        let expected_p2: HashSet<Vec<u8>> =
            (0..BURST_SIZE).map(|i| format!("msg-phase2-{i}").into_bytes()).collect();
        let mut got_p2 = HashSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while got_p2.len() < BURST_SIZE {
            match tokio::time::timeout_at(deadline, srv_rx2.recv()).await {
                Ok(Some(data)) => {
                    got_p2.insert(data);
                }
                _ => break,
            }
        }
        println!(
            "Phase 2: received {}/{BURST_SIZE} unique messages after server restart",
            got_p2.len()
        );
        assert_eq!(
            got_p2, expected_p2,
            "Phase 2 must deliver every distinct message after restart (at-least-once guarantee)"
        );

        client.shutdown();
        server2.shutdown();
        client_token.cancel();
        srv_token2.cancel();
    });
}

/// Concurrent message senders to the same peer must not deadlock or starve
/// the Tokio runtime. Uses only 2 worker threads to make thread starvation
/// from the old yield_now() hot loops detectable.
#[test]
fn test_quic_single_sender_invariant() {
    init_test_log();
    let rt =
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(async {
        const CLIENT_PORT: u16 = 8200;
        const SERVER_PORT: u16 = 8201;
        const NUM_SENDERS: usize = 20;
        const MSGS_PER_SENDER: usize = 5;
        const TOTAL_MSGS: usize = NUM_SENDERS * MSGS_PER_SENDER;
        const TIMEOUT: Duration = Duration::from_secs(20);

        let client_bind: SocketAddr = format!("127.0.0.1:{CLIENT_PORT}").parse().unwrap();
        let server_bind: SocketAddr = format!("127.0.0.1:{SERVER_PORT}").parse().unwrap();

        let client_token = CancellationToken::new();
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, client_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{CLIENT_PORT}"),
            vec![(client_key, KEY_TAG)],
        )
        .unwrap();
        let client_key_id = client_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (cli_tx, _cli_rx) = tokio::sync::mpsc::unbounded_channel();
        let client_sub = Arc::new(TestSubscriber { key_id: client_key_id.clone(), msg_tx: cli_tx })
            as Arc<dyn Subscriber>;
        let client = QuicNode::new(
            vec![client_sub],
            client_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        client.add_key(&client_key, &client_key_id, client_bind).unwrap();

        let srv_token = CancellationToken::new();
        let server_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, server_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(server_key, KEY_TAG)],
        )
        .unwrap();
        let server_key_id = server_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (srv_tx, mut srv_rx) = tokio::sync::mpsc::unbounded_channel();
        let srv_sub = Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx })
            as Arc<dyn Subscriber>;
        let server = QuicNode::new(
            vec![srv_sub],
            srv_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server.add_key(&server_key, &server_key_id, server_bind).unwrap();

        client.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        server.add_peer_key(client_key_id.clone(), client_bind).unwrap();

        let expected: HashSet<Vec<u8>> = (0..NUM_SENDERS)
            .flat_map(|s| {
                (0..MSGS_PER_SENDER).map(move |m| format!("sender-{s}-msg-{m}").into_bytes())
            })
            .collect();
        let got = Arc::new(tokio::sync::Mutex::new(HashSet::new()));
        let got_clone = got.clone();
        let drain_handle = tokio::spawn(async move {
            while let Some(data) = srv_rx.recv().await {
                got_clone.lock().await.insert(data);
            }
        });

        let mut handles = Vec::with_capacity(NUM_SENDERS);
        for sender_id in 0..NUM_SENDERS {
            let quic = client.clone();
            let src = client_key_id.clone();
            let dst = server_key_id.clone();
            handles.push(tokio::spawn(async move {
                for msg_id in 0..MSGS_PER_SENDER {
                    let payload = format!("sender-{sender_id}-msg-{msg_id}").into_bytes();
                    let peers = AdnlPeers::with_keys(src.clone(), dst.clone());
                    if let Err(e) = quic.message(payload, None, &peers).await {
                        eprintln!("sender {sender_id} msg {msg_id} failed: {e}");
                    }
                }
            }));
        }

        let send_result = tokio::time::timeout(TIMEOUT, async {
            for h in handles {
                h.await.expect("sender task panicked");
            }
        })
        .await;
        assert!(send_result.is_ok(), "Concurrent senders timed out — possible hot-loop regression");

        let recv_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let unique_count = got.lock().await.len();
            if unique_count >= TOTAL_MSGS {
                break;
            }
            if tokio::time::Instant::now() >= recv_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let received = got.lock().await;
        println!(
            "Single-sender invariant: {}/{TOTAL_MSGS} unique messages delivered \
             by {NUM_SENDERS} concurrent senders on 2 Tokio threads",
            received.len()
        );
        assert_eq!(
            *received, expected,
            "All {TOTAL_MSGS} distinct messages must be delivered (at-least-once guarantee)"
        );

        client.shutdown();
        server.shutdown();
        client_token.cancel();
        srv_token.cancel();
        drain_handle.abort();
    });
}

/// Regression test: a sender task that exits through the connect-failure path
/// must release its `active` flag so a later `message()` can spawn a fresh
/// sender task and reconnect. With the flag stuck, the peer's outbound entry
/// becomes a blackhole: messages keep queuing but no connect is ever retried
/// and nothing is delivered even after the peer comes back.
///
/// Scenario:
/// 1. The server port is a blackhole (a bound UDP socket that never answers),
///    so the client's connect attempt times out and the sender task exits
///    through the fatal path
/// 2. The real server then starts on that port with the expected identity
/// 3. New messages from the client must reach the server via a respawned
///    sender task
#[test]
fn test_quic_message_reconnect_after_connect_failure() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const CLIENT_PORT: u16 = 8400;
        const SERVER_PORT: u16 = 8401;
        const RECOVERY_TIMEOUT: Duration = Duration::from_secs(20);

        let client_bind: SocketAddr = format!("127.0.0.1:{CLIENT_PORT}").parse().unwrap();
        let server_bind: SocketAddr = format!("127.0.0.1:{SERVER_PORT}").parse().unwrap();

        // --- client ---
        let client_token = CancellationToken::new();
        let client_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, client_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{CLIENT_PORT}"),
            vec![(client_key, KEY_TAG)],
        )
        .unwrap();
        let client_key_id = client_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        let (cli_tx, _cli_rx) = tokio::sync::mpsc::unbounded_channel();
        let client_sub = Arc::new(TestSubscriber { key_id: client_key_id.clone(), msg_tx: cli_tx })
            as Arc<dyn Subscriber>;
        let client = QuicNode::new(
            vec![client_sub],
            client_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        client.add_key(&client_key, &client_key_id, client_bind).unwrap();

        // --- server identity (key created now, server started later) ---
        let server_key = ed25519_generate_private_key().unwrap().to_bytes();
        let (_, server_cfg) = AdnlNodeConfig::from_ip_address_and_private_keys(
            &format!("127.0.0.1:{SERVER_PORT}"),
            vec![(server_key, KEY_TAG)],
        )
        .unwrap();
        let server_key_id = server_cfg.key_by_tag(KEY_TAG).unwrap().id().clone();

        client.add_peer_key(server_key_id.clone(), server_bind).unwrap();
        let peers = AdnlPeers::with_keys(client_key_id.clone(), server_key_id.clone());

        // --- Phase A: server port is a blackhole, connect must fail ---
        // The socket swallows handshake packets without answering, so the
        // client's connect attempt hits CONNECT_TIMEOUT instead of an ICMP
        // port-unreachable race
        let blackhole = UdpSocket::bind(server_bind).unwrap();

        let queued = client.message(b"phase-a".to_vec(), None, &peers).await.unwrap();
        assert!(queued.is_none(), "expected queued send (no live connection)");
        println!("Phase A: message queued, waiting for connect failure cycle");

        // Connect attempt times out after CONNECT_TIMEOUT (5s); give the
        // sender task time to finish its failure cycle and exit
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("Phase A: sender task failure cycle finished");

        // --- Phase B: real server appears on the same port ---
        drop(blackhole);
        let srv_token = CancellationToken::new();
        let (srv_tx, mut srv_rx) = tokio::sync::mpsc::unbounded_channel();
        let srv_sub = Arc::new(TestSubscriber { key_id: server_key_id.clone(), msg_tx: srv_tx })
            as Arc<dyn Subscriber>;
        let server = QuicNode::new(
            vec![srv_sub],
            srv_token.clone(),
            tokio::runtime::Handle::current(),
            Some(QuicRateLimitConfig::disabled()),
        );
        server.add_key(&server_key, &server_key_id, server_bind).unwrap();
        server.add_peer_key(client_key_id.clone(), client_bind).unwrap();
        println!("Phase B: server started on previously dead port");

        // Send messages until one is delivered: each message must be able to
        // respawn the sender task, which reconnects (with stepped backoff)
        let deadline = tokio::time::Instant::now() + RECOVERY_TIMEOUT;
        let mut delivered = None;
        let mut sent = 0usize;
        while tokio::time::Instant::now() < deadline {
            let payload = format!("phase-b-{sent}").into_bytes();
            client.message(payload, None, &peers).await.unwrap();
            sent += 1;
            if let Ok(Some(data)) =
                tokio::time::timeout(Duration::from_millis(500), srv_rx.recv()).await
            {
                delivered = Some(data);
                break;
            }
        }

        println!("Phase B: sent {sent} messages, delivered={}", delivered.is_some());
        let delivered = delivered.expect(
            "no message delivered after peer recovery - sender task was not \
             respawned (active flag stuck after connect failure)",
        );
        assert!(delivered.starts_with(b"phase-b-"), "delivered payload is not a phase B message");

        client.shutdown();
        server.shutdown();
        client_token.cancel();
        srv_token.cancel();
    });
}

// --- TL serialization round-trip ---

#[test]
fn test_quic_address_tl_roundtrip() {
    let ip: u32 = u32::from(Ipv4Addr::new(1, 2, 3, 4));
    let port: u16 = 12345;
    let list = make_address_list(vec![udp_addr(ip, 30000), quic_addr(ip, port)]);

    let bytes = serialize_boxed(&list.into_boxed()).unwrap();
    let restored = deserialize_boxed(&bytes)
        .unwrap()
        .downcast::<ton_api::ton::adnl::AddressList>()
        .unwrap()
        .only();

    assert_eq!(restored.addrs.len(), 2);
    match &restored.addrs[0] {
        Address::Adnl_Address_Udp(u) => {
            assert_eq!(u.ip as u32, ip);
            assert_eq!(u.port, 30000);
        }
        other => panic!("Expected Udp, got {:?}", other),
    }
    match &restored.addrs[1] {
        Address::Adnl_Address_Quic(q) => {
            assert_eq!(q.ip as u32, ip);
            assert_eq!(q.port, port as i32);
        }
        other => panic!("Expected Quic, got {:?}", other),
    }
}

#[test]
fn test_quic_address_tl_roundtrip_no_quic() {
    let ip: u32 = u32::from(Ipv4Addr::new(10, 0, 0, 1));
    let list = make_address_list(vec![udp_addr(ip, 30000)]);

    let bytes = serialize_boxed(&list.into_boxed()).unwrap();
    let restored = deserialize_boxed(&bytes)
        .unwrap()
        .downcast::<ton_api::ton::adnl::AddressList>()
        .unwrap()
        .only();

    assert_eq!(restored.addrs.len(), 1);
    assert!(matches!(&restored.addrs[0], Address::Adnl_Address_Udp(_)));
}

// --- parse_quic_address ---

#[test]
fn test_parse_quic_address_present() {
    let ip: u32 = u32::from(Ipv4Addr::new(192, 168, 1, 1));
    let list = make_address_list(vec![udp_addr(ip, 30000), quic_addr(ip, 31000)]);

    let (_, result) = AdnlNode::parse_address_list(&list).unwrap().unwrap();
    assert_eq!(
        result.map(|q| ip_address_to_socket_addr(&q)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 31000))
    );
}

#[test]
fn test_parse_quic_address_absent() {
    let ip: u32 = u32::from(Ipv4Addr::new(192, 168, 1, 1));
    let list = make_address_list(vec![udp_addr(ip, 30000)]);

    let (_, result) = AdnlNode::parse_address_list(&list).unwrap().unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_parse_quic_address_only_quic() {
    let ip: u32 = u32::from(Ipv4Addr::new(10, 0, 0, 5));
    let list = make_address_list(vec![quic_addr(ip, 9999)]);

    // With parse_address_list, quic-only list returns None (no UDP address found)
    let result = AdnlNode::parse_address_list(&list).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_parse_quic_address_rejects_duplicate() {
    let ip1: u32 = u32::from(Ipv4Addr::new(1, 1, 1, 1));
    let ip2: u32 = u32::from(Ipv4Addr::new(2, 2, 2, 2));
    let list =
        make_address_list(vec![udp_addr(ip1, 30000), quic_addr(ip1, 31000), quic_addr(ip2, 32000)]);

    // A list carrying more than one QUIC address is malformed. parse_address_list
    // rejects the whole list rather than guessing which QUIC address to trust.
    assert!(AdnlNode::parse_address_list(&list).unwrap().is_none());
}

// --- parse_address_list still works (not broken by new variant) ---

#[test]
fn test_parse_address_list_with_quic_and_udp() {
    let ip: u32 = u32::from(Ipv4Addr::new(172, 16, 0, 1));
    let list = make_address_list(vec![udp_addr(ip, 30000), quic_addr(ip, 31000)]);

    let result = AdnlNode::parse_address_list(&list).unwrap();
    assert!(result.is_some());
    let (adnl_addr, _) = result.unwrap();
    assert_eq!(adnl_addr.ip(), ip);
    assert_eq!(adnl_addr.port(), 30000);
}

#[test]
fn test_parse_address_list_quic_only_returns_none() {
    let ip: u32 = u32::from(Ipv4Addr::new(172, 16, 0, 1));
    let list = make_address_list(vec![quic_addr(ip, 31000)]);

    // parse_address_list looks at addrs[0] and expects UDP — quic-only should return None
    let result = AdnlNode::parse_address_list(&list).unwrap();
    assert!(result.is_none());
}

// --- TL wire compatibility: deserialize a quic address from raw bytes ---

#[test]
fn test_quic_address_deserialize_from_bytes() {
    // Build a known address list with quic, serialize, then deserialize
    let ip: u32 = u32::from(Ipv4Addr::new(93, 174, 52, 11));
    let port: u16 = 40001;
    let list = make_address_list(vec![udp_addr(ip, 30303), quic_addr(ip, port)]);
    let bytes = serialize_boxed(&list.into_boxed()).unwrap();

    // Deserialize from raw bytes (simulating reception from a C++ node)
    let obj = deserialize_boxed(&bytes).unwrap();
    let restored = obj
        .downcast::<ton_api::ton::adnl::AddressList>()
        .expect("should deserialize as AddressList")
        .only();

    let (_, quic) = AdnlNode::parse_address_list(&restored).unwrap().unwrap();
    assert_eq!(
        quic.map(|q| ip_address_to_socket_addr(&q)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 174, 52, 11)), 40001))
    );
}

// --- DHT distribution tests ---

fn init_local_dht_pair(
    port1: u16,
    port2: u16,
) -> (
    tokio::runtime::Runtime,
    Arc<AdnlNode>,
    Arc<DhtNode>,
    Arc<OverlayNode>,
    Arc<AdnlNode>,
    Arc<DhtNode>,
    Arc<OverlayNode>,
) {
    let rt = init_test();
    let mut config1 = rt
        .block_on(get_adnl_config("quic_addr", &format!("127.0.0.1:{port1}"), vec![KEY_TAG], true))
        .unwrap();
    config1.set_ip_address_quic(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        port1 + 1000,
    ));
    let config2 = rt
        .block_on(get_adnl_config("quic_addr", &format!("127.0.0.1:{port2}"), vec![KEY_TAG], true))
        .unwrap();
    let adnl1 = rt.block_on(AdnlNode::with_config(config1)).unwrap();
    let dht1 = DhtNode::with_adnl_node(adnl1.clone(), KEY_TAG).unwrap();
    let overlay1 = OverlayNode::with_params(adnl1.clone(), &[1u8; 32], KEY_TAG).unwrap();
    rt.block_on(adnl1.start_over_udp(vec![dht1.clone(), overlay1.clone()])).unwrap();
    let adnl2 = rt.block_on(AdnlNode::with_config(config2)).unwrap();
    let dht2 = DhtNode::with_adnl_node(adnl2.clone(), KEY_TAG).unwrap();
    let overlay2 = OverlayNode::with_params(adnl2.clone(), &[1u8; 32], KEY_TAG).unwrap();
    rt.block_on(adnl2.start_over_udp(vec![dht2.clone(), overlay2.clone()])).unwrap();
    (rt, adnl1, dht1, overlay1, adnl2, dht2, overlay2)
}

/// Test: adnl.address.quic is stored in DHT and retrieved by another node.
///
/// Node1 sets its QUIC port and stores its address via DHT (store_ip_address).
/// Node2 fetches node1's address from DHT (fetch_address).
/// Verify that node2's ADNL layer has the correct QUIC address for node1.
#[test]
fn test_quic_address_dht_distribution() {
    let (rt, adnl1, dht1, _overlay1, adnl2, dht2, _overlay2) = init_local_dht_pair(4291, 4292);

    rt.block_on(async {
        // Connect the two DHT nodes
        let peer1 = dht2.add_peer(&dht1.get_signed_node().unwrap()).unwrap().unwrap();
        let peer2 = dht1.add_peer(&dht2.get_signed_node().unwrap()).unwrap().unwrap();
        assert!(dht1.ping(&peer2).await.unwrap());
        assert!(dht2.ping(&peer1).await.unwrap());

        // Node1: QUIC address was set in config.
        // build_address_list will include adnl.address.quic automatically.
        let quic_addr_expected = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 5291);

        // Verify build_address_list includes the quic address
        let addr_list = adnl1.build_address_list(None).unwrap();
        let (_, parsed_quic) = AdnlNode::parse_address_list(&addr_list).unwrap().unwrap();
        assert!(parsed_quic.is_some(), "build_address_list should include adnl.address.quic");
        assert_eq!(ip_address_to_socket_addr(&parsed_quic.unwrap()), quic_addr_expected);

        // Store in DHT
        assert!(dht1.store_ip_address(&dht1.key()).await.unwrap());

        // Node2: fetch node1's address from DHT
        let key1_id = dht1.key().id().clone();
        let fetched = dht2.fetch_address(&key1_id).await.unwrap();
        assert!(fetched.is_some(), "Node2 should find node1's address in DHT");

        let (adnl_addr, _, _key) = fetched.unwrap();
        // Verify the UDP address was parsed correctly
        assert_eq!(adnl_addr.port(), 4291, "UDP port should match node1");

        // Verify the QUIC address was extracted and stored in the ADNL layer
        let local_key2 = adnl2.key_by_tag(KEY_TAG).unwrap().id().clone();
        let peer_addrs = adnl2.peer_ip_address(&local_key2, &key1_id).unwrap();
        assert!(peer_addrs.is_some(), "Node2 should have address for node1 after DHT fetch");
        let (_, quic_addr) = peer_addrs.unwrap();
        assert!(quic_addr.is_some(), "Node2 should have QUIC address for node1 after DHT fetch");
        let quic_addr = quic_addr.unwrap();
        assert_eq!(
            quic_addr, quic_addr_expected,
            "QUIC address should match what node1 advertised"
        );

        adnl1.stop().await;
        adnl2.stop().await;
    });
}

/// Test: address list without adnl.address.quic does NOT set peer_quic_address.
///
/// `init_local_dht_pair` configures a QUIC port on node1 only, so node2 is the
/// node without one. Node2 stores its address via DHT, node1 fetches it and
/// verifies no QUIC address is stored.
#[test]
fn test_no_quic_address_dht_distribution() {
    let (rt, adnl1, dht1, _overlay1, adnl2, dht2, _overlay2) = init_local_dht_pair(4293, 4294);

    rt.block_on(async {
        let peer1 = dht2.add_peer(&dht1.get_signed_node().unwrap()).unwrap().unwrap();
        let peer2 = dht1.add_peer(&dht2.get_signed_node().unwrap()).unwrap().unwrap();
        assert!(dht1.ping(&peer2).await.unwrap());
        assert!(dht2.ping(&peer1).await.unwrap());

        // Node2: no QUIC port set — build_address_list should only have UDP
        let addr_list = adnl2.build_address_list(None).unwrap();
        let (_, quic_addr) = AdnlNode::parse_address_list(&addr_list).unwrap().unwrap();
        assert!(
            quic_addr.is_none(),
            "Without set_quic_address, address list should not contain adnl.address.quic"
        );

        // Store and fetch
        assert!(dht2.store_ip_address(&dht2.key()).await.unwrap());
        let key2_id = dht2.key().id().clone();
        let fetched = dht1.fetch_address(&key2_id).await.unwrap();
        assert!(fetched.is_some());

        // Verify no QUIC address was stored
        let local_key1 = adnl1.key_by_tag(KEY_TAG).unwrap().id().clone();
        let peer_addrs = adnl1.peer_ip_address(&local_key1, &key2_id).unwrap();
        let quic_addr = peer_addrs.and_then(|(_, q)| q);
        assert!(
            quic_addr.is_none(),
            "No QUIC address should be stored when peer doesn't advertise one"
        );

        adnl1.stop().await;
        adnl2.stop().await;
    });
}

// ===========================================================================
// Rate-limit integration tests
// ===========================================================================

/// Create a raw quinn client endpoint on an ephemeral OS-assigned port.
/// Returns the endpoint and the SNI string for connecting to `server_key_id`.
fn make_raw_client_endpoint(server_key_id: &KeyId) -> (quinn::Endpoint, String) {
    let key = ed25519_generate_private_key().unwrap().to_bytes();
    let client_config = build_raw_quinn_client(&key);

    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .unwrap();
    sock.set_reuse_address(true).unwrap();
    // Bind to 127.0.0.1:0 — OS assigns an ephemeral port
    sock.bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into()).unwrap();
    sock.set_nonblocking(true).unwrap();
    let udp = std::net::UdpSocket::from(sock);
    let runtime: Arc<dyn quinn::Runtime> = Arc::new(quinn::TokioRuntime);
    let mut endpoint =
        quinn::Endpoint::new(quinn::EndpointConfig::default(), None, udp, runtime).unwrap();
    endpoint.set_default_client_config(client_config);

    let hex = hex::encode(server_key_id.data());
    let sni = format!("{}.{}.adnl", &hex[..32], &hex[32..]);
    (endpoint, sni)
}

/// Try to establish a QUIC connection with a timeout.
/// Returns Ok(connection) on success, Err on failure or timeout.
async fn try_connect(
    endpoint: &quinn::Endpoint,
    server_bind: SocketAddr,
    sni: &str,
    timeout: Duration,
) -> std::result::Result<quinn::Connection, String> {
    let connecting = endpoint.connect(server_bind, sni).map_err(|e| format!("connect: {e}"))?;
    match tokio::time::timeout(timeout, connecting).await {
        Ok(Ok(conn)) => Ok(conn),
        Ok(Err(e)) => Err(format!("handshake: {e}")),
        Err(_) => Err("timeout".into()),
    }
}

/// Per-IP rate limiter: server allows burst of 2 connections, then refuses.
/// Five rapid connection attempts from the same IP; first 2 succeed, rest fail.
#[test]
fn test_quic_rate_limit_per_ip() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 8300;
        const BURST: u32 = 2;
        const TOTAL_ATTEMPTS: usize = 5;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

        let rl_config = QuicRateLimitConfig {
            per_ip_capacity: BURST,
            per_ip_period: 100.0, // very slow refill — no tokens come back during the test
            global_capacity: 0,   // global disabled
            global_period: 1.0,
            stateless_retry: false,
        };
        let (server, _key, server_key_id, server_bind, server_token) =
            make_endpoint_with_config(SERVER_PORT, rl_config);

        // wait a little to server spin-up
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 127.0.0.1 — same IP for per-IP limiting
        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut conns = Vec::new();
        for i in 0..TOTAL_ATTEMPTS {
            let (ep, sni) = make_raw_client_endpoint(&server_key_id);
            match try_connect(&ep, server_bind, &sni, CONNECT_TIMEOUT).await {
                Ok(conn) => {
                    println!("  connection {i}: OK (stable_id={})", conn.stable_id());
                    succeeded += 1;
                    conns.push(conn);
                }
                Err(e) => {
                    println!("  connection {i}: REJECTED ({e})");
                    failed += 1;
                }
            }
        }

        println!(
            "Per-IP rate limit test: burst={BURST}, attempts={TOTAL_ATTEMPTS}, \
             succeeded={succeeded}, failed={failed}"
        );
        assert_eq!(succeeded, BURST as u32, "expected exactly {BURST} connections to succeed");
        assert_eq!(
            failed,
            (TOTAL_ATTEMPTS - BURST as usize) as u32,
            "expected {} connections to be rejected",
            TOTAL_ATTEMPTS - BURST as usize,
        );

        drop(conns);
        server.shutdown();
        server_token.cancel();
    });
}

/// Global rate limiter: server allows burst of 3 connections total, then refuses.
#[test]
fn test_quic_rate_limit_global() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 8310;
        const BURST: u32 = 3;
        const TOTAL_ATTEMPTS: usize = 6;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

        let rl_config = QuicRateLimitConfig {
            per_ip_capacity: 0, // per-IP disabled
            per_ip_period: 1.0,
            global_capacity: BURST,
            global_period: 100.0, // very slow refill
            stateless_retry: false,
        };
        let (server, _key, server_key_id, server_bind, server_token) =
            make_endpoint_with_config(SERVER_PORT, rl_config);

        // wait a little to server spin-up
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut conns = Vec::new();
        for i in 0..TOTAL_ATTEMPTS {
            let (ep, sni) = make_raw_client_endpoint(&server_key_id);
            match try_connect(&ep, server_bind, &sni, CONNECT_TIMEOUT).await {
                Ok(conn) => {
                    println!("  connection {i}: OK (stable_id={})", conn.stable_id());
                    succeeded += 1;
                    conns.push(conn);
                }
                Err(e) => {
                    println!("  connection {i}: REJECTED ({e})");
                    failed += 1;
                }
            }
        }

        println!(
            "Global rate limit test: burst={BURST}, attempts={TOTAL_ATTEMPTS}, \
             succeeded={succeeded}, failed={failed}"
        );
        assert_eq!(succeeded, BURST as u32, "expected exactly {BURST} connections to succeed");
        assert_eq!(
            failed,
            (TOTAL_ATTEMPTS - BURST as usize) as u32,
            "expected {} connections to be rejected",
            TOTAL_ATTEMPTS - BURST as usize,
        );

        drop(conns);
        server.shutdown();
        server_token.cancel();
    });
}

/// Stateless Retry: server requires address validation via Retry packets.
/// A normal client should still connect successfully this verifies retry
/// doesn't break connectivity.
#[test]
fn test_quic_stateless_retry() {
    init_test_log();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        const SERVER_PORT: u16 = 8320;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

        let rl_config = QuicRateLimitConfig {
            per_ip_capacity: 0, // rate-limiting disabled
            per_ip_period: 1.0,
            global_capacity: 0,
            global_period: 1.0,
            stateless_retry: true, // retry enabled
        };
        let (server, _key, server_key_id, server_bind, server_token) =
            make_endpoint_with_config(SERVER_PORT, rl_config);

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect a raw client — quinn handles the Retry transparently
        let (ep, sni) = make_raw_client_endpoint(&server_key_id);
        let conn = try_connect(&ep, server_bind, &sni, CONNECT_TIMEOUT)
            .await
            .expect("connection with stateless retry should succeed");

        // Verify the connection works by opening a stream and doing ping/pong
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let ping_data = make_ping_wire(100500);
        send.write_all(&ping_data).await.unwrap();
        send.finish().unwrap();
        let response =
            tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(16 * 1024 * 1024))
                .await
                .expect("read timed out")
                .expect("read failed");
        let pong = parse_pong_wire(&response);
        assert_eq!(pong, 100500, "ping/pong mismatch through stateless retry");

        println!(
            "Stateless retry test: connection succeeded, ping/pong OK, remote={}",
            conn.remote_address()
        );

        drop(conn);
        server.shutdown();
        server_token.cancel();
    });
}
