/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Simplex receiver unit tests
//!
//! Tests receiver communication with multiple instances using in-process overlay.
//! Similar structure to `test_consensus.rs` and `catchain/tests/test_catchain_network.rs`
//!
//! Note: This test was moved from `tests/test_receiver.rs` to internal tests
//! as part of CODE-2 (receiver privatization). The test now uses `crate::`
//! imports to access internal types like `Receiver`, `ReceiverListener`, etc.

use crate::{
    receiver::{Receiver, ReceiverListener, ReceiverListenerPtr},
    ConsensusOverlayManagerPtr, PrivateKey, RawVoteData, SessionFactory, SessionId, SessionNode,
    ValidatorWeight,
};
use colored::Colorize;
use rand::Rng;
use std::{
    fs::{self, File},
    io::{stdout, LineWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use ton_api::{
    serialize_boxed,
    ton::{
        consensus::{
            candidatedata::Block as CandidateDataBlock,
            candidateid::CandidateId as TlCandidateId,
            simplex::{
                certificate::Certificate,
                unsignedvote::{FinalizeVote, SkipVote},
                vote::Vote as TlVote,
                votesignature::VoteSignature,
                votesignatureset::VoteSignatureSet,
                Certificate as CertificateBoxed, Vote as TlVoteBoxed,
            },
            CandidateData, CandidateParent,
        },
        validator_session::candidate::Candidate as TlCandidate,
    },
    IntoBoxed,
};
use ton_block::{error, sha256_digest, BlockIdExt, Ed25519KeyOption, Error, ShardIdent, UInt256};

include!("../../../../common/src/info.rs");

/*
    Test configuration
*/

/// Test configuration for receiver tests
#[derive(Clone, Debug)]
struct ReceiverTestConfig {
    /// Number of receiver instances
    receiver_count: usize,
    /// Number of votes to send from each receiver
    votes_per_receiver: u32,
    /// Number of broadcasts to send from each receiver
    broadcasts_per_receiver: u32,
    /// Acceptable loss factor (0.0 = no loss allowed, 0.1 = 10% loss allowed)
    acceptable_loss_factor: f64,
    /// Test name for logging
    test_name: String,
    /// Delay between sends in milliseconds
    send_delay_ms: u64,
    /// Wait time after sending for messages to propagate
    propagation_wait_ms: u64,
}

impl Default for ReceiverTestConfig {
    fn default() -> Self {
        Self {
            receiver_count: 5,
            votes_per_receiver: 10,
            broadcasts_per_receiver: 5,
            acceptable_loss_factor: 0.0,
            test_name: "receiver_test".to_string(),
            send_delay_ms: 10,
            propagation_wait_ms: 2000,
        }
    }
}

/*
    Test receiver listener
*/

/// Statistics collected by the receiver listener
struct ReceiverStats {
    /// Number of votes received
    votes_received: AtomicU32,
    /// Number of broadcasts received
    broadcasts_received: AtomicU32,
    /// Number of certificates received
    certificates_received: AtomicU32,
    /// Received certificates (for content assertions)
    received_certificates: Mutex<Vec<CertificateBoxed>>,
    /// Total active weight updates received
    active_weight_updates: AtomicU32,
    /// Last active weight value
    last_active_weight: AtomicU64,
    /// Receiver index for logging
    receiver_idx: u32,
}

impl ReceiverStats {
    fn new(receiver_idx: u32) -> Self {
        Self {
            votes_received: AtomicU32::new(0),
            broadcasts_received: AtomicU32::new(0),
            certificates_received: AtomicU32::new(0),
            received_certificates: Mutex::new(Vec::new()),
            active_weight_updates: AtomicU32::new(0),
            last_active_weight: AtomicU64::new(0),
            receiver_idx,
        }
    }
}

/// Test implementation of ReceiverListener
struct TestReceiverListener {
    stats: Arc<ReceiverStats>,
}

impl TestReceiverListener {
    fn create(receiver_idx: u32) -> (Arc<Self>, Arc<ReceiverStats>) {
        let stats = Arc::new(ReceiverStats::new(receiver_idx));
        let listener = Arc::new(Self { stats: stats.clone() });
        (listener, stats)
    }
}

impl ReceiverListener for TestReceiverListener {
    fn on_vote(&self, source_idx: u32, vote: TlVoteBoxed, _raw_vote: RawVoteData) {
        let count = self.stats.votes_received.fetch_add(1, Ordering::Relaxed) + 1;
        log::trace!(
            "Receiver {} received vote from source {}: count={}, vote={:?}",
            self.stats.receiver_idx,
            source_idx,
            count,
            vote
        );
    }

    fn on_candidate_received(
        &self,
        source_idx: u32,
        candidate: CandidateData,
        _notar_cert: Option<Vec<u8>>,
    ) {
        let count = self.stats.broadcasts_received.fetch_add(1, Ordering::Relaxed) + 1;
        log::trace!(
            "Receiver {} received candidate from source {}: count={}, slot={}",
            self.stats.receiver_idx,
            source_idx,
            count,
            candidate.slot()
        );
    }

    fn on_activity(&self, active_weight: ValidatorWeight, _last_activity: Vec<Option<SystemTime>>) {
        self.stats.active_weight_updates.fetch_add(1, Ordering::Relaxed);
        self.stats.last_active_weight.store(active_weight, Ordering::Relaxed);
        log::trace!(
            "Receiver {} activity updated: active_weight={}",
            self.stats.receiver_idx,
            active_weight
        );
    }

    fn on_certificate(&self, source_idx: u32, certificate: CertificateBoxed) {
        let count = self.stats.certificates_received.fetch_add(1, Ordering::Relaxed) + 1;
        self.stats.received_certificates.lock().unwrap().push(certificate.clone());

        log::trace!(
            "Receiver {} received certificate from source {}: count={}, {:?}",
            self.stats.receiver_idx,
            source_idx,
            count,
            certificate
        );
    }

    fn on_candidate_query_fallback(
        &self,
        _slot: crate::block::SlotIndex,
        _block_hash: UInt256,
        _want_notar: bool,
        response_callback: consensus_common::QueryResponseCallback,
    ) {
        log::trace!(
            "Receiver {} candidate_query_fallback: no-op (test mock)",
            self.stats.receiver_idx
        );
        response_callback(Err(error!("Not implemented in test mock")));
    }
}

impl Drop for TestReceiverListener {
    fn drop(&mut self) {
        log::info!("Dropping TestReceiverListener for receiver {}", self.stats.receiver_idx);
    }
}

/*
    Receiver instance
*/

/// A receiver instance for testing
struct ReceiverInstance {
    idx: u32,
    receiver: Arc<dyn Receiver + Send + Sync>,
    stats: Arc<ReceiverStats>,
    _listener: Arc<TestReceiverListener>,
    private_key: PrivateKey,
    session_id: SessionId,
}

impl ReceiverInstance {
    fn create(
        idx: u32,
        session_id: SessionId,
        nodes: &[SessionNode],
        private_key: PrivateKey,
        overlay_manager: ConsensusOverlayManagerPtr,
    ) -> Result<Self, Error> {
        let (listener, stats) = TestReceiverListener::create(idx);
        let listener_arc: Arc<dyn ReceiverListener + Send + Sync> = listener.clone();
        let listener_weak: ReceiverListenerPtr = Arc::downgrade(&listener_arc);

        // Use masterchain shard and default size limits for tests
        let shard = ShardIdent::masterchain();
        let max_candidate_size = 8 << 20; // 8 MB
        let panicked_flag = Arc::new(AtomicBool::new(false));

        let health_counters = Arc::new(crate::receiver::ReceiverHealthCounters::new());
        let receiver = crate::receiver::ReceiverWrapper::create(
            session_id.clone(),
            &shard,
            max_candidate_size,
            0,
            nodes,
            &private_key,
            overlay_manager,
            listener_weak,
            Duration::from_secs(10), // standstill_timeout
            panicked_flag,
            false,
            health_counters,
        )?;

        Ok(Self { idx, receiver, stats, _listener: listener, private_key, session_id })
    }

    fn send_vote(&self, slot: u32) {
        // Create a simple skip vote (simpler than notarize, no block_id needed)
        // Use TL types directly and sign WITH session wrapper (matches C++ pool.cpp)
        let unsigned_vote = SkipVote { slot: slot as i32 };
        let unsigned_vote_boxed = unsigned_vote.into_boxed();

        // Serialize for signing (boxed, as in C++)
        let serialized = consensus_common::serialize_tl_boxed_object!(&unsigned_vote_boxed);

        // Wrap in dataToSign and sign (matches C++ pool.cpp)
        let data_to_sign = crate::utils::create_data_to_sign(&self.session_id, &serialized);
        let signature = self.private_key.sign(&data_to_sign).expect("Failed to sign vote");

        let vote = TlVote { vote: unsigned_vote_boxed, signature: signature.to_vec().into() };

        self.receiver.send_vote(vote);
    }

    fn send_broadcast(&self, slot: u32) {
        // Create a block broadcast with properly serialized candidate data
        // Empty candidate bytes represent an empty block, which uses None for block_id in hash computation
        // To test non-empty blocks, we serialize a proper validatorSession.candidate TL object

        // Create a parent from previous slot (if slot > 0)
        // Using u32 for parent slot to avoid depending on internal SlotIndex type
        let parent_info: Option<(u32, &UInt256)> = if slot > 0 {
            None // TODO: Track actual parent hashes for multi-slot tests
        } else {
            None
        };
        let parent = CandidateParent::Consensus_CandidateWithoutParents;

        // Create dummy block data and serialize as validatorSession.candidate
        let block_data = vec![1u8, 2, 3, 4]; // Dummy data
        let collated_data = vec![]; // Empty collated data
        let root_hash = UInt256::from_slice(&sha256_digest(&block_data));

        let tl_inner = TlCandidate {
            src: UInt256::default(),
            round: slot as i32,
            root_hash: root_hash.clone(),
            data: block_data.clone().into(),
            collated_data: collated_data.clone().into(),
        };
        let candidate_bytes = consensus_common::serialize_tl_boxed_object!(&tl_inner.into_boxed());

        // Compute file_hash and collated_file_hash the same way as generated_block
        let file_hash = UInt256::from_slice(&sha256_digest(&block_data));
        let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data));

        // Create BlockIdExt for hash computation (matches receiver's extract_block_info_from_candidate)
        let block_id =
            BlockIdExt { shard_id: ShardIdent::masterchain(), seq_no: slot, root_hash, file_hash };

        // Compute candidate ID hash using u32 wrapper (same as receiver will compute)
        let candidate_hash = crate::utils::compute_candidate_id_hash_u32(
            slot,
            Some(&block_id),
            Some(&collated_file_hash),
            parent_info,
        );

        // Sign the candidate with session-scoped signature using u32 wrapper
        let signature = crate::utils::sign_candidate_u32(
            &self.session_id,
            slot,
            &candidate_hash,
            &self.private_key,
        )
        .expect("Failed to sign candidate");

        let broadcast = CandidateData::Consensus_Block(CandidateDataBlock {
            slot: slot as i32,
            candidate: candidate_bytes.into(),
            parent,
            signature: signature.into(),
        });

        self.receiver.send_block_broadcast(slot, candidate_hash, broadcast);
    }

    fn stop(&self) {
        self.receiver.stop();
    }
}

/*
    Test runner
*/

/// Run a parameterized receiver test
fn run_receiver_test<F>(
    config: ReceiverTestConfig,
    overlay_manager: ConsensusOverlayManagerPtr,
    post_test_functor: F,
) where
    F: FnOnce(&[ReceiverInstance]),
{
    // Initialize logger
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = SystemTime::now().into();
    let out_log_file_name = format!(
        "debug-simplex-receiver-{}-{}.log",
        config.test_name,
        datetime.format("%Y-%m-%d-%H.%M.%S")
    );
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

                    stdout().flush()
                }
            }
        })
        .filter_level(log::LevelFilter::Trace)
        .try_init()
        .unwrap_or_else(|_| {
            // Logger already initialized
        });

    log::info!("=== STARTING TEST: {} ===", config.test_name);
    log::info!(
        "Config: receivers={}, votes_per_receiver={}, broadcasts_per_receiver={}, loss_factor={}",
        config.receiver_count,
        config.votes_per_receiver,
        config.broadcasts_per_receiver,
        config.acceptable_loss_factor
    );

    // Generate session nodes
    let mut nodes = Vec::with_capacity(config.receiver_count);
    let mut private_keys = Vec::with_capacity(config.receiver_count);

    for _i in 0..config.receiver_count {
        let private_key = Ed25519KeyOption::generate().expect("Failed to generate private key");
        let adnl_id = private_key.id();

        nodes.push(SessionNode {
            adnl_id: adnl_id.clone(),
            public_key: private_key.clone(),
            weight: 1,
        });
        private_keys.push(private_key);
    }

    // Generate session ID
    let mut rng = rand::thread_rng();
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());

    log::info!("Session ID: {}", session_id.to_hex_string());

    // Create receivers (each receiver creates its own metrics receiver internally)
    let mut receivers = Vec::with_capacity(config.receiver_count);

    for i in 0..config.receiver_count {
        let receiver = ReceiverInstance::create(
            i as u32,
            session_id.clone(),
            &nodes,
            private_keys[i].clone(),
            overlay_manager.clone(),
        )
        .expect("Failed to create receiver");

        log::info!("Created receiver {}", i);
        receivers.push(receiver);
    }

    // Wait for receivers to initialize
    thread::sleep(Duration::from_millis(5000));

    // Send votes from each receiver
    log::info!("Sending {} votes from each receiver...", config.votes_per_receiver);
    for slot in 0..config.votes_per_receiver {
        for receiver in &receivers {
            receiver.send_vote(slot);
        }
        if config.send_delay_ms > 0 {
            thread::sleep(Duration::from_millis(config.send_delay_ms));
        }
    }

    // Send broadcasts from each receiver
    log::info!("Sending {} broadcasts from each receiver...", config.broadcasts_per_receiver);
    for slot in 0..config.broadcasts_per_receiver {
        for receiver in &receivers {
            receiver.send_broadcast(slot);
        }
        if config.send_delay_ms > 0 {
            thread::sleep(Duration::from_millis(config.send_delay_ms));
        }
    }

    // Wait for messages to propagate
    log::info!("Waiting {}ms for message propagation...", config.propagation_wait_ms);
    thread::sleep(Duration::from_millis(config.propagation_wait_ms));

    // Collect and log statistics
    log::info!("=== STATISTICS ===");

    let n = config.receiver_count as u32;
    // Expected votes: each receiver sends votes_per_receiver votes to (n-1) other receivers
    let expected_votes_per_receiver = config.votes_per_receiver * (n - 1);
    // Expected broadcasts: each receiver sends broadcasts_per_receiver broadcasts to (n-1) other receivers
    let expected_broadcasts_per_receiver = config.broadcasts_per_receiver * (n - 1);

    let mut total_votes_received = 0u32;
    let mut total_broadcasts_received = 0u32;

    for receiver in &receivers {
        let votes = receiver.stats.votes_received.load(Ordering::Relaxed);
        let broadcasts = receiver.stats.broadcasts_received.load(Ordering::Relaxed);

        log::info!(
            "Receiver {}: votes={}/{} (expected), broadcasts={}/{} (expected)",
            receiver.idx,
            votes,
            expected_votes_per_receiver,
            broadcasts,
            expected_broadcasts_per_receiver
        );

        total_votes_received += votes;
        total_broadcasts_received += broadcasts;
    }

    let total_expected_votes = expected_votes_per_receiver * n;
    let total_expected_broadcasts = expected_broadcasts_per_receiver * n;

    log::info!(
        "Total: votes={}/{}, broadcasts={}/{}",
        total_votes_received,
        total_expected_votes,
        total_broadcasts_received,
        total_expected_broadcasts
    );

    // Stop all receivers
    log::info!("Stopping receivers...");
    for receiver in &receivers {
        receiver.stop();
    }

    // Wait for shutdown
    thread::sleep(Duration::from_millis(500));

    // Call post-test functor
    post_test_functor(&receivers);

    // Assert expected values with acceptable loss
    let min_expected_votes =
        ((total_expected_votes as f64) * (1.0 - config.acceptable_loss_factor)) as u32;
    let min_expected_broadcasts =
        ((total_expected_broadcasts as f64) * (1.0 - config.acceptable_loss_factor)) as u32;

    log::info!(
        "Assertions: min_votes={}, min_broadcasts={}",
        min_expected_votes,
        min_expected_broadcasts
    );

    assert!(
        total_votes_received >= min_expected_votes,
        "Not enough votes received: {} < {} (min expected with {}% loss)",
        total_votes_received,
        min_expected_votes,
        config.acceptable_loss_factor * 100.0
    );

    assert!(
        total_broadcasts_received >= min_expected_broadcasts,
        "Not enough broadcasts received: {} < {} (min expected with {}% loss)",
        total_broadcasts_received,
        min_expected_broadcasts,
        config.acceptable_loss_factor * 100.0
    );

    // Assert no errors were logged during the test
    let errors = error_count.load(Ordering::Relaxed);
    assert!(
        errors == 0,
        "Test failed: {} ERROR log message(s) were emitted during the test. Check logs for details.",
        errors
    );

    log::info!("=== FINISHED TEST: {} ===", config.test_name);
}

/*
    Test cases
*/

#[test]
fn test_receiver_basic() {
    let overlay_manager = SessionFactory::create_in_process_overlay_manager(5);

    run_receiver_test(
        ReceiverTestConfig {
            receiver_count: 5,
            votes_per_receiver: 10,
            broadcasts_per_receiver: 5,
            acceptable_loss_factor: 0.0,
            test_name: "receiver_basic".to_string(),
            send_delay_ms: 20,
            propagation_wait_ms: 3000,
        },
        overlay_manager,
        |_receivers| {
            // Basic test: just verify message counts
        },
    );
}

/// Test candidate resolver: emulates situation where late-joining receivers
/// request candidates they missed via the candidate resolver mechanism.
///
/// Flow:
/// 1. Create receiver 0 and broadcast a candidate
/// 2. Wait for broadcast to be cached
/// 3. Create receivers 1 and 2 (they missed the broadcast)
/// 4. Receivers 1 and 2 request the candidate via request_candidate()
/// 5. Assert all receivers have received the candidate
#[test]
fn test_receiver_candidate_resolver() {
    // Initialize logging for test
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Trace).try_init();

    let overlay_manager = SessionFactory::create_in_process_overlay_manager(3);
    let session_id = UInt256::rand();

    // Create 3 validators
    let keys: Vec<_> =
        (0..3).map(|_| Ed25519KeyOption::generate().expect("Failed to generate key")).collect();
    let nodes: Vec<SessionNode> = keys
        .iter()
        .map(|k| SessionNode { public_key: k.clone(), adnl_id: k.id().clone(), weight: 1 })
        .collect();

    // Use masterchain shard and default size limits
    let shard = ShardIdent::masterchain();
    let max_candidate_size = 8 << 20; // 8 MB

    // === Step 1: Create receiver 0 and broadcast a candidate ===
    log::info!("Step 1: Creating receiver 0 and broadcasting candidate...");

    let (listener0, stats0) = TestReceiverListener::create(0);
    let listener0_arc: Arc<dyn ReceiverListener + Send + Sync> = listener0.clone();
    let panicked_flag0 = Arc::new(AtomicBool::new(false));
    let health_counters0 = Arc::new(crate::receiver::ReceiverHealthCounters::new());
    let receiver0 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[0],
        overlay_manager.clone(),
        Arc::downgrade(&listener0_arc),
        Duration::from_secs(10),
        panicked_flag0,
        false,
        health_counters0,
    )
    .expect("Failed to create receiver 0");

    // Wait for receiver to initialize
    thread::sleep(Duration::from_millis(500));

    // Create and send a block broadcast
    let slot = 5u32;
    let block_data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    let collated_data: Vec<u8> = vec![];
    let root_hash = UInt256::from_slice(&sha256_digest(&block_data));
    let file_hash = UInt256::from_slice(&sha256_digest(&block_data));
    let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data));

    // Serialize as validatorSession.candidate
    let tl_inner = TlCandidate {
        src: UInt256::default(),
        round: slot as i32,
        root_hash: root_hash.clone(),
        data: block_data.clone().into(),
        collated_data: collated_data.clone().into(),
    };
    let candidate_bytes = consensus_common::serialize_tl_boxed_object!(&tl_inner.into_boxed());

    // Create BlockIdExt for hash computation
    let block_id = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: slot,
        root_hash: root_hash.clone(),
        file_hash: file_hash.clone(),
    };

    // Compute candidate ID hash
    let candidate_hash = crate::utils::compute_candidate_id_hash_u32(
        slot,
        Some(&block_id),
        Some(&collated_file_hash),
        None, // no parent
    );

    // Sign the candidate
    let signature = crate::utils::sign_candidate_u32(&session_id, slot, &candidate_hash, &keys[0])
        .expect("Failed to sign candidate");

    let broadcast = CandidateData::Consensus_Block(CandidateDataBlock {
        slot: slot as i32,
        candidate: candidate_bytes.into(),
        parent: CandidateParent::Consensus_CandidateWithoutParents,
        signature: signature.into(),
    });

    // Send the broadcast (will be cached in receiver 0's resolver cache)
    receiver0.send_block_broadcast(slot, candidate_hash.clone(), broadcast);
    // requestCandidate currently asks for both candidate+notar. Seed notar in
    // resolver cache so late joiners can complete merged CandidateAndCert.
    receiver0.cache_notarization_cert(slot, candidate_hash.clone(), vec![0xAA, 0xBB, 0xCC]);
    log::info!(
        "Receiver 0 broadcast candidate for slot {} with hash {}",
        slot,
        &candidate_hash.to_hex_string()[..8]
    );

    // Wait for broadcast to be processed and cached
    thread::sleep(Duration::from_millis(500));

    // === Step 2: Wait to simulate late joiners ===
    log::info!("Step 2: Waiting 4 seconds to simulate late-joining receivers...");
    thread::sleep(Duration::from_secs(4));

    // === Step 3: Create receivers 1 and 2 (they missed the broadcast) ===
    log::info!("Step 3: Creating receivers 1 and 2 (late joiners)...");

    let (listener1, stats1) = TestReceiverListener::create(1);
    let listener1_arc: Arc<dyn ReceiverListener + Send + Sync> = listener1.clone();
    let panicked_flag1 = Arc::new(AtomicBool::new(false));
    let health_counters1 = Arc::new(crate::receiver::ReceiverHealthCounters::new());
    let receiver1 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[1],
        overlay_manager.clone(),
        Arc::downgrade(&listener1_arc),
        Duration::from_secs(10),
        panicked_flag1,
        false,
        health_counters1,
    )
    .expect("Failed to create receiver 1");

    let (listener2, stats2) = TestReceiverListener::create(2);
    let listener2_arc: Arc<dyn ReceiverListener + Send + Sync> = listener2.clone();
    let panicked_flag2 = Arc::new(AtomicBool::new(false));
    let health_counters2 = Arc::new(crate::receiver::ReceiverHealthCounters::new());
    let receiver2 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[2],
        overlay_manager.clone(),
        Arc::downgrade(&listener2_arc),
        Duration::from_secs(10),
        panicked_flag2,
        false,
        health_counters2,
    )
    .expect("Failed to create receiver 2");

    // Wait for receivers to initialize
    thread::sleep(Duration::from_millis(1000));

    // === Step 4: Receivers 1 and 2 request the candidate ===
    log::info!("Step 4: Receivers 1 and 2 requesting candidate via candidate resolver...");

    receiver1.request_candidate(slot, candidate_hash.clone());
    receiver2.request_candidate(slot, candidate_hash.clone());

    // Wait for request/response cycle (includes network + processing time)
    log::info!("Waiting for candidate resolver requests to complete...");
    thread::sleep(Duration::from_secs(10));

    // === Step 5: Assert all receivers have the candidate ===
    log::info!("Step 5: Checking results...");

    let r0_broadcasts = stats0.broadcasts_received.load(Ordering::Relaxed);
    let r1_broadcasts = stats1.broadcasts_received.load(Ordering::Relaxed);
    let r2_broadcasts = stats2.broadcasts_received.load(Ordering::Relaxed);

    log::info!("Receiver 0: broadcasts_received = {}", r0_broadcasts);
    log::info!("Receiver 1: broadcasts_received = {}", r1_broadcasts);
    log::info!("Receiver 2: broadcasts_received = {}", r2_broadcasts);

    // Receiver 0 should NOT receive its own broadcast (it's the sender)
    // Receivers 1 and 2 should each receive the candidate via request_candidate response

    // Stop all receivers
    log::info!("Stopping receivers...");
    receiver0.stop();
    receiver1.stop();
    receiver2.stop();
    thread::sleep(Duration::from_millis(500));

    // Assert receivers 1 and 2 received the candidate
    assert!(
        r1_broadcasts >= 1,
        "Receiver 1 should have received the candidate via resolver, got {}",
        r1_broadcasts
    );
    assert!(
        r2_broadcasts >= 1,
        "Receiver 2 should have received the candidate via resolver, got {}",
        r2_broadcasts
    );

    println!("✓ Candidate resolver test passed: late-joining receivers successfully retrieved missed candidate");
}

// ============================================================================
// Certificate send + standstill re-broadcast tests
// ============================================================================

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timeout waiting for condition");
}

fn make_skip_certificate(slot: u32) -> CertificateBoxed {
    let vote = SkipVote { slot: slot as i32 }.into_boxed();
    let sigs = vec![VoteSignature { who: 0, signature: vec![1, 2, 3].into() }.into_boxed()];
    let sig_set = VoteSignatureSet { votes: sigs.into() }.into_boxed();

    Certificate { vote, signatures: sig_set }.into_boxed()
}

fn make_finalize_certificate(slot: u32) -> CertificateBoxed {
    let id = TlCandidateId { slot: slot as i32, hash: UInt256::rand() }.into_boxed();

    let vote = FinalizeVote { id }.into_boxed();
    let sigs = vec![VoteSignature { who: 0, signature: vec![9, 9, 9].into() }.into_boxed()];
    let sig_set = VoteSignatureSet { votes: sigs.into() }.into_boxed();

    Certificate { vote, signatures: sig_set }.into_boxed()
}

fn certificate_slot(cert: &CertificateBoxed) -> u32 {
    let tl_vote = match cert {
        CertificateBoxed::Consensus_Simplex_Certificate(inner) => &inner.vote,
    };
    let vote = crate::utils::tl_unsigned_to_vote(tl_vote).expect("failed to parse unsigned vote");
    match vote {
        crate::simplex_state::Vote::Notarize(v) => v.slot.value(),
        crate::simplex_state::Vote::Finalize(v) => v.slot.value(),
        crate::simplex_state::Vote::Skip(v) => v.slot.value(),
        crate::simplex_state::Vote::NotarizeFallback(v) => v.slot.value(),
        crate::simplex_state::Vote::SkipFallback(v) => v.slot.value(),
    }
}

#[test]
fn test_certificate_serialization_matches_boxed_macro() {
    let cert = make_skip_certificate(1);
    let bytes_ton_api = serialize_boxed(&cert).expect("serialize_boxed");
    let bytes_macro = consensus_common::serialize_tl_boxed_object!(&cert);
    assert_eq!(
        bytes_ton_api, bytes_macro,
        "certificate serialization must be identical between ton_api and serialize_tl_boxed_object!"
    );
}

#[test]
fn test_receiver_send_certificate_and_standstill_rebroadcasts_cached_certificates() {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Trace).try_init();

    let overlay_manager = SessionFactory::create_in_process_overlay_manager(2);
    let session_id = UInt256::rand();

    let keys: Vec<_> =
        (0..2).map(|_| Ed25519KeyOption::generate().expect("Failed to generate key")).collect();
    let nodes: Vec<SessionNode> = keys
        .iter()
        .map(|k| SessionNode { public_key: k.clone(), adnl_id: k.id().clone(), weight: 1 })
        .collect();

    let shard = ShardIdent::masterchain();
    let max_candidate_size = 8 << 20;

    // Create receivers with short standstill timeout to simulate retransmission
    let (listener0, _stats0) = TestReceiverListener::create(0);
    let listener0_arc: Arc<dyn ReceiverListener + Send + Sync> = listener0.clone();
    let receiver0 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[0],
        overlay_manager.clone(),
        Arc::downgrade(&listener0_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 0");

    let (listener1, stats1) = TestReceiverListener::create(1);
    let listener1_arc: Arc<dyn ReceiverListener + Send + Sync> = listener1.clone();
    let receiver1 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[1],
        overlay_manager.clone(),
        Arc::downgrade(&listener1_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 1");

    thread::sleep(Duration::from_millis(500));

    // 1) Send a certificate normally
    let cert = make_skip_certificate(1);
    receiver0.send_certificate(cert.clone());

    wait_until(Duration::from_secs(2), || {
        stats1.certificates_received.load(Ordering::Relaxed) >= 1
    });

    // 2) Cache certificates for standstill replay (simulate "retransmission")
    let cert_bytes = serialize_boxed(&cert).expect("serialize cert");
    receiver0.cache_standstill_certificate(
        1,
        crate::receiver::StandstillCertificateType::Skip,
        cert_bytes,
    );

    let last_final = make_finalize_certificate(99);
    let last_final_bytes = serialize_boxed(&last_final).expect("serialize last_final");
    receiver0.cache_last_final_certificate(99, last_final_bytes);

    receiver0.set_standstill_slots(0, 10);
    receiver0.reschedule_standstill();

    wait_until(Duration::from_secs(3), || {
        stats1.certificates_received.load(Ordering::Relaxed) >= 3
    });

    // Stop receivers ASAP to avoid repeated alarm triggers affecting counts
    receiver0.stop();
    receiver1.stop();
    thread::sleep(Duration::from_millis(200));

    let received = stats1.received_certificates.lock().unwrap().clone();
    let slots: Vec<u32> = received.iter().map(certificate_slot).collect();

    let slot1_count = slots.iter().filter(|&&s| s == 1).count();
    assert!(
        slot1_count >= 2,
        "expected certificate for slot 1 to be received at least twice (send + standstill replay), got {:?}",
        slots
    );
    assert!(
        slots.iter().any(|&s| s == 99),
        "expected last_final certificate (slot 99) to be replayed on standstill, got {:?}",
        slots
    );
}

#[test]
fn test_receiver_standstill_rebroadcasts_cached_local_votes() {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Trace).try_init();

    let overlay_manager = SessionFactory::create_in_process_overlay_manager(2);
    let session_id = UInt256::rand();

    let keys: Vec<_> =
        (0..2).map(|_| Ed25519KeyOption::generate().expect("Failed to generate key")).collect();
    let nodes: Vec<SessionNode> = keys
        .iter()
        .map(|k| SessionNode { public_key: k.clone(), adnl_id: k.id().clone(), weight: 1 })
        .collect();

    let shard = ShardIdent::masterchain();
    let max_candidate_size = 8 << 20;

    // Create receivers with short standstill timeout to simulate retransmission
    let (listener0, _stats0) = TestReceiverListener::create(0);
    let listener0_arc: Arc<dyn ReceiverListener + Send + Sync> = listener0.clone();
    let receiver0 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[0],
        overlay_manager.clone(),
        Arc::downgrade(&listener0_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 0");

    let (listener1, stats1) = TestReceiverListener::create(1);
    let listener1_arc: Arc<dyn ReceiverListener + Send + Sync> = listener1.clone();
    let receiver1 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[1],
        overlay_manager.clone(),
        Arc::downgrade(&listener1_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 1");

    thread::sleep(Duration::from_millis(500));

    // Cache a signed local vote WITHOUT sending it (restart-style)
    let vote = crate::simplex_state::Vote::Skip(crate::simplex_state::SkipVote {
        slot: crate::block::SlotIndex::new(3),
    });
    let tl_vote = crate::utils::sign_vote(&vote, &session_id, &keys[0]).expect("sign_vote failed");
    let signed = match tl_vote {
        TlVoteBoxed::Consensus_Simplex_Vote(inner) => inner,
    };
    receiver0.cache_our_vote_for_standstill(signed);

    receiver0.set_standstill_slots(0, 10);
    receiver0.reschedule_standstill();

    wait_until(Duration::from_secs(3), || stats1.votes_received.load(Ordering::Relaxed) >= 1);

    // Stop receivers ASAP to avoid repeated alarm triggers affecting counts
    receiver0.stop();
    receiver1.stop();
    thread::sleep(Duration::from_millis(200));

    assert!(
        stats1.votes_received.load(Ordering::Relaxed) >= 1,
        "expected receiver 1 to receive at least one vote via standstill re-broadcast"
    );
}

#[test]
fn test_receiver_standstill_cache_does_not_overwrite_existing_certificate() {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Trace).try_init();

    let overlay_manager = SessionFactory::create_in_process_overlay_manager(2);
    let session_id = UInt256::rand();

    let keys: Vec<_> =
        (0..2).map(|_| Ed25519KeyOption::generate().expect("Failed to generate key")).collect();
    let nodes: Vec<SessionNode> = keys
        .iter()
        .map(|k| SessionNode { public_key: k.clone(), adnl_id: k.id().clone(), weight: 1 })
        .collect();

    let shard = ShardIdent::masterchain();
    let max_candidate_size = 8 << 20;

    let (listener0, _stats0) = TestReceiverListener::create(0);
    let listener0_arc: Arc<dyn ReceiverListener + Send + Sync> = listener0.clone();
    let receiver0 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[0],
        overlay_manager.clone(),
        Arc::downgrade(&listener0_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 0");

    let (listener1, stats1) = TestReceiverListener::create(1);
    let listener1_arc: Arc<dyn ReceiverListener + Send + Sync> = listener1.clone();
    let receiver1 = crate::receiver::ReceiverWrapper::create(
        session_id.clone(),
        &shard,
        max_candidate_size,
        0,
        &nodes,
        &keys[1],
        overlay_manager.clone(),
        Arc::downgrade(&listener1_arc),
        Duration::from_millis(200),
        Arc::new(AtomicBool::new(false)),
        false,
        Arc::new(crate::receiver::ReceiverHealthCounters::new()),
    )
    .expect("Failed to create receiver 1");

    thread::sleep(Duration::from_millis(500));

    let cert1 = make_skip_certificate(1);
    let cert2 = make_skip_certificate(2);

    let bytes1 = serialize_boxed(&cert1).expect("serialize cert1");
    let bytes2 = serialize_boxed(&cert2).expect("serialize cert2");

    // Cache cert1, then attempt to overwrite with cert2 (should be ignored)
    receiver0.cache_standstill_certificate(
        1,
        crate::receiver::StandstillCertificateType::Skip,
        bytes1,
    );
    receiver0.cache_standstill_certificate(
        1,
        crate::receiver::StandstillCertificateType::Skip,
        bytes2,
    );

    receiver0.set_standstill_slots(0, 3);
    receiver0.reschedule_standstill();

    wait_until(Duration::from_secs(3), || {
        stats1.certificates_received.load(Ordering::Relaxed) >= 1
    });

    receiver0.stop();
    receiver1.stop();
    thread::sleep(Duration::from_millis(200));

    let received = stats1.received_certificates.lock().unwrap().clone();
    let slots: Vec<u32> = received.iter().map(certificate_slot).collect();
    assert!(
        slots.iter().all(|&s| s == 1),
        "expected only slot 1 certificate to be replayed (no overwrite), got {:?}",
        slots
    );
}
