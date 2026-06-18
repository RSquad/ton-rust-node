/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use catchain::*;
use rand::Rng;
use std::{
    path::Path,
    sync::{mpsc::channel, Arc, Weak},
    thread,
    time::SystemTime,
};
use ton_block::{Ed25519KeyOption, UInt256, ZeroizingBytes};

include!("../../../common/src/test.rs");

const DB_PATH: &str = "../../target/test";
const NODE_COUNT: usize = 11;
const TEST_ATTEMPTS: usize = 20;
const HEIGHT_THRESHOLD: u64 = 40;
struct CatchainInstance {
    catchain: CatchainPtr,
    _nodes: Vec<CatchainNode>,
    _session_id: UInt256,
    local_idx: usize,
    _listener: Arc<dyn CatchainListener + Send + Sync>,
    value: u64,
    height: u64,
    block_payloads: Vec<u64>,
    prev_values: Vec<u64>,
}

impl CatchainInstance {
    fn value(&self) -> u64 {
        self.value
    }

    fn height(&self) -> u64 {
        self.height
    }

    fn new(
        session_id: UInt256,
        local_idx: usize,
        nodes: Vec<CatchainNode>,
        overlay_manager: CatchainOverlayManagerPtr,
    ) -> Arc<spin::mutex::SpinMutex<CatchainInstance>> {
        let listener = Arc::new(CatchainInstanceListener {
            instance: spin::mutex::SpinMutex::new(Weak::new()),
        });

        const DB_NAME: &str = "catchains_test_catchain_forks";
        let db_path = Path::new(DB_PATH).join(DB_NAME).display().to_string();
        let db_suffix = format!("catchain_{}", local_idx);
        let allow_unsafe_self_blocks_resync = false;

        let mut options = Options::default();
        options.debug_disable_db = true;
        let catchain_listener: Arc<dyn CatchainListener + Send + Sync> = listener.clone();
        let catchain = CatchainFactory::create_catchain(
            &options,
            &session_id,
            nodes.clone(),
            &nodes[local_idx].public_key,
            db_path,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            overlay_manager,
            Arc::downgrade(&catchain_listener),
        )
        .unwrap();

        let instance = Arc::new(spin::mutex::SpinMutex::new(CatchainInstance {
            catchain,
            _listener: listener.clone(),
            _nodes: nodes,
            _session_id: session_id,
            local_idx,
            value: 0,
            height: 0,
            block_payloads: Vec::new(),
            prev_values: Vec::new(),
        }));

        *listener.instance.lock() = Arc::downgrade(&instance);

        instance
    }

    fn stop_async(&self) {
        self.catchain.stop_async();
    }

    fn set_block_payload(&mut self, block: BlockPtr, payload: u64) {
        let extra_id = block.get_extra_id() as usize;

        if extra_id >= self.block_payloads.len() {
            self.block_payloads.resize(extra_id + 1, 0);
        }

        self.block_payloads[extra_id] = payload;
    }

    fn get_block_payload(&self, block: &BlockPtr) -> u64 {
        let extra_id = block.get_extra_id() as usize;

        if extra_id >= self.block_payloads.len() {
            return 0;
        }

        self.block_payloads[extra_id]
    }

    fn preprocess_block(&mut self, block: BlockPtr) {
        //info!("CatchainInstance::preprocess_block: {:?}", block);

        let mut sum = 0;

        if let Some(prev_block) = block.get_prev() {
            let payload = self.get_block_payload(&prev_block);
            sum = std::cmp::max(sum, payload);
        }

        for dep in block.get_deps() {
            let payload = self.get_block_payload(dep);
            sum = std::cmp::max(sum, payload);
        }

        let payload = block.get_payload();

        if payload.data().len() > 0 {
            assert!(payload.data().len() == 16);

            let mut x = [0u64; 2];
            let data = payload.data();
            x[0] = u64::from_le_bytes(data[0..8].try_into().expect("Slice with incorrect length"));
            x[1] = u64::from_le_bytes(data[8..16].try_into().expect("Slice with incorrect length"));

            sum = std::cmp::max(sum, x[0]);

            if sum != x[1] {
                log::warn!(
                    "CatchainInstance::preprocess_block: sum {sum} != x[1] {} (x[0] = {})",
                    x[1],
                    x[0]
                );
            }
        } else {
            assert!(block.get_deps().len() == 0);
        }

        self.set_block_payload(block, sum);
    }

    fn process_blocks(&mut self, blocks: Vec<BlockPtr>) {
        //info!("CatchainInstance::process_blocks: {:?}", blocks);

        let mut sum = self.value;

        for block in blocks {
            let payload = self.get_block_payload(&block);

            sum = std::cmp::max(sum, payload);
        }

        let value = rand::thread_rng().gen::<u64>();
        sum = std::cmp::max(sum, value);

        let x: [u64; 2] = [value, sum];

        self.value = sum;

        // create Vec<u8> from x
        let mut data = Vec::new();
        data.extend_from_slice(&x[0].to_le_bytes());
        data.extend_from_slice(&x[1].to_le_bytes());

        self.catchain.processed_block(CatchainFactory::create_block_payload(data.into()), false);

        self.height += 1;
        self.prev_values.push(self.value);

        self.catchain.request_new_block(SystemTime::now() + std::time::Duration::from_millis(200));
    }

    fn create_fork(&mut self) {
        let height = self.height - 1;

        log::warn!("Creating fork, source_id={}, height={height}", self.local_idx);

        let sum = self.prev_values[height as usize] + 1;

        let mut x = [0u64; 2];
        x[0] = sum + 1;
        x[1] = sum + 1;

        let mut data = Vec::new();
        data.extend_from_slice(&x[0].to_le_bytes());
        data.extend_from_slice(&x[1].to_le_bytes());

        self.catchain.debug_add_fork(
            CatchainFactory::create_block_payload(data.into()),
            (height + 1) as BlockHeight,
        );
    }
}

struct CatchainInstanceListener {
    instance: spin::mutex::SpinMutex<Weak<spin::mutex::SpinMutex<CatchainInstance>>>,
}

impl CatchainListener for CatchainInstanceListener {
    fn preprocess_block(&self, block: BlockPtr) {
        //info!("CatchainInstanceListener::preprocess_block: {:?}", block);

        if let Some(instance) = self.instance.lock().upgrade() {
            let mut instance = instance.lock();
            instance.preprocess_block(block);
        }
    }

    fn process_blocks(&self, blocks: Vec<BlockPtr>) {
        //info!("DummyCatchainListener::process_blocks: {:?}", blocks);

        if let Some(instance) = self.instance.lock().upgrade() {
            let mut instance = instance.lock();
            instance.process_blocks(blocks);
        }
    }

    fn finished_processing(&self) {
        //info!("CatchainInstanceListener::finished_processing");
    }

    fn started(&self) {
        log::info!("CatchainInstanceListener::started");
    }

    /// Notify about incoming broadcasts
    fn process_broadcast(&self, _source_id: PublicKeyHash, _data: BlockPayloadPtr) {}

    /// Notify about incoming query
    fn process_query(
        &self,
        _source_id: PublicKeyHash,
        _data: BlockPayloadPtr,
        _callback: QueryResponseCallback,
    ) {
    }

    fn set_time(&self, _timestamp: std::time::SystemTime) {}
}

fn test_catchain_attempt(attempt_idx: usize) {
    let overlay_manager = CatchainFactory::create_dummy_overlay_manager();
    let mut rng = rand::thread_rng();

    // generate random session id
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());

    // generate random nodes

    let mut nodes = Vec::new();
    nodes.reserve(NODE_COUNT);

    for _i in 0..NODE_COUNT {
        let private_key =
            Ed25519KeyOption::<ZeroizingBytes>::generate().expect("Failed to generate private key");
        let adnl_id = private_key.id();

        let catchain_node = CatchainNode { adnl_id: adnl_id.clone(), public_key: private_key };

        nodes.push(catchain_node);
    }

    // create catchains

    let mut catchain_instances = Vec::new();
    catchain_instances.reserve(NODE_COUNT);

    for i in 0..NODE_COUNT {
        let catchain_instance =
            CatchainInstance::new(session_id.clone(), i, nodes.clone(), overlay_manager.clone());
        catchain_instances.push(catchain_instance);
    }

    std::thread::sleep(std::time::Duration::from_secs(10));

    let mut heights_before = vec![0; NODE_COUNT];

    for (i, instance) in catchain_instances.iter().enumerate() {
        let (value, height) = {
            let instance = instance.lock();
            (instance.value(), instance.height())
        };

        heights_before[i] = height;

        log::info!("Before:CatchainInstance {} value={}, height={}", i, value, height);
    }

    let fork_cnt = if attempt_idx < 10 { 1 } else { (attempt_idx - 10) / 5 + 2 };

    for i in 0..fork_cnt {
        let instance = &catchain_instances[i];
        instance.lock().create_fork();
    }

    std::thread::sleep(std::time::Duration::from_secs(10));

    let mut heights_after = vec![0; NODE_COUNT];

    for (i, instance) in catchain_instances.iter().enumerate() {
        let (value, height) = {
            let instance = instance.lock();
            (instance.value(), instance.height())
        };

        heights_after[i] = height;

        log::info!(
            "After: CatchainInstance {} value={}, height={}, fork_cnt={}",
            i,
            value,
            height,
            fork_cnt
        );
    }

    for instance in catchain_instances.iter() {
        instance.lock().stop_async();
    }

    let mut hangs_count = 0;

    for i in 0..NODE_COUNT {
        let height_diff = heights_after[i] - heights_before[i];
        log::info!("Height diff for CatchainInstance {}: {}", i, height_diff);

        if height_diff < HEIGHT_THRESHOLD {
            hangs_count += 1;
        }
    }

    log::info!("Hangs count: {} (nodes_count={})", hangs_count, NODE_COUNT);

    assert!(hangs_count < NODE_COUNT / 3);
}

#[test]
fn test_catchain_forks() {
    const MAX_RUN: usize = 16;

    //env_logger::Builder::new()
    //    .format(|buf, record| {
    //        writeln!(
    //            buf,
    //            "{} {}:{} [{}] - {}",
    //            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
    //            record.file().unwrap(),
    //            record.line().unwrap(),
    //            record.level(),
    //            record.args()
    //        )
    //    })
    //    .filter(None, log::LevelFilter::Info)
    //    .init();
    init_test_log();

    let (sender, receiver) = channel();
    let mut tasks = Vec::new();
    for i in 0..TEST_ATTEMPTS {
        log::warn!("Test attempt #{}", i);
        if tasks.len() >= MAX_RUN {
            receiver.recv().unwrap()
        }
        let sender = sender.clone();
        let task = thread::spawn(move || {
            test_catchain_attempt(i);
            sender.send(()).unwrap();
        });
        tasks.push(task)
    }

    for task in tasks {
        task.join().unwrap();
    }
}
