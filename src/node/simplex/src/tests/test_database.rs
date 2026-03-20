/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for SimplexDb.

use super::*;
use crate::{
    block::{RawCandidateId, SlotIndex, ValidatorIndex},
    certificate::{Certificate, VoteSignature},
    simplex_state::NotarizeVote,
};
use consensus_common::SessionId;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ton_api::ton::consensus::{
    candidatehashdata::CandidateHashDataOrdinary, CandidateHashData, CandidateParent,
};
use ton_block::{BlockIdExt, ShardIdent, UInt256};

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a unique test database root path inside target directory.
fn create_test_db_root(test_name: &str) -> PathBuf {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let random: u32 = rand::random();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test_dbs/simplex")
        .join(format!("{}_{:016x}_{:08x}", test_name, timestamp, random));

    fs::create_dir_all(&path).unwrap();
    path
}

/// Construct DB path matching C++ format (local helper for tests).
/// Format: {db_root}/consensus/consensus.{workchain}.{shard}.{cc_seqno}.{session_id}/
fn make_test_db_path(
    db_root: &Path,
    shard: &ShardIdent,
    catchain_seqno: u32,
    session_id: &SessionId,
) -> PathBuf {
    let db_dir_name = format!(
        "consensus.{}.{:016x}.{}.{}",
        shard.workchain_id(),
        shard.shard_prefix_with_tag(),
        catchain_seqno,
        session_id.to_hex_string()
    );
    db_root.join("consensus").join(db_dir_name)
}

/// Creates a test session ID from a u8 seed for uniqueness
fn create_test_session_id(seed: u8) -> SessionId {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[31] = seed;
    SessionId::from(bytes)
}

/// Creates a test shard (masterchain by default)
fn create_test_shard() -> ShardIdent {
    ShardIdent::masterchain()
}

fn create_test_db(test_name: &str) -> (PathBuf, SimplexDbPtr) {
    let db_root = create_test_db_root(test_name);
    let shard = create_test_shard();
    let session_id = create_test_session_id(0x42);
    let catchain_seqno = 1;

    let db_path = make_test_db_path(&db_root, &shard, catchain_seqno, &session_id);
    let storage_id = session_id.to_hex_string();

    let db = SimplexDb::open(&db_path, &storage_id).expect("Failed to open DB");
    (db_path, db)
}

fn create_candidate_id(slot: u32, hash_byte: u8) -> RawCandidateId {
    let mut hash = [0u8; 32];
    hash[0] = hash_byte;
    RawCandidateId { slot: SlotIndex::new(slot), hash: UInt256::from(hash) }
}

fn create_block_id(seqno: u32) -> BlockIdExt {
    BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: seqno,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    }
}

fn create_candidate_hash_data() -> CandidateHashData {
    // Use CandidateHashDataOrdinary which has block, collated_file_hash, and parent
    CandidateHashData::Consensus_CandidateHashDataOrdinary(CandidateHashDataOrdinary {
        block: BlockIdExt::default(),
        collated_file_hash: UInt256::default().into(),
        parent: CandidateParent::Consensus_CandidateWithoutParents,
    })
}

fn create_notar_cert(slot: u32, block_hash: &UInt256, num_signatures: usize) -> NotarCert {
    let signatures: Vec<VoteSignature> = (0..num_signatures)
        .map(|i| VoteSignature {
            validator_idx: ValidatorIndex::new(i as u32),
            signature: vec![i as u8; 64],
        })
        .collect();

    Certificate {
        vote: NotarizeVote { slot: SlotIndex::new(slot), block_hash: block_hash.clone() },
        signatures,
    }
}

// ============================================================================
// Finalized Block Tests
// ============================================================================

#[test]
fn test_save_and_load_finalized_block() {
    let (_db_root, db) = create_test_db("test_save_and_load_finalized_block");

    let candidate_id = create_candidate_id(1, 0xAA);
    let record = FinalizedBlockRecord {
        candidate_id: candidate_id.clone(),
        block_id: create_block_id(1001),
        parent: None,
        is_final: true,
    };

    db.save_finalized_block(&record).unwrap();
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_finalized_blocks().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].candidate_id.slot, candidate_id.slot);
    assert_eq!(records[0].candidate_id.hash, candidate_id.hash);
    assert_eq!(records[0].block_id.seq_no, 1001);
    assert!(records[0].parent.is_none());
    assert!(records[0].is_final);

    db.mark_for_destroy();
}

#[test]
fn test_save_finalized_block_with_parent() {
    let (_db_root, db) = create_test_db("test_save_finalized_block_with_parent");

    let parent_id = create_candidate_id(0, 0x00);
    let candidate_id = create_candidate_id(1, 0xAA);

    // Save parent record first (chain root with no parent)
    let parent_record = FinalizedBlockRecord {
        candidate_id: parent_id.clone(),
        block_id: create_block_id(1000),
        parent: None,
        is_final: false,
    };
    db.save_finalized_block(&parent_record).unwrap();

    // Save child record with parent reference
    let record = FinalizedBlockRecord {
        candidate_id: candidate_id.clone(),
        block_id: create_block_id(1001),
        parent: Some(parent_id.clone()),
        is_final: false,
    };
    db.save_finalized_block(&record).unwrap();

    db.sync(Some(Duration::from_secs(5))).unwrap();

    // load_finalized_blocks applies chain filtering, so both records should survive
    let records = db.load_finalized_blocks().unwrap();
    assert_eq!(records.len(), 2);

    // First record (slot 0) should have no parent (chain root)
    assert!(records[0].parent.is_none());

    // Second record (slot 1) should have parent reference
    assert!(records[1].parent.is_some());
    let parent = records[1].parent.as_ref().unwrap();
    assert_eq!(parent.slot, parent_id.slot);
    assert_eq!(parent.hash, parent_id.hash);
    assert!(!records[1].is_final);

    db.mark_for_destroy();
}

#[test]
fn test_load_multiple_finalized_blocks_sorted() {
    let (_db_root, db) = create_test_db("test_load_multiple_finalized_blocks_sorted");

    // Save in reverse order
    for slot in (0..5).rev() {
        let record = FinalizedBlockRecord {
            candidate_id: create_candidate_id(slot, slot as u8),
            block_id: create_block_id(1000 + slot),
            parent: if slot > 0 {
                Some(create_candidate_id(slot - 1, (slot - 1) as u8))
            } else {
                None
            },
            is_final: true,
        };
        db.save_finalized_block(&record).unwrap();
    }
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_finalized_blocks().unwrap();
    assert_eq!(records.len(), 5);

    // Should be sorted by slot
    for (i, record) in records.iter().enumerate() {
        assert_eq!(record.candidate_id.slot.value(), i as u32);
    }

    db.mark_for_destroy();
}

// ============================================================================
// Candidate Info Tests
// ============================================================================

#[test]
fn test_save_and_load_candidate_info() {
    let (_db_root, db) = create_test_db("test_save_and_load_candidate_info");

    let candidate_id = create_candidate_id(1, 0xBB);
    let record = CandidateInfoRecord {
        candidate_id: candidate_id.clone(),
        leader_idx: 5,
        candidate_hash_data: create_candidate_hash_data(),
        signature: vec![1, 2, 3, 4, 5],
    };

    db.save_candidate_info(&record).unwrap();
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_candidate_infos().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].candidate_id.slot, candidate_id.slot);
    assert_eq!(records[0].leader_idx, 5);
    assert_eq!(records[0].signature, vec![1, 2, 3, 4, 5]);

    db.mark_for_destroy();
}

#[test]
fn test_load_multiple_candidate_infos() {
    let (_db_root, db) = create_test_db("test_load_multiple_candidate_infos");

    for slot in 0..3 {
        let record = CandidateInfoRecord {
            candidate_id: create_candidate_id(slot, slot as u8),
            leader_idx: slot,
            candidate_hash_data: create_candidate_hash_data(),
            signature: vec![slot as u8; 32],
        };
        db.save_candidate_info(&record).unwrap();
    }
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_candidate_infos().unwrap();
    assert_eq!(records.len(), 3);

    db.mark_for_destroy();
}

// ============================================================================
// Notar Cert Tests
// ============================================================================

#[test]
fn test_save_and_load_notar_cert() {
    let (_db_root, db) = create_test_db("test_save_and_load_notar_cert");

    let candidate_id = create_candidate_id(1, 0xCC);
    let cert = create_notar_cert(1, &candidate_id.hash, 3);

    db.save_notar_cert(&candidate_id, &cert).unwrap();
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_notar_certs().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].candidate_id.slot, candidate_id.slot);
    assert_eq!(records[0].candidate_id.hash, candidate_id.hash);
    // Raw bytes are stored, deserialization tested separately
    assert!(!records[0].notar_cert_bytes.is_empty());

    db.mark_for_destroy();
}

#[test]
fn test_load_multiple_notar_certs() {
    let (_db_root, db) = create_test_db("test_load_multiple_notar_certs");

    for slot in 0..3 {
        let candidate_id = create_candidate_id(slot, slot as u8);
        let cert = create_notar_cert(slot, &candidate_id.hash, 2);
        db.save_notar_cert(&candidate_id, &cert).unwrap();
    }
    db.sync(Some(Duration::from_secs(5))).unwrap();

    let records = db.load_notar_certs().unwrap();
    assert_eq!(records.len(), 3);

    db.mark_for_destroy();
}

// ============================================================================
// Mixed Operations Tests
// ============================================================================

#[test]
fn test_mixed_record_types() {
    let (_db_root, db) = create_test_db("test_mixed_record_types");

    // Save different record types
    let candidate_id = create_candidate_id(1, 0xDD);

    db.save_finalized_block(&FinalizedBlockRecord {
        candidate_id: candidate_id.clone(),
        block_id: create_block_id(1001),
        parent: None,
        is_final: true,
    })
    .unwrap();

    db.save_candidate_info(&CandidateInfoRecord {
        candidate_id: candidate_id.clone(),
        leader_idx: 0,
        candidate_hash_data: create_candidate_hash_data(),
        signature: vec![1, 2, 3],
    })
    .unwrap();

    let cert = create_notar_cert(1, &candidate_id.hash, 2);
    db.save_notar_cert(&candidate_id, &cert).unwrap();

    db.sync(Some(Duration::from_secs(5))).unwrap();

    // Load each type separately
    let finalized = db.load_finalized_blocks().unwrap();
    let infos = db.load_candidate_infos().unwrap();
    let certs = db.load_notar_certs().unwrap();

    assert_eq!(finalized.len(), 1);
    assert_eq!(infos.len(), 1);
    assert_eq!(certs.len(), 1);

    db.mark_for_destroy();
}

// ============================================================================
// Lifecycle Tests
// ============================================================================

#[test]
fn test_db_persistence_across_reopen() {
    let db_root = create_test_db_root("test_db_persistence_across_reopen");
    let shard = create_test_shard();
    let session_id = create_test_session_id(0x99);
    let catchain_seqno = 1;
    let storage_id = session_id.to_hex_string();

    let db_path = make_test_db_path(&db_root, &shard, catchain_seqno, &session_id);

    // Save data
    {
        let db = SimplexDb::open(&db_path, &storage_id).unwrap();
        db.save_finalized_block(&FinalizedBlockRecord {
            candidate_id: create_candidate_id(1, 0xEE),
            block_id: create_block_id(1001),
            parent: None,
            is_final: true,
        })
        .unwrap();
        db.sync(Some(Duration::from_secs(5))).unwrap();
        // db drops here
    }

    // Verify path exists
    assert!(db_path.exists());

    // Reopen and verify
    {
        let db = SimplexDb::open(&db_path, &storage_id).unwrap();
        let records = db.load_finalized_blocks().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].candidate_id.slot.value(), 1);
        db.mark_for_destroy();
    }
}

#[test]
fn test_mark_for_destroy() {
    let db_root = create_test_db_root("test_mark_for_destroy");
    let shard = create_test_shard();
    let session_id = create_test_session_id(0xAA);
    let catchain_seqno = 1;
    let storage_id = session_id.to_hex_string();

    let db_path = make_test_db_path(&db_root, &shard, catchain_seqno, &session_id);

    {
        let db = SimplexDb::open(&db_path, &storage_id).unwrap();
        db.save_finalized_block(&FinalizedBlockRecord {
            candidate_id: create_candidate_id(1, 0xFF),
            block_id: create_block_id(1001),
            parent: None,
            is_final: true,
        })
        .unwrap();
        db.sync(Some(Duration::from_secs(5))).unwrap();
        db.mark_for_destroy();
    } // db drops and destroys

    // Path should be deleted
    assert!(!db_path.exists());
}

#[test]
fn test_empty_db_load() {
    let (_db_root, db) = create_test_db("test_empty_db_load");

    let finalized = db.load_finalized_blocks().unwrap();
    let infos = db.load_candidate_infos().unwrap();
    let certs = db.load_notar_certs().unwrap();

    assert!(finalized.is_empty());
    assert!(infos.is_empty());
    assert!(certs.is_empty());

    db.mark_for_destroy();
}

#[test]
fn test_db_path_format() {
    // Verify the DB path matches C++ format
    let db_root = create_test_db_root("test_db_path_format");
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000000000000000).unwrap(); // workchain 0, shard 0x8000...
    let session_id = create_test_session_id(0xBB);
    let catchain_seqno = 42;
    let storage_id = session_id.to_hex_string();

    let db_path = make_test_db_path(&db_root, &shard, catchain_seqno, &session_id);

    // Verify path format matches C++ format
    // Expected: {db_root}/consensus/consensus.{workchain}.{shard_hex}.{cc_seqno}.{session_id_hex}/
    let expected_db_dir = format!(
        "consensus.{}.{:016x}.{}.{}",
        shard.workchain_id(),
        shard.shard_prefix_with_tag(),
        catchain_seqno,
        session_id.to_hex_string()
    );
    let expected_path = db_root.join("consensus").join(&expected_db_dir);
    assert_eq!(db_path, expected_path, "make_test_db_path should produce correct format");

    // Open DB at that path and verify it works
    let db = SimplexDb::open(&db_path, &storage_id).unwrap();
    assert!(db_path.exists(), "DB path {:?} should exist after open", db_path);

    db.mark_for_destroy();
}
