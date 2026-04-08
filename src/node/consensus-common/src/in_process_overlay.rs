/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
// This file implements in-process overlay mocks for unit testing,
// using spin::Mutex for fast locking and log::debug!/log::trace! for tracing overlay lifecycle and method calls.

use super::{
    BlockPayloadPtr, ConsensusNode, ConsensusOverlay, ConsensusOverlayListenerPtr,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager, ConsensusOverlayManagerPtr,
    ConsensusOverlayPtr, OverlayTransportType, PrivateKey, PublicKeyHash, QueryResponseCallback,
};
use adnl::PrivateOverlayShortId;
use std::{
    any::Any,
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    thread,
    time::Duration,
};
use ton_block::{fail, Result};

const LOG_TARGET: &str = "in_process_overlay";

/*
===================================================================================================
    OverlayClientImpl
===================================================================================================
*/

struct OverlayClientImpl {
    network: Weak<OverlayNetworkImpl>,
    listener: ConsensusOverlayListenerPtr,
    overlay_short_id: Arc<PrivateOverlayShortId>, // Context: overlay network id
    local_id: PublicKeyHash,                      // Context: this node's id
    task_sender: crossbeam_channel::Sender<Box<dyn FnOnce() + Send>>, // Sender for async tasks
}

impl Drop for OverlayClientImpl {
    fn drop(&mut self) {
        log::debug!(
            target: LOG_TARGET,
            "Dropping OverlayClientImpl (overlay_short_id: {}, local_id: {})",
            self.overlay_short_id,
            self.local_id
        );
    }
}

impl ConsensusOverlay for OverlayClientImpl {
    /// Send a message to a single peer (asynchronously)
    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "OverlayClientImpl::send_message (overlay_short_id: {}, local_id: {}, \
            from: {sender_id}, to: {receiver_id}, msg_size: {})",
            self.overlay_short_id,
            self.local_id,
            message.data().len()
        );

        // Implement via send_message_multicast
        self.send_message_multicast(&[receiver_id.clone()], sender_id, message, _is_retransmission);
    }

    /// Send a message to multiple peers (asynchronously)
    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "OverlayClientImpl::send_message_multicast \
            (overlay_short_id: {}, local_id: {}, receivers: {}, from: {sender_id}, msg_size: {})",
            self.overlay_short_id,
            self.local_id,
            receiver_ids.len(),
            message.data().len()
        );

        let network_weak = self.network.clone();
        let receivers = receiver_ids.to_vec();
        let sender_id = sender_id.clone();
        let message = message.clone();
        let overlay_short_id = self.overlay_short_id.clone();

        let task: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
            if let Some(network) = network_weak.upgrade() {
                let mut listeners_to_call = Vec::new();
                let mut dead_receiver_ids = Vec::new();
                {
                    let mut overlays = network.overlays.lock();
                    for receiver_id in &receivers {
                        if let Some(client) = overlays.get(receiver_id) {
                            if let Some(listener) = client.listener.upgrade() {
                                listeners_to_call.push(listener);
                            } else {
                                dead_receiver_ids.push(receiver_id.clone());
                            }
                        } else {
                            log::trace!(
                                target: LOG_TARGET,
                                "send_message_multicast task: \
                                receiver_id {receiver_id} not found in overlay {overlay_short_id}"
                            );
                        }
                    }
                    for id in &dead_receiver_ids {
                        overlays.remove(id);
                    }
                }
                if !dead_receiver_ids.is_empty() {
                    log::debug!(
                        target: LOG_TARGET,
                        "send_message_multicast: removed {} dead client(s) from overlay {overlay_short_id}",
                        dead_receiver_ids.len(),
                    );
                }
                for listener in listeners_to_call {
                    listener.on_message(sender_id.clone(), &message);
                }
            } else {
                log::trace!(
                    target: LOG_TARGET,
                    "send_message_multicast task: network is gone for overlay {overlay_short_id}"
                );
            }
        });

        if self.task_sender.send(task).is_err() {
            log::error!(
                target: LOG_TARGET,
                "Failed to send message_multicast task: channel is closed"
            );
        }
    }

    /// Send a query to a peer (asynchronously)
    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        name: &str,
        _timeout: Duration,
        query: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "OverlayClientImpl::send_query (overlay_short_id: {}, local_id: {}, \
            from: {sender_id}, to: {receiver_id}, name: {name}, msg_size: {})",
            self.overlay_short_id,
            self.local_id,
            query.data().len()
        );

        let network_weak = self.network.clone();
        let receiver_id = receiver_id.clone();
        let sender_id = sender_id.clone();
        let query = query.clone();
        let overlay_short_id = self.overlay_short_id.clone();

        let task: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
            let listener_to_call = if let Some(network) = network_weak.upgrade() {
                let overlays = network.overlays.lock();
                if let Some(client) = overlays.get(&receiver_id) {
                    client.listener.upgrade()
                } else {
                    log::trace!(
                        target: LOG_TARGET,
                        "send_query task: \
                        receiver_id {receiver_id} not found in overlay {overlay_short_id}"
                    );
                    None
                }
            } else {
                log::trace!(
                    target: LOG_TARGET,
                    "send_query task: network is gone for overlay {overlay_short_id}"
                );
                None
            };

            if let Some(listener) = listener_to_call {
                listener.on_query(sender_id, &query, response_callback);
            }
        });

        if self.task_sender.send(task).is_err() {
            log::error!(
                target: LOG_TARGET,
                "Failed to send query task: channel is closed"
            );
        }
    }

    /// Send a query via RLDP (asynchronously)
    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        _timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        _max_answer_size: u64,
        _v2: bool,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "OverlayClientImpl::send_query_via_rldp \
            (overlay_short_id: {}, local_id: {}, to: {dst_adnl_id}, name: {name}, msg_size: {})",
            self.overlay_short_id,
            self.local_id,
            query.data().len()
        );

        // For the mock implementation, we can treat the timeout as a Duration.
        // In a real scenario, you might need to handle the SystemTime differently.
        let timeout_duration = std::time::Duration::from_secs(5); // Dummy timeout

        // Implement via send_query
        self.send_query(
            &dst_adnl_id,
            &self.local_id,
            &name,
            timeout_duration,
            &query,
            response_callback,
        );
    }

    /// Send a broadcast (not implemented in mock)
    fn send_broadcast_fec_ex(
        &self,
        sender_id: &PublicKeyHash,
        send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
    ) {
        log::trace!(
            target: LOG_TARGET,
            "OverlayClientImpl::send_broadcast_fec_ex (overlay_short_id: {}, local_id: {}, \
            from: {sender_id}, send_as: {send_as}, msg_size: {})",
            self.overlay_short_id,
            self.local_id,
            payload.data().len()
        );

        let network_weak = self.network.clone();
        let send_as = send_as.clone();
        let payload = payload.clone();
        let overlay_short_id = self.overlay_short_id.clone();

        let task: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
            if let Some(network) = network_weak.upgrade() {
                let mut listeners_to_call = Vec::new();
                let mut dead_receiver_ids = Vec::new();
                {
                    let mut overlays = network.overlays.lock();
                    for (receiver_id, client) in overlays.iter() {
                        if let Some(listener) = client.listener.upgrade() {
                            listeners_to_call.push(listener);
                        } else {
                            dead_receiver_ids.push(receiver_id.clone());
                        }
                    }
                    for id in &dead_receiver_ids {
                        overlays.remove(id);
                    }
                }
                if !dead_receiver_ids.is_empty() {
                    log::debug!(
                        target: LOG_TARGET,
                        "send_broadcast_fec_ex: removed {} dead client(s) from overlay {overlay_short_id}",
                        dead_receiver_ids.len(),
                    );
                }
                for listener in listeners_to_call {
                    listener.on_broadcast(send_as.clone(), &payload);
                }
            } else {
                log::trace!(
                    target: LOG_TARGET,
                    "send_broadcast_fec_ex task: network is gone for overlay {overlay_short_id}"
                );
            }
        });

        if self.task_sender.send(task).is_err() {
            log::error!(
                target: LOG_TARGET,
                "Failed to send broadcast task: channel is closed"
            );
        }
    }
    /// Get implementation-specific reference
    fn get_impl(&self) -> &dyn Any {
        self
    }
}

/*
===================================================================================================
    OverlayNetworkImpl
===================================================================================================
*/

struct OverlayNetworkImpl {
    overlays: spin::Mutex<HashMap<PublicKeyHash, Arc<OverlayClientImpl>>>,
    nodes: Vec<ConsensusNode>,
    overlay_short_id: Arc<PrivateOverlayShortId>, // Context: overlay network id
    task_sender: crossbeam_channel::Sender<Box<dyn FnOnce() + Send>>, // Sender for async tasks
}

impl Drop for OverlayNetworkImpl {
    fn drop(&mut self) {
        log::debug!(
            target: LOG_TARGET,
            "Dropping OverlayNetworkImpl (overlay_short_id: {})",
            self.overlay_short_id
        );
    }
}

impl OverlayNetworkImpl {
    /// Create a new overlay network for a set of nodes
    fn new(
        overlay_short_id: Arc<PrivateOverlayShortId>,
        nodes: Vec<ConsensusNode>,
        task_sender: crossbeam_channel::Sender<Box<dyn FnOnce() + Send>>,
    ) -> Self {
        log::debug!(
            target: LOG_TARGET,
            "Creating OverlayNetworkImpl (overlay_short_id: {overlay_short_id})"
        );
        Self { overlays: spin::Mutex::new(HashMap::new()), nodes, overlay_short_id, task_sender }
    }

    /// Get or create an overlay client for the given local_id
    fn get_or_create_overlay(
        self: &Arc<Self>,
        local_id: &PublicKeyHash,
        listener: ConsensusOverlayListenerPtr,
    ) -> Result<Arc<OverlayClientImpl>> {
        let mut overlays = self.overlays.lock();
        if let Some(existing_client) = overlays.get(local_id) {
            if existing_client.listener.upgrade().is_some() {
                log::debug!(
                    target: LOG_TARGET,
                    "OverlayNetworkImpl: found existing OverlayClientImpl with live listener \
                    (overlay_short_id: {}, local_id: {local_id})",
                    self.overlay_short_id
                );
                return Ok(Arc::clone(existing_client));
            }
            log::warn!(
                target: LOG_TARGET,
                "OverlayNetworkImpl: found existing OverlayClientImpl with DEAD listener, \
                replacing (overlay_short_id: {}, local_id: {local_id})",
                self.overlay_short_id
            );
            overlays.remove(local_id);
        }

        log::debug!(
            target: LOG_TARGET,
            "OverlayNetworkImpl: creating new OverlayClientImpl \
            (overlay_short_id: {}, local_id: {local_id})",
            self.overlay_short_id
        );
        let client = Arc::new(OverlayClientImpl {
            network: Arc::downgrade(self),
            listener,
            overlay_short_id: self.overlay_short_id.clone(),
            local_id: local_id.clone(),
            task_sender: self.task_sender.clone(),
        });
        overlays.insert(local_id.clone(), Arc::clone(&client));
        Ok(client)
    }

    /// Remove an overlay client for the given local_id
    fn remove_overlay(&self, local_id: &PublicKeyHash) {
        let mut overlays = self.overlays.lock();
        overlays.remove(local_id);
        log::debug!(
            target: LOG_TARGET,
            "OverlayNetworkImpl: removed OverlayClientImpl \
            (overlay_short_id: {}, local_id: {local_id})",
            self.overlay_short_id
        );
    }

    /// Check if the network is empty
    fn is_empty(&self) -> bool {
        self.overlays.lock().is_empty()
    }

    /// Get the list of nodes for this overlay network
    fn nodes(&self) -> &Vec<ConsensusNode> {
        &self.nodes
    }
}

/*
===================================================================================================
    OverlayManagerImpl
===================================================================================================
*/

pub(crate) struct OverlayManagerImpl {
    overlay_networks: spin::Mutex<HashMap<Arc<PrivateOverlayShortId>, Arc<OverlayNetworkImpl>>>,
    task_sender: crossbeam_channel::Sender<Box<dyn FnOnce() + Send + 'static>>,
    stopper: Arc<AtomicBool>,
    join_handles: Vec<thread::JoinHandle<()>>,
}

impl Drop for OverlayManagerImpl {
    fn drop(&mut self) {
        log::debug!(
            target: LOG_TARGET,
            "Dropping OverlayManagerImpl, stopping threads..."
        );
        self.stopper.store(true, Ordering::Relaxed);
        for handle in self.join_handles.drain(..) {
            handle.join().expect("OverlayManager worker thread panicked");
        }
    }
}

impl OverlayManagerImpl {
    /// Get or create an overlay network for the given overlay_short_id
    fn get_or_create_overlay_network(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
    ) -> Result<Arc<OverlayNetworkImpl>> {
        let mut networks = self.overlay_networks.lock();
        if let Some(network) = networks.get(overlay_short_id) {
            if !nodes_equal(network.nodes(), nodes) {
                fail!("Catchain nodes mismatch");
            }
            Ok(Arc::clone(network))
        } else {
            log::debug!(
                target: LOG_TARGET,
                "OverlayManagerImpl: creating new OverlayNetworkImpl \
                for overlay_short_id: {overlay_short_id}"
            );
            let network = Arc::new(OverlayNetworkImpl::new(
                overlay_short_id.clone(),
                nodes.to_vec(),
                self.task_sender.clone(),
            ));
            networks.insert(overlay_short_id.clone(), Arc::clone(&network));
            Ok(network)
        }
    }
}

impl ConsensusOverlayManager for OverlayManagerImpl {
    /// Start an overlay for the given local_id and overlay_short_id
    fn start_overlay(
        &self,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
        overlay_listener: ConsensusOverlayListenerPtr,
        _log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        _transport_type: OverlayTransportType,
    ) -> Result<ConsensusOverlayPtr> {
        let local_id = local_validator_key.id();
        log::debug!(
            target: LOG_TARGET,
            "OverlayManagerImpl::start_overlay called \
            (overlay_short_id: {overlay_short_id}, local_id: {local_id})"
        );
        let overlay_network = self.get_or_create_overlay_network(overlay_short_id, nodes)?;
        // Compare as slices; ConsensusNode must implement PartialEq
        if !nodes_equal(overlay_network.nodes().as_slice(), nodes) {
            log::warn!(
                target: LOG_TARGET,
                "OverlayManagerImpl: nodes mismatch for overlay_short_id: {overlay_short_id}"
            );
            fail!("OverlayNetworkImpl nodes mismatch for overlay_short_id");
        }
        let overlay_client = overlay_network.get_or_create_overlay(local_id, overlay_listener)?;
        Ok(overlay_client)
    }

    /// Stop an overlay for the given overlay_short_id and overlay client
    fn stop_overlay(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        overlay: &ConsensusOverlayPtr,
    ) {
        log::debug!(
            target: LOG_TARGET,
            "OverlayManagerImpl::stop_overlay called (overlay_short_id: {overlay_short_id})"
        );
        if let Some(client_impl) = overlay.get_impl().downcast_ref::<OverlayClientImpl>() {
            let mut networks = self.overlay_networks.lock();
            if let Some(network) = networks.get_mut(overlay_short_id) {
                network.remove_overlay(&client_impl.local_id);
                if network.is_empty() {
                    networks.remove(overlay_short_id);
                }
            }
        }
    }
}

impl OverlayManagerImpl {
    /// Create a new OverlayManagerImpl
    pub(crate) fn create(num_threads: usize) -> ConsensusOverlayManagerPtr {
        let (tx, rx) = crossbeam_channel::unbounded::<Box<dyn FnOnce() + Send + 'static>>();

        let stopper = Arc::new(AtomicBool::new(false));
        let mut join_handles = Vec::with_capacity(num_threads);

        for i in 0..num_threads {
            let rx_clone = rx.clone();
            let stopper_clone = stopper.clone();
            let join_handle = thread::Builder::new()
                .name(format!("in-process-overlay-worker-{}", i))
                .spawn(move || {
                    log::debug!(
                        target: LOG_TARGET,
                        "OverlayManager worker thread {i} started"
                    );
                    while !stopper_clone.load(Ordering::Relaxed) {
                        if let Ok(task) = rx_clone.recv_timeout(Duration::from_millis(100)) {
                            task();
                        }
                    }
                    log::debug!(
                        target: LOG_TARGET,
                        "OverlayManager worker thread {i} finished"
                    );
                })
                .expect("Failed to spawn thread");
            join_handles.push(join_handle);
        }

        Arc::new(OverlayManagerImpl {
            overlay_networks: spin::Mutex::new(HashMap::new()),
            task_sender: tx,
            stopper,
            join_handles,
        })
    }
}

// Helper function to compare two slices of ConsensusNode by value
fn nodes_equal(a: &[ConsensusNode], b: &[ConsensusNode]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (x, y) in a.iter().zip(b.iter()) {
        if x.adnl_id != y.adnl_id || x.public_key.id() != y.public_key.id() {
            return false;
        }
    }
    true
}
