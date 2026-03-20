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
#![allow(dead_code)] // TODO: maybe you know more efficient way
#![allow(clippy::type_complexity)]
#![allow(clippy::vec_init_then_push)]
use super::*;
use crate::{
    define_HashmapAugE, dictionary::HashmapTraverser, error, AddSub, Grams, HashmapFilterResult,
    HashmapSubtree, IBitstring, UInt256,
};
use std::fmt;

#[derive(Eq, Clone, Debug, Default, PartialEq)]
pub struct GramStruct(Grams);

impl GramStruct {
    pub const fn with_value(value: u8) -> Self {
        Self(Grams::new(value as u64))
    }
}

impl Serializable for GramStruct {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.0.write_to(cell)?;
        cell.checked_append_reference(self.0.serialize()?)?;
        cell.checked_append_reference(self.0.serialize()?)?;
        Ok(())
    }
}

impl Deserializable for GramStruct {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.0.read_from(slice)?;
        let r = slice.checked_drain_reference()?;
        let g = Grams::construct_from_cell(r)?;
        assert_eq!(self.0, g);
        let r = slice.checked_drain_reference()?;
        let g = Grams::construct_from_cell(r)?;
        assert_eq!(self.0, g);
        Ok(())
    }
}

impl Augmentable for GramStruct {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        self.0.add(&other.0)
    }
}

define_HashmapAugE!(GramHashmap7, 7, u8, u32, GramStruct);
define_HashmapAugE!(GramHashmap8, 8, u8, u32, GramStruct);
impl HashmapSubtree for GramHashmap8 {}

impl Augmentation<GramStruct> for u32 {
    fn aug(&self) -> Result<GramStruct> {
        unreachable!()
    }
}

#[test]
fn test_hashmapaug() {
    //construct empty 7 bit_len
    let mut tree = GramHashmap7::default();
    assert_eq!(&GramStruct::with_value(0), tree.root_extra());
    assert!(tree.is_empty());
    println!("empty {}", tree);

    // add first
    let key1 = SliceData::new(vec![0xFF]);
    let value1 = key1.clone();
    let extra1 = GramStruct::with_value(1);
    tree.set_serialized(key1.clone(), &value1, &extra1).unwrap();
    println!("first {}", tree);
    assert_eq!(&GramStruct::with_value(1), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));

    // replace single
    let value1 = SliceData::new(vec![0xFB]);
    let extra1 = GramStruct::with_value(2);
    tree.set_serialized(key1.clone(), &value1, &extra1).unwrap();
    println!("replaced {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(2), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));

    // add second with same first bit
    let key2 = SliceData::new(vec![0xF1]);
    let value2 = key2.clone();
    let extra2 = GramStruct::with_value(3);
    tree.set_serialized(key2.clone(), &value2, &extra2).unwrap();
    println!("second {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(5), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));
    assert_eq!(tree.get_serialized_as_slice(key2.clone()).unwrap(), Some(value2.clone()));

    // replace second
    let value2 = SliceData::new(vec![0xF2]);
    let extra2 = GramStruct::with_value(4);
    tree.set_serialized(key2.clone(), &value2, &extra2).unwrap();
    println!("second replaced {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(6), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));
    assert_eq!(tree.get_serialized_as_slice(key2.clone()).unwrap(), Some(value2.clone()));

    // add third with dif first bit
    let key3 = SliceData::new(vec![0x01]);
    let value3 = key3.clone();
    let extra3 = GramStruct::with_value(5);
    tree.set_serialized(key3.clone(), &value3, &extra3).unwrap();
    println!("third added {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(11), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));
    assert_eq!(tree.get_serialized_as_slice(key2.clone()).unwrap(), Some(value2.clone()));
    assert_eq!(tree.get_serialized_as_slice(key3.clone()).unwrap(), Some(value3.clone()));

    // replace third
    let value3 = SliceData::new(vec![0x0F]);
    let extra3 = GramStruct::with_value(6);
    tree.set_serialized(key3.clone(), &value3, &extra3).unwrap();
    println!("third replaced {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(12), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1.clone()).unwrap(), Some(value1.clone()));
    assert_eq!(tree.get_serialized_as_slice(key2.clone()).unwrap(), Some(value2.clone()));
    assert_eq!(tree.get_serialized_as_slice(key3.clone()).unwrap(), Some(value3.clone()));

    // add fourth with same 1 bit label
    let key4 = SliceData::new(vec![0x07]);
    let value4 = key4.clone();
    let extra4 = GramStruct::with_value(7);
    tree.set_serialized(key4.clone(), &value4, &extra4).unwrap();
    println!("fourth added {}", tree);
    assert!(!tree.is_empty());
    assert_eq!(&GramStruct::with_value(19), tree.root_extra());
    assert_eq!(tree.get_serialized_as_slice(key1).unwrap(), Some(value1));
    assert_eq!(tree.get_serialized_as_slice(key2).unwrap(), Some(value2));
    assert_eq!(tree.get_serialized_as_slice(key3).unwrap(), Some(value3));
    assert_eq!(tree.get_serialized_as_slice(key4).unwrap(), Some(value4));
}

fn make_tree_with_filled_root_label() -> GramHashmap8 {
    let mut tree = GramHashmap8::default();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11111111], 8),
        &SliceData::new(vec![0b11111111]),
        &GramStruct::with_value(1),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11111100], 8),
        &SliceData::new(vec![0b11111100]),
        &GramStruct::with_value(2),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11110011], 8),
        &SliceData::new(vec![0b11110011]),
        &GramStruct::with_value(3),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11110000], 8),
        &SliceData::new(vec![0b11110000]),
        &GramStruct::with_value(4),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11001111], 8),
        &SliceData::new(vec![0b11001111]),
        &GramStruct::with_value(5),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11001100], 8),
        &SliceData::new(vec![0b11001100]),
        &GramStruct::with_value(6),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11000011], 8),
        &SliceData::new(vec![0b11000011]),
        &GramStruct::with_value(7),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11000000], 8),
        &SliceData::new(vec![0b11000000]),
        &GramStruct::with_value(8),
    )
    .unwrap();
    assert_eq!(tree.root_extra(), &GramStruct::with_value(36));
    tree
}
fn make_tree_with_empty_root_label() -> GramHashmap8 {
    let mut tree = GramHashmap8::default();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11111100], 8),
        &SliceData::new(vec![0b11111100]),
        &GramStruct::with_value(1),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11110000], 8),
        &SliceData::new(vec![0b11110000]),
        &GramStruct::with_value(2),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11001100], 8),
        &SliceData::new(vec![0b11001100]),
        &GramStruct::with_value(3),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b11000000], 8),
        &SliceData::new(vec![0b11000000]),
        &GramStruct::with_value(4),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b00111100], 8),
        &SliceData::new(vec![0b00111100]),
        &GramStruct::with_value(5),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b00110000], 8),
        &SliceData::new(vec![0b00110000]),
        &GramStruct::with_value(6),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b00001100], 8),
        &SliceData::new(vec![0b00001100]),
        &GramStruct::with_value(7),
    )
    .unwrap();
    tree.set_serialized(
        SliceData::from_raw(vec![0b00000000], 8),
        &SliceData::new(vec![0b00000000]),
        &GramStruct::with_value(8),
    )
    .unwrap();
    assert_eq!(tree.root_extra(), &GramStruct::with_value(36));
    tree
}

#[test]
fn test_hashmap_split() {
    let tree = make_tree_with_empty_root_label();
    let (left, right) = tree.split(&SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 4);
    assert_eq!(right.len().unwrap(), 4);

    tree.split(&SliceData::new(vec![0x40])).expect_err("should generate error");
    tree.split(&SliceData::new(vec![0xC0])).expect_err("should generate error");

    let (l, r) = left.split(&SliceData::new(vec![0x20])).unwrap();
    assert_eq!(l.len().unwrap(), 2);
    assert_eq!(r.len().unwrap(), 2);
    left.split(&SliceData::new(vec![0xF0])).expect_err("should generate error");

    let (l, r) = right.split(&SliceData::new(vec![0xE0])).unwrap();
    assert_eq!(l.len().unwrap(), 2);
    assert_eq!(r.len().unwrap(), 2);
    right.split(&SliceData::new(vec![0x40])).expect_err("should generate error");

    let tree = make_tree_with_filled_root_label();
    let (left, right) = tree.split(&SliceData::new(vec![0xC0])).unwrap();
    assert_eq!(left.len().unwrap(), 0);
    assert_eq!(right.len().unwrap(), 8);
    left.split(&SliceData::new(vec![0x40])).unwrap(); // split empty tree anywhere

    let (l, r) = right.split(&SliceData::new(vec![0xE0])).unwrap();
    assert_eq!(l.len().unwrap(), 4);
    assert_eq!(r.len().unwrap(), 4);

    let (left, right) = tree.split(&SliceData::new(vec![0xE0])).unwrap();
    assert_eq!(left.len().unwrap(), 4);
    assert_eq!(right.len().unwrap(), 4);

    tree.split(&SliceData::new(vec![0x40])).expect_err("should generate error");
    tree.split(&SliceData::new(vec![0xA0])).expect_err("should generate error");
    tree.split(&SliceData::new(vec![0xD0])).expect_err("should generate error");
    tree.split(&SliceData::new(vec![0xF0])).expect_err("should generate error");
}

#[test]
fn test_hashmap_merge() {
    let mut left = GramHashmap8::default();
    left.set_serialized(
        SliceData::from_raw(vec![0b11000000], 8),
        &SliceData::new(vec![0b11000000]),
        &GramStruct::with_value(1),
    )
    .unwrap();
    let mut right = GramHashmap8::default();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00000000], 8),
            &SliceData::new(vec![0b00000000]),
            &GramStruct::with_value(2),
        )
        .unwrap();
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 2);
    let mut result = GramHashmap8::default();
    result
        .set_serialized(
            SliceData::from_raw(vec![0b11000000], 8),
            &SliceData::new(vec![0b11000000]),
            &GramStruct::with_value(1),
        )
        .unwrap();
    result
        .set_serialized(
            SliceData::from_raw(vec![0b00000000], 8),
            &SliceData::new(vec![0b00000000]),
            &GramStruct::with_value(2),
        )
        .unwrap();
    assert_eq!(left, result);

    let mut left = GramHashmap8::default();
    let mut right = GramHashmap8::default();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00000000], 8),
            &SliceData::new(vec![0b00000000]),
            &GramStruct::with_value(1),
        )
        .unwrap();
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 1);
    assert_eq!(left.root_extra(), &GramStruct::with_value(1));
    assert_eq!(left, right);

    let mut left = GramHashmap8::default();
    left.set_serialized(
        SliceData::from_raw(vec![0b11000000], 8),
        &SliceData::new(vec![0b11000000]),
        &GramStruct::with_value(1),
    )
    .unwrap();
    let right = GramHashmap8::default();
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 1);
    let mut result = GramHashmap8::default();
    result
        .set_serialized(
            SliceData::from_raw(vec![0b11000000], 8),
            &SliceData::new(vec![0b11000000]),
            &GramStruct::with_value(1),
        )
        .unwrap();
    assert_eq!(left, result);

    let tree = make_tree_with_empty_root_label();
    let mut left = GramHashmap8::default();
    left.set_serialized(
        SliceData::from_raw(vec![0b11111100], 8),
        &SliceData::new(vec![0b11111100]),
        &GramStruct::with_value(1),
    )
    .unwrap();
    left.set_serialized(
        SliceData::from_raw(vec![0b11110000], 8),
        &SliceData::new(vec![0b11110000]),
        &GramStruct::with_value(2),
    )
    .unwrap();
    left.set_serialized(
        SliceData::from_raw(vec![0b11001100], 8),
        &SliceData::new(vec![0b11001100]),
        &GramStruct::with_value(3),
    )
    .unwrap();
    left.set_serialized(
        SliceData::from_raw(vec![0b11000000], 8),
        &SliceData::new(vec![0b11000000]),
        &GramStruct::with_value(4),
    )
    .unwrap();

    let mut right = GramHashmap8::default();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00111100], 8),
            &SliceData::new(vec![0b00111100]),
            &GramStruct::with_value(5),
        )
        .unwrap();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00110000], 8),
            &SliceData::new(vec![0b00110000]),
            &GramStruct::with_value(6),
        )
        .unwrap();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00001100], 8),
            &SliceData::new(vec![0b00001100]),
            &GramStruct::with_value(7),
        )
        .unwrap();
    right
        .set_serialized(
            SliceData::from_raw(vec![0b00000000], 8),
            &SliceData::new(vec![0b00000000]),
            &GramStruct::with_value(8),
        )
        .unwrap();

    assert_eq!(left.len().unwrap(), 4);
    assert_eq!(right.len().unwrap(), 4);
    assert_eq!(left.root_extra(), &GramStruct::with_value(10));
    assert_eq!(right.root_extra(), &GramStruct::with_value(26));

    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 8);
    assert_eq!(tree, left);
    assert_eq!(left.root_extra(), &GramStruct::with_value(36));
}

define_HashmapAugE!(SimpleAugDict, 8, u8, u8, GramStruct);

impl Augmentation<GramStruct> for u8 {
    fn aug(&self) -> Result<GramStruct> {
        unreachable!()
    }
}

#[test]
fn test_scan_diff_empty() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b11001100u8, &0b11001100, &GramStruct::with_value(3)).unwrap();

    tree_2.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_2.set(&0b11001100u8, &0b11001100, &GramStruct::with_value(3)).unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_scan_diff_1() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b11001100u8, &0b11001100, &GramStruct::with_value(3)).unwrap();

    tree_2.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    correct_dif.push((0b11001100, Some((0b11001100, GramStruct::with_value(3))), None));

    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec.len() == 1);
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_scan_diff_2() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();

    tree_2.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_2.set(&0b11001100u8, &0b11001100, &GramStruct::with_value(3)).unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    correct_dif.push((0b11001100, None, Some((0b11001100, GramStruct::with_value(3)))));

    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec.len() == 1);
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_scan_diff_3() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b11001100u8, &0b11001101, &GramStruct::with_value(3)).unwrap();

    tree_2.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_2.set(&0b11001100u8, &0b11001100, &GramStruct::with_value(3)).unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();
    correct_dif.push((
        0b11001100,
        Some((0b11001101, GramStruct::with_value(3))),
        Some((0b11001100, GramStruct::with_value(3))),
    ));
    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_filter_simple() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b11001100u8, &0b11001101, &GramStruct::with_value(3)).unwrap();

    tree_2.set(&0b11111100u8, &0b11111100, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11110000u8, &0b11110000, &GramStruct::with_value(2)).unwrap();
    tree_2.set(&0b11001100u8, &0b11001101, &GramStruct::with_value(3)).unwrap();
    tree_1
        .filter(|key, _value, _aug| {
            let key = key.data()[0];
            if key == 0b11001100u8 {
                Ok(HashmapFilterResult::Remove)
            } else {
                Ok(HashmapFilterResult::Accept)
            }
        })
        .unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();
    correct_dif.push((0b11001100, Some((0b11001101, GramStruct::with_value(3))), None));
    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_2
        .scan_diff_with_aug(&tree_1, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_filter() {
    let mut tree_1 = SimpleAugDict::default();
    let mut tree_2 = SimpleAugDict::default();

    tree_1.set(&0b11001100u8, &0b11001101, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b11010000u8, &0b11001101, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b11010100u8, &0b11001101, &GramStruct::with_value(3)).unwrap();
    tree_1.set(&0b11011000u8, &0b11001101, &GramStruct::with_value(4)).unwrap();
    tree_1.set(&0b11011100u8, &0b11001101, &GramStruct::with_value(5)).unwrap();
    tree_1.set(&0b11100000u8, &0b11001101, &GramStruct::with_value(6)).unwrap();
    tree_1.set(&0b11100100u8, &0b11001101, &GramStruct::with_value(7)).unwrap();
    tree_1.set(&0b11101000u8, &0b11001101, &GramStruct::with_value(8)).unwrap();

    tree_2.set(&0b11001100u8, &0b11001101, &GramStruct::with_value(1)).unwrap();
    tree_2.set(&0b11010100u8, &0b11001101, &GramStruct::with_value(3)).unwrap();
    tree_2.set(&0b11011100u8, &0b11001101, &GramStruct::with_value(5)).unwrap();
    tree_2.set(&0b11100100u8, &0b11001101, &GramStruct::with_value(7)).unwrap();

    let mut correct_dif = vec![
        (0b11010000, Some((0b11001101, GramStruct::with_value(2))), None),
        (0b11011000, Some((0b11001101, GramStruct::with_value(4))), None),
        (0b11100000, Some((0b11001101, GramStruct::with_value(6))), None),
        (0b11101000, Some((0b11001101, GramStruct::with_value(8))), None),
    ];

    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec == correct_dif);

    tree_1
        .filter(|key, _value, _aug| {
            let key = key.data()[0];
            if key % 8 == 0 {
                Ok(HashmapFilterResult::Remove)
            } else {
                Ok(HashmapFilterResult::Accept)
            }
        })
        .unwrap();

    let mut correct_dif: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();
    let mut diff_vec: Vec<(u8, Option<(u8, GramStruct)>, Option<(u8, GramStruct)>)> = Vec::new();

    tree_1
        .scan_diff_with_aug(&tree_2, |key, value1, value2| {
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap();
    correct_dif.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    diff_vec.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(diff_vec == correct_dif);
}

#[test]
fn test_traverse() {
    let mut tree_1 = SimpleAugDict::default();

    tree_1.set(&0b000u8, &0b000u8, &GramStruct::with_value(0)).unwrap();
    tree_1.set(&0b001u8, &0b001u8, &GramStruct::with_value(1)).unwrap();
    tree_1.set(&0b010u8, &0b010u8, &GramStruct::with_value(2)).unwrap();
    tree_1.set(&0b011u8, &0b011u8, &GramStruct::with_value(3)).unwrap();
    tree_1.set(&0b100u8, &0b100u8, &GramStruct::with_value(4)).unwrap();
    tree_1.set(&0b101u8, &0b101u8, &GramStruct::with_value(5)).unwrap();
    tree_1.set(&0b110u8, &0b110u8, &GramStruct::with_value(6)).unwrap();
    tree_1.set(&0b111u8, &0b111u8, &GramStruct::with_value(7)).unwrap();

    let zero_way = vec![
        GramStruct::with_value(28),
        GramStruct::with_value(6),
        GramStruct::with_value(1),
        GramStruct::with_value(0),
    ];
    let mut way = vec![];
    let res = tree_1
        .traverse_slices(
            |_key_prefix, _key_prefix_len, mut label| -> Result<TraverseNextStep<()>> {
                let aug = GramStruct::construct_from(&mut label).unwrap();
                way.push(aug);
                Ok(TraverseNextStep::VisitZero)
            },
        )
        .unwrap();
    assert!(res.is_none());
    assert_eq!(way, zero_way);

    let ones_way = vec![
        GramStruct::with_value(28),
        GramStruct::with_value(22),
        GramStruct::with_value(13),
        GramStruct::with_value(7),
    ];
    let mut way = vec![];
    let res = tree_1
        .traverse(|_key_prefix, key_prefix_len, aug, value_opt| {
            way.push(aug);
            if key_prefix_len == 8 {
                Ok(TraverseNextStep::End(value_opt.unwrap()))
            } else {
                Ok(TraverseNextStep::VisitOne)
            }
        })
        .unwrap();
    assert_eq!(res.unwrap(), 7);
    assert_eq!(way, ones_way);

    let high_way =
        vec![GramStruct::with_value(28), GramStruct::with_value(6), GramStruct::with_value(22)];
    let mut way = vec![];
    let res = tree_1
        .traverse(|_key_prefix, key_prefix_len, aug, _value_opt| -> Result<TraverseNextStep<()>> {
            way.push(aug);
            if key_prefix_len == 6 {
                Ok(TraverseNextStep::Stop)
            } else {
                Ok(TraverseNextStep::VisitZeroOne)
            }
        })
        .unwrap();
    assert!(res.is_none());
    assert_eq!(way, high_way);
}

define_HashmapAugE!(MyHashmap, 8, u8, u8, u8);

impl Augmentation<u8> for u8 {
    fn aug(&self) -> Result<u8> {
        unreachable!()
    }
}

// max
impl Augmentable for u8 {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        if *self < *other {
            *self = *other
        }
        Ok(true)
    }
}

fn check_hashmap_fill_and_filter(mut keys: Vec<u8>, remove: &[u8], stop: usize, cancel: usize) {
    keys.sort();
    let mut queue1 = MyHashmap::default();
    let mut queue2 = MyHashmap::default();
    for i in 0..keys.len() {
        let key = keys[i];
        let val = 0;
        let aug = i as u8 + 1;
        assert_eq!(
            queue1.get_raw(&key).unwrap(),
            None,
            "generated two equal random keys - try to restart test"
        );
        queue1.set(&key, &val, &aug).unwrap();
        if stop <= i || cancel < keys.len() || !remove.contains(&key) {
            queue2.set(&key, &val, &aug).unwrap();
        }
    }
    // queue1.dump();
    // println!("{:#.3}", queue1.data().cloned().unwrap());

    queue1
        .filter(|key, _val, _aug| {
            let key = key.data()[0];
            if cancel < keys.len() && keys[cancel] == key {
                Ok(HashmapFilterResult::Cancel)
            } else if stop < keys.len() && keys[stop] == key {
                Ok(HashmapFilterResult::Stop)
            } else if remove.contains(&key) {
                Ok(HashmapFilterResult::Remove)
            } else {
                Ok(HashmapFilterResult::Accept)
            }
        })
        .unwrap();
    let mut res1 = vec![];
    queue1
        .iterate_with_keys_and_aug(|key, val, aug| {
            res1.push((key, val, aug));
            Ok(true)
        })
        .unwrap();
    // println!("{:#.3}", queue1.data().cloned().unwrap_or_default());
    // assert_eq!(queue, queue2);
    // additional testing
    let mut res2 = vec![];
    queue2
        .iterate_with_keys_and_aug(|key, val, aug| {
            res2.push((key, val, aug));
            Ok(true)
        })
        .unwrap();
    assert_eq!(res1.len(), res2.len());
    if res1 != res2 {
        panic!("not equal")
    }
    for i in 0..res1.len() {
        if i % 7 == 0 {
            println!("{}", i);
            pretty_assertions::assert_eq!(res1[i], res2[i]);
        }
    }
}

#[test]
fn test_hahsmap_fill_and_filter() {
    check_hashmap_fill_and_filter([133, 167, 222].to_vec(), &[167], 2, 4);
}

#[test]
fn test_hahsmap_rand_fill_and_filter() {
    let mut rng = rand::thread_rng();
    let max = 4;
    let mut keys = vec![];
    let mut remove = vec![];
    for _ in 0..max {
        loop {
            let key = rand::Rng::gen::<u8>(&mut rng);
            if !keys.contains(&key) {
                keys.push(key);
                if rand::Rng::gen::<bool>(&mut rng) {
                    remove.push(key);
                }
                break;
            }
        }
    }
    let stop = rand::Rng::gen::<usize>(&mut rng) % keys.len();
    let cancel = keys.len(); // rand::Rng::gen::<usize>(&mut rng) % keys.len();
    println!("{:#?}", keys);
    println!("{:#?}", remove);
    println!("{} {}", stop, cancel);
    check_hashmap_fill_and_filter(keys, &remove, stop, cancel);
}

#[test]
fn test_hashmap_add_remove() {
    let mut hashmap = MyHashmap::default();
    hashmap.set(&1, &1, &1).unwrap();
    assert_eq!(hashmap.root_extra(), &1);
    hashmap.set(&2, &2, &2).unwrap();
    assert_eq!(hashmap.root_extra(), &2);
    hashmap.del(&2).unwrap();
    assert_eq!(hashmap.root_extra(), &1);
    hashmap.del(&1).unwrap();
    assert_eq!(hashmap.root_extra(), &0);
}

#[test]
fn test_find_by_aug() {
    let mut hashmap = MyHashmap::default();
    hashmap.set(&1, &1, &1).unwrap();
    assert_eq!(hashmap.find_by_root_aug().unwrap(), Some((1, 1)));
    hashmap.set(&2, &2, &2).unwrap();
    assert_eq!(hashmap.find_by_root_aug().unwrap(), Some((2, 2)));
    hashmap.set(&3, &3, &3).unwrap();
    hashmap.set(&4, &4, &4).unwrap();
    assert_eq!(hashmap.find_by_root_aug().unwrap(), Some((4, 4)));
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MinLt(u64);

impl Augmentable for MinLt {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        self.0 = self.0.min(other.0);
        Ok(true)
    }
}

impl Serializable for MinLt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.0.write_to(cell)
    }
}

impl Deserializable for MinLt {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.0 = Deserializable::construct_from(slice)?;
        Ok(())
    }
}

impl Augmentation<MinLt> for u8 {
    fn aug(&self) -> Result<MinLt> {
        unreachable!()
    }
}

define_HashmapAugE!(TestDescr, 256, UInt256, u8, MinLt);

#[test]
fn test_multiset_aug() {
    #[rustfmt::skip]
    let data = [
        ([1u8; 32], 0x11u8, 71u64),
        ([2u8; 32], 0x22u8, 73u64),
        ([8u8; 32], 0x33u8, 74u64),
    ];
    let mut hashmap = TestDescr::new();
    let mut tr = HashmapTraverser::new(&hashmap);

    for (key, value, aug) in data {
        {
            let key = key.write_to_bitstring().unwrap();
            let mut builder = aug.write_to_new_cell().unwrap();
            value.write_to(&mut builder).unwrap();
            let data = SliceData::load_bitstring(builder).unwrap();
            tr.insert(key, Some(data)).unwrap();
        }
        let key = UInt256::with_array(key);
        hashmap.set(&key, &value, &MinLt(aug)).unwrap();
    }

    println!("{:?}", tr.root);

    let new_root = tr.traverse().unwrap().unwrap();
    let new_cell = format!("{new_root:#.10}");
    let cell = format!("{:#.10}", hashmap.data().unwrap());
    pretty_assertions::assert_eq!(new_cell, cell);
    assert_eq!(&new_root, hashmap.data().unwrap());

    let mut hashmap = TestDescr::new();
    hashmap
        .multiset(data.into_iter().map(|(key, value, aug)| {
            let key = key.write_to_bitstring().unwrap();
            let mut builder = aug.write_to_new_cell().unwrap();
            value.write_to(&mut builder).unwrap();
            let data = SliceData::load_bitstring(builder).unwrap();
            (key, Some(data))
        }))
        .unwrap();

    assert_eq!(&new_root, hashmap.data().unwrap());
}

#[test]
fn test_find_leaf_hashmapaug() {
    let mut hashmap = GramHashmap8::new();

    hashmap.set(&10u8, &100u32, &GramStruct::with_value(1)).unwrap();
    hashmap.set(&20u8, &200u32, &GramStruct::with_value(2)).unwrap();
    hashmap.set(&30u8, &300u32, &GramStruct::with_value(3)).unwrap();
    hashmap.set(&50u8, &500u32, &GramStruct::with_value(5)).unwrap();

    // Test find_leaf with next=true, eq=true (find >= key)
    let result = hashmap.find_leaf(&10u8, true, true, false).unwrap();
    assert_eq!(result, Some((10u8, 100u32)));

    let result = hashmap.find_leaf(&15u8, true, true, false).unwrap();
    assert_eq!(result, Some((20u8, 200u32)));

    let result = hashmap.find_leaf(&20u8, true, true, false).unwrap();
    assert_eq!(result, Some((20u8, 200u32)));

    // Test find_leaf with next=true, eq=false (find > key)
    let result = hashmap.find_leaf(&10u8, true, false, false).unwrap();
    assert_eq!(result, Some((20u8, 200u32)));

    let result = hashmap.find_leaf(&20u8, true, false, false).unwrap();
    assert_eq!(result, Some((30u8, 300u32)));

    let result = hashmap.find_leaf(&50u8, true, false, false).unwrap();
    assert_eq!(result, None);

    // Test find_leaf with next=false, eq=true (find <= key)
    let result = hashmap.find_leaf(&30u8, false, true, false).unwrap();
    assert_eq!(result, Some((30u8, 300u32)));

    let result = hashmap.find_leaf(&25u8, false, true, false).unwrap();
    assert_eq!(result, Some((20u8, 200u32)));

    // Test find_leaf with next=false, eq=false (find < key)
    let result = hashmap.find_leaf(&30u8, false, false, false).unwrap();
    assert_eq!(result, Some((20u8, 200u32)));

    let result = hashmap.find_leaf(&10u8, false, false, false).unwrap();
    assert_eq!(result, None);
}
