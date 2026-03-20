/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{
    block_handle_db::BlockHandleStorage,
    db::rocksdb::{destroy_rocks_db, AccessType},
    tests::utils::{create_block_handle_storage, init_test_log},
    types::BlockMeta,
};
use std::collections::HashMap;
use ton_block::{BlockIdExt, UInt256};

#[test]
fn test_blocks_index_key() {
    use super::*;

    let shard1 = ShardIdent::with_tagged_prefix(0, 0x8000000000000000).unwrap();
    let shard2 = ShardIdent::with_tagged_prefix(1, 0x4000000000000000).unwrap();
    let shard3 = ShardIdent::with_tagged_prefix(2, 0x2000000000000000).unwrap();

    let key1 = BlocksIndexKey::Seqno { shard: shard1.clone(), seqno: 123 };
    let key2 = BlocksIndexKey::Lt { shard: shard1.clone(), lt: 9999999999000456 };
    let key3 = BlocksIndexKey::Utime { shard: shard1.clone(), utime: 789, seqno: 100 };

    let key4 = BlocksIndexKey::Seqno { shard: shard2.clone(), seqno: 321 };
    let key5 = BlocksIndexKey::Lt { shard: shard2.clone(), lt: 100000654 };
    let key6 = BlocksIndexKey::Utime { shard: shard2.clone(), utime: 987, seqno: 200 };

    let key7 = BlocksIndexKey::Seqno { shard: shard3.clone(), seqno: 111 };
    let key8 = BlocksIndexKey::Lt { shard: shard3.clone(), lt: 999000000222 };
    let key9 = BlocksIndexKey::Utime { shard: shard3.clone(), utime: 1999999999, seqno: 300 };

    let s1 = key1.to_string();
    let s2 = key2.to_string();
    let s3 = key3.to_string();
    let s4 = key4.to_string();
    let s5 = key5.to_string();
    let s6 = key6.to_string();
    let s7 = key7.to_string();
    let s8 = key8.to_string();
    let s9 = key9.to_string();

    assert_eq!(s1, "sn:0:0000000123:8000000000000000");
    assert_eq!(s2, "lt:0:9999999999000456:8000000000000000");
    assert_eq!(s3, "ut:0:0000000789:8000000000000000:0000000100");
    assert_eq!(s4, "sn:1:0000000321:4000000000000000");
    assert_eq!(s5, "lt:1:0000000100000654:4000000000000000");
    assert_eq!(s6, "ut:1:0000000987:4000000000000000:0000000200");
    assert_eq!(s7, "sn:2:0000000111:2000000000000000");
    assert_eq!(s8, "lt:2:0000999000000222:2000000000000000");
    assert_eq!(s9, "ut:2:1999999999:2000000000000000:0000000300");

    assert_eq!(BlocksIndexKey::prefix_with_utime(2, 1999999999), "ut:2:1999999999:");
    assert_eq!(BlocksIndexKey::prefix_with_lt(1, 100000654), "lt:1:0000000100000654:");

    assert_eq!(BlocksIndexKey::parse(&s1).unwrap(), key1);
    assert_eq!(BlocksIndexKey::parse(&s2).unwrap(), key2);
    assert_eq!(BlocksIndexKey::parse(&s3).unwrap(), key3);
    assert_eq!(BlocksIndexKey::parse(&s4).unwrap(), key4);
    assert_eq!(BlocksIndexKey::parse(&s5).unwrap(), key5);
    assert_eq!(BlocksIndexKey::parse(&s6).unwrap(), key6);
    assert_eq!(BlocksIndexKey::parse(&s7).unwrap(), key7);
    assert_eq!(BlocksIndexKey::parse(&s8).unwrap(), key8);
    assert_eq!(BlocksIndexKey::parse(&s9).unwrap(), key9);
}

const DB_PATH: &str = "../../target/test";

#[tokio::test(flavor = "multi_thread")]
async fn test_blocks_index_db() -> Result<()> {
    fn create_handle(
        storage: &BlockHandleStorage,
        shard: u64,
        lt: u64,
        seqno: u32,
        utime: u32,
        mc_seqno: u32,
    ) -> Result<Arc<BlockHandle>> {
        let meta = BlockMeta::with_data(0, utime, lt, mc_seqno, 0);
        let shard = ShardIdent::with_tagged_prefix(0, shard)?;
        let id = BlockIdExt {
            shard_id: shard,
            seq_no: seqno,
            root_hash: UInt256::rand(),
            ..Default::default()
        };
        let h = storage.create_handle(id, meta, None)?.unwrap();
        h.set_block_applied();
        Ok(h)
    }

    const DB_NAME: &str = "test_blocks_index_db";

    init_test_log();
    // std::env::set_var("RUST_BACKTRACE", "full");

    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();

    let db = RocksDb::new(DB_PATH, DB_NAME, None, AccessType::ReadWrite)?;
    let block_index_db = BlockIndexDb::with_db(db.clone(), "block_index".to_string(), true)?;
    let block_handles = create_block_handle_storage(db.clone()).0;
    let mut lt_ut = HashMap::new();

    let put = |shard,
               lt,
               seqno,
               utime,
               mc_seqno,
               offset,
               lt_ut: &mut HashMap<u32, (u64, u32)>|
     -> Result<()> {
        let handle = create_handle(&block_handles, shard, lt, seqno, utime, mc_seqno)?;
        block_index_db.put(handle.as_ref(), offset)?;
        lt_ut.insert(offset, (lt, utime));
        Ok(())
    };

    let get_lt = |prefix, lt, expected_mc, expected_offset, lt_ut: &HashMap<u32, (u64, u32)>| {
        let result = block_index_db
            .lookup_by_lt(&AccountIdPrefixFull { workchain_id: 0, prefix }, lt)
            .unwrap();
        let result = result.unwrap();
        let (orig_lt, _) = lt_ut.get(&result.offset).cloned().unwrap();
        assert!(orig_lt >= lt);
        assert_eq!(result.mc_ref, expected_mc);
        assert_eq!(result.offset, expected_offset);
    };
    let get_lt_nonexist = |prefix, lt| {
        assert!(block_index_db
            .lookup_by_lt(&AccountIdPrefixFull { workchain_id: 0, prefix }, lt)
            .unwrap()
            .is_none());
    };
    let get_utime = |prefix,
                     utime,
                     expected_mc,
                     expected_count,
                     lt_ut: &HashMap<u32, (u64, u32)>| {
        let mut result = vec![];
        block_index_db
            .lookup_by_utime(&AccountIdPrefixFull { workchain_id: 0, prefix }, utime, &mut |r| {
                result.push(r);
                Ok(true)
            })
            .unwrap();
        let (_, orig_utime) = lt_ut.get(&result[0].offset).cloned().unwrap();
        assert!(orig_utime >= utime);
        assert_eq!(result.len() as u32, expected_count);
        for result in result {
            assert_eq!(result.mc_ref, expected_mc);
        }
    };
    let get_utime_nonexist = |prefix, utime| {
        let mut found = false;
        block_index_db
            .lookup_by_utime(&AccountIdPrefixFull { workchain_id: 0, prefix }, utime, &mut |_| {
                found = true;
                Ok(true)
            })
            .unwrap();
        assert!(!found);
    };

    let get_seqno = |prefix, seqno, expected: Option<(u32, u32)>| {
        let result = block_index_db
            .lookup_by_seqno(&AccountIdPrefixFull { workchain_id: 0, prefix }, seqno)
            .unwrap();
        assert!(result.is_some() == expected.is_some());
        if let Some((expected_mc, expected_lt)) = expected {
            let result = result.unwrap();
            assert_eq!(result.mc_ref, expected_mc);
            assert_eq!(result.offset, expected_lt);
        }
    };

    for i in 0..10 {
        put(
            0xc000_0000_0000_0000,
            1_000_000 * i as u64,
            1023,
            1760361100 + i,
            590 + i,
            i + 1,
            &mut lt_ut,
        )?;
    }

    put(0x4000_0000_0000_0000, 10_000_000, 1020, 1760361140, 600, 123, &mut lt_ut)?;
    put(0x2000_0000_0000_0000, 11_000_000, 1021, 1760361145, 601, 124, &mut lt_ut)?;
    put(0x2000_0000_0000_0000, 14_000_000, 1022, 1760361147, 601, 125, &mut lt_ut)?;
    put(0xc000_0000_0000_0000, 13_000_000, 1022, 1760361147, 601, 1250, &mut lt_ut)?;
    put(0x2000_0000_0000_0000, 15_000_000, 1023, 1760361147, 601, 126, &mut lt_ut)?;
    put(0x2000_0000_0000_0000, 15_000_000, 1024, 1760361147, 601, 126, &mut lt_ut)?;
    put(0x2000_0000_0000_0000, 15_000_000, 1025, 1760361147, 601, 126, &mut lt_ut)?;

    for i in 17..200 {
        put(
            0xc000_0000_0000_0000,
            1_000_000 * i as u64,
            1023 + i,
            1760361200 + i,
            600 + i,
            1200 + i * 10,
            &mut lt_ut,
        )?;
        put(
            0x2000_0000_0000_0000,
            1_000_000 * i as u64,
            1023 + i,
            1760361200 + i,
            600 + i,
            1200 + i * 10,
            &mut lt_ut,
        )?;
    }

    //
    // by original cpp impl:
    // End LT and UTIME of the found block must always be >= the requested value
    //

    get_lt(0x2200_0000_0000_0000, 12_100_000, 601, 125, &lt_ut);
    get_lt(0x8400_0000_0000_0000, 12_999_000, 601, 1250, &lt_ut);
    get_lt(0x8456_0000_0000_0000, 0, 590, 1, &lt_ut);
    get_lt_nonexist(0x8456_0000_0000_0000, 1_000_000_000);

    get_utime(0x2200_0000_0000_0000, 1760361144, 601, 1, &lt_ut);
    get_utime_nonexist(0x8400_0000_0000_0000, 2760361148);
    get_utime(0x8456_0000_0000_0000, 0, 590, 1, &lt_ut);
    get_utime(0x2200_0000_0000_0000, 1760361147, 601, 4, &lt_ut);
    get_utime(0x8800_0000_0000_0000, 1760361147, 601, 1, &lt_ut);

    get_seqno(0x8200_0000_0000_0000, 100, None);
    get_seqno(0x2200_0000_0000_0000, 1023 + 200, None);
    get_seqno(0x8200_0000_0000_0000, 1022, Some((601, 1250)));

    let _ = (block_handles, block_index_db, db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}
