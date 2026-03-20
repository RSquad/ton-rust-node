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
    blocks::{BlkPrevInfo, BlockExtra, ExtBlkRef, ValueFlow},
    shard::ShardIdent,
    BocReader, BuilderData, Cell, CellType, Serializable, UsageTree,
};
use std::{collections::HashSet, fs::File};

#[test]
fn test_merkle_proof_invalid_arg() {
    let mut root = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root.append_raw(&[0], 1).unwrap();
    a.append_raw(&[1], 2).unwrap();
    b.append_raw(&[2], 3).unwrap();

    root.checked_append_reference(a.into_cell().unwrap()).unwrap();
    root.checked_append_reference(b.into_cell().unwrap()).unwrap();

    let mut proof_for = HashSet::new();
    proof_for.insert(UInt256::default());

    MerkleProof::create(&root.into_cell().unwrap(), |h| proof_for.contains(h))
        .expect_err("MerkleProof::create have to return error");
}

#[test]
fn test_merkle_proof1() {
    let mut root = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root.append_raw(&[0], 1).unwrap();
    a.append_raw(&[1], 2).unwrap();
    b.append_raw(&[2], 3).unwrap();

    root.checked_append_reference(a.into_cell().unwrap()).unwrap();
    root.checked_append_reference(b.into_cell().unwrap()).unwrap();

    let root = root.into_cell().unwrap();

    let mut proof_for = HashSet::new();
    proof_for.insert(root.repr_hash());
    proof_for.insert(root.reference(0).unwrap().repr_hash());

    let proof = MerkleProof::create(&root, |h| proof_for.contains(h)).unwrap();

    assert_eq!(&proof.hash.as_slice(), &root.repr_hash().as_slice());

    assert_eq!(proof.proof.level(), 1);
    let ref0 = &proof.proof.reference(0).unwrap();
    assert_eq!(ref0.cell_type(), CellType::Ordinary);
    assert_eq!(ref0.level(), 0);
    let ref1 = &proof.proof.reference(1).unwrap();
    assert_eq!(ref1.cell_type(), CellType::PrunedBranch);
    assert_eq!(ref1.level(), 1);

    let proof_root = proof.write_to_new_cell().unwrap();
    let proof2: MerkleProof =
        MerkleProof::construct_from_cell(proof_root.into_cell().unwrap()).unwrap();

    assert_eq!(proof, proof2);
}

fn create_cell(bytes: &[u8], refs: &[&Cell]) -> Cell {
    let mut c = BuilderData::new();
    c.append_raw(bytes, bytes.len() * 8).unwrap();
    for child in refs {
        c.checked_append_reference((*child).clone()).unwrap();
    }
    c.into_cell().unwrap()
}

#[test]
fn test_merkle_proof_with_subtrees() {
    /*
          root
      c5        c6
    c1  c2    c3  c4
              c1  c2
    */
    let c1 = create_cell(&[1, 1, 1], &[]);
    let c2 = create_cell(&[2, 2, 2], &[]);
    let c3 = create_cell(&[3, 3, 3], &[&c1]);
    let c4 = create_cell(&[4, 4, 4], &[&c2]);
    let c5 = create_cell(&[5, 5, 5], &[&c1, &c2]);
    let c6 = create_cell(&[6, 6, 6], &[&c3, &c4]);
    let tree = create_cell(&[1], &[&c5, &c6]);

    // proof for c6 only

    let proof =
        MerkleProof::create(&tree, |h| h == &tree.repr_hash() || h == &c6.repr_hash()).unwrap();

    let virt_tree: Cell = proof.proof.virtualize(1);
    assert!(virt_tree.repr_hash() == tree.repr_hash());

    virt_tree.reference(1).unwrap(); // c6

    assert!(virt_tree.reference(0).unwrap().cell_type() == CellType::PrunedBranch); // c5

    assert!(
        virt_tree
            .reference(1)
            .unwrap() // c6
            .reference(0)
            .unwrap()
            .cell_type()
            == CellType::PrunedBranch // c3
    );

    assert!(
        virt_tree
            .reference(1)
            .unwrap() // c6
            .reference(1)
            .unwrap()
            .cell_type()
            == CellType::PrunedBranch // c3
    );

    // proof for c4's subtree

    let proof = MerkleProof::create_with_subtrees(
        &tree,
        |h| h == &tree.repr_hash() || h == &c6.repr_hash(),
        |h| h == &c4.repr_hash(),
    )
    .unwrap();

    let virt_tree: Cell = proof.proof.virtualize(1);
    assert!(virt_tree.repr_hash() == tree.repr_hash());

    virt_tree
        .reference(1)
        .unwrap() // c6
        .reference(1)
        .unwrap() // c4
        .reference(0)
        .unwrap(); // c2

    assert!(virt_tree.reference(0).unwrap().cell_type() == CellType::PrunedBranch); // c5

    assert!(
        virt_tree
            .reference(1)
            .unwrap() // c6
            .reference(0)
            .unwrap()
            .cell_type()
            == CellType::PrunedBranch // c3
    );
}

#[test]
fn test_merkle_proof_hi_hashes() {
    // Construct 2 trees & Merkle update for it

    let mut root1 = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root1.append_raw(&[0], 1).unwrap();
    a.append_raw(&[1], 2).unwrap();
    b.append_raw(&[2], 3).unwrap();
    root1.checked_append_reference(a.clone().into_cell().unwrap()).unwrap();
    root1.checked_append_reference(b.into_cell().unwrap()).unwrap();

    let mut root2 = BuilderData::new();
    let mut b = BuilderData::new();

    root2.append_raw(&[0], 1).unwrap();
    b.append_raw(&[3], 7).unwrap();
    root2.checked_append_reference(a.into_cell().unwrap()).unwrap();
    root2.checked_append_reference(b.into_cell().unwrap()).unwrap();

    let root1 = root1.into_cell().unwrap();
    let root2 = root2.into_cell().unwrap();

    let update = MerkleUpdate::create(&root1, &root2).unwrap();

    // Construct block and insert Merkle update into
    // (it is not matter the update is not for shard state.
    // The update just must contain pruned branches)

    let mut block_info = BlockInfo::new();
    block_info.set_shard(ShardIdent::with_workchain_id(0x22222222).unwrap());
    block_info.set_seq_no(u32::MAX - 22).unwrap();
    block_info
        .set_prev_stuff(
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

    let block =
        Block::with_params(0, block_info, ValueFlow::default(), update, BlockExtra::default())
            .unwrap();
    let block_root = block.serialize().unwrap();

    // construct usage tree
    let usage_tree = UsageTree::with_root(block_root.clone());
    let block = Block::construct_from_cell(usage_tree.root_cell()).unwrap();
    block.read_info().unwrap();
    block.read_state_update().unwrap();

    // construct proof
    let block_proof = MerkleProof::create_by_usage_tree(&block_root, &usage_tree).unwrap();

    // construct proof BOC
    let proof_root = block_proof.serialize().unwrap();

    println!("{:#.222}", proof_root);
    println!("{:#.2}", block_root);

    // check block's repr hash is equal proof's higher hash
    assert_eq!(
        block_root.repr_hash().as_slice(),
        Cell::hash(&proof_root.reference(0).unwrap(), 0).as_slice()
    );
}

#[test]
fn test_merkle_proof_hi_hashes2() {
    // Construct 2 trees & Merkle update for it
    // old update's branch must be fully pruned

    let mut root1 = BuilderData::new();

    root1.append_raw(&[0], 1).unwrap();

    let mut root2 = BuilderData::new();
    let mut b = BuilderData::new();

    root2.append_raw(&[0], 2).unwrap();
    b.append_raw(&[3], 7).unwrap();
    root2.checked_append_reference(b.into_cell().unwrap()).unwrap();

    let root1 = root1.into_cell().unwrap();
    let root2 = root2.into_cell().unwrap();

    let update = MerkleUpdate::create(&root1, &root2).unwrap();

    // Construct block and insert Merkle update into
    // (it is not matter the update is not for shard state.
    // The update just must contain pruned branches)

    let block = Block::with_params(
        0,
        BlockInfo::default(),
        ValueFlow::default(),
        update,
        BlockExtra::default(),
    )
    .unwrap();
    let block_root = block.serialize().unwrap();

    // construct usage tree
    let usage_tree = UsageTree::with_root(block_root.clone());
    let block = Block::construct_from_cell(usage_tree.root_cell()).unwrap();
    block.read_info().unwrap();
    block.read_state_update().unwrap();

    // construct proof
    let block_proof = MerkleProof::create_by_usage_tree(&block_root, &usage_tree).unwrap();

    // construct proof BOC
    let proof_root = block_proof.write_to_new_cell().unwrap().into_cell().unwrap();

    println!("{:#.222}", proof_root);
    println!("{:#.222}", block_root);

    // check block's repr hash is equal proof's higher hash
    assert_eq!(
        block_root.repr_hash().as_slice(),
        Cell::hash(&proof_root.reference(0).unwrap(), 0).as_slice()
    );
}

fn get_real_ton_block(filename: &str) -> (Block, Cell) {
    let root = BocReader::new()
        .read(&mut File::open(filename).expect("Error open boc file"))
        .expect("Error deserializing boc file")
        .withdraw_single_root()
        .expect("Error deserializing boc file - expected one root");
    let block = Block::construct_from_cell(root.clone()).expect("error deserializing block");

    (block, root)
}

fn get_real_ton_state(filename: &str) -> (ShardStateUnsplit, Cell) {
    let root = BocReader::new()
        .read(&mut File::open(filename).expect("Error open boc file"))
        .expect("Error deserializing boc file")
        .withdraw_single_root()
        .expect("Error deserializing boc file - expected one root");
    let state =
        ShardStateUnsplit::construct_from_cell(root.clone()).expect("error deserializing state");

    (state, root)
}

#[test]
fn test_check_block_info_proof() {
    let block_files = vec![
        "src/tests/data/8A8270ED58F5F982EFC3A16DA19D3EF99D12D7A9E0039B911213D0F2940A1B29.boc",
        "src/tests/data/55A29231AD8FC6C6FF85C9EF430EC9F9D76B35F21A3A5C963CAD3B60701AEF48.boc",
    ];

    for block_file in block_files {
        let (_, block_root) = get_real_ton_block(block_file);

        // construct usage tree
        let usage_tree = UsageTree::with_root(block_root.clone());
        let block = Block::construct_from_cell(usage_tree.root_cell()).unwrap();
        block.read_info().unwrap();

        // construct proof
        let proof = MerkleProof::create_by_usage_tree(&block_root, &usage_tree)
            .expect("error building proof");

        let block = Block::construct_from_cell(proof.proof.clone()).unwrap();

        let info2 = check_block_info_proof(&block, &proof.hash, &block_root.repr_hash()).unwrap();
        assert_eq!(block.read_info().unwrap(), info2);
    }
}

fn get_tr_from_block(block: &Block) -> Transaction {
    let mut transaction = None;
    block
        .read_extra()
        .unwrap()
        .read_account_blocks()
        .unwrap()
        .iterate_objects(|account_block| {
            account_block.transaction_iterate(|tr| {
                transaction = Some(tr);
                Ok(false)
            })?;
            Ok(false)
        })
        .unwrap();
    transaction.unwrap()
}

fn test_check_transaction_proof(wrong: bool, block_file: &str) -> Result<()> {
    let (block, block_root) = get_real_ton_block(block_file);
    let mut transaction = get_tr_from_block(&block);
    if wrong {
        transaction.set_now(123);
    }

    let proof = transaction.prepare_proof(&block_root).unwrap();

    println!(
        "check proof for transaction acc: {:x}  lt:{}  hash:{:x}",
        transaction.account_id(),
        transaction.logical_time(),
        transaction.hash().unwrap()
    );

    let proof = MerkleProof::construct_from_cell(proof).unwrap();
    check_transaction_proof(&proof, &transaction, &block.hash().unwrap())?;
    Ok(())
}

#[test]
fn test_check_wrong_transaction_proof() {
    let block_files = vec![
        "src/tests/data/8A8270ED58F5F982EFC3A16DA19D3EF99D12D7A9E0039B911213D0F2940A1B29.boc",
        "src/tests/data/3F7B3F53F9F0684E29D67B813E2197689FE725A77491BD50A5438EA66D4341E0.boc",
        "src/tests/data/CF475DF9B65917A490AE96B021F68FF3AEC6848880C90BD3959626A2D56E5427.boc",
    ];

    for block_file in block_files {
        println!("check wrong proof for {}", block_file);
        match test_check_transaction_proof(true, block_file) {
            Result::Err(err) => println!("{}", err),
            res => panic!("unexpected result: {:?}", res),
        }
    }
}

#[test]
fn test_check_correct_transaction_proof() {
    let block_files = vec![
        "src/tests/data/8A8270ED58F5F982EFC3A16DA19D3EF99D12D7A9E0039B911213D0F2940A1B29.boc",
        "src/tests/data/3F7B3F53F9F0684E29D67B813E2197689FE725A77491BD50A5438EA66D4341E0.boc",
        "src/tests/data/CF475DF9B65917A490AE96B021F68FF3AEC6848880C90BD3959626A2D56E5427.boc",
    ];

    for block_file in block_files {
        println!("check correct proof, block: {}", block_file);
        match test_check_transaction_proof(false, block_file) {
            Result::Ok(_) => println!("OK"),
            res => panic!("unexpected result: {:?}", res),
        }
    }
}

fn get_in_msg_from_block(block: &Block) -> (Option<Message>, Option<UInt256>) {
    let mut msg = None;
    let mut tr = None;
    block
        .read_extra()
        .unwrap()
        .read_in_msg_descr()
        .unwrap()
        .iterate_with_keys(|key, in_msg| {
            let msg1 = in_msg.read_message().unwrap();
            assert_eq!(key, msg1.hash().unwrap());
            msg = Some(msg1);
            tr = in_msg.transaction_cell().map(|c| c.repr_hash());
            Ok(false)
        })
        .unwrap();
    (msg, tr)
}

fn get_out_msg_from_block(block: &Block) -> (Option<Message>, Option<UInt256>) {
    let mut msg = None;
    let mut tr = None;
    block
        .read_extra()
        .unwrap()
        .read_out_msg_descr()
        .unwrap()
        .iterate_with_keys(|key, out_msg| {
            if let Some(msg1) = out_msg.read_message().unwrap() {
                println!("{}", key);
                println!("{}", msg1.hash().unwrap());
                msg = Some(msg1);
                tr = out_msg.transaction_cell().map(|c| c.repr_hash());
                Ok(false)
            } else {
                Ok(true)
            }
        })
        .unwrap();
    (msg, tr)
}

#[test]
fn test_check_msg_proof() {
    let block_files = [
        "src/tests/data/9D134C5ABBC859B6ED7A7201757BA4CB5E837641C6E5AEACA31DDD4B1B3D51A2.boc",
        "src/tests/data/8A8270ED58F5F982EFC3A16DA19D3EF99D12D7A9E0039B911213D0F2940A1B29.boc",
        "src/tests/data/3F7B3F53F9F0684E29D67B813E2197689FE725A77491BD50A5438EA66D4341E0.boc",
        "src/tests/data/CF475DF9B65917A490AE96B021F68FF3AEC6848880C90BD3959626A2D56E5427.boc",
    ];

    for (i, block_file) in block_files.iter().enumerate() {
        let (block, block_root) = get_real_ton_block(block_file);

        let block_id = block.hash().unwrap();

        if let (Some(msg), tr_id) = get_in_msg_from_block(&block) {
            println!("{} Test in msg {:x}", i, msg.hash().unwrap());
            let proof_cell = msg.prepare_proof(true, &block_root).unwrap();
            let proof: MerkleProof = MerkleProof::construct_from_cell(proof_cell).unwrap();
            check_message_proof(&proof, &msg, &block_id, tr_id)
                .expect("error checking in message proof");
        }

        if let (Some(msg), tr_id) = get_out_msg_from_block(&block) {
            println!("{} Test out msg {:x}", i, msg.hash().unwrap());
            let proof_cell = msg.prepare_proof(false, &block_root).unwrap();
            let proof: MerkleProof = MerkleProof::construct_from_cell(proof_cell).unwrap();
            check_message_proof(&proof, &msg, &block_id, tr_id)
                .expect("error checking out message proof");
        }
    }
}

fn test_check_account_proof(
    wrong: bool,
    mut account: Account,
    state_root: &Cell,
) -> Result<BlockSeqNoAndShard> {
    let proof_cell = account.prepare_proof(state_root).unwrap();
    let proof = MerkleProof::construct_from_cell(proof_cell).unwrap();

    if wrong {
        account.set_last_tr_time(123456);
    }

    check_account_proof(&proof, &account)
}

#[test]
fn test_check_correct_account_proof() {
    let state_files =
        vec!["src/tests/data/7992DD77CEB677577A7D5A8B6F388CDA76B4D0DDE16FF5004C87215E6ADF84DD.boc"];

    for state_file in state_files {
        println!("state file: {}", state_file);

        let (state, state_root) = get_real_ton_state(state_file);

        state
            .read_accounts()
            .unwrap()
            .iterate_objects(|account| {
                let account = account.read_account().unwrap();

                println!("account: {}", account.get_id().unwrap());

                let block_id = test_check_account_proof(false, account, &state_root)
                    .expect("error checking proof");

                assert_eq!(block_id.seq_no, state.seq_no());
                assert_eq!(block_id.vert_seq_no, state.vert_seq_no());
                assert_eq!(&block_id.shard_id, state.shard());

                Ok(true)
            })
            .unwrap();
    }
}

#[test]
fn test_check_wrong_account_proof() {
    let state_files =
        vec!["src/tests/data/7992DD77CEB677577A7D5A8B6F388CDA76B4D0DDE16FF5004C87215E6ADF84DD.boc"];

    for state_file in state_files {
        println!("state file: {}", state_file);

        let (state, state_root) = get_real_ton_state(state_file);

        state
            .read_accounts()
            .unwrap()
            .iterate_objects(|account| {
                let account = account.read_account().unwrap();

                println!("account: {}", account.get_id().unwrap());

                match test_check_account_proof(true, account, &state_root) {
                    Result::Err(err) => println!("{}", err),
                    res => panic!("unexpected result: {:?}", res),
                }

                Ok(true)
            })
            .unwrap();
    }
}

#[test]
fn test_inner_merkle_proof() {
    /*
          root
      c5        c6
    c1  c2    c3  c4
              c1  c2
    */
    let mut hashes = HashSet::new();
    let c1 = create_cell(&[1, 1, 1], &[]);
    hashes.insert(c1.repr_hash());
    let c2 = create_cell(&[2, 2, 2], &[]);
    hashes.insert(c2.repr_hash());
    let c3 = create_cell(&[3, 3, 3], &[&c1]);
    hashes.insert(c3.repr_hash());
    let c4 = create_cell(&[4, 4, 4], &[&c2]);
    hashes.insert(c4.repr_hash());
    let c5 = create_cell(&[5, 5, 5], &[&c1, &c2]);
    let c6 = create_cell(&[6, 6, 6], &[&c3, &c4]);
    hashes.insert(c6.repr_hash());
    let tree = create_cell(&[1], &[&c5, &c6]);
    hashes.insert(tree.repr_hash());

    // proof for c1 and c2
    /*
          root
    pruned      c6
              c3  c4
              c1  c2
    */

    let proof1 = MerkleProof::create(&tree, |h| hashes.contains(h)).unwrap().serialize().unwrap();
    // println!("proof 1\n{:#.100}", proof1);

    // proof for c1 in proof
    /*
              root
    pruned_001      c6
                  c3  pruned_010
                  c1
        */
    let mut hashes2 = HashSet::new();
    hashes2.insert(proof1.repr_hash());
    let c6 = proof1.reference(0).unwrap();
    hashes2.insert(c6.repr_hash());
    let c3 = c6.reference(1).unwrap();
    hashes2.insert(c3.repr_hash());
    let c1 = c3.reference(0).unwrap();
    hashes2.insert(c1.repr_hash());

    let proof2 =
        MerkleProof::create(&proof1, |h| hashes2.contains(h)).unwrap().serialize().unwrap();
    let tree = format!("{:#.100}", proof2);
    // println!("proof 2\n {}", tree);
    assert_eq!(tree,
"Merkle proof   l: 000   bits: 280   refs: 1   data: 03fc5004bc31fd26d8fe7c1bafc45490fa28d5ea7a0dc52e8079317fff57bbbba80004
hashes: e4ae22ed3e06bd796b5bd450c6843669b9eba4c1a1d3d9f6725bbeef4536e271
depths: 5
 └─Merkle proof   l: 001   bits: 280   refs: 1   data: 036b663d94d562c0682bd6a8be41c639b6e52f87c20ef5776795e4ad7fdbcf04610003
   hashes: fc5004bc31fd26d8fe7c1bafc45490fa28d5ea7a0dc52e8079317fff57bbbba8 b05852923a85880d85a6fe6769241e0a529200b61829b1aeeb5b584233d1fbff
   depths: 4 4
   └─Ordinary   l: 011   bits: 8   refs: 2   data: 01
     hashes: 6b663d94d562c0682bd6a8be41c639b6e52f87c20ef5776795e4ad7fdbcf0461 69b89defd3511d324ba0045d5e367328767d9e7879eefee2997eb6d324f7bb26 61d3e69357372022d099e3b2926bb7692d1cc0461912a2ca3e57a7a880dc2f6d
     depths: 3 3 3
     ├─Pruned branch   l: 011   bits: 560   refs: 0   data: 010348a9bccff3f4284647e46cef7422ab53f73e51f96ca1b61e56a7dbd70f57f91b9cc5685a89369c2eab2d5d1ae7d5075539dda819dfa60559869d3b1eee53d40000010000
     │ hashes: 48a9bccff3f4284647e46cef7422ab53f73e51f96ca1b61e56a7dbd70f57f91b 9cc5685a89369c2eab2d5d1ae7d5075539dda819dfa60559869d3b1eee53d400 75886b76cecf4242e0fb1579905b1b68e9e8d4c5802f8799910cfac9a66c8363
     │ depths: 1 0 0
     └─Ordinary   l: 010   bits: 24   refs: 2   data: 060606
       hashes: 874b71a722fcf50bed19d9635177d6ca5482b560cdd62bed6c6ff8f1e61efe68 8b86e04f03cb3ac31dff013eef8c39d9e8c729bb0f73deff96f34115110751d0
       depths: 2 2
       ├─Ordinary   l: 010   bits: 24   refs: 1   data: 030303
       │ hashes: 093fed1748edf1bbb14e25dbbae22e8015c021831ccdc9e2427e145e24aac8c1 cba811a64f95abacf23fa29a98d99399d849ff7c8522cbb2d140f917988c1ae2
       │ depths: 1 1
       │ └─Pruned branch   l: 010   bits: 288   refs: 0   data: 010278b55d6113eba6bc4ae107b4442afa416b6bc9709b3146657e358e68fa994c340000
       │   hashes: 78b55d6113eba6bc4ae107b4442afa416b6bc9709b3146657e358e68fa994c34 99cca996c8108a2288327990fe93e19e5c8c1d80c942645f1b5f8d9a967b8a1f
       │   depths: 0 0
       └─Pruned branch   l: 010   bits: 288   refs: 0   data: 0102b82404f6e84b041b25e452b30da10f01437dfd34efbba8b5772e4bc427df01df0001
         hashes: b82404f6e84b041b25e452b30da10f01437dfd34efbba8b5772e4bc427df01df 76e25eb9c2e42ba849c900812b41b31bd3fa9f7588468f75a7c3d8fa3b766dce
         depths: 1 0");
}
