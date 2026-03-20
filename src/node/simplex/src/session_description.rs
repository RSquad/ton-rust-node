/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Session description implementation for Simplex consensus
//!
//! Contains session-level constants, validator information, and time management.
//! This module is crate-private.

use crate::{
    block::{SlotIndex, ValidatorIndex, WindowIndex},
    utils::{threshold_33, threshold_66},
    MetricsHandle, PrivateKey, PublicKey, PublicKeyHash, SessionId, SessionNode, SessionOptions,
    ValidatorWeight,
};
use std::{
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_block::{error, fail, Result, ShardIdent};

/*
    Source node description
*/

/// Validator source node description
struct Source {
    /// Public key hash for the node
    id: PublicKeyHash,
    /// Public key of the node
    public_key: PublicKey,
    /// ADNL identifier
    #[allow(dead_code)]
    adnl_id: PublicKeyHash,
    /// Node's weight according to stake
    weight: ValidatorWeight,
}

/*
    SessionDescription implementation
*/

/// Session description for Simplex consensus
///
/// Contains all immutable session-level configuration:
/// - Session identity (session_id, shard, initial_block_seqno)
/// - Validator set (sources, weights, local key)
/// - Options and timing
pub(crate) struct SessionDescription {
    /// Session identifier
    session_id: SessionId,
    /// Session options
    options: SessionOptions,
    /// Initial block seqno (first block produced by this session)
    initial_block_seqno: u32,
    /// List of validator sources
    sources: Vec<Source>,
    /// Mapping between public key hash and source index
    #[allow(dead_code)]
    rev_sources: HashMap<PublicKeyHash, ValidatorIndex>,
    /// Total weight of all validators
    total_weight: ValidatorWeight,
    /// Index of this validator in the list of sources
    self_idx: ValidatorIndex,
    /// Shard identifier
    shard: ShardIdent,
    /// Local validator's private key (for signing votes and candidates)
    local_key: PrivateKey,
    /// Session creation time
    session_creation_time: SystemTime,
    /// Current time for log replaying (0 for real-time).
    ///
    /// Stored as unix micros: `0 => real-time`, otherwise `micros_since_epoch + 1`.
    ///
    /// This is atomic so `SessionDescription` can be shared via `Arc<SessionDescription>`
    /// without a mutex (used by tests / log replay).
    replay_time_us: AtomicU64,
    /// Metrics receiver handle
    metrics_receiver: MetricsHandle,
}

impl SessionDescription {
    /*
        Constructor
    */

    /// Create new session description
    ///
    /// # Parameters
    /// * `options` - Session options (immutable configuration)
    /// * `session_id` - Unique session identifier
    /// * `initial_block_seqno` - Expected seqno for the first block produced
    /// * `nodes` - Validator set for this session
    /// * `local_key` - This validator's private key (for signing)
    /// * `shard` - Shard identifier
    /// * `session_creation_time` - When the session was created
    /// * `metrics_receiver` - Optional metrics handle
    pub fn new(
        options: &SessionOptions,
        session_id: SessionId,
        initial_block_seqno: u32,
        nodes: &[SessionNode],
        local_key: PrivateKey,
        shard: &ShardIdent,
        session_creation_time: SystemTime,
        metrics_receiver: Option<MetricsHandle>,
    ) -> Result<Self> {
        let session_id_str = session_id.to_hex_string();

        log::info!("Session {} options: {:?}", session_id_str, options);

        // Find local index for logging (may be missing if node is not in validator set)
        let local_id = local_key.id();
        let local_idx = nodes.iter().position(|n| n.public_key.id() == local_id);

        // Calculate weights (same as old log_creation_info)
        let total_weight: ValidatorWeight = nodes.iter().map(|n| n.weight).sum();
        let threshold_66_weight = threshold_66(total_weight);
        let threshold_33_weight = threshold_33(total_weight);

        log::info!(
            "Session {} creating SessionDescription: nodes={}, local_idx={}, total_weight={}, threshold_66={}, threshold_33={}",
            session_id_str,
            nodes.len(),
            local_idx.map_or("unknown".to_string(), |i| format!("v{i:03}")),
            total_weight,
            threshold_66_weight,
            threshold_33_weight,
        );

        // Log each node on separate line with session_id prefix for grep
        for (idx, node) in nodes.iter().enumerate() {
            let is_local = if Some(idx) == local_idx { " (local)" } else { "" };
            log::info!(
                "Session {} node v{:03}: weight={}, adnl_id={}, public_key_hash={}{}",
                session_id_str,
                idx,
                node.weight,
                node.adnl_id,
                node.public_key.id(),
                is_local
            );
        }

        let options = *options;
        let local_id = local_id;

        // Initialize sources
        let mut total_weight: ValidatorWeight = 0;
        let mut sources = Vec::with_capacity(nodes.len());
        let mut rev_sources = HashMap::new();

        for (node_index, node) in nodes.iter().enumerate() {
            let source = Source {
                public_key: node.public_key.clone(),
                adnl_id: node.adnl_id.clone(),
                id: node.public_key.id().clone(),
                weight: node.weight,
            };

            rev_sources.insert(source.id.clone(), ValidatorIndex::from(node_index));
            sources.push(source);

            total_weight += node.weight;
        }

        let self_idx = match rev_sources.get(local_id) {
            Some(&idx) => idx,
            None => {
                fail!(
                    "SessionDescription::new: can't find validator with local ID {} in sources",
                    local_id
                )
            }
        };

        let metrics_receiver = metrics_receiver.unwrap_or_else(|| MetricsHandle::new(None));

        Ok(Self {
            session_id,
            options,
            initial_block_seqno,
            sources,
            rev_sources,
            total_weight,
            self_idx,
            shard: shard.clone(),
            local_key,
            session_creation_time,
            replay_time_us: AtomicU64::new(0),
            metrics_receiver,
        })
    }

    /*
        Session identity
    */

    /// Get session identifier
    pub fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Get initial block seqno
    pub fn get_initial_block_seqno(&self) -> u32 {
        self.initial_block_seqno
    }

    /// Get local validator's private key
    pub fn get_local_key(&self) -> &PrivateKey {
        &self.local_key
    }

    /// Get session creation time
    pub fn get_session_creation_time(&self) -> SystemTime {
        self.session_creation_time
    }

    /*
        Options access
    */

    /// Get session options
    pub fn opts(&self) -> &SessionOptions {
        &self.options
    }

    /*
        Validators management
    */

    /// Get source public key hash by index
    pub fn get_source_public_key_hash(&self, src_idx: ValidatorIndex) -> &PublicKeyHash {
        &self.sources[src_idx.value() as usize].id
    }

    /// Get source public key by index
    pub fn get_source_public_key(&self, src_idx: ValidatorIndex) -> &PublicKey {
        &self.sources[src_idx.value() as usize].public_key
    }

    /// Get source ADNL ID by index
    #[allow(dead_code)]
    pub fn get_source_adnl_id(&self, src_idx: ValidatorIndex) -> &PublicKeyHash {
        &self.sources[src_idx.value() as usize].adnl_id
    }

    /// Get source index by public key hash (returns Result instead of panicking)
    #[allow(dead_code)]
    pub fn get_source_index(&self, public_key_hash: &PublicKeyHash) -> Result<ValidatorIndex> {
        self.rev_sources.get(public_key_hash).copied().ok_or_else(|| {
            error!(
                "SessionDescription::get_source_index: unknown public key hash: {}",
                public_key_hash
            )
        })
    }

    /// Get validator weight by index
    pub fn get_node_weight(&self, src_idx: ValidatorIndex) -> ValidatorWeight {
        self.sources[src_idx.value() as usize].weight
    }

    /// Get total number of nodes
    pub fn get_total_nodes(&self) -> usize {
        self.sources.len()
    }

    /// Get this validator's index
    pub fn get_self_idx(&self) -> ValidatorIndex {
        self.self_idx
    }

    /// Check if the given validator index is this validator
    #[inline]
    pub fn is_self(&self, idx: ValidatorIndex) -> bool {
        idx == self.self_idx
    }

    /// Get shard identifier
    pub fn get_shard(&self) -> &ShardIdent {
        &self.shard
    }

    /*
        Weights and thresholds
    */

    /// Get total weight of all validators
    pub fn get_total_weight(&self) -> ValidatorWeight {
        self.total_weight
    }

    /// Get strict 2/3 threshold for certificates (C++: `(total * 2) / 3 + 1`)
    pub fn get_threshold_66(&self) -> ValidatorWeight {
        threshold_66(self.total_weight)
    }

    /// Get strict 1/3 threshold for safety conditions (C++: `total / 3 + 1`)
    pub fn get_threshold_33(&self) -> ValidatorWeight {
        threshold_33(self.total_weight)
    }

    /*
        Slot and leader window helpers
    */

    /// Get window index for a given slot
    pub fn get_window_idx(&self, slot: SlotIndex) -> WindowIndex {
        slot.window_index(self.options.slots_per_leader_window)
    }

    /// Get first slot of the window containing the given slot
    #[allow(dead_code)]
    pub fn get_window_start_slot(&self, slot: SlotIndex) -> SlotIndex {
        slot.window_start(self.options.slots_per_leader_window)
    }

    /// Get slot offset within the window (0-based)
    pub fn get_slot_offset_in_window(&self, slot: SlotIndex) -> u32 {
        slot.offset_in_window(self.options.slots_per_leader_window)
    }

    /// Is this the first slot in its leader window?
    pub fn is_first_in_window(&self, slot: SlotIndex) -> bool {
        slot.is_first_in_window(self.options.slots_per_leader_window)
    }

    /// Is this the last slot in its leader window?
    #[allow(dead_code)]
    pub fn is_last_in_window(&self, slot: SlotIndex) -> bool {
        slot.is_last_in_window(self.options.slots_per_leader_window)
    }

    /// Get leader for the given slot's window (round-robin by window index)
    pub fn get_leader(&self, slot: SlotIndex) -> ValidatorIndex {
        ValidatorIndex::new(self.get_window_idx(slot).value() % (self.sources.len() as u32))
    }

    /// Is this node the leader for the given slot's window?
    pub fn is_self_leader(&self, slot: SlotIndex) -> bool {
        self.get_leader(slot) == self.self_idx
    }

    /*
        Time management
    */

    /// Set time for log replaying
    #[allow(dead_code)]
    pub fn set_time(&self, time: SystemTime) {
        let micros = match time.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_micros(),
            Err(_) => 0,
        };

        let micros_u64 = u64::try_from(micros).unwrap_or(u64::MAX - 1);
        self.replay_time_us.store(micros_u64.saturating_add(1), Ordering::Relaxed);
    }

    /// Clear replay time and return to real-time mode.
    #[allow(dead_code)]
    pub fn clear_time(&self) {
        self.replay_time_us.store(0, Ordering::Relaxed);
    }

    /// Get current time (SystemTime::now() for real-time, or replayed time)
    pub fn get_time(&self) -> SystemTime {
        let stored = self.replay_time_us.load(Ordering::Relaxed);
        if stored == 0 {
            SystemTime::now()
        } else {
            UNIX_EPOCH + Duration::from_micros(stored - 1)
        }
    }

    /// Check if time is in the future
    pub fn is_in_future(&self, time: SystemTime) -> bool {
        time > self.get_time()
    }

    /// Check if time is in the past
    #[allow(dead_code)]
    pub fn is_in_past(&self, time: SystemTime) -> bool {
        time < self.get_time()
    }

    /*
        Metrics
    */

    /// Get metrics receiver handle
    pub fn get_metrics_receiver(&self) -> &MetricsHandle {
        &self.metrics_receiver
    }
}

/*
    Display implementation
*/

impl fmt::Display for SessionDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SessionDescription(nodes={}, total_weight={}, self_idx={})",
            self.sources.len(),
            self.total_weight,
            self.self_idx
        )
    }
}
