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
    executor::{gas::gas_state::Gas, Engine},
    stack::{integer::IntegerData, savelist::SaveList, Stack, StackItem},
};
use std::{borrow::Cow, mem};
use ton_api::{
    ton::tvm::{
        stackentry::{
            StackEntryCell, StackEntryList, StackEntryNumber, StackEntrySlice, StackEntryTuple,
        },
        StackEntry,
    },
    IntoBoxed,
};
use ton_block::{
    error, fail, read_single_root_boc, write_boc, Account, Cell, ConfigParams, CurrencyCollection,
    Deserializable, ExtBlkRef, HashmapAugType, KeyExtBlkRef, Message, OldMcBlocksInfo, Result,
    Serializable, Sha256, ShardStateUnsplit, SliceData, UInt256, UnixTime,
};

/*
The smart-contract information
structure SmartContractInfo, passed in the first reference of the cell contained
in control register c5, is serialized as follows:

smc_info#076ef1ea actions:uint16 msgs_sent:uint16
unixtime:uint32 block_lt:uint64 trans_lt:uint64
rand_seed:uint256 balance_remaining:CurrencyCollection
myself:MsgAddress = SmartContractInfo;
*/

#[derive(Clone, Debug)]
pub enum PrevBlocksInfo {
    Tuple(StackItem),
    Raw(KeyExtBlkRef, OldMcBlocksInfo),
}

impl Default for PrevBlocksInfo {
    fn default() -> Self {
        PrevBlocksInfo::Tuple(StackItem::None)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SmartContractInfo {
    pub actions: u16,
    pub msgs_sent: u16,
    pub unix_time: u32,
    pub block_lt: u64,
    pub trans_lt: u64,
    pub rand_seed: IntegerData,
    pub balance: CurrencyCollection,
    pub myself: SliceData,
    pub config_params: ConfigParams,
    pub mycode: Cell,
    pub in_msg: Option<Message>,
    pub incoming_value: CurrencyCollection, // remaining value
    pub storage_fees_collected: u128,
    pub prev_blocks_info: PrevBlocksInfo,
    pub due_payment: u128,
    pub precompiled_gas_usage: u64,
}

pub struct SmcMethodResult {
    pub exit_code: i32,
    pub gas_used: i64,
    pub stack: Vec<StackItem>,
    pub smc_info: SmartContractInfo,
}

impl SmcMethodResult {
    pub fn into_run_result(self) -> Result<ton_api::ton::smc::runresult::RunResult> {
        Ok(ton_api::ton::smc::runresult::RunResult {
            gas_used: self.gas_used,
            stack: convert_stack(&self.stack)?,
            exit_code: self.exit_code,
        })
    }
}

impl SmartContractInfo {
    pub fn with_params(
        account: Option<&Account>,
        message_root: Option<Cell>,
        mc_state_root: Option<Cell>, // state root could be virtualized
    ) -> Result<Self> {
        let mut smci = Self { unix_time: UnixTime::now() as u32, ..Default::default() };
        if let Some(mc_state_root) = mc_state_root {
            let mc_state = ShardStateUnsplit::construct_from_cell(mc_state_root)?;
            let extra =
                mc_state.read_custom()?.ok_or_else(|| error!("No custom data in mc_state"))?;
            smci.prev_blocks_info = PrevBlocksInfo::Raw(
                KeyExtBlkRef {
                    key: extra.after_key_block,
                    blk_ref: ExtBlkRef {
                        end_lt: mc_state.gen_lt(),
                        seq_no: mc_state.seq_no(),
                        root_hash: UInt256::default(),
                        file_hash: UInt256::default(),
                    },
                },
                extra.prev_blocks,
            );
            smci.block_lt = mc_state.gen_lt() + 1;
            smci.unix_time = mc_state.gen_time() + 1;
            smci.config_params = extra.config;
        }
        if let Some(account) = account {
            if let Some(addr) = account.get_addr() {
                smci.myself = addr.write_to_bitstring()?;
            }
            if let Some(balance) = account.balance() {
                smci.balance = balance.clone();
            }
            smci.mycode = account.get_code().unwrap_or_default();
            smci.due_payment = account.due_payment().map_or(0, |g| g.as_u128());
        }
        if let Some(message_root) = message_root {
            let message = Message::construct_from_cell(message_root)?;
            if let Some(incoming_value) = message.value() {
                smci.incoming_value = incoming_value.clone();
            }
            smci.in_msg = Some(message);
        }
        Ok(smci)
    }

    pub fn unix_time(&self) -> u32 {
        self.unix_time
    }

    pub fn set_mycode(&mut self, code: Cell) {
        self.mycode = code;
    }

    pub fn set_precompiled_gas_usage(&mut self, precompiled_gas_usage: u64) {
        self.precompiled_gas_usage = precompiled_gas_usage;
    }

    // The rand_seed field here is initialized deterministically starting from the
    // rand_seed of the block, and the account address.
    pub fn calc_rand_seed(&mut self, rand_seed_block: UInt256, account_address_anycast: &[u8]) {
        // combine all parameters to vec and calculate hash of them
        self.rand_seed = if !rand_seed_block.is_zero() {
            let mut hasher = Sha256::new();
            hasher.update(&rand_seed_block);
            hasher.update(account_address_anycast);

            let sha256 = hasher.finalize();
            IntegerData::from_unsigned_bytes_be(sha256)
        } else {
            // if the user forgot to set the rand_seed_block value, then this 0 will be clearly visible on tests
            log::warn!(target: "tvm", "Not set rand_seed_block");
            IntegerData::zero()
        };
    }

    fn cc2tuple(value: &CurrencyCollection) -> StackItem {
        StackItem::tuple(vec![
            StackItem::int(value.coins.as_u128()),
            StackItem::dict(value.other.root()),
        ])
    }

    fn id2tuple(id: &ExtBlkRef) -> StackItem {
        StackItem::tuple(vec![
            StackItem::int(-1),
            StackItem::int(0x8000_0000_0000_0000u64),
            StackItem::int(id.seq_no),
            StackItem::integer(IntegerData::from_unsigned_bytes_be(id.root_hash.as_slice())),
            StackItem::integer(IntegerData::from_unsigned_bytes_be(id.file_hash.as_slice())),
        ])
    }

    fn get_block_id(&self, mc_seqno: u32) -> Result<Option<Cow<'_, ExtBlkRef>>> {
        let PrevBlocksInfo::Raw(last_mc_block_id, old_mc_blocks_info) = &self.prev_blocks_info
        else {
            return Ok(None);
        };
        if mc_seqno == last_mc_block_id.blk_ref.seq_no {
            Ok(Some(Cow::Borrowed(&last_mc_block_id.blk_ref)))
        } else if let Some(id) = old_mc_blocks_info.get(&mc_seqno)? {
            Ok(Some(Cow::Owned(id.blk_ref)))
        } else {
            Ok(None)
        }
    }

    // [ wc:Integer shard:Integer seqno:Integer root_hash:Integer file_hash:Integer] = BlockId;
    // [ last_mc_blocks:[BlockId...]
    //   prev_key_block:BlockId
    //   last_mc_blocks_100[BlockId...] ] : PrevBlocksInfo
    fn get_prev_blocks_info(&self) -> StackItem {
        let (last_mc_block_id, old_mc_blocks_info) = match &self.prev_blocks_info {
            PrevBlocksInfo::Tuple(tuple) => return tuple.clone(),
            PrevBlocksInfo::Raw(last_mc_block_id, old_mc_blocks_info) => {
                (last_mc_block_id, old_mc_blocks_info)
            }
        };
        if let Err(err) = self.get_block_id(0) {
            log::warn!(target: "tvm", "cannot read zero state id {err}")
        }
        let last_mc_seqno = last_mc_block_id.blk_ref.seq_no;

        // last 16 mc blocks
        let mut seqno = last_mc_seqno;
        let mut vec = vec![Self::id2tuple(&last_mc_block_id.blk_ref)];
        loop {
            if seqno == 0 || vec.len() >= 16 {
                break;
            }
            seqno -= 1;
            let Ok(Some(id)) = self.get_block_id(seqno) else {
                break;
            };
            vec.push(Self::id2tuple(&id));
        }
        let mut tuple = vec![StackItem::tuple(vec)];

        // last key block
        if last_mc_block_id.key {
            tuple.push(Self::id2tuple(&last_mc_block_id.blk_ref));
        } else if let Ok(Some(id)) = old_mc_blocks_info.get_prev_key_block(last_mc_seqno) {
            tuple.push(Self::id2tuple(&id));
        } else {
            tuple.push(StackItem::None);
        }

        // last 16 100th mc blocks
        let mut vec = Vec::new();
        let mut seqno = last_mc_seqno / 100 * 100;
        loop {
            if let Ok(Some(id)) = self.get_block_id(seqno) {
                vec.push(Self::id2tuple(&id));
            } else {
                break;
            }
            if seqno < 100 || vec.len() >= 16 {
                break;
            }
            seqno -= 100;
        }
        tuple.push(StackItem::tuple(vec));
        StackItem::tuple(tuple)
    }

    fn config_param_slice(&self, index: u32) -> StackItem {
        self.config_params.config_cell_slice(index).map_or(StackItem::None, StackItem::Slice)
    }

    fn get_unpacked_config_tuple(&self) -> StackItem {
        let mut storage_price = StackItem::None;
        if let Ok(storage_prices) = self.config_params.storage_prices() {
            let _ = storage_prices.map.iterate_slices_with_keys(|mut key, slice| {
                let utime_since = key.get_next_u32()?;
                if self.unix_time >= utime_since {
                    storage_price = StackItem::slice(slice);
                    return Ok(false);
                }
                Ok(true)
            });
        };
        let mut tuple = vec![storage_price];
        tuple.push(self.config_param_slice(19)); // global_id
        tuple.push(self.config_param_slice(20)); // config_mc_gas_prices
        tuple.push(self.config_param_slice(21)); // config_gas_prices
        tuple.push(self.config_param_slice(24)); // config_mc_fwd_prices
        tuple.push(self.config_param_slice(25)); // config_fwd_prices
        tuple.push(self.config_param_slice(43)); // size_limits_config
        StackItem::tuple(tuple)
    }
    fn get_message_info(&self) -> StackItem {
        let mut tuple = vec![];
        if let Some(msg) = self.in_msg.as_ref() {
            if let Some(int) = msg.int_header() {
                tuple.push(StackItem::boolean(int.bounce));
                tuple.push(StackItem::boolean(int.bounced));
                tuple.push(StackItem::slice(int.src.write_to_bitstring().unwrap_or_default()));
                tuple.push(StackItem::int(int.fwd_fee().as_u128()));
                tuple.push(StackItem::int(int.created_lt));
                tuple.push(StackItem::int(int.created_at));
                tuple.push(StackItem::int(int.value.coins.as_u128()));
            } else if let Some(ext) = msg.ext_in_header() {
                tuple.push(StackItem::boolean(false));
                tuple.push(StackItem::boolean(false));
                tuple.push(StackItem::slice(ext.src.write_to_bitstring().unwrap_or_default()));
                tuple.push(StackItem::int(ext.import_fee.as_u128()));
                tuple.push(StackItem::int(0));
                tuple.push(StackItem::int(0));
                tuple.push(StackItem::int(0));
            }
            tuple.push(StackItem::int(self.incoming_value.coins.as_u128()));
            tuple.push(StackItem::dict(self.incoming_value.other.root()));
            if let Some(init) = msg.state_init() {
                tuple.push(StackItem::cell(init.serialize().unwrap_or_default()));
            } else {
                tuple.push(StackItem::default());
            }
        } else {
            tuple.push(StackItem::boolean(false)); // bounce
            tuple.push(StackItem::boolean(false)); // bounced
            let addr_none =
                ton_block::MsgAddressExt::AddrNone.write_to_bitstring().unwrap_or_default();
            tuple.push(StackItem::Slice(addr_none)); // src
            tuple.push(StackItem::int(0)); // import fee
            tuple.push(StackItem::int(0)); // created lt
            tuple.push(StackItem::int(0)); // created at
            tuple.push(StackItem::int(0)); // original value
            tuple.push(StackItem::int(0)); // value
            tuple.push(StackItem::default()); // extra
            tuple.push(StackItem::default()); // state init
        }
        StackItem::tuple(tuple)
    }

    pub fn as_temp_data_item(&self) -> StackItem {
        // let config = ton_block_json::debug_config(&self.config_params).unwrap();
        // std::fs::write("d:\\config.json", config)?;
        // extra read some config params for usage tree
        {
            let _ = self.config_params.config(5);
            let _ = self.config_params.config(9);
            self.config_params
                .suspended_address_list()
                .unwrap_or_default()
                .unwrap_or_default()
                .is_empty();
        }
        let version = self.config_params.global_version();
        let prev_blocks_info = self.get_prev_blocks_info();
        let config_info = self.get_unpacked_config_tuple();
        let msg_info = self.get_message_info();
        let precompiled_gas_usage = if self.precompiled_gas_usage == 0 {
            StackItem::None
        } else {
            StackItem::int(self.precompiled_gas_usage)
        };
        let mut params = vec![
            StackItem::int(0x076ef1ea), // magic - should be changed because of structure change
            StackItem::int(self.actions), // actions
            StackItem::int(self.msgs_sent), // msgs
            StackItem::int(self.unix_time), // unix time
            StackItem::int(self.block_lt), // logical time
            StackItem::int(self.trans_lt), // transaction time
            StackItem::int(self.rand_seed.clone()),
            Self::cc2tuple(&self.balance),
            StackItem::Slice(self.myself.clone()),
            StackItem::dict(self.config_params.root()),
            StackItem::cell(self.mycode.clone()),
            Self::cc2tuple(&self.incoming_value),
            StackItem::int(self.storage_fees_collected),
            prev_blocks_info,
            config_info,
            StackItem::int(self.due_payment),
            precompiled_gas_usage,
        ];
        if version >= 11 {
            params.push(msg_info);
        }
        StackItem::tuple(vec![StackItem::tuple(params)])
    }
}

fn render_stack(items: &[StackItem]) -> String {
    items.iter().map(|item| item.dump_as_fift()).collect::<Vec<_>>().join(" ")
}

fn parse_stack_number(number: &str) -> Result<IntegerData> {
    let (negative, value) =
        if let Some(value) = number.strip_prefix('-') { (true, value) } else { (false, number) };

    let (radix, digits) = if let Some(hex) = value.strip_prefix("0x") {
        (16, hex)
    } else if let Some(hex) = value.strip_prefix("0X") {
        (16, hex)
    } else {
        (10, value)
    };

    // Validate hexadecimal digits explicitly to provide a clearer error on malformed input.
    if radix == 16 && !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        fail!("invalid hexadecimal literal: {number}");
    }
    let literal = if negative { Cow::Owned(format!("-{digits}")) } else { Cow::Borrowed(digits) };
    IntegerData::from_str_radix(&literal, radix)
}

pub fn convert_stack(items: &[StackItem]) -> Result<Vec<StackEntry>> {
    fn convert_item(item: &StackItem) -> Result<StackEntry> {
        let stack_entry = match item {
            StackItem::Integer(value) => {
                let number = value.to_str_hex();
                let number =
                    ton_api::ton::tvm::numberdecimal::NumberDecimal { number }.into_boxed();
                StackEntryNumber { number }.into_boxed()
            }
            StackItem::Slice(slice) => {
                let cell = slice.clone().into_cell()?;
                let bytes = write_boc(&cell)?;
                let slice = ton_api::ton::tvm::slice::Slice { bytes };
                StackEntrySlice { slice }.into_boxed()
            }
            StackItem::Cell(cell) => {
                let bytes = write_boc(cell)?;
                let cell = ton_api::ton::tvm::cell::Cell { bytes };
                StackEntryCell { cell }.into_boxed()
            }
            StackItem::None => {
                let list = ton_api::ton::tvm::list::List { elements: Vec::new() }.into_boxed();
                StackEntryList { list }.into_boxed()
            }
            StackItem::Tuple(elements) => {
                let mut probe = elements;
                let list = loop {
                    if probe.len() == 2 {
                        match probe.last() {
                            Some(StackItem::Tuple(next_tuple)) => probe = next_tuple,
                            Some(StackItem::None) => break true,
                            _ => break false,
                        }
                    } else {
                        break false;
                    }
                };
                if list {
                    let mut tuple = elements;
                    let mut elements = Vec::new();
                    while let Some(item) = tuple.first() {
                        elements.push(convert_item(item)?);
                        let Some(StackItem::Tuple(next_tuple)) = tuple.last() else {
                            break;
                        };
                        tuple = next_tuple;
                    }
                    let list = ton_api::ton::tvm::list::List { elements }.into_boxed();
                    StackEntryList { list }.into_boxed()
                } else {
                    let elements = convert_stack(elements)?;
                    let tuple = ton_api::ton::tvm::tuple::Tuple { elements }.into_boxed();
                    StackEntryTuple { tuple }.into_boxed()
                }
            }
            _ => StackEntry::Tvm_StackEntryUnsupported,
        };
        Ok(stack_entry)
    }

    let mut result = Vec::with_capacity(items.len());
    for item in items {
        result.push(convert_item(item)?)
    }
    Ok(result)
}

pub fn convert_ton_stack(items: &[StackEntry]) -> Result<Vec<StackItem>> {
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let stack_item = match item {
            StackEntry::Tvm_StackEntryCell(cell) => {
                let cell = &cell.cell;
                let cell = read_single_root_boc(&cell.bytes)?;
                StackItem::cell(cell)
            }
            StackEntry::Tvm_StackEntryList(list) => {
                let elements = convert_ton_stack(list.list.elements())?;
                let mut tuple = StackItem::None;
                for elem in elements.into_iter().rev() {
                    tuple = StackItem::tuple(vec![elem, tuple]);
                }
                tuple
            }
            StackEntry::Tvm_StackEntryNumber(number) => {
                let number = &number.number;
                let value = parse_stack_number(number.number())?;
                StackItem::int(value)
            }
            StackEntry::Tvm_StackEntrySlice(slice) => {
                let cell = read_single_root_boc(&slice.slice.bytes)?;
                let slice = SliceData::load_cell(cell)?;
                StackItem::slice(slice)
            }
            StackEntry::Tvm_StackEntryTuple(tuple) => {
                let elements = convert_ton_stack(tuple.tuple.elements())?;
                StackItem::tuple(elements)
            }
            StackEntry::Tvm_StackEntryUnsupported => {
                fail!("Unsupported stack entry encountered")
            }
        };
        result.push(stack_item);
    }
    Ok(result)
}

pub fn run_smc_method(
    account: &Account,
    mc_state_cell: Cell,
    method_id: u32,
    stack: Vec<StackEntry>,
    gen_utime: u32,
    gen_lt: u64,
) -> Result<SmcMethodResult> {
    let code = account.get_code().ok_or_else(|| error!("Account has no code"))?;
    let data = account.get_data().unwrap_or_default();
    let mut smc_info =
        SmartContractInfo::with_params(Some(account), None, Some(mc_state_cell.clone()))?;
    smc_info.unix_time = gen_utime;
    smc_info.block_lt = gen_lt;
    smc_info.trans_lt = gen_lt;

    let mut storage = convert_ton_stack(&stack)?;
    storage.push(StackItem::int(method_id));
    let stack = Stack::with_storage(storage);

    let mut ctrls = SaveList::new();
    ctrls.put(7, smc_info.as_temp_data_item())?;
    ctrls.put(4, StackItem::Cell(data))?;

    let gas = Gas::new(1000000, 0, 1000000, 1000000);

    let mc_state = ShardStateUnsplit::construct_from_cell(mc_state_cell)?;
    let libraries = vec![account.libraries().inner(), mc_state.libraries().clone().inner()];

    let caps = smc_info.config_params.capabilities();
    let mut vm =
        Engine::with_capabilities(caps).setup_checked(code, ctrls, stack, gas, libraries)?;

    let block_version = smc_info.config_params.get_global_version()?.version;
    vm.set_block_version(block_version);

    let result = vm.execute();
    let mut stack = mem::take(&mut vm.withdraw_stack().storage);
    let exit_code = match result {
        Ok(exit_code) => exit_code,
        Err(err) => {
            stack.pop();
            log::debug!(target: "executor", "VM terminated with full exception: {}", err);
            crate::error::tvm_exception_or_custom_code(&err)
        }
    };
    log::debug!("run_smc_method: exit_code={}\n", exit_code);
    log::debug!("run_smc_method: gas used={}\n", vm.gas_used());
    log::debug!("run_smc_method: result_stack_depth={}\n", stack.len());
    log::debug!("run_smc_method: result_stack dump: [ {} ]\n", render_stack(&stack));

    Ok(SmcMethodResult { exit_code, gas_used: vm.gas_used(), stack, smc_info })
}

#[cfg(test)]
#[path = "tests/test_smart_contract_info.rs"]
mod tests;
