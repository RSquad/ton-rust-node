/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # Simplex Database
//!
//! Persistent storage for Simplex consensus state.
//!
//! ## Overview
//!
//! Uses `AsyncKeyValueStorage` from `consensus-common` for non-blocking RocksDB access.
//! By default writes are fire-and-forget (matching C++ behavior with `.start().detach()`),
//! but for write-ordering parity with C++ (`co_await db->set()` / `WaitCandidateInfoStored`)
//! this module also exposes `*_async()` variants returning `StorageAsyncResultPtr<()>`.
//! Reads are blocking (used only at startup, matching C++ synchronous get).
//!
//! ## Persisted Data
//!
//! | Type | Purpose | TL Key | TL Value |
//! |------|---------|--------|----------|
//! | Finalized Block | Track finalization chain | `consensus.simplex.db.key.finalizedBlock` | `consensus.simplex.db.finalizedBlock` |
//! | Certificates (notar/final/skip) | Bootstrap certificate cache | `consensus.simplex.db.key.vote` | `consensus.simplex.db.cert` |
//! | Vote | Replay ordering recovery | `consensus.simplex.db.key.vote` | `consensus.simplex.db.vote` |
//! | Candidate Info | Candidate metadata | `consensus.simplex.db.key.candidateResolver.candidateInfo` | `consensus.simplex.db.candidateResolver.candidateInfo` |
//!
//! ## C++ Reference
//!
//! - `simplex/consensus.cpp` - `load_from_db()`, `do_finalize_block()` with `co_await db->set()`
//! - `simplex/candidate-resolver.cpp` - `load_from_db()`, `store_to_db()` with `.start().detach()`
//! - `bus.h` - `class Db` with sync `get()` / `get_by_prefix()`, async `set()`
//! - `bridge.cpp` - DB path: `{db_root}/consensus/consensus.{workchain}.{shard}.{cc_seqno}.{session_id}/`

use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex, WindowIndex},
    certificate::{FinalCert, NotarCert, SkipCert},
};
use consensus_common::{
    AsyncKeyValueStorageOptions, AsyncKeyValueStoragePtr, ConsensusCommonFactory, RawBuffer,
    StorageAsyncResultPtr,
};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_api::{
    deserialize_typed, serialize_boxed,
    ton::{
        consensus::{
            candidateid::CandidateId,
            candidateparent::CandidateParent,
            simplex::{
                certificate::Certificate as TlCertificate,
                db::{
                    candidate_resolver::{
                        candidateinfo::CandidateInfo as CandidateInfoValue,
                        CandidateInfo as CandidateInfoValueBoxed,
                    },
                    finalizedblock::FinalizedBlock as FinalizedBlockValue,
                    key::{
                        candidate::Candidate as CandidatePayloadKey,
                        candidate_resolver::{
                            candidateinfo::CandidateInfo as CandidateInfoKey,
                            CandidateInfo as CandidateInfoKeyBoxed,
                        },
                        finalizedblock::FinalizedBlock as FinalizedBlockKey,
                        vote::Vote as VoteKey,
                        Candidate as CandidatePayloadKeyBoxed,
                        FinalizedBlock as FinalizedBlockKeyBoxed, PoolState as PoolStateKey,
                        Vote as VoteKeyBoxed,
                    },
                    poolstate::PoolState as PoolStateValue,
                    vote::{Cert as CertValue, Vote as VoteValue},
                    FinalizedBlock as FinalizedBlockValueBoxed, PoolState as PoolStateValueBoxed,
                    Vote as VoteValueBoxed,
                },
                Certificate as TlCertificateBoxed,
            },
            CandidateHashData, CandidateParent as CandidateParentBoxed,
        },
        Bool,
    },
    BoxedSerialize, Constructor, IntoBoxed,
};
use ton_block::{error, sha256_digest, BlockIdExt, Result, UInt256};

// ============================================================================
// Constants
// ============================================================================

/// Log target for database operations (matches simplex crate log target)
const TARGET: &str = "simplex";

/// Default sync timeout for blocking reads
const DEFAULT_SYNC_TIMEOUT: Duration = Duration::from_secs(5);

// ============================================================================
// TL Constructor IDs (for prefix scanning)
// ============================================================================

/// Get key prefix for finalized blocks
fn prefix_finalized_block() -> u32 {
    FinalizedBlockKey::constructor_const()
}

/// Get key prefix for candidate info
fn prefix_candidate_info() -> u32 {
    CandidateInfoKey::constructor_const()
}

/// Get key prefix for votes
fn prefix_vote() -> u32 {
    VoteKey::constructor_const()
}

/// Get key prefix for pool state (singleton)
fn prefix_pool_state() -> u32 {
    // PoolState key is a zero-arg enum; get constructor from BoxedSerialize
    PoolStateKey::default().bare_object().constructor()
}

/// Get key prefix for candidate payloads (full serialized CandidateData bytes).
///
/// C++ parity: `consensus.simplex.db.key.candidate` TL type in candidate-resolver.cpp.
fn prefix_candidate_payload() -> u32 {
    CandidatePayloadKey::constructor_const()
}

// ============================================================================
// Record Types
// ============================================================================

/// Finalized block record loaded from DB
#[derive(Debug, Clone)]
pub struct FinalizedBlockRecord {
    /// Candidate ID (slot + hash)
    pub candidate_id: RawCandidateId,
    /// Block ID with seqno
    pub block_id: BlockIdExt,
    /// Parent candidate (None for first block)
    pub parent: Option<RawCandidateId>,
    /// True if finalized with FinalCert, false if only notarized
    pub is_final: bool,
}

/// Candidate info record loaded from DB
#[derive(Debug, Clone)]
pub struct CandidateInfoRecord {
    /// Candidate ID (slot + hash)
    pub candidate_id: RawCandidateId,
    /// Leader validator index
    pub leader_idx: u32,
    /// Candidate hash data (for verification)
    pub candidate_hash_data: CandidateHashData,
    /// Leader's signature
    pub signature: RawBuffer,
}

/// Notarization certificate record loaded from DB
#[derive(Debug, Clone)]
pub struct NotarCertRecord {
    /// Candidate ID (slot + hash)
    pub candidate_id: RawCandidateId,
    /// Vote signature set (raw TL bytes for deserialization)
    pub notar_cert_bytes: RawBuffer,
}

/// Finalization certificate record loaded from DB.
#[derive(Debug, Clone)]
pub struct FinalCertRecord {
    /// Candidate ID (slot + hash)
    pub candidate_id: RawCandidateId,
    /// Serialized TL `consensus.simplex.certificate` bytes
    #[allow(dead_code)] // Used by tests now; reserved for receiver/recovery restoration flow.
    pub cert_bytes: RawBuffer,
}

/// Skip certificate record loaded from DB.
#[derive(Debug, Clone)]
pub struct SkipCertRecord {
    /// Slot index for this skip certificate
    pub slot: SlotIndex,
    /// Serialized TL `consensus.simplex.certificate` bytes
    #[allow(dead_code)] // Used by tests now; reserved for receiver/recovery restoration flow.
    pub cert_bytes: RawBuffer,
}

/// Vote record for restart support
///
/// Stores votes by their hash for standstill recovery.
/// Key: vote_hash (sha256 of serialized vote)
/// Value: raw vote data + node index + seqno
///
/// C++ parity: db.cpp assigns monotonic seqno to each vote for replay ordering.
/// Votes must be replayed in the order they were originally cast.
#[derive(Debug, Clone)]
pub struct VoteRecord {
    /// Hash of the vote (key)
    pub vote_hash: UInt256,
    /// Raw serialized vote data
    pub data: RawBuffer,
    /// Validator index that submitted this vote
    pub node_idx: ValidatorIndex,
    /// Monotonic sequence number for replay ordering (C++ parity)
    pub seqno: i64,
}

/// Pool state record for restart support
///
/// Singleton record storing consensus pool state.
#[derive(Debug, Clone, Default)]
pub struct PoolStateRecord {
    /// First window that hasn't been announced yet
    /// Used for generating skip votes after restart
    pub first_nonannounced_window: WindowIndex,
}

// ============================================================================
// Bootstrap Structures
// ============================================================================

/// Complete bootstrap data loaded from DB at session startup.
///
/// Contains all persistent state needed to restore consensus after restart.
/// Use `split()` to decompose into component-specific bootstrap data.
#[derive(Debug, Clone, Default)]
pub struct Bootstrap {
    /// Finalized blocks (sorted by slot)
    pub finalized_blocks: Vec<FinalizedBlockRecord>,
    /// Candidate info records
    pub candidate_infos: Vec<CandidateInfoRecord>,
    /// Notarization certificates (for receiver cache)
    pub notar_certs: Vec<NotarCertRecord>,
    /// Finalization certificates (for receiver cache)
    pub final_certs: Vec<FinalCertRecord>,
    /// Skip certificates (for receiver cache)
    pub skip_certs: Vec<SkipCertRecord>,
    /// Votes (for standstill recovery)
    pub votes: Vec<VoteRecord>,
    /// Pool state (for skip vote generation)
    pub pool_state: Option<PoolStateRecord>,
    /// Candidate payload bytes (serialized CandidateData, for requestCandidate serving)
    pub candidate_payloads: Vec<(RawCandidateId, Vec<u8>)>,
}

/// Bootstrap data for recovery processor (session state only, no candidate_infos).
///
/// Note: candidate_infos is used directly from Bootstrap before split() to build
/// the candidate_info_map in SessionStartupRecoveryProcessor.
#[derive(Debug, Clone, Default)]
pub struct SessionBootstrap {
    /// Finalized blocks (sorted by slot)
    pub finalized_blocks: Vec<FinalizedBlockRecord>,
    /// Votes (for vote replay and local flags)
    pub votes: Vec<VoteRecord>,
    /// Pool state (for first_nonannounced_window and skip vote generation)
    pub pool_state: Option<PoolStateRecord>,
}

/// Bootstrap data for Receiver certificate cache.
#[derive(Debug, Clone, Default)]
pub struct ReceiverBootstrap {
    /// Notarization certificates for cache population
    pub notar_certs: Vec<NotarCertRecord>,
    /// Finalization certificates for cache population
    #[allow(dead_code)] // Reserved for receiver recovery parity lane.
    pub final_certs: Vec<FinalCertRecord>,
    /// Skip certificates for cache population
    #[allow(dead_code)] // Reserved for receiver recovery parity lane.
    pub skip_certs: Vec<SkipCertRecord>,
}

impl Bootstrap {
    /// Split bootstrap into component-specific parts.
    ///
    /// Consumes self for zero-copy transfer of vectors.
    /// Returns (session_boot, receiver_boot, candidate_payloads).
    pub fn split(self) -> (SessionBootstrap, ReceiverBootstrap, Vec<(RawCandidateId, Vec<u8>)>) {
        (
            SessionBootstrap {
                finalized_blocks: self.finalized_blocks,
                votes: self.votes,
                pool_state: self.pool_state,
            },
            ReceiverBootstrap {
                notar_certs: self.notar_certs,
                final_certs: self.final_certs,
                skip_certs: self.skip_certs,
            },
            self.candidate_payloads,
        )
    }

    /// Check if this is a fresh start (no persisted state)
    pub fn is_empty(&self) -> bool {
        self.finalized_blocks.is_empty()
            && self.candidate_infos.is_empty()
            && self.notar_certs.is_empty()
            && self.final_certs.is_empty()
            && self.skip_certs.is_empty()
            && self.votes.is_empty()
            && self.pool_state.is_none()
            && self.candidate_payloads.is_empty()
    }
}

// ============================================================================
// TL Conversion Helpers
// ============================================================================

/// Convert RawCandidateId to TL CandidateId
fn raw_candidate_id_to_tl(id: &RawCandidateId) -> CandidateId {
    CandidateId { slot: id.slot.value() as i32, hash: id.hash.clone().into() }
}

/// Convert TL CandidateId to RawCandidateId
fn raw_candidate_id_from_tl(tl: CandidateId) -> RawCandidateId {
    RawCandidateId { slot: SlotIndex::new(tl.slot as u32), hash: tl.hash }
}

// ============================================================================
// Serialization Functions
// ============================================================================

fn serialize_finalized_block_key(candidate_id: &RawCandidateId) -> Result<Vec<u8>> {
    let key = FinalizedBlockKey { candidateId: raw_candidate_id_to_tl(candidate_id) };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_finalized_block_value(record: &FinalizedBlockRecord) -> Result<Vec<u8>> {
    let block_id = record.block_id.clone();
    let parent = match &record.parent {
        Some(p) => CandidateParent { id: raw_candidate_id_to_tl(p).into_boxed() }.into_boxed(),
        None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
    };
    let value = FinalizedBlockValue {
        block_id,
        parent,
        is_final: if record.is_final { Bool::BoolTrue } else { Bool::BoolFalse },
    };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_finalized_block(
    key_bytes: &[u8],
    value_bytes: &[u8],
) -> Result<FinalizedBlockRecord> {
    let key: FinalizedBlockKey = deserialize_typed::<FinalizedBlockKeyBoxed>(key_bytes)?.only();
    let candidate_id = raw_candidate_id_from_tl(key.candidateId);
    let value: FinalizedBlockValue =
        deserialize_typed::<FinalizedBlockValueBoxed>(value_bytes)?.only();
    let block_id = value.block_id;
    let parent = match value.parent {
        CandidateParentBoxed::Consensus_CandidateParent(p) => {
            Some(raw_candidate_id_from_tl(p.id.only()))
        }
        CandidateParentBoxed::Consensus_CandidateWithoutParents => None,
    };
    let is_final = matches!(value.is_final, Bool::BoolTrue);
    Ok(FinalizedBlockRecord { candidate_id, block_id, parent, is_final })
}

fn serialize_candidate_info_key(candidate_id: &RawCandidateId) -> Result<Vec<u8>> {
    let key = CandidateInfoKey { candidateId: raw_candidate_id_to_tl(candidate_id) };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_candidate_info_value(record: &CandidateInfoRecord) -> Result<Vec<u8>> {
    let value = CandidateInfoValue {
        leader_id: record.leader_idx as i32,
        candidate_hash_data: record.candidate_hash_data.clone(),
        signature: record.signature.clone(),
    };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_candidate_info(key_bytes: &[u8], value_bytes: &[u8]) -> Result<CandidateInfoRecord> {
    let key: CandidateInfoKey = deserialize_typed::<CandidateInfoKeyBoxed>(key_bytes)?.only();
    let candidate_id = raw_candidate_id_from_tl(key.candidateId);
    let value: CandidateInfoValue =
        deserialize_typed::<CandidateInfoValueBoxed>(value_bytes)?.only();
    Ok(CandidateInfoRecord {
        candidate_id,
        leader_idx: value.leader_id as u32,
        candidate_hash_data: value.candidate_hash_data,
        signature: value.signature,
    })
}

fn serialize_candidate_payload_key(candidate_id: &RawCandidateId) -> Result<Vec<u8>> {
    let key = CandidatePayloadKey { candidateId: raw_candidate_id_to_tl(candidate_id) };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_candidate_payload_key(key_bytes: &[u8]) -> Result<RawCandidateId> {
    let key: CandidatePayloadKey = deserialize_typed::<CandidatePayloadKeyBoxed>(key_bytes)?.only();
    Ok(raw_candidate_id_from_tl(key.candidateId))
}

fn serialize_vote_key(vote_hash: &UInt256) -> Result<Vec<u8>> {
    let key = VoteKey { vote_hash: vote_hash.clone().into() };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_vote_value(record: &VoteRecord) -> Result<Vec<u8>> {
    let value = VoteValue {
        data: record.data.clone(),
        node_idx: record.node_idx.value() as i32,
        seqno: record.seqno,
    };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_cert_vote_entry(cert_tl: TlCertificateBoxed) -> Result<(Vec<u8>, Vec<u8>)> {
    let cert = cert_tl.only();
    let cert_boxed = cert.clone().into_boxed();
    let cert_bytes = serialize_boxed(&cert_boxed)
        .map_err(|e| error!("Failed to serialize certificate: {}", e))?;
    let cert_hash = UInt256::from_slice(&sha256_digest(&cert_bytes));
    let key = serialize_vote_key(&cert_hash)?;
    let value = CertValue { cert };
    let value_bytes = serialize_boxed(&value.into_boxed())
        .map_err(|e| error!("Failed to serialize db.cert value: {}", e))?;
    Ok((key, value_bytes))
}

enum VoteStorageEntry {
    Vote(VoteRecord),
    NotarCert(NotarCertRecord),
    FinalCert(FinalCertRecord),
    SkipCert(SkipCertRecord),
}

fn deserialize_cert_vote_entry(cert_tl: TlCertificate) -> Result<VoteStorageEntry> {
    let cert_boxed = cert_tl.clone().into_boxed();
    let cert_bytes = serialize_boxed(&cert_boxed)
        .map_err(|e| error!("Failed to serialize certificate: {}", e))?;

    match crate::utils::tl_unsigned_to_vote(cert_boxed.vote())? {
        crate::simplex_state::Vote::Notarize(vote) => {
            let notar_cert_bytes = serialize_boxed(cert_boxed.signatures())
                .map_err(|e| error!("Failed to serialize notar signatures: {}", e))?;
            Ok(VoteStorageEntry::NotarCert(NotarCertRecord {
                candidate_id: RawCandidateId { slot: vote.slot, hash: vote.block_hash },
                notar_cert_bytes: notar_cert_bytes.into(),
            }))
        }
        crate::simplex_state::Vote::Finalize(vote) => {
            Ok(VoteStorageEntry::FinalCert(FinalCertRecord {
                candidate_id: RawCandidateId { slot: vote.slot, hash: vote.block_hash },
                cert_bytes: cert_bytes.into(),
            }))
        }
        crate::simplex_state::Vote::Skip(vote) => Ok(VoteStorageEntry::SkipCert(SkipCertRecord {
            slot: vote.slot,
            cert_bytes: cert_bytes.into(),
        })),
    }
}

fn deserialize_vote_storage_entry(
    key_bytes: &[u8],
    value_bytes: &[u8],
) -> Result<VoteStorageEntry> {
    let key: VoteKey = deserialize_typed::<VoteKeyBoxed>(key_bytes)?.only();
    let value: VoteValueBoxed = deserialize_typed::<VoteValueBoxed>(value_bytes)?;
    match value {
        VoteValueBoxed::Consensus_Simplex_Db_Vote(value) => {
            Ok(VoteStorageEntry::Vote(VoteRecord {
                vote_hash: key.vote_hash.clone(),
                data: value.data,
                node_idx: ValidatorIndex::new(value.node_idx as u32),
                seqno: value.seqno,
            }))
        }
        VoteValueBoxed::Consensus_Simplex_Db_Cert(value) => deserialize_cert_vote_entry(value.cert),
    }
}

fn deserialize_vote(key_bytes: &[u8], value_bytes: &[u8]) -> Result<Option<VoteRecord>> {
    match deserialize_vote_storage_entry(key_bytes, value_bytes)? {
        VoteStorageEntry::Vote(vote) => Ok(Some(vote)),
        VoteStorageEntry::NotarCert(_)
        | VoteStorageEntry::FinalCert(_)
        | VoteStorageEntry::SkipCert(_) => Ok(None),
    }
}

fn deserialize_notar_cert(key_bytes: &[u8], value_bytes: &[u8]) -> Result<Option<NotarCertRecord>> {
    match deserialize_vote_storage_entry(key_bytes, value_bytes)? {
        VoteStorageEntry::NotarCert(record) => Ok(Some(record)),
        VoteStorageEntry::Vote(_)
        | VoteStorageEntry::FinalCert(_)
        | VoteStorageEntry::SkipCert(_) => Ok(None),
    }
}

fn deserialize_final_cert(key_bytes: &[u8], value_bytes: &[u8]) -> Result<Option<FinalCertRecord>> {
    match deserialize_vote_storage_entry(key_bytes, value_bytes)? {
        VoteStorageEntry::FinalCert(record) => Ok(Some(record)),
        VoteStorageEntry::Vote(_)
        | VoteStorageEntry::NotarCert(_)
        | VoteStorageEntry::SkipCert(_) => Ok(None),
    }
}

fn deserialize_skip_cert(key_bytes: &[u8], value_bytes: &[u8]) -> Result<Option<SkipCertRecord>> {
    match deserialize_vote_storage_entry(key_bytes, value_bytes)? {
        VoteStorageEntry::SkipCert(record) => Ok(Some(record)),
        VoteStorageEntry::Vote(_)
        | VoteStorageEntry::NotarCert(_)
        | VoteStorageEntry::FinalCert(_) => Ok(None),
    }
}

fn serialize_pool_state_key() -> Result<Vec<u8>> {
    serialize_boxed(&PoolStateKey::default()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_pool_state_value(record: &PoolStateRecord) -> Result<Vec<u8>> {
    let value = PoolStateValue {
        first_nonannounced_window: record.first_nonannounced_window.value() as i32,
    };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_pool_state(value_bytes: &[u8]) -> Result<PoolStateRecord> {
    let value: PoolStateValue = deserialize_typed::<PoolStateValueBoxed>(value_bytes)?.only();
    Ok(PoolStateRecord {
        first_nonannounced_window: WindowIndex::new(value.first_nonannounced_window as u32),
    })
}

/// Filters finalized blocks to keep only a consistent parent chain prefix.
///
/// This matches the intent of C++ `consensus.cpp::load_from_db()` which drops
/// records if parent links do not match the expected chain.
///
/// Assumes `records` are sorted by slot ascending.
fn filter_finalized_chain(mut records: Vec<FinalizedBlockRecord>) -> Vec<FinalizedBlockRecord> {
    let mut filtered = Vec::with_capacity(records.len());
    let mut expected_parent: Option<RawCandidateId> = None;

    for record in records.drain(..) {
        match &expected_parent {
            None => {
                if record.parent.is_some() {
                    // No chain root yet (expected no parent), skip this record.
                    log::warn!(
                        target: TARGET,
                        "SimplexDb: skipping finalized block slot={} (no chain root, parent is set)",
                        record.candidate_id.slot.value()
                    );
                    continue;
                }
            }
            Some(expected) => {
                if record.parent.as_ref() != Some(expected) {
                    log::warn!(
                        target: TARGET,
                        "SimplexDb: skipping finalized block slot={} (parent mismatch)",
                        record.candidate_id.slot.value()
                    );
                    continue;
                }
            }
        }

        expected_parent = Some(record.candidate_id.clone());
        filtered.push(record);
    }

    filtered
}

// ============================================================================
// SimplexDb
// ============================================================================

/// Pointer to SimplexDb
pub type SimplexDbPtr = Arc<SimplexDb>;

/// Simplex database for persistent storage.
///
/// All operations are async via `AsyncKeyValueStorage`.
/// Fire-and-forget writes (like C++ `.start().detach()`), blocking reads (startup only).
///
pub struct SimplexDb {
    /// Underlying async key-value storage
    storage: AsyncKeyValueStoragePtr,
    /// Storage ID (for logging)
    storage_id: String,
    /// Monotonic vote seqno counter (C++ parity: db.cpp next_seqno_)
    next_vote_seqno: AtomicI64,
    #[cfg(test)]
    /// Fail-next hook for notar cert save path (tests only).
    fail_next_notar_cert_save: AtomicBool,
    #[cfg(test)]
    /// Fail-next hook for finalized block save path (tests only).
    fail_next_finalized_block_save: AtomicBool,
}

impl SimplexDb {
    /// Create new SimplexDb at the given path.
    ///
    /// Blocks until DB is opened.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Full path to database directory
    /// * `storage_id` - Identifier for logging and thread naming (e.g., session_id hex)
    /// * `use_callback_thread` - Whether to use separate callback thread
    ///
    pub fn open(db_path: impl AsRef<Path>, storage_id: &str) -> Result<SimplexDbPtr> {
        let db_path = db_path.as_ref();
        let storage_id = storage_id.to_string();

        log::info!(
            target: TARGET,
            "SimplexDb {}: opening at {}",
            storage_id,
            db_path.display()
        );

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                error!(
                    "SimplexDb {}: failed to create parent dir {}: {}",
                    storage_id,
                    parent.display(),
                    e
                )
            })?;
        }

        // SimplexDb does not use callbacks
        let options = AsyncKeyValueStorageOptions { use_callback_thread: false };

        let storage =
            ConsensusCommonFactory::create_async_key_value_storage(db_path, &storage_id, options)?;

        log::info!(
            target: TARGET,
            "SimplexDb {}: opened at {}",
            storage_id,
            db_path.display()
        );

        Ok(Arc::new(Self {
            storage,
            storage_id,
            next_vote_seqno: AtomicI64::new(0),
            #[cfg(test)]
            fail_next_notar_cert_save: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_finalized_block_save: AtomicBool::new(false),
        }))
    }

    #[cfg(test)]
    /// Inject one-shot failure for `save_notar_cert_async`.
    pub fn fail_next_notar_cert_save_for_test(&self) {
        self.fail_next_notar_cert_save.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    /// Inject one-shot failure for `save_finalized_block_async`.
    pub fn fail_next_finalized_block_save_for_test(&self) {
        self.fail_next_finalized_block_save.store(true, Ordering::SeqCst);
    }

    // =========================================================================
    // Write Operations (fire-and-forget)
    // =========================================================================

    /// Save finalized block record (async result).
    ///
    /// Use this to wait for the write completion when needed.
    pub fn save_finalized_block_async(
        &self,
        record: &FinalizedBlockRecord,
    ) -> Result<StorageAsyncResultPtr<()>> {
        #[cfg(test)]
        if self.fail_next_finalized_block_save.swap(false, Ordering::SeqCst) {
            return Err(error!(
                "SimplexDb {}: injected finalized block save failure",
                self.storage_id
            ));
        }

        let key = serialize_finalized_block_key(&record.candidate_id)?;
        let value = serialize_finalized_block_value(record)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save finalized block record.
    ///
    /// Called when a block is finalized or notarized with a certificate.
    pub fn save_finalized_block(&self, record: &FinalizedBlockRecord) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_finalized_block slot={} is_final={}",
            self.storage_id,
            record.candidate_id.slot.value(),
            record.is_final
        );

        self.save_finalized_block_async(record)?;
        Ok(())
    }

    /// Save candidate info record (async result).
    pub fn save_candidate_info_async(
        &self,
        record: &CandidateInfoRecord,
    ) -> Result<StorageAsyncResultPtr<()>> {
        let key = serialize_candidate_info_key(&record.candidate_id)?;
        let value = serialize_candidate_info_value(record)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save candidate info record.
    ///
    /// Called when a candidate is received or generated.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn save_candidate_info(&self, record: &CandidateInfoRecord) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_candidate_info slot={} leader={}",
            self.storage_id,
            record.candidate_id.slot.value(),
            record.leader_idx
        );

        self.save_candidate_info_async(record)?;
        Ok(())
    }

    /// Save notarization certificate (async result).
    pub fn save_notar_cert_async(
        &self,
        _candidate_id: &RawCandidateId,
        cert: &NotarCert,
    ) -> Result<StorageAsyncResultPtr<()>> {
        #[cfg(test)]
        if self.fail_next_notar_cert_save.swap(false, Ordering::SeqCst) {
            return Err(error!("SimplexDb {}: injected notar cert save failure", self.storage_id));
        }

        let cert_tl = cert.to_tl()?;
        let (key, value) = serialize_cert_vote_entry(cert_tl)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save notarization certificate.
    ///
    /// Called when notarization is observed.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn save_notar_cert(&self, candidate_id: &RawCandidateId, cert: &NotarCert) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_notar_cert slot={} signatures={}",
            self.storage_id,
            candidate_id.slot.value(),
            cert.signatures.len()
        );

        self.save_notar_cert_async(candidate_id, cert)?;
        Ok(())
    }

    /// Save finalization certificate (async result).
    pub fn save_final_cert_async(
        &self,
        _candidate_id: &RawCandidateId,
        cert: &FinalCert,
    ) -> Result<StorageAsyncResultPtr<()>> {
        let cert_tl = cert.to_tl()?;
        let (key, value) = serialize_cert_vote_entry(cert_tl)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save finalization certificate.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn save_final_cert(&self, candidate_id: &RawCandidateId, cert: &FinalCert) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_final_cert slot={} signatures={}",
            self.storage_id,
            candidate_id.slot.value(),
            cert.signatures.len()
        );
        self.save_final_cert_async(candidate_id, cert)?;
        Ok(())
    }

    /// Save skip certificate (async result).
    pub fn save_skip_cert_async(
        &self,
        _slot: SlotIndex,
        cert: &SkipCert,
    ) -> Result<StorageAsyncResultPtr<()>> {
        let cert_tl = cert.to_tl()?;
        let (key, value) = serialize_cert_vote_entry(cert_tl)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save skip certificate.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn save_skip_cert(&self, slot: SlotIndex, cert: &SkipCert) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_skip_cert slot={} signatures={}",
            self.storage_id,
            slot.value(),
            cert.signatures.len()
        );
        self.save_skip_cert_async(slot, cert)?;
        Ok(())
    }

    /// Save vote record (async result).
    ///
    /// Assigns a monotonic seqno for replay ordering (C++ parity: db.cpp next_seqno_++).
    pub fn save_vote_async(&self, record: &VoteRecord) -> Result<StorageAsyncResultPtr<()>> {
        let seqno = self.next_vote_seqno.fetch_add(1, Ordering::Relaxed);
        let mut record_with_seqno = record.clone();
        record_with_seqno.seqno = seqno;
        let key = serialize_vote_key(&record_with_seqno.vote_hash)?;
        let value = serialize_vote_value(&record_with_seqno)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save vote record.
    ///
    /// Called when a vote is received (for standstill recovery).
    #[allow(dead_code)] // Convenience wrapper; prefer `_async()` in production code.
    pub fn save_vote(&self, record: &VoteRecord) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_vote hash={} node_idx={}",
            self.storage_id,
            hex::encode(&record.vote_hash.as_slice()[..8]),
            record.node_idx
        );

        self.save_vote_async(record)?;
        Ok(())
    }

    /// Save pool state record (async result).
    pub fn save_pool_state_async(
        &self,
        record: &PoolStateRecord,
    ) -> Result<StorageAsyncResultPtr<()>> {
        let key = serialize_pool_state_key()?;
        let value = serialize_pool_state_value(record)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save pool state record.
    ///
    /// Called periodically or on state change.
    #[allow(dead_code)] // Convenience wrapper; prefer `_async()` in production code.
    pub fn save_pool_state(&self, record: &PoolStateRecord) -> Result<()> {
        log::trace!(
            target: TARGET,
            "SimplexDb {}: save_pool_state first_nonannounced_window={}",
            self.storage_id,
            record.first_nonannounced_window
        );

        self.save_pool_state_async(record)?;
        Ok(())
    }

    // =========================================================================
    // Candidate Payload Storage (C++ CandidateResolver::store_candidate parity)
    // =========================================================================

    /// Save serialized CandidateData bytes (async, fire-and-forget).
    ///
    /// C++ parity: `candidate-resolver.cpp store_candidate()` persists the full
    /// serialized candidate so `requestCandidate(want_candidate=true)` queries
    /// can be served from DB after restart.
    pub fn save_candidate_payload_async(
        &self,
        candidate_id: &RawCandidateId,
        candidate_data_bytes: &[u8],
    ) -> Result<StorageAsyncResultPtr<()>> {
        let key = serialize_candidate_payload_key(candidate_id)?;
        Ok(self.storage.set(key, candidate_data_bytes.to_vec(), None))
    }

    /// Load a single candidate payload by id (blocking, for query fallback).
    pub fn load_candidate_payload_by_id(
        &self,
        candidate_id: &RawCandidateId,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>> {
        let key = serialize_candidate_payload_key(candidate_id)?;
        let result = self.storage.get(key, None);
        match result.wait_timeout(timeout) {
            Some(Ok(Some(value))) => Ok(Some(value)),
            Some(Ok(None)) => Ok(None),
            Some(Err(e)) => Err(e),
            None => Err(error!("SimplexDb: timeout loading candidate payload by id")),
        }
    }

    /// Load all candidate payloads asynchronously (for startup restore).
    pub fn load_candidate_payloads_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_candidate_payloads_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_candidate_payload(), None)
    }

    // =========================================================================
    // Single-Record Lookups (async, for live query fallback)
    // =========================================================================

    /// Look up a single candidate info record by candidate ID (blocking).
    ///
    /// Unlike `load_candidate_infos()` which scans all records, this looks up
    /// a single record by its exact key. Used by the RequestCandidate fallback
    /// when resolver_cache misses.
    ///
    /// Reference: C++ candidate-resolver.cpp `try_load_candidate_data_from_db()`
    pub fn load_candidate_info_by_id(
        &self,
        candidate_id: &RawCandidateId,
        timeout: Duration,
    ) -> Result<Option<CandidateInfoRecord>> {
        let key = serialize_candidate_info_key(candidate_id)?;
        let result = self.storage.get(key.clone(), None);
        match result.wait_timeout(timeout) {
            Some(Ok(Some(value))) => {
                let record = deserialize_candidate_info(&key, &value)?;
                Ok(Some(record))
            }
            Some(Ok(None)) => Ok(None),
            Some(Err(e)) => Err(e),
            None => Err(error!("SimplexDb: timeout loading candidate info by id")),
        }
    }

    /// Look up a single notar cert record by candidate ID (blocking).
    ///
    /// Used by the RequestCandidate fallback for notar_cert recovery.
    pub fn load_notar_cert_by_id(
        &self,
        candidate_id: &RawCandidateId,
        timeout: Duration,
    ) -> Result<Option<NotarCertRecord>> {
        let result = self.storage.get_by_prefix_u32(prefix_vote(), None);
        match result.wait_timeout(timeout) {
            Some(Ok(entries)) => {
                for (key, value) in entries {
                    match deserialize_notar_cert(&key, &value) {
                        Ok(Some(record)) if record.candidate_id == *candidate_id => {
                            return Ok(Some(record));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log::debug!(
                                target: TARGET,
                                "SimplexDb {}: skipping malformed cert vote entry: {}",
                                self.storage_id,
                                e
                            );
                        }
                    }
                }
                Ok(None)
            }
            Some(Err(e)) => Err(e),
            None => Err(error!("SimplexDb: timeout loading notar cert by id")),
        }
    }

    /// Look up a single final cert record by candidate ID (blocking).
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_session_processor.rs`.
    pub fn load_final_cert_by_id(
        &self,
        candidate_id: &RawCandidateId,
        timeout: Duration,
    ) -> Result<Option<FinalCertRecord>> {
        let result = self.storage.get_by_prefix_u32(prefix_vote(), None);
        match result.wait_timeout(timeout) {
            Some(Ok(entries)) => {
                for (key, value) in entries {
                    match deserialize_final_cert(&key, &value) {
                        Ok(Some(record)) if record.candidate_id == *candidate_id => {
                            return Ok(Some(record));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log::debug!(
                                target: TARGET,
                                "SimplexDb {}: skipping malformed cert vote entry: {}",
                                self.storage_id,
                                e
                            );
                        }
                    }
                }
                Ok(None)
            }
            Some(Err(e)) => Err(e),
            None => Err(error!("SimplexDb: timeout loading final cert by id")),
        }
    }

    /// Look up a single skip cert record by slot (blocking).
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_session_processor.rs`.
    pub fn load_skip_cert_by_slot(
        &self,
        slot: SlotIndex,
        timeout: Duration,
    ) -> Result<Option<SkipCertRecord>> {
        let result = self.storage.get_by_prefix_u32(prefix_vote(), None);
        match result.wait_timeout(timeout) {
            Some(Ok(entries)) => {
                for (key, value) in entries {
                    match deserialize_skip_cert(&key, &value) {
                        Ok(Some(record)) if record.slot == slot => return Ok(Some(record)),
                        Ok(_) => {}
                        Err(e) => {
                            log::debug!(
                                target: TARGET,
                                "SimplexDb {}: skipping malformed cert vote entry: {}",
                                self.storage_id,
                                e
                            );
                        }
                    }
                }
                Ok(None)
            }
            Some(Err(e)) => Err(e),
            None => Err(error!("SimplexDb: timeout loading skip cert by slot")),
        }
    }

    // =========================================================================
    // Async Read Operations (for cancellable bootstrap)
    // =========================================================================

    /// Load all finalized blocks asynchronously.
    ///
    /// Returns immediately with an async result that can be waited on
    /// with cancellation support via `wait_cancellable()`.
    pub fn load_finalized_blocks_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_finalized_blocks_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_finalized_block(), None)
    }

    /// Load all candidate infos asynchronously.
    pub fn load_candidate_infos_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_candidate_infos_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_candidate_info(), None)
    }

    /// Load all notar certs asynchronously.
    pub fn load_notar_certs_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_notar_certs_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_vote(), None)
    }

    /// Load all final certs asynchronously.
    pub fn load_final_certs_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_final_certs_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_vote(), None)
    }

    /// Load all skip certs asynchronously.
    pub fn load_skip_certs_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_skip_certs_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_vote(), None)
    }

    /// Load all votes asynchronously.
    pub fn load_votes_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_votes_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_vote(), None)
    }

    /// Load pool state asynchronously.
    ///
    /// Returns raw key-value pairs; caller deserializes.
    pub fn load_pool_state_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_pool_state_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_pool_state(), None)
    }

    // =========================================================================
    // Read Operations (blocking, startup only)
    // =========================================================================

    /// Load all finalized blocks from DB, sorted by slot.
    ///
    /// Called at session startup to restore state.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn load_finalized_blocks(&self) -> Result<Vec<FinalizedBlockRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_finalized_blocks",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_finalized_block(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading finalized blocks"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_finalized_block(&key_bytes, &value_bytes) {
                Ok(record) => records.push(record),
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize finalized block: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        // Sort by slot
        records.sort_by_key(|r| r.candidate_id.slot);

        // Filter to a consistent chain prefix (important for restart correctness)
        let total = records.len();
        let records = filter_finalized_chain(records);
        let kept = records.len();

        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} finalized blocks (kept {} after chain filter)",
            self.storage_id,
            total,
            kept
        );

        Ok(records)
    }

    /// Load all candidate info records from DB.
    ///
    /// Called at session startup to restore candidate cache.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn load_candidate_infos(&self) -> Result<Vec<CandidateInfoRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_candidate_infos",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_candidate_info(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading candidate infos"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_candidate_info(&key_bytes, &value_bytes) {
                Ok(record) => records.push(record),
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize candidate info: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} candidate infos",
            self.storage_id,
            records.len()
        );

        Ok(records)
    }

    /// Load all notarization certificates from DB.
    ///
    /// Called at session startup to restore certificate cache.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn load_notar_certs(&self) -> Result<Vec<NotarCertRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_notar_certs",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_vote(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading notar certs"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_notar_cert(&key_bytes, &value_bytes) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize notar cert: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }
        records.sort_by_key(|r| r.candidate_id.slot);

        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} notar certs",
            self.storage_id,
            records.len()
        );

        Ok(records)
    }

    /// Load all finalization certificates from DB.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn load_final_certs(&self) -> Result<Vec<FinalCertRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_final_certs",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_vote(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading final certs"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_final_cert(&key_bytes, &value_bytes) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize final cert: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        records.sort_by_key(|r| r.candidate_id.slot);
        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} final certs",
            self.storage_id,
            records.len()
        );
        Ok(records)
    }

    /// Load all skip certificates from DB.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn load_skip_certs(&self) -> Result<Vec<SkipCertRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_skip_certs",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_vote(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading skip certs"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_skip_cert(&key_bytes, &value_bytes) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize skip cert: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        records.sort_by_key(|r| r.slot);
        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} skip certs",
            self.storage_id,
            records.len()
        );
        Ok(records)
    }

    /// Load all votes from DB.
    ///
    /// Called at session startup for standstill recovery.
    #[allow(dead_code)] // Not used yet; kept for restart/debug parity.
    pub fn load_votes(&self) -> Result<Vec<VoteRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_votes",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_vote(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading votes"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_vote(&key_bytes, &value_bytes) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(e) => {
                    log::error!(
                        target: TARGET,
                        "SimplexDb {}: failed to deserialize vote: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        // C++ parity: sort by seqno for deterministic replay order (db.cpp init_votes)
        records.sort_by_key(|r| r.seqno);

        // Initialize next_vote_seqno from max seqno + 1 (C++ parity: db.cpp next_seqno_)
        if let Some(max_seqno) = records.last().map(|r| r.seqno) {
            self.next_vote_seqno.store(max_seqno + 1, Ordering::Relaxed);
        }

        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded {} votes (next_seqno={})",
            self.storage_id,
            records.len(),
            self.next_vote_seqno.load(Ordering::Relaxed)
        );

        Ok(records)
    }

    /// Load pool state from DB.
    ///
    /// Returns None if no pool state was saved (first run).
    #[allow(dead_code)] // Not used yet; kept for restart/debug parity.
    pub fn load_pool_state(&self) -> Result<Option<PoolStateRecord>> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: load_pool_state",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_pool_state(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading pool state"))??;

        if result.is_empty() {
            log::info!(
                target: TARGET,
                "SimplexDb {}: no pool state found (first run)",
                self.storage_id
            );
            return Ok(None);
        }

        // Should be exactly one record (singleton)
        if result.len() > 1 {
            log::warn!(
                target: TARGET,
                "SimplexDb {}: multiple pool state records found ({}), using first",
                self.storage_id,
                result.len()
            );
        }

        let (_key_bytes, value_bytes) = &result[0];
        let record = deserialize_pool_state(value_bytes)?;

        log::info!(
            target: TARGET,
            "SimplexDb {}: loaded pool state first_nonannounced_window={}",
            self.storage_id,
            record.first_nonannounced_window
        );

        Ok(Some(record))
    }

    /// Load complete bootstrap data from DB.
    ///
    /// Loads all persistent state needed for session restart:
    /// - Finalized blocks
    /// - Candidate infos
    /// - Notarization certificates
    /// - Finalization certificates
    /// - Skip certificates
    /// - Votes
    /// - Pool state
    ///
    /// Use `Bootstrap::split()` to decompose into SessionBootstrap and ReceiverBootstrap.
    #[allow(dead_code)] // Prefer `load_bootstrap_cancellable()` in session startup.
    pub fn load_bootstrap(&self) -> Result<Bootstrap> {
        log::info!(
            target: TARGET,
            "SimplexDb {}: load_bootstrap",
            self.storage_id
        );

        let finalized_blocks = self.load_finalized_blocks()?;
        let candidate_infos = self.load_candidate_infos()?;
        let notar_certs = self.load_notar_certs()?;
        let final_certs = self.load_final_certs()?;
        let skip_certs = self.load_skip_certs()?;
        let votes = self.load_votes()?;
        let pool_state = self.load_pool_state()?;

        // Load candidate payloads (optional, graceful if absent)
        let payloads_raw = self
            .load_candidate_payloads_async()
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading candidate payloads"))??;
        let mut candidate_payloads = Vec::with_capacity(payloads_raw.len());
        for (k, v) in payloads_raw {
            match deserialize_candidate_payload_key(&k) {
                Ok(id) => candidate_payloads.push((id, v)),
                Err(e) => {
                    log::error!(target: TARGET, "SimplexDb: skip bad candidate payload key: {e}")
                }
            }
        }

        let bootstrap = Bootstrap {
            finalized_blocks,
            candidate_infos,
            notar_certs,
            final_certs,
            skip_certs,
            votes,
            pool_state,
            candidate_payloads,
        };

        log::info!(
            target: TARGET,
            "SimplexDb {}: bootstrap loaded: {} finalized, {} candidates, {} notar certs, {} \
            final certs, {} skip certs, {} votes, {} payloads, pool_state={}",
            self.storage_id,
            bootstrap.finalized_blocks.len(),
            bootstrap.candidate_infos.len(),
            bootstrap.notar_certs.len(),
            bootstrap.final_certs.len(),
            bootstrap.skip_certs.len(),
            bootstrap.votes.len(),
            bootstrap.candidate_payloads.len(),
            bootstrap.pool_state.is_some()
        );

        Ok(bootstrap)
    }

    /// Load complete bootstrap data with cancellation support.
    ///
    /// Uses async loads with cancellable waits for graceful shutdown during startup.
    ///
    /// # Arguments
    /// * `cancel` - Cancellation flag (e.g., `Arc<AtomicBool>` for stop_requested)
    /// * `step` - Polling interval for cancellation checks
    pub fn load_bootstrap_cancellable(
        &self,
        cancel: &dyn consensus_common::Cancellable,
        step: Duration,
    ) -> Result<Bootstrap> {
        log::info!(
            target: TARGET,
            "SimplexDb {}: load_bootstrap_cancellable",
            self.storage_id
        );

        // Start all async loads in parallel
        let finalized_async = self.load_finalized_blocks_async();
        let candidates_async = self.load_candidate_infos_async();
        let notar_certs_async = self.load_notar_certs_async();
        let final_certs_async = self.load_final_certs_async();
        let skip_certs_async = self.load_skip_certs_async();
        let votes_async = self.load_votes_async();
        let pool_state_async = self.load_pool_state_async();
        let payloads_async = self.load_candidate_payloads_async();

        // Wait with cancellation support
        let finalized_raw = finalized_async.wait_cancellable(cancel, step)?;
        let candidates_raw = candidates_async.wait_cancellable(cancel, step)?;
        let notar_certs_raw = notar_certs_async.wait_cancellable(cancel, step)?;
        let final_certs_raw = final_certs_async.wait_cancellable(cancel, step)?;
        let skip_certs_raw = skip_certs_async.wait_cancellable(cancel, step)?;
        let votes_raw = votes_async.wait_cancellable(cancel, step)?;
        let pool_state_raw = pool_state_async.wait_cancellable(cancel, step)?;
        let payloads_raw = payloads_async.wait_cancellable(cancel, step)?;

        // Deserialize results
        let mut finalized_blocks = Vec::with_capacity(finalized_raw.len());
        for (k, v) in finalized_raw {
            match deserialize_finalized_block(&k, &v) {
                Ok(r) => finalized_blocks.push(r),
                Err(e) => {
                    log::error!(target: TARGET, "SimplexDb: skip bad finalized block: {e}")
                }
            }
        }
        finalized_blocks.sort_by_key(|r| r.candidate_id.slot);
        let total_finalized_blocks = finalized_blocks.len();
        let finalized_blocks = filter_finalized_chain(finalized_blocks);
        if finalized_blocks.len() != total_finalized_blocks {
            log::warn!(
                target: TARGET,
                "SimplexDb {}: finalized blocks chain filter dropped {} records",
                self.storage_id,
                total_finalized_blocks - finalized_blocks.len()
            );
        }

        let mut candidate_infos = Vec::with_capacity(candidates_raw.len());
        for (k, v) in candidates_raw {
            match deserialize_candidate_info(&k, &v) {
                Ok(r) => candidate_infos.push(r),
                Err(e) => {
                    log::error!(target: TARGET, "SimplexDb: skip bad candidate info: {e}")
                }
            }
        }

        let mut notar_certs = Vec::with_capacity(notar_certs_raw.len());
        for (k, v) in notar_certs_raw {
            match deserialize_notar_cert(&k, &v) {
                Ok(Some(r)) => notar_certs.push(r),
                Ok(None) => {}
                Err(e) => log::error!(target: TARGET, "SimplexDb: skip bad notar cert: {e}"),
            }
        }
        notar_certs.sort_by_key(|r| r.candidate_id.slot);

        let mut final_certs = Vec::with_capacity(final_certs_raw.len());
        for (k, v) in final_certs_raw {
            match deserialize_final_cert(&k, &v) {
                Ok(Some(r)) => final_certs.push(r),
                Ok(None) => {}
                Err(e) => log::error!(target: TARGET, "SimplexDb: skip bad final cert: {e}"),
            }
        }
        final_certs.sort_by_key(|r| r.candidate_id.slot);

        let mut skip_certs = Vec::with_capacity(skip_certs_raw.len());
        for (k, v) in skip_certs_raw {
            match deserialize_skip_cert(&k, &v) {
                Ok(Some(r)) => skip_certs.push(r),
                Ok(None) => {}
                Err(e) => log::error!(target: TARGET, "SimplexDb: skip bad skip cert: {e}"),
            }
        }
        skip_certs.sort_by_key(|r| r.slot);

        let mut votes = Vec::with_capacity(votes_raw.len());
        for (k, v) in votes_raw {
            match deserialize_vote(&k, &v) {
                Ok(Some(r)) => votes.push(r),
                Ok(None) => {}
                Err(e) => log::error!(target: TARGET, "SimplexDb: skip bad vote: {e}"),
            }
        }

        // C++ parity: sort by seqno for deterministic replay order (db.cpp init_votes)
        votes.sort_by_key(|r| r.seqno);

        // Initialize next_vote_seqno from max seqno + 1 (C++ parity: db.cpp next_seqno_)
        if let Some(max_seqno) = votes.last().map(|r| r.seqno) {
            self.next_vote_seqno.store(max_seqno + 1, Ordering::Relaxed);
        }

        let pool_state = if pool_state_raw.is_empty() {
            None
        } else {
            let (_k, v) = &pool_state_raw[0];
            match deserialize_pool_state(v) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::error!(target: TARGET, "SimplexDb: skip bad pool state: {e}");
                    None
                }
            }
        };

        let mut candidate_payloads = Vec::with_capacity(payloads_raw.len());
        for (k, v) in payloads_raw {
            match deserialize_candidate_payload_key(&k) {
                Ok(id) => candidate_payloads.push((id, v)),
                Err(e) => {
                    log::error!(target: TARGET, "SimplexDb: skip bad candidate payload key: {e}")
                }
            }
        }

        let bootstrap = Bootstrap {
            finalized_blocks,
            candidate_infos,
            notar_certs,
            final_certs,
            skip_certs,
            votes,
            pool_state,
            candidate_payloads,
        };

        log::info!(
            target: TARGET,
            "SimplexDb {}: bootstrap loaded: {} finalized, {} candidates, {} notar certs, {} \
            final certs, {} skip certs, {} votes, {} payloads, pool_state={}",
            self.storage_id,
            bootstrap.finalized_blocks.len(),
            bootstrap.candidate_infos.len(),
            bootstrap.notar_certs.len(),
            bootstrap.final_certs.len(),
            bootstrap.skip_certs.len(),
            bootstrap.votes.len(),
            bootstrap.candidate_payloads.len(),
            bootstrap.pool_state.is_some()
        );

        Ok(bootstrap)
    }

    // =========================================================================
    // Sync and Lifecycle
    // =========================================================================

    /// Wait for all pending writes to complete.
    pub fn sync(&self, timeout: Option<Duration>) -> Result<()> {
        log::debug!(
            target: TARGET,
            "SimplexDb {}: sync",
            self.storage_id
        );
        self.storage.sync(timeout)
    }

    /// Mark database for destruction on drop.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn mark_for_destroy(&self) {
        log::info!(
            target: TARGET,
            "SimplexDb {}: marked for destroy",
            self.storage_id
        );
        self.storage.mark_for_destroy();
    }
}

impl Drop for SimplexDb {
    fn drop(&mut self) {
        log::info!(
            target: TARGET,
            "SimplexDb {}: dropping, syncing pending writes...",
            self.storage_id
        );

        // Force sync to flush all pending writes before closing
        if let Err(e) = self.sync(Some(DEFAULT_SYNC_TIMEOUT)) {
            log::error!(
                target: TARGET,
                "SimplexDb {}: sync on drop failed: {}",
                self.storage_id,
                e
            );
        } else {
            log::info!(
                target: TARGET,
                "SimplexDb {}: sync complete",
                self.storage_id
            );
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "tests/test_database.rs"]
mod tests;
