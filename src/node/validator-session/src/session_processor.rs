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
    block_candidate_serializer::{deserialize_block_candidate, serialize_block_candidate},
    cache::FifoCache,
    session_description::SessionDescriptionImpl,
    task_queue::{
        create_completion_handler, post_callback_closure, post_closure, CompletionHandler,
        CompletionHandlerId, CompletionHandlerPtr, TaskPtr,
    },
    ton, Any, AsyncRequest, BlockCandidateSignatureVectorPtr, BlockHash, BlockId, BlockPayloadPtr,
    BlockSignature, CallbackTaskQueuePtr, CompletionHandlerProcessor, HashType, Merge,
    MovablePoolObject, PrivateKey, PublicKey, PublicKeyHash, SentBlockPtr, SentBlockWrapper,
    SessionDescription, SessionFactory, SessionId, SessionListenerPtr, SessionNode, SessionOptions,
    SessionProcessor, SessionProcessorPtr, SessionStatePtr, SessionStateWrapper, TaskQueuePtr,
    ValidatorBlockCandidateCallback, ValidatorBlockCandidateDecisionCallback,
    ValidatorBlockCandidatePtr, SKIP_ROUND_CANDIDATE_BLOCKID,
    TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG,
};
use catchain::{
    check_execution_time, instrument, profiling::ResultStatusCounter, serialize_tl_boxed_object,
    utils::get_elapsed_time, BlockPtr, CatchainPtr, QueryResponseCallback,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::{deserialize_boxed, IntoBoxed};
use ton_block::{
    crc32_digest, error, BlockSignatures, BlockSignaturesPure, BlockSignaturesVariant,
    CryptoSignature, CryptoSignaturePair, Error, KeyId, Result, UInt256, ValidatorBaseInfo,
};

/*
    Constants
*/

const DEBUG_IGNORE_PROPOSALS_PRIORITY: bool = false; //ignore proposals priority and generate block each round
const DEBUG_DUMP_BLOCKS: bool = false; //dump blocks with dependencies and actions for debugging
const DEBUG_DUMP_ON_NEW_ROUND: bool = false; //debug dump for each new round
const DEBUG_DUMP_AFTER_BLOCK_APPLYING: bool = false; //debug dump after each block applying
const DEBUG_REQUEST_NEW_BLOCKS_IMMEDIATELY: bool = false; //request new blocks immediately without waiting
const DEBUG_CHECK_ALL_BEFORE_ROUND_SWITCH: bool = false; //check updates before round switching
                                                         //TODO: remove this debug option after performance tuning
                                                         //const DEBUG_DUMP_BACKTRACE_FOR_LATE_VALIDATIONS: bool = true; //dump all late validations backtrace
const DEBUG_DUMP_BACKTRACE_FOR_LATE_VALIDATIONS: bool = false; //dump all late validations backtrace
const DEBUG_EVENTS_LOG: bool = true; //dump consensus events
const COMPLETION_HANDLERS_MAX_WAIT_PERIOD: Duration = Duration::from_millis(60000); //max wait time for completion handlers
const COMPLETION_HANDLERS_CHECK_PERIOD: Duration = Duration::from_millis(5000); //period of completion handlers checking
const BLOCK_PREPROCESSING_WARN_LATENCY: Duration = Duration::from_millis(200); //max block processing latency
const BLOCK_PROCESSING_WARN_LATENCY: Duration = Duration::from_millis(600); //max block processing latency; expect to have up to 0.5s of natural algorithm latency between process_blocks (see request_new_block for details)
const MAX_NEXT_BLOCK_WAIT_DELAY: Duration = Duration::from_millis(500); //max next block wait delay
const WARN_DUMP_PERIOD: Duration = Duration::from_millis(2000); //warning dump period
const HANGED_CONSENSUS_UPDATE_TIME: Duration = MAX_NEXT_BLOCK_WAIT_DELAY; //update interval for hanged consensus

const DEFAULT_CATCHAIN_MAX_BLOCK_DELAY: Duration = Duration::from_millis(400); //default max block delay for normal attempts
const DEFAULT_CATCHAIN_MAX_BLOCK_DELAY_SLOW: Duration = Duration::from_millis(1000); //default max block delay for slow attempts
const MAX_FUTURE_ROUND_BLOCK: i32 = 100; //max future round block relative to current round
const MAX_PAST_ROUND_BLOCK: i32 = 20; //max past round block relative to current round

const STATES_RESERVED_COUNT: usize = 100000; //reserved states count for blocks
const ROUND_DEBUG_PERIOD: std::time::Duration = Duration::from_secs(15); //round debug time
const LONG_ROUND_PERIOD: std::time::Duration = Duration::from_secs(10); //catchain batching mode is forced enabled for long rounds
const VALIDATOR_IDLE_TIMEOUT: std::time::Duration = Duration::from_secs(10); //allowed inactivity time for validator
const SESSION_WAIT_INFINITE_TIMEOUT: Duration = Duration::from_secs(3600 * 24 * 365); //infinite timeout for session wait

/*
    Implementation details for SessionProcessor
*/

/// Async request implementation
struct AsyncRequestImpl {
    request_id: u32,
    creation_time: SystemTime,
    cancelled: Arc<AtomicBool>,
    cancel_on_drop: bool,
}

impl AsyncRequestImpl {
    fn new(request_id: u32, cancel_on_drop: bool) -> Arc<Self> {
        Arc::new(Self {
            request_id,
            creation_time: SystemTime::now(),
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_on_drop,
        })
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl AsyncRequest for AsyncRequestImpl {
    fn get_request_id(&self) -> u32 {
        self.request_id
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn get_creation_time(&self) -> SystemTime {
        self.creation_time
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for AsyncRequestImpl {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

type BlockCandidateTlPtr = Rc<::ton_api::ton::validator_session::Candidate>;
type BlockCandidateMap =
    Rc<RefCell<HashMap<BlockId, (BlockCandidateTlPtr, u32, std::time::SystemTime)>>>;
type SourceRoundCandidateMap = Vec<HashMap<u32, BlockId>>;
type BlockApproveMap = HashMap<BlockId, (SystemTime, BlockPayloadPtr)>;
type BlockMap = HashMap<BlockId, BlockPayloadPtr>;
type BlockSet = HashSet<BlockId>;
type StateMergeCache = FifoCache<(HashType, HashType), SessionStatePtr>;
type BlockUpdateCache = FifoCache<u32, SessionStatePtr>;
type BlockValidationAttemptMap = HashMap<BlockId, u32>; //map block hash to validation attempt index

/// Precollated block

struct PrecollatedBlock {
    request: Arc<AsyncRequestImpl>,                //request
    candidate: Option<ValidatorBlockCandidatePtr>, //candidate; None if pending
}

type PrecollatedBlockMap = HashMap<u32, PrecollatedBlock>;

/// Delayed action with expiration time
struct DelayedAction {
    expiration_time: SystemTime,
    handler: TaskPtr,
}

pub(crate) struct SessionProcessorImpl {
    completion_task_queue: TaskQueuePtr, //task queue for session callbacks
    callbacks_task_queue: CallbackTaskQueuePtr, //task queue for session callbacks
    session_id: SessionId,               //catchain session ID (incarnation)
    session_listener: SessionListenerPtr, //session listener
    catchain: CatchainPtr,               //catchain session
    next_completion_handler_available_index: CompletionHandlerId, //index of next available complete handler
    completion_handlers: HashMap<CompletionHandlerId, Box<dyn CompletionHandler>>, //complete handlers
    completion_handlers_check_last_time: SystemTime, //time of last completion handlers check
    delayed_actions: Vec<DelayedAction>,             //list of delayed actions
    catchain_started: bool,                          //flag indicates that catchain has been started
    local_key: PrivateKey,                           //private key for signing
    description: SessionDescriptionImpl,             //session description
    catchain_max_block_delay: std::time::Duration,   //max block delay for normal attempts
    catchain_max_block_delay_slow: std::time::Duration, //max block delay for slow attempts
    block_to_state_map: Vec<Option<SessionStatePtr>>, //session states
    state_merge_cache: StateMergeCache,              //cache of merged states
    block_update_cache: BlockUpdateCache,            //cache of states after block updates
    real_state: SessionStatePtr,                     //real state
    virtual_state: SessionStatePtr,                  //virtual state
    current_round: u32,                              //current round sequence number
    first_block_round: u32, //first block round after validator group starts
    accelerated_consensus_current_collator_idx: Option<u32>, //accelerated consensus collator index
    requested_new_block: bool, //new block has been requested in catchain
    requested_new_block_now: bool, //new block has been requested in catchain to be generated immediately
    session_creation_time: SystemTime, //session creation time
    session_processor_creation_time: SystemTime, //session processor creation time
    next_awake_time: SystemTime,   //next awake timestamp
    round_started_at: SystemTime,  //round start time
    round_debug_at: SystemTime,    //round debug checkpoint time
    pending_generate: bool,        //block generation request has been sent to collator
    generated: bool,               //block has been generated
    sent_generated: bool,          //generated block has been sent to a catchain
    generated_block: BlockId,      //generated block ID
    precollated_blocks: PrecollatedBlockMap, //map of precollated blocks
    precollated_blocks_next_request_id: u32, //next request ID for precollation
    precollated_blocks_max_round: Option<u32>, //max round for precollated blocks
    precollation_requests_counter: metrics::Counter, //counter for precollation requests
    precollation_results_counter: metrics::Counter, //counter for precollation results
    blocks: BlockCandidateMap,     //map of blocks for rounds
    source_round_candidate: SourceRoundCandidateMap, //map of source round candidate
    pending_approve: BlockSet,     //set of blocks which are pending for approval
    validation_attempt_map: BlockValidationAttemptMap, //map block hash to validation attempt index
    pending_reject: BlockMap,      //map of blocks to be rejected
    rejected: BlockSet,            //set of blocks which has been rejected
    approved: BlockApproveMap,     //map of approved blocks
    active_requests: BlockSet,     //set of requested block candidates
    pending_sign: bool,            //block candidate is pending for signature
    signed: bool,                  //block candidate has been signed
    signed_block: BlockId,         //signated block ID
    signature: BlockSignature,     //block candidate signature
    compress_block_candidates: bool, //compress block candidates
    log_replay_report_current_time: SystemTime, //log replay current time (for reporting)
    validates_counter: ResultStatusCounter, //result status counter for approval requests
    collates_counter: ResultStatusCounter, //result status counter for collation requests
    collates_expire_counter: ResultStatusCounter, //result status counter for expired collation requests
    collates_precollated_counter: ResultStatusCounter, //result status counter for precollated collation requests
    commits_counter: ResultStatusCounter,              //result status counter for commits requests
    rldp_queries_counter: ResultStatusCounter,         //result status counter for RLDP queries
    preprocess_block_counter: metrics::Counter,        //counter for preprocess calls
    preprocess_block_latency_histogram: metrics::Histogram, //histogram for preprocess block latency
    process_blocks_counter: metrics::Counter,          //counter for process calls
    process_blocks_latency_histogram: metrics::Histogram, //histogram for process blocks latency
    request_new_block_counter: metrics::Counter,       //counter for new blocks requesting
    check_all_counter: metrics::Counter,               //counter for check_all calls
    last_preprocess_block_warn_dump_time: SystemTime, //last time preprocess block latency warning has been printed
    last_process_blocks_warn_dump_time: SystemTime, //last time process blocks latency warning has been printed
    last_preprocess_block_time: Vec<SystemTime>, //time of last preprocess block request from a partial validator
    round_duration_histogram: metrics::Histogram, //histogram for round duration
    first_candidate_received: bool, //first candidate for validation has been received in current round
    first_candidate_received_latency_histogram: metrics::Histogram, //histogram for first candidate approved in a round
    first_candidate_approved: bool, //first candidate for validation has been received in current round
    first_candidate_approved_latency_histogram: metrics::Histogram, //histogram for first candidate approved in a round
    first_candidate_voted: bool, //first candidate for validation has been voted in current round
    first_candidate_voted_latency_histogram: metrics::Histogram, //histogram for first candidate voted in a round
    first_candidate_precommitted: bool, //first candidate for validation has been precommitted in current round
    first_candidate_precommitted_latency_histogram: metrics::Histogram, //histogram for first candidate precommitted in a round
    block_candidate_broadcast_validation_latency_histogram: metrics::Histogram, //histogram for block candidate broadcast processing during validation
    validation_latency_histogram: metrics::Histogram, //histogram for block candidate validation
    collation_latency_histogram: metrics::Histogram,  //histogram for block candidate collation
    active_weight_gauge: metrics::Gauge,              //gauge for active weight
    use_callback_thread: bool,                        //use callback thread for session callbacks
}

/*
    Implementation for public SessionProcessor trait
*/

impl SessionProcessor for SessionProcessorImpl {
    /*
        Accessors
    */

    fn get_description(&self) -> &dyn SessionDescription {
        &self.description
    }

    fn get_impl(&self) -> &dyn Any {
        self
    }

    fn get_mut_impl(&mut self) -> &mut dyn Any {
        self
    }

    /*
        Stop processing
    */

    fn stop(&mut self, destroy_catchain_db: bool) {
        log::debug!("Stopping ValidatorSession processor...");

        // Choose which catchain method to call based on the destroy flag
        if destroy_catchain_db {
            self.catchain.destroy();
        } else {
            self.catchain.stop();
        }

        log::debug!("ValidatorSession processor has been stopped");
    }

    /*
        Session processor configuration
    */

    fn set_catchain_max_block_delay(
        &mut self,
        delay: std::time::Duration,
        delay_slow: std::time::Duration,
    ) {
        self.catchain_max_block_delay = delay;
        self.catchain_max_block_delay_slow = delay_slow;
    }

    /*
        Awake time management
    */

    fn set_next_awake_time(&mut self, timestamp: std::time::SystemTime) {
        if timestamp > self.next_awake_time {
            return;
        }

        self.next_awake_time = timestamp;
    }

    fn reset_next_awake_time(&mut self) {
        self.next_awake_time = std::time::SystemTime::now() + SESSION_WAIT_INFINITE_TIMEOUT;
    }

    fn get_next_awake_time(&self) -> std::time::SystemTime {
        self.next_awake_time
    }

    /*
        Consensus iteration checkers
    */

    fn check_all(&mut self) {
        instrument!();

        self.check_all_counter.increment(1);

        //flush caches

        self.state_merge_cache.flush();
        self.block_update_cache.flush();

        //check completion handlers

        if let Ok(completion_handlers_check_elapsed) =
            self.completion_handlers_check_last_time.elapsed()
        {
            if completion_handlers_check_elapsed > COMPLETION_HANDLERS_CHECK_PERIOD {
                instrument!();
                check_execution_time!(20_000);

                self.check_completion_handlers();
                self.completion_handlers_check_last_time = std::time::SystemTime::now();
            }
        }

        //process delayed actions

        self.process_delayed_actions();

        //no actions are needed before start of Catchain

        if !self.catchain_started {
            return;
        }

        //don't check anything if received consensus block is not the same as current round

        if self.virtual_state.get_current_round_sequence_number() != self.current_round {
            self.request_new_block(false);
            return;
        }

        //round debug dump

        if self.description.is_in_past(self.round_debug_at) {
            self.debug_dump();

            self.round_debug_at = self.description.get_time() + ROUND_DEBUG_PERIOD;
        }

        //check session state

        let attempt_seqno = self.description.get_attempt_sequence_number(self.description.get_ts());

        self.check_sign_slot();
        self.check_approve();
        self.check_generate_slot();
        self.check_action(attempt_seqno);
        self.check_vote_for_slot(attempt_seqno);

        //update metrics

        let total_active_weight = self.get_total_active_weight();

        self.active_weight_gauge.set(total_active_weight as f64);

        if self.virtual_state.has_approved_block(&self.description)
            && !self.first_candidate_approved
        {
            log::trace!("first block candidate has been approved in round #{}", self.current_round);

            self.first_candidate_approved = true;

            if let Ok(latency) = self.get_latency_from_round_start() {
                self.first_candidate_approved_latency_histogram.record(latency.as_millis() as f64);
            }
        }

        if self.virtual_state.has_voted_block(&self.description) && !self.first_candidate_voted {
            log::trace!("first block candidate has been voted in round #{}", self.current_round);

            self.first_candidate_voted = true;

            if let Ok(latency) = self.get_latency_from_round_start() {
                self.first_candidate_voted_latency_histogram.record(latency.as_millis() as f64);
            }
        }

        if self.virtual_state.has_precommitted_block() && !self.first_candidate_precommitted {
            log::trace!(
                "first block candidate has been precommitted in round #{}",
                self.current_round
            );

            self.first_candidate_precommitted = true;

            if let Ok(latency) = self.get_latency_from_round_start() {
                self.first_candidate_precommitted_latency_histogram
                    .record(latency.as_millis() as f64);
            }
        }

        //update next check_all() call timestamp

        self.set_next_awake_time(self.round_debug_at);

        if !self.description.is_accelerated_consensus_enabled() {
            self.set_next_awake_time(self.description.get_attempt_start_at(attempt_seqno + 1));
        }
    }

    /*
        Catchain blocks processing management
    */

    fn preprocess_block(&mut self, block: BlockPtr) {
        check_execution_time!(40_000);
        instrument!();

        let start_time = SystemTime::now();
        let source_id = block.get_source_id();

        log::trace!("Preprocessing block {}", block);

        self.preprocess_block_counter.increment(1);

        self.last_preprocess_block_time[source_id as usize] = start_time;

        let block_payload_processing_latency =
            get_elapsed_time(&block.get_payload().get_creation_time());

        self.preprocess_block_latency_histogram
            .record(block_payload_processing_latency.as_millis() as f64);

        if block_payload_processing_latency > BLOCK_PREPROCESSING_WARN_LATENCY {
            let block_processing_latency = get_elapsed_time(&block.get_creation_time());
            const EPS: Duration = Duration::from_millis(10);
            let delivery_issue =
                block_payload_processing_latency.saturating_sub(block_processing_latency) > EPS;

            if let Ok(warn_elapsed) = self.last_preprocess_block_warn_dump_time.elapsed() {
                if warn_elapsed > WARN_DUMP_PERIOD {
                    let source_public_key_hash =
                        self.description.get_source_public_key_hash(source_id);

                    log::warn!(
                        "{}: ValidatorSession block payload latency is {:.3}s, \
                        block latency is {:.3}s (expected_latency={:.3}s, source=v{:03} ({})): {}",
                        if delivery_issue {
                            "Delivery time issue"
                        } else {
                            "Preprocessing time issue"
                        },
                        block_payload_processing_latency.as_secs_f64(),
                        block_processing_latency.as_secs_f64(),
                        BLOCK_PREPROCESSING_WARN_LATENCY.as_secs_f64(),
                        source_id,
                        source_public_key_hash,
                        &block
                    );
                    self.last_preprocess_block_warn_dump_time = SystemTime::now();
                }
            }
        }

        let payload_len = block.get_payload().data().len();
        let deps = block.get_deps();
        let deps_len = deps.len();

        log::trace!("...received block with payload: {} bytes, and {} deps", payload_len, deps_len);

        //parse payload

        let (block_update, need_actualize_state) = if !block.get_payload().data().is_empty()
            || !deps.is_empty()
        {
            instrument!();

            log::trace!("...parsing incoming block update");

            //try to parse block update

            let block_update: Result<ton::BlockUpdate> =
                catchain::utils::deserialize_tl_boxed_object(block.get_payload().data());

            match block_update {
                Ok(block_update) => {
                    let block_update = block_update.only();
                    (Some(block_update), true)
                }
                Err(err) => {
                    let node_public_key_hash =
                        self.description.get_source_public_key_hash(block.get_source_id()).clone();

                    log::warn!(
                        "Node {} sent a block {:?} which can't be parsed: {:?}",
                        node_public_key_hash,
                        block.get_hash(),
                        err
                    );

                    (None, true)
                }
            }
        } else {
            (None, false)
        };

        //search for state in block update cache

        let state = if let Some(state) = self.get_state_for_block_update(&block_update) {
            state
        } else {
            //merge state

            log::trace!("...prev block is {:?}", block.get_prev());

            let mut state = if let Some(prev) = block.get_prev() {
                self.get_state(&prev).clone()
            } else {
                log::trace!("...create initial state");
                SessionFactory::create_state(&mut self.description)
            };

            log::trace!("...merge state {:08x?} with dependencies", state.get_hash());

            for dep_block in deps {
                let dep_state = self.get_state(dep_block).clone();
                let state_hash = state.get_hash();

                state = self.merge_states(&state, &dep_state, false);

                log::trace!(
                    "...state merged: ({:08x?}, {:08x?}) -> {:08x?}",
                    state_hash,
                    dep_state.get_hash(),
                    state.get_hash()
                );
            }

            log::trace!("...merged virtual state is: {:?}", state);

            //dump block before actions applying (for debugging only)

            if DEBUG_DUMP_BLOCKS {
                log::trace!("...dump block before actions applying: {:?}", block.get_hash());

                self.dump_block(&block);
            }

            //apply actions from incoming block & check payload

            if need_actualize_state {
                instrument!();

                let node_public_key_hash =
                    self.description.get_source_public_key_hash(block.get_source_id()).clone();
                let node_source_id = block.get_source_id();

                if let Some(block_update) = block_update {
                    log::trace!("...BlockUpdate has been received: {:?}", block_update);

                    //apply actions to state

                    let attempt_id =
                        self.description.get_attempt_sequence_number(block_update.ts as u64);

                    log::trace!("...attempt ID is {}", attempt_id);
                    log::trace!("...applying actions");

                    for msg in block_update.actions.iter() {
                        log::trace!(
                            "Node {} applying action on block {:?}: {:?}",
                            node_public_key_hash,
                            block.get_hash(),
                            msg
                        );

                        state = state.apply_action(
                            &mut self.description,
                            node_source_id,
                            attempt_id,
                            msg,
                            block.get_creation_time(),
                            block.get_payload().get_creation_time(),
                        );
                    }

                    //actualize state

                    state = state.make_all(&mut self.description, node_source_id, attempt_id);

                    //check hashes

                    log::trace!("...check hashes");

                    if state.get_hash() != block_update.state as u32 {
                        log::warn!(
                            "Node {} sent a block {:?} with hash mismatch: \
                            computed={:08x?}, received={:08x?}",
                            node_public_key_hash,
                            block.get_hash(),
                            state.get_hash(),
                            block_update.state as u32
                        );
                        for msg in block_update.actions.iter() {
                            log::warn!(
                                "Node {} sent a block {:?} with hash mismatch: \
                                applied action: {:?}",
                                node_public_key_hash,
                                block.get_hash(),
                                msg
                            );
                        }
                    }
                } else {
                    log::warn!(
                        "Node {} sent a block {:?} which can't be parsed: actualize the state",
                        node_public_key_hash,
                        block.get_hash(),
                    );

                    state = state.make_all(
                        &mut self.description,
                        node_source_id,
                        state.get_ts(node_source_id),
                    );
                }
            }

            //update session states

            log::trace!("...move state {:08x?} to persistent memory", state.get_hash());

            state = state.move_to_persistent(&mut self.description);

            if !TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG {
                //update cache

                self.block_update_cache.insert(state.get_hash(), state.clone());
            }

            state
        };

        //set state

        self.set_state(&block, state.clone());

        //dump block before actions applying (for debugging only)

        if DEBUG_DUMP_BLOCKS {
            log::trace!("...dump block after actions applying: {:?}", block.get_hash());

            self.dump_block(&block);
        }

        //update real state for self updated block

        if block.get_source_id() == self.get_local_idx() && !self.catchain_started {
            log::trace!(
                "...use preprocessed block state {:08x?} as a real state",
                self.real_state.get_hash()
            );
            self.real_state = state.clone();
        }

        let virtual_state_hash = self.virtual_state.get_hash();

        self.virtual_state = self.merge_states(&self.virtual_state.clone(), &state.clone(), true);

        log::trace!(
            "...state merged to virtual state: ({:08x?},{:08x?}) -> {:08x?}",
            virtual_state_hash,
            state.get_hash(),
            self.virtual_state.get_hash()
        );

        log::trace!("...new virtual_state: {:?}", &self.virtual_state);

        //clear temp memory after moving states to persistent memory

        log::trace!("...clear temporary memory after merging");

        self.description.get_cache().clear_temp_memory();

        //debug dump

        if DEBUG_DUMP_AFTER_BLOCK_APPLYING {
            self.debug_dump();
        }

        log::trace!("...do consensus iteration (after preprocess block)");

        //notify about starting of a new round if state is changed after merging

        let state_round = self.real_state.get_current_round_sequence_number();

        if state_round != self.current_round {
            self.new_round(state_round);
        }

        //check state in current round

        self.check_all();

        //debug output

        let processing_delay = start_time.elapsed().unwrap_or_default();

        log::trace!(
            "...finish preprocessing block {} in {}ms; state={}",
            block,
            processing_delay.as_millis(),
            state.get_hash()
        );
    }

    fn process_blocks(&mut self, blocks: Vec<BlockPtr>) {
        check_execution_time!(100_000);
        instrument!();

        let start_time = SystemTime::now();

        log::trace!("Processing blocks {:?}", blocks);

        self.process_blocks_counter.increment(1);

        //reset flags

        self.requested_new_block = false;
        self.requested_new_block_now = false;

        let prev_real_state_hash = self.real_state.get_hash();

        //merge real state

        log::trace!(
            "...merge block states to real state with hash {:08x?}",
            self.real_state.get_hash()
        );

        for block in &blocks {
            let block_payload_processing_latency =
                get_elapsed_time(&block.get_payload().get_creation_time());

            self.process_blocks_latency_histogram
                .record(block_payload_processing_latency.as_millis() as f64);

            if block_payload_processing_latency > BLOCK_PROCESSING_WARN_LATENCY {
                let block_processing_latency = get_elapsed_time(&block.get_creation_time());
                const EPS: Duration = Duration::from_millis(10);
                let delivery_issue =
                    block_payload_processing_latency.saturating_sub(block_processing_latency) > EPS;

                let warn_elapsed = get_elapsed_time(&self.last_process_blocks_warn_dump_time);

                if warn_elapsed > WARN_DUMP_PERIOD {
                    let source_id = block.get_source_id();
                    let source_public_key_hash =
                        self.description.get_source_public_key_hash(source_id);

                    log::warn!(
                        "{}: ValidatorSession block payload processing latency is {:.3}s, \
                        block processing latency is {:.3}s (expected_latency={:.3}s, \
                        source=v{:03} ({})): {}",
                        if delivery_issue {
                            "Delivery time issue"
                        } else {
                            "Processing time issue"
                        },
                        block_payload_processing_latency.as_secs_f64(),
                        block_processing_latency.as_secs_f64(),
                        BLOCK_PROCESSING_WARN_LATENCY.as_secs_f64(),
                        source_id,
                        source_public_key_hash,
                        &block
                    );

                    self.last_process_blocks_warn_dump_time = SystemTime::now();
                }
            }

            let real_state_hash = self.real_state.get_hash();
            let block_state = self.get_state(block).clone();

            self.real_state = self.merge_states(&self.real_state.clone(), &block_state, false);

            log::trace!(
                "...real state merged: ({:08x?}, {:08x?}) -> {:08x?}",
                real_state_hash,
                block_state.get_hash(),
                self.real_state.get_hash()
            );
        }

        //start new round if it has been changed according to delivered blocks

        log::trace!("...do consensus iteration (after process blocks)");

        if self.real_state.get_current_round_sequence_number() != self.current_round {
            self.new_round(self.real_state.get_current_round_sequence_number());
        }

        let local_idx = self.get_local_idx();
        let ts = self.description.get_ts();
        let attempt = self.description.get_attempt_sequence_number(ts);
        let now = std::time::SystemTime::now();

        log::trace!(
            "...local_idx={}, round={}, attempt={}, ts_unix_time={}",
            local_idx,
            self.current_round,
            attempt,
            self.description.get_unixtime(ts)
        );

        //store all state updates in a 'message' array which will be applied to real_state when all incremental updates will ge gathered

        let mut messages: Vec<ton::Message> = Vec::new();

        //process blocks generation flow

        if self.generated && !self.sent_generated {
            //generate SubmittedBlock message to notify other validators about block candidate from this validator

            let (block_candidate, _candidate_creation_time) =
                self.get_block_candidate(&self.generated_block).unwrap();
            let file_hash = catchain::utils::get_hash(block_candidate.data());
            if let Some(collated_data) = block_candidate.collated_data() {
                let collated_data_file_hash = catchain::utils::get_hash(collated_data);
                let message = ton::message::SubmittedBlock {
                    round: self.current_round as i32,
                    root_hash: block_candidate.root_hash().clone(),
                    file_hash,
                    collated_data_file_hash,
                };
                log::trace!("...generated SubmittedBlock: {:?}", message);
                messages.push(message.into_boxed());
                self.sent_generated = true;
            } else {
                log::trace!("...no collated data in candidate (compressed?)");
            }
        }

        //process blocks to approve

        log::trace!("...check approvals");

        let to_approve = self.real_state.choose_blocks_to_approve(&self.description, local_idx);

        for block in to_approve {
            let block_id = block.get_id();

            if let Some(block_pair) = self.approved.get(block_id) {
                if block_pair.0 <= self.description.get_time() {
                    //if block has been approved, add corresponding ApprovedBlock message to incremental updates

                    let message = ton::message::ApprovedBlock {
                        round: self.current_round as i32,
                        candidate: block_id.clone(),
                        signature: block_pair.1.data().clone(),
                    };

                    log::trace!("...generated ApprovedBlock: {:?}", message);

                    messages.push(message.into_boxed());
                }
            }
        }

        //process blocks to reject

        for (block_id, rejection_reason) in self.pending_reject.iter() {
            let message = ton::message::RejectedBlock {
                round: self.current_round as i32,
                candidate: block_id.clone(),
                reason: rejection_reason.data().clone(),
            };

            log::trace!("...generated RejectedBlock: {:?}", message);

            messages.push(message.into_boxed());
        }

        self.pending_reject.clear();

        //process commit

        if self.signed {
            log::trace!("...check commit");

            if let Some(block) = self.real_state.choose_block_to_sign(&self.description, local_idx)
            {
                assert!(*block.get_id() == self.signed_block);

                let message = ton::message::Commit {
                    round: self.current_round as i32,
                    candidate: self.signed_block.clone(),
                    signature: self.signature.clone(),
                };

                log::trace!("...generated Commit: {:?}", message);

                messages.push(message.into_boxed());
            }
        }

        //apply incremental updates to a state

        log::trace!("...incremental updates applying");

        for msg in &messages {
            log::trace!(
                "...applying action for node #{} and attempt {}: {:?}",
                local_idx,
                attempt,
                msg
            );

            self.real_state = self.real_state.apply_action(
                &mut self.description,
                local_idx,
                attempt,
                msg,
                now,
                now,
            );
        }

        //votes processing

        log::trace!("...check voting");

        if self.real_state.check_need_generate_vote_for(&self.description, local_idx, attempt) {
            log::trace!("...generating VOTEFOR");

            let msg = self.real_state.generate_vote_for(&mut self.description, local_idx, attempt);

            log::trace!(
                "...applying VOTEFOR action for node #{} and attempt {}: {:?}",
                local_idx,
                attempt,
                msg
            );

            self.real_state = self.real_state.apply_action(
                &mut self.description,
                local_idx,
                attempt,
                &msg,
                now,
                now,
            );

            messages.push(msg);
        }

        //generating incremental updates according to a new state

        log::trace!("...generate incremental updates and apply them to a real state");

        let mut has_non_empty_messages = false;

        loop {
            let msg = self.real_state.create_action(&self.description, local_idx, attempt);
            let stop = matches!(&msg, None | Some(ton::Message::ValidatorSession_Message_Empty(_)));

            log::trace!("...generated action: {:?}", msg.as_ref().unwrap());

            self.real_state = self.real_state.apply_action(
                &mut self.description,
                local_idx,
                attempt,
                msg.as_ref().unwrap(),
                now,
                now,
            );

            messages.push(msg.unwrap());

            const MESSAGES_COUNT_WARN: usize = 100;

            if messages.len() > MESSAGES_COUNT_WARN && messages.len() % MESSAGES_COUNT_WARN == 0 {
                log::warn!(
                    "Too many messages {} during processing blocks for session {}",
                    messages.len(),
                    self.session_id.to_hex_string()
                );
            }

            if stop {
                break;
            }

            has_non_empty_messages = true;
        }

        //move real state to persistent memory

        log::trace!("...move real state {:08x?} to persistent memory", self.real_state.get_hash());

        self.real_state = self.real_state.move_to_persistent(&mut self.description);

        log::trace!("...new real_state: {:?}", &self.real_state);

        //prepare new block to be sent to catchain

        let real_state_hash = self.real_state.get_hash();

        log::trace!("...created block with root_hash={:08x?}", real_state_hash);

        let payload = ton::blockupdate::BlockUpdate {
            ts: ts as i64,
            actions: messages,
            state: real_state_hash as i32,
        }
        .into_boxed();
        let serialized_payload = serialize_tl_boxed_object!(&payload);

        //merge changes from a real state to a virtual state

        let prev_virtual_state_hash = self.virtual_state.get_hash();

        log::trace!(
            "...merge changes from a real state {:08x?} to a virtual state {:08x?}",
            self.real_state.get_hash(),
            prev_virtual_state_hash
        );

        let new_virtual_state =
            self.merge_states(&self.virtual_state.clone(), &self.real_state.clone(), true);

        log::trace!("...new virtual_state: {:?}", &new_virtual_state);

        //send new block back to a catchain

        let round = self.real_state.get_current_round_sequence_number();

        let block_may_be_skipped = round == self.current_round
            && prev_real_state_hash == real_state_hash
            && prev_virtual_state_hash == new_virtual_state.get_hash()
            && !has_non_empty_messages;

        log::trace!(
            "...notify catchain about new block {}{:?}",
            if block_may_be_skipped { "which may be skipped " } else { "" },
            serialized_payload
        );

        self.catchain.processed_block(
            catchain::CatchainFactory::create_block_payload(serialized_payload),
            block_may_be_skipped,
        );

        //check if new round is appeared

        log::trace!(
            "...round after changes applying is {} (current is {})",
            round,
            self.current_round
        );

        if round > self.current_round {
            self.new_round(round);
        }

        //merge changes from a real state to a virtual state (so they should be equal after such merging)

        self.virtual_state = new_virtual_state;

        if prev_virtual_state_hash != self.virtual_state.get_hash() && block_may_be_skipped {
            //this is assert-like warning without halting the processing thread; only for debugging
            log::warn!(
                "Block processing was skipped due to absence of real state changes \
                but virtual state was updated"
            );
        }

        if self.description.is_accelerated_consensus_enabled()
            && log::log_enabled!(log::Level::Debug)
        {
            //TODO: remove this after debugging

            // Calculate max approved weight across all blocks in current round
            let (approved_blocks_count, approved_weight_string) = {
                // Get all blocks approved by the local validator to find all blocks in the round
                let approved_blocks = self
                    .virtual_state
                    .get_blocks_approved_by(&self.description, self.get_local_idx());
                let mut approved_blocks_count = 0;
                let mut approved_weight_string = String::new();

                approved_weight_string.push_str("[");

                let mut is_first = true;

                for block in approved_blocks {
                    let approvers =
                        self.virtual_state.get_block_approvers(&self.description, block.get_id());
                    let mut weight = 0;
                    for &approver_idx in &approvers {
                        weight += self.description.get_node_weight(approver_idx);
                    }
                    if weight
                        < self.description.get_total_weight() - self.description.get_cutoff_weight()
                            + 1
                    {
                        continue;
                    }
                    if !is_first {
                        approved_weight_string.push_str(", ");
                    } else {
                        is_first = false;
                    }
                    approved_weight_string.push_str(&format!(
                        "{} => {:.2}%",
                        block.get_id().to_hex_string(),
                        weight as f64 * 100.0 / self.description.get_total_weight() as f64
                    ));
                    approved_blocks_count += 1;
                }

                approved_weight_string.push_str("]");

                (approved_blocks_count, approved_weight_string)
            };

            log::debug!(
                "VirtualState check: src={:02}, deps_from=[{:<15}], round={:03}, att={attempt}, \
                is_generated={: <5}, approved={}, voted={}, precommitted={}, signed={}",
                self.get_local_idx(),
                blocks
                    .iter()
                    .map(|b| b.get_source_id().to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
                self.current_round,
                self.generated,
                if approved_blocks_count > 0 { approved_weight_string } else { "[]".to_string() },
                self.virtual_state.has_voted_block(&self.description),
                self.virtual_state.has_precommitted_block(),
                self.virtual_state
                    .get_committed_block(&self.description, self.current_round)
                    .is_some(),
            );
        }

        //clear temporary memory after merging

        log::trace!("...clear temporary memory");

        self.description.get_cache().clear_temp_memory();

        //debug output

        let processing_delay = start_time.elapsed().unwrap_or_default();

        log::trace!(
            "...finish processing blocks in {}ms; real_state={:08x?}, virtual_state={:08x?}",
            processing_delay.as_millis(),
            self.real_state.get_hash(),
            self.virtual_state.get_hash()
        );
    }

    fn finished_catchain_processing(&mut self) {
        check_execution_time!(100_000);
        instrument!();

        log::trace!("Finished catchain blocks processing");

        let virtual_state_hash = &self.virtual_state.get_hash();
        let real_state_hash = &self.real_state.get_hash();

        if virtual_state_hash != real_state_hash {
            log::warn!(
                "SessionProcessor: virtual state and real state hashes mismatch; \
                virtual_state={:08x?} real_state={:08x?}",
                virtual_state_hash,
                real_state_hash
            );

            if log::log_enabled!(log::Level::Debug) {
                self.debug_dump();
            }
        }

        self.virtual_state = self.real_state.clone();

        self.check_all();
    }

    fn catchain_started(&mut self) {
        instrument!();

        log::info!("Catchain startup notification has been received");

        self.catchain_started = true;

        let (self_approved_blocks, round) = {
            let self_approved_blocks =
                self.virtual_state.get_blocks_approved_by(&self.description, self.get_local_idx());
            let round = self.virtual_state.get_current_round_sequence_number();

            (self_approved_blocks, round)
        };

        for block in self_approved_blocks {
            if block.is_none() {
                continue;
            }

            let block = block.unwrap();
            let block_source_public_key =
                self.description.get_source_public_key(block.get_source_index()).clone();
            let block_source_id =
                self.description.get_source_public_key_hash(block.get_source_index()).clone();
            let block_root_hash = block.get_root_hash().clone();
            let completion_task_queue = self.completion_task_queue.clone();
            let compress_block_candidates = self.compress_block_candidates;

            self.notify_get_approved_candidate(
                &block_source_public_key,
                block.get_root_hash(),
                block.get_file_hash(),
                block.get_collated_data_file_hash(),
                Box::new(move |candidate: Result<ValidatorBlockCandidatePtr>| match candidate {
                    Err(err) => log::error!(
                        "SessionProcessor::started: \
                                failed to get candidate from a validator: {:?}",
                        err
                    ),
                    Ok(candidate) => {
                        let broadcast = ::ton_api::ton::validator_session::candidate::Candidate {
                            src: UInt256::with_array(*block_source_id.clone().data()),
                            round: round as i32,
                            root_hash: block_root_hash.clone(),
                            data: candidate.data.data().clone(),
                            collated_data: candidate.collated_data.data().clone(),
                        };
                        let data =
                            match serialize_block_candidate(broadcast, compress_block_candidates) {
                                Ok(data) => data,
                                Err(err) => {
                                    log::error!(
                                        "SessionProcessor::started: \
                                            failed to serialize block candidate: {:?}",
                                        err
                                    );
                                    return;
                                }
                            };

                        post_closure(
                            &completion_task_queue,
                            move |processor: &mut dyn SessionProcessor| {
                                processor.process_broadcast(block_source_id, data, None, false)
                            },
                        );
                    }
                }),
            );
        }

        self.check_all();
    }

    /*
        Time synchronization for Catchain log replay
    */

    fn set_time(&mut self, time: std::time::SystemTime) {
        if log::log_enabled!(log::Level::Trace) {
            if let Ok(duration) = time.duration_since(self.log_replay_report_current_time) {
                const REPORT_TIMEOUT: Duration = Duration::from_millis(1000);

                if duration > REPORT_TIMEOUT {
                    log::trace!("Set log replay time {}", catchain::utils::time_to_string(&time));
                    self.log_replay_report_current_time = time;
                }
            }
        }

        self.description.set_time(time);
    }

    /*
        Network messages processing
    */

    fn process_broadcast(
        &mut self,
        source_id: PublicKeyHash,
        data: BlockPayloadPtr,
        expected_id: Option<BlockId>,
        is_overlay_broadcast: bool,
    ) {
        instrument!();

        // note: src is not necessarily equal to the sender of this message:
        // if requested using get_broadcast_p2p, src is the creator of the block, sender possibly is some other node.
        let src_idx = match self.description.get_source_index(&source_id) {
            Ok(src_index) => src_index,
            Err(err) => {
                log::warn!("Can't get source index for node {source_id}: {err:?}");
                return;
            }
        };

        let data_hash = catchain::utils::get_hash_from_block_payload(&data);
        let candidate_creation_time = data.get_creation_time();
        let candidate = deserialize_block_candidate(
            data,
            self.compress_block_candidates,
            self.description.opts().max_block_size
                + self.description.opts().max_collated_data_size
                + 1024,
            self.description.opts().proto_version,
        );

        if let Err(err) = candidate {
            log::warn!("Can't parse broadcast {:?} from node {}: {:?}", data_hash, source_id, err);
            return;
        }

        log::trace!(
            "Processing broadcast {:?} from node {} (src_idx={})",
            data_hash,
            source_id,
            src_idx
        );

        let candidate = candidate.ok().unwrap();

        //check if the candidate was sent from the node which generated block

        let src = candidate.src();
        if src.as_slice() != source_id.data() {
            log::warn!(
                "Broadcast's {:?} source {:?} mismatches node ID {:?}",
                data_hash,
                src,
                source_id
            );
            return;
        }

        //check block size limit

        let Some(collated_data) = candidate.collated_data() else {
            log::warn!(
                "Broadcast {:?} from source {:?} has no collated data (compressed?)",
                data_hash,
                source_id
            );
            return;
        };
        let data = candidate.data();
        if data.len() > self.description.opts().max_block_size as usize
            || collated_data.len() > self.description.opts().max_collated_data_size as usize
        {
            log::warn!(
                "Broadcast {:?} from source {:?} has too big size={} / collated_size={}",
                data_hash,
                source_id,
                data.len(),
                collated_data.len()
            );
            return;
        }

        //extract data

        let file_hash = catchain::utils::get_hash(&data);
        let collated_data_file_hash = catchain::utils::get_hash(&collated_data);
        let block_round = *candidate.round() as u32;
        let block_id = self.description.candidate_id(
            src_idx,
            candidate.root_hash(),
            &file_hash,
            &collated_data_file_hash,
        );

        //check the block

        if let Some(expected_id) = expected_id {
            if expected_id != block_id {
                log::warn!(
                    "Broadcast {:?} from source v{:03} ({}) has id mismatch",
                    data_hash,
                    src_idx,
                    source_id
                );
                return;
            }
        }

        if (block_round as i32) < (self.current_round as i32 - MAX_PAST_ROUND_BLOCK)
            || (block_round as i32) >= (self.current_round as i32 + MAX_FUTURE_ROUND_BLOCK)
        {
            log::trace!(
                "Broadcast {:?} from source {:?} has invalid round {} (current round is {})",
                data_hash,
                source_id,
                block_round,
                self.current_round
            );
            return;
        }

        if let Some((_tl_block, _candidate_creation_time)) = self.get_block_candidate(&block_id) {
            self.update_block_candidate_round(&block_id, block_round);

            log::trace!("Duplicate broadcast {:?} from source {:?}", data_hash, source_id);
            return;
        }

        if self.description.is_accelerated_consensus_enabled() {
            //accelerated consensus mode
            let priority =
                self.virtual_state.get_current_round_node_priority(&self.description, src_idx);
            if block_round == self.current_round && priority < 0 {
                log::warn!(
                    "Broadcast {:?} from source {:?} skipped: \
                    source is not allowed to generate blocks in the round {}",
                    data_hash,
                    source_id,
                    block_round
                );
                return;
            }

            //TODO: add possibility to protect overflooding of block candidates in accelerated consensus mode
        } else {
            //normal consensus mode
            let priority = self.description.get_normal_node_priority(src_idx, block_round);

            if priority < 0 {
                log::warn!(
                    "Broadcast {:?} from source {:?} skipped: \
                    source is not allowed to generate blocks in the round {}",
                    data_hash,
                    source_id,
                    block_round
                );
                return;
            }
        }

        //ensure the candidate is unique

        if is_overlay_broadcast && !self.ensure_candidate_unique(src_idx, block_round, &block_id) {
            return;
        }

        //register the block

        self.set_block_candidate(&block_id, (Rc::new(candidate), candidate_creation_time));

        log::trace!(
            "...broadcast received for round {}, current round is {}",
            block_round,
            self.current_round
        );

        if block_round != self.current_round {
            return;
        }

        assert!(!self.pending_approve.contains(&block_id));
        assert!(!self.approved.contains_key(&block_id));
        assert!(!self.pending_reject.contains_key(&block_id));
        assert!(!self.rejected.contains(&block_id));

        //trying to approve this block

        let blocks =
            self.virtual_state.choose_blocks_to_approve(&self.description, self.get_local_idx());

        for block in blocks {
            if block.get_id() != &block_id {
                continue;
            }

            self.try_approve_block(block);

            break;
        }
    }

    fn process_query(
        &mut self,
        _source_id: PublicKeyHash,
        data: BlockPayloadPtr,
        callback: QueryResponseCallback,
    ) {
        instrument!();

        //read query data

        let message = match deserialize_boxed(data.data()) {
            Ok(message) => {
                let message =
                    message.downcast::<::ton_api::ton::rpc::validator_session::DownloadCandidate>();
                if let Err(err) = message {
                    callback(Err(error!("validator session: cannot parse query: {:?}", err)));
                    return;
                }

                message.unwrap()
            }
            Err(err) => {
                callback(Err(error!("validator session: cannot parse query: {:?}", err)));
                return;
            }
        };

        //check correctness

        let round_id = message.round as u32;

        if round_id > self.real_state.get_current_round_sequence_number() {
            callback(Err(error!("too big round id {}", round_id)));
            return;
        }

        let src_idx = match self
            .description
            .get_source_index(&KeyId::from_data(*message.id.src.as_slice()))
        {
            Ok(src_idx) => src_idx,
            Err(err) => {
                log::warn!("Can't get source index for node {}: {err:?}", message.id.src);
                callback(Err(error!("unknown source id")));
                return;
            }
        };
        let id = self.description.candidate_id(
            src_idx,
            &message.id.root_hash,
            &message.id.file_hash,
            &message.id.collated_data_file_hash,
        );

        let block = if round_id < self.real_state.get_current_round_sequence_number() {
            let block = self.real_state.get_committed_block(&self.description, round_id);

            if block.is_none() || block.as_ref().unwrap().get_id() != &id {
                callback(Err(error!("wrong block in old round {}", round_id)));
                return;
            }

            block.unwrap().unwrap()
        } else {
            assert!(round_id == self.real_state.get_current_round_sequence_number());

            let block = self.real_state.get_block(&self.description, &id);

            if block.is_none() || block.as_ref().unwrap().is_none() {
                callback(Err(error!("wrong block in current round {}", round_id)));
                return;
            }

            if !self.real_state.check_block_is_approved_by(self.get_local_idx(), &id) {
                callback(Err(error!("not approved in current round {}", round_id)));
                return;
            }

            block.unwrap().unwrap()
        };

        //request approved block from validator

        let source_idx = message.id.src;
        let compress_block_candidates = self.compress_block_candidates;
        let candidate_response = Box::new(move |candidate: Result<ValidatorBlockCandidatePtr>| {
            if candidate.is_err() {
                callback(Err(error!("failed to get candidate for round {}", round_id)));
                return;
            }

            let candidate = candidate.unwrap();
            let candidate = ton::candidate::Candidate {
                src: source_idx,
                round: round_id as ton::int,
                root_hash: candidate.id.root_hash.clone(),
                data: candidate.data.data().clone(),
                collated_data: candidate.collated_data.data().clone(),
            };

            callback(serialize_block_candidate(candidate, compress_block_candidates));
        });

        let source_public_key_hash =
            self.description.get_source_public_key(block.get_source_index()).clone();
        self.notify_get_approved_candidate(
            &source_public_key_hash,
            &message.id.root_hash,
            &message.id.file_hash,
            &message.id.collated_data_file_hash,
            candidate_response,
        );
    }
}

/*
    Implementation for crate CompletionHandlerProcessor trait
*/

impl CompletionHandlerProcessor for SessionProcessorImpl {
    fn get_completion_task_queue(&self) -> &TaskQueuePtr {
        &self.completion_task_queue
    }

    fn add_completion_handler(&mut self, handler: CompletionHandlerPtr) -> CompletionHandlerId {
        let handler_index = self.next_completion_handler_available_index;

        self.next_completion_handler_available_index += 1;

        const MAX_COMPLETION_HANDLER_INDEX: CompletionHandlerId = u64::MAX;

        assert!(self.next_completion_handler_available_index < MAX_COMPLETION_HANDLER_INDEX);

        self.completion_handlers.insert(handler_index, handler);

        handler_index
    }

    fn remove_completion_handler(
        &mut self,
        handler_id: CompletionHandlerId,
    ) -> Option<CompletionHandlerPtr> {
        self.completion_handlers.remove(&handler_id)
    }
}

/*
    Implementation for public Display
*/

impl fmt::Display for SessionProcessorImpl {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unimplemented!();
    }
}

/*
    Implementation internals of SessionProcessorImpl
*/

#[allow(dead_code)]
fn get_impl(value: &dyn SessionProcessor) -> &SessionProcessorImpl {
    value.get_impl().downcast_ref::<SessionProcessorImpl>().unwrap()
}

fn get_mut_impl(value: &mut dyn SessionProcessor) -> &mut SessionProcessorImpl {
    value.get_mut_impl().downcast_mut::<SessionProcessorImpl>().unwrap()
}

impl SessionProcessorImpl {
    /*
        Delayed actions
    */

    fn post_delayed_action<F>(&mut self, expiration_time: SystemTime, handler: F)
    where
        F: FnOnce(&mut dyn SessionProcessor) + Send + 'static,
    {
        let delayed_action = DelayedAction { expiration_time, handler: Box::new(handler) };

        self.delayed_actions.push(delayed_action);
        self.set_next_awake_time(expiration_time);
    }

    fn process_delayed_actions(&mut self) {
        let now = SystemTime::now();
        let mut i = 0;

        while i < self.delayed_actions.len() {
            if self.delayed_actions[i].expiration_time <= now {
                let delayed_action = self.delayed_actions.swap_remove(i);
                (delayed_action.handler)(self);
            } else {
                self.set_next_awake_time(self.delayed_actions[i].expiration_time);
                i += 1;
            }
        }
    }

    /*
        Debug utilities
    */

    fn check_completion_handlers(&mut self) {
        instrument!();

        let mut expired_handlers = Vec::new();

        for (handler_id, handler) in self.completion_handlers.iter() {
            if let Ok(latency) = handler.get_creation_time().elapsed() {
                if latency > COMPLETION_HANDLERS_MAX_WAIT_PERIOD {
                    expired_handlers.push((*handler_id, latency));
                }
            }
        }

        for (handler_id, latency) in expired_handlers.iter_mut() {
            let handler = self.completion_handlers.remove(handler_id);

            if let Some(mut handler) = handler {
                let warning = error!(
                    "Remove ValidatorSession completion handler #{handler_id} with latency {:.3}s \
                    (expected_latency={:.3}s): created at {}",
                    latency.as_secs_f64(),
                    COMPLETION_HANDLERS_MAX_WAIT_PERIOD.as_secs_f64(),
                    catchain::utils::time_to_string(&handler.get_creation_time())
                );

                log::warn!("{}", warning);

                handler.reset_with_error(warning, self);
            }
        }
    }

    fn get_total_active_weight(&self) -> u64 {
        let mut total_active_weight = 0;

        for i in 0..self.description.get_total_nodes() as usize {
            let last_preprocess_block_time = self.last_preprocess_block_time[i];
            let weight = self.description.get_node_weight(i as u32);
            let last_received_block_delay = get_elapsed_time(&last_preprocess_block_time);

            if last_received_block_delay <= VALIDATOR_IDLE_TIMEOUT {
                total_active_weight += weight;
            }
        }

        total_active_weight
    }

    fn debug_dump(&self) {
        instrument!();

        let mut result = "".to_string();
        let round_duration = self.round_started_at.elapsed();

        if let Ok(round_duration) = round_duration {
            if round_duration > ROUND_DEBUG_PERIOD {
                log::warn!(
                    "Session {} round #{} is too long (duration is {:.3}s, \
                    max expected duration is {:.3}s)",
                    self.session_id.to_hex_string(),
                    self.real_state.get_current_round_sequence_number(),
                    round_duration.as_secs_f64(),
                    ROUND_DEBUG_PERIOD.as_secs_f64()
                );
            }
        }

        //all code below will work only for debug logging mode

        if !log::log_enabled!(log::Level::Debug) {
            return;
        }

        let total_active_weight = self.get_total_active_weight();

        result = format!("{}Session {} dump:\n", result, self.session_id.to_hex_string());
        if let Ok(round_duration) = round_duration {
            result =
                format!("{}  - round_duration: {:.3}s\n", result, round_duration.as_secs_f64(),);
        }
        result =
            format!("{}  - validators_count: {}\n", result, self.description.get_total_nodes());
        result = format!("{}  - local_idx: v{:03}\n", result, self.get_local_idx());
        result = format!("{}  - total_weight: {}\n", result, self.description.get_total_weight());
        result = format!("{}  - cutoff_weight: {}\n", result, self.description.get_cutoff_weight());
        result = format!(
            "{}  - reverse_cutoff_weight: {}\n",
            result,
            self.description.get_reverse_cutoff_weight()
        );
        result = format!(
            "{}  - active_weight: {} ({:.2}%)\n",
            result,
            total_active_weight,
            100.0 * total_active_weight as f64 / self.description.get_total_weight() as f64,
        );

        let mut non_active_dump = "".to_string();

        for i in 0..self.description.get_total_nodes() as usize {
            let last_preprocess_block_time = self.last_preprocess_block_time[i];
            let last_received_block_delay = get_elapsed_time(&last_preprocess_block_time);

            if last_received_block_delay <= VALIDATOR_IDLE_TIMEOUT {
                continue;
            }

            if !non_active_dump.is_empty() {
                non_active_dump = format!("{}, ", non_active_dump);
            }

            non_active_dump = if last_preprocess_block_time != SystemTime::UNIX_EPOCH {
                format!(
                    "{}v{:03}/{:.0}s",
                    non_active_dump,
                    i,
                    last_received_block_delay.as_secs_f64()
                )
            } else {
                format!("{}v{:03}/?", non_active_dump, i)
            };
        }

        result = format!("{}  - inactive: [{}]\n", result, non_active_dump);
        result = format!("{}  - nodes:\n", result);

        for i in 0..self.description.get_total_nodes() {
            let public_key_hash = self.description.get_source_public_key_hash(i);
            let adnl_id = self.description.get_source_adnl_id(i);
            let weight = self.description.get_node_weight(i);
            let last_preprocess_block_time = self.last_preprocess_block_time[i as usize];
            let last_received_block_delay = get_elapsed_time(&last_preprocess_block_time);
            let is_active = last_received_block_delay <= VALIDATOR_IDLE_TIMEOUT;
            let last_received_block_delay = if last_preprocess_block_time != SystemTime::UNIX_EPOCH
            {
                format!("{:6.2}s", last_received_block_delay.as_secs_f64())
            } else {
                "    N/A".to_string()
            };

            result = format!(
                "{}    - v{:03}: {} last_block={}, weight={}, adnl_id={}, public_key_hash={}\n",
                result,
                i,
                if is_active { "        " } else { "inactive" },
                last_received_block_delay,
                weight,
                adnl_id,
                public_key_hash,
            );
        }

        result = format!(
            "{}  - real_state:\n    - hash: {:08x}\n{}",
            result,
            self.real_state.get_hash(),
            self.real_state.dump(&self.description)
        );
        result = format!(
            "{}  - virtual_state:\n    - hash: {:08x}\n{}",
            result,
            self.virtual_state.get_hash(),
            self.virtual_state.dump(&self.description)
        );

        log::debug!("{}", result);
    }

    fn dump_block(&self, block: &BlockPtr) {
        self.dump_block_impl(block, 1);
    }

    fn dump_block_impl(&self, block: &BlockPtr, indent: usize) {
        let indent_str = (0..indent).map(|_| "  ").collect::<String>();
        let state = self.find_state(block);

        log::trace!("{}block {:?}", indent_str, block.get_hash());
        log::trace!("{}  prev for {:?}:", indent_str, block.get_hash());

        let mut parents = "".to_string();

        if let Some(ref prev) = block.get_prev() {
            self.dump_block_impl(prev, indent + 1);

            parents = format!("{:?}", prev.get_hash());
        }

        log::trace!("{}  deps for {:?}:", indent_str, block.get_hash());

        let deps = block.get_deps();

        for dep_block in deps {
            self.dump_block_impl(dep_block, indent + 1);

            if !parents.is_empty() {
                parents = format!("{}, ", parents);
            }

            parents = format!("{}{:?}", parents, dep_block.get_hash());
        }

        log::trace!(
            "{}  state for {:?} (parents={}): {:?}",
            indent_str,
            block.get_hash(),
            parents,
            state
        );

        let block_update: Result<ton::BlockUpdate> =
            catchain::utils::deserialize_tl_boxed_object(block.get_payload().data());
        let node_public_key_hash =
            self.description.get_source_public_key_hash(block.get_source_id()).clone();
        let node_source_id = block.get_source_id();

        if let Ok(block_update) = block_update.as_ref() {
            let block_update = block_update.clone().only();
            let attempt_id = self.description.get_attempt_sequence_number(block_update.ts as u64);

            log::trace!(
                "{}  actions for {:?}, attempt={}, source={} ({}):",
                indent_str,
                block.get_hash(),
                attempt_id,
                node_source_id,
                node_public_key_hash
            );

            for msg in block_update.actions.iter() {
                log::trace!("{}    {:?}", indent_str, msg);
            }
        }
    }

    /*
        Accessors
    */

    fn get_local_idx(&self) -> u32 {
        self.description.get_self_idx()
    }

    fn get_local_id(&self) -> &PublicKeyHash {
        self.description.get_source_public_key_hash(self.description.get_self_idx())
    }

    fn get_local_key(&self) -> &PrivateKey {
        &self.local_key
    }

    /*
        Caches
    */

    fn get_state_for_block_update(
        &mut self,
        block_update: &Option<::ton_api::ton::validator_session::blockupdate::BlockUpdate>,
    ) -> Option<SessionStatePtr> {
        if block_update.is_none() || TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG {
            return None;
        }

        let block_update = block_update.as_ref().unwrap();
        let block_update_hash = block_update.state as u32;

        if let Some(state) = self.block_update_cache.get(&block_update_hash) {
            return Some(state.clone());
        }

        None
    }

    fn merge_states(
        &mut self,
        left: &SessionStatePtr,
        right: &SessionStatePtr,
        move_to_persistent: bool,
    ) -> SessionStatePtr {
        instrument!();

        let left_hash = left.get_hash();
        let right_hash = right.get_hash();

        if left_hash == right_hash && !TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG {
            return left.clone();
        }

        let merge_key = (left_hash, right_hash);

        if !TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG {
            if let Some(state) = self.state_merge_cache.get(&merge_key) {
                return state.clone();
            }
        }

        let result = {
            instrument!();

            let mut result = left.merge(right, &mut self.description);

            if move_to_persistent {
                result = result.move_to_persistent(&mut self.description);
            }

            result
        };

        if !TELEGRAM_NODE_COMPATIBILITY_HASHES_BUG {
            self.state_merge_cache.insert(merge_key, result.clone());
        }

        result
    }

    /*
        Block to state mapping
    */

    fn find_state(&self, block: &BlockPtr) -> Option<&SessionStatePtr> {
        let extra_id = block.get_extra_id() as usize;

        if extra_id < self.block_to_state_map.len() {
            if let Some(state) = &self.block_to_state_map[extra_id] {
                return Some(state);
            }
        }

        None
    }

    fn get_state(&self, block: &BlockPtr) -> &SessionStatePtr {
        let state = self.find_state(block);

        if let Some(state) = state {
            return state;
        }

        let extra_id = block.get_extra_id() as usize;

        log::error!("...can't find state for block {:?} with extra ID {}", block, extra_id);

        unreachable!();
    }

    fn set_state(&mut self, block: &BlockPtr, state: SessionStatePtr) {
        let extra_id = block.get_extra_id() as usize;

        if extra_id >= self.block_to_state_map.len() {
            self.block_to_state_map.resize(extra_id + 1, None);
        }

        log::trace!(
            "...set state {:08x?} for block {:?} with extra ID {}",
            state.get_hash(),
            block,
            extra_id
        );

        self.block_to_state_map[extra_id] = Some(state.clone());
    }

    /*
        Round management
    */

    fn get_latency_from_round_start(
        &self,
    ) -> std::result::Result<Duration, std::time::SystemTimeError> {
        self.description.get_time().duration_since(self.round_started_at)
    }

    fn new_round(&mut self, round: u32) {
        instrument!();

        //debug dump for states

        log::trace!(
            "...new round request for current round {} and round {}, {}",
            self.current_round,
            round,
            self.session_id.to_hex_string()
        );

        if DEBUG_DUMP_ON_NEW_ROUND {
            self.debug_dump();
        }

        if round != 0 {
            log::trace!(
                "...reset current round {}, because round {} is started",
                self.current_round,
                round
            );

            assert!(self.current_round < round);

            if let Ok(latency) = self.get_latency_from_round_start() {
                self.round_duration_histogram.record(latency.as_millis() as f64);
            }

            self.pending_generate = false;
            self.generated = false;
            self.sent_generated = false;

            self.pending_approve.clear();
            self.rejected.clear();
            self.pending_reject.clear();
            self.approved.clear();

            self.pending_sign = false;
            self.signed = false;
            self.signature = BlockSignature::default();
            self.signed_block = BlockId::default();

            self.first_candidate_received = false;
            self.first_candidate_approved = false;
            self.first_candidate_voted = false;
            self.first_candidate_precommitted = false;

            self.active_requests.clear();
        }

        //apply finished rounds to current state

        while self.current_round < round {
            log::trace!("...apply current round {}, target round is {}", self.current_round, round);

            if DEBUG_CHECK_ALL_BEFORE_ROUND_SWITCH {
                log::trace!(
                    "...check session state before switching of current round {}, \
                    target round is {round}",
                    self.current_round
                );

                self.check_all();
            }

            //because round was finished we expect it has commit at the end, even with empty block
            let signed_block = self
                .real_state
                .get_committed_block(&self.description, self.current_round)
                .expect("Signed block is expected");
            let src_signatures = self
                .real_state
                .get_committed_block_signatures(self.current_round)
                .expect("Signatures are expected at the round commit phase");
            let src_approve_signatures = self
                .real_state
                .get_committed_block_approve_signatures(self.current_round)
                .expect("Signatures are expected at the round commit phase");

            let signatures_exporter = |desc: &dyn SessionDescription,
                                       signatures: &BlockCandidateSignatureVectorPtr|
             -> Vec<(PublicKeyHash, BlockPayloadPtr)> {
                let mut result: Vec<(PublicKeyHash, BlockPayloadPtr)> =
                    Vec::with_capacity(desc.get_total_nodes() as usize);

                for i in 0..desc.get_total_nodes() as usize {
                    if let Some(signature) = signatures.at(i) {
                        result.push((
                            self.description.get_source_public_key_hash(i as u32).clone(),
                            catchain::CatchainFactory::create_block_payload(
                                signature.get_signature().clone(),
                            ),
                        ));
                    }
                }

                result
            };

            let signatures = signatures_exporter(&self.description, &src_signatures);
            let approve_signatures =
                signatures_exporter(&self.description, &src_approve_signatures);

            if let Some(ref signed_block) = signed_block {
                //signed block was committed

                log::trace!(
                    "...block is signed for round {}; signatures={:?}, approve_signatures={:?}",
                    self.current_round,
                    signatures,
                    approve_signatures
                );

                if DEBUG_EVENTS_LOG {
                    log::info!(
                        "EVENTS LOG: Commit for round {}: root_hash={:?}",
                        self.current_round,
                        signed_block.get_root_hash()
                    );
                }

                let signed_tl_block = self.get_block_candidate(signed_block.get_id()).clone();
                let source_idx = signed_block.get_source_index();
                let validator_public_key =
                    self.description.get_source_public_key(source_idx).clone();
                let priority = self
                    .virtual_state
                    .get_current_round_node_priority(&self.description, source_idx);

                let source_info = crate::BlockSourceInfo {
                    source: validator_public_key,
                    priority: crate::BlockCandidatePriority {
                        round: self.current_round,
                        first_block_round: self.first_block_round,
                        priority,
                    },
                };

                if let Some((signed_tl_block, _signed_block_creation_time)) = signed_tl_block {
                    //normal signed block

                    self.notify_block_committed(
                        source_info.clone(),
                        signed_block.get_root_hash(),
                        signed_block.get_file_hash(),
                        &catchain::CatchainFactory::create_block_payload(
                            signed_tl_block.data().clone(),
                        ),
                        signatures,
                        approve_signatures,
                    );
                } else {
                    //empty signed block

                    self.notify_block_committed(
                        source_info,
                        signed_block.get_root_hash(),
                        signed_block.get_file_hash(),
                        &catchain::CatchainFactory::create_empty_block_payload(),
                        signatures,
                        approve_signatures,
                    );
                }

                // Update first_block_round when block is committed
                self.first_block_round = self.current_round + 1;
            } else {
                //no block was committed

                log::trace!("...block is skipped for round {}", self.current_round);

                self.notify_block_skipped(self.current_round);

                //reset precollations to prevent round vs block id mismatch

                self.reset_precollations();
            }

            //remove obsolete round blocks payloads because we have already processed it

            self.blocks.borrow_mut().retain(|_, &mut (ref _block, round, _)| {
                round as i32 >= self.current_round as i32 - MAX_PAST_ROUND_BLOCK
            });

            //remove obsolete precollated blocks

            self.remove_precollated_block(self.current_round);

            //increment round

            self.current_round += 1;

            if DEBUG_EVENTS_LOG {
                log::info!("EVENTS LOG: New round {}", self.current_round);
            }
        }

        //update accelerated consensus collator index

        if self.description.is_accelerated_consensus_enabled() {
            let new_collator_idx =
                self.virtual_state.get_current_accelerated_consensus_collator_index();

            if new_collator_idx != self.accelerated_consensus_current_collator_idx {
                log::info!(
                    "Accelerated consensus mode: rotating collator on source #{} from {:?} to {:?}",
                    self.get_local_idx(),
                    self.accelerated_consensus_current_collator_idx,
                    new_collator_idx
                );
                self.accelerated_consensus_current_collator_idx = new_collator_idx;
            }

            if let Some(collator_idx) = self.accelerated_consensus_current_collator_idx {
                if collator_idx == self.get_local_idx() {
                    //dump precollated blocks

                    if log::log_enabled!(log::Level::Debug) {
                        let blocks_info: Vec<String> = self
                            .precollated_blocks
                            .iter()
                            .map(|(round, precollated_block)| {
                                let status = if precollated_block.candidate.is_some() {
                                    "ready"
                                } else {
                                    "pending"
                                };
                                format!("{}:{}", round, status)
                            })
                            .collect();
                        log::debug!(
                            "Precollated blocks dump (current round: {}): [{}]",
                            self.current_round,
                            blocks_info.join(", ")
                        );
                    }

                    //initiate precollation for current round

                    self.precollate_block(self.current_round);
                }
            }
        }

        //update debug checking time points

        self.round_started_at = self.description.get_time();
        self.round_debug_at = self.round_started_at + ROUND_DEBUG_PERIOD;

        //check state

        self.check_all();
    }

    fn get_current_max_block_delay(&self) -> std::time::Duration {
        if self.description.is_accelerated_consensus_enabled() {
            if self.description.is_in_past(self.round_started_at + LONG_ROUND_PERIOD) {
                return self.catchain_max_block_delay_slow;
            } else {
                return self.catchain_max_block_delay;
            }
        }

        let attempt_id = self.real_state.get_current_round_attempt_number(&self.description);
        let max_round_attempts = self.description.opts().max_round_attempts;
        let max_round_slow_attempts = max_round_attempts + 4;

        if attempt_id <= max_round_attempts {
            return self.catchain_max_block_delay;
        }

        if attempt_id >= max_round_slow_attempts {
            return self.catchain_max_block_delay_slow;
        }

        self.catchain_max_block_delay
            + std::time::Duration::from_secs_f64(
                (self.catchain_max_block_delay_slow - self.catchain_max_block_delay).as_secs_f64()
                    * (attempt_id - max_round_attempts) as f64
                    / (max_round_slow_attempts - max_round_attempts) as f64,
            )
    }

    fn request_new_block(&mut self, now: bool) {
        instrument!();

        if self.requested_new_block_now {
            //ignore double attempts to generate new block immediately

            return;
        }

        if !now && self.requested_new_block {
            //ignore double attemts to generate new block

            return;
        }

        log::trace!("...request new block from a catchain");

        //generate new block request to a catchain

        self.requested_new_block = true;

        let mut block_generation_time = SystemTime::now();

        if now {
            self.requested_new_block_now = true;
        } else if !DEBUG_REQUEST_NEW_BLOCKS_IMMEDIATELY {
            //calculate timeout when new block should be generated

            let lambda = 10.0 / (self.description.get_total_nodes() as f64);
            let delta_secs = -1.0 / lambda
                * f64::ln((self.description.generate_random_usize() % 999 + 1) as f64 * 0.001);
            let mut delta_secs = Duration::from_secs_f64(delta_secs);
            let current_max_block_delay = self.get_current_max_block_delay();

            if delta_secs > current_max_block_delay {
                delta_secs = current_max_block_delay;
            }

            if !self.description.is_accelerated_consensus_enabled() {
                let round_duration = self.round_started_at.elapsed();

                if let Ok(round_duration) = round_duration {
                    if round_duration > ROUND_DEBUG_PERIOD {
                        log::trace!(
                            "Session {} round #{} is too long (duration is {:.3}s, \
                            max expected duration is {:.3}s). Calming down",
                            self.session_id.to_hex_string(),
                            self.real_state.get_current_round_sequence_number(),
                            round_duration.as_secs_f64(),
                            ROUND_DEBUG_PERIOD.as_secs_f64()
                        );

                        if !self.description.is_accelerated_consensus_enabled() {
                            // difference from C++ node timeouts - slow down blocks generation in case of hanged consensus
                            delta_secs = std::cmp::max(delta_secs, HANGED_CONSENSUS_UPDATE_TIME);
                        }
                    }
                }
            }

            block_generation_time += delta_secs;
        }

        self.request_new_block_counter.increment(1);

        self.catchain.request_new_block(block_generation_time);
    }

    /*
        Attempts management
    */

    fn check_action(&mut self, attempt: u32) {
        instrument!();

        if !self.catchain_started {
            return;
        }

        if self.requested_new_block {
            return;
        }

        use ton_api::ton::validator_session::round::*;

        let action =
            self.virtual_state.create_action(&self.description, self.get_local_idx(), attempt);

        if let Some(action) = action {
            match action {
                Message::ValidatorSession_Message_Empty(_) => {
                    //do nothing - no changes detected
                }
                _ => {
                    //request new blocks in case if actions is not Empty
                    self.request_new_block(false);
                }
            }
        }
    }

    /*
        Blocks generation management
    */

    fn set_block_candidate(
        &mut self,
        block_id: &BlockId,
        data: (BlockCandidateTlPtr, std::time::SystemTime),
    ) {
        let block_round = *data.0.round() as u32;
        self.blocks.borrow_mut().insert(block_id.clone(), (data.0, block_round, data.1));
    }

    fn update_block_candidate_round(&mut self, block_id: &BlockId, round: u32) {
        if let Some(block) = self.blocks.borrow_mut().get_mut(block_id) {
            let current_round = block.1;
            block.1 = std::cmp::max(current_round, round);
        }
    }

    fn get_block_candidate(
        &self,
        block_id: &BlockId,
    ) -> Option<(BlockCandidateTlPtr, std::time::SystemTime)> {
        if let Some(block) = self.blocks.borrow().get(block_id) {
            return Some((block.0.clone(), block.2));
        }

        None
    }

    fn ensure_candidate_unique(&mut self, source_idx: u32, round: u32, block_id: &BlockId) -> bool {
        if let Some(candidate) = self.source_round_candidate[source_idx as usize].get(&round) {
            if candidate != block_id {
                log::warn!(
                    "Node v{:03} ({}) already has candidate in round {}: {:x}",
                    source_idx,
                    self.description.get_source_adnl_id(source_idx),
                    round,
                    block_id
                );
                return false;
            }
        }

        self.source_round_candidate[source_idx as usize].insert(round, block_id.clone());

        true
    }

    fn get_current_round_block_generation_priority_and_time(&self) -> (i32, SystemTime) {
        let priority = self
            .virtual_state
            .get_current_round_node_priority(&self.description, self.get_local_idx());

        let block_generation_time = if DEBUG_IGNORE_PROPOSALS_PRIORITY {
            self.description.get_time()
        } else {
            self.round_started_at + self.description.get_delay(priority as u32)
        };

        (priority, block_generation_time)
    }

    fn check_generate_slot(&mut self) {
        instrument!();

        //don't do anything until catchain is started

        if !self.catchain_started {
            return;
        }

        //don't generate block if it has been already generated in this round

        if self.generated || self.pending_generate {
            return;
        }

        //don't generate block if it has been sent already according to a state of this validator

        if self.real_state.check_block_is_sent_by(self.get_local_idx()) {
            self.generated = true;
            self.sent_generated = true;
            return;
        }

        let (priority, block_generation_time) =
            self.get_current_round_block_generation_priority_and_time();

        //check if we have a priority to generate block in current round

        if priority < 0 && !DEBUG_IGNORE_PROPOSALS_PRIORITY {
            return;
        }

        log::trace!("...block generation priority is {}", priority);

        if DEBUG_IGNORE_PROPOSALS_PRIORITY {
            log::warn!("...DEBUG_IGNORE_PROPOSALS_PRIORITY is enabled");
        }

        if self.description.is_in_future(block_generation_time) {
            self.set_next_awake_time(block_generation_time);
            return;
        }

        log::trace!(
            "...generating new block with priority {} at {}",
            priority,
            catchain::utils::time_to_string(&block_generation_time)
        );

        self.pending_generate = true;

        let round = self.current_round;

        //check if block was precollated in a pipeline mode

        self.collates_precollated_counter.total_increment();

        let precollated_block = self.precollated_blocks.get(&round);

        if let Some(precollated_block) = precollated_block {
            if let Some(candidate) = &precollated_block.candidate {
                log::trace!(
                    "SessionProcessor::check_generate_slot: \
                    precollated block has been found for round {}: {:?}",
                    round,
                    candidate
                );

                self.collates_precollated_counter.success();

                //invoke block candidate processing

                self.generated_block(
                    round,
                    candidate.id.root_hash.clone(),
                    candidate.data.clone(),
                    candidate.collated_data.clone(),
                );

                //precollate next block

                self.precollate_block(round + 1);

                return;
            }
        }

        self.collates_precollated_counter.failure();

        self.invoke_collation(round);
    }

    fn remove_precollated_block(&mut self, round: u32) {
        if let Some(_candidate) = self.precollated_blocks.remove(&round) {
            log::trace!(
                "SessionProcessor::remove_precollated_block: \
                removing precollated block for round {round}"
            );
            self.precollation_results_counter.increment(1);
        }
    }

    fn reset_precollations(&mut self) {
        log::debug!(
            "SessionProcessor::reset_precollations: resetting precollations on round {}",
            self.current_round
        );

        //cancel already launched precollations

        for (_round, precollated_block) in self.precollated_blocks.iter() {
            precollated_block.request.cancel();
        }

        //reset precollation state

        self.precollated_blocks.clear();
        self.precollated_blocks_max_round = None;
    }

    fn precollate_block(&mut self, mut round: u32) {
        if !self.description.is_accelerated_consensus_enabled() {
            return;
        }

        if self.precollated_blocks.len()
            >= self.description.opts().accelerated_consensus_max_precollated_blocks as usize
        {
            log::trace!(
                "SessionProcessor::precollate_block: precollated blocks limit {} reached, \
                dropping precollation request (round is {round}, current round is {})",
                self.description.opts().accelerated_consensus_max_precollated_blocks,
                self.current_round
            );
            return;
        }

        if self.precollated_blocks.get(&round).is_some() {
            if let Some(precollated_blocks_max_round) = self.precollated_blocks_max_round {
                if let Some(precollated_block) =
                    self.precollated_blocks.get(&precollated_blocks_max_round)
                {
                    if precollated_block.candidate.is_some() {
                        //all blocks in a pipeline are precollated / being precollated
                        //change round of precollations to next after max precollated round

                        let prev_round = round;

                        round = precollated_blocks_max_round + 1;

                        log::trace!(
                            "SessionProcessor::precollate_block: block for round {prev_round} is \
                            already being precollated, updating precollated block round to {round}"
                        );
                    }
                }
            }
        }

        self.invoke_collation(round);
    }

    fn invoke_collation(&mut self, round: u32) {
        //check if block is pending due to precollate request

        if self.precollated_blocks.get(&round).is_some() {
            log::trace!(
                "SessionProcessor::invoke_collation: block for round {round} is pending \
                due to precollate request, skipping collation"
            );
            return;
        }

        if round != self.current_round {
            log::trace!(
                "SessionProcessor::precollate_block: precollating block for round {round} \
                (current round is {}; {} precollation blocks left in pipeline)",
                self.current_round,
                self.description.opts().accelerated_consensus_max_precollated_blocks as usize
                    - self.precollated_blocks.len()
                    - 1
            );
        }

        let priority = self
            .virtual_state
            .get_current_round_node_priority(&self.description, self.get_local_idx());

        if priority < 0 && !DEBUG_IGNORE_PROPOSALS_PRIORITY {
            log::trace!(
                "SessionProcessor::invoke_collation: node has no priority to precollate block \
                for round {round}, skipping precollation"
            );
            return;
        }

        if self.precollated_blocks_max_round.is_none()
            || round > self.precollated_blocks_max_round.unwrap()
        {
            self.precollated_blocks_max_round = Some(round);
        }

        let request_id = self.precollated_blocks_next_request_id;
        let request = AsyncRequestImpl::new(request_id, true);
        let precollated_block = PrecollatedBlock { candidate: None, request: request.clone() };

        self.precollated_blocks_next_request_id += 1;

        self.precollated_blocks.insert(round, precollated_block);

        self.precollation_requests_counter.increment(1);

        //send block generation request to a collator

        const MAX_GENERATION_TIME: std::time::Duration = std::time::Duration::from_millis(1000);
        let start_generation_time = std::time::SystemTime::now();
        let collation_latency_histogram = self.collation_latency_histogram.clone();
        let request_clone = request.clone();

        let completion_handler = create_completion_handler(
            self,
            move |result: Result<ValidatorBlockCandidatePtr>, processor| {
                if request_clone.is_cancelled() {
                    log::warn!(
                        "SessionProcessor::invoke_collation: block for round {round} has been \
                        cancelled, skipping collation (collation request ID: {request_id})"
                    );
                    return;
                }

                let generation_duration = get_elapsed_time(&start_generation_time);

                collation_latency_histogram.record(generation_duration.as_millis() as f64);

                if generation_duration > MAX_GENERATION_TIME {
                    log::warn!(
                        "Execution time {:.3}ms for block generation in round {round} \
                        with collation request ID {request_id} is greater \
                        than expected time {:.3}ms at {}({})",
                        generation_duration.as_secs_f64() * 1000.0,
                        MAX_GENERATION_TIME.as_secs_f64() * 1000.0,
                        file!(),
                        line!()
                    );
                }

                let processor = get_mut_impl(processor);

                match result {
                    Ok(candidate) => {
                        log::trace!(
                            "SessionProcessor::check_generate_slot: new block candidate \
                            in round {round} with collation request ID {request_id} \
                            has been generated {candidate:?}"
                        );

                        processor.collates_counter.success();

                        if round == processor.current_round {
                            //process block collated for current round

                            let (priority, block_generation_time) =
                                processor.get_current_round_block_generation_priority_and_time();

                            if priority >= 0 || DEBUG_IGNORE_PROPOSALS_PRIORITY {
                                let block_generation_time_end = block_generation_time
                                    + processor.description.opts().next_candidate_delay;

                                if let Ok(offset) = block_generation_time_end.elapsed() {
                                    log::warn!(
                                        "Block generation time slot in round {round} \
                                        with collation request ID {request_id} \
                                        has been expired by {:.3}ms at {}({})",
                                        offset.as_secs_f64() * 1000.0,
                                        file!(),
                                        line!()
                                    );

                                    processor.collates_expire_counter.success();
                                } else {
                                    processor.collates_expire_counter.failure();
                                }

                                processor.generated_block(
                                    round,
                                    candidate.id.root_hash.clone(),
                                    candidate.data.clone(),
                                    candidate.collated_data.clone(),
                                );
                            }
                        } else if round > processor.current_round {
                            if let Some(precollated_candidate) =
                                processor.precollated_blocks.get_mut(&round)
                            {
                                if precollated_candidate.candidate.is_some() {
                                    log::error!(
                                        "SessionProcessor::check_generate_slot: \
                                        precollated candidate for round {round} \
                                        with collation request ID {request_id} is not None! \
                                        Precollation pipeline is broken!"
                                    );
                                    unreachable!();
                                }

                                //replace precollated candidate with new one

                                precollated_candidate.candidate = Some(candidate);

                                //TODO: initiate early validations of the block (prevalidations similar to precollations) to speed up round time
                            } else {
                                log::error!(
                                    "SessionProcessor::check_generate_slot: \
                                    precollated candidate for round {round} is not found! \
                                    Precollation pipeline is broken!"
                                );
                                unreachable!();
                            }
                        } else {
                            log::warn!(
                                "SessionProcessor::check_generate_slot: \
                                collated candidate for round {round} \
                                is expired (current round is {})",
                                processor.current_round
                            );

                            //remove precollated block from pipeline

                            processor.remove_precollated_block(round);

                            //do not precollate blocks for obsolete rounds
                            return;
                        }

                        //request to precollate next block

                        processor.precollate_block(round + 1);
                    }
                    Err(err) => {
                        processor.collates_counter.failure();

                        log::warn!(
                            "SessionProcessor::check_generate_slot: \
                            failed to generate block candidate: {:?}",
                            err
                        );

                        if processor.description.is_accelerated_consensus_enabled() {
                            //try to collate block again after timeout

                            let retry_time = SystemTime::now()
                                + processor
                                    .description
                                    .opts()
                                    .accelerated_consensus_collation_retry_timeout;

                            processor.post_delayed_action(
                                retry_time,
                                move |processor: &mut dyn SessionProcessor| {
                                    let processor = get_mut_impl(processor);
                                    let precollated_block =
                                        processor.precollated_blocks.get(&round);

                                    if round < processor.current_round {
                                        log::warn!(
                                            "SessionProcessor::check_generate_slot: \
                                            collation in round {round} failed, \
                                            but current round is {}, skipping",
                                            processor.current_round
                                        );
                                    } else if processor.precollated_blocks_max_round.is_some()
                                        && round != processor.precollated_blocks_max_round.unwrap()
                                    {
                                        log::warn!(
                                            "SessionProcessor::check_generate_slot: \
                                            collation in round {round} failed, \
                                            but round is not the max precollated round, skipping"
                                        );
                                    } else if precollated_block.is_some()
                                        && precollated_block.unwrap().candidate.is_some()
                                    {
                                        log::warn!(
                                            "SessionProcessor::check_generate_slot: \
                                            collation in round {round} failed, \
                                            but block was already precollated, skipping"
                                        );
                                    } else {
                                        log::warn!(
                                            "SessionProcessor::check_generate_slot: \
                                            collation in round {round} failed, \
                                            retrying after timeout"
                                        );

                                        //remove precollated block from pipeline
                                        processor.remove_precollated_block(round);

                                        //invoke collation again
                                        processor.invoke_collation(round);
                                    }
                                },
                            );
                        }
                    }
                }
            },
        );

        let priority = self
            .virtual_state
            .get_current_round_node_priority(&self.description, self.get_local_idx());
        let source_info = crate::BlockSourceInfo {
            source: self.description.get_source_public_key(self.get_local_idx()).clone(),
            priority: crate::BlockCandidatePriority {
                round,
                first_block_round: self.first_block_round,
                priority,
            },
        };

        self.notify_generate_slot(source_info, request.clone(), completion_handler);
    }

    fn generated_block(
        &mut self,
        round: u32,
        root_hash: BlockId,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
    ) {
        instrument!();

        //remove precollated block from pipeline

        self.remove_precollated_block(round);

        if round != self.current_round {
            //accept blocks only for current round

            return;
        }

        log::trace!(
            "SessionProcessor::generated_block: \
            candidate has been received for round={}, root_hash={:?}",
            round,
            root_hash
        );

        if DEBUG_EVENTS_LOG {
            log::info!(
                "EVENTS LOG: New block candidate has been generated for \
                round {}: root_hash={:?}, data_size={}, collated_data_size={}",
                round,
                root_hash,
                data.data().len(),
                collated_data.data().len()
            );
        }

        if data.data().len() > self.description.opts().max_block_size as usize
            || collated_data.data().len() > self.description.opts().max_collated_data_size as usize
        {
            log::error!(
                "SessionProcessor::generated_block: \
                generated candidate is too big. Dropping. size={}/{}",
                data.data().len(),
                collated_data.data().len()
            );
            return;
        }

        //prepare data

        use ton_api::ton::validator_session::*;

        let candidate_creation_time =
            if data.get_creation_time() < collated_data.get_creation_time() {
                data.get_creation_time()
            } else {
                collated_data.get_creation_time()
            };
        let file_hash = catchain::utils::get_hash_from_block_payload(&data);
        let collated_data_file_hash = catchain::utils::get_hash_from_block_payload(&collated_data);
        let candidate = candidate::Candidate {
            src: UInt256::with_array(*self.get_local_id().data()),
            round: round as i32,
            root_hash: root_hash.clone(),
            data: data.data().clone(),
            collated_data: collated_data.data().clone(),
        };
        let serialized_block =
            match serialize_block_candidate(candidate.clone(), self.compress_block_candidates) {
                Ok(data) => data,
                Err(err) => {
                    log::error!(
                    "SessionProcessor::generated_block: failed to serialize block candidate: {:?}",
                    err
                );
                    return;
                }
            };
        let block_id = self.description.candidate_id(
            self.get_local_idx(),
            &root_hash,
            &file_hash,
            &collated_data_file_hash,
        );

        //send broadcast to catchain about new block candidate

        self.catchain.send_broadcast(serialized_block.clone());

        // Setup retry mechanism if configured
        if self.description.opts().block_candidate_sending_retry_attempts > 0 {
            self.schedule_block_candidate_sending_retry(
                round,
                serialized_block.clone(),
                0, // Start with attempt 0
            );
        }

        //save block and update state

        self.set_block_candidate(
            &block_id,
            (Rc::new(candidate.into_boxed()), candidate_creation_time),
        );

        self.pending_generate = false;
        self.generated = true;
        self.generated_block = block_id;

        //request new block from the catchain

        self.request_new_block(true);
    }

    // Block candidate sending retry logic
    fn schedule_block_candidate_sending_retry(
        &mut self,
        block_candidate_round: u32,
        serialized_block: BlockPayloadPtr,
        current_attempt: u32,
    ) {
        let retry_timeout = self.description.opts().block_candidate_sending_retry_timeout;
        let retry_attempts = self.description.opts().block_candidate_sending_retry_attempts;
        let expiration_time = std::time::SystemTime::now() + retry_timeout;

        log::trace!(
            "SessionProcessor::schedule_block_candidate_sending_retry: \
            scheduling block candidate sending retry for round {block_candidate_round} \
            (current attempt is {current_attempt}/{retry_attempts}; timeout is {}ms)",
            retry_timeout.as_millis()
        );

        self.post_delayed_action(expiration_time, move |processor| {
            let processor = get_mut_impl(processor);
            let current_round = processor.current_round;

            // check if we're in the same round

            if current_round > block_candidate_round {
                // Round has already passed, no need to retry
                log::trace!(
                    "SessionProcessor::schedule_block_candidate_sending_retry: \
                    block candidate round {block_candidate_round} has passed \
                    (current={current_round}), stopping retries"
                );
                return;
            }

            if current_round < block_candidate_round {
                // Wait for the round to catch up - schedule another retry
                log::trace!(
                    "SessionProcessor::schedule_block_candidate_sending_retry: \
                    waiting for block candidate round {block_candidate_round} \
                    (current={current_round}), scheduling another retry"
                );
                processor.schedule_block_candidate_sending_retry(
                    block_candidate_round,
                    serialized_block,
                    current_attempt,
                );
                return;
            }

            // current_round == round, proceed with retry logic

            if current_attempt >= retry_attempts {
                log::trace!(
                    "SessionProcessor::schedule_block_candidate_sending_retry: \
                    max retry attempts ({retry_attempts}) reached \
                    for block candidate round {block_candidate_round}"
                );
                return;
            }

            let next_attempt = current_attempt + 1;

            log::trace!(
                "SessionProcessor::schedule_block_candidate_sending_retry: \
                retrying block candidate sending for round {block_candidate_round} \
                (attempt {next_attempt})"
            );

            processor.catchain.send_broadcast(serialized_block.clone());

            // schedule next retry if we still have attempts left

            if next_attempt < retry_attempts {
                processor.schedule_block_candidate_sending_retry(
                    block_candidate_round,
                    serialized_block,
                    next_attempt,
                );
            }
        });
    }

    /*
        Approval management
    */

    fn check_approve(&mut self) {
        instrument!();

        //don't do anything until catchain is started

        if !self.catchain_started {
            return;
        }

        //choose blocks to approve from proposed candidates

        let to_approve =
            self.virtual_state.choose_blocks_to_approve(&self.description, self.get_local_idx());

        log::trace!("block to approve {:?}", &to_approve);

        if !self.first_candidate_received && !to_approve.is_empty() {
            log::trace!("first candidate has been received in round #{}", self.current_round);

            self.first_candidate_received = true;

            if let Ok(latency) = self.get_latency_from_round_start() {
                self.first_candidate_received_latency_histogram.record(latency.as_millis() as f64);
            }
        }

        for block in to_approve {
            self.try_approve_block(block);
        }
    }

    fn try_approve_block(&mut self, block: SentBlockPtr) {
        instrument!();

        let block_id = block.get_id();

        //check if this block has been already approved

        if let Some((approve_time, _block)) = self.approved.get(block_id) {
            if approve_time <= &self.description.get_time() {
                self.request_new_block(false);
            } else {
                //awake when block will be approved (approved block may be valid from some specified by validator time)

                let approve_time = *approve_time; //make Rust happy about immutable / mutable borrowing

                self.set_next_awake_time(approve_time);
            }

            return;
        }

        //accelerated consensus mode: check only one block is approved in this round

        if self.description.is_accelerated_consensus_enabled() {
            if !self.approved.is_empty()
                || !self
                    .virtual_state
                    .get_blocks_approved_by(&self.description, self.get_local_idx())
                    .is_empty()
            {
                log::trace!(
                    "...block {:?} can't be approved because another block has been \
                    already approved in round {}",
                    block.get_id(),
                    self.current_round
                );
                return;
            }

            if !self.pending_approve.is_empty() {
                log::trace!(
                    "...block {:?} can't be approved because another block is \
                    pending for approval in round {}",
                    block.get_id(),
                    self.current_round
                );
                return;
            }
        }

        log::trace!("...try to approve block {:?} in round {}", block_id, self.current_round);

        //check if block has been waiting for approval or been rejected

        if self.pending_approve.contains(block_id) || self.rejected.contains(block_id) {
            log::trace!("...block {:?} is waiting for approval", block_id);
            return;
        }

        //compute block proposal delay according to block's source validator priority in this round

        let block_round_proposal_delay = match &block {
            Some(block) => self.description.get_delay(
                self.virtual_state
                    .get_current_round_node_priority(&self.description, block.get_source_index())
                    as u32,
            ),
            _ => self.description.get_empty_block_delay(),
        };
        let block_proposal_time = self.round_started_at + block_round_proposal_delay;

        if self.description.is_in_future(block_proposal_time) {
            //wait till block will be valid or approval

            log::trace!(
                "...block should be proposed later in round {} at {}",
                self.current_round,
                catchain::utils::time_to_string(&block_proposal_time)
            );

            self.set_next_awake_time(block_proposal_time);
            return;
        }

        //skip approval of empty block

        if block.is_none() {
            log::trace!(
                "...empty block will be automatically approved in round {}",
                self.current_round
            );

            self.approved.insert(
                block_id.clone(),
                (SystemTime::UNIX_EPOCH, catchain::CatchainFactory::create_empty_block_payload()),
            );
            self.request_new_block(false);
            return;
        }

        let block = block.as_ref().unwrap();

        if !self.ensure_candidate_unique(block.get_source_index(), self.current_round, &block_id) {
            return;
        }

        //check validation attempt index

        if let Some(validation_attempt_index) = self.validation_attempt_map.get(&block_id) {
            if *validation_attempt_index >= self.description.opts().validation_retry_attempts {
                log::trace!(
                    "...block {block_id:?} won't be validated because maximum number of validation \
                    attempts has been reached \
                    (attempt index={validation_attempt_index}, maximum number of attempts={})",
                    self.description.opts().validation_retry_attempts
                );
                return;
            }
        }

        let request_broadcast_p2p_delay = if self.description.is_accelerated_consensus_enabled() {
            self.description.opts().block_candidate_sending_retry_timeout * 2
        } else {
            Duration::from_secs(2)
        };

        let block_proposal_time = self.round_started_at
            + self.description.get_delay(block.get_source_index())
            + request_broadcast_p2p_delay;

        log::trace!(
            "...searching for block {:?} payload for round {}",
            block_id,
            self.current_round
        );

        let tl_block_opt: Option<(BlockCandidateTlPtr, std::time::SystemTime)> =
            self.get_block_candidate(block_id);

        //if block was proposed in current round - validate it

        if let Some((tl_block, broadcast_creation_time)) = tl_block_opt {
            self.update_block_candidate_round(block_id, self.current_round);

            log::trace!("...validating block {:?} for round {}", tl_block, self.current_round);

            let Some(collated_data) = tl_block.collated_data() else {
                log::trace!("...block contains no collated data (compressed?)");
                return;
            };
            let collated_data_len = collated_data.len();

            self.pending_approve.insert(block_id.clone());
            self.validation_attempt_map
                .entry(block_id.clone())
                .and_modify(|count| *count += 1)
                .or_insert(0);

            let round = self.current_round;
            let hash = block_id.clone();
            let root_hash = block.get_root_hash().clone();
            let file_hash = block.get_file_hash().clone();

            const MAX_VALIDATION_TIME: std::time::Duration = std::time::Duration::from_millis(750);
            let start_validation_time = std::time::SystemTime::now();
            let session_processor_creation_time = self.session_processor_creation_time;
            let session_creation_time = self.session_creation_time;
            let block_creation_time = block.get_source_block_creation_time();
            let block_payload_creation_time = block.get_source_block_payload_creation_time();
            let sent_block_creation_time = block.get_creation_time();
            let tl_block_clone = tl_block.clone();
            let validation_latency_histogram = self.validation_latency_histogram.clone();
            let block_candidate_broadcast_validation_latency_histogram =
                self.block_candidate_broadcast_validation_latency_histogram.clone();

            let backtrace = if DEBUG_DUMP_BACKTRACE_FOR_LATE_VALIDATIONS {
                Some(backtrace::Backtrace::new())
            } else {
                None
            };

            let completion_handler = create_completion_handler(self, move |result, processor| {
                let validation_duration = get_elapsed_time(&start_validation_time);
                let broadcast_processing_duration = get_elapsed_time(&broadcast_creation_time);

                if let Err(ref err) = &result {
                    let source_id: PublicKeyHash =
                        catchain::utils::int256_to_public_key_hash(tl_block_clone.src());
                    let source_idx = match processor.get_description().get_source_index(&source_id)
                    {
                        Ok(idx) => format!("v{idx:03}"),
                        Err(_err) => "v???".to_string(),
                    };

                    if DEBUG_EVENTS_LOG {
                        log::info!(
                            "EVENTS LOG: Validation failed for round {}: \
                                root_hash={:x}, data_size={}, collated_data_size={}",
                            round,
                            tl_block_clone.root_hash(),
                            tl_block_clone.data().len(),
                            collated_data_len
                        );
                    }

                    log::warn!(
                        "Validation failed for block {:?} with verdict {:?} \
                            (round={}, source={} ({}), full_processing_time={:.3}ms, \
                            expected_processing_time={:.3}ms, validation_time={:.3}ms, \
                            sent_block_creation_time={:.3}ms, block_creation_time={:.3}ms, \
                            block_payload_creation_time={:.3}ms, session_duration={:.3}s/{:.3}s) \
                            at {}({}); {}",
                        &tl_block_clone.root_hash(),
                        err,
                        round,
                        source_idx,
                        source_id,
                        broadcast_processing_duration.as_secs_f64() * 1000.0,
                        MAX_VALIDATION_TIME.as_secs_f64() * 1000.0,
                        validation_duration.as_secs_f64() * 1000.0,
                        get_elapsed_time(&sent_block_creation_time).as_secs_f64() * 1000.0,
                        get_elapsed_time(&block_creation_time).as_secs_f64() * 1000.0,
                        get_elapsed_time(&block_payload_creation_time).as_secs_f64() * 1000.0,
                        get_elapsed_time(&session_creation_time).as_secs_f64(),
                        get_elapsed_time(&session_processor_creation_time).as_secs_f64(),
                        file!(),
                        line!(),
                        if DEBUG_DUMP_BACKTRACE_FOR_LATE_VALIDATIONS {
                            format!("{:?}", backtrace)
                        } else {
                            "".to_string()
                        }
                    );

                    if validation_duration > MAX_VALIDATION_TIME {
                        log::warn!(
                            "Execution time {:.3}ms for validation is \
                                greater than expected time {:.3}ms at {}({})",
                            validation_duration.as_secs_f64() * 1000.0,
                            MAX_VALIDATION_TIME.as_secs_f64() * 1000.0,
                            file!(),
                            line!()
                        );
                    }

                    if broadcast_processing_duration > MAX_VALIDATION_TIME {
                        log::warn!(
                            "Execution time {:.3}ms for full block processing during \
                                validation is greater than expected time {:.3}ms (round={}, \
                                validation_time={:.3}ms, sent_block_creation_time={:.3}ms, \
                                block_creation_time={:.3}ms, block_payload_creation_time={:.3}ms, \
                                session_duration={:.3}s/{:.3}s) at {}({})",
                            broadcast_processing_duration.as_secs_f64() * 1000.0,
                            MAX_VALIDATION_TIME.as_secs_f64() * 1000.0,
                            round,
                            validation_duration.as_secs_f64() * 1000.0,
                            get_elapsed_time(&sent_block_creation_time).as_secs_f64() * 1000.0,
                            get_elapsed_time(&block_creation_time).as_secs_f64() * 1000.0,
                            get_elapsed_time(&block_payload_creation_time).as_secs_f64() * 1000.0,
                            get_elapsed_time(&session_creation_time).as_secs_f64(),
                            get_elapsed_time(&session_processor_creation_time).as_secs_f64(),
                            file!(),
                            line!(),
                        );
                    }
                } else if DEBUG_EVENTS_LOG {
                    log::info!(
                        "EVENTS LOG: Validation succeed for round {}: \
                            root_hash={:x}, data_size={}, collated_data_size={}",
                        round,
                        tl_block_clone.root_hash(),
                        tl_block_clone.data().len(),
                        collated_data_len
                    );
                }

                validation_latency_histogram.record(validation_duration.as_millis() as f64);
                block_candidate_broadcast_validation_latency_histogram
                    .record(broadcast_processing_duration.as_millis() as f64);

                let processor = get_mut_impl(processor);
                let result = if processor.description.is_accelerated_consensus_enabled() {
                    if !processor.approved.is_empty()
                        || !processor
                            .virtual_state
                            .get_blocks_approved_by(
                                &processor.description,
                                processor.get_local_idx(),
                            )
                            .is_empty()
                    {
                        Err(error!("Block {:?} can't be approved because another block has been already approved in round {}", hash, processor.current_round))
                    } else {
                        result
                    }
                } else {
                    result
                };

                match result {
                    Ok(validity_start_time) => processor.candidate_decision_ok(
                        round,
                        hash,
                        root_hash,
                        file_hash,
                        validity_start_time,
                    ),
                    Err(err) => processor.candidate_decision_fail(round, hash, err),
                }
            });

            let source_public_key =
                self.description.get_source_public_key(block.get_source_index()).clone();

            if DEBUG_EVENTS_LOG {
                log::info!(
                    "EVENTS LOG: Validating block candidate for round {round}: \
                    root_hash={:x}, data_size={}, collated_data_size={collated_data_len}, \
                    validation_attempt_index={}",
                    tl_block.root_hash(),
                    tl_block.data().len(),
                    self.validation_attempt_map.get(&block_id).unwrap_or(&0),
                );
            }

            if self.description.opts().skip_single_node_session_validations
                && self.description.get_total_nodes() == 1
                && block.get_source_index() == self.get_local_idx()
            {
                //special case - auto approve of self blocks for single node sessions
                completion_handler(Ok(SystemTime::now()));
            } else {
                let source_idx = block.get_source_index();
                let priority = self
                    .virtual_state
                    .get_current_round_node_priority(&self.description, source_idx);
                let source_info = crate::BlockSourceInfo {
                    source: source_public_key,
                    priority: crate::BlockCandidatePriority {
                        round,
                        first_block_round: self.first_block_round,
                        priority,
                    },
                };

                self.notify_candidate(
                    source_info,
                    &tl_block.root_hash().clone(),
                    &catchain::CatchainFactory::create_block_payload(tl_block.data().clone()),
                    &catchain::CatchainFactory::create_block_payload(collated_data.clone()),
                    completion_handler,
                );
            }

            return;
        }

        //if block was not proposed in current round but it's proposal time is in past - request block

        if self.description.is_in_past(block_proposal_time) {
            if self.active_requests.contains(block_id) {
                return;
            }

            log::trace!("...request absent block {:?} for round {}", block_id, self.current_round);

            let approvers = self.virtual_state.get_block_approvers(&self.description, block_id);

            if approvers.is_empty() {
                log::trace!(
                    "...block {:?} has not been aproved by any node yet in round {}",
                    block_id,
                    self.current_round
                );
                return;
            }

            let node_index = self.description.generate_random_usize() % approvers.len();
            let node_adnl_id = self.description.get_source_adnl_id(approvers[node_index]).clone();
            let source_id =
                self.description.get_source_public_key_hash(block.get_source_index()).clone();

            self.active_requests.insert(block_id.clone());

            const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

            let block_id_clone = block_id.clone();
            let node_adnl_id_clone = node_adnl_id.clone();
            let source_id_clone = source_id.clone();
            let round = self.current_round;

            self.get_broadcast_p2p(
                &node_adnl_id,
                block.get_file_hash(),
                block.get_collated_data_file_hash(),
                &source_id,
                self.current_round,
                block.get_root_hash(),
                self.description.get_time() + DOWNLOAD_TIMEOUT,
                move |result: Result<BlockPayloadPtr>, processor: &mut dyn SessionProcessor| {
                    let processor = get_mut_impl(processor);

                    if processor.current_round == round {
                        processor.active_requests.remove(&block_id_clone);
                    }

                    if let Err(err) = result {
                        processor.rldp_queries_counter.failure();

                        log::warn!(
                            "Failed to get block candidate {:?} from node {}: {:?}",
                            block_id_clone,
                            node_adnl_id_clone,
                            err
                        );
                        return;
                    }

                    processor.rldp_queries_counter.success();

                    processor.process_broadcast(
                        source_id_clone,
                        result.ok().unwrap(),
                        Some(block_id_clone),
                        false,
                    );
                },
            );

            return;
        }

        //wait until block proposal time will come

        log::trace!(
            "...wait until the next block proposal at {} (current time is {})",
            catchain::utils::time_to_string(&block_proposal_time),
            catchain::utils::time_to_string(&self.description.get_time())
        );

        self.set_next_awake_time(block_proposal_time);
    }

    fn get_broadcast_p2p<F>(
        &mut self,
        node_adnl_id: &PublicKeyHash,
        file_hash: &BlockHash,
        collated_data_file_hash: &BlockHash,
        source: &PublicKeyHash,
        round: u32,
        root_hash: &BlockHash,
        timeout: std::time::SystemTime,
        complete_handler: F,
    ) where
        F: FnOnce(Result<BlockPayloadPtr>, &mut dyn SessionProcessor) + 'static,
    {
        instrument!();

        if self.description.is_in_past(timeout) {
            complete_handler(Err(error!("get_broadcast_p2p timeout")), self);
            return;
        }

        let download_candidate = ton::DownloadCandidate {
            round: round as ton::int,
            id: ton::candidateid::CandidateId {
                src: catchain::utils::public_key_hash_to_int256(source),
                root_hash: root_hash.clone(),
                file_hash: file_hash.clone(),
                collated_data_file_hash: collated_data_file_hash.clone(),
            },
        };

        const MAX_CANDIDATE_EXTRA_SIZE: u32 = 1024; //max candidate extra size

        let serialized_download_candidate = serialize_tl_boxed_object!(&download_candidate);
        let serialized_download_candidate =
            catchain::CatchainFactory::create_block_payload(serialized_download_candidate);
        let max_answer_size = self.description.opts().max_block_size
            + self.description.opts().max_collated_data_size
            + MAX_CANDIDATE_EXTRA_SIZE;
        let response_callback = create_completion_handler(self, move |result, processor| {
            complete_handler(result, processor);
        });

        self.rldp_queries_counter.total_increment();

        self.catchain.send_query_via_rldp(
            node_adnl_id.clone(),
            "download candidate".to_string(),
            response_callback,
            timeout,
            serialized_download_candidate,
            max_answer_size as u64,
            true, // use RLDPv2
        );
    }

    fn candidate_decision_ok(
        &mut self,
        round: u32,
        hash: BlockId,
        root_hash: BlockHash,
        file_hash: BlockHash,
        validity_start_time: SystemTime,
    ) {
        instrument!();

        self.validates_counter.success();

        if round != self.current_round {
            return;
        }

        log::trace!("SessionProcessor::candidate_decision_ok: approved candidate {:?}", hash);

        use ton_api::ton::*;

        let data = serialize_tl_boxed_object!(&ton::blockid::BlockIdApprove {
            root_cell_hash: root_hash,
            file_hash
        }
        .into_boxed());

        match self.get_local_key().sign(&data) {
            Err(err) => log::error!(
                "SessionProcessor::candidate_decision_ok: failed to sign blockId {:?}: {:?}",
                data,
                err
            ),
            Ok(signature) => {
                self.candidate_approved_signed(round, hash, validity_start_time, signature.to_vec())
            }
        }
    }

    fn candidate_decision_fail(&mut self, round: u32, hash: BlockId, err: Error) {
        instrument!();

        self.validates_counter.failure();

        if round != self.current_round {
            return;
        }

        let mut reason = format!("{}", err);

        //attempt to validate the block again

        if let Some(validation_attempt_index) = self.validation_attempt_map.get(&hash) {
            let validation_attempt_index = *validation_attempt_index;
            if validation_attempt_index < self.description.opts().validation_retry_attempts {
                let retry_timeout = self.description.opts().validation_retry_timeout;
                let expiration_time = SystemTime::now() + retry_timeout;

                log::error!(
                    "SessionProcessor::candidate_decision_fail: failed candidate {hash:?}, \
                    validation_attempt_index={validation_attempt_index}/{}, reason={reason:?}. \
                    Will retry in {}ms.",
                    self.description.opts().validation_retry_attempts,
                    retry_timeout.as_millis()
                );

                self.post_delayed_action(expiration_time, move |processor| {
                    let processor = get_mut_impl(processor);

                    log::trace!(
                        "Allow to validate block {:?} again (attempt index={})",
                        hash,
                        validation_attempt_index + 1
                    );

                    if round != processor.current_round {
                        return;
                    }

                    processor.pending_approve.remove(&hash);
                });

                return;
            }
        }

        log::error!(
            "SessionProcessor::candidate_decision_fail: failed candidate {hash:?}, \
            no validation attempts left, reason={reason:?}"
        );

        self.pending_approve.remove(&hash);

        const MAX_REJECT_REASON_SIZE: usize = 1024; //max reject reason size

        if reason.len() > MAX_REJECT_REASON_SIZE {
            reason = reason[..MAX_REJECT_REASON_SIZE].to_string();
        }

        self.pending_reject.insert(
            hash.clone(),
            catchain::CatchainFactory::create_block_payload(reason.as_bytes().to_vec()),
        );
        self.rejected.insert(hash);
    }

    fn candidate_approved_signed(
        &mut self,
        _round: u32,
        hash: BlockId,
        validity_start_time: SystemTime,
        signature: BlockSignature,
    ) {
        instrument!();

        self.pending_approve.remove(&hash);

        self.approved.insert(
            hash.clone(),
            (validity_start_time, catchain::CatchainFactory::create_block_payload(signature)),
        );

        if validity_start_time <= self.description.get_time() {
            self.request_new_block(false);
        } else {
            log::warn!(
                "SessionProcessor::candidate_approved_signed: too new block {:?} with validity_start_time={:?}",
                hash,
                validity_start_time
            );
            self.set_next_awake_time(validity_start_time);
        }
    }

    /*
        Voting management
    */

    fn check_vote_for_slot(&mut self, attempt: u32) {
        instrument!();

        if !self.catchain_started {
            return;
        }

        if self.virtual_state.check_need_generate_vote_for(
            &self.description,
            self.get_local_idx(),
            attempt,
        ) {
            self.request_new_block(false);
        }
    }

    /*
        Commit management
    */

    fn check_sign_slot(&mut self) {
        instrument!();

        //if catchain is not started, there is nothing to do

        if !self.catchain_started {
            return;
        }

        //prevent second signing if we are already pending for signature

        if self.pending_sign {
            return;
        }

        //check if we have signed block

        if self.real_state.check_block_is_signed_by(self.get_local_idx()) {
            self.signed = true;
            return;
        }

        //if we block has been signed, request catchain for a new one

        if self.signed {
            self.request_new_block(false);
            return;
        }

        //choose block for signing

        let commit_candidate =
            self.virtual_state.choose_block_to_sign(&self.description, self.get_local_idx());

        if commit_candidate.is_none() {
            return;
        }

        let commit_candidate = commit_candidate.unwrap();

        //check if we are trying to sign empty block

        if commit_candidate.is_none() {
            log::trace!("...signing empty block");

            self.signed = true;
            self.signed_block = SKIP_ROUND_CANDIDATE_BLOCKID.clone();

            self.request_new_block(false);

            return;
        }

        //block signing

        let commit_candidate = commit_candidate.unwrap();

        log::trace!("...signing block {:?}", commit_candidate);

        self.pending_sign = true;

        //serialize block ID

        let block_id = ton::blockid::BlockId {
            root_cell_hash: commit_candidate.get_root_hash().clone(),
            file_hash: commit_candidate.get_file_hash().clone(),
        }
        .into_boxed();
        let block_id_serialized = serialize_tl_boxed_object!(&block_id);

        //sign serialized block ID

        let sign_result = self.get_local_key().sign(&block_id_serialized);

        if let Err(err) = sign_result {
            log::error!("...block signing error: {:?}", err);
            return;
        }

        //further process of signed block

        let block_signature = sign_result.ok().unwrap().to_vec();

        self.signed_block(self.current_round, commit_candidate.get_id().clone(), block_signature);
    }

    fn signed_block(&mut self, round: u32, hash: BlockId, signature: BlockSignature) {
        instrument!();

        if round != self.current_round {
            return;
        }

        //update state with signed block

        self.pending_sign = false;
        self.signed = true;
        self.signed_block = hash;
        self.signature = signature;

        //request new block from catchain

        self.request_new_block(false);
    }

    /*
        Listener management
    */

    fn notify_candidate(
        &mut self,
        source_info: crate::BlockSourceInfo,
        root_hash: &BlockHash,
        data: &BlockPayloadPtr,
        collated_data: &BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        check_execution_time!(20000);
        instrument!();

        log::trace!(
            "SessionProcessor::notify_candidate: \
            post on_candidate event for further processing"
        );

        let listener = self.session_listener.clone();
        let source_info_clone = source_info.clone();
        let root_hash_clone = root_hash.clone();
        let data_clone = data.clone();
        let collated_data_clone = collated_data.clone();

        self.validates_counter.total_increment();

        self.invoke_session_callback(move || {
            check_execution_time!(20000);

            if let Some(listener) = listener.upgrade() {
                log::trace!("SessionProcessor::notify_candidate: on_candidate start");

                listener.on_candidate(
                    source_info_clone,
                    root_hash_clone,
                    data_clone,
                    collated_data_clone,
                    callback,
                );

                log::trace!("SessionProcessor::notify_candidate: on_candidate finish");
            }
        });
    }

    fn notify_generate_slot(
        &mut self,
        source_info: crate::BlockSourceInfo,
        request: Arc<AsyncRequestImpl>,
        callback: ValidatorBlockCandidateCallback,
    ) {
        check_execution_time!(20000);
        instrument!();

        let round = source_info.priority.round;
        log::trace!("...post on_generate_slot event for further processing, {:x}", self.session_id);

        let listener = self.session_listener.clone();
        let source_info_clone = source_info.clone();
        let request_id = request.get_request_id();

        self.collates_counter.total_increment();
        self.collates_expire_counter.total_increment();

        self.invoke_session_callback(move || {
            check_execution_time!(20000);

            if let Some(listener) = listener.upgrade() {
                log::trace!(
                    "SessionProcessor::notify_generate_slot: on_generate_slot start \
                    for round {round} with request ID {request_id}"
                );

                listener.on_generate_slot(
                    source_info_clone,
                    request as crate::AsyncRequestPtr,
                    crate::CollationParentHint::Implicit,
                    callback,
                );

                log::trace!(
                    "SessionProcessor::notify_generate_slot: on_generate_slot finish \
                    for round {round} with request ID {request_id}"
                );
            }
        });
    }

    /// Build BlockSignaturesVariant::Ordinary from raw signature pairs
    ///
    /// Converts the raw signature data from catchain into a BlockSignaturesVariant
    /// suitable for the on_block_committed callback.
    ///
    /// # Arguments
    /// * `signatures` - Raw signature pairs from catchain (public key hash + signature payload)
    ///
    /// # Returns
    /// BlockSignaturesVariant::Ordinary with placeholder ValidatorBaseInfo (0, 0)
    /// The actual validator_info will be filled in accept_block.
    fn build_signatures_variant(
        signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) -> BlockSignaturesVariant {
        let mut pure_signatures = BlockSignaturesPure::new();

        for (public_key_hash, signature_payload) in signatures {
            // Extract the raw signature bytes from the payload
            let signature_data = signature_payload.data();

            // Try to parse as CryptoSignature (64 bytes for ed25519)
            let crypto_signature = if signature_data.len() >= 64 {
                CryptoSignature::from_bytes(&signature_data[..64]).unwrap_or_default()
            } else {
                CryptoSignature::default()
            };

            // Create signature pair with node_id_short from public key hash
            let node_id_short = UInt256::from_slice(public_key_hash.data());
            let sig_pair = CryptoSignaturePair::with_params(node_id_short, crypto_signature);
            pure_signatures.add_sigpair(sig_pair);
        }

        // Create BlockSignatures with placeholder ValidatorBaseInfo
        // The actual validator_info (catchain_seqno, validator_set_hash) will be
        // filled in accept_block when creating the block proof
        let block_signatures =
            BlockSignatures::with_params(ValidatorBaseInfo::with_params(0, 0), pure_signatures);

        BlockSignaturesVariant::Ordinary(block_signatures)
    }

    fn notify_block_committed(
        &mut self,
        source_info: crate::BlockSourceInfo,
        root_hash: &BlockHash,
        file_hash: &BlockHash,
        data: &BlockPayloadPtr,
        signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        check_execution_time!(20000);
        instrument!();

        log::trace!("...post on_block_committed event for further processing");

        let listener = self.session_listener.clone();
        let source_info_clone = source_info.clone();
        let root_hash_clone = root_hash.clone();
        let file_hash_clone = file_hash.clone();
        let data_clone = data.clone();

        // Convert raw signatures to BlockSignaturesVariant::Ordinary
        // Note: ValidatorBaseInfo uses placeholder values (0, 0) here.
        // The actual catchain_seqno and validator_list_hash_short will be
        // filled in by accept_block using the ValidatorSet.
        let signatures_variant = Self::build_signatures_variant(signatures);

        self.commits_counter.total_increment();
        self.commits_counter.success();

        self.invoke_session_callback(move || {
            check_execution_time!(20000);

            if let Some(listener) = listener.upgrade() {
                log::trace!("SessionProcessor::notify_block_committed: on_block_committed start");

                listener.on_block_committed(
                    source_info_clone,
                    root_hash_clone,
                    file_hash_clone,
                    data_clone,
                    signatures_variant,
                    approve_signatures,
                    crate::ValidatorSessionStats::default(),
                );

                log::trace!("SessionProcessor::notify_block_committed: on_block_committed finish");
            }
        });
    }

    fn notify_block_skipped(&mut self, round: u32) {
        check_execution_time!(20000);
        instrument!();

        log::trace!("...post on_block_skipped event for further processing");

        let listener = self.session_listener.clone();

        self.commits_counter.total_increment();
        self.commits_counter.failure();

        self.invoke_session_callback(move || {
            check_execution_time!(20000);

            if let Some(listener) = listener.upgrade() {
                log::trace!("SessionProcessor::notify_block_skipped: on_block_skipped start");

                listener.on_block_skipped(round);

                log::trace!("SessionProcessor::notify_block_skipped: on_block_skipped finish");
            }
        });
    }

    fn notify_get_approved_candidate(
        &mut self,
        source: &PublicKey,
        root_hash: &BlockHash,
        file_hash: &BlockHash,
        collated_data_hash: &BlockHash,
        callback: ValidatorBlockCandidateCallback,
    ) {
        check_execution_time!(20000);
        instrument!();

        log::trace!("...post get_approved_candidate event for further processing");

        let listener = self.session_listener.clone();
        let source_clone = source.clone();
        let root_hash_clone = root_hash.clone();
        let file_hash_clone = file_hash.clone();
        let collated_data_hash_clone = collated_data_hash.clone();

        self.invoke_session_callback(move || {
            check_execution_time!(20000);

            if let Some(listener) = listener.upgrade() {
                log::trace!(
                    "SessionProcessor::notify_get_approved_candidate: \
                    get_approved_candidate start"
                );

                listener.get_approved_candidate(
                    source_clone,
                    root_hash_clone,
                    file_hash_clone,
                    collated_data_hash_clone,
                    callback,
                );

                log::trace!(
                    "SessionProcessor::notify_get_approved_candidate: \
                    get_approved_candidate finish"
                );
            }
        });
    }

    /*
        Callback management
    */

    /// Invoke callback closure - checks use_callback_thread flag
    /// and either posts to callback queue or executes immediately
    fn invoke_session_callback<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.use_callback_thread {
            // Use callback thread - post to callback task queue
            post_callback_closure(&self.callbacks_task_queue, callback);
        } else {
            // Execute callback immediately in current thread
            callback();
        }
    }

    /*
        Creation
    */

    pub(crate) fn create(
        options: SessionOptions,
        session_id: SessionId,
        ids: Vec<SessionNode>,
        local_key: PrivateKey,
        listener: SessionListenerPtr,
        catchain: CatchainPtr,
        completion_task_queue: TaskQueuePtr,
        callbacks_task_queue: CallbackTaskQueuePtr,
        session_creation_time: std::time::SystemTime,
        metrics: Option<catchain::utils::MetricsHandle>,
    ) -> SessionProcessorPtr {
        //dump session params for further log replaying

        if log::log_enabled!(log::Level::Debug) {
            #[cfg(feature = "export_key")]
            let exp_pvt_key_dump = hex::encode(local_key.export_key().unwrap());
            #[cfg(not(feature = "export_key"))]
            let exp_pvt_key_dump = "<SECRET>".to_string();

            log::debug!(
                "Create validator session {} for local ID {} and key {} (timestamp={})",
                session_id.to_hex_string(),
                &hex::encode(local_key.id().data()),
                exp_pvt_key_dump,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_millis()
            );

            for node in &ids {
                let elapsed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_else(|_| Duration::new(0, 0))
                    .as_millis();
                let key: ton_api::ton::PublicKey = (&node.public_key).try_into().unwrap();
                log::debug!(
                    "Validator session {} node: weight={}, public_key={}, \
                    adnl_id={} (timestamp={})",
                    session_id.to_hex_string(),
                    node.weight,
                    &hex::encode(&catchain::serialize_tl_boxed_object!(&key)),
                    &hex::encode(node.adnl_id.data()),
                    elapsed
                );
            }
        }

        //compute accelerated consensus initial collator index

        let accelerated_consensus_initial_collator_index = if options.accelerated_consensus_enabled
        {
            let session_id_bytes = session_id.as_slice();
            let session_id_crc32 = crc32_digest(session_id_bytes);
            let total_nodes = ids.len() as u32;
            session_id_crc32 % total_nodes
        } else {
            0
        };

        //create child objects

        let local_id = local_key.id().clone();
        let mut description = SessionDescriptionImpl::new(
            &options,
            &ids,
            &local_id,
            accelerated_consensus_initial_collator_index,
            metrics,
        );

        //initialize metrics

        let metrics_receiver = description.get_metrics_receiver();

        let collates_counter = ResultStatusCounter::new(metrics_receiver, "collate_requests");
        let collates_expire_counter =
            ResultStatusCounter::new(metrics_receiver, "collate_requests_expire");
        let collates_precollated_counter =
            ResultStatusCounter::new(metrics_receiver, "collate_requests_precollated");
        let validates_counter = ResultStatusCounter::new(metrics_receiver, "validate_requests");
        let commits_counter = ResultStatusCounter::new(metrics_receiver, "commit_requests");
        let rldp_queries_counter = ResultStatusCounter::new(metrics_receiver, "rldp_queries");
        let preprocess_block_counter =
            metrics_receiver.sink().register_counter(&"preprocess_block_calls".into());
        let process_blocks_counter =
            metrics_receiver.sink().register_counter(&"process_blocks_calls".into());
        let request_new_block_counter =
            metrics_receiver.sink().register_counter(&"request_new_block_calls".into());
        let check_all_counter = metrics_receiver.sink().register_counter(&"check_all_calls".into());
        let preprocess_block_latency_histogram =
            metrics_receiver.sink().register_histogram(&"time:preprocess_block_latency".into());
        let process_blocks_latency_histogram =
            metrics_receiver.sink().register_histogram(&"time:process_blocks_latency".into());
        let first_candidate_received_latency_histogram = metrics_receiver
            .sink()
            .register_histogram(&"time:round_stage1_received_latency".into());
        let first_candidate_approved_latency_histogram = metrics_receiver
            .sink()
            .register_histogram(&"time:round_stage2_approved_latency".into());
        let first_candidate_voted_latency_histogram =
            metrics_receiver.sink().register_histogram(&"time:round_stage3_voted_latency".into());
        let first_candidate_precommitted_latency_histogram = metrics_receiver
            .sink()
            .register_histogram(&"time:round_stage4_precommitted_latency".into());
        let round_duration_histogram = metrics_receiver
            .sink()
            .register_histogram(&"time:round_stage5_committed_latency".into());
        let block_candidate_broadcast_validation_latency_histogram = metrics_receiver
            .sink()
            .register_histogram(&"time:block_candidate_broadcast_validation_latency".into());
        let validation_latency_histogram =
            metrics_receiver.sink().register_histogram(&"time:validation_latency".into());
        let collation_latency_histogram =
            metrics_receiver.sink().register_histogram(&"time:collation_latency".into());
        let active_weight_gauge = metrics_receiver.sink().register_gauge(&"active_weight".into());
        let total_weight_gauge = metrics_receiver.sink().register_gauge(&"total_weight".into());
        let cutoff_weight_gauge = metrics_receiver.sink().register_gauge(&"cutoff_weight".into());
        let precollation_requests_counter =
            metrics_receiver.sink().register_counter(&"precollation_requests".into());
        let precollation_results_counter =
            metrics_receiver.sink().register_counter(&"precollation_results".into());

        total_weight_gauge.set(description.get_total_weight() as f64);
        cutoff_weight_gauge.set(description.get_cutoff_weight() as f64);

        //initialize state

        let now = SystemTime::now();
        let state_merge_cache = FifoCache::new("state_merge".to_owned(), metrics_receiver);
        let block_update_cache = FifoCache::new("block_update".to_owned(), metrics_receiver);
        let initial_state = SessionFactory::create_state(&mut description);
        let initial_state = initial_state.move_to_persistent(&mut description);
        let accelerated_consensus_current_collator_idx =
            if description.is_accelerated_consensus_enabled() {
                log::info!(
                "Accelerated consensus mode is enabled! Initial collator index on source #{} is 0",
                description.get_self_idx()
            );
                Some(description.get_accelerated_consensus_initial_collator_index())
            } else {
                log::info!("Accelerated consensus mode is disabled!");
                None
            };

        const COMPRESS_BLOCK_CANDIDATES_VERSION: u32 = 4;

        let body = Self {
            session_id,
            local_key,
            completion_task_queue,
            callbacks_task_queue,
            session_listener: listener,
            catchain,
            next_completion_handler_available_index: 1,
            completion_handlers: HashMap::new(),
            completion_handlers_check_last_time: SystemTime::now(),
            delayed_actions: Vec::new(),
            block_to_state_map: Vec::with_capacity(STATES_RESERVED_COUNT),
            state_merge_cache,
            block_update_cache,
            catchain_started: false,
            round_started_at: description.get_time(),
            round_debug_at: description.get_time() + ROUND_DEBUG_PERIOD,
            compress_block_candidates: description.opts().proto_version
                >= COMPRESS_BLOCK_CANDIDATES_VERSION,
            description,
            catchain_max_block_delay: DEFAULT_CATCHAIN_MAX_BLOCK_DELAY,
            catchain_max_block_delay_slow: DEFAULT_CATCHAIN_MAX_BLOCK_DELAY_SLOW,
            real_state: initial_state.clone(),
            virtual_state: initial_state.clone(),
            current_round: 0,
            first_block_round: 0,
            accelerated_consensus_current_collator_idx,
            next_awake_time: now,
            session_processor_creation_time: now,
            session_creation_time,
            requested_new_block_now: false,
            requested_new_block: false,
            pending_generate: false,
            generated: false,
            sent_generated: false,
            generated_block: BlockId::default(),
            precollated_blocks: HashMap::new(),
            precollated_blocks_next_request_id: 0,
            precollated_blocks_max_round: None,
            precollation_requests_counter,
            precollation_results_counter,
            blocks: Rc::new(RefCell::new(HashMap::new())),
            source_round_candidate: vec![HashMap::new(); ids.len()],
            validation_attempt_map: HashMap::new(),
            pending_approve: HashSet::new(),
            pending_reject: HashMap::new(),
            rejected: HashSet::new(),
            approved: HashMap::new(),
            active_requests: HashSet::new(),
            pending_sign: false,
            signed: false,
            signed_block: BlockId::default(),
            signature: BlockSignature::default(),
            log_replay_report_current_time: std::time::UNIX_EPOCH,
            collates_counter,
            collates_expire_counter,
            collates_precollated_counter,
            validates_counter,
            commits_counter,
            rldp_queries_counter,
            preprocess_block_counter,
            process_blocks_counter,
            request_new_block_counter,
            preprocess_block_latency_histogram,
            process_blocks_latency_histogram,
            round_duration_histogram,
            first_candidate_received_latency_histogram,
            first_candidate_received: false,
            first_candidate_approved_latency_histogram,
            first_candidate_approved: false,
            first_candidate_voted_latency_histogram,
            first_candidate_voted: false,
            first_candidate_precommitted_latency_histogram,
            first_candidate_precommitted: false,
            block_candidate_broadcast_validation_latency_histogram,
            validation_latency_histogram,
            collation_latency_histogram,
            check_all_counter,
            last_preprocess_block_warn_dump_time: now,
            last_process_blocks_warn_dump_time: now,
            last_preprocess_block_time: vec![SystemTime::UNIX_EPOCH; ids.len()],
            active_weight_gauge,
            use_callback_thread: options.use_callback_thread,
        };

        if body.compress_block_candidates {
            log::warn!(
                "Compressing block candidates is enabled \
                (should be tested for compatibility with C++ node; \
                especially for BOC serialization flags)"
            );
        }

        if DEBUG_EVENTS_LOG {
            log::info!("EVENTS LOG: New round {}", body.current_round);
        }

        //check state

        let result = Rc::new(RefCell::new(body));

        result.borrow_mut().check_all();

        result
    }
}
