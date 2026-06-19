/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![allow(clippy::too_many_arguments)]

use crate::{
    check_execution_time, instrument,
    profiling::{InstanceCounter, ResultStatusCounter},
    received_block::ReceivedBlock,
    serialize_tl_boxed_object, ton,
    utils::{self, get_elapsed_time, MetricsHandle},
    ActivityNodePtr, BlockHash, BlockHeight, BlockPayloadPtr, CatchainFactory, CatchainNode,
    CatchainOverlayListener, CatchainOverlayLogReplayListener, CatchainOverlayManagerPtr,
    CatchainOverlayPtr, DatabasePtr, Options, PrivateKey, PrivateOverlayShortId, PublicKeyHash,
    QueryResponseCallback, RawBuffer, ReceivedBlockPtr, Receiver, ReceiverListenerPtr, ReceiverPtr,
    ReceiverSourcePtr, SessionId,
};
use adnl::OverlayUtils;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rand::Rng;
use std::{
    collections::{HashMap, VecDeque},
    convert::TryInto,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{deserialize_boxed, deserialize_boxed_with_suffix, IntoBoxed};
use ton_block::{base64_encode_url_safe, error, fail, Error, Result, UInt256};

//TODO: wait for database thread to finish in main loop

/*
    Constants
*/

const MAX_UNSAFE_INITIAL_SYNC_COMPLETE_TIME_SECS: u64 = 300; //max time to finish synchronization (unsafe mode)
const MAX_SAFE_INITIAL_SYNC_COMPLETE_TIME_SECS: u64 = 5; //max time to finish synchronization (safe mode)
const INFINITE_INITIAL_SYNC_COMPLETE_TIME_SECS: u64 = 10 * 365 * 24 * 3600; //inifinite time to finish synchronization

const RECEIVER_METRICS_DUMP_PERIOD_MS: u64 = 20000; //time for catchain metrics dump
const COMPLETION_HANDLERS_CHECK_PERIOD: Duration = Duration::from_millis(5000); //period of completion handlers checking
const COMPLETION_HANDLERS_MAX_WAIT_PERIOD: Duration = Duration::from_millis(60000); //max delay for completion handler execution

const RECEIVER_WARN_PROCESSING_LATENCY: Duration = Duration::from_millis(3000); //max processing latency
const RECEIVER_LATENCY_WARN_DUMP_PERIOD: Duration = Duration::from_millis(2000); //latency warning dump period
const RECEIVER_PROCESSING_PERIOD_MS: u64 = 1000; //receiver processing period

lazy_static::lazy_static! {
  static ref ZERO_HASH : BlockHash = BlockHash::default(); //default block hash to save the root in the DB
}

/*
    ReceiverTaskQueues
*/

struct TaskDesc<F: ?Sized> {
    task: Box<F>,                         //closure for execution
    creation_time: std::time::SystemTime, //time of task creation
}

struct ReceiverTaskQueues {
    processing_task_receiver: CrossbeamReceiver<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>, //receiver for processing thread tasks
    processing_task_sender: CrossbeamSender<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>, //sender for processing thread tasks
    db_task_receiver: CrossbeamReceiver<TaskDesc<dyn FnOnce() + Send>>, //receiver for DB thread tasks
    db_task_sender: CrossbeamSender<TaskDesc<dyn FnOnce() + Send>>,     //sender for DB thread tasks
    main_thread_post_counter: metrics::Counter, //counter for main queue posts
    main_thread_pull_counter: metrics::Counter, //counter for main queue pull
    db_thread_post_counter: metrics::Counter,   //counter for DB queue posts
    db_thread_pull_counter: metrics::Counter,   //counter for DB queue pull
}

impl ReceiverTaskQueues {
    /*
        Tasks posting
    */

    fn post_processing_closure(&self, job: Box<dyn FnOnce(&mut ReceiverImpl) + Send>) {
        let desc = TaskDesc { task: job, creation_time: SystemTime::now() };
        let _ = self.processing_task_sender.send(desc);
        self.main_thread_post_counter.increment(1);
    }

    fn post_database_closure(&self, job: Box<dyn FnOnce() + Send>) {
        let desc = TaskDesc { task: job, creation_time: SystemTime::now() };
        let _ = self.db_task_sender.send(desc);
        self.db_thread_post_counter.increment(1);
    }

    /*
        Constructor
    */

    fn new(metrics_receiver: MetricsHandle) -> Self {
        let (processing_task_sender, processing_task_receiver) =
            crossbeam_channel::unbounded::<TaskDesc<dyn FnOnce(&mut ReceiverImpl) + Send>>();
        let (db_task_sender, db_task_receiver) =
            crossbeam_channel::unbounded::<TaskDesc<dyn FnOnce() + Send>>();

        let main_thread_post_counter =
            metrics_receiver.sink().register_counter(&"receiver_main_queue.posts".into());
        let main_thread_pull_counter =
            metrics_receiver.sink().register_counter(&"receiver_main_queue.pulls".into());
        let db_thread_post_counter =
            metrics_receiver.sink().register_counter(&"receiver_db_queue.posts".into());
        let db_thread_pull_counter =
            metrics_receiver.sink().register_counter(&"receiver_db_queue.pulls".into());

        Self {
            processing_task_receiver,
            processing_task_sender,
            db_task_receiver,
            db_task_sender,
            main_thread_post_counter,
            main_thread_pull_counter,
            db_thread_post_counter,
            db_thread_pull_counter,
        }
    }
}

/*
    ReceiverThreads
*/

struct ReceiverThreadDesc {
    thread_prefix: String,                 //thread prefix
    stopped: Arc<AtomicBool>,              //stop flag
    thread_handle: thread::JoinHandle<()>, //thread handle
    _activity_node: ActivityNodePtr,       //activity node for tracking hanged threads
}

struct ReceiverThreads {
    threads: Vec<ReceiverThreadDesc>, //receiver threads
    stop_flag: Arc<AtomicBool>,       //stop flag for threads
    session_id: SessionId,            //session ID
}

impl ReceiverThreads {
    fn new(session_id: SessionId) -> Self {
        Self { threads: Vec::new(), stop_flag: Arc::new(AtomicBool::new(false)), session_id }
    }

    fn start_thread(
        &mut self,
        thread_prefix: String,
        thread_fn: Box<dyn FnOnce(Arc<AtomicBool>, ActivityNodePtr) + Send>,
    ) -> Result<Arc<AtomicBool>> {
        //create DB thread context
        let stop = self.stop_flag.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let session_id = self.session_id.to_hex_string();
        let activity_node = CatchainFactory::create_activity_node(
            format!("{}_{}", thread_prefix, session_id).to_string(),
        );
        let thread_prefix_clone = thread_prefix.clone();
        let stopped_clone = stopped.clone();
        let activity_node_clone = activity_node.clone();

        //start thread
        let handle = std::thread::Builder::new()
            .name(format!("{}:{}", thread_prefix, self.session_id.to_hex_string()))
            .spawn(move || {
                log::info!("{} thread started for session {}", thread_prefix, session_id);

                thread_fn(stop, activity_node);

                log::info!("{} thread exited for session {}", thread_prefix, session_id);

                //signal that thread is stopped

                stopped.store(true, Ordering::Relaxed);
            })?;

        //store thread handle for later joining

        self.threads.push(ReceiverThreadDesc {
            thread_prefix: thread_prefix_clone,
            stopped: stopped_clone.clone(),
            thread_handle: handle,
            _activity_node: activity_node_clone,
        });

        Ok(stopped_clone)
    }

    fn stop_threads(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);

        let all_stopped = self.threads.iter().all(|thread| thread.stopped.load(Ordering::Relaxed));

        if all_stopped {
            return;
        }

        let session_id = self.session_id.to_hex_string();

        log::info!("Stopping receiver for session {}", session_id);

        loop {
            let all_stopped =
                self.threads.iter().all(|thread| thread.stopped.load(Ordering::Relaxed));

            if all_stopped {
                break;
            }

            let threads_to_dump = self
                .threads
                .iter()
                .filter(|thread| !thread.stopped.load(Ordering::Relaxed))
                .map(|thread| thread.thread_prefix.clone())
                .collect::<Vec<_>>()
                .join(", ");

            log::info!(
                "...waiting for Receiver threads for session {}: {:?}",
                session_id,
                threads_to_dump
            );

            const CHECKING_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

            std::thread::sleep(CHECKING_INTERVAL);
        }

        log::info!("Stopped receiver for session {}", session_id);
    }

    fn remove_all_threads(&mut self) {
        log::info!("Removing all threads for session {}", self.session_id.to_hex_string());

        for thread in &mut self.threads.drain(..) {
            if let Err(err) = thread.thread_handle.join() {
                log::error!(
                    "Error joining thread {} for session {}: {:?}",
                    thread.thread_prefix,
                    self.session_id.to_hex_string(),
                    err
                );
            }
        }

        log::info!("Removed all threads for session {}", self.session_id.to_hex_string());
    }
}

/*
    ReceiverWrapper
*/

/// Parameters which copied from ReceiverImpl to ReceiverWrapper
struct ReceiverStartupResult {
    out_broadcasts_bytes: metrics::Counter, //outgoing broadcasts traffic
    out_bytes: metrics::Counter,            //outgoing traffic
    local_adnl_id: PublicKeyHash,           //local ADNL ID
    overlay: CatchainOverlayPtr,            //overlay
}

pub(crate) struct ReceiverWrapper {
    receiver_threads: ReceiverThreads,    //receiver threads management
    session_id: SessionId,                //session ID
    task_queues: Arc<ReceiverTaskQueues>, //task queues
    _metrics_receiver: MetricsHandle,     //metrics receiver
    out_broadcasts_bytes: metrics::Counter, //outgoing broadcasts traffic
    out_bytes: metrics::Counter,          //outgoing traffic
    local_adnl_id: PublicKeyHash,         //local ADNL ID
    local_id: PublicKeyHash,              //this node's public key hash (computed from local_key)
    overlay: CatchainOverlayPtr,          //overlay
}

impl Drop for ReceiverWrapper {
    fn drop(&mut self) {
        log::info!("Dropping ReceiverWrapper for session {}", self.session_id.to_hex_string());

        // Stop receiver
        self.stop();

        // Remove all threads and join them
        self.receiver_threads.remove_all_threads();

        log::info!("Dropped ReceiverWrapper for session {}", self.session_id.to_hex_string());
    }
}

impl Receiver for ReceiverWrapper {
    /// Send broadcast
    fn send_broadcast(&self, payload: BlockPayloadPtr) {
        check_execution_time!(20000);
        instrument!();

        // Update metrics counters
        self.out_broadcasts_bytes.increment(payload.data().len() as u64);
        self.out_bytes.increment(payload.data().len() as u64);

        // Send broadcast through overlay directly
        self.overlay.send_broadcast_fec_ex(&self.local_adnl_id, &self.local_id, payload, None);
    }

    /// Send query via RLDP

    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.send_query_via_rldp(
                dst_adnl_id,
                name,
                response_callback,
                timeout,
                query,
                max_answer_size,
                v2,
            );
        }));
    }

    /// Notify about blame processing state
    fn blame_processed(&self, source_id: usize) {
        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.blame_processed(source_id);
        }));
    }

    /// Adding new block
    fn add_block(&self, payload: BlockPayloadPtr, deps: Vec<BlockHash>) {
        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.add_block(payload, deps);
        }));
    }

    /// Adding new fork (for debugging)
    fn debug_add_fork(&self, payload: BlockPayloadPtr, height: BlockHeight, deps: Vec<BlockHash>) {
        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.debug_add_fork(payload, height, deps);
        }));
    }

    /// Stop receiver
    fn stop(&self) {
        self.receiver_threads.stop_threads();
    }

    /// Destroy DB
    fn destroy_db(&self) {
        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.destroy_db();
        }));
    }
}

impl ReceiverWrapper {
    /*
        Threads creation functions
    */

    /// Start processing thread
    fn start_processing_thread(
        receiver_threads: &mut ReceiverThreads,
        thread_fn: Box<dyn FnOnce(Arc<AtomicBool>, ActivityNodePtr) + Send>,
    ) -> Result<Arc<AtomicBool>> {
        receiver_threads.start_thread("Receiver".to_string(), thread_fn)
    }
    /*
        DB thread
    */

    /// Start DB thread
    fn start_db_thread(
        receiver_threads: &mut ReceiverThreads,
        task_queues: &Arc<ReceiverTaskQueues>,
        metrics_receiver: &MetricsHandle,
    ) -> Result<Arc<AtomicBool>> {
        let rx = task_queues.db_task_receiver.clone();
        let pull_counter = task_queues.db_thread_pull_counter.clone();
        let overloaded_flag = Arc::new(AtomicBool::new(false));
        let overloaded_counter =
            metrics_receiver.sink().register_counter(&"receiver_db_overloaded_counter".into());
        let loop_counter =
            metrics_receiver.sink().register_counter(&"receiver_db_loop_iterations".into());

        receiver_threads.start_thread(
            "ReceiverDB".to_string(),
            Box::new(move |stop, activity_node| {
                Self::thread_loop(
                    stop,
                    pull_counter,
                    overloaded_flag,
                    overloaded_counter,
                    loop_counter,
                    activity_node,
                    rx,
                    None::<fn() -> Duration>,
                    move |task: Box<dyn FnOnce() + Send>| {
                        task();
                    },
                    None::<fn()>,
                );
            }),
        )
    }

    /*
        Thread funcs
    */

    fn thread_loop<T: ?Sized, P, P1, P2>(
        stop_flag: Arc<AtomicBool>,
        thread_pull_counter: metrics::Counter,
        overloaded_flag: Arc<AtomicBool>,
        overloaded_counter: metrics::Counter,
        loop_counter: metrics::Counter,
        activity_node: ActivityNodePtr,
        task_receiver: CrossbeamReceiver<TaskDesc<T>>,
        timeout_fn: Option<P1>,
        mut task_processor: P,
        mut idle: Option<P2>,
    ) where
        P: FnMut(Box<T>),
        P1: Fn() -> Duration,
        P2: FnMut(),
    {
        let mut is_overloaded = false;
        let mut last_latency_warn_dump_time = SystemTime::now();

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }

            loop_counter.increment(1);

            //tick activity node for tracking hanged threads

            activity_node.tick();

            //check if thread is overloaded

            overloaded_flag.store(is_overloaded, Ordering::SeqCst);

            if is_overloaded {
                overloaded_counter.increment(1);
            }

            //invoke task from a queue

            const TASK_RECEIVE_TIMEOUT: Duration = Duration::from_millis(200);

            let timeout = match &timeout_fn {
                Some(ref timeout_fn) => std::cmp::min(timeout_fn(), TASK_RECEIVE_TIMEOUT),
                None => TASK_RECEIVE_TIMEOUT,
            };

            is_overloaded = false;

            match task_receiver.recv_timeout(timeout) {
                Ok(task_desc) => {
                    instrument!();
                    check_execution_time!(100_000);

                    thread_pull_counter.increment(1);

                    let processing_latency = get_elapsed_time(&task_desc.creation_time);
                    if processing_latency > RECEIVER_WARN_PROCESSING_LATENCY {
                        is_overloaded = true;

                        if get_elapsed_time(&last_latency_warn_dump_time)
                            > RECEIVER_LATENCY_WARN_DUMP_PERIOD
                        {
                            log::warn!(
                                "Receiver thread {} overloaded: processing latency is {:.3}s (expected max {:.3}s)",
                                activity_node.get_name(),
                                processing_latency.as_secs_f64(),
                                RECEIVER_WARN_PROCESSING_LATENCY.as_secs_f64()
                            );
                            last_latency_warn_dump_time = SystemTime::now();
                        }
                    }

                    task_processor(task_desc.task);
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    // Timeout elapsed - no items in queue
                    // is_overloaded already set to false above
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    // Channel disconnected - sleep before continuing
                    const DISCONNECTED_SLEEP: Duration = Duration::from_millis(100);
                    log::warn!(
                        "Receiver thread {} task queue disconnected. Waiting for {}ms before continuing.",
                        activity_node.get_name(),
                        DISCONNECTED_SLEEP.as_millis()
                    );
                    std::thread::sleep(DISCONNECTED_SLEEP);
                }
            }

            if let Some(idle) = idle.as_mut() {
                idle();
            }
        }

        overloaded_flag.store(false, Ordering::SeqCst);
    }

    /*
        Control methods
    */

    /// Create metrics dumper
    fn create_metrics_dumper() -> utils::MetricsDumper {
        let mut metrics_dumper = utils::MetricsDumper::new();

        metrics_dumper.add_compute_handler("received_blocks", utils::compute_instance_counter);
        metrics_dumper.add_derivative_metric("received_blocks");
        metrics_dumper.add_derivative_metric("receiver_out_messages");
        metrics_dumper.add_derivative_metric("receiver_in_messages");
        metrics_dumper.add_derivative_metric("receiver_out_queries.total");
        metrics_dumper.add_derivative_metric("receiver_in_queries.total");
        metrics_dumper.add_derivative_metric("receiver_in_broadcasts");
        metrics_dumper.add_derivative_metric("receiver_db_get_txs");
        metrics_dumper.add_derivative_metric("receiver_db_put_txs");
        metrics_dumper.add_derivative_metric("receiver_main_loop_iterations");
        metrics_dumper.add_derivative_metric("receiver_main_loop_overloads");
        metrics_dumper.add_derivative_metric("receiver_db_loop_iterations");
        metrics_dumper.add_derivative_metric("receiver_db_loop_overloads");

        metrics_dumper.add_derivative_metric("receiver_overlay_in_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_out_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_in_messages_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_out_messages_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_in_queries_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_out_queries_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_in_broadcasts_bytes");
        metrics_dumper.add_derivative_metric("receiver_overlay_out_broadcasts_bytes");

        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "receiver_overlay_in_messages.avg_size",
            "receiver_overlay_in_messages_bytes",
            "receiver_in_messages",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "receiver_overlay_out_messages.avg_size",
            "receiver_overlay_out_messages_bytes",
            "receiver_out_messages",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "receiver_overlay_in_queries.avg_size",
            "receiver_overlay_in_queries_bytes",
            "receiver_in_queries.total",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "receiver_overlay_out_queries.avg_size",
            "receiver_overlay_out_queries_bytes",
            "receiver_out_queries.total",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "receiver_overlay_in_broadcasts.avg_size",
            "receiver_overlay_in_broadcasts_bytes",
            "receiver_in_broadcasts",
            0.0,
        );

        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "receiver_main_loop_load",
            "receiver_main_loop_overloads",
            "receiver_main_loop_iterations",
            0.0,
        );
        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "receiver_db_loop_load",
            "receiver_db_loop_overloads",
            "receiver_db_loop_iterations",
            0.0,
        );
        utils::add_compute_result_metric(&mut metrics_dumper, "receiver_out_queries");
        utils::add_compute_result_metric(&mut metrics_dumper, "receiver_in_queries");
        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "received_blocks_in_duplication",
            "receiver_in_messages",
            "received_blocks.create",
            -1.0,
        );
        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "received_blocks_out_duplication",
            "receiver_out_messages",
            "received_blocks.create",
            -1.0,
        );

        metrics_dumper.add_derivative_metric("receiver_main_queue.posts");
        metrics_dumper.add_derivative_metric("receiver_main_queue.pulls");
        metrics_dumper.add_derivative_metric("receiver_db_queue.posts");
        metrics_dumper.add_derivative_metric("receiver_db_queue.pulls");
        metrics_dumper
            .add_compute_handler("receiver_main_queue", utils::compute_queue_size_counter);
        metrics_dumper.add_compute_handler("receiver_db_queue", utils::compute_queue_size_counter);

        metrics_dumper
    }

    /// Main loop
    fn main_loop(
        session_id: SessionId,
        options: Options,
        listener: ReceiverListenerPtr,
        ids: Vec<CatchainNode>,
        local_key: PrivateKey,
        path: String,
        db_suffix: String,
        allow_unsafe_self_blocks_resync: bool,
        task_queues: Arc<ReceiverTaskQueues>,
        stop_flag: Arc<AtomicBool>,
        activity_node: ActivityNodePtr,
        metrics_receiver: MetricsHandle,
        overlay_manager: CatchainOverlayManagerPtr,
        status_tx: CrossbeamSender<Result<ReceiverStartupResult>>,
        db_thread_stopped: Arc<AtomicBool>,
    ) {
        //configure metrics dumper

        let mut metrics_dumper = Self::create_metrics_dumper();
        let task_queues_clone = task_queues.clone();

        let loop_counter =
            metrics_receiver.sink().register_counter(&"receiver_main_loop_iterations".into());
        let loop_overloads_counter =
            metrics_receiver.sink().register_counter(&"receiver_main_loop_overloads".into());
        let main_thread_overloaded_flag = Arc::new(AtomicBool::new(false));

        //create receiver

        let session_id_str = session_id.to_hex_string();
        let receiver = match ReceiverImpl::new(
            session_id.clone(),
            options,
            listener,
            ids,
            local_key,
            path,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            Some(metrics_receiver.clone()),
            task_queues,
            overlay_manager,
            main_thread_overloaded_flag.clone(),
        ) {
            Ok(r) => std::cell::RefCell::new(r),
            Err(err) => {
                // propagate creation error and exit the thread
                let _ = status_tx.send(Err(err));
                return;
            }
        };

        // extract required for ReceiverWrapper fields from the receiver
        let startup_result = {
            let receiver = receiver.borrow();

            ReceiverStartupResult {
                out_broadcasts_bytes: receiver.out_broadcasts_bytes.clone(),
                out_bytes: receiver.out_bytes.clone(),
                local_adnl_id: receiver
                    .get_source(receiver.local_idx)
                    .borrow()
                    .get_adnl_id()
                    .clone(),
                overlay: receiver.overlay.clone(),
            }
        };

        // signal successful start (main loop about to start)
        let _ = status_tx.send(Ok(startup_result));

        //start main loop

        let mut next_metrics_dump_time = SystemTime::now();
        let mut last_completion_handlers_check_time = SystemTime::now();

        let idle_fn = || {
            if receiver.borrow().get_next_awake_time() <= SystemTime::now() {
                receiver.borrow_mut().reset_next_awake_time();
            }

            receiver.borrow_mut().check_all();
            //receiver.borrow_mut().process(); //absent in C++ implementation; need to check

            //dump metrics

            if let Ok(_elapsed) = next_metrics_dump_time.elapsed() {
                instrument!();
                check_execution_time!(10_000);

                if log::log_enabled!(log::Level::Debug) {
                    metrics_dumper.update(&metrics_receiver);

                    log::debug!("Catchain receiver {} metrics:", session_id_str);

                    metrics_dumper.dump(|string| log::debug!("{}{}", session_id_str, string));

                    receiver.borrow().debug_dump();
                }

                next_metrics_dump_time =
                    SystemTime::now() + Duration::from_millis(RECEIVER_METRICS_DUMP_PERIOD_MS);
            }

            //check completion handlers

            if let Ok(completion_handlers_check_elapsed) =
                last_completion_handlers_check_time.elapsed()
            {
                if completion_handlers_check_elapsed > COMPLETION_HANDLERS_CHECK_PERIOD {
                    receiver.borrow_mut().completion_handlers.check_completion_handlers();
                    last_completion_handlers_check_time = SystemTime::now();
                }
            }

            receiver.borrow_mut().set_next_awake_time(
                SystemTime::now() + Duration::from_millis(RECEIVER_PROCESSING_PERIOD_MS),
            );
        };

        let timeout_fn = || -> Duration {
            let now = SystemTime::now();
            let next_awake_time = receiver.borrow().get_next_awake_time();
            if next_awake_time <= now {
                Duration::from_secs(0)
            } else {
                next_awake_time.duration_since(now).unwrap_or_else(|_| Duration::from_secs(0))
            }
        };

        let process_fn = |task: Box<dyn FnOnce(&mut ReceiverImpl) + Send>| {
            task(&mut *receiver.borrow_mut());
        };

        Self::thread_loop(
            stop_flag,
            task_queues_clone.main_thread_pull_counter.clone(),
            main_thread_overloaded_flag,
            loop_overloads_counter,
            loop_counter,
            activity_node,
            task_queues_clone.processing_task_receiver.clone(),
            Some(timeout_fn),
            process_fn,
            Some(idle_fn),
        );

        //wait for DB thread to finish

        loop {
            if db_thread_stopped.load(Ordering::SeqCst) {
                break;
            }

            log::info!(
                "...waiting for Receiver DB thread in main loop(session_id is {:x})",
                session_id
            );

            const CHECKING_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

            std::thread::sleep(CHECKING_INTERVAL);
        }

        //cleanup

        receiver.borrow_mut().stop();
    }

    /// Create new receiver
    pub(crate) fn create(
        session_id: SessionId,
        options: Options,
        listener: ReceiverListenerPtr,
        ids: Vec<CatchainNode>,
        local_key: PrivateKey,
        path: String,
        db_suffix: String,
        allow_unsafe_self_blocks_resync: bool,
        overlay_manager: CatchainOverlayManagerPtr,
    ) -> Result<ReceiverPtr> {
        let metrics_receiver = MetricsHandle::new(Some(Duration::from_secs(30)));
        let task_queues = Arc::new(ReceiverTaskQueues::new(metrics_receiver.clone()));
        let mut receiver_threads = ReceiverThreads::new(session_id.clone());

        //start DB thread

        let db_thread_stopped =
            Self::start_db_thread(&mut receiver_threads, &task_queues, &metrics_receiver)?;

        //start processing thread

        let (status_tx, status_rx) = crossbeam_channel::bounded::<Result<ReceiverStartupResult>>(1);
        let session_id_clone = session_id.clone();
        let task_queues_clone = task_queues.clone();
        let metrics_receiver_clone = metrics_receiver.clone();
        let local_id = local_key.id().clone();

        let _processing_thread_stopped = Self::start_processing_thread(
            &mut receiver_threads,
            Box::new(move |stop_flag, activity_node| {
                Self::main_loop(
                    session_id_clone,
                    options,
                    listener,
                    ids,
                    local_key,
                    path,
                    db_suffix,
                    allow_unsafe_self_blocks_resync,
                    task_queues_clone,
                    stop_flag,
                    activity_node,
                    metrics_receiver_clone,
                    overlay_manager,
                    status_tx,
                    db_thread_stopped,
                );
            }),
        )?;

        //wait until the processing thread either reports successful start or an error

        let startup_result = loop {
            match status_rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(Ok(result)) => break result, // started successfully, return the result
                Ok(Err(err)) => return Err(err),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    log::info!(
                        "...waiting for processing thread creation for session {}",
                        session_id.to_hex_string()
                    );
                    continue;
                }
                Err(_other) => {
                    // thread terminated before reporting; treat as error
                    return Err(error!("processing thread terminated before start"));
                }
            }
        };

        //create receiver wrapper

        let wrapper = ReceiverWrapper {
            receiver_threads,
            session_id: session_id.clone(),
            task_queues,
            _metrics_receiver: metrics_receiver,
            out_broadcasts_bytes: startup_result.out_broadcasts_bytes,
            out_bytes: startup_result.out_bytes,
            local_adnl_id: startup_result.local_adnl_id,
            local_id,
            overlay: startup_result.overlay,
        };

        Ok(Arc::new(wrapper))
    }
}

/*
    Protocol utils
*/

pub(crate) fn get_max_block_height(options: &Options, sources_count: usize) -> u64 {
    if options.max_block_height_coeff == 0 {
        u64::MAX
    } else {
        options.max_block_height_coeff
            * (1 + (sources_count as u64 + options.max_deps as u64 - 1) / options.max_deps as u64)
            / 1000
    }
}

/*

*/

type CompletionHandlerId = u64;

trait CompletionHandler: std::any::Any {
    ///Time of handler creation
    fn get_creation_time(&self) -> std::time::SystemTime;

    ///Execute with error
    fn reset_with_error(&mut self, error: Error);
}

pub type ResponseCallback<T> = Box<dyn FnOnce(Result<T>) + std::marker::Send>;

struct SingleThreadedCompletionHandler<T> {
    handler: Option<ResponseCallback<T>>,
    creation_time: std::time::SystemTime, //time of handler creation
}

impl<T> SingleThreadedCompletionHandler<T> {
    fn new(handler: ResponseCallback<T>) -> Self {
        Self { handler: Some(handler), creation_time: SystemTime::now() }
    }
}

impl<T> CompletionHandler for SingleThreadedCompletionHandler<T>
where
    T: 'static,
{
    ///Time of handler creation
    fn get_creation_time(&self) -> std::time::SystemTime {
        self.creation_time
    }

    ///Execute handler with error
    fn reset_with_error(&mut self, error: Error) {
        if let Some(handler) = self.handler.take() {
            handler(Err(error));
        }
    }
}

struct CompletionHandlers {
    next_completion_handler_available_index: CompletionHandlerId, //index of next available complete handler
    completion_handlers: HashMap<CompletionHandlerId, Box<dyn CompletionHandler>>, //complete handlers
    task_queues: Arc<ReceiverTaskQueues>,                                          //tasks queues
}

impl CompletionHandlers {
    fn new(task_queues: Arc<ReceiverTaskQueues>) -> Self {
        Self {
            next_completion_handler_available_index: 1,
            completion_handlers: HashMap::new(),
            task_queues,
        }
    }

    fn store_completion_handler<T>(
        &mut self,
        response_callback: ResponseCallback<T>,
    ) -> CompletionHandlerId
    where
        T: 'static,
    {
        let handler_index = self.next_completion_handler_available_index;

        self.next_completion_handler_available_index += 1;

        const MAX_COMPLETION_HANDLER_INDEX: CompletionHandlerId = u64::MAX;

        assert!(self.next_completion_handler_available_index < MAX_COMPLETION_HANDLER_INDEX);

        let handler = Box::new(SingleThreadedCompletionHandler::new(response_callback));

        self.completion_handlers.insert(handler_index, handler);

        handler_index
    }

    fn create_completion_handler<T>(
        &mut self,
        response_callback: ResponseCallback<T>,
    ) -> Box<dyn FnOnce(Result<T>) + Send>
    where
        T: 'static + Send,
    {
        let handler_index = self.store_completion_handler(response_callback);
        let task_queues = self.task_queues.clone();

        let handler = move |result: Result<T>| {
            task_queues.post_processing_closure(Box::new(move |receiver: &mut ReceiverImpl| {
                let handler =
                    receiver.completion_handlers.completion_handlers.remove(&handler_index);

                if let Some(mut handler) = handler {
                    let handler_any: &mut dyn std::any::Any = &mut handler;

                    if let Some(handler) =
                        handler_any.downcast_mut::<SingleThreadedCompletionHandler<T>>()
                    {
                        if let Some(handler) = handler.handler.take() {
                            handler(result);
                        }
                    }
                }
            }));
        };

        Box::new(handler)
    }

    fn check_completion_handlers(&mut self) {
        let mut expired_handlers = Vec::new();
        let completion_handlers = &mut self.completion_handlers;

        for (handler_id, handler) in completion_handlers.iter() {
            if let Ok(latency) = handler.get_creation_time().elapsed() {
                if latency > COMPLETION_HANDLERS_MAX_WAIT_PERIOD {
                    expired_handlers.push((*handler_id, latency));
                }
            }
        }

        for (handler_id, latency) in expired_handlers.iter_mut() {
            let handler = completion_handlers.remove(handler_id);

            if let Some(mut handler) = handler {
                let warning = error!(
                    "Remove Catchain completion handler #{} with latency {:.3}s \
                    (expected max latency is {:.3}s): created at {}",
                    handler_id,
                    latency.as_secs_f64(),
                    COMPLETION_HANDLERS_MAX_WAIT_PERIOD.as_secs_f64(),
                    utils::time_to_string(&handler.get_creation_time())
                );

                log::warn!("{}", warning);
                handler.reset_with_error(warning);
            }
        }
    }
}

/*
    ReceiverImpl
*/

struct PendingBlock {
    payload: BlockPayloadPtr,   //payload of a new block
    dep_hashes: Vec<BlockHash>, //list of dependencies for a new block
}

pub(crate) struct ReceiverImpl {
    session_id: SessionId,                        //session ID
    incarnation: SessionId,                       //incarnation (overlay ID)
    options: Options,                             //Catchain options
    task_queues: Arc<ReceiverTaskQueues>,         //tasks queues
    blocks: HashMap<BlockHash, ReceivedBlockPtr>, //all received blocks
    blocks_to_run: VecDeque<ReceivedBlockPtr>, //blocks which has been scheduled for delivering (fully resolved)
    root_block: ReceivedBlockPtr, //root block for catchain sesssion (hash is equal to session incarnation)
    last_sent_block: ReceivedBlockPtr, //last block sent to catchain
    active_send: bool,            //active send flag for adding new blocks
    pending_blocks: VecDeque<PendingBlock>, //pending blocks for sending
    sources: Vec<ReceiverSourcePtr>, //receiver sources (knowledge about other catchain validators)
    source_public_key_hashes: Vec<PublicKeyHash>, //public key hashes of all sources
    public_key_hash_to_source: HashMap<PublicKeyHash, ReceiverSourcePtr>, //map from public key hash to source
    adnl_id_to_source: HashMap<PublicKeyHash, ReceiverSourcePtr>, //map from ADNL ID to source
    local_id: PublicKeyHash,                                      //this node's public key hash
    local_key: PrivateKey,                                        //this node's private key
    local_idx: usize,                                             //this node's source index
    _local_ids: Vec<CatchainNode>, //receiver sources identifiers (pub keys, ADNL ids)
    total_forks: usize,            //total forks number for this receiver
    neighbours: Vec<usize>,        //list of neighbour indices to synchronize
    listener: ReceiverListenerPtr, //listener for callbacks
    metrics_receiver: utils::MetricsHandle, //receiver for profiling metrics
    received_blocks_instance_counter: InstanceCounter, //received blocks instances
    in_queries_counter: ResultStatusCounter, //result status counter for queries
    out_queries_counter: ResultStatusCounter, //result status counter for queries
    in_messages_counter: metrics::Counter, //incoming messages counter
    out_messages_counter: metrics::Counter, //outgoing messages counter
    _in_broadcasts_counter: metrics::Counter, //incoming broadcasts counter
    pending_in_db: i32,            //blocks pending to read from DB
    db: Option<DatabasePtr>,       //database with (BlockHash, Payload)
    read_db: bool,                 //flag to indicate receiver is in reading DB mode
    allow_broadcasts: Arc<AtomicBool>, //flag to indicate if broadcasts are allowed
    db_root_block: BlockHash,      //root DB block
    db_suffix: String,             //DB name suffix
    allow_unsafe_self_blocks_resync: bool, //indicates we can receive self blocks from other validators
    unsafe_root_block_writing: bool, //indicates we are in the middle of unsafe root block writing
    started: bool,                   //indicates catchain is started
    next_awake_time: SystemTime,     //next awake timestamp
    next_sync_time: SystemTime,      //time to do next sync with a random neighbour
    next_neighbours_rotate_time: SystemTime, //time to change neighbours
    initial_sync_complete_time: SystemTime, //time to finish initial synchronization
    rng: rand::rngs::ThreadRng,      //random generator
    get_pending_deps_call_id: u64, //unique ID for calling get_pending_deps (to cut off duplications during the blocks graph traverse)
    intentional_fork: bool,        //indicates that the fork is intentional (for debugging)
    blame_processed: Vec<bool>,    //flags: if blame is processed for each source
    pending_fork_proofs: HashMap<usize, BlockPayloadPtr>, //pending fork proofs for each source
    completion_handlers: CompletionHandlers, //completion handlers
    _overlay_id: SessionId,        //overlay ID
    overlay_short_id: Arc<PrivateOverlayShortId>, //overlay short ID
    overlay_manager: CatchainOverlayManagerPtr, //overlay manager
    overlay: CatchainOverlayPtr,   //overlay
    _overlay_listener: Arc<dyn CatchainOverlayListener + Send + Sync>, //overlay listener
    in_bytes: metrics::Counter,    //incoming traffic
    out_bytes: metrics::Counter,   //outgoing traffic
    in_queries_bytes: metrics::Counter, //incoming queries traffic
    out_queries_bytes: metrics::Counter, //outgoing queries traffic
    _in_broadcasts_bytes: metrics::Counter, //incoming broadcasts traffic
    out_broadcasts_bytes: metrics::Counter, //outgoing broadcasts traffic
    _in_messages_bytes: metrics::Counter, //incoming messages traffic
    out_messages_bytes: metrics::Counter, //outgoing messages traffic
}

/*
    Private ReceiverImpl details
*/

impl ReceiverImpl {
    /*
        Accessors
    */

    fn get_metrics_receiver(&self) -> &MetricsHandle {
        &self.metrics_receiver
    }

    pub(crate) fn get_incarnation(&self) -> &SessionId {
        &self.incarnation
    }

    pub(crate) fn get_options(&self) -> &Options {
        &self.options
    }

    /*
        Profiling tools
    */

    pub(crate) fn get_received_blocks_instance_counter(&self) -> &InstanceCounter {
        &self.received_blocks_instance_counter
    }

    /*
        Unsafe startup
    */

    fn unsafe_start_up_check_completed(&mut self) -> bool {
        instrument!();

        let source = self.get_source(self.local_idx);
        let source = source.borrow();
        let now = SystemTime::now();

        assert!(!source.is_blamed());

        if source.has_unreceived() || source.has_undelivered() {
            log::info!(
                "Catchain: has_unreceived={} has_undelivered={}",
                source.has_unreceived(),
                source.has_undelivered()
            );
            self.run_scheduler();

            static EXPECTED_INITIAL_SYNC_DURATION_WITH_UNPROCESSED: f64 = 60.0;
            self.initial_sync_complete_time =
                now + Duration::from_secs_f64(EXPECTED_INITIAL_SYNC_DURATION_WITH_UNPROCESSED);

            return false;
        }

        let delivered_height = source.get_delivered_height();

        if delivered_height == 0 {
            assert!(self.last_sent_block.borrow().get_height() == 0);
            assert!(!self.unsafe_root_block_writing);

            return true;
        }

        if self.last_sent_block.borrow().get_height() == delivered_height {
            assert!(!self.unsafe_root_block_writing);

            return true;
        }

        if self.unsafe_root_block_writing {
            self.initial_sync_complete_time =
                now + Duration::from_secs(MAX_SAFE_INITIAL_SYNC_COMPLETE_TIME_SECS);

            log::info!("Catchain: writing=true");

            return false;
        }

        self.unsafe_root_block_writing = true;

        let block = source.get_block(delivered_height);

        assert!(block.is_some());

        let received_block = block.unwrap();
        let block = received_block.borrow();

        assert!(block.is_delivered());
        assert!(block.in_db());

        let block_id_hash = block.get_hash();
        let block_raw_data = block_id_hash.as_slice().to_vec();

        self.db.as_ref().unwrap().put_block(&ZERO_HASH, block_raw_data);

        assert!(self.last_sent_block.borrow().get_height() < block.get_height());

        self.last_sent_block = received_block.clone();
        self.unsafe_root_block_writing = false;
        self.initial_sync_complete_time =
            now + Duration::from_secs(MAX_SAFE_INITIAL_SYNC_COMPLETE_TIME_SECS);

        log::info!("Catchain: need update root");

        false
    }

    /*
        Work with sources
    */

    pub(crate) fn get_sources_count(&self) -> usize {
        self.sources.len()
    }

    pub(crate) fn get_source(&self, source_id: usize) -> ReceiverSourcePtr {
        self.sources[source_id].clone()
    }

    pub(crate) fn get_source_public_key_hash(&self, source_id: usize) -> &PublicKeyHash {
        &self.source_public_key_hashes[source_id]
    }

    fn get_source_by_hash(
        &self,
        source_public_key_hash: &PublicKeyHash,
    ) -> Option<ReceiverSourcePtr> {
        if let Some(source) = self.public_key_hash_to_source.get(source_public_key_hash) {
            return Some(source.clone());
        }

        None
    }

    fn get_source_by_adnl_id(&self, adnl_id: &PublicKeyHash) -> Option<ReceiverSourcePtr> {
        if let Some(source) = self.adnl_id_to_source.get(adnl_id) {
            return Some(source.clone());
        }

        None
    }

    /*
        General ReceivedBlock methods
    */

    pub(crate) fn get_block_by_hash(&self, b: &BlockHash) -> Option<ReceivedBlockPtr> {
        instrument!();

        match self.blocks.get(b) {
            None => None,
            Some(t) => Some(t.clone()),
        }
    }

    fn get_block_id(&self, block: &ton::Block, payload: &RawBuffer) -> ton::BlockId {
        utils::get_block_id(
            &block.incarnation,
            self.get_source_public_key_hash(block.src as usize),
            block,
            payload,
            &self.get_options(),
        )
    }

    fn get_block_dependency_id(&self, dep: &ton::BlockDep) -> ton::BlockId {
        let incarnation = &self.incarnation;
        let source_hash = self.get_source_public_key_hash(dep.src as usize);
        let height = dep.height;
        let data_hash = dep.data_hash.clone();
        ::ton_api::ton::catchain::block::id::Id {
            incarnation: incarnation.clone(),
            src: utils::public_key_hash_to_int256(source_hash),
            height,
            data_hash,
        }
        .into_boxed()
    }

    fn add_received_block(&mut self, block: ReceivedBlockPtr) {
        let hash = block.borrow().get_hash().clone();
        self.blocks.insert(hash, block);
    }

    /*
        ReceivedBlock creation
    */

    pub(crate) fn create_block(&mut self, block: &ton::BlockDep) -> ReceivedBlockPtr {
        instrument!();

        if block.height == 0 {
            return self.root_block.clone();
        }

        let hash = ReceivedBlock::get_block_dep_hash(block, self);
        let block_opt = self.get_block_by_hash(&hash);

        if let Some(block) = block_opt {
            block
        } else {
            let new_block = ReceivedBlock::create(block, self);

            self.add_received_block(new_block.clone());

            new_block
        }
    }

    /*
        Block validation management
    */

    pub(crate) fn validate_block_dependency(&self, dep: &ton::BlockDep) -> Result<()> {
        instrument!();

        ReceivedBlock::pre_validate_block_dependency(self, dep)?;

        if dep.height <= 0 {
            return Ok(());
        }

        let id = &self.get_block_dependency_id(dep);
        let serialized_block_id = serialize_tl_boxed_object!(id);

        if let Some(_block) = self.get_block_by_hash(&utils::get_hash(&serialized_block_id)) {
            return Ok(());
        }

        let source = self.get_source_by_hash(&utils::int256_to_public_key_hash(id.src()));

        let source = match source {
            Some(src) => src,
            None => {
                return Err(error!(
                    "Source not found for the given hash: {:?}",
                    utils::int256_to_public_key_hash(id.src())
                ))
            }
        };

        let public_key = source.borrow().get_public_key().clone();

        match public_key.verify(&serialized_block_id, &dep.signature) {
            Err(err) => Err(err),
            Ok(_) => Ok(()),
        }
    }

    fn validate_block_with_payload(
        &self,
        block: &ton::Block,
        payload: &BlockPayloadPtr,
    ) -> Result<()> {
        instrument!();

        ReceivedBlock::pre_validate_block(self, block, payload)?;

        if block.height <= 0 {
            return Ok(());
        }

        let id = &self.get_block_id(block, payload.data());
        let serialized_block_id = serialize_tl_boxed_object!(id);

        if let Some(_block) = self.get_block_by_hash(&utils::get_hash(&serialized_block_id)) {
            return Ok(());
        }

        let source = self.get_source_by_hash(&utils::int256_to_public_key_hash(id.src()));

        let source = match source {
            Some(src) => src,
            None => {
                return Err(error!(
                    "Source not found for the given hash: {:?}",
                    utils::int256_to_public_key_hash(id.src())
                ))
            }
        };

        let public_key = source.borrow().get_public_key().clone();

        match public_key.verify(&serialized_block_id, &block.signature) {
            Err(err) => Err(err),
            Ok(_) => Ok(()),
        }
    }

    /*
        Block delivery management
    */

    /*
        ReceivedBlock delivery flow implementation
    */

    pub(crate) fn run_block(&mut self, block: ReceivedBlockPtr) {
        self.blocks_to_run.push_back(block.clone());
    }

    pub(crate) fn deliver_block(&mut self, block: &mut crate::received_block::ReceivedBlock) {
        instrument!();

        log::trace!(
            "Catchain delivering block {:?} from source={} fork={} height={} custom={} deps={:?}",
            block.get_hash(),
            block.get_source_id(),
            block.get_fork_id(),
            block.get_height(),
            block.is_custom(),
            block.get_dep_hashes()
        );

        //notify listeners about new block appearance

        lazy_static::lazy_static! {
            static ref DEFAULT_BLOCK: BlockPayloadPtr =
                CatchainFactory::create_empty_block_payload();
        }

        self.notify_on_new_block(
            block.get_source_id(),
            block.get_fork_id(),
            block.get_hash().clone(),
            block.get_height(),
            match block.get_prev() {
                Some(ref prev) => prev.borrow().get_hash().clone(),
                _ => BlockHash::default(),
            },
            block.get_dep_hashes(),
            block.get_forks_dep_heights().clone(),
            if block.is_custom() { block.get_payload().clone() } else { DEFAULT_BLOCK.clone() },
        );

        //prepare and send message with a new block to current overlay neighbours

        let mut receiver_addresses = Vec::new();
        let is_retransmission = block.get_source_id() != self.local_idx;

        for &it in &self.neighbours {
            let neighbour = self.get_source(it);
            let adnl_id = neighbour.borrow().get_adnl_id().clone();

            if !block.mark_block_for_sending(&adnl_id) {
                continue;
            }

            if self.options.disable_gossip && is_retransmission {
                log::trace!(
                    "Skipping Gossip retransmission for block {:?} from source={} height={}",
                    block.get_hash(),
                    block.get_source_id(),
                    block.get_height()
                );
                continue;
            }

            receiver_addresses.push(adnl_id);
        }

        if receiver_addresses.is_empty() {
            return;
        }

        self.send_block_update_event_multicast(
            receiver_addresses,
            block.get_serialized_block_with_payload(),
            is_retransmission,
        );
    }

    //TODO: remove after debugging
    /*fn process(&mut self) {
        instrument!();

        if !self.blocks_to_run.is_empty() {
            self.run_scheduler();
        }
    }*/

    fn run_scheduler(&mut self) {
        instrument!();

        while let Some(block) = self.blocks_to_run.pop_front() {
            block.borrow_mut().process(self);
        }
    }

    fn add_block_for_delivery(&mut self, payload: BlockPayloadPtr, deps: Vec<BlockHash>) {
        self.pending_blocks.push_back(PendingBlock { payload, dep_hashes: deps });
    }

    fn add_block_impl(&mut self, payload: BlockPayloadPtr, deps: Vec<BlockHash>) {
        instrument!();

        log::trace!("Adding new block with deps {:?} and payload {:?}", deps, payload);

        self.active_send = true;

        //check source

        let source_opt = self.get_source_by_hash(&self.local_id);

        assert!(source_opt.is_some());

        let source = source_opt.unwrap();

        assert!(source.borrow().get_id() == self.local_idx);

        if !self.intentional_fork {
            assert!(!source.borrow().is_blamed());
        }

        //prepare prev block and dependencies

        let prev = self.last_sent_block.borrow().export_tl_dep();
        let mut dep_tls = Vec::with_capacity(deps.len());

        for dep in deps {
            let block = self.get_block_by_hash(&dep);

            if block.is_none() {
                log::error!("...can't find block with hash {:?}", dep);
                unreachable!();
            }

            let block = block.unwrap();

            if !self.intentional_fork {
                assert_ne!(block.borrow().get_source_id(), self.local_idx);
            }

            dep_tls.push(block.borrow().export_tl_dep());
        }

        //prepare block

        let height = prev.height + 1;
        let max_block_height = get_max_block_height(&self.options, self.sources.len());
        if height as u64 > max_block_height {
            log::warn!(
                "Cannot create block with height {}: max height {} is exceeded",
                height,
                max_block_height
            );
            self.active_send = false;
            return;
        }

        let mut block = ton::Block {
            incarnation: self.incarnation.clone(),
            src: self.local_idx as i32,
            height,
            data: ton::BlockData { prev, deps: dep_tls },
            signature: Vec::new(), //block will be signed later
        };

        let block_id = self.get_block_id(&block, payload.data());
        let block_id_serialized = serialize_tl_boxed_object!(&block_id);

        //block ID signing

        match self.local_key.sign(&block_id_serialized) {
            Ok(block_id_signature) => {
                block.signature = block_id_signature.to_vec();
            }
            Err(_err) => {
                log::error!("...block signing error: {:?}", _err);
                self.active_send = false;
                return;
            }
        }

        log::trace!("...block has been signed {:?}", block);

        if let Err(err) = self.validate_block_with_payload(&block, &payload) {
            let message = format!(
                "Receiver {} created broken block {:?}: {}",
                self.incarnation.to_hex_string(),
                block,
                err
            );

            log::error!("{}", message);

            self.active_send = false;
            return;
        }

        //save block to DB

        log::trace!("...save block to DB");

        if let Some(ref db) = self.db {
            let db = db.clone();
            let block_id_hash = utils::get_block_id_hash(&block_id);
            let block = block.clone();
            let payload = payload.clone();

            let debug_disable_db = self.options.debug_disable_db;
            let receiver_task_queues = self.task_queues.clone();

            self.task_queues.post_database_closure(Box::new(move || {
                    if !debug_disable_db {
                        //save mapping: sha256(block_id) -> serialized block with payload

                        match utils::serialize_block_with_payload(&block, &payload) {
                            Ok(raw_data) => {
                                db.put_block(&block_id_hash, raw_data);
                            }
                            Err(err) => log::warn!("Block serialization error: {:?}", err),
                        }

                        //save mapping for root block to it's ID

                        db.put_block(
                            &ZERO_HASH,
                            block_id_hash.as_slice().to_vec()
                        );
                    }

                    //create new block and send

                receiver_task_queues.post_processing_closure(Box::new(move |receiver| {
                        //initiate delivery flow

                        log::trace!("...deliver a new created block {:?}", block);

                        match receiver.create_block_with_payload(&block, payload) {
                            Ok(block) => {
                                receiver.last_sent_block = block.clone();

                                block.borrow_mut().written(receiver);
                            }
                            Err(err) => log::error!("...creation block error: {:?}", err),
                        }

                        receiver.run_scheduler();

                        if !receiver.intentional_fork {
                            if !receiver.last_sent_block.borrow().is_delivered() {
                                log::error!("...last sent block is not delivered: source={}, state={:?}, height={}",
                                    receiver.last_sent_block.borrow().get_source_id(),
                                    receiver.last_sent_block.borrow().get_state(),
                                    receiver.last_sent_block.borrow().get_height());
                            }
                        }

                        receiver.active_send = false;

                        if let Some(pending_block) = receiver.pending_blocks.pop_front() {
                            receiver.add_block(pending_block.payload, pending_block.dep_hashes);
                        }
                    }));
                }));
        }
    }

    fn add_fork_impl(
        &mut self,
        payload: BlockPayloadPtr,
        mut height: BlockHeight,
        deps: Vec<BlockHash>,
    ) {
        instrument!();

        //initiate fork

        log::trace!("...adding fork with height {:?}", height);

        self.intentional_fork = true;

        let source = self.get_source(self.local_idx);

        assert_eq!(source.borrow().get_id(), self.local_idx);

        if height > source.borrow().get_received_height() + 1 {
            height = source.borrow().get_received_height() + 1;
        }

        assert!(height > 0);

        let prev: ReceivedBlockPtr = if height == 1 {
            self.root_block.clone()
        } else {
            let prev = source.borrow().get_block(height - 1);
            assert!(prev.is_some());
            prev.unwrap()
        };

        let mut deps_arr = Vec::with_capacity(deps.len());
        for dep in deps {
            let block = self.get_block_by_hash(&dep);
            assert!(block.is_some(), "Cannot find block with hash {:?}", dep);
            let block = block.unwrap();
            assert_ne!(block.borrow().get_source_id(), self.local_idx);
            deps_arr.push(block.borrow().export_tl_dep());
        }

        //prepare block

        let mut block = ton::Block {
            incarnation: self.incarnation.clone(),
            src: self.local_idx as i32,
            height,
            data: ton::BlockData { prev: prev.borrow().export_tl_dep(), deps: deps_arr },
            signature: Vec::new(), //block will be signed later
        };

        let block_id = self.get_block_id(&block, payload.data());
        let block_id_serialized = serialize_tl_boxed_object!(&block_id);

        //block ID signing

        match self.local_key.sign(&block_id_serialized) {
            Ok(block_id_signature) => {
                block.signature = block_id_signature.to_vec();
            }
            Err(_err) => {
                log::error!("...block signing error: {:?}", _err);
                return;
            }
        }

        log::trace!("...block fork has been signed {:?}", block);

        if let Err(err) = self.validate_block_with_payload(&block, &payload) {
            let message = format!(
                "Receiver {} parsed broken block fork {:?}: {}",
                self.incarnation.to_hex_string(),
                block,
                err
            );

            log::warn!("{}", message);

            return;
        }

        //initiate delivery flow

        log::trace!("...deliver a new created block fork {:?}", block);

        let block = match self.create_block_with_payload(&block, payload) {
            Ok(block) => {
                self.last_sent_block = block.clone();

                block.borrow_mut().written(self);

                block
            }
            Err(err) => unreachable!("...creation block error: {:?}", err),
        };

        self.run_scheduler();

        assert!(block.borrow().is_delivered());

        self.active_send = false;

        if let Some(pending_block) = self.pending_blocks.pop_front() {
            self.add_block(pending_block.payload, pending_block.dep_hashes);
        }
    }

    /*
        Receiver blocks DB management
    */

    fn start_up_db(&mut self, path: String) -> Result<()> {
        instrument!();

        log::trace!("...starting up DB");

        if self.options.debug_disable_db {
            self.read_db();
            return Ok(());
        }

        // we create special table for catchain receiver
        let db = CatchainFactory::create_database(
            path,
            format!(
                "catchainreceiver{}{}",
                self.db_suffix,
                base64_encode_url_safe(self.incarnation.as_slice()),
            ),
            self.get_metrics_receiver(),
        )?;

        self.db = Some(db.clone());

        if let Ok(root_block) = db.get_block(&ZERO_HASH) {
            let hash: [u8; 32] =
                root_block.try_into().map_err(|_| error!("Cannot convert root block hash"))?;
            let root_block_id_hash: BlockHash = hash.into();
            self.read_db_from(root_block_id_hash);
        } else {
            self.read_db();
        }
        Ok(())
    }

    fn read_db(&mut self) {
        instrument!();

        log::trace!("...reading DB");

        log::trace!("Catchain_startup: db_root_block {:?}", self.db_root_block);
        if self.db_root_block != ZERO_HASH.clone() {
            self.run_scheduler();

            match self.get_block_by_hash(&self.db_root_block) {
                None => log::warn!(
                    "Catchain_startup: no block with hash {:?} in db",
                    self.db_root_block
                ),
                Some(blk) => self.last_sent_block = blk,
            }

            assert!(self.last_sent_block.borrow().is_delivered());
        }

        self.read_db = true;
        self.allow_broadcasts.store(true, Ordering::SeqCst);

        let now = SystemTime::now();

        let neighbours_rotate_min_period_ms =
            self.options.receiver_neighbours_rotate_min_period.as_millis() as u64;
        let neighbours_rotate_max_period_ms =
            self.options.receiver_neighbours_rotate_max_period.as_millis() as u64;

        self.next_neighbours_rotate_time = now
            + Duration::from_millis(
                self.rng
                    .gen_range(neighbours_rotate_min_period_ms..neighbours_rotate_max_period_ms),
            );
        self.next_sync_time =
            now + Duration::from_millis(((0.001 * self.rng.gen_range(0.0..60.0)) * 1000.0) as u64);
        self.initial_sync_complete_time = now
            + Duration::from_secs(if self.allow_unsafe_self_blocks_resync {
                MAX_UNSAFE_INITIAL_SYNC_COMPLETE_TIME_SECS
            } else if self.sources.len() == 1 {
                //Special case: for one node in a network we don't need optimisation with block processing start lag
                0
            } else {
                //This lag is needed for optimization purpose during the restart
                //Restart flow:
                //- node reads root (last_sent_block) which means the LAST block which was generated
                //  and written to DB before restart, so forks between restored blocks and new
                //  generated blocks are impossible
                //- node fully reads DB with all root block's deps
                //- node starts blocks preprocessing which may take significant time
                //- during this preprocessing node can generate new blocks; in case some previously dead nodes
                //  appear after restart of this node it is possible node can generate new block based on messages
                //  received from such nodes; this may lead to decentralized consensus state change before preprocessing of
                //  all block which have been generated before restart, so consensus decisions will be done earlier
                //- there is no risk to publish any new block based on non-top blocks because the order in terms of height is
                //  guaranteed by last_sent_block (which is known before start of preprocessing)
                //- constant below (MAX_SAFE_INITIAL_SYNC_COMPLETE_TIME_SECS) provides reasonable timeout to skip very old blocks
                //  and prevent fully useless early blocks processing/merging
                MAX_SAFE_INITIAL_SYNC_COMPLETE_TIME_SECS
            });

        log::trace!(
            "...waiting until {:?} for DB initial complete",
            utils::time_to_string(&self.initial_sync_complete_time)
        );

        self.set_next_awake_time(self.initial_sync_complete_time);
        self.set_next_awake_time(self.next_neighbours_rotate_time);
        self.set_next_awake_time(self.next_sync_time);
    }

    fn read_db_from(&mut self, id: BlockHash) {
        instrument!();

        log::trace!("...reading DB from block {:?}", id);

        self.pending_in_db = 1;
        self.db_root_block = id.clone();

        let block_raw_data = self.db.as_ref().unwrap().get_block(&id).unwrap();

        self.task_queues.post_processing_closure(Box::new(move |receiver| {
            receiver.read_block_from_db(&id, block_raw_data);
        }));
    }

    fn read_block_from_db(&mut self, id: &BlockHash, raw_data: RawBuffer) {
        instrument!();

        log::trace!("...reading block {:?} from DB", id);

        self.pending_in_db -= 1;

        //parse header of a block

        let (message, payload) = match deserialize_boxed_with_suffix(&raw_data) {
            Ok((message, pos)) => (message, &raw_data[pos..]),
            Err(err) => {
                log::error!("DB block {:x} parsing error: {:?}", id, err);
                return;
            }
        };

        if !message.is::<::ton_api::ton::catchain::Block>() {
            log::error!(
                "DB block {:?} parsing error: object does not contain Block message: object={:?}",
                id,
                message
            );
            return;
        }

        let block = message.downcast::<::ton_api::ton::catchain::Block>().unwrap().only();

        //parse payload of a block

        let payload = CatchainFactory::create_block_payload(payload.to_vec());

        //check block ID

        let block_id = self.get_block_id(&block, payload.data());
        let block_id_hash = utils::get_block_id_hash(&block_id);

        assert!(&block_id_hash == id);

        //skip duplicates

        if let Some(block) = self.get_block_by_hash(id) {
            if block.borrow().is_initialized() {
                assert!(block.borrow().in_db());

                if self.pending_in_db == 0 {
                    //if all dependencies are read start blocks delivering

                    self.read_db();
                }

                return;
            }
        }

        //block validation

        let _source = self.get_source(block.src as usize);

        assert!(block.incarnation == self.incarnation);

        if let Err(err) = self.validate_block_with_payload(&block, &payload) {
            let message = format!(
                "Receiver {} parsed broken block {:?} from DB: {}",
                self.incarnation.to_hex_string(),
                block,
                err
            );

            log::warn!("{}", message);

            return;
        }

        //create received block

        let block = self.create_block_with_payload(&block, payload).unwrap();

        block.borrow_mut().written(self);

        //resolve dependencies

        let mut deps = block.borrow().get_dep_hashes();
        deps.push(block.borrow().get_prev_hash().unwrap());

        for dep in &deps {
            let dep_block = self.get_block_by_hash(dep);

            if let Some(dep_block) = dep_block {
                if dep_block.borrow().is_initialized() {
                    continue;
                }
            }

            //query dependency from DB

            self.pending_in_db += 1;

            let dep_block = self.db.as_ref().unwrap().get_block(dep).unwrap();
            let dep = dep.clone();

            //do recursion for block parsing

            self.task_queues.post_processing_closure(Box::new(move |receiver| {
                receiver.read_block_from_db(&dep, dep_block);
            }));
        }

        //deliver blocks when all dependencies are requested from DB

        if self.pending_in_db == 0 {
            self.read_db();
        }
    }

    fn destroy_db(&mut self) {
        if let Some(db) = &self.db {
            db.destroy();
        }
    }

    fn block_written_to_db(&mut self, block_id: &ton::BlockId) {
        instrument!();

        let block = self.get_block_by_hash(&utils::get_block_id_hash(block_id)).unwrap();

        block.borrow_mut().written(self);

        self.run_scheduler();
    }

    /*
        Neighbours management
    */

    fn choose_neighbours(&mut self) {
        instrument!();

        log::trace!("Rotate neighbours");

        //randomly choose max neighbours from sources

        let sources_count = self.get_sources_count();
        let mut new_neighbours: Vec<usize> = Vec::new();
        let mut items_count = self.options.receiver_max_neighbours_count;

        log::trace!("...choose {} neighbours from {} sources", items_count, sources_count);

        if items_count > sources_count {
            items_count = sources_count;
        }

        for i in 0..sources_count {
            if i == self.local_idx {
                continue;
            }

            if self.get_source(i).borrow().is_blamed() {
                continue;
            }

            let random_value = self.rng.gen_range(0..sources_count - i);
            if random_value >= items_count {
                continue;
            }

            new_neighbours.push(i);
            items_count -= 1;
        }

        log::trace!("...new receiver neighbours are: {:?}", new_neighbours);

        self.neighbours = new_neighbours;
    }

    fn synchronize(&mut self) {
        instrument!();

        log::trace!("Synchronize with other validators");

        let sources_count = self.get_sources_count();
        let max_sources_sync_attempts = self.options.receiver_max_sources_sync_attempts;

        for _i in 0..max_sources_sync_attempts {
            let mut source_index = self.rng.gen_range(0..sources_count);

            if source_index == self.local_idx {
                source_index = (source_index + 1) % sources_count;
            }

            let source = self.get_source(source_index);

            if source.borrow().is_blamed() {
                continue;
            }

            self.synchronize_with(source);
            break;
        }
    }

    /*
        Sources synchronization
    */

    fn synchronize_with(&mut self, source: ReceiverSourcePtr) {
        instrument!();

        log::trace!("...synchronize with source {}", source.borrow().get_id());

        assert!(!source.borrow().is_blamed());

        //prepare the list of known delivered heights for each source
        //this list will be sent to synchronization source to obtain partial absent difference back

        let sources_delivered_heights: Vec<BlockHeight> = (0..self.get_sources_count())
            .map(|i| {
                let source = self.get_source(i);
                if source.borrow().is_blamed() {
                    -1
                } else {
                    source.borrow().get_delivered_height()
                }
            })
            .collect();

        let get_difference_request = ton::GetDifferenceRequest { rt: sources_delivered_heights };

        //send a difference query to a synchronization source

        let receiver_task_queues = self.task_queues.clone();
        let receiver_task_queues_clone = receiver_task_queues.clone();

        let get_difference_response_handler =
            move |result: Result<ton::GetDifferenceResponse>, _payload: BlockPayloadPtr| {
                receiver_task_queues_clone.post_processing_closure(Box::new(move |receiver| {
                    use ton_api::ton::catchain::*;

                    match result {
                        Err(err) => {
                            receiver.out_queries_counter.failure();

                            log::warn!("GetDifference query error: {:?}", err)
                        }
                        Ok(response) => {
                            receiver.out_queries_counter.success();

                            match response {
                                Difference::Catchain_Difference(difference) => {
                                    log::trace!("GetDifference response: {:?}", difference);
                                    //result is not used as well as in C++ node
                                }
                                Difference::Catchain_DifferenceFork(difference_fork) => {
                                    receiver.got_fork_proof(&difference_fork);
                                }
                            }
                        }
                    }
                }));
            };

        source.borrow_mut().get_mut_statistics().out_queries_count += 1;

        self.out_queries_counter.total_increment();

        self.send_get_difference_request(
            source.borrow().get_adnl_id(),
            get_difference_request,
            get_difference_response_handler,
        );

        //request for absent blocks
        let delivered_height = source.borrow().get_delivered_height();
        let received_height = source.borrow().get_received_height();

        if delivered_height >= received_height {
            return;
        }

        //get first undelivered block for the source and request its dependencies

        let first_block = source.borrow().get_block(delivered_height + 1);

        if let Some(first_block) = first_block {
            let mut dep_hashes = Vec::new();

            {
                instrument!();

                const MAX_PENDING_DEPS_COUNT: usize = 16;

                self.get_pending_deps_call_id += 1;

                first_block.borrow_mut().get_pending_deps(
                    self.get_pending_deps_call_id,
                    MAX_PENDING_DEPS_COUNT,
                    &mut dep_hashes,
                );
            }

            for dep_hash in dep_hashes {
                //send getBlock request for each absent hash

                let get_block_request = ton::GetBlockRequest { block: dep_hash.clone() };
                let source_adnl_id = source.borrow().get_adnl_id().clone();
                let source_adnl_id_clone = source_adnl_id.clone();
                let max_serialized_block_size = self.options.max_serialized_block_size as usize;
                let receiver_task_queues = receiver_task_queues.clone();

                let get_block_response_handler =
                    move |result: Result<ton::BlockResultResponse>, payload: BlockPayloadPtr| {
                        receiver_task_queues.post_processing_closure(Box::new(move |receiver| {
                        use ton_api::ton::catchain::*;

                        if payload.data().len() > max_serialized_block_size as usize {
                            let message = format!(
                                "Received block with size {} which is greater than max serialized block size {}",
                                payload.data().len(),
                                max_serialized_block_size
                            );
                            log::info!("{}", message);
                            return;
                        }

                        match result {
                            Err(err) => {
                                    receiver.out_queries_counter.failure();

                                log::warn!(
                                    "GetBlock {:} query error: {:?}",
                                    dep_hash.to_hex_string(),
                                    err
                                );
                            }
                            Ok(response) => {
                                    receiver.out_queries_counter.success();

                                match response {
                                    BlockResult::Catchain_BlockNotFound => log::warn!(
                                        "GetBlock {:} query didn't find the block",
                                        dep_hash.to_hex_string()
                                    ),
                                    BlockResult::Catchain_BlockResult(block_result) => {
                                        let _block = receiver.receive_block(
                                            &source_adnl_id,
                                            &block_result.block,
                                            payload,
                                        );
                                    }
                                }
                            }
                        }
                        }));
                    };

                source.borrow_mut().get_mut_statistics().out_queries_count += 1;

                self.out_queries_counter.total_increment();

                self.send_get_block_request(
                    &source_adnl_id_clone,
                    get_block_request,
                    get_block_response_handler,
                );
            }
        }
    }

    fn process_query(
        &mut self,
        adnl_id: PublicKeyHash,
        data: BlockPayloadPtr,
    ) -> (bool, Result<BlockPayloadPtr>) {
        instrument!();

        log::trace!("Receiver: received query from {}: {:?}", adnl_id, data);

        match deserialize_boxed(data.data()) {
            Ok(message) => {
                let message = match message.downcast::<ton::GetDifferenceRequest>() {
                    Ok(message) => {
                        return (
                            true,
                            utils::serialize_query_boxed_response(
                                self.process_get_difference_query(
                                    &adnl_id,
                                    &message,
                                    get_elapsed_time(&data.get_creation_time()),
                                ),
                            ),
                        )
                    }
                    Err(message) => message,
                };

                let message = match message.downcast::<ton::GetBlockRequest>() {
                    Ok(message) => match self.process_get_block_query(&adnl_id, &message) {
                        Ok(response) => {
                            let mut ret: RawBuffer = RawBuffer::default();
                            let mut serializer = ton_api::Serializer::new(&mut ret);

                            serializer.write_boxed(&response.0).unwrap();
                            serializer.write_bare(response.1.data()).unwrap();

                            return (true, Ok(CatchainFactory::create_block_payload(ret)));
                        }
                        Err(err) => return (true, Err(err)),
                    },
                    Err(message) => message,
                };

                (false, Err(error!("unknown query received {:?}", message)))
            }
            Err(err) => (true, Err(err)),
        }
    }

    fn process_get_difference_query(
        &mut self,
        adnl_id: &PublicKeyHash,
        query: &ton::GetDifferenceRequest,
        query_latency: std::time::Duration,
    ) -> Result<ton::GetDifferenceResponse> {
        instrument!();

        log::trace!("Got GetDifferenceRequest: {:?}", query);

        let sources_delivered_heights = &*query.rt;

        if sources_delivered_heights.len() != self.get_sources_count() {
            log::warn!("Incorrect GetDifferenceRequest query from {}", adnl_id);
            fail!("bad vt size");
        }

        //check is fork detected for sources

        let sources_count = self.get_sources_count();
        for (i, height) in sources_delivered_heights.iter().enumerate().take(sources_count) {
            if *height < 0 {
                continue;
            }
            if let Some(fork) = self.get_source(i).borrow().get_fork_proof() {
                //return differenceFork as response
                return Ok(ton::DifferenceFork {
                    left: fork.left.clone().only(),
                    right: fork.right.clone().only(),
                }
                .into_boxed());
            }
        }

        //prepare list of delivered heights for current node

        let ours_sources_delivered_heights: Vec<BlockHeight> = (0..sources_count)
            .map(|i| {
                if sources_delivered_heights[i] >= 0 {
                    let source_ptr = self.get_source(i).clone();
                    let source = source_ptr.borrow();

                    source.get_delivered_height()
                } else {
                    -1
                }
            })
            .collect();

        //compute optimal number of blocks for sending

        const MAX_BLOCKS_TO_SEND: BlockHeight = 100;
        const OVERLOAD_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
        const OVERLOAD_MAX_DIVIDER: f64 = 20.0;

        let mut max_blocks_to_send = MAX_BLOCKS_TO_SEND;

        if query_latency > OVERLOAD_DELAY {
            let mut divider = query_latency.as_secs_f64() / OVERLOAD_DELAY.as_secs_f64();

            if divider > OVERLOAD_MAX_DIVIDER {
                divider = OVERLOAD_MAX_DIVIDER;
            }

            max_blocks_to_send = (max_blocks_to_send as f64 / divider) as i32;
        }

        let mut left: BlockHeight = 0;
        let mut right: BlockHeight = max_blocks_to_send + 1;

        while right - left > 1 {
            let middle = (right + left) / 2;
            let mut sum: i64 = 0;

            for i in 0..sources_count {
                let diff = ours_sources_delivered_heights[i] - sources_delivered_heights[i];

                if sources_delivered_heights[i] >= 0 && diff > 0 {
                    //increase number of blocks for delivering if there are delivered blocks on current validator
                    //which are not known by counterparty

                    sum += if diff > middle { middle } else { diff } as i64;
                }
            }

            //limit number of blocks for sending

            if sum > max_blocks_to_send as i64 {
                right = middle;
            } else {
                left = middle;
            }
        }

        //send blocks to counterparty

        assert!(right > 0);

        let mut response_sources_delivered_heights: Vec<BlockHeight> =
            sources_delivered_heights.to_vec().clone();

        let mut total_sent_blocks = 0;

        for i in 0..sources_count {
            let diff = ours_sources_delivered_heights[i] - sources_delivered_heights[i];

            if sources_delivered_heights[i] < 0 || diff <= 0 {
                continue;
            }

            let source = self.get_source(i);
            let blocks_to_send = if diff > right { right } else { diff };

            assert!(blocks_to_send > 0);

            for _j in 0..blocks_to_send {
                response_sources_delivered_heights[i] += 1; //absent in C++ node: we need to send not source heights, but heights which we have sent to counterparty sources

                let block_ptr =
                    source.borrow().get_block(response_sources_delivered_heights[i]).unwrap();
                let mut block = block_ptr.borrow_mut();

                if block.mark_block_for_sending(adnl_id) {
                    //send block update event to counterparty

                    let is_retransmission = block.get_source_id() != self.local_idx;

                    self.send_block_update_event(
                        adnl_id,
                        block.get_serialized_block_with_payload(),
                        is_retransmission,
                    );

                    total_sent_blocks += 1;
                }
            }
        }

        const BLOCKS_SENT_WARN_THRESHOLD: usize = MAX_BLOCKS_TO_SEND as usize / 2;

        if total_sent_blocks > BLOCKS_SENT_WARN_THRESHOLD {
            log::warn!(
                "Sending {} absent blocks to node with ADNL ID {}",
                total_sent_blocks,
                adnl_id
            );
        }

        //send response to counterparty

        let response = ::ton_api::ton::catchain::difference::Difference {
            sent_upto: response_sources_delivered_heights,
        }
        .into_boxed();

        Ok(response)
    }

    fn process_get_block_query(
        &mut self,
        _adnl_id: &PublicKeyHash,
        query: &ton::GetBlockRequest,
    ) -> Result<(ton::BlockResultResponse, BlockPayloadPtr)> {
        instrument!();

        log::trace!("Got GetBlockQuery: {:?}", query);

        let block_hash = query.block.clone();
        let block_result = self.get_block_by_hash(&block_hash);

        if let Some(block_ptr) = block_result {
            let block = block_ptr.borrow();

            if block.get_height() != 0 && block.is_initialized() {
                let response =
                    ::ton_api::ton::catchain::blockresult::BlockResult { block: block.export_tl() }
                        .into_boxed();

                return Ok((response, block.get_payload().clone()));
            }
        }

        let response = ::ton_api::ton::catchain::BlockResult::Catchain_BlockNotFound;

        Ok((response, CatchainFactory::create_empty_block_payload()))
    }

    pub(crate) fn receive_block(
        &mut self,
        adnl_id: &PublicKeyHash,
        block: &ton::Block,
        payload: BlockPayloadPtr,
    ) -> Result<ReceivedBlockPtr> {
        instrument!();

        let id = self.get_block_id(block, payload.data());
        let hash = utils::get_block_id_hash(&id);
        let block_opt = self.get_block_by_hash(&hash);

        log::trace!("New block with hash={:?} and id={:?} has been received", hash, id);

        if let Some(block) = &block_opt {
            if block.borrow().is_initialized() {
                log::trace!("...skip block {:?} because it has been already initialized", hash);

                return Ok(block.clone());
            }
        }

        if block.incarnation != self.incarnation {
            let warning = error!(
                "Block from source {} incarnation mismatch: expected {} but received {:?}",
                adnl_id,
                self.incarnation.to_hex_string(),
                block.incarnation
            );
            log::warn!("{}", warning);
            return Err(warning);
        }

        let max_block_height = get_max_block_height(&self.options, self.sources.len());
        if block.height as u64 > max_block_height {
            let warning = error!(
                "Received too many blocks from source {} (height={}, max_height={})",
                adnl_id, block.height, max_block_height
            );
            log::warn!("{}", warning);
            return Err(warning);
        }

        let src_idx = block.src as usize;
        if src_idx >= self.sources.len() {
            let warning = error!(
                "Received broken block from source {} with index {} which is out of range (max index is {})",
                adnl_id, src_idx, self.sources.len() - 1
            );
            log::warn!("{}", warning);
            return Err(warning);
        }

        let source = self.get_source(src_idx);
        if source.borrow().is_fork_found() {
            if block_opt.is_none() || !block_opt.as_ref().unwrap().borrow().has_rev_deps() {
                let warning = error!(
                    "Dropping block from source {} with index {}: source has a fork",
                    adnl_id, src_idx
                );
                log::warn!("{}", warning);
                return Err(warning);
            }
        }

        if let Err(validation_error) = self.validate_block_with_payload(block, &payload) {
            let warning = error!(
                "Receiver {} received broken block from source {}: {}",
                self.incarnation.to_hex_string(),
                adnl_id,
                validation_error
            );
            log::warn!("{}", warning);
            return Err(warning);
        }

        if block.src as usize == self.local_idx {
            if !self.allow_unsafe_self_blocks_resync || self.started {
                log::error!(
                    "Receiver {} has received unknown SELF block from {} (unsafe={})",
                    self.incarnation.to_hex_string(),
                    adnl_id,
                    self.allow_unsafe_self_blocks_resync
                );

                if !cfg!(debug_assertions) {
                    panic!("Unknown SELF block is received");
                }
            } else {
                log::error!(
                    "Receiver {} has received unknown SELF block from {}. \
                    UPDATING LOCAL DATABASE. UNSAFE",
                    self.incarnation.to_hex_string(),
                    adnl_id
                );

                self.initial_sync_complete_time = SystemTime::now()
                    + Duration::from_secs(MAX_UNSAFE_INITIAL_SYNC_COMPLETE_TIME_SECS);

                self.set_next_awake_time(self.initial_sync_complete_time);
            }
        }

        let received_block = self.create_block_with_payload(block, payload.clone())?;

        if let Some(ref db) = self.db {
            let hash = hash.clone();
            let block = block.clone();
            let payload = payload.clone();
            let db = db.clone();

            let id = id.clone();
            let hash = hash.clone();
            let debug_disable_db = self.options.debug_disable_db;
            let receiver_task_queues = self.task_queues.clone();

            self.task_queues.post_database_closure(Box::new(move || {
                match utils::serialize_block_with_payload(&block, &payload) {
                    Ok(raw_data) => {
                        if !debug_disable_db {
                            db.put_block(&hash, raw_data);
                        }

                        receiver_task_queues.post_processing_closure(Box::new(move |receiver| {
                            receiver.block_written_to_db(&id);

                            log::trace!(
                                "...block {:?} has been successfully processed after receiving",
                                hash
                            );
                        }));
                    }
                    Err(err) => log::warn!("Block serialization error: {:?}", err),
                }
            }));
        }

        Ok(received_block)
    }

    /*
        Forks management
    */

    pub(crate) fn add_fork(&mut self) -> usize {
        self.total_forks += 1;

        let fork_id = self.total_forks;

        log::trace!("...new fork {} has been added for receiver", fork_id);

        fork_id
    }

    fn blame_processed(&mut self, source_id: usize) {
        self.blame_processed[source_id] = true;

        if let Some(pending_fork_proof) = self.pending_fork_proofs.get(&source_id) {
            self.add_block(pending_fork_proof.clone(), Vec::new());
            self.pending_fork_proofs.remove(&source_id);
        }
    }

    pub(crate) fn blame(&mut self, source_id: usize) {
        self.notify_on_blame(source_id);
    }

    pub(crate) fn add_fork_proof(&mut self, source_id: usize, fork_proof: &BlockPayloadPtr) {
        if self.blame_processed[source_id] {
            log::trace!("...add block {:?} as a fork proof from source {}", fork_proof, source_id);
            self.add_block(fork_proof.clone(), Vec::new());
        } else {
            self.pending_fork_proofs.insert(source_id, fork_proof.clone());
        }
    }

    fn got_fork_proof(&mut self, fork_proof: &ton::DifferenceFork) {
        if let Err(status) = self.validate_block_dependency(&fork_proof.left) {
            log::warn!("Incorrect fork blame, left is invalid: {:?}", status);
            return;
        }

        if let Err(status) = self.validate_block_dependency(&fork_proof.right) {
            log::warn!("Incorrect fork blame, right is invalid: {:?}", status);
            return;
        }

        if fork_proof.left.height != fork_proof.right.height
            || fork_proof.left.src != fork_proof.right.src
            || fork_proof.left.data_hash == fork_proof.right.data_hash
        {
            log::warn!(
                "Incorrect fork blame, not a fork: {}/{}, {}/{}, {:?}/{:?}",
                fork_proof.left.height,
                fork_proof.right.height,
                fork_proof.left.src,
                fork_proof.right.src,
                fork_proof.left.data_hash,
                fork_proof.right.data_hash
            );
            return;
        }

        let source = self.get_source(fork_proof.left.src as usize);

        source.borrow_mut().set_fork_proof(ton::BlockDataFork {
            left: fork_proof.left.clone().into_boxed(),
            right: fork_proof.right.clone().into_boxed(),
        });
        source.borrow_mut().mark_as_blamed(self);
    }

    /*
        Network messages transfering from Receiver to Overlay
    */

    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        check_execution_time!(20000);
        instrument!();

        self.out_queries_bytes.increment(query.data().len() as u64);
        self.out_bytes.increment(query.data().len() as u64);

        self.overlay.send_query_via_rldp(
            dst_adnl_id,
            name,
            response_callback,
            timeout,
            query,
            max_answer_size,
            v2,
        );
    }

    fn send_message(
        &self,
        receiver_id: PublicKeyHash,
        sender_id: PublicKeyHash,
        message: BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        check_execution_time!(20000);
        instrument!();

        self.out_messages_bytes.increment(message.data().len() as u64);
        self.out_bytes.increment(message.data().len() as u64);

        self.overlay.send_message(&receiver_id, &sender_id, &message, is_retransmission);
    }

    fn send_message_multicast(
        &self,
        receiver_ids: Vec<PublicKeyHash>,
        sender_id: PublicKeyHash,
        message: BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        check_execution_time!(20000);
        instrument!();

        self.out_messages_bytes.increment((message.data().len() * receiver_ids.len()) as u64);
        self.out_bytes.increment((message.data().len() * receiver_ids.len()) as u64);

        self.overlay.send_message_multicast(&receiver_ids, &sender_id, &message, is_retransmission);
    }

    fn send_query(
        &self,
        receiver_id: PublicKeyHash,
        sender_id: PublicKeyHash,
        name: &str,
        timeout: std::time::Duration,
        message: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        check_execution_time!(20000);
        instrument!();

        self.out_queries_bytes.increment(message.data().len() as u64);
        self.out_bytes.increment(message.data().len() as u64);

        let in_queries_bytes = self.in_queries_bytes.clone();
        let in_bytes = self.in_bytes.clone();

        let completion_handler = Box::new(move |result: Result<BlockPayloadPtr>| {
            if let Ok(payload) = &result {
                in_queries_bytes.increment(payload.data().len() as u64);
                in_bytes.increment(payload.data().len() as u64);
            }
            response_callback(result);
        });

        self.overlay.send_query(
            &receiver_id,
            &sender_id,
            name,
            timeout,
            &message,
            completion_handler,
        );
    }

    /*
        Listener callbacks (Receiver -> Catchain)
    */

    fn notify_on_started(&mut self) {
        check_execution_time!(20000);

        if let Some(listener) = self.listener.upgrade() {
            listener.on_started();
        }
    }

    fn notify_on_new_block(
        &self,
        source_id: usize,
        fork_id: usize,
        hash: BlockHash,
        height: BlockHeight,
        prev: BlockHash,
        deps: Vec<BlockHash>,
        forks_dep_heights: Vec<BlockHeight>,
        payload: BlockPayloadPtr,
    ) {
        check_execution_time!(20000);
        instrument!();

        if let Some(listener) = self.listener.upgrade() {
            listener.on_new_block(
                source_id,
                fork_id,
                hash,
                height,
                prev,
                deps,
                forks_dep_heights,
                payload,
            );
        }
    }

    fn notify_on_blame(&self, source_id: usize) {
        check_execution_time!(20000);
        if let Some(listener) = self.listener.upgrade() {
            listener.on_blame(source_id);
        }
    }

    fn notify_on_custom_query(
        &self,
        source_public_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
        response_promise: QueryResponseCallback,
    ) {
        check_execution_time!(20000);
        instrument!();

        if let Some(listener) = self.listener.upgrade() {
            listener.on_custom_query(source_public_key_hash, data, response_promise);
        }
    }

    /*
        Listener callbacks (Catchain -> Overlay)
    */

    fn send_block_update_event(
        &self,
        receiver_adnl_id: &PublicKeyHash,
        serialized_block_with_payload: &BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        instrument!();

        assert!(
            serialized_block_with_payload.data().len()
                <= self.options.max_serialized_block_size as usize
        );

        log::trace!("Send to {}: {:?}", receiver_adnl_id, serialized_block_with_payload);

        if let Some(ref source) = self.get_source_by_adnl_id(receiver_adnl_id) {
            source.borrow_mut().get_mut_statistics().out_messages_count += 1;
        }

        self.out_messages_counter.increment(1);

        self.send_message(
            receiver_adnl_id.clone(),
            self.get_source(self.local_idx).borrow().get_adnl_id().clone(),
            serialized_block_with_payload.clone(),
            is_retransmission,
        );
    }

    fn send_block_update_event_multicast(
        &mut self,
        receiver_adnl_ids: Vec<PublicKeyHash>,
        serialized_block_with_payload: &BlockPayloadPtr,
        is_retransmission: bool,
    ) {
        instrument!();

        assert!(
            serialized_block_with_payload.data().len()
                <= self.options.max_serialized_block_size as usize
        );

        log::trace!(
            "Send to {:?}: {:?}",
            utils::public_key_hashes_to_string(&receiver_adnl_ids),
            serialized_block_with_payload
        );

        self.out_messages_counter.increment(1);

        for receiver_adnl_id in &receiver_adnl_ids {
            if let Some(ref source) = self.get_source_by_adnl_id(receiver_adnl_id) {
                source.borrow_mut().get_mut_statistics().out_messages_count += 1;
            }
        }

        self.send_message_multicast(
            receiver_adnl_ids,
            self.get_source(self.local_idx).borrow().get_adnl_id().clone(),
            serialized_block_with_payload.clone(),
            is_retransmission,
        );
    }

    fn create_response_handler_boxed<T, F>(
        &mut self,
        response_callback: F,
    ) -> ResponseCallback<BlockPayloadPtr>
    where
        T: 'static + ::ton_api::BoxedDeserialize + ::ton_api::AnyBoxedSerialize,
        F: FnOnce(Result<T>, BlockPayloadPtr) + std::marker::Send + 'static,
    {
        let boxed_response_callback = Box::new(response_callback);

        let handler = move |result: Result<BlockPayloadPtr>| match result {
            Err(err) => {
                boxed_response_callback(Err(err), CatchainFactory::create_empty_block_payload())
            }
            Ok(payload) => match deserialize_boxed_with_suffix(payload.data()) {
                Ok((response, pos)) => match response.downcast::<T>() {
                    Ok(response) => {
                        let payload =
                            CatchainFactory::create_block_payload(payload.data()[pos..].to_vec());
                        boxed_response_callback(Ok(response), payload)
                    }
                    Err(obj) => boxed_response_callback(
                        Err(error!("unknown response {:?}", obj)),
                        CatchainFactory::create_empty_block_payload(),
                    ),
                },
                Err(err) => {
                    boxed_response_callback(Err(err), CatchainFactory::create_empty_block_payload())
                }
            },
        };

        let in_queries_bytes = self.in_queries_bytes.clone();
        let in_bytes = self.in_bytes.clone();

        let completion_handler = self.completion_handlers.create_completion_handler(Box::new(
            move |result: Result<BlockPayloadPtr>| {
                if let Ok(payload) = &result {
                    in_queries_bytes.increment(payload.data().len() as u64);
                    in_bytes.increment(payload.data().len() as u64);
                }
                handler(result);
            },
        ));

        completion_handler
    }

    fn send_get_block_request<F>(
        &mut self,
        receiver_adnl_id: &PublicKeyHash,
        request: ton::GetBlockRequest,
        response_callback: F,
    ) where
        F: FnOnce(Result<ton::BlockResultResponse>, BlockPayloadPtr) + std::marker::Send + 'static,
    {
        instrument!();

        log::trace!("...query GetBlock {}: {:?}", receiver_adnl_id, request);

        let serialized_message = serialize_tl_boxed_object!(&request);

        static GET_BLOCK_QUERY_TIMEOUT: Duration = Duration::from_millis(2000);

        let response_callback = self.create_response_handler_boxed(response_callback);

        self.send_query(
            receiver_adnl_id.clone(),
            self.get_source(self.local_idx).borrow().get_adnl_id().clone(),
            "sync blocks",
            GET_BLOCK_QUERY_TIMEOUT,
            CatchainFactory::create_block_payload(serialized_message),
            response_callback,
        );
    }

    fn send_get_difference_request<F>(
        &mut self,
        receiver_adnl_id: &PublicKeyHash,
        request: ton::GetDifferenceRequest,
        response_callback: F,
    ) where
        F: FnOnce(Result<ton::GetDifferenceResponse>, BlockPayloadPtr)
            + std::marker::Send
            + 'static,
    {
        instrument!();

        log::trace!("...query GetDifference {}: {:?}", receiver_adnl_id, request);

        let serialized_message = serialize_tl_boxed_object!(&request);

        static GET_DIFFERENCE_QUERY_TIMEOUT: Duration = Duration::from_millis(5000);

        let response_callback = self.create_response_handler_boxed(response_callback);

        self.send_query(
            receiver_adnl_id.clone(),
            self.get_source(self.local_idx).borrow().get_adnl_id().clone(),
            "sync",
            GET_DIFFERENCE_QUERY_TIMEOUT,
            CatchainFactory::create_block_payload(serialized_message),
            response_callback,
        );
    }

    /*
        Debug dump
    */

    fn debug_dump(&self) {
        let sources_count = self.get_sources_count();
        let session_id_str = self.session_id.to_hex_string();

        log::debug!(
            "Catchain {} debug dump (local_idx={}, sources_count={}):",
            session_id_str,
            self.local_idx,
            sources_count
        );

        for i in 0..sources_count {
            let source = self.get_source(i);
            let source = source.borrow();
            let stat = source.get_statistics();

            log::debug!(
                "{} {}v{:03}/{:03}: {} delivered={:4}{}, received={:4}{}, forks={}, \
                queries={:4}/{:4}, msgs={:4}/{:4}, in_bcasts={:4}, adnl_id={}, \
                pubkey_hash={}",
                session_id_str,
                if self.local_idx == i { ">" } else { " " },
                i,
                sources_count,
                if source.is_blamed() { "blamed" } else { "" },
                source.get_delivered_height(),
                if source.has_undelivered() { "+" } else { " " },
                source.get_received_height(),
                if source.has_unreceived() { "+" } else { " " },
                source.get_forks_count(),
                stat.in_queries_count,
                stat.out_queries_count,
                stat.in_messages_count,
                stat.out_messages_count,
                stat.in_broadcasts_count,
                source.get_adnl_id(),
                source.get_public_key_hash()
            );
        }
    }

    /*
        Queries processing
    */

    fn receive_query_from_overlay(
        &mut self,
        adnl_id: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        check_execution_time!(50000);
        instrument!();

        let source = self.get_source_by_adnl_id(&adnl_id);

        if let Some(ref source) = source {
            source.borrow_mut().get_mut_statistics().in_queries_count += 1;
        }

        let in_query_status = self.in_queries_counter.clone();

        in_query_status.total_increment();

        if !self.read_db {
            let err =
                error!("DB is not read for catchain receiver {}", self.incarnation.to_hex_string());
            response_callback(Err(err));
            return;
        }

        let data_clone = data.clone();
        let (processed, result) = self.process_query(adnl_id, data);

        if processed {
            if result.is_ok() {
                in_query_status.success();
            } else {
                in_query_status.failure();
            }

            response_callback(result);
            return;
        }

        if let Some(source) = source {
            let source_public_key_hash = source.borrow().get_public_key_hash().clone();

            in_query_status.success(); //TODO: add statistics processing for custom queries

            self.notify_on_custom_query(source_public_key_hash, data_clone, response_callback);
        }
    }

    /*
        ReceivedBlock receiving flow implementation
    */

    fn receive_message_from_overlay(&mut self, adnl_id: PublicKeyHash, data: BlockPayloadPtr) {
        instrument!();

        self.in_messages_counter.increment(1);

        if let Some(ref source) = self.get_source_by_adnl_id(&adnl_id) {
            source.borrow_mut().get_mut_statistics().in_messages_count += 1;
        }

        if !self.read_db {
            //log::warn!("DB is not read");
            return;
        }

        let bytes = data.data();

        if bytes.len() > self.options.max_serialized_block_size as usize {
            log::warn!(
                "Received block with size {} which is greater than max serialized block size {}",
                bytes.len(),
                self.options.max_serialized_block_size
            );
            return;
        }

        let result = match deserialize_boxed_with_suffix(bytes) {
            Ok((message, pos)) => {
                if message.is::<::ton_api::ton::catchain::Update>() {
                    let payload = CatchainFactory::create_block_payload(bytes[pos..].to_vec());
                    self.receive_block(
                        &adnl_id,
                        message.downcast::<::ton_api::ton::catchain::Update>().unwrap().block(),
                        payload,
                    )
                } else {
                    let err = error!("unknown message received {:?}", message);

                    //error!("{}", err);

                    Err(err)
                }
            }
            Err(err) => Err(err),
        };

        if let Err(err) = result {
            log::warn!("Error receiving block: {}", err);
        }
    }

    /*
        Adding new block (initiated by validator session during new block creation)
    */

    fn add_block(&mut self, payload: BlockPayloadPtr, deps: Vec<BlockHash>) {
        instrument!();

        if self.active_send {
            self.add_block_for_delivery(payload, deps);
            return;
        }

        self.add_block_impl(payload, deps);
    }

    fn debug_add_fork(
        &mut self,
        payload: BlockPayloadPtr,
        height: BlockHeight,
        deps: Vec<BlockHash>,
    ) {
        instrument!();

        self.add_fork_impl(payload, height, deps);
    }

    /*
        Triggers
    */

    fn check_all(&mut self) {
        instrument!();

        let now = SystemTime::now();
        log::trace!("Catchain_startup: check_all called; now {:?}", now);

        //synchronize with chosen neighbours

        if let Ok(_elapsed) = self.next_sync_time.elapsed() {
            self.synchronize();

            let neighbours_sync_min_period_ms =
                self.options.receiver_neighbours_sync_min_period.as_millis() as u64;
            let neighbours_sync_max_period_ms =
                self.options.receiver_neighbours_sync_max_period.as_millis() as u64;

            let delay = Duration::from_millis(
                self.rng
                    .gen_range(neighbours_sync_min_period_ms..neighbours_sync_max_period_ms + 1),
            );

            self.next_sync_time = now + delay;

            log::trace!(
                "...next sync is scheduled at {} (in {:.3}s from now)",
                utils::time_to_string(&self.next_sync_time),
                delay.as_secs_f64(),
            );
        }

        //rotate neighbours

        if let Ok(_elapsed) = self.next_neighbours_rotate_time.elapsed() {
            self.choose_neighbours();

            let neighbours_rotate_min_period_ms =
                self.options.receiver_neighbours_rotate_min_period.as_millis() as u64;
            let neighbours_rotate_max_period_ms =
                self.options.receiver_neighbours_rotate_max_period.as_millis() as u64;

            let delay =
                Duration::from_millis(self.rng.gen_range(
                    neighbours_rotate_min_period_ms..neighbours_rotate_max_period_ms + 1,
                ));

            self.next_neighbours_rotate_time = now + delay;

            log::trace!(
                "...next neighbours rotation is scheduled at {} (in {:.3}s from now)",
                utils::time_to_string(&self.next_neighbours_rotate_time),
                delay.as_secs_f64(),
            );
        }

        //start up checks (for unsafe startup)
        log::trace!(
            "Catchain_startup: self.started {}, self.read_db {}",
            self.started,
            self.read_db
        );

        if !self.started && self.read_db {
            let elapsed = self.initial_sync_complete_time.elapsed();
            log::trace!(
                "Catchain_startup: initial sync complete time {:?}, now {:?}, elapsed? {:?}",
                self.initial_sync_complete_time,
                SystemTime::now(),
                elapsed
            );

            if let Ok(_elapsed) = elapsed {
                let allow = if self.allow_unsafe_self_blocks_resync {
                    self.unsafe_start_up_check_completed()
                } else {
                    true
                };
                log::trace!("Catchain_startup: allow {}", allow);

                if allow {
                    self.initial_sync_complete_time =
                        now + Duration::from_secs(INFINITE_INITIAL_SYNC_COMPLETE_TIME_SECS);
                    self.started = true;

                    self.notify_on_started();
                }
            }
        }

        //update awake time

        self.set_next_awake_time(self.initial_sync_complete_time);
        self.set_next_awake_time(self.next_neighbours_rotate_time);
        self.set_next_awake_time(self.next_sync_time);
    }

    fn set_next_awake_time(&mut self, timestamp: std::time::SystemTime) {
        if timestamp > self.next_awake_time {
            return;
        }

        self.next_awake_time = timestamp;
    }

    fn reset_next_awake_time(&mut self) {
        self.next_awake_time = std::time::SystemTime::now() + self.options.idle_timeout;
    }

    fn get_next_awake_time(&self) -> std::time::SystemTime {
        self.next_awake_time
    }

    /*
        Creation & stopping
    */

    fn stop(&mut self) {
        self.overlay_manager.stop_overlay(&self.overlay_short_id, &self.overlay);
    }

    fn new(
        session_id: SessionId,
        options: Options,
        listener: ReceiverListenerPtr,
        ids: Vec<CatchainNode>,
        local_key: PrivateKey,
        path: String,
        db_suffix: String,
        allow_unsafe_self_blocks_resync: bool,
        metrics_receiver: Option<MetricsHandle>,
        task_queues: Arc<ReceiverTaskQueues>,
        overlay_manager: CatchainOverlayManagerPtr,
        main_thread_overloaded_flag: Arc<AtomicBool>,
    ) -> Result<Self> {
        let metrics_receiver = if let Some(metrics_receiver) = metrics_receiver {
            metrics_receiver.clone()
        } else {
            MetricsHandle::new(Some(Duration::from_secs(30)))
        };
        let received_blocks_instance_counter =
            InstanceCounter::new(&metrics_receiver, "received_blocks");
        let out_queries_counter =
            ResultStatusCounter::new(&metrics_receiver, "receiver_out_queries");
        let in_queries_counter = ResultStatusCounter::new(&metrics_receiver, "receiver_in_queries");
        let out_messages_counter =
            metrics_receiver.sink().register_counter(&"receiver_out_messages".into());
        let in_messages_counter =
            metrics_receiver.sink().register_counter(&"receiver_in_messages".into());
        let in_broadcasts_counter =
            metrics_receiver.sink().register_counter(&"receiver_in_broadcasts".into());

        let in_messages_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_in_messages_bytes".into());
        let out_messages_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_out_messages_bytes".into());
        let in_queries_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_in_queries_bytes".into());
        let out_queries_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_out_queries_bytes".into());
        let in_broadcasts_bytes = metrics_receiver
            .sink()
            .register_counter(&"receiver_overlay_in_broadcasts_bytes".into());
        let out_broadcasts_bytes = metrics_receiver
            .sink()
            .register_counter(&"receiver_overlay_out_broadcasts_bytes".into());
        let in_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_in_bytes".into());
        let out_bytes =
            metrics_receiver.sink().register_counter(&"receiver_overlay_out_bytes".into());

        //sources preparation

        let mut sources: Vec<PublicKeyHash> = Vec::new();
        let mut local_idx = ids.len();
        let local_id = local_key.id().clone();

        sources.reserve(ids.len());

        for i in 0..ids.len() {
            sources.push(utils::get_public_key_hash(&ids[i].public_key));

            if sources[i] == local_id {
                local_idx = i;
            }
        }

        assert!(local_idx < ids.len());

        //compute incarnation

        let sources_as_int256: Vec<UInt256> =
            sources.clone().into_iter().map(|key| utils::public_key_hash_to_int256(&key)).collect();
        let first_block = ::ton_api::ton::catchain::firstblock::Firstblock {
            unique_hash: session_id.clone(),
            nodes: sources_as_int256,
        }
        .into_boxed();
        let overlay_id = utils::get_overlay_id(&first_block)?;
        let overlay_short_id = OverlayUtils::calc_private_overlay_short_id(&first_block)?;
        let incarnation = overlay_id.clone();

        let sources_count = ids.len();

        log::debug!(
            "Creating catchain receiver for session incarnation {:?} with {} sources",
            incarnation,
            sources_count
        );

        //overlay creation

        log::info!(
            "Receiver: starting up overlay for session {:x} with ID {:x}, short_id {}",
            session_id,
            overlay_id,
            overlay_short_id
        );

        let allow_broadcasts = Arc::new(AtomicBool::new(false));

        let overlay_listener = OverlayListenerImpl::create(
            session_id.clone(),
            incarnation.clone(),
            task_queues.clone(),
            listener.clone(),
            main_thread_overloaded_flag.clone(),
            allow_broadcasts.clone(),
            in_bytes.clone(),
            out_bytes.clone(),
            in_messages_bytes.clone(),
            in_broadcasts_bytes.clone(),
            in_broadcasts_counter.clone(),
            in_queries_bytes.clone(),
            out_queries_bytes.clone(),
        );
        let overlay_data_listener: Arc<dyn CatchainOverlayListener + Send + Sync> =
            overlay_listener.clone();
        let overlay_replay_listener: Arc<dyn CatchainOverlayLogReplayListener + Send + Sync> =
            overlay_listener.clone();

        log::info!(
            "Receiver: starting up overlay for session {:x} with ID/incarnation {:x}, short_id {}",
            session_id,
            overlay_id,
            overlay_short_id
        );

        let transport_type = if options.allow_tcp_communication {
            consensus_common::OverlayTransportType::CatchainTcp
        } else {
            consensus_common::OverlayTransportType::Catchain
        };
        let overlay = overlay_manager.start_overlay(
            &local_key,
            &overlay_short_id,
            &ids,
            Arc::downgrade(&overlay_data_listener),
            Arc::downgrade(&overlay_replay_listener),
            transport_type,
            // catchain consensus does not run a block-sync overlay
            None,
        )?;

        //TODO: stop overlay in case of error

        //blocks initialization

        let root_block = ReceivedBlock::create_root(
            sources_count,
            &incarnation,
            &received_blocks_instance_counter,
        );

        log::trace!(
            "...creating root received block for receiver session incarnation {:?}",
            incarnation
        );

        let local_key_id_finder = || {
            let local_id = local_key.id();
            for (i, id) in ids.iter().enumerate() {
                if utils::get_public_key_hash(&id.public_key) == *local_id {
                    return (local_id, i);
                }
            }

            unreachable!("LocalID {:?} has not been found in catchain nodes", local_id);
        };
        let (local_id, local_idx) = local_key_id_finder();

        assert!(sources_count == 0 || local_idx != sources_count);

        log::debug!("Receiver local_idx={}, sources_count={}", local_idx, sources_count);

        let mut sources = Vec::new();
        let mut public_key_hash_to_source = HashMap::new();
        let mut adnl_id_to_source = HashMap::new();
        let mut source_public_key_hashes = Vec::new();

        for id in &ids {
            let source_id = sources.len();
            let source = crate::receiver_source::ReceiverSource::create(
                source_id,
                id.public_key.clone(),
                &id.adnl_id,
            );

            let public_key_hash = id.public_key.id().clone();

            sources.push(source.clone());
            source_public_key_hashes.push(public_key_hash.clone());
            public_key_hash_to_source.insert(public_key_hash.clone(), source.clone());
            adnl_id_to_source.insert(id.adnl_id.clone(), source.clone());
        }

        let now = SystemTime::now();
        let mut obj = ReceiverImpl {
            session_id,
            task_queues: task_queues.clone(),
            completion_handlers: CompletionHandlers::new(task_queues.clone()),
            sources,
            public_key_hash_to_source,
            adnl_id_to_source,
            source_public_key_hashes,
            incarnation: incarnation.clone(),
            options,
            blocks: HashMap::new(),
            blocks_to_run: VecDeque::new(),
            root_block: root_block.clone(),
            pending_blocks: VecDeque::new(),
            active_send: false,
            last_sent_block: root_block.clone(),
            _local_ids: ids,
            local_id: local_id.clone(),
            local_key: local_key.clone(),
            local_idx,
            total_forks: 0,
            neighbours: Vec::new(),
            listener,
            metrics_receiver,
            received_blocks_instance_counter,
            out_queries_counter,
            in_queries_counter,
            out_messages_counter,
            in_messages_counter,
            _in_broadcasts_counter: in_broadcasts_counter,
            pending_in_db: 0,
            db: None,
            read_db: false,
            allow_broadcasts,
            db_root_block: ZERO_HASH.clone(),
            db_suffix,
            allow_unsafe_self_blocks_resync,
            unsafe_root_block_writing: false,
            started: false,
            next_awake_time: now,
            next_sync_time: now,
            next_neighbours_rotate_time: now,
            initial_sync_complete_time: now
                + Duration::from_secs(INFINITE_INITIAL_SYNC_COMPLETE_TIME_SECS),
            rng: rand::thread_rng(),
            get_pending_deps_call_id: 0,
            intentional_fork: false,
            blame_processed: vec![false; sources_count],
            pending_fork_proofs: HashMap::new(),
            _overlay_id: overlay_id,
            overlay_short_id: overlay_short_id.clone(),
            overlay_manager,
            overlay,
            _overlay_listener: overlay_listener,
            in_bytes,
            out_bytes,
            in_queries_bytes,
            out_queries_bytes,
            _in_broadcasts_bytes: in_broadcasts_bytes,
            out_broadcasts_bytes,
            _in_messages_bytes: in_messages_bytes,
            out_messages_bytes,
        };

        obj.add_received_block(root_block.clone());
        obj.start_up_db(path)?;
        obj.choose_neighbours();

        Ok(obj)
    }

    fn create_block_with_payload(
        &mut self,
        block: &ton::Block,
        payload: BlockPayloadPtr,
    ) -> Result<ReceivedBlockPtr> {
        instrument!();

        if block.height == 0 {
            return Ok(self.root_block.clone());
        }

        let block_id = self.get_block_id(block, payload.data());
        let block_hash = utils::get_block_id_hash(&block_id);

        if let Some(existing_block_entry) = self.blocks.get(&block_hash) {
            let existing_block = existing_block_entry.clone();

            if !existing_block.borrow().is_initialized() {
                log::trace!(
                    "...create block with hash={:?} exists but has not been initialized",
                    block_hash
                );

                existing_block.borrow_mut().initialize(block, payload, self)?;
            }

            Ok(existing_block.clone())
        } else {
            log::trace!("...create block with hash={:?}", block_hash);

            let new_block = ReceivedBlock::create_with_payload(block, payload, self)?;

            self.add_received_block(new_block.clone());

            Ok(new_block)
        }
    }
}

/*
    Implementation of OverlayListener
*/

struct OverlayListenerImpl {
    session_id: SessionId,                        //session ID
    incarnation: SessionId,                       //incarnation
    task_queues: Arc<ReceiverTaskQueues>,         //task queues
    receiver_listener: ReceiverListenerPtr,       //receiver listener
    allow_broadcasts: Arc<AtomicBool>,            //flag to indicate if broadcasts are allowed
    in_queries_bytes: metrics::Counter,           //incoming queries traffic
    out_queries_bytes: metrics::Counter,          //outgoing queries traffic
    in_messages_bytes: metrics::Counter,          //incoming messages traffic
    in_broadcasts_bytes: metrics::Counter,        //incoming broadcasts traffic
    in_broadcasts_counter: metrics::Counter,      //incoming broadcasts counter
    in_bytes: metrics::Counter,                   //incoming traffic
    out_bytes: metrics::Counter,                  //outgoing traffic
    main_thread_overloaded_flag: Arc<AtomicBool>, //main thread overloaded flag
}

impl CatchainOverlayLogReplayListener for OverlayListenerImpl {
    fn on_time_changed(&self, timestamp: std::time::SystemTime) {
        self.task_queues.post_processing_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            if let Some(listener) = receiver.listener.upgrade() {
                listener.on_set_time(timestamp);
            }
        }));
    }
}

impl CatchainOverlayListener for OverlayListenerImpl {
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr) {
        instrument!();

        self.in_messages_bytes.increment(data.data().len() as u64);
        self.in_bytes.increment(data.data().len() as u64);

        let adnl_id = adnl_id.clone();
        let data = data.clone();

        self.task_queues.post_processing_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            let bytes = &mut data.data().as_slice();

            if log::log_enabled!(log::Level::Debug) {
                let elapsed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_else(|_| Duration::new(0, 0))
                    .as_millis();
                log::trace!(
                    "Receive message from overlay for source: \
                    size={}, payload={}, source={}, session_id={:x}, timestamp={:?}",
                    bytes.len(),
                    &hex::encode(&bytes),
                    &hex::encode(adnl_id.data()),
                    receiver.incarnation,
                    elapsed
                );
            }

            receiver.receive_message_from_overlay(adnl_id, data);
        }));
    }

    fn on_broadcast(
        &self,
        source_key_hash: PublicKeyHash,
        data: &BlockPayloadPtr,
        _source: consensus_common::BroadcastSource,
    ) {
        instrument!();
        // `_source` ignored; the block-sync overlay is simplex-only
        if !self.allow_broadcasts.load(Ordering::SeqCst) {
            log::debug!(
                "Skip broadcast from overlay for source: {}",
                &hex::encode(source_key_hash.data())
            );
            return;
        }

        self.in_broadcasts_bytes.increment(data.data().len() as u64);
        self.in_broadcasts_counter.increment(1);
        self.in_bytes.increment(data.data().len() as u64);

        if log::log_enabled!(log::Level::Debug) {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::new(0, 0))
                .as_millis();
            log::trace!(
                "Receive broadcast from overlay for source: \
                size={}, payload={}, source={}, session_id={}, timestamp={:?}",
                data.data().len(),
                &hex::encode(data.data()),
                &hex::encode(source_key_hash.data()),
                self.incarnation.to_hex_string(),
                elapsed
            );
        }

        if let Some(listener) = self.receiver_listener.upgrade() {
            listener.on_broadcast(source_key_hash.clone(), data.clone());
        }

        self.task_queues.post_processing_closure(Box::new(move |receiver: &mut ReceiverImpl| {
            if let Some(ref source) = receiver.get_source_by_hash(&source_key_hash) {
                source.borrow_mut().get_mut_statistics().in_broadcasts_count += 1;
            }
        }));
    }

    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        _response_callback: QueryResponseCallback,
    ) {
        instrument!();

        self.in_queries_bytes.increment(data.data().len() as u64);
        self.in_bytes.increment(data.data().len() as u64);

        let out_queries_bytes = self.out_queries_bytes.clone();
        let out_bytes = self.out_bytes.clone();

        let response_callback: Box<dyn FnOnce(Result<BlockPayloadPtr>) + Send> =
            Box::new(move |result: Result<BlockPayloadPtr>| {
                if let Ok(payload) = &result {
                    out_queries_bytes.increment(payload.data().len() as u64);
                    out_bytes.increment(payload.data().len() as u64);
                }
                _response_callback(result);
            });

        if !self.main_thread_overloaded_flag() {
            let adnl_id = adnl_id.clone();
            let data = data.clone();

            self.task_queues.post_processing_closure(Box::new(
                move |receiver: &mut ReceiverImpl| {
                    receiver.receive_query_from_overlay(adnl_id, data, response_callback);
                },
            ));

            return;
        }

        let warning = error!(
            "Catchain {:x} is overloaded. Skip query from ADNL ID {}",
            self.session_id, adnl_id
        );

        log::warn!("{}", warning);

        let response = match deserialize_boxed(data.data()) {
            Ok(message) => {
                if message.is::<ton::GetDifferenceRequest>() {
                    let message = &message.downcast::<ton::GetDifferenceRequest>().unwrap();

                    utils::serialize_query_boxed_response(Ok(
                        ::ton_api::ton::catchain::difference::Difference {
                            sent_upto: message.rt.clone(),
                        }
                        .into_boxed(),
                    ))
                } else if message.is::<ton::GetBlockRequest>() {
                    utils::serialize_query_boxed_response(Ok(
                        ::ton_api::ton::catchain::BlockResult::Catchain_BlockNotFound {},
                    ))
                } else {
                    Err(warning)
                }
            }
            Err(err) => Err(err),
        };

        response_callback(response);
    }
}

impl OverlayListenerImpl {
    fn create(
        session_id: SessionId,
        incarnation: SessionId,
        task_queues: Arc<ReceiverTaskQueues>,
        receiver_listener: ReceiverListenerPtr,
        main_thread_overloaded_flag: Arc<AtomicBool>,
        allow_broadcasts: Arc<AtomicBool>,
        in_bytes: metrics::Counter,
        out_bytes: metrics::Counter,
        in_messages_bytes: metrics::Counter,
        in_broadcasts_bytes: metrics::Counter,
        in_broadcasts_counter: metrics::Counter,
        in_queries_bytes: metrics::Counter,
        out_queries_bytes: metrics::Counter,
    ) -> Arc<OverlayListenerImpl> {
        Arc::new(Self {
            session_id,
            incarnation,
            task_queues,
            receiver_listener,
            in_bytes,
            out_bytes,
            in_messages_bytes,
            in_broadcasts_bytes,
            in_broadcasts_counter,
            in_queries_bytes,
            out_queries_bytes,
            main_thread_overloaded_flag,
            allow_broadcasts,
        })
    }

    fn main_thread_overloaded_flag(&self) -> bool {
        self.main_thread_overloaded_flag.load(Ordering::SeqCst)
    }
}

impl Drop for ReceiverImpl {
    fn drop(&mut self) {
        log::info!("Dropping ReceiverImpl for session {}", self.session_id.to_hex_string());

        // Stop the receiver
        self.stop();

        // Log final statistics
        log::info!(
            "ReceiverImpl final stats for session {}: blocks={}, sources={}, total_forks={}",
            self.session_id.to_hex_string(),
            self.blocks.len(),
            self.sources.len(),
            self.total_forks
        );

        log::info!("Dropped ReceiverImpl for session {}", self.session_id.to_hex_string());
    }
}

impl Drop for OverlayListenerImpl {
    fn drop(&mut self) {
        log::info!("Dropped OverlayListenerImpl for session {}", self.session_id.to_hex_string());
    }
}
