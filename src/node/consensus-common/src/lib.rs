/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Common types and utilities shared between consensus implementations.
//!
//! This crate contains shared functionality used by both catchain-based
//! validator-session and simplex consensus implementations.

// Internal modules
mod activity_node;
mod adnl_overlay;
mod async_key_value_storage;
mod block_payload;
mod dummy_catchain_overlay;
mod in_process_overlay;
mod log_player;
mod lossy_overlay;

// Public modules
pub mod compression;
pub mod profiling;
pub mod utils;

use adnl::{NetworkStack, PrivateOverlayShortId};
use std::{
    any::Any,
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};
use ton_block::{
    BlockIdExt, BlockSignaturesVariant, ConfigParams, KeyId, KeyOption, ShardIdent, UInt256,
    ValidatorSet,
};

// Test utilities module - available for tests and when test-utils feature is enabled
#[cfg(any(test, feature = "test-utils"))]
pub mod node_test_network;

// ============================================================================
// Common Type Aliases
// ============================================================================

/// Public key
pub type PublicKey = Arc<dyn KeyOption>;

/// Public key hash
pub type PublicKeyHash = Arc<KeyId>;

/// Private key
pub type PrivateKey = Arc<dyn KeyOption>;

/// Consensus session ID
pub type SessionId = UInt256;

/// Hash of the block
pub type BlockHash = UInt256;

/// Signature
pub type BlockSignature = ::ton_api::ton::bytes;

/// Raw data buffer
pub type RawBuffer = ::ton_api::ton::bytes;

/// Validator's weight
pub type ValidatorWeight = u64;

/// Validator's block ID - alias for BlockIdExt
pub type ValidatorBlockId = BlockIdExt;

/// Result for operations
pub type Result<T> = ton_block::Result<T>;

/// Pointer to a BlockPayload
pub type BlockPayloadPtr = Arc<dyn BlockPayload>;

/// Response for queries
pub type QueryResponseCallback = Box<dyn FnOnce(Result<BlockPayloadPtr>) + Send>;

/// Pointer to overlay listener
pub type ConsensusOverlayListenerPtr = Weak<dyn ConsensusOverlayListener + Send + Sync>;

/// Pointer to overlay log replay listener
pub type ConsensusOverlayLogReplayListenerPtr =
    Weak<dyn ConsensusOverlayLogReplayListener + Send + Sync>;

/// Pointer to overlay API
pub type ConsensusOverlayPtr = Arc<dyn ConsensusOverlay + Send>;

/// Pointer to overlay manager
pub type ConsensusOverlayManagerPtr = Arc<dyn ConsensusOverlayManager + Send + Sync>;

/// Activity node pointer
pub type ActivityNodePtr = Arc<dyn ActivityNode>;

/// Pointer to replay listener
pub type ConsensusReplayListenerPtr = Weak<dyn ConsensusReplayListener + Send + Sync>;

/// Pointer to LogPlayer
pub type LogPlayerPtr = Rc<dyn LogPlayer>;

/// Pointer to a validator's block candidate
pub type ValidatorBlockCandidatePtr = Arc<ValidatorBlockCandidate>;

/// Response callback for SessionListener.on_candidate
pub type ValidatorBlockCandidateDecisionCallback =
    Box<dyn FnOnce(Result<std::time::SystemTime>) + Send>;

/// Response callback for SessionListener.on_generate_slot
pub type ValidatorBlockCandidateCallback =
    Box<dyn FnOnce(Result<ValidatorBlockCandidatePtr>) + Send>;

/// Pointer to async request
pub type AsyncRequestPtr = Arc<dyn AsyncRequest + Send + Sync>;

/// Pointer to SessionListener
pub type SessionListenerPtr = Weak<dyn SessionListener + Send + Sync>;

/// Pointer to a Session
pub type SessionPtr = Arc<dyn Session + Send + Sync>;

pub use adnl_overlay::BlockSyncCheck;
pub use lossy_overlay::LossyOverlayOpts;

// ============================================================================
// Async Key-Value Storage Types
// ============================================================================

/// Pointer to async key-value storage
pub type AsyncKeyValueStoragePtr = Arc<dyn AsyncKeyValueStorage>;

/// Key type for storage operations.
///
/// Keys are byte vectors, typically serialized TL objects where:
/// - First 4 bytes = TL constructor tag (for prefix scanning)
/// - Remaining bytes = TL-serialized key fields
pub type StorageKey = Vec<u8>;

/// Value type for storage operations.
pub type StorageValue = Vec<u8>;

/// Callback type for get operation completion.
pub type StorageGetCallback = Box<dyn FnOnce(Result<Option<StorageValue>>) + Send>;

/// Callback type for write operation completion.
pub type StorageWriteCallback = Box<dyn FnOnce(Result<()>) + Send>;

/// Callback type for prefix scan completion.
pub type StoragePrefixScanCallback =
    Box<dyn FnOnce(Result<Vec<(StorageKey, StorageValue)>>) + Send>;

// ============================================================================
// Cancellable - Trait for cancellation support in async operations
// ============================================================================

/// Trait for cancellation support in long-running operations.
///
/// Used with `StorageAsyncResult::wait_cancellable()` to allow graceful
/// interruption of blocking waits during shutdown or cancellation.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use std::sync::atomic::AtomicBool;
///
/// let stop_flag = Arc::new(AtomicBool::new(false));
///
/// // In another thread: stop_flag.store(true, Ordering::Relaxed);
///
/// let result = storage.get(key, None);
/// match result.wait_cancellable(&stop_flag, Duration::from_millis(100)) {
///     Ok(value) => println!("Got value"),
///     Err(e) => println!("Cancelled or error: {}", e),
/// }
/// ```
pub trait Cancellable: Send + Sync {
    /// Check if cancellation has been requested.
    fn is_cancelled(&self) -> bool;
}

/// Blanket implementation of `Cancellable` for `Arc<AtomicBool>`.
///
/// This allows using `Arc<AtomicBool>` (like `stop_requested` in session)
/// directly as a cancellation flag.
impl Cancellable for Arc<AtomicBool> {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

// ============================================================================
// StorageAsyncResult - Async result trait for non-tokio environments
// ============================================================================

/// Typed sentinel error returned by [`StorageAsyncResult::try_get`] and
/// [`StorageAsyncResult::wait_timeout`] once the inner [`Result`] has already
/// been consumed by a prior caller (the implementation has transitioned to
/// `AsyncResultState::Taken`).
///
/// This is a *benign* signal — it means a previous caller successfully
/// observed (and acted on) the real storage outcome. Consumers that hold a
/// dedup map keyed by slot / candidate-id and may invoke the same
/// `StorageAsyncResultPtr` more than once should treat this as `Ok(())` and
/// skip side-effects such as cache updates / re-broadcasts; bumping an
/// error counter would double-count the dedup hit as a real failure.
///
/// Detection pattern (preferred over string matching):
///
/// ```ignore
/// use consensus_common::StorageResultAlreadyTaken;
/// if err.downcast_ref::<StorageResultAlreadyTaken>().is_some() {
///     // benign redundant wake — skip side-effects, no error increment
/// }
/// ```
///
/// The struct is unit-like and intentionally cheap to construct:
/// `StorageResultAlreadyTaken.into()` produces an `anyhow::Error` (= our
/// `Result`'s error type via `ton_block::Error`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default)]
pub struct StorageResultAlreadyTaken;

impl std::fmt::Display for StorageResultAlreadyTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Identical text to the legacy stringly-typed sentinel so log scrapers
        // and human readers see the same message; downcast is the structured
        // detection path.
        f.write_str("StorageAsyncResult: result already taken")
    }
}

impl std::error::Error for StorageResultAlreadyTaken {}

/// Async result for storage operations.
///
/// Similar to a Future but without requiring tokio.
/// Provides blocking wait with timeout and non-blocking poll.
///
/// # Example
///
/// ```ignore
/// let result = storage.get(key.clone(), None);
///
/// // Non-blocking check
/// if result.is_ready() {
///     if let Some(value) = result.try_get() {
///         // use value
///     }
/// }
///
/// // Blocking wait with timeout
/// match result.wait_timeout(Duration::from_secs(1)) {
///     Some(Ok(value)) => println!("Got: {:?}", value),
///     Some(Err(e)) => println!("Error: {}", e),
///     None => println!("Timeout"),
/// }
///
/// // Blocking wait indefinitely
/// match result.wait() {
///     Ok(value) => println!("Got: {:?}", value),
///     Err(e) => println!("Error: {}", e),
/// }
/// ```
pub trait StorageAsyncResult<T>: Send + Sync {
    /// Checks if result is ready (non-blocking).
    fn is_ready(&self) -> bool;

    /// Gets result if ready (non-blocking).
    ///
    /// Returns `None` if still pending.
    /// Returns `Some(Err)` if an operation error occurred or the result has
    /// already been consumed. In the "already consumed" case the inner
    /// `anyhow::Error` downcasts to [`StorageResultAlreadyTaken`] — see
    /// that type for the recommended detection pattern.
    fn try_get(&self) -> Option<Result<T>>;

    /// Waits for result with timeout (**BLOCKING**).
    ///
    /// This is the core wait method that implementations must provide.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    ///
    /// * `Some(Ok(value))` if operation completed successfully
    /// * `Some(Err(e))` if operation failed or result already taken (see
    ///   [`StorageResultAlreadyTaken`] for the typed sentinel emitted in the
    ///   already-taken case)
    /// * `None` if timeout expired (result still pending)
    fn wait_timeout(&self, timeout: Duration) -> Option<Result<T>>;

    /// Waits for result indefinitely (**BLOCKING**).
    ///
    /// # Returns
    ///
    /// * `Ok(value)` if operation completed successfully
    /// * `Err(e)` if operation failed or result already taken (see
    ///   [`StorageResultAlreadyTaken`])
    fn wait(&self) -> Result<T> {
        // Wait in 1-second chunks to allow for spurious wakeups
        // This matches typical condvar usage patterns
        loop {
            if let Some(result) = self.wait_timeout(Duration::from_secs(1)) {
                return result;
            }
        }
    }

    /// Waits for result with cancellation support (**BLOCKING**).
    ///
    /// Polls for completion using periodic `wait_timeout(step)` calls,
    /// checking the cancellation flag between each poll.
    ///
    /// This is the preferred method for startup/bootstrap operations where
    /// graceful shutdown must be possible.
    ///
    /// # Arguments
    ///
    /// * `cancel` - Cancellation flag (e.g., `Arc<AtomicBool>` from session's `stop_requested`)
    /// * `step` - Polling interval between cancellation checks (e.g., 100ms)
    ///
    /// # Returns
    ///
    /// * `Ok(value)` if operation completed successfully
    /// * `Err("Cancelled")` if cancellation was requested
    /// * `Err(e)` if operation failed
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stop_flag = Arc::new(AtomicBool::new(false));
    /// let result = db.load_finalized_blocks_async();
    ///
    /// // Wait with cancellation support (checks every 100ms)
    /// match result.wait_cancellable(&stop_flag, Duration::from_millis(100)) {
    ///     Ok(blocks) => process(blocks),
    ///     Err(e) => log::warn!("Bootstrap cancelled or failed: {}", e),
    /// }
    /// ```
    fn wait_cancellable(&self, cancel: &dyn Cancellable, step: Duration) -> Result<T> {
        loop {
            // Check cancellation first
            if cancel.is_cancelled() {
                return Err(ton_block::error!("Cancelled"));
            }

            // Wait for step duration
            match self.wait_timeout(step) {
                Some(result) => return result, // Got result (success or error)
                None => continue,              // Timeout - check cancellation and retry
            }
        }
    }
}

/// Pointer to async result.
pub type StorageAsyncResultPtr<T> = Arc<dyn StorageAsyncResult<T>>;

/// Configuration options for async key-value storage.
#[derive(Clone, Debug)]
pub struct AsyncKeyValueStorageOptions {
    /// Use separate thread for callback execution.
    ///
    /// If `true` (default):
    /// - Main thread: processes DB operations
    /// - Callback thread: executes completion callbacks
    /// - Prevents DB stalls from slow callbacks
    ///
    /// If `false`:
    /// - Only main thread
    /// - Callbacks executed in main loop (may stall DB)
    pub use_callback_thread: bool,
}

impl Default for AsyncKeyValueStorageOptions {
    fn default() -> Self {
        Self { use_callback_thread: true }
    }
}

// ============================================================================
// Async Key-Value Storage Trait
// ============================================================================

/// Async key-value storage trait.
///
/// **ALL operations are async** - they return immediately with a
/// `StorageAsyncResultPtr<T>` that can be polled or waited on.
///
/// # Reference
///
/// C++ `td/db/KeyValueAsync.h`
///
/// # Threading Model
///
/// - **Caller thread**: Posts operations, receives async result pointers
/// - **DB processing thread** (`kv-db:{storage_id}`): Accesses RocksDB
/// - **Callback thread** (`kv-cb:{storage_id}`, optional): Executes callbacks
///
/// All DB access happens in the DB processing thread. The caller thread
/// NEVER touches DB directly.
///
/// # Lifecycle
///
/// - **Creation**: Blocks until DB is opened, propagates open errors
/// - **Drop**: Syncs pending ops, waits for threads to stop
/// - **Destroy**: Call `mark_for_destroy()`, erases DB files on drop
pub trait AsyncKeyValueStorage: Send + Sync {
    // =========================================================================
    // Read Operations (Async)
    // =========================================================================

    /// Gets value by key (async).
    ///
    /// Returns async result containing:
    /// - `Ok(Some(value))` if found
    /// - `Ok(None)` if not found
    /// - `Err(e)` on error
    ///
    /// # Arguments
    ///
    /// * `key` - Key to look up
    /// * `on_complete` - Optional callback invoked when operation completes
    fn get(
        &self,
        key: StorageKey,
        on_complete: Option<StorageGetCallback>,
    ) -> StorageAsyncResultPtr<Option<StorageValue>>;

    /// Gets all key-value pairs matching prefix (async).
    ///
    /// Prefix is typically a TL tag (u32 little-endian, 4 bytes).
    fn get_by_prefix(
        &self,
        prefix: StorageKey,
        on_complete: Option<StoragePrefixScanCallback>,
    ) -> StorageAsyncResultPtr<Vec<(StorageKey, StorageValue)>>;

    /// Gets all entries where key starts with u32 prefix (TL tag).
    fn get_by_prefix_u32(
        &self,
        prefix: u32,
        on_complete: Option<StoragePrefixScanCallback>,
    ) -> StorageAsyncResultPtr<Vec<(StorageKey, StorageValue)>> {
        self.get_by_prefix(prefix.to_le_bytes().to_vec(), on_complete)
    }

    /// Checks if key exists (async).
    ///
    /// Implemented via `get()` - returns `true` if key found, `false` otherwise.
    fn contains(
        &self,
        key: StorageKey,
        on_complete: Option<Box<dyn FnOnce(Result<bool>) + Send>>,
    ) -> StorageAsyncResultPtr<bool> {
        // Wrap the callback to transform Option<Value> -> bool
        let wrapped_callback: Option<StorageGetCallback> = on_complete.map(|cb| {
            let boxed: StorageGetCallback =
                Box::new(move |result: Result<Option<StorageValue>>| {
                    cb(result.map(|opt| opt.is_some()))
                });
            boxed
        });

        // Get the result and wrap it
        let get_result = self.get(key, wrapped_callback);

        // Create a wrapper that transforms the result
        async_key_value_storage::wrap_contains_result(get_result)
    }

    // =========================================================================
    // Write Operations (Async)
    // =========================================================================

    /// Sets key-value pair (async).
    ///
    /// # Arguments
    ///
    /// * `key` - Key to set
    /// * `value` - Value to store
    /// * `on_complete` - Optional callback invoked when write completes
    fn set(
        &self,
        key: StorageKey,
        value: StorageValue,
        on_complete: Option<StorageWriteCallback>,
    ) -> StorageAsyncResultPtr<()>;

    /// Deletes key from storage (async).
    fn erase(
        &self,
        key: StorageKey,
        on_complete: Option<StorageWriteCallback>,
    ) -> StorageAsyncResultPtr<()>;

    // =========================================================================
    // Sync Operations
    // =========================================================================

    /// Flushes all pending operations (**BLOCKING** with optional timeout).
    ///
    /// Waits until all queued tasks AND callbacks complete.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Optional timeout; None = wait indefinitely
    ///
    /// # Returns
    ///
    /// * `Ok(())` if sync completed
    /// * `Err(e)` if timeout expired
    fn sync(&self, timeout: Option<Duration>) -> Result<()>;

    /// Returns count of pending operations.
    fn pending_count(&self) -> usize;

    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Marks database for destruction on drop.
    ///
    /// When the storage is dropped, the database files will be **deleted**.
    fn mark_for_destroy(&self);

    /// Returns the database path.
    fn get_path(&self) -> &std::path::Path;

    /// Returns the storage identifier.
    fn get_storage_id(&self) -> &str;
}

// ============================================================================
// Session Statistics
// ============================================================================

/// Session statistics reported alongside validator-session block-acceptance callbacks.
///
/// For Simplex these stats are currently also reused for finalized delivery.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// Total number of errors during this session
    pub errors_count: u32,
}

// ============================================================================
// Consensus Node
// ============================================================================

/// Consensus node description
#[derive(Clone)]
pub struct ConsensusNode {
    /// ADNL node short ID
    pub adnl_id: PublicKeyHash,

    /// Node public key
    pub public_key: PublicKey,
}

// ============================================================================
// Block-Sync Overlay (BlockSync)
// ============================================================================

/// 1 MiB safety margin added to the per-key broadcast cap
/// (matches C++ `block-sync-overlay.cpp`: `max_block_size + max_collated_data_size + 1 MiB`)
pub const BLOCK_SYNC_BROADCAST_SIZE_MARGIN: u32 = 1 << 20;

/// Membership and authorization for a per-session block-sync overlay
///
/// C++ ref: `validator/consensus/block-sync-overlay.cpp` (authorized_keys from current set,
/// FixedMemberList from prev|curr|next union built in `manager.cpp`)
#[derive(Clone, Debug)]
pub struct BlockSyncOverlayParams {
    /// Sorted-unique ADNL ids of the prev|curr|next mc validator sets union (FixedMemberList)
    pub members: Vec<PublicKeyHash>,

    /// Senders allowed to originate broadcasts, keyed by ADNL pubkey hash with per-key max size
    pub authorized_keys: HashMap<PublicKeyHash, u32>,

    /// Ordered ADNL pubkey hashes of the current set; precheck computes
    /// `current_set[(slot / slots_per_leader_window) % current_set.len()]`
    pub current_set: Vec<PublicKeyHash>,

    /// Leader window size from `simplex_config_v2`
    pub slots_per_leader_window: u32,

    /// Max broadcast/candidate size accepted on this overlay, derived from
    /// consensus config: `max_block_bytes + max_collated_bytes + 1 MiB` margin
    /// (matches C++ `block-sync-overlay.cpp` `max_broadcast_size`)
    pub max_broadcast_size: u32,

    /// Short id of the block-sync overlay; attached via `with_overlay_id`.
    pub overlay_id: Option<Arc<PrivateOverlayShortId>>,

    /// Identifying fields used by the telemetry log dump; not consumed by overlay logic
    pub shard: Option<ShardIdent>,
    pub session_id: Option<UInt256>,
}

impl BlockSyncOverlayParams {
    /// Build the params from masterchain `ConfigParams` and the shuffled session subset
    ///
    /// `cur_subset` is the per-(shard, cc_seqno) shuffled subset used by simplex for
    /// leader rotation; empty prev/next sets are skipped (C++ `is_null()` behavior)
    pub fn from_config(
        config: &ConfigParams,
        slots_per_leader_window: u32,
        cur_subset: &ValidatorSet,
    ) -> Result<Self> {
        let consensus = config.consensus_config()?;
        let max_size: u32 = consensus
            .max_block_bytes
            .saturating_add(consensus.max_collated_bytes)
            .saturating_add(BLOCK_SYNC_BROADCAST_SIZE_MARGIN);

        let prev = config.prev_validator_set()?;
        let curr = config.validator_set()?;
        let next = config.next_validator_set()?;

        let mut members_set: std::collections::BTreeSet<PublicKeyHash> =
            std::collections::BTreeSet::new();
        let mut authorized_keys: HashMap<PublicKeyHash, u32> = HashMap::new();
        let mut current_set: Vec<PublicKeyHash> = Vec::with_capacity(cur_subset.list().len());

        for vset in [&prev, &curr, &next] {
            for val in vset.list() {
                members_set.insert(val.adnl_addr());
            }
        }

        // current_set / authorized_keys MUST follow the SHUFFLED session order
        // (cur_subset), not the raw masterchain config order. Simplex computes
        // `expected_leader_for(slot) = current_set[(slot/window) % N]` over this
        // shuffled order; using raw mc order makes the precheck reject every
        // valid leader broadcast except by coincidence
        for val in cur_subset.list() {
            let id = val.adnl_addr();
            authorized_keys.insert(id.clone(), max_size);
            current_set.push(id);
        }

        Ok(Self {
            members: members_set.into_iter().collect(),
            authorized_keys,
            current_set,
            slots_per_leader_window,
            max_broadcast_size: max_size,
            overlay_id: None,
            shard: None,
            session_id: None,
        })
    }

    /// Attach the overlay short id (computed by simplex from session_id).
    pub fn with_overlay_id(mut self, overlay_id: Arc<PrivateOverlayShortId>) -> Self {
        self.overlay_id = Some(overlay_id);
        self
    }

    /// Attach identifying fields used by the telemetry log dump
    pub fn with_identity(mut self, shard: ShardIdent, session_id: UInt256) -> Self {
        self.shard = Some(shard);
        self.session_id = Some(session_id);
        self
    }
}

/// Role discriminator used both in Prometheus metric names and the log dump header
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockSyncRole {
    Validator,
    Observer,
}

impl BlockSyncRole {
    pub const fn as_str(&self) -> &'static str {
        match self {
            BlockSyncRole::Validator => "validator",
            BlockSyncRole::Observer => "observer",
        }
    }
}

/// Per-overlay-instance counters for the block-sync overlay
///
/// Counters are cumulative since `started_at`. Bumps update both an in-memory
/// `AtomicU64` (used by the periodic log dump) and a global Prometheus counter
/// named `block_sync_overlay_<role>_<counter>`
pub struct BlockSyncStats {
    pub overlay_short_id: Arc<PrivateOverlayShortId>,
    pub role: BlockSyncRole,
    pub shard: Option<ShardIdent>,
    pub session_id: Option<UInt256>,
    pub started_at: Instant,
    pub sent: AtomicU64,
    pub recv: AtomicU64,
    pub auth_drop: AtomicU64,
    pub precheck_drop: AtomicU64,
    pub send_err: AtomicU64,
    pub leader_mismatch: AtomicU64,
}

/// Period between log dumps; Prometheus scrape cadence is independent
pub const BLOCK_SYNC_STATS_LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Log target for the multi-line periodic dump
pub const BLOCK_SYNC_STATS_LOG_TARGET: &str = "block_sync_telemetry";

impl BlockSyncStats {
    pub fn new(
        overlay_short_id: Arc<PrivateOverlayShortId>,
        role: BlockSyncRole,
        shard: Option<ShardIdent>,
        session_id: Option<UInt256>,
    ) -> Arc<Self> {
        Arc::new(Self {
            overlay_short_id,
            role,
            shard,
            session_id,
            started_at: Instant::now(),
            sent: AtomicU64::new(0),
            recv: AtomicU64::new(0),
            auth_drop: AtomicU64::new(0),
            precheck_drop: AtomicU64::new(0),
            send_err: AtomicU64::new(0),
            leader_mismatch: AtomicU64::new(0),
        })
    }

    pub fn bump_sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_sent",
            BlockSyncRole::Observer => "block_sync_overlay_observer_sent",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }
    pub fn bump_recv(&self) {
        self.recv.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_recv",
            BlockSyncRole::Observer => "block_sync_overlay_observer_recv",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }
    pub fn bump_auth_drop(&self) {
        self.auth_drop.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_auth_drop",
            BlockSyncRole::Observer => "block_sync_overlay_observer_auth_drop",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }
    pub fn bump_precheck_drop(&self) {
        self.precheck_drop.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_precheck_drop",
            BlockSyncRole::Observer => "block_sync_overlay_observer_precheck_drop",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }
    pub fn bump_send_err(&self) {
        self.send_err.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_send_err",
            BlockSyncRole::Observer => "block_sync_overlay_observer_send_err",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }
    pub fn bump_leader_mismatch(&self) {
        self.leader_mismatch.fetch_add(1, Ordering::Relaxed);
        let name = match self.role {
            BlockSyncRole::Validator => "block_sync_overlay_validator_leader_mismatch",
            BlockSyncRole::Observer => "block_sync_overlay_observer_leader_mismatch",
        };
        metrics::counter!(name, "shard" => self.shard_label()).increment(1);
    }

    fn shard_label(&self) -> String {
        self.shard.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "none".into())
    }

    /// Emit a single multi-line block at INFO into `BLOCK_SYNC_STATS_LOG_TARGET`
    pub fn dump(&self) {
        let uptime = self.started_at.elapsed().as_secs();
        let shard_str = self.shard.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "-".into());
        let session_str =
            self.session_id.as_ref().map(|s| s.to_hex_string()).unwrap_or_else(|| "-".into());
        log::info!(
            target: BLOCK_SYNC_STATS_LOG_TARGET,
            "BSYNC STAT\n  \
             overlay:        {}\n  \
             shard:          {shard_str}\n  \
             session:        {session_str}\n  \
             role:           {}\n  \
             uptime:         {uptime}s\n  \
             sent:           {}\n  \
             recv:           {}\n  \
             precheck_drop:  {}\n  \
             auth_drop:      {}\n  \
             send_err:       {}\n  \
             leader_mismatch:{}\n",
            self.overlay_short_id,
            self.role.as_str(),
            self.sent.load(Ordering::Relaxed),
            self.recv.load(Ordering::Relaxed),
            self.precheck_drop.load(Ordering::Relaxed),
            self.auth_drop.load(Ordering::Relaxed),
            self.send_err.load(Ordering::Relaxed),
            self.leader_mismatch.load(Ordering::Relaxed),
        );
    }

    /// Spawn a tokio task that calls `dump()` every `BLOCK_SYNC_STATS_LOG_INTERVAL`
    /// until `stop` is observed
    pub fn spawn_ticker(self: Arc<Self>, runtime: tokio::runtime::Handle, stop: Arc<AtomicBool>) {
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(BLOCK_SYNC_STATS_LOG_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // skip immediate first tick
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                ticker.tick().await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                self.dump();
            }
        });
    }
}

// ============================================================================
// Block Payload
// ============================================================================

/// Trait for block payload data
pub trait BlockPayload: fmt::Debug + Send + Sync {
    /// Get raw data buffer
    fn data(&self) -> &RawBuffer;

    /// Block creation time
    fn get_creation_time(&self) -> std::time::SystemTime;
}

// ============================================================================
// Overlay Interfaces
// ============================================================================

/// Which overlay an inbound broadcast arrived on
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastSource {
    /// Inbound on the consensus private overlay (votes/certs)
    ConsensusOverlay,
    /// Inbound on the per-session block-sync overlay (candidates only)
    BlockSyncOverlay,
}

pub trait ConsensusOverlayListener: Send + Sync {
    /// Incoming message processing
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr);

    /// Incoming broadcast processing
    fn on_broadcast(
        &self,
        source_key_hash: PublicKeyHash,
        data: &BlockPayloadPtr,
        source: BroadcastSource,
    );

    /// Incoming query processing
    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    );
}

/// Overlay listener interface to control time during the log replay
pub trait ConsensusOverlayLogReplayListener: Send + Sync {
    /// Set timestamp for all further events
    fn on_time_changed(&self, timestamp: std::time::SystemTime);
}

/// Overlay outgoing interface (Consensus -> Overlay)
pub trait ConsensusOverlay: Send + Sync {
    /// Send message
    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        is_retransmission: bool,
    );

    /// Send message to multiple sources
    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        is_retransmission: bool,
    );

    /// Send query
    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        name: &str,
        timeout: std::time::Duration,
        message: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    );

    /// Send query via RLDP (ADNL ID of the current node should be registered for the query)
    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    );

    /// Send broadcast with optional extra metadata (e.g. consensus.broadcastExtra for slot info)
    fn send_broadcast_fec_ex(
        &self,
        sender_id: &PublicKeyHash,
        send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
        extra: Option<Vec<u8>>,
    );

    /// Implementation specific
    fn get_impl(&self) -> &dyn Any;
}

/// Transport type for consensus overlay communication
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayTransportType {
    /// Catchain consensus over ADNL UDP only
    Catchain,
    /// Catchain consensus over ADNL UDP + TCP
    CatchainTcp,
    /// Simplex consensus over ADNL UDP + TCP
    Simplex,
    /// Simplex consensus over QUIC streams
    SimplexQuic,
}

impl OverlayTransportType {
    pub fn allow_tcp(&self) -> bool {
        matches!(self, Self::CatchainTcp)
    }

    pub fn use_quic(&self) -> bool {
        matches!(self, Self::SimplexQuic)
    }
}

/// Overlay manager
pub trait ConsensusOverlayManager {
    /// Create new overlay; when `enable_observers=true`, simplex attaches the
    /// block-sync overlay id to `block_sync_params` via `with_overlay_id`.
    fn start_overlay(
        &self,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
        overlay_listener: ConsensusOverlayListenerPtr,
        log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        transport_type: OverlayTransportType,
        block_sync_params: Option<BlockSyncOverlayParams>,
    ) -> Result<ConsensusOverlayPtr>;

    /// Stop existing overlay
    fn stop_overlay(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        overlay: &ConsensusOverlayPtr,
    );
}

// ============================================================================
// Activity Node
// ============================================================================

/// Activity node for liveness tracking
pub trait ActivityNode: Send + Sync {
    /// Name of the object
    fn get_name(&self) -> String;

    /// Get creation time
    fn get_creation_time(&self) -> std::time::SystemTime;

    /// Get last activity notification time
    fn get_access_time(&self) -> std::time::SystemTime;

    /// Notify about activity
    fn tick(&self);
}

// ============================================================================
// Replay Listener
// ============================================================================

/// Listener for consensus replaying
pub trait ConsensusReplayListener {
    /// Start of replaying
    fn replay_started(&self);

    /// Finish of replaying
    fn replay_finished(&self);
}

// ============================================================================
// Log Replay
// ============================================================================

/// Consensus log replay options
#[derive(Clone)]
pub struct LogReplayOptions {
    /// Path to the log file with data to be replayed
    pub log_file_name: String,

    /// Optional: preferred session ID (if None, the last session ID in log will be used)
    pub session_id: Option<String>,

    /// Optional: replay without delays
    pub replay_without_delays: bool,

    /// Consensus DB path
    pub db_path: String,

    /// Consensus DB suffix
    pub db_suffix: String,

    /// Flag which indicates that unsafe self node blocks resync mode is enabled
    pub allow_unsafe_self_blocks_resync: bool,
}

/// Consensus log player
pub trait LogPlayer {
    /// Get session ID
    fn get_session_id(&self) -> &SessionId;

    /// Get validator local ID
    fn get_local_id(&self) -> &PublicKeyHash;

    /// Get validator private key
    fn get_local_key(&self) -> &PrivateKey;

    /// Get list of nodes
    fn get_nodes(&self) -> &[ConsensusNode];

    /// Get weights
    fn get_weights(&self) -> &Vec<ValidatorWeight>;

    /// Get overlay manager
    fn get_overlay_manager(
        &self,
        replay_listener: ConsensusReplayListenerPtr,
    ) -> ConsensusOverlayManagerPtr;
}

// ============================================================================
// Factory
// ============================================================================

/// Consensus common factory for creating shared objects
pub struct ConsensusCommonFactory;

impl ConsensusCommonFactory {
    /// Create block payload
    pub fn create_block_payload(data: RawBuffer) -> BlockPayloadPtr {
        block_payload::BlockPayloadImpl::create(data)
    }

    /// Create empty payload
    pub fn create_empty_block_payload() -> BlockPayloadPtr {
        Self::create_block_payload(RawBuffer::default())
    }

    /// Create activity node
    pub fn create_activity_node(name: String) -> ActivityNodePtr {
        activity_node::ActivityNodeManager::create_node(name)
    }

    /// Create dummy overlay manager
    pub fn create_dummy_overlay_manager() -> ConsensusOverlayManagerPtr {
        dummy_catchain_overlay::DummyConsensusOverlayManager::create()
    }

    /// Create in-process overlay manager
    pub fn create_in_process_overlay_manager(num_threads: usize) -> ConsensusOverlayManagerPtr {
        in_process_overlay::OverlayManagerImpl::create(num_threads)
    }

    /// Create a lossy overlay manager that wraps a base overlay manager
    /// and simulates network packet loss for consensus testing.
    pub fn create_lossy_overlay_manager(
        base_overlay_manager: Arc<dyn ConsensusOverlayManager + Send + Sync + 'static>,
        config: lossy_overlay::LossyOverlayOpts,
    ) -> ConsensusOverlayManagerPtr {
        lossy_overlay::LossyOverlayManager::create(base_overlay_manager, config)
    }

    /// Create ADNL overlay manager
    pub fn create_adnl_overlay_manager(
        runtime_handle: tokio::runtime::Handle,
        stack: Arc<NetworkStack>,
        broadcast_hops: Option<u8>,
        track_private_peers: bool,
    ) -> Result<ConsensusOverlayManagerPtr> {
        adnl_overlay::AdnlOverlayManager::create(
            runtime_handle,
            stack,
            broadcast_hops,
            track_private_peers,
        )
    }

    /// Create log replay object
    pub fn create_log_player(log_replay_options: &LogReplayOptions) -> Result<LogPlayerPtr> {
        log_player::LogPlayerImpl::create_log_player(log_replay_options)
    }

    /// Enumerate all log replay objects
    pub fn create_log_players(log_replay_options: &LogReplayOptions) -> Vec<LogPlayerPtr> {
        log_player::LogPlayerImpl::create_log_players(log_replay_options)
    }

    /// Create async key-value storage with RocksDB backend.
    ///
    /// # Arguments
    ///
    /// * `db_path` - **Full path** to database file/directory
    /// * `storage_id` - Identifier for logging and thread naming (e.g., session ID prefix)
    /// * `options` - Configuration options
    ///
    /// # Returns
    ///
    /// Arc-wrapped storage instance (blocks until DB is opened)
    ///
    /// # Errors
    ///
    /// Returns error if DB cannot be opened.
    ///
    /// # Logging
    ///
    /// Logs at INFO level: "AsyncKeyValueStorage {storage_id}: opening at {db_path}"
    pub fn create_async_key_value_storage(
        db_path: impl AsRef<std::path::Path>,
        storage_id: &str,
        options: AsyncKeyValueStorageOptions,
    ) -> Result<AsyncKeyValueStoragePtr> {
        async_key_value_storage::RocksDbAsyncKeyValueStorage::open(db_path, storage_id, options)
    }
}

// ============================================================================
// Session Node
// ============================================================================

/// Session node description (validator in the consensus group)
#[derive(Clone, Debug)]
pub struct SessionNode {
    /// ADNL node short ID
    pub adnl_id: PublicKeyHash,

    /// Node public key
    pub public_key: PublicKey,

    /// Weight of the validator
    pub weight: ValidatorWeight,
}

// ============================================================================
// Block Candidate Types
// ============================================================================

/// Block candidate priority information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockCandidatePriority {
    /// Current round number
    pub round: u32,

    /// First block round after validator group starts
    /// Used for telemetry and monitoring
    pub first_block_round: u32,

    /// Block candidate priority in this round (-1 if not allowed to generate)
    pub priority: i32,
}

/// Block source information combining validator identity and priority
#[derive(Debug, Clone)]
pub struct BlockSourceInfo {
    /// Source validator public key
    pub source: PublicKey,

    /// Priority information
    pub priority: BlockCandidatePriority,
}

/// Validator's block candidate
#[derive(Debug)]
pub struct ValidatorBlockCandidate {
    /// Public key of validator
    pub public_key: PublicKey,

    /// Block's identifier
    pub id: ValidatorBlockId,

    /// Collated file hash
    pub collated_file_hash: BlockHash,

    /// Block's data
    pub data: BlockPayloadPtr,

    /// Block's collated data
    pub collated_data: BlockPayloadPtr,
}

// ============================================================================
// Async Request
// ============================================================================

/// Async request tracking interface
pub trait AsyncRequest: Send + Sync {
    /// Get unique request identifier
    fn get_request_id(&self) -> u32;

    /// Check if request has been cancelled
    fn is_cancelled(&self) -> bool;

    /// Get request creation time
    fn get_creation_time(&self) -> std::time::SystemTime;

    /// Cancel the request
    fn cancel(&self);
}

// ============================================================================
// Session Listener
// ============================================================================

/// Collation parent hint for `SessionListener::on_generate_slot`.
///
/// This allows consensus implementations (e.g. Simplex) to provide explicit
/// previous block ids for collation without changing validator-session behavior.
#[derive(Debug, Clone)]
pub enum CollationParentHint {
    /// Keep existing behavior (validator-session / default ValidatorGroup logic).
    Implicit,
    /// Collate on top of these explicit parents (locked at collation start).
    ///
    /// Intended for Simplex notarized-parent collation (before finalization)
    /// and first-block session starts after merge, where two parents are possible.
    Explicit(Vec<ValidatorBlockId>),
}

/// Purpose of a resolver-driven parent/state lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverPurpose {
    /// Resolve state for Simplex collation on an explicit parent anchor.
    SimplexCollationParent,
    /// Resolve state for candidate-native Simplex validation.
    SimplexValidationParent,
    /// Resolve state for local Catchain speculative collation only.
    CatchainLocalCollation,
}

/// Store-only candidate observation flags sent from consensus to validator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CandidateObservedFlags {
    /// The candidate body is locally available.
    pub body_present: bool,
    /// This block id is currently usable as a Simplex parent anchor.
    pub parent_ready: bool,
    /// The candidate belongs to the local speculative collation chain.
    pub local_collated: bool,
}

/// Options for validator-triggered candidate/body availability requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnsureCandidateAvailabilityOptions {
    /// Why the validator is requesting availability.
    pub purpose: ResolverPurpose,
    /// Request the ancestry required to resolve the candidate state, not just the
    /// direct block body.
    pub include_parent_chain: bool,
}

/// Session listener callbacks API
///
/// This trait defines the callback interface from consensus to higher layer
/// (ValidatorManager). Both catchain-based validator-session and simplex
/// consensus use this interface.
pub trait SessionListener: Send + Sync {
    /// New block candidate appears - needs validation
    ///
    /// Called when a block candidate is received from another validator
    /// and needs to be validated before it can be approved.
    fn on_candidate(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    );

    /// New block should be collated
    ///
    /// Called when this validator should generate a new block.
    /// The `request` parameter can be used to check if the request
    /// was cancelled (e.g., when the slot expired).
    fn on_generate_slot(
        &self,
        source_info: BlockSourceInfo,
        request: AsyncRequestPtr,
        parent: CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    );

    // ---- Catchain-only callbacks ----

    /// New block is committed (catchain only).
    ///
    /// Called for sequential block acceptance callbacks.
    /// Simplex delivers finalized blocks via `on_block_finalized` instead.
    #[allow(clippy::too_many_arguments)]
    fn on_block_committed(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        stats: SessionStats,
    );

    /// Block generation is skipped for the current round (catchain only).
    fn on_block_skipped(&self, round: u32);

    /// Ask validator to retrieve a previously approved block candidate (catchain only).
    fn get_approved_candidate(
        &self,
        source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        collated_data_hash: BlockHash,
        callback: ValidatorBlockCandidateCallback,
    );

    // ---- Simplex-only callbacks ----

    /// A block candidate was observed by Simplex consensus.
    ///
    /// Notifies the validator layer about a candidate so it can be cached
    /// in the resolver without triggering validation. The flags describe
    /// whether the body is present, the block is notarized (parent-ready),
    /// and whether this node collated it.
    ///
    /// Counterpart: C++ `BlockProducerImpl` receives candidates via the
    /// state-resolver pipeline; this callback is the Rust equivalent.
    #[allow(unused_variables)]
    fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
    }

    /// A block has been finalized (FinalCert observed) and is ready for
    /// validator-side acceptance (Simplex only).
    ///
    /// Called immediately when a finalization certificate is collected for a
    /// slot, regardless of whether predecessors have been committed yet.
    ///
    /// `block_id` carries the full `BlockIdExt` (shard, seqno, root_hash,
    /// file_hash) so `ValidatorGroup` can derive the block identity without
    /// relying on sequential `prev_block_ids` tracking.
    #[allow(unused_variables)]
    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
    }
}

// ============================================================================
// Session Interface
// ============================================================================

/// Base session interface
///
/// This is the common interface for all consensus session implementations
/// (both catchain-based validator-session and simplex).
pub trait Session: fmt::Display + Send + Sync {
    /// Signal the session to begin active consensus processing.
    ///
    /// For Simplex sessions, `prev_blocks` and `min_masterchain_block_id`
    /// mirror the C++ `start(blocks, min_mc_block_id)` contract:
    /// - `prev_blocks` is the exact shard parent set for the first block
    /// - `min_masterchain_block_id` is the MC anchor that collation and
    ///   validation must not go behind
    ///
    /// The session overlay is created at `create()` time so it can warm up
    /// connections to peers. The FSM timeout clock only starts after
    /// `start()` is called, preventing premature skip-votes on an
    /// unconnected overlay.
    ///
    /// For Catchain sessions, the parameters are ignored (no-op).
    fn start(&self, prev_blocks: Vec<BlockIdExt>, min_masterchain_block_id: BlockIdExt);

    /// Stop the session (blocks until all threads have stopped)
    /// Database is preserved for potential restart/recovery.
    fn stop(&self);

    /// Request session stop without waiting (non-blocking)
    /// Database is preserved for potential restart/recovery.
    ///
    /// Use this when you need to initiate stop from a context where blocking
    /// is not allowed (e.g., from a locked mutex). Follow up with stop() to
    /// wait for completion.
    fn stop_async(&self);

    /// Destroy the session and its database (blocks until complete)
    /// Use this for expired/GC'd sessions that won't be restarted.
    /// This is the equivalent of C++ ValidatorSession::destroy().
    fn destroy(&self);

    /// Get self as Any for downcasting to concrete session type
    ///
    /// This allows callers to downcast from `SessionPtr` to a specific
    /// session type (e.g., `CatchainSession` or `SimplexSession`) when
    /// consensus-specific methods are needed.
    fn as_any(&self) -> &dyn Any;
}

#[cfg(test)]
mod block_sync_overlay_params_tests {
    //! Cross-checked against C++ `block-sync-overlay.cpp` (current-set authorized_keys)
    //! and `manager.cpp` (prev|curr|next members union)

    use super::*;
    use ton_block::{
        ConfigParam32, ConfigParam34, ConfigParam36, ConfigParamEnum, ConsensusConfig, SigPubKey,
        ValidatorDescr, ValidatorSet,
    };

    /// Deterministic ValidatorDescr from a single-byte seed. `adnl_addr` left
    /// `None` so `ValidatorDescr::adnl_addr()` derives the id from the public
    /// key (matches the C++ `addr.is_zero() ? key : addr` fallback)
    fn vdescr(seed: u8) -> ValidatorDescr {
        let key = SigPubKey::with_bytes([seed; 32]);
        ValidatorDescr::with_params(key, 1, None)
    }

    fn vset(seeds: &[u8]) -> ValidatorSet {
        if seeds.is_empty() {
            return ValidatorSet::default();
        }
        let list: Vec<_> = seeds.iter().map(|&s| vdescr(s)).collect();
        ValidatorSet::new(0, 100, 1, list).unwrap()
    }

    fn config_with(
        prev: ValidatorSet,
        curr: ValidatorSet,
        next: ValidatorSet,
        max_block: u32,
        max_collated: u32,
    ) -> ConfigParams {
        let mut consensus = ConsensusConfig::new();
        consensus.round_candidates = 1;
        consensus.max_block_bytes = max_block;
        consensus.max_collated_bytes = max_collated;

        let mut params = ConfigParams::default();
        params.set_config(ConfigParamEnum::ConfigParam29(consensus)).unwrap();
        if !prev.list().is_empty() {
            params
                .set_config(ConfigParamEnum::ConfigParam32(ConfigParam32::with_validator_set(prev)))
                .unwrap();
        }
        if !curr.list().is_empty() {
            params
                .set_config(ConfigParamEnum::ConfigParam34(ConfigParam34::with_validator_set(curr)))
                .unwrap();
        }
        if !next.list().is_empty() {
            params
                .set_config(ConfigParamEnum::ConfigParam36(ConfigParam36::with_validator_set(next)))
                .unwrap();
        }
        params
    }

    /// Overlapping prev/curr/next yield a sorted-unique members vector covering all three
    #[test]
    fn members_union_dedup() {
        let curr = vset(&[2, 3, 4]);
        let cfg = config_with(vset(&[1, 2, 3]), curr.clone(), vset(&[3, 4, 5]), 1 << 22, 1 << 22);
        let p =
            BlockSyncOverlayParams::from_config(&cfg, /*slots_per_leader_window*/ 4, &curr)
                .unwrap();
        assert_eq!(p.members.len(), 5, "expected dedup union of size 5, got {:?}", p.members);
        let mut sorted = p.members.clone();
        sorted.sort();
        assert_eq!(p.members, sorted, "members must be sorted");
    }

    /// Empty prev/next is not an error: members == current set
    #[test]
    fn members_union_handles_empty_prev_next() {
        let curr = vset(&[10, 11]);
        let cfg = config_with(
            ValidatorSet::default(),
            curr.clone(),
            ValidatorSet::default(),
            1 << 22,
            1 << 22,
        );
        let p =
            BlockSyncOverlayParams::from_config(&cfg, /*slots_per_leader_window*/ 4, &curr)
                .unwrap();
        assert_eq!(p.members.len(), 2);
        assert_eq!(p.authorized_keys.len(), 2);
    }

    /// `authorized_keys` covers the current set only (prev/next-only ids excluded).
    /// C++ parity: block-sync-overlay.cpp line 41-44 iterates `bus.validator_set`,
    /// not the wider overlay_members union
    #[test]
    fn authorized_keys_only_current_set() {
        let curr = vset(&[2, 3]); // current
        let cfg = config_with(
            vset(&[1]), // prev-only
            curr.clone(),
            vset(&[4]), // next-only
            1 << 22,
            1 << 22,
        );
        let p =
            BlockSyncOverlayParams::from_config(&cfg, /*slots_per_leader_window*/ 4, &curr)
                .unwrap();
        let curr_ids: Vec<_> = vset(&[2, 3]).list().iter().map(|v| v.adnl_addr()).collect();
        assert_eq!(p.authorized_keys.len(), 2);
        for id in &curr_ids {
            assert!(p.authorized_keys.contains_key(id));
        }
        // prev-only id (seed=1) and next-only id (seed=4) must NOT be authorized
        let prev_id = vset(&[1]).list()[0].adnl_addr();
        let next_id = vset(&[4]).list()[0].adnl_addr();
        assert!(!p.authorized_keys.contains_key(&prev_id));
        assert!(!p.authorized_keys.contains_key(&next_id));
        // Both must still appear in `members`
        assert!(p.members.contains(&prev_id));
        assert!(p.members.contains(&next_id));
    }

    /// `max_broadcast_size = max_block_bytes + max_collated_bytes + 1 MiB`
    /// (matches C++ `block-sync-overlay.cpp:40`)
    #[test]
    fn authorized_keys_size_from_consensus_config() {
        let curr = vset(&[1]);
        let cfg = config_with(
            ValidatorSet::default(),
            curr.clone(),
            ValidatorSet::default(),
            2 * 1024 * 1024, // 2 MiB
            3 * 1024 * 1024, // 3 MiB
        );
        let p =
            BlockSyncOverlayParams::from_config(&cfg, /*slots_per_leader_window*/ 4, &curr)
                .unwrap();
        let expected = 2 * 1024 * 1024 + 3 * 1024 * 1024 + (1 << 20); // 6 MiB
        let only_key = p.authorized_keys.values().next().copied().unwrap();
        assert_eq!(only_key, expected);
    }

    /// `current_set` carries the same elements as `authorized_keys.keys()` and is
    /// ordered by `validator_set.list()` position
    #[test]
    fn current_set_is_ordered_subset_of_authorized() {
        let curr = vset(&[10, 11, 12]);
        let cfg = config_with(
            ValidatorSet::default(),
            curr.clone(),
            ValidatorSet::default(),
            1 << 22,
            1 << 22,
        );
        let p = BlockSyncOverlayParams::from_config(&cfg, 4, &curr).unwrap();
        assert_eq!(p.current_set.len(), 3);
        assert_eq!(p.authorized_keys.len(), 3);
        for id in &p.current_set {
            assert!(p.authorized_keys.contains_key(id));
        }
        // current_set MUST follow cur_subset.list() order (shuffled per-session
        // order in production), not raw HashMap iteration
        let expected: Vec<_> = curr.list().iter().map(|v| v.adnl_addr()).collect();
        assert_eq!(p.current_set, expected);
    }

    /// When cur_subset differs from cfg.validator_set (the shard subset case in
    /// production), current_set / authorized_keys MUST follow cur_subset, not
    /// the wider masterchain set. members still covers the full union
    #[test]
    fn current_set_uses_cur_subset_not_mc_validator_set() {
        let mc_curr = vset(&[10, 11, 12, 13, 14]);
        // Simulate a shard subset: 3 validators in a different (shuffled) order
        let shard_subset = vset(&[13, 10, 12]);
        let cfg = config_with(
            ValidatorSet::default(),
            mc_curr.clone(),
            ValidatorSet::default(),
            1 << 22,
            1 << 22,
        );
        let p = BlockSyncOverlayParams::from_config(&cfg, 4, &shard_subset).unwrap();
        // current_set follows the shard subset order, NOT the mc validator_set order
        let expected: Vec<_> = shard_subset.list().iter().map(|v| v.adnl_addr()).collect();
        assert_eq!(p.current_set, expected);
        // authorized covers only the shard subset
        assert_eq!(p.authorized_keys.len(), 3);
        let mc_only_id = vset(&[11]).list()[0].adnl_addr();
        assert!(
            !p.authorized_keys.contains_key(&mc_only_id),
            "mc-only validator must NOT be in authorized_keys"
        );
        // members still covers the wider mc set (so overlay survives rotation)
        let mc_only_member = vset(&[14]).list()[0].adnl_addr();
        assert!(p.members.contains(&mc_only_member));
    }

    /// slots_per_leader_window is plumbed through verbatim
    #[test]
    fn slots_per_leader_window_plumbed_through() {
        let curr = vset(&[1]);
        let cfg = config_with(
            ValidatorSet::default(),
            curr.clone(),
            ValidatorSet::default(),
            1 << 22,
            1 << 22,
        );
        let p = BlockSyncOverlayParams::from_config(&cfg, 16, &curr).unwrap();
        assert_eq!(p.slots_per_leader_window, 16);
        let p = BlockSyncOverlayParams::from_config(&cfg, 1, &curr).unwrap();
        assert_eq!(p.slots_per_leader_window, 1);
    }
}
