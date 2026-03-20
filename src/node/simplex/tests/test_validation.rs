/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Validation test for Simplex consensus
//!
//! Tests that a two-node session correctly triggers the validation callback.
//! Node 0 generates a block, Node 1 receives it and triggers on_candidate.

use colored::Colorize;
use rand::Rng;
use simplex::*;
use std::{
    fs::{self, File},
    io::{LineWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use ton_block::{
    error, sha256_digest, BlockIdExt, BlockSignaturesVariant, Ed25519KeyOption, ShardIdent, UInt256,
};

include!("../../../common/src/info.rs");

/*
    Test constants
*/

/// Maximum time to wait for validation callback
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Test name for logging
const TEST_NAME: &str = "simplex_validation";

/*
    Test listener implementation
*/

/// Listener that tracks both collation and validation callbacks
struct ValidationTestListener {
    /// Node index (0 = leader, 1 = validator)
    node_idx: u32,
    /// Set to true when on_candidate is called (validation request)
    validation_requested: Arc<AtomicBool>,
    /// Validation count
    validation_count: Arc<AtomicU32>,
    /// Set to true when on_generate_slot is called
    collation_requested: Arc<AtomicBool>,
    /// Collation count
    collation_count: Arc<AtomicU32>,
    /// Public key for generating candidates
    public_key: PublicKey,
    /// Next expected seqno for collation - increases after each successful collation
    next_expected_collation_seqno: Arc<AtomicU32>,
    /// Next expected seqno for commit - initialized with initial_block_seqno, +1 for each commit
    next_expected_commit_seqno: Arc<AtomicU32>,
}

impl SessionListener for ValidationTestListener {
    fn on_candidate(
        &self,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        log::info!(
            "ValidationTestListener[{}]::on_candidate: VALIDATION REQUESTED for slot={}, hash={:?}",
            self.node_idx,
            source_info.priority.round,
            root_hash
        );

        // Mark validation as requested
        self.validation_requested.store(true, Ordering::Release);
        self.validation_count.fetch_add(1, Ordering::Relaxed);

        // Accept the candidate
        callback(Ok(SystemTime::now()));
    }

    fn on_generate_slot(
        &self,
        source_info: simplex::BlockSourceInfo,
        request: simplex::AsyncRequestPtr,
        parent: consensus_common::CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        let slot = source_info.priority.round;
        let request_id = request.get_request_id();

        log::info!(
            "ValidationTestListener[{}]::on_generate_slot: COLLATION REQUESTED for slot {} (request_id={})",
            self.node_idx,
            slot,
            request_id
        );

        // Mark collation as requested
        self.collation_requested.store(true, Ordering::Release);
        self.collation_count.fetch_add(1, Ordering::Relaxed);

        // Derive seqno from explicit parent hint or use stable counter value for implicit case
        let seqno = match &parent {
            consensus_common::CollationParentHint::Implicit => {
                // Keep seqno stable across retries for the same slot.
                // The counter is advanced on commit.
                self.next_expected_collation_seqno.load(Ordering::SeqCst)
            }
            consensus_common::CollationParentHint::Explicit(parent_id) => {
                // Explicit parent: derive seqno from parent (parent_seqno + 1)
                let derived_seqno = parent_id.seq_no + 1;
                // Update counter to match derived seqno for next iteration
                self.next_expected_collation_seqno.store(derived_seqno + 1, Ordering::SeqCst);
                derived_seqno
            }
        };

        // Generate dummy candidate with proper hashes
        // The collator must provide file_hash = sha256(data) and collated_file_hash = sha256(collated_data)
        // to match what the receiver will compute from the data
        let block_data = vec![1u8, 2, 3, 4];
        let collated_data_bytes: Vec<u8> = vec![];

        // Compute hashes that match what receiver will compute
        let file_hash = UInt256::from_slice(&sha256_digest(&block_data));
        let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data_bytes));

        // root_hash can be anything (it's extracted from the TL structure)
        let root_hash = file_hash.clone();

        log::debug!(
            "ValidationTestListener[{}]::on_generate_slot: slot={}, seqno={}",
            self.node_idx,
            slot,
            seqno
        );

        let candidate = ValidatorBlockCandidate {
            public_key: self.public_key.clone(),
            id: BlockIdExt::with_params(
                ShardIdent::masterchain(),
                seqno, // Use tracked seqno
                root_hash,
                file_hash,
            ),
            collated_file_hash,
            data: consensus_common::ConsensusCommonFactory::create_block_payload(block_data),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                collated_data_bytes,
            ),
        };

        callback(Ok(Arc::new(candidate)));
    }

    fn on_block_committed(
        &self,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: consensus_common::SessionStats,
    ) {
        let slot = source_info.priority.round;

        // Increment next_expected_commit_seqno and update next_expected_collation_seqno
        let committed_seqno = self.next_expected_commit_seqno.fetch_add(1, Ordering::SeqCst);
        let next_commit_seqno = committed_seqno + 1;
        self.next_expected_collation_seqno.store(next_commit_seqno, Ordering::SeqCst);

        log::info!(
            "ValidationTestListener[{}]::on_block_committed: slot={}, hash={:?}, committed_seqno={}, next_expected={}",
            self.node_idx,
            slot,
            root_hash,
            committed_seqno,
            next_commit_seqno
        );
    }

    fn on_block_skipped(&self, _round: u32) {
        unreachable!("on_block_skipped should not be called in Simplex");
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        // Not used in this test
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

/*
    Test runner
*/

fn run_validation_test() {
    const DB_PATH: &str = "../../target/test";

    // Initialize logger
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = SystemTime::now().into();
    let out_log_file_name =
        format!("debug-simplex-validation-test-{}.log", datetime.format("%Y-%m-%d-%H.%M.%S"));
    let logs_path = Path::new("../../target/logs");
    fs::create_dir_all(logs_path).expect("unable to create output log path");
    let file =
        File::create(logs_path.join(out_log_file_name)).expect("unable to create output log file");
    let file = Arc::new(Mutex::new(LineWriter::new(file)));

    // Error counter - test will fail if any errors are logged
    let error_count = Arc::new(AtomicU32::new(0));
    let error_count_for_logger = error_count.clone();

    env_logger::Builder::new()
        .format(move |buf, record| {
            // Track errors
            if record.level() == log::Level::Error {
                error_count_for_logger.fetch_add(1, Ordering::Relaxed);
            }

            let message = format!("{}", record.args());
            let level = format!("{}", record.level());
            let line = match record.line() {
                Some(line) => format!("({})", line),
                None => "".to_string(),
            };
            let source = format!("{}{}", record.target(), line);
            let thread_id = thread::current().id();

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
                _ => writeln!(
                    buf,
                    "{} [{: <5}] - {:?} - {: <45}| {}",
                    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                    level,
                    thread_id,
                    source,
                    message
                ),
            }
        })
        .filter_level(log::LevelFilter::Debug)
        .try_init()
        .unwrap_or_else(|_| {
            // Logger already initialized
        });

    log::info!("=== STARTING VALIDATION TEST ===");

    // Create two nodes
    let private_key_0 = Ed25519KeyOption::generate().expect("Failed to generate private key 0");
    let private_key_1 = Ed25519KeyOption::generate().expect("Failed to generate private key 1");

    let node_0 = SessionNode {
        adnl_id: private_key_0.id().clone(),
        public_key: private_key_0.clone(),
        weight: 1,
    };
    let node_1 = SessionNode {
        adnl_id: private_key_1.id().clone(),
        public_key: private_key_1.clone(),
        weight: 1,
    };
    let nodes = vec![node_0, node_1];

    // Create overlay manager with 2 nodes
    let overlay_manager = SessionFactory::create_in_process_overlay_manager(2);

    // Generate session ID
    let rand_name: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path_0 = format!("{}/{}_{}_node0", DB_PATH, TEST_NAME, rand_name);
    let db_path_1 = format!("{}/{}_{}_node1", DB_PATH, TEST_NAME, rand_name);
    let mut rng = rand::thread_rng();
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());

    // Session options - fast timing for quick test
    let session_opts = SessionOptions {
        proto_version: 0,
        target_rate: Duration::from_millis(500),
        first_block_timeout: Duration::from_millis(1000),
        slots_per_leader_window: 1,
        ..Default::default()
    };

    // Create listeners with tracking
    let collation_requested_0 = Arc::new(AtomicBool::new(false));
    let collation_count_0 = Arc::new(AtomicU32::new(0));
    let validation_requested_0 = Arc::new(AtomicBool::new(false));
    let validation_count_0 = Arc::new(AtomicU32::new(0));

    let collation_requested_1 = Arc::new(AtomicBool::new(false));
    let collation_count_1 = Arc::new(AtomicU32::new(0));
    let validation_requested_1 = Arc::new(AtomicBool::new(false));
    let validation_count_1 = Arc::new(AtomicU32::new(0));

    let initial_block_seqno = 1; // First block seqno=1 (seqno 0 is zerostate)

    let listener_0 = Arc::new(ValidationTestListener {
        node_idx: 0,
        validation_requested: validation_requested_0.clone(),
        validation_count: validation_count_0.clone(),
        collation_requested: collation_requested_0.clone(),
        collation_count: collation_count_0.clone(),
        public_key: private_key_0.clone(),
        next_expected_collation_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
        next_expected_commit_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
    });

    let listener_1 = Arc::new(ValidationTestListener {
        node_idx: 1,
        validation_requested: validation_requested_1.clone(),
        validation_count: validation_count_1.clone(),
        collation_requested: collation_requested_1.clone(),
        collation_count: collation_count_1.clone(),
        public_key: private_key_1.clone(),
        next_expected_collation_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
        next_expected_commit_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
    });

    let session_listener_0: Arc<dyn SessionListener + Send + Sync> = listener_0.clone();
    let session_listener_1: Arc<dyn SessionListener + Send + Sync> = listener_1.clone();

    let shard = ShardIdent::masterchain();

    log::info!("Creating sessions with 2 nodes (node 0 is leader for slot 0)");

    let initial_block_seqno = 1; // First block seqno=1 (seqno 0 is zerostate)

    // Create session for node 0 (leader for first slot)
    let session_0 = SessionFactory::create_session(
        &session_opts,
        &session_id,
        &shard,
        initial_block_seqno,
        nodes.clone(),
        &private_key_0,
        db_path_0,
        overlay_manager.clone(),
        Arc::downgrade(&session_listener_0),
    )
    .expect("Failed to create session 0");

    // Create session for node 1
    let session_1 = SessionFactory::create_session(
        &session_opts,
        &session_id,
        &shard,
        initial_block_seqno,
        nodes.clone(),
        &private_key_1,
        db_path_1,
        overlay_manager.clone(),
        Arc::downgrade(&session_listener_1),
    )
    .expect("Failed to create session 1");

    log::info!("Sessions created, waiting for validation callback on node 1...");

    // Wait for validation callback on node 1 (node 0 should collate, node 1 should validate)
    let test_start = Instant::now();
    let mut validation_triggered = false;

    while test_start.elapsed() < VALIDATION_TIMEOUT {
        // Check if node 1 received a validation request
        if validation_requested_1.load(Ordering::Acquire) {
            validation_triggered = true;
            log::info!("VALIDATION CALLBACK TRIGGERED on node 1 after {:?}", test_start.elapsed());
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    // Log intermediate state
    log::info!(
        "State after waiting: node0 collations={}, validations={}; node1 collations={}, validations={}",
        collation_count_0.load(Ordering::Relaxed),
        validation_count_0.load(Ordering::Relaxed),
        collation_count_1.load(Ordering::Relaxed),
        validation_count_1.load(Ordering::Relaxed),
    );

    // Stop sessions
    session_0.stop();
    session_1.stop();

    // Wait a bit for cleanup
    thread::sleep(Duration::from_millis(100));

    // Report results
    log::info!(
        "Test completed: validation_triggered={}, node0_collations={}, node1_validations={}",
        validation_triggered,
        collation_count_0.load(Ordering::Relaxed),
        validation_count_1.load(Ordering::Relaxed),
    );

    // Assert that node 0 generated at least one block
    assert!(
        collation_requested_0.load(Ordering::Relaxed),
        "Node 0 should have received a collation request (it's leader for slot 0)"
    );

    // Assert that node 1 validated at least one block
    assert!(
        validation_triggered,
        "Node 1's on_candidate was NOT called within {:?}. \
        Node 0 should have broadcast a block that node 1 validates.",
        VALIDATION_TIMEOUT
    );

    // Assert no errors were logged during the test
    let errors = error_count.load(Ordering::Relaxed);
    assert!(
        errors == 0,
        "Test failed: {} ERROR log message(s) were emitted during the test. Check logs for details.",
        errors
    );

    log::info!("=== VALIDATION TEST PASSED ===");
}

/*
    Test entry point
*/

#[test]
fn test_two_node_validation() {
    run_validation_test();
}
