/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{Account, Deserializable, DICT_HASH_MIN_CELLS};

#[test]
fn test_storage_stat() {
    let acc_state1 = include_bytes!("data/storage_stat/acc.boc");
    let mut acc1 = Account::construct_from_bytes(acc_state1).unwrap();
    let storage_info1 = acc1.storage_info().unwrap().clone();

    // calc dictionary and check its hash
    let dict_root1 = acc1.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root1.repr_hash(), storage_info1.dict_hash().unwrap());
    assert_eq!(storage_info1.used(), acc1.storage_info().unwrap().used());

    // import storage stat dict and check again
    let mut acc1 = Account::construct_from_bytes(acc_state1).unwrap();
    acc1.import_storage_stat_dict(dict_root1.clone()).unwrap();

    let dict_root1 = acc1.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root1.repr_hash(), storage_info1.dict_hash().unwrap());
    assert_eq!(storage_info1.used(), acc1.storage_info().unwrap().used());

    // change account state and update dictionary
    let acc_state2 = include_bytes!("data/storage_stat/acc1.boc");
    let mut acc2 = Account::construct_from_bytes(acc_state2).unwrap();
    assert!(acc2.import_storage_stat_dict(dict_root1).is_err());
    let storage_info2 = acc2.storage_info().unwrap();

    *acc1.state_init_mut().unwrap() = acc2.state_init().unwrap().clone();

    let dict_root2 = acc1.update_storage_stat(DICT_HASH_MIN_CELLS).unwrap().unwrap();
    assert_eq!(dict_root2.repr_hash(), storage_info2.dict_hash().unwrap());
    assert_eq!(storage_info2.used(), acc1.storage_info().unwrap().used());
}
