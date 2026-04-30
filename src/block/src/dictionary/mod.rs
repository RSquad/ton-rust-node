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
    cell::{BuilderData, Cell, IBitstring, SliceData},
    error::{ExceptionCode, Result},
    fail, GasConsumer, Mask,
};
use std::marker::PhantomData;

mod hashmap;
pub use hashmap::HashmapE;
mod pfxhashmap;
pub use pfxhashmap::PfxHashmapE;

pub mod hashmapaug;
pub use hashmapaug::*;

pub type Leaf = Result<Option<SliceData>>;

pub const ADD: u8 = 0x01;
pub const REPLACE: u8 = 0x02;

fn hm_label_same(key: &SliceData, same: bool, max_len: usize) -> Result<BuilderData> {
    let len = key.remaining_bits();
    debug_assert!(len <= max_len && max_len <= 1023);
    let k = 16 - (max_len as u16).leading_zeros() as usize;
    // 0 <= k <= 10
    let mut b = BuilderData::new();
    if len > 1 && k < 2 * len - 1 {
        // hml_same
        b.append_bits(3, 2)?;
        b.append_bits(same as usize, 1)?;
        b.append_bits(len, k)?;
    } else if k < len {
        // hml_long
        // (len <= 1 || len <= (k + 1) / 2) && (k < len)
        b.append_bits(2, 2)?;
        b.append_bits(len, k)?;
        b.append_bits(if same { usize::MAX } else { 0 }, len)?;
    } else {
        // hml_short
        // (len <= 1 || len <= (k + 1) / 2) && (k >= len)
        b.append_bit_zero()?;
        b.append_bits(usize::MAX - 1, len + 1)?;
        b.append_bits(if same { usize::MAX } else { 0 }, len)?;
    }
    Ok(b)
}

pub fn hm_label(key: &SliceData, max_len: usize) -> Result<BuilderData> {
    let len = key.remaining_bits();
    debug_assert!(len <= max_len && max_len <= 1023);
    if len > 0 {
        let bit = key.get_bit(0)?;
        if key.are_bits_same(bit, 0, key.remaining_bits()) {
            return hm_label_same(key, bit, max_len);
        }
    }
    let k = 16 - (max_len as u16).leading_zeros() as usize;
    // 0 <= k <= 10
    let mut b = BuilderData::new();
    if k < len {
        // hml_long
        b.append_bits(2, 2)?;
        b.append_bits(len, k)?;
    } else {
        // hml_short
        // len <= k
        b.append_bit_zero()?;
        b.append_bits(usize::MAX - 1, len + 1)?;
    }
    b.append_bytestring(key)?;
    Ok(b)
}

// reading hmLabel from SliceData
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelReader {
    cursor: SliceData,
    already_read: bool,
}

impl LabelReader {
    fn get_label_short(cursor: &mut SliceData, max: &mut usize) -> Result<SliceData> {
        let mut len = 0;
        while cursor.get_next_bit()? {
            len += 1;
        }
        *max = max.checked_sub(len).ok_or(ExceptionCode::CellUnderflow)?;
        Ok(cursor.shrink_data(len..))
    }
    fn get_label_short_length(cursor: &mut SliceData, max: usize) -> Result<usize> {
        let mut len = 0;
        while cursor.get_next_bit()? {
            len += 1;
        }
        if len <= max {
            Ok(len)
        } else {
            fail!(ExceptionCode::CellUnderflow)
        }
    }
    fn get_label_long(cursor: &mut SliceData, max: &mut usize) -> Result<SliceData> {
        let len = cursor.get_next_size(*max)? as usize;
        *max = max.checked_sub(len).ok_or(ExceptionCode::CellUnderflow)?;
        Ok(cursor.shrink_data(len..))
    }
    fn get_label_long_length(cursor: &mut SliceData, max: usize) -> Result<usize> {
        let len = cursor.get_next_size(max)? as usize;
        if len <= max {
            Ok(len)
        } else {
            fail!(ExceptionCode::CellUnderflow)
        }
    }
    fn get_label_same(
        cursor: &mut SliceData,
        max: &mut usize,
        mut key: BuilderData,
    ) -> Result<BuilderData> {
        let value = if cursor.get_next_bit()? { 0xFF } else { 0 };
        let len = cursor.get_next_size(*max)? as usize;
        key.append_raw(&vec![value; len / 8 + 1], len)?;
        *max = max.checked_sub(len).ok_or(ExceptionCode::CellUnderflow)?;
        Ok(key)
    }
    fn get_label_same_length(cursor: &mut SliceData, max: usize) -> Result<usize> {
        cursor.get_next_bit()?;
        let len = cursor.get_next_size(max)? as usize;
        if len <= max {
            Ok(len)
        } else {
            fail!(ExceptionCode::CellUnderflow)
        }
    }
    pub fn new(cursor: SliceData) -> Self {
        Self { cursor, already_read: false }
    }
    pub fn with_cell(cursor: &Cell) -> Result<Self> {
        Ok(Self::new(SliceData::load_cell_ref(cursor)?))
    }
    pub fn next_reader<T: HashmapType + ?Sized>(
        &mut self,
        index: usize,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Self> {
        if !self.is_fork::<T>()? {
            fail!("this edge must contain fork")
        }
        Ok(Self::new(gas_consumer.load_cell(self.reference(index)?)?))
    }
    pub fn already_read(&self) -> bool {
        self.already_read
    }
    pub fn remainder(self) -> Result<SliceData> {
        if !self.already_read {
            fail!("label not yet read!")
        }
        Ok(self.cursor)
    }
    pub fn reference(&self, index: usize) -> Result<Cell> {
        if !self.already_read {
            fail!("label not yet read!")
        }
        self.cursor.reference(index)
    }
    pub fn get_label_raw(&mut self, max: &mut usize, key: BuilderData) -> Result<BuilderData> {
        if self.already_read {
            fail!("label already read!")
        }
        self.already_read = true;
        Self::read_label_raw(&mut self.cursor, max, key)
    }
    pub fn read_label_raw(
        cursor: &mut SliceData,
        max: &mut usize,
        mut key: BuilderData,
    ) -> Result<BuilderData> {
        if cursor.is_empty_bitstring() {
        } else if !cursor.get_next_bit()? {
            key.append_bytestring(&Self::get_label_short(cursor, max)?)?;
        } else if !cursor.get_next_bit()? {
            key.append_bytestring(&Self::get_label_long(cursor, max)?)?;
        } else {
            key = Self::get_label_same(cursor, max, key)?;
        }
        Ok(key)
    }
    pub fn read_label_length(cursor: &mut SliceData, max: usize) -> Result<usize> {
        if cursor.is_empty_bitstring() {
            Ok(0)
        } else if !cursor.get_next_bit()? {
            Self::get_label_short_length(cursor, max)
        } else if !cursor.get_next_bit()? {
            Self::get_label_long_length(cursor, max)
        } else {
            Self::get_label_same_length(cursor, max)
        }
    }
    pub fn get_label(&mut self, max: usize) -> Result<SliceData> {
        if self.already_read {
            fail!("label already read!")
        }
        self.already_read = true;
        Self::read_label(&mut self.cursor, max)
    }
    pub fn read_label(cursor: &mut SliceData, mut max: usize) -> Result<SliceData> {
        // note: in case of max is 0 it is normal to read bits from the slice
        // but if you mistakely pass 0 to this function it causes undefined behavoiur
        if cursor.is_empty_bitstring() {
            Ok(SliceData::default())
        } else if !cursor.get_next_bit()? {
            Self::get_label_short(cursor, &mut max)
        } else if !cursor.get_next_bit()? {
            Self::get_label_long(cursor, &mut max)
        } else {
            SliceData::load_bitstring(Self::get_label_same(
                cursor,
                &mut max,
                BuilderData::default(),
            )?)
        }
    }
    pub fn skip_label(&mut self, max: &mut usize) -> Result<()> {
        if self.already_read {
            fail!("label already read!")
        }
        self.already_read = true;
        let mut len = 0;
        // note: in case of max is 0 it is normal to read bits from the slice
        // but if you mistakely pass 0 to this function it causes undefined behavoiur
        if self.cursor.is_empty_bitstring() {
        } else if !self.cursor.get_next_bit()? {
            while self.cursor.get_next_bit()? {
                len += 1;
            }
            self.cursor.move_by(len)?;
        } else if !self.cursor.get_next_bit()? {
            len = self.cursor.get_next_size(*max)? as usize;
            self.cursor.move_by(len)?;
        } else {
            self.cursor.get_next_bit()?;
            len = self.cursor.get_next_size(*max)? as usize;
        }
        *max = max.checked_sub(len).ok_or(ExceptionCode::CellUnderflow)?;
        Ok(())
    }
    pub fn is_fork<T: HashmapType + ?Sized>(&mut self) -> Result<bool> {
        T::is_fork(&mut self.cursor)
    }
    pub fn is_leaf<T: HashmapType + ?Sized>(&mut self) -> Result<bool> {
        Ok(T::is_leaf(&mut self.cursor))
    }
}

// methods working with root
impl SliceData {
    pub fn get_dictionary_opt(&mut self) -> Option<SliceData> {
        if !self.get_bit_opt(0)? {
            Some(self.shrink_data(1..))
        } else if self.remaining_references() == 0 {
            None
        } else {
            Some(self.shrink(1.., 1..))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn find_leaf<T: HashmapType + ?Sized>(
    mut data: Cell,
    path: &mut BuilderData,
    mut bit_len: usize,
    mut key: SliceData,
    next_index: usize,
    eq: bool,
    signed_int: bool,
    gas_consumer: &mut dyn GasConsumer,
) -> Result<Option<SliceData>> {
    let mut cursor = gas_consumer.load_cell(data.clone())?;
    let label = LabelReader::read_label(&mut cursor, bit_len)?;
    match SliceData::common_prefix(&key, &label) {
        (_, None, Some(_)) => fail!(ExceptionCode::DictionaryError),
        (prefix_opt, Some(remainder), Some(_)) => {
            // hm_edge is sliced
            let key_bit = remainder.get_bit(0)? as usize;
            let next = match signed_int && path.is_empty() && prefix_opt.is_none() {
                false => next_index,
                true => 1 - next_index,
            };
            if key_bit != next {
                Ok(None)
            } else {
                get_min_max::<T>(data, path, bit_len, next_index, next, gas_consumer)
            }
        }
        (_, None, None) => {
            if eq {
                // same leaf found
                path.append_bytestring(&label)?;
                Ok(Some(cursor))
            } else {
                Ok(None)
            }
        }
        (prefix_opt, Some(remainder), None) => {
            // label fully in key
            if !T::is_fork(&mut cursor)? {
                fail!(ExceptionCode::DictionaryError)
            }
            let next = match signed_int && path.is_empty() && prefix_opt.is_none() {
                false => next_index,
                true => 1 - next_index,
            };
            path.append_bytestring(&label)?;
            key = remainder;
            let key_bit = key.get_next_bit_int()?;
            bit_len = bit_len
                .checked_sub(label.remaining_bits() + 1)
                .ok_or(ExceptionCode::CellUnderflow)?;
            let length_in_bits = path.length_in_bits();
            path.append_bit_bool(key_bit == 1)?;
            let res = find_leaf::<T>(
                cursor.reference(key_bit)?,
                path,
                bit_len,
                key,
                next_index,
                eq,
                false,
                gas_consumer,
            )?;
            if res.is_some() || key_bit != next {
                return Ok(res);
            }
            path.trunc(length_in_bits)?;
            path.append_bit_bool(key_bit == 0)?;
            data = cursor.reference(1 - key_bit)?;
            get_min_max::<T>(data, path, bit_len, next_index, next_index, gas_consumer)
        }
    }
}

/// search min or max element from current subtree. Append path and returns element if found
pub fn get_min_max<T: HashmapType + ?Sized>(
    mut data: Cell,
    path: &mut BuilderData,
    mut bit_len: usize,
    next_index: usize, // 0 - for min, 1 - for max
    mut index: usize,
    gas_consumer: &mut dyn GasConsumer,
) -> Result<Option<SliceData>> {
    loop {
        let mut cursor = gas_consumer.load_cell(data)?;
        let label = LabelReader::read_label(&mut cursor, bit_len)?;
        let label_length = label.remaining_bits();
        if path.is_empty() && !label.is_empty_bitstring() {
            index = next_index;
        }
        path.append_bytestring(&label)?;
        if T::is_fork(&mut cursor)? && bit_len > label_length {
            bit_len -= label_length + 1;
            path.append_bit_bool(index == 1)?;
            data = cursor.reference(index)?;
        } else if bit_len == label_length {
            return Ok(Some(cursor));
        } else {
            fail!(ExceptionCode::DictionaryError)
        }
        index = next_index;
    }
}

// difference for different hashmap types
pub trait HashmapType {
    fn write_hashmap_data(&self, cell: &mut BuilderData) -> Result<()> {
        if let Some(root) = self.data() {
            cell.append_bit_one()?;
            cell.checked_append_reference(root.clone())?;
        } else {
            cell.append_bit_zero()?;
        }
        Ok(())
    }
    fn read_hashmap_data(&mut self, slice: &mut SliceData) -> Result<()> {
        *self.data_mut() = slice.get_next_dictionary()?;
        Ok(())
    }
    fn is_empty(&self) -> bool {
        self.data().is_none()
    }

    fn check_key(bit_len: usize, key: &SliceData) -> bool;
    fn check_key_fail(bit_len: usize, key: &SliceData) -> Result<()> {
        match !key.is_empty_bitstring() && Self::check_key(bit_len, key) {
            true => Ok(()),
            false => fail!("Bad key {} for dict", key),
        }
    }
    fn make_cell_with_label(key: &SliceData, max: usize) -> Result<BuilderData> {
        hm_label(key, max)
    }
    fn make_cell_with_label_and_data(
        key: &SliceData,
        max: usize,
        _is_leaf: bool,
        data: &SliceData,
    ) -> Result<BuilderData> {
        let mut builder = Self::make_cell_with_label(key, max)?;
        builder.checked_append_references_and_data(data)?;
        Ok(builder)
    }
    fn make_cell_with_label_and_builder(
        key: &SliceData,
        max: usize,
        _is_leaf: bool,
        data: &BuilderData,
    ) -> Result<BuilderData> {
        let mut builder = Self::make_cell_with_label(key, max)?;
        builder.append_builder(data)?;
        Ok(builder)
    }
    fn make_cell_with_remainder(
        key: &SliceData,
        max: usize,
        remainder: &SliceData,
    ) -> Result<BuilderData> {
        let mut builder = Self::make_cell_with_label(key, max)?;
        builder.checked_append_references_and_data(remainder)?;
        Ok(builder)
    }
    fn make_edge(
        key: &SliceData,
        bit_len: usize,
        is_left: bool,
        mut next: SliceData,
    ) -> Result<BuilderData> {
        let mut next_bit_len =
            bit_len.checked_sub(key.remaining_bits() + 1).ok_or(ExceptionCode::CellUnderflow)?;
        let mut key = key.as_builder()?;
        key.append_bit_bool(!is_left)?;
        let label = LabelReader::read_label_raw(&mut next, &mut next_bit_len, key)?;
        let is_leaf = Self::is_leaf(&mut next);
        Self::make_cell_with_label_and_data(
            &SliceData::load_bitstring(label)?,
            bit_len,
            is_leaf,
            &next,
        )
    }
    fn make_fork(
        key: &SliceData,
        bit_len: usize,
        mut left: Cell,
        mut right: Cell,
        swap: bool,
    ) -> Result<(BuilderData, BuilderData)> {
        let mut builder = hm_label(key, bit_len)?;
        let mut remainder = BuilderData::new();
        if swap {
            std::mem::swap(&mut left, &mut right);
        }
        remainder.checked_append_reference(left)?;
        remainder.checked_append_reference(right)?;
        builder.append_builder(&remainder)?;
        Ok((builder, remainder))
    }
    fn make_leaf(key: &SliceData, bit_len: usize, value: &SliceData) -> Result<BuilderData> {
        let mut builder = hm_label(key, bit_len)?;
        builder.checked_append_references_and_data(value)?;
        Ok(builder)
    }
    /// Assemble a subtree from a label and two optional children.
    fn make_subtree_cell(
        label: &SliceData,
        n: usize,
        left: Option<Cell>,
        right: Option<Cell>,
    ) -> Result<Option<Cell>> {
        match (left, right) {
            (None, None) => Ok(None),
            (Some(left), Some(right)) => {
                let (builder, _) = Self::make_fork(label, n, left, right, false)?;
                Ok(Some(builder.into_cell()?))
            }
            (Some(child), None) => {
                let next = SliceData::load_cell(child)?;
                Ok(Some(Self::make_edge(label, n, true, next)?.into_cell()?))
            }
            (None, Some(child)) => {
                let next = SliceData::load_cell(child)?;
                Ok(Some(Self::make_edge(label, n, false, next)?.into_cell()?))
            }
        }
    }
    fn is_fork(slice: &mut SliceData) -> Result<bool>;
    fn is_leaf(slice: &mut SliceData) -> bool;
    fn inner(self) -> Option<Cell>;
    fn data(&self) -> Option<&Cell>;
    fn data_mut(&mut self) -> &mut Option<Cell>;
    fn bit_len(&self) -> usize;
    fn iter(&self) -> HashmapIterator<Self> {
        HashmapIterator::from_hashmap(self)
    }
    fn hashmap_get(&self, mut key: SliceData, gas_consumer: &mut dyn GasConsumer) -> Leaf {
        let mut bit_len = self.bit_len();
        Self::check_key_fail(bit_len, &key)?;
        let mut cursor = match self.data().cloned() {
            Some(root) => gas_consumer.load_cell(root)?,
            _ => return Ok(None),
        };
        let mut label = LabelReader::read_label(&mut cursor, bit_len)?;
        while key.erase_prefix(&label) && !key.is_empty_bitstring() {
            if !Self::is_fork(&mut cursor)? {
                return Ok(None);
            }
            let next_index = key.get_next_bit_int()?;
            cursor = gas_consumer.load_cell(cursor.reference(next_index)?)?;
            bit_len = bit_len
                .checked_sub(label.remaining_bits() + 1)
                .ok_or(ExceptionCode::CellUnderflow)?;
            label = LabelReader::read_label(&mut cursor, bit_len)?;
        }
        if key.is_empty_bitstring() && Self::is_leaf(&mut cursor) {
            Ok(Some(cursor))
        } else {
            Ok(None)
        }
    }

    fn hashmap_set_with_mode(
        &mut self,
        key: SliceData,
        leaf: &BuilderData,
        gas_consumer: &mut dyn GasConsumer,
        mode: u8,
    ) -> Leaf {
        let bit_len = self.bit_len();
        Self::check_key_fail(bit_len, &key)?;
        if let Some(root) = self.data() {
            let mut root = root.clone();
            let mut ins = HashmapInserter::<Self>::new(leaf, gas_consumer, mode);
            let result = ins.put_to_node_with_mode(&mut root, bit_len, key);
            *self.data_mut() = Some(root);
            result
        } else if mode.bit(ADD) {
            let cell = gas_consumer.finalize_cell(Self::make_cell_with_label_and_builder(
                &key, bit_len, true, leaf,
            )?)?;
            *self.data_mut() = Some(cell);
            Ok(None)
        } else {
            Ok(None)
        }
    }

    fn hashmap_setref_with_mode(
        &mut self,
        key: SliceData,
        value: Cell,
        gas_consumer: &mut dyn GasConsumer,
        mode: u8,
    ) -> Leaf {
        let mut builder = BuilderData::default();
        builder.checked_append_reference(value)?;
        self.hashmap_set_with_mode(key, &builder, gas_consumer, mode)
    }

    fn hashmap_multiset<'a, I>(&mut self, iter: I) -> Result<()>
    where
        I: IntoIterator<Item = (FixedBitsKey<'a>, Option<SliceData>)>,
    {
        let bit_len = self.bit_len();
        let mut values: Vec<_> = iter.into_iter().collect();
        if values.is_empty() {
            return Ok(());
        }
        for (key, _) in &values {
            if key.bit_len() != bit_len {
                fail!("multiset key bit_len {} != expected {bit_len}: {key:?}", key.bit_len());
            }
        }
        values.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        *self.data_mut() =
            dict_multiset_impl::<Self>(self.data().cloned(), &values, bit_len, bit_len, 0)?;
        Ok(())
    }

    /// iterate all elements with callback function
    fn iterate_slices<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(SliceData, SliceData) -> Result<bool>,
    {
        if let Some(root) = self.data() {
            iterate_internal::<Self, _>(
                LabelReader::with_cell(root)?,
                BuilderData::default(),
                self.bit_len(),
                &mut |k, v| p(SliceData::load_bitstring(k)?, v),
            )
        } else {
            Ok(true)
        }
    }

    /// returns count of objects in tree - don't use it - try is_empty()
    fn len(&self) -> Result<usize> {
        match self.data() {
            Some(root) => {
                let mut len = 0;
                let cursor = LabelReader::with_cell(root)?;
                count_internal::<Self>(cursor, self.bit_len(), &mut len, usize::MAX)?;
                Ok(len)
            }
            None => Ok(0),
        }
    }
    /// counts elements to max counter - can be used as validate
    fn count(&self, max: usize) -> Result<usize> {
        match self.data() {
            Some(root) => {
                let mut len = 0;
                let cursor = LabelReader::with_cell(root)?;
                count_internal::<Self>(cursor, self.bit_len(), &mut len, max)?;
                Ok(len)
            }
            None => Ok(0),
        }
    }
    /// determines if hashmap contains one element
    fn is_single(&self) -> Result<Option<(BuilderData, SliceData)>> {
        if let Some(root) = self.data() {
            let mut bit_len = self.bit_len();
            let mut cursor = LabelReader::with_cell(root)?;
            let key = cursor.get_label_raw(&mut bit_len, BuilderData::default())?;
            if bit_len == 0 {
                return Ok(Some((key, cursor.remainder()?)));
            }
        }
        Ok(None)
    }

    // split
    fn hashmap_split(&self, key: &SliceData) -> Result<(Option<Cell>, Option<Cell>)> {
        let mut bit_len = self.bit_len();
        let data = match self.data() {
            Some(data) => data,
            _ => return Ok((None, None)),
        };
        let mut cursor = SliceData::load_cell_ref(data)?;
        let label = LabelReader::read_label(&mut cursor, bit_len)?;
        let (left, right) = match SliceData::common_prefix(&label, key) {
            // normal case label == key
            (_prefix, None, None) => {
                bit_len = bit_len
                    .checked_sub(label.remaining_bits() + 1)
                    .ok_or(ExceptionCode::CellUnderflow)?;
                (cursor.reference(0)?, cursor.reference(1)?)
            }
            // normal case with empty branch
            (_prefix, Some(mut label_remainder), None) => match label_remainder.get_next_bit()? {
                false => return Ok((Some(data.clone()), None)),
                true => return Ok((None, Some(data.clone()))),
            },
            // wrong hashmap tree
            _ => fail!("split fail: root label: x{:x} and key: x{:x}", label, key),
        };
        cursor = SliceData::load_cell(left)?;
        let label = LabelReader::read_label(&mut cursor, bit_len)?;
        let mut builder = key.as_builder()?;
        builder.append_bit_zero()?;
        builder.append_bytestring(&label)?;
        let left = Self::make_cell_with_label_and_data(
            &SliceData::load_bitstring(builder)?,
            self.bit_len(),
            false,
            &cursor,
        )?;
        cursor = SliceData::load_cell(right)?;
        let label = LabelReader::read_label(&mut cursor, bit_len)?;
        let mut builder = key.as_builder()?;
        builder.append_bit_one()?;
        builder.append_bytestring(&label)?;
        let right = Self::make_cell_with_label_and_data(
            &SliceData::load_bitstring(builder)?,
            self.bit_len(),
            false,
            &cursor,
        )?;
        Ok((Some(left.into_cell()?), Some(right.into_cell()?)))
    }

    // merge
    fn hashmap_merge(&mut self, other: &Self, key: &SliceData) -> Result<()> {
        let bit_len = self.bit_len();
        if bit_len != other.bit_len() || key.remaining_bits() > bit_len {
            return Ok(()); // fail!("data in hashmaps do not correspond each other or key too long")
        }
        let cell1 = match self.data() {
            Some(data) => data.clone(),
            None => {
                *self.data_mut() = other.data().cloned();
                return Ok(());
            }
        };
        let cell2 = match other.data() {
            Some(data) => data.clone(),
            None => return Ok(()),
        };
        *self.data_mut() = Some(merge_nodes::<Self>(cell1, cell2, bit_len, key)?);
        Ok(())
    }

    fn scan_diff<F>(&self, other: &Self, mut func: F) -> Result<bool>
    where
        F: FnMut(SliceData, Option<SliceData>, Option<SliceData>) -> Result<bool>,
    {
        let bit_len = self.bit_len();
        if bit_len != other.bit_len() {
            fail!("Different bitlen")
        }
        dict_scan_diff::<Self, _>(
            self.data().cloned(),
            other.data().cloned(),
            BuilderData::default(),
            bit_len,
            bit_len,
            &mut func,
        )
    }

    /// combine all items from two hashamps
    /// will fail if trees have different items with same key
    fn combine_with(&mut self, other: &Self) -> Result<bool> {
        let bit_len = self.bit_len();
        if bit_len != other.bit_len() {
            fail!("Different bitlen")
        }
        match (self.data().cloned(), other.data().cloned()) {
            (Some(mut cell1), Some(cell2)) => {
                if dict_combine_with_cell::<Self>(&mut cell1, cell2, bit_len)? {
                    *self.data_mut() = Some(cell1);
                    return Ok(true);
                }
                Ok(false)
            }
            (None, Some(cell2)) => {
                *self.data_mut() = Some(cell2);
                Ok(true)
            }
            (_, None) => Ok(false),
        }
    }
}

fn merge_nodes<T: HashmapType + ?Sized>(
    cell1: Cell,
    cell2: Cell,
    mut bit_len: usize,
    key: &SliceData,
) -> Result<Cell> {
    let mut stack = vec![];
    let mut slice1 = SliceData::load_cell_ref(&cell1)?;
    let mut slice2 = SliceData::load_cell_ref(&cell2)?;
    let mut label1 = LabelReader::read_label(&mut slice1, bit_len)?;
    let mut label2 = LabelReader::read_label(&mut slice2, bit_len)?;
    // we will cut both trees on same segments then construct full tree
    let mut data = loop {
        // find next common segment
        let (prefix, key1, key2) = SliceData::common_prefix(&label1, &label2);
        // segment could be empty so next branch starts after previous in next bit
        let prefix = prefix.unwrap_or_default();
        // calculate new_bit_len for next level
        // note: fork gets one bit
        let new_bit_len =
            bit_len.checked_sub(prefix.remaining_bits() + 1).ok_or(ExceptionCode::CellUnderflow)?;
        // dbg!("--------", &label1, &label2, bit_len, &prefix, &key1, &key2);

        match (key1, key2) {
            // difference found - proceed to merge
            (Some(mut key1), Some(mut key2)) => {
                if let (_, _, Some(_)) = SliceData::common_prefix(&prefix, key) {
                    fail!("common prefix of merging hashmaps is too short")
                }
                let bit1 = key1.get_next_bit()?;
                let bit2 = key2.get_next_bit()?;
                if bit1 && !bit2 {
                    std::mem::swap(&mut key1, &mut key2);
                    std::mem::swap(&mut slice1, &mut slice2);
                }
                let is_leaf1 = T::is_leaf(&mut slice1);
                let is_leaf2 = T::is_leaf(&mut slice2);

                let left = T::make_cell_with_label_and_data(&key1, new_bit_len, is_leaf1, &slice1)?;
                let right =
                    T::make_cell_with_label_and_data(&key2, new_bit_len, is_leaf2, &slice2)?;
                let (root, _) =
                    T::make_fork(&prefix, bit_len, left.into_cell()?, right.into_cell()?, false)?;
                break root.into_cell()?;
            }
            (None, Some(mut remainder)) => {
                let next_index = remainder.get_next_bit_int()?;
                let cell = slice1.reference(next_index)?;
                // dbg!(next_index, &slice1, &slice2);
                stack.push((prefix, bit_len, slice1, next_index));
                slice1 = SliceData::load_cell(cell)?;
                label1 = LabelReader::read_label(&mut slice1, new_bit_len)?;
                label2 = remainder;
            }
            (Some(mut remainder), None) => {
                let next_index = remainder.get_next_bit_int()?;
                let cell = slice2.reference(next_index)?;
                // dbg!(next_index, &slice1, &slice2);
                stack.push((prefix, bit_len, slice2, next_index));
                slice2 = SliceData::load_cell(cell)?;
                label2 = LabelReader::read_label(&mut slice2, new_bit_len)?;
                label1 = remainder;
            }
            (None, None) => {
                // we have two forks with the same prefix
                // we get left edges of both forks and merge them
                let cell1 = slice1.reference(0)?;
                let cell2 = slice2.reference(0)?;
                let left = merge_nodes::<T>(cell1, cell2, new_bit_len, key)?;

                // we get right edges of both forks and merge them
                let cell1 = slice1.reference(1)?;
                let cell2 = slice2.reference(1)?;
                let right = merge_nodes::<T>(cell1, cell2, new_bit_len, key)?;

                let (root, _) = T::make_fork(&prefix, bit_len, left, right, false)?;
                break root.into_cell()?;
            }
        }
        bit_len = new_bit_len;
    };

    // now we construct full tree as is with subtrees
    while let Some((prefix, bit_len, slice, next_index)) = stack.pop() {
        // subtree we get as is
        let cell = slice.reference(1 - next_index)?;
        // construct new fork
        let (root, _) = T::make_fork(&prefix, bit_len, data, cell, next_index == 1)?;
        data = root.into_cell()?;
    }
    Ok(data)
}

fn dict_combine_with_cell<T: HashmapType + ?Sized>(
    cell1: &mut Cell,
    cell2: Cell,
    bit_len: usize,
) -> Result<bool> {
    let mut cursor2 = SliceData::load_cell(cell2)?;
    let label2 = LabelReader::read_label(&mut cursor2, bit_len)?;
    let bit_len2 =
        bit_len.checked_sub(label2.remaining_bits()).ok_or(ExceptionCode::CellUnderflow)?;
    dict_combine_with::<T>(cell1, bit_len, cursor2, label2, bit_len2)
}

fn dict_combine_with<T: HashmapType + ?Sized>(
    cell1: &mut Cell,
    bit_len: usize,
    mut cursor2: SliceData,
    label2: SliceData,
    bit_len2: usize,
) -> Result<bool> {
    let mut cursor1 = SliceData::load_cell_ref(cell1)?;
    let label1 = LabelReader::read_label(&mut cursor1, bit_len)?;
    let bit_len1 =
        bit_len.checked_sub(label1.remaining_bits()).ok_or(ExceptionCode::CellUnderflow)?;
    match SliceData::common_prefix(&label1, &label2) {
        (_prefix_opt, None, None) => {
            if cursor1 == cursor2 {
                // same level
                return Ok(false);
            } else if bit_len1 == 0 {
                // do not allow to replace leafs
                fail!(ExceptionCode::DictionaryError)
            } else {
                // continue like with two separate trees
                let mut left1 = cursor1.checked_drain_reference()?;
                let left2 = cursor2.checked_drain_reference()?;
                let mut right1 = cursor1.checked_drain_reference()?;
                let right2 = cursor2.checked_drain_reference()?;
                let res1 = dict_combine_with_cell::<T>(&mut left1, left2, bit_len1 - 1)?;
                let res2 = dict_combine_with_cell::<T>(&mut right1, right2, bit_len1 - 1)?;
                if res1 || res2 {
                    let (builder, _) = T::make_fork(&label1, bit_len, left1, right1, false)?;
                    *cell1 = builder.into_cell()?;
                    return Ok(true);
                }
            }
        }
        (prefix_opt, Some(mut rem1), rem2_opt) => {
            // slice edge
            let next_index = rem1.get_next_bit_int()?;
            *cell1 = if let Some(mut rem2) = rem2_opt {
                // simple slice of both trees and make new fork
                rem2.get_next_bit_int()?; // == 1 - next_index
                let prefix = prefix_opt.unwrap_or_default(); //
                let bit_len1 = bit_len - prefix.remaining_bits() - 1;
                let left = T::make_cell_with_remainder(&rem1, bit_len1, &cursor1)?.into_cell()?;
                let right = T::make_cell_with_remainder(&rem2, bit_len1, &cursor2)?.into_cell()?;
                let (builder, _) = T::make_fork(&prefix, bit_len, left, right, next_index != 0)?;
                builder.into_cell()?
            } else if bit_len2 == 0 {
                // second should not stop here
                fail!(ExceptionCode::DictionaryError)
            } else {
                // slice edge of first and add items from first to second, then make new fork
                let mut next = cursor2.reference(next_index)?;
                let other = cursor2.reference(1 - next_index)?;
                dict_combine_with::<T>(&mut next, bit_len2 - 1, cursor1, rem1, bit_len1)?;
                let (builder, _) = T::make_fork(&label2, bit_len, next, other, next_index != 0)?;
                builder.into_cell()?
            };
            return Ok(true);
        }
        (_prefix_opt, None, Some(mut rem2)) => {
            if bit_len1 == 0 {
                // it should not be leaf
                fail!(ExceptionCode::DictionaryError)
            } else {
                // select branch and continue, then make new fork
                let next_index = rem2.get_next_bit_int()?;
                let mut next = cursor1.reference(next_index)?;
                let other = cursor1.reference(1 - next_index)?;
                if !dict_combine_with::<T>(&mut next, bit_len1 - 1, cursor2, rem2, bit_len2)? {
                    return Ok(false);
                }
                let (builder, _) = T::make_fork(&label1, bit_len, next, other, next_index != 0)?;
                *cell1 = builder.into_cell()?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn scan_diff_leaf_reched<T, F>(
    cursor_1: LabelReader,
    cursor_2: LabelReader,
    key1: BuilderData,
    key2: BuilderData,
    bit_len_1: usize,
    bit_len_2: usize,
    func: &mut F,
) -> Result<bool>
where
    T: HashmapType + ?Sized,
    F: FnMut(SliceData, Option<SliceData>, Option<SliceData>) -> Result<bool>,
{
    debug_assert!(bit_len_1 == 0 || bit_len_2 == 0, "should be called only if one leaf reached");
    if bit_len_1 == 0 && bit_len_2 == 0 {
        // 1 and 2 leaves reached
        if key1 == key2 {
            if cursor_1 != cursor_2 {
                return func(
                    SliceData::load_bitstring(key1)?,
                    Some(cursor_1.remainder()?),
                    Some(cursor_2.remainder()?),
                );
            }
        } else if !func(SliceData::load_bitstring(key1)?, Some(cursor_1.remainder()?), None)?
            || !func(SliceData::load_bitstring(key2)?, None, Some(cursor_2.remainder()?))?
        {
            return Ok(false);
        }
    } else if bit_len_1 == 0 {
        // leaf of 1 is reached
        let mut chk = false;
        let cursor_1 = cursor_1.remainder()?;
        if !iterate_internal::<T, _>(cursor_2, key2, bit_len_2, &mut |key, cursor| {
            if key1 != key {
                func(SliceData::load_bitstring(key)?, None, Some(cursor))
            } else {
                chk = true;
                match cursor == cursor_1 {
                    true => Ok(true),
                    false => {
                        func(SliceData::load_bitstring(key)?, Some(cursor_1.clone()), Some(cursor))
                    }
                }
            }
        })? || (!chk && !func(SliceData::load_bitstring(key1)?, Some(cursor_1), None)?)
        {
            return Ok(false);
        }
    } else {
        // leaf of 2 is reached
        debug_assert_eq!(bit_len_2, 0);
        let mut chk = false;
        let cursor_2 = cursor_2.remainder()?;
        if !iterate_internal::<T, _>(cursor_1, key1, bit_len_1, &mut |key, cursor| {
            if key2 != key {
                func(SliceData::load_bitstring(key)?, Some(cursor), None)
            } else {
                chk = true;
                match cursor == cursor_2 {
                    true => Ok(true),
                    false => {
                        func(SliceData::load_bitstring(key)?, Some(cursor), Some(cursor_2.clone()))
                    }
                }
            }
        })? || (!chk && !func(SliceData::load_bitstring(key2)?, None, Some(cursor_2))?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn dict_scan_diff<T, F>(
    cell_1: Option<Cell>,
    cell_2: Option<Cell>,
    key: BuilderData,
    mut bit_len_1: usize,
    mut bit_len_2: usize,
    func: &mut F,
) -> Result<bool>
where
    F: FnMut(SliceData, Option<SliceData>, Option<SliceData>) -> Result<bool>,
    T: HashmapType + ?Sized,
{
    let (mut cursor_1, mut cursor_2) = match (cell_1, cell_2) {
        (Some(cell_1), Some(cell_2)) => {
            if cell_1 == cell_2 {
                return Ok(true);
            } else {
                (
                    LabelReader::new(SliceData::load_cell(cell_1)?),
                    LabelReader::new(SliceData::load_cell(cell_2)?),
                )
            }
        }
        (Some(cell), None) => {
            return iterate_internal::<T, _>(
                // only 1 leaves
                LabelReader::with_cell(&cell)?,
                key,
                bit_len_1,
                &mut |key, cursor| func(SliceData::load_bitstring(key)?, Some(cursor), None),
            );
        }
        (None, Some(cell)) => {
            return iterate_internal::<T, _>(
                // only 2 leaves
                LabelReader::with_cell(&cell)?,
                key,
                bit_len_2,
                &mut |key, cursor| func(SliceData::load_bitstring(key)?, None, Some(cursor)),
            );
        }
        _ => return Ok(true),
    };
    let mut key1 = cursor_1.get_label_raw(&mut bit_len_1, key.clone())?;
    let mut key2 = cursor_2.get_label_raw(&mut bit_len_2, key)?;
    loop {
        if bit_len_1 == 0 || bit_len_2 == 0 {
            // 1 and/or 2 leaf reached
            return scan_diff_leaf_reched::<T, _>(
                cursor_1, cursor_2, key1, key2, bit_len_1, bit_len_2, func,
            );
        } else if key1 == key2 {
            // same level is reached
            if cursor_1 == cursor_2 {
                return Ok(true);
            } else if bit_len_1 == 0 {
                // same 1 and 2 leaves reached
                return func(
                    SliceData::load_bitstring(key1)?,
                    Some(cursor_1.remainder()?),
                    Some(cursor_2.remainder()?),
                );
            } else {
                // same branch reached - continue scan_diff as from start
                debug_assert_eq!(bit_len_1, bit_len_2);
                bit_len_1 -= 1;
                for i in 0..2 {
                    let mut key = key1.clone();
                    key.append_bit_bool(i != 0)?;
                    let child1 = Some(cursor_1.reference(i)?);
                    let child2 = Some(cursor_2.reference(i)?);
                    if !dict_scan_diff::<T, _>(child1, child2, key, bit_len_1, bit_len_1, func)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        match key1.compare_data(&key2)? {
            (Some(next_bit), None) => {
                // key1 includes key2
                bit_len_2 -= 1;
                let mut key = key2.clone();
                key.append_bit_bool(next_bit == 0)?;
                if !dict_scan_diff::<T, _>(
                    None,
                    Some(cursor_2.reference(1 - next_bit)?),
                    key,
                    0,
                    bit_len_2,
                    func,
                )? {
                    return Ok(false);
                }
                key2.append_bit_bool(next_bit == 1)?;
                cursor_2 = cursor_2.next_reader::<T>(next_bit, &mut 0)?;
                key2 = cursor_2.get_label_raw(&mut bit_len_2, key2)?;
            }
            (None, Some(next_bit)) => {
                // key2 includes key1
                bit_len_1 -= 1;
                let mut key = key1.clone();
                key.append_bit_bool(next_bit == 0)?;
                if !dict_scan_diff::<T, _>(
                    Some(cursor_1.reference(1 - next_bit)?),
                    None,
                    key,
                    bit_len_1,
                    0,
                    func,
                )? {
                    return Ok(false);
                }
                key1.append_bit_bool(next_bit == 1)?;
                cursor_1 = cursor_1.next_reader::<T>(next_bit, &mut 0)?;
                key1 = cursor_1.get_label_raw(&mut bit_len_1, key1)?;
            }
            (Some(_), Some(_)) => {
                // 1 and 2 are different - iterate both
                bit_len_1 -= 1;
                bit_len_2 -= 1;
                for i in 0..2 {
                    let mut key = key1.clone();
                    key.append_bit_bool(i == 1)?;
                    if !dict_scan_diff::<T, _>(
                        Some(cursor_1.reference(i)?),
                        None,
                        key,
                        bit_len_1,
                        0,
                        func,
                    )? {
                        return Ok(false);
                    }
                    let mut key = key2.clone();
                    key.append_bit_bool(i == 1)?;
                    if !dict_scan_diff::<T, _>(
                        None,
                        Some(cursor_2.reference(i)?),
                        key,
                        0,
                        bit_len_2,
                        func,
                    )? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            (None, None) => unreachable!("checked upper"),
        }
    }
}

/// concurently count elements in two hashmaps using total sum
fn hashmap_concurent_len<T: HashmapType + ?Sized>(
    data0: &Cell,
    data1: &Cell,
    bit_len: usize,
    total_size: usize,
) -> Result<(usize, usize)> {
    let mut size0 = 0;
    let mut size1 = 0;
    let cursor0 = LabelReader::with_cell(data0)?;
    let cursor1 = LabelReader::with_cell(data1)?;
    let mut vec0 = vec![(cursor0, bit_len)];
    let mut vec1 = vec![(cursor1, bit_len)];
    loop {
        let Some((mut cursor, mut bit_len)) = vec0.pop() else {
            return Ok((size0, total_size - size0));
        };
        if vec1.is_empty() {
            return Ok((total_size - size1, size1));
        }
        cursor.skip_label(&mut bit_len)?;
        if let Some(bit_len) = bit_len.checked_sub(1) {
            for i in 0..2 {
                vec0.push((cursor.next_reader::<T>(i, &mut 0)?, bit_len));
            }
        } else {
            size0 += 1;
        }
        let Some((mut cursor, mut bit_len)) = vec1.pop() else {
            fail!("unreachable: vec1 was checked for emptyness");
        };
        cursor.skip_label(&mut bit_len)?;
        if let Some(bit_len) = bit_len.checked_sub(1) {
            for i in 0..2 {
                vec1.push((cursor.next_reader::<T>(i, &mut 0)?, bit_len));
            }
        } else {
            size1 += 1;
        }
    }
}

/// iterate all elements with callback function
fn iterate_internal<T, F>(
    mut cursor: LabelReader,
    mut key: BuilderData,
    mut bit_len: usize,
    found: &mut F,
) -> Result<bool>
where
    F: FnMut(BuilderData, SliceData) -> Result<bool>,
    T: HashmapType + ?Sized,
{
    if !cursor.already_read() {
        key = cursor.get_label_raw(&mut bit_len, key)?;
    }
    if bit_len == 0 {
        found(key, cursor.remainder()?)
    } else {
        bit_len -= 1;
        for i in 0..2 {
            let mut key = key.clone();
            key.append_bit_bool(i != 0)?;
            if !iterate_internal::<T, F>(cursor.next_reader::<T>(i, &mut 0)?, key, bit_len, found)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// count all elements with callback function
fn count_internal<T: HashmapType + ?Sized>(
    mut cursor: LabelReader,
    mut bit_len: usize,
    len: &mut usize,
    max: usize,
) -> Result<bool> {
    if !cursor.already_read() {
        cursor.skip_label(&mut bit_len)?;
    }
    match bit_len.checked_sub(1) {
        Some(bit_len) => {
            for i in 0..2 {
                if !count_internal::<T>(cursor.next_reader::<T>(i, &mut 0)?, bit_len, len, max)? {
                    return Ok(false);
                }
            }
        }
        None if *len == max => return Ok(false),
        None => *len += 1,
    }
    Ok(true)
}

struct HashmapInserter<'a, T: HashmapType + ?Sized> {
    leaf: &'a BuilderData,
    gas_consumer: &'a mut dyn GasConsumer,
    mode: u8,
    phantom: PhantomData<T>,
}

impl<'a, T: HashmapType + ?Sized> HashmapInserter<'a, T> {
    fn new(leaf: &'a BuilderData, gas_consumer: &'a mut dyn GasConsumer, mode: u8) -> Self {
        Self { leaf, gas_consumer, mode, phantom: PhantomData::<T> }
    }

    /// Puts element to required branch by looking at first bit
    fn put_to_fork_with_mode(
        &mut self,
        builder: &mut BuilderData,
        bit_len: usize,
        mut key: SliceData,
    ) -> Leaf {
        if builder.references_used() != 2 {
            fail!(ExceptionCode::CellUnderflow)
        }
        let next_index = key.get_next_bit_int()?;
        let mut cell = builder.references()[next_index].clone();
        let bit_len = bit_len.checked_sub(1).ok_or(ExceptionCode::CellUnderflow)?;
        let result = self.put_to_node_with_mode(&mut cell, bit_len, key);
        builder.replace_reference_cell(next_index, cell);
        result
    }

    /// Continues or finishes search of place
    fn put_to_node_with_mode(&mut self, cell: &mut Cell, bit_len: usize, key: SliceData) -> Leaf {
        let mut result = Ok(None);
        let mut slice = self.gas_consumer.load_cell(cell.clone())?;
        let label = LabelReader::read_label(&mut slice, bit_len)?;
        if label == key {
            // replace existing leaf
            if T::is_leaf(&mut slice) {
                result = Ok(Some(slice));
                if self.mode.bit(REPLACE) {
                    *cell = self.gas_consumer.finalize_cell(
                        T::make_cell_with_label_and_builder(&key, bit_len, true, self.leaf)?,
                    )?;
                }
            } else {
                fail!(ExceptionCode::FatalError)
            }
        } else if label.is_empty_bitstring() {
            // 1-bit edge
            let is_leaf = T::is_leaf(&mut slice);
            let mut builder = slice.as_builder()?;
            match self.put_to_fork_with_mode(&mut builder, bit_len, key)? {
                None => {
                    if self.mode.bit(ADD) {
                        *cell = self.gas_consumer.finalize_cell(
                            T::make_cell_with_label_and_builder(
                                &label, bit_len, is_leaf, &builder,
                            )?,
                        )?;
                    }
                }
                Some(val) => {
                    if self.mode.bit(REPLACE) {
                        *cell = self.gas_consumer.finalize_cell(
                            T::make_cell_with_label_and_builder(
                                &label, bit_len, is_leaf, &builder,
                            )?,
                        )?;
                    }
                    result = Ok(Some(val));
                }
            }
        } else {
            match SliceData::common_prefix(&label, &key) {
                (_, _, None) => {
                    // variable length: key shorter than edge
                    if self.mode.bit(ADD) {
                        let is_leaf = T::is_leaf(&mut slice);
                        *cell = self.gas_consumer.finalize_cell(
                            T::make_cell_with_label_and_data(&label, bit_len, is_leaf, &slice)?,
                        )?;
                    }
                }
                (label_prefix, Some(label_remainder), Some(key_remainder)) => {
                    if self.mode.bit(ADD) {
                        let b = self.slice_edge(
                            slice,
                            bit_len,
                            label_prefix.unwrap_or_default(),
                            label_remainder,
                            key_remainder,
                        )?;
                        *cell = self.gas_consumer.finalize_cell(b)?;
                    }
                }
                (Some(prefix), None, Some(key_remainder)) => {
                    // next iteration
                    let is_leaf = T::is_leaf(&mut slice);
                    let mut builder = slice.as_builder()?;
                    result = self.put_to_fork_with_mode(
                        &mut builder,
                        bit_len
                            .checked_sub(prefix.remaining_bits())
                            .ok_or(ExceptionCode::CellUnderflow)?,
                        key_remainder,
                    );
                    let make_cell = match result {
                        Ok(None) => self.mode.bit(ADD),
                        Ok(Some(_)) => self.mode.bit(REPLACE),
                        _ => false,
                    };
                    if make_cell {
                        *cell = self.gas_consumer.finalize_cell(
                            T::make_cell_with_label_and_builder(
                                &label, bit_len, is_leaf, &builder,
                            )?,
                        )?;
                    }
                }
                error @ (_, _, _) => {
                    log::error!(
                        target: "tvm",
                        "If we hit this, there's certainly a bug. {:?}. \
                        Passed: label: {}, key: {} ",
                        error, label, key
                    );
                    fail!(ExceptionCode::FatalError)
                }
            }
        }
        result
    }

    /// Slices the edge and puts new leaf
    fn slice_edge(
        &mut self,
        mut slice: SliceData,
        bit_len: usize,
        prefix: SliceData,
        mut label: SliceData,
        mut key: SliceData,
    ) -> Result<BuilderData> {
        key.move_by(1)?;
        let label_bit = label.get_next_bit()?;
        let length =
            bit_len.checked_sub(prefix.remaining_bits() + 1).ok_or(ExceptionCode::CellUnderflow)?;
        let is_leaf = T::is_leaf(&mut slice);
        // Remainder of tree
        let existing_cell = self
            .gas_consumer
            .finalize_cell(T::make_cell_with_label_and_data(&label, length, is_leaf, &slice)?)?;
        // Leaf for fork
        let another_cell = self
            .gas_consumer
            .finalize_cell(T::make_cell_with_label_and_builder(&key, length, true, self.leaf)?)?;
        let (builder, _) = T::make_fork(&prefix, bit_len, existing_cell, another_cell, label_bit)?;
        Ok(builder)
    }
}

// remove method
fn remove_node<T: HashmapType + ?Sized>(
    cell_opt: &mut Option<Cell>,
    bit_len: usize,
    key: SliceData,
    allow_subtree: bool,
    gas_consumer: &mut dyn GasConsumer,
) -> Result<Option<(SliceData, SliceData)>> {
    let mut cursor = match cell_opt {
        Some(cell) => gas_consumer.load_cell(cell.clone())?,
        _ => return Ok(None),
    };
    let label = LabelReader::read_label(&mut cursor, bit_len)?;
    match SliceData::common_prefix(&label, &key) {
        // end of label - recursive down by tree
        (_, None, Some(mut remainder)) => {
            if let Some(next_bit_len) = bit_len.checked_sub(1 + label.remaining_bits()) {
                if T::is_fork(&mut cursor)? {
                    let next_index = remainder.get_next_bit_int()?;
                    let mut next_cell = Some(cursor.reference(next_index)?);
                    let result = remove_node::<T>(
                        &mut next_cell,
                        next_bit_len,
                        remainder,
                        allow_subtree,
                        gas_consumer,
                    )?;
                    if result.is_some() {
                        let other = cursor.reference(1 - next_index)?;
                        if let Some(next) = next_cell {
                            let (builder, _) =
                                T::make_fork(&label, bit_len, next, other, next_index == 1)?;
                            *cell_opt = Some(gas_consumer.finalize_cell(builder)?)
                        } else {
                            let builder = T::make_edge(
                                &label,
                                bit_len,
                                next_index == 1,
                                gas_consumer.load_cell(other)?,
                            )?;
                            *cell_opt = Some(gas_consumer.finalize_cell(builder)?)
                        }
                    }
                    return Ok(result);
                }
            }
            fail!(ExceptionCode::CellUnderflow)
        }
        // end of kkey - leaf found
        (_, None, None) => {
            if T::is_leaf(&mut cursor) {
                *cell_opt = None;
                Ok(Some((cursor, SliceData::default())))
            } else {
                fail!(ExceptionCode::CellUnderflow)
            }
        }
        // no exact branch
        (_, Some(_), Some(_)) => Ok(None),
        // end of key - subtree found
        (_, Some(label), None) => {
            if allow_subtree {
                *cell_opt = None;
                Ok(Some((cursor, label)))
            } else {
                fail!(ExceptionCode::CellUnderflow)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HashmapFilterResult {
    Cancel, // cancel traverse and skip changes
    Stop,   // cancel traverse and accept changes
    Remove, // remove element and continue
    Accept, // accept element and continue
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HashmapFilterSplitResult {
    Cancel, // cancel traverse and skip changes
    Stay,   // stay in original hashmap
    Move,   // move to new hashmap
}

pub trait HashmapRemover: HashmapType + Clone + Sized {
    fn after_remove(&mut self) -> Result<()> {
        // it is for augmented hashmaps
        Ok(())
    }
    // removes item from hashmap by key with returning this item
    fn hashmap_remove(
        &mut self,
        key: SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<SliceData>> {
        let bit_len = self.bit_len();
        Self::check_key_fail(bit_len, &key)?;
        let result = remove_node::<Self>(self.data_mut(), bit_len, key, false, gas_consumer)?;
        self.after_remove()?;
        Ok(result.map(|(r, _l)| r))
    }
    // removes item from hashmap by key with returning this item
    fn remove(&mut self, key: SliceData) -> Result<Option<SliceData>> {
        self.hashmap_remove(key, &mut 0)
    }
    /// removes subtree by prefix and returns it as new full tree
    fn hashmap_slice(
        &mut self,
        key: SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Self> {
        let bit_len = self.bit_len();
        let result =
            remove_node::<Self>(self.data_mut(), bit_len, key.clone(), true, gas_consumer)?;
        self.after_remove()?;

        let mut leftover = self.clone();
        if let Some((mut remainder, label)) = result {
            let is_leaf = Self::is_leaf(&mut remainder);

            let mut builder = key.into_builder()?;
            builder.append_bytestring(&label)?;
            let label = SliceData::load_bitstring(builder)?;

            let root =
                Self::make_cell_with_label_and_data(&label, self.bit_len(), is_leaf, &remainder)?;
            *leftover.data_mut() = Some(gas_consumer.finalize_cell(root)?)
        } else {
            *leftover.data_mut() = None;
        }
        leftover.after_remove()?;
        Ok(leftover)
    }
    // removes items from hashamp in one pass
    // closure must return decision for item to accept it or to remove it
    fn hashmap_filter<F>(&mut self, mut func: F) -> Result<HashmapFilterResult>
    where
        F: FnMut(&BuilderData, SliceData) -> Result<HashmapFilterResult>,
    {
        self.hashmap_filter_with_root(|key, data, _root| func(key, data))
    }
    // removes items from hashamp in one pass
    // closure must return decision for item to accept it or to remove it
    fn hashmap_filter_with_root<F>(&mut self, mut func: F) -> Result<HashmapFilterResult>
    where
        F: FnMut(&BuilderData, SliceData, Option<&Cell>) -> Result<HashmapFilterResult>,
    {
        let bit_len = self.bit_len();
        let mut result = HashmapFilterResult::Accept;
        if let Some(cell) = self.data() {
            let (removed, res) = filter_next::<Self, _>(
                cell.clone(),
                BuilderData::default(),
                bit_len,
                &mut result,
                None,
                &mut func,
            )?;
            if removed {
                *self.data_mut() = res.map(|res| res.cell);
                self.after_remove()?;
                return Ok(HashmapFilterResult::Remove);
            }
        }
        Ok(result)
    }
    // splits hashmap in one pass with creating new one
    // closure must return decision for item to stay or to move to other hashmap
    // no creation of tree only removing
    fn hashmap_filter_split<F>(&mut self, mut func: F) -> Result<Self>
    where
        F: FnMut(&BuilderData, SliceData) -> Result<HashmapFilterSplitResult>,
    {
        let mut new_map = self.clone();
        let bit_len = self.bit_len();
        let mut result = HashmapFilterSplitResult::Stay;
        if let Some(cell) = self.data() {
            let (_, left, right) = filter_split_next::<Self, _>(
                cell.clone(),
                BuilderData::default(),
                bit_len,
                &mut result,
                &mut func,
            )?;
            *self.data_mut() = left.map(|res| res.cell);
            self.after_remove()?;
            *new_map.data_mut() = right.map(|res| res.cell);
            new_map.after_remove()?;
        }
        Ok(new_map)
    }
}

struct ForkComponent {
    cell: Cell,
    key: BuilderData,
    remainder: SliceData,
}

/// make variable edge
/// fork or edge or None using next array of edges
fn make_var_edge<T: HashmapType + ?Sized>(
    key: BuilderData,             // current key for fork
    mut next: Vec<ForkComponent>, // new fork components or new edge
    key_length: usize,            // length of key on previous level
    bit_len: usize,               // current bit_len for making label
) -> Result<Option<ForkComponent>> {
    if let Some(ForkComponent { cell: right, key: new_key, remainder }) = next.pop() {
        if let Some(ForkComponent { cell: left, key: _, remainder: _ }) = next.pop() {
            // prepare new fork
            let mut label = SliceData::load_bitstring(key.clone())?;
            label.move_by(key_length)?;
            let (builder, remainder) = T::make_fork(&label, bit_len, left, right, false)?;
            let cell = builder.into_cell()?;
            let remainder = SliceData::load_builder(remainder)?;
            Ok(Some(ForkComponent { cell, key, remainder }))
        } else {
            // replace fork with edge
            let key = new_key;
            let mut label = SliceData::load_bitstring(key.clone())?;
            label.move_by(key_length)?;
            let mut builder = T::make_cell_with_label(&label, bit_len)?;
            builder.checked_append_references_and_data(&remainder)?;
            let cell = builder.into_cell()?;
            Ok(Some(ForkComponent { cell, key, remainder }))
        }
    } else {
        Ok(None)
    }
}

fn filter_next<T, F>(
    cell: Cell,
    key: BuilderData,
    mut bit_len: usize,
    result: &mut HashmapFilterResult,
    mut root: Option<Cell>,
    func: &mut F,
) -> Result<(bool, Option<ForkComponent>)>
// is_removed and remainder
where
    T: HashmapType + ?Sized,
    F: FnMut(&BuilderData, SliceData, Option<&Cell>) -> Result<HashmapFilterResult>,
{
    if result == &HashmapFilterResult::Cancel {
        return Ok((false, None));
    }

    if result == &HashmapFilterResult::Stop {
        return Ok((false, Some(ForkComponent { cell, key, remainder: SliceData::default() })));
    }
    let mut cursor = SliceData::load_cell(cell.clone())?;
    let key_length = key.length_in_bits();
    let this_bit_len = bit_len;
    let key = LabelReader::read_label_raw(&mut cursor, &mut bit_len, key)?;
    let remainder = cursor.clone();
    if bit_len == 0 {
        let removed = match func(&key, cursor, root.as_ref())? {
            HashmapFilterResult::Remove => return Ok((true, None)),
            HashmapFilterResult::Accept => false,
            new_result => {
                *result = new_result;
                false
            }
        };
        return Ok((removed, Some(ForkComponent { cell, key, remainder })));
    }
    let mut changed = false;
    bit_len -= 1;
    let mut next = vec![];
    for i in 0..2 {
        let mut key = key.clone();
        key.append_bit_bool(i == 1)?;
        let child = cursor.checked_drain_reference()?;
        let (removed, remainder) =
            filter_next::<T, F>(child, key, bit_len, result, root.clone(), func)?;
        if result == &HashmapFilterResult::Cancel {
            return Ok((false, None));
        }
        changed |= removed;
        if let Some(remainder) = remainder {
            next.push(remainder);
        }
        if changed && root.is_none() {
            root = Some(cell.clone());
        }
    }
    if !changed {
        Ok((false, Some(ForkComponent { cell, key, remainder })))
    } else {
        let remainder = make_var_edge::<T>(key, next, key_length, this_bit_len)?;
        Ok((true, remainder))
    }
}

const SPLIT_RESULT_CHANGED_NONE: u8 = 0;
const SPLIT_RESULT_CHANGED_FIRST: u8 = 1;
const SPLIT_RESULT_CHANGED_SECOND: u8 = 2;

fn filter_split_next<T, F>(
    cell: Cell,
    key: BuilderData,
    mut bit_len: usize,
    result: &mut HashmapFilterSplitResult,
    func: &mut F,
) -> Result<(u8, Option<ForkComponent>, Option<ForkComponent>)>
// split result flag and remainders
where
    T: HashmapType + ?Sized,
    F: FnMut(&BuilderData, SliceData) -> Result<HashmapFilterSplitResult>,
{
    // we get cancel signal just skip all operations and quit
    if result == &HashmapFilterSplitResult::Cancel {
        return Ok((0, None, None));
    }
    // load edge to slice or cancel operation if fail (it is only can be root case)
    let mut cursor = SliceData::load_cell_ref(&cell)?;
    // current key length
    let key_length = key.length_in_bits();
    // store current maximum bit_len
    let this_bit_len = bit_len;
    // continue read key from label
    let key = LabelReader::read_label_raw(&mut cursor, &mut bit_len, key)?;
    let remainder = cursor.clone();
    // leaf found
    if bit_len == 0 {
        // analyze on client side and cut left or right
        match func(&key, cursor)? {
            // item stay in original tree
            HashmapFilterSplitResult::Stay => {
                let next = ForkComponent { cell, key, remainder };
                return Ok((SPLIT_RESULT_CHANGED_SECOND, Some(next), None));
            }
            // item moved to the new tree
            HashmapFilterSplitResult::Move => {
                let next = ForkComponent { cell, key, remainder };
                return Ok((SPLIT_RESULT_CHANGED_FIRST, None, Some(next)));
            }
            HashmapFilterSplitResult::Cancel => {
                *result = HashmapFilterSplitResult::Cancel;
                return Ok((SPLIT_RESULT_CHANGED_NONE, None, None));
            }
        };
    }
    bit_len -= 1;
    let mut next1 = vec![];
    let mut next2 = vec![];
    let mut split_result = SPLIT_RESULT_CHANGED_NONE;
    for i in 0..2 {
        let mut key = key.clone();
        key.append_bit_bool(i == 1)?;
        let cell = cursor.checked_drain_reference()?;
        // after split we will receive new edges
        let (this_split_result, left, right) =
            filter_split_next::<T, F>(cell, key, bit_len, result, func)?;
        if result == &HashmapFilterSplitResult::Cancel {
            return Ok((SPLIT_RESULT_CHANGED_NONE, None, None));
        }
        split_result |= this_split_result;
        if let Some(left) = left {
            next1.push(left);
        }
        if let Some(right) = right {
            next2.push(right);
        }
    }
    let left = if (split_result & SPLIT_RESULT_CHANGED_FIRST) != 0 {
        make_var_edge::<T>(key.clone(), next1, key_length, this_bit_len)?
    } else {
        Some(ForkComponent { cell: cell.clone(), key: key.clone(), remainder: remainder.clone() })
    };
    let right = if (split_result & SPLIT_RESULT_CHANGED_SECOND) != 0 {
        make_var_edge::<T>(key, next2, key_length, this_bit_len)?
    } else {
        Some(ForkComponent { cell, key, remainder })
    };
    Ok((split_result, left, right))
}

pub trait HashmapSubtree: HashmapRemover + Clone + Sized {
    /// make subtree by common prefix
    fn subtree_with_prefix(
        &self,
        prefix: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Self> {
        let mut new_map = self.clone();
        if prefix.is_empty_bitstring() {
            return Ok(new_map);
        }
        if let Some((key, mut remainder, None)) = self.subtree_root(prefix, gas_consumer)? {
            let label = SliceData::load_bitstring(key)?;
            let is_leaf = Self::is_leaf(&mut remainder);
            if remainder.cell_opt() != self.data() {
                let root = Self::make_cell_with_label_and_data(
                    &label,
                    self.bit_len(),
                    is_leaf,
                    &remainder,
                )?;
                *new_map.data_mut() = Some(gas_consumer.finalize_cell(root)?);
                new_map.after_remove()?;
            }
        } else {
            *new_map.data_mut() = None;
            new_map.after_remove()?;
        }
        Ok(new_map)
    }

    /// transform to subtree with the common prefix
    // TBD
    #[allow(clippy::wrong_self_convention)]
    fn into_subtree_with_prefix(
        &mut self,
        prefix: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<()> {
        *self = self.subtree_with_prefix(prefix, gas_consumer)?;
        Ok(())
    }

    /// returns key, subtree remainder and prefix_remainder of traversed subtree
    /// if prefix fully found its remainder is None
    fn subtree_root(
        &self,
        prefix: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<(BuilderData, SliceData, Option<SliceData>)>> {
        if prefix.is_empty_bitstring() {
            fail!("Prefix is empty")
        }
        let prefix_len = prefix.remaining_bits();
        if prefix_len > self.bit_len() {
            fail!("Prefix too long {} more than {}", prefix_len, self.bit_len())
        }
        if let Some(root) = self.data() {
            let mut cursor = LabelReader::new(gas_consumer.load_cell(root.clone())?);
            let (key, prefix_remainder_opt) =
                down_by_tree::<Self>(prefix, &mut cursor, self.bit_len(), gas_consumer)?;
            Ok(Some((key, cursor.remainder()?, prefix_remainder_opt)))
        } else {
            Ok(None)
        }
    }

    /// returns original root cell of traversed subtree
    fn subtree_root_cell(&self, prefix: &SliceData) -> Result<Option<Cell>> {
        if let Some((_key, remainder, None)) = self.subtree_root(prefix, &mut 0)? {
            Ok(Some(remainder.cell()?))
        } else {
            Ok(None)
        }
    }
}

fn down_by_tree<T>(
    prefix: &SliceData,
    cursor: &mut LabelReader,
    mut bit_len: usize,
    gas_consumer: &mut dyn GasConsumer,
) -> Result<(BuilderData, Option<SliceData>)>
where
    T: HashmapType + ?Sized,
{
    let mut key = BuilderData::default();
    loop {
        key = cursor.get_label_raw(&mut bit_len, key)?;
        let label = SliceData::load_cell(key.clone().into_cell()?)?;
        match SliceData::common_prefix(&label, prefix) {
            (_, None, Some(mut rem_prefix)) => {
                // continue down
                bit_len = bit_len.checked_sub(1).ok_or(ExceptionCode::CellUnderflow)?;
                let next_index = rem_prefix.get_next_bit_int()?;
                key.append_bit_bool(next_index == 1)?;
                *cursor = cursor.next_reader::<T>(next_index, gas_consumer)?;
            }
            (_, _, rem_prefix) => return Ok((key, rem_prefix)),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct HashmapIterator<T: HashmapType + ?Sized> {
    pos: Vec<(LabelReader, usize, BuilderData)>,
    phantom: PhantomData<T>,
}

impl<T: HashmapType + ?Sized> HashmapIterator<T> {
    pub fn from_hashmap(tree: &T) -> Self {
        let mut pos = vec![];
        if let Some(root) = tree.data() {
            // must be checked here for Pruned cell
            let cursor = SliceData::load_cell_ref(root).expect("need to check root");
            pos.push((LabelReader::new(cursor), tree.bit_len(), BuilderData::default()));
        }
        Self { pos, phantom: PhantomData::<T> }
    }
    // is_leaf and is_fork are not used here
    pub fn next_item(&mut self) -> Result<Option<(BuilderData, SliceData)>> {
        while let Some((mut cursor, mut bit_len, key)) = self.pos.pop() {
            let key = cursor.get_label_raw(&mut bit_len, key)?;
            if bit_len == 0 {
                return Ok(Some((key, cursor.remainder()?)));
            }
            for index in 0..2 {
                let mut key = key.clone();
                key.append_bit_bool(index == 0)?;
                let cursor = cursor.next_reader::<T>(1 - index, &mut 0)?;
                self.pos.push((cursor, bit_len - 1, key));
            }
        }
        Ok(None)
    }
}

impl<T: HashmapType + ?Sized> Iterator for HashmapIterator<T> {
    type Item = Result<(BuilderData, SliceData)>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_item().transpose()
    }
}

// --- Batch multiset implementation ---

/// Zero-copy bit-level view over a byte buffer
#[derive(Clone, Copy, Eq, Debug)]
pub struct FixedBitsKey<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
    bit_len: usize,
}

impl<'a> FixedBitsKey<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_offset: 0, bit_len: bytes.len() * 8 }
    }

    pub fn with_offset(bytes: &'a [u8], bit_offset: usize, bit_len: usize) -> Result<Self> {
        if bytes.len() * 8 < bit_offset + bit_len {
            fail!(
                "FixedBitsKey: buffer too small ({} bytes for offset {} + {} bits)",
                bytes.len(),
                bit_offset,
                bit_len,
            );
        }
        Ok(Self { bytes, bit_offset, bit_len })
    }

    pub fn from_slice_data(slice: &'a SliceData) -> Self {
        Self { bytes: slice.storage(), bit_offset: slice.pos(), bit_len: slice.remaining_bits() }
    }

    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    fn check_range(&self, offset: usize, len: usize) -> Result<()> {
        if offset + len > self.bit_len {
            fail!(
                "FixedBitsKey: range {}..{} exceeds bit_len {}",
                offset,
                offset + len,
                self.bit_len
            );
        }
        Ok(())
    }

    /// Read a single bit at `offset`.
    pub fn get_bit(&self, offset: usize) -> Result<bool> {
        self.check_range(offset, 1)?;
        let abs = self.bit_offset + offset;
        Ok((self.bytes[abs / 8] >> (7 - abs % 8)) & 1 != 0)
    }

    /// Extract a range of key bits as a `SliceData` (for label construction).
    pub fn slice(&self, offset: usize, len: usize) -> Result<SliceData> {
        if len == 0 {
            return Ok(SliceData::default());
        }
        self.check_range(offset, len)?;
        let abs = self.bit_offset + offset;
        let mut result = SliceData::with_bitstring(self.bytes, abs + len);
        result.move_by(abs)?;
        Ok(result)
    }

    /// Common prefix length (in bits) between this key from `self_offset` and
    /// `other` from `other_offset`, up to `max_len` bits.
    pub fn common_prefix_bits(
        &self,
        self_offset: usize,
        other: &Self,
        other_offset: usize,
        max_len: usize,
    ) -> Result<usize> {
        self.check_range(self_offset, max_len)?;
        other.check_range(other_offset, max_len)?;

        let abs_a = self.bit_offset + self_offset;
        let abs_b = other.bit_offset + other_offset;
        let ra = abs_a % 8;
        let rb = abs_b % 8;
        let mut qa = abs_a / 8;
        let mut qb = abs_b / 8;

        #[inline(always)]
        fn get_bits(bytes: &[u8], q: usize, r: usize, bits: usize) -> u8 {
            let mask = 0xFFu8.wrapping_shl((8 - bits) as u32);
            if r + bits <= 8 {
                (bytes[q] << r) & mask
            } else {
                // bytes[q + 1] is required for the data, so it's guaranteed valid when
                // the caller has already checked the bit range via `check_range`.
                let hi = (bytes[q] as u16) << 8;
                let lo = bytes[q + 1] as u16;
                ((hi | lo) >> (8 - r)) as u8 & mask
            }
        }

        let mut i = 0;
        while i + 8 <= max_len {
            let a = get_bits(self.bytes, qa, ra, 8);
            let b = get_bits(other.bytes, qb, rb, 8);
            if a != b {
                return Ok(i + (a ^ b).leading_zeros() as usize);
            }
            i += 8;
            qa += 1;
            qb += 1;
        }
        let tail = max_len - i;
        if tail > 0 {
            let a = get_bits(self.bytes, qa, ra, tail);
            let b = get_bits(other.bytes, qb, rb, tail);
            if a != b {
                return Ok(i + (a ^ b).leading_zeros() as usize);
            }
        }
        Ok(max_len)
    }
}

impl<'a> PartialEq for FixedBitsKey<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl<'a> Ord for FixedBitsKey<'a> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.bit_len != other.bit_len {
            return self.bit_len.cmp(&other.bit_len);
        }
        // Fast path: both keys start on a byte boundary.
        if self.bit_offset.is_multiple_of(8) && other.bit_offset.is_multiple_of(8) {
            let sa = self.bit_offset / 8;
            let ob = other.bit_offset / 8;
            let full_bytes = self.bit_len / 8;
            let tail = self.bit_len % 8;
            let cmp = self.bytes[sa..sa + full_bytes].cmp(&other.bytes[ob..ob + full_bytes]);
            if cmp != std::cmp::Ordering::Equal || tail == 0 {
                return cmp;
            }
            let mask = 0xFFu8 << (8 - tail);
            return (self.bytes[sa + full_bytes] & mask)
                .cmp(&(other.bytes[ob + full_bytes] & mask));
        }
        self.cmp_unaligned(other)
    }
}

impl<'a> FixedBitsKey<'a> {
    /// Slow path for `Ord::cmp` when keys are not byte-aligned.
    fn cmp_unaligned(&self, other: &Self) -> std::cmp::Ordering {
        let common_len = self.common_prefix_bits(0, other, 0, self.bit_len).unwrap_or(0);
        if common_len == self.bit_len {
            std::cmp::Ordering::Equal
        } else {
            let a = self.get_bit(common_len).unwrap_or(false);
            let b = other.get_bit(common_len).unwrap_or(false);
            a.cmp(&b)
        }
    }
}

impl<'a> PartialOrd for FixedBitsKey<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Binary search for the first index in `values` whose key has bit 1 at `bit_pos`.
/// Equivalent to `partition_point` but propagates errors.
fn partition_by_bit(
    values: &[(FixedBitsKey<'_>, Option<SliceData>)],
    bit_pos: usize,
) -> Result<usize> {
    let mut lo = 0;
    let mut hi = values.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if values[mid].0.get_bit(bit_pos)? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

/// Build a new hashmap subtree from a sorted slice of key-value pairs.
/// Values with `None` data are treated as deletions of non-existent keys (no-ops).
fn dict_build_impl<T: HashmapType + ?Sized>(
    values: &[(FixedBitsKey<'_>, Option<SliceData>)],
    total_key_len: usize,
    prefix_len: usize,
) -> Result<Option<Cell>> {
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() == 1 {
        return match &values[0].1 {
            Some(data) => {
                let remaining = total_key_len - prefix_len;
                let label = values[0].0.slice(prefix_len, remaining)?;
                let builder = T::make_cell_with_label_and_data(&label, remaining, true, data)?;
                Ok(Some(builder.into_cell()?))
            }
            None => Ok(None),
        };
    }
    let remaining = total_key_len - prefix_len;
    let first = &values[0].0;
    let last = &values.last().unwrap().0;
    let common = first.common_prefix_bits(prefix_len, last, prefix_len, remaining)?;
    debug_assert!(common < remaining);

    let split_pos = prefix_len + common;
    let idx = partition_by_bit(values, split_pos)?;

    let left = dict_build_impl::<T>(&values[..idx], total_key_len, split_pos + 1)?;
    let right = dict_build_impl::<T>(&values[idx..], total_key_len, split_pos + 1)?;

    let label = values[0].0.slice(prefix_len, common)?;
    make_subtree_cell::<T>(&label, remaining, left, right)
}

/// Merge an existing hashmap cell with a sorted slice of new values.
/// `n` is the remaining key bits at this level, `skip` is bits to skip in the dict's label.
fn dict_multiset_impl<T: HashmapType + ?Sized>(
    dict: Option<Cell>,
    values: &[(FixedBitsKey<'_>, Option<SliceData>)],
    n: usize,
    total_key_len: usize,
    skip: usize,
) -> Result<Option<Cell>> {
    let prefix_len = total_key_len - n;

    if dict.is_none() {
        return dict_build_impl::<T>(values, total_key_len, prefix_len);
    }
    if values.is_empty() {
        debug_assert_eq!(skip, 0);
        return Ok(dict);
    }

    // Compute common prefix length among values
    let l2 = if values.len() <= 1 {
        n
    } else {
        values[0].0.common_prefix_bits(prefix_len, &values.last().unwrap().0, prefix_len, n)?
    };

    // Split values into left/right at the divergence point
    let split_pos = prefix_len + l2;
    let (values_left, values_right) = if split_pos < total_key_len && values.len() > 1 {
        let idx = partition_by_bit(values, split_pos)?;
        (&values[..idx], &values[idx..])
    } else {
        (&values[..0], values)
    };

    // Parse dict's label
    let dict_cell = dict.unwrap();
    let mut dict_rem = SliceData::load_cell(dict_cell.clone())?;
    let dict_label = LabelReader::read_label(&mut dict_rem, n + skip)?;
    let l1 = dict_label.remaining_bits() - skip;

    // Find how many bits of dict's label match the values' common prefix.
    let dict_label_key = FixedBitsKey::from_slice_data(&dict_label);
    let c = values[0].0.common_prefix_bits(prefix_len, &dict_label_key, skip, l1.min(l2))?;

    if c < l1 && c < l2 {
        // Case 1: Disjoint — dict and values diverge before either label ends.
        // Prune dict's label and create a fork at the divergence point.
        let pruned_label = dict_label.get_slice(skip + c + 1, l1 - c - 1)?;
        let pruned = T::make_cell_with_label_and_data(&pruned_label, n - c - 1, false, &dict_rem)?;
        let dict_side = pruned.into_cell()?;

        let values_side = dict_build_impl::<T>(values, total_key_len, prefix_len + c + 1)?;

        let dict_goes_right = dict_label.get_bit(skip + c)?;
        let (left, right) = if !dict_goes_right {
            (Some(dict_side), values_side)
        } else {
            (values_side, Some(dict_side))
        };

        let label = dict_label.get_slice(skip, c)?;
        return make_subtree_cell::<T>(&label, n, left, right);
    }

    if c == l1 && c == l2 {
        // Case 2: Labels match exactly.
        if c == n {
            // Leaf match: update or delete
            return match &values[0].1 {
                Some(data) => {
                    let label = dict_label.get_slice(skip, c)?;
                    let builder = T::make_cell_with_label_and_data(&label, n, true, data)?;
                    Ok(Some(builder.into_cell()?))
                }
                None => Ok(None),
            };
        }
        // Fork: recurse into both children
        let left_child = dict_rem.checked_drain_reference()?;
        let right_child = dict_rem.checked_drain_reference()?;

        let c1 =
            dict_multiset_impl::<T>(Some(left_child), values_left, n - c - 1, total_key_len, 0)?;
        let c2 =
            dict_multiset_impl::<T>(Some(right_child), values_right, n - c - 1, total_key_len, 0)?;

        let label = dict_label.get_slice(skip, c)?;
        return make_subtree_cell::<T>(&label, n, c1, c2);
    }

    if c == l1 {
        // Case 3: c == l1 < l2 — dict's label is shorter (dict is a fork).
        // All values share the same bit at the fork point, go to one side.
        let left_child = dict_rem.checked_drain_reference()?;
        let right_child = dict_rem.checked_drain_reference()?;

        let values_go_right = values[0].0.get_bit(prefix_len + c)?;
        let (c1, c2) = if !values_go_right {
            let c1 =
                dict_multiset_impl::<T>(Some(left_child), values, n - c - 1, total_key_len, 0)?;
            (c1, Some(right_child))
        } else {
            let c2 =
                dict_multiset_impl::<T>(Some(right_child), values, n - c - 1, total_key_len, 0)?;
            (Some(left_child), c2)
        };

        let label = dict_label.get_slice(skip, c)?;
        return make_subtree_cell::<T>(&label, n, c1, c2);
    }

    // Case 4: c == l2 < l1 — values' prefix is shorter (values fork within dict's edge).
    // Dict goes to one side (with skip), other side is built from values.
    debug_assert!(c == l2 && c < l1);
    let dict_goes_right = dict_label.get_bit(skip + c)?;
    let (c1, c2) = if !dict_goes_right {
        let c1 = dict_multiset_impl::<T>(
            Some(dict_cell),
            values_left,
            n - c - 1,
            total_key_len,
            skip + c + 1,
        )?;
        let c2 = dict_build_impl::<T>(values_right, total_key_len, prefix_len + c + 1)?;
        (c1, c2)
    } else {
        let c2 = dict_multiset_impl::<T>(
            Some(dict_cell),
            values_right,
            n - c - 1,
            total_key_len,
            skip + c + 1,
        )?;
        let c1 = dict_build_impl::<T>(values_left, total_key_len, prefix_len + c + 1)?;
        (c1, c2)
    };

    let label = values[0].0.slice(prefix_len, c)?;
    make_subtree_cell::<T>(&label, n, c1, c2)
}

/// Assemble a subtree from a label and two optional children.
fn make_subtree_cell<T: HashmapType + ?Sized>(
    label: &SliceData,
    n: usize,
    left: Option<Cell>,
    right: Option<Cell>,
) -> Result<Option<Cell>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(left), Some(right)) => {
            let (builder, _) = T::make_fork(label, n, left, right, false)?;
            Ok(Some(builder.into_cell()?))
        }
        (Some(child), None) => {
            let next = SliceData::load_cell(child)?;
            Ok(Some(T::make_edge(label, n, true, next)?.into_cell()?))
        }
        (None, Some(child)) => {
            let next = SliceData::load_cell(child)?;
            Ok(Some(T::make_edge(label, n, false, next)?.into_cell()?))
        }
    }
}

#[cfg(test)]
#[path = "tests/test_dictionary.rs"]
mod tests;
