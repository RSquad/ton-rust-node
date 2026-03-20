/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::archives::package_entry_id::{GetFileName, PackageEntryId};
use ton_block::{BlockIdExt, ShardIdent, UInt256};

#[test]
fn test_filenames() {
    let id = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0x0d80000000000000).unwrap(),
        135,
        UInt256::default(),
        UInt256::default(),
    );
    let id_str = id.filename();
    assert_eq!(
        id_str,
        concat!(
            "(0,0d80000000000000,135)",
            ":0000000000000000000000000000000000000000000000000000000000000000",
            ":0000000000000000000000000000000000000000000000000000000000000000"
        )
    );

    let id_master = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        UInt256::default(),
        UInt256::default(),
    );
    let id_master_str = id_master.filename();
    assert_eq!(
        id_master_str,
        concat!(
            "(-1,8000000000000000,0)",
            ":0000000000000000000000000000000000000000000000000000000000000000",
            ":0000000000000000000000000000000000000000000000000000000000000000"
        )
    );

    test_entry(PackageEntryId::Empty, "empty".to_string());
    test_entry(PackageEntryId::Block(id.clone()), format!("block_{}", id_str));
    test_entry(PackageEntryId::ZeroState(id.clone()), format!("zerostate_{}", id_str));
    test_entry(PackageEntryId::Proof(id.clone()), format!("proof_{}", id_str));
    test_entry(PackageEntryId::ProofLink(id.clone()), format!("prooflink_{}", id_str));
    test_entry(PackageEntryId::Signatures(id.clone()), format!("signatures_{}", id_str));
    test_entry(PackageEntryId::BlockInfo(id.clone()), format!("info_{}", id_str));
    test_entry(
        PackageEntryId::PersistentState((id_master, id.clone())),
        format!("state_{}_{}", id_master_str, id_str),
    );
}

fn test_entry(entry_id: PackageEntryId<BlockIdExt>, expected: String) {
    let filename = entry_id.filename();
    assert_eq!(filename, expected);
    let parsed = PackageEntryId::from_filename(&filename).unwrap();
    assert_eq!(parsed, entry_id)
}

#[test]
fn test_package_with_parse() {
    let shard_id = ShardIdent::with_tagged_prefix(555, 0xF8000000_00000000).unwrap();
    let id = BlockIdExt::with_params(shard_id.clone(), 100, UInt256::rand(), UInt256::rand());
    let filename = PackageEntryId::Block(id.clone()).filename_short();
    let (workchain_id, shard_ident, seq_no) = parse_short_filename(&filename).unwrap();
    assert_eq!(workchain_id, 555);
    assert_eq!(shard_ident, 0xF8000000_00000000);
    assert_eq!(seq_no, 100);
}
