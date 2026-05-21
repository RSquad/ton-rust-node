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
    network::{
        build_block_broadcast_compressed, build_block_broadcast_compressed_v2,
        decompress_block_broadcast, decompress_block_broadcast_v2,
    },
    test_helper::create_network,
};
use adnl::{common::TaggedTlObject, node::IpAddress};
use ton_api::{
    ton::{
        adnl::Pong as AdnlPongBoxed, rpc::adnl::Ping as AdnlPing,
        ton_node::blocksignature::BlockSignature,
    },
    AnyBoxedSerialize,
};
use ton_block::{
    BlockSignatures, BlockSignaturesPure, BlockSignaturesSimplex, BlockSignaturesVariant,
    CryptoSignature, CryptoSignaturePair, Ed25519KeyOption, UInt256, ValidatorBaseInfo,
    ZeroizingBytes,
};

#[test]
fn test_calc_new_shards() {
    let result = calc_new_shards(0, 1).unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x4000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xc000_0000_0000_0000).unwrap()
    ));

    let result = calc_new_shards(0, 2).unwrap();
    assert_eq!(result.len(), 6);
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x4000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xc000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x2000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x6000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xa000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xe000_0000_0000_0000).unwrap()
    ));

    let result = calc_new_shards(1, 2).unwrap();
    assert_eq!(result.len(), 4);
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x2000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x6000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xa000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xe000_0000_0000_0000).unwrap()
    ));

    let result = calc_new_shards(0, 3).unwrap();
    assert_eq!(result.len(), 14);
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x4000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xc000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x2000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x6000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xa000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xe000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x1000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x3000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x5000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x7000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x9000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xb000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xd000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xf000_0000_0000_0000).unwrap()
    ));

    let result = calc_new_shards(2, 3).unwrap();
    assert_eq!(result.len(), 8);
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x1000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x3000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x5000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x7000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0x9000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xb000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xd000_0000_0000_0000).unwrap()
    ));
    assert!(result.contains(
        &ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, 0xf000_0000_0000_0000).unwrap()
    ));

    let result = calc_new_shards(1, 1).unwrap();
    assert_eq!(result.len(), 0);

    let result = calc_new_shards(2, 1).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_trim_shard() {
    fn test(shard: u64, min_split: u8, expected: u64) {
        let shard = ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, shard).unwrap();
        let shard = trim_shard(&shard, min_split).unwrap();
        assert_eq!(shard, ShardIdent::with_tagged_prefix(BASE_WORKCHAIN_ID, expected).unwrap());
    }
    test(0x8000_0000_0000_0000, 0, 0x8000_0000_0000_0000);
    test(0x8000_0000_0000_0000, 1, 0x8000_0000_0000_0000);
    test(0x8000_0000_0000_0000, 2, 0x8000_0000_0000_0000);
    test(0x8000_0000_0000_0000, 3, 0x8000_0000_0000_0000);

    test(0x2000_0000_0000_0000, 0, 0x8000_0000_0000_0000);
    test(0x2000_0000_0000_0000, 1, 0x4000_0000_0000_0000);
    test(0x2000_0000_0000_0000, 2, 0x2000_0000_0000_0000);
    test(0x2000_0000_0000_0000, 3, 0x2000_0000_0000_0000);

    test(0xf000_0000_0000_0000, 0, 0x8000_0000_0000_0000);
    test(0xf000_0000_0000_0000, 1, 0xc000_0000_0000_0000);
    test(0xf000_0000_0000_0000, 2, 0xe000_0000_0000_0000);
    test(0xf000_0000_0000_0000, 3, 0xf000_0000_0000_0000);
    test(0xf000_0000_0000_0000, 4, 0xf000_0000_0000_0000);

    test(0x4600_0000_0000_0000, 0, 0x8000_0000_0000_0000);
    test(0x4600_0000_0000_0000, 1, 0x4000_0000_0000_0000);
    test(0x4600_0000_0000_0000, 2, 0x6000_0000_0000_0000);
    test(0x4600_0000_0000_0000, 3, 0x5000_0000_0000_0000);
    test(0x4600_0000_0000_0000, 4, 0x4800_0000_0000_0000);
    test(0x4600_0000_0000_0000, 5, 0x4400_0000_0000_0000);
    test(0x4600_0000_0000_0000, 6, 0x4600_0000_0000_0000);
    test(0x4600_0000_0000_0000, 7, 0x4600_0000_0000_0000);
    test(0x4600_0000_0000_0000, 8, 0x4600_0000_0000_0000);
}

#[test]
fn test_trim_shard_special() {
    assert_eq!(trim_shard(&ShardIdent::MASTERCHAIN, 0).unwrap(), ShardIdent::MASTERCHAIN);
    assert_eq!(trim_shard(&ShardIdent::MASTERCHAIN, 1).unwrap(), ShardIdent::MASTERCHAIN);
    assert_eq!(trim_shard(&ShardIdent::MASTERCHAIN, 5).unwrap(), ShardIdent::MASTERCHAIN);
    assert_eq!(
        trim_shard(&ShardIdent::with_tagged_prefix(12, 0xc000_0000_0000_0000).unwrap(), 0).unwrap(),
        ShardIdent::full(12)
    );
    assert_eq!(trim_shard(&ShardIdent::full(12), 0).unwrap(), ShardIdent::full(12));
    assert_eq!(
        trim_shard(&ShardIdent::with_tagged_prefix(12, 0xc000_0000_0000_0000).unwrap(), 3).unwrap(),
        ShardIdent::full(12)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_overlay_client() {
    const IP_NODE: &str = "127.0.0.1:4000";
    const KEY_TAG: usize = 2;
    const QUERY_ERROR: &str = "No reply to query \
        (TLObject tl_id:#1faaa1bf Ping { value: 1 } in 1 attempts";
    const SHARD: i64 = 0x8000000000000000u64 as i64;
    const WORKCHAIN: i32 = 0;
    let network = create_network(None, Some("test_overlay_client"), IP_NODE).await.unwrap();
    network.start().await.unwrap();
    let context = network.context().clone();
    let client = OverlayClient::new_public(
        context.stack.overlay.calc_overlay_short_id(WORKCHAIN, SHARD).unwrap(),
        context.stack.overlay.calc_overlay_id(WORKCHAIN, SHARD).unwrap(),
        context.clone(),
        network.cancellation_token().clone(),
        DhtSearchPolicy::default(),
        None,
    )
    .await
    .unwrap();
    let peer_key = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
    let this_key = context.stack.adnl.key_by_tag(KEY_TAG).unwrap();
    let peer = context
        .stack
        .adnl
        .add_peer(
            this_key.id(),
            &IpAddress::from_versioned_string("127.0.0.1:5000", None).unwrap(),
            None,
            &peer_key,
        )
        .unwrap()
        .unwrap();
    let neighbours = client.neighbours();
    assert!(neighbours.add(peer).unwrap());
    match client
        .send_adnl_query::<AdnlPongBoxed>(
            &TaggedTlObject {
                object: AdnlPing { value: 1 }.into_tl_object(),
                #[cfg(feature = "telemetry")]
                tag: 1,
            },
            Some(1),
            Some(10),
            None,
        )
        .await
    {
        Ok(_) => assert!(false),
        Err(e) => {
            println!("Error: {}", e);
            assert!(e.to_string().starts_with(QUERY_ERROR))
        }
    }
}

#[test]
fn test_block_broadcast_compression() {
    let block =
        BlockStuff::read_block_from_file("src/tests/static/test_master_block_proof/block__3082184")
            .unwrap();
    let proof = BlockProofStuff::read_from_file(
        block.id(),
        "src/tests/static/test_master_block_proof/proof__3082184",
        !block.id().shard().is_masterchain(),
    )
    .unwrap();
    let mut signatures = vec![];
    signatures.push(BlockSignature {
        who: UInt256::rand(),
        signature: (0..64).map(|_| rand::random::<u8>()).collect::<Vec<u8>>().try_into().unwrap(),
    });
    let cc_seqno = rand::random();
    let validator_set_hash = rand::random();

    let bcast_c = build_block_broadcast_compressed(
        &block,
        &proof,
        cc_seqno,
        signatures.clone(),
        validator_set_hash,
    )
    .unwrap();

    let bcast = decompress_block_broadcast(bcast_c).unwrap();

    let block2 =
        BlockStuff::deserialize_block_checked(bcast.id.clone(), Arc::new(bcast.data.clone()))
            .unwrap();
    assert_eq!(block, block2);
    let proof2 = BlockProofStuff::deserialize(
        block2.id(),
        bcast.proof,
        !block2.id().shard().is_masterchain(),
    )
    .unwrap();
    assert_eq!(proof.root_cell(), proof2.root_cell());
    assert_eq!(cc_seqno, bcast.catchain_seqno as u32);
    assert_eq!(validator_set_hash, bcast.validator_set_hash as u32);
    assert_eq!(signatures, bcast.signatures);
}

/// Helper to create test BlockSignaturesVariant::Ordinary
fn create_test_ordinary_signatures(
    cc_seqno: u32,
    validator_set_hash: u32,
) -> BlockSignaturesVariant {
    let validator_info = ValidatorBaseInfo::with_params(validator_set_hash, cc_seqno);
    let mut pure_sigs = BlockSignaturesPure::default();

    // Add a few test signatures
    for _ in 0..3 {
        let node_id = UInt256::rand();
        let sig_bytes: Vec<u8> = (0..64).map(|_| rand::random::<u8>()).collect();
        let sig = CryptoSignature::from_bytes(&sig_bytes).unwrap();
        pure_sigs.add_sigpair(CryptoSignaturePair::with_params(node_id, sig));
    }

    BlockSignaturesVariant::Ordinary(BlockSignatures::with_params(validator_info, pure_sigs))
}

/// Helper to create test BlockSignaturesVariant::Simplex
fn create_test_simplex_signatures(
    cc_seqno: u32,
    validator_set_hash: u32,
) -> BlockSignaturesVariant {
    use ton_api::IntoBoxed;

    let validator_info = ValidatorBaseInfo::with_params(validator_set_hash, cc_seqno);
    let mut pure_sigs = BlockSignaturesPure::default();

    // Add a few test signatures
    for _ in 0..3 {
        let node_id = UInt256::rand();
        let sig_bytes: Vec<u8> = (0..64).map(|_| rand::random::<u8>()).collect();
        let sig = CryptoSignature::from_bytes(&sig_bytes).unwrap();
        pure_sigs.add_sigpair(CryptoSignaturePair::with_params(node_id, sig));
    }

    let session_id = UInt256::rand();
    let slot = 42u32;

    // Create a proper TL CandidateHashDataEmpty using the TL library
    // TL: consensus.candidateId slot:int hash:int256 = consensus.CandidateId;
    // TL: consensus.candidateHashDataEmpty block:tonNode.blockIdExt parent:consensus.candidateId = consensus.CandidateHashData;
    let tl_block_id = ton_block::BlockIdExt::with_params(
        ton_block::ShardIdent::masterchain(),
        10,
        UInt256::rand(),
        UInt256::rand(),
    );
    let parent_candidate_id =
        ton_api::ton::consensus::candidateid::CandidateId { slot: 5, hash: UInt256::rand() };
    let candidate_hash_data = ton_api::ton::consensus::candidatehashdata::CandidateHashDataEmpty {
        block: tl_block_id,
        parent: parent_candidate_id,
    };

    // Serialize to TL bytes
    let candidate_data = BlockSignaturesSimplex::bytes_to_cell_tree(
        &serialize_boxed(&candidate_hash_data.into_boxed()).unwrap(),
    )
    .unwrap();
    let simplex_sigs = BlockSignaturesSimplex::with_params(
        validator_info,
        pure_sigs,
        session_id,
        slot,
        candidate_data,
        true, // is_final
    );

    BlockSignaturesVariant::Simplex(simplex_sigs)
}

#[test]
fn test_block_broadcast_compression_v2_ordinary() {
    // Test V2 broadcast with Ordinary signatures
    let block =
        BlockStuff::read_block_from_file("src/tests/static/test_master_block_proof/block__3082184")
            .unwrap();
    let proof = BlockProofStuff::read_from_file(
        block.id(),
        "src/tests/static/test_master_block_proof/proof__3082184",
        !block.id().shard().is_masterchain(),
    )
    .unwrap();

    let cc_seqno = rand::random::<u32>();
    let validator_set_hash = rand::random::<u32>();
    let signatures = create_test_ordinary_signatures(cc_seqno, validator_set_hash);

    let bcast_v2 = build_block_broadcast_compressed_v2(&block, &proof, &signatures).unwrap();
    let decompressed = decompress_block_broadcast_v2(bcast_v2).unwrap();

    // Verify block and proof roundtrip
    let block2 = BlockStuff::deserialize_block_checked(
        decompressed.id.clone(),
        Arc::new(decompressed.data.clone()),
    )
    .unwrap();
    assert_eq!(block, block2);

    let proof2 = BlockProofStuff::deserialize(
        block2.id(),
        decompressed.proof,
        !block2.id().shard().is_masterchain(),
    )
    .unwrap();
    assert_eq!(proof.root_cell(), proof2.root_cell());

    // Verify signatures type is preserved
    assert!(matches!(decompressed.signatures, BlockSignaturesVariant::Ordinary(_)));

    // Verify validator info is preserved
    let decompressed_info = decompressed.signatures.validator_info();
    assert_eq!(decompressed_info.catchain_seqno, cc_seqno);
    assert_eq!(decompressed_info.validator_list_hash_short, validator_set_hash);
}

#[test]
fn test_block_broadcast_compression_v2_simplex() {
    // Test V2 broadcast with Simplex signatures
    let block =
        BlockStuff::read_block_from_file("src/tests/static/test_master_block_proof/block__3082184")
            .unwrap();
    let proof = BlockProofStuff::read_from_file(
        block.id(),
        "src/tests/static/test_master_block_proof/proof__3082184",
        !block.id().shard().is_masterchain(),
    )
    .unwrap();

    let cc_seqno = rand::random::<u32>();
    let validator_set_hash = rand::random::<u32>();
    let signatures = create_test_simplex_signatures(cc_seqno, validator_set_hash);

    let bcast_v2 = build_block_broadcast_compressed_v2(&block, &proof, &signatures).unwrap();
    let decompressed = decompress_block_broadcast_v2(bcast_v2).unwrap();

    // Verify block and proof roundtrip
    let block2 = BlockStuff::deserialize_block_checked(
        decompressed.id.clone(),
        Arc::new(decompressed.data.clone()),
    )
    .unwrap();
    assert_eq!(block, block2);

    let proof2 = BlockProofStuff::deserialize(
        block2.id(),
        decompressed.proof,
        !block2.id().shard().is_masterchain(),
    )
    .unwrap();
    assert_eq!(proof.root_cell(), proof2.root_cell());

    // Verify signatures type is preserved
    assert!(matches!(decompressed.signatures, BlockSignaturesVariant::Simplex(_)));

    // Verify validator info is preserved
    let decompressed_info = decompressed.signatures.validator_info();
    assert_eq!(decompressed_info.catchain_seqno, cc_seqno);
    assert_eq!(decompressed_info.validator_list_hash_short, validator_set_hash);

    // Verify simplex-specific fields
    if let BlockSignaturesVariant::Simplex(ref simplex) = decompressed.signatures {
        assert_eq!(simplex.slot, 42);
        assert!(simplex.is_final);
    } else {
        panic!("Expected Simplex signatures");
    }
}
