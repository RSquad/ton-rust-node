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
use crate::stack::{
    continuation::ContinuationData,
    integer::{conversion::FromInt, IntegerData},
};
use std::{
    cmp::Ordering,
    fmt, mem,
    ops::{Range, RangeInclusive},
    slice::Iter,
    sync::Arc,
};
use ton_block::{
    error, fail, BuilderData, Cell, CellType, ExceptionCode, Result, Serializable, SliceData,
    Status,
};

pub mod continuation;
pub mod savelist;
#[macro_use]
pub mod integer;

#[macro_export]
macro_rules! int {
    (nan) => {
        StackItem::nan()
    };
    ($value: expr) => {
        StackItem::integer(IntegerData::from($value).unwrap())
    };
    (parse $str: expr) => {
        StackItem::integer(std::str::FromStr::from_str($str).unwrap())
    };
    (parse_hex $str: expr) => {
        StackItem::integer(IntegerData::from_str_radix($str, 16).unwrap())
    };
}

#[macro_export]
macro_rules! boolean {
    ($val:expr) => {
        if $val {
            int!(-1)
        } else {
            int!(0)
        }
    };
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum StackItem {
    #[default]
    None,
    Builder(Arc<BuilderData>),
    Cell(Cell),
    Continuation(Arc<ContinuationData>),
    Integer(Arc<IntegerData>),
    Slice(SliceData),
    Tuple(Arc<Vec<StackItem>>),
}

impl Drop for StackItem {
    fn drop(&mut self) {
        if self.is_tuple() || self.is_continuation() {
            let mut stack = vec![];
            collect_items(&mut stack, self);
            while let Some(ref mut item) = stack.pop() {
                collect_items(&mut stack, item);
            }
        }
    }
}

fn collect_items(stack: &mut Vec<StackItem>, item: &mut StackItem) {
    match item {
        StackItem::Tuple(data) => match Arc::try_unwrap(std::mem::take(data)) {
            Ok(ref mut tuple) => {
                for item in tuple {
                    stack.push(std::mem::take(item))
                }
            }
            Err(data) => drop(data),
        },
        StackItem::Continuation(data) => match Arc::try_unwrap(std::mem::take(data)) {
            Ok(ref mut cont) => {
                for creg in [0, 1, 2, 3, 7] {
                    if let Some(item) = cont.savelist.get_mut(creg) {
                        stack.push(std::mem::take(item))
                    }
                }
                for item in &mut cont.stack.storage {
                    stack.push(std::mem::take(item))
                }
            }
            Err(data) => drop(data),
        },
        _ => (),
    }
}

impl StackItem {
    /// new default stack item
    pub const fn default() -> Self {
        StackItem::None
    }

    /// new stack item as builder
    pub fn builder(builder: BuilderData) -> Self {
        StackItem::Builder(Arc::new(builder))
    }

    /// new stack item as cell
    pub fn cell(cell: Cell) -> Self {
        StackItem::Cell(cell)
    }

    /// new stack item as cell
    pub fn dict(root: Option<&Cell>) -> Self {
        root.cloned().map_or(StackItem::None, StackItem::Cell)
    }

    /// new stack item as continuation
    pub fn continuation(continuation: ContinuationData) -> Self {
        StackItem::Continuation(Arc::new(continuation))
    }

    /// new stack item as integer
    pub fn int(integer: impl Into<IntegerData>) -> Self {
        StackItem::Integer(Arc::new(integer.into()))
    }

    /// new stack item as integer from string
    pub fn big_int(integer: &str) -> Result<Self> {
        Ok(StackItem::Integer(Arc::new(integer.parse()?)))
    }

    /// new stack item as integer with internal data
    pub fn integer(integer: IntegerData) -> Self {
        StackItem::Integer(Arc::new(integer))
    }

    /// new stack item as integer not a number
    pub fn nan() -> Self {
        StackItem::Integer(Arc::new(IntegerData::nan()))
    }

    /// new stack item as bool
    pub fn boolean(boolean: bool) -> Self {
        match boolean {
            true => StackItem::int(IntegerData::minus_one()),
            false => StackItem::int(IntegerData::zero()),
        }
    }

    /// new stack item as slice
    pub fn slice(slice: SliceData) -> Self {
        StackItem::Slice(slice)
    }

    /// new stack item as slice from bistring
    pub fn to_slice(item: impl Serializable) -> Result<Self> {
        let slice = item.write_to_bitstring()?;
        Ok(StackItem::Slice(slice))
    }

    pub fn bitstring(item: &str) -> Result<Self> {
        let slice = SliceData::from_string(item)?;
        Ok(StackItem::Slice(slice))
    }

    /// new stack item as tuple
    pub fn tuple(tuple: Vec<StackItem>) -> Self {
        StackItem::Tuple(Arc::new(tuple))
    }

    /// Returns integer not equal to zero
    /// Checks type and NaN
    pub fn as_bool(&self) -> Result<bool> {
        match self {
            StackItem::Integer(data) => {
                if data.is_nan() {
                    fail!(ExceptionCode::IntegerOverflow)
                } else {
                    Ok(!data.is_zero())
                }
            }
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a bool"),
        }
    }

    pub fn as_builder(&self) -> Result<&BuilderData> {
        match self {
            StackItem::Builder(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a builder"),
        }
    }

    /// Extracts builder to modify, exceptions should not be after
    /// If is single reference it will not clone on write
    pub fn as_builder_mut(&mut self) -> Result<BuilderData> {
        match self {
            StackItem::Builder(data) => Ok(mem::take(Arc::make_mut(data))),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a builder"),
        }
    }

    pub fn as_cell(&self) -> Result<&Cell> {
        match self {
            StackItem::Cell(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a cell"),
        }
    }

    pub fn as_continuation(&self) -> Result<&ContinuationData> {
        match self {
            StackItem::Continuation(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item {} is not a continuation", self),
        }
    }

    pub fn as_continuation_mut(&mut self) -> Result<&mut ContinuationData> {
        match self {
            StackItem::Continuation(data) => Ok(Arc::make_mut(data)),
            _ => fail!(ExceptionCode::TypeCheckError, "item {} is not a continuation", self),
        }
    }

    /// Returns type D None or Cell
    pub fn as_dict(&self) -> Result<Option<&Cell>> {
        match self {
            StackItem::None => Ok(None),
            StackItem::Cell(data) => Ok(Some(data)),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a dictionary"),
        }
    }

    pub fn as_integer(&self) -> Result<&IntegerData> {
        match self {
            StackItem::Integer(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not an integer"),
        }
    }

    pub fn as_integer_value<T>(&self, range: RangeInclusive<T>) -> Result<T>
    where
        T: PartialOrd + std::fmt::Display + FromInt,
    {
        self.as_integer()?.as_integer_value(range)
    }

    pub fn as_stack_index(&self) -> Result<usize> {
        self.as_integer_value(0..=(1 << 30) - 1)
    }

    pub fn as_integer_mut(&mut self) -> Result<&mut IntegerData> {
        match self {
            StackItem::Integer(data) => Ok(Arc::make_mut(data)),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not an integer"),
        }
    }

    pub fn as_slice(&self) -> Result<&SliceData> {
        match self {
            StackItem::Slice(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item {} is not a slice", self),
        }
    }

    pub fn as_tuple(&self) -> Result<&[StackItem]> {
        match self {
            StackItem::Tuple(data) => Ok(data),
            _ => fail!(ExceptionCode::TypeCheckError, "item {} is not a tuple", self),
        }
    }

    pub fn tuple_item(&self, index: usize, default: bool) -> Result<StackItem> {
        let tuple = self.as_tuple()?;
        match tuple.get(index) {
            Some(value) => Ok(value.clone()),
            None if default => Ok(StackItem::None),
            None => fail!(
                ExceptionCode::RangeCheckError,
                "tuple index is {} but length is {}",
                index,
                tuple.len()
            ),
        }
    }

    pub fn tuple_item_ref(&self, index: usize) -> Result<&StackItem> {
        let tuple = self.as_tuple()?;
        match tuple.get(index) {
            Some(value) => Ok(value),
            None => fail!(
                ExceptionCode::RangeCheckError,
                "tuple index is {} but length is {}",
                index,
                tuple.len()
            ),
        }
    }

    /// Extracts tuple to modify, exceptions should not be after
    /// If is single reference it will not clone on write
    pub fn as_tuple_mut(&mut self) -> Result<Vec<StackItem>> {
        match self {
            StackItem::Tuple(data) => Ok(mem::take(Arc::make_mut(data))),
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a tuple"),
        }
    }

    // Extracts tuple items
    pub fn withdraw_tuple_part(&mut self, length: usize) -> Result<Vec<StackItem>> {
        match self {
            StackItem::Tuple(arc) => match Arc::try_unwrap(mem::take(arc)) {
                Ok(mut tuple) => {
                    tuple.truncate(length);
                    Ok(tuple)
                }
                Err(arc) => Ok(arc[0..length].to_vec()),
            },
            _ => fail!(ExceptionCode::TypeCheckError, "item is not a tuple"),
        }
    }

    /// Returns integer as coins and checks range 0..2^120
    pub fn as_coins(&self) -> Result<u128> {
        self.as_integer_value(0..=(1u128 << 120) - 1)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, StackItem::None)
    }

    pub fn is_zero(&self) -> bool {
        if let StackItem::Integer(data) = self {
            return data.is_zero();
        }
        false
    }

    pub fn is_slice(&self) -> bool {
        matches!(self, StackItem::Slice(_))
    }

    pub fn is_tuple(&self) -> bool {
        matches!(self, StackItem::Tuple(_))
    }

    pub fn is_continuation(&self) -> bool {
        matches!(self, StackItem::Continuation(_))
    }

    pub fn withdraw(&mut self) -> StackItem {
        mem::take(self)
    }

    pub fn dump_as_fift(&self) -> String {
        match self {
            StackItem::None => "(null)".to_string(),
            StackItem::Integer(data) => data.clone().to_string(),
            StackItem::Cell(data) => format!("C{{{:X}}}", data.repr_hash()),
            StackItem::Continuation(_data) => {
                format!("Cont{{{}}}", "vmc_std")
            }
            StackItem::Builder(data) => {
                let bits = data.length_in_bits();
                let mut bytes = vec![data.references_used() as u8];
                let mut l = 2 * (bits / 8) as u8;
                let tag = if bits & 7 != 0 {
                    l += 1;
                    0x80 >> (bits & 7)
                } else {
                    0
                };
                bytes.push(l);
                bytes.extend_from_slice(data.data());
                *bytes.last_mut().unwrap() |= tag; // safe because vector always not empty
                format!("BC{{{}}}", hex::encode(bytes))
            }
            StackItem::Slice(data) => {
                if data.is_none() {
                    return "CS{null}".to_string();
                }
                let d1 = |level_mask: u8, refs_count: u8, is_special: u8| {
                    refs_count + 8 * is_special + 32 * level_mask
                };
                let d2 = |bits: u32| {
                    let res = ((bits / 8) * 2) as u8;
                    if bits & 7 != 0 {
                        res + 1
                    } else {
                        res
                    }
                };
                let start = data.pos();
                let end = start + data.remaining_bits();
                let refs = data.get_references();
                let cell = match data.cell() {
                    Ok(cell) => cell,
                    Err(err) => return err.to_string(),
                };
                let data = match SliceData::load_cell_ref(&cell) {
                    Ok(data) => data,
                    Err(err) => return err.to_string(),
                };
                let mut bytes = vec![];
                let is_special = cell.cell_type() != CellType::Ordinary;
                bytes.push(d1(
                    cell.level_mask().mask(),
                    cell.references_count() as u8,
                    is_special as u8,
                ));
                bytes.push(d2(data.remaining_bits() as u32));
                bytes.extend_from_slice(data.storage());
                if bytes.last() == Some(&0x80) {
                    bytes.pop();
                }
                format!(
                    "CS{{Cell{{{}}} bits: {}..{}; refs: {}..{}}}",
                    hex::encode(bytes),
                    start,
                    end,
                    refs.start,
                    refs.end
                )
            }
            StackItem::Tuple(data) => {
                if data.is_empty() {
                    "[]".to_string()
                } else {
                    format!(
                        "[ {} ]",
                        data.iter().map(|v| v.dump_as_fift()).collect::<Vec<_>>().join(" ")
                    )
                }
            }
        }
    }
}

#[rustfmt::skip]
impl fmt::Display for StackItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            StackItem::None            => write!(f, "Null"),
            StackItem::Builder(x)      => write!(f, "Builder {}", Arc::as_ref(x)),
            StackItem::Cell(x)         => write!(f, "Cell x{:x} x{:x}", x.repr_hash(), x),
            StackItem::Continuation(x) => write!(f, "Continuation x{:x}", x.code().repr_hash()),
            StackItem::Integer(x)      => write!(f, "{}", Arc::as_ref(x)),
            StackItem::Slice(x)        => write!(f, "Slice x{:x}", x),
            StackItem::Tuple(x)        => {
                if f.alternate() {
                    write!(f, "Tuple ({})", x.len())
                } else {
                    write!(f, "Tuple ({})", x.len())?;
                    f.debug_list().entries(x.iter().map(|v| format!("{:#}", v))).finish()
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stack {
    pub storage: Vec<StackItem>,
}

impl Stack {
    pub const fn new() -> Self {
        Stack { storage: Vec::new() }
    }

    pub const fn with_storage(storage: Vec<StackItem>) -> Self {
        Stack { storage }
    }

    // Swaps blocks (0...j-1) and (j...j+i-1)
    // e.g. block_swap(i=2, j=4): (8 7 6 {5 4} {3 2 1 0} -> 8 7 6 {3 2 1 0} {5 4})
    pub fn block_swap(&mut self, i: usize, j: usize) -> Status {
        if self.depth() < j + i {
            fail!(ExceptionCode::StackUnderflow)
        } else {
            let mut block = self.drop_range(j..j + i)?;
            while let Some(x) = block.pop() {
                self.push(x);
            }
            Ok(())
        }
    }

    pub fn clear(&mut self) {
        self.storage.clear()
    }

    pub fn depth(&self) -> usize {
        self.storage.len()
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn drop_top(&mut self, n: usize) {
        let depth = self.depth();
        if depth < n {
            log::error!(
                 target: "tvm",
                 "Corrupted stack state. This method can only be called \
                  when stack state is well known."
            );
        } else {
            self.storage.truncate(depth - n);
        }
    }

    pub fn drop(&mut self, i: usize) -> Result<StackItem> {
        let depth = self.depth();
        if i >= depth {
            fail!(ExceptionCode::StackUnderflow)
        } else {
            Ok(self.storage.remove(depth - i - 1))
        }
    }

    pub fn pop(&mut self) -> Result<StackItem> {
        self.storage.pop().ok_or_else(|| error!(ExceptionCode::StackUnderflow))
    }

    pub fn drop_range(&mut self, range: Range<usize>) -> Result<Vec<StackItem>> {
        if range.is_empty() {
            return Ok(vec![]);
        }
        let depth = self.depth();
        if range.end > depth {
            fail!(
                ExceptionCode::StackUnderflow,
                "drop_range: {}..{}, depth: {}",
                range.start,
                range.end,
                depth
            )
        } else {
            Ok(self.storage.drain(depth - range.end..depth - range.start).rev().collect())
        }
    }

    pub fn drop_range_straight(&mut self, range: Range<usize>) -> Result<Vec<StackItem>> {
        if range.is_empty() {
            return Ok(vec![]);
        }
        let depth = self.depth();
        match range.end.cmp(&depth) {
            Ordering::Greater => {
                fail!(
                    ExceptionCode::StackUnderflow,
                    "drop_range: {}..{}, depth: {}",
                    range.start,
                    range.end,
                    depth
                )
            }
            Ordering::Equal => {
                let mut rem = Vec::from(&self.storage[depth - range.start..]);
                self.storage.truncate(depth - range.start);
                std::mem::swap(&mut rem, &mut self.storage);
                Ok(rem)
            }
            Ordering::Less => {
                Ok(self.storage.drain(depth - range.end..depth - range.start).collect())
            }
        }
    }

    pub fn append(&mut self, other: &mut Vec<StackItem>) {
        self.storage.append(other)
    }

    pub fn get(&self, i: usize) -> Result<&StackItem> {
        if self.depth() > i {
            Ok(&self.storage[self.depth() - i - 1])
        } else {
            fail!(
                ExceptionCode::StackUnderflow,
                "get: {i} from stack, but depth is {}",
                self.depth()
            )
        }
    }

    pub fn get_mut(&mut self, i: usize) -> &mut StackItem {
        let depth = self.depth();
        &mut self.storage[depth - i - 1]
    }

    pub fn insert(&mut self, i: usize, item: StackItem) -> &mut Stack {
        let depth = self.depth();
        self.storage.insert(depth - i, item);
        self
    }
    /// pushes a new var to stack
    pub fn push(&mut self, item: StackItem) -> &mut Stack {
        self.storage.push(item);
        self
    }
    pub fn push_bool(&mut self, boolean: bool) -> &mut Stack {
        self.push(StackItem::boolean(boolean));
        self
    }
    /// pushes a builder as new var to stack
    pub fn push_builder(&mut self, builder: BuilderData) -> &mut Stack {
        self.push(StackItem::builder(builder));
        self
    }
    /// pushes a cell as new var to stack
    pub fn push_cell(&mut self, cell: Cell) -> &mut Stack {
        self.push(StackItem::cell(cell));
        self
    }
    /// pushes a continuation as new var to stack
    pub fn push_cont(&mut self, item: ContinuationData) -> &mut Stack {
        self.push(StackItem::continuation(item));
        self
    }
    /// pushes an integer as new var to stack
    pub fn push_int(&mut self, item: impl Into<IntegerData>) -> &mut Stack {
        self.push(StackItem::int(item));
        self
    }
    /// pushes an integer NaN as new var to stack
    pub fn push_nan(&mut self) -> &mut Stack {
        self.push(StackItem::int(IntegerData::nan()));
        self
    }
    /// pushes a none
    pub fn push_null(&mut self) -> &mut Stack {
        self.push(StackItem::None);
        self
    }
    /// pushes a slice data
    pub fn push_slice(&mut self, item: SliceData) -> &mut Stack {
        self.push(StackItem::slice(item));
        self
    }
    /// pushes a vector as tuple
    pub fn push_tuple(&mut self, items: Vec<StackItem>) -> &mut Stack {
        self.push(StackItem::tuple(items));
        self
    }

    // Reverses order of (j...j+i-1)
    pub fn reverse_range(&mut self, range: Range<usize>) -> Status {
        let depth = self.depth();
        if range.end > depth {
            fail!(ExceptionCode::StackUnderflow)
        } else {
            let length = range.end - range.start;
            for i in 0..length / 2 {
                self.storage.swap(depth - range.start - i - 1, depth - range.end + i);
            }
            Ok(())
        }
    }

    /// pushes a copy of the stack var to stack
    pub fn push_copy(&mut self, index: usize) -> Status {
        let depth = self.depth();
        if index >= depth {
            fail!(ExceptionCode::StackUnderflow)
        } else {
            let item = self.storage[depth - 1 - index].clone();
            self.push(item);
            Ok(())
        }
    }

    /// swaps two values inside the stack
    pub fn swap(&mut self, i: usize, j: usize) -> Status {
        let depth = self.depth();
        if (i >= depth) || (j >= depth) {
            fail!(ExceptionCode::StackUnderflow)
        } else {
            self.storage.swap(depth - i - 1, depth - j - 1);
            Ok(())
        }
    }

    fn eq_builder(x: &BuilderData, y: &StackItem) -> bool {
        match y {
            StackItem::Builder(y) => x.eq(y),
            _ => false,
        }
    }

    fn eq_cell(x: &Cell, y: &StackItem) -> bool {
        match y {
            StackItem::Cell(y) => x.eq(y),
            _ => false,
        }
    }

    fn eq_continuation(x: &ContinuationData, y: &StackItem) -> bool {
        match y {
            StackItem::Continuation(y) => x.eq(y),
            _ => false,
        }
    }

    fn eq_integer(x: &IntegerData, y: &StackItem) -> bool {
        match y {
            StackItem::Integer(y) => x.eq(y),
            _ => false,
        }
    }

    fn eq_slice(x: &SliceData, y: &StackItem) -> bool {
        match y {
            StackItem::Slice(y) => x.eq(y),
            _ => false,
        }
    }

    fn eq_tuple(x: &[StackItem], y: &StackItem) -> bool {
        match y {
            StackItem::Tuple(y) => {
                let len = x.len();
                if len != y.len() {
                    return false;
                }
                for i in 0..len {
                    if !Stack::eq_item(&x[i], &y[i]) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn eq_item(x: &StackItem, y: &StackItem) -> bool {
        match x {
            StackItem::Builder(x) => Stack::eq_builder(x, y),
            StackItem::Cell(x) => Stack::eq_cell(x, y),
            StackItem::Continuation(x) => Stack::eq_continuation(x, y),
            StackItem::Integer(x) => Stack::eq_integer(x, y),
            StackItem::Slice(x) => Stack::eq_slice(x, y),
            StackItem::Tuple(x) => Stack::eq_tuple(x, y),
            StackItem::None => y == &StackItem::None,
        }
    }

    pub fn iter(&self) -> Iter<'_, StackItem> {
        self.storage.iter()
    }
}

impl PartialEq for Stack {
    fn eq(&self, stack: &Stack) -> bool {
        if self.depth() != stack.depth() {
            return false;
        }
        for i in 0..self.depth() {
            if let (Ok(a), Ok(b)) = (self.get(i), stack.get(i)) {
                if !Stack::eq_item(a, b) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

impl fmt::Display for Stack {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            &self.storage.iter().fold(String::new(), |acc, item| format!("{}{}\n", acc, item)),
        )
    }
}

pub fn read_stack_item(slice: &mut SliceData) -> Result<StackItem> {
    let item = match slice.get_next_byte()? {
        0x00 => StackItem::None,
        0x01 => {
            let value = slice.get_next_i64()?;
            StackItem::int(value)
        }
        0x02 => match slice.get_next_byte()? {
            0x00 => {
                let value = IntegerData::from_bytes(slice.get_next_u256()?, 256, false, true)?;
                StackItem::integer(value)
            }
            0x01 => {
                let mut vec = vec![0xff];
                vec.extend_from_slice(&slice.get_next_u256()?);
                let value = IntegerData::from_bytes(&vec, 256 + 8, true, true)?;
                StackItem::integer(value)
            }
            0xFF => StackItem::nan(),
            tag => fail!("wrong tag for Integer StackItem: {tag}"),
        },
        0x03 => {
            let cell = slice.checked_drain_reference()?;
            StackItem::cell(cell)
        }
        0x04 => {
            let start = slice.get_next_int(10)? as usize;
            let end = slice.get_next_int(10)? as usize;
            let ref_start = slice.get_next_int(4)? as usize;
            let ref_end = slice.get_next_int(4)? as usize;
            let cell = slice.checked_drain_reference()?;
            let slice = SliceData::load_cell_with_window(cell, start..end, ref_start..ref_end)?;
            StackItem::slice(slice)
        }
        0x07 => {
            let length = slice.get_next_u16()? as usize;
            if length == 0 {
                return Ok(StackItem::tuple(Vec::new()));
            }
            let mut tuple = Vec::with_capacity(length);
            for _ in 1..length {
                let cell = slice.checked_drain_reference()?;
                let item = {
                    let cell = slice.checked_drain_reference()?;
                    read_stack_item(&mut SliceData::load_cell(cell)?)?
                };
                tuple.push(item);
                if !slice.is_empty_cell() {
                    fail!(ExceptionCode::CellUnderflow, "garbage in tuple: {slice}");
                }
                *slice = SliceData::load_cell(cell)?;
            }
            let item = read_stack_item(slice)?;
            tuple.push(item);
            tuple.reverse();
            StackItem::tuple(tuple)
        }
        tag => fail!("wrong tag for StackItem: {tag}"),
    };
    Ok(item)
}

#[cfg(test)]
#[path = "../tests/test_stack.rs"]
mod tests;
