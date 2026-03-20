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
use std::fmt::{self, Display, Formatter};
use ton_block::{
    error, fail, AccountId, AccountIdPrefixFull, AddSub, Cell, ChildCell, Deserializable,
    EnqueuedMsg, Grams, IntermediateAddress, Message, MsgEnvelope, MsgMetadata, OutMsgQueueKey,
    Result, ShardIdent, SliceData, UInt256,
};
use ton_executor::BlockchainConfig;

#[cfg(test)]
#[path = "tests/test_messages.rs"]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MsgEnvelopeStuff {
    env: MsgEnvelope,
    msg: Message,
    src_prefix: AccountIdPrefixFull,
    dst_prefix: AccountIdPrefixFull,
    cur_prefix: AccountIdPrefixFull,
    next_prefix: AccountIdPrefixFull,
    dst_account_id: AccountId,
}

impl MsgEnvelopeStuff {
    pub fn from_envelope(env: MsgEnvelope) -> Result<Self> {
        let msg = env.read_message()?;
        let src = msg.src_ref().ok_or_else(|| {
            error!("source address of message {:x} is invalid", env.message_hash())
        })?;
        let src_prefix = AccountIdPrefixFull::checked_prefix(src)?;
        let Some(dst) = msg.dst_ref() else {
            fail!("destination address of message {:x} is invalid", env.message_hash())
        };
        let (worckchain_id, dst_account_id) = dst.extract_std_address(true)?;
        let dst_prefix = AccountIdPrefixFull::std_prefix(worckchain_id, &dst_account_id)?;

        let cur_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, env.cur_addr())?;
        let next_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, env.next_addr())?;
        Ok(Self { env, msg, src_prefix, dst_prefix, cur_prefix, next_prefix, dst_account_id })
    }
    pub fn new(
        msg: Message,
        msg_cell: Cell,
        shard: &ShardIdent,
        fwd_fee: Grams,
        use_hypercube: bool,
    ) -> Result<Self> {
        let src = msg.src_ref().ok_or_else(|| {
            error!("source address of message {:x} is invalid", msg_cell.repr_hash())
        })?;
        let src_prefix = AccountIdPrefixFull::checked_prefix(src)?;
        let dst = msg.dst_ref().ok_or_else(|| {
            error!("destination address of message {:x} is invalid", msg_cell.repr_hash())
        })?;
        let (worckchain_id, dst_account_id) = dst.extract_std_address(true)?;
        let dst_prefix = AccountIdPrefixFull::std_prefix(worckchain_id, &dst_account_id)?;
        let (cur_addr, next_addr) =
            perform_hypercube_routing(&src_prefix, &dst_prefix, shard, use_hypercube)?;
        let env = MsgEnvelope::with_routing(
            ChildCell::with_cell(msg_cell),
            fwd_fee,
            cur_addr,
            next_addr,
            0,
            None,
        );
        let cur_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, env.cur_addr())?;
        let next_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, env.next_addr())?;
        Ok(Self { env, msg, src_prefix, dst_prefix, cur_prefix, next_prefix, dst_account_id })
    }
    pub fn next_hop(
        &self,
        shard: &ShardIdent,
        config: &BlockchainConfig,
        from_dispatch_queue: bool,
    ) -> Result<(Self, Grams)> {
        let mut fwd_fee_remaining = *self.fwd_fee_remaining();
        let transit_fee = if from_dispatch_queue {
            Default::default()
        } else {
            let fwd_prices = config.get_fwd_prices(self.message().is_masterchain());
            fwd_prices.next_fee_checked(&fwd_fee_remaining)?
        };
        fwd_fee_remaining.sub(&transit_fee)?;

        let (cur_addr, next_addr) =
            perform_hypercube_routing(&self.next_prefix, &self.dst_prefix, shard, true)?;
        let cur_prefix =
            self.next_prefix.interpolate_addr_intermediate(&self.dst_prefix, &cur_addr)?;
        let next_prefix =
            self.next_prefix.interpolate_addr_intermediate(&self.dst_prefix, &next_addr)?;
        let env = MsgEnvelope::with_routing(
            self.env.message().clone(),
            fwd_fee_remaining,
            cur_addr,
            next_addr,
            self.emitted_lt(),
            self.metadata().cloned(),
        );
        let msg = self.message().clone();
        let env = MsgEnvelopeStuff {
            env,
            msg,
            src_prefix: self.src_prefix.clone(),
            dst_prefix: self.dst_prefix.clone(),
            cur_prefix,
            next_prefix,
            dst_account_id: self.dst_account_id.clone(),
        };
        Ok((env, transit_fee))
    }
    pub fn inner(&self) -> &MsgEnvelope {
        &self.env
    }
    pub fn message(&self) -> &Message {
        &self.msg
    }
    pub fn message_hash(&self) -> UInt256 {
        self.env.message_hash()
    }
    pub fn message_cell(&self) -> Cell {
        self.env.message_cell()
    }
    #[cfg(test)]
    pub fn src_prefix(&self) -> &AccountIdPrefixFull {
        &self.src_prefix
    }
    pub fn dst_prefix(&self) -> &AccountIdPrefixFull {
        &self.dst_prefix
    }
    pub fn cur_prefix(&self) -> &AccountIdPrefixFull {
        &self.cur_prefix
    }
    pub fn next_prefix(&self) -> &AccountIdPrefixFull {
        &self.next_prefix
    }
    pub fn emitted_lt(&self) -> u64 {
        self.env.emitted_lt()
    }
    pub fn metadata(&self) -> Option<&MsgMetadata> {
        self.env.metadata()
    }
    pub fn fwd_fee_remaining(&self) -> &Grams {
        self.env.fwd_fee_remaining()
    }
    pub fn msg_metadata_add_depth(&self) -> Option<MsgMetadata> {
        self.env.metadata_add_depth()
    }
    pub fn set_metadata(&mut self, emitted_lt: u64, metadata: Option<MsgMetadata>) {
        self.env.set_metadata(emitted_lt, metadata);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MsgEnqueueStuff {
    enq: EnqueuedMsg,
    env: MsgEnvelopeStuff,
    lt: u64, // got from HashmapAug
}

impl MsgEnqueueStuff {
    fn checked_new(enq: EnqueuedMsg, env: MsgEnvelopeStuff, lt: u64, text: &str) -> Result<Self> {
        if env.emitted_lt() != 0 && env.emitted_lt() != lt || enq.enqueued_lt < lt {
            fail!(
                "Inconsistent {text} hash: {:x}, lt values: created_lt={:?}, emitted_lt={}, enqueued_lt={}, lt={}",
                env.message_hash(),
                env.message().created_lt(),
                env.emitted_lt(),
                enq.enqueued_lt,
                lt
            )
        }
        Ok(Self { enq, env, lt })
    }
    pub fn construct_from(slice: &mut SliceData, lt: u64) -> Result<MsgEnqueueStuff> {
        let enq = EnqueuedMsg::construct_from(slice)?;
        let env = MsgEnvelopeStuff::from_envelope(enq.read_envelope_msg()?)?;
        MsgEnqueueStuff::checked_new(enq, env, lt, "construct_from")
    }
    pub fn from_enqueue(enq: EnqueuedMsg, mut lt: u64) -> Result<MsgEnqueueStuff> {
        let env = MsgEnvelopeStuff::from_envelope(enq.read_envelope_msg()?)?;
        if lt == 0 {
            lt = env
                .message()
                .created_lt()
                .ok_or_else(|| error!("wrong message type {:x}", env.message_hash()))?;
        }
        MsgEnqueueStuff::checked_new(enq, env, lt, "from_enqueue")
    }
    pub fn from_envelope(env: MsgEnvelopeStuff, lt: u64) -> Result<MsgEnqueueStuff> {
        let enq = EnqueuedMsg::with_param(lt, env.inner())?;
        MsgEnqueueStuff::checked_new(enq, env, lt, "from_envelope")
    }
    pub fn next_hop(
        &self,
        shard: &ShardIdent,
        enqueued_lt: u64,
        config: &BlockchainConfig,
        from_dispatch_queue: bool,
    ) -> Result<(MsgEnqueueStuff, Grams)> {
        let (env, transit_fee) = self.env.next_hop(shard, config, from_dispatch_queue)?;
        let enq = EnqueuedMsg::with_param(enqueued_lt, env.inner())?;
        let enq = MsgEnqueueStuff::checked_new(enq, env, self.lt(), "next_hop")?;
        Ok((enq, transit_fee))
    }
    /// create enqeue for message
    /// create envelope message
    /// all fee from message
    /// lt = enqueue_lt = created_lt
    pub fn new(
        msg: Message,
        msg_cell: Cell,
        shard: &ShardIdent,
        fwd_fee: Grams,
        use_hypercube: bool,
        metadata: Option<MsgMetadata>,
    ) -> Result<MsgEnqueueStuff> {
        let lt = msg.created_lt().unwrap_or_default();
        let enqueued_lt = lt;
        let mut env = MsgEnvelopeStuff::new(msg, msg_cell, shard, fwd_fee, use_hypercube)?;
        env.set_metadata(0, metadata);
        let enq = EnqueuedMsg::with_param(enqueued_lt, env.inner())?;
        MsgEnqueueStuff::checked_new(enq, env, lt, "new")
    }

    pub fn same_workchain(&self) -> bool {
        let src = self.message().src_workchain_id();
        let dst = self.message().dst_workchain_id();
        src == dst
    }
    pub fn inner(&self) -> &EnqueuedMsg {
        &self.enq
    }
    pub fn envelope(&self) -> &MsgEnvelope {
        self.env.inner()
    }
    pub fn envelope_hash(&self) -> UInt256 {
        self.enq.envelope_hash()
    }
    pub fn msg_metadata_add_depth(&self) -> Option<MsgMetadata> {
        self.env.msg_metadata_add_depth()
    }
    pub fn envelope_cell(&self) -> ChildCell<MsgEnvelope> {
        self.enq.out_msg.clone()
    }
    pub fn message_hash(&self) -> UInt256 {
        self.env.message_hash()
    }
    pub fn message(&self) -> &Message {
        &self.env.msg
    }
    pub fn message_cell(&self) -> Cell {
        self.env.message_cell()
    }
    pub fn out_msg_key(&self) -> OutMsgQueueKey {
        OutMsgQueueKey::with_account_prefix(self.next_prefix(), self.message_hash())
    }
    pub fn dst_account_id(&self) -> &AccountId {
        &self.env.dst_account_id
    }
    /// LT - got from HashmapAug
    pub fn lt(&self) -> u64 {
        self.lt
    }
    /// this lt is for enqueue or transit
    pub fn enqueued_lt(&self) -> u64 {
        self.enq.enqueued_lt
    }
    /// this lt is for emit of deferred message
    pub fn emitted_lt(&self) -> u64 {
        self.env.emitted_lt()
    }
    // Unused
    //    pub fn src_prefix(&self) -> &AccountIdPrefixFull { self.env.src_prefix() }
    pub fn dst_prefix(&self) -> &AccountIdPrefixFull {
        self.env.dst_prefix()
    }
    pub fn cur_prefix(&self) -> &AccountIdPrefixFull {
        self.env.cur_prefix()
    }
    pub fn next_prefix(&self) -> &AccountIdPrefixFull {
        self.env.next_prefix()
    }
    pub fn fwd_fee_remaining(&self) -> &Grams {
        self.env.fwd_fee_remaining()
    }
    pub fn metadata(&self) -> Option<&MsgMetadata> {
        self.env.metadata()
    }

    pub fn clear_routing(&mut self) -> Result<()> {
        self.env.env.clear_routing();
        self.env.cur_prefix = self
            .env
            .src_prefix
            .interpolate_addr_intermediate(&self.env.dst_prefix, self.env.env.cur_addr())?;
        self.env.next_prefix = self
            .env
            .src_prefix
            .interpolate_addr_intermediate(&self.env.dst_prefix, self.env.env.next_addr())?;
        self.enq.out_msg = ChildCell::with_struct(self.envelope())?;
        Ok(())
    }
}

impl Display for MsgEnqueueStuff {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "message with (lt,hash)=({},{:x}), enqueued_lt={}",
            self.lt,
            self.message_hash(),
            self.enq.enqueued_lt
        )?;
        if f.alternate() {
            writeln!(f)?;
            writeln!(f, "src: {}", self.env.src_prefix)?;
            writeln!(f, "dst: {}", self.env.dst_prefix)?;
            writeln!(f, "cur: {}", self.env.cur_prefix)?;
            writeln!(f, "nxt: {}", self.env.next_prefix)?;
        }
        Ok(())
    }
}

/// Returns count of the first bits matched in both addresses
pub fn count_matching_bits(this: &AccountIdPrefixFull, other: &AccountIdPrefixFull) -> u8 {
    if this.workchain_id != other.workchain_id {
        (this.workchain_id ^ other.workchain_id).leading_zeros() as u8
    } else if this.prefix != other.prefix {
        32 + (this.prefix ^ other.prefix).leading_zeros() as u8
    } else {
        96
    }
}

/// Performs Hypercube Routing from src to dest address.
/// Result: (transit_addr_dest_bits, nh_addr_dest_bits)
pub fn perform_hypercube_routing(
    src: &AccountIdPrefixFull,
    dest: &AccountIdPrefixFull,
    cur_shard: &ShardIdent,
    use_hypercube: bool,
) -> Result<(IntermediateAddress, IntermediateAddress)> {
    if use_hypercube {
        let transit = src.interpolate_addr_intermediate(dest, &IntermediateAddress::default())?;
        if !cur_shard.contains_full_prefix(&transit) {
            fail!("Shard {} must fully contain transit prefix {}", cur_shard, transit)
        }

        if cur_shard.contains_full_prefix(dest) {
            // If destination is in this shard, set cur:=next_hop:=dest
            return Ok((IntermediateAddress::full_dest(), IntermediateAddress::full_dest()));
        }

        if transit.is_masterchain() || dest.is_masterchain() {
            // Route messages to/from masterchain directly
            return Ok((IntermediateAddress::default(), IntermediateAddress::full_dest()));
        }

        if transit.workchain_id != dest.workchain_id {
            return Ok((IntermediateAddress::default(), IntermediateAddress::use_dest_bits(32)?));
        }

        let prefix = cur_shard.shard_prefix_with_tag();
        let x = prefix & (prefix - 1);
        let y = prefix | (prefix - 1);
        let t = transit.prefix;
        let q = dest.prefix ^ t;
        // Top i bits match, next 4 bits differ:
        let mut i = q.leading_zeros() as u8 & 0xFC;
        let mut m = u64::MAX >> i;
        while i < 60 {
            m >>= 4;
            let h = t ^ (q & !m);
            i += 4;
            if h < x || h > y {
                let cur_prefix = IntermediateAddress::use_dest_bits(28 + i)?;
                let next_prefix = IntermediateAddress::use_dest_bits(32 + i)?;
                return Ok((cur_prefix, next_prefix));
            }
        }
        fail!("cannot perform hypercube routing from {} to {} via {}", src, dest, cur_shard)
    } else if cur_shard.contains_full_prefix(dest) {
        Ok((IntermediateAddress::full_dest(), IntermediateAddress::full_dest()))
    } else {
        Ok((IntermediateAddress::default(), IntermediateAddress::full_dest()))
    }
}
