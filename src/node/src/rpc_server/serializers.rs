/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::rpc_server::JsonResult;
use std::{
    convert::{TryFrom, TryInto},
    fmt,
};
use ton_api::{
    ton::tvm::{
        cell::Cell as TvmCell, list, numberdecimal::NumberDecimal, slice, stackentry, tuple, List,
        Number, StackEntry, Tuple,
    },
    IntoBoxed,
};
use ton_block::{
    base64_decode, base64_encode, read_single_root_boc, BlockIdExt, BocFlags, BocWriter, Cell,
    CellType, Deserializable, Error as TonError, Message, Serializable, ShardAccount, SliceData,
    Transaction, TransactionDescr, UInt256, ADDR_FORMAT_BOUNCE, ADDR_FORMAT_TESTNET,
    ADDR_FORMAT_URL_SAFE,
};

pub(crate) fn serialize_cell_opt(cell: Option<&Cell>) -> String {
    if let Some(cell) = cell {
        match write_boc(cell) {
            Ok(boc) => base64_encode(&boc),
            Err(err) => format!("Failed to serialize cell to boc: {err}"),
        }
    } else {
        String::new()
    }
}

pub(crate) fn write_boc(root_cell: &Cell) -> ton_block::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let writer = BocWriter::with_flags([root_cell.clone()], BocFlags::Crc32)?;
    writer.write(&mut buf)?;
    Ok(buf)
}

fn decode_comment_from_body(body: &SliceData) -> Option<String> {
    let mut slice = body.clone();
    if slice.remaining_bits() < 32 {
        return None;
    }

    let op = slice.get_next_u32().ok()?;
    if op != 0 {
        return None;
    }

    let bits_left = slice.remaining_bits();
    if bits_left == 0 {
        return Some(String::new());
    }
    if bits_left % 8 != 0 {
        return None;
    }

    let bytes_len = bits_left / 8;
    let bytes = slice.get_next_bytes(bytes_len).ok()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn serialize_block_id(block_id: &BlockIdExt) -> serde_json::Value {
    serde_json::json!({
        "@type": "ton.blockIdExt",
        "workchain": block_id.shard().workchain_id(),
        "shard": (block_id.shard().shard_prefix_with_tag() as i64).to_string(),
        "seqno": block_id.seq_no(),
        "root_hash": serialize_uint256(block_id.root_hash()),
        "file_hash": serialize_uint256(block_id.file_hash())
    })
}

pub(crate) fn serialize_shard_account(shard_account: &ShardAccount) -> serde_json::Value {
    serde_json::json!({
        "@type": "internal.transactionId",
        "lt": shard_account.last_trans_lt().to_string(),
        "hash": serialize_uint256(shard_account.last_trans_hash())
    })
}

pub(crate) fn serialize_uint256(id: &UInt256) -> serde_json::Value {
    serde_json::json!(base64_encode(id.as_slice()))
}

pub(crate) fn serialize_transaction(
    tr: &Transaction,
    tr_cell: Cell,
    address: &str,
    type_name: &str,
    account: Option<&str>,
    testnet: bool,
) -> JsonResult {
    let message_format = match type_name {
        "ext.transaction" => MessageFormat::Ext,
        _ => MessageFormat::Raw,
    };
    let descr = tr.read_description()?;
    let action_phase_fees = match &descr {
        TransactionDescr::Ordinary(descr) => {
            descr.action.as_ref().map_or(0, action_phase_external_fees)
        }
        TransactionDescr::TickTock(descr) => {
            descr.action.as_ref().map_or(0, action_phase_external_fees)
        }
        TransactionDescr::SplitPrepare(descr) => {
            descr.action.as_ref().map_or(0, action_phase_external_fees)
        }
        TransactionDescr::MergeInstall(descr) => {
            descr.action.as_ref().map_or(0, action_phase_external_fees)
        }
        _ => 0,
    };
    let fee = tr.total_fees().coins.as_u128() + action_phase_fees;
    let storage_fee = match &descr {
        TransactionDescr::Ordinary(descr) => {
            descr.storage_ph.as_ref().map_or(0, |ph| ph.storage_fees_collected.as_u128())
        }
        TransactionDescr::TickTock(descr) => descr.storage.storage_fees_collected.as_u128(),
        _ => 0, // wrong tr type
    };
    let in_msg = if let Some(msg_cell) = tr.in_msg_cell() {
        Some(serialize_message(msg_cell, None, Some(address), testnet, message_format)?)
    } else {
        None
    };
    let mut out_msgs = Vec::new();
    tr.out_msgs.iterate_slices(|slice| {
        out_msgs.push(serialize_message(
            slice.reference(0)?,
            Some(address),
            None,
            testnet,
            message_format,
        )?);
        Ok(true)
    })?;

    let mut obj = serde_json::Map::new();
    obj.insert("@type".into(), serde_json::Value::String(type_name.into()));
    obj.insert(
        "address".into(),
        serde_json::json!({
            "@type": "accountAddress",
            "account_address": address,
        }),
    );
    if let Some(acc) = account {
        obj.insert("account".into(), serde_json::Value::String(acc.to_string()));
    }
    obj.insert("utime".into(), serde_json::json!(tr.now()));
    obj.insert("data".into(), serde_json::json!(serialize_cell_opt(Some(&tr_cell))));
    obj.insert(
        "transaction_id".into(),
        serde_json::json!({
            "@type": "internal.transactionId",
            "lt": tr.logical_time().to_string(),
            "hash": serialize_uint256(&tr_cell.repr_hash()),
        }),
    );
    obj.insert("fee".into(), serde_json::json!(fee.to_string()));
    obj.insert("storage_fee".into(), serde_json::json!(storage_fee.to_string()));
    obj.insert("other_fee".into(), serde_json::json!((fee - storage_fee).to_string()));
    if let Some(in_msg) = in_msg {
        obj.insert("in_msg".into(), in_msg);
    }
    obj.insert("out_msgs".into(), serde_json::json!(out_msgs));
    Ok(serde_json::Value::Object(obj))
}

fn action_phase_external_fees(action: &ton_block::TrActionPhase) -> u128 {
    action.total_fwd_fees().as_u128().saturating_sub(action.total_action_fees().as_u128())
}

fn serialize_message(
    msg_cell: Cell,
    src: Option<&str>,
    dst: Option<&str>,
    testnet: bool,
    format: MessageFormat,
) -> JsonResult {
    let hash = msg_cell.repr_hash().clone();
    let msg = Message::construct_from_cell(msg_cell).unwrap_or_default();
    let int_header = msg.int_header().cloned().unwrap_or_default();
    let (body, body_hash, body_opt) = if let Some(body) = msg.body() {
        let body_cell = body.clone().into_cell().unwrap_or_default();
        let bh = body_cell.repr_hash().clone();
        (serialize_cell_opt(Some(&body_cell)), bh, Some(body))
    } else {
        // toncenter v2: empty body is serialized as BoC of an empty cell, not as ""
        let empty = Cell::default();
        let bh = empty.repr_hash().clone();
        (serialize_cell_opt(Some(&empty)), bh, None)
    };
    let init_state =
        serialize_cell_opt(msg.state_init().and_then(|init| init.serialize().ok()).as_ref());
    let addr_mode = if testnet {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_TESTNET
    } else {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE
    };
    let source = src.map_or_else(|| msg.src_to_string(addr_mode), |s| Ok(s.to_string()))?;
    let destination = dst.map_or_else(|| msg.dst_to_string(addr_mode), |s| Ok(s.to_string()))?;
    let mut msg_data_json = serde_json::json!({
        "@type": "msg.dataRaw",
        "body": &body,
        "init_state": &init_state,
    });

    if let Some(body) = &body_opt {
        if let Some(text) = decode_comment_from_body(body) {
            let text_b64 = base64_encode(text.as_bytes());
            msg_data_json = serde_json::json!({
                "@type": "msg.dataText",
                "text": text_b64,
            });
        }
    }
    let message = body_opt
        .as_ref()
        .map(|body| format!("{}\n", base64_encode(body.get_bytestring(0).as_slice())))
        .unwrap_or_default();

    let (msg_type, source, destination) = match format {
        MessageFormat::Raw => (
            "raw.message",
            serde_json::json!({
                "@type": "accountAddress",
                "account_address": source,
            }),
            serde_json::json!({
                "@type": "accountAddress",
                "account_address": destination,
            }),
        ),
        MessageFormat::Ext => {
            ("ext.message", serde_json::json!(source), serde_json::json!(destination))
        }
    };

    Ok(serde_json::json!({
        "@type": msg_type,
        "hash": serialize_uint256(&hash),
        "source": source,
        "destination": destination,
        "value": int_header.value.coins.to_string(),
        "extra_currencies": Vec::<String>::new(), // TODO: fill extra currencies,
        "fwd_fee": int_header.fwd_fee.to_string(),
        "ihr_fee": int_header.extra_flags.to_string(),
        "created_lt": int_header.created_lt.to_string(),
        "body_hash": serialize_uint256(&body_hash),
        "msg_data": msg_data_json,
        "message": message,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageFormat {
    Raw,
    Ext,
}

type ConversionError = TonError;
type ConvResult<T> = ton_block::Result<T>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum NumberJson {
    #[serde(rename = "tvm.numberDecimal")]
    Decimal { number: String },
}

fn normalize_number_decimal(number: &str) -> String {
    let (negative, value) =
        if let Some(value) = number.strip_prefix('-') { (true, value) } else { (false, number) };
    let Some(hex_digits) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) else {
        return number.to_owned();
    };
    if hex_digits.is_empty() {
        return number.to_owned();
    }
    let Some(parsed) = num_bigint::BigInt::parse_bytes(hex_digits.as_bytes(), 16) else {
        return number.to_owned();
    };
    let parsed = if negative { -parsed } else { parsed };
    parsed.to_str_radix(10)
}

impl From<&Number> for NumberJson {
    fn from(value: &Number) -> Self {
        match value {
            Number::Tvm_NumberDecimal(inner) => {
                NumberJson::Decimal { number: normalize_number_decimal(&inner.number) }
            }
        }
    }
}

impl TryFrom<NumberJson> for Number {
    type Error = ConversionError;

    fn try_from(value: NumberJson) -> ConvResult<Self> {
        match value {
            NumberJson::Decimal { number } => {
                Ok(Number::Tvm_NumberDecimal(NumberDecimal { number }))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum CellJson {
    #[serde(rename = "tvm.cell")]
    Cell { bytes: String },
}

impl From<&TvmCell> for CellJson {
    fn from(value: &TvmCell) -> Self {
        CellJson::Cell { bytes: base64_encode(&value.bytes) }
    }
}

impl TryFrom<CellJson> for TvmCell {
    type Error = ConversionError;

    fn try_from(value: CellJson) -> ConvResult<Self> {
        match value {
            CellJson::Cell { bytes } => {
                let decoded = base64_decode(&bytes)?;
                Ok(TvmCell { bytes: decoded })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum SliceJson {
    #[serde(rename = "tvm.slice")]
    Slice { bytes: String },
}

impl From<&slice::Slice> for SliceJson {
    fn from(value: &slice::Slice) -> Self {
        SliceJson::Slice { bytes: base64_encode(&value.bytes) }
    }
}

impl TryFrom<SliceJson> for slice::Slice {
    type Error = ConversionError;
    fn try_from(value: SliceJson) -> ConvResult<Self> {
        match value {
            SliceJson::Slice { bytes } => {
                let decoded = base64_decode(&bytes)?;
                Ok(slice::Slice { bytes: decoded })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum ListJson {
    #[serde(rename = "tvm.list")]
    List { elements: Vec<StackEntryJson> },
}

impl From<&List> for ListJson {
    fn from(value: &List) -> Self {
        match value {
            List::Tvm_List(inner) => {
                let elements = inner.elements.iter().map(StackEntryJson::from).collect();
                ListJson::List { elements }
            }
        }
    }
}

impl TryFrom<ListJson> for List {
    type Error = ConversionError;
    fn try_from(value: ListJson) -> ConvResult<Self> {
        match value {
            ListJson::List { elements } => {
                let elements: Vec<StackEntry> =
                    elements.into_iter().map(StackEntry::try_from).collect::<Result<_, _>>()?;
                Ok(List::Tvm_List(list::List { elements }))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum TupleJson {
    #[serde(rename = "tvm.tuple")]
    Tuple { elements: Vec<StackEntryJson> },
}

impl From<&Tuple> for TupleJson {
    fn from(value: &Tuple) -> Self {
        match value {
            Tuple::Tvm_Tuple(inner) => {
                let elements = inner.elements.iter().map(StackEntryJson::from).collect();
                TupleJson::Tuple { elements }
            }
        }
    }
}

impl TryFrom<TupleJson> for Tuple {
    type Error = ConversionError;
    fn try_from(value: TupleJson) -> ConvResult<Self> {
        match value {
            TupleJson::Tuple { elements } => {
                let elements: Vec<StackEntry> =
                    elements.into_iter().map(StackEntry::try_from).collect::<Result<_, _>>()?;
                Ok(Tuple::Tvm_Tuple(tuple::Tuple { elements }))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "@type")]
enum StackEntryJson {
    #[serde(rename = "tvm.stackEntryNumber")]
    Number { number: NumberJson },
    #[serde(rename = "tvm.stackEntryCell")]
    Cell { cell: CellJson },
    #[serde(rename = "tvm.stackEntryList")]
    List { list: ListJson },
    #[serde(rename = "tvm.stackEntrySlice")]
    Slice { slice: SliceJson },
    #[serde(rename = "tvm.stackEntryTuple")]
    Tuple { tuple: TupleJson },
    #[serde(rename = "tvm.stackEntryUnsupported")]
    Unsupported,
}

impl From<&StackEntry> for StackEntryJson {
    fn from(value: &StackEntry) -> Self {
        match value {
            StackEntry::Tvm_StackEntryNumber(inner) => {
                StackEntryJson::Number { number: NumberJson::from(&inner.number) }
            }
            StackEntry::Tvm_StackEntryCell(inner) => {
                StackEntryJson::Cell { cell: CellJson::from(&inner.cell) }
            }
            StackEntry::Tvm_StackEntryList(inner) => {
                StackEntryJson::List { list: ListJson::from(&inner.list) }
            }
            StackEntry::Tvm_StackEntrySlice(inner) => {
                StackEntryJson::Slice { slice: SliceJson::from(&inner.slice) }
            }
            StackEntry::Tvm_StackEntryTuple(inner) => {
                StackEntryJson::Tuple { tuple: TupleJson::from(&inner.tuple) }
            }
            StackEntry::Tvm_StackEntryUnsupported => StackEntryJson::Unsupported,
        }
    }
}

impl TryFrom<StackEntryJson> for StackEntry {
    type Error = ConversionError;

    fn try_from(value: StackEntryJson) -> ConvResult<Self> {
        Ok(match value {
            StackEntryJson::Number { number } => {
                stackentry::StackEntryNumber { number: number.try_into()? }.into_boxed()
            }
            StackEntryJson::Cell { cell } => {
                stackentry::StackEntryCell { cell: cell.try_into()? }.into_boxed()
            }
            StackEntryJson::List { list } => {
                stackentry::StackEntryList { list: list.try_into()? }.into_boxed()
            }
            StackEntryJson::Slice { slice } => {
                stackentry::StackEntrySlice { slice: slice.try_into()? }.into_boxed()
            }
            StackEntryJson::Tuple { tuple } => {
                StackEntry::Tvm_StackEntryTuple(stackentry::StackEntryTuple {
                    tuple: tuple.try_into()?,
                })
            }
            StackEntryJson::Unsupported => StackEntry::Tvm_StackEntryUnsupported,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CellObject {
    data: CellDataObject,
    refs: Vec<CellObject>,
    special: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CellDataObject {
    b64: String,
    len: usize,
}

fn _cell_object_from_boc(bytes: &[u8]) -> ConvResult<CellObject> {
    let root = read_single_root_boc(bytes)?;
    build_cell_object(&root)
}

fn build_cell_object(cell: &Cell) -> ConvResult<CellObject> {
    let refs = (0..cell.references_count())
        .map(|idx| {
            let reference = cell.reference(idx)?;
            build_cell_object(&reference)
        })
        .collect::<Result<_, _>>()?;
    Ok(CellObject {
        data: CellDataObject { b64: base64_encode(cell.data()), len: cell.bit_length() },
        refs,
        special: cell.cell_type() != CellType::Ordinary,
    })
}

#[derive(serde::Serialize)]
struct CellEntrySerializable {
    bytes: String,
    object: CellObject,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CellInput {
    Simple(String),
    Detailed {
        bytes: String,
        #[serde(default)]
        _object: Option<serde::de::IgnoredAny>,
    },
}

impl CellInput {
    fn into_bytes(self) -> ConvResult<Vec<u8>> {
        let data = match self {
            CellInput::Simple(bytes) => bytes,
            CellInput::Detailed { bytes, .. } => bytes,
        };
        base64_decode(&data).map_err(ConversionError::from)
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SliceInput {
    Simple(String),
    Detailed {
        bytes: String,
        #[serde(default)]
        _object: Option<serde::de::IgnoredAny>,
    },
}

impl SliceInput {
    fn into_bytes(self) -> ConvResult<Vec<u8>> {
        let data = match self {
            SliceInput::Simple(bytes) => bytes,
            SliceInput::Detailed { bytes, .. } => bytes,
        };
        base64_decode(&data).map_err(ConversionError::from)
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum NumberInput {
    String(String),
    Int(i64),
    UInt(u64),
    Float(f64),
}

impl NumberInput {
    fn into_string(self) -> String {
        match self {
            NumberInput::String(s) => s,
            NumberInput::Int(v) => v.to_string(),
            NumberInput::UInt(v) => v.to_string(),
            NumberInput::Float(v) => {
                if v.fract() == 0.0 {
                    format!("{:.0}", v)
                } else {
                    v.to_string()
                }
            }
        }
    }
}

pub(crate) fn serialize_stack<Item>(stack: impl IntoIterator<Item = Item>) -> JsonResult
where
    Item: Into<StackEntry>,
{
    stack.into_iter().map(|e| serialize_stack_entry(&e.into())).collect()
}

fn serialize_stack_entry(entry: &StackEntry) -> JsonResult {
    let result = match entry {
        StackEntry::Tvm_StackEntryNumber(value) => {
            serde_json::json!(["num", value.number.number()])
        }
        StackEntry::Tvm_StackEntryCell(cell) => {
            let root_cell = read_single_root_boc(&cell.cell.bytes)?;
            let boc = write_boc(&root_cell)?;
            let bytes = base64_encode(&boc);
            let object = build_cell_object(&root_cell)?;
            serde_json::json!(["cell", CellEntrySerializable { bytes, object }])
        }
        StackEntry::Tvm_StackEntryList(list) => {
            serde_json::json!(["list", ListJson::from(&list.list)])
        }
        StackEntry::Tvm_StackEntrySlice(slice_entry) => {
            serde_json::json!(["slice", base64_encode(&slice_entry.slice.bytes)])
        }
        StackEntry::Tvm_StackEntryTuple(tuple_entry) => {
            serde_json::json!(["tuple", TupleJson::from(&tuple_entry.tuple)])
        }
        StackEntry::Tvm_StackEntryUnsupported => {
            serde_json::json!(["unsupported"])
        }
    };
    Ok(result)
}

#[derive(Debug)]
#[allow(non_camel_case_types)]
pub(crate) enum RPCStackEntry {
    Tvm_StackEntryCell(stackentry::StackEntryCell),
    Tvm_StackEntryList(stackentry::StackEntryList),
    Tvm_StackEntryNumber(stackentry::StackEntryNumber),
    Tvm_StackEntrySlice(stackentry::StackEntrySlice),
    Tvm_StackEntryTuple(stackentry::StackEntryTuple),
}

impl Into<StackEntry> for RPCStackEntry {
    fn into(self) -> StackEntry {
        match self {
            RPCStackEntry::Tvm_StackEntryCell(e) => StackEntry::Tvm_StackEntryCell(e),
            RPCStackEntry::Tvm_StackEntryList(e) => StackEntry::Tvm_StackEntryList(e),
            RPCStackEntry::Tvm_StackEntryNumber(e) => StackEntry::Tvm_StackEntryNumber(e),
            RPCStackEntry::Tvm_StackEntrySlice(e) => StackEntry::Tvm_StackEntrySlice(e),
            RPCStackEntry::Tvm_StackEntryTuple(e) => StackEntry::Tvm_StackEntryTuple(e),
        }
    }
}

impl<'de> serde::Deserialize<'de> for RPCStackEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StackEntryVisitor;

        impl<'de> serde::de::Visitor<'de> for StackEntryVisitor {
            type Value = RPCStackEntry;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a TVM stack entry encoded as a tagged sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let tag_raw: String = seq.next_element()?.ok_or_else(|| {
                    serde::de::Error::invalid_length(0, &"tag as the first element")
                })?;
                let tag = tag_raw.to_lowercase();
                match tag.as_str() {
                    "num" | "number" | "tvm.numberdecimal" => {
                        let value: NumberInput = seq.next_element()?.ok_or_else(|| {
                            serde::de::Error::invalid_length(1, &"payload for `num`")
                        })?;
                        let inner = stackentry::StackEntryNumber {
                            number: Number::Tvm_NumberDecimal(NumberDecimal {
                                number: value.into_string(),
                            }),
                        };
                        Ok(RPCStackEntry::Tvm_StackEntryNumber(inner))
                    }
                    "cell" | "tvm.cell" => {
                        let payload: CellInput = seq.next_element()?.ok_or_else(|| {
                            serde::de::Error::invalid_length(1, &"payload for `cell`")
                        })?;
                        let bytes = payload.into_bytes().map_err(|e| {
                            serde::de::Error::custom(format!("invalid base64 for `cell`: {e}"))
                        })?;
                        let inner = stackentry::StackEntryCell { cell: TvmCell { bytes } };
                        Ok(RPCStackEntry::Tvm_StackEntryCell(inner))
                    }
                    "slice" | "tvm.slice" => {
                        let payload: SliceInput = seq.next_element()?.ok_or_else(|| {
                            serde::de::Error::invalid_length(1, &"payload for `slice`")
                        })?;
                        let bytes = payload.into_bytes().map_err(|e| {
                            serde::de::Error::custom(format!("invalid base64 for `slice`: {e}"))
                        })?;
                        let inner = stackentry::StackEntrySlice {
                            slice: ton_api::ton::tvm::slice::Slice { bytes },
                        };
                        Ok(RPCStackEntry::Tvm_StackEntrySlice(inner))
                    }
                    "list" | "tvm.list" => {
                        let payload: ListJson = seq.next_element()?.ok_or_else(|| {
                            serde::de::Error::invalid_length(1, &"payload for `list`")
                        })?;
                        let list = payload.try_into().map_err(|e: ConversionError| {
                            serde::de::Error::custom(e.to_string())
                        })?;
                        let inner = stackentry::StackEntryList { list };
                        Ok(RPCStackEntry::Tvm_StackEntryList(inner))
                    }
                    "tuple" | "tvm.tuple" => {
                        let payload: TupleJson = seq.next_element()?.ok_or_else(|| {
                            serde::de::Error::invalid_length(1, &"payload for `tuple`")
                        })?;
                        let tuple = payload.try_into().map_err(|e: ConversionError| {
                            serde::de::Error::custom(e.to_string())
                        })?;
                        let inner = stackentry::StackEntryTuple { tuple };
                        Ok(RPCStackEntry::Tvm_StackEntryTuple(inner))
                    }
                    _other => Err(serde::de::Error::unknown_variant(
                        &tag_raw,
                        &["num", "cell", "slice", "list", "tuple"],
                    )),
                }
            }
        }

        deserializer.deserialize_seq(StackEntryVisitor)
    }
}

#[cfg(test)]
#[path = "tests/test_serializers.rs"]
pub mod test;
