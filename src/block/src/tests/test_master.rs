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
use crate::{
    read_single_root_boc, transactions::tests::generate_test_shard_account_block,
    write_read_and_assert, Block, BlockExtra, ConfigParam0, ConfigParamEnum, Deserializable,
    ExtBlkRef, HashmapAugType, HashmapE, HashmapType, InMsgFinal, ShardStateUnsplit,
    BASE_WORKCHAIN_ID,
};
use std::collections::HashMap;

#[test]
fn test_libraries() {
    let mut id = [0u8; 32];
    id[0] = 44;
    let acc_id = AccountId::from(id);

    let mut id = [0u8; 32];
    id[0] = 39;
    let my_id = AccountId::from(id);

    let mut id = [0u8; 32];
    id[0] = 157;
    let your_id = AccountId::from(id);

    let lib_code = SliceData::new(vec![0x11, 0x80]).into_cell().unwrap();
    let lib1 = LibDescr::from_lib_data_by_publisher(lib_code, my_id.clone());

    let lib_code = SliceData::new(vec![0x75, 0x80]).into_cell().unwrap();
    let mut lib2 = LibDescr::from_lib_data_by_publisher(lib_code, my_id);
    lib2.publishers_mut().set(&your_id, &EmptyValue).unwrap();

    let mut data = HashmapE::with_bit_len(256);
    let key = SliceData::load_builder(acc_id.write_to_new_cell().unwrap()).unwrap();
    data.set_builder(key.clone(), &lib1.write_to_new_cell().unwrap()).unwrap();
    data.set_builder(key, &lib2.write_to_new_cell().unwrap()).unwrap();

    let cell = data.serialize().unwrap();
    let mut restored_data = HashmapE::with_bit_len(256);
    restored_data.read_from_cell(cell).unwrap();

    assert_eq!(data, restored_data);
}

#[test]
fn test_shard_descr() {
    let descr_none =
        ShardDescr::with_params(42, 17, 25, UInt256::from([70; 32]), FutureSplitMerge::None);
    let descr_split = ShardDescr::with_params(
        42,
        17,
        25,
        UInt256::from([70; 32]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let descr_merge = ShardDescr::with_params(
        42,
        17,
        25,
        UInt256::from([70; 32]),
        FutureSplitMerge::Merge { merge_utime: 0x12345678, interval: 0x87654321 },
    );

    write_read_and_assert(descr_none);
    write_read_and_assert(descr_split);
    write_read_and_assert(descr_merge);
}

#[test]
fn test_mc_state_extra() {
    let mut extra = McStateExtra::default();
    let shard1 =
        ShardDescr::with_params(23, 77, 234, UInt256::from([131; 32]), FutureSplitMerge::None);
    let shard1_1 =
        ShardDescr::with_params(25, 177, 230, UInt256::from([131; 32]), FutureSplitMerge::None);
    let shard2 = ShardDescr::with_params(
        15,
        78,
        235,
        UInt256::from([77; 32]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let shard2_2 = ShardDescr::with_params(
        115,
        8,
        35,
        UInt256::from([77; 32]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let ident = extra.add_workchain(11, &shard1).unwrap();
    extra.shards.split_shard(&ident, |_| Ok((shard1, shard1_1))).unwrap();
    let ident = extra.add_workchain(22, &shard2).unwrap();
    extra.shards.split_shard(&ident, |_| Ok((shard2, shard2_2))).unwrap();

    let key = SliceData::load_builder(123u32.write_to_new_cell().unwrap()).unwrap();
    let value = 0x11u8.write_to_new_cell().unwrap();
    extra.config.config_params.set_builder(key, &value).unwrap();

    extra
        .prev_blocks
        .set(
            &2342,
            &KeyExtBlkRef {
                key: false,
                blk_ref: ExtBlkRef {
                    end_lt: 1,
                    seq_no: 999,
                    root_hash: UInt256::from([10; 32]),
                    file_hash: UInt256::from([10; 32]),
                },
            },
            &KeyMaxLt { key: false, max_end_lt: 1000001 },
        )
        .unwrap();
    extra
        .prev_blocks
        .set(
            &664324,
            &KeyExtBlkRef {
                key: false,
                blk_ref: ExtBlkRef {
                    end_lt: 1000,
                    seq_no: 1999,
                    root_hash: UInt256::from([13; 32]),
                    file_hash: UInt256::from([14; 32]),
                },
            },
            &KeyMaxLt { key: false, max_end_lt: 1000002 },
        )
        .unwrap();

    write_read_and_assert(extra.clone());
}

fn build_mc_block_extra() -> McBlockExtra {
    let mut extra = McBlockExtra::default();
    let shard1 =
        ShardDescr::with_params(23, 77, 234, UInt256::from([131; 32]), FutureSplitMerge::None);
    let shard1_1 =
        ShardDescr::with_params(25, 177, 230, UInt256::from([131; 32]), FutureSplitMerge::None);
    let shard2 = ShardDescr::with_params(
        15,
        78,
        235,
        UInt256::from([77; 32]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let shard2_2 = ShardDescr::with_params(
        115,
        8,
        35,
        UInt256::from([77; 32]),
        FutureSplitMerge::Split { split_utime: 0x12345678, interval: 0x87654321 },
    );
    let ident = ShardIdent::with_workchain_id(11).unwrap();
    extra.shards.add_workchain(11, 134, UInt256::default(), UInt256::default()).unwrap();
    extra
        .fees
        .store_shard_fees(
            &ident,
            CurrencyCollection::with_coins(1),
            CurrencyCollection::with_coins(1),
        )
        .unwrap();
    extra.shards.split_shard(&ident, |_| Ok((shard1, shard1_1))).unwrap();
    let ident = ShardIdent::with_workchain_id(22).unwrap();
    extra.shards.add_workchain(22, 135, UInt256::default(), UInt256::default()).unwrap();
    extra
        .fees
        .store_shard_fees(
            &ident,
            CurrencyCollection::with_coins(1),
            CurrencyCollection::with_coins(1),
        )
        .unwrap();
    extra.shards.split_shard(&ident, |_| Ok((shard2, shard2_2))).unwrap();
    extra.write_recover_create_msg(&InMsg::Final(InMsgFinal::default())).unwrap();
    extra
}

#[test]
fn test_mc_block_extra() {
    let mut extra = build_mc_block_extra();
    extra = write_read_and_assert(extra);

    let mut cp = ConfigParams::default();
    cp.set_config(ConfigParamEnum::ConfigParam0(ConfigParam0 {
        config_addr: AccountId::from([1; 32]),
    }))
    .unwrap();

    extra.set_config(cp);
    let extra = write_read_and_assert(extra);

    let mut block_extra = BlockExtra::default();
    block_extra.write_account_blocks(&generate_test_shard_account_block()).unwrap();
    block_extra.write_custom(&extra).unwrap();

    write_read_and_assert(block_extra);

    // let mut count = 0;
    // restored_extra.shard_hashes.iterate_with_keys(|id: u32, shard_descrs| {
    //     shard_descrs.iterate(|descr| {
    //         count += 1;
    //         println!("{}. {} {}", count, id, descr.0);
    //         Ok(true)
    //     }).unwrap();
    //     Ok(true)
    // }).unwrap();
}

#[test]
fn test_serialization_shard_hashes() {
    let mut shard_hashes = ShardHashes::default();

    for n in 0..12i32 {
        let descr = ShardDescr::with_params(
            42,
            17,
            25,
            UInt256::from([n as u8; 32]),
            FutureSplitMerge::None,
        );
        let shards = BinTree::with_item(&descr).unwrap();
        shard_hashes.set(&n, &InRefValue(shards)).unwrap();
    }

    write_read_and_assert(shard_hashes);
}

#[test]
fn test_real_shard_hashes() {
    let block = Block::construct_from_file("src/tests/data/key_block_not_all_shardes.boc").unwrap();
    let extra = block.read_extra().unwrap().read_custom().unwrap().expect("need key block");
    let shards = extra.shards();
    let mut count = shards.dump("shards");
    println!("total: {}", count);

    let mut result = vec![];
    println!("---- pairs ----");
    shards
        .iterate_shards_with_siblings(|shard, _descr, sibling| {
            let sib = shard.sibling();
            result.iter().for_each(|item| assert_ne!(item, &sib));
            println!(
                "shard: {}:{:064b} sibling: {}",
                shard.workchain_id(),
                shard.shard_prefix_with_tag(),
                sibling.is_some()
            );
            result.push(shard);
            count -= 1;
            count -= sibling.is_some() as usize;
            Ok(true)
        })
        .unwrap();
    println!("total: {}", result.len());
    println!("----  end  ----");
    assert_eq!(count, 0);

    // 0400000000000000

    let shard = ShardIdent::with_tagged_prefix(
        0,
        0b0000010000000000000000000000000000000000000000000000000000000000,
    )
    .unwrap();

    let found_shard = shards.get_shard(&shard).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let shard2 = ShardIdent::with_tagged_prefix(
        0,
        0b0000011000000000000000000000000000000000000000000000000000000000,
    )
    .unwrap();
    let found_shard = shards.get_shard(&shard2).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.find_shard(&shard2).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let left_ancestor_mask = shard.left_ancestor_mask().unwrap();
    let right_ancestor_mask = shard.right_ancestor_mask().unwrap();

    let found_shard = shards.get_shard(&left_ancestor_mask).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.get_shard(&right_ancestor_mask).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.find_shard(&left_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let found_shard = shards.find_shard(&right_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    // 5400000000000000

    let shard = ShardIdent::with_tagged_prefix(
        0,
        0b0101010000000000000000000000000000000000000000000000000000000000,
    )
    .unwrap();

    let found_shard = shards.get_shard(&shard).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let shard2 = ShardIdent::with_tagged_prefix(
        0,
        0b0101010010000000000100000000000000000000000000000000000000000000,
    )
    .unwrap();
    let found_shard = shards.get_shard(&shard2).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.find_shard(&shard2).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let left_ancestor_mask = shard.left_ancestor_mask().unwrap();
    let right_ancestor_mask = shard.right_ancestor_mask().unwrap();

    let found_shard = shards.get_shard(&left_ancestor_mask).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.get_shard(&right_ancestor_mask).unwrap();
    assert!(found_shard.is_none());

    let found_shard = shards.find_shard(&left_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    let found_shard = shards.find_shard(&right_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), shard);

    // 1400000000000000 + 1c00000000000000 = 1800000000000000

    let shard = ShardIdent::with_tagged_prefix(0, 0x1800000000000000).unwrap();
    let left_ancestor = ShardIdent::with_tagged_prefix(0, 0x1400000000000000).unwrap();
    let right_ancestor = ShardIdent::with_tagged_prefix(0, 0x1c00000000000000).unwrap();

    let left_ancestor_mask = shard.left_ancestor_mask().unwrap();
    let right_ancestor_mask = shard.right_ancestor_mask().unwrap();

    let found_shard = shards.find_shard(&left_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), left_ancestor);

    let found_shard = shards.find_shard(&right_ancestor_mask).unwrap();
    assert!(found_shard.is_some());
    assert_eq!(*found_shard.unwrap().shard(), right_ancestor);
}

#[test]
fn test_serialization_shard_fees() {
    let mut shard_fees = ShardFees::default();

    //let mut summ = 0;
    for n in 1..12u32 {
        //summ += 2 * n * 100;
        let mut cc = CurrencyCollection::with_coins(n as u64 * 100);
        cc.set_other(n, n as u128).unwrap();
        let fee = ShardFeeCreated::with_fee(cc);
        let ident = ShardIdentFull::new(n as i32, 0x8000_0000_0000_0000);
        shard_fees.set_augmentable(&ident, &fee).unwrap();
        assert!(!shard_fees.is_empty());
        //assert_eq!(shard_fees.root_extra().fees.coins, summ.into());
    }

    write_read_and_assert(shard_fees);
}

#[test]
fn test_get_next_prev_key_block() {
    let bytes = std::fs::read("src/tests/data/free-ton-mc-state-61884").unwrap();
    let root = read_single_root_boc(&bytes).unwrap();
    let shard_state = ShardStateUnsplit::construct_from_cell(root).unwrap();
    let prev_blocks = &shard_state.read_custom().unwrap().unwrap().prev_blocks;

    // Find all key blocks by full hashmap's enumerating (brute force)
    let mut all_key_blocks = HashMap::new();
    let mut last_seqno = 0;
    prev_blocks
        .iterate_with_keys_and_aug(|seqno, id, aug| {
            if aug.key && seqno != 0 {
                println!("{:?}", id);
                all_key_blocks.insert(seqno, id);
            }
            last_seqno = seqno;
            Ok(true)
        })
        .unwrap();
    println!("last_seqno: {}", last_seqno);

    let mut seqno = 0;
    let mut key_blocks = vec![];
    while let Some(id) = prev_blocks.get_next_key_block(seqno + 1).unwrap() {
        println!("{:?}", id);
        seqno = id.seq_no;
        key_blocks.push(id);
    }
    assert_eq!(key_blocks.len(), all_key_blocks.len());
    for id in key_blocks.iter() {
        assert!(all_key_blocks.contains_key(&id.seq_no));
    }

    let key_id = key_blocks[key_blocks.len() - 1].clone();
    let id = prev_blocks.get_prev_key_block(key_id.seq_no).unwrap().unwrap();
    assert_eq!(id.root_hash, key_id.root_hash);

    let mut seqno = key_blocks[key_blocks.len() - 1].seq_no + 2;
    let mut key_blocks2 = vec![];
    while let Some(id) = prev_blocks.get_prev_key_block(seqno - 1).unwrap() {
        println!("{:?}", id);
        seqno = id.seq_no;
        if seqno == 0 {
            break;
        }
        key_blocks2.insert(0, id);
    }
    assert_eq!(key_blocks, key_blocks2);

    for id in key_blocks {
        let id = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: id.seq_no,
            root_hash: id.root_hash,
            file_hash: id.file_hash,
        };
        assert!(prev_blocks.check_block(&id).is_ok());

        let mut fake_id = id.clone();
        fake_id.root_hash = UInt256::from([123; 32]);
        assert!(prev_blocks.check_block(&fake_id).is_err());

        let mut fake_id = id.clone();
        fake_id.file_hash = UInt256::from([123; 32]);
        assert!(prev_blocks.check_block(&fake_id).is_err());

        let mut fake_id = id.clone();
        fake_id.shard_id = ShardIdent::with_workchain_id(BASE_WORKCHAIN_ID).unwrap();
        assert!(prev_blocks.check_block(&fake_id).is_err());
    }
}

#[test]
fn test_counters() {
    let mut c = Counters::default();
    assert!(c.increase_by(1, 100500));
    assert!(c.increase_by(1, 100501));
    assert!(c.increase_by(1, 100502));
    assert!(c.increase_by(1, 100503));
    assert_eq!(c.total(), 4);
}
