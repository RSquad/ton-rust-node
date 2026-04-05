/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![allow(dead_code)]

include!("../../../common/src/config.rs");

use crate::{
    BlockPayloadPtr, ConsensusCommonFactory, ConsensusNode, ConsensusOverlay,
    ConsensusOverlayListener, ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListenerPtr,
    ConsensusOverlayManager, ConsensusOverlayManagerPtr, ConsensusOverlayPtr, OverlayTransportType,
    PrivateKey, PrivateOverlayShortId, PublicKeyHash, QueryResponseCallback,
};
use adnl::{node::AdnlNode, DhtNode, NetworkStack, OverlayNode, QuicNode, RldpNode};
use futures;
use lazy_static::lazy_static;
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

const KEY_TAG: usize = 0;
const CHANNEL_RESET_TIMEOUT_SEC: u8 = 5;
const PORT_BASE: u16 = 30000;

lazy_static! {
    static ref NODE_NETWORK_TEST_MUTEX: Mutex<()> = Mutex::new(());
}

pub struct Node {
    pub stack: Arc<NetworkStack>,
    pub overlay_manager: ConsensusOverlayManagerPtr,
    pub adnl_tag: usize,
    /// Network enable switch for test-gremlin scenarios.
    ///
    /// When `false`, this node drops all inbound overlay events and refuses to send
    /// outbound overlay traffic (queries fail fast).
    pub network_enabled: Arc<AtomicBool>,
}

pub struct NodeTestNetwork<'a> {
    mutex_lock: MutexGuard<'a, ()>, // lock to prevent multiple tests from running concurrently
    nodes: Vec<Arc<Node>>,
    runtime: tokio::runtime::Runtime,
    auto_shutdown: bool,
    is_shutdown: AtomicBool,
}

impl<'a> NodeTestNetwork<'a> {
    /// Create a new ADNL test network with the specified number of nodes and auto shutdown enabled
    pub fn create(test_name: &str, num_nodes: usize, num_threads_per_node: usize) -> Self {
        Self::create_with_options(test_name, num_nodes, num_threads_per_node, true, true, false)
    }

    /// Create a new ADNL test network with the specified number of nodes and configurable options
    pub fn create_with_options(
        test_name: &str,
        num_nodes: usize,
        num_threads_per_node: usize,
        auto_shutdown: bool,
        is_tcp_enabled: bool,
        is_quic_enabled: bool,
    ) -> Self {
        let mutex_lock = NODE_NETWORK_TEST_MUTEX.lock().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            //.worker_threads(num_nodes * 2)
            .worker_threads(num_nodes * num_threads_per_node)
            .enable_all()
            .build()
            .expect("Failed to create runtime");

        let nodes = Self::create_test_adnl_network(
            test_name,
            &runtime,
            num_nodes,
            is_tcp_enabled,
            is_quic_enabled,
        );

        Self { nodes, runtime, auto_shutdown, mutex_lock, is_shutdown: AtomicBool::new(false) }
    }

    /// Create a new ADNL test network with auto shutdown disabled (for performance tests)
    /// TCP is enabled by default for backward compatibility
    pub fn create_no_auto_shutdown(
        test_name: &str,
        num_nodes: usize,
        num_threads_per_node: usize,
        is_tcp_enabled: bool,
    ) -> Self {
        Self::create_with_options(
            test_name,
            num_nodes,
            num_threads_per_node,
            false,
            is_tcp_enabled,
            false,
        )
    }

    /// Get the number of nodes in the network
    pub fn num_nodes(&self) -> usize {
        self.nodes.len()
    }

    /// Get a node by index
    pub fn get_node(&self, index: usize) -> &Arc<Node> {
        &self.nodes[index]
    }

    /// Disable overlay networking for a specific node (inbound + outbound).
    pub fn disable_node_network(&self, node_idx: usize) {
        self.set_node_network_enabled(node_idx, false);
    }

    /// Enable overlay networking for a specific node (inbound + outbound).
    pub fn enable_node_network(&self, node_idx: usize) {
        self.set_node_network_enabled(node_idx, true);
    }

    /// Set overlay networking enabled/disabled for a node.
    pub fn set_node_network_enabled(&self, node_idx: usize, enabled: bool) {
        let node = &self.nodes[node_idx];
        node.network_enabled.store(enabled, Ordering::SeqCst);
        log::info!(
            "NodeTestNetwork: node {} overlay network {}",
            node_idx,
            if enabled { "ENABLED" } else { "DISABLED" }
        );
    }

    /// Check if overlay networking is enabled for a node.
    pub fn is_node_network_enabled(&self, node_idx: usize) -> bool {
        self.nodes[node_idx].network_enabled.load(Ordering::SeqCst)
    }

    /// Get all nodes
    pub fn get_nodes(&self) -> &Vec<Arc<Node>> {
        &self.nodes
    }

    /// Get the runtime reference
    pub fn get_runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    /// Check if the network has been shutdown
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    /// Manually shutdown all ADNL nodes (useful when auto_shutdown is disabled)
    pub fn shutdown(&self) {
        // Check if already shutdown and set flag atomically
        if self
            .is_shutdown
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            log::debug!("NodeTestNetwork shutdown already called, skipping");
            return;
        }

        log::debug!("NodeTestNetwork shutting down {} nodes", self.nodes.len());

        // Shutdown ADNL nodes
        let mut stop_futures = Vec::new();
        for node in &self.nodes {
            stop_futures.push(node.stack.adnl.stop());
        }
        self.runtime.block_on(async {
            let timeout_duration = std::time::Duration::from_secs(30);
            if let Err(_) =
                tokio::time::timeout(timeout_duration, futures::future::join_all(stop_futures))
                    .await
            {
                log::warn!("NodeTestNetwork shutdown timed out after 30 seconds");
            } else {
                log::debug!("NodeTestNetwork shutdown completed successfully");
            }
        });
    }

    async fn init_adnl_node(test_name: &str, ip: String) -> Arc<AdnlNode> {
        let configs_path = Path::new("../../target/configs");
        std::fs::create_dir_all(configs_path).expect("unable to create output configs path");
        let adnl_config_prefix = format!("{}/{}-catchain-adnl", configs_path.display(), test_name);

        let resolved_ip = resolve_ip(&ip).await.unwrap();
        let config_path = get_test_config_path(adnl_config_prefix.as_str(), &resolved_ip).unwrap();
        let config = if config_path.exists() {
            let json_str = std::fs::read_to_string(&config_path).unwrap();
            let json: AdnlNodeConfigJson = serde_json::from_str(&json_str).unwrap();
            adnl::node::AdnlNodeConfig::from_json_config(&json).unwrap()
        } else {
            let (json, bin) = generate_adnl_configs(&ip, vec![KEY_TAG], Some(resolved_ip)).unwrap();
            std::fs::File::create(&config_path)
                .unwrap()
                .write_all(serde_json::to_string_pretty(&json).unwrap().as_bytes())
                .unwrap();
            bin
        };
        AdnlNode::with_config(config).await.unwrap()
    }

    fn create_test_adnl_network(
        test_name: &str,
        runtime: &tokio::runtime::Runtime,
        num_nodes: usize,
        is_tcp_enabled: bool,
        is_quic_enabled: bool,
    ) -> Vec<Arc<Node>> {
        // Create futures for ADNL node initialization
        let mut futures = Vec::new();
        for i in 0..num_nodes {
            let ip = format!("127.0.0.1:{}", PORT_BASE + i as u16);
            let future = Self::init_adnl_node(test_name, ip);
            futures.push(future);
        }

        const ZERO_STATE_FILE_HASH: [u8; 32] = [1u8; 32];

        // Wait for all futures and collect results
        let nodes = runtime.block_on(async {
            let cancellation_token = tokio_util::sync::CancellationToken::new();
            let mut nodes = Vec::new();
            for (i, future) in futures.into_iter().enumerate() {
                let adnl = future.await;
                let dht = DhtNode::with_adnl_node(adnl.clone(), KEY_TAG).unwrap();
                let overlay =
                    OverlayNode::with_params(adnl.clone(), &ZERO_STATE_FILE_HASH, KEY_TAG).unwrap();
                let rldp =
                    RldpNode::with_params(adnl.clone(), vec![overlay.clone()], None).unwrap();
                overlay.set_rldp(rldp.clone()).unwrap();

                let quic = if is_quic_enabled {
                    let quic = QuicNode::new(
                        vec![overlay.clone()],
                        cancellation_token.clone(),
                        None,
                        tokio::runtime::Handle::current(),
                    );
                    overlay.set_quic(quic.clone()).unwrap();
                    Some(quic)
                } else {
                    None
                };

                let stack = Arc::new(NetworkStack { adnl, dht, overlay, rldp, quic });

                const BROADCAST_HOPS: Option<u8> = None;
                const TRACK_PRIVATE_PEERS: bool = true;
                let overlay_manager = ConsensusCommonFactory::create_adnl_overlay_manager(
                    runtime.handle().clone(),
                    stack.clone(),
                    BROADCAST_HOPS,
                    TRACK_PRIVATE_PEERS,
                )
                .unwrap();
                let network_enabled = Arc::new(AtomicBool::new(true));
                let overlay_manager =
                    ToggleableOverlayManager::create(overlay_manager, network_enabled.clone(), i);

                let subscribers: Vec<Arc<dyn adnl::common::Subscriber>> =
                    vec![stack.overlay.clone(), stack.dht.clone(), stack.rldp.clone()];
                if is_tcp_enabled {
                    stack.adnl.start_over_udp_tcp(subscribers).await.unwrap();
                } else {
                    stack.adnl.start_over_udp(subscribers).await.unwrap();
                }
                stack.adnl.set_channel_reset_timeout(CHANNEL_RESET_TIMEOUT_SEC).await;

                log::info!(
                    "Node started: index: {}, id: {}, ip: 127.0.0.1:{}, TCP support {}",
                    i,
                    stack.adnl.key_by_tag(KEY_TAG).unwrap().id(),
                    PORT_BASE + i as u16,
                    if stack.adnl.check_options(AdnlNode::OPTION_UDP_TCP) { "ON" } else { "OFF" }
                );

                nodes.push(Arc::new(Node {
                    stack,
                    overlay_manager,
                    adnl_tag: KEY_TAG,
                    network_enabled,
                }));
            }
            nodes
        });

        // Link with DHT network
        if num_nodes > 1 {
            for i in 0..num_nodes {
                for j in i + 1..num_nodes {
                    let dht_node1 = nodes[i].stack.dht.clone();
                    let dht_node2 = nodes[j].stack.dht.clone();

                    dht_node1.add_peer(&dht_node2.get_signed_node().unwrap()).unwrap();
                    dht_node2.add_peer(&dht_node1.get_signed_node().unwrap()).unwrap();
                }
            }
        }

        nodes
    }
}

impl<'a> Drop for NodeTestNetwork<'a> {
    fn drop(&mut self) {
        if self.auto_shutdown {
            self.shutdown();
        }
    }
}

// ============================================================================
// Network toggle wrappers (test utilities)
// ============================================================================

/// Overlay listener wrapper that drops inbound events when the node is "network disabled".
struct ToggleableOverlayListener {
    inner: ConsensusOverlayListenerPtr,
    enabled: Arc<AtomicBool>,
    node_idx: usize,
}

impl ConsensusOverlayListener for ToggleableOverlayListener {
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} dropping inbound message from {} (network disabled)",
                self.node_idx,
                adnl_id
            );
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            inner.on_message(adnl_id, data);
        }
    }

    fn on_broadcast(&self, source_key_hash: PublicKeyHash, data: &BlockPayloadPtr) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} dropping inbound broadcast from {} (network disabled)",
                self.node_idx,
                source_key_hash
            );
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            inner.on_broadcast(source_key_hash, data);
        }
    }

    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            // Intentionally drop the callback to simulate a network partition:
            // the requester should hit its own timeout.
            log::trace!(
                "NodeTestNetwork: node {} dropping inbound query from {} (network disabled)",
                self.node_idx,
                adnl_id
            );
            let _ = data; // keep signature stable; avoid unused warnings if logging disabled
            let _ = response_callback;
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            inner.on_query(adnl_id, data, response_callback);
        }
    }
}

/// Overlay wrapper that drops outbound traffic when the node is "network disabled".
struct ToggleableOverlay {
    inner: ConsensusOverlayPtr,
    enabled: Arc<AtomicBool>,
    node_idx: usize,
}

impl ConsensusOverlay for ToggleableOverlay {
    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} dropping outbound message to {} (network disabled)",
                self.node_idx,
                receiver_id
            );
            let _ = sender_id;
            let _ = message;
            let _ = is_retransmission;
            return;
        }
        self.inner.send_message(receiver_id, sender_id, message, is_retransmission);
    }

    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} dropping outbound multicast ({} receivers) (network disabled)",
                self.node_idx,
                receiver_ids.len()
            );
            let _ = sender_id;
            let _ = message;
            let _ = is_retransmission;
            return;
        }
        self.inner.send_message_multicast(receiver_ids, sender_id, message, is_retransmission);
    }

    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        name: &str,
        timeout: std::time::Duration,
        message: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} failing outbound query '{}' to {} (network disabled)",
                self.node_idx,
                name,
                receiver_id
            );
            let _ = sender_id;
            let _ = timeout;
            let _ = message;
            response_callback(Err(ton_block::error!("Network disabled")));
            return;
        }
        self.inner.send_query(receiver_id, sender_id, name, timeout, message, response_callback);
    }

    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} failing outbound RLDP query '{}' to {} (network disabled)",
                self.node_idx,
                name,
                dst_adnl_id
            );
            let _ = timeout;
            let _ = query;
            let _ = max_answer_size;
            let _ = v2;
            response_callback(Err(ton_block::error!("Network disabled")));
            return;
        }
        self.inner.send_query_via_rldp(
            dst_adnl_id,
            name,
            response_callback,
            timeout,
            query,
            max_answer_size,
            v2,
        );
    }

    fn send_broadcast_fec_ex(
        &self,
        sender_id: &PublicKeyHash,
        send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
        extra: Option<Vec<u8>>,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            log::trace!(
                "NodeTestNetwork: node {} dropping outbound broadcast (network disabled)",
                self.node_idx
            );
            let _ = sender_id;
            let _ = send_as;
            let _ = payload;
            return;
        }
        self.inner.send_broadcast_fec_ex(sender_id, send_as, payload, extra);
    }

    fn get_impl(&self) -> &dyn std::any::Any {
        self.inner.get_impl()
    }
}

/// Overlay manager wrapper that installs the inbound/outbound toggle wrappers.
struct ToggleableOverlayManager {
    inner: ConsensusOverlayManagerPtr,
    enabled: Arc<AtomicBool>,
    node_idx: usize,
    listener_keepalive: Mutex<HashMap<Arc<PrivateOverlayShortId>, Arc<ToggleableOverlayListener>>>,
}

impl ToggleableOverlayManager {
    fn create(
        inner: ConsensusOverlayManagerPtr,
        enabled: Arc<AtomicBool>,
        node_idx: usize,
    ) -> ConsensusOverlayManagerPtr {
        Arc::new(Self { inner, enabled, node_idx, listener_keepalive: Mutex::new(HashMap::new()) })
    }
}

impl ConsensusOverlayManager for ToggleableOverlayManager {
    fn start_overlay(
        &self,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
        overlay_listener: ConsensusOverlayListenerPtr,
        log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        transport_type: OverlayTransportType,
    ) -> Result<ConsensusOverlayPtr> {
        // Wrap listener to gate inbound traffic.
        let listener = Arc::new(ToggleableOverlayListener {
            inner: overlay_listener,
            enabled: self.enabled.clone(),
            node_idx: self.node_idx,
        });
        let listener_weak = Arc::downgrade(&listener) as ConsensusOverlayListenerPtr;

        let overlay = self.inner.start_overlay(
            local_validator_key,
            overlay_short_id,
            nodes,
            listener_weak,
            log_replay_listener,
            transport_type,
        )?;

        // Keep the listener alive only after start_overlay succeeds.
        self.listener_keepalive.lock().unwrap().insert(overlay_short_id.clone(), listener);

        // Wrap overlay to gate outbound traffic.
        Ok(Arc::new(ToggleableOverlay {
            inner: overlay,
            enabled: self.enabled.clone(),
            node_idx: self.node_idx,
        }))
    }

    fn stop_overlay(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        overlay: &ConsensusOverlayPtr,
    ) {
        self.listener_keepalive.lock().unwrap().remove(overlay_short_id);
        self.inner.stop_overlay(overlay_short_id, overlay);
    }
}
