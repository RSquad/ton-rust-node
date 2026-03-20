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
    db::DbKey,
    tests::utils::{FILE_HASH, ROOT_HASH},
};
use ton_block::{BlockIdExt, ShardIdent, UInt256};

fn create_block_id() -> BlockIdExt {
    BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(-1, 0x8000_0000_0000_0000).unwrap(),
        1,
        UInt256::from(ROOT_HASH),
        UInt256::from(FILE_HASH),
    )
}

#[test]
fn test_block_id_formatting() {
    let block_id = create_block_id();
    assert_eq!(
        block_id.to_string().to_lowercase(),
        format!(
            "(-1:8000000000000000, 1, rh {}, fh {})",
            hex::encode(ROOT_HASH),
            hex::encode(FILE_HASH)
        )
        .to_lowercase()
    );
}

#[test]
fn test_block_key_formatting() {
    let block_id = create_block_id();
    assert_eq!(block_id.key(), ROOT_HASH);
}
