/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Block candidate compression utilities.
//!
//! Provides LZ4 compression for block candidates, combining block data
//! and collated data into a single compressed BOC format.

use std::sync::Arc;
use ton_block::{
    boc_compression::boc_decompress, error, fail, lz4_compress, lz4_decompress, BocFlags,
    BocReader, BocWriter, Cell, Lz4DecompressMode,
};

/// Compress block candidate data (block + collated data) into a single compressed buffer.
///
/// # Arguments
/// * `block` - Block data as BOC bytes
/// * `collated_data` - Collated data as BOC bytes (can be empty)
///
/// # Returns
/// * `Ok((compressed_data, decompressed_size))` - Compressed data and original size
/// * `Err` - If compression fails
pub fn compress_candidate_data(
    block: &[u8],
    collated_data: &[u8],
) -> crate::Result<(Vec<u8>, usize)> {
    let boc1 = BocReader::new().read(block)?;

    if boc1.roots.len() != 1 {
        fail!("block candidate should have exactly one root");
    }

    let mut roots: Vec<Arc<Cell>> = vec![Arc::new(boc1.roots[0].clone())];

    if !collated_data.is_empty() {
        let boc2 = BocReader::new().read(collated_data)?;

        for i in 0..boc2.roots.len() {
            roots.push(Arc::new(boc2.roots[i].clone()));
        }
    }

    let mut data = Vec::new();
    BocWriter::with_flags(roots.iter().map(|cell| cell.as_ref().clone()), BocFlags::Crc32)?
        .write(&mut data)?;

    let decompressed_size = data.len();
    let compressed = lz4_compress(&data, false)?;

    log::trace!(
        "Compressing block candidate: {} -> {}",
        block.len() + collated_data.len(),
        compressed.len()
    );

    Ok((compressed, decompressed_size))
}

/// Decompress block candidate data back into block and collated data.
///
/// # Arguments
/// * `compressed` - Compressed data from `compress_candidate_data`
/// * `improved_compression` - Whether to use improved compression
/// * `decompressed_size` - Expected decompressed size
/// * `proto_version` - Protocol version
///
/// # Returns
/// * `Ok((block_data, collated_data))` - Decompressed block and collated data
/// * `Err` - If decompression fails or size mismatch
pub fn decompress_candidate_data(
    compressed: &[u8],
    improved_compression: bool,
    decompressed_size: usize,
    proto_version: u32,
) -> crate::Result<(Vec<u8>, Vec<u8>)> {
    let mut roots = if !improved_compression {
        let decompressed =
            lz4_decompress(compressed, Lz4DecompressMode::WithMaxSize(decompressed_size as i32))
                .map_err(|err| error!("Failed to decompress data: {}", err))?;

        if decompressed.len() != decompressed_size {
            fail!(
                "Decompressed size mismatch: expected {}, got {}",
                decompressed_size,
                decompressed.len()
            );
        };
        BocReader::new().read(&decompressed)?.roots
    } else {
        boc_decompress(compressed, decompressed_size)
            .map_err(|err| error!("Failed to decompress data: {}", err))?
    };

    if roots.is_empty() {
        fail!("BOC is empty");
    }

    let mut block_data = Vec::new();
    // Write boc with all possible flags (C++ implementation: mode 31)
    BocWriter::with_flags([roots.remove(0)], BocFlags::all())?.write(&mut block_data)?;

    // Serialize the remaining roots (collated data)
    // Matches C++ ton-node-cpp/validator-session/candidate-serializer.cpp:
    //   int collated_data_mode = proto_version >= 5 ? 2 : 31;
    // Simplex (proto_version >= 5): mode 2 (CRC32 only)
    // Catchain (proto_version < 5): mode 31 (all flags)
    // Note: ton-cpp-testnet simplified to always mode 2 (no catchain support).
    let collated_data_flags = if proto_version >= 5 { BocFlags::Crc32 } else { BocFlags::all() };
    let mut collated_data = Vec::new();
    if !roots.is_empty() {
        BocWriter::with_flags(roots, collated_data_flags)?.write(&mut collated_data)?;
    }

    log::debug!(
        "Decompressing block candidate: {} -> {}",
        compressed.len(),
        block_data.len() + collated_data.len()
    );

    Ok((block_data, collated_data))
}

/// Re-serialize BOC data with the specified flags to produce canonical bytes.
///
/// This is used by the simplex sender to produce the same bytes that the receiver
/// would compute after decompression, ensuring hash consistency for candidate IDs.
///
/// The C++ receiver (types.cpp) hashes the decompressed bytes:
///   file_hash = sha256(block re-serialized with mode 31)
///   collated_file_hash = sha256(collated re-serialized with mode 2)
/// The Rust sender calls canonicalize_boc with matching flags to produce
/// identical bytes, so the resulting CandidateId hashes agree.
///
/// # Arguments
/// * `data` - BOC bytes (any serialization mode)
/// * `flags` - Target BocFlags (e.g., BocFlags::Crc32 for collated, BocFlags::all() for block)
///
/// # Returns
/// * Canonical bytes re-serialized with the given flags, or empty vec if input is empty
pub fn canonicalize_boc(data: &[u8], flags: BocFlags) -> crate::Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let roots = BocReader::new().read(data)?.roots;
    let mut result = Vec::new();
    if !roots.is_empty() {
        BocWriter::with_flags(roots, flags)?.write(&mut result)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::BuilderData;

    fn make_cell(data: &[u8], refs: Vec<Cell>) -> Cell {
        let mut b = BuilderData::new();
        b.append_raw(data, data.len() * 8).unwrap();
        for r in refs {
            b.checked_append_reference(r).unwrap();
        }
        b.into_cell().unwrap()
    }

    /// Helper: test compress→decompress round-trip for a given proto_version.
    /// The collated data flags depend on proto_version:
    ///   proto_version < 5  → BocFlags::all()  (catchain mode)
    ///   proto_version >= 5 → BocFlags::Crc32  (simplex mode)
    /// Block data always uses BocFlags::all().
    fn roundtrip_check(block_root: Cell, collated_roots: Vec<Cell>, proto_version: u32) {
        // Serialize block data (always BocFlags::all())
        let mut block_bytes = Vec::new();
        BocWriter::with_flags([block_root], BocFlags::all())
            .unwrap()
            .write(&mut block_bytes)
            .unwrap();

        // Serialize collated data with the flags matching proto_version
        let collated_data_flags =
            if proto_version >= 5 { BocFlags::Crc32 } else { BocFlags::all() };
        let mut collated_bytes = Vec::new();
        if !collated_roots.is_empty() {
            BocWriter::with_flags(collated_roots, collated_data_flags)
                .unwrap()
                .write(&mut collated_bytes)
                .unwrap();
        }

        // Compress
        let (compressed, decompressed_size) =
            compress_candidate_data(&block_bytes, &collated_bytes).unwrap();

        // Decompress with the same proto_version
        let (rt_block, rt_collated) =
            decompress_candidate_data(&compressed, false, decompressed_size, proto_version)
                .unwrap();

        assert_eq!(
            block_bytes, rt_block,
            "Block data changed after round-trip (proto_version={})",
            proto_version
        );
        assert_eq!(
            collated_bytes, rt_collated,
            "Collated data changed after round-trip (proto_version={})",
            proto_version
        );
    }

    #[test]
    fn test_roundtrip_basic_catchain() {
        // proto_version=0 (catchain): collated data uses BocFlags::all()
        let block_leaf1 = make_cell(&[0xAA; 32], vec![]);
        let block_leaf2 = make_cell(&[0xBB; 32], vec![]);
        let block_root = make_cell(&[0x01; 16], vec![block_leaf1, block_leaf2]);

        let coll_leaf1 = make_cell(&[0xCC; 32], vec![]);
        let coll_leaf2 = make_cell(&[0xDD; 32], vec![]);
        let coll_root1 = make_cell(&[0x02; 16], vec![coll_leaf1]);
        let coll_root2 = make_cell(&[0x03; 16], vec![coll_leaf2]);
        let coll_root3 = make_cell(&[0x04; 8], vec![]);

        roundtrip_check(block_root, vec![coll_root1, coll_root2, coll_root3], 0);
    }

    #[test]
    fn test_roundtrip_basic_simplex() {
        // proto_version=5 (simplex): collated data uses BocFlags::Crc32
        let block_leaf1 = make_cell(&[0xAA; 32], vec![]);
        let block_leaf2 = make_cell(&[0xBB; 32], vec![]);
        let block_root = make_cell(&[0x01; 16], vec![block_leaf1, block_leaf2]);

        let coll_leaf1 = make_cell(&[0xCC; 32], vec![]);
        let coll_leaf2 = make_cell(&[0xDD; 32], vec![]);
        let coll_root1 = make_cell(&[0x02; 16], vec![coll_leaf1]);
        let coll_root2 = make_cell(&[0x03; 16], vec![coll_leaf2]);
        let coll_root3 = make_cell(&[0x04; 8], vec![]);

        roundtrip_check(block_root, vec![coll_root1, coll_root2, coll_root3], 5);
    }

    fn make_deep_tree(depth: usize, tag: u8) -> Cell {
        let mut cell = make_cell(&[tag; 8], vec![]);
        for i in 0..depth {
            let data: Vec<u8> = vec![tag, i as u8, 0, 0, 0, 0, 0, 0];
            cell = make_cell(&data, vec![cell]);
        }
        cell
    }

    fn make_wide_tree(width: usize, depth: usize, tag: u8) -> Cell {
        let mut leaves = Vec::new();
        for i in 0..width {
            let data: Vec<u8> = vec![tag, i as u8, 0, 0, 0, 0, 0, 0];
            leaves.push(make_cell(&data, vec![]));
        }
        let mut level = leaves;
        while level.len() > 1 {
            let mut next = Vec::new();
            for chunk in level.chunks(2) {
                let refs: Vec<Cell> = chunk.to_vec();
                let data: Vec<u8> = vec![tag, next.len() as u8, depth as u8, 0, 0, 0, 0, 0];
                next.push(make_cell(&data, refs));
            }
            level = next;
        }
        level.into_iter().next().unwrap()
    }

    #[test]
    fn test_roundtrip_deep_tree_catchain() {
        let block_root = make_deep_tree(80, 0xAA);
        let coll_root1 = make_deep_tree(80, 0xBB);
        let coll_root2 = make_deep_tree(40, 0xCC);
        let coll_root3 = make_cell(&[0xDD; 8], vec![]);
        roundtrip_check(block_root, vec![coll_root1, coll_root2, coll_root3], 0);
    }

    #[test]
    fn test_roundtrip_deep_tree_simplex() {
        let block_root = make_deep_tree(80, 0xAA);
        let coll_root1 = make_deep_tree(80, 0xBB);
        let coll_root2 = make_deep_tree(40, 0xCC);
        let coll_root3 = make_cell(&[0xDD; 8], vec![]);
        roundtrip_check(block_root, vec![coll_root1, coll_root2, coll_root3], 5);
    }

    #[test]
    fn test_roundtrip_wide_tree_catchain() {
        let block_root = make_wide_tree(128, 7, 0xAA);
        let coll_root1 = make_wide_tree(64, 6, 0xBB);
        let coll_root2 = make_wide_tree(32, 5, 0xCC);
        roundtrip_check(block_root, vec![coll_root1, coll_root2], 0);
    }

    #[test]
    fn test_roundtrip_wide_tree_simplex() {
        let block_root = make_wide_tree(128, 7, 0xAA);
        let coll_root1 = make_wide_tree(64, 6, 0xBB);
        let coll_root2 = make_wide_tree(32, 5, 0xCC);
        roundtrip_check(block_root, vec![coll_root1, coll_root2], 5);
    }

    #[test]
    fn test_roundtrip_merkle_proofs_catchain() {
        use std::collections::HashSet;
        use ton_block::{MerkleProof, Serializable};

        let block_leaf1 = make_cell(&[0xAA; 64], vec![]);
        let block_leaf2 = make_cell(&[0xBB; 64], vec![]);
        let block_mid = make_cell(&[0x11; 32], vec![block_leaf1, block_leaf2]);
        let block_root = make_cell(&[0x01; 16], vec![block_mid]);

        let proof_tree_leaf1 = make_cell(&[0xCC; 64], vec![]);
        let proof_tree_leaf2 = make_cell(&[0xDD; 64], vec![]);
        let proof_tree_mid =
            make_cell(&[0x22; 32], vec![proof_tree_leaf1.clone(), proof_tree_leaf2]);
        let proof_tree_root = make_cell(&[0x02; 16], vec![proof_tree_mid.clone()]);

        let mut proof_hashes = HashSet::new();
        proof_hashes.insert(proof_tree_root.repr_hash());
        proof_hashes.insert(proof_tree_mid.repr_hash());
        proof_hashes.insert(proof_tree_leaf1.repr_hash());

        let merkle_proof =
            MerkleProof::create(&proof_tree_root, |h| proof_hashes.contains(h)).unwrap();
        let proof_cell = merkle_proof.serialize().unwrap();
        let extra_root = make_cell(&[0xEE; 16], vec![]);

        roundtrip_check(block_root, vec![proof_cell, extra_root], 0);
    }

    #[test]
    fn test_roundtrip_merkle_proofs_simplex() {
        use std::collections::HashSet;
        use ton_block::{MerkleProof, Serializable};

        let block_leaf1 = make_cell(&[0xAA; 64], vec![]);
        let block_leaf2 = make_cell(&[0xBB; 64], vec![]);
        let block_mid = make_cell(&[0x11; 32], vec![block_leaf1, block_leaf2]);
        let block_root = make_cell(&[0x01; 16], vec![block_mid]);

        let proof_tree_leaf1 = make_cell(&[0xCC; 64], vec![]);
        let proof_tree_leaf2 = make_cell(&[0xDD; 64], vec![]);
        let proof_tree_mid =
            make_cell(&[0x22; 32], vec![proof_tree_leaf1.clone(), proof_tree_leaf2]);
        let proof_tree_root = make_cell(&[0x02; 16], vec![proof_tree_mid.clone()]);

        let mut proof_hashes = HashSet::new();
        proof_hashes.insert(proof_tree_root.repr_hash());
        proof_hashes.insert(proof_tree_mid.repr_hash());
        proof_hashes.insert(proof_tree_leaf1.repr_hash());

        let merkle_proof =
            MerkleProof::create(&proof_tree_root, |h| proof_hashes.contains(h)).unwrap();
        let proof_cell = merkle_proof.serialize().unwrap();
        let extra_root = make_cell(&[0xEE; 16], vec![]);

        roundtrip_check(block_root, vec![proof_cell, extra_root], 5);
    }

    #[test]
    fn test_roundtrip_shared_cells_catchain() {
        let shared_cell = make_cell(&[0xFF; 32], vec![]);
        let block_root = make_cell(&[0x01; 16], vec![shared_cell.clone()]);
        let coll_root1 = make_cell(&[0x02; 16], vec![shared_cell]);
        let coll_root2 = make_cell(&[0x03; 8], vec![]);
        roundtrip_check(block_root, vec![coll_root1, coll_root2], 0);
    }

    #[test]
    fn test_roundtrip_shared_cells_simplex() {
        let shared_cell = make_cell(&[0xFF; 32], vec![]);
        let block_root = make_cell(&[0x01; 16], vec![shared_cell.clone()]);
        let coll_root1 = make_cell(&[0x02; 16], vec![shared_cell]);
        let coll_root2 = make_cell(&[0x03; 8], vec![]);
        roundtrip_check(block_root, vec![coll_root1, coll_root2], 5);
    }
}
