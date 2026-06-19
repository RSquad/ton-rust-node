/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Utility functions and types for Simplex consensus
//!
//! This module contains:
//! - Threshold calculation helpers (2/3, 1/3)
//! - Session-scoped signature verification and creation
//! - Candidate ID hash computation (separate functions for empty and non-empty blocks)
//! - Vote TL conversion and signature handling
//! - Hex dump utilities for trace logging
//!
//! ## Signature Scheme
//!
//! All signatures in Simplex consensus are session-scoped. The data to sign is wrapped:
//!
//! ```text
//! data_to_sign = consensus.dataToSign(session_id, actual_data)
//! signature = Ed25519.sign(private_key, serialize(data_to_sign))
//! ```
//!
//! This ensures signatures cannot be replayed across different sessions.
//!
//! ## Candidate ID Hash Computation
//!
//! Different TL types are used for hash computation based on block type:
//!
//! - **Non-empty blocks**: `compute_candidate_id_hash()` uses `candidateHashDataOrdinary`
//! - **Empty blocks**: `compute_candidate_id_hash_empty()` uses `candidateHashDataEmpty`
//!
//! This matches C++ reference implementation (`consensus-types.cpp`).
//!

use crate::{PrivateKey, PublicKey, SessionId, ValidatorWeight};
use std::{
    any::Any,
    backtrace::Backtrace,
    cmp::max,
    panic,
    sync::{Arc, Once},
    thread,
    time::Duration,
};
use ton_api::{
    ton::{
        consensus::{
            blocksyncoverlayid::BlockSyncOverlayId,
            candidatehashdata::{CandidateHashDataEmpty, CandidateHashDataOrdinary},
            candidateid::CandidateId,
            candidateparent::CandidateParent,
            datatosign::DataToSign,
            simplex::vote::Vote as SimplexVote,
            CandidateParent as CandidateParentBoxed,
        },
        pub_::publickey::Overlay,
        validator_session::Candidate,
    },
    IntoBoxed,
};
use ton_block::{
    error, fail, read_boc, sha256_digest, Block, BlockIdExt, ConsensusExtraData, Deserializable,
    KeyId, Result, ShardIdent, UInt256,
};

/*
    Thread panic reporting (SXMAIN / SXCB / SXRCV)
*/

const INSTALL_PANIC_HOOK_ONCE: bool = true;

static SIMPLEX_PANIC_HOOK_ONCE: Once = Once::new();

/// Convert a panic payload to a readable string (best-effort).
pub(crate) fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Install a global panic hook that logs panics from Simplex threads as **FATAL**.
///
/// This is used together with `catch_unwind` so that:
/// - we always get a structured log entry (with backtrace) for panics inside SX threads
/// - thread stop flags can still be set after unwinding is caught
pub(crate) fn install_simplex_panic_hook_once() {
    if !INSTALL_PANIC_HOOK_ONCE {
        return;
    }

    SIMPLEX_PANIC_HOOK_ONCE.call_once(|| {
        let prev = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let thread = thread::current();
            let thread_name = thread.name().unwrap_or("<unnamed>");
            let is_simplex_thread = thread_name.starts_with("SXMAIN:")
                || thread_name.starts_with("SXCB:")
                || thread_name.starts_with("SXRCV:");

            if is_simplex_thread {
                let payload = panic_payload_to_string(info.payload());
                let location = info
                    .location()
                    .map(|l| format!("{}:{}", l.file(), l.line()))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let bt = Backtrace::force_capture();

                log::error!(
                    "FATAL PANIC: thread={} location={} payload=\"{}\" backtrace={:?}",
                    thread_name,
                    location,
                    payload,
                    bt
                );
            }

            // Preserve the previous hook behavior (stderr output, etc).
            prev(info);
        }));
    });
}

/*
    Activity tracking constants
*/

/// Threshold for considering a validator active based on last activity time
/// Used by SimplexState::debug_dump and Receiver for active weight calculation
pub const ACTIVITY_THRESHOLD: Duration = Duration::from_secs(30);

/*
    Threshold calculation helpers

    C++ reference (ton-node-cpp-simplex) uses STRICT thresholds:
    - 2/3 quorum: (total_weight * 2) / 3 + 1  (equivalent to weight * 3 > total_weight * 2)
      - validator/consensus/simplex/pool.cpp
      - crypto/block/signature-set.cpp (check_threshold)
    - 1/3 quorum: total_weight / 3 + 1
      - validator-session/validator-session-types.h
*/

/// Calculate strict 2/3 threshold for quorum/certificates.
///
/// Matches C++: `(total_weight * 2) / 3 + 1` (i.e. **strictly greater** than 2/3).
pub fn threshold_66(total_weight: ValidatorWeight) -> ValidatorWeight {
    if total_weight == 0 {
        return 0;
    }
    (((total_weight as u128) * 2) / 3 + 1) as ValidatorWeight
}

/// Calculate strict 1/3 threshold for safety conditions.
///
/// Matches C++: `total_weight / 3 + 1` (i.e. **strictly greater** than 1/3).
pub fn threshold_33(total_weight: ValidatorWeight) -> ValidatorWeight {
    if total_weight == 0 {
        return 0;
    }
    ((total_weight as u128) / 3 + 1) as ValidatorWeight
}

/*
    Hex dump utilities for trace logging
*/

/// Format bytes as hex string for logging
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/*
    Session-scoped signature utilities
*/

/// Create the session-scoped data to sign wrapper
///
/// Wraps the data in a `consensus.dataToSign` TL object with the session ID.
pub fn create_data_to_sign(session_id: &SessionId, data: &[u8]) -> Vec<u8> {
    let data_to_sign = DataToSign { session_id: session_id.clone(), data: data.to_vec() };
    consensus_common::serialize_tl_boxed_object!(&data_to_sign.into_boxed())
}

/// Verify a session-scoped signature
///
/// # Arguments
///
/// * `session_id` - The session identifier
/// * `data` - The actual data that was signed (without session wrapper)
/// * `signature` - The signature to verify
/// * `public_key` - The public key to verify against
///
/// # Returns
///
/// `true` if the signature is valid, `false` otherwise
pub fn check_session_signature(
    session_id: &SessionId,
    data: &[u8],
    signature: &[u8],
    public_key: &PublicKey,
) -> bool {
    let data_to_sign = create_data_to_sign(session_id, data);
    public_key.verify(&data_to_sign, signature).is_ok()
}

/// Create a session-scoped signature
///
/// This is the signing counterpart to `check_session_signature`.
///
/// # Arguments
///
/// * `session_id` - The session identifier
/// * `data` - The actual data to sign (without session wrapper)
/// * `private_key` - The private key to sign with
///
/// # Returns
///
/// The signature bytes, or an error if signing fails
pub fn sign_with_session(
    session_id: &SessionId,
    data: &[u8],
    private_key: &PrivateKey,
) -> Result<Vec<u8>> {
    let data_to_sign = create_data_to_sign(session_id, data);
    let signature = private_key.sign(&data_to_sign)?;
    Ok(signature.to_vec())
}

/*
    Candidate signature utilities
*/

/// Create the candidate ID data to sign
///
/// Serializes slot and hash into a `consensus.candidateId` TL object.
/// This matches C++ testnet behavior which signs bare `candidateId` directly,
/// not wrapped in `candidateParent`.
///
/// # Arguments
///
/// * `slot` - The slot number
/// * `hash` - The candidate ID hash
///
/// # Returns
///
/// Serialized TL object representing the candidate ID for signing
pub fn create_candidate_id_to_sign(slot: SlotIndex, hash: &UInt256) -> Vec<u8> {
    let candidate_id = CandidateId { slot: slot.value() as i32, hash: hash.clone() };
    consensus_common::serialize_tl_boxed_object!(&candidate_id.into_boxed())
}

/// Verify a block candidate signature
///
/// Verifies that the signature was created by the leader for the given candidate.
/// The signature is over `consensus.candidateId(slot, candidate_hash)`
/// wrapped in `consensus.dataToSign(session_id, ...)`.
///
/// # Arguments
///
/// * `session_id` - The session identifier
/// * `slot` - The candidate slot
/// * `candidate_hash` - The candidate ID hash
/// * `signature` - The signature to verify
/// * `public_key` - The leader's public key
///
/// # Returns
///
/// `true` if the signature is valid, `false` otherwise
pub fn check_candidate_signature(
    session_id: &SessionId,
    slot: SlotIndex,
    candidate_hash: &UInt256,
    signature: &[u8],
    public_key: &PublicKey,
) -> bool {
    let id_to_sign = create_candidate_id_to_sign(slot, candidate_hash);
    check_session_signature(session_id, &id_to_sign, signature, public_key)
}

/// Sign a block candidate
///
/// # Arguments
///
/// * `session_id` - The session identifier
/// * `slot` - The candidate slot
/// * `candidate_hash` - The candidate ID hash
/// * `private_key` - The private key to sign with
///
/// # Returns
///
/// The signature bytes, or an error if signing fails
pub fn sign_candidate(
    session_id: &SessionId,
    slot: SlotIndex,
    candidate_hash: &UInt256,
    private_key: &PrivateKey,
) -> Result<Vec<u8>> {
    let id_to_sign = create_candidate_id_to_sign(slot, candidate_hash);
    sign_with_session(session_id, &id_to_sign, private_key)
}

/*
    Candidate ID hash computation

    Computes SHA256 hash of serialized consensus.candidateIdData TL object.
*/

/// Compute the candidate ID hash
///
/// # Arguments
///
/// * `slot` - The slot number
/// * `block_id` - Optional block ID (None for empty blocks)
/// * `collated_file_hash` - Hash of collated data (zero if no block)
/// * `parent` - Optional parent candidate info (slot, hash)
///
/// # Returns
///
/// The computed candidate ID hash
pub fn compute_candidate_id_hash(
    _slot: SlotIndex, // Slot is passed for context but not used in CandidateHashData
    block_id: Option<&BlockIdExt>,
    collated_file_hash: Option<&UInt256>,
    parent: Option<(SlotIndex, &UInt256)>,
) -> UInt256 {
    let serialized = build_candidate_hash_data_bytes(block_id, collated_file_hash, parent);
    let hash = sha256_digest(&serialized);
    UInt256::from_slice(&hash)
}

/// Build the serialized CandidateHashData TL bytes
///
/// This function creates the TL-serialized representation of the candidate hash data
/// that is used both for computing the candidate_id_hash (via SHA256) and for
/// storing in BlockSignaturesSimplex for signature verification.
///
/// # Arguments
///
/// * `block_id` - Optional block ID (None for empty blocks)
/// * `collated_file_hash` - Hash of collated data (zero if no block)
/// * `parent` - Optional parent candidate info (slot, hash)
///
/// # Returns
///
/// The serialized CandidateHashData TL bytes
pub fn build_candidate_hash_data_bytes(
    block_id: Option<&BlockIdExt>,
    collated_file_hash: Option<&UInt256>,
    parent: Option<(SlotIndex, &UInt256)>,
) -> Vec<u8> {
    // Create parent TL object
    let parent_tl = match parent {
        Some((parent_slot, parent_hash)) => {
            // Create CandidateId for parent (TL uses i32 for slot)
            let parent_id =
                CandidateId { slot: parent_slot.value() as i32, hash: parent_hash.clone() };
            CandidateParent { id: parent_id.into_boxed() }.into_boxed()
        }
        None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
    };

    // Create block ID TL object
    let block_id_tl = match block_id {
        Some(id) => BlockIdExt {
            shard_id: id.shard_id.clone(),
            seq_no: id.seq_no,
            root_hash: id.root_hash.clone(),
            file_hash: id.file_hash.clone(),
        },
        None => BlockIdExt::default(),
    };

    // Create collated file hash
    let collated_hash = collated_file_hash.cloned().unwrap_or_default();

    // Create candidateHashDataOrdinary TL object
    // Reference: C++ RawCandidateId::create computes hash from CandidateHashData
    let candidate_hash_data = CandidateHashDataOrdinary {
        block: block_id_tl,
        collated_file_hash: collated_hash,
        parent: parent_tl,
    };

    // Serialize to TL bytes
    consensus_common::serialize_tl_boxed_object!(&candidate_hash_data.into_boxed())
}

/// Compute the candidate ID hash for empty blocks
///
/// Uses `candidateHashDataEmpty` TL type which has a different parent representation
/// than `candidateHashDataOrdinary`. The parent is `CandidateId` directly, NOT wrapped
/// in `CandidateParent`.
///
/// Reference: C++ `CandidateId::create_hash_data()` in `consensus-types.cpp`
///
/// # Arguments
///
/// * `referenced_block` - The inherited BlockIdExt from parent
/// * `parent` - Parent candidate info (slot, hash) - REQUIRED for empty blocks
///
/// # Returns
///
/// The computed candidate ID hash
pub fn compute_candidate_id_hash_empty(
    referenced_block: &BlockIdExt,
    parent: (SlotIndex, &UInt256),
) -> UInt256 {
    let serialized = build_candidate_hash_data_bytes_empty(referenced_block, parent);
    let hash = sha256_digest(&serialized);
    UInt256::from_slice(&hash)
}

/// Build the serialized CandidateHashData TL bytes for empty blocks
///
/// Uses `candidateHashDataEmpty` TL type. The parent is `CandidateId` directly,
/// NOT wrapped in `CandidateParent`.
///
/// # Arguments
///
/// * `referenced_block` - The inherited BlockIdExt from parent
/// * `parent` - Parent candidate info (slot, hash) - REQUIRED for empty blocks
///
/// # Returns
///
/// The serialized CandidateHashData TL bytes
pub fn build_candidate_hash_data_bytes_empty(
    referenced_block: &BlockIdExt,
    parent: (SlotIndex, &UInt256),
) -> Vec<u8> {
    // For empty blocks, parent is CandidateId directly (not wrapped in CandidateParent)
    // TL uses i32 for slot
    let parent_id = CandidateId { slot: parent.0.value() as i32, hash: parent.1.clone() };

    // Create candidateHashDataEmpty TL object
    // Reference: C++ creates consensus_candidateHashDataEmpty with direct CandidateId parent
    // Note: TL schema uses lowercase `consensus.candidateId` (the inner struct, not boxed enum)
    let candidate_hash_data =
        CandidateHashDataEmpty { block: referenced_block.clone(), parent: parent_id };

    // Serialize to TL bytes
    consensus_common::serialize_tl_boxed_object!(&candidate_hash_data.into_boxed())
}

/*
    Block candidate extraction from consensus.candidate bytes

    Extracts BlockIdExt and collated_file_hash from validatorSession.candidate bytes.
*/

/// Extract `gen_utime_ms` from collated data if it carries `ConsensusExtraData`.
///
/// Returns `None` for empty / invalid BOCs or when the collated data does not
/// carry Simplex `ConsensusExtraData`.
pub fn extract_consensus_gen_utime_ms(collated_data: &[u8]) -> Option<u64> {
    let roots = read_boc(collated_data).ok()?.roots;
    roots.into_iter().find_map(|root| {
        ConsensusExtraData::construct_from_cell(root).ok().map(|extra| extra.gen_utime_ms)
    })
}

/// Block info extracted from candidate bytes
#[derive(Clone, Debug)]
pub struct ExtractedBlockInfo {
    /// Block ID with shard, seqno, root_hash, file_hash
    pub block_id: BlockIdExt,
    /// Hash of collated data
    pub collated_file_hash: UInt256,
    /// Block data bytes
    pub data: Vec<u8>,
    /// Collated data bytes
    pub collated_data: Vec<u8>,
}

/// Extract block info from consensus.candidate bytes
///
/// The candidate bytes contain a serialized validatorSession_candidate (or compressed variant).
/// This function deserializes it and computes the necessary hashes.
///
/// Matches C++ reference: `RawCandidate::deserialize()` in `consensus-types.cpp`
///
/// # Arguments
///
/// * `candidate_bytes` - The candidate field from consensus.candidate
/// * `shard` - Shard identifier for this consensus session
/// * `max_size` - Maximum allowed decompressed size
///
/// # Returns
///
/// Extracted block info including data and collated_data, or None if candidate_bytes is empty (empty block)
pub fn extract_block_info_from_candidate(
    candidate_bytes: &[u8],
    shard: &ShardIdent,
    max_size: usize,
    proto_version: u32,
) -> Result<Option<ExtractedBlockInfo>> {
    // Empty candidate means empty block
    if candidate_bytes.is_empty() {
        return Ok(None);
    }

    // Deserialize the inner validatorSession_candidate
    let candidate_vec = candidate_bytes.to_vec();
    let inner_candidate =
        consensus_common::utils::deserialize_tl_boxed_object::<Candidate>(&candidate_vec)?;

    // Extract fields based on variant (compressed or uncompressed)
    let (round, root_hash, data, collated_data) = match &inner_candidate {
        Candidate::ValidatorSession_Candidate(c) => {
            // Validate src is zeros (C++ protocol requirement)
            if c.src != UInt256::default() {
                fail!("src field of the candidate broadcast must be null")
            }
            (c.round, c.root_hash.clone(), c.data.to_vec(), c.collated_data.to_vec())
        }
        Candidate::ValidatorSession_CompressedCandidate(c) => {
            // Validate src is zeros (C++ protocol requirement)
            if c.src != UInt256::default() {
                fail!("src field of the candidate broadcast must be null")
            }
            // Check decompressed size limit
            if c.decompressed_size as usize > max_size {
                fail!(
                    "Decompressed candidate size {} exceeds limit {max_size}",
                    c.decompressed_size
                )
            }

            // C++ simplex always uses mode 2 (CRC32 only) for collated data
            // re-serialization, regardless of proto_version. The proto_version >= 5
            // gate in decompress_candidate_data selects mode 2; lower versions select
            // mode 31.
            let effective_proto = max(proto_version, 5);
            let (block_data, collated_data) =
                consensus_common::compression::decompress_candidate_data(
                    &c.data,
                    false,
                    c.decompressed_size as usize,
                    effective_proto,
                )?;

            (c.round, c.root_hash.clone(), block_data, collated_data)
        }
        #[allow(unreachable_patterns)]
        _ => {
            fail!("Unknown validatorSession.Candidate variant")
        }
    };

    // Check size limits
    if data.len() + collated_data.len() > max_size {
        fail!(
            "Candidate data size {} + collated {} exceeds limit {}",
            data.len(),
            collated_data.len(),
            max_size
        )
    }

    // Compute file_hash = sha256(data)
    let file_hash = UInt256::from_slice(&sha256_digest(&data));

    // Compute collated_file_hash = sha256(collated_data)
    let collated_file_hash = UInt256::from_slice(&sha256_digest(&collated_data));

    // Build BlockIdExt
    let block_id = BlockIdExt {
        shard_id: shard.clone(),
        seq_no: round as u32,
        root_hash: UInt256::from_slice(root_hash.as_slice()),
        file_hash,
    };

    Ok(Some(ExtractedBlockInfo { block_id, collated_file_hash, data, collated_data }))
}

/// Compute candidate ID hash from consensus.candidate message
///
/// This is a convenience function that extracts block info and computes the hash.
///
/// # Arguments
///
/// * `slot` - The slot number from consensus.candidate
/// * `candidate_bytes` - The candidate field from consensus.candidate
/// * `parent` - Optional parent info (slot, hash)
/// * `shard` - Shard identifier for this consensus session
/// * `max_size` - Maximum allowed candidate size
///
/// # Returns
///
/// The computed candidate ID hash
pub fn compute_candidate_id_hash_from_bytes(
    slot: SlotIndex,
    candidate_bytes: &[u8],
    parent: Option<(SlotIndex, &UInt256)>,
    shard: &ShardIdent,
    max_size: usize,
    proto_version: u32,
) -> Result<UInt256> {
    let block_info =
        extract_block_info_from_candidate(candidate_bytes, shard, max_size, proto_version)?;

    let hash = match block_info {
        Some(info) => compute_candidate_id_hash(
            slot,
            Some(&info.block_id),
            Some(&info.collated_file_hash),
            parent,
        ),
        None => {
            // Empty block - use default BlockIdExt and zero collated hash
            compute_candidate_id_hash(slot, None, None, parent)
        }
    };

    Ok(hash)
}

/*
    Public API wrappers for external tests

    These functions accept u32 slot parameters for use by external tests
    that cannot access internal SlotIndex type.
*/

/// Compute candidate ID hash (public API with u32 slot)
///
/// This is a convenience wrapper for external tests that cannot access
/// the internal SlotIndex type.
///
/// # Arguments
///
/// * `slot` - The slot number (as u32)
/// * `block_id` - Optional block ID (None for empty blocks)
/// * `collated_file_hash` - Hash of collated data (None if no block)
/// * `parent` - Optional parent candidate info (slot as u32, hash)
///
/// # Returns
///
/// The computed candidate ID hash
pub fn compute_candidate_id_hash_u32(
    slot: u32,
    block_id: Option<&BlockIdExt>,
    collated_file_hash: Option<&UInt256>,
    parent: Option<(u32, &UInt256)>,
) -> UInt256 {
    compute_candidate_id_hash(
        SlotIndex(slot),
        block_id,
        collated_file_hash,
        parent.map(|(s, h)| (SlotIndex(s), h)),
    )
}

/// Sign a block candidate (public API with u32 slot)
///
/// This is a convenience wrapper for external tests that cannot access
/// the internal SlotIndex type.
pub fn sign_candidate_u32(
    session_id: &SessionId,
    slot: u32,
    candidate_hash: &UInt256,
    private_key: &PrivateKey,
) -> Result<Vec<u8>> {
    sign_candidate(session_id, SlotIndex(slot), candidate_hash, private_key)
}

/*
    Vote TL Serialization

    Conversion between FSM Vote types and TL simplexConsensus.* types.

    FSM types (simplex_state.rs):
    - Vote::Notarize(NotarizeVote)
    - Vote::Finalize(FinalizeVote)
    - Vote::Skip(SkipVote)
    TL types (ton_api::ton::simplex_consensus::*):
    - UnsignedVote::SimplexConsensus_NotarizeVote
    - UnsignedVote::SimplexConsensus_FinalizeVote
    - UnsignedVote::SimplexConsensus_SkipVote

    Wire format: consensus.simplex.vote { vote, signature }
*/

use crate::{
    block::SlotIndex,
    simplex_state::{FinalizeVote, NotarizeVote, SkipVote, Vote},
};
use ton_api::ton::consensus::simplex::{self as tl_simplex, unsignedvote as tl_unsigned};

/// Convert FSM Vote to TL UnsignedVote
///
/// Used when creating votes for broadcast.
///
/// # C++ Protocol (3 vote types on wire)
///
/// The TL protocol only defines 3 vote types:
/// - NotarizeVote { id: CandidateId }
/// - FinalizeVote { id: CandidateId }
/// - SkipVote { slot: int }
///
/// # Returns
///
/// Serialized TL vote for wire-compatible vote types.
pub fn vote_to_tl_unsigned(vote: &Vote) -> Result<tl_simplex::UnsignedVote> {
    match vote {
        Vote::Notarize(v) => {
            // Create CandidateId from slot and candidate hash (block_hash)
            // Note: block_hash is the candidate hash, NOT the block's root_hash
            let candidate_id =
                CandidateId { slot: v.slot.value() as i32, hash: v.block_hash.clone() };
            Ok(tl_unsigned::NotarizeVote { id: candidate_id.into_boxed() }.into_boxed())
        }

        Vote::Finalize(v) => {
            // Note: block_hash is the candidate hash, NOT the block's root_hash
            let candidate_id =
                CandidateId { slot: v.slot.value() as i32, hash: v.block_hash.clone() };
            Ok(tl_unsigned::FinalizeVote { id: candidate_id.into_boxed() }.into_boxed())
        }

        Vote::Skip(v) => Ok(tl_unsigned::SkipVote { slot: v.slot.value() as i32 }.into_boxed()),
    }
}

/// Convert TL UnsignedVote to FSM Vote
///
/// Used when processing incoming votes from the network.
///
/// Note: C++ protocol only sends 3 vote types (Notarize, Finalize, Skip).
///
/// # Errors
///
/// Returns error if the TL vote has invalid slot numbers (negative).
pub fn tl_unsigned_to_vote(tl: &tl_simplex::UnsignedVote) -> Result<Vote> {
    match tl {
        tl_simplex::UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
            let slot = *v.id.slot();
            if slot < 0 {
                fail!("Invalid slot number: {}", slot)
            }
            // Extract candidate hash from CandidateId
            // Note: This is the candidate hash (computed from CandidateHashData),
            // NOT the block's root_hash
            let block_hash = UInt256::from_slice(v.id.hash().as_slice());
            Ok(Vote::Notarize(NotarizeVote { slot: SlotIndex(slot as u32), block_hash }))
        }

        tl_simplex::UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
            let slot = *v.id.slot();
            if slot < 0 {
                fail!("Invalid slot number: {}", slot)
            }
            // Extract candidate hash from CandidateId
            let block_hash = UInt256::from_slice(v.id.hash().as_slice());
            Ok(Vote::Finalize(FinalizeVote { slot: SlotIndex(slot as u32), block_hash }))
        }

        tl_simplex::UnsignedVote::Consensus_Simplex_SkipVote(v) => {
            let slot = v.slot;
            if slot < 0 {
                fail!("Invalid slot number: {}", slot)
            }
            Ok(Vote::Skip(SkipVote { slot: SlotIndex(slot as u32) }))
        }
    }
}

/// Serialize unsigned vote to bytes for signing
///
/// Uses TL boxed serialization.
pub fn serialize_unsigned_vote(vote: &tl_simplex::UnsignedVote) -> Vec<u8> {
    consensus_common::serialize_tl_boxed_object!(vote)
}

/// Sign a vote with session-scoped signature
///
/// Creates a signed vote ready for network broadcast:
/// 1. Convert FSM vote to TL UnsignedVote
/// 2. Serialize the unsigned vote
/// 3. Wrap in consensus.dataToSign(session_id, vote_data)
/// 4. Sign the wrapped data
/// 5. Wrap in consensus.simplex.vote
///
/// # C++ Reference (pool.cpp)
///
/// ```cpp
/// auto vote_to_sign = serialize_tl_object(vote.to_tl(), true);
/// auto data_to_sign = create_serialize_tl_object<consensus::tl::dataToSign>(bus.session_id, std::move(vote_to_sign));
/// auto signature = co_await td::actor::ask(bus.keyring, &keyring::Keyring::sign_message, ...);
/// ```
///
/// # Returns
///
/// Signed vote for network broadcast.
pub fn sign_vote(
    vote: &Vote,
    session_id: &SessionId,
    private_key: &PrivateKey,
) -> Result<tl_simplex::Vote> {
    // Convert to TL unsigned vote.
    let unsigned_vote = vote_to_tl_unsigned(vote)?;

    // Serialize for signing (boxed, as in C++)
    let vote_data = serialize_unsigned_vote(&unsigned_vote);

    // Wrap in consensus.dataToSign(session_id, vote_data) - matches C++ pool.cpp
    let data_to_sign = create_data_to_sign(session_id, &vote_data);

    // Sign the session-wrapped data
    let signature = private_key.sign(&data_to_sign)?;

    // Create signed vote
    let signed_vote = SimplexVote { vote: unsigned_vote, signature: signature.to_vec() };

    Ok(signed_vote.into_boxed())
}

/// Verify a signed vote's signature
///
/// Verify a signed vote's signature
///
/// # C++ Reference (types.cpp lines 25-28)
///
/// ```cpp
/// bool PeerValidator::check_signature(ValidatorSessionId session, td::Slice data, td::Slice signature) const {
///   auto signed_data = create_serialize_tl_object<tl::dataToSign>(session, td::BufferSlice(data));
///   return key.create_encryptor().move_as_ok()->check_signature(signed_data, signature).is_ok();
/// }
/// ```
///
/// The vote data is wrapped in consensus.dataToSign(session_id, vote_data) before verification.
///
/// # Returns
///
/// `true` if the signature is valid for the given public key.
pub fn verify_vote_signature(
    tl_vote: &tl_simplex::Vote,
    session_id: &SessionId,
    public_key: &PublicKey,
) -> bool {
    let tl_simplex::Vote::Consensus_Simplex_Vote(ref inner) = tl_vote;

    // Serialize the unsigned vote
    let vote_data = serialize_unsigned_vote(&inner.vote);

    // Wrap in consensus.dataToSign(session_id, vote_data) - matches C++ types.cpp
    let data_to_sign = create_data_to_sign(session_id, &vote_data);

    // Verify the session-wrapped data
    public_key.verify(&data_to_sign, &inner.signature).is_ok()
}

/// Extract the FSM vote from a signed TL vote
///
/// Does NOT verify the signature - use `verify_vote_signature` first.
pub fn extract_vote(tl_vote: &tl_simplex::Vote) -> Result<Vote> {
    let tl_simplex::Vote::Consensus_Simplex_Vote(ref inner) = tl_vote;
    tl_unsigned_to_vote(&inner.vote)
}

/// Extract the FSM vote AND signature from a signed TL vote
///
/// Returns (vote, signature_bytes).
///
/// Does NOT verify the signature - use `verify_vote_signature` first.
pub fn extract_vote_and_signature(tl_vote: &tl_simplex::Vote) -> Result<(Vote, Vec<u8>)> {
    let tl_simplex::Vote::Consensus_Simplex_Vote(ref inner) = tl_vote;
    let vote = tl_unsigned_to_vote(&inner.vote)?;
    let signature = inner.signature.to_vec();
    Ok((vote, signature))
}

/// Get the slot number from an unsigned vote
///
/// Returns the primary slot.
pub fn get_vote_slot(vote: &tl_simplex::UnsignedVote) -> i32 {
    match vote {
        tl_simplex::UnsignedVote::Consensus_Simplex_NotarizeVote(v) => *v.id.slot(),
        tl_simplex::UnsignedVote::Consensus_Simplex_FinalizeVote(v) => *v.id.slot(),
        tl_simplex::UnsignedVote::Consensus_Simplex_SkipVote(v) => v.slot,
    }
}

/*
    Block Info Extraction (before_split support)
*/

/// Extract before_split flag from block payload
///
/// Parses the block payload to read BlockInfo and extract the before_split flag.
/// This is used to implement C++ parity for empty block generation during shard split/merge.
///
/// # Arguments
///
/// * `block_data` - Block payload bytes (serialized Block structure)
///
/// # Returns
///
/// * `Ok(bool)` - before_split flag value
/// * `Err(...)` - Failed to parse block (empty payload, parse error, etc.)
///
/// # Reference
///
/// C++ utils.cpp `get_before_split()`:
/// ```cpp
/// td::Result<bool> get_before_split(const td::Ref<BlockData>& block) {
///   block::gen::Block::Record blk;
///   block::gen::BlockInfo::Record info;
///   if (!(tlb::unpack_cell(block->root_cell(), blk) && tlb::unpack_cell(blk.info, info))) {
///     return td::Status::Error("cannot unpack block header");
///   }
///   return info.before_split;
/// }
/// ```
///
pub fn extract_before_split_flag(block_data: &[u8]) -> Result<bool> {
    if block_data.is_empty() {
        // Empty payload (likely an empty block candidate) - return false
        return Ok(false);
    }

    // Deserialize Block from payload bytes
    let block = Block::construct_from_bytes(block_data)
        .map_err(|e| error!("Failed to deserialize block: {}", e))?;

    // Extract BlockInfo
    let block_info = block.read_info().map_err(|e| error!("Failed to read BlockInfo: {}", e))?;

    // Return before_split flag
    Ok(block_info.before_split())
}

/// Compute the block-sync overlay short id from a session id.
///
/// Mirrors C++ `block-sync-overlay.cpp:48-50`:
///   overlay_seed = serialize(consensus.blockSyncOverlayId{ session_id })
///   overlay_full_id = OverlayIdFull{ overlay_seed }
///   overlay_short_id = overlay_full_id.compute_short_id()
///
/// The block-sync seed does NOT include the validator-set node list, so its
/// short id differs from the consensus overlay's short id even for the same
/// `session_id`. Verified byte-equal with C++ via the
/// `test_block_sync_overlay_id_matches_cpp` compat test
pub fn compute_block_sync_overlay_short_id(session_id: &SessionId) -> Result<Arc<KeyId>> {
    let overlay_seed = BlockSyncOverlayId { session_id: session_id.clone() };
    let serialized = consensus_common::serialize_tl_boxed_object!(&overlay_seed.into_boxed());
    let overlay_pubkey = Overlay { name: serialized }.into_boxed();
    Ok(KeyId::from_data(adnl::common::hash_boxed(&overlay_pubkey)?))
}
