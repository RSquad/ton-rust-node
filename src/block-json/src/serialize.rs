/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use num::BigInt;
use num_traits::sign::Signed;
use serde_json::{Map, Value};
use std::{collections::HashMap, fmt};
use ton_block::*;

const VERSION: u32 = 8;
// Version changes
// 2 - fix var account addresses tag in block (`8_` postfix)
// 3 - `balance_delta` added to transaction
// 4 - decimal number fields companions
// 5 - storage stat in account
// 6 - deprecated
// 7 - ext_in_msg_fee in transaction
// 8 - file_hash in block and shard state

const STD_ACCOUNT_ID_LENGTH: usize = 256;

#[derive(Clone, Copy)]
pub enum SerializationMode {
    Standart,
    QServer,
    Debug,
}

impl SerializationMode {
    pub fn is_standart(&self) -> bool {
        matches!(self, SerializationMode::Standart)
    }

    pub fn is_q_server(&self) -> bool {
        matches!(self, SerializationMode::QServer | SerializationMode::Debug)
    }
}

struct SignedCurrencyCollection {
    pub grams: BigInt,
    pub other: HashMap<u32, BigInt>,
}

impl SignedCurrencyCollection {
    pub fn new() -> Self {
        SignedCurrencyCollection { grams: 0.into(), other: HashMap::new() }
    }

    pub fn from_cc(cc: &CurrencyCollection) -> Result<Self> {
        let mut other = HashMap::new();
        cc.other.iterate_with_keys(|key, value| {
            other.insert(key, value.value().clone());
            Ok(true)
        })?;

        Ok(SignedCurrencyCollection { grams: cc.grams.as_u128().into(), other })
    }

    pub fn add(&mut self, other: &Self) {
        self.grams += &other.grams;
        for (key, value) in self.other.iter_mut() {
            if let Some(other_value) = other.other.get(key) {
                *value += other_value;
            }
        }
        for (key, value) in other.other.iter() {
            if !self.other.contains_key(key) {
                self.other.insert(*key, value.clone());
            }
        }
    }

    pub fn sub(&mut self, other: &Self) {
        self.grams -= &other.grams;
        for (key, value) in self.other.iter_mut() {
            if let Some(other_value) = other.other.get(key) {
                *value -= other_value;
            }
        }
        for (key, value) in other.other.iter() {
            if !self.other.contains_key(key) {
                self.other.insert(*key, -value.clone());
            }
        }
    }
}

fn get_msg_fees(msg: &Message) -> Option<&Grams> {
    match msg.header() {
        CommonMsgInfo::IntMsgInfo(header) => Some(&header.fwd_fee),
        _ => None,
    }
}

pub fn u64_to_string(value: u64) -> String {
    let mut string = format!("{:x}", value);
    string.insert_str(0, &format!("{:x}", string.len() - 1));
    string
}

pub fn bigint_to_string(value: &BigInt) -> String {
    if num::bigint::Sign::Minus == value.sign() {
        let bytes: Vec<u8> = value.to_bytes_be().1.iter().map(|byte| byte ^ 0xFF).collect();
        let string = hex::encode(bytes).trim_start_matches('f').to_owned();
        format!("-{:02x}{}", (string.len() - 1) ^ 0xFF, string)
    } else {
        let mut string = format!("{:x}", value);
        string.insert_str(0, &format!("{:02x}", string.len() - 1));
        string
    }
}

pub fn block_order(block: &Block, mc_seq_no: u32) -> Result<String> {
    let info = block.read_info()?;
    let master_order = u64_to_string(mc_seq_no as u64);
    if !info.shard().is_masterchain() {
        let mut workchain_order = u64_to_string(info.shard().workchain_id().unsigned_abs() as u64);
        if info.shard().workchain_id() < 0 {
            workchain_order = format!("-{}", workchain_order);
        }
        let seq_no_order = u64_to_string(info.seq_no() as u64);
        let shard_order = u64_to_string(info.shard().shard_prefix_with_tag().reverse_bits());
        Ok(master_order + &workchain_order + &seq_no_order + &shard_order)
    } else if info.seq_no() != mc_seq_no {
        fail!(
            "provided mc_seq_no {} is not equal to seq_no of masterchain block {}",
            mc_seq_no,
            info.seq_no(),
        )
    } else {
        Ok(master_order + "m")
    }
}

fn serialize_grams(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    value: &Grams,
    mode: SerializationMode,
) {
    let string = match mode {
        SerializationMode::Standart => {
            serialize_field(map, &(id_str.to_owned() + "_dec"), value.to_string());
            let mut string = format!("{:x}", value.as_u128());
            string.insert_str(0, &format!("{:02x}", string.len() - 1));
            string
        }
        SerializationMode::QServer => {
            format!("0x{:x}", value.as_u128())
        }
        SerializationMode::Debug => value.to_string(),
    };

    serialize_field(map, id_str, string);
}

fn serialize_u64(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    value: &u64,
    mode: SerializationMode,
) {
    let string = match mode {
        SerializationMode::Standart => {
            serialize_field(map, &(id_str.to_owned() + "_dec"), value.to_string());
            u64_to_string(*value)
        }
        SerializationMode::QServer => {
            format!("0x{:x}", value)
        }
        SerializationMode::Debug => value.to_string(),
    };
    serialize_field(map, id_str, string);
}

fn serialize_lt(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    value: u64,
    mode: SerializationMode,
) {
    let string = match mode {
        SerializationMode::Standart => {
            serialize_field(map, &(id_str.to_owned() + "_dec"), value.to_string());
            u64_to_string(value)
        }
        SerializationMode::QServer => {
            format!("0x{:x}", value)
        }
        SerializationMode::Debug => format!("{}_{}", value / 1_000_000, value % 1_000_000),
    };

    serialize_field(map, id_str, string);
}

fn serialize_bigint(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    value: &BigInt,
    mode: SerializationMode,
) {
    let string = match mode {
        SerializationMode::Standart => bigint_to_string(value),
        SerializationMode::QServer => {
            if num::bigint::Sign::Minus == value.sign() {
                format!("-0x{:x}", value.abs())
            } else {
                format!("0x{:x}", value)
            }
        }
        SerializationMode::Debug => value.to_string(),
    };

    if let SerializationMode::Standart = mode {
        serialize_field(map, &(id_str.to_owned() + "_dec"), value.to_string());
    }
    serialize_field(map, id_str, string);
}

pub fn shard_to_string(value: u64) -> String {
    format!("{:016x}", value)
}

fn construct_address(workchain_id: i32, account_id: AccountId) -> Result<MsgAddressInt> {
    if (-128..=127).contains(&workchain_id) && account_id.remaining_bits() == STD_ACCOUNT_ID_LENGTH
    {
        MsgAddressInt::with_standart(None, workchain_id as i8, account_id)
    } else {
        MsgAddressInt::with_variant(None, workchain_id, account_id)
    }
}

fn serialize_cell(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    cell: Option<&Cell>,
    write_hash: bool,
) -> Result<()> {
    if let Some(cell) = cell {
        if !cell.is_pruned() {
            let bytes = write_boc(cell)?;
            serialize_field(map, id_str, base64_encode(bytes));
        }
        if write_hash {
            let string = id_str.to_owned() + "_hash";
            serialize_uint256(map, &string, &cell.repr_hash())
        }
    }
    Ok(())
}

fn serialize_slice(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    slice: Option<&SliceData>,
    write_hash: bool,
) -> Result<()> {
    if let Some(slice) = slice {
        let cell = slice.clone().into_cell()?;
        let bytes = write_boc(&cell)?;
        serialize_field(map, id_str, base64_encode(bytes));
        if write_hash {
            let string = id_str.to_owned() + "_hash";
            serialize_uint256(map, &string, &cell.repr_hash())
        }
    }
    Ok(())
}

fn serialize_id(map: &mut Map<String, Value>, id_str: &str, id: Option<&impl fmt::LowerHex>) {
    if let Some(id) = id {
        serialize_uint256(map, id_str, id);
    }
}

fn serialize_uint256(map: &mut Map<String, Value>, name: &str, value: &impl fmt::LowerHex) {
    map.insert(name.to_string(), format!("{:x}", value).into());
}

fn serialize_field(map: &mut Map<String, Value>, id_str: &str, value: impl Into<Value>) {
    map.insert(id_str.to_string(), value.into());
}

fn serialize_split_info(map: &mut Map<String, Value>, split_info: &SplitMergeInfo) {
    serialize_field(map, "cur_shard_pfx_len", split_info.cur_shard_pfx_len);
    serialize_field(map, "acc_split_depth", split_info.acc_split_depth);
    serialize_id(map, "this_addr", Some(&split_info.this_addr));
    serialize_id(map, "sibling_addr", Some(&split_info.sibling_addr));
}

fn serialize_storage_phase<'a>(
    map: &mut Map<String, Value>,
    ph: Option<&'a TrStoragePhase>,
    mode: SerializationMode,
) -> Option<&'a Grams> {
    if let Some(ph) = ph {
        let mut ph_map = serde_json::Map::new();
        serialize_grams(&mut ph_map, "storage_fees_collected", &ph.storage_fees_collected, mode);
        if let Some(grams) = &ph.storage_fees_due {
            serialize_grams(&mut ph_map, "storage_fees_due", grams, mode);
        }
        let status_change = match ph.status_change {
            AccStatusChange::Unchanged => 0,
            AccStatusChange::Frozen => 1,
            AccStatusChange::Deleted => 2,
        };
        serialize_field(&mut ph_map, "status_change", status_change);
        if mode.is_q_server() {
            let status_change = match ph.status_change {
                AccStatusChange::Unchanged => "unchanged",
                AccStatusChange::Frozen => "frozen",
                AccStatusChange::Deleted => "deleted",
            };
            serialize_field(&mut ph_map, "status_change_name", status_change);
        }
        serialize_field(map, "storage", ph_map);
        Some(&ph.storage_fees_collected)
    } else {
        None
    }
}

fn serialize_compute_phase<'a>(
    map: &mut Map<String, Value>,
    ph: Option<&'a TrComputePhase>,
    mode: SerializationMode,
) -> Option<&'a Grams> {
    let mut ph_map = serde_json::Map::new();
    let mut fees = None;
    let (type_, type_name) = match ph {
        Some(TrComputePhase::Skipped(ph)) => {
            let reason = match ph.reason {
                ComputeSkipReason::NoState => 0,
                ComputeSkipReason::BadState => 1,
                ComputeSkipReason::NoGas => 2,
                ComputeSkipReason::Suspended => 5,
            };
            ph_map.insert("skipped_reason".to_string(), reason.into());
            if mode.is_q_server() {
                let reason = match ph.reason {
                    ComputeSkipReason::NoState => "noState",
                    ComputeSkipReason::BadState => "badState",
                    ComputeSkipReason::NoGas => "noGas",
                    ComputeSkipReason::Suspended => "suspended",
                };
                ph_map.insert("skipped_reason_name".to_string(), reason.into());
            }
            (0, "skipped")
        }
        Some(TrComputePhase::Vm(ph)) => {
            ph_map.insert("success".to_string(), ph.success.into());
            ph_map.insert("msg_state_used".to_string(), ph.msg_state_used.into());
            ph_map.insert("account_activated".to_string(), ph.account_activated.into());
            serialize_grams(&mut ph_map, "gas_fees", &ph.gas_fees, mode);
            fees = Some(&ph.gas_fees);
            ph_map.insert("gas_used".to_string(), ph.gas_used.as_u64().into());
            ph_map.insert("gas_limit".to_string(), ph.gas_limit.as_u64().into());
            if let Some(value) = ph.gas_credit.as_ref() {
                ph_map.insert("gas_credit".to_string(), value.as_u32().into());
            }
            ph_map.insert("mode".to_string(), ph.mode.into());
            ph_map.insert("exit_code".to_string(), ph.exit_code.into());
            ph.exit_arg.map(|value| ph_map.insert("exit_arg".to_string(), value.into()));
            ph_map.insert("vm_steps".to_string(), ph.vm_steps.into());
            serialize_id(&mut ph_map, "vm_init_state_hash", Some(&ph.vm_init_state_hash));
            serialize_id(&mut ph_map, "vm_final_state_hash", Some(&ph.vm_final_state_hash));
            (1, "vm")
        }
        None => return None,
    };

    ph_map.insert("compute_type".to_string(), type_.into());
    if mode.is_q_server() {
        ph_map.insert("compute_type_name".to_string(), type_name.into());
    }
    serialize_field(map, "compute", ph_map);
    fees
}

fn serialize_credit_phase(
    map: &mut Map<String, Value>,
    ph: Option<&TrCreditPhase>,
    mode: SerializationMode,
) -> Result<()> {
    if let Some(ph) = ph {
        let mut ph_map = serde_json::Map::new();
        if let Some(grams) = &ph.due_fees_collected {
            serialize_grams(&mut ph_map, "due_fees_collected", grams, mode);
        }
        serialize_cc(&mut ph_map, "credit", &ph.credit, mode)?;
        serialize_field(map, "credit", ph_map);
    }
    Ok(())
}

fn serialize_action_phase<'a>(
    map: &mut Map<String, Value>,
    ph: Option<&'a TrActionPhase>,
    mode: SerializationMode,
) -> Option<&'a Grams> {
    if let Some(ph) = ph {
        let mut ph_map = serde_json::Map::new();
        ph_map.insert("success".to_string(), ph.success.into());
        ph_map.insert("valid".to_string(), ph.valid.into());
        ph_map.insert("no_funds".to_string(), ph.no_funds.into());
        let status_change = match ph.status_change {
            AccStatusChange::Unchanged => 0,
            AccStatusChange::Frozen => 1,
            AccStatusChange::Deleted => 2,
        };
        serialize_field(&mut ph_map, "status_change", status_change);
        if let Some(grams) = ph.total_fwd_fees.as_ref() {
            serialize_grams(&mut ph_map, "total_fwd_fees", grams, mode);
        }
        if let Some(grams) = ph.total_action_fees.as_ref() {
            serialize_grams(&mut ph_map, "total_action_fees", grams, mode);
        }
        let fees = ph.total_action_fees.as_ref();
        ph_map.insert("result_code".to_string(), ph.result_code.into());
        ph.result_arg.map(|value| ph_map.insert("result_arg".to_string(), value.into()));
        ph_map.insert("tot_actions".to_string(), ph.tot_actions.into());
        ph_map.insert("spec_actions".to_string(), ph.spec_actions.into());
        ph_map.insert("skipped_actions".to_string(), ph.skipped_actions.into());
        ph_map.insert("msgs_created".to_string(), ph.msgs_created.into());
        ph_map.insert("action_list_hash".to_string(), ph.action_list_hash.as_hex_string().into());
        ph_map.insert("tot_msg_size_cells".to_string(), ph.tot_msg_size.cells().into());
        ph_map.insert("tot_msg_size_bits".to_string(), ph.tot_msg_size.bits().into());
        serialize_field(map, "action", ph_map);
        fees
    } else {
        None
    }
}

fn serialize_bounce_phase<'a>(
    map: &mut Map<String, Value>,
    ph: Option<&'a TrBouncePhase>,
    mode: SerializationMode,
) -> Option<&'a Grams> {
    let mut ph_map = serde_json::Map::new();
    let mut fees = None;
    let (bounce_type, type_name) = match ph {
        Some(TrBouncePhase::Negfunds) => (0, "negFunds"),
        Some(TrBouncePhase::Nofunds(ph)) => {
            ph_map.insert("msg_size_cells".to_string(), ph.msg_size.cells().into());
            ph_map.insert("msg_size_bits".to_string(), ph.msg_size.bits().into());
            serialize_grams(&mut ph_map, "req_fwd_fees", &ph.req_fwd_fees, mode);
            (1, "noFunds")
        }
        Some(TrBouncePhase::Ok(ph)) => {
            ph_map.insert("msg_size_cells".to_string(), ph.msg_size.cells().into());
            ph_map.insert("msg_size_bits".to_string(), ph.msg_size.bits().into());
            fees = Some(&ph.msg_fees);
            serialize_grams(&mut ph_map, "msg_fees", &ph.msg_fees, mode);
            serialize_grams(&mut ph_map, "fwd_fees", &ph.fwd_fees, mode);
            (2, "ok")
        }
        None => return None,
    };
    ph_map.insert("bounce_type".to_string(), bounce_type.into());
    if mode.is_q_server() {
        ph_map.insert("bounce_type_name".to_string(), type_name.into());
    }
    serialize_field(map, "bounce", ph_map);
    fees
}

fn serialize_cc(
    map: &mut Map<String, Value>,
    prefix: &'static str,
    cc: &CurrencyCollection,
    mode: SerializationMode,
) -> Result<()> {
    serialize_grams(map, prefix, &cc.grams, mode);
    let other = serialize_ecc(&cc.other, mode)?;
    if !other.is_empty() {
        map.insert(format!("{}_other", prefix), other.into());
    }
    Ok(())
}

fn serialize_ecc(
    ecc: &ExtraCurrencyCollection,
    mode: SerializationMode,
) -> Result<Vec<Map<String, Value>>> {
    let mut other = Vec::new();
    ecc.iterate_with_keys(|key: u32, ref mut value| -> Result<bool> {
        let mut other_map = Map::new();
        serialize_field(&mut other_map, "currency", key);
        serialize_bigint(&mut other_map, "value", value.value(), mode);
        other.push(other_map);
        Ok(true)
    })?;
    Ok(other)
}

fn serialize_scc(
    map: &mut Map<String, Value>,
    prefix: &'static str,
    scc: &SignedCurrencyCollection,
    mode: SerializationMode,
) {
    serialize_bigint(map, prefix, &scc.grams, mode);
    let mut other = Vec::new();
    for (key, value) in &scc.other {
        let mut other_map = Map::new();
        serialize_field(&mut other_map, "currency", *key);
        serialize_bigint(&mut other_map, "value", value, mode);
        other.push(other_map);
    }
    if !other.is_empty() {
        map.insert(format!("{}_other", prefix), other.into());
    }
}

fn serialize_intermidiate_address(
    map: &mut Map<String, Value>,
    id_str: &'static str,
    addr: &IntermediateAddress,
) {
    let addr = match addr {
        IntermediateAddress::Regular(addr) => addr.use_src_bits().to_string(),
        IntermediateAddress::Simple(addr) => {
            format!("{}:{:x}", addr.workchain_id, addr.addr_pfx)
        }
        IntermediateAddress::Ext(addr) => {
            format!("{}:{:x}", addr.workchain_id, addr.addr_pfx)
        }
    };
    map.insert(id_str.to_string(), addr.into());
}

fn serialize_envelope_msg(env: &MsgEnvelope, mode: SerializationMode) -> Map<String, Value> {
    let mut map = Map::new();
    let msg = env.read_message().unwrap_or_default();
    serialize_id(&mut map, "msg_id", Some(&env.message_cell().repr_hash()));
    if let SerializationMode::Debug = mode {
        let (cur_prefix, next_prefix) = env.calc_cur_next_prefix().unwrap_or_default();
        if let Some(src) = msg.src_ref() {
            if let Ok(src_prefix) = AccountIdPrefixFull::checked_prefix(src) {
                map.insert("src_prefix".to_string(), src_prefix.to_string().into());
            }
        }
        if let Some(dst) = msg.dst_ref() {
            if let Ok(dst_prefix) = AccountIdPrefixFull::checked_prefix(dst) {
                map.insert("dst_prefix".to_string(), dst_prefix.to_string().into());
            }
        }
        map.insert("cur_prefix".to_string(), cur_prefix.to_string().into());
        map.insert("next_prefix".to_string(), next_prefix.to_string().into());
        serialize_lt(&mut map, "create_lt", msg.created_lt().unwrap_or_default(), mode);
        serialize_lt(&mut map, "emitted_lt", env.emitted_lt(), mode);
    }
    if let Some(metadata) = env.metadata() {
        serialize_field(&mut map, "metadata_depth", metadata.depth());
        serialize_field(&mut map, "metadata_initiator_addr", metadata.initiator_addr().to_string());
        serialize_lt(&mut map, "metadata_initiator_lt", metadata.initiator_lt(), mode);
    }
    serialize_intermidiate_address(&mut map, "cur_addr", env.cur_addr());
    serialize_intermidiate_address(&mut map, "next_addr", env.next_addr());
    serialize_grams(&mut map, "fwd_fee_remaining", env.fwd_fee_remaining(), mode);
    map
}

fn serialize_in_msg(msg: &InMsg, mode: SerializationMode) -> Result<Value> {
    let mut map = Map::new();
    let (type_, type_name) = match msg {
        InMsg::External(msg) => {
            serialize_id(&mut map, "msg_id", Some(&msg.message_cell().repr_hash()));
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            (0, "external")
        }
        InMsg::IHR(msg) => {
            serialize_id(&mut map, "msg_id", Some(&msg.message_cell().repr_hash()));
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            serialize_grams(&mut map, "ihr_fee", msg.ihr_fee(), mode);
            serialize_cell(&mut map, "proof_created", Some(msg.proof_created()), false)?;
            (1, "ihr")
        }
        InMsg::Immediate(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_envelope_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            serialize_grams(&mut map, "fwd_fee", &msg.fwd_fee, mode);
            (2, "immediately")
        }
        InMsg::Final(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_envelope_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            serialize_grams(&mut map, "fwd_fee", &msg.fwd_fee, mode);
            (3, "final")
        }
        InMsg::DeferredFinal(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_envelope_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            serialize_grams(&mut map, "fwd_fee", &msg.fwd_fee, mode);
            (8, "deferred final")
        }
        InMsg::Transit(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_in_message()?, mode).into(),
            );
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            serialize_grams(&mut map, "transit_fee", &msg.transit_fee, mode);
            (4, "transit")
        }
        InMsg::DeferredTransit(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_in_envelope_message()?, mode).into(),
            );
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_envelope_message()?, mode).into(),
            );
            (9, "deferred transit")
        }
        InMsg::DiscardedFinal(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_envelope_message()?, mode).into(),
            );
            serialize_u64(&mut map, "transaction_id", &msg.transaction_id(), mode);
            serialize_grams(&mut map, "fwd_fee", &msg.fwd_fee, mode);
            (5, "discardedFinal")
        }
        InMsg::DiscardedTransit(msg) => {
            map.insert(
                "in_msg".to_string(),
                serialize_envelope_msg(&msg.read_envelope_message()?, mode).into(),
            );
            serialize_u64(&mut map, "transaction_id", &msg.transaction_id(), mode);
            serialize_grams(&mut map, "fwd_fee", msg.fwd_fee(), mode);
            serialize_cell(&mut map, "proof_delivered", Some(msg.proof_delivered()), false)?;
            (6, "discardedTransit")
        }
        InMsg::None => (-1, "none"),
    };
    map.insert("msg_type".to_string(), type_.into());
    if mode.is_q_server() {
        map.insert("msg_type_name".to_string(), type_name.into());
    }
    Ok(map.into())
}

fn serialize_out_msg(msg: &OutMsg, mode: SerializationMode) -> Result<Value> {
    let mut map = Map::new();
    let (type_, type_name) = match msg {
        OutMsg::External(msg) => {
            serialize_id(&mut map, "msg_id", Some(&msg.message_cell().repr_hash()));
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            (0, "external")
        }
        OutMsg::Immediate(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            map.insert(
                "reimport".to_string(),
                serialize_in_msg(&msg.read_reimport_message()?, mode)?,
            );
            (1, "immediately")
        }
        OutMsg::New(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            (2, "outMsgNew")
        }
        OutMsg::NewDefer(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            serialize_id(&mut map, "transaction_id", Some(&msg.transaction_cell().repr_hash()));
            (8, "out deferred new")
        }
        OutMsg::Transit(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            map.insert("imported".to_string(), serialize_in_msg(&msg.read_imported()?, mode)?);
            (3, "transit")
        }
        OutMsg::DeferredTransit(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            map.insert("imported".to_string(), serialize_in_msg(&msg.read_imported()?, mode)?);
            (9, "out deferred transit")
        }
        OutMsg::DequeueImmediate(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            map.insert(
                "reimport".to_string(),
                serialize_in_msg(&msg.read_reimport_message()?, mode)?,
            );
            (4, "dequeueImmediately")
        }
        OutMsg::Dequeue(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            serialize_lt(&mut map, "import_block_lt", msg.import_block_lt, mode);
            (5, "dequeue")
        }
        OutMsg::TransitRequeued(msg) => {
            map.insert(
                "out_msg".to_string(),
                serialize_envelope_msg(&msg.read_out_message()?, mode).into(),
            );
            map.insert("imported".to_string(), serialize_in_msg(&msg.read_imported()?, mode)?);
            (6, "transitRequeued")
        }
        OutMsg::DequeueShort(msg) => {
            serialize_id(&mut map, "msg_env_hash", Some(&msg.msg_env_hash));
            map.insert("next_workchain".to_string(), msg.next_workchain.into());
            map.insert("next_addr_pfx".to_string(), shard_to_string(msg.next_addr_pfx).into());
            if let SerializationMode::Debug = mode {
                map.insert(
                    "next_prefix".to_string(),
                    format!("{}:{:016X}", msg.next_workchain, msg.next_addr_pfx).into(),
                );
            }
            serialize_lt(&mut map, "import_block_lt", msg.import_block_lt, mode);
            (7, "dequeueShort")
        }
        OutMsg::None => (-1, "none"),
    };
    map.insert("msg_type".to_string(), type_.into());
    if mode.is_q_server() {
        map.insert("msg_type_name".to_string(), type_name.into());
    }
    Ok(map.into())
}

fn serialize_validators_stat(stat: &ValidatorsStat) -> Result<Value> {
    let mut map = Map::new();
    for i in 0..stat.len() as u16 {
        serialize_field(&mut map, &i.to_string(), stat.get(i)?);
    }
    Ok(map.into())
}

fn serialize_shard_descr(descr: &ShardDescr, mode: SerializationMode) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "seq_no", descr.seq_no);
    serialize_field(&mut map, "reg_mc_seqno", descr.reg_mc_seqno);
    serialize_lt(&mut map, "start_lt", descr.start_lt, mode);
    serialize_lt(&mut map, "end_lt", descr.end_lt, mode);
    serialize_field(&mut map, "root_hash", descr.root_hash.as_hex_string());
    serialize_field(&mut map, "file_hash", descr.file_hash.as_hex_string());
    serialize_field(&mut map, "before_split", descr.before_split);
    serialize_field(&mut map, "before_merge", descr.before_merge);
    serialize_field(&mut map, "want_split", descr.want_split);
    serialize_field(&mut map, "want_merge", descr.want_merge);
    serialize_field(&mut map, "nx_cc_updated", descr.nx_cc_updated);
    serialize_field(&mut map, "gen_utime", descr.gen_utime);
    serialize_field(&mut map, "next_catchain_seqno", descr.next_catchain_seqno);
    serialize_field(&mut map, "next_validator_shard", shard_to_string(descr.next_validator_shard));
    serialize_field(&mut map, "min_ref_mc_seqno", descr.min_ref_mc_seqno);
    serialize_field(&mut map, "flags", descr.flags);
    serialize_cc(&mut map, "fees_collected", &descr.fees_collected, mode)?;
    serialize_cc(&mut map, "funds_created", &descr.funds_created, mode)?;
    match descr.split_merge_at {
        FutureSplitMerge::Split { split_utime, interval } => {
            serialize_field(&mut map, "split_utime", split_utime);
            serialize_field(&mut map, "split_interval", interval);
        }
        FutureSplitMerge::Merge { merge_utime, interval } => {
            serialize_field(&mut map, "merge_utime", merge_utime);
            serialize_field(&mut map, "merge_interval", interval);
        }
        FutureSplitMerge::None => (),
    };
    Ok(map.into())
}

fn serialize_config_proposal_setup(cps: &ConfigProposalSetup) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "min_tot_rounds", cps.min_tot_rounds);
    serialize_field(&mut map, "max_tot_rounds", cps.max_tot_rounds);
    serialize_field(&mut map, "min_wins", cps.min_wins);
    serialize_field(&mut map, "max_losses", cps.max_losses);
    serialize_field(&mut map, "min_store_sec", cps.min_store_sec);
    serialize_field(&mut map, "max_store_sec", cps.max_store_sec);
    serialize_field(&mut map, "bit_price", cps.bit_price);
    serialize_field(&mut map, "cell_price", cps.cell_price);
    Ok(map.into())
}

fn serialize_mandatory_params(mp: &MandatoryParams) -> Result<Value> {
    let mut vector = Vec::new();
    mp.iterate_keys(|n: u32| -> Result<bool> {
        vector.push(n);
        Ok(true)
    })?;
    Ok(vector.into())
}

fn serialize_workchains(wcs: &Workchains) -> Result<Value> {
    let mut vector = Vec::new();
    wcs.iterate_with_keys(|key: u32, wc: WorkchainDescr| -> Result<bool> {
        let mut map = Map::new();
        serialize_field(&mut map, "workchain_id", key);
        serialize_field(&mut map, "enabled_since", wc.enabled_since);
        serialize_field(&mut map, "monitor_min_split", wc.monitor_min_split());
        serialize_field(&mut map, "min_split", wc.min_split());
        serialize_field(&mut map, "max_split", wc.max_split());
        serialize_field(&mut map, "active", wc.active);
        serialize_field(&mut map, "accept_msgs", wc.accept_msgs);
        serialize_field(&mut map, "flags", wc.flags);
        serialize_uint256(&mut map, "zerostate_root_hash", &wc.zerostate_root_hash);
        serialize_uint256(&mut map, "zerostate_file_hash", &wc.zerostate_file_hash);
        serialize_field(&mut map, "version", wc.version);
        match wc.format {
            WorkchainFormat::Basic(f) => {
                serialize_field(&mut map, "basic", true);
                serialize_field(&mut map, "vm_version", f.vm_version);
                serialize_field(&mut map, "vm_mode", f.vm_mode);
            }
            WorkchainFormat::Extended(f) => {
                serialize_field(&mut map, "basic", false);
                serialize_field(&mut map, "min_addr_len", f.min_addr_len());
                serialize_field(&mut map, "max_addr_len", f.max_addr_len());
                serialize_field(&mut map, "addr_len_step", f.addr_len_step());
                serialize_field(&mut map, "workchain_type_id", f.workchain_type_id());
            }
        }
        vector.push(Value::from(map));
        Ok(true)
    })?;
    Ok(vector.into())
}

fn serialize_storage_prices(wcs: &ConfigParam18Map, mode: SerializationMode) -> Result<Value> {
    let mut vector = Vec::new();
    wcs.iterate(|val| {
        let mut map = Map::new();
        serialize_field(&mut map, "utime_since", val.utime_since);
        serialize_u64(&mut map, "bit_price_ps", &val.bit_price_ps, mode);
        serialize_u64(&mut map, "cell_price_ps", &val.cell_price_ps, mode);
        serialize_u64(&mut map, "mc_bit_price_ps", &val.mc_bit_price_ps, mode);
        serialize_u64(&mut map, "mc_cell_price_ps", &val.mc_cell_price_ps, mode);
        vector.push(Value::from(map));
        Ok(true)
    })?;
    Ok(vector.into())
}

fn serialize_gas_limits_prices(
    map: &mut Map<String, Value>,
    gp: &GasLimitsPrices,
    mode: SerializationMode,
) {
    serialize_u64(map, "flat_gas_limit", &gp.flat_gas_limit, mode);
    serialize_u64(map, "flat_gas_price", &gp.flat_gas_price, mode);
    serialize_u64(map, "gas_price", &gp.gas_price, mode);
    serialize_u64(map, "gas_limit", &gp.gas_limit, mode);
    serialize_u64(map, "special_gas_limit", &gp.special_gas_limit, mode);
    serialize_u64(map, "gas_credit", &gp.gas_credit, mode);
    serialize_u64(map, "block_gas_limit", &gp.block_gas_limit, mode);
    serialize_u64(map, "freeze_due_limit", &gp.freeze_due_limit, mode);
    serialize_u64(map, "delete_due_limit", &gp.delete_due_limit, mode);
}

fn serialize_params_limits(pl: &ParamLimits) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "underload", pl.underload());
    serialize_field(&mut map, "soft_limit", pl.soft_limit());
    serialize_field(&mut map, "hard_limit", pl.hard_limit());
    Ok(map.into())
}

fn serialize_msg_queue_limits(ql: &ImportedMsgQueueLimits) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "max_bytes", ql.max_bytes);
    serialize_field(&mut map, "max_msgs", ql.max_msgs);
    Ok(map.into())
}

fn serialize_block_limits(map: &mut Map<String, Value>, bl: &BlockLimits) -> Result<()> {
    serialize_field(map, "bytes", serialize_params_limits(bl.bytes())?);
    serialize_field(map, "gas", serialize_params_limits(bl.gas())?);
    serialize_field(map, "lt_delta", serialize_params_limits(bl.lt_delta())?);
    if let Some(cd) = bl.collated_data() {
        serialize_field(map, "collated_data", serialize_params_limits(cd)?);
    }
    if let Some(ql) = bl.imported_msg_queue() {
        serialize_field(map, "imported_msg_queue", serialize_msg_queue_limits(ql)?);
    }
    Ok(())
}

fn serialize_msg_fwd_prices(
    map: &mut Map<String, Value>,
    fp: &MsgForwardPrices,
    mode: SerializationMode,
) -> Result<()> {
    serialize_u64(map, "lump_price", &fp.lump_price, mode);
    serialize_u64(map, "bit_price", &fp.bit_price, mode);
    serialize_u64(map, "cell_price", &fp.cell_price, mode);
    serialize_field(map, "ihr_price_factor", fp.ihr_price_factor);
    serialize_field(map, "first_frac", fp.first_frac);
    serialize_field(map, "next_frac", fp.next_frac);
    Ok(())
}

fn serialize_fundamental_smc_addresses(addresses: &FundamentalSmcAddresses) -> Result<Value> {
    let mut vector = Vec::<Value>::new();
    addresses.iterate_keys(|k: UInt256| -> Result<bool> {
        vector.push(k.as_hex_string().into());
        Ok(true)
    })?;
    Ok(vector.into())
}

fn serialize_validators_set(
    map: &mut Map<String, Value>,
    set: &ValidatorSet,
    mode: SerializationMode,
) -> Result<()> {
    serialize_field(map, "utime_since", set.utime_since());
    serialize_field(map, "utime_until", set.utime_until());
    serialize_field(map, "total", set.total());
    serialize_field(map, "main", set.main());
    serialize_u64(map, "total_weight", &set.total_weight(), mode);
    let mut vector = Vec::<Value>::new();
    for v in set.list() {
        let mut map = Map::new();
        serialize_field(&mut map, "public_key", hex::encode(v.public_key.as_slice()));
        serialize_u64(&mut map, "weight", &v.weight, mode);
        serialize_id(&mut map, "adnl_addr", v.adnl_addr.as_ref());
        vector.push(map.into());
    }
    serialize_field(map, "list", Value::from(vector));
    Ok(())
}

fn serialize_validator_signed_temp_keys(stk: &ValidatorKeys) -> Result<Value> {
    let mut vector = Vec::<Value>::new();
    stk.iterate_with_keys(|key: UInt256, val| -> Result<bool> {
        let mut map = Map::new();
        serialize_uint256(&mut map, "map_key", &key);
        serialize_uint256(&mut map, "adnl_addr", val.key().adnl_addr());
        serialize_field(
            &mut map,
            "temp_public_key",
            hex::encode(val.key().temp_public_key().key_bytes()),
        );
        serialize_field(&mut map, "seqno", val.key().seqno());
        serialize_field(&mut map, "valid_until", val.key().valid_until());
        let (r, s) = val.signature().as_r_s_bytes();
        serialize_field(&mut map, "signature_r", hex::encode(r));
        serialize_field(&mut map, "signature_s", hex::encode(s));
        vector.push(Value::from(map));
        Ok(true)
    })?;
    Ok(vector.into())
}

fn serialize_misbehaviour_punishment_config(
    config: &MisbehaviourPunishmentConfig,
    mode: SerializationMode,
) -> Result<Value> {
    let mut map = Map::new();
    serialize_grams(&mut map, "default_flat_fine", &config.default_flat_fine, mode);
    serialize_field(&mut map, "default_proportional_fine", config.default_proportional_fine);
    serialize_field(&mut map, "severity_flat_mult", config.severity_flat_mult);
    serialize_field(&mut map, "severity_proportional_mult", config.severity_proportional_mult);
    serialize_field(&mut map, "unpunishable_interval", config.unpunishable_interval);
    serialize_field(&mut map, "long_interval", config.long_interval);
    serialize_field(&mut map, "long_flat_mult", config.long_flat_mult);
    serialize_field(&mut map, "long_proportional_mult", config.long_proportional_mult);
    serialize_field(&mut map, "medium_interval", config.medium_interval);
    serialize_field(&mut map, "medium_flat_mult", config.medium_flat_mult);
    serialize_field(&mut map, "medium_proportional_mult", config.medium_proportional_mult);
    Ok(map.into())
}

fn serialize_limits(limits: &SizeLimitsConfig) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "max_msg_bits", limits.max_msg_bits);
    serialize_field(&mut map, "max_msg_cells", limits.max_msg_cells);
    serialize_field(&mut map, "max_library_cells", limits.max_library_cells);
    serialize_field(&mut map, "max_vm_data_depth", limits.max_vm_data_depth);
    serialize_field(&mut map, "max_ext_msg_size", limits.max_ext_msg_size);
    serialize_field(&mut map, "max_ext_msg_depth", limits.max_ext_msg_depth);
    serialize_field(&mut map, "max_acc_state_cells", limits.max_acc_state_cells);
    serialize_field(&mut map, "max_mc_acc_state_cells", limits.max_mc_acc_state_cells);
    serialize_field(&mut map, "max_acc_public_libraries", limits.max_acc_public_libraries);
    serialize_field(&mut map, "defer_out_queue_size_limit", limits.defer_out_queue_size_limit);
    serialize_field(&mut map, "max_msg_extra_currencies", limits.max_msg_extra_currencies);
    serialize_field(&mut map, "max_acc_fixed_prefix_length", limits.max_acc_fixed_prefix_length);
    serialize_field(
        &mut map,
        "acc_state_cells_for_storage_dict",
        limits.acc_state_cells_for_storage_dict,
    );
    Ok(map.into())
}

fn serialize_suspended_address_list(sa: &SuspendedAddressList) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "suspended_until", sa.suspended_until());
    let mut vector: Vec<Value> = Vec::new();
    sa.iterate_addresses(|a: SuspendedAddressesKey| -> Result<bool> {
        vector.push(format!("{}:{:x}", a.workchain_id, a.address).into());
        Ok(true)
    })?;
    serialize_field(&mut map, "addresses", vector);
    Ok(map.into())
}

fn serialize_precompiled_contracts_list(pl: &PrecompiledContractsList) -> Result<Value> {
    let mut list = Map::new();
    pl.iterate(|key, gas| -> Result<bool> {
        list.insert(key.as_hex_string(), gas.into());
        Ok(true)
    })?;
    let mut map = Map::new();
    serialize_field(&mut map, "precompiled_contracts_list", list);
    Ok(map.into())
}

fn serialize_accelerator(acc: &AcceleratedConsensusConfig) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "enabled", acc.enabled);
    serialize_field(
        &mut map,
        "failed_collation_retry_timeout_ms",
        acc.failed_collation_retry_timeout_ms,
    );
    serialize_field(
        &mut map,
        "skip_rounds_count_for_collator_rotation",
        acc.skip_rounds_count_for_collator_rotation,
    );
    serialize_field(&mut map, "max_precollated_blocks", acc.max_precollated_blocks);
    Ok(map.into())
}

fn serialize_simplex_config(cfg: &SimplexConfig) -> Result<Value> {
    let mut map = Map::new();
    serialize_field(&mut map, "target_rate_ms", cfg.target_rate_ms);
    serialize_field(&mut map, "slots_per_leader_window", cfg.slots_per_leader_window);
    serialize_field(&mut map, "first_block_timeout_ms", cfg.first_block_timeout_ms);
    serialize_field(&mut map, "max_leader_window_desync", cfg.max_leader_window_desync);
    Ok(map.into())
}

fn serialize_new_consensus_config_all(cfg: &NewConsensusConfigAll) -> Result<Value> {
    let mut map = Map::new();
    if let Some(ref mc) = cfg.mc {
        map.insert("mc".to_string(), serialize_simplex_config(mc)?);
    }
    if let Some(ref shard) = cfg.shard {
        map.insert("shard".to_string(), serialize_simplex_config(shard)?);
    }
    Ok(map.into())
}

fn serialize_oracle_bridge_params(pl: &OracleBridgeParams) -> Result<Value> {
    let mut oracles = Map::new();
    pl.oracles.iterate_slices_with_keys(|key, value| {
        oracles.insert(key.as_hex_string(), value.as_hex_string().into());
        Ok(true)
    })?;
    let mut map = Map::new();
    serialize_field(&mut map, "bridge_address", pl.bridge_address.as_hex_string());
    serialize_field(
        &mut map,
        "oracle_mutlisig_address",
        pl.oracle_mutlisig_address.as_hex_string(),
    );
    serialize_field(&mut map, "external_chain_address", pl.external_chain_address.as_hex_string());
    serialize_field(&mut map, "oracles", oracles);
    Ok(map.into())
}

fn serialize_jetton_bridge_params(
    pl: &JettonBridgeParams,
    mode: SerializationMode,
) -> Result<Value> {
    let mut oracles = Map::new();
    pl.oracles.iterate_slices_with_keys(|key, value| {
        oracles.insert(key.as_hex_string(), value.as_hex_string().into());
        Ok(true)
    })?;
    let mut map = Map::new();
    serialize_field(&mut map, "oracles", oracles);
    serialize_field(&mut map, "bridge_address", pl.bridge_address.as_hex_string());
    serialize_field(&mut map, "oracles_address", pl.oracles_address.as_hex_string());
    serialize_field(&mut map, "external_chain_address", pl.external_chain_address.as_hex_string());
    serialize_field(&mut map, "state_flags", pl.state_flags);
    serialize_grams(&mut map, "bridge_burn_fee", &pl.prices.bridge_burn_fee, mode);
    serialize_grams(&mut map, "bridge_mint_fee", &pl.prices.bridge_mint_fee, mode);
    serialize_grams(
        &mut map,
        "wallet_min_tons_for_storage",
        &pl.prices.wallet_min_tons_for_storage,
        mode,
    );
    serialize_grams(&mut map, "wallet_gas_consumption", &pl.prices.wallet_gas_consumption, mode);
    serialize_grams(
        &mut map,
        "minter_min_tons_for_storage",
        &pl.prices.minter_min_tons_for_storage,
        mode,
    );
    serialize_grams(
        &mut map,
        "discover_gas_consumption",
        &pl.prices.discover_gas_consumption,
        mode,
    );
    Ok(map.into())
}

fn serialize_crypto_signature(s: &CryptoSignaturePair) -> Result<Value> {
    let mut map = Map::new();
    serialize_uint256(&mut map, "node_id", &s.node_id_short);
    let (r, s) = s.sign.as_r_s_bytes();
    serialize_field(&mut map, "r", hex::encode(r));
    serialize_field(&mut map, "s", hex::encode(s));
    Ok(map.into())
}

pub fn serialize_known_config_param(
    number: u32,
    param: Cell,
    mode: SerializationMode,
) -> Result<Option<Value>> {
    let mut map = Map::new();

    match ConfigParamEnum::construct_from_cell_and_number(param, number)? {
        ConfigParamEnum::ConfigParam0(c) => {
            return Ok(Some(c.config_addr.as_hex_string().into()));
        }
        ConfigParamEnum::ConfigParam1(c) => {
            return Ok(Some(c.elector_addr.as_hex_string().into()));
        }
        ConfigParamEnum::ConfigParam2(c) => {
            return Ok(Some(c.minter_addr.as_hex_string().into()));
        }
        ConfigParamEnum::ConfigParam3(c) => {
            return Ok(Some(c.fee_collector_addr.as_hex_string().into()));
        }
        ConfigParamEnum::ConfigParam4(c) => {
            return Ok(Some(c.dns_root_addr.as_hex_string().into()));
        }
        ConfigParamEnum::ConfigParam5(c) => {
            if let Some(addr) = &c.blackhole_addr {
                serialize_uint256(&mut map, "blackhole_addr", addr);
            }
            serialize_field(&mut map, "fee_burn_num", c.fee_burn_num);
            serialize_field(&mut map, "fee_burn_denom", c.fee_burn_denom);
        }
        ConfigParamEnum::ConfigParam6(c) => {
            serialize_grams(&mut map, "mint_new_price", &c.mint_new_price, mode);
            serialize_grams(&mut map, "mint_add_price", &c.mint_add_price, mode);
        }
        ConfigParamEnum::ConfigParam7(c) => {
            return Ok(Some(serialize_ecc(&c.to_mint, mode)?.into()));
        }
        ConfigParamEnum::ConfigParam8(c) => {
            serialize_field(&mut map, "version", c.global_version.version);
            serialize_u64(&mut map, "capabilities", &c.global_version.capabilities, mode);
        }
        ConfigParamEnum::ConfigParam9(c) => {
            return Ok(Some(serialize_mandatory_params(&c.mandatory_params)?));
        }
        ConfigParamEnum::ConfigParam10(c) => {
            return Ok(Some(serialize_mandatory_params(&c.critical_params)?));
        }
        ConfigParamEnum::ConfigParam11(c) => {
            serialize_field(
                &mut map,
                "normal_params",
                serialize_config_proposal_setup(&c.read_normal_params()?)?,
            );
            serialize_field(
                &mut map,
                "critical_params",
                serialize_config_proposal_setup(&c.read_critical_params()?)?,
            );
        }
        ConfigParamEnum::ConfigParam12(c) => {
            return Ok(Some(serialize_workchains(&c.workchains)?));
        }
        ConfigParamEnum::ConfigParam13(c) => {
            let boc = write_boc(&c.cell)?;
            serialize_field(&mut map, "boc", base64_encode(boc));
        }
        ConfigParamEnum::ConfigParam14(c) => {
            serialize_grams(
                &mut map,
                "masterchain_block_fee",
                &c.block_create_fees.masterchain_block_fee,
                mode,
            );
            serialize_grams(
                &mut map,
                "basechain_block_fee",
                &c.block_create_fees.basechain_block_fee,
                mode,
            );
        }
        ConfigParamEnum::ConfigParam15(c) => {
            serialize_field(&mut map, "validators_elected_for", c.validators_elected_for);
            serialize_field(&mut map, "elections_start_before", c.elections_start_before);
            serialize_field(&mut map, "elections_end_before", c.elections_end_before);
            serialize_field(&mut map, "stake_held_for", c.stake_held_for);
        }
        ConfigParamEnum::ConfigParam16(c) => {
            serialize_field(&mut map, "max_validators", c.max_validators.as_u32());
            serialize_field(&mut map, "max_main_validators", c.max_main_validators.as_u32());
            serialize_field(&mut map, "min_validators", c.min_validators.as_u32());
        }
        ConfigParamEnum::ConfigParam17(c) => {
            serialize_grams(&mut map, "min_stake", &c.min_stake, mode);
            serialize_grams(&mut map, "max_stake", &c.max_stake, mode);
            serialize_grams(&mut map, "min_total_stake", &c.min_total_stake, mode);
            serialize_field(&mut map, "max_stake_factor", c.max_stake_factor);
        }
        ConfigParamEnum::ConfigParam18(c) => {
            return Ok(Some(serialize_storage_prices(&c.map, mode)?));
        }
        ConfigParamEnum::ConfigParam19(c) => {
            serialize_field(&mut map, "global_id", c);
        }
        ConfigParamEnum::ConfigParam20(c) => {
            serialize_gas_limits_prices(&mut map, &c, mode);
        }
        ConfigParamEnum::ConfigParam21(c) => {
            serialize_gas_limits_prices(&mut map, &c, mode);
        }
        ConfigParamEnum::ConfigParam22(c) => {
            serialize_block_limits(&mut map, &c)?;
        }
        ConfigParamEnum::ConfigParam23(c) => {
            serialize_block_limits(&mut map, &c)?;
        }
        ConfigParamEnum::ConfigParam24(c) => {
            serialize_msg_fwd_prices(&mut map, &c, mode)?;
        }
        ConfigParamEnum::ConfigParam25(c) => {
            serialize_msg_fwd_prices(&mut map, &c, mode)?;
        }
        ConfigParamEnum::ConfigParam28(c) => {
            serialize_field(&mut map, "shuffle_mc_validators", c.shuffle_mc_validators);
            serialize_field(&mut map, "isolate_mc_validators", c.isolate_mc_validators);
            serialize_field(&mut map, "mc_catchain_lifetime", c.mc_catchain_lifetime);
            serialize_field(&mut map, "shard_catchain_lifetime", c.shard_catchain_lifetime);
            serialize_field(&mut map, "shard_validators_lifetime", c.shard_validators_lifetime);
            serialize_field(&mut map, "shard_validators_num", c.shard_validators_num);
        }
        ConfigParamEnum::ConfigParam29(c) => {
            serialize_field(&mut map, "new_catchain_ids", c.new_catchain_ids);
            serialize_field(&mut map, "round_candidates", c.round_candidates);
            serialize_field(&mut map, "next_candidate_delay_ms", c.next_candidate_delay_ms);
            serialize_field(&mut map, "consensus_timeout_ms", c.consensus_timeout_ms);
            serialize_field(&mut map, "fast_attempts", c.fast_attempts);
            serialize_field(&mut map, "attempt_duration", c.attempt_duration);
            serialize_field(&mut map, "catchain_max_deps", c.catchain_max_deps);
            serialize_field(&mut map, "max_block_bytes", c.max_block_bytes);
            serialize_field(&mut map, "max_collated_bytes", c.max_collated_bytes);
            serialize_field(&mut map, "catchain_max_blocks_coeff", c.catchain_max_blocks_coeff);
            serialize_field(&mut map, "proto_version", c.proto_version);
        }
        ConfigParamEnum::ConfigParam30(ref c) => {
            return Ok(Some(serialize_new_consensus_config_all(c)?));
        }
        ConfigParamEnum::ConfigParam31(c) => {
            return Ok(Some(serialize_fundamental_smc_addresses(&c.fundamental_smc_addr)?));
        }
        ConfigParamEnum::ConfigParam32(c) => {
            serialize_validators_set(&mut map, &c.prev_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam33(c) => {
            serialize_validators_set(&mut map, &c.prev_temp_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam34(c) => {
            serialize_validators_set(&mut map, &c.cur_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam35(c) => {
            serialize_validators_set(&mut map, &c.cur_temp_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam36(c) => {
            serialize_validators_set(&mut map, &c.next_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam37(c) => {
            serialize_validators_set(&mut map, &c.next_temp_validators, mode)?;
        }
        ConfigParamEnum::ConfigParam39(c) => {
            return Ok(Some(serialize_validator_signed_temp_keys(&c.validator_keys)?));
        }
        ConfigParamEnum::ConfigParam40(ref c) => {
            return Ok(Some(serialize_misbehaviour_punishment_config(c, mode)?));
        }
        ConfigParamEnum::ConfigParam43(ref c) => {
            return Ok(Some(serialize_limits(c)?));
        }
        ConfigParamEnum::ConfigParam44(ref c) => {
            return Ok(Some(serialize_suspended_address_list(c)?));
        }
        ConfigParamEnum::ConfigParam45(ref c) => {
            return Ok(Some(serialize_precompiled_contracts_list(c)?));
        }
        ConfigParamEnum::ConfigParam63(ref c) => {
            return Ok(Some(serialize_accelerator(c)?));
        }
        ConfigParamEnum::ConfigParam71(ref c) => {
            return Ok(Some(serialize_oracle_bridge_params(c)?));
        }
        ConfigParamEnum::ConfigParam72(ref c) => {
            return Ok(Some(serialize_oracle_bridge_params(c)?));
        }
        ConfigParamEnum::ConfigParam73(ref c) => {
            return Ok(Some(serialize_oracle_bridge_params(c)?));
        }
        ConfigParamEnum::ConfigParam79(ref c) => {
            return Ok(Some(serialize_jetton_bridge_params(c, mode)?));
        }
        ConfigParamEnum::ConfigParam81(ref c) => {
            return Ok(Some(serialize_jetton_bridge_params(c, mode)?));
        }
        ConfigParamEnum::ConfigParam82(ref c) => {
            return Ok(Some(serialize_jetton_bridge_params(c, mode)?));
        }
        _ => return Ok(None),
    }

    Ok(Some(map.into()))
}

fn serialize_unknown_config_param(number: u32, param: &Cell) -> Result<Value> {
    let mut map = Map::new();

    map.insert("number".to_string(), number.into());
    serialize_cell(&mut map, "boc", Some(param), false)?;

    Ok(map.into())
}

fn serialize_block_ref(blk_ref: &ExtBlkRef, key: Option<bool>, mode: SerializationMode) -> Value {
    let mut blk_ref_map = Map::new();
    serialize_lt(&mut blk_ref_map, "end_lt", blk_ref.end_lt, mode);
    blk_ref_map.insert("seq_no".to_string(), blk_ref.seq_no.into());
    if let Some(key) = key {
        blk_ref_map.insert("key".to_string(), key.into());
    }
    serialize_id(&mut blk_ref_map, "root_hash", Some(&blk_ref.root_hash));
    serialize_id(&mut blk_ref_map, "file_hash", Some(&blk_ref.file_hash));
    blk_ref_map.into()
}

fn serialize_shard_hashes(
    map: &mut Map<String, Value>,
    id_str: &str,
    hashes: &ShardHashes,
    mode: SerializationMode,
) -> Result<()> {
    let mut shard_hashes = Vec::new();
    let mut min_gen_utime = u32::MAX;
    let mut max_gen_utime = 0;
    hashes.iterate_with_keys(
        &mut |key: i32, InRefValue(tree): InRefValue<BinTree<ShardDescr>>| {
            tree.iterate(&mut |shard: SliceData, descr| {
                if let Ok(descr) = serialize_shard_descr(&descr, mode) {
                    let shard_ident = ShardIdent::with_prefix_slice(key, shard)?;
                    shard_hashes.push(serde_json::json!({
                        "workchain_id": key,
                        "shard": shard_ident.shard_prefix_as_str_with_tag(),
                        "descr": descr,
                    }));
                }
                min_gen_utime = std::cmp::min(min_gen_utime, descr.gen_utime);
                max_gen_utime = std::cmp::max(max_gen_utime, descr.gen_utime);
                Ok(true)
            })
        },
    )?;
    if !shard_hashes.is_empty() {
        map.insert(id_str.to_string(), shard_hashes.into());
        serialize_field(map, "min_shard_gen_utime", min_gen_utime);
        serialize_field(map, "max_shard_gen_utime", max_gen_utime);
    }

    Ok(())
}

pub fn debug_possible_config_params(config_params: &ConfigParams) -> Result<String> {
    let mode = SerializationMode::Debug;
    let mut known_cp_map = Map::new();
    for num in 0..100 {
        if let Ok(Some(cp)) = config_params.config_params.get(num.write_to_bitstring()?) {
            if let Some(cp) = cp.reference_opt(0) {
                if let Ok(Some(cp)) = serialize_known_config_param(num, cp.clone(), mode) {
                    known_cp_map.insert(format!("p{}", num), cp);
                }
            }
        }
    }
    let json = serde_json::json!({"config": known_cp_map});
    Ok(serde_json::to_string_pretty(&json)?)
}

pub fn serialize_config(
    map: &mut Map<String, Value>,
    config: &ConfigParams,
    mode: SerializationMode,
) -> Result<()> {
    serialize_id(map, "config_addr", Some(&config.config_addr));
    let mut known_cp_map = Map::new();
    let mut unknown_cp_vec = Vec::new();
    config.config_params.iterate_slices(|mut num, mut cp_ref| -> Result<bool> {
        //println!("key {}", num);
        let num = num.get_next_u32()?;
        let cp = cp_ref.checked_drain_reference()?;
        if let Some(cp) = serialize_known_config_param(num, cp.clone(), mode)? {
            known_cp_map.insert(format!("p{}", num), cp);
        } else {
            unknown_cp_vec.push(serialize_unknown_config_param(num, &cp)?);
        }
        Ok(true)
    })?;
    serialize_field(map, "config", known_cp_map);
    if !unknown_cp_vec.is_empty() {
        serialize_field(map, "unknown_config", unknown_cp_vec);
    }
    Ok(())
}

fn serialize_counters(counters: &Counters, mode: SerializationMode) -> Value {
    let mut map = Map::new();
    map.insert("valid".to_string(), counters.is_valid().into());
    map.insert("last_updated".to_string(), counters.last_updated().into());
    serialize_u64(&mut map, "total", &counters.total(), mode);
    map.insert("cnt2048".to_string(), counters.cnt2048().into());
    map.insert("cnt65536".to_string(), counters.cnt65536().into());
    map.into()
}

fn serialize_block_create_stats(
    map: &mut Map<String, Value>,
    id_str: &str,
    stats: &BlockCreateStats,
    mode: SerializationMode,
) -> Result<()> {
    let mut counters = Vec::new();
    stats.counters.iterate_slices_with_keys(|ref mut key, ref mut value| -> Result<bool> {
        let value = CreatorStats::construct_from(value)?;
        counters.push(serde_json::json!({
            "public_key": format!("{:x}", key),
            "mc_blocks": serialize_counters(value.mc_blocks(), mode),
            "shard_blocks": serialize_counters(value.shard_blocks(), mode),
        }));
        Ok(true)
    })?;
    map.insert(id_str.to_string(), counters.into());
    Ok(())
}

fn serialize_shard_accounts(
    map: &mut Map<String, Value>,
    id_str: &str,
    shard_accounts: &ShardAccounts,
    mode: SerializationMode,
) -> Result<()> {
    let mut accounts = Vec::new();
    shard_accounts.iterate_objects(&mut |ref mut value: ShardAccount| -> Result<bool> {
        let account = value.read_account()?;
        let account_set = AccountSerializationSet {
            account,
            prev_code_hash: None,
            boc: write_boc(&value.account_cell())?,
            proof: None,
        };
        let mut account = db_serialize_account_ex("id", &account_set, mode)?;
        account.remove("json_version");
        accounts.push(account);
        Ok(true)
    })?;
    map.insert(id_str.to_string(), accounts.into());
    Ok(())
}

fn serialize_libraries(
    map: &mut Map<String, Value>,
    id_str: &str,
    libraries: &Libraries,
) -> Result<()> {
    let mut libraries_vec = Vec::new();
    libraries.iterate_slices_with_keys(|ref mut key, ref mut value| -> Result<bool> {
        let value = LibDescr::construct_from(value)?;
        let mut publishers = Vec::new();
        value.publishers().iterate_slices_with_keys(|ref mut key, _| -> Result<bool> {
            publishers.push(key.as_hex_string());
            Ok(true)
        })?;

        libraries_vec.push(serde_json::json!({
            "hash": key.as_hex_string(),
            "publishers": publishers,
            "lib": base64_encode(write_boc(value.lib())?)
        }));
        Ok(true)
    })?;
    map.insert(id_str.to_string(), libraries_vec.into());
    Ok(())
}

fn serialize_out_msg_queue_info(
    map: &mut Map<String, Value>,
    id_str: &str,
    info: &OutMsgQueueInfo,
    mode: SerializationMode,
) -> Result<()> {
    let mut out_queue = Vec::new();
    info.out_queue().iterate_with_keys(&mut |ref mut key: OutMsgQueueKey,
                                              value: EnqueuedMsg|
     -> Result<bool> {
        let mut msg_map = serialize_envelope_msg(&value.read_envelope_msg()?, mode);
        msg_map.insert("dest_workchain".to_string(), key.workchain_id.into());
        msg_map.insert("dest_addr_prefix".to_string(), shard_to_string(key.prefix).into());
        serialize_lt(&mut msg_map, "enqueued_lt", value.enqueued_lt(), mode);
        out_queue.push(msg_map);
        Ok(true)
    })?;

    let mut proc_info = Vec::new();
    info.proc_info().iterate_slices_with_keys(&mut |mut key: SliceData,
                                                     mut value: SliceData|
     -> Result<bool> {
        let mut processed_map = Map::new();
        let value = ProcessedUpto::construct_from(&mut value)?;
        processed_map.insert("shard".to_string(), shard_to_string(key.get_next_u64()?).into());
        processed_map.insert("mc_seqno".to_string(), key.get_next_u32()?.into());
        serialize_lt(&mut processed_map, "last_msg_lt", value.last_msg_lt, mode);
        processed_map
            .insert("last_msg_hash".to_string(), value.last_msg_hash.as_hex_string().into());
        proc_info.push(processed_map);
        Ok(true)
    })?;

    map.insert(
        id_str.to_string(),
        serde_json::json!({
            "out_queue": out_queue,
            "proc_info": proc_info,
        }),
    );

    Ok(())
}

fn serialize_mc_state_extra(
    map: &mut Map<String, Value>,
    id_str: &str,
    extra: &McStateExtra,
    mode: SerializationMode,
) -> Result<()> {
    let mut extra_map = Map::new();
    serialize_shard_hashes(&mut extra_map, "shard_hashes", extra.shards(), mode)?;
    serialize_config(&mut extra_map, &extra.config, mode)?;
    serialize_field(
        &mut extra_map,
        "validator_list_hash_short",
        extra.validator_info.validator_list_hash_short,
    );
    serialize_field(&mut extra_map, "catchain_seqno", extra.validator_info.catchain_seqno);
    serialize_field(&mut extra_map, "nx_cc_updated", extra.validator_info.nx_cc_updated);
    // `prev_blocks` field is quite huge and not useful. Don't need to serialize it
    //serialize_field(&mut master_map, "prev_blocks", serialize_old_mc_blocks_info(&master.prev_blocks, mode)?);
    serialize_field(&mut extra_map, "after_key_block", extra.after_key_block);
    if let Some(block_ref) = &extra.last_key_block {
        serialize_field(
            &mut extra_map,
            "last_key_block",
            serialize_block_ref(block_ref, None, mode),
        );
    }
    if let Some(stats) = &extra.block_create_stats {
        serialize_block_create_stats(&mut extra_map, "block_create_stats", stats, mode)?;
    }
    serialize_cc(&mut extra_map, "global_balance", &extra.global_balance, mode)?;
    if !extra.validators_stat.is_empty() {
        serialize_field(
            &mut extra_map,
            "validators_unreliability",
            serialize_validators_stat(&extra.validators_stat)?,
        );
    }
    map.insert(id_str.to_string(), extra_map.into());
    Ok(())
}

fn serialize_file_hash(map: &mut Map<String, Value>, file_hash: Option<&UInt256>, boc: &[u8]) {
    match file_hash {
        Some(file_hash) => serialize_id(map, "file_hash", Some(file_hash)),
        None => serialize_id(map, "file_hash", Some(&UInt256::calc_file_hash(boc))),
    }
}

#[derive(Default)]
pub struct BlockSerializationSet {
    pub block: Block,
    pub id: BlockId,
    pub status: BlockProcessingStatus,
    pub boc: Vec<u8>,
}

pub struct BlockSerializationSetFH<'a> {
    pub block: &'a Block,
    pub id: &'a BlockId,
    pub status: BlockProcessingStatus,
    pub boc: &'a [u8],
    pub file_hash: Option<&'a UInt256>,
}

impl<'a> From<&'a BlockSerializationSet> for BlockSerializationSetFH<'a> {
    fn from(set: &'a BlockSerializationSet) -> Self {
        BlockSerializationSetFH {
            block: &set.block,
            id: &set.id,
            status: set.status,
            boc: &set.boc,
            file_hash: None,
        }
    }
}

pub fn debug_block_map(block: Block) -> Result<Map<String, Value>> {
    let root_cell = block.serialize()?;
    let set = BlockSerializationSet {
        block,
        id: root_cell.repr_hash(),
        status: BlockProcessingStatus::Finalized,
        boc: Vec::new(),
    };
    db_serialize_block_ex("id", &set, SerializationMode::Debug)
}

pub fn debug_block(block: Block) -> Result<String> {
    let map = debug_block_map(block)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn serialize_config_param(config: &ConfigParams, config_number: u32) -> Result<String> {
    let mut master_map = Map::new();
    let key = SliceData::load_builder(config_number.write_to_new_cell()?)?;
    if let Some(mut cell) = config.config_params.get(key)? {
        let param = cell.checked_drain_reference()?;
        if let Some(cp) =
            serialize_known_config_param(config_number, param, SerializationMode::Standart)?
        {
            master_map.insert(format!("p{}", &config_number), cp);
        }
    }
    let json = serde_json::to_string_pretty(&master_map)?;
    Ok(json)
}

pub fn debug_block_full(block: &Block) -> Result<String> {
    let root_cell = block.serialize()?;
    let set = BlockSerializationSet {
        block: block.clone(),
        id: root_cell.repr_hash(),
        status: BlockProcessingStatus::Finalized,
        boc: Vec::new(),
    };
    let map = db_serialize_block_ex("id", &set, SerializationMode::Debug)?;

    let mut text = format!("Block: {:#}\n", serde_json::json!(map));
    let extra = block.read_extra()?;
    let in_msgs = extra.read_in_msg_descr()?;
    in_msgs.iterate_objects(|in_msg| {
        let msg = in_msg.read_message()?;
        text += &format!("InMsg: {}\n", debug_message(msg)?);
        Ok(true)
    })?;
    let out_msgs = extra.read_out_msg_descr()?;
    out_msgs.iterate_objects(|out_msg| {
        if let Some(msg) = out_msg.read_message()? {
            text += &format!("OutMsg: {}\n", debug_message(msg)?);
        }
        Ok(true)
    })?;
    let acc_blocks = extra.read_account_blocks()?;
    acc_blocks.iterate_objects(|block| {
        block.transactions().iterate_objects(|InRefValue(tr)| {
            text += &format!("Transaction: {}\n", debug_transaction(tr)?);
            Ok(true)
        })
    })?;
    Ok(text)
}

pub fn db_serialize_block<'a>(
    id_str: &'static str,
    set: impl Into<BlockSerializationSetFH<'a>>,
) -> Result<Map<String, Value>> {
    db_serialize_block_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_block_ex<'a>(
    id_str: &'static str,
    set: impl Into<BlockSerializationSetFH<'a>>,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let set: BlockSerializationSetFH = set.into();
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    serialize_id(&mut map, id_str, Some(set.id));
    serialize_file_hash(&mut map, set.file_hash, set.boc);
    serialize_field(&mut map, "status", set.status as u8);
    if mode.is_q_server() {
        serialize_field(
            &mut map,
            "status_name",
            match set.status {
                BlockProcessingStatus::Unknown => "unknown",
                BlockProcessingStatus::Proposed => "proposed",
                BlockProcessingStatus::Finalized => "finalized",
                BlockProcessingStatus::Refused => "refused",
            },
        );
    }
    map.insert("boc".to_string(), base64_encode(set.boc).into());
    map.insert("global_id".to_string(), set.block.global_id.into());
    let block_info = set.block.read_info()?;
    map.insert("version".to_string(), block_info.version().into());
    map.insert("after_merge".to_string(), block_info.after_merge().into());
    map.insert("before_split".to_string(), block_info.before_split().into());
    map.insert("after_split".to_string(), block_info.after_split().into());
    map.insert("want_split".to_string(), block_info.want_split().into());
    map.insert("want_merge".to_string(), block_info.want_merge().into());
    map.insert("key_block".to_string(), block_info.key_block().into());
    map.insert("vert_seqno_incr".to_string(), block_info.vert_seqno_incr().into());
    map.insert("seq_no".to_string(), block_info.seq_no().into());
    map.insert("vert_seq_no".to_string(), block_info.vert_seq_no().into());
    map.insert("gen_utime".to_string(), block_info.gen_utime().into());
    serialize_lt(&mut map, "start_lt", block_info.start_lt(), mode);
    serialize_lt(&mut map, "end_lt", block_info.end_lt(), mode);
    map.insert(
        "gen_validator_list_hash_short".to_string(),
        block_info.gen_validator_list_hash_short().into(),
    );
    map.insert("gen_catchain_seqno".to_string(), block_info.gen_catchain_seqno().into());
    map.insert("min_ref_mc_seqno".to_string(), block_info.min_ref_mc_seqno().into());
    map.insert("prev_key_block_seqno".to_string(), block_info.prev_key_block_seqno().into());
    map.insert("workchain_id".to_string(), block_info.shard().workchain_id().into());
    map.insert("shard".to_string(), block_info.shard().shard_prefix_as_str_with_tag().into());

    if let Some(gs) = block_info.gen_software() {
        serialize_field(&mut map, "gen_software_version", gs.version);
        serialize_u64(&mut map, "gen_software_capabilities", &gs.capabilities, mode);
    }

    let prev_block_ref = block_info.read_prev_ref()?;
    map.insert("prev_seq_no".to_string(), prev_block_ref.prev1()?.seq_no.into());

    let (vert_prev1, vert_prev2) = match &block_info.read_prev_vert_ref()? {
        Some(blk) => (Some(blk.prev1()?), blk.prev2()?),
        None => (None, None),
    };
    [
        ("master_ref", block_info.read_master_ref()?.map(|blk| blk.master)),
        ("prev_ref", Some(prev_block_ref.prev1()?)),
        ("prev_alt_ref", prev_block_ref.prev2()?),
        ("prev_vert_ref", vert_prev1),
        ("prev_vert_alt_ref", vert_prev2),
    ]
    .iter()
    .for_each(|(id_str, blk_ref)| {
        if let Some(blk_ref) = blk_ref {
            map.insert(id_str.to_string(), serialize_block_ref(blk_ref, None, mode));
        }
    });
    let value_flow = set.block.read_value_flow()?;
    let mut value_map = Map::new();
    serialize_cc(&mut value_map, "from_prev_blk", &value_flow.from_prev_blk, mode)?;
    serialize_cc(&mut value_map, "to_next_blk", &value_flow.to_next_blk, mode)?;
    serialize_cc(&mut value_map, "imported", &value_flow.imported, mode)?;
    serialize_cc(&mut value_map, "exported", &value_flow.exported, mode)?;
    serialize_cc(&mut value_map, "fees_collected", &value_flow.fees_collected, mode)?;
    serialize_cc(&mut value_map, "fees_imported", &value_flow.fees_imported, mode)?;
    serialize_cc(&mut value_map, "recovered", &value_flow.recovered, mode)?;
    serialize_cc(&mut value_map, "created", &value_flow.created, mode)?;
    serialize_cc(&mut value_map, "minted", &value_flow.minted, mode)?;
    map.insert("value_flow".to_string(), value_map.into());

    let state_update = set.block.read_state_update()?;
    serialize_id(&mut map, "old_hash", Some(&state_update.old_hash));
    serialize_id(&mut map, "new_hash", Some(&state_update.new_hash));
    map.insert("old_depth".to_string(), state_update.old_depth.into());
    map.insert("new_depth".to_string(), state_update.new_depth.into());

    let extra = set.block.read_extra()?;
    let mut msgs = vec![];
    extra.read_in_msg_descr()?.iterate_objects(|ref msg| {
        msgs.push(serialize_in_msg(msg, mode)?);
        Ok(true)
    })?;
    map.insert("in_msg_descr".to_string(), msgs.into());

    let mut msgs = vec![];
    extra.read_out_msg_descr()?.iterate_objects(|ref msg| {
        msgs.push(serialize_out_msg(msg, mode)?);
        Ok(true)
    })?;

    map.insert("out_msg_descr".to_string(), msgs.into());
    let mut total_tr_count = 0;
    let mut account_blocks = Vec::new();
    extra.read_account_blocks()?.iterate_objects(|account_block| {
        let workchain = block_info.shard().workchain_id();
        let address = construct_address(workchain, account_block.account_addr())?;
        let mut map = Map::new();
        serialize_field(&mut map, "account_addr", address.to_string());
        let mut transactions = Vec::new();
        account_block.transaction_iterate_full(|key, transaction_cell, cc| {
            let mut map = Map::new();
            serialize_lt(&mut map, "lt", key, mode);
            serialize_id(&mut map, "transaction_id", Some(&transaction_cell.repr_hash()));
            serialize_cc(&mut map, "total_fees", &cc, mode)?;
            transactions.push(map);
            Ok(true)
        })?;
        let tr_count = transactions.len();
        serialize_field(&mut map, "transactions", transactions);
        let state_update = account_block.read_state_update()?;
        serialize_id(&mut map, "old_hash", Some(&state_update.old_hash));
        serialize_id(&mut map, "new_hash", Some(&state_update.new_hash));
        serialize_field(&mut map, "tr_count", tr_count);
        account_blocks.push(map);
        total_tr_count += tr_count;
        Ok(true)
    })?;
    if !account_blocks.is_empty() {
        serialize_field(&mut map, "account_blocks", account_blocks);
    }
    serialize_field(&mut map, "tr_count", total_tr_count);

    serialize_id(&mut map, "rand_seed", Some(&extra.rand_seed));
    serialize_id(&mut map, "created_by", Some(&extra.created_by));

    if let Some(master) = extra.read_custom()? {
        let mut master_map = Map::new();
        serialize_shard_hashes(&mut master_map, "shard_hashes", master.hashes(), mode)?;
        let mut fees_map = Vec::new();
        master.fees().iterate_slices(|mut key, ref mut shard| {
            let workchain_id = key.get_next_i32()?;
            let shard_prefix = key.get_next_u64()?;
            let shard = ShardFeeCreated::construct_from(shard)?;
            let mut map = Map::new();
            map.insert("workchain_id".to_string(), workchain_id.into());
            map.insert("shard".to_string(), shard_to_string(shard_prefix).into());
            serialize_cc(&mut map, "fees", &shard.fees, mode)?;
            serialize_cc(&mut map, "create", &shard.create, mode)?;
            fees_map.push(map);
            Ok(true)
        })?;
        if !fees_map.is_empty() {
            master_map.insert("shard_fees".to_string(), fees_map.into());
        }
        let mut crypto_signs = vec![];
        master.prev_blk_signatures().iterate(|s| {
            crypto_signs.push(serialize_crypto_signature(&s)?);
            Ok(true)
        })?;
        master_map.insert("prev_blk_signatures".to_string(), crypto_signs.into());
        if let Some(msg) = &master.read_recover_create_msg()? {
            master_map.insert("recover_create_msg".to_string(), serialize_in_msg(msg, mode)?);
        }
        if let Some(msg) = &master.read_mint_msg()? {
            master_map.insert("mint_msg".to_string(), serialize_in_msg(msg, mode)?);
        }
        if let Some(config) = master.config() {
            serialize_config(&mut master_map, config, mode)?;
        }
        map.insert("master".to_string(), master_map.into());
    }
    Ok(map)
}

#[derive(Default)]
pub struct TransactionSerializationSet {
    pub transaction: Transaction,
    pub id: TransactionId,
    pub status: TransactionProcessingStatus,
    pub block_id: Option<BlockId>,
    pub workchain_id: i32,
    pub boc: Vec<u8>,
    pub proof: Option<Vec<u8>>,
}

pub struct TransactionSerializationSetEx<'a> {
    pub transaction: &'a Transaction,
    pub id: &'a TransactionId,
    pub status: TransactionProcessingStatus,
    pub block_id: Option<&'a BlockId>,
    pub workchain_id: Option<i32>,
    pub boc: &'a [u8],
    pub proof: Option<&'a [u8]>,
}

impl<'a> From<&'a TransactionSerializationSet> for TransactionSerializationSetEx<'a> {
    fn from(set: &'a TransactionSerializationSet) -> Self {
        TransactionSerializationSetEx {
            transaction: &set.transaction,
            id: &set.id,
            status: set.status,
            block_id: set.block_id.as_ref(),
            workchain_id: Some(set.workchain_id),
            boc: &set.boc,
            proof: set.proof.as_deref(),
        }
    }
}

pub fn debug_transaction(transaction: Transaction) -> Result<String> {
    let root_cell = transaction.serialize()?;
    let set = TransactionSerializationSetEx {
        transaction: &transaction,
        id: &root_cell.repr_hash(),
        status: TransactionProcessingStatus::Finalized,
        block_id: None,
        workchain_id: None,
        boc: &[],
        proof: None,
    };
    let map = db_serialize_transaction_ex("id", set, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn db_serialize_transaction<'a>(
    id_str: &'static str,
    set: impl Into<TransactionSerializationSetEx<'a>>,
) -> Result<Map<String, Value>> {
    db_serialize_transaction_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_transaction_ex<'a>(
    id_str: &'static str,
    set: impl Into<TransactionSerializationSetEx<'a>>,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let set: TransactionSerializationSetEx = set.into();
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    serialize_id(&mut map, id_str, Some(set.id));
    serialize_id(&mut map, "block_id", set.block_id);
    if let Some(proof) = &set.proof {
        serialize_field(&mut map, "proof", base64_encode(proof));
    }
    serialize_field(&mut map, "boc", base64_encode(set.boc));
    serialize_field(&mut map, "status", set.status as u8);
    if mode.is_q_server() {
        serialize_field(
            &mut map,
            "status_name",
            match set.status {
                TransactionProcessingStatus::Unknown => "unknown",
                TransactionProcessingStatus::Preliminary => "preliminary",
                TransactionProcessingStatus::Proposed => "proposed",
                TransactionProcessingStatus::Finalized => "finalized",
                TransactionProcessingStatus::Refused => "refused",
            },
        );
    }
    let mut ext_in_msg_fee = None;
    let (tr_type, tr_type_name) = match &set.transaction.read_description()? {
        TransactionDescr::Ordinary(tr) => {
            let mut fees = set.transaction.total_fees().grams;
            if let Some(fee) = serialize_storage_phase(&mut map, tr.storage_ph.as_ref(), mode) {
                fees.sub(fee)?;
            }
            serialize_credit_phase(&mut map, tr.credit_ph.as_ref(), mode)?;
            if let Some(fee) = serialize_compute_phase(&mut map, Some(&tr.compute_ph), mode) {
                fees.sub(fee)?;
            }
            if let Some(fee) = serialize_action_phase(&mut map, tr.action.as_ref(), mode) {
                fees.sub(fee)?;
            }
            ext_in_msg_fee = Some(fees);
            serialize_bounce_phase(&mut map, tr.bounce.as_ref(), mode);
            serialize_field(&mut map, "credit_first", tr.credit_first);
            serialize_field(&mut map, "aborted", tr.aborted);
            serialize_field(&mut map, "destroyed", tr.destroyed);
            (0b0000, "ordinary")
        }
        TransactionDescr::Storage(tr) => {
            serialize_storage_phase(&mut map, Some(tr), mode);
            (0b0001, "storage")
        }
        TransactionDescr::TickTock(tr) => {
            serialize_storage_phase(&mut map, Some(&tr.storage), mode);
            serialize_compute_phase(&mut map, Some(&tr.compute_ph), mode);
            serialize_action_phase(&mut map, tr.action.as_ref(), mode);
            serialize_field(&mut map, "aborted", tr.aborted);
            serialize_field(&mut map, "destroyed", tr.destroyed);
            match &tr.tt {
                TransactionTickTock::Tick => (0b0010, "tick"),
                TransactionTickTock::Tock => (0b0011, "tock"),
            }
        }
        TransactionDescr::SplitPrepare(tr) => {
            serialize_split_info(&mut map, &tr.split_info);
            serialize_compute_phase(&mut map, Some(&tr.compute_ph), mode);
            serialize_action_phase(&mut map, tr.action.as_ref(), mode);
            serialize_field(&mut map, "aborted", tr.aborted);
            serialize_field(&mut map, "destroyed", tr.destroyed);
            (0b0100, "splitPrepare")
        }
        TransactionDescr::SplitInstall(tr) => {
            serialize_split_info(&mut map, &tr.split_info);
            serialize_id(
                &mut map,
                "prepare_transaction",
                tr.prepare_transaction.hash().ok().as_ref(),
            );
            serialize_field(&mut map, "installed", tr.installed);
            (0b0101, "splitInstall")
        }
        TransactionDescr::MergePrepare(tr) => {
            serialize_split_info(&mut map, &tr.split_info);
            serialize_storage_phase(&mut map, Some(&tr.storage_ph), mode);
            serialize_field(&mut map, "aborted", tr.aborted);
            (0b0110, "mergePrepare")
        }
        TransactionDescr::MergeInstall(tr) => {
            serialize_split_info(&mut map, &tr.split_info);
            serialize_id(
                &mut map,
                "prepare_transaction",
                tr.prepare_transaction.hash().ok().as_ref(),
            );
            serialize_credit_phase(&mut map, tr.credit_ph.as_ref(), mode)?;
            serialize_compute_phase(&mut map, Some(&tr.compute_ph), mode);
            serialize_action_phase(&mut map, tr.action.as_ref(), mode);
            serialize_field(&mut map, "aborted", tr.aborted);
            serialize_field(&mut map, "destroyed", tr.destroyed);
            (0b0111, "mergeInstall")
        }
    };
    serialize_field(&mut map, "tr_type", tr_type);
    if mode.is_q_server() {
        serialize_field(&mut map, "tr_type_name", tr_type_name);
    }
    serialize_lt(&mut map, "lt", set.transaction.logical_time(), mode);
    serialize_id(&mut map, "prev_trans_hash", Some(set.transaction.prev_trans_hash()));
    serialize_lt(&mut map, "prev_trans_lt", set.transaction.prev_trans_lt(), mode);
    serialize_field(&mut map, "now", set.transaction.now());
    serialize_field(&mut map, "outmsg_cnt", set.transaction.msg_count());
    serialize_account_status(&mut map, "orig_status", &set.transaction.orig_status, mode);
    serialize_account_status(&mut map, "end_status", &set.transaction.end_status, mode);
    let mut balance_delta = SignedCurrencyCollection::new();
    let mut address_from_message = None;
    if !set.transaction.in_msg.is_empty() {
        let msg = &set.transaction.in_msg;
        serialize_id(&mut map, "in_msg", Some(&msg.hash()));

        let msg = msg.read_struct()?;
        if let Some(value) = msg.get_value() {
            balance_delta.add(&SignedCurrencyCollection::from_cc(value)?);
        }
        address_from_message = msg.dst_ref().cloned();

        if msg.is_inbound_external() {
            serialize_grams(&mut map, "ext_in_msg_fee", &ext_in_msg_fee.unwrap_or_default(), mode);
        }
    }
    let mut out_ids = vec![];
    set.transaction.out_msgs.iterate_slices(|slice| {
        if let Some(cell) = slice.reference_opt(0) {
            out_ids.push(cell.repr_hash().as_hex_string());
            let msg = Message::construct_from_cell(cell)?;
            if let Some(value) = msg.get_value() {
                balance_delta.sub(&SignedCurrencyCollection::from_cc(value)?);
            }
            if let Some(fwd_fee) = get_msg_fees(&msg) {
                balance_delta.grams -= fwd_fee.as_u128();
            }
            if address_from_message.is_none() {
                address_from_message = msg.src_ref().cloned();
            }
        }
        Ok(true)
    })?;
    serialize_field(&mut map, "out_msgs", out_ids);
    if let Some(workchain_id) = set.workchain_id {
        let account_addr = construct_address(workchain_id, set.transaction.account_id().clone())?;
        serialize_field(&mut map, "account_addr", account_addr.to_string());
        serialize_field(&mut map, "workchain_id", workchain_id);
    } else if let Some(address) = address_from_message {
        serialize_field(&mut map, "account_addr", address.to_string());
        serialize_field(&mut map, "workchain_id", address.workchain_id());
    } else {
        serialize_field(&mut map, "account_id", set.transaction.account_id().as_hex_string());
    }
    serialize_cc(&mut map, "total_fees", set.transaction.total_fees(), mode)?;
    balance_delta.sub(&SignedCurrencyCollection::from_cc(set.transaction.total_fees())?);
    serialize_scc(&mut map, "balance_delta", &balance_delta, mode);
    let state_update = set.transaction.read_state_update()?;
    serialize_id(&mut map, "old_hash", Some(&state_update.old_hash));
    serialize_id(&mut map, "new_hash", Some(&state_update.new_hash));
    Ok(map)
}

fn serialize_account_status(
    map: &mut Map<String, Value>,
    name: &'static str,
    status: &AccountStatus,
    mode: SerializationMode,
) {
    serialize_field(
        map,
        name,
        match status {
            AccountStatus::AccStateUninit => 0b00,
            AccountStatus::AccStateFrozen => 0b10,
            AccountStatus::AccStateActive => 0b01,
            AccountStatus::AccStateNonexist => 0b11,
        },
    );

    if mode.is_q_server() {
        let name = format!("{}_name", name);
        serialize_field(
            map,
            &name,
            match status {
                AccountStatus::AccStateUninit => "Uninit",
                AccountStatus::AccStateFrozen => "Frozen",
                AccountStatus::AccStateActive => "Active",
                AccountStatus::AccStateNonexist => "NonExist",
            },
        );
    }
}

#[derive(Default)]
pub struct AccountSerializationSet {
    pub account: Account,
    pub prev_code_hash: Option<UInt256>,
    pub boc: Vec<u8>,
    pub proof: Option<Vec<u8>>,
}

pub fn debug_account(account: Account) -> Result<String> {
    let set =
        AccountSerializationSet { account, prev_code_hash: None, boc: Vec::new(), proof: None };
    let map = db_serialize_account_ex("id", &set, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn db_serialize_account(
    id_str: &'static str,
    set: &AccountSerializationSet,
) -> Result<Map<String, Value>> {
    db_serialize_account_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_account_ex(
    id_str: &'static str,
    set: &AccountSerializationSet,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    if let Some(addr) = set.account.get_addr() {
        serialize_field(&mut map, id_str, addr.to_string());
        serialize_field(&mut map, "workchain_id", addr.workchain_id());
    }
    serialize_field(&mut map, "boc", base64_encode(&set.boc));
    if let Some(storage_stat) = set.account.storage_info() {
        serialize_field(&mut map, "last_paid", storage_stat.last_paid());
        serialize_u64(&mut map, "bits", &storage_stat.used().bits(), mode);
        serialize_u64(&mut map, "cells", &storage_stat.used().cells(), mode);
        serialize_id(&mut map, "dict_hash", storage_stat.dict_hash());
        if let Some(grams) = storage_stat.due_payment() {
            serialize_grams(&mut map, "due_payment", grams, mode);
        }
    }
    serialize_lt(&mut map, "last_trans_lt", set.account.last_tr_time().unwrap_or_default(), mode);
    set.account.balance().map(|cc| serialize_cc(&mut map, "balance", cc, mode)).transpose()?;
    match set.account.status() {
        AccountStatus::AccStateActive => {
            if let Some(state) = set.account.state_init() {
                if let Some(fixed_prefix_length) = state.fixed_prefix_length() {
                    serialize_field(&mut map, "fixed_prefix_length", fixed_prefix_length.as_u32());
                }
                if let Some(special) = state.special() {
                    serialize_field(&mut map, "tick", special.tick);
                    serialize_field(&mut map, "tock", special.tock);
                };
                serialize_cell(&mut map, "code", state.code(), true)?;
                serialize_cell(&mut map, "data", state.data(), true)?;
                serialize_cell(&mut map, "library", state.libraries().root(), true)?;
            }
        }
        AccountStatus::AccStateFrozen => {
            serialize_id(&mut map, "state_hash", set.account.frozen_hash())
        }
        AccountStatus::AccStateUninit => {}
        AccountStatus::AccStateNonexist => {
            fail!("Attempt to call serde::Serialize::serialize for AccountNone")
        }
    };
    if let Some(proof) = &set.proof {
        serialize_field(&mut map, "proof", base64_encode(proof));
    }
    serialize_account_status(&mut map, "acc_type", &set.account.status(), mode);
    serialize_id(&mut map, "prev_code_hash", set.prev_code_hash.as_ref());
    Ok(map)
}

#[derive(Default)]
pub struct DeletedAccountSerializationSet {
    pub account_id: AccountId,
    pub prev_code_hash: Option<UInt256>,
    pub workchain_id: i32,
}

pub fn db_serialize_deleted_account(
    id_str: &'static str,
    set: &DeletedAccountSerializationSet,
) -> Result<Map<String, Value>> {
    db_serialize_deleted_account_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_deleted_account_ex(
    id_str: &'static str,
    set: &DeletedAccountSerializationSet,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    let address = construct_address(set.workchain_id, set.account_id.clone())?;
    serialize_field(&mut map, id_str, address.to_string());
    serialize_field(&mut map, "workchain_id", set.workchain_id);
    serialize_account_status(&mut map, "acc_type", &AccountStatus::AccStateNonexist, mode);
    serialize_id(&mut map, "prev_code_hash", set.prev_code_hash.as_ref());

    Ok(map)
}

#[derive(Default)]
pub struct MessageSerializationSet {
    pub message: Message,
    pub id: MessageId,
    pub block_id: Option<UInt256>,
    pub transaction_id: Option<UInt256>,
    pub transaction_now: Option<u32>,
    pub status: MessageProcessingStatus,
    pub boc: Vec<u8>,
    pub proof: Option<Vec<u8>>,
}

pub fn debug_message(message: Message) -> Result<String> {
    let root_cell = message.serialize()?;
    let set = MessageSerializationSet {
        message,
        id: root_cell.repr_hash(),
        block_id: None,
        transaction_id: None,
        transaction_now: None,
        status: MessageProcessingStatus::Finalized,
        boc: Vec::new(),
        proof: None,
    };
    let map = db_serialize_message_ex("id", &set, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn db_serialize_message(
    id_str: &'static str,
    set: &MessageSerializationSet,
) -> Result<Map<String, Value>> {
    db_serialize_message_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_message_ex(
    id_str: &'static str,
    set: &MessageSerializationSet,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    serialize_id(&mut map, id_str, Some(&set.id));
    // isn't needed there - because message should be fully immutable from source block to destination one
    //serialize_id(&mut map, "block_id", set.block_id.as_ref());
    serialize_id(&mut map, "transaction_id", set.transaction_id.as_ref());
    if let Some(proof) = &set.proof {
        serialize_field(&mut map, "proof", base64_encode(proof));
    }
    serialize_field(&mut map, "boc", base64_encode(&set.boc));
    serialize_field(&mut map, "status", set.status as u8);
    if mode.is_q_server() {
        serialize_field(
            &mut map,
            "status_name",
            match set.status {
                MessageProcessingStatus::Unknown => "unknown",
                MessageProcessingStatus::Queued => "queued",
                MessageProcessingStatus::Processing => "processing",
                MessageProcessingStatus::Preliminary => "preliminary",
                MessageProcessingStatus::Proposed => "proposed",
                MessageProcessingStatus::Finalized => "finalized",
                MessageProcessingStatus::Refused => "refused",
                MessageProcessingStatus::Transiting => "transiting",
            },
        );
    }
    if let Some(state) = &set.message.state_init() {
        if let Some(fixed_prefix_length) = state.fixed_prefix_length() {
            serialize_field(&mut map, "fixed_prefix_length", fixed_prefix_length.as_u32());
        }
        if let Some(special) = state.special() {
            serialize_field(&mut map, "tick", special.tick);
            serialize_field(&mut map, "tock", special.tock);
        };
        serialize_cell(&mut map, "code", state.code(), true)?;
        serialize_cell(&mut map, "data", state.data(), true)?;
        serialize_cell(&mut map, "library", state.libraries().root(), true)?;
    }

    serialize_slice(&mut map, "body", set.message.body(), true)?;
    match set.message.header() {
        CommonMsgInfo::IntMsgInfo(ref header) => {
            serialize_field(&mut map, "msg_type", 0);
            if mode.is_q_server() {
                serialize_field(&mut map, "msg_type_name", "internal");
            }
            serialize_field(&mut map, "src", header.src.to_string());
            if let Some(src_addr) = header.src_ref() {
                serialize_field(&mut map, "src_workchain_id", src_addr.workchain_id());
            }
            serialize_field(&mut map, "dst", header.dst.to_string());
            serialize_field(&mut map, "dst_workchain_id", header.dst.workchain_id());
            serialize_field(&mut map, "ihr_disabled", header.ihr_disabled);
            serialize_grams(&mut map, "extra_flags", &header.extra_flags, mode);
            serialize_grams(&mut map, "fwd_fee", &header.fwd_fee, mode);
            serialize_field(&mut map, "bounce", header.bounce);
            serialize_field(&mut map, "bounced", header.bounced);
            serialize_cc(&mut map, "value", &header.value, mode)?;
            serialize_lt(&mut map, "created_lt", header.created_lt, mode);
            serialize_field(&mut map, "created_at", header.created_at);
        }
        CommonMsgInfo::ExtInMsgInfo(ref header) => {
            serialize_field(&mut map, "msg_type", 1);
            if mode.is_q_server() {
                serialize_field(&mut map, "msg_type_name", "extIn");
            }
            serialize_field(&mut map, "src", header.src.to_string());
            serialize_field(&mut map, "dst", header.dst.to_string());
            serialize_field(&mut map, "dst_workchain_id", header.dst.workchain_id());
            serialize_grams(&mut map, "import_fee", &header.import_fee, mode);
            if let Some(now) = set.transaction_now {
                serialize_field(&mut map, "created_at", now);
            }
        }
        CommonMsgInfo::ExtOutMsgInfo(ref header) => {
            serialize_field(&mut map, "msg_type", 2);
            if mode.is_q_server() {
                serialize_field(&mut map, "msg_type_name", "extOut");
            }
            serialize_field(&mut map, "src", header.src.to_string());
            if let Some(src_addr) = header.src() {
                serialize_field(&mut map, "src_workchain_id", src_addr.workchain_id());
            }
            serialize_field(&mut map, "dst", header.dst.to_string());
            serialize_lt(&mut map, "created_lt", header.created_lt, mode);
            serialize_field(&mut map, "created_at", header.created_at);
        }
    }
    Ok(map)
}

pub fn db_serialize_block_signatures(
    id_str: &'static str,
    block_id: &UInt256,
    signatures_set: &[CryptoSignaturePair],
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    let mut signs = Vec::new();
    serialize_field(&mut map, "json_version", VERSION);
    serialize_uint256(&mut map, id_str, block_id);
    for s in signatures_set.iter() {
        signs.push(serialize_crypto_signature(s)?);
    }
    serialize_field(&mut map, "signatures", signs);
    Ok(map)
}

pub fn db_serialize_block_proof(
    id_str: &'static str,
    proof: &BlockProof,
) -> Result<Map<String, Value>> {
    db_serialize_block_proof_ex(id_str, proof, SerializationMode::Standart)
}

pub fn db_serialize_block_proof_ex(
    id_str: &'static str,
    proof: &BlockProof,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();

    serialize_field(&mut map, "json_version", VERSION);
    serialize_uint256(&mut map, id_str, &proof.proof_for.root_hash);

    let merkle_proof = MerkleProof::construct_from_cell(proof.root.clone())?;
    let block_virt_root = merkle_proof.proof.virtualize(1);
    let virt_block = Block::construct_from_cell(block_virt_root)?;
    let block_info = virt_block.read_info()?;

    map.insert("gen_utime".to_string(), block_info.gen_utime().into());
    map.insert("seq_no".to_string(), block_info.seq_no().into());
    map.insert("workchain_id".to_string(), block_info.shard().workchain_id().into());
    map.insert("shard".to_string(), block_info.shard().shard_prefix_as_str_with_tag().into());
    serialize_cell(&mut map, "proof", Some(&proof.root), false)?;

    if let Some(signatures) = proof.signatures.as_ref() {
        // Serialize common fields
        map.insert(
            "validator_list_hash_short".to_string(),
            signatures.validator_info().validator_list_hash_short.into(),
        );
        map.insert("catchain_seqno".to_string(), signatures.validator_info().catchain_seqno.into());
        serialize_u64(&mut map, "sig_weight", &signatures.pure_signatures().weight(), mode);

        let mut signs = Vec::new();
        signatures.pure_signatures().signatures().iterate_slices(
            |_key, mut value| -> Result<bool> {
                signs.push(serialize_crypto_signature(&CryptoSignaturePair::construct_from(
                    &mut value,
                )?)?);
                Ok(true)
            },
        )?;
        serialize_field(&mut map, "signatures", signs);

        // Serialize variant-specific fields
        match signatures {
            BlockSignaturesVariant::Ordinary(_) => {
                serialize_field(&mut map, "signature_type", "ordinary");
            }
            BlockSignaturesVariant::Simplex(simplex) => {
                serialize_field(&mut map, "signature_type", "simplex");
                serialize_uint256(&mut map, "session_id", &simplex.session_id);
                serialize_field(&mut map, "slot", simplex.slot);
                serialize_cell(&mut map, "candidate_data", Some(&simplex.candidate_data), true)?;
            }
        }
    }
    Ok(map)
}

#[derive(Default)]
pub struct ShardStateSerializationSet {
    pub state: ShardStateUnsplit,
    pub block_id: Option<UInt256>,
    pub workchain_id: i32,
    pub id: String,
    pub boc: Vec<u8>,
}

pub fn db_serialize_shard_state(
    id_str: &'static str,
    set: &ShardStateSerializationSet,
) -> Result<Map<String, Value>> {
    db_serialize_shard_state_ex(id_str, set, SerializationMode::Standart)
}

pub fn db_serialize_shard_state_ex(
    id_str: &'static str,
    set: &ShardStateSerializationSet,
    mode: SerializationMode,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    serialize_field(&mut map, "json_version", VERSION);
    serialize_field(&mut map, id_str, set.id.as_str());
    let cell = read_single_root_boc(set.boc.as_slice())?;
    serialize_id(&mut map, "root_hash", Some(&cell.repr_hash()));
    serialize_file_hash(&mut map, None, &set.boc);
    serialize_id(&mut map, "block_id", set.block_id.as_ref());
    serialize_field(&mut map, "workchain_id", set.workchain_id);
    serialize_field(&mut map, "boc", base64_encode(&set.boc));
    serialize_field(&mut map, "global_id", set.state.global_id());
    serialize_field(&mut map, "shard", set.state.shard().shard_prefix_as_str_with_tag());
    serialize_field(&mut map, "seq_no", set.state.seq_no());
    serialize_field(&mut map, "vert_seq_no", set.state.vert_seq_no());
    serialize_field(&mut map, "gen_utime", set.state.gen_time());
    serialize_lt(&mut map, "gen_lt", set.state.gen_lt(), mode);
    serialize_field(&mut map, "min_ref_mc_seqno", set.state.min_ref_mc_seqno());
    serialize_field(&mut map, "before_split", set.state.before_split());
    serialize_u64(&mut map, "overload_history", &set.state.overload_history(), mode);
    serialize_u64(&mut map, "underload_history", &set.state.underload_history(), mode);
    serialize_cc(&mut map, "total_balance", set.state.total_balance(), mode)?;
    serialize_cc(&mut map, "total_validator_fees", set.state.total_validator_fees(), mode)?;
    if let Some(block_info) = set.state.master_ref() {
        map.insert("master_ref".to_string(), serialize_block_ref(&block_info.master, None, mode));
    }
    if let Some(master) = set.state.read_custom()? {
        serialize_mc_state_extra(&mut map, "master", &master, mode)?;
    }
    serialize_shard_accounts(&mut map, "accounts", &set.state.read_accounts()?, mode)?;
    serialize_libraries(&mut map, "libraries", set.state.libraries())?;
    serialize_out_msg_queue_info(
        &mut map,
        "out_msg_queue_info",
        &set.state.read_out_msg_queue_info()?,
        mode,
    )?;
    Ok(map)
}

pub fn debug_state(mut state: ShardStateUnsplit) -> Result<String> {
    state.write_accounts(&Default::default())?;
    let set = ShardStateSerializationSet {
        block_id: None,
        workchain_id: state.shard().workchain_id(),
        id: state.shard().to_string(),
        boc: state.write_to_bytes()?,
        state,
    };
    let map = db_serialize_shard_state_ex("id", &set, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn debug_state_full(state: ShardStateUnsplit) -> Result<String> {
    let set = ShardStateSerializationSet {
        block_id: None,
        workchain_id: state.shard().workchain_id(),
        id: state.shard().to_string(),
        boc: state.write_to_bytes()?,
        state,
    };
    let map = db_serialize_shard_state_ex("id", &set, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

pub fn debug_config(config: &ConfigParams) -> Result<String> {
    let mut map = Map::new();
    serialize_config(&mut map, config, SerializationMode::Debug)?;
    Ok(format!("{:#}", serde_json::json!(map)))
}

#[cfg(test)]
#[path = "tests/test_serialize.rs"]
mod tests;
