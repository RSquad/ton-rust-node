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

#[cfg(test)]
use crate::test_helper::compare_transactions;
use crate::{
    block::{BlockIdExtExtention, BlockStuff},
    engine_traits::EngineOperations,
    error::NodeError,
    shard_state::ShardStateStuff,
    types::{
        messages::{count_matching_bits, perform_hypercube_routing, MsgEnqueueStuff},
        top_block_descr::{Mode as TopBlockDescrMode, TopBlockDescrStuff},
    },
    validating_utils::{
        check_cur_validator_set, check_this_shard_mc_info, fmt_next_block_descr,
        may_update_shard_block_info, supported_capabilities, supported_version,
        UNREGISTERED_CHAIN_MAX_LEN,
    },
    validator::{
        collator::PREV_STATE_WAIT_TIMEOUT_MS,
        consensus::ResolverPurpose,
        out_msg_queue::{MsgQueueManager, StatesManager},
        state_resolver_cache::{self, StateResolverCache},
        validator_utils::calc_subset_for_masterchain,
        BlockCandidate, McData,
    },
    CHECK,
};
use adnl::common::add_unbound_object_to_map_with_update;
#[cfg(test)]
use std::fs;
use std::{
    collections::HashMap,
    mem,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
#[cfg(test)]
use ton_block::{base64_encode, write_boc, UsageTree};
use ton_block::{
    fail, read_boc, Account, AccountBlock, AccountDispatchQueue, AccountId, AccountIdPrefixFull,
    AccountStatus, AccountStorageDictProof, AddSub, Augmentation, Block, BlockCreateStats,
    BlockError, BlockExtra, BlockIdExt, BlockInfo, BlockLimits, Cell, CellType, Coins,
    ConfigParamEnum, ConfigParams, ConsensusExtraData, Counters, CreatorStats, CurrencyCollection,
    DepthBalanceInfo, Deserializable, EnqueuedMsg, FundamentalSmcAddresses, GlobalCapabilities,
    HashmapAugType, HashmapType, InMsg, InMsgDescr, KeyExtBlkRef, KeyMaxLt, LibDescr, Libraries,
    McBlockExtra, McShardRecord, McStateExtra, MerkleProof, MerkleUpdate, Message, MsgAddressInt,
    MsgEnvelope, MsgMetadata, OutMsg, OutMsgDescr, OutMsgQueueKey, Result, Serializable,
    ShardAccount, ShardAccountBlocks, ShardAccounts, ShardFeeCreated, ShardHashes, ShardIdent,
    ShardStateUnsplit, SizeLimitsConfig, SliceData, StateInitLib, TopBlockDescrSet, TrComputePhase,
    Transaction, TransactionDescr, UInt15, UInt256, ValidatorSet, ValueFlow, WorkchainDescr,
    INVALID_WORKCHAIN_ID, MASTERCHAIN_ID, MAX_SPLIT_DEPTH,
};
#[cfg(feature = "xp25")]
use ton_block::{ShardDescr, SHARD_FULL};
use ton_executor::{
    BlockchainConfig, ExecuteParams, OrdinaryTransactionExecutor, TickTockTransactionExecutor,
    TransactionExecutor,
};
use ton_vm::smart_contract_info::PrevBlocksInfo;
#[cfg(test)]
#[path = "tests/test_validate_query.rs"]
mod tests;

// pub const SPLIT_MERGE_DELAY: u32 = 100;        // prepare (delay) split/merge for 100 seconds
// pub const SPLIT_MERGE_INTERVAL: u32 = 100;     // split/merge is enabled during 60 second interval
pub const MIN_SPLIT_MERGE_INTERVAL: u32 = 30; // split/merge interval must be at least 30 seconds
pub const MAX_SPLIT_MERGE_DELAY: u32 = 1000; // end of split/merge interval must be at most 1000 seconds in the future

macro_rules! error {
    ($($arg:tt)*) => {
        ton_block::error!(NodeError::ValidatorReject(
            format!("=====> {}:{} {}", file!(), line!(), format_args!($($arg)*))
        ))
    };
}

macro_rules! reject_query {
    ($($arg:tt)*) => {
        return Err(error!($($arg)*))
    }
}

macro_rules! soft_reject_query {
    ($($arg:tt)*) => {
        return Err(ton_block::error!(NodeError::ValidatorSoftReject(
            format!("=====> {}:{} {}", file!(), line!(), format_args!($($arg)*))
        )))
    }
}

type LibPublisher = (UInt256, AccountId, bool);

struct ValidateResult {
    lt_hash: lockfree::map::Map<u8, (u64, UInt256)>,

    msg_proc_lt: lockfree::queue::Queue<(AccountId, u64, u64)>,
    msg_emitted_lt: lockfree::queue::Queue<(AccountId, u64, u64)>,
    lib_publishers: lockfree::queue::Queue<LibPublisher>,

    min_shard_ref_mc_seqno: AtomicU32,
    max_shard_utime: AtomicU32, // TODO: is never used
    max_shard_lt: AtomicU64,
    have_unprocessed_account_dispatch_queue: AtomicBool,
    removed_dispatch_queue_messages: lockfree::map::Map<(AccountId, u64), Cell>,
    new_dispatch_queue_messages: lockfree::map::Map<(AccountId, u64), Cell>,
    account_expected_defer_all_messages: lockfree::set::Set<AccountId>,
    blackhole_burned: Mutex<Coins>,
}

impl Default for ValidateResult {
    fn default() -> Self {
        let lt_hash = lockfree::map::Map::new();
        lt_hash.insert(0, (u64::MIN, UInt256::MAX));
        lt_hash.insert(1, (u64::MAX, UInt256::MAX));
        lt_hash.insert(2, (u64::MAX, UInt256::MAX)); // claimed_proc_lt_hash
        Self {
            lt_hash,
            msg_proc_lt: Default::default(),
            msg_emitted_lt: Default::default(),
            lib_publishers: Default::default(),
            min_shard_ref_mc_seqno: AtomicU32::new(u32::MAX),
            max_shard_utime: AtomicU32::new(u32::MIN),
            max_shard_lt: AtomicU64::new(u64::MIN),
            have_unprocessed_account_dispatch_queue: AtomicBool::new(false),
            removed_dispatch_queue_messages: lockfree::map::Map::new(),
            new_dispatch_queue_messages: lockfree::map::Map::new(),
            account_expected_defer_all_messages: lockfree::set::Set::new(),
            blackhole_burned: Mutex::new(Coins::default()),
        }
    }
}
#[derive(Default)]
struct ValidateBase {
    global_id: i32,
    is_fake: bool,
    is_simplex: bool,
    created_by: UInt256,
    after_merge: bool,
    after_split: bool,

    prev_blocks_ids: Vec<BlockIdExt>,

    // TODO: maybe make some fileds Option
    // data from block_candidate
    block: BlockStuff,
    info: BlockInfo,
    value_flow: ValueFlow,
    state_update: MerkleUpdate,
    extra: BlockExtra,
    mc_extra: McBlockExtra,
    in_msg_descr: InMsgDescr,
    total_imported_msgs: AtomicU32,
    out_msg_descr: OutMsgDescr,
    account_blocks: ShardAccountBlocks,
    recover_create_msg: Option<InMsg>,
    mint_msg: Option<InMsg>,
    // from collated_data
    top_shard_descr_dict: TopBlockDescrSet,
    virt_states: HashMap<UInt256, ShardStateUnsplit>, // prev state and neighbour out msg queues proofs by block root hash
    storage_dict_proofs: HashMap<UInt256, Cell>,
    full_collated_data: bool,
    now_ms: Option<u64>, // gen_utime_ms from ConsensusExtraData (simplex consensus)

    gas_used: Arc<AtomicU64>,
    transactions_executed: Arc<AtomicU32>,

    // master chain state members
    config_params: ConfigParams,
    block_limits: BlockLimits,
    limits: SizeLimitsConfig,
    special_smartcontracts: FundamentalSmcAddresses,

    prev_states: Vec<Arc<ShardStateStuff>>,
    prev_state: Option<Arc<ShardStateStuff>>, // TODO: remove
    prev_state_accounts: ShardAccounts,
    prev_state_extra: McStateExtra,
    prev_validator_fees: CurrencyCollection,

    // from next_state of masterchain or from global masterchain for workchain
    next_state: Option<Arc<ShardStateStuff>>,
    next_state_accounts: ShardAccounts,
    next_state_extra: McStateExtra,

    prev_blocks_info: PrevBlocksInfo,

    result: ValidateResult,

    next_block_descr: Arc<String>,

    #[cfg(test)]
    mc_usage_tree: UsageTree,
    #[cfg(test)]
    pure_root: Cell,
}

impl ValidateBase {
    fn shard(&self) -> &ShardIdent {
        self.block_id().shard()
    }
    fn block_id(&self) -> &BlockIdExt {
        self.block.id()
    }
    fn now(&self) -> u32 {
        self.info.gen_utime()
    }
    fn is_special_in_msg(&self, in_msg: &InMsg) -> bool {
        self.recover_create_msg.as_ref() == Some(in_msg) || self.mint_msg.as_ref() == Some(in_msg)
    }
    fn min_shard_ref_mc_seqno(&self) -> u32 {
        self.result.min_shard_ref_mc_seqno.load(Ordering::Relaxed)
    }
    fn max_shard_lt(&self) -> u64 {
        self.result.max_shard_lt.load(Ordering::Relaxed)
    }
    fn is_special_smartcontract(&self, addr: &AccountId) -> Result<bool> {
        Ok(self.special_smartcontracts.get_raw(addr.clone())?.is_some())
    }
    fn have_unprocessed_account_dispatch_queue(&self) -> bool {
        self.result.have_unprocessed_account_dispatch_queue.load(Ordering::Relaxed)
    }
    fn removed_dispatch_queue_messages(&self, addr: &AccountId, lt: u64) -> Option<Cell> {
        let k_v = self.result.removed_dispatch_queue_messages.remove(&(addr.clone(), lt))?;
        Some(k_v.val().clone())
    }
    fn new_dispatch_queue_messages(&self, addr: AccountId, lt: u64) -> Option<Cell> {
        let k_v = self.result.new_dispatch_queue_messages.remove(&(addr, lt))?;
        Some(k_v.val().clone())
    }
    fn prev_state(&self) -> Result<&ShardStateUnsplit> {
        self.prev_state
            .as_deref()
            .ok_or_else(|| error!("Prev state is not initialized in validator query"))?
            .state()
    }
    fn next_state(&self) -> Result<&ShardStateUnsplit> {
        self.next_state
            .as_deref()
            .ok_or_else(|| error!("Next state is not initialized in validator query"))?
            .state()
    }
}

pub struct ValidateQuery {
    // current state of blockchain
    shard: ShardIdent,
    min_mc_seqno: u32,
    // block_id: BlockIdExt,
    block_candidate: BlockCandidate,
    // other
    validator_set: ValidatorSet,
    is_fake: bool,
    multithread: bool,
    is_simplex: bool,
    // previous state can be as two states for merge
    prev_blocks_ids: Vec<BlockIdExt>,
    old_mc_shards: ShardHashes, // old_shard_conf_
    // new state after applying block_candidate
    new_mc_shards: ShardHashes, // new_shard_conf_
    // temp
    update_shard_cc: bool,

    create_stats_enabled: bool,
    block_create_total: u64,
    block_create_count: HashMap<UInt256, u64>,

    engine: Arc<dyn EngineOperations>,

    /// Simplex speculative state cache for notarized-but-not-yet-applied parents.
    state_resolver_cache: Option<Arc<tokio::sync::Mutex<StateResolverCache>>>,

    next_block_descr: Arc<String>,
}

type TasksVec = Vec<Box<dyn FnOnce() -> Result<()> + Send + 'static>>;

enum MsgProcessedStatus {
    NotProcessed,
    ProcessedInThisBlock,
    ProcessedInPreviousBlock,
}

impl ValidateQuery {
    fn shard(&self) -> &ShardIdent {
        &self.shard
    }

    async fn wait_prev_state_via_engine_or_cache(
        &self,
        prev_id: &BlockIdExt,
    ) -> Result<Arc<ShardStateStuff>> {
        match &self.state_resolver_cache {
            Some(cache) => {
                state_resolver_cache::wait_prev_state(
                    cache,
                    &self.engine,
                    prev_id,
                    ResolverPurpose::SimplexValidationParent,
                    PREV_STATE_WAIT_TIMEOUT_MS,
                )
                .await
            }
            None => {
                self.engine
                    .clone()
                    .wait_state(prev_id, Some(PREV_STATE_WAIT_TIMEOUT_MS), true)
                    .await
            }
        }
    }

    pub fn new(
        shard: ShardIdent,
        min_mc_seqno: u32,
        prev_blocks_ids: Vec<BlockIdExt>,
        state_resolver_cache: Option<Arc<tokio::sync::Mutex<StateResolverCache>>>,
        block_candidate: BlockCandidate,
        validator_set: ValidatorSet,
        engine: Arc<dyn EngineOperations>,
        is_fake: bool,
        multithread: bool,
        is_simplex: bool,
    ) -> Self {
        let next_block_descr = Arc::new(fmt_next_block_descr(&block_candidate.block_id));
        Self {
            engine,
            shard,
            min_mc_seqno,
            block_candidate,
            validator_set,
            is_fake,
            multithread,
            is_simplex,
            prev_blocks_ids,
            old_mc_shards: Default::default(),
            // new state after applying block_candidate
            new_mc_shards: Default::default(),
            update_shard_cc: Default::default(),
            create_stats_enabled: Default::default(),
            block_create_total: Default::default(),
            block_create_count: Default::default(),
            state_resolver_cache,
            next_block_descr,
        }
    }

    /*
     *
     *   INITIAL PARSE & LOAD REQUIRED DATA
     *
     */

    fn init_base(&mut self) -> Result<ValidateBase> {
        let mut base = ValidateBase {
            next_block_descr: self.next_block_descr.clone(),
            is_fake: self.is_fake,
            is_simplex: self.is_simplex,
            created_by: self.block_candidate.created_by.clone(),
            prev_blocks_ids: mem::take(&mut self.prev_blocks_ids),
            ..Default::default()
        };
        let block_id = &self.block_candidate.block_id;
        log::info!(
            target: "validate_query",
            "({}): validate query for {:#} started",
            self.next_block_descr,
            block_id
        );
        if block_id.shard() != self.shard() {
            soft_reject_query!(
                "block candidate belongs to shard {} different from current shard {}",
                block_id.shard(),
                self.shard()
            )
        }
        if !block_id.shard().is_masterchain() && !block_id.shard().is_standard_workchain() {
            soft_reject_query!(
                "can validate block candidates only for masterchain (-1) and base workchain (0) \
                and standard workchain (1-255)"
            )
        }
        if block_id.shard().is_masterchain() && base.prev_blocks_ids.is_empty() {
            self.min_mc_seqno = 0
        }
        match base.prev_blocks_ids.len() {
            2 => {
                if block_id.shard().is_masterchain() {
                    soft_reject_query!("cannot merge shards in masterchain")
                }
                if !(block_id.shard().is_parent_for(base.prev_blocks_ids[0].shard())
                    && block_id.shard().is_parent_for(base.prev_blocks_ids[1].shard())
                    && (base.prev_blocks_ids[0].shard().shard_prefix_with_tag()
                        < base.prev_blocks_ids[1].shard().shard_prefix_with_tag()))
                {
                    soft_reject_query!(
                        "the two previous blocks for a merge operation \
                        are not siblings or are not children of current shard"
                    )
                }
                for blk in &base.prev_blocks_ids {
                    if blk.seq_no == 0 {
                        soft_reject_query!(
                            "previous blocks for a block merge operation must have non-zero seqno"
                        )
                    }
                }
                base.after_merge = true;
            }
            1 => {
                // creating next block
                if base.prev_blocks_ids[0].shard() != block_id.shard() {
                    base.after_split = true;
                    if !base.prev_blocks_ids[0].shard().is_parent_for(block_id.shard()) {
                        soft_reject_query!(
                            "previous block does not belong to \
                            the shard we are generating a new block for"
                        )
                    }
                    if block_id.shard().is_masterchain() {
                        soft_reject_query!("cannot split shards in masterchain")
                    }
                }
                if block_id.shard().is_masterchain()
                    && self.min_mc_seqno > base.prev_blocks_ids[0].seq_no
                {
                    soft_reject_query!(
                        "cannot refer to specified masterchain block {} \
                        because it is later than {} the immediately preceding masterchain block",
                        self.min_mc_seqno,
                        base.prev_blocks_ids[0].seq_no
                    )
                }
            }
            0 => {
                soft_reject_query!("must have one or two previous blocks to generate a next block")
            }
            _ => soft_reject_query!("cannot have more than two previous blocks"),
        }

        // 4. unpack block candidate (while necessary data is being loaded)
        Self::unpack_block_candidate(base, &mut self.block_candidate)
    }

    async fn init_mc_data(&mut self, base: &mut ValidateBase) -> Result<McData> {
        // 2. learn latest masterchain state and block id
        let mc_data = self.get_ref_mc_state(base).await?;
        base.config_params = mc_data.config().clone();
        base.block_limits = base.config_params.block_limits(base.shard().is_masterchain())?;
        base.limits = base.config_params.size_limits_config()?;
        base.special_smartcontracts = base.config_params.fundamental_smc_addr()?;

        // 3. load state(s) corresponding to previous block(s)
        for i in 0..base.prev_blocks_ids.len() {
            let block_id = &base.prev_blocks_ids[i];
            log::debug!(
                target: "validate_query",
                "({}): load state for prev block {} of {} {}",
                self.next_block_descr,
                i + 1,
                base.prev_blocks_ids.len(),
                block_id,
            );
            let prev_state = if !block_id.is_masterchain() && base.full_collated_data {
                let state = base.virt_states.remove(block_id.root_hash()).ok_or_else(|| {
                    error!("collated data has no state for previous block {}", block_id)
                })?;
                ShardStateStuff::from_state(
                    block_id.clone(),
                    state,
                    #[cfg(feature = "telemetry")]
                    self.engine.engine_telemetry(),
                    self.engine.engine_allocated(),
                )?
            } else {
                self.wait_prev_state_via_engine_or_cache(block_id).await?
            };
            if &self.shard == prev_state.shard() && prev_state.state()?.before_split() {
                reject_query!(
                    "cannot accept new unsplit shardchain block for {} \
                    after previous block {} with before_split set",
                    self.shard,
                    prev_state.block_id()
                )
            }
            base.prev_states.push(prev_state);
        }
        if !base.shard().is_masterchain() {
            // It is impossible to get the master state (it have got in 'get_ref_mc_state' above)
            // for block without proof. But proof can appear bit later due to parralelism specials.
            // So this check is not needed.

            // 5.1. request corresponding block handle
            // let _handle = self.engine.load_block_handle(mc_data.state.block_id())?.ok_or_else(
            //     || error!("Cannot load handle for masterblock {}", mc_data.state.block_id())
            // )?;
            // if !self.is_fake && !handle.has_proof() && handle.id().seq_no() != 0 {
            //     reject_query!("reference masterchain block {} for block {} does not have a valid proof",
            //         handle.id(), base.block_id())
            // }
        } else if &base.prev_blocks_ids[0] != mc_data.state.block_id() {
            soft_reject_query!(
                "cannot validate masterchain block {} because it refers to masterchain \
                block {} but its (expected) previous block is {}",
                base.block_id(),
                mc_data.state.block_id(),
                base.prev_blocks_ids[0]
            )
        }
        Ok(mc_data)
    }

    // unpack block candidate, and check root hash and file hash
    fn unpack_block_candidate(
        mut base: ValidateBase,
        block_candidate: &mut BlockCandidate,
    ) -> Result<ValidateBase> {
        CHECK!(!block_candidate.data.is_empty());
        // 1. deserialize block itself
        let data = Arc::new(mem::take(&mut block_candidate.data));
        base.block = BlockStuff::deserialize_block_checked(block_candidate.block_id.clone(), data)?;
        // 3. initial block parse
        Self::init_parse(&mut base)?;
        // ...
        Self::extract_collated_data(&mut base, block_candidate)?;
        Ok(base)
    }

    // init_parse
    fn init_parse(base: &mut ValidateBase) -> Result<()> {
        base.global_id = base.block.block()?.global_id();
        base.info = base.block.block()?.read_info()?;
        let block_id = BlockIdExt::from_ext_blk(base.info.read_master_id()?);
        CHECK!(block_id.shard_id.is_masterchain());
        let prev_blocks_ids = base.info.read_prev_ids()?;

        if prev_blocks_ids.len() != base.prev_blocks_ids.len() {
            soft_reject_query!(
                "block header declares {} previous blocks, but we are given {}",
                prev_blocks_ids.len(),
                base.prev_blocks_ids.len()
            )
        }
        for (i, blk) in base.prev_blocks_ids.iter().enumerate() {
            if &prev_blocks_ids[i] != blk {
                soft_reject_query!(
                    "previous block #{} mismatch: expected {}, found in header {}",
                    i + 1,
                    blk,
                    prev_blocks_ids[i]
                );
            }
        }
        if base.info.after_split() != base.after_split {
            // ??? impossible
            reject_query!("after_split mismatch in block header")
        }
        if base.info.shard() != base.shard() {
            reject_query!("shard mismatch in the block header")
        }
        base.state_update = base.block.block()?.read_state_update()?;
        base.value_flow = base.block.block()?.read_value_flow()?;

        if base.info.key_block() {
            log::info!(
                target: "validate_query",
                "({}): validating key block {}",
                base.next_block_descr,
                base.block_id()
            );
        }
        if base.info.start_lt() >= base.info.end_lt() {
            reject_query!("block has start_lt greater than or equal to end_lt")
        }
        if base.info.shard().is_masterchain()
            && (base.info.after_merge() || base.info.before_split() || base.info.after_split())
        {
            reject_query!("block header declares split/merge for a masterchain block")
        }
        if base.info.after_merge() && base.info.after_split() {
            reject_query!("a block cannot be both after merge and after split at the same time")
        }
        if base.info.after_split() && base.shard().is_full() {
            reject_query!("a block with empty shard prefix cannot be after split")
        }
        if base.info.after_merge() && !base.shard().can_split() {
            reject_query!("a block split 60 times cannot be after merge")
        }
        if base.info.key_block() && !base.shard().is_masterchain() {
            reject_query!("a non-masterchain block cannot be a key block")
        }
        if base.info.vert_seqno_incr() != 0 {
            // what about non-masterchain blocks?
            reject_query!("new blocks cannot have vert_seqno_incr set")
        }
        if base.info.after_merge() != base.after_merge {
            reject_query!("after_merge value mismatch in block header")
        }
        base.extra = base.block.block()?.read_extra()?;

        if &base.created_by != base.extra.created_by() {
            reject_query!(
                "block candidate {} has creator {:x} \
                but the block header contains different value {:x}",
                base.block_id(),
                base.created_by,
                base.extra.created_by()
            )
        }
        if base.shard().is_masterchain() {
            base.mc_extra = base
                .extra
                .read_custom()?
                .ok_or_else(|| error!("masterchain block candidate without McBlockExtra"))?;
            if base.mc_extra.is_key_block() != base.info.key_block() {
                reject_query!("key_block flag mismatch in BlockInfo and McBlockExtra")
            }
            if base.info.key_block() && base.mc_extra.config().is_none() {
                reject_query!("key_block must contain ConfigParams in McBlockExtra")
            }
            base.recover_create_msg = base.mc_extra.read_recover_create_msg()?;
            base.mint_msg = base.mc_extra.read_mint_msg()?;
        } else if base.extra.is_masterchain() {
            reject_query!("non-masterchain block cannot have McBlockExtra")
        }
        // ...
        Ok(())
    }

    fn extract_collated_data_from_roots(
        base: &mut ValidateBase,
        collated_roots: Vec<Cell>,
    ) -> Result<()> {
        let mut blocks = HashMap::new();
        let mut states = HashMap::new();
        for (idx, croot) in collated_roots.iter().enumerate() {
            match croot.cell_type() {
                CellType::Ordinary => match TopBlockDescrSet::construct_from_cell(croot.clone()) {
                    Ok(descr) => {
                        if base.top_shard_descr_dict.is_empty() {
                            log::debug!(
                                target: "validate_query",
                                "({}): collated datum #{idx} is a TopBlockDescrSet",
                                base.next_block_descr
                            );
                            base.top_shard_descr_dict = descr;
                            base.top_shard_descr_dict
                                .count(10000)
                                .map_err(|err| error!("invalid TopBlockDescrSet : {}", err))?;
                        } else {
                            reject_query!("duplicate TopBlockDescrSet in collated data")
                        }
                    }
                    Err(err) => {
                        if let Some(BlockError::InvalidConstructorTag { t: _, s: _ }) =
                            err.downcast_ref()
                        {
                            // Try AccountStorageDictProof first
                            match AccountStorageDictProof::construct_from_cell(croot.clone()) {
                                Ok(dict_proof) => {
                                    log::debug!(
                                        target: "validate_query",
                                        "({}): collated datum # {idx} is an AccountStorageDictProof",
                                        base.next_block_descr
                                    );
                                    let dict_proof =
                                        MerkleProof::construct_from_cell(dict_proof.proof)?;
                                    base.storage_dict_proofs
                                        .insert(dict_proof.hash, dict_proof.proof.virtualize(1));
                                    base.full_collated_data = true;
                                }
                                Err(_) => {
                                    // Try ConsensusExtraData
                                    match ConsensusExtraData::construct_from_cell(croot.clone()) {
                                        Ok(extra) => {
                                            log::debug!(
                                                target: "validate_query",
                                                "({}): collated datum # {idx} is a ConsensusExtraData, gen_utime_ms={}",
                                                base.next_block_descr,
                                                extra.gen_utime_ms
                                            );
                                            if base.now_ms.is_some() {
                                                reject_query!(
                                                    "duplicate ConsensusExtraData in collated data"
                                                )
                                            }
                                            // Check: ConsensusExtraData is only valid when simplex is enabled
                                            if !base.is_simplex {
                                                reject_query!("unexpected ConsensusExtraData")
                                            }
                                            base.now_ms = Some(extra.gen_utime_ms);
                                        }
                                        Err(_) => {
                                            let tag = SliceData::load_cell_ref(croot)?
                                                .get_next_u32()
                                                .unwrap_or(0);
                                            log::warn!(
                                                target: "validate_query",
                                                "({}): collated datum # {idx} has unknown type (tag {:#010x}), ignoring",
                                                base.next_block_descr,
                                                tag
                                            );
                                        }
                                    }
                                }
                            }
                        } else {
                            return Err(err);
                        }
                    }
                },
                CellType::MerkleProof => {
                    let merkle_proof = match MerkleProof::construct_from_cell(croot.clone()) {
                        Err(err) => reject_query!("invalid Merkle proof: {:?}", err),
                        Ok(mp) => mp,
                    };
                    let virt_root = merkle_proof.proof.virtualize(1);
                    let virt_root_hash = merkle_proof.hash;
                    log::debug!(
                        target: "validate_query",
                        "({}): collated datum # {idx} is a Merkle proof with root hash {:x}",
                        base.next_block_descr,
                        virt_root_hash
                    );
                    if let Ok(block) = Block::construct_from_cell(virt_root.clone()) {
                        blocks.insert(virt_root_hash, block);
                    } else if let Ok(state) =
                        ShardStateUnsplit::construct_from_cell(virt_root.clone())
                    {
                        states.insert(virt_root_hash, state);
                    } else {
                        log::warn!(
                            target: "validate_query",
                            "collated Merkle proof root is neither a block nor a shard state"
                        );
                        continue;
                    }
                    base.full_collated_data = true;
                }
                _ => reject_query!("it is a special cell, but not a Merkle proof root"),
            }
        }
        for (block_hash, block) in blocks {
            let state_hash = block.read_state_update()?.new_hash;
            let state = states.remove(&state_hash).ok_or_else(|| {
                error!(
                    "collated data block {} has no corresponding state {} in collated data",
                    block_hash.as_hex_string(),
                    state_hash.as_hex_string(),
                )
            })?;
            base.virt_states.insert(block_hash, state);
        }
        // block ID for zerostate contains root hash of the state instead of block
        base.virt_states.extend(states);
        Ok(())
    }

    // processes further and sorts data in collated_roots
    fn extract_collated_data(
        base: &mut ValidateBase,
        block_candidate: &BlockCandidate,
    ) -> Result<()> {
        if !block_candidate.collated_data.is_empty() {
            // 8. deserialize collated data
            let collated_roots = match read_boc(&block_candidate.collated_data) {
                Ok(result) => result.roots,
                Err(err) => reject_query!("cannot deserialize collated data: {}", err),
            };
            // 9. extract/classify collated data
            Self::extract_collated_data_from_roots(base, collated_roots)?;
        }
        Ok(())
    }

    async fn get_ref_mc_state(&mut self, base: &mut ValidateBase) -> Result<McData> {
        let (_, mc_id) = base.info.read_master_id()?.master_block_id();
        let mc_state = self.engine.clone().wait_state(&mc_id, Some(1_000), true).await?;
        log::debug!(
            target: "validate_query",
            "({}): in ValidateQuery::get_ref_mc_state() {}",
            self.next_block_descr,
            mc_state.block_id()
        );
        if mc_state.state()?.seq_no() < self.min_mc_seqno {
            reject_query!(
                "requested to validate a block referring to \
                an unknown future masterchain block {} < {}",
                mc_state.state()?.seq_no(),
                self.min_mc_seqno
            )
        }
        #[cfg(test)]
        if base.is_fake {
            base.pure_root = mc_state.root_cell().clone();
            base.mc_usage_tree = UsageTree::with_root(base.pure_root.clone());
            let mc_state = ShardStateStuff::from_root_cell(
                mc_state.block_id().clone(),
                base.mc_usage_tree.root_cell(),
                #[cfg(feature = "telemetry")]
                self.engine.engine_telemetry(),
                self.engine.engine_allocated(),
            )?;
            return self.try_unpack_mc_state(base, mc_state);
        }
        self.try_unpack_mc_state(base, mc_state)
    }

    // fn process_mc_state(&mut self, mc_data: &McData, mc_state: &ShardStateStuff) -> Result<()> {
    //     if mc_data.state.block_id() != mc_state.block_id() {
    //         if !mc_data.state.has_prev_block(mc_state.block_id())? {
    //             reject_query!("attempting to register masterchain state for block {} \
    //                 which is not an ancestor of most recent masterchain block {}",
    //                     mc_state.block_id(), mc_data.state.block_id())
    //         }
    //     }
    //     self.engine.set_aux_mc_state(mc_state)?;
    //     Ok(())
    // }

    fn try_unpack_mc_state(
        &mut self,
        base: &ValidateBase,
        mc_state: Arc<ShardStateStuff>,
    ) -> Result<McData> {
        log::debug!(
            target: "validate_query",
            "({}): unpacking reference masterchain state {}",
            self.next_block_descr, mc_state.block_id()
        );
        let mc_state_extra = mc_state.shard_state_extra()?.clone();
        let config_params = mc_state_extra.config();
        CHECK!(config_params, inited);
        let block_version = base.info.gen_software().map_or(0, |v| v.version);
        if block_version < config_params.global_version() {
            reject_query!(
                "This block version {} is too old, node_version: {} net version: {}",
                block_version,
                supported_version(),
                config_params.global_version()
            )
        }
        // ihr_enabled_ = config_params->ihr_enabled();
        self.create_stats_enabled =
            config_params.has_capability(GlobalCapabilities::CapCreateStatsEnabled);
        if config_params.has_capabilities()
            && (config_params.capabilities() & !supported_capabilities()) != 0
        {
            log::error!(
                target: "validate_query",
                "({}): block generation capabilities {} have been enabled in global configuration, \
                but we support only {} (upgrade validator software?)",
                self.next_block_descr,
                config_params.capabilities(),
                supported_capabilities()
            );
        }
        if config_params.global_version() > supported_version() {
            log::error!(
                target: "validate_query",
                "({}): block version {} have been enabled in global configuration, \
                but we support only {} (upgrade validator software?)",
                self.next_block_descr,
                config_params.global_version(),
                supported_version()
            );
        }
        self.old_mc_shards = mc_state_extra.shards().clone();
        self.new_mc_shards = if base.shard().is_masterchain() {
            base.mc_extra.shards().clone()
        } else {
            #[cfg(feature = "xp25")]
            {
                crate::validating_utils::extend_ref_shard_blocks(
                    &base.extra.read_wc_custom()?.ref_shard_blocks,
                )?
            }
            #[cfg(not(feature = "xp25"))]
            {
                self.old_mc_shards.clone()
            }
        };
        if base.global_id != mc_state.state()?.global_id() {
            reject_query!(
                "blockchain global id mismatch: new block has {} \
                while the masterchain configuration expects {}",
                base.global_id,
                mc_state.state()?.global_id()
            )
        }
        CHECK!(&base.info, inited);
        if base.info.vert_seq_no() != mc_state.state()?.vert_seq_no() {
            reject_query!(
                "vertical seqno mismatch: new block has {} \
                while the masterchain configuration expects {}",
                base.info.vert_seq_no(),
                mc_state.state()?.vert_seq_no()
            )
        }
        let (prev_key_block_seqno, prev_key_block);
        if mc_state_extra.after_key_block {
            prev_key_block_seqno = mc_state.block_id().seq_no();
            prev_key_block = Some(mc_state.block_id().clone());
        } else if let Some(block_ref) = mc_state_extra.last_key_block.clone() {
            prev_key_block_seqno = block_ref.seq_no;
            prev_key_block = Some(block_ref.master_block_id().1);
        } else {
            prev_key_block_seqno = 0;
            prev_key_block = None;
        };
        if base.info.prev_key_block_seqno() != prev_key_block_seqno {
            reject_query!(
                "previous key block seqno value in candidate block header is {} \
                while the correct value corresponding to reference masterchain state {} is {}",
                base.info.prev_key_block_seqno(),
                mc_state.block_id(),
                prev_key_block_seqno
            )
        }
        if !base.shard().is_masterchain() {
            check_this_shard_mc_info(
                base.shard(),
                base.block_id(),
                base.after_merge,
                base.after_split,
                base.info.before_split(),
                &base.prev_blocks_ids,
                config_params,
                &mc_state_extra,
                true,
                base.now(),
            )
            .map_err(|e| {
                error!(
                    "masterchain configuration does not admit creating block {}: {}",
                    base.block_id(),
                    e
                )
            })?;
        }
        Ok(McData { mc_state_extra, state: mc_state, prev_key_block_seqno, prev_key_block })
    }

    /*
     *
     *  METHODS CALLED FROM try_validate() stage 0
     *
     */

    async fn compute_next_state(
        &mut self,
        base: &mut ValidateBase,
        mc_data: &McData,
    ) -> Result<()> {
        let prev_state = base.prev_states[0].clone();
        base.prev_state = Some(prev_state.clone());
        let prev_state_root = if base.after_merge && base.prev_states.len() == 2 {
            self.check_one_prev_state(base, &base.prev_states[0])?;
            self.check_one_prev_state(base, &base.prev_states[1])?;
            let left = base.prev_states[0].root_cell().clone();
            let right = base.prev_states[1].root_cell().clone();
            ShardStateStuff::construct_split_root(left, right)?
        } else {
            CHECK!(base.prev_states.len(), 1);
            self.check_one_prev_state(base, &base.prev_states[0])?;
            base.prev_states[0].root_cell().clone()
        };
        log::debug!(target: "validate_query", "({}): computing next state", self.next_block_descr);
        let engine = self.engine.clone();
        let state_update = base.state_update.clone();
        let next_block_descr = self.next_block_descr.clone();
        let is_fake = base.is_fake;
        let (next_state_root, _) = tokio::task::spawn_blocking(move || {
            let fast_result = if is_fake {
                Err(error!("not supported in test env"))
            } else {
                engine.db_cells_factory()
                    .and_then(|cf| engine.db_cells_loader().map(|cl| (cf, cl)))
                    .and_then(|(cf, cl)| {
                        state_update.apply_with_loader(&prev_state_root, &cf, cl.deref())
                    })
            };
            match fast_result {
                Ok(r) => Ok(r),
                Err(e) => {
                    log::debug!(
                        "({}): Failed the fast attempt of Merkle update applying: {}. Trying classic approach...",
                        next_block_descr, e
                    );
                    state_update.apply_for(&prev_state_root).map_err(|err| {
                        error!("cannot apply Merkle update from block to compute new state : {}", err)
                    })
                }
            }
        }).await??;
        log::debug!(target: "validate_query", "({}): next state computed", self.next_block_descr);
        let next_state = ShardStateStuff::from_root_cell(
            base.block_id().clone(),
            next_state_root.clone(),
            #[cfg(feature = "telemetry")]
            self.engine.engine_telemetry(),
            self.engine.engine_allocated(),
        )?;
        base.next_state = Some(next_state.clone());
        if base.info.end_lt() != next_state.state()?.gen_lt() {
            reject_query!(
                "new state contains generation lt {} distinct from end_lt {} in block header",
                next_state.state()?.gen_lt(),
                base.info.end_lt()
            )
        }
        if base.now() != next_state.state()?.gen_time() {
            reject_query!(
                "new state contains generation time {} distinct from the value {} in block header",
                next_state.state()?.gen_time(),
                base.now()
            )
        }
        if base.info.before_split() != next_state.state()?.before_split() {
            reject_query!("before_split value mismatch in new state and in block header")
        }
        if (base.block_id().seq_no != next_state.state()?.seq_no())
            || (base.shard() != next_state.state()?.shard())
        {
            reject_query!(
                "header of new state claims it belongs to block {} instead of {}",
                next_state.state()?.shard(),
                base.block_id().shard()
            )
        }
        if next_state.state()?.custom_cell().is_some() != base.shard().is_masterchain() {
            reject_query!("McStateExtra in the new state of a non-masterchain block, or conversely")
        }
        if base.shard().is_masterchain() {
            base.prev_state_extra = prev_state.shard_state_extra()?.clone();
            base.next_state_extra = next_state.shard_state_extra()?.clone();
            let next_state_extra = next_state.shard_state_extra()?;
            if next_state_extra.shards() != base.mc_extra.shards() {
                reject_query!("ShardHashes in the new state and in the block differ")
            }
            if base.info.key_block() {
                CHECK!(base.mc_extra.config().is_some());
                if let Some(config) = base.mc_extra.config() {
                    if config != &next_state_extra.config {
                        reject_query!(
                            "ConfigParams in the header of the new key block \
                            and in the new state differ"
                        )
                    }
                }
            }
        } else {
            base.prev_state_extra = mc_data.state.shard_state_extra()?.clone();
            base.next_state_extra = mc_data.state.shard_state_extra()?.clone();
        }
        Ok(())
    }

    // similar to Collator::unpack_one_last_state()
    fn check_one_prev_state(&self, base: &ValidateBase, ss: &ShardStateStuff) -> Result<()> {
        if ss.state()?.vert_seq_no() > base.info.vert_seq_no() {
            reject_query!(
                "one of previous states {} has vertical seqno {} larger \
                than that of the new block {}",
                ss.state()?.id(),
                ss.state()?.vert_seq_no(),
                base.info.vert_seq_no()
            )
        }
        Ok(())
    }

    fn unpack_prev_state(&mut self, base: &mut ValidateBase) -> Result<()> {
        base.prev_state_accounts = base.prev_states[0].state()?.read_accounts()?;
        base.prev_validator_fees = base.prev_states[0].state()?.total_validator_fees().clone();
        if let Some(state) = base.prev_states.get(1) {
            CHECK!(base.after_merge);
            let key = state.shard().merge()?.shard_key(false);
            base.prev_state_accounts.hashmap_merge(&state.state()?.read_accounts()?, &key)?;
            base.prev_state_accounts.update_root_extra()?;
            base.prev_validator_fees.add(state.state()?.total_validator_fees())?;
        } else if base.after_split {
            base.prev_state_accounts.split_for(&base.shard().shard_key(false))?;
            base.prev_state_accounts.update_root_extra()?;
            if base.shard().is_right_child() {
                base.prev_validator_fees.coins += 1;
            }
            base.prev_validator_fees.coins /= 2;
        }
        Ok(())
    }

    fn unpack_next_state(&self, base: &mut ValidateBase, mc_data: &McData) -> Result<()> {
        log::debug!(target: "validate_query", "({}): unpacking new state", self.next_block_descr);
        let next_state = base.next_state()?;
        if next_state.gen_time() != base.now() {
            reject_query!(
                "new state of {} claims to have been generated at unixtime {}, \
                but the block header contains {}",
                next_state.id(),
                next_state.gen_time(),
                base.info.gen_utime()
            )
        }
        if next_state.gen_lt() != base.info.end_lt() {
            reject_query!(
                "new state of {} claims to have been generated at logical time {}, \
                but the block header contains end lt {}",
                next_state.id(),
                next_state.gen_lt(),
                base.info.end_lt()
            )
        }
        if !base.shard().is_masterchain() {
            let Some(master_ref) = next_state.master_ref() else {
                reject_query!("new state of {} doesn't have have master ref", next_state.id())
            };
            let (_, mc_blkid) = master_ref.master.clone().master_block_id();
            if &mc_blkid != mc_data.state.block_id() {
                reject_query!(
                    "new state refers to masterchain block {} different from {} \
                    indicated in block header",
                    mc_blkid,
                    mc_data.state.block_id()
                )
            }
        }
        if next_state.vert_seq_no() != base.info.vert_seq_no() {
            reject_query!(
                "new state has vertical seqno {} different from {} \
                declared in the new block header",
                next_state.vert_seq_no(),
                base.info.vert_seq_no()
            )
        }
        base.next_state_accounts = next_state.read_accounts()?;
        // ...
        Ok(())
    }

    async fn init_output_queue_manager(
        &self,
        mc_data: &McData,
        base: &mut ValidateBase,
    ) -> Result<MsgQueueManager> {
        let states_manager = if base.full_collated_data {
            StatesManager::with_validator_data_nosync(
                self.engine.clone(),
                mem::take(&mut base.virt_states),
            )?
        } else {
            StatesManager::with_validator_data(self.engine.clone())
        };
        MsgQueueManager::init(
            &self.engine,
            &mc_data.state,
            self.shard().clone(),
            self.block_candidate.block_id.seq_no,
            &self.new_mc_shards,
            &base.prev_states,
            base.next_state.as_ref(),
            base.after_merge,
            base.after_split,
            None,
            None,
            None,
            None,
            states_manager,
        )
        .await
    }

    fn check_tsbd(
        &self,
        base: &ValidateBase,
        mc_data: &McData,
        id: &BlockIdExt,
    ) -> Result<(TopBlockDescrStuff, usize)> {
        let tsbd = match base.top_shard_descr_dict.get_top_block_descr(id.shard())? {
            Some(tbd) => TopBlockDescrStuff::new(tbd, id, base.is_fake, false)?,
            None => {
                reject_query!("no ShardTopBlockDescr for shard {} is present in collated data", id)
            }
        };
        if tsbd.proof_for() != id {
            reject_query!(
                "ShardTopBlockDescr is for block {} \
                instead of {} declared in new shardchain configuration",
                tsbd.proof_for(),
                id
            )
        }
        // following checks are similar to those of Collator::import_new_shard_top_blocks()
        let do_flags = TopBlockDescrMode::FAIL_NEW | TopBlockDescrMode::FAIL_TOO_NEW;
        let mut res_flags = 0;
        let chain_len = tsbd
            .prevalidate(mc_data.state.block_id(), &mc_data.state, do_flags, &mut res_flags)
            .map_err(|err| {
                error!(
                    "ShardTopBlockDescr for {} is invalid: res_flags={}, err: {}",
                    tsbd.proof_for(),
                    res_flags,
                    err
                )
            })?;
        if chain_len <= 0 || chain_len > UNREGISTERED_CHAIN_MAX_LEN as i32 {
            reject_query!(
                "ShardTopBlockDescr for {} is invalid: its chain length is {} (not in range 1..{})",
                tsbd.proof_for(),
                chain_len,
                UNREGISTERED_CHAIN_MAX_LEN
            )
        }
        let chain_len = chain_len as usize;
        if tsbd.gen_utime() > base.now() {
            reject_query!(
                "ShardTopBlockDescr for {} is invalid: \
                it claims to be generated at {} while it is still {}",
                tsbd.proof_for(),
                tsbd.gen_utime(),
                base.now()
            )
        }

        Ok((tsbd, chain_len))
    }

    // similar to Collator::update_one_shard()
    fn check_one_shard(
        &mut self,
        base: &ValidateBase,
        mc_data: &McData,
        info: &McShardRecord,
        sibling: Option<&McShardRecord>,
        wc_info: Option<&WorkchainDescr>,
    ) -> Result<()> {
        let shard = info.shard();
        log::debug!(
            target: "validate_query",
            "({}): checking shard {} in new shard configuration",
            self.next_block_descr,
            shard
        );
        if info.descr.next_validator_shard != shard.shard_prefix_with_tag() {
            reject_query!(
                "new shard configuration for shard {} contains different next_validator_shard {}",
                shard,
                info.descr.next_validator_shard
            )
        }

        let old = self.old_mc_shards.find_shard(&shard.left_ancestor_mask()?)?;
        let mut prev: Option<McShardRecord> = None;
        let mut cc_seqno = !0;
        let mut old_before_merge = false;
        let workchain_created = false;
        if old.is_none() {
            if !shard.is_full() {
                reject_query!(
                    "new shard configuration contains split shard {} unknown before",
                    shard
                )
            }
            let Some(wc_info) = wc_info else {
                reject_query!(
                    "new shard configuration contains newly-created \
                    shard {shard} for an unknown workchain"
                )
            };
            if !wc_info.active {
                reject_query!(
                    "new shard configuration contains newly-created shard {shard} \
                    for an inactive workchain"
                )
            }
            if info.descr.seq_no != 0 {
                reject_query!(
                    "newly-created shard {} starts with non-zero seqno {}",
                    shard,
                    info.descr.seq_no
                )
            }
            if info.descr.root_hash != wc_info.zerostate_root_hash
                || info.descr.file_hash != wc_info.zerostate_file_hash
            {
                reject_query!(
                    "new shard configuration contains newly-created shard {shard}
                    with incorrect zerostate hashes"
                )
            }
            if info.descr.end_lt >= base.info.start_lt() {
                reject_query!(
                    "newly-created shard {shard} has incorrect logical time {} \
                    for a new block with start_lt={}",
                    info.descr.end_lt,
                    base.info.start_lt()
                );
            }
            if info.descr.gen_utime > base.now() {
                reject_query!(
                    "newly-created shard {shard} has incorrect creation time {} \
                    for a new block created only at {}",
                    info.descr.gen_utime,
                    base.now()
                )
            }
            if info.descr.before_split
                || info.descr.before_merge
                || info.descr.want_split
                || info.descr.want_merge
            {
                reject_query!(
                    "newly-created shard {} has merge/split flags (incorrectly) set",
                    shard
                )
            }
            if info.descr.min_ref_mc_seqno != u32::MAX {
                reject_query!(
                    "newly-created shard {} has finite min_ref_mc_seqno {}",
                    shard,
                    info.descr.min_ref_mc_seqno
                )
            }
            if info.descr.reg_mc_seqno != base.block_id().seq_no {
                reject_query!(
                    "newly-created shard {shard} has registration mc seqno {} \
                    different from seqno of current block {}",
                    info.descr.reg_mc_seqno,
                    base.block_id().seq_no
                )
            }
            if !info.descr.fees_collected.is_zero()? {
                reject_query!("newly-created shard {} has non-zero fees_collected", shard)
            }
            cc_seqno = 0;
        } else if let Some(old) = old.clone() {
            if old.block_id == info.block_id {
                // shard unchanged ?
                log::debug!(
                    target: "validate_query",
                    "({}): shard {shard} unchanged",
                    self.next_block_descr
                );
                if !old.basic_info_equal(info, true, true) {
                    reject_query!(
                        "shard information for block {} listed in new shard configuration differs \
                        from that present in the old shard configuration for the same block",
                        info.block_id
                    );
                }
                cc_seqno = old.descr.next_catchain_seqno;
                prev = Some(old);
                // ...
            } else {
                // shard changed, extract and check TopShardBlockDescr from collated data
                log::debug!(
                    target: "validate_query",
                    "({}): shard {shard} changed from {} to {}",
                    self.next_block_descr,
                    old.block_id.seq_no,
                    info.block_id.seq_no
                );
                if info.descr.reg_mc_seqno != base.block_id().seq_no {
                    reject_query!(
                        "shard information for block {} has been updated \
                        in the new shard configuration, but it has reg_mc_seqno={} \
                        different from that of the current block {}",
                        info.block_id,
                        info.descr.reg_mc_seqno,
                        base.block_id().seq_no
                    )
                }
                let (sh_bd, chain_len) = self.check_tsbd(base, mc_data, &info.block_id)?;
                let descr = sh_bd
                    .get_top_descr(chain_len)
                    .map_err(|err| error!("No top descr for {:?}: {}", sh_bd, err))?;
                CHECK!(&descr, inited);
                CHECK!(descr.block_id(), sh_bd.proof_for());
                let start_blks = sh_bd.get_prev_at(chain_len);
                may_update_shard_block_info(
                    mc_data.state.shards()?,
                    &descr,
                    &start_blks,
                    base.info.start_lt(),
                    None,
                )
                .map_err(|err| {
                    error!(
                        "new top shard block {:?} cannot be added to shard configuration: {}",
                        sh_bd, err
                    )
                })?;
                if !descr.basic_info_equal(info, true, false) {
                    reject_query!(
                        "shard information for block {:?} listed in new shard configuration \
                        differs from that present in ShardTopBlockDescr (and block header)",
                        info.block_id
                    )
                }
                // all fields in info and descr are equal
                // except fsm*, before_merge_, next_catchain_seqno_
                // of these, only next_catchain_seqno_ makes sense in descr
                cc_seqno = descr.descr.next_catchain_seqno;
                // check that there is a corresponding record in ShardFees
                if let Some(import) =
                    base.mc_extra.fees().get_serialized(shard.full_key_with_tag()?)?
                {
                    if import.fees != descr.descr.fees_collected {
                        reject_query!(
                            "ShardFees record for new shard top block {:?} \
                            declares fees_collected={:?}, but the shard configuration \
                            contains a different value {}",
                            sh_bd,
                            import.fees,
                            descr.descr.fees_collected
                        )
                    }
                    if import.create != descr.descr.funds_created {
                        reject_query!(
                            "ShardFees record for new shard top block {:?} \
                            declares funds_created={:?}, but the shard configuration \
                            contains a different value {}",
                            sh_bd,
                            import.create,
                            descr.descr.funds_created
                        );
                    }
                } else if !descr.descr.fees_collected.is_zero()? {
                    reject_query!(
                        "new shard top block {:?} has been registered and \
                        has non-zero collected fees {}, but there is \
                        no corresponding entry in ShardFees",
                        sh_bd,
                        descr.descr.fees_collected
                    )
                }
                // register shard block creators
                self.register_shard_block_creators(base, &sh_bd.get_creator_list(chain_len)?)?;
                // ...
                if old.shard().is_parent_for(shard) {
                    // shard has been split
                    log::debug!(
                        target: "validate_query",
                        "({}): detected shard split {} -> {}",
                        self.next_block_descr,
                        old.shard(),
                        shard
                    );
                    // ...
                } else if shard.is_parent_for(old.shard()) {
                    // shard has been merged
                    if let Some(old2) =
                        self.old_mc_shards.find_shard(&shard.right_ancestor_mask()?)?
                    {
                        if &old.shard().sibling() != old2.shard() {
                            reject_query!(
                                "shard {} has been impossibly merged from more than two shards \
                                {}, {} and others",
                                shard,
                                old.shard(),
                                old2.shard()
                            )
                        }
                        log::debug!(
                            target: "validate_query",
                            "({}): detected shard merge {} + {} -> {}",
                            self.next_block_descr,
                            old.shard(),
                            old2.shard(),
                            shard
                        );
                    } else {
                        // CHECK!(old2.is_some());
                        reject_query!("No plus_one shard") // TODO: check here
                    }
                    // ...
                } else if shard == old.shard() {
                    // shard updated without split/merge
                    prev = Some(old);
                    // ...
                } else {
                    reject_query!(
                        "new configuration contains shard {} that could not be \
                        obtained from previously existing shard {}",
                        shard,
                        old.shard()
                    );
                    // ...
                }
            }
        }
        let mut fsm_inherited = false;
        if let Some(prev) = &prev {
            // shard was not created, split or merged; it is a successor of `prev`
            fsm_inherited = !prev.descr().is_fsm_none() && prev.descr().fsm_equal(info.descr());
            old_before_merge = prev.descr.before_merge;
            if !prev.descr().is_fsm_none() &&               // fsm was not none
                !prev.descr().fsm_equal(info.descr()) &&     // fsm was changed
                base.now() < prev.descr().fsm_utime_end() && // split/merge time is not come
                !info.descr.before_split
            {
                reject_query!(
                    "future split/merge information for shard {} has been arbitrarily \
                    changed without a good reason",
                    shard
                )
            }
            if fsm_inherited
                && (base.now() > prev.descr().fsm_utime_end() || info.descr.before_split)
            {
                reject_query!(
                    "future split/merge information for shard {} has been carried on \
                    to the new shard configuration, but it is either expired \
                    (expire time {}, now {}), or before_split bit has been set ({})",
                    shard,
                    prev.descr().fsm_utime_end(),
                    base.now(),
                    info.descr.before_split
                );
            }
        } else {
            // shard was created, split or merged
            if info.descr.before_split {
                reject_query!(
                    "a newly-created, split or merged shard {} cannot have \
                    before_split set immediately after",
                    shard
                )
            }
        }
        let wc_info = wc_info.expect("in ton node it is a bug");
        let depth = shard.prefix_len();

        let split_cond = (info.descr.want_split || depth < wc_info.min_split())
            && depth < wc_info.max_split()
            && depth < MAX_SPLIT_DEPTH;

        let merge_cond = depth > wc_info.min_split()
            && (info.descr.want_merge || depth > wc_info.max_split())
            && (sibling.map(|s| s.descr.want_merge).unwrap_or_default()
                || depth > wc_info.max_split());

        if !fsm_inherited && !info.descr().is_fsm_none() {
            {
                let fsm_begin = info.descr().fsm_utime();
                let fsm_end = info.descr().fsm_utime_end();
                if fsm_begin < base.now()
                    || fsm_end <= fsm_begin
                    || fsm_end < fsm_begin + MIN_SPLIT_MERGE_INTERVAL
                    || fsm_end > base.now() + MAX_SPLIT_MERGE_DELAY
                {
                    reject_query!(
                        "incorrect future split/merge interval {} .. {} \
                        set for shard {} in new shard configuration (it is {} now)",
                        fsm_begin,
                        fsm_end,
                        shard,
                        base.now()
                    );
                }
            }
            if info.descr().is_fsm_split() && !split_cond {
                reject_query!(
                    "announcing future split for shard {} in new shard configuration, \
                    but split conditions are not met",
                    shard
                )
            }
            if info.descr().is_fsm_merge() && !merge_cond {
                reject_query!(
                    "announcing future merge for shard {} in new shard configuration, \
                    but merge conditions are not met",
                    shard
                )
            }
        }
        if info.descr.before_merge {
            if !sibling.map(|s| s.descr.before_merge).unwrap_or_default() {
                reject_query!(
                    "before_merge set for shard {} in shard configuration, \
                    but not for its sibling",
                    shard
                )
            }
            if !info.descr().is_fsm_merge() {
                reject_query!(
                    "before_merge set for shard {} in shard configuration, \
                    but it has not been announced in future split/merge \
                    for this shard",
                    shard
                )
            }
            if !merge_cond {
                reject_query!(
                    "before_merge set for shard {} in shard configuration, \
                    but merge conditions are not met",
                    shard
                )
            }
        }
        CHECK!(cc_seqno != !0);
        let cc_updated = info.descr.next_catchain_seqno != cc_seqno;
        if info.descr.next_catchain_seqno != cc_seqno + cc_updated as u32 {
            reject_query!(
                "new shard configuration for shard {} changed catchain seqno \
                from {} to {} (only updates by at most one are allowed)",
                shard,
                cc_seqno,
                info.descr.next_catchain_seqno
            )
        }
        if !cc_updated && self.update_shard_cc {
            reject_query!(
                "new shard configuration for shard {} has unchanged catchain seqno {}, \
                but it must have been updated for all shards",
                shard,
                cc_seqno
            )
        }
        let bm_cleared = !info.descr.before_merge && old_before_merge;
        if !cc_updated && bm_cleared && !workchain_created {
            reject_query!(
                "new shard configuration for shard {} has unchanged catchain seqno {} \
                while the before_merge bit has been cleared",
                shard,
                cc_seqno
            )
        }
        if cc_updated && !(self.update_shard_cc || bm_cleared) {
            reject_query!(
                "new shard configuration for shard {} has increased catchain seqno {} \
                without a good reason",
                shard,
                cc_seqno
            )
        }

        base.result
            .min_shard_ref_mc_seqno
            .fetch_min(info.descr.min_ref_mc_seqno, Ordering::Relaxed);
        base.result.max_shard_utime.fetch_max(info.descr.gen_utime, Ordering::Relaxed);
        base.result.max_shard_lt.fetch_max(info.descr.end_lt, Ordering::Relaxed);
        // dbg!(base.min_shard_ref_mc_seqno, base.max_shard_utime, base.max_shard_lt);
        Ok(())
    }

    #[cfg(feature = "xp25")]
    async fn check_ref_shard_blocks(
        &mut self,
        base: &ValidateBase,
        mc_data: &McData,
    ) -> Result<()> {
        let d = &self.next_block_descr;

        log::debug!(target: "validate_query", "({d}): check_ref_shard_blocks");

        enum RefBlockStatus {
            ToCheck(BlockIdExt),
            Checking(BlockIdExt),
        }
        #[derive(Debug)]
        enum KnownBlockStatus {
            NotCommited,
            Commited,
            Applied,
            Checked,
        }

        fn remember_block(
            known_blocks: &mut HashMap<BlockIdExt, KnownBlockStatus>,
            id: BlockIdExt,
            status: KnownBlockStatus,
            block_descr: &str,
        ) {
            log::trace!(target: "validate_query", "({block_descr}): remember {:?} {}", status, id);
            known_blocks.insert(id, status);
        }

        // We have next categories of blocks:
        // 1. Applied - blocks that are commited into masterchain and applied at current node;
        // 2. Commited - blocks that are commited into masterchain but not applied at current node;
        // 3. Not commited - blocks that are not commited into masterchain. It is possible to have forks among them.
        //
        // Among not commited blocks we have:
        // 1. Top shards blocks - headers of shards (except own shard)
        // 2. Blocks older than top shards blocks
        // 3. Blocks newer than top shards blocks
        //
        // Check ref shard blocks algorithm:
        // 1. collect all commited blocks
        // 2. collect all top blocks and blocks older than top blocks downto last applied (or commited)
        // 3. collect own shard blocks from prev blocks upto last applied (or commited)
        // 4. recursively check that all ref shard blocks refer to previosly collected blocks
        //    or applied blocks. Applied blocks can't be newer than top blocks in same shard.

        let mut known_blocks = HashMap::new();
        let mut top_blocks = Vec::new();

        // 1. Collecting all commited blocks.
        // It is easy to detect applied blocks using block handle.
        // But to detect commited blocks we need to check masterchain blocks.

        let now = Instant::now();
        let mut stack = mc_data.state.top_blocks_all()?;
        while let Some(id) = stack.pop() {
            let handle = self
                .engine
                .load_block_handle(&id)?
                .ok_or_else(|| error!("Can't load handle for block {}", id))?;
            if handle.is_applied() {
                remember_block(&mut known_blocks, id, KnownBlockStatus::Applied, d);
            } else {
                stack.push(self.engine.load_block_prev1(&id)?);
                if let Some(prev2) = self.engine.load_block_prev2(&id)? {
                    stack.push(prev2);
                }
                remember_block(&mut known_blocks, id, KnownBlockStatus::Commited, d);
            }
        }
        let elapsed = now.elapsed();
        metrics::histogram!("ton_node_validator_check_refs_collect_committed_seconds")
            .record(elapsed);
        if elapsed.as_millis() > 10 {
            log::warn!(target: "validate_query", "({d}): Collecting commited blocks TIME {} ms",
                elapsed.as_millis());
        }
        // For first block - remember shard's zerostate
        if base.block_id().seq_no() == 1 && base.shard().is_masterchain() {
            let workchains = mc_data.config().workchains()?;
            workchains.iterate_with_keys(|wc_id, wc| {
                let zs_id = BlockIdExt {
                    shard_id: ShardIdent::with_tagged_prefix(wc_id, SHARD_FULL)?,
                    seq_no: 0,
                    root_hash: wc.zerostate_root_hash,
                    file_hash: wc.zerostate_file_hash,
                };
                remember_block(&mut known_blocks, zs_id, KnownBlockStatus::Applied, d);
                Ok(true)
            })?;
        }

        // 2. collect all top blocks and blocks older than top blocks

        let mut collect_known_chain = |top_block_id: &BlockIdExt,
                                       known_blocks: &mut HashMap<BlockIdExt, KnownBlockStatus>|
         -> Result<()> {
            log::trace!(target: "validate_query", "({d}): collect_known_chain({})", top_block_id);
            top_blocks.push(top_block_id.clone());
            let mut stack = vec![top_block_id.clone()];
            // Collect prev blocks upto first applied
            while let Some(id) = stack.pop() {
                match known_blocks.get(&id) {
                    Some(KnownBlockStatus::Checked) => {
                        fail!(
                            "INTERNAL ERROR: KnownBlockStatus::Checked is impossible \
                            status at collect_known_chain {} ",
                            id
                        );
                    }
                    None => {
                        stack.push(self.engine.load_block_prev1(&id)?);
                        if let Some(prev2) = self.engine.load_block_prev2(&id)? {
                            stack.push(prev2);
                        }
                        remember_block(known_blocks, id, KnownBlockStatus::NotCommited, d);
                    }
                    _ => (),
                }
            }
            Ok(())
        };

        let now = Instant::now();
        if base.shard().is_masterchain() {
            self.new_mc_shards.iterate_shards(|shard: ShardIdent, descr: ShardDescr| {
                let top_block = BlockIdExt {
                    shard_id: shard,
                    seq_no: descr.seq_no,
                    root_hash: descr.root_hash,
                    file_hash: descr.file_hash,
                };
                collect_known_chain(&top_block, &mut known_blocks)?;
                Ok(true)
            })?;
        } else {
            base.extra.read_wc_custom()?.ref_shard_blocks.iterate_shard_block_refs(
                |top_block, end_lt| {
                    // Remember max LT for future checks in check_utime_lt
                    base.result.max_shard_lt.fetch_max(end_lt, Ordering::Relaxed);

                    // Check top block descr if need
                    if !top_block.shard().intersect_with(base.shard()) {
                        // for this moment known_blocks contains only applied or commited blocks
                        // so if it doesn't contain our block it means we maybe need to check it
                        if !known_blocks.contains_key(&top_block) {
                            // if ref block is updated we must check it
                            for prev_state in &base.prev_states {
                                if prev_state
                                    .state()?
                                    .read_wc_custom()?
                                    .ok_or_else(|| error!("No wc_custom in prev state"))?
                                    .ref_shard_blocks
                                    .ref_shard_block(top_block.shard())?
                                    .map_or_else(
                                        || true,
                                        |rsb| rsb.root_hash != top_block.root_hash,
                                    )
                                {
                                    self.check_tsbd(base, mc_data, &top_block)?;
                                    break;
                                }
                            }
                        }
                    } else {
                        //if base.block_id().seq_no() > 1 {
                        if !base.prev_blocks_ids.contains(&top_block) {
                            fail!("Ref block {} is not a prev block", top_block);
                        }
                    }

                    collect_known_chain(&top_block, &mut known_blocks)?;
                    Ok(true)
                },
            )?;
        }
        let elapsed = now.elapsed();
        metrics::histogram!("ton_node_validator_check_refs_collect_chains_seconds").record(elapsed);
        if elapsed.as_millis() > 10 {
            log::warn!(target: "validate_query", "({d}): Collecting allowed blocks chains TIME {} ms",
                elapsed.as_millis());
        }

        // 3. collect own shard blocks
        if !self.shard.is_masterchain() {
            for prev_state in &base.prev_states {
                base.result.max_shard_lt.fetch_max(prev_state.gen_lt()?, Ordering::Relaxed);
                if !known_blocks.contains_key(&prev_state.block_id()) {
                    collect_known_chain(prev_state.block_id(), &mut known_blocks)?;
                }
            }
        }

        // 4. dependency checking

        let now = Instant::now();
        let mut whitespace = String::new();
        for top_block in &top_blocks {
            let mut stack = Vec::new();
            stack.push(RefBlockStatus::ToCheck(top_block.clone()));
            while let Some(checked_id) = stack.pop() {
                match checked_id {
                    RefBlockStatus::ToCheck(checked_id) => {
                        log::debug!(target: "validate_query", "({d}):{whitespace}checking {checked_id}");

                        match known_blocks.get(&checked_id) {
                            Some(KnownBlockStatus::Applied) => {
                                log::debug!(target: "validate_query", "({d}):{whitespace}{checked_id} is applied (cached)");
                                continue;
                            }
                            Some(KnownBlockStatus::Checked) => {
                                log::debug!(target: "validate_query", "({d}):{whitespace}{checked_id} is already checked");
                                continue;
                            }
                            Some(KnownBlockStatus::Commited) => {
                                log::debug!(target: "validate_query", "({d}):{whitespace}{checked_id} will be commited");
                                continue;
                            }
                            Some(KnownBlockStatus::NotCommited) => {
                                let state = self
                                    .engine
                                    .clone()
                                    .wait_state(&checked_id, Some(1_000), false)
                                    .await?;

                                stack.push(RefBlockStatus::Checking(checked_id.clone()));
                                if log::log_enabled!(target: "validate_query", log::Level::Debug) {
                                    whitespace.push('|');
                                }

                                state
                                    .state()?
                                    .read_wc_custom()?
                                    .ok_or_else(|| {
                                        error!("RefBlock {} has no ref_shard_blocks", checked_id)
                                    })?
                                    .ref_shard_blocks
                                    .iterate_shard_block_refs(|ref_block_id, _| {
                                        stack.push(RefBlockStatus::ToCheck(ref_block_id));
                                        Ok(true)
                                    })?;
                            }
                            None => {
                                let handle =
                                    self.engine.load_block_handle(&checked_id)?.ok_or_else(
                                        || error!("RefBlock {} can't load handle", checked_id),
                                    )?;
                                if handle.is_applied() {
                                    // Check that block is not too new
                                    let top_block = top_blocks
                                        .iter()
                                        .filter(|t| t.shard().intersect_with(handle.id().shard()))
                                        .max_by_key(|t| t.seq_no)
                                        .ok_or_else(|| {
                                            error!(
                                                "Ref block {checked_id} can't find the top \
                                            block of the proper shard"
                                            )
                                        })?;
                                    if checked_id.seq_no() > top_block.seq_no() {
                                        reject_query!(
                                            "Ref block {checked_id} is \
                                            newer than top block in this shard"
                                        );
                                    }

                                    known_blocks
                                        .insert(checked_id.clone(), KnownBlockStatus::Applied);
                                    log::debug!(target: "validate_query", "({d}):{whitespace}{checked_id} is applied");
                                } else {
                                    reject_query!("Ref block {checked_id} is unknown");
                                }
                            }
                        }
                    }
                    RefBlockStatus::Checking(checked_id) => {
                        if log::log_enabled!(target: "validate_query", log::Level::Debug) {
                            whitespace.pop();
                        }
                        log::debug!(target: "validate_query", "({d}):{whitespace}{checked_id} checked");
                        known_blocks.insert(checked_id, KnownBlockStatus::Checked);
                    }
                }
            }
        }
        let elapsed = now.elapsed();
        metrics::histogram!("ton_node_validator_check_refs_deps_seconds").record(elapsed);
        if elapsed.as_millis() > 10 {
            log::warn!(target: "validate_query", "({d}): Dependency checking TIME {} ms",
                elapsed.as_millis());
        }
        Ok(())
    }

    // checks old_shard_conf_ -> base.mc_extra.shards() transition using top_shard_descr_dict_ from collated data
    // similar to Collator::update_shard_config()
    fn check_shard_layout(&mut self, base: &ValidateBase, mc_data: &McData) -> Result<()> {
        if !base.shard().is_masterchain() {
            return Ok(());
        }
        let prev_now = base.prev_state()?.gen_time();
        if prev_now > base.now() {
            reject_query!("creation time is not monotonic: {} after {}", base.now(), prev_now)
        }
        let ccvc = base.next_state_extra.config.catchain_config()?;
        let wc_set = base.next_state_extra.config.workchains()?;
        self.update_shard_cc = base.info.key_block();
        {
            self.update_shard_cc |=
                base.now() / ccvc.shard_catchain_lifetime > prev_now / ccvc.shard_catchain_lifetime;
        }
        if self.update_shard_cc {
            log::debug!(
                target: "validate_query",
                "({}): catchain_seqno of all shards must be updated",
                self.next_block_descr
            );
        }

        let mut wc_id = INVALID_WORKCHAIN_ID;
        let mut wc_info = None;
        self.new_mc_shards
            .clone()
            .iterate_shards_with_siblings(|shard, descr, sibling| {
                if wc_id != shard.workchain_id() {
                    wc_id = shard.workchain_id();
                    if wc_id == INVALID_WORKCHAIN_ID || wc_id == MASTERCHAIN_ID {
                        reject_query!(
                            "new shard configuration contains shards of invalid workchain {}",
                            wc_id
                        )
                    }
                    wc_info = wc_set.get(&wc_id)?;
                }
                let descr = McShardRecord::from_shard_descr(shard, descr);
                if let Some(sibling) = sibling {
                    let sibling = McShardRecord::from_shard_descr(descr.shard().sibling(), sibling);
                    self.check_one_shard(base, mc_data, &descr, Some(&sibling), wc_info.as_ref())?;
                    self.check_one_shard(base, mc_data, &sibling, Some(&descr), wc_info.as_ref())?;
                } else {
                    self.check_one_shard(base, mc_data, &descr, None, wc_info.as_ref())?;
                }
                Ok(true)
            })
            .map_err(|err| error!("new shard configuration is invalid : {:?}", err))?;

        base.prev_state_extra.config.workchains()?.iterate_keys(|wc_id: i32| {
            if base.mc_extra.shards().get(&wc_id)?.is_none() {
                reject_query!(
                    "shards of workchain {} existed in previous \
                    shardchain configuration, but are absent from new",
                    wc_id
                )
            }
            Ok(true)
        })?;
        wc_set.iterate_with_keys(|wc_id: i32, wc_info| {
            if wc_info.active && base.mc_extra.shards().get(&wc_id)?.is_none() {
                reject_query!(
                    "workchain {} is active, but is absent from new shard configuration",
                    wc_id
                )
            }
            Ok(true)
        })?;
        self.check_mc_validator_info(
            base,
            base.info.key_block()
                || (base.now() / ccvc.mc_catchain_lifetime > prev_now / ccvc.mc_catchain_lifetime),
        )
    }

    // similar to Collator::register_shard_block_creators
    fn register_shard_block_creators(
        &mut self,
        _base: &ValidateBase,
        creator_list: &[UInt256],
    ) -> Result<()> {
        for x in creator_list {
            log::debug!(
                target: "validate_query",
                "({}): registering block creator {x:x}",
                self.next_block_descr
            );
            if !x.is_zero() {
                *self.block_create_count.entry(x.clone()).or_default() += 1;
            }
            self.block_create_total += 1;
        }
        Ok(())
    }

    // parallel to 4. of Collator::create_mc_state_extra()
    // checks validator_info in mc_state_extra
    fn check_mc_validator_info(&self, base: &ValidateBase, update_mc_cc: bool) -> Result<()> {
        CHECK!(&base.prev_state_extra, inited);
        CHECK!(&base.next_state_extra, inited);
        let old_info = &base.prev_state_extra.validator_info;
        let new_info = &base.next_state_extra.validator_info;

        let cc_updated = new_info.catchain_seqno != old_info.catchain_seqno;
        if new_info.catchain_seqno != old_info.catchain_seqno + cc_updated as u32 {
            reject_query!(
                "new masterchain state increased masterchain catchain seqno from {} to {} \
                (only updates by at most one are allowed)",
                old_info.catchain_seqno,
                new_info.catchain_seqno
            )
        }
        if cc_updated != update_mc_cc {
            match cc_updated {
                true => reject_query!("masterchain catchain seqno increased without any reason"),
                false => reject_query!("masterchain catchain seqno unchanged while it had to"),
            }
        }
        let now = base.next_state()?.gen_time();
        let prev_now = base.prev_state()?.gen_time();
        let ccvc = base.next_state_extra.config.catchain_config()?;
        let cur_validators = base.next_state_extra.config.validator_set()?;
        let lifetime = ccvc.mc_catchain_lifetime;
        let is_key_block = base.info.key_block();
        let mut cc_updated = false;
        let catchain_seqno = new_info.catchain_seqno;
        if is_key_block || (now / lifetime > prev_now / lifetime) {
            cc_updated = true;
            log::debug!(
                target: "validate_query",
                "({}): increased masterchain catchain seqno to {catchain_seqno}",
                self.next_block_descr
            );
        }
        let subset = calc_subset_for_masterchain(
            &cur_validators,
            &base.next_state_extra.config,
            catchain_seqno,
        )?;

        if subset.validators.is_empty() {
            reject_query!(
                "cannot compute next masterchain validator set from new masterchain state"
            )
        }

        let vlist_hash = ValidatorSet::calc_subset_hash_short(
            &subset.validators,
            /* new_info.catchain_seqno */ 0,
        )?;
        if new_info.validator_list_hash_short != vlist_hash {
            reject_query!(
                "new masterchain validator list hash incorrect hash: expected {}, found {}",
                new_info.validator_list_hash_short,
                vlist_hash
            );
        }
        log::debug!(
            target: "validate_query",
            "({}): masterchain validator set hash changed from {} to {}",
            self.next_block_descr,
            old_info.validator_list_hash_short,
            vlist_hash
        );
        if new_info.nx_cc_updated != cc_updated & self.update_shard_cc {
            reject_query!("new_info.nx_cc_updated has incorrect value {}", new_info.nx_cc_updated)
        }

        Ok(())
    }

    fn check_utime_lt(&self, base: &ValidateBase, mc_data: &McData) -> Result<()> {
        CHECK!(&base.config_params, inited);
        // C++ parity: allow_same_timestamp_ = global_version_ >= 13.
        // Depends only on the global protocol version, not on consensus type.
        // When true, also skips the future-time check (base.now > engine.now + 15)
        // which C++ simplex testnet does not have.
        let allow_same_timestamp = {
            #[cfg(feature = "xp25")]
            {
                true
            }
            #[cfg(not(feature = "xp25"))]
            {
                base.config_params.global_version()
                    >= super::SIMPLEX_ALLOW_SAME_TIMESTAMP_FROM_GLOBAL_VERSION
            }
        };
        let mut gen_lt = u64::MIN;

        for state in &base.prev_states {
            if base.info.start_lt() <= state.state()?.gen_lt() {
                reject_query!(
                    "block has start_lt {} less than or equal to lt {} of the previous state",
                    base.info.start_lt(),
                    state.state()?.gen_lt()
                )
            }
            let prev_utime = state.state()?.gen_time();
            let creation_time_invalid = if allow_same_timestamp {
                base.now() < prev_utime
            } else {
                base.now() <= prev_utime
            };
            if creation_time_invalid {
                reject_query!(
                    "block has creation time {} less than {}that of the previous state ({})",
                    base.now(),
                    if allow_same_timestamp { "" } else { "or equal to " },
                    prev_utime
                )
            }
            gen_lt = gen_lt.max(state.state()?.gen_lt());
        }
        let ref_mc_utime = mc_data.state.state()?.gen_time();
        let creation_time_invalid = if allow_same_timestamp {
            base.now() < ref_mc_utime
        } else {
            base.now() <= ref_mc_utime
        };
        if creation_time_invalid {
            reject_query!(
                "block has creation time {} less than {}that of the reference masterchain state ({})",
                base.now(),
                if allow_same_timestamp { "" } else { "or equal to " },
                ref_mc_utime
            )
        }

        // C++ parity: C++ variants (mainnet, simplex-testnet, alpenglow-work) do not
        // reject blocks for being too far in the future. To stay compatible with blocks
        // produced by C++ collators, we only emit a warning instead of rejecting.
        if !allow_same_timestamp {
            let now = self.engine.now();
            if base.now() > now + 15 {
                log::warn!(
                    "block has creation time {} too much in the future (local time is {now})",
                    base.now(),
                );
            }
        }

        if base.info.start_lt() <= mc_data.state.state()?.gen_lt() {
            reject_query!(
                "block has start_lt {} less than or equal to lt {} \
                of the reference masterchain state",
                base.info.start_lt(),
                mc_data.state.state()?.gen_lt()
            )
        }

        let lt_bound = gen_lt.max(mc_data.state.state()?.gen_lt()).max(base.max_shard_lt());

        if base.info.start_lt() > lt_bound + base.config_params.get_lt_align() * 4 {
            reject_query!(
                "block has start_lt {} which is too large \
                without a good reason (lower bound is {})",
                base.info.start_lt(),
                lt_bound + 1
            )
        }

        let max_lt_growth = mc_data.config().get_max_lt_growth();
        if base.shard().is_masterchain() && base.info.start_lt() - gen_lt > max_lt_growth {
            reject_query!(
                "block increases logical time from previous state by {} \
                which exceeds the limit ({})",
                base.info.start_lt() - gen_lt,
                max_lt_growth
            )
        }
        let delta_hard = base.block_limits.lt_delta().hard_limit() as u64;
        if base.info.end_lt() - base.info.start_lt() > delta_hard {
            reject_query!(
                "block increased logical time by {} which is larger than the hard limit {}",
                base.info.end_lt() - base.info.start_lt(),
                delta_hard
            )
        }
        if base.is_simplex {
            match base.now_ms {
                None => reject_query!("now_ms is not set"),
                Some(now_ms) if now_ms / 1000 != base.info.gen_utime() as u64 => {
                    reject_query!(
                        "gen_utime is {}, but gen_utime_ms in ConsensusExtraData is {}",
                        base.info.gen_utime(),
                        now_ms
                    )
                }
                _ => {}
            }
        }
        Ok(())
    }

    /*
     *
     *  METHODS CALLED FROM try_validate() stage 1
     *
     */

    fn load_block_data(base: &mut ValidateBase) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): unpacking block structures",
            base.next_block_descr
        );
        base.in_msg_descr = base.extra.read_in_msg_descr()?;
        base.out_msg_descr = base.extra.read_out_msg_descr()?;
        base.account_blocks = base.extra.read_account_blocks()?;
        // run some hand-written checks from block::tlb::
        // (letmatic tests from block::gen:: have been already run for the entire block)
        // count and validate
        log::debug!(target: "validate_query", "({}): validating InMsgDescr", base.next_block_descr);
        // base.in_msg_descr.count(1000000)?;
        log::debug!(target: "validate_query", "({}): validating OutMsgDescr", base.next_block_descr);
        // base.out_msg_descr.count(1000000)?;
        log::debug!(target: "validate_query", "({}): validating ShardAccountBlocks", base.next_block_descr);
        // base.account_blocks.count(1000000)?;
        Ok(())
    }

    fn precheck_value_flow(base: Arc<ValidateBase>) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): value flow: {}",
            base.next_block_descr,
            base.value_flow
        );
        // if !base.value_flow.validate() {
        //     reject_query!("ValueFlow of block {} is invalid (in-balance is not equal to out-balance)", base.block_id())
        // }
        if !base.shard().is_masterchain() && !base.value_flow.minted.is_zero()? {
            reject_query!(
                "ValueFlow of block {} \
                is invalid (non-zero minted value in a non-masterchain block)",
                base.block_id()
            )
        }
        if !base.shard().is_masterchain() && !base.value_flow.recovered.is_zero()? {
            reject_query!(
                "ValueFlow of block {} \
                is invalid (non-zero recovered value in a non-masterchain block)",
                base.block_id()
            )
        }
        if !base.value_flow.recovered.is_zero()? && base.recover_create_msg.is_none() {
            reject_query!(
                "ValueFlow of block {} \
                has a non-zero recovered fees value, but there is no recovery InMsg",
                base.block_id()
            )
        }
        if base.value_flow.recovered.is_zero()? && base.recover_create_msg.is_some() {
            reject_query!(
                "ValueFlow of block {} \
                has a zero recovered fees value, but there is a recovery InMsg",
                base.block_id()
            )
        }
        if !base.value_flow.minted.is_zero()? && base.mint_msg.is_none() {
            reject_query!(
                "ValueFlow of block {} \
                has a non-zero minted value, but there is no mint InMsg",
                base.block_id()
            )
        }
        if base.value_flow.minted.is_zero()? && base.mint_msg.is_some() {
            reject_query!(
                "ValueFlow of block {} \
                has a zero minted value, but there is a mint InMsg",
                base.block_id()
            )
        }
        if !base.value_flow.minted.is_zero()? {
            let to_mint = Self::compute_minted_amount(&base).map_err(|err| {
                error!(
                    "cannot compute the correct amount of extra currencies to be minted : {}",
                    err
                )
            })?;
            if base.value_flow.minted != to_mint {
                reject_query!(
                    "invalid extra currencies amount to be minted: declared {}, expected {}",
                    base.value_flow.minted,
                    to_mint
                )
            }
        }
        let create_fee =
            base.config_params.block_create_fees(base.shard().is_masterchain()).unwrap_or_default();
        let create_fee = CurrencyCollection::from_coins(create_fee >> base.shard().prefix_len());
        if base.value_flow.created != create_fee {
            reject_query!(
                "ValueFlow of block {} declares block creation fee {}, \
                but the current configuration expects it to be {}",
                base.block_id(),
                base.value_flow.created,
                create_fee
            )
        }
        if !base.value_flow.fees_imported.is_zero()? && !base.shard().is_masterchain() {
            reject_query!(
                "ValueFlow of block {} \
                is invalid (non-zero fees_imported in a non-masterchain block)",
                base.block_id()
            )
        }
        let cc = base.prev_state_accounts.full_balance();
        if cc != &base.value_flow.from_prev_blk {
            reject_query!(
                "ValueFlow for {} declares from_prev_blk={} \
                but the sum over all accounts present in the previous state is {}",
                base.block_id(),
                base.value_flow.from_prev_blk,
                cc
            )
        }
        let cc = base.next_state_accounts.full_balance();
        if cc != &base.value_flow.to_next_blk {
            reject_query!(
                "ValueFlow for {} declares to_next_blk={} but the sum over all accounts \
                present in the new state is {}",
                base.block_id(),
                base.value_flow.to_next_blk,
                cc
            )
        }
        let cc = base.in_msg_descr.full_import_fees();
        if cc.value_imported != base.value_flow.imported {
            reject_query!(
                "ValueFlow for {} declares imported={} but the sum over all inbound messages \
                listed in InMsgDescr is {}",
                base.block_id(),
                base.value_flow.imported,
                cc.value_imported
            );
        }
        let fees_import = CurrencyCollection::from_coins(cc.fees_collected);
        let cc = base.out_msg_descr.full_exported();
        if cc != &base.value_flow.exported {
            reject_query!(
                "ValueFlow for {} declares exported={} but the sum over all outbound messages \
                listed in OutMsgDescr is {}",
                base.block_id(),
                base.value_flow.exported,
                cc
            )
        }
        let transaction_fees = base.account_blocks.full_transaction_fees();
        let expected_fee_burned = Self::expected_fee_burned(&base, transaction_fees, &fees_import)?;
        if !base.shard().is_masterchain() && !base.value_flow.burned.is_zero()? {
            reject_query!(
                "ValueFlow of block {} is invalid (non-zero burned value in a non-masterchain block)",
                base.block_id()
            )
        }

        let mut expected_fees = transaction_fees.clone();
        expected_fees.add(&base.value_flow.fees_imported)?;
        expected_fees.add(&base.value_flow.created)?;
        expected_fees.add(&fees_import)?;
        expected_fees.sub(&expected_fee_burned)?;
        if base.value_flow.fees_collected != expected_fees {
            reject_query!(
                "ValueFlow for {} declares fees_collected={} but \
                the total message import fees are {}, the total transaction fees are {}, \
                creation fee for this block is {} and the total imported fees from shards \
                are {}, the burned fees are {} with a total of {}",
                base.block_id(),
                base.value_flow.fees_collected.coins,
                fees_import,
                transaction_fees.coins,
                base.value_flow.created.coins,
                base.value_flow.fees_imported.coins,
                expected_fee_burned.coins,
                expected_fees.coins
            )
        }
        Ok(())
    }

    fn expected_fee_burned(
        base: &ValidateBase,
        transaction_fees: &CurrencyCollection,
        fees_import: &CurrencyCollection,
    ) -> Result<CurrencyCollection> {
        if !base.shard().is_masterchain() {
            return Ok(Default::default());
        }
        let Some(ConfigParamEnum::ConfigParam5(burning_cfg)) = base.config_params.config(5)? else {
            return Ok(Default::default());
        };

        let total_fees = transaction_fees.coins.as_u128() + fees_import.coins.as_u128();
        let mut burned = burning_cfg.calculate_burned_fees(total_fees)?;

        let mut imported_base = base.value_flow.fees_imported.clone();
        if !imported_base.sub(&base.mc_extra.fees().root_extra().create)? {
            fail!(
                "fees_imported ({}) is smaller than imported created fees ({})",
                base.value_flow.fees_imported,
                base.mc_extra.fees().root_extra().create
            );
        }
        let burned_imported = burning_cfg.calculate_burned_fees(imported_base.coins.as_u128())?;
        burned.add(&burned_imported)?;
        Ok(CurrencyCollection::from_coins(burned))
    }

    fn check_burned_value_flow(base: &ValidateBase) -> Result<()> {
        if !base.shard().is_masterchain() {
            return Ok(());
        }
        let fees_import =
            CurrencyCollection::from_coins(base.in_msg_descr.full_import_fees().fees_collected);
        let mut expected_burned = Self::expected_fee_burned(
            base,
            base.account_blocks.full_transaction_fees(),
            &fees_import,
        )?;
        expected_burned.coins.add(
            &*base
                .result
                .blackhole_burned
                .lock()
                .map_err(|_| error!("blackhole burned accumulator is poisoned"))?,
        )?;
        if base.value_flow.burned != expected_burned {
            reject_query!(
                "ValueFlow of block {} declares burned fees {}, but the expected value is {}",
                base.block_id(),
                base.value_flow.burned.coins,
                expected_burned
            )
        }
        Ok(())
    }

    // similar to Collator::compute_minted_amount()
    fn compute_minted_amount(base: &ValidateBase) -> Result<CurrencyCollection> {
        let mut to_mint = CurrencyCollection::default();
        if !base.shard().is_masterchain() {
            return Ok(to_mint);
        }
        let to_mint_config = match base.config_params.config(7)? {
            Some(ConfigParamEnum::ConfigParam7(param)) => param.to_mint,
            _ => return Ok(to_mint),
        };
        to_mint_config
            .iterate_with_keys(|curr_id: u32, amount| {
                let amount2 =
                    base.prev_state_extra.global_balance.get_other(curr_id)?.unwrap_or_default();
                if amount > amount2 {
                    let mut delta = amount.clone();
                    delta.sub(&amount2)?;
                    log::debug!(
                        target: "validate_query",
                        "({}): currency #{}: existing {}, required {}, to be minted {}",
                        base.next_block_descr,
                        curr_id,
                        amount2,
                        amount,
                        delta
                    );
                    if curr_id != 0 {
                        to_mint.set_other_ex(curr_id, &delta)?;
                    }
                }
                Ok(true)
            })
            .map_err(|err| error!("error scanning extra currencies to be minted : {}", err))?;
        if !to_mint.is_zero()? {
            log::debug!(
                target: "validate_query",
                "({}): new currencies to be minted: {}",
                base.next_block_descr,
                to_mint
            );
        }
        Ok(to_mint)
    }

    fn precheck_one_account_update(
        base: &ValidateBase,
        acc_id: AccountId,
        old_val_extra: Option<(ShardAccount, DepthBalanceInfo)>,
        new_val_extra: Option<(ShardAccount, DepthBalanceInfo)>,
    ) -> Result<bool> {
        log::debug!(
            target: "validate_query",
            "({}): checking update of account {:x}",
            base.next_block_descr,
            acc_id
        );
        let Some(acc_blk) = base.account_blocks.get(&acc_id)? else {
            reject_query!(
                "the state of account {acc_id:x} changed in the new state with respect to the old \
                state, but the block contains no AccountBlock for this account",
            )
        };
        let hash_upd = acc_blk.read_state_update().map_err(|err| {
            error!(
                "cannot extract (HASH_UPDATE Account) from the AccountBlock of {:x} : {}",
                acc_id, err
            )
        })?;
        if acc_id != *acc_blk.account_id() {
            reject_query!(
                "AccountBlock of account {:x} appears to belong to another account {:x}",
                acc_id,
                acc_blk.account_id()
            )
        }
        if let Some((old_state, _old_extra)) = old_val_extra {
            if hash_upd.old_hash != *old_state.account_cell().repr_hash() {
                reject_query!(
                    "(HASH_UPDATE Account) from the AccountBlock of {:x} \
                    has incorrect old hash",
                    acc_id
                )
            }
        }
        if let Some((new_state, new_extra)) = new_val_extra {
            if hash_upd.new_hash != *new_state.account_cell().repr_hash() {
                reject_query!(
                    "(HASH_UPDATE Account) from the AccountBlock of {:x} \
                    has incorrect new hash",
                    acc_id
                )
            }
            // check augmentation
            let new_account = new_state
                .read_account()
                .map_err(|err| error!("cannot read Account of {acc_id:x} from new state: {err}"))?;
            let extra = new_account.aug()?;
            if extra != new_extra {
                reject_query!(
                    "invalid account {acc_id:x} augmentation {new_extra:?}, recomputed {extra:?}",
                )
            }
        }
        Ok(true)
    }

    fn precheck_account_updates(base: Arc<ValidateBase>) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): pre-checking all Account updates between the old and the new state",
            base.next_block_descr
        );
        // let prev_accounts = base.prev_state_accounts.clone();
        // let next_accounts = base.next_state_accounts.clone();
        base.prev_state_accounts.scan_diff_with_aug(
            &base.next_state_accounts,
            |key, old_val_extra, new_val_extra| {
                Self::precheck_one_account_update(&base, key, old_val_extra, new_val_extra)
            },
        )?;
        Ok(())
    }

    fn precheck_one_transaction(
        base: &ValidateBase,
        acc_id: &AccountId,
        trans_lt: u64,
        trans_root: Cell,
        prev_trans_lt: &mut u64,
        prev_trans_hash: &mut UInt256,
        prev_trans_lt_len: &mut u64,
        acc_state_hash: &mut UInt256,
    ) -> Result<Option<(UInt256, bool)>> {
        log::debug!(
            target: "validate_query",
            "({}): pre-checking Transaction {}",
            base.next_block_descr,
            trans_lt
        );
        let trans = Transaction::construct_from_cell(trans_root.clone())?;
        if trans.account_id() != acc_id || trans.logical_time() != trans_lt {
            reject_query!(
                "transaction {} of {:x} claims to be transaction {} of {:x}",
                trans_lt,
                acc_id,
                trans.logical_time(),
                trans.account_id()
            )
        }
        if trans.now() != base.now() {
            reject_query!(
                "transaction {} of {:x} claims that current time is {}
                while the block header indicates {}",
                trans_lt,
                acc_id,
                trans.now(),
                base.now()
            )
        }
        if trans.prev_trans_hash() != prev_trans_hash || &trans.prev_trans_lt() != prev_trans_lt {
            reject_query!(
                "transaction {} of {:x} claims that the previous transaction was {}:{:x} \
                while the correct value is {}:{:x}",
                trans_lt,
                acc_id,
                trans.prev_trans_lt(),
                trans.prev_trans_hash(),
                prev_trans_lt,
                prev_trans_hash
            )
        }
        if trans_lt < *prev_trans_lt + *prev_trans_lt_len {
            reject_query!(
                "transaction {} of {:x} starts at logical time {}, \
                earlier than the previous transaction {} .. {} ends",
                trans_lt,
                acc_id,
                trans_lt,
                prev_trans_lt,
                *prev_trans_lt + *prev_trans_lt_len
            )
        }
        let lt_len = trans.msg_count() as u64 + 1;
        if trans_lt <= base.info.start_lt() || trans_lt + lt_len > base.info.end_lt() {
            reject_query!(
                "transaction {} .. {} of {:x} is not inside the logical time interval {} .. {} \
                of the encompassing new block",
                trans_lt,
                trans_lt + lt_len,
                acc_id,
                base.info.start_lt(),
                base.info.end_lt()
            )
        }
        let hash_upd = trans.read_state_update()?;
        if &hash_upd.old_hash != acc_state_hash {
            reject_query!(
                "transaction {} of {:x} claims to start from account state with hash {:x} \
                while the actual value is {:x}",
                trans_lt,
                acc_id,
                hash_upd.old_hash,
                acc_state_hash
            )
        }
        let msg_info = match (trans.in_msg_cell(), trans.read_in_msg()?) {
            (Some(root), Some(msg)) => Some((root.repr_hash().clone(), msg.is_internal())),
            _ => None,
        };
        *prev_trans_lt_len = lt_len;
        *prev_trans_lt = trans_lt;
        *prev_trans_hash = trans_root.repr_hash().clone();
        *acc_state_hash = hash_upd.new_hash;
        let mut c = 0;
        // trans.out_msgs.iterate_slices_with_keys(|key, value| {
        trans.out_msgs.iterate_keys(|key: UInt15| {
            if c != key.0 {
                reject_query!(
                    "transaction {} of {:x} has invalid indices \
                    in the out_msg dictionary (keys 0 .. {} expected)",
                    trans_lt,
                    acc_id,
                    trans.msg_count() - 1
                )
            } else {
                c += 1;
                Ok(true)
            }
        })?;
        Ok(msg_info)
    }

    // NB: could be run in parallel for different accounts
    fn precheck_one_account_block(
        base: &ValidateBase,
        acc_id: &AccountId,
        acc_blk: AccountBlock,
    ) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): pre-checking AccountBlock for {:x}",
            base.next_block_descr,
            acc_id
        );

        if !base.shard().contains_account(acc_id)? {
            reject_query!(
                "new block {} contains AccountBlock for account {:x} \
                not belonging to the block's shard {}",
                base.block_id(),
                acc_id,
                base.shard()
            )
        }
        let hash_upd = acc_blk.read_state_update()?;
        if acc_blk.account_id() != acc_id {
            reject_query!(
                "AccountBlock of account {:x} appears to belong to another account {:x}",
                acc_id,
                acc_blk.account_id()
            )
        }
        let old_state =
            base.prev_state_accounts.get_serialized(acc_id.clone())?.unwrap_or_default();
        let new_state = base.next_state_accounts.get_serialized(acc_id.clone())?;
        if hash_upd.old_hash != *old_state.account_cell().repr_hash() {
            reject_query!(
                "(HASH_UPDATE Account) from the AccountBlock of {:x} has incorrect old hash",
                acc_id
            )
        }
        if hash_upd.new_hash != *new_state.clone().unwrap_or_default().account_cell().repr_hash() {
            reject_query!(
                "(HASH_UPDATE Account) from the AccountBlock of {:x} has incorrect new hash",
                acc_id
            )
        }
        // acc_blk.transactions.count(1000000)?;
        let min_trans = acc_blk.transactions().get_minmax_key(true, false)?;
        let max_trans = acc_blk.transactions().get_minmax_key(false, false)?;
        let (Some(min_trans), Some(max_trans)) = (min_trans, max_trans) else {
            reject_query!(
                "cannot extract minimal and maximal keys from \
                the transaction dictionary of account {acc_id:x}"
            )
        };
        if min_trans <= base.info.start_lt() || max_trans >= base.info.end_lt() {
            reject_query!(
                "new block contains transactions {} .. {} outside of the block's lt range {} .. {}",
                min_trans,
                max_trans,
                base.info.start_lt(),
                base.info.end_lt()
            )
        }
        let mut last_trans_lt_len = 1;
        let mut last_trans_lt = old_state.last_trans_lt();
        let mut last_trans_hash = old_state.last_trans_hash().clone();
        let mut acc_state_hash = hash_upd.old_hash;
        acc_blk.transactions().iterate_slices(|key, trans_slice| {
            let trans_lt = key.get_int(64)?;
            Self::precheck_one_transaction(
                base,
                &acc_id,
                trans_lt,
                trans_slice.reference(0)?,
                &mut last_trans_lt,
                &mut last_trans_hash,
                &mut last_trans_lt_len,
                &mut acc_state_hash,
            )
            .map_err(|err| {
                error!("transaction {:x} of account {:x} is invalid : {}", trans_lt, acc_id, err)
            })?;

            Ok(true)
        })?;
        if let Some(new_state) = new_state {
            if last_trans_lt != new_state.last_trans_lt()
                || &last_trans_hash != new_state.last_trans_hash()
            {
                reject_query!(
                    "last transaction mismatch for account {:x} : block lists {}:{:x} but \
                    the new state claims that it is {}:{:x}",
                    acc_id,
                    last_trans_lt,
                    last_trans_hash,
                    new_state.last_trans_lt(),
                    new_state.last_trans_hash()
                )
            }
        }
        if acc_state_hash != hash_upd.new_hash {
            reject_query!(
                "final state hash mismatch in (HASH_UPDATE Account) for account {:x}",
                acc_id
            )
        }
        Ok(())
    }

    fn precheck_account_transactions(base: Arc<ValidateBase>, tasks: &mut TasksVec) -> Result<()> {
        // log::debug!(target: "validate_query", "({}): pre-checking all AccountBlocks, \
        //     and all transactions of all accounts", base.next_block_descr);
        base.account_blocks.iterate_with_keys(|key, acc_blk| {
            let base = base.clone();
            Self::add_task(tasks, move || {
                Self::precheck_one_account_block(&base, &key, acc_blk).map_err(|err| {
                    error!(
                        "invalid AccountBlock for account {:x} in the new block {} : {}",
                        key,
                        base.block_id(),
                        err
                    )
                })
            });
            Ok(true)
        })?;
        Ok(())
    }

    fn lookup_transaction(base: &ValidateBase, addr: &AccountId, lt: u64) -> Result<Option<Cell>> {
        if let Some(block) = base.account_blocks.get_serialized(addr.clone())? {
            if let Some(slice) = block.transactions().get_as_slice(&lt)? {
                return Ok(Some(slice.reference(0)?));
            }
        }
        Ok(None)
    }

    // checks that a ^Transaction refers to a transaction present in the ShardAccountBlocks
    fn is_valid_transaction_ref(
        base: &ValidateBase,
        transaction: &Transaction,
        hash: UInt256,
    ) -> Result<()> {
        match Self::lookup_transaction(base, transaction.account_id(), transaction.logical_time())?
        {
            Some(trans_cell) => {
                if trans_cell != hash {
                    reject_query!(
                        "transaction {} of {:x} has a different hash",
                        transaction.logical_time(),
                        transaction.account_id()
                    )
                } else {
                    Ok(())
                }
            }
            None => reject_query!(
                "transaction {} of {:x} not found",
                transaction.logical_time(),
                transaction.account_id()
            ),
        }
    }

    // checks that any change in OutMsgQueue in the state is accompanied by an OutMsgDescr record in the block
    // also checks that the keys are correct
    // Message can be removed from queue if it was not from this shard - split case
    fn precheck_one_message_queue_update(
        base: &ValidateBase,
        out_msg_queue_size: &mut usize,
        out_msg_id: &OutMsgQueueKey,
        old_value: Option<(EnqueuedMsg, u64)>,
        new_value: Option<(EnqueuedMsg, u64)>,
    ) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): checking update of enqueued outbound message {:x}",
            base.next_block_descr,
            out_msg_id,
        );
        let out_msg_opt = base.out_msg_descr.get(&out_msg_id.hash)?;
        log::trace!(
            target: "validate_query",
            "({}): {}",
            base.next_block_descr,
            out_msg_opt.clone().unwrap_or_default(),
        );
        let (enq, dequeue, m_str) = match (old_value, new_value) {
            (Some(_), Some(_)) => {
                reject_query!(
                    "EnqueuedMsg with key {out_msg_id:x} has been changed in the OutMsgQueue, \
                    but the key did not change"
                )
            }
            (None, Some((new_enq, lt))) => {
                let enqueued_lt = new_enq.enqueued_lt();
                if enqueued_lt < base.info.start_lt() || enqueued_lt >= base.info.end_lt() {
                    reject_query!(
                        "new EnqueuedMsg with key {out_msg_id:x} has enqueued_lt={enqueued_lt} \
                        outside of this block's range {} .. {}",
                        base.info.start_lt(),
                        base.info.end_lt()
                    )
                }
                *out_msg_queue_size += 1;
                (MsgEnqueueStuff::from_enqueue(new_enq, lt)?, false, "en")
            }
            (Some((old_enq, lt)), None) => {
                let enqueued_lt = old_enq.enqueued_lt();
                if enqueued_lt >= base.info.start_lt() {
                    reject_query!(
                        "new EnqueuedMsg with key {out_msg_id:x} has enqueued_lt={enqueued_lt} \
                        greater than or equal to this block's start_lt={}",
                        base.info.start_lt()
                    )
                }
                *out_msg_queue_size -= 1;
                (MsgEnqueueStuff::from_enqueue(old_enq, lt)?, true, "de")
            }
            (None, None) => {
                // unreachable
                return Ok(());
            }
        };
        let Some(out_msg) = out_msg_opt else {
            reject_query!(
                "no OutMsgDescr corresponding to {m_str}queued message with key {out_msg_id:x}",
            )
        };
        let correct = match out_msg {
            OutMsg::New(_) | OutMsg::Transit(_) | OutMsg::DeferredTransit(_) => !dequeue,
            OutMsg::Dequeue(_) | OutMsg::DequeueImmediate(_) | OutMsg::DequeueShort(_) => dequeue,
            OutMsg::TransitRequeued(_) => true,
            _ => false,
        };
        if !correct {
            reject_query!(
                "OutMsgDescr corresponding to {m_str}queued message with key {:x} \
                has invalid tag ${:05b}",
                out_msg_id,
                out_msg.tag()
            )
        }
        if dequeue {
            // dequeued message
            if let OutMsg::TransitRequeued(info) = out_msg {
                // this is a msg_export_tr_req$111, a re-queued transit message (after merge)
                // check that q_msg_env still contains msg
                let q_msg = info.out_message_cell();
                if *info.out_message_cell().repr_hash() != out_msg_id.hash {
                    reject_query!(
                        "MsgEnvelope in the old outbound queue with key {:x} \
                        contains a Message with incorrect hash {:x}",
                        out_msg_id,
                        q_msg.repr_hash()
                    )
                }
                // must be msg_import_tr$100
                let in_msg = info.read_imported()?;
                match in_msg {
                    InMsg::Immediate(info) => {
                        if info.envelope_message_hash() != enq.envelope_hash() {
                            reject_query!(
                                "OutMsgDescr corresponding to dequeued message with key {:x} \
                                is a msg_export_tr_req referring to a reimport InMsgDescr that \
                                contains a MsgEnvelope distinct from that \
                                originally kept in the old queue",
                                out_msg_id
                            )
                        }
                    }
                    _ => reject_query!("OutMsgDescr for {:x} refers to a reimport InMsgDescr with invalid tag ${:05b} \
                        instead of msg_import_tr$100", out_msg_id, in_msg.tag())
                }
            } else if out_msg.envelope_message_hash() != Some(enq.envelope_hash()) {
                reject_query!(
                    "OutMsgDescr corresponding to dequeued message with key {:x} contains a \
                    MsgEnvelope distinct from that originally kept in the old queue",
                    out_msg_id
                )
            }
        } else {
            if out_msg.envelope_message_hash() != Some(enq.envelope_hash()) {
                reject_query!(
                    "OutMsgDescr corresponding to enqueued message with key {:x} \
                    contains a MsgEnvelope distinct from that stored in the new queue",
                    out_msg_id
                )
            }
        };
        if enq.message_hash() != out_msg_id.hash {
            reject_query!(
                "OutMsgDescr for {:x} contains a message with different hash {:x}",
                out_msg_id.hash,
                enq.message_hash()
            )
        }
        // in all cases above, we have to check that all 352-bit key is correct (including first 96 bits)
        // otherwise we might not be able to correctly recover OutMsgQueue entries starting from OutMsgDescr later
        // or we might have several OutMsgQueue entries with different 352-bit keys all having the same last 256 bits (with the message hash)
        let new_key = enq.out_msg_key();
        if &new_key != out_msg_id {
            reject_query!(
                "OutMsgDescr for {:x} contains a MsgEnvelope that should be stored \
                under different key {:x}",
                out_msg_id,
                new_key
            )
        }
        Ok(())
    }

    fn precheck_message_queue_update(
        base: Arc<ValidateBase>,
        manager: &MsgQueueManager,
    ) -> Result<()> {
        log::debug!(target: "validate_query", "({}): pre-checking the difference between the \
            old and the new outbound message queues", base.next_block_descr);
        let prev_out_queue = manager.prev().out_queue();
        let next_out_queue = manager.next().out_queue();
        let old_out_msg_queue_size = manager.prev().out_queue_extra().out_queue_size();
        let mut expected_out_msg_queue_size = old_out_msg_queue_size;
        let result = prev_out_queue.scan_diff_with_aug(next_out_queue, |key, val1, val2| {
            Self::precheck_one_message_queue_update(
                &base,
                &mut expected_out_msg_queue_size,
                &key,
                val1,
                val2,
            )?;
            Ok(true)
        });
        if let Err(err) = result {
            reject_query!(
                "invalid OutMsgQueue dictionary difference between the old and the new state: {:?}",
                err
            )
        }
        let new_out_msg_queue_size = manager.next().out_queue_extra().out_queue_size();
        log::info!(
            target: "validate_query",
            "({}): outbound message queue size: {} -> new size: {}, expected: {}",
            base.next_block_descr,
            old_out_msg_queue_size,
            new_out_msg_queue_size,
            expected_out_msg_queue_size
        );
        if new_out_msg_queue_size != expected_out_msg_queue_size {
            reject_query!(
                "outbound message queue size in the new state is not correct (expected: {}, found: {})",
                expected_out_msg_queue_size,
                new_out_msg_queue_size,
            );
        }
        Ok(())
    }

    ///
    /// Performs a check on the difference between the old and new dispatch queues for one account.
    ///
    /// @param addr The 256-bit address of the account.
    /// @param old_val_extra The old value of the account dispatch queue.
    /// @param new_val_extra The new value of the account dispatch queue.
    ///
    /// @returns reject or error, Ok(true) on success.
    ///
    fn check_account_dispatch_queue_update(
        base: &ValidateBase,
        addr: AccountId,
        old_queue: AccountDispatchQueue,
        new_queue: AccountDispatchQueue,
        processed_account_dispatch_queues: &mut usize,
    ) -> Result<bool> {
        let mut expected_size = old_queue.len();
        let mut max_removed_lt = 0;
        let mut min_added_lt = u64::MAX;
        old_queue.messages().scan_diff(&new_queue.messages(), |lt: u64, old_msg, new_msg| {
            let (enq, old) = match (old_msg, new_msg) {
                (Some(_), Some(_)) => {
                    reject_query!("invalid AccountDispatchQueue diff for account {addr:x}")
                }
                (Some(enq), None) => {
                    log::debug!(target: "validate_query", "({}): removed message from DispatchQueue: \
                        account={addr:x}, lt={lt}", base.next_block_descr);
                    expected_size -= 1;
                    (enq, true)
                }
                (None, Some(enq)) => {
                    log::debug!(target: "validate_query", "({}): added message to DispatchQueue: \
                        account={addr:x}, lt={lt}", base.next_block_descr);
                    expected_size += 1;
                    if base.shard().is_masterchain() && base.is_special_smartcontract(&addr)? {
                        reject_query!("cannot defer message from a special account -1:{:x}", addr)
                    }
                    (enq, false)
                }
                (None, None) => return Ok(true) // no change - unreachable
            };
            if lt != enq.enqueued_lt() {
                reject_query!(
                    "invalid EnqueuedMsg in AccountDispatchQueue for {addr:x}: \
                    lt mismatch ({lt} != {})",
                    enq.enqueued_lt(),
                )
            }
            let env = enq.read_envelope_msg()?;
            if env.emitted_lt() != 0 {
                reject_query!(
                    "invalid EnqueuedMsg in AccountDispatchQueue for {addr:x}, lt={lt}: \
                    unexpected emitted_lt"
                )
            }
            let msg = env.read_message()?;
            let created_lt = msg.created_lt().unwrap_or_default();
            if lt != created_lt {
                reject_query!(
                    "invalid EnqueuedMsg in AccountDispatchQueue for {addr:x}: \
                    lt mismatch ({lt} != {created_lt})"
                )
            }
            if old {
                base.result.removed_dispatch_queue_messages.insert((addr.clone(), lt), enq.envelope_cell());
                max_removed_lt = max_removed_lt.max(lt);
            } else {
                base.result.new_dispatch_queue_messages.insert((addr.clone(), lt), enq.envelope_cell());
                min_added_lt = min_added_lt.min(lt);
            }
            Ok(true)
        })?;
        if expected_size != new_queue.len() {
            reject_query!(
                "invalid count in AccountDispatchQuery for {addr:x}: \
                expected={expected_size}, found={}",
                new_queue.len()
            )
        }
        if let Some((key, _)) = new_queue.messages().find_min_max_raw(true, false)? {
            let new_min_lt = u64::construct_from_bitstring(key)?;
            if new_min_lt <= max_removed_lt {
                reject_query!(
                    "invalid AccountDispatchQuery update for {addr:x}: max removed lt is {max_removed_lt}, \
                    but lt={new_min_lt} is still in queue"
                )
            }
        }
        if let Some((key, _)) = old_queue.messages().find_min_max_raw(false, false)? {
            let old_max_lt = u64::construct_from_bitstring(key)?;
            if old_max_lt >= min_added_lt {
                reject_query!(
                    "invalid AccountDispatchQuery update for {addr:x}: min added lt is {min_added_lt}, \
                    but lt={old_max_lt} was present in the queue"
                )
            }
            if max_removed_lt != old_max_lt {
                log::trace!(
                    target: "validate_query",
                   "Some old messages are still in DispatchQueue for {addr:x}, \
                   meaning that all new messages from this account must be deferred \
                   {max_removed_lt} != {old_max_lt}"
                );
                let _ = base.result.account_expected_defer_all_messages.insert(addr);
            }
        }
        if !old_queue.is_empty() && max_removed_lt != 0 {
            *processed_account_dispatch_queues += 1;
        }
        Ok(true)
    }

    ///
    /// Pre-check the difference between the old and new dispatch queues and put the difference to
    /// new_dispatch_queue_messages, old_dispatch_queue_messages
    ///
    /// @returns reject or error
    ///
    fn unpack_dispatch_queue_update(
        base: Arc<ValidateBase>,
        manager: &MsgQueueManager,
    ) -> Result<()> {
        log::info!(target: "validate_query", "checking the difference between the old and the new dispatch queues");
        let prev = manager.prev().out_queue_extra();
        let next = manager.next().out_queue_extra();
        let mut processed_account_dispatch_queues = 0;
        prev.dispatch_queue().scan_diff_with_default(
            next.dispatch_queue(),
            |addr, old_queue, new_queue| {
                Self::check_account_dispatch_queue_update(
                    &base,
                    addr,
                    old_queue,
                    new_queue,
                    &mut processed_account_dispatch_queues,
                )
            },
        )?;
        if prev.out_queue_size <= base.limits.defer_out_queue_size_limit as usize {
            // Check that at least one message was taken from each AccountDispatchQueue
            let mut total_account_dispatch_queues = 0;
            prev.dispatch_queue().iterate_slices(|_, _| {
                total_account_dispatch_queues += 1;
                Ok(total_account_dispatch_queues <= processed_account_dispatch_queues)
            })?;
            if total_account_dispatch_queues != processed_account_dispatch_queues {
                base.result.have_unprocessed_account_dispatch_queue.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn update_max_processed_lt_hash(base: &ValidateBase, lt: u64, hash: &UInt256) -> Result<bool> {
        add_unbound_object_to_map_with_update(&base.result.lt_hash, 0, |lt_hash| {
            if let Some((proc_lt, proc_hash)) = lt_hash {
                if !(proc_lt < &lt || (&lt == proc_lt && proc_hash < hash)) {
                    return Ok(None);
                }
            }
            Ok(Some((lt, hash.clone())))
        })
    }

    fn update_min_enqueued_lt_hash(base: &ValidateBase, lt: u64, hash: &UInt256) -> Result<bool> {
        add_unbound_object_to_map_with_update(&base.result.lt_hash, 1, |lt_hash| {
            if let Some((min_enq_lt, min_enq_hash)) = lt_hash {
                if !(&lt < min_enq_lt || (&lt == min_enq_lt && hash < min_enq_hash)) {
                    return Ok(None);
                }
            }
            Ok(Some((lt, hash.clone())))
        })
    }

    // check that the enveloped message (MsgEnvelope) was present in the output queue of a neighbor, and that it has not been processed before
    fn check_imported_message(
        base: &ValidateBase,
        manager: &MsgQueueManager,
        env: &MsgEnvelope,
        env_hash: &UInt256,
        created_lt: u64,
    ) -> Result<()> {
        let (cur_prefix, next_prefix) = env.calc_cur_next_prefix()?;
        if !base.shard().contains_full_prefix(&next_prefix) {
            reject_query!(
                "imported message with hash {env_hash:x} has next hop address {next_prefix}... not in this shard"
            )
        }
        let key = OutMsgQueueKey::with_account_prefix(
            &next_prefix,
            env.message_cell().repr_hash().clone(),
        );
        if let (Some(block_id), enq) = manager.find_message(&key, &cur_prefix)? {
            let Some(enq) = enq else {
                reject_query!(
                    "imported internal message with hash {env_hash:x} and previous address \
                    {cur_prefix}..., next hop address {next_prefix} could not be found in \
                    the outbound message queue of neighbor {block_id} under key {key:x}"
                )
            };
            if &enq.envelope_hash() != env_hash {
                reject_query!(
                    "imported internal message from the outbound message queue of neighbor \
                    {block_id} under key {key:x} has a different MsgEnvelope in that outbound \
                    message queue",
                )
            }
            if manager.prev().already_processed(&enq)? {
                reject_query!(
                    "imported internal message with hash {env_hash:x} and lt={created_lt} has \
                    been already imported by a previous block of this shardchain",
                )
            }
            Self::update_max_processed_lt_hash(base, created_lt, &key.hash)?;
            Ok(())
        } else {
            reject_query!(
                "imported internal message with hash {env_hash:x} and previous address \
                {cur_prefix}..., next hop address {next_prefix} has previous address not \
                belonging to any neighbor"
            )
        }
    }

    ///
    /// Checks the validity of an inbound message listed in InMsgDescr.
    ///
    /// @param key The 256-bit key of the inbound message.
    /// @param in_msg The inbound message to be checked serialized using InMsg TLB-scheme.
    ///
    /// @returns True if the inbound message is valid, false otherwise.
    ///
    fn check_in_msg(
        base: &ValidateBase,
        manager: &MsgQueueManager,
        key: &UInt256,
        in_msg: &InMsg,
    ) -> Result<()> {
        log::debug!(target: "validate_query", "({}): checking InMsg with key {key:x}", base.next_block_descr);
        CHECK!(in_msg, inited);
        // initial checks and unpack
        let msg_hash = in_msg.message_cell()?.repr_hash().clone();
        if msg_hash != *key {
            reject_query!(
                "InMsg with key {key:x} refers to a message with different hash {msg_hash:x}"
            )
        }
        let trans_cell = in_msg.transaction_cell();
        let msg_env_cell = in_msg.in_msg_envelope_cell().unwrap_or_default();
        let msg_env_hash = msg_env_cell.repr_hash().clone();
        let env = in_msg.read_in_msg_envelope()?.unwrap_or_default();
        let msg = in_msg.read_message()?;
        let created_lt = msg.created_lt().unwrap_or_default();
        let Some(dst) = msg.dst_ref() else {
            reject_query!("InMsg with key {key:x} refers to a message with no destination address")
        };
        let (workchain_id, addr) = dst.extract_std_address(true).map_err(|err| {
            error!(
                "destination {dst} of inbound internal message with hash {key:x} \
                is an invalid blockchain address {err}"
            )
        })?;
        if let Some(trans_cell) = trans_cell.clone() {
            let transaction = Transaction::construct_from_cell(trans_cell.clone())?;
            // check that the transaction reference is valid, and that
            // it points to a Transaction which indeed processes this input message
            Self::is_valid_transaction_ref(base, &transaction, trans_cell.repr_hash().clone())
                .map_err(|err| {
                    error!(
                        "InMsg corresponding to inbound message with key {key:x} contains \
                        an invalid Transaction reference \
                        (transaction not in the block's transaction list) : {err}"
                    )
                })?;
            if let Some(tr_msg_cell) = transaction.in_msg_cell() {
                if *tr_msg_cell.repr_hash() != msg_hash {
                    reject_query!(
                        "InMsg corresponding to inbound message with key {key:x} \
                        refers to transaction that does not process this inbound message"
                    )
                }
            }
            if &addr != transaction.account_id() {
                reject_query!(
                    "InMsg corresponding to inbound message with hash {key:x} and \
                    destination address {addr:x} claims that the message is processed by \
                    transaction {} of another account {:x}",
                    transaction.logical_time(),
                    transaction.account_id()
                )
            }
        }
        let mut from_dispatch_queue = false;
        let mut fwd_fee = Coins::zero();
        match in_msg {
            // msg_import_ext$000 msg:^(Message Any) transaction:^Transaction
            // importing an inbound external message
            InMsg::External(_) => {
                let dest_prefix = AccountIdPrefixFull::std_prefix(workchain_id, &addr)?;
                if !base.shard().contains_full_prefix(&dest_prefix) {
                    reject_query!(
                        "inbound external message with hash {key:x} has destination address \
                        {dest_prefix}... not in this shard",
                    )
                }
                return Ok(()); // nothing to check more
            }
            // msg_import_imm$011 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins
            // importing and processing an internal message generated in this very block
            InMsg::Immediate(info) => {
                let emitted_lt = if env.emitted_lt() == 0 { created_lt } else { env.emitted_lt() };
                if !base.is_special_in_msg(in_msg) {
                    Self::update_max_processed_lt_hash(base, emitted_lt, key)?;
                }
                fwd_fee = info.fwd_fee;
            }
            // msg_import_fin$100 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins
            // importing and processing an internal message with destination in this shard
            InMsg::Final(info) => {
                fwd_fee = info.fwd_fee;
                base.total_imported_msgs.fetch_add(1, Ordering::Relaxed);
            }
            // msg_import_tr$101 in_msg:^MsgEnvelope out_msg:^MsgEnvelope transit_fee:Coins
            // importing and relaying a (transit) internal message with destination outside this shard
            InMsg::Transit(info) => {
                fwd_fee = info.transit_fee;
                base.total_imported_msgs.fetch_add(1, Ordering::Relaxed);
            }
            // msg_import_ihr$010 msg:^(Message Any) transaction:^Transaction ihr_fee:Coins proof_created:^Cell
            InMsg::IHR(_) => reject_query!(
                "InMsg with key {:x} \
                is a msg_import_ihr, but IHR messages are not enabled in this version",
                key
            ),
            // msg_discard_tr$111 in_msg:^MsgEnvelope transaction_id:uint64 fwd_fee:Coins proof_delivered:^Cell
            InMsg::DiscardedTransit(_) => reject_query!(
                "InMsg with key {:x} \
                is a msg_discard_tr, but IHR messages are not enabled in this version",
                key
            ),
            // msg_discard_fin$110 in_msg:^MsgEnvelope transaction_id:uint64 fwd_fee:Coins
            InMsg::DiscardedFinal(_) => reject_query!(
                "InMsg with key {:x} \
                is a msg_discard_fin, but IHR messages are not enabled in this version",
                key
            ),
            InMsg::DeferredFinal(info) => {
                from_dispatch_queue = true;
                fwd_fee = info.fwd_fee;
            }
            InMsg::DeferredTransit(_) => {
                from_dispatch_queue = true;
            }
            _ => reject_query!("InMsg with key {:x} has impossible tag", key),
        };
        if !from_dispatch_queue && base.have_unprocessed_account_dispatch_queue() {
            // Collator is requeired to take at least one message from each AccountDispatchQueue
            // (unless the block is full or unless out_msg_queue_size is big)
            // If some AccountDispatchQueue is unporcessed then it's not allowed to import other messages except for externals
            reject_query!(
                "required DispatchQueue processing is not done, \
                but some other internal messages are imported"
            )
        }
        // common checks for all (non-external) inbound messages
        // unpack int_msg_info$0 ... = CommonMsgInfo, especially message addresses
        let Some(header) = msg.int_header() else {
            reject_query!(
                "InMsg with key {key:x} is not a msg_import_ext$000, \
                but it does not refer to an inbound internal message"
            )
        };
        // extract source, current, next hop and destination address prefixes
        let Some(src) = header.src_ref() else {
            reject_query!(
                "source of inbound internal message with hash {key:x} \
                is an invalid blockchain address"
            )
        };
        let src_prefix = AccountIdPrefixFull::checked_prefix(src)?;
        let dest_prefix = AccountIdPrefixFull::checked_prefix(&header.dst)?;
        let cur_prefix = src_prefix.interpolate_addr_intermediate(&dest_prefix, env.cur_addr())?;
        let next_prefix =
            src_prefix.interpolate_addr_intermediate(&dest_prefix, env.next_addr())?;
        // check that next hop is nearer to the destination than the current address
        if count_matching_bits(&dest_prefix, &next_prefix)
            < count_matching_bits(&dest_prefix, &cur_prefix)
        {
            reject_query!(
                "next hop address {next_prefix}... of inbound internal message with hash {key:x} \
                is further from its destination {dest_prefix}... than its current address {cur_prefix}..."
            )
        }
        // next hop address must belong to this shard (otherwise we should never had imported this message)
        if !base.shard().contains_full_prefix(&next_prefix) {
            reject_query!(
                "next hop address {next_prefix}... of inbound internal message with hash {key:x} \
                does not belong to the current block's shard {}",
                base.shard()
            )
        }
        // next hop may coincide with current address only if destination is already reached
        if !from_dispatch_queue && next_prefix == cur_prefix && cur_prefix != dest_prefix {
            reject_query!(
                "next hop address {next_prefix}... of inbound internal message with hash {key:x} \
                coincides with its current address, but this message \
                has not reached its final destination {dest_prefix}... yet"
            )
        }
        if from_dispatch_queue && next_prefix != cur_prefix {
            reject_query!(
                "next hop address {next_prefix}... of deferred internal message with hash {key:x} \
                must coincide with its current prefix {cur_prefix}...",
            );
        }
        // if a message is processed by a transaction, it must have destination inside the current shard
        if trans_cell.is_some() && !base.shard().contains_full_prefix(&dest_prefix) {
            reject_query!(
                "inbound internal message with hash {key:x} has destination address \
                {dest_prefix}... not in this shard, but it is processed nonetheless",
            )
        }
        // if a message is not processed by a transaction, its final destination must be outside this shard,
        // or it is a deferred message (dispatch queue -> out msg queue)
        if trans_cell.is_none()
            && base.shard().contains_full_prefix(&dest_prefix)
            && !matches!(in_msg, InMsg::DeferredTransit(_))
        {
            reject_query!(
                "inbound internal message with hash {key:x} has destination address \
                {dest_prefix}... in this shard, but it is not processed by a transaction"
            )
        }
        // unpack original forwarding fee
        let orig_fwd_fee = &header.fwd_fee;
        // CHECK!(orig_fwd_fee.is_some());
        if env.fwd_fee_remaining() > orig_fwd_fee {
            reject_query!(
                "inbound internal message with hash {key:x} has remaining forwarding fee {} \
                larger than the original (total) forwarding fee {orig_fwd_fee}",
                env.fwd_fee_remaining(),
            )
        }

        if from_dispatch_queue {
            let (_, addr) = src.extract_std_address(true).map_err(|err| {
                error!(
                    "source {src} of deferred inbound message with hash {key:x} \
                    is an invalid blockchain address {err}"
                )
            })?;
            // Check that the message was removed from DispatchQueue
            let Some(dispatched_msg_env_cell) =
                base.removed_dispatch_queue_messages(&addr, created_lt)
            else {
                reject_query!(
                    "deferred InMsg with src_addr={addr:x} lt={created_lt} was not removed from \
                    the dispatch queue"
                )
            };
            let emitted_lt = env.emitted_lt();
            if emitted_lt == 0 {
                reject_query!(
                    "no dispatch_lt in deferred InMsg with src_addr={addr:x}, lt={created_lt}"
                )
            }
            if emitted_lt < base.info.start_lt() || emitted_lt > base.info.end_lt() {
                reject_query!(
                    "dispatch_lt in deferred InMsg with src_addr={addr:x}, lt={created_lt} \
                    is not between start and end of the block"
                )
            }
            let mut env2 = env.clone();
            env2.clear_emitted_lt();
            let cell = env2.serialize()?;
            if cell != dispatched_msg_env_cell {
                reject_query!(
                    "deferred InMsg with src_addr={addr:x}, lt={created_lt} msg envelope \
                    hash mismatch: {:x} in DispatchQueue, {:x} expected",
                    dispatched_msg_env_cell.repr_hash(),
                    cell.repr_hash()
                )
            }
            if matches!(in_msg, InMsg::DeferredFinal(_)) {
                base.result.msg_emitted_lt.push((addr, created_lt, emitted_lt));
            }
        }

        let out_msg_opt = base.out_msg_descr.get(key)?;
        let (out_msg_env, reimport) = match out_msg_opt.as_ref() {
            Some(out_msg) => (out_msg.msg_envelope_cell(), out_msg.read_reimport_message()?),
            None => (None, None),
        };
        let mut tr_req = "";
        let mut from_dispatch_queue = false;

        // continue checking inbound message
        match in_msg {
            InMsg::Immediate(_) => {
                // msg_import_imm$011 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins
                // importing and processing an internal message generated in this very block
                if cur_prefix != dest_prefix {
                    reject_query!(
                        "inbound internal message with hash {key:x} is a msg_import_imm$011, but \
                        its current address {cur_prefix} is somehow distinct from its final \
                        destination {dest_prefix}"
                    )
                }
                CHECK!(trans_cell.is_some());
                // check that the message has been created in this very block
                if !base.shard().contains_full_prefix(&src_prefix) {
                    reject_query!(
                        "inbound internal message with hash {:x} is a msg_import_imm$011, \
                        but its source address {} does not belong to this shard",
                        key,
                        src_prefix
                    )
                }
                if let Some(OutMsg::Immediate(_)) = out_msg_opt.as_ref() {
                    CHECK!(out_msg_env.is_some());
                    CHECK!(reimport.is_some());
                } else if !base.is_special_in_msg(in_msg) {
                    reject_query!(
                        "inbound internal message with hash {:x} is a msg_import_imm$011, but the \
                        corresponding OutMsg does not exist, or is not a valid msg_export_imm$010",
                        key
                    )
                }
                // fwd_fee must be equal to the fwd_fee_remaining of this MsgEnvelope
                if &fwd_fee != env.fwd_fee_remaining() {
                    reject_query!(
                        "msg_import_imm$011 InMsg with hash {:x} is invalid because its collected \
                        fwd_fee={} is not equal to fwd_fee_remaining={} of this message (envelope)",
                        key,
                        fwd_fee,
                        env.fwd_fee_remaining()
                    )
                }
                // ...
            }
            InMsg::Final(info) => {
                // msg_import_fin$100 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins
                // importing and processing an internal message with destination in this shard
                CHECK!(trans_cell.is_some());
                CHECK!(base.shard().contains_full_prefix(&next_prefix));
                if base.shard().contains_full_prefix(&cur_prefix) {
                    // we imported this message from our shard!
                    if let Some(OutMsg::DequeueImmediate(_)) = out_msg_opt.as_ref() {
                        CHECK!(out_msg_env.is_some());
                        CHECK!(reimport.is_some());
                    } else {
                        reject_query!(
                            "inbound internal message with hash {:x} is a msg_import_fin$100 \
                            with current address {}... already in our shard, but the corresponding \
                            OutMsg does not exist, or is not a valid msg_export_deq_imm$100",
                            key,
                            cur_prefix
                        )
                    }
                } else {
                    CHECK!(cur_prefix != next_prefix);
                    // check that the message was present in the output queue of a neighbor, and that it has not been processed before
                    Self::check_imported_message(
                        base,
                        manager,
                        &env,
                        &info.envelope_message_hash(),
                        created_lt,
                    )?;
                }
                // ...
                // fwd_fee must be equal to the fwd_fee_remaining of this MsgEnvelope
                if &fwd_fee != env.fwd_fee_remaining() {
                    reject_query!(
                        "msg_import_fin$100 InMsg with hash {:x} is invalid because \
                        its collected fwd_fee={} is not equal to fwd_fee_remaining={} of \
                        this message (envelope)",
                        key,
                        fwd_fee,
                        env.fwd_fee_remaining()
                    )
                }
                // ...
            }
            InMsg::DeferredFinal(info) => {
                from_dispatch_queue = true;
                // fwd_fee must be equal to the fwd_fee_remaining of this MsgEnvelope
                if info.fwd_fee() != env.fwd_fee_remaining() {
                    reject_query!(
                        "msg_import_imm$10100 InMsg with hash {key:x} is invalid because \
                        its collected fwd_fee={} is not equal to \
                        fwd_fee_remaining={} of this message (envelope)",
                        info.fwd_fee(),
                        env.fwd_fee_remaining(),
                    )
                }
            }
            InMsg::Transit(_) => {
                // msg_import_tr$101 in_msg:^MsgEnvelope out_msg:^MsgEnvelope transit_fee:Coins
                // importing and relaying a (transit) internal message with destination outside this shard
                if cur_prefix == dest_prefix {
                    reject_query!(
                        "inbound internal message with hash {:x} is a msg_import_tr$101 \
                        (a transit message), but its current address {}... is already equal to \
                        its final destination",
                        key,
                        cur_prefix
                    )
                }
                let Some(out_msg) = &out_msg_opt else {
                    reject_query!(
                        "inbound internal message with hash {key:x} is a msg_import_tr$101 \
                        (transit message), but the corresponding OutMsg does not exist",
                    )
                };
                tr_req = if base.shard().contains_full_prefix(&cur_prefix) {
                    // we imported this message from our shard!
                    // (very rare situation possible only after merge)
                    if !matches!(out_msg, OutMsg::TransitRequeued(_)) {
                        reject_query!(
                            "inbound internal message with hash {key:x} is a msg_import_tr$101 \
                            (transit message) with current address {cur_prefix}... already in our shard, \
                            but the corresponding OutMsg is not a valid msg_export_tr_req$111"
                        )
                    }
                    "requeued"
                } else {
                    if !matches!(out_msg, OutMsg::Transit(_)) {
                        reject_query!(
                            "inbound internal message with hash {key:x} is a msg_import_tr$101 \
                            (transit message) with current address {cur_prefix}... outside of our shard, \
                            but the corresponding OutMsg is not a valid msg_export_tr$011"
                        )
                    }
                    // check that the message was present in the output queue of a neighbor, and that it has not been processed before
                    Self::check_imported_message(base, manager, &env, &msg_env_hash, created_lt)?;
                    "usual"
                };
                CHECK!(out_msg_env.is_some());
                CHECK!(reimport.is_some());
            }
            InMsg::DeferredTransit(_) => {
                from_dispatch_queue = true;
                // msg_import_deferred_tr$10101 in_msg:^MsgEnvelope out_msg:^MsgEnvelope
                // importing and relaying a (transit) internal message with destination outside this shard
                if out_msg_opt.is_none() {
                    reject_query!(
                        "inbound internal message with hash {key:x} \
                        is a msg_import_deferred_tr$00101 with current address {cur_prefix}... \
                        outside of our shard, but the corresponding OutMsg \
                        is not a valid msg_export_deferred_tr$10101"
                    )
                };
            }
            _ => reject_query!("Forbiden InMsg type : {in_msg:?}"),
        }
        if from_dispatch_queue && cur_prefix != next_prefix {
            reject_query!(
                "next hop address {next_prefix}... of deferred internal message with hash \
                {key:x} must coincide with its current prefix {cur_prefix}..."
            )
        }
        if let Some(tr_env_cell) = in_msg.out_msg_envelope_cell() {
            CHECK!(trans_cell.is_none());
            // perform hypercube routing for this transit message
            let route_info =
                perform_hypercube_routing(&next_prefix, &dest_prefix, base.shard(), true).map_err(
                    |err| {
                        error!(
                            "cannot perform (check) hypercube routing for transit inbound message \
                            with hash {key:x}: src={src_prefix} cur={cur_prefix} \
                            next={next_prefix} dest={dest_prefix}; our shard is {} : {err}",
                            base.shard(),
                        )
                    },
                )?;
            let new_cur_prefix =
                next_prefix.interpolate_addr_intermediate(&dest_prefix, &route_info.0)?;
            let new_next_prefix =
                next_prefix.interpolate_addr_intermediate(&dest_prefix, &route_info.1)?;
            // unpack out_msg:^MsgEnvelope from msg_import_tr
            let tr_env = MsgEnvelope::construct_from_cell(tr_env_cell.clone()).map_err(|err| {
                error!(
                    "InMsg for transit message with hash {key:x} refers to \
                    an invalid rewritten message envelope {err:?}"
                )
            })?;
            // the rewritten transit message envelope must contain the same message
            if tr_env.message_cell().repr_hash() != key {
                reject_query!(
                    "InMsg for transit message with hash {:x} refers to a rewritten message \
                    envelope containing another message",
                    key
                )
            }
            // check that the message has been routed according to hypercube routing
            let tr_cur_prefix =
                src_prefix.interpolate_addr_intermediate(&dest_prefix, tr_env.cur_addr())?;
            let tr_next_prefix =
                src_prefix.interpolate_addr_intermediate(&dest_prefix, tr_env.next_addr())?;
            if tr_cur_prefix != new_cur_prefix || tr_next_prefix != new_next_prefix {
                reject_query!(
                    "InMsg for transit message with hash {:x} tells us that it has been \
                    adjusted to current address {}... and hext hop address {} while the \
                    correct values dictated by hypercube routing are {}... and {}...",
                    key,
                    tr_cur_prefix,
                    tr_next_prefix,
                    new_cur_prefix,
                    new_next_prefix
                )
            }
            // check that the collected transit fee with new fwd_fee_remaining equal the original fwd_fee_remaining
            // (correctness of fwd_fee itself will be checked later)
            let mut fee = *tr_env.fwd_fee_remaining();
            fee.add(&fwd_fee)?;
            if tr_env.fwd_fee_remaining() > orig_fwd_fee || &fee != env.fwd_fee_remaining() {
                reject_query!(
                    "InMsg for transit message with hash {key:x} declares transit fees of {fwd_fee}, \
                    but fwd_fees_remaining has decreased from {} to {} in transit",
                    env.fwd_fee_remaining(),
                    tr_env.fwd_fee_remaining()
                )
            }
            if tr_env.metadata() != env.metadata() {
                reject_query!(
                    "InMsg for transit message with hash {key:x} contains invalid MsgMetadata: \
                    {:?} in in_msg, but {:?} in out_msg",
                    tr_env.metadata(),
                    env.metadata(),
                )
            }
            if tr_env.emitted_lt() != env.emitted_lt() {
                reject_query!(
                    "InMsg for transit message with hash {key:x} contains \
                    invalid emitted_lt: {} in in_msg, but {} in out_msg",
                    tr_env.emitted_lt(),
                    env.emitted_lt(),
                )
            }
            if Some(tr_env_cell) != out_msg_env {
                reject_query!(
                    "InMsg for transit message with hash {key:x} contains rewritten MsgEnvelope \
                    different from that stored in corresponding OutMsgDescr ({tr_req} transit)"
                )
            }
            // check the amount of the transit fee
            let transit_fee = if from_dispatch_queue {
                Coins::zero()
            } else {
                base.config_params.fwd_prices(false)?.next_fee_checked(env.fwd_fee_remaining())?
            };
            if transit_fee != fwd_fee {
                reject_query!(
                    "InMsg for transit message with hash {key:x} declared collected transit fees \
                    to be {fwd_fee} (deducted from the remaining forwarding fees of {}), but \
                    we have computed another value of transit fees {transit_fee}",
                    env.fwd_fee_remaining(),
                )
            }
        }

        if let Some(reimport) = reimport {
            // transit message: msg_export_tr + msg_import_tr
            // or message re-imported from this very shard
            // either msg_export_imm + msg_import_imm
            // or msg_export_deq_imm + msg_import_fin
            // or msg_export_tr_req + msg_import_tr (rarely, only after merge)
            // must have a corresponding OutMsg record
            if in_msg != &reimport {
                reject_query!(
                    "OutMsg corresponding to reimport InMsg with hash {:x} \
                    refers to a different reimport InMsg",
                    key
                )
            }
            // for transit messages, OutMsg refers to the newly-created outbound messages (not to the re-imported old outbound message)
            match in_msg {
                InMsg::Transit(_) | InMsg::DeferredTransit(_) => (),
                _ => {
                    if out_msg_env != Some(msg_env_cell) {
                        reject_query!(
                            "InMsg with hash {key:x} is a reimport record, but the \
                            corresponding OutMsg exports a MsgEnvelope with a different hash"
                        )
                    }
                }
            }
        }
        Ok(())
    }

    fn check_in_msg_descr(
        base: Arc<ValidateBase>,
        manager: Arc<MsgQueueManager>,
        tasks: &mut TasksVec,
    ) -> Result<()> {
        // log::debug!(target: "validate_query", "({}): checking inbound messages listed in InMsgDescr", base.next_block_descr);
        let result = base.in_msg_descr.iterate_with_keys(|key, in_msg| {
            let base = base.clone();
            let manager = manager.clone();
            Self::add_task(tasks, move || {
                Self::check_in_msg(&base, &manager, &key, &in_msg).map_err(|err| {
                    error!(
                        "invalid InMsg with key (message hash) {:x} in the new block {} : {} {:?}",
                        key,
                        base.block_id(),
                        err,
                        in_msg
                    )
                })
            });
            Ok(true)
        });
        if let Err(err) = result {
            reject_query!(
                "invalid InMsgDescr dictionary in the new block {} : {}",
                base.block_id(),
                err
            )
        }
        Ok(())
    }

    fn check_reimport(
        base: &ValidateBase,
        out_msg: &OutMsg,
        in_msg_key: &UInt256,
    ) -> Result<Option<InMsg>> {
        if let Some(reimport_cell) = out_msg.reimport_cell() {
            // transit message: msg_export_tr + msg_import_tr
            // or message re-imported from this very shard
            // either msg_export_imm + msg_import_imm
            // or msg_export_deq_imm + msg_import_fin (rarely)
            // or msg_export_tr_req + msg_import_tr (rarely)
            // (the last two cases possible only after merge)
            //
            // check that reimport is a valid InMsg registered in InMsgDescr
            let Some(in_msg_slice) = base.in_msg_descr.get_as_slice(in_msg_key)? else {
                reject_query!(
                    "OutMsg with key {in_msg_key:x} refers to a (re)import InMsg, \
                    but there is no InMsg with such a key",
                )
            };
            let mut reimport_slice = SliceData::load_cell(reimport_cell)?;
            if in_msg_slice != reimport_slice {
                reject_query!(
                    "OutMsg with key {in_msg_key:x} refers to a (re)import InMsg, \
                    but the actual InMsg with this key is different from the one referred to"
                )
            }
            Ok(Some(InMsg::construct_from(&mut reimport_slice)?))
        } else {
            Ok(None)
        }
    }

    ///
    /// Checks the validity of an outbound message listed in OutMsgDescr.
    ///
    /// # Arguments
    ///
    /// * `base` - The base structure containing validation context and limits.
    /// * `key` - The 256-bit key of the outbound message.
    /// * `out_msg` - The outbound message to be checked serialized using OutMsg TLB-scheme.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the outbound message is valid, or an error if it is not.
    ///
    fn check_out_msg(
        base: &ValidateBase,
        manager: &MsgQueueManager,
        key: &UInt256,
        out_msg: &OutMsg,
    ) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): checking OutMsg with key {key:x} has tag ${:05b}",
            base.next_block_descr,
            out_msg.tag()
        );
        // initial checks and unpack
        CHECK!(out_msg, inited);
        // will be get from real message later
        let mut created_lt = 0;
        let trans_cell = out_msg.transaction_cell();
        // it will be used correctly by checking type of OutMsg
        let env = out_msg.read_msg_envelope()?.unwrap_or_default();
        let msg_cell = out_msg.message_cell()?.unwrap_or_default();
        let mut src_wc = INVALID_WORKCHAIN_ID;
        let mut src_addr = AccountId::ZERO_ID;
        // will be get from env message or deq short later
        let mut src_prefix = AccountIdPrefixFull::default();
        let mut dest_prefix = AccountIdPrefixFull::default();
        let mut cur_prefix = AccountIdPrefixFull::default();
        let mut next_prefix = AccountIdPrefixFull::default();
        let mut import_block_lt = 0;

        let (add, remove) = match out_msg {
            // msg_export_ext$000 msg:^(Message Any) transaction:^Transaction = OutMsg;
            // exporting an outbound external message
            OutMsg::External(info) => {
                let msg_hash = info.message_hash();
                if &msg_hash != key {
                    reject_query!(
                        "OutMsg with key {key:x} refers to a message with different hash {msg_hash:x}",
                    )
                }
                let msg = info.read_message()?;
                if !msg.is_outbound_external() {
                    reject_query!(
                        "OutMsg with key {key:x} is a msg_export_ext$000, \
                        but it does not refer to an external outbound message"
                    )
                }
                let Some(src) = msg.src_ref() else {
                    reject_query!(
                        "source of outbound message with hash {key:x} \
                        is an invalid blockchain address"
                    )
                };
                (src_wc, src_addr) = src.extract_std_address(true)?;
                src_prefix = AccountIdPrefixFull::std_prefix(src_wc, &src_addr)?;
                if !base.shard().contains_full_prefix(&src_prefix) {
                    reject_query!(
                        "outbound external message with hash {key:x} \
                        has source address {src_prefix}... not in this shard",
                    )
                }
                created_lt = msg.created_lt().unwrap_or_default();
                (false, false)
            }
            OutMsg::Immediate(_) => (false, false),
            OutMsg::New(_) | OutMsg::Transit(_) => (true, false), // added to OutMsgQueue
            OutMsg::Dequeue(info) => {
                import_block_lt = info.import_block_lt;
                (false, true)
            }
            OutMsg::DequeueShort(info) => {
                next_prefix.workchain_id = info.next_workchain;
                next_prefix.prefix = info.next_addr_pfx;
                import_block_lt = info.import_block_lt;
                (false, true)
            }
            OutMsg::DequeueImmediate(_) => (false, true),
            OutMsg::TransitRequeued(_) => (true, true), // removed from OutMsgQueue, and then added
            OutMsg::NewDefer(_) => (false, false),
            OutMsg::DeferredTransit(_) => {
                let emitted_lt = env.emitted_lt();
                if emitted_lt == 0 {
                    reject_query!(
                        "msg_export_deferred_tr for OutMsg with key {key:x} \
                        does not have emitted_lt in MsgEnvelope",
                    )
                }
                if emitted_lt < base.info.start_lt() || emitted_lt > base.info.end_lt() {
                    reject_query!(
                        "emitted_lt for msg_export_deferred_tr with key {key:x} \
                        is not between start and end lt of the block"
                    )
                }
                (true, false)
            }
            _ => reject_query!("OutMsg with key (message hash) {key:x} has an unknown tag"),
        };
        // common checks for all (non-external) inbound messages
        if !matches!(out_msg, OutMsg::DequeueShort(_) | OutMsg::External(_)) {
            let msg_hash = env.message_hash();
            if &msg_hash != key {
                reject_query!(
                    "OutMsg with key {key:x} refers to a message with different hash {msg_hash:x}",
                )
            }
            let msg = env.read_message()?;
            let Some(header) = msg.int_header() else {
                reject_query!(
                    "OutMsg with key {key:x} must be internal \
                    but it does not refer to an internal message"
                )
            };
            let Some(src) = msg.src_ref() else {
                reject_query!(
                    "source of outbound internal message with hash {key:x} \
                    is an invalid blockchain address"
                )
            };
            (src_wc, src_addr) = src.extract_std_address(true)?;
            src_prefix = AccountIdPrefixFull::std_prefix(src_wc, &src_addr)?;
            dest_prefix = AccountIdPrefixFull::checked_prefix(&header.dst)?;
            created_lt = msg.created_lt().unwrap_or_default();
            if matches!(out_msg, OutMsg::NewDefer(_)) {
                if !env.cur_addr().is_zero() || !env.next_addr().is_zero() {
                    reject_query!(
                        "cur_addr and next_addr of the message in DispatchQueue must be zero"
                    )
                }
                cur_prefix = dest_prefix.clone();
                next_prefix = dest_prefix.clone();
            } else {
                cur_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, env.cur_addr())?;
                next_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, env.next_addr())?;
                // check that next hop is nearer to the destination than the current address
                if count_matching_bits(&dest_prefix, &next_prefix)
                    < count_matching_bits(&dest_prefix, &cur_prefix)
                {
                    reject_query!(
                        "next hop address {next_prefix}... of outbound internal message with hash \
                        {key:x} is further from its destination {dest_prefix}... than its current \
                        address {}...",
                        cur_prefix
                    )
                }
                // current address must belong to this shard (otherwise we should never had exported this message)
                if !base.shard().contains_full_prefix(&cur_prefix) {
                    reject_query!(
                        "current address {cur_prefix}... of outbound internal message with hash \
                        {key:x} does not belong to the current block's shard {}",
                        base.shard()
                    )
                }
                // next hop may coincide with current address only if destination is already reached
                if next_prefix == cur_prefix && cur_prefix != dest_prefix {
                    reject_query!(
                        "next hop address {next_prefix}... of outbound internal message with hash {key:x} \
                        coincides with its current address, but this message has not reached \
                        its final destination {dest_prefix} ... yet",
                    )
                }
            }
            // if a message is created by a transaction, it must have source inside the current shard
            if trans_cell.is_some() && !base.shard().contains_full_prefix(&src_prefix) {
                reject_query!(
                    "outbound internal message with hash {key:x} has source address {src_prefix}... \
                    not in this shard, but it has been created here by a Transaction nonetheless"
                )
            }
            // unpack original forwarding fee
            if env.fwd_fee_remaining() > &header.fwd_fee {
                reject_query!(
                    "outbound internal message with hash {key:x} has remaining forwarding fee {} \
                    larger than the original (total) forwarding fee {}",
                    env.fwd_fee_remaining(),
                    header.fwd_fee
                )
            }
        }

        if let Some(trans_cell) = &trans_cell {
            let transaction = Transaction::construct_from_cell(trans_cell.clone())?;
            // check that the transaction reference is valid, and that it
            // points to a Transaction which indeed creates this outbound internal message
            Self::is_valid_transaction_ref(base, &transaction, trans_cell.repr_hash().clone())
                .map_err(|err| {
                    error!(
                        "OutMsg corresponding to outbound message with key {key:x} \
                        contains an invalid Transaction reference (transaction not in the \
                        block's transaction list : {err:?})"
                    )
                })?;
            if !transaction.contains_out_msg(created_lt, key) {
                reject_query!(
                    "OutMsg corresponding to outbound message with key {key:x} \
                    refers to transaction that does not create this outbound message",
                )
            }
            if &src_addr != transaction.account_id() {
                reject_query!(
                    "OutMsg corresponding to outbound message with hash {key:x} and \
                    source address {src_addr:x} claims that the message was created by \
                    transaction {} of another account {:x}",
                    transaction.logical_time(),
                    transaction.account_id()
                )
            }
        }

        if matches!(out_msg, OutMsg::External(_)) {
            return Ok(()); // nothing to check more for external messages
        }
        let Some(msg_env_hash) = out_msg.envelope_message_hash() else {
            reject_query!("OutMsg with key {key:x} does not contain MsgEnvelope")
        };
        // check the OutMsgQueue update effected by this OutMsg
        let q_key = OutMsgQueueKey::with_account_prefix(&next_prefix, key.clone());
        let q_entry = manager.next().message(&q_key)?;
        let old_q_entry = manager.prev().message(&q_key)?;
        if let OutMsg::NewDefer(_) = out_msg {
            // check the DispatchQueue update
            if old_q_entry.is_some() || q_entry.is_some() {
                reject_query!(
                    "OutMsg with key (message hash) {key:x} shouldn't exist in the old and \
                    the new message queues"
                );
            }
            let Some(expected_msg_env) =
                base.new_dispatch_queue_messages(src_addr.clone(), created_lt)
            else {
                reject_query!(
                    "new deferred OutMsg with src_addr={src_addr:x}, lt={created_lt} was not \
                    added to the dispatch queue"
                )
            };
            if *expected_msg_env.repr_hash() != msg_env_hash {
                reject_query!(
                    "new deferred OutMsg with src_addr={src_addr:x}, lt={created_lt} msg envelope \
                    hash mismatch: {msg_env_hash:x} in OutMsg, {:x} in DispatchQueue",
                    expected_msg_env.repr_hash()
                )
            }
        } else {
            if old_q_entry.is_some() && q_entry.is_some() {
                reject_query!(
                    "OutMsg with key (message hash) {key:x} should have removed or \
                    added OutMsgQueue entry with key {q_key:x}, but it is present both in the \
                    old and in the new output queues"
                )
            }
            if (add || remove) && old_q_entry.is_none() && q_entry.is_none() {
                reject_query!(
                    "OutMsg with key (message hash) {key:x} should have removed or added \
                    OutMsgQueue entry with key {q_key:x}, but it is absent both from the old \
                    and from the new output queues"
                )
            }
            if !(add || remove) && (old_q_entry.is_some() || q_entry.is_some()) {
                reject_query!(
                    "OutMsg with key (message hash) {key:x} is a msg_export_imm$010, \
                    so the OutMsgQueue entry with key {q_key:x} should never be created, \
                    but it is present in either the old or the new output queue"
                )
            }
            // NB: if mode!=0, the OutMsgQueue entry has been changed, so we have already checked some conditions in precheck_one_message_queue_update()
            if add {
                let Some(enq) = q_entry else {
                    reject_query!(
                        "OutMsg with key {key:x} was expected to create OutMsgQueue entry \
                        with key {q_key:x} but it did not"
                    )
                };
                if enq.envelope_hash() != msg_env_hash {
                    reject_query!(
                        "OutMsg with key {key:x} has created OutMsgQueue entry with key {q_key:x} \
                        containing a different MsgEnvelope"
                    )
                }
                // ...
            } else if remove {
                let Some(old_enq) = &old_q_entry else {
                    reject_query!(
                        "OutMsg with key {key:x} was expected to remove OutMsgQueue \
                        entry with key {q_key:x} but it did not exist in the old queue"
                    )
                };
                if old_enq.envelope_hash() != msg_env_hash {
                    reject_query!(
                        "OutMsg with key {key:x} has dequeued OutMsgQueue entry \
                        with key {q_key:x} containing a different MsgEnvelope"
                    )
                }
                // ...
            }
        }

        let reimport = Self::check_reimport(base, out_msg, key)?;

        // ...
        match out_msg {
            // msg_export_imm
            OutMsg::Immediate(_) => match &reimport {
                // msg_import_imm
                Some(InMsg::Immediate(info)) => {
                    if info.envelope_message_hash() != msg_env_hash {
                        reject_query!(
                            "msg_import_imm InMsg record corresponding to msg_export_imm OutMsg \
                            record with key {key:x} re-imported a different MsgEnvelope"
                        )
                    }
                    if !base.shard().contains_full_prefix(&dest_prefix) {
                        reject_query!(
                            "msg_export_imm OutMsg record with key {key:x} refers to a message \
                            with destination {dest_prefix} outside this shard"
                        )
                    }
                    if cur_prefix != dest_prefix || next_prefix != dest_prefix {
                        reject_query!(
                            "msg_export_imm OutMsg record with key {key:x} refers to a message \
                            that has not been routed to its final destination"
                        )
                    }
                }
                _ => reject_query!(
                    "cannot unpack msg_import_imm InMsg record corresponding to \
                    msg_export_imm OutMsg record with key {key:x}"
                ), // ...
            },
            // msg_export_new
            OutMsg::New(_) => {
                log::debug!(
                    target: "validate_query",
                    "({}): src: {}, dst: {dest_prefix}, shard: {}",
                    base.next_block_descr,
                    src_prefix,
                    base.shard()
                );
                // perform hypercube routing for this new message
                let route_info =
                    perform_hypercube_routing(&src_prefix, &dest_prefix, base.shard(), true)
                        .map_err(|err| {
                            error!(
                                "cannot perform (check) hypercube routing for \
                                new outbound message with hash {key:x} : {err}"
                            )
                        })?;
                let new_cur_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, &route_info.0)?;
                let new_next_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, &route_info.1)?;
                if cur_prefix != new_cur_prefix || next_prefix != new_next_prefix {
                    reject_query!(
                        "OutMsg for new message with hash {key:x} tells us that it has been \
                        routed to current address {cur_prefix}... and hext hop address \
                        {next_prefix} while the correct values dictated by hypercube routing \
                        are {new_cur_prefix}... and {new_next_prefix}..."
                    )
                }
                CHECK!(base.shard().contains_full_prefix(&src_prefix));
                if base.shard().contains_full_prefix(&dest_prefix) {
                    // log::debug!(target: "validate_query", "(THIS) src=" << src_prefix cur=" << cur_prefix next=" << next_prefix dest=" << dest_prefix route_info=(" << route_info.first << "," << route_info.second << ")";
                    CHECK!(cur_prefix == dest_prefix);
                    CHECK!(next_prefix == dest_prefix);
                    Self::update_min_enqueued_lt_hash(base, created_lt, key)?;
                } else {
                    // sanity check of the implementation of hypercube routing
                    // log::debug!(target: "validate_query", "(THAT) src=" << src_prefix cur=" << cur_prefix next=" << next_prefix dest=" << dest_prefix;
                    CHECK!(base.shard().contains_full_prefix(&cur_prefix));
                    CHECK!(!base.shard().contains_full_prefix(&next_prefix));
                }
                // ...
            }
            OutMsg::NewDefer(_) => (),
            // msg_export_deferred_tr
            OutMsg::DeferredTransit(_) => {
                // msg_import_deferred_tr
                let Some(InMsg::DeferredTransit(info)) = &reimport else {
                    reject_query!(
                        "cannot unpack msg_import_tr InMsg record corresponding to \
                        msg_export_deferred_tr OutMsg record with key {key:x}"
                    )
                };
                let in_env = info.read_in_envelope_message()?;
                CHECK!(in_env.message_cell(), msg_cell);
                let in_cur_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, in_env.cur_addr())?;
                if !base.shard().contains_full_prefix(&in_cur_prefix) {
                    reject_query!(
                        "msg_export_deferred_tr OutMsg record with key {key:x} corresponds to \
                        msg_import_deferred_tr InMsg record with current imported message address \
                        {in_cur_prefix} NOT inside the current shard"
                    )
                }
            }
            // msg_export_tr
            OutMsg::Transit(_) => {
                // msg_import_tr
                let Some(InMsg::Transit(info)) = &reimport else {
                    reject_query!(
                        "cannot unpack msg_import_tr InMsg record corresponding to \
                        msg_export_tr OutMsg record with key {key:x}"
                    )
                };
                let in_env = info.read_in_message()?;
                CHECK!(in_env.message_cell(), msg_cell);
                let in_cur_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, in_env.cur_addr())?;
                let in_next_prefix =
                    src_prefix.interpolate_addr_intermediate(&dest_prefix, in_env.next_addr())?;
                if base.shard().contains_full_prefix(&in_cur_prefix) {
                    reject_query!(
                        "msg_export_tr OutMsg record with key {:x} corresponds to \
                        msg_import_tr InMsg record with current imported message address {} \
                        inside the current shard \
                        (msg_export_tr_req should have been used instead)",
                        key,
                        in_cur_prefix
                    );
                }
                // we have already checked correctness of hypercube routing in InMsg::msg_import_tr case of check_in_msg()
                CHECK!(base.shard().contains_full_prefix(&in_next_prefix));
                CHECK!(base.shard().contains_full_prefix(&cur_prefix));
                CHECK!(!base.shard().contains_full_prefix(&next_prefix));
                // ...
            }
            // msg_export_deq
            OutMsg::DequeueShort(_) | OutMsg::Dequeue(_) => {
                // check that the message has been indeed processed by a neighbor
                let Some(enq) = old_q_entry else {
                    reject_query!(
                        "cannot unpack old OutMsgQueue entry corresponding to \
                        msg_export_deq OutMsg entry with key {key:x}"
                    )
                };
                let mut delivered = false;
                let mut deliver_lt = 0;
                for neighbor in manager.neighbors() {
                    // could look up neighbor with shard containing enq.next_prefix more efficiently
                    // (instead of checking all neighbors)
                    if !neighbor.is_disabled() && neighbor.already_processed(&enq)? {
                        delivered = true;
                        deliver_lt = neighbor.end_lt();
                        break;
                    }
                }
                if !delivered {
                    reject_query!(
                        "msg_export_deq OutMsg entry with key {key:x} attempts to dequeue \
                        a message with next hop {next_prefix} that has not been yet processed \
                        by the corresponding neighbor"
                    )
                }
                if deliver_lt != import_block_lt {
                    log::warn!(
                        target: "validate_query",
                        "({}): msg_export_deq OutMsg entry with key {key:x} claims the dequeued \
                        message with next hop {next_prefix} has been delivered in block with \
                        end_lt={import_block_lt} while the correct value is {deliver_lt}",
                        base.next_block_descr,
                    );
                }
            }
            // msg_export_tr_req
            OutMsg::TransitRequeued(_) => match &reimport {
                // msg_import_tr
                Some(InMsg::Transit(info)) => {
                    let in_env = info.read_in_message()?;
                    let msg_hash = in_env.message_hash();
                    if &msg_hash != key {
                        reject_query!("hash {msg_hash:x} != Message hash {key:x}")
                    }
                    let in_env = info.read_in_message()?;
                    let in_cur_prefix = src_prefix
                        .interpolate_addr_intermediate(&dest_prefix, in_env.cur_addr())?;
                    let in_next_prefix = src_prefix
                        .interpolate_addr_intermediate(&dest_prefix, in_env.next_addr())?;
                    if !base.shard().contains_full_prefix(&in_cur_prefix) {
                        reject_query!(
                            "msg_export_tr_req OutMsg record with key {:x} corresponds to \
                            msg_import_tr InMsg record with current imported message address {} \
                            outside the current shard (msg_export_tr should have been used instead,
                            because there was no re-queueing)",
                            key,
                            in_cur_prefix
                        )
                    }
                    // we have already checked correctness of hypercube routing in InMsg::msg_import_tr case of check_in_msg()
                    CHECK!(base.shard().contains_full_prefix(&in_next_prefix));
                    CHECK!(base.shard().contains_full_prefix(&cur_prefix));
                    CHECK!(!base.shard().contains_full_prefix(&next_prefix));
                    // so we have just to check that the rewritten message (envelope) has been enqueued
                    // (already checked above for q_entry since mode = 3)
                    // and that the original message (envelope) has been dequeued
                    let q_key = OutMsgQueueKey::with_account_prefix(&in_next_prefix, key.clone());
                    let q_entry = manager.next().message(&q_key)?;
                    let Some(enq) = manager.prev().message(&q_key)? else {
                        reject_query!(
                            "msg_export_tr_req OutMsg record with key {:x} was expected to dequeue \
                            message from OutMsgQueue with key {:x} but such a message is absent \
                            from the old OutMsgQueue",
                            key,
                            q_key
                        )
                    };
                    if q_entry.is_some() {
                        reject_query!(
                            "msg_export_tr_req OutMsg record with key {:x} \
                            was expected to dequeue message from OutMsgQueue with key {:x} \
                            but such a message is still present in the new OutMsgQueue",
                            key,
                            q_key
                        )
                    }
                    if enq.envelope_hash() != info.in_envelope_message_hash() {
                        reject_query!(
                            "msg_import_tr InMsg entry corresponding to msg_export_tr_req OutMsg \
                            entry with key {:x} has re-imported a different MsgEnvelope from that \
                            present in the old OutMsgQueue",
                            key
                        )
                    }
                }
                _ => reject_query!(
                    "cannot unpack msg_import_tr InMsg record corresponding to \
                    msg_export_tr_req OutMsg record with key {:x}",
                    key
                ),
            },
            // msg_export_deq_imm
            OutMsg::DequeueImmediate(_) => match &reimport {
                // msg_import_fin
                Some(InMsg::Final(info)) => {
                    if info.envelope_message_hash() != msg_env_hash {
                        reject_query!(
                            "msg_import_fin InMsg record corresponding to msg_export_deq_imm OutMsg \
                            record with key {key:x} somehow imported a different MsgEnvelope from that \
                            dequeued by msg_export_deq_imm"
                        )
                    }
                    if !base.shard().contains_full_prefix(&cur_prefix) {
                        reject_query!(
                            "msg_export_deq_imm OutMsg record with key {key:x} \
                            dequeued a MsgEnvelope with current address {cur_prefix}... outside current shard",
                        )
                    }
                    // we have already checked more conditions in check_in_msg() case msg_import_fin
                    CHECK!(base.shard().contains_full_prefix(&next_prefix)); // sanity check
                    CHECK!(base.shard().contains_full_prefix(&dest_prefix)); // sanity check
                                                                             // ...
                }
                _ => reject_query!(
                    "cannot unpack msg_import_fin InMsg record corresponding to msg_export_deq_imm \
                    OutMsg record with key {key:x}"
                ),
            },
            _ => reject_query!("unknown OutMsg tag {:?}", out_msg),
        }
        if matches!(
            out_msg,
            OutMsg::Immediate(_)
                | OutMsg::DequeueImmediate(_)
                | OutMsg::New(_)
                | OutMsg::DeferredTransit(_)
        ) {
            if src_wc != base.shard().workchain_id() {
                return Ok(()); // no need to check further
            }
            if let (OutMsg::Immediate(_), Some(in_msg)) = (out_msg, &reimport) {
                if base.is_special_in_msg(in_msg) {
                    // special InMsg, no need to check further
                    return Ok(());
                }
            }
            let emitted_lt = if env.emitted_lt() == 0 { created_lt } else { env.emitted_lt() };
            base.result.msg_emitted_lt.push((src_addr, created_lt, emitted_lt));
        }
        Ok(())
    }

    fn check_out_msg_descr(
        base: Arc<ValidateBase>,
        manager: Arc<MsgQueueManager>,
        tasks: &mut TasksVec,
    ) -> Result<()> {
        // log::debug!(target: "validate_query", "({}): checking outbound messages listed in OutMsgDescr", base.next_block_descr);
        base.out_msg_descr
            .iterate_with_keys(|key, out_msg| {
                let base = base.clone();
                let manager = manager.clone();
                Self::add_task(tasks, move || {
                    Self::check_out_msg(&base, &manager, &key, &out_msg).map_err(|err| {
                        error!(
                            "invalid OutMsg with key {key:x} in the new block {} : {err:?} {out_msg:?}",
                            base.block_id(),
                        )
                    })
                });
                Ok(true)
            })
            .map_err(|err| {
                error!(
                    "invalid OutMsgDescr dictionary in the new block {} : {}",
                    base.block_id(),
                    err
                )
            })?;
        Ok(())
    }

    fn check_dispatch_queue_update(base: &ValidateBase) -> Result<()> {
        if let Some(k_v) = base.result.new_dispatch_queue_messages.iter().next() {
            reject_query!(
                "DispatchQueue has a new message with src_addr={:x}, \
                lt={}, but no correseponding OutMsg exists",
                k_v.key().0,
                k_v.key().1,
            )
        }
        if let Some(k_v) = base.result.removed_dispatch_queue_messages.iter().next() {
            reject_query!(
                "message with src_addr={:x}, lt={} was removed \
                from DispatchQueue, but no correseponding InMsg exists",
                k_v.key().0,
                k_v.key().1,
            )
        }
        Ok(())
    }

    // compare to Collator::update_processed_upto()
    fn check_processed_upto(
        base: &ValidateBase,
        manager: &MsgQueueManager,
        mc_data: &McData,
    ) -> Result<()> {
        log::debug!(target: "validate_query", "checking ProcessedInfo");
        if !manager.next().is_reduced() {
            reject_query!(
                "new ProcessedInfo is not reduced (some entries completely cover other entries)"
            );
        }
        let (claimed_proc_lt, claimed_proc_hash);
        let ok_upd = manager.next().is_simple_update_of(manager.prev());
        if !ok_upd.0 {
            reject_query!(
                "new ProcessedInfo is not obtained from old ProcessedInfo \
                by adding at most one new entry"
            )
        } else if let Some(upd) = ok_upd.1 {
            if upd.shard != base.shard().shard_prefix_with_tag() {
                reject_query!(
                    "newly-added ProcessedInfo entry refers to shard {} distinct from the \
                    current shard {}",
                    ShardIdent::with_tagged_prefix(base.shard().workchain_id(), upd.shard)?,
                    base.shard()
                )
            }
            #[cfg(not(feature = "xp25"))]
            {
                let ref_mc_seqno = match base.shard().is_masterchain() {
                    true => base.block_id().seq_no,
                    false => mc_data.state.state()?.seq_no(),
                };
                if upd.mc_seqno() != ref_mc_seqno {
                    reject_query!(
                        "newly-added ProcessedInfo entry refers to masterchain block {} but \
                        the processed inbound message queue belongs to masterchain block {}",
                        upd.mc_seqno(),
                        ref_mc_seqno
                    )
                }
            }
            #[cfg(feature = "xp25")]
            {
                if upd.seqno != base.block_id().seq_no {
                    reject_query!(
                        "newly-added ProcessedInfo entry refers to block {} but \
                        the processed inbound message queue belongs to block {}",
                        upd.seqno,
                        base.block_id().seq_no
                    )
                }
                if !base.shard().is_masterchain()
                    && upd.mc_seqno() != mc_data.state.state()?.seq_no()
                {
                    reject_query!(
                        "newly-added ProcessedInfo entry refers to masterchain block {} but \
                        the processed inbound message queue belongs to masterchain block {}",
                        upd.mc_seqno(),
                        mc_data.state.state()?.seq_no()
                    )
                }
            }
            if upd.last_msg_lt >= base.info.end_lt() {
                reject_query!(
                    "newly-added ProcessedInfo entry claims that the last processed message \
                    has lt {} larger than this block's end lt {}",
                    upd.last_msg_lt,
                    base.info.end_lt()
                )
            }
            if upd.last_msg_lt == 0 {
                reject_query!(
                    "newly-added ProcessedInfo entry claims that \
                    the last processed message has zero lt"
                )
            }
            claimed_proc_lt = upd.last_msg_lt;
            claimed_proc_hash = upd.last_msg_hash;
        } else {
            claimed_proc_lt = 0;
            claimed_proc_hash = UInt256::default();
        }
        log::debug!(
            target: "validate_query",
            "({}): ProcessedInfo claims to have processed all inbound messages up to ({},{:x})",
            base.next_block_descr,
            claimed_proc_lt, claimed_proc_hash
        );
        if let Some(key_val) = base.result.lt_hash.get(&0) {
            let (proc_lt, proc_hash) = key_val.val();
            if &claimed_proc_lt < proc_lt
                || (&claimed_proc_lt == proc_lt && proc_lt != &0 && &claimed_proc_hash < proc_hash)
            {
                reject_query!(
                    "the ProcessedInfo claims to have processed messages only upto ({},{:x}), \
                    but there is a InMsg processing record for later message ({},{:x})",
                    claimed_proc_lt,
                    claimed_proc_hash,
                    proc_lt,
                    proc_hash
                )
            }
        }
        if let Some(key_val) = base.result.lt_hash.get(&1) {
            let (min_enq_lt, min_enq_hash) = key_val.val();
            if min_enq_lt < &claimed_proc_lt
                || (min_enq_lt == &claimed_proc_lt && &claimed_proc_hash >= min_enq_hash)
            {
                reject_query!(
                    "the ProcessedInfo claims to have processed all messages only upto ({},{:x}), \
                    but there is a OutMsg enqueuing record for earlier message ({},{:x})",
                    claimed_proc_lt,
                    claimed_proc_hash,
                    min_enq_lt,
                    min_enq_hash
                )
            }
        }
        base.result.lt_hash.insert(2, (claimed_proc_lt, claimed_proc_hash));
        // ...
        Ok(())
    }

    // similar to Collator::process_inbound_message
    fn check_neighbor_outbound_message_processed(
        base: &ValidateBase,
        manager: &MsgQueueManager,
        enq: MsgEnqueueStuff,
        key: &OutMsgQueueKey,
        nb_block_id: &BlockIdExt,
    ) -> Result<MsgProcessedStatus> {
        CHECK!(base.shard().contains_full_prefix(enq.next_prefix()));

        let in_msg = base.in_msg_descr.get(&key.hash)?;
        let out_msg = base.out_msg_descr.get(&key.hash)?;
        let f0 = manager.prev().already_processed(&enq)?;
        let f1 = manager.next().already_processed(&enq)?;
        if f0 && !f1 {
            reject_query!(
                "a previously processed message has been un-processed \
                (impossible situation after the validation of ProcessedInfo)"
            )
        } else if f0 {
            // f0 && f1
            // this message has been processed in a previous block of this shard
            // just check that we have not imported it once again
            if in_msg.is_some() {
                reject_query!(
                    "have an InMsg entry for processing again already processed EnqueuedMsg \
                    with key {:x} of neighbor {}",
                    key,
                    nb_block_id
                )
            }
            if base.shard().contains_full_prefix(enq.cur_prefix()) {
                // if this message comes from our own outbound queue, we must have dequeued it
                let deq_hash = match out_msg {
                    None => reject_query!(
                        "our old outbound queue contains EnqueuedMsg with key {:x} already \
                        processed by this shard, but there is no ext_message_deq OutMsg \
                        record for this message in this block",
                        key
                    ),
                    Some(OutMsg::DequeueShort(deq)) => deq.msg_env_hash,
                    Some(OutMsg::DequeueImmediate(deq)) => {
                        deq.out_message_cell().repr_hash().clone()
                    }
                    Some(deq) => reject_query!(
                        "{:?} msg_export_deq OutMsg record for already \
                        processed EnqueuedMsg with key {:x} of old outbound queue",
                        deq,
                        key
                    ),
                };
                if deq_hash != enq.envelope_hash() {
                    reject_query!(
                        "unpack ext_message_deq OutMsg record for already processed EnqueuedMsg \
                        with key {:x} of old outbound queue contains a different MsgEnvelope",
                        key
                    )
                }
            }
            // next check is incorrect after a merge, when ns_.processed_upto has > 1 entries
            // we effectively comment it out
            Ok(MsgProcessedStatus::ProcessedInPreviousBlock)
            // NB. we might have a non-trivial dequeueing out_entry with this message hash, but another envelope (for transit messages)
            // (so we cannot assert that out_entry is null)
            // if self.claimed_proc_lt != 0
            //     && (self.claimed_proc_lt < created_lt || (self.claimed_proc_lt == created_lt && self.claimed_proc_hash < key.hash)) {
            //     log::error!(target: "validate_query", "internal inconsistency: new ProcessedInfo claims \
            //         to have processed all messages up to ({},{}) but we had somehow already processed a message ({},{}) \
            //         from OutMsgQueue of neighbor {} key {:x}", self.claimed_proc_lt, self.claimed_proc_hash,
            //             created_lt, key.hash, nb_block_id, key)
            //     return Ok(false)
            // }
            // Ok(true)
        } else if f1 {
            // !f0 && f1
            // this message must have been imported and processed in this very block
            // (because it is marked processed after this block, but not before)
            if let Some(key_val) = base.result.lt_hash.get(&2) {
                let (claimed_proc_lt, claimed_proc_hash) = key_val.val();
                if claimed_proc_lt == &0
                    || claimed_proc_lt < &enq.lt()
                    || (claimed_proc_lt == &enq.lt() && claimed_proc_hash < &key.hash)
                {
                    reject_query!(
                        "internal inconsistency: new ProcessedInfo claims to have processed all \
                        messages up to ({},{:x}), but we had somehow processed in this block \
                        a message ({},{:x}) from OutMsgQueue of neighbor {} key {:x}",
                        claimed_proc_lt,
                        claimed_proc_hash,
                        enq.lt(),
                        key,
                        nb_block_id,
                        key
                    )
                }
            }
            // must have a msg_import_fin or msg_import_tr InMsg record
            let hash = match in_msg {
                Some(InMsg::Final(info)) => info.envelope_message_hash(),
                Some(InMsg::Transit(info)) => info.in_envelope_message_hash(),
                None => reject_query!(
                    "there is no InMsg entry for processing EnqueuedMsg with key {:x} of neighbor \
                    {} which is claimed to be processed by new ProcessedInfo of this block",
                    key,
                    nb_block_id
                ),
                _ => reject_query!(
                    "expected either a msg_import_fin or a msg_import_tr InMsg record for \
                    processing EnqueuedMsg with key {:x} of neighbor {} which is claimed \
                    to be processed by new ProcessedInfo of this block",
                    key,
                    nb_block_id
                ),
            };
            if hash != enq.envelope_hash() {
                reject_query!(
                    "InMsg record for processing EnqueuedMsg with key {:x} of neighbor {} \
                    which is claimed to be processed by new ProcessedInfo of this block \
                    contains a reference to a different MsgEnvelope",
                    key,
                    nb_block_id
                );
            }
            // all other checks have been done while checking InMsgDescr
            Ok(MsgProcessedStatus::ProcessedInThisBlock)
        } else {
            // !f0 && !f1
            // the message is left unprocessed in our virtual "inbound queue"
            // just a simple sanity check
            if let Some(key_val) = base.result.lt_hash.get(&2) {
                let (claimed_proc_lt, claimed_proc_hash) = key_val.val();
                if claimed_proc_lt != &0
                    && !(claimed_proc_lt < &enq.lt()
                        || (claimed_proc_lt == &enq.lt() && claimed_proc_hash < &key.hash))
                {
                    log::error!(
                        target: "validate_query",
                        "({}): internal inconsistency: new ProcessedInfo claims to have processed \
                        all messages up to ({},{:x}), but we somehow have not processed \
                        a message ({},{:x}) from OutMsgQueue of neighbor {} key {:x}",
                        base.next_block_descr,
                        claimed_proc_lt, claimed_proc_hash,
                        enq.lt(), key.hash,
                        nb_block_id, key
                    );
                }
            }
            Ok(MsgProcessedStatus::NotProcessed)
        }
    }

    // return true if all queues are processed
    fn check_in_queue(base: &ValidateBase, manager: &MsgQueueManager) -> Result<bool> {
        let mut remaining_messages_count = base.total_imported_msgs.load(Ordering::Relaxed);
        log::debug!(
            target: "validate_query",
            "({}): check_in_queue neighbours {}, messages: {}",
            base.next_block_descr,
            manager.neighbors().len(),
            remaining_messages_count
        );
        if remaining_messages_count == 0 {
            return Ok(true);
        }
        let iter = manager.merge_out_queue_iter(base.shard())?;
        for k_v in iter {
            let (msg_key, enq, nb_block_id) = k_v?;
            log::debug!(
                target: "validate_query",
                "({}): processing inbound message with (lt,hash)=({},{:x}) from neighbor - {}",
                base.next_block_descr,
                enq.lt(),
                msg_key.hash,
                nb_block_id
            );
            match Self::check_neighbor_outbound_message_processed(
                base,
                manager,
                enq,
                &msg_key,
                &nb_block_id,
            ) {
                Err(err) => {
                    reject_query!(
                        "error processing outbound internal message {:x} of neighbor {} : {}",
                        msg_key.hash,
                        nb_block_id,
                        err
                    )
                }
                Ok(MsgProcessedStatus::NotProcessed) => return Ok(false),
                Ok(MsgProcessedStatus::ProcessedInPreviousBlock) => (),
                Ok(MsgProcessedStatus::ProcessedInThisBlock) => {
                    remaining_messages_count -= 1;
                    if remaining_messages_count == 0 {
                        break;
                    }
                }
            }
        }
        Ok(true)
    }

    // checks that all messages imported from our outbound queue into neighbor shards have been dequeued
    // similar to Collator::out_msg_queue_cleanup()
    // (but scans new outbound queue instead of the old)
    // this operation could be long and must be called if outbound queue is short only
    fn _check_delivered_dequeued(base: &ValidateBase, manager: &MsgQueueManager) -> Result<bool> {
        log::debug!(
            target: "validate_query",
            "({}): scanning new outbound queue and checking delivery status of all messages",
            base.next_block_descr
        );
        for nb in manager.neighbors() {
            if !nb.is_disabled() && !nb.can_check_processed() {
                reject_query!(
                    "internal error: no info for checking processed messages from neighbor {}",
                    nb.block_id()
                )
            }
        }
        // TODO: warning may be too much messages
        manager.next().out_queue().iterate_with_keys_and_aug(|msg_key, enq, lt| {
            if lt >= base.info.start_lt() {
                log::debug!(
                    target: "validate_query",
                    "({}): stop scanning new outbound queue",
                    base.next_block_descr
                );
                return Ok(false);
            }
            let enq = MsgEnqueueStuff::from_enqueue(enq, lt)?;
            if msg_key.hash != enq.message_hash() {
                reject_query!(
                    "cannot unpack EnqueuedMsg with key {msg_key:x} in the new OutMsgQueue"
                )
            }
            for nb in manager.neighbors() {
                // could look up neighbor with shard containing enq.next_prefix more efficiently
                // (instead of checking all neighbors)
                if !nb.is_disabled() && nb.already_processed(&enq)? {
                    // the message has been delivered but not removed from queue!
                    reject_query!(
                        "({}): outbound queue not cleaned up completely (overfull block?): \
                        outbound message with (lt,hash)=({lt},{:x}) enqueued_lt={} has \
                        been already delivered and processed by neighbor {} but it has not been \
                        dequeued in this block and it is still present in the new outbound queue",
                        base.next_block_descr,
                        msg_key.hash,
                        enq.enqueued_lt(),
                        nb.block_id()
                    );
                }
            }
            if !base.shard().contains_full_prefix(enq.cur_prefix()) {
                reject_query!(
                    "({}): outbound queue not cleaned up completely (overfull block?): outbound \
                    message with (lt,hash)=({lt},{:x}) enqueued_lt={} has been left from \
                    split and should be removed in underload block",
                    base.next_block_descr,
                    msg_key.hash,
                    enq.enqueued_lt()
                )
            }
            Ok(true)
        })?;
        Ok(true)
    }

    // similar to Collator::make_account()
    fn unpack_account(base: &ValidateBase, addr: &AccountId) -> Result<(Cell, Account)> {
        match base.prev_state_accounts.get(addr)? {
            Some(shard_acc) => {
                let new_acc = shard_acc.read_account()?;
                if !new_acc.belongs_to_shard(base.shard())? {
                    reject_query!(
                        "old state of account {addr:x} does not really belong to current shard"
                    )
                }
                Ok((shard_acc.account_cell(), new_acc))
            }
            None => {
                let new_acc = Account::default();
                Ok((new_acc.serialize()?, new_acc))
            }
        }
    }

    fn check_one_transaction(
        base: &ValidateBase,
        config: BlockchainConfig,
        libraries: Libraries,
        account: &mut Account,
        account_addr: &AccountId,
        account_root: &mut Cell,
        storage_dict: &mut Option<Cell>,
        lt: u64,
        trans_root: Cell,
        is_first: bool,
        is_last: bool,
    ) -> Result<bool> {
        let trans_hash = trans_root.repr_hash();
        log::debug!(
            target: "validate_query",
            "({}): checking {lt} transaction {trans_hash:x} of account {account_addr:x}",
            base.next_block_descr,
        );
        let trans = Transaction::construct_from_cell(trans_root.clone())?;
        let account_create = account.is_none();
        let workchain_id = base.shard().workchain_id() as i8;

        // check input message
        let mut money_imported = CurrencyCollection::default();
        let mut money_exported = CurrencyCollection::default();
        let in_msg_opt = trans.read_in_msg()?;
        let in_msg_cell = trans.in_msg_cell();
        let mut msg_metadata = None;
        let mut simple_msg_metadata = true;
        if let Some(msg) = &in_msg_opt {
            let in_msg_hash = trans.in_msg.hash();
            let in_msg = match base.in_msg_descr.get(&in_msg_hash)? {
                Some(in_msg) => in_msg,
                None => {
                    reject_query!(
                        "inbound message with hash {in_msg_hash:x} of transaction {lt} of account \
                        {account_addr:x} does not have a corresponding InMsg record"
                    )
                }
            };
            // once we know there is a InMsg with correct hash, we already know that it contains
            // a message with this hash (by the verification of InMsg), so it is our message
            // have still to check its destination address and imported value
            // and that it refers to this transaction
            match in_msg {
                InMsg::External(_) => (),
                InMsg::IHR(_) | InMsg::Immediate(_) | InMsg::Final(_) | InMsg::DeferredFinal(_) => {
                    let Some(header) = msg.int_header() else {
                        reject_query!(
                            "inbound message transaction {lt} of {account_addr:x} \
                            must have internal message header",
                        )
                    };
                    if header.created_lt >= lt {
                        reject_query!(
                            "transaction {lt} of {account_addr:x} processed inbound message \
                            created later at logical time {}",
                            header.created_lt
                        )
                    }
                    let mut emitted_lt = header.created_lt;
                    let is_special = base.is_special_in_msg(&in_msg);
                    simple_msg_metadata = is_special;
                    let Some(env) = in_msg.read_in_msg_envelope()? else {
                        reject_query!(
                            "InMsg record for inbound message with hash {in_msg_hash:x} of \
                            transaction {lt} of account {account_addr:x} does not have a valid MsgEnvelope"
                        )
                    };
                    if !is_special {
                        msg_metadata = env.metadata_add_depth();
                    }
                    if env.emitted_lt() != 0 {
                        emitted_lt = env.emitted_lt();
                    }
                    if header.created_lt != base.info.start_lt() || !is_special {
                        base.result.msg_proc_lt.push((account_addr.clone(), lt, emitted_lt));
                    }
                    money_imported = header.value.clone();
                }
                _ => reject_query!(
                    "inbound message with hash {in_msg_hash:x} of transaction {lt} \
                    of account {account_addr:x} has an invalid InMsg record ${:05b} \
                    (not one of msg_import_ext, msg_import_fin, \
                    msg_import_imm or msg_import_ihr)",
                    in_msg.tag(),
                ),
            }
            let dst = match msg.dst_ref() {
                Some(MsgAddressInt::AddrStd(dst)) => dst,
                _ => reject_query!(
                    "inbound message with hash {in_msg_hash:x} transaction {lt} of \
                    {account_addr:x} must have std internal destination address"
                ),
            };
            if dst.workchain_id != workchain_id || account_addr != &dst.address {
                reject_query!(
                    "inbound message of transaction {lt} of account {account_addr:x} \
                    has a different destination address {}:{:x}",
                    dst.workchain_id,
                    dst.address
                )
            }
            CHECK!(in_msg.transaction_cell().is_some());
            if let Some(cell) = in_msg.transaction_cell() {
                if cell != trans_root {
                    reject_query!(
                        "InMsg record for inbound message with hash {in_msg_hash:x} of \
                        transaction {lt} of account {account_addr:x} refers to a different \
                        processing transaction"
                    )
                }
            }
        }
        let new_msg_metadata = match msg_metadata {
            Some(metadata) => Some(metadata),
            None if simple_msg_metadata => Some(MsgMetadata::new(
                MsgAddressInt::standard(workchain_id, account_addr.clone()),
                lt,
            )),
            None => None,
        };
        // check output messages
        trans.out_msgs.iterate_slices_with_keys(|key, out_msg| {
            let out_msg_root = out_msg.reference(0)?;
            let out_msg_hash = out_msg_root.repr_hash();
            let i = key.get_int(15)? + 1;
            let Some(out_msg) = base.out_msg_descr.get(&out_msg_hash)? else {
                reject_query!(
                    "outbound message #{i} with hash {out_msg_hash:x} of transaction {lt} of \
                    account {account_addr:x} does not have a corresponding  record"
                )
            };
            // once we know there is an OutMsg with correct hash, we already know that it contains
            // a message with this hash (by the verification of OutMsg), so it is our message
            // have still to check its source address, lt and imported value
            // and that it refers to this transaction as its origin
            let msg = Message::construct_from_cell(out_msg_root.clone())?;
            let mut is_exported = false;
            match out_msg {
                OutMsg::External(_) => is_exported = true,
                OutMsg::Immediate(_) | OutMsg::New(_) | OutMsg::NewDefer(_) => {
                    let Some(header) = msg.int_header() else {
                        reject_query!(
                            "transaction {lt} of {account_addr:x} must have internal message header"
                        )
                    };
                    let Some(msg_env) = out_msg.read_msg_envelope()? else {
                        reject_query!(
                            "cannot unpack outbound message #{i} with hash {out_msg_hash:x} \
                            of transaction {lt} of account {account_addr:x}"
                        )
                    };
                    money_exported.add(&header.value)?;
                    money_exported.coins.add(msg_env.fwd_fee_remaining())?;
                    let msg_metadata = msg_env.metadata();
                    if msg_metadata != new_msg_metadata.as_ref() {
                        reject_query!(
                            "outbound message #{i} with hash {out_msg_hash:x} of transaction {lt} \
                            of account {account_addr:x} has invalid metadata in an OutMsg record: \
                            expected {new_msg_metadata:?}, found {msg_metadata:?}"
                        )
                    }
                }
                _ => reject_query!(
                    "outbound message #{i} with hash {out_msg_hash:x} of transaction {lt} of \
                    account {account_addr:x} has an invalid OutMsg record (not one of \
                    msg_export_ext, msg_export_new, msg_export_imm or msg_export_new_defer)"
                ),
            }
            let created_lt = msg.created_lt().unwrap_or_default();
            let src = match msg.src_ref() {
                Some(MsgAddressInt::AddrStd(src)) => src,
                _ => reject_query!(
                    "outbound message #{i} with hash {out_msg_hash:x} of transaction {lt} of \
                    account {account_addr:x} does not have a correct header"
                ),
            };
            if src.workchain_id != workchain_id
                || account_addr != &src.address
            {
                reject_query!(
                    "outbound message #{i} of transaction {lt} of account {account_addr:x} has a different \
                    source address {}:{:x}",
                    src.workchain_id,
                    src.address
                )
            }
            let Some(cell) = out_msg.transaction_cell() else {
                reject_query!(
                    "outbound message #{i} of transaction {lt} of account {account_addr:x} has no \
                    reference to its processing transaction",
                )
            };
            if cell != trans_root {
                reject_query!(
                    "OutMsg record for outbound message #{i} with hash {out_msg_hash:x} \
                    of transaction {lt} of account {account_addr:x} refers to a different \
                    processing transaction",
                )
            }
            if !is_exported {
                let addr_contains = base.result.account_expected_defer_all_messages.contains(account_addr);
                if matches!(out_msg, OutMsg::NewDefer(_)) {
                    log::debug!(
                        target: "validate_query",
                        "message from account {workchain_id}:{account_addr:x} with lt \
                        {created_lt} was deferred");
                    if !addr_contains && !base.config_params.has_capability(GlobalCapabilities::CapDeferMessages) {
                        reject_query!(
                            "outbound message #{i} on account {workchain_id}:{account_addr:x} \
                            is deferred, but deferring messages is disabled",
                        )
                    }
                    if !addr_contains && i == 1 {
                        reject_query!(
                            "outbound message #1 on account {workchain_id}:{account_addr:x} \
                            must not be deferred (the first message cannot be deferred unless \
                            some previous messages are deferred)",
                        )
                    }
                    let _ = base.result.account_expected_defer_all_messages.insert(account_addr.clone());
                } else if addr_contains {
                    reject_query!(
                        "outbound message #{i} on account {workchain_id}:{account_addr:x} \
                        must be deferred because this account has earlier messages in DispatchQueue",
                    )
                }
            }
            Ok(true)
        })?;
        // check general transaction data
        let old_balance = account.balance().cloned().unwrap_or_default();
        let descr = trans.read_description()?;
        let split = descr.is_split();
        if split || descr.is_merge() {
            if base.shard().is_masterchain() {
                reject_query!(
                    "transaction {} of account {:x} is a split/merge prepare/install transaction, \
                    which is impossible in a masterchain block",
                    lt,
                    account_addr
                )
            }
            if split && !base.info.before_split() {
                reject_query!(
                    "transaction {} of account {:x} is a split prepare/install transaction, \
                    but this block is not before a split",
                    lt,
                    account_addr
                )
            }
            if split && !is_last {
                reject_query!(
                    "transaction {} of account {:x} is a split prepare/install transaction, \
                    but it is not the last transaction for this account in this block",
                    lt,
                    account_addr
                )
            }
            if !split && !base.info.after_merge() {
                reject_query!(
                    "transaction {} of account {:x} is a merge prepare/install transaction, \
                    is a merge prepare/install transaction, but \
                    this block is not immediately after a merge",
                    lt,
                    account_addr
                )
            }
            if !split && !is_first {
                reject_query!(
                    "transaction {} of account {:x} is a merge prepare/install transaction, \
                    is a merge prepare/install transaction, but it is not the first transaction \
                    for this account in this block",
                    lt,
                    account_addr
                )
            }
            // check later a global configuration flag in base.config_params.global_flags_
            // (for now, split/merge transactions are always globally disabled)
            reject_query!(
                "transaction {} of account {:x} is a split/merge prepare/install transaction, \
                which are globally disabled",
                lt,
                account_addr
            )
        }
        if let TransactionDescr::TickTock(ref info) = descr {
            if !base.shard().is_masterchain() {
                reject_query!(
                    "transaction {} of account {:x} is a tick-tock transaction, \
                    which is impossible outside a masterchain block",
                    lt,
                    account_addr
                )
            }
            if let Some(acc_tick_tock) = account.get_tick_tock() {
                if !info.tt.is_tock() {
                    if !is_first {
                        reject_query!(
                            "transaction {} of account {:x} is a tick transaction, \
                            but this is not the first transaction of this account",
                            lt,
                            account_addr
                        )
                    }
                    if lt != base.info.start_lt() + 1 {
                        reject_query!(
                            "transaction {} of account {:x} is a tick transaction, but its logical \
                            start time differs from block's start time {} by more than one",
                            lt,
                            account_addr,
                            base.info.start_lt()
                        )
                    }
                    if !acc_tick_tock.tick {
                        reject_query!(
                            "transaction {} of account {:x} is a tick transaction, \
                            but this account has not enabled tick transactions",
                            lt,
                            account_addr
                        )
                    }
                } else {
                    if !is_last {
                        reject_query!(
                            "transaction {} of account {:x} is a tock transaction, \
                            but this is not the last transaction of this account",
                            lt,
                            account_addr
                        )
                    }
                    if !acc_tick_tock.tock {
                        reject_query!(
                            "transaction {} of account {:x} is a tock transaction, \
                            but this account has not enabled tock transactions",
                            lt,
                            account_addr
                        )
                    }
                }
            } else {
                reject_query!(
                    "transaction {} of account {:x} is a tick-tock transaction, \
                    but this account is not listed as special",
                    lt,
                    account_addr
                )
            }
        }
        // check if account has tick_tock attribute
        if let Some(acc_tick_tock) = account.get_tick_tock() {
            let tick_tock = match descr {
                TransactionDescr::TickTock(ref info) => Some(info.tt.clone()),
                _ => None,
            };
            if is_first
                && base.shard().is_masterchain()
                && acc_tick_tock.tick
                && !tick_tock.as_ref().map(|tt| tt.is_tick()).unwrap_or_default()
                && !account_create
            {
                reject_query!(
                    "transaction {} of account {:x} is the first transaction \
                    for this special tick account in this block, \
                    but the transaction is not a tick transaction",
                    lt,
                    account_addr
                )
            }
            if is_last
                && base.shard().is_masterchain()
                && acc_tick_tock.tock
                && !tick_tock.as_ref().map(|tt| tt.is_tock()).unwrap_or_default()
                && trans.end_status == AccountStatus::AccStateActive
            {
                reject_query!(
                    "transaction {} of account {:x} is the last transaction \
                    for this special tock account in this block, \
                    but the transaction is not a tock transaction",
                    lt,
                    account_addr
                )
            }
        }
        if let TransactionDescr::Storage(_) = descr {
            if !is_first {
                reject_query!(
                    "transaction {} of account {:x} is a storage transaction, \
                    but it is not the first transaction for this account in this block",
                    lt,
                    account_addr
                )
            }
        }
        // check that the original account state has correct hash
        let state_update = trans.read_state_update()?;
        let old_hash = account_root.repr_hash().clone();
        if state_update.old_hash != old_hash {
            reject_query!(
                "transaction {} of account {:x} claims that the original \
                account state hash must be {:x} but the actual value is {:x}",
                lt,
                account_addr,
                state_update.old_hash,
                old_hash
            )
        }
        #[cfg(test)]
        let block_version = config.block_version();
        let dict_hash_min_cells = config.size_limits_config().acc_state_cells_for_storage_dict;
        let params = ExecuteParams {
            state_libs: libraries.inner(),
            block_unixtime: base.now(),
            block_lt: base.info.start_lt(),
            last_tr_lt: lt,
            seed_block: base.extra.rand_seed().clone(),
            debug: false,
            prev_blocks_info: base.prev_blocks_info.clone(),
            ..ExecuteParams::default()
        };
        let executor: Box<dyn TransactionExecutor> = match descr {
            TransactionDescr::Ordinary(_) => {
                if in_msg_cell.is_none() {
                    reject_query!(
                        "ordinary transaction {} of account {:x} has no inbound message",
                        lt,
                        account_addr
                    )
                }
                Box::new(OrdinaryTransactionExecutor::new(config))
            }
            TransactionDescr::Storage(_) => {
                if in_msg_cell.is_some() {
                    reject_query!(
                        "storage transaction {} of account {:x} has an inbound message",
                        lt,
                        account_addr
                    )
                }
                if trans.msg_count() != 0 {
                    reject_query!(
                        "storage transaction {} of account {:x} has at least one outbound message",
                        lt,
                        account_addr
                    )
                }
                // FIXME
                reject_query!(
                    "unable to verify storage transaction {} of account {:x}",
                    lt,
                    account_addr
                )
            }
            TransactionDescr::TickTock(ref info) => {
                if in_msg_cell.is_some() {
                    reject_query!(
                        "{} transaction {} of account {:x} has an inbound message",
                        if info.tt.is_tock() { "tock" } else { "tick" },
                        lt,
                        account_addr
                    )
                }
                Box::new(TickTockTransactionExecutor::new(config, info.tt.clone()))
            }
            TransactionDescr::MergePrepare(_) => {
                if in_msg_cell.is_some() {
                    reject_query!(
                        "merge prepare transaction {} of account {:x} has an inbound message",
                        lt,
                        account_addr
                    )
                }
                if trans.msg_count() != 1 {
                    reject_query!(
                        "merge prepare transaction {} of account {:x} must have \
                        exactly one outbound message",
                        lt,
                        account_addr
                    )
                }
                // FIXME
                reject_query!(
                    "unable to verify merge prepare transaction {} of account {:x}",
                    lt,
                    account_addr
                )
            }
            TransactionDescr::MergeInstall(_) => {
                if in_msg_cell.is_none() {
                    reject_query!(
                        "merge install transaction {} of account {:x} has no inbound message",
                        lt,
                        account_addr
                    )
                }
                // FIXME
                reject_query!(
                    "unable to verify merge install transaction {} of account {:x}",
                    lt,
                    account_addr
                )
            }
            TransactionDescr::SplitPrepare(_) => {
                if in_msg_cell.is_some() {
                    reject_query!(
                        "split prepare transaction {} of account {:x} has an inbound message",
                        lt,
                        account_addr
                    )
                }
                if trans.msg_count() != 1 {
                    reject_query!(
                        "merge prepare transaction {} of account {:x} must have \
                        exactly one outbound message",
                        lt,
                        account_addr
                    )
                }
                // FIXME
                reject_query!(
                    "unable to verify split prepare transaction {} of account {:x}",
                    lt,
                    account_addr
                )
            }
            TransactionDescr::SplitInstall(_) => {
                if in_msg_cell.is_none() {
                    reject_query!(
                        "split install transaction {} of account {:x} has no inbound message",
                        lt,
                        account_addr
                    )
                }
                // FIXME
                reject_query!(
                    "unable to verify split install transaction {} of account {:x}",
                    lt,
                    account_addr
                )
            }
        };
        #[cfg(test)]
        let old_account_root = account_root.clone();
        #[cfg(test)]
        let mut our_trans = None;
        let mut blackhole_burned = Coins::default();
        let mut error = None;
        match executor.execute_with_params(in_msg_cell, account, params) {
            Ok(mut trans_execute) => {
                *storage_dict = account.calc_storage_stat_dict(dict_hash_min_cells)?;
                #[cfg(test)]
                if block_version < 12 {
                    account.del_storage_stat();
                }
                *account_root = account.serialize()?;
                let new_hash = account_root.repr_hash().clone();
                if state_update.new_hash != new_hash {
                    error = Some(error!(
                        "{:x} transaction {} of {:x} is invalid: it claims that the new \
                        account state hash is {:x} but the re-computed value is {:x}, account: {:?}",
                        trans_hash, lt, account_addr, state_update.new_hash, new_hash, account
                    ));
                    // #[cfg(test)]
                    // {
                    //     // try to compare with last state of account
                    //     let new_shard_acc = base
                    //         .next_state_accounts
                    //         .get_serialized(trans.account_id().clone())?
                    //         .unwrap();
                    //     let new_account = new_shard_acc.read_account()?;
                    //     pretty_assertions::assert_eq!(account, &new_account);
                    // }
                } else if trans.out_msgs != trans_execute.out_msgs {
                    error = Some(error!(
                        "{:x} transaction {} of {:x} is invalid: it has produced a set of \
                        outbound messages different from that listed in the transaction",
                        trans_hash, lt, account_addr,
                    ));
                } else {
                    if let Some(TrComputePhase::Vm(compute_ph)) = descr.compute_phase_ref() {
                        base.gas_used.fetch_add(compute_ph.gas_used.as_u64(), Ordering::Relaxed);
                    }
                    base.transactions_executed.fetch_add(1, Ordering::Relaxed);
                    blackhole_burned = trans_execute.blackhole_burned().clone();
                    if !blackhole_burned.is_zero() {
                        base.result
                            .blackhole_burned
                            .lock()
                            .map_err(|_| error!("blackhole burned accumulator is poisoned"))?
                            .add(&blackhole_burned)?;
                    }

                    // we cannot know prev transaction in executor
                    trans_execute.set_prev_trans_hash(trans.prev_trans_hash().clone());
                    trans_execute.set_prev_trans_lt(trans.prev_trans_lt());
                    trans_execute.write_state_update(&state_update)?;
                    let trans_execute_root = trans_execute.serialize()?;
                    if trans_root != trans_execute_root {
                        error = Some(error!(
                            "re created {trans_hash:x} transaction {lt} doesn't correspond"
                        ));
                    }
                }
                #[cfg(test)]
                {
                    our_trans = Some(trans_execute);
                }
            }
            Err(err) => error = Some(err),
        };
        if error.is_none() {
            // check new balance and value flow
            let mut left_balance = old_balance.clone();
            left_balance.add(&money_imported)?;
            let new_balance = account.balance().cloned().unwrap_or_default();
            let mut right_balance = new_balance.clone();
            right_balance.add(&money_exported)?;
            right_balance.add(trans.total_fees())?;
            right_balance.coins.add(&blackhole_burned)?;
            if left_balance != right_balance {
                error = Some(error!(
                    "transaction {} of {:x} violates the currency flow condition: \
                    old balance={} + imported={} does not equal new balance={} + exported=\
                    {} + total_fees={} + burned={}",
                    lt,
                    account_addr,
                    old_balance.coins,
                    money_imported.coins,
                    new_balance.coins,
                    money_exported.coins,
                    trans.total_fees().coins,
                    blackhole_burned
                ));
            }
        }
        if let Some(err) = error {
            #[cfg(test)]
            if !base.full_collated_data {
                let _ = Self::prepare_transaction_for_log(
                    &old_account_root,
                    account_root,
                    executor.config().raw_config(),
                    &trans,
                    our_trans.as_ref(),
                );
                if let Some(our_trans) = &our_trans {
                    if let Err(compare) = compare_transactions(our_trans, &trans, true) {
                        Self::prepare_transaction_for_test(
                            base,
                            old_account_root,
                            &trans,
                            err.to_string(),
                        )?;
                        panic!("{} {:?} {:?}", base.block_id(), err, compare);
                    }
                    // pretty_assertions::assert_eq!(trans.read_description()?, our_trans.read_description()?);
                    // trans.out_msgs.scan_diff(&our_trans.out_msgs, |_key: UInt15, msg1, msg2| {
                    //     pretty_assertions::assert_eq!(msg1, msg2);
                    //     Ok(true)
                    // })?;
                    // pretty_assertions::assert_eq!(trans, our_trans);
                }
            }

            return Err(err);
        }
        Ok(true)
    }

    // NB: may be run in parallel for different accounts
    fn check_account_transactions(
        engine: Arc<dyn EngineOperations>,
        base: &ValidateBase,
        config: BlockchainConfig,
        libraries: Libraries,
        account_addr: &AccountId,
        account_root: &mut Cell,
        account: Account,
        acc_block: AccountBlock,
    ) -> Result<()> {
        // CHECK!(account.get_id(), Some(account_addr.as_slice().into()));
        let Some((min_trans_lt, _)) = acc_block.transactions().get_min(false)? else {
            reject_query!("no minimal transaction")
        };
        let Some((max_trans_lt, _)) = acc_block.transactions().get_max(false)? else {
            reject_query!("no maximal transaction")
        };
        let mut new_account = account.clone();
        let mut storage_dict = if let Some(dict_hash) = new_account.dict_hash() {
            if !base.block_id().is_masterchain() && base.full_collated_data {
                base.storage_dict_proofs.get(&dict_hash).cloned()
            } else {
                engine.get_account_storage_dict(dict_hash)
            }
        } else {
            None
        };
        if let Some(dict) = &storage_dict {
            log::debug!(
                target: "validate_query",
                "Using storage dict with hash {:x} for account {:x}",
                dict.repr_hash(),
                account_addr
            );
            new_account.import_storage_stat_dict(dict.clone())?;
        }
        acc_block
            .transactions()
            .iterate_slices_with_keys(|lt, trans| {
                Self::check_one_transaction(
                    base,
                    config.clone(),
                    libraries.clone(),
                    &mut new_account,
                    account_addr,
                    account_root,
                    &mut storage_dict,
                    lt,
                    trans.reference(0)?,
                    lt == min_trans_lt,
                    lt == max_trans_lt,
                )
            })
            .map_err(|err| {
                error!("at least one Transaction of account {account_addr:x} is invalid : {err}",)
            })?;
        if let Some(dict) = storage_dict {
            if new_account.dict_hash().is_some() {
                let size = new_account.storage_info_cells();
                log::trace!(
                    target: "validate_query",
                    "{}: updated storage dict with hash {:x} for account {account_addr:x} of size {size}",
                    base.next_block_descr,
                    dict.repr_hash(),
                );
                engine.add_account_storage_dict(dict, size);
            }
        }

        if base.shard().is_masterchain() {
            Self::scan_account_libraries(
                base,
                account.libraries(),
                new_account.libraries(),
                account_addr,
            )
        } else {
            Ok(())
        }
    }

    fn check_transactions(
        engine: Arc<dyn EngineOperations>,
        base: Arc<ValidateBase>,
        libraries: Libraries,
        tasks: &mut TasksVec,
    ) -> Result<()> {
        // log::debug!(target: "validate_query", "({}): checking all transactions", base.next_block_descr);
        let (capabilities, block_version) =
            base.info.gen_software().map_or((0, 0), |v| (v.capabilities, v.version));
        let config =
            BlockchainConfig::with_params(capabilities, block_version, base.config_params.clone())?;
        base.account_blocks.iterate_with_keys_and_aug(|account_addr, acc_block, _fee| {
            let base = base.clone();
            let libraries = libraries.clone();
            let (mut account_root, account) = Self::unpack_account(&base, &account_addr)?;
            let config = config.clone();
            let engine = engine.clone();
            Self::add_task(tasks, move || {
                Self::check_account_transactions(
                    engine,
                    &base,
                    config,
                    libraries,
                    &account_addr,
                    &mut account_root,
                    account,
                    acc_block,
                )
            });
            Ok(true)
        })?;
        Ok(())
    }

    // similar to Collator::update_account_public_libraries()
    fn scan_account_libraries(
        base: &ValidateBase,
        orig_libs: StateInitLib,
        final_libs: StateInitLib,
        addr: &AccountId,
    ) -> Result<()> {
        orig_libs
            .scan_diff(&final_libs, |key: UInt256, old, new| {
                let f = old.map_or(false, |descr| descr.is_public_library());
                let g = new.map_or(false, |descr| descr.is_public_library());
                if f != g {
                    base.result.lib_publishers.push((key, addr.clone(), g));
                }
                Ok(true)
            })
            .map_err(|err| {
                error!("error scanning old and new libraries of account {:x} : {}", addr, err)
            })?;
        Ok(())
    }

    fn check_all_ticktock_processed(base: &ValidateBase) -> Result<()> {
        if !base.shard().is_masterchain() {
            return Ok(());
        }
        log::debug!(
            target: "validate_query",
            "({}): getting the list of special tick-tock smart contracts",
            base.next_block_descr
        );
        let ticktock_smcs =
            base.config_params.special_ticktock_smartcontracts(3, &base.prev_state_accounts)?;
        log::debug!(
            target: "validate_query",
            "({}): have {} tick-tock smart contracts",
            base.next_block_descr,
            ticktock_smcs.len()
        );
        for (addr, tick_tock) in ticktock_smcs {
            log::debug!(
                target: "validate_query",
                "({}): special smart contract {addr:x} with ticktock={tick_tock}",
                base.next_block_descr,
            );
            if base.account_blocks.get(&addr)?.is_none() {
                reject_query!(
                    "there are no transactions (and in particular, no tick-tock transactions) \
                    for special smart contract {addr:x} with ticktock={tick_tock} in this block"
                )
            }
        }
        Ok(())
    }

    fn check_message_processing_order(base: &mut ValidateBase) -> Result<()> {
        let mut msg_proc_lt = mem::take(&mut base.result.msg_proc_lt).collect::<Vec<_>>();
        msg_proc_lt.sort();
        let iter = msg_proc_lt.iter().zip(msg_proc_lt.iter().skip(1));
        for ((key, lt, emitted_lt), (next_key, next_lt, next_emitted_lt)) in iter {
            if key == next_key && emitted_lt > next_emitted_lt {
                reject_query!(
                    "incorrect message processing order for sender {key:x}: \
                    transaction {lt} processes message created at logical time {emitted_lt},
                    but a later transaction {next_lt} processes an earlier message \
                    created at logical time {next_emitted_lt}"
                )
            }
        }

        let mut msg_emitted_lt = mem::take(&mut base.result.msg_emitted_lt).collect::<Vec<_>>();
        msg_emitted_lt.sort();
        // log::info!(
        //     target: "validate_query",
        //     "msg_emitted_lt: {}",
        //     msg_emitted_lt.iter().map(|(key, lt, emitted_lt)| format!("{key:x} lt:{lt} emitted_lt:{emitted_lt}")).collect::<Vec<String>>().join("\n")
        // );
        let iter = msg_emitted_lt.iter().zip(msg_emitted_lt.iter().skip(1));
        for ((key, lt, emitted_lt), (next_key, next_lt, next_emitted_lt)) in iter {
            if key == next_key && emitted_lt >= next_emitted_lt {
                reject_query!(
                    "incorrect deferred message processing order for sender {key:x}: \
                    message with created_lt {lt} has emitted_lt {emitted_lt}, but \
                    message with created_lt {next_lt} has emitted_lt {next_emitted_lt}"
                )
            }
        }
        Ok(())
    }

    fn check_special_message(
        base: &ValidateBase,
        in_msg: Option<&InMsg>,
        amount: &CurrencyCollection,
        addr: AccountId,
    ) -> Result<()> {
        let in_msg = match in_msg {
            Some(in_msg) => in_msg,
            None if amount.is_zero()? => return Ok(()),
            None => reject_query!("no special message, but amount is {}", amount),
        };
        CHECK!(!amount.is_zero()?);
        if !base.shard().is_masterchain() {
            reject_query!("special messages can be present in masterchain only")
        }
        let env = match in_msg {
            InMsg::Immediate(in_msg) => in_msg.read_envelope_message()?,
            _ => reject_query!("wrong type of special message"),
        };
        let msg_hash = env.message_hash();
        log::debug!(
            target: "validate_query",
            "({}): checking special message with hash {msg_hash:x} and expected amount {amount}",
            base.next_block_descr,
        );
        let Some(msg) = base.in_msg_descr.get(&msg_hash)? else {
            reject_query!(
                "InMsg of special message with hash {msg_hash:x} is not registered in InMsgDescr"
            )
        };
        if &msg != in_msg {
            reject_query!(
                "InMsg of special message with hash {msg_hash:x} differs \
                from the InMsgDescr entry with this key"
            )
        }

        let msg = env.read_message()?;
        // CHECK!(tlb::unpack(cs, info));  // this has been already checked for all InMsgDescr
        let Some(header) = msg.int_header() else {
            reject_query!("InMsg of special message with hash {msg_hash:x} has wrong header")
        };
        if header.src.rewrite_pfx().is_some() {
            reject_query!(
                "source address of message {msg_hash:x} contains anycast info - it is not supported"
            )
        }
        if header.dst.rewrite_pfx().is_some() {
            reject_query!(
                "destination address of message {msg_hash:x} contains anycast info - it is not supported"
            )
        }
        let Some(src) = header.src_ref() else {
            reject_query!("source address of message {msg_hash:x} is wrong")
        };
        let src_prefix = AccountIdPrefixFull::checked_prefix(src)?;
        let dest_prefix = AccountIdPrefixFull::checked_prefix(&header.dst)?;
        let cur_prefix = src_prefix.interpolate_addr_intermediate(&dest_prefix, env.cur_addr())?;
        let next_prefix =
            src_prefix.interpolate_addr_intermediate(&dest_prefix, env.next_addr())?;
        if cur_prefix != dest_prefix || next_prefix != dest_prefix {
            reject_query!(
                "special message with hash {msg_hash:x} \
                has not been routed to its final destination",
            )
        }
        if !base.shard().contains_full_prefix(&src_prefix) {
            reject_query!(
                "special message with hash {msg_hash:x} \
                has source address {src_prefix}... outside this shard",
            )
        }
        if !base.shard().contains_full_prefix(&dest_prefix) {
            reject_query!(
                "special message with hash {msg_hash:x} \
                has destination address {dest_prefix}... outside this shard"
            )
        }
        if !env.fwd_fee_remaining().is_zero() {
            reject_query!(
                "special message with hash {msg_hash:x} \
                has a non-zero fwd_fee_remaining",
            );
        }
        if !header.fwd_fee.is_zero() {
            reject_query!("special message with hash {:x} has a non-zero fwd_fee", msg_hash);
        }
        if !header.extra_flags.is_zero() {
            reject_query!("special message with hash {msg_hash:x} has non-zero extra_flags",)
        }
        if &header.value != amount {
            reject_query!(
                "special message with hash {msg_hash:x} carries an incorrect amount {} \
                instead of {amount} postulated by ValueFlow",
                header.value,
            )
        }
        let (src, dst) = match (&header.src_ref(), &header.dst) {
            (Some(MsgAddressInt::AddrStd(src)), MsgAddressInt::AddrStd(dst)) => {
                (src, dst)
            }
            _ => reject_query!(
                "cannot unpack source and destination addresses of special message with hash {msg_hash:x}",
            ),
        };
        if src.workchain_id as i32 != MASTERCHAIN_ID
            || !src.address.contains_bytes(UInt256::ZERO.as_slice())
        {
            reject_query!(
                "special message with hash {msg_hash:x} has a non-zero source address {}:{:x}",
                src.workchain_id,
                src.address
            )
        }
        // CHECK!(dest_wc == masterchainId);
        // CHECK!(vm::load_cell_slice(addr_cell).prefetch_bits_to(correct_addr));
        if addr != dst.address {
            reject_query!(
                "special message with hash {msg_hash:x} has destination address -1:{:x} \
                but the correct address defined by the configuration is {addr:x}",
                dst.address,
            )
        }

        if msg.has_body() {
            reject_query!("special message with hash {:x} has a non-empty body", msg_hash)
        }
        Ok(())
    }

    fn check_special_messages(base: &ValidateBase) -> Result<()> {
        Self::check_special_message(
            base,
            base.recover_create_msg.as_ref(),
            &base.value_flow.recovered,
            base.config_params.fee_collector_address()?,
        )?;
        Self::check_special_message(
            base,
            base.mint_msg.as_ref(),
            &base.value_flow.minted,
            base.config_params.minter_address()?,
        )?;
        Ok(())
    }

    fn check_one_library_update(
        key: UInt256,
        old: Option<LibDescr>,
        new: Option<LibDescr>,
        lib_publishers2: &mut Vec<LibPublisher>,
    ) -> Result<bool> {
        let new = match new {
            Some(new) => {
                if *new.lib().repr_hash() != key {
                    reject_query!(
                        "LibDescr with key {:x} in the libraries dictionary of the new state \
                        contains a library with different root hash {:x}",
                        key,
                        new.lib().repr_hash()
                    )
                }
                new
            }
            None => LibDescr::default(),
        };
        let old = old.unwrap_or_default();
        old.publishers()
            .scan_diff(new.publishers(), |publisher: AccountId, _old, new| {
                lib_publishers2.push((key.clone(), publisher, new.is_some()));
                Ok(true)
            })
            .map_err(|err| {
                error!("invalid publishers set for shard library with hash {key:x} : {err}")
            })
    }

    fn check_shard_libraries(base: &mut ValidateBase) -> Result<()> {
        let mut lib_publishers2 = vec![];
        let old = base.prev_state()?.libraries().clone();
        let new = base.next_state()?.libraries().clone();
        old.scan_diff(&new, |key: UInt256, old, new| {
            Self::check_one_library_update(key, old, new, &mut lib_publishers2)
        })
        .map_err(|err| error!("invalid shard libraries dictionary in the new state : {}", err))?;

        let mut lib_publishers = mem::take(&mut base.result.lib_publishers).collect::<Vec<_>>();
        lib_publishers.sort();
        lib_publishers2.sort();
        if lib_publishers != lib_publishers2 {
            // TODO: better error message with by-element comparison?
            reject_query!(
                "the set of public libraries and their publishing accounts \
                has not been updated correctly"
            )
        }
        Ok(())
    }

    fn check_new_state(
        base: &mut ValidateBase,
        mc_data: &McData,
        manager: &MsgQueueManager,
    ) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): checking header of the new shardchain state",
            base.next_block_descr
        );
        let prev_state = base.prev_state()?;
        let next_state = base.next_state()?;
        let my_mc_seqno = if base.shard().is_masterchain() {
            base.block_id().seq_no()
        } else {
            mc_data.state.block_id().seq_no()
        };
        let min_seq_no = match manager.next().min_seqno() {
            0 => u32::MAX,
            min_seq_no => min_seq_no,
        };
        let ref_mc_seqno = min_seq_no.min(my_mc_seqno).min(base.min_shard_ref_mc_seqno());
        if next_state.min_ref_mc_seqno() != ref_mc_seqno {
            reject_query!(
                "new state of {} has minimal referenced masterchain block seqno {} \
                but the value computed from all shard references and previous masterchain \
                block reference is {} = min({},{},{})",
                base.block_id(),
                next_state.min_ref_mc_seqno(),
                ref_mc_seqno,
                my_mc_seqno,
                base.min_shard_ref_mc_seqno(),
                min_seq_no
            )
        }
        // before_split:(## 1) -> checked in unpack_next_state()
        // accounts:^ShardAccounts -> checked in precheck_account_updates() + other
        // ^[ overload_history:uint64 underload_history:uint64
        if next_state.overload_history() & next_state.underload_history() & 1 != 0 {
            reject_query!(
                "lower-order bits both set in the new state's overload_history \
                and underload history (block cannot be both overloaded and underloaded)"
            )
        }
        if base.after_split || base.after_merge {
            if (next_state.overload_history() | next_state.underload_history()) & !1 != 0 {
                reject_query!(
                    "new block is immediately after split or after merge, \
                    but the old underload or overload history has not been cleared"
                )
            }
        } else {
            if (next_state.overload_history() ^ (prev_state.overload_history() << 1)) & !1 != 0 {
                reject_query!(
                    "new overload history {} is not compatible \
                    with the old overload history {}",
                    next_state.overload_history(),
                    prev_state.overload_history()
                )
            }
            if (next_state.underload_history() ^ (prev_state.underload_history() << 1)) & !1 != 0 {
                reject_query!(
                    "new underload history {}  is not compatible \
                    with the old underload history {}",
                    next_state.underload_history(),
                    prev_state.underload_history()
                )
            }
        }
        if next_state.total_balance() != &base.value_flow.to_next_blk {
            reject_query!(
                "new state declares total balance {} different from to_next_blk in value flow \
                (obtained by summing balances of all accounts in the new state): {}",
                next_state.total_balance(),
                base.value_flow.to_next_blk
            )
        }
        log::debug!(
            target: "validate_query",
            "({}): checking total validator fees: new={}+recovered={} == old={}+collected={}",
            base.next_block_descr,
            next_state.total_validator_fees(), base.value_flow.recovered,
            base.prev_validator_fees, base.value_flow.fees_collected
        );
        let mut new = base.value_flow.recovered.clone();
        new.add(next_state.total_validator_fees())?;
        let mut old = base.value_flow.fees_collected.clone();
        old.add(&base.prev_validator_fees)?;
        if new != old {
            reject_query!(
                "new state declares total validator fees {} not equal to the sum of \
                old total validator fees {} and the fees collected in this block {} \
                minus the recovered fees {}",
                next_state.total_validator_fees(),
                &base.prev_validator_fees,
                base.value_flow.fees_collected,
                base.value_flow.recovered
            )
        }
        // libraries:(HashmapE 256 LibDescr)
        if base.shard().is_masterchain() {
            Self::check_shard_libraries(base).map_err(|err| {
                error!("the set of public libraries in the new state is invalid : {}", err)
            })?;
        }
        let next_state = base.next_state()?;
        if !base.shard().is_masterchain() && !next_state.libraries().is_empty() {
            reject_query!(
                "new state contains a non-empty public library collection, \
                which is not allowed for non-masterchain blocks"
            )
        }
        // TODO: it seems was tested in unpack_next_state
        if base.shard().is_masterchain() && next_state.master_ref().is_some() {
            reject_query!("new state contains a masterchain block reference (master_ref)")
        } else if !base.shard().is_masterchain() && next_state.master_ref().is_none() {
            reject_query!("new state does not contain a masterchain block reference (master_ref)")
        }

        // custom:(Maybe ^McStateExtra) -> checked in check_mc_state_extra()
        // = ShardStateUnsplit;
        Ok(())
    }

    fn check_config_update(base: &ValidateBase) -> Result<()> {
        if base.next_state_extra.config.config_params.count(10000).is_err() {
            reject_query!("new configuration failed to pass letmated validity checks")
        }
        if base.prev_state_extra.config.config_params.count(10000).is_err() {
            reject_query!("old configuration failed to pass letmated validity checks")
        }
        if !base.next_state_extra.config.valid_config_data(false, None)? {
            reject_query!(
                "new configuration parameters failed to pass per-parameter letmated validity \
                checks, or one of mandatory configuration parameters is missing"
            )
        }
        let new_accounts = base.next_state()?.read_accounts()?;
        let config_address = base.prev_state_extra.config.config_address()?;
        let new_config_address = base.next_state_extra.config.config_address()?;
        let cfg_acc_changed = config_address != new_config_address;
        let config_address = config_address.into();
        let old_config_root = match new_accounts.get(&config_address)? {
            Some(account) => account.read_account()?.get_data(),
            None => reject_query!(
                "cannot extract configuration from the new state of the (old) configuration \
                smart contract {config_address:x}"
            ),
        };
        let new_config_root = match new_accounts.get(&config_address)? {
            Some(account) => account.read_account()?.get_data(),
            None => reject_query!(
                "cannot extract configuration from the new state of the (new) configuration \
                smart contract {new_config_address:x}"
            ),
        };
        if old_config_root != new_config_root {
            reject_query!(
                "the new configuration is different from that stored in the persistent data of the \
                (new) configuration smart contract {config_address:x}"
            )
        }
        let old_config = base.prev_state_extra.config.clone();
        let new_config = base.next_state_extra.config.clone();
        if !old_config.valid_config_data(true, None)? {
            reject_query!(
                "configuration extracted from (old) configuration smart contract {:x} failed to \
                pass per-parameter validity checks, or one of mandatory parameters is missing",
                old_config.config_addr
            )
        }
        if new_config.important_config_parameters_changed(&old_config, false)? {
            // same as the check in Collator::create_mc_state_extra()
            log::warn!(
                target: "validate_query",
                "({}): the global configuration changes in block {}",
                base.next_block_descr,
                base.block_id()
            );
            if !base.info.key_block() {
                reject_query!(
                    "important parameters in the global configuration have changed, \
                    but the block is not marked as a key block"
                )
            }
        } else if base.info.key_block()
            && !(cfg_acc_changed
                || new_config.important_config_parameters_changed(&old_config, true)?)
        {
            reject_query!(
                "no important parameters have been changed, but the block is marked as a key block"
            )
        }
        let want_cfg_addr = match old_config.config(0)? {
            Some(ConfigParamEnum::ConfigParam0(param)) => param.config_addr,
            _ => {
                if cfg_acc_changed {
                    reject_query!(
                        "new state of old configuration smart contract {:x} contains no value \
                        for parameter 0 (new configuration smart contract address), but the \
                        configuration smart contract has been somehow changed to {:x}",
                        old_config.config_addr,
                        new_config.config_addr
                    )
                } else {
                    return Ok(());
                }
            }
        };
        if want_cfg_addr == config_address {
            if cfg_acc_changed {
                reject_query!(
                    "new state of old configuration smart contract {:x} contains the same value \
                    for parameter 0 (configuration smart contract address), but the \
                    configuration smart contract has been somehow changed to {:x}",
                    old_config.config_addr,
                    new_config.config_addr
                )
            }
            return Ok(());
        }
        if want_cfg_addr != new_config.config_addr && cfg_acc_changed {
            reject_query!(
                "new state of old configuration smart contract {:x} contains {:x} \
                as the value for parameter 0 (new configuration smart contract address), but the \
                configuration smart contract has been somehow changed to a different value {:x}",
                old_config.config_addr,
                want_cfg_addr,
                new_config.config_addr
            )
        }
        // now old_cfg_addr = new_cfg_addr != want_cfg_addr
        // the configuration smart contract has not been switched to want_cfg_addr, have to check why
        let want_config_root = match new_accounts.get(&want_cfg_addr.clone().into()) {
            Ok(Some(account)) => account.read_account()?.get_data(),
            _ => {
                log::warn!(
                    target: "validate_query",
                    "({}): switching of configuration smart contract did not happen \
                    because the suggested new configuration smart contract {want_cfg_addr:x} does not \
                    contain a valid configuration",
                    base.next_block_descr,
                );
                return Ok(());
            }
        };
        // if !base.config_params.valid_config_data(&base.prev_state_extra.config, true, false)? {
        let want_config =
            ConfigParams::with_address_and_params(want_cfg_addr.clone(), want_config_root);
        if !want_config.valid_config_data(false, None)? {
            log::warn!(
                target: "validate_query",
                "({}): switching of configuration smart contract did not happen because \
                the configuration extracted from suggested new configuration smart contract {want_cfg_addr:x} \
                failed to pass per-parameter validity checks, or one of \
                mandatory configuration parameters is missing",
                base.next_block_descr,
            );
            return Ok(());
        }
        reject_query!(
            "old configuration smart contract {:x} suggested {:x} as the new configuration \
            smart contract, but the switchover did not happen without a good reason \
            (the suggested configuration appears to be valid)",
            old_config.config_addr,
            want_cfg_addr
        )
    }

    fn check_one_prev_dict_update(
        base: &ValidateBase,
        mc_data: &McData,
        seq_no: u32,
        old: Option<(KeyExtBlkRef, KeyMaxLt)>,
        new: Option<(KeyExtBlkRef, KeyMaxLt)>,
    ) -> Result<bool> {
        let new_val = if old.is_some() {
            if new.is_some() {
                reject_query!(
                    "entry with seqno {} changed in the new previous blocks dictionary as compared \
                    to its old version (entries should never change once they have been added)",
                    seq_no
                )
            } else {
                // if this becomes allowed in some situations, then check necessary conditions and return true
                reject_query!(
                    "entry with seqno {} disappeared in the new previous blocks dictionary as \
                    compared to the old previous blocks dictionary",
                    seq_no
                )
            }
        } else if seq_no != mc_data.state.state()?.seq_no() {
            reject_query!(
                "new previous blocks dictionary contains a new entry with seqno {} \
                while the only new entry must be for the previous block with seqno {}",
                seq_no,
                mc_data.state.state()?.seq_no()
            )
        } else {
            new.expect("check scan_diff for Hashmap").0
        };
        log::debug!(
            target: "validate_query",
            "({}): prev block id for {} is present",
            base.next_block_descr,
            seq_no
        );
        let (end_lt, block_id, key) = new_val.master_block_id();
        if block_id.seq_no != seq_no {
            reject_query!(
                "new previous blocks dictionary entry with seqno {} in fact describes a block {} \
                with different seqno",
                seq_no,
                block_id
            )
        }
        if block_id != base.prev_blocks_ids[0] {
            reject_query!(
                "new previous blocks dictionary has a new entry for previous block {} while \
                the correct previous block is {}",
                block_id,
                base.prev_blocks_ids[0]
            )
        }
        if end_lt != mc_data.state.state()?.gen_lt() {
            reject_query!(
                "previous blocks dictionary has new entry for previous block {} \
                indicating end_lt={} but the correct value is {}",
                block_id,
                end_lt,
                mc_data.state.state()?.gen_lt()
            )
        }
        if key != mc_data.mc_state_extra.after_key_block {
            reject_query!(
                "previous blocks dictionary has new entry for previous block {} indicating \
                is_key_block={} but the correct value is {}",
                block_id,
                key,
                mc_data.mc_state_extra.after_key_block
            )
        }
        Ok(true)
    }

    // somewhat similar to Collator::create_mc_state_extra()
    fn check_mc_state_extra(&self, base: &ValidateBase, mc_data: &McData) -> Result<()> {
        let prev_state = base.prev_state()?;
        let next_state = base.next_state()?;
        if !base.shard().is_masterchain() {
            if next_state.custom_cell().is_some() {
                reject_query!(
                    "new state defined by non-masterchain block {} contains a McStateExtra",
                    base.block_id()
                )
            }
            return Ok(Default::default());
        }
        let import_created = base.mc_extra.fees().root_extra().create.clone();
        log::debug!(
            target: "validate_query",
            "({}): checking header of McStateExtra in the new masterchain state",
            base.next_block_descr
        );
        if prev_state.custom_cell().is_none() {
            reject_query!("previous masterchain state did not contain a McStateExtra")
        }
        if next_state.custom_cell().is_none() {
            reject_query!("new masterchain state does not contain a McStateExtra")
        }
        // masterchain_state_extra#cc26
        // shard_hashes:ShardHashes has been checked separately
        // config:ConfigParams
        Self::check_config_update(base)?;
        // ...
        if base.next_state_extra.block_create_stats.is_some() != self.create_stats_enabled {
            reject_query!(
                "new McStateExtra has block_create_stats, \
                but active configuration defines create_stats_enabled={}",
                self.create_stats_enabled
            )
        }
        // validator_info:ValidatorInfo
        // (already checked in check_mc_validator_info())
        // prev_blocks_ids:OldMcBlocksInfo
        // comment this temporary if long test
        base.prev_state_extra
            .prev_blocks
            .scan_diff_with_aug(&base.next_state_extra.prev_blocks, |seq_no: u32, old, new| {
                Self::check_one_prev_dict_update(base, mc_data, seq_no, old, new)
            })
            .map_err(|err| {
                error!("invalid previous block dictionary in the new state : {}", err)
            })?;
        if let Some((seq_no, _)) = base.prev_state_extra.prev_blocks.get_max(false)? {
            if seq_no >= mc_data.state.state()?.seq_no() {
                reject_query!(
                    "previous block dictionary for the previous state with seqno {} \
                    contains information about 'previous' masterchain block with seqno {}",
                    mc_data.state.state()?.seq_no(),
                    seq_no
                )
            }
        }
        let Some((seq_no, _)) = base.next_state_extra.prev_blocks.get_max(false)? else {
            reject_query!(
                "new previous blocks dictionary is empty \
                (at least the immediately previous block should be there)"
            )
        };
        CHECK!(base.block_id().seq_no == mc_data.state.block_id().seq_no() + 1);
        if seq_no > mc_data.state.state()?.seq_no() {
            reject_query!(
                "previous block dictionary for the new state with seqno {} \
                contains information about a future masterchain block with seqno {}",
                base.block_id().seq_no,
                seq_no
            )
        }
        if seq_no != mc_data.state.state()?.seq_no() {
            reject_query!(
                "previous block dictionary for the new state of masterchain block {} \
                does not contain information about immediately previous block with seqno {}",
                base.block_id(),
                mc_data.state.state()?.seq_no()
            )
        }
        // after_key_block:Bool
        if base.next_state_extra.after_key_block != base.info.key_block() {
            reject_query!(
                "new McStateExtra has after_key_block={} \
                while the block header claims is_master_state={}",
                base.next_state_extra.after_key_block,
                base.info.key_block()
            )
        }
        if base.prev_state_extra.last_key_block.is_some()
            && base.next_state_extra.last_key_block.is_none()
        {
            reject_query!(
                "old McStateExtra had a non-trivial last_key_block, but the new one does not"
            )
        }
        if base.next_state_extra.last_key_block == base.prev_state_extra.last_key_block {
            // TODO: check here
            if mc_data.mc_state_extra.after_key_block {
                reject_query!(
                    "last_key_block remains unchanged in the new masterchain state, but \
                    the previous block is a key block (it should become the new last_key_block)"
                )
            }
        } else if base.next_state_extra.last_key_block.is_none() {
            reject_query!(
                "last_key_block:(Maybe ExtBlkRef) changed in the new state, \
                but it became a nothing$0"
            )
        } else if let Some(ref last_key_block) = base.next_state_extra.last_key_block {
            let block_id = BlockIdExt::from_ext_blk(last_key_block.clone());
            if block_id != base.prev_blocks_ids[0]
                || last_key_block.end_lt != mc_data.state.state()?.gen_lt()
            {
                reject_query!(
                    "last_key_block has been set in the new masterchain state to {} with lt {}, \
                    but the only possible value for this update is the previous block {} \
                    with lt {}",
                    block_id,
                    last_key_block.end_lt,
                    base.prev_blocks_ids[0],
                    mc_data.state.state()?.gen_lt()
                )
            }
            if !mc_data.mc_state_extra.after_key_block {
                reject_query!(
                    "last_key_block has been updated to the previous block {}, \
                    but it is not a key block",
                    block_id
                )
            }
        }
        if let Some(block_ref) = base.next_state_extra.last_key_block.clone() {
            let key_block_id = block_ref.master_block_id().1;
            if Some(&key_block_id) != mc_data.prev_key_block() {
                reject_query!(
                    "new masterchain state declares previous key block to be {:?} \
                    but the value computed from previous masterchain state is {:?}",
                    key_block_id,
                    mc_data.prev_key_block()
                )
            }
        } else if let Some(last_key_block) = mc_data.prev_key_block() {
            reject_query!(
                "new masterchain state declares no previous key block, but the block header \
                announces previous key block seqno {}",
                last_key_block.seq_no
            )
        }
        if let Some(new_block_create_stats) = base.next_state_extra.block_create_stats.clone() {
            let old_block_create_stats =
                base.prev_state_extra.block_create_stats.clone().unwrap_or_default();
            if !base.is_fake
                && !self.check_block_create_stats(
                    base,
                    old_block_create_stats,
                    new_block_create_stats,
                )?
            {
                reject_query!("invalid BlockCreateStats update in the new masterchain state")
            }
        }
        let mut expected_global_balance = base.prev_state_extra.global_balance.clone();
        expected_global_balance.add(&base.value_flow.minted)?;
        expected_global_balance.add(&base.value_flow.created)?;
        expected_global_balance.add(&import_created)?;
        if base.next_state_extra.global_balance != expected_global_balance {
            reject_query!(
                "global balance changed in unexpected way: \
                expected old + minted + created + import_created = {} + {} + {} + {} + {}, {}",
                base.prev_state_extra.global_balance,
                base.value_flow.minted,
                base.value_flow.created,
                import_created,
                expected_global_balance,
                base.next_state_extra.global_balance
            )
        }

        // ...
        Ok(())
    }

    fn check_counter_update(
        base: &ValidateBase,
        oc: &Counters,
        nc: &Counters,
        expected_incr: u64,
    ) -> Result<()> {
        let mut cc = oc.clone();
        if nc.is_zero() {
            if expected_incr != 0 {
                reject_query!(
                    "new counter total is zero, but the total should have been increased by {}",
                    expected_incr
                )
            }
            if oc.is_zero() {
                return Ok(());
            }
            cc.increase_by(0, base.now());
            if !cc.almost_zero() {
                reject_query!(
                    "counter has been reset to zero, but it still has non-zero components \
                    after relaxation: {:?}; original value before relaxation was {:?}",
                    cc,
                    oc
                )
            }
            return Ok(());
        }
        if expected_incr == 0 {
            if oc == nc {
                return Ok(());
            } else {
                reject_query!(
                    "unnecessary relaxation of counter from {:?} to {:?} without an increment",
                    oc,
                    nc
                )
            }
        }
        if nc.total() < oc.total() {
            reject_query!(
                "total counter goes back from {} to {} (increment by {} expected instead)",
                oc.total(),
                nc.total(),
                expected_incr
            )
        }
        if nc.total() != oc.total() + expected_incr {
            reject_query!(
                "total counter has been incremented by {}, from {} to {} \
                (increment by {} expected instead)",
                nc.total() - oc.total(),
                oc.total(),
                nc.total(),
                expected_incr
            )
        }
        if !cc.increase_by(expected_incr, base.now()) {
            reject_query!("old counter value {:?} cannot be increased by {}", oc, expected_incr)
        }
        if !cc.almost_equals(nc) {
            reject_query!(
                "counter {:?} has been increased by {} with an incorrect resulting value {:?}; \
                correct result should be {:?} (up to +/-1 in the last two components)",
                oc,
                expected_incr,
                nc,
                cc
            );
        }
        Ok(())
    }

    fn check_one_block_creator_update(
        &self,
        base: &ValidateBase,
        key: &UInt256,
        old: Option<CreatorStats>,
        new: Option<CreatorStats>,
    ) -> Result<bool> {
        log::debug!(
            target: "validate_query",
            "({}): checking update of CreatorStats for {key:x}",
            self.next_block_descr,
        );
        let (new, new_exists) = match new {
            Some(new) => (new, true),
            None => (CreatorStats::default(), false),
        };
        let old = old.unwrap_or_default();
        let (mc_incr, shard_incr) = if key.is_zero() {
            (!base.created_by.is_zero(), self.block_create_total)
        } else {
            (&base.created_by == key, self.block_create_count.get(key).cloned().unwrap_or(0))
        };
        Self::check_counter_update(base, old.mc_blocks(), new.mc_blocks(), mc_incr as u64)
            .map_err(|err| {
                error!(
                    "invalid update of created masterchain blocks \
                    counter in CreatorStats for {key:x} : {err}"
                )
            })?;
        Self::check_counter_update(base, old.shard_blocks(), new.shard_blocks(), shard_incr)
            .map_err(|err| {
                error!(
                    "invalid update of created shardchain blocks \
                    counter in CreatorStats for {key:x} : {err}"
                )
            })?;
        if new.mc_blocks().is_zero() && new.shard_blocks().is_zero() && new_exists {
            reject_query!(
                "new CreatorStats for {key:x} contains two zero counters \
                (it should have been completely deleted instead)"
            )
        }
        Ok(true)
    }

    // similar to Collator::update_block_creator_stats()
    fn check_block_create_stats(
        &self,
        base: &ValidateBase,
        old: BlockCreateStats,
        new: BlockCreateStats,
    ) -> Result<bool> {
        log::debug!(
            target: "validate_query",
            "({}): checking all CreatorStats updates between the old and the new state",
            self.next_block_descr
        );
        old.counters
            .scan_diff(&new.counters, |key: UInt256, old, new| {
                self.check_one_block_creator_update(base, &key, old, new)
            })
            .map_err(|err| {
                error!("invalid BlockCreateStats dictionary in the new state : {}", err)
            })?;
        for key in self.block_create_count.keys() {
            let old_val = old.counters.get(key)?;
            let new_val = new.counters.get(key)?;
            if old_val.is_none() != new_val.is_none() || old_val == new_val {
                continue;
            }
            if !self.check_one_block_creator_update(base, key, old_val, new_val)? {
                reject_query!("invalid update of BlockCreator entry for {:x}", key)
            }
        }
        let key = UInt256::ZERO;
        let old_val = old.counters.get(&key)?;
        let new_val = new.counters.get(&key)?;
        if new_val.is_none() && (!base.created_by.is_zero() || self.block_create_total != 0) {
            reject_query!(
                "new masterchain state does not contain a BlockCreator entry with zero key with \
                total statistics"
            )
        }
        if !self.check_one_block_creator_update(base, &key, old_val, new_val)? {
            reject_query!("invalid update of BlockCreator entry for {:x}", key)
        }
        Ok(true)
    }

    fn check_one_shard_fee(
        base: &ValidateBase,
        shard: &ShardIdent,
        fees: &CurrencyCollection,
        created: &CurrencyCollection,
    ) -> Result<bool> {
        let Some(descr) = base.mc_extra.shards().get_shard(shard)? else {
            reject_query!(
                "ShardFees contains a record for shard {shard} \
                but there is no corresponding record in the new shard configuration",
            )
        };
        if descr.descr.reg_mc_seqno != base.block_id().seq_no {
            reject_query!(
                "ShardFees contains a record for shard {} but the corresponding record in \
                the shard configuration has not been updated by this block",
                shard
            )
        }
        if fees != &descr.descr.fees_collected {
            reject_query!(
                "ShardFees record for shard {} contains fees_collected value {} different \
                from that present in shard configuration {}",
                shard,
                fees,
                descr.descr.fees_collected
            )
        }
        if created != &descr.descr.funds_created {
            reject_query!(
                "ShardFees record for shard {} contains funds_created value {} different \
                from that present in shard configuration {}",
                shard,
                created,
                descr.descr.funds_created
            )
        }
        Ok(true)
    }

    fn check_mc_block_extra(base: &ValidateBase, _mc_data: &McData) -> Result<()> {
        log::debug!(
            target: "validate_query",
            "({}): checking all CreatorStats updates between the old and the new state",
            base.next_block_descr
        );
        if !base.shard().is_masterchain() {
            return Ok(());
        }
        // masterchain_block_extra#cca5
        // key_block:(## 1) -> checked in init_parse()
        // shard_hashes:ShardHashes -> checked in compute_next_state() and check_shard_layout()
        // shard_fees:ShardFees
        base.mc_extra.fees().iterate_slices_with_keys_and_aug(|key, mut created, aug| {
            let created = ShardFeeCreated::construct_from(&mut created)?;
            let shard = ShardIdent::with_tagged_prefix(key.workchain_id, key.prefix)?;
            if created != aug
                || !Self::check_one_shard_fee(base, &shard, &created.fees, &created.create)?
            {
                reject_query!(
                    "ShardFees entry with key {:x} corresponding to shard {} is invalid",
                    key,
                    shard
                )
            }
            Ok(true)
        })?;
        let fees_imported = base.mc_extra.fees().root_extra().fees.clone();
        if fees_imported != base.value_flow.fees_imported {
            reject_query!(
                "invalid fees_imported in value flow: declared {}, correct value is {}",
                base.value_flow.fees_imported,
                fees_imported
            )
        }
        // ^[ prev_blk_signatures:(HashmapE 16 CryptoSignaturePair)
        // prev_blk_signatures is not implemented and should be empty
        if !base.mc_extra.prev_blk_signatures().is_empty() {
            reject_query!("block contains non-empty signature set for the previous block")
        }
        //   recover_create_msg:(Maybe ^InMsg)
        //   mint_msg:(Maybe ^InMsg) ]
        // config:key_block?ConfigParams -> checked in compute_next_state() and ???
        Ok(())
    }

    /*
     *
     *   MAIN VALIDATOR FUNCTION
     *     (invokes other methods in a suitable order)
     *
     */

    async fn common_preparation(&mut self) -> Result<(ValidateBase, McData)> {
        let mut base = self.init_base()?;
        let mc_data = self.init_mc_data(&mut base).await?;
        // stage 0
        self.compute_next_state(&mut base, &mc_data).await?;
        self.unpack_prev_state(&mut base)?;
        self.unpack_next_state(&mut base, &mc_data)?;
        base.prev_blocks_info = PrevBlocksInfo::Raw(
            mc_data.last_mc_block_id(),
            mc_data.mc_state_extra.prev_blocks.clone(),
        );
        Self::load_block_data(&mut base)?;
        Ok((base, mc_data))
    }

    fn add_task(tasks: &mut TasksVec, task: impl FnOnce() -> Result<()> + Send + 'static) {
        tasks.push(Box::new(task))
    }

    async fn run_tasks(&self, tasks: TasksVec) -> Result<()> {
        if self.multithread {
            let tasks = tasks.into_iter().map(|t| tokio::task::spawn_blocking(t));
            futures::future::join_all(tasks)
                .await
                .into_iter()
                .find(|r| match r {
                    Err(_) => true,
                    Ok(Err(_)) => true,
                    Ok(Ok(_)) => false,
                })
                .unwrap_or(Ok(Ok(())))??;
        } else {
            for task in tasks {
                task()?;
            }
        }
        Ok(())
    }

    async fn validate(&mut self) -> Result<ValidateBase> {
        let (mut base, mc_data) = self.common_preparation().await?;

        let manager = self.init_output_queue_manager(&mc_data, &mut base).await?;
        self.check_shard_layout(&base, &mc_data)?;

        #[cfg(feature = "xp25")]
        self.check_ref_shard_blocks(&base, &mc_data).await.map_err(|e| {
            let labels = [("shard", self.shard.to_string())];
            metrics::counter!("ton_node_validator_ref_block_failures_total", &labels).increment(1);
            error!("Error while checking ref blocks: {}", e)
        })?;

        check_cur_validator_set(
            &self.validator_set,
            base.block_id(),
            base.shard(),
            mc_data.mc_state_extra(),
            &self.old_mc_shards,
            mc_data.state(),
            base.is_fake,
        )?;
        self.check_utime_lt(&base, &mc_data)?;
        // stage 1
        // log::debug!(target: "validate_query", "running letmated validity checks for block candidate {}", base.block_id());
        // if (!block::gen::t_Block.validate_ref(1000000, block_root_)) {
        //     reject_query!("block ",  id_ + " failed to pass letmated validity checks");
        // }

        let base = Arc::new(base);
        let manager = Arc::new(manager);
        let mut tasks = vec![];
        let b = base.clone();
        Self::add_task(&mut tasks, move || Self::precheck_value_flow(b));
        let b = base.clone();
        Self::add_task(&mut tasks, move || Self::precheck_account_updates(b));
        Self::precheck_account_transactions(base.clone(), &mut tasks)?;
        Self::precheck_message_queue_update(base.clone(), &manager)?;
        Self::unpack_dispatch_queue_update(base.clone(), &manager)?;

        Self::check_in_msg_descr(base.clone(), manager.clone(), &mut tasks)?;
        Self::check_out_msg_descr(base.clone(), manager.clone(), &mut tasks)?;
        Self::check_transactions(
            self.engine.clone(),
            base.clone(),
            mc_data.libraries()?.clone(),
            &mut tasks,
        )?;

        self.run_tasks(tasks).await?;

        let mut base = Arc::try_unwrap(base).map_err(|base| {
            error!(
                "Somebody haven't released Arc: strong: {} weak: {}",
                Arc::strong_count(&base),
                Arc::weak_count(&base)
            )
        })?;

        Self::check_dispatch_queue_update(&base)?;
        Self::check_processed_upto(&base, &manager, &mc_data)?;
        Self::check_in_queue(&base, &manager)?;
        // Excessive check: validity of message in queue is checked elsewhere
        // Self::check_delivered_dequeued(&base, &manager)?;
        Self::check_all_ticktock_processed(&base)?;
        Self::check_message_processing_order(&mut base)?;
        Self::check_burned_value_flow(&base)?;
        Self::check_new_state(&mut base, &mc_data, &manager)?;
        Self::check_mc_block_extra(&base, &mc_data)?;
        self.check_mc_state_extra(&base, &mc_data)?;
        Self::check_special_messages(&base)?;
        Ok(base)
    }

    /// returns error message in Option if validation could not be determined
    #[allow(dead_code)]
    pub async fn try_verify(self) -> Result<Option<String>> {
        match self.try_validate().await {
            Err(err) => {
                if let Some(NodeError::Timeout(_)) = err.downcast_ref() {
                    Ok(Some(err.to_string()))
                } else {
                    Err(err)
                }
            }
            Ok(_) => Ok(None),
        }
    }

    pub async fn try_validate(mut self) -> Result<Option<Arc<ShardStateStuff>>> {
        let block_id = self.block_candidate.block_id.clone();
        log::info!("({}): VALIDATE {}", self.next_block_descr, block_id);
        let now = Instant::now();

        let result = self.validate().await;
        let duration = now.elapsed().as_millis() as u64;
        let base = match result {
            Err(err) => {
                let add = if let Some(NodeError::Timeout(_)) = err.downcast_ref() {
                    "(skipped) "
                } else {
                    ""
                };
                log::warn!(
                    "({}): VALIDATION FAILED {}{} TIME {}ms ERR {}",
                    self.next_block_descr,
                    add,
                    block_id,
                    duration,
                    err
                );
                return Err(err);
            }
            Ok(base) => base,
        };
        let gas_used = base.gas_used.load(Ordering::Relaxed);
        let ratio = gas_used.checked_div(duration).unwrap_or(gas_used);
        // Candidate age: now - gen_utime_ms (from ConsensusExtraData, simplex only).
        // Reported as "AGE: -" for catchain candidates (no per-block ms timestamp).
        // Negative ages (clock skew) are clamped to 0
        let age_ms_str = match base.now_ms {
            Some(gen_ms) => {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(gen_ms);
                let age = now_ms.saturating_sub(gen_ms);
                format!("{age}ms")
            }
            None => "-".to_string(),
        };
        log::info!(
            "({}): ASYNC VALIDATED {} TIME {}ms AGE: {} GAS_RATE: {}",
            self.next_block_descr,
            base.block_id(),
            duration,
            age_ms_str,
            ratio
        );

        let labels = [("shard", base.block_id().shard().to_string())];
        metrics::histogram!("ton_node_validator_gas_rate_ratio", &labels).record(ratio as f64);

        #[cfg(not(test))]
        #[cfg(feature = "telemetry")]
        self.engine.validator_telemetry().succeeded_attempt(
            &self.shard,
            now.elapsed(),
            base.transactions_executed.load(Ordering::Relaxed),
            gas_used as u32,
        );

        // With CapFullCollatedData the prev state is virtualized from collated_data,
        // so the computed next_state may contain pruned cells.
        if base.full_collated_data {
            Ok(None)
        } else {
            Ok(base.next_state)
        }
    }
}

#[cfg(test)]
impl ValidateQuery {
    fn prepare_transaction_for_test(
        base: &ValidateBase,
        account_root: Cell,
        trans: &Transaction,
        err: String,
    ) -> Result<()> {
        let path = format!(
            "../target/cmp/validator/{},{},{}",
            base.shard().workchain_id(),
            base.shard().shard_prefix_as_str_with_tag(),
            base.block_id().seq_no()
        );
        fs::create_dir_all(&path).ok();
        let mc_proof = MerkleProof::create_by_usage_tree(&base.pure_root, &base.mc_usage_tree)?;
        mc_proof.write_to_file(format!("{}/mc_state_proof.boc", path))?;
        let account_addr = trans.account_id();
        // sometimes it can be usable if it is the last transaction for this account
        // comment next two lines if it is not last transaction in account block
        let new_shard_acc =
            base.next_state_accounts.get_serialized(account_addr.clone())?.unwrap_or_default();
        // let new_shard_acc = ShardAccount::construct_from(&mut new_shard_acc)?;
        let new_account_root = new_shard_acc.account_cell();
        let config_cell = base.config_params.serialize()?;
        let trans_root = trans.serialize()?;
        let msg_cell = trans.in_msg_cell().unwrap_or_default();
        fs::write(format!("{}/{:x}.txt", path, trans_root.repr_hash()), err.as_bytes())?;

        let serialize_boc = |cell, name| {
            let data = write_boc(cell).unwrap();
            fs::write(format!("{}/{}", path, name), data).unwrap();
        };
        serialize_boc(&account_root, "account_old.boc");
        serialize_boc(&new_account_root, "account_new.boc");
        serialize_boc(&msg_cell, "message.boc");
        serialize_boc(&trans_root, "transaction.boc");
        serialize_boc(&config_cell, "config.boc");
        Ok(())
    }

    fn prepare_transaction_for_log(
        account_before: &Cell,
        account_after: &Cell,
        config: &ConfigParams,
        trans: &Transaction,
        trans_execute: Option<&Transaction>,
    ) {
        let cell_to_base64 = |cell| {
            let data = write_boc(cell).unwrap();
            base64_encode(data)
        };
        log::trace!(target: "validate_reject",
            "acc_before: {}\nacc_after: {}\nconfig: {}\ntrans_origin: {}\ntrans_execute: {}",
            cell_to_base64(account_before),
            cell_to_base64(account_after),
            base64_encode(config.write_to_bytes().unwrap()),
            trans.write_to_base64().unwrap(),
            trans_execute.map_or_else(|| "none".to_string(), |trans_execute| {
                trans_execute.write_to_base64().unwrap()
            })
        );
    }
}
