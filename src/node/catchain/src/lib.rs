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

/// Modules (local)
mod block;
mod catchain;
mod database;
mod received_block;
mod receiver;
mod receiver_source;
pub mod utils;

use crate::utils::MetricsHandle;
use adnl::{node::AdnlNode, NetworkStack, PrivateOverlayShortId};
// Re-export macros from consensus-common
pub use consensus_common::check_execution_time;
/// Modules re-exported from consensus-common
pub use consensus_common::profiling;
// Re-export traits with Catchain prefix for backward compatibility
pub use consensus_common::ConsensusNode as CatchainNode;
pub use consensus_common::{
    instrument, serialize_tl_bare_object, serialize_tl_boxed_object,
    ConsensusOverlay as CatchainOverlay, ConsensusOverlayListener as CatchainOverlayListener,
    ConsensusOverlayListenerPtr as CatchainOverlayListenerPtr,
    ConsensusOverlayLogReplayListener as CatchainOverlayLogReplayListener,
    ConsensusOverlayLogReplayListenerPtr as CatchainOverlayLogReplayListenerPtr,
    ConsensusOverlayManager as CatchainOverlayManager,
    ConsensusOverlayManagerPtr as CatchainOverlayManagerPtr,
    ConsensusOverlayPtr as CatchainOverlayPtr, ConsensusReplayListener as CatchainReplayListener,
    ConsensusReplayListenerPtr as CatchainReplayListenerPtr, LossyOverlayOpts,
};
// Re-export common types from consensus-common
pub use consensus_common::{
    ActivityNode, ActivityNodePtr, BlockHash, BlockPayload, BlockPayloadPtr, BlockSignature,
    ConsensusCommonFactory, LogPlayer, LogPlayerPtr, LogReplayOptions, PrivateKey, PublicKey,
    PublicKeyHash, QueryResponseCallback, RawBuffer, Result, SessionId, ValidatorWeight,
};
use std::{cell::RefCell, fmt, path::Path, rc::Rc, sync::Arc, time::SystemTime};

// ============================================================================
// Catchain-specific Types
// ============================================================================

pub type DatabasePtr = Arc<dyn Database>;

/// Overlay ID
pub type OverlayId = PublicKeyHash;

/// Overlay full ID
pub type OverlayFullId = SessionId;

/// Height of the block
pub type BlockHeight = i32;

/// Block extra data identifier (is used by validator session to match blocks and states)
pub type BlockExtraId = u64;

/// Pointer to internal ReceivedBlock implementation
pub(crate) type ReceivedBlockPtr = Rc<RefCell<received_block::ReceivedBlock>>;

/// Pointer to Block
pub type BlockPtr = Arc<dyn Block>;

/// Pointer to internal ReceiverSource implementation
pub(crate) type ReceiverSourcePtr = Rc<RefCell<receiver_source::ReceiverSource>>;

/// Pointer to Receiver
pub type ReceiverPtr = Arc<dyn Receiver>;

/// Pointer to ReceiverListener
pub type ReceiverListenerPtr = std::sync::Weak<dyn ReceiverListener + Send + Sync>;

/// Pointer to a Catchain
pub type CatchainPtr = Arc<dyn Catchain>;

/// Pointer to Catchain listener for validator session
pub type CatchainListenerPtr = std::sync::Weak<dyn CatchainListener + Send + Sync>;

/// Pointer to ADNL Node
pub type AdnlNodePtr = Arc<AdnlNode>;

pub mod ton {
    pub use ::ton_api::ton::{catchain::*, rpc::catchain::*};

    /// Catchain block ID
    pub type BlockId = block::Id;

    /// Catchain block dependency
    pub type BlockDep = block::dep::Dep;

    pub type BlockDepVec = Vec<::ton_api::ton::catchain::block::dep::Dep>;

    /// Catchain block data (internal structure)
    pub type BlockData = block::data::Data;

    /// Catchain block payload
    pub type BlockInnerData = block::inner::Data;

    /// Catchain block
    pub type Block = block::Block;

    /// Catchain first block
    pub type FirstBlock = firstblock::Firstblock;

    /// Block data fork
    pub type BlockDataFork = block::inner::catchain::block::data::data::Fork;

    /// Event which will be received as a response for GetBlockRequest, GetBlocksRequest
    pub type BlockUpdateEvent = blockupdate::BlockUpdate;

    /// Sent when no forks are detected
    pub type GetDifferenceResponse = Difference;

    /// Sent when forks are detected
    pub type DifferenceFork = difference::DifferenceFork;

    /// Response for GetBlockRequest which is sent if the block is found
    pub type BlockResultResponse = BlockResult;

    /// This query is used by the catchain component to request an absent block from another validator
    pub type GetBlockRequest = GetBlock;

    /// This is the initial request sent by one validator to another one to receive absent blocks
    pub type GetDifferenceRequest = GetDifference;
}

/// Catchain receiver options
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Timeout for catchain main loop procesing
    pub idle_timeout: std::time::Duration,

    /// Maximum number of dependencies for a block to merge
    pub max_deps: u32,

    /// Max serialized block size
    pub max_serialized_block_size: u32,

    /// Block hash covers data
    pub block_hash_covers_data: bool,

    /// Max block height = max_block_height_coeff * (1 + N / max_deps) / 1000
    /// N - number of participants
    /// 0 - unlimited
    pub max_block_height_coeff: u64,

    /// Should internal database be used for debugging
    pub debug_disable_db: bool,

    /// Check blocks processed by ValidatorSession but don't use them in Catchain DAG (for debugging and log replay)
    pub skip_processed_blocks: bool,

    /// Receiver: max number of neighbours to synchronize
    pub receiver_max_neighbours_count: usize,

    /// Receiver: min time for catchain sync with neighbour nodes
    pub receiver_neighbours_sync_min_period: std::time::Duration,

    /// Receiver: max time for catchain sync with neighbour nodes
    pub receiver_neighbours_sync_max_period: std::time::Duration,

    /// Receiver: max number of attempts to find a source to synchronize
    pub receiver_max_sources_sync_attempts: usize,

    /// Receiver: min time for catchain neighbours rotation
    pub receiver_neighbours_rotate_min_period: std::time::Duration,

    /// Receiver: max time for catchain neighbours rotation
    pub receiver_neighbours_rotate_max_period: std::time::Duration,

    /// Disable GOSSIP mode
    pub disable_gossip: bool,

    /// Enable TCP based communication
    pub allow_tcp_communication: bool,
}

/// State of the received block
#[derive(PartialEq, Copy, Clone, Debug)]
pub enum ReceivedBlockState {
    /// Block is not initialized
    Null,

    /// Block is a part of fork
    Ill,

    /// Block is initialized
    Initialized,

    /// Block is delivered
    Delivered,
}

/// Receiver which contains all receiver sources
pub trait Receiver: Send + Sync {
    /// Notify about blame processing state
    fn blame_processed(&self, source_id: usize);

    /// Adding new block
    fn add_block(&self, payload: BlockPayloadPtr, deps: Vec<BlockHash>);

    /// Adding new fork (for debugging)
    fn debug_add_fork(&self, payload: BlockPayloadPtr, height: BlockHeight, deps: Vec<BlockHash>);

    /// Send broadcast
    fn send_broadcast(&self, payload: BlockPayloadPtr);

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
    );

    /// Stop receiver
    fn stop(&self);

    /// Destroy DB
    fn destroy_db(&self);
}

/// Catchain block
pub trait Block: fmt::Display + fmt::Debug + Send + Sync {
    /// Block creation time
    fn get_creation_time(&self) -> std::time::SystemTime;

    /// Get block extra data ID
    fn get_extra_id(&self) -> BlockExtraId;

    /// Payload
    fn get_payload(&self) -> &BlockPayloadPtr;

    /// Receiver source identifier
    fn get_source_id(&self) -> u32;

    /// Fork ID
    fn get_fork_id(&self) -> usize;

    /// Receiver source public hey hash
    fn get_source_public_key_hash(&self) -> &PublicKeyHash;

    /// Block hash
    fn get_hash(&self) -> &BlockHash;

    /// Block height
    fn get_height(&self) -> BlockHeight;

    /// Previous block
    fn get_prev(&self) -> Option<BlockPtr>;

    /// Get dependency blocks
    fn get_deps(&self) -> &Vec<BlockPtr>;

    /// Mapping from fork index to block dependency height
    /// 0 if the block does not have dependency from specified fork
    fn get_forks_dep_heights(&self) -> &Vec<BlockHeight>;

    /// Is this block is descendat of specified one
    fn is_descendant_of(&self, block: &dyn Block) -> bool;
}

/// Database for blocks saving
pub trait Database: Send + Sync {
    /// Return path to db
    fn get_db_path(&self) -> &Path;

    /// Has block written to DB
    fn is_block_in_db(&self, hash: &BlockHash) -> bool;

    /// Get block from DB
    fn get_block(&self, hash: &BlockHash) -> Result<RawBuffer>;

    /// Push block to database
    fn put_block(&self, hash: &BlockHash, data: RawBuffer);

    /// Erase block from database
    fn erase_block(&self, hash: &BlockHash);

    /// Destroy DB (after drop)
    fn destroy(&self);
}

/// Listener for Receiver callbacks
pub trait ReceiverListener {
    /// Notification about receiver started
    fn on_started(&self);

    /// New block receiving event
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
    );

    /// Incoming broadcast processing
    fn on_broadcast(&self, source_key_hash: PublicKeyHash, data: BlockPayloadPtr);

    /// Source blame event
    fn on_blame(&self, source_id: usize);

    /// Custom query event
    fn on_custom_query(
        &self,
        source_public_key_hash: PublicKeyHash,
        data: BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    );

    /// Set timestamp for all further events
    fn on_set_time(&self, timestamp: std::time::SystemTime);
}

/// Listener for Catchain
pub trait CatchainListener {
    /// Preprocess block
    fn preprocess_block(&self, block: BlockPtr);

    /// Process blocks
    fn process_blocks(&self, blocks: Vec<BlockPtr>);

    /// Notify about finished of blocks processing
    fn finished_processing(&self);

    /// Notify about catchain start
    fn started(&self);

    /// Notify about incoming broadcasts
    fn process_broadcast(&self, source_id: PublicKeyHash, data: BlockPayloadPtr);

    /// Notify about incoming query
    fn process_query(
        &self,
        source_id: PublicKeyHash,
        data: BlockPayloadPtr,
        callback: QueryResponseCallback,
    );

    /// Set timestamp for all further events
    fn set_time(&self, timestamp: std::time::SystemTime);
}

/// Root class for Catchain processing
pub trait Catchain: Send + Sync {
    /// Request for a new block
    fn request_new_block(&self, time: SystemTime);

    /// Mark block as processed
    fn processed_block(&self, payload: BlockPayloadPtr, may_be_skipped: bool);

    /// Send broadcast
    fn send_broadcast(&self, payload: BlockPayloadPtr);

    /// Stop the Catchain (blocks until complete, preserves DB)
    fn stop(&self);

    /// Async stop the Catchain (non-blocking, preserves DB)
    fn stop_async(&self);

    /// Destroy the Catchain and its database (blocks until complete)
    fn destroy(&self);

    /// Send query via RLDP
    fn send_query_via_rldp(
        &self,
        dst: PublicKeyHash,
        name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    );

    /// Adding new fork (for debugging)
    fn debug_add_fork(&self, payload: BlockPayloadPtr, height: BlockHeight);
}

/// Catchain factory
pub struct CatchainFactory;

impl CatchainFactory {
    /// Create block payload
    pub fn create_block_payload(data: RawBuffer) -> BlockPayloadPtr {
        ConsensusCommonFactory::create_block_payload(data)
    }

    /// Create empty payload
    pub fn create_empty_block_payload() -> BlockPayloadPtr {
        ConsensusCommonFactory::create_empty_block_payload()
    }

    /// Create new block
    pub fn create_block(
        source_id: usize,
        fork_id: usize,
        source_public_key_hash: PublicKeyHash,
        height: BlockHeight,
        hash: BlockHash,
        payload: BlockPayloadPtr,
        prev_block: Option<BlockPtr>,
        deps: Vec<BlockPtr>,
        forks_dep_heights: Vec<BlockHeight>,
        extra_id: BlockExtraId,
    ) -> BlockPtr {
        block::BlockImpl::create(
            source_id,
            fork_id,
            source_public_key_hash,
            height,
            hash,
            payload,
            prev_block,
            deps,
            forks_dep_heights,
            extra_id,
        )
    }

    /// Create receiver
    pub fn create_receiver(
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
        receiver::ReceiverWrapper::create(
            session_id,
            options,
            listener,
            ids,
            local_key,
            path,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            overlay_manager,
        )
    }

    /// Create dummy overlay manager
    pub fn create_dummy_overlay_manager() -> CatchainOverlayManagerPtr {
        ConsensusCommonFactory::create_dummy_overlay_manager()
    }

    /// Create a lossy overlay manager that wraps a base overlay manager
    /// and simulates network packet loss for consensus testing.
    pub fn create_lossy_overlay_manager(
        base_overlay_manager: Arc<dyn CatchainOverlayManager + Send + Sync + 'static>,
        config: LossyOverlayOpts,
    ) -> CatchainOverlayManagerPtr {
        ConsensusCommonFactory::create_lossy_overlay_manager(base_overlay_manager, config)
    }

    /// Create in-process overlay manager
    pub fn create_in_process_overlay_manager(num_threads: usize) -> CatchainOverlayManagerPtr {
        ConsensusCommonFactory::create_in_process_overlay_manager(num_threads)
    }

    /// Create ADNL overlay manager
    pub fn create_adnl_overlay_manager(
        runtime_handle: tokio::runtime::Handle,
        stack: Arc<NetworkStack>,
        broadcast_hops: Option<u8>,
        track_private_peers: bool,
    ) -> Result<CatchainOverlayManagerPtr> {
        ConsensusCommonFactory::create_adnl_overlay_manager(
            runtime_handle,
            stack,
            broadcast_hops,
            track_private_peers,
        )
    }

    /// Create Catchain database
    pub fn create_database(
        path: String,
        name: String,
        metrics: &MetricsHandle,
    ) -> Result<DatabasePtr> {
        database::DatabaseImpl::create(&path, &name, metrics)
    }

    /// Create Catchain root object
    pub fn create_catchain(
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
        catchain::CatchainImpl::create(
            options,
            session_id,
            ids,
            local_key,
            path,
            db_suffix,
            allow_unsafe_self_blocks_resync,
            overlay_manager,
            listener,
        )
    }

    /// Create log replay object
    pub fn create_log_player(log_replay_options: &LogReplayOptions) -> Result<LogPlayerPtr> {
        ConsensusCommonFactory::create_log_player(log_replay_options)
    }

    /// Enumerate all log replay objects
    pub fn create_log_players(log_replay_options: &LogReplayOptions) -> Vec<LogPlayerPtr> {
        ConsensusCommonFactory::create_log_players(log_replay_options)
    }

    /// Create Catchain root object with log replaying overlay
    pub fn create_catchain_replay(
        options: &Options,
        log_replay_options: &LogReplayOptions,
        catchain_listener: CatchainListenerPtr,
        replay_listener: CatchainReplayListenerPtr,
    ) -> Result<CatchainPtr> {
        let player = Self::create_log_player(log_replay_options)?;

        Self::create_catchain(
            options,
            player.get_session_id(),
            player.get_nodes().to_vec(),
            player.get_local_key(),
            log_replay_options.db_path.clone(),
            log_replay_options.db_suffix.clone(),
            log_replay_options.allow_unsafe_self_blocks_resync,
            player.get_overlay_manager(replay_listener),
            catchain_listener,
        )
    }

    /// Create activity node
    pub fn create_activity_node(name: String) -> ActivityNodePtr {
        ConsensusCommonFactory::create_activity_node(name)
    }
}
