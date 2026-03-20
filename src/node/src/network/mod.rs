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
pub mod catchain_client;
pub mod control;
pub mod custom_overlay_client;
pub mod fast_sync_overlay_client;
pub mod full_node_overlay_client;
pub mod full_node_overlays;
pub mod full_node_service;
pub mod liteserver;
pub mod neighbours;
pub mod node_network;
pub mod overlay_client;
#[cfg(feature = "telemetry")]
pub mod telemetry;

use crate::{block::BlockStuff, block_proof::BlockProofStuff, engine_traits::EngineOperations};
use std::io::Cursor;
use ton_api::{
    serialize_boxed,
    ton::{
        ton_node::{
            block_broadcase_compressed::Data,
            blocksignature::BlockSignature,
            broadcast::{
                BlockBroadcast, BlockBroadcastCompressed, BlockBroadcastCompressedV2,
                NewBlockCandidateBroadcast, NewBlockCandidateBroadcastCompressed,
            },
            signature_set::signatureset::{Ordinary as TlOrdinary, Simplex as TlSimplex},
            SignatureSet,
        },
        Bool,
    },
    Deserializer, IntoBoxed, Serializer,
};
use ton_block::{
    boc_compression::{boc_compress, boc_decompress, CompressionAlgorithm},
    fail, lz4_compress, lz4_decompress, read_boc, write_boc, BlockIdExt, BlockSignatures,
    BlockSignaturesPure, BlockSignaturesSimplex, BlockSignaturesVariant, BocFlags, BocWriter, Cell,
    CryptoSignature, CryptoSignaturePair, Deserializable, HashmapType, Lz4DecompressMode, Result,
    UInt256, ValidatorBaseInfo,
};

pub const MAX_COMPRESSED_SIZE: usize = 16 << 20; // 16 MB

// This constant is used in a small number of places in the cpp code, despite its name.
// It may conflict with limits in the network configuration.
pub const MAX_BLOCK_SIZE: usize = 4 << 20; // 4 MB.

fn decompress_block_broadcast(broadcast: BlockBroadcastCompressed) -> Result<BlockBroadcast> {
    let data = lz4_decompress(
        &broadcast.compressed,
        Lz4DecompressMode::WithMaxSize(MAX_COMPRESSED_SIZE as i32),
    )?;
    let decompressed = Deserializer::new(&mut Cursor::new(data)).read_boxed::<Data>()?.only();

    let mut roots = read_boc(&decompressed.proof_data)?.roots;
    if roots.len() != 2 {
        fail!("Invalid BlockBroadcastCompressed: expected 2 roots, got {}", roots.len());
    }
    let proof = write_boc(&roots.remove(0))?;
    let mut block = Vec::new();
    BocWriter::with_flags(roots, BocFlags::all())?.write(&mut block)?;

    Ok(BlockBroadcast {
        id: broadcast.id,
        catchain_seqno: broadcast.catchain_seqno,
        validator_set_hash: broadcast.validator_set_hash,
        signatures: decompressed.signatures,
        proof,
        data: block,
    })
}

/// Decompressed V2 block broadcast data.
///
/// Contains signatures as `BlockSignaturesVariant` which supports both
/// ordinary (catchain) and simplex signature schemes.
pub struct BlockBroadcastV2 {
    pub id: BlockIdExt,
    pub signatures: BlockSignaturesVariant,
    pub proof: Vec<u8>,
    pub data: Vec<u8>,
}

/// Decompress V2 block broadcast, returning normalized `BlockBroadcastV2`.
///
/// Unlike V1 where signatures + proof + data are compressed together, V2:
/// - Proof is stored uncompressed
/// - Only block data is compressed
/// - Uses SignatureSet for both ordinary and simplex signatures
///
/// This matches C++ `deserialize_block_broadcast(tonNode_blockBroadcastCompressedV2&)`
/// in `full-node-serializer.cpp`.
pub(crate) fn decompress_block_broadcast_v2(
    broadcast: BlockBroadcastCompressedV2,
) -> Result<BlockBroadcastV2> {
    // Decompress only block data (proof is already uncompressed in V2)
    let roots = boc_decompress(&broadcast.data_compressed, MAX_COMPRESSED_SIZE)?;
    if roots.len() != 1 {
        fail!("Invalid BlockBroadcastCompressedV2: expected 1 root, got {}", roots.len());
    }
    let data = BocWriter::with_flags(roots, BocFlags::all())?.write_to_vec()?;

    // Convert SignatureSet to BlockSignaturesVariant
    let signatures = unpack_signature_set(broadcast.signature_set)?;

    Ok(BlockBroadcastV2 { id: broadcast.id, signatures, proof: broadcast.proof, data })
}

fn pack_block_signatures(signatures: &BlockSignatures) -> Result<Vec<BlockSignature>> {
    let mut packed_signatures = vec![];
    signatures.pure_signatures.signatures().iterate_slices(|ref mut _key, ref mut slice| {
        let sign = CryptoSignaturePair::construct_from(slice)?;
        packed_signatures.push(BlockSignature {
            who: UInt256::with_array(*sign.node_id_short.as_slice()),
            signature: sign.sign.as_bytes().to_vec(),
        });
        Ok(true)
    })?;
    Ok(packed_signatures)
}

/// Pack pure signatures into TL BlockSignature vector
fn pack_pure_signatures(pure_signatures: &BlockSignaturesPure) -> Result<Vec<BlockSignature>> {
    let mut packed = vec![];
    pure_signatures.signatures().iterate_slices(|ref mut _key, ref mut slice| {
        let sign = CryptoSignaturePair::construct_from(slice)?;
        packed.push(BlockSignature {
            who: UInt256::with_array(*sign.node_id_short.as_slice()),
            signature: sign.sign.as_bytes().to_vec(),
        });
        Ok(true)
    })?;
    Ok(packed)
}

/// Unpack TL BlockSignature vector into BlockSignaturesPure
fn unpack_pure_signatures(signatures: &[BlockSignature]) -> Result<BlockSignaturesPure> {
    let mut pure = BlockSignaturesPure::default();
    for sig in signatures {
        let crypto_sig = CryptoSignature::from_bytes(&sig.signature)?;
        pure.add_sigpair(CryptoSignaturePair::with_params(sig.who.clone(), crypto_sig));
    }
    Ok(pure)
}

/// Pack BlockSignaturesVariant into TL SignatureSet for V2 broadcast
pub(crate) fn pack_signature_set(signatures: &BlockSignaturesVariant) -> Result<SignatureSet> {
    match signatures {
        BlockSignaturesVariant::Ordinary(sigs) => {
            let packed = pack_block_signatures(sigs)?;
            Ok(SignatureSet::TonNode_SignatureSet_Ordinary(TlOrdinary {
                cc_seqno: sigs.validator_info.catchain_seqno as i32,
                validator_set_hash: sigs.validator_info.validator_list_hash_short as i32,
                signatures: packed,
            }))
        }
        BlockSignaturesVariant::Simplex(sigs) => {
            let packed = pack_pure_signatures(&sigs.pure_signatures)?;
            // Serialize candidate_data Cell to bytes for TL
            let candidate_bytes = sigs.candidate_data_bytes()?;
            // Parse the candidate_data bytes back to TL CandidateHashData
            let candidate = Deserializer::new(&mut Cursor::new(&candidate_bytes))
                .read_boxed::<ton_api::ton::consensus::CandidateHashData>()?;
            Ok(SignatureSet::TonNode_SignatureSet_Simplex(TlSimplex {
                final_: if sigs.is_final { Bool::BoolTrue } else { Bool::BoolFalse },
                cc_seqno: sigs.validator_info.catchain_seqno as i32,
                validator_set_hash: sigs.validator_info.validator_list_hash_short as i32,
                signatures: packed,
                session_id: sigs.session_id.clone(),
                slot: sigs.slot as i32,
                candidate,
            }))
        }
    }
}

/// Unpack TL SignatureSet into BlockSignaturesVariant
pub(crate) fn unpack_signature_set(sig_set: SignatureSet) -> Result<BlockSignaturesVariant> {
    match sig_set {
        SignatureSet::TonNode_SignatureSet_Ordinary(sigs) => {
            let validator_info = ValidatorBaseInfo::with_params(
                sigs.validator_set_hash as u32,
                sigs.cc_seqno as u32,
            );
            let pure_signatures = unpack_pure_signatures(&sigs.signatures)?;
            Ok(BlockSignaturesVariant::Ordinary(BlockSignatures::with_params(
                validator_info,
                pure_signatures,
            )))
        }
        SignatureSet::TonNode_SignatureSet_Simplex(sigs) => {
            let validator_info = ValidatorBaseInfo::with_params(
                sigs.validator_set_hash as u32,
                sigs.cc_seqno as u32,
            );
            let pure_signatures = unpack_pure_signatures(&sigs.signatures)?;
            let is_final = matches!(sigs.final_, Bool::BoolTrue);
            // Serialize TL candidate back to bytes for BlockSignaturesSimplex
            let candidate =
                BlockSignaturesSimplex::bytes_to_cell_tree(&serialize_boxed(&sigs.candidate)?)?;
            let simplex_sigs = BlockSignaturesSimplex::with_params(
                validator_info,
                pure_signatures,
                sigs.session_id.clone(),
                sigs.slot as u32,
                candidate,
                is_final,
            );
            Ok(BlockSignaturesVariant::Simplex(simplex_sigs))
        }
    }
}

fn build_block_broadcast(
    block: &BlockStuff,
    proof: &BlockProofStuff,
    catchain_seqno: u32,
    signatures: Vec<BlockSignature>,
    validator_set_hash: u32,
) -> BlockBroadcast {
    BlockBroadcast {
        id: block.id().clone(),
        catchain_seqno: catchain_seqno as i32,
        validator_set_hash: validator_set_hash as i32,
        signatures,
        proof: proof.data().to_vec(),
        data: block.data().to_vec(),
    }
}

fn build_block_broadcast_compressed(
    block: &BlockStuff,
    proof: &BlockProofStuff,
    catchain_seqno: u32,
    signatures: Vec<BlockSignature>,
    validator_set_hash: u32,
) -> Result<BlockBroadcastCompressed> {
    let mut boc = Vec::new();
    BocWriter::with_roots([proof.root_cell().clone(), block.root_cell().clone()])?
        .write(&mut boc)?;

    let data = ton_api::ton::ton_node::block_broadcase_compressed::ton_node::block_broadcast_compressed::data::Data {
        signatures,
        proof_data: boc,
    }.into_boxed();

    let mut bytes = Vec::new();
    Serializer::new(&mut bytes).write_boxed(&data)?;
    let compressed = lz4_compress(bytes, false)?;

    Ok(BlockBroadcastCompressed {
        id: block.id().clone(),
        catchain_seqno: catchain_seqno as i32,
        validator_set_hash: validator_set_hash as i32,
        compressed,
    })
}

/// Build V2 block broadcast with SignatureSet support (both ordinary and simplex).
///
/// Unlike V1 which compresses signatures + proof + data together, V2:
/// - Stores proof uncompressed
/// - Only compresses block data
/// - Uses SignatureSet for signature type flexibility
///
/// This matches C++ `serialize_block_broadcast_v2` in `full-node-serializer.cpp`.
pub(crate) fn build_block_broadcast_compressed_v2(
    block: &BlockStuff,
    proof: &BlockProofStuff,
    signatures: &BlockSignaturesVariant,
) -> Result<BlockBroadcastCompressedV2> {
    let signature_set = pack_signature_set(signatures)?;

    // Compress only block data (proof stays uncompressed in V2).
    //
    // Match C++ `serialize_block_broadcast_v2`:
    // - parse block data BOC into a root cell
    // - compress using `ImprovedStructureLZ4` (`vm::boc_compress`)
    // - store the compressed BOC (with algorithm byte prefix)
    let data_compressed =
        boc_compress(vec![block.root_cell().clone()], CompressionAlgorithm::ImprovedStructureLZ4)?;

    Ok(BlockBroadcastCompressedV2 {
        id: block.id().clone(),
        signature_set,
        proof: proof.data().to_vec(),
        data_compressed,
    })
}

pub(crate) fn check_sync_for_listen_bcasts(engine: &dyn EngineOperations) -> bool {
    if let Ok(Some(id)) = engine.load_last_applied_mc_block_id() {
        if let Ok(Some(handle)) = engine.load_block_handle(&id) {
            return handle.gen_utime() + 20 * 60 > engine.now();
        }
    }
    false
}

#[allow(dead_code)]
fn build_block_candidate_broadcast_compressed(
    id: BlockIdExt,
    cc_seqno: u32,
    validator_set_hash: u32,
    block_root: &Cell,
) -> Result<NewBlockCandidateBroadcastCompressed> {
    let mut boc = Vec::new();
    BocWriter::with_flags([block_root.clone()], BocFlags::Crc32)?.write(&mut boc)?;

    let compressed = lz4_compress(boc, false)?;

    Ok(NewBlockCandidateBroadcastCompressed {
        id,
        catchain_seqno: cc_seqno as i32,
        validator_set_hash: validator_set_hash as i32,
        // In original cpp code it is zero too
        collator_signature: BlockSignature { who: UInt256::ZERO, signature: vec![] },
        compressed,
    })
}

fn decompress_and_check_candidate_data(
    broadcast: &NewBlockCandidateBroadcastCompressed,
) -> Result<Vec<u8>> {
    if broadcast.compressed.len() > MAX_COMPRESSED_SIZE {
        fail!("Invalid BlockCandidateBroadcastCompressed: compressed size exceeds limit");
    }

    let decompressed = lz4_decompress(
        &broadcast.compressed,
        Lz4DecompressMode::WithMaxSize(MAX_COMPRESSED_SIZE as i32),
    )?;
    if decompressed.len() > MAX_BLOCK_SIZE {
        fail!("Invalid BlockCandidateBroadcastCompressed: decompressed size exceeds limit");
    }

    let mut roots = read_boc(&decompressed)?.roots;
    if roots.len() != 1 {
        fail!("Invalid BlockCandidateBroadcastCompressed: expected 1 root, got {}", roots.len());
    }
    let block_root = roots.remove(0);
    if *broadcast.id.root_hash() != block_root.repr_hash() {
        fail!("Invalid BlockCandidateBroadcastCompressed: root hash mismatch");
    }

    let mut canonical_boc = Vec::new();
    BocWriter::with_flags([block_root], BocFlags::all())?.write(&mut canonical_boc)?;
    Ok(canonical_boc)
}

fn check_block_candidate_data(broadcast: &NewBlockCandidateBroadcast) -> Result<()> {
    if broadcast.data.len() > MAX_BLOCK_SIZE {
        fail!("Invalid BlockCandidateBroadcast: data size exceeds limit");
    }
    Ok(())
}
