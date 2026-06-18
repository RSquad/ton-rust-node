/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::rpc_server::{serializers::write_boc, JsonResult};
use ton_api::ton::tvm::{stackentry, StackEntry};
use ton_block::{
    base64_encode, error, fail, read_single_root_boc, Cell, Deserializable, HashmapE, MsgAddress,
    MsgAddressInt, Result, Sha256, SliceData, ADDR_FORMAT_BOUNCE, ADDR_FORMAT_TESTNET,
    ADDR_FORMAT_URL_SAFE,
};

const DEFAULT_JETTON_KEYS: [&str; 9] = [
    "uri",
    "name",
    "description",
    "image",
    "image_data",
    "symbol",
    "decimals",
    "amount_style",
    "render_type",
];

fn token_attr_key(name: &str) -> SliceData {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let hash: [u8; 32] = hasher.finalize().into();
    SliceData::from_raw(hash.to_vec(), 256)
}

fn ensure_stack_len(stack: &[StackEntry], expected: usize, ctx: &str) -> Result<()> {
    if stack.len() < expected {
        fail!("{ctx}: expected at least {expected} stack entries, got {}", stack.len());
    }
    Ok(())
}

fn parse_token_attr_value_cell(cell: &Cell) -> Result<String> {
    let actual_cell = if cell.bit_length() > 0 {
        cell.clone()
    } else {
        cell.reference(0)
            .map_err(|e| error!("TokenData value cell has no data and no refs: {e}"))?
    };
    let mut slice = SliceData::load_cell(actual_cell)
        .map_err(|e| error!("failed to load slice from TokenData value cell: {e}"))?;
    let prefix =
        slice.get_next_int(8).map_err(|e| error!("failed to read TokenData value prefix: {e}"))?;
    match prefix {
        0 => {
            let bytes = read_snake_bytes(&mut slice)
                .map_err(|e| error!("failed to read snake string from TokenData: {e}"))?;
            String::from_utf8(bytes)
                .map_err(|e| error!("failed to decode TokenData string as UTF-8: {e}"))
        }
        1 => {
            fail!("TokenData 'chunks' content is not supported yet");
        }
        other => fail!("unexpected TokenData value prefix: {other}"),
    }
}

fn read_snake_bytes(slice: &mut SliceData) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        while slice.remaining_bits() >= 8 {
            let b =
                slice.get_next_int(8).map_err(|e| error!("failed to read TokenData byte: {e}"))?;
            out.push(b as u8);
        }
        if slice.remaining_references() == 0 {
            break;
        }
        let next_cell = slice
            .checked_drain_reference()
            .map_err(|e| error!("failed to read TokenData snake ref: {e}"))?;
        *slice = SliceData::load_cell(next_cell)
            .map_err(|e| error!("failed to load TokenData continuation cell: {e}"))?;
    }
    Ok(out)
}

fn stack_entry_boc(entry: &StackEntry) -> Result<Vec<u8>> {
    match entry {
        StackEntry::Tvm_StackEntryCell(stackentry::StackEntryCell { cell }) => {
            Ok(cell.bytes.clone())
        }
        StackEntry::Tvm_StackEntrySlice(stackentry::StackEntrySlice { slice }) => {
            Ok(slice.bytes.clone())
        }
        other => fail!("expected tvm.stackEntryCell or tvm.stackEntrySlice, got {:?}", other),
    }
}

fn stack_cell_to_boc_base64(entry: &StackEntry) -> Result<String> {
    let cell = read_stack_cell_root(entry)?;
    let boc = write_boc(&cell)?;
    Ok(base64_encode(&boc))
}

fn read_stack_cell_root(entry: &StackEntry) -> Result<Cell> {
    let boc = stack_entry_boc(entry)?;
    read_single_root_boc(&boc).map_err(|e| error!("invalid BOC in stack entry: {e}"))
}

fn parse_stack_index_u64(entry: &StackEntry, context: &str) -> Result<u64> {
    let s = read_stack_num(entry)?;
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
            .map_err(|e| error!("failed to parse {context} from hex `{s}`: {e}"))
    } else {
        s.parse::<u64>().map_err(|e| error!("failed to parse {context} from decimal `{s}`: {e}"))
    }
}

fn read_stack_num(entry: &StackEntry) -> Result<String> {
    let number =
        entry.number().ok_or_else(|| error!("expected tvm.stackEntryNumber, got {:?}", entry))?;
    Ok(number.number().clone())
}

fn read_stack_bool(entry: &StackEntry) -> Result<bool> {
    let s = read_stack_num(entry)?;
    let s = s.trim().to_ascii_lowercase();
    Ok(!matches!(s.as_str(), "0" | "0x0"))
}

fn msg_address_to_opt(addr: MsgAddress) -> Result<Option<MsgAddressInt>> {
    Ok(match addr {
        MsgAddress::AddrStd(a) => Some(MsgAddressInt::AddrStd(a)),
        MsgAddress::AddrVar(a) => Some(MsgAddressInt::AddrVar(a)),
        MsgAddress::AddrNone | MsgAddress::AddrExt(_) => None,
    })
}

fn parse_msg_address_from_cell(cell: &Cell) -> Result<Option<MsgAddressInt>> {
    let mut slice = SliceData::load_cell(cell.clone())
        .map_err(|e| error!("failed to load slice from cell: {e}"))?;
    let addr = MsgAddress::construct_from(&mut slice)
        .map_err(|e| error!("failed to parse MsgAddress from cell slice: {e}"))?;
    msg_address_to_opt(addr)
}

fn parse_msg_address_from_stack_entry(entry: &StackEntry) -> Result<Option<MsgAddressInt>> {
    match entry {
        StackEntry::Tvm_StackEntryCell(stackentry::StackEntryCell { cell }) => {
            let root = read_single_root_boc(&cell.bytes)
                .map_err(|e| error!("invalid address cell BOC in stack entry: {e}"))?;
            parse_msg_address_from_cell(&root)
        }
        StackEntry::Tvm_StackEntrySlice(stackentry::StackEntrySlice { slice }) => {
            if let Ok(root) = read_single_root_boc(&slice.bytes) {
                return parse_msg_address_from_cell(&root);
            }

            const ADDR_BITS: usize = 267;

            if slice.bytes.len() * 8 < ADDR_BITS {
                fail!(
                    "stack slice with MsgAddress is too small: have {} bits, need {}",
                    slice.bytes.len() * 8,
                    ADDR_BITS
                );
            }
            let mut s = SliceData::from_raw(slice.bytes.clone(), ADDR_BITS);
            let addr = MsgAddress::construct_from(&mut s)
                .map_err(|e| error!("failed to parse MsgAddress from stack slice: {e}"))?;

            let res = msg_address_to_opt(addr)?;
            Ok(res)
        }
        other => fail!(
            "expected tvm.stackEntryCell or tvm.stackEntrySlice for MsgAddress, got {:?}",
            other
        ),
    }
}

fn parse_jetton_content_from_stack(entry: &StackEntry) -> JsonResult {
    let cell = read_stack_cell_root(entry)?;
    parse_jetton_content_cell(&cell)
}

fn parse_jetton_content_cell(cell: &Cell) -> JsonResult {
    let mut slice = SliceData::load_cell(cell.clone())
        .map_err(|e| error!("failed to load slice from jetton content cell: {e}"))?;
    let prefix = slice.get_next_int(8)?;
    match prefix {
        0 => parse_jetton_content_onchain(&mut slice),
        1 => parse_jetton_content_offchain(&mut slice),
        other => fail!("unexpected TokenData prefix: {other}"),
    }
}

fn parse_jetton_content_offchain(slice: &mut SliceData) -> JsonResult {
    let bytes = read_snake_bytes(slice)?;
    let s = String::from_utf8(bytes)
        .map_err(|e| error!("failed to decode offchain TokenData as UTF-8: {e}"))?;
    Ok(serde_json::json!({
        "type": "offchain",
        "data": s,
    }))
}

fn parse_jetton_content_onchain(slice: &mut SliceData) -> JsonResult {
    let has_data = slice
        .get_next_int(1)
        .map_err(|e| error!("failed to read onchain TokenData has_data flag: {e}"))?
        != 0;
    if !has_data {
        return Ok(serde_json::json!({
            "type": "onchain",
            "data": serde_json::Map::<String, serde_json::Value>::new(),
        }));
    }

    let dict_root: Cell = slice
        .checked_drain_reference()
        .map_err(|e| error!("failed to read TokenData dict root ref: {e}"))?;
    let dict = HashmapE::with_hashmap(256, Some(dict_root));

    let mut data = serde_json::Map::<String, serde_json::Value>::new();
    for name in DEFAULT_JETTON_KEYS {
        let key = token_attr_key(name);
        if let Some(value_slice) = dict
            .get(key)
            .map_err(|e| error!("fail;ed to read TokenData dict entry for `{name}`: {e}"))?
        {
            let value_cell = if value_slice.remaining_bits() == 0
                && value_slice.remaining_references() > 0
            {
                value_slice.reference(0).map_err(|e| {
                    error!("TokenData dict value for `{name}` has invalid reference: {e}")
                })?
            } else {
                let builder = value_slice.as_builder().map_err(|e| {
                    error!("failed to convert TokenData dict slice to cell for `{name}`: {e}")
                })?;
                builder.into_cell().map_err(|e| {
                    error!("failed to convert TokenData value builder into cell for `{name}`: {e}")
                })?
            };

            let value_str = parse_token_attr_value_cell(&value_cell)?;
            data.insert(name.to_owned(), serde_json::Value::String(value_str));
        }
    }
    Ok(serde_json::json!({
        "type": "onchain",
        "data": data,
    }))
}

fn msg_address_to_string(
    addr_opt: &Option<MsgAddressInt>,
    is_testnet: bool,
) -> Result<Option<String>> {
    let addr_mode = if is_testnet {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_TESTNET
    } else {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE
    };
    addr_opt.as_ref().map(|addr| addr.to_string_custom(addr_mode)).transpose()
}

pub(crate) fn parse_jetton_master_data(stack: &[StackEntry], is_testnet: bool) -> JsonResult {
    ensure_stack_len(stack, 5, "get_jetton_data")?;
    let total_supply = parse_stack_index_u64(&stack[0], "jetton total_supply")?;
    let mintable = read_stack_bool(&stack[1])?;
    let admin_addr_opt = parse_msg_address_from_stack_entry(&stack[2])?;
    let admin_address = msg_address_to_string(&admin_addr_opt, is_testnet)?;
    let jetton_content = parse_jetton_content_from_stack(&stack[3])?;
    let jetton_wallet_code = stack_cell_to_boc_base64(&stack[4])?;
    Ok(serde_json::json!({
        "total_supply": total_supply,
        "mintable": mintable,
        "admin_address": admin_address,
        "jetton_content": jetton_content,
        "jetton_wallet_code": jetton_wallet_code,
        "contract_type": "jetton_master",
    }))
}

pub(crate) fn parse_jetton_wallet_data(stack: &[StackEntry], is_testnet: bool) -> JsonResult {
    ensure_stack_len(stack, 4, "get_wallet_data")?;

    // balance
    let balance = parse_stack_index_u64(&stack[0], "jetton balance")?;

    //  owner cell/slice with MsgAddress
    let owner_addr_opt = parse_msg_address_from_stack_entry(&stack[1])?;
    let owner = msg_address_to_string(&owner_addr_opt, is_testnet)?;

    // jetton master address
    let jetton_addr_opt = parse_msg_address_from_stack_entry(&stack[2])?;
    let jetton = msg_address_to_string(&jetton_addr_opt, is_testnet)?;
    let jetton_wallet_code = stack_cell_to_boc_base64(&stack[3])?;

    Ok(serde_json::json!({
        "balance": balance,
        "owner": owner,
        "jetton": jetton,
        "jetton_wallet_code": jetton_wallet_code,
        "contract_type": "jetton_wallet",
    }))
}

pub(crate) fn parse_nft_collection(stack: &[StackEntry], is_testnet: bool) -> JsonResult {
    ensure_stack_len(stack, 3, "get_collection_data")?;
    let next_item_index = parse_stack_index_u64(&stack[0], "nft collection_index")?;
    let collection_content = parse_jetton_content_from_stack(&stack[1])?;
    let owner_addr_opt = parse_msg_address_from_stack_entry(&stack[2])?;
    let owner = msg_address_to_string(&owner_addr_opt, is_testnet)?;
    Ok(serde_json::json!({
        "next_item_index": next_item_index,
        "collection_content": collection_content,
        "owner_address": owner,
        "contract_type": "nft_collection",
    }))
}

pub(crate) fn parse_nft_item_data(stack: &[StackEntry], is_testnet: bool) -> JsonResult {
    ensure_stack_len(stack, 5, "get_nft_data")?;

    let init = read_stack_bool(&stack[0])?;
    let index = parse_stack_index_u64(&stack[1], "nft item index")?;
    let collection_addr_opt = parse_msg_address_from_stack_entry(&stack[2])?;
    let collection_address = msg_address_to_string(&collection_addr_opt, is_testnet)?;
    let owner_addr_opt = parse_msg_address_from_stack_entry(&stack[3])?;
    let owner = msg_address_to_string(&owner_addr_opt, is_testnet)?;
    let content_cell = read_stack_cell_root(&stack[4])?;
    let content = parse_jetton_content_cell(&content_cell)?;

    Ok(serde_json::json!({
        "init": init,
        "index": index,
        "owner_address": owner,
        "collection_address": collection_address,
        "content": content,
        "contract_type": "nft_item",
    }))
}

#[cfg(test)]
#[path = "tests/test_token.rs"]
pub mod test;
