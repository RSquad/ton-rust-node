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
use super::*;

mod test_bintree {
    use super::*;
    use crate::CurrencyCollection;

    fn prepare_key(key: usize, bits: usize) -> SliceData {
        let mut builder = BuilderData::new();
        builder.append_bits(key, bits).unwrap();
        SliceData::load_bitstring(builder).unwrap()
    }

    #[test]
    fn test_bintree_new_simple() {
        let mut tree = BinTree::with_item(&0u8).unwrap();
        assert_eq!(tree.get(SliceData::default()).unwrap(), Some(0));
        assert!(tree.update(SliceData::default(), |v| Ok(v + 11)).unwrap());
        assert_eq!(tree.get(SliceData::default()).unwrap(), Some(11));

        assert!(tree.split(SliceData::default(), |left| Ok((left, 22))).unwrap());
        assert_eq!(tree.get(prepare_key(0, 1)).unwrap(), Some(11));
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(22));
        assert!(tree.update(prepare_key(1, 1), |v| Ok(2 + v)).unwrap());
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(24));

        let tree2 = tree.get_data();
        println!("{:.3}", tree2.clone().into_cell().unwrap());

        assert!(tree.split(prepare_key(0, 1), |left| Ok((left, 33))).unwrap());
        let tree3 = tree.get_data();
        println!("{:.3}", tree3.clone().into_cell().unwrap());
        assert_eq!(tree.get(prepare_key(0, 1)).unwrap(), None);
        assert_eq!(tree.get(prepare_key(0, 2)).unwrap(), Some(11));
        assert_eq!(tree.get(prepare_key(1, 2)).unwrap(), Some(33));
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(24));

        assert!(!tree.split(prepare_key(0, 1), |left| Ok((left, 34))).unwrap());
        assert!(!tree.update(prepare_key(0, 1), |_| Ok(1)).unwrap());
        assert_eq!(tree3, tree.get_data());
        assert!(!tree.merge(prepare_key(1, 1), |left, _right| Ok(left)).unwrap());
        assert_eq!(tree3, tree.get_data());
        assert!(!tree.merge(prepare_key(0, 2), |left, _right| Ok(left)).unwrap());
        assert_eq!(tree3, tree.get_data());
        assert!(!tree.merge(prepare_key(1, 2), |left, _right| Ok(left)).unwrap());
        assert_eq!(tree3, tree.get_data());

        assert!(tree.merge(prepare_key(0, 1), |left, _right| Ok(left)).unwrap());
        assert_eq!(tree.get(prepare_key(0, 1)).unwrap(), Some(11));
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(24));
        assert_eq!(tree2, tree.get_data());
    }

    #[test]
    fn test_bintreeaug_new_simple() {
        let mut tree = BinTreeAug::with_item(&11u8, &CurrencyCollection::with_coins(1)).unwrap();
        assert_eq!(tree.get(SliceData::default()).unwrap(), Some(11));
        assert_eq!(tree.root_extra(), &CurrencyCollection::with_coins(1));

        assert!(tree.split(SliceData::default(), &22, &CurrencyCollection::with_coins(2)).unwrap());
        let tree2 = tree.get_data();
        println!("{}", tree2);
        assert_eq!(tree.get(prepare_key(0, 1)).unwrap(), Some(11));
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(22));
        assert_eq!(tree.root_extra(), &CurrencyCollection::with_coins(3));

        assert!(tree.split(prepare_key(0, 1), &33, &CurrencyCollection::with_coins(4)).unwrap());
        let tree3 = tree.get_data();
        println!("{}", tree3);
        assert_eq!(tree.get(prepare_key(0, 1)).unwrap(), None);
        assert_eq!(tree.get(prepare_key(0, 2)).unwrap(), Some(11));
        assert_eq!(tree.get(prepare_key(1, 2)).unwrap(), Some(33));
        assert_eq!(tree.get(prepare_key(1, 1)).unwrap(), Some(22));
        assert_eq!(tree.root_extra(), &CurrencyCollection::with_coins(7));

        assert!(!tree.split(prepare_key(0, 1), &34, &CurrencyCollection::with_coins(5)).unwrap());
        assert_eq!(tree3, tree.get_data());
        assert_eq!(tree.root_extra(), &CurrencyCollection::with_coins(7));
    }

    #[test]
    fn test_bintree_find() {
        let mut tree = BinTree::with_item(&1u8).unwrap();
        println!("{:#.3}", tree.data.clone().into_cell().unwrap());
        // empty root
        assert_eq!(tree.get(SliceData::default()).unwrap(), Some(1));
        assert_eq!(tree.find(SliceData::default()).unwrap(), Some((SliceData::default(), 1)));
        // 0 and 1
        assert!(tree.split(SliceData::default(), |left| Ok((left, 2))).unwrap());
        // 00, 01, 1
        assert!(tree.split(prepare_key(0, 1), |left| Ok((left, 3))).unwrap());
        // 00, 01, 10, 11
        assert!(tree.split(prepare_key(1, 1), |left| Ok((left, 4))).unwrap());
        // 000, 001, 01, 10, 11
        assert!(tree.split(prepare_key(0, 2), |left| Ok((left, 5))).unwrap());
        assert_eq!(tree.get(prepare_key(1, 3)).unwrap(), Some(5));

        // 000(1), 001(5), 01(3), 10(2), 11(4)
        assert_eq!(tree.find(prepare_key(0b0011, 4)).unwrap(), Some((prepare_key(1, 3), 5)));
        assert_eq!(tree.find(prepare_key(0b001111, 6)).unwrap(), Some((prepare_key(1, 3), 5)));
        assert_eq!(tree.find(prepare_key(0b000111, 6)).unwrap(), Some((prepare_key(0, 3), 1)));
    }
}
