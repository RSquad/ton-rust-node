/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use colored::Colorize;
use rand::Rng;
use serde::{Deserialize, Serialize};
use spin::mutex::SpinMutex;
use std::{
    fs::File,
    io::{LineWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use ton_block::{error, BlockIdExt, BlockSignaturesVariant, Ed25519KeyOption, ShardIdent, UInt256};
use validator_session::*;

include!("../../../common/src/info.rs");

//TODO: enable compressed candidates support
const PROTO_VERSION: u32 = 0;

#[derive(Serialize, Deserialize)]
struct DummyCollatedData {
    creation_timestamp: u64,
    round: u32,
}

impl DummyCollatedData {
    fn new(round: u32) -> Self {
        Self {
            creation_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            round,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap()
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        bincode::deserialize(bytes).unwrap()
    }
}

/// Test configuration structure containing all configurable parameters
#[derive(Clone, Debug)]
struct TestConfig {
    /// Maximum round to wait for in the test
    max_wait_round: u32,
    /// Number of validator nodes in the test
    node_count: usize,
    /// Probability of generation failure (0.0 - 1.0)
    generation_failure_probability: f64,
    /// Probability of candidate rejection (0.0 - 1.0)
    candidate_rejection_probability: f64,
    /// Maximum number of collations before ignoring requests
    max_collations: u32,
    /// Catchain max block delay for normal attempts
    catchain_max_block_delay: std::time::Duration,
    /// Next candidate delay (empty block delay)
    next_candidate_delay: std::time::Duration,
    /// Number of validation retry attempts
    validation_retry_attempts: u32,
    /// Timeout between validation retry attempts
    validation_retry_timeout: std::time::Duration,
    /// Test name for logging and database naming
    test_name: String,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            max_wait_round: 500,
            node_count: 11,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 1500,
            catchain_max_block_delay: std::time::Duration::from_millis(5),
            next_candidate_delay: std::time::Duration::from_millis(2000),
            validation_retry_attempts: 0,
            validation_retry_timeout: std::time::Duration::from_millis(1000),
            test_name: "accelerated_consensus_session".to_string(),
        }
    }
}
struct SessionInstance {
    source_index: u32,
    public_key: PublicKey,
    batch_processed: Arc<AtomicBool>,
    collation_requested: Arc<AtomicBool>,
    is_collator: Arc<AtomicBool>,
    collation_count: Arc<std::sync::atomic::AtomicU32>,
    on_candidate_count: Arc<std::sync::atomic::AtomicU32>,
    on_block_committed_count: Arc<std::sync::atomic::AtomicU32>,
    config: TestConfig,
    expected_current_round: Arc<std::sync::atomic::AtomicU32>,
    expected_collation_round: Arc<std::sync::atomic::AtomicU32>,
    _session: SessionPtr,
    _listener: Arc<dyn SessionListener + Send + Sync>,
}

struct SessionInstanceListener {
    instance: SpinMutex<Weak<SpinMutex<SessionInstance>>>,
}

impl SessionInstance {
    fn is_finished(&self) -> bool {
        self.batch_processed.load(Ordering::Relaxed)
    }

    fn collation_requested(&self) -> bool {
        self.collation_requested.load(Ordering::Relaxed)
    }

    fn collation_count(&self) -> u32 {
        self.collation_count.load(Ordering::Relaxed)
    }

    fn on_candidate_count(&self) -> u32 {
        self.on_candidate_count.load(Ordering::Relaxed)
    }

    fn on_block_committed_count(&self) -> u32 {
        self.on_block_committed_count.load(Ordering::Relaxed)
    }

    fn finish_round(&self, round: u32, force_skip_collation_pipeline: bool) {
        let expected_current = self.expected_current_round.load(Ordering::SeqCst);
        if round != expected_current {
            log::error!(
                "round {} != expected_current_round {}, expected_current_round sequence violation (source #{})",
                round,
                expected_current,
                self.source_index
            );
        }

        let new_expected_current_round = round + 1;
        self.expected_current_round.store(new_expected_current_round, Ordering::SeqCst);

        let expected_collation_round = self.expected_collation_round.load(Ordering::SeqCst);

        if new_expected_current_round <= expected_collation_round && !force_skip_collation_pipeline
        {
            //do nothing, normal case
        } else {
            if new_expected_current_round > expected_collation_round {
                log::debug!(
                    "Collation priority assignment not detected on round {} (expected_current_round = {}, expected_collation_round = {}, source = #{})",
                    round,
                    new_expected_current_round,
                    expected_collation_round,
                    self.source_index
                );
            } else if force_skip_collation_pipeline {
                log::debug!(
                    "Force skipping collation pipeline after round {} for source #{}",
                    round,
                    self.source_index
                );
            }

            let was_collator = self.is_collator.swap(false, Ordering::Release);
            if was_collator {
                log::info!(
                    "COLLATOR STATUS: Node lost collator status on round {} (source #{})",
                    round,
                    self.source_index
                );
            }

            self.expected_collation_round.store(new_expected_current_round, Ordering::SeqCst);
        }

        if round >= self.config.max_wait_round {
            self.batch_processed.store(true, Ordering::Release);
            log::info!("Test finished after round {} for source #{}", round, self.source_index);
        }
    }
}

impl SessionListener for SessionInstance {
    fn on_candidate(
        &self,
        source_info: validator_session::BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        let round = source_info.priority.round;
        // Increment on_candidate counter
        self.on_candidate_count.fetch_add(1, Ordering::Relaxed);

        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;
        let collated_data = DummyCollatedData::from_bytes(data.data());
        let latency = now - collated_data.creation_timestamp;

        // Validate that the round in the collated data matches the expected round
        if collated_data.round != round {
            log::error!(
                "ROUND MISMATCH in on_candidate: candidate data contains round {} but callback round is {} (source #{})",
                collated_data.round,
                round,
                self.source_index
            );
            assert_eq!(
                collated_data.round,
                round,
                "Round mismatch in on_candidate: candidate data contains round {} but callback round is {}",
                collated_data.round,
                round
            );
        }

        log::info!(
            "SessionListener::on_candidate: new candidate for \
           round {} from source {} with hash {:?} appeared with latency {} ms (priority={}, first_block_round={}, self source #{})",
            round,
            source_info.source.id(),
            root_hash,
            latency,
            source_info.priority.priority,
            source_info.priority.first_block_round,
            self.source_index
        );

        let mut rng = rand::thread_rng();

        // Check if candidate should be rejected based on probability
        if rng.gen::<f64>() < self.config.candidate_rejection_probability {
            log::warn!("SessionListener::on_candidate: rejecting candidate for round {} from source {} (simulated rejection, self source #{})", round, source_info.source.id(), self.source_index);
            callback(Err(error!(
                "Simulated candidate rejection for round {} from source {}",
                round,
                source_info.source.id()
            )));
            return;
        }

        callback(Ok(std::time::SystemTime::now()))
    }

    fn on_generate_slot(
        &self,
        source_info: validator_session::BlockSourceInfo,
        request: validator_session::AsyncRequestPtr,
        parent: validator_session::CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        debug_assert!(
            matches!(parent, validator_session::CollationParentHint::Implicit),
            "validator-session tests: Explicit parent hint is not expected yet"
        );

        let round = source_info.priority.round;
        let request_id = request.get_request_id();
        log::info!(
            "SessionListener::on_generate_slot: collator request for round {} with request ID {} (priority={}, first_block_round={}, source #{})",
            round,
            request_id,
            source_info.priority.priority,
            source_info.priority.first_block_round,
            self.source_index
        );

        //CAS check: atomically set is_collator from false to true when node becomes a collator on this round
        if !self.is_collator.swap(true, Ordering::Relaxed) {
            log::info!(
                "COLLATOR STATUS: Node became collator on round {} (source #{})",
                round,
                self.source_index
            );
        }

        // Validate round sequencing - fail if round is obsolete (round < expected_current_round)
        let expected_current_round = self.expected_current_round.load(Ordering::SeqCst);
        if round < expected_current_round {
            log::warn!(
                "on_generate_slot called for obsolete round {} which is less than expected current round {} (source #{})",
                round,
                expected_current_round,
                self.source_index
            );
            callback(Err(error!(
                "on_generate_slot called for obsolete round {} which is less than expected current round {}",
                round,
                expected_current_round
            )));
            return;
        }

        // Validate collation round sequencing - must be exact match
        let expected_collation_round = self.expected_collation_round.load(Ordering::SeqCst);
        if round != expected_collation_round {
            log::error!(
                "COLLATION ROUND MISMATCH: on_generate_slot called for round {} with request ID {} but expected collation round is {} (source #{})",
                round,
                request_id,
                expected_collation_round,
                self.source_index
            );
            assert_eq!(
                round, expected_collation_round,
                "on_generate_slot called for round {} but expected collation round is {}",
                round, expected_collation_round
            );
        }

        // Check if we've reached the maximum number of collations
        let current_count = self.collation_count.fetch_add(1, Ordering::SeqCst);
        if current_count >= self.config.max_collations {
            log::warn!("SessionListener::on_generate_slot: ignoring collation request for round {} with request ID {} (reached max collations: {}, source #{})", round, request_id, self.config.max_collations, self.source_index);
            return;
        }

        let mut rng = rand::thread_rng();

        // Check if generation should fail based on probability
        if rng.gen::<f64>() < self.config.generation_failure_probability {
            log::warn!("SessionListener::on_generate_slot: generation failed for round {} with request ID {} (simulated failure, source #{})", round, request_id, self.source_index);
            self.collation_requested.store(true, Ordering::Release);
            callback(Err(error!(
                "Simulated generation failure for round {} with request ID {}",
                round, request_id
            )));
            return;
        }

        let mut hash_bytes = [0; 32];

        for byte in &mut hash_bytes {
            *byte = rng.gen_range(0..255);
        }

        let hash = UInt256::with_array(hash_bytes);

        let collated_data = DummyCollatedData::new(round);
        let candidate = ValidatorBlockCandidate {
            public_key: self.public_key.clone(),
            id: BlockIdExt::with_params(
                ShardIdent::masterchain(),
                0, // seq_no not tracked in these tests
                hash.clone(),
                hash.clone(),
            ),
            collated_file_hash: hash.clone(),
            data: catchain::CatchainFactory::create_block_payload(collated_data.to_bytes()),
            collated_data: catchain::CatchainFactory::create_empty_block_payload(),
        };

        self.collation_requested.store(true, Ordering::Release);

        self.expected_collation_round.store(round + 1, Ordering::SeqCst);

        callback(Ok(Arc::new(candidate)));
    }

    fn on_block_committed(
        &self,
        source_info: validator_session::BlockSourceInfo,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: validator_session::ValidatorSessionStats,
    ) {
        let round = source_info.priority.round;
        self.on_block_committed_count.fetch_add(1, Ordering::Relaxed);

        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;
        let collated_data = DummyCollatedData::from_bytes(data.data());
        let latency = now - collated_data.creation_timestamp;
        log::info!(
            "SessionListener::on_block_committed: new block from source {} with hash {:?} has been committed in round {} with latency {} ms (priority={}, first_block_round={}, source #{})",
            source_info.source.id(),
            root_hash,
            round,
            latency,
            source_info.priority.priority,
            source_info.priority.first_block_round,
            self.source_index
        );

        // Finish round with force_skip_collation_pipeline=false (similar to ValidatorGroup)
        const FORCE_SKIP_COLLATION_PIPELINE: bool = false;
        self.finish_round(round, FORCE_SKIP_COLLATION_PIPELINE);
    }

    fn on_block_skipped(&self, round: u32) {
        log::info!(
            "SessionListener::on_block_skipped: round {} has no committed block (source #{})",
            round,
            self.source_index
        );

        self.on_block_committed_count.fetch_add(1, Ordering::Relaxed);

        // Finish round with force_skip_collation_pipeline=true (similar to ValidatorGroup)
        const FORCE_SKIP_COLLATION_PIPELINE: bool = true;
        self.finish_round(round, FORCE_SKIP_COLLATION_PIPELINE);
    }

    fn get_approved_candidate(
        &self,
        source: PublicKey,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        log::info!(
            "SessionListener::get_approved_candidate: \
            approved candidate from source {:?} request for block hash {:?} (self source #{})",
            source,
            root_hash,
            self.source_index
        );
    }

    fn get_committed_candidate(
        &self,
        block_id: BlockIdExt,
        callback: consensus_common::CommittedBlockProofCallback,
    ) {
        log::info!("get_committed_candidate: STUB for block_id={block_id}");
        callback(Err(error!("get_committed_candidate not implemented in test")));
    }
}

impl SessionListener for SessionInstanceListener {
    fn on_candidate(
        &self,
        source_info: validator_session::BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_candidate(source_info, root_hash, data, collated_data, callback);
        }
    }

    fn on_generate_slot(
        &self,
        source_info: validator_session::BlockSourceInfo,
        request: validator_session::AsyncRequestPtr,
        parent: validator_session::CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_generate_slot(source_info, request, parent, callback);
        }
    }

    fn on_block_committed(
        &self,
        source_info: validator_session::BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        stats: validator_session::ValidatorSessionStats,
    ) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_block_committed(
                source_info,
                root_hash,
                file_hash,
                data,
                signatures,
                approve_signatures,
                stats,
            );
        }
    }

    fn on_block_skipped(&self, round: u32) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_block_skipped(round);
        }
    }

    fn get_approved_candidate(
        &self,
        source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        collated_data_hash: BlockHash,
        callback: ValidatorBlockCandidateCallback,
    ) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.get_approved_candidate(
                source,
                root_hash,
                file_hash,
                collated_data_hash,
                callback,
            );
        }
    }

    fn get_committed_candidate(
        &self,
        block_id: BlockIdExt,
        callback: consensus_common::CommittedBlockProofCallback,
    ) {
        log::info!("get_committed_candidate: STUB for block_id={block_id}");
        callback(Err(error!("get_committed_candidate not implemented in test")));
    }
}

/// Generalized test function that runs validator session tests with configurable parameters
fn run_validator_session_accelerated_consensus_test<F>(config: TestConfig, post_test_functor: F)
where
    F: FnOnce(&Vec<Arc<SpinMutex<SessionInstance>>>) -> (),
{
    // Constants for all tests
    const DB_PATH: &str = "../../target/test";
    const ACCELERATED_CONSENSUS_ENABLED: bool = true;
    const ROUND_CANDIDATES: u32 = 1;

    //init logger - same log file for all tests
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = std::time::SystemTime::now().into();
    let out_log_file_name =
        format!("debug-accelerated-consensus-tests-{}.log", datetime.format("%Y-%m-%d-%H.%M.%S"));
    let logs_path = Path::new("../../target/logs");
    std::fs::create_dir_all(logs_path).expect("unable to create output log path");
    let file =
        File::create(logs_path.join(out_log_file_name)).expect("unable to create output log file");
    let file = Arc::new(Mutex::new(LineWriter::new(file)));

    env_logger::Builder::new()
        .format(move |buf, record| {
            let message = format!("{}", record.args());
            let level = format!("{}", record.level());
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

            file.write_all(log_line.as_bytes())?;
            file.write_all(b"\n")?;

            let (message, level) = match record.level() {
                log::Level::Error => (message.red(), level.red()),
                log::Level::Warn => (message.yellow(), level.yellow()),
                log::Level::Trace => (message.dimmed(), level.dimmed()),
                log::Level::Info => {
                    if record.target() == module_path!() {
                        (message.bright_green().bold(), level.bright_green().bold())
                    } else {
                        (message.bright_white().bold(), level.bright_white().bold())
                    }
                }
                _ => (message.normal(), level.normal()),
            };

            match record.level() {
                log::Level::Trace | log::Level::Debug => Ok(()),
                _ => {
                    writeln!(
                        buf,
                        "{} [{: <5}] - {:?} - {: <45}| {}",
                        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                        level,
                        thread_id,
                        source,
                        message
                    )?;

                    std::io::stdout().flush()
                }
            }
        })
        //.filter_level(log::LevelFilter::Info)
        .filter_level(log::LevelFilter::Debug)
        .try_init()
        .unwrap_or_else(|_| {
            // Logger already initialized, which is fine for multiple tests
        });

    // Log test start
    log::info!("=== STARTING TEST: {} ===", config.test_name);

    //initialize Validator Session

    //generate random nodes
    let mut nodes = Vec::new();
    nodes.reserve(config.node_count);

    for _i in 0..config.node_count {
        let private_key = Ed25519KeyOption::generate().expect("Failed to generate private key");
        let adnl_id = private_key.id();

        let catchain_node =
            SessionNode { adnl_id: adnl_id.clone(), public_key: private_key, weight: 1 };

        nodes.push(catchain_node);
    }

    let overlay_threads_count = config.node_count;
    let overlay_manager =
        catchain::CatchainFactory::create_in_process_overlay_manager(overlay_threads_count);

    let rand_name: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path = format!("{}/vs_test_{}_{}", DB_PATH, config.test_name, rand_name);
    let mut rng = rand::thread_rng();
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());
    let session_opts = SessionOptions {
        proto_version: PROTO_VERSION,
        accelerated_consensus_enabled: ACCELERATED_CONSENSUS_ENABLED,
        round_candidates: ROUND_CANDIDATES,
        next_candidate_delay: config.next_candidate_delay,
        catchain_max_deps: config.node_count as u32,
        catchain_receiver_max_neighbours_count: config.node_count,
        catchain_receiver_neighbours_sync_min_period: std::time::Duration::from_millis(1000),
        catchain_receiver_neighbours_sync_max_period: std::time::Duration::from_millis(2000),
        validation_retry_attempts: config.validation_retry_attempts,
        validation_retry_timeout: config.validation_retry_timeout,
        //catchain_idle_timeout: std::time::Duration::from_millis(100),
        ..Default::default()
    };

    let mut instances = Vec::new();
    instances.reserve(config.node_count);

    for i in 0..config.node_count {
        let local_key = nodes[i].public_key.clone();
        let db_suffix = format!("catchain_{}", i);
        let listener = Arc::new(SessionInstanceListener { instance: SpinMutex::new(Weak::new()) });
        let session_listener: Arc<dyn SessionListener + Send + Sync> = listener.clone();
        let session = SessionFactory::create_session(
            &session_opts,
            &session_id,
            nodes.clone(),
            &local_key,
            db_path.clone(),
            db_suffix,
            false,
            overlay_manager.clone(),
            Arc::downgrade(&session_listener),
        )
        .unwrap();

        session.set_catchain_max_block_delay(
            config.catchain_max_block_delay,
            2 * config.catchain_max_block_delay,
        );

        let session_instance = Arc::new(SpinMutex::new(SessionInstance {
            public_key: nodes[i].public_key.clone(),
            batch_processed: Arc::new(AtomicBool::new(false)),
            collation_requested: Arc::new(AtomicBool::new(false)),
            collation_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            on_candidate_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            on_block_committed_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            is_collator: Arc::new(AtomicBool::new(false)),
            config: config.clone(),
            expected_current_round: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            expected_collation_round: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            source_index: i as u32,
            _session: session,
            _listener: listener.clone(),
        }));

        *listener.instance.lock() = Arc::downgrade(&session_instance);

        instances.push(session_instance);
    }

    //wait for all instances to finish

    'main_loop: loop {
        for instance in instances.iter() {
            let is_finished = instance.lock().is_finished();
            if !is_finished {
                std::thread::sleep(Duration::from_millis(10));
                continue 'main_loop;
            }
        }

        break;
    }

    // Call post-test functor with all instances
    post_test_functor(&instances);

    for (index, instance) in instances.iter().enumerate() {
        let is_finished = instance.lock().is_finished();
        let was_collation_requested = instance.lock().collation_requested();
        let collation_count = instance.lock().collation_count();
        let candidate_count = instance.lock().on_candidate_count();
        let commit_count = instance.lock().on_block_committed_count();
        log::info!(
            "Instance {}: finished={}, collation_requested={}, collation_count={}, candidate_count={}, commit_count={}",
            index,
            is_finished,
            was_collation_requested,
            collation_count,
            candidate_count,
            commit_count
        );
        assert!(is_finished);
    }

    // Log test completion
    log::info!("=== FINISHED TEST: {} ===", config.test_name);
}

//#[ignore]
#[test]
fn test_accelerated_consensus_session_normal_case() {
    run_validator_session_accelerated_consensus_test(
        TestConfig {
            max_wait_round: 50,
            node_count: 11,
            generation_failure_probability: 0.2,
            candidate_rejection_probability: 0.2,
            max_collations: 15,
            catchain_max_block_delay: std::time::Duration::from_millis(5),
            next_candidate_delay: std::time::Duration::from_millis(2000),
            test_name: "accelerated_consensus_normal_case".to_string(),
            ..Default::default()
        },
        |_instances| {
            // No additional checks for normal case test
        },
    );
}

#[test]
fn test_accelerated_consensus_session_ideal_case() {
    run_validator_session_accelerated_consensus_test(
        TestConfig {
            max_wait_round: 50,
            node_count: 11,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 150,
            catchain_max_block_delay: std::time::Duration::from_millis(1),
            next_candidate_delay: std::time::Duration::from_millis(2000),
            test_name: "accelerated_consensus_ideal_case".to_string(),
            ..Default::default()
        },
        |_instances| {
            // No additional checks for normal case test
        },
    );
}

//#[ignore]
#[test]
fn test_accelerated_consensus_session_precollations() {
    run_validator_session_accelerated_consensus_test(
        TestConfig {
            max_wait_round: 25,
            node_count: 11,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 50,
            catchain_max_block_delay: std::time::Duration::from_millis(500),
            next_candidate_delay: std::time::Duration::from_millis(10000),
            test_name: "accelerated_consensus_precollations".to_string(),
            ..Default::default()
        },
        |_instances| {
            // No additional checks for precollations test
        },
    );
}

//#[ignore]
#[test]
fn test_accelerated_consensus_validation_retries() {
    run_validator_session_accelerated_consensus_test(
        TestConfig {
            max_wait_round: 50,
            node_count: 11,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.5,
            max_collations: 100,
            catchain_max_block_delay: std::time::Duration::from_millis(5),
            next_candidate_delay: std::time::Duration::from_millis(2000),
            validation_retry_attempts: 3,
            validation_retry_timeout: std::time::Duration::from_millis(500),
            test_name: "accelerated_consensus_validation_retries".to_string(),
            ..Default::default()
        },
        |instances| {
            // Validation retries test: Assert that candidate calls > commit count due to retries
            for (index, instance) in instances.iter().enumerate() {
                let candidate_count = instance.lock().on_candidate_count();
                let commit_count = instance.lock().on_block_committed_count();

                log::info!(
                    "VALIDATION RETRIES CHECK - Instance {}: candidate_count={}, commit_count={}",
                    index,
                    candidate_count,
                    commit_count
                );

                // Assert that candidate calls were more than commit count due to retries
                assert!(
                    candidate_count > commit_count,
                    "Instance {}: Expected candidate_count ({}) > commit_count ({}) due to validation retries",
                    index, candidate_count, commit_count
                );
            }
        },
    );
}
