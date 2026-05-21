/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Test helpers for cross-implementation compatibility tests.
//!
//! Provides utilities for creating Rust ADNL+overlay nodes and
//! exchanging peers with cpp test nodes.

use crate::CppTestNode;
use adnl::{
    common::{
        hash, AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice,
        TaggedTlObject,
    },
    node::{AdnlNode, AdnlNodeConfig, AdnlSendMethod, IpAddress},
    OverlayNode, OverlayNodeInfo, OverlayParams, OverlayShortId, QuicNode, QuicRateLimitConfig,
    RldpNode,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use ton_api::{
    deserialize_boxed, serialize_boxed, serialize_boxed_append,
    ton::{
        adnl::{
            address::address::{Quic as AdnlAddrQuic, Udp as AdnlAddrUdp},
            addresslist::AddressList,
            Address as AdnlAddress,
        },
        overlay::{
            message::Message as OverlayMessage, node::Node as OverlayNodeV1,
            nodev2::NodeV2 as OverlayNodeV2, Node as OverlayNodeBoxed,
        },
        pub_::publickey::{Ed25519 as Ed25519PubKey, Overlay as OverlayKey},
        rpc::overlay::Query as OverlayQuery,
        ton_node::data::Data as TonNodeData,
    },
    IntoBoxed, TLObject,
};
use ton_block::{
    base64_decode, base64_encode, sha256_digest, Ed25519KeyOption, KeyId, KeyOption, Result,
    UInt256, ZeroizingBytes,
};

const KEY_TAG_OVERLAY: usize = 2;

/// A Rust ADNL + overlay test node
pub struct RustTestNode {
    pub rt: tokio::runtime::Runtime,
    pub adnl: Arc<AdnlNode>,
    pub overlay: Arc<OverlayNode>,
    pub addr: String,
    pub port: u16,
}

impl RustTestNode {
    /// Create a new Rust ADNL+overlay node on the given IP:port.
    /// If `with_rldp` is true, an RLDP node is created and registered,
    /// enabling TwostepFec broadcasts.
    pub fn new(ip: &str, port: u16, with_rldp: bool) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("Failed to create tokio runtime");

        let addr = format!("{}:{}", ip, port);
        let zero_state = [0u8; 32]; // Test zero state

        // Generate deterministic key from address
        let key_data = sha256_digest(addr.as_bytes());
        let keys = vec![(key_data, KEY_TAG_OVERLAY)];
        let (_, config) =
            AdnlNodeConfig::from_ip_address_and_private_keys(&addr, keys).expect("Config failed");

        let adnl = rt.block_on(AdnlNode::with_config(config)).expect("ADNL node creation failed");

        let overlay = OverlayNode::with_params(adnl.clone(), &zero_state, KEY_TAG_OVERLAY)
            .expect("Overlay node creation failed");

        if with_rldp {
            let rldp = RldpNode::with_params(adnl.clone(), vec![overlay.clone()], None)
                .expect("RLDP node creation failed");
            overlay.set_rldp(rldp.clone()).expect("set_rldp failed");

            let subscribers: Vec<Arc<dyn Subscriber>> = vec![overlay.clone(), rldp];

            rt.block_on(async {
                adnl.start_over_udp(subscribers).await.expect("Failed to start ADNL UDP");
            });
        } else {
            let subscribers: Vec<Arc<dyn Subscriber>> = vec![overlay.clone()];

            rt.block_on(async {
                adnl.start_over_udp(subscribers).await.expect("Failed to start ADNL UDP");
            });
        }

        Self { rt, adnl, overlay, addr, port }
    }

    /// Get the ADNL key ID (hex)
    pub fn adnl_id_hex(&self) -> String {
        self.adnl
            .key_by_tag(KEY_TAG_OVERLAY)
            .expect("No key")
            .id()
            .data()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    /// Get the ADNL public key as base64 TL-serialized
    pub fn pubkey_tl_b64(&self) -> String {
        let key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        let pub_key = key.pub_key().expect("No pub key");
        // Export as TL-serialized pub.ed25519{key:int256}
        let tl_key =
            Ed25519PubKey { key: UInt256::with_array(pub_key.try_into().expect("Wrong key size")) };
        let serialized = serialize_boxed(&tl_key.into_boxed()).expect("Serialization failed");
        base64_encode(&serialized)
    }

    /// Get the ADNL key Arc<KeyId>
    pub fn adnl_key_id(&self) -> Arc<KeyId> {
        self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key").id().clone()
    }

    /// Compute the overlay name TL bytes for a given workchain/shard
    /// This is the bytes that should be passed to C++ as overlay_name
    pub fn compute_overlay_name(&self, workchain: i32, shard: i64) -> Vec<u8> {
        let overlay_id = self.overlay.calc_overlay_id(workchain, shard).expect("calc_overlay_id");
        overlay_id.to_vec()
    }

    /// Compute overlay short ID
    pub fn compute_overlay_short_id(&self, workchain: i32, shard: i64) -> Arc<OverlayShortId> {
        self.overlay.calc_overlay_short_id(workchain, shard).expect("calc_overlay_short_id")
    }

    /// Compute overlay short ID from arbitrary name bytes.
    /// This wraps the name in a TL pub.overlay{name} structure before hashing,
    /// matching the C++ OverlayIdFull::compute_short_id() behavior.
    pub fn compute_overlay_short_id_from_name(&self, name: &[u8]) -> Arc<OverlayShortId> {
        let overlay_key = OverlayKey { name: name.to_vec().into() };
        let id = hash(overlay_key).expect("hash overlay key");
        OverlayShortId::from_data(id)
    }

    /// Add public overlay
    pub fn add_public_overlay(&self, overlay_id: &Arc<OverlayShortId>) {
        self.rt.block_on(async {
            let params = OverlayParams::with_id_only(overlay_id);
            self.overlay.add_local_workchain_overlay(params).expect("Failed to add overlay");
        });
    }

    /// Add private overlay with given peer ADNL IDs (hex strings)
    /// Note: For simplicity, this creates a public overlay internally since creating
    /// a true private overlay requires a signing key. The C++ side creates a private
    /// overlay which doesn't require DHT.
    pub fn add_private_overlay(&self, overlay_id: &Arc<OverlayShortId>, _peers: Vec<String>) {
        self.rt.block_on(async {
            // For test purposes, just create as public overlay on Rust side
            // The C++ side creates a true private overlay
            let params = OverlayParams::with_id_only(overlay_id);
            self.overlay.add_local_workchain_overlay(params).expect("Failed to add overlay");
        });
    }

    /// Add a true private overlay with signing key and peer list.
    /// Unlike `add_private_overlay` (which creates a public overlay as shortcut),
    /// this creates a real private overlay where `try_consume_custom` is dispatched
    /// to the registered consumer.
    pub fn add_true_private_overlay(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        peers: &[Arc<KeyId>],
        use_quic: bool,
    ) {
        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        let params = OverlayParams {
            flags: 0,
            hops: None,
            overlay_id,
            runtime: Some(self.rt.handle().clone()),
        };
        self.overlay
            .add_private_overlay(params, &local_key, peers, use_quic)
            .expect("add_private_overlay failed");
    }

    /// Parse C++ node's base64 TL public key to raw 32-byte key
    fn parse_cpp_pubkey(pubkey_tl_b64: &str) -> [u8; 32] {
        let tl_bytes = base64_decode(pubkey_tl_b64).expect("decode pubkey b64");
        // TL: pub.ed25519#4813b4c6 key:int256 = PublicKey
        // Skip 4-byte constructor, take 32-byte key
        assert!(tl_bytes.len() >= 36, "TL pubkey too short: {}", tl_bytes.len());
        let key_bytes: [u8; 32] = tl_bytes[4..36].try_into().expect("wrong key len");
        key_bytes
    }

    /// Add the C++ node as an ADNL peer (but not to any overlay)
    pub fn add_cpp_peer(&self, cpp: &CppTestNode) {
        let raw_key = Self::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        let ip = IpAddress::from_versioned_string(&format!("127.0.0.1:{}", cpp.udp_port()), None)
            .expect("parse IP");

        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        self.adnl.add_peer(local_key.id(), &ip, None, &pubkey).expect("add_peer");
    }

    /// Add the C++ node as an ADNL peer AND to a specific public overlay via signed node.
    pub fn add_cpp_peer_to_overlay(&self, cpp: &mut CppTestNode, overlay_id: &Arc<OverlayShortId>) {
        let raw_key = Self::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        let ip = IpAddress::from_versioned_string(&format!("127.0.0.1:{}", cpp.udp_port()), None)
            .expect("parse IP");

        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        self.adnl.add_peer(local_key.id(), &ip, None, &pubkey).expect("add_peer");

        let signed_node = Self::get_cpp_signed_node(cpp, overlay_id);
        self.overlay.add_public_peer(&ip, &signed_node, overlay_id).expect("add_public_peer");
    }

    /// Add another Rust node as an ADNL peer AND to a specific public overlay via signed node.
    pub fn add_rust_peer_to_overlay(&self, other: &RustTestNode, overlay_id: &Arc<OverlayShortId>) {
        let other_key = other.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key on other node");
        let other_pubkey_data = other_key.pub_key().expect("No pub key on other node");
        let other_pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(
            other_pubkey_data.try_into().expect("Wrong key size"),
        );
        let other_ip = IpAddress::from_versioned_string(&other.addr, None).expect("parse other IP");

        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        self.adnl.add_peer(local_key.id(), &other_ip, None, &other_pubkey).expect("add_peer");

        let signed_node =
            other.overlay.get_signed_node(overlay_id, false).expect("get_signed_node");
        self.overlay.add_public_peer(&other_ip, &signed_node, overlay_id).expect("add_public_peer");
    }

    /// Get the KeyId for the C++ node (based on its public key)
    pub fn cpp_key_id(cpp: &CppTestNode) -> Arc<KeyId> {
        let raw_key = Self::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        pubkey.id().clone()
    }

    /// Get a signed overlay node description from the C++ node.
    fn get_cpp_signed_node(
        cpp: &mut CppTestNode,
        overlay_id: &Arc<OverlayShortId>,
    ) -> OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2> {
        let node_b64 = cpp
            .get_overlay_node_info(&hex::encode(overlay_id.data()))
            .expect("get_overlay_node_info failed");
        let node_bytes = base64_decode(&node_b64).expect("decode node_tl");
        let tl_obj = deserialize_boxed(&node_bytes).expect("deserialize node TL");
        let node = tl_obj.downcast::<OverlayNodeBoxed>().expect("downcast to overlay.Node");
        OverlayNodeInfo::V1(node.only())
    }

    /// Send a point-to-point overlay message (not broadcast) to a specific peer.
    /// This uses overlay.message() - the same path as consensus votes/certificates.
    pub fn send_message(&self, overlay_id: &Arc<OverlayShortId>, dst: &Arc<KeyId>, data: &[u8]) {
        self.rt.block_on(async {
            let tagged = TaggedByteSlice::with_object(data);
            self.overlay.message(dst, &tagged, overlay_id).await.expect("overlay message failed");
            println!("send_message: OK");
        });
    }

    /// Send broadcast via overlay
    pub fn send_broadcast(&self, overlay_id: &Arc<OverlayShortId>, data: &[u8]) {
        self.rt.block_on(async {
            let tagged = TaggedByteSlice::with_object(data);
            self.overlay
                .broadcast(overlay_id, &tagged, None, 0, AdnlSendMethod::Fast)
                .await
                .expect("broadcast failed");
        });
    }

    /// Send two-step FEC broadcast via overlay (requires RLDP)
    pub fn send_broadcast_twostep(&self, overlay_id: &Arc<OverlayShortId>, data: &[u8]) {
        self.rt.block_on(async {
            let tagged = TaggedByteSlice::with_object(data);
            self.overlay
                .broadcast_twostep(overlay_id, &tagged, None, 0, Vec::new())
                .await
                .expect("broadcast_twostep failed");
        });
    }

    /// Wait for a broadcast with timeout
    pub fn wait_for_broadcast(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        timeout_secs: u64,
    ) -> Option<Vec<u8>> {
        self.rt.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.overlay.wait_for_broadcast(overlay_id),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten()
            .map(|info| info.data)
        })
    }

    /// Register a query consumer (Subscriber) for an overlay.
    /// This is required for receiving RLDP queries on the Rust side.
    pub fn register_consumer(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        consumer: Arc<dyn Subscriber>,
    ) {
        self.overlay.add_consumer(overlay_id, consumer).expect("add_consumer failed");
    }

    /// Send an RLDP query via overlay and return the answer.
    /// Requires the node to have been created with `with_rldp=true`.
    ///
    /// The data is wrapped in a `tonNode.data` TL envelope and prepended with
    /// the `overlay.query` prefix, matching the C++ `Overlays::send_query_via` behavior.
    /// The answer is the raw bytes returned by the responder's echo handler.
    pub fn send_rldp_query(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        dst: &Arc<KeyId>,
        data: &[u8],
        max_answer_size: u64,
        v2: bool,
    ) -> Option<Vec<u8>> {
        self.rt.block_on(async {
            // Wrap data in tonNode.data TL envelope
            let tl_data = TonNodeData { data: data.to_vec().into() };
            // Get overlay query prefix (overlay.query{overlay=id})
            let mut query =
                self.overlay.get_query_prefix(overlay_id).expect("get_query_prefix failed");
            // Append the TL-serialized data object after the prefix
            serialize_boxed_append(&mut query, &tl_data.into_boxed())
                .expect("serialize_boxed_append failed");
            let tagged = TaggedByteSlice::with_object(&query);
            let (answer, _roundtrip) = self
                .overlay
                .query_via_rldp(dst, &tagged, overlay_id, Some(max_answer_size), v2, None)
                .await
                .expect("query_via_rldp failed");
            answer
        })
    }

    /// Stop the node
    pub fn stop(&self) {
        self.rt.block_on(async {
            self.adnl.stop().await;
        });
    }
}

/// A test consumer that echoes back queries
pub struct EchoConsumer;

impl EchoConsumer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

#[async_trait::async_trait]
impl Subscriber for EchoConsumer {
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        // Echo back - use .into() to properly handle telemetry feature
        Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Object(object.into())))))
    }
}

/// A subscriber that collects overlay messages (point-to-point, not broadcasts).
/// Used to verify delivery of overlay.message() calls on the receiving side.
pub struct MessageCollector {
    messages: Mutex<Vec<Vec<u8>>>,
    notify: tokio::sync::Notify,
}

impl MessageCollector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { messages: Mutex::new(Vec::new()), notify: tokio::sync::Notify::new() })
    }

    /// Wait until at least `count` messages are collected, or timeout.
    pub fn wait_for_messages(
        &self,
        rt: &tokio::runtime::Runtime,
        count: usize,
        timeout_secs: u64,
    ) -> Vec<Vec<u8>> {
        rt.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
            loop {
                {
                    let msgs = self.messages.lock().unwrap();
                    if msgs.len() >= count {
                        return msgs.clone();
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    return self.messages.lock().unwrap().clone();
                }
                tokio::select! {
                    _ = self.notify.notified() => {}
                    _ = tokio::time::sleep_until(deadline) => {
                        return self.messages.lock().unwrap().clone();
                    }
                }
            }
        })
    }
}

#[async_trait::async_trait]
impl Subscriber for MessageCollector {
    async fn try_consume_custom(&self, data: &[u8], _peers: &AdnlPeers) -> Result<bool> {
        self.messages.lock().unwrap().push(data.to_vec());
        self.notify.notify_waiters();
        Ok(true)
    }
}

/// A QUIC-capable subscriber that stores received messages and echoes queries.
/// Used for transport-level QUIC tests.
pub struct QuicTestSubscriber {
    key_id: Arc<KeyId>,
    msg_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl QuicTestSubscriber {
    pub fn new(key_id: Arc<KeyId>) -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Arc::new(Self { key_id, msg_tx: tx }), rx)
    }
}

#[async_trait::async_trait]
impl Subscriber for QuicTestSubscriber {
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
        // Echo back
        Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Object(object.into())))))
    }
}

/// A Rust ADNL + overlay + QUIC test node.
/// Extends RustTestNode with QuicNode for cross-implementation QUIC testing.
pub struct RustQuicTestNode {
    pub rt: tokio::runtime::Runtime,
    pub adnl: Arc<AdnlNode>,
    pub overlay: Arc<OverlayNode>,
    pub quic: Arc<QuicNode>,
    pub addr: String,
    pub port: u16,
    #[allow(dead_code)]
    key_data: [u8; 32],
    cancellation_token: tokio_util::sync::CancellationToken,
    quic_msg_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl RustQuicTestNode {
    /// Create a new Rust ADNL+overlay+QUIC node on the given IP:port.
    /// ADNL listens on `port` (UDP), QUIC listens on `port+1000`.
    pub fn new(ip: &str, port: u16) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("Failed to create tokio runtime");

        let addr = format!("{}:{}", ip, port);
        let zero_state = [0u8; 32];

        // Generate deterministic key from address
        let key_data = sha256_digest(addr.as_bytes());
        let keys = vec![(key_data, KEY_TAG_OVERLAY)];
        let (_, config) =
            AdnlNodeConfig::from_ip_address_and_private_keys(&addr, keys).expect("Config failed");

        let key_id = config.key_by_tag(KEY_TAG_OVERLAY).expect("No key").id().clone();

        let adnl = rt.block_on(AdnlNode::with_config(config)).expect("ADNL node creation failed");

        let overlay = OverlayNode::with_params(adnl.clone(), &zero_state, KEY_TAG_OVERLAY)
            .expect("Overlay node creation failed");

        // Start ADNL over UDP with overlay as subscriber
        let subscribers: Vec<Arc<dyn Subscriber>> = vec![overlay.clone()];
        rt.block_on(async {
            adnl.start_over_udp(subscribers).await.expect("Failed to start ADNL UDP");
        });

        // Create QuicNode with both a test subscriber and overlay
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let (test_sub, quic_msg_rx) = QuicTestSubscriber::new(key_id.clone());

        let quic = {
            let _guard = rt.enter();
            let quic_subscribers: Vec<Arc<dyn Subscriber>> =
                vec![test_sub as Arc<dyn Subscriber>, overlay.clone()];
            let quic = QuicNode::new(
                quic_subscribers,
                cancellation_token.clone(),
                rt.handle().clone(),
                Some(QuicRateLimitConfig::disabled()),
            );
            let bind_addr = SocketAddr::new(
                Ipv4Addr::from(adnl.ip_address_adnl().ip()).into(),
                adnl.ip_address_adnl().port() + QuicNode::OFFSET_PORT,
            );
            quic.add_key(&key_data, &key_id, bind_addr).expect("QUIC add_key failed");
            quic
        };

        Self {
            rt,
            adnl,
            overlay,
            quic,
            addr,
            port,
            key_data,
            cancellation_token,
            quic_msg_rx: Mutex::new(quic_msg_rx),
        }
    }

    /// Get the ADNL key ID (hex)
    pub fn adnl_id_hex(&self) -> String {
        self.adnl
            .key_by_tag(KEY_TAG_OVERLAY)
            .expect("No key")
            .id()
            .data()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    /// Get the ADNL public key as base64 TL-serialized
    pub fn pubkey_tl_b64(&self) -> String {
        let key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        let pub_key = key.pub_key().expect("No pub key");
        let tl_key =
            Ed25519PubKey { key: UInt256::with_array(pub_key.try_into().expect("Wrong key size")) };
        let serialized = serialize_boxed(&tl_key.into_boxed()).expect("Serialization failed");
        base64_encode(&serialized)
    }

    /// Get the ADNL key Arc<KeyId>
    pub fn adnl_key_id(&self) -> Arc<KeyId> {
        self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key").id().clone()
    }

    /// Add the C++ node as an ADNL peer (UDP)
    pub fn add_cpp_peer(&self, cpp: &CppTestNode) {
        let raw_key = RustTestNode::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        let ip = IpAddress::from_versioned_string(&format!("127.0.0.1:{}", cpp.udp_port()), None)
            .expect("parse IP");
        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        self.adnl.add_peer(local_key.id(), &ip, None, &pubkey).expect("add_peer");
    }

    /// Add the C++ node as a QUIC peer (registers its QUIC address = udp_port + 1000)
    pub fn add_cpp_quic_peer(&self, cpp: &CppTestNode) {
        let raw_key = RustTestNode::parse_cpp_pubkey(cpp.pubkey());
        let pubkey: Arc<dyn KeyOption> =
            Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        let quic_addr: SocketAddr =
            format!("127.0.0.1:{}", cpp.udp_port() + 1000).parse().expect("parse QUIC addr");
        self.quic.add_peer_key(pubkey.id().clone(), quic_addr).expect("add_quic_peer");
    }

    /// Add the C++ node as both ADNL and QUIC peer by simulating reception of an
    /// AddressList that contains adnl.address.quic (the new address type from C++ PR #2184).
    /// The QUIC address is discovered via `parse_quic_address` — no hardcoded offset.
    pub fn add_cpp_peer_via_address_list(&self, cpp: &CppTestNode, quic_port: u16) {
        let raw_key = RustTestNode::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);

        // Build an AddressList as a C++ node with PR #2184 would advertise:
        // both adnl.address.udp and adnl.address.quic
        let ip: u32 = u32::from(std::net::Ipv4Addr::new(127, 0, 0, 1));
        let addr_list = AddressList {
            addrs: vec![
                AdnlAddress::Adnl_Address_Udp(AdnlAddrUdp {
                    ip: ip as i32,
                    port: cpp.udp_port() as i32,
                }),
                AdnlAddress::Adnl_Address_Quic(AdnlAddrQuic {
                    ip: ip as i32,
                    port: quic_port as i32,
                }),
            ]
            .into(),
            version: adnl::common::Version::get(),
            reinit_date: adnl::common::Version::get(),
            priority: 0,
            expire_at: 0,
        };

        // Parse ADNL and QUIC addresses from the address list
        let (adnl_addr, quic_addr) =
            AdnlNode::parse_address_list(&addr_list).expect("parse").expect("has ADNL addr");

        // Add ADNL peer using the UDP address, passing QUIC address too
        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        let quic_addr = quic_addr.expect("AddressList should contain adnl.address.quic");
        self.adnl
            .add_peer(local_key.id(), &adnl_addr, Some(&quic_addr), &pubkey)
            .expect("add_peer");

        // Do NOT call quic.add_peer_key — let ensure_peer_registered discover
        // the QUIC address via adnl.peer_ip_address() at connection time.
    }

    /// Add C++ node as both ADNL peer (UDP) and QUIC peer, and to overlay
    pub fn add_cpp_peer_full(&self, cpp: &mut CppTestNode, overlay_id: &Arc<OverlayShortId>) {
        let raw_key = RustTestNode::parse_cpp_pubkey(cpp.pubkey());
        let pubkey = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&raw_key);
        let ip = IpAddress::from_versioned_string(&format!("127.0.0.1:{}", cpp.udp_port()), None)
            .expect("parse IP");
        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        self.adnl.add_peer(local_key.id(), &ip, None, &pubkey).expect("add_peer");

        // Add to overlay via signed node
        let signed_node = RustTestNode::get_cpp_signed_node(cpp, overlay_id);
        self.overlay.add_public_peer(&ip, &signed_node, overlay_id).expect("add_public_peer");

        // Add QUIC peer
        let quic_addr: SocketAddr =
            format!("127.0.0.1:{}", cpp.udp_port() + 1000).parse().expect("parse QUIC addr");
        self.quic.add_peer_key(pubkey.id().clone(), quic_addr).expect("add_quic_peer");
    }

    /// Get the KeyId for the C++ node
    pub fn cpp_key_id(cpp: &CppTestNode) -> Arc<KeyId> {
        RustTestNode::cpp_key_id(cpp)
    }

    /// Compute overlay name
    pub fn compute_overlay_name(&self, workchain: i32, shard: i64) -> Vec<u8> {
        let overlay_id = self.overlay.calc_overlay_id(workchain, shard).expect("calc_overlay_id");
        overlay_id.to_vec()
    }

    /// Compute overlay short ID
    pub fn compute_overlay_short_id(&self, workchain: i32, shard: i64) -> Arc<OverlayShortId> {
        self.overlay.calc_overlay_short_id(workchain, shard).expect("calc_overlay_short_id")
    }

    /// Add public overlay
    pub fn add_public_overlay(&self, overlay_id: &Arc<OverlayShortId>) {
        self.rt.block_on(async {
            let params = OverlayParams::with_id_only(overlay_id);
            self.overlay.add_local_workchain_overlay(params).expect("Failed to add overlay");
        });
    }

    /// Send a QUIC message (internally wrapped in quic.request.Message TL by QuicNode).
    /// Note: `data` is sent as-is inside quic_message.data_. On C++ side this goes through
    /// AdnlLocalId::deliver which requires matching TL prefix (e.g. overlay.message).
    /// Use `send_quic_overlay_message` to properly format data for overlay delivery.
    pub fn send_quic_message(&self, dst: &Arc<KeyId>, data: &[u8]) {
        let src = self.adnl_key_id();
        self.rt.block_on(async {
            self.quic
                .message(data.to_vec(), Some(&*self.adnl), &AdnlPeers::with_keys(src, dst.clone()))
                .await
                .expect("QUIC message failed");
        });
    }

    /// Send a QUIC message with overlay TL wrapping.
    /// Data is formatted as: overlay.message { overlay_id } ++ payload
    /// which matches the C++ AdnlLocalId callback prefix for overlay routing.
    pub fn send_quic_overlay_message(
        &self,
        dst: &Arc<KeyId>,
        overlay_id: &Arc<OverlayShortId>,
        payload: &[u8],
    ) {
        let src = self.adnl_key_id();
        let mut overlay_data = serialize_boxed(
            &OverlayMessage { overlay: UInt256::with_array(*overlay_id.data()) }.into_boxed(),
        )
        .expect("serialize overlay message prefix");
        overlay_data.extend_from_slice(payload);

        self.rt.block_on(async {
            self.quic
                .message(overlay_data, Some(&*self.adnl), &AdnlPeers::with_keys(src, dst.clone()))
                .await
                .expect("QUIC message failed");
        });
    }

    /// Send a QUIC query (internally wrapped in quic.request.Query TL by QuicNode).
    /// Returns the TL-deserialized answer bytes.
    /// Use `send_quic_overlay_query` for overlay-routed queries.
    pub fn send_quic_query(&self, dst: &Arc<KeyId>, data: &[u8]) -> Vec<u8> {
        let src = self.adnl_key_id();
        self.rt.block_on(async {
            self.quic
                .query(
                    data.to_vec(),
                    Some(&*self.adnl),
                    &AdnlPeers::with_keys(src, dst.clone()),
                    None,
                )
                .await
                .expect("QUIC query failed")
                .expect("empty QUIC query answer")
        })
    }

    /// Send a QUIC query with overlay TL wrapping, with a timeout.
    /// Data is formatted as: overlay.query { overlay_id } ++ payload
    /// Returns Ok(answer) or Err if timeout/connection fails.
    pub fn send_quic_overlay_query(
        &self,
        dst: &Arc<KeyId>,
        overlay_id: &Arc<OverlayShortId>,
        payload: &[u8],
        timeout_secs: u64,
    ) -> std::result::Result<Vec<u8>, String> {
        let src = self.adnl_key_id();
        let mut overlay_data =
            serialize_boxed(&OverlayQuery { overlay: UInt256::with_array(*overlay_id.data()) })
                .expect("serialize overlay query prefix");
        overlay_data.extend_from_slice(payload);

        self.rt.block_on(async {
            match tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.quic.query(
                    overlay_data,
                    Some(&*self.adnl),
                    &AdnlPeers::with_keys(src, dst.clone()),
                    None,
                ),
            )
            .await
            {
                Ok(Ok(Some(answer))) => Ok(answer),
                Ok(Ok(None)) => Err("empty QUIC query answer".to_string()),
                Ok(Err(e)) => Err(format!("QUIC query failed: {}", e)),
                Err(_) => Err("QUIC query timed out".to_string()),
            }
        })
    }

    /// Receive a QUIC message with timeout (from the test subscriber channel)
    pub fn recv_quic_message(&self, timeout_secs: u64) -> Option<Vec<u8>> {
        let mut rx = self.quic_msg_rx.lock().unwrap();
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(timeout_secs), rx.recv()).await.ok().flatten()
        })
    }

    /// Send overlay message via ADNL (overlay.message()).
    pub fn send_overlay_message(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        dst: &Arc<KeyId>,
        data: &[u8],
    ) {
        self.rt.block_on(async {
            let tagged = TaggedByteSlice::with_object(data);
            self.overlay.message(dst, &tagged, overlay_id).await.expect("overlay message failed");
            println!("send_overlay_message: OK");
        });
    }

    /// Send overlay query via ADNL (overlay.query()).
    /// Returns deserialized TLObject response, or None on timeout.
    pub fn send_overlay_query(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        dst: &Arc<KeyId>,
        query: &TaggedTlObject,
        timeout_ms: Option<u64>,
    ) -> Option<TLObject> {
        self.rt.block_on(async {
            self.overlay
                .query(dst, query, overlay_id, timeout_ms)
                .await
                .expect("overlay query failed")
        })
    }

    /// Create a private overlay with signing key and peer list.
    pub fn add_private_overlay(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        peers: &[Arc<KeyId>],
        use_quic: bool,
    ) {
        let local_key = self.adnl.key_by_tag(KEY_TAG_OVERLAY).expect("No key");
        let params = OverlayParams {
            flags: 0,
            hops: None,
            overlay_id,
            runtime: Some(self.rt.handle().clone()),
        };
        self.overlay
            .add_private_overlay(params, &local_key, peers, use_quic)
            .expect("add_private_overlay failed");
    }

    /// Stop the node. Shuts down the QUIC endpoint and ADNL node.
    pub fn stop(self) {
        self.cancellation_token.cancel();
        self.quic.shutdown();
        let adnl = self.adnl.clone();
        self.rt.block_on(async move {
            adnl.stop().await;
            // Give time for spawned tasks to observe cancellation and endpoint shutdown
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
    }
}
