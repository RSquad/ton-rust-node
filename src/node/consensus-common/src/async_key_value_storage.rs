/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! RocksDB-based async key-value storage implementation.
//!
//! Internal module - use `ConsensusCommonFactory::create_async_key_value_storage()`.
//!
//! # Architecture
//!
//! All operations are async - they return `StorageAsyncResultPtr<T>` immediately.
//! Database access happens exclusively in the DB processing thread.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │ Caller Thread                                                           │
//! │   storage.get/set/erase → posts closure → returns StorageAsyncResultPtr │
//! └────────────────────────────────┬────────────────────────────────────────┘
//!                                  │
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │ DB Processing Thread (kv-db:{storage_id})                               │
//! │   - Opens CatchainPersistentDb (thread-local)                           │
//! │   - Processes closures from task queue                                  │
//! │   - Posts callbacks to callback queue (if enabled)                      │
//! │   - Dumps metrics every 30s                                             │
//! └────────────────────────────────┬────────────────────────────────────────┘
//!                                  │ (if use_callback_thread=true)
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │ Callback Thread (kv-cb:{storage_id})                                    │
//! │   - Executes completion callbacks                                       │
//! │   - Prevents DB stalls from slow callbacks                              │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    utils::{MetricsDumper, MetricsHandle},
    ActivityNodePtr, AsyncKeyValueStorage, AsyncKeyValueStorageOptions, AsyncKeyValueStoragePtr,
    ConsensusCommonFactory, Result, StorageAsyncResult, StorageAsyncResultPtr, StorageGetCallback,
    StorageKey, StoragePrefixScanCallback, StorageValue, StorageWriteCallback,
};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
// TODO: Create a separate storage type (e.g., ConsensusKeyValueDb) in node/storage
// instead of reusing CatchainPersistentDb
use storage::catchain_persistent_db::CatchainPersistentDb;

// ============================================================================
// Constants
// ============================================================================

/// Log target for all storage operations
const LOG_TARGET: &str = "consensus_storage";

/// Thread name prefix for DB processing thread
const DB_THREAD_NAME: &str = "kv-db";

/// Thread name prefix for callback thread
const CALLBACK_THREAD_NAME: &str = "kv-callback";

/// Metrics dump period
const METRICS_DUMP_PERIOD: Duration = Duration::from_secs(30);

/// Thread poll timeout
const THREAD_POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// Periodic log interval during stop wait
const STOP_LOG_INTERVAL: Duration = Duration::from_millis(300);

// ============================================================================
// StorageAsyncResult Implementation (Hidden)
// ============================================================================

/// Internal state for async result.
enum AsyncResultState<T> {
    /// Operation pending
    Pending,
    /// Operation completed with result
    Ready(Result<T>),
    /// Result was already taken
    Taken,
}

/// Implementation of StorageAsyncResult (hidden from users).
struct StorageAsyncResultImpl<T> {
    state: Arc<Mutex<AsyncResultState<T>>>,
    condvar: Arc<Condvar>,
}

impl<T> StorageAsyncResultImpl<T> {
    /// Creates a new pending result.
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(AsyncResultState::Pending)),
            condvar: Arc::new(Condvar::new()),
        })
    }

    /// Sets the result and signals any waiters.
    /// This method is internal - not exposed in the trait.
    fn set(&self, result: Result<T>) {
        let mut guard = self.state.lock().unwrap();
        *guard = AsyncResultState::Ready(result);
        self.condvar.notify_all();
    }
}

impl<T: Clone + Send + Sync + 'static> StorageAsyncResult<T> for StorageAsyncResultImpl<T> {
    fn is_ready(&self) -> bool {
        let guard = self.state.lock().unwrap();
        matches!(*guard, AsyncResultState::Ready(_))
    }

    fn try_get(&self) -> Option<Result<T>> {
        let mut guard = self.state.lock().unwrap();
        match std::mem::replace(&mut *guard, AsyncResultState::Taken) {
            AsyncResultState::Pending => {
                *guard = AsyncResultState::Pending;
                None
            }
            AsyncResultState::Ready(result) => Some(result),
            AsyncResultState::Taken => {
                Some(Err(ton_block::error!("StorageAsyncResult: result already taken")))
            }
        }
    }

    fn wait_timeout(&self, timeout: Duration) -> Option<Result<T>> {
        let mut guard = self.state.lock().unwrap();
        let deadline = Instant::now() + timeout;

        loop {
            match &*guard {
                AsyncResultState::Pending => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return None;
                    }
                    let (new_guard, wait_result) =
                        self.condvar.wait_timeout(guard, remaining).unwrap();
                    guard = new_guard;
                    if wait_result.timed_out() && matches!(*guard, AsyncResultState::Pending) {
                        return None;
                    }
                }
                AsyncResultState::Ready(_) => break,
                AsyncResultState::Taken => {
                    return Some(Err(ton_block::error!(
                        "StorageAsyncResult: result already taken"
                    )));
                }
            }
        }

        // Take the result
        match std::mem::replace(&mut *guard, AsyncResultState::Taken) {
            AsyncResultState::Ready(result) => Some(result),
            _ => Some(Err(ton_block::error!("StorageAsyncResult: unexpected state"))),
        }
    }
}

// ============================================================================
// Helper Functions (pub(crate) for use in lib.rs)
// ============================================================================

/// Wraps a get result into a contains result (Option<Value> -> bool).
///
/// Used by AsyncKeyValueStorage::contains() default implementation in lib.rs.
pub(crate) fn wrap_contains_result(
    get_result: StorageAsyncResultPtr<Option<StorageValue>>,
) -> StorageAsyncResultPtr<bool> {
    Arc::new(ContainsResultWrapper { inner: get_result })
}

/// Wrapper that transforms Option<Value> result to bool.
struct ContainsResultWrapper {
    inner: StorageAsyncResultPtr<Option<StorageValue>>,
}

impl StorageAsyncResult<bool> for ContainsResultWrapper {
    fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    fn try_get(&self) -> Option<Result<bool>> {
        self.inner.try_get().map(|result| result.map(|opt| opt.is_some()))
    }

    fn wait_timeout(&self, timeout: Duration) -> Option<Result<bool>> {
        self.inner.wait_timeout(timeout).map(|result| result.map(|opt| opt.is_some()))
    }
}

// ============================================================================
// Task and Callback Types
// ============================================================================

/// Task closure type - executed in DB processing thread with DB access
type StorageTask = Box<
    dyn FnOnce(&CatchainPersistentDb, &StorageMetrics, &Option<Sender<StorageCallback>>) + Send,
>;

/// Callback closure type - executed in callback thread
type StorageCallback = Box<dyn FnOnce() + Send>;

/// Task descriptor with creation time (for latency tracking)
struct TaskDesc {
    task: StorageTask,
    #[allow(dead_code)]
    creation_time: SystemTime, // For future latency tracking
}

// ============================================================================
// Metrics
// ============================================================================

/// Storage metrics
#[allow(dead_code)]
struct StorageMetrics {
    reads: metrics::Counter,
    writes: metrics::Counter,
    erases: metrics::Counter,
    prefix_scans: metrics::Counter,
    syncs: metrics::Counter,
    db_queue_posts: metrics::Counter,
    db_queue_pulls: metrics::Counter,
    callback_queue_posts: metrics::Counter,
    callback_queue_pulls: metrics::Counter,
}

impl StorageMetrics {
    fn new(metrics_receiver: &MetricsHandle) -> Self {
        let sink = metrics_receiver.sink();
        Self {
            reads: sink.register_counter(&"async_kv_reads".into()),
            writes: sink.register_counter(&"async_kv_writes".into()),
            erases: sink.register_counter(&"async_kv_erases".into()),
            prefix_scans: sink.register_counter(&"async_kv_prefix_scans".into()),
            syncs: sink.register_counter(&"async_kv_syncs".into()),
            db_queue_posts: sink.register_counter(&"async_kv_db_queue.posts".into()),
            db_queue_pulls: sink.register_counter(&"async_kv_db_queue.pulls".into()),
            callback_queue_posts: sink.register_counter(&"async_kv_callback_queue.posts".into()),
            callback_queue_pulls: sink.register_counter(&"async_kv_callback_queue.pulls".into()),
        }
    }
}

// ============================================================================
// RocksDB Async Key-Value Storage Implementation
// ============================================================================

/// RocksDB-based async key-value storage.
pub struct RocksDbAsyncKeyValueStorage {
    /// Path to database
    path: PathBuf,
    /// Storage identifier (for logging)
    storage_id: String,
    /// Task queue sender (to DB processing thread)
    task_tx: Sender<TaskDesc>,
    /// Callback queue sender (to callback thread, if enabled)
    callback_tx: Option<Sender<StorageCallback>>,
    /// Pending operation count
    pending_count: Arc<AtomicUsize>,
    /// Stop request flag
    is_stop_requested: Arc<AtomicBool>,
    /// DB processing thread stopped flag
    db_thread_stopped: Arc<AtomicBool>,
    /// Callback thread stopped flag (always present, set true immediately if no callback thread)
    callback_thread_stopped: Arc<AtomicBool>,
    /// Destroy on drop flag
    mark_for_destroy: Arc<AtomicBool>,
    /// DB processing thread handle (for join on drop)
    /// No Mutex needed - only accessed in Drop which has &mut self
    db_thread_handle: Option<JoinHandle<()>>,
    /// Callback thread handle (for join on drop, None if no callback thread)
    /// No Mutex needed - only accessed in Drop which has &mut self
    callback_thread_handle: Option<JoinHandle<()>>,
    /// Metrics receiver
    _metrics_receiver: MetricsHandle,
    /// Activity node for DB thread
    _db_activity_node: ActivityNodePtr,
    /// Activity node for callback thread (None if no callback thread)
    _callback_activity_node: Option<ActivityNodePtr>,
    /// Metrics counter for queue posts
    db_queue_posts: metrics::Counter,
    /// Metrics counter for callback queue posts
    callback_queue_posts: Option<metrics::Counter>,
}

impl RocksDbAsyncKeyValueStorage {
    /// Opens a new RocksDB async key-value storage.
    ///
    /// **Blocks** until database is opened or error occurs.
    pub fn open(
        path: impl AsRef<Path>,
        storage_id: &str,
        options: AsyncKeyValueStorageOptions,
    ) -> Result<AsyncKeyValueStoragePtr> {
        let path = path.as_ref().to_path_buf();
        let storage_id_owned = storage_id.to_string();

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: opening at {}",
            storage_id,
            path.display()
        );

        // Create metrics receiver
        let metrics_receiver = MetricsHandle::new(Some(Duration::from_secs(60)));

        // Create metrics counters
        let db_queue_posts =
            metrics_receiver.sink().register_counter(&"async_kv_db_queue.posts".into());
        let callback_queue_posts = if options.use_callback_thread {
            Some(metrics_receiver.sink().register_counter(&"async_kv_callback_queue.posts".into()))
        } else {
            None
        };

        // Create task queue
        let (task_tx, task_rx) = bounded::<TaskDesc>(10000);

        // Create callback queue (if enabled)
        let (callback_tx, callback_rx) = if options.use_callback_thread {
            let (tx, rx) = bounded::<StorageCallback>(10000);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        // Create atomic flags
        let is_stop_requested = Arc::new(AtomicBool::new(false));
        let db_thread_stopped = Arc::new(AtomicBool::new(false));
        let mark_for_destroy = Arc::new(AtomicBool::new(false));
        let pending_count = Arc::new(AtomicUsize::new(0));

        // Create activity nodes
        let db_activity_node = ConsensusCommonFactory::create_activity_node(format!(
            "{}:{}",
            DB_THREAD_NAME, storage_id
        ));

        // Create one-shot channel for init result
        let (init_tx, init_rx) = bounded::<Result<()>>(1);

        // Clone variables for DB thread
        let path_for_db = path.clone();
        let storage_id_for_db = storage_id_owned.clone();
        let is_stop_for_db = is_stop_requested.clone();
        let db_stopped_for_db = db_thread_stopped.clone();
        let mark_destroy_for_db = mark_for_destroy.clone();
        let pending_for_db = pending_count.clone();
        let metrics_receiver_for_db = metrics_receiver.clone();
        let db_activity_for_db = db_activity_node.clone();
        let callback_tx_for_db = callback_tx.clone();

        // Start DB processing thread
        let db_thread = thread::Builder::new()
            .name(format!("{}:{}", DB_THREAD_NAME, storage_id))
            .spawn(move || {
                Self::db_processing_loop(
                    path_for_db,
                    storage_id_for_db,
                    task_rx,
                    callback_tx_for_db,
                    is_stop_for_db,
                    db_stopped_for_db,
                    mark_destroy_for_db,
                    pending_for_db,
                    metrics_receiver_for_db,
                    db_activity_for_db,
                    init_tx,
                );
            })
            .map_err(|e| ton_block::error!("Failed to spawn DB processing thread: {}", e))?;

        // Wait for initialization result
        let init_result = init_rx.recv().map_err(|_| {
            ton_block::error!("AsyncKeyValueStorage {}: init channel closed", storage_id)
        })?;

        if let Err(e) = init_result {
            // Wait for DB thread to exit
            let _ = db_thread.join();
            return Err(e);
        }

        // Create callback thread stopped flag (always present)
        // If no callback thread, set to true immediately
        let callback_thread_stopped = Arc::new(AtomicBool::new(!options.use_callback_thread));

        // Start callback thread (if enabled)
        let (callback_thread_handle, callback_activity_node) =
            if let Some(callback_rx) = callback_rx {
                let callback_activity = ConsensusCommonFactory::create_activity_node(format!(
                    "{}:{}",
                    CALLBACK_THREAD_NAME, storage_id
                ));

                let is_stop_for_callback = is_stop_requested.clone();
                let callback_stopped_clone = callback_thread_stopped.clone();
                let callback_activity_clone = callback_activity.clone();
                let storage_id_for_callback = storage_id_owned.clone();
                let metrics_receiver_for_callback = metrics_receiver.clone();

                let handle = thread::Builder::new()
                    .name(format!("{}:{}", CALLBACK_THREAD_NAME, storage_id))
                    .spawn(move || {
                        Self::callback_loop(
                            storage_id_for_callback,
                            callback_rx,
                            is_stop_for_callback,
                            callback_stopped_clone,
                            callback_activity_clone,
                            metrics_receiver_for_callback,
                        );
                    })
                    .map_err(|e| ton_block::error!("Failed to spawn callback thread: {}", e))?;

                (Some(handle), Some(callback_activity))
            } else {
                // No callback thread
                (None, None)
            };

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: opened at {}",
            storage_id,
            path.display()
        );

        Ok(Arc::new(Self {
            path,
            storage_id: storage_id_owned,
            task_tx,
            callback_tx,
            pending_count,
            is_stop_requested,
            db_thread_stopped,
            callback_thread_stopped,
            mark_for_destroy,
            db_thread_handle: Some(db_thread),
            callback_thread_handle: callback_thread_handle,
            _metrics_receiver: metrics_receiver,
            _db_activity_node: db_activity_node,
            _callback_activity_node: callback_activity_node,
            db_queue_posts,
            callback_queue_posts,
        }))
    }

    /// DB processing loop - runs in dedicated thread, owns CatchainPersistentDb
    fn db_processing_loop(
        path: PathBuf,
        storage_id: String,
        task_rx: Receiver<TaskDesc>,
        callback_tx: Option<Sender<StorageCallback>>,
        is_stop_requested: Arc<AtomicBool>,
        is_stopped: Arc<AtomicBool>,
        mark_for_destroy: Arc<AtomicBool>,
        pending_count: Arc<AtomicUsize>,
        metrics_receiver: MetricsHandle,
        activity_node: ActivityNodePtr,
        init_tx: Sender<Result<()>>,
    ) {
        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: DB processing thread starting at {}",
            storage_id,
            path.display()
        );

        // Open DB using CatchainPersistentDb pattern (thread-local)
        let db = match CatchainPersistentDb::new(&path, &storage_id) {
            Ok(db) => {
                let _ = init_tx.send(Ok(()));
                db
            }
            Err(e) => {
                log::error!(
                    target: LOG_TARGET,
                    "AsyncKeyValueStorage {}: failed to open DB at {}: {}",
                    storage_id,
                    path.display(),
                    e
                );
                let _ = init_tx.send(Err(e));
                is_stopped.store(true, Ordering::SeqCst);
                return;
            }
        };

        // Create metrics
        let metrics = StorageMetrics::new(&metrics_receiver);

        // Create metrics dumper
        let mut metrics_dumper = MetricsDumper::new();
        metrics_dumper.add_derivative_metric("async_kv_reads");
        metrics_dumper.add_derivative_metric("async_kv_writes");
        metrics_dumper.add_derivative_metric("async_kv_erases");
        metrics_dumper.add_derivative_metric("async_kv_prefix_scans");
        metrics_dumper.add_derivative_metric("async_kv_syncs");
        metrics_dumper.add_derivative_metric("async_kv_db_queue.posts");
        metrics_dumper.add_derivative_metric("async_kv_db_queue.pulls");
        metrics_dumper.add_derivative_metric("async_kv_callback_queue.posts");
        metrics_dumper.add_derivative_metric("async_kv_callback_queue.pulls");

        let mut next_metrics_dump_time = SystemTime::now() + METRICS_DUMP_PERIOD;

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: DB processing thread started at {}",
            storage_id,
            path.display()
        );

        // Processing loop
        loop {
            activity_node.tick();

            // Check stop flag
            if is_stop_requested.load(Ordering::SeqCst) {
                log::debug!(
                    target: LOG_TARGET,
                    "AsyncKeyValueStorage {}: stop requested, exiting DB processing loop",
                    storage_id
                );
                break;
            }

            // Pull task with timeout
            match task_rx.recv_timeout(THREAD_POLL_TIMEOUT) {
                Ok(task_desc) => {
                    metrics.db_queue_pulls.increment(1);

                    // Decrement pending count before execution so that sync() sees
                    // consistent state when its marker task signals completion
                    pending_count.fetch_sub(1, Ordering::SeqCst);

                    // Execute task with DB access
                    (task_desc.task)(&db, &metrics, &callback_tx);
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    log::warn!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: task queue disconnected",
                        storage_id
                    );
                    break;
                }
            }

            // Dump metrics periodically
            if next_metrics_dump_time.elapsed().is_ok() {
                metrics_dumper.update(&metrics_receiver);

                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {} metrics:",
                        storage_id
                    );
                    metrics_dumper.dump(|string| {
                        log::debug!(target: LOG_TARGET, "{}{}", storage_id, string);
                    });
                }

                next_metrics_dump_time = SystemTime::now() + METRICS_DUMP_PERIOD;
            }
        }

        // Cleanup
        if mark_for_destroy.load(Ordering::SeqCst) {
            log::info!(
                target: LOG_TARGET,
                "AsyncKeyValueStorage {}: destroying DB at {}",
                storage_id,
                path.display()
            );
            drop(db);
            if let Err(e) = std::fs::remove_dir_all(&path) {
                log::warn!(
                    target: LOG_TARGET,
                    "AsyncKeyValueStorage {}: failed to remove DB: {}",
                    storage_id,
                    e
                );
            }
        }

        is_stopped.store(true, Ordering::SeqCst);

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: DB processing thread stopped at {}",
            storage_id,
            path.display()
        );
    }

    /// Callback loop - runs in dedicated thread, executes callbacks
    fn callback_loop(
        storage_id: String,
        callback_rx: Receiver<StorageCallback>,
        is_stop_requested: Arc<AtomicBool>,
        is_stopped: Arc<AtomicBool>,
        activity_node: ActivityNodePtr,
        metrics_receiver: MetricsHandle,
    ) {
        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: callback thread starting",
            storage_id
        );

        let metrics_pulls =
            metrics_receiver.sink().register_counter(&"async_kv_callback_queue.pulls".into());

        loop {
            activity_node.tick();

            if is_stop_requested.load(Ordering::SeqCst) {
                break;
            }

            match callback_rx.recv_timeout(THREAD_POLL_TIMEOUT) {
                Ok(callback) => {
                    metrics_pulls.increment(1);
                    callback();
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            }
        }

        is_stopped.store(true, Ordering::SeqCst);

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: callback thread stopped",
            storage_id
        );
    }

    /// Posts a task to the DB processing thread
    fn post_task(&self, task: StorageTask) {
        self.pending_count.fetch_add(1, Ordering::SeqCst);
        self.db_queue_posts.increment(1);

        let _ = self.task_tx.send(TaskDesc { task, creation_time: SystemTime::now() });
    }

    /// Invokes callback (either directly or via callback queue)
    fn invoke_callback<F: FnOnce() + Send + 'static>(
        callback_tx: &Option<Sender<StorageCallback>>,
        metrics: &StorageMetrics,
        f: F,
    ) {
        if let Some(tx) = callback_tx {
            metrics.callback_queue_posts.increment(1);
            let _ = tx.send(Box::new(f));
        } else {
            f();
        }
    }

    /// Internal stop - called from Drop
    fn stop_internal(&self) {
        log::debug!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: stopping...",
            self.storage_id
        );

        self.is_stop_requested.store(true, Ordering::SeqCst);

        // Wait for both threads with periodic logging
        let mut last_log_time = SystemTime::now();
        loop {
            let db_stopped = self.db_thread_stopped.load(Ordering::SeqCst);
            let callback_stopped = self.callback_thread_stopped.load(Ordering::SeqCst);

            if db_stopped && callback_stopped {
                break;
            }

            std::thread::sleep(Duration::from_millis(10));

            if last_log_time.elapsed().unwrap_or_default() > STOP_LOG_INTERVAL {
                if !db_stopped {
                    log::debug!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: waiting for DB processing thread to stop...",
                        self.storage_id
                    );
                }
                if !callback_stopped {
                    log::debug!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: waiting for callback thread to stop...",
                        self.storage_id
                    );
                }
                last_log_time = SystemTime::now();
            }
        }

        log::debug!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: stopped",
            self.storage_id
        );
    }

    /// Helper to format key for trace logs (first 16 bytes as hex)
    fn format_key(key: &[u8]) -> String {
        let len = std::cmp::min(key.len(), 16);
        hex::encode(&key[..len])
    }
}

impl AsyncKeyValueStorage for RocksDbAsyncKeyValueStorage {
    fn get(
        &self,
        key: StorageKey,
        on_complete: Option<StorageGetCallback>,
    ) -> StorageAsyncResultPtr<Option<StorageValue>> {
        log::trace!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: get key={}...",
            self.storage_id,
            Self::format_key(&key)
        );

        let result = StorageAsyncResultImpl::<Option<StorageValue>>::new();
        let result_clone = result.clone();
        let storage_id = self.storage_id.clone();

        self.post_task(Box::new(move |db, metrics, callback_tx| {
            metrics.reads.increment(1);

            // Access underlying rocksdb through Deref chain
            use std::ops::Deref;
            match db.deref().deref().get(&key) {
                Ok(value) => {
                    log::trace!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: get key={}... found={}",
                        storage_id,
                        Self::format_key(&key),
                        value.is_some()
                    );
                    let value_clone = value.clone();
                    result_clone.set(Ok(value));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || {
                            callback(Ok(value_clone))
                        });
                    }
                }
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: get failed: {}",
                        storage_id,
                        e
                    );
                    let err_msg = format!("get failed: {}", e);
                    result_clone.set(Err(ton_block::error!("{}", err_msg)));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || {
                            callback(Err(ton_block::error!("{}", err_msg)));
                        });
                    }
                }
            };
        }));

        result
    }

    fn get_by_prefix(
        &self,
        prefix: StorageKey,
        on_complete: Option<StoragePrefixScanCallback>,
    ) -> StorageAsyncResultPtr<Vec<(StorageKey, StorageValue)>> {
        log::trace!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: get_by_prefix prefix={}...",
            self.storage_id,
            Self::format_key(&prefix)
        );

        let result = StorageAsyncResultImpl::<Vec<(StorageKey, StorageValue)>>::new();
        let result_clone = result.clone();
        let storage_id = self.storage_id.clone();

        self.post_task(Box::new(move |db, metrics, callback_tx| {
            metrics.prefix_scans.increment(1);

            let mut results = Vec::new();
            let mut scan_error: Option<String> = None;

            // Access underlying rocksdb through Deref chain:
            // CatchainPersistentDb -> RocksDb -> rocksdb::DBWithThreadMode
            use std::ops::Deref;
            let iter = db
                .deref()
                .deref()
                .iterator(rocksdb::IteratorMode::From(&prefix, rocksdb::Direction::Forward));

            for item in iter {
                match item {
                    Ok((key, value)) => {
                        if key.starts_with(&prefix) {
                            results.push((key.to_vec(), value.to_vec()));
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        log::error!(
                            target: LOG_TARGET,
                            "AsyncKeyValueStorage {}: prefix scan failed: {}",
                            storage_id,
                            e
                        );
                        scan_error = Some(format!("prefix scan failed: {}", e));
                        break;
                    }
                }
            }

            // Return error if scan failed
            if let Some(err_msg) = scan_error {
                let err_msg_clone = err_msg.clone();
                result_clone.set(Err(ton_block::error!("{}", err_msg)));
                if let Some(callback) = on_complete {
                    Self::invoke_callback(callback_tx, metrics, move || {
                        callback(Err(ton_block::error!("{}", err_msg_clone)));
                    });
                }
                return;
            }

            log::trace!(
                target: LOG_TARGET,
                "AsyncKeyValueStorage {}: get_by_prefix prefix={}... found={}",
                storage_id,
                Self::format_key(&prefix),
                results.len()
            );

            let results_clone = results.clone();
            result_clone.set(Ok(results));
            if let Some(callback) = on_complete {
                Self::invoke_callback(callback_tx, metrics, move || callback(Ok(results_clone)));
            }
        }));

        result
    }

    fn set(
        &self,
        key: StorageKey,
        value: StorageValue,
        on_complete: Option<StorageWriteCallback>,
    ) -> StorageAsyncResultPtr<()> {
        log::trace!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: set key={}... value_len={}",
            self.storage_id,
            Self::format_key(&key),
            value.len()
        );

        let result = StorageAsyncResultImpl::<()>::new();
        let result_clone = result.clone();
        let storage_id = self.storage_id.clone();

        self.post_task(Box::new(move |db, metrics, callback_tx| {
            metrics.writes.increment(1);

            // Access underlying rocksdb through Deref chain
            use std::ops::Deref;
            match db.deref().deref().put(&key, &value) {
                Ok(()) => {
                    result_clone.set(Ok(()));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || callback(Ok(())));
                    }
                }
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: set failed: {}",
                        storage_id,
                        e
                    );
                    let err_msg = format!("set failed: {}", e);
                    result_clone.set(Err(ton_block::error!("{}", err_msg)));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || {
                            callback(Err(ton_block::error!("{}", err_msg)));
                        });
                    }
                }
            };
        }));

        result
    }

    fn erase(
        &self,
        key: StorageKey,
        on_complete: Option<StorageWriteCallback>,
    ) -> StorageAsyncResultPtr<()> {
        log::trace!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: erase key={}...",
            self.storage_id,
            Self::format_key(&key)
        );

        let result = StorageAsyncResultImpl::<()>::new();
        let result_clone = result.clone();
        let storage_id = self.storage_id.clone();

        self.post_task(Box::new(move |db, metrics, callback_tx| {
            metrics.erases.increment(1);

            // Access underlying rocksdb through Deref chain
            use std::ops::Deref;
            match db.deref().deref().delete(&key) {
                Ok(()) => {
                    result_clone.set(Ok(()));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || callback(Ok(())));
                    }
                }
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "AsyncKeyValueStorage {}: erase failed: {}",
                        storage_id,
                        e
                    );
                    let err_msg = format!("erase failed: {}", e);
                    result_clone.set(Err(ton_block::error!("{}", err_msg)));
                    if let Some(callback) = on_complete {
                        Self::invoke_callback(callback_tx, metrics, move || {
                            callback(Err(ton_block::error!("{}", err_msg)));
                        });
                    }
                }
            };
        }));

        result
    }

    fn sync(&self, timeout: Option<Duration>) -> Result<()> {
        log::debug!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: sync started",
            self.storage_id
        );

        let start_time = std::time::Instant::now();

        // 1. Wait for DB queue to drain
        let db_sync = StorageAsyncResultImpl::<()>::new();
        let db_sync_clone = db_sync.clone();
        let storage_id = self.storage_id.clone();

        self.post_task(Box::new(move |_db, metrics, _callback_tx| {
            metrics.syncs.increment(1);
            log::trace!(
                target: LOG_TARGET,
                "AsyncKeyValueStorage {}: DB sync marker reached",
                storage_id
            );
            db_sync_clone.set(Ok(()));
        }));

        // Wait for DB queue with optional timeout
        match timeout {
            Some(t) => {
                db_sync.wait_timeout(t).ok_or_else(|| {
                    ton_block::error!("sync: timeout expired waiting for DB queue")
                })??;
            }
            None => {
                db_sync.wait()?;
            }
        }

        // 2. Wait for callback queue to drain (if enabled)
        // Uses same StorageAsyncResultImpl pattern as DB queue (DRY principle)
        if let Some(ref callback_tx) = self.callback_tx {
            // Calculate remaining timeout
            let remaining_timeout = timeout.map(|t| {
                let elapsed = start_time.elapsed();
                t.saturating_sub(elapsed)
            });

            // Check if timeout already expired
            if let Some(t) = remaining_timeout {
                if t.is_zero() {
                    return Err(ton_block::error!("sync: timeout expired waiting for DB queue"));
                }
            }

            let callback_sync = StorageAsyncResultImpl::<()>::new();
            let callback_sync_clone = callback_sync.clone();

            if let Some(ref counter) = self.callback_queue_posts {
                counter.increment(1);
            }
            let _ = callback_tx.send(Box::new(move || {
                callback_sync_clone.set(Ok(()));
            }));

            // Wait for callback queue with optional timeout
            match remaining_timeout {
                Some(t) => {
                    callback_sync
                        .wait_timeout(t)
                        .ok_or_else(|| ton_block::error!("sync: callback queue timeout"))??;
                }
                None => {
                    callback_sync.wait()?;
                }
            }
        }

        log::debug!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: sync completed",
            self.storage_id
        );

        Ok(())
    }

    fn pending_count(&self) -> usize {
        self.pending_count.load(Ordering::SeqCst)
    }

    fn mark_for_destroy(&self) {
        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: marked for destroy",
            self.storage_id
        );
        self.mark_for_destroy.store(true, Ordering::SeqCst);
    }

    fn get_path(&self) -> &Path {
        &self.path
    }

    fn get_storage_id(&self) -> &str {
        &self.storage_id
    }
}

impl Drop for RocksDbAsyncKeyValueStorage {
    fn drop(&mut self) {
        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: dropping at {}",
            self.storage_id,
            self.path.display()
        );

        // Stop threads
        self.stop_internal();

        // Join threads
        if let Some(handle) = self.db_thread_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_thread_handle.take() {
            let _ = handle.join();
        }

        log::info!(
            target: LOG_TARGET,
            "AsyncKeyValueStorage {}: dropped at {}",
            self.storage_id,
            self.path.display()
        );
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "tests/test_async_key_value_storage.rs"]
mod tests;
