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

#[test]
fn set_get_simple() {
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    assert_eq!(tree.get(SliceData::from_raw(vec![0b11111111], 8)).unwrap(), None);
    assert_eq!(
        tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap(),
        None
    );
    assert_eq!(
        tree.get(SliceData::from_raw(vec![0b11111111], 8)).unwrap(),
        Some(SliceData::new(vec![0b11111111]))
    );
}

#[test]
fn setref_and_get() {
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());
    assert_eq!(tree.get(SliceData::from_raw(vec![0b11111111], 8)).unwrap(), None);
    assert_eq!(
        tree.setref(
            SliceData::from_raw(vec![0b11111111], 8),
            &BuilderData::with_raw(vec![0b11111111], 8).unwrap().into_cell().unwrap()
        )
        .unwrap(),
        None
    );

    let mut new_reference = SliceData::new_empty();
    new_reference.append_reference(SliceData::new(vec![0b11111111, 0x80])).unwrap();
    assert_eq!(tree.get(SliceData::from_raw(vec![0b11111111], 8)).unwrap(), Some(new_reference));
}

#[test]
fn dictionary_rebuilding_key_tree() {
    let key_table = [
        SliceData::from_raw(vec![0b11111111], 8),
        SliceData::from_raw(vec![0b00000000], 8),
        SliceData::from_raw(vec![0b10000000], 8),
        SliceData::from_raw(vec![0b01111111], 8),
        SliceData::from_raw(vec![0b11000000], 8),
        SliceData::from_raw(vec![0b00111111], 8),
    ];

    let value_table = [
        SliceData::new(vec![0b11111111, 0x80]),
        SliceData::new(vec![0b01111111, 0x80]),
        SliceData::new(vec![0b00111111, 0x80]),
        SliceData::new(vec![0b00011111, 0x80]),
        SliceData::new(vec![0b00001111, 0x80]),
        SliceData::new(vec![0b00000111, 0x80]),
    ];

    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    for i in 0..6 {
        assert_eq!(tree.get(key_table[i].clone()).unwrap(), None);
        assert_eq!(tree.set(key_table[i].clone(), &value_table[i]).unwrap(), None);
        println!("{} -> {}", i, tree);
        assert_eq!(tree.get(key_table[i].clone()).unwrap(), Some(value_table[i].clone()));
    }

    println!("{}", tree);
    for i in 0..6 {
        println!("{}", i);
        let value = tree.set(key_table[i].clone(), &value_table[5 - i]).unwrap();
        assert_eq!(value, Some(value_table[i].clone()));
        let value = tree.get(key_table[i].clone()).unwrap();
        assert_eq!(value, Some(value_table[5 - i].clone()));
    }
    for i in 0..6 {
        assert_eq!(tree.get(key_table[i].clone()).unwrap(), Some(value_table[5 - i].clone()));
    }
}

#[test]
fn dictionary_rebuilding_tree() {
    let mut values = Vec::new();
    let mut tree = run_dictionary_8_bits(|(_, v)| values.push(v));

    let n = values.len();
    for i in 0..n {
        let v = values[i];
        let key = SliceData::from_raw(vec![v], 8);
        let old_value = SliceData::new(vec![v, 0x80]);
        let new_value = SliceData::new(vec![values[n - 1 - i], 0x80]);
        println!("replace: {}", v);
        let value = tree.set(key.clone(), &new_value).unwrap();
        assert_eq!(value, Some(old_value));
        let value = tree.get(key).unwrap();
        assert_eq!(value, Some(new_value));
    }
    for i in 0..n {
        let key = SliceData::from_raw(vec![values[i]], 8);
        let value = SliceData::new(vec![values[n - 1 - i], 0x80]);
        assert_eq!(tree.get(key).unwrap(), Some(value));
    }
}

fn run_dictionary_8_bits<F: FnMut((&mut HashmapE, u8))>(mut f: F) -> HashmapE {
    let table = [
        0b11111111, // FF 255   -1
        0b11110000, // F0 240  -16
        0b10001111, // 8F 143 -113
        0b10000000, // 80 128 -128
        0b00000000, // 00 0      0
        0b00001111, // 0F 15    15
        0b01110000, // 70 112  112
        0b01111111, // 7F 127  127
    ];

    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    table.iter().for_each(|&v| {
        let key = SliceData::from_raw(vec![v], 8);
        let value = SliceData::new(vec![v, 0x80]);
        assert_eq!(tree.get(key.clone()).unwrap(), None);
        assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
        log::trace!(target: "tests", "initial put value: {:08b} {}", v, tree);
        assert_eq!(tree.get(key).unwrap(), Some(value));
    });

    println!("{}", tree);

    table.iter().for_each(|&v| f((&mut tree, v)));

    tree
}

fn check_value_unsigned(tree: &HashmapE, value: u8, check: u8, next: bool) {
    if next {
        assert!(value < check);
    } else {
        assert!(value > check);
    }
    println!("check value {:08b} -> {:08b}", value, check);
    let (key, value) = tree
        .find_leaf(SliceData::from_raw(vec![value], 8), next, false, false, &mut 0)
        .unwrap()
        .unwrap();
    assert_eq!(value, SliceData::new(vec![check, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x: u8 = key.get_bits(0, 8).unwrap();
    assert_eq!(check, x);
}

#[test]
fn get_prev_in_tree_8_bits_unsigned() {
    let mut values = Vec::new();
    let tree = run_dictionary_8_bits(|(_, v)| values.push(v));

    assert_eq!(
        tree.find_leaf(SliceData::from_raw(vec![0], 8), false, false, false, &mut 0).unwrap(),
        None
    );

    values.sort_unstable();
    let mut start = 0;
    for (i, end) in values.iter().enumerate() {
        for v in start + 1..=*end {
            if i == 0 {
                assert_eq!(
                    tree.find_leaf(SliceData::from_raw(vec![v], 8), false, false, false, &mut 0)
                        .unwrap(),
                    None
                );
            } else {
                check_value_unsigned(&tree, v, start, false);
            }
        }
        start = *end;
    }
}

#[test]
fn get_next_in_tree_8_bits_unsigned() {
    let mut values = Vec::new();
    let tree = run_dictionary_8_bits(|(_, v)| values.push(v));

    values.sort_unstable();
    values.reverse();
    let mut end = 255;
    for (i, start) in values.iter().enumerate() {
        for v in *start..end {
            if i == 0 {
                assert_eq!(
                    tree.find_leaf(SliceData::from_raw(vec![v], 8), true, false, false, &mut 0)
                        .unwrap(),
                    None
                );
            } else {
                check_value_unsigned(&tree, v, end, true);
            }
        }
        end = *start;
    }
}

fn check_value_signed(tree: &HashmapE, value: i8, check: i8, next: bool) {
    if next {
        assert!(value < check);
    } else {
        assert!(value > check);
    }
    println!("check value {:08b} ({}) -> {:08b} ({})", value, value, check, check);
    let (key, value) = tree
        .find_leaf(SliceData::from_raw(vec![value as u8], 8), next, false, true, &mut 0)
        .unwrap()
        .unwrap();
    assert_eq!(value, SliceData::new(vec![check as u8, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x = key.get_bits(0, 8).unwrap() as i8;
    assert_eq!(check, x);
}

#[test]
fn get_prev_in_tree_8_bits_signed() {
    let mut values = Vec::new();
    let tree = run_dictionary_8_bits(|(_, v)| values.push(v as i8));
    check_value_signed(&tree, 0, -1, false);

    assert_eq!(
        tree.find_leaf(SliceData::from_raw(vec![0], 8), false, false, false, &mut 0).unwrap(),
        None
    );

    values.sort_unstable();
    let mut start = -128;
    for (i, end) in values.iter().enumerate() {
        for v in start + 1..=*end {
            if i == 0 {
                assert_eq!(
                    tree.find_leaf(
                        SliceData::from_raw(vec![v as u8], 8),
                        false,
                        false,
                        true,
                        &mut 0
                    )
                    .unwrap(),
                    None
                );
            } else {
                check_value_signed(&tree, v, start, false);
            }
        }
        start = *end;
    }
}

#[test]
fn get_next_in_tree_8_bits_signed() {
    let mut values = Vec::new();
    let tree = run_dictionary_8_bits(|(_, v)| values.push(v as i8));

    values.sort_unstable();
    values.reverse();
    let mut end = 127;
    for (i, start) in values.iter().enumerate() {
        for v in *start..end {
            if i == 0 {
                assert_eq!(
                    tree.find_leaf(
                        SliceData::from_raw(vec![v as u8], 8),
                        true,
                        false,
                        true,
                        &mut 0
                    )
                    .unwrap(),
                    None
                );
            } else {
                check_value_signed(&tree, v, end, true);
            }
        }
        end = *start;
    }
}

#[test]
fn set_delete_get_once() {
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    let key = SliceData::from_raw(vec![0b11111111], 8);
    let value = SliceData::new(vec![0b11111111, 0x80]);
    assert_eq!(tree.get(key.clone()).unwrap(), None);
    assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
    assert_eq!(tree.remove(key.clone()).unwrap(), Some(value));
    assert_eq!(tree.get(key).unwrap(), None);
    assert!(tree.is_empty());
}

#[test]
fn dictionary_delete_in_tree() {
    let tree = run_dictionary_8_bits(|(tree, v)| {
        let key = SliceData::from_raw(vec![v], 8);
        let value = SliceData::new(vec![v, 0x80]);
        println!("before remove {:08b}", v);
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
        assert_eq!(tree.remove(key.clone()).unwrap(), Some(value));
        println!("after remove {}", tree);
        assert_eq!(tree.get(key).unwrap(), None);
    });
    assert!(tree.is_empty());
}

#[test]
fn dictionary_find_min_max_in_empty_root() {
    let tree = HashmapE::with_bit_len(1);
    assert!(tree.is_empty());

    assert_eq!(tree.get_min(false, &mut 0).unwrap(), None);
    assert_eq!(tree.get_max(false, &mut 0).unwrap(), None);
}

#[test]
fn dictionary_find_min_in_singleton() {
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    assert_eq!(
        tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap(),
        None
    );

    assert_eq!(
        tree.get_min(false, &mut 0).unwrap().unwrap(),
        (BuilderData::with_raw(vec![0b11111111], 8).unwrap(), SliceData::new(vec![0b11111111]))
    );
}

#[test]
fn dictionary_find_min_in_tree_8_bits_signed() {
    let tree = run_dictionary_8_bits(|_| ());

    let (key, value) = tree.get_min(true, &mut 0).unwrap().unwrap();
    assert_eq!(value, SliceData::new(vec![0b10000000, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x: u8 = key.get_bits(0, 8).unwrap();
    assert_eq!(0b10000000, x);
}

#[test]
fn dictionary_find_min_in_tree_8_bits_unsigned() {
    let tree = run_dictionary_8_bits(|_| ());

    let (key, value) = tree.get_min(false, &mut 0).unwrap().unwrap();
    assert_eq!(value, SliceData::new(vec![0b00000000, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x: u8 = key.get_bits(0, 8).unwrap();
    assert_eq!(0b00000000, x);
}

#[test]
fn dictionary_find_max_in_singleton() {
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    let key = SliceData::from_raw(vec![0b11111111], 8);
    let value = SliceData::new(vec![0b11111111]);
    assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
    assert_eq!(tree.get_max(false, &mut 0).unwrap().unwrap(), (key.into_builder().unwrap(), value));
}

#[test]
fn dictionary_find_max_in_tree_8_bits_signed() {
    let tree = run_dictionary_8_bits(|_| ());

    let (key, value) = tree.get_max(true, &mut 0).unwrap().unwrap();
    assert_eq!(value, SliceData::new(vec![0b01111111, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x: u8 = key.get_bits(0, 8).unwrap();
    assert_eq!(0b01111111, x);
}

#[test]
fn dictionary_find_max_in_tree_8_bits_unsignend() {
    let tree = run_dictionary_8_bits(|_| ());

    let (key, value) = tree.get_max(false, &mut 0).unwrap().unwrap();
    assert_eq!(value, SliceData::new(vec![0b11111111, 0x80]));

    let key = SliceData::load_builder(key).unwrap();
    assert_eq!(key.remaining_bits(), 8);
    let x: u8 = key.get_bits(0, 8).unwrap();
    assert_eq!(0b11111111, x);
}

#[test]
fn test_by_spec() {
    let table = [
        (SliceData::from_raw(vec![0, 13], 16), SliceData::new(vec![0, 169, 0x80])),
        (SliceData::from_raw(vec![0, 17], 16), SliceData::new(vec![1, 33, 0x80])),
        (SliceData::from_raw(vec![0, 239], 16), SliceData::new(vec![223, 33, 0x80])),
    ];
    let mut tree = HashmapE::with_bit_len(16);
    table.iter().for_each(|(key, value)| {
        println!("put key:{}, value:{}", key, value);
        tree.set(key.clone(), value).unwrap();
        println!("{}", tree);
    });

    let mut root = SliceData::new(vec![0xC0]);
    let mut fork_0 = SliceData::new(vec![0xC8, 0x80]);
    let mut fork_0_0 = SliceData::new(vec![0x62]);
    let fork_0_0_0 = SliceData::new(vec![0xA6, 0x80, 0x54, 0xC0]);
    let fork_0_0_1 = SliceData::new(vec![0xA0, 0x80, 0x90, 0xC0]);
    let fork_0_1 = SliceData::new(vec![0xBE, 0xFD, 0xF2, 0x18]);
    fork_0_0.append_reference(fork_0_0_0).unwrap();
    fork_0_0.append_reference(fork_0_0_1).unwrap();
    fork_0.append_reference(fork_0_0).unwrap();
    fork_0.append_reference(fork_0_1).unwrap();
    root.append_reference(fork_0).unwrap();

    let mut builder = BuilderData::new();
    tree.write_hashmap_data(&mut builder).unwrap();
    assert_eq!(SliceData::load_builder(builder).unwrap(), root);
}

#[test]
fn test_dictionary_of_dictionaries() {
    let mut values = Vec::new();
    let tree1 = run_dictionary_8_bits(|(_, v)| values.push(v));
    let mut tree2 = HashmapE::with_bit_len(7);
    let key2_1 = SliceData::from_raw(vec![0xFF], 7);
    let val2_1 = SliceData::new(vec![0xFF]);
    assert_eq!(tree2.set(key2_1.clone(), &val2_1).unwrap(), None);
    assert_eq!(tree2.get(key2_1).unwrap(), Some(val2_1));

    let key2_2 = SliceData::from_raw(vec![0], 7);
    let val2_2 = SliceData::new(vec![1]);
    assert_eq!(tree2.set(key2_2.clone(), &val2_2).unwrap(), None);
    assert_eq!(tree2.get(key2_2).unwrap(), Some(val2_2));

    let mut root = HashmapE::with_bit_len(3);
    let key1 = SliceData::from_raw(vec![0xFF], 3);
    assert_eq!(root.setref(key1.clone(), tree1.data().unwrap()).unwrap(), None);
    assert_eq!(
        root.get(key1.clone()).unwrap().unwrap().reference(0).as_ref().unwrap(),
        tree1.data().unwrap()
    );

    let key2 = SliceData::from_raw(vec![0xC0], 3);
    assert_eq!(root.setref(key2.clone(), tree2.data().unwrap()).unwrap(), None);
    assert_eq!(
        root.get(key2.clone()).unwrap().unwrap().reference(0).as_ref().unwrap(),
        tree2.data().unwrap()
    );
    assert_eq!(
        root.get(key1).unwrap().unwrap().reference(0).as_ref().unwrap(),
        tree1.data().unwrap()
    );
    assert_eq!(
        root.get(key2).unwrap().unwrap().reference(0).as_ref().unwrap(),
        tree2.data().unwrap()
    );
}

#[test]
fn test_dictionary_in_data() {
    let mut values = Vec::new();
    let tree = run_dictionary_8_bits(|(_, v)| values.push(v));
    let mut builder = BuilderData::with_raw(vec![0b11010000], 3).unwrap();
    tree.write_hashmap_data(&mut builder).unwrap();
    let mut slice = SliceData::load_builder(builder).unwrap();

    if let Some(new_tree) = slice.get_dictionary_opt() {
        let new_tree = HashmapE::with_hashmap(8, new_tree.reference(0).ok());
        for v in values {
            let key = SliceData::from_raw(vec![v], 8);
            let value = SliceData::new(vec![v, 0x80]);
            assert_eq!(new_tree.get(key).unwrap(), Some(value));
        }
    }
}

#[test]
fn set_get_keys_hashmap() {
    let table = [0b11111111, 0b11110111, 0b11111001, 0b11111011];

    let mut tree = HashmapE::with_bit_len(7);
    assert!(tree.is_empty());

    for (i, v) in table.iter().enumerate() {
        println!("set {} - value {:8b}", i, v);
        let key = SliceData::from_raw(vec![*v], 7);
        let value = SliceData::new(vec![*v]);
        // assert_eq!(tree.get(key.clone()).unwrap(), None);
        assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
        println!("after {}", tree);
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
    }
}

fn run_remove(keys: &[u8]) {
    let mut slices: Vec<SliceData> = vec![];
    let mut tree = HashmapE::with_bit_len(8);
    assert!(tree.is_empty());

    for v in keys {
        let key = SliceData::from_raw(vec![*v], 8);
        let value = SliceData::new(vec![*v, 0x80]);
        log::trace!(target: "tests", "set key{} with value {} to tree {}", key, value, tree);
        assert_eq!(tree.get(key.clone()).unwrap(), None);
        assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
        slices.push(SliceData::load_cell_ref(tree.data().unwrap()).unwrap());
    }
    println!("tree: {}", tree);
    for v in keys.iter().rev() {
        let key = SliceData::from_raw(vec![*v], 8);
        let value = SliceData::new(vec![*v, 0x80]);
        assert_eq!(slices.pop().unwrap(), SliceData::load_cell_ref(tree.data().unwrap()).unwrap());
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
        assert_eq!(tree.remove(key.clone()).unwrap(), Some(value.clone()));
        assert_eq!(tree.get(key.clone()).unwrap(), None);
    }
    assert!(tree.is_empty());
}

#[test]
fn test_remove() {
    run_remove(&[0u8, 1, 2, 3, 32]);
    run_remove(&[128u8, 129, 254, 255]);
    run_remove(&[0u8, 1, 2, 3, 32, 128, 129, 254, 255]);
}

#[test]
fn test_special_remove() {
    let mut tree = HashmapE::with_bit_len(8);
    tree.set(SliceData::from_raw(vec![255], 8), &SliceData::new(vec![0b1111_0000])).unwrap();
    tree.set(SliceData::from_raw(vec![0], 8), &SliceData::new(vec![0b0001_0000])).unwrap();
    tree.set(SliceData::from_raw(vec![2], 8), &SliceData::new(vec![0b0101_0000])).unwrap();
    println!("{}", tree);

    tree.set(SliceData::from_raw(vec![3], 8), &SliceData::new(vec![0b0111_0000])).unwrap();
    println!("{}", tree);
    assert_eq!(
        tree.remove(SliceData::from_raw(vec![3], 8)).unwrap(),
        Some(SliceData::new(vec![0b0111_0000]))
    );
    println!("{}", tree);
    assert_eq!(tree.get(SliceData::from_raw(vec![3], 8)).unwrap(), None);
    assert_eq!(tree.remove(SliceData::from_raw(vec![3], 8)).unwrap(), None);

    tree.set(SliceData::from_raw(vec![1], 8), &SliceData::new(vec![0b0011_0000])).unwrap();
    println!("{}", tree);

    assert_eq!(
        tree.remove(SliceData::from_raw(vec![1], 8)).unwrap(),
        Some(SliceData::new(vec![0b0011_0000]))
    );
    println!("{}", tree);
    assert_eq!(tree.get(SliceData::from_raw(vec![1], 8)).unwrap(), None);
    assert_eq!(tree.remove(SliceData::from_raw(vec![1], 8)).unwrap(), None);
    println!("{}", tree);
}

fn make_big_tree() -> HashmapE {
    let mut tree = HashmapE::with_bit_len(8);
    tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000])).unwrap();

    tree.set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11110011], 8), &SliceData::new(vec![0b11110011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11000011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();

    tree.set(SliceData::from_raw(vec![0b10111111], 8), &SliceData::new(vec![0b10111111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10111100], 8), &SliceData::new(vec![0b10111100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10110011], 8), &SliceData::new(vec![0b10110011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10110000], 8), &SliceData::new(vec![0b10110000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10001111], 8), &SliceData::new(vec![0b10001111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10001100], 8), &SliceData::new(vec![0b10001100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10000011], 8), &SliceData::new(vec![0b10000011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b10000000], 8), &SliceData::new(vec![0b10000000])).unwrap();
    tree
}
fn make_tree_with_filled_root_label() -> HashmapE {
    let mut tree = HashmapE::with_bit_len(8);
    tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11110011], 8), &SliceData::new(vec![0b11110011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11000011])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();
    tree
}
fn make_tree_with_empty_root_label() -> HashmapE {
    let mut tree = HashmapE::with_bit_len(8);
    tree.set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b00111100], 8), &SliceData::new(vec![0b00111100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b00110000], 8), &SliceData::new(vec![0b00110000])).unwrap();
    tree.set(SliceData::from_raw(vec![0b00001100], 8), &SliceData::new(vec![0b00001100])).unwrap();
    tree.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000])).unwrap();
    tree
}

mod subtree_with_prefix {
    use super::*;

    #[test]
    fn uncommon_prefix_len() {
        let tree = make_tree_with_filled_root_label();
        let tree = tree.subtree_with_prefix(&SliceData::new_empty(), &mut 0).unwrap();
        assert_eq!(tree, make_tree_with_filled_root_label());

        let tree = make_tree_with_empty_root_label();
        let tree = tree.subtree_with_prefix(&SliceData::new_empty(), &mut 0).unwrap();
        assert_eq!(tree, make_tree_with_empty_root_label());
    }

    #[test]
    fn empty_tree() {
        let tree = HashmapE::with_bit_len(8);
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11111111], 1), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(8));

        let tree = HashmapE::with_bit_len(8);
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11111111], 2), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(8));

        let tree = HashmapE::with_bit_len(8);
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11111111], 8), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(8));

        let tree = HashmapE::with_bit_len(4);
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00000000], 1), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(4));

        let tree = HashmapE::with_bit_len(4);
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00000000], 4), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(4));
    }

    #[test]
    fn subtree_missing() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00000000], 1), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(8));

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11001000], 6), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(8));

        let mut tree = HashmapE::with_bit_len(16);
        tree.set(
            SliceData::from_raw(vec![0b00000000, 0b00001010], 16),
            &SliceData::new(vec![0b11111111]),
        )
        .unwrap();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00000010], 8), &mut 0).unwrap();
        assert_eq!(tree, HashmapE::with_bit_len(16));
    }

    #[test]
    fn normal_flow_with_filled_root_label() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11001100], 6), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 3), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11001000], 5), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn normal_flow_with_empty_root_label() {
        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11001100], 6), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 3), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11001000], 5), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn normal_flow_in_big_tree() {
        let tree = make_big_tree();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_big_tree();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b10000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b10001111], 8), &SliceData::new(vec![0b10001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b10001100], 8), &SliceData::new(vec![0b10001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b10000011], 8), &SliceData::new(vec![0b10000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b10000000], 8), &SliceData::new(vec![0b10000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_big_tree();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 2), &mut 0).unwrap();
        assert_eq!(tree, make_tree_with_filled_root_label());
    }

    #[test]
    fn subtree_is_one_elem() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11110010], 7), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11110011], 8), &SliceData::new(vec![0b11110011]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11110000], 7), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11110000], 5), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00001100], 6), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b00001100], 8), &SliceData::new(vec![0b00001100]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00001100], 7), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b00001100], 8), &SliceData::new(vec![0b00001100]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn subtree_is_whole_tree() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 1), &mut 0).unwrap();
        assert_eq!(tree, make_tree_with_filled_root_label());
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 2), &mut 0).unwrap();
        assert_eq!(tree, make_tree_with_filled_root_label());
    }

    #[test]
    fn subtree_is_root_branch() {
        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 2), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11000000], 1), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn tree_with_one_item() {
        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b11111111], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00010010], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b00010010], 1), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(8);
        expected
            .set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(2);
        tree.set(SliceData::from_raw(vec![0b10000000], 2), &SliceData::new(vec![0b10000000]))
            .unwrap();
        let tree =
            tree.subtree_with_prefix(&SliceData::from_raw(vec![0b10000000], 1), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(2);
        expected
            .set(SliceData::from_raw(vec![0b10000000], 2), &SliceData::new(vec![0b10000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }
}

mod subtree_without_prefix {
    use super::*;

    #[test]
    fn normal_flow_with_empty_label() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11110000], 4), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 4), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00110000], 4), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 4), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = expected
            .subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 2), &mut 0)
            .unwrap();
        let mut expected = HashmapE::with_bit_len(2);
        expected
            .set(SliceData::from_raw(vec![0b11000000], 2), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 2), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11000000], 4), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 4), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = expected
            .subtree_without_prefix(&SliceData::from_raw(vec![0b00000000], 2), &mut 0)
            .unwrap();
        let mut expected = HashmapE::with_bit_len(2);
        expected
            .set(SliceData::from_raw(vec![0b00000000], 2), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn normal_flow_with_some_label() {
        let tree = make_tree_with_filled_root_label();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 3), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(5);
        expected
            .set(SliceData::from_raw(vec![0b01111000], 5), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b01100000], 5), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00011000], 5), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 5), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = expected
            .subtree_without_prefix(&SliceData::from_raw(vec![0b01100000], 3), &mut 0)
            .unwrap();
        let mut expected = HashmapE::with_bit_len(2);
        expected
            .set(SliceData::from_raw(vec![0b11000000], 2), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 2), &SliceData::new(vec![0b11001100]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_tree_with_empty_root_label();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 3), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(5);
        expected
            .set(SliceData::from_raw(vec![0b01100000], 5), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 5), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = expected
            .subtree_without_prefix(&SliceData::from_raw(vec![0b00000000], 1), &mut 0)
            .unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11000000], 4), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 4), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn tree_with_one_item() {
        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b11111111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11111111], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11111111], 4), &SliceData::new(vec![0b11111111]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b00010010], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b00100000], 4), &SliceData::new(vec![0b00010010]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(8);
        tree.set(SliceData::from_raw(vec![0b00010010], 8), &SliceData::new(vec![0b00010010]))
            .unwrap();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b00010010], 1), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(7);
        expected
            .set(SliceData::from_raw(vec![0b00100100], 7), &SliceData::new(vec![0b00010010]))
            .unwrap();
        assert_eq!(tree, expected);

        let mut tree = HashmapE::with_bit_len(2);
        tree.set(SliceData::from_raw(vec![0b10000000], 2), &SliceData::new(vec![0b10000000]))
            .unwrap();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b10000000], 1), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(1);
        expected
            .set(SliceData::from_raw(vec![0b00000000], 1), &SliceData::new(vec![0b10000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }

    #[test]
    fn normal_flow_in_big_tree() {
        let tree = make_big_tree();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b11000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11110000], 4), &SliceData::new(vec![0b11001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 4), &SliceData::new(vec![0b11001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00110000], 4), &SliceData::new(vec![0b11000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 4), &SliceData::new(vec![0b11000000]))
            .unwrap();
        assert_eq!(tree, expected);

        let tree = make_big_tree();
        let tree =
            tree.subtree_without_prefix(&SliceData::from_raw(vec![0b10000000], 4), &mut 0).unwrap();
        let mut expected = HashmapE::with_bit_len(4);
        expected
            .set(SliceData::from_raw(vec![0b11110000], 4), &SliceData::new(vec![0b10001111]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b11000000], 4), &SliceData::new(vec![0b10001100]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00110000], 4), &SliceData::new(vec![0b10000011]))
            .unwrap();
        expected
            .set(SliceData::from_raw(vec![0b00000000], 4), &SliceData::new(vec![0b10000000]))
            .unwrap();
        assert_eq!(tree, expected);
    }
}

mod hashmap_replace {
    use super::*;

    #[test]
    fn test_key_already_present_in_big_tree() {
        let mut tree = make_big_tree();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11001111], 8),
                &SliceData::new(vec![0b11001111]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11001111])
        );
        assert_eq!(tree, make_big_tree());
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11001111], 8),
                &SliceData::new(vec![0b11111111]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11001111])
        );
        let mut expected = make_big_tree();
        expected
            .set(SliceData::from_raw(vec![0b11001111], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        assert_eq!(tree, expected);

        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11001111], 8),
                &SliceData::new(vec![0b11001111]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11111111])
        );
        assert_eq!(tree, make_big_tree());
    }

    #[test]
    fn test_key_already_present_in_tree_with_filled_root_label() {
        let mut tree = make_tree_with_filled_root_label();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11000011], 8),
                &SliceData::new(vec![0b11000011]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11000011])
        );
        assert_eq!(tree, make_tree_with_filled_root_label());
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11000011], 8),
                &SliceData::new(vec![0b11111111]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11000011])
        );
        let mut expected = make_tree_with_filled_root_label();
        expected
            .set(SliceData::from_raw(vec![0b11000011], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        assert_eq!(tree, expected);

        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11000011], 8),
                &SliceData::new(vec![0b11000011]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11111111])
        );
        assert_eq!(tree, make_tree_with_filled_root_label());
    }

    #[test]
    fn test_key_already_present_in_tree_with_empty_root_label() {
        let mut tree = make_tree_with_empty_root_label();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b00111100], 8),
                &SliceData::new(vec![0b00111100]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b00111100])
        );
        assert_eq!(tree, make_tree_with_empty_root_label());
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b00111100], 8),
                &SliceData::new(vec![0b11111111]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b00111100])
        );
        let mut expected = make_tree_with_empty_root_label();
        expected
            .set(SliceData::from_raw(vec![0b00111100], 8), &SliceData::new(vec![0b11111111]))
            .unwrap();
        assert_eq!(tree, expected);

        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b00111100], 8),
                &SliceData::new(vec![0b00111100]),
                &mut 0
            )
            .unwrap()
            .unwrap(),
            SliceData::new(vec![0b11111111])
        );
        assert_eq!(tree, make_tree_with_empty_root_label());
    }

    #[test]
    fn test_key_is_absent_in_big_tree() {
        let mut tree = make_big_tree();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b10010011], 8),
                &SliceData::new(vec![0b10010011]),
                &mut 0
            )
            .unwrap(),
            None
        );
        assert_eq!(tree, make_big_tree());
    }

    #[test]
    fn test_key_is_absent_in_tree_with_filled_root_label() {
        let mut tree = make_tree_with_filled_root_label();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b11010011], 8),
                &SliceData::new(vec![0b11010011]),
                &mut 0
            )
            .unwrap(),
            None
        );
        assert_eq!(tree, make_tree_with_filled_root_label());
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b01010011], 8),
                &SliceData::new(vec![0b01010011]),
                &mut 0
            )
            .unwrap(),
            None
        );
        assert_eq!(tree, make_tree_with_filled_root_label());
    }

    #[test]
    fn test_key_is_absent_in_tree_with_empty_root_label() {
        let mut tree = make_tree_with_empty_root_label();
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b00110100], 8),
                &SliceData::new(vec![0b00110100]),
                &mut 0
            )
            .unwrap(),
            None
        );
        assert_eq!(tree, make_tree_with_empty_root_label());
        assert_eq!(
            tree.replace_with_gas(
                SliceData::from_raw(vec![0b01110100], 8),
                &SliceData::new(vec![0b01110100]),
                &mut 0
            )
            .unwrap(),
            None
        );
        assert_eq!(tree, make_tree_with_empty_root_label());
    }
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
    let mut left = HashmapE::with_bit_len(8);
    left.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();
    let mut right = HashmapE::with_bit_len(8);
    right.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000])).unwrap();
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 2);
    let mut result = HashmapE::with_bit_len(8);
    result
        .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
        .unwrap();
    result
        .set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000]))
        .unwrap();
    assert_eq!(left, result);

    let mut left = HashmapE::with_bit_len(8);
    let mut right = HashmapE::with_bit_len(8);
    right.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000])).unwrap();
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 1);
    assert_eq!(left, right);

    let mut left = HashmapE::with_bit_len(8);
    left.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();
    let right = HashmapE::with_bit_len(8);
    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 1);
    let mut result = HashmapE::with_bit_len(8);
    result
        .set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000]))
        .unwrap();
    assert_eq!(left, result);

    let tree = make_tree_with_empty_root_label();
    let mut left = HashmapE::with_bit_len(8);
    left.set(SliceData::from_raw(vec![0b11111100], 8), &SliceData::new(vec![0b11111100])).unwrap();
    left.set(SliceData::from_raw(vec![0b11110000], 8), &SliceData::new(vec![0b11110000])).unwrap();
    left.set(SliceData::from_raw(vec![0b11001100], 8), &SliceData::new(vec![0b11001100])).unwrap();
    left.set(SliceData::from_raw(vec![0b11000000], 8), &SliceData::new(vec![0b11000000])).unwrap();

    let mut right = HashmapE::with_bit_len(8);
    right.set(SliceData::from_raw(vec![0b00111100], 8), &SliceData::new(vec![0b00111100])).unwrap();
    right.set(SliceData::from_raw(vec![0b00110000], 8), &SliceData::new(vec![0b00110000])).unwrap();
    right.set(SliceData::from_raw(vec![0b00001100], 8), &SliceData::new(vec![0b00001100])).unwrap();
    right.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0b00000000])).unwrap();

    assert_eq!(left.len().unwrap(), 4);
    assert_eq!(right.len().unwrap(), 4);

    left.merge(&right, &SliceData::new(vec![0x80])).unwrap();
    assert_eq!(left.len().unwrap(), 8);
    assert_eq!(tree, left);
}

#[test]
fn test_scan_diff_main() {
    let mut tree_1 = HashmapE::with_bit_len(8);
    tree_1.set(SliceData::from_raw(vec![0], 8), &SliceData::new(vec![0])).unwrap();
    tree_1.set(SliceData::from_raw(vec![1], 8), &SliceData::new(vec![2])).unwrap();
    tree_1.set(SliceData::from_raw(vec![2], 8), &SliceData::new(vec![2])).unwrap();
    tree_1.set(SliceData::from_raw(vec![3], 8), &SliceData::new(vec![7])).unwrap();
    tree_1.set(SliceData::from_raw(vec![4], 8), &SliceData::new(vec![4])).unwrap();

    let mut tree_2 = HashmapE::with_bit_len(8);
    tree_2.set(SliceData::from_raw(vec![0], 8), &SliceData::new(vec![0])).unwrap();
    tree_2.set(SliceData::from_raw(vec![1], 8), &SliceData::new(vec![1])).unwrap();
    tree_2.set(SliceData::from_raw(vec![3], 8), &SliceData::new(vec![3])).unwrap();
    tree_2.set(SliceData::from_raw(vec![4], 8), &SliceData::new(vec![4])).unwrap();
    tree_2.set(SliceData::from_raw(vec![5], 8), &SliceData::new(vec![5])).unwrap();
    tree_2.set(SliceData::from_raw(vec![6], 8), &SliceData::new(vec![5])).unwrap();
    tree_2.set(SliceData::from_raw(vec![10], 8), &SliceData::new(vec![9])).unwrap();

    let correct_dif = vec![
        (SliceData::from_raw(vec![2], 8), Some(SliceData::new(vec![2])), None),
        (
            SliceData::from_raw(vec![3], 8),
            Some(SliceData::new(vec![7])),
            Some(SliceData::new(vec![3])),
        ),
        (
            SliceData::from_raw(vec![1], 8),
            Some(SliceData::new(vec![2])),
            Some(SliceData::new(vec![1])),
        ),
        (SliceData::from_raw(vec![5], 8), None, Some(SliceData::new(vec![5]))),
        (SliceData::from_raw(vec![6], 8), None, Some(SliceData::new(vec![5]))),
        (SliceData::from_raw(vec![10], 8), None, Some(SliceData::new(vec![9]))),
    ];

    scan_diff_and_compare(tree_1, tree_2, correct_dif);
}

#[test]
fn test_scan_diff_main2() {
    let mut tree_1 = HashmapE::with_bit_len(8);
    tree_1.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0])).unwrap();
    tree_1.set(SliceData::from_raw(vec![0b00000001], 8), &SliceData::new(vec![2])).unwrap();
    tree_1.set(SliceData::from_raw(vec![0b00000010], 8), &SliceData::new(vec![2])).unwrap();
    tree_1.set(SliceData::from_raw(vec![0b00000011], 8), &SliceData::new(vec![7])).unwrap();
    tree_1.set(SliceData::from_raw(vec![0b00000100], 8), &SliceData::new(vec![4])).unwrap();
    tree_1.set(SliceData::from_raw(vec![0b00001010], 8), &SliceData::new(vec![9])).unwrap();

    let mut tree_2 = HashmapE::with_bit_len(8);
    tree_2.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000001], 8), &SliceData::new(vec![1])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000011], 8), &SliceData::new(vec![3])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000100], 8), &SliceData::new(vec![4])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000101], 8), &SliceData::new(vec![5])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000110], 8), &SliceData::new(vec![5])).unwrap();

    let correct_dif = vec![
        (SliceData::from_raw(vec![2], 8), Some(SliceData::new(vec![2])), None),
        (
            SliceData::from_raw(vec![3], 8),
            Some(SliceData::new(vec![7])),
            Some(SliceData::new(vec![3])),
        ),
        (
            SliceData::from_raw(vec![1], 8),
            Some(SliceData::new(vec![2])),
            Some(SliceData::new(vec![1])),
        ),
        (SliceData::from_raw(vec![5], 8), None, Some(SliceData::new(vec![5]))),
        (SliceData::from_raw(vec![6], 8), None, Some(SliceData::new(vec![5]))),
        (SliceData::from_raw(vec![10], 8), Some(SliceData::new(vec![9])), None),
    ];

    scan_diff_and_compare(tree_1, tree_2, correct_dif);
}

#[test]
fn test_scan_diff_main3() {
    let mut tree_1 = HashmapE::with_bit_len(8);
    tree_1.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0])).unwrap();

    let mut tree_2 = HashmapE::with_bit_len(8);
    tree_2.set(SliceData::from_raw(vec![0b00000000], 8), &SliceData::new(vec![0])).unwrap();
    tree_2.set(SliceData::from_raw(vec![0b00000001], 8), &SliceData::new(vec![1])).unwrap();

    let correct_dif = vec![(SliceData::from_raw(vec![1], 8), None, Some(SliceData::new(vec![1])))];

    scan_diff_and_compare(tree_1, tree_2, correct_dif);
}

#[test]
fn test_scan_diff_main_single_different() {
    let mut tree_1 = HashmapE::with_bit_len(8);
    tree_1.set(SliceData::from_raw(vec![0], 8), &SliceData::new(vec![0])).unwrap();

    let mut tree_2 = HashmapE::with_bit_len(8);
    tree_2.set(SliceData::from_raw(vec![1], 8), &SliceData::new(vec![1])).unwrap();

    let correct_dif = vec![
        (SliceData::from_raw(vec![0], 8), Some(SliceData::new(vec![0])), None),
        (SliceData::from_raw(vec![1], 8), None, Some(SliceData::new(vec![1]))),
    ];

    scan_diff_and_compare(tree_1, tree_2, correct_dif);
}

#[test]
fn test_scan_diff_empty() {
    let mut tree_1 = HashmapE::with_bit_len(8);
    tree_1.set(SliceData::from_raw(vec![0], 8), &SliceData::new(vec![0])).unwrap();
    tree_1.set(SliceData::from_raw(vec![1], 8), &SliceData::new(vec![1])).unwrap();
    tree_1.set(SliceData::from_raw(vec![2], 8), &SliceData::new(vec![2])).unwrap();
    tree_1.set(SliceData::from_raw(vec![3], 8), &SliceData::new(vec![3])).unwrap();
    tree_1.set(SliceData::from_raw(vec![4], 8), &SliceData::new(vec![4])).unwrap();

    let tree_2 = HashmapE::with_bit_len(8);

    let correct_dif = vec![
        (SliceData::from_raw(vec![0], 8), Some(SliceData::new(vec![0])), None),
        (SliceData::from_raw(vec![1], 8), Some(SliceData::new(vec![1])), None),
        (SliceData::from_raw(vec![2], 8), Some(SliceData::new(vec![2])), None),
        (SliceData::from_raw(vec![3], 8), Some(SliceData::new(vec![3])), None),
        (SliceData::from_raw(vec![4], 8), Some(SliceData::new(vec![4])), None),
    ];

    scan_diff_and_compare(tree_1, tree_2, correct_dif);
}

fn scan_diff_and_compare(
    tree_1: HashmapE,
    tree_2: HashmapE,
    mut correct_dif: Vec<(SliceData, Option<SliceData>, Option<SliceData>)>,
) {
    correct_dif.sort_unstable();
    let mut diff_vec = vec![];

    assert!(tree_1
        .scan_diff(&tree_2, |key, value1, value2| {
            if let Some(ref k) = value1 {
                println!("ready value1 {}", k);
            }
            if let Some(ref k) = value2 {
                println!("ready value2 {}", k);
            }
            println!("ready key {}", key);
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap());
    assert_eq!(correct_dif.len(), diff_vec.len());

    diff_vec.sort_unstable();
    assert_eq!(diff_vec, correct_dif);

    let mut diff_vec = vec![];
    assert!(tree_2
        .scan_diff(&tree_1, |key, value2, value1| {
            if let Some(ref k) = value1 {
                println!("ready value1 {}", k);
            }
            if let Some(ref k) = value2 {
                println!("ready value2 {}", k);
            }
            println!("ready key {}", key);
            diff_vec.push((key, value1, value2));
            Ok(true)
        })
        .unwrap());
    assert_eq!(correct_dif.len(), diff_vec.len());

    diff_vec.sort_unstable();
    assert_eq!(diff_vec, correct_dif);
}

#[test]
fn test_is_single_item_in_hashmap() {
    let mut tree = HashmapE::with_bit_len(8);
    tree.set(SliceData::from_raw(vec![0b0000_0000], 8), &SliceData::new(vec![1])).unwrap();

    assert_eq!(tree.is_single().unwrap().unwrap().1, SliceData::new(vec![1]));

    tree.set(SliceData::from_raw(vec![0b0111_1111], 8), &SliceData::new(vec![2])).unwrap();
    assert_eq!(tree.is_single().unwrap(), None);

    tree.set(SliceData::from_raw(vec![0b1111_1111], 8), &SliceData::new(vec![3])).unwrap();
    assert_eq!(tree.is_single().unwrap(), None);
}

fn check_tree_iterator(tree: &HashmapE) {
    let len = tree.len().unwrap();

    let mut res0: Vec<(SliceData, SliceData)> = vec![];
    let mut res1: Vec<(SliceData, SliceData)> = vec![];
    let mut res2: Vec<(SliceData, SliceData)> = vec![];

    for k_v in tree {
        let (k, v) = k_v.unwrap();
        res0.push((SliceData::load_builder(k).unwrap(), v));
    }
    assert_eq!(res0.len(), len);

    let mut iter = tree.iter();
    for k_v in &mut iter {
        let (k, v) = k_v.unwrap();
        res1.push((SliceData::load_builder(k).unwrap(), v));
    }
    assert!(iter.next().is_none());

    tree.iterate_slices(|k, v| {
        res2.push((k, v));
        Ok(true)
    })
    .unwrap();
    res0.sort();
    res1.sort();
    res2.sort();
    assert_eq!(res0, res1);
    assert_eq!(res0, res2);
}

#[test]
fn test_hashmap_iterator() {
    check_tree_iterator(&make_big_tree());
    check_tree_iterator(&make_tree_with_filled_root_label());
    check_tree_iterator(&make_tree_with_empty_root_label());
    let mut tree = HashmapE::with_bit_len(8);
    check_tree_iterator(&tree);
    tree.set(SliceData::from_raw(vec![0b0000_0000], 8), &SliceData::new(vec![1])).unwrap();
    check_tree_iterator(&tree);
}

fn check_hashmap_fill_and_filter(
    mut keys: Vec<u8>,
    remove: &[u8],
    stop_index: usize,
    cancel_index: usize,
) {
    keys.sort_unstable();
    println!("\nKEYS {:?}", keys);
    let mut queue1 = HashmapE::with_bit_len(8);
    let mut queue2 = queue1.clone();
    for i in 0..keys.len() {
        let key = keys[i];
        let val = i as u8 + 1;
        let val = SliceData::from_raw(vec![val], 8);
        let slice = SliceData::from_raw(vec![key], 8);
        assert!(
            queue1.get(slice.clone()).unwrap().is_none(),
            "generated two equal random keys - try to restart test"
        );
        queue1.set(slice.clone(), &val).unwrap();
        #[allow(clippy::if_same_then_else)]
        if cancel_index <= stop_index && cancel_index < keys.len() {
            queue2.set(slice.clone(), &val).unwrap();
        } else if i >= stop_index {
            queue2.set(slice.clone(), &val).unwrap();
        } else if !remove.contains(&key) {
            queue2.set(slice.clone(), &val).unwrap();
        }
    }
    // queue1.dump();
    // println!("{:#.3}", queue1.data().cloned().unwrap());

    queue1
        .hashmap_filter(|key, _val| {
            let key = SliceData::load_builder(key.clone()).unwrap().get_next_byte()?;
            if keys.get(cancel_index) == Some(&key) {
                println!("CANCEL: {}", key);
                Ok(HashmapFilterResult::Cancel)
            } else if keys.get(stop_index) == Some(&key) {
                println!("STOP: {}", key);
                Ok(HashmapFilterResult::Stop)
            } else if remove.contains(&key) {
                println!("DEL: {}", key);
                Ok(HashmapFilterResult::Remove)
            } else {
                println!("STAY: {}", key);
                Ok(HashmapFilterResult::Accept)
            }
        })
        .unwrap();
    queue1
        .hashmap_filter(|key, _val| {
            let key = SliceData::load_builder(key.clone()).unwrap().get_next_byte()?;
            println!("PRINT {}", key);
            Ok(HashmapFilterResult::Accept)
        })
        .unwrap();
    let mut res1 = vec![];
    queue1
        .iterate_slices(|key, val| {
            res1.push((key, val));
            Ok(true)
        })
        .unwrap();
    // println!("{:#.3}", queue1.data().cloned().unwrap_or_default());
    // assert_eq!(queue, queue2);
    // additional testing
    let mut res2 = vec![];
    queue2
        .iterate_slices(|key, val| {
            res2.push((key, val));
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
    check_hashmap_fill_and_filter(vec![65, 76, 150, 202], &[76], 4, 4);
    check_hashmap_fill_and_filter(vec![65, 76, 150, 202], &[76], 2, 3);
    check_hashmap_fill_and_filter(vec![202, 65, 76, 150], &[202, 65, 76], 0, 4);
    check_hashmap_fill_and_filter(vec![202, 65, 76, 150], &[202, 65, 76], 1, 4);
    check_hashmap_fill_and_filter(vec![202, 65, 76, 150], &[202, 65, 76], 2, 4);
    check_hashmap_fill_and_filter(vec![202, 65, 76, 150], &[202, 65, 76], 3, 4);
    check_hashmap_fill_and_filter(vec![202, 65, 76, 150], &[202, 65, 76], 4, 4);
    check_hashmap_fill_and_filter(vec![133, 167, 222], &[167], 2, 4);
}

#[test]
fn test_signed_lookup_only_negative() {
    let mut tree = HashmapE::with_bit_len(8);
    let key3 = SliceData::from_raw(vec![(-3i8) as u8], 8);
    let val3 = SliceData::from_raw(vec![0x33], 8);
    tree.set(key3.clone(), &val3).unwrap();
    let key5 = SliceData::from_raw(vec![(-5i8) as u8], 8);
    let val5 = SliceData::from_raw(vec![0x55], 8);
    tree.set(key5.clone(), &val5).unwrap();
    let key4 = SliceData::from_raw(vec![(-4i8) as u8], 8);
    let val4 = SliceData::from_raw(vec![0x44], 8);
    tree.set(key4.clone(), &val4).unwrap();

    assert_eq!(tree.get(key3.clone()).unwrap().unwrap(), val3);
    assert_eq!(tree.get(key4.clone()).unwrap().unwrap(), val4);
    assert_eq!(tree.get(key5).unwrap().unwrap(), val5);

    let (_k, v) = tree.find_leaf(key4.clone(), false, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);

    let (_k, v) = tree.get_min(false, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);
    let (_k, v) = tree.get_max(false, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let (_k, v) = tree.get_min(true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);
    let (_k, v) = tree.get_max(true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let key1 = SliceData::from_raw(vec![100], 8);
    let (_k, v) = tree.find_leaf(key1, false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let (_k, v) = tree.find_leaf(key4.clone(), false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);

    let (_k, v) = tree.find_leaf(key4, false, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);

    let (_k, v) = tree.find_leaf(key3.clone(), false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let (_k, v) = tree.find_leaf(key3, false, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);
}

#[test]
fn test_signed_lookup_positive_and_negative() {
    let mut tree = HashmapE::with_bit_len(8);
    let key3 = SliceData::from_raw(vec![(-3i8) as u8], 8);
    let val3 = SliceData::from_raw(vec![0x33], 8);
    tree.set(key3.clone(), &val3).unwrap();
    let key5 = SliceData::from_raw(vec![(-5i8) as u8], 8);
    let val5 = SliceData::from_raw(vec![0x55], 8);
    tree.set(key5.clone(), &val5).unwrap();
    let key4 = SliceData::from_raw(vec![4], 8);
    let val4 = SliceData::from_raw(vec![0x44], 8);
    tree.set(key4.clone(), &val4).unwrap();

    println!("{:#.3}", tree.data().unwrap());

    assert_eq!(tree.get(key3.clone()).unwrap().unwrap(), val3);
    assert_eq!(tree.get(key4.clone()).unwrap().unwrap(), val4);
    assert_eq!(tree.get(key5.clone()).unwrap().unwrap(), val5);

    let (_k, v) = tree.get_min(false, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);
    let (_k, v) = tree.get_max(false, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let (_k, v) = tree.get_min(true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);
    let (_k, v) = tree.get_max(true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);

    let key1 = SliceData::from_raw(vec![100], 8);
    let (_k, v) = tree.find_leaf(key1, false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);

    let (_k, v) = tree.find_leaf(key4.clone(), false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);

    let (_k, v) = tree.find_leaf(key4.clone(), false, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    assert!(tree.find_leaf(key4, true, false, true, &mut 0).unwrap().is_none());

    let (_k, v) = tree.find_leaf(key5, false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);

    let (_k, v) = tree.find_leaf(key3.clone(), false, true, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val3);

    let (_k, v) = tree.find_leaf(key3.clone(), false, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val5);

    let (_k, v) = tree.find_leaf(key3, true, false, true, &mut 0).unwrap().unwrap();
    assert_eq!(v, val4);
}

#[test]
fn test_combine_hashmaps() {
    let init1 = [(0b0000_0000, 0x33), (0b0011_0000, 0x55)];
    combine_trees(&init1, &[(0b0000_0000, 0x33)], false).unwrap();
    combine_trees(&init1, &[(0b0000_0001, 0x77)], true).unwrap();
    combine_trees(&init1, &[(0b1111_0000, 0x77)], true).unwrap();
    combine_trees(&init1, &[(0b0111_0000, 0x77)], true).unwrap();

    combine_trees(&init1, &[(0b0000_0011, 0x77), (0b0000_0111, 0x66)], true).unwrap();
    combine_trees(&init1, &[(0b0000_0001, 0x77), (0b0000_0011, 0x66)], true).unwrap();
    combine_trees(&init1, &[(0b0000_0000, 0x33), (0b0000_0001, 0x66)], true).unwrap();
    combine_trees(&init1, &[(0b0011_0011, 0x77), (0b0011_0111, 0x66)], true).unwrap();

    let init2 =
        [(0b0000_0000, 0x31), (0b0001_0000, 0x33), (0b0010_0000, 0x35), (0b0011_0000, 0x37)];
    combine_trees(
        &init1,
        &[(0b0000_0001, 0x21), (0b0001_0001, 0x23), (0b0010_0001, 0x25), (0b0011_0001, 0x27)],
        true,
    )
    .unwrap();
    combine_trees(
        &init2,
        &[(0b0000_0001, 0x21), (0b0001_0001, 0x23), (0b0010_0001, 0x25), (0b0011_0001, 0x27)],
        true,
    )
    .unwrap();
}

fn combine_trees(init1: &[(u8, u8)], init2: &[(u8, u8)], combine_result: bool) -> Result<()> {
    let mut tree1 = HashmapE::with_bit_len(8);
    for (key, val) in init1 {
        let key = SliceData::from_raw(vec![*key], 8);
        let val = SliceData::from_raw(vec![*val], 8);
        tree1.set(key, &val)?;
    }
    let mut tree = tree1.clone();
    let mut tree2 = HashmapE::with_bit_len(8);
    for (key, val) in init2 {
        let key = SliceData::from_raw(vec![*key], 8);
        let val = SliceData::from_raw(vec![*val], 8);
        tree.set(key.clone(), &val)?;
        tree2.set(key, &val)?;
    }
    assert_eq!(tree1.combine_with(&tree2)?, combine_result);
    tree1.scan_diff(&tree, |key, _val1, _val2| {
        println!("{:x} {:?} {:?}", key, _val1, _val2);
        Ok(true)
    })?;
    assert_eq!(tree1, tree);
    Ok(())
}

#[test]
fn subtree_wrong_prefix() {
    let tree = make_tree_with_filled_root_label();
    // no subtree with such prefix
    let wrong_prefix = SliceData::from_raw(vec![0b11111000], 6);

    // new sub_tree is empty but bit_len was shorted
    let sub_tree = tree.subtree_without_prefix(&wrong_prefix, &mut 0).unwrap();
    assert!(sub_tree.is_empty());
    assert_eq!(sub_tree.bit_len(), 2);

    // new sub_tree is empty with the same bit_len
    let sub_tree = tree.subtree_with_prefix(&wrong_prefix, &mut 0).unwrap();
    assert!(sub_tree.is_empty());
    assert_eq!(sub_tree.bit_len(), 8);

    assert!(tree.subtree_root_cell(&wrong_prefix).unwrap().is_none());

    // too long prefix
    let wrong_prefix = SliceData::from_raw(vec![0b11111100, 0], 9);

    // cannot create subtree with such prefix
    tree.subtree_without_prefix(&wrong_prefix, &mut 0).unwrap_err();

    // cannot create subtree with such prefix
    tree.subtree_with_prefix(&wrong_prefix, &mut 0).unwrap_err();

    tree.subtree_root_cell(&wrong_prefix).unwrap_err();
}
