/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for simplex signature helpers and top-shard-descr promotion rules.

use super::can_promote_to_top_shard_descr;
use crate::validating_utils::{build_checked_data, simplex_to_sign};
use ton_api::{IntoBoxed, Serializer};
use ton_block::{
    Block, BlockIdExt, BlockSignatures, BlockSignaturesPure, BlockSignaturesSimplex,
    BlockSignaturesVariant, UInt256, ValidatorBaseInfo,
};

#[test]
fn test_simplex_to_sign_structure() {
    // TL constructor IDs
    const DATA_TO_SIGN: u32 = 0xa8e33df8;

    let session_id = UInt256::rand();
    let candidate_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let slot: u32 = 42;

    // Test finalize vote
    let bs_final = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        session_id.clone(),
        slot,
        BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap(),
    );

    let to_sign = simplex_to_sign(&bs_final).unwrap();

    // Check DATA_TO_SIGN constructor at start
    let constructor = u32::from_le_bytes(to_sign[0..4].try_into().unwrap());
    assert_eq!(constructor, DATA_TO_SIGN);

    // Check session_id follows constructor (32 bytes)
    assert_eq!(&to_sign[4..36], session_id.as_slice());

    // Test notarize vote
    let bs_notar = BlockSignaturesSimplex::new_notarize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        session_id.clone(),
        slot,
        BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap(),
    );

    let to_sign_notar = simplex_to_sign(&bs_notar).unwrap();

    // Finalize and notarize should produce different outputs
    // (different vote constructor IDs)
    assert_ne!(to_sign, to_sign_notar);

    // But same length (structure is identical)
    assert_eq!(to_sign.len(), to_sign_notar.len());

    // Same session_id in both
    assert_eq!(&to_sign[4..36], &to_sign_notar[4..36]);
}

#[test]
fn test_build_checked_data_ordinary() {
    let block_id = BlockIdExt::default();

    let ordinary = BlockSignaturesVariant::Ordinary(BlockSignatures::with_params(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
    ));

    let checked_data = build_checked_data(&ordinary, &block_id).unwrap();

    // Should use Block::build_data_for_sign
    let expected = Block::build_data_for_sign(&block_id.root_hash, &block_id.file_hash);
    assert_eq!(checked_data, expected.to_vec());
}

#[test]
fn test_build_checked_data_simplex() {
    let block_id = BlockIdExt::default();
    let session_id = UInt256::rand();
    // Candidate data must be TL-serialized consensus.CandidateHashData (C++ requirement).
    let parent_candidate_id =
        ton_api::ton::consensus::candidateid::CandidateId { slot: 0, hash: UInt256::rand() };
    let candidate_hash_data = ton_api::ton::consensus::candidatehashdata::CandidateHashDataEmpty {
        block: block_id.clone(),
        parent: parent_candidate_id,
    };
    let mut candidate_data = Vec::new();
    Serializer::new(&mut candidate_data).write_boxed(&candidate_hash_data.into_boxed()).unwrap();

    let simplex = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        session_id.clone(),
        10,
        BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap(),
    );

    let variant = BlockSignaturesVariant::Simplex(simplex.clone());

    let checked_data = build_checked_data(&variant, &block_id).unwrap();

    // Should use simplex_to_sign
    let expected = simplex_to_sign(&simplex).unwrap();
    assert_eq!(checked_data, expected);
}

fn make_simplex_variant(is_final: bool) -> BlockSignaturesVariant {
    let session_id = UInt256::rand();
    let candidate_data = BlockSignaturesSimplex::bytes_to_cell_tree(&[0xFE, 0xED]).unwrap();
    let validator_info = ValidatorBaseInfo::with_params(1, 2);
    let signatures = BlockSignaturesPure::with_weight(100);

    let simplex = if is_final {
        BlockSignaturesSimplex::new_finalize(
            validator_info,
            signatures,
            session_id,
            10,
            candidate_data,
        )
    } else {
        BlockSignaturesSimplex::new_notarize(
            validator_info,
            signatures,
            session_id,
            10,
            candidate_data,
        )
    };

    BlockSignaturesVariant::Simplex(simplex)
}

#[test]
fn test_top_shard_descr_promotion_allows_ordinary_signatures() {
    let ordinary = BlockSignaturesVariant::Ordinary(BlockSignatures::with_params(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
    ));

    assert!(can_promote_to_top_shard_descr(&ordinary));
}

#[test]
fn test_top_shard_descr_promotion_allows_final_simplex_signatures() {
    assert!(can_promote_to_top_shard_descr(&make_simplex_variant(true)));
}

#[test]
fn test_top_shard_descr_promotion_rejects_notarized_simplex_signatures() {
    assert!(!can_promote_to_top_shard_descr(&make_simplex_variant(false)));
}
