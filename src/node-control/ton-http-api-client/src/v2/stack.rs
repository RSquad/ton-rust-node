/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use base64::Engine;
use serde::ser::SerializeSeq;
use std::fmt::Formatter;
use ton_api::{
    IntoBoxed,
    ton::tvm::{
        List, Number, StackEntry, Tuple, cell::Cell as TvmCell, list, numberdecimal::NumberDecimal,
        slice, stackentry, tuple,
    },
};
use ton_block::{Result as BlockResult, base64_encode};

type ConversionError = anyhow::Error;
type ConvResult<T> = BlockResult<T>;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "@type")]
enum NumberJson {
    #[serde(rename = "tvm.numberDecimal")]
    Decimal { number: String },
}

impl From<&Number> for NumberJson {
    fn from(value: &Number) -> Self {
        match value {
            Number::Tvm_NumberDecimal(inner) => {
                NumberJson::Decimal { number: inner.number.clone() }
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

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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
                let elements: Vec<StackEntry> = elements
                    .into_iter()
                    .map(StackEntry::try_from)
                    .collect::<std::result::Result<_, _>>()?;
                Ok(List::Tvm_List(list::List { elements }))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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
                let elements: Vec<StackEntry> = elements
                    .into_iter()
                    .map(StackEntry::try_from)
                    .collect::<std::result::Result<_, _>>()?;
                Ok(Tuple::Tvm_Tuple(tuple::Tuple { elements }))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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

#[derive(Clone, Debug)]
#[allow(non_camel_case_types)]
pub enum RPCStackEntry {
    Tvm_StackEntryCell(stackentry::StackEntryCell),
    Tvm_StackEntryList(stackentry::StackEntryList),
    Tvm_StackEntryNumber(stackentry::StackEntryNumber),
    Tvm_StackEntrySlice(stackentry::StackEntrySlice),
    Tvm_StackEntryTuple(stackentry::StackEntryTuple),
    Tvm_StackEntryUnsupported,
}

impl Into<StackEntry> for RPCStackEntry {
    fn into(self) -> StackEntry {
        match self {
            RPCStackEntry::Tvm_StackEntryCell(e) => StackEntry::Tvm_StackEntryCell(e),
            RPCStackEntry::Tvm_StackEntryList(e) => StackEntry::Tvm_StackEntryList(e),
            RPCStackEntry::Tvm_StackEntryNumber(e) => StackEntry::Tvm_StackEntryNumber(e),
            RPCStackEntry::Tvm_StackEntrySlice(e) => StackEntry::Tvm_StackEntrySlice(e),
            RPCStackEntry::Tvm_StackEntryTuple(e) => StackEntry::Tvm_StackEntryTuple(e),
            RPCStackEntry::Tvm_StackEntryUnsupported => StackEntry::Tvm_StackEntryUnsupported,
        }
    }
}

impl From<StackEntry> for RPCStackEntry {
    fn from(value: StackEntry) -> Self {
        match value {
            StackEntry::Tvm_StackEntryCell(e) => RPCStackEntry::Tvm_StackEntryCell(e),
            StackEntry::Tvm_StackEntryList(e) => RPCStackEntry::Tvm_StackEntryList(e),
            StackEntry::Tvm_StackEntryNumber(e) => RPCStackEntry::Tvm_StackEntryNumber(e),
            StackEntry::Tvm_StackEntrySlice(e) => RPCStackEntry::Tvm_StackEntrySlice(e),
            StackEntry::Tvm_StackEntryTuple(e) => RPCStackEntry::Tvm_StackEntryTuple(e),
            StackEntry::Tvm_StackEntryUnsupported => RPCStackEntry::Tvm_StackEntryUnsupported,
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

            fn expecting(&self, f: &mut Formatter) -> std::fmt::Result {
                f.write_str("a TVM stack entry encoded as a tagged sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
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
                    "unsupported" => Ok(RPCStackEntry::Tvm_StackEntryUnsupported),
                    _other => Err(serde::de::Error::unknown_variant(
                        &tag_raw,
                        &["num", "cell", "slice", "list", "tuple", "unsupported"],
                    )),
                }
            }
        }

        deserializer.deserialize_seq(StackEntryVisitor)
    }
}

impl serde::Serialize for RPCStackEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let entry = self.clone().into();
        let json_value = serialize_stack_entry(&entry).map_err(serde::ser::Error::custom)?;
        let arr = json_value.as_array().ok_or(serde::ser::Error::custom("Expected array"))?;
        let mut seq = serializer.serialize_seq(Some(2))?;
        seq.serialize_element(&arr[0])?;
        seq.serialize_element(&arr[1])?;
        seq.end()
    }
}

fn serialize_stack_entry(entry: &StackEntry) -> Result<serde_json::Value, String> {
    let result = match entry {
        StackEntry::Tvm_StackEntryNumber(value) => {
            serde_json::json!(["num", value.number.number()])
        }
        StackEntry::Tvm_StackEntryCell(cell) => {
            let bytes = base64_encode(&cell.cell.bytes);
            serde_json::json!(["cell", { "bytes": bytes }])
        }
        StackEntry::Tvm_StackEntrySlice(slice_entry) => {
            serde_json::json!(["slice", base64_encode(&slice_entry.slice.bytes)])
        }
        StackEntry::Tvm_StackEntryList(list) => {
            let elements: Result<Vec<_>, _> =
                list.list.elements().iter().map(serialize_stack_entry).collect();
            serde_json::json!(["list", { "@type": "tvm.list", "elements": elements? }])
        }
        StackEntry::Tvm_StackEntryTuple(tuple) => {
            let elements: Result<Vec<_>, _> =
                tuple.tuple.elements().iter().map(serialize_stack_entry).collect();
            serde_json::json!(["tuple", { "@type": "tvm.tuple", "elements": elements? }])
        }
        StackEntry::Tvm_StackEntryUnsupported => {
            return Err("Unsupported stack entry".to_string());
        }
    };
    Ok(result)
}

pub fn base64_decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>, base64::DecodeError> {
    Ok(base64::engine::general_purpose::STANDARD.decode(input)?)
}
