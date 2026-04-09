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
    engine_traits::EngineOperations,
    ext_messages::EXT_MESSAGES_TRACE_TARGET,
    rng::random::secure_256_bits,
    shard_state::ShardStateStuff,
    types::{
        accounts::ShardAccountStuff,
        limits::BlockLimitStatus,
        messages::{MsgEnqueueStuff, MsgEnvelopeStuff},
        top_block_descr::{cmp_shard_block_descr, Mode as TbdMode, TopBlockDescrStuff},
    },
    validating_utils::{
        check_cur_validator_set, check_this_shard_mc_info, may_update_shard_block_info,
        supported_capabilities, supported_version, UNREGISTERED_CHAIN_MAX_LEN,
    },
    validator::{
        out_msg_queue::{MsgQueueManager, OutMsgQueueInfoStuff, StatesManager},
        validator_group::PipelineContext,
        validator_utils::{calc_subset_for_masterchain, PrevBlockHistory},
        BlockCandidate, CollatorSettings, McData,
    },
    CHECK,
};
use adnl::common::Wait;
use futures::try_join;
use rand::Rng;
use std::{
    cmp::{max, min},
    collections::{BTreeMap, BinaryHeap, HashMap, HashSet},
    mem,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use ton_block::{
    error, fail, Account, AccountDispatchQueue, AccountId, AccountStorageDictProof, AddSub,
    BlkPrevInfo, Block, BlockCreateStats, BlockExtra, BlockIdExt, BlockInfo, BocFlags, BocWriter,
    Cell, ChildCell, Coins, CommonMsgInfo, ConfigParamEnum, ConfigParams, CreatorStats,
    CurrencyCollection, Deserializable, Error, ExtBlkRef, FutureSplitMerge, GlobalCapabilities,
    GlobalVersion, HashmapAugType, HashmapRemover, HashmapType, InMsg, InMsgDescr,
    InternalMessageHeader, KeyExtBlkRef, KeyMaxLt, Libraries, McBlockExtra, McShardRecord,
    McStateExtra, MerkleProof, MerkleUpdate, Message, MsgAddressInt, MsgMetadata, OutMsg,
    OutMsgDescr, OutMsgQueueKey, ParamLimitIndex, ProcessedInfoKey, ProcessedUpto, Result,
    Serializable, ShardAccount, ShardAccountBlocks, ShardAccounts, ShardDescr, ShardFees,
    ShardHashes, ShardIdent, ShardStateSplit, ShardStateUnsplit, SliceData, StorageStatDict,
    TopBlockDescrSet, Transaction, TransactionTickTock, UInt256, UsageTree, ValidatorSet,
    ValueFlow, WorkchainDescr, Workchains, MASTERCHAIN_ID,
};
#[cfg(feature = "xp25")]
use ton_block::{RefShardBlocks, ShardBlockRef, WcExtra};
use ton_executor::{
    BlockchainConfig, ExecuteParams, OrdinaryTransactionExecutor, TickTockTransactionExecutor,
    TransactionExecutor,
};
use ton_vm::smart_contract_info::PrevBlocksInfo;

// TODO move all constants (see validator query too) into one place
pub const SPLIT_MERGE_DELAY: u32 = 100; // prepare (delay) split/merge for 100 seconds
pub const SPLIT_MERGE_INTERVAL: u32 = 100; // split/merge is enabled during 60 second interval
pub const MAX_ERROR_ATTEMPTS: u32 = 5;

pub struct CycleVec<'a, T> {
    items: Vec<Option<&'a T>>,
    pos: Option<usize>,
}

impl<'a, T> CycleVec<'a, T> {
    pub fn from_slice(slice: &'a [T]) -> Self {
        Self { items: slice.iter().map(Option::Some).collect(), pos: None }
    }

    pub fn move_next(&mut self) -> Option<&T> {
        let len = self.items.len();
        let mut pos = self.pos.map_or(0, |pos| (pos + 1) % len);
        for _ in 0..len {
            if let Some(t) = self.items[pos] {
                self.pos = Some(pos);
                return Some(t);
            }
            pos = (pos + 1) % len;
        }
        None
    }

    pub fn remove_current(&mut self) -> Option<&T> {
        self.items.get_mut(self.pos?).and_then(|t| t.take())
    }
}

struct ImportedData {
    mc_state: Arc<ShardStateStuff>,
    prev_states: Vec<Arc<ShardStateStuff>>,
    prev_ext_blocks_refs: Vec<ExtBlkRef>,
    top_shard_blocks_descr: Vec<Arc<TopBlockDescrStuff>>,
}

pub struct PrevData {
    states: Vec<Arc<ShardStateStuff>>,
    _pure_states: Vec<Arc<ShardStateStuff>>,
    state_root: Cell, // pure cell without used tree my be no need
    accounts: ShardAccounts,
    gen_utime: u32,
    gen_lt: u64,
    total_validator_fees: CurrencyCollection,
    overload_history: u64,
    underload_history: u64,
}

impl PrevData {
    pub fn from_prev_states(
        states: Vec<Arc<ShardStateStuff>>,
        _pure_states: Vec<Arc<ShardStateStuff>>,
        state_root: Cell,
        subshard: Option<&ShardIdent>,
    ) -> Result<Self> {
        let mut gen_utime = states[0].state()?.gen_time();
        let mut gen_lt = states[0].state()?.gen_lt();
        let mut accounts = states[0].state()?.read_accounts()?;
        let mut total_validator_fees = states[0].state()?.total_validator_fees().clone();
        let mut overload_history = 0;
        let mut underload_history = 0;
        if let Some(state) = states.get(1) {
            gen_utime = max(gen_utime, state.state()?.gen_time());
            gen_lt = max(gen_lt, state.state()?.gen_lt());
            let key = state.shard().merge()?.shard_key(false);
            accounts.merge(&state.state()?.read_accounts()?, &key)?;
            total_validator_fees.add(state.state()?.total_validator_fees())?;
        } else if let Some(subshard) = subshard {
            accounts.split_for(&subshard.shard_key(false))?;
            if subshard.is_right_child() {
                total_validator_fees.coins += 1;
            }
            total_validator_fees.coins /= 2;
        } else {
            overload_history = states[0].state()?.overload_history();
            underload_history = states[0].state()?.underload_history();
        }
        Ok(Self {
            states,
            _pure_states,
            state_root,
            accounts,
            gen_utime,
            gen_lt,
            total_validator_fees,
            overload_history,
            underload_history,
        })
    }

    fn accounts(&self) -> &ShardAccounts {
        &self.accounts
    }
    fn overload_history(&self) -> u64 {
        self.overload_history
    }
    fn underload_history(&self) -> u64 {
        self.underload_history
    }
    fn prev_state_utime(&self) -> u32 {
        self.gen_utime
    }
    fn prev_state_lt(&self) -> u64 {
        self.gen_lt
    }
    fn prev_vert_seqno(&self) -> Result<u32> {
        Ok(self.states[0].state()?.vert_seq_no())
    }
    fn total_balance(&self) -> &CurrencyCollection {
        self.accounts.root_extra().balance()
    }
    fn total_validator_fees(&self) -> &CurrencyCollection {
        &self.total_validator_fees
    }
    fn state(&self) -> &ShardStateStuff {
        &self.states[0]
    }
    fn account(&self, account_id: &AccountId) -> Result<Option<ShardAccount>> {
        self.accounts.get_serialized(account_id.clone())
    }
}

#[derive(Debug)]
enum AsyncMessage {
    Recover(Message, Cell),
    Mint(Message, Cell),
    Ext(Arc<Message>, Cell, UInt256),
    Int(MsgEnqueueStuff, bool),
    New(MsgEnqueueStuff, ChildCell<Transaction>), // prev_trans_cell
    Deferred(MsgEnqueueStuff),
    TickTock(TransactionTickTock),
}

#[derive(Clone, Eq, PartialEq)]
struct NewMessage {
    enq: MsgEnqueueStuff,
    tr_cell: ChildCell<Transaction>,
    is_special: bool, // TBD: no need?
    index: usize,
}

impl NewMessage {
    fn new(
        enq: MsgEnqueueStuff,
        tr_cell: ChildCell<Transaction>,
        is_special: bool,
        index: usize,
    ) -> Self {
        Self { enq, tr_cell, is_special, index }
    }

    fn new_deferred(enq: MsgEnqueueStuff) -> NewMessage {
        Self { enq, tr_cell: ChildCell::default(), is_special: false, index: 0 }
    }
}

impl Ord for NewMessage {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.enq.lt(), other.enq.message_hash()).cmp(&(self.enq.lt(), self.enq.message_hash()))
    }
}

impl PartialOrd for NewMessage {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct CollatorData {
    // lists, empty by default
    in_msgs: InMsgDescr,
    out_msgs: OutMsgDescr,
    out_msg_queue_info: OutMsgQueueInfoStuff,
    shard_fees: ShardFees,
    shard_top_block_descriptors: Vec<Arc<TopBlockDescrStuff>>,
    block_create_count: HashMap<UInt256, u64>,
    new_messages: BinaryHeap<NewMessage>, // using for priority queue
    accepted_ext_messages: Vec<(UInt256, i32)>, // message id and wokchain id
    rejected_ext_messages: Vec<(UInt256, String)>, // message id and reject reason
    usage_tree: UsageTree,
    imported_visited: HashSet<UInt256>,
    last_dispatch_queue_emitted_lt: HashMap<AccountId, u64>,
    unprocessed_deferred_messages: HashMap<AccountId, usize>, // number of messages from dispatch queue in new_msgs
    sender_generated_messages_count: HashMap<AccountId, usize>,
    dispatch_queue_total_limit_reached: bool,
    have_unprocessed_account_dispatch_queue: bool,

    // determined fields
    gen_utime: u32,
    gen_utime_ms: u64,
    config: BlockchainConfig,
    collated_block_descr: Arc<String>,
    block_limit_class: ParamLimitIndex,
    error_attempt: u32,

    // fields, uninitialized by default
    start_lt: Option<u64>,
    value_flow: ValueFlow,
    min_ref_mc_seqno: Option<u32>,
    prev_stuff: Option<BlkPrevInfo>,
    shards: Option<ShardHashes>,
    mint_msg: Option<InMsg>,
    recover_create_msg: Option<InMsg>,

    // fields with default values
    skip_topmsgdescr: bool,
    skip_extmsg: bool,
    shard_conf_adjusted: bool,
    block_limit_status: BlockLimitStatus,
    block_create_total: u64,
    inbound_queues_empty: bool,
    enqueue_only: bool,
    last_proc_int_msg: (u64, UInt256),
    shards_max_end_lt: u64,
    before_split: bool,
    now_upper_limit: u32,
    msg_queue_depth_sum: usize,
    old_out_msg_queue_size: usize,
    out_msg_queue_size: usize,

    // Split/merge
    want_merge: bool,
    underload_history: u64,
    want_split: bool,
    overload_history: u64,
    block_full: bool,

    // Block metrics
    dequeue_count: usize,
    enqueue_count: usize,
    transit_count: usize,
    execute_count: usize,
    out_msg_count: usize,
    in_msg_count: usize,
    remove_count: usize,
}

impl CollatorData {
    pub fn new(
        gen_utime: u32,
        gen_utime_ms: u64,
        config: BlockchainConfig,
        usage_tree: UsageTree,
        prev_data: &PrevData,
        is_masterchain: bool,
        collated_block_descr: Arc<String>,
        error_attempt: u32,
    ) -> Result<Self> {
        let limits = Arc::new(config.raw_config().block_limits(is_masterchain)?);
        let ret = Self {
            in_msgs: InMsgDescr::default(),
            out_msgs: OutMsgDescr::default(),
            out_msg_queue_info: OutMsgQueueInfoStuff::default(),
            shard_fees: ShardFees::default(),
            shard_top_block_descriptors: Vec::new(),
            block_create_count: HashMap::new(),
            new_messages: Default::default(),
            accepted_ext_messages: Default::default(),
            rejected_ext_messages: Default::default(),
            usage_tree,
            imported_visited: HashSet::new(),
            unprocessed_deferred_messages: HashMap::new(),
            sender_generated_messages_count: HashMap::new(),
            last_dispatch_queue_emitted_lt: HashMap::new(),
            dispatch_queue_total_limit_reached: false,
            have_unprocessed_account_dispatch_queue: false,
            gen_utime,
            gen_utime_ms,
            config,
            collated_block_descr,
            block_limit_class: ParamLimitIndex::Underload,
            start_lt: None,
            value_flow: ValueFlow::default(),
            now_upper_limit: u32::MAX,
            old_out_msg_queue_size: 0,
            out_msg_queue_size: 0,
            shards_max_end_lt: 0,
            min_ref_mc_seqno: None,
            prev_stuff: None,
            shards: None,
            mint_msg: None,
            recover_create_msg: None,
            skip_topmsgdescr: false,
            skip_extmsg: false,
            shard_conf_adjusted: false,
            block_limit_status: BlockLimitStatus::with_limits(limits),
            block_create_total: 0,
            inbound_queues_empty: false,
            enqueue_only: false,
            last_proc_int_msg: (0, UInt256::default()),
            want_merge: false,
            underload_history: prev_data.underload_history() << 1,
            want_split: false,
            overload_history: prev_data.overload_history() << 1,
            block_full: false,
            dequeue_count: 0,
            enqueue_count: 0,
            transit_count: 0,
            execute_count: 0,
            out_msg_count: 0,
            in_msg_count: 0,
            remove_count: 0,
            msg_queue_depth_sum: 0,
            before_split: false,
            error_attempt,
        };
        Ok(ret)
    }

    fn gen_utime(&self) -> u32 {
        self.gen_utime
    }

    fn gen_utime_ms(&self) -> u64 {
        self.gen_utime_ms
    }

    //
    // Lists
    //

    fn in_msgs_root(&self) -> Result<Cell> {
        self.in_msgs.data().cloned().ok_or_else(|| error!("in msg descr is empty"))
    }

    fn out_msgs_root(&self) -> Result<Cell> {
        self.out_msgs.data().cloned().ok_or_else(|| error!("out msg descr is empty"))
    }

    fn update_last_proc_int_msg(&mut self, new_lt_hash: (u64, UInt256)) -> Result<()> {
        if self.last_proc_int_msg < new_lt_hash {
            CHECK!(new_lt_hash.0 > 0);
            log::trace!("last_proc_int_msg updated to ({},{:x})", new_lt_hash.0, new_lt_hash.1);
            self.last_proc_int_msg = new_lt_hash;
        } else {
            log::error!(
                "{} processed message ({},{:x}) AFTER message ({},{:x})",
                self.collated_block_descr,
                new_lt_hash.0,
                new_lt_hash.1,
                self.last_proc_int_msg.0,
                self.last_proc_int_msg.1
            );
            self.last_proc_int_msg.0 = u64::MAX;
            fail!("internal message processing order violated!")
        }
        Ok(())
    }

    fn update_lt(&mut self, lt: u64) {
        self.block_limit_status.update_lt(lt);
    }

    /// put InMsg to block
    fn add_in_msg_to_block(&mut self, in_msg: &InMsg) -> Result<()> {
        self.in_msg_count += 1;
        let msg_cell = in_msg.serialize()?;
        self.in_msgs.insert(in_msg)?;
        self.block_limit_status.register_in_msg_op(&msg_cell, &self.in_msgs_root()?)
    }

    /// put OutMsg to block
    fn add_out_msg_to_block(&mut self, key: &UInt256, out_msg: &OutMsg) -> Result<()> {
        self.out_msg_count += 1;
        self.out_msgs.insert_with_key(key, out_msg)?;

        let msg_cell = out_msg.serialize()?;
        self.block_limit_status.register_out_msg_op(&msg_cell, &self.out_msgs_root()?)
    }

    fn register_dispatch_queue_op(&mut self, force: bool) -> Result<()> {
        let root = self.out_msg_queue_info.out_queue_extra_mut().dispatch_queue.data();
        self.block_limit_status.register_dispatch_queue_op(root, &self.usage_tree, force)
    }

    fn defer_message(&mut self, account_id: &AccountId, enq: &MsgEnqueueStuff) -> Result<()> {
        self.out_msg_queue_info.out_queue_extra_mut().insert(account_id, enq.lt(), enq.inner())?;
        self.register_dispatch_queue_op(false)?;
        Ok(())
    }

    /// delete message from state queue
    fn del_out_msg_from_state(&mut self, key: &OutMsgQueueKey) -> Result<()> {
        log::debug!("del_out_msg_from_state {:x}", key);
        self.dequeue_count += 1;
        self.out_msg_queue_info.del_message(key)?;
        self.out_msg_queue_size = self.out_msg_queue_size.saturating_sub(1);
        self.block_limit_status.register_out_msg_queue_op(
            self.out_msg_queue_info.out_queue().data(),
            &self.usage_tree,
            false,
        )?;
        Ok(())
    }

    /// add message to state queue
    /// augmentation value is in enq.lt()
    /// it could be less than start_lt in case of reenqueue
    fn add_msg_to_state(&mut self, enq: &MsgEnqueueStuff, force: bool) -> Result<()> {
        if enq.lt() > enq.enqueued_lt() {
            log::warn!(
                "Adding OutMsgQueue message with hash {:x}, lt={}, enqueue_lt={}",
                enq.envelope().message_hash(),
                enq.lt(),
                enq.enqueued_lt()
            );
        }
        self.enqueue_count += 1;
        self.msg_queue_depth_sum += self.out_msg_queue_info.add_message(enq)?;
        self.out_msg_queue_size += 1;
        self.block_limit_status.register_out_msg_queue_op(
            self.out_msg_queue_info.out_queue().data(),
            &self.usage_tree,
            force,
        )?;
        Ok(())
    }

    fn enqueue_transit_message(
        &mut self,
        shard: &ShardIdent,
        enq: &MsgEnqueueStuff,
        requeue: bool,
        from_dispatch_queue: bool,
    ) -> Result<()> {
        let lt = self.start_lt()?.max(enq.lt());
        let (new_enq, transit_fee) = enq.next_hop(shard, lt, &self.config, from_dispatch_queue)?;
        let in_msg = if from_dispatch_queue {
            log::debug!(
                "{}: enqueueing message from dispatch queue {:x}, emitted_lt={}",
                self.collated_block_descr,
                enq.message_hash(),
                enq.emitted_lt()
            );
            InMsg::deferred_transit(enq.envelope_cell(), new_enq.envelope_cell())
        } else {
            log::debug!(
                "{}: enqueueing transit message {:x}",
                self.collated_block_descr,
                enq.message_hash()
            );
            InMsg::transit(enq.envelope_cell(), new_enq.envelope_cell(), transit_fee)
        };
        let imported_cell = ChildCell::with_struct(&in_msg)?;
        let out_msg = if from_dispatch_queue {
            OutMsg::deferred_transit(new_enq.envelope_cell(), imported_cell)
        } else {
            OutMsg::transit(new_enq.envelope_cell(), imported_cell, requeue)
        };
        self.transit_count += 1;
        self.add_in_msg_to_block(&in_msg)?;
        self.add_out_msg_to_block(&enq.message_hash(), &out_msg)?;
        self.add_msg_to_state(&new_enq, false)
    }

    pub fn add_top_block_descriptor(&mut self, tbd: Arc<TopBlockDescrStuff>) {
        self.shard_top_block_descriptors.push(tbd)
    }

    pub fn shard_fees(&self) -> &ShardFees {
        &self.shard_fees
    }

    pub fn store_shard_fees_zero(&mut self, shard: &ShardIdent) -> Result<()> {
        self.shard_fees.store_shard_fees(
            shard,
            CurrencyCollection::with_coins(0),
            CurrencyCollection::with_coins(0),
        )
    }

    pub fn store_shard_fees(&mut self, shard: &McShardRecord) -> Result<()> {
        self.shard_fees.store_shard_fees(
            shard.shard(),
            shard.descr.fees_collected.clone(),
            shard.descr.funds_created.clone(),
        )
    }

    pub fn register_shard_block_creators(&mut self, creators: Vec<UInt256>) -> Result<()> {
        for creator in creators {
            let prev_value = *self.block_create_count.get(&creator).unwrap_or(&0);
            self.block_create_count.insert(creator, prev_value + 1);
            self.block_create_total += 1;
        }
        Ok(())
    }
    pub fn block_create_count(&self) -> &HashMap<UInt256, u64> {
        &self.block_create_count
    }
    pub fn block_create_total(&self) -> u64 {
        self.block_create_total
    }

    fn count_bits_u64(mut x: u64) -> isize {
        x = (x & 0x5555555555555555) + ((x >> 1) & 0x5555555555555555);
        x = (x & 0x3333333333333333) + ((x >> 2) & 0x3333333333333333);
        x = (x & 0x0F0F0F0F0F0F0F0F) + ((x >> 4) & 0x0F0F0F0F0F0F0F0F);
        x = (x & 0x00FF00FF00FF00FF) + ((x >> 8) & 0x00FF00FF00FF00FF);
        x = (x & 0x0000FFFF0000FFFF) + ((x >> 16) & 0x0000FFFF0000FFFF);
        x = (x & 0x00000000FFFFFFFF) + ((x >> 32) & 0x00000000FFFFFFFF);
        x as isize
    }

    fn history_weight(history: u64) -> isize {
        Self::count_bits_u64(history & 0xffff) * 3
            + Self::count_bits_u64(history & 0xffff0000) * 2
            + Self::count_bits_u64(history & 0xffff00000000)
            - (3 + 2 + 1) * 16 * 2 / 3
    }

    //
    // fields, uninitialized by default
    //

    fn start_lt(&self) -> Result<u64> {
        self.start_lt.ok_or_else(|| error!("`start_lt` is not initialized yet"))
    }

    fn set_start_lt(&mut self, lt: u64) -> Result<()> {
        if self.start_lt.is_some() {
            fail!("`start_lt` is already initialized")
        }
        self.block_limit_status.update_lt(lt);
        self.start_lt = Some(lt);
        Ok(())
    }

    fn prev_stuff(&self) -> Result<&BlkPrevInfo> {
        self.prev_stuff.as_ref().ok_or_else(|| error!("`prev_stuff` is not initialized yet"))
    }

    fn now_upper_limit(&self) -> u32 {
        self.now_upper_limit
    }

    fn set_now_upper_limit(&mut self, val: u32) {
        self.now_upper_limit = val;
    }

    fn shards_max_end_lt(&self) -> u64 {
        self.shards_max_end_lt
    }

    fn update_shards_max_end_lt(&mut self, val: u64) {
        if val > self.shards_max_end_lt {
            self.shards_max_end_lt = val;
        }
    }

    fn update_min_mc_seqno(&mut self, mc_seqno: u32) -> u32 {
        let min_ref_mc_seqno = min(self.min_ref_mc_seqno.unwrap_or(u32::MAX), mc_seqno);
        self.min_ref_mc_seqno = Some(min_ref_mc_seqno);
        min_ref_mc_seqno
    }

    fn min_mc_seqno(&self) -> Result<u32> {
        self.min_ref_mc_seqno.ok_or_else(|| error!("`min_ref_mc_seqno` is not initialized yet"))
    }

    fn shards(&self) -> Result<&ShardHashes> {
        self.shards.as_ref().ok_or_else(|| error!("`shards` is not initialized yet"))
    }

    fn shards_mut(&mut self) -> Result<&mut ShardHashes> {
        self.shards.as_mut().ok_or_else(|| error!("`shards` is not initialized yet"))
    }

    fn set_shards(&mut self, shards: ShardHashes) -> Result<()> {
        if self.shards.is_some() {
            fail!("`shards` is already initialized")
        }
        self.shards = Some(shards);
        Ok(())
    }

    //
    // fields with default values
    //

    fn skip_topmsgdescr(&self) -> bool {
        self.skip_topmsgdescr
    }
    fn set_skip_topmsgdescr(&mut self) {
        self.skip_topmsgdescr = true;
    }

    fn skip_extmsg(&self) -> bool {
        self.skip_extmsg
    }
    fn set_skip_extmsg(&mut self) {
        self.skip_extmsg = true;
    }

    fn set_shard_conf_adjusted(&mut self) {
        self.shard_conf_adjusted = true;
    }

    fn dequeue_message(
        &mut self,
        enq: &MsgEnqueueStuff,
        deliver_lt: u64,
        short: bool,
    ) -> Result<()> {
        self.dequeue_count += 1;
        let out_msg = match short {
            true => OutMsg::dequeue_short(enq.envelope_hash(), enq.next_prefix(), deliver_lt),
            false => OutMsg::dequeue_long(enq.envelope_cell(), deliver_lt),
        };
        self.add_out_msg_to_block(&enq.message_hash(), &out_msg)
    }

    fn want_merge(&self) -> (bool, u64) {
        (self.want_merge, self.underload_history)
    }

    fn want_split(&self) -> (bool, u64) {
        (self.want_split, self.overload_history)
    }

    fn before_split(&self) -> bool {
        self.before_split
    }
    fn set_before_split(&mut self, value: bool) {
        self.before_split = value
    }

    fn estimate_pruned_count(&self) -> usize {
        if self.enqueue_count != 0 {
            let total_count = self.dequeue_count + self.enqueue_count + self.remove_count;
            total_count * self.msg_queue_depth_sum / self.enqueue_count
        } else {
            0
        }
    }

    fn limit_fits(&self, level: ParamLimitIndex) -> bool {
        let pruned_count = self.estimate_pruned_count();
        self.block_limit_status.fits(level, pruned_count)
    }
    fn classify_block_limit_class(&mut self) {
        let pruned_count = self.estimate_pruned_count();
        self.block_limit_class =
            self.block_limit_status.classify(pruned_count).max(self.block_limit_class);
    }

    fn last_dispatch_queue_emitted_lt(&self, account_id: &AccountId) -> u64 {
        self.last_dispatch_queue_emitted_lt.get(account_id).cloned().unwrap_or_default()
    }

    fn add_unprocessed_deferred_message(&mut self, account_id: AccountId) {
        *self.unprocessed_deferred_messages.entry(account_id).or_default() += 1;
    }

    fn del_unprocessed_deferred_messages(&mut self, account_id: &AccountId) {
        if let Some(count) = self.unprocessed_deferred_messages.get_mut(account_id) {
            *count = count.saturating_sub(1)
        }
    }

    fn has_unprocessed_deferred_messages(&self, account_id: &AccountId) -> bool {
        self.unprocessed_deferred_messages.get(account_id).is_some_and(|p| *p != 0)
    }

    fn sender_generated_messages_count(&self, account_id: &AccountId) -> usize {
        self.sender_generated_messages_count.get(account_id).cloned().unwrap_or_default()
    }

    fn add_sender_generated_message(&mut self, account_id: AccountId) -> usize {
        let count = self.sender_generated_messages_count.entry(account_id).or_default();
        *count += 1;
        *count
    }

    fn has_dispatch_queue(&self, account_id: &AccountId) -> Result<bool> {
        self.out_msg_queue_info.out_queue_extra().dispatch_queue().contains(account_id)
    }

    fn must_be_deferred(&self, account_id: &AccountId) -> Result<bool> {
        Ok(self.has_unprocessed_deferred_messages(account_id)
            || self.has_dispatch_queue(account_id)?)
    }
}

type MessageSender = tokio::sync::mpsc::UnboundedSender<(Arc<AsyncMessage>, Option<MsgMetadata>)>;
struct ExecutionManager {
    changed_accounts:
        BTreeMap<AccountId, (MessageSender, tokio::task::JoinHandle<Result<ShardAccountStuff>>)>,

    receive_tr: tokio::sync::mpsc::UnboundedReceiver<
        Option<(Arc<AsyncMessage>, Option<MsgMetadata>, Result<Transaction>)>,
    >,
    wait_tr: Arc<Wait<(Arc<AsyncMessage>, Option<MsgMetadata>, Result<Transaction>)>>,
    max_collate_threads: usize,
    libraries: Libraries,
    gen_utime: u32,

    // bloc's start logical time
    start_lt: u64,
    // actual maximum logical time
    max_lt: Arc<AtomicU64>,
    // this time is used if account's lt is smaller
    min_lt: Arc<AtomicU64>,
    // block random seed
    seed_block: UInt256,

    total_trans_duration: Arc<AtomicU64>,
    collated_block_descr: Arc<String>,
    debug: bool,
    lt_compatible: bool,
    config: BlockchainConfig,
    prev_blocks_info: PrevBlocksInfo,
    engine: Arc<dyn EngineOperations>,
    cancel_ext: Arc<AtomicBool>,
}

impl ExecutionManager {
    pub fn new(
        engine: Arc<dyn EngineOperations>,
        gen_utime: u32,
        start_lt: u64,
        seed_block: UInt256,
        mc_data: &McData,
        config: BlockchainConfig,
        max_collate_threads: usize,
        collated_block_descr: Arc<String>,
        debug: bool,
        lt_compatible: bool,
    ) -> Result<Self> {
        log::trace!("{}: ExecutionManager::new", collated_block_descr);
        let (wait_tr, receive_tr) = Wait::new();
        Ok(Self {
            changed_accounts: BTreeMap::new(),
            receive_tr,
            wait_tr,
            max_collate_threads,
            libraries: mc_data.libraries()?.clone(),
            config,
            start_lt,
            gen_utime,
            seed_block,
            max_lt: Arc::new(AtomicU64::new(start_lt + 1)),
            min_lt: Arc::new(AtomicU64::new(start_lt + 1)),
            total_trans_duration: Arc::new(AtomicU64::new(0)),
            collated_block_descr,
            debug,
            lt_compatible,
            prev_blocks_info: PrevBlocksInfo::Raw(
                mc_data.last_mc_block_id(),
                mc_data.state.shard_state_extra()?.prev_blocks.clone(),
            ),
            engine,
            cancel_ext: Arc::new(AtomicBool::new(false)),
        })
    }

    // checks if a number of parallel transactilns is not too big, waits and finalizes some if needed.
    pub async fn check_parallel_transactions(
        &mut self,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!("{}: check_parallel_transactions", self.collated_block_descr);
        if self.wait_tr.count() >= self.max_collate_threads {
            self.wait_transaction(collator_data).await?;
        }
        Ok(())
    }

    pub async fn execute(
        &mut self,
        account_id: AccountId,
        msg: AsyncMessage,
        msg_metadata: Option<MsgMetadata>,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
    ) -> Result<bool> {
        log::trace!("{}: execute (adding into queue): {:x}", self.collated_block_descr, account_id);
        if let Some((sender, _handle)) = self.changed_accounts.get(&account_id) {
            self.wait_tr.request();
            sender.send((Arc::new(msg), msg_metadata))?;
        } else {
            let shard_acc = if let Some(shard_acc) = prev_data.accounts().account(&account_id)? {
                shard_acc
            } else if let AsyncMessage::Ext(_, _, msg_id) = msg {
                collator_data
                    .rejected_ext_messages
                    .push((msg_id, format!("account {:x} not found", account_id)));
                return Ok(true); // skip external messages for unexisting accounts
            } else {
                ShardAccount::default()
            };
            let (sender, handle) =
                self.spawn_account_job(account_id.clone(), shard_acc, collator_data)?;
            self.wait_tr.request();
            sender.send((Arc::new(msg), msg_metadata))?;
            self.changed_accounts.insert(account_id, (sender, handle));
        }

        self.check_parallel_transactions(collator_data).await?;

        Ok(true)
    }

    fn spawn_account_job(
        &self,
        account_id: AccountId,
        shard_acc: ShardAccount,
        collator_data: &CollatorData,
    ) -> Result<(MessageSender, tokio::task::JoinHandle<Result<ShardAccountStuff>>)> {
        log::trace!("{}: start_account_job: {:x}", self.collated_block_descr, account_id);

        let lt = collator_data.last_dispatch_queue_emitted_lt(&account_id);
        let debug = self.debug;
        let block_unixtime = self.gen_utime;
        let block_lt = self.start_lt;
        let seed_block = self.seed_block.clone();
        let collated_block_descr = self.collated_block_descr.clone();
        let total_trans_duration = self.total_trans_duration.clone();
        let wait_tr = self.wait_tr.clone();
        let config = self.config.clone();
        let min_lt = self.min_lt.clone();
        let max_lt = self.max_lt.clone();
        let libraries = self.libraries.clone().inner();
        let prev_blocks_info = self.prev_blocks_info.clone();
        let (sender, mut receiver) =
            tokio::sync::mpsc::unbounded_channel::<(Arc<AsyncMessage>, Option<MsgMetadata>)>();
        let engine = self.engine.clone();
        let dict_hash_min_cells = self.config.size_limits_config().acc_state_cells_for_storage_dict;
        let lt_compatible = self.lt_compatible;
        let cancel_ext = self.cancel_ext.clone();
        let handle = tokio::spawn(async move {
            let lt = lt.max(min_lt.load(Ordering::Relaxed));
            let full_collated_data = config.has_capability(GlobalCapabilities::CapFullCollatedData);
            let mut shard_acc = tokio::task::spawn_blocking(move || {
                ShardAccountStuff::init(
                    &engine,
                    account_id,
                    shard_acc,
                    lt,
                    full_collated_data,
                    lt_compatible,
                    dict_hash_min_cells,
                )
            })
            .await??;
            while let Some((new_msg, msg_metadata)) = receiver.recv().await {
                log::trace!(
                    "{}: new message for {:x}",
                    collated_block_descr,
                    shard_acc.account_id()
                );
                if cancel_ext.load(Ordering::Relaxed) {
                    log::debug!(
                        "{}: account {:x} ext message cancelled by cutoff timeout",
                        collated_block_descr,
                        shard_acc.account_id()
                    );
                    let transaction_res = Err(error!("cancelled by cutoff timeout"));
                    wait_tr.respond(Some((new_msg, msg_metadata, transaction_res)));
                    continue;
                }

                let config = config.clone(); // TODO: use Arc

                let mut min_lt = min_lt.load(Ordering::Relaxed);
                if let AsyncMessage::Deferred(enq) = &*new_msg {
                    min_lt = min_lt.max(enq.emitted_lt().saturating_add(1));
                };
                shard_acc.fetch_max_lt(min_lt);
                let mut account = shard_acc.account().clone();

                let params = ExecuteParams {
                    state_libs: libraries.clone(),
                    block_unixtime,
                    block_lt,
                    last_tr_lt: shard_acc.lt(),
                    seed_block: seed_block.clone(),
                    debug,
                    prev_blocks_info: prev_blocks_info.clone(),
                    ..ExecuteParams::default()
                };
                let new_msg1 = new_msg.clone();
                let (mut transaction_res, account, duration) =
                    tokio::task::spawn_blocking(move || {
                        let now = Instant::now();
                        (
                            Self::execute_new_message(&new_msg1, &mut account, config, params),
                            account,
                            now.elapsed().as_micros() as u64,
                        )
                    })
                    .await?;

                if cancel_ext.load(Ordering::Relaxed) {
                    log::debug!(
                        "{}: account {:x} ext message cancelled by cutoff timeout",
                        collated_block_descr,
                        shard_acc.account_id()
                    );
                    let transaction_res =
                        Err(error!("cancelled by cutoff timeout after execution"));
                    wait_tr.respond(Some((new_msg, msg_metadata, transaction_res)));
                    continue;
                }

                if let Ok(transaction) = transaction_res.as_mut() {
                    let res = shard_acc.add_transaction(transaction, account);
                    if let Err(err) = res {
                        log::error!("FAILED to add transaction to shard account staff: {}", &err);
                        fail!(err);
                    }
                }
                total_trans_duration.fetch_add(duration, Ordering::Relaxed);
                log::trace!(
                    "{}: account {:x} TIME execute {}μ;",
                    collated_block_descr,
                    shard_acc.account_id(),
                    duration
                );

                max_lt.fetch_max(shard_acc.lt(), Ordering::Relaxed);
                wait_tr.respond(Some((new_msg, msg_metadata, transaction_res)));
            }
            Ok(shard_acc)
        });
        Ok((sender, handle))
    }

    fn execute_new_message(
        new_msg: &AsyncMessage,
        account: &mut Account,
        config: BlockchainConfig,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        let (executor, msg_opt): (Box<dyn TransactionExecutor>, _) = match new_msg {
            AsyncMessage::Int(enq, _) | AsyncMessage::New(enq, _) | AsyncMessage::Deferred(enq) => {
                (Box::new(OrdinaryTransactionExecutor::new(config)), Some(enq.message_cell()))
            }
            AsyncMessage::Recover(_, msg_cel) | AsyncMessage::Mint(_, msg_cel) => {
                (Box::new(OrdinaryTransactionExecutor::new(config)), Some(msg_cel.clone()))
            }
            AsyncMessage::Ext(_, msg_cell, _) => {
                (Box::new(OrdinaryTransactionExecutor::new(config)), Some(msg_cell.clone()))
            }
            AsyncMessage::TickTock(tt) => {
                (Box::new(TickTockTransactionExecutor::new(config, tt.clone())), None)
            }
        };

        executor.execute_with_params(msg_opt, account, params)
    }

    async fn wait_transaction(&mut self, collator_data: &mut CollatorData) -> Result<()> {
        log::trace!("{}: wait_transaction", self.collated_block_descr);
        let wait_op = self.wait_tr.wait(&mut self.receive_tr, false).await;
        if let Some(Some((new_msg, msg_metadata, transaction_res))) = wait_op {
            self.finalize_transaction(new_msg, msg_metadata, transaction_res, collator_data)?;
        }
        Ok(())
    }

    fn finalize_transaction(
        &mut self,
        new_msg: Arc<AsyncMessage>,
        mut msg_metadata: Option<MsgMetadata>,
        transaction_res: Result<Transaction>,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        if let AsyncMessage::Ext(msg, _, msg_id) = &*new_msg {
            let address = msg
                .dst_ref()
                .ok_or_else(|| error!("External inbound message without dst address"))?;
            if let Err(err) = transaction_res {
                log::warn!(
                    target: EXT_MESSAGES_TRACE_TARGET,
                    "{}: account {} rejected inbound external message {:x}, by reason: {}",
                    self.collated_block_descr, address, msg_id, err
                );
                collator_data.rejected_ext_messages.push((msg_id.clone(), err.to_string()));
                return Ok(());
            } else {
                log::debug!(
                    target: EXT_MESSAGES_TRACE_TARGET,
                    "{}: account {} accepted inbound external message {:x}",
                    self.collated_block_descr, address, msg_id,
                );
                collator_data
                    .accepted_ext_messages
                    .push((msg_id.clone(), msg.dst_workchain_id().unwrap_or_default()));
            }
        }
        let tr = transaction_res?;
        if let Some(msg_metadata) = msg_metadata.as_mut() {
            msg_metadata.update_initiator_lt(tr.logical_time());
        }
        let tr_cell = ChildCell::with_struct(&tr)?;
        log::trace!(
            "{}: finalize_transaction {} with hash {:x}, {:x}",
            self.collated_block_descr,
            tr.logical_time(),
            tr_cell.cell().repr_hash(),
            tr.account_id()
        );
        let mut is_special = false;
        let in_msg_opt = match new_msg.deref() {
            AsyncMessage::Int(enq, our) => {
                let in_msg = InMsg::final_msg(
                    enq.envelope_cell(),
                    tr_cell.clone(),
                    *enq.fwd_fee_remaining(),
                );
                if *our {
                    let out_msg = OutMsg::dequeue_immediate(
                        enq.envelope_cell(),
                        ChildCell::with_struct(&in_msg)?,
                    );
                    collator_data.add_out_msg_to_block(&enq.message_hash(), &out_msg)?;
                    collator_data.del_out_msg_from_state(&enq.out_msg_key())?;
                }
                Some(in_msg)
            }
            AsyncMessage::New(enq, prev_tr_cell) => {
                let env_cell = enq.envelope_cell();
                let in_msg =
                    InMsg::immediate(env_cell.clone(), tr_cell.clone(), *enq.fwd_fee_remaining());
                let out_msg = OutMsg::immediate(
                    env_cell,
                    prev_tr_cell.clone(),
                    ChildCell::with_struct(&in_msg)?,
                );
                collator_data.add_out_msg_to_block(&enq.message_hash(), &out_msg)?;
                Some(in_msg)
            }
            AsyncMessage::Deferred(enq) => {
                let env_cell = enq.envelope_cell();
                let in_msg = InMsg::deferred_final(
                    env_cell.clone(),
                    tr_cell.clone(),
                    *enq.fwd_fee_remaining(),
                );
                // no need output message for deferred messages
                // let out_msg = OutMsg::new_defer(env_cell, ChildCell::default());
                // collator_data.add_out_msg_to_block(&enq.message_hash(), &out_msg)?;
                Some(in_msg)
            }
            AsyncMessage::Mint(msg, _) | AsyncMessage::Recover(msg, _) => {
                is_special = true;
                let env = MsgEnvelopeStuff::new(
                    msg.clone(),
                    tr.in_msg_cell().ok_or_else(|| error!("Transaction must have in_msg_cell"))?,
                    &ShardIdent::masterchain(),
                    Coins::default(),
                    false,
                )?;
                Some(InMsg::immediate(
                    ChildCell::with_struct(env.inner())?,
                    tr_cell.clone(),
                    Coins::default(),
                ))
            }
            AsyncMessage::Ext(msg, _, _) => {
                let in_msg =
                    InMsg::external(ChildCell::with_struct(msg.as_ref())?, tr_cell.clone());
                Some(in_msg)
            }
            AsyncMessage::TickTock(_) => {
                is_special = true;
                None
            }
        };
        if tr.orig_status != tr.end_status {
            log::info!(
                "{}: Status of account {:x} was changed from {:?} to {:?} by message {:X}",
                self.collated_block_descr,
                tr.account_id(),
                tr.orig_status,
                tr.end_status,
                tr.in_msg_cell().unwrap_or_default().repr_hash()
            );
        }
        self.new_transaction(
            collator_data,
            &tr,
            tr_cell,
            in_msg_opt.as_ref(),
            msg_metadata,
            is_special,
        )?;
        if !tr.blackhole_burned().is_zero() {
            collator_data.value_flow.burned.coins.add(tr.blackhole_burned())?;
        }

        collator_data.update_lt(self.max_lt.load(Ordering::Relaxed));

        match &*new_msg {
            AsyncMessage::Mint(_, _) => collator_data.mint_msg = in_msg_opt,
            AsyncMessage::Recover(_, _) => collator_data.recover_create_msg = in_msg_opt,
            _ => (),
        }
        collator_data.block_full |= !collator_data.limit_fits(ParamLimitIndex::Normal);
        Ok(())
    }

    /// add in and out messages to block, and to new message queue
    fn new_transaction(
        &self,
        collator_data: &mut CollatorData,
        transaction: &Transaction,
        tr_cell: ChildCell<Transaction>,
        in_msg_opt: Option<&InMsg>,
        msg_metadata: Option<MsgMetadata>,
        is_special: bool,
    ) -> Result<()> {
        // log::trace!(
        //     "new transaction, message {:x}\n{}",
        //     in_msg_opt.map(|m| m.message_cell().unwrap().repr_hash()).unwrap_or_default(),
        //     ton_block_json::debug_transaction(transaction.clone()).unwrap_or_default(),
        // );
        collator_data.execute_count += 1;
        let gas_used = transaction.gas_used().unwrap_or(0);
        collator_data.block_limit_status.add_gas_used(gas_used as u32);
        collator_data
            .block_limit_status
            .add_transaction(transaction.logical_time() == collator_data.start_lt()? + 1);
        if let Some(in_msg) = in_msg_opt {
            collator_data.add_in_msg_to_block(in_msg)?;
        }
        let shard = collator_data.out_msg_queue_info.shard().clone();
        let mut index = 0;
        transaction.out_msgs.iterate_slices(|slice| {
            let msg_cell = slice.reference(0)?;
            let msg = Message::construct_from_cell(msg_cell.clone())?;
            match msg.header() {
                CommonMsgInfo::IntMsgInfo(info) => {
                    let fwd_fee = *info.fwd_fee();
                    // LT - created_lt
                    let enq = MsgEnqueueStuff::new(
                        msg,
                        msg_cell,
                        &shard,
                        fwd_fee,
                        true,
                        msg_metadata.clone(),
                    )?;
                    let new_message = NewMessage::new(enq, tr_cell.clone(), is_special, index);
                    collator_data.new_messages.push(new_message);
                    index += 1;
                }
                CommonMsgInfo::ExtOutMsgInfo(_) => {
                    let out_msg = OutMsg::external(ChildCell::with_cell(msg_cell), tr_cell.clone());
                    collator_data.add_out_msg_to_block(&out_msg.read_message_hash()?, &out_msg)?;
                }
                CommonMsgInfo::ExtInMsgInfo(_) => {
                    fail!("External inbound message cannot be output")
                }
            };
            Ok(true)
        })?;
        Ok(())
    }
}

pub enum CollateResult {
    Ok {
        candidate: BlockCandidate,
        new_state: ShardStateUnsplit,
        new_block: Block,
        usage_tree: UsageTree,
        /// The serialized block root cell, stored to avoid re-serialization
        /// when sending block candidate broadcast
        block_root: Cell,
    },
    Err {
        usage_tree: UsageTree,
        err: Error,
    },
}

pub struct Collator {
    engine: Arc<dyn EngineOperations>,
    shard: ShardIdent,
    min_mc_seqno: u32,
    prev_blocks_ids: Vec<BlockIdExt>,
    pipeline_context: PipelineContext,
    new_block_id_part: BlockIdExt,
    created_by: UInt256,
    after_merge: bool,
    after_split: bool,
    validator_set: ValidatorSet,

    // string with format like `-1:8000000000000000, 100500`, is used for logging.
    collated_block_descr: Arc<String>,
    collator_settings: CollatorSettings,

    debug: bool,
    rand_seed: UInt256,

    started: Instant,
    stop_flag: Arc<AtomicBool>,
}

impl Collator {
    pub fn new(
        shard: ShardIdent,
        min_mc_seqno: u32,
        prev_blocks_history: &PrevBlockHistory,
        pipeline_context: PipelineContext,
        validator_set: ValidatorSet,
        created_by: UInt256,
        engine: Arc<dyn EngineOperations>,
        rand_seed: Option<UInt256>,
        collator_settings: CollatorSettings,
    ) -> Result<Self> {
        let prev_blocks_ids = prev_blocks_history.get_prevs().to_vec();
        let collated_block_descr = Arc::new(prev_blocks_history.get_next_block_descr(None));
        log::trace!("{}: new", collated_block_descr);
        log::debug!("{} pipeline context: {}", collated_block_descr, pipeline_context);
        log::debug!("{} prev_blocks_ids:{}", collated_block_descr, prev_blocks_history);

        let new_block_seqno = match prev_blocks_ids.len() {
            1 => prev_blocks_ids[0].seq_no() + 1,
            2 => max(prev_blocks_ids[0].seq_no(), prev_blocks_ids[1].seq_no()) + 1,
            _ => fail!("`prev_blocks_ids` has invalid length"),
        };

        if prev_blocks_history.get_next_seqno() != Some(new_block_seqno) {
            fail!(
                "`prev_blocks_history.get_next_seqno()`={:?} is not equal to `new_block_seqno`={}",
                prev_blocks_history.get_next_seqno(),
                new_block_seqno
            )
        }

        // check inputs

        if !shard.is_masterchain() && !shard.is_standard_workchain() {
            fail!(
                "Collator can create block candidates \
                only for masterchain (-1) and base workchain (0)"
            )
        }
        if shard.is_masterchain() && !shard.is_masterchain_ext() {
            fail!("Sub-shards cannot exist in the masterchain")
        }
        let mut after_merge = false;
        let mut after_split = false;
        if prev_blocks_ids.len() == 2 {
            if shard.is_masterchain() {
                fail!("cannot merge shards in masterchain")
            }
            if !(prev_blocks_ids.iter().all(|id| shard.is_parent_for(id.shard()))
                && prev_blocks_ids[0].shard().shard_prefix_with_tag()
                    < prev_blocks_ids[1].shard().shard_prefix_with_tag())
            {
                fail!(
                    "The two previous blocks for a merge operation are not siblings or are not \
                    children of current shard"
                );
            }
            if prev_blocks_ids.iter().any(|id| id.seq_no() == 0) {
                fail!("previous blocks for a block merge operation must have non-zero seqno");
            }
            after_merge = true;
        } else {
            CHECK!(prev_blocks_ids.len(), 1);
            if *prev_blocks_ids[0].shard() != shard {
                if !prev_blocks_ids[0].shard().is_ancestor_for(&shard) {
                    fail!(
                        "Previous block does not belong \
                        to the shard we are generating a new block for"
                    );
                }
                if shard.is_masterchain() {
                    fail!("cannot split shards in masterchain");
                }
                after_split = true;
            }
            if shard.is_masterchain() && min_mc_seqno > prev_blocks_ids[0].seq_no() {
                fail!(
                    "cannot refer to specified masterchain block because it is later than \
                    the immediately preceding masterchain block"
                );
            }
        }

        let rand_seed = rand_seed.unwrap_or_else(|| secure_256_bits().into());

        Ok(Self {
            new_block_id_part: BlockIdExt {
                shard_id: shard.clone(),
                seq_no: new_block_seqno,
                root_hash: UInt256::default(),
                file_hash: UInt256::default(),
            },
            engine,
            shard,
            min_mc_seqno,
            prev_blocks_ids,
            pipeline_context,
            created_by,
            after_merge,
            after_split,
            validator_set,
            collated_block_descr,
            collator_settings,
            debug: true,
            rand_seed,
            started: Instant::now(),
            stop_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn collate(mut self) -> Result<CollateResult> {
        log::info!(
            "{}: COLLATE min_mc_seqno = {}, prev_blocks_ids: {} {}",
            self.collated_block_descr,
            self.min_mc_seqno,
            self.prev_blocks_ids[0],
            if self.prev_blocks_ids.len() > 1 {
                self.prev_blocks_ids[1].to_string()
            } else {
                "".to_string()
            }
        );
        self.init_timeout();

        let mut collator_data;
        let mut empty_attempt = 0;
        let mut error_attempt = 0;
        let mut duration;
        // inside the loop try to collate new block
        let (collate_result, exec_manager) = loop {
            let attempt_started = Instant::now();

            // load required data including masterchain and shards states
            let imported_data = self.import_data().await.inspect_err(|e| {
                log::warn!(
                    "{}: COLLATION FAILED: TIME: {}ms import_data: {:?}",
                    self.collated_block_descr,
                    self.started.elapsed().as_millis(),
                    e
                );
            })?;

            let mc_data;
            let prev_data;
            // unpack state, perform some checkes, import masterchain and shards blocks
            (mc_data, prev_data, collator_data) =
                self.prepare_data(imported_data, error_attempt).await.inspect_err(|e| {
                    log::warn!(
                        "{}: COLLATION FAILED: TIME: {}ms prepare_data: {:?}",
                        self.collated_block_descr,
                        self.started.elapsed().as_millis(),
                        e
                    );
                })?;

            // load messages and process them to produce block candidate
            let result = self.do_collate(&mc_data, &prev_data, &mut collator_data).await;
            duration = attempt_started.elapsed().as_millis() as u32;
            match result {
                Err(err) => {
                    log::warn!(
                        "{}: COLLATION FAILED: TIME: {}ms do_collate attempt {}: {:?}",
                        self.collated_block_descr,
                        self.started.elapsed().as_millis(),
                        error_attempt,
                        err
                    );

                    if error_attempt + 1 < MAX_ERROR_ATTEMPTS && self.check_stop_flag().is_ok() {
                        log::warn!("{}: Retrying block collation", self.collated_block_descr);
                        error_attempt += 1;
                        continue;
                    } else {
                        let collate_result =
                            CollateResult::Err { usage_tree: collator_data.usage_tree, err };
                        return Ok(collate_result);
                    }
                }
                Ok(Some(result)) => break result,
                Ok(None) => (),
            }

            // sleep after empty collation to respect the collation time iterval
            empty_attempt += 1;
            let sleep = self.engine.collator_config().empty_collation_sleep_ms;
            let sleep = if duration < sleep { sleep - duration } else { 0 };
            log::debug!(
                "{}: EMPTY COLLATION: \
                TIME: {duration}ms, attempt: {empty_attempt}, sleep for {sleep}ms...",
                self.collated_block_descr
            );

            tokio::time::sleep(Duration::from_millis(sleep as u64)).await;
        };

        let ratio = match duration {
            0 => collator_data.block_limit_status.gas_used(),
            duration => collator_data.block_limit_status.gas_used() / duration,
        };
        let pruned_count = collator_data.estimate_pruned_count();
        let estimate_size =
            collator_data.block_limit_status.estimate_block_size(None, pruned_count) as usize;

        if let CollateResult::Ok { candidate, .. } = &collate_result {
            log::info!(
                "{}: ASYNC COLLATED SIZE: {} ESTIMATEED SIZE: {} \
                GAS: {} TIME: {}ms GAS_RATE: {} TRANS: {}ms ID: {}",
                self.collated_block_descr,
                candidate.data.len(),
                estimate_size,
                collator_data.block_limit_status.gas_used(),
                duration,
                ratio,
                exec_manager.total_trans_duration.load(Ordering::Relaxed) / 1000,
                candidate.block_id,
            );

            if estimate_size > 400_000
                && 100 * estimate_size.abs_diff(candidate.data.len()) / estimate_size > 5
            {
                log::warn!("{}: diff is too much", self.collated_block_descr)
            }

            report_collation_metrics(
                &self.shard,
                collator_data.dequeue_count,
                collator_data.enqueue_count,
                collator_data.in_msg_count,
                collator_data.out_msg_count,
                collator_data.transit_count,
                collator_data.execute_count,
                collator_data.block_limit_status.gas_used(),
                ratio,
                candidate.data.len(),
                duration,
            );
        }

        #[cfg(not(test))]
        #[cfg(feature = "telemetry")]
        self.engine.collator_telemetry().succeeded_attempt(
            &self.shard,
            self.started.elapsed(),
            collator_data.execute_count as u32,
            collator_data.block_limit_status.gas_used(),
        );

        Ok(collate_result)
    }

    async fn import_data(&self) -> Result<ImportedData> {
        log::trace!("{}: import_data", self.collated_block_descr);

        if self.shard.is_masterchain() {
            let (prev_states, prev_ext_blocks_refs) = self.import_prev_stuff().await?;
            let top_shard_blocks_descr =
                self.engine.get_shard_blocks(&prev_states[0], None).await?;
            Ok(ImportedData {
                mc_state: prev_states[0].clone(),
                prev_states,
                prev_ext_blocks_refs,
                top_shard_blocks_descr,
            })
        } else {
            #[cfg(not(feature = "xp25"))]
            {
                let (mc_state, (prev_states, prev_ext_blocks_refs)) =
                    try_join!(self.import_mc_stuff(), self.import_prev_stuff())?;

                Ok(ImportedData {
                    mc_state,
                    prev_states,
                    prev_ext_blocks_refs,
                    top_shard_blocks_descr: Vec::new(),
                })
            }

            #[cfg(feature = "xp25")]
            loop {
                let (mc_state, (prev_states, prev_ext_blocks_refs)) =
                    try_join!(self.import_mc_stuff(), self.import_prev_stuff())?;

                let top_shard_blocks_descr = {
                    // Wait until all blocks referenced in master are applied.
                    //
                    // NOTE: sequential await is ok, because we are waiting for a notification,
                    // while the operation is initiated somewhere in the background.
                    for block_id in mc_state.shard_hashes()?.top_blocks_all()? {
                        if block_id.seq_no != 0 {
                            self.engine.wait_applied_block(&block_id, Some(1000)).await?;
                        }
                    }

                    let mut actual_mc_seqno = mc_state.block_id().seq_no;
                    match self.engine.get_shard_blocks(&mc_state, Some(&mut actual_mc_seqno)).await
                    {
                        Ok(top_blocks) => top_blocks,
                        Err(_) if actual_mc_seqno != mc_state.block_id().seq_no => continue,
                        Err(e) => return Err(e),
                    }
                };

                return Ok(ImportedData {
                    mc_state,
                    prev_states,
                    prev_ext_blocks_refs,
                    top_shard_blocks_descr,
                });
            }
        }
    }

    async fn prepare_data(
        &self,
        mut imported_data: ImportedData,
        error_attempt: u32,
    ) -> Result<(McData, PrevData, CollatorData)> {
        log::trace!("{}: prepare_data", self.collated_block_descr);

        self.check_stop_flag()?;

        CHECK!(imported_data.prev_states.len() == 1 + self.after_merge as usize);
        CHECK!(imported_data.prev_states.len() == self.prev_blocks_ids.len());

        CHECK!(imported_data.mc_state.block_id(), inited);

        let mc_data = self.unpack_last_mc_state(imported_data.mc_state)?;
        let state_root = self.unpack_last_state(&mc_data, &imported_data.prev_states)?;
        let _pure_states = imported_data.prev_states.clone();
        let usage_tree =
            self.create_usage_tree(state_root.clone(), &mut imported_data.prev_states)?;
        self.check_stop_flag()?;

        let subshard = match self.after_split {
            true => Some(&self.shard),
            false => None,
        };
        let prev_data = PrevData::from_prev_states(
            imported_data.prev_states,
            _pure_states,
            state_root,
            subshard,
        )?;
        let is_masterchain = self.shard.is_masterchain();
        self.check_stop_flag()?;

        let (now, now_ms) = self.init_utime(&mc_data, &prev_data)?;
        let config = BlockchainConfig::with_params(
            mc_data.config().capabilities(),
            supported_version(),
            mc_data.config().clone(),
        )?;
        let mut collator_data = CollatorData::new(
            now,
            now_ms,
            config,
            usage_tree,
            &prev_data,
            is_masterchain,
            self.collated_block_descr.clone(),
            error_attempt,
        )?;
        if !self.shard.is_masterchain() {
            let (now_upper_limit, before_split, _accept_msgs) = check_this_shard_mc_info(
                &self.shard,
                &self.new_block_id_part,
                self.after_merge,
                self.after_split,
                false,
                &self.prev_blocks_ids,
                collator_data.config.raw_config(),
                mc_data.mc_state_extra(),
                false,
                now,
            )?;
            collator_data.set_now_upper_limit(now_upper_limit);
            collator_data.set_before_split(before_split);
        }
        self.check_stop_flag()?;

        check_cur_validator_set(
            &self.validator_set,
            &self.new_block_id_part,
            &self.shard,
            mc_data.mc_state_extra(),
            mc_data.mc_state_extra().shards(),
            mc_data.state(),
            self.collator_settings.is_fake,
        )?;

        self.check_utime(&prev_data, &mut collator_data)?;

        if is_masterchain {
            self.adjust_shard_config(&mc_data, &mut collator_data)?;
            self.import_new_shard_top_blocks_for_masterchain(
                imported_data.top_shard_blocks_descr,
                &prev_data,
                &mc_data,
                &mut collator_data,
            )?;
        } else {
            #[cfg(feature = "xp25")]
            self.import_new_shard_top_blocks_for_shard(
                imported_data.top_shard_blocks_descr,
                &prev_data,
                &mc_data,
                &mut collator_data,
            )?;
        }

        self.init_lt(&mc_data, &prev_data, &mut collator_data)?;

        collator_data.prev_stuff = Some(BlkPrevInfo::new(imported_data.prev_ext_blocks_refs)?);

        Ok((mc_data, prev_data, collator_data))
    }

    async fn do_collate(
        &self,
        mc_data: &McData,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
    ) -> Result<Option<(CollateResult, ExecutionManager)>> {
        log::debug!("{}: do_collate", self.collated_block_descr);

        self.check_stop_flag()?;

        // loads out queues from neighbors and out queue of current shard
        let mut output_queue_manager =
            self.request_neighbor_msg_queues(mc_data, prev_data, collator_data).await?;

        collator_data.old_out_msg_queue_size =
            output_queue_manager.prev().out_queue_extra().out_queue_size();
        collator_data.out_msg_queue_size = collator_data.old_out_msg_queue_size;

        let mut out_queue_cleaned_partial = false;
        let mut out_queue_clean_deleted = 0;

        if !self.after_split {
            // delete delivered messages from output queue for a limited time
            let now = Instant::now();
            let cc = self.engine.collator_config();
            let clean_timeout_nanos = (cc.cutoff_timeout_ms as i128)
                * 1_000_000
                * (cc.clean_timeout_percentage_points as i128)
                / 1000;
            let processed;
            (out_queue_cleaned_partial, processed, out_queue_clean_deleted) = self
                .clean_out_msg_queue(
                    collator_data,
                    &mut output_queue_manager,
                    clean_timeout_nanos,
                    cc.optimistic_clean_percentage_points,
                )
                .await?;
            let elapsed = now.elapsed().as_millis();
            log::debug!(
                "{}: TIME: clean_out_msg_queue initial {}ms;",
                self.collated_block_descr,
                elapsed
            );
            let labels = [("shard", self.shard.to_string()), ("step", "initial".to_owned())];
            metrics::gauge!("ton_node_outqueue_clean_partial", &labels)
                .set(if out_queue_cleaned_partial { 1.0 } else { 0.0 });
            metrics::gauge!("ton_node_outqueue_clean_duration_seconds", &labels)
                .set(elapsed as f64);
            metrics::gauge!("ton_node_outqueue_clean_processed", &labels).set(processed as f64);
            metrics::gauge!("ton_node_outqueue_clean_deleted", &labels)
                .set(out_queue_clean_deleted as f64);
        } else {
            log::debug!(
                "{}: TIME: clean_out_msg_queue initial SKIPPED because of after_split block",
                self.collated_block_descr
            );
        }

        // copy out msg queue from next state which is cleared compared to previous
        collator_data.out_msg_queue_info = output_queue_manager.take_next();
        collator_data.out_msg_queue_info.forced_fix_out_queue()?;

        // compute created / minted / recovered / from_prev_blk
        self.update_value_flow(mc_data, prev_data, collator_data)?;

        let max_collate_threads = if self.collator_settings.lt_compatible {
            log::debug!(
                "{}: LT compatible mode, using single thread for collation",
                self.collated_block_descr
            );
            1
        } else {
            log::debug!(
                "{}: max_collate_threads {}",
                self.collated_block_descr,
                self.engine.collator_config().max_collate_threads
            );
            self.engine.collator_config().max_collate_threads as usize
        };
        let mut exec_manager = ExecutionManager::new(
            self.engine.clone(),
            collator_data.gen_utime(),
            collator_data.start_lt()?,
            self.rand_seed.clone(),
            mc_data,
            collator_data.config.clone(),
            max_collate_threads,
            self.collated_block_descr.clone(),
            self.debug,
            self.collator_settings.lt_compatible,
        )?;

        self.process_dispatch_queue(collator_data).await?;

        // tick & special transactions
        if self.shard.is_masterchain() {
            self.create_ticktock_transactions(false, prev_data, collator_data, &mut exec_manager)
                .await?;
            self.create_special_transactions(prev_data, collator_data, &mut exec_manager).await?;
        }

        // merge prepare / merge install
        // ** will be implemented later **

        if !self.after_split {
            // import inbound internal messages, process or transit
            let now = Instant::now();
            self.process_inbound_internal_messages(
                prev_data,
                collator_data,
                &output_queue_manager,
                &mut exec_manager,
            )
            .await?;
            log::debug!(
                "{}: TIME: process_inbound_internal_messages {}ms;",
                self.collated_block_descr,
                now.elapsed().as_millis()
            );

            // import inbound external messages (if space&gas left)
            let now = Instant::now();
            self.process_inbound_external_messages(prev_data, collator_data, &mut exec_manager)
                .await?;
            log::debug!(
                "{}: TIME: process_inbound_external_messages {}ms; messages left: {}",
                self.collated_block_descr,
                now.elapsed().as_millis(),
                self.engine.get_external_messages_len(),
            );
            metrics::histogram!("ton_node_collator_process_ext_messages_seconds")
                .record(now.elapsed());

            // process newly-generated messages (if space&gas left)
            // (if we were unable to process all inbound messages, all new messages must be queued)
            let now = Instant::now();
            collator_data.enqueue_only = !collator_data.inbound_queues_empty;
            self.process_new_messages(prev_data, collator_data, &mut exec_manager).await?;
            log::debug!(
                "{}: TIME: process_new_messages {}ms;",
                self.collated_block_descr,
                now.elapsed().as_millis()
            );
            metrics::histogram!("ton_node_collator_process_new_messages_seconds")
                .record(now.elapsed());
        } else {
            log::debug!(
                "{}: messages processing SKIPPED because of after_split block",
                self.collated_block_descr
            );
        }

        let clean_remaining_timeout_nanos = self.get_remaining_clean_time_limit_nanos();

        if !collator_data.block_full
            && out_queue_cleaned_partial
            && out_queue_clean_deleted == 0
            && clean_remaining_timeout_nanos > 10_000_000
        {
            if !self.after_split {
                // we have collation time left and out msg queue was not fully processed
                // so will try to clean more for a remaining time only by random algorithm
                let now = Instant::now();

                // set current out msg queue to manager to process new clean
                *output_queue_manager.next_mut() = mem::take(&mut collator_data.out_msg_queue_info);

                let processed;
                (out_queue_cleaned_partial, processed, out_queue_clean_deleted) = self
                    .clean_out_msg_queue(
                        collator_data,
                        &mut output_queue_manager,
                        clean_remaining_timeout_nanos,
                        0,
                    )
                    .await?;
                let elapsed = now.elapsed().as_millis();
                log::debug!(
                    "{}: TIME: clean_out_msg_queue remaining {}ms;",
                    self.collated_block_descr,
                    elapsed
                );
                let labels = [("shard", self.shard.to_string()), ("step", "remaining".to_owned())];
                metrics::gauge!("ton_node_outqueue_clean_partial", &labels)
                    .set(if out_queue_cleaned_partial { 1.0 } else { 0.0 });
                metrics::gauge!("ton_node_outqueue_clean_duration_seconds", &labels)
                    .set(elapsed as f64);
                metrics::gauge!("ton_node_outqueue_clean_processed", &labels).set(processed as f64);
                metrics::gauge!("ton_node_outqueue_clean_deleted", &labels)
                    .set(out_queue_clean_deleted as f64);

                // copy out msg queue from manager after clean
                collator_data.out_msg_queue_info = output_queue_manager.take_next();
                collator_data.out_msg_queue_info.forced_fix_out_queue()?;
            } else {
                log::debug!(
                    "{}: TIME: clean_out_msg_queue remaining SKIPPED because of after_split block",
                    self.collated_block_descr
                );
            }
        } else {
            let labels = [("shard", self.shard.to_string()), ("step", "remaining".to_owned())];
            metrics::gauge!("ton_node_outqueue_clean_partial", &labels).set(0.0);
            metrics::gauge!("ton_node_outqueue_clean_duration_seconds", &labels).set(0.0);
            metrics::gauge!("ton_node_outqueue_clean_processed", &labels).set(0.0);
            metrics::gauge!("ton_node_outqueue_clean_deleted", &labels).set(0.0);
        }

        // split prepare / split install
        // ** will be implemented later **

        // tock transactions
        if self.shard.is_masterchain() {
            self.create_ticktock_transactions(true, prev_data, collator_data, &mut exec_manager)
                .await?;
        }

        // process newly-generated messages (only by including them into output queue)
        collator_data.enqueue_only = true;
        self.process_new_messages(prev_data, collator_data, &mut exec_manager).await?;

        // If block is empty - stop collation to try one more time (may be there are some new messages)
        if self.is_empty_block(collator_data) {
            return Ok(None);
        }

        // update block history
        self.check_block_overload(collator_data, out_queue_cleaned_partial);

        // update processed upto
        self.update_processed_upto(mc_data, collator_data)?;

        //collator_data.block_limit_status.dump_block_size();

        // serialize everything
        let (collate_result, exec_manager) = self
            .finalize_block(mc_data, prev_data, collator_data, exec_manager, &output_queue_manager)
            .await?;

        Ok(Some((collate_result, exec_manager)))
    }

    fn is_empty_block(&self, collator_data: &CollatorData) -> bool {
        if self.shard.is_masterchain() {
            let cc = self.engine.collator_config_mc();
            cc.retry_if_empty
                && (self.started.elapsed().as_millis() as u32) < cc.finalize_empty_after_ms
                && collator_data.dequeue_count == 0
                && collator_data.enqueue_count == 0
                && collator_data.in_msg_count <= 1 // Any master block contains one special
                                                   // "recover" message, don't count it here
                && collator_data.out_msg_count == 0
                && collator_data.transit_count == 0
                && collator_data.remove_count == 0
                && collator_data.execute_count <= 5 // Master blocks usually process 2 tick, 2 tock
                                                    // and 1 special transactions, don't count them here
                && !collator_data.shard_conf_adjusted
        } else {
            let cc = self.engine.collator_config();
            !self.after_split
                && cc.retry_if_empty
                && (self.started.elapsed().as_millis() as u32) < cc.finalize_empty_after_ms
                && collator_data.dequeue_count == 0
                && collator_data.enqueue_count == 0
                && collator_data.in_msg_count == 0
                && collator_data.out_msg_count == 0
                && collator_data.transit_count == 0
                && collator_data.remove_count == 0
                && collator_data.execute_count == 0
        }
    }

    async fn clean_out_msg_queue(
        &self,
        collator_data: &mut CollatorData,
        output_queue_manager: &mut MsgQueueManager,
        clean_timeout_nanos: i128,
        optimistic_clean_percentage_points: u32,
    ) -> Result<(bool, i32, i32)> {
        log::debug!("{}: clean_out_msg_queue", self.collated_block_descr);
        // log::debug!("{}: clean_out_msg_queue {}", self.collated_block_descr, output_queue_manager.next().out_queue()?.len()?);
        let short = collator_data.config.has_capability(GlobalCapabilities::CapShortDequeue);
        let result = output_queue_manager
            .clean_out_msg_queue(
                clean_timeout_nanos,
                optimistic_clean_percentage_points,
                |enq, deliver_lt, root| {
                    self.check_stop_flag()?;
                    collator_data.out_msg_queue_size -= 1;
                    if let Some(deliver_lt) = deliver_lt {
                        log::trace!(
                            "{}: dequeue message: {:x}",
                            self.collated_block_descr,
                            enq.message_hash()
                        );
                        collator_data.dequeue_message(&enq, deliver_lt, short)?;
                        collator_data.block_limit_status.register_out_msg_queue_op(
                            root,
                            &collator_data.usage_tree,
                            false,
                        )?;
                    } else {
                        // let bytes = enq.enqueued().write_to_bytes()?;
                        // log::trace!("{}: remove split message: {:x} size: {}", self.collated_block_descr, enq.message_hash(), bytes.len());
                        log::trace!(
                            "{}: remove split message: {:x} ",
                            self.collated_block_descr,
                            enq.message_hash()
                        );
                        collator_data.block_limit_status.register_remove_split_msg();
                        collator_data.remove_count += 1;
                    }
                    // normal limit reached, but we can add for soft and hard limit
                    let stop = !collator_data.limit_fits(ParamLimitIndex::Normal);
                    Ok(stop)
                },
            )
            .await?;
        let root = output_queue_manager.next().out_queue().data();
        collator_data.block_limit_status.register_out_msg_queue_op(
            root,
            &collator_data.usage_tree,
            true,
        )?;
        Ok(result)
    }

    //
    // import
    //

    async fn import_mc_stuff(&self) -> Result<Arc<ShardStateStuff>> {
        log::trace!("{}: import_mc_stuff", self.collated_block_descr);
        let mc_state = self.engine.load_last_applied_mc_state().await?;

        if mc_state.block_id().seq_no() < self.min_mc_seqno {
            fail!(
                "requested to create a block referring to a non-existent future masterchain block"
            );
        }
        Ok(mc_state)
    }

    async fn import_prev_stuff(&self) -> Result<(Vec<Arc<ShardStateStuff>>, Vec<ExtBlkRef>)> {
        log::trace!("{}: import_prev_stuff", self.collated_block_descr);
        let mut prev_states = vec![];
        let mut prev_ext_blocks_refs = vec![];
        for (i, prev_id) in self.prev_blocks_ids.iter().enumerate() {
            let prev_state = match self.pipeline_context.try_get_state(prev_id) {
                Some(state) => state,
                None => self.engine.clone().wait_state(prev_id, Some(1_000), true).await?,
            };

            let end_lt = prev_state.state()?.gen_lt();
            let ext_block_ref = ExtBlkRef {
                end_lt,
                seq_no: prev_id.seq_no,
                root_hash: prev_id.root_hash.clone(),
                file_hash: prev_id.file_hash.clone(),
            };
            prev_ext_blocks_refs.push(ext_block_ref);

            if log::log_enabled!(log::Level::Trace) {
                if self.prev_blocks_ids.len() > 1 {
                    log::trace!(
                        "{}: processed upto from {}",
                        self.collated_block_descr,
                        prev_state.shard()
                    );
                }
                prev_state.proc_info()?.iterate_with_keys(|key: ProcessedInfoKey, value| {
                    log::trace!(
                        "{}: prev processed upto {} {:x} - {} {:x}",
                        self.collated_block_descr,
                        key.mc_seqno,
                        key.shard,
                        value.last_msg_lt,
                        value.last_msg_hash
                    );
                    Ok(true)
                })?;
            }

            prev_states.push(prev_state);
            if self.shard.is_masterchain() && prev_states[i].block_id().seq_no() < self.min_mc_seqno
            {
                fail!(
                    "requested to create a block referring to \
                    a non-existent future masterchain block"
                );
            }
        }
        Ok((prev_states, prev_ext_blocks_refs))
    }

    //
    // prepare
    //

    fn unpack_last_mc_state(&self, mc_state: Arc<ShardStateStuff>) -> Result<McData> {
        log::trace!("{}: unpack_last_mc_state", self.collated_block_descr);

        let mc_data = McData::new(mc_state)?;

        // capabilities & global version
        if mc_data.config().has_capabilities()
            && (0 != (mc_data.config().capabilities() & !supported_capabilities()))
        {
            fail!(
                "block generation capabilities {:016x} have been enabled in global configuration, \
                but we support only {:016x} (upgrade validator software?)",
                mc_data.config().capabilities(),
                supported_capabilities()
            );
        }
        if mc_data.config().global_version() > supported_version() {
            fail!(
                "block version {} have been enabled in global configuration, \
                but we support only {} (upgrade validator software?)",
                mc_data.config().global_version(),
                supported_version()
            );
        }

        Ok(mc_data)
    }

    fn unpack_last_state(
        &self,
        mc_data: &McData,
        prev_states: &[Arc<ShardStateStuff>],
    ) -> Result<Cell> {
        log::trace!("{}: unpack_last_state", self.collated_block_descr);
        for state in prev_states.iter() {
            self.check_one_state(mc_data, state)?;
        }
        if self.after_merge {
            ShardStateStuff::construct_split_root(
                prev_states[0].root_cell().clone(),
                prev_states[1].root_cell().clone(),
            )
        } else {
            Ok(prev_states[0].root_cell().clone())
        }
    }

    fn check_one_state(&self, mc_data: &McData, state: &Arc<ShardStateStuff>) -> Result<()> {
        log::trace!("{}: check_one_state {}", self.collated_block_descr, state.block_id());
        if state.state()?.vert_seq_no() > mc_data.vert_seq_no()? {
            fail!(
                "cannot create new block with vertical seqno {} prescribed by the current \
                masterchain configuration because the previous state of shard {} \
                has larger vertical seqno {}",
                mc_data.vert_seq_no()?,
                state.block_id().shard(),
                state.state()?.vert_seq_no()
            );
        }
        Ok(())
    }

    // create usage tree and recreate prev states with usage tree
    fn create_usage_tree(
        &self,
        state_root: Cell,
        prev_states: &mut Vec<Arc<ShardStateStuff>>,
    ) -> Result<UsageTree> {
        log::trace!("{}: create_usage_tree", self.collated_block_descr);
        let usage_tree = UsageTree::with_params(state_root, true);
        let root_cell = usage_tree.root_cell();
        *prev_states = if prev_states.len() == 2 {
            let ss_split = ShardStateSplit::construct_from_cell(root_cell.clone())?;
            vec![
                ShardStateStuff::from_root_cell(
                    prev_states[0].block_id().clone(),
                    ss_split.left,
                    #[cfg(feature = "telemetry")]
                    self.engine.engine_telemetry(),
                    self.engine.engine_allocated(),
                )?,
                ShardStateStuff::from_root_cell(
                    prev_states[1].block_id().clone(),
                    ss_split.right,
                    #[cfg(feature = "telemetry")]
                    self.engine.engine_telemetry(),
                    self.engine.engine_allocated(),
                )?,
            ]
        } else {
            vec![ShardStateStuff::from_root_cell(
                prev_states[0].block_id().clone(),
                root_cell.clone(),
                #[cfg(feature = "telemetry")]
                self.engine.engine_telemetry(),
                self.engine.engine_allocated(),
            )?]
        };
        Ok(usage_tree)
    }

    fn init_utime(&self, mc_data: &McData, prev_data: &PrevData) -> Result<(u32, u64)> {
        // consider unixtime and lt from previous block(s) of the same shardchain
        let prev_now = prev_data.prev_state_utime();
        let prev = max(mc_data.state().state()?.gen_time(), prev_now);
        log::trace!("{}: init_utime prev_time: {}", self.collated_block_descr, prev);
        let allow_same_timestamp = self.allow_same_timestamp(mc_data);
        let now_ms = self.collator_settings.min_gen_utime_ms.map_or_else(
            || self.engine.now_ms(),
            |min_now_ms| self.engine.now_ms().max(min_now_ms),
        );
        // Compute gen_utime_ms first, then derive gen_utime from it (like C++).
        // This guarantees gen_utime_ms / 1000 == gen_utime, avoiding second-boundary
        // mismatches in ConsensusExtraData validation.
        let (gen_utime, gen_utime_ms) = Self::calc_utime(prev, now_ms, allow_same_timestamp);
        Ok((gen_utime, gen_utime_ms))
    }

    /// Whether this shard is allowed to have `gen_utime` equal to the previous one.
    ///
    /// C++ parity: `allow_same_timestamp_ = global_version_ >= 13`.
    /// Depends only on the global protocol version, not on consensus type.
    /// When false (global_version < 13), gen_utime must strictly increase (prev + 1).
    /// When true, gen_utime may equal the previous block's (non-decreasing).
    fn allow_same_timestamp(&self, mc_data: &McData) -> bool {
        #[cfg(feature = "xp25")]
        {
            let _ = mc_data;
            true
        }
        #[cfg(not(feature = "xp25"))]
        {
            mc_data.config().global_version()
                >= super::SIMPLEX_ALLOW_SAME_TIMESTAMP_FROM_GLOBAL_VERSION
        }
    }

    /// Compute gen_utime_ms and gen_utime from previous block time and current wall clock.
    ///
    /// Mirrors C++ collator.cpp:
    /// ```cpp
    /// now_ms_ = std::max((td::uint64)(prev + (allow_same ? 0 : 1)) * 1000,
    ///                    (td::uint64)(td::Clocks::system() * 1000));
    /// now_ = (UnixTime)(now_ms_ / 1000);
    /// ```
    ///
    /// By computing milliseconds first and deriving seconds, we guarantee
    /// `gen_utime_ms / 1000 == gen_utime` always holds.
    #[inline]
    fn calc_utime(prev: u32, now_ms: u64, allow_same_timestamp: bool) -> (u32, u64) {
        let prev_sec = if allow_same_timestamp {
            // Non-decreasing: do NOT force +1 when blocks are produced faster than 1/sec.
            prev
        } else {
            // Strictly increasing gen_utime (legacy behavior), saturating at u32::MAX.
            prev.saturating_add(1)
        };
        let prev_ms = prev_sec as u64 * 1000;
        // Clamp to u32::MAX seconds range to prevent wraparound on cast.
        let max_ms = u32::MAX as u64 * 1000;
        let gen_utime_ms = max(prev_ms, now_ms).min(max_ms);
        let gen_utime = (gen_utime_ms / 1000) as u32;
        (gen_utime, gen_utime_ms)
    }

    fn check_utime(&self, prev_data: &PrevData, collator_data: &mut CollatorData) -> Result<()> {
        let now = collator_data.gen_utime;
        if now > collator_data.now_upper_limit() {
            fail!(
                "error initializing unix time for the new block: \
                failed to observe end of fsm_split time interval for this shard"
            );
        }

        // check whether masterchain catchain rotation is overdue
        let prev_now = prev_data.prev_state_utime();
        let ccvc = collator_data.config.raw_config().catchain_config()?;
        let lifetime = ccvc.mc_catchain_lifetime;
        if self.shard.is_masterchain()
            && now / lifetime > prev_now / lifetime
            && now > (prev_now / lifetime + 1) * lifetime + 20
        {
            let overdue = now - (prev_now / lifetime + 1) * lifetime;
            let mut rng = rand::thread_rng();
            let skip_topmsgdescr = rng.gen_range(0..1024) < 256; // probability 1/4
            let skip_extmsg = rng.gen_range(0..1024) < 256; // skip ext msg probability 1/4
            if skip_topmsgdescr {
                collator_data.set_skip_topmsgdescr();
                log::warn!(
                    "{}: randomly skipping import of new shard data because of \
                    overdue masterchain catchain rotation (overdue by {} seconds)",
                    self.collated_block_descr,
                    overdue
                );
            }
            if skip_extmsg {
                collator_data.set_skip_extmsg();
                log::warn!(
                    "{}: randomly skipping external message import because of \
                    overdue masterchain catchain rotation (overdue by {} seconds)",
                    self.collated_block_descr,
                    overdue
                );
            }
        } else if self.shard.is_masterchain() && now > prev_now + 60 {
            let interval = now - prev_now;
            let mut rng = rand::thread_rng();
            let skip_topmsgdescr = rng.gen_range(0..1024) < 128; // probability 1/8
            let skip_extmsg = rng.gen_range(0..1024) < 128; // skip ext msg probability 1/8
            if skip_topmsgdescr {
                collator_data.set_skip_topmsgdescr();
                log::warn!(
                    "{}: randomly skipping import of new shard data because of \
                    overdue masterchain block (last block was {} seconds ago)",
                    self.collated_block_descr,
                    interval
                );
            }
            if skip_extmsg {
                collator_data.set_skip_extmsg();
                log::warn!(
                    "{}: randomly skipping external message import because of \
                    overdue masterchain block (last block was {} seconds ago)",
                    self.collated_block_descr,
                    interval
                );
            }
        }
        Ok(())
    }

    fn init_lt(
        &self,
        mc_data: &McData,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!("{}: init_lt", self.collated_block_descr);

        #[cfg(not(feature = "xp25"))]
        let mut start_lt = if !self.shard.is_masterchain() {
            max(mc_data.state().state()?.gen_lt(), prev_data.prev_state_lt())
        } else {
            max(mc_data.state().state()?.gen_lt(), collator_data.shards_max_end_lt())
        };
        #[cfg(feature = "xp25")]
        let mut start_lt = max(
            max(mc_data.state().state()?.gen_lt(), prev_data.prev_state_lt()),
            collator_data.shards_max_end_lt(),
        );

        let align = mc_data.get_lt_align();
        let incr = align - start_lt % align;
        if incr < align || 0 == start_lt {
            if start_lt >= (!incr + 1) {
                fail!("cannot compute start logical time (uint64 overflow)");
            }
            start_lt += incr;
        }

        collator_data.set_start_lt(start_lt)?;
        log::debug!("{}: start_lt set to {}", self.collated_block_descr, start_lt);

        Ok(())
    }

    async fn request_neighbor_msg_queues(
        &self,
        mc_data: &McData,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
    ) -> Result<MsgQueueManager> {
        log::debug!("{}: request_neighbor_msg_queues", self.collated_block_descr);
        let states_manager = StatesManager::with_collator_data(
            self.engine.clone(),
            self.pipeline_context.clone(),
            collator_data.config.has_capability(GlobalCapabilities::CapFullCollatedData),
        )?;
        MsgQueueManager::init(
            &self.engine,
            mc_data.state(),
            self.shard.clone(),
            self.new_block_id_part.seq_no,
            collator_data.shards.as_ref().unwrap_or_else(|| mc_data.mc_state_extra.shards()),
            &prev_data.states,
            None,
            self.after_merge,
            self.after_split,
            Some(&self.stop_flag),
            Some(&collator_data.usage_tree),
            Some(&mut collator_data.imported_visited),
            Some(self.collated_block_descr.clone()),
            states_manager,
        )
        .await
    }

    fn adjust_shard_config(
        &self,
        mc_data: &McData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!("{}: adjust_shard_config", self.collated_block_descr);
        CHECK!(self.shard.is_masterchain());
        collator_data.set_shards(mc_data.state().shards()?.clone())?;
        let wc_set = collator_data.config.raw_config().workchains()?;
        wc_set.iterate_with_keys(|wc_id: i32, wc_info| {
            log::trace!(
                "
                {}: adjust_shard_config workchain {wc_id}, active {}, enabled_since {} (now {})",
                self.collated_block_descr,
                wc_info.active(),
                wc_info.enabled_since,
                collator_data.gen_utime
            );
            if wc_info.active()
                && wc_info.enabled_since <= collator_data.gen_utime
                && !collator_data.shards()?.contains(&wc_id)?
            {
                log::info!(
                    "{}: adjust_shard_config added new wc {wc_id}",
                    self.collated_block_descr
                );
                collator_data.set_shard_conf_adjusted();
                collator_data.shards_mut()?.add_workchain(
                    wc_id,
                    self.new_block_id_part.seq_no(),
                    wc_info.zerostate_root_hash,
                    wc_info.zerostate_file_hash,
                )?;
                collator_data.store_shard_fees_zero(&ShardIdent::with_workchain_id(wc_id)?)?;
                self.check_stop_flag()?;
            }
            Ok(true)
        })?;
        Ok(())
    }

    fn import_new_shard_top_blocks_for_masterchain(
        &self,
        mut shard_top_blocks: Vec<Arc<TopBlockDescrStuff>>,
        prev_data: &PrevData,
        mc_data: &McData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!("{}: import_new_shard_top_blocks_for_masterchain", self.collated_block_descr);

        if collator_data.skip_topmsgdescr() {
            log::warn!(
                "{}: import_new_shard_top_blocks_for_masterchain: SKIPPED",
                self.collated_block_descr
            );
            return Ok(());
        }

        let mut blocks_to_add = vec![];
        #[cfg(feature = "xp25")]
        let mut cancelled = false;
        let mut new_shard_descrs = collator_data.shards()?.clone();

        let lt_limit =
            prev_data.prev_state_lt() + collator_data.config.raw_config().get_max_lt_growth();
        shard_top_blocks.sort_by(|a, b| cmp_shard_block_descr(a, b));
        let mut shards_updated = HashSet::new();
        let mut tb_act = 0;
        let mut prev_bd = Option::<Arc<TopBlockDescrStuff>>::None;
        let mut prev_descr = Option::<McShardRecord>::None;
        let mut prev_shard = ShardIdent::default();
        let mut prev_chain_len = 0;
        for sh_bd in shard_top_blocks {
            self.check_stop_flag()?;
            let mut res_flags = 0;
            let now = Instant::now();

            let result = sh_bd.prevalidate(
                mc_data.state().block_id(),
                mc_data.state(),
                TbdMode::FAIL_NEW | TbdMode::FAIL_TOO_NEW,
                &mut res_flags,
            );
            log::trace!(
                "{}: prevalidate TIME: {}μ for {}",
                self.collated_block_descr,
                now.elapsed().as_micros(),
                sh_bd.proof_for().shard()
            );
            let chain_len = match result {
                Ok(len) => {
                    if len <= 0 || len > UNREGISTERED_CHAIN_MAX_LEN as i32 {
                        log::warn!(
                            "{}: ShardTopBlockDescr for {} skipped: its chain length is {}",
                            self.collated_block_descr,
                            sh_bd.proof_for(),
                            len
                        );
                        #[cfg(feature = "xp25")]
                        {
                            cancelled = true;
                            break;
                        }
                        #[cfg(not(feature = "xp25"))]
                        continue;
                    }
                    len as usize
                }
                Err(e) => {
                    log::warn!(
                        "{}: ShardTopBlockDescr for {} skipped: res_flags = {}, error: {}",
                        self.collated_block_descr,
                        sh_bd.proof_for(),
                        res_flags,
                        e
                    );
                    #[cfg(feature = "xp25")]
                    {
                        cancelled = true;
                        break;
                    }
                    #[cfg(not(feature = "xp25"))]
                    continue;
                }
            };
            let wrong_time;
            #[cfg(feature = "xp25")]
            {
                wrong_time = sh_bd.gen_utime() > collator_data.gen_utime
            };
            #[cfg(not(feature = "xp25"))]
            {
                wrong_time = sh_bd.gen_utime() >= collator_data.gen_utime
            };
            if wrong_time {
                log::debug!(
                    "{}: ShardTopBlockDescr for {} skipped: it claims to be generated at {} \
                    while it is still {}",
                    self.collated_block_descr,
                    sh_bd.proof_for(),
                    sh_bd.gen_utime(),
                    collator_data.gen_utime()
                );
                #[cfg(feature = "xp25")]
                {
                    cancelled = true;
                    break;
                }
                #[cfg(not(feature = "xp25"))]
                continue;
            }
            let mut descr = sh_bd.get_top_descr(chain_len)?;
            CHECK!(descr.block_id() == sh_bd.proof_for());
            let shard = descr.shard();
            let start_blks = sh_bd.get_prev_at(chain_len);
            let now = Instant::now();
            let result = may_update_shard_block_info(
                &mut new_shard_descrs,
                &descr,
                &start_blks,
                lt_limit,
                Some(&shards_updated),
            );
            log::trace!(
                "{}: may_update_shard_block_info TIME: {}μ for {}",
                self.collated_block_descr,
                now.elapsed().as_micros(),
                descr.shard()
            );
            match result {
                Err(e) => {
                    log::warn!(
                        "{}: cannot add new top shard block {} to shard configuration: {}",
                        self.collated_block_descr,
                        sh_bd.proof_for(),
                        e
                    );
                    #[cfg(feature = "xp25")]
                    {
                        cancelled = true;
                        break;
                    }
                    #[cfg(not(feature = "xp25"))]
                    continue;
                }
                Ok((true, _)) => {
                    // before split

                    CHECK!(start_blks.len() == 1);

                    if &prev_shard.sibling() == shard {
                        CHECK!(start_blks.len() == 1);
                        let prev_bd =
                            prev_bd.clone().ok_or_else(|| error!("Can't unwrap `prev_bd`"))?;
                        let start_blks2 = prev_bd.get_prev_at(prev_chain_len);
                        CHECK!(start_blks2.len() == 1);
                        CHECK!(start_blks == start_blks2);
                        let mut prev_descr = prev_descr
                            .clone()
                            .ok_or_else(|| error!("Can't unwrap `prev_descr`"))?;

                        prev_descr.descr.reg_mc_seqno = self.new_block_id_part.seq_no;
                        descr.descr.reg_mc_seqno = self.new_block_id_part.seq_no;
                        if let Err(e) = self.update_shard_block_info2(
                            &mut new_shard_descrs,
                            prev_descr.clone(),
                            descr.clone(),
                            &start_blks2,
                            Some(&mut shards_updated),
                        ) {
                            log::debug!(
                                "{}: cannot add new split top shard blocks {} and {} \
                                to shard configuration: {}",
                                self.collated_block_descr,
                                sh_bd.proof_for(),
                                prev_bd.proof_for(),
                                e
                            );
                            #[cfg(feature = "xp25")]
                            {
                                cancelled = true;
                                break;
                            }
                            #[cfg(not(feature = "xp25"))]
                            {
                                prev_shard = ShardIdent::default();
                            }
                        } else {
                            log::debug!(
                                "{}: updated top shard block information with {} and {}",
                                self.collated_block_descr,
                                sh_bd.proof_for(),
                                prev_bd.proof_for()
                            );
                            tb_act += 2;
                            prev_shard = ShardIdent::default();

                            blocks_to_add.push((prev_descr, prev_chain_len, prev_bd.clone()));
                            blocks_to_add.push((descr, chain_len, sh_bd.clone()));
                        }
                    } else if *shard == prev_shard {
                        log::debug!(
                            "{}: skip postponing new top shard block {}",
                            self.collated_block_descr,
                            sh_bd.proof_for()
                        );
                    } else {
                        log::debug!(
                            "{}: postpone adding new top shard block {}",
                            self.collated_block_descr,
                            sh_bd.proof_for()
                        );
                        prev_bd = Some(sh_bd);
                        prev_descr = Some(descr.clone());
                        prev_shard = shard.clone();
                        prev_chain_len = chain_len;
                    }
                }
                Ok((false, _)) => {
                    // not before split (usual block)

                    if prev_bd.is_some() {
                        prev_bd = None;
                        prev_descr = None;
                        prev_shard = ShardIdent::default();
                    }

                    descr.descr.reg_mc_seqno = self.new_block_id_part.seq_no;
                    let result = self.update_shard_block_info(
                        &mut new_shard_descrs,
                        descr.clone(),
                        &start_blks,
                        Some(&mut shards_updated),
                    );
                    if let Err(e) = result {
                        log::debug!(
                            "{}: cannot add new top shard block {} to shard configuration: {}",
                            self.collated_block_descr,
                            sh_bd.proof_for(),
                            e
                        );
                        #[cfg(feature = "xp25")]
                        {
                            cancelled = true;
                            break;
                        }
                    } else {
                        log::debug!(
                            "{}: updated top shard block information with {}",
                            self.collated_block_descr,
                            sh_bd.proof_for()
                        );
                        tb_act += 1;
                        blocks_to_add.push((descr, chain_len, sh_bd.clone()));
                    }
                }
            }
            if self.check_cutoff_timeout() {
                log::warn!(
                    "{}: TIMEOUT is elapsed, \
                    stop processing import_new_shard_top_blocks_for_masterchain",
                    self.collated_block_descr
                );
                #[cfg(feature = "xp25")]
                {
                    cancelled = true;
                }
                break;
            }
        }

        #[cfg(feature = "xp25")]
        if cancelled {
            // In case of XP25 all shards are already linked to each other,
            // so we can't include one and skip another.
            log::warn!(
                "{}: import_new_shard_top_blocks_for_masterchain: CANCELLED",
                self.collated_block_descr
            );
            return Ok(());
        }

        *collator_data.shards_mut()? = new_shard_descrs;

        for (descr, chain_len, tbd) in blocks_to_add {
            collator_data.store_shard_fees(&descr)?;
            collator_data.register_shard_block_creators(tbd.get_creator_list(chain_len)?)?;
            collator_data.update_shards_max_end_lt(descr.descr.end_lt);
            collator_data.add_top_block_descriptor(tbd);
        }

        if tb_act > 0 {
            collator_data.set_shard_conf_adjusted();
        }

        let shard_fees = collator_data.shard_fees().root_extra().clone();

        collator_data.value_flow.fees_collected.add(&shard_fees.fees)?;
        if let Some(burning_cfg) = collator_data.config.burning_config() {
            let Some(imported_base) =
                shard_fees.fees.coins.as_u128().checked_sub(shard_fees.create.coins.as_u128())
            else {
                fail!(
                    "fees_imported is smaller than imported created fees: {} < {}",
                    shard_fees.fees.coins,
                    shard_fees.create.coins
                )
            };
            let burned = burning_cfg.calculate_burned_fees(imported_base)?;
            if !burned.is_zero() {
                collator_data.value_flow.burned.coins.add(&burned)?;
                collator_data.value_flow.fees_collected.coins.sub(&burned)?;
            }
        }
        collator_data.value_flow.fees_imported = shard_fees.fees;

        Ok(())
    }

    #[cfg(feature = "xp25")]
    fn import_new_shard_top_blocks_for_shard(
        &self,
        shard_top_blocks: Vec<Arc<TopBlockDescrStuff>>,
        prev_data: &PrevData,
        mc_data: &McData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        use crate::validating_utils::build_shard_hashes_tree;
        use std::collections::hash_map;

        fn make_brief_shard_descr(shard_descr: &ShardDescr) -> ShardBlockRef {
            ShardBlockRef {
                seq_no: shard_descr.seq_no,
                root_hash: shard_descr.root_hash.clone(),
                file_hash: shard_descr.file_hash.clone(),
                end_lt: shard_descr.end_lt,
            }
        }

        enum PrevRefs {
            AfterZeroState,
            Single(RefShardBlocks),
            AfterMerge { left: RefShardBlocks, right: RefShardBlocks },
        }

        impl PrevRefs {
            fn new(
                new_block_id: &BlockIdExt,
                prev_states: &[Arc<ShardStateStuff>],
            ) -> Result<Self> {
                fn load_refs(s: &ShardStateStuff) -> Result<RefShardBlocks> {
                    Ok(s.state()?
                        .read_wc_custom()?
                        .ok_or_else(|| error!("No wc-custom in prev state"))?
                        .ref_shard_blocks)
                }

                if new_block_id.seq_no <= 1 {
                    return Ok(Self::AfterZeroState);
                }

                match prev_states {
                    [single] => load_refs(single.as_ref()).map(Self::Single),
                    [left, right] => Ok(Self::AfterMerge {
                        left: load_refs(left.as_ref())?,
                        right: load_refs(right.as_ref())?,
                    }),
                    _ => fail!("Invalid previous states"),
                }
            }

            fn fill_shard_top_blocks(
                &self,
                shard_ident: &ShardIdent,
                shard_descr: &ShardDescr,
                shard_top_blocks: &mut HashMap<u64, ShardBlockRef>,
            ) -> Result<()> {
                // Adds a new shard block ref
                let mut insert_top_block = |shard: &ShardIdent, top_blocks: ShardBlockRef| {
                    if top_blocks.seq_no <= shard_descr.seq_no {
                        // Masterchain already contains a newer state for this shard
                        return false;
                    }
                    match shard_top_blocks.entry(shard.shard_prefix_with_tag()) {
                        // Just insert a new block ref if it doesn't exist
                        hash_map::Entry::Vacant(entry) => {
                            entry.insert(top_blocks);
                            true
                        }
                        // Update block ref only with a newer one
                        hash_map::Entry::Occupied(mut entry) => {
                            let entry = entry.get_mut();
                            let newer = top_blocks.seq_no > entry.seq_no;
                            if newer {
                                *entry = top_blocks;
                            }
                            newer
                        }
                    }
                };

                // Tries to fill shard block references from the specified refs
                let mut update_ref = |refs: &RefShardBlocks| {
                    if shard_descr.before_split {
                        // Try to find left AND right shard block references if the shard is going to split
                        let (left, right) = shard_ident.split()?;
                        match (refs.ref_shard_block(&left)?, refs.ref_shard_block(&right)?) {
                            (Some(left_ref), Some(right_ref)) => {
                                return Ok(insert_top_block(&left, left_ref)
                                    || insert_top_block(&right, right_ref));
                            }
                            (None, None) => {}
                            _ => fail!("Invalid ref_shard_blocks in prev states"),
                        }
                    } else if shard_descr.before_merge {
                        // Try to find parent shard block reference if the shard is going to merge
                        let parent = shard_ident.merge()?;
                        if let Some(shard_ref) = refs.ref_shard_block(&parent)? {
                            return Ok(insert_top_block(&parent, shard_ref));
                        }
                    }

                    // Find the exact shard block reference if shard wasn't changed
                    Result::Ok(match refs.ref_shard_block(shard_ident)? {
                        Some(shard_ref) => insert_top_block(shard_ident, shard_ref),
                        None => false,
                    })
                };

                let updated = match self {
                    // Just use block ref from shard description for the first block
                    Self::AfterZeroState => false,
                    // Use references from the previous state
                    Self::Single(refs) => update_ref(refs)?,
                    // Use the latest references from the previous states
                    Self::AfterMerge { left, right } => update_ref(left)? || update_ref(right)?,
                };

                if !updated {
                    // Use a block reference from the shard description if no suitable refs were found
                    let top_block = make_brief_shard_descr(shard_descr);
                    shard_top_blocks.insert(shard_ident.shard_prefix_with_tag(), top_block);
                }

                Ok(())
            }
        }

        log::trace!("{}: import_new_shard_top_blocks_for_shard", self.collated_block_descr);

        // Group new resolved top block descriptions by workchain and shard
        let mut all_shard_top_blocks = HashMap::<i32, HashMap<u64, _>>::new();
        for tbds in shard_top_blocks {
            collator_data.update_shards_max_end_lt(tbds.end_lt());
            // Don't add top block descriptions for the own shard (we trust ourself)
            if !self.shard.intersect_with(tbds.proof_for().shard()) {
                let id = tbds.proof_for().clone();
                log::trace!(
                    "{}: import_new_shard_top_blocks_for_shard add tbd {}",
                    self.collated_block_descr,
                    id
                );
                let value = ShardBlockRef {
                    seq_no: id.seq_no,
                    file_hash: id.file_hash,
                    root_hash: id.root_hash,
                    end_lt: tbds.end_lt(),
                };
                all_shard_top_blocks
                    .entry(id.shard_id.workchain_id())
                    .or_default()
                    .insert(id.shard_id.shard_prefix_with_tag(), value);
                collator_data.add_top_block_descriptor(tbds);
            }
        }

        // Add prev refs to all_shard_top_blocks
        for prev_state in &prev_data._pure_states {
            let id = prev_state.block_id();
            let value = ShardBlockRef {
                seq_no: id.seq_no,
                file_hash: id.file_hash.clone(),
                root_hash: id.root_hash.clone(),
                end_lt: prev_state.state()?.gen_lt(),
            };
            all_shard_top_blocks
                .entry(id.shard_id.workchain_id())
                .or_default()
                .insert(id.shard_id.shard_prefix_with_tag(), value);
            log::trace!(
                "{}: import_new_shard_top_blocks_for_shard add prev {}",
                self.collated_block_descr,
                id
            );
        }

        // Adjust shard top blocks to contain a full set of shard ids
        let prev_refs = PrevRefs::new(&self.new_block_id_part, &prev_data.states)?;
        mc_data.state().shards()?.iterate_shards(|shard, descr| {
            let shard_top_blocks = all_shard_top_blocks.entry(shard.workchain_id()).or_default();

            // Check whether resolved map contains all shards
            let has_top_block = if descr.before_split {
                let (left, right) = shard.split()?;
                let has_top_block = shard_top_blocks.contains_key(&left.shard_prefix_with_tag())
                    && shard_top_blocks.contains_key(&right.shard_prefix_with_tag());

                if !has_top_block {
                    shard_top_blocks.remove(&left.shard_prefix_with_tag());
                    shard_top_blocks.remove(&right.shard_prefix_with_tag());
                }

                has_top_block
            } else if descr.before_merge {
                let shard = shard.merge()?;
                shard_top_blocks.contains_key(&shard.shard_prefix_with_tag())
            } else {
                shard_top_blocks.contains_key(&shard.shard_prefix_with_tag())
            };

            if !has_top_block {
                prev_refs.fill_shard_top_blocks(&shard, &descr, shard_top_blocks)?;
            }

            Ok(true)
        })?;

        // Build shard hashes tree
        let shards = build_shard_hashes_tree(all_shard_top_blocks)?;
        collator_data.set_shards(shards)?;
        Ok(())
    }

    //
    // collate
    //
    fn update_value_flow(
        &self,
        mc_data: &McData,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!("{}: update_value_flow", self.collated_block_descr);

        if self.shard.is_masterchain() {
            collator_data.value_flow.created.coins =
                collator_data.config.raw_config().block_create_fees(true)?;

            collator_data.value_flow.recovered = collator_data.value_flow.created.clone();
            collator_data.value_flow.recovered.add(&collator_data.value_flow.fees_collected)?;
            collator_data
                .value_flow
                .recovered
                .add(mc_data.state().state()?.total_validator_fees())?;

            match collator_data.config.raw_config().fee_collector_address() {
                Err(_) => {
                    log::debug!(
                        "{}: fee recovery disabled \
                       (no collector smart contract defined in configuration)",
                        self.collated_block_descr
                    );
                    collator_data.value_flow.recovered = CurrencyCollection::default();
                }
                Ok(_addr) => {
                    if collator_data.value_flow.recovered.coins.as_u128() < 1_000_000_000 {
                        log::debug!(
                            "{}: fee recovery skipped ({})",
                            self.collated_block_descr,
                            collator_data.value_flow.recovered
                        );
                        collator_data.value_flow.recovered = CurrencyCollection::default();
                    }
                }
            };

            collator_data.value_flow.minted = self.compute_minted_amount(mc_data, collator_data)?;

            if !collator_data.value_flow.minted.is_zero()?
                && collator_data.config.raw_config().minter_address().is_err()
            {
                log::warn!(
                    "{}: minting of {} disabled: no minting smart contract defined",
                    self.collated_block_descr,
                    collator_data.value_flow.minted
                );
                collator_data.value_flow.minted = CurrencyCollection::default();
            }
        } else {
            collator_data.value_flow.created.coins =
                collator_data.config.raw_config().block_create_fees(false)?;
            collator_data.value_flow.created.coins >>= self.shard.prefix_len();
        }
        collator_data.value_flow.from_prev_blk = prev_data.total_balance().clone();
        Ok(())
    }

    fn compute_minted_amount(
        &self,
        mc_data: &McData,
        collator_data: &CollatorData,
    ) -> Result<CurrencyCollection> {
        log::trace!("{}: compute_minted_amount", self.collated_block_descr);

        CHECK!(self.shard.is_masterchain());
        let mut to_mint = CurrencyCollection::default();

        let to_mint_cp = match collator_data.config.raw_config().to_mint() {
            Err(e) => {
                log::warn!(
                    "{}: Can't get config param 7 (to_mint): {}",
                    self.collated_block_descr,
                    e
                );
                return Ok(to_mint);
            }
            Ok(v) => v,
        };

        let old_global_balance = mc_data.global_balance();
        to_mint_cp.iterate_with_keys(|key: u32, amount| {
            let amount2 = old_global_balance.get_other(key)?.unwrap_or_default();
            if amount > amount2 {
                let mut delta = amount.clone();
                delta.sub(&amount2)?;
                log::debug!(
                    "{}: currency #{}: existing {}, required {}, to be minted {}",
                    self.collated_block_descr,
                    key,
                    amount2,
                    amount,
                    delta
                );
                if key != 0 {
                    to_mint.set_other_ex(key, &delta)?;
                }
            }
            Ok(true)
        })?;

        Ok(to_mint)
    }

    fn process_deferred_message(
        &self,
        account_id: &AccountId,
        lt: u64,
        collator_data: &mut CollatorData,
    ) -> Result<(MsgEnqueueStuff, AccountDispatchQueue)> {
        // remove message from main dispatch queue
        let dispatch_queue =
            &mut collator_data.out_msg_queue_info.out_queue_extra_mut().dispatch_queue;
        let Some(mut acc_queue) = dispatch_queue.get(account_id)? else {
            fail!("No dispatch queue for account {account_id:x}")
        };
        let enq = acc_queue.remove(lt)?;
        if acc_queue.is_empty() {
            dispatch_queue.remove(account_id.clone())?;
        } else {
            dispatch_queue.set_augmentable(account_id, &acc_queue)?;
        }
        if enq.envelope_cell().level() != 0 {
            fail!("cannot import a message with non-zero level!");
        }
        collator_data.add_sender_generated_message(account_id.clone());
        let mut env = enq.read_envelope_msg()?;
        let message = env.read_message()?;
        let header =
            message.int_header().ok_or_else(|| error!("deferred message is not internal"))?;
        if lt != header.created_lt {
            fail!("deferred internal message has augmentation lt mismatch");
        }
        if env.fwd_fee_remaining() > header.fwd_fee() {
            fail!("deferred internal message has fwd_fee_remaining > original fwd_fee");
        }
        // TODO: validate message libraries
        if !env.cur_addr().is_zero() || !env.next_addr().is_zero() {
            fail!("message in DispatchQueue is expected to have zero cur_addr and next_addr");
        }

        // 5. calculate emitted_lt
        let mut emitted_lt = collator_data.start_lt()?;
        if let Some(lt) = collator_data.last_dispatch_queue_emitted_lt.get(account_id) {
            emitted_lt = emitted_lt.max(*lt);
        }
        emitted_lt += 1;
        collator_data.last_dispatch_queue_emitted_lt.insert(account_id.clone(), emitted_lt);

        env.set_emitted_lt(emitted_lt);
        let env = MsgEnvelopeStuff::from_envelope(env)?;
        log::info!(
            "delivering deferred message from account {account_id:x}, \
            lt={lt}, emitted_lt={emitted_lt} with hash: {:x} and prefix: {} initiator lt: {}",
            env.message_hash(),
            env.dst_prefix(),
            env.metadata().map_or(0, |metadata| metadata.initiator_lt())
        );
        collator_data.register_dispatch_queue_op(false)?;
        collator_data.add_unprocessed_deferred_message(account_id.clone());
        // LT - emitted_lt
        Ok((MsgEnqueueStuff::from_envelope(env, emitted_lt)?, acc_queue))
    }

    /// put new messages from dispatch queue to processing
    /// LT will be set for each message
    /// All messages must be processed in this block
    async fn process_dispatch_queue(&self, collator_data: &mut CollatorData) -> Result<()> {
        log::debug!("{}: process_dispatch_queue", self.collated_block_descr);
        let hard_defer_out_queue_size_limit =
            collator_data.config.size_limits_config().defer_out_queue_size_limit as usize;
        let defer_out_queue_size_limit = hard_defer_out_queue_size_limit
            .max(self.engine.collator_config().defer_out_queue_size_limit);
        if collator_data.out_msg_queue_size > defer_out_queue_size_limit
            && collator_data.old_out_msg_queue_size > hard_defer_out_queue_size_limit
        {
            return Ok(());
        }
        collator_data.have_unprocessed_account_dispatch_queue = true;
        let max_total_count = [
            1 << 30,
            self.engine.collator_config().dispatch_phase_2_max_total,
            self.engine.collator_config().dispatch_phase_3_max_total,
        ];
        let dispatch_phase_3_max_per_initiator =
            if self.engine.collator_config().dispatch_phase_3_max_per_initiator != 0 {
                self.engine.collator_config().dispatch_phase_3_max_per_initiator
            } else if collator_data.out_msg_queue_size < 256 {
                10
            } else if collator_data.out_msg_queue_size < 512 {
                2
            } else if collator_data.out_msg_queue_size < 1500 {
                1
            } else {
                0
            };
        let max_per_initiator = [
            1 << 30,
            self.engine.collator_config().dispatch_phase_2_max_per_initiator,
            dispatch_phase_3_max_per_initiator,
        ];
        let mut priority_list = CycleVec::from_slice(&self.engine.collator_config().priority_list);
        let mut total_count = 0;
        let iter = max_total_count.iter().zip(max_per_initiator.iter()).enumerate();
        for (iter, (max_total_count, max_per_initiator)) in iter {
            log::debug!(
                "{}: process_dispatch_queue iter: {iter}, max_total_count: {max_total_count}, max_per_initiator: {max_per_initiator}",
                self.collated_block_descr,
            );
            if max_total_count == &0 || max_per_initiator == &0 {
                continue;
            }
            let mut count_per_initiator = HashMap::<(i32, SliceData, u64), usize>::new();
            if iter != 0 && collator_data.error_attempt != 0 {
                log::debug!(
                    "{}: Attempt #{} skip process_dispatch_queue",
                    self.collated_block_descr,
                    collator_data.error_attempt,
                );
                break;
            }
            let extra = collator_data.out_msg_queue_info.out_queue_extra();
            let mut current_dispatch_queue = extra.dispatch_queue.clone();
            while !current_dispatch_queue.is_empty() {
                collator_data.block_full |= !collator_data.limit_fits(ParamLimitIndex::Normal);
                if collator_data.block_full {
                    log::info!(
                        "{}: BLOCK FULL, stop processing dispatch queue",
                        self.collated_block_descr,
                    );
                    collator_data.classify_block_limit_class();
                    collator_data.register_dispatch_queue_op(true)?;
                    break;
                }
                if self.check_cutoff_timeout() {
                    log::warn!(
                        "{}: TIMEOUT ({}ms) is elapsed, stop processing internal messages",
                        self.collated_block_descr,
                        self.engine.collator_config().cutoff_timeout_ms
                    );
                    collator_data.register_dispatch_queue_op(true)?;
                    break;
                }
                let mut result = None;
                while let Some(src_addr) = priority_list.move_next() {
                    if !self.shard.contains_address(src_addr)? {
                        priority_list.remove_current();
                    } else if let Some(queue) = current_dispatch_queue.get(src_addr.address())? {
                        result = Some((src_addr.address().clone(), queue));
                        break;
                    } else {
                        priority_list.remove_current();
                    }
                }
                let (account_id, acc_queue) = if let Some(result) = result {
                    result
                } else if let Some(result) = current_dispatch_queue.find_by_root_aug()? {
                    result
                } else {
                    fail!("unreachable: dispatch queue is empty")
                };
                let Some(lt) = acc_queue.oldest()? else {
                    fail!("account dispatch queue for {account_id:x} is empty")
                };
                let (enq, acc_queue) = self
                    .process_deferred_message(&account_id, lt, collator_data)
                    .map_err(|err| {
                        error!(
                            "error processing internal message from dispatch queue: \
                            account={account_id:x}, lt={lt}, err: {err:?}"
                        )
                    })?;
                let env_cell = enq.envelope_cell().cell();
                collator_data.block_limit_status.add_cell(&env_cell)?;

                // dispatch only one message per account in first iteration
                // dispatch not more than config messages per account in second iteration if not in whitelist
                if iter == 0
                    || iter == 1
                        && collator_data.sender_generated_messages_count(&account_id)
                            >= self.engine.collator_config().defer_messages_after
                        && !self.whitelist_contains(self.shard.workchain_id(), &account_id)
                    || acc_queue.len() == 0
                {
                    log::trace!(
                        "finish processing internal message from dispatch queue: \
                        account={account_id:x}"
                    );
                    current_dispatch_queue.remove(account_id.clone())?;
                } else {
                    current_dispatch_queue.set(&account_id, &acc_queue, &lt)?;
                }
                if let Some(msg_metadata) = enq.metadata() {
                    let initiator = msg_metadata.initiator();
                    let initiator_count = count_per_initiator.entry(initiator).or_default();
                    *initiator_count += 1;
                    if *initiator_count >= *max_per_initiator {
                        log::trace!(
                            "finish processing internal message from dispatch queue: \
                            account={account_id:x} initiator lt: {}",
                            msg_metadata.initiator_lt()
                        );
                        current_dispatch_queue.remove(account_id.clone())?;
                    }
                }
                let new_message = NewMessage::new_deferred(enq);
                collator_data.new_messages.push(new_message);
                total_count += 1;
                if total_count >= *max_total_count {
                    collator_data.dispatch_queue_total_limit_reached = true;
                    log::debug!(
                        "{}: DISPATCH_QUEUE_STAGE_{iter}: total limit {total_count} reached",
                        self.collated_block_descr,
                    );
                    break;
                }
            }
            if iter == 0 {
                collator_data.have_unprocessed_account_dispatch_queue = false;
            }
            collator_data.register_dispatch_queue_op(true)?;
        }
        Ok(())
    }

    async fn create_ticktock_transactions(
        &self,
        tock: bool,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        log::trace!("{}: create_ticktock_transactions", self.collated_block_descr);
        let fundamental_dict = collator_data.config.raw_config().fundamental_smc_addr()?;
        for res in &fundamental_dict {
            let account_id = SliceData::load_bitstring(res?.0)?;
            self.create_ticktock_transaction(
                account_id,
                tock,
                prev_data,
                collator_data,
                exec_manager,
            )
            .await?;
            self.check_stop_flag()?;
        }
        let account_id = collator_data.config.raw_config().config_addr.clone();
        self.create_ticktock_transaction(account_id, tock, prev_data, collator_data, exec_manager)
            .await?;
        self.wait_transactions(exec_manager, collator_data, false).await?;
        Ok(())
    }

    async fn create_ticktock_transaction(
        &self,
        account_id: AccountId,
        tock: bool,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        log::trace!(
            "{}: create_ticktock_transaction({}) acc: {account_id:x}",
            self.collated_block_descr,
            if tock { "tock" } else { "tick" },
        );
        CHECK!(self.shard.is_masterchain());

        let account = prev_data
            .account(&account_id)?
            .ok_or_else(|| error!("Can't find account {:x}", account_id))?
            .read_account()?;
        let tick_tock = account.get_tick_tock().cloned().unwrap_or_default();

        if (tick_tock.tock && tock) || (tick_tock.tick && !tock) {
            let tt = TransactionTickTock::new(tock);
            // different accounts can produce messages with same LT which cause order violation
            let initiator_addr = MsgAddressInt::with_params(-1, account_id.clone())?;
            let msg_metadata = Some(MsgMetadata::new(initiator_addr, 0));
            exec_manager
                .execute(
                    account_id,
                    AsyncMessage::TickTock(tt),
                    msg_metadata,
                    prev_data,
                    collator_data,
                )
                .await?;
        }

        Ok(())
    }

    async fn create_special_transactions(
        &self,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        if !self.shard.is_masterchain() {
            return Ok(());
        }
        log::debug!("{}: create_special_transactions", self.collated_block_descr);

        let account_id = collator_data.config.raw_config().fee_collector_address()?;
        self.create_special_transaction(
            account_id,
            collator_data.value_flow.recovered.clone(),
            AsyncMessage::Recover,
            prev_data,
            collator_data,
            exec_manager,
        )
        .await?;
        self.check_stop_flag()?;

        let account_id = collator_data.config.raw_config().minter_address()?;
        self.create_special_transaction(
            account_id,
            collator_data.value_flow.minted.clone(),
            AsyncMessage::Mint,
            prev_data,
            collator_data,
            exec_manager,
        )
        .await?;

        self.wait_transactions(exec_manager, collator_data, false).await?;

        Ok(())
    }

    async fn create_special_transaction(
        &self,
        account_id: AccountId,
        amount: CurrencyCollection,
        f: impl FnOnce(Message, Cell) -> AsyncMessage,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        log::trace!(
            "{}: create_special_transaction: recover {} to account {:x}",
            self.collated_block_descr,
            amount.coins,
            account_id
        );
        if amount.is_zero()? || !self.shard.is_masterchain() {
            return Ok(());
        }
        let mut hdr = InternalMessageHeader::with_addresses_and_bounce(
            MsgAddressInt::with_standart(None, -1, AccountId::ZERO_ID)?,
            MsgAddressInt::with_standart(None, -1, account_id.clone())?,
            amount,
            true,
        );
        hdr.created_lt = collator_data.start_lt()?;
        hdr.created_at = collator_data.gen_utime.into();
        let msg = Message::with_int_header(hdr);
        let msg_cell = msg.serialize()?;
        let initiator_addr = MsgAddressInt::with_params(-1, account_id.clone())?;
        let msg_metadata = Some(MsgMetadata::new(initiator_addr, 0));
        exec_manager
            .execute(account_id, f(msg, msg_cell), msg_metadata, prev_data, collator_data)
            .await?;
        Ok(())
    }

    async fn process_inbound_internal_messages(
        &self,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        output_queue_manager: &MsgQueueManager,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        log::debug!("{}: process_inbound_internal_messages", self.collated_block_descr);
        let mut iter = output_queue_manager.merge_out_queue_iter(&self.shard)?;
        for k_v in iter.by_ref() {
            if collator_data.block_full {
                log::debug!(
                    "{}: BLOCK FULL, stop processing internal messages",
                    self.collated_block_descr
                );
                break;
            }
            if self.check_cutoff_timeout() {
                log::warn!(
                    "{}: TIMEOUT ({}ms) is elapsed, stop processing internal messages",
                    self.collated_block_descr,
                    self.engine.collator_config().cutoff_timeout_ms
                );
                break;
            }
            self.check_stop_flag()?;

            let (key, enq, block_id) = match k_v {
                Ok(k_v) => k_v,
                Err(err) => {
                    // this code is for collator bundles not to produce error accessing pruned messages
                    #[cfg(test)]
                    if self.collator_settings.is_bundle
                        && err.downcast_ref() == Some(&ExceptionCode::PrunedCellAccess)
                    {
                        log::warn!("pruned cell access detected");
                        break;
                    }
                    return Err(err);
                }
            };
            log::trace!(
                "{}: message {:x}, created lt: {}, lt: {}, enq lt: {} emitted lt: {}",
                self.collated_block_descr,
                key,
                enq.message().created_lt().unwrap_or_default(),
                enq.lt(),
                enq.enqueued_lt(),
                enq.emitted_lt(),
            );
            collator_data.update_last_proc_int_msg((enq.lt(), enq.message_hash()))?;
            if collator_data.out_msg_queue_info.already_processed(&enq)? {
                log::trace!(
                    "{}: message {:x} has been already processed by us before, skipping",
                    self.collated_block_descr,
                    key.hash
                );
            } else {
                self.check_inbound_internal_message(&key, &enq, block_id.shard()).map_err(
                    |err| {
                        error!(
                            "problem processing internal inbound message with hash {:x} : {err}",
                            key.hash
                        )
                    },
                )?;
                let our = self.shard.contains_full_prefix(enq.cur_prefix());
                let to_us = self.shard.contains_full_prefix(enq.dst_prefix());
                if to_us {
                    let account_id = enq.dst_account_id().clone();
                    log::debug!(
                        "{}: internal message {:x} sent to execution to account {account_id:x}",
                        self.collated_block_descr,
                        key.hash,
                    );
                    let msg_metadata = enq.msg_metadata_add_depth();
                    let msg = AsyncMessage::Int(enq, our);
                    if !exec_manager
                        .execute(account_id, msg, msg_metadata, prev_data, collator_data)
                        .await?
                    {
                        break;
                    }
                } else {
                    // println!("{:x} {:#}", key, enq);
                    // println!("cur: {}, dst: {}", enq.cur_prefix(), enq.dst_prefix());
                    log::debug!(
                        "{}: enqueue_transit_message {:x}",
                        self.collated_block_descr,
                        enq.message_hash()
                    );
                    collator_data.enqueue_transit_message(&self.shard, &enq, our, false)?;
                    if our {
                        collator_data.del_out_msg_from_state(&key)?;
                    }
                }
            }
        }
        // all internal messages are processed
        collator_data.inbound_queues_empty = iter.next().is_none();
        Ok(())
    }

    fn check_inbound_internal_message(
        &self,
        key: &OutMsgQueueKey,
        enq: &MsgEnqueueStuff,
        nb_shard: &ShardIdent,
    ) -> Result<()> {
        let header = enq.message().int_header().ok_or_else(|| error!("message is not internal"))?;
        if enq.emitted_lt() == 0 && enq.lt() != header.created_lt {
            fail!(
                "inbound internal message has an augmentation value in source OutMsgQueue \
                distinct from the one in its contents (CommonMsgInfo)"
            )
        }
        if enq.emitted_lt() != 0 && enq.lt() != enq.emitted_lt() {
            fail!(
                "inbound internal message has an augmentation value in source OutMsgQueue \
                distinct from the one in its contents (deferred_it in MsgEnvelope)"
            )
        }
        if enq.fwd_fee_remaining() > header.fwd_fee() {
            fail!(
                "inbound internal message \
                has fwd_fee_remaining={} larger than original fwd_fee={}",
                enq.fwd_fee_remaining(),
                header.fwd_fee()
            )
        }
        if !nb_shard.contains_full_prefix(enq.cur_prefix()) {
            fail!(
                "inbound internal message \
                does not have current address in the originating neighbor shard"
            )
        }
        if !self.shard.contains_full_prefix(enq.next_prefix()) {
            fail!("inbound internal message does not have next hop address in our shard")
        }
        if key.workchain_id != enq.next_prefix().workchain_id {
            fail!(
                "inbound internal message has invalid key in OutMsgQueue \
                : its first 96 bits differ from next_hop_addr"
            )
        }
        Ok(())
    }

    async fn process_inbound_external_messages(
        &self,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        if collator_data.skip_extmsg() {
            log::debug!(
                "{}: skipping processing of inbound external messages",
                self.collated_block_descr
            );
            return Ok(());
        }
        if collator_data.error_attempt >= 2 {
            log::info!(
                "{}: attempt #{}: skipping external messages",
                self.collated_block_descr,
                collator_data.error_attempt
            );
            return Ok(());
        }
        log::debug!("{}: process_inbound_external_messages", self.collated_block_descr);
        let finish_time_ms = self.get_external_messages_finish_time_micros();
        let mut iter =
            self.engine.get_external_messages_iterator(self.shard.clone(), finish_time_ms);
        loop {
            let Some((msg, msg_id)) = iter.next() else {
                break;
            };
            let msg_cell = msg.serialize()?; // it could be obtained
            let header = msg
                .ext_in_header()
                .ok_or_else(|| error!("message {:x} is not external inbound message", msg_id))?;
            if self.shard.contains_address(&header.dst)? {
                if !collator_data.limit_fits(ParamLimitIndex::Soft) {
                    log::debug!(
                        "{}: BLOCK FULL, stop processing external messages",
                        self.collated_block_descr
                    );
                    break;
                }
                if self.check_cutoff_timeout() {
                    log::warn!(
                        "{}: TIMEOUT is elapsed, stop processing external messages",
                        self.collated_block_descr
                    );
                    break;
                }
                let (_, account_id) = header.dst.extract_std_address(true)?;
                log::debug!(
                    "{}: external message {msg_id:x} sent to execution to account {account_id:x}",
                    self.collated_block_descr,
                );
                let msg = AsyncMessage::Ext(msg, msg_cell, msg_id);
                let initiator_addr =
                    MsgAddressInt::with_params(self.shard.workchain_id(), account_id.clone())?;
                let msg_metadata = Some(MsgMetadata::new(initiator_addr, 0));
                if !exec_manager
                    .execute(account_id, msg, msg_metadata, prev_data, collator_data)
                    .await?
                {
                    break;
                }
            } else {
                // usually node collates more than one shard, the message can belong another one,
                // so we can't postpone it
                // (difference with t-node)
                // collator_data.to_delay.push(id);
            }
            self.check_stop_flag()?;
        }
        self.wait_transactions(exec_manager, collator_data, true).await?;
        let accepted = mem::take(&mut collator_data.accepted_ext_messages);
        let rejected = mem::take(&mut collator_data.rejected_ext_messages);
        self.engine.complete_external_messages(rejected, accepted)?;
        Ok(())
    }

    fn whitelist_contains(&self, workchain_id: i32, account_id: &AccountId) -> bool {
        let mut iter = self.engine.collator_config().whitelist.iter();
        iter.any(|addr| addr.workchain_id() == workchain_id && addr.address() == account_id)
    }

    fn process_new_message(
        &self,
        mut msg: NewMessage,
        collator_data: &mut CollatorData,
    ) -> Result<Option<AsyncMessage>> {
        let from_dispatch_queue = msg.tr_cell.is_empty();
        if (collator_data.block_full || collator_data.have_unprocessed_account_dispatch_queue)
            && !collator_data.enqueue_only
        {
            log::debug!(
                "{}: BLOCK FULL or unprocessed dispatch queue, stop processing new messages",
                self.collated_block_descr
            );
            collator_data.enqueue_only = true;
        }
        let Some(info) = msg.enq.message().int_header() else {
            fail!("unreachable: new message is not internal")
        };
        let msg_hash = msg.enq.message_hash();
        let workchain_id = info.src()?.workchain_id();
        let account_id = info.src()?.address().clone();
        let is_special_account = workchain_id == MASTERCHAIN_ID
            && collator_data.config.is_special_account(true, &account_id)?;
        let enqueue =
            collator_data.enqueue_only || !self.shard.contains_full_prefix(msg.enq.dst_prefix());
        let mut defer = false;
        if from_dispatch_queue {
            collator_data.del_unprocessed_deferred_messages(&account_id);
        } else {
            if collator_data.config.deferring_enabled
                && !is_special_account
                && msg.index != 0
                && !self.whitelist_contains(workchain_id, &account_id)
                && (collator_data.add_sender_generated_message(account_id.clone())
                    >= self.engine.collator_config().defer_messages_after
                    || collator_data.out_msg_queue_size
                        > self.engine.collator_config().defer_out_queue_size_limit)
            {
                defer = true;
            }
            if collator_data.must_be_deferred(&account_id)? {
                defer = true;
            }
        }
        if enqueue || defer {
            if from_dispatch_queue {
                collator_data.enqueue_transit_message(&self.shard, &msg.enq, false, true)?;
            } else if defer {
                log::trace!("Defer message with hash {:x}", msg.enq.message_hash());
                msg.enq.clear_routing()?;
                debug_assert!(msg.enq.envelope().cur_addr().is_zero());
                debug_assert!(msg.enq.envelope().next_addr().is_zero());
                let out_msg = OutMsg::new_defer(msg.enq.envelope_cell(), msg.tr_cell.clone());
                collator_data.add_out_msg_to_block(&msg_hash, &out_msg)?;
                collator_data.defer_message(&account_id, &msg.enq)?;
            } else {
                let out_msg = OutMsg::new(msg.enq.envelope_cell(), msg.tr_cell.clone());
                collator_data.add_out_msg_to_block(&msg_hash, &out_msg)?;
                collator_data.add_msg_to_state(&msg.enq, false)?;
            }
            Ok(None)
        } else {
            log::debug!(
                "{}: new message {msg_hash:x} sent to execution to account {account_id:x}",
                self.collated_block_descr,
            );
            collator_data.update_last_proc_int_msg((msg.enq.lt(), msg_hash.clone()))?;
            let msg = if from_dispatch_queue {
                AsyncMessage::Deferred(msg.enq)
            } else {
                let out_msg = OutMsg::new(msg.enq.envelope_cell(), msg.tr_cell.clone());
                collator_data.add_out_msg_to_block(&msg_hash, &out_msg)?;
                AsyncMessage::New(msg.enq, msg.tr_cell)
            };
            Ok(Some(msg))
        }
    }

    async fn process_new_messages(
        &self,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        exec_manager: &mut ExecutionManager,
    ) -> Result<()> {
        log::debug!("{}: process_new_messages", self.collated_block_descr);
        while !collator_data.new_messages.is_empty() {
            // In the iteration we execute only existing messages.
            // Newly generating messages will be executed next iteration (only after waiting).

            let mut new_messages = mem::take(&mut collator_data.new_messages);
            // we can get sorted items somehow later
            while let Some(msg) = new_messages.pop() {
                self.check_stop_flag()?;
                exec_manager.max_lt.fetch_max(msg.enq.emitted_lt(), Ordering::Relaxed);
                collator_data.update_lt(exec_manager.max_lt.load(Ordering::Relaxed));
                collator_data.block_limit_status.remove_extra_out_msg_op();
                let msg_metadata = msg.enq.msg_metadata_add_depth();
                let account_id = msg.enq.dst_account_id().clone();
                if let Some(msg) = self.process_new_message(msg, collator_data)? {
                    if !exec_manager
                        .execute(account_id, msg, msg_metadata, prev_data, collator_data)
                        .await?
                    {
                        collator_data.enqueue_only = true;
                    }
                    if self.collator_settings.lt_compatible {
                        new_messages.append(&mut collator_data.new_messages);
                    }
                }
                self.check_stop_flag()?;
            }
            self.wait_transactions(exec_manager, collator_data, false).await?;
            self.check_stop_flag()?;
        }

        Ok(())
    }

    // waits and finalizes all parallel tasks
    async fn wait_transactions(
        &self,
        exec_manager: &mut ExecutionManager,
        collator_data: &mut CollatorData,
        allow_cancel: bool,
    ) -> Result<()> {
        log::trace!("{}: wait_transactions", self.collated_block_descr);
        while exec_manager.wait_tr.count() != 0 {
            if allow_cancel && self.check_cutoff_timeout() {
                log::warn!(
                    "{}: TIMEOUT is elapsed, cancelling remaining external messages",
                    self.collated_block_descr
                );
                exec_manager.cancel_ext.store(true, Ordering::Relaxed);
                break;
            }
            exec_manager.wait_transaction(collator_data).await?;
        }
        exec_manager
            .min_lt
            .fetch_max(exec_manager.max_lt.load(Ordering::Relaxed), Ordering::Relaxed);
        Ok(())
    }

    fn update_processed_upto(
        &self,
        mc_data: &McData,
        collator_data: &mut CollatorData,
    ) -> Result<()> {
        log::trace!(
            "{}: update_processed_upto {}:{:x}",
            self.collated_block_descr,
            collator_data.last_proc_int_msg.0,
            collator_data.last_proc_int_msg.1
        );

        let ref_mc_seqno = match self.shard.is_masterchain() {
            true => self.new_block_id_part.seq_no,
            false => mc_data.state().block_id().seq_no,
        };

        // Use masterchain seqno for `ProcessedUptoStuff` for the old implementation
        #[cfg(not(feature = "xp25"))]
        let seqno = ref_mc_seqno;

        // Use shard seqno for `ProcessedUptoStuff` for the new implementation
        #[cfg(feature = "xp25")]
        let seqno = self.new_block_id_part.seq_no;

        collator_data.update_min_mc_seqno(ref_mc_seqno);
        let lt = collator_data.last_proc_int_msg.0;
        if lt != 0 {
            let hash = collator_data.last_proc_int_msg.1.clone();
            collator_data.out_msg_queue_info.add_processed_upto(
                seqno,
                #[cfg(feature = "xp25")]
                ref_mc_seqno,
                lt,
                hash,
            )?;
            collator_data.out_msg_queue_info.compactify()?;
        // TODO: need to think about this later, maybe config->lt is 0 always...
        } else if collator_data.inbound_queues_empty {
            if let Some(lt) = mc_data.state().state()?.gen_lt().checked_sub(1) {
                collator_data.out_msg_queue_info.add_processed_upto(
                    seqno,
                    #[cfg(feature = "xp25")]
                    ref_mc_seqno,
                    lt,
                    UInt256::MAX,
                )?;
                collator_data.out_msg_queue_info.compactify()?;
            }
        }
        Ok(())
    }

    fn check_block_overload(
        &self,
        collator_data: &mut CollatorData,
        out_queue_cleaned_partial: bool,
    ) {
        log::trace!("{}: check_block_overload", self.collated_block_descr);
        collator_data.classify_block_limit_class();
        if collator_data.block_limit_class == ParamLimitIndex::Underload {
            // we don't want to merge if collation too long
            if !self.check_cutoff_timeout()
                && !out_queue_cleaned_partial
                && !collator_data.before_split
            {
                collator_data.underload_history |= 1;
                log::info!("{}: Block is underloaded", self.collated_block_descr);
            }
        } else if collator_data.block_limit_class >= ParamLimitIndex::Soft {
            collator_data.overload_history |= 1;
            log::info!(
                "{}: Block is overloaded (category {:?})",
                self.collated_block_descr,
                collator_data.block_limit_class
            );
        } else {
            log::info!("{}: Block is loaded normally", self.collated_block_descr);
        }

        if let Some(true) = self.collator_settings.want_split {
            log::info!("{}: want_split manually set", self.collated_block_descr);
            collator_data.want_split = true;
            return;
        } else if let Some(true) = self.collator_settings.want_merge {
            log::info!("{}: want_merge manually set", self.collated_block_descr);
            collator_data.want_merge = true;
            return;
        }

        if CollatorData::history_weight(collator_data.overload_history) >= 0 {
            log::info!(
                "{}: want_split set because of overload history 0x{:X}",
                self.collated_block_descr,
                collator_data.overload_history
            );
            collator_data.want_split = true;
        } else if CollatorData::history_weight(collator_data.underload_history) >= 0 {
            log::info!(
                "{}: want_merge set because of underload history 0x{:X}",
                self.collated_block_descr,
                collator_data.underload_history
            );
            collator_data.want_merge = true;
        }
    }

    //
    // finalize
    //
    async fn finalize_block(
        &self,
        mc_data: &McData,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        mut exec_manager: ExecutionManager,
        output_queue_manager: &MsgQueueManager,
    ) -> Result<(CollateResult, ExecutionManager)> {
        log::trace!("{}: finalize_block", self.collated_block_descr);
        let (want_split, overload_history) = collator_data.want_split();
        let (want_merge, underload_history) = collator_data.want_merge();

        // update shard accounts tree and prepare accounts blocks
        let mut new_accounts = prev_data.accounts.clone();
        let mut accounts = ShardAccountBlocks::default();
        let config_addr = match self.shard.is_masterchain() {
            true => prev_data.state().config_params()?.config_address().ok(),
            false => None,
        };
        let mut changed_accounts = BTreeMap::new();
        let mut new_config_opt = None;
        for (account_id, (sender, handle)) in mem::take(&mut exec_manager.changed_accounts) {
            mem::drop(sender);
            let mut shard_acc = handle.await.map_err(|err| {
                error!("account {:x} thread didn't finish: {}", account_id, err)
            })??;
            if let Some(addr) = &config_addr {
                if addr == &account_id {
                    new_config_opt = Some(Self::extract_new_config(shard_acc.account(), addr)?);
                }
            }
            if shard_acc.is_touched() {
                let acc_block = shard_acc.update_shard_state(&mut new_accounts)?;
                accounts.insert(&acc_block)?;
                let account = shard_acc.account();
                if let Some(storage_dict) = shard_acc.storage_dict() {
                    if account.dict_hash().is_some() {
                        let size = account.storage_info().map_or(0, |info| info.used().cells());
                        log::trace!(
                            "{}: updated storage dict with hash {:x} for account {:x} of size {}",
                            self.collated_block_descr,
                            storage_dict.repr_hash(),
                            account_id,
                            size
                        );
                        self.engine.add_account_storage_dict(storage_dict, size)
                    }
                }
                changed_accounts.insert(account_id, shard_acc);
            }
        }

        if let Some(hardfork_config) = self.engine.get_config_for_hardfork() {
            let mut new_config =
                new_config_opt.unwrap_or_else(|| collator_data.config.raw_config().clone());
            self.apply_hardfork_config(&hardfork_config, &mut new_config)?;
            new_config_opt = Some(new_config);
        }

        log::trace!("{}: finalize_block: calc value flow", self.collated_block_descr);
        // calc value flow
        let mut value_flow = collator_data.value_flow.clone();
        value_flow.imported = collator_data.in_msgs.root_extra().value_imported.clone();
        value_flow.exported = collator_data.out_msgs.root_extra().clone();
        let mut total_fees = accounts.root_extra().clone();
        total_fees.coins.add(&collator_data.in_msgs.root_extra().fees_collected)?;

        value_flow.fees_collected.add(&total_fees)?;
        if self.shard.is_masterchain() {
            if let Some(burning_cfg) = collator_data.config.burning_config() {
                let burned = burning_cfg.calculate_burned_fees(total_fees.coins.as_u128())?;
                value_flow.fees_collected.coins.sub(&burned)?;
                value_flow.burned.coins.add(&burned)?;
            }
        }
        value_flow.fees_collected.add(&value_flow.created)?;
        value_flow.to_next_blk = new_accounts.full_balance().clone();
        //value_flow.to_next_blk.add(&value_flow.recovered)?;
        value_flow.remove_zero_currencies()?;

        // println!("{}", &value_flow);

        collator_data.out_msg_queue_info.out_queue_extra_mut().out_queue_size =
            collator_data.out_msg_queue_size;

        let (out_msg_queue_info, min_ref_mc_seqno) =
            collator_data.out_msg_queue_info.serialize()?;
        collator_data.update_min_mc_seqno(min_ref_mc_seqno);
        let (mc_state_extra, master_ref) = if self.shard.is_masterchain() {
            let (extra, min_seqno) =
                self.create_mc_state_extra(prev_data, collator_data, new_config_opt)?;
            collator_data.update_min_mc_seqno(min_seqno);
            (Some(extra), None)
        } else {
            (None, Some(mc_data.master_ref()?))
        };
        let gen_validator_list_hash_short = ValidatorSet::calc_subset_hash_short(
            self.validator_set.list(),
            self.validator_set.catchain_seqno(),
        )?;

        log::trace!("{}: finalize_block: fill block info", self.collated_block_descr);
        // calc block info
        let mut info = BlockInfo::default();
        info.set_version(0);
        info.set_before_split(collator_data.before_split());
        info.set_want_merge(want_merge);
        info.set_want_split(want_split);
        info.set_after_split(self.after_split);
        info.set_prev_stuff(self.after_merge, collator_data.prev_stuff()?)?;
        info.set_shard(self.shard.clone());
        info.set_seq_no(self.new_block_id_part.seq_no)?;
        info.set_vertical_stuff(0, prev_data.prev_vert_seqno()?, None)?;
        info.set_start_lt(collator_data.start_lt()?);
        info.set_end_lt(collator_data.block_limit_status.lt() + 1);
        info.set_gen_utime(collator_data.gen_utime());
        info.set_gen_validator_list_hash_short(gen_validator_list_hash_short);
        info.set_gen_catchain_seqno(self.validator_set.catchain_seqno());
        info.set_min_ref_mc_seqno(collator_data.min_mc_seqno()?);
        info.set_prev_key_block_seqno(mc_data.prev_key_block_seqno());
        info.write_master_ref(master_ref.as_ref())?;

        if collator_data.config.raw_config().has_capability(GlobalCapabilities::CapReportVersion) {
            info.set_gen_software(Some(GlobalVersion {
                version: supported_version(),
                capabilities: supported_capabilities(),
            }));
        }

        log::trace!("{}: finalize_block: calc new state", self.collated_block_descr);
        // Calc new state, then state update

        let mut new_state = ShardStateUnsplit::with_ident(self.shard.clone());
        new_state.set_global_id(prev_data.state().state()?.global_id());
        new_state.set_seq_no(self.new_block_id_part.seq_no);
        new_state.set_vert_seq_no(prev_data.prev_vert_seqno()?);
        new_state.set_gen_time(collator_data.gen_utime());
        new_state.set_gen_lt(info.end_lt());
        new_state.set_before_split(info.before_split());
        new_state.set_overload_history(overload_history);
        new_state.set_underload_history(underload_history);
        new_state.set_min_ref_mc_seqno(collator_data.min_mc_seqno()?);
        new_state.write_accounts(&new_accounts)?;
        new_state.write_out_msg_queue_info(&out_msg_queue_info)?;
        new_state.set_master_ref(master_ref);
        let mut total_balance = new_accounts.root_extra().balance().clone();
        total_balance.remove_zero_currencies()?;
        new_state.set_total_balance(total_balance);
        let mut total_validator_fees = prev_data.total_validator_fees().clone();
        // total_validator_fees.add(&value_flow.created)?;
        // total_validator_fees.add(&accounts.root_extra())?;
        total_validator_fees.add(&value_flow.fees_collected)?;
        total_validator_fees.sub(&value_flow.recovered)?;
        total_validator_fees.remove_zero_currencies()?;
        new_state.set_total_validator_fees(total_validator_fees);
        if self.shard.is_masterchain() {
            *new_state.libraries_mut() =
                self.update_public_libraries(exec_manager.libraries.clone(), &changed_accounts)?;
        }
        new_state.write_custom(mc_state_extra.as_ref())?;
        if self.engine.get_config_for_hardfork().is_some() {
            new_state.update_config_smc()?;
        }

        #[cfg(feature = "xp25")]
        if !self.shard.is_masterchain() {
            let shards = &collator_data.shards()?;
            let mut ids = Vec::new();
            // TODO: impl bin tree value update
            shards.iterate_shards(|shard_id, descr| {
                let block_id = BlockIdExt {
                    shard_id,
                    seq_no: descr.seq_no,
                    file_hash: descr.file_hash,
                    root_hash: descr.root_hash,
                };
                ids.push((block_id, descr.end_lt));
                Ok(true)
            })?;
            let ref_shard_blocks = RefShardBlocks::with_ids(ids.iter())?;
            new_state.write_wc_custom(Some(&WcExtra { ref_shard_blocks }))?;
        }

        if log::log_enabled!(log::Level::Trace) {
            new_state.read_out_msg_queue_info()?.proc_info().iterate_slices_with_keys(
                |ref mut key, ref mut value| {
                    let key = ProcessedInfoKey::construct_from(key)?;
                    let value = ProcessedUpto::construct_from(value)?;
                    log::trace!(
                        "{}: new processed upto {} {:x} - {} {:x}",
                        self.collated_block_descr,
                        key.mc_seqno,
                        key.shard,
                        value.last_msg_lt,
                        value.last_msg_hash
                    );
                    Ok(true)
                },
            )?;
        }

        log::trace!("{}: finalize_block: calc merkle update", self.collated_block_descr);
        let new_ss_root = new_state.serialize()?;

        self.check_stop_flag()?;

        // let mut visited_from_root = HashSet::new();
        // Self::_check_visited_integrity(&prev_data.state_root, &visited, &mut visited_from_root);
        // assert_eq!(visited.len(), visited_from_root.len());

        let state_update =
            self.create_merkle_update(prev_data, collator_data, &new_ss_root).inspect_err(|e| {
                log::error!("{}: create_merkle_update {:?}", self.collated_block_descr, e);
            })?;

        self.check_stop_flag()?;

        // calc block extra
        let mut extra = BlockExtra::default();
        extra.write_in_msg_descr(&collator_data.in_msgs)?;
        extra.write_out_msg_descr(&collator_data.out_msgs)?;
        extra.write_account_blocks(&accounts)?;
        log::trace!("{}: finalize_block: BlockExtra 1", self.collated_block_descr);
        // mc block extra
        if let Some(mc_state_extra) = mc_state_extra {
            log::trace!("{}: finalize_block: McBlockExtra", self.collated_block_descr);
            let mut mc_block_extra = McBlockExtra::default();
            *mc_block_extra.hashes_mut() = collator_data.shards.clone().unwrap();
            *mc_block_extra.fees_mut() = collator_data.shard_fees.clone();
            if let Some(msg) = &collator_data.recover_create_msg {
                mc_block_extra.write_recover_create_msg(msg)?;
            }
            if let Some(msg) = &collator_data.mint_msg {
                mc_block_extra.write_mint_msg(msg)?;
            }
            if mc_state_extra.after_key_block {
                info.set_key_block(true);
                *mc_block_extra.config_mut() = Some(mc_state_extra.config().clone());
            }
            extra.write_custom(&mc_block_extra)?;
        }
        extra.rand_seed = self.rand_seed.clone();
        extra.created_by = self.created_by.clone();

        #[cfg(feature = "xp25")]
        if !self.shard.is_masterchain() {
            let wcc =
                new_state.read_wc_custom()?.ok_or_else(|| error!("No wc custom in new state"))?;
            extra.write_wc_custom(Some(&wcc))?;
        }

        let global_id = mc_data.state().state()?.global_id();
        // construct block
        let new_block = Block::with_params(global_id, info, value_flow, state_update, extra)?;
        let mut block_id = self.new_block_id_part.clone();
        let workchain_id = block_id.shard().workchain_id();

        log::trace!("{}: finalize_block: fill block candidate", self.collated_block_descr);
        let cell = new_block.serialize()?;
        block_id.root_hash = cell.repr_hash();
        let mut data = Vec::new();
        // Block must be serialized the same way as in the cpp collator implementation
        // because all nodes expect this serialisation while receiving compressed blocks.
        // Mode 31 in cpp is equivalent to BocFlags::all()
        BocWriter::with_flags([cell.clone()], BocFlags::all())?.write(&mut data)?;
        block_id.file_hash = UInt256::calc_file_hash(&data);

        // !!!! DEBUG !!!!
        // if let Ok(block_str) = ton_block_json::debug_block(new_block.clone()) {
        //     let _ = std::fs::write(
        //         format!("tmp/{}.json", block_id), block_str
        //     );
        // }
        // !!!! DEBUG !!!!

        self.check_stop_flag()?;

        let collated_data = self.create_collated_data(
            collator_data,
            prev_data,
            changed_accounts.values(),
            output_queue_manager,
        )?;

        let candidate = BlockCandidate {
            block_id,
            data: data.clone(),
            collated_file_hash: UInt256::calc_file_hash(&collated_data),
            collated_data,
            created_by: self.created_by.clone(),
        };
        if workchain_id != -1
            && (collator_data.dequeue_count != 0
                || collator_data.enqueue_count != 0
                || collator_data.in_msg_count != 0
                || collator_data.out_msg_count != 0
                || collator_data.execute_count != 0
                || collator_data.transit_count != 0
                || collator_data.remove_count != 0)
        {
            log::debug!(
                "{}: finalize_block finished: \
                dequeue_count: {}, enqueue_count: {}, in_msg_count: {}, out_msg_count: {}, \
                execute_count: {}, transit_count: {}, remove_count: {} msg_queue_depth_sum: {}",
                self.collated_block_descr,
                collator_data.dequeue_count,
                collator_data.enqueue_count,
                collator_data.in_msg_count,
                collator_data.out_msg_count,
                collator_data.execute_count,
                collator_data.transit_count,
                collator_data.remove_count,
                collator_data.msg_queue_depth_sum
            );
        }
        log::trace!(
            "{}: finalize_block finished: \
            dequeue_count: {}, enqueue_count: {}, in_msg_count: {}, out_msg_count: {}, \
            execute_count: {}, transit_count: {}, remove_count: {}, data len: {}",
            self.collated_block_descr,
            collator_data.dequeue_count,
            collator_data.enqueue_count,
            collator_data.in_msg_count,
            collator_data.out_msg_count,
            collator_data.execute_count,
            collator_data.transit_count,
            collator_data.remove_count,
            candidate.data.len()
        );
        let collate_result = CollateResult::Ok {
            candidate,
            new_state,
            usage_tree: mem::take(&mut collator_data.usage_tree),
            new_block,
            block_root: cell,
        };
        Ok((collate_result, exec_manager))
    }

    fn _check_visited_integrity(
        cell: &Cell,
        visited: &HashSet<UInt256>,
        visited_from_root: &mut HashSet<UInt256>,
    ) {
        if visited.contains(&cell.repr_hash()) {
            visited_from_root.insert(cell.repr_hash());
            for r in cell.clone_references() {
                Self::_check_visited_integrity(&r, visited, visited_from_root);
            }
        }
    }

    fn extract_new_config(account: &Account, config_addr: &AccountId) -> Result<ConfigParams> {
        let new_config_root = account
            .get_data()
            .ok_or_else(|| error!("Can't extract config's contract data"))?
            .reference(0)?;
        let new_config =
            ConfigParams::with_address_and_params(config_addr.clone(), Some(new_config_root));

        Ok(new_config)
    }

    fn apply_hardfork_config(
        &self,
        hardfork: &ConfigParams,
        config: &mut ConfigParams,
    ) -> Result<()> {
        log::info!("{}: applying hardfork config update", self.collated_block_descr,);
        // Replace config parameters with the ones from hardfork config, if any
        hardfork.config_params.iterate_slices(|key, value| {
            let num = u32::construct_from(&mut key.clone())?;
            let param =
                ConfigParamEnum::construct_from_cell_and_number(value.clone().reference(0)?, num)?;
            log::info!(
                "{}: applying hardfork config update: set param #{}\n{:?}",
                self.collated_block_descr,
                num,
                param
            );
            config.config_params.set(key, &value)?;
            Ok(true)
        })?;
        // Config smart contract will be updated further,
        // see 'new_state.update_config_smc()?;' in 'finalize_block'
        Ok(())
    }

    fn create_merkle_update(
        &self,
        prev_data: &PrevData,
        collator_data: &CollatorData,
        new_ss_root: &Cell,
    ) -> Result<MerkleUpdate> {
        // Full state update

        // let mut visited_from_root = HashSet::new();
        // Self::_check_visited_integrity(&prev_data.state_root, &visited, &mut visited_from_root);
        // assert_eq!(visited.len(), visited_from_root.len());

        #[cfg(test)]
        let need_full_state_update = self.collator_settings.is_bundle;
        #[cfg(not(test))]
        let need_full_state_update = true;
        let state_update;
        if need_full_state_update {
            let now = Instant::now();
            state_update = MerkleUpdate::create_fast(&prev_data.state_root, new_ss_root, |h| {
                collator_data.usage_tree.contains(h) || collator_data.imported_visited.contains(h)
            })?;
            log::trace!(
                "{}: TIME: merkle update creating {}ms;",
                self.collated_block_descr,
                now.elapsed().as_millis()
            );
        } else {
            state_update = MerkleUpdate::default();
        }

        // let new_root2 = state_update.apply_for(&prev_data.state_root)?;
        // assert_eq!(new_root2.repr_hash(), new_ss_root.repr_hash());

        Ok(state_update)
    }

    fn update_public_libraries(
        &self,
        mut libraries: Libraries,
        accounts: &BTreeMap<AccountId, ShardAccountStuff>,
    ) -> Result<Libraries> {
        log::trace!("{}: update_public_libraries", self.collated_block_descr);
        for (_, acc) in accounts.iter() {
            acc.update_public_libraries(&mut libraries)?;
        }
        Ok(libraries)
    }

    fn create_mc_state_extra(
        &self,
        prev_data: &PrevData,
        collator_data: &mut CollatorData,
        new_config_opt: Option<ConfigParams>,
    ) -> Result<(McStateExtra, u32)> {
        log::trace!("{}: create_mc_state_extra", self.collated_block_descr);
        CHECK!(!self.after_merge);
        CHECK!(self.new_block_id_part.shard_id.is_masterchain());

        // 1. update config:ConfigParams
        let state_extra = prev_data.state().shard_state_extra()?;
        let old_config = state_extra.config();
        let (config, is_key_block) = if let Some(new_config) = new_config_opt {
            if !new_config.valid_config_data(true, None)? {
                fail!(
                    "configuration smart contract {:x} \
                    contains an invalid configuration in its data",
                    new_config.config_addr
                );
            }
            let is_key_block =
                new_config.important_config_parameters_changed(state_extra.config(), false)?;
            if is_key_block {
                log::info!("{}: IS KEY BLOCK", self.collated_block_descr);
            }
            (new_config, is_key_block)
        } else {
            (old_config.clone(), false)
        };

        let now = collator_data.gen_utime();
        let prev_now = prev_data.prev_state_utime();

        // 2. update shard_hashes and shard_fees
        let ccvc = config.catchain_config()?;

        let workchains = config.workchains()?;
        let update_shard_cc = {
            let lifetimes = now / ccvc.shard_catchain_lifetime;
            let prev_lifetimes = prev_now / ccvc.shard_catchain_lifetime;
            is_key_block || (lifetimes > prev_lifetimes)
        };
        let min_ref_mc_seqno =
            self.update_shard_config(collator_data, &workchains, update_shard_cc)?;
        // 3. save new shard_hashes
        // just take collator_data.shards()

        // 4. check extension flags
        // tate_extra.flags is checked in the McStateExtra::read_from

        // 5. update validator_info
        let mut validator_info = state_extra.validator_info.clone();
        let cur_validators = config.validator_set()?;
        let lifetime = ccvc.mc_catchain_lifetime;
        let mut cc_updated = false;
        if is_key_block || (now / lifetime > prev_now / lifetime) {
            validator_info.catchain_seqno += 1;
            cc_updated = true;
            log::debug!(
                "{}: increased masterchain catchain seqno to {}",
                self.collated_block_descr,
                validator_info.catchain_seqno
            );
        }
        let subset =
            calc_subset_for_masterchain(&cur_validators, &config, validator_info.catchain_seqno)?;
        // t-node calculates subset with valid catchain_seqno and then subset_hash_short with zero one...
        let hash_short = ValidatorSet::calc_subset_hash_short(&subset.validators, 0)?;

        {
            validator_info.nx_cc_updated = cc_updated & update_shard_cc;
        }

        validator_info.validator_list_hash_short = hash_short;

        // 6. update prev_blocks (add prev block's id to the dictionary)
        let key = self.new_block_id_part.seq_no == 1 || // prev block is a zerostate, not sure it is correct TODO
                  state_extra.after_key_block;
        let mut prev_blocks = state_extra.prev_blocks.clone();
        let prev_blk_ref = ExtBlkRef {
            end_lt: prev_data.prev_state_lt(),
            seq_no: prev_data.state().block_id().seq_no,
            root_hash: prev_data.state().block_id().root_hash.clone(),
            file_hash: prev_data.state().block_id().file_hash.clone(),
        };

        prev_blocks.set(
            &self.prev_blocks_ids[0].seq_no,
            &KeyExtBlkRef { key, blk_ref: prev_blk_ref.clone() },
            &KeyMaxLt { key, max_end_lt: prev_data.prev_state_lt() },
        )?;

        // 7. update after_key_block:Bool and last_key_block:(Maybe ExtBlkRef)
        let last_key_block = if state_extra.after_key_block {
            Some(prev_blk_ref)
        } else {
            state_extra.last_key_block.clone()
        };

        // 8. update global balance
        let mut global_balance = state_extra.global_balance.clone();
        global_balance.add(&collator_data.value_flow.created)?;
        global_balance.add(&collator_data.value_flow.minted)?;
        global_balance.add(&collator_data.shard_fees().root_extra().create)?;

        // 9. update block creator stats
        let block_create_stats =
            if state_extra.config().has_capability(GlobalCapabilities::CapCreateStatsEnabled) {
                let mut stat = state_extra.block_create_stats.clone().unwrap_or_default();
                self.update_block_creator_stats(collator_data, &mut stat)?;
                Some(stat)
            } else {
                None
            };

        Ok((
            McStateExtra {
                shards: collator_data.shards()?.clone(),
                config,
                validator_info,
                prev_blocks,
                after_key_block: is_key_block,
                last_key_block,
                block_create_stats,
                global_balance,
            },
            min_ref_mc_seqno,
        ))
    }

    fn update_shard_config(
        &self,
        collator_data: &mut CollatorData,
        wc_set: &Workchains,
        update_cc: bool,
    ) -> Result<u32> {
        log::trace!(
            "{}: update_shard_config, (update_cc: {})",
            self.collated_block_descr,
            update_cc
        );

        let mut min_ref_mc_seqno = u32::MAX;

        // TODO iterate_shards_with_siblings_mut when it will be done

        let old_shards = collator_data.shards()?.clone();
        old_shards.iterate_shards_with_siblings(|shard, mut descr, mut sibling| {
            min_ref_mc_seqno = min(min_ref_mc_seqno, descr.min_ref_mc_seqno);

            let unchanged_sibling = sibling.clone();

            let mut update_one_shard =
                |ident, descr: &mut ShardDescr, sibling: Option<&ShardDescr>| -> Result<()> {
                    min_ref_mc_seqno = min(min_ref_mc_seqno, descr.min_ref_mc_seqno);
                    let updated = self.update_one_shard(
                        collator_data,
                        &ident,
                        descr,
                        sibling,
                        wc_set.get(&ident.workchain_id())?.as_ref(),
                        update_cc,
                    )?;
                    if updated {
                        collator_data.shards_mut()?.update_shard(&ident, |_| Ok(descr.clone()))?;
                    }
                    Ok(())
                };

            if let Some(sibling) = sibling.as_mut() {
                update_one_shard(shard.sibling(), sibling, Some(&descr))?;
            }

            update_one_shard(shard.clone(), &mut descr, unchanged_sibling.as_ref())?;

            Ok(true)
        })?;

        Ok(min_ref_mc_seqno)
    }

    fn update_one_shard(
        &self,
        collator_data: &CollatorData,
        shard: &ShardIdent,
        info: &mut ShardDescr,
        sibling: Option<&ShardDescr>,
        wc_info: Option<&WorkchainDescr>, // new wc config (with changes made in the current block)
        mut update_cc: bool,
    ) -> Result<bool> {
        log::trace!("{}: update_one_shard {}", self.collated_block_descr, shard);

        let now = collator_data.gen_utime();
        let mut changed = false;
        let old_before_merge = info.before_merge;
        info.before_merge = false;

        #[allow(clippy::if_same_then_else)]
        if !info.is_fsm_none() && (now >= info.fsm_utime_end() || info.before_split) {
            info.split_merge_at = FutureSplitMerge::None;
            changed = true;
        } else if info.is_fsm_merge()
            && (sibling.is_none() || sibling.as_ref().unwrap().before_split)
        {
            info.split_merge_at = FutureSplitMerge::None;
            changed = true;
        }

        if !info.before_split {
            if let Some(wc_info) = &wc_info {
                // workchain present in configuration?
                let depth = shard.prefix_len();
                if info.is_fsm_none() &&                                // split/merge is not in progress
                   (info.want_split || depth < wc_info.min_split()) &&  // shard want splits (because of limits) or min_split was increased ↑ (in current or prev blocks)
                   depth < wc_info.max_split() &&                       // max_split allows split
                   depth < 60
                // hardcoded max max split allows split
                {
                    // prepare split
                    info.split_merge_at = FutureSplitMerge::Split {
                        split_utime: now + SPLIT_MERGE_DELAY,
                        interval: SPLIT_MERGE_INTERVAL,
                    };
                    changed = true;
                    log::debug!(
                        "{}: preparing to split shard {} during {}..{}",
                        self.collated_block_descr,
                        shard,
                        info.fsm_utime(),
                        info.fsm_utime_end()
                    );
                } else if let Some(sibling) = sibling {
                    if info.is_fsm_none() &&                                // split/merge is not in progress
                       depth > wc_info.min_split() &&                       // current min_split allows merge
                      (info.want_merge || depth > wc_info.max_split()) &&   // shard wants merge (because of limits) or max_split was decreased ↓ (in current or prev blocks)
                      !sibling.before_split && sibling.is_fsm_none() &&     // sibling shard is not going to split/merge now
                      (sibling.want_merge || depth > wc_info.max_split())
                    // sibling shard want merge or need merge (because of max_split)
                    {
                        // prepare merge
                        info.split_merge_at = FutureSplitMerge::Merge {
                            merge_utime: now + SPLIT_MERGE_DELAY,
                            interval: SPLIT_MERGE_INTERVAL,
                        };
                        changed = true;
                        log::debug!(
                            "{}: preparing to merge shard {} with {} during {}..{}",
                            self.collated_block_descr,
                            shard,
                            shard.sibling(),
                            info.fsm_utime(),
                            info.fsm_utime_end()
                        );
                    } else if info.is_fsm_merge() &&                                               // merge is in progress
                         depth > wc_info.min_split() &&                                            // min_split allows merge
                        !sibling.before_split &&                                                   // sibling is not going to split
                         sibling.is_fsm_merge() &&                                                 // sibling is in merge progress too
                        (depth > wc_info.max_split() || (info.want_merge && sibling.want_merge))
                    // max_split was decreased or both shards want merge
                    {
                        // merge time come
                        if now >= info.fsm_utime() && now >= sibling.fsm_utime() {
                            info.before_merge = true;
                            changed = true;
                            log::debug!(
                                "{}: force immediate merging of shard {} with {}",
                                self.collated_block_descr,
                                shard,
                                shard.sibling()
                            );
                        }
                    }
                }
            }
        }

        if info.before_merge != old_before_merge {
            update_cc |= old_before_merge;
            changed = true;
        }

        if update_cc {
            info.next_catchain_seqno += 1;
            changed = true;
        }

        if changed {
            log::trace!(
                "{}: update_one_shard {} changed {:?}",
                self.collated_block_descr,
                shard,
                info
            );
        }

        Ok(changed)
    }

    fn update_shard_block_info(
        &self,
        shards: &mut ShardHashes,
        mut new_info: McShardRecord,
        old_blkids: &[BlockIdExt],
        shards_updated: Option<&mut HashSet<ShardIdent>>,
    ) -> Result<()> {
        let (before_split, ancestor) = may_update_shard_block_info(
            shards,
            &new_info,
            old_blkids,
            !0,
            shards_updated.as_deref(),
        )?;

        if before_split {
            fail!(
                "cannot apply the after-split update for {} \
                without a corresponding sibling update",
                new_info.block_id()
            );
        }
        if let Some(ancestor) = ancestor {
            if ancestor.descr.split_merge_at != FutureSplitMerge::None {
                new_info.descr.split_merge_at = ancestor.descr.split_merge_at;
            }
        }

        let shard = new_info.shard().clone();

        if old_blkids.len() == 2 {
            shards.merge_shards(&shard, |_, _| Ok(new_info.descr))?;
        } else {
            shards.update_shard(&shard, |_| Ok(new_info.descr))?;
        }

        if let Some(shards_updated) = shards_updated {
            shards_updated.insert(shard);
        }
        Ok(())
    }

    fn update_shard_block_info2(
        &self,
        shards: &mut ShardHashes,
        mut new_info1: McShardRecord,
        mut new_info2: McShardRecord,
        old_blkids: &[BlockIdExt],
        shards_updated: Option<&mut HashSet<ShardIdent>>,
    ) -> Result<()> {
        let (before_split_1, _) = may_update_shard_block_info(
            shards,
            &new_info1,
            old_blkids,
            !0,
            shards_updated.as_deref(),
        )?;
        let (before_split_2, _) = may_update_shard_block_info(
            shards,
            &new_info2,
            old_blkids,
            !0,
            shards_updated.as_deref(),
        )?;

        if !before_split_1 || !before_split_2 {
            fail!(
                "the two updates in update_shard_block_info2 \
                must follow a shard split event"
            );
        }
        if new_info1.shard().shard_prefix_with_tag() > new_info2.shard().shard_prefix_with_tag() {
            mem::swap(&mut new_info1, &mut new_info2);
        }

        let shard1 = new_info1.shard().clone();

        shards
            .split_shard(&new_info1.shard().merge()?, |_| Ok((new_info1.descr, new_info2.descr)))?;

        if let Some(shards_updated) = shards_updated {
            shards_updated.insert(shard1);
        }

        Ok(())
    }

    // reinit shard collators when new network config is applied

    fn update_block_creator_stats(
        &self,
        collator_data: &CollatorData,
        block_create_stats: &mut BlockCreateStats,
    ) -> Result<()> {
        log::trace!("{}: update_block_creator_stats", self.collated_block_descr);

        for (creator, count) in collator_data.block_create_count().iter() {
            self.update_block_creator_count(
                block_create_stats,
                collator_data.gen_utime(),
                creator,
                *count,
                0,
            )?;
        }

        let has_creator = self.created_by != UInt256::default();
        if has_creator {
            self.update_block_creator_count(
                block_create_stats,
                collator_data.gen_utime(),
                &self.created_by,
                0,
                1,
            )?;
        }
        if has_creator || collator_data.block_create_total() > 0 {
            self.update_block_creator_count(
                block_create_stats,
                collator_data.gen_utime(),
                &UInt256::default(),
                collator_data.block_create_total(),
                if has_creator { 1 } else { 0 },
            )?;
        }

        let mut rng = rand::thread_rng();
        let key: [u8; 32] = rng.gen();
        let mut key: UInt256 = key.into();
        let mut scanned = 0;
        let mut removed = 0;
        while scanned < 100 {
            let stat = block_create_stats.counters.find_leaf(&key, false, false, false)?;
            if let Some((found_key, mut stat)) = stat {
                let res =
                    self.creator_count_outdated(&found_key, collator_data.gen_utime(), &mut stat)?;
                if !res {
                    log::trace!(
                        "{}: prunning CreatorStats for {:x}",
                        self.collated_block_descr,
                        found_key
                    );
                    block_create_stats.counters.remove(&found_key)?;
                    removed += 1;
                }
                scanned += 1;
                key = found_key;
            } else {
                break;
            }
        }
        log::trace!(
            "{}: removed {} stale CreatorStats entries out of {} scanned",
            self.collated_block_descr,
            removed,
            scanned
        );
        Ok(())
    }

    fn update_block_creator_count(
        &self,
        stats: &mut BlockCreateStats,
        now: u32,
        key: &UInt256,
        shard_incr: u64,
        mc_incr: u64,
    ) -> Result<()> {
        log::trace!(
            "{}: update_block_creator_count, key {:x}, shard_incr {}, mc_incr {}",
            self.collated_block_descr,
            key,
            shard_incr,
            mc_incr
        );

        let mut stat = stats.counters.get(key)?.unwrap_or_default();
        if mc_incr > 0 && !stat.mc_blocks.increase_by(mc_incr, now) {
            fail!(
                "cannot increase masterchain block counter in CreatorStats for {:x} by {} \
                (old value is {:?})",
                key,
                mc_incr,
                stat.mc_blocks
            );
        }
        if shard_incr > 0 && !stat.shard_blocks.increase_by(shard_incr, now) {
            fail!(
                "cannot increase shardchain block counter in CreatorStats for {:x} by {} \
                (old value is {:?})",
                key,
                shard_incr,
                stat.shard_blocks
            );
        };
        stats.counters.set(key, &stat)?;
        Ok(())
    }

    fn creator_count_outdated(
        &self,
        key: &UInt256,
        now: u32,
        stat: &mut CreatorStats,
    ) -> Result<bool> {
        log::trace!("{}: creator_count_outdated, key {:x}", self.collated_block_descr, key);

        if !(stat.mc_blocks.increase_by(0, now) && stat.shard_blocks.increase_by(0, now)) {
            fail!("cannot amortize counters in CreatorStats for {:x}", key);
        }
        if 0 == (stat.mc_blocks.cnt65536() | stat.shard_blocks.cnt65536()) {
            log::trace!("{}: removing stale CreatorStats for {:x}", self.collated_block_descr, key);
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn init_timeout(&mut self) {
        self.started = Instant::now();

        let stop_timeout = self.engine.collator_config().stop_timeout_ms;
        let stop_flag = self.stop_flag.clone();
        tokio::spawn(async move {
            futures_timer::Delay::new(Duration::from_millis(stop_timeout as u64)).await;
            stop_flag.store(true, Ordering::Relaxed);
        });
    }

    fn check_cutoff_timeout(&self) -> bool {
        let cutoff_timeout = self.engine.collator_config().cutoff_timeout_ms;
        self.started.elapsed().as_millis() as u32 > cutoff_timeout
    }

    fn get_remaining_cutoff_time_limit_nanos(&self) -> i128 {
        let cutoff_timeout_nanos =
            self.engine.collator_config().cutoff_timeout_ms as i128 * 1_000_000;
        let elapsed_nanos = self.started.elapsed().as_nanos() as i128;
        cutoff_timeout_nanos - elapsed_nanos
    }

    fn get_remaining_clean_time_limit_nanos(&self) -> i128 {
        let remaining_cutoff_timeout_nanos = self.get_remaining_cutoff_time_limit_nanos();
        let cc = self.engine.collator_config();
        let max_secondary_clean_timeout_nanos = (cc.cutoff_timeout_ms as i128)
            * 1_000_000
            * (cc.max_secondary_clean_timeout_percentage_points as i128)
            / 1000;
        remaining_cutoff_timeout_nanos.min(max_secondary_clean_timeout_nanos)
    }

    fn get_external_messages_finish_time_micros(&self) -> u64 {
        let now = self.engine.now_ms();
        let cc = self.engine.collator_config();
        now + (cc.cutoff_timeout_ms * cc.external_messages_timeout_percentage_points / 1000) as u64
    }

    fn check_stop_flag(&self) -> Result<()> {
        if self.stop_flag.load(Ordering::Relaxed) {
            fail!("Stop flag was set")
        }
        Ok(())
    }

    fn create_collated_data<'a>(
        &self,
        collator_data: &CollatorData,
        prev_data: &PrevData,
        accounts: impl Iterator<Item = &'a ShardAccountStuff>,
        output_queue_manager: &MsgQueueManager,
    ) -> Result<Vec<u8>> {
        let mut roots = Vec::new();

        // 1. store the set of used shard block descriptions
        if !collator_data.shard_top_block_descriptors.is_empty() {
            let mut tbds = TopBlockDescrSet::default();
            for stbd in &collator_data.shard_top_block_descriptors {
                tbds.insert(stbd.proof_for().shard(), stbd.top_block_descr())?;
            }
            roots.push(tbds.serialize()?);
        }

        // 1.2 store info for simplex consensus (C++ parity)
        if self.collator_settings.is_simplex {
            let extra = ton_block::ConsensusExtraData {
                flags: 0,
                gen_utime_ms: collator_data.gen_utime_ms(),
            };
            roots.push(extra.serialize()?);
        }

        let collated_data_flags = if collator_data
            .config
            .raw_config()
            .consensus_config()
            .map(|c| c.proto_version >= 5)
            .unwrap_or(false)
        {
            BocFlags::Crc32
        } else {
            BocFlags::all()
        };

        if !collator_data.config.has_capability(GlobalCapabilities::CapFullCollatedData) {
            if roots.is_empty() {
                return Ok(vec![]);
            } else {
                return BocWriter::with_flags(roots, collated_data_flags)?.write_to_vec();
            }
        }

        // add required cells to the proof:
        // TODO: this code is similar to the one in C++ collator implementation,
        // looks like scan_diff in C++ works a bit differently, because Rust validator
        // successsfully verifies the block without adding all these cells,
        // while C++ validator fails with pruned cells error. Probably need to revise
        // and fix C++ implementation later and then unify the code here.
        if !self.shard.is_masterchain() {
            output_queue_manager.prev().out_queue().scan_diff_with_aug(
                output_queue_manager.next().out_queue(),
                |_, val1, val2| {
                    if let Some((enq, lt)) = val1 {
                        let _ = MsgEnqueueStuff::from_enqueue(enq, lt)?;
                    }
                    if let Some((enq, lt)) = val2 {
                        let _ = MsgEnqueueStuff::from_enqueue(enq, lt)?;
                    }
                    Ok(true)
                },
            )?;
            output_queue_manager.prev().out_queue_extra().dispatch_queue().scan_diff_with_aug(
                collator_data.out_msg_queue_info.out_queue_extra().dispatch_queue(),
                |_, val1, val2| {
                    if let Some((queue, _)) = val1 {
                        queue.messages().find_min_max_key::<u64>(false, false)?;
                    }
                    if let Some((queue, _)) = val2 {
                        queue.messages().find_min_max_key::<u64>(true, false)?;
                    }
                    Ok(true)
                },
            )?;
        }

        // 2. Proofs for hashes of states: previous states + neighbors
        for (block_id, proof) in output_queue_manager.block_proofs() {
            if output_queue_manager.neighbor_usages().contains_key(block_id) {
                roots.push(proof.clone());
            }
        }

        let prev_states = if self.shard.is_masterchain() {
            vec![]
        } else if self.after_merge {
            let shard_split = ShardStateSplit::construct_from_cell(prev_data.state_root.clone())?;
            vec![shard_split.left, shard_split.right]
        } else {
            vec![prev_data.state_root.clone()]
        };

        // 3. Proofs for message queues
        let mut neighbours_msg_queue_visited =
            Vec::with_capacity(output_queue_manager.neighbor_usages().len());
        for neighbour_usage in output_queue_manager.neighbor_usages().values() {
            neighbours_msg_queue_visited.push(neighbour_usage.build_visited_set());

            if prev_states.contains(&neighbour_usage.original_root()) {
                continue;
            }

            let proof = MerkleProof::create_by_usage_tree(
                &neighbour_usage.original_root(),
                neighbour_usage,
            )?;
            roots.push(proof.serialize()?);
        }

        // 4. Previous state proof (only shadchains) and storage dict proofs
        if !self.shard.is_masterchain() {
            let mut roots_to_include = HashSet::new();
            let mut ss_visited = collator_data.usage_tree.build_visited_set();
            // extend with visited cells from neighbour msg queues to properly calculate previous state proof size difference:
            // if some cell is already included in neighbour msg queue proof, it should not be counted again
            // and we also include message queue from the previous state into the proof
            ss_visited.extend(neighbours_msg_queue_visited.into_iter().flatten());
            for account_stuff in accounts {
                if let Some(dict_usage) = account_stuff.storage_dict_usage() {
                    let dict_proof_size = dict_usage.estimate_proof_serialized_size()?;
                    let dict = StorageStatDict::with_hashmap(Some(dict_usage.original_root()))
                        .export_keys::<UInt256>()?;
                    let mut ss_visited_update = ss_visited.clone();
                    let mut ss_proof_size_diff = 0;
                    for update in account_stuff.account_updates() {
                        ss_proof_size_diff += UsageTree::add_branch_to_visited(
                            update,
                            &mut ss_visited_update,
                            &|hash| dict.contains(hash),
                        )?;
                    }
                    if ss_proof_size_diff > dict_proof_size {
                        let proof = MerkleProof::create_by_usage_tree(
                            &dict_usage.original_root(),
                            dict_usage,
                        )?;
                        roots.push(
                            AccountStorageDictProof { proof: proof.serialize()? }.serialize()?,
                        );
                        log::debug!(
                            "Added storage dict proof with hash {:x} for account {:x}",
                            dict_usage.original_root().repr_hash(),
                            account_stuff.account_id(),
                        );
                    } else {
                        ss_visited = ss_visited_update;
                    }
                } else {
                    log::debug!("Added full account state {:x} ", account_stuff.account_id());
                    roots_to_include.insert(account_stuff.original_root().repr_hash());
                }
            }
            for state in prev_states {
                let proof = MerkleProof::create_with_subtrees(
                    &state,
                    |hash| {
                        ss_visited.contains(hash) || collator_data.imported_visited.contains(hash)
                    },
                    |hash| roots_to_include.contains(hash),
                )?;
                roots.push(proof.serialize()?);
            }
        }

        if roots.is_empty() {
            Ok(vec![])
        } else {
            BocWriter::with_flags(roots, collated_data_flags)?.write_to_vec()
        }
    }
}

#[test]
fn test_count_bits_u64() {
    fn count_bits(mut value: u64) -> isize {
        let mut result = 0;
        while value > 0 {
            result += (value & 1) as isize;
            value >>= 1;
        }
        result
    }
    let test_cases = vec![
        0,
        1,
        2,
        3,
        0b101011_10110111,
        0b01111010_11101101_11101011_10110111u64,
        0b01111010_11101101_11101011_10110111_01111010_11101101_11101011_10110111u64,
        0xFFFFFFFFFFFFFFFF,
    ];

    for test_case in test_cases {
        assert_eq!(
            CollatorData::count_bits_u64(test_case),
            count_bits(test_case),
            "test case: {}",
            test_case
        );
    }
}

pub fn report_collation_metrics(
    shard: &ShardIdent,
    dequeue_msg_count: usize,
    enqueue_msg_count: usize,
    in_msg_count: usize,
    out_msg_count: usize,
    transit_msg_count: usize,
    executed_trs_count: usize,
    gas_used: u32,
    gas_rate: u32,
    block_size: usize,
    time: u32,
) {
    let labels = [("shard", shard.to_string())];
    metrics::histogram!("ton_node_collator_duration_seconds", &labels)
        .record((time as f64) / 1000.0);
    metrics::counter!("ton_node_collator_dequeued_messages_total", &labels)
        .increment(dequeue_msg_count as u64);
    metrics::counter!("ton_node_collator_enqueued_messages_total", &labels)
        .increment(enqueue_msg_count as u64);
    metrics::counter!("ton_node_collator_inbound_messages_total", &labels)
        .increment(in_msg_count as u64);
    metrics::counter!("ton_node_collator_outbound_messages_total", &labels)
        .increment(out_msg_count as u64);
    metrics::counter!("ton_node_collator_transit_messages_total", &labels)
        .increment(transit_msg_count as u64);
    metrics::counter!("ton_node_collator_executed_transactions_total", &labels)
        .increment(executed_trs_count as u64);
    metrics::histogram!("ton_node_collator_gas_used", &labels).record(gas_used as f64);
    metrics::histogram!("ton_node_collator_gas_rate_ratio", &labels).record(gas_rate as f64);
    metrics::histogram!("ton_node_block_size_bytes", &labels).record(block_size as f64);
}

#[cfg(test)]
use ton_block::ExceptionCode;
#[cfg(test)]
#[path = "tests/test_collator.rs"]
mod tests;
