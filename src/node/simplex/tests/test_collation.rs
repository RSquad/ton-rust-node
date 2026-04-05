/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Collation-only test for Simplex consensus
//!
//! Tests that a single-node session correctly triggers the collation callback.
//! This is the simplest possible consensus test - one node, one collation.

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
    error, sha256_digest, BlockIdExt, BlockSignaturesVariant, BocFlags, BocWriter, BuilderData,
    Ed25519KeyOption, ShardIdent, UInt256,
};

include!("../../../common/src/info.rs");

/*
    Test constants
*/

/// Maximum time to wait for collation callback
const COLLATION_TIMEOUT: Duration = Duration::from_secs(25);

/// Test name for logging
const TEST_NAME: &str = "simplex_collation_only";

/*
    Test listener implementation
*/

/// Simple listener that tracks collation callbacks
struct CollationTestListener {
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

impl SessionListener for CollationTestListener {
    fn on_candidate(
        &self,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        log::info!(
            "CollationTestListener::on_candidate: slot={}, hash={:?}",
            source_info.priority.round,
            root_hash
        );
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
            "CollationTestListener::on_generate_slot: COLLATION REQUESTED for slot {} (request_id={})",
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
        // to match what the receiver will compute from the data.
        // Block data MUST be valid BOC — compress_candidate_data requires it.
        let block_data = {
            let mut b = BuilderData::new();
            b.append_raw(&[1u8, 2, 3, 4], 32).unwrap();
            let cell = b.into_cell().unwrap();
            let mut buf = Vec::new();
            BocWriter::with_flags([cell], BocFlags::all()).unwrap().write(&mut buf).unwrap();
            buf
        };
        let collated_data_bytes: Vec<u8> = vec![];

        // Compute hashes that match what receiver will compute
        let file_hash = UInt256::from_slice(&sha256_digest(&block_data));
        let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data_bytes));

        // root_hash can be anything (it's extracted from the TL structure)
        let root_hash = file_hash.clone();

        log::debug!("on_generate_slot: slot={}, seqno={}", slot, seqno);

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
            "CollationTestListener::on_block_committed: slot={}, hash={:?}, committed_seqno={}, next_expected={}",
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
        log::info!("get_committed_candidate: STUB for block_id={}", block_id);
        callback(Err(error!("get_committed_candidate not implemented in test")));
    }
}

/*
    Test runner
*/

fn run_collation_test() {
    const DB_PATH: &str = "../../target/test";

    // Initialize logger
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = SystemTime::now().into();
    let out_log_file_name =
        format!("debug-simplex-collation-test-{}.log", datetime.format("%Y-%m-%d-%H.%M.%S"));
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

    log::info!("=== STARTING COLLATION TEST ===");

    // Create single node
    let private_key = Ed25519KeyOption::generate().expect("Failed to generate private key");
    let adnl_id = private_key.id();
    let public_key = private_key.clone();

    let node = SessionNode { adnl_id: adnl_id.clone(), public_key: private_key.clone(), weight: 1 };
    let nodes = vec![node];

    // Create overlay manager with 1 thread
    let overlay_manager = SessionFactory::create_in_process_overlay_manager(1);

    // Generate session ID
    let rand_name: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path = format!("{}/{}_{}", DB_PATH, TEST_NAME, rand_name);
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

    // Create listener with tracking
    let collation_requested = Arc::new(AtomicBool::new(false));
    let collation_count = Arc::new(AtomicU32::new(0));

    let initial_block_seqno = 1; // First block will have seqno=1 (seqno 0 is zerostate)
    let listener = Arc::new(CollationTestListener {
        collation_requested: collation_requested.clone(),
        collation_count: collation_count.clone(),
        public_key,
        next_expected_collation_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
        next_expected_commit_seqno: Arc::new(AtomicU32::new(initial_block_seqno)),
    });

    let session_listener: Arc<dyn SessionListener + Send + Sync> = listener.clone();

    // Create session
    let shard = ShardIdent::masterchain();

    log::info!("Creating session with 1 node (self is leader for all slots)");

    let initial_block_seqno = 1; // First block seqno=1 (seqno 0 is zerostate)
    let session = SessionFactory::create_session(
        &session_opts,
        &session_id,
        &shard,
        nodes,
        &private_key,
        db_path,
        overlay_manager,
        Arc::downgrade(&session_listener),
    )
    .expect("Failed to create session");
    session.start(initial_block_seqno);

    log::info!("Session created, waiting for collation callback...");

    // Wait for collation callback
    let test_start = Instant::now();
    let mut collation_triggered = false;

    while test_start.elapsed() < COLLATION_TIMEOUT {
        if collation_requested.load(Ordering::Acquire) {
            collation_triggered = true;
            log::info!("COLLATION CALLBACK TRIGGERED after {:?}", test_start.elapsed());
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    // Stop session
    session.stop();

    // Wait a bit for cleanup
    thread::sleep(Duration::from_millis(100));

    // Report results
    let final_count = collation_count.load(Ordering::Relaxed);
    log::info!(
        "Test completed: collation_triggered={}, collation_count={}",
        collation_triggered,
        final_count
    );

    // Assert
    assert!(
        collation_triggered,
        "Collation callback was NOT triggered within {:?}",
        COLLATION_TIMEOUT
    );

    // Assert no errors were logged during the test
    let errors = error_count.load(Ordering::Relaxed);
    assert!(
        errors == 0,
        "Test failed: {} ERROR log message(s) were emitted during the test. Check logs for details.",
        errors
    );

    log::info!("=== COLLATION TEST PASSED ===");
}

/*
    Test entry point
*/

#[test]
fn test_single_node_collation() {
    run_collation_test();
}
