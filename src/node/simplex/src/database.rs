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
//! | Notar Cert | Certificate cache | `consensus.simplex.db.key.candidateResolver.notarCert` | `consensus.simplex.db.candidateResolver.notarCert` |
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
    certificate::NotarCert,
};
use consensus_common::{
    AsyncKeyValueStorageOptions, AsyncKeyValueStoragePtr, ConsensusCommonFactory, RawBuffer,
    StorageAsyncResultPtr,
};
use std::{path::Path, sync::Arc, time::Duration};
use ton_api::{
    deserialize_typed, serialize_boxed,
    ton::{
        consensus::{
            candidateid::CandidateId,
            candidateparent::CandidateParent,
            simplex::{
                db::{
                    candidate_resolver::{
                        candidateinfo::CandidateInfo as CandidateInfoValue,
                        notarcert::NotarCert as NotarCertValue,
                        CandidateInfo as CandidateInfoValueBoxed, NotarCert as NotarCertValueBoxed,
                    },
                    finalizedblock::FinalizedBlock as FinalizedBlockValue,
                    key::{
                        candidate_resolver::{
                            candidateinfo::CandidateInfo as CandidateInfoKey,
                            notarcert::NotarCert as NotarCertKey,
                            CandidateInfo as CandidateInfoKeyBoxed, NotarCert as NotarCertKeyBoxed,
                        },
                        finalizedblock::FinalizedBlock as FinalizedBlockKey,
                        vote::Vote as VoteKey,
                        FinalizedBlock as FinalizedBlockKeyBoxed, PoolState as PoolStateKey,
                        Vote as VoteKeyBoxed,
                    },
                    poolstate::PoolState as PoolStateValue,
                    vote::Vote as VoteValue,
                    FinalizedBlock as FinalizedBlockValueBoxed, PoolState as PoolStateValueBoxed,
                    Vote as VoteValueBoxed,
                },
                votesignatureset::VoteSignatureSet,
                VoteSignature as VoteSignatureBoxed,
            },
            CandidateHashData, CandidateParent as CandidateParentBoxed,
        },
        Bool,
    },
    BoxedSerialize, Constructor, IntoBoxed,
};
use ton_block::{error, BlockIdExt, Result, UInt256};

// ============================================================================
// Constants
// ============================================================================

/// Log target for database operations (matches simplex crate log target)
const LOG_TARGET: &str = "simplex";

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

/// Get key prefix for notar certs
fn prefix_notar_cert() -> u32 {
    NotarCertKey::constructor_const()
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

/// Vote record for restart support
///
/// Stores votes by their hash for standstill recovery.
/// Key: vote_hash (sha256 of serialized vote)
/// Value: raw vote data + node index
#[derive(Debug, Clone)]
pub struct VoteRecord {
    /// Hash of the vote (key)
    pub vote_hash: UInt256,
    /// Raw serialized vote data
    pub data: RawBuffer,
    /// Validator index that submitted this vote
    pub node_idx: ValidatorIndex,
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
    /// Votes (for standstill recovery)
    pub votes: Vec<VoteRecord>,
    /// Pool state (for skip vote generation)
    pub pool_state: Option<PoolStateRecord>,
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

/// Bootstrap data for Receiver (notar cert cache)
#[derive(Debug, Clone, Default)]
pub struct ReceiverBootstrap {
    /// Notarization certificates for cache population
    pub notar_certs: Vec<NotarCertRecord>,
}

impl Bootstrap {
    /// Split bootstrap into component-specific parts.
    ///
    /// Consumes self for zero-copy transfer of vectors.
    pub fn split(self) -> (SessionBootstrap, ReceiverBootstrap) {
        (
            SessionBootstrap {
                finalized_blocks: self.finalized_blocks,
                votes: self.votes,
                pool_state: self.pool_state,
            },
            ReceiverBootstrap { notar_certs: self.notar_certs },
        )
    }

    /// Check if this is a fresh start (no persisted state)
    pub fn is_empty(&self) -> bool {
        self.finalized_blocks.is_empty()
            && self.candidate_infos.is_empty()
            && self.notar_certs.is_empty()
            && self.votes.is_empty()
            && self.pool_state.is_none()
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

fn serialize_notar_cert_key(candidate_id: &RawCandidateId) -> Result<Vec<u8>> {
    let key = NotarCertKey { candidateId: raw_candidate_id_to_tl(candidate_id) };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_notar_cert_value(cert: &NotarCert) -> Result<Vec<u8>> {
    let tl_sigs: Vec<VoteSignatureBoxed> = cert.signatures.iter().map(|sig| sig.to_tl()).collect();
    let value = NotarCertValue { notar: VoteSignatureSet { votes: tl_sigs.into() } };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_notar_cert(key_bytes: &[u8], value_bytes: &[u8]) -> Result<NotarCertRecord> {
    let key: NotarCertKey = deserialize_typed::<NotarCertKeyBoxed>(key_bytes)?.only();
    let candidate_id = raw_candidate_id_from_tl(key.candidateId);
    let value: NotarCertValue = deserialize_typed::<NotarCertValueBoxed>(value_bytes)?.only();

    // Receiver expects boxed bytes of `consensus.simplex.voteSignatureSet` (matches C++).
    let notar_cert_bytes = serialize_boxed(&value.notar.into_boxed())
        .map_err(|e| error!("Failed to serialize VoteSignatureSet: {}", e))?;

    Ok(NotarCertRecord { candidate_id, notar_cert_bytes })
}

fn serialize_vote_key(vote_hash: &UInt256) -> Result<Vec<u8>> {
    let key = VoteKey { vote_hash: vote_hash.clone().into() };
    serialize_boxed(&key.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn serialize_vote_value(record: &VoteRecord) -> Result<Vec<u8>> {
    let value =
        VoteValue { data: record.data.clone(), node_idx: record.node_idx.value() as i32, seqno: 0 };
    serialize_boxed(&value.into_boxed()).map_err(|e| error!("serialization failed: {}", e))
}

fn deserialize_vote(key_bytes: &[u8], value_bytes: &[u8]) -> Result<VoteRecord> {
    let key: VoteKey = deserialize_typed::<VoteKeyBoxed>(key_bytes)?.only();
    let value: VoteValue = deserialize_typed::<VoteValueBoxed>(value_bytes)?.only();
    Ok(VoteRecord {
        vote_hash: key.vote_hash.clone(),
        data: value.data,
        node_idx: ValidatorIndex::new(value.node_idx as u32),
    })
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
                        target: LOG_TARGET,
                        "SimplexDb: skipping finalized block slot={} (no chain root, parent is set)",
                        record.candidate_id.slot.value()
                    );
                    continue;
                }
            }
            Some(expected) => {
                if record.parent.as_ref() != Some(expected) {
                    log::warn!(
                        target: LOG_TARGET,
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
            target: LOG_TARGET,
            "SimplexDb {}: opening at {}",
            storage_id,
            db_path.display()
        );

        // SimplexDb does not use callbacks
        let options = AsyncKeyValueStorageOptions { use_callback_thread: false };

        let storage =
            ConsensusCommonFactory::create_async_key_value_storage(db_path, &storage_id, options)?;

        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: opened at {}",
            storage_id,
            db_path.display()
        );

        Ok(Arc::new(Self { storage, storage_id }))
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
        let key = serialize_finalized_block_key(&record.candidate_id)?;
        let value = serialize_finalized_block_value(record)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save finalized block record.
    ///
    /// Called when a block is finalized or notarized with a certificate.
    pub fn save_finalized_block(&self, record: &FinalizedBlockRecord) -> Result<()> {
        log::trace!(
            target: LOG_TARGET,
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
            target: LOG_TARGET,
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
        candidate_id: &RawCandidateId,
        cert: &NotarCert,
    ) -> Result<StorageAsyncResultPtr<()>> {
        let key = serialize_notar_cert_key(candidate_id)?;
        let value = serialize_notar_cert_value(cert)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save notarization certificate.
    ///
    /// Called when notarization is observed.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn save_notar_cert(&self, candidate_id: &RawCandidateId, cert: &NotarCert) -> Result<()> {
        log::trace!(
            target: LOG_TARGET,
            "SimplexDb {}: save_notar_cert slot={} signatures={}",
            self.storage_id,
            candidate_id.slot.value(),
            cert.signatures.len()
        );

        self.save_notar_cert_async(candidate_id, cert)?;
        Ok(())
    }

    /// Save vote record (async result).
    pub fn save_vote_async(&self, record: &VoteRecord) -> Result<StorageAsyncResultPtr<()>> {
        let key = serialize_vote_key(&record.vote_hash)?;
        let value = serialize_vote_value(record)?;
        Ok(self.storage.set(key, value, None))
    }

    /// Save vote record.
    ///
    /// Called when a vote is received (for standstill recovery).
    #[allow(dead_code)] // Convenience wrapper; prefer `_async()` in production code.
    pub fn save_vote(&self, record: &VoteRecord) -> Result<()> {
        log::trace!(
            target: LOG_TARGET,
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
            target: LOG_TARGET,
            "SimplexDb {}: save_pool_state first_nonannounced_window={}",
            self.storage_id,
            record.first_nonannounced_window
        );

        self.save_pool_state_async(record)?;
        Ok(())
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
            target: LOG_TARGET,
            "SimplexDb {}: load_finalized_blocks_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_finalized_block(), None)
    }

    /// Load all candidate infos asynchronously.
    pub fn load_candidate_infos_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: LOG_TARGET,
            "SimplexDb {}: load_candidate_infos_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_candidate_info(), None)
    }

    /// Load all notar certs asynchronously.
    pub fn load_notar_certs_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: LOG_TARGET,
            "SimplexDb {}: load_notar_certs_async",
            self.storage_id
        );
        self.storage.get_by_prefix_u32(prefix_notar_cert(), None)
    }

    /// Load all votes asynchronously.
    pub fn load_votes_async(&self) -> StorageAsyncResultPtr<Vec<(Vec<u8>, Vec<u8>)>> {
        log::debug!(
            target: LOG_TARGET,
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
            target: LOG_TARGET,
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
            target: LOG_TARGET,
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
                        target: LOG_TARGET,
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
            target: LOG_TARGET,
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
            target: LOG_TARGET,
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
                        target: LOG_TARGET,
                        "SimplexDb {}: failed to deserialize candidate info: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        log::info!(
            target: LOG_TARGET,
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
            target: LOG_TARGET,
            "SimplexDb {}: load_notar_certs",
            self.storage_id
        );

        let result = self
            .storage
            .get_by_prefix_u32(prefix_notar_cert(), None)
            .wait_timeout(DEFAULT_SYNC_TIMEOUT)
            .ok_or_else(|| error!("SimplexDb: timeout loading notar certs"))??;

        let mut records = Vec::with_capacity(result.len());
        for (key_bytes, value_bytes) in result {
            match deserialize_notar_cert(&key_bytes, &value_bytes) {
                Ok(record) => records.push(record),
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "SimplexDb {}: failed to deserialize notar cert: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: loaded {} notar certs",
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
            target: LOG_TARGET,
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
                Ok(record) => records.push(record),
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "SimplexDb {}: failed to deserialize vote: {}",
                        self.storage_id,
                        e
                    );
                }
            }
        }

        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: loaded {} votes",
            self.storage_id,
            records.len()
        );

        Ok(records)
    }

    /// Load pool state from DB.
    ///
    /// Returns None if no pool state was saved (first run).
    #[allow(dead_code)] // Not used yet; kept for restart/debug parity.
    pub fn load_pool_state(&self) -> Result<Option<PoolStateRecord>> {
        log::debug!(
            target: LOG_TARGET,
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
                target: LOG_TARGET,
                "SimplexDb {}: no pool state found (first run)",
                self.storage_id
            );
            return Ok(None);
        }

        // Should be exactly one record (singleton)
        if result.len() > 1 {
            log::warn!(
                target: LOG_TARGET,
                "SimplexDb {}: multiple pool state records found ({}), using first",
                self.storage_id,
                result.len()
            );
        }

        let (_key_bytes, value_bytes) = &result[0];
        let record = deserialize_pool_state(value_bytes)?;

        log::info!(
            target: LOG_TARGET,
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
    /// - Votes
    /// - Pool state
    ///
    /// Use `Bootstrap::split()` to decompose into SessionBootstrap and ReceiverBootstrap.
    #[allow(dead_code)] // Prefer `load_bootstrap_cancellable()` in session startup.
    pub fn load_bootstrap(&self) -> Result<Bootstrap> {
        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: load_bootstrap",
            self.storage_id
        );

        let finalized_blocks = self.load_finalized_blocks()?;
        let candidate_infos = self.load_candidate_infos()?;
        let notar_certs = self.load_notar_certs()?;
        let votes = self.load_votes()?;
        let pool_state = self.load_pool_state()?;

        let bootstrap =
            Bootstrap { finalized_blocks, candidate_infos, notar_certs, votes, pool_state };

        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: bootstrap loaded: {} finalized, {} candidates, {} certs, {} votes, pool_state={}",
            self.storage_id,
            bootstrap.finalized_blocks.len(),
            bootstrap.candidate_infos.len(),
            bootstrap.notar_certs.len(),
            bootstrap.votes.len(),
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
            target: LOG_TARGET,
            "SimplexDb {}: load_bootstrap_cancellable",
            self.storage_id
        );

        // Start all async loads in parallel
        let finalized_async = self.load_finalized_blocks_async();
        let candidates_async = self.load_candidate_infos_async();
        let certs_async = self.load_notar_certs_async();
        let votes_async = self.load_votes_async();
        let pool_state_async = self.load_pool_state_async();

        // Wait with cancellation support
        let finalized_raw = finalized_async.wait_cancellable(cancel, step)?;
        let candidates_raw = candidates_async.wait_cancellable(cancel, step)?;
        let certs_raw = certs_async.wait_cancellable(cancel, step)?;
        let votes_raw = votes_async.wait_cancellable(cancel, step)?;
        let pool_state_raw = pool_state_async.wait_cancellable(cancel, step)?;

        // Deserialize results
        let mut finalized_blocks = Vec::with_capacity(finalized_raw.len());
        for (k, v) in finalized_raw {
            match deserialize_finalized_block(&k, &v) {
                Ok(r) => finalized_blocks.push(r),
                Err(e) => {
                    log::error!(target: LOG_TARGET, "SimplexDb: skip bad finalized block: {}", e)
                }
            }
        }
        finalized_blocks.sort_by_key(|r| r.candidate_id.slot);
        let total_finalized_blocks = finalized_blocks.len();
        let finalized_blocks = filter_finalized_chain(finalized_blocks);
        if finalized_blocks.len() != total_finalized_blocks {
            log::warn!(
                target: LOG_TARGET,
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
                    log::error!(target: LOG_TARGET, "SimplexDb: skip bad candidate info: {}", e)
                }
            }
        }

        let mut notar_certs = Vec::with_capacity(certs_raw.len());
        for (k, v) in certs_raw {
            match deserialize_notar_cert(&k, &v) {
                Ok(r) => notar_certs.push(r),
                Err(e) => log::error!(target: LOG_TARGET, "SimplexDb: skip bad notar cert: {}", e),
            }
        }

        let mut votes = Vec::with_capacity(votes_raw.len());
        for (k, v) in votes_raw {
            match deserialize_vote(&k, &v) {
                Ok(r) => votes.push(r),
                Err(e) => log::error!(target: LOG_TARGET, "SimplexDb: skip bad vote: {}", e),
            }
        }

        let pool_state = if pool_state_raw.is_empty() {
            None
        } else {
            let (_k, v) = &pool_state_raw[0];
            match deserialize_pool_state(v) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::error!(target: LOG_TARGET, "SimplexDb: skip bad pool state: {}", e);
                    None
                }
            }
        };

        let bootstrap =
            Bootstrap { finalized_blocks, candidate_infos, notar_certs, votes, pool_state };

        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: bootstrap loaded: {} finalized, {} candidates, {} certs, {} votes, pool_state={}",
            self.storage_id,
            bootstrap.finalized_blocks.len(),
            bootstrap.candidate_infos.len(),
            bootstrap.notar_certs.len(),
            bootstrap.votes.len(),
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
            target: LOG_TARGET,
            "SimplexDb {}: sync",
            self.storage_id
        );
        self.storage.sync(timeout)
    }

    /// Mark database for destruction on drop.
    #[allow(dead_code)] // Used by unit tests in `node/simplex/src/tests/test_database.rs`.
    pub fn mark_for_destroy(&self) {
        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: marked for destroy",
            self.storage_id
        );
        self.storage.mark_for_destroy();
    }
}

impl Drop for SimplexDb {
    fn drop(&mut self) {
        log::info!(
            target: LOG_TARGET,
            "SimplexDb {}: dropping, syncing pending writes...",
            self.storage_id
        );

        // Force sync to flush all pending writes before closing
        if let Err(e) = self.sync(Some(DEFAULT_SYNC_TIMEOUT)) {
            log::error!(
                target: LOG_TARGET,
                "SimplexDb {}: sync on drop failed: {}",
                self.storage_id,
                e
            );
        } else {
            log::info!(
                target: LOG_TARGET,
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
