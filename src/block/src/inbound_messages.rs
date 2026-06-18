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
//! # message descriptors
//!
//! Implementation of blockchain spec (3.2) structs: InMsg and InMsgDescr.
//! Serialization and deserialization of this structs.

use crate::{
    define_HashmapAugE,
    dictionary::hashmapaug::{Augmentable, Augmentation, HashmapAugType},
    envelope_message::MsgEnvelope,
    error,
    error::BlockError,
    fail,
    messages::Message,
    transactions::Transaction,
    types::{AddSub, ChildCell, Coins, CurrencyCollection},
    BuilderData, Cell, Deserializable, IBitstring, Result, Serializable, SliceData, UInt256,
};
use std::fmt;

#[cfg(test)]
#[path = "tests/test_in_msgs.rs"]
mod tests;

///internal helper macros for reading InMsg variants
macro_rules! read_descr {
    ($slice:expr, $variant:ident) => {{
        InMsg::$variant(Deserializable::construct_from($slice)?)
    }};
}

///internal helper macros for writing constructor tags in InMsg variants
macro_rules! write_tag {
    ($builder:expr, $tag:ident, $tag_len:expr) => {{
        $builder.append_bits($tag as usize, $tag_len).unwrap();
        $builder
    }};
}

//3.2.7. Augmentation of InMsgDescr
#[derive(Default, PartialEq, Eq, Clone, Debug)]
pub struct ImportFees {
    pub fees_collected: Coins,
    pub value_imported: CurrencyCollection,
}

impl Augmentable for ImportFees {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        let mut result = self.fees_collected.calc(&other.fees_collected)?;
        result |= self.value_imported.calc(&other.value_imported)?;
        Ok(result)
    }
}

impl ImportFees {
    pub fn with_coins(coins: u64) -> Self {
        Self { fees_collected: Coins::from(coins), value_imported: CurrencyCollection::default() }
    }
}

impl Serializable for ImportFees {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.fees_collected.write_to(cell)?;
        self.value_imported.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for ImportFees {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.fees_collected.read_from(cell)?;
        self.value_imported.read_from(cell)?;
        Ok(())
    }
}

//constructor tags of InMsg variants (only 3 bits are used)
const MSG_IMPORT_EXT: u8 = 0b000;
const MSG_IMPORT_IHR: u8 = 0b010;
const MSG_IMPORT_IMM: u8 = 0b011;
const MSG_IMPORT_FIN: u8 = 0b100;
const MSG_IMPORT_TR: u8 = 0b101;
const MSG_DISCARD_FIN: u8 = 0b110;
const MSG_DISCARD_TR: u8 = 0b111;
const MSG_DEFERRED_FIN: u8 = 0b00100;
const MSG_DEFERRED_TR: u8 = 0b00101;

///
/// Inbound message
/// blockchain spec 3.2.2. Descriptor of an inbound message.
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum InMsg {
    #[default]
    None,
    /// Inbound external messages
    /// msg_import_ext$000 msg:^(Message Any) transaction:^Transaction = InMsg;
    External(InMsgExternal),
    /// Internal IHR messages with destination addresses in this block
    /// msg_import_ihr$010 msg:^(Message Any) transaction:^Transaction ihr_fee:Coins proof_created:^Cell = InMsg;
    IHR(InMsgIHR),
    /// Internal messages with destinations in this block
    /// msg_import_imm$011 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins = InMsg;
    Immediate(InMsgFinal),
    /// Immediately routed internal messages
    /// msg_import_fin$100 in_msg:^MsgEnvelope transaction:^Transaction fwd_fee:Coins = InMsg;
    Final(InMsgFinal),
    /// Transit internal messages
    /// msg_import_tr$101  in_msg:^MsgEnvelope out_msg:^MsgEnvelope transit_fee:Coins = InMsg;
    Transit(InMsgTransit),
    /// Discarded internal messages with destinations in this block
    /// msg_discard_fin$110 in_msg:^MsgEnvelope transaction_id:uint64 fwd_fee:Coins = InMsg;
    DiscardedFinal(InMsgDiscardedFinal),
    /// Discarded transit internal messages
    /// msg_discard_tr$111 in_msg:^MsgEnvelope transaction_id:uint64 fwd_fee:Coins proof_delivered:^Cell = InMsg;
    DiscardedTransit(InMsgDiscardedTransit),
    /// msg_import_deferred_fin$00100 Deferred internal messages with destinations in this block
    DeferredFinal(InMsgDeferredFinal),
    /// msg_import_deferred_tr$00101 Deferred transit internal messages
    DeferredTransit(InMsgDeferredTransit),
}

impl fmt::Display for InMsg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let msg_cell = self.message_cell().unwrap_or_default();
        let msg_hash = msg_cell.repr_hash();
        let tr_cell = self.transaction_cell().unwrap_or_default();
        let tr_hash = tr_cell.repr_hash();
        match self {
            InMsg::External(_x) => {
                write!(f, "InMsg msg_import_ext$000 msg: {:x} tr: {:x}", msg_hash, tr_hash)
            }
            InMsg::IHR(_x) => {
                write!(f, "InMsg msg_import_ihr$010 msg: {:x} tr: {:x}", msg_hash, tr_hash)
            }
            InMsg::Immediate(x) => write!(
                f,
                "InMsg msg_import_imm$011 msg: {:x} tr: {:x} fee: {}",
                msg_hash, tr_hash, x.fwd_fee
            ),
            InMsg::Transit(x) => write!(
                f,
                "InMsg msg_import_tr$101 in_msg: {:x} out_msg: {:x} fee: {}",
                msg_hash,
                x.out_msg.read_struct().unwrap_or_default().message_hash(),
                x.transit_fee
            ),
            InMsg::Final(x) => write!(
                f,
                "InMsg msg_import_fin$100 msg: {:x} tr: {:x} fee: {}",
                msg_hash, tr_hash, x.fwd_fee
            ),
            InMsg::DiscardedFinal(x) => write!(
                f,
                "InMsg msg_discard_fin$110 msg: {:x} tr: {} fee: {}",
                msg_hash, x.transaction_id, x.fwd_fee
            ),
            InMsg::DiscardedTransit(x) => write!(
                f,
                "InMsg msg_discard_tr$111 msg: {:x} tr: {:x} fee: {} proof: {:x}",
                msg_hash,
                x.transaction_id,
                x.fwd_fee,
                x.proof_delivered.repr_hash()
            ),
            InMsg::DeferredFinal(x) => write!(
                f,
                "InMsg msg_import_deferred_fin$00100 msg: {:x} tr: {:x} fee: {}",
                msg_hash, tr_hash, x.fwd_fee
            ),
            InMsg::DeferredTransit(x) => write!(
                f,
                "InMsg msg_import_deferred_tr$00101 in_msg: {:x} out_msg: {:x}",
                msg_hash,
                x.out_envelope_message_hash()
            ),
            InMsg::None => write!(f, "InMsg msg_unknown"),
        }
    }
}

impl InMsg {
    /// Create external
    pub fn external(msg_cell: ChildCell<Message>, tr_cell: ChildCell<Transaction>) -> InMsg {
        InMsg::External(InMsgExternal::with_cells(msg_cell, tr_cell))
    }
    pub fn is_external(&self) -> bool {
        matches!(self, InMsg::External(_))
    }
    /// Create IHR
    pub fn ihr(
        msg_cell: ChildCell<Message>,
        tr_cell: ChildCell<Transaction>,
        ihr_fee: Coins,
        proof: Cell,
    ) -> InMsg {
        InMsg::IHR(InMsgIHR::with_cells(msg_cell, tr_cell, ihr_fee, proof))
    }
    /// Create Immediate
    pub fn immediate(
        env_cell: ChildCell<MsgEnvelope>,
        tr_cell: ChildCell<Transaction>,
        fwd_fee: Coins,
    ) -> InMsg {
        InMsg::Immediate(InMsgFinal::with_cells(env_cell, tr_cell, fwd_fee))
    }
    /// Create Final
    pub fn final_msg(
        env_cell: ChildCell<MsgEnvelope>,
        tr_cell: ChildCell<Transaction>,
        fwd_fee: Coins,
    ) -> InMsg {
        InMsg::Final(InMsgFinal::with_cells(env_cell, tr_cell, fwd_fee))
    }
    /// Create Transit
    pub fn transit(
        in_msg_cell: ChildCell<MsgEnvelope>,
        out_msg_cell: ChildCell<MsgEnvelope>,
        fwd_fee: Coins,
    ) -> InMsg {
        InMsg::Transit(InMsgTransit::with_cells(in_msg_cell, out_msg_cell, fwd_fee))
    }
    /// Create DiscardedFinal
    pub fn discarded_final(env_cell: ChildCell<MsgEnvelope>, tr_id: u64, fwd_fee: Coins) -> InMsg {
        InMsg::DiscardedFinal(InMsgDiscardedFinal::with_cells(env_cell, tr_id, fwd_fee))
    }
    /// Create DiscardedTransit
    pub fn discarded_transit(
        env_cell: ChildCell<MsgEnvelope>,
        tr_id: u64,
        fwd_fee: Coins,
        proof: Cell,
    ) -> InMsg {
        InMsg::DiscardedTransit(InMsgDiscardedTransit::with_cells(env_cell, tr_id, fwd_fee, proof))
    }
    /// Create Deferred Final
    pub fn deferred_final(
        env_cell: ChildCell<MsgEnvelope>,
        tr_cell: ChildCell<Transaction>,
        fwd_fee: Coins,
    ) -> InMsg {
        InMsg::DeferredFinal(InMsgDeferredFinal::with_cells(env_cell, tr_cell, fwd_fee))
    }
    /// Create Deferred Transit
    pub fn deferred_transit(
        in_msg_cell: ChildCell<MsgEnvelope>,
        out_msg_cell: ChildCell<MsgEnvelope>,
    ) -> InMsg {
        InMsg::DeferredTransit(InMsgDeferredTransit::with_cells(in_msg_cell, out_msg_cell))
    }

    /// Check if is valid message
    pub fn is_valid(&self) -> bool {
        self != &InMsg::None
    }

    pub fn tag(&self) -> u8 {
        match self {
            InMsg::External(_) => MSG_IMPORT_EXT,
            InMsg::IHR(_) => MSG_IMPORT_IHR,
            InMsg::Immediate(_) => MSG_IMPORT_IMM,
            InMsg::Final(_) => MSG_IMPORT_FIN,
            InMsg::Transit(_) => MSG_IMPORT_TR,
            InMsg::DiscardedFinal(_) => MSG_DISCARD_FIN,
            InMsg::DiscardedTransit(_) => MSG_DISCARD_TR,
            InMsg::DeferredFinal(_) => MSG_DEFERRED_FIN,
            InMsg::DeferredTransit(_) => MSG_DEFERRED_TR,
            InMsg::None => 8,
        }
    }

    ///
    /// Get transaction from inbound message
    /// Transaction exist only in External, IHR, Immediate and Final inbound messages.
    /// For other messages function returned None
    ///
    pub fn read_transaction(&self) -> Result<Option<Transaction>> {
        let trans_opt = match self {
            InMsg::External(ref x) => Some(x.read_transaction()?),
            InMsg::IHR(ref x) => Some(x.read_transaction()?),
            InMsg::Immediate(ref x) => Some(x.read_transaction()?),
            InMsg::Final(ref x) => Some(x.read_transaction()?),
            InMsg::Transit(ref _x) => None,
            InMsg::DiscardedFinal(ref _x) => None,
            InMsg::DiscardedTransit(ref _x) => None,
            InMsg::DeferredFinal(x) => Some(x.read_transaction()?),
            InMsg::DeferredTransit(_) => None,
            InMsg::None => fail!("wrong message type"),
        };
        Ok(trans_opt)
    }

    ///
    /// Get transaction cell from inbound message
    /// Transaction exist only in External, IHR, Immediate and Final inbound messages.
    /// For other messages function returned None
    ///
    pub fn transaction_cell(&self) -> Option<Cell> {
        match self {
            InMsg::External(x) => Some(x.transaction_cell()),
            InMsg::IHR(x) => Some(x.transaction_cell()),
            InMsg::Immediate(x) => Some(x.transaction_cell()),
            InMsg::Final(x) => Some(x.transaction_cell()),
            InMsg::DeferredFinal(x) => Some(x.transaction_cell()),
            _ => None,
        }
    }

    ///
    /// Get message
    ///
    pub fn read_message(&self) -> Result<Message> {
        match self {
            InMsg::External(x) => x.read_message(),
            InMsg::IHR(x) => x.read_message(),
            InMsg::Immediate(x) => x.read_envelope_message()?.read_message(),
            InMsg::Final(x) => x.read_envelope_message()?.read_message(),
            InMsg::Transit(x) => x.read_in_message()?.read_message(),
            InMsg::DiscardedFinal(x) => x.read_envelope_message()?.read_message(),
            InMsg::DiscardedTransit(x) => x.read_envelope_message()?.read_message(),
            InMsg::DeferredFinal(x) => x.read_envelope_message()?.read_message(),
            InMsg::DeferredTransit(x) => x.read_in_envelope_message()?.read_message(),
            InMsg::None => fail!("wrong msg type"),
        }
    }

    ///
    /// Get message cell
    ///
    pub fn message_cell(&self) -> Result<Cell> {
        let msg_cell = match self {
            InMsg::External(x) => x.message_cell(),
            InMsg::IHR(x) => x.message_cell(),
            InMsg::Immediate(x) => x.read_envelope_message()?.message_cell(),
            InMsg::Final(x) => x.read_envelope_message()?.message_cell(),
            InMsg::Transit(x) => x.read_in_message()?.message_cell(),
            InMsg::DiscardedFinal(x) => x.read_envelope_message()?.message_cell(),
            InMsg::DiscardedTransit(x) => x.read_envelope_message()?.message_cell(),
            InMsg::DeferredFinal(x) => x.read_envelope_message()?.message_cell(),
            InMsg::DeferredTransit(x) => x.read_in_envelope_message()?.message_cell(),
            InMsg::None => fail!("wrong message type"),
        };
        Ok(msg_cell)
    }

    ///
    /// Get in envelope message cell
    ///
    pub fn in_msg_envelope_cell(&self) -> Option<Cell> {
        match self {
            InMsg::External(_) => None,
            InMsg::IHR(_) => None,
            InMsg::Immediate(x) => Some(x.envelope_message_cell()),
            InMsg::Final(x) => Some(x.envelope_message_cell()),
            InMsg::Transit(x) => Some(x.in_msg.cell()),
            InMsg::DiscardedFinal(x) => Some(x.envelope_message_cell()),
            InMsg::DiscardedTransit(x) => Some(x.in_msg.cell()),
            InMsg::DeferredFinal(x) => Some(x.envelope_message_cell()),
            InMsg::DeferredTransit(x) => Some(x.in_envelope_message_cell()),
            InMsg::None => None,
        }
    }

    ///
    /// Get in envelope message
    ///
    pub fn read_in_msg_envelope(&self) -> Result<Option<MsgEnvelope>> {
        Ok(match self {
            InMsg::External(_) => None,
            InMsg::IHR(_) => None,
            InMsg::Immediate(x) => Some(x.read_envelope_message()?),
            InMsg::Final(x) => Some(x.read_envelope_message()?),
            InMsg::Transit(x) => Some(x.read_in_message()?),
            InMsg::DiscardedFinal(x) => Some(x.read_envelope_message()?),
            InMsg::DiscardedTransit(x) => Some(x.read_envelope_message()?),
            InMsg::DeferredFinal(x) => Some(x.read_envelope_message()?),
            InMsg::DeferredTransit(x) => Some(x.read_in_envelope_message()?),
            InMsg::None => fail!("wrong message type"),
        })
    }

    ///
    /// Get transit out envelope message cell
    ///
    pub fn out_msg_envelope_cell(&self) -> Option<Cell> {
        match self {
            InMsg::Transit(x) => Some(x.out_msg.cell()),
            InMsg::DeferredTransit(x) => Some(x.out_msg.cell()),
            _ => None,
        }
    }

    ///
    /// Get transit out envelope message
    ///
    pub fn read_out_msg_envelope(&self) -> Result<Option<MsgEnvelope>> {
        let env = match self {
            InMsg::Transit(x) => x.read_out_message()?,
            InMsg::DeferredTransit(x) => x.read_out_envelope_message()?,
            _ => return Ok(None),
        };
        Ok(Some(env))
    }

    pub fn get_fee(&self) -> Result<ImportFees> {
        self.aug()
    }
}

impl Augmentation<ImportFees> for InMsg {
    fn aug(&self) -> Result<ImportFees> {
        let msg = self.read_message()?;
        let header = match msg.int_header() {
            Some(header) => header,
            None => return Ok(ImportFees::default()),
        };
        let mut fees = ImportFees::default();
        match self {
            InMsg::External(_) => {
                //println!("InMsg::External");
            }
            InMsg::IHR(_) => {
                //println!("InMsg::IHR");

                fees.value_imported = header.value.clone();
            }
            InMsg::Immediate(_) => {
                //println!("InMsg::Immediate");
                fees.fees_collected = header.fwd_fee;
            }
            InMsg::Final(x) => {
                //println!("InMsg::Final");
                let env = x.read_envelope_message()?;
                if env.fwd_fee_remaining() != x.fwd_fee() {
                    fail!("fwd_fee_remaining not equal to fwd_fee")
                }
                fees.fees_collected = *env.fwd_fee_remaining();

                fees.value_imported = header.value.clone();
                fees.value_imported.coins.add(env.fwd_fee_remaining())?;
            }
            InMsg::Transit(x) => {
                //println!("InMsg::Transit");
                let env = x.read_in_message()?;
                if env.fwd_fee_remaining() < x.transit_fee() {
                    fail!("fwd_fee_remaining less than transit_fee")
                }

                fees.fees_collected = *x.transit_fee();

                fees.value_imported = header.value.clone();
                fees.value_imported.coins.add(env.fwd_fee_remaining())?;
            }
            InMsg::DiscardedFinal(_) => {
                //println!("InMsg::DiscardedFinal");
                fees.fees_collected = header.fwd_fee;

                fees.value_imported.coins = header.fwd_fee;
            }
            InMsg::DiscardedTransit(_) => {
                //println!("InMsg::DiscardedTransit");
                fees.fees_collected = header.fwd_fee;

                fees.value_imported.coins = header.fwd_fee;
            }
            InMsg::DeferredFinal(x) => {
                let env = x.read_envelope_message()?;
                if env.fwd_fee_remaining() != x.fwd_fee() {
                    fail!("fwd_fee_remaining not equal to fwd_fee")
                }
                fees.fees_collected = *env.fwd_fee_remaining();

                fees.value_imported = header.value.clone();
                fees.value_imported.coins.add(env.fwd_fee_remaining())?;
            }
            InMsg::DeferredTransit(x) => {
                let env = x.read_in_envelope_message()?;
                fees.value_imported = header.value.clone();
                fees.value_imported.coins.add(env.fwd_fee_remaining())?;
            }
            InMsg::None => fail!("wrong InMsg type"),
        }
        Ok(fees)
    }
}

impl Serializable for InMsg {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            InMsg::External(x) => x.write_to(write_tag!(cell, MSG_IMPORT_EXT, 3)),
            InMsg::IHR(x) => x.write_to(write_tag!(cell, MSG_IMPORT_IHR, 3)),
            InMsg::Immediate(x) => x.write_to(write_tag!(cell, MSG_IMPORT_IMM, 3)),
            InMsg::Final(x) => x.write_to(write_tag!(cell, MSG_IMPORT_FIN, 3)),
            InMsg::Transit(x) => x.write_to(write_tag!(cell, MSG_IMPORT_TR, 3)),
            InMsg::DiscardedFinal(x) => x.write_to(write_tag!(cell, MSG_DISCARD_FIN, 3)),
            InMsg::DiscardedTransit(x) => x.write_to(write_tag!(cell, MSG_DISCARD_TR, 3)),
            InMsg::DeferredFinal(x) => x.write_to(write_tag!(cell, MSG_DEFERRED_FIN, 5)),
            InMsg::DeferredTransit(x) => x.write_to(write_tag!(cell, MSG_DEFERRED_TR, 5)),
            InMsg::None => Ok(()), // Due to ChildCell it is need sometimes to serialize default InMsg
        }
    }
}

impl Deserializable for InMsg {
    fn construct_from(cell: &mut SliceData) -> Result<Self> {
        let mut tag = cell.get_next_int(3)?;
        let msg = match tag as u8 {
            MSG_IMPORT_EXT => read_descr!(cell, External),
            MSG_IMPORT_IHR => read_descr!(cell, IHR),
            MSG_IMPORT_IMM => read_descr!(cell, Immediate),
            MSG_IMPORT_FIN => read_descr!(cell, Final),
            MSG_IMPORT_TR => read_descr!(cell, Transit),
            MSG_DISCARD_FIN => read_descr!(cell, DiscardedFinal),
            MSG_DISCARD_TR => read_descr!(cell, DiscardedTransit),
            _ => {
                tag = (tag << 2) + cell.get_next_int(2)?;
                match tag as u8 {
                    MSG_DEFERRED_FIN => read_descr!(cell, DeferredFinal),
                    MSG_DEFERRED_TR => read_descr!(cell, DeferredTransit),
                    _ => fail!(Self::invalid_tag(tag as u32)),
                }
            }
        };
        Ok(msg)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgExternal {
    msg: ChildCell<Message>,
    transaction: ChildCell<Transaction>,
}

impl InMsgExternal {
    pub fn with_cells(msg_cell: ChildCell<Message>, tr_cell: ChildCell<Transaction>) -> Self {
        InMsgExternal { msg: msg_cell, transaction: tr_cell }
    }

    pub fn read_message(&self) -> Result<Message> {
        self.msg.read_struct()
    }

    pub fn message_cell(&self) -> Cell {
        self.msg.cell()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }
}

impl Serializable for InMsgExternal {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgExternal {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgIHR {
    msg: ChildCell<Message>,
    transaction: ChildCell<Transaction>,
    ihr_fee: Coins,
    proof_created: Cell,
}

impl InMsgIHR {
    pub fn with_cells(
        msg: ChildCell<Message>,
        transaction: ChildCell<Transaction>,
        ihr_fee: Coins,
        proof_created: Cell,
    ) -> Self {
        InMsgIHR { msg, transaction, ihr_fee, proof_created }
    }

    pub fn read_message(&self) -> Result<Message> {
        self.msg.read_struct()
    }

    pub fn message_cell(&self) -> Cell {
        self.msg.cell()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }

    pub fn ihr_fee(&self) -> &Coins {
        &self.ihr_fee
    }

    pub fn proof_created(&self) -> &Cell {
        &self.proof_created
    }
}

impl Serializable for InMsgIHR {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        self.msg.write_to(builder)?;
        self.transaction.write_to(builder)?;
        self.ihr_fee.write_to(builder)?;
        builder.checked_append_reference(self.proof_created.clone())?;
        Ok(())
    }
}

impl Deserializable for InMsgIHR {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        self.ihr_fee.read_from(slice)?;
        self.proof_created = slice.checked_drain_reference()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgFinal {
    in_msg: ChildCell<MsgEnvelope>,
    transaction: ChildCell<Transaction>,
    pub fwd_fee: Coins,
}

impl InMsgFinal {
    pub fn with_cells(
        in_msg: ChildCell<MsgEnvelope>,
        transaction: ChildCell<Transaction>,
        fwd_fee: Coins,
    ) -> Self {
        InMsgFinal { in_msg, transaction, fwd_fee }
    }

    pub fn read_envelope_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }

    pub fn fwd_fee(&self) -> &Coins {
        &self.fwd_fee
    }
}

impl Serializable for InMsgFinal {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        self.fwd_fee.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgFinal {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        self.fwd_fee.read_from(slice)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgTransit {
    in_msg: ChildCell<MsgEnvelope>,
    out_msg: ChildCell<MsgEnvelope>,
    pub transit_fee: Coins,
}

impl InMsgTransit {
    pub fn with_cells(
        in_msg: ChildCell<MsgEnvelope>,
        out_msg: ChildCell<MsgEnvelope>,
        fee: Coins,
    ) -> Self {
        InMsgTransit { in_msg, out_msg, transit_fee: fee }
    }

    pub fn read_in_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn read_out_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn in_envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn in_envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn out_envelope_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn transit_fee(&self) -> &Coins {
        &self.transit_fee
    }
}

impl Serializable for InMsgTransit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(cell)?;
        self.out_msg.write_to(cell)?;
        self.transit_fee.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgTransit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.out_msg.read_from(slice)?;
        self.transit_fee.read_from(slice)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgDiscardedFinal {
    in_msg: ChildCell<MsgEnvelope>,
    pub transaction_id: u64,
    pub fwd_fee: Coins,
}

impl InMsgDiscardedFinal {
    pub fn with_cells(in_msg: ChildCell<MsgEnvelope>, transaction_id: u64, fee: Coins) -> Self {
        InMsgDiscardedFinal { in_msg, transaction_id, fwd_fee: fee }
    }

    pub fn read_envelope_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn message_cell(&self) -> Result<Cell> {
        Ok(self.read_envelope_message()?.message_cell())
    }

    pub fn transaction_id(&self) -> u64 {
        self.transaction_id
    }

    pub fn fwd_fee(&self) -> &Coins {
        &self.fwd_fee
    }
}

impl Serializable for InMsgDiscardedFinal {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(cell)?;
        self.transaction_id.write_to(cell)?;
        self.fwd_fee.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgDiscardedFinal {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.transaction_id.read_from(slice)?;
        self.fwd_fee.read_from(slice)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgDiscardedTransit {
    in_msg: ChildCell<MsgEnvelope>,
    transaction_id: u64,
    fwd_fee: Coins,
    proof_delivered: Cell,
}

impl InMsgDiscardedTransit {
    pub fn with_cells(
        in_msg: ChildCell<MsgEnvelope>,
        transaction_id: u64,
        fee: Coins,
        proof: Cell,
    ) -> Self {
        InMsgDiscardedTransit { in_msg, transaction_id, fwd_fee: fee, proof_delivered: proof }
    }

    pub fn read_envelope_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn message_cell(&self) -> Result<Cell> {
        Ok(self.in_msg.read_struct()?.message_cell())
    }

    pub fn transaction_id(&self) -> u64 {
        self.transaction_id
    }

    pub fn fwd_fee(&self) -> &Coins {
        &self.fwd_fee
    }

    pub fn proof_delivered(&self) -> &Cell {
        &self.proof_delivered
    }
}

impl Serializable for InMsgDiscardedTransit {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(builder)?;
        self.transaction_id.write_to(builder)?;
        self.fwd_fee.write_to(builder)?;
        builder.checked_append_reference(self.proof_delivered.clone())?;
        Ok(())
    }
}

impl Deserializable for InMsgDiscardedTransit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.transaction_id.read_from(slice)?;
        self.fwd_fee.read_from(slice)?;
        self.proof_delivered = slice.checked_drain_reference()?;
        Ok(())
    }
}

// msg_import_deferred_fin$00100 in_msg:^MsgEnvelope
// transaction:^Transaction fwd_fee:Coins = InMsg;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgDeferredFinal {
    in_msg: ChildCell<MsgEnvelope>,
    transaction: ChildCell<Transaction>,
    pub fwd_fee: Coins,
}

impl InMsgDeferredFinal {
    pub fn with_cells(
        in_msg: ChildCell<MsgEnvelope>,
        transaction: ChildCell<Transaction>,
        fee: Coins,
    ) -> Self {
        InMsgDeferredFinal { in_msg, transaction, fwd_fee: fee }
    }

    pub fn read_envelope_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn message_cell(&self) -> Result<Cell> {
        Ok(self.read_envelope_message()?.message_cell())
    }

    pub fn read_transaction(&self) -> Result<Transaction> {
        self.transaction.read_struct()
    }

    pub fn transaction_cell(&self) -> Cell {
        self.transaction.cell()
    }

    pub fn fwd_fee(&self) -> &Coins {
        &self.fwd_fee
    }
}

impl Serializable for InMsgDeferredFinal {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(cell)?;
        self.transaction.write_to(cell)?;
        self.fwd_fee.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgDeferredFinal {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.transaction.read_from(slice)?;
        self.fwd_fee.read_from(slice)?;
        Ok(())
    }
}

// msg_import_deferred_tr$00101 in_msg:^MsgEnvelope out_msg:^MsgEnvelope = InMsg;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMsgDeferredTransit {
    in_msg: ChildCell<MsgEnvelope>,
    out_msg: ChildCell<MsgEnvelope>,
}

impl InMsgDeferredTransit {
    pub fn with_cells(in_msg: ChildCell<MsgEnvelope>, out_msg: ChildCell<MsgEnvelope>) -> Self {
        InMsgDeferredTransit { in_msg, out_msg }
    }

    pub fn read_in_envelope_message(&self) -> Result<MsgEnvelope> {
        self.in_msg.read_struct()
    }

    pub fn in_envelope_message_cell(&self) -> Cell {
        self.in_msg.cell()
    }

    pub fn in_envelope_message_hash(&self) -> UInt256 {
        self.in_msg.hash()
    }

    pub fn read_out_envelope_message(&self) -> Result<MsgEnvelope> {
        self.out_msg.read_struct()
    }

    pub fn out_envelope_message_cell(&self) -> Cell {
        self.out_msg.cell()
    }

    pub fn out_envelope_message_hash(&self) -> UInt256 {
        self.out_msg.hash()
    }
}

impl Serializable for InMsgDeferredTransit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.in_msg.write_to(cell)?;
        self.out_msg.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for InMsgDeferredTransit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.in_msg.read_from(slice)?;
        self.out_msg.read_from(slice)?;
        Ok(())
    }
}

//3.2.8. Structure of InMsgDescr
//_ (HashmapAugE 256 InMsg ImportFees) = InMsgDescr
define_HashmapAugE!(InMsgDescr, 256, UInt256, InMsg, ImportFees);

impl InMsgDescr {
    /// insert new or replace existing, key - hash of Message
    pub fn insert_with_key(&mut self, key: UInt256, in_msg: &InMsg) -> Result<()> {
        let aug = in_msg.aug()?;
        self.set(&key, in_msg, &aug)
    }

    /// insert new or replace existing
    pub fn insert(&mut self, in_msg: &InMsg) -> Result<()> {
        self.insert_with_key(in_msg.message_cell()?.repr_hash().clone(), in_msg)
    }

    /// insert or replace existion record
    /// use to improve speed
    pub fn insert_serialized(
        &mut self,
        key: &SliceData,
        msg_slice: &SliceData,
        fees: &ImportFees,
    ) -> Result<()> {
        if self.set_builder_serialized(key.clone(), &msg_slice.as_builder()?, fees).is_ok() {
            Ok(())
        } else {
            fail!(BlockError::Other("Error insert serialized message".to_string()))
        }
    }

    pub fn full_import_fees(&self) -> &ImportFees {
        self.root_extra()
    }
}
