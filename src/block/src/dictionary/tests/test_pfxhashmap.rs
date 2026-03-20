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

fn run_prefix_hashmap(table: &[(u8, usize)]) {
    let mut tree = PfxHashmapE::with_bit_len(7);
    assert!(tree.is_empty());
    let mut history = vec![];

    for (i, (v, len)) in table.iter().enumerate() {
        println!("set {} - value {:8b}", i, v);
        let key = SliceData::from_raw(vec![*v], *len);
        let value = SliceData::new(vec![*v]);
        assert_eq!(tree.get(key.clone()).unwrap(), None);
        assert_eq!(tree.set(key.clone(), &value).unwrap(), None);
        println!("after {}", tree);
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
        let prefix_key = SliceData::from_raw(vec![*v], 8);
        if let (_, _, Some(postfix)) = SliceData::common_prefix(&prefix_key, &key) {
            assert_eq!(
                tree.get_prefix_leaf_with_gas(prefix_key.clone(), &mut 0).unwrap(),
                (prefix_key.clone(), Some(value.clone()), postfix)
            );
        }

        let prefix_key = SliceData::from_raw(vec![*v], 3);
        assert!(tree.is_prefix(prefix_key.clone()).unwrap());
        assert_eq!(tree.get(prefix_key.clone()).unwrap(), None);
        assert_eq!(tree.set(prefix_key.clone(), &value).unwrap(), None);
        assert_eq!(tree.get(prefix_key.clone()).unwrap(), None);

        history.push(tree.data().unwrap().clone());
    }

    let (prefix, value, remainder) =
        tree.get_prefix_leaf_with_gas(SliceData::from_raw(vec![0b11111111], 8), &mut 0).unwrap();
    assert_eq!(value, Some(SliceData::new(vec![0b11111111])));
    assert_eq!(prefix, SliceData::from_raw(vec![0b11111111], 7));
    assert_eq!(remainder, SliceData::from_raw(vec![0b11111111], 1));

    for (i, (v, len)) in table.iter().rev().enumerate() {
        assert_eq!(tree.data(), history.pop().as_ref());
        println!("remove {} - value {:8b}", i, *v);
        let key = SliceData::from_raw(vec![*v], *len);
        let value = SliceData::new(vec![*v]);
        assert_eq!(tree.get(key.clone()).unwrap(), Some(value.clone()));
        assert_eq!(tree.remove(key.clone()).unwrap(), Some(value));
        assert_eq!(tree.get(key.clone()).unwrap(), None);
        println!("after {}", tree);
    }

    assert!(tree.is_empty());
}

#[test]
fn test_prefix_hashmap() {
    run_prefix_hashmap(&[(0b11111111, 7), (0b11110111, 5), (0b11111001, 7), (0b11111011, 7)]);
    run_prefix_hashmap(&[(0b11111111, 7), (0b01110111, 5), (0b11111001, 7), (0b11111011, 7)]);
}
