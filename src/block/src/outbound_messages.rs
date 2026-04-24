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
use crate::{
    define_HashmapAugE, define_HashmapE,
    dictionary::hashmapaug::{Augmentable, Augmentation, HashmapAugType},
    envelope_message::MsgEnvelope,
    error,
    error::BlockError,
    fail,
    inbound_messages::InMsg,
    messages::Message,
    miscellaneous::ProcessedInfo,
    shard::{AccountIdPrefixFull, ShardIdent},
    transactions::Transaction,
    types::{AddSub, ChildCell, CurrencyCollection},
    AccountId, BuilderData, Cell, Deserializable, HashmapSubtree, HashmapType, IBitstring,
    MerkleProof, Result, Serializable, SliceData, UInt256,
};
use std::{collections::HashSet, fmt};

#[cfg(test)]
#[path = "tests/test_out_msgs.rs"]
mod tests;

/*
        3.3 Outbound message queue and descriptors
 This section discusses OutMsgDescr, the structure representing all outbound
 messages of a block, along with their envelopes and brief descriptions of the
 reasons for including them into OutMsgDescr. This structure also describes
 all modifications of OutMsgQueue, which is a part of the shardchain state.
*/

//constructor tags of InMsg variants (only wrote bits are used (3 or 4))
const OUT_MSG_EXT: u8 = 0b000;
const OUT_MSG_IMM: u8 = 0b010;
const OUT_MSG_NEW: u8 = 0b001;
const OUT_MSG_TR: u8 = 0b011;
const OUT_MSG_DEQ_IMM: u8 = 0b100;
const OUT_MSG_DEQ: u8 = 0b1100; // is not used due CapShortDequeue
const OUT_MSG_DEQ_SHORT: u8 = 0b1101;
const OUT_MSG_TRDEQ: u8 = 0b111;
const OUT_MSG_NEW_DEFER: u8 = 0b10100;
const OUT_MSG_DEFER_TR: u8 = 0b10101;

/*
_ enqueued_lt:uint64 out_msg:^MsgEnvelope = EnqueuedMsg;
*/

///
/// EnqueuedMsg structure
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct EnqueuedMsg {
    pub enqueued_lt: u64,
    pub out_msg: ChildCell<MsgEnvelope>,
}

impl EnqueuedMsg {
    /// New default instance EnqueuedMsg structure
    pub fn new() -> Self {
        Self::default()
    }

    /// New instance EnqueuedMsg structure
    pub fn with_param(enqueued_lt: u64, env: &MsgEnvelope) -> Result<Self> {
        Ok(EnqueuedMsg { enqueued_lt, out_msg: ChildCell::with_struct(env)? })
    }

    pub fn created_lt(&self) -> Result<u64> {
        self.read_envelope_msg()?.created_lt()
    }

    pub fn enqueued_lt(&self) -> u64 {
        self.enqueued_lt
    }

    pub fn envelope_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn envelope_hash(&self) -> UInt256 {
        self.out_msg.hash()
    }

    pub fn read_envelope_msg(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }
}

impl Augmentation<u64> for EnqueuedMsg {
    fn aug(&self) -> Result<u64> {
        let env = self.read_envelope_msg()?;
        let emitted_lt = env.emitted_lt();
        if emitted_lt != 0 {
            Ok(emitted_lt)
        } else {
            env.created_lt()
        }
    }
}

impl Serializable for EnqueuedMsg {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.enqueued_lt.write_to(cell)?;
        self.out_msg.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for EnqueuedMsg {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.enqueued_lt.read_from(cell)?;
        self.out_msg.read_from(cell)?;
        Ok(())
    }
}

/////////////////////////////////////////////////////////////////////////////////////////
// Blockchain: 3.3.5
// _ (HashmapAugE 256 OutMsg CurrencyCollection) = OutMsgDescr;
//
define_HashmapAugE!(OutMsgDescr, 256, UInt256, OutMsg, CurrencyCollection);

impl OutMsgDescr {
    /// insert new or replace existing (returning prev existing value), key - hash of Message
    pub fn insert_with_key_return_prev(
        &mut self,
        key: &UInt256,
        out_msg: &OutMsg,
    ) -> Result<Option<SliceData>> {
        let aug = out_msg.aug()?;
        self.set_return_prev(key, out_msg, &aug)
    }
    /// insert new or replace existing, key - hash of Message
    pub fn insert_with_key(&mut self, key: &UInt256, out_msg: &OutMsg) -> Result<()> {
        self.insert_with_key_return_prev(key, out_msg)?;
        Ok(())
    }

    /// insert new or replace existing (returning prev existing value)
    pub fn insert_return_prev(&mut self, out_msg: &OutMsg) -> Result<()> {
        self.insert_with_key(&out_msg.read_message_hash()?, out_msg)
    }
    /// insert new or replace existing
    pub fn insert(&mut self, out_msg: &OutMsg) -> Result<()> {
        self.insert_return_prev(out_msg)?;
        Ok(())
    }

    /// insert or replace existion record (returning prev existing value)
    /// use to improve speed
    pub fn insert_serialized_return_prev(
        &mut self,
        key: &SliceData,
        msg_slice: &SliceData,
        exported: &CurrencyCollection,
    ) -> Result<Option<SliceData>> {
        match self.set_builder_serialized(key.clone(), &msg_slice.as_builder()?, exported) {
            Ok((result, _)) => Ok(result),
            Err(err) => fail!(BlockError::Other(format!("Error insert serialized message: {err}"))),
        }
    }
    pub fn insert_serialized(
        &mut self,
        key: &SliceData,
        msg_slice: &SliceData,
        exported: &CurrencyCollection,
    ) -> Result<()> {
        self.insert_serialized_return_prev(key, msg_slice, exported)?;
        Ok(())
    }

    pub fn full_exported(&self) -> &CurrencyCollection {
        self.root_extra()
    }
}

////////////////////////////////////////////////////////////////////////////////////////
// Blockchain: 3.3.6
// _ (HashmapAugE 352 EnqueuedMsg uint64) = OutMsgQueue;
// key uint352 = 32 - dest workchain_id, 64 - first 64 bit of dest account address, 256 - message hash
// aug - min created_lt
define_HashmapAugE!(OutMsgQueue, 352, OutMsgQueueKey, EnqueuedMsg, MinMsgTime);
impl HashmapSubtree for OutMsgQueue {}

pub type MinMsgTime = u64;

impl Augmentable for MinMsgTime {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        if *self > *other {
            *self = *other;
        }
        Ok(true)
    }
}

#[cfg(test)]
impl OutMsgQueue {
    /// insert OutMessage to OutMsgQueue
    pub fn insert(
        &mut self,
        workchain_id: i32,
        prefix: u64,
        env: &MsgEnvelope,
        enqueued_lt: u64,
    ) -> Result<()> {
        let created_lt = env.read_message()?.created_lt().unwrap_or_default();
        let hash = env.message_hash();
        let key = OutMsgQueueKey::with_workchain_id_and_prefix(workchain_id, prefix, hash);
        let enq = EnqueuedMsg::with_param(enqueued_lt, env)?;
        self.set(&key, &enq, &created_lt)
    }
}

///
/// The key used for an outbound message m is the concatenation of its 32-bit
/// next-hop workchain_id, the first 64 bits of the next-hop address inside that
/// workchain, and the representation hash Hash(m) of the message m itself
///

#[derive(Clone, Eq, Hash, Debug, PartialEq, Default)]
pub struct OutMsgQueueKey {
    pub workchain_id: i32,
    pub prefix: u64,
    pub hash: UInt256,
}

impl OutMsgQueueKey {
    pub fn with_workchain_id_and_prefix(workchain_id: i32, prefix: u64, hash: UInt256) -> Self {
        Self { workchain_id, prefix, hash }
    }

    // Note! hash of Message
    pub fn with_account_prefix(prefix: &AccountIdPrefixFull, hash: UInt256) -> Self {
        Self::with_workchain_id_and_prefix(prefix.workchain_id, prefix.prefix, hash)
    }
}

impl Serializable for OutMsgQueueKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.workchain_id.write_to(cell)?;
        self.prefix.write_to(cell)?;
        self.hash.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgQueueKey {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.workchain_id.read_from(slice)?;
        self.prefix.read_from(slice)?;
        self.hash.read_from(slice)?;
        Ok(())
    }
}

impl fmt::LowerHex for OutMsgQueueKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            write!(f, "0x")?;
        }
        write!(f, "{}:{:016X}, hash: {:x}", self.workchain_id, self.prefix, self.hash)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub(crate) struct OutMsgQueuesInfo {
    pub local_queue: OutMsgQueueInfo,
}

impl OutMsgQueuesInfo {
    pub fn with_local_queue(local_queue: OutMsgQueueInfo) -> Self {
        Self { local_queue }
    }
}

impl Serializable for OutMsgQueuesInfo {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        self.local_queue.write_to(builder)?;
        Ok(())
    }
}

impl Deserializable for OutMsgQueuesInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.local_queue.read_from(cell)?;
        Ok(())
    }
}

/*
// key - created_lt
_ messages:(HashmapE 64 EnqueuedMsg) count:uint48 = AccountDispatchQueue;
*/

define_HashmapE!(AccountDispatchMessages, 64, EnqueuedMsg);

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct AccountDispatchQueue {
    messages: AccountDispatchMessages,
    count: u64, // uint48
}

impl AccountDispatchQueue {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn len(&self) -> u64 {
        self.count
    }
    pub fn messages(&self) -> &AccountDispatchMessages {
        &self.messages
    }
    pub fn oldest(&self) -> Result<Option<u64>> {
        match self.messages.find_min_max_raw(true, false)? {
            Some((key, _)) => u64::construct_from_bitstring(key).map(Some),
            None => Ok(None),
        }
    }
    pub fn insert(&mut self, lt: u64, enq: &EnqueuedMsg) -> Result<()> {
        if self.messages.set_with_return(&lt, enq)?.is_some() {
            fail!("AccountDispatchQueue: message with lt={} already exists", lt)
        }
        self.count += 1;
        Ok(())
    }
    pub fn remove(&mut self, lt: u64) -> Result<EnqueuedMsg> {
        let Some(enq) = self.messages.remove_with_return(&lt)? else {
            fail!("AccountDispatchQueue: message with lt={} not found", lt)
        };
        self.count -= 1;
        Ok(enq)
    }
}

impl Serializable for AccountDispatchQueue {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.messages.write_to(cell)?;
        cell.append_bits(self.count as usize, 48)?; // uint48
        Ok(())
    }
}

impl Deserializable for AccountDispatchQueue {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.messages.read_from(cell)?;
        self.count = cell.get_next_int(48)?; // uint48
        Ok(())
    }
}

impl Augmentation<MinMsgTime> for AccountDispatchQueue {
    fn aug(&self) -> Result<MinMsgTime> {
        let min_lt = match self.messages.find_min_max_raw(true, false)? {
            Some((key, _)) => u64::construct_from_bitstring(key)?,
            None => 0,
        };
        Ok(min_lt)
    }
}

/*
// key - sender address, aug - min created_lt
_ (HashmapAugE 256 AccountDispatchQueue uint64) = DispatchQueue;
 */
define_HashmapAugE!(DispatchQueue, 256, AccountId, AccountDispatchQueue, MinMsgTime);

/*
out_msg_queue_extra#0 dispatch_queue:DispatchQueue out_queue_size:(Maybe uint48) = OutMsgQueueExtra;
 */
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct OutMsgQueueExtra {
    pub dispatch_queue: DispatchQueue,
    pub out_queue_size: usize, // uint48
}

impl OutMsgQueueExtra {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dispatch_queue(&self) -> &DispatchQueue {
        &self.dispatch_queue
    }

    pub fn out_queue_size(&self) -> usize {
        self.out_queue_size
    }

    pub fn split(&mut self, sub_shard: &ShardIdent, size0: usize) -> Result<Self> {
        let split_key = sub_shard.parent_shard_key(false);
        let (q0, q1) = self.dispatch_queue.split(&split_key)?;
        let out_queue_size = self.out_queue_size - size0;
        self.out_queue_size = size0;
        let dispatch_queue = if sub_shard.is_left_child() {
            self.dispatch_queue = q0;
            q1
        } else {
            self.dispatch_queue = q1;
            q0
        };
        Ok(OutMsgQueueExtra { dispatch_queue, out_queue_size })
    }

    pub fn merge(&mut self, other: &Self, key: &SliceData) -> Result<()> {
        self.dispatch_queue.merge(&other.dispatch_queue, key)?;
        self.out_queue_size += other.out_queue_size;
        Ok(())
    }

    pub fn insert(&mut self, account_id: &AccountId, lt: u64, enq: &EnqueuedMsg) -> Result<()> {
        let (mut queue, min_lt) = match self.dispatch_queue.get_with_aug(account_id)? {
            Some((queue, min_lt)) => (queue, min_lt.min(lt)),
            None => (AccountDispatchQueue::default(), lt),
        };
        queue.insert(lt, enq)?;
        self.dispatch_queue.set(account_id, &queue, &min_lt)?;
        Ok(())
    }
}

impl Serializable for OutMsgQueueExtra {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(0, 4)?; // tag 0
        self.dispatch_queue.write_to(cell)?;
        cell.append_bit_one()?; // has size
        cell.append_bits(self.out_queue_size, 48)?; // uint48
        Ok(())
    }
}

impl Deserializable for OutMsgQueueExtra {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_int(4)?;
        if tag != 0 {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.dispatch_queue.read_from(cell)?;
        if cell.get_next_bit()? {
            self.out_queue_size = cell.get_next_int(48)? as usize; // uint48
        } else {
            self.out_queue_size = 0;
        }
        Ok(())
    }
}

/*
_ out_queue:OutMsgQueue proc_info:ProcessedInfo
  extra:(Maybe OutMsgQueueExtra) = OutMsgQueueInfo;
*/
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct OutMsgQueueInfo {
    out_queue: OutMsgQueue,
    proc_info: ProcessedInfo,
    extra: OutMsgQueueExtra,
}

#[derive(Default)]
pub struct ProofForWc {
    pub proof: MerkleProof,
    pub root_hashes: HashSet<UInt256>,
    pub sub_queue_root_hash: UInt256,
    pub sub_queue_root_hash_2: Option<UInt256>,
}

impl OutMsgQueueInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_params(
        out_queue: OutMsgQueue,
        proc_info: ProcessedInfo,
        extra: OutMsgQueueExtra,
    ) -> Self {
        OutMsgQueueInfo { out_queue, proc_info, extra }
    }

    pub fn out_queue(&self) -> &OutMsgQueue {
        &self.out_queue
    }

    pub fn set_out_queue(&mut self, out_queue: OutMsgQueue) {
        self.out_queue = out_queue;
    }

    pub fn out_queue_mut(&mut self) -> &mut OutMsgQueue {
        &mut self.out_queue
    }

    pub fn proc_info(&self) -> &ProcessedInfo {
        &self.proc_info
    }

    pub fn proc_info_mut(&mut self) -> &mut ProcessedInfo {
        &mut self.proc_info
    }

    pub fn set_proc_info(&mut self, proc_info: ProcessedInfo) {
        self.proc_info = proc_info;
    }

    pub fn extra(&self) -> &OutMsgQueueExtra {
        &self.extra
    }

    pub fn merge_with(&mut self, other: &Self) -> Result<bool> {
        let mut result = self.out_queue.combine_with(&other.out_queue)?;
        if result {
            self.out_queue.update_root_extra()?;
        }
        result |= self.proc_info.combine_with(&other.proc_info)?;
        Ok(result)
    }
}

impl Serializable for OutMsgQueueInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_queue.write_to(cell)?;
        self.proc_info.write_to(cell)?;
        cell.append_bit_one()?; // has extra
        self.extra.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgQueueInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.out_queue.read_from(cell)?;
        self.proc_info.read_from(cell)?;
        if cell.get_next_bit()? {
            self.extra.read_from(cell)?;
        } else {
            self.extra = OutMsgQueueExtra::default();
        }
        Ok(())
    }
}

///
/// OutMsg structure
/// blockchain spec 3.3.3. Descriptor of an outbound message
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum OutMsg {
    #[default]
    None,
    /// External outbound messages, or “messages to nowhere”
    /// msg_export_ext$000 msg:^(Message Any) transaction:^Transaction = OutMsg;
    External(OutMsgExternal),
    /// Ordinary (internal) outbound messages
    /// msg_export_new$001 out_msg:^MsgEnvelope transaction:^Transaction = OutMsg;
    New(OutMsgNew),
    /// Immediately processed internal outbound messages
    /// msg_export_imm$010 out_msg:^MsgEnvelope transaction:^Transaction reimport:^InMsg = OutMsg;
    Immediate(OutMsgImmediate),
    /// Transit (internal) outbound messages
    /// msg_export_tr$011 out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
    Transit(OutMsgTransit),
    /// msg_export_deq_imm$100 out_msg:^MsgEnvelope reimport:^InMsg = OutMsg;
    DequeueImmediate(OutMsgDequeueImmediate),
    /// msg_export_deq$1100 out_msg:^MsgEnvelope import_block_lt:uint63 = OutMsg;
    Dequeue(OutMsgDequeue),
    /// msg_export_deq_short$1101 msg_env_hash:bits256 next_workchain:int32 next_addr_pfx:uint64 import_block_lt:uint64 = OutMsg;
    DequeueShort(OutMsgDequeueShort),
    /// msg_export_tr_req$111 out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
    TransitRequeued(OutMsgTransitRequeued),
    /// Deferred ordinary (internal) outbound messages
    /// msg_export_new_defer$10100 out_msg:^MsgEnvelope transaction:^Transaction = OutMsg;
    NewDefer(OutMsgNewDefer),
    /// msg_export_deferred_tr$10101  out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
    DeferredTransit(OutMsgDefferedTransit),
}

impl fmt::Display for OutMsg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OutMsg::External(_) => write!(f, "OutMsg::External")?,
            OutMsg::Immediate(_) => write!(f, "OutMsg::Immediate")?,
            OutMsg::New(_) => write!(f, "OutMsg::New")?,
            OutMsg::Transit(x) => {
                write!(f, "OutMsg::Transit")?;
                if let Ok(env) = x.out_msg.read_struct() {
                    write!(f, " env: {env:?}")?;
                }
                if let Ok(in_msg) = x.imported.read_struct() {
                    write!(f, " imported: {in_msg}")?;
                }
            }
            OutMsg::Dequeue(_) => write!(f, "OutMsg::Dequeue")?,
            OutMsg::DequeueShort(_) => write!(f, "OutMsg::DequeueShort")?,
            OutMsg::DequeueImmediate(_) => write!(f, "OutMsg::DequeueImmediate")?,
            OutMsg::TransitRequeued(_) => write!(f, "OutMsg::TransitRequeued")?,
            OutMsg::NewDefer(_) => write!(f, "OutMsg::NewDefer")?,
            OutMsg::DeferredTransit(_) => write!(f, "OutMsg::DeferredTransit")?,
            OutMsg::None => write!(f, "OutMsg::None")?,
        }
        Ok(())
    }
}

impl OutMsg {
    /// Create External
    pub fn external(msg_cell: ChildCell<Message>, tr_cell: ChildCell<Transaction>) -> OutMsg {
        OutMsg::External(OutMsgExternal::with_cells(msg_cell, tr_cell))
    }
    /// Create Ordinary internal message
    pub fn new(env_cell: ChildCell<MsgEnvelope>, tr_cell: ChildCell<Transaction>) -> OutMsg {
        OutMsg::New(OutMsgNew::with_cells(env_cell, tr_cell))
    }
    /// Create Immediate internal message
    pub fn immediate(
        env_cell: ChildCell<MsgEnvelope>,
        tr_cell: ChildCell<Transaction>,
        reimport_msg_cell: ChildCell<InMsg>,
    ) -> OutMsg {
        OutMsg::Immediate(OutMsgImmediate::with_cells(env_cell, tr_cell, reimport_msg_cell))
    }
    /// Create Transit internal message
    pub fn transit(
        env_cell: ChildCell<MsgEnvelope>,
        imported_cell: ChildCell<InMsg>,
        requeue: bool,
    ) -> OutMsg {
        if requeue {
            OutMsg::TransitRequeued(OutMsgTransitRequeued::with_cells(env_cell, imported_cell))
        } else {
            OutMsg::Transit(OutMsgTransit::with_cells(env_cell, imported_cell))
        }
    }
    /// Create Dequeue internal message
    pub fn dequeue_long(env_cell: ChildCell<MsgEnvelope>, import_block_lt: u64) -> OutMsg {
        OutMsg::Dequeue(OutMsgDequeue::with_cells(env_cell, import_block_lt))
    }
    /// Create Dequeue Short internal message
    pub fn dequeue_short(
        msg_env_hash: UInt256,
        next_prefix: &AccountIdPrefixFull,
        import_block_lt: u64,
    ) -> OutMsg {
        OutMsg::DequeueShort(OutMsgDequeueShort {
            msg_env_hash,
            next_workchain: next_prefix.workchain_id,
            next_addr_pfx: next_prefix.prefix,
            import_block_lt,
        })
    }

    /// Create Dequeue immediate message
    pub fn dequeue_immediate(
        env_cell: ChildCell<MsgEnvelope>,
        reimport_msg_cell: ChildCell<InMsg>,
    ) -> OutMsg {
        OutMsg::DequeueImmediate(OutMsgDequeueImmediate::with_cells(env_cell, reimport_msg_cell))
    }

    /// Create new defer internal message
    pub fn new_defer(env_cell: ChildCell<MsgEnvelope>, tr_cell: ChildCell<Transaction>) -> OutMsg {
        OutMsg::NewDefer(OutMsgNewDefer::with_cells(env_cell, tr_cell))
    }

    /// Create Transit internal message
    pub fn deferred_transit(
        env_cell: ChildCell<MsgEnvelope>,
        imported_cell: ChildCell<InMsg>,
    ) -> OutMsg {
        OutMsg::DeferredTransit(OutMsgDefferedTransit::with_cells(env_cell, imported_cell))
    }

    /// Check if is valid message
    pub fn is_valid(&self) -> bool {
        self != &OutMsg::None
    }

    pub fn tag(&self) -> u8 {
        match self {
            OutMsg::External(_) => OUT_MSG_EXT,
            OutMsg::Immediate(_) => OUT_MSG_IMM,
            OutMsg::New(_) => OUT_MSG_NEW,
            OutMsg::Transit(_) => OUT_MSG_TR,
            OutMsg::Dequeue(_) => OUT_MSG_DEQ,            // 4 bits
            OutMsg::DequeueShort(_) => OUT_MSG_DEQ_SHORT, // 4 bits
            OutMsg::DequeueImmediate(_) => OUT_MSG_DEQ_IMM,
            OutMsg::TransitRequeued(_) => OUT_MSG_TRDEQ,
            OutMsg::NewDefer(_) => OUT_MSG_NEW_DEFER, // 5 bits
            OutMsg::DeferredTransit(_) => OUT_MSG_DEFER_TR, // 5 bits
            OutMsg::None => 16,
        }
    }

    ///
    /// the function returns the message envelop (if exists)
    ///
    pub fn read_msg_envelope(&self) -> Result<Option<MsgEnvelope>> {
        let msg_opt = match self {
            OutMsg::External(_) => None,
            OutMsg::Immediate(x) => Some(x.read_out_message()?),
            OutMsg::New(x) => Some(x.read_out_message()?),
            OutMsg::Transit(x) => Some(x.read_out_message()?),
            OutMsg::Dequeue(x) => Some(x.read_out_message()?),
            OutMsg::DequeueShort(_) => None,
            OutMsg::DequeueImmediate(x) => Some(x.read_out_message()?),
            OutMsg::TransitRequeued(x) => Some(x.read_out_message()?),
            OutMsg::NewDefer(x) => Some(x.read_out_message()?),
            OutMsg::DeferredTransit(x) => Some(x.read_out_message()?),
            OutMsg::None => fail!("wrong message type"),
        };
        Ok(msg_opt)
    }

    ///
    /// the function returns the message envelop (if exists)
    ///
    pub fn msg_envelope_cell(&self) -> Option<Cell> {
        match self {
            OutMsg::External(_) => None,
            OutMsg::Immediate(x) => Some(x.out_message_cell()),
            OutMsg::New(x) => Some(x.out_message_cell()),
            OutMsg::Transit(x) => Some(x.out_message_cell()),
            OutMsg::Dequeue(x) => Some(x.out_message_cell()),
            OutMsg::DequeueShort(_) => None,
            OutMsg::DequeueImmediate(x) => Some(x.out_message_cell()),
            OutMsg::TransitRequeued(x) => Some(x.out_message_cell()),
            OutMsg::NewDefer(x) => Some(x.out_message_cell()),
            OutMsg::DeferredTransit(x) => Some(x.out_message_cell()),
            OutMsg::None => None,
        }
    }

    ///
    /// the function returns the message (if exists)
    ///
    pub fn read_message(&self) -> Result<Option<Message>> {
        let msg_opt = match self {
            OutMsg::External(x) => Some(x.read_message()?),
            OutMsg::Immediate(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::New(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::Transit(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::Dequeue(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::DequeueShort(_) => None,
            OutMsg::DequeueImmediate(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::TransitRequeued(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::NewDefer(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::DeferredTransit(x) => Some(x.read_out_message()?.read_message()?),
            OutMsg::None => fail!("wrong message type"),
        };
        Ok(msg_opt)
    }

    ///
    /// the function returns the messages hash
    ///
    pub fn read_message_hash(&self) -> Result<UInt256> {
        let hash = match self {
            OutMsg::External(x) => x.message_cell().repr_hash().clone(),
            OutMsg::Immediate(x) => x.read_out_message()?.message_cell().repr_hash().clone(),
            OutMsg::New(x) => x.read_out_message()?.message_hash(),
            OutMsg::Transit(x) => x.read_out_message()?.message_hash(),
            OutMsg::Dequeue(x) => x.read_out_message()?.message_hash(),
            OutMsg::DequeueShort(_) => fail!("dequeue short out msg doesn't have message hash"),
            OutMsg::DequeueImmediate(x) => x.read_out_message()?.message_hash(),
            OutMsg::TransitRequeued(x) => x.read_out_message()?.message_hash(),
            OutMsg::NewDefer(x) => x.read_out_message()?.message_hash(),
            OutMsg::DeferredTransit(x) => x.read_out_message()?.message_hash(),
            OutMsg::None => fail!("wrong message type"),
        };
        Ok(hash)
    }

    ///
    /// the function returns the message cell (if exists)
    ///
    pub fn message_cell(&self) -> Result<Option<Cell>> {
        let msg_opt = match self {
            OutMsg::External(x) => Some(x.message_cell()),
            OutMsg::Immediate(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::New(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::Transit(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::Dequeue(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::DequeueShort(_) => None,
            OutMsg::DequeueImmediate(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::TransitRequeued(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::NewDefer(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::DeferredTransit(x) => Some(x.read_out_message()?.message_cell()),
            OutMsg::None => fail!("wrong message type"),
        };
        Ok(msg_opt)
    }

    ///
    /// the function returns the message envelope hash (if exists)
    ///
    pub fn envelope_message_hash(&self) -> Option<UInt256> {
        match self {
            OutMsg::External(_) => None,
            OutMsg::Immediate(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::New(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::Transit(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::Dequeue(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::DequeueShort(x) => Some(x.msg_env_hash.clone()),
            OutMsg::DequeueImmediate(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::TransitRequeued(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::NewDefer(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::DeferredTransit(x) => Some(x.out_message_cell().repr_hash().clone()),
            OutMsg::None => None,
        }
    }

    pub fn transaction_cell(&self) -> Option<Cell> {
        match self {
            OutMsg::External(x) => Some(x.transaction_cell()),
            OutMsg::Immediate(x) => Some(x.transaction_cell()),
            OutMsg::New(x) => Some(x.transaction_cell()),
            OutMsg::Transit(_x) => None,
            OutMsg::Dequeue(_x) => None,
            OutMsg::DequeueShort(_x) => None,
            OutMsg::DequeueImmediate(_x) => None,
            OutMsg::TransitRequeued(_x) => None,
            OutMsg::NewDefer(x) => Some(x.transaction_cell()),
            OutMsg::DeferredTransit(_x) => None,
            OutMsg::None => None,
        }
    }

    pub fn read_transaction(&self) -> Result<Option<Transaction>> {
        match self.transaction_cell() {
            Some(cell) => Ok(Some(Transaction::construct_from_cell(cell)?)),
            None => Ok(None),
        }
    }

    pub fn read_reimport_message(&self) -> Result<Option<InMsg>> {
        let msg = match self {
            OutMsg::Immediate(x) => x.read_reimport_message()?,
            OutMsg::Transit(x) => x.read_imported()?,
            OutMsg::DequeueImmediate(x) => x.read_reimport_message()?,
            OutMsg::TransitRequeued(x) => x.read_imported()?,
            OutMsg::DeferredTransit(x) => x.read_imported()?,
            _ => return Ok(None),
        };
        Ok(Some(msg))
    }

    pub fn reimport_cell(&self) -> Option<Cell> {
        match self {
            OutMsg::Immediate(x) => Some(x.reimport_message_cell()),
            OutMsg::Transit(x) => Some(x.imported_cell()),
            OutMsg::DequeueImmediate(x) => Some(x.reimport_message_cell()),
            OutMsg::TransitRequeued(x) => Some(x.imported_cell()),
            OutMsg::DeferredTransit(x) => Some(x.imported_cell()),
            _ => None,
        }
    }

    pub fn exported_value(&self) -> Result<CurrencyCollection> {
        self.aug()
    }
}

impl Augmentation<CurrencyCollection> for OutMsg {
    fn aug(&self) -> Result<CurrencyCollection> {
        let mut exported = CurrencyCollection::default();
        match self {
            OutMsg::New(x) => {
                let env = x.read_out_message()?;
                let msg = env.read_message()?;
                // exported value = msg.value + msg.ihr_fee + fwd_fee_remaining
                exported.add(msg.header().get_value().unwrap())?;
                exported.coins.add(env.fwd_fee_remaining())?;
            }
            OutMsg::Transit(x) => {
                let env = x.read_out_message()?;
                let msg = env.read_message()?;
                // exported value = msg.value + msg.ihr_fee + fwd_fee_remaining
                exported.add(msg.header().get_value().unwrap())?;
                exported.coins.add(env.fwd_fee_remaining())?;
            }
            OutMsg::TransitRequeued(x) => {
                let env = x.read_out_message()?;
                let msg = env.read_message()?;
                // exported value = msg.value + msg.ihr_fee + fwd_fee_remaining
                exported.add(msg.header().get_value().unwrap())?;
                exported.coins.add(env.fwd_fee_remaining())?;
            }
            OutMsg::NewDefer(x) => {
                let env = x.read_out_message()?;
                let msg = env.read_message()?;
                // exported value = msg.value + msg.ihr_fee + fwd_fee_remaining
                exported.add(msg.header().get_value().unwrap())?;
                exported.coins.add(env.fwd_fee_remaining())?;
            }
            OutMsg::DeferredTransit(x) => {
                let env = x.read_out_message()?;
                let msg = env.read_message()?;
                // exported value = msg.value + msg.ihr_fee + fwd_fee_remaining
                exported.add(msg.header().get_value().unwrap())?;
                exported.coins.add(env.fwd_fee_remaining())?;
            }
            OutMsg::None => fail!("wrong OutMsg type"),
            // for other types - no value exported
            _ => (), // OutMsg::External(x) =>
                     // OutMsg::Immediate(x) =>
                     // OutMsg::Dequeue(x) =>
                     // OutMsg::DequeueImmediate(x) =>
        }
        Ok(exported)
    }
}

///internal helper macros for reading InMsg variants
macro_rules! read_descr {
    ($slice:expr, $variant:ident) => {{
        OutMsg::$variant(Deserializable::construct_from($slice)?)
    }};
}

///internal helper macros for writing InMsg variants
macro_rules! write_tag {
    ($builder:expr, $tag:expr, $tag_len:expr) => {{
        $builder.append_bits($tag as usize, $tag_len)?;
        $builder
    }};
}

impl Serializable for OutMsg {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            OutMsg::External(x) => x.write_to(write_tag!(cell, OUT_MSG_EXT, 3)),
            OutMsg::Immediate(x) => x.write_to(write_tag!(cell, OUT_MSG_IMM, 3)),
            OutMsg::New(x) => x.write_to(write_tag!(cell, OUT_MSG_NEW, 3)),
            OutMsg::Transit(x) => x.write_to(write_tag!(cell, OUT_MSG_TR, 3)),
            OutMsg::Dequeue(x) => x.write_to(write_tag!(cell, OUT_MSG_DEQ, 4)),
            OutMsg::DequeueShort(x) => x.write_to(write_tag!(cell, OUT_MSG_DEQ_SHORT, 4)),
            OutMsg::DequeueImmediate(x) => x.write_to(write_tag!(cell, OUT_MSG_DEQ_IMM, 3)),
            OutMsg::TransitRequeued(x) => x.write_to(write_tag!(cell, OUT_MSG_TRDEQ, 3)),
            OutMsg::NewDefer(x) => x.write_to(write_tag!(cell, OUT_MSG_NEW_DEFER, 5)),
            OutMsg::DeferredTransit(x) => x.write_to(write_tag!(cell, OUT_MSG_DEFER_TR, 5)),
            OutMsg::None => {
                fail!(BlockError::InvalidOperation("OutMsg::None can't be serialized".to_string()))
            }
        }
    }
}

impl Deserializable for OutMsg {
    fn construct_from(cell: &mut SliceData) -> Result<Self> {
        let mut tag = cell.get_next_int(3)?;
        let msg = match tag as u8 {
            OUT_MSG_EXT => read_descr!(cell, External),
            OUT_MSG_IMM => read_descr!(cell, Immediate),
            OUT_MSG_NEW => read_descr!(cell, New),
            OUT_MSG_TR => read_descr!(cell, Transit),
            OUT_MSG_DEQ_IMM => read_descr!(cell, DequeueImmediate),
            OUT_MSG_TRDEQ => read_descr!(cell, TransitRequeued),
            _ => {
                tag = (tag << 1) + cell.get_next_int(1)?;
                match tag as u8 {
                    OUT_MSG_DEQ => read_descr!(cell, Dequeue),
                    OUT_MSG_DEQ_SHORT => read_descr!(cell, DequeueShort),
                    _ => {
                        tag = (tag << 1) + cell.get_next_int(1)?;
                        match tag as u8 {
                            OUT_MSG_NEW_DEFER => read_descr!(cell, NewDefer),
                            OUT_MSG_DEFER_TR => read_descr!(cell, DeferredTransit),
                            _ => fail!(Self::invalid_tag(tag as u32)),
                        }
                    }
                }
            }
        };
        Ok(msg)
    }
}

///
/// msg_export_ext$000 msg:^Message transaction:^Transaction = OutMsg;
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgExternal {
    msg: ChildCell<Message>,
    transaction: ChildCell<Transaction>,
}

impl OutMsgExternal {
    pub fn with_cells(msg: ChildCell<Message>, transaction: ChildCell<Transaction>) -> Self {
        OutMsgExternal { msg, transaction }
    }

    pub fn read_message(&self) -> Result<Message> {
        self.msg.read_struct()
    }

    pub fn message_cell(&self) -> Cell {
        self.msg.cell()
    }

    pub fn message_hash(&self) -> UInt256 {
        self.msg.hash()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }
}

impl Serializable for OutMsgExternal {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgExternal {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_imm$010 out_msg:^MsgEnvelope transaction:^Transaction reimport:^InMsg = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgImmediate {
    out_msg: ChildCell<MsgEnvelope>,
    transaction: ChildCell<Transaction>,
    reimport: ChildCell<InMsg>,
}

impl OutMsgImmediate {
    pub fn with_cells(
        out_msg: ChildCell<MsgEnvelope>,
        transaction: ChildCell<Transaction>,
        reimport: ChildCell<InMsg>,
    ) -> OutMsgImmediate {
        OutMsgImmediate { out_msg, transaction, reimport }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }

    pub fn read_reimport_message(&self) -> Result<InMsg> {
        self.reimport.read_struct()
    }

    pub fn reimport_message_cell(&self) -> Cell {
        self.reimport.cell()
    }
}

impl Serializable for OutMsgImmediate {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        self.reimport.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgImmediate {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        self.reimport.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_new$001 out_msg:^MsgEnvelope transaction:^Transaction = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgNew {
    out_msg: ChildCell<MsgEnvelope>,
    transaction: ChildCell<Transaction>,
}

impl OutMsgNew {
    pub fn with_cells(
        out_msg: ChildCell<MsgEnvelope>,
        transaction: ChildCell<Transaction>,
    ) -> Self {
        OutMsgNew { out_msg, transaction }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }
}

impl Serializable for OutMsgNew {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgNew {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_tr$011 out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgTransit {
    out_msg: ChildCell<MsgEnvelope>,
    imported: ChildCell<InMsg>,
}

impl OutMsgTransit {
    pub fn with_cells(out_msg: ChildCell<MsgEnvelope>, imported: ChildCell<InMsg>) -> Self {
        OutMsgTransit { out_msg, imported }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_imported(&self) -> Result<InMsg> {
        self.imported.read_struct()
    }

    pub fn imported_cell(&self) -> Cell {
        self.imported.cell()
    }
}

impl Serializable for OutMsgTransit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.imported.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgTransit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.imported.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_deq$110 out_msg:^MsgEnvelope import_block_lt:uint64 = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgDequeueImmediate {
    out_msg: ChildCell<MsgEnvelope>,
    reimport: ChildCell<InMsg>,
}

impl OutMsgDequeueImmediate {
    pub fn with_cells(out_msg: ChildCell<MsgEnvelope>, reimport: ChildCell<InMsg>) -> Self {
        OutMsgDequeueImmediate { out_msg, reimport }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_reimport_message(&self) -> Result<InMsg> {
        self.reimport.read_struct()
    }

    pub fn reimport_message_cell(&self) -> Cell {
        self.reimport.cell()
    }
}

impl Serializable for OutMsgDequeueImmediate {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.reimport.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgDequeueImmediate {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.reimport.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_deq$1100 out_msg:^MsgEnvelope import_block_lt:uint63 = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgDequeue {
    pub out_msg: ChildCell<MsgEnvelope>,
    pub import_block_lt: u64,
}

impl OutMsgDequeue {
    pub fn with_cells(out_msg: ChildCell<MsgEnvelope>, lt: u64) -> Self {
        OutMsgDequeue { out_msg, import_block_lt: lt }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }
}

impl Serializable for OutMsgDequeue {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        cell.append_bits(self.import_block_lt as usize, 63)?;
        Ok(())
    }
}

impl Deserializable for OutMsgDequeue {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.import_block_lt = slice.get_next_int(63)?;
        Ok(())
    }
}

///
/// msg_export_deq_short$1101 msg_env_hash:bits256 next_workchain:int32 next_addr_pfx:uint64 import_block_lt:uint64 = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgDequeueShort {
    pub msg_env_hash: UInt256,
    pub next_workchain: i32,
    pub next_addr_pfx: u64,
    pub import_block_lt: u64,
}

impl Serializable for OutMsgDequeueShort {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.msg_env_hash.write_to(cell)?;
        self.next_workchain.write_to(cell)?;
        self.next_addr_pfx.write_to(cell)?;
        self.import_block_lt.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgDequeueShort {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.msg_env_hash.read_from(cell)?;
        self.next_workchain.read_from(cell)?;
        self.next_addr_pfx.read_from(cell)?;
        self.import_block_lt.read_from(cell)?;
        Ok(())
    }
}

///
/// msg_export_tr_req$111 out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgTransitRequeued {
    out_msg: ChildCell<MsgEnvelope>,
    imported: ChildCell<InMsg>,
}

impl OutMsgTransitRequeued {
    pub fn with_cells(out_msg: ChildCell<MsgEnvelope>, imported: ChildCell<InMsg>) -> Self {
        OutMsgTransitRequeued { out_msg, imported }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_imported(&self) -> Result<InMsg> {
        self.imported.read_struct()
    }

    pub fn imported_cell(&self) -> Cell {
        self.imported.cell()
    }
}

impl Serializable for OutMsgTransitRequeued {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.imported.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgTransitRequeued {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.imported.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_new_defer$10100 out_msg:^MsgEnvelope transaction:^Transaction = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgNewDefer {
    out_msg: ChildCell<MsgEnvelope>,
    transaction: ChildCell<Transaction>,
}

impl OutMsgNewDefer {
    pub fn with_cells(
        out_msg: ChildCell<MsgEnvelope>,
        transaction: ChildCell<Transaction>,
    ) -> Self {
        Self { out_msg, transaction }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }
}

impl Serializable for OutMsgNewDefer {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgNewDefer {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        Ok(())
    }
}

///
/// msg_export_deferred_tr$10101  out_msg:^MsgEnvelope imported:^InMsg = OutMsg;
///

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutMsgDefferedTransit {
    out_msg: ChildCell<MsgEnvelope>,
    imported: ChildCell<InMsg>,
}

impl OutMsgDefferedTransit {
    pub fn with_cells(out_msg: ChildCell<MsgEnvelope>, imported: ChildCell<InMsg>) -> Self {
        Self { out_msg, imported }
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn read_imported(&self) -> Result<InMsg> {
        self.imported.read_struct()
    }

    pub fn imported_cell(&self) -> Cell {
        self.imported.cell()
    }
}

impl Serializable for OutMsgDefferedTransit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.out_msg.write_to(cell)?;
        self.imported.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for OutMsgDefferedTransit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.out_msg.read_from(slice)?;
        self.imported.read_from(slice)?;
        Ok(())
    }
}
