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
use crate::block::BlockStuff;
use adnl::common::add_unbound_object_to_map_with_update;
use lockfree::map::Map;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use ton_block::{
    fail, read_boc, types::UInt256, Deserializable, HashmapAugType, InMsg, Message, Result,
    ShardIdent, UnixTime,
};

#[cfg(test)]
#[path = "tests/test_ext_messages.rs"]
mod tests;

const LIMIT_MEMPOOL_PER_ADDRESS: u32 = 256;
const MAX_EXTERNAL_MESSAGE_DEPTH: u16 = 512;
pub const MAX_EXTERNAL_MESSAGE_SIZE: usize = 65535;
const MESSAGE_LIFETIME: u32 = 600; // seconds
const MESSAGE_MAX_GENERATIONS: u8 = 3;

pub const EXT_MESSAGES_TRACE_TARGET: &str = "ext_messages";

#[derive(Clone)]
struct MessageKeeper {
    addr_key: (i32, UInt256),
    message: Arc<Message>,

    // active: bool,            0x1_00_00000000
    // generation: u8,          0x0_ff_00000000
    // reactivate_at: u32,      0x0_00_ffffffff
    atomic_storage: Arc<AtomicU64>,

    hash_norm: UInt256,
}

impl MessageKeeper {
    fn new(message: Arc<Message>, addr_key: (i32, UInt256)) -> Result<Self> {
        let mut atomic_storage = 0;
        Self::set_active(&mut atomic_storage, true);
        let hash_norm = message.normalized_hash()?;

        Ok(Self {
            addr_key,
            message,
            atomic_storage: Arc::new(AtomicU64::new(atomic_storage)),
            hash_norm,
        })
    }

    fn message(&self) -> &Arc<Message> {
        &self.message
    }

    fn check_active(&self, now: u32) -> bool {
        let mut atomic_storage = self.atomic_storage.load(Ordering::Relaxed);
        let active = Self::fetch_active(atomic_storage);
        let generation = Self::fetch_generation(atomic_storage);
        let reactivate_at = Self::fetch_reactivate_at(atomic_storage);

        if !active && reactivate_at <= now {
            Self::set_active(&mut atomic_storage, true);
            Self::set_generation(&mut atomic_storage, generation + 1);
            self.atomic_storage.store(atomic_storage, Ordering::Relaxed);
            true
        } else {
            active
        }
    }

    fn can_postpone(&self) -> bool {
        let atomic_storage = self.atomic_storage.load(Ordering::Relaxed);
        Self::fetch_generation(atomic_storage) < MESSAGE_MAX_GENERATIONS
    }

    fn postpone(&self, now: u32) {
        let mut atomic_storage = self.atomic_storage.load(Ordering::Relaxed);
        let active = Self::fetch_active(atomic_storage);

        if active {
            let generation = Self::fetch_generation(atomic_storage);
            Self::set_active(&mut atomic_storage, false);
            Self::set_reactivate_at(&mut atomic_storage, now + generation as u32 * 5);
            self.atomic_storage.store(atomic_storage, Ordering::Relaxed);
        }
    }

    fn fetch_active(atomic_storage: u64) -> bool {
        atomic_storage & 0x1_00_00000000 != 0
    }
    fn set_active(atomic_storage: &mut u64, active: bool) {
        if active {
            *atomic_storage |= 0x1_00_00000000;
        } else {
            *atomic_storage &= 0x0_ff_ffffffff;
        }
    }

    fn fetch_generation(atomic_storage: u64) -> u8 {
        ((atomic_storage & 0x0_ff_00000000) >> 32) as u8
    }
    fn set_generation(atomic_storage: &mut u64, generation: u8) {
        *atomic_storage &= 0x1_00_ffffffff;
        *atomic_storage |= (generation as u64) << 32;
    }

    fn fetch_reactivate_at(atomic_storage: u64) -> u32 {
        (atomic_storage & 0x0_00_ffffffff) as u32
    }
    fn set_reactivate_at(atomic_storage: &mut u64, reactivate_at: u32) {
        *atomic_storage &= 0x1_ff_00000000;
        *atomic_storage |= reactivate_at as u64;
    }
}

#[derive(Clone)]
struct MessageDescription {
    id: UInt256,
    workchain_id: i32,
    prefix: u64,
}

struct OrderMap {
    seqno: Arc<AtomicU32>,
    map: Map<u32, MessageDescription>,
}

impl OrderMap {
    fn new(id: UInt256, workchain_id: i32, prefix: u64) -> Self {
        let seqno = Arc::new(AtomicU32::new(1));
        let map = Map::new();
        map.insert(0, MessageDescription { id, workchain_id, prefix });
        Self { seqno, map }
    }
    fn insert(&self, id: UInt256, workchain_id: i32, prefix: u64) {
        let seqno = self.seqno.fetch_add(1, Ordering::Relaxed);
        self.map.insert(seqno, MessageDescription { id, workchain_id, prefix });
    }
}

pub struct MessagesPool {
    // per-address message count for rate limiting (key = (workchain_id, account_id))
    per_address: Map<(i32, UInt256), AtomicU32>,
    // map by hash of message
    messages: Map<UInt256, MessageKeeper>,
    // map by timestamp, inside map by seqno for hash of message, workchain_id and prefix of dst address
    order: Map<u32, Arc<OrderMap>>,
    // map by normalized hash to set of raw hashes (for variant-aware cleanup)
    norm_messages: Map<UInt256, Arc<Map<UInt256, ()>>>,
    // minimal timestamp
    min_timestamp: AtomicU32,

    // maximum number of messages in pool
    maximum_queue_length: Option<u32>,

    // total number of messages in pool
    total_messages: AtomicU32,
    #[cfg(test)]
    total_in_order: AtomicU32,

    // sender for applied-block cleanup worker
    applied_blocks_tx: mpsc::UnboundedSender<Arc<BlockStuff>>,
}

impl MessagesPool {
    pub fn new(
        now: u32,
        maximum_queue_length: Option<u32>,
    ) -> (Self, mpsc::UnboundedReceiver<Arc<BlockStuff>>) {
        metrics::gauge!("ton_node_ext_messages_queue_size").set(0f64);
        let (applied_blocks_tx, applied_blocks_rx) = mpsc::unbounded_channel();

        let pool = Self {
            per_address: Map::new(),
            messages: Map::with_hasher(Default::default()),
            order: Map::with_hasher(Default::default()),
            norm_messages: Map::with_hasher(Default::default()),
            min_timestamp: AtomicU32::new(now),
            maximum_queue_length,
            total_messages: AtomicU32::new(0),
            #[cfg(test)]
            total_in_order: AtomicU32::new(0),
            applied_blocks_tx,
        };
        (pool, applied_blocks_rx)
    }

    pub fn push_applied_block(&self, block: Arc<BlockStuff>) {
        if let Err(e) = self.applied_blocks_tx.send(block) {
            log::warn!(
                target: EXT_MESSAGES_TRACE_TARGET,
                "Failed to send applied block to cleanup worker: {e}"
            );
        }
    }

    pub fn start_applied_blocks_worker(
        self: Arc<Self>,
        mut rx: mpsc::UnboundedReceiver<Arc<BlockStuff>>,
    ) {
        tokio::spawn(async move {
            while let Some(block) = rx.recv().await {
                if let Err(e) = self.process_applied_block(&block) {
                    log::error!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "Failed to process applied block {}: {e}",
                        block.id()
                    );
                }
            }
            log::info!(target: EXT_MESSAGES_TRACE_TARGET, "Applied blocks cleanup worker stopped");
        });
    }

    fn process_applied_block(&self, block: &BlockStuff) -> Result<()> {
        let extra = block.block()?.read_extra()?;
        let in_msg_descr = extra.read_in_msg_descr()?;
        let mut hash_norms = Vec::new();
        in_msg_descr.iterate_with_keys(|_key: UInt256, in_msg: InMsg| {
            if let InMsg::External(ext) = in_msg {
                match ext.read_message() {
                    Ok(message) => match message.normalized_hash() {
                        Ok(hash_norm) => hash_norms.push(hash_norm),
                        Err(e) => log::warn!(
                            target: EXT_MESSAGES_TRACE_TARGET,
                            "process_applied_block {}: failed to compute hash_norm: {e}",
                            block.id()
                        ),
                    },
                    Err(e) => log::warn!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "process_applied_block {}: failed to read ext message: {e}",
                        block.id()
                    ),
                }
            }
            Ok(true)
        })?;
        if !hash_norms.is_empty() {
            log::debug!(
                target: EXT_MESSAGES_TRACE_TARGET,
                "process_applied_block {}: erasing {} norm hash(es) from pool",
                block.id(),
                hash_norms.len()
            );
            self.erase_by_hash_norm(&hash_norms);
        }
        Ok(())
    }

    fn erase_by_hash_norm(&self, hash_norms: &[UInt256]) {
        for hash_norm in hash_norms {
            let bucket = match self.norm_messages.get(hash_norm) {
                Some(b) => b,
                None => continue,
            };
            let ids_to_remove: Vec<UInt256> =
                bucket.val().iter().map(|e| e.key().clone()).collect();
            for raw_id in ids_to_remove {
                log::debug!(
                    target: EXT_MESSAGES_TRACE_TARGET,
                    "erase_by_hash_norm: removing external message {:x}", raw_id
                );
                if let Some(guard) = self.messages.remove(&raw_id) {
                    self.finalize_removal(&raw_id, guard.val());
                }
            }
        }
    }

    #[cfg(test)]
    pub fn set_now(&self, now: u32) {
        self.min_timestamp.store(now, Ordering::Relaxed);
    }

    pub fn new_message_raw(&self, data: &[u8], now: u32) -> Result<()> {
        let (id, message) = create_ext_message(data)?;
        let message = Arc::new(message);
        self.new_message(&id, message, now)?;
        Ok(())
    }

    pub fn new_message(&self, id: &UInt256, message: Arc<Message>, now: u32) -> Result<()> {
        let timestamp = self.min_timestamp.load(Ordering::Relaxed);
        if now < timestamp {
            fail!("now {} is less than minimum {} for {:x}", now, timestamp, id)
        }
        if self.messages.get(id).is_some() {
            return Ok(());
        }
        for timestamp in
            self.min_timestamp.load(Ordering::Relaxed)..now.saturating_sub(MESSAGE_LIFETIME)
        {
            self.clear_expired_messages(timestamp, u64::MAX);
            self.increment_min_timestamp(timestamp);
        }
        if let Some(maximum_queue_length) = self.maximum_queue_length {
            if self.total_messages.load(Ordering::Relaxed) >= maximum_queue_length {
                fail!("maximum number of messages in pool is reached")
            }
        }

        let workchain_id = message.dst_workchain_id().unwrap_or_default();
        let account_id = message
            .int_dst_account_id()
            .map_or(UInt256::default(), |s| UInt256::from_slice(&s.get_bytestring(0)));
        let addr_key = (workchain_id, account_id);

        // Per-address rate limiting
        if let Some(guard) = self.per_address.get(&addr_key) {
            if guard.val().load(Ordering::Relaxed) >= LIMIT_MEMPOOL_PER_ADDRESS {
                fail!(
                    "per-address limit ({}) reached for {}:{}",
                    LIMIT_MEMPOOL_PER_ADDRESS,
                    workchain_id,
                    addr_key.1.to_hex_string()
                )
            }
        }

        log::debug!(target: EXT_MESSAGES_TRACE_TARGET, "adding external message {:x}", id);
        let prefix =
            message.int_dst_account_id().map_or(0, |slice| slice.get_int(64).unwrap_or_default());
        let keeper = MessageKeeper::new(message, addr_key.clone())?;
        let hash_norm = keeper.hash_norm.clone();
        self.messages.insert(id.clone(), keeper);

        // Index by normalized hash for variant-aware cleanup
        add_unbound_object_to_map_with_update(&self.norm_messages, hash_norm, |bucket| {
            if let Some(bucket) = bucket {
                bucket.insert(id.clone(), ());
                Ok(None)
            } else {
                let m = Arc::new(Map::with_hasher(Default::default()));
                m.insert(id.clone(), ());
                Ok(Some(m))
            }
        })?;

        // Increment per-address counter
        if let Some(guard) = self.per_address.get(&addr_key) {
            guard.val().fetch_add(1, Ordering::Relaxed);
        } else {
            self.per_address.insert(addr_key, AtomicU32::new(1));
        }
        self.total_messages.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        self.total_in_order.fetch_add(1, Ordering::Relaxed);
        metrics::gauge!("ton_node_ext_messages_queue_size").increment(1f64);

        add_unbound_object_to_map_with_update(&self.order, now, |map| {
            if let Some(map) = map {
                map.insert(id.clone(), workchain_id, prefix);
                Ok(None)
            } else {
                let entry = Arc::new(OrderMap::new(id.clone(), workchain_id, prefix));
                Ok(Some(entry))
            }
        })?;
        Ok(())
    }

    pub fn iter(
        self: Arc<MessagesPool>,
        shard: ShardIdent,   // shard is used to filter messages
        now: u32,            // now is used to check if message is active
        finish_time_ms: u64, // finish_time_ms is used to limit the time of iteration
    ) -> MessagePoolIter {
        MessagePoolIter::new(self, shard, now, finish_time_ms)
    }

    pub fn complete_messages(
        &self,
        to_delay: Vec<(UInt256, String)>,
        to_delete: Vec<(UInt256, i32)>,
        now: u32,
    ) -> Result<()> {
        for (id, reason) in &to_delay {
            let result = self.messages.remove_with(id, |(_, keeper)| {
                if keeper.can_postpone() {
                    log::debug!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "complete_messages: postponed external message {:x} with reason {} while enumerating to_delay list",
                        id, reason
                    );
                    keeper.postpone(now);
                    false
                } else {
                    true
                }
            });
            if let Some(guard) = result {
                log::debug!(
                    target: EXT_MESSAGES_TRACE_TARGET,
                    "complete_messages: removing external message {:x} with reason {} because can't postpone",
                    id, reason,
                );
                self.finalize_removal(id, guard.val());
            }
        }

        // Remove accepted messages and all their normalized-hash variants from the pool
        let hash_norms: Vec<UInt256> = to_delete
            .iter()
            .filter_map(|(id, _wc)| self.messages.get(id).map(|g| g.val().hash_norm.clone()))
            .collect();
        self.erase_by_hash_norm(&hash_norms);

        Ok(())
    }

    pub fn total_messages(&self) -> u32 {
        self.total_messages.load(Ordering::Relaxed)
    }

    fn increment_min_timestamp(&self, timestamp: u32) {
        let _ = self.min_timestamp.compare_exchange(
            timestamp,
            timestamp + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    fn remove_from_norm_index(&self, id: &UInt256, hash_norm: &UInt256) {
        if let Some(bucket) = self.norm_messages.get(hash_norm) {
            bucket.val().remove(id);
        }
    }

    // Completes removal of a message that has already been extracted from `messages`.
    // Updates norm_messages, per_address counter, queue size gauge, and total count.
    fn finalize_removal(&self, id: &UInt256, keeper: &MessageKeeper) {
        self.remove_from_norm_index(id, &keeper.hash_norm);
        self.decrement_per_address(&keeper.addr_key);
        metrics::gauge!("ton_node_ext_messages_queue_size").decrement(1f64);
        self.total_messages.fetch_sub(1, Ordering::Relaxed);
    }

    fn decrement_per_address(&self, addr_key: &(i32, UInt256)) {
        if let Some(guard) = self.per_address.get(addr_key) {
            let prev = guard.val().fetch_sub(1, Ordering::Relaxed);
            if prev <= 1 {
                self.per_address.remove(addr_key);
            }
        }
    }

    fn clear_expired_messages(&self, timestamp: u32, finish_time_ms: u64) -> bool {
        let order = match self.order.get(&timestamp) {
            Some(guard) => guard.val().clone(),
            None => return true,
        };
        log::debug!(
            target: EXT_MESSAGES_TRACE_TARGET,
            "removing order map for timestamp {} because it is expired", timestamp
        );
        for seqno in 0..order.seqno.load(Ordering::Relaxed) {
            if finish_time_ms < UnixTime::now_ms() {
                self.order.insert(timestamp, order);
                return false;
            }
            if let Some(guard) = order.map.remove(&seqno) {
                if let Some(guard) = self.messages.remove(&guard.val().id) {
                    metrics::counter!("ton_node_ext_messages_expired_total").increment(1);
                    log::debug!(
                        target: EXT_MESSAGES_TRACE_TARGET,
                        "removing external message {:x} because it is expired", guard.key()
                    );
                    self.finalize_removal(guard.key(), guard.val());
                }
                #[cfg(test)]
                self.total_in_order.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.order.remove(&timestamp);
        true
    }
}

#[cfg(test)]
impl MessagesPool {
    fn get_messages(
        self: &Arc<Self>,
        shard: &ShardIdent,
        now: u32,
    ) -> Result<Vec<(Arc<Message>, UInt256)>> {
        Ok(self.clone().iter(shard.clone(), now, u64::MAX).collect())
    }

    pub fn has_messages(&self) -> bool {
        self.messages.iter().next().is_some()
    }

    pub fn clear(&mut self) {
        self.messages.clear()
    }
}

pub struct MessagePoolIter {
    pool: Arc<MessagesPool>,
    shard: ShardIdent,
    now: u32,
    timestamp: u32,
    seqno: u32,
    finish_time_ms: u64,
}

impl MessagePoolIter {
    fn new(pool: Arc<MessagesPool>, shard: ShardIdent, now: u32, finish_time_ms: u64) -> Self {
        // Start from newest messages (now) and iterate backwards to oldest
        // (matching C++ behavior where newest messages get priority)
        Self { pool, shard, now, timestamp: now, seqno: 0, finish_time_ms }
    }

    fn find_in_map(
        &mut self,
        map: &Map<u32, MessageDescription>,
    ) -> Option<(Arc<Message>, UInt256)> {
        // if link is valid we check if message is for desired shard and is active
        let descr = map.get(&self.seqno)?;
        let keeper = self.pool.messages.get(&descr.val().id)?;
        if self.shard.contains_prefix(descr.val().workchain_id, descr.val().prefix)
            && keeper.val().check_active(self.now)
        {
            return Some((keeper.val().message().clone(), descr.val().id.clone()));
        }
        // let descr = map.get(&self.seqno)?.1.clone();
        // let keeper = self.pool.messages.get(&descr.id)?.1.clone();
        // if self.shard.contains_prefix(descr.workchain_id, descr.prefix) && keeper.check_active(self.now) {
        //     return Some((keeper.message().clone(), descr.id));
        // }
        None
    }
}

impl Iterator for MessagePoolIter {
    type Item = (Arc<Message>, UInt256);

    fn next(&mut self) -> Option<Self::Item> {
        let now = UnixTime::now_ms();
        let min_timestamp = self.pool.min_timestamp.load(Ordering::Relaxed);
        // Iterate from newest to oldest (reverse chronological order)
        loop {
            if self.finish_time_ms < now {
                return None;
            }
            if self.timestamp < min_timestamp {
                return None;
            }
            // Check if this timestamp is expired
            if self.timestamp + MESSAGE_LIFETIME < self.now {
                if self.timestamp == min_timestamp {
                    if !self.pool.clear_expired_messages(self.timestamp, self.finish_time_ms) {
                        return None;
                    }
                    self.pool.increment_min_timestamp(self.timestamp);
                }
                // Skip expired timestamps
                if self.timestamp == 0 {
                    return None;
                }
                self.timestamp -= 1;
                self.seqno = 0;
                continue;
            }
            if let Some(order) =
                self.pool.order.get(&self.timestamp).map(|guard| guard.val().clone())
            {
                while self.seqno < order.seqno.load(Ordering::Relaxed) {
                    if self.finish_time_ms < now {
                        return None;
                    }
                    let result = self.find_in_map(&order.map);
                    self.seqno += 1;
                    if result.is_some() {
                        return result;
                    }
                }
            }
            if self.timestamp == 0 {
                return None;
            }
            self.timestamp -= 1;
            self.seqno = 0;
        }
    }
}

pub fn create_ext_message(data: &[u8]) -> Result<(UInt256, Message)> {
    if data.len() > MAX_EXTERNAL_MESSAGE_SIZE {
        fail!("External message is too large: {}", data.len())
    }

    let read_result = read_boc(data)?;
    let root = read_result.withdraw_single_root()?;
    if root.level() != 0 {
        fail!("External message must have zero level, but has {}", root.level())
    }
    if root.repr_depth() >= MAX_EXTERNAL_MESSAGE_DEPTH {
        fail!("External message {:x} is too deep: {}", root.repr_hash(), root.repr_depth())
    }
    let message = Message::construct_from_cell(root.clone())?;
    if let Some(header) = message.ext_in_header() {
        if header.dst.rewrite_pfx().is_some() {
            fail!(
                "External inbound message {:x} contains anycast info - it is not supported",
                root.repr_hash()
            )
        }
        Ok((root.repr_hash(), message))
    } else {
        fail!("External inbound message {:x} doesn't have proper header", root.repr_hash())
    }
}
