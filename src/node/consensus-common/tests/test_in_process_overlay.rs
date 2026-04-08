/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use chrono;
use consensus_common::{
    BlockPayload, BlockPayloadPtr, ConsensusCommonFactory, ConsensusNode, ConsensusOverlayListener,
    ConsensusOverlayListenerPtr, ConsensusOverlayLogReplayListener,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManagerPtr, OverlayTransportType,
    PublicKeyHash, QueryResponseCallback,
};
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
use ton_block::{KeyId, Result};

include!("../../../common/src/test.rs");

fn init_overlay_test_log(test_name: &str) {
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = std::time::SystemTime::now().into();
    let out_log_file_name =
        format!("debug-{}-{}.log", test_name, datetime.format("%Y-%m-%d-%H.%M.%S"));
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
                    writeln!(
                        buf,
                        "{} [{level_style}{l: <5}{level_style:#}] - \
                        {thread_id:?} - {source: <45}| {message}",
                        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                    )?;

                    std::io::stdout().flush()
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
            target: "in_process_overlay_test",
            "on_message called (local_id: {}, from: {from}, msg_size: {})",
            self.local_id,
            message.data().len()
        );
        self.msg_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_query(&self, from: PublicKeyHash, message: &BlockPayloadPtr, _cb: QueryResponseCallback) {
        log::trace!(
            target: "in_process_overlay_test",
            "on_query called (local_id: {}, from: {from}, msg_size: {})",
            self.local_id,
            message.data().len()
        );
        self.query_count.fetch_add(1, Ordering::SeqCst);
    }
    fn on_broadcast(&self, from: PublicKeyHash, payload: &BlockPayloadPtr) {
        log::trace!(
            target: "in_process_overlay_test",
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

fn make_payload() -> BlockPayloadPtr {
    Arc::new(DummyPayload { data: vec![1, 2, 3, 4], sent_at: SystemTime::now() })
}

fn make_query_callback() -> QueryResponseCallback {
    Box::new(|_response| {})
}

fn make_log_replay_listener() -> ConsensusOverlayLogReplayListenerPtr {
    let listener = Arc::new(DummyLogReplayListener);
    Arc::downgrade(&listener) as ConsensusOverlayLogReplayListenerPtr
}

/// Common test function that runs overlay tests with the provided overlay manager
fn run_overlay_test(manager: ConsensusOverlayManagerPtr) -> Result<()> {
    const NUM_NODES: usize = 3;

    // Create 3 nodes with unique key pairs
    let mut node_ids = Vec::new();
    let mut nodes = Vec::new();
    for _ in 0..NUM_NODES {
        let private_key =
            ton_block::Ed25519KeyOption::generate().expect("Failed to generate private key");
        let public_key_bytes = private_key
            .pub_key()
            .expect("Failed to get public key")
            .try_into()
            .expect("Invalid public key length");
        let public_key = ton_block::Ed25519KeyOption::from_public_key(public_key_bytes);
        let adnl_id = public_key.id();
        node_ids.push(adnl_id.clone());
        nodes.push(ConsensusNode { adnl_id: adnl_id.clone(), public_key });
    }

    let random_data: [u8; 32] = rand::random();
    let overlay_short_id = KeyId::from_data(random_data);

    // Create overlays and listeners for each node
    let mut overlays = Vec::new();
    let mut listeners = Vec::new();
    let mut msg_counters = Vec::new();
    let mut query_counters = Vec::new();
    let mut broadcast_counters = Vec::new();
    for node in &nodes {
        let (listener, weak_listener, msg_count, query_count, broadcast_count) =
            make_listener(node.public_key.id().clone());
        const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::Catchain;
        let overlay = manager.start_overlay(
            &node.public_key,
            &overlay_short_id,
            &nodes,
            weak_listener,
            make_log_replay_listener(),
            TRANSPORT_TYPE,
        )?;
        overlays.push(overlay);
        listeners.push(listener);
        msg_counters.push(msg_count);
        query_counters.push(query_count);
        broadcast_counters.push(broadcast_count);
    }

    // Simulate sending messages, queries, and broadcasts using the overlay interface
    for i in 0..NUM_NODES {
        // Send point-to-point messages and queries
        for j in 0..NUM_NODES {
            if i != j {
                let is_retransmission = false;
                overlays[i].send_message(
                    &node_ids[j],
                    &node_ids[i],
                    &make_payload(),
                    is_retransmission,
                );
                overlays[i].send_query(
                    &node_ids[j],
                    &node_ids[i],
                    "test_query",
                    std::time::Duration::from_secs(5),
                    &make_payload(),
                    make_query_callback(),
                );
            }
        }
        // Send a broadcast. This will be received by all, including the sender.
        overlays[i].send_broadcast_fec_ex(&node_ids[i], &node_ids[i], make_payload());
    }

    // Wait for all broadcasts to be delivered, with a timeout, instead of a fixed sleep
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    loop {
        if start.elapsed() > timeout {
            panic!("Timeout waiting for broadcasts to be delivered");
        }
        let all_received = broadcast_counters.iter().all(|c| c.load(Ordering::SeqCst) == NUM_NODES);

        if all_received {
            break; // All broadcasts have been received
        }
        std::thread::sleep(std::time::Duration::from_millis(10)); // Avoid busy-waiting
    }

    // Each node should have received (NUM_NODES - 1) messages and queries.
    // Each node sends one broadcast, and every broadcast is received by all NUM_NODES nodes.
    // So, each node should receive NUM_NODES broadcasts.
    for idx in 0..NUM_NODES {
        assert_eq!(
            msg_counters[idx].load(Ordering::SeqCst),
            NUM_NODES - 1,
            "Node {} msg count",
            idx
        );
        assert_eq!(
            query_counters[idx].load(Ordering::SeqCst),
            NUM_NODES - 1,
            "Node {} query count",
            idx
        );
        assert_eq!(
            broadcast_counters[idx].load(Ordering::SeqCst),
            NUM_NODES,
            "Node {} broadcast count",
            idx
        );
    }

    Ok(())
}

#[test]
fn test_in_process_overlay() -> Result<()> {
    // Init logger with file output (similar to accelerated consensus test)
    init_overlay_test_log("in-process-overlay");

    log::info!("=== STARTING TEST: test_in_process_overlay ===");

    // Create overlay manager using the public factory
    let manager = ConsensusCommonFactory::create_in_process_overlay_manager(1);

    run_overlay_test(manager)
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

    fn on_broadcast(&self, _from: PublicKeyHash, payload: &BlockPayloadPtr) {
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

/// Common performance test function that runs with the provided overlay manager
fn run_overlay_performance_test(manager: ConsensusOverlayManagerPtr) -> Result<()> {
    const NUM_NODES: usize = 11;
    const TEST_DURATION: Duration = Duration::from_secs(5);
    const SLEEP_TIME: Duration = Duration::from_micros(100);

    // Setup nodes
    let mut node_ids = Vec::new();
    let mut nodes = Vec::new();
    for _ in 0..NUM_NODES {
        let private_key =
            ton_block::Ed25519KeyOption::generate().expect("Failed to generate private key");
        let public_key_bytes = private_key
            .pub_key()
            .expect("Failed to get public key")
            .try_into()
            .expect("Invalid public key length");
        let public_key = ton_block::Ed25519KeyOption::from_public_key(public_key_bytes);
        let adnl_id = public_key.id();
        node_ids.push(adnl_id.clone());
        nodes.push(ConsensusNode { adnl_id: adnl_id.clone(), public_key });
    }

    let random_data: [u8; 32] = rand::random();
    let overlay_short_id = KeyId::from_data(random_data);

    // Create overlays and listeners
    let mut overlays = Vec::new();
    let mut all_listeners = Vec::new();
    for node in &nodes {
        let (listener, weak_listener, ..) = make_performance_listener();
        const TRANSPORT_TYPE: OverlayTransportType = OverlayTransportType::Catchain;
        let overlay = manager.start_overlay(
            &node.public_key,
            &overlay_short_id,
            &nodes,
            weak_listener,
            make_log_replay_listener(),
            TRANSPORT_TYPE,
        )?;
        overlays.push(overlay);
        all_listeners.push(listener);
    }

    let overlays_arc = Arc::new(overlays);
    let node_ids_arc = Arc::new(node_ids);
    let mut handles = vec![];

    // Spawn threads to send data
    for i in 0..NUM_NODES {
        let overlays = Arc::clone(&overlays_arc);
        let node_ids = Arc::clone(&node_ids_arc);

        let handle = thread::spawn(move || {
            let start = Instant::now();
            let mut messages_sent = 0;
            let mut queries_sent = 0;
            let mut broadcasts_sent = 0;

            while start.elapsed() < TEST_DURATION {
                for j in 0..NUM_NODES {
                    if i == j {
                        continue;
                    }
                    let is_retransmission = false;
                    overlays[i].send_message(
                        &node_ids[j],
                        &node_ids[i],
                        &make_payload(),
                        is_retransmission,
                    );
                    messages_sent += 1;
                    overlays[i].send_query(
                        &node_ids[j],
                        &node_ids[i],
                        "perf_test",
                        Duration::from_secs(1),
                        &make_payload(),
                        make_query_callback(),
                    );
                    queries_sent += 1;
                }
                overlays[i].send_broadcast_fec_ex(&node_ids[i], &node_ids[i], make_payload());
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

    // Allow time for all messages to be processed by the background thread
    thread::sleep(Duration::from_secs(2));

    // Aggregate all latencies
    let mut all_msg_latencies = Vec::new();
    let mut all_query_latencies = Vec::new();
    let mut all_broadcast_latencies = Vec::new();

    for listener in all_listeners {
        all_msg_latencies.extend(listener.msg_latencies.lock().unwrap().iter());
        all_query_latencies.extend(listener.query_latencies.lock().unwrap().iter());
        all_broadcast_latencies.extend(listener.broadcast_latencies.lock().unwrap().iter());
    }

    // Report results
    println!("\n--- In-Process Overlay Performance Report ---");
    println!("Test duration: {:?}", TEST_DURATION);
    println!("Nodes: {}", NUM_NODES);
    println!("\nThroughput:");
    println!(
        "  Messages:   {} sent ({:.2}/sec)",
        total_messages_sent,
        total_messages_sent as f64 / TEST_DURATION.as_secs_f64()
    );
    println!(
        "  Queries:    {} sent ({:.2}/sec)",
        total_queries_sent,
        total_queries_sent as f64 / TEST_DURATION.as_secs_f64()
    );
    println!(
        "  Broadcasts: {} sent ({:.2}/sec)",
        total_broadcasts_sent,
        total_broadcasts_sent as f64 / TEST_DURATION.as_secs_f64()
    );

    println!("\nLatency:");
    print_latency_stats("Messages", &mut all_msg_latencies);
    print_latency_stats("Queries", &mut all_query_latencies);
    print_latency_stats("Broadcasts", &mut all_broadcast_latencies);

    Ok(())
}

#[test]
//#[ignore]
fn test_in_process_overlay_performance() -> Result<()> {
    // Init logger with file output (similar to accelerated consensus test)
    init_overlay_test_log("in-process-overlay-performance");

    log::info!("=== STARTING TEST: test_in_process_overlay_performance ===");

    const NUM_THREADS: usize = 16;
    let manager = ConsensusCommonFactory::create_in_process_overlay_manager(NUM_THREADS);

    run_overlay_performance_test(manager)
}
