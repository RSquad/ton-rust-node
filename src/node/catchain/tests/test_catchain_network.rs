/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use catchain::{
    BlockHash, BlockPayloadPtr, BlockPtr, CatchainFactory, CatchainListener, CatchainNode,
    CatchainOverlayManagerPtr, Options, PrivateKey, PublicKeyHash, QueryResponseCallback,
};
use chrono;
use consensus_common::node_test_network::NodeTestNetwork;
use rand::Rng;
use std::{
    collections::HashSet,
    fs::File,
    io::{LineWriter, Write},
    path::Path,
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::IntoBoxed;
use ton_block::{Ed25519KeyOption, UInt256};

include!("../../../common/src/test.rs");

const DB_PATH: &str = "../../target/test";
const NODE_COUNT: usize = 11;
const NUM_THREADS_PER_NODE: usize = 30;
const OVERLAY_THREADS: usize = NODE_COUNT;

fn init_catchain_test_log(test_name: &str) {
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = std::time::SystemTime::now().into();
    let out_log_file_name =
        format!("debug-catchain-{}-{}.log", test_name, datetime.format("%Y-%m-%d-%H.%M.%S"));
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
                    // Filter out specific modules from stdout (exact match) and specific message content
                    let target = record.target();
                    let should_skip_stdout = target == "adnl"
                        || target == "adnl_query"
                        || target == "telemetry"
                        || target == "overlay"
                        || message.contains("absent blocks to node");

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

struct NodeDesc {
    overlay_manager: CatchainOverlayManagerPtr,
    adnl_id: Arc<ton_block::KeyId>,
    private_key: PrivateKey,
}

#[derive(Debug)]
struct BlockDesc {
    timestamp: SystemTime,
    data: Vec<u8>,
}

impl BlockDesc {
    fn new() -> Arc<Self> {
        let timestamp = SystemTime::now();

        // Create a validator session BlockUpdate and serialize it
        let validator_session_payload = ton_api::ton::validator_session::blockupdate::BlockUpdate {
            ts: timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
            actions: vec![],
            state: rand::random::<i32>(), // Random state for variety
        }
        .into_boxed();

        // Serialize the validator session BlockUpdate
        let mut serialized_data = ton_api::ton::bytes::default();
        let mut serializer = ton_api::Serializer::new(&mut serialized_data);
        serializer.write_boxed(&validator_session_payload).unwrap();

        Arc::new(Self { timestamp, data: serialized_data })
    }
}

impl catchain::BlockPayload for BlockDesc {
    fn data(&self) -> &Vec<u8> {
        &self.data
    }

    fn get_creation_time(&self) -> SystemTime {
        self.timestamp
    }
}

struct CatchainInstance {
    latencies: spin::mutex::SpinMutex<Vec<Duration>>,
    processed_blocks: spin::mutex::SpinMutex<HashSet<BlockHash>>,
    catchain: Arc<dyn catchain::Catchain>,
    _listener: Arc<CatchainListenerImpl>,
}

impl CatchainInstance {
    fn process_blocks(&self, blocks: Vec<BlockPtr>) {
        for block in blocks {
            self.process_block_recursive(block);
        }
        self.catchain.request_new_block(SystemTime::now() + Duration::from_millis(10));
        self.catchain.processed_block(BlockDesc::new(), false);
    }

    fn process_block_recursive(&self, block: BlockPtr) {
        if !self.processed_blocks.lock().insert(block.get_hash().clone()) {
            return;
        }

        let creation_time = block.get_payload().get_creation_time();
        if let Ok(latency) = SystemTime::now().duration_since(creation_time) {
            self.latencies.lock().push(latency);
        }

        if let Some(prev) = block.get_prev() {
            self.process_block_recursive(prev);
        }

        for dep in block.get_deps() {
            self.process_block_recursive(dep.clone());
        }
    }
}

struct CatchainListenerImpl {
    instance: spin::mutex::SpinMutex<Weak<CatchainInstance>>,
}

impl Drop for CatchainListenerImpl {
    fn drop(&mut self) {
        log::info!("Dropping CatchainListenerImpl");
    }
}

impl CatchainListener for CatchainListenerImpl {
    fn preprocess_block(&self, _block: BlockPtr) {}
    fn process_blocks(&self, blocks: Vec<BlockPtr>) {
        if let Some(instance) = self.instance.lock().upgrade() {
            instance.process_blocks(blocks);
        }
    }
    fn finished_processing(&self) {}
    fn started(&self) {
        log::info!("Started");
    }
    fn process_broadcast(&self, _source_id: PublicKeyHash, _data: BlockPayloadPtr) {}
    fn process_query(
        &self,
        _source_id: PublicKeyHash,
        _data: BlockPayloadPtr,
        _callback: QueryResponseCallback,
    ) {
    }
    fn set_time(&self, _timestamp: std::time::SystemTime) {}
}

/// Generic test function that runs catchain network test with the provided node descriptions
fn test_catchain_network(
    test_name: &str,
    node_descs: Vec<NodeDesc>,
    disable_gossip: bool,
    allow_tcp_communication: bool,
) {
    init_catchain_test_log(test_name);

    let start = std::time::Instant::now();
    let num_nodes = node_descs.len();

    let mut rng = rand::thread_rng();
    let rand_name: String = rng
        .clone()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path_prefix = format!("{}/catchain_network_test_{}", DB_PATH, rand_name);

    let mut nodes = Vec::new();
    let mut private_keys = Vec::new();
    for desc in &node_descs {
        nodes.push(CatchainNode {
            adnl_id: desc.adnl_id.clone(),
            public_key: desc.private_key.clone(),
        });
        private_keys.push(desc.private_key.clone());
    }

    let session_id: UInt256 = rng.gen::<[u8; 32]>().into();
    let session_opts = Options { allow_tcp_communication, disable_gossip, ..Default::default() };

    let mut instances = vec![];
    for i in 0..num_nodes {
        let listener_impl =
            Arc::new(CatchainListenerImpl { instance: spin::mutex::SpinMutex::new(Weak::new()) });

        let listener: Weak<dyn CatchainListener + Send + Sync> =
            Arc::downgrade(&(listener_impl.clone() as Arc<dyn CatchainListener + Send + Sync>));

        let catchain = CatchainFactory::create_catchain(
            &session_opts,
            &session_id,
            nodes.clone(),
            &private_keys[i],
            db_path_prefix.clone(),
            format!("catchain_{}", i),
            false,
            node_descs[i].overlay_manager.clone(),
            listener,
        )
        .unwrap();

        let instance = Arc::new(CatchainInstance {
            latencies: spin::mutex::SpinMutex::new(Vec::new()),
            processed_blocks: spin::mutex::SpinMutex::new(HashSet::new()),
            catchain,
            _listener: listener_impl.clone(),
        });

        *listener_impl.instance.lock() = Arc::downgrade(&instance);
        instances.push(instance);
        log::info!("Created instance {}", i);
    }

    //instances[0].catchain.request_new_block(SystemTime::now());

    std::thread::sleep(std::time::Duration::from_secs(40));

    for instance in &instances {
        instance.catchain.stop();
    }

    let mut all_latencies = Vec::new();
    for instance in instances {
        let mut latencies = instance.latencies.lock();
        all_latencies.append(&mut latencies);
    }

    all_latencies.sort();

    assert!(!all_latencies.is_empty());

    if !all_latencies.is_empty() {
        let len = all_latencies.len();
        let elapsed = start.elapsed().as_secs_f64().max(1e-9);
        let count_m = (len as f64) / 1_000.0;
        let throughput_kps = len as f64 / elapsed;
        let min = all_latencies[0].as_millis();
        let median = all_latencies[len / 2].as_millis();
        let p75 = all_latencies[(len as f64 * 0.75) as usize].as_millis();
        let p95 = all_latencies[(len as f64 * 0.95) as usize].as_millis();
        let max = all_latencies[len - 1].as_millis();

        log::info!("Latencies: count={:.3}K, throughput={:.2} blocks/s:", count_m, throughput_kps);
        log::info!("- min: {}ms", min);
        log::info!("- median: {}ms", median);
        log::info!("- 75th percentile: {}ms", p75);
        log::info!("- 95th percentile: {}ms", p95);
        log::info!("- max: {}ms", max);
    }
}

#[test]
fn test_catchain_network_in_process_overlay() {
    log::info!("=== STARTING TEST: test_catchain_network_in_process_overlay ===");

    let overlay_manager = CatchainFactory::create_in_process_overlay_manager(OVERLAY_THREADS);

    // Create node descriptions with shared overlay manager and generated private keys
    let mut node_descs = Vec::new();
    for _ in 0..NODE_COUNT {
        let private_key = Ed25519KeyOption::generate().unwrap();
        let adnl_id = private_key.id().clone();
        node_descs.push(NodeDesc {
            overlay_manager: overlay_manager.clone(),
            adnl_id,
            private_key,
        });
    }

    const DISABLE_GOSSIP: bool = false;
    const ALLOW_TCP_COMMUNICATION: bool = false;

    test_catchain_network(
        "catchain-network-in-process-overlay",
        node_descs,
        DISABLE_GOSSIP,
        ALLOW_TCP_COMMUNICATION,
    );
}

#[test]
fn test_catchain_network_adnl_overlay() {
    log::info!("=== STARTING TEST: test_catchain_network_adnl_overlay ===");

    const DISABLE_GOSSIP: bool = true;
    const ALLOW_TCP_COMMUNICATION: bool = true;

    // Create ADNL test network with manual shutdown control to avoid runtime issues
    let test_network = NodeTestNetwork::create_no_auto_shutdown(
        "test_catchain_network_adnl_overlay",
        NODE_COUNT,
        NUM_THREADS_PER_NODE,
        ALLOW_TCP_COMMUNICATION,
    );

    // Create node descriptions from ADNL test network
    let mut node_descs = Vec::new();
    for i in 0..NODE_COUNT {
        let test_node = test_network.get_node(i);
        let private_key = Ed25519KeyOption::generate().unwrap();
        let adnl_id = test_node.stack.adnl.key_by_tag(test_node.adnl_tag).unwrap().id().clone();
        node_descs.push(NodeDesc {
            overlay_manager: test_node.overlay_manager.clone(),
            adnl_id,
            private_key,
        });
    }

    test_catchain_network(
        "catchain-network-adnl-overlay",
        node_descs,
        DISABLE_GOSSIP,
        ALLOW_TCP_COMMUNICATION,
    );

    // Manually shutdown ADNL nodes within runtime context to avoid "no reactor" errors
    test_network.shutdown();
}
