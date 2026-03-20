/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Certificate types for Simplex consensus vote aggregation
//!
//! This module implements vote aggregation certificates matching the C++ reference
//! implementation (`certificate.h`, `certificate.cpp`).
//!
//! ## Type Hierarchy
//!
//! ```text
//! VoteSignature (single validator signature)
//! ├── validator_idx: ValidatorIndex    // Index into validator set (compact)
//! └── signature: Vec<u8>               // Ed25519 signature bytes
//!
//! Certificate<T> (aggregated signatures meeting threshold)
//! ├── vote: T                          // The vote being certified
//! └── signatures: Vec<VoteSignature>   // Aggregated signatures (>= 2/3 weight)
//! ```
//!
//! ## Type Aliases
//!
//! - `NotarCert = Certificate<NotarizeVote>` - Notarization certificate
//! - `FinalCert = Certificate<FinalizeVote>` - Finalization certificate
//! - `SkipCert = Certificate<SkipVote>` - Skip certificate
//!
//! ## Usage
//!
//! Certificates are created when vote weight reaches 2/3 threshold.
//! They are serialized to TL for:
//! - Block finalization notifications (`BlockFinalized` event)
//! - Candidate resolver responses (`candidateAndCert.notar`)
//!
//! ## TL Schema
//!
//! ```tl
//! consensus.simplex.voteSignature who:int signature:bytes = consensus.simplex.VoteSignature;
//! consensus.simplex.voteSignatureSet votes:(vector consensus.simplex.VoteSignature) = consensus.simplex.VoteSignatureSet;
//! consensus.simplex.certificate vote:consensus.simplex.UnsignedVote signatures:consensus.simplex.VoteSignatureSet = consensus.simplex.Certificate;
//! ```
//!
//! ## C++ Reference
//!
//! - `certificate.h`: `Certificate<T>` template struct
//! - `certificate.cpp`: TL serialization, signature verification

use crate::{
    block::{SlotIndex, ValidatorIndex},
    session_description::SessionDescription,
    simplex_state::{FinalizeVote, NotarizeVote, SkipVote, Vote},
    utils::{serialize_unsigned_vote, vote_to_tl_unsigned},
    SessionId,
};
use std::sync::Arc;
use ton_api::{
    deserialize_typed,
    ton::consensus::simplex::{
        certificate::Certificate as TlCertificate, votesignature::VoteSignature as TlVoteSignature,
        votesignatureset::VoteSignatureSet, Certificate as CertificateBoxed, UnsignedVote,
        VoteSignature as VoteSignatureBoxed, VoteSignatureSet as VoteSignatureSetBoxed,
    },
    IntoBoxed,
};
use ton_block::{error, fail, Result, UInt256};

/*
    ============================================================================
    Vote Signature
    ============================================================================

    Single vote signature from a validator.
    Reference: C++ `Certificate<T>::VoteSignature` in `certificate.h`
*/

/// Single vote signature from a validator
///
/// Reference: C++ `Certificate<T>::VoteSignature` in `certificate.h`
///
/// Uses validator **index** (not public key) for compact serialization.
/// Validator public key is looked up from SessionDescription when verifying.
///
/// # TL Schema
///
/// ```tl
/// consensus.simplex.voteSignature who:int signature:bytes = consensus.simplex.VoteSignature;
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VoteSignature {
    /// Validator index (position in validator set)
    ///
    /// Must be valid for the session's validator set (0 <= idx < num_validators).
    pub validator_idx: ValidatorIndex,

    /// Ed25519 signature bytes over session-scoped vote
    ///
    /// Signature is computed over: `session_id || serialize(UnsignedVote)`
    pub signature: Vec<u8>,
}

impl VoteSignature {
    /// Create a new VoteSignature
    #[inline]
    pub fn new(validator_idx: ValidatorIndex, signature: Vec<u8>) -> Self {
        Self { validator_idx, signature }
    }

    /// Create from TL object
    ///
    /// # Arguments
    ///
    /// * `tl` - TL VoteSignature object
    ///
    /// # Returns
    ///
    /// VoteSignature with validator index and signature bytes
    pub fn from_tl(tl: &VoteSignatureBoxed) -> Self {
        Self {
            validator_idx: ValidatorIndex::new(*tl.who() as u32),
            signature: tl.signature().clone(),
        }
    }

    /// Convert to TL object
    ///
    /// # Returns
    ///
    /// Boxed TL VoteSignature object
    pub fn to_tl(&self) -> VoteSignatureBoxed {
        TlVoteSignature {
            who: self.validator_idx.value() as i32,
            signature: self.signature.clone(),
        }
        .into_boxed()
    }
}

/*
    ============================================================================
    Certificate
    ============================================================================

    Aggregated vote signatures meeting 2/3 threshold.
    Reference: C++ `Certificate<T>` template struct in `certificate.h`
*/

/// Certificate of aggregated vote signatures
///
/// Reference: C++ `Certificate<T>` template struct in `certificate.h`
///
/// A certificate proves that >= 2/3 of validators (by weight) have voted
/// for the same thing. Certificates are the basis for consensus finality.
///
/// # Type Parameters
///
/// - `T` - The vote type being certified (NotarizeVote, FinalizeVote, or SkipVote)
///
/// # Invariants
///
/// - `signatures` must have total weight >= 2/3 of total validator weight
/// - No duplicate validators in `signatures`
/// - All signatures must be valid for the vote
///
/// # TL Schema
///
/// ```tl
/// consensus.simplex.certificate vote:consensus.simplex.UnsignedVote
///     signatures:consensus.simplex.VoteSignatureSet = consensus.simplex.Certificate;
/// ```
#[derive(Clone, Debug)]
pub(crate) struct Certificate<T> {
    /// The vote being certified
    #[allow(dead_code)] // Used in to_tl() for full serialization
    pub vote: T,

    /// Aggregated signatures (must have >= 2/3 weight)
    pub signatures: Vec<VoteSignature>,
}

/// Type alias for notarization certificate
///
/// Proves >= 2/3 voted to notarize a specific block
pub(crate) type NotarCert = Certificate<NotarizeVote>;

/// Type alias for finalization certificate
///
/// Proves >= 2/3 voted to finalize a specific block
pub(crate) type FinalCert = Certificate<FinalizeVote>;

/// Type alias for skip certificate
///
/// Proves >= 2/3 voted to skip a specific slot
pub(crate) type SkipCert = Certificate<SkipVote>;

/// Pointer type for NotarCert
#[allow(dead_code)]
pub(crate) type NotarCertPtr = Arc<NotarCert>;

/// Pointer type for FinalCert
#[allow(dead_code)]
pub(crate) type FinalCertPtr = Arc<FinalCert>;

/// Pointer type for SkipCert
#[allow(dead_code)]
pub(crate) type SkipCertPtr = Arc<SkipCert>;

impl NotarCert {
    /// Deserialize NotarCert from TL VoteSignatureSet bytes.
    ///
    /// This creates a NotarCert from raw bytes WITHOUT verification.
    /// Used only for recovery where the certificate was already verified
    /// when originally stored.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw TL-serialized VoteSignatureSet bytes (boxed)
    ///
    /// # Returns
    ///
    /// NotarCert with signatures extracted from the TL bytes
    pub fn from_tl_bytes_for_candidate(
        bytes: &[u8],
        slot: SlotIndex,
        block_hash: UInt256,
    ) -> Result<Self> {
        let tl_boxed: VoteSignatureSetBoxed = deserialize_typed(bytes)?;
        let signatures: Vec<VoteSignature> =
            tl_boxed.votes().iter().map(VoteSignature::from_tl).collect();

        Ok(Self::new(NotarizeVote { slot, block_hash }, signatures))
    }
}

impl<T: Clone> Certificate<T> {
    /// Create a new Certificate
    ///
    /// # Arguments
    ///
    /// * `vote` - The vote being certified
    /// * `signatures` - Vector of vote signatures
    ///
    /// # Note
    ///
    /// This does NOT verify signatures or weight threshold.
    /// Use `from_tl()` for verified construction.
    #[inline]
    pub fn new(vote: T, signatures: Vec<VoteSignature>) -> Self {
        Self { vote, signatures }
    }

    /// Get total weight of signatures
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description for validator weight lookup
    ///
    /// # Returns
    ///
    /// Sum of weights for all validators in signatures
    pub fn total_weight(&self, desc: &SessionDescription) -> u64 {
        self.signatures.iter().map(|sig| desc.get_node_weight(sig.validator_idx)).sum()
    }

    /// Check if certificate has sufficient weight (>= 2/3)
    ///
    /// # Arguments
    ///
    /// * `desc` - Session description for weight calculation
    ///
    /// # Returns
    ///
    /// true if total signature weight >= 2/3 threshold
    #[allow(dead_code)] // Available for validation in future
    pub fn has_sufficient_weight(&self, desc: &SessionDescription) -> bool {
        self.total_weight(desc) >= desc.get_threshold_66()
    }

    /// Convert signatures to TL VoteSignatureSet
    ///
    /// Reference: C++ `Certificate<T>::to_tl_vote_signature_set()`
    ///
    /// Used by candidate resolver to send just signatures (not full certificate).
    ///
    /// # Returns
    ///
    /// Boxed TL VoteSignatureSet object
    pub fn to_tl_vote_signature_set(&self) -> VoteSignatureSetBoxed {
        let tl_sigs: Vec<VoteSignatureBoxed> =
            self.signatures.iter().map(|sig| sig.to_tl()).collect();
        VoteSignatureSet { votes: tl_sigs.into() }.into_boxed()
    }
}

/*
    ============================================================================
    Vote Type Trait for TL Conversion
    ============================================================================
*/

/// Trait for converting FSM vote types to TL
pub(crate) trait ToTlUnsignedVote: Clone {
    /// Convert to TL UnsignedVote
    fn to_tl_unsigned(&self) -> Result<UnsignedVote>;

    /// Convert to FSM Vote enum
    #[allow(dead_code)]
    fn to_vote(&self) -> Vote;
}

impl ToTlUnsignedVote for NotarizeVote {
    fn to_tl_unsigned(&self) -> Result<UnsignedVote> {
        vote_to_tl_unsigned(&Vote::Notarize(self.clone()))
    }

    fn to_vote(&self) -> Vote {
        Vote::Notarize(self.clone())
    }
}

impl ToTlUnsignedVote for FinalizeVote {
    fn to_tl_unsigned(&self) -> Result<UnsignedVote> {
        vote_to_tl_unsigned(&Vote::Finalize(self.clone()))
    }

    fn to_vote(&self) -> Vote {
        Vote::Finalize(self.clone())
    }
}

impl ToTlUnsignedVote for SkipVote {
    fn to_tl_unsigned(&self) -> Result<UnsignedVote> {
        vote_to_tl_unsigned(&Vote::Skip(self.clone()))
    }

    fn to_vote(&self) -> Vote {
        Vote::Skip(self.clone())
    }
}

impl ToTlUnsignedVote for Vote {
    fn to_tl_unsigned(&self) -> Result<UnsignedVote> {
        vote_to_tl_unsigned(self)
    }

    fn to_vote(&self) -> Vote {
        self.clone()
    }
}

/*
    ============================================================================
    Vote Type Trait for TL Verification
    ============================================================================

    This trait enables generic certificate verification from TL.
    Each vote type knows how to:
    - Extract slot and optional block_hash from TL
    - Verify the TL vote matches expected values
    - Create itself from expected values
*/

/// Trait for verifying vote types from TL during certificate verification
#[allow(dead_code)] // Infrastructure for full TL verification
pub(crate) trait VerifiableVote: ToTlUnsignedVote {
    /// Extract slot and optional block hash from TL unsigned vote
    ///
    /// Returns None if the TL vote is not the expected type.
    fn extract_from_tl(tl_vote: &UnsignedVote) -> Option<(SlotIndex, Option<UInt256>)>;

    /// Create vote from expected slot and optional block hash
    fn create(slot: SlotIndex, block_hash: Option<&UInt256>) -> Result<Self>;

    /// Vote type name for error messages
    fn vote_type_name() -> &'static str;
}

impl VerifiableVote for NotarizeVote {
    fn extract_from_tl(tl_vote: &UnsignedVote) -> Option<(SlotIndex, Option<UInt256>)> {
        match tl_vote {
            UnsignedVote::Consensus_Simplex_NotarizeVote(v) => {
                Some((SlotIndex::new(*v.id.slot() as u32), Some(v.id.hash().clone())))
            }
            _ => None,
        }
    }

    fn create(slot: SlotIndex, block_hash: Option<&UInt256>) -> Result<Self> {
        let hash = block_hash.ok_or_else(|| error!("NotarizeVote requires block_hash"))?;
        Ok(NotarizeVote { slot, block_hash: hash.clone() })
    }

    fn vote_type_name() -> &'static str {
        "notarize"
    }
}

impl VerifiableVote for FinalizeVote {
    fn extract_from_tl(tl_vote: &UnsignedVote) -> Option<(SlotIndex, Option<UInt256>)> {
        match tl_vote {
            UnsignedVote::Consensus_Simplex_FinalizeVote(v) => {
                Some((SlotIndex::new(*v.id.slot() as u32), Some(v.id.hash().clone())))
            }
            _ => None,
        }
    }

    fn create(slot: SlotIndex, block_hash: Option<&UInt256>) -> Result<Self> {
        let hash = block_hash.ok_or_else(|| error!("FinalizeVote requires block_hash"))?;
        Ok(FinalizeVote { slot, block_hash: hash.clone() })
    }

    fn vote_type_name() -> &'static str {
        "finalize"
    }
}

impl VerifiableVote for SkipVote {
    fn extract_from_tl(tl_vote: &UnsignedVote) -> Option<(SlotIndex, Option<UInt256>)> {
        match tl_vote {
            UnsignedVote::Consensus_Simplex_SkipVote(v) => {
                Some((SlotIndex::new(v.slot as u32), None))
            }
            _ => None,
        }
    }

    fn create(slot: SlotIndex, _block_hash: Option<&UInt256>) -> Result<Self> {
        Ok(SkipVote { slot })
    }

    fn vote_type_name() -> &'static str {
        "skip"
    }
}

/*
    ============================================================================
    Certificate TL Serialization
    ============================================================================
*/

impl<T: ToTlUnsignedVote> Certificate<T> {
    /// Convert to TL Certificate
    ///
    /// Reference: C++ `Certificate<T>::to_tl()`
    ///
    /// # Returns
    ///
    /// Result with boxed TL Certificate object, or error if vote conversion fails
    #[allow(dead_code)] // Available for full serialization in future
    pub fn to_tl(&self) -> Result<CertificateBoxed> {
        Ok(TlCertificate {
            vote: self.vote.to_tl_unsigned()?,
            signatures: self.to_tl_vote_signature_set(),
        }
        .into_boxed())
    }
}

/*
    ============================================================================
    Certificate Deserialization with Verification
    ============================================================================
*/

impl<T: ToTlUnsignedVote> Certificate<T> {
    /// Deserialize from TL VoteSignatureSet with signature verification
    ///
    /// Reference: C++ `Certificate<T>::from_tl(voteSignatureSet&&, T vote, const Bus& bus)`
    ///
    /// # Arguments
    ///
    /// * `tl_sigs` - TL VoteSignatureSet object
    /// * `vote` - The vote being certified (caller provides, verified against signatures)
    /// * `desc` - Session description for validator lookup
    /// * `session_id` - Session ID for signature verification
    ///
    /// # Returns
    ///
    /// Ok(Certificate) if:
    /// - All validator indices are valid
    /// - No duplicate validators
    /// - All signatures verify
    /// - Total weight >= 2/3 threshold
    ///
    /// # Errors
    ///
    /// - Invalid validator index
    /// - Duplicate validator in signatures
    /// - Invalid signature
    /// - Insufficient weight (< 2/3)
    pub fn from_tl_signatures(
        tl_sigs: &VoteSignatureSetBoxed,
        vote: T,
        desc: &SessionDescription,
        session_id: &SessionId,
    ) -> Result<Self> {
        let votes_vec = tl_sigs.votes();

        // Get vote bytes for signature verification
        // Wrap in dataToSign(session_id, vote_bytes) - matches C++ types.cpp
        let unsigned_vote = vote.to_tl_unsigned()?;
        let raw_vote_bytes = serialize_unsigned_vote(&unsigned_vote);
        let vote_bytes = crate::utils::create_data_to_sign(session_id, &raw_vote_bytes);

        let num_validators = desc.get_total_nodes();
        let mut voted = vec![false; num_validators];
        let mut signatures = Vec::with_capacity(votes_vec.len());
        let mut voted_weight: u64 = 0;

        for tl_sig in votes_vec.iter() {
            let sig = VoteSignature::from_tl(tl_sig);
            let validator_idx = sig.validator_idx;

            // Check validator index bounds
            if validator_idx.value() as usize >= num_validators {
                fail!(
                    "Invalid validator index {} in certificate (num_validators={})",
                    validator_idx,
                    num_validators
                )
            }

            // Check for duplicates
            if voted[validator_idx.value() as usize] {
                fail!("Duplicate validator index {} in certificate", validator_idx)
            }
            voted[validator_idx.value() as usize] = true;

            // Verify signature
            let validator_key = desc.get_source_public_key(validator_idx);
            if validator_key.verify(&vote_bytes, &sig.signature).is_err() {
                fail!("Invalid vote signature for validator {}", validator_idx)
            }

            voted_weight += desc.get_node_weight(validator_idx);
            signatures.push(sig);
        }

        // Check weight threshold
        let threshold = desc.get_threshold_66();
        if voted_weight < threshold {
            fail!(
                "Not enough signatures in certificate: weight {} < threshold {}",
                voted_weight,
                threshold
            )
        }

        Ok(Self { vote, signatures })
    }
}

/*
    ============================================================================
    Certificate from Generic Vote (for parsing TL Certificate)
    ============================================================================
*/

impl Certificate<Vote> {
    /// Parse and verify certificate from TL Certificate
    ///
    /// **Validation Policy (C++ strict)**:
    /// - Invalid validator index: Reject
    /// - Duplicate validator index: Reject
    /// - Invalid signatures: Reject the entire certificate
    /// - Insufficient weight (< 2/3): Reject the entire certificate
    ///
    /// Reference: C++ `Certificate<Vote>::from_tl(certificate&&, const Bus& bus)`
    ///
    /// # Arguments
    /// * `tl_cert` - TL Certificate object
    /// * `desc` - Session description for validator lookup and weight computation
    /// * `session_id` - Session ID for signature verification
    ///
    /// # Returns
    /// Ok(Certificate<Vote>) if valid, Err with description if rejected
    pub fn from_tl(
        tl_cert: &CertificateBoxed,
        desc: &SessionDescription,
        session_id: &SessionId,
    ) -> Result<Self> {
        let unsigned_vote = tl_cert.vote();

        // Parse vote from TL
        let vote = crate::utils::tl_unsigned_to_vote(unsigned_vote)?;

        // Get vote bytes for signature verification
        // Wrap in dataToSign(session_id, vote_bytes) - matches C++ types.cpp
        let raw_vote_bytes = serialize_unsigned_vote(unsigned_vote);
        let vote_bytes = crate::utils::create_data_to_sign(session_id, &raw_vote_bytes);

        // Parse and verify signatures
        let votes_vec = tl_cert.signatures().votes();
        let num_validators = desc.get_total_nodes();
        let mut voted = vec![false; num_validators];
        let mut signatures = Vec::with_capacity(votes_vec.len());
        let mut voted_weight: u64 = 0;

        for tl_sig in votes_vec.iter() {
            let sig = VoteSignature::from_tl(tl_sig);
            let validator_idx = sig.validator_idx;

            // C++ strict: reject invalid validator index
            if validator_idx.value() as usize >= num_validators {
                fail!("Invalid validator index {} in certificate", validator_idx)
            }

            // C++ strict: reject duplicates
            if voted[validator_idx.value() as usize] {
                fail!("Duplicate validator index {} in certificate", validator_idx)
            }

            // Verify signature - REJECT the entire certificate if any signature is invalid
            let validator_key = desc.get_source_public_key(validator_idx);
            if validator_key.verify(&vote_bytes, &sig.signature).is_err() {
                fail!("Invalid vote signature for validator {}", validator_idx)
            }

            // Mark as voted and accumulate weight
            voted[validator_idx.value() as usize] = true;
            voted_weight += desc.get_node_weight(validator_idx);
            signatures.push(sig);
        }

        // Check weight threshold - REJECT if insufficient weight after processing
        let threshold = desc.get_threshold_66();
        if voted_weight < threshold {
            // Match C++ behavior (`certificate.cpp`): reject with a generic message.
            fail!("Not enough signatures in certificate")
        }

        Ok(Self { vote, signatures })
    }
}

/*
    ============================================================================
    Generic Certificate Verification from TL
    ============================================================================

    Verifies and creates certificates from TL for any vote type.
    Reference: C++ CandidateResolver certificate verification
*/

impl<T: VerifiableVote> Certificate<T> {
    /// Verify and create Certificate from TL Certificate
    ///
    /// This method:
    /// 1. Verifies the certificate contains the expected vote type for the expected slot/block
    /// 2. Verifies all signatures
    /// 3. Checks the weight meets the 2/3 threshold
    /// 4. Returns the verified Certificate
    ///
    /// # Arguments
    ///
    /// * `tl_cert` - TL Certificate object to verify
    /// * `expected_slot` - Expected slot number in the vote
    /// * `expected_block_hash` - Expected block hash (Some for notarize/finalize, None for skip)
    /// * `desc` - Session description for validator lookup and weight calculation
    ///
    /// # Returns
    ///
    /// Ok(Arc<Certificate<T>>) if valid, Err with descriptive message otherwise
    ///
    /// # Errors
    ///
    /// - Certificate contains wrong vote type
    /// - Slot mismatch
    /// - Block hash mismatch (for notarize/finalize)
    /// - Invalid validator index
    /// - Duplicate validator
    /// - Invalid signature
    /// - Insufficient weight (< 2/3)
    #[allow(dead_code)] // Available for TL verification in future
    pub fn verify_from_tl(
        tl_cert: &CertificateBoxed,
        expected_slot: SlotIndex,
        expected_block_hash: Option<&UInt256>,
        desc: &SessionDescription,
        session_id: &SessionId,
    ) -> Result<Arc<Self>> {
        // Extract vote and signatures from TL
        let tl_vote = tl_cert.vote();

        // Verify vote type and extract slot/hash
        let (cert_slot, cert_hash) = T::extract_from_tl(tl_vote)
            .ok_or_else(|| error!("Certificate contains non-{} vote", T::vote_type_name()))?;

        // Verify slot matches
        if cert_slot != expected_slot {
            fail!("Certificate slot mismatch: expected {}, got {}", expected_slot, cert_slot)
        }

        // Verify block hash matches (if applicable)
        match (expected_block_hash, &cert_hash) {
            (Some(expected), Some(actual)) if expected != actual => {
                fail!("Certificate block hash mismatch")
            }
            (Some(_), None) => {
                fail!("Certificate missing block hash for {} vote", T::vote_type_name())
            }
            _ => {}
        }

        // Get vote bytes for signature verification
        // Wrap in dataToSign(session_id, vote_bytes) - matches C++ types.cpp
        let raw_vote_bytes = serialize_unsigned_vote(tl_vote);
        let vote_bytes = crate::utils::create_data_to_sign(session_id, &raw_vote_bytes);

        // Parse signatures
        let votes_vec = tl_cert.signatures().votes();
        let num_validators = desc.get_total_nodes();
        let mut voted = vec![false; num_validators];
        let mut signatures = Vec::with_capacity(votes_vec.len());

        for tl_sig in votes_vec.iter() {
            let sig = VoteSignature::from_tl(tl_sig);
            let validator_idx = sig.validator_idx;

            // Check validator index bounds
            if validator_idx.value() as usize >= num_validators {
                fail!(
                    "Invalid validator index {} in certificate (num_validators={})",
                    validator_idx,
                    num_validators
                )
            }

            // Check for duplicates
            if voted[validator_idx.value() as usize] {
                fail!("Duplicate validator index {} in certificate", validator_idx)
            }
            voted[validator_idx.value() as usize] = true;

            // Verify signature
            let validator_key = desc.get_source_public_key(validator_idx);
            if validator_key.verify(&vote_bytes, &sig.signature).is_err() {
                fail!("Invalid vote signature for validator {}", validator_idx)
            }

            signatures.push(sig);
        }

        // Create the vote and certificate
        let vote = T::create(expected_slot, expected_block_hash)?;
        let cert = Certificate::new(vote, signatures);

        // Check weight threshold using Certificate method
        if !cert.has_sufficient_weight(desc) {
            let total = cert.total_weight(desc);
            let threshold = desc.get_threshold_66();
            fail!("Certificate weight {} below threshold {}", total, threshold)
        }

        Ok(Arc::new(cert))
    }
}
