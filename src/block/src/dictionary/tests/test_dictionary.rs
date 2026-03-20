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
use crate::Serializable;

#[test]
fn test_hmlabel() {
    fn check_label(key: SliceData, max: usize, value: usize) {
        let label = hm_label(&key, max).unwrap();
        println!("key: {}, max: {}, hm_label: {} value: {:b}", key, max, label, value);
        let len = label.length_in_bits();
        let x: usize = SliceData::load_builder(label).unwrap().get_next_int(len).unwrap() as usize;
        assert_eq!(x, value);
    }
    // check same
    check_label(SliceData::from_raw(vec![0], 8), 16, 0b11001000);
    check_label(SliceData::from_raw(vec![0b11111000], 5), 8, 0b1110101);

    //check
    check_label(SliceData::from_raw(vec![0], 1), 2, 0b0100);

    //additional special tests here:
}

#[test]
fn test_long_keys() {
    hm_label(&SliceData::from_raw(vec![0x77; 64], 512), 512).expect("must be constructed");
    hm_label(&SliceData::from_raw(vec![0x77; 96], 768), 768).expect("must be constructed");
    hm_label(&SliceData::from_raw(vec![0x77; 128], 1011), 1011).expect("must be constructed");
    hm_label(&SliceData::from_raw(vec![0x77; 128], 1012), 1012)
        .expect_err("must not be constructed");
}

#[test]
fn test_merge_complex() -> Result<()> {
    fn init(keys: &[u8], out_keys: &mut Vec<SliceData>) -> Result<HashmapE> {
        let mut tree = HashmapE::with_bit_len(8);
        for key in keys {
            let key = SliceData::new(vec![*key, 0x80]);
            tree.set(key.clone(), &key)?;
            out_keys.push(key);
        }
        Ok(tree)
    }
    fn check(keys1: &[u8], keys2: &[u8]) -> Result<()> {
        let keys = &mut vec![];

        let mut tree1 = init(keys1, keys)?;
        let tree2 = init(keys2, keys)?;

        tree1.merge(&tree2, &SliceData::default())?;

        assert_eq!(tree1.len()?, keys.len());
        for key in keys {
            let value = tree1.get(key.clone())?.expect("must present");
            assert_eq!(key, &value)
        }
        Ok(())
    }

    fn bad_check(keys1: &[u8], keys2: &[u8]) -> Result<()> {
        let keys = &mut vec![];

        let mut tree1 = init(keys1, keys)?;
        let tree2 = init(keys2, keys)?;

        tree1
            .merge(&tree2, &SliceData::default())
            .expect_err("hashmap should not merge same leafs");
        Ok(())
    }

    let keys1 = [0b0000_0000, 0b0011_0000];
    let keys2 = [0b0000_0000, 0b0011_1111];
    bad_check(&keys1, &keys2)?;

    let keys1 = [0b0000_0000, 0b0100_0000, 0b0000_1000];
    let keys2 = [0b0000_0001, 0b0011_1111, 0b0001_1111, 0b0011_0000];
    check(&keys1, &keys2)?;

    let keys1 = [0b1111_1111, 0b1011_1111, 0b1111_0111];
    let keys2 = [0b1111_1110, 0b1100_0000, 0b1110_0000];
    check(&keys1, &keys2)?;

    let keys1 = [0b0000_0000, 0b0010_0000];
    let keys2 = [0b0001_1111, 0b0011_1111];
    check(&keys1, &keys2)?;

    let keys1 = [0b0001_0000, 0b0011_0000];
    let keys2 = [0b0001_1111, 0b0011_1111];
    check(&keys1, &keys2)?;

    Ok(())
}

#[test]
fn test_multiset_random() {
    let mut hashmap = HashmapE::with_bit_len(256);
    let mut keys = Vec::new();
    for _ in 0..10 {
        let key = rand::random::<[u8; 32]>().write_to_bitstring().unwrap();
        let value = rand::random::<u8>().write_to_bitstring().unwrap();
        println!("(\"{key:x}\", Some(0x{value:x}u8)),");
        keys.push(key.clone());
        hashmap.set(key, &value).unwrap();
    }
    println!();
    let mut tr = HashmapTraverser::new(&hashmap);
    for _ in 0..10 {
        if rand::random::<bool>() {
            let key = keys.swap_remove(rand::random::<usize>() % keys.len());
            println!("(\"{key:x}\", None,");
            hashmap.remove(key.clone()).unwrap();
            tr.insert(key, None).unwrap();
        } else {
            let key = rand::random::<[u8; 32]>().write_to_bitstring().unwrap();
            let value = rand::random::<u8>().write_to_bitstring().unwrap();
            println!("(\"{key:x}\", Some(0x{value:x}u8)),");
            hashmap.set(key.clone(), &value).unwrap();
            tr.insert(key, Some(value)).unwrap();
        }
    }
    let new_root = tr.traverse().unwrap().unwrap();
    let new_cell = format!("{new_root:#.10}");
    let cell = format!("{:#.10}", hashmap.data().unwrap());
    pretty_assertions::assert_eq!(new_cell, cell);
    assert_eq!(&new_root, hashmap.data().unwrap());
}

#[test]
fn test_multiset_hashmap() {
    let data = [
        ([1u8; 32], Some(0x11u8)),
        ([2u8; 32], Some(0x22u8)),
        ([8u8; 32], Some(0x33u8)),
        ([17u8; 32], Some(0x44u8)),
        ([128; 32], None),
    ];
    let mut hashmap = HashmapE::with_bit_len(256);
    hashmap
        .set([1u8; 32].write_to_bitstring().unwrap(), &0x77u8.write_to_bitstring().unwrap())
        .unwrap();
    hashmap
        .set([128; 32].write_to_bitstring().unwrap(), &0x55u8.write_to_bitstring().unwrap())
        .unwrap();

    let mut tr = HashmapTraverser::new(&hashmap);
    for (key, value) in data.into_iter() {
        let key: SliceData = key.write_to_bitstring().unwrap();
        if let Some(value) = value {
            let value = value.write_to_bitstring().unwrap();
            hashmap.set(key.clone(), &value).unwrap();
            tr.insert(key, Some(value)).unwrap();
        } else {
            hashmap.remove(key.clone()).unwrap();
            tr.insert(key, None).unwrap();
        }
    }

    println!("{:?}", tr.root);

    let new_root = tr.traverse().unwrap().unwrap();
    let new_cell = format!("{new_root:#.10}");
    let cell = format!("{:#.10}", hashmap.data().unwrap());
    pretty_assertions::assert_eq!(new_cell, cell);
    assert_eq!(&new_root, hashmap.data().unwrap());
}
