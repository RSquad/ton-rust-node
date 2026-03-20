/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use catchain::{serialize_tl_boxed_object, BlockPayloadPtr};
use consensus_common::compression::{compress_candidate_data, decompress_candidate_data};
use ton_api::{ton::validator_session::candidate, IntoBoxed};
use ton_block::fail;

pub(crate) fn serialize_block_candidate(
    candidate: candidate::Candidate,
    compression_enabled: bool,
) -> catchain::Result<BlockPayloadPtr> {
    if !compression_enabled {
        return Ok(catchain::CatchainFactory::create_block_payload(serialize_tl_boxed_object!(
            &candidate.into_boxed()
        )));
    }

    let (compressed, decompressed_size) =
        compress_candidate_data(&candidate.data, &candidate.collated_data)?;

    let compressed_candidate = ton_api::ton::validator_session::candidate::CompressedCandidate {
        src: candidate.src,
        round: candidate.round,
        root_hash: candidate.root_hash,
        data: compressed,
        decompressed_size: decompressed_size as i32,
    }
    .into_boxed();

    return Ok(catchain::CatchainFactory::create_block_payload(serialize_tl_boxed_object!(
        &compressed_candidate
    )));
}

pub(crate) fn deserialize_block_candidate(
    data: BlockPayloadPtr,
    _compression_enabled: bool,
    max_decompressed_data_size: u32,
    proto_version: u32,
) -> catchain::Result<crate::ton::Candidate> {
    let candidate = catchain::utils::deserialize_tl_boxed_object::<
        ton_api::ton::validator_session::Candidate,
    >(data.data())?;

    match &candidate {
        ton_api::ton::validator_session::Candidate::ValidatorSession_CompressedCandidate(
            compressed_candidate,
        ) => {
            if compressed_candidate.decompressed_size as u32 > max_decompressed_data_size {
                fail!("decompressed size is too big");
            }

            let (block, collated_data) = decompress_candidate_data(
                &compressed_candidate.data,
                false,
                compressed_candidate.decompressed_size as usize,
                proto_version,
            )?;

            Ok(ton_api::ton::validator_session::candidate::Candidate {
                src: compressed_candidate.src.clone(),
                round: compressed_candidate.round,
                root_hash: compressed_candidate.root_hash.clone(),
                data: block,
                collated_data,
            }
            .into_boxed())
        }
        ton_api::ton::validator_session::Candidate::ValidatorSession_CompressedCandidateV2(
            compressed_candidate,
        ) => {
            let (block, collated_data) = decompress_candidate_data(
                &compressed_candidate.data,
                true,
                max_decompressed_data_size as usize,
                proto_version,
            )?;

            Ok(ton_api::ton::validator_session::candidate::Candidate {
                src: compressed_candidate.src.clone(),
                round: compressed_candidate.round,
                root_hash: compressed_candidate.root_hash.clone(),
                data: block,
                collated_data: collated_data,
            }
            .into_boxed())
        }
        ton_api::ton::validator_session::Candidate::ValidatorSession_Candidate(_candidate) => {
            Ok(candidate)
        }
        #[allow(unreachable_patterns)]
        _ => fail!(
            "expected CompressedCandidate, CompressedCandidateV2 or Candidate, got another variant"
        ),
    }
}
