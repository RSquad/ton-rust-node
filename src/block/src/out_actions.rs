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
    error::{BlockError, Error},
    fail,
    messages::Message,
    types::CurrencyCollection,
    BuilderData, Cell, Deserializable, IBitstring, Result, Serializable, SliceData, UInt256,
};
use std::collections::LinkedList;

pub const ACTION_SEND_MSG: u32 = 0x0ec3c86d;
pub const ACTION_SET_CODE: u32 = 0xad4de08e;
pub const ACTION_RESERVE: u32 = 0x36e6b809;
pub const ACTION_CHANGE_LIB: u32 = 0x26fa1dd4;

#[cfg(test)]
#[path = "tests/test_out_actions.rs"]
mod tests;

/*
out_list_empty$_ = OutList 0;
out_list$_ {n:#} prev:^(OutList n) action:OutAction = OutList (n+1);
action_reserve#ad4de08e = OutAction;
action_send_msg#0ec3c86d out_msg:^Message = OutAction;
action_set_code#ad4de08e new_code:^Cell = OutAction;
*/

///
/// List of output actions
///
pub type OutActions = LinkedList<OutAction>;

pub fn unpack_out_action_slices(mut cell: SliceData) -> Result<Vec<SliceData>> {
    let mut slices_rev = Vec::new();
    loop {
        if cell.remaining_references() == 0 {
            if cell.is_empty_cell() {
                break;
            }
            fail!("cell is not empty")
        }
        let prev_cell = cell.checked_drain_reference()?;
        slices_rev.push(cell);
        cell = SliceData::load_cell(prev_cell)?;
    }
    slices_rev.reverse();
    Ok(slices_rev)
}

pub fn deserialize_out_action_slices(
    action_slices: Vec<SliceData>,
) -> std::result::Result<Vec<OutAction>, (usize, Error)> {
    let mut parsed_actions = Vec::with_capacity(action_slices.len());
    for (i, mut action_slice) in action_slices.into_iter().enumerate() {
        match OutAction::construct_from(&mut action_slice) {
            Ok(action) => parsed_actions.push(action),
            Err(err) => return Err((i, err)),
        }
    }
    Ok(parsed_actions)
}

///
/// Implementation of Serializable for OutActions
///
impl Serializable for OutActions {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let mut builder = BuilderData::new();

        for action in self.iter() {
            let mut next_builder = BuilderData::new();

            next_builder.checked_append_reference(builder.into_cell()?)?;
            action.write_to(&mut next_builder)?;

            builder = next_builder;
        }

        cell.append_builder(&builder)?;
        Ok(())
    }
}

///
/// Implementation of Deserializable for OutActions
///
impl Deserializable for OutActions {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let actions = match deserialize_out_action_slices(unpack_out_action_slices(cell.clone())?) {
            Ok(actions) => actions,
            Err((_, err)) => return Err(err),
        };
        self.extend(actions);
        Ok(())
    }
}

///
/// Enum OutAction
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum OutAction {
    ///
    /// Action for send message
    ///
    SendMsg { mode: u8, out_msg: Message },

    ///
    /// Action for set new code of smart-contract
    ///
    SetCode { new_code: Cell },

    ///
    /// Action for reserving some account balance.
    /// It is roughly equivalent to creating an output
    /// message carrying x nanocoins to oneself,so that
    /// the subsequent output actions would not be able
    /// to spend more money than the remainder.
    ///
    ReserveCurrency { mode: u8, value: CurrencyCollection },

    ///
    /// Action for change library.
    ///
    ChangeLibrary { mode: u8, code: Option<Cell>, hash: Option<UInt256> },

    #[default]
    None,
}

/// Flags of SendMsg action
pub const SENDMSG_ORDINARY: u8 = 0;
pub const SENDMSG_PAY_FEE_SEPARATELY: u8 = 1;
pub const SENDMSG_IGNORE_ERROR: u8 = 2;
pub const SENDMSG_BOUNCE_IF_FAIL: u8 = 16;
pub const SENDMSG_DELETE_IF_EMPTY: u8 = 32;
pub const SENDMSG_REMAINING_MSG_BALANCE: u8 = 64;
pub const SENDMSG_ALL_BALANCE: u8 = 128;
//mask for cheking valid flags
pub const SENDMSG_VALID_FLAGS: u8 = SENDMSG_ORDINARY
    | SENDMSG_PAY_FEE_SEPARATELY
    | SENDMSG_IGNORE_ERROR
    | SENDMSG_BOUNCE_IF_FAIL
    | SENDMSG_DELETE_IF_EMPTY
    | SENDMSG_REMAINING_MSG_BALANCE
    | SENDMSG_ALL_BALANCE;

/// variants of reserve action
pub const RESERVE_EXACTLY: u8 = 0;
pub const RESERVE_ALL_BUT: u8 = 1;
pub const RESERVE_IGNORE_ERROR: u8 = 2;
pub const RESERVE_PLUS_ORIG: u8 = 4;
pub const RESERVE_REVERSE: u8 = 8;
pub const RESERVE_BOUNCE_IF_FAIL: u8 = 16;
pub const RESERVE_VALID_MODES: u8 = RESERVE_EXACTLY
    | RESERVE_ALL_BUT
    | RESERVE_IGNORE_ERROR
    | RESERVE_PLUS_ORIG
    | RESERVE_REVERSE
    | RESERVE_BOUNCE_IF_FAIL;

pub const CHANGE_LIB_MODE: u8 = 0;
pub const SET_LIB_CODE_MODE: u8 = 1;
pub const CHANGE_SET_LIB_MASK: u8 = 1;

pub const SET_LIB_CODE_REMOVE: u8 = 0;
pub const SET_LIB_CODE_ADD_PRIVATE: u8 = 1;
pub const SET_LIB_CODE_ADD_PUBLIC: u8 = 2;
pub const SET_LIB_CODE_ADD_PRIVATE_OR_PUBLIC_MASK: u8 = 3;
pub const CHANGE_SET_LIB_BOUNCE_IF_FAIL: u8 = 16;
pub const CHANGE_SET_LIB_VALID_MODES: u8 =
    CHANGE_SET_LIB_BOUNCE_IF_FAIL | SET_LIB_CODE_ADD_PRIVATE_OR_PUBLIC_MASK;

///
/// Implementation of Output Actions
///
impl OutAction {
    ///
    /// Create new instance OutAction::ActionSend
    ///
    pub fn new_send(mode: u8, out_msg: Message) -> Self {
        OutAction::SendMsg { mode, out_msg }
    }

    ///
    /// Create new instance OutAction::ActionCode
    ///
    pub fn new_set(new_code: Cell) -> Self {
        OutAction::SetCode { new_code }
    }

    ///
    /// Create new instance OutAction::ReserveCurrency
    ///
    pub fn new_reserve(mode: u8, value: CurrencyCollection) -> Self {
        OutAction::ReserveCurrency { mode, value }
    }

    ///
    /// Create new instance OutAction::ChangeLibrary
    ///
    pub fn new_change_library(mode: u8, code: Option<Cell>, hash: Option<UInt256>) -> Self {
        OutAction::ChangeLibrary { mode, code, hash }
    }
}

impl Serializable for OutAction {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            OutAction::SendMsg { mode, out_msg } => {
                ACTION_SEND_MSG.write_to(cell)?; // tag
                mode.write_to(cell)?;
                cell.checked_append_reference(out_msg.serialize()?)?;
            }
            OutAction::SetCode { new_code } => {
                ACTION_SET_CODE.write_to(cell)?; //tag
                cell.checked_append_reference(new_code.clone())?;
            }
            OutAction::ReserveCurrency { mode, value } => {
                ACTION_RESERVE.write_to(cell)?; // tag
                mode.write_to(cell)?;
                value.write_to(cell)?;
            }
            OutAction::ChangeLibrary { mode, code, hash } => {
                ACTION_CHANGE_LIB.write_to(cell)?; // tag
                mode.write_to(cell)?;
                if let Some(value) = hash {
                    value.write_to(cell)?;
                }
                if let Some(value) = code {
                    cell.checked_append_reference(value.clone())?;
                }
            }
            OutAction::None => fail!(BlockError::InvalidOperation("self is None".to_string())),
        }
        Ok(())
    }
}

impl Deserializable for OutAction {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        if cell.remaining_bits() < std::mem::size_of::<u32>() * 8 {
            fail!(BlockError::InvalidArg("cell can't be shorter than 32 bits".to_string()))
        }
        let tag = cell.get_next_u32()?;
        match tag {
            ACTION_SEND_MSG => {
                let mode = cell.get_next_byte()?;
                let msg = Message::construct_from_reference(cell)?;
                *self = OutAction::new_send(mode, msg);
            }
            ACTION_SET_CODE => *self = OutAction::new_set(cell.checked_drain_reference()?),
            ACTION_RESERVE => {
                let mode = cell.get_next_byte()?;
                let value = Deserializable::construct_from(cell)?;
                *self = OutAction::new_reserve(mode, value);
            }
            ACTION_CHANGE_LIB => {
                let mode = cell.get_next_byte()?;
                let flags = (mode >> 1) & SET_LIB_CODE_ADD_PRIVATE_OR_PUBLIC_MASK;
                match (mode & CHANGE_SET_LIB_MASK, flags) {
                    (CHANGE_LIB_MODE, 0) => {
                        let hash = UInt256::construct_from(cell)?;
                        *self = OutAction::new_change_library(mode, None, Some(hash));
                    }
                    (SET_LIB_CODE_MODE, SET_LIB_CODE_REMOVE)
                    | (SET_LIB_CODE_MODE, SET_LIB_CODE_ADD_PRIVATE)
                    | (SET_LIB_CODE_MODE, SET_LIB_CODE_ADD_PUBLIC) => {
                        let code = cell.checked_drain_reference()?;
                        *self = OutAction::new_change_library(mode, Some(code), None);
                    }
                    _ => fail!("wrong mode for ChangeLibrary action: {mode}"),
                }
            }
            tag => fail!(BlockError::InvalidConstructorTag { t: tag, s: "OutAction".to_string() }),
        }
        Ok(())
    }
}
