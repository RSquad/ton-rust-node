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
    error,
    error::BlockError,
    fail,
    messages::{Message, MsgAddressInt},
    shard::{AccountIdPrefixFull, ShardIdent},
    types::{AddSub, ChildCell, Coins},
    BuilderData, Cell, Deserializable, IBitstring, Result, Serializable, SliceData, UInt256,
};
use std::cmp::Ordering;

#[cfg(test)]
#[path = "tests/test_envelope_message.rs"]
mod tests;

/*

3.1.15. Enveloped messages. Message envelopes are used for attaching
routing information, such as the current (transit) address and the next-hop
address, to inbound, transit, and outbound messages (cf. 2.1.16). The message
itself is kept in a separate cell and referred to from the message envelope
by a cell reference.

*/

/////////////////////////////////////////////////////////////////////
///
/// interm_addr_regular$0 use_dest_bits:(#<= 96) = IntermediateAddress;
/// interm_addr_simple$10 workchain_id:int8 addr_pfx:(64 * Bit) = IntermediateAddress;
/// interm_addr_ext$11 workchain_id:int32 addr_pfx:(64 * Bit) = IntermediateAddress;
///

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IntermediateAddress {
    Regular(IntermediateAddressRegular),
    Simple(IntermediateAddressSimple),
    Ext(IntermediateAddressExt),
}

impl IntermediateAddress {
    pub fn use_src_bits(use_src_bits: u8) -> Result<Self> {
        let ia = IntermediateAddressRegular::with_use_src_bits(use_src_bits)?;
        Ok(IntermediateAddress::Regular(ia))
    }

    pub fn use_dest_bits(use_dest_bits: u8) -> Result<Self> {
        let ia = IntermediateAddressRegular::with_use_dest_bits(use_dest_bits)?;
        Ok(IntermediateAddress::Regular(ia))
    }

    pub fn full_src() -> Self {
        let src = IntermediateAddressRegular::with_use_dest_bits(0).unwrap();
        IntermediateAddress::Regular(src)
    }

    pub fn full_dest() -> Self {
        let dest = IntermediateAddressRegular::with_use_src_bits(0).unwrap();
        IntermediateAddress::Regular(dest)
    }

    pub fn any_masterchain() -> Self {
        let master = IntermediateAddressSimple::with_addr(-1, 0x8000000000000000);
        IntermediateAddress::Simple(master)
    }
    ///
    /// Get workchain_id
    ///
    pub fn workchain_id(&self) -> Result<i32> {
        match self {
            IntermediateAddress::Simple(simple) => Ok(simple.workchain_id() as i32),
            IntermediateAddress::Ext(ext) => Ok(ext.workchain_id()),
            _ => fail!("Unsupported address type"),
        }
    }

    ///
    /// Get prefix
    ///
    pub fn prefix(&self) -> Result<u64> {
        match self {
            IntermediateAddress::Simple(simple) => Ok(simple.addr_pfx()),
            IntermediateAddress::Ext(ext) => Ok(ext.addr_pfx()),
            _ => fail!("Unsupported address type"),
        }
    }

    pub fn is_zero(&self) -> bool {
        match self {
            IntermediateAddress::Simple(simple) => simple.addr_pfx() == 0,
            IntermediateAddress::Ext(ext) => ext.addr_pfx() == 0,
            IntermediateAddress::Regular(reg) => reg.use_dest_bits == 0,
        }
    }
}

impl Default for IntermediateAddress {
    fn default() -> Self {
        IntermediateAddress::full_src()
    }
}

impl PartialOrd<u8> for IntermediateAddress {
    fn partial_cmp(&self, other: &u8) -> Option<Ordering> {
        match self {
            IntermediateAddress::Regular(ia) => Some(ia.use_dest_bits.cmp(other)),
            _ => None,
        }
    }
}

impl PartialEq<u8> for IntermediateAddress {
    fn eq(&self, other: &u8) -> bool {
        match self {
            IntermediateAddress::Regular(ia) => &ia.use_dest_bits == other,
            _ => false,
        }
    }
}

impl Serializable for IntermediateAddress {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            IntermediateAddress::Regular(addr) => {
                cell.append_raw(&[0b00000000], 1)?; // tag = $0
                addr.write_to(cell)?;
            }
            IntermediateAddress::Simple(addr) => {
                cell.append_raw(&[0b10000000], 2)?; // tag = $10
                addr.write_to(cell)?;
            }
            IntermediateAddress::Ext(addr) => {
                cell.append_raw(&[0b11000000], 2)?; // tag = $11
                addr.write_to(cell)?;
            }
        };
        Ok(())
    }
}

impl Deserializable for IntermediateAddress {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = if cell.get_next_bit()? {
            if cell.get_next_bit()? {
                // tag = 11
                let mut addr = IntermediateAddressExt::default();
                addr.read_from(cell)?;
                IntermediateAddress::Ext(addr)
            } else {
                // tag = $10
                let mut addr = IntermediateAddressSimple::default();
                addr.read_from(cell)?;
                IntermediateAddress::Simple(addr)
            }
        } else {
            // tag = $0
            let mut addr = IntermediateAddressRegular::default();
            addr.read_from(cell)?;
            IntermediateAddress::Regular(addr)
        };

        Ok(())
    }
}

/////////////////////////////////////////////////////////////////
///
/// interm_addr_regular$0 use_dest_bits:(#<= 96) = IntermediateAddress;
///

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct IntermediateAddressRegular {
    use_dest_bits: u8,
}

pub static FULL_BITS: u8 = 96;

impl IntermediateAddressRegular {
    pub fn with_use_src_bits(use_src_bits: u8) -> Result<Self> {
        if use_src_bits > FULL_BITS {
            fail!(BlockError::InvalidArg(format!("use_src_bits must be <= {}", FULL_BITS)))
        }
        Ok(IntermediateAddressRegular { use_dest_bits: FULL_BITS - use_src_bits })
    }

    pub fn with_use_dest_bits(use_dest_bits: u8) -> Result<Self> {
        if use_dest_bits > FULL_BITS {
            fail!(BlockError::InvalidArg(format!("use_dest_bits must be <= {}", FULL_BITS)))
        }
        Ok(IntermediateAddressRegular { use_dest_bits })
    }

    pub fn use_src_bits(&self) -> u8 {
        FULL_BITS - self.use_dest_bits
    }

    pub fn use_dest_bits(&self) -> u8 {
        self.use_dest_bits
    }

    pub fn set_use_src_bits(&mut self, use_src_bits: u8) -> Result<()> {
        if use_src_bits > FULL_BITS {
            fail!(BlockError::InvalidArg(format!("use_src_bits must be <= {}", FULL_BITS)))
        }
        self.use_dest_bits = FULL_BITS - use_src_bits;
        Ok(())
    }
}

impl Serializable for IntermediateAddressRegular {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        // write 7-bit from use_dest_bits
        cell.append_raw(&[self.use_dest_bits << 1], 7)?;
        Ok(())
    }
}

impl Deserializable for IntermediateAddressRegular {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.use_dest_bits = cell.get_next_bits(7)?[0] >> 1; // read 7 bit into use_dest_bits
        if self.use_dest_bits > FULL_BITS {
            fail!(BlockError::InvalidArg(format!("use_dest_bits must be <= {}", FULL_BITS)))
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////
///
/// interm_addr_simple$10 workchain_id:int8 addr_pfx:(64 * Bit) = IntermediateAddress;
///

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct IntermediateAddressSimple {
    pub workchain_id: i8,
    pub addr_pfx: u64,
}

impl IntermediateAddressSimple {
    pub const fn with_addr(workchain_id: i8, addr_pfx: u64) -> Self {
        Self { workchain_id, addr_pfx }
    }

    pub const fn workchain_id(&self) -> i8 {
        self.workchain_id
    }

    pub const fn addr_pfx(&self) -> u64 {
        self.addr_pfx
    }

    pub fn set_workchain_id(&mut self, workchain_id: i8) {
        self.workchain_id = workchain_id;
    }

    pub fn set_addr_pfx(&mut self, addr_pfx: u64) {
        self.addr_pfx = addr_pfx;
    }
}

impl Serializable for IntermediateAddressSimple {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.workchain_id.write_to(cell)?;
        self.addr_pfx.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for IntermediateAddressSimple {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.workchain_id.read_from(cell)?;
        self.addr_pfx.read_from(cell)?;
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////
///
/// interm_addr_ext$11 workchain_id:int32 addr_pfx:(64 * Bit) = IntermediateAddress;
///

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct IntermediateAddressExt {
    pub workchain_id: i32,
    pub addr_pfx: u64,
}

impl IntermediateAddressExt {
    pub const fn with_addr(workchain_id: i32, addr_pfx: u64) -> Self {
        Self { workchain_id, addr_pfx }
    }

    pub const fn workchain_id(&self) -> i32 {
        self.workchain_id
    }

    pub const fn addr_pfx(&self) -> u64 {
        self.addr_pfx
    }

    pub fn set_workchain_id(&mut self, workchain_id: i32) {
        self.workchain_id = workchain_id;
    }

    pub fn set_addr_pfx(&mut self, addr_pfx: u64) {
        self.addr_pfx = addr_pfx;
    }
}

impl Serializable for IntermediateAddressExt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.workchain_id.write_to(cell)?;
        self.addr_pfx.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for IntermediateAddressExt {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.workchain_id.read_from(cell)?;
        self.addr_pfx.read_from(cell)?;
        Ok(())
    }
}

const MSG_METADATA_TAG: usize = 0x0;

// msg_metadata#0 depth:uint32 initiator_addr:MsgAddressInt initiator_lt:uint64 = MsgMetadata;
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct MsgMetadata {
    depth: u32,
    initiator_addr: MsgAddressInt,
    initiator_lt: u64,
}

impl MsgMetadata {
    pub fn new(initiator_addr: MsgAddressInt, initiator_lt: u64) -> Self {
        MsgMetadata { depth: 0, initiator_addr, initiator_lt }
    }

    pub fn depth(&self) -> u32 {
        self.depth
    }

    pub fn initiator_addr(&self) -> &MsgAddressInt {
        &self.initiator_addr
    }

    pub fn initiator_lt(&self) -> u64 {
        self.initiator_lt
    }

    pub fn initiator(&self) -> (i32, SliceData, u64) {
        (
            self.initiator_addr.workchain_id(),
            self.initiator_addr.address().clone(),
            self.initiator_lt,
        )
    }

    pub fn update_initiator_lt(&mut self, lt: u64) {
        if self.initiator_lt == 0 {
            self.initiator_lt = lt;
        }
    }

    fn add_depth(&self) -> Self {
        Self {
            depth: self.depth + 1,
            initiator_addr: self.initiator_addr.clone(),
            initiator_lt: self.initiator_lt,
        }
    }
}

impl Serializable for MsgMetadata {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(MSG_METADATA_TAG, 4)?;
        self.depth.write_to(cell)?;
        self.initiator_addr.write_to(cell)?;
        self.initiator_lt.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for MsgMetadata {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_int(4)? as usize;
        if tag != MSG_METADATA_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.depth.read_from(cell)?;
        self.initiator_addr.read_from(cell)?;
        self.initiator_lt.read_from(cell)?;
        Ok(())
    }
}

const MSG_ENVELOPE_TAG: usize = 0x4;
const MSG_ENVELOPE_TAG2: usize = 0x5;

// msg_envelope#4
//   cur_addr:IntnveloMsgEnvelope;
//   next_addr:IntermediateAddress fwd_fee_remaining:Coins
//   msg:^(Message Any)
// msg_envelope#5
//   cur_addr:IntnveloMsgEnvelope;
//   next_addr:IntermediateAddress fwd_fee_remaining:Coins
//   msg:^(Message Any)
//   emitted_lt:(Maybe uint64)
//   metadata:(Maybe MsgMetadata) = MsgEnvelope;
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct MsgEnvelope {
    cur_addr: IntermediateAddress,
    next_addr: IntermediateAddress,
    fwd_fee_remaining: Coins,
    msg: ChildCell<Message>,
    emitted_lt: u64,
    metadata: Option<MsgMetadata>,
}

impl MsgEnvelope {
    ///
    /// Create Envelope with message and remainig_fee
    ///
    #[cfg(test)]
    pub fn with_message_and_fee(msg: &Message, fwd_fee_remaining: Coins) -> Result<Self> {
        if !msg.is_internal() {
            fail!("MsgEnvelope can be made only for internal messages");
        }
        Ok(Self::with_routing(
            ChildCell::with_struct(msg)?,
            fwd_fee_remaining,
            IntermediateAddress::full_dest(),
            IntermediateAddress::full_dest(),
            1,
            Default::default(),
        ))
    }

    ///
    /// Create Envelope with message and remainig_fee and routing settings
    ///
    pub fn with_routing(
        msg: ChildCell<Message>,
        fwd_fee_remaining: Coins,
        cur_addr: IntermediateAddress,
        next_addr: IntermediateAddress,
        emitted_lt: u64,
        metadata: Option<MsgMetadata>,
    ) -> Self {
        MsgEnvelope { cur_addr, next_addr, fwd_fee_remaining, msg, emitted_lt, metadata }
    }

    ///
    /// Create Envelope with hypercube routing params
    ///
    pub fn hypercube_routing(
        msg: &Message,
        src_shard: &ShardIdent,
        fwd_fee_remaining: Coins,
    ) -> Result<Self> {
        let msg_cell = msg.serialize()?;
        let src = msg.src_ref().ok_or_else(|| {
            error!("source address of message {:x} is invalid", msg_cell.repr_hash())
        })?;
        let src_prefix = AccountIdPrefixFull::prefix(src)?;
        let dst = msg.dst_ref().ok_or_else(|| {
            error!("destination address of message {:x} is invalid", msg_cell.repr_hash())
        })?;
        let dst_prefix = AccountIdPrefixFull::prefix(dst)?;
        let ia = IntermediateAddress::default();
        let route_info = src_prefix.perform_hypercube_routing(&dst_prefix, src_shard, ia)?;
        Ok(MsgEnvelope {
            cur_addr: route_info.0,
            next_addr: route_info.1,
            fwd_fee_remaining,
            msg: ChildCell::with_cell(msg_cell),
            emitted_lt: 0,
            metadata: None,
        })
    }

    /// calc prefixes with routing info
    pub fn calc_cur_next_prefix(&self) -> Result<(AccountIdPrefixFull, AccountIdPrefixFull)> {
        let msg = self.read_message()?;
        let src = msg.src_ref().ok_or_else(|| {
            error!("source address of message {:x} is invalid", self.message_hash())
        })?;
        let src_prefix = AccountIdPrefixFull::prefix(src)?;
        let dst = msg.dst_ref().ok_or_else(|| {
            error!("destination address of message {:x} is invalid", self.message_hash())
        })?;
        let dst_prefix = AccountIdPrefixFull::prefix(dst)?;

        let cur_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, &self.cur_addr)?;
        let next_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, &self.next_addr)?;
        Ok((cur_prefix, next_prefix))
    }

    ///
    /// Read message struct from envelope
    ///
    pub fn read_message(&self) -> Result<Message> {
        self.msg.read_struct()
    }

    ///
    /// Return message cell from envelope
    ///
    pub fn message_cell(&self) -> Cell {
        self.msg.cell()
    }

    pub fn message(&self) -> &ChildCell<Message> {
        &self.msg
    }

    ///
    /// Return message hash from envelope
    ///
    pub fn message_hash(&self) -> UInt256 {
        self.msg.hash()
    }

    ///
    /// Get created lt
    ///
    pub fn created_lt(&self) -> Result<u64> {
        self.read_message()?
            .created_lt()
            .ok_or_else(|| error!("wrong message type {:x}", self.message_hash()))
    }

    ///
    /// Get remaining fee of envelope
    ///
    pub fn fwd_fee_remaining(&self) -> &Coins {
        &self.fwd_fee_remaining
    }

    ///
    /// Collect transfer fee from envelope
    ///
    pub fn collect_fee(&mut self, fee: Coins) -> bool {
        self.fwd_fee_remaining.sub(&fee).unwrap() // no excpetion here
    }

    #[cfg(test)]
    pub(crate) fn set_cur_addr(&mut self, addr: IntermediateAddress) -> &mut Self {
        self.cur_addr = addr;
        self
    }

    #[cfg(test)]
    pub(crate) fn set_next_addr(&mut self, addr: IntermediateAddress) -> &mut Self {
        self.next_addr = addr;
        self
    }

    ///
    /// Get current address
    ///
    pub fn cur_addr(&self) -> &IntermediateAddress {
        &self.cur_addr
    }

    ///
    /// Get next address
    ///
    pub fn next_addr(&self) -> &IntermediateAddress {
        &self.next_addr
    }

    /// is message route in one workchain
    pub fn same_workchain(&self) -> Result<bool> {
        let msg = self.read_message()?;
        debug_assert!(
            msg.is_internal(),
            "Message with hash {:x} is not internal",
            self.message_cell().repr_hash()
        );
        if let (Some(src), Some(dst)) = (msg.src_ref(), msg.dst_ref()) {
            return Ok(src.workchain_id() == dst.workchain_id());
        }
        fail!(
            "Message with hash {:x} has wrong type of src/dst address",
            self.message_cell().repr_hash()
        )
    }

    pub const fn metadata(&self) -> Option<&MsgMetadata> {
        self.metadata.as_ref()
    }

    pub fn metadata_add_depth(&self) -> Option<MsgMetadata> {
        self.metadata.as_ref().map(|msg_metadata| msg_metadata.add_depth())
    }

    pub fn set_metadata(&mut self, emitted_lt: u64, metadata: Option<MsgMetadata>) {
        self.emitted_lt = emitted_lt;
        self.metadata = metadata;
    }

    pub const fn emitted_lt(&self) -> u64 {
        self.emitted_lt
    }

    pub fn clear_emitted_lt(&mut self) {
        self.emitted_lt = 0;
    }

    pub fn set_emitted_lt(&mut self, emitted_lt: u64) {
        self.emitted_lt = emitted_lt;
    }

    pub fn clear_routing(&mut self) {
        self.cur_addr = Default::default();
        self.next_addr = Default::default();
    }
}

impl Serializable for MsgEnvelope {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let tag = if self.emitted_lt == 0 && self.metadata.is_none() {
            MSG_ENVELOPE_TAG
        } else {
            MSG_ENVELOPE_TAG2
        };
        cell.append_bits(tag, 4)?;
        self.cur_addr.write_to(cell)?;
        self.next_addr.write_to(cell)?;
        self.fwd_fee_remaining.write_to(cell)?;
        cell.checked_append_reference(self.msg.cell())?;
        if tag == MSG_ENVELOPE_TAG2 {
            self.emitted_lt.write_maybe_to(cell, self.emitted_lt != 0)?;
            self.metadata.write_to(cell)?;
        }
        Ok(())
    }
}

impl Deserializable for MsgEnvelope {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_int(4)? as usize;
        if tag != MSG_ENVELOPE_TAG && tag != MSG_ENVELOPE_TAG2 {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.cur_addr.read_from(cell)?;
        self.next_addr.read_from(cell)?;
        self.fwd_fee_remaining.read_from(cell)?;
        let msg_cell = cell.checked_drain_reference()?;
        self.msg = ChildCell::with_cell(msg_cell);
        if tag == MSG_ENVELOPE_TAG2 {
            self.emitted_lt.read_maybe_from(cell)?;
            self.metadata.read_from(cell)?;
        } else {
            self.emitted_lt = 0;
            self.metadata = None;
        }
        Ok(())
    }
}
