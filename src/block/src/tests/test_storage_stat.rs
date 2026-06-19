/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    Account, AccountStorageStat, BuilderData, Cell, Deserializable, IBitstring, MerkleProof,
    StorageStatDict, UInt256, UsageTree, DICT_HASH_MIN_CELLS,
};
use std::collections::HashSet;

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

// `add_hint` must drop any previously seeded hint, even on its early-return paths (empty `loaded`
// or empty dict). Otherwise a stale hint from a prior call could be applied to the next diff.
#[test]
fn test_add_hint_clears_stale_hint() {
    let parent = node(0x0A, &[node(0x55, &[leaf(0xCC)])]);

    // A non-empty dict is required, otherwise `add_hint` short-circuits before touching the hint.
    let mut full = AccountStorageStat::default();
    full.replace_roots([parent.clone()].as_slice().into()).unwrap();
    let dict = full.calc_dict().unwrap().cloned().unwrap();

    let mut stat = AccountStorageStat::default();
    stat.dict = StorageStatDict::with_hashmap(Some(dict));
    stat.roots = [parent.clone()].as_slice().into();

    let loaded: HashSet<UInt256> = [parent.repr_hash().clone()].into_iter().collect();
    stat.add_hint(&loaded);
    assert!(!stat.hint.is_empty(), "hint is seeded from loaded cells");

    // Second call with nothing loaded must clear the previously seeded hint, not keep it.
    stat.add_hint(&HashSet::new());
    assert!(stat.hint.is_empty(), "empty `loaded` clears the stale hint");
}

// A subtree that is full in the new state but pruned in the old (collated-data) proof — both in the
// stored state and in the storage-stat dict. Without the hint the validator-style incremental diff
// re-counts the shared subtree as new (overcount); `add_hint` marks it pre-existing so totals stay
// correct. Mirrors the real `check_ethalon_bundle` failure at the unit level.
#[test]
fn test_add_hint_treats_pruned_shared_cell_as_preexisting() {
    let gc = leaf(0xCC);
    let shared = node(0x55, &[gc.clone()]); // shared subtree: `shared` + `gc`
    let old_parent = node(0x0A, &[shared.clone()]);
    let new_parent = node(0x0B, &[shared.clone()]); // different root, same shared subtree
    let shared_hash = shared.repr_hash().clone();

    // Full stat over the old state: counts old_parent + shared + gc.
    let mut full = AccountStorageStat::default();
    full.replace_roots([old_parent.clone()].as_slice().into()).unwrap();
    let full_dict = full.calc_dict().unwrap().cloned().unwrap();
    let full_cells = full.total_cells();
    let full_bits = full.total_bits();

    // Prune the whole `shared` subtree (both `shared` and `gc`) out of the dict: look up only
    // `old_parent` through a usage tree, then build the proof from it. `dict.get(shared)` /
    // `dict.get(gc)` then walk into a pruned branch and return None. (`gc` must be pruned too,
    // otherwise it would resolve as a dict hit and the overcount would cancel — the real bug
    // overcounts the *children* of the shared cell, not the shared cell itself.)
    let usage = UsageTree::with_root(full_dict.clone());
    let tracked = StorageStatDict::with_hashmap(Some(usage.root_cell()));
    tracked.get(old_parent.repr_hash()).unwrap();
    let pruned_dict =
        MerkleProof::create_by_usage_tree(&full_dict, &usage).unwrap().proof.virtualize(1);
    let probe = StorageStatDict::with_hashmap(Some(pruned_dict.clone()));
    assert!(probe.get(old_parent.repr_hash()).unwrap().is_some(), "old root entry survives");
    assert!(probe.get(&shared_hash).unwrap().is_none(), "shared entry is pruned from the dict");
    assert!(probe.get(gc.repr_hash()).unwrap().is_none(), "shared child entry is pruned too");

    // Prune `shared` out of the old stored state too: old_parent now references a pruned branch.
    let old_parent_pruned =
        MerkleProof::create(&old_parent, |h| *h != shared_hash).unwrap().proof.virtualize(1);
    assert_eq!(
        old_parent_pruned.reference(0).unwrap().references_count(),
        0,
        "shared subtree is pruned in the old state"
    );

    let seed = |stat: &mut AccountStorageStat| {
        stat.dict = StorageStatDict::with_hashmap(Some(pruned_dict.clone()));
        stat.roots = [old_parent_pruned.clone()].as_slice().into();
        stat.total_cells = full_cells;
        stat.total_bits = full_bits;
        stat.dict_updated = false;
    };
    let loaded: HashSet<UInt256> = [old_parent_pruned.repr_hash().clone()].into_iter().collect();

    // Without the hint: `shared` is re-counted as a brand-new cell → overcount.
    let mut without_hint = AccountStorageStat::default();
    seed(&mut without_hint);
    without_hint.replace_roots([new_parent.clone()].as_slice().into()).unwrap();

    // With the hint: `shared` is recognised as pre-existing → totals unchanged.
    let mut with_hint = AccountStorageStat::default();
    seed(&mut with_hint);
    with_hint.add_hint(&loaded);
    assert!(with_hint.hint.contains(&shared_hash), "hint marks the shared subtree");
    with_hint.replace_roots([new_parent].as_slice().into()).unwrap();

    assert_eq!(with_hint.total_cells(), full_cells, "with hint: shared subtree not re-counted");
    assert_eq!(
        without_hint.total_cells(),
        full_cells + 1,
        "without hint: shared cell wrongly counted as new"
    );
    assert!(with_hint.total_bits() < without_hint.total_bits(), "hint avoids the extra bits too");
}
