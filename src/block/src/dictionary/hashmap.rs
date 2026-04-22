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
use super::*;
use crate::{
    cell::{BuilderData, Cell, SliceData},
    error::{ExceptionCode, Result},
    fail, Deserializable, GasConsumer, Serializable,
};
use std::fmt;

///////////////////////////////////////////////
/// Length of key should not exceed bit_len
/// If key length is less than bit_len it should be filled by zeros on the left <- TODO:
///
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashmapE {
    bit_len: usize,
    data: Option<Cell>,
}

impl fmt::Display for HashmapE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.data() {
            Some(cell) => write!(f, "Hashmap: {}", cell),
            None => write!(f, "Empty Hashmap"),
        }
    }
}

impl HashmapE {
    /// constructs with bit_len
    pub const fn with_bit_len(bit_len: usize) -> Self {
        Self::with_hashmap(bit_len, None)
    }
    /// construct with bit_len and root representing Hashmap
    pub const fn with_hashmap(bit_len: usize, data: Option<Cell>) -> Self {
        Self { bit_len, data }
    }
    /// serialize not empty root in current cell
    pub fn write_hashmap_root(&self, cell: &mut BuilderData) -> Result<()> {
        match self.data() {
            Some(root) => {
                cell.checked_append_references_and_data(&SliceData::load_cell_ref(root)?)?;
                Ok(())
            }
            None => fail!(ExceptionCode::CellUnderflow),
        }
    }
    /// deserialize not empty root
    pub fn read_hashmap_root(&mut self, slice: &mut SliceData) -> Result<()> {
        let mut root = slice.clone();
        let label = LabelReader::read_label(slice, self.bit_len)?;
        if label.remaining_bits() != self.bit_len {
            slice.shrink_references(2..)?;
            root.shrink_by_remainder(slice);
        } else {
            // all remainded slice as single item
            slice.clear_all_bits();
            slice.clear_all_references();
        }
        self.data = Some(root.into_cell()?);
        Ok(())
    }
    /// checks if dictionary is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_none()
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
    pub fn add_with_gas(
        &mut self,
        key: SliceData,
        value: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, &value.as_builder()?, gas_consumer, ADD)
    }
    pub fn add_builder_with_gas(
        &mut self,
        key: SliceData,
        value: &BuilderData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_set_with_mode(key, value, gas_consumer, ADD)
    }
    /// sets value as reference
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
    pub fn addref_with_gas(
        &mut self,
        key: SliceData,
        value: Cell,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Leaf {
        self.hashmap_setref_with_mode(key, value, gas_consumer, ADD)
    }
    /// gets next/this or previous leaf
    pub fn find_leaf(
        &self,
        key: SliceData,
        next: bool,
        eq: bool,
        signed_int: bool,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<(BuilderData, SliceData)>> {
        Self::check_key_fail(self.bit_len, &key)?;
        match self.data() {
            Some(root) => {
                let mut path = BuilderData::new();
                let next_index = match next {
                    true => 0,
                    false => 1,
                };
                let result = find_leaf::<Self>(
                    root.clone(),
                    &mut path,
                    self.bit_len,
                    key,
                    next_index,
                    eq,
                    signed_int,
                    gas_consumer,
                )?;
                Ok(result.map(|value| (path, value)))
            }
            None => Ok(None),
        }
    }
    /// removes item
    pub fn remove(&mut self, key: SliceData) -> Leaf {
        self.hashmap_remove(key, &mut 0)
    }
    /// removes item spending gas
    pub fn remove_with_gas(&mut self, key: SliceData, gas_consumer: &mut dyn GasConsumer) -> Leaf {
        self.hashmap_remove(key, gas_consumer)
    }
    /// gets item with minimal key
    pub fn get_min(
        &self,
        signed: bool,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<(BuilderData, SliceData)>> {
        self.get_min_max(true, signed, gas_consumer)
    }
    /// gets item with maxiaml key
    pub fn get_max(
        &self,
        signed: bool,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<(BuilderData, SliceData)>> {
        self.get_min_max(false, signed, gas_consumer)
    }
    /// gets item with minimal or maxiaml key
    pub fn get_min_max(
        &self,
        min: bool,
        signed: bool,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Option<(BuilderData, SliceData)>> {
        match self.data() {
            Some(root) => {
                let mut path = BuilderData::new();
                let (next_index, index) = match (min, signed) {
                    (true, true) => (0, 1),
                    (true, false) => (0, 0),
                    (false, true) => (1, 0),
                    (false, false) => (1, 1),
                };
                let result = get_min_max::<Self>(
                    root.clone(),
                    &mut path,
                    self.bit_len,
                    next_index,
                    index,
                    gas_consumer,
                )?;
                Ok(result.map(|value| (path, value)))
            }
            None => Ok(None),
        }
    }
    /// split to subtrees by key
    pub fn split(&self, key: &SliceData) -> Result<(Self, Self)> {
        let (left, right) = self.hashmap_split(key)?;
        let left = Self::with_hashmap(self.bit_len, left);
        let right = Self::with_hashmap(self.bit_len, right);
        Ok((left, right))
    }
    /// Merge other tree to current roots should be at least merge key
    pub fn merge(&mut self, other: &Self, key: &SliceData) -> Result<()> {
        self.hashmap_merge(other, key)
    }

    // /// returns subtree by prefix with same bit_len
    // pub fn subtree_with_prefix(&self, prefix: &SliceData, gas_consumer: &mut dyn GasConsumer) -> Result<Self> {
    //     self.subtree_by_prefix(prefix, gas_consumer)
    // }

    /// returns subtree by prefix with shorted bit_len by prefix length
    pub fn subtree_without_prefix(
        &self,
        prefix: &SliceData,
        gas_consumer: &mut dyn GasConsumer,
    ) -> Result<Self> {
        if prefix.is_empty_bitstring() {
            return Ok(self.clone());
        }
        let prefix_len = prefix.remaining_bits();
        if let Some((key, mut remainder, None)) = self.subtree_root(prefix, gas_consumer)? {
            let bit_len = self.bit_len() - prefix_len;
            let mut label = SliceData::load_bitstring(key)?;
            label.move_by(prefix_len)?;
            let is_leaf = Self::is_leaf(&mut remainder);
            let root = Self::make_cell_with_label_and_data(&label, bit_len, is_leaf, &remainder)?;
            Ok(HashmapE::with_hashmap(bit_len, Some(gas_consumer.finalize_cell(root)?)))
        } else {
            Ok(HashmapE::with_bit_len(self.bit_len() - prefix_len))
        }
    }
}

impl Serializable for HashmapE {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.write_hashmap_data(cell)?;
        Ok(())
    }
}

impl Deserializable for HashmapE {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.read_hashmap_data(slice)?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        if slice.get_next_bit()? {
            slice.checked_drain_reference()?;
        }
        Ok(())
    }
}

// hm_edge#_ {n:#} {X:Type} {l:#} {m:#} label:(HmLabel ~l n)
// {n = (~m) + l} node:(HashmapNode m X) = Hashmap n X;
// hmn_leaf#_ {X:Type} value:X = HashmapNode 0 X;
// hmn_fork#_ {n:#} {X:Type} left:^(Hashmap n X)
// right:^(Hashmap n X) = HashmapNode (n+1) X;
impl HashmapType for HashmapE {
    fn check_key(bit_len: usize, key: &SliceData) -> bool {
        bit_len == key.remaining_bits()
    }
    fn make_cell_with_label_and_data(
        key: &SliceData,
        max: usize,
        _is_leaf: bool,
        data: &SliceData,
    ) -> Result<BuilderData> {
        let mut builder = hm_label(key, max)?;
        builder.checked_append_references_and_data(data)?;
        Ok(builder)
    }
    fn is_fork(slice: &mut SliceData) -> Result<bool> {
        Ok(slice.remaining_references() > 1)
    }
    fn is_leaf(_slice: &mut SliceData) -> bool {
        true
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

impl HashmapRemover for HashmapE {}
impl HashmapSubtree for HashmapE {}

impl IntoIterator for &HashmapE {
    type Item = <HashmapIterator<HashmapE> as std::iter::Iterator>::Item;
    type IntoIter = HashmapIterator<HashmapE>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[macro_export]
macro_rules! define_HashmapE {
    ( $varname:ident, $bit_len:expr, $x_type:ty ) => {
        #[derive(PartialEq, Clone, Debug, Eq)]
        pub struct $varname($crate::HashmapE);

        #[allow(dead_code)]
        impl $varname {
            /// default constructor
            pub const fn new() -> Self {
                Self($crate::HashmapE::with_hashmap($bit_len, None))
            }
            /// clear data
            pub fn clear(&mut self) {
                self.0 = $crate::HashmapE::with_bit_len($bit_len);
            }
            /// constructor with HashmapE root
            pub const fn with_hashmap(data: Option<Cell>) -> Self {
                Self($crate::HashmapE::with_hashmap($bit_len, data))
            }
            /// constructor with single element
            pub fn with_key_and_value<K: Serializable>(key: &K, value: &$x_type) -> Result<Self> {
                let mut hashmap = Self::default();
                hashmap.set(key, value)?;
                Ok(hashmap)
            }
            pub fn root(&self) -> Option<&Cell> {
                $crate::HashmapType::data(&self.0)
            }
            pub fn inner(self) -> $crate::HashmapE {
                self.0
            }
            /// Used for not empty Hashmaps
            pub fn read_hashmap_root(&mut self, slice: &mut SliceData) -> Result<()> {
                self.0.read_hashmap_root(slice)
            }
            /// Used for not empty Hashmaps
            pub fn write_hashmap_root(&self, cell: &mut BuilderData) -> Result<()> {
                self.0.write_hashmap_root(cell)
            }
            /// Return true if no items
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
            /// Calculates length
            pub fn len(&self) -> Result<usize> {
                $crate::HashmapType::len(&self.0)
            }
            pub fn count(&self, max: usize) -> Result<usize> {
                $crate::HashmapType::count(&self.0, max)
            }
            /// iterates items
            pub fn iterate<F>(&self, mut p: F) -> Result<bool>
            where
                F: FnMut($x_type) -> Result<bool>,
            {
                $crate::HashmapType::iterate_slices(&self.0, |_, ref mut slice| {
                    p(<$x_type>::construct_from(slice)?)
                })
            }
            /// iterates items as raw slices
            pub fn iterate_slices<F>(&self, mut p: F) -> Result<bool>
            where
                F: FnMut(SliceData) -> Result<bool>,
            {
                $crate::HashmapType::iterate_slices(&self.0, |_, slice| p(slice))
            }
            /// iterates keys
            pub fn iterate_keys<K, F>(&self, mut p: F) -> Result<bool>
            where
                K: Default + Deserializable,
                F: FnMut(K) -> Result<bool>,
            {
                $crate::HashmapType::iterate_slices(&self.0, |ref mut key, _| {
                    p(K::construct_from(key)?)
                })
            }
            /// iterates items with keys
            pub fn iterate_with_keys<K, F>(&self, mut p: F) -> Result<bool>
            where
                K: Default + Deserializable,
                F: FnMut(K, $x_type) -> Result<bool>,
            {
                $crate::HashmapType::iterate_slices(&self.0, |ref mut key, ref mut slice| {
                    p(K::construct_from(key)?, <$x_type>::construct_from(slice)?)
                })
            }
            /// iterates items as slices with keys
            pub fn iterate_slices_with_keys<F>(&self, mut p: F) -> Result<bool>
            where
                F: FnMut(SliceData, SliceData) -> Result<bool>,
            {
                $crate::HashmapType::iterate_slices(&self.0, |key, slice| p(key, slice))
            }
            pub fn set_raw(
                &mut self,
                key: SliceData,
                value: &BuilderData,
            ) -> Result<Option<SliceData>> {
                self.0.set_builder(key, &value)
            }
            pub fn set<K: Serializable>(&mut self, key: &K, value: &$x_type) -> Result<()> {
                let key = key.write_to_bitstring()?;
                let value = value.write_to_new_cell()?;
                self.0.set_builder(key, &value)?;
                Ok(())
            }
            pub fn set_with_return<K: Serializable>(
                &mut self,
                key: &K,
                value: &$x_type,
            ) -> Result<Option<SliceData>> {
                let key = key.write_to_bitstring()?;
                let value = value.write_to_new_cell()?;
                self.0.set_builder(key, &value)
            }
            pub fn setref<K: Serializable>(&mut self, key: &K, value: Cell) -> Result<()> {
                let key = key.write_to_bitstring()?;
                self.0.setref(key, value)?;
                Ok(())
            }
            pub fn add_key_serialized(&mut self, key: SliceData) -> Result<()> {
                let value = BuilderData::default();
                self.0.set_builder(key, &value)?;
                Ok(())
            }
            pub fn add_key<K: Serializable>(&mut self, key: &K) -> Result<()> {
                let key = key.write_to_bitstring()?;
                self.add_key_serialized(key)
            }
            pub fn multiset<I>(&mut self, iter: I) -> Result<()>
            where
                I: Iterator<Item = (SliceData, Option<SliceData>)>,
            {
                $crate::HashmapType::hashmap_multiset(&mut self.0, iter)?;
                Ok(())
            }
            pub fn get<K: Serializable>(&self, key: &K) -> Result<Option<$x_type>> {
                self.get_as_slice(key)?
                    .map(|ref mut slice| <$x_type>::construct_from(slice))
                    .transpose()
            }
            pub fn get_as_slice<K: Serializable>(&self, key: &K) -> Result<Option<SliceData>> {
                let key = key.write_to_bitstring()?;
                self.get_raw(key)
            }
            pub fn get_ref<K: Serializable>(&self, key: &K) -> Result<Option<Cell>> {
                Ok(self.get_as_slice(key)?.and_then(|slice| slice.reference_opt(0)))
            }
            pub fn get_raw(&self, key: SliceData) -> Result<Option<SliceData>> {
                self.0.get(key)
            }
            pub fn contains<K: Serializable>(&self, key: &K) -> Result<bool> {
                Ok(self.get_as_slice(key)?.is_some())
            }
            pub fn remove<K: Serializable>(&mut self, key: &K) -> Result<bool> {
                let key = key.write_to_bitstring()?;
                let leaf = self.0.remove(key)?;
                Ok(leaf.is_some())
            }
            pub fn remove_with_return<K: Serializable>(
                &mut self,
                key: &K,
            ) -> Result<Option<$x_type>> {
                let key = key.write_to_bitstring()?;
                let Some(mut leaf) = self.0.remove(key)? else {
                    return Ok(None);
                };
                <$x_type>::construct_from(&mut leaf).map(Some)
            }
            pub fn retire<F>(&mut self, mut f: F) -> Result<$crate::HashmapFilterResult>
            where
                F: FnMut($x_type) -> bool,
            {
                self.filter(|_, value| {
                    if f(value) {
                        Ok($crate::HashmapFilterResult::Accept)
                    } else {
                        Ok($crate::HashmapFilterResult::Remove)
                    }
                })
            }
            pub fn check_key<K: Serializable>(&self, key: &K) -> Result<bool> {
                let key = key.write_to_bitstring()?;
                self.0.get(key).map(|value| value.is_some())
            }
            pub fn export_vector(&self) -> Result<Vec<$x_type>> {
                let mut vec = Vec::new();
                $crate::HashmapType::iterate_slices(&self.0, |_, ref mut slice| {
                    vec.push(<$x_type>::construct_from(slice)?);
                    Ok(true)
                })?;
                Ok(vec)
            }
            pub fn merge(&mut self, other: &Self, split_key: &SliceData) -> Result<()> {
                self.0.merge(&other.0, split_key)
            }
            pub fn split(&self, split_key: &SliceData) -> Result<(Self, Self)> {
                self.0.split(split_key).map(|(left, right)| (Self(left), Self(right)))
            }
            pub fn combine_with(&mut self, other: &Self) -> Result<bool> {
                $crate::HashmapType::combine_with(&mut self.0, &other.0)
            }
            pub fn scan_diff<K, F>(&self, other: &Self, mut op: F) -> Result<bool>
            where
                K: Deserializable,
                F: FnMut(K, Option<$x_type>, Option<$x_type>) -> Result<bool>,
            {
                $crate::HashmapType::scan_diff(&self.0, &other.0, |mut key, value1, value2| {
                    let key = K::construct_from(&mut key)?;
                    let value1 =
                        value1.map(|ref mut slice| <$x_type>::construct_from(slice)).transpose()?;
                    let value2 =
                        value2.map(|ref mut slice| <$x_type>::construct_from(slice)).transpose()?;
                    op(key, value1, value2)
                })
            }

            // removes items from hashamp in one pass
            // closure must return decision for item to remove it or to accept it
            pub fn filter<F>(&mut self, mut func: F) -> Result<$crate::HashmapFilterResult>
            where
                F: FnMut(&$crate::BuilderData, $x_type) -> Result<$crate::HashmapFilterResult>,
            {
                $crate::HashmapRemover::hashmap_filter(&mut self.0, |key, mut value| {
                    let value = $crate::Deserializable::construct_from(&mut value)?;
                    func(key, value)
                })
            }

            // removes items from hashamp in one pass if predicate returns true
            // closure must return decision for item to remove it or to accept it
            pub fn filter_all_values<F>(
                &mut self,
                mut func: F,
                mut limit: Option<usize>,
            ) -> Result<$crate::HashmapFilterResult>
            where
                F: FnMut(&$x_type) -> bool,
            {
                $crate::HashmapRemover::hashmap_filter(&mut self.0, |_, ref mut value| {
                    let value = $crate::Deserializable::construct_from(value)?;
                    if func(&value) {
                        if let Some(limit) = limit.as_mut() {
                            if *limit == 0 {
                                return Ok($crate::HashmapFilterResult::Cancel);
                            }
                            *limit -= 1;
                        }
                        Ok($crate::HashmapFilterResult::Remove)
                    } else {
                        Ok($crate::HashmapFilterResult::Accept)
                    }
                })
            }

            pub fn export_keys<K: Deserializable + Eq + std::hash::Hash>(
                &self,
            ) -> Result<std::collections::HashSet<K>> {
                let mut keys = std::collections::HashSet::new();
                self.iterate_keys(|key: K| {
                    keys.insert(key);
                    Ok(true)
                })?;
                Ok(keys)
            }

            pub fn find_leaf<K: Deserializable + Serializable>(
                &self,
                key: &K,
                next: bool,
                eq: bool,
                signed_int: bool,
            ) -> Result<Option<(K, $x_type)>> {
                let key = key.write_to_bitstring()?;
                if let Some((k, mut v)) = self.0.find_leaf(key, next, eq, signed_int, &mut 0)? {
                    // BuilderData, SliceData
                    let key = K::construct_from_cell(k.into_cell()?)?;
                    let value = <$x_type>::construct_from(&mut v)?;
                    Ok(Some((key, value)))
                } else {
                    Ok(None)
                }
            }
            pub fn find_min_max_raw(
                &self,
                min: bool,
                signed: bool,
            ) -> Result<Option<(BuilderData, SliceData)>> {
                self.0.get_min_max(min, signed, &mut 0)
            }
            pub fn find_min_max_key<K: Deserializable + Serializable>(
                &self,
                min: bool,
                signed: bool,
            ) -> Result<Option<K>> {
                self.0
                    .get_min_max(min, signed, &mut 0)?
                    .map(|(key, _)| K::construct_from_bitstring(key))
                    .transpose()
            }
        }

        impl Default for $varname {
            fn default() -> Self {
                $varname($crate::HashmapE::with_bit_len($bit_len))
            }
        }

        impl Serializable for $varname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                self.0.write_to(cell)
            }
        }

        impl Deserializable for $varname {
            fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
                self.0.read_from(slice)
            }
            fn skip(slice: &mut SliceData) -> Result<()> {
                $crate::HashmapE::skip(slice)
            }
        }
    };
}

#[cfg(test)]
#[path = "tests/test_hashmap.rs"]
mod tests;
