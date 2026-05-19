/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    utils::MetricsHandle, BlockPayloadPtr, ConsensusCommonFactory, ConsensusNode, ConsensusOverlay,
    ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager,
    ConsensusOverlayManagerPtr, ConsensusOverlayPtr, OverlayTransportType, PrivateKey,
    PublicKeyHash, QueryResponseCallback, Result,
};
use adnl::{
    common::{
        AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice, TaggedTlObject,
        TimedAnswer, Wait,
    },
    node::{AdnlNode, AdnlSendMethod},
    CatchainData, DhtNode, NetworkStack, OverlayNode, OverlayParams, PrivateOverlayShortId,
};
use std::{
    any::Any,
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::mpsc;
use ton_api::{
    deserialize_boxed, serialize_bare, serialize_boxed, serialize_boxed_append,
    ton::{
        catchain::BroadcastWrapper,
        consensus::{
            simplex::{Certificate as SimplexCertificate, Vote as SimplexVote},
            RequestError as ConsensusRequestError,
        },
        overlay::{
            broadcast::BroadcastTwostepSimple, broadcast_twostep::id::Id as BroadcastTwostepId,
            broadcast_twostep_simple::tosign::ToSign as BroadcastTwostepSimpleToSign,
            Certificate as OverlayCertificate,
        },
    },
    BoxedSerialize, IntoBoxed, Serializer, TLObject,
};
use ton_block::{error, fail, sha256_digest, KeyId, KeyOption, UInt256};

const LOG_TARGET: &str = "consensus_adnl_overlay";

fn describe_query_response_error(error: &ConsensusRequestError) -> &'static str {
    match error {
        ConsensusRequestError::Consensus_RequestError => "consensus.requestError",
    }
}

fn extract_query_response_error(data: &[u8]) -> Option<String> {
    let message = deserialize_boxed(data).ok()?;
    let error = message.downcast::<ConsensusRequestError>().ok()?;
    Some(describe_query_response_error(&error).to_string())
}

fn normalize_query_response_payload(data: Vec<u8>) -> Result<BlockPayloadPtr> {
    if let Some(error_name) = extract_query_response_error(data.as_slice()) {
        return Err(error!("Peer returned {}", error_name));
    }

    Ok(ConsensusCommonFactory::create_block_payload(data))
}

/// Stream tags for task processor routing in ADNL overlay
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdnlOverlayStreamTag {
    OutgoingMessages = 0,
    OutgoingQueries = 1,
    OutgoingRldp = 2,
    OutgoingBroadcasts = 3,
}

impl AdnlOverlayStreamTag {
    /// Get the number of stream tags
    pub const fn count() -> u32 {
        4
    }
}

impl From<AdnlOverlayStreamTag> for u32 {
    fn from(tag: AdnlOverlayStreamTag) -> Self {
        tag as u32
    }
}

/*
    Task processor for sequential execution of closures
*/

struct TaskDesc {
    // Store a closure that produces a Future when called
    // This ensures the Future is constructed only in the task processor's loop
    task: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + 'static>,
    creation_time: SystemTime,
}

pub struct TaskProcessor {
    name: String,
    task_sender: mpsc::UnboundedSender<TaskDesc>,
    stop_requested: Arc<AtomicBool>,
    is_stopped: Arc<AtomicBool>,
    post_counter: metrics::Counter,
}

impl TaskProcessor {
    /// Create and immediately start new task processor with given name and metrics
    pub fn new(
        name: String,
        metrics_handle: MetricsHandle,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        log::debug!(target: LOG_TARGET, "Creating and starting TaskProcessor: {}", name);

        let (task_sender, mut task_receiver) = mpsc::unbounded_channel::<TaskDesc>();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let is_stopped = Arc::new(AtomicBool::new(false));

        let post_counter = metrics_handle
            .sink()
            .register_counter(&format!("task_processor_{}.posts", name).into());
        let pull_counter = metrics_handle
            .sink()
            .register_counter(&format!("task_processor_{}.pulls", name).into());

        // Start the async task loop immediately
        let name_clone = name.clone();
        let stop_requested_clone = stop_requested.clone();
        let is_stopped_clone = is_stopped.clone();
        let pull_counter_clone = pull_counter.clone();

        let _ = runtime_handle.spawn(async move {
            const CHECK_INTERVAL: Duration = Duration::from_millis(100);
            const TASK_AGE_WARNING_THRESHOLD: Duration = Duration::from_secs(5);
            const WARNING_THROTTLE_INTERVAL: Duration = Duration::from_secs(10);

            // Allow first warning immediately
            let mut last_warning_time = Instant::now() - WARNING_THROTTLE_INTERVAL;

            log::debug!(target: LOG_TARGET, "TaskProcessor loop started: {}", name_clone);

            loop {
                // Check stop flag every iteration (eliminates select! starvation)
                if stop_requested_clone.load(Ordering::Relaxed) {
                    log::debug!(target: LOG_TARGET, "TaskProcessor {} stop requested", name_clone);
                    break;
                }

                // Try to receive task with timeout
                match tokio::time::timeout(CHECK_INTERVAL, task_receiver.recv()).await {
                    Ok(Some(task_desc)) => {
                        // Task received - process it
                        pull_counter_clone.increment(1);

                        // Check task age and warn if it's too old (throttled to avoid spam)
                        if let Ok(elapsed) = task_desc.creation_time.elapsed() {
                            if elapsed > TASK_AGE_WARNING_THRESHOLD {
                                let now = Instant::now();
                                if now.duration_since(last_warning_time)
                                    >= WARNING_THROTTLE_INTERVAL
                                {
                                    log::warn!(
                                        target: LOG_TARGET,
                                        "TaskProcessor {name_clone}: \
                                        Processing delayed task (age: {elapsed:?})"
                                    );
                                    last_warning_time = now;
                                }
                            }
                        }

                        let future = (task_desc.task)();
                        future.await;
                    }
                    Ok(None) => {
                        // Channel closed
                        log::debug!(
                            target: LOG_TARGET,
                            "TaskProcessor channel closed: {name_clone}"
                        );
                        break;
                    }
                    Err(_) => {
                        // Timeout occurred - continue loop to check stop flag
                        continue;
                    }
                }
            }

            log::debug!(target: LOG_TARGET, "TaskProcessor loop finished: {}", name_clone);

            // Mark as actually stopped
            is_stopped_clone.store(true, Ordering::Relaxed);
        });

        Self { name, task_sender, stop_requested, is_stopped, post_counter }
    }

    /// Post an async closure to the task queue
    pub fn post_closure<F>(&self, closure: F)
    where
        F: FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + 'static,
    {
        if self.stop_requested.load(Ordering::Relaxed) {
            log::trace!(
                target: LOG_TARGET,
                "TaskProcessor {} stop requested, ignoring posted closure",
                self.name
            );
            return;
        }

        // Store the closure directly using Box::new
        let task_desc = TaskDesc { task: Box::new(closure), creation_time: SystemTime::now() };

        if let Err(_) = self.task_sender.send(task_desc) {
            log::warn!(target: LOG_TARGET, "Failed to send task to TaskProcessor {}", self.name);
        } else {
            self.post_counter.increment(1);
        }
    }

    /// Stop the task processor asynchronously
    pub fn stop_async(&self) {
        log::debug!(target: LOG_TARGET, "Stopping TaskProcessor asynchronously: {}", self.name);
        self.stop_requested.store(true, Ordering::Relaxed);
    }

    /// Stop the task processor and wait for completion
    pub fn stop(&self) {
        log::debug!(target: LOG_TARGET, "Stopping TaskProcessor synchronously: {}", self.name);

        self.stop_async();

        // Wait for is_stopped with small sleep delays
        const STOP_WAIT_DELAY: Duration = Duration::from_millis(100);

        let mut wait_count = 0;
        while !self.is_stopped.load(Ordering::Relaxed) {
            if wait_count % 10 == 0 {
                // Log every second
                log::info!(
                    target: LOG_TARGET,
                    "TaskProcessor {}: Waiting for stop completion... ({}ms)",
                    self.name,
                    wait_count * 100
                );
            }

            std::thread::sleep(STOP_WAIT_DELAY);
            wait_count += 1;
        }

        log::debug!(
            target: LOG_TARGET,
            "TaskProcessor {}: Stopped after {}ms",
            self.name,
            wait_count * 100
        );
    }
}

impl Drop for TaskProcessor {
    fn drop(&mut self) {
        log::debug!(target: LOG_TARGET, "Dropping TaskProcessor: {}", self.name);
        self.stop();
        log::debug!(target: LOG_TARGET, "Dropped TaskProcessor: {}", self.name);
    }
}

/*
    Task processor manager for managing multiple task processors
*/

pub struct TaskProcessorManager {
    name: String,
    processors: HashMap<u32, Arc<TaskProcessor>>,
    metrics_handle: MetricsHandle,
    stop_requested: Arc<AtomicBool>,
    is_stopped: Arc<AtomicBool>,
    runtime_handle: tokio::runtime::Handle,
}

impl TaskProcessorManager {
    /// Create new task processor manager with processors for each tag
    pub fn new(
        name: String,
        _adnl_ids: &[PublicKeyHash],
        num_tags: u32,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        log::info!(
            target: LOG_TARGET,
            "Creating TaskProcessorManager {name} with {num_tags} tags"
        );

        let metrics_handle = MetricsHandle::new(Some(Duration::from_secs(30)));
        let mut processors = HashMap::new();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let is_stopped = Arc::new(AtomicBool::new(false));
        let mut processor_names = Vec::new();

        // Create TaskProcessor for each tag
        for tag in 0..num_tags {
            let processor_name = format!("stream{:03}", tag);
            processor_names.push(processor_name.clone());

            let processor = Arc::new(TaskProcessor::new(
                processor_name,
                metrics_handle.clone(),
                runtime_handle.clone(),
            ));
            processors.insert(tag, processor);
        }

        log::info!(
            target: LOG_TARGET,
            "TaskProcessorManager {name}: Created and started {} TaskProcessors",
            processors.len()
        );

        let manager = Self {
            name: name.clone(),
            processors,
            metrics_handle,
            stop_requested,
            is_stopped,
            runtime_handle,
        };

        // Start metrics reporting automatically
        manager.start_metrics_reporting(processor_names);

        manager
    }

    /// Start the metrics reporting task
    fn start_metrics_reporting(&self, processor_names: Vec<String>) {
        let metrics_handle = self.metrics_handle.clone();
        let stop_requested = self.stop_requested.clone();
        let is_stopped = self.is_stopped.clone();
        let manager_name = self.name.clone();

        log::debug!(
            target: LOG_TARGET,
            "TaskProcessorManager {manager_name}: Starting metrics reporting"
        );

        let _handle = self.runtime_handle.spawn(async move {
            const METRICS_DUMP_PERIOD: Duration = Duration::from_secs(30);
            const SLEEP_PERIOD: Duration = Duration::from_millis(300);

            let mut next_metrics_dump_time = SystemTime::now() + METRICS_DUMP_PERIOD;

            log::debug!(
                target: LOG_TARGET,
                "TaskProcessorManager {manager_name}: Metrics loop started"
            );

            while !stop_requested.load(Ordering::Relaxed) {
                tokio::time::sleep(SLEEP_PERIOD).await;

                // Check if it's time to dump metrics
                if SystemTime::now() >= next_metrics_dump_time {
                    if log::log_enabled!(log::Level::Debug) {
                        // Create metrics dumper inside the task to avoid Send issues
                        let mut metrics_dumper = Self::create_metrics_dumper(&processor_names);
                        metrics_dumper.update(&metrics_handle);

                        log::debug!(
                            target: LOG_TARGET,
                            "TaskProcessorManager {manager_name} metrics:"
                        );
                        metrics_dumper.dump(
                            |string| log::debug!(target: LOG_TARGET, "{manager_name}: {string}"),
                        );
                    }

                    next_metrics_dump_time = SystemTime::now() + METRICS_DUMP_PERIOD;
                }
            }

            log::debug!(
                target: LOG_TARGET,
                "TaskProcessorManager {manager_name}: Metrics loop finished"
            );

            // Mark as actually stopped
            is_stopped.store(true, Ordering::Relaxed);
        });
    }

    /// Create metrics dumper for TaskProcessorManager
    fn create_metrics_dumper(processor_names: &[String]) -> crate::utils::MetricsDumper {
        let mut metrics_dumper = crate::utils::MetricsDumper::new();

        // Add derivative metrics for each task processor's posts and pulls
        for processor_name in processor_names {
            let posts_metric = format!("task_processor_{}.posts", processor_name);
            let pulls_metric = format!("task_processor_{}.pulls", processor_name);

            metrics_dumper.add_derivative_metric(&posts_metric);
            metrics_dumper.add_derivative_metric(&pulls_metric);
        }

        // Add computed queue size metrics for task processors
        for processor_name in processor_names {
            let queue_metric = format!("task_processor_{}_queue", processor_name);
            metrics_dumper
                .add_compute_handler(&queue_metric, crate::utils::compute_queue_size_counter);
        }

        metrics_dumper
    }

    /// Post an async closure to the appropriate task processor
    pub fn post_closure<F>(&self, _adnl_id: &PublicKeyHash, tag: u32, closure: F)
    where
        F: FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + 'static,
    {
        if self.stop_requested.load(Ordering::Relaxed) {
            log::trace!(
                target: LOG_TARGET,
                "TaskProcessorManager {} is stopped, ignoring posted closure",
                self.name
            );
            return;
        }

        if let Some(processor) = self.processors.get(&tag) {
            processor.post_closure(closure);
        } else {
            log::warn!(
                target: LOG_TARGET,
                "TaskProcessorManager {}: No TaskProcessor found for tag {tag}",
                self.name
            );
        }
    }

    /// Stop all task processors asynchronously
    pub fn stop_async(&self) {
        log::info!(
            target: LOG_TARGET,
            "TaskProcessorManager {}: Stopping asynchronously",
            self.name
        );

        self.stop_requested.store(true, Ordering::Relaxed);

        // Stop all task processors asynchronously
        for processor in self.processors.values() {
            processor.stop_async();
        }
    }

    /// Stop all task processors and wait for completion
    pub fn stop(&self) {
        log::info!(
            target: LOG_TARGET,
            "TaskProcessorManager {}: Stopping synchronously",
            self.name
        );

        self.stop_async();

        // Wait for is_stopped with small sleep delays
        const STOP_WAIT_DELAY: Duration = Duration::from_millis(100);

        let mut wait_count = 0;
        while !self.is_stopped.load(Ordering::Relaxed) {
            if wait_count % 10 == 0 {
                // Log every second
                log::info!(
                    target: LOG_TARGET,
                    "TaskProcessorManager {}: Waiting for stop completion... ({}ms)",
                    self.name,
                    wait_count * 100
                );
            }

            std::thread::sleep(STOP_WAIT_DELAY);
            wait_count += 1;
        }

        // Manually stop all task processors
        log::debug!(
            target: LOG_TARGET,
            "TaskProcessorManager {}: Stopping {} TaskProcessors",
            self.name,
            self.processors.len()
        );
        for (tag, processor) in &self.processors {
            log::trace!(
                target: LOG_TARGET,
                "TaskProcessorManager {}: Stopping TaskProcessor for tag={tag}",
                self.name
            );

            processor.stop();
        }

        log::info!(
            target: LOG_TARGET,
            "TaskProcessorManager {}: Stopped after {}ms",
            self.name,
            wait_count * 100
        );
    }
}

impl Drop for TaskProcessorManager {
    fn drop(&mut self) {
        log::info!(
            target: LOG_TARGET,
            "TaskProcessorManager {}: Dropping with {} processors",
            self.name,
            self.processors.len()
        );
        self.stop();
        log::info!(target: LOG_TARGET, "TaskProcessorManager {}: Dropped", self.name);
    }
}

/*
    Peer with async address resolution
*/

struct Peer {
    src_adnl_addr: Arc<KeyId>,
    dst_adnl_addr: Arc<KeyId>,
    overlay_node: Arc<OverlayNode>,
    dht_node: Arc<DhtNode>,
    is_stop_requested: Arc<AtomicBool>,
    is_stopped: Arc<AtomicBool>,
}

impl Peer {
    /// Create new peer and start address resolution loop
    fn new(
        src_adnl_addr: Arc<KeyId>,
        dst_adnl_addr: Arc<KeyId>,
        overlay_node: Arc<OverlayNode>,
        dht_node: Arc<DhtNode>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Arc<Self> {
        let peer = Arc::new(Self {
            src_adnl_addr,
            dst_adnl_addr,
            overlay_node,
            dht_node,
            is_stop_requested: Arc::new(AtomicBool::new(false)),
            is_stopped: Arc::new(AtomicBool::new(false)),
        });

        // Start address resolution loop
        let peer_clone = peer.clone();
        runtime_handle.spawn(async move {
            peer_clone.run_address_resolution_loop().await;
        });

        peer
    }

    /// Run async loop to resolve and update peer addresses
    async fn run_address_resolution_loop(self: Arc<Self>) {
        let mut current_addr = None;
        let mut last_resolution = Instant::now();
        let resolution_interval = Duration::from_secs(10);
        let check_interval = Duration::from_millis(100);

        loop {
            // Check stop flag every 100ms
            if self.is_stop_requested.load(Ordering::Relaxed) {
                break;
            }

            // Resolve address only every 10 seconds
            if last_resolution.elapsed() < resolution_interval {
                tokio::time::sleep(check_interval).await;
                continue;
            }

            last_resolution = Instant::now();

            // Try to fetch address from DHT
            match self.dht_node.fetch_address(&self.dst_adnl_addr).await {
                Ok(Some((adnl_addr, quic_addr, key))) => {
                    // Check if address changed (first time or address different)
                    let addr_changed = current_addr.is_none();

                    if addr_changed {
                        // Delete old address if exists
                        if current_addr.is_some() {
                            if let Err(e) = self.overlay_node.delete_private_peers(
                                &self.src_adnl_addr,
                                &[self.dst_adnl_addr.clone()],
                            ) {
                                log::warn!(
                                    target: LOG_TARGET,
                                    "Error deleting old peer address: {e:?}"
                                );
                            }
                        }

                        // Add new address
                        let add_result = self.overlay_node.add_private_peers_to_adnl(
                            &self.src_adnl_addr,
                            vec![(adnl_addr, quic_addr, key)],
                        );

                        if let Err(e) = add_result {
                            log::warn!(target: LOG_TARGET, "Error adding peer address: {:?}", e);
                        } else {
                            log::debug!(
                                target: LOG_TARGET,
                                "Peer address updated: {:?}",
                                self.dst_adnl_addr
                            );
                            current_addr = Some(()); // Mark that we have an address
                        }
                    }
                }
                Ok(None) => {
                    log::trace!(
                        target: LOG_TARGET,
                        "Peer address not found in DHT: {:?}",
                        self.dst_adnl_addr
                    );
                }
                Err(e) => {
                    log::warn!(
                        target: LOG_TARGET,
                        "DHT fetch error for peer {:?}: {e:?}",
                        self.dst_adnl_addr
                    );
                }
            }
        }

        // Delete private peer when exiting resolution loop
        if let Err(e) = self
            .overlay_node
            .delete_private_peers(&self.src_adnl_addr, &[self.dst_adnl_addr.clone()])
        {
            log::warn!(target: LOG_TARGET, "Error deleting peer at loop exit: {:?}", e);
        }

        // Mark as stopped (finished)
        self.is_stopped.store(true, Ordering::Relaxed);
    }

    /// Stop peer resolution loop asynchronously
    fn stop_async(&self) {
        self.is_stop_requested.store(true, Ordering::Relaxed);
    }

    /// Stop peer resolution loop synchronously and wait for completion
    fn stop(&self) {
        log::trace!(
            target: LOG_TARGET,
            "Stopping Peer: {:?} -> {:?}",
            self.src_adnl_addr,
            self.dst_adnl_addr
        );

        // Stop the resolution loop
        self.stop_async();

        // Wait for resolution loop to finish (sleep wait)
        let mut wait_count = 0;
        while !self.is_stopped.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            wait_count += 1;

            if wait_count % 10 == 0 {
                log::trace!(
                    target: LOG_TARGET,
                    "...waiting for peer resolution loop to finish: {:?} -> {:?}",
                    self.src_adnl_addr,
                    self.dst_adnl_addr,
                );
            }
        }

        log::trace!(
            target: LOG_TARGET,
            "Peer resolution loop finished: {:?} -> {:?}",
            self.src_adnl_addr,
            self.dst_adnl_addr
        );
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        // Call synchronous stop which waits for completion
        self.stop();
    }
}

/*
    Peer storage
*/

struct PeerStorage {
    peers: Arc<Mutex<HashMap<(Arc<KeyId>, Arc<KeyId>), Weak<Peer>>>>,
}

impl PeerStorage {
    fn new() -> Self {
        Self { peers: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Get or create peer for given src/dst ADNL addresses
    fn get_peer(
        self: &Arc<Self>,
        src_adnl_addr: Arc<KeyId>,
        dst_adnl_addr: Arc<KeyId>,
        overlay_node: Arc<OverlayNode>,
        dht_node: Arc<DhtNode>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Arc<Peer> {
        let mut peers = self.peers.lock().expect("PeerStorage mutex poisoned");
        let key = (src_adnl_addr.clone(), dst_adnl_addr.clone());

        // Try to upgrade existing weak reference
        if let Some(weak_peer) = peers.get(&key) {
            if let Some(peer) = weak_peer.upgrade() {
                return peer;
            }
        }

        // Create new peer if not exists or was dropped
        let peer = Peer::new(src_adnl_addr, dst_adnl_addr, overlay_node, dht_node, runtime_handle);

        peers.insert(key, Arc::downgrade(&peer));
        peer
    }

    /// Remove peers with move semantics and stop last instances
    fn remove_peers(&self, mut peers_to_remove: Vec<Arc<Peer>>) {
        //hold the lock for the duration of the function to avoid race conditions for new peers creation
        let mut peers_map = self.peers.lock().expect("PeerStorage mutex poisoned");

        for peer in &peers_to_remove {
            let key = (peer.src_adnl_addr.clone(), peer.dst_adnl_addr.clone());

            // Check if this is the last strong reference (Arc::strong_count == 2: one in vec, one we're holding)
            if Arc::strong_count(peer) == 2 {
                // This is the last reference - stop the peer and remove from map
                peer.stop_async();
                peers_map.remove(&key);
                log::trace!(
                    target: LOG_TARGET,
                    "PeerStorage: Stopping and removing last instance of peer {:?} -> {:?}",
                    peer.src_adnl_addr,
                    peer.dst_adnl_addr
                );
            }
        }

        // Manually stop all peers (this waits for completion)
        for peer in &peers_to_remove {
            peer.stop();
        }

        let peers_count = peers_to_remove.len();

        peers_to_remove.clear();

        log::trace!(target: LOG_TARGET, "PeerStorage: Stopped {} peers", peers_count);
    }
}

/*
    ADNL-based consensus overlay consumer implementation
*/

struct AdnlOverlayConsumer {
    overlay_id: Arc<PrivateOverlayShortId>, //overlay identifier for logging
    overlay: Weak<AdnlOverlay>,             //weak reference to overlay to avoid cycles
    stop_requested: Arc<AtomicBool>,        //shared stop flag with overlay
}

impl AdnlOverlayConsumer {
    /// Create new consumer with weak reference to overlay
    pub fn new(
        overlay_id: Arc<PrivateOverlayShortId>,
        overlay: Weak<AdnlOverlay>,
        stop_requested: Arc<AtomicBool>,
    ) -> Self {
        log::debug!(
            target: LOG_TARGET,
            "Creating AdnlOverlayConsumer for overlay_id={overlay_id}"
        );
        Self { overlay_id, overlay, stop_requested }
    }
}

#[async_trait::async_trait]
impl Subscriber for AdnlOverlayConsumer {
    async fn try_consume_custom(&self, data: &[u8], peers: &AdnlPeers) -> Result<bool> {
        let object = deserialize_boxed(data)?;

        // Handle catchain broadcast wrapper
        let object = match object.downcast::<BroadcastWrapper>() {
            Ok(BroadcastWrapper::Catchain_BroadcastWrapper(broadcast)) => {
                if let Some(overlay) = self.overlay.upgrade() {
                    let sender_id = crate::utils::int256_to_public_key_hash(&broadcast.sender_id);
                    let data = broadcast.data;
                    overlay.validate_and_process_broadcast(sender_id, &data);
                }
                return Ok(true);
            }
            // Not a broadcast wrapper - keep the original object
            Err(object) => object,
        };

        // Handle simplex direct messages (may come as custom messages in some paths)
        let simplex_kind = if object.is::<SimplexVote>() {
            Some("vote")
        } else if object.is::<SimplexCertificate>() {
            Some("certificate")
        } else {
            None
        };
        if let Some(simplex_kind) = simplex_kind {
            if let Some(overlay) = self.overlay.upgrade() {
                let payload = crate::ConsensusCommonFactory::create_block_payload(data.to_vec());
                let sender_id = peers.other().clone();
                log::trace!(
                    target: LOG_TARGET,
                    "AdnlOverlayConsumer: received simplex {} (custom) from {}",
                    simplex_kind,
                    sender_id
                );
                overlay.process_message(sender_id, payload);
            }
            return Ok(true);
        }

        Ok(false)
    }

    /// Try to consume query - upgrade weak reference and delegate to overlay
    async fn try_consume_query(&self, query: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        // Check if overlay is stopped
        if self.stop_requested.load(Ordering::Relaxed) {
            log::warn!(
                target: LOG_TARGET,
                "AdnlOverlayConsumer: Overlay {} was stopped!",
                &self.overlay_id
            );
            fail!("Overlay {} was stopped!", &self.overlay_id);
        }

        // Try to upgrade weak reference to overlay
        if let Some(overlay) = self.overlay.upgrade() {
            overlay.process_query(query, _peers).await
        } else {
            log::warn!(
                target: LOG_TARGET,
                "AdnlOverlayConsumer: Overlay {} was dropped!",
                &self.overlay_id
            );
            fail!("Overlay {} was dropped!", &self.overlay_id);
        }
    }
}

/*
    ADNL-based consensus overlay implementation
*/

struct AdnlOverlay {
    stack: Arc<NetworkStack>,                //ADNL network stack
    overlay_id: Arc<PrivateOverlayShortId>,  //private overlay short identifier
    local_id: PublicKeyHash,                 //local validator key hash
    local_validator_key: Arc<dyn KeyOption>, //local validator key for signing broadcasts
    local_adnl_key: Arc<dyn KeyOption>,      //local ADNL key for two-step broadcast signing
    adnl_to_validator: HashMap<PublicKeyHash, PublicKeyHash>, //ADNL key hash → validator key hash
    all_node_ids: Vec<PublicKeyHash>, //all node ADNL IDs in the overlay for multicast emulation of broadcast messages
    listener: ConsensusOverlayListenerPtr, //consensus overlay listener for incoming events
    stop_requested: Arc<AtomicBool>,  //atomic flag indicating if overlay stop is requested
    consumer: Arc<AdnlOverlayConsumer>, //consumer for handling overlay messages
    runtime_handle: tokio::runtime::Handle, //runtime handle for spawning broadcast listeners
    peers: Vec<Arc<Peer>>,            //active peers with address resolution
    peers_storage: Arc<PeerStorage>,  //peer storage for cleanup
    is_tcp_available: bool,           //flag indicating if TCP or QUIC is available for multicast
    is_quic_available: bool,          //flag indicating if QUIC transport is available
    task_processor_manager: TaskProcessorManager, //task processor manager for sequential processing
}

impl AdnlOverlay {
    /// Create new overlay implementation
    pub fn new(
        runtime_handle: tokio::runtime::Handle,
        stack: Arc<NetworkStack>,
        overlay_id: Arc<PrivateOverlayShortId>,
        local_validator_key: PrivateKey,
        listener: ConsensusOverlayListenerPtr,
        nodes: &[ConsensusNode],
        broadcast_hops: Option<u8>,
        track_private_peers: bool,
        peer_storage: Arc<PeerStorage>,
        transport_type: OverlayTransportType,
    ) -> Result<Arc<Self>> {
        let local_id = local_validator_key.id();
        let allow_tcp_communication = transport_type.allow_tcp();
        let use_quic = transport_type.use_quic();

        log::debug!(
            target: LOG_TARGET,
            "Creating new AdnlOverlay: overlay_id={overlay_id}, local_id={local_id}, \
            nodes_count={}, transport={transport_type:?}",
            nodes.len()
        );

        // Find local ADNL key from nodes by matching local_id
        let mut local_adnl_key: Option<Arc<dyn KeyOption>> = None;
        let mut peers = Vec::new();

        for node in nodes {
            if node.public_key.id() == local_id {
                // Found local node - extract ADNL key
                local_adnl_key = Some(stack.adnl.key_by_id(&node.adnl_id)?);
                log::trace!(
                    target: LOG_TARGET,
                    "Found local ADNL key: local_id={local_id}, adnl_id={}",
                    node.adnl_id
                );
                continue; // Skip adding local node to peers
            }
            peers.push(node.adnl_id.clone());
        }

        let local_adnl_key = local_adnl_key
            .ok_or_else(|| error!("Local ADNL key not found in nodes for local_id: {local_id}"))?;

        log::debug!(
            target: LOG_TARGET,
            "AdnlOverlay setup: overlay_id={overlay_id}, local_adnl_key={}, peers_count={}",
            local_adnl_key.id(),
            peers.len()
        );

        // Register QUIC keys if QUIC is enabled
        let quic_enabled = if use_quic {
            if let Some(quic) = &stack.quic {
                // Register local validator's ADNL key as a TLS identity on a per-port endpoint
                let key_bytes: [u8; 32] = *local_adnl_key.pvt_key()?;
                let ip_addr = stack.adnl.ip_address_adnl();
                let quic_port =
                    ip_addr.port().checked_add(adnl::QuicNode::OFFSET_PORT).ok_or_else(|| {
                        error!(
                            "QUIC bind port overflow: ADNL port {} + offset {} exceeds u16",
                            ip_addr.port(),
                            adnl::QuicNode::OFFSET_PORT
                        )
                    })?;
                let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), quic_port);
                quic.add_key(&key_bytes, local_adnl_key.id(), bind_addr)?;
                log::info!(
                    target: LOG_TARGET,
                    "Registered QUIC key for local ADNL id={}",
                    local_adnl_key.id()
                );

                log::info!(
                    target: LOG_TARGET,
                    "Using QUIC overlay transport for overlay {overlay_id}"
                );
                true
            } else {
                log::warn!(
                    target: LOG_TARGET,
                    "use_quic=true but QUIC transport not available, falling back to ADNL"
                );
                false
            }
        } else {
            false
        };

        // Initialize private overlay
        let params = OverlayParams {
            flags: 0,
            hops: broadcast_hops,
            overlay_id: &overlay_id,
            runtime: Some(runtime_handle.clone()),
        };
        stack.overlay.add_private_overlay(params, &local_adnl_key, &peers, use_quic)?;

        let stop_requested = Arc::new(AtomicBool::new(false));

        // Create Peer objects for address resolution if tracking is enabled
        let peer_objects = if track_private_peers {
            let mut peer_vec = Vec::new();
            for peer_id in &peers {
                let peer = peer_storage.get_peer(
                    local_adnl_key.id().clone(),
                    peer_id.clone(),
                    stack.overlay.clone(),
                    stack.dht.clone(),
                    runtime_handle.clone(),
                );
                peer_vec.push(peer);
            }
            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay: Created {} peer objects for address resolution",
                peer_vec.len()
            );
            peer_vec
        } else {
            Vec::new()
        };

        // Create overlay instance (without consumer)
        let local_id = local_id.clone();
        let overlay = Arc::new_cyclic(|weak_overlay| {
            // Create consumer with weak reference to overlay
            let consumer = Arc::new(AdnlOverlayConsumer::new(
                overlay_id.clone(),
                weak_overlay.clone(),
                stop_requested.clone(),
            ));

            // Point-to-point multicast is used for broadcasts when TCP transport is available
            let is_tcp_available = stack.is_tcp_available() && allow_tcp_communication;
            if is_tcp_available {
                log::debug!(
                    target: LOG_TARGET,
                    "AdnlOverlay: using multicast for overlay {overlay_id} \
                    (tcp={allow_tcp_communication}, quic={use_quic})"
                );
            } else {
                log::debug!(
                    target: LOG_TARGET,
                    "AdnlOverlay: using UDP only for overlay {overlay_id} \
                    (allow_tcp_communication={allow_tcp_communication})"
                );
            }

            // Initialize TaskProcessorManager with overlay_short_id as name
            let overlay_name = format!("overlay_{}", overlay_id);
            let all_node_ids: Vec<PublicKeyHash> =
                nodes.iter().map(|node| node.adnl_id.clone()).collect();
            let task_processor_manager = TaskProcessorManager::new(
                overlay_name,
                &all_node_ids,
                AdnlOverlayStreamTag::count(),
                runtime_handle.clone(),
            );

            // Build ADNL key → validator key mapping for two-step broadcast receive
            let adnl_to_validator: HashMap<PublicKeyHash, PublicKeyHash> = nodes
                .iter()
                .map(|node| (node.adnl_id.clone(), node.public_key.id().clone()))
                .collect();

            AdnlOverlay {
                stack,
                overlay_id: overlay_id.clone(),
                local_id,
                local_validator_key,
                local_adnl_key: local_adnl_key.clone(),
                adnl_to_validator,
                listener: listener.clone(),
                stop_requested: stop_requested.clone(),
                consumer,
                runtime_handle: runtime_handle.clone(),
                peers: peer_objects,
                peers_storage: peer_storage.clone(),
                is_tcp_available: is_tcp_available,
                is_quic_available: quic_enabled,
                all_node_ids: all_node_ids,
                task_processor_manager,
            }
        });

        // Add consumer to overlay node for message handling
        overlay.stack.overlay.add_consumer(&overlay_id, overlay.consumer.clone())?;

        log::debug!(
            target: LOG_TARGET,
            "Successfully initialized AdnlOverlay: overlay_id={overlay_id}"
        );

        Ok(overlay)
    }

    /// Stop the overlay and cleanup resources
    pub fn stop(&self) {
        log::trace!(target: LOG_TARGET, "Stopping AdnlOverlay: overlay_id={}", self.overlay_id);

        // Use CAS to ensure stop is called only once
        if self
            .stop_requested
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay already stopped: overlay_id={}",
                self.overlay_id
            );
            return; // Already stopped
        }

        // Stop task processor manager synchronously
        self.task_processor_manager.stop();

        // Delete private overlay from ADNL node within tokio runtime context
        let overlay_node = self.stack.overlay.clone();
        let overlay_id = self.overlay_id.clone();

        // Use the stored runtime handle to run in proper tokio context
        self.runtime_handle.block_on(async move {
            if let Err(e) = overlay_node.delete_private_overlay(&overlay_id) {
                log::warn!(target: LOG_TARGET, "Error deleting private overlay: {:?}", e);
            }
        });

        log::trace!(
            target: LOG_TARGET,
            "AdnlOverlay: Will cleanup {} peers on drop",
            self.peers.len()
        );

        log::debug!(
            target: LOG_TARGET,
            "Successfully stopped overlay: overlay_id={}, local_id={}",
            self.overlay_id,
            self.local_id
        );
    }

    /// Process incoming query from consumer
    pub async fn process_query(&self, query: TLObject, peers: &AdnlPeers) -> Result<QueryResult> {
        log::trace!(
            target: LOG_TARGET,
            "AdnlOverlay::process_query: overlay_id={}",
            self.overlay_id
        );

        let now = Instant::now();
        let data = serialize_boxed(&query).map_err(|e| {
            log::warn!(target: LOG_TARGET, "AdnlOverlay: query is bad: {:?}", e);
            e
        })?;

        // Spawn async task for consensus query processing
        let consensus_listener = self.listener.clone();
        let peers = peers.clone();
        let stop_requested = self.stop_requested.clone();

        let handle = tokio::spawn(async move {
            let (wait, mut queue_reader) = Wait::new();

            // Check if overlay is stopped before processing
            if stop_requested.load(Ordering::Relaxed) {
                log::trace!(target: LOG_TARGET, "AdnlOverlay: Query cancelled - overlay stopped");
                wait.respond(None); // Respond with None to close the waiter
                let res = wait
                    .wait(&mut queue_reader, true)
                    .await
                    .ok_or_else(|| error!("Waiting returned an internal error!"))?
                    .ok_or_else(|| error!("Answer was not set!"))?;
                return Ok(TimedAnswer {
                    answer: res?,
                    #[cfg(feature = "telemetry")]
                    actual_start_at: None,
                });
            }

            // Route query to consensus listener
            if let Some(listener) = consensus_listener.upgrade() {
                wait.request();
                let wait_for_response = wait.clone();
                let stop_requested_clone = stop_requested.clone();

                listener.on_query(
                    peers.other().clone(),
                    &crate::ConsensusCommonFactory::create_block_payload(data),
                    Box::new(move |result| {
                        // Check if stopped before responding
                        if stop_requested_clone.load(Ordering::Relaxed) {
                            log::trace!(
                                target: LOG_TARGET,
                                "AdnlOverlay: Query response cancelled - overlay stopped"
                            );
                            wait_for_response.respond(None);
                            return;
                        }

                        // Transform BlockPayloadPtr result to Answer
                        let answer_result = result.and_then(|payload| {
                            deserialize_boxed(payload.data())
                                .map(|answer| Some(Answer::Object(answer.into())))
                        });
                        wait_for_response.respond(Some(answer_result));
                    }),
                );
            }

            // Wait for response
            let res = wait
                .wait(&mut queue_reader, true)
                .await
                .ok_or_else(|| {
                    log::warn!(
                        target: LOG_TARGET,
                        "AdnlOverlay: Waiting returned an internal error (query: {query:?})"
                    );
                    error!("Waiting returned an internal error!")
                })?
                .ok_or_else(|| error!("Answer was not set!"))?;

            // Log timing and metrics
            let elapsed = now.elapsed();
            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay: query elapsed: {}ms",
                elapsed.as_millis()
            );
            metrics::histogram!("ton_node_network_consensus_overlay_query_seconds").record(elapsed);

            Ok(TimedAnswer {
                answer: res?,
                #[cfg(feature = "telemetry")]
                actual_start_at: None,
            })
        });

        Ok(QueryResult::Consumed(QueryAnswer::Pending(handle)))
    }

    /// Process incoming broadcast data — deliver directly to listener.
    fn validate_and_process_broadcast(self: Arc<Self>, recv_from: PublicKeyHash, data: &[u8]) {
        let payload = crate::ConsensusCommonFactory::create_block_payload(data.to_vec());
        self.process_broadcast(recv_from, payload);
    }

    /// Process incoming broadcast.
    /// `recv_from` may be an ADNL key hash (from two-step broadcasts) or a
    /// validator key hash (from BroadcastWrapper).  Translate to validator key
    /// hash when possible so the consensus layer sees a consistent identifier.
    fn process_broadcast(self: Arc<Self>, recv_from: PublicKeyHash, data: BlockPayloadPtr) {
        let source =
            self.adnl_to_validator.get(&recv_from).cloned().unwrap_or_else(|| recv_from.clone());
        log::trace!(
            target: LOG_TARGET,
            "AdnlOverlay: private overlay broadcast received (recv_from={recv_from}, source={source})"
        );
        if let Some(listener) = self.listener.upgrade() {
            listener.on_broadcast(source, &data);
        }
    }

    /// Process incoming direct message (e.g., simplex votes)
    fn process_message(&self, recv_from: PublicKeyHash, data: BlockPayloadPtr) {
        log::trace!(target: LOG_TARGET, "AdnlOverlay: direct message received from {}", recv_from);
        if let Some(listener) = self.listener.upgrade() {
            listener.on_message(recv_from, &data);
        }
    }

    /// Start broadcast listeners (similar to CatchainClient::run_wait_broadcast)
    pub fn run_wait_broadcast(self: Arc<Self>) {
        log::trace!(
            target: LOG_TARGET,
            "Starting broadcast listeners for overlay_id={}",
            self.overlay_id
        );

        let overlay_id = self.overlay_id.clone();
        let overlay = Arc::downgrade(&self);
        let overlay_node = self.stack.overlay.clone();
        let stop_requested1 = self.stop_requested.clone();
        let stop_requested2 = self.stop_requested.clone();

        // Spawn task for regular broadcasts
        self.runtime_handle.spawn(async move {
            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay::wait_broadcasts started for overlay_id={overlay_id}"
            );

            let receiver = overlay_node.clone();

            loop {
                if stop_requested1.load(Ordering::Relaxed) {
                    log::trace!(
                        target: LOG_TARGET,
                        "AdnlOverlay::wait_broadcasts stopping for overlay_id={overlay_id}"
                    );
                    break;
                }

                let message = receiver.wait_for_broadcast(&overlay_id).await;
                match message {
                    Ok(Some(message)) => {
                        log::trace!(
                            target: LOG_TARGET,
                            "AdnlOverlay::wait_broadcasts: received broadcast \
                            data_len={} recv_from={} overlay={overlay_id}",
                            message.data.len(),
                            message.recv_from
                        );
                        if let Some(overlay) = overlay.upgrade() {
                            let payload =
                                crate::ConsensusCommonFactory::create_block_payload(message.data);
                            overlay.process_broadcast(message.recv_from, payload);
                        }
                    }
                    Ok(None) => {
                        log::trace!(
                            target: LOG_TARGET,
                            "AdnlOverlay::wait_broadcasts finished for overlay_id={overlay_id}"
                        );
                        break;
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "AdnlOverlay: private overlay broadcast error: {e}"
                        );
                    }
                }
            }
        });

        let overlay_id = self.overlay_id.clone();
        let overlay_node = self.stack.overlay.clone();
        let listener = self.listener.clone();

        // Spawn task for consensus broadcasts
        self.runtime_handle.spawn(async move {
            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay::wait_consensus_broadcast started for overlay_id={overlay_id}"
            );

            let receiver = overlay_node.clone();
            let consensus_listener = listener.clone();

            loop {
                if stop_requested2.load(Ordering::Relaxed) {
                    log::trace!(
                        target: LOG_TARGET,
                        "AdnlOverlay::wait_consensus_broadcast stopping for overlay_id={overlay_id}"
                    );
                    break;
                }

                let message = receiver.wait_for_catchain(&overlay_id).await;
                match message {
                Ok(Some((catchain_block_update, inner_update, source_id))) => {
                    log::trace!(
                        target: LOG_TARGET,
                        "AdnlOverlay: catchain broadcast ValidatorSession_BlockUpdate received"
                    );
                    if let Some(listener) = consensus_listener.upgrade() {
                        // Serialize catchain block update and inner update similar to reference
                        let mut data: crate::RawBuffer = crate::RawBuffer::default();
                        let mut serializer = Serializer::new(&mut data);

                        match serializer.write_boxed(&catchain_block_update.into_boxed()) {
                            Ok(_) => {
                                match inner_update {
                                    CatchainData::Catchain(upd) => {
                                        if let Err(e) = serializer.write_boxed(&upd.into_boxed()) {
                                            log::error!(
                                                target: LOG_TARGET,
                                                "AdnlOverlay: Failed to serialize catchain update: {e}"
                                            );
                                            continue;
                                        }
                                    }
                                    CatchainData::ValidatorSession(upd) => {
                                        if let Err(e) = serializer.write_boxed(&upd.into_boxed()) {
                                            log::error!(
                                                target: LOG_TARGET,
                                                "AdnlOverlay: Failed to serialize validator session update: {e}"
                                            );
                                            continue;
                                        }
                                    }
                                }

                                let data = crate::ConsensusCommonFactory::create_block_payload(data);
                                log::trace!(
                                    target: LOG_TARGET,
                                    "AdnlOverlay: routing consensus broadcast to listener via on_message"
                                );
                                listener.on_message(source_id, &data);
                            }
                            Err(e) => {
                                log::error!(
                                    target: LOG_TARGET,
                                    "AdnlOverlay: Failed to serialize catchain block update: {e}"
                                );
                            }
                        }
                    }
                }
                    Ok(None) => {
                        log::trace!(
                            target: LOG_TARGET,
                            "AdnlOverlay::wait_consensus_broadcast finished for overlay_id={overlay_id}"
                        );
                        break;
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "AdnlOverlay: consensus broadcast error: {e}"
                        );
                    }
                }
            }
        });
    }
}

impl Drop for AdnlOverlay {
    fn drop(&mut self) {
        log::debug!(target: LOG_TARGET, "Dropping AdnlOverlay: overlay_id={}", self.overlay_id);
        self.stop();

        // Take peers vector and pass to remove_peers for cleanup
        let peers_vec = std::mem::take(&mut self.peers);
        self.peers_storage.remove_peers(peers_vec);
    }
}

impl ConsensusOverlay for AdnlOverlay {
    /// Get implementation-specific object
    fn get_impl(&self) -> &dyn Any {
        self
    }

    /// Send direct message to specific receiver
    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        // Delegate to multicast with single receiver
        self.send_message_multicast(&[receiver_id.clone()], sender_id, message, is_retransmission);
    }

    /// Send message to multiple receivers (multicast)
    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        if self.stop_requested.load(Ordering::Relaxed) {
            log::warn!(
                target: LOG_TARGET,
                "AdnlOverlay: Overlay {} was stopped!",
                &self.overlay_id
            );
            return;
        }

        log::trace!(
            target: LOG_TARGET,
            "AdnlOverlay::send_message_multicast (overlay: {}, from: {}, to: [{:#?}])",
            &self.overlay_id,
            &sender_id,
            &receiver_ids,
        );

        // Send message to each receiver through TaskProcessorManager
        for receiver_id in receiver_ids {
            let overlay_node = self.stack.overlay.clone();
            let overlay_id = self.overlay_id.clone();
            let msg = message.clone();
            let receiver = receiver_id.clone();
            let sender_id = sender_id.clone();
            let is_quic = self.is_quic_available;

            // Post async closure to task processor for this receiver with OutgoingMessages tag
            self.task_processor_manager.post_closure(
                receiver_id,
                AdnlOverlayStreamTag::OutgoingMessages.into(),
                move || {
                    Box::pin(async move {
                        // Execute async message sending directly in task processor loop
                        // Extract message data and calculate tag from first 4 bytes (little-endian)
                        let buf = &msg.data();
                        let msg_tagged = TaggedByteSlice {
                            #[cfg(feature = "telemetry")]
                            tag: 0x80000001, // Catchain one-way messages
                            object: &buf[..],
                        };

                        // Execute async message sending — via QUIC if available, else ADNL
                        let result = if is_quic {
                            overlay_node.message_via_quic(&receiver, &msg_tagged, &overlay_id).await
                        } else {
                            overlay_node.message(&receiver, &msg_tagged, &overlay_id).await
                        };

                        if let Err(e) = result {
                            let tag = if buf.len() >= 4 {
                                u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
                            } else {
                                0
                            };

                            log::warn!(
                                target: LOG_TARGET,
                                "AdnlOverlay: Failed to send message with tag {tag:08x} \
                                from {sender_id} to {receiver}: {e}"
                            );
                        }
                    })
                },
            );
        }
    }

    /// Send query with response callback
    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        _sender_id: &PublicKeyHash,
        _name: &str,
        timeout: Duration,
        message: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        let receiver = receiver_id.clone();
        let msg = message.clone();
        let overlay_id = self.overlay_id.clone();
        let overlay_node = self.stack.overlay.clone();
        let stop_requested = self.stop_requested.clone();
        let is_quic = self.is_quic_available;

        if stop_requested.load(Ordering::Relaxed) {
            log::warn!(target: LOG_TARGET, "AdnlOverlay: Overlay {} was stopped!", &overlay_id);
            return;
        }

        self.runtime_handle.spawn(async move {
            if stop_requested.load(Ordering::Relaxed) {
                log::warn!(target: LOG_TARGET, "AdnlOverlay: Overlay {} was stopped!", &overlay_id);
                return;
            }

            // Execute query with timeout and handle result
            let result = async {
                let query: TaggedTlObject = deserialize_boxed(msg.data())?.into();
                let timeout_ms = Some(AdnlNode::calc_timeout(Some(timeout.as_millis() as u64)));
                let now = Instant::now();

                let result = if is_quic {
                    overlay_node.query_via_quic(&receiver, &query, &overlay_id, timeout_ms).await?
                } else {
                    overlay_node.query(&receiver, &query, &overlay_id, timeout_ms).await?
                };

                let elapsed = now.elapsed();
                log::trace!(
                    target: LOG_TARGET,
                    "AdnlOverlay::send_query result (overlay: {}, key_id: {}): {:?} ({}ms)",
                    overlay_id,
                    receiver,
                    result,
                    elapsed.as_millis()
                );

                metrics::histogram!("ton_node_network_consensus_overlay_query_seconds")
                    .record(elapsed);

                let result = result.ok_or_else(|| error!("answer is None!"))?;
                let data = serialize_boxed(&result)?;
                normalize_query_response_payload(data)
            }
            .await;

            // Only call callback if overlay is not stopped
            if !stop_requested.load(Ordering::Relaxed) {
                response_callback(result);
            } else {
                log::trace!(
                    target: LOG_TARGET,
                    "AdnlOverlay: Skipping query callback - overlay stopped"
                );
            }
        });
    }

    /// Send query via RLDP for large messages.
    /// On QUIC-enabled overlays (simplex+quic) the query is routed through QUIC
    /// bi-directional streams instead, since QUIC handles flow control natively
    /// and doesn't need `max_answer_size`.
    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        _name: String,
        response_callback: QueryResponseCallback,
        _timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        let overlay_id = self.overlay_id.clone();
        let overlay_node = self.stack.overlay.clone();
        let stop_requested = self.stop_requested.clone();
        let runtime_handle = self.runtime_handle.clone();
        let is_quic = self.is_quic_available;

        if stop_requested.load(Ordering::Relaxed) {
            log::warn!(target: LOG_TARGET, "AdnlOverlay: Overlay {} was stopped!", &overlay_id);
            return;
        }

        runtime_handle.spawn(async move {
            if stop_requested.load(Ordering::Relaxed) {
                log::warn!(target: LOG_TARGET, "AdnlOverlay: Overlay {} was stopped!", &overlay_id);
                return;
            }

            let result = async {
                let query_body = deserialize_boxed(query.data())?;

                if is_quic {
                    // QUIC path: use bi-directional stream query (no max_answer_size needed)
                    let tagged: TaggedTlObject = query_body.into();
                    let result = overlay_node
                        .query_via_quic(&dst_adnl_id, &tagged, &overlay_id, None)
                        .await?;
                    let result = result.ok_or_else(|| error!("QUIC query answer is None!"))?;
                    let data = serialize_boxed(&result)?;
                    normalize_query_response_payload(data)
                } else {
                    // RLDP path: traditional large-message query
                    let mut query_data = overlay_node.get_query_prefix(&overlay_id)?;
                    serialize_boxed_append(&mut query_data, &query_body)?;

                    let (data, _) = overlay_node
                        .query_via_rldp(
                            &dst_adnl_id,
                            &TaggedByteSlice {
                                object: &query_data[..],
                                #[cfg(feature = "telemetry")]
                                tag: query_body.bare_object().constructor(),
                            },
                            &overlay_id,
                            Some(max_answer_size),
                            v2,
                            None,
                        )
                        .await?;
                    let data = data.ok_or_else(|| error!("answer is None!"))?;
                    normalize_query_response_payload(data)
                }
            }
            .await;

            log::info!(target: LOG_TARGET, "AdnlOverlay::send_query_via_rldp: {:?}", result);

            // Only call callback if overlay is not stopped
            if !stop_requested.load(Ordering::Relaxed) {
                response_callback(result);
            } else {
                log::trace!(
                    target: LOG_TARGET,
                    "AdnlOverlay: Skipping RLDP query callback - overlay stopped"
                );
            }
        });
    }

    /// Send broadcast with FEC (Forward Error Correction)
    fn send_broadcast_fec_ex(
        &self,
        sender_id: &PublicKeyHash,
        _send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
        extra: Option<Vec<u8>>,
    ) {
        let overlay_id = self.overlay_id.clone();
        let stop_requested = self.stop_requested.clone();

        if stop_requested.load(Ordering::Relaxed) {
            log::warn!(target: LOG_TARGET, "AdnlOverlay: Overlay {overlay_id} was stopped!");
            return;
        }

        if self.is_quic_available || !self.is_tcp_available {
            // QUIC or ADNL/UDP path
            // If extra given, use two-step broadcast via QUIC/RLDP
            // Otherwise use canonic broadcast via ADNL
            let msg = payload.clone();
            let overlay_node = self.stack.overlay.clone();
            let local_validator_key = self.local_validator_key.clone();
            let transport = if self.is_quic_available { "QUIC" } else { "ADNL/UDP" };

            self.runtime_handle.spawn(async move {
                if stop_requested.load(Ordering::Relaxed) {
                    log::warn!(
                        target: LOG_TARGET,
                        "AdnlOverlay: Overlay {overlay_id} was stopped!"
                    );
                    return;
                }

                let msg_tagged = TaggedByteSlice {
                    object: msg.data(),
                    #[cfg(feature = "telemetry")]
                    tag: 0x80000002, // Consensus broadcast
                };

                let result = if let Some(extra) = extra {
                    // Twostep broadcast with extra
                    overlay_node
                        .broadcast_twostep(
                            &overlay_id,
                            &msg_tagged,
                            Some(&local_validator_key),
                            0,
                            extra,
                        )
                        .await
                } else {
                    // Canonic broadcast
                    overlay_node
                        .broadcast(
                            &overlay_id,
                            &msg_tagged,
                            Some(&local_validator_key),
                            0,
                            AdnlSendMethod::Fast,
                        )
                        .await
                };

                match &result {
                    Ok(info) if info.send_to == 0 => log::warn!(
                        target: LOG_TARGET,
                        "AdnlOverlay::send_broadcast_fec_ex ({transport}) \
                        overlay={overlay_id}: send_to=0 packets={} — \
                        broadcast emitted to 0 neighbours (overlay known_peers empty?)",
                        info.packets,
                    ),
                    Ok(info) => log::debug!(
                        target: LOG_TARGET,
                        "AdnlOverlay::send_broadcast_fec_ex ({transport}) \
                        overlay={overlay_id} packets={} send_to={}",
                        info.packets, info.send_to,
                    ),
                    Err(e) => log::warn!(
                        target: LOG_TARGET,
                        "AdnlOverlay::send_broadcast_fec_ex ({transport}) \
                        overlay={overlay_id} FAILED: {e:?}"
                    ),
                }
            });
        } else {
            // TCP path: manually build BroadcastTwostepSimple and multicast
            const IS_RETRANSMISSION: bool = false;

            log::trace!(
                target: LOG_TARGET,
                "AdnlOverlay::send_broadcast_fec_ex: is_tcp_available=true, \
                sending BroadcastTwostepSimple (payload_len={}) to {} peers",
                payload.data().len(), self.all_node_ids.len(),
            );

            let result = (|| -> Result<()> {
                let data = payload.data().to_vec();
                let extra = extra.unwrap_or_default();
                let date = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i32;
                let flags: i32 = 0;

                let data_hash = sha256_digest(&data);
                let bcast_id = {
                    let id = BroadcastTwostepId {
                        date,
                        flags,
                        src: UInt256::from_slice(self.local_adnl_key.id().data()),
                        src_adnl_id: UInt256::from_slice(self.local_adnl_key.id().data()),
                        data_hash: UInt256::with_array(data_hash),
                        // Broadcast simulation over TCP, no partitioning
                        data_size: data.len() as i32,
                        part_size: data.len() as i32,
                        extra: extra.clone(),
                    };
                    let id_bytes = serialize_bare(&id)?;
                    sha256_digest(&id_bytes)
                };

                let to_sign = BroadcastTwostepSimpleToSign {
                    id: UInt256::with_array(bcast_id),
                    data: data.clone(),
                };
                let to_sign_bytes = serialize_bare(&to_sign)?;
                let signature = self.local_adnl_key.sign(&to_sign_bytes)?;

                // Build BroadcastTwostepSimple TL
                let bcast = BroadcastTwostepSimple {
                    date,
                    flags,
                    src: (&self.local_adnl_key).try_into()?,
                    src_adnl_id: UInt256::from_slice(self.local_adnl_key.id().data()),
                    certificate: OverlayCertificate::Overlay_EmptyCertificate,
                    data,
                    extra,
                    signature,
                }
                .into_boxed();

                // Serialize as overlay Broadcast (same format the overlay layer
                // expects on the receive side in try_consume_custom)
                let serialized = serialize_boxed(&bcast)?;
                let broadcast_payload =
                    crate::ConsensusCommonFactory::create_block_payload(serialized);

                log::trace!(
                    target: LOG_TARGET,
                    "AdnlOverlay::send_broadcast_fec_ex: sending BroadcastTwostepSimple \
                    ({} bytes payload) via TCP multicast to {} peers",
                    broadcast_payload.data().len(),
                    self.all_node_ids.len(),
                );

                self.send_message_multicast(
                    &self.all_node_ids,
                    sender_id,
                    &broadcast_payload,
                    IS_RETRANSMISSION,
                );
                Ok(())
            })();

            if let Err(err) = result {
                log::error!(
                    target: LOG_TARGET,
                    "AdnlOverlay::send_broadcast_fec_ex: failed to build/send TCP broadcast: {err}"
                );
            }
        }
    }
}

/*
    ADNL-based overlay manager implementation
*/

pub struct AdnlOverlayManager {
    runtime_handle: tokio::runtime::Handle, //runtime handle for spawning tasks
    stack: Arc<NetworkStack>,               // ADNL network stack
    overlays: Arc<spin::Mutex<HashMap<Arc<PrivateOverlayShortId>, Arc<AdnlOverlay>>>>, //active overlays managed by this manager
    broadcast_hops: Option<u8>, //default broadcast hops from network context
    track_private_peers: bool,  //flag to track and manage private peers
    peers_storage: Arc<PeerStorage>, //peer storage with ref counting for all overlays
}

impl AdnlOverlayManager {
    /// Create new overlay manager
    pub fn create(
        runtime_handle: tokio::runtime::Handle,
        stack: Arc<NetworkStack>,
        broadcast_hops: Option<u8>,
        track_private_peers: bool,
    ) -> Result<ConsensusOverlayManagerPtr> {
        log::trace!(
            target: LOG_TARGET,
            "Creating AdnlOverlayManager (broadcast_hops={:?}, track_private_peers={})",
            broadcast_hops,
            track_private_peers
        );

        Ok(Arc::new(Self {
            runtime_handle,
            stack,
            overlays: Arc::new(spin::Mutex::new(HashMap::new())),
            broadcast_hops,
            track_private_peers,
            peers_storage: Arc::new(PeerStorage::new()),
        }))
    }
}

impl Drop for AdnlOverlayManager {
    fn drop(&mut self) {
        let overlay_count = self.overlays.lock().len();
        log::debug!(
            target: LOG_TARGET,
            "Dropping AdnlOverlayManager ({} active overlays)",
            overlay_count
        );
    }
}

impl ConsensusOverlayManager for AdnlOverlayManager {
    /// Start new consensus overlay with given parameters
    fn start_overlay(
        &self,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
        overlay_listener: ConsensusOverlayListenerPtr,
        _log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        transport_type: OverlayTransportType,
    ) -> Result<ConsensusOverlayPtr> {
        // Searching for existing overlay
        if let Some(existing) = self.overlays.lock().get(overlay_short_id) {
            return Ok(existing.clone());
        }

        let local_id = local_validator_key.id();

        log::trace!(
            target: LOG_TARGET,
            "Starting overlay: overlay_id={}, local_id={}, nodes_count={}, transport={:?}",
            overlay_short_id,
            local_id,
            nodes.len(),
            transport_type,
        );

        // Create new overlay instance with improved initialization
        let overlay = AdnlOverlay::new(
            self.runtime_handle.clone(),
            self.stack.clone(),
            overlay_short_id.clone(),
            local_validator_key.clone(),
            overlay_listener,
            nodes,
            self.broadcast_hops, // Use broadcast_hops from manager configuration
            self.track_private_peers,
            self.peers_storage.clone(),
            transport_type,
        )?;

        // Atomic overlay addition under lock
        let overlay = {
            let mut overlays = self.overlays.lock();

            // Check if overlay already exists under lock
            if let Some(existing) = overlays.get(overlay_short_id) {
                return Ok(existing.clone());
            }

            // Add to managed overlays atomically
            overlays.insert(overlay_short_id.clone(), overlay.clone());

            log::trace!(
                target: LOG_TARGET,
                "Successfully started overlay: overlay_id={overlay_short_id}"
            );

            overlay
        };

        // Start broadcast listeners
        overlay.clone().run_wait_broadcast();

        Ok(overlay)
    }

    /// Stop existing consensus overlay
    fn stop_overlay(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        overlay: &ConsensusOverlayPtr,
    ) {
        log::trace!(target: LOG_TARGET, "Stopping overlay: overlay_id={}", overlay_short_id);

        // Try to downcast to our implementation
        if let Some(overlay_impl) = overlay.get_impl().downcast_ref::<AdnlOverlay>() {
            overlay_impl.stop();
            self.overlays.lock().remove(overlay_short_id);
        } else {
            log::warn!(
                target: LOG_TARGET,
                "Cannot downcast overlay to AdnlOverlay: overlay_id={overlay_short_id}"
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/test_adnl_overlay.rs"]
mod tests;
