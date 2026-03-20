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
    executor::{
        engine::{storage::fetch_stack, Engine},
        serialize_currency_collection,
        types::Instruction,
    },
    stack::{
        integer::{behavior::OperationBehavior, IntegerData},
        StackItem,
    },
};
use num::{bigint::Sign, BigInt};
use ton_block::{
    fail, BuilderData, Cell, Deserializable, ExceptionCode, GasConsumer, IBitstring, Message,
    MsgAddressInt, MsgForwardPrices, Result, Serializable, SizeLimitsConfig, SliceData, Status,
    StorageUsageCalc, StorageUsed, ACTION_CHANGE_LIB, ACTION_RESERVE, ACTION_SEND_MSG,
    ACTION_SET_CODE, CHANGE_SET_LIB_VALID_MODES, SENDMSG_ALL_BALANCE,
    SENDMSG_REMAINING_MSG_BALANCE,
};

fn get_bigint(slice: &SliceData) -> BigInt {
    let bits = slice.remaining_bits();
    if bits == 0 {
        BigInt::from(0)
    } else if bits < 256 {
        BigInt::from_bytes_be(Sign::Plus, &slice.get_bytestring(0)) << (256 - bits)
    } else {
        BigInt::from_bytes_be(Sign::Plus, &slice.get_bytestring(0)[..32])
    }
}

// Blockchain related instructions ********************************************

fn add_action(
    engine: &mut Engine,
    action_id: u32,
    cell: Option<Cell>,
    suffix: BuilderData,
) -> Status {
    let mut new_action = BuilderData::new();
    let c5 = engine.ctrls.get(5).ok_or(ExceptionCode::TypeCheckError)?;
    new_action.checked_append_reference(c5.as_cell()?.clone())?;
    new_action.append_u32(action_id)?.append_builder(&suffix)?;
    if let Some(cell) = cell {
        new_action.checked_append_reference(cell)?;
    }
    let cell = engine.finalize_cell(new_action)?;
    engine.ctrls.put(5, StackItem::Cell(cell))?;
    Ok(())
}

/// CHANGELIB (h x - )
pub(super) fn execute_changelib(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("CHANGELIB"))?;
    fetch_stack(engine, 2)?;
    let x = engine.cmd.var(0).as_integer_value(0..=CHANGE_SET_LIB_VALID_MODES)?;
    if x & !CHANGE_SET_LIB_VALID_MODES != 0 {
        fail!(ExceptionCode::RangeCheckError, "invalid setlibcode mode {x}");
    }
    let hash = engine.cmd.var(1).as_integer()?.as_u256()?;
    let mut suffix = BuilderData::with_raw(vec![x * 2], 8)?;
    suffix.append_raw(&hash, 256)?;
    add_action(engine, ACTION_CHANGE_LIB, None, suffix)
}

fn calc_storage_used_short(
    engine: &mut Engine,
    msg: &Message,
    limits: &SizeLimitsConfig,
) -> Result<StorageUsed> {
    let mut calc =
        StorageUsageCalc::with_limits(limits.max_msg_cells as u64, limits.max_msg_bits as u64);
    let (body_to_ref, init_to_ref) = msg.recalc_serialization_params()?;
    if let Some(body) = msg.body() {
        let root = body.clone().into_builder()?;
        calc.append_builder(&root, body_to_ref, engine)?;
    }
    if let Some(init) = msg.state_init() {
        let root = init.write_to_new_cell()?;
        calc.append_builder(&root, init_to_ref, engine)?;
    }
    let sstat = calc.storage_used()?;
    Ok(sstat)
}

/// SENDMSG (c x – fee): pop mode and message cell from stack and put it at the
/// end of output actions list and calc fee.
pub(super) fn execute_send_msg(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SENDMSG"))?;
    fetch_stack(engine, 2)?;
    let mut x = engine.cmd.var(0).as_integer_value(0..=0x4FF)?;
    let send = (x & 1024) == 0;
    x &= !1024;
    if x >= 256 {
        fail!(ExceptionCode::RangeCheckError);
    }
    let x = x as u8;
    let cell = engine.cmd.var(1).as_cell()?.clone();
    // println!("msg: {}", ton_block::base64_encode(ton_block::write_boc(&cell)?));
    let mut msg = Message::construct_with_gas_consumer(cell.clone(), engine)?;
    let my_addr = engine.smci_param(8)?.as_slice()?;
    let my_addr = MsgAddressInt::construct_from(&mut my_addr.clone())?;
    let is_masterchain = my_addr.is_masterchain() | msg.is_dst_masterchain();
    msg.set_src_address(my_addr);
    let index = if is_masterchain { 4 } else { 5 };
    let prices = engine.smci_extra_param(14, index)?.as_slice()?;
    let prices = MsgForwardPrices::construct_from(&mut prices.clone())?;
    // it is not published yet
    let limits = if let Ok(limits) = engine.smci_extra_param(14, 6)?.as_slice() {
        SizeLimitsConfig::construct_from(&mut limits.clone())?
    } else {
        SizeLimitsConfig::default()
    };

    if let Some(hdr) = msg.int_header_mut() {
        if x & SENDMSG_ALL_BALANCE != 0 {
            hdr.value.grams = engine.smci_extra_param(7, 0)?.as_grams()?.try_into()?
        } else if x & SENDMSG_REMAINING_MSG_BALANCE != 0 {
            hdr.value.grams += engine.smci_extra_param(11, 0)?.as_grams()?
        }
        let fwd_full_fees = prices.lump_price.into();
        let fwd_mine_fees = prices.mine_fee_checked(&fwd_full_fees)?;
        hdr.fwd_fee = hdr.fwd_fee.max(fwd_full_fees - fwd_mine_fees);
    }
    // TODO: need to remove extra load when cpp is fixed
    engine.load_cell(cell.clone())?;
    let msg_storage = calc_storage_used_short(engine, &msg, &limits)?;
    let fee = prices.calc_fwd_fee(msg_storage.bits(), msg_storage.cells());
    engine.cc.stack.push(StackItem::int(fee));
    if send {
        let suffix = BuilderData::with_raw(vec![x], 8)?;
        add_action(engine, ACTION_SEND_MSG, Some(cell), suffix)?;
    }
    Ok(())
}

/// SENDRAWMSG (c x – ): pop mode and message cell from stack and put it at the
/// end of output actions list.
pub(super) fn execute_sendrawmsg(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SENDRAWMSG"))?;
    fetch_stack(engine, 2)?;
    let x = engine.cmd.var(0).as_integer_value(0..=255)?;
    let cell = engine.cmd.var(1).as_cell()?.clone();
    let suffix = BuilderData::with_raw(vec![x], 8)?;
    add_action(engine, ACTION_SEND_MSG, Some(cell), suffix)
}

/// SETCODE (c - )
pub(super) fn execute_setcode(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETCODE"))?;
    fetch_stack(engine, 1)?;
    let cell = engine.cmd.var(0).as_cell()?.clone();
    add_action(engine, ACTION_SET_CODE, Some(cell), BuilderData::new())
}

/// SETLIBCODE (c x - )
pub(super) fn execute_setlibcode(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETLIBCODE"))?;
    fetch_stack(engine, 2)?;
    let x = engine.cmd.var(0).as_integer_value(0..=CHANGE_SET_LIB_VALID_MODES)?;
    if x & !CHANGE_SET_LIB_VALID_MODES != 0 {
        fail!(ExceptionCode::RangeCheckError, "invalid setlibcode mode {x}");
    }
    let cell = engine.cmd.var(1).as_cell()?.clone();
    let suffix = BuilderData::with_raw(vec![x * 2 + 1], 8)?;
    add_action(engine, ACTION_CHANGE_LIB, Some(cell), suffix)
}

/// RAWRESERVE (x y - )
pub(super) fn execute_rawreserve(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RAWRESERVE"))?;
    fetch_stack(engine, 2)?;
    let y = engine.cmd.var(0).as_integer_value(0..=0b0001_1111)?;
    let mut suffix = BuilderData::with_raw(vec![y], 8)?;
    let x = engine.cmd.var(1).as_grams()?;
    suffix.append_builder(&serialize_currency_collection(x, None)?)?;
    add_action(engine, ACTION_RESERVE, None, suffix)
}

/// RAWRESERVEX (s y - )
pub(super) fn execute_rawreservex(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RAWRESERVEX"))?;
    fetch_stack(engine, 3)?;
    let y = engine.cmd.var(0).as_integer_value(0..=0b0001_1111)?;
    let mut suffix = BuilderData::with_raw(vec![y], 8)?;
    let other = engine.cmd.var(1).as_dict()?;
    let x = engine.cmd.var(2).as_grams()?;
    suffix.append_builder(&serialize_currency_collection(x, other.cloned())?)?;
    add_action(engine, ACTION_RESERVE, None, suffix)
}

pub(super) fn execute_ldmsgaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new(if T::quiet() {
        "LDMSGADDRQ"
    } else {
        "LDMSGADDR"
    }))?;
    fetch_stack(engine, 1)?;
    let mut slice = engine.cmd.var(0).as_slice()?.clone();
    let mut remainder = slice.clone();
    if parse_address(&mut remainder).is_ok() {
        slice.shrink_by_remainder(&remainder);
        engine.cc.stack.push(StackItem::Slice(slice));
        engine.cc.stack.push(StackItem::Slice(remainder));
        if T::quiet() {
            engine.cc.stack.push_bool(true);
        }
        Ok(())
    } else if T::quiet() {
        let var = engine.cmd.pop_var()?;
        engine.cc.stack.push(var);
        engine.cc.stack.push_bool(false);
        Ok(())
    } else {
        fail!(ExceptionCode::CellUnderflow)
    }
}

fn ldoptstdaddr(engine: &mut Engine, quiet: bool, none_allowed: bool) -> Status {
    fetch_stack(engine, 1)?;
    let mut slice = engine.cmd.var(0).as_slice()?.clone();
    match parse_opt_std_address(&mut slice, none_allowed) {
        Ok(result) => {
            engine.cc.stack.push(result);
            engine.cc.stack.push(StackItem::Slice(slice));
            if quiet {
                engine.cc.stack.push_bool(true);
            }
        }
        Err(err) => {
            if quiet {
                engine.cc.stack.push(engine.cmd.var_mut(0).withdraw());
                engine.cc.stack.push_bool(false);
            } else {
                return Err(err);
            }
        }
    }
    Ok(())
}

pub(super) fn execute_ldoptstdaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "LDOPTSTDADDRQ" } else { "LDOPTSTDADDR" };
    engine.load_instruction(Instruction::new(name))?;
    ldoptstdaddr(engine, T::quiet(), true)
}

pub(super) fn execute_ldstdaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "LDSTDADDRQ" } else { "LDSTDADDR" };
    engine.load_instruction(Instruction::new(name))?;
    ldoptstdaddr(engine, T::quiet(), false)
}

fn load_address<F, T>(engine: &mut Engine, name: &'static str, op: F) -> Status
where
    F: FnOnce(Vec<StackItem>, &mut dyn GasConsumer) -> Result<Vec<StackItem>>,
    T: OperationBehavior,
{
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    let mut slice = engine.cmd.var(0).as_slice()?.clone();
    let result = match parse_address(&mut slice) {
        Ok(addr) => match op(addr, engine) {
            Ok(mut stack) => {
                stack.drain(..).for_each(|var| {
                    engine.cc.stack.push(var);
                });
                None
            }
            Err(err) => Some(err),
        },
        Err(err) => Some(err),
    };
    if T::quiet() {
        engine.cc.stack.push_bool(result.is_none());
    } else if let Some(err) = result {
        return Err(err);
    }
    Ok(())
}

pub(super) fn execute_parsemsgaddr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    load_address::<_, T>(
        engine,
        if T::quiet() { "PARSEMSGADDRQ" } else { "PARSEMSGADDR" },
        |tuple, _| Ok(vec![StackItem::tuple(tuple)]),
    )
}

// (s - x y) compose rewrite_pfx and address to a 256 bit integer
pub(super) fn execute_rewrite_std_addr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    load_address::<_, T>(
        engine,
        if T::quiet() { "REWRITESTDADDRQ" } else { "REWRITESTDADDR" },
        |tuple, _| {
            if tuple.len() == 4 {
                let addr = tuple[3].as_slice()?;
                let mut y = match addr.remaining_bits() {
                    256 => IntegerData::from(get_bigint(addr))?,
                    _ => fail!(ExceptionCode::CellUnderflow),
                };
                if tuple[1].is_slice() {
                    let rewrite_pfx = tuple[1].as_slice()?;
                    let bits = rewrite_pfx.remaining_bits();
                    if bits > 256 {
                        fail!(ExceptionCode::CellUnderflow)
                    } else if bits > 0 {
                        let prefix = IntegerData::from(get_bigint(rewrite_pfx))?;
                        let mask = IntegerData::mask(256 - bits);
                        y = y.and::<T>(&mask)?.or::<T>(&prefix)?;
                    }
                };
                let x = tuple[2].clone();
                Ok(vec![x, StackItem::int(y)])
            } else {
                fail!(ExceptionCode::CellUnderflow)
            }
        },
    )
}

// (s - x s') compose rewrite_pfx and address to a slice
pub(super) fn execute_rewrite_var_addr<T: OperationBehavior>(engine: &mut Engine) -> Status {
    load_address::<_, T>(
        engine,
        if T::quiet() { "REWRITEVARADDRQ" } else { "REWRITEVARADDR" },
        |tuple, gas_consumer| {
            if tuple.len() == 4 {
                let mut addr = tuple[3].as_slice()?.clone();
                if let Ok(rewrite_pfx) = tuple[1].as_slice() {
                    let bits = rewrite_pfx.remaining_bits();
                    if bits > addr.remaining_bits() {
                        fail!(ExceptionCode::CellUnderflow)
                    } else if bits > 0 {
                        let mut b = rewrite_pfx.as_builder()?;
                        addr.shrink_data(bits..);
                        b.append_bytestring(&addr)?;
                        addr = gas_consumer.finalize_cell_and_load(b)?;
                    }
                };
                let x = tuple[2].clone();
                Ok(vec![x, StackItem::Slice(addr)])
            } else {
                fail!(ExceptionCode::CellUnderflow)
            }
        },
    )
}

fn read_rewrite_pfx(slice: &mut SliceData) -> Result<Option<SliceData>> {
    match slice.get_next_bit()? {
        true => {
            let len = slice.get_next_int(5)?;
            Ok(Some(slice.get_next_slice(len as usize)?))
        }
        false => Ok(None),
    }
}

fn parse_address(cell: &mut SliceData) -> Result<Vec<StackItem>> {
    let addr_type = cell.get_next_int(2)? as u8;
    let mut tuple = vec![int!(addr_type)];
    match addr_type & 0b11 {
        0b00 => (),
        0b01 => {
            let len = cell.get_next_int(9)?;
            tuple.push(StackItem::Slice(cell.get_next_slice(len as usize)?));
        }
        0b10 => {
            tuple.push(match read_rewrite_pfx(cell)? {
                Some(_slice) => {
                    fail!(ExceptionCode::CellUnderflow, "Anycast is not allowed")
                    // StackItem::Slice(slice)
                }
                None => StackItem::None,
            });
            tuple.push(int!(cell.get_next_byte()? as i8));
            tuple.push(StackItem::Slice(cell.get_next_slice(256)?));
        }
        0b11 => {
            fail!(ExceptionCode::CellUnderflow, "AddVar is not allowed")
            // tuple.push(match read_rewrite_pfx(cell)? {
            //     Some(slice) => StackItem::Slice(slice),
            //     None => StackItem::None
            // });
            // let len = cell.get_next_int(9)?;
            // tuple.push(int!(cell.get_next_i32()?));
            // tuple.push(StackItem::Slice(cell.get_next_slice(len as usize)?));
        }
        _ => (),
    }
    Ok(tuple)
}

fn parse_opt_std_address(slice: &mut SliceData, none_allowed: bool) -> Result<StackItem> {
    let mut cell = slice.clone();
    match slice.get_next_int(2)? {
        0b10 => {
            if read_rewrite_pfx(slice)?.is_some() {
                fail!(ExceptionCode::CellUnderflow, "Anycast is not allowed")
            }
            slice.move_by(8 + 256)?;
            cell.shrink_by_remainder(slice);
            Ok(StackItem::Slice(cell))
        }
        0b00 if none_allowed => Ok(StackItem::None),
        tag => fail!(ExceptionCode::CellUnderflow, "0b{tag:02b} is not a standard address tag"),
    }
}
