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
    bintree::BinTreeType,
    read_boc, read_single_root_boc,
    transactions::tests::{create_test_transaction_set, generate_test_shard_account_block},
    types::{AddSub, Coins},
    write_read_and_assert, AccountBlock, Cell, HashmapAugType, HashmapE, Message, MsgEnvelope,
    OutMsg, TickTock, Transaction,
};
use std::{
    fs::{read, read_dir},
    path::Path,
    str::FromStr,
};

#[test]
fn test_serialize_tick_tock() {
    let tt = TickTock::default();
    let mut tt = write_read_and_assert(tt);

    tt.tick = true;
    let mut tt = write_read_and_assert(tt);

    tt.tock = true;
    write_read_and_assert(tt);
}

fn test_blockinfo(block_info: BlockInfo) {
    let mut block_extra = BlockExtra::new();
    block_extra.write_account_blocks(&generate_test_shard_account_block()).unwrap();

    let mut collection = CurrencyCollection::with_coins(3);
    collection.set_other(1005004, 2_000_003).unwrap();

    let value_flow = ValueFlow {
        from_prev_blk: collection,
        to_next_blk: CurrencyCollection::default(),
        imported: CurrencyCollection::default(),
        exported: CurrencyCollection::default(),
        fees_collected: CurrencyCollection::default(),
        fees_imported: CurrencyCollection::default(),
        recovered: CurrencyCollection::default(),
        created: CurrencyCollection::default(),
        minted: CurrencyCollection::default(),
        burned: CurrencyCollection::default(),
    };

    let state_update = MerkleUpdate::default();

    let block = Block::with_params(0, block_info, value_flow, state_update, block_extra).unwrap();

    write_read_and_assert(block);
}

#[test]
#[should_panic]
fn test_block_info_with_invalid_seq_no() {
    let mut info = BlockInfo::new();
    info.set_seq_no(0).unwrap();
}

#[test]
#[should_panic]
fn test_block_info_with_invalid_prev_stuff_1() {
    let mut info = BlockInfo::new();
    info.set_prev_stuff(false, &BlkPrevInfo::default_blocks()).unwrap();
}
#[test]
#[should_panic]
fn test_block_info_with_invalid_prev_stuff_2() {
    let mut info = BlockInfo::new();
    info.set_prev_stuff(true, &BlkPrevInfo::default_block()).unwrap();
}

#[test]
#[should_panic]
fn test_block_info_with_invalid_vertical_stuff_1() {
    let mut info = BlockInfo::new();
    info.set_vertical_stuff(1, 0, None).unwrap();
}
#[test]
#[should_panic]
fn test_block_info_with_invalid_vertical_stuff_2() {
    let mut info = BlockInfo::new();
    info.set_vertical_stuff(1, 0, None).unwrap();
}

#[test]
fn test_block_info_with_seq_no() {
    let mut info = BlockInfo::new();
    info.set_seq_no(1).unwrap();
    info.set_prev_stuff(
        true,
        &BlkPrevInfo::Blocks {
            prev1: ChildCell::with_struct(&ExtBlkRef {
                end_lt: 1,
                seq_no: 1000,
                root_hash: UInt256::from([10; 32]),
                file_hash: UInt256::from([10; 32]),
            })
            .unwrap(),
            prev2: ChildCell::with_struct(&ExtBlkRef {
                end_lt: 1,
                seq_no: 999,
                root_hash: UInt256::from([10; 32]),
                file_hash: UInt256::from([10; 32]),
            })
            .unwrap(),
        },
    )
    .unwrap();
    write_read_and_assert(info.clone());

    info.set_vertical_stuff(
        1,
        32,
        Some(BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: 1000,
                root_hash: UInt256::from([10; 32]),
                file_hash: UInt256::from([10; 32]),
            },
        }),
    )
    .unwrap();
    write_read_and_assert(info);
}

#[test]
fn test_blockinfo_some_some_none() {
    let mut info = BlockInfo::new();
    info.set_shard(ShardIdent::with_workchain_id(0x22222222).unwrap());
    info.set_seq_no(u32::MAX - 22).unwrap();
    info.set_prev_stuff(
        false,
        &BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: 1000,
                root_hash: UInt256::from([10; 32]),
                file_hash: UInt256::from([10; 32]),
            },
        },
    )
    .unwrap();
    test_blockinfo(info);
}

#[test]
fn test_currency_collection() {
    let mut cc = CurrencyCollection::from_coins(Coins::one());
    cc.set_other(500, 9_000_000 + 777).unwrap();
    cc.set_other(1005001, 8_000_000 + 1005700).unwrap();
    cc.set_other(1005002, 555_000_000 + 1070500).unwrap();
    cc.set_other(10023, 1_000_000 + 1).unwrap();
    cc.set_other(1005004, 6_767_000_000 + 8888).unwrap();
    cc.set_other(10035, 13_000_000 + 1).unwrap();
    cc.set_other(1005006, 4_000_000 + 6).unwrap();
    cc.set_other(1005007, 5_000_000 + 7).unwrap();
    cc.set_other(10047, 1_000_000 + 1).unwrap();
    cc.set_other(10050, 1_111_000_000 + 100500).unwrap();
    cc.set_other(1001, 10_042_222_000_000 + 1006500).unwrap();
    cc.set_other(105, 1_000_000 + 1).unwrap();
    cc.set_other(1000, 2_000_000 + 5).unwrap();
    cc.set_other(10500, 3_000_000 + 6).unwrap();
    cc.set_other(10, 4_000_000 + 777).unwrap();
    cc.set_other(100, 74_000_000 + 7).unwrap();
    cc.set_other(1000, 1_000_000 + 1).unwrap();
    cc.set_other(1005000, 1_005_050_000_000 + 100500).unwrap();
    cc.set_other(80, 100_500_000_000 + 8).unwrap();

    write_read_and_assert(cc);
}

#[test]
fn test_value_flow() {
    let mut from_prev_blk = CurrencyCollection::with_coins(1);
    let mut to_next_blk = CurrencyCollection::with_coins(1);
    let mut imported = CurrencyCollection::with_coins(1);
    let mut exported = CurrencyCollection::with_coins(1);
    let mut fees_collected = CurrencyCollection::with_coins(1);
    let mut fees_imported = CurrencyCollection::with_coins(1);
    let mut recovered = CurrencyCollection::with_coins(1);
    let mut created = CurrencyCollection::with_coins(1);
    let mut minted = CurrencyCollection::with_coins(1);
    let mut burned = CurrencyCollection::with_coins(1);

    from_prev_blk.set_other(1001, 1_000_000 + 1).unwrap();
    from_prev_blk.set_other(100500, 9_000_000 + 777).unwrap();
    from_prev_blk.set_other(100500, 8_000_000 + 1005700).unwrap();
    from_prev_blk.set_other(100500, 555_000_000 + 1070500).unwrap();

    to_next_blk.set_other(1002, 1_000_000 + 1).unwrap();
    to_next_blk.set_other(100500, 6_767_000_000 + 8888).unwrap();

    imported.set_other(1003, 1_000_000 + 1).unwrap();
    imported.set_other(100500, 4_000_000 + 6).unwrap();
    imported.set_other(100500, 5_000_000 + 7).unwrap();

    exported.set_other(1004, 1_000_000 + 1).unwrap();
    exported.set_other(100500, 1_111_000_000 + 100500).unwrap();
    exported.set_other(100500, 1_002_222_000_000 + 100500).unwrap();

    fees_collected.set_other(1005, 1_000_000 + 1).unwrap();
    fees_collected.set_other(100500, 2_000_000 + 5).unwrap();
    fees_collected.set_other(100500, 3_000_000 + 6).unwrap();
    fees_collected.set_other(100500, 4_000_000 + 777).unwrap();

    fees_imported.set_other(100500, 123).unwrap();

    recovered.set_other(100500, 321).unwrap();

    created.set_other(100, 7_000_000 + 7).unwrap();

    minted.set_other(100, 1_000_000 + 1).unwrap();
    minted.set_other(100500, 100_500_000_000 + 100500).unwrap();
    minted.set_other(8, 100_500_000_000 + 8).unwrap();

    burned.set_other(2100, 1_000_000 + 2).unwrap();
    burned.set_other(2100500, 100_500_000_000 + 100500 + 2).unwrap();
    burned.set_other(28, 100_500_000_000 + 8 + 2).unwrap();

    let value_flow = ValueFlow {
        from_prev_blk,
        to_next_blk,
        imported,
        exported,
        fees_collected,
        fees_imported,
        recovered,
        created,
        minted,
        burned,
    };

    write_read_and_assert(value_flow);
}

fn read_file_de_and_serialise(filename: &Path) -> Cell {
    let orig_bytes =
        read(Path::new(filename)).unwrap_or_else(|_| panic!("Error reading file {:?}", filename));
    let mut root_cells = read_boc(orig_bytes).expect("Error deserializing BOC").roots;
    root_cells.remove(0)
}

#[test]
fn test_real_ton_boc() {
    for entry in read_dir(Path::new("src/tests/data")).expect("Error reading BOCs dir") {
        let entry = entry.unwrap();
        let in_path = entry.path();
        if !in_path.is_dir() {
            if let Some(_in_file_name) = in_path.clone().file_name() {
                if match in_path.extension() {
                    Some(ext) => ext != "boc",
                    _ => true,
                } {
                    continue;
                }
                println!("BOC file: {:?}", in_path);
                read_file_de_and_serialise(&in_path);
            }
        }
    }
}

#[test]
fn test_real_ton_mgs() {
    //let in_path = Path::new("src/tests/data/wallet-query.boc");
    //let in_path = Path::new("src/tests/data/new-wallet-query.boc");
    //let in_path = Path::new("src/tests/data/send-to-query.boc");
    let in_path = Path::new("src/tests/data/int-msg-query.boc");

    println!("MSG file: {:?}", in_path);
    let root_cell = read_file_de_and_serialise(in_path);

    println!("slice = {}", root_cell);
    let msg = Message::construct_from_cell(root_cell).unwrap();
    println!("Message = {:?}", msg);
}

fn test_real_block(in_path: &Path) -> Block {
    println!();
    println!("Block file: {:?}", in_path);
    let root_cell = read_file_de_and_serialise(in_path);
    // println!("slice = {}", root_cell);

    let block = Block::construct_from_cell(root_cell.clone()).unwrap();

    // TODO: Restore output
    // println!("Block:\n{}\n\n", serde_json::to_string_pretty(&block).unwrap());

    // block.extra().in_msg_descr().iterate(|mut in_msg| {
    //     if let Some(msg) = in_msg.message_mut() {
    //         println!("InMsg:\n{}\n\n", serde_json::to_string_pretty(&msg).unwrap());
    //     }
    //     Ok(true)
    // }).unwrap();

    // block.extra().out_msg_descr().iterate(|mut out_msg| {
    //     if let Some(msg) = out_msg.message_mut() {
    //         println!("OutMsg:\n{}\n\n", serde_json::to_string_pretty(&msg).unwrap());
    //     }
    //     Ok(true)
    // }).unwrap();

    // block.extra().account_blocks().iterate(|account_block| {
    //     println!("AccountBlock ID: {:?}", account_block.account_id());
    //     account_block.transaction_iterate(|transaction| {
    //         println!("\nTransaction: {}\n\n", serde_json::to_string_pretty(&transaction).unwrap());
    //         Ok(true)
    //     })?;
    //     Ok(true)
    // }).unwrap();

    let extra = block.read_extra().unwrap();
    if let Some(custom) = extra.read_custom().unwrap() {
        println!("McBlockExtra\n\nShardes");
        custom
            .hashes()
            .iterate_with_keys(|key, InRefValue(shard_hashes)| {
                println!("\nnext workchain");
                shard_hashes.iterate(|shard, shard_descr| {
                    let shard_ident = ShardIdent::with_prefix_slice(key, shard)?;
                    println!(
                        "\n\nshard: {}, shard_descr: {:?}\n\n",
                        shard_ident.shard_prefix_as_str_with_tag(),
                        shard_descr
                    );
                    Ok(true)
                })?;
                Ok(true)
            })
            .unwrap();

        println!("Fees");
        custom
            .fees()
            .iterate_with_keys(|key, shard_fees| {
                println!("\n\nkey: {:?}, shard_fees: {:?}\n\n", key, shard_fees);
                Ok(true)
            })
            .unwrap();
    }

    extra
        .read_in_msg_descr()
        .unwrap()
        .iterate_objects(|in_msg| {
            println!();
            println!("InMsg: {:?}", in_msg);
            Ok(true)
        })
        .unwrap();

    extra
        .read_out_msg_descr()
        .unwrap()
        .iterate_objects(|out_msg| {
            println!();
            println!("OutMsg: {:?}", out_msg);
            Ok(true)
        })
        .unwrap();

    extra
        .read_account_blocks()
        .unwrap()
        .iterate_objects(|account_block| {
            //println!("AccountBlock: {:?}", account_block);
            println!("AccountBlock ID: {:?}", account_block.account_id());
            account_block.transaction_iterate(|transaction| {
                println!();
                println!("\tTransaction: {:?}", transaction);
                Ok(true)
            })?;
            Ok(true)
        })
        .unwrap();
    println!();

    let cell = block.serialize().unwrap();
    assert_eq!(root_cell, cell);
    write_read_and_assert(block)
}

#[test]
fn test_real_ton_key_block() {
    let in_path = Path::new("src/tests/data/key_block.boc");
    let block = test_real_block(in_path);
    let block = Block::construct_from_cell(block.serialize().unwrap()).unwrap();

    if let Some(custom) = block.read_extra().unwrap().read_custom().unwrap() {
        if let Some(c) = custom.config() {
            crate::config_params::dump_config(&c.config_params);
            // let bytes = serialize_toc(c.config_params.data().unwrap()).unwrap();
            // std::fs::write("src/tests/data/config.boc", bytes).unwrap();
        }
    }
}

#[test]
fn test_all_real_ton_block_with_transaction() {
    for entry in read_dir(Path::new("src/tests/data/block_with_transaction"))
        .expect("Error reading BOCs dir")
    {
        let entry = entry.unwrap();
        let in_path = entry.path();
        if !in_path.is_dir() {
            if let Some(_in_file_name) = in_path.clone().file_name() {
                if match in_path.extension() {
                    Some(ext) => ext != "boc",
                    _ => true,
                } {
                    continue;
                }
                test_real_block(&in_path);
            }
        }
    }
}

#[test]
fn test_real_ton_config() {
    // to get current config run lite_client with saveconfig config.boc
    let in_path = Path::new("src/tests/data/config.boc");
    println!("Config file: {:?}", in_path);
    let root_cell = read_file_de_and_serialise(in_path);
    println!("cell = {:#.2}", root_cell);

    crate::config_params::dump_config(&HashmapE::with_hashmap(32, Some(root_cell)));
}

#[test]
fn test_block_id_ext() {
    let b = BlockIdExt::default();
    let b1 = BlockIdExt::default();

    assert_eq!(b, b1);

    let b = BlockIdExt::with_params(
        ShardIdent::default(),
        3784685,
        UInt256::from([1; 32]),
        UInt256::from([2; 32]),
    );

    write_read_and_assert(b);
}

#[test]
fn test_block_id_ext_from_str() {
    let id1: BlockIdExt = "(0:1800000000000000, 1203696, \
         rh 59b6e56610aa5df5e8ee4cc5f1081cd5d08473f10e0899f7763d580b2a635f90, \
         fh 1b4d177339538562d10166d87823783b7e747ee80d85d033459928fd0605a126)"
        .parse()
        .unwrap();

    let id2 = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0x1800000000000000).unwrap(),
        1203696,
        UInt256::from_str("59b6e56610aa5df5e8ee4cc5f1081cd5d08473f10e0899f7763d580b2a635f90")
            .unwrap(),
        UInt256::from_str("1b4d177339538562d10166d87823783b7e747ee80d85d033459928fd0605a126")
            .unwrap(),
    );

    assert_eq!(id1, id2);

    let id1: BlockIdExt = "(-1:8000000000000000, 994703, \
         rh 04da9f61d063d49a5bb4e0c253ed81e1e2a27513e77d630a9aca1e29971fbf4e, \
         fh ba1059b7a17104b4b44742326076e8394f21c1a1dd21fc1b3737d3ca8d779756)"
        .parse()
        .unwrap();

    let id2 = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(-1, 0x8000000000000000).unwrap(),
        994703,
        UInt256::from_str("04da9f61d063d49a5bb4e0c253ed81e1e2a27513e77d630a9aca1e29971fbf4e")
            .unwrap(),
        UInt256::from_str("ba1059b7a17104b4b44742326076e8394f21c1a1dd21fc1b3737d3ca8d779756")
            .unwrap(),
    );

    assert_eq!(id1, id2);

    let id3 = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(-1, 0x8000000000000000).unwrap(),
        31333,
        UInt256::from_str("04da9f616763d49a5bb4e0c253ed81e1e2a27513e77d630a9aca1e29971fbf4e")
            .unwrap(),
        UInt256::from_str("ba1059b7a17104b4b44742323336e8394f21c1a1dd21fc1b376643ca8d779756")
            .unwrap(),
    );

    assert_eq!(id3, id3.to_string().parse().unwrap());
}

#[test]
fn calc_value_flow() {
    // master
    //let boc =
    //    "src/tests/data/91FDE9DA6661FE9D1FCB013C1079411AFC7BFEDF7FE533C6FD48D25388A3FC26.boc";
    // new testnet WC
    //let boc =
    //    "src/tests/data/EC6D799FC7EA14D9FD1D840542514BA774EA3FB5E04B70B6E314C48F95B0C131.boc";
    // old testnet WC
    let boc = "src/tests/data/9C2B3FC5AD455917D374CFADBED8FC2343E31A27C1DF2EB29E84404FA96DE9F8.boc";
    let root_cell = read_file_de_and_serialise(Path::new(boc));
    let block = Block::construct_from_cell(root_cell).unwrap();
    let mut new_transaction_fees = Coins::default();

    block
        .read_extra()
        .unwrap()
        .read_account_blocks()
        .unwrap()
        .iterate_objects(|account_block: AccountBlock| {
            new_transaction_fees.add(&account_block.transactions().root_extra().coins)?;
            Ok(true)
        })
        .unwrap();

    let import_fees = block.read_extra().unwrap().read_in_msg_descr().unwrap().root_extra().clone();
    let mut fees_collected = import_fees.fees_collected;
    let value_imported = import_fees.value_imported;
    let exported = block.read_extra().unwrap().read_out_msg_descr().unwrap().root_extra().coins;

    fees_collected.add(&new_transaction_fees).unwrap();

    let ethalon_value_flow = block.read_value_flow().unwrap();

    println!("exported       = {:12}", ethalon_value_flow.exported.coins);
    println!("fees_imported  = {:12}", ethalon_value_flow.fees_imported.coins);
    println!("recovered      = {:12}", ethalon_value_flow.recovered.coins);
    println!("created        = {:12}", ethalon_value_flow.created.coins);
    println!("minted         = {:12}", ethalon_value_flow.minted.coins);
    println!("imported       = {:12}", ethalon_value_flow.imported.coins);
    println!("fees_collected = {:12}", ethalon_value_flow.fees_collected.coins);

    let created = Coins::from(1_000_000_000 / 4); // 1G / 4 shards  // 1_700_000_000 for masterchain

    fees_collected.add(&ethalon_value_flow.fees_imported.coins).unwrap(); // TODO calc it
    fees_collected.add(&created).unwrap(); // TODO calc it

    assert_eq!(ethalon_value_flow.exported.coins, exported);
    // assert_eq!(ethalon_value_flow.fees_imported.coins, // TODO
    // assert_eq!(ethalon_value_flow.recovered.coins, // TODO
    assert_eq!(ethalon_value_flow.created.coins, created);
    // assert_eq!(ethalon_value_flow.minted.coins, // TODO
    assert_eq!(ethalon_value_flow.imported, value_imported);
    assert_eq!(ethalon_value_flow.fees_collected.coins, fees_collected);
}

#[test]
fn test_read_tob_block_descr() {
    let data = std::fs::read("src/tests/data/top_block_descr.boc").unwrap();
    let cell = read_single_root_boc(data).unwrap();
    let descr = TopBlockDescr::construct_from_cell(cell).unwrap();
    println!("{:?}", descr);
}

#[test]
fn block_info_serde() {
    let block_info =
        super::BlockInfo { version: 0, gen_utime: 1684756262u32.into(), ..Default::default() };
    let serialized = block_info.serialize().unwrap();
    let deserialized =
        super::BlockInfo::construct_from(&mut SliceData::load_cell(serialized).unwrap()).unwrap();
    {
        let gen_utime = 1684756262u32;
        assert_eq!(deserialized.gen_utime(), gen_utime);
    }
    assert_eq!(block_info, deserialized);
}

fn create_test_block() -> Block {
    let mut outmsg_descr = OutMsgDescr::default();
    let trans_data = create_test_transaction_set();
    let enveloped = MsgEnvelope::with_message_and_fee(&Message::default(), 1.into()).unwrap();
    let mut tr =
        Transaction::with_address_and_status(trans_data.account_id, trans_data.orig_status.clone());
    for ref msg in trans_data.out_msgs {
        tr.add_out_message(msg).unwrap();
    }
    tr.write_in_msg(Some(&trans_data.in_msg)).unwrap();
    tr.set_logical_time(trans_data.lt);
    tr.orig_status = trans_data.orig_status;
    let out_msg = OutMsg::new(
        ChildCell::with_struct(&enveloped).unwrap(),
        ChildCell::with_struct(&tr).unwrap(),
    );
    outmsg_descr.insert(&out_msg).unwrap();
    let mut block_extra = BlockExtra::new();
    block_extra.write_account_blocks(&generate_test_shard_account_block()).unwrap();
    block_extra.write_out_msg_descr(&outmsg_descr).unwrap();

    let block_info = BlockInfo::new();
    let value_flow = ValueFlow::default();
    let state_update = MerkleUpdate::default();

    Block::with_params(1, block_info, value_flow, state_update, block_extra).unwrap()
}

#[test]
fn test_serde_block() {
    let block = create_test_block();
    let cell = block.serialize().unwrap();
    let block2 = Block::construct_from_cell(cell.clone()).unwrap();
    assert_eq!(block, block2);
}
