/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Session implementation for Simplex consensus
//!
//! Multi-threaded wrapper for SessionProcessor. Manages thread creation,
//! task queues, metrics dumping, and session lifecycle.
//!
//! # Threading Model
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────────────┐
//! │ SessionImpl                                                               │
//! │                                                                           │
//! │  ┌─────────────────────────────┐   ┌─────────────────────────────┐        │
//! │  │ SXMAIN:{session_id}         │   │ SXCB:{session_id}           │        │
//! │  │ (Main Thread)               │   │ (Callback Thread, optional) │        │
//! │  │                             │   │                             │        │
//! │  │  SessionProcessor           │   │  Pulls from callbacks_queue │        │
//! │  │  ├─ SimplexState FSM        │   │  Invokes SessionListener:   │        │
//! │  │  ├─ check_all() loop        │   │  ├─ on_candidate            │        │
//! │  │  └─ process events          │   │  ├─ on_generate_slot        │        │
//! │  │                             │   │  └─ on_block_committed      │        │
//! │  │  Pulls from main_task_queue │   │                             │        │
//! │  └─────────────────────────────┘   └─────────────────────────────┘        │
//! │              ▲                                                            │
//! │              │ ReceiverListener callbacks                                 │
//! │              │                                                            │
//! │  ┌───────────────────────────────────────────────────────────────┐        │
//! │  │ SXRCV:{session_id} (Receiver Thread)                          │        │
//! │  │  ├─ Deserialize TL messages                                   │        │
//! │  │  ├─ Verify signatures                                         │        │
//! │  │  └─ Post to main_task_queue via ReceiverListenerImpl          │        │
//! │  └───────────────────────────────────────────────────────────────┘        │
//! │              │                                                            │
//! │              ▼ CatchainOverlay                                            │
//! │  ┌───────────────────────────────────────────────────────────────┐        │
//! │  │ ConsensusOverlayManager (from consensus-common)               │        │
//! │  └───────────────────────────────────────────────────────────────┘        │
//! └───────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Components
//!
//! - **SessionImpl**: Public session interface, owns threads and task queues
//! - **ReceiverListenerImpl**: Bridge between Receiver and SessionProcessor
//! - **TaskQueueImpl**: Crossbeam-based task queue with latency monitoring
//!
//! # Thread Communication
//!
//! All cross-thread communication uses task queues with closures:
//! - `main_task_queue`: Network events → SessionProcessor
//! - `callbacks_task_queue`: SessionProcessor → SessionListener (if enabled)

use crate::{
    receiver::{ReceiverListener, ReceiverListenerPtr},
    session_description::SessionDescription,
    session_processor::SessionProcessor,
    startup_recovery::{SessionStartupRecoveryOptions, SessionStartupRecoveryProcessor},
    task_queue::{CallbackTaskPtr, CallbackTaskQueuePtr, TaskPtr, TaskQueue, TaskQueuePtr},
    ActivityNodePtr, ConsensusOverlayManagerPtr, ConsensusSession, LogReplayOptions, MetricsHandle,
    PrivateKey, RawVoteData, SessionId, SessionListenerPtr, SessionNode, SessionOptions,
    SessionPtr, SessionReplayListenerPtr, SimplexSession, ValidatorWeight,
};
use consensus_common::{
    check_execution_time,
    utils::{
        add_compute_percentage_metric, add_compute_relative_metric, add_compute_result_metric,
        get_elapsed_time, MetricsDumper,
    },
};
use crossbeam::channel::{bounded, Sender};
use std::{
    any::Any,
    cell::Cell,
    cmp,
    collections::BTreeMap,
    fmt, panic,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    thread,
    time::{Duration, SystemTime},
};
use ton_api::ton::consensus::{
    simplex::{Certificate, Vote},
    CandidateData,
};
use ton_block::{error, Error, Result, ShardIdent, UInt256};

/*
    Constants
*/

const MAIN_LOOP_NAME: &str = "SXMAIN"; // Simplex main processing thread
const CALLBACKS_LOOP_NAME: &str = "SXCB"; // Simplex callbacks thread
const TASK_QUEUE_WARN_PROCESSING_LATENCY: Duration = Duration::from_millis(1000); // max processing latency
const TASK_QUEUE_LATENCY_WARN_DUMP_PERIOD: Duration = Duration::from_millis(2000); // latency warning dump period
const SESSION_METRICS_DUMP_PERIOD_MS: u64 = 15000; // period of metrics dump
const SESSION_PROFILING_DUMP_PERIOD_MS: u64 = 30000; // period of profiling dump
const SESSION_HEALTH_CHECK_PERIOD_MS: u64 = 20000;
const LOG_TARGET_PROFILING: &str = "simplex_profiling"; // log target for profiling

/*
===================================================================================================
    ReceiverListenerImpl - bridge between Receiver and SessionProcessor
===================================================================================================
*/

/// Implementation of ReceiverListener that posts callbacks to the main task queue.
struct ReceiverListenerImpl {
    task_queue: TaskQueuePtr,
    session_id: SessionId,
}

impl ReceiverListener for ReceiverListenerImpl {
    /// Handle incoming vote from the network
    fn on_vote(&self, source_idx: u32, vote: Vote, raw_vote: RawVoteData) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_vote(source_idx, vote, raw_vote);
        }));
    }

    /// Handle incoming block candidate (from broadcast or query response)
    fn on_candidate_received(
        &self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_candidate_received(source_idx, candidate, notar_cert);
        }));
    }

    /// Handle activity updates from the receiver
    fn on_activity(&self, active_weight: ValidatorWeight, last_activity: Vec<Option<SystemTime>>) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_activity(active_weight, last_activity);
        }));
    }

    /// Handle incoming certificate from network
    fn on_certificate(&self, source_idx: u32, certificate: Certificate) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_certificate(source_idx, certificate);
        }));
    }

    /// Handle RequestCandidate cache miss by delegating to SessionProcessor
    fn on_candidate_query_fallback(
        &self,
        slot: crate::block::SlotIndex,
        block_hash: UInt256,
        want_notar: bool,
        response_callback: consensus_common::QueryResponseCallback,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.handle_candidate_query_fallback(
                slot,
                block_hash,
                want_notar,
                response_callback,
            );
        }));
    }
}

impl ReceiverListenerImpl {
    /// Create new ReceiverListenerImpl
    fn create(task_queue: TaskQueuePtr, session_id: SessionId) -> Arc<Self> {
        Arc::new(Self { task_queue, session_id })
    }
}

impl Drop for ReceiverListenerImpl {
    fn drop(&mut self) {
        log::debug!("Dropped ReceiverListenerImpl for session {}", self.session_id.to_hex_string());
    }
}

/*
===================================================================================================
    TaskQueue Implementation
===================================================================================================
*/

trait DefaultTaskFactory<FuncPtr: Send + 'static> {
    fn create_default_task() -> FuncPtr;
}

impl DefaultTaskFactory<TaskPtr> for TaskPtr {
    fn create_default_task() -> TaskPtr {
        Box::new(|_processor: &mut SessionProcessor| {})
    }
}

impl DefaultTaskFactory<CallbackTaskPtr> for CallbackTaskPtr {
    fn create_default_task() -> CallbackTaskPtr {
        Box::new(|| {})
    }
}

struct TaskDesc<FuncPtr> {
    task: FuncPtr,             // closure for execution
    creation_time: SystemTime, // task creation time
}

struct TaskQueueImpl<FuncPtr> {
    name: String,                                                         // queue name
    queue_sender: crossbeam::channel::Sender<Box<TaskDesc<FuncPtr>>>,     // queue sender
    queue_receiver: crossbeam::channel::Receiver<Box<TaskDesc<FuncPtr>>>, // queue receiver
    post_counter: metrics::Counter,                                       // counter for queue posts
    pull_counter: metrics::Counter,                                       // counter for queue pulls
    is_overloaded: Arc<AtomicBool>, // atomic flag for overload indication
    linked_queue: Option<Arc<dyn TaskQueue<FuncPtr>>>, // linked task queue to wake up
}

impl<FuncPtr> TaskQueue<FuncPtr> for TaskQueueImpl<FuncPtr>
where
    FuncPtr: Send + DefaultTaskFactory<FuncPtr> + 'static,
{
    fn is_overloaded(&self) -> bool {
        self.is_overloaded.load(Ordering::Relaxed)
    }

    fn is_empty(&self) -> bool {
        self.queue_receiver.is_empty()
    }

    fn post_closure(&self, task: FuncPtr) {
        let task_desc = Box::new(TaskDesc::<FuncPtr> { task, creation_time: SystemTime::now() });
        if let Err(send_error) = self.queue_sender.send(task_desc) {
            log::error!("SimplexSession post closure error: {}", send_error);
        } else {
            self.post_counter.increment(1);

            if let Some(ref linked_queue) = &self.linked_queue {
                linked_queue.post_closure(FuncPtr::create_default_task());
            }
        }
    }

    fn pull_closure(
        &self,
        timeout: Duration,
        last_warn_dump_time: &mut SystemTime,
    ) -> Option<FuncPtr> {
        match self.queue_receiver.recv_timeout(timeout) {
            Ok(task_desc) => {
                let processing_latency = get_elapsed_time(&task_desc.creation_time);
                if processing_latency > TASK_QUEUE_WARN_PROCESSING_LATENCY {
                    self.is_overloaded.store(true, Ordering::Release);

                    if let Ok(warn_elapsed) = last_warn_dump_time.elapsed() {
                        if warn_elapsed > TASK_QUEUE_LATENCY_WARN_DUMP_PERIOD {
                            log::warn!(
                                "SimplexSession {} task queue latency is {:.3}s \
                                (expected max latency is {:.3}s)",
                                self.name,
                                processing_latency.as_secs_f64(),
                                TASK_QUEUE_WARN_PROCESSING_LATENCY.as_secs_f64()
                            );
                            *last_warn_dump_time = SystemTime::now();
                        }
                    }
                } else {
                    self.is_overloaded.store(false, Ordering::Release);
                }

                self.pull_counter.increment(1);

                Some(task_desc.task)
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                self.is_overloaded.store(false, Ordering::Release);
                None
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                const DISCONNECTED_SLEEP: Duration = Duration::from_millis(100);
                log::warn!(
                    "SimplexSession {} task queue disconnected. Waiting for {}ms before returning.",
                    self.name,
                    DISCONNECTED_SLEEP.as_millis()
                );
                thread::sleep(DISCONNECTED_SLEEP);

                self.is_overloaded.store(false, Ordering::Release);
                None
            }
        }
    }

    fn flush(&self) {
        while !self.queue_receiver.is_empty() {
            let _result = self.queue_receiver.try_recv();
        }
    }
}

impl<FuncPtr> TaskQueueImpl<FuncPtr>
where
    FuncPtr: Send + 'static,
{
    pub(crate) fn new(
        name: String,
        queue_sender: crossbeam::channel::Sender<Box<TaskDesc<FuncPtr>>>,
        queue_receiver: crossbeam::channel::Receiver<Box<TaskDesc<FuncPtr>>>,
        linked_queue: Option<Arc<dyn TaskQueue<FuncPtr>>>,
        metrics_receiver: MetricsHandle,
    ) -> Self {
        let pull_counter =
            metrics_receiver.sink().register_counter(&format!("{}_queue.pulls", name).into());
        let post_counter =
            metrics_receiver.sink().register_counter(&format!("{}_queue.posts", name).into());

        Self {
            name,
            queue_sender,
            queue_receiver,
            pull_counter,
            post_counter,
            is_overloaded: Arc::new(AtomicBool::new(false)),
            linked_queue,
        }
    }
}

/*
===================================================================================================
    Session Implementation
===================================================================================================
*/

/// Simplex session implementation
pub(crate) struct SessionImpl {
    /// Atomic flag to indicate that all processing threads should be stopped
    stop_flag: Arc<AtomicBool>,
    /// Indicates database should be destroyed on stop
    destroy_db_flag: Arc<AtomicBool>,
    /// Atomic flag: main_loop should begin active FSM processing.
    /// Set by `start(seqno)`. The overlay is created at `create()` time and
    /// warms up while main_loop polls this flag, so peers are connected
    /// before the first_block_timeout clock starts ticking.
    start_flag: Arc<AtomicBool>,
    /// Initial block seqno, provided by `start(seqno)`.
    /// Read by main_loop after start_flag is set, before SessionDescription
    /// creation.
    deferred_initial_seqno: Arc<AtomicU32>,
    /// Atomic flag to indicate main processing thread has stopped
    main_processing_thread_stopped: Arc<AtomicBool>,
    /// Atomic flag to indicate callbacks processing thread has stopped
    callbacks_processing_thread_stopped: Arc<AtomicBool>,
    /// Atomic flag to indicate at least one internal thread panicked
    panicked_flag: Arc<AtomicBool>,
    /// Task queue for main thread tasks processing
    #[allow(dead_code)]
    main_task_queue: TaskQueuePtr,
    /// Task queue for session callbacks processing
    _callbacks_task_queue: CallbackTaskQueuePtr,
    /// Session identifier
    session_id: SessionId,
    /// Activity node for session lifetime tracking
    _activity_node: ActivityNodePtr,
    /// Receiver listener (must be kept alive while receiver is active)
    /// Note: Receiver itself is created and owned by main_loop (via SessionProcessor)
    _receiver_listener: Arc<dyn ReceiverListener + Send + Sync>,
}

impl ConsensusSession for SessionImpl {
    fn start(&self, initial_block_seqno: u32) {
        log::info!(
            "SimplexSession {}: start(seqno={}) called — storing seqno and unblocking main loop",
            self.session_id.to_hex_string(),
            initial_block_seqno
        );
        self.deferred_initial_seqno.store(initial_block_seqno, Ordering::Release);
        self.start_flag.store(true, Ordering::Release);
    }

    fn stop(&self) {
        self.stop_async();
        self.stop_impl(false);
    }

    fn stop_async(&self) {
        // Just set the stop flag without waiting for threads to finish.
        // NOTE: do NOT set destroy_db_flag here — stop preserves the DB
        // for potential restart/recovery (per Session trait contract).
        self.stop_flag.store(true, Ordering::Release);
    }

    fn destroy(&self) {
        self.destroy_db_flag.store(true, Ordering::SeqCst);
        self.stop();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl SimplexSession for SessionImpl {
    fn notify_mc_finalized(&self, mc_block_seqno: u32) {
        // Post closure to main queue for thread-safe update of last_mc_finalized_seqno
        // in SessionProcessor. This is used for shard empty block decisions.
        self.main_task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.set_mc_finalized_seqno(mc_block_seqno);
        }));
    }

    fn is_stopped(&self) -> bool {
        // Check if both processing threads have stopped.
        self.callbacks_processing_thread_stopped.load(Ordering::Relaxed)
            && self.main_processing_thread_stopped.load(Ordering::Relaxed)
    }

    fn is_panicked(&self) -> bool {
        self.panicked_flag.load(Ordering::Relaxed)
    }
}

impl fmt::Display for SessionImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SimplexSession({:x?})", self.session_id)
    }
}

impl Drop for SessionImpl {
    fn drop(&mut self) {
        log::info!("Dropping SimplexSession (session_id is {})", self.session_id.to_hex_string());

        self.stop_impl(false);
    }
}

impl SessionImpl {
    /*
        Session stopping
    */

    fn stop_impl(&self, destroy_db: bool) {
        if destroy_db {
            self.destroy_db_flag.store(true, Ordering::SeqCst);
        }

        self.stop_flag.store(true, Ordering::Release);

        loop {
            if self.callbacks_processing_thread_stopped.load(Ordering::Relaxed)
                && self.main_processing_thread_stopped.load(Ordering::Relaxed)
            {
                break;
            }

            log::info!(
                "...waiting for SimplexSession threads (session_id is {})",
                self.session_id.to_hex_string()
            );

            const CHECKING_INTERVAL: Duration = Duration::from_millis(300);
            thread::sleep(CHECKING_INTERVAL);
        }

        log::info!(
            "SimplexSession has been stopped (session_id is {})",
            self.session_id.to_hex_string()
        );
    }

    /*
        Main loop & session callbacks processing loop
    */

    #[allow(clippy::too_many_arguments)]
    fn main_loop(
        should_stop_flag: Arc<AtomicBool>,
        is_stopped_flag: Arc<AtomicBool>,
        destroy_db_flag: Arc<AtomicBool>,
        start_flag: Arc<AtomicBool>,
        deferred_initial_seqno: Arc<AtomicU32>,
        panicked_flag: Arc<AtomicBool>,
        task_queue: TaskQueuePtr,
        callbacks_task_queue: CallbackTaskQueuePtr,
        options: SessionOptions,
        session_id: SessionId,
        shard: ShardIdent,
        ids: Vec<SessionNode>,
        local_key: PrivateKey,
        listener: SessionListenerPtr,
        overlay_manager: ConsensusOverlayManagerPtr,
        receiver_listener: ReceiverListenerPtr,
        max_candidate_size: usize,
        max_candidate_query_answer_size: u64,
        db_path: String,
        session_activity_node: ActivityNodePtr,
        session_creation_time: SystemTime,
        metrics_receiver: MetricsHandle,
        init_result_sender: Sender<Result<()>>,
    ) {
        log::info!(
            "SimplexSession main loop is started (session_id is {}); \
            session thread creation time is {:.3}ms",
            session_id.to_hex_string(),
            get_elapsed_time(&session_creation_time).as_secs_f64() * 1000.0,
        );

        // Signal thread start based on wait_for_db_init option:
        // - If false: send Ok(()) now (non-blocking for caller)
        // - If true: wait until full initialization completes
        let init_signaled = Cell::new(false);
        if !options.wait_for_db_init {
            if init_result_sender.send(Ok(())).is_err() {
                log::warn!(
                    "SimplexSession {} main loop: failed to send init result (receiver dropped)",
                    session_id.to_hex_string()
                );
                is_stopped_flag.store(true, Ordering::Release);
                return;
            }
            init_signaled.set(true);
        }

        // Configure metrics
        let loop_counter =
            metrics_receiver.sink().register_counter(&"simplex_main_loop_iterations".into());
        let loop_overloads_counter =
            metrics_receiver.sink().register_counter(&"simplex_main_loop_overloads".into());

        // Track startup errors (passed to SessionProcessor for unified error tracking)
        let startup_errors = Cell::new(0u32);

        // Single helper for all fatal startup failures during bootstrap/init phase.
        // - increments startup_errors (later passed into SessionProcessor)
        // - sends init error to creator only if init wasn't signaled yet (wait_for_db_init=true)
        // - marks thread stopped and returns from main_loop via explicit `return` at call sites
        let fail_startup = |err: Error, ctx: &str| {
            log::error!("Session {} {}: {:?}", session_id.to_hex_string(), ctx, err);
            startup_errors.set(startup_errors.get().saturating_add(1));
            if !init_signaled.get() {
                let _ = init_result_sender.send(Err(err));
            }
            is_stopped_flag.store(true, Ordering::Release);
        };

        // Phase 4: Open database (cancellable via stop_flag)
        let storage_id = session_id.to_hex_string();
        let db = match crate::database::SimplexDb::open(&db_path, &storage_id) {
            Ok(db) => db,
            Err(err) => {
                fail_startup(err, "failed to open SimplexDb");
                return;
            }
        };

        // Check if we should stop before loading bootstrap
        if should_stop_flag.load(Ordering::Relaxed) {
            log::info!("Session {} stopping before bootstrap load", session_id.to_hex_string());
            if !init_signaled.get() {
                let _ =
                    init_result_sender.send(Err(error!("Session stopped before bootstrap load")));
            }
            is_stopped_flag.store(true, Ordering::Release);
            return;
        }

        // Phase 4: Create receiver and register overlay BEFORE loading bootstrap.
        // This ensures the restarted node is in the overlay map as soon as possible so it
        // receives broadcasts (candidates/votes) during bootstrap load; otherwise messages
        // are dropped to the zombie client and we can stall finalization (only 3/5 nodes
        // have the candidate).
        let health_counters = Arc::new(crate::receiver::ReceiverHealthCounters::new());
        let receiver = match crate::receiver::ReceiverWrapper::create(
            session_id.clone(),
            &shard,
            max_candidate_size,
            max_candidate_query_answer_size,
            options.proto_version,
            &ids,
            &local_key,
            overlay_manager.clone(),
            receiver_listener,
            options.standstill_timeout,
            panicked_flag.clone(),
            options.use_quic,
            health_counters.clone(),
        ) {
            Ok(r) => r,
            Err(err) => {
                fail_startup(err, "failed to create receiver");
                return;
            }
        };

        // Phase 4: Load bootstrap (cancellable via stop_flag)
        let bootstrap = match db
            .load_bootstrap_cancellable(&should_stop_flag, Duration::from_millis(100))
        {
            Ok(boot) => boot,
            Err(err) => {
                // Check if this was a cancellation
                if should_stop_flag.load(Ordering::Relaxed) {
                    log::info!("Session {} bootstrap load cancelled", session_id.to_hex_string());
                    if !init_signaled.get() {
                        let _ = init_result_sender
                            .send(Err(error!("Session bootstrap load cancelled")));
                    }
                } else {
                    fail_startup(err, "failed to load bootstrap");
                }
                return;
            }
        };

        // Phase 4: Check if this is a fresh start
        let is_fresh_start = bootstrap.is_empty();

        log::info!(
            "Session {} bootstrap loaded: fresh_start={}, finalized_blocks={}, candidate_infos={}, notar_certs={}",
            session_id.to_hex_string(),
            is_fresh_start,
            bootstrap.finalized_blocks.len(),
            bootstrap.candidate_infos.len(),
            bootstrap.notar_certs.len(),
        );

        // Signal init complete before the start gate — the overlay and DB are
        // fully ready.  The caller (create()) can return and later call
        // start(seqno) to unblock the FSM.
        if !init_signaled.get() {
            if init_result_sender.send(Ok(())).is_err() {
                log::warn!(
                    "SimplexSession {} main loop: failed to send init result (receiver dropped)",
                    session_id.to_hex_string()
                );
            }
            init_signaled.set(true);
        }

        // Wait for start(seqno) before creating SessionDescription.
        // The overlay is already registered and warming up peer connections
        // while we poll here, so by the time start() is called the overlay
        // should have established connectivity -- preventing premature
        // first_block_timeout skips that occur when the FSM starts before
        // any peers are reachable.
        if !start_flag.load(Ordering::Acquire) {
            log::info!(
                "SimplexSession {} waiting for start(seqno) signal (overlay warming up)...",
                session_id.to_hex_string()
            );
            while !start_flag.load(Ordering::Acquire) {
                if should_stop_flag.load(Ordering::Relaxed) {
                    log::info!(
                        "SimplexSession {} stopped while waiting for start()",
                        session_id.to_hex_string()
                    );
                    is_stopped_flag.store(true, Ordering::Release);
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        let initial_block_seqno = deferred_initial_seqno.load(Ordering::Acquire);
        log::info!(
            "SimplexSession {} start(seqno={}) received, creating SessionDescription",
            session_id.to_hex_string(),
            initial_block_seqno
        );

        // Phase 4a: Create session description (immutable session configuration)
        let description = match SessionDescription::new(
            &options,
            session_id.clone(),
            initial_block_seqno,
            &ids,
            local_key,
            &shard,
            session_creation_time,
            Some(metrics_receiver.clone()),
        ) {
            Ok(d) => Arc::new(d),
            Err(err) => {
                fail_startup(err, "failed to create SessionDescription");
                return;
            }
        };

        // Phase 4b: Create session processor (bootstrap state applied separately via recovery)
        // Clone description Arc before passing to processor (needed for recovery processor)
        let description_for_recovery = description.clone();
        let mut processor = match SessionProcessor::new(
            description,
            listener,
            task_queue.clone(),
            callbacks_task_queue.clone(),
            overlay_manager,
            receiver,
            should_stop_flag.clone(),
            db,
            startup_errors.get(),
            health_counters,
        ) {
            Ok(p) => p,
            Err(err) => {
                fail_startup(err, "failed to create SessionProcessor");
                return;
            }
        };

        // Phase 5: Apply bootstrap state via SessionStartupRecoveryProcessor
        // This replays votes, sets finalized boundary, applies local flags,
        // generates skip votes, and restores receiver cache.
        if !is_fresh_start {
            let recovery_options = SessionStartupRecoveryOptions {
                restart_recommit_strategy: options.restart_recommit_strategy,
                initial_block_seqno,
            };

            let recovery_processor = SessionStartupRecoveryProcessor::new(
                session_id.clone(),
                description_for_recovery,
                recovery_options,
                bootstrap,
            );

            // Capture count before apply_bootstrap consumes the processor
            let finalized_count = recovery_processor.finalized_block_count();

            if let Err(err) = recovery_processor.apply_bootstrap(&mut processor) {
                // Recovery failure is logged but not fatal - continue with potentially
                // inconsistent state rather than failing the entire session startup.
                log::error!(
                    "Session {} recovery failed (continuing anyway): {:?}",
                    session_id.to_hex_string(),
                    err
                );
                startup_errors.set(startup_errors.get().saturating_add(1));
            } else {
                log::info!(
                    "Session {} recovery complete: {} finalized blocks replayed",
                    session_id.to_hex_string(),
                    finalized_count
                );
            }
        }

        // Create metrics dumper for computed/derivative metrics
        let mut metrics_dumper = Self::create_metrics_dumper();

        // Main loop timing.
        //
        // IMPORTANT: Do not use `SystemTime::now()` here. All session timing must go
        // through `SessionDescription::get_time()` so tests can override time.
        let mut last_warn_dump_time = SystemTime::now(); // only for queue-latency warnings
        let mut next_metrics_dump_time = processor.get_description().get_time();
        let mut next_profiling_dump_time = next_metrics_dump_time;
        let mut next_health_check_time = next_metrics_dump_time;

        loop {
            {
                session_activity_node.tick();
                loop_counter.increment(1);

                // Check if the main loop should be stopped
                if should_stop_flag.load(Ordering::Relaxed) {
                    processor.stop(destroy_db_flag.load(Ordering::Relaxed));
                    break;
                }

                // Check overload flag
                if task_queue.is_overloaded() {
                    loop_overloads_counter.increment(1);
                }

                // Handle session event with timeout
                let now = processor.get_description().get_time();
                let timeout =
                    processor.get_next_awake_time().duration_since(now).unwrap_or_default();

                const MAX_TIMEOUT: Duration = Duration::from_millis(100);
                let timeout = cmp::min(timeout, MAX_TIMEOUT);

                let task = task_queue.pull_closure(timeout, &mut last_warn_dump_time);

                check_execution_time!(150_000);

                if let Some(task) = task {
                    check_execution_time!(100_000);
                    task(&mut processor);
                }

                // Do checks only when next awake time is reached
                if processor.get_next_awake_time() <= processor.get_description().get_time() {
                    check_execution_time!(20_000);

                    processor.reset_next_awake_time();
                    processor.check_all();
                }
            }

            check_execution_time!(50_000);

            // Dump metrics
            if processor.get_description().get_time() >= next_metrics_dump_time {
                check_execution_time!(50_000);

                metrics_dumper.update(processor.get_metrics_receiver());

                if log::log_enabled!(log::Level::Debug) {
                    let session_id_str = session_id.to_hex_string();
                    log::debug!("SimplexSession {} metrics:", &session_id_str);

                    metrics_dumper.dump(|string| {
                        log::debug!("{}{}", session_id_str, string);
                    });
                }

                next_metrics_dump_time = processor.get_description().get_time()
                    + Duration::from_millis(SESSION_METRICS_DUMP_PERIOD_MS);
            }

            // Dump profiling
            if processor.get_description().get_time() >= next_profiling_dump_time {
                check_execution_time!(50_000);

                if log::log_enabled!(target: LOG_TARGET_PROFILING, log::Level::Debug) {
                    let profiling_dump = consensus_common::profiling::Profiler::local_instance()
                        .with(|profiler| profiler.borrow().dump());

                    log::debug!(
                        target: LOG_TARGET_PROFILING,
                        "SimplexSession {} profiling: {}",
                        &session_id.to_hex_string(),
                        profiling_dump
                    );
                }

                next_profiling_dump_time = processor.get_description().get_time()
                    + Duration::from_millis(SESSION_PROFILING_DUMP_PERIOD_MS);
            }

            // Periodic health check dump
            // Logs brief status to INFO, full details to DEBUG
            if processor.get_description().get_time() >= next_health_check_time {
                check_execution_time!(50_000);

                processor.health_check_dump();

                next_health_check_time = processor.get_description().get_time()
                    + Duration::from_millis(SESSION_HEALTH_CHECK_PERIOD_MS);
            }
        }

        // Finishing routines
        task_queue.flush();

        log::info!(
            "SimplexSession main loop is finished (session_id is {})",
            session_id.to_hex_string()
        );

        is_stopped_flag.store(true, Ordering::Release);
    }

    fn callbacks_loop(
        should_stop_flag: Arc<AtomicBool>,
        is_stopped_flag: Arc<AtomicBool>,
        task_queue: CallbackTaskQueuePtr,
        session_id: SessionId,
        metrics_receiver: MetricsHandle,
    ) {
        log::info!(
            "SimplexSession callbacks processing loop is started (session_id is {})",
            session_id.to_hex_string()
        );

        let activity_node = consensus_common::ConsensusCommonFactory::create_activity_node(
            format!("SimplexCallbacks_{}", session_id.to_hex_string()),
        );

        // Configure metrics
        let loop_counter =
            metrics_receiver.sink().register_counter(&"simplex_callbacks_loop_iterations".into());
        let loop_overloads_counter =
            metrics_receiver.sink().register_counter(&"simplex_callbacks_loop_overloads".into());

        // Callbacks processing loop
        let mut last_warn_dump_time = SystemTime::now();

        loop {
            activity_node.tick();
            loop_counter.increment(1);

            // Check if the loop should be stopped
            if should_stop_flag.load(Ordering::Relaxed) {
                break;
            }

            // Check overload flag
            if task_queue.is_overloaded() {
                loop_overloads_counter.increment(1);
            }

            // Handle session callback event with timeout
            const MAX_TIMEOUT: Duration = Duration::from_millis(100);

            let task = task_queue.pull_closure(MAX_TIMEOUT, &mut last_warn_dump_time);

            if let Some(task) = task {
                check_execution_time!(100_000);
                task();
            }
        }

        // Finishing routines
        task_queue.flush();

        log::info!(
            "SimplexSession callbacks processing loop is finished (session_id is {})",
            session_id.to_hex_string()
        );

        is_stopped_flag.store(true, Ordering::Release);
    }

    /*
        Metrics configuration
    */

    /// Create metrics dumper for Simplex session
    ///
    /// Configures derivative metrics (rate of change), percentage metrics,
    /// and result status metrics similar to validator-session.
    fn create_metrics_dumper() -> MetricsDumper {
        let mut metrics_dumper = MetricsDumper::new();

        // Derivative metrics for loop counters (rate per second)
        metrics_dumper.add_derivative_metric("simplex_main_loop_iterations");
        metrics_dumper.add_derivative_metric("simplex_main_loop_overloads");
        metrics_dumper.add_derivative_metric("simplex_callbacks_loop_iterations");
        metrics_dumper.add_derivative_metric("simplex_callbacks_loop_overloads");

        // Percentage metrics for loop load
        add_compute_percentage_metric(
            &mut metrics_dumper,
            "simplex_main_loop_load",
            "simplex_main_loop_overloads",
            "simplex_main_loop_iterations",
            0.0,
        );
        add_compute_percentage_metric(
            &mut metrics_dumper,
            "simplex_callbacks_loop_load",
            "simplex_callbacks_loop_overloads",
            "simplex_callbacks_loop_iterations",
            0.0,
        );

        // Percentage metric for active nodes
        add_compute_percentage_metric(
            &mut metrics_dumper,
            "simplex_active_nodes_percent",
            "simplex_active_weight",
            "simplex_total_weight",
            0.0,
        );

        // Result status counters (total/success/failure metrics)
        add_compute_result_metric(&mut metrics_dumper, "simplex_validates");
        add_compute_result_metric(&mut metrics_dumper, "simplex_collates");
        add_compute_result_metric(&mut metrics_dumper, "simplex_collates_expire");
        add_compute_result_metric(&mut metrics_dumper, "simplex_collates_precollated");
        add_compute_result_metric(&mut metrics_dumper, "simplex_commits");

        // Derivative metrics for result counters
        metrics_dumper.add_derivative_metric("simplex_validates.total");
        metrics_dumper.add_derivative_metric("simplex_validates.success");
        metrics_dumper.add_derivative_metric("simplex_validates.failure");
        metrics_dumper.add_derivative_metric("simplex_collates.total");
        metrics_dumper.add_derivative_metric("simplex_collates.success");
        metrics_dumper.add_derivative_metric("simplex_collates.failure");
        metrics_dumper.add_derivative_metric("simplex_commits.total");
        metrics_dumper.add_derivative_metric("simplex_commits.success");
        metrics_dumper.add_derivative_metric("simplex_commits.failure");

        // Derivative metrics for processing counters
        metrics_dumper.add_derivative_metric("simplex_check_all_calls");
        metrics_dumper.add_derivative_metric("simplex_process_events_calls");

        // Relative metrics
        add_compute_relative_metric(
            &mut metrics_dumper,
            "simplex_iterations_per_check_all",
            "simplex_main_loop_iterations",
            "simplex_check_all_calls",
            0.0,
        );

        // Queue metrics (names from TaskQueueImpl::new with format!("{}_queue.X", name))
        metrics_dumper.add_derivative_metric("processing_queue.pulls");
        metrics_dumper.add_derivative_metric("processing_queue.posts");
        metrics_dumper.add_derivative_metric("callbacks_queue.pulls");
        metrics_dumper.add_derivative_metric("callbacks_queue.posts");

        metrics_dumper.add_compute_handler(
            "processing_queue",
            consensus_common::utils::compute_queue_size_counter,
        );
        metrics_dumper.add_compute_handler(
            "callbacks_queue",
            consensus_common::utils::compute_queue_size_counter,
        );

        // Precollation metrics
        metrics_dumper.add_derivative_metric("simplex_precollation_requests");
        metrics_dumper.add_derivative_metric("simplex_precollation_results");
        metrics_dumper.add_compute_handler(
            "simplex_precollation_pending",
            |_basic_key: &str, metrics: &BTreeMap<String, consensus_common::utils::Metric>| {
                consensus_common::utils::compute_diff_counter(
                    "simplex_precollation",
                    metrics,
                    "_requests",
                    "_results",
                )
            },
        );

        metrics_dumper.add_derivative_metric("simplex_errors");
        metrics_dumper.add_derivative_metric("simplex_misbehavior");
        metrics_dumper.add_derivative_metric("simplex_batch_commits");

        metrics_dumper.add_derivative_metric("simplex_last_finalized_slot");
        metrics_dumper.add_derivative_metric("simplex_first_non_finalized_slot");
        metrics_dumper.add_derivative_metric("simplex_first_non_progressed_slot");

        metrics_dumper.add_derivative_metric("simplex_skip_total");
        metrics_dumper.add_derivative_metric("simplex_votes_in_notarize");
        metrics_dumper.add_derivative_metric("simplex_votes_in_finalize");
        metrics_dumper.add_derivative_metric("simplex_votes_in_skip");
        metrics_dumper.add_derivative_metric("simplex_votes_out_notarize");
        metrics_dumper.add_derivative_metric("simplex_votes_out_finalize");
        metrics_dumper.add_derivative_metric("simplex_votes_out_skip");
        metrics_dumper.add_derivative_metric("simplex_certs_in");
        metrics_dumper.add_derivative_metric("simplex_certs_relayed");
        metrics_dumper.add_derivative_metric("simplex_cert_conflict");
        metrics_dumper.add_derivative_metric("simplex_cert_verify_fail");
        metrics_dumper.add_derivative_metric("simplex_validation_reject");
        metrics_dumper.add_derivative_metric("simplex_validation_late_callback");
        metrics_dumper.add_derivative_metric("simplex_health_warnings");

        metrics_dumper
    }

    /*
        Task queue creation
    */

    pub(crate) fn create_task_queue(
        name: impl ToString,
        linked_queue: Option<TaskQueuePtr>,
        metrics_receiver: MetricsHandle,
    ) -> TaskQueuePtr {
        type ChannelPair = (
            crossbeam::channel::Sender<Box<TaskDesc<TaskPtr>>>,
            crossbeam::channel::Receiver<Box<TaskDesc<TaskPtr>>>,
        );

        let (queue_sender, queue_receiver): ChannelPair = crossbeam::channel::unbounded();
        let task_queue: TaskQueuePtr = Arc::new(TaskQueueImpl::<TaskPtr>::new(
            name.to_string(),
            queue_sender,
            queue_receiver,
            linked_queue,
            metrics_receiver,
        ));

        task_queue
    }

    pub(crate) fn create_callback_task_queue(
        metrics_receiver: MetricsHandle,
    ) -> CallbackTaskQueuePtr {
        type ChannelPair = (
            crossbeam::channel::Sender<Box<TaskDesc<CallbackTaskPtr>>>,
            crossbeam::channel::Receiver<Box<TaskDesc<CallbackTaskPtr>>>,
        );

        let (queue_sender, queue_receiver): ChannelPair = crossbeam::channel::unbounded();
        let task_queue: CallbackTaskQueuePtr = Arc::new(TaskQueueImpl::<CallbackTaskPtr>::new(
            "callbacks".to_string(),
            queue_sender,
            queue_receiver,
            None,
            metrics_receiver,
        ));

        task_queue
    }

    /*
        Session creation
    */

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        options: &SessionOptions,
        session_id: &SessionId,
        shard: &ShardIdent,
        ids: Vec<SessionNode>,
        local_key: &PrivateKey,
        db_path: String,
        overlay_manager: ConsensusOverlayManagerPtr,
        listener: SessionListenerPtr,
    ) -> Result<SessionPtr> {
        log::info!(
            "Creating SimplexSession (session_id is {}, shard={}, nodes_count={}, db_path={})",
            session_id.to_hex_string(),
            shard,
            ids.len(),
            db_path
        );

        let session_creation_time = SystemTime::now();

        // Ensure panics in SX threads are logged as FATAL with backtrace.
        crate::utils::install_simplex_panic_hook_once();

        // Create metrics receiver
        let metrics_receiver = MetricsHandle::new(Some(Duration::from_secs(30)));

        // Create task queues
        let main_task_queue = Self::create_task_queue("processing", None, metrics_receiver.clone());
        let callbacks_task_queue = Self::create_callback_task_queue(metrics_receiver.clone());

        // Create activity node
        let session_activity_node = consensus_common::ConsensusCommonFactory::create_activity_node(
            format!("SimplexSession_{}", session_id.to_hex_string()),
        );

        // Create thread synchronization flags
        let stop_flag = Arc::new(AtomicBool::new(false));
        let destroy_db_flag = Arc::new(AtomicBool::new(false));
        let start_flag = Arc::new(AtomicBool::new(false));
        let deferred_initial_seqno = Arc::new(AtomicU32::new(0));
        let main_processing_thread_stopped = Arc::new(AtomicBool::new(false));
        let callbacks_processing_thread_stopped = Arc::new(AtomicBool::new(false));
        let panicked_flag = Arc::new(AtomicBool::new(false));

        // Create receiver listener (posts callbacks to main task queue)
        // Note: Receiver itself is created in main_loop after bootstrap loading
        let receiver_listener: Arc<dyn ReceiverListener + Send + Sync> =
            ReceiverListenerImpl::create(main_task_queue.clone(), session_id.clone());
        let receiver_listener_weak: ReceiverListenerPtr = Arc::downgrade(&receiver_listener);

        // Compute max candidate size for receiver (local validation guard, +1KB slack)
        let max_candidate_size = options.max_block_size + options.max_collated_data_size + 1024;
        // Network response budget for requestCandidate queries (C++ PR #2195 parity: +1MB)
        let max_candidate_query_answer_size: u64 =
            (options.max_block_size + options.max_collated_data_size) as u64 + (1 << 20);

        // Create session (receiver is created in main_loop after bootstrap loading)
        let session = SessionImpl {
            stop_flag: stop_flag.clone(),
            destroy_db_flag: destroy_db_flag.clone(),
            start_flag: start_flag.clone(),
            deferred_initial_seqno: deferred_initial_seqno.clone(),
            main_processing_thread_stopped: main_processing_thread_stopped.clone(),
            callbacks_processing_thread_stopped: callbacks_processing_thread_stopped.clone(),
            panicked_flag: panicked_flag.clone(),
            main_task_queue: main_task_queue.clone(),
            _callbacks_task_queue: callbacks_task_queue.clone(),
            session_id: session_id.clone(),
            _activity_node: session_activity_node.clone(),
            _receiver_listener: receiver_listener,
        };

        let session = Arc::new(session);

        // Clone variables for threads
        let stop_flag_for_main_loop = stop_flag.clone();
        let stop_flag_for_callbacks_loop = stop_flag.clone();
        let callbacks_task_queue_for_callbacks_loop = callbacks_task_queue.clone();
        let local_key_clone = local_key.clone();
        let session_id_clone = session_id.clone();
        let options_clone = *options;
        let shard_clone = shard.clone();
        let panicked_flag_for_main_loop = panicked_flag.clone();
        let panicked_flag_for_callbacks_loop = panicked_flag.clone();

        // Create channel for main loop initialization result
        let (init_result_sender, init_result_receiver) = bounded::<Result<()>>(1);

        // Create main processing thread
        let metrics_receiver_clone = metrics_receiver.clone();
        let _main_processing_thread = thread::Builder::new()
            .name(format!("{}:{}", MAIN_LOOP_NAME, session_id.to_hex_string()))
            .spawn(move || {
                crate::utils::install_simplex_panic_hook_once();

                let stop_flag_for_panic = stop_flag_for_main_loop.clone();
                let stopped_flag_for_panic = main_processing_thread_stopped.clone();
                let panicked_flag_for_panic = panicked_flag_for_main_loop.clone();

                let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    SessionImpl::main_loop(
                        stop_flag_for_main_loop,
                        main_processing_thread_stopped,
                        destroy_db_flag,
                        start_flag,
                        deferred_initial_seqno,
                        panicked_flag_for_main_loop,
                        main_task_queue,
                        callbacks_task_queue,
                        options_clone,
                        session_id_clone,
                        shard_clone,
                        ids,
                        local_key_clone,
                        listener,
                        overlay_manager,
                        receiver_listener_weak,
                        max_candidate_size,
                        max_candidate_query_answer_size,
                        db_path,
                        session_activity_node,
                        session_creation_time,
                        metrics_receiver_clone,
                        init_result_sender,
                    );
                }));

                if let Err(panic_payload) = result {
                    log::error!(
                        "FATAL PANIC: caught panic in {}: payload=\"{}\"; forcing session stop",
                        thread::current().name().unwrap_or("<unnamed>"),
                        crate::utils::panic_payload_to_string(panic_payload.as_ref())
                    );
                    panicked_flag_for_panic.store(true, Ordering::Release);
                    stop_flag_for_panic.store(true, Ordering::Release);
                }

                // Always mark thread as stopped (normal exit or panic).
                stopped_flag_for_panic.store(true, Ordering::Release);
            });

        // Wait for main loop initialization result
        match init_result_receiver.recv() {
            Ok(Ok(())) => {
                log::debug!(
                    "SimplexSession {}: main loop initialized successfully",
                    session_id.to_hex_string()
                );
            }
            Ok(Err(err)) => {
                log::error!(
                    "SimplexSession {}: main loop initialization failed: {:?}",
                    session_id.to_hex_string(),
                    err
                );
                // Stop the session and return error
                session.stop_flag.store(true, Ordering::Release);
                return Err(err);
            }
            Err(_) => {
                // Channel disconnected - thread panicked or exited unexpectedly
                let err = error!(
                    "SimplexSession {}: main loop thread terminated unexpectedly",
                    session_id.to_hex_string()
                );
                log::error!("{}", err);
                session.stop_flag.store(true, Ordering::Release);
                return Err(err);
            }
        }

        // Conditionally start callbacks thread based on use_callback_thread option
        if options.use_callback_thread {
            log::info!(
                "SimplexSession {}: Starting callback processing thread (use_callback_thread=true)",
                session_id.to_hex_string()
            );

            let session_id_clone = session_id.clone();
            let _callbacks_processing_thread = thread::Builder::new()
                .name(format!("{}:{}", CALLBACKS_LOOP_NAME, session_id.to_hex_string()))
                .spawn(move || {
                    crate::utils::install_simplex_panic_hook_once();

                    let stop_flag_for_panic = stop_flag_for_callbacks_loop.clone();
                    let stopped_flag_for_panic = callbacks_processing_thread_stopped.clone();
                    let panicked_flag_for_panic = panicked_flag_for_callbacks_loop.clone();

                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        SessionImpl::callbacks_loop(
                            stop_flag_for_callbacks_loop,
                            callbacks_processing_thread_stopped,
                            callbacks_task_queue_for_callbacks_loop,
                            session_id_clone,
                            metrics_receiver,
                        );
                    }));

                    if let Err(panic_payload) = result {
                        log::error!(
                            "FATAL PANIC: caught panic in {}: payload=\"{}\"; forcing session stop",
                            thread::current().name().unwrap_or("<unnamed>"),
                            crate::utils::panic_payload_to_string(panic_payload.as_ref())
                        );
                        panicked_flag_for_panic.store(true, Ordering::Release);
                        stop_flag_for_panic.store(true, Ordering::Release);
                    }

                    // Always mark thread as stopped (normal exit or panic).
                    stopped_flag_for_panic.store(true, Ordering::Release);
                });
        } else {
            log::info!(
                "SimplexSession {}: Callback processing thread is disabled (use_callback_thread=false), \
                callbacks will execute synchronously in main session thread",
                session_id.to_hex_string()
            );

            // Set the callback thread as already stopped since we're not starting it
            callbacks_processing_thread_stopped.store(true, Ordering::Release);
        }

        Ok(session)
    }

    /// Create session with log replay
    pub fn create_replay(
        _options: &SessionOptions,
        _log_replay_options: &LogReplayOptions,
        _session_listener: SessionListenerPtr,
        _replay_listener: SessionReplayListenerPtr,
    ) -> Result<SessionPtr> {
        unimplemented!("SessionImpl::create_replay: log replay not implemented yet")
    }
}
