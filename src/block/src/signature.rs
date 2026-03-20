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
use crate::{
    blocks::BlockIdExt,
    define_HashmapE,
    error::BlockError,
    fail,
    validators::{ValidatorBaseInfo, ValidatorDescr},
    BuilderData, Cell, Deserializable, Ed25519KeyOption, HashmapE, HashmapType, IBitstring,
    KeyOption, Result, Serializable, SliceData, UInt256, ED25519_PUBLIC_KEY_LENGTH,
    ED25519_SIGNATURE_LENGTH,
};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    str::FromStr,
    sync::Arc,
};

/*
ed25519_signature#5 R:bits256 s:bits256 = CryptoSignature;
*/
///
/// CryptoSignature
///
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CryptoSignature([u8; ED25519_SIGNATURE_LENGTH]);

impl Default for CryptoSignature {
    fn default() -> Self {
        Self([0; ED25519_SIGNATURE_LENGTH])
    }
}

impl CryptoSignature {
    pub fn with_bytes(bytes: [u8; ED25519_SIGNATURE_LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self::with_bytes(bytes.try_into()?))
    }

    pub fn from_r_s_str(r: &str, s: &str) -> Result<Self> {
        let mut signature = Self::default();
        hex::decode_to_slice(r, &mut signature.0[..ED25519_SIGNATURE_LENGTH / 2]).map_err(
            |err| BlockError::InvalidData(format!("error parsing `r` hex string: {}", err)),
        )?;
        hex::decode_to_slice(s, &mut signature.0[ED25519_SIGNATURE_LENGTH / 2..]).map_err(
            |err| BlockError::InvalidData(format!("error parsing `s` hex string: {}", err)),
        )?;
        Ok(signature)
    }

    pub fn with_r_s(r: &[u8; 32], s: &[u8; 32]) -> Self {
        let mut signature = Self::default();
        signature.0[..ED25519_SIGNATURE_LENGTH / 2].copy_from_slice(r);
        signature.0[ED25519_SIGNATURE_LENGTH / 2..].copy_from_slice(s);
        signature
    }

    pub fn as_r_s_bytes(&self) -> (&[u8], &[u8]) {
        let r_bytes = &self.0[..ED25519_SIGNATURE_LENGTH / 2];
        let s_bytes = &self.0[ED25519_SIGNATURE_LENGTH / 2..];
        (r_bytes, s_bytes)
    }

    pub fn as_bytes(&self) -> &[u8; ED25519_SIGNATURE_LENGTH] {
        &self.0
    }
}

impl FromStr for CryptoSignature {
    type Err = crate::Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut signature = Self::default();
        hex::decode_to_slice(s, &mut signature.0)?;
        Ok(signature)
    }
}

const CRYPTO_SIGNATURE_TAG: u8 = 0x5;

impl Serializable for CryptoSignature {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(CRYPTO_SIGNATURE_TAG as usize, 4)?;
        cell.append_raw(&self.0, ED25519_SIGNATURE_LENGTH * 8)?;
        Ok(())
    }
}

impl Deserializable for CryptoSignature {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(4)? as u8;
        if tag != CRYPTO_SIGNATURE_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        slice.get_next_bytes_to_slice(&mut self.0)?;
        Ok(())
    }
}

/*
sig_pair$_ node_id_short:bits256 sign:CryptoSignature = CryptoSignaturePair;
*/
///
/// CryptoSignaturePair
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CryptoSignaturePair {
    pub node_id_short: UInt256,
    pub sign: CryptoSignature,
}

impl CryptoSignaturePair {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_params(node_id_short: UInt256, sign: CryptoSignature) -> Self {
        CryptoSignaturePair { node_id_short, sign }
    }
}

impl Serializable for CryptoSignaturePair {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.node_id_short.write_to(cell)?;
        self.sign.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for CryptoSignaturePair {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.node_id_short.read_from(cell)?;
        self.sign.read_from(cell)?;
        Ok(())
    }
}

/*
ed25519_pubkey#8e81278a pubkey:bits256 = SigPubKey;
*/

///
/// SigPubKey
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SigPubKey([u8; ED25519_PUBLIC_KEY_LENGTH]);

const SIG_PUB_KEY_TAG: u32 = 0x8e81278a;

impl SigPubKey {
    pub fn with_bytes(bytes: [u8; ED25519_PUBLIC_KEY_LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.as_ref().try_into()?))
    }

    pub fn key_bytes(&self) -> &[u8; ED25519_PUBLIC_KEY_LENGTH] {
        self.as_bytes()
    }
    pub fn as_bytes(&self) -> &[u8; ED25519_PUBLIC_KEY_LENGTH] {
        &self.0
    }

    pub fn pub_key(&self) -> Arc<dyn KeyOption> {
        Ed25519KeyOption::from_public_key(&self.0)
    }

    pub fn key_id(&self) -> [u8; 32] {
        *self.pub_key().id().data()
    }

    // be careful here - we recreate public key object everytime
    pub fn verify_signature(&self, data: &[u8], signature: &CryptoSignature) -> bool {
        self.pub_key().verify(data, signature.as_bytes()).is_ok()
    }

    pub fn as_slice(&self) -> &[u8; 32] {
        &self.0
    }
}

impl PartialEq<UInt256> for SigPubKey {
    fn eq(&self, other: &UInt256) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl FromStr for SigPubKey {
    type Err = crate::Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut public_key = Self::default();
        hex::decode_to_slice(s, &mut public_key.0)?;
        Ok(public_key)
    }
}

impl AsRef<[u8]> for SigPubKey {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Serializable for SigPubKey {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(SIG_PUB_KEY_TAG)?;
        cell.append_raw(&self.0, ED25519_PUBLIC_KEY_LENGTH * 8)?;
        Ok(())
    }
}

impl Deserializable for SigPubKey {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let tag = slice.get_next_u32()?;
        if tag != SIG_PUB_KEY_TAG {
            fail!(Self::invalid_tag(tag))
        }
        let mut public_key = Self::default();
        slice.get_next_bytes_to_slice(&mut public_key.0)?;
        Ok(public_key)
    }
}

/*
  PROOFS
*/

/*
block_signatures_pure#_
    sig_count:uint32
    sig_weight:uint64
    signatures:(HashmapE 16 CryptoSignaturePair)
= BlockSignaturesPure;
*/

define_HashmapE! {CryptoSignaturePairDict, 16, CryptoSignaturePair}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlockSignaturesPure {
    sig_count: u32,
    sig_weight: u64,
    signatures: CryptoSignaturePairDict,
}

impl BlockSignaturesPure {
    pub fn new() -> Self {
        Self::default()
    }
    /// New instance of BlockSignaturesPure
    pub fn with_weight(sig_weight: u64) -> Self {
        Self { sig_count: 0, sig_weight, signatures: CryptoSignaturePairDict::default() }
    }

    /// Get count of signatures
    pub fn count(&self) -> u32 {
        self.sig_count
    }

    /// Get weight
    pub fn weight(&self) -> u64 {
        self.sig_weight
    }

    pub fn set_weight(&mut self, weight: u64) {
        self.sig_weight = weight;
    }

    /// Add crypto signature pair to BlockSignaturesPure
    pub fn add_sigpair(&mut self, signature: CryptoSignaturePair) {
        self.signatures.set(&(self.sig_count as u16), &signature).unwrap();
        self.sig_count += 1;
    }

    pub fn signatures(&self) -> &HashmapE {
        &self.signatures.0
    }

    pub fn check_signatures(&self, validators_list: &[ValidatorDescr], data: &[u8]) -> Result<u64> {
        // Calc validators short ids
        let mut validators_map = HashMap::new();
        for vd in validators_list {
            validators_map.insert(vd.compute_node_id_short(), vd);
        }

        // Check signatures
        let mut weight = 0;
        let mut used_keys = HashSet::new();
        self.signatures().iterate_slices(|ref mut _key, ref mut slice| {
            let sign = CryptoSignaturePair::construct_from(slice)?;
            if let Some(vd) = validators_map.get(&sign.node_id_short) {
                if !used_keys.insert(sign.node_id_short) {
                    fail!(BlockError::DuplicatedSignature)
                }
                if !vd.verify_signature(data, &sign.sign) {
                    fail!(BlockError::BadSignature)
                }
                weight += vd.weight;
            }
            Ok(true)
        })?;
        Ok(weight)
    }
}

impl Serializable for BlockSignaturesPure {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.sig_count.write_to(cell)?;
        self.sig_weight.write_to(cell)?;
        self.signatures.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for BlockSignaturesPure {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.sig_count.read_from(cell)?;
        self.sig_weight.read_from(cell)?;
        self.signatures.read_from(cell)?;
        Ok(())
    }
}

/*
block_signatures#11
    validator_info:ValidatorBaseInfo
    pure_signatures:BlockSignaturesPure
= BlockSignatures;
*/

///
/// BlockSignatures
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct BlockSignatures {
    pub validator_info: ValidatorBaseInfo,
    pub pure_signatures: BlockSignaturesPure,
}

impl BlockSignatures {
    /// Create new empty instance of BlockSignatures
    pub fn new() -> Self {
        Self::default()
    }

    /// Create new instance of BlockSignatures
    pub fn with_params(
        validator_info: ValidatorBaseInfo,
        pure_signatures: BlockSignaturesPure,
    ) -> Self {
        BlockSignatures { validator_info, pure_signatures }
    }
}

const BLOCK_SIGNATURES_TAG: u8 = 0x11;

impl Serializable for BlockSignatures {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(BLOCK_SIGNATURES_TAG)?;
        self.validator_info.write_to(cell)?;
        self.pure_signatures.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for BlockSignatures {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != BLOCK_SIGNATURES_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.validator_info.read_from(cell)?;
        self.pure_signatures.read_from(cell)?;
        Ok(())
    }
}

/*
block_signatures_simplex#12
    validator_info:ValidatorBaseInfo
    pure_signatures:BlockSignaturesPure
    session_id:bits256
    slot:uint32
    candidate_data:^Cell
= BlockSignatures;

Simplex consensus signatures with verification context.
Contains all data needed to reconstruct the signed data for verification.

Reference: C++ signature-set.h BlockSignatureSetSimplex
*/

const BLOCK_SIGNATURES_SIMPLEX_TAG: u8 = 0x12;

/// BlockSignaturesSimplex (Simplex format, tag 0x12)
///
/// Contains signatures from Simplex consensus along with the context
/// needed to verify them. Unlike ordinary signatures which sign
/// `ton_blockId(root_hash, file_hash)`, Simplex signatures sign
/// `consensus.dataToSign(session_id, vote)` where vote is a
/// notarize or finalize vote containing the CandidateId.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockSignaturesSimplex {
    pub validator_info: ValidatorBaseInfo,
    pub pure_signatures: BlockSignaturesPure,
    /// Simplex session ID (256-bit)
    pub session_id: UInt256,
    /// Slot number in the consensus session
    pub slot: u32,
    /// CandidateHashData stored as a Cell tree (matches TL-B: candidate_data:^Cell)
    /// Contains raw TL-serialized bytes (no length prefix), possibly spanning multiple cells
    pub candidate_data: Cell,
    /// true = FinalizeVote, false = NotarizeVote
    pub is_final: bool,
}

impl Default for BlockSignaturesSimplex {
    fn default() -> Self {
        Self {
            validator_info: ValidatorBaseInfo::default(),
            pure_signatures: BlockSignaturesPure::default(),
            session_id: UInt256::default(),
            slot: 0,
            candidate_data: Cell::default(),
            is_final: false,
        }
    }
}

impl BlockSignaturesSimplex {
    /// Create new instance with candidate_data as bytes (converts to Cell internally)
    pub fn with_params(
        validator_info: ValidatorBaseInfo,
        pure_signatures: BlockSignaturesPure,
        session_id: UInt256,
        slot: u32,
        candidate_data: Cell,
        is_final: bool,
    ) -> Self {
        //let candidate_data = Self::bytes_to_cell_tree(&candidate_data_bytes)?;
        Self { validator_info, pure_signatures, session_id, slot, candidate_data, is_final }
    }

    // ========================================================================
    // Factory methods (match C++ create_simplex / create_simplex_approve)
    // ========================================================================

    /// Create new instance for FINALIZED signatures (is_final = true)
    ///
    /// Reference: C++ BlockSignatureSet::create_simplex() - signature-set.cpp
    /// Use this for signatures that will be stored in block proofs.
    pub fn new_finalize(
        validator_info: ValidatorBaseInfo,
        pure_signatures: BlockSignaturesPure,
        session_id: UInt256,
        slot: u32,
        candidate_data: Cell,
    ) -> Self {
        Self::with_params(validator_info, pure_signatures, session_id, slot, candidate_data, true)
    }

    /// Create new instance for NOTARIZED (approve) signatures (is_final = false)
    ///
    /// Reference: C++ BlockSignatureSet::create_simplex_approve() - signature-set.cpp
    /// Use this for approve/notarize signatures during consensus.
    /// These CANNOT be serialized to cell format.
    pub fn new_notarize(
        validator_info: ValidatorBaseInfo,
        pure_signatures: BlockSignaturesPure,
        session_id: UInt256,
        slot: u32,
        candidate_data: Cell,
    ) -> Self {
        Self::with_params(validator_info, pure_signatures, session_id, slot, candidate_data, false)
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Returns true if this is a finalized signature set
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    /// Set validator info (used when creating block proof)
    pub fn set_validator_info(&mut self, validator_info: ValidatorBaseInfo) {
        self.validator_info = validator_info;
    }

    /// Get mutable reference to pure_signatures (used for updating weight)
    pub fn pure_signatures_mut(&mut self) -> &mut BlockSignaturesPure {
        &mut self.pure_signatures
    }

    /// Get candidate_data as bytes (extracts from Cell tree)
    pub fn candidate_data_bytes(&self) -> Result<Vec<u8>> {
        Self::cell_tree_to_bytes(&self.candidate_data)
    }

    // ========================================================================
    // CellString: matches C++ vm::CellString (crypto/vm/cells/CellString.cpp)
    //
    // Stores raw bytes into a cell chain WITHOUT length prefix.
    // Each cell holds up to 127 bytes (1016 bits, byte-aligned from 1023).
    // Remaining data goes into child cell via reference.
    // ========================================================================

    const CELL_STRING_MAX_BYTES: usize = 1024;
    const CELL_STRING_MAX_CHAIN: usize = 16;
    const CELL_STRING_BYTES_PER_CELL: usize = 127; // Cell::max_bits / 8

    /// Store bytes into cell chain (C++ vm::CellString::store)
    pub fn bytes_to_cell_tree(bytes: &[u8]) -> Result<Cell> {
        if bytes.len() > Self::CELL_STRING_MAX_BYTES {
            fail!("String is too long (1)");
        }
        let depth = bytes.len().div_ceil(Self::CELL_STRING_BYTES_PER_CELL).max(1);
        if depth > Self::CELL_STRING_MAX_CHAIN {
            fail!("String is too long (2)");
        }

        let head = bytes.len().min(Self::CELL_STRING_BYTES_PER_CELL);
        let mut builder = BuilderData::new();
        builder.append_raw(&bytes[..head], head * 8)?;
        if head < bytes.len() {
            builder.checked_append_reference(Self::bytes_to_cell_tree(&bytes[head..])?)?;
        }
        builder.into_cell()
    }

    /// Load bytes from cell chain (C++ vm::CellString::load)
    fn cell_tree_to_bytes(cell: &Cell) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        let mut current = cell.clone();

        for _ in 0..Self::CELL_STRING_MAX_CHAIN {
            let mut slice = SliceData::load_cell_ref(&current)?;
            let bits = slice.remaining_bits();
            if bits % 8 != 0 {
                fail!("Size is not divisible by 8");
            }
            result.extend(slice.get_next_bytes(bits / 8)?);
            if result.len() > Self::CELL_STRING_MAX_BYTES {
                fail!("String is too long (1)");
            }
            if slice.remaining_references() == 0 {
                return Ok(result);
            }
            current = slice.checked_drain_reference()?;
        }
        fail!("String is too long (2)")
    }
}

impl Serializable for BlockSignaturesSimplex {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if !self.is_final {
            // Match C++ behavior: approve/notar signatures must not be serialized into block proofs.
            fail!("cannot serialize approve simplex signatures to cell");
        }
        cell.append_u8(BLOCK_SIGNATURES_SIMPLEX_TAG)?;
        self.validator_info.write_to(cell)?;
        self.pure_signatures.write_to(cell)?;
        cell.append_raw(self.session_id.as_slice(), 256)?;
        cell.append_u32(self.slot)?;

        // Store candidate_data Cell as a reference
        cell.checked_append_reference(self.candidate_data.clone())?;
        Ok(())
    }
}

impl Deserializable for BlockSignaturesSimplex {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != BLOCK_SIGNATURES_SIMPLEX_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.validator_info.read_from(cell)?;
        self.pure_signatures.read_from(cell)?;
        let mut session_id_bytes = [0u8; 32];
        cell.get_next_bytes_to_slice(&mut session_id_bytes)?;
        self.session_id = UInt256::from(session_id_bytes);
        self.slot = cell.get_next_u32()?;

        // Read candidate_data Cell from reference
        self.candidate_data = cell.checked_drain_reference()?;
        // In C++ only finalized simplex signature sets are serialized into block proofs.
        // The `final_` flag is not present in `block.tlb` cell schema.
        self.is_final = true;
        Ok(())
    }
}

/// Unified block signatures - either ordinary (catchain) or simplex
///
/// This enum allows code to handle both signature formats uniformly,
/// with `check_signatures()` automatically using the appropriate
/// verification scheme.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockSignaturesVariant {
    /// Catchain/validator-session signatures (tag 0x11)
    Ordinary(BlockSignatures),
    /// Simplex consensus signatures (tag 0x12)
    Simplex(BlockSignaturesSimplex),
}

impl Default for BlockSignaturesVariant {
    fn default() -> Self {
        Self::Ordinary(BlockSignatures::default())
    }
}

impl BlockSignaturesVariant {
    /// Get pure signatures (common to both types)
    pub fn pure_signatures(&self) -> &BlockSignaturesPure {
        match self {
            Self::Ordinary(s) => &s.pure_signatures,
            Self::Simplex(s) => &s.pure_signatures,
        }
    }

    /// Get mutable pure signatures
    pub fn pure_signatures_mut(&mut self) -> &mut BlockSignaturesPure {
        match self {
            Self::Ordinary(s) => &mut s.pure_signatures,
            Self::Simplex(s) => &mut s.pure_signatures,
        }
    }

    /// Get validator info (common to both types)
    pub fn validator_info(&self) -> &ValidatorBaseInfo {
        match self {
            Self::Ordinary(s) => &s.validator_info,
            Self::Simplex(s) => &s.validator_info,
        }
    }

    /// Get mutable validator info
    pub fn validator_info_mut(&mut self) -> &mut ValidatorBaseInfo {
        match self {
            Self::Ordinary(s) => &mut s.validator_info,
            Self::Simplex(s) => &mut s.validator_info,
        }
    }

    /// Create Ordinary variant from existing BlockSignatures
    pub fn from_ordinary(sigs: BlockSignatures) -> Self {
        Self::Ordinary(sigs)
    }

    /// Create Simplex variant from existing BlockSignaturesSimplex
    pub fn from_simplex(sigs: BlockSignaturesSimplex) -> Self {
        Self::Simplex(sigs)
    }

    /// Returns true if this is an Ordinary variant
    pub fn is_ordinary(&self) -> bool {
        matches!(self, Self::Ordinary(_))
    }

    /// Returns true if this is a Simplex variant
    pub fn is_simplex(&self) -> bool {
        matches!(self, Self::Simplex(_))
    }

    /// Get as Ordinary variant if applicable
    pub fn as_ordinary(&self) -> Option<&BlockSignatures> {
        match self {
            Self::Ordinary(s) => Some(s),
            Self::Simplex(_) => None,
        }
    }

    /// Get as Simplex variant if applicable
    pub fn as_simplex(&self) -> Option<&BlockSignaturesSimplex> {
        match self {
            Self::Ordinary(_) => None,
            Self::Simplex(s) => Some(s),
        }
    }
}

impl Serializable for BlockSignaturesVariant {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            Self::Ordinary(s) => s.write_to(cell),
            Self::Simplex(s) => s.write_to(cell),
        }
    }
}

impl Deserializable for BlockSignaturesVariant {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        match tag {
            BLOCK_SIGNATURES_TAG => {
                let mut sigs = BlockSignatures::default();
                sigs.validator_info.read_from(cell)?;
                sigs.pure_signatures.read_from(cell)?;
                *self = Self::Ordinary(sigs);
            }
            BLOCK_SIGNATURES_SIMPLEX_TAG => {
                let mut sigs = BlockSignaturesSimplex::default();
                sigs.validator_info.read_from(cell)?;
                sigs.pure_signatures.read_from(cell)?;
                let mut session_id_bytes = [0u8; 32];
                cell.get_next_bytes_to_slice(&mut session_id_bytes)?;
                sigs.session_id = UInt256::from(session_id_bytes);
                sigs.slot = cell.get_next_u32()?;
                // Read candidate_data Cell from reference
                sigs.candidate_data = cell.checked_drain_reference()?;
                // See comment in BlockSignaturesSimplex::read_from
                sigs.is_final = true;
                *self = Self::Simplex(sigs);
            }
            _ => fail!(BlockSignatures::invalid_tag(tag as u32)),
        }
        Ok(())
    }
}

/*
block_proof#c3
    proof_for:BlockIdExt
    root:^Cell
    signatures:(Maybe ^BlockSignaturesVariant)
= BlockProof;
*/

///
/// BlockProof
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct BlockProof {
    pub proof_for: BlockIdExt,
    pub root: Cell,
    pub signatures: Option<BlockSignaturesVariant>,
}

impl BlockProof {
    /// Create new empty instance of BlockProof
    pub fn new() -> Self {
        Self::default()
    }

    /// Create new instance of BlockProof with variant signatures
    pub fn with_params(
        proof_for: BlockIdExt,
        root: Cell,
        signatures: Option<BlockSignaturesVariant>,
    ) -> Self {
        BlockProof { proof_for, root, signatures }
    }

    /// Create new instance of BlockProof with ordinary (catchain) signatures
    ///
    /// Convenience constructor for creating proofs with standard BlockSignatures.
    pub fn with_ordinary_signatures(
        proof_for: BlockIdExt,
        root: Cell,
        signatures: Option<BlockSignatures>,
    ) -> Self {
        BlockProof { proof_for, root, signatures: signatures.map(BlockSignaturesVariant::Ordinary) }
    }
}

const BLOCK_PROOF_TAG: u8 = 0xC3;

impl Serializable for BlockProof {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(BLOCK_PROOF_TAG)?;
        self.proof_for.write_to(cell)?;
        cell.checked_append_reference(self.root.clone())?;
        if let Some(s) = self.signatures.as_ref() {
            cell.append_bit_one()?;
            cell.checked_append_reference(s.serialize()?)?;
        } else {
            cell.append_bit_zero()?;
        }
        Ok(())
    }
}

impl Deserializable for BlockProof {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_byte()?;
        if tag != BLOCK_PROOF_TAG {
            fail!(Self::invalid_tag(tag as u32))
        }
        self.proof_for.read_from(cell)?;
        self.root = cell.checked_drain_reference()?;
        self.signatures = if cell.get_next_bit()? {
            Some(BlockSignaturesVariant::construct_from_reference(cell)?)
        } else {
            None
        };
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/test_signature.rs"]
mod tests;
