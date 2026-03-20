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
use ton_block::{read_single_root_boc, Block, BlockIdExt, BlockProof, MerkleProof, ShardIdent};

#[test]
fn test_block_stuff_deserialize() {
    // block	(-1,8000000000000000,2429446)
    // roothash	A3A94C6D84B310D35A15A8ACC731EF3E04661C871200C2488952AC892A5543E8
    // filehash	F2BC2888EAE466FC172140A90949EDBC7091D81CCB4194A225C56C0F1D095097

    let name =
        "src/tests/static/F2BC2888EAE466FC172140A90949EDBC7091D81CCB4194A225C56C0F1D095097.boc";
    let bs = BlockStuff::read_block_from_file(name).unwrap();
    let id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 2429446,
        root_hash: "A3A94C6D84B310D35A15A8ACC731EF3E04661C871200C2488952AC892A5543E8"
            .parse()
            .unwrap(),
        file_hash: "F2BC2888EAE466FC172140A90949EDBC7091D81CCB4194A225C56C0F1D095097"
            .parse()
            .unwrap(),
    };
    assert_eq!(bs.id(), &id);
}

#[test]
fn test_proof_from_proof() {
    let proof_data =
        std::fs::read("src/tests/static/test_master_block_proof_shuffle/proof__3236531").unwrap();
    let proof_root = read_single_root_boc(&proof_data).unwrap();
    let proof = BlockProof::construct_from_cell(proof_root.clone()).unwrap();
    let original_proof = MerkleProof::construct_from_cell(proof.root.clone()).unwrap();
    let block_virt_root = original_proof.proof.clone().virtualize(1);

    let usage_tree = UsageTree::with_params(block_virt_root.clone(), true);

    let block = Block::construct_from_cell(usage_tree.root_cell()).unwrap();
    let info = block.read_info().unwrap();
    let _prev = info.read_prev_ref().unwrap();
    let _vprev = info.read_prev_vert_ref().unwrap();
    let _mc_ref = info.read_master_ref().unwrap();
    let _su = block.read_state_update().unwrap();
    let new_proof = MerkleProof::create_by_usage_tree(&block_virt_root, &usage_tree).unwrap();

    let _cell = proof.serialize().unwrap();

    assert_eq!(new_proof.hash, new_proof.proof.hash(0));
    assert_eq!(new_proof.proof.hash(0), original_proof.proof.hash(0));
}

#[test]
fn test_construct_and_check_prev_stuff_master() {
    let proof_data =
        std::fs::read("src/tests/static/test_master_block_proof_shuffle/proof__3236531").unwrap();
    let proof_root = read_single_root_boc(&proof_data).unwrap();
    let proof = BlockProof::construct_from_cell(proof_root.clone()).unwrap();
    let merkle_proof = MerkleProof::construct_from_cell(proof.root.clone()).unwrap();
    let block_virt_root = merkle_proof.proof.clone().virtualize(1);

    let (id, stuff) =
        construct_and_check_prev_stuff(&block_virt_root, &proof.proof_for, true).unwrap();

    let virt_block = Block::construct_from_cell(block_virt_root.clone()).unwrap();
    let virt_block_info = virt_block.read_info().unwrap();
    let prev = virt_block_info.read_prev_ids().unwrap();
    let prev = &prev[0];

    let mc_block_id = BlockIdExt {
        shard_id: virt_block_info.shard().clone(),
        seq_no: prev.seq_no,
        root_hash: prev.root_hash.clone(),
        file_hash: prev.file_hash.clone(),
    };

    let mut id2 = proof.proof_for.clone();
    id2.file_hash = UInt256::default();

    assert_eq!(id, id2);
    assert_eq!(stuff.mc_block_id, mc_block_id);
    assert_eq!(stuff.prev[0], mc_block_id);
    assert_eq!(stuff.prev.len(), 1);
    assert!(!stuff._after_split);
}

#[test]
fn test_construct_and_check_prev_stuff_shard() {
    let proof_data =
        std::fs::read("src/tests/static/test_shard_block_proof/proof_4377262").unwrap();
    let proof_root = read_single_root_boc(&proof_data).unwrap();
    let proof = BlockProof::construct_from_cell(proof_root.clone()).unwrap();
    let merkle_proof = MerkleProof::construct_from_cell(proof.root.clone()).unwrap();
    let block_virt_root = merkle_proof.proof.clone().virtualize(1);

    let (id, stuff) =
        construct_and_check_prev_stuff(&block_virt_root, &proof.proof_for, true).unwrap();

    let virt_block = Block::construct_from_cell(block_virt_root.clone()).unwrap();
    let virt_block_info = virt_block.read_info().unwrap();

    let prev = virt_block_info.read_prev_ids().unwrap();
    let prev = &prev[0];
    let prev_block_id = BlockIdExt {
        shard_id: virt_block_info.shard().clone(),
        seq_no: prev.seq_no,
        root_hash: prev.root_hash.clone(),
        file_hash: prev.file_hash.clone(),
    };

    let mc_block_id = virt_block_info.read_master_ref().unwrap().unwrap().master;
    let mc_block_id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: mc_block_id.seq_no,
        root_hash: mc_block_id.root_hash.clone(),
        file_hash: mc_block_id.file_hash.clone(),
    };

    let mut id2 = proof.proof_for.clone();
    id2.file_hash = UInt256::default();

    assert_eq!(id, id2);
    assert_eq!(stuff.mc_block_id, mc_block_id);
    assert_eq!(stuff.prev[0], prev_block_id);
    assert_eq!(stuff.prev.len(), 1);
    assert!(!stuff._after_split);
}
