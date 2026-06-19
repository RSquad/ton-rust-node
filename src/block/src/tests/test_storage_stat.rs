/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    Account, AccountStorageStat, BuilderData, Cell, Deserializable, IBitstring, DICT_HASH_MIN_CELLS,
};

#[test]
fn test_storage_stat() {
    let acc_state1 = include_bytes!("data/storage_stat/acc.boc");
    let mut acc1 = Account::construct_from_bytes(acc_state1).unwrap();
    let storage_info1 = acc1.storage_info().unwrap().clone();

    // calc dictionary and check its hash
    let dict_root1 = acc1.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root1.repr_hash(), storage_info1.dict_hash().unwrap());
    assert_eq!(storage_info1.used(), acc1.storage_info().unwrap().used());

    // import storage stat dict and check again
    let mut acc1 = Account::construct_from_bytes(acc_state1).unwrap();
    acc1.import_storage_stat_dict(dict_root1.clone()).unwrap();

    let dict_root1 = acc1.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root1.repr_hash(), storage_info1.dict_hash().unwrap());
    assert_eq!(storage_info1.used(), acc1.storage_info().unwrap().used());

    // change account state and update dictionary
    let acc_state2 = include_bytes!("data/storage_stat/acc1.boc");
    let mut acc2 = Account::construct_from_bytes(acc_state2).unwrap();
    assert!(acc2.import_storage_stat_dict(dict_root1).is_err());
    let storage_info2 = acc2.storage_info().unwrap();

    *acc1.state_init_mut().unwrap() = acc2.state_init().unwrap().clone();

    let dict_root2 = acc1.calc_storage_stat_dict(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root2.repr_hash(), storage_info2.dict_hash().unwrap());
    assert_eq!(storage_info2.used(), acc1.storage_info().unwrap().used());
}

// Full-removal edge case for `replace_roots`: removing all roots empties the dict but leaves
// zero-refcount entries in the cache. The next root change then hits "empty dict + non-empty
// cache" and must NOT clear (the cache is still complete). Verifies the full→empty→full cycle
// gives exactly the same totals and dict as a fresh build over the final roots.
#[test]
fn test_storage_stat_full_removal_then_readd() {
    fn leaf(tag: u32) -> Cell {
        let mut b = BuilderData::new();
        b.append_u32(tag).unwrap();
        b.into_cell().unwrap()
    }
    fn node(tag: u32, children: &[Cell]) -> Cell {
        let mut b = BuilderData::new();
        b.append_u32(tag).unwrap();
        for c in children {
            b.checked_append_reference(c.clone()).unwrap();
        }
        b.into_cell().unwrap()
    }

    let shared = leaf(0x5EED);
    let root_a = node(0x0A, &[shared.clone(), leaf(0xAA)]);
    let root_b = node(0x0B, &[shared.clone(), leaf(0xBB)]); // shares `shared` with root_a

    // Reference: a fresh stat built directly over the final roots.
    let mut reference = AccountStorageStat::default();
    reference.replace_roots([root_b.clone()].as_slice().into()).unwrap();
    let ref_dict = reference.calc_dict().unwrap().map(|c| c.repr_hash().clone());
    let (ref_cells, ref_bits) = (reference.total_cells(), reference.total_bits());

    // Edge path: build over root_a, remove everything, then re-add root_b.
    let mut stat = AccountStorageStat::default();
    stat.replace_roots([root_a].as_slice().into()).unwrap();
    assert!(stat.calc_dict().unwrap().is_some(), "dict built for root_a");

    stat.replace_roots([].as_slice().into()).unwrap();
    assert!(stat.calc_dict().unwrap().is_none(), "dict emptied after full removal");
    assert_eq!(stat.total_cells(), 0, "totals zeroed after full removal");
    assert!(!stat.cache.is_empty(), "cache keeps zero-refcount entries — the edge state");

    stat.replace_roots([root_b].as_slice().into()).unwrap();
    let edge_dict = stat.calc_dict().unwrap().map(|c| c.repr_hash().clone());

    assert_eq!(stat.total_cells(), ref_cells, "cells match fresh build");
    assert_eq!(stat.total_bits(), ref_bits, "bits match fresh build");
    assert_eq!(edge_dict, ref_dict, "dict matches fresh build after full→empty→full");
}
