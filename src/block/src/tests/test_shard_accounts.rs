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
use crate::{generate_test_account, write_read_and_assert, AccountTestOptions};

#[test]
fn test_serialization_shard_account() {
    let mut shard_acc = ShardAccounts::default();
    for n in 5..6 {
        let acc = generate_test_account(true, AccountTestOptions::with_default_setup(true));
        shard_acc.insert(n, &acc, UInt256::default(), 0).unwrap();
    }
    write_read_and_assert(shard_acc);
}
