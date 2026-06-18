/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
include!("../../../common/src/test.rs");

use chrono;
use consensus_common::{
    node_test_network::{Node, NodeTestNetwork},
    BlockPayload, BlockPayloadPtr, ConsensusCommonFactory, ConsensusNode, ConsensusOverlayListener,
    ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListener,
    ConsensusOverlayLogReplayListenerPtr, OverlayTransportType, PublicKeyHash,
    QueryResponseCallback, Result,
};
use secrets_vault::vault_block::get_key_option_factory;
use std::{
    fs::File,
    io::{LineWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use tokio;
use ton_api::IntoBoxed;
use ton_block::KeyId;

fn init_overlay_test_log(test_name: &str) {
    let datetime: chrono::DateTime<chrono::offset::Utc> = std::time::SystemTime::now().into();
    let out_log_file_name =
        format!("debug-adnl-overlay-{}-{}.log", test_name, datetime.format("%Y-%m-%d-%H.%M.%S"));
    let logs_path = Path::new("../../target/logs");
    std::fs::create_dir_all(logs_path).expect("unable to create output log path");
    let file =
        File::create(logs_path.join(out_log_file_name)).expect("unable to create output log file");
    let file = Arc::new(Mutex::new(LineWriter::new(file)));

    env_logger::Builder::new()
        .format(move |buf, record| {
            let message = format!("{}", record.args());
            let level = record.level();
            let level_style = buf.default_level_style(level);

            let line = match record.line() {
                Some(line) => format!("({})", line),
                None => "".to_string(),
            };
            let source = format!("{}{}", record.target(), line);
            let thread_id = std::thread::current().id();

            let mut file = file.lock().unwrap();
            let log_line = format!(
                "{} [{: <5}] - {:?} - {: <45}| {}",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                level,
                thread_id,
                source,
                message
            );

            file.write_all(log_line.as_bytes()).ok();
            file.write_all(b"\n").ok();

            match record.level() {
                log::Level::Trace | log::Level::Debug => Ok(()),
                l => {
                    // Filter out adnl_query, overlay, and adnl modules from stdout
                    let target = record.target();
                    let should_skip_stdout = target.contains("adnl_query")
                        || target == "overlay"
                        || target.contains("telemetry")
                        || target == "adnl";

                    if !should_skip_stdout {
                        writeln!(
                            buf,
                            "{} [{level_style}{l: <5}{level_style:#}] - \
                            {thread_id:?} - {source: <45}| {message}",
                            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                        )?;

                        std::io::stdout().flush()
                    } else {
                        Ok(())
                    }
                }
            }
        })
        .filter_level(log::LevelFilter::Debug)
        .try_init()
        .unwrap_or_else(|_| {
            // Logger already initialized, which is fine for multiple tests
        });
}

#[derive(Debug)]
struct DummyPayload {
    data: Vec<u8>,
    sent_at: SystemTime,
}
impl BlockPayload for DummyPayload {
    fn data(&self) -> &Vec<u8> {
        &self.data
    }
    fn get_creation_time(&self) -> SystemTime {
        self.sent_at
    }
}

// Dummy log replay listener
struct DummyLogReplayListener;
impl ConsensusOverlayLogReplayListener for DummyLogReplayListener {
    fn on_time_changed(&self, _time: SystemTime) {}
}

// Simple mock listener that increments counters for received events
struct TestListener {
    local_id: PublicKeyHash,
    msg_count: Arc<AtomicUsize>,
    query_count: Arc<AtomicUsize>,
    broadcast_count: Arc<AtomicUsize>,
}

impl ConsensusOverlayListener for TestListener {
    fn on_message(&self, from: PublicKeyHash, message: &BlockPayloadPtr) {
        log::trace!(
            target: "adnl_overlay_test",
            "on_message called (local_id: {}, from: {from}, msg_size: {})",
            self.local_id,
            message.data().len()
        );
        self.msg_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_query(&self, from: PublicKeyHash, message: &BlockPayloadPtr, _cb: QueryResponseCallback) {
        log::trace!(
            target: "adnl_overlay_test",
            "on_query called (local_id: {}, from: {from}, msg_size: {})",
            self.local_id,
            message.data().len()
        );
        self.query_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_broadcast(
        &self,
        from: PublicKeyHash,
        payload: &BlockPayloadPtr,
        _source: consensus_common::BroadcastSource,
    ) {
        log::trace!(
            target: "adnl_overlay_test",
            "on_broadcast called (local_id: {}, from: {from}, msg_size: {})",
            self.local_id, payload.data().len()
        );
        self.broadcast_count.fetch_add(1, Ordering::SeqCst);
    }
}

fn make_listener(
    local_id: PublicKeyHash,
) -> (
    Arc<TestListener>,
    ConsensusOverlayListenerPtr,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let msg_count = Arc::new(AtomicUsize::new(0));
    let query_count = Arc::new(AtomicUsize::new(0));
    let broadcast_count = Arc::new(AtomicUsize::new(0));
    let listener = Arc::new(TestListener {
        local_id,
        msg_count: msg_count.clone(),
        query_count: query_count.clone(),
        broadcast_count: broadcast_count.clone(),
    });
    let weak_listener = Arc::downgrade(&listener) as ConsensusOverlayListenerPtr;
    (listener, weak_listener, msg_count, query_count, broadcast_count)
}

fn make_query_payload() -> BlockPayloadPtr {
    // Create a GetDifference query with mock delivered heights for sources
    let sources_delivered_heights: Vec<i32> = vec![0, 1, 2]; // Mock heights for 3 sources

    let get_difference_query =
        ton_api::ton::rpc::catchain::GetDifference { rt: sources_delivered_heights };

    // Serialize the query using the catchain macro
    let serialized_query = consensus_common::serialize_tl_boxed_object!(&get_difference_query);

    ConsensusCommonFactory::create_block_payload(serialized_query)
}

fn make_message_payload() -> BlockPayloadPtr {
    // Create a dummy ton::Block similar to what export_tl() creates
    let incarnation = ton_block::UInt256::default();
    let signature = ton_api::ton::bytes::default();

    // Create a minimal block dependency (prev block)
    let prev_dep = ton_api::ton::catchain::block::dep::Dep {
        src: 0,
        height: 0,
        data_hash: incarnation.clone(),
        signature: signature.clone(),
    };

    // Create block data with prev and empty deps
    let block_data = ton_api::ton::catchain::block::data::Data { prev: prev_dep, deps: vec![] };

    // Create the block
    let block = ton_api::ton::catchain::block::Block {
        incarnation: incarnation.clone(),
        src: 0,
        height: 1,
        data: block_data,
        signature: signature.clone(),
    };

    // Create BlockUpdateEvent and serialize it
    let block_update_event =
        ton_api::ton::catchain::blockupdate::BlockUpdate { block }.into_boxed();

    let mut serialized_message = ton_api::ton::bytes::default();
    let mut serializer = ton_api::Serializer::new(&mut serialized_message);
    serializer.write_boxed(&block_update_event).unwrap();

    // Append serialized validator session BlockUpdate
    let validator_session_payload = ton_api::ton::validator_session::blockupdate::BlockUpdate {
        ts: 0,
        actions: vec![],
        state: 0,
    }
    .into_boxed();

    let mut serialized_validator_session_message = ton_api::ton::bytes::default();
    let mut payload_serializer =
        ton_api::Serializer::new(&mut serialized_validator_session_message);
    payload_serializer.write_boxed(&validator_session_payload).unwrap();

    serialized_message.extend(serialized_validator_session_message.iter());

    ConsensusCommonFactory::create_block_payload(serialized_message)
}

fn make_broadcast_payload() -> BlockPayloadPtr {
    // Create a validator session candidate with dummy data
    let src = ton_block::UInt256::from([1u8; 32]); // dummy source validator id
    let root_hash = ton_block::UInt256::from([2u8; 32]); // dummy root hash
    let round = 1;

    // Create dummy block data (simulating a real block)
    let dummy_block_data = b"dummy_block_data_for_testing_purposes_with_some_length";

    // Create dummy collated data (simulating transaction collection)
    let dummy_collated_data = b"dummy_collated_transactions_data_for_performance_testing";

    let candidate = ton_api::ton::validator_session::candidate::Candidate {
        src: src.clone(),
        round,
        root_hash: root_hash.clone(),
        data: dummy_block_data.to_vec(),
        collated_data: dummy_collated_data.to_vec(),
    }
    .into_boxed();

    // Serialize the candidate
    let mut serialized_candidate = ton_api::ton::bytes::default();
    let mut serializer = ton_api::Serializer::new(&mut serialized_candidate);
    serializer.write_boxed(&candidate).unwrap();

    // Create a DummyPayload containing the serialized candidate
    Arc::new(DummyPayload { data: serialized_candidate, sent_at: SystemTime::now() })
}

fn make_query_callback() -> QueryResponseCallback {
    Box::new(|_response| {})
}

fn make_log_replay_listener() -> ConsensusOverlayLogReplayListenerPtr {
    let listener = Arc::new(DummyLogReplayListener);
    Arc::downgrade(&listener) as ConsensusOverlayLogReplayListenerPtr
}

/// Common test function that runs overlay tests with the provided overlay manager
fn run_overlay_test(
    test_nodes: Vec<Arc<Node>>,
    transport_type: OverlayTransportType,
) -> Result<()> {
    let num_nodes = test_nodes.len();

    // Create Catchain nodes with unique key pairs
    let mut consensus_nodes = Vec::new();
    for i in 0..num_nodes {
        let private_key =
            get_key_option_factory().generate().expect("Failed to generate private key");
        let public_key_bytes = private_key
            .pub_key()
            .expect("Failed to get public key")
            .try_into()
            .expect("Invalid public key length");
        let public_key = get_key_option_factory().from_public_key(public_key_bytes);
        let adnl_id =
            test_nodes[i].stack.adnl.key_by_tag(test_nodes[i].adnl_tag).unwrap().id().clone();
        consensus_nodes.push(ConsensusNode { adnl_id: adnl_id.clone(), public_key });
    }

    let random_data: [u8; 32] = rand::random();
    let overlay_short_id = KeyId::from_data(random_data);

    // Create overlays and listeners for each node
    let mut overlays = Vec::new();
    let mut listeners = Vec::new();
    let mut msg_counters = Vec::new();
    let mut query_counters = Vec::new();
    let mut broadcast_counters = Vec::new();
    for (i, consensus_node) in consensus_nodes.iter().enumerate() {
        let (listener, weak_listener, msg_count, query_count, broadcast_count) =
            make_listener(consensus_node.public_key.id().clone());
        let overlay = test_nodes[i].overlay_manager.start_overlay(
            &consensus_node.public_key,
            &overlay_short_id,
            &consensus_nodes,
            weak_listener,
            make_log_replay_listener(),
            transport_type,
            None,
        )?;
        overlays.push(overlay);
        listeners.push(listener);
        msg_counters.push(msg_count);
        query_counters.push(query_count);
        broadcast_counters.push(broadcast_count);
    }

    // Sleep before sending messages, queries, and broadcasts (to minimize absent peers)
    std::thread::sleep(std::time::Duration::from_secs(4));

    // Simulate sending messages, queries, and broadcasts using the overlay interface
    let message_payload = make_message_payload();
    let query_payload = make_query_payload();
    let broadcast_payload = make_broadcast_payload();

    const LOOP_COUNT: usize = 10;
    for _ in 0..LOOP_COUNT {
        for i in 0..num_nodes {
            let node1 = &consensus_nodes[i];
            // Send point-to-point messages and queries
            for j in 0..num_nodes {
                if i != j {
                    let node2 = &consensus_nodes[j];
                    let is_retransmission = i % 2 == 0; //test both TCP and UDP sendings
                    overlays[i].send_message(
                        &node2.adnl_id,
                        &node1.adnl_id,
                        &message_payload,
                        is_retransmission,
                    );
                    overlays[i].send_query(
                        &node2.adnl_id,
                        &node1.adnl_id,
                        "test_query",
                        std::time::Duration::from_secs(5),
                        &query_payload,
                        make_query_callback(),
                    );
                }
            }
            // Send a broadcast. Two-step broadcasts are received by all other nodes
            // (the sender does not self-deliver; this matches C++ behaviour).
            overlays[i].send_broadcast_fec_ex(
                &node1.adnl_id,
                &node1.public_key.id(),
                broadcast_payload.clone(),
                None,
            );
        }
    }

    // Wait for all messages, queries and broadcasts to be sent
    std::thread::sleep(std::time::Duration::from_secs(4));

    // Wait for all broadcasts to be delivered, with a timeout, instead of a fixed sleep.
    // Two-step broadcasts (QUIC/C++ compatible) are NOT self-delivered; each node
    // receives from the other (num_nodes - 1) senders.
    let expected_events_count = num_nodes;
    let expected_broadcast_count = num_nodes - 1;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    loop {
        if start.elapsed() > timeout {
            panic!("Timeout waiting for broadcasts to be delivered");
        }
        let all_received =
            broadcast_counters.iter().all(|c| c.load(Ordering::SeqCst) >= expected_broadcast_count);

        if all_received {
            break; // All broadcasts have been received
        }
        std::thread::sleep(std::time::Duration::from_millis(1000)); // Avoid busy-waiting

        log::info!(
            "Broadcast progress (expected {}): [{}]",
            expected_events_count,
            broadcast_counters
                .iter()
                .enumerate()
                .map(|(_i, counter)| format!("{}", counter.load(Ordering::SeqCst)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Each node should have received at least (num_nodes - 1) messages, queries and broadcasts.
    for idx in 0..num_nodes {
        assert!(
            msg_counters[idx].load(Ordering::SeqCst) >= expected_events_count,
            "Node {idx} expected at least {expected_events_count} messages, got {}",
            msg_counters[idx].load(Ordering::SeqCst)
        );
        assert!(
            query_counters[idx].load(Ordering::SeqCst) >= expected_events_count,
            "Node {idx} expected at least {expected_events_count} queries, got {}",
            query_counters[idx].load(Ordering::SeqCst)
        );
        assert!(
            broadcast_counters[idx].load(Ordering::SeqCst) >= expected_broadcast_count,
            "Node {idx} expected at least {expected_broadcast_count} broadcasts, got {}",
            broadcast_counters[idx].load(Ordering::SeqCst)
        );
    }

    // Manually stop all overlays before returning to ensure clean shutdown
    for (i, overlay) in overlays.iter().enumerate() {
        test_nodes[i].overlay_manager.stop_overlay(&overlay_short_id, overlay);
    }

    Ok(())
}

//#[ignore]
#[test]
fn test_adnl_overlay_delivery() -> Result<()> {
    init_overlay_test_log("adnl-overlay");

    log::info!("=== STARTING TEST: test_adnl_overlay_delivery ===");

    const NUM_NODES: usize = 5;
    const NUM_THREADS_PER_NODE: usize = 30;
    const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::CatchainTcp;

    let test_network =
        NodeTestNetwork::create("test_adnl_overlay_delivery", NUM_NODES, NUM_THREADS_PER_NODE);
    let result = run_overlay_test(test_network.get_nodes().clone(), TRANSPORT_TYPE);

    test_network.shutdown();

    result
}

#[test]
fn test_adnl_overlay_quic_delivery() -> Result<()> {
    init_overlay_test_log("adnl-overlay-quic");

    log::info!("=== STARTING TEST: test_adnl_overlay_quic_delivery ===");

    const NUM_NODES: usize = 5;
    const NUM_THREADS_PER_NODE: usize = 30;
    const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::SimplexQuic;

    let test_network = NodeTestNetwork::create_with_options(
        "test_adnl_overlay_quic_delivery",
        NUM_NODES,
        NUM_THREADS_PER_NODE,
        true, // auto_shutdown
        true, // is_tcp_enabled
        true, // is_quic_enabled
    );
    // Quinn QUIC requires a Tokio runtime context on the calling thread
    let _runtime_guard = test_network.get_runtime().enter();
    let result = run_overlay_test(test_network.get_nodes().clone(), TRANSPORT_TYPE);

    test_network.shutdown();

    result
}

#[test]
fn test_adnl_overlay_network_disable_toggle() -> Result<()> {
    init_overlay_test_log("adnl-overlay-toggle");
    log::info!("=== STARTING TEST: test_adnl_overlay_network_disable_toggle ===");

    const NUM_NODES: usize = 2;
    const NUM_THREADS_PER_NODE: usize = 10;
    const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::CatchainTcp;

    let test_network = NodeTestNetwork::create(
        "test_adnl_overlay_network_disable_toggle",
        NUM_NODES,
        NUM_THREADS_PER_NODE,
    );
    let test_nodes = test_network.get_nodes().clone();

    // Build consensus node identities (same pattern as run_overlay_test)
    let mut consensus_nodes = Vec::new();
    for i in 0..NUM_NODES {
        let private_key =
            get_key_option_factory().generate().expect("Failed to generate private key");
        let public_key_bytes = private_key
            .pub_key()
            .expect("Failed to get public key")
            .try_into()
            .expect("Invalid public key length");
        let public_key = get_key_option_factory().from_public_key(public_key_bytes);
        let adnl_id =
            test_nodes[i].stack.adnl.key_by_tag(test_nodes[i].adnl_tag).unwrap().id().clone();
        consensus_nodes.push(ConsensusNode { adnl_id: adnl_id.clone(), public_key });
    }

    let overlay_short_id = KeyId::from_data(rand::random());

    // Start overlays and listeners
    let mut overlays = Vec::new();
    let mut listeners = Vec::new(); // keep Arc listeners alive (overlay stores Weak)
    let mut msg_counters = Vec::new();
    for (i, consensus_node) in consensus_nodes.iter().enumerate() {
        let (listener, weak_listener, msg_count, _query_count, _broadcast_count) =
            make_listener(consensus_node.public_key.id().clone());
        let overlay = test_nodes[i].overlay_manager.start_overlay(
            &consensus_node.public_key,
            &overlay_short_id,
            &consensus_nodes,
            weak_listener,
            make_log_replay_listener(),
            TRANSPORT_TYPE,
            None,
        )?;
        overlays.push(overlay);
        listeners.push(listener);
        msg_counters.push(msg_count);
    }

    // Give the overlay some time to establish peers to avoid flakiness.
    std::thread::sleep(Duration::from_secs(1));

    let node0 = &consensus_nodes[0];
    let node1 = &consensus_nodes[1];

    // Helper: wait until msg counter increments (or timeout).
    let wait_increment = |counter: &Arc<AtomicUsize>, before: usize| -> Result<()> {
        let start = Instant::now();
        let timeout = Duration::from_secs(5);
        while start.elapsed() < timeout {
            let now = counter.load(Ordering::SeqCst);
            if now > before {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(ton_block::error!(
            "Timeout waiting for message delivery: before={}, now={}",
            before,
            counter.load(Ordering::SeqCst)
        ))
    };

    // 1) Baseline: message should be delivered when both nodes enabled.
    let before = msg_counters[1].load(Ordering::SeqCst);
    overlays[0].send_message(&node1.adnl_id, &node0.adnl_id, &make_message_payload(), false);
    wait_increment(&msg_counters[1], before)?;

    // 2) Disable node1 (inbound drop): message should NOT be delivered to node1.
    test_network.disable_node_network(1);
    let before_disabled_in = msg_counters[1].load(Ordering::SeqCst);
    overlays[0].send_message(&node1.adnl_id, &node0.adnl_id, &make_message_payload(), false);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        msg_counters[1].load(Ordering::SeqCst),
        before_disabled_in,
        "expected node1 to drop inbound messages while network disabled"
    );

    // 3) Enable node1, disable node0 (outbound drop): message should NOT be delivered.
    test_network.enable_node_network(1);
    test_network.disable_node_network(0);
    let before_disabled_out = msg_counters[1].load(Ordering::SeqCst);
    overlays[0].send_message(&node1.adnl_id, &node0.adnl_id, &make_message_payload(), false);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        msg_counters[1].load(Ordering::SeqCst),
        before_disabled_out,
        "expected node0 to drop outbound messages while network disabled"
    );

    // 4) Re-enable node0: message should be delivered again.
    test_network.enable_node_network(0);
    let before_reenabled = msg_counters[1].load(Ordering::SeqCst);
    overlays[0].send_message(&node1.adnl_id, &node0.adnl_id, &make_message_payload(), false);
    wait_increment(&msg_counters[1], before_reenabled)?;

    // Stop overlays
    for (i, overlay) in overlays.iter().enumerate() {
        test_nodes[i].overlay_manager.stop_overlay(&overlay_short_id, overlay);
    }

    test_network.shutdown();
    Ok(())
}

//
// Performance Test
//

const CALLBACK_SLEEP_TIME: Duration = Duration::from_micros(10);

struct PerformanceTestListener {
    msg_latencies: Arc<Mutex<Vec<Duration>>>,
    query_latencies: Arc<Mutex<Vec<Duration>>>,
    broadcast_latencies: Arc<Mutex<Vec<Duration>>>,
}

impl ConsensusOverlayListener for PerformanceTestListener {
    fn on_message(&self, _from: PublicKeyHash, message: &BlockPayloadPtr) {
        let latency =
            SystemTime::now().duration_since(message.get_creation_time()).unwrap_or_default();
        self.msg_latencies.lock().unwrap().push(latency);
        std::thread::sleep(CALLBACK_SLEEP_TIME);
    }

    fn on_query(&self, _from: PublicKeyHash, query: &BlockPayloadPtr, _cb: QueryResponseCallback) {
        let latency =
            SystemTime::now().duration_since(query.get_creation_time()).unwrap_or_default();
        self.query_latencies.lock().unwrap().push(latency);
        std::thread::sleep(CALLBACK_SLEEP_TIME);
    }

    fn on_broadcast(
        &self,
        _from: PublicKeyHash,
        payload: &BlockPayloadPtr,
        _source: consensus_common::BroadcastSource,
    ) {
        let latency =
            SystemTime::now().duration_since(payload.get_creation_time()).unwrap_or_default();
        self.broadcast_latencies.lock().unwrap().push(latency);
        std::thread::sleep(CALLBACK_SLEEP_TIME);
    }
}

fn make_performance_listener() -> (
    Arc<PerformanceTestListener>,
    ConsensusOverlayListenerPtr,
    Arc<Mutex<Vec<Duration>>>,
    Arc<Mutex<Vec<Duration>>>,
    Arc<Mutex<Vec<Duration>>>,
) {
    let msg_latencies = Arc::new(Mutex::new(Vec::new()));
    let query_latencies = Arc::new(Mutex::new(Vec::new()));
    let broadcast_latencies = Arc::new(Mutex::new(Vec::new()));

    let listener = Arc::new(PerformanceTestListener {
        msg_latencies: msg_latencies.clone(),
        query_latencies: query_latencies.clone(),
        broadcast_latencies: broadcast_latencies.clone(),
    });

    let weak_listener = Arc::downgrade(&listener) as ConsensusOverlayListenerPtr;
    (listener, weak_listener, msg_latencies, query_latencies, broadcast_latencies)
}

fn print_latency_stats(name: &str, latencies: &mut Vec<Duration>) {
    if latencies.is_empty() {
        println!("  {}: No data.", name);
        return;
    }

    latencies.sort();

    let count = latencies.len();
    let min = latencies.first().unwrap();
    let max = latencies.last().unwrap();
    let median = latencies[count / 2];
    let total_latency: Duration = latencies.iter().sum();
    let avg = total_latency / count as u32;

    println!(
        "  {}: count={}, min={:?}, max={:?}, median={:?}, avg={:?}",
        name, count, min, max, median, avg
    );
}

/// Common performance test function that runs with the provided ADNL test nodes
fn run_adnl_overlay_performance_test(
    test_nodes: Vec<Arc<Node>>,
    transport_type: OverlayTransportType,
) -> Result<()> {
    let num_nodes = test_nodes.len();
    const TEST_DURATION: Duration = Duration::from_secs(5);
    const SLEEP_TIME: Duration = Duration::from_micros(1000);

    // Create Catchain nodes with unique key pairs
    let mut consensus_nodes = Vec::new();
    for i in 0..num_nodes {
        let private_key =
            get_key_option_factory().generate().expect("Failed to generate private key");
        let public_key_bytes = private_key
            .pub_key()
            .expect("Failed to get public key")
            .try_into()
            .expect("Invalid public key length");
        let public_key = get_key_option_factory().from_public_key(public_key_bytes);
        let adnl_id =
            test_nodes[i].stack.adnl.key_by_tag(test_nodes[i].adnl_tag).unwrap().id().clone();
        consensus_nodes.push(ConsensusNode { adnl_id: adnl_id.clone(), public_key });
    }

    let random_data: [u8; 32] = rand::random();
    let overlay_short_id = KeyId::from_data(random_data);

    // Create overlays and listeners for each node
    let mut overlays = Vec::new();
    let mut all_listeners = Vec::new();
    for (i, consensus_node) in consensus_nodes.iter().enumerate() {
        let (listener, weak_listener, ..) = make_performance_listener();
        let overlay = test_nodes[i].overlay_manager.start_overlay(
            &consensus_node.public_key,
            &overlay_short_id,
            &consensus_nodes,
            weak_listener,
            make_log_replay_listener(),
            transport_type,
            None,
        )?;
        overlays.push(overlay);
        all_listeners.push(listener);
    }

    // Sleep before starting performance test (to minimize absent peers)
    std::thread::sleep(std::time::Duration::from_secs(2));

    let overlays_arc = Arc::new(overlays);
    let consensus_nodes_arc = Arc::new(consensus_nodes);
    let mut handles = vec![];

    // Spawn threads to send data
    for i in 0..num_nodes {
        let overlays = Arc::clone(&overlays_arc);
        let consensus_nodes = Arc::clone(&consensus_nodes_arc);

        let handle = thread::spawn(move || {
            let start = Instant::now();
            let mut messages_sent = 0;
            let mut queries_sent = 0;
            let mut broadcasts_sent = 0;

            while start.elapsed() < TEST_DURATION {
                for j in 0..num_nodes {
                    if i == j {
                        continue;
                    }
                    let node1 = &consensus_nodes[i];
                    let node2 = &consensus_nodes[j];
                    let is_retransmission = i % 2 == 0; //test both TCP and UDP sendings
                    overlays[i].send_message(
                        &node2.adnl_id,
                        &node1.adnl_id,
                        &make_message_payload(),
                        is_retransmission,
                    );
                    messages_sent += 1;

                    // Send 1 query for every QUERY_INTERVAL messages
                    const QUERY_INTERVAL: usize = 10;
                    if messages_sent % QUERY_INTERVAL == 0 {
                        overlays[i].send_query(
                            &node2.adnl_id,
                            &node1.adnl_id,
                            "perf_test",
                            Duration::from_secs(1),
                            &make_query_payload(),
                            make_query_callback(),
                        );
                        queries_sent += 1;
                    }
                }
                let node1 = &consensus_nodes[i];
                overlays[i].send_broadcast_fec_ex(
                    &node1.adnl_id,
                    &node1.public_key.id(),
                    make_broadcast_payload(),
                    None,
                );
                broadcasts_sent += 1;
                thread::sleep(SLEEP_TIME);
            }
            (messages_sent, queries_sent, broadcasts_sent)
        });
        handles.push(handle);
    }

    // Collect send counts
    let mut total_messages_sent = 0;
    let mut total_queries_sent = 0;
    let mut total_broadcasts_sent = 0;
    for handle in handles {
        let (m, q, b) = handle.join().unwrap();
        total_messages_sent += m;
        total_queries_sent += q;
        total_broadcasts_sent += b;
    }

    // Allow time for all messages to be processed by the background thread with countdown
    const WAIT_DURATION: Duration = Duration::from_secs(30);
    const COUNTDOWN_INTERVAL: Duration = Duration::from_secs(5);

    let start_wait = Instant::now();
    let mut next_countdown = start_wait + COUNTDOWN_INTERVAL;

    println!("\nWaiting for message processing to complete...");

    while start_wait.elapsed() < WAIT_DURATION {
        if Instant::now() >= next_countdown {
            let remaining = WAIT_DURATION - start_wait.elapsed();
            println!(
                "Still waiting for {:?} seconds for message processing to complete...",
                remaining.as_secs()
            );
            next_countdown = Instant::now() + COUNTDOWN_INTERVAL;
        }
        thread::sleep(Duration::from_millis(100));
    }

    println!("Message processing wait completed. Stopping overlays...");

    // Manually stop all overlays before collecting statistics
    for (i, overlay) in overlays_arc.iter().enumerate() {
        test_nodes[i].overlay_manager.stop_overlay(&overlay_short_id, overlay);
    }

    println!("All overlays stopped. Collecting statistics...");

    // Collect latencies and compute received counts
    let mut all_msg_latencies = Vec::new();
    let mut all_query_latencies = Vec::new();
    let mut all_broadcast_latencies = Vec::new();

    // Aggregate received counts from all nodes
    let mut total_messages_received = 0;
    let mut total_queries_received = 0;
    let mut total_broadcasts_received = 0;

    println!("\n--- ADNL Overlay Performance Report ---");
    println!("Test duration: {:?}", TEST_DURATION);
    println!("Nodes: {}", num_nodes);

    // Dump latencies independently for each node and collect received counts
    println!("\n=== Individual Node Latency Reports ===");
    for (node_idx, listener) in all_listeners.iter().enumerate() {
        println!("\n--- Node {} Latencies ---", node_idx);

        let mut node_msg_latencies: Vec<Duration> = listener.msg_latencies.lock().unwrap().clone();
        let mut node_query_latencies: Vec<Duration> =
            listener.query_latencies.lock().unwrap().clone();
        let mut node_broadcast_latencies: Vec<Duration> =
            listener.broadcast_latencies.lock().unwrap().clone();

        // Count received packets for this node
        let node_messages_received = node_msg_latencies.len();
        let node_queries_received = node_query_latencies.len();
        let node_broadcasts_received = node_broadcast_latencies.len();

        // Print individual node stats
        println!(
            "  Received: {} messages, {} queries, {} broadcasts",
            node_messages_received, node_queries_received, node_broadcasts_received
        );
        print_latency_stats("Messages", &mut node_msg_latencies);
        print_latency_stats("Queries", &mut node_query_latencies);
        print_latency_stats("Broadcasts", &mut node_broadcast_latencies);

        // Aggregate for overall stats
        all_msg_latencies.extend(node_msg_latencies.iter());
        all_query_latencies.extend(node_query_latencies.iter());
        all_broadcast_latencies.extend(node_broadcast_latencies.iter());

        // Add to total received counts
        total_messages_received += node_messages_received;
        total_queries_received += node_queries_received;
        total_broadcasts_received += node_broadcasts_received;
    }

    // Calculate loss rates
    let message_loss_rate = if total_messages_sent > 0 {
        100.0 * (1.0 - (total_messages_received as f64 / total_messages_sent as f64))
    } else {
        0.0
    };
    let query_loss_rate = if total_queries_sent > 0 {
        100.0 * (1.0 - (total_queries_received as f64 / total_queries_sent as f64))
    } else {
        0.0
    };

    // Special case for broadcasts: we expect each node to receive broadcasts from all other nodes, so we divide by the number of nodes
    total_broadcasts_received = total_broadcasts_received / num_nodes;

    let broadcast_loss_rate = if total_broadcasts_sent > 0 {
        100.0 * (1.0 - (total_broadcasts_received as f64 / total_broadcasts_sent as f64))
    } else {
        0.0
    };

    println!("\n=== Throughput Summary ===");
    println!(
        "  Messages:   {} sent ({:.2}/sec) -> {} received ({:.2}/sec) | Loss: {:.2}%",
        total_messages_sent,
        total_messages_sent as f64 / TEST_DURATION.as_secs_f64(),
        total_messages_received,
        total_messages_received as f64 / TEST_DURATION.as_secs_f64(),
        message_loss_rate
    );
    println!(
        "  Queries:    {} sent ({:.2}/sec) -> {} received ({:.2}/sec) | Loss: {:.2}%",
        total_queries_sent,
        total_queries_sent as f64 / TEST_DURATION.as_secs_f64(),
        total_queries_received,
        total_queries_received as f64 / TEST_DURATION.as_secs_f64(),
        query_loss_rate
    );
    println!(
        "  Broadcasts: {} sent ({:.2}/sec) -> {} received ({:.2}/sec) | Loss: {:.2}%",
        total_broadcasts_sent,
        total_broadcasts_sent as f64 / TEST_DURATION.as_secs_f64(),
        total_broadcasts_received,
        total_broadcasts_received as f64 / TEST_DURATION.as_secs_f64(),
        broadcast_loss_rate
    );

    // Assert that we received non-zero messages, queries, and broadcasts
    assert!(
        total_messages_received > 0,
        "Performance test failed: No messages received! Expected > 0 messages but got {}",
        total_messages_received
    );
    assert!(
        total_queries_received > 0,
        "Performance test failed: No queries received! Expected > 0 queries but got {}",
        total_queries_received
    );
    assert!(
        total_broadcasts_received > 0,
        "Performance test failed: No broadcasts received! Expected > 0 broadcasts but got {}",
        total_broadcasts_received
    );

    // Additional sanity checks - ensure we received a reasonable number of messages
    // We expect at least some minimum delivery rate
    const MIN_EXPECTED_DELIVERY_RATE: f64 = 0.5;
    let min_expected_messages =
        std::cmp::max(1, total_messages_sent * MIN_EXPECTED_DELIVERY_RATE as usize);
    let min_expected_queries =
        std::cmp::max(1, total_queries_sent * MIN_EXPECTED_DELIVERY_RATE as usize);
    let min_expected_broadcasts =
        std::cmp::max(1, total_broadcasts_sent * MIN_EXPECTED_DELIVERY_RATE as usize);

    assert!(
        total_messages_received >= min_expected_messages,
        "Performance test failed: Too few messages received! Expected at least {} from total {} messages but got {}. Loss rate: {:.2}%",
        min_expected_messages,
        total_messages_sent,
        total_messages_received,
        message_loss_rate
    );
    assert!(
        total_queries_received >= min_expected_queries,
        "Performance test failed: Too few queries received! Expected at least {} from total {} queries but got {}. Loss rate: {:.2}%",
        min_expected_queries,
        total_queries_sent,
        total_queries_received,
        query_loss_rate
    );
    assert!(
        total_broadcasts_received >= min_expected_broadcasts,
        "Performance test failed: Too few broadcasts received! Expected at least {} broadcasts from total {} sent but got {}. Loss rate: {:.2}%",
        min_expected_broadcasts,
        total_broadcasts_sent,
        total_broadcasts_received,
        broadcast_loss_rate
    );

    // Overall aggregated stats for comparison
    println!("\n=== Overall Aggregated Latency Stats ===");
    print_latency_stats("All Messages", &mut all_msg_latencies);
    print_latency_stats("All Queries", &mut all_query_latencies);
    print_latency_stats("All Broadcasts", &mut all_broadcast_latencies);

    Ok(())
}

//#[ignore]
#[test]
fn test_adnl_overlay_performance() -> Result<()> {
    init_overlay_test_log("adnl-overlay-performance");

    log::info!("=== STARTING TEST: test_adnl_overlay_performance ===");

    const NUM_NODES: usize = 11;
    const NUM_THREADS_PER_NODE: usize = 30;
    const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::CatchainTcp;

    let test_network =
        NodeTestNetwork::create("test_adnl_overlay_performance", NUM_NODES, NUM_THREADS_PER_NODE);
    let result =
        run_adnl_overlay_performance_test(test_network.get_nodes().clone(), TRANSPORT_TYPE);

    result
}
