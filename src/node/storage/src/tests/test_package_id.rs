/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::archives::package_id::{PackageId, PackageType};
//use ton_block::UnixTime32;
use ton_block::ShardIdent;
//use std::path::Path;

#[test]
fn test_construction() {
    assert_eq!(
        format!("{:?}", PackageId::for_block(0xFFFF_FFF0)),
        format!("{:?}", PackageId::with_values(0xFFFF_FFF0, PackageType::Blocks))
    );
    assert_eq!(
        format!("{:?}", PackageId::for_key_block(0xFFFF_FFF0)),
        format!("{:?}", PackageId::with_values(0xFFFF_FFF0, PackageType::KeyBlocks))
    );
}

#[test]
fn test_paths() {
    let mc_shard = ShardIdent::masterchain();
    let wc_shard = ShardIdent::with_tagged_prefix(0, 0xa000000000000000).unwrap();

    let id = PackageId::for_block(123);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0000/archive.00123.pack"
    );
    assert_eq!(
        format!("{}", id.full_path("/", &wc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0000/archive.00123.0:a000000000000000.pack"
    );

    let id = PackageId::for_block(123_456);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0001/archive.123456.pack"
    );
    assert_eq!(
        format!("{}", id.full_path("/", &wc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0001/archive.123456.0:a000000000000000.pack"
    );

    let id = PackageId::for_block(6_123_456);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0061/archive.6123456.pack"
    );
    assert_eq!(
        format!("{}", id.full_path("/", &wc_shard).unwrap().display()).as_str(),
        "/archive/packages/arch0061/archive.6123456.0:a000000000000000.pack"
    );

    let id = PackageId::for_key_block(123);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/key000/key.archive.000123.pack"
    );

    let id = PackageId::for_key_block(223_456);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/key000/key.archive.223456.pack"
    );

    let id = PackageId::for_key_block(12_223_456);
    assert_eq!(
        format!("{}", id.full_path("/", &mc_shard).unwrap().display()).as_str(),
        "/archive/packages/key001/key.archive.12223456.pack"
    );

    //let temp = PackageId::for_temp(&UnixTime32::new(20_000));
    //assert_eq!(temp.name().as_str(), "temp.archive.18000.pack");
    //assert_eq!(temp.path().as_str(), "files/packages/");
    //assert_eq!(
    //    temp.full_path(Path::new("/tmp/")),
    //    Path::new("/tmp/files/packages/temp.archive.18000.pack")
    //);
}
