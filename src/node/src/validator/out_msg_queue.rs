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
use super::out_msg_queue_cleaner;
use crate::{
    block::BlockStuff,
    engine_traits::EngineOperations,
    shard_state::{ShardHashesStuff, ShardStateStuff},
    types::messages::MsgEnqueueStuff,
    validator::validator_group::PipelineContext,
    CHECK,
};
use std::{
    cmp::{max, min},
    collections::{
        btree_map::{self, BTreeMap},
        HashMap, HashSet,
    },
    fmt::{Debug, Display, Formatter},
    iter::Iterator,
    mem,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use ton_block::{
    error, fail, AccountIdPrefixFull, Block, BlockIdExt, BocWriter, BuilderData, Cell,
    Deserializable, HashmapAugType, HashmapFilterResult, HashmapFilterSplitResult, HashmapRemover,
    HashmapSubtree, HashmapType, IBitstring, ImportedMsgQueueLimits, LabelReader, MerkleProof,
    OutMsgQueue, OutMsgQueueExtra, OutMsgQueueInfo, OutMsgQueueKey, ProcessedInfo,
    ProcessedInfoKey, ProcessedUpto, Result, Serializable, ShardHashes, ShardIdent,
    ShardStateUnsplit, SliceData, UInt256, UsageTree, MASTERCHAIN_ID,
};

#[cfg(test)]
#[path = "tests/test_out_msg_queue.rs"]
mod tests;

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ProcessedUptoStuff {
    /// An abstract at-least-an-ancestor shard which can refer to
    /// a newly created shard during split.
    pub shard: u64,

    /// Block seqno with a different meaning depending on a context:
    /// - Masterchain block seqno if used without a direct intershard communication.
    /// - A block seqno, corresponding to [`exact_shard`] otherwise.
    ///
    /// [`exact_shard`]: ProcessedUptoStuff::exact_shard
    pub seqno: u32,

    pub last_msg_lt: u64,
    pub last_msg_hash: UInt256,

    /// An original shard in case of altering [`ProcessedUptoStuff::shard`].
    #[cfg(feature = "xp25")]
    original_shard: Option<u64>,
    /// A computed masterchain block seqno.
    #[cfg(feature = "xp25")]
    real_mc_seqno: u32,

    mc_end_lt: u64,
    ref_shards: Option<ShardHashes>,
}

impl ProcessedUptoStuff {
    pub fn with_params(
        shard: u64,
        seqno: u32,
        last_msg_lt: u64,
        last_msg_hash: UInt256,
        #[cfg(feature = "xp25")] original_shard: Option<u64>,
        #[cfg(feature = "xp25")] real_mc_seqno: u32,
    ) -> Self {
        Self {
            shard,
            seqno,
            last_msg_lt,
            last_msg_hash,
            #[cfg(feature = "xp25")]
            original_shard,
            #[cfg(feature = "xp25")]
            real_mc_seqno,
            mc_end_lt: 0,
            ref_shards: None,
        }
    }

    #[cfg(feature = "xp25")]
    pub fn exact_shard(&self) -> u64 {
        self.original_shard.unwrap_or(self.shard)
    }

    #[cfg(feature = "xp25")]
    pub fn mc_seqno(&self) -> u32 {
        self.real_mc_seqno
    }
    #[cfg(not(feature = "xp25"))]
    pub fn mc_seqno(&self) -> u32 {
        self.seqno
    }

    pub fn contains(&self, other: &Self) -> bool {
        // NOTE: an abstract shards are checked here.
        // In case of direct intershard communication `mc_seqno` does not change
        // its order properties:
        //   - `shard` field behaves the same (we use an additional field `original_shard`
        //     if we need to know an exact shard)
        //   - `mc_seqno` as shard seqno grows the same way as masterchain seqno

        ShardIdent::is_ancestor(self.shard, other.shard)
            && self.seqno >= other.seqno
            && ((self.last_msg_lt > other.last_msg_lt)
                || ((self.last_msg_lt == other.last_msg_lt)
                    && (self.last_msg_hash >= other.last_msg_hash)))
    }
    pub fn can_check_processed(&self) -> bool {
        self.ref_shards.is_some()
    }

    fn already_processed(&self, enq: &MsgEnqueueStuff) -> Result<bool> {
        log::trace!(
            "already_processed: shard={:016x}, last_msg_lt={}, last_msg_hash={:x}, \
            cur_prefix={:016x}, dst={:016x}, enq_hash={:x}",
            self.shard,
            self.last_msg_lt,
            self.last_msg_hash,
            enq.cur_prefix().prefix,
            enq.dst_prefix().prefix,
            enq.message_hash(),
        );
        if enq.lt() > self.last_msg_lt {
            log::trace!(
                "already_processed: enq_hash={:x} `enq.created_lt() > self.last_msg_lt`",
                enq.message_hash()
            );
            return Ok(false);
        }
        if !ShardIdent::contains(self.shard, enq.next_prefix().prefix) {
            log::trace!(
                "already_processed: enq_hash={:x} `!ShardIdent::contains(next_prefix)`",
                enq.message_hash()
            );
            return Ok(false);
        }
        if enq.lt() == self.last_msg_lt && self.last_msg_hash < enq.message_hash() {
            log::trace!(
                "already_processed: enq_hash={:x} `enq.created_lt() == self.last_msg_lt`",
                enq.message_hash()
            );
            return Ok(false);
        }
        if enq.same_workchain() && ShardIdent::contains(self.shard, enq.cur_prefix().prefix) {
            log::trace!(
                "already_processed: enq_hash={:x} `ShardIdent::contains(cur_prefix)`",
                enq.message_hash()
            );
            // this branch is needed only for messages generated in the same shard
            // (such messages could have been processed without a reference from the masterchain)
            // enable this branch only if an extra boolean parameter is set
            return Ok(true);
        }
        let shard_end_lt = self.compute_shard_end_lt(enq.cur_prefix())?;
        log::trace!(
            "already_processed: enq_hash={:x} shard_end_lt={shard_end_lt}, processed={}",
            enq.message_hash(),
            enq.enqueued_lt() < shard_end_lt
        );
        Ok(enq.enqueued_lt() < shard_end_lt)
    }

    pub fn compute_shard_end_lt(&self, prefix: &AccountIdPrefixFull) -> Result<u64> {
        let shard_end_lt = if prefix.is_masterchain() {
            self.mc_end_lt
        } else {
            let shard = self
                .ref_shards
                .as_ref()
                .ok_or_else(|| {
                    error!(
                        "ProcessedUpTo record for {} ({}:{:x}) has no info about shards",
                        self.seqno, self.last_msg_lt, self.last_msg_hash
                    )
                })?
                .find_shard_by_prefix(prefix)?
                .ok_or_else(|| {
                    error!(
                        "ProcessedUpTo record for {} ({}:{:x}) has no info about shard prefix {}",
                        self.seqno, self.last_msg_lt, self.last_msg_hash, prefix
                    )
                })?;

            log::trace!(
                "compute_shard_end_lt: prefix={:016x}, seqno={:016x}, end_lt={}, full_id={}",
                prefix.prefix,
                self.seqno,
                shard.descr().end_lt,
                shard.block_id()
            );

            shard.descr().end_lt
        };
        Ok(shard_end_lt)
    }
}

impl Display for ProcessedUptoStuff {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "shard: {:016X}, mc_seqno: {}, mc_end_lt: {}, last_msg_lt: {}, last_msg_hash: {:x}",
            self.shard, self.seqno, self.mc_end_lt, self.last_msg_lt, self.last_msg_hash
        )
    }
}

#[derive(Default, Clone)]
pub struct OutMsgQueueInfoStuff {
    block_id: BlockIdExt,
    out_queue: OutMsgQueue,
    out_queue_extra: OutMsgQueueExtra,
    entries: Vec<ProcessedUptoStuff>,
    min_seqno: u32,
    end_lt: u64,
    disabled: bool,
}

impl OutMsgQueueInfoStuff {
    pub async fn from_shard_state(
        state: &ShardStateStuff,
        states_manager: &mut StatesManager,
    ) -> Result<Self> {
        Self::from_out_queue_info(
            state.block_id().clone(),
            state.state()?.read_out_msg_queue_info()?,
            state.gen_lt()?,
            states_manager,
        )
        .await
    }

    async fn from_out_queue_info(
        block_id: BlockIdExt,
        out_queue_info: OutMsgQueueInfo,
        end_lt: u64,
        states_manager: &mut StatesManager,
    ) -> Result<Self> {
        Self::with_params(
            block_id,
            out_queue_info.out_queue().clone(),
            out_queue_info.proc_info(),
            out_queue_info.extra().clone(),
            end_lt,
            states_manager,
        )
        .await
    }

    async fn with_params(
        block_id: BlockIdExt,
        out_queue: OutMsgQueue,
        proc_info: &ProcessedInfo,
        out_queue_extra: OutMsgQueueExtra,
        end_lt: u64,
        states_manager: &mut StatesManager,
    ) -> Result<Self> {
        // NOTE: no new states are loaded for an old implementation
        #[cfg(not(feature = "xp25"))]
        let _ = states_manager;

        // unpack ProcessedUptoStuff
        let mut entries = vec![];
        let mut min_seqno = u32::MAX;

        for item in proc_info.clone().inner().iter() {
            let (key, mut value) = item?;
            let key = ProcessedInfoKey::construct_from(&mut SliceData::load_builder(key)?)?;
            let value = ProcessedUpto::construct_from(&mut value)?;
            #[cfg(not(feature = "xp25"))]
            if value.original_shard.is_some() {
                fail!("ProcessedUpto could not contain original_shard");
            }

            #[cfg(not(feature = "xp25"))]
            let entry = {
                if key.mc_seqno < min_seqno {
                    min_seqno = key.mc_seqno;
                }
                ProcessedUptoStuff::with_params(
                    key.shard,
                    key.mc_seqno,
                    value.last_msg_lt,
                    value.last_msg_hash,
                )
            };

            #[cfg(feature = "xp25")]
            let entry = {
                let real_mc_seqno = if block_id.shard().is_masterchain() {
                    key.mc_seqno
                } else {
                    let shard = ShardIdent::with_tagged_prefix(
                        block_id.shard().workchain_id(),
                        value.original_shard.unwrap_or(key.shard),
                    )?;

                    let state = states_manager
                        .request_shard_state(shard, key.mc_seqno, Some(10_000))
                        .await?;

                    state
                        .state()?
                        .master_ref()
                        .ok_or_else(|| error!("shard state {block_id} doesn't have master_ref"))?
                        .master
                        .seq_no
                };

                if real_mc_seqno < min_seqno {
                    min_seqno = real_mc_seqno;
                };

                ProcessedUptoStuff::with_params(
                    key.shard,
                    key.mc_seqno,
                    value.last_msg_lt,
                    value.last_msg_hash,
                    value.original_shard,
                    real_mc_seqno,
                )
            };

            entries.push(entry);
        }

        Ok(Self {
            block_id,
            out_queue,
            out_queue_extra,
            entries,
            min_seqno,
            end_lt,
            disabled: false,
        })
    }

    fn merge(&mut self, other: &Self) -> Result<()> {
        let shard = self.shard().merge()?;

        self.out_queue_mut().combine_with(other.out_queue())?;
        self.out_queue_mut().update_root_extra()?;
        self.out_queue_extra_mut().merge(other.out_queue_extra(), &shard.shard_key(false))?;
        for entry in &other.entries {
            if self.min_seqno > entry.mc_seqno() {
                self.min_seqno = entry.mc_seqno();
            }
            self.entries.push(entry.clone());
        }
        self.block_id = BlockIdExt::with_params(
            shard,
            max(self.block_id.seq_no, other.block_id.seq_no),
            UInt256::default(),
            UInt256::default(),
        );
        self.compactify()?;
        Ok(())
    }

    fn calc_split_queues(
        self_queue: &mut OutMsgQueue,
        old_shard: &ShardIdent,
        shard: &ShardIdent,
    ) -> Result<OutMsgQueue> {
        self_queue.hashmap_filter_split(|key, mut slice| {
            let lt = u64::construct_from(&mut slice)?;
            // we need only cur prefix but deserialize all
            let enq = MsgEnqueueStuff::construct_from(&mut slice, lt)?;
            if !old_shard.contains_full_prefix(enq.cur_prefix()) {
                fail!(
                    "OutMsgQueue message with key {} does not \
                    contain current address belonging to shard {old_shard}",
                    hex::encode(key.data())
                )
            }
            if shard.contains_full_prefix(enq.cur_prefix()) {
                Ok(HashmapFilterSplitResult::Stay)
            } else {
                Ok(HashmapFilterSplitResult::Move)
            }
        })
    }

    pub async fn precalc_split_queues(
        engine: &Arc<dyn EngineOperations>,
        block_id: &BlockIdExt,
    ) -> Result<()> {
        if engine.set_split_queues_calculating(block_id) {
            let ss = engine.clone().wait_state(block_id, Some(10_000), false).await?;
            let usage_tree = UsageTree::with_params(ss.root_cell().clone(), true);
            let root_cell = usage_tree.root_cell();
            let ss = ShardStateStuff::from_state(
                block_id.clone(),
                ShardStateUnsplit::construct_from_cell(root_cell)?,
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry(),
                engine.engine_allocated(),
            )?;
            let mut queue0 = ss.state()?.read_out_msg_queue_info()?;
            let queue0 = queue0.out_queue_mut();
            let (s0, _s1) = block_id.shard().split()?;
            let now = Instant::now();
            let queue1 = Self::calc_split_queues(queue0, block_id.shard(), &s0)?;
            log::info!(
                "precalc_split_queues after block {}, TIME {}ms",
                block_id,
                now.elapsed().as_millis()
            );
            engine.set_split_queues(
                block_id,
                queue0.clone(),
                queue1,
                usage_tree.build_visited_set(),
            );
        } else {
            log::trace!("precalc_split_queues {} already calculating or calculated", block_id);
        }
        Ok(())
    }

    fn split(
        &mut self,
        subshard: ShardIdent,
        engine: &Arc<dyn EngineOperations>,
        usage_tree: Option<&UsageTree>,
        imported_visited: Option<&mut HashSet<UInt256>>,
    ) -> Result<Self> {
        let shard = self.block_id().shard().clone();
        let (s0, _s1) = shard.split()?;
        let (sibling_queue, size0) = if let Some((q0, q1, visited)) =
            engine.get_split_queues(self.block_id())
        {
            let (size0, size1) = q0.concurent_len(&q1, self.out_queue_extra().out_queue_size)?;
            if let Some(imported_visited) = imported_visited {
                for cell_id in visited {
                    imported_visited.insert(cell_id);
                }
            }
            log::info!("Use split queues from cache (prev block {})", self.block_id());
            if s0 == subshard {
                self.out_queue = q0;
                (q1, size0)
            } else {
                self.out_queue = q1;
                (q0, size1)
            }
        } else {
            let now = Instant::now();
            let sibling_queue = Self::calc_split_queues(self.out_queue_mut(), &shard, &subshard)?;
            let (size0, size1) = self
                .out_queue()
                .concurent_len(&sibling_queue, self.out_queue_extra().out_queue_size)?;
            debug_assert_eq!(size0, self.out_queue().len().unwrap());
            debug_assert_eq!(size1, sibling_queue.len().unwrap());
            if let Some(usage_tree) = usage_tree {
                let (q0, q1) = if s0 == subshard {
                    (self.out_queue().clone(), sibling_queue.clone())
                } else {
                    (sibling_queue.clone(), self.out_queue().clone())
                };
                let visited = usage_tree.build_visited_set();
                engine.set_split_queues(self.block_id(), q0, q1, visited);
                log::warn!(
                    "There is no precalculated split queues (prev block {}), \
                        calculated TIME {}ms",
                    self.block_id(),
                    now.elapsed().as_millis()
                );
            }
            (sibling_queue, size0)
        };

        let sibling_out_queue_extra = self.out_queue_extra_mut().split(&subshard, size0)?;
        let sibling = subshard.sibling();

        let mut entries = vec![];
        let mut min_seqno = u32::MAX;
        self.min_seqno = min_seqno;
        for mut entry in mem::take(&mut self.entries).drain(..) {
            if ShardIdent::shard_intersects(entry.shard, sibling.shard_prefix_with_tag()) {
                let mut entry = entry.clone();
                #[cfg(feature = "xp25")]
                {
                    entry.original_shard = Some(entry.exact_shard());
                }
                entry.shard =
                    ShardIdent::shard_intersection(entry.shard, sibling.shard_prefix_with_tag());
                log::debug!("to sibling {}", entry);
                if min_seqno > entry.mc_seqno() {
                    min_seqno = entry.mc_seqno();
                }
                entries.push(entry);
            }
            if ShardIdent::shard_intersects(entry.shard, subshard.shard_prefix_with_tag()) {
                #[cfg(feature = "xp25")]
                {
                    entry.original_shard = Some(entry.exact_shard());
                }
                entry.shard =
                    ShardIdent::shard_intersection(entry.shard, subshard.shard_prefix_with_tag());
                log::debug!("to us {}", entry);
                if self.min_seqno > entry.mc_seqno() {
                    self.min_seqno = entry.mc_seqno();
                }
                self.entries.push(entry);
            }
        }
        self.compactify()?;
        self.block_id.shard_id = subshard;

        let block_id = BlockIdExt::with_params(
            sibling,
            self.block_id().seq_no,
            UInt256::default(),
            UInt256::default(),
        );
        let mut sibling = OutMsgQueueInfoStuff {
            block_id,
            out_queue: sibling_queue,
            out_queue_extra: sibling_out_queue_extra,
            entries,
            min_seqno,
            end_lt: self.end_lt,
            disabled: false,
        };
        sibling.compactify()?;
        Ok(sibling)
    }

    pub fn serialize(&self) -> Result<(OutMsgQueueInfo, u32)> {
        let mut min_seqno = u32::MAX;
        let mut proc_info = ProcessedInfo::default();
        for entry in &self.entries {
            min_seqno = min(min_seqno, entry.mc_seqno());
            let key = ProcessedInfoKey::with_params(entry.shard, entry.seqno);
            let value = ProcessedUpto::with_params(
                entry.last_msg_lt,
                entry.last_msg_hash.clone(),
                #[cfg(feature = "xp25")]
                entry.original_shard,
                #[cfg(not(feature = "xp25"))]
                None,
            );
            proc_info.set(&key, &value)?
        }
        Ok((
            OutMsgQueueInfo::with_params(
                self.out_queue().clone(),
                proc_info,
                self.out_queue_extra().clone(),
            ),
            min_seqno,
        ))
    }

    pub fn fix_processed_upto(
        &mut self,
        seqno: u32,
        next_mc_end_lt: u64,
        next_shards: Option<&ShardHashes>,
        states_manager: &StatesManager,
        stop_flag: &Option<&AtomicBool>,
    ) -> Result<()> {
        let workchain = self.shard().workchain_id();
        let masterchain = workchain == MASTERCHAIN_ID;
        for entry in &mut self.entries {
            if entry.ref_shards.is_none() {
                check_stop_flag(stop_flag)?;
                if next_shards.is_some() && masterchain && entry.seqno == seqno + 1 {
                    entry.mc_end_lt = next_mc_end_lt;
                    entry.ref_shards = next_shards.cloned();
                } else {
                    #[cfg(not(feature = "xp25"))]
                    let (shard, seqno) = (ShardIdent::masterchain(), min(entry.seqno, seqno));

                    #[cfg(feature = "xp25")]
                    let (shard, seqno) = (
                        ShardIdent::with_tagged_prefix(workchain, entry.exact_shard())?,
                        entry.seqno,
                    );

                    let (mc_end_lt, ref_shards) = states_manager.get_entry_data(&shard, seqno)?;

                    entry.mc_end_lt = mc_end_lt;
                    entry.ref_shards = Some(ref_shards);
                };
            }
        }
        Ok(())
    }
    pub fn block_id(&self) -> &BlockIdExt {
        &self.block_id
    }

    pub fn shard(&self) -> &ShardIdent {
        self.block_id.shard()
    }

    fn set_shard(&mut self, shard_ident: ShardIdent) {
        self.block_id.shard_id = shard_ident
    }

    fn disable(&mut self) {
        self.disabled = true
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn out_queue(&self) -> &OutMsgQueue {
        &self.out_queue
    }

    pub fn out_queue_mut(&mut self) -> &mut OutMsgQueue {
        &mut self.out_queue
    }

    pub fn out_queue_extra(&self) -> &OutMsgQueueExtra {
        &self.out_queue_extra
    }

    pub fn out_queue_extra_mut(&mut self) -> &mut OutMsgQueueExtra {
        &mut self.out_queue_extra
    }

    pub fn forced_fix_out_queue(&mut self) -> Result<()> {
        let queue = self.out_queue_mut();
        if queue.is_empty() && queue.root_extra() != &0 {
            queue.after_remove()?;
        }
        Ok(())
    }

    pub fn message(&self, key: &OutMsgQueueKey) -> Result<Option<MsgEnqueueStuff>> {
        self.out_queue()
            .get_with_aug(key)?
            .map(|(enq, lt)| MsgEnqueueStuff::from_enqueue(enq, lt))
            .transpose()
    }

    pub fn add_message(&mut self, enq: &MsgEnqueueStuff) -> Result<usize> {
        let key = enq.out_msg_key();
        let (_, depth) =
            self.out_queue_mut().set_with_prev_and_depth(&key, enq.inner(), &enq.lt())?;
        Ok(depth)
    }

    pub fn del_message(&mut self, key: &OutMsgQueueKey) -> Result<SliceData> {
        self.out_queue_mut()
            .remove(key.write_to_bitstring()?)?
            .ok_or_else(|| error!("error deleting from out_msg_queue dictionary: {:x}", key))
    }

    // remove all messages which are not from new_shard
    fn filter_messages(&mut self, new_shard: &ShardIdent) -> Result<()> {
        let old_shard = self.shard().clone();
        self.out_queue_mut().hashmap_filter(|_key, mut slice| {
            // log::debug!("scanning OutMsgQueue entry with key {:x}", key);
            let lt = u64::construct_from(&mut slice)?;
            let enq = MsgEnqueueStuff::construct_from(&mut slice, lt)?;
            if !old_shard.contains_full_prefix(enq.cur_prefix()) {
                fail!(
                    "OutMsgQueue message with key {:x} does not contain current \
                    address belonging to shard {}",
                    enq.out_msg_key(),
                    old_shard
                )
            }
            match new_shard.contains_full_prefix(enq.cur_prefix()) {
                true => Ok(HashmapFilterResult::Accept),
                false => Ok(HashmapFilterResult::Remove),
            }
        })?;
        Ok(())
    }

    pub fn end_lt(&self) -> u64 {
        self.end_lt
    }
    pub fn can_check_processed(&self) -> bool {
        for entry in &self.entries {
            if !entry.can_check_processed() {
                return false;
            }
        }
        true
    }
    pub fn add_processed_upto(
        &mut self,
        seqno: u32,
        #[cfg(feature = "xp25")] real_mc_seqno: u32,
        last_msg_lt: u64,
        last_msg_hash: UInt256,
    ) -> Result<()> {
        let entry = ProcessedUptoStuff {
            shard: self.shard().shard_prefix_with_tag(),
            seqno,
            last_msg_lt,
            last_msg_hash,
            #[cfg(feature = "xp25")]
            real_mc_seqno,
            #[cfg(feature = "xp25")]
            original_shard: None,
            mc_end_lt: 0,
            ref_shards: None,
        };
        self.entries.push(entry);
        self.compactify()?;
        Ok(())
    }
    pub fn entries(&self) -> &Vec<ProcessedUptoStuff> {
        &self.entries
    }
    pub fn min_seqno(&self) -> u32 {
        self.min_seqno
    }
    pub fn already_processed(&self, enq: &MsgEnqueueStuff) -> Result<bool> {
        if self.shard().contains_full_prefix(enq.next_prefix()) {
            for entry in &self.entries {
                if entry.already_processed(enq)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn compactify(&mut self) -> Result<bool> {
        Self::compactify_entries(&mut self.entries)
    }
    fn compactify_entries(entries: &mut Vec<ProcessedUptoStuff>) -> Result<bool> {
        let n = entries.len();
        let mut mark = Vec::new();
        mark.resize(n, false);
        let mut found = false;
        for i in 0..n {
            for j in 0..n {
                if i != j && !mark[j] && entries[j].contains(&entries[i]) {
                    mark[i] = true;
                    found = true;
                    break;
                }
            }
        }
        if found {
            for i in (0..n).rev() {
                if mark[i] {
                    entries.remove(i);
                }
            }
        }
        Ok(found)
    }

    pub fn is_reduced(&self) -> bool {
        Self::is_reduced_entries(&self.entries)
    }
    fn is_reduced_entries(entries: &[ProcessedUptoStuff]) -> bool {
        let n = entries.len();
        for i in 1..n {
            for j in 0..i {
                if entries[i].contains(&entries[j]) || entries[j].contains(&entries[i]) {
                    return false;
                }
            }
        }
        true
    }
    pub fn contains(&self, other: &Self) -> bool {
        for entry in &other.entries {
            if !self.contains_value(entry) {
                return false;
            }
        }
        true
    }
    pub fn contains_value(&self, value: &ProcessedUptoStuff) -> bool {
        for entry in &self.entries {
            if entry.contains(value) {
                return true;
            }
        }
        false
    }
    pub fn is_simple_update_of(&self, other: &Self) -> (bool, Option<ProcessedUptoStuff>) {
        if !self.contains(other) {
            log::debug!("Does not cointain the previous value");
            return (false, None);
        }

        if other.contains(self) {
            log::debug!("Coincides with the previous value");
            return (true, None);
        }

        let mut found = None;
        for entry in &self.entries {
            if !other.contains_value(entry) {
                if found.is_some() {
                    log::debug!("Has more than two new entries");
                    return (false, found); // ok = false: update is not simple
                }
                found = Some(entry.clone());
            }
        }
        (true, found)
    }
}

pub struct MsgQueueManager {
    prev_out_queue_info: OutMsgQueueInfoStuff,
    next_out_queue_info: OutMsgQueueInfoStuff,
    neighbors: Vec<OutMsgQueueInfoStuff>,
    block_descr: Arc<String>,
    states_manager: StatesManager,
}

impl MsgQueueManager {
    #[allow(clippy::too_many_arguments)]
    pub async fn init(
        engine: &Arc<dyn EngineOperations>,
        last_mc_state: &Arc<ShardStateStuff>,
        shard: ShardIdent,
        new_seq_no: u32,
        shards: &ShardHashes,
        prev_states: &[Arc<ShardStateStuff>],
        next_state_opt: Option<&Arc<ShardStateStuff>>,
        after_merge: bool,
        after_split: bool,
        stop_flag: Option<&AtomicBool>,
        usage_tree: Option<&UsageTree>,
        imported_visited: Option<&mut HashSet<UInt256>>,
        block_descr: Option<Arc<String>>,
        mut states_manager: StatesManager,
    ) -> Result<Self> {
        let block_descr = block_descr.unwrap_or_else(|| Arc::new(String::default()));

        states_manager.insert(last_mc_state).await?;

        for prev_state in prev_states {
            log::trace!("Cached prev state {}", prev_state.block_id());
            states_manager.insert(prev_state).await?;
        }

        // Cache the next state if exists and precompute the next `mc_end_lt`
        // to reduce the compute of state queries in `fix_processed_upto`
        let mut next_mc_end_lt = 0;
        if let Some(state) = next_state_opt {
            states_manager.insert(state).await?;
            if state.shard().is_masterchain() {
                next_mc_end_lt = state.gen_lt()?;
            }
        }

        log::debug!("{}: request a preliminary list of neighbors for {}", block_descr, shard);
        let shards = ShardHashesStuff::from(shards.clone());
        let neighbor_list = shards.neighbours_for(&shard)?;
        let mut neighbors = vec![];
        log::debug!(
            "{}: got a preliminary list of {} neighbors for {}",
            block_descr,
            neighbor_list.len(),
            shard
        );
        for (i, nb_shard_record) in neighbor_list.iter().enumerate() {
            let nb_block_id = nb_shard_record.block_id();

            log::debug!("{}: neighbors #{} ---> {:#}", block_descr, i + 1, nb_block_id.shard());

            let shard_state = states_manager.get_state(nb_block_id, Some(1000)).await?;

            let nb = Self::load_out_queue_info(
                &shard_state,
                last_mc_state,
                &mut states_manager,
                block_descr.clone(),
            )
            .await?;
            neighbors.push(nb);

            check_stop_flag(&stop_flag)?;
        }

        // TODO: `shards.is_empty()` might not be needed
        if shards.is_empty() || last_mc_state.block_id().seq_no() != 0 {
            let nb = Self::load_out_queue_info(
                last_mc_state,
                last_mc_state,
                &mut states_manager,
                block_descr.clone(),
            )
            .await?;
            neighbors.push(nb);
        }

        let mut prev_out_queue_info = Self::load_out_queue_info(
            &prev_states[0],
            last_mc_state,
            &mut states_manager,
            block_descr.clone(),
        )
        .await?;

        if prev_states[0].block_id().seq_no != 0 {
            if let Some(merge_state) = prev_states.get(1) {
                CHECK!(after_merge);
                log::debug!(
                    "{}: prepare merge for states {} and {}",
                    block_descr,
                    prev_states[0].block_id(),
                    merge_state.block_id()
                );
                let merge_out_queue_info = Self::load_out_queue_info(
                    merge_state,
                    last_mc_state,
                    &mut states_manager,
                    block_descr.clone(),
                )
                .await?;
                prev_out_queue_info.merge(&merge_out_queue_info)?;
                Self::add_trivial_neighbor_after_merge(
                    &mut neighbors,
                    &shard,
                    &prev_out_queue_info,
                    prev_states,
                    &stop_flag,
                    block_descr.clone(),
                )?;
            } else {
                let sibling_out_queue_info = if after_split {
                    log::debug!(
                        "{}: prepare split for state {}",
                        block_descr,
                        prev_states[0].block_id()
                    );
                    Some(prev_out_queue_info.split(
                        shard.clone(),
                        engine,
                        usage_tree,
                        imported_visited,
                    )?)
                } else {
                    None
                };
                Self::add_trivial_neighbor(
                    &mut neighbors,
                    &shard,
                    &prev_out_queue_info,
                    sibling_out_queue_info,
                    prev_states[0].shard(),
                    &stop_flag,
                    block_descr.clone(),
                )?;
            }
        }
        let mut next_out_queue_info = match next_state_opt {
            Some(next_state) => {
                Self::load_out_queue_info(
                    next_state,
                    last_mc_state,
                    &mut states_manager,
                    block_descr.clone(),
                )
                .await?
            }
            None => prev_out_queue_info.clone(),
        };

        // `ProcessedUptoStuff` seqno is a masterchain seqno for the old implementation
        #[cfg(not(feature = "xp25"))]
        let seqno = {
            _ = new_seq_no; // unused
            last_mc_state.block_id().seq_no()
        };

        // `ProcessedUptoStuff` seqno is an exact shard seqno for the new implementation
        #[cfg(feature = "xp25")]
        let seqno = new_seq_no;

        prev_out_queue_info.fix_processed_upto(seqno, 0, None, &states_manager, &stop_flag)?;
        next_out_queue_info.fix_processed_upto(
            seqno,
            next_mc_end_lt,
            Some(shards.as_ref()),
            &states_manager,
            &stop_flag,
        )?;

        for neighbor in &mut neighbors {
            neighbor.fix_processed_upto(seqno, 0, None, &states_manager, &stop_flag)?;
        }

        Ok(MsgQueueManager {
            prev_out_queue_info,
            next_out_queue_info,
            neighbors,
            block_descr,
            states_manager,
        })
    }

    pub async fn load_out_queue_info(
        state: &Arc<ShardStateStuff>,
        last_mc_state: &Arc<ShardStateStuff>,
        states_manager: &mut StatesManager,
        block_descr: Arc<String>,
    ) -> Result<OutMsgQueueInfoStuff> {
        log::debug!(
            "{}: unpacking OutMsgQueueInfo of neighbor {:#}",
            block_descr,
            state.block_id()
        );
        let nb = OutMsgQueueInfoStuff::from_shard_state(state, states_manager).await?;
        // if (verbosity >= 2) {
        //     block::gen::t_ProcessedInfo.print(std::cerr, qinfo.proc_info);
        //     qinfo.proc_info->print_rec(std::cerr);
        // }
        // require masterchain blocks referred to in ProcessedUpto
        // TODO: perform this only if there are messages for this shard in our output queue
        // .. (have to check the above condition and perform a `break` here) ..
        // ..
        #[cfg(not(feature = "xp25"))]
        for entry in nb.entries() {
            // TODO add loop and stop_flag checking
            states_manager.request_mc_state(last_mc_state, entry.seqno, Some(10_000)).await?;
        }

        // Request exact shard states of neighbours for the new implementation
        #[cfg(feature = "xp25")]
        for entry in nb.entries() {
            if state.shard().is_masterchain() {
                states_manager.request_mc_state(last_mc_state, entry.seqno, Some(10_000)).await?;
            } else {
                let shard = ShardIdent::with_tagged_prefix(
                    state.block_id().shard().workchain_id(),
                    entry.exact_shard(),
                )?;
                states_manager.request_shard_state(shard, entry.seqno, Some(10_000)).await?;
            }
        }

        Ok(nb)
    }

    fn already_processed(&self, enq: &MsgEnqueueStuff) -> Result<(bool, u64)> {
        for neighbor in &self.neighbors {
            if !neighbor.is_disabled() && neighbor.already_processed(enq)? {
                return Ok((true, neighbor.end_lt()));
            }
        }
        Ok((false, 0))
    }

    fn get_max_processed_lt_from_queue_info(&self) -> u64 {
        let current_shard = self.prev_out_queue_info.shard();
        let mut max_lt = 0;
        for neighbor in &self.neighbors {
            if neighbor.shard().workchain_id() != -1 || neighbor.shard() == current_shard {
                for entry in &neighbor.entries {
                    if entry.last_msg_lt > max_lt {
                        max_lt = entry.last_msg_lt;
                    }
                }
            }
            log::trace!(
                "{}: get_max_processed_lt: (current shard {} != neighbor shard {}) = {}, max_lt {}",
                self.block_descr,
                current_shard,
                neighbor.shard(),
                neighbor.shard() != current_shard,
                max_lt,
            );
        }
        max_lt
    }

    fn add_trivial_neighbor_after_merge(
        neighbors: &mut [OutMsgQueueInfoStuff],
        shard: &ShardIdent,
        real_out_queue_info: &OutMsgQueueInfoStuff,
        prev_states: &[Arc<ShardStateStuff>],
        stop_flag: &Option<&AtomicBool>,
        block_descr: Arc<String>,
    ) -> Result<()> {
        log::debug!("{}: in add_trivial_neighbor_after_merge()", block_descr);
        CHECK!(prev_states.len(), 2);
        let mut found = 0;
        for (i, nb) in neighbors.iter_mut().enumerate() {
            if shard.intersect_with(nb.shard()) {
                found += 1;
                log::debug!(
                    "{}: neighbor #{} : {} intersects our shard {}",
                    block_descr,
                    i,
                    nb.block_id(),
                    shard
                );
                if !shard.is_parent_for(nb.shard()) || found > 2 {
                    fail!("impossible shard configuration in add_trivial_neighbor_after_merge()")
                }
                let prev_shard = prev_states[found - 1].shard();
                if nb.shard() != prev_shard {
                    fail!(
                        "neighbor shard {} does not match that of our ancestor {}",
                        nb.shard(),
                        prev_shard
                    )
                }
                if found == 1 {
                    *nb = real_out_queue_info.clone();
                    log::debug!(
                        "{}: adjusted neighbor #{} : {} with shard expansion \
                        (immediate after-merge adjustment)",
                        block_descr,
                        i,
                        nb.block_id()
                    );
                } else {
                    nb.disable();
                    log::debug!(
                        "{}: disabling neighbor #{} : {} \
                        (immediate after-merge adjustment)",
                        block_descr,
                        i,
                        nb.block_id()
                    );
                }

                check_stop_flag(stop_flag)?;
            }
        }
        CHECK!(found == 2);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_trivial_neighbor(
        neighbors: &mut Vec<OutMsgQueueInfoStuff>,
        shard: &ShardIdent,
        real_out_queue_info: &OutMsgQueueInfoStuff,
        mut sibling_out_queue_info: Option<OutMsgQueueInfoStuff>,
        prev_shard: &ShardIdent,
        stop_flag: &Option<&AtomicBool>,
        block_descr: Arc<String>,
    ) -> Result<()> {
        log::debug!("{}: in add_trivial_neighbor()", block_descr);
        // Possible cases are:
        // 1. prev_shard = shard = one of neighbors
        //    => replace neighbor by (more recent) prev_shard info
        // 2. shard is child of prev_shard = one of neighbors
        //    => after_split must be set;
        //       replace neighbor by new split data (and shrink its shard);
        //       insert new virtual neighbor (our future sibling).
        // 3. prev_shard = shard = child of one of neighbors
        //    => after_split must be clear (we are continuing an after-split chain);
        //       make our virtual sibling from the neighbor (split its queue);
        //       insert ourselves from prev_shard data
        // In all of the above cases, our shard intersects exactly one neighbor, which has the same shard or its parent.
        // 4. there are two neighbors intersecting shard = prev_shard, which are its children.
        // 5. there are two prev_shards, the two children of shard, and two neighbors coinciding with prev_shards
        let mut found = 0;
        let mut cs = 0;
        let n = neighbors.len();
        for i in 0..n {
            let nb = &mut neighbors[i];
            if !shard.intersect_with(nb.shard()) {
                continue;
            }
            found += 1;
            log::debug!(
                "{}: neighbor #{} : {} intersects our shard {}",
                block_descr,
                i,
                nb.block_id(),
                shard
            );
            if nb.shard() == prev_shard {
                if prev_shard == shard {
                    // case 1. Normal.
                    CHECK!(found == 1);
                    *nb = real_out_queue_info.clone();
                    log::debug!(
                        "{}: adjusted neighbor #{} : {} (simple replacement)",
                        block_descr,
                        i,
                        nb.block_id()
                    );
                    cs = 1;
                } else if nb.shard().is_parent_for(shard) {
                    // case 2. Immediate after-split.
                    CHECK!(found == 1);
                    CHECK!(sibling_out_queue_info.is_some());
                    if let Some(sibling) = sibling_out_queue_info.take() {
                        *nb = sibling;
                    }
                    log::debug!(
                        "{}: adjusted neighbor #{} : {} with shard \
                        shrinking to our sibling (immediate after-split adjustment)",
                        block_descr,
                        i,
                        nb.block_id()
                    );

                    let nb = real_out_queue_info.clone();
                    log::debug!(
                        "{}: created neighbor #{} : {} with shard \
                        shrinking to our (immediate after-split adjustment)",
                        block_descr,
                        n,
                        nb.block_id()
                    );
                    neighbors.push(nb);
                    cs = 2;
                } else {
                    fail!("impossible shard configuration in add_trivial_neighbor()")
                }
            } else if nb.shard().is_parent_for(shard) && shard == prev_shard {
                // case 3. Continued after-split
                CHECK!(found == 1);
                CHECK!(sibling_out_queue_info.is_none());

                // compute the part of virtual sibling's OutMsgQueue with destinations in our shard
                let sib_shard = shard.sibling();
                let shard_prefix = shard.shard_key(true);
                nb.out_queue_mut().into_subtree_with_prefix(&shard_prefix, &mut 0)?;
                nb.filter_messages(&sib_shard).map_err(|err| {
                    error!(
                        "cannot filter virtual sibling's OutMsgQueue \
                        from that of the last common ancestor: {}",
                        err
                    )
                })?;
                nb.set_shard(sib_shard);
                log::debug!(
                    "{}: adjusted neighbor #{} : {} with shard shrinking \
                    to our sibling (continued after-split adjustment)",
                    block_descr,
                    i,
                    nb.block_id()
                );

                let nb = real_out_queue_info.clone();
                log::debug!(
                    "{}: created neighbor #{} : {} from our preceding state \
                    (continued after-split adjustment)",
                    block_descr,
                    n,
                    nb.block_id()
                );
                neighbors.push(nb);
                cs = 3;
            } else if shard.is_parent_for(nb.shard()) && shard == prev_shard {
                // case 4. Continued after-merge.
                if found == 1 {
                    cs = 4;
                }
                CHECK!(cs == 4);
                CHECK!(found <= 2);
                if found == 1 {
                    *nb = real_out_queue_info.clone();
                    log::debug!(
                        "{}: adjusted neighbor #{} : {} with shard expansion \
                        (continued after-merge adjustment)",
                        block_descr,
                        i,
                        nb.block_id()
                    );
                } else {
                    nb.disable();
                    log::debug!(
                        "{}: disabling neighbor #{} : {} (continued after-merge adjustment)",
                        block_descr,
                        i,
                        nb.block_id()
                    );
                }
            } else {
                fail!("impossible shard configuration in add_trivial_neighbor()")
            }
            check_stop_flag(stop_flag)?;
        }
        // dbg!(found, cs);
        CHECK!(found != 0 && cs != 0);
        CHECK!(found == (1 + (cs == 4) as usize));
        Ok(())
    }

    pub async fn clean_out_msg_queue(
        &mut self,
        clean_timeout_nanos: i128,
        optimistic_clean_percentage_points: u32,
        mut on_message: impl FnMut(MsgEnqueueStuff, Option<u64>, Option<&Cell>) -> Result<bool>,
    ) -> Result<(bool, i32, i32)> {
        let timer = Instant::now();

        log::debug!(
            "{}: in clean_out_msg_queue: cleaning output messages imported by neighbors",
            self.block_descr
        );
        if self.next_out_queue_info.out_queue().is_empty() {
            return Ok((false, 0, 0));
        }

        for neighbor in self.neighbors.iter() {
            if !neighbor.is_disabled() && !neighbor.can_check_processed() {
                fail!(
                    "Internal error: no info for checking processed messages from neighbor {}",
                    neighbor.block_id()
                )
            }
        }
        let total = self.next_out_queue_info.out_queue_extra().out_queue_size();
        let mut block_full = false;
        let mut partial = false;
        let mut queue = self.next_out_queue_info.out_queue().clone();
        let mut deleted = 0;
        let mut skipped = 0;
        let root = self.next_out_queue_info.out_queue().data();

        let ordered_cleaning_timeout_nanos =
            clean_timeout_nanos * (optimistic_clean_percentage_points as i128) / 1000;
        let random_cleaning_timeout_nanos = clean_timeout_nanos - ordered_cleaning_timeout_nanos;

        log::debug!(
            "{}: clean_out_msg_queue total messages: {}, clean_timeout = {} nanos, ordered_cleaning_timeout = {} nanos, random_cleaning_timeout = {} nanos",
            self.block_descr,
            total,
            clean_timeout_nanos,
            ordered_cleaning_timeout_nanos,
            random_cleaning_timeout_nanos,
        );

        if ordered_cleaning_timeout_nanos > 0 {
            let max_processed_lt = self.get_max_processed_lt_from_queue_info();

            let mut clean_timeout_check = 50_000_000;
            let max_clean_timeout_check = 550_000_000;

            partial = out_msg_queue_cleaner::hashmap_filter_ordered_by_lt_hash(
                &mut queue,
                max_processed_lt,
                ordered_cleaning_timeout_nanos,
                |node_obj| {
                    if block_full {
                        log::debug!("{}: BLOCK FULL when ordered cleaning output queue, cleanup is partial", self.block_descr);
                        partial = true;
                        return Ok(HashmapFilterResult::Stop);
                    }

                    let elapsed_nanos = timer.elapsed().as_nanos() as i128;
                    if clean_timeout_check <= max_clean_timeout_check && elapsed_nanos >= clean_timeout_check {
                        log::trace!(
                            "{}: clean_out_msg_queue: ordered cleaning time elapsed {} nanos: processed = {}, deleted = {}, skipped = {}",
                            self.block_descr, elapsed_nanos, deleted + skipped, deleted, skipped,
                        );
                        clean_timeout_check += 50_000_000;
                    }

                    let lt = node_obj.lt();
                    let mut data_and_refs = node_obj.data_and_refs()?;
                    let enq = MsgEnqueueStuff::construct_from(&mut data_and_refs, lt)?;

                    let (processed, end_lt) = self.already_processed(&enq)?;
                    if processed {
                        block_full = on_message(enq, Some(end_lt), root)?;
                        deleted += 1;
                        return Ok(HashmapFilterResult::Remove);
                    }
                    skipped += 1;
                    Ok(HashmapFilterResult::Accept)
                },
                Some(format!("{}: ", self.block_descr)),
            ).map_err(|e| {
                log::error!(
                    "{}: clean_out_msg_queue: error while ordered cleaning output queue, last state was:{} processed = {}, deleted = {}, skipped = {}: e = {}",
                    self.block_descr,
                    if partial { " partial," } else { "" },
                    deleted + skipped, deleted, skipped,
                    e,
                );
                e
            })?;

            let ordered_clean_elapsed = timer.elapsed().as_nanos() as i128;

            log::debug!(
                "{}: clean_out_msg_queue: cleaning finished (ordered) in {} nanos:{} processed = {}, deleted = {}, skipped = {}",
                self.block_descr,
                ordered_clean_elapsed,
                if partial { " partial," } else { "" },
                deleted + skipped, deleted, skipped,
            );
        } else {
            // not time limit on the ordered clean = queue not cleaned = partial
            partial = true;
        }

        if random_cleaning_timeout_nanos > 0 && partial {
            partial = false;

            let mut random_deleted = 0;
            let mut random_skipped = 0;

            let random_clean_timer = Instant::now();

            let mut clean_timeout_check = 50_000_000;
            let max_clean_timeout_check = 550_000_000;

            queue.hashmap_filter(|_key, mut slice| {
                if block_full {
                    log::debug!("{}: BLOCK FULL when random cleaning output queue, cleanup is partial", self.block_descr);
                    partial = true;
                    return Ok(HashmapFilterResult::Stop)
                }

                let elapsed_nanos = random_clean_timer.elapsed().as_nanos() as i128;

                if clean_timeout_check <= max_clean_timeout_check && elapsed_nanos >= clean_timeout_check {
                    log::debug!(
                        "{}: clean_out_msg_queue: random cleaning time elapsed {} nanos: processed = {}, deleted = {}, skipped = {}",
                        self.block_descr, elapsed_nanos,
                        random_deleted + random_skipped, random_deleted, random_skipped,
                    );
                    clean_timeout_check += 50_000_000;
                }

                // stop when reached the time limit
                if elapsed_nanos >= random_cleaning_timeout_nanos {
                    log::debug!(
                        "{}: clean_out_msg_queue: stopped random cleaning output queue because of time elapsed {} nanos >= {} nanos limit",
                        self.block_descr, elapsed_nanos, random_cleaning_timeout_nanos,
                    );
                    partial = true;
                    return Ok(HashmapFilterResult::Stop)
                }

                let lt = u64::construct_from(&mut slice)?;
                let enq = MsgEnqueueStuff::construct_from(&mut slice, lt)?;
                let (processed, end_lt) = self.already_processed(&enq)?;
                if processed {
                    block_full = on_message(enq, Some(end_lt), root)?;
                    random_deleted += 1;
                    return Ok(HashmapFilterResult::Remove)
                }
                random_skipped += 1;
                Ok(HashmapFilterResult::Accept)
            }).map_err(|e| {
                log::error!(
                    "{}: clean_out_msg_queue: error while random cleaning output queue, last state was:{} processed = {}, deleted = {}, skipped = {}: e = {}",
                    self.block_descr,
                    if partial { " partial," } else { "" },
                    random_deleted + random_skipped, random_deleted, random_skipped,
                    e,
                );
                e
            })?;

            let random_clean_elapsed = random_clean_timer.elapsed().as_nanos() as i128;

            log::debug!(
                "{}: clean_out_msg_queue: cleaning finished (random) in {} nanos:{} processed = {}, deleted = {}, skipped = {}",
                self.block_descr,
                random_clean_elapsed,
                if partial { " partial," } else { "" },
                random_deleted + random_skipped, random_deleted, random_skipped,
            );

            deleted += random_deleted;
            skipped += random_skipped;
        }

        let total_clean_elapsed_nanos = timer.elapsed().as_nanos();

        log::debug!(
            "{}: clean_out_msg_queue: cleaning finished (total) in {} nanos:{} processed = {}, deleted = {}, skipped = {}",
            self.block_descr,
            total_clean_elapsed_nanos,
            if partial { " partial," } else { "" },
            deleted + skipped, deleted, skipped,
        );

        self.next_out_queue_info.out_queue = queue;

        Ok((partial, deleted + skipped, deleted))
    }
}

pub struct StatesManager {
    engine: Arc<dyn EngineOperations>,
    pipeline_context: PipelineContext,
    states: HashMap<ShardIdent, BTreeMap<u32, Arc<ShardStateStuff>>>,
    state_usages: HashMap<BlockIdExt, UsageTree>,
    block_proofs: HashMap<BlockIdExt, Cell>,
    collect_proofs: bool,
    preloaded_states_only: bool,
}

impl StatesManager {
    pub fn with_collator_data(
        engine: Arc<dyn EngineOperations>,
        pipeline_context: PipelineContext,
        collect_proofs: bool,
    ) -> Result<Self> {
        let mut cs = Self {
            engine,
            pipeline_context: PipelineContext::default(),
            states: Default::default(),
            state_usages: Default::default(),
            block_proofs: Default::default(),
            collect_proofs,
            preloaded_states_only: false,
        };
        for (state, block) in pipeline_context.states_with_blocks() {
            let state = if collect_proofs && !state.shard().is_masterchain() {
                if state.seq_no() > 0 {
                    cs.block_proofs
                        .insert(state.block_id().clone(), Self::create_block_state_proof(block)?);
                }
                // TODO: include only used states into usages
                cs.add_state_usage(state)?
            } else {
                state.clone()
            };
            cs.states
                .entry(state.shard().clone())
                .or_default()
                .insert(state.seq_no(), state.clone());
        }
        cs.pipeline_context = pipeline_context;
        Ok(cs)
    }

    pub fn with_validator_data(engine: Arc<dyn EngineOperations>) -> Self {
        Self {
            engine,
            pipeline_context: PipelineContext::default(),
            states: Default::default(),
            state_usages: Default::default(),
            block_proofs: Default::default(),
            collect_proofs: false,
            preloaded_states_only: false,
        }
    }

    pub fn with_validator_data_nosync(
        engine: Arc<dyn EngineOperations>,
        preloaded_states: HashMap<UInt256, ShardStateUnsplit>,
    ) -> Result<Self> {
        let mut states: HashMap<ShardIdent, BTreeMap<u32, Arc<ShardStateStuff>>> = HashMap::new();
        for (block_hash, state) in preloaded_states {
            let shard_ident = state.shard().clone();
            let seq_no = state.seq_no();
            if states
                .entry(shard_ident.clone())
                .or_default()
                .insert(
                    seq_no,
                    ShardStateStuff::from_state(
                        BlockIdExt::with_params(
                            shard_ident.clone(),
                            state.seq_no(),
                            block_hash,
                            UInt256::default(),
                        ),
                        state,
                        #[cfg(feature = "telemetry")]
                        engine.engine_telemetry(),
                        engine.engine_allocated(),
                    )?,
                )
                .is_some()
            {
                fail!("duplicate preloaded state for shard {} seqno {}", shard_ident, seq_no);
            }
        }
        Ok(Self {
            engine,
            pipeline_context: PipelineContext::default(),
            states,
            state_usages: Default::default(),
            block_proofs: Default::default(),
            collect_proofs: false,
            preloaded_states_only: true,
        })
    }

    pub async fn get_state(
        &mut self,
        id: &BlockIdExt,
        timeout_ms: Option<u64>,
    ) -> Result<Arc<ShardStateStuff>> {
        if let Some(state) = self.try_get_state_with_id(id) {
            return Ok(state);
        } else if self.preloaded_states_only {
            fail!("State {id} not found in preloaded states");
        }

        let state = self.engine.clone().wait_state(id, timeout_ms, true).await?;
        self.insert(&state).await
    }

    pub fn try_get_state_with_id(&self, id: &BlockIdExt) -> Option<Arc<ShardStateStuff>> {
        self.try_get_state(&id.shard(), id.seq_no)
            .filter(|s| s.block_id().root_hash == id.root_hash)
    }

    fn try_get_state(&self, shard: &ShardIdent, seq_no: u32) -> Option<Arc<ShardStateStuff>> {
        if let Some(states) = self.states.get(shard) {
            if let Some(state_stuff) = states.get(&seq_no) {
                return Some(state_stuff.clone());
            }
        }
        None
    }

    pub fn get_entry_data(&self, shard: &ShardIdent, seq_no: u32) -> Result<(u64, ShardHashes)> {
        let state_stuff = self.try_get_state(shard, seq_no).ok_or_else(|| {
            error!("state for block {}:{} was not previously cached", shard, seq_no)
        })?;
        let state = state_stuff.state()?;

        let mc_end_lt = match state.master_ref() {
            None => state.gen_lt(),
            Some(master_ref) => master_ref.master.end_lt,
        };

        #[cfg(not(feature = "xp25"))]
        let shard_hashes = state_stuff.shards()?.clone();

        #[cfg(feature = "xp25")]
        let shard_hashes = match state_stuff.shard_hashes_raw_opt() {
            Some(shard_hashes) => shard_hashes.clone(),
            // TODO: replace shard hashes with something more optimal
            None => {
                let rsb = state
                    .read_wc_custom()?
                    .ok_or_else(|| error!("Cannot read wc custom"))?
                    .ref_shard_blocks;
                crate::validating_utils::extend_ref_shard_blocks(&rsb)?
            }
        };

        Ok((mc_end_lt, shard_hashes))
    }

    fn add_state_usage(&mut self, state: &Arc<ShardStateStuff>) -> Result<Arc<ShardStateStuff>> {
        let usage_tree = UsageTree::with_params(state.root_cell().clone(), true);
        let root = usage_tree.root_cell();
        self.state_usages.insert(state.block_id().clone(), usage_tree);
        ShardStateStuff::from_root_cell(
            state.block_id().clone(),
            root,
            #[cfg(feature = "telemetry")]
            self.engine.engine_telemetry(),
            self.engine.engine_allocated(),
        )
    }

    fn create_block_state_proof(block: &Block) -> Result<Cell> {
        let usage_tree = UsageTree::with_params(block.serialize()?, true);
        let block = Block::construct_from_cell(usage_tree.root_cell())?;
        block.read_state_update()?;
        MerkleProof::create_by_usage_tree(&usage_tree.original_root(), &usage_tree)?.serialize()
    }

    async fn load_and_create_block_state_proof(
        engine: &Arc<dyn EngineOperations>,
        block_id: &BlockIdExt,
    ) -> Result<Cell> {
        let block_handle = engine
            .load_block_handle(block_id)?
            .ok_or_else(|| error!("cannot load block handle for neighbor {}", block_id))?;
        let block_stuff = engine.load_block(&block_handle).await?;
        let usage_tree = UsageTree::with_params(block_stuff.root_cell().clone(), true);
        let block = Block::construct_from_cell(usage_tree.root_cell())?;
        block.read_state_update()?;
        let proof = MerkleProof::create_by_usage_tree(&usage_tree.original_root(), &usage_tree)?
            .serialize()?;
        Ok(proof)
    }

    pub async fn insert(&mut self, state: &Arc<ShardStateStuff>) -> Result<Arc<ShardStateStuff>> {
        let state = if self.collect_proofs && !state.shard().is_masterchain() {
            if state.seq_no() > 0 && !self.block_proofs.contains_key(state.block_id()) {
                self.block_proofs.insert(
                    state.block_id().clone(),
                    Self::load_and_create_block_state_proof(&self.engine, state.block_id()).await?,
                );
            }
            self.add_state_usage(state)?
        } else {
            state.clone()
        };
        self.states.entry(state.shard().clone()).or_default().insert(state.seq_no(), state.clone());
        Ok(state)
    }

    pub async fn request_mc_state(
        &mut self,
        last_mc_state: &Arc<ShardStateStuff>,
        seq_no: u32,
        timeout_ms: Option<u64>,
    ) -> Result<()> {
        let states = self.states.entry(ShardIdent::masterchain()).or_default();
        if let btree_map::Entry::Vacant(entry) = states.entry(seq_no) {
            let last_mc_seqno = last_mc_state.state()?.seq_no();
            if seq_no >= last_mc_seqno {
                fail!("Requested too new master chain state {}, last is {}", seq_no, last_mc_seqno);
            }

            let block_id = match last_mc_state.shard_state_extra()?.prev_blocks.get(&seq_no) {
                Ok(Some(result)) => result.master_block_id().1,
                _ => fail!(
                    "cannot find previous masterchain block with seqno {} \
                    to load corresponding state as required",
                    seq_no
                ),
            };

            let state = self.engine.clone().wait_state(&block_id, timeout_ms, true).await?;
            entry.insert(state);
        }
        Ok(())
    }

    #[cfg(feature = "xp25")]
    pub async fn request_shard_state(
        &mut self,
        shard: ShardIdent,
        seq_no: u32,
        timeout_ms: Option<u64>,
    ) -> Result<Arc<ShardStateStuff>> {
        if self.preloaded_states_only {
            return self
                .states
                .get(&shard)
                .map(|states| states.get(&seq_no).cloned())
                .flatten()
                .ok_or_else(|| error!("Preloaded state {shard}:{seq_no} not found"));
        }

        let has_requested_shard = match self.states.get(&shard) {
            Some(shard) => {
                if let Some(shard_state) = shard.get(&seq_no) {
                    return Ok(shard_state.clone());
                }
                true
            }
            None => false,
        };

        let mut closest_state = None;

        // Fast path if the specified path already exists
        if has_requested_shard {
            if let Some(states) = &self.states.get(&shard) {
                if let Some((_, state)) = states.range(seq_no..).next() {
                    closest_state = Some(state.clone());
                }
            }
        }

        // Slow path is there were splits or merges for the specified shard
        if closest_state.is_none() {
            for (known_shard, states) in &self.states {
                // Ignore shards where we will not find some references
                if !shard.intersect_with(known_shard) {
                    continue;
                }

                // Find the closest state in some shard
                let Some((_, state)) = states.range(seq_no..).next() else {
                    continue;
                };

                // Update the closest state if the found one is closer
                if !matches!(&closest_state, Some(closest_state) if closest_state.seq_no() <= state.seq_no())
                {
                    closest_state = Some(state.clone());
                }
            }
        }

        if log::log_enabled!(log::Level::Trace) {
            let mut cached_list = string_builder::Builder::default();
            for (shard, states) in &self.states {
                for seqno in states.keys() {
                    cached_list.append(format!("\n{shard}:{seqno}"));
                }
            }
            log::trace!(
                "Cached states for {shard}:{seq_no}: {}",
                cached_list.string().unwrap_or_default()
            );
        }

        // Find full block id
        let is_target_block =
            |block_id: &BlockIdExt| block_id.shard() == &shard && block_id.seq_no == seq_no;
        let is_possible_block = |block_id: &BlockIdExt| {
            block_id.shard().intersect_with(&shard) && block_id.seq_no > seq_no
        };

        let mut closest_block_id = match closest_state {
            Some(state) => state.block_id().clone(),
            None => fail!("Failed to find the closest state for {}:{}", shard, seq_no),
        };
        log::trace!("Closest block id for {shard}:{seq_no} is {closest_block_id}");

        // TODO: simplify
        let block_id = loop {
            let (prev1, prev2) =
                if let Some(ids) = self.pipeline_context.get_prev_for(&closest_block_id) {
                    (
                        ids.get(0).cloned().ok_or_else(|| error!("INTERNAL ERROR: no prev1"))?,
                        ids.get(1).cloned(),
                    )
                } else {
                    (self.engine.load_block_prev1(&closest_block_id)?, None)
                };

            log::trace!("Found prev1 for {shard}:{seq_no} = {prev1}");

            // Fast check if the target block was found
            if is_target_block(&prev1) {
                break Some(prev1);
            }

            // Check if left shard is ok
            let suits_prev1 = is_possible_block(&prev1);

            if closest_block_id.shard().is_parent_for(prev1.shard()) {
                if let Some(prev2) = prev2.or(self.engine.load_block_prev2(&closest_block_id)?) {
                    log::trace!("Found prev2 for {shard}:{seq_no} = {prev2}");

                    // Fast check if the target block was found
                    if is_target_block(&prev2) {
                        break Some(prev2);
                    }

                    // Check if right shard is ok
                    let suits_prev2 = is_possible_block(&prev2);
                    if suits_prev1 && suits_prev2 {
                        // Choose the closest if both shards are ok
                        closest_block_id = if prev1.seq_no < prev2.seq_no { prev1 } else { prev2 };
                        continue;
                    } else if suits_prev2 {
                        // Choose right shard if it is the only suitable one
                        closest_block_id = prev2;
                        continue;
                    }
                }
            }

            if suits_prev1 {
                // Choose left shard if it is the only suitable one
                closest_block_id = prev1;
            } else {
                // No suitable shards found
                break None;
            }
        }
        .ok_or_else(|| error!("Failed to find the full shard block id for {}:{}", shard, seq_no))?;

        self.get_state(&block_id, timeout_ms).await
    }

    pub fn neighbor_usages(&self) -> &HashMap<BlockIdExt, UsageTree> {
        &self.state_usages
    }
    pub fn block_proofs(&self) -> &HashMap<BlockIdExt, Cell> {
        &self.block_proofs
    }
}

impl MsgQueueManager {
    /// create iterator for merging all output messages from all neighbors to our shard
    pub fn merge_out_queue_iter(
        &self,
        shard: &ShardIdent,
    ) -> Result<MsgQueueMergerIterator<BlockIdExt>> {
        MsgQueueMergerIterator::from_manager(self, shard)
    }
    /// find enquque message and return it with neighbor id
    pub fn find_message(
        &self,
        key: &OutMsgQueueKey,
        prefix: &AccountIdPrefixFull,
    ) -> Result<(Option<BlockIdExt>, Option<MsgEnqueueStuff>)> {
        for nb in &self.neighbors {
            if !nb.is_disabled() && nb.shard().contains_full_prefix(prefix) {
                return Ok((Some(nb.block_id().clone()), nb.message(key)?));
            }
        }
        Ok((None, None))
    }
    pub fn prev(&self) -> &OutMsgQueueInfoStuff {
        &self.prev_out_queue_info
    }
    pub fn next(&self) -> &OutMsgQueueInfoStuff {
        &self.next_out_queue_info
    }
    pub fn next_mut(&mut self) -> &mut OutMsgQueueInfoStuff {
        &mut self.next_out_queue_info
    }
    pub fn take_next(&mut self) -> OutMsgQueueInfoStuff {
        mem::take(&mut self.next_out_queue_info)
    }
    // Unused
    //    pub fn shard(&self) -> &ShardIdent { &self.shard }
    pub fn neighbors(&self) -> &Vec<OutMsgQueueInfoStuff> {
        &self.neighbors
    }
    pub fn neighbor_usages(&self) -> &HashMap<BlockIdExt, UsageTree> {
        self.states_manager.neighbor_usages()
    }
    pub fn block_proofs(&self) -> &HashMap<BlockIdExt, Cell> {
        self.states_manager.block_proofs()
    }
    // Unused
    //    pub fn neighbor(&self, shard: &ShardIdent) -> Option<&OutMsgQueueInfoStuff> {
    //        for nb in &self.neighbors {
    //            if nb.shard() == shard {
    //                return Some(nb)
    //            }
    //        }
    //        None
    //    }
}

#[derive(Eq, PartialEq)]
struct RootRecord<T> {
    lt: u64,
    cursor: SliceData,
    bit_len: usize,
    key: BuilderData,
    id: T,
}

impl<T: Eq> RootRecord<T> {
    fn new(lt: u64, cursor: SliceData, bit_len: usize, key: BuilderData, id: T) -> Self {
        Self { lt, cursor, bit_len, key, id }
    }
    fn from_cell(cell: &Cell, mut bit_len: usize, id: T) -> Result<Self> {
        let mut cursor = SliceData::load_cell_ref(cell)?;
        let key = LabelReader::read_label_raw(&mut cursor, &mut bit_len, BuilderData::default())?;
        let lt = cursor.get_next_u64()?;
        Ok(Self { lt, cursor, bit_len, key, id })
    }
}

impl<T: Eq> Ord for RootRecord<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // first compare lt descending, because Vec is a stack
        let mut cmp = self.lt.cmp(&other.lt);
        if cmp == std::cmp::Ordering::Equal {
            // check if we have full key and leaf
            cmp = self.key.length_in_bits().cmp(&other.key.length_in_bits());
            // compare hashes descending, because Vec is a stack
            if cmp == std::cmp::Ordering::Equal && self.key.length_in_bits() == 352 {
                cmp = self.key.data()[12..44].cmp(&other.key.data()[12..44]);
            }
        }
        cmp.reverse()
    }
}
impl<T: Eq> PartialOrd for RootRecord<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// it iterates messages ascending create_lt and hash
pub struct MsgQueueMergerIterator<T> {
    // store branches descending by lt and hash because Vec works like Stack
    roots: Vec<RootRecord<T>>,
}

impl MsgQueueMergerIterator<BlockIdExt> {
    pub fn from_manager(manager: &MsgQueueManager, shard: &ShardIdent) -> Result<Self> {
        let shard_prefix = shard.shard_key(true);
        let mut roots = vec![];
        for nb in manager.neighbors.iter().filter(|nb| !nb.is_disabled()) {
            let mut out_queue_short = nb.out_queue().clone();
            out_queue_short.into_subtree_with_prefix(&shard_prefix, &mut 0)?;
            if let Some(cell) = out_queue_short.data() {
                roots.push(RootRecord::from_cell(
                    cell,
                    out_queue_short.bit_len(),
                    nb.block_id().clone(),
                )?);
                // roots.push(RootRecord::new(lt, cursor, bit_len, key, nb.block_id().clone()));
            }
        }
        if !roots.is_empty() {
            roots.sort();
            debug_assert!(roots.first().unwrap().lt >= roots.last().unwrap().lt);
        }
        Ok(Self { roots })
    }
    pub fn from_states(
        states: &[(BlockIdExt, ShardStateUnsplit)],
        shard: &ShardIdent,
    ) -> Result<Self> {
        let shard_prefix = shard.shard_key(true);
        let mut roots = vec![];
        for (id, state) in states {
            let mut out_queue_short = state.read_out_msg_queue_info()?.out_queue().clone();
            out_queue_short.into_subtree_with_prefix(&shard_prefix, &mut 0)?;
            if let Some(cell) = out_queue_short.data() {
                roots.push(RootRecord::from_cell(cell, out_queue_short.bit_len(), id.clone())?);
            }
        }
        if !roots.is_empty() {
            roots.sort();
            debug_assert!(roots.first().unwrap().lt >= roots.last().unwrap().lt);
        }
        Ok(Self { roots })
    }
}

impl MsgQueueMergerIterator<u8> {
    #[cfg(test)]
    pub fn from_queue(out_queue: &OutMsgQueue) -> Result<Self> {
        let mut roots = Vec::new();
        if let Some(cell) = out_queue.data() {
            roots.push(RootRecord::from_cell(cell, out_queue.bit_len(), 0)?);
        }
        Ok(Self { roots })
    }
}

impl<T: Clone + Eq> MsgQueueMergerIterator<T> {
    fn insert(&mut self, root: RootRecord<T>) {
        let idx = self.roots.binary_search(&root).unwrap_or_else(|x| x);
        self.roots.insert(idx, root);
        debug_assert!(self.roots.first().unwrap().lt >= self.roots.last().unwrap().lt);
    }
    fn next_item_raw(&mut self) -> Result<Option<(Cell, SliceData, u64, T)>> {
        while let Some(root) = self.roots.pop() {
            if root.bit_len == 0 {
                let key = root.key.into_cell()?;
                let enq = root.cursor;
                return Ok(Some((key, enq, root.lt, root.id)));
            }
            for idx in 0..2 {
                let mut bit_len = root.bit_len - 1;
                let mut cursor = SliceData::load_cell(root.cursor.reference(idx)?)?;
                let mut key = root.key.clone();
                key.append_bit_bool(idx == 1)?;
                key = LabelReader::read_label_raw(&mut cursor, &mut bit_len, key)?;
                let lt = cursor.get_next_u64()?;
                self.insert(RootRecord::new(lt, cursor, bit_len, key, root.id.clone()));
            }
        }
        Ok(None)
    }
    fn next_item(&mut self) -> Result<Option<(OutMsgQueueKey, MsgEnqueueStuff, T)>> {
        if let Some((key, mut cursor, lt, id)) = self.next_item_raw()? {
            let key = OutMsgQueueKey::construct_from_cell(key)?;
            let enq = MsgEnqueueStuff::construct_from(&mut cursor, lt)?;
            Ok(Some((key, enq, id)))
        } else {
            Ok(None)
        }
    }
}

impl<T: Clone + Eq> Iterator for MsgQueueMergerIterator<T> {
    type Item = Result<(OutMsgQueueKey, MsgEnqueueStuff, T)>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_item().transpose()
    }
}

fn check_stop_flag(stop_flag: &Option<&AtomicBool>) -> Result<()> {
    if let Some(stop_flag) = stop_flag {
        if stop_flag.load(Ordering::Relaxed) {
            fail!("Stop flag was set")
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub async fn build_queue_proof(
    dst_shard: &ShardIdent,
    neighbors: &[BlockIdExt],
    limits: &ImportedMsgQueueLimits,
    engine: &Arc<dyn EngineOperations>,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u32>)> {
    if neighbors.is_empty() {
        fail!("build_queue_proof: no neighbors provided");
    }

    log::debug!("build_queue_proof for shard {} with {} neighbors", dst_shard, neighbors.len());

    // Load blocks and states in parallel
    let (blocks, states) = load_data_for_queue_proof(dst_shard, neighbors, engine).await?;

    // Build state proofs (proof of state's root in block tree)
    let mut state_proofs_roots = Vec::with_capacity(blocks.len());
    for block in &blocks {
        if let Some(block) = block {
            let usage_tree = UsageTree::with_root(block.root_cell().clone());
            let _ = Block::construct_from_cell(usage_tree.root_cell())?.read_state_update()?;
            let proof = MerkleProof::create_by_usage_tree(block.root_cell(), &usage_tree)?;
            state_proofs_roots.push(proof.serialize()?);
        }
    }

    // Build queue proofs
    let (queue_proofs_roots, msgs_count) = build_proofs(
        dst_shard,
        neighbors,
        states.iter().map(|s| s.root_cell().clone()).collect::<Vec<Cell>>().as_slice(),
        limits,
    )?;

    // Build result proofs
    let state_proofs_boc = BocWriter::with_roots(state_proofs_roots)?.write_to_vec()?;
    let queue_proofs_boc = BocWriter::with_roots(queue_proofs_roots)?.write_to_vec()?;

    Ok((state_proofs_boc, queue_proofs_boc, msgs_count))
}

pub fn build_proofs(
    dst_shard: &ShardIdent,
    neighbors: &[BlockIdExt],
    state_roots: &[Cell],
    limits: &ImportedMsgQueueLimits,
) -> Result<(Vec<Cell>, Vec<u32>)> {
    log::trace!("build_queue_proof: dst_shard = {}", dst_shard);

    // 1) build usage tree for each state
    let mut usage_trees = Vec::with_capacity(neighbors.len());
    let mut usage_states = Vec::with_capacity(neighbors.len());
    for (state_root, id) in state_roots.iter().zip(neighbors.iter()) {
        let usage_tree = UsageTree::with_root(state_root.clone());
        let usage_state = ShardStateUnsplit::construct_from_cell(usage_tree.root_cell())?;
        usage_states.push((id.clone(), usage_state));
        usage_trees.push(usage_tree);
    }

    // 2) build iterator for merging queues from all neighbors
    let mut iterator = MsgQueueMergerIterator::from_states(&usage_states, dst_shard)?;

    // 3) iterate over merged queues
    let mut estimated_proof_size = 0;
    let mut total_msgs = 0;
    let mut visited = ahash::AHashSet::new();
    let mut msgs_count = vec![0; neighbors.len()];

    fn visit(
        mut slice: SliceData,
        proof_size: &mut usize,
        visited: &mut ahash::AHashSet<UInt256>,
    ) -> Result<()> {
        if visited.insert(slice.cell()?.repr_hash()) {
            *proof_size += 12 + (slice.remaining_bits() + 7) / 8 + slice.remaining_references() * 3;
            while let Some(child) = slice.get_next_maybe_reference()? {
                visit(SliceData::load_cell(child)?, proof_size, visited)?;
            }
        }
        Ok(())
    }

    while let Some((key_cell, msg_slice, _lt, block_id)) = iterator.next_item_raw()? {
        log::trace!(
            "build_queue_proof: processing message {:?}",
            OutMsgQueueKey::construct_from_cell(key_cell)?
        );

        visit(msg_slice, &mut estimated_proof_size, &mut visited)?;
        total_msgs += 1;
        let idx = neighbors
            .iter()
            .position(|id| id == &block_id)
            .ok_or_else(|| error!("build_queue_proof: neighbor {} not found in list", block_id))?;
        msgs_count[idx] += 1;

        if estimated_proof_size >= limits.max_bytes as usize {
            log::debug!(
                "build_queue_proof: estimated proof size {} achived ({})",
                estimated_proof_size,
                limits.max_bytes
            );
            break;
        }
        if total_msgs >= limits.max_msgs {
            log::debug!(
                "build_queue_proof: total messages {} achived ({})",
                total_msgs,
                limits.max_msgs
            );
            break;
        }
    }
    log::debug!(
        "build_queue_proof: total messages {}, estimated proof size {}",
        total_msgs,
        estimated_proof_size
    );

    // 4) build proofs
    let mut queue_proofs_roots = Vec::with_capacity(neighbors.len());
    for (usage_tree, state_root) in usage_trees.into_iter().zip(state_roots) {
        let proof = MerkleProof::create_by_usage_tree(state_root, &usage_tree)?;
        queue_proofs_roots.push(proof.serialize()?);
    }

    Ok((queue_proofs_roots, msgs_count))
}

#[allow(dead_code)]
async fn load_data_for_queue_proof(
    dst_shard: &ShardIdent,
    neighbors: &[BlockIdExt],
    engine: &Arc<dyn EngineOperations>,
) -> Result<(Vec<Option<BlockStuff>>, Vec<Arc<ShardStateStuff>>)> {
    let mut load_block_tasks = Vec::with_capacity(neighbors.len());
    for block_id in neighbors {
        if dst_shard.intersect_with(block_id.shard()) {
            fail!(
                "Build_queue_proof: neighbor {block_id} intersects with \
                destination shard {dst_shard}"
            )
        }
        if block_id.shard().is_masterchain() {
            fail!("Build_queue_proof: neighbor {block_id} is masterchain")
        }
        if !dst_shard.is_neighbor_for(block_id.shard()) {
            fail!(
                "Build_queue_proof: neighbor {block_id} is not a neighbor \
                for destination shard {dst_shard}"
            )
        }
        let engine = engine.clone();
        let block_id = block_id.clone();
        load_block_tasks.push(async move {
            if block_id.seq_no() == 0 {
                Ok(None)
            } else {
                let handle = engine
                    .load_block_handle(&block_id)?
                    .ok_or_else(|| error!("Can't load block handle {block_id}"))?;
                let block = engine.load_block(&handle).await?;
                Ok(Some(block))
            }
        });
    }

    let mut load_state_tasks = Vec::with_capacity(neighbors.len());
    for block_id in neighbors {
        let engine = engine.clone();
        let block_id = block_id.clone();
        load_state_tasks.push(async move { engine.wait_state(&block_id, Some(1_000), true).await });
    }

    let (blocks, states) = futures::future::try_join(
        futures::future::try_join_all(load_block_tasks),
        futures::future::try_join_all(load_state_tasks),
    )
    .await?;

    Ok((blocks, states))
}
