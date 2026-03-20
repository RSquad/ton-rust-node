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
    error::BlockError, fail, BuilderData, Cell, Deserializable, ExceptionCode, HashmapType,
    IBitstring, LabelReader, Leaf, Result, Serializable, SliceData,
};
use std::cmp::Ordering;

/// trait for types used as Augment to calc aug on forks
pub trait Augmentable: Clone + Default + Serializable + Deserializable {
    fn calc(&mut self, other: &Self) -> Result<bool>;
}
/// trait for objects in hashmap to help get augmentation from object
pub trait Augmentation<Y: Augmentable> {
    fn aug(&self) -> Result<Y>;
}
pub type AugResult<Y> = Result<(Option<SliceData>, Y)>;

/// How to continue hashmap's traverse operation
pub enum TraverseNextStep<R> {
    /// Continue traverse to the "0", "1" or both branches
    VisitZero,
    VisitOne,
    VisitZeroOne,
    VisitOneZero,
    /// Stop traverse current branch
    Stop,
    /// End traverse and return given result from traverse function
    End(R),
}

///////////////////////////////////////////////
/// Length of key should not exceed bit_len
///
#[macro_export]
macro_rules! define_HashmapAugE {
    ( $varname:ident, $bit_len:expr, $k_type:ty, $x_type:ty, $y_type:ty ) => {
        #[derive(Clone, Debug, Eq, PartialEq)] // cannot Default
        pub struct $varname {
            extra: $y_type,
            data: Option<Cell>,
        }

        impl $varname {
            /// Dumps hashmap contents
            pub fn dump(&self) {
                $crate::HashmapType::iterate_slices(self, |ref mut key, ref mut value| {
                    dbg!(<$k_type>::construct_from(key).unwrap());
                    dbg!(<$x_type>::construct_from(value).unwrap());
                    dbg!(<$y_type>::construct_from(value).unwrap());
                    Ok(true)
                })
                .unwrap();
            }
            /// Constructs new HashmapAugE for bit_len keys
            pub fn new() -> Self {
                Self::default()
            }
            /// Constructs from cell, extracts total aug
            pub fn with_hashmap(data: Option<Cell>) -> Result<Self> {
                let extra = match data {
                    Some(ref root) => Self::find_extra(root, $bit_len)?,
                    None => <$y_type>::default(),
                };
                Ok(Self { extra, data })
            }
            /// split map by key
            pub fn split(&self, key: &SliceData) -> Result<(Self, Self)> {
                let (left, right) = $crate::HashmapType::hashmap_split(self, key)?;
                Ok((Self::with_hashmap(left)?, Self::with_hashmap(right)?))
            }
            /// merge maps
            pub fn merge(&mut self, other: &Self, key: &SliceData) -> Result<()> {
                if $bit_len != $crate::HashmapType::bit_len(other)
                    || key.remaining_bits() > $bit_len
                {
                    fail!("data in hashmaps do not correspond each other or key too long")
                }
                if self.data.is_none() {
                    self.data = other.data.clone();
                    self.set_root_extra(other.extra.clone());
                } else {
                    $crate::Augmentable::calc(&mut self.extra, &other.extra)?;
                    $crate::HashmapType::hashmap_merge(self, other, key)?;
                }
                Ok(())
            }
            /// split hashmap by decision function
            pub fn filter_with_split<F>(&mut self, mut func: F) -> Result<Self>
            where
                F: FnMut(
                    &$crate::BuilderData,
                    $x_type,
                    $y_type,
                ) -> Result<$crate::HashmapFilterSplitResult>,
            {
                $crate::HashmapRemover::hashmap_filter_split(self, |key, mut value| {
                    let aug = $crate::Deserializable::construct_from(&mut value)?;
                    let value = $crate::Deserializable::construct_from(&mut value)?;
                    func(key, value, aug)
                })
            }

            // removes items from hashamp in one pass
            // closure must return decision for item to accept it or to remove it
            pub fn filter<F>(&mut self, mut func: F) -> Result<$crate::HashmapFilterResult>
            where
                F: FnMut(
                    &$crate::BuilderData,
                    $x_type,
                    $y_type,
                ) -> Result<$crate::HashmapFilterResult>,
            {
                $crate::HashmapRemover::hashmap_filter(self, |key, mut value| {
                    let aug = $crate::Deserializable::construct_from(&mut value)?;
                    let value = $crate::Deserializable::construct_from(&mut value)?;
                    func(key, value, aug)
                })
            }

            pub fn del(&mut self, key: &$k_type) -> Result<()> {
                let key = $crate::Serializable::write_to_bitstring(key)?;
                $crate::HashmapRemover::hashmap_remove(self, key, &mut 0)?;
                Ok(())
            }

            /// Find leaf with key >= given key (next=true) or <= given key (next=false)
            /// eq=true includes exact match, eq=false excludes it
            pub fn find_leaf(
                &self,
                key: &$k_type,
                next: bool,
                eq: bool,
                signed_int: bool,
            ) -> Result<Option<($k_type, $x_type)>> {
                let key = $crate::Serializable::write_to_bitstring(key)?;
                match self.data.as_ref() {
                    Some(root) => {
                        let mut path = $crate::BuilderData::new();
                        let next_index = if next { 0 } else { 1 };
                        let result = $crate::find_leaf::<Self>(
                            root.clone(),
                            &mut path,
                            $bit_len,
                            key,
                            next_index,
                            eq,
                            signed_int,
                            &mut 0,
                        )?;
                        match result {
                            Some(mut slice) => {
                                let found_key = <$k_type>::construct_from_cell(path.into_cell()?)?;
                                // Skip aug, read value
                                <$y_type>::skip(&mut slice)?;
                                let value = <$x_type>::construct_from(&mut slice)?;
                                Ok(Some((found_key, value)))
                            }
                            None => Ok(None),
                        }
                    }
                    None => Ok(None),
                }
            }
        }

        // hm_edge#_ {n:#} {X:Type} {l:#} {m:#} label:(HmLabel ~l n)
        // {n = (~m) + l} node:(HashmapAugNode m X) = HashmapAug n X;
        // hmn_leaf#_ {X:Type} value:X = HashmapAugNode 0 X;
        // hmn_fork#_ {n:#} {X:Type} left:^(HashmapAug n X)
        // right:^(HashmapAug n X) = HashmapAugNode (n+1) X;
        impl $crate::HashmapType for $varname {
            fn check_key(bit_len: usize, key: &SliceData) -> bool {
                bit_len == key.remaining_bits()
            }
            fn make_cell_with_label_and_data(
                key: &SliceData,
                max: usize,
                _is_leaf: bool,
                data: &SliceData,
            ) -> Result<BuilderData> {
                let mut builder = $crate::hm_label(key, max)?;
                builder.checked_append_references_and_data(data)?;
                Ok(builder)
            }
            fn make_cell_with_label_and_builder(
                key: &SliceData,
                max: usize,
                _is_leaf: bool,
                data: &BuilderData,
            ) -> Result<BuilderData> {
                let mut builder = $crate::hm_label(key, max)?;
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
                let next_bit_len = bit_len
                    .checked_sub(key.remaining_bits() + 1)
                    .ok_or_else(|| error!("fail too short label"))?;
                let mut builder = Self::make_cell_with_label(key, bit_len)?;
                let aug = Self::calc_extra(&left, &right, next_bit_len)?;
                let mut remainder = BuilderData::new();
                if swap {
                    std::mem::swap(&mut left, &mut right);
                }
                remainder.checked_append_reference(left)?;
                remainder.checked_append_reference(right)?;
                aug.write_to(&mut remainder)?;
                builder.append_builder(&remainder)?;
                Ok((builder, remainder))
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
                $bit_len
            }
        }

        impl $crate::HashmapAugType<$k_type, $x_type, $y_type> for $varname {
            fn root_extra(&self) -> &$y_type {
                &self.extra
            }
            fn set_root_extra(&mut self, aug: $y_type) {
                self.extra = aug;
            }
        }

        impl $crate::HashmapRemover for $varname {
            fn after_remove(&mut self) -> Result<()> {
                let aug = match &self.data {
                    Some(root) => Self::find_extra(root, $bit_len)?,
                    None => <$y_type>::default(),
                };
                self.set_root_extra(aug);
                Ok(())
            }
        }

        impl $varname {
            /// scans differences in two hashmaps
            pub fn scan_diff_with_aug<F>(&self, other: &Self, mut op: F) -> Result<bool>
            where
                F: FnMut(
                    $k_type,
                    Option<($x_type, $y_type)>,
                    Option<($x_type, $y_type)>,
                ) -> Result<bool>,
            {
                $crate::HashmapType::scan_diff(
                    self,
                    other,
                    |mut key, mut value_aug1, mut value_aug2| {
                        let key = <$k_type>::construct_from(&mut key)?;
                        let value_aug1 = value_aug1.as_mut().map(Self::value_aug).transpose()?;
                        let value_aug2 = value_aug2.as_mut().map(Self::value_aug).transpose()?;
                        op(key, value_aug1, value_aug2)
                    },
                )
            }
            pub fn scan_diff_with_default<F>(&self, other: &Self, mut op: F) -> Result<bool>
            where
                F: FnMut(SliceData, $x_type, $x_type) -> Result<bool>,
            {
                $crate::HashmapType::scan_diff(
                    self,
                    other,
                    |key, mut value_aug1, mut value_aug2| {
                        let value1 = value_aug1.as_mut().map(Self::value_skip_aug).transpose()?;
                        let value2 = value_aug2.as_mut().map(Self::value_skip_aug).transpose()?;
                        op(key, value1.unwrap_or_default(), value2.unwrap_or_default())
                    },
                )
            }
        }
        impl Default for $varname {
            fn default() -> Self {
                Self { extra: <$y_type>::default(), data: None }
            }
        }

        impl $crate::Serializable for $varname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                if let Some(root) = &self.data {
                    cell.append_bit_one()?;
                    cell.checked_append_reference(root.clone())?;
                } else {
                    cell.append_bit_zero()?;
                }
                self.root_extra().write_to(cell)?;
                Ok(())
            }
        }

        impl $crate::Deserializable for $varname {
            fn construct_from(slice: &mut SliceData) -> Result<Self> {
                let data = match slice.get_next_bit()? {
                    true => Some(slice.checked_drain_reference()?),
                    false => None,
                };
                let extra = <$y_type>::construct_from(slice)?;
                if data.is_none() && extra != <$y_type>::default() {
                    fail!(
                        "root extra for empty HashmapAugE {} is not default",
                        std::any::type_name::<Self>()
                    )
                }
                Ok(Self { extra, data })
            }
        }

        impl fmt::Display for $varname {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                match &self.data {
                    Some(cell) => write!(f, "HashmapAug: {}", cell),
                    None => write!(f, "Empty HashmapAug"),
                }
            }
        }
    };
}

pub trait HashmapAugType<
    K: Deserializable + Serializable,
    X: Deserializable + Serializable + Augmentation<Y>,
    Y: Augmentable,
>: HashmapType
{
    fn root_extra(&self) -> &Y;
    fn set_root_extra(&mut self, aug: Y);
    fn update_root_extra(&mut self) -> Result<&Y> {
        let aug = match self.data() {
            Some(root) => Self::find_extra(root, self.bit_len())?,
            None => Y::default(),
        };
        self.set_root_extra(aug);
        Ok(self.root_extra())
    }
    fn value_aug(slice: &mut SliceData) -> Result<(X, Y)> {
        let aug = Y::construct_from(slice)?;
        let val = X::construct_from(slice)?;
        Ok((val, aug))
    }
    fn value_skip_aug(slice: &mut SliceData) -> Result<X> {
        Y::skip(slice)?;
        let val = X::construct_from(slice)?;
        Ok(val)
    }
    /// TODO unused code?
    //fn aug_value(slice: &mut SliceData) -> Result<(Y, X)> {
    //    let aug = Y::construct_from(slice)?;
    //    let val = X::construct_from(slice)?;
    //    Ok((aug, val))
    //}
    //fn key_value_aug(key: BuilderData, mut slice: SliceData) -> Result<(K, X, Y)> {
    //    let key = K::construct_from_cell(key.into_cell()?)?;
    //    let (val, aug) = Self::value_aug(&mut slice)?;
    //    Ok((key, val, aug))
    //}
    fn contains(&self, key: &K) -> Result<bool> {
        let key = key.write_to_bitstring()?;
        Ok(self.get_serialized_raw(key)?.is_some())
    }
    fn get_serialized_raw(&self, key: SliceData) -> Leaf {
        self.hashmap_get(key, &mut 0)
    }
    fn get_serialized_as_slice(&self, key: SliceData) -> Result<Option<SliceData>> {
        self.get_serialized_raw(key)?
            .map(|mut slice| {
                Y::skip(&mut slice)?;
                Ok(slice)
            })
            .transpose()
    }
    fn get_serialized(&self, key: SliceData) -> Result<Option<X>> {
        self.get_serialized_as_slice(key)?
            .map(|mut slice| X::construct_from(&mut slice))
            .transpose()
    }
    fn get_serialized_with_aug(&self, key: SliceData) -> Result<Option<(X, Y)>> {
        self.get_serialized_raw(key)?
            .map(|mut slice| {
                let aug = Y::construct_from(&mut slice)?;
                Ok((X::construct_from(&mut slice)?, aug))
            })
            .transpose()
    }
    /// gets aug and item in combined slice
    fn get_raw(&self, key: &K) -> Leaf {
        let key = key.write_to_bitstring()?;
        self.get_serialized_raw(key)
    }
    /// get item as slice
    fn get_as_slice(&self, key: &K) -> Leaf {
        self.get_raw(key)?
            .map(|mut slice| {
                Y::skip(&mut slice)?;
                Ok(slice)
            })
            .transpose()
    }
    /// get item and aug
    fn get(&self, key: &K) -> Result<Option<X>> {
        self.get_as_slice(key)?.map(|mut slice| X::construct_from(&mut slice)).transpose()
    }
    /// get item with aug equal to root aug
    fn find_by_root_aug(&self) -> Result<Option<(K, X)>> {
        let mut bit_len = self.bit_len();
        let Some(data) = self.data() else {
            return Ok(None);
        };
        let mut cursor = LabelReader::with_cell(data)?;
        let mut path = cursor.get_label_raw(&mut bit_len, BuilderData::new())?;
        let mut slice = cursor.remainder()?;
        let root_aug_slice = slice.clone();
        while Self::is_fork(&mut slice)? {
            bit_len -= 1;
            let mut max = bit_len;
            let mut cursor = LabelReader::with_cell(&slice.reference(0)?)?;
            let mut path0 = path.clone();
            path0.append_bit_zero()?;
            let path0 = cursor.get_label_raw(&mut max, path0)?;
            let slice0 = cursor.remainder()?;
            if root_aug_slice.compare_bitstrings(&slice0) == Ordering::Equal {
                path = path0;
                slice = slice0;
                bit_len = max;
                continue;
            }
            let mut cursor = LabelReader::with_cell(&slice.reference(1)?)?;
            path.append_bit_one()?;
            path = cursor.get_label_raw(&mut bit_len, path)?;
            slice = cursor.remainder()?;
        }
        let key = K::construct_from_bitstring(path)?;
        Y::skip(&mut slice)?;
        let value = X::construct_from(&mut slice)?;
        Ok(Some((key, value)))
    }
    /// get item as slice and aug
    fn get_as_slice_with_aug(&self, key: &K) -> Result<Option<(SliceData, Y)>> {
        match self.get_raw(key)? {
            Some(mut slice) => {
                let aug = Y::construct_from(&mut slice)?;
                Ok(Some((slice, aug)))
            }
            None => Ok(None),
        }
    }
    /// get item and aug
    fn get_with_aug(&self, key: &K) -> Result<Option<(X, Y)>> {
        match self.get_raw(key)? {
            Some(mut slice) => {
                let aug = Y::construct_from(&mut slice)?;
                Ok(Some((X::construct_from(&mut slice)?, aug)))
            }
            None => Ok(None),
        }
    }
    /// sets item to hashmapaug returning prev value if exists by key
    fn set_return_prev(&mut self, key: &K, value: &X, aug: &Y) -> Result<Option<SliceData>> {
        let (value, _) = self.set_with_prev_and_depth(key, value, aug)?;
        Ok(value)
    }
    /// sets item to hashmapaug returning prev value if exists by key and depth of tree
    fn set_with_prev_and_depth(
        &mut self,
        key: &K,
        value: &X,
        aug: &Y,
    ) -> Result<(Option<SliceData>, usize)> {
        let key = key.write_to_bitstring()?;
        let value = value.write_to_new_cell()?;
        self.set_builder_serialized(key, &value, aug)
    }
    /// sets item to hashmapaug
    fn set(&mut self, key: &K, value: &X, aug: &Y) -> Result<()> {
        self.set_return_prev(key, value, aug)?;
        Ok(())
    }
    /// sets item to hashmapaug, aug automatically calculates by value
    fn set_augmentable(&mut self, key: &K, value: &X) -> Result<()> {
        let key = key.write_to_bitstring()?;
        let aug = value.aug()?;
        let value = value.write_to_new_cell()?;
        self.set_builder_serialized(key, &value, &aug)?;
        Ok(())
    }
    /// sets item to hashmapaug as ref
    fn setref(&mut self, key: &K, cell: Cell, aug: &Y) -> Result<()> {
        let key = key.write_to_bitstring()?;
        let value = BuilderData::with_ref(cell);
        self.set_builder_serialized(key, &value, aug)?;
        Ok(())
    }
    /// multiset items to hashmapaug
    fn multiset<I>(&mut self, iter: I) -> Result<()>
    where
        I: Iterator<Item = (SliceData, Option<SliceData>)>,
    {
        self.hashmap_multiset(iter)?;
        self.update_root_extra()?;
        Ok(())
    }

    fn find_key(&self, min: bool, signed: bool) -> Result<Option<(BuilderData, SliceData)>> {
        match self.data() {
            Some(root) => {
                let mut path = BuilderData::new();
                let (next_index, index) = match (min, signed) {
                    (true, true) => (0, 1),
                    (true, false) => (0, 0),
                    (false, true) => (1, 0),
                    (false, false) => (1, 1),
                };
                let result = crate::get_min_max::<Self>(
                    root.clone(),
                    &mut path,
                    self.bit_len(),
                    next_index,
                    index,
                    &mut 0,
                )?;
                match result {
                    Some(value) => Ok(Some((path, value))),
                    None => Ok(None),
                }
            }
            None => Ok(None),
        }
    }
    /// gets item with minimal key
    fn get_min(&self, signed: bool) -> Result<Option<(K, X)>> {
        match self.find_key(true, signed)? {
            Some((key, mut val)) => {
                let key = K::construct_from_bitstring(key)?;
                Y::skip(&mut val)?;
                let val = X::construct_from(&mut val)?;
                Ok(Some((key, val)))
            }
            None => Ok(None),
        }
    }
    /// gets item with maximal key
    fn get_max(&self, signed: bool) -> Result<Option<(K, X)>> {
        match self.find_key(false, signed)? {
            Some((key, mut val)) => {
                let key = K::construct_from_bitstring(key)?;
                Y::skip(&mut val)?;
                let val = X::construct_from(&mut val)?;
                Ok(Some((key, val)))
            }
            None => Ok(None),
        }
    }
    /// gets item with aug for minimal or maximal key
    fn get_minmax(&self, min: bool, signed: bool) -> Result<Option<(K, X, Y)>> {
        match self.find_key(min, signed)? {
            Some((key, mut val)) => {
                let key = K::construct_from_bitstring(key)?;
                let aug = Y::construct_from(&mut val)?;
                let val = X::construct_from(&mut val)?;
                Ok(Some((key, val, aug)))
            }
            None => Ok(None),
        }
    }
    /// gets item with aug for minimal or maximal key
    fn get_minmax_key(&self, min: bool, signed: bool) -> Result<Option<K>> {
        match self.find_key(min, signed)? {
            Some((key, _)) => {
                let key = K::construct_from_bitstring(key)?;
                Ok(Some(key))
            }
            None => Ok(None),
        }
    }

    // /// Checks if HashmapAugE is empty
    // fn is_empty(&self) -> bool {
    //     self.data().is_none()
    // }
    /// Serialization HashmapAug root of HashmapAugE to BuilderData - just append
    fn write_hashmap_root(&self, cell: &mut BuilderData) -> Result<()> {
        if let Some(root) = self.data() {
            cell.checked_append_references_and_data(&SliceData::load_cell_ref(root)?)?;
            Ok(())
        } else {
            fail!("no reference in HashmapAug with bit len {}", self.bit_len())
        }
    }
    /// deserialize not empty root
    fn read_hashmap_root(&mut self, slice: &mut SliceData) -> Result<()> {
        let mut root = slice.clone(); // copy to get as data
        let label = LabelReader::read_label(slice, self.bit_len())?;
        if label.remaining_bits() != self.bit_len() {
            // fork
            slice.shrink_references(2..); // left, right
            self.set_root_extra(Y::construct_from(slice)?);
        } else {
            // single leaf as root
            self.set_root_extra(Y::construct_from(slice)?);
            let mut value = X::default();
            value.read_from(slice)?;
        }
        root.shrink_by_remainder(slice);
        *self.data_mut() = Some(root.into_cell()?);
        Ok(())
    }
    /// return object slice if it is single in hashmap
    fn single(&self) -> Result<Option<SliceData>> {
        if let Some(root) = self.data() {
            let mut slice = SliceData::load_cell_ref(root)?;
            let label =
                LabelReader::read_label_raw(&mut slice, &mut self.bit_len(), Default::default())?;
            if label.length_in_bits() == self.bit_len() {
                Y::skip(&mut slice)?;
                return Ok(Some(slice));
            }
        }
        Ok(None)
    }
    /// return object if it is single in hashmap
    fn single_value(&self) -> Result<Option<X>> {
        self.single()?.map(|ref mut slice| X::construct_from(slice)).transpose()
    }
    fn concurent_len(&self, other: &Self, total: usize) -> Result<(usize, usize)> {
        if self.bit_len() != other.bit_len() {
            fail!(
                "cannot calculate concurent len for hashmaps with different bit_len: {} and {}",
                self.bit_len(),
                other.bit_len()
            );
        }
        let Some(data0) = self.data() else {
            return Ok((0, total));
        };
        let Some(data1) = other.data() else {
            return Ok((total, 0));
        };
        super::hashmap_concurent_len::<Self>(data0, data1, self.bit_len(), total)
    }
    /// iterates all objects in tree with callback function
    fn iterate_slices_with_keys<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(K, SliceData) -> Result<bool>,
    {
        crate::HashmapType::iterate_slices(self, |mut key, mut slice| {
            let key = K::construct_from(&mut key)?;
            Y::skip(&mut slice)?;
            p(key, slice)
        })
    }
    /// iterates all objects as slices with keys and augs in tree with callback function
    fn iterate_slices_with_keys_and_aug<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(K, SliceData, Y) -> Result<bool>,
    {
        crate::HashmapType::iterate_slices(self, |mut key, mut slice| {
            let key = K::construct_from(&mut key)?;
            let aug = Y::construct_from(&mut slice)?;
            p(key, slice, aug)
        })
    }
    /// rename to iterate when method is removed in types
    /// iterates objects
    fn iterate_objects<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(X) -> Result<bool>,
    {
        crate::HashmapType::iterate_slices(self, |_, mut slice| {
            <Y>::skip(&mut slice)?;
            p(X::construct_from(&mut slice)?)
        })
    }
    /// iterate objects with keys
    fn iterate_with_keys<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(K, X) -> Result<bool>,
    {
        crate::HashmapType::iterate_slices(self, |mut key, mut slice| {
            let key = K::construct_from(&mut key)?;
            <Y>::skip(&mut slice)?;
            p(key, X::construct_from(&mut slice)?)
        })
    }
    /// iterate objects with keys and augs
    fn iterate_with_keys_and_aug<F>(&self, mut p: F) -> Result<bool>
    where
        F: FnMut(K, X, Y) -> Result<bool>,
    {
        crate::HashmapType::iterate_slices(self, |mut key, mut slice| {
            let key = K::construct_from(&mut key)?;
            let aug = Y::construct_from(&mut slice)?;
            p(key, X::construct_from(&mut slice)?, aug)
        })
    }
    #[cfg(test)]
    /// Puts element to the tree
    fn set_serialized(
        &mut self,
        key: SliceData,
        leaf: &SliceData,
        extra: &Y,
    ) -> Result<Option<SliceData>> {
        let (value, _) = self.set_builder_serialized(key, &leaf.as_builder()?, extra)?;
        Ok(value)
    }
    /// Puts element to the tree
    fn set_builder_serialized(
        &mut self,
        key: SliceData,
        leaf: &BuilderData,
        extra: &Y,
    ) -> Result<(Option<SliceData>, usize)> {
        let bit_len = self.bit_len();
        Self::check_key_fail(bit_len, &key)?;
        // ahme_empty$0 {n:#} {X:Type} {Y:Type} extra:Y = HashmapAugE n X Y;
        // ahme_root$1 {n:#} {X:Type} {Y:Type} root:^(HashmapAug n X Y) extra:Y = HashmapAugE n X Y;
        if let Some(mut root) = self.data().cloned() {
            let mut depth = 0;
            let (result, extra) =
                self.put_to_node(&mut root, bit_len, key, leaf, extra, &mut depth)?;
            self.set_root_extra(extra);
            *self.data_mut() = Some(root);
            Ok((result, depth))
        } else {
            self.set_root_extra(extra.clone());
            let builder = Self::make_cell_with_label_and_builder(
                &key,
                bit_len,
                true,
                &self.combine(extra, leaf)?,
            )?;
            *self.data_mut() = Some(builder.into_cell()?);
            Ok((None, 0))
        }
    }
    // Puts element to required branch by first bit
    fn put_to_fork(
        &self,
        slice: &mut SliceData,
        bit_len: usize,
        mut key: SliceData,
        leaf: &BuilderData,
        extra: &Y,
        depth: &mut usize,
    ) -> AugResult<Y> {
        let next_index = key.get_next_bit_int()?;
        // ahmn_fork#_ {n:#} {X:Type} {Y:Type} left:^(HashmapAug n X Y) right:^(HashmapAug n X Y) extra:Y
        // = HashmapAugNode (n + 1) X Y;
        if slice.remaining_references() < 2 {
            fail!(BlockError::InvalidArg("slice must contain 2 or more references".to_string()))
        }
        let mut references = slice.shrink_references(2..); // left and right, drop extra
        assert_eq!(references.len(), 2);
        let mut fork_extra = Self::find_extra(&references[1 - next_index], bit_len - 1)?;
        let (result, extra) =
            self.put_to_node(&mut references[next_index], bit_len - 1, key, leaf, extra, depth)?;
        fork_extra.calc(&extra)?;
        let mut builder = BuilderData::new();
        for reference in references.drain(..) {
            builder.checked_append_reference(reference)?;
        }
        fork_extra.write_to(&mut builder)?;
        *slice = SliceData::load_builder(builder)?;
        *depth += 1;
        Ok((result, fork_extra))
    }
    // Continues or finishes search of place
    fn put_to_node(
        &self,
        cell: &mut Cell,
        bit_len: usize,
        key: SliceData,
        leaf: &BuilderData,
        extra: &Y,
        depth: &mut usize,
    ) -> AugResult<Y> {
        let result;
        let mut cursor = LabelReader::with_cell(cell)?;
        let label = cursor.get_label(bit_len)?;
        let mut slice = cursor.remainder()?;
        let builder = if label == key {
            // replace existing leaf
            Y::skip(&mut slice)?; // skip extra
            let res_extra = extra.clone();
            result = Ok((Some(slice), res_extra));
            Self::make_cell_with_label_and_builder(
                &key,
                bit_len,
                true,
                &self.combine(extra, leaf)?,
            )?
        } else if label.is_empty_bitstring() {
            // 1-bit edge just recalc extra
            result = self.put_to_fork(&mut slice, bit_len, key, leaf, extra, depth);
            Self::make_cell_with_label_and_data(&label, bit_len, false, &slice)?
        } else {
            match SliceData::common_prefix(&label, &key) {
                (label_prefix, Some(label_remainder), Some(key_remainder)) => {
                    // new leaf insert
                    let (extra, builder) = self.slice_edge(
                        slice,
                        label_remainder,
                        label_prefix.unwrap_or_default(),
                        bit_len,
                        key_remainder,
                        leaf,
                        extra,
                    )?;
                    // makes one pruned branch
                    *cell = builder.into_cell()?;
                    *depth = 1;
                    return Ok((None, extra));
                }
                (Some(prefix), None, Some(key_remainder)) => {
                    // next iteration
                    result = self.put_to_fork(
                        &mut slice,
                        bit_len - prefix.remaining_bits(),
                        key_remainder,
                        leaf,
                        extra,
                        depth,
                    );
                    Self::make_cell_with_label_and_data(&label, bit_len, false, &slice)?
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
        };
        *cell = builder.into_cell()?;
        result
    }
    // Slices the edge and put new leaf
    #[allow(clippy::too_many_arguments)]
    fn slice_edge(
        &self,
        mut slice: SliceData, // leftover data after label reading - we don't know if it is leaf or fork
        mut label: SliceData, // label of the leftover
        prefix: SliceData,    // label for new fork
        bit_len: usize,       // total length of key on this level
        mut key: SliceData,
        leaf: &BuilderData,
        extra: &Y,
    ) -> Result<(Y, BuilderData)> {
        key.move_by(1)?;
        let label_bit = label.get_next_bit()?; // we must know if it left or right
        let length = bit_len - 1 - prefix.remaining_bits(); //
        let is_leaf = length == label.remaining_bits();
        // Common prefix
        let mut builder = Self::make_cell_with_label(&prefix, bit_len)?;
        // Remainder of tree
        let existing_cell = Self::make_cell_with_label_and_data(&label, length, is_leaf, &slice)?;
        // AugResult<Y> for fork
        if !is_leaf {
            if slice.remaining_references() < 2 {
                debug_assert!(false, "fork should have at least two refs");
            }
            slice.shrink_references(2..); // drain left, right
        }
        let mut fork_extra = Y::construct_from(&mut slice)?;
        fork_extra.calc(extra)?;
        // Leaf for fork
        let another_cell = Self::make_cell_with_label_and_builder(
            &key,
            length,
            true,
            &self.combine(extra, leaf)?,
        )?;
        if !label_bit {
            builder.checked_append_reference(existing_cell.into_cell()?)?;
            builder.checked_append_reference(another_cell.into_cell()?)?;
        } else {
            builder.checked_append_reference(another_cell.into_cell()?)?;
            builder.checked_append_reference(existing_cell.into_cell()?)?;
        };
        fork_extra.write_to(&mut builder)?;
        Ok((fork_extra, builder))
    }
    // Combines extra with leaf
    fn combine(&self, extra: &Y, leaf: &BuilderData) -> Result<BuilderData> {
        let mut builder = extra.write_to_new_cell()?;
        builder.append_builder(leaf)?;
        Ok(builder)
    }
    // Gets label then get_extra
    fn find_extra(cell: &Cell, bit_len: usize) -> Result<Y> {
        let mut cursor = LabelReader::with_cell(cell)?;
        let label = cursor.get_label(bit_len)?;
        let mut slice = cursor.remainder()?;
        if label.remaining_bits() != bit_len {
            // fork - drain left and right
            if slice.remaining_references() < 2 {
                fail!(ExceptionCode::CellUnderflow)
            }
            slice.shrink_references(2..);
        }
        Y::construct_from(&mut slice)
    }
    // Calc new extra for fork
    fn calc_extra(left: &Cell, right: &Cell, bit_len: usize) -> Result<Y> {
        let mut aug = Self::find_extra(left, bit_len)?;
        aug.calc(&Self::find_extra(right, bit_len)?)?;
        Ok(aug)
    }
    //
    fn traverse<F, R>(&self, mut p: F) -> Result<Option<R>>
    where
        F: FnMut(&[u8], usize, Y, Option<X>) -> Result<TraverseNextStep<R>>,
    {
        self.traverse_slices(|key_prefix, prefix_len, mut label| {
            let aug = Y::construct_from(&mut label)?;
            if prefix_len == self.bit_len() {
                let val = X::construct_from(&mut label)?;
                p(key_prefix, prefix_len, aug, Some(val))
            } else {
                p(key_prefix, prefix_len, aug, None)
            }
        })
    }
    //
    fn traverse_slices<F, R>(&self, mut p: F) -> Result<Option<R>>
    where
        F: FnMut(&[u8], usize, SliceData) -> Result<TraverseNextStep<R>>,
    {
        if let Some(root) = self.data() {
            Self::traverse_internal(root, BuilderData::default(), self.bit_len(), &mut |k, l, n| {
                p(k, l, n)
            })
        } else {
            Ok(None)
        }
    }
    /// recursive traverse tree and call callback function
    fn traverse_internal<F, R>(
        cell: &Cell,
        mut key: BuilderData,
        mut bit_len: usize,
        callback: &mut F,
    ) -> Result<Option<R>>
    where
        F: FnMut(&[u8], usize, SliceData) -> Result<crate::TraverseNextStep<R>>,
    {
        let mut cursor = LabelReader::with_cell(cell)?;
        let label = cursor.get_label(bit_len)?;
        let slice = cursor.remainder()?;
        let label_length = label.remaining_bits();
        match label_length.cmp(&bit_len) {
            Ordering::Less => {
                bit_len -= label_length + 1;

                let mut aug = slice.clone();
                aug.checked_drain_reference()?;
                aug.checked_drain_reference()?;
                key.checked_append_references_and_data(&label)?;
                let to_visit = match callback(key.data(), key.length_in_bits(), aug)? {
                    TraverseNextStep::Stop => return Ok(None),
                    TraverseNextStep::End(r) => return Ok(Some(r)),
                    TraverseNextStep::VisitZero => [Some(0), None],
                    TraverseNextStep::VisitOne => [Some(1), None],
                    TraverseNextStep::VisitZeroOne => [Some(0), Some(1)],
                    TraverseNextStep::VisitOneZero => [Some(1), Some(0)],
                };
                for i in to_visit.iter().flatten() {
                    let mut key = key.clone();
                    key.append_bit_bool(*i != 0)?;
                    if let Some(r) =
                        Self::traverse_internal(&slice.reference(*i)?, key, bit_len, callback)?
                    {
                        return Ok(Some(r));
                    }
                }
            }
            Ordering::Equal => {
                key.checked_append_references_and_data(&label)?;
                if let TraverseNextStep::End(r) = callback(key.data(), key.length_in_bits(), slice)?
                {
                    return Ok(Some(r));
                }
            }
            _ => fail!(BlockError::InvalidData("label_length > bit_len".to_string())),
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "tests/test_hashmapaug.rs"]
mod tests;
