/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Simplex consensus protocol integration tests
//!
//! Tests multi-instance consensus with in-process and ADNL overlays.
//! Similar structure to `validator-session/tests/test_accelerated_consensus_session.rs`

use colored::Colorize;
use consensus_common::{
    node_test_network::NodeTestNetwork, ConsensusCommonFactory, ConsensusOverlayManagerPtr,
    ResolverPurpose,
};
use lazy_static::lazy_static;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use simplex::*;
use spin::mutex::SpinMutex;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, LineWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use ton_api::{
    ton::validator_session::{candidate::Candidate, Candidate as CandidateBoxed},
    IntoBoxed,
};
use ton_block::{
    error, sha256_digest, BlockIdExt, BlockSignaturesVariant, BocFlags, BocReader, BocWriter,
    BuilderData, Ed25519KeyOption, ShardIdent, UInt256,
};

/*
    Test serialization mutex

    Prevents parallel execution of simplex consensus tests.
    Each test acquires this lock and holds it for the duration of the test.
    Reference: node/consensus-common/src/node_test_network.rs
*/
lazy_static! {
    static ref SIMPLEX_TEST_MUTEX: Mutex<()> = Mutex::new(());
}

/// Upper bound for resolver-probe repair requests in integration tests.
const RESOLVER_PROBE_MAX_REQUESTS: u32 = 256;

include!("../../../common/src/info.rs");

fn session_start_args(
    shard: &ShardIdent,
    initial_block_seqno: u32,
) -> (Vec<BlockIdExt>, BlockIdExt) {
    (
        vec![BlockIdExt::with_params(
            shard.clone(),
            initial_block_seqno.saturating_sub(1),
            UInt256::default(),
            UInt256::default(),
        )],
        BlockIdExt::with_params(
            ShardIdent::masterchain(),
            0,
            UInt256::default(),
            UInt256::default(),
        ),
    )
}

/*
    Overlay type configuration
*/

/// Overlay type for test configuration
#[derive(Clone, Debug)]
enum OverlayType {
    /// In-process overlay (fast, no real network)
    InProcess,
    /// ADNL overlay (real network stack, localhost)
    Adnl,
}

/*
    Test data structures
*/

/// Dummy collated data for testing
#[derive(Serialize, Deserialize)]
struct DummyCollatedData {
    creation_timestamp: u64,
    slot: u32,
    seqno: u32,
    source_index: u32,
}

impl DummyCollatedData {
    fn new(slot: u32, seqno: u32, source_index: u32) -> Self {
        Self {
            creation_timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            slot,
            seqno,
            source_index,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        // Wrap in single-cell BOC — compress_candidate_data requires valid BOC input
        let raw = bincode::serialize(self).unwrap();
        let mut b = BuilderData::new();
        b.append_raw(&raw, raw.len() * 8).unwrap();
        let cell = b.into_cell().unwrap();
        let mut buf = Vec::new();
        BocWriter::with_flags([cell], BocFlags::all()).unwrap().write(&mut buf).unwrap();
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        // Extract from BOC wrapper
        let boc = BocReader::new().read(bytes).unwrap();
        let cell = &boc.roots[0];
        let raw = cell.data();
        bincode::deserialize(raw).unwrap()
    }
}

/*
    Test configuration
*/

/// Test configuration structure containing all configurable parameters
#[derive(Clone, Debug)]
struct TestConfig {
    /// Total number of rounds to complete (commits + skips)
    total_rounds: u32,
    /// Minimum percentage of commits required (0.0 - 1.0)
    /// Default 0.5 means at least 50% of rounds must be commits (not skips)
    min_finalized_percent: f64,
    /// Number of validator nodes in the test
    node_count: usize,
    /// Probability of generation failure (0.0 - 1.0)
    generation_failure_probability: f64,
    /// Probability of candidate rejection (0.0 - 1.0)
    candidate_rejection_probability: f64,
    /// Maximum number of collations before ignoring requests
    max_collations: u32,
    /// Target rate between blocks
    target_rate: Duration,
    /// First block timeout
    first_block_timeout: Duration,
    /// Test name for logging and database naming
    test_name: String,
    /// Test timeout - if exceeded, test behavior depends on expect_timeout
    test_timeout: Duration,
    /// If true, test passes when timeout occurs (for unimplemented features)
    /// If false, test fails when timeout occurs
    expect_timeout: bool,
    /// Shard identifier for the session (masterchain or specific shard)
    /// Default: masterchain
    shard: ShardIdent,
    /// Interval between MC finalization notifications for shard sessions
    /// If None, no MC notification thread is started (for masterchain sessions)
    /// If Some(duration), a thread periodically calls notify_mc_finalized
    mc_notification_interval: Option<Duration>,
    /// Overlay type (in-process or ADNL)
    overlay_type: OverlayType,
    /// Optional net-gremlin: temporarily disable networking for selected nodes.
    ///
    /// Only supported for `OverlayType::Adnl`.
    net_gremlin: Option<NetGremlinConfig>,
    /// Restart gremlin configuration for restart chaos testing.
    ///
    /// Randomly stops and restarts sessions to test recovery from persistent storage.
    /// Supports both `OverlayType::InProcess` and `OverlayType::Adnl`.
    restart_gremlin: Option<RestartGremlinConfig>,
    /// Lossy overlay configuration for network impairment simulation.
    ///
    /// When set, wraps the overlay manager with `LossyOverlayManager` to simulate
    /// packet loss and latency. Can be used alongside toggleable overlay (net-gremlin).
    /// Default: None (no loss/delay).
    lossy_overlay: Option<consensus_common::LossyOverlayOpts>,
    /// Which node indices receive the lossy overlay. If None, ALL nodes get lossy overlay
    /// (backward compatible). If Some(vec), only specified indices are wrapped.
    /// Used by FinalCert-recovery gremlin to target specific nodes.
    lossy_overlay_node_indices: Option<Vec<usize>>,
    /// Override standstill timeout (default: 10s from SessionOptions).
    /// Restart tests benefit from a shorter interval so recovered nodes
    /// receive cached certificates faster than the skip timeout.
    standstill_timeout: Option<Duration>,
    /// Override slots_per_leader_window (default: 1).
    /// Set > 1 to test candidate chaining within a leader window.
    /// Precollation depth is derived automatically as (window - 1).
    slots_per_leader_window: Option<u32>,
}

/// Network gremlin configuration (net-gremlin).
///
/// This simulates temporary network partitions by disabling a node's overlay
/// networking (inbound + outbound) for some duration.
#[derive(Clone, Debug)]
struct NetGremlinConfig {
    /// How long to keep a node disabled.
    disable_duration: Duration,
    /// How long to wait between disable cycles (after re-enabling).
    disable_interval: Duration,
    /// Total number of disable cycles (disable+enable). If 0, runs until test completes.
    max_cycles: u32,
    /// RNG seed for deterministic scheduling.
    seed: u64,
}

/// Restart gremlin configuration (restart-gremlin).
///
/// This simulates node restarts by stopping and restarting sessions while
/// preserving the DB path (enabling state recovery from persistent storage).
/// Mirrors C++ `run_gremlin` in test-consensus.cpp.
#[derive(Clone, Debug)]
struct RestartGremlinConfig {
    /// How long a node stays down after stop before restart.
    downtime: Duration,
    /// How long to wait between restart cycles (after restart completes).
    restart_interval: Duration,
    /// Total number of restart cycles. If 0, runs until test completes.
    max_cycles: u32,
    /// RNG seed for deterministic scheduling.
    seed: u64,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            total_rounds: 100,
            min_finalized_percent: 0.5, // At least 50% commits (not skips)
            node_count: 11,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 200,
            target_rate: Duration::from_millis(200),
            first_block_timeout: Duration::from_millis(1000),
            test_name: "simplex_consensus".to_string(),
            test_timeout: Duration::from_secs(120),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None, // No MC notifications for masterchain
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        }
    }
}

/*
    Latency statistics
*/

/// Latency statistics for a single instance
#[derive(Debug, Clone)]
struct LatencyStats {
    count: usize,
    min: f64,
    max: f64,
    median: f64,
    mean: f64,
    sigma: f64, // standard deviation
    ci_95_low: f64,
    ci_95_high: f64,
}

impl LatencyStats {
    /// Compute statistics from a vector of latency samples (in milliseconds)
    fn compute(samples: &[u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }

        let count = samples.len();
        let mut sorted: Vec<f64> = samples.iter().map(|&x| x as f64).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min = sorted[0];
        let max = sorted[count - 1];

        // Median
        let median = if count % 2 == 0 {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        } else {
            sorted[count / 2]
        };

        // Mean
        let sum: f64 = sorted.iter().sum();
        let mean = sum / count as f64;

        // Standard deviation (sigma)
        let variance: f64 = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count as f64;
        let sigma = variance.sqrt();

        // 95% confidence interval (using t-distribution approximation for small samples)
        // For large n, z_95 ≈ 1.96
        let z_95 = 1.96;
        let margin = z_95 * sigma / (count as f64).sqrt();
        let ci_95_low = mean - margin;
        let ci_95_high = mean + margin;

        Some(Self { count, min, max, median, mean, sigma, ci_95_low, ci_95_high })
    }

    /// Format as a table row
    fn to_table_row(&self, instance_idx: usize) -> String {
        format!(
            "| {:^8} | {:^7} | {:^10.2} | {:^10.2} | {:^10.2} | {:^10.2} | {:^10.2} | {:^22} |",
            instance_idx,
            self.count,
            self.min,
            self.max,
            self.median,
            self.mean,
            self.sigma,
            format!("[{:.2}, {:.2}]", self.ci_95_low, self.ci_95_high)
        )
    }
}

/// Print latency statistics table header
fn print_latency_table_header() {
    log::info!(
        "+----------+---------+------------+------------+------------+------------+------------+------------------------+"
    );
    log::info!(
        "| {:^8} | {:^7} | {:^10} | {:^10} | {:^10} | {:^10} | {:^10} | {:^22} |",
        "Instance",
        "Count",
        "Min (ms)",
        "Max (ms)",
        "Med (ms)",
        "Avg (ms)",
        "σ (ms)",
        "95% CI (ms)"
    );
    log::info!(
        "+----------+---------+------------+------------+------------+------------+------------+------------------------+"
    );
}

/// Print latency statistics table footer
fn print_latency_table_footer() {
    log::info!(
        "+----------+---------+------------+------------+------------+------------+------------+------------------------+"
    );
}

/*
    Session instance
*/

/// Shared storage of finalized block root hashes for harness-level introspection.
/// All instances share one set via Arc<Mutex<..>>.
type FinalizedBlocksMap = Arc<Mutex<HashSet<UInt256>>>;

/// Session instance for a single validator node
struct SessionInstance {
    source_index: u32,
    public_key: PublicKey,
    batch_processed: Arc<AtomicBool>,
    collation_requested: Arc<AtomicBool>,
    is_collator: Arc<AtomicBool>,
    collation_count: Arc<AtomicU32>,
    on_candidate_count: Arc<AtomicU32>,
    on_candidate_observed_count: Arc<AtomicU32>,
    parent_missing_observed_count: Arc<AtomicU32>,
    resolver_probe_requests_count: Arc<AtomicU32>,
    on_block_finalized_count: Arc<AtomicU32>,
    config: TestConfig,
    /// Finalization latencies in milliseconds (for statistical analysis)
    commit_latencies: Arc<Mutex<Vec<u64>>>,
    /// Maximum finalized seqno observed so far.  Updated atomically in
    /// `on_block_finalized`; used for restart-gremlin recovery progress calculation.
    max_finalized_seqno: Arc<AtomicU32>,
    /// Set of seqnos for which a finalization has been delivered.
    /// Invariant: every seqno appears at most once (asserted in `on_block_finalized`).
    finalized_seqnos: Arc<Mutex<HashSet<u32>>>,
    /// Session errors count
    session_errors_count: Arc<AtomicU32>,
    /// Shared finalized block roots for harness-level introspection.
    finalized_blocks: FinalizedBlocksMap,
    simplex_session: Arc<dyn SimplexSession + Send + Sync>,
    _session: SessionPtr,
    _listener: Arc<dyn SessionListener + Send + Sync>,
}

/// Listener wrapper that delegates to SessionInstance
struct SessionInstanceListener {
    instance: SpinMutex<Weak<SpinMutex<SessionInstance>>>,
    /// Maximum finalized seqno - shared with SessionInstance, available immediately.
    max_finalized_seqno: Arc<AtomicU32>,
    /// Finalized seqnos set - shared with SessionInstance, available immediately.
    finalized_seqnos: Arc<Mutex<HashSet<u32>>>,
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

    fn on_candidate_observed_count(&self) -> u32 {
        self.on_candidate_observed_count.load(Ordering::Relaxed)
    }

    fn parent_missing_observed_count(&self) -> u32 {
        self.parent_missing_observed_count.load(Ordering::Relaxed)
    }

    fn resolver_probe_requests_count(&self) -> u32 {
        self.resolver_probe_requests_count.load(Ordering::Relaxed)
    }

    fn on_block_finalized_count(&self) -> u32 {
        self.on_block_finalized_count.load(Ordering::Relaxed)
    }

    fn unique_finalized_seqno_count(&self) -> usize {
        self.finalized_seqnos.lock().map(|s| s.len()).unwrap_or(0)
    }

    fn session_errors_count(&self) -> u32 {
        self.session_errors_count.load(Ordering::Relaxed)
    }

    fn get_latency_stats(&self) -> Option<LatencyStats> {
        if let Ok(latencies) = self.commit_latencies.lock() {
            LatencyStats::compute(&latencies)
        } else {
            None
        }
    }

    fn finish_slot(&self, slot: u32) {
        let finalized = self.on_block_finalized_count.load(Ordering::SeqCst);
        if finalized >= self.config.total_rounds {
            self.batch_processed.store(true, Ordering::Release);
            log::info!(
                "Test finished after {} finalized blocks for source #{} (slot={})",
                finalized,
                self.source_index,
                slot
            );
        }
    }
}

/*
    SessionListener implementation for SessionInstance
*/

impl SessionListener for SessionInstance {
    fn on_candidate(
        &self,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        // SIMPLEX_ROUNDLESS: Assert round is always u32::MAX
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "on_candidate: round must be SIMPLEX_ROUNDLESS in roundless mode"
        );
        self.on_candidate_count.fetch_add(1, Ordering::Relaxed);

        let now =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64;
        let collated_data = DummyCollatedData::from_bytes(data.data());
        let latency = now - collated_data.creation_timestamp;
        // Extract slot from the embedded collated data (set by collator)
        let slot = collated_data.slot;

        log::info!(
            "SessionListener::on_candidate: new candidate for \
            slot {} from source {} with hash {:?} appeared with latency {} ms (self source #{})",
            slot,
            source_info.source.id(),
            root_hash,
            latency,
            self.source_index
        );

        let mut rng = rand::thread_rng();

        // Check if candidate should be rejected based on probability
        if rng.gen::<f64>() < self.config.candidate_rejection_probability {
            log::warn!(
                "SessionListener::on_candidate: rejecting candidate for slot {} from source {} (simulated rejection, self source #{})",
                slot,
                source_info.source.id(),
                self.source_index
            );
            callback(Err(error!(
                "Simulated candidate rejection for slot {} from source {}",
                slot,
                source_info.source.id()
            )));
            return;
        }

        callback(Ok(SystemTime::now()))
    }

    fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        self.on_candidate_observed_count.fetch_add(1, Ordering::Relaxed);
        if !flags.parent_ready {
            self.parent_missing_observed_count.fetch_add(1, Ordering::Relaxed);
        }

        // Runtime resolver-probe used by the ghost-parent integration test:
        // emulate validator-side demand requests for candidate chain availability.
        let resolver_probe_enabled = self.config.test_name == "simplex_ghost_parent_resolver_probe";
        if !resolver_probe_enabled || self.source_index != 0 || !flags.body_present {
            return;
        }

        let current_requests = self.resolver_probe_requests_count.load(Ordering::Relaxed);
        if current_requests >= RESOLVER_PROBE_MAX_REQUESTS {
            return;
        }

        self.resolver_probe_requests_count.fetch_add(1, Ordering::Relaxed);
        self.simplex_session.ensure_candidate_available(
            block_id.clone(),
            EnsureCandidateAvailabilityOptions {
                purpose: ResolverPurpose::SimplexCollationParent,
                include_parent_chain: true,
            },
        );
        log::info!(
            target: "simplex_resolver",
            "resolver probe request source_idx={} block_id={} parent_ready={} local_collated={}",
            self.source_index,
            block_id,
            flags.parent_ready,
            flags.local_collated
        );
    }

    fn on_generate_slot(
        &self,
        source_info: simplex::BlockSourceInfo,
        request: simplex::AsyncRequestPtr,
        parent: consensus_common::CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        // SIMPLEX_ROUNDLESS: Assert round is always u32::MAX
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "on_generate_slot: round must be SIMPLEX_ROUNDLESS in roundless mode"
        );
        let request_id = request.get_request_id();

        // CAS check: atomically set is_collator from false to true
        if !self.is_collator.swap(true, Ordering::Relaxed) {
            log::info!("COLLATOR STATUS: Node became collator (source #{})", self.source_index);
        }

        // Check if we've reached the maximum number of collations
        let current_count = self.collation_count.fetch_add(1, Ordering::SeqCst);
        if current_count >= self.config.max_collations {
            log::warn!(
                "SessionListener::on_generate_slot: ignoring collation request (reached max collations: {}, source #{})",
                self.config.max_collations,
                self.source_index
            );
            return;
        }

        let mut rng = rand::thread_rng();

        // Check if generation should fail based on probability
        if rng.gen::<f64>() < self.config.generation_failure_probability {
            log::warn!(
                "SessionListener::on_generate_slot: generation failed (simulated failure, source #{})",
                self.source_index
            );
            self.collation_requested.store(true, Ordering::Release);
            callback(Err(error!("Simulated generation failure")));
            return;
        }

        let seqno = match &parent {
            consensus_common::CollationParentHint::Implicit => {
                panic!("Simplex consensus test must not receive implicit parent hints")
            }
            consensus_common::CollationParentHint::Explicit(parent_ids) => {
                parent_ids.iter().map(|id| id.seq_no).max().unwrap_or(0) + 1
            }
        };

        // Use seqno as the slot value for embedded data (since slot isn't exposed in API)
        let slot_for_data = seqno;

        log::info!(
            "SessionListener::on_generate_slot: collator request for seqno {} with request ID {} (source #{})",
            seqno,
            request_id,
            self.source_index
        );

        // Generate random root_hash (represents block content hash)
        let mut root_hash_bytes = [0; 32];
        for byte in &mut root_hash_bytes {
            *byte = rng.gen_range(0..255);
        }
        let root_hash = UInt256::with_array(root_hash_bytes);

        // Create block data with slot (using seqno as proxy), seqno, and source for validation tracking
        let collated_data_payload = DummyCollatedData::new(slot_for_data, seqno, self.source_index);
        let data_bytes = collated_data_payload.to_bytes();
        let collated_bytes = vec![]; // Empty collated data

        // Compute file_hash and collated_file_hash from SHA256 of actual data
        // This must match what the receiver computes in extract_block_info_from_candidate
        let file_hash = UInt256::from_slice(&sha256_digest(&data_bytes));
        let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_bytes));

        log::debug!("on_generate_slot: seqno={} (source #{})", seqno, self.source_index);

        let candidate = Arc::new(ValidatorBlockCandidate {
            public_key: self.public_key.clone(),
            id: BlockIdExt::with_params(
                ShardIdent::masterchain(),
                seqno, // Use tracked seqno
                root_hash.clone(),
                file_hash,
            ),
            collated_file_hash,
            data: consensus_common::ConsensusCommonFactory::create_block_payload(data_bytes),
            collated_data: consensus_common::ConsensusCommonFactory::create_block_payload(
                collated_bytes,
            ),
        });

        self.collation_requested.store(true, Ordering::Release);

        callback(Ok(candidate));
    }

    fn on_block_committed(
        &self,
        _source_info: simplex::BlockSourceInfo,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: consensus_common::SessionStats,
    ) {
        panic!(
            "on_block_committed must not be called for Simplex sessions (finalized-driven only)"
        );
    }

    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "on_block_finalized: round must be SIMPLEX_ROUNDLESS in roundless mode"
        );
        let slot = match &signatures {
            BlockSignaturesVariant::Simplex(s) => s.slot,
            _ => 0,
        };

        let seqno = block_id.seq_no();
        self.on_block_finalized_count.fetch_add(1, Ordering::Relaxed);

        let now =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64;

        let data_bytes = data.data();
        let latency = if !data_bytes.is_empty() {
            let collated_data = DummyCollatedData::from_bytes(data_bytes);
            now - collated_data.creation_timestamp
        } else {
            0
        };

        if let Ok(mut latencies) = self.commit_latencies.lock() {
            latencies.push(latency);
        }

        log::info!(
            "SessionListener::on_block_finalized: block from source {} hash {:?} \
            finalized at slot={}, seqno={}, latency={} ms (source #{})",
            source_info.source.id(),
            root_hash,
            slot,
            seqno,
            latency,
            self.source_index
        );

        if let Ok(mut map) = self.finalized_blocks.lock() {
            map.insert(root_hash.clone());
        }

        self.finish_slot(slot);
    }

    fn on_block_skipped(&self, _round: u32) {
        unreachable!("on_block_skipped should not be called in Simplex");
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        panic!(
            "unexpected legacy get_approved_candidate request in simplex consensus test \
             (source #{}, root_hash={}); active simplex flow must not use this callback",
            self.source_index,
            root_hash.to_hex_string()
        );
    }
}

/*
    SessionListener delegation wrapper
*/

impl SessionListener for SessionInstanceListener {
    fn on_candidate(
        &self,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        // SIMPLEX_ROUNDLESS: Assert round is always u32::MAX
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "SessionInstanceListener::on_candidate: round must be SIMPLEX_ROUNDLESS"
        );
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_candidate(source_info, root_hash, data, collated_data, callback);
        }
    }

    fn on_generate_slot(
        &self,
        source_info: simplex::BlockSourceInfo,
        request: simplex::AsyncRequestPtr,
        parent: consensus_common::CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        // SIMPLEX_ROUNDLESS: Assert round is always u32::MAX
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "SessionInstanceListener::on_generate_slot: round must be SIMPLEX_ROUNDLESS"
        );
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_generate_slot(source_info, request, parent, callback);
        }
    }

    fn on_block_committed(
        &self,
        _source_info: simplex::BlockSourceInfo,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: consensus_common::SessionStats,
    ) {
        panic!(
            "on_block_committed must not be called for Simplex sessions (finalized-driven only)"
        );
    }

    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: simplex::BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        assert_eq!(
            source_info.priority.round, SIMPLEX_ROUNDLESS,
            "SessionInstanceListener::on_block_finalized: round must be SIMPLEX_ROUNDLESS"
        );

        let data_bytes = data.data();
        let is_empty = data_bytes.is_empty();
        let slot = match &signatures {
            BlockSignaturesVariant::Simplex(s) => s.slot,
            _ => 0,
        };

        if !is_empty {
            let seqno = block_id.seq_no();
            // Atomically advance max_finalized_seqno (out-of-order safe).
            self.max_finalized_seqno.fetch_max(seqno + 1, Ordering::SeqCst);
            log::trace!(
                "SessionInstanceListener::on_block_finalized: slot={}, seqno={}",
                slot,
                seqno
            );
        } else {
            log::trace!("SessionInstanceListener::on_block_finalized: slot={}, empty block", slot);
        }

        // Invariant: exactly one finalization per seqno (checked even before
        // SessionInstance is wired, so recovery duplicates are caught too).
        if let Ok(mut seen) = self.finalized_seqnos.lock() {
            let seqno = block_id.seq_no();
            assert!(
                seen.insert(seqno),
                "DUPLICATE finalization for seqno {} (listener, slot={})",
                seqno,
                slot
            );
        }

        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_block_finalized(
                block_id,
                source_info,
                root_hash,
                file_hash,
                data,
                signatures,
                approve_signatures,
            );
        }
    }

    fn on_block_skipped(&self, round: u32) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_block_skipped(round);
        }
    }

    fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        if let Some(instance) = self.instance.lock().upgrade() {
            let instance = instance.lock();
            instance.on_candidate_observed(block_id, data, collated_data, flags);
        }
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        root_hash: BlockHash,
        _file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        panic!(
            "unexpected legacy get_approved_candidate request in simplex consensus listener \
             (root_hash={}); active simplex flow must not use this callback",
            root_hash.to_hex_string()
        );
    }
}

/*
    Test runner
*/

/// Generalized test function that runs simplex consensus tests with configurable parameters
fn run_simplex_consensus_test<F>(config: TestConfig, post_test_functor: F)
where
    F: FnOnce(&Vec<Arc<SpinMutex<SessionInstance>>>),
{
    // Acquire global test mutex to prevent parallel test execution
    // The lock is held for the duration of the test and released on drop
    let _test_lock = SIMPLEX_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    const DB_PATH: &str = "../../target/test";

    // Initialize logger - same log file for all tests
    if !is_test_logging_enabled() {
        return;
    }

    let datetime: chrono::DateTime<chrono::offset::Utc> = SystemTime::now().into();
    let out_log_file_name =
        format!("debug-simplex-consensus-tests-{}.log", datetime.format("%Y-%m-%d-%H.%M.%S"));
    let logs_path = Path::new("../../target/logs");
    fs::create_dir_all(logs_path).expect("unable to create output log path");
    let log_file_path = logs_path.join(&out_log_file_name);
    let file = File::create(&log_file_path).expect("unable to create output log file");
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
            let thread_id = thread::current().id();

            // Write to the log file first, then drop the lock to avoid blocking other threads
            // on stdout formatting / printing (important for TRACE-heavy debug sessions).
            {
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
            }

            let (message, _level) = match record.level() {
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
                            "{} [{: <5}] - {:?} - {: <45}| {}",
                            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                            l,
                            thread_id,
                            source,
                            message
                        )?;

                        io::stdout().flush()
                    } else {
                        Ok(())
                    }
                }
            }
        })
        .filter_level(log::LevelFilter::Info)
        .filter_module("simplex", log::LevelFilter::Trace)
        .filter_module("test_consensus", log::LevelFilter::Trace)
        .try_init()
        .unwrap_or_else(|_| {
            // Logger already initialized, which is fine for multiple tests
        });

    log::info!("Log file: {}", log_file_path.display());
    log::info!("=== STARTING TEST: {} ===", config.test_name);

    // Create overlay infrastructure based on config
    let (nodes, overlay_managers, test_network_opt): (
        Vec<SessionNode>,
        Vec<ConsensusOverlayManagerPtr>,
        Option<NodeTestNetwork>,
    ) = match &config.overlay_type {
        OverlayType::InProcess => {
            // Generate random nodes with in-process overlay
            let mut nodes = Vec::with_capacity(config.node_count);
            for _i in 0..config.node_count {
                let private_key =
                    Ed25519KeyOption::generate().expect("Failed to generate private key");
                let adnl_id = private_key.id();
                let node =
                    SessionNode { adnl_id: adnl_id.clone(), public_key: private_key, weight: 1 };
                nodes.push(node);
            }

            // Create single shared overlay manager
            let overlay_threads_count = config.node_count;
            let overlay_manager =
                SessionFactory::create_in_process_overlay_manager(overlay_threads_count);
            let overlay_managers = vec![overlay_manager; config.node_count];

            (nodes, overlay_managers, None)
        }
        OverlayType::Adnl => {
            // Create ADNL test network
            const NUM_THREADS_PER_NODE: usize = 10;
            const IS_TCP_ENABLED: bool = false;
            let test_network = NodeTestNetwork::create_no_auto_shutdown(
                &config.test_name,
                config.node_count,
                NUM_THREADS_PER_NODE,
                IS_TCP_ENABLED,
            );

            // Generate nodes with ADNL IDs from test network
            let mut nodes = Vec::with_capacity(config.node_count);
            let mut overlay_managers = Vec::with_capacity(config.node_count);

            for i in 0..config.node_count {
                let test_node = test_network.get_node(i);
                // Use the ADNL key from the test network - this key is already registered
                // with the ADNL node and will be used for message signing and routing
                let private_key = test_node.stack.adnl.key_by_tag(test_node.adnl_tag).unwrap();
                let adnl_id = private_key.id().clone();

                let node =
                    SessionNode { adnl_id: adnl_id.clone(), public_key: private_key, weight: 1 };
                nodes.push(node);
                overlay_managers.push(test_node.overlay_manager.clone());
            }

            (nodes, overlay_managers, Some(test_network))
        }
    };

    // Wrap overlay managers with lossy overlay if configured.
    // This adds packet loss and delay simulation on top of the regular overlay.
    // Layering: Base (InProcess/ADNL) -> Toggleable (net-gremlin, if ADNL) -> Lossy (loss/delay)
    // The lossy wrapper is transparent to sessions - they just use ConsensusOverlayManager.
    let overlay_managers: Vec<ConsensusOverlayManagerPtr> = if let Some(lossy_opts) =
        &config.lossy_overlay
    {
        let target_nodes = &config.lossy_overlay_node_indices;
        log::info!(
            "Wrapping overlay managers with lossy overlay: {:?} (nodes: {:?})",
            lossy_opts,
            target_nodes.as_ref().map_or("all".to_string(), |v| format!("{:?}", v))
        );
        overlay_managers
            .into_iter()
            .enumerate()
            .map(|(idx, manager)| {
                let apply = target_nodes.as_ref().map_or(true, |indices| indices.contains(&idx));
                if apply {
                    ConsensusCommonFactory::create_lossy_overlay_manager(
                        manager,
                        lossy_opts.clone(),
                    )
                } else {
                    manager
                }
            })
            .collect()
    } else {
        overlay_managers
    };

    // Generate session ID and paths
    // Note: Each node needs its own unique db_path because they all run in the same process
    // and would otherwise try to lock the same RocksDB database files
    let rand_name: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path_base = format!("{}/simplex_test_{}_{}", DB_PATH, config.test_name, rand_name);
    let mut rng = rand::thread_rng();
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());

    // Session options
    let slots_per_window = config.slots_per_leader_window.unwrap_or(1);
    let mut session_opts = SessionOptions {
        proto_version: 0,
        target_rate: config.target_rate,
        first_block_timeout: config.first_block_timeout,
        slots_per_leader_window: slots_per_window,
        empty_block_mc_lag_threshold: if config.shard.is_masterchain() {
            None // MC uses internal finalization tracking
        } else {
            Some(8) // Shard default
        },
        ..Default::default()
    };
    // Integration stress tests can temporarily run far ahead in slot space while still
    // converging; keep a wide future-slot horizon to avoid synthetic test stalls.
    session_opts.max_leader_window_desync = 10_000;
    if let Some(st) = config.standstill_timeout {
        session_opts.standstill_timeout = st;
    }

    let finalized_blocks: FinalizedBlocksMap = Arc::new(Mutex::new(HashSet::new()));

    // Create session instances
    let mut instances = Vec::with_capacity(config.node_count);

    for i in 0..config.node_count {
        let local_key = nodes[i].public_key.clone();
        let initial_block_seqno = 1u32; // First block seqno=1 (seqno 0 is zerostate)

        let max_finalized_seqno = Arc::new(AtomicU32::new(initial_block_seqno));
        let finalized_seqnos: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));

        let listener = Arc::new(SessionInstanceListener {
            instance: SpinMutex::new(Weak::new()),
            max_finalized_seqno: max_finalized_seqno.clone(),
            finalized_seqnos: finalized_seqnos.clone(),
        });
        let session_listener: Arc<dyn SessionListener + Send + Sync> = listener.clone();

        let shard = config.shard.clone();
        let overlay_manager = overlay_managers[i].clone();
        let db_path = format!("{}_node{}", db_path_base, i);

        let session = SessionFactory::create_session(
            &session_opts,
            &session_id,
            &shard,
            nodes.clone(),
            &local_key,
            db_path,
            overlay_manager,
            Arc::downgrade(&session_listener),
        )
        .unwrap();
        let (prev_blocks, min_masterchain_block_id) =
            session_start_args(&shard, initial_block_seqno);
        session.start(prev_blocks, min_masterchain_block_id);
        let simplex_session = session.clone() as Arc<dyn SimplexSession + Send + Sync>;

        let session_instance = Arc::new(SpinMutex::new(SessionInstance {
            public_key: nodes[i].public_key.clone(),
            batch_processed: Arc::new(AtomicBool::new(false)),
            collation_requested: Arc::new(AtomicBool::new(false)),
            collation_count: Arc::new(AtomicU32::new(0)),
            on_candidate_count: Arc::new(AtomicU32::new(0)),
            on_candidate_observed_count: Arc::new(AtomicU32::new(0)),
            parent_missing_observed_count: Arc::new(AtomicU32::new(0)),
            resolver_probe_requests_count: Arc::new(AtomicU32::new(0)),
            on_block_finalized_count: Arc::new(AtomicU32::new(0)),
            is_collator: Arc::new(AtomicBool::new(false)),
            config: config.clone(),
            commit_latencies: Arc::new(Mutex::new(Vec::new())),
            max_finalized_seqno: max_finalized_seqno.clone(),
            finalized_seqnos: finalized_seqnos.clone(),
            session_errors_count: Arc::new(AtomicU32::new(0)),
            finalized_blocks: finalized_blocks.clone(),
            simplex_session,
            source_index: i as u32,
            _session: session,
            _listener: listener.clone(),
        }));

        *listener.instance.lock() = Arc::downgrade(&session_instance);

        instances.push(session_instance);
    }

    // ADNL startup delay - wait for overlay connections to stabilize
    // This is necessary because ADNL nodes need time to discover each other
    // and establish reliable connections before consensus messages are exchanged
    if matches!(config.overlay_type, OverlayType::Adnl) {
        const ADNL_STARTUP_DELAY: Duration = Duration::from_secs(3);
        log::info!(
            "Waiting {:?} for ADNL overlay to stabilize before starting consensus...",
            ADNL_STARTUP_DELAY
        );
        thread::sleep(ADNL_STARTUP_DELAY);
        log::info!("ADNL overlay stabilization complete, starting consensus");
    }

    // MC notification thread for shard sessions
    let mc_thread_stop_requested = Arc::new(AtomicBool::new(false));
    let mc_thread_stopped = Arc::new(AtomicBool::new(false));
    let mut mc_thread_handle: Option<thread::JoinHandle<()>> = if let Some(mc_interval) =
        config.mc_notification_interval
    {
        // Collect weak pointers to all sessions
        let session_targets: Vec<(Weak<dyn SimplexSession + Send + Sync>, ShardIdent)> = instances
            .iter()
            .map(|inst| {
                let guard = inst.lock();
                (
                    Arc::downgrade(&guard._session) as Weak<dyn SimplexSession + Send + Sync>,
                    guard.config.shard.clone(),
                )
            })
            .collect();

        let stop_requested = mc_thread_stop_requested.clone();
        let stopped = mc_thread_stopped.clone();
        let test_name = config.test_name.clone();

        Some(thread::spawn(move || {
            log::info!("[MC-Thread] Started MC notification thread for test '{}'", test_name);
            let mut mc_seqno: u32 = 0;

            while !stop_requested.load(Ordering::SeqCst) {
                thread::sleep(mc_interval);

                if stop_requested.load(Ordering::SeqCst) {
                    break;
                }

                // Notify all sessions about MC finalization
                let mut notified_count = 0;
                for (weak_session, shard) in &session_targets {
                    if let Some(session) = weak_session.upgrade() {
                        let mc_registered_top = BlockIdExt::with_params(
                            shard.clone(),
                            mc_seqno,
                            UInt256::rand(),
                            UInt256::rand(),
                        );
                        session.notify_mc_finalized(mc_registered_top);
                        notified_count += 1;
                    }
                }

                log::debug!(
                    "[MC-Thread] Notified {} sessions about MC block seqno={}",
                    notified_count,
                    mc_seqno
                );
                mc_seqno += 1;
            }

            stopped.store(true, Ordering::SeqCst);
            log::info!("[MC-Thread] MC notification thread stopped");
        }))
    } else {
        mc_thread_stopped.store(true, Ordering::SeqCst); // No thread to stop
        None
    };

    // Wait for all instances to finish or timeout
    let test_start = Instant::now();
    let mut timed_out = false;

    // Net-gremlin runtime state (single-node partition toggles).
    struct NetGremlinState {
        rng: StdRng,
        disabled_node: Option<usize>,
        next_action_at: Instant,
        cycles_done: u32,
    }

    let mut net_gremlin_state = config.net_gremlin.as_ref().map(|cfg| NetGremlinState {
        rng: StdRng::seed_from_u64(cfg.seed),
        disabled_node: None,
        next_action_at: Instant::now() + cfg.disable_interval,
        cycles_done: 0,
    });

    // Restart-gremlin runtime state (session stop/restart cycles).
    #[derive(Clone)]
    enum RestartNodeState {
        /// No node currently being stopped.
        Idle,
        /// Called stop_async(), waiting for is_stopped() to return true.
        Stopping(usize),
        /// Node stopped, waiting for downtime before restart.
        Down(usize, Instant),
    }

    struct RestartGremlinState {
        rng: StdRng,
        /// Current node state.
        node_state: RestartNodeState,
        next_pick_at: Instant,
        cycles_done: u32,
    }

    // Session creation context for restart-gremlin (per-node parameters).
    #[derive(Clone)]
    struct SessionCreationContext {
        db_path: String,
        nodes: Arc<Vec<SessionNode>>,
        local_key: PrivateKey,
        overlay_manager: ConsensusOverlayManagerPtr,
        session_opts: SessionOptions,
        session_id: UInt256,
        shard: ShardIdent,
        initial_block_seqno: u32,
    }

    // Capture session creation contexts for potential restarts.
    let nodes_arc = Arc::new(nodes.clone());
    let session_contexts: Vec<SessionCreationContext> = (0..config.node_count)
        .map(|i| SessionCreationContext {
            db_path: format!("{}_node{}", db_path_base, i),
            nodes: nodes_arc.clone(),
            local_key: nodes[i].public_key.clone(),
            overlay_manager: overlay_managers[i].clone(),
            session_opts: session_opts.clone(),
            session_id: session_id.clone(),
            shard: config.shard.clone(),
            initial_block_seqno: 1,
        })
        .collect();

    let mut restart_gremlin_state =
        config.restart_gremlin.as_ref().map(|cfg| RestartGremlinState {
            rng: StdRng::seed_from_u64(cfg.seed),
            node_state: RestartNodeState::Idle,
            next_pick_at: Instant::now() + cfg.restart_interval,
            cycles_done: 0,
        });

    'main_loop: loop {
        // Fail fast if any session thread panicked.
        // Otherwise the test may stall waiting for progress that will never happen.
        for (idx, inst) in instances.iter().enumerate() {
            let inst = inst.lock();
            if inst._session.is_panicked() {
                log::error!(
                    "Test '{}' detected PANIC in session {} (instance idx={}, finished={}, finalized={})",
                    config.test_name,
                    session_id.to_hex_string(),
                    idx,
                    inst.is_finished(),
                    inst.on_block_finalized_count(),
                );
                panic!(
                    "Test '{}' failed: session panicked (instance idx={})",
                    config.test_name, idx
                );
            }
        }

        // Check timeout
        if test_start.elapsed() >= config.test_timeout {
            timed_out = true;
            log::warn!(
                "Test timeout reached after {:?} (expect_timeout={})",
                config.test_timeout,
                config.expect_timeout
            );
            break;
        }

        // Net-gremlin tick (ADNL only).
        if let (Some(cfg), Some(state)) = (config.net_gremlin.as_ref(), net_gremlin_state.as_mut())
        {
            if let Some(test_network) = test_network_opt.as_ref() {
                let now = Instant::now();
                if now >= state.next_action_at {
                    // Stop scheduling new cycles if max reached and nothing is currently disabled.
                    if cfg.max_cycles != 0
                        && state.cycles_done >= cfg.max_cycles
                        && state.disabled_node.is_none()
                    {
                        // no-op
                    } else if let Some(node_idx) = state.disabled_node.take() {
                        test_network.enable_node_network(node_idx);
                        state.cycles_done = state.cycles_done.saturating_add(1);
                        state.next_action_at = now + cfg.disable_interval;
                    } else {
                        // Choose a node to disable. Keep it simple: one random node at a time.
                        let node_idx = state.rng.gen_range(0..config.node_count);
                        test_network.disable_node_network(node_idx);
                        state.disabled_node = Some(node_idx);
                        state.next_action_at = now + cfg.disable_duration;
                    }
                }
            }
        }

        // Restart-gremlin tick (session stop/restart).
        if let (Some(cfg), Some(state)) =
            (config.restart_gremlin.as_ref(), restart_gremlin_state.as_mut())
        {
            let now = Instant::now();
            match state.node_state.clone() {
                RestartNodeState::Idle => {
                    // Check if we should pick a new node to stop.
                    if cfg.max_cycles != 0 && state.cycles_done >= cfg.max_cycles {
                        // All cycles done, no-op.
                    } else if now >= state.next_pick_at {
                        // Choose a random running node to stop.
                        let running_nodes: Vec<usize> = instances
                            .iter()
                            .enumerate()
                            .filter(|(_, inst)| !inst.lock().is_finished())
                            .map(|(idx, _)| idx)
                            .collect();

                        if !running_nodes.is_empty() {
                            let pick = state.rng.gen_range(0..running_nodes.len());
                            let node_idx = running_nodes[pick];

                            log::info!(
                                "[restart-gremlin] Initiating stop for node {} (will restart after {:?} downtime)",
                                node_idx,
                                cfg.downtime
                            );

                            // Initiate async stop (non-blocking).
                            instances[node_idx].lock()._session.stop_async();

                            state.node_state = RestartNodeState::Stopping(node_idx);
                        }
                    }
                }
                RestartNodeState::Stopping(node_idx) => {
                    // Poll until the session is fully stopped.
                    if instances[node_idx].lock()._session.is_stopped() {
                        log::info!(
                            "[restart-gremlin] Node {} stopped, starting {:?} downtime",
                            node_idx,
                            cfg.downtime
                        );
                        state.node_state = RestartNodeState::Down(node_idx, now);
                    }
                }
                RestartNodeState::Down(node_idx, down_since) => {
                    // Check if downtime has elapsed.
                    if now >= down_since + cfg.downtime {
                        log::info!(
                            "[restart-gremlin] Restarting node {} after {:?} downtime (cycle {})",
                            node_idx,
                            cfg.downtime,
                            state.cycles_done + 1
                        );

                        let ctx = &session_contexts[node_idx];

                        let (prev_next_seqno, prev_commits, finalized_seqnos) = {
                            let inst = instances[node_idx].lock();
                            (
                                inst.max_finalized_seqno.load(Ordering::SeqCst),
                                inst.on_block_finalized_count.load(Ordering::SeqCst),
                                inst.finalized_seqnos.clone(),
                            )
                        };

                        // Create seqno counters BEFORE listener - they will be updated by
                        // on_block_finalized during recovery, before SessionInstance is wired.
                        // Recovery callbacks may move it forward further before SessionInstance is wired.
                        // Preserve the finalized seqno set across restarts so the duplicate-finalization
                        // invariant remains global for the whole test, not just the latest process lifetime.
                        let max_finalized_seqno =
                            Arc::new(AtomicU32::new(prev_next_seqno.max(ctx.initial_block_seqno)));

                        let new_listener = Arc::new(SessionInstanceListener {
                            instance: SpinMutex::new(Weak::new()),
                            max_finalized_seqno: max_finalized_seqno.clone(),
                            finalized_seqnos: finalized_seqnos.clone(),
                        });
                        let session_listener: Arc<dyn SessionListener + Send + Sync> =
                            new_listener.clone();

                        let new_session = SessionFactory::create_session(
                            &ctx.session_opts,
                            &ctx.session_id,
                            &ctx.shard,
                            ctx.nodes.as_ref().clone(),
                            &ctx.local_key,
                            ctx.db_path.clone(),
                            ctx.overlay_manager.clone(),
                            Arc::downgrade(&session_listener),
                        );

                        match new_session {
                            Ok(session) => {
                                let (prev_blocks, min_masterchain_block_id) =
                                    session_start_args(&ctx.shard, ctx.initial_block_seqno);
                                session.start(prev_blocks, min_masterchain_block_id);
                                let simplex_session =
                                    session.clone() as Arc<dyn SimplexSession + Send + Sync>;
                                // Create a completely new SessionInstance with fresh state.
                                // The seqno trackers are shared with the listener - they were already
                                // updated by on_block_finalized during recovery (before this point).
                                let recovered_next_seqno =
                                    max_finalized_seqno.load(Ordering::SeqCst);
                                let recovered_commits = recovered_next_seqno
                                    .saturating_sub(ctx.initial_block_seqno)
                                    .max(prev_commits);
                                let recovered_finished = recovered_commits >= config.total_rounds;
                                log::info!(
                                    "[restart-gremlin] Node {} seqno after recovery: next_seqno={}, commits={} (prev_next_seqno={}, prev_commits={})",
                                    node_idx,
                                    recovered_next_seqno,
                                    recovered_commits,
                                    prev_next_seqno,
                                    prev_commits
                                );

                                let new_instance = Arc::new(SpinMutex::new(SessionInstance {
                                    public_key: ctx.local_key.clone(),
                                    batch_processed: Arc::new(AtomicBool::new(recovered_finished)),
                                    collation_requested: Arc::new(AtomicBool::new(false)),
                                    collation_count: Arc::new(AtomicU32::new(0)),
                                    on_candidate_count: Arc::new(AtomicU32::new(0)),
                                    on_candidate_observed_count: Arc::new(AtomicU32::new(0)),
                                    parent_missing_observed_count: Arc::new(AtomicU32::new(0)),
                                    resolver_probe_requests_count: Arc::new(AtomicU32::new(0)),
                                    on_block_finalized_count: Arc::new(AtomicU32::new(
                                        recovered_commits,
                                    )),
                                    is_collator: Arc::new(AtomicBool::new(false)),
                                    config: config.clone(),
                                    commit_latencies: Arc::new(Mutex::new(Vec::new())),
                                    max_finalized_seqno: max_finalized_seqno.clone(),
                                    finalized_seqnos: finalized_seqnos.clone(),
                                    session_errors_count: Arc::new(AtomicU32::new(0)),
                                    finalized_blocks: finalized_blocks.clone(),
                                    simplex_session,
                                    source_index: node_idx as u32,
                                    _session: session,
                                    _listener: new_listener.clone(),
                                }));

                                // Link the new listener to the new instance.
                                *new_listener.instance.lock() = Arc::downgrade(&new_instance);

                                // Replace the old instance in the instances vector.
                                // Note: We need to use unsafe or interior mutability since instances is a Vec.
                                // For now, we update the contents of the existing Arc<SpinMutex<SessionInstance>>.
                                {
                                    let mut old_inst = instances[node_idx].lock();
                                    let new_inst = new_instance.lock();
                                    old_inst.public_key = new_inst.public_key.clone();
                                    old_inst.batch_processed = new_inst.batch_processed.clone();
                                    old_inst.collation_requested =
                                        new_inst.collation_requested.clone();
                                    old_inst.collation_count = new_inst.collation_count.clone();
                                    old_inst.on_candidate_count =
                                        new_inst.on_candidate_count.clone();
                                    old_inst.on_candidate_observed_count =
                                        new_inst.on_candidate_observed_count.clone();
                                    old_inst.parent_missing_observed_count =
                                        new_inst.parent_missing_observed_count.clone();
                                    old_inst.resolver_probe_requests_count =
                                        new_inst.resolver_probe_requests_count.clone();
                                    old_inst.on_block_finalized_count =
                                        new_inst.on_block_finalized_count.clone();
                                    old_inst.is_collator = new_inst.is_collator.clone();
                                    old_inst.commit_latencies = new_inst.commit_latencies.clone();
                                    old_inst
                                        .max_finalized_seqno
                                        .clone_from(&new_inst.max_finalized_seqno);
                                    old_inst
                                        .finalized_seqnos
                                        .clone_from(&new_inst.finalized_seqnos);
                                    old_inst.session_errors_count =
                                        new_inst.session_errors_count.clone();
                                    old_inst.finalized_blocks = new_inst.finalized_blocks.clone();
                                    old_inst.simplex_session = new_inst.simplex_session.clone();
                                    old_inst._session = new_inst._session.clone();
                                    old_inst._listener = new_inst._listener.clone();
                                }
                                // Re-link listener to the original instances entry.
                                *new_listener.instance.lock() =
                                    Arc::downgrade(&instances[node_idx]);

                                log::info!(
                                    "[restart-gremlin] Node {} restarted successfully with fresh SessionInstance",
                                    node_idx
                                );
                            }
                            Err(e) => {
                                log::error!(
                                    "[restart-gremlin] Failed to restart node {}: {:?}",
                                    node_idx,
                                    e
                                );
                            }
                        }

                        state.cycles_done = state.cycles_done.saturating_add(1);
                        state.next_pick_at = now + cfg.restart_interval;
                        state.node_state = RestartNodeState::Idle;
                    }
                }
            }
        }

        let not_finished: Vec<usize> = instances
            .iter()
            .enumerate()
            .filter(|(_, inst)| !inst.lock().is_finished())
            .map(|(idx, _)| idx)
            .collect();

        if !not_finished.is_empty() {
            log::debug!(
                "Waiting for {} instance(s) to finish: {:?}",
                not_finished.len(),
                not_finished
            );
            thread::sleep(Duration::from_millis(200));
            continue 'main_loop;
        } else {
            log::info!("All instances {} reported finished", instances.len());
        }

        break;
    }

    // Helper function to stop all sessions and MC thread
    // Note: ADNL network shutdown is done separately at the very end (like catchain tests)
    let mut stop_sessions_and_mc = || {
        // Ensure net-gremlin doesn't leave the network disabled during shutdown.
        if let Some(state) = net_gremlin_state.as_mut() {
            if let Some(node_idx) = state.disabled_node.take() {
                if let Some(test_network) = test_network_opt.as_ref() {
                    test_network.enable_node_network(node_idx);
                }
            }
        }

        // Note: restart-gremlin may have a node in stopping/down state. We don't need to
        // restart it during shutdown - just log and clear the state.
        if let Some(state) = restart_gremlin_state.as_mut() {
            match &state.node_state {
                RestartNodeState::Idle => {}
                RestartNodeState::Stopping(node_idx) => {
                    log::info!(
                        "[restart-gremlin] Node {} was stopping during shutdown, skipping restart",
                        node_idx
                    );
                }
                RestartNodeState::Down(node_idx, _) => {
                    log::info!(
                        "[restart-gremlin] Node {} was down during shutdown, skipping restart",
                        node_idx
                    );
                }
            }
            state.node_state = RestartNodeState::Idle;
        }

        // Request async stop for all sessions (non-blocking)
        log::info!("Requesting async stop for all sessions...");
        for instance in instances.iter() {
            instance.lock()._session.stop_async();
        }

        // Request MC thread stop (non-blocking, like stop_async)
        mc_thread_stop_requested.store(true, Ordering::SeqCst);

        // Collect session references and call synchronous stop outside the lock
        let sessions: Vec<_> = instances.iter().map(|inst| inst.lock()._session.clone()).collect();

        for (idx, session) in sessions.iter().enumerate() {
            log::info!("Stopping session {} synchronously...", idx);
            session.stop();
            log::info!("Session {} stopped", idx);
        }

        log::info!("All {} sessions stopped", instances.len());

        // Wait for MC thread to stop and join it
        log::info!("Waiting for MC thread to stop...");
        let mc_stop_timeout = Instant::now();
        while !mc_thread_stopped.load(Ordering::SeqCst) {
            if mc_stop_timeout.elapsed() > Duration::from_secs(5) {
                log::warn!("MC thread did not stop within 5 seconds");
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Join the MC thread if it exists
        if let Some(handle) = mc_thread_handle.take() {
            if let Err(e) = handle.join() {
                log::warn!("MC thread join failed: {:?}", e);
            }
        }
        log::info!("MC thread stopped");
    };

    // Handle timeout vs completion
    if timed_out {
        if config.expect_timeout {
            log::info!(
                "Test '{}' passed: timeout expected and occurred (implementation pending)",
                config.test_name
            );
            stop_sessions_and_mc();
            // Shutdown ADNL test network at the very end (like catchain)
            if let Some(test_network) = test_network_opt {
                log::info!("Shutting down ADNL test network...");
                test_network.shutdown();
                log::info!("ADNL test network shutdown complete");
            }
            return;
        } else {
            log::error!(
                "Test '{}' TIMEOUT: {} did not complete within {:?}",
                config.test_name,
                config.test_name,
                config.test_timeout
            );
            stop_sessions_and_mc();
            // Shutdown ADNL before panic
            if let Some(test_network) = test_network_opt {
                log::info!("Shutting down ADNL test network...");
                test_network.shutdown();
                log::info!("ADNL test network shutdown complete");
            }
            panic!(
                "Test '{}' failed: timeout after {:?} but completion was expected",
                config.test_name, config.test_timeout
            );
        }
    } else if config.expect_timeout {
        stop_sessions_and_mc();
        // Shutdown ADNL before panic
        if let Some(test_network) = test_network_opt {
            log::info!("Shutting down ADNL test network...");
            test_network.shutdown();
            log::info!("ADNL test network shutdown complete");
        }
        panic!("Test '{}' failed: expected timeout but test completed normally", config.test_name);
    } else {
        log::info!("Test '{}' completed normally", config.test_name);
        stop_sessions_and_mc();
    }

    // Call post-test functor with all instances
    post_test_functor(&instances);

    // Log final statistics
    for (index, instance) in instances.iter().enumerate() {
        let inst = instance.lock();
        let is_finished = inst.is_finished();
        log::info!(
            "Instance {}: finished={}, collation_requested={}, collation_count={}, candidate_count={}, observed_count={}, parent_missing={}, resolver_requests={}, finalized_count={}",
            index,
            inst.is_finished(),
            inst.collation_requested(),
            inst.collation_count(),
            inst.on_candidate_count(),
            inst.on_candidate_observed_count(),
            inst.parent_missing_observed_count(),
            inst.resolver_probe_requests_count(),
            inst.on_block_finalized_count()
        );
        let finalized_count = inst.on_block_finalized_count();
        let unique_seqnos = inst.unique_finalized_seqno_count();
        drop(inst);
        assert!(is_finished);
        assert_eq!(
            finalized_count as usize, unique_seqnos,
            "Instance {}: finalized_count ({}) != unique seqno count ({}) — duplicate finalization detected",
            index, finalized_count, unique_seqnos
        );
    }

    // Log finalization latency statistics table
    log::info!("");
    log::info!("=== COMMIT LATENCY STATISTICS ===");
    print_latency_table_header();

    let mut all_latencies: Vec<u64> = Vec::new();
    for (index, instance) in instances.iter().enumerate() {
        let inst = instance.lock();
        let stats = inst.get_latency_stats();
        // Clone latencies before dropping the lock
        let instance_latencies: Vec<u64> =
            inst.commit_latencies.lock().map(|l| l.clone()).unwrap_or_default();
        drop(inst);

        if let Some(stats) = stats {
            log::info!("{}", stats.to_table_row(index));
        }
        // Collect all latencies for aggregate stats
        all_latencies.extend(instance_latencies);
    }

    print_latency_table_footer();

    // Print aggregate statistics across all instances
    if let Some(aggregate_stats) = LatencyStats::compute(&all_latencies) {
        log::info!("");
        log::info!(
            "Aggregate (all instances): count={}, min={:.2}ms, max={:.2}ms, median={:.2}ms, avg={:.2}ms, σ={:.2}ms, 95%CI=[{:.2}, {:.2}]ms",
            aggregate_stats.count,
            aggregate_stats.min,
            aggregate_stats.max,
            aggregate_stats.median,
            aggregate_stats.mean,
            aggregate_stats.sigma,
            aggregate_stats.ci_95_low,
            aggregate_stats.ci_95_high
        );
    }

    // Assert no session errors occurred during the test
    let total_errors: u32 = instances.iter().map(|inst| inst.lock().session_errors_count()).sum();
    assert!(
        total_errors == 0,
        "Test failed: {} session error(s) occurred during the test. Check logs for details.",
        total_errors
    );

    drop(instances);

    log::info!("=== FINISHED TEST: {} ===", config.test_name);

    // Shutdown ADNL test network at the very end (like catchain tests)
    // Reference: node/catchain/tests/test_catchain_network.rs
    if let Some(test_network) = test_network_opt {
        log::info!("Shutting down ADNL test network...");
        test_network.shutdown();
        log::info!("ADNL test network shutdown complete");
    }
}

/*
    Test cases
*/

/// Basic consensus test - runs to completion with finalization (in-process overlay)
#[test]
fn test_simplex_consensus_basic() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 100,
            min_finalized_percent: 0.5, // At least 50% commits
            node_count: 7,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 10000,
            target_rate: Duration::from_millis(200),
            first_block_timeout: Duration::from_millis(1000),
            test_name: "simplex_basic".to_string(),
            test_timeout: Duration::from_secs(180),
            expect_timeout: false, // Expect completion, not timeout
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None, // Masterchain - no MC notifications
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            // Verify commit rate meets minimum requirement
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Consensus test with simulated failures
#[test]
fn test_simplex_consensus_with_failures() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 30,
            min_finalized_percent: 0.3, // Lower threshold due to failures
            node_count: 11,
            generation_failure_probability: 0.1,
            candidate_rejection_probability: 0.1,
            max_collations: 150,
            target_rate: Duration::from_millis(300),
            first_block_timeout: Duration::from_millis(2000),
            test_name: "simplex_with_failures".to_string(),
            // This scenario includes randomized generation/rejection failures and can
            // occasionally complete just above 120s on loaded CI/containers.
            // Candidate repair pacing (C++-aligned partial-merge / per-peer dedup) can add
            // wall-clock; keep headroom so the harness does not flake under load.
            test_timeout: Duration::from_secs(240),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None, // Masterchain - no MC notifications
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            // Verify commit rate meets minimum requirement
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// FinalCert-recovery gremlin test
///
/// Validates that a node with heavy message loss recovers missing FinalCerts
/// via finalized delivery rather than relying on a direct recovery path.
///
/// Setup: 7 MC nodes. Node 0 gets 40% broadcast loss + 30% message loss.
/// Other nodes have no loss and form a stable 6/7 majority (threshold=5).
/// Node 0 will miss FinalizeVotes for some slots, hitting `WaitingForFinalCert`.
/// Recovery: the node eventually receives the finalization certificate from peers
/// and the finalized block is delivered via `on_block_finalized`.
#[test]
fn test_simplex_consensus_finalcert_recovery() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 60,
            min_finalized_percent: 0.3,
            node_count: 7,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 200,
            target_rate: Duration::from_millis(300),
            first_block_timeout: Duration::from_millis(2000),
            test_name: "simplex_finalcert_recovery".to_string(),
            test_timeout: Duration::from_secs(240),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: Some(consensus_common::LossyOverlayOpts {
                lost_broadcast_probability: 0.4,
                lost_message_probability: 0.3,
                lost_query_probability: 0.3,
                ..Default::default()
            }),
            lossy_overlay_node_indices: Some(vec![0]),
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            // All nodes (including the lossy node 0) must reach min commit count
            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                let errors = instance.lock().session_errors_count.load(Ordering::Relaxed);
                log::info!(
                    "[finalcert-recovery] Instance {}: {} commits, {} errors \
                     (min required: {}, lossy={})",
                    idx,
                    commits,
                    errors,
                    min_finalized,
                    idx == 0
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }

            let finalized_count =
                instances[0].lock().finalized_blocks.lock().map(|m| m.len()).unwrap_or(0);
            log::info!(
                "[finalcert-recovery] Shared finalized blocks map: {} entries",
                finalized_count
            );
            assert!(
                finalized_count > 0,
                "FinalizedBlocksMap should have entries from on_block_finalized"
            );
        },
    );
}

/// Shard consensus test with MC finalization notifications
///
/// Tests shard session behavior with periodic masterchain finalization notifications.
/// This triggers empty block generation when MC finalization lags behind shard seqno.
#[test]
fn test_simplex_consensus_shard_with_mc_notifications() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 100,
            min_finalized_percent: 0.5, // At least 50% commits
            node_count: 7,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 500,
            target_rate: Duration::from_millis(200),
            first_block_timeout: Duration::from_millis(1000),
            test_name: "simplex_shard_mc".to_string(),
            test_timeout: Duration::from_secs(180),
            expect_timeout: false,
            // Use a shard chain (workchain 0, full shard)
            shard: ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
            // MC notification every 1 second - simulates slow MC finalization
            mc_notification_interval: Some(Duration::from_secs(1)),
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            // Verify commit rate meets minimum requirement
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Basic consensus test with ADNL overlay (real localhost network)
///
/// This test uses the real ADNL network stack instead of the in-process overlay,
/// testing the full network path including message serialization and routing.
#[test]
fn test_simplex_consensus_adnl_overlay() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 50, // Fewer rounds due to higher network latency
            min_finalized_percent: 0.5,
            node_count: 5, // Smaller network for faster test
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 200,
            target_rate: Duration::from_millis(200), // Slower rate for network latency
            first_block_timeout: Duration::from_millis(1000),
            test_name: "simplex_adnl".to_string(),
            test_timeout: Duration::from_secs(180), // Longer timeout for ADNL
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::Adnl,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            // Verify commit rate meets minimum requirement
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Net-gremlin (network partition) test using ADNL overlay.
///
/// Temporarily disables overlay networking for random nodes while consensus is running.
#[test]
fn test_simplex_consensus_adnl_net_gremlin() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 30,
            min_finalized_percent: 0.4, // allow some skips under partitions
            node_count: 5,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 2000,
            target_rate: Duration::from_millis(200),
            first_block_timeout: Duration::from_millis(1200),
            test_name: "simplex_adnl_net_gremlin".to_string(),
            test_timeout: Duration::from_secs(180),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::Adnl,
            net_gremlin: Some(NetGremlinConfig {
                disable_duration: Duration::from_secs(1),
                disable_interval: Duration::from_secs(0),
                max_cycles: 3,
                seed: 0xC0FFEE,
            }),
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: None,
        },
        |instances| {
            // Verify commit rate meets minimum requirement (best-effort under partitions).
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Restart-gremlin test: randomly stop and restart sessions with DB persistence.
///
/// This test validates that sessions can correctly recover from persistent storage
/// after being stopped and restarted. Mirrors C++ `run_gremlin` in test-consensus.cpp.
///
#[test]
fn test_simplex_consensus_restart_gremlin() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 50,
            min_finalized_percent: 0.3,
            node_count: 5,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 2000,
            target_rate: Duration::from_millis(200),
            first_block_timeout: Duration::from_millis(1200),
            test_name: "simplex_restart_gremlin".to_string(),
            test_timeout: Duration::from_secs(180), // Longer timeout for restart cycles
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: Some(RestartGremlinConfig {
                downtime: Duration::from_secs(2),
                restart_interval: Duration::from_secs(5),
                max_cycles: 2,
                seed: 0xDEADBEEF,
            }),
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            // 1s rebroadcast cadence can flood restart-gremlin runs (large [begin,end) ranges),
            // causing receiver queues to explode and the test to stall intermittently.
            standstill_timeout: Some(Duration::from_secs(5)),
            slots_per_leader_window: None,
        },
        |instances| {
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "Instance {}: {} commits out of {} total_rounds",
                    idx,
                    commits,
                    config.total_rounds
                );
                // Note: restarted nodes may have fewer commits if they were down during commit phase.
                // We use a lower min_finalized_percent to account for this.
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/*
    Unit tests for hash consistency (signature verification)
*/

/// Test that collated_file_hash is computed consistently between signing and verification paths.
///
/// This test prevents regression of the bug where collator.rs set collated_file_hash to
/// UInt256::default() instead of computing sha256(collated_data), causing all signatures
/// to be invalid because the hash used for signing differed from verification.
///
/// The test simulates:
/// 1. Collator creating a BlockCandidate with properly computed collated_file_hash
/// 2. Serializing the candidate into TL format (as done in session_processor.rs)
/// 3. Deserializing and recomputing collated_file_hash (as done in receiver.rs)
/// 4. Verifying both hashes match
#[test]
fn test_collated_file_hash_consistency() {
    // Simulate block data and collated data
    let block_data = b"test block data for hash consistency verification";
    let collated_data = b"test collated data - this is what gets hashed";

    // 1. Compute hashes as the COLLATOR should (like C++ block::compute_file_hash)
    let file_hash = UInt256::from_slice(&sha256_digest(block_data));
    let collated_file_hash_signing = UInt256::from_slice(&sha256_digest(collated_data));

    // 2. Serialize into TL candidate format (simulating session_processor.rs signing path)
    let tl_candidate = Candidate {
        src: UInt256::default(),
        round: 1,
        root_hash: UInt256::from([0x42u8; 32]),
        data: block_data.to_vec(),
        collated_data: collated_data.to_vec(),
    };

    let serialized =
        consensus_common::serialize_tl_boxed_object!(&tl_candidate.clone().into_boxed());

    // 3. Deserialize and recompute hashes (simulating receiver.rs verification path)
    let deserialized =
        consensus_common::utils::deserialize_tl_boxed_object::<CandidateBoxed>(&serialized)
            .expect("Failed to deserialize TL candidate");

    let extracted_data = match &deserialized {
        CandidateBoxed::ValidatorSession_Candidate(c) => (&c.data, &c.collated_data),
        _ => panic!("Unexpected candidate variant"),
    };

    // Compute hashes as RECEIVER does (simulating extract_block_info_from_candidate in utils.rs)
    let file_hash_verification = UInt256::from_slice(&sha256_digest(extracted_data.0.as_slice()));
    let collated_file_hash_verification =
        UInt256::from_slice(&sha256_digest(extracted_data.1.as_slice()));

    // 4. Assert hashes match between signing and verification paths
    assert_eq!(
        file_hash, file_hash_verification,
        "file_hash mismatch: signing={:x} vs verification={:x}",
        file_hash, file_hash_verification
    );

    assert_eq!(
        collated_file_hash_signing, collated_file_hash_verification,
        "collated_file_hash mismatch: signing={:x} vs verification={:x}. \
        This would cause all signatures to be invalid!",
        collated_file_hash_signing, collated_file_hash_verification
    );

    // 5. Verify that using UInt256::default() would fail (the original bug)
    let wrong_collated_file_hash = UInt256::default();
    assert_ne!(
        wrong_collated_file_hash, collated_file_hash_verification,
        "UInt256::default() should NOT match the computed hash. \
        If this fails, collated_data might be empty."
    );
}

/// Verify the start gate: sessions create the overlay immediately but do NOT
/// begin FSM processing until `start(seqno)` is called.
///
/// This tests the overlay-warmup fix for the mixed C++/Rust timing gap where
/// the FSM's `first_block_timeout` would fire before the overlay had established
/// peer connections, permanently stalling finalization.
#[test]
fn test_simplex_start_gate() {
    let _test_lock = SIMPLEX_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    const DB_PATH: &str = "../../target/test";

    if !is_test_logging_enabled() {
        return;
    }

    let _ = env_logger::builder().is_test(true).try_init();

    let node_count = 7usize;
    let shard = ShardIdent::masterchain();
    let initial_block_seqno = 1u32;

    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let key = Ed25519KeyOption::generate().unwrap();
        let adnl_id = key.id().clone();
        nodes.push(SessionNode { public_key: key, adnl_id, weight: 1 });
    }

    let overlay_manager = SessionFactory::create_in_process_overlay_manager(node_count);

    let rand_name: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path_base = format!("{}/simplex_start_gate_{}", DB_PATH, rand_name);
    let mut rng = rand::thread_rng();
    let session_id: UInt256 = UInt256::from(rng.gen::<[u8; 32]>());

    let session_opts = SessionOptions {
        proto_version: 0,
        target_rate: Duration::from_millis(200),
        first_block_timeout: Duration::from_millis(1000),
        slots_per_leader_window: 1,
        wait_for_db_init: true,
        max_leader_window_desync: 10_000,
        ..Default::default()
    };

    let finalized_blocks: FinalizedBlocksMap = Arc::new(Mutex::new(HashSet::new()));
    let finalized_counters: Vec<Arc<AtomicU32>> =
        (0..node_count).map(|_| Arc::new(AtomicU32::new(0))).collect();
    let mut sessions: Vec<SessionPtr> = Vec::new();
    // Keep instances alive so the listener Weak pointers remain valid.
    let mut instances: Vec<Arc<SpinMutex<SessionInstance>>> = Vec::new();

    let config = TestConfig {
        total_rounds: 10,
        min_finalized_percent: 0.5,
        node_count,
        generation_failure_probability: 0.0,
        candidate_rejection_probability: 0.0,
        max_collations: 10000,
        target_rate: Duration::from_millis(200),
        first_block_timeout: Duration::from_millis(1000),
        test_name: "simplex_start_gate".to_string(),
        test_timeout: Duration::from_secs(60),
        expect_timeout: false,
        shard: shard.clone(),
        mc_notification_interval: None,
        overlay_type: OverlayType::InProcess,
        net_gremlin: None,
        restart_gremlin: None,
        lossy_overlay: None,
        lossy_overlay_node_indices: None,
        standstill_timeout: None,
        slots_per_leader_window: None,
    };

    for i in 0..node_count {
        let local_key = nodes[i].public_key.clone();
        let db_path = format!("{}_node{}", db_path_base, i);
        let max_finalized_seqno = Arc::new(AtomicU32::new(initial_block_seqno));
        let finalized_seqnos: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));

        let listener = Arc::new(SessionInstanceListener {
            instance: SpinMutex::new(Weak::new()),
            max_finalized_seqno: max_finalized_seqno.clone(),
            finalized_seqnos: finalized_seqnos.clone(),
        });
        let session_listener: Arc<dyn SessionListener + Send + Sync> = listener.clone();

        let session = SessionFactory::create_session(
            &session_opts,
            &session_id,
            &shard,
            nodes.clone(),
            &local_key,
            db_path,
            overlay_manager.clone(),
            Arc::downgrade(&session_listener),
        )
        .expect("Failed to create session");
        let simplex_session = session.clone() as Arc<dyn SimplexSession + Send + Sync>;

        let session_instance = Arc::new(SpinMutex::new(SessionInstance {
            source_index: i as u32,
            public_key: nodes[i].public_key.clone(),
            batch_processed: Arc::new(AtomicBool::new(false)),
            collation_requested: Arc::new(AtomicBool::new(false)),
            collation_count: Arc::new(AtomicU32::new(0)),
            on_candidate_count: Arc::new(AtomicU32::new(0)),
            on_candidate_observed_count: Arc::new(AtomicU32::new(0)),
            parent_missing_observed_count: Arc::new(AtomicU32::new(0)),
            resolver_probe_requests_count: Arc::new(AtomicU32::new(0)),
            on_block_finalized_count: finalized_counters[i].clone(),
            is_collator: Arc::new(AtomicBool::new(false)),
            config: config.clone(),
            commit_latencies: Arc::new(Mutex::new(Vec::new())),
            max_finalized_seqno,
            finalized_seqnos,
            session_errors_count: Arc::new(AtomicU32::new(0)),
            finalized_blocks: finalized_blocks.clone(),
            simplex_session,
            _session: session.clone(),
            _listener: listener.clone(),
        }));

        *listener.instance.lock() = Arc::downgrade(&session_instance);

        sessions.push(session);
        instances.push(session_instance);
    }

    // Phase 1a: verify no commits while sessions are gated (overlay is warming up)
    log::info!("[start_gate] Phase 1a: verifying no commits for 2s without start()");
    thread::sleep(Duration::from_secs(2));
    for (i, counter) in finalized_counters.iter().enumerate() {
        let commits = counter.load(Ordering::Relaxed);
        assert_eq!(
            commits, 0,
            "Node {} committed {} blocks before start() was called — start gate failed",
            i, commits
        );
    }
    log::info!("[start_gate] Phase 1a passed: zero commits before start()");

    // Phase 1b: prolonged cold-start delay (regression guard).
    // Even after a longer pre-start delay, no session may produce commits before start().
    const EXTRA_COLD_START_DELAY: Duration = Duration::from_secs(5);
    log::info!(
        "[start_gate] Phase 1b: verifying no commits after extra {:?} cold delay",
        EXTRA_COLD_START_DELAY
    );
    thread::sleep(EXTRA_COLD_START_DELAY);
    for (i, counter) in finalized_counters.iter().enumerate() {
        let commits = counter.load(Ordering::Relaxed);
        assert_eq!(
            commits, 0,
            "Node {} committed {} blocks during prolonged cold-start delay before start()",
            i, commits
        );
    }
    log::info!("[start_gate] Phase 1b passed: zero commits during prolonged cold delay");

    // Phase 2: call start(seqno) on all sessions, then wait for commits
    log::info!(
        "[start_gate] Phase 2: calling start(seqno={}) on all sessions",
        initial_block_seqno
    );
    for session in &sessions {
        let (prev_blocks, min_masterchain_block_id) =
            session_start_args(&shard, initial_block_seqno);
        session.start(prev_blocks, min_masterchain_block_id);
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let min_finalized = 3u32;
    loop {
        thread::sleep(Duration::from_millis(200));
        let all_committed =
            finalized_counters.iter().all(|c| c.load(Ordering::Relaxed) >= min_finalized);
        if all_committed {
            break;
        }
        if Instant::now() > deadline {
            for (i, counter) in finalized_counters.iter().enumerate() {
                log::error!("[start_gate] Node {} commits: {}", i, counter.load(Ordering::Relaxed));
            }
            panic!(
                "Timed out waiting for {} commits after start() — \
                 sessions did not begin processing after start gate was released",
                min_finalized
            );
        }
    }

    log::info!(
        "[start_gate] Phase 2 passed: all nodes committed >= {} blocks after start()",
        min_finalized
    );

    for session in &sessions {
        session.stop();
    }
    drop(instances);
    log::info!("[start_gate] Test passed");
}

/// Test candidate chaining within a multi-slot leader window (C++ parity).
///
/// Configures `slots_per_leader_window = 4`. Precollation depth is derived
/// automatically from `slots_per_leader_window`, so the leader can chain
/// candidates across slots within a single window. With
/// chaining, the leader generates slot N+1 with slot N's candidate as parent,
/// which causes seqnos to increment (1, 2, 3, 4) instead of repeating seqno=1.
///
/// Acceptance: the test reaches the commit threshold (at least 30% of rounds
/// committed), proving that chained candidates are notarized and finalized.
#[test]
fn test_simplex_consensus_candidate_chaining() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 20,
            min_finalized_percent: 0.3,
            node_count: 4,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 2000,
            target_rate: Duration::from_millis(300),
            first_block_timeout: Duration::from_millis(3000),
            test_name: "simplex_candidate_chaining".to_string(),
            test_timeout: Duration::from_secs(120),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: None,
            lossy_overlay_node_indices: None,
            standstill_timeout: None,
            slots_per_leader_window: Some(4),
        },
        |instances| {
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "[chaining] Instance {}: {} commits out of {} total_rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds). \
                    Candidate chaining may not be working correctly.",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Candidate chaining in multi-slot windows should remain live under moderate
/// packet loss, validating in-window publication parity behavior.
#[test]
fn test_simplex_consensus_candidate_chaining_with_lossy_overlay() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 20,
            min_finalized_percent: 0.2,
            node_count: 4,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 2500,
            target_rate: Duration::from_millis(300),
            first_block_timeout: Duration::from_millis(3000),
            test_name: "simplex_candidate_chaining_lossy".to_string(),
            test_timeout: Duration::from_secs(150),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: Some(consensus_common::LossyOverlayOpts {
                lost_broadcast_probability: 0.15,
                lost_message_probability: 0.1,
                lost_query_probability: 0.1,
                ..Default::default()
            }),
            lossy_overlay_node_indices: Some(vec![0]),
            standstill_timeout: None,
            slots_per_leader_window: Some(4),
        },
        |instances| {
            let config = &instances[0].lock().config.clone();
            let min_finalized = (config.total_rounds as f64 * config.min_finalized_percent) as u32;

            for (idx, instance) in instances.iter().enumerate() {
                let commits = instance.lock().on_block_finalized_count();
                log::info!(
                    "[chaining-lossy] Instance {}: {} commits out of {} rounds (min required: {})",
                    idx,
                    commits,
                    config.total_rounds,
                    min_finalized
                );
                assert!(
                    commits >= min_finalized,
                    "Instance {} has {} commits but requires at least {} ({}% of {} rounds)",
                    idx,
                    commits,
                    min_finalized,
                    config.min_finalized_percent * 100.0,
                    config.total_rounds
                );
            }
        },
    );
}

/// Integration test: exercise resolver-demand requests during live consensus.
///
/// The harness enables lossy delivery on one node and uses
/// `SessionListener::on_candidate_observed` to emulate validator-side resolver
/// demand (`ensure_candidate_available(include_parent_chain=true)`).
#[test]
fn test_simplex_consensus_ghost_parent_resolver_probe() {
    run_simplex_consensus_test(
        TestConfig {
            total_rounds: 24,
            min_finalized_percent: 0.2,
            node_count: 4,
            generation_failure_probability: 0.0,
            candidate_rejection_probability: 0.0,
            max_collations: 2500,
            target_rate: Duration::from_millis(300),
            first_block_timeout: Duration::from_millis(3000),
            test_name: "simplex_ghost_parent_resolver_probe".to_string(),
            test_timeout: Duration::from_secs(150),
            expect_timeout: false,
            shard: ShardIdent::masterchain(),
            mc_notification_interval: None,
            overlay_type: OverlayType::InProcess,
            net_gremlin: None,
            restart_gremlin: None,
            lossy_overlay: Some(consensus_common::LossyOverlayOpts {
                lost_broadcast_probability: 0.25,
                lost_message_probability: 0.2,
                lost_query_probability: 0.2,
                ..Default::default()
            }),
            lossy_overlay_node_indices: Some(vec![0]),
            standstill_timeout: None,
            slots_per_leader_window: Some(4),
        },
        |instances| {
            let probe = instances[0].lock();
            let observed = probe.on_candidate_observed_count();
            let parent_missing = probe.parent_missing_observed_count();
            let requests = probe.resolver_probe_requests_count();
            let finalized = probe.on_block_finalized_count();
            drop(probe);

            log::info!(
                "[ghost-parent-resolver-probe] node0 observed={}, parent_missing={}, requests={}, finalized={}",
                observed,
                parent_missing,
                requests,
                finalized
            );

            assert!(observed > 0, "resolver probe node should observe at least one candidate");
            assert!(
                requests > 0,
                "resolver probe should issue ensure_candidate_available requests"
            );
            assert!(finalized > 0, "resolver probe node should finalize blocks");
            assert!(
                parent_missing > 0 || requests >= 10,
                "expected unresolved-parent observations or sustained resolver probing under lossy delivery"
            );
        },
    );
}

/// Test that empty collated_data produces a valid (non-default) hash
#[test]
fn test_empty_collated_data_hash() {
    let empty_collated_data: &[u8] = &[];

    // Hash of empty data should be sha256 of empty input, NOT UInt256::default()
    let hash_of_empty = UInt256::from_slice(&sha256_digest(empty_collated_data));
    let default_hash = UInt256::default();

    // SHA256 of empty input is e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    // This is NOT all zeros
    assert_ne!(
        hash_of_empty, default_hash,
        "SHA256 of empty data should NOT be UInt256::default(). \
        Empty collated_data is valid but must still be hashed properly."
    );
}
