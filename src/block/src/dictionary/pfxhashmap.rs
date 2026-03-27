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
use super::{
    hm_label, BuilderData, Cell, HashmapRemover, HashmapType, IBitstring, Leaf, SliceData, ADD,
    REPLACE,
};
use crate::{GasConsumer, LabelReader, Result};
use std::fmt;

#[derive(Clone, Debug)]
pub struct PfxHashmapE {
    bit_len: usize,
    data: Option<Cell>,
}

impl fmt::Display for PfxHashmapE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.data() {
            Some(cell) => write!(f, "PfxHashmap: {}", cell),
            None => write!(f, "Empty PfxHashmap"),
        }
    }
}

impl PfxHashmapE {
    /// constructs with bit_len
    pub const fn with_bit_len(bit_len: usize) -> Self {
        Self::with_hashmap(bit_len, None)
    }
    /// construct with bit_len and root representing Hashmap
    pub const fn with_hashmap(bit_len: usize, data: Option<Cell>) -> Self {
        Self { bit_len, data }
    }
    /// gets value from hahsmap
    pub fn get(&self, key: SliceData) -> Leaf {
        self.hashmap_get(key, &mut 0)
    }
    pub fn get_with_gas(&self, key: SliceData, gas_consumer: &mut dyn GasConsumer) -> Leaf {
        self.hashmap_get(key, gas_consumer)
    }
    /// sets value as SliceData
    pub fn set(&mut self, key: SliceData, value: &SliceData) -> Leaf {
        self.hashmap_set_with_mode(key, &value.as_builder()?, &mut 0, ADD | REPLACE)
    }
    pub fn set_builder(&mut self, key: SliceData, value: &BuilderData) -> Leaf {
        self.hashmap_set_with_mode(key, value, &mut 0, ADD | REPLACE)
    }
    pub fn set_with_gas(
        &mut self,
        key: SliceData,
        value: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, &value.as_builder()?, gas_consumer, ADD | REPLACE)
    }
    pub fn set_builder_with_gas(
        &mut self,
        key: SliceData,
        value: &BuilderData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, value, gas_consumer, ADD | REPLACE)
    }
    pub fn replace_with_gas(
        &mut self,
        key: SliceData,
        value: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, &value.as_builder()?, gas_consumer, REPLACE)
    }
    pub fn replace_builder_with_gas(
        &mut self,
        key: SliceData,
        value: &BuilderData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, value, gas_consumer, REPLACE)
    }
    /// sets value as reference in empty SliceData
    pub fn setref(&mut self, key: SliceData, value: Cell) -> Leaf {
        self.hashmap_setref_with_mode(key, value, &mut 0, ADD | REPLACE)
    }
    pub fn setref_with_gas(
        &mut self,
        key: SliceData,
        value: Cell,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_setref_with_mode(key, value, gas_consumer, ADD | REPLACE)
    }
    pub fn replaceref_with_gas(
        &mut self,
        key: SliceData,
        value: Cell,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_setref_with_mode(key, value, gas_consumer, REPLACE)
    }
    /// removes item
    pub fn remove(&mut self, key: SliceData) -> Leaf {
        self.hashmap_remove(key, &mut 0)
    }
    pub fn remove_with_gas(&mut self, key: SliceData, gas_consumer: &mut dyn GasConsumer) -> Leaf {
        self.hashmap_remove(key, gas_consumer)
    }
    /// true if key is prefix of any item in PfxHashmap
    pub fn is_prefix(&self, mut key: SliceData) -> Result<bool> {
        let mut bit_len = self.bit_len;
        let mut cursor = match self.data() {
            Some(root) if !key.is_empty_bitstring() => SliceData::load_cell_ref(root)?,
            _ => return Ok(false),
        };
        let mut label = LabelReader::read_label(&mut cursor, bit_len)?;
        loop {
            match SliceData::common_prefix(&label, &key) {
                (_, None, None) => (),                         // label == key
                (_, None, Some(remainder)) => key = remainder, // usual case
                (_, _, None) => return Ok(true),               // key is prefix
                (_, Some(_), Some(_)) => return Ok(false),
            }
            if Self::is_leaf(&mut cursor) {
                return Ok(false);
            }
            let next_index = key.get_next_bit_int()?;
            if next_index >= cursor.remaining_references() || bit_len < label.remaining_bits() + 1 {
                debug_assert!(false);
                return Ok(false); // problem
            }
            cursor = SliceData::load_cell(cursor.reference(next_index)?)?;
            bit_len -= label.remaining_bits() + 1;
            label = LabelReader::read_label(&mut cursor, bit_len)?;
        }
    }
    /// finds item in PfxHashmap which key is prefix of key and returns value with path and suffix
    pub fn get_prefix_leaf_with_gas(
        &self,
        mut key: SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<(SliceData, Option<SliceData>, SliceData)> {
        let mut bit_len = self.bit_len;
        let mut cursor = match self.data().cloned() {
            Some(root) if !key.is_empty_bitstring() => gas_consumer.load_cell(root)?,
            _ => return Ok((SliceData::default(), None, key)),
        };
        let mut path = BuilderData::default();
        let mut label = LabelReader::read_label(&mut cursor, bit_len)?;
        loop {
            path.append_bytestring(&label)?;
            match SliceData::common_prefix(&label, &key) {
                (_, None, None) => {
                    // label == key
                    key.clear_all_bits();
                }
                (_, None, Some(remainder)) => key = remainder, // usual case
                (_, _, None) => {
                    return Ok((SliceData::load_bitstring(path)?, None, SliceData::default()))
                } // key is prefix
                (_, Some(_), Some(remainder)) => {
                    return Ok((SliceData::load_bitstring(path)?, None, remainder))
                }
            }
            if Self::is_leaf(&mut cursor) {
                return Ok((SliceData::load_bitstring(path)?, Some(cursor), key));
            } else if key.is_empty_bitstring() {
                return Ok((SliceData::load_bitstring(path)?, None, key));
            }
            let next_index = key.get_next_bit_int()?;
            if next_index >= cursor.remaining_references() || bit_len < label.remaining_bits() + 1 {
                debug_assert!(false);
                return Ok((SliceData::load_bitstring(path)?, None, key)); // problem
            }
            path.append_bit_bool(next_index == 1)?;
            cursor = gas_consumer.load_cell(cursor.reference(next_index)?)?;
            bit_len -= label.remaining_bits() + 1;
            label = LabelReader::read_label(&mut cursor, bit_len)?;
        }
    }
    #[allow(dead_code)]
    pub fn get_leaf_by_prefix(
        &self,
        mut key: SliceData,
    ) -> Result<(SliceData, Option<SliceData>, SliceData)> {
        let mut bit_len = self.bit_len;
        let mut cursor = match self.data() {
            Some(root) if !key.is_empty_bitstring() => SliceData::load_cell_ref(root)?,
            _ => return Ok((SliceData::default(), None, key)),
        };
        let mut path = BuilderData::default();
        let mut label = LabelReader::read_label(&mut cursor, bit_len)?;
        loop {
            path.checked_append_references_and_data(&label)?;
            match SliceData::common_prefix(&label, &key) {
                (_, None, None) => {
                    // label == key
                    key.clear_all_bits();
                }
                (_, None, Some(remainder)) => key = remainder, // usual case
                (_, _, None) => break,                         // key is prefix
                (_, Some(_), Some(remainder)) => {
                    return Ok((SliceData::load_bitstring(path)?, None, remainder))
                }
            }
            if Self::is_leaf(&mut cursor) {
                return Ok((SliceData::load_bitstring(path)?, Some(cursor), key));
            }
            let next_index = key.get_next_bit_int()?;
            if next_index >= cursor.remaining_references() || bit_len < label.remaining_bits() + 1 {
                debug_assert!(false);
                return Ok((SliceData::load_bitstring(path)?, None, key)); // problem
            }
            path.append_bit_bool(next_index == 1)?;
            cursor = SliceData::load_cell(cursor.reference(next_index)?)?;
            bit_len -= label.remaining_bits() + 1;
            label = LabelReader::read_label(&mut cursor, bit_len)?;
        }
        key = SliceData::default();
        loop {
            if Self::is_leaf(&mut cursor) {
                return Ok((SliceData::load_bitstring(path)?, Some(cursor), key));
            }
            let next_index = 0;
            if next_index >= cursor.remaining_references() {
                return Ok((SliceData::load_bitstring(path)?, None, key)); // problem
            }
            path.append_bit_bool(next_index == 1)?;
            cursor = SliceData::load_cell(cursor.reference(next_index)?)?;
            if bit_len < label.remaining_bits() + 1 {
                return Ok((SliceData::load_bitstring(path)?, None, key)); // problem
            }
            bit_len -= label.remaining_bits() + 1;
            label = LabelReader::read_label(&mut cursor, bit_len)?;
            path.checked_append_references_and_data(&label)?;
        }
    }
}

// phm_edge#_ {n:#} {X:Type} {l:#} {m:#} label:(HmLabel ~l n)
// {n = (~m) + l} node:(PfxHashmapNode m X) = PfxHashmap n X;
// phmn_leaf$0 {n:#} {X:Type} value:X = PfxHashmapNode n X;
// phmn_fork$1 {n:#} {X:Type} left:^(PfxHashmap n X)
// right:^(PfxHashmap n X) = PfxHashmapNode (n+1) X;
impl HashmapType for PfxHashmapE {
    fn check_key(bit_len: usize, key: &SliceData) -> bool {
        bit_len >= key.remaining_bits()
    }
    fn make_cell_with_label(key: &SliceData, max: usize) -> Result<BuilderData> {
        let mut builder = hm_label(key, max)?;
        builder.append_bit_one()?;
        Ok(builder)
    }
    fn make_cell_with_label_and_data(
        key: &SliceData,
        max: usize,
        is_leaf: bool,
        data: &SliceData,
    ) -> Result<BuilderData> {
        let mut builder = hm_label(key, max)?;
        builder.append_bit_bool(!is_leaf)?;
        builder.checked_append_references_and_data(data)?;
        Ok(builder)
    }
    fn make_cell_with_label_and_builder(
        key: &SliceData,
        max: usize,
        is_leaf: bool,
        data: &BuilderData,
    ) -> Result<BuilderData> {
        let mut builder = hm_label(key, max)?;
        builder.append_bit_bool(!is_leaf)?;
        builder.append_builder(data)?;
        Ok(builder)
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
        remainder.append_bit_one()?;
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
        builder.append_bit_zero()?;
        Ok(builder)
    }
    fn is_fork(slice: &mut SliceData) -> Result<bool> {
        Ok(slice.get_next_bit()? && slice.remaining_references() > 1)
    }
    fn is_leaf(slice: &mut SliceData) -> bool {
        matches!(slice.get_next_bit_opt(), Some(0))
    }
    fn inner(self) -> Option<Cell> {
        self.data
    }
    fn data(&self) -> Option<&Cell> {
        self.data.as_ref()
    }
    fn data_mut(&mut self) -> &mut Option<Cell> {
        &mut self.data
    }
    fn bit_len(&self) -> usize {
        self.bit_len
    }
}

impl HashmapRemover for PfxHashmapE {}

#[cfg(test)]
#[path = "tests/test_pfxhashmap.rs"]
mod tests;
