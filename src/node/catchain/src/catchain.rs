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
    profiling::Profiler,
    utils::{self, get_elapsed_time, MetricsDumper, MetricsHandle},
    ActivityNodePtr, Block, BlockExtraId, BlockHash, BlockHeight, BlockPayloadPtr, BlockPtr,
    Catchain, CatchainFactory, CatchainListenerPtr, CatchainNode, CatchainOverlayManagerPtr,
    CatchainPtr, Options, PrivateKey, PublicKeyHash, QueryResponseCallback, ReceiverListener,
    ReceiverPtr, SessionId,
};
use rand::Rng;
use std::{
    cell::RefCell,
    collections::{HashMap, LinkedList},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use ton_block::{Result, UInt256};

/*
    Constants
*/

const CATCHAIN_PROCESSING_PERIOD_MS: u64 = 1000; //idle time for catchain timed events in milliseconds
const CATCHAIN_METRICS_DUMP_PERIOD_MS: u64 = 20000; //time for catchain metrics dump
const CATCHAIN_PROFILING_DUMP_PERIOD_MS: u64 = 30000; //time for catchain profiling dump
const CATCHAIN_INFINITE_SEND_PROCESS_TIMEOUT: Duration = Duration::from_secs(3600 * 24 * 3650); //large timeout as a infinite timeout simulation for send process
const CATCHAIN_WARN_PROCESSING_LATENCY: Duration = Duration::from_millis(3000); //max processing latency
const CATCHAIN_LATENCY_WARN_DUMP_PERIOD: Duration = Duration::from_millis(2000); //latency warning dump period
const BLOCKS_PROCESSING_STACK_CAPACITY: usize = 1000; //number of blocks in stack for CatchainProcessor::set_processed method
const MAIN_LOOP_NAME: &str = "CC"; //catchain main loop short thread name
const CATCHAIN_MAIN_LOOP_THREAD_STACK_SIZE: usize = 1024 * 1024 * 32; //stack size for catchain main loop thread
const LOG_TARGET_PROFILING: &str = "catchain_profiling"; //log target for profiling

/*
    Options
*/

impl Default for Options {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_millis(16000),
            max_deps: 4,
            max_serialized_block_size: 16 * 1024,
            max_block_height_coeff: 0,
            block_hash_covers_data: false,
            debug_disable_db: false,
            skip_processed_blocks: false,
            receiver_max_neighbours_count: 5,
            receiver_neighbours_sync_min_period: Duration::from_millis(100),
            receiver_neighbours_sync_max_period: Duration::from_millis(200),
            receiver_max_sources_sync_attempts: 3,
            receiver_neighbours_rotate_min_period: Duration::from_millis(60000),
            receiver_neighbours_rotate_max_period: Duration::from_millis(120000),
            disable_gossip: false,
            allow_tcp_communication: false,
        }
    }
}

/*
    Special blocks storage to control blocks destrocution order
    (to avoid deep recursion on destruction of long chains of blocks)
*/

struct BlocksStorage {
    storage: LinkedList<BlockPtr>,
}

impl BlocksStorage {
    fn add(&mut self, block: BlockPtr) {
        self.storage.push_back(block);
    }

    fn new() -> Self {
        Self { storage: LinkedList::new() }
    }
}

impl Drop for BlocksStorage {
    fn drop(&mut self) {
        log::trace!("...removing blocks from catchain");

        while !self.storage.is_empty() {
            self.storage.pop_back();
        }

        log::trace!("...catchain blocks have been destroyed");
    }
}

/*
    Catchain processor (for use in a separate thread)
*/

#[derive(Debug)]
struct BlockDesc {
    preprocessed: bool, //has this block been preprocessing in a Catchain iteration
    processed: bool,    //has this block been processing in a Catchain iteration
}

type BlockDescPtr = Rc<RefCell<BlockDesc>>;

struct CatchainProcessor {
    _task_queues: CatchainTaskQueuesPtr,    //task queues for catchain
    receiver: ReceiverPtr,                  //catchain receiver
    catchain_listener: CatchainListenerPtr, //listener for outgoing events
    options: Options,                       //catchain options
    receiver_started: bool,                 //flag which indicates that receiver is started
    next_block_generation_time: SystemTime, //time to generate next block
    blocks: HashMap<BlockHash, BlockPtr>,   //all catchain blocks
    block_descs: HashMap<BlockHash, BlockDescPtr>, //all catchain blocks descriptions (internal processor structures)
    top_blocks: HashMap<BlockHash, BlockPtr>,      //map of top blocks by hash
    top_source_blocks: Vec<Option<BlockPtr>>,      //list of top blocks for each source
    sources: Vec<PublicKeyHash>,                   //list of validator public key hashes
    blamed_sources: Vec<bool>,                     //mask if a sources is blamed
    process_deps: Vec<BlockHash>, //list of block hashes which were used as dependencies for next consensus iteration
    processing_blocks_stack: Vec<(BlockPtr, BlockDescPtr)>, //array of block desc which are being processed
    processing_blocks_stack_tmp: Vec<(BlockPtr, BlockDescPtr)>, //temporary array of block desc which are being processed
    session_id: SessionId,                                      //catchain session ID
    _ids: Vec<CatchainNode>,                                    //list of nodes
    _local_id: PublicKeyHash,       //public key hash of current validator
    local_idx: usize,               //index of current validator in the list of sources
    _local_adnl_id: PublicKeyHash,  //ADNL ID of current validator
    force_process: bool, //flag which indicates catchain was requested (by validator session) to generate new block
    active_process: bool, //flag which indicates catchain is in process of generation of a new block
    rng: rand::rngs::ThreadRng, //random generator
    current_extra_id: BlockExtraId, //current block extra identifier
    current_time: Option<std::time::SystemTime>, //current time for log replaying
    process_blocks_requests_counter: metrics::Counter, //counter for send_process
    process_blocks_responses_counter: metrics::Counter, //counter for processed_block
    process_blocks_skip_responses_counter: metrics::Counter, //counter for processed_block with may_be_skipped=true
    process_blocks_batching_requests_counter: metrics::Counter, //counter for processed_block with batching request
    processed_block_payload_size_histogram: metrics::Histogram, //histogram for processed blocks sizes
    blocks_storage: BlocksStorage, //must be last field to force drop order;
                                   //storage for catchain blocks;
                                   //is needed only to avoid stack overflow on dropping very long chains of blocks during destruction
}

/*
    Catchain task queues
*/

struct TaskDesc<F: ?Sized> {
    task: Box<F>,                         //closure for execution
    creation_time: std::time::SystemTime, //time of task creation
}

struct TaskQueue<F: ?Sized> {
    sender: crossbeam::channel::Sender<TaskDesc<F>>,
    receiver: crossbeam::channel::Receiver<TaskDesc<F>>,
    post_counter: metrics::Counter, //counter for queue posts
    pull_counter: metrics::Counter, //counter for queue pull
}

impl<F: ?Sized> TaskQueue<F> {
    fn new(metrics_receiver: MetricsHandle, prefix: &str) -> Self {
        let (sender, receiver) = crossbeam::channel::unbounded();
        Self {
            sender,
            receiver,
            post_counter: metrics_receiver
                .sink()
                .register_counter(&format!("{}.posts", prefix).into()),
            pull_counter: metrics_receiver
                .sink()
                .register_counter(&format!("{}.pulls", prefix).into()),
        }
    }

    fn post_closure(&self, task_fn: Box<F>) {
        let task_desc =
            TaskDesc::<F> { task: task_fn, creation_time: std::time::SystemTime::now() };
        self.post_counter.increment(1);
        let _ = self.sender.send(task_desc);
    }

    fn pull_closure(&self, timeout: std::time::Duration) -> Option<TaskDesc<F>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(task_desc) => {
                self.pull_counter.increment(1);
                Some(task_desc)
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                // Timeout elapsed - no items in queue
                None
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                // Channel disconnected - sleep before returning
                const DISCONNECTED_SLEEP: std::time::Duration =
                    std::time::Duration::from_millis(100);
                log::warn!(
                    "Catchain task queue disconnected. Waiting for {}ms before returning.",
                    DISCONNECTED_SLEEP.as_millis()
                );
                std::thread::sleep(DISCONNECTED_SLEEP);

                None
            }
        }
    }
}

struct CatchainTaskQueues {
    main_queue: TaskQueue<dyn FnOnce(&mut CatchainProcessor) + Send>,
}

impl CatchainTaskQueues {
    fn new(metrics_receiver: MetricsHandle) -> Self {
        Self { main_queue: TaskQueue::new(metrics_receiver.clone(), "main_queue") }
    }

    fn post_main_queue_closure(
        &self,
        task_fn: impl FnOnce(&mut CatchainProcessor) + Send + 'static,
    ) {
        self.main_queue.post_closure(Box::new(task_fn));
    }

    #[allow(dead_code)]
    fn pull_main_queue_closure(
        &self,
        timeout: std::time::Duration,
    ) -> Option<TaskDesc<dyn FnOnce(&mut CatchainProcessor) + Send>> {
        self.main_queue.pull_closure(timeout)
    }
}

type CatchainTaskQueuesPtr = Arc<CatchainTaskQueues>;

/*
    Implementation details for Receiver
*/

pub(crate) struct CatchainImpl {
    task_queues: CatchainTaskQueuesPtr, //task queues for catchain
    should_stop_flag: Arc<AtomicBool>, //atomic flag to indicate that Catchain thread should be stopped
    main_thread_is_stopped_flag: Arc<AtomicBool>, //atomic flag to indicate that Catchain thread has been stopped
    _main_thread_overloaded_flag: Arc<AtomicBool>, //indicates that catchain main thrad is overloaded
    destroy_db_flag: Arc<AtomicBool>,              //indicates catchain has to destroy DB
    session_id: SessionId,                         //session ID
    _activity_node: ActivityNodePtr, //activity node for tracing lifetime of this catchain
    receiver: ReceiverPtr,           //receiver for catchain
    _receiver_listener: Arc<dyn ReceiverListener + Send + Sync>, //receiver listener
}

/*
    Implementation of ReceiverListener
*/

struct ReceiverListenerImpl {
    task_queues: CatchainTaskQueuesPtr, //task queues for catchain
}

impl ReceiverListener for ReceiverListenerImpl {
    /*
        Set time callback
    */

    fn on_set_time(&self, time: std::time::SystemTime) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_time_changed(time);
        });
    }

    /*
        Catchain started callback
    */

    fn on_started(&self) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_started();
        });
    }

    /*
        Blocks management
    */

    fn on_new_block(
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
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_new_block(
                source_id,
                fork_id,
                hash,
                height,
                prev,
                deps,
                forks_dep_heights,
                payload,
            );
        });
    }

    fn on_broadcast(&self, source_key_hash: PublicKeyHash, data: BlockPayloadPtr) {
        //TODO: call processor directly instead of posting closure
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_broadcast_from_receiver(source_key_hash, data);
        });
    }

    /*
        Nodes blaming management
    */

    fn on_blame(&self, source_id: usize) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_blame(source_id);
        });
    }

    /*
        Network messages transfering from Receiver to Validator Session
    */

    fn on_custom_query(
        &self,
        source_public_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        //TODO: call processor directly instead of posting closure
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.on_custom_query(source_public_key_hash, data, response_callback);
        });
    }
}

impl ReceiverListenerImpl {
    /*
        Listener creation
    */

    fn create(task_queues: CatchainTaskQueuesPtr) -> Arc<dyn ReceiverListener + Send + Sync> {
        Arc::new(ReceiverListenerImpl { task_queues })
    }
}

/*
    Implementation of CatchainProcessor
*/

impl CatchainProcessor {
    /*
        Stopping
    */

    fn stop(&mut self) {
        log::debug!("Stopping CatchainProcessor...");

        self.receiver.stop();

        log::debug!("CatchainProcessor has been stopped");
    }

    fn destroy_db(&mut self) {
        log::debug!("Destroying Catchain DB...");

        self.receiver.destroy_db();
    }

    /*
        Blocks management
    */

    fn get_block(&self, hash: BlockHash) -> Option<BlockPtr> {
        if self.blocks.contains_key(&hash) {
            Some(self.blocks[&hash].clone())
        } else {
            None
        }
    }

    fn get_block_desc(&self, block: &dyn Block) -> Option<BlockDescPtr> {
        let hash = &block.get_hash();

        if self.block_descs.contains_key(hash) {
            Some(self.block_descs[hash].clone())
        } else {
            None
        }
    }

    fn is_processed(&self, block: &dyn Block) -> bool {
        match self.get_block_desc(block) {
            Some(desc) => desc.borrow().processed,
            _ => false,
        }
    }

    /*
        Listener notifications
    */

    fn notify_preprocess_block(&mut self, block: BlockPtr) {
        check_execution_time!(10000);

        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.preprocess_block(block.clone());
        }
    }

    fn notify_process_blocks(&mut self, blocks: Vec<BlockPtr>) {
        check_execution_time!(10000);

        if let Some(listener) = self.catchain_listener.upgrade() {
            self.process_blocks_requests_counter.increment(1);

            listener.process_blocks(blocks);
        }
    }

    fn notify_finished_processing(&mut self) {
        check_execution_time!(10000);

        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.finished_processing();
        }
    }

    fn notify_started(&mut self) {
        check_execution_time!(10000);

        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.started();
        }
    }

    fn notify_custom_query(
        &mut self,
        source_public_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.process_query(source_public_key_hash, data, response_callback);
        }
    }

    fn notify_broadcast(&mut self, source_public_key_hash: PublicKeyHash, data: BlockPayloadPtr) {
        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.process_broadcast(source_public_key_hash, data);
        }
    }

    fn notify_set_time(&mut self, time: std::time::SystemTime) {
        if let Some(listener) = self.catchain_listener.upgrade() {
            listener.set_time(time);
        }
    }

    /*
        Main loop
    */

    pub(self) fn main_loop(
        task_queues: CatchainTaskQueuesPtr,
        should_stop_flag: Arc<AtomicBool>,
        is_stopped_flag: Arc<AtomicBool>,
        overloaded_flag: Arc<AtomicBool>,
        destroy_db_flag: Arc<AtomicBool>,
        options: Options,
        session_id: SessionId,
        ids: Vec<CatchainNode>,
        local_key: PrivateKey,
        listener: CatchainListenerPtr,
        catchain_activity_node: ActivityNodePtr,
        metrics_receiver: MetricsHandle,
        receiver: ReceiverPtr,
    ) {
        log::info!("Catchain main loop is started (session_id is {:x})", session_id);

        //configure metrics

        let loop_counter =
            metrics_receiver.sink().register_counter(&"catchain_main_loop_iterations".into());
        let loop_overloads_counter =
            metrics_receiver.sink().register_counter(&"catchain_main_loop_overloads".into());
        let main_thread_pull_counter = task_queues.main_queue.pull_counter.clone();

        //create catchain processor

        let processor_opt = CatchainProcessor::create(
            task_queues.clone(),
            options,
            session_id.clone(),
            ids,
            local_key,
            listener,
            metrics_receiver.clone(),
            receiver,
        );

        let mut processor = match processor_opt {
            Ok(processor) => processor,
            Err(err) => {
                log::error!(
                    "CatchainProcessor::main_loop: error during creation of CatchainProcessor: {:?}",
                    err
                );
                overloaded_flag.store(false, Ordering::SeqCst);
                is_stopped_flag.store(true, Ordering::SeqCst);
                return;
            }
        };

        //configure metrics dumper

        let mut metrics_dumper = MetricsDumper::new();

        metrics_dumper.add_compute_handler("received_blocks", utils::compute_instance_counter);
        metrics_dumper.add_derivative_metric("received_blocks");
        metrics_dumper.add_derivative_metric("receiver_out_messages");
        metrics_dumper.add_derivative_metric("receiver_in_messages");
        metrics_dumper.add_derivative_metric("receiver_out_queries.total");
        metrics_dumper.add_derivative_metric("receiver_in_queries.total");
        metrics_dumper.add_derivative_metric("receiver_in_broadcasts");
        metrics_dumper.add_derivative_metric("db_get_txs");
        metrics_dumper.add_derivative_metric("db_put_txs");
        metrics_dumper.add_derivative_metric("catchain_main_loop_iterations");
        metrics_dumper.add_derivative_metric("catchain_main_loop_overloads");

        metrics_dumper.add_derivative_metric("process_blocks_requests");
        metrics_dumper.add_derivative_metric("process_blocks_responses");
        metrics_dumper.add_derivative_metric("process_blocks_skip_responses");
        metrics_dumper.add_derivative_metric("process_blocks_batching_requests");

        metrics_dumper.add_derivative_metric("overlay_in_bytes");
        metrics_dumper.add_derivative_metric("overlay_out_bytes");
        metrics_dumper.add_derivative_metric("overlay_in_messages_bytes");
        metrics_dumper.add_derivative_metric("overlay_out_messages_bytes");
        metrics_dumper.add_derivative_metric("overlay_in_queries_bytes");
        metrics_dumper.add_derivative_metric("overlay_out_queries_bytes");
        metrics_dumper.add_derivative_metric("overlay_in_broadcasts_bytes");
        metrics_dumper.add_derivative_metric("overlay_out_broadcasts_bytes");

        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "overlay_in_messages.avg_size",
            "overlay_in_messages_bytes",
            "receiver_in_messages",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "overlay_out_messages.avg_size",
            "overlay_out_messages_bytes",
            "receiver_out_messages",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "overlay_in_queries.avg_size",
            "overlay_in_queries_bytes",
            "receiver_in_queries.total",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "overlay_out_queries.avg_size",
            "overlay_out_queries_bytes",
            "receiver_out_queries.total",
            0.0,
        );
        utils::add_compute_relative_metric(
            &mut metrics_dumper,
            "overlay_in_broadcasts.avg_size",
            "overlay_in_broadcasts_bytes",
            "receiver_in_broadcasts",
            0.0,
        );

        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "process_blocks_skipping",
            "process_blocks_skip_responses",
            "process_blocks_responses",
            0.0,
        );
        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "process_blocks_batching",
            "process_blocks_batching_requests",
            "process_blocks_responses",
            0.0,
        );

        utils::add_compute_percentage_metric(
            &mut metrics_dumper,
            "catchain_main_loop_load",
            "catchain_main_loop_overloads",
            "catchain_main_loop_iterations",
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

        metrics_dumper.add_derivative_metric("main_queue.posts");
        metrics_dumper.add_derivative_metric("main_queue.pulls");
        metrics_dumper.add_compute_handler("main_queue", utils::compute_queue_size_counter);

        //start main loop

        let mut next_metrics_dump_time =
            SystemTime::now() + Duration::from_millis(CATCHAIN_METRICS_DUMP_PERIOD_MS);
        let mut next_profiling_dump_time =
            SystemTime::now() + Duration::from_millis(CATCHAIN_PROFILING_DUMP_PERIOD_MS);
        let mut last_latency_warn_dump_time = SystemTime::now();
        let mut is_overloaded = false;

        loop {
            {
                instrument!();

                catchain_activity_node.tick();
                loop_counter.increment(1);

                //check if the main loop should be stopped

                if should_stop_flag.load(Ordering::SeqCst) {
                    if destroy_db_flag.load(Ordering::SeqCst) {
                        processor.destroy_db();
                    }

                    processor.stop();
                    break;
                }

                //check overload flag

                overloaded_flag.store(is_overloaded, Ordering::SeqCst);

                if is_overloaded {
                    loop_overloads_counter.increment(1);
                }

                //handle catchain event with timeout

                let next_awake_time =
                    SystemTime::now() + Duration::from_millis(CATCHAIN_PROCESSING_PERIOD_MS);
                let next_awake_time = if processor.receiver_started {
                    std::cmp::min(next_awake_time, processor.next_block_generation_time)
                } else {
                    next_awake_time
                };
                let next_awake_time = std::cmp::min(next_awake_time, next_metrics_dump_time);
                let next_awake_time = std::cmp::min(next_awake_time, next_profiling_dump_time);

                let timeout = next_awake_time.duration_since(SystemTime::now()).unwrap_or_default();

                let task_desc = {
                    instrument!();

                    task_queues.main_queue.pull_closure(timeout)
                };

                is_overloaded = false;

                if let Some(task_desc) = task_desc {
                    main_thread_pull_counter.increment(1);

                    let processing_latency = get_elapsed_time(&task_desc.creation_time);
                    if processing_latency > CATCHAIN_WARN_PROCESSING_LATENCY {
                        is_overloaded = true;

                        if get_elapsed_time(&last_latency_warn_dump_time)
                            > CATCHAIN_LATENCY_WARN_DUMP_PERIOD
                        {
                            log::warn!(
                                "Catchain processing latency is {:.3}s \
                                (expected max latency is {:.3}s)",
                                processing_latency.as_secs_f64(),
                                CATCHAIN_WARN_PROCESSING_LATENCY.as_secs_f64()
                            );
                            last_latency_warn_dump_time = SystemTime::now();
                        }
                    }

                    instrument!();
                    check_execution_time!(100000);

                    let task = task_desc.task;

                    task(&mut processor);
                }

                //generate new block

                if processor.receiver_started {
                    if let Ok(_elapsed) = processor.next_block_generation_time.elapsed() {
                        //initiate new block generation if there are no active top blocks

                        processor.reset_next_block_generation_time(
                            CATCHAIN_INFINITE_SEND_PROCESS_TIMEOUT,
                        );
                        processor.send_process_attempt();
                    } else if let Ok(delay) =
                        processor.next_block_generation_time.duration_since(SystemTime::now())
                    {
                        log::trace!(
                            "Waiting for {:.3}s for a new block generation time slot",
                            delay.as_secs_f64()
                        );
                    }
                }
            }

            //dump metrics

            if next_metrics_dump_time.elapsed().is_ok() {
                instrument!();
                check_execution_time!(5_000);

                if log::log_enabled!(log::Level::Debug) {
                    metrics_dumper.update(&metrics_receiver);

                    let session_id_str = processor.session_id.to_hex_string();

                    log::debug!("Catchain {:x} metrics:", processor.session_id);

                    metrics_dumper.dump(|string| log::debug!("{}{}", session_id_str, string));
                }

                next_metrics_dump_time =
                    SystemTime::now() + Duration::from_millis(CATCHAIN_METRICS_DUMP_PERIOD_MS);
            }

            //dump profiling

            if next_profiling_dump_time.elapsed().is_ok() {
                instrument!();
                check_execution_time!(5_000);

                if log::log_enabled!(target: LOG_TARGET_PROFILING, log::Level::Debug) {
                    let profiling_dump =
                        Profiler::local_instance().with(|profiler| profiler.borrow().dump());

                    log::debug!(
                        target: LOG_TARGET_PROFILING,
                        "Catchain {:x} profiling: {}",
                        processor.session_id,
                        profiling_dump
                    );
                }

                next_profiling_dump_time =
                    SystemTime::now() + Duration::from_millis(CATCHAIN_PROFILING_DUMP_PERIOD_MS);
            }
        }

        //waiting for receiver to stop

        processor.receiver.stop();

        drop(task_queues);

        log::info!("Catchain main loop is finished (session_id is {:x})", session_id);

        overloaded_flag.store(false, Ordering::SeqCst);
        is_stopped_flag.store(true, Ordering::SeqCst);
    }

    /*
        New block generation flow
    */

    fn recursive_blocks_update<Pred, Update>(
        &mut self,
        mut block: BlockPtr,
        pred: Pred,
        update: Update,
    ) where
        Pred: Fn(&BlockDescPtr) -> bool,
        Update: Fn(&mut CatchainProcessor, &BlockDescPtr, BlockPtr),
    {
        let mut block_desc = self.get_block_desc(&*block).unwrap();

        loop {
            if !pred(&block_desc) {
                //recursive processing of block dependencies

                self.processing_blocks_stack_tmp.clear();

                let mut has_unprocessed_deps = false;

                for block in block.get_deps().iter().rev() {
                    let block_desc = self.get_block_desc(&**block).unwrap();

                    if pred(&block_desc) {
                        continue;
                    }

                    self.processing_blocks_stack_tmp.push((block.clone(), block_desc.clone()));

                    has_unprocessed_deps = true;
                }

                if let Some(block) = block.get_prev() {
                    let block_desc = self.get_block_desc(&*block).unwrap();

                    if !pred(&block_desc) {
                        self.processing_blocks_stack_tmp.push((block.clone(), block_desc.clone()));

                        has_unprocessed_deps = true;
                    }
                }

                if has_unprocessed_deps {
                    self.processing_blocks_stack.push((block.clone(), block_desc.clone()));
                    self.processing_blocks_stack.append(&mut self.processing_blocks_stack_tmp);
                } else {
                    update(self, &block_desc, block);
                }
            }

            //trying to move to the next block

            let next = self.processing_blocks_stack.pop();

            if next.is_none() {
                //if all blocks have been processed, stop the loop

                break;
            }

            let (next_block, next_block_desc) = next.unwrap();

            //if we have next block in stack for processing - move to it

            block = next_block;
            block_desc = next_block_desc;
        }
    }

    fn send_preprocess(&mut self, block: BlockPtr, is_root: bool) {
        instrument!();

        if log::log_enabled!(log::Level::Trace)
            && is_root
            && !self.get_block_desc(&*block).unwrap().borrow().preprocessed
        {
            log::trace!("CatchainProcessor::send_preprocess for block {}", block);
        }

        self.recursive_blocks_update(
            block,
            |block_desc| block_desc.borrow().preprocessed,
            |processor, block_desc, block| {
                block_desc.borrow_mut().preprocessed = true;

                //notify listeners

                log::trace!(
                    "...start preprocessing block {:?} from source {}",
                    block.get_hash(),
                    block.get_source_id()
                );

                processor.notify_preprocess_block(block.clone());

                log::trace!(
                    "...finish preprocessing block {:?} from source {}",
                    block.get_hash(),
                    block.get_source_id()
                );
            },
        );
    }

    fn remove_random_top_block(&mut self) -> BlockPtr {
        instrument!();
        check_execution_time!(100);

        let random_value = self.rng.gen_range(0..self.top_blocks.len());
        let hash = self.top_blocks.keys().nth(random_value).unwrap().clone();
        self.top_blocks.remove(&hash).unwrap()
    }

    fn set_processed(&mut self, block: BlockPtr) {
        instrument!();

        self.recursive_blocks_update(
            block,
            |block_desc| block_desc.borrow().processed,
            |_processor, block_desc, _block| block_desc.borrow_mut().processed = true,
        );
    }

    fn send_process(&mut self) {
        instrument!();

        assert!(self.receiver_started);

        log::trace!("Send blocks processing...");

        let mut blocks: Vec<BlockPtr> = Vec::new();
        let mut block_hashes: Vec<BlockHash> = Vec::new();

        log::trace!("...{} top blocks found", self.top_blocks.len());

        while !self.top_blocks.is_empty() && blocks.len() < self.options.max_deps as usize {
            let block = self.remove_random_top_block();
            let source_id = block.get_source_id();

            assert!(source_id < self.sources.len() as u32);

            if !self.blamed_sources[source_id as usize] {
                log::trace!(
                    "...choose block {:?} from source #{} pubkeyhash={}",
                    block.get_hash(),
                    block.get_source_id(),
                    block.get_source_public_key_hash()
                );

                block_hashes.push(block.get_hash().clone());
                blocks.push(block.clone());

                // Potential risk:
                // - The block is marked as processed.
                // - A blame event occurs.
                // - `send_process` finishes with `processed_block` and `may_be_skipped = true`, causing the block to return to the top.
                // - As a result, the block will not be cleared during blame clearance.
                //
                // However, this is not expected to have any significant impact on the protocol.
                self.set_processed(block);
            }
        }

        self.process_deps = block_hashes;

        log::trace!("...creating block for deps: {:?}", self.process_deps);

        self.notify_process_blocks(blocks);

        log::trace!("...finish creating block");
    }

    fn reset_next_block_generation_time(&mut self, timeout: Duration) {
        self.next_block_generation_time = SystemTime::now() + timeout;
    }

    fn update_next_block_generation_time(&mut self, time: SystemTime) {
        if time > self.next_block_generation_time {
            return;
        }

        self.next_block_generation_time = time;
    }

    fn send_process_attempt(&mut self) {
        instrument!();

        if self.active_process {
            return;
        }

        self.active_process = true;

        self.send_process();
    }

    fn request_new_block(&mut self, time: SystemTime) {
        if !self.receiver_started {
            return;
        }

        if !self.force_process || !self.active_process {
            log::debug!("Catchain forcing creation of a new block");
        }

        if self.active_process {
            self.force_process = true;
        } else {
            self.update_next_block_generation_time(time);
        }
    }

    fn processed_block(&mut self, payload: BlockPayloadPtr, may_be_skipped: bool) {
        instrument!();

        self.process_blocks_responses_counter.increment(1);
        self.processed_block_payload_size_histogram.record(payload.data().len() as f64);

        assert!(self.receiver_started);

        if !may_be_skipped {
            log::trace!(
                "Catchain created block: deps={:?}, payload size is {}",
                self.process_deps,
                payload.data().len()
            );

            if !self.options.skip_processed_blocks {
                self.receiver.add_block(payload, self.process_deps.drain(..).collect());
            }
        } else {
            log::trace!("Catchain created skip-block: deps={:?}", self.process_deps);
        }

        self.process_deps.clear();

        assert!(self.active_process);

        log::trace!("Catchain top blocks: {}", self.top_blocks.len());

        if may_be_skipped {
            self.process_blocks_skip_responses_counter.increment(1);
        }

        let continue_processing = self.force_process || !self.top_blocks.is_empty();

        if continue_processing {
            self.force_process = false;

            self.send_process();
        } else {
            self.active_process = false;

            self.process_blocks_batching_requests_counter.increment(1);

            log::debug!("...catchain finish processing");

            self.notify_finished_processing();

            //force set next block generation time and ignore all earlier wakeups

            self.reset_next_block_generation_time(self.options.idle_timeout);
        }
    }

    /*
        Events processing Overlay -> Catchain
    */

    fn on_broadcast_from_receiver(
        &mut self,
        source_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
    ) {
        instrument!();

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "Receive broadcast from overlay for source {}: {:?}",
                source_key_hash,
                data
            );
        }

        self.notify_broadcast(source_key_hash, data);
    }

    fn on_time_changed(&mut self, time: std::time::SystemTime) {
        self.current_time = Some(time);

        self.notify_set_time(time);
    }

    /*
        Start up callback from Receiver
    */

    fn on_started(&mut self) {
        log::debug!("Catchain has been successfully started");

        //notify about start of catchain

        self.notify_started();

        self.receiver_started = true;

        //initiate blocks processing

        assert!(!self.active_process);

        self.send_process_attempt();
    }

    /*
        Blocks processing
    */

    fn generate_extra_id(&mut self) -> BlockExtraId {
        const MAX_EXTRA_ID: BlockExtraId = u64::MAX;

        assert!(self.current_extra_id < MAX_EXTRA_ID);

        let result = self.current_extra_id;

        self.current_extra_id += 1;

        result
    }

    pub fn on_new_block(
        &mut self,
        source_id: usize,
        fork_id: usize,
        hash: BlockHash,
        height: BlockHeight,
        prev: BlockHash,
        deps: Vec<BlockHash>,
        forks_dep_heights: Vec<BlockHeight>,
        payload: BlockPayloadPtr,
    ) {
        instrument!();

        log::trace!(
            "New catchain block {:x} (source_id={}, fork={}, height={})",
            hash,
            source_id,
            fork_id,
            height
        );

        if self.top_blocks.is_empty() && !self.active_process && self.receiver_started {
            self.update_next_block_generation_time(SystemTime::now() + self.options.idle_timeout);
        }

        //obtain prev block and remove it block from the top blocks because new block is on top now

        let hash_zero = UInt256::default();
        let mut prev_block = None;

        if prev != hash_zero
        //TODO: check if we really need this comparison
        {
            prev_block = self.get_block(prev.clone());

            if self.top_blocks.contains_key(&prev) {
                self.top_blocks.remove(&prev);
            }
        }

        assert!(source_id < self.sources.len());

        //initialize new block dependencies and update top blocks (if dependency is on top)

        let mut block_deps: Vec<BlockPtr> = Vec::with_capacity(deps.len());

        for dep in &deps {
            if !self.blamed_sources[source_id] && self.top_blocks.contains_key(dep) {
                self.top_blocks.remove(dep);
            }

            let block_dep = self.get_block(dep.clone());

            assert!(block_dep.is_some());

            block_deps.push(block_dep.unwrap());
        }

        assert!(
            height as u64
                <= crate::receiver::get_max_block_height(&self.options, self.sources.len())
        );

        //create and register a new block

        let source_public_key_hash = &self.sources[source_id];
        let block = CatchainFactory::create_block(
            source_id,
            fork_id,
            source_public_key_hash.clone(),
            height,
            hash.clone(),
            payload,
            prev_block,
            block_deps,
            forks_dep_heights,
            self.generate_extra_id(),
        );

        self.blocks.insert(hash.clone(), block.clone());
        self.blocks_storage.add(block.clone());
        self.block_descs.insert(
            hash.clone(),
            Rc::new(RefCell::new(BlockDesc { processed: false, preprocessed: false })),
        );

        //update top of the blocks and initiate blocks processing if needed

        if !self.blamed_sources[source_id] {
            self.send_preprocess(block.clone(), true);

            self.top_source_blocks[source_id] = Some(block.clone());

            if source_id != self.local_idx {
                self.top_blocks.insert(hash.clone(), block.clone());

                log::trace!(
                    "...block {:?} has been added to top blocks (top_blocks_count={})",
                    &hash,
                    self.top_blocks.len()
                );
            }

            if self.top_blocks.is_empty() && !self.active_process && self.receiver_started {
                self.update_next_block_generation_time(
                    SystemTime::now() + self.options.idle_timeout,
                );
            }
        }
    }

    /*
        Nodes blaming management
    */

    pub fn on_blame(&mut self, source_id: usize) {
        //do not blame same validator again

        if self.blamed_sources[source_id] {
            return;
        }

        self.blamed_sources[source_id] = true;

        //remove top block for blamed validator and recompute top blocks

        self.top_source_blocks[source_id] = None;

        self.top_blocks.clear();

        let sources_count = self.sources.len();

        for i in 0..sources_count {
            if self.blamed_sources[i] || self.top_source_blocks[i].is_none() || i == self.local_idx
            {
                continue;
            }

            if let Some(ref parent_block) = self.top_source_blocks[i] {
                let mut need_to_add_block = true;

                if self.is_processed(&*parent_block.clone()) {
                    continue;
                }

                for j in 0..sources_count {
                    if i == j || self.blamed_sources[j] || self.top_source_blocks[j].is_none() {
                        continue;
                    }

                    if let Some(ref block) = self.top_source_blocks[j] {
                        if block.is_descendant_of(&*parent_block.clone()) {
                            need_to_add_block = false;
                            break;
                        }
                    }
                }

                if need_to_add_block {
                    let parent_hash = parent_block.get_hash().clone();

                    self.top_blocks.insert(parent_hash, parent_block.clone());
                }
            }
        }

        //remove source_id from process_deps to prevent fork referencing for new blocks

        let mut i = 0;
        while i < self.process_deps.len() {
            if self.blocks[&self.process_deps[i]].get_source_id() as usize == source_id {
                if let Some(last) = self.process_deps.pop() {
                    if i < self.process_deps.len() {
                        self.process_deps[i] = last;
                    }
                }
            } else {
                i += 1;
            }
        }

        //notify receiver about blame processing state

        self.receiver.blame_processed(source_id);
    }

    /*
        Network messages transfering from Receiver to Validator Session
    */

    pub fn on_custom_query(
        &mut self,
        source_public_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "CatchainProcessor.on_custom_query: public_key_hash={} payload={:?}",
                source_public_key_hash,
                data
            );
        }

        self.notify_custom_query(source_public_key_hash, data, response_callback);
    }

    /*
        Network messages transfering from Validator Session to Overlay
    */

    fn send_query_via_rldp(
        &mut self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        check_execution_time!(20_000);
        instrument!();

        self.receiver.send_query_via_rldp(
            dst_adnl_id,
            name,
            response_callback,
            timeout,
            query,
            max_answer_size,
            v2,
        );
    }

    /*
        Debug interface
    */

    fn debug_add_fork(&self, payload: BlockPayloadPtr, height: BlockHeight) {
        self.receiver.debug_add_fork(payload, height, Vec::new());
    }

    /*
        Creation
    */

    pub fn create(
        task_queues: CatchainTaskQueuesPtr,
        options: Options,
        session_id: SessionId,
        ids: Vec<CatchainNode>,
        local_key: PrivateKey,
        listener: CatchainListenerPtr,
        metrics_receiver: MetricsHandle,
        receiver: ReceiverPtr,
    ) -> Result<CatchainProcessor> {
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

        //configure metrics

        let process_blocks_requests_counter =
            metrics_receiver.sink().register_counter(&"process_blocks_requests".into());
        let process_blocks_responses_counter =
            metrics_receiver.sink().register_counter(&"process_blocks_responses".into());
        let process_blocks_skip_responses_counter =
            metrics_receiver.sink().register_counter(&"process_blocks_skip_responses".into());
        let process_blocks_batching_requests_counter =
            metrics_receiver.sink().register_counter(&"process_blocks_batching_requests".into());
        let processed_block_payload_size_histogram =
            metrics_receiver.sink().register_histogram(&"processed_block_payload_size".into());

        //catchain processor creation

        let body = Self {
            _task_queues: task_queues,
            session_id,
            next_block_generation_time: SystemTime::now(),
            options,
            receiver,
            catchain_listener: listener,
            blocks_storage: BlocksStorage::new(),
            blocks: HashMap::new(),
            block_descs: HashMap::new(),
            top_blocks: HashMap::new(),
            top_source_blocks: vec![None; ids.len()],
            sources,
            blamed_sources: vec![false; ids.len()],
            process_deps: Vec::new(),
            processing_blocks_stack: Vec::with_capacity(BLOCKS_PROCESSING_STACK_CAPACITY),
            processing_blocks_stack_tmp: Vec::with_capacity(BLOCKS_PROCESSING_STACK_CAPACITY),
            _local_adnl_id: ids[local_idx].adnl_id.clone(),
            _ids: ids,
            _local_id: local_id,
            local_idx,
            force_process: false,
            active_process: false,
            rng: rand::thread_rng(),
            current_extra_id: 0,
            receiver_started: false,
            current_time: None,
            process_blocks_requests_counter,
            process_blocks_responses_counter,
            process_blocks_skip_responses_counter,
            process_blocks_batching_requests_counter,
            processed_block_payload_size_histogram,
        };

        Ok(body)
    }
}

impl Drop for CatchainProcessor {
    fn drop(&mut self) {
        log::debug!("Dropping CatchainProcessor...");
        self.stop();

        log::trace!("...catchain has been stopped");
    }
}

// Dummy overlay has been moved to dummy_catchain_overlay.rs

/*
    Implementation of public Catchain trait
*/

impl Catchain for CatchainImpl {
    /*
        Catchain stop
    */

    fn stop_async(&self) {
        // Don't modify destroy_db_flag - preserve its current value
        self.should_stop_flag.store(true, Ordering::SeqCst);
    }

    fn stop(&self) {
        self.stop_async();

        loop {
            if self.main_thread_is_stopped_flag.load(Ordering::SeqCst) {
                break;
            }

            log::info!(
                "...waiting for Catchain threads (session_id is {:x}), main={}",
                self.session_id,
                self.main_thread_is_stopped_flag.load(Ordering::SeqCst),
            );

            const CHECKING_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

            std::thread::sleep(CHECKING_INTERVAL);
        }

        //waiting for receiver to stop

        self.receiver.stop();

        log::info!("Catchain has been stopped (session_id is {:x})", self.session_id);
    }

    fn destroy(&self) {
        // Set destroy flag first (once true, cannot be unset)
        self.destroy_db_flag.store(true, Ordering::SeqCst);
        // Then perform blocking stop
        self.stop();
    }

    /*
        Catchain blocks processing
    */

    fn request_new_block(&self, time: SystemTime) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.request_new_block(time);
        });
    }

    fn processed_block(&self, payload: BlockPayloadPtr, may_be_skipped: bool) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.processed_block(payload, may_be_skipped);
        });
    }

    /*
        Network access interface
    */

    fn send_broadcast(&self, payload: BlockPayloadPtr) {
        check_execution_time!(20_000);
        instrument!();

        self.receiver.send_broadcast(payload);
    }

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
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.send_query_via_rldp(
                dst_adnl_id,
                name,
                response_callback,
                timeout,
                query,
                max_answer_size,
                v2,
            );
        });
    }

    /*
        Debug interface
    */

    fn debug_add_fork(&self, payload: BlockPayloadPtr, height: BlockHeight) {
        self.task_queues.post_main_queue_closure(move |processor: &mut CatchainProcessor| {
            processor.debug_add_fork(payload, height);
        });
    }
}

/*
    Private CatchainImpl details
*/

impl Drop for CatchainImpl {
    fn drop(&mut self) {
        log::debug!("Dropping Catchain...");
        // stop() without destroying DB - preserve data on unexpected drop
        self.stop();
    }
}

impl CatchainImpl {
    /*
        Catchain creation
    */

    pub fn create(
        options: &Options,
        session_id: &SessionId,
        ids: Vec<CatchainNode>,
        local_key: &PrivateKey,
        path: String,
        db_suffix: String,
        allow_unsafe_self_blocks_resync: bool,
        overlay_manager: CatchainOverlayManagerPtr,
        listener: CatchainListenerPtr,
    ) -> Result<CatchainPtr> {
        log::debug!("Creating Catchain...");

        let should_stop_flag = Arc::new(AtomicBool::new(false));
        let destroy_db_flag = Arc::new(AtomicBool::new(false));
        let main_thread_is_stopped_flag = Arc::new(AtomicBool::new(false));
        let main_thread_overloaded_flag = Arc::new(AtomicBool::new(false));
        let name = format!("Catchain_{:x}", session_id);
        let catchain_activity_node = CatchainFactory::create_activity_node(name);

        let metrics_receiver = MetricsHandle::new(Some(Duration::from_secs(30)));
        let task_queues = Arc::new(CatchainTaskQueues::new(metrics_receiver.clone()));

        // Create receiver and receiver listener after catchain is created
        let receiver_listener = ReceiverListenerImpl::create(task_queues.clone());
        let receiver = CatchainFactory::create_receiver(
            session_id.clone(),
            *options,
            Arc::downgrade(&receiver_listener),
            ids.clone(),
            local_key.clone(),
            path.clone(),
            db_suffix.clone(),
            allow_unsafe_self_blocks_resync,
            overlay_manager.clone(),
        )?;

        let body: CatchainImpl = CatchainImpl {
            task_queues: task_queues.clone(),
            should_stop_flag: should_stop_flag.clone(),
            main_thread_is_stopped_flag: main_thread_is_stopped_flag.clone(),
            _main_thread_overloaded_flag: main_thread_overloaded_flag.clone(),
            destroy_db_flag: destroy_db_flag.clone(),
            session_id: session_id.clone(),
            _activity_node: catchain_activity_node.clone(),
            receiver: receiver.clone(),
            _receiver_listener: receiver_listener.clone(),
        };

        let catchain = Arc::new(body);

        let local_key = local_key.clone();
        let session_id = session_id.clone();
        let options = *options;

        let task_queues_for_main_loop = task_queues.clone();

        let stop_flag_for_main_loop = should_stop_flag.clone();
        let _main_thread = std::thread::Builder::new()
            .name(format!("{}:{:x}", MAIN_LOOP_NAME, session_id))
            .stack_size(CATCHAIN_MAIN_LOOP_THREAD_STACK_SIZE)
            .spawn(move || {
                CatchainProcessor::main_loop(
                    task_queues_for_main_loop,
                    stop_flag_for_main_loop,
                    main_thread_is_stopped_flag,
                    main_thread_overloaded_flag,
                    destroy_db_flag,
                    options,
                    session_id,
                    ids,
                    local_key,
                    listener,
                    catchain_activity_node,
                    metrics_receiver,
                    receiver,
                );
            });

        Ok(catchain)
    }
}
