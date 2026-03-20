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
    tests::utils::{get_test_block_id, get_test_shard_ident, FILE_HASH, ROOT_HASH},
    traits::Serializable,
    types::BlockMeta,
};
use std::sync::atomic::Ordering;
use ton_block::{BlockIdExt, Result, ShardIdent};

static SHARD_IDENT_SERIALIZED: [u8; 12] = [
    // workchain_id = -1 (4 bytes)
    0xFF, 0xFF, 0xFF, 0xFF, // prefix = 0x8000_0000_0000_0000 (8 bytes)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
];

static SEQ_NO_SERIALIZED: [u8; 4] = [
    // seq_no = 1830539 (4 bytes)
    0x8B, 0xEE, 0x1B, 0x00,
];

#[test]
fn test_shard_ident_serialization() -> Result<()> {
    let shard_ident = get_test_shard_ident();
    let data = shard_ident.serialize();
    assert_eq!(data, SHARD_IDENT_SERIALIZED);
    let new_shard_ident = ShardIdent::deserialize(data.as_slice())?;
    assert_eq!(new_shard_ident, shard_ident);
    Ok(())
}

#[test]
fn test_block_id_ext_serialization() -> Result<()> {
    let block_id_ext = get_test_block_id();
    let data = block_id_ext.serialize();
    assert_eq!(
        data,
        [&SHARD_IDENT_SERIALIZED[..], &SEQ_NO_SERIALIZED[..], &ROOT_HASH[..], &FILE_HASH[..]]
            .concat()
            .as_slice()
    );
    let new_block_id_ext = BlockIdExt::deserialize(data.as_slice())?;
    assert_eq!(new_block_id_ext, block_id_ext);
    Ok(())
}

#[test]
fn test_block_meta_serialization() -> Result<()> {
    let meta = BlockMeta::with_data(
        0x00_00_56_78,
        0x90_AB_CD_EF,
        0x11_22_33_44_55_66_77_88,
        0x01_02_03_04,
        0x01_01_00_00,
    );
    meta.test_counter.store(0x10203040, Ordering::Relaxed);

    let expected: Vec<u8> = vec![
        0x04, 0x03, 0x02, 0x01, 0x78, 0x56, 0x00, 0x00, 0xEF, 0xCD, 0xAB, 0x90, 0x88, 0x77, 0x66,
        0x55, 0x44, 0x33, 0x22, 0x11, 0x40, 0x30, 0x20, 0x10,
    ];

    let data = meta.serialize();
    assert_eq!(&data[..], &expected[..]);
    let new_meta = BlockMeta::deserialize(data.as_slice())?;
    assert_eq!(new_meta.flags(), meta.flags());
    assert_eq!(new_meta.gen_utime, meta.gen_utime);
    assert_eq!(new_meta.end_lt, meta.end_lt);
    assert_eq!(new_meta.masterchain_ref_seq_no(), meta.masterchain_ref_seq_no());

    assert_eq!(
        new_meta.test_counter.load(Ordering::SeqCst),
        meta.test_counter.load(Ordering::SeqCst)
    );
    Ok(())
}
